//! Guest backtraces from `.ARM.exidx` / `.ARM.extab` (ARM EHABI), used to explain stops in stripped libraries.
//!
//! Only the stack-pointer / register-restoring part of the EHABI is implemented (no personality routines,
//! no language-specific data): that is all a backtrace needs.

use crate::Linker;
use bhpc_mem::GuestMemory;

/// One backtrace entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    /// Program counter (return address for every frame but the first).
    pub pc: u32,
    /// Stack pointer of that frame.
    pub sp: u32,
}

const R_SP: usize = 13;
const R_LR: usize = 14;
const R_PC: usize = 15;

/// Sign-extend a `prel31` field and add it to `base`.
fn prel31(base: u32, v: u32) -> u32 {
    let off = ((v << 1) as i32) >> 1;
    base.wrapping_add(off as u32)
}

/// Locate the unwind opcodes for `pc` in the table `(exidx, count)`.
pub(crate) fn opcodes(mem: &GuestMemory, exidx: (u32, u32), pc: u32) -> Option<Vec<u8>> {
    let (table, count) = exidx;
    let fn_of = |i: u32| -> Option<u32> {
        let at = table.wrapping_add(i * 8);
        Some(prel31(at, mem.read_u32(at).ok()?))
    };
    // Last entry whose function starts at or before pc.
    let (mut lo, mut hi) = (0u32, count);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if fn_of(mid)? <= pc {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    let entry = table.wrapping_add(lo.checked_sub(1)? * 8);
    let word = mem.read_u32(entry + 4).ok()?;
    if word == 1 {
        return None; // EXIDX_CANTUNWIND
    }
    if word & 0x8000_0000 != 0 {
        return Some(vec![(word >> 16) as u8, (word >> 8) as u8, word as u8]);
    }
    let extab = prel31(entry + 4, word);
    let first = mem.read_u32(extab).ok()?;
    let (mut ops, extra, mut at) = if first & 0x8000_0000 != 0 {
        // Compact model: personality index in bits 27..24; 1 and 2 carry a word count.
        let extra = if (first >> 24) & 0xF == 0 { 0 } else { (first >> 16) & 0xFF };
        let head = if extra == 0 { vec![(first >> 16) as u8] } else { vec![] };
        let mut v = head;
        v.push((first >> 8) as u8);
        v.push(first as u8);
        (v, extra, extab + 4)
    } else {
        // Generic model: personality pointer, then a word holding the length and three opcodes.
        let w = mem.read_u32(extab + 4).ok()?;
        (vec![(w >> 16) as u8, (w >> 8) as u8, w as u8], w >> 24, extab + 8)
    };
    for _ in 0..extra {
        let w = mem.read_u32(at).ok()?;
        ops.extend_from_slice(&w.to_be_bytes());
        at += 4;
    }
    Some(ops)
}

/// Run unwind opcodes over `regs`; `false` when an opcode is not understood or memory is unreadable.
pub(crate) fn execute(mem: &GuestMemory, ops: &[u8], regs: &mut [u32; 16]) -> bool {
    let mut i = 0;
    let pop = |regs: &mut [u32; 16], mask: u32| -> bool {
        for r in 0..16 {
            if mask & (1 << r) != 0 {
                match mem.read_u32(regs[R_SP]) {
                    Ok(v) => regs[r] = v,
                    Err(_) => return false,
                }
                regs[R_SP] = regs[R_SP].wrapping_add(4);
            }
        }
        true
    };
    let mut pc_set = false;
    while i < ops.len() {
        let op = ops[i];
        i += 1;
        match op {
            0x00..=0x3F => regs[R_SP] = regs[R_SP].wrapping_add((u32::from(op & 0x3F) << 2) + 4),
            0x40..=0x7F => regs[R_SP] = regs[R_SP].wrapping_sub((u32::from(op & 0x3F) << 2) + 4),
            0x80..=0x8F => {
                let Some(&b) = ops.get(i) else { return false };
                i += 1;
                let mask = (u32::from(op & 0x0F) << 8 | u32::from(b)) << 4;
                if mask == 0 {
                    return false; // "refuse to unwind"
                }
                pc_set |= mask & (1 << R_PC) != 0;
                if !pop(regs, mask) {
                    return false;
                }
            }
            0x90..=0x9F if op & 0xF != 13 && op & 0xF != 15 => regs[R_SP] = regs[usize::from(op & 0xF)],
            0xA0..=0xA7 => {
                if !pop(regs, ((1u32 << ((op & 7) + 1)) - 1) << 4) {
                    return false;
                }
            }
            0xA8..=0xAF => {
                if !pop(regs, (((1u32 << ((op & 7) + 1)) - 1) << 4) | 1 << R_LR) {
                    return false;
                }
            }
            0xB0 => break,
            0xB1 => {
                let Some(&b) = ops.get(i) else { return false };
                i += 1;
                if b == 0 || b & 0xF0 != 0 || !pop(regs, u32::from(b)) {
                    return false;
                }
            }
            0xB2 => {
                let mut shift = 0;
                let mut v = 0u32;
                loop {
                    let Some(&b) = ops.get(i) else { return false };
                    i += 1;
                    v |= u32::from(b & 0x7F) << shift;
                    shift += 7;
                    if b & 0x80 == 0 || shift > 28 {
                        break;
                    }
                }
                regs[R_SP] = regs[R_SP].wrapping_add(0x204 + (v << 2));
            }
            0xB3 | 0xC8 | 0xC9 => {
                let Some(&b) = ops.get(i) else { return false };
                i += 1;
                let n = u32::from(b & 0xF) + 1;
                regs[R_SP] = regs[R_SP].wrapping_add(n * 8 + u32::from(op == 0xB3) * 4);
            }
            0xB8..=0xBF | 0xD0..=0xD7 => {
                let n = u32::from(op & 7) + 1;
                regs[R_SP] = regs[R_SP].wrapping_add(n * 8 + u32::from(op < 0xC0) * 4);
            }
            _ => return false,
        }
    }
    if !pc_set {
        regs[R_PC] = regs[R_LR];
    }
    true
}

/// Unwind `regs` (r0..r15 at the stop) through the EHABI tables. `find` maps a pc to its module's
/// `(exidx, count)`. The first frame is the stop itself; unwinding ends at an unknown module, a
/// `CANTUNWIND` entry, a zero or non-advancing pc, or after `max` frames.
pub fn backtrace(
    mem: &GuestMemory,
    find: &dyn Fn(u32) -> Option<(u32, u32)>,
    mut regs: [u32; 16],
    max: usize,
) -> Vec<Frame> {
    let mut out = Vec::new();
    while out.len() < max {
        out.push(Frame { pc: regs[R_PC], sp: regs[R_SP] });
        // Return addresses point after the call: look up the call instruction itself.
        let probe = if out.len() == 1 { regs[R_PC] } else { regs[R_PC].wrapping_sub(1) };
        let Some(ex) = find(probe) else { break };
        let Some(ops) = opcodes(mem, ex, probe) else { break };
        let before = (regs[R_PC], regs[R_SP]);
        if !execute(mem, &ops, &mut regs) || regs[R_PC] == 0 || (regs[R_PC], regs[R_SP]) == before {
            break;
        }
    }
    out
}

impl Linker {
    /// Guest backtrace for `regs` (r0..r15), at most `max` frames; see [`backtrace`].
    pub fn backtrace(&self, regs: [u32; 16], max: usize) -> Vec<Frame> {
        backtrace(&self.mem, &|pc| self.find_exidx(pc), regs, max)
    }
}

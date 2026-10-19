//! Hardware division for the guest's software division helpers.
//!
//! The game targets ARMv7-A without the divide extension, so every `/` and `%` on `int` calls libgcc's helpers
//! (`__aeabi_uidiv`, `__aeabi_idivmod`, `__modsi3`, ... exported by `libv.so`): a shift-and-subtract loop of a few
//! dozen instructions. Profiles of world creation show them at about 2% of the main guest thread. The JIT
//! translates `udiv`/`sdiv` to the host's divide instruction, so each helper's entry is redirected to a four- to
//! six-instruction routine that divides in hardware.
//!
//! Semantics are exactly the helpers': quotients truncate toward zero, remainders take the dividend's sign, and
//! `INT_MIN / -1` wraps to `INT_MIN` with remainder 0 (what both libgcc and `sdiv` give). A zero divisor is not
//! handled here: the routine jumps to the helper's own zero-divisor path (which the patch leaves intact), so the
//! guest still gets its saturated result and `__aeabi_idiv0`. Registers follow the helpers' contract (only r0-r3
//! change). A helper whose prologue does not have the expected zero-divisor test is left alone, as is any Thumb
//! entry. `BHPC_GUEST_HLE=0` turns the patch off.

use bhpc_mem::{GuestMemory, MemError};

/// Environment variable that disables the patch when set to `0`.
pub const DISABLE_ENV: &str = "BHPC_GUEST_HLE";

/// Whether the patch applies, given the value of [`DISABLE_ENV`].
pub fn enabled(v: Option<&std::ffi::OsStr>) -> bool {
    v.is_none_or(|v| v != "0")
}

/// What a helper computes from `r0 = n`, `r1 = d`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// `r0 = n / d` (unsigned).
    UDiv,
    /// `r0 = n / d` (signed).
    SDiv,
    /// `r0 = n / d`, `r1 = n % d` (unsigned).
    UDivMod,
    /// `r0 = n / d`, `r1 = n % d` (signed).
    SDivMod,
    /// `r0 = n % d` (unsigned).
    UMod,
    /// `r0 = n % d` (signed).
    SMod,
}

/// The helpers and what they compute (aliases share an address and are patched once).
pub const HELPERS: [(&str, Op); 8] = [
    ("__aeabi_uidiv", Op::UDiv),
    ("__udivsi3", Op::UDiv),
    ("__aeabi_idiv", Op::SDiv),
    ("__divsi3", Op::SDiv),
    ("__aeabi_uidivmod", Op::UDivMod),
    ("__aeabi_idivmod", Op::SDivMod),
    ("__umodsi3", Op::UMod),
    ("__modsi3", Op::SMod),
];

const CMP_R1_0: u32 = 0xE351_0000; // cmp r1, #0
const SUBS_R2_R1_1: u32 = 0xE251_2001; // subs r2, r1, #1
const BX_LR: u32 = 0xE12F_FF1E;
const LDR_PC_LITERAL: u32 = 0xE51F_F004; // ldr pc, [pc, #-4]
/// `ldreq pc, [pc, #imm]` without the offset.
const LDREQ_PC: u32 = 0x059F_F000;
/// Bytes reserved per routine.
const SLOT: u32 = 32;

/// Target of the ARM `b` instruction `word` at `pc` when its condition is `cond`.
fn branch(word: u32, pc: u32, cond: u32) -> Option<u32> {
    if word >> 24 != (cond << 4 | 0xA) {
        return None;
    }
    let off = ((word << 8) as i32 >> 6) as u32;
    Some(pc.wrapping_add(8).wrapping_add(off))
}

/// Where the helper at `entry` goes for a zero divisor: `cmp r1, #0; beq Z` or libgcc's `subs r2, r1, #1` followed
/// within two instructions by `blo Z` (`bxeq lr` / flag-setting tests in between).
pub fn zero_path(mem: &GuestMemory, entry: u32) -> Option<u32> {
    let w = |i: u32| mem.read_u32(entry + 4 * i).ok();
    match w(0)? {
        CMP_R1_0 => branch(w(1)?, entry + 4, 0x0),
        SUBS_R2_R1_1 => (1..=2).find_map(|i| branch(w(i)?, entry + 4 * i, 0x3)),
        _ => None,
    }
}

/// The replacement routine for `op` whose zero-divisor case continues at `zero`.
pub fn routine(op: Op, zero: u32) -> Vec<u32> {
    let body: &[u32] = match op {
        Op::UDiv => &[0xE730_F110],                              // udiv r0, r0, r1
        Op::SDiv => &[0xE710_F110],                              // sdiv r0, r0, r1
        Op::UDivMod => &[0xE732_F110, 0xE061_0192, 0xE1A0_0002], // udiv r2,r0,r1; mls r1,r2,r1,r0; mov r0,r2
        Op::SDivMod => &[0xE712_F110, 0xE061_0192, 0xE1A0_0002], // sdiv r2,r0,r1; mls r1,r2,r1,r0; mov r0,r2
        Op::UMod => &[0xE732_F110, 0xE060_0192],                 // udiv r2,r0,r1; mls r0,r2,r1,r0
        Op::SMod => &[0xE712_F110, 0xE060_0192],                 // sdiv r2,r0,r1; mls r0,r2,r1,r0
    };
    // cmp r1,#0; ldreq pc,=zero; body; bx lr; .word zero (the literal follows bx lr; pc reads 8 ahead).
    let literal = 2 + body.len() as u32 + 1;
    let mut v = vec![CMP_R1_0, LDREQ_PC | ((literal - 3) * 4)];
    v.extend_from_slice(body);
    v.push(BX_LR);
    v.push(zero);
    v
}

/// Redirect every helper `export` finds (ARM entries with a recognised prologue) to its hardware routine, written
/// to a freshly mapped page. Returns the patched names. The caller must drop translated code afterwards.
pub fn install(mem: &GuestMemory, export: &dyn Fn(&str) -> Option<u32>) -> Result<Vec<&'static str>, MemError> {
    let mut page = None;
    let mut done: Vec<u32> = Vec::new();
    let mut patched = Vec::new();
    for (name, op) in HELPERS {
        let Some(entry) = export(name).filter(|e| e & 1 == 0) else { continue };
        if done.contains(&entry) {
            patched.push(name);
            continue;
        }
        let Some(zero) = zero_path(mem, entry) else { continue };
        let base = match page {
            Some(b) => b,
            None => *page.insert(mem.map_anywhere(0x1000, 0x1000)?),
        };
        let at = base + SLOT * done.len() as u32;
        for (i, w) in routine(op, zero).into_iter().enumerate() {
            mem.write_u32(at + 4 * i as u32, w)?;
        }
        mem.write_u32(entry, LDR_PC_LITERAL)?;
        mem.write_u32(entry + 4, at)?;
        done.push(entry);
        patched.push(name);
    }
    Ok(patched)
}

/// [`install`] for each of `modules` modules (`export(i, name)` looks `name` up in module `i`) unless `env`, the value
/// of [`DISABLE_ENV`], turns the patch off. Returns the number of patched helper names.
pub fn install_all(
    mem: &GuestMemory,
    env: Option<&std::ffi::OsStr>,
    modules: usize,
    export: &dyn Fn(usize, &str) -> Option<u32>,
) -> Result<usize, MemError> {
    if !enabled(env) {
        return Ok(0);
    }
    let mut n = 0;
    for i in 0..modules {
        n += install(mem, &|name| export(i, name))?.len();
    }
    Ok(n)
}

//! A stale timer fire must not put a run loop to sleep.
//!
//! Apportable's `libSystem.so` emulates Mach timer ports (`mk_timer_*`, used by CoreFoundation for every run loop's
//! timers) with a timer thread: when an armed deadline passes, it queues a message on the port. The receive side
//! (`mach_msg` on the run loop's port set, internal function in libSystem) then reads the port's *current* deadline
//! and, while that lies in the future, `usleep`s until it, holding up the receiving thread.
//!
//! That only works if nothing re-arms the timer between the fire and the receive. CoreFoundation does exactly that: a
//! run loop re-arms its timer port for its next timer right after it arms it for one that is due now, so a fire that
//! the timer thread queues in between is received with a deadline seconds away. The receiving thread (the Android UI
//! thread, which delivers every click) then sleeps for those seconds: the game keeps drawing, but its buttons react
//! seconds or half a minute late. With the guest threads running one at a time this window was a few instructions
//! wide; with parallel guest threads (`docs/perf/parallel-guest-threads.md`) the timer thread lands in it in most
//! sessions.
//!
//! The fix makes the receive behave like the Mach kernel, where a queued fire is delivered at once and re-arming does
//! not take it back: when the deadline is still in the future, the receive skips the sleep and the disarm, and
//! returns the message. CoreFoundation treats it as an early wake-up (no timer due, it arms the next one again) and
//! the timer thread fires again at the new deadline, which the receive then disarms as before. The patch is one
//! branch in guest memory (never in the game files), placed where the sleep loop's code matches [`LOOP`].

use bhpc_loader::{Segment, PF_X};
use bhpc_mem::{GuestMemory, MemError};

/// The library that has the loop.
pub const LIBRARY: &str = "libSystem.so";

/// The receive's sleep loop: the test "deadline later than now", the `usleep((deadline - now) / 1000)` loop body, the
/// disarm (`deadline = never`) and the first instruction after it (loading the port to unlock it). `None` stands for a
/// `bl` (any target: the division helper, `usleep`, the clock).
pub const LOOP: [Option<u32>; 27] = [
    Some(0xE59D_001C), // ldr r0, [sp, #28]      ; deadline > now
    Some(0xE310_0001), // tst r0, #1
    Some(0x0A00_0011), // beq disarm
    Some(0xE51B_001C), // ldr r0, [r11, #-28]    ; loop body: the sleep (patched)
    Some(0xE590_1000),
    Some(0xE590_0004),
    Some(0xE51B_2028),
    Some(0xE51B_3024),
    Some(0xE051_1002),
    Some(0xE0C0_0003),
    Some(0xE3A0_2FFA),
    Some(0xE3A0_3000),
    Some(0xE58D_0018),
    Some(0xE1A0_0001),
    Some(0xE59D_1018),
    None, // bl __aeabi_uldivmod
    None, // bl usleep
    None, // bl <now>
    Some(0xE50B_1024),
    Some(0xE50B_0028),
    Some(0xEAFF_FFCE), // b <loop test>
    Some(0xE51B_001C), // disarm: deadline = never
    Some(0xE3E0_1000),
    Some(0xE580_1004),
    Some(0xE3E0_1001),
    Some(0xE580_1000),
    Some(0xE51B_0008), // ldr r0, [r11, #-8]    ; after the disarm: unlock the port, build the message
];
/// Index in [`LOOP`] of the word the patch replaces (the loop body's first instruction).
pub const PATCH_AT: usize = 3;
/// Index in [`LOOP`] the patch branches to (past the disarm).
pub const RESUME_AT: usize = 26;

/// Whether `w` is an ARM `bl` (any condition except the unconditional space).
fn is_bl(w: u32) -> bool {
    w & 0x0F00_0000 == 0x0B00_0000 && w >> 28 != 0xF
}

/// Byte offset (word aligned) in `code` where [`LOOP`] starts, if it occurs.
pub fn find(code: &[u8]) -> Option<usize> {
    let words: Vec<u32> = code.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    words
        .windows(LOOP.len())
        .position(|win| win.iter().zip(&LOOP).all(|(w, p)| p.map_or(is_bl(*w), |p| p == *w)))
        .map(|i| i * 4)
}

/// An ARM `b` from `from` to `to` (both word aligned, within ±32 MiB).
pub fn branch(from: u32, to: u32) -> u32 {
    0xEA00_0000 | ((to.wrapping_sub(from.wrapping_add(8)) >> 2) & 0x00FF_FFFF)
}

/// Patch the loop in the `len` bytes of guest code at `start`; the patched address if the loop was there.
pub fn install(mem: &GuestMemory, start: u32, len: u32) -> Result<Option<u32>, MemError> {
    let code = mem.read_bytes(start, len)?;
    let Some(off) = find(&code) else { return Ok(None) };
    let at = start + off as u32;
    let (from, to) = (at + 4 * PATCH_AT as u32, at + 4 * RESUME_AT as u32);
    mem.write_u32(from, branch(from, to))?;
    Ok(Some(from))
}

/// Patch the loop in the executable segments of every [`LIBRARY`] among `modules` (name, segments); the patched
/// addresses.
pub fn install_all<'a>(
    mem: &GuestMemory,
    modules: impl IntoIterator<Item = (&'a str, &'a [Segment])>,
) -> Result<Vec<u32>, MemError> {
    let mut out = Vec::new();
    for (_, segments) in modules.into_iter().filter(|(n, _)| n.ends_with(LIBRARY)) {
        for s in segments.iter().filter(|s| s.prot & PF_X != 0) {
            out.extend(install(mem, s.start, s.len)?);
        }
    }
    Ok(out)
}

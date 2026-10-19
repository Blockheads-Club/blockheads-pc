//! "Unlock 30 FPS cap": the game asks for every second display refresh, this lets it draw at every one, at any rate.
//!
//! The game (`-[EvolutionViewController updateFrameInterval]`) sets `animationFrameInterval` to 2, which reaches
//! Apportable's `-[CADisplayLink setFrameInterval:]`. Apportable's display link is a repeating `NSTimer` of
//! `frameInterval / 60.0` seconds (`_startTimer`; `fire` uses the same 60.0 for `duration` and its adaptive mode), so
//! the game draws 30 frames a second. Its simulation runs on measured time (`drawFrame` passes the real time since the
//! last frame, clamped to 0.1 s, to `update:accurateDT:`), so more frames make it smoother, not faster.
//!
//! With the unlock the emulated display refreshes at [`UNLOCKED_HZ`] (the 60.0 constants of the display link's methods,
//! patched in guest memory, never in the game files) and the game's normal interval 2 becomes 1: the timer no longer
//! limits anything and the frame-rate cap (`--fps-limit`, vsync) alone decides the rate. Longer intervals (the game's
//! 2 fps idle mode asks for 30) keep their length in seconds.
//!
//! The methods are found through the Objective-C class metadata (`OBJC_CLASS_$_CADisplayLink`, exported) before the
//! runtime touches it: method lists then still hold selector names as C strings.

use bhpc_mem::GuestMemory;

/// Refresh rate of the emulated display with the unlock (the game's normal mode then times frames at this rate).
pub const UNLOCKED_HZ: f64 = 1000.0;
/// Apportable's display refresh rate.
pub const DEVICE_HZ: f64 = 60.0;
/// Bytes after the last method's start that still belong to the class's code (its literal pool).
const TAIL: u32 = 0x100;

/// The frame interval to use instead of the one the game asked for (`setFrameInterval:`), with the display running at
/// [`UNLOCKED_HZ`].
pub fn unlocked_interval(requested: i32) -> i32 {
    if requested <= 2 {
        1
    } else {
        (f64::from(requested) * UNLOCKED_HZ / DEVICE_HZ).round() as i32
    }
}

/// Implementation of the method `sel` of a not yet realised ObjC class (`class`: the `OBJC_CLASS_$_` address;
/// `meta`: class methods). 32-bit ABI: `class_t.data` at +16, `class_ro_t.baseMethods` at +20, method entries
/// `{name, types, imp}` after an `{entsize, count}` header.
pub fn find_method(mem: &GuestMemory, class: u32, sel: &str, meta: bool) -> Option<u32> {
    methods(mem, class, meta).into_iter().find_map(|(name, imp)| (name == sel).then_some(imp))
}

/// Selector names and implementations of a class's method list (empty when it cannot be read).
pub fn methods(mem: &GuestMemory, class: u32, meta: bool) -> Vec<(String, u32)> {
    let read = || -> Option<Vec<(String, u32)>> {
        let class = if meta { mem.read_u32(class).ok()? } else { class };
        let ro = mem.read_u32(class + 16).ok()? & !3;
        let list = mem.read_u32(ro + 20).ok()?;
        if list == 0 {
            return Some(Vec::new());
        }
        let size = mem.read_u32(list).ok()? & !3;
        let count = mem.read_u32(list + 4).ok()?;
        let mut out = Vec::new();
        for i in 0..count.min(4096) {
            let m = list + 8 + i * size;
            let name = mem.read_cstr(mem.read_u32(m).ok()?).ok()?;
            out.push((name, mem.read_u32(m + 8).ok()?));
        }
        Some(out)
    };
    read().unwrap_or_default()
}

/// Replace every 4-byte aligned `60.0` double in the code of the given methods (from the first one's start to
/// [`TAIL`] bytes after the last one's) by `hz`. Returns how many were replaced.
pub fn patch_refresh_rate(mem: &GuestMemory, imps: &[u32], hz: f64) -> usize {
    let (Some(&lo), Some(&hi)) = (imps.iter().min(), imps.iter().max()) else {
        return 0;
    };
    let (lo, hi) = (lo & !3, (hi & !3) + TAIL);
    let Ok(code) = mem.read_bytes(lo, hi - lo) else {
        return 0;
    };
    let old = DEVICE_HZ.to_le_bytes();
    let mut n = 0;
    for off in (0..code.len().saturating_sub(7)).step_by(4) {
        if code[off..off + 8] == old && mem.write_bytes(lo + off as u32, &hz.to_le_bytes()).is_ok() {
            n += 1;
        }
    }
    n
}

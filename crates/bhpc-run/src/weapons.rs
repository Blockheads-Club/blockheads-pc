//! Weapon skins (mod system M10): the texture of a held sprite item per blockhead, with the inline-hook primitive.
//!
//! The game draws a held tool or item as a sprite inside the big blockhead drawing function
//! (`libApplication.so` + [`DRAW`]): it binds the shared `Items` atlas, asks the exported `texCoordsForItemType`
//! for the item's rectangle in that atlas and draws a quad. There is no per-blockhead item texture, and the atlas is
//! shared with every UI icon, so a skin needs two things the ObjC swizzles cannot give:
//!
//! 1. which blockhead is drawing: before hooks on the three entry points the game draws a blockhead with
//!    (`-[Blockhead draw:...]` the body, `drawTransparentInventoryItem:...` the item in its hand while it is used,
//!    `drawForButton...` the portrait in the HUD) remember it for the thread;
//! 2. an inline hook on `texCoordsForItemType`: called from inside that function (the return address is in
//!    [`DRAW`]) for a blockhead whose player wears a weapon cosmetic for that item, it binds the mod's texture
//!    (through the game's own `glBindTexture`) and answers with the mod texture's rectangle. Everywhere else the
//!    call goes straight on to the original (`tail_call` to the trampoline).
//!
//! The hook is only installed while a loaded mod declares a weapon cosmetic, and only on the game build whose
//! function prologue matches [`PROLOGUE`] (the address range is specific to it).
//!
//! Dev aid: `BHPC_DEV_WEAPON_ITEM=<item type>` shows the weapon cosmetic a blockhead wears (see
//! `BHPC_DEV_COSMETICS=weapon=<key>`) on that item, whatever its `base` says.

use std::cell::Cell;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use bhpc_cpu::Cpu;

use crate::cosmetics;

/// Offset of the blockhead drawing function in `libApplication.so` of the 1.7.6 build, and where it ends.
const DRAW: std::ops::Range<u32> = 0xc0_0780..0xc0_1a98;
/// Its first four instructions (`push {r4-r8, r10, r11, lr}; add r11, sp, #24; sub sp, sp, #1152; bic sp, sp, #15`).
const PROLOGUE: [u32; 4] = [0xe92d_4df0, 0xe28d_b018, 0xe24d_dd12, 0xe3cd_d00f];
/// `GL_TEXTURE_2D`.
const TEXTURE_2D: u32 = 0xde1;

thread_local! {
    /// Which draw entry point runs (a label, for the trace).
    static KIND: Cell<&'static str> = const { Cell::new("") };
    /// The blockhead the world is drawing on this thread (0: none, or a draw that is not the world's).
    static CURRENT: Cell<u32> = const { Cell::new(0) };
}

/// The blockhead the draw hooks saw last (`0` outside the world's blockhead drawing).
pub fn current() -> u32 {
    CURRENT.with(Cell::get)
}

/// `-[Blockhead draw:...]` starts for `bh`.
pub fn drawing(bh: u32) {
    CURRENT.with(|c| c.set(bh));
}

/// A draw entry point ran (`BHPC_WEAPONS_TRACE=1` logs the first calls of each).
pub fn entered(which: &'static str, bh: u32) {
    drawing(bh);
    KIND.with(|k| k.set(which));
    if std::env::var_os("BHPC_WEAPONS_TRACE").is_some() {
        static N: AtomicU32 = AtomicU32::new(0);
        if N.fetch_add(1, Ordering::Relaxed) < 400 {
            log(&format!("{which}: {bh:#x}"));
        }
    }
}

static ORIG: AtomicU32 = AtomicU32::new(0);
static BIND: AtomicU32 = AtomicU32::new(0);
static BASE: AtomicU32 = AtomicU32::new(0);
static THUNK: OnceLock<u32> = OnceLock::new();
static STATE: AtomicU32 = AtomicU32::new(OFF);
const OFF: u32 = 0;
const ON: u32 = 1;
/// The build is not the known one, or hooking failed: do not try again this run.
const GAVE_UP: u32 = 2;

fn log(msg: &str) {
    eprintln!("[weapons] {msg}");
}

/// `BHPC_DEV_WEAPON_ITEM`.
fn dev_item() -> Option<u16> {
    std::env::var("BHPC_DEV_WEAPON_ITEM").ok()?.parse().ok()
}

/// Installs or removes the hook as mods declare weapon cosmetics (once per frame on the main thread).
pub fn sync(c: &mut Cpu) {
    let want = cosmetics::has_weapons();
    match (want, STATE.load(Ordering::Relaxed)) {
        (true, OFF) => install(c),
        (false, ON) => remove(c),
        _ => {}
    }
}

/// Takes the hook out (the session ends).
pub fn remove(c: &mut Cpu) {
    if STATE.load(Ordering::Relaxed) == ON {
        if let Some(n) = crate::native::get() {
            n.unhook_internal(c);
        }
        STATE.store(OFF, Ordering::Relaxed);
        log("hook removed");
    }
}

fn install(c: &mut Cpu) {
    let Some(n) = crate::native::get() else { return };
    let Some(base) = n.module_base("libApplication.so") else { return };
    let words: Vec<u32> = (0..4).map(|i| c.mem().read_u32(base + DRAW.start + 4 * i).unwrap_or(0)).collect();
    if words != PROLOGUE {
        log("weapon skins are off: this is not the game build they know");
        STATE.store(GAVE_UP, Ordering::Relaxed);
        return;
    }
    let Some(bind) = n.import_target(c, "libApplication.so", "__wrap_glBindTexture") else {
        log("weapon skins are off: glBindTexture is not imported");
        STATE.store(GAVE_UP, Ordering::Relaxed);
        return;
    };
    let thunk = match THUNK.get() {
        Some(t) => *t,
        None => {
            let t = c.register_shared_thunk("bhmod:weapon_texcoords", Arc::new(tex_coords)).unwrap_or(0);
            let _ = THUNK.set(t);
            t
        }
    };
    match n.hook_internal(c, "_Z20texCoordsForItemType8ItemType", thunk) {
        Ok(orig) => {
            ORIG.store(orig, Ordering::Relaxed);
            BIND.store(bind, Ordering::Relaxed);
            BASE.store(base, Ordering::Relaxed);
            STATE.store(ON, Ordering::Relaxed);
            log("hook installed on texCoordsForItemType");
        }
        Err(e) => {
            log(&format!("weapon skins are off: {e}"));
            STATE.store(GAVE_UP, Ordering::Relaxed);
        }
    }
}

/// The replacement of `texCoordsForItemType(ItemType) -> Vector4` (result through `r0`, the item in `r1`).
fn tex_coords(c: &mut Cpu) {
    let (sret, item, lr) = (c.reg(0), c.reg(1), c.lr());
    let base = BASE.load(Ordering::Relaxed);
    let in_draw = lr >= base.wrapping_add(DRAW.start) && lr < base.wrapping_add(DRAW.end);
    if in_draw && std::env::var_os("BHPC_WEAPONS_TRACE").is_some() {
        thread_local! { static LAST: Cell<(u32, &'static str, u32)> = const { Cell::new((u32::MAX, "", 0)) }; }
        let now = (item, KIND.with(Cell::get), current());
        if LAST.with(|l| l.replace(now)) != now {
            log(&format!("sprite item {} in {} of {:#x} (lr {:#x})", now.0, now.1, now.2, lr.wrapping_sub(base)));
        }
    }
    if in_draw && current() != 0 {
        if let Some((name, uv)) = cosmetics::weapon_skin(c, current(), item as u16, dev_item()) {
            for (i, f) in uv.iter().enumerate() {
                let _ = c.mem().write_u32(sret + 4 * i as u32, f.to_bits());
            }
            let _ = c.call(BIND.load(Ordering::Relaxed), &[TEXTURE_2D, name]);
            c.ret_u32(sret);
            return;
        }
    }
    c.tail_call(ORIG.load(Ordering::Relaxed));
}

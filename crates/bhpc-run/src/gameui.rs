//! The game's own on-screen controls, seen and steered from the host: "Hide game buttons", the controller's cursor
//! grid, walking, zoom and pause.
//!
//! The game draws its in-world controls itself, with GL, from a small Objective-C view tree of its own (not cocos2d,
//! not UIKit): `-[GameView render:]` renders the `World`, whose `UIManager` renders, in order, the `WorldUI` (the
//! heads-up buttons: inventory, blockhead portraits, pause, time crystals, the "use" button, chat), the `DPad` and
//! the pop-up `GameUIView`s (crafting, chests, the pause menu `PauseUI`, sign and trade windows...). Touches take the
//! same path: `UIManager` asks each part `touchIsInUI:` / `startTouch:...` and only touches nobody takes reach the
//! world.
//!
//! * **Hide game buttons** hooks the render and touch methods of `WorldUI` and `DPad` (see `play.rs`): while hidden,
//!   rendering returns at once and the touch methods answer "not mine", so taps go to the world. The pop-ups, the
//!   pause menu and the game's alerts are other classes and stay. Showing again just lets the methods run.
//! * **Cursor grid**: the world's tap matrices (`World.tapModelviewMatrix`, `tapProjectionMatrix`, `tapViewport`)
//!   give the tile grid on screen ([`bhpc_input::Grid`]).
//! * **Zoom**: `GameView.pinchScale` (a double, 1 = normal, smaller is closer) is what the game's pinch gesture
//!   changes; it is written directly, within the game's own limits.
//! * **Pause**: `-[World pauseButtonTapped]` / `-[World pauseResumeButtonTapped]`, the pause button's own actions,
//!   chosen by whether the `PauseUI` shows (`GameUIView.displayed`).
//!
//! Objects are found from the `GameView` that `-[GameView render:]` was last called on; every read checks for null
//! pointers and unreadable memory, so a missing object only means "not in a world".

use bhpc_input::grid::{Grid, Mat4, Pt};
use bhpc_input::pad::WALK_FINGER;
use bhpc_input::GuestEvent;
use bhpc_mem::GuestMemory;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// The instance variables used, by their `OBJC_IVAR_$_` export names.
pub const IVAR_NAMES: [&str; 12] = [
    "GameView.world",
    "GameView.pinchScale",
    "World.uiManager",
    "World.tapModelviewMatrix",
    "World.tapProjectionMatrix",
    "World.tapViewport",
    "UIManager.pauseUI",
    "UIManager.dpad",
    "GameUIView.displayed",
    "DPad.displayed",
    "DPad.windowInfo",
    "DPad.rightSide",
];

/// Offsets of the instance variables in [`IVAR_NAMES`] (same order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ivars {
    pub gv_world: u32,
    pub gv_pinch: u32,
    pub world_ui: u32,
    pub tap_mv: u32,
    pub tap_pj: u32,
    pub tap_vp: u32,
    pub ui_pause: u32,
    pub ui_dpad: u32,
    pub view_displayed: u32,
    pub dpad_displayed: u32,
    pub dpad_window: u32,
    pub dpad_right: u32,
}

impl Ivars {
    /// Read every offset: `export` gives the address of an `OBJC_IVAR_$_<name>` symbol, which holds the offset.
    pub fn read(mem: &GuestMemory, export: &dyn Fn(&str) -> Option<u32>) -> Result<Ivars, String> {
        let mut v = [0u32; 12];
        for (slot, name) in v.iter_mut().zip(IVAR_NAMES) {
            let at = export(&format!("OBJC_IVAR_$_{name}")).ok_or(format!("no ivar {name}"))?;
            *slot = mem.read_u32(at).map_err(|e| format!("ivar {name}: {e}"))?;
        }
        let [gv_world, gv_pinch, world_ui, tap_mv, tap_pj, tap_vp, ui_pause, ui_dpad, view_displayed, dpad_displayed, dpad_window, dpad_right] =
            v;
        Ok(Ivars {
            gv_world,
            gv_pinch,
            world_ui,
            tap_mv,
            tap_pj,
            tap_vp,
            ui_pause,
            ui_dpad,
            view_displayed,
            dpad_displayed,
            dpad_window,
            dpad_right,
        })
    }
}

/// Closest zoom (`GameView.pinchScale`) the game allows (its pinch gesture stops there in a normal world).
pub const ZOOM_MIN: f64 = 0.5;
/// Farthest zoom the controller goes to.
pub const ZOOM_MAX: f64 = 4.0;
/// One shoulder-button step.
pub const ZOOM_STEP: f64 = 1.25;
/// The levels the cycle button goes through: normal, far, farther, close.
pub const ZOOM_LEVELS: [f64; 4] = [1.0, 2.0, 4.0, 0.5];

/// The zoom one step closer (`closer`) or farther from `z`, inside the limits.
pub fn zoom_step(z: f64, closer: bool) -> f64 {
    let z = if z.is_finite() && z > 0.0 { z } else { 1.0 };
    (if closer { z / ZOOM_STEP } else { z * ZOOM_STEP }).clamp(ZOOM_MIN, ZOOM_MAX)
}

/// The next level of [`ZOOM_LEVELS`] after `z` (from a zoom between levels: the one after the nearest).
pub fn zoom_cycle(z: f64) -> f64 {
    let nearest =
        ZOOM_LEVELS.iter().enumerate().min_by(|a, b| (a.1 - z).abs().total_cmp(&(b.1 - z).abs())).map_or(0, |(i, _)| i);
    ZOOM_LEVELS[(nearest + 1) % ZOOM_LEVELS.len()]
}

/// The message shown when the game's buttons are hidden or shown.
pub fn message(hidden: bool) -> &'static str {
    if hidden {
        "GAME BUTTONS HIDDEN"
    } else {
        "GAME BUTTONS SHOWN"
    }
}

/// Shared between the game's hooks (main thread) and the UI thread.
#[derive(Debug, Default)]
pub struct GameUi {
    pub ivars: Ivars,
    hidden: AtomicBool,
    /// The controller's walking finger is on the D-pad: the hidden D-pad still takes that touch.
    walking: AtomicBool,
    game_view: AtomicU32,
}

fn f32_at(mem: &GuestMemory, at: u32) -> Option<f32> {
    mem.read_u32(at).ok().map(f32::from_bits)
}

fn f64_at(mem: &GuestMemory, at: u32) -> Option<f64> {
    mem.read_u64(at).ok().map(f64::from_bits)
}

fn ptr(mem: &GuestMemory, at: u32) -> Option<u32> {
    mem.read_u32(at).ok().filter(|p| *p != 0)
}

fn flag(mem: &GuestMemory, at: u32) -> bool {
    mem.read_u8(at).is_ok_and(|b| b != 0)
}

impl GameUi {
    pub fn new(ivars: Ivars) -> GameUi {
        GameUi { ivars, ..GameUi::default() }
    }

    /// The game's buttons are hidden.
    pub fn hidden(&self) -> bool {
        self.hidden.load(Ordering::SeqCst)
    }

    /// Hide them when shown and show them when hidden; returns whether they are hidden now.
    pub fn toggle(&self) -> bool {
        !self.hidden.fetch_xor(true, Ordering::SeqCst)
    }

    /// The controller's walking finger is down (or not).
    pub fn set_walking(&self, on: bool) {
        self.walking.store(on, Ordering::SeqCst);
    }

    /// Follow the controller's walking finger (`bhpc_input::pad::WALK_FINGER`) in the touches about to be delivered.
    pub fn track_walk(&self, ev: &GuestEvent) {
        match ev {
            GuestEvent::TouchBegin { id: WALK_FINGER, .. } => self.set_walking(true),
            GuestEvent::TouchEnd { id: WALK_FINGER, .. } => self.set_walking(false),
            GuestEvent::TouchCancel { ids, .. } if ids.contains(&WALK_FINGER) => self.set_walking(false),
            _ => {}
        }
    }

    /// Whether a hooked method of the game's buttons should do nothing now: always while they are hidden, except
    /// the D-pad's (`dpad`) while the controller walks with it.
    pub fn suppress(&self, dpad: bool) -> bool {
        self.hidden() && !(dpad && self.walking.load(Ordering::SeqCst))
    }

    /// `-[GameView render:]` ran on `view`.
    pub fn saw_game_view(&self, view: u32) {
        self.game_view.store(view, Ordering::SeqCst);
    }

    /// The `GameView` seen last.
    pub fn game_view(&self) -> Option<u32> {
        Some(self.game_view.load(Ordering::SeqCst)).filter(|v| *v != 0)
    }

    /// The world being played, if any.
    pub fn world(&self, mem: &GuestMemory) -> Option<u32> {
        ptr(mem, self.game_view()?.wrapping_add(self.ivars.gv_world))
    }

    fn ui_manager(&self, mem: &GuestMemory) -> Option<u32> {
        ptr(mem, self.world(mem)?.wrapping_add(self.ivars.world_ui))
    }

    /// The tile grid on screen, in content pixels of a `content`-sized picture.
    pub fn grid(&self, mem: &GuestMemory, content: (f32, f32)) -> Option<Grid> {
        let w = self.world(mem)?;
        let mat = |at: u32| -> Option<Mat4> {
            let mut m = [0f32; 16];
            for (i, v) in m.iter_mut().enumerate() {
                *v = f32_at(mem, w.wrapping_add(at + 4 * i as u32))?;
            }
            Some(m)
        };
        let (mv, pj) = (mat(self.ivars.tap_mv)?, mat(self.ivars.tap_pj)?);
        let mut vp = [0i32; 4];
        for (i, v) in vp.iter_mut().enumerate() {
            *v = mem.read_u32(w.wrapping_add(self.ivars.tap_vp + 4 * i as u32)).ok()? as i32;
        }
        Grid::from_gl(&mv, &pj, vp, content)
    }

    /// The pause menu shows.
    pub fn pause_shown(&self, mem: &GuestMemory) -> bool {
        self.ui_manager(mem)
            .and_then(|ui| ptr(mem, ui.wrapping_add(self.ivars.ui_pause)))
            .is_some_and(|p| flag(mem, p.wrapping_add(self.ivars.view_displayed)))
    }

    /// The game's D-pad, when it shows one: the size of the picture it lays itself out in (game points,
    /// `DPad.windowInfo`) and whether it sits on the right (`DPad.rightSide`).
    pub fn dpad(&self, mem: &GuestMemory) -> Option<((f32, f32), bool)> {
        let d = ptr(mem, self.ui_manager(mem)?.wrapping_add(self.ivars.ui_dpad))?;
        if !flag(mem, d.wrapping_add(self.ivars.dpad_displayed)) {
            return None;
        }
        let wi = ptr(mem, d.wrapping_add(self.ivars.dpad_window))?;
        let size = (f32_at(mem, wi)?, f32_at(mem, wi.wrapping_add(4))?);
        Some((size, flag(mem, d.wrapping_add(self.ivars.dpad_right))))
    }

    /// The current zoom (`GameView.pinchScale`).
    pub fn zoom(&self, mem: &GuestMemory) -> Option<f64> {
        self.world(mem)?;
        f64_at(mem, self.game_view()?.wrapping_add(self.ivars.gv_pinch)).filter(|z| z.is_finite())
    }

    /// Set the zoom; false outside a world.
    pub fn set_zoom(&self, mem: &GuestMemory, z: f64) -> bool {
        self.world(mem).is_some()
            && self.game_view().is_some_and(|v| mem.write_u64(v.wrapping_add(self.ivars.gv_pinch), z.to_bits()).is_ok())
    }
}

/// Distance of the D-pad's centre from the two edges of its corner (game points; as the game draws it).
pub const DPAD_INSET: f32 = 120.0;

/// Where the game's D-pad sits, in content pixels: `size` is the picture the D-pad lays itself out in
/// ([`GameUi::dpad`]), `content` the touch space. The D-pad is drawn in the bottom-left corner (bottom-right when the
/// game's "D-pad on the right" option is on), its centre `inset` points from both edges.
pub fn dpad_centre(size: (f32, f32), content: (f32, f32), right_side: bool, inset: f32) -> Option<Pt> {
    if size.0 <= 0.0 || size.1 <= 0.0 {
        return None;
    }
    let (sx, sy) = (content.0 / size.0, content.1 / size.1);
    let x = if right_side { size.0 - inset } else { inset };
    Some((x * sx, (size.1 - inset) * sy))
}

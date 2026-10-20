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
//!   changes. The wheel and the controller step through the game's own levels ([`SNAP_LEVELS`]) with short pinches
//!   of the game's handler ([`WheelAnim`], [`pinch_args`]); the value is written directly only as a fallback.
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
pub const IVAR_NAMES: [&str; 13] = [
    "GameView.world",
    "GameView.pinchScale",
    "GameView.windowInfo",
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
    /// `GameView.windowInfo`, held in the view: width and height of the view in points first.
    pub gv_window: u32,
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
        let mut v = [0u32; 13];
        for (slot, name) in v.iter_mut().zip(IVAR_NAMES) {
            let at = export(&format!("OBJC_IVAR_$_{name}")).ok_or(format!("no ivar {name}"))?;
            *slot = mem.read_u32(at).map_err(|e| format!("ivar {name}: {e}"))?;
        }
        let [gv_world, gv_pinch, gv_window, world_ui, tap_mv, tap_pj, tap_vp, ui_pause, ui_dpad, view_displayed, dpad_displayed, dpad_window, dpad_right] =
            v;
        Ok(Ivars {
            gv_world,
            gv_pinch,
            gv_window,
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

/// The zoom levels the game itself settles on (`-[GameView update:accurateDT:]`): when no pinch is in progress it
/// eases `pinchScale` to the nearest of these, with the switch points in [`SNAP_BETWEEN`]. A pinch that ends short of
/// a switch point therefore springs back; this is why one small wheel pinch per notch did nothing.
pub const SNAP_LEVELS: [f64; 6] = [0.5, 1.0, 1.5, 2.0, 3.0, 8.0];
/// Where the game switches from one of [`SNAP_LEVELS`] to the next.
pub const SNAP_BETWEEN: [f64; 5] = [0.6, 1.25, 1.75, 2.5, 5.0];
/// How long one wheel step takes on screen.
pub const WHEEL_ZOOM_MS: i64 = 140;

/// Index in [`SNAP_LEVELS`] of the level the game settles `z` on (a broken zoom counts as normal).
pub fn snap_level(z: f64) -> usize {
    if z.is_nan() {
        return 1;
    }
    SNAP_BETWEEN.iter().filter(|b| z >= **b).count()
}

/// The level `steps` wheel notches (positive = closer) from the level `z` settles on, within the game's levels.
pub fn wheel_target(z: f64, steps: i32) -> f64 {
    let i = (snap_level(z) as i64 - i64::from(steps)).clamp(0, SNAP_LEVELS.len() as i64 - 1);
    SNAP_LEVELS[i as usize]
}

/// One wheel step played as a short pinch of the game's own (see [`pinch_args`]): from zoom `from` to `to` around
/// `centre` (view points), started at `start_ms`. `base` is the zoom the pinch began at (the game's `pinchStartScale`);
/// a notch during a step keeps the pinch going and only moves `from`, `to` and `start_ms`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelAnim {
    pub base: f64,
    pub from: f64,
    pub to: f64,
    pub start_ms: i64,
    pub centre: (f32, f32),
}

impl WheelAnim {
    /// The zoom to show at `now_ms` (eased out, even in ratio) and whether the step is complete.
    pub fn at(&self, now_ms: i64) -> (f64, bool) {
        let t = ((now_ms - self.start_ms) as f64 / WHEEL_ZOOM_MS as f64).clamp(0.0, 1.0);
        let e = 1.0 - (1.0 - t) * (1.0 - t);
        (self.from * (self.to / self.from).powf(e), t >= 1.0)
    }

    /// The pinch factor that makes the game show `zoom`: it sets `pinchScale = pinchStartScale / factor`.
    pub fn factor(&self, zoom: f64) -> f32 {
        (self.base / zoom) as f32
    }

    /// The game ignored the pinch (a button under the cursor, a world still loading): it was asked for a zoom
    /// clearly away from the start but still shows the start.
    pub fn refused(&self, want: f64, shown: f64) -> bool {
        (want / self.base - 1.0).abs() > 1e-3 && (shown / self.base - 1.0).abs() < 1e-6
    }
}

/// A point in content pixels (`content`-sized touch space) in the view's points (`view`, `GameView.windowInfo`),
/// the space the game's pinch centre is in.
pub fn view_point(p: (i32, i32), content: (f32, f32), view: (f32, f32)) -> Option<(f32, f32)> {
    let ok = |v: f32| v.is_finite() && v > 0.0;
    if !(ok(content.0) && ok(content.1) && ok(view.0) && ok(view.1)) {
        return None;
    }
    Some((p.0 as f32 * view.0 / content.0, p.1 as f32 * view.1 / content.1))
}

/// Selector of the game's pinch handler on `GameView` (fed by the `UIPinchGestureRecognizer` of its `EAGLView`).
pub const PINCH_SELECTOR: &str = "pinchGesture:velocity:state:center:numberOfTouches:";

/// Pinch states of UIKit's gesture recognisers as the game's handler tests them.
pub const PINCH_BEGAN: u32 = 1;
pub const PINCH_CHANGED: u32 = 2;
pub const PINCH_ENDED: u32 = 3;

/// The arguments of `-[GameView pinchGesture:velocity:state:center:numberOfTouches:]` on `view` (`sel` its selector):
/// began (factor 1: takes the start zoom and centre), changed (the game sets the zoom and moves the camera so the
/// centre stays put; no spring-back while a pinch is in progress) or ended. The velocity is NaN, so the game stores
/// no pinch momentum.
pub fn pinch_args(view: u32, sel: u32, state: u32, factor: f32, centre: (f32, f32)) -> [u32; 8] {
    [view, sel, factor.to_bits(), f32::NAN.to_bits(), state, centre.0.to_bits(), centre.1.to_bits(), 2]
}

/// The levels the controller's cycle button goes through: normal, far, farther, close. All are levels of the game
/// itself ([`SNAP_LEVELS`]), so none springs back; 1.5 is left to the shoulder buttons and 8 (a tiny picture) out.
pub const ZOOM_LEVELS: [f64; 4] = [1.0, 2.0, 3.0, 0.5];

/// The next level of [`ZOOM_LEVELS`] after the one `z` settles on (from a level outside the cycle: the one after the
/// nearest).
pub fn zoom_cycle(z: f64) -> f64 {
    let z = SNAP_LEVELS[snap_level(z)];
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

    /// Size of the game view in points (`GameView.windowInfo`), in a world.
    pub fn view_size(&self, mem: &GuestMemory) -> Option<(f32, f32)> {
        self.world(mem)?;
        let at = self.game_view()?.wrapping_add(self.ivars.gv_window);
        Some((f32_at(mem, at)?, f32_at(mem, at.wrapping_add(4))?)).filter(|(w, h)| *w > 0.0 && *h > 0.0)
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

//! Game controller mapping: a cursor over the game picture, a touch at the cursor, the left stick on the game's
//! on-screen D-pad, and host actions for zoom, pause and the game's buttons.
//!
//! | Controller | Effect |
//! |---|---|
//! | left stick | walks: a finger (pointer 2) held on the game's D-pad, moved in the stick's direction |
//! | right stick | moves the cursor (speed grows with the tilt); it snaps to the tile under it when released. With [`PadConfig::tile_cursor`] it steps tile by tile in a world instead (see below) |
//! | D-pad | moves the cursor one tile (or [`PadConfig::step`]) per press, repeating while held |
//! | A | touch at the cursor (pointer 0): a tap, or hold to dig / drag |
//! | B | Android back key |
//! | X | [`HostAction::ToggleGameButtons`] |
//! | Y | [`HostAction::ZoomCycle`] |
//! | L1 / R1 | [`HostAction::ZoomOut`] / [`HostAction::ZoomIn`] |
//! | Start | [`HostAction::Pause`] |
//! | R3 | cursor back to the middle of the picture |
//!
//! The first controller connected drives the game; others are ignored until it is unplugged.
//!
//! **Tile by tile** ([`PadConfig::tile_cursor`], `bhpc-run --pad-tile-cursor`): while the game's tile grid is known
//! (in a world), the cursor always sits on the centre of one world tile. Tilting the right stick steps it one tile in
//! the stick's direction (eight directions), at once, then again after [`PadConfig::tile_delay_ms`] and every
//! [`PadConfig::tile_repeat_ms`] while held; both shrink from their first value (barely tilted) to their second (full
//! tilt). The cursor stays on its tile when the camera moves or zooms (it is placed again with every new grid),
//! except while A holds a finger down, so a drag never chases the view it moves. Without a grid (menus) the stick
//! moves the cursor freely as usual.

use crate::event::{EventSink, GuestEvent, HostAction, KEYCODE_BACK};
use crate::grid::{Grid, Pt, Tile};

/// Controller buttons (SDL `GameController` layout: A is the bottom face button).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadButton {
    A,
    B,
    X,
    Y,
    Back,
    Guide,
    Start,
    LeftStick,
    RightStick,
    LeftShoulder,
    RightShoulder,
    Up,
    Down,
    Left,
    Right,
    Other,
}

impl PadButton {
    /// The button an SDL `SDL_GameControllerButton` value stands for.
    pub fn from_sdl(n: u8) -> PadButton {
        use PadButton::*;
        const ALL: [PadButton; 15] =
            [A, B, X, Y, Back, Guide, Start, LeftStick, RightStick, LeftShoulder, RightShoulder, Up, Down, Left, Right];
        ALL.get(usize::from(n)).copied().unwrap_or(Other)
    }
}

/// Controller axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadAxis {
    LeftX,
    LeftY,
    RightX,
    RightY,
    LeftTrigger,
    RightTrigger,
    Other,
}

impl PadAxis {
    /// The axis an SDL `SDL_GameControllerAxis` value stands for.
    pub fn from_sdl(n: u8) -> PadAxis {
        use PadAxis::*;
        [LeftX, LeftY, RightX, RightY, LeftTrigger, RightTrigger].get(usize::from(n)).copied().unwrap_or(Other)
    }
}

/// SDL's axis value (`-32768..=32767`) as `-1.0..=1.0`.
pub fn axis_value(raw: i16) -> f32 {
    (f32::from(raw) / 32767.0).clamp(-1.0, 1.0)
}

/// What a controller did. `pad` is the controller's instance id.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PadEvent {
    Added(u32),
    Removed(u32),
    Button {
        pad: u32,
        button: PadButton,
        pressed: bool,
    },
    /// Sticks: -1 (left/up) to 1 (right/down); triggers 0 to 1.
    Axis {
        pad: u32,
        axis: PadAxis,
        value: f32,
    },
}

/// Tunables. Distances are content pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PadConfig {
    /// Controllers drive the game at all.
    pub enabled: bool,
    /// Stick tilt (0..1) below which a stick counts as centred.
    pub deadzone: f32,
    /// Cursor speed at full tilt, per second.
    pub cursor_speed: f32,
    /// D-pad cursor step when the tile grid is unknown.
    pub step: f32,
    /// A held D-pad direction repeats after this long, then every `repeat_ms`.
    pub repeat_delay_ms: i64,
    pub repeat_ms: i64,
    /// How far the walking finger moves from the D-pad's centre at full tilt.
    pub walk_reach: f32,
    /// The cursor moves tile by tile in a world (see the module documentation).
    pub tile_cursor: bool,
    /// Tile by tile: the right stick's first repeat comes this long after the first step, and then a step every
    /// `tile_repeat_ms`; `[barely tilted, full tilt]`, linear in between.
    pub tile_delay_ms: [i64; 2],
    pub tile_repeat_ms: [i64; 2],
}

impl Default for PadConfig {
    fn default() -> PadConfig {
        PadConfig {
            enabled: true,
            deadzone: 0.25,
            cursor_speed: 700.0,
            step: 32.0,
            repeat_delay_ms: 320,
            repeat_ms: 90,
            walk_reach: 60.0,
            tile_cursor: false,
            tile_delay_ms: [350, 150],
            tile_repeat_ms: [250, 50],
        }
    }
}

/// Pointer id of the touch at the cursor (the same as the mouse's: the Java dialogs only take pointer 0).
pub const CURSOR_FINGER: i32 = 0;
/// Pointer id of the walking finger.
pub const WALK_FINGER: i32 = 2;

/// The controller part of the mapper.
#[derive(Debug, Clone)]
pub struct PadState {
    pub cfg: PadConfig,
    /// Connected controllers, oldest first; the first one is the active one.
    pads: Vec<u32>,
    /// Content size the cursor stays inside.
    bounds: (f32, f32),
    cursor: Pt,
    /// The cursor is drawn: a controller is connected and was the last thing used.
    shown: bool,
    /// The window has the keyboard focus: controller input counts only then.
    focused: bool,
    left: Pt,
    right: Pt,
    /// Held D-pad direction and when it next repeats.
    dpad: Option<((i32, i32), i64)>,
    held: [bool; 4],
    /// A is down: the cursor finger touches the game.
    touching: bool,
    /// The walking finger: where it went down and where it is.
    walk: Option<(Pt, Pt)>,
    /// Where the game's D-pad is (its centre), when it shows one.
    dpad_at: Option<Pt>,
    grid: Option<Grid>,
    /// The right stick moved the cursor: snap it to a tile once the stick is released.
    snap_due: bool,
    /// Tile by tile: the world tile the cursor is on (`None`: work it out from the cursor).
    tile: Option<Tile>,
    /// Tile by tile: the right stick's direction and when it next steps.
    rstep: Option<((i32, i32), i64)>,
    last_tick: Option<i64>,
}

fn len((x, y): Pt) -> f32 {
    (x * x + y * y).sqrt()
}

impl PadState {
    pub fn new(cfg: PadConfig, bounds: (f32, f32)) -> PadState {
        PadState {
            cfg,
            pads: Vec::new(),
            bounds,
            cursor: (bounds.0 / 2.0, bounds.1 / 2.0),
            shown: false,
            focused: true,
            left: (0.0, 0.0),
            right: (0.0, 0.0),
            dpad: None,
            held: [false; 4],
            touching: false,
            walk: None,
            dpad_at: None,
            grid: None,
            snap_due: false,
            tile: None,
            rstep: None,
            last_tick: None,
        }
    }

    /// The cursor position (content pixels) while it is shown.
    pub fn cursor(&self) -> Option<Pt> {
        self.shown.then_some(self.cursor)
    }

    /// Size of the tile under the cursor on screen (content pixels), when the grid is known.
    pub fn tile_size(&self) -> Option<f32> {
        self.grid.as_ref().map(|g| g.tile_size(self.cursor)).filter(|s| *s > 0.0)
    }

    /// True while the left stick holds the walking finger down.
    pub fn walking(&self) -> bool {
        self.walk.is_some()
    }

    /// True while A holds the cursor finger down.
    pub fn touching(&self) -> bool {
        self.touching
    }

    /// The active controller.
    pub fn active(&self) -> Option<u32> {
        self.pads.first().copied()
    }

    /// The content size changed.
    pub fn set_bounds(&mut self, bounds: (f32, f32)) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.cursor = self.clamp(self.cursor);
        }
    }

    /// Where the game's D-pad is now (`None`: it shows none, the left stick does nothing).
    pub fn set_dpad(&mut self, at: Option<Pt>) {
        self.dpad_at = at;
    }

    /// The game's tile grid on screen (`None`: unknown, no snapping). Tile by tile, the cursor goes back onto its
    /// tile wherever the new grid puts it (not while A holds a finger down: then it keeps its place on screen).
    pub fn set_grid(&mut self, grid: Option<Grid>) {
        self.grid = grid;
        match self.tile_grid() {
            None => self.tile = None,
            Some(_) if self.touching => self.tile = None,
            Some(g) => self.settle(&g),
        }
    }

    /// The grid when the cursor moves tile by tile.
    fn tile_grid(&self) -> Option<Grid> {
        self.grid.filter(|_| self.cfg.tile_cursor)
    }

    fn inside(&self, (x, y): Pt) -> bool {
        (0.0..=self.bounds.0).contains(&x) && (0.0..=self.bounds.1).contains(&y)
    }

    /// Put the cursor on the centre of its tile (or of the tile under it). A tile that left the picture gives way
    /// to the one under the nearest point of the picture.
    fn settle(&mut self, g: &Grid) {
        let Some(t) = self.tile.or_else(|| g.tile(self.cursor)) else { return };
        let (t, at) = match g.centre(t) {
            Some(p) if self.inside(p) => (t, p),
            p => {
                let Some(t) = g.tile(self.clamp(p.unwrap_or(self.cursor))) else { return };
                let Some(p) = g.centre(t) else { return };
                (t, self.clamp(p))
            }
        };
        self.cursor = at;
        self.tile = Some(t);
    }

    /// The window gained or lost the focus. Without it buttons and sticks are ignored (a launcher that shows the
    /// game inside its own window gets controller events even when another application is in front).
    pub fn set_focused(&mut self, on: bool) {
        self.focused = on;
    }

    /// The mouse moved: the mouse is in charge, the cursor hides.
    pub fn mouse_moved(&mut self) {
        self.shown = false;
    }

    fn clamp(&self, (x, y): Pt) -> Pt {
        (x.clamp(0.0, self.bounds.0.max(0.0)), y.clamp(0.0, self.bounds.1.max(0.0)))
    }

    /// Lift every finger the controller holds (focus lost, controller gone, disabled).
    pub fn release(&mut self, now_ms: i64, sink: &mut dyn EventSink) {
        let (mut ids, mut xs, mut ys) = (vec![], vec![], vec![]);
        if self.touching {
            ids.push(CURSOR_FINGER);
            xs.push(self.cursor.0);
            ys.push(self.cursor.1);
        }
        if let Some((_, at)) = self.walk {
            ids.push(WALK_FINGER);
            xs.push(at.0);
            ys.push(at.1);
        }
        if !ids.is_empty() {
            sink.deliver(&GuestEvent::TouchCancel { ids, xs, ys, time_ms: now_ms });
        }
        self.touching = false;
        self.walk = None;
        self.left = (0.0, 0.0);
        self.right = (0.0, 0.0);
        self.dpad = None;
        self.rstep = None;
        self.held = [false; 4];
    }

    /// One controller event. `mouse_busy`: a mouse gesture owns pointer 0 (A then does nothing).
    pub fn handle(&mut self, ev: PadEvent, now_ms: i64, mouse_busy: bool, sink: &mut dyn EventSink) {
        if !self.cfg.enabled {
            return;
        }
        match ev {
            PadEvent::Added(id) => {
                if !self.pads.contains(&id) {
                    self.pads.push(id);
                }
            }
            PadEvent::Removed(id) => {
                if self.active() == Some(id) {
                    self.release(now_ms, sink);
                    self.shown = false;
                }
                self.pads.retain(|p| *p != id);
            }
            PadEvent::Button { .. } | PadEvent::Axis { .. } if !self.focused => {}
            PadEvent::Button { pad, button, pressed } if Some(pad) == self.active() => {
                self.shown = true;
                self.button(button, pressed, now_ms, mouse_busy, sink);
            }
            PadEvent::Axis { pad, axis, value } if Some(pad) == self.active() => {
                let v = value.clamp(-1.0, 1.0);
                match axis {
                    PadAxis::LeftX => self.left.0 = v,
                    PadAxis::LeftY => self.left.1 = v,
                    PadAxis::RightX => self.right.0 = v,
                    PadAxis::RightY => self.right.1 = v,
                    _ => return,
                }
                if matches!(axis, PadAxis::RightX | PadAxis::RightY) && len(self.right) >= self.cfg.deadzone {
                    self.shown = true;
                }
                self.walk_update(now_ms, sink);
            }
            PadEvent::Button { .. } | PadEvent::Axis { .. } => {}
        }
    }

    fn button(&mut self, b: PadButton, pressed: bool, now_ms: i64, mouse_busy: bool, sink: &mut dyn EventSink) {
        let action = |a| if pressed { Some(a) } else { None };
        let act = match b {
            PadButton::A => {
                if pressed && !self.touching && !mouse_busy {
                    self.touching = true;
                    let (x, y) = self.cursor;
                    sink.deliver(&GuestEvent::TouchBegin { id: CURSOR_FINGER, x, y, time_ms: now_ms });
                } else if !pressed && self.touching {
                    self.touching = false;
                    let (x, y) = self.cursor;
                    sink.deliver(&GuestEvent::TouchEnd { id: CURSOR_FINGER, x, y, time_ms: now_ms });
                }
                None
            }
            PadButton::B => {
                let ev = if pressed {
                    GuestEvent::KeyDown { key_code: KEYCODE_BACK, unicode: 0, meta: 0, time_ms: now_ms }
                } else {
                    GuestEvent::KeyUp { key_code: KEYCODE_BACK, unicode: 0, meta: 0, time_ms: now_ms }
                };
                sink.deliver(&ev);
                None
            }
            PadButton::X => action(HostAction::ToggleGameButtons),
            PadButton::Y => action(HostAction::ZoomCycle),
            PadButton::LeftShoulder => action(HostAction::ZoomOut),
            PadButton::RightShoulder => action(HostAction::ZoomIn),
            PadButton::Start => action(HostAction::Pause),
            PadButton::RightStick => {
                if pressed {
                    let c = (self.bounds.0 / 2.0, self.bounds.1 / 2.0);
                    let tile = self.tile_grid().and_then(|g| Some((g.tile(c)?, g)));
                    match tile.and_then(|(t, g)| Some((t, g.centre(t)?))) {
                        Some((t, p)) => self.move_to_tile(t, p, now_ms, sink),
                        None => self.move_cursor(c, now_ms, sink),
                    }
                }
                None
            }
            PadButton::Up | PadButton::Down | PadButton::Left | PadButton::Right => {
                let i = [PadButton::Up, PadButton::Down, PadButton::Left, PadButton::Right]
                    .iter()
                    .position(|d| *d == b)
                    .unwrap_or(0);
                self.held[i] = pressed;
                let dir = self.dpad_dir();
                if dir == (0, 0) {
                    self.dpad = None;
                } else if pressed {
                    self.step_cursor(dir, now_ms, sink);
                    self.dpad = Some((dir, now_ms + self.cfg.repeat_delay_ms));
                } else if let Some((_, next)) = self.dpad {
                    self.dpad = Some((dir, next));
                }
                None
            }
            PadButton::Back | PadButton::Guide | PadButton::LeftStick | PadButton::Other => None,
        };
        if let Some(a) = act {
            sink.host_action(a);
        }
    }

    fn dpad_dir(&self) -> (i32, i32) {
        let [u, d, l, r] = self.held.map(i32::from);
        (r - l, d - u)
    }

    fn step_cursor(&mut self, (dx, dy): (i32, i32), now_ms: i64, sink: &mut dyn EventSink) {
        if let Some(g) = self.tile_grid() {
            // Tile by tile: onto the centre of the next tile, or nowhere when that is off the picture.
            let next = self.tile.or_else(|| g.tile(self.cursor)).map(|t| g.neighbour(t, dx, dy));
            if let Some((t, p)) = next.and_then(|t| Some((t, g.centre(t)?))).filter(|(_, p)| self.inside(*p)) {
                self.move_to_tile(t, p, now_ms, sink);
            }
            return;
        }
        let to = match &self.grid {
            Some(g) => g.step(self.cursor, dx, dy),
            None => (self.cursor.0 + dx as f32 * self.cfg.step, self.cursor.1 + dy as f32 * self.cfg.step),
        };
        self.move_cursor(to, now_ms, sink);
    }

    /// Tile by tile: the cursor onto tile `t`, whose centre is at `p`.
    fn move_to_tile(&mut self, t: Tile, p: Pt, now_ms: i64, sink: &mut dyn EventSink) {
        self.move_cursor(p, now_ms, sink);
        self.tile = (!self.touching).then_some(t);
    }

    fn move_cursor(&mut self, to: Pt, now_ms: i64, sink: &mut dyn EventSink) {
        let to = self.clamp(to);
        if to == self.cursor {
            return;
        }
        self.cursor = to;
        if self.touching {
            self.send_moves(now_ms, sink);
        }
    }

    /// A move of every finger the controller holds (Android reports all pointers in a move).
    fn send_moves(&self, now_ms: i64, sink: &mut dyn EventSink) {
        let (mut ids, mut xs, mut ys) = (vec![], vec![], vec![]);
        if self.touching {
            ids.push(CURSOR_FINGER);
            xs.push(self.cursor.0);
            ys.push(self.cursor.1);
        }
        if let Some((_, at)) = self.walk {
            ids.push(WALK_FINGER);
            xs.push(at.0);
            ys.push(at.1);
        }
        sink.deliver(&GuestEvent::TouchMove { ids, xs, ys, time_ms: now_ms });
    }

    /// Put the walking finger where the left stick says.
    fn walk_update(&mut self, now_ms: i64, sink: &mut dyn EventSink) {
        let tilt = len(self.left);
        let target = self.dpad_at.filter(|_| tilt >= self.cfg.deadzone);
        match (self.walk, target) {
            (None, Some(c)) => {
                self.walk = Some((c, c));
                sink.deliver(&GuestEvent::TouchBegin { id: WALK_FINGER, x: c.0, y: c.1, time_ms: now_ms });
                self.walk_update(now_ms, sink);
            }
            (Some((start, at)), Some(_)) => {
                let r = self.cfg.walk_reach * ((tilt - self.cfg.deadzone) / (1.0 - self.cfg.deadzone)).clamp(0.2, 1.0);
                let to = (start.0 + self.left.0 / tilt * r, start.1 + self.left.1 / tilt * r);
                if to != at {
                    self.walk = Some((start, to));
                    self.send_moves(now_ms, sink);
                }
            }
            (Some((_, at)), None) => {
                self.walk = None;
                sink.deliver(&GuestEvent::TouchEnd { id: WALK_FINGER, x: at.0, y: at.1, time_ms: now_ms });
            }
            (None, None) => {}
        }
    }

    /// Advance time: the right stick moves the cursor, a held D-pad repeats. Call about once per frame.
    pub fn tick(&mut self, now_ms: i64, sink: &mut dyn EventSink) {
        let dt = self.last_tick.map_or(0, |t| (now_ms - t).clamp(0, 50)) as f32 / 1000.0;
        self.last_tick = Some(now_ms);
        if !self.cfg.enabled || self.pads.is_empty() {
            return;
        }
        let tilt = len(self.right);
        if self.tile_grid().is_some() {
            self.snap_due = false;
            self.tile_tick(tilt, now_ms, sink);
            return self.dpad_tick(now_ms, sink);
        }
        self.rstep = None;
        if tilt >= self.cfg.deadzone {
            // Quadratic response: fine control near the centre, speed at full tilt.
            let k = ((tilt - self.cfg.deadzone) / (1.0 - self.cfg.deadzone)).min(1.0);
            let v = self.cfg.cursor_speed * k * k * dt / tilt;
            let to = (self.cursor.0 + self.right.0 * v, self.cursor.1 + self.right.1 * v);
            self.move_cursor(to, now_ms, sink);
            self.snap_due = true;
        } else if self.snap_due {
            self.snap_due = false;
            if let Some(g) = &self.grid {
                let to = g.snap(self.cursor);
                self.move_cursor(to, now_ms, sink);
            }
        }
        self.dpad_tick(now_ms, sink);
    }

    /// A held D-pad direction repeats.
    fn dpad_tick(&mut self, now_ms: i64, sink: &mut dyn EventSink) {
        if let Some((dir, next)) = self.dpad {
            if now_ms >= next {
                self.step_cursor(dir, now_ms, sink);
                self.dpad = Some((dir, now_ms + self.cfg.repeat_ms));
            }
        }
    }
}

/// The stick's direction in one of eight: a step on an axis counts when the tilt is within 67.5 degrees of it.
pub fn octant((x, y): Pt) -> (i32, i32) {
    const TAN_22_5: f32 = 0.414_213_57;
    let part = |a: f32, b: f32| if a.abs() > TAN_22_5 * b.abs() { a.signum() as i32 } else { 0 };
    (part(x, y), part(y, x))
}

/// `ms[0]` at `k = 0`, `ms[1]` at `k = 1`, linear in between.
fn lerp_ms(ms: [i64; 2], k: f32) -> i64 {
    ms[0] + ((ms[1] - ms[0]) as f32 * k.clamp(0.0, 1.0)).round() as i64
}

impl PadState {
    /// Tile by tile: the right stick steps the cursor once on tilt, then repeats faster the more it is tilted.
    fn tile_tick(&mut self, tilt: f32, now_ms: i64, sink: &mut dyn EventSink) {
        if tilt < self.cfg.deadzone {
            self.rstep = None;
            return;
        }
        let k = (tilt - self.cfg.deadzone) / (1.0 - self.cfg.deadzone);
        let dir = octant(self.right);
        self.rstep = match self.rstep {
            None => {
                self.step_cursor(dir, now_ms, sink);
                Some((dir, now_ms + lerp_ms(self.cfg.tile_delay_ms, k)))
            }
            Some((_, next)) if now_ms >= next => {
                self.step_cursor(dir, now_ms, sink);
                Some((dir, now_ms + lerp_ms(self.cfg.tile_repeat_ms, k)))
            }
            Some((_, next)) => Some((dir, next)),
        };
    }
}

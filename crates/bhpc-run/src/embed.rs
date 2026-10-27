//! Play-mode options that come from the launcher, and the in-window (`--embed`) plumbing: the launcher's input
//! commands as window events, frames as BGRA pixels. Pure logic; `play.rs` (SDL, threads) wires it up.

use bhpc_app::{Event, Key, Metrics, Mods, MouseButton};
use bhpc_input::{Bindings, HostEvent};
use bhpc_launcher::embed::{Cmd, Layout, MOD_ALT, MOD_CTRL, MOD_SHIFT};
use bhpc_launcher::keys::KeyBindings;
use std::path::PathBuf;

/// What the launcher's config (`--config` expanded into flags) says about the window and the controls.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayFlags {
    /// Start in fullscreen (`--fullscreen`).
    pub fullscreen: bool,
    /// The game may use vsync (off with `--no-vsync`).
    pub vsync: bool,
    /// Window title (`--title`).
    pub title: Option<String>,
    /// Key bindings (`--bind ACTION=BINDING`, the first one given for an action wins).
    pub bindings: Bindings,
    /// Render for the launcher window instead of a window of our own: the frame file (`--embed FILE`).
    pub embed: Option<PathBuf>,
    /// Present at most this many frames per second (`--fps-limit N`; absent or 0: no cap).
    pub fps_limit: Option<u32>,
    /// Draw the FPS counter over the game picture (`--show-fps`).
    pub show_fps: bool,
    /// Corner of the FPS counter (`--fps-position tl|tr|bl|br`, default top-left).
    pub fps_position: bhpc_app::hud::Corner,
    /// Lift the game's 30 FPS cap (`--unlock-fps`, see `fpsunlock`).
    pub unlock_fps: bool,
    /// World speed multiplier (`--game-speed F`, default 1: untouched; see `gamespeed`).
    pub game_speed: f64,
    /// How many points the game gets for the window (`--zoom phone|close|tablet`, default phone; see `aspect`).
    pub zoom: crate::aspect::GameZoom,
    /// The game UI scale on top of the zoom (`--ui-scale F`, default 1; the zoom's points divided by it, see `aspect`).
    pub ui_scale: f32,
    /// Game controllers drive the game (off with `--no-controller`).
    pub controller: bool,
    /// The game's D-pad stays hidden (`--hide-dpad`).
    pub hide_dpad: bool,
    /// The controller cursor moves tile by tile in a world (`--pad-tile-cursor`).
    pub pad_tile_cursor: bool,
    /// Language of the messages shown over the game (`--lang CODE`, the launcher's language; default English).
    pub lang: bhpc_app::hud::Lang,
}

/// Reads the flags above from a command line.
pub fn parse_play_flags(args: &[String]) -> Result<PlayFlags, String> {
    let value = |name: &str| args.iter().position(|a| a == name).map(|i| args.get(i + 1).cloned());
    let mut keys = KeyBindings::default();
    // `expand_args` puts explicit flags before the config's, and the first occurrence wins: apply from the back.
    let binds: Vec<&String> =
        args.iter().enumerate().filter(|(_, a)| *a == "--bind").filter_map(|(i, _)| args.get(i + 1)).collect();
    for b in binds.into_iter().rev() {
        keys.apply_flag(b)?;
    }
    Ok(PlayFlags {
        fullscreen: args.iter().any(|a| a == "--fullscreen"),
        vsync: !args.iter().any(|a| a == "--no-vsync"),
        title: value("--title").map(|v| v.ok_or("--title needs a text")).transpose()?,
        bindings: keys.to_bindings(),
        embed: value("--embed").map(|v| v.map(PathBuf::from).ok_or("--embed needs a file")).transpose()?,
        fps_limit: value("--fps-limit")
            .map(|v| v.and_then(|n| n.parse::<u32>().ok()).ok_or("--fps-limit needs a number (0 = no limit)"))
            .transpose()?
            .filter(|n| *n > 0),
        show_fps: args.iter().any(|a| a == "--show-fps"),
        fps_position: value("--fps-position")
            .map(|v| v.as_deref().and_then(bhpc_app::hud::Corner::parse).ok_or("--fps-position needs tl, tr, bl or br"))
            .transpose()?
            .unwrap_or_default(),
        unlock_fps: args.iter().any(|a| a == "--unlock-fps"),
        game_speed: value("--game-speed")
            .map(|v| {
                v.as_deref().and_then(crate::gamespeed::parse_speed).ok_or_else(|| {
                    let (lo, hi) = crate::gamespeed::GAME_SPEED_RANGE;
                    format!("--game-speed needs a number from {lo} to {hi}")
                })
            })
            .transpose()?
            .unwrap_or(1.0),
        zoom: value("--zoom")
            .map(|v| v.as_deref().and_then(crate::aspect::GameZoom::parse).ok_or("--zoom needs phone, close or tablet"))
            .transpose()?
            .unwrap_or_default(),
        ui_scale: value("--ui-scale")
            .map(|v| {
                v.as_deref().and_then(crate::aspect::parse_fine).ok_or_else(|| {
                    let (lo, hi) = crate::aspect::FINE_RANGE;
                    format!("--ui-scale needs a number from {lo} to {hi}")
                })
            })
            .transpose()?
            .unwrap_or(1.0),
        controller: !args.iter().any(|a| a == "--no-controller"),
        hide_dpad: args.iter().any(|a| a == "--hide-dpad"),
        pad_tile_cursor: args.iter().any(|a| a == "--pad-tile-cursor"),
        lang: value("--lang")
            .map(|v| v.as_deref().map(bhpc_app::hud::Lang::parse).ok_or("--lang needs a language code"))
            .transpose()?
            .unwrap_or_default(),
    })
}

fn mods(bits: u8) -> Mods {
    Mods { ctrl: bits & MOD_CTRL != 0, shift: bits & MOD_SHIFT != 0, alt: bits & MOD_ALT != 0 }
}

/// A launcher command as the window event the SDL backend would have produced. `Size` and `Quit` are not events
/// (the caller resizes the window and ends the run).
pub fn cmd_event(cmd: &Cmd) -> Option<Event> {
    Some(match cmd {
        Cmd::Size { .. } | Cmd::Layout(_) | Cmd::Quit | Cmd::ModsReload | Cmd::ModsChat(_) => return None,
        Cmd::Move { x, y } => Event::MouseMove { x: *x, y: *y },
        Cmd::Button { button, pressed, x, y, .. } => {
            let button = match button {
                0 => MouseButton::Left,
                1 => MouseButton::Middle,
                2 => MouseButton::Right,
                n => MouseButton::Other(*n),
            };
            Event::MouseButton { button, pressed: *pressed, x: *x, y: *y }
        }
        Cmd::Wheel { dx, dy, .. } => Event::Wheel { dx: *dx, dy: *dy },
        Cmd::Key { hid, pressed, repeat, mods: m } => {
            Event::Key { key: Key(*hid), pressed: *pressed, repeat: *repeat, mods: mods(*m) }
        }
        Cmd::Text(t) => Event::Text(t.clone()),
        Cmd::Focus(on) => Event::Focus(*on),
        Cmd::Visible(on) => Event::Visible(*on),
    })
}

/// The metrics of a window of this many device pixels (no extra DPI scale: the launcher sends device pixels).
pub fn size_metrics(width: u32, height: u32) -> Metrics {
    Metrics::new((width, height), (width, height))
}

/// Remembers what the plain window events do not carry: which modifiers are held (from the modifier keys) and where
/// the pointer is, so button and wheel events reach the mapper with both.
#[derive(Debug, Default)]
pub struct InputTracker {
    mods: bhpc_input::Mods,
    pos: (f32, f32),
}

impl InputTracker {
    /// The mapper's view of `ev`.
    pub fn map(&mut self, ev: &Event) -> Option<HostEvent> {
        match ev {
            Event::MouseMove { x, y } | Event::MouseButton { x, y, .. } => self.pos = (*x, *y),
            Event::Key { key, pressed, .. } => match key.0 {
                224 | 228 => self.mods.ctrl = *pressed,
                225 | 229 => self.mods.shift = *pressed,
                226 | 230 => self.mods.alt = *pressed,
                _ => {}
            },
            _ => {}
        }
        Some(match crate::input::host_event(ev)? {
            HostEvent::MouseButton { button, pressed, x, y, .. } => {
                HostEvent::MouseButton { button, pressed, x, y, mods: self.mods }
            }
            HostEvent::Wheel { dy, .. } => HostEvent::Wheel { dy, x: self.pos.0, y: self.pos.1, mods: self.mods },
            other => other,
        })
    }

    /// The modifiers currently held.
    pub fn mods(&self) -> bhpc_input::Mods {
        self.mods
    }
}

/// Turns bottom-up RGBA pixels (what `glReadPixels` returns) into the frame file's top-down opaque BGRA. `out` is
/// reused between frames (resized, never cleared).
pub fn rgba_bottom_up_to_bgra(width: u32, height: u32, rgba: &[u8], out: &mut Vec<u8>) {
    let row = width as usize * 4;
    out.resize(row * height as usize, 0);
    for (dst, src) in out.chunks_exact_mut(row).zip(rgba.chunks_exact(row).rev()) {
        for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            d.copy_from_slice(&[s[2], s[1], s[0], 255]);
        }
    }
}

/// The pixels of a read-back frame (bottom-up RGBA) in the layout the launcher asked for: as they are for a GL view,
/// converted into `scratch` for the others.
pub fn frame_pixels<'a>(layout: Layout, width: u32, height: u32, rgba: &'a [u8], scratch: &'a mut Vec<u8>) -> &'a [u8] {
    match layout {
        Layout::GlRgba => rgba,
        Layout::Bgra => {
            rgba_bottom_up_to_bgra(width, height, rgba, scratch);
            scratch
        }
    }
}

/// The frame-rate cap of the in-window game: the display's refresh rate (60 Hz when unknown), or the player's own cap
/// when it is lower. The launcher shows at most one frame per refresh; the game drawing 200 frames a second in its
/// hidden window (vsync does not hold it back there) only took the GPU from the launcher's own page.
pub fn embedded_fps_limit(player: Option<u32>, refresh: Option<u32>) -> u32 {
    let display = refresh.filter(|r| (24..=500).contains(r)).unwrap_or(60);
    player.filter(|&l| l > 0).map_or(display, |l| l.min(display))
}

/// Holds back the frames the game draws right after the window changed size while it still lays out for the old
/// one: its surface keeps the old aspect for a few frames (letterboxed, black bars, the picture jumping). The launcher
/// shows its last frame stretched meanwhile. Never longer than [`TRANSITION_HOLD`] (some screens only lay out again
/// on the next touch).
#[derive(Debug, Clone, Default)]
pub struct TransitionGate {
    drawable: (u32, u32),
    since: Option<std::time::Instant>,
}

/// The longest a frame of the old layout is held back after a size change.
pub const TRANSITION_HOLD: std::time::Duration = std::time::Duration::from_millis(500);

impl TransitionGate {
    /// Whether the frame with the game's `surface` in a window of `drawable` pixels, finished at `now`, is held back.
    pub fn hold(&mut self, drawable: (u32, u32), surface: (u32, u32), now: std::time::Instant) -> bool {
        if drawable != self.drawable {
            // The first size is not a change.
            self.since = (self.drawable != (0, 0)).then_some(now);
            self.drawable = drawable;
        }
        let Some(since) = self.since else { return false };
        if now.duration_since(since) >= TRANSITION_HOLD {
            self.since = None;
            return false;
        }
        let aspect = |(w, h): (u32, u32)| f64::from(w.max(1)) / f64::from(h.max(1));
        let off = (aspect(surface) / aspect(drawable) - 1.0).abs() > 0.02;
        if !off {
            self.since = None;
        }
        off
    }
}

/// Which of the game's frames go to the launcher: about one per refresh of the display. A fixed minimum gap did not
/// do: a game drawing 80 frames a second (12.5 ms apart) with a 15 ms gap sent every other one, 40 a second, unevenly.
/// This keeps a due time that advances by one display period per frame sent, so 80 frames become about 60.
#[derive(Debug, Clone)]
pub struct FramePacer {
    period: std::time::Duration,
    due: Option<std::time::Instant>,
}

/// How early a frame may come and still count for its display period (frame times jitter).
const PACE_SLACK: std::time::Duration = std::time::Duration::from_millis(2);

impl FramePacer {
    /// For a display refreshing `hz` times a second (60 when unknown or implausible).
    pub fn new(hz: Option<u32>) -> FramePacer {
        let hz = hz.filter(|h| (24..=500).contains(h)).unwrap_or(60);
        FramePacer { period: std::time::Duration::from_secs(1) / hz, due: None }
    }

    /// Whether the frame finished at `now` goes to the launcher.
    pub fn take(&mut self, now: std::time::Instant) -> bool {
        match self.due {
            Some(due) if now + PACE_SLACK < due => false,
            Some(due) => {
                // A game that stalled does not get a burst of frames to catch up.
                self.due = Some(if due + self.period < now { now + self.period } else { due + self.period });
                true
            }
            None => {
                self.due = Some(now + self.period);
                true
            }
        }
    }
}

/// Whether frames go through the asynchronous read-back (`BHPC_EMBED_ASYNC`, on unless `0`).
pub fn async_readback(env: Option<&str>) -> bool {
    env != Some("0")
}

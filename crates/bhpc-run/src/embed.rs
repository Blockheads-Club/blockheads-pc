//! Play-mode options that come from the launcher, and the in-window (`--embed`) plumbing: the launcher's input
//! commands as window events, frames as BGRA pixels. Pure logic; `play.rs` (SDL, threads) wires it up.

use bhpc_app::{Event, Key, Metrics, Mods, MouseButton};
use bhpc_input::{Bindings, HostEvent};
use bhpc_launcher::embed::{Cmd, MOD_ALT, MOD_CTRL, MOD_SHIFT};
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
    })
}

fn mods(bits: u8) -> Mods {
    Mods { ctrl: bits & MOD_CTRL != 0, shift: bits & MOD_SHIFT != 0, alt: bits & MOD_ALT != 0 }
}

/// A launcher command as the window event the SDL backend would have produced. `Size` and `Quit` are not events
/// (the caller resizes the window and ends the run).
pub fn cmd_event(cmd: &Cmd) -> Option<Event> {
    Some(match cmd {
        Cmd::Size { .. } | Cmd::Quit => return None,
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

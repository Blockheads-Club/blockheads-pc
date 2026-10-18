//! Platform independent window and input events.

use crate::metrics::Metrics;

/// A key as its USB HID usage id (SDL2 scancodes use the same numbers), independent of the keyboard layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key(pub u32);

impl Key {
    pub const ENTER: Key = Key(40);
    pub const ESCAPE: Key = Key(41);
    pub const BACKSPACE: Key = Key(42);
    pub const F11: Key = Key(68);
}

/// Modifier keys held during a key event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    Other(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Down,
    Move,
    Up,
}

/// Window and input events. Coordinates are logical window pixels (not drawable pixels): divide by nothing, multiply
/// by [`Metrics::scale`] to get framebuffer pixels.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The user asked to close the window.
    Quit,
    /// Size or DPI scale changed.
    Resized(Metrics),
    Focus(bool),
    /// The window was minimised (`false`) or restored (`true`).
    Visible(bool),
    /// Fullscreen state changed (after a toggle).
    Fullscreen(bool),
    MouseMove {
        x: f32,
        y: f32,
    },
    MouseButton {
        button: MouseButton,
        pressed: bool,
        x: f32,
        y: f32,
    },
    Wheel {
        dx: f32,
        dy: f32,
    },
    Key {
        key: Key,
        pressed: bool,
        repeat: bool,
        mods: Mods,
    },
    /// Committed text from the keyboard or an IME.
    Text(String),
    Touch {
        id: i64,
        phase: TouchPhase,
        x: f32,
        y: f32,
    },
}

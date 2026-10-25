//! Key binding model. The actions are the ones `bhpc-input` implements; the defaults reproduce its
//! hard-wired mapping (left button = touch, right drag = pan, wheel = zoom, Ctrl+left drag = pinch,
//! Esc = back, F11 = fullscreen, H = hide the game's buttons).

use bhpc_input::HostKey;
use serde::{Deserialize, Serialize};
use std::fmt;

/// What a binding triggers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    /// One-finger touch / drag.
    Touch,
    /// Two-finger pan.
    Pan,
    /// Wheel pinch (zoom).
    Zoom,
    /// Drag pinch.
    Pinch,
    /// Android back key.
    Back,
    /// Toggle fullscreen.
    Fullscreen,
    /// Hide / show the game's own on-screen buttons.
    HideUi,
    /// Open the game's chat.
    Chat,
}

impl Action {
    /// All actions in UI order.
    pub const ALL: [Action; 8] = [
        Action::Touch,
        Action::Pan,
        Action::Zoom,
        Action::Pinch,
        Action::Back,
        Action::Fullscreen,
        Action::HideUi,
        Action::Chat,
    ];

    /// Name used in `--bind NAME=BINDING` flags.
    pub fn name(self) -> &'static str {
        match self {
            Action::Touch => "touch",
            Action::Pan => "pan",
            Action::Zoom => "zoom",
            Action::Pinch => "pinch",
            Action::Back => "back",
            Action::Fullscreen => "fullscreen",
            Action::HideUi => "hide_ui",
            Action::Chat => "chat",
        }
    }

    /// The action called `name` (see [`Action::name`]).
    pub fn from_name(name: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.name() == name)
    }

    /// Label for the UI.
    pub fn label(self) -> &'static str {
        match self {
            Action::Touch => "Touch / drag",
            Action::Pan => "Pan (two fingers)",
            Action::Zoom => "Zoom (wheel pinch)",
            Action::Pinch => "Pinch (drag)",
            Action::Back => "Back",
            Action::Fullscreen => "Toggle fullscreen",
            Action::HideUi => "Hide game buttons",
            Action::Chat => "Open chat",
        }
    }
}

/// Mouse buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// A key (letters and digits are stored upper case).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Escape,
    Enter,
    Backspace,
    Delete,
    Tab,
    Space,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    F(u8),
    Char(char),
}

const NAMED: [(&str, Key); 12] = [
    ("Esc", Key::Escape),
    ("Enter", Key::Enter),
    ("Backspace", Key::Backspace),
    ("Delete", Key::Delete),
    ("Tab", Key::Tab),
    ("Space", Key::Space),
    ("Up", Key::Up),
    ("Down", Key::Down),
    ("Home", Key::Home),
    ("End", Key::End),
    ("ArrowLeft", Key::Left),
    ("ArrowRight", Key::Right),
];

impl Key {
    /// The `bhpc-input` key, `Other` for keys it does not know (function keys are `F(n)`).
    pub fn host_key(self) -> HostKey {
        match self {
            Key::Escape => HostKey::Escape,
            Key::Enter => HostKey::Enter,
            Key::Backspace => HostKey::Backspace,
            Key::Delete => HostKey::Delete,
            Key::Tab => HostKey::Tab,
            Key::Space => HostKey::Space,
            Key::Left => HostKey::Left,
            Key::Right => HostKey::Right,
            Key::Up => HostKey::Up,
            Key::Down => HostKey::Down,
            Key::Home => HostKey::Home,
            Key::End => HostKey::End,
            Key::F(n) => HostKey::F(n),
            Key::Char(c) => HostKey::Char(c.to_ascii_lowercase()),
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::F(n) => write!(f, "F{n}"),
            Key::Char(c) => write!(f, "{c}"),
            k => f.write_str(NAMED.iter().find(|(_, v)| v == k).map_or("?", |(n, _)| n)),
        }
    }
}

/// The thing that is pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Mouse(Button),
    Wheel,
    Key(Key),
}

/// Modifier set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// A modifier combination plus an input. Text form: `Ctrl+Shift+Left`, `Wheel`, `Esc`, `F11`, `Q`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub mods: Modifiers,
    pub input: Input,
}

/// Text that is not a binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid binding: {}", self.0)
    }
}

impl std::error::Error for ParseError {}

impl Binding {
    /// A binding without modifiers.
    pub fn plain(input: Input) -> Binding {
        Binding { mods: Modifiers::default(), input }
    }

    /// Parses the text form (case-insensitive).
    pub fn parse(text: &str) -> Result<Binding, ParseError> {
        let err = || ParseError(text.to_string());
        let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let last = parts.pop().unwrap_or_default();
        let mut mods = Modifiers::default();
        for m in parts {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" => mods.ctrl = true,
                "shift" => mods.shift = true,
                "alt" => mods.alt = true,
                _ => return Err(err()),
            }
        }
        let lower = last.to_ascii_lowercase();
        let input = match lower.as_str() {
            "left" => Input::Mouse(Button::Left),
            "right" => Input::Mouse(Button::Right),
            "middle" => Input::Mouse(Button::Middle),
            "wheel" => Input::Wheel,
            _ => Input::Key(parse_key(&lower).ok_or_else(err)?),
        };
        Ok(Binding { mods, input })
    }
}

fn parse_key(lower: &str) -> Option<Key> {
    if let Some((_, k)) = NAMED.iter().find(|(n, _)| n.eq_ignore_ascii_case(lower)) {
        return Some(*k);
    }
    // Browsers name the arrow keys `ArrowUp` / `ArrowDown`.
    match lower {
        "arrowup" => return Some(Key::Up),
        "arrowdown" => return Some(Key::Down),
        _ => {}
    }
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        return (1..=12).contains(&n).then_some(Key::F(n));
    }
    let mut chars = lower.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphanumeric() => Some(Key::Char(c.to_ascii_uppercase())),
        _ => None,
    }
}

impl fmt::Display for Binding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [(self.mods.ctrl, "Ctrl"), (self.mods.shift, "Shift"), (self.mods.alt, "Alt")] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        match self.input {
            Input::Mouse(Button::Left) => f.write_str("Left"),
            Input::Mouse(Button::Right) => f.write_str("Right"),
            Input::Mouse(Button::Middle) => f.write_str("Middle"),
            Input::Wheel => f.write_str("Wheel"),
            Input::Key(Key::Left) => f.write_str("ArrowLeft"),
            Input::Key(k) => write!(f, "{k}"),
        }
    }
}

impl Binding {
    /// The same binding as `bhpc-input` understands it.
    pub fn trigger(&self) -> bhpc_input::Trigger {
        let mods = bhpc_input::Mods { ctrl: self.mods.ctrl, shift: self.mods.shift, alt: self.mods.alt };
        let input = match self.input {
            Input::Mouse(Button::Left) => bhpc_input::Input::Mouse(bhpc_input::MouseButton::Left),
            Input::Mouse(Button::Right) => bhpc_input::Input::Mouse(bhpc_input::MouseButton::Right),
            Input::Mouse(Button::Middle) => bhpc_input::Input::Mouse(bhpc_input::MouseButton::Middle),
            Input::Wheel => bhpc_input::Input::Wheel,
            Input::Key(k) => bhpc_input::Input::Key(k.host_key()),
        };
        bhpc_input::Trigger { mods, input }
    }
}

impl Serialize for Binding {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Binding {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Binding::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// One binding per action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyBindings {
    pub touch: Binding,
    pub pan: Binding,
    pub zoom: Binding,
    pub pinch: Binding,
    pub back: Binding,
    pub fullscreen: Binding,
    pub hide_ui: Binding,
    pub chat: Binding,
}

impl Default for KeyBindings {
    fn default() -> Self {
        let ctrl = Modifiers { ctrl: true, ..Modifiers::default() };
        KeyBindings {
            touch: Binding::plain(Input::Mouse(Button::Left)),
            pan: Binding::plain(Input::Mouse(Button::Right)),
            zoom: Binding::plain(Input::Wheel),
            pinch: Binding { mods: ctrl, input: Input::Mouse(Button::Left) },
            back: Binding::plain(Input::Key(Key::Escape)),
            fullscreen: Binding::plain(Input::Key(Key::F(11))),
            hide_ui: Binding::plain(Input::Key(Key::Char('H'))),
            chat: Binding::plain(Input::Key(Key::Char('T'))),
        }
    }
}

impl KeyBindings {
    /// The binding of `action`.
    pub fn get(&self, action: Action) -> Binding {
        match action {
            Action::Touch => self.touch,
            Action::Pan => self.pan,
            Action::Zoom => self.zoom,
            Action::Pinch => self.pinch,
            Action::Back => self.back,
            Action::Fullscreen => self.fullscreen,
            Action::HideUi => self.hide_ui,
            Action::Chat => self.chat,
        }
    }

    /// Rebinds `action`.
    pub fn set(&mut self, action: Action, b: Binding) {
        match action {
            Action::Touch => self.touch = b,
            Action::Pan => self.pan = b,
            Action::Zoom => self.zoom = b,
            Action::Pinch => self.pinch = b,
            Action::Back => self.back = b,
            Action::Fullscreen => self.fullscreen = b,
            Action::HideUi => self.hide_ui = b,
            Action::Chat => self.chat = b,
        }
    }

    /// Pairs of actions that share a binding.
    pub fn conflicts(&self) -> Vec<(Action, Action)> {
        let mut out = Vec::new();
        for (i, a) in Action::ALL.iter().enumerate() {
            for b in &Action::ALL[i + 1..] {
                if self.get(*a) == self.get(*b) {
                    out.push((*a, *b));
                }
            }
        }
        out
    }
}

impl KeyBindings {
    /// The mapping `bhpc-input` applies.
    pub fn to_bindings(&self) -> bhpc_input::Bindings {
        bhpc_input::Bindings {
            touch: self.touch.trigger(),
            pan: self.pan.trigger(),
            zoom: self.zoom.trigger(),
            pinch: self.pinch.trigger(),
            back: self.back.trigger(),
            fullscreen: self.fullscreen.trigger(),
            hide_ui: self.hide_ui.trigger(),
            chat: self.chat.trigger(),
        }
    }

    /// `--bind ACTION=BINDING` flags for the actions that differ from the defaults.
    pub fn bind_flags(&self) -> Vec<String> {
        let default = KeyBindings::default();
        let mut out = Vec::new();
        for a in Action::ALL {
            if self.get(a) != default.get(a) {
                out.push("--bind".to_string());
                out.push(format!("{}={}", a.name(), self.get(a)));
            }
        }
        out
    }

    /// Applies one `ACTION=BINDING` value of a `--bind` flag.
    pub fn apply_flag(&mut self, flag: &str) -> Result<(), String> {
        let (name, text) = flag.split_once('=').ok_or_else(|| format!("--bind wants ACTION=BINDING, got {flag:?}"))?;
        let action = Action::from_name(name).ok_or_else(|| format!("unknown action {name:?}"))?;
        let binding = Binding::parse(text).map_err(|e| e.to_string())?;
        self.set(action, binding);
        Ok(())
    }
}

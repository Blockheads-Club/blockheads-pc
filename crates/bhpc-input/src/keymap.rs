//! Host key to Android `KeyEvent.KEYCODE_*` and unicode mapping.

use crate::event::{HostKey, Mods};

/// Android `KeyEvent.META_*` bits.
const META_SHIFT_ON: i32 = 0x1;
const META_ALT_ON: i32 = 0x2;
const META_CTRL_ON: i32 = 0x1000;

/// `KeyEvent.getMetaState` for `mods`.
pub fn meta_state(mods: Mods) -> i32 {
    let mut m = 0;
    if mods.shift {
        m |= META_SHIFT_ON;
    }
    if mods.alt {
        m |= META_ALT_ON;
    }
    if mods.ctrl {
        m |= META_CTRL_ON;
    }
    m
}

/// The key a USB HID usage id (an SDL scancode) stands for; `HostKey::Other` for the keys the mapper does not know.
pub fn host_key_from_hid(hid: u32) -> HostKey {
    match hid {
        n @ 4..=29 => HostKey::Char((b'a' + (n - 4) as u8) as char),
        n @ 30..=38 => HostKey::Char((b'1' + (n - 30) as u8) as char),
        39 => HostKey::Char('0'),
        // Return and the keypad's Enter.
        40 | 88 => HostKey::Enter,
        41 => HostKey::Escape,
        42 => HostKey::Backspace,
        43 => HostKey::Tab,
        44 => HostKey::Space,
        n @ 58..=69 => HostKey::F((n - 57) as u8),
        74 => HostKey::Home,
        76 => HostKey::Delete,
        77 => HostKey::End,
        79 => HostKey::Right,
        80 => HostKey::Left,
        81 => HostKey::Down,
        82 => HostKey::Up,
        _ => HostKey::Other,
    }
}

const LETTERS: [&str; 26] = [
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s", "t", "u", "v", "w",
    "x", "y", "z",
];
const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
const FKEYS: [&str; 12] = ["f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12"];
const KEYPAD: [&str; 10] = [
    "keypad-0", "keypad-1", "keypad-2", "keypad-3", "keypad-4", "keypad-5", "keypad-6", "keypad-7", "keypad-8",
    "keypad-9",
];

/// The key name of the mod API's `input` interface (mod-sdk `wit/blockheads-api/input.wit`) for a USB HID usage id
/// (an SDL scancode); `None` for keys it does not name.
pub fn api_key_name(hid: u32) -> Option<&'static str> {
    Some(match hid {
        n @ 4..=29 => LETTERS[(n - 4) as usize],
        n @ 30..=38 => DIGITS[(n - 29) as usize],
        39 => "0",
        40 | 88 => "enter",
        41 => "escape",
        42 => "backspace",
        43 => "tab",
        44 => "space",
        45 => "minus",
        46 => "equals",
        47 => "left-bracket",
        48 => "right-bracket",
        49 => "backslash",
        51 => "semicolon",
        52 => "apostrophe",
        53 => "grave",
        54 => "comma",
        55 => "period",
        56 => "slash",
        n @ 58..=69 => FKEYS[(n - 58) as usize],
        73 => "insert",
        74 => "home",
        75 => "page-up",
        76 => "delete",
        77 => "end",
        78 => "page-down",
        79 => "right",
        80 => "left",
        81 => "down",
        82 => "up",
        n @ 89..=97 => KEYPAD[(n - 88) as usize],
        98 => "keypad-0",
        224 => "left-ctrl",
        225 => "left-shift",
        226 => "left-alt",
        228 => "right-ctrl",
        229 => "right-shift",
        230 => "right-alt",
        _ => return None,
    })
}

/// Android key code for `key`, or `None` when the game has no use for it (`HostKey::Other`, and the
/// host-only function keys).
pub fn key_code(key: HostKey) -> Option<i32> {
    Some(match key {
        HostKey::Escape => crate::event::KEYCODE_BACK,
        HostKey::Enter => 66,
        HostKey::Backspace => 67, // KEYCODE_DEL
        HostKey::Delete => 112,   // KEYCODE_FORWARD_DEL
        HostKey::Tab => 61,
        HostKey::Space => 62,
        HostKey::Up => 19,
        HostKey::Down => 20,
        HostKey::Left => 21,
        HostKey::Right => 22,
        HostKey::Home => 122, // KEYCODE_MOVE_HOME
        HostKey::End => 123,  // KEYCODE_MOVE_END
        HostKey::Char(c) if c.is_ascii_digit() => 7 + (c as i32 - '0' as i32),
        HostKey::Char(c) if c.is_ascii_alphabetic() => 29 + (c.to_ascii_lowercase() as i32 - 'a' as i32),
        HostKey::F(_) | HostKey::Char(_) | HostKey::Other => return None,
    })
}

/// `KeyEvent.getUnicodeChar` for `key`: the printable character, 0 for non-printing keys.
pub fn unicode(key: HostKey, mods: Mods) -> i32 {
    match key {
        HostKey::Space => ' ' as i32,
        HostKey::Char(c) if mods.shift => c.to_ascii_uppercase() as i32,
        HostKey::Char(c) => c.to_ascii_lowercase() as i32,
        _ => 0,
    }
}

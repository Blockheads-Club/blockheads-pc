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
        40 => HostKey::Enter,
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

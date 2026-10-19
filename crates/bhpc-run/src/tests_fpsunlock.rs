//! The 30 FPS unlock: interval mapping, ObjC method lookup and the refresh-rate patch.

use crate::fpsunlock::*;
use bhpc_mem::GuestMemory;

const BASE: u32 = 0x4000_0000;

/// A class `Foo` at BASE with methods `setFrameInterval:` and `fire`, its metaclass with `+make` (no list when
/// `meta_list` is false), code with two 60.0 constants (one aligned only to 4) at BASE + 0x1000.
fn image(meta_list: bool) -> (GuestMemory, u32) {
    let m = GuestMemory::new().unwrap();
    m.map(BASE, 0x4000).unwrap();
    let w = |a: u32, v: u32| m.write_u32(BASE + a, v).unwrap();
    // class_t at 0, metaclass at 0x40; class_ro at 0x100 / 0x140; method lists at 0x200 / 0x240; names at 0x300.
    w(0, BASE + 0x40);
    w(16, (BASE + 0x100) | 1);
    w(0x40 + 16, BASE + 0x140);
    w(0x100 + 20, BASE + 0x200);
    w(0x140 + 20, if meta_list { BASE + 0x240 } else { 0 });
    w(0x200, 12 | 3);
    w(0x204, 2);
    for (i, (name, imp)) in [(0x300, 0x1000), (0x320, 0x1100)].into_iter().enumerate() {
        let e = 0x208 + 12 * i as u32;
        w(e, BASE + name);
        w(e + 4, 0);
        w(e + 8, BASE + imp);
    }
    w(0x240, 12);
    w(0x244, 1);
    w(0x248, BASE + 0x340);
    w(0x250, BASE + 0x1200);
    m.write_cstr(BASE + 0x300, "setFrameInterval:").unwrap();
    m.write_cstr(BASE + 0x320, "fire").unwrap();
    m.write_cstr(BASE + 0x340, "make").unwrap();
    m.write_bytes(BASE + 0x1010, &60.0f64.to_le_bytes()).unwrap();
    m.write_bytes(BASE + 0x1124, &60.0f64.to_le_bytes()).unwrap();
    // Past the tail of the last method: left alone.
    m.write_bytes(BASE + 0x1300, &60.0f64.to_le_bytes()).unwrap();
    (m, BASE)
}

#[test]
fn the_normal_interval_becomes_every_refresh_and_long_ones_keep_their_length() {
    assert_eq!(unlocked_interval(1), 1);
    assert_eq!(unlocked_interval(2), 1);
    assert_eq!(unlocked_interval(0), 1);
    // 30 refreshes of a 60 Hz display (the game's 2 fps idle mode) last half a second at UNLOCKED_HZ too.
    assert_eq!(f64::from(unlocked_interval(30)) / UNLOCKED_HZ, 0.5);
    assert_eq!(f64::from(unlocked_interval(3)) / UNLOCKED_HZ, 0.05);
}

#[test]
fn methods_are_found_in_the_class_metadata() {
    let (m, class) = image(true);
    assert_eq!(find_method(&m, class, "setFrameInterval:", false), Some(BASE + 0x1000));
    assert_eq!(find_method(&m, class, "fire", false), Some(BASE + 0x1100));
    assert_eq!(find_method(&m, class, "make", false), None);
    assert_eq!(find_method(&m, class, "make", true), Some(BASE + 0x1200));
    let (m, class) = image(false);
    assert!(methods(&m, class, true).is_empty());
    // Unreadable metadata: nothing.
    assert!(methods(&m, 0x10, false).is_empty());
}

#[test]
fn only_the_refresh_rate_constants_of_the_methods_change() {
    let (m, class) = image(true);
    let imps: Vec<u32> = methods(&m, class, false).into_iter().map(|(_, imp)| imp).collect();
    assert_eq!(patch_refresh_rate(&m, &imps, UNLOCKED_HZ), 2);
    let read = |a: u32| f64::from_le_bytes(m.read_bytes(BASE + a, 8).unwrap().try_into().unwrap());
    assert_eq!((read(0x1010), read(0x1124), read(0x1300)), (UNLOCKED_HZ, UNLOCKED_HZ, 60.0));
    // Already patched: nothing left to do.
    assert_eq!(patch_refresh_rate(&m, &imps, UNLOCKED_HZ), 0);
    assert_eq!(patch_refresh_rate(&m, &[], UNLOCKED_HZ), 0);
    // Code that cannot be read.
    assert_eq!(patch_refresh_rate(&m, &[0x10], UNLOCKED_HZ), 0);
}

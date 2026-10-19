//! Game speed: the flag value and the scaled world step.

use crate::gamespeed::*;

#[test]
fn speeds_parse_within_the_range() {
    assert_eq!(parse_speed("2"), Some(2.0));
    assert_eq!(parse_speed(" 0.5x"), Some(0.5));
    assert_eq!(parse_speed("1.25X"), Some(1.25));
    assert_eq!(parse_speed("0.25"), Some(0.25));
    assert_eq!(parse_speed("4"), Some(4.0));
    for bad in ["0.2", "4.5", "0", "-1", "fast", "", "NaN"] {
        assert_eq!(parse_speed(bad), None, "{bad}");
    }
}

#[test]
fn single_player_steps_are_scaled() {
    let step = |dt: f32, speed| f32::from_bits(scaled_step(dt.to_bits(), speed, 0, 0));
    assert_eq!(step(0.1, 3.0), 0.3);
    assert_eq!(step(1.0 / 60.0, 2.0), 2.0 / 60.0);
    assert_eq!(step(0.04, 0.5), 0.02);
    assert_eq!(step(0.0, 2.0), 0.0);
}

#[test]
fn multiplayer_steps_are_untouched() {
    let dt = 0.05f32.to_bits();
    assert_eq!(scaled_step(dt, 2.0, 0x4000_1000, 0), dt);
    assert_eq!(scaled_step(dt, 2.0, 0, 0x4000_2000), dt);
}

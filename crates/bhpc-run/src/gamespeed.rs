//! "Game speed": the world simulation runs faster or slower than real time, in single-player only.
//!
//! `-[EvolutionViewController drawFrame]` measures the time since the last frame, clamps it to 0.1 s and hands it
//! to `-[GameView update:accurateDT:]`, which moves the camera and calls
//! `-[World update:accurateDT:pinchScale:dragInProgress:]`. Everything in the world advances by that step there:
//! the clock (`worldTime += accurateDT * worldTimeMultiplier`, which the day/night cycle follows), blockheads, their
//! needs, creatures, plants, physics and machines (`DynamicWorld`). The game itself fast-forwards a sleep by
//! multiplying both steps by 20 at the start of that method, so a scaled step is something it is built for.
//!
//! The hook scales the two float arguments (`dt` in r2, `accurateDT` in r3, softfp) on entry, after the 0.1 s clamp.
//! The menus, the camera, rendering, sound and every host clock (timers, networking) keep real time.
//!
//! In multiplayer the server owns the world: a client (`World.client` set) or a host with players
//! (`World.server` set) runs at normal speed, since the same method also runs the heartbeat and timeout timers and
//! a faster local simulation would only fight the server's corrections.

/// Accepted speeds (inclusive).
pub use bhpc_launcher::window::GAME_SPEED_RANGE;

/// Parse a `--game-speed` value: a multiplier in [`GAME_SPEED_RANGE`].
pub fn parse_speed(v: &str) -> Option<f64> {
    let s: f64 = v.trim().trim_end_matches(['x', 'X']).parse().ok()?;
    (GAME_SPEED_RANGE.0..=GAME_SPEED_RANGE.1).contains(&s).then_some(s)
}

/// The world step for a frame step `dt` (raw f32 bits, as the game passes it) at `speed`, unless the world takes
/// part in a multiplayer game (`client`/`server`: the World's pointers of those names, 0 when absent).
pub fn scaled_step(bits: u32, speed: f64, client: u32, server: u32) -> u32 {
    if client != 0 || server != 0 {
        return bits;
    }
    ((f64::from(f32::from_bits(bits)) * speed) as f32).to_bits()
}

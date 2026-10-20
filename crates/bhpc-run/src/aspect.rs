//! The game's screen follows the window: its logical size is the window size divided by a UI scale, so any aspect
//! (16:10, 4:3, 21:9, a tall window) fills the window without bars.
//!
//! The game's `main` asks Apportable's UIKit for `+[UIScreenMode emulatedMode:11]`, which hands the display's pixel
//! size to `+[UIScreenMode bestEmulatedMode:]`: that picks the closest *iOS device* from a fixed list by aspect and
//! size (aspect <= 1.55: iPad 1024x768 / 2048x1536; < 1.9: iPhone 5/6/6+ 1136x640, 1334x750, 2208x1242; wider:
//! iPhone X sizes). `-[UIScreen _applyMode]` then publishes that size as the content size
//! (`VerdeActivity.setContentSize`), and the host letterboxes it into the window: on a 16:10 screen the game is a
//! 16:9 iPhone 5 picture with bars.
//!
//! Mode 12 is Apportable's *native* mode (`+[UINativeScreenMode nativeMode:]`): its size is the display's real
//! pixels (read live from `Display.getRealMetrics`) and its scale `VerdePluginNativeWidth() / 600`. The host asks for
//! mode 12 instead of 11 and replaces the scale given to `-[UIScreenMode initWithScale:]` by [`ui_scale`], which
//! keeps the iPhone 5 density the game was laid out for: at least [`MIN_POINTS`] points on each side, so the UI is
//! as big as before and a wider or taller window shows more of the world (squarer windows stay in the game's phone
//! layout, see [`PHONE_MAX_SHORT`]). All of this is patched in guest memory at
//! start; the game files are not changed.
//!
//! When the window changes size later, the host gives the game a new native mode of the new size (play mode,
//! `ScreenModeCalls`): the content size, framebuffer and `-[GameView resize:insets:]` follow, and the game's views
//! re-lay out through `windowInfoChanged:`. The scale stays the one of the first mode (the game copies it into its
//! `PIXEL_SCALE` once at start, and its views keep positions made with it), so the mode's size is not the window's
//! pixels but [`content_size`]: the window's aspect with the points a start at that size would give, drawn at the
//! first scale and stretched to the window by the host.

/// The smallest logical size (long side, short side) in points: the iPhone 5 in landscape.
pub const MIN_POINTS: (f32, f32) = (568.0, 320.0);
/// The game's menus and HUD switch to their iPad layout (made for 1024x768 points) once both sides reach 415
/// points (`-[MainMenuUI render:projectionMatrix:]` and others compare with 415.0): the phone layout keeps the short
/// side below that.
pub const PHONE_MAX_SHORT: f32 = 414.0;
/// The narrowest phone the game supports (iPhone 4, 480x320 points).
pub const PHONE_MIN_LONG: f32 = 480.0;
/// The iPad's logical size (long, short): the layout for windows too square for the phone layout.
pub const PAD_POINTS: (f32, f32) = (1024.0, 768.0);
/// `+[UIScreenMode emulatedMode:]`: the best emulated iOS device for the display (what the game's `main` asks for).
pub const MODE_BEST_EMULATED: u32 = 11;
/// `+[UIScreenMode emulatedMode:]`: the display's native pixels.
pub const MODE_NATIVE: u32 = 12;
/// How far after the start of `+[UINativeScreenMode nativeMode:]` its call of `initWithScale:` returns.
pub const NATIVE_MODE_LEN: u32 = 0x100;

/// Display pixels per point for a display of `width` x `height` pixels (either orientation). The phone layout:
/// the largest scale that keeps [`MIN_POINTS`], but small enough to keep the short side within
/// [`PHONE_MAX_SHORT`]; a window too square for that (long side under [`PHONE_MIN_LONG`]) gets the iPad layout of
/// [`PAD_POINTS`]. `1.0` for an empty display.
pub fn ui_scale(width: i32, height: i32) -> f32 {
    let (long, short) = (width.max(height) as f32, width.min(height) as f32);
    if short <= 0.0 {
        return 1.0;
    }
    let phone = (long / MIN_POINTS.0).min(short / MIN_POINTS.1).max(short / PHONE_MAX_SHORT);
    if long / phone >= PHONE_MIN_LONG {
        phone
    } else {
        (long / PAD_POINTS.0).min(short / PAD_POINTS.1)
    }
}

/// The mode to create instead of `requested` (`emulatedMode:`'s argument): the native mode for the best emulated
/// device, anything else unchanged.
pub fn screen_mode(requested: u32) -> u32 {
    if requested == MODE_BEST_EMULATED {
        MODE_NATIVE
    } else {
        requested
    }
}

/// `initWithScale:` was called by `nativeMode:` (implementation at `native_mode`) when it returns to `lr`.
pub fn called_from_native_mode(lr: u32, native_mode: u32) -> bool {
    lr.wrapping_sub(native_mode) < NATIVE_MODE_LEN
}

/// The logical size (points) the game gets for a display of `width` x `height` pixels.
pub fn points(width: i32, height: i32) -> (f32, f32) {
    let s = ui_scale(width, height);
    (width as f32 / s, height as f32 / s)
}

/// Drawable (physical) pixels of a window of `logical` size, with the pixel ratio of a window whose sizes are
/// `window_logical` and `window_drawable` (HiDPI); `logical` itself without a usable ratio.
pub fn drawable_for(logical: (u32, u32), window_logical: (u32, u32), window_drawable: (u32, u32)) -> (u32, u32) {
    let ratio = |d: u32, l: u32| if l > 0 && d > 0 { d as f64 / l as f64 } else { 1.0 };
    let (rx, ry) = (ratio(window_drawable.0, window_logical.0), ratio(window_drawable.1, window_logical.1));
    ((logical.0 as f64 * rx).round() as u32, (logical.1 as f64 * ry).round() as u32)
}

/// The size in pixels of the native mode for a display of `width` x `height` pixels when the game's scale is
/// `scale` (the first mode's): the display's aspect and the points of [`ui_scale`], so the layout is the one a start
/// at this size gets. The display itself while `scale` is its own scale.
pub fn content_size(width: i32, height: i32, scale: f32) -> (f32, f32) {
    let k = scale / ui_scale(width, height);
    ((width as f32 * k).round().max(1.0), (height as f32 * k).round().max(1.0))
}

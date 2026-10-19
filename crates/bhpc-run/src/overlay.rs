//! Hands the rendered Java view tree to the window: the views (popups, dialogs) are not part of the game's GL frame,
//! so the window blends them over it when it presents the frame. Also the pictures of the controller cursor and of
//! short host messages ([`SpriteCache`]).

use bhpc_app::hud::{cursor_picture, hud_scale, message_sprite, Sprite};
use bhpc_gles::{GlWindow, Overlay};
use bhpc_jni::UiFrame;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Remembers which frame the window already has, so a picture is handed over (shared, not copied) only when it
/// changed.
#[derive(Default)]
pub struct OverlayFeed {
    /// `Some(serial)` while the window shows an overlay.
    sent: Mutex<Option<u64>>,
}

impl OverlayFeed {
    /// Give the window the current picture (`None`: nothing to draw). Call before the frame is presented, from the
    /// thread that presents it.
    pub fn update(&self, window: &dyn GlWindow, frame: Option<Arc<UiFrame>>) {
        let mut sent = self.sent.lock().unwrap_or_else(PoisonError::into_inner);
        match frame {
            Some(f) if *sent != Some(f.serial) => {
                window.set_overlay(Some(Overlay {
                    width: f.width,
                    height: f.height,
                    serial: f.serial,
                    rgba: f.rgba.clone(),
                }));
                *sent = Some(f.serial);
            }
            Some(_) => {}
            None if sent.take().is_some() => window.set_overlay(None),
            None => {}
        }
    }
}

/// What the controller cursor and the host's messages look like right now (kept by the UI thread, drawn when a frame
/// is presented).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PadView {
    /// Cursor position in content pixels, while it shows.
    pub cursor: Option<(f32, f32)>,
    /// Size of a tile on screen in content pixels (the cursor frames one tile), when known.
    pub tile: Option<f32>,
    /// Size of the content (touch) space.
    pub content: (f32, f32),
    /// A message and when it was posted.
    pub message: Option<(String, Instant)>,
}

/// How long a message stays up.
pub const MESSAGE_TIME: Duration = Duration::from_millis(2500);
/// Cursor size (content pixels) when the tile size is unknown.
pub const CURSOR_SIZE: f32 = 32.0;

/// The pictures of a [`PadView`], rebuilt only when they change.
#[derive(Default)]
pub struct SpriteCache {
    cursor: Option<(u32, Overlay)>,
    message: Option<(String, u32, Overlay)>,
    serial: u64,
}

impl SpriteCache {
    fn next(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }

    /// The sprites for `view` on a game picture of `picture` pixels (the cursor first, then the message).
    pub fn sprites(&mut self, view: &PadView, picture: (u32, u32), now: Instant) -> Vec<Sprite> {
        let (cw, ch) = view.content;
        if cw <= 0.0 || ch <= 0.0 || picture.0 == 0 || picture.1 == 0 {
            return Vec::new();
        }
        let scale = (picture.0 as f32 / cw).min(picture.1 as f32 / ch);
        let mut out = Vec::new();
        if let Some((x, y)) = view.cursor {
            let size = (view.tile.unwrap_or(CURSOR_SIZE) * scale).round() as u32;
            if self.cursor.as_ref().is_none_or(|(s, _)| *s != size) {
                let p = cursor_picture(size);
                let serial = self.next();
                self.cursor =
                    Some((size, Overlay { width: p.width, height: p.height, serial, rgba: Arc::new(p.rgba) }));
            }
            if let Some((_, ov)) = &self.cursor {
                out.push(Sprite { overlay: ov.clone(), at: (x / cw, y / ch) });
            }
        }
        if let Some((text, _)) = view.message.as_ref().filter(|(_, at)| now.duration_since(*at) < MESSAGE_TIME) {
            let scale = hud_scale(picture.1);
            if self.message.as_ref().is_none_or(|(t, s, _)| t != text || *s != scale) {
                let serial = self.next();
                let sp = message_sprite(text, scale, serial, (0.5, 0.12));
                self.message = Some((text.clone(), scale, sp.overlay));
            }
            if let Some((_, _, ov)) = &self.message {
                out.push(Sprite { overlay: ov.clone(), at: (0.5, 0.12) });
            }
        }
        out
    }
}

//! Hands the rendered Java view tree to the window: the views (popups, dialogs) are not part of the game's GL frame,
//! so the window blends them over it when it presents the frame.

use bhpc_gles::{GlWindow, Overlay};
use bhpc_jni::UiFrame;
use std::sync::{Arc, Mutex, PoisonError};

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

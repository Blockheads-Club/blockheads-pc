//! Hands the rendered Java view tree to the window: the views (popups, dialogs) are not part of the game's GL frame,
//! so the window blends them over it when it presents the frame. Also the pictures of the controller cursor and of
//! short host messages ([`SpriteCache`]).

use bhpc_app::hud::{
    cursor_picture, hud_scale, localise, message_sprite, mic_picture, overhead_picture, Lang, Overhead, Sprite,
};
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

/// What a composed frame was made from: the views' serial, the mod picture, the size.
type ComposeKey = (u64, usize, u32, u32);

/// Puts a picture of the mod UI (premultiplied RGBA of the same size) over the Java views' frame, so the window
/// gets one overlay. The result is rebuilt only when either input changes.
#[derive(Default)]
pub struct Composer {
    last: Option<(ComposeKey, Arc<UiFrame>)>,
    serial: u64,
}

impl Composer {
    /// `base`: the Java views (or `None`); `top`: the mod UI picture and the window size it was drawn for.
    pub fn compose(
        &mut self,
        base: Option<Arc<UiFrame>>,
        top: Option<(Arc<Vec<u8>>, u32, u32)>,
    ) -> Option<Arc<UiFrame>> {
        let Some((top, w, h)) = top else {
            self.last = None;
            return base;
        };
        let base = base.filter(|b| (b.width, b.height) == (w, h));
        let key = (base.as_ref().map_or(0, |b| b.serial), Arc::as_ptr(&top) as usize, w, h);
        if let Some((k, f)) = &self.last {
            if *k == key {
                return Some(f.clone());
            }
        }
        let mut px = match &base {
            Some(b) => b.rgba.as_ref().clone(),
            None => vec![0; top.len()],
        };
        for (d, s) in px.chunks_exact_mut(4).zip(top.chunks_exact(4)) {
            let a = u32::from(s[3]);
            if a == 0 {
                continue;
            }
            for i in 0..4 {
                d[i] = (u32::from(s[i]) + u32::from(d[i]) * (255 - a) / 255).min(255) as u8;
            }
        }
        self.serial += 1;
        let f = Arc::new(UiFrame { width: w, height: h, rgba: Arc::new(px), serial: (1 << 63) | self.serial });
        self.last = Some((key, f.clone()));
        Some(f)
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
    /// The microphone records (voice chat): the host's mic-on indicator shows. Set by the host only, from
    /// `bhpc_audio::capture::MicControl::live`; mods have no way to change it.
    pub mic: bool,
    /// Voice chat icons above blockheads (mod API `ui.set-overhead`), at content pixels.
    pub overheads: Vec<(Overhead, (f32, f32))>,
}

/// Where the mic-on indicator sits (top-right corner of the game picture).
pub const MIC_AT: (f32, f32) = (0.965, 0.06);

/// How long a message stays up.
pub const MESSAGE_TIME: Duration = Duration::from_millis(2500);
/// Cursor size (content pixels) when the tile size is unknown.
pub const CURSOR_SIZE: f32 = 32.0;

/// The pictures of a [`PadView`], rebuilt only when they change.
#[derive(Default)]
pub struct SpriteCache {
    cursor: Option<(u32, Overlay)>,
    message: Option<(String, u32, Overlay)>,
    mic: Option<(u32, Overlay)>,
    overheads: Vec<(Overhead, u32, Overlay)>,
    serial: u64,
    /// The messages' language.
    lang: Lang,
}

impl SpriteCache {
    /// Messages shown in `lang`.
    pub fn new(lang: Lang) -> SpriteCache {
        SpriteCache { lang, ..SpriteCache::default() }
    }

    fn next(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }

    /// The sprites for `view` on a game picture of `picture` pixels (the cursor first, then the message, then the mic-on indicator).
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
                let sp = message_sprite(&localise(self.lang, text), scale, serial, (0.5, 0.12));
                self.message = Some((text.clone(), scale, sp.overlay));
            }
            if let Some((_, _, ov)) = &self.message {
                out.push(Sprite { overlay: ov.clone(), at: (0.5, 0.12) });
            }
        }
        if !view.overheads.is_empty() {
            let size = 9 * hud_scale(picture.1);
            for (kind, (x, y)) in &view.overheads {
                let at = self.overheads.iter().position(|(k, s, _)| k == kind && *s == size);
                let at = at.unwrap_or_else(|| {
                    let p = overhead_picture(*kind, size);
                    let serial = self.next();
                    let ov = Overlay { width: p.width, height: p.height, serial, rgba: Arc::new(p.rgba) };
                    self.overheads.retain(|(_, s, _)| *s == size);
                    self.overheads.push((*kind, size, ov));
                    self.overheads.len() - 1
                });
                out.push(Sprite { overlay: self.overheads[at].2.clone(), at: (x / cw, y / ch) });
            }
        }
        if view.mic {
            let size = 12 * hud_scale(picture.1);
            if self.mic.as_ref().is_none_or(|(s, _)| *s != size) {
                let p = mic_picture(size);
                let serial = self.next();
                self.mic = Some((size, Overlay { width: p.width, height: p.height, serial, rgba: Arc::new(p.rgba) }));
            }
            if let Some((_, ov)) = &self.mic {
                out.push(Sprite { overlay: ov.clone(), at: MIC_AT });
            }
        }
        out
    }
}

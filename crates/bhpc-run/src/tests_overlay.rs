//! The overlay feed hands each new picture to the window once.

use crate::overlay::OverlayFeed;
use bhpc_gles::{GlWindow, Overlay};
use bhpc_jni::UiFrame;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Window(Mutex<Vec<Option<u64>>>);

impl GlWindow for Window {
    fn create_context(&self, _: Option<bhpc_gles::HostContext>) -> Option<bhpc_gles::HostContext> {
        None
    }
    fn destroy_context(&self, _: bhpc_gles::HostContext) {}
    fn make_current(&self, _: Option<bhpc_gles::HostContext>) -> bool {
        true
    }
    fn swap_buffers(&self) {}
    fn set_swap_interval(&self, _: i32) -> bool {
        true
    }
    fn drawable_size(&self) -> (u32, u32) {
        (2, 1)
    }
    fn proc_address(&self, _: &str) -> *const c_void {
        std::ptr::null()
    }
    fn set_overlay(&self, o: Option<Overlay>) {
        self.0.lock().unwrap().push(o.map(|o| o.serial));
    }
}

fn frame(serial: u64) -> Option<Arc<UiFrame>> {
    Some(Arc::new(UiFrame { width: 2, height: 1, rgba: Arc::new(vec![0; 8]), serial }))
}

#[test]
fn only_changes_reach_the_window() {
    let (feed, w) = (OverlayFeed::default(), Window::default());
    feed.update(&w, None);
    feed.update(&w, frame(1));
    feed.update(&w, frame(1));
    feed.update(&w, frame(2));
    feed.update(&w, None);
    feed.update(&w, None);
    assert_eq!(*w.0.lock().unwrap(), vec![Some(1), Some(2), None]);
}

#[test]
fn the_window_default_ignores_overlays() {
    struct Bare;
    impl GlWindow for Bare {
        fn create_context(&self, _: Option<u64>) -> Option<u64> {
            None
        }
        fn destroy_context(&self, _: u64) {}
        fn make_current(&self, _: Option<u64>) -> bool {
            true
        }
        fn swap_buffers(&self) {}
        fn set_swap_interval(&self, _: i32) -> bool {
            true
        }
        fn drawable_size(&self) -> (u32, u32) {
            (1, 1)
        }
        fn proc_address(&self, _: &str) -> *const c_void {
            std::ptr::null()
        }
    }
    Bare.set_overlay(None);
}

#[test]
fn the_cursor_and_messages_become_sprites_rebuilt_only_on_change() {
    use crate::overlay::{PadView, SpriteCache, MESSAGE_TIME};
    use std::time::{Duration, Instant};
    let now = Instant::now();
    let mut cache = SpriteCache::default();
    let mut v = PadView { cursor: Some((250.0, 200.0)), tile: None, content: (1000.0, 800.0), message: None };
    // A 2000x1600 picture: twice the content, the default cursor is 64 px.
    let s = cache.sprites(&v, (2000, 1600), now);
    assert_eq!(s.len(), 1);
    assert_eq!((s[0].overlay.width, s[0].at), (64, (0.25, 0.25)));
    let serial = s[0].overlay.serial;
    v.cursor = Some((500.0, 400.0));
    let s = cache.sprites(&v, (2000, 1600), now);
    assert_eq!((s[0].overlay.serial, s[0].at), (serial, (0.5, 0.5)), "same picture, new place");
    v.tile = Some(20.0);
    let s = cache.sprites(&v, (2000, 1600), now);
    assert_eq!(s[0].overlay.width, 40);
    assert_ne!(s[0].overlay.serial, serial);
    // A message shows for a while, under the cursor's slot.
    v.message = Some(("GAME BUTTONS HIDDEN".into(), now));
    let s = cache.sprites(&v, (2000, 1600), now + Duration::from_millis(10));
    assert_eq!(s.len(), 2);
    assert_eq!(s[1].at, (0.5, 0.12));
    let m = s[1].overlay.serial;
    assert_eq!(cache.sprites(&v, (2000, 1600), now).last().unwrap().overlay.serial, m);
    v.cursor = None;
    let s = cache.sprites(&v, (2000, 1600), now + MESSAGE_TIME - Duration::from_millis(1));
    assert_eq!((s.len(), s[0].overlay.serial), (1, m));
    assert!(cache.sprites(&v, (2000, 1600), now + MESSAGE_TIME).is_empty(), "expired");
    v.message = Some(("GAME BUTTONS SHOWN".into(), now));
    assert_ne!(cache.sprites(&v, (2000, 1600), now)[0].overlay.serial, m);
    // Nothing to place things on.
    assert!(cache.sprites(&v, (0, 1600), now).is_empty());
    assert!(cache.sprites(&PadView::default(), (2000, 1600), now).is_empty());
}

#[test]
fn messages_are_drawn_in_the_launcher_language() {
    use crate::overlay::{PadView, SpriteCache};
    use bhpc_app::hud::{hud_scale, render_label, Lang};
    use std::time::Instant;
    let now = Instant::now();
    let v = PadView {
        cursor: None,
        tile: None,
        content: (1000.0, 800.0),
        message: Some(("GAME BUTTONS SHOWN".into(), now)),
    };
    let s = SpriteCache::new(Lang::Ru).sprites(&v, (1000, 800), now);
    let want = render_label("КНОПКИ ИГРЫ ПОКАЗАНЫ", hud_scale(800));
    assert_eq!((s[0].overlay.width, &*s[0].overlay.rgba), (want.width, &want.rgba));
}

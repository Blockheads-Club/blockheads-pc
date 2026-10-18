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
    Some(Arc::new(UiFrame { width: 2, height: 1, rgba: vec![0; 8], serial }))
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

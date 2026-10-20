//! The windowing side of EGL, implemented by `bhpc-app` (SDL2) and by mocks in tests.

use std::ffi::c_void;
use std::sync::Arc;

/// Pixels the window draws over the game's frame (the Java view tree rendered by the fake JVM): premultiplied RGBA,
/// top row first, covering the whole drawable. `serial` changes with the pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlay {
    pub width: u32,
    pub height: u32,
    pub serial: u64,
    pub rgba: Arc<Vec<u8>>,
}

impl Overlay {
    /// The rows `first..end` outside of which this picture equals `prev` (same size), so a window that holds `prev`
    /// only uploads those; `None` when the pictures are equal, all rows when the sizes differ.
    pub fn changed_rows(&self, prev: &Overlay) -> Option<(u32, u32)> {
        if (self.width, self.height) != (prev.width, prev.height) || self.rgba.len() != prev.rgba.len() {
            return Some((0, self.height));
        }
        let stride = (self.width as usize * 4).max(1);
        let rows = || self.rgba.chunks(stride).zip(prev.rgba.chunks(stride));
        let first = rows().position(|(a, b)| a != b)?;
        let last = rows().rposition(|(a, b)| a != b).unwrap_or(first);
        Some((first as u32, last as u32 + 1))
    }
}

/// Opaque handle of a host GL context created by a [`GlWindow`].
pub type HostContext = u64;

/// One window with GL contexts. All methods may be called from any guest thread; SDL's rule that a context is
/// current on one thread at a time is kept by `Gles`, which only makes a context current in the calling thread.
pub trait GlWindow: Send + Sync {
    /// Create a GLES2 context, optionally sharing objects with `share`. `None` on failure.
    fn create_context(&self, share: Option<HostContext>) -> Option<HostContext>;
    /// Destroy a context created by [`GlWindow::create_context`].
    fn destroy_context(&self, ctx: HostContext);
    /// Make `ctx` current in the calling thread (`None` releases the current context). False on failure.
    fn make_current(&self, ctx: Option<HostContext>) -> bool;
    /// Present the back buffer of the window.
    fn swap_buffers(&self);
    /// Set the swap interval (0 = immediate, 1 = vsync). False if unsupported.
    fn set_swap_interval(&self, interval: i32) -> bool;
    /// Size of the drawable in pixels.
    fn drawable_size(&self) -> (u32, u32);
    /// Address of a GL function of the current context (null if unknown).
    fn proc_address(&self, name: &str) -> *const c_void;
    /// The contexts are desktop OpenGL (compatibility profile), not OpenGL ES: shader sources are translated
    /// ([`crate::glsl::to_desktop`]).
    fn desktop_gl(&self) -> bool {
        false
    }

    // The game renders into a surface of its own size (`VerdeActivity.setContentSize`), like an Android
    // `SurfaceHolder.setFixedSize`; the window scales it to the drawable. A window that supports this keeps an
    // offscreen target of that size and presents it letterboxed. The defaults render straight into the window.

    /// The game's surface size changed (called without a current context; applied lazily).
    fn set_surface_size(&self, _width: u32, _height: u32) {}
    /// Size of the surface the game renders into (what `eglQuerySurface` reports).
    fn surface_size(&self) -> (u32, u32) {
        self.drawable_size()
    }
    /// The host framebuffer object that stands for the game's default framebuffer in the current context
    /// (0 when the window itself is the target).
    fn default_framebuffer(&self) -> u32 {
        0
    }
    /// Set (or clear) the overlay that [`GlWindow::compose`] draws on top of the game's surface, blended, over the
    /// whole drawable. Needs no current context.
    fn set_overlay(&self, _overlay: Option<Overlay>) {}
    /// Draw the game's surface into the window's back buffer (letterboxed), without swapping. Needs the game's
    /// context to be current and leaves its GL state untouched.
    fn compose(&self) {}
}

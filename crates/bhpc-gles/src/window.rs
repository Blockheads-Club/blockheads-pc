//! The windowing side of EGL, implemented by `bhpc-app` (SDL2) and by mocks in tests.

use std::ffi::c_void;

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
    /// Draw the game's surface into the window's back buffer (letterboxed), without swapping. Needs the game's
    /// context to be current and leaves its GL state untouched.
    fn compose(&self) {}
}

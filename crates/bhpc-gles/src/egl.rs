//! EGL on top of a [`GlWindow`]: one display, one config (RGBA8888, 24-bit depth, 8-bit stencil, ES2), contexts
//! backed by host contexts, window surfaces backed by the single window.
//!
//! The Rust methods of [`Gles`] (`egl_*`) are what the Java-side `VerdeActivity` natives (`makeCurrentContext`,
//! `createSharedContext`, `swapBuffers`, ...) map to in `bhpc-jni`; the thunks are the guest-visible `egl*`.

use crate::consts::*;
use crate::marshal::args;
use crate::state::{guest_cstr, guest_write, Gles};
use crate::window::HostContext;
use crate::Handler;
use bhpc_cpu::Cpu;
use std::collections::{HashMap, HashSet};
use std::thread::ThreadId;

/// The only display handle.
pub const EGL_DISPLAY: u32 = 1;
/// The only config handle.
pub const EGL_CONFIG: u32 = 1;
const FIRST_HANDLE: u32 = 0x1000;

pub(crate) struct EglState {
    pub error: u32,
    pub initialized: bool,
    pub contexts: HashMap<u32, HostContext>,
    pub surfaces: HashSet<u32>,
    pub current: HashMap<ThreadId, (u32, u32)>,
    pub swap_interval: i32,
    /// Runs in the swapping guest thread right before every `eglSwapBuffers` of the guest.
    pub frame_hook: Option<FrameHook>,
    next: u32,
}

/// Callback of [`Gles::set_frame_hook`].
pub type FrameHook = std::sync::Arc<dyn Fn(&mut Cpu) + Send + Sync>;

impl Default for EglState {
    fn default() -> Self {
        EglState {
            error: EGL_SUCCESS,
            initialized: false,
            contexts: HashMap::new(),
            surfaces: HashSet::new(),
            current: HashMap::new(),
            swap_interval: 1,
            frame_hook: None,
            next: FIRST_HANDLE,
        }
    }
}

impl EglState {
    fn handle(&mut self) -> u32 {
        let h = self.next;
        self.next += 0x10;
        h
    }
}

/// Value of a config attribute of the single config.
pub fn config_attrib(attr: u32) -> Option<u32> {
    Some(match attr {
        EGL_BUFFER_SIZE => 32,
        EGL_RED_SIZE | EGL_GREEN_SIZE | EGL_BLUE_SIZE | EGL_ALPHA_SIZE => 8,
        EGL_DEPTH_SIZE => 24,
        EGL_STENCIL_SIZE => 8,
        EGL_CONFIG_ID | EGL_NATIVE_VISUAL_ID => 1,
        EGL_CONFIG_CAVEAT | EGL_NATIVE_VISUAL_TYPE | EGL_TRANSPARENT_TYPE => EGL_NONE,
        EGL_LEVEL | EGL_SAMPLES | EGL_SAMPLE_BUFFERS | EGL_NATIVE_RENDERABLE => 0,
        EGL_SURFACE_TYPE => EGL_WINDOW_BIT,
        EGL_RENDERABLE_TYPE => EGL_OPENGL_ES2_BIT | EGL_OPENGL_ES_BIT,
        EGL_MAX_PBUFFER_WIDTH | EGL_MAX_PBUFFER_HEIGHT => 4096,
        EGL_MAX_PBUFFER_PIXELS => 4096 * 4096,
        _ => return None,
    })
}

/// Does the single config satisfy the `(attribute, value)` pairs of an `eglChooseConfig` call?
pub fn config_matches(attribs: &[(u32, u32)]) -> bool {
    attribs.iter().all(|&(a, v)| match a {
        EGL_RED_SIZE | EGL_GREEN_SIZE | EGL_BLUE_SIZE | EGL_ALPHA_SIZE | EGL_DEPTH_SIZE | EGL_STENCIL_SIZE => {
            v as i32 == EGL_DONT_CARE || v <= config_attrib(a).unwrap_or(0)
        }
        EGL_SAMPLES | EGL_SAMPLE_BUFFERS => v == 0 || v as i32 == EGL_DONT_CARE,
        EGL_SURFACE_TYPE => v & !EGL_WINDOW_BIT == 0 || v as i32 == EGL_DONT_CARE,
        EGL_RENDERABLE_TYPE => v == 0 || v & (EGL_OPENGL_ES2_BIT | EGL_OPENGL_ES_BIT) != 0 || v as i32 == EGL_DONT_CARE,
        _ => true,
    })
}

impl Gles {
    /// Install a callback that runs in the guest thread that calls `eglSwapBuffers`, before the frame is presented
    /// (the embedder uses it to run work on the game's render thread).
    pub fn set_frame_hook(&self, hook: FrameHook) {
        self.lock().egl.frame_hook = Some(hook);
    }

    fn egl_fail(&self, e: u32) -> u32 {
        self.lock().egl.error = e;
        e
    }

    fn egl_ok(&self) {
        self.lock().egl.error = EGL_SUCCESS;
    }

    fn egl_check_display(&self, dpy: u32) -> Result<(), u32> {
        let s = self.lock();
        if dpy != EGL_DISPLAY {
            Err(EGL_BAD_DISPLAY)
        } else if !s.egl.initialized {
            Err(EGL_NOT_INITIALIZED)
        } else {
            Ok(())
        }
    }

    /// `eglInitialize`.
    pub fn egl_initialize(&self) {
        self.lock().egl.initialized = true;
        self.egl_ok();
    }

    /// Create a context (optionally sharing objects with the context handle `share`, 0 for none).
    pub fn egl_create_context(&self, share: u32) -> Result<u32, u32> {
        let host_share = if share == 0 {
            None
        } else {
            let known = self.lock().egl.contexts.get(&share).copied();
            match known {
                Some(h) => Some(h),
                None => return Err(self.egl_fail(EGL_BAD_CONTEXT)),
            }
        };
        let Some(host) = self.window.create_context(host_share) else { return Err(self.egl_fail(EGL_BAD_ALLOC)) };
        let mut s = self.lock();
        let id = s.egl.handle();
        s.egl.contexts.insert(id, host);
        s.egl.error = EGL_SUCCESS;
        Ok(id)
    }

    /// Register a host context that the embedder created itself (for example the one it made current to load the
    /// driver's entry points) as an EGL context, and return its handle.
    pub fn egl_adopt_context(&self, host: HostContext) -> u32 {
        let mut s = self.lock();
        let id = s.egl.handle();
        s.egl.contexts.insert(id, host);
        id
    }

    /// Destroy a context. Releases it first if it is current in the calling thread.
    pub fn egl_destroy_context(&self, ctx: u32) -> Result<(), u32> {
        let host = self.lock().egl.contexts.get(&ctx).copied();
        let Some(host) = host else { return Err(self.egl_fail(EGL_BAD_CONTEXT)) };
        let me = std::thread::current().id();
        let was_current = {
            let mut s = self.lock();
            s.egl.contexts.remove(&ctx);
            let cur = s.egl.current.get(&me).is_some_and(|c| c.0 == ctx);
            if cur {
                s.egl.current.remove(&me);
            }
            cur
        };
        if was_current {
            self.window.make_current(None);
        }
        self.window.destroy_context(host);
        self.egl_ok();
        Ok(())
    }

    /// `eglCreateWindowSurface`: a surface backed by the window.
    pub fn egl_create_surface(&self) -> u32 {
        let mut s = self.lock();
        let id = s.egl.handle();
        s.egl.surfaces.insert(id);
        s.egl.error = EGL_SUCCESS;
        id
    }

    /// `eglDestroySurface`.
    pub fn egl_destroy_surface(&self, surface: u32) -> Result<(), u32> {
        if !self.lock().egl.surfaces.remove(&surface) {
            return Err(self.egl_fail(EGL_BAD_SURFACE));
        }
        self.egl_ok();
        Ok(())
    }

    /// `eglMakeCurrent` for the calling thread. `ctx == 0` releases the current context.
    pub fn egl_make_current(&self, draw: u32, read: u32, ctx: u32) -> Result<(), u32> {
        let me = std::thread::current().id();
        if ctx == 0 {
            self.window.make_current(None);
            self.lock().egl.current.remove(&me);
            self.egl_ok();
            return Ok(());
        }
        let host = {
            let s = self.lock();
            let known = |h: u32| h == 0 || s.egl.surfaces.contains(&h);
            match s.egl.contexts.get(&ctx) {
                None => return Err(drop_and(s, self, EGL_BAD_CONTEXT)),
                Some(_) if !known(draw) || !known(read) => return Err(drop_and(s, self, EGL_BAD_SURFACE)),
                Some(h) => *h,
            }
        };
        if !self.window.make_current(Some(host)) {
            return Err(self.egl_fail(EGL_BAD_ACCESS));
        }
        self.lock().egl.current.insert(me, (ctx, draw));
        self.egl_ok();
        Ok(())
    }

    /// Context current in the calling thread (0 if none): `eglGetCurrentContext`.
    pub fn egl_current_context(&self) -> u32 {
        self.lock().egl.current.get(&std::thread::current().id()).map_or(0, |c| c.0)
    }

    /// Draw surface current in the calling thread (0 if none).
    pub fn egl_current_surface(&self) -> u32 {
        self.lock().egl.current.get(&std::thread::current().id()).map_or(0, |c| c.1)
    }

    /// `eglSwapBuffers`: present the window. The surface must be valid and a context current in this thread.
    pub fn egl_swap_buffers(&self, surface: u32) -> Result<(), u32> {
        {
            let s = self.lock();
            if !s.egl.surfaces.contains(&surface) {
                return Err(drop_and(s, self, EGL_BAD_SURFACE));
            }
            if !s.egl.current.contains_key(&std::thread::current().id()) {
                return Err(drop_and(s, self, EGL_BAD_CONTEXT));
            }
        }
        self.window.swap_buffers();
        self.egl_ok();
        Ok(())
    }

    /// `eglSwapInterval`.
    pub fn egl_swap_interval(&self, interval: i32) -> bool {
        let ok = self.window.set_swap_interval(interval.max(0));
        if ok {
            self.lock().egl.swap_interval = interval.max(0);
        }
        ok
    }
}

/// Record `e` as the EGL error after releasing the state lock.
fn drop_and(s: std::sync::MutexGuard<'_, crate::state::Inner>, g: &Gles, e: u32) -> u32 {
    drop(s);
    g.egl_fail(e)
}

// ---------- thunks ----------

fn ret_bool(c: &mut Cpu, r: Result<(), u32>) {
    c.ret_u32(r.is_ok() as u32);
}

fn read_attribs(c: &mut Cpu, addr: u32) -> Option<Vec<(u32, u32)>> {
    let mut out = Vec::new();
    if addr == 0 {
        return Some(out);
    }
    for i in 0..64u32 {
        let Ok(k) = c.mem().read_u32(addr + i * 8) else {
            c.request_stop(bhpc_cpu::Stop::Fault(bhpc_cpu::Fault { addr: addr + i * 8, size: 8, write: false }));
            return None;
        };
        if k == EGL_NONE {
            break;
        }
        let v = c.mem().read_u32(addr + i * 8 + 4).unwrap_or(0);
        out.push((k, v));
    }
    Some(out)
}

fn get_error(c: &mut Cpu, g: &Gles) {
    let e = std::mem::replace(&mut g.lock().egl.error, EGL_SUCCESS);
    c.ret_u32(e);
}

fn get_display(c: &mut Cpu, _g: &Gles) {
    c.ret_u32(EGL_DISPLAY);
}

fn initialize(c: &mut Cpu, g: &Gles) {
    args!(c; dpy, major, minor);
    if dpy != EGL_DISPLAY {
        g.egl_fail(EGL_BAD_DISPLAY);
        return c.ret_u32(0);
    }
    g.egl_initialize();
    if major != 0 && !guest_write(c, major, &1u32.to_le_bytes()) {
        return;
    }
    if minor != 0 && !guest_write(c, minor, &4u32.to_le_bytes()) {
        return;
    }
    c.ret_u32(1);
}

fn terminate(c: &mut Cpu, g: &Gles) {
    args!(c; dpy);
    if dpy != EGL_DISPLAY {
        g.egl_fail(EGL_BAD_DISPLAY);
        return c.ret_u32(0);
    }
    g.lock().egl.initialized = false;
    g.egl_ok();
    c.ret_u32(1);
}

fn query_string(c: &mut Cpu, g: &Gles) {
    args!(c; _dpy, name);
    let text: &[u8] = match name {
        EGL_VENDOR => b"bhpc",
        EGL_VERSION => b"1.4 bhpc",
        EGL_CLIENT_APIS => b"OpenGL_ES",
        EGL_EXTENSIONS => b"",
        _ => {
            g.egl_fail(EGL_BAD_PARAMETER);
            return c.ret_u32(0);
        }
    };
    let mut s = g.lock();
    let key = 0x1_0000 + name;
    let p = match s.strings.get(&key) {
        Some(p) => *p,
        None => {
            let p = s.alloc_cstr(text);
            if p != 0 {
                s.strings.insert(key, p);
            }
            p
        }
    };
    s.egl.error = EGL_SUCCESS;
    drop(s);
    c.ret_u32(p);
}

/// Write the single config handle: shared by `eglGetConfigs` and `eglChooseConfig`.
fn configs(c: &mut Cpu, g: &Gles, dpy: u32, out: u32, size: u32, num: u32, matches: bool) {
    if let Err(e) = g.egl_check_display(dpy) {
        g.egl_fail(e);
        return c.ret_u32(0);
    }
    if num == 0 {
        g.egl_fail(EGL_BAD_PARAMETER);
        return c.ret_u32(0);
    }
    let n = (matches && size >= 1 && out != 0) as u32;
    let count = if out == 0 { matches as u32 } else { n };
    if n == 1 && !guest_write(c, out, &EGL_CONFIG.to_le_bytes()) {
        return;
    }
    if !guest_write(c, num, &count.to_le_bytes()) {
        return;
    }
    g.egl_ok();
    c.ret_u32(1);
}

fn get_configs(c: &mut Cpu, g: &Gles) {
    args!(c; dpy, out, size, num);
    configs(c, g, dpy, out, size, num, true);
}

fn choose_config(c: &mut Cpu, g: &Gles) {
    args!(c; dpy, attribs, out, size, num);
    let Some(list) = read_attribs(c, attribs) else { return };
    configs(c, g, dpy, out, size, num, config_matches(&list));
}

fn get_config_attrib(c: &mut Cpu, g: &Gles) {
    args!(c; dpy, config, attr, value);
    if let Err(e) = g.egl_check_display(dpy) {
        g.egl_fail(e);
        return c.ret_u32(0);
    }
    if config != EGL_CONFIG {
        g.egl_fail(EGL_BAD_CONFIG);
        return c.ret_u32(0);
    }
    let Some(v) = config_attrib(attr) else {
        g.egl_fail(EGL_BAD_ATTRIBUTE);
        return c.ret_u32(0);
    };
    if !guest_write(c, value, &v.to_le_bytes()) {
        return;
    }
    g.egl_ok();
    c.ret_u32(1);
}

fn create_window_surface(c: &mut Cpu, g: &Gles) {
    args!(c; dpy, config, _window, _attribs);
    if let Err(e) = g.egl_check_display(dpy) {
        g.egl_fail(e);
        return c.ret_u32(0);
    }
    if config != EGL_CONFIG {
        g.egl_fail(EGL_BAD_CONFIG);
        return c.ret_u32(0);
    }
    c.ret_u32(g.egl_create_surface());
}

/// Pbuffers are not supported.
fn create_pbuffer_surface(c: &mut Cpu, g: &Gles) {
    g.egl_fail(EGL_BAD_MATCH);
    c.ret_u32(0);
}

fn destroy_surface(c: &mut Cpu, g: &Gles) {
    args!(c; dpy, surface);
    let r = g.egl_check_display(dpy).map_err(|e| g.egl_fail(e)).and_then(|_| g.egl_destroy_surface(surface));
    ret_bool(c, r);
}

fn query_surface(c: &mut Cpu, g: &Gles) {
    args!(c; dpy, surface, attr, value);
    if let Err(e) = g.egl_check_display(dpy) {
        g.egl_fail(e);
        return c.ret_u32(0);
    }
    if !g.lock().egl.surfaces.contains(&surface) {
        g.egl_fail(EGL_BAD_SURFACE);
        return c.ret_u32(0);
    }
    let (w, h) = g.surface_size();
    let v = match attr {
        EGL_WIDTH => w,
        EGL_HEIGHT => h,
        EGL_CONFIG_ID => EGL_CONFIG,
        EGL_RENDER_BUFFER => EGL_BACK_BUFFER,
        _ => {
            g.egl_fail(EGL_BAD_ATTRIBUTE);
            return c.ret_u32(0);
        }
    };
    if !guest_write(c, value, &v.to_le_bytes()) {
        return;
    }
    g.egl_ok();
    c.ret_u32(1);
}

fn bind_api(c: &mut Cpu, g: &Gles) {
    args!(c; api);
    if api != EGL_OPENGL_ES_API {
        g.egl_fail(EGL_BAD_PARAMETER);
        return c.ret_u32(0);
    }
    g.egl_ok();
    c.ret_u32(1);
}

fn query_api(c: &mut Cpu, _g: &Gles) {
    c.ret_u32(EGL_OPENGL_ES_API);
}

fn wait(c: &mut Cpu, g: &Gles) {
    g.egl_ok();
    c.ret_u32(1);
}

fn release_thread(c: &mut Cpu, g: &Gles) {
    let _ = g.egl_make_current(0, 0, 0);
    c.ret_u32(1);
}

fn create_context(c: &mut Cpu, g: &Gles) {
    args!(c; dpy, config, share, attribs);
    if let Err(e) = g.egl_check_display(dpy) {
        g.egl_fail(e);
        return c.ret_u32(0);
    }
    if config != EGL_CONFIG {
        g.egl_fail(EGL_BAD_CONFIG);
        return c.ret_u32(0);
    }
    let Some(list) = read_attribs(c, attribs) else { return };
    if list.iter().any(|&(k, v)| k != EGL_CONTEXT_CLIENT_VERSION || v != 2) {
        g.egl_fail(EGL_BAD_ATTRIBUTE);
        return c.ret_u32(0);
    }
    c.ret_u32(g.egl_create_context(share).unwrap_or(0));
}

fn destroy_context(c: &mut Cpu, g: &Gles) {
    args!(c; dpy, ctx);
    let r = g.egl_check_display(dpy).map_err(|e| g.egl_fail(e)).and_then(|_| g.egl_destroy_context(ctx));
    ret_bool(c, r);
}

fn make_current(c: &mut Cpu, g: &Gles) {
    args!(c; dpy, draw, read, ctx);
    let r = g.egl_check_display(dpy).map_err(|e| g.egl_fail(e)).and_then(|_| g.egl_make_current(draw, read, ctx));
    ret_bool(c, r);
}

fn get_current_context(c: &mut Cpu, g: &Gles) {
    c.ret_u32(g.egl_current_context());
}

fn get_current_surface(c: &mut Cpu, g: &Gles) {
    c.ret_u32(g.egl_current_surface());
}

fn get_current_display(c: &mut Cpu, g: &Gles) {
    c.ret_u32(if g.egl_current_context() != 0 { EGL_DISPLAY } else { 0 });
}

fn query_context(c: &mut Cpu, g: &Gles) {
    args!(c; dpy, ctx, attr, value);
    if let Err(e) = g.egl_check_display(dpy) {
        g.egl_fail(e);
        return c.ret_u32(0);
    }
    if !g.lock().egl.contexts.contains_key(&ctx) {
        g.egl_fail(EGL_BAD_CONTEXT);
        return c.ret_u32(0);
    }
    let v = match attr {
        EGL_CONTEXT_CLIENT_TYPE => EGL_OPENGL_ES_API,
        EGL_CONTEXT_CLIENT_VERSION => 2,
        EGL_CONFIG_ID => EGL_CONFIG,
        EGL_RENDER_BUFFER => EGL_BACK_BUFFER,
        _ => {
            g.egl_fail(EGL_BAD_ATTRIBUTE);
            return c.ret_u32(0);
        }
    };
    if !guest_write(c, value, &v.to_le_bytes()) {
        return;
    }
    g.egl_ok();
    c.ret_u32(1);
}

fn swap_buffers(c: &mut Cpu, g: &Gles) {
    args!(c; dpy, surface);
    let hook = g.lock().egl.frame_hook.clone();
    if let Some(h) = hook {
        h(c);
    }
    let r = g.egl_check_display(dpy).map_err(|e| g.egl_fail(e)).and_then(|_| g.egl_swap_buffers(surface));
    ret_bool(c, r);
}

fn swap_interval(c: &mut Cpu, g: &Gles) {
    args!(c; dpy, interval);
    if let Err(e) = g.egl_check_display(dpy) {
        g.egl_fail(e);
        return c.ret_u32(0);
    }
    let ok = g.egl_swap_interval(interval as i32);
    c.ret_u32(ok as u32);
}

fn get_proc_address(c: &mut Cpu, g: &Gles) {
    args!(c; name);
    let Some(n) = guest_cstr(c, name) else { return };
    let addr = std::str::from_utf8(&n).ok().filter(|n| crate::provides(n)).and_then(|n| crate::resolve(c, g, n));
    c.ret_u32(addr.unwrap_or(0));
}

/// EGL thunks: `(guest symbol, thunk)`.
pub const TABLE: &[(&str, Handler)] = &[
    ("eglGetError", get_error),
    ("eglGetDisplay", get_display),
    ("eglInitialize", initialize),
    ("eglTerminate", terminate),
    ("eglQueryString", query_string),
    ("eglGetConfigs", get_configs),
    ("eglChooseConfig", choose_config),
    ("eglGetConfigAttrib", get_config_attrib),
    ("eglCreateWindowSurface", create_window_surface),
    ("eglCreatePbufferSurface", create_pbuffer_surface),
    ("eglDestroySurface", destroy_surface),
    ("eglQuerySurface", query_surface),
    ("eglBindAPI", bind_api),
    ("eglQueryAPI", query_api),
    ("eglWaitClient", wait),
    ("eglWaitGL", wait),
    ("eglWaitNative", wait),
    ("eglReleaseThread", release_thread),
    ("eglCreateContext", create_context),
    ("eglDestroyContext", destroy_context),
    ("eglMakeCurrent", make_current),
    ("eglGetCurrentContext", get_current_context),
    ("eglGetCurrentSurface", get_current_surface),
    ("eglGetCurrentDisplay", get_current_display),
    ("eglQueryContext", query_context),
    ("eglSwapBuffers", swap_buffers),
    ("eglSwapInterval", swap_interval),
    ("eglGetProcAddress", get_proc_address),
];

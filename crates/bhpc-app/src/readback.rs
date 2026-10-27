//! Asynchronous pixel read-back for the in-window mode (SDL/GL glue).
//!
//! A plain `glReadPixels` into client memory makes the calling thread wait until the GPU has drawn the frame and
//! copied it out. Here the copy goes into a pixel buffer object instead (`GL_PIXEL_PACK_BUFFER`, persistently and
//! coherently mapped), the GPU signals a fence when it is done, and a thread of our own hands the mapped pixels to a
//! sink (the frame file). The game's render thread never waits: when every buffer is still in flight the frame is
//! simply not read back.
//!
//! The fence is a native one (`EGL_ANDROID_native_fence_sync`, a file descriptor the thread polls) where EGL has it;
//! elsewhere (WGL on Windows, CGL on macOS) it is a GL sync object, which only a thread with the context can query:
//! the render thread checks those without waiting (`glClientWaitSync` with a zero timeout) whenever it starts a
//! frame and passes the finished ones on.
//!
//! Everything it needs is optional in GLES2: [`Readback::start`] says [`Started::Unsupported`] when an entry point
//! is missing, and the caller keeps using the synchronous read-back.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;

const GL_VERSION: u32 = 0x1F02;
const PIXEL_PACK_BUFFER: u32 = 0x88EB;
const PIXEL_PACK_BUFFER_BINDING: u32 = 0x88ED;
const READ_FRAMEBUFFER_BINDING: u32 = 0x8CAA;
const READ_FRAMEBUFFER: u32 = 0x8CA8;
const RGBA: u32 = 0x1908;
const UNSIGNED_BYTE: u32 = 0x1401;
const MAP_READ_BIT: u32 = 0x0001;
const MAP_PERSISTENT_BIT: u32 = 0x0040;
const MAP_COHERENT_BIT: u32 = 0x0080;
const EGL_SYNC_NATIVE_FENCE_ANDROID: u32 = 0x3144;
const EGL_NO_NATIVE_FENCE_FD_ANDROID: i32 = -1;
const EGL_NONE: i32 = 0x3038;
const SYNC_GPU_COMMANDS_COMPLETE: u32 = 0x9117;
const ALREADY_SIGNALED: u32 = 0x911A;
const TIMEOUT_EXPIRED: u32 = 0x911B;
const CONDITION_SATISFIED: u32 = 0x911C;
/// Buffers in flight at most.
const BUFFERS: usize = 3;

/// Where finished frames go: width, height and the pixels (RGBA, bottom row first). Runs on the read-back thread.
pub type Sink = Arc<dyn Fn(u32, u32, &[u8]) + Send + Sync>;

/// What [`Readback::start`] did with a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Started {
    /// The copy is under way; the sink gets the pixels.
    Queued,
    /// Every buffer is still in flight: this frame is skipped.
    Busy,
    /// The driver lacks something this needs; use the synchronous read-back.
    Unsupported,
}

/// Declares a table of entry points: each field with the names it may go by (the first one the driver knows wins).
macro_rules! fns {
    ($name:ident { $($field:ident [$($n:literal),+]: fn($($t:ty),*) $(-> $r:ty)?;)* }) => {
        struct $name { $($field: unsafe extern "C" fn($($t),*) $(-> $r)?,)* }
        impl $name {
            fn resolve(get: &dyn Fn(&[&str]) -> Option<*const c_void>) -> Option<$name> {
                Some($name { $($field: {
                    let p = get(&[$($n),+])?;
                    // SAFETY: a non-null entry point returned for this name has this C signature.
                    unsafe { std::mem::transmute::<*const c_void, unsafe extern "C" fn($($t),*) $(-> $r)?>(p) }
                },)* })
            }
        }
    };
}

fns! { Fns {
    gen_buffers ["glGenBuffers"]: fn(i32, *mut u32);
    delete_buffers ["glDeleteBuffers"]: fn(i32, *const u32);
    bind_buffer ["glBindBuffer"]: fn(u32, u32);
    buffer_storage ["glBufferStorageEXT", "glBufferStorage"]: fn(u32, isize, *const c_void, u32);
    map_buffer_range ["glMapBufferRange", "glMapBufferRangeEXT"]: fn(u32, isize, isize, u32) -> *mut c_void;
    bind_framebuffer ["glBindFramebuffer"]: fn(u32, u32);
    get_integerv ["glGetIntegerv"]: fn(u32, *mut i32);
    read_pixels ["glReadPixels"]: fn(i32, i32, i32, i32, u32, u32, *mut c_void);
    flush ["glFlush"]: fn();
}}

fns! { Egl {
    get_current_display ["eglGetCurrentDisplay"]: fn() -> *mut c_void;
    create_sync ["eglCreateSyncKHR"]: fn(*mut c_void, u32, *const i32) -> *mut c_void;
    destroy_sync ["eglDestroySyncKHR"]: fn(*mut c_void, *mut c_void) -> u32;
    dup_native_fence_fd ["eglDupNativeFenceFDANDROID"]: fn(*mut c_void, *mut c_void) -> i32;
}}

fns! { GlSync {
    fence_sync ["glFenceSync"]: fn(u32, u32) -> *mut c_void;
    client_wait_sync ["glClientWaitSync"]: fn(*mut c_void, u32, u64) -> u32;
    delete_sync ["glDeleteSync"]: fn(*mut c_void);
}}

/// How the GPU says a copy is done.
enum Fences {
    /// Native fence file descriptors, polled by the read-back thread (Unix with EGL).
    #[cfg_attr(not(unix), allow(dead_code))]
    Native(Egl),
    /// GL sync objects, checked by the render thread.
    Gl(GlSync),
}

/// The entry points this needs, with the fences the driver offers.
struct Loaded {
    gl: Fns,
    fences: Fences,
}

impl Loaded {
    fn load(loader: &dyn Fn(&str) -> *const c_void) -> Option<Loaded> {
        let get = |names: &[&str]| names.iter().map(|n| loader(n)).find(|p| !p.is_null());
        let gl = Fns::load(&get)?;
        let native = if cfg!(unix) { Egl::resolve(&get).map(Fences::Native) } else { None };
        let fences = native.or_else(|| GlSync::resolve(&get).map(Fences::Gl))?;
        Some(Loaded { gl, fences })
    }
}

impl Fns {
    fn load(get: &dyn Fn(&[&str]) -> Option<*const c_void>) -> Option<Fns> {
        // Pixel pack buffers need GLES 3 (or desktop GL); a GLES 2 context would take the buffer offset as a pointer.
        let get_string = get(&["glGetString"])?;
        // SAFETY: `glGetString` has this signature; GL_VERSION gives a static NUL-terminated string or null.
        let version = unsafe {
            let f =
                std::mem::transmute::<*const c_void, unsafe extern "C" fn(u32) -> *const std::ffi::c_char>(get_string);
            let v = f(GL_VERSION);
            (!v.is_null()).then(|| std::ffi::CStr::from_ptr(v).to_string_lossy().into_owned())
        }?;
        if !has_pack_buffers(&version) {
            return None;
        }
        Fns::resolve(get)
    }
}

/// One pixel buffer: its GL name, size, mapping and whether the read-back thread still uses it.
struct Buffer {
    name: u32,
    size: (u32, u32),
    ptr: usize,
    busy: Arc<AtomicBool>,
}

/// A frame in flight, for the read-back thread.
struct Ticket {
    /// Native fence descriptor to wait for; negative when the GPU is known to be done already (GL sync path).
    fence: i32,
    ptr: usize,
    size: (u32, u32),
    busy: Arc<AtomicBool>,
    sink: Sink,
}

/// The pixel buffers and the thread that waits for them. Use it with one GL share group only.
#[derive(Default)]
pub struct Readback {
    fns: Option<Loaded>,
    /// Copies behind a GL sync object (address of the sync), oldest first, until the render thread sees them done.
    pending: VecDeque<(usize, Ticket)>,
    /// Loading failed once: do not try again.
    unsupported: bool,
    buffers: Vec<Buffer>,
    next: usize,
    thread: Option<Sender<Ticket>>,
}

impl Readback {
    pub fn new() -> Readback {
        Readback::default()
    }

    /// Whether a frame can be started now: a buffer is free (or support is still unknown, or known to be missing,
    /// which [`Readback::start`] reports). Passes copies the GPU has finished on first, so call it on the render
    /// thread with the context current.
    pub fn has_room(&mut self) -> bool {
        self.poll();
        self.fns.is_none() || self.free_buffer().is_some()
    }

    /// Hands the GL-sync copies the GPU has finished (in order) to the read-back thread, without waiting.
    fn poll(&mut self) {
        let Some(Loaded { fences: Fences::Gl(s), .. }) = self.fns.as_ref() else { return };
        while let Some(&(sync, _)) = self.pending.front() {
            // SAFETY: a sync object made by `start` in this share group and not deleted yet.
            let status = unsafe { (s.client_wait_sync)(sync as *mut c_void, 0, 0) };
            if status == TIMEOUT_EXPIRED {
                break;
            }
            // SAFETY: as above; deleted exactly once.
            unsafe { (s.delete_sync)(sync as *mut c_void) };
            let (_, ticket) = self.pending.pop_front().expect("front exists");
            if status == ALREADY_SIGNALED || status == CONDITION_SATISFIED {
                if let Some(tx) = self.thread.as_ref() {
                    let _ = tx.send(ticket);
                }
            } else {
                // GL_WAIT_FAILED: the frame is lost, the buffer free again.
                ticket.busy.store(false, Ordering::SeqCst);
            }
        }
    }

    /// Copy the `size` pixels of framebuffer `fbo` into a free buffer and have the read-back thread pass them to
    /// `sink` once the GPU is done. A GL context of the share group must be current in this thread.
    pub fn start(
        &mut self,
        loader: &dyn Fn(&str) -> *const c_void,
        fbo: u32,
        size: (u32, u32),
        sink: &Sink,
    ) -> Started {
        if self.unsupported {
            return Started::Unsupported;
        }
        if self.fns.is_none() {
            self.fns = Loaded::load(loader);
            if self.fns.is_none() {
                self.unsupported = true;
                eprintln!("[gl] asynchronous read-back unavailable: missing GL/EGL entry points");
                return Started::Unsupported;
            }
        }
        self.poll();
        let Loaded { gl: fns, fences } = self.fns.as_ref().expect("loaded above");
        let bytes = size.0 as usize * size.1 as usize * 4;
        // SAFETY: the entry points come from the driver; every name used was made here, in the current share group,
        // and the bindings this changes are put back.
        unsafe {
            let Some(i) = self.free_buffer() else { return Started::Busy };
            if self.buffers.len() == i {
                self.buffers.push(Buffer { name: 0, size: (0, 0), ptr: 0, busy: Arc::default() });
            }
            let (mut prev_buf, mut prev_fb) = (0, 0);
            (fns.get_integerv)(PIXEL_PACK_BUFFER_BINDING, &mut prev_buf);
            (fns.get_integerv)(READ_FRAMEBUFFER_BINDING, &mut prev_fb);
            let b = &mut self.buffers[i];
            if b.size != size {
                // Immutable storage cannot be resized: make a new buffer.
                (fns.delete_buffers)(1, &b.name);
                (fns.gen_buffers)(1, &mut b.name);
                (fns.bind_buffer)(PIXEL_PACK_BUFFER, b.name);
                let flags = MAP_READ_BIT | MAP_PERSISTENT_BIT | MAP_COHERENT_BIT;
                (fns.buffer_storage)(PIXEL_PACK_BUFFER, bytes as isize, std::ptr::null(), flags);
                b.ptr = (fns.map_buffer_range)(PIXEL_PACK_BUFFER, 0, bytes as isize, flags) as usize;
                b.size = if b.ptr == 0 { (0, 0) } else { size };
            }
            // A native fence descriptor, or a GL sync object (address); `None` when neither could be made.
            let fence = if b.ptr == 0 {
                None
            } else {
                (fns.bind_buffer)(PIXEL_PACK_BUFFER, b.name);
                (fns.bind_framebuffer)(READ_FRAMEBUFFER, fbo);
                (fns.read_pixels)(0, 0, size.0 as i32, size.1 as i32, RGBA, UNSIGNED_BYTE, std::ptr::null_mut());
                match fences {
                    Fences::Native(egl) => Some(native_fence(fns, egl)).filter(|fd| *fd >= 0).map(Err),
                    Fences::Gl(s) => {
                        let sync = (s.fence_sync)(SYNC_GPU_COMMANDS_COMPLETE, 0);
                        (fns.flush)();
                        (!sync.is_null()).then_some(Ok(sync as usize))
                    }
                }
            };
            (fns.bind_buffer)(PIXEL_PACK_BUFFER, prev_buf as u32);
            (fns.bind_framebuffer)(READ_FRAMEBUFFER, prev_fb as u32);
            let Some(fence) = fence else {
                self.unsupported = true;
                eprintln!("[gl] asynchronous read-back unavailable: no mapped buffer or fence");
                return Started::Unsupported;
            };
            b.busy.store(true, Ordering::SeqCst);
            let (fd, sync) = match fence {
                Err(fd) => (fd, None),
                Ok(sync) => (EGL_NO_NATIVE_FENCE_FD_ANDROID, Some(sync)),
            };
            let ticket = Ticket { fence: fd, ptr: b.ptr, size, busy: b.busy.clone(), sink: sink.clone() };
            self.next = i + 1;
            let thread = self.thread.get_or_insert_with(spawn);
            match sync {
                // Passed on by `poll` once the GPU is done.
                Some(sync) => {
                    self.pending.push_back((sync, ticket));
                    Started::Queued
                }
                None => thread.send(ticket).map_or(Started::Unsupported, |()| Started::Queued),
            }
        }
    }

    /// The next buffer the read-back thread is not using (round robin), else a new one while there are fewer than
    /// [`BUFFERS`].
    fn free_buffer(&self) -> Option<usize> {
        let n = self.buffers.len();
        (0..n)
            .map(|k| (self.next + k) % n)
            .find(|&i| !self.buffers[i].busy.load(Ordering::SeqCst))
            .or((n < BUFFERS).then_some(n))
    }
}

/// Whether a context with this `GL_VERSION` string has pixel pack buffers: GLES 3 and later, or desktop GL.
fn has_pack_buffers(version: &str) -> bool {
    match version.strip_prefix("OpenGL ES ") {
        Some(rest) => rest.chars().next().and_then(|c| c.to_digit(10)).is_some_and(|major| major >= 3),
        None => !version.is_empty(),
    }
}

/// A native fence for everything submitted so far, as a file descriptor (negative when unavailable).
unsafe fn native_fence(fns: &Fns, egl: &Egl) -> i32 {
    let dpy = (egl.get_current_display)();
    if dpy.is_null() {
        return EGL_NO_NATIVE_FENCE_FD_ANDROID;
    }
    let attribs = [EGL_NONE];
    let sync = (egl.create_sync)(dpy, EGL_SYNC_NATIVE_FENCE_ANDROID, attribs.as_ptr());
    if sync.is_null() {
        return EGL_NO_NATIVE_FENCE_FD_ANDROID;
    }
    // The fence exists once the commands before it are flushed.
    (fns.flush)();
    let fd = (egl.dup_native_fence_fd)(dpy, sync);
    (egl.destroy_sync)(dpy, sync);
    fd
}

/// The read-back thread: waits for each ticket's fence, then hands its pixels to the sink and frees the buffer.
fn spawn() -> Sender<Ticket> {
    let (tx, rx) = channel::<Ticket>();
    let _ = std::thread::Builder::new().name("gl-readback".into()).spawn(move || {
        for t in rx {
            let ready = t.fence < 0 || wait_fd(t.fence);
            if ready {
                let len = t.size.0 as usize * t.size.1 as usize * 4;
                // SAFETY: the mapping stays valid while `busy` is set (the buffer is neither deleted nor
                // recreated meanwhile), and the fence says the GPU is done writing it; coherent mapping.
                let px = unsafe { std::slice::from_raw_parts(t.ptr as *const u8, len) };
                (t.sink)(t.size.0, t.size.1, px);
            }
            t.busy.store(false, Ordering::SeqCst);
        }
    });
    tx
}

/// Waits (up to a second) for a native fence descriptor and closes it; whether it signalled.
#[cfg(unix)]
fn wait_fd(fd: i32) -> bool {
    let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    // SAFETY: a valid descriptor we own; poll only reads and writes the one struct.
    let ready = unsafe {
        loop {
            let r = libc::poll(&mut pfd, 1, 1000);
            if r >= 0 || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                break r > 0;
            }
        }
    };
    // SAFETY: the descriptor came from `eglDupNativeFenceFDANDROID` and is ours to close.
    unsafe { libc::close(fd) };
    ready
}

/// No native fences off Unix: every ticket arrives done.
#[cfg(not(unix))]
fn wait_fd(_: i32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_buffers_need_gles3_or_desktop_gl() {
        assert!(has_pack_buffers("OpenGL ES 3.2 NVIDIA 615.71.09"));
        assert!(has_pack_buffers("OpenGL ES 3.0 Mesa 26.2.3"));
        assert!(!has_pack_buffers("OpenGL ES 2.0 Mesa"));
        assert!(!has_pack_buffers("OpenGL ES x"));
        assert!(has_pack_buffers("4.6.0 NVIDIA 615.71.09"));
        assert!(!has_pack_buffers(""));
    }
}

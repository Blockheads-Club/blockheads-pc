//! Shared GL state: the host backend, guest memory helpers, bookkeeping that the marshalling needs
//! (bound buffers, pixel-store alignment, mapped buffers) and the string pool.

use crate::api::{GlApi, HostGl};
use crate::consts::*;
use crate::egl::EglState;
use crate::window::GlWindow;
use bhpc_cpu::{Cpu, Fault, Stop};
use bhpc_mem::{GuestHeap, GuestMemory, MemError};
use std::collections::{BTreeMap, HashMap};
use std::ffi::{c_void, CStr};
use std::io::Write;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Configuration of a [`Gles`] instance.
#[derive(Debug, Clone)]
pub struct GlesConfig {
    /// Guest address of the arena that holds the string pool and `glMapBufferOES` shadows (mapped by [`Gles::new`]).
    pub arena_base: u32,
    /// Size of the arena.
    pub arena_size: u32,
    /// Call `glGetError` after every guest GL call and log failures (debug aid, costs one host call per GL call).
    pub check_errors: bool,
    /// Advertise `GL_OES_vertex_array_object` (the game ships a software VAO fallback, so off by default).
    pub expose_vao: bool,
    /// Advertise `GL_OES_mapbuffer` (the game ships a software fallback, so off by default).
    pub expose_mapbuffer: bool,
}

impl GlesConfig {
    pub fn new(arena_base: u32, arena_size: u32) -> GlesConfig {
        GlesConfig { arena_base, arena_size, check_errors: false, expose_vao: false, expose_mapbuffer: false }
    }
}

/// Counters for diagnostics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    /// Guest GL/EGL calls handled.
    pub calls: u64,
    /// Calls to functions that are accepted but not implemented (ES1 fixed pipeline, vendor extensions), by name.
    pub unsupported: BTreeMap<String, u64>,
    /// Host GL errors seen after a call (only with `check_errors`): `(function, error)`.
    pub gl_errors: Vec<(String, u32)>,
}

/// A buffer currently mapped through `glMapBufferOES`.
#[derive(Debug, Clone, Copy)]
pub struct MapRec {
    pub target: u32,
    pub ptr: u32,
    pub size: u32,
}

pub(crate) struct Inner {
    pub api: Box<dyn GlApi + Send>,
    pub mem: Arc<GuestMemory>,
    pub heap: GuestHeap,
    pub cfg: GlesConfig,
    /// Error raised by the HLE layer itself, reported by the next `glGetError`.
    pub err: u32,
    pub strings: HashMap<u32, u32>,
    pub array_buffer: u32,
    pub vao: u32,
    /// Element array buffer bound per vertex array object (0 = the default one).
    pub elem: HashMap<u32, u32>,
    pub buf_size: HashMap<u32, isize>,
    /// Mapped buffers by buffer name.
    pub maps: HashMap<u32, MapRec>,
    pub unpack_align: i32,
    pub pack_align: i32,
    /// Guest pointers given to `glVertexAttribPointer`, for `glGetVertexAttribPointerv`.
    pub attrib_ptrs: [u32; 16],
    pub stats: Stats,
    pub log: Box<dyn Write + Send>,
    pub egl: EglState,
}

/// The GL/EGL HLE. Cheap to clone; all clones share the same state.
#[derive(Clone)]
pub struct Gles {
    pub(crate) inner: Arc<Mutex<Inner>>,
    pub(crate) window: Arc<dyn GlWindow>,
}

impl Gles {
    /// Build the HLE over an explicit backend (tests use a mock). Maps the arena in `mem`.
    pub fn new(
        cfg: GlesConfig,
        api: Box<dyn GlApi + Send>,
        window: Arc<dyn GlWindow>,
        mem: Arc<GuestMemory>,
    ) -> Result<Gles, MemError> {
        let heap = GuestHeap::new(mem.clone(), cfg.arena_base, cfg.arena_size)?;
        let inner = Inner {
            api,
            mem,
            heap,
            cfg,
            err: GL_NO_ERROR,
            strings: HashMap::new(),
            array_buffer: 0,
            vao: 0,
            elem: HashMap::new(),
            buf_size: HashMap::new(),
            maps: HashMap::new(),
            unpack_align: 4,
            pack_align: 4,
            attrib_ptrs: [0; 16],
            stats: Stats::default(),
            log: Box::new(std::io::stderr()),
            egl: EglState::default(),
        };
        Ok(Gles { inner: Arc::new(Mutex::new(inner)), window })
    }

    /// Build the HLE over the real host GL: function pointers come from `window.proc_address`. A GL context must be
    /// current in the calling thread for drivers that resolve entry points per context.
    pub fn with_host(cfg: GlesConfig, window: Arc<dyn GlWindow>, mem: Arc<GuestMemory>) -> Result<Gles, MemError> {
        let w = window.clone();
        let api = HostGl::load(&mut move |n| w.proc_address(n));
        Gles::new(cfg, Box::new(api), window, mem)
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Redirect diagnostics (unsupported calls, GL errors) from stderr.
    pub fn set_log(&self, log: Box<dyn Write + Send>) {
        self.lock().log = log;
    }

    /// Snapshot of the diagnostic counters.
    pub fn stats(&self) -> Stats {
        self.lock().stats.clone()
    }

    /// Size of the drawable in pixels.
    pub fn drawable_size(&self) -> (u32, u32) {
        self.window.drawable_size()
    }

    /// Size of the surface the game renders into.
    pub fn surface_size(&self) -> (u32, u32) {
        self.window.surface_size()
    }
}

/// Keep extensions that the HLE layer can honour and that the game understands. Vendor extensions
/// (QCOM, AMD, NV, IMG, APPLE, ...) and compressed formats we cannot upload are dropped.
pub fn filter_extensions(host: &str, expose_vao: bool, expose_mapbuffer: bool) -> String {
    const ALLOWED: &[&str] = &[
        "GL_OES_blend_equation_separate",
        "GL_OES_blend_func_separate",
        "GL_OES_blend_subtract",
        "GL_OES_depth24",
        "GL_OES_depth_texture",
        "GL_OES_element_index_uint",
        "GL_OES_packed_depth_stencil",
        "GL_OES_rgb8_rgba8",
        "GL_OES_standard_derivatives",
        "GL_OES_texture_float",
        "GL_OES_texture_half_float",
        "GL_OES_texture_npot",
        "GL_OES_vertex_half_float",
        "GL_EXT_blend_minmax",
        "GL_EXT_texture_filter_anisotropic",
        "GL_EXT_texture_format_BGRA8888",
    ];
    let mut out: Vec<&str> = host.split_whitespace().filter(|e| ALLOWED.contains(e)).collect();
    if expose_vao
        && host.split_whitespace().any(|e| e == "GL_OES_vertex_array_object" || e == "GL_ARB_vertex_array_object")
    {
        out.push("GL_OES_vertex_array_object");
    }
    if expose_mapbuffer {
        out.push("GL_OES_mapbuffer");
    }
    out.join(" ")
}

/// The `GL_VERSION` the game sees: always ES 2.0, keeping the driver's suffix.
pub fn es2_version(host: &str) -> String {
    let rest = host.strip_prefix("OpenGL ES ").map(|r| r.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.'));
    match rest {
        Some(r) if !r.trim().is_empty() => format!("OpenGL ES 2.0 {}", r.trim()),
        _ => "OpenGL ES 2.0".to_string(),
    }
}

impl Inner {
    /// Raise a GL error of the HLE layer (the first one wins until `glGetError` reads it).
    pub fn raise(&mut self, e: u32) {
        if self.err == GL_NO_ERROR {
            self.err = e;
        }
    }

    pub fn element_buffer(&self) -> u32 {
        self.elem.get(&self.vao).copied().unwrap_or(0)
    }

    /// Bound buffer for `target` (0 for unknown targets).
    pub fn bound_buffer(&self, target: u32) -> u32 {
        match target {
            GL_ARRAY_BUFFER => self.array_buffer,
            GL_ELEMENT_ARRAY_BUFFER => self.element_buffer(),
            _ => 0,
        }
    }

    /// Forget a deleted buffer.
    pub fn forget_buffer(&mut self, id: u32) {
        if self.array_buffer == id {
            self.array_buffer = 0;
        }
        self.elem.retain(|_, b| *b != id);
        self.buf_size.remove(&id);
        if let Some(m) = self.maps.remove(&id) {
            self.heap.free(m.ptr);
        }
    }

    /// The guest string for `GL_VENDOR` & co (allocated once, then cached).
    pub fn gl_string(&mut self, name: u32) -> u32 {
        if let Some(p) = self.strings.get(&name) {
            return *p;
        }
        let host = self.api.glGetString(name);
        let host = if host.is_null() {
            None
        } else {
            // SAFETY: GL returns NUL-terminated static strings.
            Some(unsafe { CStr::from_ptr(host.cast()) }.to_string_lossy().into_owned())
        };
        let text = match (name, host) {
            (GL_SHADING_LANGUAGE_VERSION, _) => Some("OpenGL ES GLSL ES 1.00".to_string()),
            (GL_VERSION, Some(h)) => Some(es2_version(&h)),
            (GL_EXTENSIONS, Some(h)) => Some(filter_extensions(&h, self.cfg.expose_vao, self.cfg.expose_mapbuffer)),
            (GL_VENDOR | GL_RENDERER, Some(h)) => Some(h),
            _ => None,
        };
        let Some(text) = text else {
            self.raise(GL_INVALID_ENUM);
            return 0;
        };
        let p = self.alloc_cstr(text.as_bytes());
        if p != 0 {
            self.strings.insert(name, p);
        }
        p
    }

    pub(crate) fn alloc_cstr(&mut self, s: &[u8]) -> u32 {
        let p = self.heap.alloc(s.len() as u32 + 1);
        if p == 0 {
            self.raise(GL_OUT_OF_MEMORY);
            return 0;
        }
        let mut v = s.to_vec();
        v.push(0);
        // The block was just allocated inside the mapped arena.
        let _ = self.mem.write_bytes(p, &v);
        p
    }

    /// Count a call to an accepted-but-unimplemented function and log the first one.
    pub fn note_unsupported(&mut self, name: &str) {
        let n = self.stats.unsupported.entry(name.to_string()).or_insert(0);
        *n += 1;
        if *n == 1 {
            let _ = writeln!(self.log, "bhpc-gles: unsupported call {name} (ignored)");
        }
    }

    /// Post-call hook: count the call and, in debug mode, drain the host error queue.
    pub fn after_call(&mut self, name: &str) {
        self.stats.calls += 1;
        if !self.cfg.check_errors || name == "glGetError" {
            return;
        }
        for _ in 0..8 {
            let e = self.api.glGetError();
            if e == GL_NO_ERROR {
                break;
            }
            let _ = writeln!(self.log, "bhpc-gles: GL error {e:#x} after {name}");
            self.stats.gl_errors.push((name.to_string(), e));
        }
    }
}

/// A validated host pointer to `len` bytes of guest memory at `addr`. A null `addr` gives a null pointer. An
/// unmapped range stops the guest with a fault (what a real device would do) and gives `None`.
pub(crate) fn host_ptr(c: &mut Cpu, addr: u32, len: u32, write: bool) -> Option<*mut c_void> {
    if addr == 0 {
        return Some(std::ptr::null_mut());
    }
    let mem = c.mem().clone();
    if !mem.is_mapped(addr, len.max(1)) {
        c.request_stop(Stop::Fault(Fault { addr, size: len, write }));
        return None;
    }
    // SAFETY: `addr` is inside the reserved window (it is mapped).
    Some(unsafe { mem.base().add(addr as usize) }.cast())
}

/// Read a NUL-terminated guest string (a fault stops the guest).
pub(crate) fn guest_cstr(c: &mut Cpu, addr: u32) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut a = addr;
    loop {
        let chunk = 4096 - (a & 4095);
        let Ok(b) = c.mem().read_bytes(a, chunk) else {
            c.request_stop(Stop::Fault(Fault { addr: a, size: 1, write: false }));
            return None;
        };
        if let Some(i) = b.iter().position(|x| *x == 0) {
            out.extend_from_slice(&b[..i]);
            return Some(out);
        }
        out.extend_from_slice(&b);
        a = a.wrapping_add(chunk);
    }
}

/// Write bytes to guest memory (a fault stops the guest).
pub(crate) fn guest_write(c: &mut Cpu, addr: u32, data: &[u8]) -> bool {
    if data.is_empty() {
        return true;
    }
    if c.mem().write_bytes(addr, data).is_err() {
        c.request_stop(Stop::Fault(Fault { addr, size: data.len() as u32, write: true }));
        return false;
    }
    true
}

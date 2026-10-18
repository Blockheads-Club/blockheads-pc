//! The game's offscreen surface (SDL/GLES glue; see [`crate::surface`] for the geometry).
//!
//! The game renders into a framebuffer object of the size it announced and the window presents that, scaled and
//! letterboxed, right before the buffer swap. Texture and renderbuffer storage and the compositing program are shared
//! between the contexts of the window (they share objects); framebuffer objects are not, so each context gets its own
//! that points at the shared storage.

use crate::surface::{fit, pack, unpack};
use bhpc_gles::Overlay;
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CString};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

const FRAMEBUFFER: u32 = 0x8D40;
const RENDERBUFFER: u32 = 0x8D41;
const COLOR_ATTACHMENT0: u32 = 0x8CE0;
const DEPTH_ATTACHMENT: u32 = 0x8D00;
const STENCIL_ATTACHMENT: u32 = 0x8D20;
const DEPTH24_STENCIL8: u32 = 0x88F0;
const DEPTH_COMPONENT16: u32 = 0x81A5;
const STENCIL_INDEX8: u32 = 0x8D48;
const FRAMEBUFFER_COMPLETE: u32 = 0x8CD5;
const TEXTURE_2D: u32 = 0x0DE1;
const RGBA: u32 = 0x1908;
const UNSIGNED_BYTE: u32 = 0x1401;
const TEXTURE_MIN_FILTER: u32 = 0x2801;
const TEXTURE_MAG_FILTER: u32 = 0x2800;
const TEXTURE_WRAP_S: u32 = 0x2802;
const TEXTURE_WRAP_T: u32 = 0x2803;
const LINEAR: i32 = 0x2601;
const CLAMP_TO_EDGE: i32 = 0x812F;
const ARRAY_BUFFER: u32 = 0x8892;
const STATIC_DRAW: u32 = 0x88E4;
const FLOAT: u32 = 0x1406;
const TRIANGLE_STRIP: u32 = 5;
const COLOR_BUFFER_BIT: u32 = 0x4000;
const VERTEX_SHADER: u32 = 0x8B31;
const FRAGMENT_SHADER: u32 = 0x8B30;
const COMPILE_STATUS: u32 = 0x8B81;
const LINK_STATUS: u32 = 0x8B82;
const TEXTURE0: u32 = 0x84C0;
const ACTIVE_TEXTURE: u32 = 0x84E0;
const TEXTURE_BINDING_2D: u32 = 0x8069;
const FRAMEBUFFER_BINDING: u32 = 0x8CA6;
const CURRENT_PROGRAM: u32 = 0x8B8D;
const ARRAY_BUFFER_BINDING: u32 = 0x8894;
const VIEWPORT: u32 = 0x0BA2;
const COLOR_WRITEMASK: u32 = 0x0C23;
const COLOR_CLEAR_VALUE: u32 = 0x0C22;
const VERTEX_ATTRIB_ARRAY_ENABLED: u32 = 0x8622;
const VERTEX_ATTRIB_ARRAY_SIZE: u32 = 0x8623;
const VERTEX_ATTRIB_ARRAY_STRIDE: u32 = 0x8624;
const VERTEX_ATTRIB_ARRAY_TYPE: u32 = 0x8625;
const VERTEX_ATTRIB_ARRAY_NORMALIZED: u32 = 0x886A;
const VERTEX_ATTRIB_ARRAY_BUFFER_BINDING: u32 = 0x889F;
const VERTEX_ATTRIB_ARRAY_POINTER: u32 = 0x8645;
/// Capabilities that would disturb a plain textured quad: scissor, stencil, depth, blend, cull, dither.
const BLEND: u32 = 0x0BE2;
const BLEND_SRC_RGB: u32 = 0x80C9;
const BLEND_DST_RGB: u32 = 0x80C8;
const BLEND_SRC_ALPHA: u32 = 0x80CB;
const BLEND_DST_ALPHA: u32 = 0x80CA;
const ONE: u32 = 1;
const ONE_MINUS_SRC_ALPHA: u32 = 0x0303;
const CAPS: [u32; 6] = [0x0C11, 0x0B90, 0x0B71, 0x0BE2, 0x0B44, 0x0BD0];

const VS: &str =
    "attribute vec2 pos; attribute vec2 uv; varying vec2 v; void main() { v = uv; gl_Position = vec4(pos, 0.0, 1.0); }";
const FS: &str = "precision mediump float; varying vec2 v; uniform sampler2D t; void main() { gl_FragColor = vec4(texture2D(t, v).rgb, 1.0); }";
/// The overlay is premultiplied RGBA and is blended with `ONE, ONE_MINUS_SRC_ALPHA`.
const FS_OVERLAY: &str =
    "precision mediump float; varying vec2 v; uniform sampler2D t; void main() { gl_FragColor = texture2D(t, v); }";
/// Full-screen strip: position then texture coordinate.
const QUAD: [f32; 16] = [-1.0, -1.0, 0.0, 0.0, 1.0, -1.0, 1.0, 0.0, -1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0];

macro_rules! gl_table {
    ($($name:ident: fn($($t:ty),*) $(-> $r:ty)?;)*) => {
        #[allow(non_snake_case)]
        struct Gl { $($name: unsafe extern "C" fn($($t),*) $(-> $r)?,)* }
        impl Gl {
            fn load(loader: &dyn Fn(&str) -> *const c_void) -> Option<Gl> {
                Some(Gl { $($name: {
                    let p = loader(stringify!($name));
                    if p.is_null() { return None; }
                    // SAFETY: a non-null entry point returned for a GL name has that function's C signature.
                    unsafe { std::mem::transmute::<*const c_void, unsafe extern "C" fn($($t),*) $(-> $r)?>(p) }
                },)* })
            }
        }
    };
}

gl_table! {
    glGenFramebuffers: fn(i32, *mut u32);
    glBindFramebuffer: fn(u32, u32);
    glFramebufferTexture2D: fn(u32, u32, u32, u32, i32);
    glFramebufferRenderbuffer: fn(u32, u32, u32, u32);
    glCheckFramebufferStatus: fn(u32) -> u32;
    glGenRenderbuffers: fn(i32, *mut u32);
    glBindRenderbuffer: fn(u32, u32);
    glRenderbufferStorage: fn(u32, u32, i32, i32);
    glDeleteRenderbuffers: fn(i32, *const u32);
    glGenTextures: fn(i32, *mut u32);
    glBindTexture: fn(u32, u32);
    glDeleteTextures: fn(i32, *const u32);
    glTexImage2D: fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void);
    glTexParameteri: fn(u32, u32, i32);
    glActiveTexture: fn(u32);
    glCreateShader: fn(u32) -> u32;
    glShaderSource: fn(u32, i32, *const *const c_char, *const i32);
    glCompileShader: fn(u32);
    glGetShaderiv: fn(u32, u32, *mut i32);
    glDeleteShader: fn(u32);
    glCreateProgram: fn() -> u32;
    glAttachShader: fn(u32, u32);
    glBindAttribLocation: fn(u32, u32, *const c_char);
    glLinkProgram: fn(u32);
    glGetProgramiv: fn(u32, u32, *mut i32);
    glUseProgram: fn(u32);
    glGetUniformLocation: fn(u32, *const c_char) -> i32;
    glUniform1i: fn(i32, i32);
    glGenBuffers: fn(i32, *mut u32);
    glBindBuffer: fn(u32, u32);
    glBufferData: fn(u32, isize, *const c_void, u32);
    glGetIntegerv: fn(u32, *mut i32);
    glGetBooleanv: fn(u32, *mut u8);
    glGetFloatv: fn(u32, *mut f32);
    glIsEnabled: fn(u32) -> u8;
    glEnable: fn(u32);
    glDisable: fn(u32);
    glViewport: fn(i32, i32, i32, i32);
    glColorMask: fn(u8, u8, u8, u8);
    glBlendFuncSeparate: fn(u32, u32, u32, u32);
    glTexSubImage2D: fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void);
    glClearColor: fn(f32, f32, f32, f32);
    glClear: fn(u32);
    glDrawArrays: fn(u32, i32, i32);
    glVertexAttribPointer: fn(u32, i32, u32, u8, i32, *const c_void);
    glEnableVertexAttribArray: fn(u32);
    glDisableVertexAttribArray: fn(u32);
    glGetVertexAttribiv: fn(u32, u32, *mut i32);
    glGetVertexAttribPointerv: fn(u32, u32, *mut *mut c_void);
}

/// Storage shared by all contexts.
#[derive(Default)]
struct Shared {
    /// Size of the allocated texture and renderbuffers.
    size: (u32, u32),
    /// Bumped whenever the storage was recreated; framebuffers with an older number are re-attached.
    generation: u64,
    tex: u32,
    /// Depth+stencil renderbuffer, or `[depth, stencil]` when packed storage is unavailable (`split`).
    rbs: Vec<u32>,
    split: bool,
    program: u32,
    vbo: u32,
    sampler: i32,
    overlay_program: u32,
    overlay_sampler: i32,
    /// The overlay texture, its size and the serial of the pixels it holds.
    overlay_tex: u32,
    overlay_size: (u32, u32),
    overlay_serial: Option<u64>,
}

#[derive(Default)]
struct State {
    shared: Shared,
    /// Framebuffer object and attached generation per host context.
    fbos: HashMap<usize, (u32, u64)>,
    /// Giving up (no GL support or an incomplete framebuffer): the window is the target.
    broken: bool,
}

/// The offscreen target of the game and its presentation.
pub struct VirtualSurface {
    /// Announced size, see [`pack`].
    want: AtomicU64,
    gl: Mutex<Option<Gl>>,
    state: Mutex<State>,
    overlay: Mutex<Option<Overlay>>,
}

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

impl VirtualSurface {
    pub fn new() -> VirtualSurface {
        VirtualSurface {
            want: AtomicU64::new(0),
            gl: Mutex::new(None),
            state: Mutex::new(State::default()),
            overlay: Mutex::new(None),
        }
    }

    pub fn set_size(&self, w: u32, h: u32) {
        self.want.store(pack(w, h), Ordering::SeqCst);
    }

    /// Set (or clear) the picture [`VirtualSurface::compose`] blends over the whole drawable.
    pub fn set_overlay(&self, overlay: Option<Overlay>) {
        *self.overlay.lock().unwrap_or_else(PoisonError::into_inner) = overlay;
    }

    pub fn size(&self) -> Option<(u32, u32)> {
        unpack(self.want.load(Ordering::SeqCst))
    }

    /// Run `f` with the GL entry points (resolved on first use with a context current).
    fn with_gl<T>(&self, loader: &dyn Fn(&str) -> *const c_void, f: impl FnOnce(&Gl, &mut State) -> T) -> Option<T> {
        let mut gl = self.gl.lock().unwrap_or_else(PoisonError::into_inner);
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if st.broken {
            return None;
        }
        if gl.is_none() {
            *gl = Gl::load(loader);
        }
        match gl.as_ref() {
            Some(g) => Some(f(g, &mut st)),
            None => {
                st.broken = true;
                eprintln!("[gl] offscreen surface unavailable: missing GL entry points");
                None
            }
        }
    }

    /// The framebuffer standing for the game's default framebuffer in context `ctx` (0: render into the window).
    pub fn framebuffer(&self, ctx: usize, loader: &dyn Fn(&str) -> *const c_void) -> u32 {
        let Some(size) = self.size() else { return 0 };
        self.with_gl(loader, |g, st| {
            // SAFETY: a context is current in this thread; all objects are created and used in its share group.
            unsafe { ensure(g, st, ctx, size) }
        })
        .unwrap_or(0)
    }

    /// Draw the surface into the window's back buffer (`drawable` pixels), letterboxed. A no-op until the game bound
    /// the surface in this context.
    pub fn compose(&self, ctx: usize, drawable: (u32, u32), loader: &dyn Fn(&str) -> *const c_void) {
        let Some(size) = self.size() else { return };
        let overlay = self.overlay.lock().unwrap_or_else(PoisonError::into_inner).clone();
        self.with_gl(loader, |g, st| {
            if !st.fbos.contains_key(&ctx) {
                return;
            }
            // SAFETY: as in `framebuffer`; the original GL state is restored before returning.
            unsafe {
                ensure(g, st, ctx, size);
                draw(g, &mut st.shared, fit(size, drawable), overlay.as_ref().map(|o| (o, drawable)));
            }
        });
    }
}

impl Default for VirtualSurface {
    fn default() -> Self {
        VirtualSurface::new()
    }
}

/// Make sure the storage has `size` and context `ctx` has a framebuffer attached to it; returns that framebuffer
/// (0 when the setup failed). Leaves the framebuffer binding as it was.
unsafe fn ensure(g: &Gl, st: &mut State, ctx: usize, size: (u32, u32)) -> u32 {
    let mut prev = [0i32; 1];
    (g.glGetIntegerv)(FRAMEBUFFER_BINDING, prev.as_mut_ptr());
    if st.shared.program == 0 {
        match build_program(g, &mut st.shared) {
            Some(()) => {}
            None => {
                st.broken = true;
                eprintln!("[gl] offscreen surface unavailable: cannot build the compositing program");
                return 0;
            }
        }
    }
    if st.shared.size != size {
        realloc(g, &mut st.shared, size);
    }
    let entry = st.fbos.entry(ctx).or_insert_with(|| {
        let mut f = 0u32;
        (g.glGenFramebuffers)(1, &mut f);
        (f, 0)
    });
    let fbo = entry.0;
    if entry.1 != st.shared.generation {
        let mut complete = attach(g, &st.shared, fbo, prev[0] as u32);
        if !complete && !st.shared.split {
            // No packed depth+stencil storage: retry with separate renderbuffers.
            st.shared.split = true;
            realloc(g, &mut st.shared, size);
            complete = attach(g, &st.shared, fbo, prev[0] as u32);
        }
        if !complete {
            st.broken = true;
            eprintln!("[gl] offscreen surface unavailable: the framebuffer is incomplete");
            return 0;
        }
        entry.1 = st.shared.generation;
        eprintln!("[gl] offscreen surface {}x{} for context {ctx:#x}", size.0, size.1);
    }
    fbo
}

/// Attach the shared storage to `fbo` and report whether the result is complete; binds `restore` afterwards.
unsafe fn attach(g: &Gl, sh: &Shared, fbo: u32, restore: u32) -> bool {
    (g.glBindFramebuffer)(FRAMEBUFFER, fbo);
    (g.glFramebufferTexture2D)(FRAMEBUFFER, COLOR_ATTACHMENT0, TEXTURE_2D, sh.tex, 0);
    match sh.rbs[..] {
        [packed] => {
            (g.glFramebufferRenderbuffer)(FRAMEBUFFER, DEPTH_ATTACHMENT, RENDERBUFFER, packed);
            (g.glFramebufferRenderbuffer)(FRAMEBUFFER, STENCIL_ATTACHMENT, RENDERBUFFER, packed);
        }
        [depth, stencil] => {
            (g.glFramebufferRenderbuffer)(FRAMEBUFFER, DEPTH_ATTACHMENT, RENDERBUFFER, depth);
            (g.glFramebufferRenderbuffer)(FRAMEBUFFER, STENCIL_ATTACHMENT, RENDERBUFFER, stencil);
        }
        _ => {}
    }
    let status = (g.glCheckFramebufferStatus)(FRAMEBUFFER);
    (g.glBindFramebuffer)(FRAMEBUFFER, restore);
    status == FRAMEBUFFER_COMPLETE
}

unsafe fn realloc(g: &Gl, sh: &mut Shared, size: (u32, u32)) {
    if sh.tex != 0 {
        (g.glDeleteTextures)(1, &sh.tex);
        (g.glDeleteRenderbuffers)(sh.rbs.len() as i32, sh.rbs.as_ptr());
    }
    let (w, h) = (size.0 as i32, size.1 as i32);
    let mut prev_tex = 0;
    (g.glGetIntegerv)(TEXTURE_BINDING_2D, &mut prev_tex);
    (g.glGenTextures)(1, &mut sh.tex);
    (g.glBindTexture)(TEXTURE_2D, sh.tex);
    (g.glTexImage2D)(TEXTURE_2D, 0, RGBA as i32, w, h, 0, RGBA, UNSIGNED_BYTE, std::ptr::null());
    for (p, v) in [
        (TEXTURE_MIN_FILTER, LINEAR),
        (TEXTURE_MAG_FILTER, LINEAR),
        (TEXTURE_WRAP_S, CLAMP_TO_EDGE),
        (TEXTURE_WRAP_T, CLAMP_TO_EDGE),
    ] {
        (g.glTexParameteri)(TEXTURE_2D, p, v);
    }
    (g.glBindTexture)(TEXTURE_2D, prev_tex as u32);
    // Packed depth+stencil first; separate renderbuffers when the driver cannot do that (`split`).
    let mut prev_rb = 0;
    (g.glGetIntegerv)(0x8CA7, &mut prev_rb);
    let make = |format: u32| {
        let mut r = 0u32;
        (g.glGenRenderbuffers)(1, &mut r);
        (g.glBindRenderbuffer)(RENDERBUFFER, r);
        (g.glRenderbufferStorage)(RENDERBUFFER, format, w, h);
        r
    };
    sh.rbs = if sh.split { vec![make(DEPTH_COMPONENT16), make(STENCIL_INDEX8)] } else { vec![make(DEPTH24_STENCIL8)] };
    (g.glBindRenderbuffer)(RENDERBUFFER, prev_rb as u32);
    sh.size = size;
    sh.generation += 1;
}

/// Compile and link the shared vertex shader with `fs`; returns the program and its sampler uniform.
unsafe fn link(g: &Gl, fs: &str) -> Option<(u32, i32)> {
    let compile = |kind: u32, src: &str| -> Option<u32> {
        let s = (g.glCreateShader)(kind);
        let c = cstr(src);
        let p = c.as_ptr();
        (g.glShaderSource)(s, 1, &p, std::ptr::null());
        (g.glCompileShader)(s);
        let mut ok = 0;
        (g.glGetShaderiv)(s, COMPILE_STATUS, &mut ok);
        (ok != 0).then_some(s)
    };
    let (vs, fs) = (compile(VERTEX_SHADER, VS)?, compile(FRAGMENT_SHADER, fs)?);
    let p = (g.glCreateProgram)();
    (g.glAttachShader)(p, vs);
    (g.glAttachShader)(p, fs);
    (g.glBindAttribLocation)(p, 0, cstr("pos").as_ptr());
    (g.glBindAttribLocation)(p, 1, cstr("uv").as_ptr());
    (g.glLinkProgram)(p);
    (g.glDeleteShader)(vs);
    (g.glDeleteShader)(fs);
    let mut ok = 0;
    (g.glGetProgramiv)(p, LINK_STATUS, &mut ok);
    (ok != 0).then(|| (p, (g.glGetUniformLocation)(p, cstr("t").as_ptr())))
}

/// The compositing programs (surface and overlay) and the quad they draw.
unsafe fn build_program(g: &Gl, sh: &mut Shared) -> Option<()> {
    let (p, sampler) = link(g, FS)?;
    let (op, osampler) = link(g, FS_OVERLAY)?;
    let mut vbo = 0u32;
    let mut prev = 0;
    (g.glGetIntegerv)(ARRAY_BUFFER_BINDING, &mut prev);
    (g.glGenBuffers)(1, &mut vbo);
    (g.glBindBuffer)(ARRAY_BUFFER, vbo);
    (g.glBufferData)(ARRAY_BUFFER, std::mem::size_of_val(&QUAD) as isize, QUAD.as_ptr().cast(), STATIC_DRAW);
    (g.glBindBuffer)(ARRAY_BUFFER, prev as u32);
    (sh.program, sh.sampler, sh.overlay_program, sh.overlay_sampler, sh.vbo) = (p, sampler, op, osampler, vbo);
    Some(())
}

#[derive(Default, Clone, Copy)]
struct Attrib {
    enabled: i32,
    size: i32,
    kind: i32,
    normalized: i32,
    stride: i32,
    buffer: i32,
    pointer: usize,
}

/// Draw the surface texture into framebuffer 0 inside `vp`, restoring every piece of state it touches.
unsafe fn draw(g: &Gl, sh: &mut Shared, vp: crate::surface::Viewport, overlay: Option<(&Overlay, (u32, u32))>) {
    let (mut fb, mut prog, mut buf, mut tex, mut unit) = (0, 0, 0, 0, 0);
    let (mut view, mut mask, mut clear) = ([0i32; 4], [0u8; 4], [0f32; 4]);
    (g.glGetIntegerv)(FRAMEBUFFER_BINDING, &mut fb);
    (g.glGetIntegerv)(CURRENT_PROGRAM, &mut prog);
    (g.glGetIntegerv)(ARRAY_BUFFER_BINDING, &mut buf);
    (g.glGetIntegerv)(ACTIVE_TEXTURE, &mut unit);
    (g.glGetIntegerv)(VIEWPORT, view.as_mut_ptr());
    (g.glGetBooleanv)(COLOR_WRITEMASK, mask.as_mut_ptr());
    (g.glGetFloatv)(COLOR_CLEAR_VALUE, clear.as_mut_ptr());
    let caps: Vec<bool> = CAPS.iter().map(|c| (g.glIsEnabled)(*c) != 0).collect();
    let mut attribs = [Attrib::default(); 2];
    for (i, a) in attribs.iter_mut().enumerate() {
        let i = i as u32;
        (g.glGetVertexAttribiv)(i, VERTEX_ATTRIB_ARRAY_ENABLED, &mut a.enabled);
        (g.glGetVertexAttribiv)(i, VERTEX_ATTRIB_ARRAY_SIZE, &mut a.size);
        (g.glGetVertexAttribiv)(i, VERTEX_ATTRIB_ARRAY_TYPE, &mut a.kind);
        (g.glGetVertexAttribiv)(i, VERTEX_ATTRIB_ARRAY_NORMALIZED, &mut a.normalized);
        (g.glGetVertexAttribiv)(i, VERTEX_ATTRIB_ARRAY_STRIDE, &mut a.stride);
        (g.glGetVertexAttribiv)(i, VERTEX_ATTRIB_ARRAY_BUFFER_BINDING, &mut a.buffer);
        let mut p: *mut c_void = std::ptr::null_mut();
        (g.glGetVertexAttribPointerv)(i, VERTEX_ATTRIB_ARRAY_POINTER, &mut p);
        a.pointer = p as usize;
    }
    (g.glActiveTexture)(TEXTURE0);
    (g.glGetIntegerv)(TEXTURE_BINDING_2D, &mut tex);

    (g.glBindFramebuffer)(FRAMEBUFFER, 0);
    for c in CAPS {
        (g.glDisable)(c);
    }
    (g.glColorMask)(1, 1, 1, 1);
    (g.glClearColor)(0.0, 0.0, 0.0, 1.0);
    (g.glClear)(COLOR_BUFFER_BIT);
    (g.glViewport)(vp.x, vp.y, vp.w, vp.h);
    (g.glUseProgram)(sh.program);
    (g.glBindTexture)(TEXTURE_2D, sh.tex);
    (g.glUniform1i)(sh.sampler, 0);
    (g.glBindBuffer)(ARRAY_BUFFER, sh.vbo);
    let stride = 16;
    (g.glVertexAttribPointer)(0, 2, FLOAT, 0, stride, std::ptr::null());
    (g.glVertexAttribPointer)(1, 2, FLOAT, 0, stride, 8usize as *const c_void);
    (g.glEnableVertexAttribArray)(0);
    (g.glEnableVertexAttribArray)(1);
    (g.glDrawArrays)(TRIANGLE_STRIP, 0, 4);
    if let Some((ov, drawable)) = overlay {
        draw_overlay(g, sh, ov, drawable);
    }

    // Put everything back.
    for (i, a) in attribs.iter().enumerate() {
        (g.glBindBuffer)(ARRAY_BUFFER, a.buffer as u32);
        if a.size != 0 {
            (g.glVertexAttribPointer)(
                i as u32,
                a.size,
                a.kind as u32,
                a.normalized as u8,
                a.stride,
                a.pointer as *const c_void,
            );
        }
        if a.enabled != 0 {
            (g.glEnableVertexAttribArray)(i as u32);
        } else {
            (g.glDisableVertexAttribArray)(i as u32);
        }
    }
    (g.glBindBuffer)(ARRAY_BUFFER, buf as u32);
    (g.glBindTexture)(TEXTURE_2D, tex as u32);
    (g.glActiveTexture)(unit as u32);
    (g.glUseProgram)(prog as u32);
    for (c, on) in CAPS.iter().zip(caps) {
        if on {
            (g.glEnable)(*c);
        }
    }
    (g.glColorMask)(mask[0], mask[1], mask[2], mask[3]);
    (g.glClearColor)(clear[0], clear[1], clear[2], clear[3]);
    (g.glViewport)(view[0], view[1], view[2], view[3]);
    (g.glBindFramebuffer)(FRAMEBUFFER, fb as u32);
}

/// Blend the overlay picture over the whole drawable. Runs inside [`draw`], with the quad's buffer and attributes set
/// up and framebuffer 0 bound; changes the program, texture, viewport, blend state (restored by the caller, except the
/// blend function which is restored here).
unsafe fn draw_overlay(g: &Gl, sh: &mut Shared, ov: &Overlay, drawable: (u32, u32)) {
    let (w, h) = (ov.width as usize, ov.height as usize);
    if w == 0 || h == 0 || ov.rgba.len() != w * h * 4 || sh.overlay_program == 0 {
        return;
    }
    if sh.overlay_tex == 0 {
        (g.glGenTextures)(1, &mut sh.overlay_tex);
    }
    (g.glBindTexture)(TEXTURE_2D, sh.overlay_tex);
    if sh.overlay_serial != Some(ov.serial) || sh.overlay_size != (ov.width, ov.height) {
        // Rows go bottom first in GL.
        let mut flipped = Vec::with_capacity(ov.rgba.len());
        for row in ov.rgba.chunks_exact(w * 4).rev() {
            flipped.extend_from_slice(row);
        }
        if sh.overlay_size != (ov.width, ov.height) || sh.overlay_serial.is_none() {
            (g.glTexImage2D)(
                TEXTURE_2D,
                0,
                RGBA as i32,
                w as i32,
                h as i32,
                0,
                RGBA,
                UNSIGNED_BYTE,
                flipped.as_ptr().cast(),
            );
            for (p, v) in [
                (TEXTURE_MIN_FILTER, LINEAR),
                (TEXTURE_MAG_FILTER, LINEAR),
                (TEXTURE_WRAP_S, CLAMP_TO_EDGE),
                (TEXTURE_WRAP_T, CLAMP_TO_EDGE),
            ] {
                (g.glTexParameteri)(TEXTURE_2D, p, v);
            }
        } else {
            (g.glTexSubImage2D)(TEXTURE_2D, 0, 0, 0, w as i32, h as i32, RGBA, UNSIGNED_BYTE, flipped.as_ptr().cast());
        }
        sh.overlay_serial = Some(ov.serial);
        sh.overlay_size = (ov.width, ov.height);
    }
    let mut blend = [0i32; 4];
    for (i, p) in [BLEND_SRC_RGB, BLEND_DST_RGB, BLEND_SRC_ALPHA, BLEND_DST_ALPHA].into_iter().enumerate() {
        (g.glGetIntegerv)(p, &mut blend[i]);
    }
    (g.glViewport)(0, 0, drawable.0 as i32, drawable.1 as i32);
    (g.glUseProgram)(sh.overlay_program);
    (g.glUniform1i)(sh.overlay_sampler, 0);
    (g.glEnable)(BLEND);
    (g.glBlendFuncSeparate)(ONE, ONE_MINUS_SRC_ALPHA, ONE, ONE_MINUS_SRC_ALPHA);
    (g.glDrawArrays)(TRIANGLE_STRIP, 0, 4);
    (g.glDisable)(BLEND);
    (g.glBlendFuncSeparate)(blend[0] as u32, blend[1] as u32, blend[2] as u32, blend[3] as u32);
}

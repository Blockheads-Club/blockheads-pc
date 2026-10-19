//! The OpenGL painter of the in-window game view (Linux): draws the newest frame of the frame file as a texture in a
//! `GtkGLArea`, so the picture goes to the screen through the GPU instead of cairo.
//!
//! The frame's pixels are uploaded straight from the shared mapping of the frame file (`FrameReader::read_with`):
//! one copy, done by the driver. A view that draws this way asks the runner for frames in the layout `glReadPixels`
//! gives (`Layout::GlRgba`), so neither side converts pixels on the CPU; frames in the cairo layout (before the runner
//! switched) are swizzled and flipped by the shader instead.
//!
//! The entry points come from the GL library GDK uses: EGL (Wayland, and X11 with GDK's EGL) or GLX.

use bhpc_launcher::embed::{FrameInfo, Layout};
use std::ffi::{c_char, c_void, CStr, CString};

const TEXTURE_2D: u32 = 0x0DE1;
const RGBA: u32 = 0x1908;
const RGBA8: i32 = 0x8058;
const UNSIGNED_BYTE: u32 = 0x1401;
const TEXTURE_MIN_FILTER: u32 = 0x2801;
const TEXTURE_MAG_FILTER: u32 = 0x2800;
const TEXTURE_WRAP_S: u32 = 0x2802;
const TEXTURE_WRAP_T: u32 = 0x2803;
const NEAREST: i32 = 0x2600;
const CLAMP_TO_EDGE: i32 = 0x812F;
const UNPACK_ALIGNMENT: u32 = 0x0CF5;
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
const BLEND: u32 = 0x0BE2;
const SCISSOR_TEST: u32 = 0x0C11;

/// Full-view strip: position then texture coordinate (`v` = 0 at the bottom).
const QUAD: [f32; 16] = [-1.0, -1.0, 0.0, 0.0, 1.0, -1.0, 1.0, 0.0, -1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0];

macro_rules! gl_table {
    ($($name:ident: fn($($t:ty),*) $(-> $r:ty)?;)*) => {
        #[allow(non_snake_case)]
        struct Gl { $($name: unsafe extern "C" fn($($t),*) $(-> $r)?,)* }
        impl Gl {
            fn load(loader: &dyn Fn(&str) -> *const c_void) -> Result<Gl, String> {
                Ok(Gl { $($name: {
                    let p = loader(stringify!($name));
                    if p.is_null() { return Err(format!("no {}", stringify!($name))); }
                    // SAFETY: a non-null entry point returned for a GL name has that function's C signature.
                    unsafe { std::mem::transmute::<*const c_void, unsafe extern "C" fn($($t),*) $(-> $r)?>(p) }
                },)* })
            }
        }
    };
}

gl_table! {
    glGenTextures: fn(i32, *mut u32);
    glBindTexture: fn(u32, u32);
    glDeleteTextures: fn(i32, *const u32);
    glTexImage2D: fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void);
    glTexSubImage2D: fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void);
    glTexParameteri: fn(u32, u32, i32);
    glPixelStorei: fn(u32, i32);
    glActiveTexture: fn(u32);
    glCreateShader: fn(u32) -> u32;
    glShaderSource: fn(u32, i32, *const *const c_char, *const i32);
    glCompileShader: fn(u32);
    glGetShaderiv: fn(u32, u32, *mut i32);
    glGetShaderInfoLog: fn(u32, i32, *mut i32, *mut c_char);
    glDeleteShader: fn(u32);
    glCreateProgram: fn() -> u32;
    glAttachShader: fn(u32, u32);
    glBindAttribLocation: fn(u32, u32, *const c_char);
    glLinkProgram: fn(u32);
    glGetProgramiv: fn(u32, u32, *mut i32);
    glUseProgram: fn(u32);
    glDeleteProgram: fn(u32);
    glGetUniformLocation: fn(u32, *const c_char) -> i32;
    glUniform1i: fn(i32, i32);
    glUniform1f: fn(i32, f32);
    glGenBuffers: fn(i32, *mut u32);
    glBindBuffer: fn(u32, u32);
    glBufferData: fn(u32, isize, *const c_void, u32);
    glDeleteBuffers: fn(i32, *const u32);
    glVertexAttribPointer: fn(u32, i32, u32, u8, i32, *const c_void);
    glEnableVertexAttribArray: fn(u32);
    glDrawArrays: fn(u32, i32, i32);
    glViewport: fn(i32, i32, i32, i32);
    glClearColor: fn(f32, f32, f32, f32);
    glClear: fn(u32);
    glDisable: fn(u32);
}

/// Core profiles draw nothing without a vertex array object; GLES 2 and legacy contexts may not have one.
struct Vao {
    gen: unsafe extern "C" fn(i32, *mut u32),
    bind: unsafe extern "C" fn(u32),
    delete: unsafe extern "C" fn(i32, *const u32),
}

/// Which GL dialect the context speaks: GLSL needs a matching `#version`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// OpenGL ES 2 or later (`#version 100`).
    Es,
    /// Desktop GL 2.x (`#version 110`).
    Legacy,
    /// Desktop GL 3.2+ core profile (`#version 150`).
    Core,
}

/// The shader sources for `dialect`. The fragment shader takes the frame's layout as uniforms: `bgra` swaps red and
/// blue (cairo's byte order), `top_down` flips rows (cairo's row order); `alpha` fades the picture in.
pub fn shaders(dialect: Dialect) -> (String, String) {
    let (version, attr, vary_out, vary_in, frag_out, tex) = match dialect {
        Dialect::Es => ("#version 100\nprecision mediump float;", "attribute", "varying", "varying", "", "texture2D"),
        Dialect::Legacy => ("#version 110", "attribute", "varying", "varying", "", "texture2D"),
        Dialect::Core => ("#version 150", "in", "out", "in", "out vec4 frag;", "texture"),
    };
    let color = if dialect == Dialect::Core { "frag" } else { "gl_FragColor" };
    let vs = format!(
        "{version}\n{attr} vec2 pos; {attr} vec2 uv; uniform float top_down; {vary_out} vec2 v;\n\
         void main() {{ v = vec2(uv.x, mix(uv.y, 1.0 - uv.y, top_down)); gl_Position = vec4(pos, 0.0, 1.0); }}\n"
    );
    let fs = format!(
        "{version}\n{vary_in} vec2 v; uniform sampler2D t; uniform float bgra; uniform float alpha; {frag_out}\n\
         void main() {{ vec3 c = {tex}(t, v).rgb; c = mix(c, c.bgr, bgra); {color} = vec4(c * alpha, alpha); }}\n"
    );
    (vs, fs)
}

/// Where the frame goes inside a view of `view` device pixels: its own size, at the top-left corner (as the cairo
/// painter does; the runner renders at the view's size, so this only shows during a resize). GL viewport origin is
/// the bottom-left corner.
pub fn frame_viewport(view: (i32, i32), frame: (u32, u32)) -> [i32; 4] {
    let (w, h) = (frame.0 as i32, frame.1 as i32);
    [0, view.1 - h, w, h]
}

/// The shader uniforms (`bgra`, `top_down`) for a frame in `layout`.
pub fn layout_uniforms(layout: Layout) -> (f32, f32) {
    match layout {
        Layout::Bgra => (1.0, 1.0),
        Layout::GlRgba => (0.0, 0.0),
    }
}

/// The GL objects of one view. Make its `GtkGLArea`'s context current before every call.
pub struct GlPainter {
    gl: Gl,
    vao: Option<(Vao, u32)>,
    program: u32,
    vbo: u32,
    tex: u32,
    uniforms: [i32; 3],
    /// Size and layout of the frame in the texture (`None`: no frame yet).
    frame: Option<(u32, u32, Layout)>,
}

impl GlPainter {
    /// Builds the program and buffers in the current context.
    pub fn new(dialect: Dialect) -> Result<GlPainter, String> {
        let loader = loader()?;
        let gl = Gl::load(&loader)?;
        let vao = match dialect {
            Dialect::Core => {
                let get = |n: &str| Some(loader(n)).filter(|p| !p.is_null());
                let (gen, bind, delete) = (
                    get("glGenVertexArrays").ok_or("no glGenVertexArrays")?,
                    get("glBindVertexArray").ok_or("no glBindVertexArray")?,
                    get("glDeleteVertexArrays").ok_or("no glDeleteVertexArrays")?,
                );
                // SAFETY: non-null entry points for these names have these signatures.
                Some(unsafe {
                    Vao {
                        gen: std::mem::transmute::<*const c_void, unsafe extern "C" fn(i32, *mut u32)>(gen),
                        bind: std::mem::transmute::<*const c_void, unsafe extern "C" fn(u32)>(bind),
                        delete: std::mem::transmute::<*const c_void, unsafe extern "C" fn(i32, *const u32)>(delete),
                    }
                })
            }
            _ => None,
        };
        let (vs, fs) = shaders(dialect);
        // SAFETY: a context is current (the caller's contract); all names are created here and owned by the painter.
        unsafe {
            let program = build_program(&gl, &vs, &fs)?;
            let uniform = |n: &str| (gl.glGetUniformLocation)(program, cstr(n).as_ptr());
            let uniforms = [uniform("bgra"), uniform("top_down"), uniform("alpha")];
            (gl.glUseProgram)(program);
            (gl.glUniform1i)(uniform("t"), 0);
            let vao = vao.map(|v| {
                let mut name = 0;
                (v.gen)(1, &mut name);
                (v.bind)(name);
                (v, name)
            });
            let mut vbo = 0;
            (gl.glGenBuffers)(1, &mut vbo);
            (gl.glBindBuffer)(ARRAY_BUFFER, vbo);
            (gl.glBufferData)(ARRAY_BUFFER, std::mem::size_of_val(&QUAD) as isize, QUAD.as_ptr().cast(), STATIC_DRAW);
            for (i, offset) in [(0u32, 0usize), (1, 8)] {
                (gl.glVertexAttribPointer)(i, 2, FLOAT, 0, 16, offset as *const c_void);
                (gl.glEnableVertexAttribArray)(i);
            }
            let mut tex = 0;
            (gl.glGenTextures)(1, &mut tex);
            (gl.glBindTexture)(TEXTURE_2D, tex);
            for (p, v) in [
                (TEXTURE_MIN_FILTER, NEAREST),
                (TEXTURE_MAG_FILTER, NEAREST),
                (TEXTURE_WRAP_S, CLAMP_TO_EDGE),
                (TEXTURE_WRAP_T, CLAMP_TO_EDGE),
            ] {
                (gl.glTexParameteri)(TEXTURE_2D, p, v);
            }
            Ok(GlPainter { gl, vao, program, vbo, tex, uniforms, frame: None })
        }
    }

    /// Puts a frame's pixels into the texture.
    pub fn upload(&mut self, info: FrameInfo, pixels: &[u8]) {
        let (w, h) = (info.width, info.height);
        if pixels.len() != w as usize * h as usize * 4 {
            return;
        }
        let g = &self.gl;
        // SAFETY: a context is current; `pixels` holds w*h RGBA texels and GL copies them before returning.
        unsafe {
            (g.glActiveTexture)(TEXTURE0);
            (g.glBindTexture)(TEXTURE_2D, self.tex);
            (g.glPixelStorei)(UNPACK_ALIGNMENT, 4);
            if self.frame.is_some_and(|(fw, fh, _)| (fw, fh) == (w, h)) {
                (g.glTexSubImage2D)(
                    TEXTURE_2D,
                    0,
                    0,
                    0,
                    w as i32,
                    h as i32,
                    RGBA,
                    UNSIGNED_BYTE,
                    pixels.as_ptr().cast(),
                );
            } else {
                (g.glTexImage2D)(
                    TEXTURE_2D,
                    0,
                    RGBA8,
                    w as i32,
                    h as i32,
                    0,
                    RGBA,
                    UNSIGNED_BYTE,
                    pixels.as_ptr().cast(),
                );
            }
        }
        self.frame = Some((w, h, info.layout));
    }

    /// Draws into the area's framebuffer (bound by GTK) of `view` device pixels: black at `alpha`, with the frame
    /// (if any) at its own size in the top-left corner.
    pub fn draw(&self, view: (i32, i32), alpha: f32) {
        let g = &self.gl;
        // SAFETY: a context is current; only objects of this painter are used.
        unsafe {
            (g.glDisable)(BLEND);
            (g.glDisable)(SCISSOR_TEST);
            (g.glViewport)(0, 0, view.0, view.1);
            (g.glClearColor)(0.0, 0.0, 0.0, alpha);
            (g.glClear)(COLOR_BUFFER_BIT);
            let Some((w, h, layout)) = self.frame else { return };
            let [x, y, vw, vh] = frame_viewport(view, (w, h));
            (g.glViewport)(x, y, vw, vh);
            (g.glUseProgram)(self.program);
            if let Some((v, name)) = &self.vao {
                (v.bind)(*name);
            }
            (g.glBindBuffer)(ARRAY_BUFFER, self.vbo);
            for (i, offset) in [(0u32, 0usize), (1, 8)] {
                (g.glVertexAttribPointer)(i, 2, FLOAT, 0, 16, offset as *const c_void);
                (g.glEnableVertexAttribArray)(i);
            }
            let (bgra, top_down) = layout_uniforms(layout);
            (g.glUniform1f)(self.uniforms[0], bgra);
            (g.glUniform1f)(self.uniforms[1], top_down);
            (g.glUniform1f)(self.uniforms[2], alpha);
            (g.glActiveTexture)(TEXTURE0);
            (g.glBindTexture)(TEXTURE_2D, self.tex);
            (g.glDrawArrays)(TRIANGLE_STRIP, 0, 4);
        }
    }

    /// Deletes the GL objects (the context must be current).
    pub fn destroy(self) {
        let g = &self.gl;
        // SAFETY: a context is current; the names are this painter's.
        unsafe {
            (g.glDeleteTextures)(1, &self.tex);
            (g.glDeleteBuffers)(1, &self.vbo);
            (g.glDeleteProgram)(self.program);
            if let Some((v, name)) = &self.vao {
                (v.delete)(1, name);
            }
        }
    }
}

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

unsafe fn build_program(g: &Gl, vs: &str, fs: &str) -> Result<u32, String> {
    let compile = |kind: u32, src: &str| -> Result<u32, String> {
        let sh = (g.glCreateShader)(kind);
        let src = cstr(src);
        (g.glShaderSource)(sh, 1, &src.as_ptr(), std::ptr::null());
        (g.glCompileShader)(sh);
        let mut ok = 0;
        (g.glGetShaderiv)(sh, COMPILE_STATUS, &mut ok);
        if ok == 0 {
            let mut log = vec![0 as c_char; 1024];
            (g.glGetShaderInfoLog)(sh, log.len() as i32, std::ptr::null_mut(), log.as_mut_ptr());
            (g.glDeleteShader)(sh);
            return Err(format!("shader: {}", CStr::from_ptr(log.as_ptr()).to_string_lossy()));
        }
        Ok(sh)
    };
    let (v, f) = (compile(VERTEX_SHADER, vs)?, compile(FRAGMENT_SHADER, fs)?);
    let p = (g.glCreateProgram)();
    (g.glAttachShader)(p, v);
    (g.glAttachShader)(p, f);
    (g.glBindAttribLocation)(p, 0, cstr("pos").as_ptr());
    (g.glBindAttribLocation)(p, 1, cstr("uv").as_ptr());
    (g.glLinkProgram)(p);
    (g.glDeleteShader)(v);
    (g.glDeleteShader)(f);
    let mut ok = 0;
    (g.glGetProgramiv)(p, LINK_STATUS, &mut ok);
    if ok == 0 {
        (g.glDeleteProgram)(p);
        return Err("the program does not link".into());
    }
    Ok(p)
}

/// `dlopen`ed once: a library handle and its `GetProcAddress`.
struct Lib {
    get_proc: unsafe extern "C" fn(*const c_char) -> *const c_void,
    /// `eglGetCurrentContext` (EGL only): tells whether GDK's context is an EGL one.
    current: Option<unsafe extern "C" fn() -> *mut c_void>,
}

fn open_lib(names: &[&str], get_proc: &str, current: Option<&str>) -> Option<Lib> {
    // SAFETY: dlopen/dlsym with NUL-terminated names; the handles stay open for the life of the process.
    unsafe {
        let handle = names
            .iter()
            .map(|n| libc::dlopen(cstr(n).as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL))
            .find(|h| !h.is_null())?;
        let sym = |n: &str| Some(libc::dlsym(handle, cstr(n).as_ptr())).filter(|p| !p.is_null());
        let get_proc =
            std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*const c_char) -> *const c_void>(sym(get_proc)?);
        let current = match current {
            Some(n) => Some(std::mem::transmute::<*mut c_void, unsafe extern "C" fn() -> *mut c_void>(sym(n)?)),
            None => None,
        };
        Some(Lib { get_proc, current })
    }
}

/// The `GetProcAddress` of the API GDK's current context belongs to.
fn loader() -> Result<impl Fn(&str) -> *const c_void, String> {
    let egl = open_lib(&["libEGL.so.1", "libEGL.so"], "eglGetProcAddress", Some("eglGetCurrentContext"));
    // SAFETY: plain EGL query.
    let lib = match egl.filter(|l| l.current.is_some_and(|c| !unsafe { c() }.is_null())) {
        Some(l) => l,
        None => open_lib(&["libGL.so.1", "libGL.so"], "glXGetProcAddressARB", None).ok_or("no EGL or GLX library")?,
    };
    // SAFETY: the library's GetProcAddress takes a NUL-terminated name.
    Ok(move |name: &str| unsafe { (lib.get_proc)(cstr(name).as_ptr()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shaders_speak_the_contexts_dialect() {
        let (vs, fs) = shaders(Dialect::Core);
        assert!(vs.starts_with("#version 150") && vs.contains("in vec2 pos") && vs.contains("out vec2 v"));
        assert!(fs.contains("out vec4 frag;") && fs.contains("frag = ") && fs.contains("texture(t, v)"));
        let (vs, fs) = shaders(Dialect::Es);
        assert!(vs.starts_with("#version 100\nprecision mediump float;") && vs.contains("attribute vec2 uv"));
        assert!(fs.contains("gl_FragColor") && fs.contains("texture2D(t, v)") && fs.contains("varying vec2 v"));
        let (vs, _) = shaders(Dialect::Legacy);
        assert!(vs.starts_with("#version 110\n"));
    }

    #[test]
    fn frames_sit_in_the_top_left_corner() {
        assert_eq!(frame_viewport((800, 600), (800, 600)), [0, 0, 800, 600]);
        assert_eq!(frame_viewport((800, 600), (400, 500)), [0, 100, 400, 500]);
    }

    #[test]
    fn the_cairo_layout_is_swizzled_and_flipped() {
        assert_eq!(layout_uniforms(Layout::Bgra), (1.0, 1.0));
        assert_eq!(layout_uniforms(Layout::GlRgba), (0.0, 0.0));
    }
}

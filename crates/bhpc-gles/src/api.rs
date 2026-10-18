//! The host GL surface: one trait method per host function, a dynamic-library backed implementation
//! ([`HostGl`]) and (in tests) a recording mock. Everything is generated from one function list so the
//! signatures cannot drift apart.
//!
//! Entries are tagged `direct` (all arguments are scalars: the guest thunk is generated too) or `host`
//! (pointer arguments: the thunk is written by hand in the `marshal` modules).

#![allow(clippy::too_many_arguments, clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::c_void;

/// `const void *`.
pub type P = *const c_void;
/// `void *`.
pub type PM = *mut c_void;

/// Value of an argument of a generated function, as seen by the recording mock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Arg {
    I(i64),
    F(f64),
    P(usize),
}

/// A value that can be read from guest registers (softfp: floats travel as bits in core registers).
pub trait FromGuest: Sized {
    fn take(a: &mut bhpc_cpu::ArgCursor, c: &bhpc_cpu::Cpu) -> Self;
}

/// A value that can be returned to the guest.
pub trait ToGuest {
    fn put(self, c: &mut bhpc_cpu::Cpu);
}

macro_rules! guest_int {
    ($($t:ty),*) => {$(
        impl FromGuest for $t {
            fn take(a: &mut bhpc_cpu::ArgCursor, c: &bhpc_cpu::Cpu) -> Self { a.u32(c) as $t }
        }
        impl ToGuest for $t {
            fn put(self, c: &mut bhpc_cpu::Cpu) { c.ret_u32(self as u32) }
        }
    )*};
}
guest_int!(u32, i32, u8);

impl FromGuest for f32 {
    fn take(a: &mut bhpc_cpu::ArgCursor, c: &bhpc_cpu::Cpu) -> Self {
        a.f32(c)
    }
}

impl ToGuest for () {
    fn put(self, _: &mut bhpc_cpu::Cpu) {}
}

/// A throw-away value used by the generated exercisers and dummy backends.
pub trait Dummy {
    fn dummy() -> Self;
}
macro_rules! dummy_default {
    ($($t:ty),*) => {$(impl Dummy for $t { fn dummy() -> Self { Default::default() } })*};
}
dummy_default!((), u8, u32, i32, isize, f32);
impl Dummy for P {
    fn dummy() -> Self {
        std::ptr::null()
    }
}
impl Dummy for PM {
    fn dummy() -> Self {
        std::ptr::null_mut()
    }
}

/// Conversion of a call argument for the recorder.
pub trait ToArg {
    fn to_arg(&self) -> Arg;
}
macro_rules! to_arg_int {
    ($($t:ty),*) => {$(impl ToArg for $t { fn to_arg(&self) -> Arg { Arg::I(*self as i64) } })*};
}
to_arg_int!(u8, u32, i32, isize);
impl ToArg for f32 {
    fn to_arg(&self) -> Arg {
        Arg::F(*self as f64)
    }
}
impl ToArg for P {
    fn to_arg(&self) -> Arg {
        Arg::P(*self as usize)
    }
}
impl ToArg for PM {
    fn to_arg(&self) -> Arg {
        Arg::P(*self as usize)
    }
}

/// Conversion of a recorded return value.
pub trait FromRet {
    fn from_ret(v: i64) -> Self;
}
macro_rules! from_ret_int {
    ($($t:ty),*) => {$(impl FromRet for $t { fn from_ret(v: i64) -> Self { v as $t } })*};
}
from_ret_int!(u8, u32, i32);
impl FromRet for () {
    fn from_ret(_: i64) -> Self {}
}
impl FromRet for P {
    fn from_ret(v: i64) -> Self {
        v as usize as P
    }
}

/// The master list. `$cb` is invoked with `{ kind fn Name(args) -> ret; ... }`.
macro_rules! gl_list {
    ($cb:ident) => {
        $cb! {
            direct fn glActiveTexture(texture: u32);
            direct fn glAttachShader(program: u32, shader: u32);
            direct fn glBindRenderbuffer(target: u32, renderbuffer: u32);
            direct fn glBindTexture(target: u32, texture: u32);
            direct fn glBlendColor(r: f32, g: f32, b: f32, a: f32);
            direct fn glBlendEquation(mode: u32);
            direct fn glBlendEquationSeparate(rgb: u32, alpha: u32);
            direct fn glBlendFunc(sfactor: u32, dfactor: u32);
            direct fn glBlendFuncSeparate(srgb: u32, drgb: u32, salpha: u32, dalpha: u32);
            direct fn glCheckFramebufferStatus(target: u32) -> u32;
            direct fn glClear(mask: u32);
            direct fn glClearColor(r: f32, g: f32, b: f32, a: f32);
            direct fn glClearDepthf(d: f32);
            direct fn glClearStencil(s: i32);
            direct fn glColorMask(r: u8, g: u8, b: u8, a: u8);
            direct fn glCopyTexImage2D(target: u32, level: i32, format: u32, x: i32, y: i32, w: i32, h: i32, border: i32);
            direct fn glCopyTexSubImage2D(target: u32, level: i32, xo: i32, yo: i32, x: i32, y: i32, w: i32, h: i32);
            direct fn glCreateProgram() -> u32;
            direct fn glCreateShader(kind: u32) -> u32;
            direct fn glCullFace(mode: u32);
            direct fn glDepthFunc(func: u32);
            direct fn glDepthMask(flag: u8);
            direct fn glDepthRangef(n: f32, f: f32);
            direct fn glDeleteProgram(program: u32);
            direct fn glDeleteShader(shader: u32);
            direct fn glDetachShader(program: u32, shader: u32);
            direct fn glDisable(cap: u32);
            direct fn glDisableVertexAttribArray(index: u32);
            direct fn glDrawArrays(mode: u32, first: i32, count: i32);
            direct fn glEnable(cap: u32);
            direct fn glEnableVertexAttribArray(index: u32);
            direct fn glFinish();
            direct fn glFlush();
            direct fn glFramebufferRenderbuffer(target: u32, attachment: u32, rbtarget: u32, rb: u32);
            direct fn glFramebufferTexture2D(target: u32, attachment: u32, textarget: u32, texture: u32, level: i32);
            direct fn glFrontFace(mode: u32);
            direct fn glGenerateMipmap(target: u32);
            direct fn glHint(target: u32, mode: u32);
            direct fn glIsBuffer(buffer: u32) -> u8;
            direct fn glIsEnabled(cap: u32) -> u8;
            direct fn glIsFramebuffer(fb: u32) -> u8;
            direct fn glIsProgram(program: u32) -> u8;
            direct fn glIsRenderbuffer(rb: u32) -> u8;
            direct fn glIsShader(shader: u32) -> u8;
            direct fn glIsTexture(texture: u32) -> u8;
            direct fn glLineWidth(width: f32);
            direct fn glLinkProgram(program: u32);
            direct fn glPolygonOffset(factor: f32, units: f32);
            direct fn glReleaseShaderCompiler();
            direct fn glRenderbufferStorage(target: u32, format: u32, w: i32, h: i32);
            direct fn glSampleCoverage(value: f32, invert: u8);
            direct fn glScissor(x: i32, y: i32, w: i32, h: i32);
            direct fn glStencilFunc(func: u32, r: i32, mask: u32);
            direct fn glStencilFuncSeparate(face: u32, func: u32, r: i32, mask: u32);
            direct fn glStencilMask(mask: u32);
            direct fn glStencilMaskSeparate(face: u32, mask: u32);
            direct fn glStencilOp(fail: u32, zfail: u32, zpass: u32);
            direct fn glStencilOpSeparate(face: u32, fail: u32, zfail: u32, zpass: u32);
            direct fn glTexParameterf(target: u32, pname: u32, param: f32);
            direct fn glTexParameteri(target: u32, pname: u32, param: i32);
            direct fn glUniform1f(loc: i32, x: f32);
            direct fn glUniform2f(loc: i32, x: f32, y: f32);
            direct fn glUniform3f(loc: i32, x: f32, y: f32, z: f32);
            direct fn glUniform4f(loc: i32, x: f32, y: f32, z: f32, w: f32);
            direct fn glUniform1i(loc: i32, x: i32);
            direct fn glUniform2i(loc: i32, x: i32, y: i32);
            direct fn glUniform3i(loc: i32, x: i32, y: i32, z: i32);
            direct fn glUniform4i(loc: i32, x: i32, y: i32, z: i32, w: i32);
            direct fn glUseProgram(program: u32);
            direct fn glValidateProgram(program: u32);
            direct fn glVertexAttrib1f(index: u32, x: f32);
            direct fn glVertexAttrib2f(index: u32, x: f32, y: f32);
            direct fn glVertexAttrib3f(index: u32, x: f32, y: f32, z: f32);
            direct fn glVertexAttrib4f(index: u32, x: f32, y: f32, z: f32, w: f32);
            direct fn glViewport(x: i32, y: i32, w: i32, h: i32);
            direct fn glIsVertexArray(array: u32) -> u8;
            host fn glRenderbufferStorageMultisample(target: u32, samples: i32, format: u32, w: i32, h: i32);

            host fn glBindAttribLocation(program: u32, index: u32, name: P);
            host fn glBindBuffer(target: u32, buffer: u32);
            host fn glBufferData(target: u32, size: isize, data: P, usage: u32);
            host fn glBufferSubData(target: u32, offset: isize, size: isize, data: P);
            host fn glCompressedTexImage2D(target: u32, level: i32, format: u32, w: i32, h: i32, border: i32, size: i32, data: P);
            host fn glCompressedTexSubImage2D(target: u32, level: i32, xo: i32, yo: i32, w: i32, h: i32, format: u32, size: i32, data: P);
            host fn glDeleteBuffers(n: i32, buffers: P);
            host fn glDeleteFramebuffers(n: i32, ids: P);
            host fn glDeleteRenderbuffers(n: i32, ids: P);
            host fn glDeleteTextures(n: i32, ids: P);
            host fn glDrawElements(mode: u32, count: i32, kind: u32, indices: P);
            host fn glGenBuffers(n: i32, ids: PM);
            host fn glGenFramebuffers(n: i32, ids: PM);
            host fn glGenRenderbuffers(n: i32, ids: PM);
            host fn glGenTextures(n: i32, ids: PM);
            host fn glGetActiveAttrib(program: u32, index: u32, bufsize: i32, length: PM, size: PM, kind: PM, name: PM);
            host fn glGetActiveUniform(program: u32, index: u32, bufsize: i32, length: PM, size: PM, kind: PM, name: PM);
            host fn glGetAttachedShaders(program: u32, max: i32, count: PM, shaders: PM);
            host fn glGetAttribLocation(program: u32, name: P) -> i32;
            host fn glGetBooleanv(pname: u32, out: PM);
            host fn glGetBufferParameteriv(target: u32, pname: u32, out: PM);
            host fn glGetError() -> u32;
            host fn glGetFloatv(pname: u32, out: PM);
            host fn glGetFramebufferAttachmentParameteriv(target: u32, attachment: u32, pname: u32, out: PM);
            host fn glGetIntegerv(pname: u32, out: PM);
            host fn glGetProgramInfoLog(program: u32, bufsize: i32, length: PM, log: PM);
            host fn glGetProgramiv(program: u32, pname: u32, out: PM);
            host fn glGetRenderbufferParameteriv(target: u32, pname: u32, out: PM);
            host fn glCompileShader(shader: u32);
            host fn glGetShaderInfoLog(shader: u32, bufsize: i32, length: PM, log: PM);
            host fn glGetShaderPrecisionFormat(shader: u32, precision: u32, range: PM, bits: PM);
            host fn glGetShaderSource(shader: u32, bufsize: i32, length: PM, src: PM);
            host fn glGetShaderiv(shader: u32, pname: u32, out: PM);
            host fn glGetString(name: u32) -> P;
            host fn glGetTexParameterfv(target: u32, pname: u32, out: PM);
            host fn glGetTexParameteriv(target: u32, pname: u32, out: PM);
            host fn glGetUniformLocation(program: u32, name: P) -> i32;
            host fn glGetUniformfv(program: u32, loc: i32, out: PM);
            host fn glGetUniformiv(program: u32, loc: i32, out: PM);
            host fn glGetVertexAttribfv(index: u32, pname: u32, out: PM);
            host fn glGetVertexAttribiv(index: u32, pname: u32, out: PM);
            host fn glPixelStorei(pname: u32, param: i32);
            host fn glReadPixels(x: i32, y: i32, w: i32, h: i32, format: u32, kind: u32, pixels: PM);
            host fn glShaderSource(shader: u32, count: i32, strings: P, lengths: P);
            host fn glTexImage2D(target: u32, level: i32, internal: i32, w: i32, h: i32, border: i32, format: u32, kind: u32, pixels: P);
            host fn glTexParameterfv(target: u32, pname: u32, params: P);
            host fn glTexParameteriv(target: u32, pname: u32, params: P);
            host fn glTexSubImage2D(target: u32, level: i32, xo: i32, yo: i32, w: i32, h: i32, format: u32, kind: u32, pixels: P);
            host fn glUniform1fv(loc: i32, count: i32, v: P);
            host fn glUniform2fv(loc: i32, count: i32, v: P);
            host fn glUniform3fv(loc: i32, count: i32, v: P);
            host fn glUniform4fv(loc: i32, count: i32, v: P);
            host fn glUniform1iv(loc: i32, count: i32, v: P);
            host fn glUniform2iv(loc: i32, count: i32, v: P);
            host fn glUniform3iv(loc: i32, count: i32, v: P);
            host fn glBindFramebuffer(target: u32, framebuffer: u32);
            host fn glUniform4iv(loc: i32, count: i32, v: P);
            host fn glUniformMatrix2fv(loc: i32, count: i32, transpose: u8, v: P);
            host fn glUniformMatrix3fv(loc: i32, count: i32, transpose: u8, v: P);
            host fn glUniformMatrix4fv(loc: i32, count: i32, transpose: u8, v: P);
            host fn glVertexAttrib1fv(index: u32, v: P);
            host fn glVertexAttrib2fv(index: u32, v: P);
            host fn glVertexAttrib3fv(index: u32, v: P);
            host fn glVertexAttrib4fv(index: u32, v: P);
            host fn glVertexAttribPointer(index: u32, size: i32, kind: u32, norm: u8, stride: i32, ptr: P);
            host fn glGenVertexArrays(n: i32, ids: PM);
            host fn glBindVertexArray(array: u32);
            host fn glDeleteVertexArrays(n: i32, ids: P);
            host fn glTexImage3D(target: u32, level: i32, internal: i32, w: i32, h: i32, d: i32, border: i32, format: u32, kind: u32, pixels: P);
            host fn glTexSubImage3D(target: u32, level: i32, xo: i32, yo: i32, zo: i32, w: i32, h: i32, d: i32, format: u32, kind: u32, pixels: P);
            host fn glCopyTexSubImage3D(target: u32, level: i32, xo: i32, yo: i32, zo: i32, x: i32, y: i32, w: i32, h: i32);
            host fn glCompressedTexImage3D(target: u32, level: i32, format: u32, w: i32, h: i32, d: i32, border: i32, size: i32, data: P);
            host fn glCompressedTexSubImage3D(target: u32, level: i32, xo: i32, yo: i32, zo: i32, w: i32, h: i32, d: i32, format: u32, size: i32, data: P);
        }
    };
}
pub(crate) use gl_list;

macro_rules! gen_trait {
    ($($kind:ident fn $name:ident($($an:ident: $at:ty),*) $(-> $rt:ty)?;)*) => {
        /// The host GL implementation the guest thunks drive. Pointer arguments are host pointers.
        #[allow(non_snake_case)]
        pub trait GlApi {
            /// True if the host provides `name` (an optional function may be missing).
            fn supports(&self, name: &str) -> bool;
            $(fn $name(&mut self, $($an: $at),*) $(-> $rt)?;)*
        }

        /// Names of all functions of [`GlApi`].
        pub const API_NAMES: &[&str] = &[$(stringify!($name)),*];

        /// A [`GlApi`] over function pointers resolved by name (for example `SDL_GL_GetProcAddress`).
        #[allow(non_snake_case)]
        pub struct HostGl {
            $($name: Option<unsafe extern "C" fn($($at),*) $(-> $rt)?>,)*
        }

        impl HostGl {
            /// Resolve every function with `loader`, which returns null for unknown names. A missing
            /// `glFoo` is retried as `glFooOES` and `glFooEXT`.
            pub fn load(loader: &mut dyn FnMut(&str) -> *const c_void) -> HostGl {
                let mut find = |n: &str| -> *const c_void {
                    let p = loader(n);
                    if !p.is_null() {
                        return p;
                    }
                    let p = loader(&format!("{n}OES"));
                    if !p.is_null() {
                        return p;
                    }
                    loader(&format!("{n}EXT"))
                };
                HostGl {
                    // SAFETY: a non-null pointer returned for a GL entry point has that entry point's C signature.
                    $($name: { let p = find(stringify!($name)); if p.is_null() { None } else { Some(unsafe { std::mem::transmute::<*const c_void, unsafe extern "C" fn($($at),*) $(-> $rt)?>(p) }) } },)*
                }
            }
        }

        impl GlApi for HostGl {
            fn supports(&self, name: &str) -> bool {
                match name {
                    $(stringify!($name) => self.$name.is_some(),)*
                    _ => false,
                }
            }
            $(fn $name(&mut self, $($an: $at),*) $(-> $rt)? {
                match self.$name {
                    // SAFETY: the pointer was resolved for exactly this signature; the caller validated pointer arguments.
                    Some(f) => unsafe { f($($an),*) },
                    None => Dummy::dummy(),
                }
            })*
        }

        /// Call every method with dummy arguments (coverage of the generated forwarders).
        #[cfg(test)]
        #[allow(non_snake_case)]
        pub fn exercise_all(api: &mut dyn GlApi) {
            $(let _ = api.$name($(<$at as Dummy>::dummy()),*);)*
        }

        /// Dummy C implementations of every function, standing in for a GL driver.
        #[cfg(test)]
        #[allow(non_snake_case, unused_variables, clippy::missing_safety_doc)]
        pub mod dummy_driver {
            use super::*;
            $(pub unsafe extern "C" fn $name($($an: $at),*) $(-> $rt)? { Dummy::dummy() })*
            pub fn lookup(name: &str) -> *const c_void {
                match name {
                    $(stringify!($name) => $name as unsafe extern "C" fn($($at),*) $(-> $rt)? as *const c_void,)*
                    _ => std::ptr::null(),
                }
            }
        }

        /// Recording mock backend.
        #[cfg(test)]
        #[allow(non_snake_case)]
        impl GlApi for crate::tests_support::Recorder {
            fn supports(&self, name: &str) -> bool {
                !self.is_unsupported(name)
            }
            $(fn $name(&mut self, $($an: $at),*) $(-> $rt)? {
                self.record(stringify!($name), vec![$($an.to_arg()),*])
            })*
        }
    };
}
gl_list!(gen_trait);

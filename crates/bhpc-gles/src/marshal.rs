//! Hand-written guest thunks for GL functions with pointer arguments. Guest pointers become host pointers
//! (`base + addr`) after the range was checked to be mapped; strings and arrays that need a different layout
//! (shader sources, names) are copied.

use crate::api::{P, PM};
use crate::consts::*;
use crate::imgsize::image_bytes;
use crate::state::{guest_cstr, guest_write, host_ptr, Gles};
use crate::Handler;
use bhpc_cpu::Cpu;

macro_rules! args {
    ($c:expr; $($n:ident),+) => {
        let mut a = $c.args();
        $(let $n = a.u32($c);)+
    };
}
macro_rules! ptr {
    ($c:expr, $addr:expr, $len:expr, $w:expr) => {
        match host_ptr($c, $addr, $len, $w) {
            Some(p) => p,
            None => return,
        }
    };
}
pub(crate) use args;
pub(crate) use ptr;

/// Number of values `glGet*v` returns for `pname`.
pub fn query_count(pname: u32) -> u32 {
    match pname {
        GL_VIEWPORT | GL_SCISSOR_BOX | GL_COLOR_CLEAR_VALUE | GL_COLOR_WRITEMASK | GL_BLEND_COLOR => 4,
        GL_MAX_VIEWPORT_DIMS | GL_DEPTH_RANGE | GL_ALIASED_POINT_SIZE_RANGE | GL_ALIASED_LINE_WIDTH_RANGE => 2,
        _ => 1,
    }
}

/// Index type size for `glDrawElements` validation.
pub fn index_size(kind: u32) -> u32 {
    match kind {
        GL_UNSIGNED_INT => 4,
        GL_UNSIGNED_SHORT => 2,
        _ => 1,
    }
}

fn get_error(c: &mut Cpu, g: &Gles) {
    let mut s = g.lock();
    let e = if s.err != GL_NO_ERROR { std::mem::replace(&mut s.err, GL_NO_ERROR) } else { s.api.glGetError() };
    drop(s);
    c.ret_u32(e);
}

// ---------- buffers ----------

fn bind_buffer(c: &mut Cpu, g: &Gles) {
    args!(c; target, buffer);
    let mut s = g.lock();
    match target {
        GL_ARRAY_BUFFER => s.array_buffer = buffer,
        GL_ELEMENT_ARRAY_BUFFER => {
            let vao = s.vao;
            s.elem.insert(vao, buffer);
        }
        _ => {}
    }
    s.api.glBindBuffer(target, buffer);
}

fn buffer_data(c: &mut Cpu, g: &Gles) {
    args!(c; target, size, data, usage);
    let mut s = g.lock();
    if (size as i32) < 0 {
        s.raise(GL_INVALID_VALUE);
        return;
    }
    let p = ptr!(c, data, size, false);
    let bound = s.bound_buffer(target);
    if bound != 0 {
        s.buf_size.insert(bound, size as isize);
    }
    s.api.glBufferData(target, size as isize, p as P, usage);
}

fn buffer_sub_data(c: &mut Cpu, g: &Gles) {
    args!(c; target, offset, size, data);
    let mut s = g.lock();
    if (size as i32) < 0 || (offset as i32) < 0 {
        s.raise(GL_INVALID_VALUE);
        return;
    }
    let p = ptr!(c, data, size, false);
    s.api.glBufferSubData(target, offset as isize, size as isize, p as P);
}

fn delete_buffers(c: &mut Cpu, g: &Gles) {
    args!(c; n, ids);
    let mut s = g.lock();
    if (n as i32) <= 0 {
        if (n as i32) < 0 {
            s.raise(GL_INVALID_VALUE);
        }
        return;
    }
    let p = ptr!(c, ids, n * 4, false);
    let Ok(raw) = c.mem().read_bytes(ids, n * 4) else { return };
    for id in raw.chunks_exact(4) {
        s.forget_buffer(u32::from_le_bytes([id[0], id[1], id[2], id[3]]));
    }
    s.api.glDeleteBuffers(n as i32, p as P);
}

macro_rules! id_list {
    ($(($fname:ident, $m:ident, $ptr_ty:ty, $write:expr)),* $(,)?) => {$(
        fn $fname(c: &mut Cpu, g: &Gles) {
            args!(c; n, ids);
            let mut s = g.lock();
            if (n as i32) <= 0 {
                if (n as i32) < 0 {
                    s.raise(GL_INVALID_VALUE);
                }
                return;
            }
            let p = ptr!(c, ids, n * 4, $write);
            s.api.$m(n as i32, p as $ptr_ty);
        }
    )*};
}
id_list!(
    (gen_buffers, glGenBuffers, PM, true),
    (gen_framebuffers, glGenFramebuffers, PM, true),
    (gen_renderbuffers, glGenRenderbuffers, PM, true),
    (gen_textures, glGenTextures, PM, true),
    (delete_framebuffers, glDeleteFramebuffers, P, false),
    (delete_renderbuffers, glDeleteRenderbuffers, P, false),
    (delete_textures, glDeleteTextures, P, false),
);

// ---------- drawing and vertex arrays ----------

/// Pointer argument of an index/vertex pointer: an offset into the bound buffer, or client memory.
pub(crate) fn indices_ptr(c: &mut Cpu, buffer_bound: bool, addr: u32, bytes: u32) -> Option<P> {
    if buffer_bound {
        Some(addr as usize as P)
    } else {
        host_ptr(c, addr, bytes, false).map(|p| p as P)
    }
}

fn draw_elements(c: &mut Cpu, g: &Gles) {
    args!(c; mode, count, kind, indices);
    let mut s = g.lock();
    if (count as i32) < 0 {
        s.raise(GL_INVALID_VALUE);
        return;
    }
    let bound = s.element_buffer() != 0;
    let Some(p) = indices_ptr(c, bound, indices, count.saturating_mul(index_size(kind))) else { return };
    s.api.glDrawElements(mode, count as i32, kind, p);
}

fn vertex_attrib_pointer(c: &mut Cpu, g: &Gles) {
    args!(c; index, size, kind, norm, stride, ptr);
    let mut s = g.lock();
    if let Some(slot) = s.attrib_ptrs.get_mut(index as usize) {
        *slot = ptr;
    }
    let bound = s.array_buffer != 0;
    let Some(p) = indices_ptr(c, bound, ptr, 1) else { return };
    s.api.glVertexAttribPointer(index, size as i32, kind, norm as u8, stride as i32, p);
}

// ---------- textures ----------

fn pixel_storei(c: &mut Cpu, g: &Gles) {
    args!(c; pname, param);
    let mut s = g.lock();
    if matches!(param, 1 | 2 | 4 | 8) {
        match pname {
            GL_UNPACK_ALIGNMENT => s.unpack_align = param as i32,
            GL_PACK_ALIGNMENT => s.pack_align = param as i32,
            _ => {}
        }
    }
    s.api.glPixelStorei(pname, param as i32);
}

fn tex_image_2d(c: &mut Cpu, g: &Gles) {
    args!(c; target, level, internal, w, h, border, format, kind, pixels);
    let mut s = g.lock();
    let Some(len) = image_bytes(w as i32, h as i32, 1, format, kind, s.unpack_align) else {
        s.raise(GL_INVALID_ENUM);
        return;
    };
    let p = ptr!(c, pixels, len, false);
    s.api.glTexImage2D(target, level as i32, internal as i32, w as i32, h as i32, border as i32, format, kind, p as P);
}

fn tex_sub_image_2d(c: &mut Cpu, g: &Gles) {
    args!(c; target, level, xo, yo, w, h, format, kind, pixels);
    let mut s = g.lock();
    let Some(len) = image_bytes(w as i32, h as i32, 1, format, kind, s.unpack_align) else {
        s.raise(GL_INVALID_ENUM);
        return;
    };
    let p = ptr!(c, pixels, len, false);
    s.api.glTexSubImage2D(target, level as i32, xo as i32, yo as i32, w as i32, h as i32, format, kind, p as P);
}

fn compressed_tex_image_2d(c: &mut Cpu, g: &Gles) {
    args!(c; target, level, format, w, h, border, size, data);
    let mut s = g.lock();
    if (size as i32) < 0 {
        s.raise(GL_INVALID_VALUE);
        return;
    }
    let p = ptr!(c, data, size, false);
    s.api.glCompressedTexImage2D(target, level as i32, format, w as i32, h as i32, border as i32, size as i32, p as P);
}

fn compressed_tex_sub_image_2d(c: &mut Cpu, g: &Gles) {
    args!(c; target, level, xo, yo, w, h, format, size, data);
    let mut s = g.lock();
    if (size as i32) < 0 {
        s.raise(GL_INVALID_VALUE);
        return;
    }
    let p = ptr!(c, data, size, false);
    s.api.glCompressedTexSubImage2D(
        target,
        level as i32,
        xo as i32,
        yo as i32,
        w as i32,
        h as i32,
        format,
        size as i32,
        p as P,
    );
}

fn read_pixels(c: &mut Cpu, g: &Gles) {
    args!(c; x, y, w, h, format, kind, pixels);
    let mut s = g.lock();
    let Some(len) = image_bytes(w as i32, h as i32, 1, format, kind, s.pack_align) else {
        s.raise(GL_INVALID_ENUM);
        return;
    };
    let p = ptr!(c, pixels, len, true);
    s.api.glReadPixels(x as i32, y as i32, w as i32, h as i32, format, kind, p);
}

fn tex_parameter_v(c: &mut Cpu, g: &Gles, float: bool) {
    args!(c; target, pname, params);
    let mut s = g.lock();
    let p = ptr!(c, params, 4, false);
    if float {
        s.api.glTexParameterfv(target, pname, p as P);
    } else {
        s.api.glTexParameteriv(target, pname, p as P);
    }
}
fn tex_parameterfv(c: &mut Cpu, g: &Gles) {
    tex_parameter_v(c, g, true)
}
fn tex_parameteriv(c: &mut Cpu, g: &Gles) {
    tex_parameter_v(c, g, false)
}

// ---------- shaders and programs ----------

fn shader_source(c: &mut Cpu, g: &Gles) {
    args!(c; shader, count, strings, lengths);
    let mut s = g.lock();
    if (count as i32) < 0 || count > 4096 {
        s.raise(GL_INVALID_VALUE);
        return;
    }
    let Ok(table) = (if count == 0 { Ok(Vec::new()) } else { c.mem().read_bytes(strings, count * 4) }) else {
        return fault_read(c, strings, count * 4);
    };
    let lens = if lengths == 0 || count == 0 {
        Vec::new()
    } else {
        let Ok(l) = c.mem().read_bytes(lengths, count * 4) else { return fault_read(c, lengths, count * 4) };
        l
    };
    let mut sources: Vec<Vec<u8>> = Vec::new();
    for i in 0..count as usize {
        let word = |b: &[u8]| i32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
        let addr = word(&table) as u32;
        let len = if lens.is_empty() { -1 } else { word(&lens) };
        let bytes = if len >= 0 {
            match c.mem().read_bytes(addr, len as u32) {
                Ok(b) => b,
                Err(_) => return fault_read(c, addr, len as u32),
            }
        } else {
            let Some(b) = guest_cstr(c, addr) else { return };
            b
        };
        sources.push(bytes);
    }
    let ptrs: Vec<*const u8> = sources.iter().map(|b| b.as_ptr()).collect();
    let lens: Vec<i32> = sources.iter().map(|b| b.len() as i32).collect();
    s.api.glShaderSource(shader, count as i32, ptrs.as_ptr().cast(), lens.as_ptr().cast());
}

pub(crate) fn fault_read(c: &mut Cpu, addr: u32, len: u32) {
    c.request_stop(bhpc_cpu::Stop::Fault(bhpc_cpu::Fault { addr, size: len, write: false }));
}

/// `glGetAttribLocation`, `glGetUniformLocation`: a name in, a location out.
macro_rules! name_lookup {
    ($fname:ident, $m:ident) => {
        fn $fname(c: &mut Cpu, g: &Gles) {
            args!(c; program, name);
            let Some(mut n) = guest_cstr(c, name) else { return };
            n.push(0);
            let r = g.lock().api.$m(program, n.as_ptr().cast());
            c.ret_u32(r as u32);
        }
    };
}
name_lookup!(get_attrib_location, glGetAttribLocation);
name_lookup!(get_uniform_location, glGetUniformLocation);

fn bind_attrib_location(c: &mut Cpu, g: &Gles) {
    args!(c; program, index, name);
    let Some(mut n) = guest_cstr(c, name) else { return };
    n.push(0);
    g.lock().api.glBindAttribLocation(program, index, n.as_ptr().cast());
}

/// `glGetShaderInfoLog` & co: `(object, bufsize, *length, *buffer)`.
macro_rules! info_log {
    ($fname:ident, $m:ident) => {
        fn $fname(c: &mut Cpu, g: &Gles) {
            args!(c; object, bufsize, length, buffer);
            let mut s = g.lock();
            if (bufsize as i32) < 0 {
                s.raise(GL_INVALID_VALUE);
                return;
            }
            let pl = ptr!(c, length, 4, true);
            let pb = ptr!(c, buffer, bufsize, true);
            s.api.$m(object, bufsize as i32, pl, pb);
        }
    };
}
info_log!(get_shader_info_log, glGetShaderInfoLog);
info_log!(get_program_info_log, glGetProgramInfoLog);
info_log!(get_shader_source, glGetShaderSource);

fn active_var(c: &mut Cpu, g: &Gles, uniform: bool) {
    args!(c; program, index, bufsize, length, size, kind, name);
    let mut s = g.lock();
    if (bufsize as i32) < 0 {
        s.raise(GL_INVALID_VALUE);
        return;
    }
    let pl = ptr!(c, length, 4, true);
    let ps = ptr!(c, size, 4, true);
    let pk = ptr!(c, kind, 4, true);
    let pn = ptr!(c, name, bufsize, true);
    if uniform {
        s.api.glGetActiveUniform(program, index, bufsize as i32, pl, ps, pk, pn);
    } else {
        s.api.glGetActiveAttrib(program, index, bufsize as i32, pl, ps, pk, pn);
    }
}
fn get_active_attrib(c: &mut Cpu, g: &Gles) {
    active_var(c, g, false)
}
fn get_active_uniform(c: &mut Cpu, g: &Gles) {
    active_var(c, g, true)
}

fn get_attached_shaders(c: &mut Cpu, g: &Gles) {
    args!(c; program, max, count, shaders);
    let mut s = g.lock();
    if (max as i32) < 0 {
        s.raise(GL_INVALID_VALUE);
        return;
    }
    let pc = ptr!(c, count, 4, true);
    let ps = ptr!(c, shaders, max * 4, true);
    s.api.glGetAttachedShaders(program, max as i32, pc, ps);
}

fn get_shader_precision_format(c: &mut Cpu, g: &Gles) {
    args!(c; shader, precision, range, bits);
    let mut s = g.lock();
    let pr = ptr!(c, range, 8, true);
    let pb = ptr!(c, bits, 4, true);
    s.api.glGetShaderPrecisionFormat(shader, precision, pr, pb);
}

// ---------- state queries ----------

fn get_integerv(c: &mut Cpu, g: &Gles) {
    args!(c; pname, out);
    let mut s = g.lock();
    let forced: Option<&[i32]> = match pname {
        GL_NUM_SHADER_BINARY_FORMATS | GL_NUM_PROGRAM_BINARY_FORMATS_OES => Some(&[0]),
        GL_SHADER_BINARY_FORMATS | GL_PROGRAM_BINARY_FORMATS_OES => Some(&[]),
        _ => None,
    };
    if let Some(v) = forced {
        let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        guest_write(c, out, &bytes);
        return;
    }
    let n = if pname == GL_COMPRESSED_TEXTURE_FORMATS {
        let mut k = 0i32;
        s.api.glGetIntegerv(GL_NUM_COMPRESSED_TEXTURE_FORMATS, (&mut k as *mut i32).cast());
        k.max(0) as u32
    } else {
        query_count(pname)
    };
    let p = ptr!(c, out, n * 4, true);
    s.api.glGetIntegerv(pname, p);
}

fn get_floatv(c: &mut Cpu, g: &Gles) {
    args!(c; pname, out);
    let mut s = g.lock();
    let p = ptr!(c, out, query_count(pname) * 4, true);
    s.api.glGetFloatv(pname, p);
}

fn get_booleanv(c: &mut Cpu, g: &Gles) {
    args!(c; pname, out);
    let mut s = g.lock();
    let p = ptr!(c, out, query_count(pname), true);
    s.api.glGetBooleanv(pname, p);
}

/// `(a, b, *out)` queries that write one value.
macro_rules! get2 {
    ($(($fname:ident, $m:ident)),* $(,)?) => {$(
        fn $fname(c: &mut Cpu, g: &Gles) {
            args!(c; a, b, out);
            let mut s = g.lock();
            let p = ptr!(c, out, 4, true);
            s.api.$m(a, b, p);
        }
    )*};
}
get2!(
    (get_program_iv, glGetProgramiv),
    (get_shader_iv, glGetShaderiv),
    (get_renderbuffer_parameteriv, glGetRenderbufferParameteriv),
    (get_tex_parameterfv, glGetTexParameterfv),
    (get_tex_parameteriv, glGetTexParameteriv),
);

fn get_buffer_parameteriv(c: &mut Cpu, g: &Gles) {
    args!(c; target, pname, out);
    let mut s = g.lock();
    let p = ptr!(c, out, 4, true);
    if pname == GL_BUFFER_MAPPED_OES {
        let mapped = s.maps.contains_key(&s.bound_buffer(target)) as u32;
        drop(s);
        guest_write(c, out, &mapped.to_le_bytes());
        return;
    }
    s.api.glGetBufferParameteriv(target, pname, p);
}

fn get_framebuffer_attachment_parameteriv(c: &mut Cpu, g: &Gles) {
    args!(c; target, attachment, pname, out);
    let mut s = g.lock();
    let p = ptr!(c, out, 4, true);
    s.api.glGetFramebufferAttachmentParameteriv(target, attachment, pname, p);
}

fn get_uniform_v(c: &mut Cpu, g: &Gles, float: bool) {
    args!(c; program, loc, out);
    let mut s = g.lock();
    let p = ptr!(c, out, 4, true);
    if float {
        s.api.glGetUniformfv(program, loc as i32, p);
    } else {
        s.api.glGetUniformiv(program, loc as i32, p);
    }
}
fn get_uniformfv(c: &mut Cpu, g: &Gles) {
    get_uniform_v(c, g, true)
}
fn get_uniformiv(c: &mut Cpu, g: &Gles) {
    get_uniform_v(c, g, false)
}

fn get_vertex_attrib_v(c: &mut Cpu, g: &Gles, float: bool) {
    args!(c; index, pname, out);
    let mut s = g.lock();
    let p = ptr!(c, out, if pname == GL_CURRENT_VERTEX_ATTRIB { 16 } else { 4 }, true);
    if float {
        s.api.glGetVertexAttribfv(index, pname, p);
    } else {
        s.api.glGetVertexAttribiv(index, pname, p);
    }
}
fn get_vertex_attribfv(c: &mut Cpu, g: &Gles) {
    get_vertex_attrib_v(c, g, true)
}
fn get_vertex_attribiv(c: &mut Cpu, g: &Gles) {
    get_vertex_attrib_v(c, g, false)
}

fn get_vertex_attrib_pointerv(c: &mut Cpu, g: &Gles) {
    args!(c; index, pname, out);
    let mut s = g.lock();
    if pname != GL_VERTEX_ATTRIB_ARRAY_POINTER || index >= 16 {
        s.raise(if pname != GL_VERTEX_ATTRIB_ARRAY_POINTER { GL_INVALID_ENUM } else { GL_INVALID_VALUE });
        return;
    }
    let v = s.attrib_ptrs[index as usize];
    drop(s);
    guest_write(c, out, &v.to_le_bytes());
}

/// `glGetPointerv` is an ES1 query; no client state pointers exist on the ES2 path.
fn get_pointerv(c: &mut Cpu, _g: &Gles) {
    args!(c; _pname, out);
    guest_write(c, out, &0u32.to_le_bytes());
}

fn get_string(c: &mut Cpu, g: &Gles) {
    args!(c; name);
    let p = g.lock().gl_string(name);
    c.ret_u32(p);
}

// ---------- uniforms and vertex attributes (vector forms) ----------

macro_rules! uniform_v {
    ($(($fname:ident, $m:ident, $n:expr)),* $(,)?) => {$(
        fn $fname(c: &mut Cpu, g: &Gles) {
            args!(c; loc, count, v);
            let mut s = g.lock();
            if (count as i32) < 0 {
                s.raise(GL_INVALID_VALUE);
                return;
            }
            let p = ptr!(c, v, count.saturating_mul($n * 4), false);
            s.api.$m(loc as i32, count as i32, p as P);
        }
    )*};
}
uniform_v!(
    (uniform1fv, glUniform1fv, 1),
    (uniform2fv, glUniform2fv, 2),
    (uniform3fv, glUniform3fv, 3),
    (uniform4fv, glUniform4fv, 4),
    (uniform1iv, glUniform1iv, 1),
    (uniform2iv, glUniform2iv, 2),
    (uniform3iv, glUniform3iv, 3),
    (uniform4iv, glUniform4iv, 4),
);

macro_rules! uniform_matrix {
    ($(($fname:ident, $m:ident, $n:expr)),* $(,)?) => {$(
        fn $fname(c: &mut Cpu, g: &Gles) {
            args!(c; loc, count, transpose, v);
            let mut s = g.lock();
            if (count as i32) < 0 {
                s.raise(GL_INVALID_VALUE);
                return;
            }
            let p = ptr!(c, v, count.saturating_mul($n * 4), false);
            s.api.$m(loc as i32, count as i32, transpose as u8, p as P);
        }
    )*};
}
uniform_matrix!(
    (uniform_matrix2fv, glUniformMatrix2fv, 4),
    (uniform_matrix3fv, glUniformMatrix3fv, 9),
    (uniform_matrix4fv, glUniformMatrix4fv, 16),
);

macro_rules! attrib_v {
    ($(($fname:ident, $m:ident, $n:expr)),* $(,)?) => {$(
        fn $fname(c: &mut Cpu, g: &Gles) {
            args!(c; index, v);
            let mut s = g.lock();
            let p = ptr!(c, v, $n * 4, false);
            s.api.$m(index, p as P);
        }
    )*};
}
attrib_v!(
    (vertex_attrib1fv, glVertexAttrib1fv, 1),
    (vertex_attrib2fv, glVertexAttrib2fv, 2),
    (vertex_attrib3fv, glVertexAttrib3fv, 3),
    (vertex_attrib4fv, glVertexAttrib4fv, 4),
);

/// Pointer-argument GL functions: `(guest symbol, thunk)`.
pub const TABLE: &[(&str, Handler)] = &[
    ("glGetError", get_error),
    ("glBindBuffer", bind_buffer),
    ("glBufferData", buffer_data),
    ("glBufferSubData", buffer_sub_data),
    ("glDeleteBuffers", delete_buffers),
    ("glGenBuffers", gen_buffers),
    ("glGenFramebuffers", gen_framebuffers),
    ("glGenRenderbuffers", gen_renderbuffers),
    ("glGenTextures", gen_textures),
    ("glDeleteFramebuffers", delete_framebuffers),
    ("glDeleteRenderbuffers", delete_renderbuffers),
    ("glDeleteTextures", delete_textures),
    ("glDrawElements", draw_elements),
    ("glVertexAttribPointer", vertex_attrib_pointer),
    ("glPixelStorei", pixel_storei),
    ("glTexImage2D", tex_image_2d),
    ("glTexSubImage2D", tex_sub_image_2d),
    ("glCompressedTexImage2D", compressed_tex_image_2d),
    ("glCompressedTexSubImage2D", compressed_tex_sub_image_2d),
    ("glReadPixels", read_pixels),
    ("glTexParameterfv", tex_parameterfv),
    ("glTexParameteriv", tex_parameteriv),
    ("glShaderSource", shader_source),
    ("glGetAttribLocation", get_attrib_location),
    ("glGetUniformLocation", get_uniform_location),
    ("glBindAttribLocation", bind_attrib_location),
    ("glGetShaderInfoLog", get_shader_info_log),
    ("glGetProgramInfoLog", get_program_info_log),
    ("glGetShaderSource", get_shader_source),
    ("glGetActiveAttrib", get_active_attrib),
    ("glGetActiveUniform", get_active_uniform),
    ("glGetAttachedShaders", get_attached_shaders),
    ("glGetShaderPrecisionFormat", get_shader_precision_format),
    ("glGetIntegerv", get_integerv),
    ("glGetFloatv", get_floatv),
    ("glGetBooleanv", get_booleanv),
    ("glGetProgramiv", get_program_iv),
    ("glGetShaderiv", get_shader_iv),
    ("glGetRenderbufferParameteriv", get_renderbuffer_parameteriv),
    ("glGetTexParameterfv", get_tex_parameterfv),
    ("glGetTexParameteriv", get_tex_parameteriv),
    ("glGetBufferParameteriv", get_buffer_parameteriv),
    ("glGetFramebufferAttachmentParameteriv", get_framebuffer_attachment_parameteriv),
    ("glGetUniformfv", get_uniformfv),
    ("glGetUniformiv", get_uniformiv),
    ("glGetVertexAttribfv", get_vertex_attribfv),
    ("glGetVertexAttribiv", get_vertex_attribiv),
    ("glGetVertexAttribPointerv", get_vertex_attrib_pointerv),
    ("glGetPointerv", get_pointerv),
    ("glGetString", get_string),
    ("glUniform1fv", uniform1fv),
    ("glUniform2fv", uniform2fv),
    ("glUniform3fv", uniform3fv),
    ("glUniform4fv", uniform4fv),
    ("glUniform1iv", uniform1iv),
    ("glUniform2iv", uniform2iv),
    ("glUniform3iv", uniform3iv),
    ("glUniform4iv", uniform4iv),
    ("glUniformMatrix2fv", uniform_matrix2fv),
    ("glUniformMatrix3fv", uniform_matrix3fv),
    ("glUniformMatrix4fv", uniform_matrix4fv),
    ("glVertexAttrib1fv", vertex_attrib1fv),
    ("glVertexAttrib2fv", vertex_attrib2fv),
    ("glVertexAttrib3fv", vertex_attrib3fv),
    ("glVertexAttrib4fv", vertex_attrib4fv),
];

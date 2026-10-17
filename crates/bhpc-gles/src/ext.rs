//! OES/EXT/IMG extension thunks, fixed-point (`x`) entry points that map onto ES2 and the vertex array
//! object, mapped buffer, 3D texture and multi-draw emulation.

use crate::api::{P, PM};
use crate::consts::*;
use crate::imgsize::image_bytes;
use crate::marshal::{args, fault_read, indices_ptr, ptr};
use crate::state::{guest_write, host_ptr, Gles};
use crate::Handler;
use bhpc_cpu::Cpu;

/// Raise `GL_INVALID_OPERATION` and return if the host lacks `$name`.
macro_rules! need {
    ($s:expr, $name:expr) => {
        if !$s.api.supports($name) {
            $s.raise(GL_INVALID_OPERATION);
            return;
        }
    };
}

// ---------- vertex array objects ----------

fn gen_vertex_arrays(c: &mut Cpu, g: &Gles) {
    args!(c; n, ids);
    let mut s = g.lock();
    need!(s, "glGenVertexArrays");
    if (n as i32) <= 0 {
        return;
    }
    let p = ptr!(c, ids, n * 4, true);
    s.api.glGenVertexArrays(n as i32, p);
}

fn bind_vertex_array(c: &mut Cpu, g: &Gles) {
    args!(c; array);
    let mut s = g.lock();
    need!(s, "glBindVertexArray");
    s.vao = array;
    s.api.glBindVertexArray(array);
}

fn delete_vertex_arrays(c: &mut Cpu, g: &Gles) {
    args!(c; n, ids);
    let mut s = g.lock();
    need!(s, "glDeleteVertexArrays");
    if (n as i32) <= 0 {
        return;
    }
    let p = ptr!(c, ids, n * 4, false);
    let Ok(raw) = c.mem().read_bytes(ids, n * 4) else { return };
    for id in raw.chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])) {
        s.elem.remove(&id);
        if s.vao == id {
            s.vao = 0;
        }
    }
    s.api.glDeleteVertexArrays(n as i32, p as P);
}

// ---------- mapped buffers (a guest shadow flushed on unmap) ----------

fn map_buffer(c: &mut Cpu, g: &Gles) {
    args!(c; target, access);
    let mut s = g.lock();
    let buffer = s.bound_buffer(target);
    if access != GL_WRITE_ONLY_OES {
        s.raise(GL_INVALID_ENUM);
        return c.ret_u32(0);
    }
    if buffer == 0 || s.maps.contains_key(&buffer) {
        s.raise(GL_INVALID_OPERATION);
        return c.ret_u32(0);
    }
    let size = s.buf_size.get(&buffer).copied().unwrap_or(0).max(0) as u32;
    let ptr = s.heap.alloc(size.max(1));
    if ptr == 0 {
        s.raise(GL_OUT_OF_MEMORY);
        return c.ret_u32(0);
    }
    let _ = s.mem.write_bytes(ptr, &vec![0; size.max(1) as usize]);
    s.maps.insert(buffer, crate::state::MapRec { target, ptr, size });
    c.ret_u32(ptr);
}

fn unmap_buffer(c: &mut Cpu, g: &Gles) {
    args!(c; target);
    let mut s = g.lock();
    let buffer = s.bound_buffer(target);
    let Some(m) = s.maps.remove(&buffer) else {
        s.raise(GL_INVALID_OPERATION);
        return c.ret_u32(0);
    };
    if let Some(p) = host_ptr(c, m.ptr, m.size, false) {
        s.api.glBufferSubData(m.target, 0, m.size as isize, p as P);
    }
    s.heap.free(m.ptr);
    c.ret_u32(GL_TRUE);
}

fn get_buffer_pointerv(c: &mut Cpu, g: &Gles) {
    args!(c; target, pname, out);
    let mut s = g.lock();
    if pname != GL_BUFFER_MAP_POINTER_OES {
        s.raise(GL_INVALID_ENUM);
        return;
    }
    let v = s.maps.get(&s.bound_buffer(target)).map_or(0, |m| m.ptr);
    drop(s);
    guest_write(c, out, &v.to_le_bytes());
}

// ---------- 3D textures (ES3 host) ----------

fn tex_image_3d(c: &mut Cpu, g: &Gles) {
    args!(c; target, level, internal, w, h, d, border, format, kind, pixels);
    let mut s = g.lock();
    need!(s, "glTexImage3D");
    let Some(len) = image_bytes(w as i32, h as i32, d as i32, format, kind, s.unpack_align) else {
        s.raise(GL_INVALID_ENUM);
        return;
    };
    let p = ptr!(c, pixels, len, false);
    let (l, i, w, h, d, b) = (level as i32, internal as i32, w as i32, h as i32, d as i32, border as i32);
    s.api.glTexImage3D(target, l, i, w, h, d, b, format, kind, p as P);
}

fn tex_sub_image_3d(c: &mut Cpu, g: &Gles) {
    args!(c; target, level, xo, yo, zo, w, h, d, format, kind, pixels);
    let mut s = g.lock();
    need!(s, "glTexSubImage3D");
    let Some(len) = image_bytes(w as i32, h as i32, d as i32, format, kind, s.unpack_align) else {
        s.raise(GL_INVALID_ENUM);
        return;
    };
    let p = ptr!(c, pixels, len, false);
    let (l, x, y, z) = (level as i32, xo as i32, yo as i32, zo as i32);
    s.api.glTexSubImage3D(target, l, x, y, z, w as i32, h as i32, d as i32, format, kind, p as P);
}

fn copy_tex_sub_image_3d(c: &mut Cpu, g: &Gles) {
    args!(c; target, level, xo, yo, zo, x, y, w, h);
    let mut s = g.lock();
    need!(s, "glCopyTexSubImage3D");
    let v = |a: u32| a as i32;
    s.api.glCopyTexSubImage3D(target, v(level), v(xo), v(yo), v(zo), v(x), v(y), v(w), v(h));
}

fn compressed_tex_image_3d(c: &mut Cpu, g: &Gles) {
    args!(c; target, level, format, w, h, d, border, size, data);
    let mut s = g.lock();
    need!(s, "glCompressedTexImage3D");
    let p = ptr!(c, data, size, false);
    let v = |a: u32| a as i32;
    s.api.glCompressedTexImage3D(target, v(level), format, v(w), v(h), v(d), v(border), v(size), p as P);
}

fn compressed_tex_sub_image_3d(c: &mut Cpu, g: &Gles) {
    args!(c; target, level, xo, yo, zo, w, h, d, format, size, data);
    let mut s = g.lock();
    need!(s, "glCompressedTexSubImage3D");
    let p = ptr!(c, data, size, false);
    let v = |a: u32| a as i32;
    s.api.glCompressedTexSubImage3D(target, v(level), v(xo), v(yo), v(zo), v(w), v(h), v(d), format, v(size), p as P);
}

// ---------- multi draw (looped) ----------

fn read_words(c: &mut Cpu, addr: u32, n: u32) -> Option<Vec<u32>> {
    match c.mem().read_bytes(addr, n * 4) {
        Ok(b) => Some(b.chunks_exact(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect()),
        Err(_) => {
            fault_read(c, addr, n * 4);
            None
        }
    }
}

fn multi_draw_arrays(c: &mut Cpu, g: &Gles) {
    args!(c; mode, first, count, primcount);
    let mut s = g.lock();
    if (primcount as i32) <= 0 {
        return;
    }
    let Some(firsts) = read_words(c, first, primcount) else { return };
    let Some(counts) = read_words(c, count, primcount) else { return };
    for (f, n) in firsts.into_iter().zip(counts) {
        s.api.glDrawArrays(mode, f as i32, n as i32);
    }
}

fn multi_draw_elements(c: &mut Cpu, g: &Gles) {
    args!(c; mode, count, kind, indices, primcount);
    let mut s = g.lock();
    if (primcount as i32) <= 0 {
        return;
    }
    let Some(counts) = read_words(c, count, primcount) else { return };
    let Some(addrs) = read_words(c, indices, primcount) else { return };
    let bound = s.element_buffer() != 0;
    for (n, a) in counts.into_iter().zip(addrs) {
        let Some(p) = indices_ptr(c, bound, a, n.saturating_mul(crate::marshal::index_size(kind))) else { return };
        s.api.glDrawElements(mode, n as i32, kind, p);
    }
}

// ---------- fixed point ----------

macro_rules! fixed1 {
    ($(($fname:ident, $m:ident)),* $(,)?) => {$(
        fn $fname(c: &mut Cpu, g: &Gles) {
            args!(c; v);
            g.lock().api.$m(fixed(v as i32));
        }
    )*};
}
fixed1!((clear_depthx, glClearDepthf), (line_widthx, glLineWidth));

fn clear_colorx(c: &mut Cpu, g: &Gles) {
    args!(c; r, gr, b, a);
    g.lock().api.glClearColor(fixed(r as i32), fixed(gr as i32), fixed(b as i32), fixed(a as i32));
}
fn depth_rangex(c: &mut Cpu, g: &Gles) {
    args!(c; n, f);
    g.lock().api.glDepthRangef(fixed(n as i32), fixed(f as i32));
}
fn polygon_offsetx(c: &mut Cpu, g: &Gles) {
    args!(c; f, u);
    g.lock().api.glPolygonOffset(fixed(f as i32), fixed(u as i32));
}
fn sample_coveragex(c: &mut Cpu, g: &Gles) {
    args!(c; v, invert);
    g.lock().api.glSampleCoverage(fixed(v as i32), invert as u8);
}
fn tex_parameterx(c: &mut Cpu, g: &Gles) {
    args!(c; target, pname, param);
    g.lock().api.glTexParameteri(target, pname, param as i32);
}
fn tex_parameterxv(c: &mut Cpu, g: &Gles) {
    args!(c; target, pname, params);
    let mut s = g.lock();
    let p = ptr!(c, params, 4, false);
    s.api.glTexParameteriv(target, pname, p as P);
}
fn get_tex_parameterxv(c: &mut Cpu, g: &Gles) {
    args!(c; target, pname, out);
    let mut s = g.lock();
    let p = ptr!(c, out, 4, true);
    s.api.glGetTexParameteriv(target, pname, p as PM);
}

// ---------- IMG multisample ----------

fn renderbuffer_storage_multisample(c: &mut Cpu, g: &Gles) {
    args!(c; target, samples, format, w, h);
    let mut s = g.lock();
    if s.api.supports("glRenderbufferStorageMultisample") {
        s.api.glRenderbufferStorageMultisample(target, samples as i32, format, w as i32, h as i32);
    } else {
        s.api.glRenderbufferStorage(target, format, w as i32, h as i32);
    }
}

fn framebuffer_texture_2d_multisample(c: &mut Cpu, g: &Gles) {
    args!(c; target, attachment, textarget, texture, level);
    g.lock().api.glFramebufferTexture2D(target, attachment, textarget, texture, level as i32);
}

/// Extension and fixed-point thunks: `(guest symbol, thunk)`.
pub const TABLE: &[(&str, Handler)] = &[
    ("glGenVertexArrays", gen_vertex_arrays),
    ("glBindVertexArray", bind_vertex_array),
    ("glDeleteVertexArrays", delete_vertex_arrays),
    ("glMapBufferOES", map_buffer),
    ("glUnmapBufferOES", unmap_buffer),
    ("glGetBufferPointervOES", get_buffer_pointerv),
    ("glTexImage3D", tex_image_3d),
    ("glTexSubImage3D", tex_sub_image_3d),
    ("glCopyTexSubImage3D", copy_tex_sub_image_3d),
    ("glCompressedTexImage3D", compressed_tex_image_3d),
    ("glCompressedTexSubImage3D", compressed_tex_sub_image_3d),
    ("glMultiDrawArraysEXT", multi_draw_arrays),
    ("glMultiDrawElementsEXT", multi_draw_elements),
    ("glClearColorx", clear_colorx),
    ("glClearDepthx", clear_depthx),
    ("glDepthRangex", depth_rangex),
    ("glLineWidthx", line_widthx),
    ("glPolygonOffsetx", polygon_offsetx),
    ("glSampleCoveragex", sample_coveragex),
    ("glTexParameterx", tex_parameterx),
    ("glTexParameterxv", tex_parameterxv),
    ("glGetTexParameterxv", get_tex_parameterxv),
    ("glRenderbufferStorageMultisample", renderbuffer_storage_multisample),
    ("glRenderbufferStorageMultisampleIMG", renderbuffer_storage_multisample),
    ("glFramebufferTexture2DMultisampleIMG", framebuffer_texture_2d_multisample),
];

/// Extension names that are the same function as a core one.
pub const ALIASES: &[(&str, &str)] = &[
    ("glBindFramebufferOES", "glBindFramebuffer"),
    ("glBindRenderbufferOES", "glBindRenderbuffer"),
    ("glBindVertexArrayOES", "glBindVertexArray"),
    ("glBlendEquationOES", "glBlendEquation"),
    ("glBlendEquationSeparateOES", "glBlendEquationSeparate"),
    ("glBlendFuncSeparateOES", "glBlendFuncSeparate"),
    ("glCheckFramebufferStatusOES", "glCheckFramebufferStatus"),
    ("glClearColorxOES", "glClearColorx"),
    ("glClearDepthfOES", "glClearDepthf"),
    ("glClearDepthxOES", "glClearDepthx"),
    ("glCompressedTexImage3DOES", "glCompressedTexImage3D"),
    ("glCompressedTexSubImage3DOES", "glCompressedTexSubImage3D"),
    ("glCopyTexSubImage3DOES", "glCopyTexSubImage3D"),
    ("glDeleteFramebuffersOES", "glDeleteFramebuffers"),
    ("glDeleteRenderbuffersOES", "glDeleteRenderbuffers"),
    ("glDeleteVertexArraysOES", "glDeleteVertexArrays"),
    ("glDepthRangefOES", "glDepthRangef"),
    ("glDepthRangexOES", "glDepthRangex"),
    ("glFramebufferRenderbufferOES", "glFramebufferRenderbuffer"),
    ("glFramebufferTexture2DOES", "glFramebufferTexture2D"),
    ("glGenFramebuffersOES", "glGenFramebuffers"),
    ("glGenRenderbuffersOES", "glGenRenderbuffers"),
    ("glGenVertexArraysOES", "glGenVertexArrays"),
    ("glGenerateMipmapOES", "glGenerateMipmap"),
    ("glGetFramebufferAttachmentParameterivOES", "glGetFramebufferAttachmentParameteriv"),
    ("glGetRenderbufferParameterivOES", "glGetRenderbufferParameteriv"),
    ("glGetTexParameterxvOES", "glGetTexParameterxv"),
    ("glIsFramebufferOES", "glIsFramebuffer"),
    ("glIsRenderbufferOES", "glIsRenderbuffer"),
    ("glIsVertexArrayOES", "glIsVertexArray"),
    ("glLineWidthxOES", "glLineWidthx"),
    ("glPolygonOffsetxOES", "glPolygonOffsetx"),
    ("glRenderbufferStorageOES", "glRenderbufferStorage"),
    ("glSampleCoveragexOES", "glSampleCoveragex"),
    ("glTexImage3DOES", "glTexImage3D"),
    ("glTexParameterxOES", "glTexParameterx"),
    ("glTexParameterxvOES", "glTexParameterxv"),
    ("glTexSubImage3DOES", "glTexSubImage3D"),
];

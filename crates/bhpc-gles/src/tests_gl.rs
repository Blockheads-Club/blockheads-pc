//! Marshalling tests: guest registers and memory in, host calls out.

use crate::api::Arg;
use crate::consts::*;
use crate::imgsize::{bytes_per_pixel, image_bytes};
use crate::marshal::{index_size, query_count};
use crate::state::{es2_version, filter_extensions};
use crate::tests_support::*;
use bhpc_cpu::Stop;
use std::sync::{Arc, Mutex};

const S: u32 = SCRATCH;
const BAD: u32 = 0x1000_0000;
const END: u32 = SCRATCH + 0x10_0000;

fn fault(r: Result<bhpc_cpu::CallResult, Stop>) -> bool {
    matches!(r, Err(Stop::Fault(_)))
}

fn err(e: &mut Env) -> u32 {
    e.call("glGetError", &[])
}

#[test]
fn helper_tables() {
    assert_eq!(query_count(GL_VIEWPORT), 4);
    assert_eq!(query_count(GL_DEPTH_RANGE), 2);
    assert_eq!(query_count(0x1234), 1);
    assert_eq!([index_size(GL_UNSIGNED_INT), index_size(GL_UNSIGNED_SHORT), index_size(GL_UNSIGNED_BYTE)], [4, 2, 1]);
    assert_eq!(bytes_per_pixel(GL_RGBA, GL_UNSIGNED_BYTE), Some(4));
    assert_eq!(bytes_per_pixel(GL_RGB, GL_UNSIGNED_SHORT_5_6_5), Some(2));
    assert_eq!(bytes_per_pixel(GL_DEPTH_STENCIL_OES, GL_UNSIGNED_INT_24_8_OES), Some(4));
    assert_eq!(bytes_per_pixel(GL_LUMINANCE_ALPHA, GL_FLOAT), Some(8));
    assert_eq!(bytes_per_pixel(GL_ALPHA, GL_HALF_FLOAT_OES), Some(2));
    assert_eq!(bytes_per_pixel(GL_ALPHA, GL_SHORT), Some(2));
    assert_eq!(bytes_per_pixel(0x1234, GL_UNSIGNED_BYTE), None);
    assert_eq!(bytes_per_pixel(GL_RGB, 0x1234), None);
    assert_eq!(image_bytes(3, 2, 1, GL_RGB, GL_UNSIGNED_BYTE, 4), Some(12 + 9));
    assert_eq!(image_bytes(3, 2, 1, GL_RGB, GL_UNSIGNED_BYTE, 1), Some(18));
    assert_eq!(image_bytes(3, 2, 1, GL_RGB, GL_UNSIGNED_BYTE, 3), Some(21), "odd alignments fall back to 4");
    assert_eq!(image_bytes(2, 2, 2, GL_RGBA, GL_UNSIGNED_BYTE, 4), Some(8 * 3 + 8));
    assert_eq!(image_bytes(0, 5, 1, GL_RGBA, GL_UNSIGNED_BYTE, 4), Some(0));
    assert_eq!(image_bytes(5, 5, 1, 0x1234, GL_UNSIGNED_BYTE, 4), None);
    assert_eq!(image_bytes(65536, 65536, 1, GL_RGBA, GL_UNSIGNED_BYTE, 4), None);
    assert_eq!(fixed(0x18000), 1.5);
}

#[test]
fn strings_helpers() {
    assert_eq!(es2_version("OpenGL ES 3.2 Mesa 24.1.0"), "OpenGL ES 2.0 Mesa 24.1.0");
    assert_eq!(es2_version("OpenGL ES 3.0"), "OpenGL ES 2.0");
    assert_eq!(es2_version("4.6 Core"), "OpenGL ES 2.0");
    let host =
        "GL_OES_depth24 GL_QCOM_tiled GL_IMG_texture_compression_pvrtc GL_OES_vertex_array_object GL_OES_texture_npot";
    assert_eq!(filter_extensions(host, false, false), "GL_OES_depth24 GL_OES_texture_npot");
    assert_eq!(
        filter_extensions(host, true, true),
        "GL_OES_depth24 GL_OES_texture_npot GL_OES_vertex_array_object GL_OES_mapbuffer"
    );
    assert_eq!(filter_extensions("GL_ARB_vertex_array_object", true, false), "GL_OES_vertex_array_object");
    assert_eq!(filter_extensions("GL_OES_depth24", true, false), "GL_OES_depth24", "VAO only if the host has it");
}

// ---------- buffers ----------

#[test]
fn bind_buffer_tracks_targets_per_vao() {
    let mut e = Env::new();
    e.call("glBindBuffer", &[GL_ARRAY_BUFFER, 3]);
    e.call("glBindBuffer", &[GL_ELEMENT_ARRAY_BUFFER, 4]);
    e.call("glBindBuffer", &[0x1234, 9]);
    {
        let s = e.gl.lock();
        assert_eq!((s.array_buffer, s.element_buffer(), s.bound_buffer(GL_ARRAY_BUFFER)), (3, 4, 3));
        assert_eq!(s.bound_buffer(0x1234), 0);
    }
    e.call("glBindVertexArray", &[7]);
    assert_eq!(e.gl.lock().element_buffer(), 0, "element buffers belong to the vertex array");
    e.call("glBindBuffer", &[GL_ELEMENT_ARRAY_BUFFER, 5]);
    e.call("glBindVertexArray", &[0]);
    assert_eq!(e.gl.lock().element_buffer(), 4);
    assert_eq!(e.rec.take().len(), 6);
}

#[test]
fn buffer_data_validates_and_translates_the_pointer() {
    let mut e = Env::new();
    e.call("glBindBuffer", &[GL_ARRAY_BUFFER, 3]);
    e.rec.take();
    e.call("glBufferData", &[GL_ARRAY_BUFFER, 64, S, 0x88E4]);
    e.call("glBufferData", &[GL_ARRAY_BUFFER, 32, 0, 0x88E4]);
    assert_eq!(e.rec.take()[0].1, [i(GL_ARRAY_BUFFER as i64), i(64), Arg::P(e.host(S)), i(0x88E4)]);
    assert_eq!(e.gl.lock().buf_size[&3], 32);
    e.call("glBufferData", &[GL_ARRAY_BUFFER, -1i32 as u32, S, 0]);
    assert_eq!(err(&mut e), GL_INVALID_VALUE);
    assert!(e.rec.names().is_empty());
    assert!(fault(e.try_call("glBufferData", &[GL_ARRAY_BUFFER, 16, BAD, 0])));
    assert!(fault(e.try_call("glBufferData", &[GL_ARRAY_BUFFER, 16, END - 8, 0])), "range must be fully mapped");
    e.call("glBufferData", &[GL_ARRAY_BUFFER, 16, END - 16, 0]);
    e.call("glBindBuffer", &[GL_ARRAY_BUFFER, 0]);
    e.call("glBufferData", &[GL_ARRAY_BUFFER, 8, S, 0]);
    assert_eq!(e.gl.lock().buf_size.len(), 1, "an unbound target records no size");
}

#[test]
fn buffer_sub_data() {
    let mut e = Env::new();
    e.call("glBufferSubData", &[GL_ARRAY_BUFFER, 4, 8, S]);
    assert_eq!(e.rec.take()[0].1, [i(GL_ARRAY_BUFFER as i64), i(4), i(8), Arg::P(e.host(S))]);
    e.call("glBufferSubData", &[GL_ARRAY_BUFFER, 4, -8i32 as u32, S]);
    e.call("glBufferSubData", &[GL_ARRAY_BUFFER, -4i32 as u32, 8, S]);
    assert_eq!(err(&mut e), GL_INVALID_VALUE);
    assert!(fault(e.try_call("glBufferSubData", &[GL_ARRAY_BUFFER, 0, 8, BAD])));
}

#[test]
fn delete_buffers_forgets_state_and_frees_mappings() {
    let mut e = Env::with(|c| c.expose_mapbuffer = true);
    e.put_words(S, &[5, 6]);
    e.call("glBindBuffer", &[GL_ARRAY_BUFFER, 5]);
    e.call("glBufferData", &[GL_ARRAY_BUFFER, 16, 0, 0]);
    e.call("glMapBufferOES", &[GL_ARRAY_BUFFER, GL_WRITE_ONLY_OES]);
    e.call("glBindBuffer", &[GL_ELEMENT_ARRAY_BUFFER, 6]);
    assert_eq!(e.gl.lock().maps.len(), 1);
    e.rec.take();
    e.call("glDeleteBuffers", &[2, S]);
    {
        let s = e.gl.lock();
        assert_eq!((s.array_buffer, s.element_buffer(), s.maps.len(), s.buf_size.len()), (0, 0, 0, 0));
    }
    assert_eq!(e.rec.take()[0].1, [i(2), Arg::P(e.host(S))]);
    e.call("glDeleteBuffers", &[0, S]);
    e.call("glDeleteBuffers", &[-1i32 as u32, S]);
    assert_eq!(err(&mut e), GL_INVALID_VALUE);
    assert!(e.rec.names().is_empty());
    assert!(fault(e.try_call("glDeleteBuffers", &[1, BAD])));
}

#[test]
fn gen_and_delete_name_lists() {
    for (name, api) in [
        ("glGenBuffers", "glGenBuffers"),
        ("glGenFramebuffers", "glGenFramebuffers"),
        ("glGenRenderbuffers", "glGenRenderbuffers"),
        ("glGenTextures", "glGenTextures"),
        ("glDeleteFramebuffers", "glDeleteFramebuffers"),
        ("glDeleteRenderbuffers", "glDeleteRenderbuffers"),
        ("glDeleteTextures", "glDeleteTextures"),
        ("glGenFramebuffersOES", "glGenFramebuffers"),
    ] {
        let mut e = Env::new();
        e.call(name, &[3, S]);
        assert_eq!(e.rec.take(), [(api.to_string(), vec![i(3), Arg::P(e.host(S))])], "{name}");
        e.call(name, &[0, S]);
        e.call(name, &[-2i32 as u32, S]);
        assert!(e.rec.names().is_empty());
        assert_eq!(err(&mut e), GL_INVALID_VALUE);
        assert!(fault(e.try_call(name, &[1, BAD])), "{name}");
    }
}

// ---------- drawing ----------

#[test]
fn draw_elements_client_memory_and_buffer_offsets() {
    let mut e = Env::new();
    e.call("glDrawElements", &[4, 6, GL_UNSIGNED_SHORT, S]);
    assert_eq!(e.rec.take()[0].1, [i(4), i(6), i(GL_UNSIGNED_SHORT as i64), Arg::P(e.host(S))]);
    assert!(fault(e.try_call("glDrawElements", &[4, 6, GL_UNSIGNED_INT, END - 23])), "6 u32 indices need 24 bytes");
    e.call("glDrawElements", &[4, 6, GL_UNSIGNED_INT, END - 24]);
    e.call("glDrawElements", &[4, -1i32 as u32, GL_UNSIGNED_BYTE, S]);
    assert_eq!(err(&mut e), GL_INVALID_VALUE);
    e.rec.take();
    e.call("glBindBuffer", &[GL_ELEMENT_ARRAY_BUFFER, 2]);
    e.call("glDrawElements", &[4, 6, GL_UNSIGNED_SHORT, 0x40]);
    assert_eq!(e.rec.take()[1].1[3], Arg::P(0x40), "with an element buffer the pointer is an offset");
    e.call("glDrawElements", &[4, 6, GL_UNSIGNED_SHORT, BAD]);
}

#[test]
fn vertex_attrib_pointer_client_arrays_and_vbos() {
    let mut e = Env::new();
    e.call("glVertexAttribPointer", &[2, 3, GL_FLOAT, 0, 20, S + 8]);
    assert_eq!(e.rec.take()[0].1, [i(2), i(3), i(GL_FLOAT as i64), i(0), i(20), Arg::P(e.host(S + 8))]);
    e.call("glVertexAttribPointer", &[2, 3, GL_FLOAT, 0, 20, 0]);
    assert_eq!(e.rec.take()[0].1[5], Arg::P(0));
    e.call("glVertexAttribPointer", &[99, 4, GL_FLOAT, 1, 0, S]);
    assert_eq!(e.gl.lock().attrib_ptrs[2], 0);
    assert!(fault(e.try_call("glVertexAttribPointer", &[0, 4, GL_FLOAT, 1, 0, BAD])));
    e.call("glBindBuffer", &[GL_ARRAY_BUFFER, 1]);
    e.rec.take();
    e.call("glVertexAttribPointer", &[1, 2, GL_FLOAT, 0, 8, 12]);
    assert_eq!(e.rec.take()[0].1[5], Arg::P(12));
    assert_eq!(e.gl.lock().attrib_ptrs[1], 12);
    // glGetVertexAttribPointerv returns what the guest passed.
    e.call("glGetVertexAttribPointerv", &[1, GL_VERTEX_ATTRIB_ARRAY_POINTER, S + 64]);
    assert_eq!(e.word(S + 64), 12);
    e.call("glGetVertexAttribPointerv", &[1, 0x1234, S + 64]);
    assert_eq!(err(&mut e), GL_INVALID_ENUM);
    e.call("glGetVertexAttribPointerv", &[16, GL_VERTEX_ATTRIB_ARRAY_POINTER, S + 64]);
    assert_eq!(err(&mut e), GL_INVALID_VALUE);
    assert!(fault(e.try_call("glGetVertexAttribPointerv", &[1, GL_VERTEX_ATTRIB_ARRAY_POINTER, BAD])));
}

// ---------- textures ----------

#[test]
fn pixel_store_and_tex_images() {
    let mut e = Env::new();
    e.call("glPixelStorei", &[GL_UNPACK_ALIGNMENT, 1]);
    e.call("glPixelStorei", &[GL_PACK_ALIGNMENT, 8]);
    e.call("glPixelStorei", &[GL_UNPACK_ALIGNMENT, 3]);
    e.call("glPixelStorei", &[0x1234, 2]);
    let (ua, pa) = {
        let s = e.gl.lock();
        (s.unpack_align, s.pack_align)
    };
    assert_eq!((ua, pa), (1, 8));
    assert_eq!(e.rec.take().len(), 4);

    // 3x2 RGB with alignment 1 = 18 bytes: the last byte of the mapping is enough.
    let tex = [GL_TEXTURE_2D, 0, GL_RGB, 3, 2, 0, GL_RGB, GL_UNSIGNED_BYTE, END - 18];
    e.call("glTexImage2D", &tex);
    assert_eq!(
        e.rec.take()[0].1,
        [i(0xDE1), i(0), i(GL_RGB as i64), i(3), i(2), i(0), i(GL_RGB as i64), i(5121), Arg::P(e.host(END - 18))]
    );
    let mut short = tex;
    short[8] = END - 17;
    assert!(fault(e.try_call("glTexImage2D", &short)));
    short[8] = 0;
    e.call("glTexImage2D", &short);
    assert_eq!(e.rec.take()[0].1[8], Arg::P(0), "null pixels allocate storage only");
    short[6] = 0x1234;
    e.call("glTexImage2D", &short);
    assert_eq!(err(&mut e), GL_INVALID_ENUM);

    let sub = [GL_TEXTURE_2D, 1, 2, 3, 4, 5, GL_RGBA, GL_UNSIGNED_BYTE, S];
    e.call("glTexSubImage2D", &sub);
    assert_eq!(e.rec.take()[0].1[..5], [i(0xDE1), i(1), i(2), i(3), i(4)]);
    let mut bad = sub;
    bad[6] = 0x1234;
    e.call("glTexSubImage2D", &bad);
    assert_eq!(err(&mut e), GL_INVALID_ENUM);
    bad = sub;
    bad[8] = BAD;
    assert!(fault(e.try_call("glTexSubImage2D", &bad)));
}

const GL_TEXTURE_2D: u32 = 0xDE1;

#[test]
fn compressed_textures_and_read_pixels() {
    let mut e = Env::new();
    e.call("glCompressedTexImage2D", &[GL_TEXTURE_2D, 0, 0x8D64, 4, 4, 0, 8, S]);
    assert_eq!(e.rec.take()[0].1[6..], [i(8), Arg::P(e.host(S))]);
    e.call("glCompressedTexImage2D", &[GL_TEXTURE_2D, 0, 0x8D64, 4, 4, 0, -8i32 as u32, S]);
    assert!(fault(e.try_call("glCompressedTexImage2D", &[GL_TEXTURE_2D, 0, 0x8D64, 4, 4, 0, 8, BAD])));
    e.call("glCompressedTexSubImage2D", &[GL_TEXTURE_2D, 0, 0, 0, 4, 4, 0x8D64, 8, S]);
    assert_eq!(e.rec.take()[0].1[7..], [i(8), Arg::P(e.host(S))]);
    e.call("glCompressedTexSubImage2D", &[GL_TEXTURE_2D, 0, 0, 0, 4, 4, 0x8D64, -8i32 as u32, S]);
    assert_eq!(err(&mut e), GL_INVALID_VALUE);
    assert!(fault(e.try_call("glCompressedTexSubImage2D", &[GL_TEXTURE_2D, 0, 0, 0, 4, 4, 0x8D64, 8, BAD])));

    e.call("glPixelStorei", &[GL_PACK_ALIGNMENT, 1]);
    e.call("glReadPixels", &[0, 0, 2, 2, GL_RGBA, GL_UNSIGNED_BYTE, S]);
    assert_eq!(e.rec.take().last().unwrap().1[6], Arg::P(e.host(S)));
    e.call("glReadPixels", &[0, 0, 2, 2, 0x1234, GL_UNSIGNED_BYTE, S]);
    assert_eq!(err(&mut e), GL_INVALID_ENUM);
    assert!(fault(e.try_call("glReadPixels", &[0, 0, 2, 2, GL_RGBA, GL_UNSIGNED_BYTE, END - 15])));
}

#[test]
fn tex_parameter_vectors() {
    for (name, api) in [("glTexParameterfv", "glTexParameterfv"), ("glTexParameteriv", "glTexParameteriv")] {
        let mut e = Env::new();
        e.call(name, &[GL_TEXTURE_2D, 0x2801, S]);
        assert_eq!(e.rec.take()[0], (api.to_string(), vec![i(0xDE1), i(0x2801), Arg::P(e.host(S))]));
        assert!(fault(e.try_call(name, &[GL_TEXTURE_2D, 0x2801, BAD])));
    }
}

// ---------- shaders ----------

#[test]
fn shader_source_copies_strings() {
    let mut e = Env::new();
    e.put(S + 0x100, b"void main(){}\0");
    e.put(S + 0x200, b"abcdef");
    e.put_words(S, &[S + 0x100, S + 0x200]);
    e.put_words(S + 0x20, &[-1i32 as u32, 3]);
    let seen = Arc::new(Mutex::new(Vec::<(Vec<u8>, i32)>::new()));
    let s2 = seen.clone();
    e.rec.on_call(move |name, args| {
        if name != "glShaderSource" {
            return;
        }
        let (Arg::I(count), Arg::P(strs), Arg::P(lens)) = (args[1], args[2], args[3]) else { panic!() };
        for k in 0..count as usize {
            // SAFETY: the thunk passes arrays of `count` valid entries that live during the call.
            let (sp, len) = unsafe { (*(strs as *const *const u8).add(k), *(lens as *const i32).add(k)) };
            let bytes = unsafe { std::slice::from_raw_parts(sp, len as usize) }.to_vec();
            s2.lock().unwrap().push((bytes, len));
        }
    });
    e.call("glShaderSource", &[9, 2, S, S + 0x20]);
    assert_eq!(*seen.lock().unwrap(), [(b"void main(){}".to_vec(), 13), (b"abc".to_vec(), 3)]);
    seen.lock().unwrap().clear();
    e.call("glShaderSource", &[9, 1, S, 0]);
    assert_eq!(seen.lock().unwrap()[0].1, 13, "NULL lengths means NUL-terminated strings");
    e.call("glShaderSource", &[9, 0, S, 0]);
    e.call("glShaderSource", &[9, 5000, S, 0]);
    e.call("glShaderSource", &[9, -1i32 as u32, S, 0]);
    assert_eq!(err(&mut e), GL_INVALID_VALUE);
    assert!(fault(e.try_call("glShaderSource", &[9, 1, BAD, 0])), "string table");
    assert!(fault(e.try_call("glShaderSource", &[9, 1, S, BAD])), "length table");
    e.put_words(S + 0x40, &[BAD]);
    assert!(fault(e.try_call("glShaderSource", &[9, 1, S + 0x40, 0])), "string pointer");
    e.put_words(S + 0x50, &[2]);
    assert!(fault(e.try_call("glShaderSource", &[9, 1, S + 0x40, S + 0x50])), "explicit length over unmapped memory");
}

#[test]
fn names_are_copied_and_may_span_pages() {
    let mut e = Env::new();
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let s2 = seen.clone();
    e.rec.on_call(move |name, args| {
        let ptr = match (name, args) {
            ("glGetAttribLocation" | "glGetUniformLocation", [_, Arg::P(n)]) => *n,
            ("glBindAttribLocation", [_, _, Arg::P(n)]) => *n,
            _ => return,
        };
        // SAFETY: a NUL terminated copy that lives during the call.
        s2.lock().unwrap().push(unsafe { std::ffi::CStr::from_ptr(ptr as *const _) }.to_string_lossy().into_owned());
    });
    e.put(S + 8, b"u_matrix\0");
    e.rec.set_ret("glGetAttribLocation", 3);
    assert_eq!(e.call("glGetAttribLocation", &[4, S + 8]), 3);
    e.rec.set_ret("glGetUniformLocation", -1);
    assert_eq!(e.call("glGetUniformLocation", &[4, S + 8]) as i32, -1);
    e.call("glBindAttribLocation", &[4, 1, S + 8]);
    let mut long = vec![b'a'; 5000];
    long.push(0);
    e.put(S + 0x1000 + 100, &long);
    e.call("glBindAttribLocation", &[4, 1, S + 0x1000 + 100]);
    let seen = seen.lock().unwrap();
    assert_eq!(seen[..3], ["u_matrix", "u_matrix", "u_matrix"]);
    assert_eq!(seen[3].len(), 5000);
    for n in ["glGetAttribLocation", "glGetUniformLocation"] {
        assert!(fault(e.try_call(n, &[4, BAD])));
    }
    assert!(fault(e.try_call("glBindAttribLocation", &[4, 1, BAD])));
}

#[test]
fn info_logs_and_active_variables() {
    for n in ["glGetShaderInfoLog", "glGetProgramInfoLog", "glGetShaderSource"] {
        let mut e = Env::new();
        e.call(n, &[5, 64, S, S + 16]);
        assert_eq!(e.rec.take()[0].1, [i(5), i(64), Arg::P(e.host(S)), Arg::P(e.host(S + 16))]);
        e.call(n, &[5, -1i32 as u32, S, S + 16]);
        assert_eq!(err(&mut e), GL_INVALID_VALUE);
        assert!(fault(e.try_call(n, &[5, 64, BAD, S])));
        assert!(fault(e.try_call(n, &[5, 64, S, BAD])));
    }
    for n in ["glGetActiveAttrib", "glGetActiveUniform"] {
        let mut e = Env::new();
        e.call(n, &[5, 1, 32, S, S + 4, S + 8, S + 16]);
        let c = e.rec.take();
        assert_eq!(
            c[0].1[2..],
            [i(32), Arg::P(e.host(S)), Arg::P(e.host(S + 4)), Arg::P(e.host(S + 8)), Arg::P(e.host(S + 16))]
        );
        e.call(n, &[5, 1, -1i32 as u32, S, S, S, S]);
        assert_eq!(err(&mut e), GL_INVALID_VALUE);
        for k in 0..4 {
            let mut a = [5, 1, 32, S, S + 4, S + 8, S + 16];
            a[3 + k] = BAD;
            assert!(fault(e.try_call(n, &a)), "{n} {k}");
        }
    }
    let mut e = Env::new();
    e.call("glGetAttachedShaders", &[5, 4, S, S + 8]);
    assert_eq!(e.rec.take()[0].1[1..], [i(4), Arg::P(e.host(S)), Arg::P(e.host(S + 8))]);
    e.call("glGetAttachedShaders", &[5, -1i32 as u32, S, S + 8]);
    assert_eq!(err(&mut e), GL_INVALID_VALUE);
    assert!(fault(e.try_call("glGetAttachedShaders", &[5, 4, BAD, S])));
    assert!(fault(e.try_call("glGetAttachedShaders", &[5, 4, S, BAD])));
    e.call("glGetShaderPrecisionFormat", &[0x8B30, 0x8DF2, S, S + 8]);
    assert_eq!(e.rec.take().len(), 1);
    assert!(fault(e.try_call("glGetShaderPrecisionFormat", &[0x8B30, 0x8DF2, BAD, S])));
    assert!(fault(e.try_call("glGetShaderPrecisionFormat", &[0x8B30, 0x8DF2, S, BAD])));
}

// ---------- queries ----------

#[test]
fn get_integerv_counts_overrides_and_compressed_formats() {
    let mut e = Env::new();
    e.call("glGetIntegerv", &[GL_VIEWPORT, END - 16]);
    assert_eq!(e.rec.take()[0].1[1], Arg::P(e.host(END - 16)));
    assert!(fault(e.try_call("glGetIntegerv", &[GL_VIEWPORT, END - 15])));
    e.call("glGetIntegerv", &[0x0D33, S]);
    // Overrides: no binary formats, so the game always uploads sources.
    e.put_words(S, &[7, 7]);
    e.call("glGetIntegerv", &[GL_NUM_SHADER_BINARY_FORMATS, S]);
    e.call("glGetIntegerv", &[GL_NUM_PROGRAM_BINARY_FORMATS_OES, S + 4]);
    assert_eq!((e.word(S), e.word(S + 4)), (0, 0));
    e.rec.take();
    e.put_words(S, &[7]);
    e.call("glGetIntegerv", &[GL_SHADER_BINARY_FORMATS, S]);
    e.call("glGetIntegerv", &[GL_PROGRAM_BINARY_FORMATS_OES, S]);
    assert_eq!(e.word(S), 7, "the format list is empty");
    assert!(e.rec.names().is_empty());
    assert!(fault(e.try_call("glGetIntegerv", &[GL_NUM_SHADER_BINARY_FORMATS, BAD])));
    // COMPRESSED_TEXTURE_FORMATS asks the driver for the count first.
    let base = e.mem().base() as usize;
    e.rec.on_call(move |n, a| {
        if let ("glGetIntegerv", [Arg::I(0x86A2), Arg::P(out)]) = (n, a) {
            assert!(*out >= base);
            // SAFETY: the thunk passes a pointer to a local i32.
            unsafe { *(*out as *mut i32) = 3 };
        }
    });
    e.call("glGetIntegerv", &[GL_COMPRESSED_TEXTURE_FORMATS, END - 12]);
    assert!(fault(e.try_call("glGetIntegerv", &[GL_COMPRESSED_TEXTURE_FORMATS, END - 11])));
    e.rec.on_call(|_, _| {});
    e.call("glGetIntegerv", &[GL_COMPRESSED_TEXTURE_FORMATS, S]);
}

#[test]
fn get_float_and_boolean() {
    let mut e = Env::new();
    e.call("glGetFloatv", &[GL_ALIASED_LINE_WIDTH_RANGE, END - 8]);
    assert!(fault(e.try_call("glGetFloatv", &[GL_ALIASED_LINE_WIDTH_RANGE, END - 7])));
    e.call("glGetBooleanv", &[GL_COLOR_WRITEMASK, END - 4]);
    assert!(fault(e.try_call("glGetBooleanv", &[GL_COLOR_WRITEMASK, END - 3])));
    assert_eq!(e.rec.take().len(), 2);
}

#[test]
fn single_value_queries() {
    for n in [
        "glGetProgramiv",
        "glGetShaderiv",
        "glGetRenderbufferParameteriv",
        "glGetTexParameterfv",
        "glGetTexParameteriv",
        "glGetRenderbufferParameterivOES",
    ] {
        let mut e = Env::new();
        e.call(n, &[1, 2, S]);
        assert_eq!(e.rec.take()[0].1[2], Arg::P(e.host(S)), "{n}");
        assert!(fault(e.try_call(n, &[1, 2, END - 3])), "{n}");
    }
    let mut e = Env::new();
    e.call("glGetFramebufferAttachmentParameteriv", &[1, 2, 3, S]);
    assert_eq!(e.rec.take()[0].1[3], Arg::P(e.host(S)));
    assert!(fault(e.try_call("glGetFramebufferAttachmentParameteriv", &[1, 2, 3, BAD])));
    for n in ["glGetUniformfv", "glGetUniformiv"] {
        e.call(n, &[4, 5, S]);
        assert_eq!(e.rec.take()[0].1[2], Arg::P(e.host(S)));
        assert!(fault(e.try_call(n, &[4, 5, BAD])));
    }
    for n in ["glGetVertexAttribfv", "glGetVertexAttribiv"] {
        e.call(n, &[1, 0x8622, END - 4]);
        e.call(n, &[1, GL_CURRENT_VERTEX_ATTRIB, END - 16]);
        assert!(fault(e.try_call(n, &[1, GL_CURRENT_VERTEX_ATTRIB, END - 15])));
        assert_eq!(e.rec.take().len(), 2);
    }
}

#[test]
fn buffer_parameters_report_the_shadow_mapping() {
    let mut e = Env::with(|c| c.expose_mapbuffer = true);
    e.call("glBindBuffer", &[GL_ARRAY_BUFFER, 1]);
    e.call("glBufferData", &[GL_ARRAY_BUFFER, 32, 0, 0]);
    e.rec.take();
    e.call("glGetBufferParameteriv", &[GL_ARRAY_BUFFER, GL_BUFFER_MAPPED_OES, S]);
    assert_eq!(e.word(S), 0);
    e.call("glMapBufferOES", &[GL_ARRAY_BUFFER, GL_WRITE_ONLY_OES]);
    e.call("glGetBufferParameteriv", &[GL_ARRAY_BUFFER, GL_BUFFER_MAPPED_OES, S]);
    assert_eq!(e.word(S), 1);
    assert!(e.rec.names().iter().all(|n| n != "glGetBufferParameteriv"), "answered without the driver");
    e.call("glGetBufferParameteriv", &[GL_ARRAY_BUFFER, 0x8764, S]);
    assert_eq!(e.rec.take().last().unwrap().0, "glGetBufferParameteriv");
    assert!(fault(e.try_call("glGetBufferParameteriv", &[GL_ARRAY_BUFFER, 0x8764, BAD])));
    e.call("glGetPointerv", &[0x808E, S]);
    assert_eq!(e.word(S), 0);
}

fn cstr_ret(e: &Env, name: &str, s: &'static [u8]) {
    assert_eq!(s.last(), Some(&0));
    e.rec.set_ret(name, s.as_ptr() as i64);
}

#[test]
fn get_string_filters_and_caches() {
    let mut e = Env::with(|c| c.expose_mapbuffer = true);
    cstr_ret(&e, "glGetString", b"OpenGL ES 3.2 Mesa 24.1\0");
    let v = e.call("glGetString", &[GL_VERSION]);
    assert_eq!(e.mem().read_cstr(v).unwrap(), "OpenGL ES 2.0 Mesa 24.1");
    assert_eq!(e.call("glGetString", &[GL_VERSION]), v, "cached");
    assert_eq!(e.rec.names().len(), 1);
    let g = e.call("glGetString", &[GL_SHADING_LANGUAGE_VERSION]);
    assert_eq!(e.mem().read_cstr(g).unwrap(), "OpenGL ES GLSL ES 1.00");
    cstr_ret(&e, "glGetString", b"GL_OES_depth24 GL_QCOM_foo GL_OES_texture_npot\0");
    let x = e.call("glGetString", &[GL_EXTENSIONS]);
    assert_eq!(e.mem().read_cstr(x).unwrap(), "GL_OES_depth24 GL_OES_texture_npot GL_OES_mapbuffer");
    cstr_ret(&e, "glGetString", b"Acme\0");
    let vend = e.call("glGetString", &[GL_VENDOR]);
    assert_eq!(e.mem().read_cstr(vend).unwrap(), "Acme");
    assert_ne!(e.call("glGetString", &[GL_RENDERER]), 0);
    assert_eq!(e.call("glGetString", &[0x1234]), 0);
    assert_eq!(err(&mut e), GL_INVALID_ENUM);
}

#[test]
fn get_string_with_a_null_driver_string_is_an_error() {
    let mut e = Env::new();
    assert_eq!(e.call("glGetString", &[GL_VERSION]), 0);
    assert_eq!(err(&mut e), GL_INVALID_ENUM);
    assert_ne!(e.call("glGetString", &[GL_SHADING_LANGUAGE_VERSION]), 0, "fixed, independent of the driver");
}

#[test]
fn get_string_reports_arena_exhaustion() {
    let mut e = Env::with(|c| c.arena_size = 0x1000);
    let long: &'static [u8] = Box::leak([vec![b'x'; 6000], vec![0]].concat().into_boxed_slice());
    cstr_ret(&e, "glGetString", long);
    assert_eq!(e.call("glGetString", &[GL_VENDOR]), 0);
    assert_eq!(err(&mut e), GL_OUT_OF_MEMORY);
}

// ---------- uniforms and attributes ----------

#[test]
fn uniform_vectors_and_matrices() {
    let widths = [
        ("glUniform1fv", 4),
        ("glUniform2fv", 8),
        ("glUniform3fv", 12),
        ("glUniform4fv", 16),
        ("glUniform1iv", 4),
        ("glUniform2iv", 8),
        ("glUniform3iv", 12),
        ("glUniform4iv", 16),
    ];
    for (n, w) in widths {
        let mut e = Env::new();
        e.call(n, &[3, 2, END - 2 * w]);
        assert_eq!(e.rec.take()[0].1, [i(3), i(2), Arg::P(e.host(END - 2 * w))], "{n}");
        assert!(fault(e.try_call(n, &[3, 2, END - 2 * w + 1])), "{n}");
        e.call(n, &[3, -1i32 as u32, S]);
        assert_eq!(err(&mut e), GL_INVALID_VALUE);
        assert!(e.rec.names().is_empty());
    }
    for (n, w) in [("glUniformMatrix2fv", 16), ("glUniformMatrix3fv", 36), ("glUniformMatrix4fv", 64)] {
        let mut e = Env::new();
        e.call(n, &[3, 1, 0, END - w]);
        assert_eq!(e.rec.take()[0].1, [i(3), i(1), i(0), Arg::P(e.host(END - w))], "{n}");
        assert!(fault(e.try_call(n, &[3, 1, 0, END - w + 1])), "{n}");
        e.call(n, &[3, -1i32 as u32, 0, S]);
        assert_eq!(err(&mut e), GL_INVALID_VALUE);
    }
    for (n, w) in
        [("glVertexAttrib1fv", 4), ("glVertexAttrib2fv", 8), ("glVertexAttrib3fv", 12), ("glVertexAttrib4fv", 16)]
    {
        let mut e = Env::new();
        e.call(n, &[2, END - w]);
        assert_eq!(e.rec.take()[0].1, [i(2), Arg::P(e.host(END - w))], "{n}");
        assert!(fault(e.try_call(n, &[2, END - w + 1])), "{n}");
    }
}

// ---------- extensions ----------

#[test]
fn vertex_array_objects() {
    let mut e = Env::new();
    e.call("glGenVertexArraysOES", &[2, S]);
    assert_eq!(e.rec.take()[0], ("glGenVertexArrays".to_string(), vec![i(2), Arg::P(e.host(S))]));
    e.call("glGenVertexArrays", &[0, S]);
    assert!(fault(e.try_call("glGenVertexArrays", &[1, BAD])));
    e.call("glBindVertexArrayOES", &[5]);
    assert_eq!(e.gl.lock().vao, 5);
    e.call("glBindBuffer", &[GL_ELEMENT_ARRAY_BUFFER, 8]);
    e.put_words(S, &[5, 6]);
    e.rec.take();
    e.call("glDeleteVertexArraysOES", &[2, S]);
    let (vao, n) = {
        let s = e.gl.lock();
        (s.vao, s.elem.len())
    };
    assert_eq!((vao, n), (0, 0));
    assert_eq!(e.rec.take()[0].1, [i(2), Arg::P(e.host(S))]);
    e.call("glDeleteVertexArrays", &[7, S]);
    e.call("glDeleteVertexArrays", &[0, S]);
    assert!(fault(e.try_call("glDeleteVertexArrays", &[1, BAD])));
    e.call("glDeleteVertexArrays", &[1, S + 8]);
    assert_eq!(e.gl.lock().vao, 0);
}

#[test]
fn vertex_array_calls_fail_without_host_support() {
    let mut e = Env::new();
    for n in ["glGenVertexArrays", "glBindVertexArray", "glDeleteVertexArrays", "glTexImage3D", "glTexSubImage3D"] {
        e.rec.missing(n);
    }
    for n in ["glCopyTexSubImage3D", "glCompressedTexImage3D", "glCompressedTexSubImage3D"] {
        e.rec.missing(n);
    }
    let zero = [0u32; 11];
    for (n, k) in [
        ("glGenVertexArraysOES", 2),
        ("glBindVertexArrayOES", 1),
        ("glDeleteVertexArraysOES", 2),
        ("glTexImage3DOES", 10),
        ("glTexSubImage3DOES", 11),
        ("glCopyTexSubImage3DOES", 9),
        ("glCompressedTexImage3DOES", 9),
        ("glCompressedTexSubImage3DOES", 11),
    ] {
        e.call(n, &zero[..k]);
        assert_eq!(err(&mut e), GL_INVALID_OPERATION, "{n}");
    }
    assert!(e.rec.names().iter().all(|n| n == "glGetError"));
}

#[test]
fn map_buffer_is_a_flushed_guest_shadow() {
    let mut e = Env::with(|c| c.expose_mapbuffer = true);
    e.call("glBindBuffer", &[GL_ARRAY_BUFFER, 4]);
    e.call("glBufferData", &[GL_ARRAY_BUFFER, 16, 0, 0]);
    e.rec.take();
    assert_eq!(e.call("glMapBufferOES", &[GL_ARRAY_BUFFER, 0x88B8]), 0);
    assert_eq!(err(&mut e), GL_INVALID_ENUM);
    assert_eq!(e.call("glMapBufferOES", &[GL_ELEMENT_ARRAY_BUFFER, GL_WRITE_ONLY_OES]), 0);
    assert_eq!(err(&mut e), GL_INVALID_OPERATION, "no buffer bound");
    let ptr = e.call("glMapBufferOES", &[GL_ARRAY_BUFFER, GL_WRITE_ONLY_OES]);
    assert!((ARENA..ARENA + (4 << 20)).contains(&ptr));
    assert_eq!(e.call("glMapBufferOES", &[GL_ARRAY_BUFFER, GL_WRITE_ONLY_OES]), 0);
    assert_eq!(err(&mut e), GL_INVALID_OPERATION, "already mapped");
    e.put(ptr, &[1, 2, 3, 4]);
    e.call("glGetBufferPointervOES", &[GL_ARRAY_BUFFER, GL_BUFFER_MAP_POINTER_OES, S]);
    assert_eq!(e.word(S), ptr);
    e.call("glGetBufferPointervOES", &[GL_ARRAY_BUFFER, 0x1234, S]);
    assert_eq!(err(&mut e), GL_INVALID_ENUM);
    assert_eq!(e.call("glUnmapBufferOES", &[GL_ARRAY_BUFFER]), 1);
    assert_eq!(
        e.rec.take()[0],
        ("glBufferSubData".to_string(), vec![i(GL_ARRAY_BUFFER as i64), i(0), i(16), Arg::P(e.host(ptr))])
    );
    assert_eq!(e.call("glUnmapBufferOES", &[GL_ARRAY_BUFFER]), 0);
    assert_eq!(err(&mut e), GL_INVALID_OPERATION);
    e.call("glGetBufferPointervOES", &[GL_ARRAY_BUFFER, GL_BUFFER_MAP_POINTER_OES, S]);
    assert_eq!(e.word(S), 0);
}

#[test]
fn map_buffer_out_of_arena_memory() {
    let mut e = Env::with(|c| c.arena_size = 0x1000);
    e.call("glBindBuffer", &[GL_ARRAY_BUFFER, 4]);
    e.call("glBufferData", &[GL_ARRAY_BUFFER, 1 << 20, 0, 0]);
    assert_eq!(e.call("glMapBufferOES", &[GL_ARRAY_BUFFER, GL_WRITE_ONLY_OES]), 0);
    assert_eq!(err(&mut e), GL_OUT_OF_MEMORY);
    // An untracked size maps one byte.
    e.call("glBindBuffer", &[GL_ARRAY_BUFFER, 5]);
    assert_ne!(e.call("glMapBufferOES", &[GL_ARRAY_BUFFER, GL_WRITE_ONLY_OES]), 0);
}

#[test]
fn texture_3d_forwarders() {
    let mut e = Env::new();
    e.call("glTexImage3DOES", &[0x806F, 0, GL_RGBA, 2, 2, 2, 0, GL_RGBA, GL_UNSIGNED_BYTE, S]);
    assert_eq!(e.rec.take()[0].0, "glTexImage3D");
    e.call("glTexImage3D", &[0x806F, 0, GL_RGBA, 2, 2, 2, 0, 0x1234, GL_UNSIGNED_BYTE, S]);
    assert_eq!(err(&mut e), GL_INVALID_ENUM);
    assert!(fault(e.try_call("glTexImage3D", &[0x806F, 0, GL_RGBA, 2, 2, 2, 0, GL_RGBA, GL_UNSIGNED_BYTE, BAD])));
    e.call("glTexSubImage3D", &[0x806F, 0, 0, 0, 0, 2, 2, 2, GL_RGBA, GL_UNSIGNED_BYTE, S]);
    assert_eq!(e.rec.take().last().unwrap().0, "glTexSubImage3D");
    e.call("glTexSubImage3D", &[0x806F, 0, 0, 0, 0, 2, 2, 2, 0x1234, GL_UNSIGNED_BYTE, S]);
    assert_eq!(err(&mut e), GL_INVALID_ENUM);
    assert!(fault(e.try_call("glTexSubImage3D", &[0x806F, 0, 0, 0, 0, 2, 2, 2, GL_RGBA, GL_UNSIGNED_BYTE, BAD])));
    e.call("glCopyTexSubImage3DOES", &[0x806F, 0, 1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(e.rec.take().last().unwrap().1[2..], [i(1), i(2), i(3), i(4), i(5), i(6), i(7)]);
    e.call("glCompressedTexImage3D", &[0x806F, 0, 0x8D64, 4, 4, 1, 0, 8, S]);
    assert_eq!(e.rec.take().last().unwrap().1[7..], [i(8), Arg::P(e.host(S))]);
    assert!(fault(e.try_call("glCompressedTexImage3D", &[0x806F, 0, 0x8D64, 4, 4, 1, 0, 8, BAD])));
    e.call("glCompressedTexSubImage3D", &[0x806F, 0, 0, 0, 0, 4, 4, 1, 0x8D64, 8, S]);
    assert_eq!(e.rec.take().last().unwrap().1[9..], [i(8), Arg::P(e.host(S))]);
    assert!(fault(e.try_call("glCompressedTexSubImage3D", &[0x806F, 0, 0, 0, 0, 4, 4, 1, 0x8D64, 8, BAD])));
}

#[test]
fn multi_draw_is_looped() {
    let mut e = Env::new();
    e.put_words(S, &[0, 10]);
    e.put_words(S + 8, &[3, 6]);
    e.call("glMultiDrawArraysEXT", &[4, S, S + 8, 2]);
    let c = e.rec.take();
    assert_eq!(c.iter().map(|x| x.1.clone()).collect::<Vec<_>>(), [[i(4), i(0), i(3)], [i(4), i(10), i(6)]]);
    e.call("glMultiDrawArraysEXT", &[4, S, S + 8, 0]);
    assert!(fault(e.try_call("glMultiDrawArraysEXT", &[4, BAD, S, 1])));
    assert!(fault(e.try_call("glMultiDrawArraysEXT", &[4, S, BAD, 1])));
    assert!(e.rec.names().is_empty());

    e.put_words(S + 16, &[S + 0x100, S + 0x200]);
    e.call("glMultiDrawElementsEXT", &[4, S + 8, GL_UNSIGNED_SHORT, S + 16, 2]);
    let c = e.rec.take();
    assert_eq!(c[0].1, [i(4), i(3), i(GL_UNSIGNED_SHORT as i64), Arg::P(e.host(S + 0x100))]);
    assert_eq!(c[1].1[3], Arg::P(e.host(S + 0x200)));
    e.call("glMultiDrawElementsEXT", &[4, S + 8, GL_UNSIGNED_SHORT, S + 16, 0]);
    assert!(fault(e.try_call("glMultiDrawElementsEXT", &[4, BAD, GL_UNSIGNED_SHORT, S + 16, 1])));
    assert!(fault(e.try_call("glMultiDrawElementsEXT", &[4, S + 8, GL_UNSIGNED_SHORT, BAD, 1])));
    e.put_words(S + 24, &[BAD]);
    assert!(fault(e.try_call("glMultiDrawElementsEXT", &[4, S + 8, GL_UNSIGNED_SHORT, S + 24, 1])));
    e.rec.take();
    e.call("glBindBuffer", &[GL_ELEMENT_ARRAY_BUFFER, 1]);
    e.put_words(S + 32, &[0x10, 0x20]);
    e.call("glMultiDrawElementsEXT", &[4, S + 8, GL_UNSIGNED_BYTE, S + 32, 2]);
    let c = e.rec.take();
    assert_eq!((c[1].1[3], c[2].1[3]), (Arg::P(0x10), Arg::P(0x20)));
}

#[test]
fn fixed_point_entry_points_convert_to_float() {
    let mut e = Env::new();
    let one_half = 0x8000;
    e.call("glClearColorx", &[one_half, 0x10000, 0, 0x20000]);
    e.call("glClearColorxOES", &[0, 0, 0, 0]);
    e.call("glClearDepthx", &[one_half]);
    e.call("glDepthRangexOES", &[0, 0x10000]);
    e.call("glLineWidthx", &[0x30000]);
    e.call("glPolygonOffsetx", &[0x10000, one_half]);
    e.call("glSampleCoveragex", &[one_half, 1]);
    e.call("glTexParameterx", &[0xDE1, 0x2801, 0x2601]);
    e.call("glTexParameterxv", &[0xDE1, 0x2801, S]);
    e.call("glGetTexParameterxv", &[0xDE1, 0x2801, S]);
    assert!(fault(e.try_call("glTexParameterxv", &[0xDE1, 0x2801, BAD])));
    assert!(fault(e.try_call("glGetTexParameterxvOES", &[0xDE1, 0x2801, BAD])));
    let c = e.rec.take();
    assert_eq!(c[0], ("glClearColor".to_string(), vec![f(0.5), f(1.0), f(0.0), f(2.0)]));
    assert_eq!(c[2], ("glClearDepthf".to_string(), vec![f(0.5)]));
    assert_eq!(c[3].1, [f(0.0), f(1.0)]);
    assert_eq!(c[4], ("glLineWidth".to_string(), vec![f(3.0)]));
    assert_eq!(c[5], ("glPolygonOffset".to_string(), vec![f(1.0), f(0.5)]));
    assert_eq!(c[6], ("glSampleCoverage".to_string(), vec![f(0.5), i(1)]));
    assert_eq!(c[7], ("glTexParameteri".to_string(), vec![i(0xDE1), i(0x2801), i(0x2601)]));
    assert_eq!(c[8].0, "glTexParameteriv");
    assert_eq!(c[9].0, "glGetTexParameteriv");
}

#[test]
fn multisample_img_falls_back_to_plain_storage() {
    let mut e = Env::new();
    e.call("glRenderbufferStorageMultisampleIMG", &[0x8D41, 4, 0x8058, 64, 32]);
    assert_eq!(
        e.rec.take()[0],
        ("glRenderbufferStorageMultisample".to_string(), vec![i(0x8D41), i(4), i(0x8058), i(64), i(32)])
    );
    e.rec.missing("glRenderbufferStorageMultisample");
    e.call("glRenderbufferStorageMultisample", &[0x8D41, 4, 0x8058, 64, 32]);
    assert_eq!(e.rec.take()[0], ("glRenderbufferStorage".to_string(), vec![i(0x8D41), i(0x8058), i(64), i(32)]));
    e.call("glFramebufferTexture2DMultisampleIMG", &[0x8D40, 0x8CE0, 0xDE1, 3, 0, 4]);
    assert_eq!(
        e.rec.take()[0],
        ("glFramebufferTexture2D".to_string(), vec![i(0x8D40), i(0x8CE0), i(0xDE1), i(3), i(0)])
    );
}

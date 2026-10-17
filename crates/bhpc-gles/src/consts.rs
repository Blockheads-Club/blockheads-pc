//! GL and EGL constants used by the HLE layer.
#![allow(dead_code)]

pub const GL_NO_ERROR: u32 = 0;
pub const GL_INVALID_ENUM: u32 = 0x0500;
pub const GL_INVALID_VALUE: u32 = 0x0501;
pub const GL_INVALID_OPERATION: u32 = 0x0502;
pub const GL_OUT_OF_MEMORY: u32 = 0x0505;

pub const GL_TRUE: u32 = 1;

pub const GL_ARRAY_BUFFER: u32 = 0x8892;
pub const GL_ELEMENT_ARRAY_BUFFER: u32 = 0x8893;
pub const GL_WRITE_ONLY_OES: u32 = 0x88B9;
pub const GL_BUFFER_ACCESS_OES: u32 = 0x88BB;
pub const GL_BUFFER_MAPPED_OES: u32 = 0x88BC;
pub const GL_BUFFER_MAP_POINTER_OES: u32 = 0x88BD;

pub const GL_UNPACK_ALIGNMENT: u32 = 0x0CF5;
pub const GL_PACK_ALIGNMENT: u32 = 0x0D05;

pub const GL_BYTE: u32 = 0x1400;
pub const GL_UNSIGNED_BYTE: u32 = 0x1401;
pub const GL_SHORT: u32 = 0x1402;
pub const GL_UNSIGNED_SHORT: u32 = 0x1403;
pub const GL_INT: u32 = 0x1404;
pub const GL_UNSIGNED_INT: u32 = 0x1405;
pub const GL_FLOAT: u32 = 0x1406;
pub const GL_HALF_FLOAT_OES: u32 = 0x8D61;
pub const GL_UNSIGNED_SHORT_4_4_4_4: u32 = 0x8033;
pub const GL_UNSIGNED_SHORT_5_5_5_1: u32 = 0x8034;
pub const GL_UNSIGNED_SHORT_5_6_5: u32 = 0x8363;
pub const GL_UNSIGNED_INT_24_8_OES: u32 = 0x84FA;

pub const GL_DEPTH_COMPONENT: u32 = 0x1902;
pub const GL_ALPHA: u32 = 0x1906;
pub const GL_RGB: u32 = 0x1907;
pub const GL_RGBA: u32 = 0x1908;
pub const GL_LUMINANCE: u32 = 0x1909;
pub const GL_LUMINANCE_ALPHA: u32 = 0x190A;
pub const GL_DEPTH_STENCIL_OES: u32 = 0x84F9;
pub const GL_BGRA_EXT: u32 = 0x80E1;

pub const GL_VENDOR: u32 = 0x1F00;
pub const GL_RENDERER: u32 = 0x1F01;
pub const GL_VERSION: u32 = 0x1F02;
pub const GL_EXTENSIONS: u32 = 0x1F03;
pub const GL_SHADING_LANGUAGE_VERSION: u32 = 0x8B8C;

pub const GL_VIEWPORT: u32 = 0x0BA2;
pub const GL_SCISSOR_BOX: u32 = 0x0C10;
pub const GL_COLOR_CLEAR_VALUE: u32 = 0x0C22;
pub const GL_COLOR_WRITEMASK: u32 = 0x0C23;
pub const GL_MAX_VIEWPORT_DIMS: u32 = 0x0D3A;
pub const GL_DEPTH_RANGE: u32 = 0x0B70;
pub const GL_ALIASED_POINT_SIZE_RANGE: u32 = 0x846D;
pub const GL_ALIASED_LINE_WIDTH_RANGE: u32 = 0x846E;
pub const GL_BLEND_COLOR: u32 = 0x8005;
pub const GL_NUM_COMPRESSED_TEXTURE_FORMATS: u32 = 0x86A2;
pub const GL_COMPRESSED_TEXTURE_FORMATS: u32 = 0x86A3;
pub const GL_NUM_SHADER_BINARY_FORMATS: u32 = 0x8DF9;
pub const GL_SHADER_BINARY_FORMATS: u32 = 0x8DF8;
pub const GL_NUM_PROGRAM_BINARY_FORMATS_OES: u32 = 0x87FE;
pub const GL_PROGRAM_BINARY_FORMATS_OES: u32 = 0x87FF;
pub const GL_CURRENT_VERTEX_ATTRIB: u32 = 0x8626;
pub const GL_VERTEX_ATTRIB_ARRAY_POINTER: u32 = 0x8645;

/// 16.16 fixed point to float.
pub fn fixed(v: i32) -> f32 {
    v as f32 / 65536.0
}

// EGL
pub const EGL_SUCCESS: u32 = 0x3000;
pub const EGL_NOT_INITIALIZED: u32 = 0x3001;
pub const EGL_BAD_ACCESS: u32 = 0x3002;
pub const EGL_BAD_ALLOC: u32 = 0x3003;
pub const EGL_BAD_ATTRIBUTE: u32 = 0x3004;
pub const EGL_BAD_CONFIG: u32 = 0x3005;
pub const EGL_BAD_CONTEXT: u32 = 0x3006;
pub const EGL_BAD_DISPLAY: u32 = 0x3008;
pub const EGL_BAD_MATCH: u32 = 0x3009;
pub const EGL_BAD_NATIVE_WINDOW: u32 = 0x300B;
pub const EGL_BAD_PARAMETER: u32 = 0x300C;
pub const EGL_BAD_SURFACE: u32 = 0x300D;

pub const EGL_BUFFER_SIZE: u32 = 0x3020;
pub const EGL_ALPHA_SIZE: u32 = 0x3021;
pub const EGL_BLUE_SIZE: u32 = 0x3022;
pub const EGL_GREEN_SIZE: u32 = 0x3023;
pub const EGL_RED_SIZE: u32 = 0x3024;
pub const EGL_DEPTH_SIZE: u32 = 0x3025;
pub const EGL_STENCIL_SIZE: u32 = 0x3026;
pub const EGL_CONFIG_CAVEAT: u32 = 0x3027;
pub const EGL_CONFIG_ID: u32 = 0x3028;
pub const EGL_LEVEL: u32 = 0x3029;
pub const EGL_MAX_PBUFFER_HEIGHT: u32 = 0x302A;
pub const EGL_MAX_PBUFFER_PIXELS: u32 = 0x302B;
pub const EGL_MAX_PBUFFER_WIDTH: u32 = 0x302C;
pub const EGL_NATIVE_RENDERABLE: u32 = 0x302D;
pub const EGL_NATIVE_VISUAL_ID: u32 = 0x302E;
pub const EGL_NATIVE_VISUAL_TYPE: u32 = 0x302F;
pub const EGL_SAMPLES: u32 = 0x3031;
pub const EGL_SAMPLE_BUFFERS: u32 = 0x3032;
pub const EGL_SURFACE_TYPE: u32 = 0x3033;
pub const EGL_TRANSPARENT_TYPE: u32 = 0x3034;
pub const EGL_NONE: u32 = 0x3038;
pub const EGL_RENDERABLE_TYPE: u32 = 0x3040;
pub const EGL_HEIGHT: u32 = 0x3056;
pub const EGL_WIDTH: u32 = 0x3057;
pub const EGL_VENDOR: u32 = 0x3053;
pub const EGL_VERSION: u32 = 0x3054;
pub const EGL_EXTENSIONS: u32 = 0x3055;
pub const EGL_CLIENT_APIS: u32 = 0x308D;
pub const EGL_CONTEXT_CLIENT_VERSION: u32 = 0x3098;
pub const EGL_CONTEXT_CLIENT_TYPE: u32 = 0x3097;
pub const EGL_RENDER_BUFFER: u32 = 0x3086;
pub const EGL_BACK_BUFFER: u32 = 0x3084;
pub const EGL_OPENGL_ES_API: u32 = 0x30A0;
pub const EGL_WINDOW_BIT: u32 = 0x0004;
pub const EGL_OPENGL_ES2_BIT: u32 = 0x0004;
pub const EGL_OPENGL_ES_BIT: u32 = 0x0001;
pub const EGL_DONT_CARE: i32 = -1;

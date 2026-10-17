//! Size of client-side pixel data, to validate guest pointers before the host driver reads or writes them.

use crate::consts::*;

/// Bytes per pixel of `format`/`kind`, or `None` for combinations GLES2 does not define.
pub fn bytes_per_pixel(format: u32, kind: u32) -> Option<u32> {
    let comps = match format {
        GL_ALPHA | GL_LUMINANCE | GL_DEPTH_COMPONENT | GL_DEPTH_STENCIL_OES => 1,
        GL_LUMINANCE_ALPHA => 2,
        GL_RGB => 3,
        GL_RGBA | GL_BGRA_EXT => 4,
        _ => return None,
    };
    match kind {
        GL_UNSIGNED_BYTE | GL_BYTE => Some(comps),
        GL_SHORT | GL_UNSIGNED_SHORT | GL_HALF_FLOAT_OES => Some(2 * comps),
        GL_INT | GL_UNSIGNED_INT | GL_FLOAT => Some(4 * comps),
        GL_UNSIGNED_SHORT_4_4_4_4 | GL_UNSIGNED_SHORT_5_5_5_1 | GL_UNSIGNED_SHORT_5_6_5 => Some(2),
        GL_UNSIGNED_INT_24_8_OES => Some(4),
        _ => None,
    }
}

/// Bytes spanned by a `w` x `h` x `depth` image whose rows are padded to `align` (1, 2, 4 or 8).
/// `None` if the format is unknown or the size does not fit 32 bits.
pub fn image_bytes(w: i32, h: i32, depth: i32, format: u32, kind: u32, align: i32) -> Option<u32> {
    let bpp = bytes_per_pixel(format, kind)? as u64;
    if w <= 0 || h <= 0 || depth <= 0 {
        return Some(0);
    }
    let align = if matches!(align, 1 | 2 | 4 | 8) { align as u64 } else { 4 };
    let row = w as u64 * bpp;
    let stride = row.div_ceil(align) * align;
    let slice = stride * (h as u64 - 1) + row;
    let total = stride * h as u64 * (depth as u64 - 1) + slice;
    u32::try_from(total).ok()
}

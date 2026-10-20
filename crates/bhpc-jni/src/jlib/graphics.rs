//! `android.graphics`: the bitmaps and typefaces the game hands to its `com.apportable.ui` views.
//!
//! A `Bitmap` is an object with `width`, `height` and `pixels` (a guest block of `width * height * 4` bytes, RGBA with
//! premultiplied alpha like Android's `ARGB_8888`): the native side fills it through `AndroidBitmap_lockPixels` or
//! `copyPixelsFromBuffer`, and the host draws views from it ([`crate::render`]). A `Typeface` remembers where the font
//! came from (file or asset path, or a family name and style).

use super::{int, object, void};
use crate::value::{ClassId, Obj, Value};
use crate::vm::{Call, Data, JResult, State, Vm};

/// `Typeface` style bits.
pub(crate) const BOLD: i32 = 1;
pub(crate) const ITALIC: i32 = 2;

/// Pixels of a bitmap: `(width, height, guest address)`.
pub(crate) fn bitmap_info(vm: &Vm, b: Obj) -> Option<(u32, u32, u32)> {
    bitmap_info_in(&vm.st(), b)
}

/// [`bitmap_info`] for code that already holds the VM state.
pub(crate) fn bitmap_info_in(st: &State, b: Obj) -> Option<(u32, u32, u32)> {
    let int = |n: &str| st.field_named(b, n).map_or(0, |v| v.as_int() as u32);
    let ptr = int("pixels");
    (ptr != 0).then(|| (int("width"), int("height"), ptr))
}

/// A new transparent bitmap (None when the size is empty, absurd or the guest heap is exhausted).
pub(crate) fn new_bitmap(vm: &Vm, w: i32, h: i32) -> Option<Obj> {
    let bytes = (w as i64).checked_mul(h as i64)?.checked_mul(4).filter(|b| *b > 0 && *b <= 1 << 28)?;
    let ptr = vm.galloc(bytes as u32);
    if ptr == 0 {
        return None;
    }
    let o = vm.alloc_object(vm.find_class("android/graphics/Bitmap").unwrap_or(ClassId(0)));
    vm.set_field(o, "width", Value::Int(w));
    vm.set_field(o, "height", Value::Int(h));
    vm.set_field(o, "pixels", Value::Int(ptr as i32));
    Some(o)
}

/// The longest side a decoded picture keeps (larger ones are scaled down: the game crops a small square of it).
pub(crate) const MAX_PICTURE_SIDE: u32 = 1024;

/// Decode a PNG or JPEG file into premultiplied RGBA: `(width, height, pixels)`, scaled to fit
/// [`MAX_PICTURE_SIDE`].
pub(crate) fn decode_picture(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    let img = if w.max(h) > MAX_PICTURE_SIDE {
        let k = MAX_PICTURE_SIDE as f64 / w.max(h) as f64;
        let size = |v: u32| ((v as f64 * k).round() as u32).max(1);
        img.resize_exact(size(w), size(h), image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut px = rgba.into_raw();
    for p in px.chunks_exact_mut(4) {
        let a = u16::from(p[3]);
        for c in &mut p[..3] {
            *c = ((u16::from(*c) * a + 127) / 255) as u8;
        }
    }
    Some((w, h, px))
}

/// A bitmap of the picture in the host file `path` (`None` when it cannot be read or decoded).
pub(crate) fn bitmap_from_file(vm: &Vm, path: &str) -> Option<Obj> {
    let (w, h, px) = decode_picture(&std::fs::read(path).ok()?)?;
    let b = new_bitmap(vm, w as i32, h as i32)?;
    let (_, _, ptr) = bitmap_info(vm, b)?;
    vm.mem().write_bytes(ptr, &px).ok()?;
    Some(b)
}

/// The decoded view of a typeface object.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Face {
    /// File or asset path (empty for a family).
    pub path: String,
    /// Family name (`sans-serif`, `Helvetica`, ...).
    pub name: String,
    pub style: i32,
}

pub(crate) fn face_of(vm: &Vm, t: Obj) -> Face {
    face_in(&vm.st(), t)
}

/// [`face_of`] for code that already holds the VM state.
pub(crate) fn face_in(st: &State, t: Obj) -> Face {
    let s = |n: &str| {
        let text = |o: Obj| st.obj(o).and_then(|ob| if let Data::Str(u) = &ob.data { Some(u.clone()) } else { None });
        st.field_named(t, n)
            .and_then(|v| v.as_ref())
            .and_then(text)
            .map(|u| String::from_utf16_lossy(&u))
            .unwrap_or_default()
    };
    Face { path: s("path"), name: s("name"), style: st.field_named(t, "style").map_or(0, |v| v.as_int()) }
}

fn new_face(c: &Call, path: Option<String>, name: Option<String>, style: i32) -> JResult {
    let o = c.vm.alloc_object(c.class);
    if let Some(p) = path {
        let s = c.vm.new_string(&p);
        c.vm.set_field(o, "path", Value::Ref(Some(s)));
    }
    if let Some(n) = name {
        let s = c.vm.new_string(&n);
        c.vm.set_field(o, "name", Value::Ref(Some(s)));
    }
    c.vm.set_field(o, "style", Value::Int(style));
    object(Some(o))
}

pub(crate) fn install(vm: &Vm) {
    vm.define("android/graphics/Bitmap")
        .field("width", "I")
        .field("height", "I")
        .field("pixels", "I")
        .static_method("createBitmap", "(IILandroid/graphics/Bitmap$Config;)Landroid/graphics/Bitmap;", |c| {
            object(new_bitmap(c.vm, c.int(0), c.int(1)))
        })
        .method("getWidth", "()I", |c| int(c.vm.get_field(c.this, "width").as_int()))
        .method("getHeight", "()I", |c| int(c.vm.get_field(c.this, "height").as_int()))
        .method("recycle", "()V", |_| void())
        .method("copyPixelsFromBuffer", "(Ljava/nio/Buffer;)V", |c| {
            let (Some(src), Some((w, h, ptr))) = (c.obj(0), bitmap_info(c.vm, c.this)) else { return void() };
            let (addr, cap) = (c.vm.get_field(src, "address").as_long(), c.vm.get_field(src, "capacity").as_long());
            let n = (w as u64 * h as u64 * 4).min(cap.max(0) as u64) as u32;
            if let Ok(bytes) = c.vm.mem().read_bytes(addr as u32, n) {
                let _ = c.vm.mem().write_bytes(ptr, &bytes);
                c.vm.ui_changed();
            }
            void()
        });
    vm.define("android/graphics/BitmapFactory").static_method(
        "decodeFile",
        "(Ljava/lang/String;)Landroid/graphics/Bitmap;",
        |c| object(c.string(0).and_then(|p| bitmap_from_file(c.vm, &p))),
    );
    vm.define("android/graphics/Typeface")
        .field("path", "Ljava/lang/String;")
        .field("name", "Ljava/lang/String;")
        .field("style", "I")
        .static_method("createFromFile", "(Ljava/lang/String;)Landroid/graphics/Typeface;", |c| {
            new_face(c, c.string(0), None, 0)
        })
        .static_method(
            "createFromAsset",
            "(Landroid/content/res/AssetManager;Ljava/lang/String;)Landroid/graphics/Typeface;",
            |c| new_face(c, c.string(1), None, 0),
        )
        .static_method("create", "(Ljava/lang/String;I)Landroid/graphics/Typeface;", |c| {
            new_face(c, None, c.string(0), c.int(1))
        })
        .static_method("create", "(Landroid/graphics/Typeface;I)Landroid/graphics/Typeface;", |c| {
            let base = c.obj(0).map(|t| face_of(c.vm, t)).unwrap_or_default();
            new_face(c, (!base.path.is_empty()).then_some(base.path), Some(base.name), c.int(1))
        });
}

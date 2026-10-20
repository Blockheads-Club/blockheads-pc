//! Bitmaps, typefaces, `libjnigraphics` and the software renderer of the view tree.

use crate::tests::Env;
use crate::{MemAssets, Obj, UiFrame, Value};
use std::sync::Arc;

const UI: &str = "com/apportable/ui";

fn rect(e: &mut Env, r: [f32; 4]) -> Value {
    Value::Ref(Some(e.new_obj("android/graphics/RectF", "(FFFF)V", &r.map(Value::Float))))
}

fn view(e: &mut Env, kind: &str, handle: i32, frame: [f32; 4]) -> Obj {
    let ctx = Value::Ref(Some(e.activity()));
    let rc = rect(e, frame);
    let class = format!("{UI}/{kind}");
    let sig = format!("(Landroid/content/Context;ILandroid/graphics/RectF;)L{class};");
    let r = e.scall(&class, "create", &sig, &[ctx, Value::Int(handle), rc]);
    e.obj(r)
}

fn add(e: &mut Env, parent: Obj, child: Obj) {
    e.call(parent, "addSubview", "(Lcom/apportable/ui/View;)V", &[Value::Ref(Some(child))]).unwrap();
}

fn set(e: &mut Env, o: Obj, name: &str, sig: &str, args: &[Value]) {
    e.call(o, name, sig, args).unwrap();
}

fn bitmap(e: &mut Env, w: i32, h: i32, rgba: [u8; 4]) -> Obj {
    let cfg = Value::Ref(None);
    let r = e.scall(
        "android/graphics/Bitmap",
        "createBitmap",
        "(IILandroid/graphics/Bitmap$Config;)Landroid/graphics/Bitmap;",
        &[Value::Int(w), Value::Int(h), cfg],
    );
    let b = e.obj(r);
    let (_, _, ptr) = crate::jlib::bitmap_info(&e.vm, b).unwrap();
    let px: Vec<u8> = (0..w * h).flat_map(|_| rgba).collect();
    e.mem().write_bytes(ptr, &px).unwrap();
    b
}

fn argb(a: u8, r: u8, g: u8, b: u8) -> i32 {
    u32::from_be_bytes([a, r, g, b]) as i32
}

fn px(f: &UiFrame, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * f.width + x) * 4) as usize;
    f.rgba[i..i + 4].try_into().unwrap()
}

fn lit(f: &UiFrame, x0: u32, y0: u32, x1: u32, y1: u32) -> usize {
    (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| px(f, x, y)[3] != 0).count()
}

/// The test window is 800x480: the portrait space (480x800) is shifted by (160, -160).
const OX: u32 = 160;

/// Window > root (800x480) > container (480x800).
fn scene(e: &mut Env) -> (Obj, Obj) {
    let window = view(e, "Window", 1, [0.0; 4]);
    let root = view(e, "View", 2, [0.0, 0.0, 800.0, 480.0]);
    let container = view(e, "View", 3, [0.0, 0.0, 480.0, 800.0]);
    add(e, window, root);
    add(e, root, container);
    (window, container)
}

/// A view at (0, 300) of the portrait space, so that its own (0, 0) is window row 140 and column `OX`.
fn holder(e: &mut Env, container: Obj) -> Obj {
    let h = view(e, "View", 30, [0.0, 300.0, 480.0, 800.0]);
    add(e, container, h);
    h
}

fn env_with_font() -> Env {
    let font = bhpc_vfs::fonts::bundled(bhpc_vfs::fonts::Style::Regular).to_vec();
    Env::with(|c| {
        c.assets =
            Box::new(MemAssets(vec![("fonts/Face.ttf".into(), font), ("Broken.otf".into(), b"not a font".to_vec())]))
    })
}

fn face(e: &mut Env, path: &str) -> Value {
    let p = e.string(path);
    let r = e.scall(
        "android/graphics/Typeface",
        "createFromAsset",
        "(Landroid/content/res/AssetManager;Ljava/lang/String;)Landroid/graphics/Typeface;",
        &[Value::Ref(None), p],
    );
    Value::Ref(Some(e.obj(r)))
}

fn label(e: &mut Env, parent: Obj, frame: [f32; 4], text: &str) -> Obj {
    let l = view(e, "Label", 10, frame);
    add(e, parent, l);
    let t = e.string(text);
    set(e, l, "setText", "(Ljava/lang/String;)V", &[t]);
    set(e, l, "setTextColor", "(I)V", &[Value::Int(argb(255, 255, 255, 255))]);
    l
}

fn set_font(e: &mut Env, l: Obj, font: Value, size: f32) {
    set(e, l, "setFont", "(Landroid/graphics/Typeface;F)V", &[font, Value::Float(size)]);
}

#[test]
fn bitmaps_hold_pixels_in_guest_memory() {
    let mut e = Env::new();
    let b = bitmap(&mut e, 4, 3, [1, 2, 3, 4]);
    assert_eq!(e.call(b, "getWidth", "()I", &[]).unwrap().as_int(), 4);
    assert_eq!(e.call(b, "getHeight", "()I", &[]).unwrap().as_int(), 3);
    e.call(b, "recycle", "()V", &[]).unwrap();
    let (_, _, ptr) = crate::jlib::bitmap_info(&e.vm, b).unwrap();
    // `copyPixelsFromBuffer` reads a direct buffer; a short buffer fills only the start.
    let src: Vec<u8> = (0..48).collect();
    let addr = e.bytes(&src, 0);
    let buf = e.jni("NewDirectByteBuffer", &[addr, 16, 0]).r0;
    let buf = e.vm.resolve(buf).unwrap();
    e.call(b, "copyPixelsFromBuffer", "(Ljava/nio/Buffer;)V", &[Value::Ref(Some(buf))]).unwrap();
    let px = e.mem().read_bytes(ptr, 48).unwrap();
    assert_eq!(&px[..16], &src[..16]);
    assert_eq!(&px[16..20], &[1, 2, 3, 4]);
    let buf = e.jni("NewDirectByteBuffer", &[addr, 4096, 0]).r0;
    let buf = e.vm.resolve(buf).unwrap();
    e.call(b, "copyPixelsFromBuffer", "(Ljava/nio/Buffer;)V", &[Value::Ref(Some(buf))]).unwrap();
    assert_eq!(e.mem().read_bytes(ptr, 48).unwrap(), src);
    // A null buffer and a buffer with no memory change nothing.
    e.call(b, "copyPixelsFromBuffer", "(Ljava/nio/Buffer;)V", &[Value::Ref(None)]).unwrap();
    let far = e.jni("NewDirectByteBuffer", &[0x1000, 48, 0]).r0;
    let far = e.vm.resolve(far).unwrap();
    e.call(b, "copyPixelsFromBuffer", "(Ljava/nio/Buffer;)V", &[Value::Ref(Some(far))]).unwrap();
    assert_eq!(e.mem().read_bytes(ptr, 48).unwrap(), src);
}

#[test]
fn bad_bitmap_sizes_and_exhausted_memory_give_null() {
    let (mut e, fail) = Env::flaky();
    let sig = "(IILandroid/graphics/Bitmap$Config;)Landroid/graphics/Bitmap;";
    let make = |e: &mut Env, w: i32, h: i32| {
        let args = [Value::Int(w), Value::Int(h), Value::Ref(None)];
        e.scall("android/graphics/Bitmap", "createBitmap", sig, &args).unwrap().as_ref()
    };
    assert!(make(&mut e, 2, 2).is_some());
    for (w, h) in [(0, 5), (-1, 5), (5, 0), (100_000, 100_000), (i32::MAX, i32::MAX)] {
        assert_eq!(make(&mut e, w, h), None, "{w}x{h}");
    }
    fail.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(make(&mut e, 2, 2), None);
}

#[test]
fn typefaces_remember_their_origin() {
    let mut e = Env::new();
    let class = "android/graphics/Typeface";
    let path = e.string("fonts/X.otf");
    let r = e.scall(class, "createFromFile", "(Ljava/lang/String;)Landroid/graphics/Typeface;", &[path]);
    let from_file = e.obj(r);
    let f = crate::jlib::face_of(&e.vm, from_file);
    assert_eq!((f.path.as_str(), f.name.as_str(), f.style), ("fonts/X.otf", "", 0));
    let asset = face(&mut e, "A.otf");
    assert_eq!(crate::jlib::face_of(&e.vm, asset.as_ref().unwrap()).path, "A.otf");
    let name = e.string("Helvetica");
    let r = e.scall(class, "create", "(Ljava/lang/String;I)Landroid/graphics/Typeface;", &[name, Value::Int(3)]);
    let fam = e.obj(r);
    let f = crate::jlib::face_of(&e.vm, fam);
    assert_eq!((f.path.as_str(), f.name.as_str(), f.style), ("", "Helvetica", 3));
    let r = e.scall(
        class,
        "create",
        "(Landroid/graphics/Typeface;I)Landroid/graphics/Typeface;",
        &[Value::Ref(Some(from_file)), Value::Int(1)],
    );
    let derived = crate::jlib::face_of(&e.vm, e.obj(r));
    assert_eq!((derived.path.as_str(), derived.style), ("fonts/X.otf", 1));
    let r = e.scall(
        class,
        "create",
        "(Landroid/graphics/Typeface;I)Landroid/graphics/Typeface;",
        &[Value::Ref(None), Value::Int(2)],
    );
    let plain = crate::jlib::face_of(&e.vm, e.obj(r));
    assert_eq!((plain.path.as_str(), plain.name.as_str(), plain.style), ("", "", 2));
}

#[test]
fn libjnigraphics_works_on_fake_bitmaps() {
    let mut e = Env::new();
    let b = bitmap(&mut e, 5, 2, [9, 9, 9, 9]);
    let h = e.local(b);
    let env = e.env_ptr();
    let call = |e: &mut Env, name: &str, args: &[u32]| {
        let a = e.cpu.thunk_addr(name).unwrap_or_else(|| panic!("no {name}"));
        e.cpu.call(a, args).unwrap().r0
    };
    let out = e.buf(32);
    assert_eq!(call(&mut e, "AndroidBitmap_getInfo", &[env, h, out]), 0);
    let info: Vec<u32> = (0..5).map(|i| e.mem().read_u32(out + 4 * i).unwrap()).collect();
    assert_eq!(info, vec![5, 2, 20, 1, 0]);
    let slot = e.buf(4);
    assert_eq!(call(&mut e, "AndroidBitmap_lockPixels", &[env, h, slot]), 0);
    let (_, _, ptr) = crate::jlib::bitmap_info(&e.vm, b).unwrap();
    assert_eq!(e.mem().read_u32(slot).unwrap(), ptr);
    assert_eq!(call(&mut e, "AndroidBitmap_unlockPixels", &[env, h]), 0);
    // Not a bitmap, a null reference or no output pointer: ANDROID_BITMAP_RESULT_BAD_PARAMETER.
    let other = e.local(e.vm.alloc_object(e.vm.find_class("java/lang/Object").unwrap()));
    for bad in [other, 0] {
        assert_eq!(call(&mut e, "AndroidBitmap_getInfo", &[env, bad, out]), u32::MAX);
        assert_eq!(call(&mut e, "AndroidBitmap_lockPixels", &[env, bad, slot]), u32::MAX);
        assert_eq!(call(&mut e, "AndroidBitmap_unlockPixels", &[env, bad]), u32::MAX);
    }
    assert_eq!(call(&mut e, "AndroidBitmap_getInfo", &[env, h, 0]), u32::MAX);
}

#[test]
fn view_properties_show_in_the_dump_and_survive_gc() {
    let mut e = env_with_font();
    let (_, container) = scene(&mut e);
    let img = view(&mut e, "ImageView", 20, [0.0, 0.0, 10.0, 10.0]);
    let bmp = bitmap(&mut e, 2, 2, [255, 0, 0, 255]);
    set(
        &mut e,
        img,
        "setImage",
        "(Landroid/graphics/Bitmap;II)V",
        &[Value::Ref(Some(bmp)), Value::Int(2), Value::Int(2)],
    );
    add(&mut e, container, img);
    set(&mut e, container, "_setBackgroundColor", "(I)V", &[Value::Int(argb(0x80, 1, 2, 3))]);
    set(&mut e, container, "setClipsToBounds", "(Z)V", &[Value::Int(1)]);
    let btn = view(&mut e, "Button", 21, [0.0, 20.0, 50.0, 40.0]);
    let on = bitmap(&mut e, 2, 2, [0, 255, 0, 255]);
    set(&mut e, btn, "setBackgroundDrawable", "(Landroid/graphics/Bitmap;I)V", &[Value::Ref(Some(on)), Value::Int(0)]);
    set(&mut e, btn, "setIconDrawable", "(Landroid/graphics/Bitmap;I)V", &[Value::Ref(Some(on)), Value::Int(0)]);
    add(&mut e, container, btn);
    let text = label(&mut e, container, [0.0, 50.0, 100.0, 70.0], "Hi");
    let f = face(&mut e, "fonts/Face.ttf");
    set_font(&mut e, text, f, 14.0);
    let d = e.vm.dump_views();
    assert!(d.contains("bg=#80010203 clip"), "{d}");
    assert!(d.contains("img=2x2:4"), "{d}");
    assert!(d.contains("bgimg=2x2:4"), "{d}");
    assert!(d.contains("font=\"fonts/Face.ttf\"/\"\"/0 14pt color=#ffffffff align=0 lines=1 \"Hi\""), "{d}");
    // Replacing and clearing the bitmaps of a button / image.
    set(&mut e, btn, "setBackgroundDrawable", "(Landroid/graphics/Bitmap;I)V", &[Value::Ref(None), Value::Int(0)]);
    set(&mut e, img, "removeImage", "()V", &[]);
    let d = e.vm.dump_views();
    assert!(!d.contains("bgimg") && !d.contains("img=2x2"), "{d}");
    // A bitmap that only a view refers to is not collected.
    let d = e.vm.dump_views();
    e.vm.gc();
    assert_eq!(d, e.vm.dump_views());
    let frame = e.vm.render_ui().unwrap();
    assert!(lit(&frame, 0, 0, 800, 480) > 0);
    // A view whose bitmap has no pixels (not a bitmap) summarises as none.
    assert_eq!(e.vm.bitmap_summary(img), "none");
}

#[test]
fn nothing_visible_means_no_frame() {
    let mut e = Env::new();
    assert!(e.vm.render_ui().is_none(), "no windows");
    let (_, container) = scene(&mut e);
    assert!(e.vm.render_ui().is_none(), "no drawing views");
    let v = view(&mut e, "View", 4, [0.0, 0.0, 50.0, 50.0]);
    add(&mut e, container, v);
    set(&mut e, v, "_setBackgroundColor", "(I)V", &[Value::Int(argb(0, 9, 9, 9))]);
    assert!(e.vm.render_ui().is_none(), "a transparent background draws nothing");
    set(&mut e, v, "_setBackgroundColor", "(I)V", &[Value::Int(argb(255, 9, 9, 9))]);
    assert!(e.vm.render_ui().is_some());
    set(&mut e, v, "setHidden", "(Z)V", &[Value::Int(1)]);
    assert!(e.vm.render_ui().is_none(), "hidden");
    set(&mut e, v, "setHidden", "(Z)V", &[Value::Int(0)]);
    set(&mut e, v, "_setAlpha", "(F)V", &[Value::Float(0.0)]);
    assert!(e.vm.render_ui().is_none(), "fully transparent");
}

#[test]
fn fills_are_placed_clipped_and_blended() {
    let mut e = Env::new();
    let (_, container) = scene(&mut e);
    // A popup at (100, 300) of the portrait space shows at (260, 140) of the window.
    let popup = view(&mut e, "View", 4, [100.0, 300.0, 200.0, 350.0]);
    add(&mut e, container, popup);
    set(&mut e, popup, "_setBackgroundColor", "(I)V", &[Value::Int(argb(255, 200, 100, 50))]);
    let f = e.vm.render_ui().unwrap();
    assert_eq!((f.width, f.height), (800, 480));
    assert_eq!(px(&f, OX + 100, 140), [200, 100, 50, 255]);
    assert_eq!(px(&f, OX + 99, 140), [0; 4]);
    assert_eq!(px(&f, OX + 199, 189), [200, 100, 50, 255]);
    assert_eq!(px(&f, OX + 200, 189), [0; 4]);
    // The same frame again: cached (same serial).
    assert_eq!(e.vm.render_ui().unwrap().serial, f.serial);
    // Half alpha of a child is multiplied by the parent's.
    set(&mut e, popup, "_setAlpha", "(F)V", &[Value::Float(0.5)]);
    let child = view(&mut e, "View", 5, [0.0, 0.0, 10.0, 10.0]);
    add(&mut e, popup, child);
    set(&mut e, child, "_setBackgroundColor", "(I)V", &[Value::Int(argb(255, 255, 255, 255))]);
    set(&mut e, child, "_setAlpha", "(F)V", &[Value::Float(0.5)]);
    let g = e.vm.render_ui().unwrap();
    assert!(g.serial > f.serial);
    let p = px(&g, OX + 100, 140);
    // white at 25 % over the orange at 50 %: alpha .25 + .5 * .75, red 255 * .25 + 200 * .5 * .75
    assert!((158..=162).contains(&p[3]) && (137..=141).contains(&p[0]), "{p:?}");
    let q = px(&g, OX + 150, 140);
    assert!((127..=129).contains(&q[3]), "{q:?}");
    // Clipping to the parent's frame, bounds shifting the children.
    set(&mut e, popup, "_setAlpha", "(F)V", &[Value::Float(1.0)]);
    set(&mut e, popup, "setClipsToBounds", "(Z)V", &[Value::Int(1)]);
    let wide = view(&mut e, "View", 6, [-20.0, 40.0, 400.0, 50.0]);
    add(&mut e, popup, wide);
    set(&mut e, wide, "_setBackgroundColor", "(I)V", &[Value::Int(argb(255, 0, 0, 255))]);
    let b = rect(&mut e, [10.0, 0.0, 110.0, 50.0]);
    set(&mut e, popup, "setBounds", "(Landroid/graphics/RectF;)V", &[b]);
    let h = e.vm.render_ui().unwrap();
    assert_eq!(px(&h, OX + 100, 140 + 40), [0, 0, 255, 255]);
    assert_eq!(px(&h, OX + 99, 140 + 40), [0; 4], "clipped at the popup's left edge");
    assert_eq!(px(&h, OX + 200, 140 + 40), [0; 4], "clipped at its right edge");
}

#[test]
fn a_scrim_over_the_portrait_space_covers_the_window() {
    let mut e = Env::new();
    let (_, container) = scene(&mut e);
    set(&mut e, container, "_setBackgroundColor", "(I)V", &[Value::Int(argb(255, 0, 0, 80))]);
    let f = e.vm.render_ui().unwrap();
    for (x, y) in [(0, 0), (799, 0), (0, 479), (799, 479), (400, 240)] {
        assert_eq!(px(&f, x, y), [0, 0, 80, 255]);
    }
    // A smaller fill is not stretched.
    let small = view(&mut e, "View", 4, [0.0, 160.0, 100.0, 260.0]);
    add(&mut e, container, small);
    set(&mut e, small, "_setBackgroundColor", "(I)V", &[Value::Int(argb(255, 255, 0, 0))]);
    let g = e.vm.render_ui().unwrap();
    assert_eq!(px(&g, 0, 0), [0, 0, 80, 255]);
    assert_eq!(px(&g, OX + 10, 50), [255, 0, 0, 255]);
    assert_eq!(px(&g, OX + 10, 150), [0, 0, 80, 255]);
}

#[test]
fn portrait_windows_need_no_shift() {
    let mut p = crate::tests::test_platform();
    (p.screen_width, p.screen_height) = (480, 800);
    let mut e = Env::with(|c| c.platform = p);
    let window = view(&mut e, "Window", 1, [0.0; 4]);
    let v = view(&mut e, "View", 2, [10.0, 20.0, 30.0, 40.0]);
    add(&mut e, window, v);
    set(&mut e, v, "_setBackgroundColor", "(I)V", &[Value::Int(argb(255, 5, 6, 7))]);
    let f = e.vm.render_ui().unwrap();
    assert_eq!((f.width, f.height), (480, 800));
    assert_eq!(px(&f, 10, 20), [5, 6, 7, 255]);
    assert_eq!(px(&f, 9, 20), [0; 4]);
}

#[test]
fn images_stretch_to_their_frame_and_buttons_pick_a_state() {
    let mut e = Env::new();
    let (_, container) = scene(&mut e);
    let container = holder(&mut e, container);
    let img = view(&mut e, "ImageView", 4, [0.0, 0.0, 8.0, 4.0]);
    add(&mut e, container, img);
    let bmp = bitmap(&mut e, 2, 2, [0, 0, 0, 0]);
    let (_, _, ptr) = crate::jlib::bitmap_info(&e.vm, bmp).unwrap();
    // premultiplied: left column opaque red, right column half transparent green
    let pixels = [255, 0, 0, 255, 0, 128, 0, 128, 255, 0, 0, 255, 0, 128, 0, 128];
    e.mem().write_bytes(ptr, &pixels).unwrap();
    set(
        &mut e,
        img,
        "setImage",
        "(Landroid/graphics/Bitmap;II)V",
        &[Value::Ref(Some(bmp)), Value::Int(2), Value::Int(2)],
    );
    let f = e.vm.render_ui().unwrap();
    let y = 140; // the holder's first row
    assert_eq!(px(&f, OX, y)[..2], [255, 0]);
    assert!(px(&f, OX + 7, y)[1] > 100 && px(&f, OX + 7, y)[3] > 100);
    // 1:1 and shrunk drawing, plus an empty image rectangle and a zero-size bitmap frame.
    let big = bitmap(&mut e, 16, 16, [10, 20, 30, 255]);
    let shrunk = view(&mut e, "ImageView", 5, [0.0, 20.0, 4.0, 24.0]);
    add(&mut e, container, shrunk);
    set(
        &mut e,
        shrunk,
        "setImage",
        "(Landroid/graphics/Bitmap;II)V",
        &[Value::Ref(Some(big)), Value::Int(0), Value::Int(0)],
    );
    let flat = view(&mut e, "ImageView", 6, [0.0, 30.0, 0.0, 30.0]);
    add(&mut e, container, flat);
    set(
        &mut e,
        flat,
        "setImage",
        "(Landroid/graphics/Bitmap;II)V",
        &[Value::Ref(Some(big)), Value::Int(0), Value::Int(0)],
    );
    let f = e.vm.render_ui().unwrap();
    assert_eq!(px(&f, OX + 1, y + 20), [10, 20, 30, 255]);
    assert_eq!(lit(&f, OX, y + 30, OX + 20, y + 40), 0);
    // Button states: normal, highlighted, disabled (each falls back to normal when missing).
    let btn = view(&mut e, "Button", 7, [0.0, 50.0, 10.0, 60.0]);
    add(&mut e, container, btn);
    let colours = [(0, [255u8, 0, 0, 255]), (1, [0, 255, 0, 255]), (2, [0, 0, 255, 255])];
    let state_bg = |e: &mut Env, only: &[i32]| {
        for (s, c) in colours {
            let b = bitmap(e, 1, 1, c);
            let b = if only.contains(&s) { Value::Ref(Some(b)) } else { Value::Ref(None) };
            set(e, btn, "setBackgroundDrawable", "(Landroid/graphics/Bitmap;I)V", &[b, Value::Int(s)]);
        }
    };
    state_bg(&mut e, &[0]);
    set(&mut e, btn, "setHighlighted", "(Z)V", &[Value::Int(1)]);
    assert_eq!(px(&e.vm.render_ui().unwrap(), OX + 5, y + 55)[..3], [255, 0, 0], "highlight falls back to normal");
    state_bg(&mut e, &[0, 1, 2]);
    assert_eq!(px(&e.vm.render_ui().unwrap(), OX + 5, y + 55)[..3], [0, 255, 0]);
    set(&mut e, btn, "setHighlighted", "(Z)V", &[Value::Int(0)]);
    assert_eq!(px(&e.vm.render_ui().unwrap(), OX + 5, y + 55)[..3], [255, 0, 0]);
    set(&mut e, btn, "setEnabled", "(Z)V", &[Value::Int(0)]);
    assert_eq!(px(&e.vm.render_ui().unwrap(), OX + 5, y + 55)[..3], [0, 0, 255]);
}

#[test]
fn text_is_drawn_in_the_game_font_aligned_and_wrapped() {
    let mut e = env_with_font();
    let (_, container) = scene(&mut e);
    let container = holder(&mut e, container);
    let font = face(&mut e, "fonts/Face.ttf");
    let l = label(&mut e, container, [0.0, 100.0, 200.0, 140.0], "Hello");
    set_font(&mut e, l, font, 16.0);
    let y0 = 240;
    let area = |f: &UiFrame, a: u32, b: u32| lit(f, OX + a, y0, OX + b, y0 + 40);
    let left = e.vm.render_ui().unwrap();
    assert!(area(&left, 0, 60) > 50, "left aligned text starts at the left edge");
    assert_eq!(area(&left, 100, 200), 0);
    set(&mut e, l, "setTextAlignment", "(I)V", &[Value::Int(2)]);
    let right = e.vm.render_ui().unwrap();
    assert!(area(&right, 140, 200) > 50 && area(&right, 0, 100) == 0, "right aligned");
    set(&mut e, l, "setTextAlignment", "(I)V", &[Value::Int(1)]);
    let mid = e.vm.render_ui().unwrap();
    assert!(area(&mid, 70, 130) > 50 && area(&mid, 0, 50) == 0 && area(&mid, 150, 200) == 0, "centred");
    // The text is white where the glyphs are solid.
    assert!((0..40).any(|dy| (60..140).any(|dx| px(&mid, OX + dx, y0 + dy) == [255, 255, 255, 255])));
    // Wrapping at spaces and at newlines, limited by the number of lines.
    let t = e.string("one two three four five six seven eight nine ten\nsecond paragraph");
    set(&mut e, l, "setText", "(Ljava/lang/String;)V", &[t]);
    set(&mut e, l, "setNumberOfLines", "(I)V", &[Value::Int(0)]);
    let tall = view(&mut e, "Label", 11, [0.0, 0.0, 0.0, 0.0]);
    add(&mut e, container, tall);
    let rows = |f: &UiFrame| (240..280u32).filter(|dy| lit(f, OX, *dy, OX + 200, *dy + 1) > 0).count();
    let many = e.vm.render_ui().unwrap();
    assert!(rows(&many) > 20, "several lines are drawn: {}", rows(&many));
    set(&mut e, l, "setNumberOfLines", "(I)V", &[Value::Int(1)]);
    let one = e.vm.render_ui().unwrap();
    assert!(rows(&one) < rows(&many) / 2, "one line only: {} vs {}", rows(&one), rows(&many));
    set(&mut e, l, "setNumberOfLines", "(I)V", &[Value::Int(2)]);
    assert!(e.vm.render_ui().unwrap().serial > one.serial);
    // Shadow: drawn first, offset, in its own colour.
    set(&mut e, l, "setNumberOfLines", "(I)V", &[Value::Int(1)]);
    let t = e.string("I");
    set(&mut e, l, "setText", "(Ljava/lang/String;)V", &[t]);
    set(&mut e, l, "setTextAlignment", "(I)V", &[Value::Int(0)]);
    set(&mut e, l, "setShadowColor", "(I)V", &[Value::Int(argb(255, 255, 0, 0))]);
    set(&mut e, l, "setShadowOffset", "(FF)V", &[Value::Float(3.0), Value::Float(3.0)]);
    let shadow = e.vm.render_ui().unwrap();
    let reds = (0..40)
        .flat_map(|dy| (0..30).map(move |dx| (dx, dy)))
        .filter(|&(dx, dy)| px(&shadow, OX + dx, y0 + dy)[0] > 100 && px(&shadow, OX + dx, y0 + dy)[1] < 50)
        .count();
    assert!(reds > 5, "a red shadow shows: {reds}");
}

#[test]
fn long_words_shrink_or_get_cut() {
    let mut e = env_with_font();
    let (_, container) = scene(&mut e);
    let container = holder(&mut e, container);
    let font = face(&mut e, "fonts/Face.ttf");
    let l = label(&mut e, container, [0.0, 100.0, 60.0, 130.0], "Supercalifragilisticexpialidocious");
    set_font(&mut e, l, font, 20.0);
    let y0 = 240;
    let cols = |f: &UiFrame| (0..120u32).filter(|dx| lit(f, OX + *dx, y0, OX + *dx + 1, y0 + 30) > 0).count();
    // Too wide for one line: cut with an ellipsis inside the frame.
    let cut = e.vm.render_ui().unwrap();
    assert!(cols(&cut) > 30 && lit(&cut, OX + 62, y0, OX + 120, y0 + 30) == 0);
    // A minimum font size shrinks the text instead.
    set(&mut e, l, "setMinimumFontSize", "(F)V", &[Value::Float(6.0)]);
    let shrunk = e.vm.render_ui().unwrap();
    assert!(lit(&shrunk, OX, y0, OX + 62, y0 + 30) > 0 && lit(&shrunk, OX + 62, y0, OX + 120, y0 + 30) == 0);
    assert_ne!(cut.serial, shrunk.serial);
    // Many lines: a long word is split by characters, and the tail of a limited text is an ellipsis.
    set(&mut e, l, "setNumberOfLines", "(I)V", &[Value::Int(0)]);
    set(&mut e, l, "setMinimumFontSize", "(F)V", &[Value::Float(0.0)]);
    let split = e.vm.render_ui().unwrap();
    assert!(lit(&split, OX, y0 - 20, OX + 62, y0 + 60) > lit(&cut, OX, y0 - 20, OX + 62, y0 + 60));
    // A single wide character fits nowhere and is still drawn (no endless loop).
    let t = e.string("WWWW");
    set(&mut e, l, "setText", "(Ljava/lang/String;)V", &[t]);
    let narrow = view(&mut e, "Label", 12, [0.0, 200.0, 2.0, 220.0]);
    add(&mut e, container, narrow);
    let t = e.string("WW");
    set(&mut e, narrow, "setText", "(Ljava/lang/String;)V", &[t]);
    assert!(e.vm.render_ui().is_some());
    // Several wide characters in a small frame: the tail ellipsis is itself cut back to fit.
    let tight = view(&mut e, "Label", 13, [0.0, 250.0, 40.0, 270.0]);
    add(&mut e, container, tight);
    let t = e.string("WWWWWWWW");
    set(&mut e, tight, "setText", "(Ljava/lang/String;)V", &[t]);
    let f = e.vm.render_ui().unwrap();
    assert!(lit(&f, OX, 250 + 140, OX + 41, 270 + 140) > 0 && lit(&f, OX + 41, 390, OX + 120, 410) == 0);
}

#[test]
fn fonts_fall_back_to_the_bundled_ones() {
    let mut e = env_with_font();
    let (_, container) = scene(&mut e);
    let container = holder(&mut e, container);
    let l = label(&mut e, container, [0.0, 0.0, 100.0, 20.0], "Aa");
    let ink = |e: &mut Env| lit(&e.vm.render_ui().unwrap(), OX, 140, OX + 100, 170);
    // No font set: regular.
    let plain = ink(&mut e);
    assert!(plain > 10);
    let new_face = |e: &mut Env, class: &str, sig: &str, args: &[Value]| {
        let r = e.scall("android/graphics/Typeface", class, sig, args);
        Value::Ref(Some(e.obj(r)))
    };
    let name = e.string("sans-serif");
    let bold = new_face(&mut e, "create", "(Ljava/lang/String;I)Landroid/graphics/Typeface;", &[name, Value::Int(1)]);
    set_font(&mut e, l, bold, 16.0);
    let b = ink(&mut e);
    let name = e.string("sans-serif");
    let italic = new_face(&mut e, "create", "(Ljava/lang/String;I)Landroid/graphics/Typeface;", &[name, Value::Int(2)]);
    set_font(&mut e, l, italic, 16.0);
    let i = ink(&mut e);
    let name = e.string("sans-serif");
    let regular =
        new_face(&mut e, "create", "(Ljava/lang/String;I)Landroid/graphics/Typeface;", &[name, Value::Int(0)]);
    set_font(&mut e, l, regular, 16.0);
    let r = ink(&mut e);
    assert!(b != r && i != r, "bold {b}, italic {i}, regular {r}");
    // An asset that is missing or is not a font, by path or by file name, ends at the bundled font as well.
    for path in ["missing/Nothing.otf", "Broken.otf", "deep/dir/Face.ttf"] {
        let f = face(&mut e, path);
        set_font(&mut e, l, f, 16.0);
        assert_eq!(ink(&mut e), r, "{path}");
    }
    // The base name of the path is tried too.
    let f = face(&mut e, "other/place/Face.ttf");
    set_font(&mut e, l, f, 16.0);
    assert_eq!(ink(&mut e), r);
}

#[test]
fn buttons_without_a_caption_label_draw_their_own_text() {
    let mut e = env_with_font();
    let (_, container) = scene(&mut e);
    let container = holder(&mut e, container);
    let btn = view(&mut e, "Button", 4, [0.0, 0.0, 100.0, 30.0]);
    add(&mut e, container, btn);
    let t = e.string("Go");
    set(&mut e, btn, "setText", "(Ljava/lang/String;I)V", &[t, Value::Int(0)]);
    set(&mut e, btn, "setTextColor", "(I)V", &[Value::Int(argb(255, 255, 255, 255))]);
    assert!(lit(&e.vm.render_ui().unwrap(), OX, 140, OX + 100, 170) > 0);
    // With a child label the caption is the label's job.
    let inner = label(&mut e, btn, [0.0, 0.0, 100.0, 30.0], "");
    let _ = inner;
    assert!(e.vm.render_ui().is_none());
}

#[test]
fn frames_are_cached_until_something_changes() {
    let mut e = Env::new();
    let (_, container) = scene(&mut e);
    let v = view(&mut e, "View", 4, [0.0, 0.0, 10.0, 10.0]);
    add(&mut e, container, v);
    set(&mut e, v, "_setBackgroundColor", "(I)V", &[Value::Int(argb(255, 1, 1, 1))]);
    let a = e.vm.render_ui().unwrap();
    let b = e.vm.render_ui().unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    e.vm.set_screen_size(900, 500);
    let c = e.vm.render_ui().unwrap();
    assert_eq!((c.width, c.height), (900, 500));
    assert!(c.serial > b.serial);
    set(&mut e, v, "setHidden", "(Z)V", &[Value::Int(1)]);
    assert!(e.vm.render_ui().is_none());
    set(&mut e, v, "setHidden", "(Z)V", &[Value::Int(0)]);
    assert!(e.vm.render_ui().unwrap().serial > c.serial, "a frame after a gap is new");
}

#[test]
fn a_view_cycle_or_a_dead_child_does_not_hang_the_renderer() {
    let mut e = Env::new();
    let (_, container) = scene(&mut e);
    let v = view(&mut e, "View", 4, [0.0, 0.0, 10.0, 10.0]);
    add(&mut e, container, v);
    set(&mut e, v, "_setBackgroundColor", "(I)V", &[Value::Int(argb(255, 1, 1, 1))]);
    // Forge a cycle and a dangling child directly in the tree.
    e.vm.ui_update(v, |i| i.children.push(container));
    e.vm.ui_update(v, |i| i.children.push(Obj(u32::MAX)));
    assert!(e.vm.render_ui().is_some());
}

// Golden images: popups as the game builds them (the welcome popup's view tree, from `dumpviews`), rendered on the
// game's 1024x768 window. The pixels are pinned by a hash; `BHPC_GOLDEN_DIR=dir` also writes them as PAM files to look at.

/// A bitmap with a deterministic pattern: a vertical gradient from `top` to `bottom` with a 2-pixel `edge` border.
fn panel_bitmap(e: &mut Env, w: i32, h: i32, top: [u8; 3], bottom: [u8; 3], edge: [u8; 4]) -> Obj {
    let b = bitmap(e, w, h, [0; 4]);
    let (_, _, ptr) = crate::jlib::bitmap_info(&e.vm, b).unwrap();
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            if x < 2 || y < 2 || x >= w - 2 || y >= h - 2 {
                px.extend_from_slice(&edge);
            } else {
                let t = y as f32 / (h - 1) as f32;
                let mix = |i: usize| (f32::from(top[i]) * (1.0 - t) + f32::from(bottom[i]) * t).round() as u8;
                px.extend_from_slice(&[mix(0), mix(1), mix(2), 255]);
            }
        }
    }
    e.mem().write_bytes(ptr, &px).unwrap();
    b
}

fn styled_label(e: &mut Env, parent: Obj, frame: [f32; 4], text: &str, size: f32, lines: i32) -> Obj {
    let l = label(e, parent, frame, text);
    let f = face(e, "fonts/Face.ttf");
    set_font(e, l, f, size);
    set(e, l, "setTextAlignment", "(I)V", &[Value::Int(1)]);
    set(e, l, "setNumberOfLines", "(I)V", &[Value::Int(lines)]);
    set(e, l, "_setBackgroundColor", "(I)V", &[Value::Int(0)]);
    l
}

/// The welcome popup: scrim, panel image, title, wrapped message and two buttons with caption labels.
/// Returns the OK button.
fn welcome_popup(e: &mut Env) -> Obj {
    e.vm.set_screen_size(1024, 768);
    let window = view(e, "Window", 1, [0.0; 4]);
    let root = view(e, "View", 2, [0.0, 0.0, 1024.0, 768.0]);
    let scrim = view(e, "View", 3, [0.0, 0.0, 768.0, 1024.0]);
    add(e, window, root);
    add(e, root, scrim);
    set(e, scrim, "_setBackgroundColor", "(I)V", &[Value::Int(0xab10_215a_u32 as i32)]);
    let panel = view(e, "View", 4, [244.0, 380.0, 524.0, 643.0]);
    add(e, scrim, panel);
    let img = view(e, "ImageView", 5, [0.0, 0.0, 280.0, 263.0]);
    add(e, panel, img);
    let bmp = panel_bitmap(e, 280, 263, [16, 30, 80], [8, 20, 60], [255, 255, 255, 255]);
    set(e, img, "setImage", "(Landroid/graphics/Bitmap;II)V", &[Value::Ref(Some(bmp)), Value::Int(0), Value::Int(0)]);
    styled_label(e, panel, [10.0, 10.0, 270.0, 30.0], "WELCOME TO THE BLOCKHEADS!", 16.0, 0);
    let msg = "YOU'RE IN A MASSIVE WORLD FULL OF THINGS TO FIND, EXPLORE, COLLECT AND CREATE, FROM CAVE TROLLS TO \
               STEAM TRAINS!\n\nBUT YOU CAN'T DO IT ALONE.\n\nTO START, WARP IN YOUR FIRST BLOCKHEAD BY TAPPING ON \
               THE PORTAL.";
    styled_label(e, panel, [10.0, 40.0, 270.0, 203.0], msg, 13.0, 0);
    let mut ok = None;
    for (i, (x, caption, colour)) in
        [(10.0, "SKIP", [40u8, 70, 150]), (145.0, "OK", [20, 130, 30])].into_iter().enumerate()
    {
        let btn = view(e, "Button", 10 + i as i32, [x, 213.0, x + 125.0, 253.0]);
        add(e, panel, btn);
        let dark = colour.map(|c| c / 2);
        let normal = panel_bitmap(e, 240, 80, colour, dark, [200, 200, 200, 255]);
        let lit_up = panel_bitmap(e, 240, 80, [255, 255, 255], colour, [200, 200, 200, 255]);
        for (state, b) in [(0, normal), (1, lit_up)] {
            set(
                e,
                btn,
                "setBackgroundDrawable",
                "(Landroid/graphics/Bitmap;I)V",
                &[Value::Ref(Some(b)), Value::Int(state)],
            );
        }
        styled_label(e, btn, [20.0, 10.0, 105.0, 30.0], caption, 16.0, 1);
        ok = Some(btn);
    }
    ok.unwrap()
}

/// FNV-1a over the pixels (and the size).
fn pixel_hash(f: &UiFrame) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in f.width.to_le_bytes().iter().chain(&f.height.to_le_bytes()).chain(f.rgba.iter()) {
        h = (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3);
    }
    h
}

fn golden(name: &str, f: &UiFrame, want: u64) {
    if let Some(dir) = std::env::var_os("BHPC_GOLDEN_DIR") {
        let mut out =
            format!("P7\nWIDTH {}\nHEIGHT {}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n", f.width, f.height)
                .into_bytes();
        out.extend_from_slice(&f.rgba);
        std::fs::write(std::path::Path::new(&dir).join(format!("{name}.pam")), out).unwrap();
    }
    assert_eq!(pixel_hash(f), want, "golden image {name} changed (hash {:#018x})", pixel_hash(f));
}

#[test]
fn golden_welcome_popup() {
    let mut e = env_with_font();
    let ok = welcome_popup(&mut e);
    golden("welcome", &e.vm.render_ui().unwrap(), 0xd9d2_0767_e05f_fad3);
    set(&mut e, ok, "setHighlighted", "(Z)V", &[Value::Int(1)]);
    golden("welcome_ok_pressed", &e.vm.render_ui().unwrap(), 0x5a6d_d7b3_3a0b_3d31);
    set(&mut e, ok, "setHighlighted", "(Z)V", &[Value::Int(0)]);
    golden("welcome", &e.vm.render_ui().unwrap(), 0xd9d2_0767_e05f_fad3);
}

/// Text fields, a scrolled clipping view, a half transparent view and shadowed, shrunk, right-aligned text.
#[test]
fn golden_dialog_with_fields() {
    let mut e = env_with_font();
    e.vm.set_screen_size(1024, 768);
    let window = view(&mut e, "Window", 1, [0.0; 4]);
    let root = view(&mut e, "View", 2, [0.0, 0.0, 1024.0, 768.0]);
    add(&mut e, window, root);
    let dialog = view(&mut e, "View", 3, [200.0, 300.0, 560.0, 600.0]);
    add(&mut e, root, dialog);
    set(&mut e, dialog, "_setBackgroundColor", "(I)V", &[Value::Int(argb(230, 30, 30, 40))]);
    let field = view(&mut e, "TextField", 4, [20.0, 20.0, 340.0, 60.0]);
    add(&mut e, dialog, field);
    set(&mut e, field, "_setBackgroundColor", "(I)V", &[Value::Int(argb(255, 250, 250, 250))]);
    let t = e.string("My World 1");
    set(&mut e, field, "setText", "(Ljava/lang/String;)V", &[t]);
    set(&mut e, field, "setTextColor", "(I)V", &[Value::Int(argb(255, 0, 0, 0))]);
    let f = face(&mut e, "fonts/Face.ttf");
    set_font(&mut e, field, f, 20.0);
    let scroll = view(&mut e, "View", 5, [20.0, 80.0, 340.0, 200.0]);
    add(&mut e, dialog, scroll);
    set(&mut e, scroll, "setClipsToBounds", "(Z)V", &[Value::Int(1)]);
    let r = rect(&mut e, [0.0, 50.0, 0.0, 0.0]);
    set(&mut e, scroll, "setBounds", "(Landroid/graphics/RectF;)V", &[r]);
    for i in 0..6 {
        let y = i as f32 * 40.0;
        let row = styled_label(
            &mut e,
            scroll,
            [0.0, y, 320.0, y + 36.0],
            &format!("WORLD NUMBER {i} WITH A LONG NAME"),
            18.0,
            1,
        );
        set(&mut e, row, "setMinimumFontSize", "(F)V", &[Value::Float(10.0)]);
        set(&mut e, row, "_setBackgroundColor", "(I)V", &[Value::Int(argb(255, 60, 60 + 30 * i as u8, 90))]);
        set(&mut e, row, "setTextAlignment", "(I)V", &[Value::Int(2)]);
        set(&mut e, row, "setShadowColor", "(I)V", &[Value::Int(argb(200, 0, 0, 0))]);
        set(&mut e, row, "setShadowOffset", "(FF)V", &[Value::Float(1.0), Value::Float(2.0)]);
    }
    let faded = view(&mut e, "View", 6, [20.0, 220.0, 340.0, 280.0]);
    add(&mut e, dialog, faded);
    set(&mut e, faded, "_setBackgroundColor", "(I)V", &[Value::Int(argb(255, 200, 40, 40))]);
    set(&mut e, faded, "_setAlpha", "(F)V", &[Value::Float(0.5)]);
    golden("dialog", &e.vm.render_ui().unwrap(), 0xab89_6260_cf12_7458);
}

/// The picture drawn from scratch, without the renderer's cache (which is kept for the next frame).
fn full_render(e: &Env) -> Option<Vec<u8>> {
    let cache = std::mem::take(&mut e.vm.st().render);
    e.vm.set_ui_dirty_tracking(false);
    let f = e.vm.render_ui().map(|f| f.rgba.to_vec());
    e.vm.st().render = cache;
    f
}

/// Every change, drawn by repainting only what changed, gives the pixels of a drawing from scratch.
#[test]
fn partial_redraws_match_full_ones() {
    let mut e = env_with_font();
    let ok = welcome_popup(&mut e);
    let first = e.vm.render_ui().unwrap();
    let label = view(&mut e, "Label", 40, [0.0, 0.0, 0.0, 0.0]);
    let parent = e.vm.st().ui.views[&ok].parent.unwrap();
    add(&mut e, parent, label);
    type Change = Box<dyn Fn(&mut Env)>;
    let changes: Vec<Change> = vec![
        Box::new(move |e| set(e, ok, "setHighlighted", "(Z)V", &[Value::Int(1)])),
        Box::new(move |e| set(e, ok, "setHighlighted", "(Z)V", &[Value::Int(0)])),
        Box::new(move |e| {
            let r = rect(e, [30.0, 150.0, 250.0, 175.0]);
            set(e, label, "setFrame", "(Landroid/graphics/RectF;)V", &[r]);
            let t = e.string("A NEW LINE OF TEXT");
            set(e, label, "setText", "(Ljava/lang/String;)V", &[t]);
            set(e, label, "setShadowColor", "(I)V", &[Value::Int(argb(255, 0, 0, 0))]);
            set(e, label, "setShadowOffset", "(FF)V", &[Value::Float(-3.0), Value::Float(4.0)]);
        }),
        Box::new(move |e| set(e, label, "_setBackgroundColor", "(I)V", &[Value::Int(argb(128, 200, 100, 0))])),
        Box::new(move |e| set(e, ok, "_setAlpha", "(F)V", &[Value::Float(0.4)])),
        Box::new(move |e| set(e, label, "setHidden", "(Z)V", &[Value::Int(1)])),
        Box::new(move |e| set(e, ok, "setEnabled", "(Z)V", &[Value::Int(0)])),
        Box::new(move |e| {
            // Moved off the window's edge: only the part inside is drawn.
            let r = rect(e, [-300.0, -20.0, -200.0, 20.0]);
            set(e, ok, "setFrame", "(Landroid/graphics/RectF;)V", &[r]);
        }),
        Box::new(move |e| set(e, parent, "removeSubview", "(Lcom/apportable/ui/View;)V", &[Value::Ref(Some(ok))])),
    ];
    let mut serial = first.serial;
    for (n, change) in changes.iter().enumerate() {
        change(&mut e);
        let f = e.vm.render_ui().unwrap();
        assert!(f.serial >= serial);
        serial = f.serial;
        assert!(*f.rgba == full_render(&e).unwrap(), "change {n} drawn like from scratch");
    }
}

/// Unchanged views are not even looked at again, except now and then; bitmap writes through the bitmap interfaces
/// count as changes.
#[test]
fn unchanged_views_are_answered_from_the_cache() {
    let mut e = Env::new();
    let (_, container) = scene(&mut e);
    let container = holder(&mut e, container);
    let img = view(&mut e, "ImageView", 4, [0.0, 0.0, 2.0, 2.0]);
    add(&mut e, container, img);
    let bmp = bitmap(&mut e, 2, 2, [255, 0, 0, 255]);
    set(
        &mut e,
        img,
        "setImage",
        "(Landroid/graphics/Bitmap;II)V",
        &[Value::Ref(Some(bmp)), Value::Int(0), Value::Int(0)],
    );
    let (_, _, ptr) = crate::jlib::bitmap_info(&e.vm, bmp).unwrap();
    let a = e.vm.render_ui().unwrap();
    // A write the renderer is not told of shows within RECHECK_FRAMES frames.
    e.mem().write_bytes(ptr, &[0, 0, 255, 255]).unwrap();
    for _ in 0..crate::render::RECHECK_FRAMES {
        assert!(Arc::ptr_eq(&e.vm.render_ui().unwrap(), &a));
    }
    let b = e.vm.render_ui().unwrap();
    assert_eq!(px(&b, OX, 140)[..3], [0, 0, 255]);
    // `copyPixelsFromBuffer` and a lock of the pixels are changes.
    let src = e.bytes(&[0, 255, 0, 255].repeat(4), 0);
    let buf = e.jni("NewDirectByteBuffer", &[src, 16, 0]).r0;
    let buf = e.vm.resolve(buf).unwrap();
    e.call(bmp, "copyPixelsFromBuffer", "(Ljava/nio/Buffer;)V", &[Value::Ref(Some(buf))]).unwrap();
    assert_eq!(px(&e.vm.render_ui().unwrap(), OX, 140)[..3], [0, 255, 0]);
    let g = e.vm.st().ui.generation;
    e.vm.ui_changed();
    assert!(e.vm.st().ui.generation > g);
    // Nothing shown is cached as well.
    set(&mut e, img, "setHidden", "(Z)V", &[Value::Int(1)]);
    assert!(e.vm.render_ui().is_none());
    assert!(e.vm.render_ui().is_none());
    // With the tracking off every frame looks again.
    set(&mut e, img, "setHidden", "(Z)V", &[Value::Int(0)]);
    e.vm.set_ui_dirty_tracking(false);
    let c = e.vm.render_ui().unwrap();
    e.mem().write_bytes(ptr, &[9, 9, 9, 255]).unwrap();
    let d = e.vm.render_ui().unwrap();
    assert!(d.serial > c.serial);
    assert_eq!(px(&d, OX, 140)[..3], [9, 9, 9]);
}

/// Cost of the renderer on the welcome popup: `cargo test --release -p bhpc-jni bench_popup -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark"]
fn bench_popup() {
    let median = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    // CPU time of this thread: what the work costs, also on a busy machine (wall time where there is no such clock).
    #[cfg(unix)]
    let cpu = || {
        let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: valid out pointer.
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
        t.tv_sec as f64 * 1e6 + t.tv_nsec as f64 / 1e3
    };
    #[cfg(not(unix))]
    let start = std::time::Instant::now();
    #[cfg(not(unix))]
    let cpu = || start.elapsed().as_secs_f64() * 1e6;
    let time = |f: &mut dyn FnMut()| {
        let t = cpu();
        f();
        cpu() - t
    };
    let mut e = env_with_font();
    let ok = welcome_popup(&mut e);
    let first = median(
        (0..9)
            .map(|_| {
                e.vm.st().render = Default::default();
                time(&mut || drop(e.vm.render_ui()))
            })
            .collect(),
    );
    let steady = median((0..2000).map(|_| time(&mut || drop(e.vm.render_ui()))).collect());
    let mut on = 0;
    let toggle = median(
        (0..60)
            .map(|_| {
                on ^= 1;
                set(&mut e, ok, "setHighlighted", "(Z)V", &[Value::Int(on)]);
                time(&mut || drop(e.vm.render_ui()))
            })
            .collect(),
    );
    println!("[bench] welcome popup: first {first:.0} us, unchanged {steady:.1} us, button highlight {toggle:.0} us");
}

/// An `AlertView` with `title`, `message`, an input line holding `text` and the buttons, shown.
fn text_alert(e: &mut Env, title: &str, message: &str, text: &str, buttons: &[&str], cancel: bool) -> (Obj, Obj) {
    let ctx = Value::Ref(Some(e.activity()));
    let r = e.scall(
        &format!("{UI}/AlertView"),
        "create",
        "(Landroid/content/Context;I)Lcom/apportable/ui/AlertView;",
        &[ctx, Value::Int(70)],
    );
    let alert = e.obj(r);
    for (name, s) in [("setTitle", title), ("setMessage", message)] {
        let v = e.string(s);
        set(e, alert, name, "(Ljava/lang/String;)V", &[v]);
    }
    for (i, b) in buttons.iter().enumerate() {
        let v = e.string(b);
        set(e, alert, "addButtonWithTitle", "(Ljava/lang/String;I)V", &[v, Value::Int(i as i32 + 1)]);
    }
    if cancel {
        let v = e.string("Cancel");
        set(e, alert, "setCancelButtonTitle", "(Ljava/lang/String;)V", &[v]);
    }
    let field = view(e, "TextField", 71, [0.0; 4]);
    let v = e.string(text);
    set(e, field, "setText", "(Ljava/lang/String;)V", &[v]);
    add(e, alert, field);
    set(e, alert, "show", "()V", &[]);
    (alert, field)
}

fn dialog_layout(e: &Env) -> crate::render::dialog::Layout {
    let alert = e.vm.st().dialog.clone().unwrap().alert;
    let font = ab_glyph::FontArc::try_from_vec(bhpc_vfs::fonts::bundled(bhpc_vfs::fonts::Style::Regular).to_vec());
    crate::render::dialog::layout((800.0, 480.0), &alert, &font.unwrap())
}

#[test]
fn an_alert_with_an_input_line_is_drawn_as_a_dialog() {
    use crate::TouchPhase;
    let mut e = Env::new();
    assert!(!e.vm.dialog_shown());
    assert_eq!(e.vm.take_dialog_pick(), None);
    e.vm.set_dialog_caret(Some(1)); // nothing shown: ignored
    assert_eq!(e.vm.render_ui(), None, "nothing to draw");
    let (alert, field) = text_alert(&mut e, "World Name", "", "MY WORLD", &["Done"], true);
    assert!(e.vm.dialog_shown());
    let l = dialog_layout(&e);
    assert_eq!(l.message, None);
    assert_eq!(l.buttons.iter().map(|b| (b.0.as_str(), b.1)).collect::<Vec<_>>(), [("Cancel", 0), ("Done", 1)]);
    let f = e.vm.render_ui().unwrap();
    // A scrim over the whole window, the panel, the white field with its text, no caret yet.
    assert_ne!(px(&f, 5, 5)[3], 0);
    let fl = l.field.unwrap().map(|v| v as u32);
    assert_eq!(px(&f, fl[2] - 3, fl[1] + 3), [255; 4]);
    let dark = |f: &UiFrame, x0: u32, x1: u32| {
        (x0..x1).filter(|&x| (fl[1] + 8..fl[3] - 8).any(|y| px(f, x, y)[0] < 100)).count()
    };
    let text_cols = dark(&f, fl[0], fl[2]);
    assert!(text_cols > 20, "the text is drawn");
    e.vm.set_dialog_caret(Some(2));
    e.vm.set_dialog_caret(Some(2)); // unchanged
    let with_caret = e.vm.render_ui().unwrap();
    assert!(dark(&with_caret, fl[0], fl[2]) > text_cols, "the caret is drawn");
    // A long text scrolls so that the caret stays in view.
    e.vm.set_text_of(field, &"W".repeat(80));
    e.vm.set_dialog_caret(Some(80));
    let long = e.vm.render_ui().unwrap();
    assert!(dark(&long, fl[2] - 30, fl[2] - 6) > 0);
    e.vm.set_text_of(field, "");
    e.vm.set_dialog_caret(None);
    let empty = e.vm.render_ui().unwrap();
    assert_eq!(dark(&empty, fl[0], fl[2]), 0, "no text, no caret");

    // The dialog is modal: every touch is its own; press and release on a button picks it.
    let vm = e.vm.clone();
    let touch = |e: &mut Env, p, x: f32, y: f32| vm.ui_touch(&mut e.cpu, p, x, y).unwrap();
    let centre = |r: [f32; 4]| ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    let (cx, cy) = centre(l.buttons[0].2);
    let (dx, dy) = centre(l.buttons[1].2);
    assert!(touch(&mut e, TouchPhase::Begin, 5.0, 5.0));
    assert!(touch(&mut e, TouchPhase::End, 5.0, 5.0));
    assert_eq!(e.vm.take_dialog_pick(), None, "outside the buttons");
    assert!(touch(&mut e, TouchPhase::Begin, cx, cy));
    let pressed = e.vm.render_ui().unwrap();
    assert_eq!(px(&pressed, cx as u32, l.buttons[0].2[1] as u32 + 2), [0x33, 0xB5, 0xE5, 0xFF], "highlighted");
    assert!(touch(&mut e, TouchPhase::Move, dx, dy));
    assert!(touch(&mut e, TouchPhase::End, dx, dy));
    assert_eq!(e.vm.take_dialog_pick(), None, "released on another button");
    assert!(touch(&mut e, TouchPhase::Begin, dx, dy));
    assert!(touch(&mut e, TouchPhase::Cancel, dx, dy));
    assert!(touch(&mut e, TouchPhase::End, dx, dy));
    assert_eq!(e.vm.take_dialog_pick(), None, "cancelled");
    assert!(touch(&mut e, TouchPhase::Begin, dx, dy));
    assert!(touch(&mut e, TouchPhase::End, dx, dy));
    assert_eq!(e.vm.take_dialog_pick().map(|(a, p)| (a.view, p)), Some((alert, 1)));
    assert_eq!(e.vm.take_dialog_pick(), None, "taken once");

    // Answering another alert leaves it; answering this one closes it.
    e.vm.close_dialog(field);
    assert!(e.vm.dialog_shown());
    let shown = e.vm.take_alerts();
    assert_eq!(shown[0].view, alert);
    e.vm.alert_click(&mut e.cpu, &shown[0], 1).unwrap();
    assert!(!e.vm.dialog_shown());
    assert!(!touch(&mut e, TouchPhase::Begin, dx, dy), "touches reach the game again");
    assert_eq!(e.vm.render_ui(), None);
}

#[test]
fn the_dialog_lays_out_messages_and_any_number_of_buttons() {
    let mut e = Env::new();
    text_alert(&mut e, "", "Give your blockhead a name. It can be changed later.", "", &[], false);
    let l = dialog_layout(&e);
    let m = l.message.unwrap();
    assert!(m[1] > l.title[3] && l.field.unwrap()[1] > m[3], "title, message, field from the top");
    assert!(l.buttons.is_empty());
    assert!(e.vm.render_ui().is_some());
    // A big window scales the dialog up; a narrow one keeps a margin.
    let alert = e.vm.st().dialog.clone().unwrap().alert;
    let font =
        ab_glyph::FontArc::try_from_vec(bhpc_vfs::fonts::bundled(bhpc_vfs::fonts::Style::Regular).to_vec()).unwrap();
    let big = crate::render::dialog::layout((3072.0, 1536.0), &alert, &font);
    assert_eq!(big.scale, 2.0);
    let narrow = crate::render::dialog::layout((300.0, 600.0), &alert, &font);
    assert_eq!(narrow.panel[0], 16.0);
}

/// An `AlertView` without an input line: `buttons` get indices 1.., `cancel` is index 0.
fn button_alert(e: &mut Env, title: &str, message: &str, buttons: &[&str], cancel: Option<&str>) -> Obj {
    let ctx = Value::Ref(Some(e.activity()));
    let r = e.scall(
        &format!("{UI}/AlertView"),
        "create",
        "(Landroid/content/Context;I)Lcom/apportable/ui/AlertView;",
        &[ctx, Value::Int(70)],
    );
    let alert = e.obj(r);
    for (name, s) in [("setTitle", title), ("setMessage", message)] {
        let v = e.string(s);
        set(e, alert, name, "(Ljava/lang/String;)V", &[v]);
    }
    for (i, b) in buttons.iter().enumerate() {
        let v = e.string(b);
        set(e, alert, "addButtonWithTitle", "(Ljava/lang/String;I)V", &[v, Value::Int(i as i32 + 1)]);
    }
    if let Some(c) = cancel {
        let v = e.string(c);
        set(e, alert, "setCancelButtonTitle", "(Ljava/lang/String;)V", &[v]);
    }
    set(e, alert, "show", "()V", &[]);
    alert
}

#[test]
fn a_system_alert_is_drawn_as_a_dialog_that_keys_steer() {
    use crate::DialogNav as N;
    let mut e = Env::new();
    assert!(!e.vm.dialog_nav(N::Accept), "no dialog");
    assert_eq!(e.vm.dialog_alert(), None);
    let alert = button_alert(&mut e, "Exit?", "Are you sure you want to exit The Blockheads?", &["Yes"], Some("No"));
    assert_eq!(e.vm.dialog_alert().map(|a| a.view), Some(alert));
    assert!(e.vm.dialog_shown());
    let l = dialog_layout(&e);
    assert_eq!(l.field, None, "no input line");
    assert_eq!(l.buttons.iter().map(|b| (b.0.as_str(), b.1)).collect::<Vec<_>>(), [("No", 0), ("Yes", 1)]);
    // The default (positive) button has the focus: lighter, underlined.
    let f = e.vm.render_ui().unwrap();
    let (no, yes) = (l.buttons[0].2.map(|v| v as u32), l.buttons[1].2.map(|v| v as u32));
    assert_eq!(px(&f, yes[0] + 3, yes[1] + 3), [0x5A, 0x5A, 0x5A, 0xFF]);
    assert_eq!(px(&f, yes[0] + 3, yes[3] - 1), [0x33, 0xB5, 0xE5, 0xFF]);
    assert_eq!(px(&f, no[0] + 3, no[1] + 3), [0x3D, 0x3D, 0x3D, 0xFF]);
    // Arrows move the focus and stop at the ends; Enter presses the focused button, Escape the cancel button.
    assert!(e.vm.dialog_nav(N::Next));
    assert_eq!(e.vm.st().dialog.as_ref().unwrap().focus, Some(1), "already the last one");
    e.vm.dialog_nav(N::Prev);
    e.vm.dialog_nav(N::Prev);
    assert_eq!(e.vm.st().dialog.as_ref().unwrap().focus, Some(0));
    let moved = e.vm.render_ui().unwrap();
    assert_eq!(px(&moved, no[0] + 3, no[1] + 3), [0x5A, 0x5A, 0x5A, 0xFF]);
    e.vm.dialog_nav(N::Accept);
    assert_eq!(e.vm.take_dialog_pick().map(|(_, p)| p), Some(0));
    e.vm.dialog_nav(N::Next);
    e.vm.dialog_nav(N::Accept);
    assert_eq!(e.vm.take_dialog_pick().map(|(_, p)| p), Some(1));
    e.vm.dialog_nav(N::Cancel);
    assert_eq!(e.vm.take_dialog_pick().map(|(_, p)| p), Some(0));
    // A script finds the dialog's texts and clicks its buttons; exact captions win over parts of others.
    assert!(e.vm.has_text("sure you want") && e.vm.has_text("exit?") && e.vm.has_text("yes"));
    assert!(!e.vm.has_text("maybe"));
    let centre = |r: [f32; 4]| ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    assert_eq!(e.vm.control_screen_pos("YES"), Some(centre(l.buttons[1].2)));
    assert_eq!(e.vm.control_screen_pos("es"), Some(centre(l.buttons[1].2)), "a part of a caption");
    assert_eq!(e.vm.control_screen_pos("Exit?"), None, "only buttons are clicked");
    // Answering it closes the dialog.
    let shown = e.vm.take_alerts();
    e.vm.alert_click(&mut e.cpu, &shown[0], 1).unwrap();
    assert!(!e.vm.dialog_shown());
    assert!(!e.vm.has_text("exit?"));
}

#[test]
fn alerts_wait_for_their_turn_and_can_be_dismissed() {
    use crate::DialogNav as N;
    let mut e = Env::new();
    let first = button_alert(&mut e, "Restore Double-Time", "", &["Restore"], Some("Cancel"));
    // Shown again while up: still one dialog.
    set(&mut e, first, "show", "()V", &[]);
    let second = button_alert(&mut e, "Allow", "Allow?", &["Allow"], Some("Don't Allow"));
    let third = button_alert(&mut e, "Note", "", &["A", "B", "C", "D"], None);
    assert_eq!(e.vm.dialog_alert().map(|a| a.view), Some(first));
    assert!(!e.vm.has_text("don't"), "a waiting alert is not shown");
    // Closing a waiting alert drops it; closing the shown one shows the next.
    e.vm.close_dialog(second);
    set(&mut e, first, "dismiss", "()V", &[]);
    assert_eq!(e.vm.dialog_alert().map(|a| a.view), Some(third));
    // Four buttons are stacked, the first of them focused; there is no cancel button for Escape.
    let l = dialog_layout(&e);
    assert!(l.buttons.windows(2).all(|w| w[1].2[1] > w[0].2[3] && w[1].2[0] == w[0].2[0]), "stacked");
    assert_eq!(l.buttons.iter().map(|b| b.1).collect::<Vec<_>>(), [1, 2, 3, 4]);
    assert_eq!(e.vm.st().dialog.as_ref().unwrap().focus, Some(1));
    e.vm.dialog_nav(N::Cancel);
    assert_eq!(e.vm.take_dialog_pick(), None);
    e.vm.dialog_nav(N::Next);
    e.vm.dialog_nav(N::Next);
    e.vm.dialog_nav(N::Accept);
    assert_eq!(e.vm.take_dialog_pick().map(|(_, p)| p), Some(3));
    // A focus that matches no button (no buttons at all) goes to the ends; Accept then has nothing to press.
    e.vm.st().dialog.as_mut().unwrap().focus = None;
    e.vm.dialog_nav(N::Prev);
    assert_eq!(e.vm.st().dialog.as_ref().unwrap().focus, Some(4));
    e.vm.st().dialog.as_mut().unwrap().focus = None;
    e.vm.dialog_nav(N::Next);
    assert_eq!(e.vm.st().dialog.as_ref().unwrap().focus, Some(1));
    e.vm.st().dialog.as_mut().unwrap().focus = None;
    e.vm.dialog_nav(N::Accept);
    assert_eq!(e.vm.take_dialog_pick().map(|(_, p)| p), Some(1), "the default button");
    e.vm.close_dialog(third);
    let empty = button_alert(&mut e, "Busy", "", &[], None);
    for nav in [N::Next, N::Prev, N::Accept] {
        assert!(e.vm.dialog_nav(nav));
    }
    assert_eq!(e.vm.take_dialog_pick(), None);
    assert_eq!(e.vm.control_screen_pos("Busy"), None);
    e.vm.close_dialog(empty);
    assert!(!e.vm.dialog_shown());
    assert!(e.vm.swap_buffers(), "the default graphics hooks present nothing");
}

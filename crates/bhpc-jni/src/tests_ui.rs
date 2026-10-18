//! The UIKit view tree: geometry, hit testing, button events, text focus and the text lookups scripts use.

use crate::tests::Env;
use crate::{Obj, TouchPhase, Value};

fn rect(e: &mut Env, l: f32, t: f32, r: f32, b: f32) -> Value {
    Value::Ref(Some(e.new_obj(
        "android/graphics/RectF",
        "(FFFF)V",
        &[Value::Float(l), Value::Float(t), Value::Float(r), Value::Float(b)],
    )))
}

fn view(e: &mut Env, kind: &str, handle: i32, frame: Option<(f32, f32, f32, f32)>) -> Obj {
    let ctx = Value::Ref(Some(e.vm.activity()));
    let class = format!("com/apportable/ui/{kind}");
    match frame {
        None => {
            e.scall(&class, "create", &format!("(Landroid/content/Context;I)L{class};"), &[ctx, Value::Int(handle)])
        }
        Some((l, t, r, b)) => {
            let rc = rect(e, l, t, r, b);
            e.scall(
                &class,
                "create",
                &format!("(Landroid/content/Context;ILandroid/graphics/RectF;)L{class};"),
                &[ctx, Value::Int(handle), rc],
            )
        }
    }
    .unwrap()
    .as_ref()
    .unwrap()
}

fn add(e: &mut Env, parent: Obj, child: Obj) {
    e.call(parent, "addSubview", "(Lcom/apportable/ui/View;)V", &[Value::Ref(Some(child))]).unwrap();
}

fn set_text(e: &mut Env, button: Obj, text: &str, state: i32) {
    let t = e.string(text);
    e.call(button, "setText", "(Ljava/lang/String;I)V", &[t, Value::Int(state)]).unwrap();
}

/// Window > container (portrait 480x800 inside the 800x480 screen) > popup > [label "Hello", Button "Go", Button "No"].
struct Scene {
    window: Obj,
    popup: Obj,
    go: Obj,
    no: Obj,
    label: Obj,
}

fn scene(e: &mut Env) -> Scene {
    let window = view(e, "Window", 1, None);
    let container = view(e, "View", 2, Some((0.0, 0.0, 480.0, 800.0)));
    let popup = view(e, "View", 3, Some((100.0, 200.0, 300.0, 400.0)));
    let label = view(e, "Label", 4, Some((10.0, 10.0, 190.0, 40.0)));
    let go = view(e, "Button", 5, Some((10.0, 150.0, 90.0, 190.0)));
    let no = view(e, "Button", 6, Some((100.0, 150.0, 190.0, 190.0)));
    add(e, window, container);
    add(e, container, popup);
    for c in [label, go, no] {
        add(e, popup, c);
    }
    let t = e.string("Hello there");
    e.call(label, "setText", "(Ljava/lang/String;)V", &[t]).unwrap();
    set_text(e, go, "Go", 0);
    set_text(e, no, "No", 0);
    Scene { window, popup, go, no, label }
}

fn event_calls(e: &Env) -> u64 {
    e.vm.report().members.iter().filter(|m| m.class.ends_with("Control") && m.name == "event").map(|m| m.calls).sum()
}

#[test]
fn rectf_holds_four_floats() {
    let mut e = Env::new();
    let r = e.new_obj(
        "android/graphics/RectF",
        "(FFFF)V",
        &[Value::Float(1.0), Value::Float(2.0), Value::Float(5.0), Value::Float(8.0)],
    );
    let f = |e: &mut Env, n: &str| e.call(r, n, "()F", &[]).unwrap().as_float();
    assert_eq!((f(&mut e, "width"), f(&mut e, "height")), (4.0, 6.0));
    assert_eq!((f(&mut e, "centerX"), f(&mut e, "centerY")), (3.0, 5.0));
    assert_eq!(e.call(r, "isEmpty", "()Z", &[]), Ok(Value::Int(0)));
    e.call(r, "offset", "(FF)V", &[Value::Float(1.0), Value::Float(-1.0)]).unwrap();
    assert_eq!(e.vm.rect_of(Some(r)), [2.0, 1.0, 6.0, 7.0]);
    assert_eq!(e.call(r, "contains", "(FF)Z", &[Value::Float(3.0), Value::Float(2.0)]), Ok(Value::Int(1)));
    assert_eq!(e.call(r, "contains", "(FF)Z", &[Value::Float(6.0), Value::Float(2.0)]), Ok(Value::Int(0)));
    e.call(r, "set", "(FFFF)V", &[Value::Float(0.0), Value::Float(0.0), Value::Float(0.0), Value::Float(9.0)]).unwrap();
    assert_eq!(e.call(r, "isEmpty", "()Z", &[]), Ok(Value::Int(1)));
    let copy = e.new_obj("android/graphics/RectF", "(Landroid/graphics/RectF;)V", &[Value::Ref(Some(r))]);
    assert_eq!(e.vm.rect_of(Some(copy)), [0.0, 0.0, 0.0, 9.0]);
    let empty = e.new_obj("android/graphics/RectF", "()V", &[]);
    assert_eq!(e.vm.rect_of(Some(empty)), [0.0; 4]);
    assert_eq!(e.vm.rect_of(None), [0.0; 4]);
}

#[test]
fn screen_and_ui_coordinates_turn_a_quarter_for_landscape_windows() {
    // the test platform is 800x480 (landscape): UI space is 480 wide and 800 tall
    let e = Env::new();
    assert_eq!(e.vm.screen_to_ui(100.0, 50.0), (430.0, 100.0));
    assert_eq!(e.vm.ui_to_screen(430.0, 100.0), (100.0, 50.0));
    let mut p = crate::tests::test_platform();
    p.screen_width = 480;
    p.screen_height = 800;
    let portrait = Env::with(|c| c.platform = p);
    assert_eq!(portrait.vm.screen_to_ui(10.0, 20.0), (10.0, 20.0));
    assert_eq!(portrait.vm.ui_to_screen(10.0, 20.0), (10.0, 20.0));
}

#[test]
fn hit_testing_finds_enabled_visible_controls() {
    let mut e = Env::new();
    let s = scene(&mut e);
    // popup origin (100,200); Go at (110..190, 350..390), No at (200..290, 350..390), in UI space
    let go_centre = e.vm.ui_to_screen(150.0, 370.0);
    let hit = e.vm.hit_test(go_centre.0 - 0.0, go_centre.1).map(|h| h.view);
    // hit_test takes UI coordinates (Window space), not screen pixels
    assert_eq!(hit, None, "screen pixels are not UI coordinates");
    let h = e.vm.hit_test(150.0, 370.0).unwrap();
    assert_eq!((h.view, h.kind.as_str(), h.handle), (s.go, "Button", 5));
    assert_eq!(e.vm.hit_test(250.0, 370.0).unwrap().view, s.no);
    assert_eq!(e.vm.hit_test(150.0, 220.0), None, "the label is not a control");
    assert_eq!(e.vm.hit_test(10.0, 10.0), None, "empty space");
    assert_eq!(e.vm.hit_test(600.0, 370.0), None, "outside the container");
    // disabled, hidden, translucent and non-interactive views are skipped
    e.call(s.go, "setEnabled", "(Z)V", &[Value::Int(0)]).unwrap();
    assert_eq!(e.vm.hit_test(150.0, 370.0), None);
    e.call(s.go, "setEnabled", "(Z)V", &[Value::Int(1)]).unwrap();
    e.call(s.no, "setHidden", "(Z)V", &[Value::Int(1)]).unwrap();
    assert_eq!(e.vm.hit_test(250.0, 370.0), None);
    e.call(s.no, "setHidden", "(Z)V", &[Value::Int(0)]).unwrap();
    e.call(s.no, "_setAlpha", "(F)V", &[Value::Float(0.0)]).unwrap();
    assert_eq!(e.vm.hit_test(250.0, 370.0), None);
    e.call(s.no, "_setAlpha", "(F)V", &[Value::Float(1.0)]).unwrap();
    e.call(s.popup, "setUserInteractionEnabled", "(Z)V", &[Value::Int(0)]).unwrap();
    assert_eq!(e.vm.hit_test(250.0, 370.0), None, "the popup's subtree ignores touches");
    e.call(s.popup, "setUserInteractionEnabled", "(Z)V", &[Value::Int(1)]).unwrap();
    // later subviews are on top; insertSubview puts one below, removeSubview takes it away
    let over = view(&mut e, "Button", 7, Some((10.0, 150.0, 90.0, 190.0)));
    add(&mut e, s.popup, over);
    assert_eq!(e.vm.hit_test(150.0, 370.0).unwrap().view, over);
    e.call(s.popup, "removeSubview", "(Lcom/apportable/ui/View;)V", &[Value::Ref(Some(over))]).unwrap();
    assert_eq!(e.vm.hit_test(150.0, 370.0).unwrap().view, s.go);
    e.call(s.popup, "insertSubview", "(Lcom/apportable/ui/View;I)V", &[Value::Ref(Some(over)), Value::Int(0)]).unwrap();
    assert_eq!(e.vm.hit_test(150.0, 370.0).unwrap().view, s.go, "inserted below");
    e.call(s.popup, "insertSubview", "(Lcom/apportable/ui/View;I)V", &[Value::Ref(Some(over)), Value::Int(99)])
        .unwrap();
    assert_eq!(e.vm.hit_test(150.0, 370.0).unwrap().view, over, "index is clamped");
    // a view moves between parents; bounds shift the content of a parent
    let rc = rect(&mut e, 0.0, 0.0, 50.0, 50.0);
    e.call(over, "setFrame", "(Landroid/graphics/RectF;)V", &[rc]).unwrap();
    e.call(s.window, "addSubview", "(Lcom/apportable/ui/View;)V", &[Value::Ref(Some(over))]).unwrap();
    assert_eq!(e.vm.hit_test(25.0, 25.0).unwrap().view, over);
    assert_eq!(e.vm.hit_test(150.0, 370.0).unwrap().view, s.go);
    let b = rect(&mut e, -10.0, 0.0, 480.0, 800.0);
    let container = e.vm.view_rect(s.popup).unwrap();
    assert_eq!(container.0, [100.0, 200.0, 200.0, 200.0]);
    e.call(s.popup, "setBounds", "(Landroid/graphics/RectF;)V", &[b]).unwrap();
    assert_eq!(e.vm.view_rect(s.go).unwrap().0, [120.0, 350.0, 80.0, 40.0], "children follow the bounds origin");
    // bad arguments are ignored
    e.call(s.popup, "addSubview", "(Lcom/apportable/ui/View;)V", &[Value::Ref(Some(s.popup))]).unwrap();
    e.call(s.popup, "addSubview", "(Lcom/apportable/ui/View;)V", &[Value::Ref(None)]).unwrap();
    e.call(s.popup, "removeSubview", "(Lcom/apportable/ui/View;)V", &[Value::Ref(None)]).unwrap();
    let plain = e.new_obj("java/lang/Object", "()V", &[]);
    assert_eq!(e.vm.view_rect(plain), None);
}

#[test]
fn text_lookups_serve_scripts() {
    let mut e = Env::new();
    let s = scene(&mut e);
    assert!(e.vm.has_text("hello"));
    assert!(e.vm.has_text("GO"));
    assert!(!e.vm.has_text("missing"));
    // the label of a button stands for the button: here the text is on the button itself
    let (x, y) = e.vm.control_screen_pos("go").unwrap();
    assert_eq!((x, y), e.vm.ui_to_screen(150.0, 370.0));
    let (lx, ly) = e.vm.control_screen_pos("hello").unwrap();
    assert_eq!((lx, ly), e.vm.ui_to_screen(200.0, 225.0), "a plain label is clicked at its own centre");
    assert_eq!(e.vm.control_screen_pos("missing"), None);
    // a label inside a button resolves to the button
    let inner = view(&mut e, "Label", 8, Some((5.0, 5.0, 25.0, 15.0)));
    let t = e.string("Inner");
    e.call(inner, "setText", "(Ljava/lang/String;)V", &[t]).unwrap();
    add(&mut e, s.no, inner);
    assert_eq!(e.vm.control_screen_pos("inner"), Some(e.vm.ui_to_screen(245.0, 370.0)));
    // a hidden view does not count; only the normal state of a button gives its text
    e.call(s.label, "setHidden", "(Z)V", &[Value::Int(1)]).unwrap();
    assert!(!e.vm.has_text("hello"));
    set_text(&mut e, s.go, "Pressed", 1);
    assert!(!e.vm.has_text("pressed"));
    // detached views are never shown
    let orphan = view(&mut e, "Label", 9, Some((0.0, 0.0, 5.0, 5.0)));
    let t = e.string("Orphan");
    e.call(orphan, "setText", "(Ljava/lang/String;)V", &[t]).unwrap();
    assert!(!e.vm.has_text("orphan"));
}

#[test]
fn the_dump_lists_the_tree() {
    let mut e = Env::new();
    let s = scene(&mut e);
    e.call(s.no, "setEnabled", "(Z)V", &[Value::Int(0)]).unwrap();
    e.call(s.go, "_setAlpha", "(F)V", &[Value::Float(0.5)]).unwrap();
    e.call(s.label, "setUserInteractionEnabled", "(Z)V", &[Value::Int(0)]).unwrap();
    e.call(s.window, "setHidden", "(Z)V", &[Value::Int(1)]).unwrap();
    let d = e.vm.dump_views();
    assert!(d.starts_with("Window#1 @"), "{d}");
    assert!(d.contains("\n      Button#5 ") && d.contains("alpha=0.50") && d.contains("\"Go\""), "{d}");
    assert!(d.contains("noinput") && d.contains("disabled") && d.contains("hidden"), "{d}");
    assert!(d.contains("[100,200 200x200]"), "{d}");
}

#[test]
fn touches_on_controls_are_played_as_android_would() {
    let mut e = Env::new();
    let s = scene(&mut e);
    let ret = e.code(&[0xE12F_FF1E]);
    let control = e.vm.class_named("com/apportable/ui/Control").unwrap();
    e.vm.register_native(control, "event", "(II)V", ret);
    let (gx, gy) = e.vm.ui_to_screen(150.0, 370.0);
    let (nx, ny) = e.vm.ui_to_screen(250.0, 370.0);
    let vm = e.vm.clone();
    let mut touch = |e: &mut Env, p: TouchPhase, x: f32, y: f32| vm.ui_touch(&mut e.cpu, p, x, y).unwrap();
    // press, drag and release on the button: all consumed, down and up-inside reach the native side
    assert!(touch(&mut e, TouchPhase::Begin, gx, gy));
    assert!(touch(&mut e, TouchPhase::Move, gx, gy));
    assert!(touch(&mut e, TouchPhase::End, gx, gy));
    assert_eq!(event_calls(&e), 2);
    // press on one button, release on another: up-outside
    assert!(touch(&mut e, TouchPhase::Begin, gx, gy));
    assert!(touch(&mut e, TouchPhase::End, nx, ny));
    assert_eq!(event_calls(&e), 4);
    // a cancelled touch ends the capture without an up event
    assert!(touch(&mut e, TouchPhase::Begin, gx, gy));
    assert!(touch(&mut e, TouchPhase::Cancel, gx, gy));
    assert!(!touch(&mut e, TouchPhase::Cancel, gx, gy));
    assert!(!touch(&mut e, TouchPhase::Move, gx, gy), "nothing is captured any more");
    assert!(!touch(&mut e, TouchPhase::End, gx, gy));
    assert_eq!(event_calls(&e), 5);
    // touches that start elsewhere belong to the game's window
    assert!(!touch(&mut e, TouchPhase::Begin, 700.0, 5.0));
    assert!(!touch(&mut e, TouchPhase::End, 700.0, 5.0));
    // a text field takes the keyboard focus when tapped, a tap that ends outside does not
    let field = view(&mut e, "TextField", 11, Some((0.0, 0.0, 200.0, 40.0)));
    add(&mut e, s.window, field);
    let (fx, fy) = e.vm.ui_to_screen(100.0, 20.0);
    assert!(touch(&mut e, TouchPhase::Begin, fx, fy));
    assert!(touch(&mut e, TouchPhase::End, 700.0, 5.0));
    assert_eq!(e.vm.text_focus(), None);
    assert!(touch(&mut e, TouchPhase::Begin, fx, fy));
    assert!(touch(&mut e, TouchPhase::End, fx, fy));
    assert_eq!(e.vm.text_focus(), Some(field));
}

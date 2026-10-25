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
fn the_portrait_ui_space_is_centred_on_landscape_windows() {
    // the test platform is 800x480 (landscape): UI space is 480 wide and 800 tall, centred: shifted by (160, -160)
    let e = Env::new();
    assert_eq!(e.vm.ui_offset(), (160.0, -160.0));
    assert_eq!(e.vm.screen_to_ui(100.0, 50.0), (-60.0, 210.0));
    assert_eq!(e.vm.ui_to_screen(-60.0, 210.0), (100.0, 50.0));
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
    let touch = |e: &mut Env, p: TouchPhase, x: f32, y: f32| vm.ui_touch(&mut e.cpu, p, x, y).unwrap();
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

#[test]
fn views_can_be_reparented_removed_and_never_loop() {
    let mut e = Env::new();
    let (win, a, b) = (view(&mut e, "Window", 1, None), view(&mut e, "View", 2, None), view(&mut e, "View", 3, None));
    let arg = |o: Obj| [Value::Ref(Some(o))];
    add(&mut e, win, a);
    add(&mut e, win, b);
    // moving a view to another parent takes it out of the old one
    add(&mut e, a, b);
    let flat = |e: &Env| e.vm.dump_views().lines().map(|l| l.len() - l.trim_start().len()).collect::<Vec<_>>();
    assert_eq!(flat(&e), [0, 2, 4]);
    e.call(win, "insertSubview", "(Lcom/apportable/ui/View;I)V", &[Value::Ref(Some(b)), Value::Int(0)]).unwrap();
    assert_eq!(flat(&e), [0, 2, 2]);
    e.call(win, "removeSubview", "(Lcom/apportable/ui/View;)V", &arg(b)).unwrap();
    assert_eq!(flat(&e), [0, 2, 0]);
    e.call(win, "removeSubview", "(Lcom/apportable/ui/View;)V", &arg(b)).unwrap();
    // a loop made by the game cannot hang the lookups
    add(&mut e, a, win);
    assert_eq!(e.vm.hit_test(1.0, 1.0), None);
    assert_eq!(e.vm.view_rect(a), None);
}

#[test]
fn text_fields_keep_their_text_and_the_focus() {
    let mut e = Env::new();
    let field = view(&mut e, "TextField", 11, Some((0.0, 0.0, 200.0, 40.0)));
    let other = view(&mut e, "TextField", 12, Some((0.0, 0.0, 200.0, 40.0)));
    let t = e.string("hello");
    e.call(field, "setText", "(Ljava/lang/String;)V", &[t]).unwrap();
    assert_eq!(e.vm.text_of(field), "hello");
    let got = e.call(field, "getText", "()Ljava/lang/String;", &[]).unwrap().as_ref().unwrap();
    assert_eq!(e.vm.string(got).as_deref(), Some("hello"));
    e.call(field, "becomeFirstResponder", "()V", &[]).unwrap();
    assert_eq!(e.vm.text_focus(), Some(field));
    e.call(other, "resignFirstResponder", "()V", &[]).unwrap();
    assert_eq!(e.vm.text_focus(), Some(field), "only the focused field can resign");
    e.call(field, "resignFirstResponder", "()V", &[]).unwrap();
    assert_eq!(e.vm.text_focus(), None);
}

#[test]
fn an_alert_with_an_input_line_takes_the_focus_until_it_is_answered() {
    let mut e = Env::new();
    let ctx = Value::Ref(Some(e.vm.activity()));
    let alert = e
        .scall(
            "com/apportable/ui/AlertView",
            "create",
            "(Landroid/content/Context;I)Lcom/apportable/ui/AlertView;",
            &[ctx, Value::Int(70)],
        )
        .unwrap()
        .as_ref()
        .unwrap();
    let field = view(&mut e, "TextField", 71, Some((0.0, 0.0, 200.0, 40.0)));
    add(&mut e, alert, field);
    e.call(alert, "show", "()V", &[]).unwrap();
    assert_eq!(e.vm.text_focus(), Some(field));
    let shown = e.vm.take_alerts();
    e.vm.alert_click(&mut e.cpu, &shown[0], 0).unwrap();
    assert_eq!(e.vm.text_focus(), None);
}

#[test]
fn the_newest_window_is_hit_first() {
    let mut e = Env::new();
    let hit_in_new_window = |e: &mut Env, handle: i32| {
        let win = view(e, "Window", handle, None);
        let button = view(e, "Button", handle + 1, Some((0.0, 0.0, 100.0, 100.0)));
        add(e, win, button);
        e.vm.hit_test(10.0, 10.0).map(|h| h.handle)
    };
    assert_eq!(hit_in_new_window(&mut e, 1), Some(2));
    assert_eq!(hit_in_new_window(&mut e, 10), Some(11), "two windows cover the same spot: the newer wins");
}

#[test]
fn only_clipping_views_confine_their_subviews() {
    // The game's landscape root view (800x480) holds a portrait popup whose button lies below the root's bottom edge.
    let mut e = Env::new();
    let window = view(&mut e, "Window", 1, None);
    let root = view(&mut e, "View", 2, Some((0.0, 0.0, 800.0, 480.0)));
    let popup = view(&mut e, "View", 3, Some((0.0, 300.0, 480.0, 700.0)));
    let ok = view(&mut e, "Button", 4, Some((10.0, 300.0, 200.0, 380.0)));
    add(&mut e, window, root);
    add(&mut e, root, popup);
    add(&mut e, popup, ok);
    assert_eq!(e.vm.hit_test(100.0, 640.0).map(|h| h.view), Some(ok), "drawn there, so it takes the touch");
    assert_eq!(e.vm.hit_test(300.0, 640.0), None, "beside the button");
    e.call(root, "setClipsToBounds", "(Z)V", &[Value::Int(1)]).unwrap();
    assert_eq!(e.vm.hit_test(100.0, 640.0), None, "a clipping root hides and shields it");
    assert_eq!(e.vm.hit_test(100.0, 300.0), None, "inside the root, outside the button");
}

#[test]
fn landscape_layouts_inside_the_portrait_space_are_not_shifted() {
    let mut e = Env::new();
    let window = view(&mut e, "Window", 1, None);
    let root = view(&mut e, "View", 2, Some((0.0, 0.0, 800.0, 480.0)));
    // the chat: a full-screen view of the landscape root, laid out for the landscape window
    let chat = view(&mut e, "View", 3, Some((0.0, 0.0, 800.0, 480.0)));
    let text = view(&mut e, "TextView", 4, Some((0.0, 80.0, 800.0, 180.0)));
    let close = view(&mut e, "Button", 5, Some((0.0, 0.0, 100.0, 70.0)));
    let send = view(&mut e, "Button", 6, Some((700.0, 0.0, 800.0, 70.0)));
    add(&mut e, window, root);
    add(&mut e, root, chat);
    for c in [text, close, send] {
        add(&mut e, chat, c);
    }
    set_text(&mut e, send, "SEND", 0);
    let (r, shown) = e.vm.view_rect(chat).unwrap();
    assert!(shown);
    assert_eq!(r, [-160.0, 160.0, 800.0, 480.0]);
    assert_eq!(e.vm.ui_to_screen(r[0], r[1]), (0.0, 0.0), "the window's corner");
    // touches and lookups find the controls where they are drawn
    let (ux, uy) = e.vm.screen_to_ui(750.0, 30.0);
    assert_eq!(e.vm.hit_test(ux, uy).map(|h| h.view), Some(send));
    assert_eq!(e.vm.control_screen_pos("send"), Some((750.0, 35.0)));
    // the tablet chat: a panel at the left of the landscape root, its (20, 20) the window's
    let panel = view(&mut e, "View", 8, Some((20.0, 20.0, 220.0, 300.0)));
    add(&mut e, root, panel);
    let (r, _) = e.vm.view_rect(panel).unwrap();
    assert_eq!(e.vm.ui_to_screen(r[0], r[1]), (20.0, 20.0));
    // a popup centred across the portrait space (480 wide) stays there
    let popup = view(&mut e, "View", 9, Some((140.0, 300.0, 340.0, 500.0)));
    add(&mut e, root, popup);
    assert_eq!(e.vm.view_rect(popup).unwrap().0, [140.0, 300.0, 200.0, 200.0]);
    // the root itself, a full-screen child of the window and a smaller view are placed in the portrait space
    assert_eq!(e.vm.view_rect(root).unwrap().0, [0.0, 0.0, 800.0, 480.0]);
    let full = view(&mut e, "View", 7, Some((0.0, 0.0, 800.0, 480.0)));
    add(&mut e, window, full);
    assert_eq!(e.vm.view_rect(full).unwrap().0, [0.0, 0.0, 800.0, 480.0]);
    // the return key of the text view stands for the right-most shown, enabled button of its panel
    assert_eq!(e.vm.return_button(text), Some(send));
    e.call(send, "setHidden", "(Z)V", &[Value::Int(1)]).unwrap();
    assert_eq!(e.vm.return_button(text), Some(close));
    e.call(close, "setEnabled", "(Z)V", &[Value::Int(0)]).unwrap();
    assert_eq!(e.vm.return_button(text), None);
    assert_eq!(e.vm.return_button(window), None, "no panel");
    // a button is tapped with a touch down and a touch up inside
    let ret = e.code(&[0xE12F_FF1E]);
    let control = e.vm.class_named("com/apportable/ui/Control").unwrap();
    e.vm.register_native(control, "event", "(II)V", ret);
    let vm = e.vm.clone();
    vm.press_button(&mut e.cpu, send).unwrap();
    assert_eq!(event_calls(&e), 2);
    // portrait windows have no landscape layouts to move
    let mut p = crate::tests::test_platform();
    (p.screen_width, p.screen_height) = (480, 800);
    assert!(Env::with(|c| c.platform = p).vm.landscape().is_none());
}

#[test]
fn the_text_of_a_text_view_is_kept_for_the_renderer() {
    let mut e = Env::new();
    let tv = view(&mut e, "TextView", 4, Some((0.0, 0.0, 100.0, 50.0)));
    assert_eq!(e.vm.text_of(tv), "", "no text yet");
    e.vm.replace_text(tv, 0, 0, "Türkçe!");
    e.vm.replace_text(tv, 6, 5, "");
    assert_eq!(e.vm.text_of(tv), "Türkçe", "replacing past the end is clamped");
    assert!(e.vm.dump_views().contains("\"Türkçe\""));
    assert_eq!(e.vm.view_kind(tv).as_deref(), Some("TextView"));
    assert_eq!(e.vm.view_kind(Obj(0)), None);
}

#[test]
fn the_chat_reads_like_a_desktop_chat() {
    let mut e = Env::new();
    let window = view(&mut e, "Window", 1, None);
    let root = view(&mut e, "View", 2, Some((0.0, 0.0, 800.0, 480.0)));
    let panel = view(&mut e, "View", 3, Some((20.0, 20.0, 220.0, 300.0)));
    let bg = view(&mut e, "ImageView", 4, Some((0.0, 0.0, 200.0, 280.0)));
    let header = view(&mut e, "View", 5, Some((0.0, 0.0, 120.0, 30.0)));
    let close = view(&mut e, "Button", 6, Some((5.0, 5.0, 45.0, 25.0)));
    let title = view(&mut e, "Label", 7, Some((40.0, 5.0, 80.0, 25.0)));
    let line = view(&mut e, "View", 8, Some((0.0, 28.0, 120.0, 30.0)));
    let send = view(&mut e, "Button", 9, Some((75.0, 5.0, 115.0, 25.0)));
    let field_bg = view(&mut e, "ImageView", 10, Some((0.0, 30.0, 120.0, 70.0)));
    let text = view(&mut e, "TextView", 11, Some((0.0, 30.0, 120.0, 70.0)));
    let scroll = view(&mut e, "ScrollView", 12, Some((0.0, 72.0, 120.0, 272.0)));
    let newest = view(&mut e, "View", 13, Some((0.0, 0.0, 120.0, 40.0)));
    let older = view(&mut e, "View", 14, Some((0.0, 40.0, 120.0, 80.0)));
    add(&mut e, window, root);
    add(&mut e, root, panel);
    for c in [bg, header, field_bg, text, scroll] {
        add(&mut e, panel, c);
    }
    for c in [close, title, line, send] {
        add(&mut e, header, c);
    }
    add(&mut e, scroll, newest);
    add(&mut e, scroll, older);
    // `x, y, w, h` relative to the panel
    let at = |e: &Env, v: Obj| {
        let (p, r) = (e.vm.view_rect(panel).unwrap().0, e.vm.view_rect(v).unwrap().0);
        [r[0] - p[0], r[1] - p[1], r[2], r[3]]
    };
    assert_eq!(at(&e, header), [0.0, 0.0, 200.0, 30.0], "the header spans the box");
    assert_eq!((at(&e, title), at(&e, line)), ([0.0, 5.0, 200.0, 20.0], [0.0, 28.0, 200.0, 2.0]));
    assert_eq!(at(&e, close), [5.0, 5.0, 40.0, 20.0], "CLOSE stays");
    assert_eq!(at(&e, bg), [0.0, 0.0, 200.0, 280.0], "the box itself stays");
    assert_eq!(at(&e, scroll), [4.0, 34.0, 192.0, 202.0], "the messages follow the header");
    assert_eq!(at(&e, field_bg), [0.0, 236.0, 116.0, 44.0], "the input line at the bottom, up to the list");
    assert_eq!(at(&e, text), [4.0, 236.0, 112.0, 44.0]);
    assert_eq!(at(&e, send), [120.0, 250.0, 76.0, 20.0], "SEND next to it, wide enough for its caption");
    // SEND's caption fills it; anything else in it stays.
    let caption = view(&mut e, "Label", 30, Some((2.0, 3.0, 30.0, 15.0)));
    let badge = view(&mut e, "ImageView", 31, Some((1.0, 1.0, 5.0, 5.0)));
    add(&mut e, send, caption);
    add(&mut e, send, badge);
    let r = e.vm.view_rect(send).unwrap().0;
    let rel = |e: &Env, v: Obj| {
        let c = e.vm.view_rect(v).unwrap().0;
        [c[0] - r[0], c[1] - r[1], c[2], c[3]]
    };
    assert_eq!((rel(&e, caption), rel(&e, badge)), ([0.0, 0.0, 76.0, 20.0], [1.0, 1.0, 4.0, 4.0]));
    // A message row: the picture at the left, the date small at the top right, the message under it.
    let content = view(&mut e, "View", 32, Some((0.0, 0.0, 120.0, 40.0)));
    let avatar = view(&mut e, "ImageView", 33, Some((0.0, 0.0, 30.0, 30.0)));
    let date = view(&mut e, "Label", 34, Some((40.0, -10.0, 120.0, 20.0)));
    let message = view(&mut e, "TextView", 35, Some((40.0, -2.0, 120.0, 45.0)));
    let other = view(&mut e, "View", 36, Some((1.0, 2.0, 3.0, 4.0)));
    add(&mut e, newest, content);
    for c in [avatar, date, message, other] {
        add(&mut e, content, c);
    }
    let r = e.vm.view_rect(content).unwrap().0;
    let inrow = |e: &Env, v: Obj| {
        let c = e.vm.view_rect(v).unwrap().0;
        [c[0] - r[0], c[1] - r[1], c[2], c[3]]
    };
    assert_eq!(inrow(&e, avatar), [4.0, 4.0, 36.0, 36.0]);
    assert_eq!(inrow(&e, date), [44.0, 2.0, 72.0, 16.0]);
    assert_eq!(inrow(&e, message), [44.0, 16.0, 72.0, 22.0]);
    assert_eq!(inrow(&e, other), [1.0, 2.0, 2.0, 2.0], "anything else stays");
    // Its text and the input line's are set smaller; the header's is not.
    let scale = |e: &Env, parent: Obj, child: Obj| {
        let st = e.vm.st();
        let v = &st.ui.views;
        crate::ui::text_scale(v, &v[&parent], &v[&child])
    };
    let small = crate::ui::CHAT_TEXT_SCALE;
    assert_eq!(
        [scale(&e, content, message), scale(&e, content, date), scale(&e, panel, text), scale(&e, header, title)],
        [small, small, small, 1.0]
    );
    assert_eq!((scale(&e, content, avatar), scale(&e, panel, field_bg)), (1.0, 1.0));
    // A row the game left under a later one is not shown (it is covered on the phone too).
    let stale = view(&mut e, "View", 37, Some((0.0, 40.0, 120.0, 70.0)));
    add(&mut e, scroll, stale);
    {
        // Under the later rows: first among the list's children.
        let mut st = e.vm.st();
        let kids = &mut st.ui.views.get_mut(&scroll).unwrap().children;
        kids.retain(|c| *c != stale);
        kids.insert(0, stale);
    }
    let sr = e.vm.view_rect(scroll).unwrap().0;
    assert!(e.vm.view_rect(stale).unwrap().0[1] < sr[1], "moved above the shown part");
    e.vm.ui_detach(stale);
    // An empty, flat text view next to the input line does not count as it.
    let flat = view(&mut e, "TextView", 38, Some((0.0, 0.0, 10.0, 0.0)));
    add(&mut e, panel, flat);
    assert_eq!(at(&e, text), [4.0, 236.0, 112.0, 44.0]);
    e.vm.ui_detach(flat);
    let s = at(&e, scroll)[1];
    assert_eq!((at(&e, newest)[1] - s, at(&e, older)[1] - s), (162.0, 122.0), "the newest message at the bottom");
    // touches find SEND where it is drawn; Return and the pause menu find the buttons
    let r = e.vm.view_rect(send).unwrap().0;
    assert_eq!(e.vm.hit_test(r[0] + 5.0, r[1] + 5.0).map(|h| h.view), Some(send));
    assert_eq!(e.vm.return_button(text), Some(send));
    assert_eq!(e.vm.chat_close_button(), Some((close, true)));
    // A player's press: a send that leaves the panel composing needs nothing more.
    let ret = e.code(&[0xE12F_FF1E]);
    let control = e.vm.class_named("com/apportable/ui/Control").unwrap();
    e.vm.register_native(control, "event", "(II)V", ret);
    let vm = e.vm.clone();
    vm.press_chat_button(&mut e.cpu, send).unwrap();
    assert_eq!(event_calls(&e), 2);
    // A send that took the game back to the list: the right-most button (NEW) is pressed, the input line focused.
    let before = vm.chat_buttons();
    e.call(text, "setHidden", "(Z)V", &[Value::Int(1)]).unwrap();
    vm.after_chat_press(&mut e.cpu, send, before).unwrap();
    assert_eq!((event_calls(&e), e.vm.text_focus()), (4, Some(text)));
    // CLOSE that took it back to the list: CLOSE again, the chat closes.
    e.call(text, "setHidden", "(Z)V", &[Value::Int(0)]).unwrap();
    let before = vm.chat_buttons();
    e.call(text, "setHidden", "(Z)V", &[Value::Int(1)]).unwrap();
    vm.after_chat_press(&mut e.cpu, close, before).unwrap();
    assert_eq!(event_calls(&e), 6);
    // Nothing more for another button, from the list, or when the panel still composes.
    let before = vm.chat_buttons();
    vm.after_chat_press(&mut e.cpu, close, before).unwrap();
    e.call(text, "setHidden", "(Z)V", &[Value::Int(0)]).unwrap();
    let before = vm.chat_buttons();
    vm.after_chat_press(&mut e.cpu, title, vm.chat_buttons()).unwrap();
    e.call(text, "setHidden", "(Z)V", &[Value::Int(1)]).unwrap();
    vm.after_chat_press(&mut e.cpu, title, before).unwrap();
    vm.after_chat_press(&mut e.cpu, close, None).unwrap();
    assert_eq!(event_calls(&e), 6);
    e.call(text, "setHidden", "(Z)V", &[Value::Int(0)]).unwrap();
    // scrolled by 20: the shown part moves
    let b = rect(&mut e, 0.0, 20.0, 120.0, 220.0);
    e.call(scroll, "setBounds", "(Landroid/graphics/RectF;)V", &[b]).unwrap();
    assert_eq!(at(&e, newest)[1] - s, 182.0);
    // the list without the input line (NEW instead of SEND): the messages reach the bottom, the header keeps NEW
    e.call(text, "setHidden", "(Z)V", &[Value::Int(1)]).unwrap();
    assert_eq!(at(&e, scroll), [4.0, 34.0, 192.0, 242.0]);
    assert_eq!(at(&e, send), [75.0, 5.0, 40.0, 20.0]);
    assert_eq!(e.vm.chat_close_button(), Some((close, false)));
    // The game takes the input line out while it lists: the panel is then known by its title.
    e.vm.ui_detach(text);
    e.vm.ui_detach(field_bg);
    assert_eq!(at(&e, scroll)[1], 72.0, "an untitled panel without an input line is no chat");
    let t = e.string("CHAT");
    e.call(title, "setText", "(Ljava/lang/String;)V", &[t]).unwrap();
    assert_eq!(at(&e, scroll), [4.0, 34.0, 192.0, 242.0]);
    let s = at(&e, scroll)[1];
    let b = rect(&mut e, 0.0, 0.0, 120.0, 200.0);
    e.call(scroll, "setBounds", "(Landroid/graphics/RectF;)V", &[b]).unwrap();
    assert_eq!(at(&e, newest)[1] - s, 202.0, "the newest still at the bottom");
    assert_eq!(e.vm.chat_close_button(), Some((close, false)));
    e.call(panel, "setHidden", "(Z)V", &[Value::Int(1)]).unwrap();
    assert_eq!(e.vm.chat_close_button(), None, "no chat shown");
    // a scroll view outside a chat panel keeps its order
    let list = view(&mut e, "ScrollView", 20, Some((300.0, 0.0, 400.0, 100.0)));
    let row = view(&mut e, "View", 21, Some((0.0, 0.0, 100.0, 20.0)));
    add(&mut e, root, list);
    add(&mut e, list, row);
    assert_eq!(e.vm.view_rect(row).unwrap().0[1], e.vm.view_rect(list).unwrap().0[1]);
}

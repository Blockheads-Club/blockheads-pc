//! Event translation and delivery into the fake JVM.
use crate::input::{args_for, coalesce_moves, host_event, host_key, JniSink};
use bhpc_app::{Event, Key, Metrics, Mods as AppMods, MouseButton as AB, TouchPhase};
use bhpc_input::{EventSink, GuestEvent, HostAction, HostEvent, HostKey, Lifecycle, Mods, MouseButton};
use bhpc_jni::{Value, VmConfig};
use std::sync::Arc;

#[test]
fn window_events_are_translated() {
    let m = Metrics::new((800, 600), (1600, 1200));
    assert_eq!(host_event(&Event::Quit), Some(HostEvent::Quit));
    assert_eq!(
        host_event(&Event::Resized(m)),
        Some(HostEvent::Resized { width: 1600, height: 1200, logical_width: 800, logical_height: 600 })
    );
    assert_eq!(host_event(&Event::Focus(true)), Some(HostEvent::FocusGained));
    assert_eq!(host_event(&Event::Focus(false)), Some(HostEvent::FocusLost));
    assert_eq!(host_event(&Event::Visible(false)), Some(HostEvent::Minimized));
    assert_eq!(host_event(&Event::Visible(true)), Some(HostEvent::Restored));
    assert_eq!(host_event(&Event::Fullscreen(true)), None);
    assert_eq!(host_event(&Event::Touch { id: 1, phase: TouchPhase::Down, x: 1.0, y: 2.0 }), None);
    assert_eq!(host_event(&Event::MouseMove { x: 1.0, y: 2.0 }), Some(HostEvent::MouseMove { x: 1.0, y: 2.0 }));
    for (b, want) in [(AB::Left, MouseButton::Left), (AB::Right, MouseButton::Right), (AB::Middle, MouseButton::Middle)]
    {
        assert_eq!(
            host_event(&Event::MouseButton { button: b, pressed: true, x: 3.0, y: 4.0 }),
            Some(HostEvent::MouseButton { button: want, pressed: true, x: 3.0, y: 4.0, mods: Mods::default() })
        );
    }
    assert_eq!(host_event(&Event::MouseButton { button: AB::Other(4), pressed: true, x: 0.0, y: 0.0 }), None);
    assert_eq!(
        host_event(&Event::Wheel { dx: 0.0, dy: 2.0 }),
        Some(HostEvent::Wheel { dy: 2.0, x: 0.0, y: 0.0, mods: bhpc_input::Mods::default() })
    );
    let key =
        Event::Key { key: Key(4), pressed: true, repeat: false, mods: AppMods { shift: true, ctrl: false, alt: true } };
    assert_eq!(
        host_event(&key),
        Some(HostEvent::Key {
            key: HostKey::Char('a'),
            pressed: true,
            repeat: false,
            mods: Mods { ctrl: false, shift: true, alt: true }
        })
    );
    assert_eq!(host_event(&Event::Text("é".into())), Some(HostEvent::TextInput("é".into())));
    use bhpc_input::{PadAxis, PadButton, PadEvent};
    assert_eq!(host_event(&Event::PadAdded(4)), Some(HostEvent::Pad(PadEvent::Added(4))));
    assert_eq!(host_event(&Event::PadRemoved(4)), Some(HostEvent::Pad(PadEvent::Removed(4))));
    assert_eq!(
        host_event(&Event::PadButton { id: 4, button: 1, pressed: true }),
        Some(HostEvent::Pad(PadEvent::Button { pad: 4, button: PadButton::B, pressed: true }))
    );
    assert_eq!(
        host_event(&Event::PadAxis { id: 4, axis: 2, value: 32767 }),
        Some(HostEvent::Pad(PadEvent::Axis { pad: 4, axis: PadAxis::RightX, value: 1.0 }))
    );
}

#[test]
fn hid_codes_map_to_keys() {
    let k = |n| host_key(Key(n));
    assert_eq!(
        (k(4), k(29), k(30), k(38), k(39)),
        (HostKey::Char('a'), HostKey::Char('z'), HostKey::Char('1'), HostKey::Char('9'), HostKey::Char('0'))
    );
    assert_eq!(
        [k(40), k(41), k(42), k(43), k(44), k(68), k(74), k(76), k(77), k(79), k(80), k(81), k(82), k(200)],
        [
            HostKey::Enter,
            HostKey::Escape,
            HostKey::Backspace,
            HostKey::Tab,
            HostKey::Space,
            HostKey::F(11),
            HostKey::Home,
            HostKey::Delete,
            HostKey::End,
            HostKey::Right,
            HostKey::Left,
            HostKey::Down,
            HostKey::Up,
            HostKey::Other
        ]
    );
}

fn rig() -> (Box<bhpc_cpu::Cpu>, bhpc_jni::Jni) {
    let (cpu, st) = crate::new_env(Box::new(std::io::sink()), None).unwrap();
    let cfg = VmConfig::new(bhpc_jni::Platform::default(), Box::new(bhpc_jni::MemAssets::default()));
    let jni = bhpc_jni::install(&cpu, cpu.mem().clone(), Box::new(crate::apkrun::BionicHeap(st)), cfg).unwrap();
    (cpu, jni)
}

#[test]
fn arguments_follow_the_java_signatures() {
    let (_cpu, jni) = rig();
    let vm = &jni.vm;
    let begin = GuestEvent::TouchBegin { id: 3, x: 1.5, y: 2.5, time_ms: 9 };
    assert_eq!(args_for(vm, &begin, 0), [Value::Int(3), Value::Float(1.5), Value::Float(2.5), Value::Long(9)]);
    let end = GuestEvent::TouchEnd { id: 3, x: 1.5, y: 2.5, time_ms: 9 };
    assert_eq!(args_for(vm, &end, 0).len(), 4);
    let mv = GuestEvent::TouchMove { ids: vec![0, 1], xs: vec![1.0, 2.0], ys: vec![3.0, 4.0], time_ms: 5 };
    let a = args_for(vm, &mv, 0);
    let arr = a[0].as_ref().unwrap();
    assert_eq!(vm.array_len(arr), Some(2));
    assert_eq!(a[3], Value::Long(5));
    let cancel = GuestEvent::TouchCancel { ids: vec![0], xs: vec![1.0], ys: vec![1.0], time_ms: 1 };
    assert_eq!(args_for(vm, &cancel, 0).len(), 4);
    let key = GuestEvent::KeyDown { key_code: 4, unicode: 0, meta: 1, time_ms: 7 };
    assert_eq!(args_for(vm, &key, 0), [Value::Int(4), Value::Int(0), Value::Long(7), Value::Int(1)]);
    let up = GuestEvent::KeyUp { key_code: 4, unicode: 0, meta: 1, time_ms: 7 };
    assert_eq!(args_for(vm, &up, 0).len(), 4);
    assert_eq!(args_for(vm, &GuestEvent::WindowFocus(true), 0), [Value::Int(1)]);
    let sc = GuestEvent::SurfaceChanged { width: 640, height: 480 };
    assert_eq!(args_for(vm, &sc, 0), [Value::Int(640), Value::Int(480)]);
    assert!(args_for(vm, &sc, 1).is_empty());
    assert!(args_for(vm, &GuestEvent::Lifecycle(Lifecycle::DidBecomeActive), 0).is_empty());
    let t = GuestEvent::TextShouldChange { field: 2, location: 1, length: 0, replacement: "x".into() };
    assert_eq!(args_for(vm, &t, 0).len(), 4);
    assert_eq!(args_for(vm, &GuestEvent::TextDidChange { field: 2 }, 0), [Value::Int(2)]);
    assert_eq!(args_for(vm, &GuestEvent::TextShouldReturn { field: 2 }, 0), [Value::Int(2)]);
}

#[test]
fn delivery_calls_natives_and_collects_errors() {
    let (mut cpu, jni) = rig();
    let calls = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let c = calls.clone();
    jni.vm.set_log(move |m| c.lock().unwrap().push(m.to_string()));
    jni.set_trace_calls(true);
    let mut sink = JniSink {
        vm: &jni.vm,
        cpu: &mut cpu,
        actions: Vec::new(),
        errors: Vec::new(),
        returned: false,
        main: None,
        game: None,
    };
    // No guest is loaded, so the natives are reported stubs that answer 0/void.
    assert!(!sink.deliver(&GuestEvent::KeyDown { key_code: 4, unicode: 0, meta: 0, time_ms: 1 }), "boolean 0");
    assert!(sink.deliver(&GuestEvent::TouchBegin { id: 0, x: 0.0, y: 0.0, time_ms: 1 }), "void is accepted");
    assert!(sink.deliver(&GuestEvent::SurfaceChanged { width: 1, height: 1 }), "static + activity instance native");
    sink.host_action(HostAction::Quit);
    assert_eq!(sink.actions, [HostAction::Quit]);
    assert!(sink.errors.is_empty(), "{:?}", sink.errors);
    let log = calls.lock().unwrap().clone();
    assert!(log.iter().any(|m| m.contains("Window.nativeKeyDown")), "{log:?}");
    assert!(log.iter().any(|m| m.contains("VerdeActivity.nativeRootGeometryChanged")), "{log:?}");
    // A native that stops the guest comes back as an error string.
    jni.vm.define("com/apportable/ui/Window").static_method("nativeOnWindowFocusChanged", "(Z)V", |_| {
        Err(bhpc_jni::Throw::Stop(bhpc_cpu::Stop::Host("boom".into())))
    });
    assert!(sink.deliver(&GuestEvent::WindowFocus(true)));
    assert_eq!(sink.errors.len(), 1);
    assert!(sink.errors[0].contains("nativeOnWindowFocusChanged"), "{:?}", sink.errors);
}

#[test]
fn single_finger_touches_reach_the_control_layer() {
    use crate::input::control_touch;
    let one = |ids: Vec<i32>| (ids, vec![1.0], vec![2.0]);
    let (ids, xs, ys) = one(vec![0]);
    assert_eq!(
        control_touch(&GuestEvent::TouchMove { ids: ids.clone(), xs: xs.clone(), ys: ys.clone(), time_ms: 0 }),
        Some((bhpc_jni::TouchPhase::Move, 1.0, 2.0))
    );
    assert_eq!(
        control_touch(&GuestEvent::TouchCancel { ids, xs, ys, time_ms: 0 }),
        Some((bhpc_jni::TouchPhase::Cancel, 1.0, 2.0))
    );
    assert_eq!(
        control_touch(&GuestEvent::TouchEnd { id: 0, x: 3.0, y: 4.0, time_ms: 0 }),
        Some((bhpc_jni::TouchPhase::End, 3.0, 4.0))
    );
    let two = GuestEvent::TouchMove { ids: vec![0, 1], xs: vec![1.0, 2.0], ys: vec![1.0, 2.0], time_ms: 0 };
    assert_eq!(control_touch(&two), None);
    // The controller's walking finger alone is not pointer 0.
    let walk = GuestEvent::TouchMove { ids: vec![2], xs: vec![1.0], ys: vec![1.0], time_ms: 0 };
    assert_eq!(control_touch(&walk), None);
    assert_eq!(control_touch(&GuestEvent::TouchBegin { id: 1, x: 0.0, y: 0.0, time_ms: 0 }), None);
}

#[test]
fn controls_take_touches_and_text_goes_to_the_focused_field() {
    let (mut cpu, jni) = rig();
    let vm = jni.vm.clone();
    let ctx = Value::Ref(Some(vm.activity()));
    let rect = vm
        .new_object(
            &mut cpu,
            "android/graphics/RectF",
            "(FFFF)V",
            &[Value::Float(0.0), Value::Float(0.0), Value::Float(5000.0), Value::Float(5000.0)],
        )
        .unwrap();
    let make = |cpu: &mut bhpc_cpu::Cpu, kind: &str, handle: i32| {
        let class = format!("com/apportable/ui/{kind}");
        let sig = format!("(Landroid/content/Context;ILandroid/graphics/RectF;)L{class};");
        let args = [ctx, Value::Int(handle), Value::Ref(Some(rect))];
        vm.call_static(cpu, &class, "create", &sig, &args).unwrap().as_ref().unwrap()
    };
    let window = make(&mut cpu, "Window", 1);
    let button = make(&mut cpu, "Button", 2);
    let field = make(&mut cpu, "TextField", 3);
    for child in [field, button] {
        vm.call_method(&mut cpu, window, "addSubview", "(Lcom/apportable/ui/View;)V", &[Value::Ref(Some(child))])
            .unwrap();
    }
    let mut sink = JniSink {
        vm: &vm,
        cpu: &mut cpu,
        actions: Vec::new(),
        errors: Vec::new(),
        returned: false,
        main: None,
        game: None,
    };
    // A press on the button is swallowed by the control layer.
    assert!(sink.deliver(&GuestEvent::TouchBegin { id: 0, x: 10.0, y: 10.0, time_ms: 0 }));
    assert!(sink.deliver(&GuestEvent::TouchEnd { id: 0, x: 10.0, y: 10.0, time_ms: 0 }));
    assert!(sink.errors.is_empty(), "{:?}", sink.errors);
    // A failing control event is reported and the touch still goes on to the game.
    vm.define("com/apportable/ui/Button")
        .method("event", "(II)V", |_| Err(bhpc_jni::Throw::Stop(bhpc_cpu::Stop::Host("no".into()))));
    sink.deliver(&GuestEvent::TouchBegin { id: 0, x: 10.0, y: 10.0, time_ms: 0 });
    assert!(sink.errors.iter().any(|e| e.starts_with("ui touch")), "{:?}", sink.errors);
    sink.errors.clear();
    // Text events need a focused field.
    let change = GuestEvent::TextShouldChange { field: 0, location: 0, length: 0, replacement: "hi".into() };
    assert!(!sink.deliver(&change), "no focus");
    assert_eq!(sink.field_text(), None);
    vm.set_text_focus(Some(field));
    let t = change.targets()[0];
    vm.define(t.class).method(t.method, t.signature, |_| Ok(Value::Int(1)));
    assert!(sink.deliver(&change));
    assert_eq!(vm.text_of(field), "hi");
    assert_eq!(sink.field_text().as_deref(), Some("hi"), "the mapper reads the guest's text back");
    assert!(!sink.returned);
    sink.deliver(&GuestEvent::TextShouldReturn { field: 0 });
    assert!(sink.returned);
}

#[test]
fn a_text_view_gets_the_text_view_natives_and_its_return_key_presses_the_send_button() {
    use std::sync::Mutex;
    let (mut cpu, jni) = rig();
    let vm = jni.vm.clone();
    let ctx = Value::Ref(Some(vm.activity()));
    let rect = vm
        .new_object(
            &mut cpu,
            "android/graphics/RectF",
            "(FFFF)V",
            &[Value::Float(0.0), Value::Float(0.0), Value::Float(100.0), Value::Float(50.0)],
        )
        .unwrap();
    let make = |cpu: &mut bhpc_cpu::Cpu, kind: &str, handle: i32| {
        let class = format!("com/apportable/ui/{kind}");
        let sig = format!("(Landroid/content/Context;ILandroid/graphics/RectF;)L{class};");
        let args = [ctx, Value::Int(handle), Value::Ref(Some(rect))];
        vm.call_static(cpu, &class, "create", &sig, &args).unwrap().as_ref().unwrap()
    };
    let panel = make(&mut cpu, "View", 1);
    let text = make(&mut cpu, "TextView", 7);
    let send = make(&mut cpu, "Button", 8);
    for child in [text, send] {
        vm.call_method(&mut cpu, panel, "addSubview", "(Lcom/apportable/ui/View;)V", &[Value::Ref(Some(child))])
            .unwrap();
    }
    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    let (l1, l2, l3) = (log.clone(), log.clone(), log.clone());
    let answer = Arc::new(std::sync::atomic::AtomicI32::new(1));
    let a = answer.clone();
    vm.define("com/apportable/ui/TextView")
        .method("textShouldChange", "(ILjava/lang/String;II)Z", move |c| {
            let t = c.string(1).unwrap_or_default();
            l1.lock().unwrap().push(format!("should {} {t:?} {} {}", c.int(0), c.int(2), c.int(3)));
            Ok(Value::Int(a.load(std::sync::atomic::Ordering::SeqCst)))
        })
        .method("textDidChange", "(ILjava/lang/String;)V", move |c| {
            let t = c.string(1).unwrap_or_default();
            l2.lock().unwrap().push(format!("did {} {t:?}", c.int(0)));
            Ok(Value::Int(0))
        });
    vm.define("com/apportable/ui/Button").method("event", "(II)V", move |c| {
        l3.lock().unwrap().push(format!("button {} {}", c.int(0), c.int(1)));
        Ok(Value::Int(0))
    });
    let mut sink = JniSink {
        vm: &vm,
        cpu: &mut cpu,
        actions: Vec::new(),
        errors: Vec::new(),
        returned: false,
        main: None,
        game: None,
    };
    vm.set_text_focus(Some(text));
    // Typed text asks the input filter (handle, the typed text, its start and length); the guest may refuse.
    let typed = GuestEvent::TextShouldChange { field: 7, location: 3, length: 0, replacement: "ğ".into() };
    assert!(sink.deliver(&typed));
    assert_eq!(vm.text_of(text), "ğ");
    answer.store(0, std::sync::atomic::Ordering::SeqCst);
    assert!(!sink.deliver(&typed));
    assert_eq!(vm.text_of(text), "ğ", "refused");
    answer.store(1, std::sync::atomic::Ordering::SeqCst);
    // The text watcher reports the whole text.
    sink.deliver(&GuestEvent::TextDidChange { field: 7 });
    // The return key asks about and reports the whole text, then presses the button that sends it.
    sink.deliver(&GuestEvent::TextShouldReturn { field: 7 });
    assert!(sink.errors.is_empty(), "{:?}", sink.errors);
    assert_eq!(
        *log.lock().unwrap(),
        [
            "should 7 \"ğ\" 0 1",
            "should 7 \"ğ\" 0 1",
            "did 7 \"ğ\"",
            "should 7 \"ğ\" 0 1",
            "did 7 \"ğ\"",
            "button 8 1",
            "button 8 64"
        ]
    );
    // Events that are not text events mean nothing to a text view, and failing natives are reported.
    vm.define("com/apportable/ui/TextView").method("textDidChange", "(ILjava/lang/String;)V", |_| {
        Err(bhpc_jni::Throw::Stop(bhpc_cpu::Stop::Host("no".into())))
    });
    vm.define("com/apportable/ui/Button")
        .method("event", "(II)V", |_| Err(bhpc_jni::Throw::Stop(bhpc_cpu::Stop::Host("no".into()))));
    sink.deliver(&GuestEvent::TextShouldReturn { field: 7 });
    assert!(sink.errors.iter().any(|e| e.starts_with("TextView.textDidChange")), "{:?}", sink.errors);
    assert!(sink.errors.iter().any(|e| e.starts_with("return button")), "{:?}", sink.errors);
    // without a button for it the return key only reports the text
    let alone = make(&mut *sink.cpu, "TextView", 9);
    assert!(sink.vm.return_button(alone).is_none());
    sink.errors.clear();
    vm.set_text_focus(Some(alone));
    sink.deliver(&GuestEvent::TextShouldReturn { field: 9 });
    assert!(sink.errors.iter().all(|e| !e.starts_with("return button")), "{:?}", sink.errors);
}

#[test]
fn control_touches_and_clicks_cross_the_content_surface() {
    use crate::input::{content_to_screen, screen_to_window};
    use bhpc_input::Geometry;
    // The game picked 1024x768 content for a 1280x960 screen: views are 1.25 times the content.
    assert_eq!(content_to_screen((1.25, 1.25), 100.0, 200.0), (125.0, 250.0));
    // The window is the same size as the screen: a view position is a window position.
    let g = Geometry { content: Some((1024.0, 768.0)), window: (1280, 960) };
    assert_eq!(screen_to_window(&g, (1.25, 1.25), 625.0, 500.0), (625.0, 500.0));
    // A wider window letterboxes: the content centre stays the window centre.
    let wide = Geometry { content: Some((1024.0, 768.0)), window: (1920, 960) };
    assert_eq!(screen_to_window(&wide, (1.25, 1.25), 640.0, 480.0), (960.0, 480.0));
}

#[test]
fn keys_and_controller_buttons_steer_a_dialog() {
    use crate::input::dialog_input;
    use bhpc_jni::DialogNav as N;
    let key = |code: u32, pressed: bool, shift: bool| Event::Key {
        key: Key(code),
        pressed,
        repeat: false,
        mods: AppMods { shift, ..AppMods::default() },
    };
    let pad = |button: u8, pressed: bool| Event::PadButton { id: 0, button, pressed };
    // Buttons only: Enter/keypad Enter/Space press the focused button, Escape and Back cancel, arrows and Tab move.
    for (code, shift, want) in [
        (40, false, Some(N::Accept)),
        (88, false, Some(N::Accept)),
        (44, false, Some(N::Accept)),
        (41, false, Some(N::Cancel)),
        (80, false, Some(N::Prev)),
        (82, false, Some(N::Prev)),
        (79, false, Some(N::Next)),
        (81, false, Some(N::Next)),
        (43, false, Some(N::Next)),
        (43, true, Some(N::Prev)),
        (4, false, None),
    ] {
        assert_eq!(dialog_input(&key(code, true, shift), false, false), Some(want), "key {code}");
    }
    assert_eq!(dialog_input(&key(42, true, false), false, true), Some(Some(N::Cancel)), "the Back binding");
    assert_eq!(dialog_input(&key(40, false, false), false, false), None, "releases go on");
    assert_eq!(dialog_input(&Event::Text("a".into()), false, false), Some(None));
    // With an input line the keys belong to the field.
    assert_eq!(dialog_input(&key(80, true, false), true, false), None);
    assert_eq!(dialog_input(&Event::Text("a".into()), true, false), None);
    // The controller steers both kinds.
    for (button, want) in
        [(0, Some(N::Accept)), (1, Some(N::Cancel)), (11, Some(N::Prev)), (13, Some(N::Prev)), (12, Some(N::Next))]
            .into_iter()
            .chain([(14, Some(N::Next)), (6, None)])
    {
        assert_eq!(dialog_input(&pad(button, true), true, false), Some(want), "button {button}");
    }
    assert_eq!(dialog_input(&pad(0, false), false, false), None);
    assert_eq!(dialog_input(&Event::MouseMove { x: 1.0, y: 1.0 }, false, false), None);

    // The releases of what the dialog took stay with it, also after it closed; other releases go on.
    let mut f = crate::input::DialogInput::default();
    assert_eq!(f.filter(&key(41, true, false), None, false), None, "no dialog");
    assert_eq!(f.filter(&key(41, false, false), None, false), None);
    assert_eq!(f.filter(&key(41, true, false), Some(false), false), Some(Some(N::Cancel)));
    assert_eq!(f.filter(&pad(1, true), Some(false), false), Some(Some(N::Cancel)));
    assert_eq!(f.filter(&Event::MouseMove { x: 1.0, y: 1.0 }, Some(false), false), None);
    assert_eq!(f.filter(&Event::Text("a".into()), Some(false), false), Some(None));
    assert_eq!(f.filter(&key(41, false, false), None, false), Some(None), "swallowed");
    assert_eq!(f.filter(&key(41, false, false), None, false), None, "once");
    assert_eq!(f.filter(&pad(1, false), Some(false), false), Some(None));
    assert_eq!(f.filter(&key(4, false, false), Some(false), false), None, "pressed before the dialog");
}

#[test]
fn coalesce_keeps_the_newest_move_of_each_burst() {
    let mv = |x: f32| Event::MouseMove { x, y: 0.0 };
    let btn = |pressed| Event::MouseButton { button: AB::Left, pressed, x: 0.0, y: 0.0 };
    assert_eq!(coalesce_moves(vec![]), vec![]);
    assert_eq!(coalesce_moves(vec![mv(1.0), mv(2.0), mv(3.0)]), vec![mv(3.0)]);
    // Moves around a button stay on their side of it.
    assert_eq!(
        coalesce_moves(vec![mv(1.0), mv(2.0), btn(true), mv(3.0), mv(4.0), btn(false), mv(5.0)]),
        vec![mv(2.0), btn(true), mv(4.0), btn(false), mv(5.0)]
    );
}

#[test]
fn touch_key_and_text_natives_run_on_the_main_thread_queue() {
    use crate::input::runs_on_main;
    use crate::mainq::MainQueue;
    use std::sync::OnceLock;
    assert!(runs_on_main(&GuestEvent::TouchBegin { id: 0, x: 0.0, y: 0.0, time_ms: 0 }));
    assert!(runs_on_main(&GuestEvent::KeyDown { key_code: 4, unicode: 0, meta: 0, time_ms: 0 }));
    assert!(!runs_on_main(&GuestEvent::WindowFocus(true)));
    assert!(!runs_on_main(&GuestEvent::SurfaceChanged { width: 1, height: 1 }));
    assert!(!runs_on_main(&GuestEvent::Lifecycle(Lifecycle::DidBecomeActive)));
    let (mut cpu, jni) = rig();
    // The "main thread" is whoever waits: the poster drains the queue itself while it waits.
    let cell: Arc<OnceLock<Arc<MainQueue>>> = Arc::new(OnceLock::new());
    let c2 = cell.clone();
    let q = MainQueue::new(move |c| c2.get().unwrap().drain(c), 5);
    cell.set(q.clone()).ok();
    let game = std::sync::Arc::new(crate::gameui::GameUi::default());
    let mut sink = JniSink {
        vm: &jni.vm,
        cpu: &mut cpu,
        actions: Vec::new(),
        errors: Vec::new(),
        returned: false,
        main: Some(q),
        game: Some(game.clone()),
    };
    assert!(sink.deliver(&GuestEvent::TouchBegin { id: 0, x: 0.0, y: 0.0, time_ms: 1 }));
    assert!(!sink.deliver(&GuestEvent::KeyDown { key_code: 4, unicode: 0, meta: 0, time_ms: 1 }));
    // Surface events stay on the calling thread.
    assert!(sink.deliver(&GuestEvent::SurfaceChanged { width: 1, height: 1 }));
    assert!(sink.errors.is_empty(), "{:?}", sink.errors);
    // The walking finger is announced to the game's controls before the game gets it.
    game.toggle();
    sink.deliver(&GuestEvent::TouchBegin { id: 2, x: 0.0, y: 0.0, time_ms: 2 });
    assert!(!game.suppress(true));
}

#[test]
fn control_events_run_on_the_main_thread_queue() {
    // A button's `Control.event` sends the game's UIKit actions (the tutorial popup's OK unpauses the world): with
    // parallel guest threads they must not run on the UI thread in the middle of the main thread's frame.
    use crate::mainq::MainQueue;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, OnceLock};
    let (mut cpu, jni) = rig();
    let vm = jni.vm.clone();
    let ctx = Value::Ref(Some(vm.activity()));
    let rect = vm
        .new_object(
            &mut cpu,
            "android/graphics/RectF",
            "(FFFF)V",
            &[Value::Float(0.0), Value::Float(0.0), Value::Float(5000.0), Value::Float(5000.0)],
        )
        .unwrap();
    let make = |cpu: &mut bhpc_cpu::Cpu, kind: &str| {
        let class = format!("com/apportable/ui/{kind}");
        let sig = format!("(Landroid/content/Context;ILandroid/graphics/RectF;)L{class};");
        vm.call_static(cpu, &class, "create", &sig, &[ctx, Value::Int(1), Value::Ref(Some(rect))])
            .unwrap()
            .as_ref()
            .unwrap()
    };
    let window = make(&mut cpu, "Window");
    let button = make(&mut cpu, "Button");
    vm.call_method(&mut cpu, window, "addSubview", "(Lcom/apportable/ui/View;)V", &[Value::Ref(Some(button))]).unwrap();
    // The queue's "main thread" drains while the poster waits; the button records where its events ran.
    static DRAINING: AtomicBool = AtomicBool::new(false);
    static RAN: Mutex<Vec<bool>> = Mutex::new(Vec::new());
    vm.define("com/apportable/ui/Button").method("event", "(II)V", |_| {
        RAN.lock().unwrap().push(DRAINING.load(Ordering::SeqCst));
        Ok(Value::Void)
    });
    let cell: Arc<OnceLock<Arc<MainQueue>>> = Arc::new(OnceLock::new());
    let c2 = cell.clone();
    let q = MainQueue::new(
        move |c| {
            DRAINING.store(true, Ordering::SeqCst);
            c2.get().unwrap().drain(c);
            DRAINING.store(false, Ordering::SeqCst);
        },
        5,
    );
    cell.set(q.clone()).ok();
    let mut sink = JniSink {
        vm: &vm,
        cpu: &mut cpu,
        actions: Vec::new(),
        errors: Vec::new(),
        returned: false,
        main: Some(q),
        game: None,
    };
    assert!(sink.deliver(&GuestEvent::TouchBegin { id: 0, x: 10.0, y: 10.0, time_ms: 0 }));
    assert!(sink.deliver(&GuestEvent::TouchEnd { id: 0, x: 10.0, y: 10.0, time_ms: 0 }));
    assert!(sink.errors.is_empty(), "{:?}", sink.errors);
    assert_eq!(*RAN.lock().unwrap(), [true, true], "down and up-inside both ran on the main thread");
    // A main thread that never drains (no frames, no kick): the poster runs the event itself in the end.
    sink.main = Some(MainQueue::new(|_| {}, 1));
    assert!(sink.deliver(&GuestEvent::TouchBegin { id: 0, x: 10.0, y: 10.0, time_ms: 0 }));
    assert_eq!(*RAN.lock().unwrap(), [true, true, false]);
    // The stale queue entry is a no-op once the poster ran the event itself.
    sink.main.as_ref().unwrap().drain(sink.cpu);
    assert_eq!(RAN.lock().unwrap().len(), 3);
}

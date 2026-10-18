//! Event translation and delivery into the fake JVM.
use crate::input::{args_for, host_event, host_key, JniSink};
use bhpc_app::{Event, Key, Metrics, Mods as AppMods, MouseButton as AB, TouchPhase};
use bhpc_input::{EventSink, GuestEvent, HostAction, HostEvent, HostKey, Lifecycle, Mods, MouseButton};
use bhpc_jni::{Value, VmConfig};
use std::sync::Arc;

#[test]
fn window_events_are_translated() {
    let m = Metrics::new((800, 600), (1600, 1200));
    assert_eq!(host_event(&Event::Quit), Some(HostEvent::Quit));
    assert_eq!(host_event(&Event::Resized(m)), Some(HostEvent::Resized { width: 1600, height: 1200 }));
    assert_eq!(host_event(&Event::Focus(true)), Some(HostEvent::FocusGained));
    assert_eq!(host_event(&Event::Focus(false)), Some(HostEvent::FocusLost));
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
    assert_eq!(host_event(&Event::Wheel { dx: 0.0, dy: 2.0 }), Some(HostEvent::Wheel { dy: 2.0, x: 0.0, y: 0.0 }));
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
            HostKey::F11,
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
    let mut sink = JniSink { vm: &jni.vm, cpu: &mut cpu, actions: Vec::new(), errors: Vec::new(), returned: false };
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

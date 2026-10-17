use crate::tests::{test_platform, Env};
use crate::*;
use bhpc_cpu::Stop;
use std::sync::{Arc, Mutex};

#[test]
fn native_env_lists_what_the_game_reads() {
    let env = native_env(&test_platform());
    let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_else(|| panic!("{k}"));
    assert_eq!(get("LOCALE"), "tr_TR");
    assert_eq!(get("LANGUAGES"), "tr");
    assert_eq!(get("TZ"), "Europe/Istanbul");
    assert_eq!(get("PACKAGE_CODE_PATH"), "/tmp/test.apk");
    assert_eq!(get("PACKAGE_RESOURCE_PATH"), "/tmp/test.apk");
    assert_eq!(get("PACKAGE_NAME"), "com.majicjungle.blockheads");
    assert_eq!(get("SDK_INT"), "19");
    assert_eq!(get("DEBUG_BUILD"), "0");
    assert_eq!(get("apportable.opengles2"), "true");
    assert_eq!(get("apportable.opengles.fast_color"), "true");
    assert!(get("HOME").contains("bhpc-jni"));
    assert_eq!(get("TMPDIR"), Platform::default().cache_dir);
    assert!(get("EXTERNAL_CACHEDIR").ends_with("/sdcard/cache"));
    assert!(get("ANDROID_DOWNLOAD_DIRECTORY").ends_with("/sdcard/Download"));
    let mut names: Vec<&str> = env.iter().map(|(n, _)| n.as_str()).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), env.len(), "no duplicates");
}

#[test]
fn stages_map_to_natives() {
    let all = [
        Stage::LibraryInit,
        Stage::ActivityCreate,
        Stage::ActivityStart,
        Stage::ActivityResume,
        Stage::ActivityPause,
        Stage::ActivityStop,
        Stage::ActivityDestroy,
        Stage::ActivityLowMemory,
        Stage::RootGeometry,
        Stage::SurfaceInit,
        Stage::SurfaceChanged,
        Stage::ContextValid,
        Stage::ContextInvalid,
        Stage::WillEnterForeground,
        Stage::DidBecomeActive,
        Stage::WillResignActive,
        Stage::DidEnterBackground,
        Stage::WillTerminate,
        Stage::MemoryWarning,
        Stage::MainThreadRun,
    ];
    let e = Env::new();
    for s in all {
        let (class, name, sig, is_static) = s.target();
        // Every stage names a method the fake class library declares as a native.
        let c = e.vm.class_named(class).unwrap_or_else(|| panic!("{class}"));
        let m = e.vm.method_id(c, name, sig, is_static).unwrap();
        let (_, st, _, _, _) = e.vm.method_info(m).unwrap();
        assert_eq!(st, is_static, "{name}");
        assert!(!e.vm.stage_bound(s));
    }
    assert!(e.vm.report().stub_classes.is_empty());
}

#[test]
fn stages_run_bound_natives_and_report_unbound_ones() {
    let mut e = Env::new();
    // A native that writes its argument r1 (class/this handle) to scratch so we can see the receiver kind.
    let record = e.code(&[0xE12F_FF1E]);
    let vm = e.vm.clone();
    let lc = vm.class_named("com/apportable/Lifecycle").unwrap();
    vm.register_native(lc, "contextDidBecomeValid", "()V", record);
    assert!(vm.stage_bound(Stage::ContextValid));
    vm.run_stage(&mut e.cpu, Stage::ContextValid).unwrap();
    // Unbound stage: a stub call that still succeeds and is visible in the report.
    vm.run_stage(&mut e.cpu, Stage::ActivityCreate).unwrap();
    assert!(vm.report().stubs().any(|m| m.name == "nativeOnCreate" && m.calls == 1));
    // Instance native on the activity.
    let act = vm.class_named("com/apportable/activity/VerdeActivity").unwrap();
    vm.register_native(act, "nativeRootGeometryChanged", "()V", record);
    assert!(vm.stage_bound(Stage::RootGeometry));
    vm.run_stage(&mut e.cpu, Stage::RootGeometry).unwrap();
    assert_eq!(vm.activity(), e.activity());
    // The surface view natives run on their own receiver; the size is the screen size.
    let gl = vm.class_named("com/apportable/gl/GLSurfaceView").unwrap();
    vm.register_native(gl, "nativeInit", "()V", record);
    vm.register_native(gl, "nativeOnSurfaceChanged", "(II)V", record);
    vm.run_stage(&mut e.cpu, Stage::SurfaceInit).unwrap();
    vm.run_stage(&mut e.cpu, Stage::SurfaceChanged).unwrap();
    assert!(!vm.stage_bound(Stage::MainThreadRun));
    // Exports found by the symbol resolver count as bound, short or overloaded name.
    vm.set_symbol_resolver(|n| (n == "Java_com_apportable_MainThread_nativeRun").then_some(0x1000));
    assert!(vm.stage_bound(Stage::MainThreadRun));
    assert!(!vm.stage_bound(Stage::ActivityPause));
    vm.set_symbol_resolver(|n| (n == "Java_com_apportable_activity_VerdeActivity_nativeOnPause__").then_some(0x1000));
    assert!(vm.stage_bound(Stage::ActivityPause));
    // A faulting native stops the run.
    let fault = e.code(&[0xE3A0_0000, 0xE580_0000, 0xE12F_FF1E]);
    let mt = vm.class_named("com/apportable/MainThread").unwrap();
    vm.register_native(mt, "nativeRun", "()V", fault);
    assert!(matches!(vm.run_stage(&mut e.cpu, Stage::MainThreadRun), Err(Throw::Stop(Stop::Fault(_)))));
}

#[test]
fn set_native_env_calls_setenv_for_each_pair() {
    let mut e = Env::new();
    let vm = e.vm.clone();
    // setenv(env, cls, name, value) stub: counts calls by jumping to a thunk would need libc;
    // use a host-implemented replacement instead.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    vm.define("com/apportable/LibraryLoader").static_method(
        "setenv",
        "(Ljava/lang/String;Ljava/lang/String;)V",
        move |c| {
            s2.lock().unwrap().push((c.string(0).unwrap(), c.string(1).unwrap()));
            Ok(Value::Void)
        },
    );
    let vars = native_env(&test_platform());
    assert_eq!(vm.set_native_env(&mut e.cpu, &vars), Ok(vars.len()));
    assert_eq!(*seen.lock().unwrap(), vars);
    // An exception from setenv aborts.
    vm.define("com/apportable/LibraryLoader").static_method("setenv", "(Ljava/lang/String;Ljava/lang/String;)V", |c| {
        Err(c.vm.throw_new("java/lang/IllegalStateException", "no"))
    });
    assert!(vm.set_native_env(&mut e.cpu, &vars).is_err());
}

#[test]
fn onload_runs_with_the_java_vm_pointer() {
    let mut e = Env::new();
    let vm = e.vm.clone();
    let jv = e.layout.java_vm;
    // JNI_OnLoad(vm, reserved) that returns its first argument, so the host can see what it got.
    let echo = e.code(&[0xE12F_FF1E]);
    assert_eq!(vm.call_onload(&mut e.cpu, echo), Ok(jv));
    // Without install the JavaVM pointer is 0.
    let mem = e.mem();
    let heap = bhpc_mem::GuestHeap::new(mem.clone(), 0x6000_0000, 1 << 20).unwrap();
    let bare = new_vm(mem, Box::new(heap), VmConfig::new(Platform::default(), Box::new(NoAssets)));
    assert_eq!(bare.call_onload(&mut e.cpu, echo), Ok(0));
    // A faulting JNI_OnLoad is reported.
    let fault = e.code(&[0xE3A0_0000, 0xE580_0000, 0xE12F_FF1E]);
    assert!(matches!(vm.call_onload(&mut e.cpu, fault), Err(Stop::Fault(_))));
}

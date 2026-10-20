//! `com.apportable.net.Reachability` and `WifiManager` (`jlib/net.rs`).

use crate::tests::{test_platform, Env};
use crate::*;
use std::sync::{Arc, Mutex};

const R: &str = "com/apportable/net/Reachability";

/// A `Reachability` whose natives record their calls; returns it and the log.
fn reachability(e: &mut Env) -> (Obj, Arc<Mutex<Vec<String>>>) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (l1, l2, l3) = (log.clone(), log.clone(), log.clone());
    e.vm.define(R)
        .method("didStart", "()V", move |_| {
            l1.lock().unwrap().push("didStart".into());
            Ok(Value::Void)
        })
        .method("stateChanged", "(III)V", move |c| {
            l2.lock().unwrap().push(format!("stateChanged {} {} {}", c.int(0), c.int(1), c.int(2)));
            Ok(Value::Void)
        })
        .method("stateChangeFinished", "()V", move |_| {
            l3.lock().unwrap().push("finished".into());
            Ok(Value::Void)
        });
    let ctx = Value::Ref(Some(e.activity()));
    let r = e.new_obj(R, "(Landroid/content/Context;)V", &[ctx]);
    (r, log)
}

fn run_main(e: &mut Env) -> usize {
    let vm = e.vm.clone();
    let l = vm.main_looper();
    vm.pump(&mut e.cpu, l, u64::MAX / 2).unwrap()
}

#[test]
fn reachability_reports_wifi_from_the_main_looper() {
    let mut e = Env::new();
    let (r, log) = reachability(&mut e);
    assert_eq!(e.vm.get_field(r, "context").as_ref(), Some(e.activity()));
    e.call(r, "start", "()V", &[]).unwrap();
    // Nothing runs inside start(): the broadcast is posted.
    assert!(log.lock().unwrap().is_empty());
    assert_eq!(run_main(&mut e), 1);
    assert_eq!(*log.lock().unwrap(), ["didStart", "stateChanged 1 0 4", "finished"]);
    e.call(r, "stop", "()V", &[]).unwrap();
}

#[test]
fn reachability_without_a_network_reports_disconnected() {
    let mut e = Env::with(|c| c.platform = Platform { lan: false, ..test_platform() });
    let (r, log) = reachability(&mut e);
    e.call(r, "start", "()V", &[]).unwrap();
    run_main(&mut e);
    assert_eq!(log.lock().unwrap()[1], "stateChanged 1 0 2");
    // A notifier without a target does nothing.
    let n = e.new_obj("com/apportable/net/Reachability$Notify", "()V", &[]);
    e.call(n, "run", "()V", &[]).unwrap();
    assert_eq!(log.lock().unwrap().len(), 3);
    let wifi = e.new_obj("android/net/wifi/WifiManager", "()V", &[]);
    assert_eq!(e.call(wifi, "isWifiEnabled", "()Z", &[]), Ok(Value::Int(0)));
}

#[test]
fn wifi_manager_and_multicast_lock() {
    let mut e = Env::new();
    let wifi = e.new_obj("android/net/wifi/WifiManager", "()V", &[]);
    assert_eq!(e.call(wifi, "isWifiEnabled", "()Z", &[]), Ok(Value::Int(1)));
    let tag = e.string("mdns");
    let sig = "(Ljava/lang/String;)Landroid/net/wifi/WifiManager$MulticastLock;";
    let r = e.call(wifi, "createMulticastLock", sig, &[tag]);
    let lock = e.obj(r);
    e.call(lock, "setReferenceCounted", "(Z)V", &[Value::Int(0)]).unwrap();
    e.call(lock, "acquire", "()V", &[]).unwrap();
    assert_eq!(e.call(lock, "isHeld", "()Z", &[]), Ok(Value::Int(1)));
    e.call(lock, "release", "()V", &[]).unwrap();
    let report = e.vm.report();
    assert!(!report.stubs().any(|m| m.class.starts_with("android/net/wifi/WifiManager")));
}

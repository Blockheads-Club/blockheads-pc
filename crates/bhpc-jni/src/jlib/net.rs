//! Network state for the game: Apportable's `com.apportable.net.Reachability` and `WifiManager`.
//!
//! SystemConfiguration's `SCNetworkReachability` on Apportable is fed by the Java `Reachability`
//! class: `start()` registers a connectivity receiver, which calls the natives `didStart`,
//! `stateChanged(type, from, to)` and `stateChangeFinished`. libApplication's
//! `-[AndroidReachability stateChange:from:to:]` maps `to` (low nibble: 4 = connected) and the
//! `ConnectivityManager` network type to flags: `TYPE_WIFI` (1), `TYPE_ETHERNET` (9) ... give
//! `Reachable | IsDirect`, `TYPE_MOBILE` (0) gives `Reachable | IsWWAN`. Without the callbacks the
//! game believes it is on cellular and hides the LAN host/join buttons.
//!
//! A PC is "on WiFi" when the host has a non-loopback IPv4 interface ([`Platform::lan`]); the
//! report is posted to the main looper so it never runs inside the caller of `start()`.

use super::{boolean, object, void};
use crate::looper;
use crate::value::{ClassId, Value};
use crate::vm::{Call, JResult, Vm};

/// `ConnectivityManager.TYPE_WIFI`.
pub(crate) const TYPE_WIFI: i32 = 1;
/// The `to` state `-[AndroidReachability stateChange:from:to:]` treats as connected.
pub(crate) const STATE_CONNECTED: i32 = 4;
/// The `to` state it treats as disconnected (no flags).
pub(crate) const STATE_DISCONNECTED: i32 = 2;

const REACHABILITY: &str = "com/apportable/net/Reachability";
const NOTIFY: &str = "com/apportable/net/Reachability$Notify";

pub(crate) fn install(vm: &Vm) {
    vm.define(REACHABILITY)
        .field("context", "Landroid/content/Context;")
        .method("<init>", "(Landroid/content/Context;)V", |c| {
            let (o, ctx) = (c.this, c.obj(0));
            c.vm.set_field(o, "context", Value::Ref(ctx));
            void()
        })
        .method("start", "()V", |c| {
            let k = c.vm.find_class(NOTIFY).unwrap_or(ClassId(0));
            let r = c.vm.alloc_object(k);
            c.vm.set_field(r, "target", Value::Ref(Some(c.this)));
            looper::post(c.vm, looper::main_looper(c.vm), r, 0);
            void()
        })
        .method("stop", "()V", |_| void())
        .natives(&[
            ("didStart", "()V", false),
            ("stateChanged", "(III)V", false),
            ("stateChangeFinished", "()V", false),
        ]);
    vm.define(NOTIFY)
        .implements(&["java/lang/Runnable"])
        .field("target", "Lcom/apportable/net/Reachability;")
        .method("run", "()V", report);
    vm.define("android/net/wifi/WifiManager").method("isWifiEnabled", "()Z", |c| boolean(c.vm.platform().lan)).method(
        "createMulticastLock",
        "(Ljava/lang/String;)Landroid/net/wifi/WifiManager$MulticastLock;",
        |c| {
            let k = c.vm.find_class("android/net/wifi/WifiManager$MulticastLock").unwrap_or(ClassId(0));
            object(Some(c.vm.alloc_object(k)))
        },
    );
    vm.define("android/net/wifi/WifiManager$MulticastLock")
        .method("acquire", "()V", |_| void())
        .method("release", "()V", |_| void())
        .method("setReferenceCounted", "(Z)V", |_| void())
        .method("isHeld", "()Z", |_| boolean(true));
}

/// `Reachability$Notify.run`: the connectivity broadcast, as the natives expect it.
fn report(c: &mut Call) -> JResult {
    let Some(t) = c.vm.get_field(c.this, "target").as_ref() else { return void() };
    let to = if c.vm.platform().lan { STATE_CONNECTED } else { STATE_DISCONNECTED };
    c.vm.call_method(c.cpu, t, "didStart", "()V", &[])?;
    c.vm.call_method(c.cpu, t, "stateChanged", "(III)V", &[Value::Int(TYPE_WIFI), Value::Int(0), Value::Int(to)])?;
    c.vm.call_method(c.cpu, t, "stateChangeFinished", "()V", &[])?;
    void()
}

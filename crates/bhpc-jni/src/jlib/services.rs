//! The Google services the Android build talks to, answered the way a device without them answers.
//!
//! - **In-app billing** (`com.apportable.iap.ApportableIAP`). On Android the class starts Google's billing helper and,
//!   when that fails, keeps `mBillingSupported` false: `setupFinished` still reaches the native side, every request
//!   (`querySkus`, `purchaseObject`, `restoreTransactions`) answers false. Here the setup ends the same way, and a
//!   product query fails at once (`querySkusResponse(.., error = true)`) instead of never answering, so the native
//!   StoreKit releases its request. Nothing is ever bought: the store keeps its `...` prices and purchases fail.
//! - **Google Play Games** (`com.apportable.plusgames.PlusGamesManager`). A sign-in (`connect`) fails with
//!   `SERVICE_MISSING`, as on a device without Play services: `nativeOnConnectionFailed` tells the game, which clears
//!   its "sign in" setting.
//!
//! The game itself shows nothing for either: its store keeps `...` on every price (there are no products to show
//! or buy) and "View achievements" only asks whether the player is signed in (`isLoggedIn`), then stops (it signs
//! in once per run, at start-up). So the host tells the user ([`Vm::take_notices`]), once they have touched the game:
//! the start-up sign-in stays quiet. "View achievements" is recognised by its lone `isLoggedIn`: the return to the
//! main menu asks the same, but queries the store's products right after it ([`GAMES_CHECK_MS`]).

use super::{boolean, void};
use crate::value::{ClassId, Obj, Value};
use crate::vm::{Call, JResult, Vm};
use std::collections::HashMap;

/// The message shown when the user asks for achievements or leaderboards.
pub const GAMES_NOTICE: &str = "ACHIEVEMENTS ARE NOT AVAILABLE ON PC";
/// How long after an `isLoggedIn` a product query still marks it as the main menu's (not "View achievements").
pub const GAMES_CHECK_MS: u64 = 400;
/// The message shown when the user tries to buy.
pub const STORE_NOTICE: &str = "IN-APP PURCHASES ARE NOT AVAILABLE ON PC";
/// `ConnectionResult.SERVICE_MISSING`.
pub const SERVICE_MISSING: i32 = 1;
/// What a failed product query reports (the text of Google's `IabResult` for `BILLING_RESPONSE_RESULT_BILLING_UNAVAILABLE`).
pub const BILLING_UNAVAILABLE: &str =
    "IabResult: Billing service unavailable on device. (response: 3:Billing Unavailable)";

const IAP: &str = "com/apportable/iap/ApportableIAP";
const GAMES: &str = "com/apportable/plusgames/PlusGamesManager";
const CALLBACK: &str = "com/apportable/HostCallback";

/// An argument of a deferred call: a value, or a string made when the call runs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Arg {
    V(Value),
    S(String),
}

/// A Java-to-native callback that runs later on the main looper (where Android's listeners run).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Deferred {
    /// Instance natives: the receiver; static natives: `None`.
    pub recv: Option<Obj>,
    pub class: &'static str,
    pub name: &'static str,
    pub sig: &'static str,
    pub args: Vec<Arg>,
}

/// Host-side state of the services.
#[derive(Debug, Default)]
pub(crate) struct Services {
    /// Messages for the user, oldest first.
    pub notices: Vec<String>,
    /// The user has touched the game.
    pub active: bool,
    pub deferred: HashMap<i32, Deferred>,
    pub next_id: i32,
    /// An `isLoggedIn` after user input: when to tell that achievements are not available (unless a product query
    /// comes first).
    pub games_check: Option<u64>,
}

impl Vm {
    /// Messages the host should show the user since the last call (for example "achievements are not available").
    pub fn take_notices(&self) -> Vec<String> {
        let now = self.now_ms();
        if self.st().services.games_check.is_some_and(|t| t <= now) {
            self.st().services.games_check = None;
            self.notice(GAMES_NOTICE);
        }
        std::mem::take(&mut self.st().services.notices)
    }

    /// The user interacted with the game: from now on the services' failures are told (see [`Vm::take_notices`]).
    pub fn note_user_input(&self) {
        self.st().services.active = true;
    }

    /// Queue `text` for the user unless it is already waiting; silent before the first user input.
    pub(crate) fn notice(&self, text: &str) {
        let mut st = self.st();
        let s = &mut st.services;
        if s.active && !s.notices.iter().any(|n| n == text) {
            s.notices.push(text.to_string());
        }
    }
}

/// Run `call` on the main looper.
pub(crate) fn defer(vm: &Vm, call: Deferred) {
    let id = {
        let mut st = vm.st();
        let s = &mut st.services;
        s.next_id += 1;
        let id = s.next_id;
        s.deferred.insert(id, call);
        id
    };
    let r = vm.alloc_object(vm.find_class(CALLBACK).unwrap_or(ClassId(0)));
    vm.set_field(r, "id", Value::Int(id));
    vm.post_main(r, 0);
}

fn run_deferred(c: &mut Call) -> JResult {
    let id = c.vm.get_field(c.this, "id").as_int();
    let Some(d) = c.vm.st().services.deferred.remove(&id) else { return void() };
    let args: Vec<Value> = d
        .args
        .iter()
        .map(|a| match a {
            Arg::V(v) => *v,
            Arg::S(s) => Value::Ref(Some(c.vm.new_string(s))),
        })
        .collect();
    match d.recv {
        Some(o) => c.vm.call_method(c.cpu, o, d.name, d.sig, &args)?,
        None => c.vm.call_static(c.cpu, d.class, d.name, d.sig, &args)?,
    };
    void()
}

/// The native peer of an `ApportableIAP` (`<init>(long, boolean)`).
fn iap_peer(c: &Call) -> Value {
    Value::Long(c.vm.get_field(c.this, "mObject").as_long())
}

pub(crate) fn install(vm: &Vm) {
    vm.define(CALLBACK).implements(&["java/lang/Runnable"]).field("id", "I").method("run", "()V", run_deferred);
    vm.define(IAP)
        .field("mObject", "J")
        .natives(&[
            ("purchaseRequestDidError", "(JLjava/lang/String;Ljava/lang/String;Ljava/lang/String;)V", true),
            ("querySkusResponse", "(JJLjava/lang/String;Z)V", true),
            ("restorePurchasesCompleted", "(J)V", true),
            ("restorePurchasesDidError", "(JLjava/lang/String;)V", true),
            ("setupFinished", "(J)V", true),
        ])
        .method("<init>", "(JZ)V", |c| {
            let peer = Value::Long(c.long(0));
            c.vm.set_field(c.this, "mObject", peer);
            c.vm.log("in-app billing: unavailable (no Google Play billing on PC)");
            defer(
                c.vm,
                Deferred { recv: None, class: IAP, name: "setupFinished", sig: "(J)V", args: vec![Arg::V(peer)] },
            );
            void()
        })
        .method("isBillingSupported", "()Z", |_| boolean(false))
        .method("areSubscriptionsSupported", "()Z", |_| boolean(false))
        .method("querySkus", "(JLjava/lang/String;)Z", |c| {
            // No message here: the game queries its products by itself (start, back to the menu).
            c.vm.st().services.games_check = None;
            let (peer, request) = (iap_peer(c), Value::Long(c.long(0)));
            defer(
                c.vm,
                Deferred {
                    recv: None,
                    class: IAP,
                    name: "querySkusResponse",
                    sig: "(JJLjava/lang/String;Z)V",
                    args: vec![
                        Arg::V(peer),
                        Arg::V(request),
                        Arg::S(BILLING_UNAVAILABLE.into()),
                        Arg::V(Value::Int(1)),
                    ],
                },
            );
            boolean(true)
        })
        .method("purchaseObject", "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Z", |c| {
            c.vm.notice(STORE_NOTICE);
            boolean(false)
        })
        .method("restoreTransactions", "()Z", |_| boolean(false))
        .method("consumePurchase", "(Ljava/lang/String;)Z", |_| boolean(false))
        .method("confirmNotification", "(Ljava/lang/String;I)V", |_| void())
        .method("handleActivityResult", "(IILandroid/content/Intent;)Z", |_| boolean(false));
    vm.define(GAMES)
        .field("mObject", "I")
        .method("<init>", "(ILandroid/app/Activity;)V", |c| {
            let peer = Value::Int(c.int(0));
            c.vm.set_field(c.this, "mObject", peer);
            void()
        })
        .method("connect", "(Landroid/view/View;)V", |c| {
            c.vm.log("Google Play Games: sign-in fails (no Play services on PC)");
            c.vm.notice(GAMES_NOTICE);
            let peer = c.vm.get_field(c.this, "mObject");
            defer(
                c.vm,
                Deferred {
                    recv: Some(c.this),
                    class: GAMES,
                    name: "nativeOnConnectionFailed",
                    sig: "(II)V",
                    args: vec![Arg::V(peer), Arg::V(Value::Int(SERVICE_MISSING))],
                },
            );
            void()
        })
        .method("isLoggedIn", "()Z", |c| {
            let at = c.vm.now_ms() + GAMES_CHECK_MS;
            let mut st = c.vm.st();
            if st.services.active {
                st.services.games_check = Some(at);
            }
            boolean(false)
        })
        .method("isAchievementsLoaded", "()Z", |_| boolean(false))
        .method("getShouldShowInvitationInbox", "()Z", |_| boolean(false))
        .method("showAchievements", "()V", |c| {
            c.vm.notice(GAMES_NOTICE);
            void()
        })
        .method("showLeaderboard", "(Ljava/lang/String;)V", |c| {
            c.vm.notice(GAMES_NOTICE);
            void()
        })
        .native("nativeOnConnectionFailed", "(II)V", false);
}

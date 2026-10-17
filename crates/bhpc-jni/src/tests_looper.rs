//! Looper, Handler, Message and MessageQueue.

use crate::tests::Env;
use crate::*;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

type Log = Arc<Mutex<Vec<String>>>;

fn define_recorders(e: &Env) -> Log {
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let l1 = log.clone();
    e.vm.define("t/Task").implements(&["java/lang/Runnable"]).field("name", "Ljava/lang/String;").method(
        "run",
        "()V",
        move |c| {
            let n = c.vm.get_field(c.this, "name").as_ref().and_then(|s| c.vm.string(s)).unwrap_or_default();
            l1.lock().unwrap().push(format!("run {n}"));
            Ok(Value::Void)
        },
    );
    let l2 = log.clone();
    e.vm.define("t/MyHandler").extends("android/os/Handler").method(
        "handleMessage",
        "(Landroid/os/Message;)V",
        move |c| {
            let what = c.vm.get_field(c.obj(0).unwrap(), "what").as_int();
            l2.lock().unwrap().push(format!("handle {what}"));
            Ok(Value::Void)
        },
    );
    let l3 = log.clone();
    e.vm.define("t/Cb").implements(&["android/os/Handler$Callback"]).field("eat", "Z").method(
        "handleMessage",
        "(Landroid/os/Message;)Z",
        move |c| {
            let what = c.vm.get_field(c.obj(0).unwrap(), "what").as_int();
            l3.lock().unwrap().push(format!("callback {what}"));
            Ok(Value::Int(c.vm.get_field(c.this, "eat").as_int()))
        },
    );
    log
}

fn task(e: &mut Env, name: &str) -> Obj {
    let t = e.new_obj("t/Task", "()V", &[]);
    let n = e.vm.new_string(name);
    e.vm.set_field(t, "name", Value::Ref(Some(n)));
    t
}

fn drain(e: &mut Env, looper: Obj, now: u64) -> usize {
    let vm = e.vm.clone();
    vm.pump(&mut e.cpu, looper, now).unwrap()
}

#[test]
fn handler_posts_runnables_in_time_order() {
    let mut e = Env::new();
    let log = define_recorders(&e);
    let ml = e.vm.attach_main_looper(e.cpu.processor_id());
    assert_eq!(ml, e.vm.main_looper());
    let h = e.new_obj("android/os/Handler", "()V", &[]);
    assert_eq!(e.vm.get_field(h, "looper"), Value::Ref(Some(ml)));
    assert_eq!(e.call(h, "getLooper", "()Landroid/os/Looper;", &[]), Ok(Value::Ref(Some(ml))));
    let (a, b, c, d, f) =
        (task(&mut e, "a"), task(&mut e, "b"), task(&mut e, "c"), task(&mut e, "d"), task(&mut e, "front"));
    let run = "(Ljava/lang/Runnable;)Z";
    assert_eq!(
        e.call(h, "postDelayed", "(Ljava/lang/Runnable;J)Z", &[Value::Ref(Some(a)), Value::Long(500)]),
        Ok(Value::Int(1))
    );
    assert_eq!(e.call(h, "post", run, &[Value::Ref(Some(b))]), Ok(Value::Int(1)));
    e.call(h, "postDelayed", "(Ljava/lang/Runnable;J)Z", &[Value::Ref(Some(c)), Value::Long(100)]).unwrap();
    e.call(h, "postAtTime", "(Ljava/lang/Runnable;J)Z", &[Value::Ref(Some(d)), Value::Long(1300)]).unwrap();
    e.call(h, "postAtFrontOfQueue", run, &[Value::Ref(Some(f))]).unwrap();
    assert_eq!(e.vm.pending_messages(ml), 5);
    assert_eq!(e.vm.next_wake(ml), Some(0), "the front-of-queue message is due immediately");
    assert_eq!(drain(&mut e, ml, 1000), 2);
    assert_eq!(log.lock().unwrap().as_slice(), ["run front", "run b"]);
    assert_eq!(e.vm.next_wake(ml), Some(1100));
    assert_eq!(e.vm.next_due(ml, 1099), None);
    assert_eq!(drain(&mut e, ml, 1100), 1);
    e.clock.store(5000, Ordering::Relaxed);
    assert_eq!(drain(&mut e, ml, 5000), 2);
    assert_eq!(log.lock().unwrap().as_slice(), ["run front", "run b", "run c", "run d", "run a"]);
    assert_eq!(e.vm.next_wake(ml), None);
    assert_eq!(e.vm.next_due(ml, u64::MAX), None);
    // postAtTime in the past is "now".
    e.call(h, "postAtTime", "(Ljava/lang/Runnable;J)Z", &[Value::Ref(Some(a)), Value::Long(1)]).unwrap();
    let now = e.vm.now_ms();
    assert_eq!(drain(&mut e, ml, now), 1);
}

#[test]
fn messages_dispatch_to_callbacks_and_handlers() {
    let mut e = Env::new();
    let log = define_recorders(&e);
    let ml = e.vm.attach_main_looper(e.cpu.processor_id());
    let h = e.new_obj("t/MyHandler", "()V", &[]);
    let vmc = e.vm.clone();
    let msg = move |what: i32| {
        let m = vmc.alloc_object(vmc.class_named("android/os/Message").unwrap());
        vmc.set_field(m, "what", Value::Int(what));
        m
    };
    // sendMessage variants.
    let send = "(Landroid/os/Message;)Z";
    let m1 = msg(1);
    assert_eq!(e.call(h, "sendMessage", send, &[Value::Ref(Some(m1))]), Ok(Value::Int(1)));
    assert_eq!(e.vm.get_field(m1, "target"), Value::Ref(Some(h)));
    e.call(h, "sendMessageDelayed", "(Landroid/os/Message;J)Z", &[Value::Ref(Some(msg(2))), Value::Long(50)]).unwrap();
    e.call(h, "sendMessageAtTime", "(Landroid/os/Message;J)Z", &[Value::Ref(Some(msg(3))), Value::Long(1010)]).unwrap();
    e.call(h, "sendMessageAtFrontOfQueue", send, &[Value::Ref(Some(msg(4)))]).unwrap();
    e.call(h, "sendEmptyMessage", "(I)Z", &[Value::Int(5)]).unwrap();
    e.call(h, "sendEmptyMessageDelayed", "(IJ)Z", &[Value::Int(6), Value::Long(5000)]).unwrap();
    assert_eq!(drain(&mut e, ml, 1000), 3);
    assert_eq!(log.lock().unwrap().as_slice(), ["handle 4", "handle 1", "handle 5"]);
    assert_eq!(drain(&mut e, ml, 1060), 2);
    assert_eq!(drain(&mut e, ml, 1500), 0);
    assert_eq!(drain(&mut e, ml, 9000), 1);
    assert_eq!(log.lock().unwrap().len(), 6);
    // hasMessages / removeMessages.
    e.call(h, "sendEmptyMessage", "(I)Z", &[Value::Int(7)]).unwrap();
    e.call(h, "sendEmptyMessage", "(I)Z", &[Value::Int(8)]).unwrap();
    assert_eq!(e.call(h, "hasMessages", "(I)Z", &[Value::Int(7)]), Ok(Value::Int(1)));
    assert_eq!(e.call(h, "hasMessages", "(I)Z", &[Value::Int(9)]), Ok(Value::Int(0)));
    e.call(h, "removeMessages", "(I)V", &[Value::Int(7)]).unwrap();
    assert_eq!(e.call(h, "hasMessages", "(I)Z", &[Value::Int(7)]), Ok(Value::Int(0)));
    assert_eq!(e.vm.pending_messages(ml), 1);
    // Messages of other handlers are untouched.
    let other = e.new_obj("android/os/Handler", "()V", &[]);
    e.call(other, "removeMessages", "(I)V", &[Value::Int(8)]).unwrap();
    assert_eq!(e.vm.pending_messages(ml), 1);
    // removeCallbacks / removeCallbacksAndMessages.
    let (t1, t2) = (task(&mut e, "t1"), task(&mut e, "t2"));
    e.call(h, "post", "(Ljava/lang/Runnable;)Z", &[Value::Ref(Some(t1))]).unwrap();
    e.call(h, "post", "(Ljava/lang/Runnable;)Z", &[Value::Ref(Some(t2))]).unwrap();
    assert_eq!(
        e.call(h, "hasMessages", "(I)Z", &[Value::Int(0)]),
        Ok(Value::Int(0)),
        "runnables are not 'what' messages"
    );
    e.call(h, "removeCallbacks", "(Ljava/lang/Runnable;)V", &[Value::Ref(Some(t1))]).unwrap();
    assert_eq!(e.vm.pending_messages(ml), 2);
    e.call(h, "removeCallbacksAndMessages", "(Ljava/lang/Object;)V", &[Value::Ref(None)]).unwrap();
    assert_eq!(e.vm.pending_messages(ml), 0);
    let tagged = msg(10);
    let token = e.vm.new_string("token");
    e.vm.set_field(tagged, "obj", Value::Ref(Some(token)));
    e.call(h, "sendMessage", send, &[Value::Ref(Some(tagged))]).unwrap();
    e.call(h, "sendEmptyMessage", "(I)Z", &[Value::Int(11)]).unwrap();
    e.call(h, "removeCallbacksAndMessages", "(Ljava/lang/Object;)V", &[Value::Ref(Some(token))]).unwrap();
    assert_eq!(e.vm.pending_messages(ml), 1);
}

#[test]
fn handler_callbacks_and_obtain() {
    let mut e = Env::new();
    let log = define_recorders(&e);
    let ml = e.vm.attach_main_looper(e.cpu.processor_id());
    let cb = e.new_obj("t/Cb", "()V", &[]);
    // Handler(Callback): the callback sees the message first; returning false falls through.
    let h = e.new_obj("android/os/Handler", "(Landroid/os/Handler$Callback;)V", &[Value::Ref(Some(cb))]);
    e.call(h, "sendEmptyMessage", "(I)Z", &[Value::Int(1)]).unwrap();
    e.vm.set_field(cb, "eat", Value::Int(1));
    e.call(h, "sendEmptyMessage", "(I)Z", &[Value::Int(2)]).unwrap();
    drain(&mut e, ml, 1000);
    assert_eq!(log.lock().unwrap().as_slice(), ["callback 1", "callback 2"]);
    // Handler(Looper, Callback) and Handler(Looper).
    let h2 = e.new_obj(
        "android/os/Handler",
        "(Landroid/os/Looper;Landroid/os/Handler$Callback;)V",
        &[Value::Ref(Some(ml)), Value::Ref(Some(cb))],
    );
    assert_eq!(e.vm.get_field(h2, "looper"), Value::Ref(Some(ml)));
    let h3 = e.new_obj("android/os/Handler", "(Landroid/os/Looper;)V", &[Value::Ref(Some(ml))]);
    assert_eq!(e.vm.get_field(h3, "callback"), Value::Ref(None));
    // Message.obtain family.
    let mo = |e: &mut Env, sig: &str, args: &[Value]| {
        e.scall("android/os/Message", "obtain", sig, args).unwrap().as_ref().unwrap()
    };
    let m = mo(&mut e, "()Landroid/os/Message;", &[]);
    assert_eq!(e.vm.get_field(m, "target"), Value::Ref(None));
    let hv = Value::Ref(Some(h3));
    let m = mo(&mut e, "(Landroid/os/Handler;)Landroid/os/Message;", &[hv]);
    assert_eq!(e.vm.get_field(m, "target"), hv);
    let m = mo(&mut e, "(Landroid/os/Handler;I)Landroid/os/Message;", &[hv, Value::Int(3)]);
    assert_eq!(e.vm.get_field(m, "what"), Value::Int(3));
    let payload = e.vm.new_string("p");
    let m = mo(
        &mut e,
        "(Landroid/os/Handler;ILjava/lang/Object;)Landroid/os/Message;",
        &[hv, Value::Int(4), Value::Ref(Some(payload))],
    );
    assert_eq!(e.vm.get_field(m, "obj"), Value::Ref(Some(payload)));
    let m =
        mo(&mut e, "(Landroid/os/Handler;III)Landroid/os/Message;", &[hv, Value::Int(5), Value::Int(6), Value::Int(7)]);
    assert_eq!((e.vm.get_field(m, "arg1"), e.vm.get_field(m, "arg2")), (Value::Int(6), Value::Int(7)));
    let m = mo(
        &mut e,
        "(Landroid/os/Handler;IIILjava/lang/Object;)Landroid/os/Message;",
        &[hv, Value::Int(5), Value::Int(6), Value::Int(7), Value::Ref(Some(payload))],
    );
    assert_eq!(e.vm.get_field(m, "obj"), Value::Ref(Some(payload)));
    let t = task(&mut e, "msgtask");
    let m = mo(&mut e, "(Landroid/os/Handler;Ljava/lang/Runnable;)Landroid/os/Message;", &[hv, Value::Ref(Some(t))]);
    assert_eq!(e.vm.get_field(m, "callback"), Value::Ref(Some(t)));
    // sendToTarget enqueues through the handler; recycle is a no-op; target accessors.
    e.call(m, "sendToTarget", "()V", &[]).unwrap();
    assert_eq!(e.vm.pending_messages(ml), 1);
    drain(&mut e, ml, 5000);
    assert_eq!(log.lock().unwrap().last().unwrap(), "run msgtask");
    e.call(m, "recycle", "()V", &[]).unwrap();
    e.call(m, "setTarget", "(Landroid/os/Handler;)V", &[Value::Ref(None)]).unwrap();
    assert_eq!(e.call(m, "getTarget", "()Landroid/os/Handler;", &[]), Ok(Value::Ref(None)));
    let r = e.call(m, "sendToTarget", "()V", &[]);
    assert!(matches!(r, Err(Throw::Exc(_))));
    // Handler.obtainMessage family.
    let ho = |e: &mut Env, sig: &str, args: &[Value]| e.call(h3, "obtainMessage", sig, args).unwrap().as_ref().unwrap();
    let m = ho(&mut e, "()Landroid/os/Message;", &[]);
    assert_eq!(e.vm.get_field(m, "target"), hv);
    let m = ho(&mut e, "(I)Landroid/os/Message;", &[Value::Int(8)]);
    assert_eq!(e.vm.get_field(m, "what"), Value::Int(8));
    let m = ho(&mut e, "(ILjava/lang/Object;)Landroid/os/Message;", &[Value::Int(8), Value::Ref(Some(payload))]);
    assert_eq!(e.vm.get_field(m, "obj"), Value::Ref(Some(payload)));
    let m = ho(&mut e, "(III)Landroid/os/Message;", &[Value::Int(8), Value::Int(1), Value::Int(2)]);
    assert_eq!(e.vm.get_field(m, "arg2"), Value::Int(2));
    // Null messages are rejected.
    for (name, sig, args) in [
        ("sendMessage", "(Landroid/os/Message;)Z", vec![Value::Ref(None)]),
        ("sendMessageDelayed", "(Landroid/os/Message;J)Z", vec![Value::Ref(None), Value::Long(1)]),
        ("sendMessageAtTime", "(Landroid/os/Message;J)Z", vec![Value::Ref(None), Value::Long(1)]),
        ("sendMessageAtFrontOfQueue", "(Landroid/os/Message;)Z", vec![Value::Ref(None)]),
        ("dispatchMessage", "(Landroid/os/Message;)V", vec![Value::Ref(None)]),
    ] {
        assert!(matches!(e.call(h3, name, sig, &args), Err(Throw::Exc(_))), "{name}");
    }
}

#[test]
fn looper_statics_and_threads() {
    let mut e = Env::new();
    let l = "android/os/Looper";
    // No looper on this thread yet.
    assert_eq!(e.scall(l, "myLooper", "()Landroid/os/Looper;", &[]), Ok(Value::Ref(None)));
    let main = e.scall(l, "getMainLooper", "()Landroid/os/Looper;", &[]).unwrap().as_ref().unwrap();
    assert_eq!(main, e.vm.main_looper());
    // A handler without a looper on the thread falls back to the main looper.
    let h = e.new_obj("android/os/Handler", "()V", &[]);
    assert_eq!(e.vm.get_field(h, "looper"), Value::Ref(Some(main)));
    // prepare() creates a private looper, a second prepare fails.
    e.scall(l, "prepare", "()V", &[]).unwrap();
    let mine = e.scall(l, "myLooper", "()Landroid/os/Looper;", &[]).unwrap().as_ref().unwrap();
    assert_ne!(mine, main);
    let r = e.scall(l, "prepare", "(Z)V", &[Value::Int(0)]);
    assert!(matches!(r, Err(Throw::Exc(_))));
    let h2 = e.new_obj("android/os/Handler", "()V", &[]);
    assert_eq!(e.vm.get_field(h2, "looper"), Value::Ref(Some(mine)));
    // Looper methods.
    let q = e.call(mine, "getQueue", "()Landroid/os/MessageQueue;", &[]).unwrap();
    assert_eq!(e.scall(l, "myQueue", "()Landroid/os/MessageQueue;", &[]), Ok(q));
    let t = e.call(mine, "getThread", "()Ljava/lang/Thread;", &[]).unwrap().as_ref().unwrap();
    assert_eq!(e.vm.get_field(t, "id"), Value::Long(1));
    let t = e.call(main, "getThread", "()Ljava/lang/Thread;", &[]).unwrap().as_ref().unwrap();
    assert_eq!(e.vm.get_field(t, "id"), Value::Long(1), "unattached loopers report thread 0");
    e.call(mine, "quit", "()V", &[]).unwrap();
    e.call(mine, "quitSafely", "()V", &[]).unwrap();
    e.scall(l, "loop", "()V", &[]).unwrap();
    e.scall(l, "prepareMainLooper", "()V", &[]).unwrap();
    assert_eq!(e.scall(l, "myLooper", "()Landroid/os/Looper;", &[]), Ok(Value::Ref(Some(main))));
    // MessageQueue.
    let mq = q.as_ref().unwrap();
    assert_eq!(e.call(mq, "isIdle", "()Z", &[]), Ok(Value::Int(1)));
    e.call(mq, "addIdleHandler", "(Landroid/os/MessageQueue$IdleHandler;)V", &[Value::Ref(None)]).unwrap();
    e.call(mq, "removeIdleHandler", "(Landroid/os/MessageQueue$IdleHandler;)V", &[Value::Ref(None)]).unwrap();
    // The thread is now attached to main; next() pops due messages of its looper.
    let mqm = e.scall(l, "myQueue", "()Landroid/os/MessageQueue;", &[]).unwrap().as_ref().unwrap();
    assert_eq!(e.call(mqm, "next", "()Landroid/os/Message;", &[]), Ok(Value::Ref(None)));
    let hm = e.new_obj("android/os/Handler", "()V", &[]);
    e.call(hm, "sendEmptyMessage", "(I)Z", &[Value::Int(1)]).unwrap();
    assert_eq!(e.call(mqm, "isIdle", "()Z", &[]), Ok(Value::Int(0)));
    e.clock.store(5000, Ordering::Relaxed);
    let m = e.call(mqm, "next", "()Landroid/os/Message;", &[]).unwrap().as_ref().unwrap();
    assert_eq!(e.vm.get_field(m, "what"), Value::Int(1));
    // Dispatching a message with neither callback nor target does nothing.
    let vm = e.vm.clone();
    let bare = vm.alloc_object(vm.class_named("android/os/Message").unwrap());
    assert_eq!(vm.dispatch_message(&mut e.cpu, bare), Ok(()));
    // A failing runnable aborts the pump.
    vm.define("t/Fail")
        .implements(&["java/lang/Runnable"])
        .method("run", "()V", |c| Err(c.vm.throw_new("java/lang/RuntimeException", "x")));
    let f = e.new_obj("t/Fail", "()V", &[]);
    assert!(vm.post_main(f, 0));
    let now = e.vm.now_ms();
    assert!(matches!(vm.pump(&mut e.cpu, main, now), Err(Throw::Exc(_))));
}

#[test]
fn looper_edge_cases() {
    let mut e = Env::new();
    // Enqueueing into an object that is not a looper reports failure.
    let not_looper = e.vm.new_string("nope");
    let msg = e.vm.alloc_object(e.vm.class_named("android/os/Message").unwrap());
    assert!(!crate::looper::enqueue(&e.vm, not_looper, msg, 1));
    let hollow = e.vm.alloc_object(e.vm.class_named("android/os/Looper").unwrap());
    assert!(!crate::looper::enqueue(&e.vm, hollow, msg, 1), "no queue");
    let q = e.vm.new_string("not a queue");
    e.vm.set_field(hollow, "queue", Value::Ref(Some(q)));
    assert!(!crate::looper::enqueue(&e.vm, hollow, msg, 1), "queue without Queue data");
    assert_eq!(e.vm.pending_messages(hollow), 0);
    assert_eq!(e.vm.next_wake(hollow), None);
    assert_eq!(e.vm.next_due(hollow, 10), None);
    let r = e.vm.alloc_object(e.vm.class_named("android/os/Looper").unwrap());
    assert!(!crate::looper::post(&e.vm, r, msg, 5));
    // run_tick_actions with nothing registered is fine.
    let vm = e.vm.clone();
    assert_eq!(vm.run_tick_actions(&mut e.cpu), Ok(()));
}

#[test]
fn dispatch_message_directly() {
    let mut e = Env::new();
    let log = define_recorders(&e);
    let ml = e.vm.attach_main_looper(e.cpu.processor_id());
    // A message carrying a callback runs it.
    let h = e.new_obj("android/os/Handler", "()V", &[]);
    let t = task(&mut e, "direct");
    let m = e
        .scall(
            "android/os/Message",
            "obtain",
            "(Landroid/os/Handler;Ljava/lang/Runnable;)Landroid/os/Message;",
            &[Value::Ref(Some(h)), Value::Ref(Some(t))],
        )
        .unwrap();
    e.call(h, "dispatchMessage", "(Landroid/os/Message;)V", &[m]).unwrap();
    assert_eq!(log.lock().unwrap().as_slice(), ["run direct"]);
    // The handler callback can consume the message, or let it fall through to handleMessage.
    let cb = e.new_obj("t/Cb", "()V", &[]);
    let hc = e.new_obj("t/MyHandler", "(Landroid/os/Handler$Callback;)V", &[Value::Ref(Some(cb))]);
    let plain = e
        .scall(
            "android/os/Message",
            "obtain",
            "(Landroid/os/Handler;I)Landroid/os/Message;",
            &[Value::Ref(Some(hc)), Value::Int(9)],
        )
        .unwrap();
    e.call(hc, "dispatchMessage", "(Landroid/os/Message;)V", &[plain]).unwrap();
    e.vm.set_field(cb, "eat", Value::Int(1));
    e.call(hc, "dispatchMessage", "(Landroid/os/Message;)V", &[plain]).unwrap();
    assert_eq!(log.lock().unwrap()[1..], ["callback 9", "handle 9", "callback 9"]);
    // The base Handler.handleMessage does nothing.
    let bare = e
        .scall(
            "android/os/Message",
            "obtain",
            "(Landroid/os/Handler;I)Landroid/os/Message;",
            &[Value::Ref(Some(h)), Value::Int(1)],
        )
        .unwrap();
    e.call(h, "dispatchMessage", "(Landroid/os/Message;)V", &[bare]).unwrap();
    e.call(h, "sendEmptyMessage", "(I)Z", &[Value::Int(2)]).unwrap();
    assert_eq!(drain(&mut e, ml, 5000), 1);
    // A handler on a looper without a queue still answers queries.
    let hollow = e.vm.alloc_object(e.vm.class_named("android/os/Looper").unwrap());
    let hh = e.new_obj("android/os/Handler", "(Landroid/os/Looper;)V", &[Value::Ref(Some(hollow))]);
    e.call(hh, "removeMessages", "(I)V", &[Value::Int(1)]).unwrap();
    assert_eq!(e.call(hh, "hasMessages", "(I)Z", &[Value::Int(1)]), Ok(Value::Int(0)));
    assert_eq!(e.call(hh, "sendEmptyMessage", "(I)Z", &[Value::Int(1)]), Ok(Value::Int(0)));
}

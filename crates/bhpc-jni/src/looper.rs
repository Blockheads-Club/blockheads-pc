//! `android.os.Looper/Handler/Message/MessageQueue`: the timed message queue Apportable's unified
//! run loop is built on, plus the host API to drive it.
//!
//! The fake `Looper.loop()` returns immediately: the host owns the loop. Typical driver:
//!
//! ```text
//! vm.attach_main_looper(cpu.processor_id());
//! loop { vm.pump(cpu, vm.main_looper(), now)?; sleep_until(vm.next_wake(vm.main_looper())); }
//! ```

use crate::jlib::{boolean, object, void};
use crate::value::{Obj, Value};
use crate::vm::{Call, Data, JResult, Throw, Vm};
use bhpc_cpu::Cpu;

/// The main looper (created on first use).
pub fn main_looper(vm: &Vm) -> Obj {
    if let Some(l) = vm.st().main_looper {
        return l;
    }
    let l = new_looper(vm);
    vm.st().main_looper = Some(l);
    l
}

fn new_looper(vm: &Vm) -> Obj {
    let k = vm.find_class("android/os/Looper").unwrap_or(crate::value::ClassId(0));
    let l = vm.alloc_object(k);
    let q = vm.new_data_object("android/os/MessageQueue", Data::Queue(Vec::new()));
    vm.set_field(l, "queue", Value::Ref(Some(q)));
    l
}

/// Make guest thread `pid` the main looper's thread (`Looper.myLooper()` returns it there).
pub fn attach_main(vm: &Vm, pid: usize) -> Obj {
    let l = main_looper(vm);
    vm.st().loopers.insert(pid, l);
    l
}

fn current_looper(vm: &Vm, pid: usize) -> Obj {
    let cur = vm.st().loopers.get(&pid).copied();
    cur.unwrap_or_else(|| main_looper(vm))
}

/// Insert `msg` into the queue of `looper`, ordered by `when`.
pub(crate) fn enqueue(vm: &Vm, looper: Obj, msg: Obj, when: i64) -> bool {
    vm.set_field(msg, "when", Value::Long(when));
    let mut st = vm.st();
    let Some(q) = st.field_named(looper, "queue").and_then(|v| v.as_ref()) else { return false };
    let Some(Data::Queue(items)) = st.obj(q).map(|o| o.data.clone()) else { return false };
    let at =
        items.iter().position(|m| st.field_named(*m, "when").map_or(0, |w| w.as_long()) > when).unwrap_or(items.len());
    if let Some(Data::Queue(items)) = st.obj_mut(q).map(|o| &mut o.data) {
        items.insert(at, msg);
    }
    true
}

/// Queue a `Runnable` on `looper`, to run `delay_ms` from now.
pub fn post(vm: &Vm, looper: Obj, runnable: Obj, delay_ms: i64) -> bool {
    let m = new_message(vm);
    vm.set_field(m, "callback", Value::Ref(Some(runnable)));
    enqueue(vm, looper, m, vm.now_ms() as i64 + delay_ms.max(0))
}

fn new_message(vm: &Vm) -> Obj {
    let k = vm.find_class("android/os/Message").unwrap_or(crate::value::ClassId(0));
    vm.alloc_object(k)
}

fn queue_items(vm: &Vm, looper: Obj) -> Vec<Obj> {
    let st = vm.st();
    let q = st.field_named(looper, "queue").and_then(|v| v.as_ref());
    match q.and_then(|q| st.obj(q)).map(|o| &o.data) {
        Some(Data::Queue(v)) => v.clone(),
        _ => Vec::new(),
    }
}

fn set_queue(vm: &Vm, looper: Obj, items: Vec<Obj>) {
    let q = vm.get_field(looper, "queue").as_ref();
    if let Some(q) = q {
        vm.with_data(q, |d| *d = Data::Queue(items));
    }
}

impl Vm {
    /// The main looper object.
    pub fn main_looper(&self) -> Obj {
        main_looper(self)
    }

    /// Bind the main looper to guest thread `pid`.
    pub fn attach_main_looper(&self, pid: usize) -> Obj {
        attach_main(self, pid)
    }

    /// Queue a `Runnable` on the main looper.
    pub fn post_main(&self, runnable: Obj, delay_ms: i64) -> bool {
        post(self, main_looper(self), runnable, delay_ms)
    }

    /// Remove and return the first message of `looper` that is due at `now` (VM clock, ms).
    pub fn next_due(&self, looper: Obj, now: u64) -> Option<Obj> {
        let items = queue_items(self, looper);
        let first = *items.first()?;
        if self.get_field(first, "when").as_long() > now as i64 {
            return None;
        }
        set_queue(self, looper, items[1..].to_vec());
        Some(first)
    }

    /// When the next message of `looper` is due (VM clock ms), if any.
    pub fn next_wake(&self, looper: Obj) -> Option<u64> {
        let first = *queue_items(self, looper).first()?;
        Some(self.get_field(first, "when").as_long().max(0) as u64)
    }

    /// Number of queued messages.
    pub fn pending_messages(&self, looper: Obj) -> usize {
        queue_items(self, looper).len()
    }

    /// Run `msg` the way `Handler.dispatchMessage` does.
    pub fn dispatch_message(&self, cpu: &mut Cpu, msg: Obj) -> Result<(), Throw> {
        let target = self.get_field(msg, "target").as_ref();
        let callback = self.get_field(msg, "callback").as_ref();
        if let Some(cb) = callback {
            self.call_method(cpu, cb, "run", "()V", &[])?;
            return Ok(());
        }
        let Some(t) = target else { return Ok(()) };
        self.call_method(cpu, t, "dispatchMessage", "(Landroid/os/Message;)V", &[Value::Ref(Some(msg))])?;
        Ok(())
    }

    /// Dispatch every message of `looper` due at `now`; returns how many ran.
    pub fn pump(&self, cpu: &mut Cpu, looper: Obj, now: u64) -> Result<usize, Throw> {
        let mut n = 0;
        while let Some(m) = self.next_due(looper, now) {
            self.dispatch_message(cpu, m)?;
            n += 1;
        }
        Ok(n)
    }

    /// Run all registered `MainThread` tick actions once.
    pub fn run_tick_actions(&self, cpu: &mut Cpu) -> Result<(), Throw> {
        let actions = self.st().ticks.clone();
        for a in actions {
            self.call_method(cpu, a, "run", "()V", &[])?;
        }
        Ok(())
    }
}

fn msg_arg(c: &Call, i: usize) -> Result<Obj, Throw> {
    c.obj(i).ok_or_else(|| c.vm.throw_new("java/lang/NullPointerException", "message"))
}

fn handler_looper(c: &Call) -> Obj {
    c.vm.get_field(c.this, "looper").as_ref().unwrap_or(Obj::NULL)
}

fn send(c: &mut Call, msg: Obj, when: i64) -> JResult {
    c.vm.set_field(msg, "target", Value::Ref(Some(c.this)));
    boolean(enqueue(c.vm, handler_looper(c), msg, when))
}

fn init_handler(c: &mut Call, looper: Option<Obj>, callback: Option<Obj>) -> JResult {
    let l = looper.unwrap_or_else(|| current_looper(c.vm, c.cpu.processor_id()));
    c.vm.set_field(c.this, "looper", Value::Ref(Some(l)));
    c.vm.set_field(c.this, "callback", Value::Ref(callback));
    void()
}

fn obtain(c: &mut Call, handler: Option<Obj>, what: i32, a1: i32, a2: i32, obj: Option<Obj>) -> JResult {
    let m = new_message(c.vm);
    c.vm.set_field(m, "target", Value::Ref(handler));
    c.vm.set_field(m, "what", Value::Int(what));
    c.vm.set_field(m, "arg1", Value::Int(a1));
    c.vm.set_field(m, "arg2", Value::Int(a2));
    c.vm.set_field(m, "obj", Value::Ref(obj));
    object(Some(m))
}

fn remove_where(c: &Call, keep: impl Fn(&Vm, Obj) -> bool) {
    let l = handler_looper(c);
    let items: Vec<Obj> = queue_items(c.vm, l)
        .into_iter()
        .filter(|m| c.vm.get_field(*m, "target").as_ref() != Some(c.this) || keep(c.vm, *m))
        .collect();
    set_queue(c.vm, l, items);
}

fn has_where(c: &Call, hit: impl Fn(&Vm, Obj) -> bool) -> bool {
    queue_items(c.vm, handler_looper(c))
        .into_iter()
        .any(|m| c.vm.get_field(m, "target").as_ref() == Some(c.this) && hit(c.vm, m))
}

pub(crate) fn install(vm: &Vm) {
    vm.define("android/os/MessageQueue")
        .method("next", "()Landroid/os/Message;", |c| {
            let now = c.vm.now_ms();
            let looper = current_looper(c.vm, c.cpu.processor_id());
            object(c.vm.next_due(looper, now))
        })
        .method("addIdleHandler", "(Landroid/os/MessageQueue$IdleHandler;)V", |_| void())
        .method("removeIdleHandler", "(Landroid/os/MessageQueue$IdleHandler;)V", |_| void())
        .method("isIdle", "()Z", |c| {
            boolean(c.vm.with_data(c.this, |d| matches!(d, Data::Queue(q) if q.is_empty())).unwrap_or(true))
        });
    vm.define("android/os/Looper")
        .field("queue", "Landroid/os/MessageQueue;")
        .static_method("getMainLooper", "()Landroid/os/Looper;", |c| object(Some(main_looper(c.vm))))
        .static_method("myLooper", "()Landroid/os/Looper;", |c| {
            object(c.vm.st().loopers.get(&c.cpu.processor_id()).copied())
        })
        .static_method("prepare", "()V", prepare)
        .static_method("prepare", "(Z)V", prepare)
        .static_method("prepareMainLooper", "()V", |c| {
            attach_main(c.vm, c.cpu.processor_id());
            void()
        })
        .static_method("loop", "()V", |_| void())
        .static_method("myQueue", "()Landroid/os/MessageQueue;", |c| {
            let l = current_looper(c.vm, c.cpu.processor_id());
            Ok(c.vm.get_field(l, "queue"))
        })
        .method("getQueue", "()Landroid/os/MessageQueue;", |c| Ok(c.vm.get_field(c.this, "queue")))
        .method("getThread", "()Ljava/lang/Thread;", |c| {
            let pid = c.vm.st().loopers.iter().find(|(_, l)| **l == c.this).map_or(0, |(p, _)| *p);
            object(Some(crate::jlib::thread_object(c.vm, pid)))
        })
        .method("quit", "()V", |_| void())
        .method("quitSafely", "()V", |_| void());
    vm.define("android/os/Message")
        .field("what", "I")
        .field("arg1", "I")
        .field("arg2", "I")
        .field("obj", "Ljava/lang/Object;")
        .field("target", "Landroid/os/Handler;")
        .field("callback", "Ljava/lang/Runnable;")
        .field("when", "J")
        .method("<init>", "()V", |_| void())
        .static_method("obtain", "()Landroid/os/Message;", |c| obtain(c, None, 0, 0, 0, None))
        .static_method("obtain", "(Landroid/os/Handler;)Landroid/os/Message;", |c| obtain(c, c.obj(0), 0, 0, 0, None))
        .static_method("obtain", "(Landroid/os/Handler;I)Landroid/os/Message;", |c| {
            obtain(c, c.obj(0), c.int(1), 0, 0, None)
        })
        .static_method("obtain", "(Landroid/os/Handler;ILjava/lang/Object;)Landroid/os/Message;", |c| {
            obtain(c, c.obj(0), c.int(1), 0, 0, c.obj(2))
        })
        .static_method("obtain", "(Landroid/os/Handler;III)Landroid/os/Message;", |c| {
            obtain(c, c.obj(0), c.int(1), c.int(2), c.int(3), None)
        })
        .static_method("obtain", "(Landroid/os/Handler;IIILjava/lang/Object;)Landroid/os/Message;", |c| {
            obtain(c, c.obj(0), c.int(1), c.int(2), c.int(3), c.obj(4))
        })
        .static_method("obtain", "(Landroid/os/Handler;Ljava/lang/Runnable;)Landroid/os/Message;", |c| {
            let m = obtain(c, c.obj(0), 0, 0, 0, None)?;
            if let Some(mo) = m.as_ref() {
                c.vm.set_field(mo, "callback", c.args[1]);
            }
            Ok(m)
        })
        .method("recycle", "()V", |_| void())
        .method("sendToTarget", "()V", |c| {
            let t = c.vm.get_field(c.this, "target").as_ref();
            match t {
                Some(t) => {
                    c.vm.call_method(c.cpu, t, "sendMessage", "(Landroid/os/Message;)Z", &[Value::Ref(Some(c.this))])?;
                    void()
                }
                None => Err(c.vm.throw_new("java/lang/NullPointerException", "Message.target")),
            }
        })
        .method("setTarget", "(Landroid/os/Handler;)V", |c| {
            c.vm.set_field(c.this, "target", c.args[0]);
            void()
        })
        .method("getTarget", "()Landroid/os/Handler;", |c| Ok(c.vm.get_field(c.this, "target")));
    vm.define("android/os/Handler")
        .field("looper", "Landroid/os/Looper;")
        .field("callback", "Landroid/os/Handler$Callback;")
        .method("<init>", "()V", |c| init_handler(c, None, None))
        .method("<init>", "(Landroid/os/Looper;)V", |c| init_handler(c, c.obj(0), None))
        .method("<init>", "(Landroid/os/Handler$Callback;)V", |c| init_handler(c, None, c.obj(0)))
        .method("<init>", "(Landroid/os/Looper;Landroid/os/Handler$Callback;)V", |c| {
            init_handler(c, c.obj(0), c.obj(1))
        })
        .method("getLooper", "()Landroid/os/Looper;", |c| Ok(c.vm.get_field(c.this, "looper")))
        .method("handleMessage", "(Landroid/os/Message;)V", |_| void())
        .method("dispatchMessage", "(Landroid/os/Message;)V", |c| {
            let m = msg_arg(c, 0)?;
            if let Some(cb) = c.vm.get_field(m, "callback").as_ref() {
                c.vm.call_method(c.cpu, cb, "run", "()V", &[])?;
                return void();
            }
            if let Some(cb) = c.vm.get_field(c.this, "callback").as_ref() {
                let handled =
                    c.vm.call_method(c.cpu, cb, "handleMessage", "(Landroid/os/Message;)Z", &[Value::Ref(Some(m))])?;
                if handled.as_int() != 0 {
                    return void();
                }
            }
            c.vm.call_method(c.cpu, c.this, "handleMessage", "(Landroid/os/Message;)V", &[Value::Ref(Some(m))])?;
            void()
        })
        .method("sendMessage", "(Landroid/os/Message;)Z", |c| {
            let m = msg_arg(c, 0)?;
            send(c, m, c.vm.now_ms() as i64)
        })
        .method("sendMessageDelayed", "(Landroid/os/Message;J)Z", |c| {
            let m = msg_arg(c, 0)?;
            send(c, m, c.vm.now_ms() as i64 + c.long(1).max(0))
        })
        .method("sendMessageAtTime", "(Landroid/os/Message;J)Z", |c| {
            let m = msg_arg(c, 0)?;
            send(c, m, c.long(1))
        })
        .method("sendMessageAtFrontOfQueue", "(Landroid/os/Message;)Z", |c| {
            let m = msg_arg(c, 0)?;
            send(c, m, i64::MIN)
        })
        .method("sendEmptyMessage", "(I)Z", |c| {
            let m = obtain(c, None, c.int(0), 0, 0, None)?.as_ref().unwrap_or(Obj::NULL);
            send(c, m, c.vm.now_ms() as i64)
        })
        .method("sendEmptyMessageDelayed", "(IJ)Z", |c| {
            let m = obtain(c, None, c.int(0), 0, 0, None)?.as_ref().unwrap_or(Obj::NULL);
            send(c, m, c.vm.now_ms() as i64 + c.long(1).max(0))
        })
        .method("post", "(Ljava/lang/Runnable;)Z", |c| post_runnable(c, 0))
        .method("postDelayed", "(Ljava/lang/Runnable;J)Z", |c| post_runnable(c, c.long(1).max(0)))
        .method("postAtTime", "(Ljava/lang/Runnable;J)Z", |c| {
            let delay = c.long(1) - c.vm.now_ms() as i64;
            post_runnable(c, delay.max(0))
        })
        .method("postAtFrontOfQueue", "(Ljava/lang/Runnable;)Z", |c| {
            let m = new_message(c.vm);
            c.vm.set_field(m, "callback", c.args[0]);
            send(c, m, i64::MIN)
        })
        .method("obtainMessage", "()Landroid/os/Message;", |c| obtain(c, Some(c.this), 0, 0, 0, None))
        .method("obtainMessage", "(I)Landroid/os/Message;", |c| obtain(c, Some(c.this), c.int(0), 0, 0, None))
        .method("obtainMessage", "(ILjava/lang/Object;)Landroid/os/Message;", |c| {
            obtain(c, Some(c.this), c.int(0), 0, 0, c.obj(1))
        })
        .method("obtainMessage", "(III)Landroid/os/Message;", |c| {
            obtain(c, Some(c.this), c.int(0), c.int(1), c.int(2), None)
        })
        .method("removeMessages", "(I)V", |c| {
            let w = c.int(0);
            remove_where(c, move |vm, m| {
                vm.get_field(m, "what").as_int() != w || vm.get_field(m, "callback").as_ref().is_some()
            });
            void()
        })
        .method("removeCallbacks", "(Ljava/lang/Runnable;)V", |c| {
            let r = c.obj(0);
            remove_where(c, move |vm, m| vm.get_field(m, "callback").as_ref() != r);
            void()
        })
        .method("removeCallbacksAndMessages", "(Ljava/lang/Object;)V", |c| {
            let o = c.obj(0);
            remove_where(c, move |vm, m| o.is_some() && vm.get_field(m, "obj").as_ref() != o);
            void()
        })
        .method("hasMessages", "(I)Z", |c| {
            let w = c.int(0);
            boolean(has_where(c, move |vm, m| {
                vm.get_field(m, "what").as_int() == w && vm.get_field(m, "callback").as_ref().is_none()
            }))
        });
    vm.define("android/os/Handler$Callback").interface();
    vm.define("android/os/MessageQueue$IdleHandler").interface();
}

fn prepare(c: &mut Call) -> JResult {
    let pid = c.cpu.processor_id();
    let have = c.vm.st().loopers.contains_key(&pid);
    if have {
        return Err(c.vm.throw_new("java/lang/RuntimeException", "Only one Looper may be created per thread"));
    }
    let l = new_looper(c.vm);
    c.vm.st().loopers.insert(pid, l);
    void()
}

fn post_runnable(c: &mut Call, delay: i64) -> JResult {
    let m = new_message(c.vm);
    c.vm.set_field(m, "callback", c.args[0]);
    send(c, m, c.vm.now_ms() as i64 + delay)
}

impl Vm {
    /// Remember that `view` needs its native `layoutSubviews` called by the next [`Vm::run_layout`].
    pub fn mark_layout(&self, view: Obj) {
        let mut st = self.st();
        if !st.dirty_views.contains(&view) {
            st.dirty_views.push(view);
        }
    }

    /// The layout pass of the fake Android UI thread: call the native `callLayoutSubviews(mObject)` of every
    /// view marked since the last pass. Views marked while this runs wait for the next pass. Returns how many
    /// views were laid out.
    pub fn run_layout(&self, cpu: &mut Cpu) -> Result<usize, Throw> {
        let views = std::mem::take(&mut self.st().dirty_views);
        for v in &views {
            let id = self.get_field(*v, "mObject");
            self.call_method(cpu, *v, "callLayoutSubviews", "(I)V", &[id])?;
        }
        // A window that was just ordered front receives window focus once it is laid out (Android delivers
        // `onWindowFocusChanged(true)` after attaching it).
        if std::mem::take(&mut self.st().focus_pending) {
            self.call_static(cpu, "com/apportable/ui/Window", "nativeOnWindowFocusChanged", "(Z)V", &[Value::Int(1)])?;
        }
        Ok(views.len())
    }
}

//! `<pthread.h>` on top of the cooperative scheduler in [`crate::sched`]. Bionic's mutex, condition variable, once control and
//! attribute objects are one 32-bit word each (`pthread_rwlock_t` is 40 bytes), so the host keeps
//! the real state in side tables keyed by the object's guest address and only maintains the word
//! the static initialisers produce (the mutex type lives in bits 14..15).
//!
//! A call that would block forever (every guest thread blocked) stops the guest with a clear message
//! instead of hanging the host.

use crate::guest::{rd32, wr, wr32};
use crate::layout::{attr, errno, sync, SIGSET_SIZE};
use crate::sched::{self, block, ctx, notify, WaitEnd};
use crate::state::{MutexState, RwState};
use crate::{builtin, lock, Impl, Shared};
use bhpc_cpu::{Cpu, Stop};
use std::time::{Duration, Instant};

macro_rules! args {
    ($c:expr; $($n:ident),+) => {
        let mut a = $c.args();
        $(let $n = a.u32($c);)+
    };
}

// ---------- once ----------

/// Guest address of the `__pthread_once_return` continuation thunk (registered on first use).
fn once_return(c: &mut Cpu, st: &Shared) -> u32 {
    let known = lock(st).once_return;
    if known != 0 {
        return known;
    }
    let f = builtin("__pthread_once_return", st).expect("built-in");
    let addr = c.register_shared_thunk("__pthread_once_return", f).expect("thunk page has room");
    lock(st).once_return = addr;
    addr
}

fn pthread_once(c: &mut Cpu, st: &Shared) {
    args!(c; once, init);
    let Some(state) = rd32(c, once) else { return };
    if state != 0 {
        return c.ret_u32(0);
    }
    // Run `init` as a tail call whose return lands in the continuation thunk, which pops the
    // original return address again (a nested JIT run is not allowed).
    let ret = once_return(c, st);
    let sp = c.sp() - 8;
    if !(wr32(c, once, sync::ONCE_DONE) && wr32(c, sp, c.reg(12))) {
        return;
    }
    c.set_sp(sp);
    c.set_reg(14, ret);
    c.tail_call(init);
}

fn once_return_thunk(c: &mut Cpu, _: &Shared) {
    let sp = c.sp();
    let orig = c.mem().read_u32(sp).unwrap_or(0);
    c.set_sp(sp + 8);
    c.ret_u32(0);
    c.tail_call(orig);
}

// ---------- keys ----------

fn key_create(c: &mut Cpu, st: &Shared) {
    args!(c; out, destructor);
    let mut s = lock(st);
    if s.keys.len() as u32 >= sync::KEYS_MAX {
        drop(s);
        return c.ret_u32(errno::EAGAIN);
    }
    let k = s.next_key;
    s.next_key += 1;
    s.keys.insert(k, (0, destructor));
    drop(s);
    if wr32(c, out, k) {
        c.ret_u32(0);
    }
}

fn key_delete(c: &mut Cpu, st: &Shared) {
    let k = c.args().u32(c);
    let mut s = lock(st);
    let known = s.keys.remove(&k).is_some();
    s.tls.retain(|(_, key), _| *key != k);
    drop(s);
    c.ret_u32(if known { 0 } else { errno::EINVAL });
}

fn getspecific(c: &mut Cpu, st: &Shared) {
    let k = c.args().u32(c);
    let v = lock(st).tls.get(&(ctx().tid, k)).copied().unwrap_or(0);
    c.ret_u32(v);
}

fn setspecific(c: &mut Cpu, st: &Shared) {
    args!(c; k, v);
    let mut s = lock(st);
    let ok = s.keys.contains_key(&k);
    if ok {
        s.tls.insert((ctx().tid, k), v);
    }
    drop(s);
    c.ret_u32(if ok { 0 } else { errno::EINVAL });
}

// ---------- mutex ----------

fn mutex_entry(c: &mut Cpu, st: &Shared, m: u32) -> Option<MutexState> {
    if let Some(e) = lock(st).mutexes.get(&m) {
        return Some(*e);
    }
    let word = rd32(c, m)?;
    let e = MutexState { kind: (word >> sync::MUTEX_TYPE_SHIFT) & 3, count: 0, owner: 0 };
    lock(st).mutexes.insert(m, e);
    Some(e)
}

fn mutex_init(c: &mut Cpu, st: &Shared) {
    args!(c; m, at);
    let kind = if at == 0 {
        0
    } else {
        match rd32(c, at) {
            Some(k) => k & 3,
            None => return,
        }
    };
    if wr32(c, m, kind << sync::MUTEX_TYPE_SHIFT) {
        lock(st).mutexes.insert(m, MutexState { kind, count: 0, owner: 0 });
        c.ret_u32(0);
    }
}

fn mutex_destroy(c: &mut Cpu, st: &Shared) {
    let m = c.args().u32(c);
    lock(st).mutexes.remove(&m);
    c.ret_u32(0);
}

/// Take the mutex for the calling thread, blocking while another thread (or, for a normal mutex, the
/// caller itself) holds it. `None` when the guest was stopped (deadlock).
fn acquire_mutex(c: &mut Cpu, st: &Shared, m: u32, count: u32) -> Option<u32> {
    let me = ctx().tid;
    loop {
        let e = mutex_entry(c, st, m)?;
        let mine = e.count > 0 && e.owner == me;
        if e.count == 0 || (mine && e.kind == sync::MUTEX_RECURSIVE) {
            lock(st).mutexes.insert(m, MutexState { count: e.count + count, owner: me, ..e });
            return Some(0);
        }
        if mine && e.kind == sync::MUTEX_ERRORCHECK {
            return Some(errno::EDEADLK);
        }
        block(c, st, None, "pthread_mutex_lock")?;
    }
}

fn mutex_lock(c: &mut Cpu, st: &Shared) {
    let m = c.args().u32(c);
    if let Some(r) = acquire_mutex(c, st, m, 1) {
        c.ret_u32(r);
    }
}

fn mutex_trylock(c: &mut Cpu, st: &Shared) {
    let m = c.args().u32(c);
    let Some(e) = mutex_entry(c, st, m) else { return };
    let me = ctx().tid;
    if e.count > 0 && !(e.kind == sync::MUTEX_RECURSIVE && e.owner == me) {
        return c.ret_u32(errno::EBUSY);
    }
    lock(st).mutexes.insert(m, MutexState { count: e.count + 1, owner: me, ..e });
    c.ret_u32(0);
}

fn mutex_unlock(c: &mut Cpu, st: &Shared) {
    let m = c.args().u32(c);
    let Some(e) = mutex_entry(c, st, m) else { return };
    if e.count == 0 {
        // Unlocking an unlocked normal mutex is tolerated; the checking kinds report it.
        return c.ret_u32(if e.kind == sync::MUTEX_NORMAL { 0 } else { errno::EPERM });
    }
    if e.kind != sync::MUTEX_NORMAL && e.owner != ctx().tid {
        return c.ret_u32(errno::EPERM);
    }
    let count = if e.kind == sync::MUTEX_RECURSIVE { e.count - 1 } else { 0 };
    lock(st).mutexes.insert(m, MutexState { count, ..e });
    if count == 0 {
        notify(st);
    }
    c.ret_u32(0);
}

fn mutexattr_init(c: &mut Cpu, _: &Shared) {
    let at = c.args().u32(c);
    if wr32(c, at, 0) {
        c.ret_u32(0);
    }
}

fn mutexattr_settype(c: &mut Cpu, _: &Shared) {
    args!(c; at, kind);
    if kind > sync::MUTEX_ERRORCHECK {
        return c.ret_u32(errno::EINVAL);
    }
    if wr32(c, at, kind) {
        c.ret_u32(0);
    }
}

fn mutexattr_gettype(c: &mut Cpu, _: &Shared) {
    args!(c; at, out);
    let Some(k) = rd32(c, at) else { return };
    if wr32(c, out, k) {
        c.ret_u32(0);
    }
}

fn ok_zero(c: &mut Cpu, _: &Shared) {
    c.ret_u32(0);
}

// ---------- condition variables ----------

fn cond_init(c: &mut Cpu, _: &Shared) {
    let cv = c.args().u32(c);
    if wr32(c, cv, 0) {
        c.ret_u32(0);
    }
}

/// Wait on `cv`, releasing `m` meanwhile. Returns the result code, or `None` if the guest was stopped.
fn cond_wait_common(c: &mut Cpu, st: &Shared, cv: u32, m: u32, deadline: Option<Instant>) -> Option<u32> {
    let e = mutex_entry(c, st, m)?;
    let ticket = {
        let mut s = lock(st);
        let t = s.next_wait;
        s.next_wait += 1;
        s.cond_q.entry(cv).or_default().push_back(t);
        s.mutexes.insert(m, MutexState { count: 0, ..e });
        t
    };
    notify(st);
    let result = loop {
        if lock(st).woken.remove(&ticket) {
            break 0;
        }
        if block(c, st, deadline, "pthread_cond_wait")? == WaitEnd::TimedOut {
            let mut s = lock(st);
            s.cond_q.entry(cv).or_default().retain(|t| *t != ticket);
            // A signal that raced with the timeout still counts as a wake-up.
            break if s.woken.remove(&ticket) { 0 } else { errno::ETIMEDOUT };
        }
    };
    acquire_mutex(c, st, m, e.count.max(1))?;
    Some(result)
}

fn cond_wait(c: &mut Cpu, st: &Shared) {
    args!(c; cv, m);
    if let Some(r) = cond_wait_common(c, st, cv, m, None) {
        c.ret_u32(r);
    }
}

/// Which clock a timed wait's timespec is relative to.
#[derive(Clone, Copy)]
enum Timeout {
    /// Absolute `CLOCK_REALTIME`.
    Realtime,
    /// Absolute `CLOCK_MONOTONIC`.
    Monotonic,
    /// Relative to now.
    Relative,
}

fn cond_timedwait_with(c: &mut Cpu, st: &Shared, kind: Timeout) {
    args!(c; cv, m, ts);
    let (Some(sec), Some(nsec)) = (rd32(c, ts), rd32(c, ts + 4)) else { return };
    if nsec >= 1_000_000_000 {
        return c.ret_u32(errno::EINVAL);
    }
    let want = u64::from(sec) * 1_000_000_000 + u64::from(nsec);
    let remaining = match kind {
        Timeout::Relative => want,
        Timeout::Realtime | Timeout::Monotonic => {
            let mut s = lock(st);
            let (now_s, now_ns) =
                if matches!(kind, Timeout::Realtime) { s.clock.realtime() } else { s.clock.monotonic() };
            want.saturating_sub((now_s.max(0) as u64) * 1_000_000_000 + u64::from(now_ns))
        }
    };
    let deadline = Instant::now() + Duration::from_nanos(remaining);
    if let Some(r) = cond_wait_common(c, st, cv, m, Some(deadline)) {
        c.ret_u32(r);
    }
}

fn cond_timedwait(c: &mut Cpu, st: &Shared) {
    cond_timedwait_with(c, st, Timeout::Realtime);
}
fn cond_timedwait_monotonic(c: &mut Cpu, st: &Shared) {
    cond_timedwait_with(c, st, Timeout::Monotonic);
}
fn cond_timedwait_relative(c: &mut Cpu, st: &Shared) {
    cond_timedwait_with(c, st, Timeout::Relative);
}

/// Wake `n` waiters of `cv` (`usize::MAX` = all).
fn wake(st: &Shared, cv: u32, n: usize) {
    let mut s = lock(st);
    for _ in 0..n {
        let Some(t) = s.cond_q.get_mut(&cv).and_then(std::collections::VecDeque::pop_front) else { break };
        s.woken.insert(t);
    }
    drop(s);
    notify(st);
}

fn cond_signal(c: &mut Cpu, st: &Shared) {
    let cv = c.args().u32(c);
    wake(st, cv, 1);
    c.ret_u32(0);
}

fn cond_broadcast(c: &mut Cpu, st: &Shared) {
    let cv = c.args().u32(c);
    wake(st, cv, usize::MAX);
    c.ret_u32(0);
}

// ---------- read/write locks ----------

fn rwlock_init(c: &mut Cpu, st: &Shared) {
    args!(c; l, _attr);
    if wr(c, l, &[0u8; sync::RWLOCK_SIZE as usize]) {
        lock(st).rwlocks.remove(&l);
        c.ret_u32(0);
    }
}

fn rwlock_destroy(c: &mut Cpu, st: &Shared) {
    let l = c.args().u32(c);
    lock(st).rwlocks.remove(&l);
    c.ret_u32(0);
}

fn rw_state(st: &Shared, l: u32) -> RwState {
    lock(st).rwlocks.get(&l).copied().unwrap_or_default()
}

fn rw_set(st: &Shared, l: u32, v: RwState) {
    lock(st).rwlocks.insert(l, v);
}

fn rwlock_rdlock(c: &mut Cpu, st: &Shared) {
    let l = c.args().u32(c);
    loop {
        let s = rw_state(st, l);
        if !s.writer {
            rw_set(st, l, RwState { readers: s.readers + 1, ..s });
            return c.ret_u32(0);
        }
        if block(c, st, None, "pthread_rwlock_rdlock").is_none() {
            return;
        }
    }
}

fn rwlock_tryrdlock(c: &mut Cpu, st: &Shared) {
    let l = c.args().u32(c);
    let s = rw_state(st, l);
    if s.writer {
        return c.ret_u32(errno::EBUSY);
    }
    rw_set(st, l, RwState { readers: s.readers + 1, ..s });
    c.ret_u32(0);
}

fn rwlock_wrlock(c: &mut Cpu, st: &Shared) {
    let l = c.args().u32(c);
    loop {
        let s = rw_state(st, l);
        if !s.writer && s.readers == 0 {
            rw_set(st, l, RwState { readers: 0, writer: true });
            return c.ret_u32(0);
        }
        if block(c, st, None, "pthread_rwlock_wrlock").is_none() {
            return;
        }
    }
}

fn rwlock_trywrlock(c: &mut Cpu, st: &Shared) {
    let l = c.args().u32(c);
    let s = rw_state(st, l);
    if s.writer || s.readers > 0 {
        return c.ret_u32(errno::EBUSY);
    }
    rw_set(st, l, RwState { readers: 0, writer: true });
    c.ret_u32(0);
}

fn rwlock_unlock(c: &mut Cpu, st: &Shared) {
    let l = c.args().u32(c);
    let s = rw_state(st, l);
    if s.writer {
        rw_set(st, l, RwState::default());
    } else if s.readers > 0 {
        rw_set(st, l, RwState { readers: s.readers - 1, ..s });
    } else {
        return c.ret_u32(errno::EPERM);
    }
    notify(st);
    c.ret_u32(0);
}

// ---------- thread identity ----------

fn pthread_self(c: &mut Cpu, st: &Shared) {
    let t = sched::self_handle(st);
    c.ret_u32(t);
}

fn pthread_equal(c: &mut Cpu, _: &Shared) {
    args!(c; a, b);
    c.ret_u32((a == b) as u32);
}

/// `pthread_create(&t, attr, start, arg)`: a new guest thread on its own CPU, scheduled cooperatively.
fn pthread_create(c: &mut Cpu, st: &Shared) {
    args!(c; out, attr_ptr, start, arg);
    if start == 0 {
        return c.ret_u32(errno::EINVAL);
    }
    let (stack, detached) = if attr_ptr == 0 {
        (attr::DEFAULT_STACK_SIZE, false)
    } else {
        let (Some(flags), Some(size)) = (rd32(c, attr_ptr + attr::FLAGS), rd32(c, attr_ptr + attr::STACK_SIZE)) else {
            return;
        };
        (size, flags & attr::CREATE_DETACHED != 0)
    };
    match sched::spawn(c, st, start, arg, stack, detached) {
        Ok(handle) => {
            if wr32(c, out, handle) {
                c.ret_u32(0);
            }
        }
        Err(e) => c.ret_u32(e),
    }
}

/// `pthread_join`: wait for the thread to finish and collect its return value.
fn pthread_join(c: &mut Cpu, st: &Shared) {
    args!(c; t, ret);
    if t == sched::self_handle(st) {
        return c.ret_u32(errno::EDEADLK);
    }
    loop {
        let rec = lock(st).threads.get(&t).cloned();
        let Some(rec) = rec else { return c.ret_u32(errno::ESRCH) };
        if rec.detached {
            return c.ret_u32(errno::EINVAL);
        }
        if rec.finished {
            if ret != 0 && !wr32(c, ret, rec.retval) {
                return;
            }
            let mut s = lock(st);
            s.threads.remove(&t);
            s.heap.free(t);
            drop(s);
            return c.ret_u32(0);
        }
        if block(c, st, None, "pthread_join").is_none() {
            return;
        }
    }
}

/// `pthread_detach`: nobody will join; a finished thread is reaped at once.
fn pthread_detach(c: &mut Cpu, st: &Shared) {
    let t = c.args().u32(c);
    let mut s = lock(st);
    let r = match s.threads.get_mut(&t) {
        None => errno::ESRCH,
        Some(rec) if rec.detached => errno::EINVAL,
        Some(rec) => {
            rec.detached = true;
            if rec.finished {
                s.threads.remove(&t);
                s.heap.free(t);
            }
            0
        }
    };
    drop(s);
    c.ret_u32(r);
}

/// `pthread_exit(value)`: end the calling thread.
fn pthread_exit(c: &mut Cpu, st: &Shared) {
    let value = c.args().u32(c);
    let handle = ctx().handle;
    if handle == 0 {
        return c.request_stop(Stop::Host("the main thread called pthread_exit".into()));
    }
    if let Some(t) = lock(st).threads.get_mut(&handle) {
        t.retval = value;
    }
    c.request_stop(Stop::Host(sched::EXIT_MARK.into()));
}

fn sched_yield(c: &mut Cpu, st: &Shared) {
    let sched = lock(st).sched.clone();
    sched.yield_now();
    c.ret_u32(0);
}

fn pthread_sigmask(c: &mut Cpu, _: &Shared) {
    args!(c; _how, _set, old);
    if old != 0 && !wr(c, old, &[0u8; SIGSET_SIZE as usize]) {
        return;
    }
    c.ret_u32(0);
}

// ---------- thread attributes ----------

fn attr_init(c: &mut Cpu, _: &Shared) {
    let at = c.args().u32(c);
    let mut b = [0u8; attr::SIZE as usize];
    b[attr::STACK_SIZE as usize..][..4].copy_from_slice(&attr::DEFAULT_STACK_SIZE.to_le_bytes());
    b[attr::GUARD_SIZE as usize..][..4].copy_from_slice(&attr::DEFAULT_GUARD_SIZE.to_le_bytes());
    if wr(c, at, &b) {
        c.ret_u32(0);
    }
}

/// `pthread_attr_get*(attr, out)`: store the word at `off` into `*out`.
fn attr_get(c: &mut Cpu, off: u32) {
    args!(c; at, out);
    let Some(v) = rd32(c, at + off) else { return };
    if wr32(c, out, v) {
        c.ret_u32(0);
    }
}

/// `pthread_attr_set*(attr, v)`: store `v` at `off`.
fn attr_set(c: &mut Cpu, off: u32) {
    args!(c; at, v);
    if wr32(c, at + off, v) {
        c.ret_u32(0);
    }
}

fn attr_getdetachstate(c: &mut Cpu, _: &Shared) {
    args!(c; at, out);
    let Some(f) = rd32(c, at + attr::FLAGS) else { return };
    if wr32(c, out, f & attr::CREATE_DETACHED) {
        c.ret_u32(0);
    }
}

fn attr_setdetachstate(c: &mut Cpu, _: &Shared) {
    args!(c; at, state);
    if state > 1 {
        return c.ret_u32(errno::EINVAL);
    }
    let Some(f) = rd32(c, at + attr::FLAGS) else { return };
    if wr32(c, at + attr::FLAGS, f & !attr::CREATE_DETACHED | state) {
        c.ret_u32(0);
    }
}

fn attr_getstacksize(c: &mut Cpu, _: &Shared) {
    attr_get(c, attr::STACK_SIZE)
}
fn attr_setstacksize(c: &mut Cpu, _: &Shared) {
    let size = c.reg(1);
    if size < 16 * 1024 {
        return c.ret_u32(errno::EINVAL);
    }
    attr_set(c, attr::STACK_SIZE)
}
fn attr_getguardsize(c: &mut Cpu, _: &Shared) {
    attr_get(c, attr::GUARD_SIZE)
}
fn attr_setguardsize(c: &mut Cpu, _: &Shared) {
    attr_set(c, attr::GUARD_SIZE)
}
fn attr_getschedpolicy(c: &mut Cpu, _: &Shared) {
    attr_get(c, attr::SCHED_POLICY)
}
fn attr_setschedpolicy(c: &mut Cpu, _: &Shared) {
    attr_set(c, attr::SCHED_POLICY)
}
fn attr_getstackaddr(c: &mut Cpu, _: &Shared) {
    attr_get(c, attr::STACK_BASE)
}
fn attr_setstackaddr(c: &mut Cpu, _: &Shared) {
    attr_set(c, attr::STACK_BASE)
}

/// `param` is a `struct sched_param { int sched_priority; }`.
fn attr_getschedparam(c: &mut Cpu, _: &Shared) {
    args!(c; at, param);
    let Some(v) = rd32(c, at + attr::SCHED_PRIORITY) else { return };
    if wr32(c, param, v) {
        c.ret_u32(0);
    }
}

fn attr_setschedparam(c: &mut Cpu, _: &Shared) {
    args!(c; at, param);
    let Some(v) = rd32(c, param) else { return };
    if wr32(c, at + attr::SCHED_PRIORITY, v) {
        c.ret_u32(0);
    }
}

fn attr_getstack(c: &mut Cpu, _: &Shared) {
    args!(c; at, base_out, size_out);
    let (Some(b), Some(s)) = (rd32(c, at + attr::STACK_BASE), rd32(c, at + attr::STACK_SIZE)) else { return };
    if wr32(c, base_out, b) && wr32(c, size_out, s) {
        c.ret_u32(0);
    }
}

fn attr_setstack(c: &mut Cpu, _: &Shared) {
    args!(c; at, base, size);
    if size < 16 * 1024 {
        return c.ret_u32(errno::EINVAL);
    }
    if wr32(c, at + attr::STACK_BASE, base) && wr32(c, at + attr::STACK_SIZE, size) {
        c.ret_u32(0);
    }
}

/// Only `PTHREAD_SCOPE_SYSTEM` (0) is supported.
fn attr_setscope(c: &mut Cpu, _: &Shared) {
    let scope = c.reg(1);
    c.ret_u32(if scope == 0 { 0 } else { errno::ENOTSUP });
}

pub(crate) const TABLE: &[(&str, Impl)] = &[
    ("pthread_once", pthread_once),
    ("__pthread_once_return", once_return_thunk),
    ("pthread_key_create", key_create),
    ("pthread_key_delete", key_delete),
    ("pthread_getspecific", getspecific),
    ("pthread_setspecific", setspecific),
    ("pthread_mutex_init", mutex_init),
    ("pthread_mutex_destroy", mutex_destroy),
    ("pthread_mutex_lock", mutex_lock),
    ("pthread_mutex_trylock", mutex_trylock),
    ("pthread_mutex_unlock", mutex_unlock),
    ("pthread_mutexattr_init", mutexattr_init),
    ("pthread_mutexattr_destroy", ok_zero),
    ("pthread_mutexattr_settype", mutexattr_settype),
    ("pthread_mutexattr_gettype", mutexattr_gettype),
    ("pthread_mutexattr_setpshared", ok_zero),
    ("pthread_cond_init", cond_init),
    ("pthread_cond_destroy", ok_zero),
    ("pthread_cond_signal", cond_signal),
    ("pthread_cond_broadcast", cond_broadcast),
    ("pthread_cond_wait", cond_wait),
    ("pthread_cond_timedwait", cond_timedwait),
    ("pthread_cond_timedwait_relative_np", cond_timedwait_relative),
    ("pthread_cond_timedwait_monotonic", cond_timedwait_monotonic),
    ("pthread_condattr_init", mutexattr_init),
    ("pthread_condattr_destroy", ok_zero),
    ("pthread_condattr_setpshared", ok_zero),
    ("pthread_rwlock_init", rwlock_init),
    ("pthread_rwlock_destroy", rwlock_destroy),
    ("pthread_rwlock_rdlock", rwlock_rdlock),
    ("pthread_rwlock_tryrdlock", rwlock_tryrdlock),
    ("pthread_rwlock_wrlock", rwlock_wrlock),
    ("pthread_rwlock_trywrlock", rwlock_trywrlock),
    ("pthread_rwlock_unlock", rwlock_unlock),
    ("pthread_self", pthread_self),
    ("pthread_equal", pthread_equal),
    ("pthread_join", pthread_join),
    ("pthread_create", pthread_create),
    ("pthread_exit", pthread_exit),
    ("pthread_detach", pthread_detach),
    ("pthread_kill", ok_zero),
    ("pthread_sigmask", pthread_sigmask),
    ("sched_yield", sched_yield),
    ("pthread_attr_init", attr_init),
    ("pthread_attr_destroy", ok_zero),
    ("pthread_attr_getdetachstate", attr_getdetachstate),
    ("pthread_attr_setdetachstate", attr_setdetachstate),
    ("pthread_attr_getstacksize", attr_getstacksize),
    ("pthread_attr_setstacksize", attr_setstacksize),
    ("pthread_attr_getguardsize", attr_getguardsize),
    ("pthread_attr_setguardsize", attr_setguardsize),
    ("pthread_attr_getschedpolicy", attr_getschedpolicy),
    ("pthread_attr_setschedpolicy", attr_setschedpolicy),
    ("pthread_attr_getstackaddr", attr_getstackaddr),
    ("pthread_attr_setstackaddr", attr_setstackaddr),
    ("pthread_attr_getschedparam", attr_getschedparam),
    ("pthread_attr_setschedparam", attr_setschedparam),
    ("pthread_attr_getstack", attr_getstack),
    ("pthread_attr_setstack", attr_setstack),
    ("pthread_attr_setscope", attr_setscope),
];

#[cfg(test)]
mod tests {
    use crate::layout::attr;
    use crate::tests_env::{Env, SCRATCH};
    use bhpc_cpu::Stop;

    const M: u32 = SCRATCH + 0x100;
    const AT: u32 = SCRATCH + 0x200;
    const OUT: u32 = SCRATCH + 0x300;
    const BAD: u32 = 0x10;

    #[test]
    fn once_runs_init_once_via_tail_call() {
        let mut e = Env::new();
        let code = SCRATCH + 0x400;
        let (flag, once) = (SCRATCH + 0x800, SCRATCH + 0x804);
        // init: *flag += 5 (mov r1,#5; ldr r2,[pc,#4]; str r1,[r2]; bx lr; .word flag)
        for (i, w) in [0xE3A0_1005u32, 0xE59F_2004, 0xE582_1000, 0xE12F_FF1E, flag].iter().enumerate() {
            e.mem().write_u32(code + 4 * i as u32, *w).unwrap();
        }
        assert_eq!(e.call("pthread_once", &[once, code]).r0, 0);
        assert_eq!(e.mem().read_u32(flag).unwrap(), 5);
        assert_eq!(e.mem().read_u32(once).unwrap(), 2);
        assert_eq!(e.call("pthread_once", &[once, code]).r0, 0);
        assert_eq!(e.mem().read_u32(flag).unwrap(), 5, "second call is a no-op");
        // A second control reuses the already registered continuation thunk.
        let once2 = SCRATCH + 0x808;
        assert_eq!(e.call("pthread_once", &[once2, code]).r0, 0);
        assert_eq!(e.mem().read_u32(flag).unwrap(), 5);
        assert!(matches!(e.try_call("pthread_once", &[BAD, code]), Err(Stop::Fault(_))));
        // The continuation thunk is reachable by name.
        assert!(e.cpu.thunk_addr("__pthread_once_return").is_some());
    }

    #[test]
    fn once_with_unwritable_stack_faults() {
        let mut e = Env::new();
        let once = SCRATCH + 0x804;
        let sp = e.cpu.sp();
        e.cpu.set_sp(0x20);
        assert!(matches!(e.try_call("pthread_once", &[once, SCRATCH]), Err(Stop::Fault(_))));
        e.cpu.set_sp(sp);
        // The write of the "done" flag fails for an unmapped control (checked above); here the flag is fine.
        e.mem().write_u32(once, 0).unwrap();
        let _ = sp;
    }

    #[test]
    fn keys() {
        let mut e = Env::new();
        assert_eq!(e.call("pthread_key_create", &[OUT, 0]).r0, 0);
        let k = e.mem().read_u32(OUT).unwrap();
        assert_ne!(k, 0);
        assert_eq!(e.call("pthread_getspecific", &[k]).r0, 0);
        assert_eq!(e.call("pthread_setspecific", &[k, 77]).r0, 0);
        assert_eq!(e.call("pthread_getspecific", &[k]).r0, 77);
        assert_eq!(e.call("pthread_key_delete", &[k]).r0, 0);
        assert_eq!(e.call("pthread_key_delete", &[k]).r0, 22);
        assert_eq!(e.call("pthread_setspecific", &[k, 1]).r0, 22);
        assert_eq!(e.call("pthread_getspecific", &[k]).r0, 0);
        assert!(matches!(e.try_call("pthread_key_create", &[BAD, 0]), Err(Stop::Fault(_))));
        // The table is bounded by PTHREAD_KEYS_MAX.
        let mut results = Vec::new();
        for _ in 0..140 {
            results.push(e.call("pthread_key_create", &[OUT, 0]).r0);
        }
        assert_eq!(results.iter().filter(|r| **r == 0).count(), 127, "one slot was freed/used above");
        assert!(results.contains(&11));
    }

    #[test]
    fn normal_mutex() {
        let mut e = Env::new();
        assert_eq!(e.call("pthread_mutex_lock", &[M]).r0, 0);
        assert_eq!(e.call("pthread_mutex_trylock", &[M]).r0, 16);
        assert_eq!(e.call("pthread_mutex_unlock", &[M]).r0, 0);
        assert_eq!(e.call("pthread_mutex_trylock", &[M]).r0, 0);
        assert_eq!(e.call("pthread_mutex_unlock", &[M]).r0, 0);
        assert_eq!(e.call("pthread_mutex_unlock", &[M]).r0, 0, "unlocking an unlocked normal mutex is tolerated");
        assert_eq!(e.call("pthread_mutex_lock", &[M]).r0, 0);
        let r = e.try_call("pthread_mutex_lock", &[M]);
        assert!(matches!(r, Err(Stop::Host(ref m)) if m.contains("deadlock")), "{r:?}");
        assert_eq!(e.call("pthread_mutex_destroy", &[M]).r0, 0);
        assert_eq!(e.call("pthread_mutex_lock", &[M]).r0, 0, "destroy forgets the lock state");
        for f in ["pthread_mutex_lock", "pthread_mutex_trylock", "pthread_mutex_unlock"] {
            assert!(matches!(e.try_call(f, &[BAD]), Err(Stop::Fault(_))), "{f}");
        }
    }

    #[test]
    fn recursive_and_errorcheck_mutexes() {
        let mut e = Env::new();
        e.mem().write_u32(M, 0x4000).unwrap();
        assert_eq!(e.call("pthread_mutex_lock", &[M]).r0, 0);
        assert_eq!(e.call("pthread_mutex_lock", &[M]).r0, 0);
        assert_eq!(e.call("pthread_mutex_trylock", &[M]).r0, 0);
        for _ in 0..3 {
            assert_eq!(e.call("pthread_mutex_unlock", &[M]).r0, 0);
        }
        assert_eq!(e.call("pthread_mutex_unlock", &[M]).r0, 1, "unbalanced unlock");
        let m2 = M + 8;
        e.mem().write_u32(m2, 0x8000).unwrap();
        assert_eq!(e.call("pthread_mutex_lock", &[m2]).r0, 0);
        assert_eq!(e.call("pthread_mutex_lock", &[m2]).r0, 35, "error-checking mutex reports the deadlock");
        assert_eq!(e.call("pthread_mutex_unlock", &[m2]).r0, 0);
        assert_eq!(e.call("pthread_mutex_unlock", &[m2]).r0, 1);
    }

    #[test]
    fn mutex_init_and_attributes() {
        let mut e = Env::new();
        let m = M + 0x20;
        assert_eq!(e.call("pthread_mutexattr_init", &[AT]).r0, 0);
        assert_eq!(e.call("pthread_mutexattr_settype", &[AT, 1]).r0, 0);
        let mut ty = SCRATCH + 0x340;
        assert_eq!(e.call("pthread_mutexattr_gettype", &[AT, ty]).r0, 0);
        assert_eq!(e.mem().read_u32(ty).unwrap(), 1);
        assert_eq!(e.call("pthread_mutexattr_settype", &[AT, 7]).r0, 22);
        assert_eq!(e.call("pthread_mutexattr_setpshared", &[AT, 1]).r0, 0);
        e.mem().write_u32(m, 0xffff).unwrap();
        assert_eq!(e.call("pthread_mutex_init", &[m, AT]).r0, 0);
        assert_eq!(e.mem().read_u32(m).unwrap(), 0x4000);
        assert_eq!(e.call("pthread_mutex_lock", &[m]).r0, 0);
        assert_eq!(e.call("pthread_mutex_lock", &[m]).r0, 0, "recursive");
        assert_eq!(e.call("pthread_mutex_init", &[m, 0]).r0, 0);
        assert_eq!(e.mem().read_u32(m).unwrap(), 0);
        assert_eq!(e.call("pthread_mutex_trylock", &[m]).r0, 0);
        assert_eq!(e.call("pthread_mutexattr_destroy", &[AT]).r0, 0);
        ty += 4;
        assert!(matches!(e.try_call("pthread_mutexattr_init", &[BAD]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("pthread_mutexattr_settype", &[BAD, 1]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("pthread_mutexattr_gettype", &[BAD, ty]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("pthread_mutexattr_gettype", &[AT, BAD]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("pthread_mutex_init", &[m, BAD]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("pthread_mutex_init", &[BAD, 0]), Err(Stop::Fault(_))));
    }

    #[test]
    fn condition_variables() {
        let mut e = Env::new();
        assert_eq!(e.call("pthread_condattr_init", &[AT]).r0, 0);
        assert_eq!(e.call("pthread_condattr_setpshared", &[AT, 0]).r0, 0);
        assert_eq!(e.call("pthread_condattr_destroy", &[AT]).r0, 0);
        e.mem().write_u32(M, 9).unwrap();
        assert_eq!(e.call("pthread_cond_init", &[M, 0]).r0, 0);
        assert_eq!(e.mem().read_u32(M).unwrap(), 0);
        assert_eq!(e.call("pthread_cond_signal", &[M]).r0, 0);
        assert_eq!(e.call("pthread_cond_broadcast", &[M]).r0, 0);
        for f in ["pthread_cond_timedwait", "pthread_cond_timedwait_relative_np", "pthread_cond_timedwait_monotonic"] {
            assert_eq!(e.call(f, &[M, M + 8, AT]).r0, 110, "{f}");
        }
        let r = e.try_call("pthread_cond_wait", &[M, M + 8]);
        assert!(matches!(r, Err(Stop::Host(ref m)) if m.contains("pthread_cond_wait")), "{r:?}");
        assert_eq!(e.call("pthread_cond_destroy", &[M]).r0, 0);
        assert!(matches!(e.try_call("pthread_cond_init", &[BAD, 0]), Err(Stop::Fault(_))));
    }

    #[test]
    fn read_write_locks() {
        let mut e = Env::new();
        let l = SCRATCH + 0x500;
        e.mem().write_bytes(l, &[0xff; 40]).unwrap();
        assert_eq!(e.call("pthread_rwlock_init", &[l, 0]).r0, 0);
        assert_eq!(e.mem().read_bytes(l, 40).unwrap(), vec![0; 40]);
        assert_eq!(e.call("pthread_rwlock_rdlock", &[l]).r0, 0);
        assert_eq!(e.call("pthread_rwlock_tryrdlock", &[l]).r0, 0);
        assert_eq!(e.call("pthread_rwlock_trywrlock", &[l]).r0, 16);
        assert_eq!(e.call("pthread_rwlock_unlock", &[l]).r0, 0);
        assert_eq!(e.call("pthread_rwlock_unlock", &[l]).r0, 0);
        assert_eq!(e.call("pthread_rwlock_unlock", &[l]).r0, 1, "not locked");
        assert_eq!(e.call("pthread_rwlock_wrlock", &[l]).r0, 0);
        assert_eq!(e.call("pthread_rwlock_tryrdlock", &[l]).r0, 16);
        assert_eq!(e.call("pthread_rwlock_trywrlock", &[l]).r0, 16);
        let r = e.try_call("pthread_rwlock_rdlock", &[l]);
        assert!(matches!(r, Err(Stop::Host(_))), "{r:?}");
        let r = e.try_call("pthread_rwlock_wrlock", &[l]);
        assert!(matches!(r, Err(Stop::Host(_))), "{r:?}");
        assert_eq!(e.call("pthread_rwlock_unlock", &[l]).r0, 0);
        assert_eq!(e.call("pthread_rwlock_trywrlock", &[l]).r0, 0);
        assert_eq!(e.call("pthread_rwlock_unlock", &[l]).r0, 0);
        assert_eq!(e.call("pthread_rwlock_rdlock", &[l]).r0, 0);
        let r = e.try_call("pthread_rwlock_wrlock", &[l]);
        assert!(matches!(r, Err(Stop::Host(_))), "{r:?}");
        assert_eq!(e.call("pthread_rwlock_destroy", &[l]).r0, 0);
        assert_eq!(e.call("pthread_rwlock_wrlock", &[l]).r0, 0, "destroy forgets the state");
        assert!(matches!(e.try_call("pthread_rwlock_init", &[BAD, 0]), Err(Stop::Fault(_))));
    }

    #[test]
    fn thread_identity_and_misc() {
        let mut e = Env::new();
        let me = e.call("pthread_self", &[]).r0;
        assert_ne!(me, 0);
        assert_eq!(e.mem().read_bytes(me, 256).unwrap().len(), 256, "the handle points at readable memory");
        assert_eq!(e.call("pthread_equal", &[me, me]).r0, 1);
        assert_eq!(e.call("pthread_equal", &[me, 5]).r0, 0);
        assert_eq!(e.call("pthread_join", &[me, 0]).r0, 35, "joining yourself would deadlock");
        assert_eq!(e.call("pthread_detach", &[me]).r0, 3);
        assert_eq!(e.call("pthread_kill", &[me, 0]).r0, 0);
        assert_eq!(e.call("sched_yield", &[]).r0, 0);
        e.mem().write_u32(OUT, 0xffff).unwrap();
        assert_eq!(e.call("pthread_sigmask", &[0, 0, OUT]).r0, 0);
        assert_eq!(e.mem().read_u32(OUT).unwrap(), 0);
        assert_eq!(e.call("pthread_sigmask", &[0, 0, 0]).r0, 0);
        assert!(matches!(e.try_call("pthread_sigmask", &[0, 0, BAD]), Err(Stop::Fault(_))));
    }

    #[test]
    fn thread_attributes() {
        let mut e = Env::new();
        assert_eq!(e.call("pthread_attr_init", &[AT]).r0, 0);
        let m = e.mem();
        assert_eq!(m.read_u32(AT + attr::STACK_SIZE).unwrap(), 1024 * 1024 - 8192);
        assert_eq!(m.read_u32(AT + attr::GUARD_SIZE).unwrap(), 4096);
        let get = |e: &mut Env, f: &str| {
            assert_eq!(e.call(f, &[AT, OUT]).r0, 0, "{f}");
            e.mem().read_u32(OUT).unwrap()
        };
        assert_eq!(get(&mut e, "pthread_attr_getstacksize"), 1024 * 1024 - 8192);
        assert_eq!(get(&mut e, "pthread_attr_getguardsize"), 4096);
        assert_eq!(get(&mut e, "pthread_attr_getdetachstate"), 0);
        assert_eq!(get(&mut e, "pthread_attr_getschedpolicy"), 0);
        assert_eq!(get(&mut e, "pthread_attr_getstackaddr"), 0);
        assert_eq!(get(&mut e, "pthread_attr_getschedparam"), 0);
        assert_eq!(e.call("pthread_attr_setstacksize", &[AT, 1 << 20]).r0, 0);
        assert_eq!(e.call("pthread_attr_setstacksize", &[AT, 100]).r0, 22);
        assert_eq!(get(&mut e, "pthread_attr_getstacksize"), 1 << 20);
        assert_eq!(e.call("pthread_attr_setguardsize", &[AT, 8192]).r0, 0);
        assert_eq!(get(&mut e, "pthread_attr_getguardsize"), 8192);
        assert_eq!(e.call("pthread_attr_setdetachstate", &[AT, 1]).r0, 0);
        assert_eq!(get(&mut e, "pthread_attr_getdetachstate"), 1);
        assert_eq!(e.call("pthread_attr_setdetachstate", &[AT, 0]).r0, 0);
        assert_eq!(get(&mut e, "pthread_attr_getdetachstate"), 0);
        assert_eq!(e.call("pthread_attr_setdetachstate", &[AT, 2]).r0, 22);
        assert_eq!(e.call("pthread_attr_setschedpolicy", &[AT, 1]).r0, 0);
        assert_eq!(get(&mut e, "pthread_attr_getschedpolicy"), 1);
        assert_eq!(e.call("pthread_attr_setstackaddr", &[AT, 0x1234]).r0, 0);
        assert_eq!(get(&mut e, "pthread_attr_getstackaddr"), 0x1234);
        m.write_u32(OUT + 0x40, 5).unwrap();
        assert_eq!(e.call("pthread_attr_setschedparam", &[AT, OUT + 0x40]).r0, 0);
        assert_eq!(get(&mut e, "pthread_attr_getschedparam"), 5);
        assert_eq!(e.call("pthread_attr_setstack", &[AT, 0x8000, 1 << 16]).r0, 0);
        assert_eq!(e.call("pthread_attr_setstack", &[AT, 0x8000, 4]).r0, 22);
        assert_eq!(e.call("pthread_attr_getstack", &[AT, OUT, OUT + 4]).r0, 0);
        assert_eq!((m.read_u32(OUT).unwrap(), m.read_u32(OUT + 4).unwrap()), (0x8000, 1 << 16));
        assert_eq!(e.call("pthread_attr_setscope", &[AT, 0]).r0, 0);
        assert_eq!(e.call("pthread_attr_setscope", &[AT, 1]).r0, 95);
        assert_eq!(e.call("pthread_attr_destroy", &[AT]).r0, 0);
        for f in [
            "pthread_attr_init",
            "pthread_attr_setguardsize",
            "pthread_attr_setschedpolicy",
            "pthread_attr_setstackaddr",
            "pthread_attr_setstacksize",
            "pthread_attr_setstack",
            "pthread_attr_setschedparam",
        ] {
            assert!(matches!(e.try_call(f, &[BAD, 1 << 16, 1 << 16]), Err(Stop::Fault(_))), "{f}");
        }
        for f in [
            "pthread_attr_getstacksize",
            "pthread_attr_getdetachstate",
            "pthread_attr_getschedparam",
            "pthread_attr_getstack",
        ] {
            assert!(matches!(e.try_call(f, &[BAD, OUT, OUT]), Err(Stop::Fault(_))), "{f}");
            assert!(matches!(e.try_call(f, &[AT, BAD, BAD]), Err(Stop::Fault(_))), "{f}");
        }
        assert!(matches!(e.try_call("pthread_attr_setschedparam", &[AT, BAD]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("pthread_attr_setdetachstate", &[BAD, 0]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("pthread_attr_getstack", &[AT, OUT, BAD]), Err(Stop::Fault(_))));
    }
}

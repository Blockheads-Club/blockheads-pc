//! Cooperative guest-thread scheduler.
//!
//! Every guest thread is a host thread with its own [`Cpu`], but only the holder of the *baton* runs guest
//! code, so host functions never race and execution is easy to reason about. The baton is a FIFO ticket
//! lock. A thread gives it up only when it blocks (mutex, condition variable, join, sleep, futex), yields
//! or exits. Blocking is "poll with epochs": the blocked thread notes the current epoch, releases the baton
//! and sleeps until the epoch changes (any state change that could unblock someone calls [`Sched::notify`])
//! or its deadline passes, then queues for the baton and re-checks its condition.
//!
//! If every live thread is blocked without a deadline, the process is deadlocked: the waiters are woken with
//! [`WaitEnd::Deadlock`] and the guest is stopped with a message instead of hanging the host.

use crate::{lock, Shared};
use bhpc_cpu::{Cpu, Stop};
use std::cell::Cell;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

/// How a wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitEnd {
    /// Some state changed; re-check the condition.
    Woken,
    /// The deadline passed.
    TimedOut,
    /// Every thread is blocked forever, or the scheduler was shut down.
    Deadlock,
}

struct Inner {
    next_ticket: u64,
    serving: u64,
    epoch: u64,
    /// Threads blocked without a deadline since the last epoch change.
    blocked: u32,
    /// Live guest threads (the main thread counts).
    live: u32,
    deadlock: bool,
    shutdown: bool,
    /// First failure of a non-main thread.
    failure: Option<String>,
}

/// The baton and the wait machinery. One per guest process.
pub struct Sched {
    inner: Mutex<Inner>,
    cv: Condvar,
}

/// Identity of the guest thread running on this host thread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ctx {
    /// 0 for the main thread, 1.. for threads created with `pthread_create`.
    pub tid: u32,
    /// Guest `pthread_t` (0 = the main thread's, kept in the state).
    pub handle: u32,
    /// Address of this thread's `errno` cell (0 = the main thread's).
    pub errno: u32,
}

thread_local! {
    static CTX: Cell<Ctx> = const { Cell::new(Ctx { tid: 0, handle: 0, errno: 0 }) };
    /// Test hook: make the next [`spawn`] on this thread fail to create its host thread.
    static FAIL_SPAWN: Cell<bool> = const { Cell::new(false) };
}

/// Make the next [`spawn`] on this host thread fail at the OS level (for tests).
pub fn fail_next_spawn() {
    FAIL_SPAWN.with(|f| f.set(true));
}

/// The calling host thread's guest identity.
pub fn ctx() -> Ctx {
    CTX.with(Cell::get)
}

pub(crate) fn set_ctx(c: Ctx) {
    CTX.with(|x| x.set(c));
}

impl Sched {
    /// A scheduler whose baton is held by the creating (main) thread.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Sched {
        Sched {
            inner: Mutex::new(Inner {
                next_ticket: 1,
                serving: 0,
                epoch: 0,
                blocked: 0,
                live: 1,
                deadlock: false,
                shutdown: false,
                failure: None,
            }),
            cv: Condvar::new(),
        }
    }

    fn g(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn take_baton(&self, mut g: MutexGuard<'_, Inner>) {
        let t = g.next_ticket;
        g.next_ticket += 1;
        while g.serving != t {
            g = self.cv.wait(g).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Wait for the baton (a new thread's first action).
    pub(crate) fn acquire(&self) {
        self.take_baton(self.g());
    }

    /// Something changed that may unblock a waiter.
    pub fn notify(&self) {
        let mut g = self.g();
        g.epoch += 1;
        g.blocked = 0;
        self.cv.notify_all();
    }

    /// Let the other runnable threads go first.
    pub fn yield_now(&self) {
        let mut g = self.g();
        let t = g.next_ticket;
        g.next_ticket += 1;
        g.serving += 1;
        self.cv.notify_all();
        while g.serving != t {
            g = self.cv.wait(g).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Release the baton and sleep until [`Sched::notify`], the deadline, or a deadlock; then take the baton again.
    pub fn wait(&self, deadline: Option<Instant>) -> WaitEnd {
        let mut g = self.g();
        let epoch = g.epoch;
        g.serving += 1;
        if deadline.is_none() {
            g.blocked += 1;
            if g.blocked >= g.live {
                g.deadlock = true;
            }
        }
        self.cv.notify_all();
        let end = loop {
            if g.deadlock || g.shutdown {
                break WaitEnd::Deadlock;
            }
            if g.epoch != epoch {
                break WaitEnd::Woken;
            }
            match deadline {
                None => g = self.cv.wait(g).unwrap_or_else(PoisonError::into_inner),
                Some(d) => {
                    let now = Instant::now();
                    if now >= d {
                        break WaitEnd::TimedOut;
                    }
                    g = self.cv.wait_timeout(g, d - now).unwrap_or_else(PoisonError::into_inner).0;
                }
            }
        };
        if deadline.is_none() && g.epoch == epoch && g.blocked > 0 {
            g.blocked -= 1;
        }
        self.take_baton(g);
        end
    }

    /// A thread was created.
    pub(crate) fn thread_started(&self) {
        self.g().live += 1;
    }

    /// A thread that was announced with [`Sched::thread_started`] could not be created. The caller keeps the baton.
    pub(crate) fn thread_cancelled(&self) {
        let mut g = self.g();
        g.live -= 1;
        g.epoch += 1;
        g.blocked = 0;
        self.cv.notify_all();
    }

    /// The calling thread is done: give up the baton for good.
    pub(crate) fn thread_exit(&self, failure: Option<String>) {
        let mut g = self.g();
        g.live -= 1;
        if g.failure.is_none() {
            g.failure = failure;
        }
        g.epoch += 1;
        g.blocked = 0;
        g.serving += 1;
        self.cv.notify_all();
    }

    /// Wake everything with [`WaitEnd::Deadlock`]; no thread will block again.
    pub fn shutdown(&self) {
        let mut g = self.g();
        g.shutdown = true;
        self.cv.notify_all();
    }

    /// Number of live guest threads.
    pub fn live(&self) -> u32 {
        self.g().live
    }

    /// First failure recorded by a non-main thread, if any.
    pub fn failure(&self) -> Option<String> {
        self.g().failure.clone()
    }
}

/// Block the calling guest thread (see [`Sched::wait`]). On deadlock the guest is stopped and `None` returned.
pub fn block(c: &mut Cpu, st: &Shared, deadline: Option<Instant>, what: &str) -> Option<WaitEnd> {
    let sched = lock(st).sched.clone();
    match sched.wait(deadline) {
        WaitEnd::Deadlock => {
            let msg = sched.failure().map_or_else(
                || format!("deadlock: every guest thread is blocked (in {what})"),
                |f| format!("a guest thread failed: {f}"),
            );
            c.request_stop(Stop::Host(msg));
            None
        }
        end => Some(end),
    }
}

/// Sleep the calling guest thread for `ns` nanoseconds. Alone, this is the host clock's sleep; with other
/// threads alive the baton is handed over for the duration.
pub fn sleep(c: &mut Cpu, st: &Shared, ns: u64) {
    let sched = lock(st).sched.clone();
    if sched.live() <= 1 {
        lock(st).clock.sleep(ns);
        return;
    }
    let deadline = Instant::now() + std::time::Duration::from_nanos(ns);
    while Instant::now() < deadline {
        if block(c, st, Some(deadline), "sleep").is_none() {
            return;
        }
    }
}

/// Default length of a guest time slice, in guest cycles (about a millisecond of host time).
pub const DEFAULT_SLICE_CYCLES: u64 = 2_000_000;

/// Give the guest `cpu` (and the threads and nested calls derived from it) a time slice: every `cycles`
/// guest cycles it hands the baton to the other runnable guest threads, so a thread spinning on a flag
/// (`ldrex`/`strex` loops, spin locks) cannot starve the one that would set it, and checks the wall-clock
/// watchdog set with [`set_wall_deadline`]. Needs a cycle-counting CPU with a budget (`Cpu::set_ticks`).
pub fn enable_time_slice(cpu: &mut Cpu, st: &Shared, cycles: u64) {
    let st = st.clone();
    cpu.set_slice(cycles, Arc::new(move |_| slice_expired(&st)));
}

/// Stop the guest (every thread, at its next slice boundary) once the host clock passes `deadline`.
/// `None` removes the limit.
pub fn set_wall_deadline(st: &Shared, deadline: Option<Instant>) {
    lock(st).wall_deadline = deadline;
}

/// One time slice is used up: enforce the wall-clock limit, otherwise let the other threads run.
fn slice_expired(st: &Shared) -> Option<Stop> {
    let (sched, deadline) = {
        let s = lock(st);
        (s.sched.clone(), s.wall_deadline)
    };
    if deadline.is_some_and(|d| Instant::now() >= d) {
        // Wake the blocked threads too: nobody is going to make progress any more.
        sched.shutdown();
        return Some(Stop::Host("watchdog: the wall-clock limit for the guest was exceeded".into()));
    }
    sched.yield_now();
    None
}

/// Wake threads waiting in [`block`].
pub fn notify(st: &Shared) {
    let sched = lock(st).sched.clone();
    sched.notify();
}

/// The `pthread_t` of the calling guest thread.
pub fn self_handle(st: &Shared) -> u32 {
    match ctx().handle {
        0 => lock(st).main_thread,
        h => h,
    }
}

/// Address of the calling thread's `errno` cell.
pub fn errno_addr(st: &Shared) -> u32 {
    match ctx().errno {
        0 => lock(st).errno,
        e => e,
    }
}

/// Bookkeeping for a thread created by `pthread_create`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadRec {
    pub tid: u32,
    pub detached: bool,
    pub finished: bool,
    pub retval: u32,
}

/// Marker `Stop::Host` message of `pthread_exit`.
pub const EXIT_MARK: &str = "pthread_exit";

/// Create a guest thread running `start(arg)`; returns its `pthread_t` or an `errno`.
pub(crate) fn spawn(
    c: &mut Cpu,
    st: &Shared,
    start: u32,
    arg: u32,
    stack_size: u32,
    detached: bool,
) -> Result<u32, u32> {
    use crate::layout::errno;
    let (mem, sched) = {
        let s = lock(st);
        (s.mem.clone(), s.sched.clone())
    };
    let Ok(stack) = mem.map_anywhere(stack_size.max(16 * 1024), 4096) else { return Err(errno::EAGAIN) };
    let top = stack + stack_size.max(16 * 1024);
    let Ok(cpu) = c.new_thread() else { return Err(errno::EAGAIN) };
    let (handle, tid) = {
        let mut s = lock(st);
        let handle = s.heap.calloc(1, 256);
        if handle == 0 {
            return Err(errno::EAGAIN);
        }
        s.next_tid += 1;
        let tid = s.next_tid;
        s.threads.insert(handle, ThreadRec { tid, detached, finished: false, retval: 0 });
        (handle, tid)
    };
    sched.thread_started();
    let counting = c.cycle_counting();
    let injected = FAIL_SPAWN.with(|f| f.replace(false));
    let spawned = if injected {
        Err(std::io::Error::other("injected spawn failure"))
    } else {
        let parts = ThreadParts { st: st.clone(), sched, tid, handle, start, arg, top, counting };
        std::thread::Builder::new().name(format!("guest-{tid}")).spawn(move || thread_main(cpu, parts)).map(|_| ())
    };
    if spawned.is_err() {
        sched_undo(st, handle);
        return Err(errno::EAGAIN);
    }
    Ok(handle)
}

/// Everything a new guest thread needs besides its CPU.
struct ThreadParts {
    st: Shared,
    sched: Arc<Sched>,
    tid: u32,
    handle: u32,
    start: u32,
    arg: u32,
    top: u32,
    counting: bool,
}

/// Gives up the baton when a guest thread's host thread ends, however it ends.
struct ExitGuard {
    sched: Arc<Sched>,
    failure: Option<String>,
}

impl Drop for ExitGuard {
    fn drop(&mut self) {
        let panic_note = "host panic in a guest thread".to_string();
        let failure = self.failure.take().or(std::thread::panicking().then_some(panic_note));
        self.sched.thread_exit(failure);
    }
}

/// Body of a guest thread's host thread.
fn thread_main(mut cpu: Box<Cpu>, p: ThreadParts) {
    let ThreadParts { st, sched, tid, handle, start, arg, top, counting } = p;
    set_ctx(Ctx { tid, handle, errno: handle });
    sched.acquire();
    let mut guard = ExitGuard { sched, failure: None };
    cpu.set_sp(top);
    if counting {
        cpu.set_ticks(u64::MAX >> 2);
    }
    let mut ret = 0;
    match cpu.call(start, &[arg]) {
        Ok(r) => ret = r.r0,
        Err(Stop::Host(m)) if m == EXIT_MARK => ret = lock(&st).threads.get(&handle).map_or(0, |t| t.retval),
        Err(e) => guard.failure = Some(e.to_string()),
    }
    if guard.failure.is_none() {
        run_tls_destructors(&mut cpu, &st, tid);
    }
    let mut s = lock(&st);
    let t = s.threads.get_mut(&handle).expect("a thread's record lives until it is reaped");
    t.finished = true;
    t.retval = ret;
    if t.detached {
        s.threads.remove(&handle);
        s.heap.free(handle);
    }
    s.tls.retain(|(t, _), _| *t != tid);
    drop(s);
}

fn sched_undo(st: &Shared, handle: u32) {
    let mut s = lock(st);
    s.threads.remove(&handle);
    s.heap.free(handle);
    let sched = s.sched.clone();
    drop(s);
    sched.thread_cancelled();
}

/// Call the destructors of the thread's non-null TLS values once, as `pthread_exit` does.
fn run_tls_destructors(cpu: &mut Cpu, st: &Shared, tid: u32) {
    let pending: Vec<(u32, u32)> = {
        let s = lock(st);
        s.tls
            .iter()
            .filter(|((t, _), v)| *t == tid && **v != 0)
            .filter_map(|((_, k), v)| s.keys.get(k).filter(|e| e.1 != 0).map(|e| (e.1, *v)))
            .collect()
    };
    for (dtor, value) in pending {
        let _ = cpu.call(dtor, &[value]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{attr, errno};
    use crate::tests_env::{Env, SCRATCH};
    use std::sync::atomic::{AtomicU32, Ordering};

    const T: u32 = SCRATCH + 0x100; // pthread_t out
    const M: u32 = SCRATCH + 0x200; // mutex
    const CV: u32 = SCRATCH + 0x210; // condition variable
    const FLAG: u32 = SCRATCH + 0x300;
    const OUT: u32 = SCRATCH + 0x400;
    const ATTR: u32 = SCRATCH + 0x500;
    const TS: u32 = SCRATCH + 0x600;
    const BAD: u32 = 0x10;

    /// Register a guest "function" implemented in Rust (a thunk) and return its address.
    fn thunk(e: &Env, name: &str, f: impl Fn(&mut Cpu) + Send + Sync + 'static) -> u32 {
        e.cpu.register_shared_thunk(name, Arc::new(f)).unwrap()
    }

    fn ret_zero(c: &mut Cpu) {
        c.ret_u32(0);
    }

    /// Give other threads the baton until `*addr` becomes non-zero (they need real time to start).
    fn pump(e: &mut Env, addr: u32) {
        for _ in 0..5000 {
            if e.mem().read_u32(addr).unwrap() != 0 {
                break;
            }
            e.call("sched_yield", &[]);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(e.mem().read_u32(addr).unwrap() != 0, "no thread made progress");
    }

    fn create(e: &mut Env, attr_ptr: u32, start: u32, arg: u32) -> u32 {
        e.call("pthread_create", &[T, attr_ptr, start, arg]).r0
    }

    fn handle(e: &Env) -> u32 {
        e.mem().read_u32(T).unwrap()
    }

    #[test]
    fn create_join_returns_the_value_and_gives_each_thread_its_own_identity() {
        let mut e = Env::new();
        let st = e.st.clone();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let (tid_main, handle_main) = (ctx().tid, e.call("pthread_self", &[]).r0);
        let work = thunk(&e, "work", move |c| {
            let arg = c.args().u32(c);
            crate::misc::set_errno(c, &st, 77);
            seen2.lock().unwrap().push((ctx().tid, self_handle(&st), crate::misc::get_errno(c, &st), arg));
            c.ret_u32(arg + 1);
        });
        assert_eq!(create(&mut e, 0, work, 41), 0);
        let t = handle(&e);
        assert_ne!(t, handle_main);
        assert_eq!(e.call("pthread_join", &[t, OUT]).r0, 0);
        assert_eq!(e.mem().read_u32(OUT).unwrap(), 42);
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 1);
        assert_eq!((seen[0].0, seen[0].1, seen[0].2, seen[0].3), (1, t, 77, 41));
        assert_eq!(tid_main, 0);
        assert_eq!(e.errno(), 0, "errno is per thread");
        // The handle is gone after the join.
        assert_eq!(e.call("pthread_join", &[t, OUT]).r0, errno::ESRCH);
        assert_eq!(e.st.lock().unwrap().sched.live(), 1);
        assert_eq!(e.st.lock().unwrap().next_tid_value(), 1);
        // Joining without collecting the value.
        assert_eq!(create(&mut e, 0, work, 1), 0);
        let t = handle(&e);
        assert_eq!(e.call("pthread_join", &[t, 0]).r0, 0);
    }

    #[test]
    fn create_validates_its_arguments_and_reports_resource_failures() {
        let mut e = Env::new();
        let nop = thunk(&e, "nop", ret_zero);
        assert_eq!(create(&mut e, 0, 0, 0), errno::EINVAL);
        assert!(matches!(e.try_call("pthread_create", &[BAD, 0, nop, 0]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("pthread_create", &[T, BAD, nop, 0]), Err(Stop::Fault(_))));
        // The thread created before the unwritable `pthread_t` faulted still runs to its end; let it finish.
        for _ in 0..5000 {
            if e.st.lock().unwrap().sched.live() == 1 {
                break;
            }
            e.call("sched_yield", &[]);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        // A stack that cannot be mapped.
        e.call("pthread_attr_init", &[ATTR]);
        e.mem().write_u32(ATTR + attr::STACK_SIZE, 0xF000_0000).unwrap();
        assert_eq!(create(&mut e, ATTR, nop, 0), errno::EAGAIN);
        // The host refuses to create a thread.
        fail_next_spawn();
        assert_eq!(create(&mut e, 0, nop, 0), errno::EAGAIN);
        assert_eq!(e.st.lock().unwrap().sched.live(), 1, "a failed spawn leaves no phantom thread");
        // No guest heap for the thread block.
        let mut s = Env::small();
        let snop = thunk(&s, "nop", ret_zero);
        while s.call("malloc", &[16]).r0 != 0 {}
        assert_eq!(s.call("pthread_create", &[T, 0, snop, 0]).r0, errno::EAGAIN);
        // Out of processors: every thread blocked on a condition nobody signals.
        let mut e = Env::new();
        let park = thunk(&e, "park", move |c| {
            let (lock_f, wait_f, unlock_f) = (
                c.thunk_addr("pthread_mutex_lock").unwrap(),
                c.thunk_addr("pthread_cond_wait").unwrap(),
                c.thunk_addr("pthread_mutex_unlock").unwrap(),
            );
            c.call(lock_f, &[M]).unwrap();
            while c.mem().read_u32(FLAG).unwrap() == 0 {
                let n = c.mem().read_u32(FLAG + 4).unwrap();
                c.mem().write_u32(FLAG + 4, n + 1).unwrap();
                let _ = c.call(wait_f, &[CV, M]);
            }
            c.call(unlock_f, &[M]).unwrap();
            c.ret_u32(0);
        });
        for f in ["pthread_mutex_lock", "pthread_cond_wait", "pthread_mutex_unlock"] {
            e.addr(f);
        }
        let mut made = Vec::new();
        let mut last = 0;
        for _ in 0..80 {
            last = create(&mut e, 0, park, 0);
            if last != 0 {
                break;
            }
            made.push(handle(&e));
        }
        assert_eq!(last, errno::EAGAIN, "the exclusive monitor has {} processors", bhpc_cpu::MAX_CPUS);
        assert!(made.len() >= 8);
        // Let them all reach the wait first, then finish them.
        for _ in 0..5000 {
            if e.mem().read_u32(FLAG + 4).unwrap() as usize >= made.len() {
                break;
            }
            e.call("sched_yield", &[]);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        e.mem().write_u32(FLAG, 1).unwrap();
        e.call("pthread_cond_broadcast", &[CV]);
        e.call("sched_yield", &[]);
        for t in made {
            // Each parked thread holds the mutex until it returns; hand it over in turn.
            assert_eq!(e.call("pthread_join", &[t, 0]).r0, 0);
        }
    }

    #[test]
    fn detach_and_join_error_codes() {
        let mut e = Env::new();
        let nop = thunk(&e, "nop", |c| {
            c.mem().write_u32(FLAG, 1).unwrap();
            c.ret_u32(5)
        });
        let me = e.call("pthread_self", &[]).r0;
        assert_eq!(e.call("pthread_join", &[me, 0]).r0, errno::EDEADLK);
        assert_eq!(e.call("pthread_join", &[0x1234, 0]).r0, errno::ESRCH);
        assert_eq!(e.call("pthread_detach", &[0x1234]).r0, errno::ESRCH);
        // A detached thread cannot be joined or detached again; it is reaped when it finishes.
        assert_eq!(create(&mut e, 0, nop, 0), 0);
        let t = handle(&e);
        assert_eq!(e.call("pthread_detach", &[t]).r0, 0);
        assert_eq!(e.call("pthread_detach", &[t]).r0, errno::EINVAL);
        assert_eq!(e.call("pthread_join", &[t, 0]).r0, errno::EINVAL);
        pump(&mut e, FLAG);
        assert_eq!(e.call("pthread_join", &[t, 0]).r0, errno::ESRCH, "reaped");
        e.mem().write_u32(FLAG, 0).unwrap();
        // Detached from the start through the attribute.
        e.call("pthread_attr_init", &[ATTR]);
        e.call("pthread_attr_setdetachstate", &[ATTR, 1]);
        assert_eq!(create(&mut e, ATTR, nop, 0), 0);
        let t = handle(&e);
        pump(&mut e, FLAG);
        assert_eq!(e.call("pthread_join", &[t, 0]).r0, errno::ESRCH);
        e.mem().write_u32(FLAG, 0).unwrap();
        // Detaching a thread that already finished reaps it at once.
        assert_eq!(create(&mut e, 0, nop, 0), 0);
        let t = handle(&e);
        pump(&mut e, FLAG);
        assert_eq!(e.call("pthread_detach", &[t]).r0, 0);
        assert_eq!(e.call("pthread_join", &[t, 0]).r0, errno::ESRCH);
        // join with a bad result pointer faults.
        assert_eq!(create(&mut e, 0, nop, 0), 0);
        let t = handle(&e);
        assert!(matches!(e.try_call("pthread_join", &[t, BAD]), Err(Stop::Fault(_))));
    }

    #[test]
    fn a_contended_mutex_hands_over_when_the_holder_unlocks() {
        let mut e = Env::new();
        let (lock_f, unlock_f) = (e.addr("pthread_mutex_lock"), e.addr("pthread_mutex_unlock"));
        let w = thunk(&e, "locker", move |c| {
            c.mem().write_u32(FLAG + 0x10, 1).unwrap();
            c.call(lock_f, &[M]).unwrap();
            let n = c.mem().read_u32(FLAG).unwrap();
            c.mem().write_u32(FLAG, n + 1).unwrap();
            c.call(unlock_f, &[M]).unwrap();
        });
        assert_eq!(e.call("pthread_mutex_lock", &[M]).r0, 0);
        assert_eq!(create(&mut e, 0, w, 0), 0);
        let t = handle(&e);
        pump(&mut e, FLAG + 0x10);
        assert_eq!(e.mem().read_u32(FLAG).unwrap(), 0, "the worker is blocked on the mutex");
        assert_eq!(e.call("pthread_mutex_trylock", &[M]).r0, errno::EBUSY);
        assert_eq!(e.call("pthread_mutex_unlock", &[M]).r0, 0);
        assert_eq!(e.call("pthread_join", &[t, 0]).r0, 0);
        assert_eq!(e.mem().read_u32(FLAG).unwrap(), 1);
        // A recursive mutex held by another thread is busy and cannot be unlocked by us.
        let rec = SCRATCH + 0x240;
        e.mem().write_u32(rec, 0x4000).unwrap();
        let hold = thunk(&e, "hold", move |c| {
            c.call(lock_f, &[rec]).unwrap();
            c.mem().write_u32(FLAG + 0x20, 1).unwrap();
            c.call(c.thunk_addr("sched_yield").unwrap(), &[]).unwrap();
            c.call(c.thunk_addr("sched_yield").unwrap(), &[]).unwrap();
        });
        e.addr("sched_yield");
        assert_eq!(create(&mut e, 0, hold, 0), 0);
        let t = handle(&e);
        pump(&mut e, FLAG + 0x20);
        assert_eq!(e.call("pthread_mutex_trylock", &[rec]).r0, errno::EBUSY);
        assert_eq!(e.call("pthread_mutex_unlock", &[rec]).r0, errno::EPERM);
        assert_eq!(e.call("pthread_join", &[t, 0]).r0, 0);
    }

    #[test]
    fn condition_variables_wake_signalled_and_broadcast_waiters() {
        let mut e = Env::new();
        let (lock_f, unlock_f, wait_f) =
            (e.addr("pthread_mutex_lock"), e.addr("pthread_mutex_unlock"), e.addr("pthread_cond_wait"));
        // Waiters count themselves in, wait for FLAG, then count themselves out.
        let waiter = thunk(&e, "waiter", move |c| {
            c.call(lock_f, &[M]).unwrap();
            let n = c.mem().read_u32(FLAG + 4).unwrap();
            c.mem().write_u32(FLAG + 4, n + 1).unwrap();
            while c.mem().read_u32(FLAG).unwrap() == 0 {
                c.call(wait_f, &[CV, M]).unwrap();
            }
            let n = c.mem().read_u32(FLAG + 8).unwrap();
            c.mem().write_u32(FLAG + 8, n + 1).unwrap();
            c.call(unlock_f, &[M]).unwrap();
        });
        let mut ts = Vec::new();
        for _ in 0..3 {
            assert_eq!(create(&mut e, 0, waiter, 0), 0);
            ts.push(handle(&e));
        }
        // Let all three reach the wait.
        for _ in 0..5000 {
            if e.mem().read_u32(FLAG + 4).unwrap() == 3 {
                break;
            }
            e.call("sched_yield", &[]);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(e.mem().read_u32(FLAG + 4).unwrap(), 3);
        // signal wakes one (after the predicate flips it passes the lock on).
        e.call("pthread_mutex_lock", &[M]);
        e.mem().write_u32(FLAG, 1).unwrap();
        e.call("pthread_cond_signal", &[CV]);
        e.call("pthread_mutex_unlock", &[M]);
        pump(&mut e, FLAG + 8);
        assert_eq!(e.mem().read_u32(FLAG + 8).unwrap(), 1, "exactly one waiter was woken");
        e.call("pthread_cond_broadcast", &[CV]);
        for t in ts {
            assert_eq!(e.call("pthread_join", &[t, 0]).r0, 0);
        }
        assert_eq!(e.mem().read_u32(FLAG + 8).unwrap(), 3);
        // Signalling with no waiters does nothing.
        e.call("pthread_cond_signal", &[CV + 0x40]);
        e.call("pthread_cond_broadcast", &[CV + 0x40]);
    }

    #[test]
    fn timed_waits_time_out_or_are_signalled() {
        let mut e = Env::new();
        let mem = e.mem();
        e.call("pthread_mutex_lock", &[M]);
        // relative
        mem.write_u32(TS, 0).unwrap();
        mem.write_u32(TS + 4, 3_000_000).unwrap();
        let t0 = Instant::now();
        assert_eq!(e.call("pthread_cond_timedwait_relative_np", &[CV, M, TS]).r0, errno::ETIMEDOUT);
        assert!(t0.elapsed() >= std::time::Duration::from_millis(2));
        // The mutex is held again after the timeout.
        assert_eq!(e.call("pthread_mutex_trylock", &[M]).r0, errno::EBUSY);
        // absolute, realtime (fake clock at 1_600_000_000 s) and monotonic (5000 s)
        mem.write_u32(TS, 1_600_000_000).unwrap();
        mem.write_u32(TS + 4, 3_000_000).unwrap();
        assert_eq!(e.call("pthread_cond_timedwait", &[CV, M, TS]).r0, errno::ETIMEDOUT);
        mem.write_u32(TS, 5000).unwrap();
        assert_eq!(e.call("pthread_cond_timedwait_monotonic", &[CV, M, TS]).r0, errno::ETIMEDOUT);
        // Deadline already in the past.
        mem.write_u32(TS, 1).unwrap();
        assert_eq!(e.call("pthread_cond_timedwait", &[CV, M, TS]).r0, errno::ETIMEDOUT);
        // Invalid or unreadable timespec.
        mem.write_u32(TS + 4, 1_000_000_000).unwrap();
        assert_eq!(e.call("pthread_cond_timedwait", &[CV, M, TS]).r0, errno::EINVAL);
        assert!(matches!(e.try_call("pthread_cond_timedwait", &[CV, M, BAD]), Err(Stop::Fault(_))));
        // A signal arriving before the deadline wins.
        let (signal_f, lock_f, unlock_f) =
            (e.addr("pthread_cond_signal"), e.addr("pthread_mutex_lock"), e.addr("pthread_mutex_unlock"));
        let signaller = thunk(&e, "signaller", move |c| {
            c.call(lock_f, &[M]).unwrap();
            c.call(signal_f, &[CV]).unwrap();
            c.call(unlock_f, &[M]).unwrap();
        });
        assert_eq!(create(&mut e, 0, signaller, 0), 0);
        let t = handle(&e);
        mem.write_u32(TS, 5).unwrap();
        mem.write_u32(TS + 4, 0).unwrap();
        let t0 = Instant::now();
        assert_eq!(e.call("pthread_cond_timedwait_relative_np", &[CV, M, TS]).r0, 0);
        assert!(t0.elapsed() < std::time::Duration::from_secs(4));
        e.call("pthread_mutex_unlock", &[M]);
        assert_eq!(e.call("pthread_join", &[t, 0]).r0, 0);
        // A wait on a mutex that is not held by anyone is tolerated.
        mem.write_u32(TS, 0).unwrap();
        mem.write_u32(TS + 4, 2_000_000).unwrap();
        assert_eq!(e.call("pthread_cond_timedwait_relative_np", &[CV, M + 0x20, TS]).r0, errno::ETIMEDOUT);
    }

    #[test]
    fn deadlocks_stop_the_guest_instead_of_hanging() {
        let mut e = Env::new();
        let wait_f = e.addr("pthread_cond_wait");
        let lock_f = e.addr("pthread_mutex_lock");
        let stuck = thunk(&e, "stuck", move |c| {
            c.call(lock_f, &[M]).unwrap();
            let _ = c.call(wait_f, &[CV, M]);
        });
        assert_eq!(create(&mut e, 0, stuck, 0), 0);
        let t = handle(&e);
        let r = e.try_call("pthread_join", &[t, 0]);
        assert!(matches!(r, Err(Stop::Host(ref m)) if m.contains("deadlock") && m.contains("pthread_join")), "{r:?}");
        // A lone thread locking a held normal mutex deadlocks too.
        let mut e = Env::new();
        e.call("pthread_mutex_lock", &[M]);
        assert!(matches!(e.try_call("pthread_mutex_lock", &[M]), Err(Stop::Host(m)) if m.contains("deadlock")));
        // rwlocks and condition waits as well.
        let l = SCRATCH + 0x700;
        e.call("pthread_rwlock_wrlock", &[l]);
        assert!(e.try_call("pthread_rwlock_rdlock", &[l]).is_err());
        assert!(e.try_call("pthread_rwlock_wrlock", &[l]).is_err());
        assert!(e.try_call("pthread_cond_wait", &[CV, M]).is_err());
    }

    #[test]
    fn a_failing_thread_is_reported_to_the_joiner() {
        let mut e = Env::new();
        let boom = thunk(&e, "boom", |c| c.request_stop(Stop::Host("worker died".into())));
        assert_eq!(create(&mut e, 0, boom, 0), 0);
        let t = handle(&e);
        assert_eq!(e.call("pthread_join", &[t, 0]).r0, 0, "join still collects the thread");
        let sched = e.st.lock().unwrap().sched.clone();
        assert!(sched.failure().unwrap().contains("worker died"));
        // Later blocking operations name the failure.
        e.call("pthread_mutex_lock", &[M]);
        let r = e.try_call("pthread_mutex_lock", &[M]);
        assert!(
            matches!(r, Err(Stop::Host(ref m)) if m.contains("a guest thread failed") && m.contains("worker died")),
            "{r:?}"
        );
    }

    #[test]
    fn pthread_exit_ends_the_thread_with_a_value() {
        let mut e = Env::new();
        let exit_f = e.addr("pthread_exit");
        let quitter = thunk(&e, "quitter", move |c| {
            // pthread_exit stops this thread: the call never returns normally.
            assert!(c.call_or_stop(exit_f, &[99]).is_none());
        });
        assert_eq!(create(&mut e, 0, quitter, 0), 0);
        let t = handle(&e);
        assert_eq!(e.call("pthread_join", &[t, OUT]).r0, 0);
        assert_eq!(e.mem().read_u32(OUT).unwrap(), 99);
        // On the main thread it stops the whole guest.
        assert!(matches!(e.try_call("pthread_exit", &[1]), Err(Stop::Host(m)) if m.contains("main thread")));
    }

    #[test]
    fn thread_specific_data_is_per_thread_and_destructors_run_at_exit() {
        let mut e = Env::new();
        let dtor_log = Arc::new(Mutex::new(Vec::new()));
        let log2 = dtor_log.clone();
        let dtor = thunk(&e, "dtor", move |c| {
            let v = c.args().u32(c);
            log2.lock().unwrap().push(v);
        });
        e.call("pthread_key_create", &[OUT, dtor]);
        let key = e.mem().read_u32(OUT).unwrap();
        e.call("pthread_setspecific", &[key, 7]);
        let (get_f, set_f) = (e.addr("pthread_getspecific"), e.addr("pthread_setspecific"));
        let seen = Arc::new(AtomicU32::new(99));
        let seen2 = seen.clone();
        let worker = thunk(&e, "tls_worker", move |c| {
            seen2.store(c.call(get_f, &[key]).unwrap().r0, Ordering::SeqCst);
            c.call(set_f, &[key, 123]).unwrap();
        });
        assert_eq!(create(&mut e, 0, worker, 0), 0);
        let t = handle(&e);
        assert_eq!(e.call("pthread_join", &[t, 0]).r0, 0);
        assert_eq!(seen.load(Ordering::SeqCst), 0, "a new thread starts with no value");
        assert_eq!(*dtor_log.lock().unwrap(), [123], "the destructor ran for the thread's value");
        assert_eq!(e.call("pthread_getspecific", &[key]).r0, 7, "the main thread's value is untouched");
        // Deleting the key drops every thread's value.
        e.call("pthread_key_delete", &[key]);
        assert_eq!(e.call("pthread_getspecific", &[key]).r0, 0);
    }

    #[test]
    fn sleeping_and_yielding_let_other_threads_run() {
        let mut e = Env::new();
        let bump = thunk(&e, "bump", |c| {
            let n = c.mem().read_u32(FLAG).unwrap();
            c.mem().write_u32(FLAG, n + 1).unwrap();
        });
        assert_eq!(create(&mut e, 0, bump, 0), 0);
        let t = handle(&e);
        // nanosleep with a live second thread hands the baton over and really waits.
        e.mem().write_u32(TS, 0).unwrap();
        e.mem().write_u32(TS + 4, 2_000_000).unwrap();
        let t0 = Instant::now();
        assert_eq!(e.call("nanosleep", &[TS, 0]).r0, 0);
        assert!(t0.elapsed() >= std::time::Duration::from_millis(2));
        assert_eq!(e.mem().read_u32(FLAG).unwrap(), 1);
        e.call("pthread_join", &[t, 0]);
        // A thread sleeping while main joins it.
        let (usleep_f, sleep_f) = (e.addr("usleep"), e.addr("sleep"));
        let sleeper = thunk(&e, "sleeper", move |c| {
            c.call(usleep_f, &[1000]).unwrap();
            c.call(sleep_f, &[0]).unwrap();
        });
        assert_eq!(create(&mut e, 0, sleeper, 0), 0);
        let t = handle(&e);
        assert_eq!(e.call("pthread_join", &[t, 0]).r0, 0);
    }

    #[test]
    fn readers_wait_for_a_writer() {
        let mut e = Env::new();
        let l = SCRATCH + 0x700;
        let (rd_f, un_f) = (e.addr("pthread_rwlock_rdlock"), e.addr("pthread_rwlock_unlock"));
        let reader = thunk(&e, "reader", move |c| {
            c.mem().write_u32(FLAG + 0x10, 1).unwrap();
            c.call(rd_f, &[l]).unwrap();
            c.mem().write_u32(FLAG, 1).unwrap();
            c.call(un_f, &[l]).unwrap();
        });
        e.call("pthread_rwlock_wrlock", &[l]);
        assert_eq!(create(&mut e, 0, reader, 0), 0);
        let t = handle(&e);
        pump(&mut e, FLAG + 0x10);
        assert_eq!(e.mem().read_u32(FLAG).unwrap(), 0);
        e.call("pthread_rwlock_unlock", &[l]);
        // A writer waits for the reader.
        e.call("pthread_join", &[t, 0]);
        assert_eq!(e.mem().read_u32(FLAG).unwrap(), 1);
        e.call("pthread_rwlock_rdlock", &[l]);
        let wr_f = e.addr("pthread_rwlock_wrlock");
        let writer = thunk(&e, "writer", move |c| {
            c.mem().write_u32(FLAG + 0x20, 1).unwrap();
            c.call(wr_f, &[l]).unwrap();
            c.mem().write_u32(FLAG, 2).unwrap();
            c.call(un_f, &[l]).unwrap();
        });
        assert_eq!(create(&mut e, 0, writer, 0), 0);
        let t = handle(&e);
        pump(&mut e, FLAG + 0x20);
        assert_eq!(e.mem().read_u32(FLAG).unwrap(), 1);
        e.call("pthread_rwlock_unlock", &[l]);
        e.call("pthread_join", &[t, 0]);
        assert_eq!(e.mem().read_u32(FLAG).unwrap(), 2);
    }

    #[test]
    fn scheduler_primitives() {
        let s = Sched::new();
        assert_eq!(s.live(), 1);
        assert_eq!(s.failure(), None);
        // A timed wait with nobody around times out and keeps the baton.
        let t0 = Instant::now();
        assert_eq!(s.wait(Some(Instant::now() + std::time::Duration::from_millis(2))), WaitEnd::TimedOut);
        assert!(t0.elapsed() >= std::time::Duration::from_millis(2));
        // An untimed wait alone is a deadlock; so is every later one.
        assert_eq!(s.wait(None), WaitEnd::Deadlock);
        assert_eq!(s.wait(Some(Instant::now() + std::time::Duration::from_secs(60))), WaitEnd::Deadlock);
        let s = Sched::new();
        s.yield_now();
        s.notify();
        s.shutdown();
        assert_eq!(s.wait(None), WaitEnd::Deadlock);
        // Thread bookkeeping and failure recording (first failure wins).
        let s = Sched::new();
        s.thread_started();
        assert_eq!(s.live(), 2);
        s.thread_exit(Some("first".into()));
        s.thread_started();
        s.thread_exit(Some("second".into()));
        assert_eq!((s.live(), s.failure().as_deref()), (1, Some("first")));
        // Identity defaults to the main thread.
        assert_eq!(ctx(), Ctx::default());
        set_ctx(Ctx { tid: 5, handle: 9, errno: 3 });
        assert_eq!(ctx().tid, 5);
        set_ctx(Ctx::default());
        assert_eq!(WaitEnd::Woken, WaitEnd::Woken);
    }

    #[test]
    fn threads_on_a_cycle_counting_cpu_get_their_own_budget() {
        let mut e = Env::counting();
        let nop = thunk(&e, "nop", |c| c.ret_u32(3));
        assert_eq!(create(&mut e, 0, nop, 0), 0);
        let t = handle(&e);
        assert_eq!(e.call("pthread_join", &[t, OUT]).r0, 0);
        assert_eq!(e.mem().read_u32(OUT).unwrap(), 3);
    }

    #[test]
    fn sleeping_after_a_shutdown_stops_the_guest() {
        let mut e = Env::new();
        let sched = e.st.lock().unwrap().sched.clone();
        sched.thread_started(); // pretend a second thread exists so sleep takes the scheduler path
        sched.shutdown();
        e.mem().write_u32(TS, 1).unwrap();
        e.mem().write_u32(TS + 4, 0).unwrap();
        assert!(matches!(e.try_call("nanosleep", &[TS, 0]), Err(Stop::Host(m)) if m.contains("sleep")));
    }

    #[test]
    fn pthread_once_waiters_block_until_the_initialiser_returns() {
        let mut e = Env::new();
        let once_f = e.addr("pthread_once");
        let st = e.st.clone();
        let (once, runs, seen) = (SCRATCH + 0x900, SCRATCH + 0x910, SCRATCH + 0x920);
        let init = thunk(&e, "slow_init", move |c| {
            let n = c.mem().read_u32(runs).unwrap();
            // Hand the baton to the other thread while the initialiser is "running".
            sleep(c, &st, 30_000_000);
            c.mem().write_u32(runs, n + 1).unwrap();
            c.ret_u32(0);
        });
        let worker = thunk(&e, "once_worker", move |c| {
            let r = c.call(once_f, &[once, init]).unwrap();
            let runs_seen = c.mem().read_u32(runs).unwrap();
            c.mem().write_u32(seen, runs_seen + 100 * r.r0).unwrap();
            c.ret_u32(0);
        });
        assert_eq!(create(&mut e, 0, worker, 0), 0);
        let t = handle(&e);
        assert_eq!(e.call("pthread_once", &[once, init]).r0, 0);
        assert_eq!(e.call("pthread_join", &[t, 0]).r0, 0);
        assert_eq!(e.mem().read_u32(runs).unwrap(), 1, "the initialiser ran exactly once");
        assert_eq!(e.mem().read_u32(seen).unwrap(), 1, "the waiter returned only after the initialiser finished");
        assert_eq!(e.mem().read_u32(once).unwrap(), crate::layout::sync::ONCE_DONE);
        assert!(lock(&e.st).once_owner.is_empty());
    }

    #[test]
    fn pthread_once_detects_reentry_and_a_stuck_initialiser() {
        let mut e = Env::new();
        let once_f = e.addr("pthread_once");
        let (once, out) = (SCRATCH + 0x900, SCRATCH + 0x910);
        let init = e.cpu.register_shared_thunk(
            "reenter",
            Arc::new(move |c: &mut Cpu| {
                let me = c.thunk_addr("reenter").unwrap();
                let r = c.call(once_f, &[once, me]).unwrap();
                c.mem().write_u32(out, r.r0).unwrap();
                c.ret_u32(0);
            }),
        );
        let init = init.unwrap();
        assert_eq!(e.call("pthread_once", &[once, init]).r0, 0);
        assert_eq!(e.mem().read_u32(out).unwrap(), errno::EDEADLK);
        // Another (dead) thread owns the in-progress initialiser and nobody can finish it.
        let stuck = SCRATCH + 0x930;
        e.mem().write_u32(stuck, crate::layout::sync::ONCE_IN_PROGRESS).unwrap();
        lock(&e.st).once_owner.insert(stuck, 99);
        let r = e.try_call("pthread_once", &[stuck, init]);
        assert!(matches!(r, Err(Stop::Host(ref m)) if m.contains("pthread_once")), "{r:?}");
    }

    #[test]
    fn futex_wait_and_wake() {
        const NEG_EAGAIN: u32 = -(errno::EAGAIN as i32) as u32;
        const NEG_TIMEDOUT: u32 = -(errno::ETIMEDOUT as i32) as u32;
        let mut e = Env::new();
        let (w, ts) = (SCRATCH + 0xA00, SCRATCH + 0xA10);
        let wait_f = e.addr("__futex_wait");
        let st = e.st.clone();
        // The word differs from the expected value.
        e.mem().write_u32(w, 1).unwrap();
        assert_eq!(e.call("__futex_wait", &[w, 0, 0]).r0, NEG_EAGAIN);
        // Waking without waiters wakes nobody.
        assert_eq!(e.call("__futex_wake", &[w, 1]).r0, 0);
        // A timed wait that nobody wakes times out; a bad timeout is rejected.
        e.mem().write_u32(ts, 0).unwrap();
        e.mem().write_u32(ts + 4, 2_000_000).unwrap();
        assert_eq!(e.call("__futex_wait", &[w, 1, ts]).r0, NEG_TIMEDOUT);
        e.mem().write_u32(ts + 4, 1_000_000_000).unwrap();
        assert_eq!(e.call("__futex_wait", &[w, 1, ts]).r0 as i32, -(errno::EINVAL as i32));
        // Faults.
        assert!(matches!(e.try_call("__futex_wait", &[BAD, 0, 0]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("__futex_wait", &[w, 1, BAD]), Err(Stop::Fault(_))));
        // Two waiters in other threads; wake one, then the other.
        let out = SCRATCH + 0xA20;
        let waiter = thunk(&e, "futex_waiter", move |c| {
            let idx = c.args().u32(c);
            let r = c.call(wait_f, &[w, 1, 0]).unwrap();
            c.mem().write_u32(out + 4 * idx, r.r0 + 10).unwrap();
            c.ret_u32(0);
        });
        let mut ts_ = Vec::new();
        for i in 0..2 {
            assert_eq!(create(&mut e, 0, waiter, i), 0);
            ts_.push(handle(&e));
        }
        // Let both threads reach the wait.
        for _ in 0..5000 {
            if lock(&st).futex_q.get(&w).map_or(0, |q| q.len()) == 2 {
                break;
            }
            e.call("sched_yield", &[]);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(e.call("__futex_wake", &[w, 1]).r0, 1);
        pump(&mut e, out);
        assert_eq!(e.call("__futex_wake", &[w, 100]).r0, 1);
        for t in ts_ {
            assert_eq!(e.call("pthread_join", &[t, 0]).r0, 0);
        }
        assert_eq!(e.mem().read_u32(out).unwrap(), 10);
        assert_eq!(e.mem().read_u32(out + 4).unwrap(), 10);
        // A lone thread waiting forever is a deadlock.
        assert!(matches!(e.try_call("__futex_wait", &[w, 1, 0]), Err(Stop::Host(m)) if m.contains("__futex_wait")));
    }

    /// `ldr r1,[r0]; cmp r1,#0; beq .-8; bx lr`: spin until the word at `r0` becomes non-zero.
    const SPIN: [u32; 4] = [0xE590_1000, 0xE351_0000, 0x0AFF_FFFC, 0xE12F_FF1E];
    /// The same wait with exclusive loads: `ldrex r1,[r0]; cmp r1,#0; beq .-8; bx lr`.
    const SPIN_EX: [u32; 4] = [0xE190_1F9F, 0xE351_0000, 0x0AFF_FFFC, 0xE12F_FF1E];

    #[test]
    fn time_slices_let_the_flag_setter_run_while_another_thread_spins() {
        for code in [SPIN, SPIN_EX] {
            let mut e = Env::counting();
            enable_time_slice(&mut e.cpu, &e.st, 20_000);
            let spin = SCRATCH + 0x1000;
            for (i, w) in code.iter().enumerate() {
                e.mem().write_u32(spin + 4 * i as u32, *w).unwrap();
            }
            let flag = SCRATCH + 0xB00;
            let setter = thunk(&e, "setter", move |c| {
                c.mem().write_u32(flag, 1).unwrap();
                c.ret_u32(0);
            });
            assert_eq!(create(&mut e, 0, setter, 0), 0);
            let t = handle(&e);
            // Without preemption this call would spin forever: the setter never gets the baton.
            e.cpu.set_ticks(1 << 40);
            assert_eq!(e.try_call_addr(spin, &[flag]).map(|r| r.r0), Ok(flag));
            e.call("pthread_join", &[t, 0]);
        }
    }

    #[test]
    fn the_wall_clock_watchdog_stops_a_spinning_guest() {
        let mut e = Env::counting();
        enable_time_slice(&mut e.cpu, &e.st, 20_000);
        let spin = SCRATCH + 0x1000;
        for (i, w) in SPIN.iter().enumerate() {
            e.mem().write_u32(spin + 4 * i as u32, *w).unwrap();
        }
        e.cpu.set_ticks(1 << 40);
        set_wall_deadline(&e.st, Some(Instant::now() + std::time::Duration::from_millis(30)));
        let r = e.try_call_addr(spin, &[SCRATCH + 0xB10]);
        assert!(matches!(r, Err(Stop::Host(ref m)) if m.contains("watchdog")), "{r:?}");
        // Removing the limit lets a later slice pass again.
        set_wall_deadline(&e.st, None);
        e.mem().write_u32(SCRATCH + 0xB10, 1).unwrap();
        assert_eq!(e.try_call_addr(spin, &[SCRATCH + 0xB10]).map(|r| r.r0), Ok(SCRATCH + 0xB10));
    }
}

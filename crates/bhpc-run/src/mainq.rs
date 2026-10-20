//! Work for the guest main thread. The game's touch, key and text natives must run on the thread that owns the GL
//! context (the Apportable main thread, `t0`), but the Android UI guest thread receives the events: it posts them
//! here, the main thread drains the queue right before every `eglSwapBuffers` (`Gles::set_frame_hook`) and the UI
//! thread waits for the answer. While the main thread presents no frames (start-up, before the game has a GL view) a
//! waiting poster *kicks* it ([`MainQueue::set_kick`]: play mode asks libdispatch to drain the queue from the main run
//! loop), so even the answer to the start-up alert runs there. That answer creates the game's `GameView`; on the UI
//! thread it raced the main thread's `initOpenGL` once guest threads ran in parallel, and a projection computed from a
//! zero view size left the world black for the whole session.

use bhpc_cpu::Cpu;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

type Job = Box<dyn FnOnce(&mut Cpu) + Send>;
type Wait = Box<dyn Fn(&mut Cpu) + Send + Sync>;

/// Queue of jobs for the main thread plus the way a poster waits (a scheduler sleep in play mode).
pub struct MainQueue {
    jobs: Mutex<VecDeque<Job>>,
    wait: Wait,
    max_waits: u32,
    /// After how many waits a poster runs the kick, and the kick (see [`MainQueue::set_kick`]).
    kick: OnceLock<(u32, Wait)>,
}

impl MainQueue {
    /// `wait` yields for a moment (it must let other guest threads run); after `max_waits` rounds without an
    /// answer a posted job that the main thread has not started runs on the posting thread instead.
    pub fn new(wait: impl Fn(&mut Cpu) + Send + Sync + 'static, max_waits: u32) -> Arc<MainQueue> {
        Arc::new(MainQueue {
            jobs: Mutex::new(VecDeque::new()),
            wait: Box::new(wait),
            max_waits,
            kick: OnceLock::new(),
        })
    }

    /// A poster whose job is still queued after `after` waits calls `kick` once (on its own thread): it makes the main
    /// thread drain the queue when no frame is coming. Only the first kick set counts.
    pub fn set_kick(&self, after: u32, kick: impl Fn(&mut Cpu) + Send + Sync + 'static) {
        let _ = self.kick.set((after, Box::new(kick)));
    }

    /// Run every queued job (main thread).
    pub fn drain(&self, c: &mut Cpu) {
        loop {
            let job = self.jobs.lock().unwrap_or_else(PoisonError::into_inner).pop_front();
            match job {
                Some(j) => j(c),
                None => break,
            }
        }
    }

    /// Run `f` on the main thread and return its result.
    pub fn call<R: Send + 'static>(&self, c: &mut Cpu, f: impl FnOnce(&mut Cpu) -> R + Send + 'static) -> R {
        let slot = Arc::new(Mutex::new(Some(f)));
        let out = Arc::new(Mutex::new(None));
        let (s2, o2) = (slot.clone(), out.clone());
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner).push_back(Box::new(move |c| {
            let f = s2.lock().unwrap_or_else(PoisonError::into_inner).take();
            if let Some(f) = f {
                let r = f(c);
                *o2.lock().unwrap_or_else(PoisonError::into_inner) = Some(r);
            }
        }));
        let result = |out: &Mutex<Option<R>>| out.lock().unwrap_or_else(PoisonError::into_inner).take();
        let mut waits = 0;
        loop {
            if let Some(r) = result(&out) {
                return r;
            }
            if waits >= self.max_waits {
                // Nobody drains (no frames): run it here, unless the main thread started it meanwhile (then wait).
                let f = slot.lock().unwrap_or_else(PoisonError::into_inner).take();
                if let Some(f) = f {
                    return f(c);
                }
            }
            // (A kick while the main thread runs the job already is a harmless extra drain.)
            if let Some((_, kick)) = self.kick.get().filter(|(after, _)| waits == *after) {
                kick(c);
            }
            waits = waits.saturating_add(1);
            (self.wait)(c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;
    use std::time::Duration;

    fn cpu() -> Box<Cpu> {
        crate::new_env(Box::new(std::io::sink()), None).unwrap().0
    }

    #[test]
    fn an_undrained_job_runs_on_the_poster_after_the_timeout() {
        let q = MainQueue::new(|_| {}, 3);
        let mut c = cpu();
        assert_eq!(q.call(&mut c, |_| 7), 7);
        // The stale queue entry is a no-op once the poster ran the job itself.
        q.drain(&mut c);
    }

    #[test]
    fn a_drained_job_answers_the_poster() {
        let cell: Arc<OnceLock<Arc<MainQueue>>> = Arc::new(OnceLock::new());
        let c2 = cell.clone();
        let q = MainQueue::new(move |c| c2.get().unwrap().drain(c), 3);
        cell.set(q.clone()).ok();
        let mut c = cpu();
        assert_eq!(q.call(&mut c, |_| "main"), "main");
    }

    #[test]
    fn a_waiting_poster_kicks_the_main_thread_once() {
        use std::sync::atomic::{AtomicU32, Ordering};
        type Cell = Arc<OnceLock<Arc<MainQueue>>>;
        // The kick plays the main thread's run loop: it counts and drains the queue.
        fn kicker(cell: &Cell, kicks: &Arc<AtomicU32>) -> impl Fn(&mut Cpu) + Send + Sync + 'static {
            let (cell, kicks) = (cell.clone(), kicks.clone());
            move |c| {
                kicks.fetch_add(1, Ordering::SeqCst);
                cell.get().unwrap().drain(c);
            }
        }
        // No frames (nothing drains on its own): the kick after 2 waits makes the "main thread" drain.
        let cell: Cell = Arc::new(OnceLock::new());
        let waits = Arc::new(AtomicU32::new(0));
        let w2 = waits.clone();
        let q = MainQueue::new(move |_| _ = w2.fetch_add(1, Ordering::SeqCst), 50);
        cell.set(q.clone()).ok();
        let (kicks, ignored) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
        q.set_kick(2, kicker(&cell, &kicks));
        q.set_kick(0, kicker(&cell, &ignored));
        let mut c = cpu();
        assert_eq!(q.call(&mut c, |_| 9), 9);
        assert_eq!((kicks.load(Ordering::SeqCst), waits.load(Ordering::SeqCst)), (1, 3));
        assert_eq!(ignored.load(Ordering::SeqCst), 0, "only the first kick counts");
        // A job answered before the kick is due does not kick.
        let cell2: Cell = Arc::new(OnceLock::new());
        let c3 = cell2.clone();
        let quick = MainQueue::new(move |c| c3.get().unwrap().drain(c), 50);
        cell2.set(quick.clone()).ok();
        let early = Arc::new(AtomicU32::new(0));
        quick.set_kick(3, kicker(&cell2, &early));
        assert_eq!(quick.call(&mut c, |_| 1), 1);
        assert_eq!(early.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_job_the_main_thread_already_started_is_waited_for() {
        let q = MainQueue::new(|_| std::thread::sleep(Duration::from_millis(1)), 100);
        let q2 = q.clone();
        let drainer = std::thread::spawn(move || {
            let mut c = cpu();
            for _ in 0..4000 {
                q2.drain(&mut c);
                std::thread::sleep(Duration::from_micros(200));
            }
        });
        let mut c = cpu();
        let r = q.call(&mut c, |_| {
            std::thread::sleep(Duration::from_millis(400));
            5
        });
        assert_eq!(r, 5);
        drainer.join().unwrap();
    }
}

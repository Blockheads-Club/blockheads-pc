//! The frame-rate cap and the FPS counter of a play run, applied where the game presents a frame (its swap call).
//!
//! The swap runs on a guest thread that holds the guest scheduler's baton (`bhpc_bionic::sched`): a plain sleep there
//! would freeze every other guest thread for the whole wait. [`GuestClock`] hands the baton over while it sleeps and
//! takes it back for the last stretch, which [`FramePacer`] spends spinning for an accurate wake-up.

use bhpc_app::hud::{fps_hud, Corner, Hud};
use bhpc_app::{Clock, FpsCounter, FramePacer, SystemClock};
use bhpc_bionic::sched::{Sched, WaitEnd};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The wall clock, whose sleeps let the other guest threads run.
pub struct GuestClock {
    sched: Arc<Sched>,
    real: SystemClock,
}

impl GuestClock {
    pub fn new(sched: Arc<Sched>) -> GuestClock {
        GuestClock { sched, real: SystemClock::new() }
    }
}

impl Clock for GuestClock {
    fn now(&self) -> Duration {
        self.real.now()
    }

    fn sleep(&mut self, d: Duration) {
        let until = Instant::now() + d;
        while Instant::now() < until {
            // Woken early by some other thread's state change: keep waiting. A stopped scheduler cannot hand the
            // baton over any more; then this is an ordinary sleep.
            if self.sched.wait(Some(until)) == WaitEnd::Deadlock {
                std::thread::sleep(until.saturating_duration_since(Instant::now()));
                return;
            }
        }
    }
}

/// What happens at every presented frame: wait for its slot under the cap, count it, refresh the counter picture.
pub struct SwapPacer {
    pacer: FramePacer,
    clock: GuestClock,
    /// The counter, the serial of its latest picture and its corner, when it is shown.
    fps: Option<(FpsCounter, u64, Corner)>,
}

impl SwapPacer {
    /// `limit`: frames per second at most (`None`: no cap); `fps`: produce the counter picture for this corner.
    pub fn new(sched: Arc<Sched>, limit: Option<u32>, fps: Option<Corner>) -> SwapPacer {
        SwapPacer {
            pacer: FramePacer::new(limit),
            clock: GuestClock::new(sched),
            fps: fps.map(|c| (FpsCounter::default(), 0, c)),
        }
    }

    /// Call when a frame is drawn and about to be presented. Waits for its slot, then returns a new counter picture
    /// (for a game picture `height` pixels tall) when the rate was re-measured.
    pub fn frame(&mut self, height: u32) -> Option<Hud> {
        self.pacer.end_frame(&mut self.clock);
        let (counter, serial, corner) = self.fps.as_mut()?;
        let rate = counter.frame(self.clock.now())?;
        *serial += 1;
        Some(fps_hud(rate, *serial, height, *corner))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_sleeps_hand_the_baton_over_and_end_on_time() {
        // The creating thread holds the baton; a wait gives it up and takes it back.
        let sched = Arc::new(Sched::new());
        let mut c = GuestClock::new(sched.clone());
        let t0 = c.now();
        c.sleep(Duration::from_millis(20));
        assert!(c.now() - t0 >= Duration::from_millis(20));
        // A shut-down scheduler: still a real sleep.
        sched.shutdown();
        let t1 = c.now();
        c.sleep(Duration::from_millis(10));
        assert!(c.now() - t1 >= Duration::from_millis(10));
        c.sleep(Duration::ZERO);
    }

    #[test]
    fn frames_are_capped_and_counted() {
        let mut p = SwapPacer::new(Arc::new(Sched::new()), Some(50), Some(Corner::BottomRight));
        let start = Instant::now();
        let mut huds = Vec::new();
        for _ in 0..36 {
            huds.extend(p.frame(1080));
        }
        // The first frame starts the schedule; 35 more at 50 Hz take 700 ms.
        let took = start.elapsed();
        assert!(took >= Duration::from_millis(695), "{took:?}");
        assert!(!huds.is_empty() && huds.len() <= 2, "about one update per 500 ms: {}", huds.len());
        assert_eq!(huds[0].overlay.serial, 1);
        assert_eq!((huds[0].margin, huds[0].corner), (12, Corner::BottomRight));
        // No counter, no cap: returns at once with nothing to draw.
        let mut q = SwapPacer::new(Arc::new(Sched::new()), None, None);
        let start = Instant::now();
        for _ in 0..1000 {
            assert!(q.frame(768).is_none());
        }
        assert!(start.elapsed() < Duration::from_millis(500));
    }
}

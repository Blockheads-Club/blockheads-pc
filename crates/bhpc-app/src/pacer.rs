//! Frame pacing for loops that are not paced by vsync.

use std::time::{Duration, Instant};

/// Time source and sleeper, replaceable in tests.
pub trait Clock {
    fn now(&self) -> Duration;
    fn sleep(&mut self, d: Duration);
}

/// The wall clock.
pub struct SystemClock(Instant);

impl SystemClock {
    pub fn new() -> SystemClock {
        SystemClock(Instant::now())
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// What one frame took.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameTiming {
    /// Time since the previous frame ended (the whole frame including the pacing sleep).
    pub dt: Duration,
    /// How long `end_frame` slept.
    pub slept: Duration,
}

/// Keeps a fixed frame rate without drift: deadlines advance by one period; a frame that is more than one
/// period late resets the schedule instead of running a burst of catch-up frames.
pub struct FramePacer {
    period: Option<Duration>,
    deadline: Option<Duration>,
    last: Option<Duration>,
    smoothed: f32,
}

impl FramePacer {
    /// `None` disables sleeping (vsync or an external driver paces the loop).
    pub fn new(target_fps: Option<u32>) -> FramePacer {
        let period = target_fps.filter(|f| *f > 0).map(|f| Duration::from_secs_f64(1.0 / f as f64));
        FramePacer { period, deadline: None, last: None, smoothed: 0.0 }
    }

    pub fn period(&self) -> Option<Duration> {
        self.period
    }

    /// Call at the end of each frame. Sleeps until the next deadline when pacing is enabled.
    pub fn end_frame(&mut self, clock: &mut dyn Clock) -> FrameTiming {
        let now = clock.now();
        let mut slept = Duration::ZERO;
        if let Some(period) = self.period {
            match self.deadline {
                // The first frame only starts the schedule.
                None => self.deadline = Some(now),
                Some(d) => {
                    let next = d + period;
                    if next > now {
                        slept = next - now;
                        clock.sleep(slept);
                        self.deadline = Some(next);
                    } else if now - next > period {
                        // Far behind: restart the schedule instead of bursting.
                        self.deadline = Some(now);
                    } else {
                        self.deadline = Some(next);
                    }
                }
            }
        }
        let end = clock.now();
        let dt = self.last.map_or(Duration::ZERO, |l| end.saturating_sub(l));
        self.last = Some(end);
        if !dt.is_zero() {
            let inst = 1.0 / dt.as_secs_f32();
            self.smoothed = if self.smoothed == 0.0 { inst } else { self.smoothed * 0.9 + inst * 0.1 };
        }
        FrameTiming { dt, slept }
    }

    /// Smoothed frames per second (0 before the second frame).
    pub fn fps(&self) -> f32 {
        self.smoothed
    }
}

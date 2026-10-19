//! Frame pacing (a frame-rate cap on an absolute schedule) and a frames-per-second counter.

use std::time::{Duration, Instant};

/// How much of a wait is spent spinning (yielding) instead of sleeping: OS sleeps overshoot by up to about a
/// millisecond, so the last stretch before a deadline is waited out in a `yield_now` loop.
pub const SPIN_MARGIN: Duration = Duration::from_micros(1500);

/// Time source and sleeper, replaceable in tests.
pub trait Clock {
    fn now(&self) -> Duration;
    fn sleep(&mut self, d: Duration);
    /// Give up the CPU briefly (one step of a busy wait).
    fn spin(&mut self) {
        std::thread::yield_now();
    }
    /// Wait until `now() >= deadline`: an OS sleep for all but the last [`SPIN_MARGIN`], then spinning, so the
    /// wake-up is accurate to a few microseconds instead of the scheduler's granularity.
    fn sleep_until(&mut self, deadline: Duration) {
        loop {
            let now = self.now();
            if now >= deadline {
                return;
            }
            let left = deadline - now;
            if left > SPIN_MARGIN {
                self.sleep(left - SPIN_MARGIN);
            } else {
                self.spin();
            }
        }
    }
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
                        clock.sleep_until(next);
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

/// Counts presented frames and reports their rate about every `window` (frames in the window / its length).
#[derive(Debug, Clone)]
pub struct FpsCounter {
    window: Duration,
    start: Option<Duration>,
    frames: u32,
}

impl FpsCounter {
    /// Half a second between updates.
    pub const DEFAULT_WINDOW: Duration = Duration::from_millis(500);

    pub fn new(window: Duration) -> FpsCounter {
        FpsCounter { window: window.max(Duration::from_millis(1)), start: None, frames: 0 }
    }

    /// Count a frame presented at `now`; returns the new rate when a window just ended.
    pub fn frame(&mut self, now: Duration) -> Option<f32> {
        let Some(start) = self.start else {
            self.start = Some(now);
            return None;
        };
        self.frames += 1;
        let span = now.saturating_sub(start);
        if span < self.window {
            return None;
        }
        let fps = self.frames as f32 / span.as_secs_f32();
        (self.start, self.frames) = (Some(now), 0);
        Some(fps)
    }
}

impl Default for FpsCounter {
    fn default() -> Self {
        FpsCounter::new(FpsCounter::DEFAULT_WINDOW)
    }
}

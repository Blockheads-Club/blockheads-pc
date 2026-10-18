//! `timerfd` for libkqueue's timer filter (and so for libdispatch's `dispatch_after` and timer sources).
//!
//! The guest's libSystem implements `timerfd_create/settime/gettime` as `syscall(350/353/354, ...)` like Android's
//! libc does; without these the kqueue timer filter fails to arm and every delayed dispatch block (the tutorial
//! popup's hide, for one) never runs. A timer is a descriptor that becomes readable when it expires; `read` returns
//! the number of expirations as a little-endian `u64`.

use super::{rd, wr, Kind, Table, Vfd, EMFILE, O_NONBLOCK};
use bhpc_bionic::layout::errno;
use bhpc_bionic::misc::fail;
use bhpc_bionic::Shared;
use bhpc_cpu::Cpu;
use std::time::{Duration, Instant};

pub(super) const SYS_CREATE: u32 = 350;
pub(super) const SYS_SETTIME: u32 = 353;
pub(super) const SYS_GETTIME: u32 = 354;
const TFD_TIMER_ABSTIME: u32 = 1;

/// One armed (or idle) timer.
#[derive(Default, Clone, Debug, PartialEq)]
pub(crate) struct Timer {
    pub deadline: Option<Instant>,
    pub interval: Duration,
    /// Expirations counted but not yet read.
    pub pending: u64,
}

impl Timer {
    /// Expirations not yet read at `now`.
    pub fn due(&self, now: Instant) -> u64 {
        self.pending + self.elapsed(now)
    }

    /// Expirations since the deadline (0 before it).
    fn elapsed(&self, now: Instant) -> u64 {
        match self.deadline {
            Some(d) if d <= now => {
                let extra = if self.interval.is_zero() { 0 } else { (now - d).as_nanos() / self.interval.as_nanos() };
                1 + extra as u64
            }
            _ => 0,
        }
    }

    /// Read and clear the expirations at `now`, moving the deadline past them (or disarming a one-shot).
    pub fn take(&mut self, now: Instant) -> u64 {
        let fresh = self.elapsed(now);
        let n = self.pending + fresh;
        if fresh > 0 {
            self.deadline = if self.interval.is_zero() {
                None
            } else {
                self.deadline.map(|d| d + self.interval * u32::try_from(fresh).unwrap_or(u32::MAX))
            };
        }
        self.pending = 0;
        n
    }

    /// Arm for `value` from `now` (zero disarms), repeating every `interval` if that is not zero.
    pub fn arm(&mut self, now: Instant, value: Duration, interval: Duration) {
        self.deadline = (!value.is_zero()).then(|| now + value);
        self.interval = interval;
        self.pending = 0;
    }

    /// Time left until the next expiration.
    pub fn remaining(&self, now: Instant) -> Duration {
        self.deadline.map_or(Duration::ZERO, |d| d.saturating_duration_since(now))
    }
}

impl Table {
    /// Deadline of the timer behind `fd`, if it is one and armed.
    pub(super) fn timer_deadline(&self, fd: u32) -> Option<Instant> {
        match self.fds.get(&fd)?.kind {
            Kind::Timer(n) => self.timers[n].deadline,
            _ => None,
        }
    }
}

fn spec(d: Duration) -> [u8; 8] {
    let mut b = [0u8; 8];
    b[..4].copy_from_slice(&(d.as_secs() as i32).to_le_bytes());
    b[4..].copy_from_slice(&(d.subsec_nanos() as i32).to_le_bytes());
    b
}

fn parse(b: &[u8]) -> Duration {
    let sec = i32::from_le_bytes(b[..4].try_into().expect("4 bytes")).max(0) as u64;
    let nsec = i32::from_le_bytes(b[4..8].try_into().expect("4 bytes")).clamp(0, 999_999_999) as u32;
    Duration::new(sec, nsec)
}

/// `struct itimerspec` (interval, then value) as the ARM guest lays it out.
fn itimerspec(interval: Duration, value: Duration) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&spec(interval));
    b[8..].copy_from_slice(&spec(value));
    b
}

impl Vfd {
    /// `syscall(SYS_timerfd_*, ...)`: the number is the first argument.
    pub(super) fn timerfd(&self, c: &mut Cpu, st: &Shared, nr: u32) {
        let mut a = c.args();
        let _nr = a.u32(c);
        let (x, y, z, w) = (a.u32(c), a.u32(c), a.u32(c), a.u32(c));
        match nr {
            SYS_CREATE => {
                let mut t = self.t();
                t.timers.push(Timer::default());
                let n = t.timers.len() - 1;
                let fd = t.insert(Kind::Timer(n), y & O_NONBLOCK != 0, false);
                drop(t);
                match fd {
                    Some(fd) => c.ret_u32(fd),
                    None => fail(c, st, EMFILE),
                }
            }
            SYS_SETTIME => self.timerfd_settime(c, st, (x, y, z, w)),
            _ => self.timerfd_gettime(c, st, x, y),
        }
    }

    fn timer_of(&self, fd: u32) -> Result<usize, u32> {
        match self.t().fds.get(&fd).map(|e| e.kind.clone()) {
            Some(Kind::Timer(n)) => Ok(n),
            Some(_) => Err(errno::EINVAL),
            None => Err(errno::EBADF),
        }
    }

    fn timerfd_settime(&self, c: &mut Cpu, st: &Shared, (fd, flags, new, old): (u32, u32, u32, u32)) {
        let n = match self.timer_of(fd) {
            Ok(n) => n,
            Err(e) => return fail(c, st, e),
        };
        let Some(raw) = rd(c, new, 16) else { return };
        let (interval, mut value) = (parse(&raw[..8]), parse(&raw[8..]));
        let now = Instant::now();
        if flags & TFD_TIMER_ABSTIME != 0 && !value.is_zero() {
            // An absolute time on the guest's monotonic clock, which is what `clock_gettime` answers.
            let (secs, nanos) = bhpc_bionic::lock(st).clock.monotonic();
            let here = Duration::new(secs.max(0) as u64, nanos);
            value = value.saturating_sub(here).max(Duration::from_nanos(1));
        }
        let before = {
            let mut t = self.t();
            let timer = &mut t.timers[n];
            let before = (timer.interval, timer.remaining(now));
            timer.arm(now, value, interval);
            before
        };
        if old != 0 && !wr(c, old, &itimerspec(before.0, before.1)) {
            return;
        }
        // Waiters computed their wake-up time from the old state.
        bhpc_bionic::sched::notify(st);
        c.ret_u32(0);
    }

    fn timerfd_gettime(&self, c: &mut Cpu, st: &Shared, fd: u32, out: u32) {
        let n = match self.timer_of(fd) {
            Ok(n) => n,
            Err(e) => return fail(c, st, e),
        };
        let (interval, left) = {
            let t = self.t();
            (t.timers[n].interval, t.timers[n].remaining(Instant::now()))
        };
        if wr(c, out, &itimerspec(interval, left)) {
            c.ret_u32(0);
        }
    }
}

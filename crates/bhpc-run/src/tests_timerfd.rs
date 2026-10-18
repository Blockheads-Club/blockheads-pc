//! `syscall(2)` eventfds and timerfds (libkqueue's wake-up and timer filters), driven through the guest-visible thunk.
use crate::tests_vfd::{rig, Rig};
use crate::vfd::timerfd::Timer;
use bhpc_bionic::layout::errno;
use std::time::{Duration, Instant};

const NEG1: u32 = u32::MAX;
const O_NONBLOCK: u32 = 0x800;
const EPOLLIN: u32 = 1;
const SYS_TIMERFD_CREATE: u32 = 350;
const SYS_EVENTFD: u32 = 351;
const SYS_TIMERFD_SETTIME: u32 = 353;
const SYS_TIMERFD_GETTIME: u32 = 354;
const SYS_EVENTFD2: u32 = 356;
const ABSTIME: u32 = 1;

fn spec(sec: i32, nsec: i32) -> Vec<u8> {
    [sec.to_le_bytes(), nsec.to_le_bytes()].concat()
}

fn itimerspec(interval: (i32, i32), value: (i32, i32)) -> Vec<u8> {
    [spec(interval.0, interval.1), spec(value.0, value.1)].concat()
}

impl Rig {
    fn sys(&mut self, args: &[u32]) -> u32 {
        self.call("syscall", args)
    }

    fn timer(&mut self, flags: u32) -> u32 {
        let fd = self.sys(&[SYS_TIMERFD_CREATE, 1, flags]);
        assert_ne!(fd, NEG1);
        fd
    }

    fn arm(&mut self, fd: u32, flags: u32, interval: (i32, i32), value: (i32, i32), old: u32) -> u32 {
        let new = self.put(2000, &itimerspec(interval, value));
        self.sys(&[SYS_TIMERFD_SETTIME, fd, flags, new, old])
    }

    fn expirations(&mut self, fd: u32) -> Option<u64> {
        let (n, v) = self.read(fd, 8);
        (n == 8).then(|| u64::from_le_bytes(v.try_into().unwrap()))
    }

    fn remaining(&mut self, fd: u32) -> (i32, i32, i32, i32) {
        let out = self.scratch + 2200;
        assert_eq!(self.sys(&[SYS_TIMERFD_GETTIME, fd, out]), 0);
        let w = |i: u32| i32::from_le_bytes(self.get(2200 + 4 * i, 4).try_into().unwrap());
        (w(0), w(1), w(2), w(3))
    }
}

#[test]
fn eventfd_syscalls_create_counters_and_unknown_ones_fail() {
    let mut r = rig();
    let e = r.sys(&[SYS_EVENTFD, 5, 0]);
    assert_eq!(r.expirations(e), Some(5));
    let n = r.sys(&[SYS_EVENTFD2, 0, O_NONBLOCK]);
    assert_eq!(r.expirations(n), None);
    assert_eq!(r.errno(), errno::EAGAIN);
    assert_eq!(r.write(n, &1u64.to_le_bytes()), 8);
    assert_eq!(r.expirations(n), Some(1));
    assert_eq!(r.sys(&[9999]), NEG1, "other syscalls keep bionic's answer");
    assert_eq!(r.errno(), errno::ENOSYS);
}

#[test]
fn timers_expire_repeat_and_report_what_is_left() {
    let mut r = rig();
    let t = r.timer(O_NONBLOCK);
    assert_eq!(r.expirations(t), None, "an idle timer has nothing to read");
    assert_eq!(r.errno(), errno::EAGAIN);
    assert_eq!(r.read(t, 4).0, NEG1);
    assert_eq!(r.errno(), errno::EINVAL);
    // One shot, relative, with the previous setting handed back.
    let old = r.scratch + 2400;
    assert_eq!(r.arm(t, 0, (0, 0), (0, 40_000_000), old), 0);
    assert_eq!(r.get(2400, 16), itimerspec((0, 0), (0, 0)));
    let (_, interval_ns, left_s, left_ns) = r.remaining(t);
    assert_eq!((interval_ns, left_s), (0, 0));
    assert!(left_ns > 0 && left_ns <= 40_000_000);
    assert_eq!(r.expirations(t), None);
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(r.expirations(t), Some(1));
    assert_eq!(r.expirations(t), None, "a one-shot timer is spent");
    // Repeating: several expirations pile up and are counted at once.
    assert_eq!(r.arm(t, 0, (0, 10_000_000), (0, 10_000_000), old), 0);
    std::thread::sleep(Duration::from_millis(55));
    assert!(r.expirations(t).unwrap() >= 3);
    let (_, interval_ns, _, _) = r.remaining(t);
    assert_eq!(interval_ns, 10_000_000);
    // Re-arming reports the old setting; zero disarms.
    assert_eq!(r.arm(t, 0, (0, 0), (0, 0), old), 0);
    assert_eq!(r.get(2400 + 4, 4), 10_000_000i32.to_le_bytes());
    std::thread::sleep(Duration::from_millis(25));
    assert_eq!(r.expirations(t), None);
}

#[test]
fn absolute_timers_use_the_guest_monotonic_clock() {
    let mut r = rig();
    let t = r.timer(O_NONBLOCK);
    let ts = r.scratch + 2600;
    assert_eq!(r.call("clock_gettime", &[1, ts]), 0);
    let now = r.cpu.mem().read_u32(ts).unwrap() as i32;
    // 30 ms from now, as an absolute time.
    let ns = i32::from_le_bytes(r.get(2604, 4).try_into().unwrap()) + 30_000_000;
    let (sec, nsec) = (now + ns / 1_000_000_000, ns % 1_000_000_000);
    assert_eq!(r.arm(t, ABSTIME, (0, 0), (sec, nsec), 0), 0);
    assert_eq!(r.expirations(t), None);
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(r.expirations(t), Some(1));
    // A time in the past fires at once; zero with the flag still disarms.
    assert_eq!(r.arm(t, ABSTIME, (0, 0), (1, 0), 0), 0);
    assert_eq!(r.expirations(t), Some(1));
    assert_eq!(r.arm(t, ABSTIME, (0, 0), (0, 0), 0), 0);
    assert_eq!(r.expirations(t), None);
}

#[test]
fn blocking_reads_and_epoll_wait_for_the_timer() {
    let mut r = rig();
    let t = r.timer(0);
    assert_eq!(r.arm(t, 0, (0, 0), (0, 30_000_000), 0), 0);
    let start = Instant::now();
    assert_eq!(r.expirations(t), Some(1), "a blocking read sleeps until the expiry");
    assert!(start.elapsed() >= Duration::from_millis(25));
    // epoll with no timeout sleeps no longer than the watched timer.
    let ep = r.call("epoll_create1", &[0]);
    let ev = r.event(EPOLLIN, 42);
    assert_eq!(r.call("epoll_ctl", &[ep, 1, t, ev]), 0);
    assert_eq!(r.arm(t, 0, (0, 0), (0, 30_000_000), 0), 0);
    assert_eq!(r.wait(ep, 4, 0).0, 0, "not yet");
    let start = Instant::now();
    assert_eq!(r.wait(ep, 4, -1), (1, vec![(EPOLLIN, 42)]));
    assert!(start.elapsed() >= Duration::from_millis(25));
    assert!(r.expirations(t).is_some());
    // A finite timeout and a timer: the earlier of the two ends the wait.
    assert_eq!(r.arm(t, 0, (0, 0), (0, 30_000_000), 0), 0);
    let start = Instant::now();
    assert_eq!(r.wait(ep, 4, 2000).0, 1);
    assert!(start.elapsed() < Duration::from_millis(1500), "woken by the timer, not the timeout");
}

#[test]
fn timer_calls_validate_their_descriptor() {
    let mut r = rig();
    let (pipe_r, _) = r.pipe(0);
    let spec_at = r.put(2000, &itimerspec((0, 0), (0, 1)));
    for fd in [pipe_r, 9999] {
        let want = if fd == 9999 { errno::EBADF } else { errno::EINVAL };
        assert_eq!(r.sys(&[SYS_TIMERFD_SETTIME, fd, 0, spec_at, 0]), NEG1);
        assert_eq!(r.errno(), want);
        assert_eq!(r.sys(&[SYS_TIMERFD_GETTIME, fd, r.scratch + 2200]), NEG1);
        assert_eq!(r.errno(), want);
    }
    let t = r.timer(0);
    assert!(r.faults("syscall", &[SYS_TIMERFD_SETTIME, t, 0, 0xDEAD_0000, 0]));
    assert!(r.faults("syscall", &[SYS_TIMERFD_SETTIME, t, 0, spec_at, 0xDEAD_0000]));
    assert!(r.faults("syscall", &[SYS_TIMERFD_GETTIME, t, 0xDEAD_0000]));
}

#[test]
fn running_out_of_descriptors_fails_timer_creation() {
    let mut r = rig();
    let mut last = 0;
    while last != NEG1 {
        last = r.sys(&[SYS_TIMERFD_CREATE, 1, 0]);
    }
    assert_eq!(r.errno(), 24, "EMFILE");
}

#[test]
fn timer_arithmetic() {
    let now = Instant::now();
    let mut t = Timer::default();
    assert_eq!((t.due(now), t.take(now), t.remaining(now)), (0, 0, Duration::ZERO));
    t.arm(now, Duration::from_millis(10), Duration::ZERO);
    assert_eq!(t.due(now), 0);
    assert_eq!(t.remaining(now), Duration::from_millis(10));
    let later = now + Duration::from_millis(15);
    assert_eq!((t.due(later), t.take(later)), (1, 1));
    assert_eq!(t.deadline, None);
    t.arm(now, Duration::from_millis(10), Duration::from_millis(5));
    let later = now + Duration::from_millis(22);
    assert_eq!(t.due(later), 3);
    assert_eq!(t.take(later), 3);
    assert_eq!(t.deadline, Some(now + Duration::from_millis(25)));
    // Unread expirations carry over until they are read.
    t.pending = 2;
    assert_eq!(t.take(now), 2);
    assert_eq!(t.pending, 0);
}

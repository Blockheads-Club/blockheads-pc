//! `<time.h>` / `<sys/time.h>` / sleeping. The guest's time zone is always UTC.

use crate::guest::{rd32, wr32};
use crate::layout::{clock, errno, timespec, tm};
use crate::misc::fail;
use crate::{lock, Impl, Shared};
use bhpc_cpu::Cpu;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Time source and sleeper behind the guest's clocks (replaceable for deterministic tests).
pub trait HostClock: Send {
    /// Wall-clock time since the epoch: `(seconds, nanoseconds)`.
    fn realtime(&mut self) -> (i64, u32);
    /// Monotonic time.
    fn monotonic(&mut self) -> (i64, u32);
    /// CPU time consumed by the process.
    fn cpu_time(&mut self) -> (i64, u32);
    /// Block the calling thread for `ns` nanoseconds.
    fn sleep(&mut self, ns: u64);
}

/// The real clock.
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    pub fn new() -> SystemClock {
        SystemClock { start: Instant::now() }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock::new()
    }
}

impl HostClock for SystemClock {
    fn realtime(&mut self) -> (i64, u32) {
        let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        (d.as_secs() as i64, d.subsec_nanos())
    }
    fn monotonic(&mut self) -> (i64, u32) {
        // Pretend the device booted 1000 s before this process started.
        let d = self.start.elapsed();
        (d.as_secs() as i64 + 1000, d.subsec_nanos())
    }
    fn cpu_time(&mut self) -> (i64, u32) {
        // Approximation: the wall time since start (the guest is the only workload).
        let d = self.start.elapsed();
        (d.as_secs() as i64, d.subsec_nanos())
    }
    fn sleep(&mut self, ns: u64) {
        std::thread::sleep(std::time::Duration::from_nanos(ns));
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (month 1..=12).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `(year, month 1..=12, day)` of a day count since 1970-01-01.
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

/// Broken-down UTC time: the eleven 32-bit words of `struct tm` before the zone pointer.
pub fn break_down(t: i64) -> [i32; 9] {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let wday = (days + 4).rem_euclid(7);
    let yday = days - days_from_civil(y, 1, 1);
    [
        (secs % 60) as i32,
        (secs / 60 % 60) as i32,
        (secs / 3600) as i32,
        d as i32,
        (m - 1) as i32,
        (y - 1900) as i32,
        wday as i32,
        yday as i32,
        0,
    ]
}

fn write_tm(c: &mut Cpu, st: &Shared, at: u32, t: i64) -> bool {
    let utc = lock(st).utc;
    let f = break_down(t);
    for (i, v) in f.iter().enumerate() {
        if !wr32(c, at + 4 * i as u32, *v as u32) {
            return false;
        }
    }
    wr32(c, at + tm::GMTOFF, 0) && wr32(c, at + tm::ZONE, utc)
}

fn time(c: &mut Cpu, st: &Shared) {
    let p = c.args().u32(c);
    let secs = lock(st).clock.realtime().0 as u32;
    if p != 0 && !wr32(c, p, secs) {
        return;
    }
    c.ret_u32(secs);
}

fn clock_pick(st: &Shared, id: u32) -> Option<(i64, u32)> {
    let mut s = lock(st);
    match id {
        clock::REALTIME | clock::REALTIME_COARSE => Some(s.clock.realtime()),
        clock::MONOTONIC | clock::MONOTONIC_RAW | clock::MONOTONIC_COARSE | clock::BOOTTIME => {
            Some(s.clock.monotonic())
        }
        clock::PROCESS_CPUTIME | clock::THREAD_CPUTIME => Some(s.clock.cpu_time()),
        _ => None,
    }
}

fn clock_gettime(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (id, ts) = (a.u32(c), a.u32(c));
    let Some((sec, nsec)) = clock_pick(st, id) else { return fail(c, st, errno::EINVAL) };
    if wr32(c, ts + timespec::SEC, sec as u32) && wr32(c, ts + timespec::NSEC, nsec) {
        c.ret_u32(0);
    }
}

fn clock_getres(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (id, ts) = (a.u32(c), a.u32(c));
    if clock_pick(st, id).is_none() {
        return fail(c, st, errno::EINVAL);
    }
    if ts != 0 && !(wr32(c, ts + timespec::SEC, 0) && wr32(c, ts + timespec::NSEC, 1)) {
        return;
    }
    c.ret_u32(0);
}

fn gettimeofday(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (tv, tz) = (a.u32(c), a.u32(c));
    let (sec, nsec) = lock(st).clock.realtime();
    if tv != 0 && !(wr32(c, tv, sec as u32) && wr32(c, tv + 4, nsec / 1000)) {
        return;
    }
    if tz != 0 && !(wr32(c, tz, 0) && wr32(c, tz + 4, 0)) {
        return;
    }
    c.ret_u32(0);
}

fn clock_fn(c: &mut Cpu, st: &Shared) {
    let (sec, nsec) = lock(st).clock.cpu_time();
    c.ret_u32((sec as u32).wrapping_mul(clock::CLOCKS_PER_SEC).wrapping_add(nsec / 1000));
}

fn gmtime_r(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (tp, out) = (a.u32(c), a.u32(c));
    let Some(t) = rd32(c, tp) else { return };
    if write_tm(c, st, out, t as i32 as i64) {
        c.ret_u32(out);
    }
}

fn gmtime(c: &mut Cpu, st: &Shared) {
    let tp = c.args().u32(c);
    let buf = lock(st).tm_buf;
    let Some(t) = rd32(c, tp) else { return };
    if write_tm(c, st, buf, t as i32 as i64) {
        c.ret_u32(buf);
    }
}

fn mktime(c: &mut Cpu, st: &Shared) {
    let p = c.args().u32(c);
    let mut f = [0i64; 6];
    for (i, slot) in f.iter_mut().enumerate() {
        let Some(v) = rd32(c, p + 4 * i as u32) else { return };
        *slot = v as i32 as i64;
    }
    let [sec, min, hour, mday, mon, year] = f;
    let y = year + 1900 + mon.div_euclid(12);
    let days = days_from_civil(y, mon.rem_euclid(12) + 1, 1) + mday - 1;
    let t = days * 86_400 + hour * 3600 + min * 60 + sec;
    if i32::try_from(t).is_err() {
        return c.ret_u32(-1i32 as u32);
    }
    if write_tm(c, st, p, t) {
        c.ret_u32(t as u32);
    }
}

fn nanosleep(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (req, rem) = (a.u32(c), a.u32(c));
    let (Some(sec), Some(nsec)) = (rd32(c, req + timespec::SEC), rd32(c, req + timespec::NSEC)) else { return };
    if (sec as i32) < 0 || nsec >= 1_000_000_000 {
        return fail(c, st, errno::EINVAL);
    }
    lock(st).clock.sleep(sec as u64 * 1_000_000_000 + nsec as u64);
    if rem != 0 && !(wr32(c, rem + timespec::SEC, 0) && wr32(c, rem + timespec::NSEC, 0)) {
        return;
    }
    c.ret_u32(0);
}

fn usleep(c: &mut Cpu, st: &Shared) {
    let us = c.args().u32(c);
    lock(st).clock.sleep(us as u64 * 1000);
    c.ret_u32(0);
}

fn sleep(c: &mut Cpu, st: &Shared) {
    let s = c.args().u32(c);
    lock(st).clock.sleep(s as u64 * 1_000_000_000);
    c.ret_u32(0);
}

fn tzset(_: &mut Cpu, _: &Shared) {}

pub(crate) const TABLE: &[(&str, Impl)] = &[
    ("time", time),
    ("clock_gettime", clock_gettime),
    ("clock_getres", clock_getres),
    ("gettimeofday", gettimeofday),
    ("clock", clock_fn),
    ("gmtime", gmtime),
    ("gmtime_r", gmtime_r),
    ("localtime", gmtime),
    ("localtime_r", gmtime_r),
    ("mktime", mktime),
    ("nanosleep", nanosleep),
    ("usleep", usleep),
    ("sleep", sleep),
    ("tzset", tzset),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testenv::{Env, SCRATCH};

    #[test]
    fn civil_conversions_round_trip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(days_from_civil(2000, 2, 29)), (2000, 2, 29));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        for d in [-800_000i64, -1, 0, 1, 59, 60, 365, 11_016, 20_000, 2_000_000] {
            let (y, m, dd) = civil_from_days(d);
            assert_eq!(days_from_civil(y, m, dd), d);
        }
        // 2001-09-09 01:46:40 UTC, a Sunday, day 251 of the year.
        assert_eq!(break_down(1_000_000_000), [40, 46, 1, 9, 8, 101, 0, 251, 0]);
        assert_eq!(break_down(-1), [59, 59, 23, 31, 11, 69, 3, 364, 0]);
    }

    #[test]
    fn time_and_clocks_use_the_host_clock() {
        let mut e = Env::new();
        e.clock.set(1_700_000_000, 123_456_789);
        assert_eq!(e.call("time", &[0]).r0, 1_700_000_000);
        assert_eq!(e.call("time", &[SCRATCH]).r0, 1_700_000_000);
        assert_eq!(e.mem().read_u32(SCRATCH).unwrap(), 1_700_000_000);
        assert_eq!(e.call("clock_gettime", &[0, SCRATCH]).r0, 0);
        assert_eq!(e.mem().read_u32(SCRATCH).unwrap(), 1_700_000_000);
        assert_eq!(e.mem().read_u32(SCRATCH + 4).unwrap(), 123_456_789);
        for (id, sec) in [(1, 5000u32), (4, 5000), (6, 5000), (7, 5000), (5, 1_700_000_000), (2, 7), (3, 7)] {
            e.call("clock_gettime", &[id, SCRATCH]);
            assert_eq!(e.mem().read_u32(SCRATCH).unwrap(), sec, "clock {id}");
        }
        assert_eq!(e.call("clock_gettime", &[99, SCRATCH]).r0 as i32, -1);
        assert_eq!(e.errno(), 22);
        assert_eq!(e.call("clock_getres", &[1, SCRATCH]).r0, 0);
        assert_eq!(e.mem().read_u32(SCRATCH + 4).unwrap(), 1);
        assert_eq!(e.call("clock_getres", &[1, 0]).r0, 0);
        assert_eq!(e.call("clock_getres", &[99, 0]).r0 as i32, -1);
        assert_eq!(e.call("gettimeofday", &[SCRATCH, SCRATCH + 16]).r0, 0);
        assert_eq!(e.mem().read_u32(SCRATCH + 4).unwrap(), 123_456);
        assert_eq!(e.mem().read_u32(SCRATCH + 16).unwrap(), 0);
        assert_eq!(e.call("gettimeofday", &[0, 0]).r0, 0);
        assert_eq!(e.call("clock", &[]).r0, 7_000_000);
        e.clock.set_cpu(2, 500_000);
        assert_eq!(e.call("clock", &[]).r0, 2_000_500);
    }

    #[test]
    fn broken_down_time_and_mktime() {
        let mut e = Env::new();
        e.mem().write_u32(SCRATCH, 1_000_000_000).unwrap();
        let r = e.call("gmtime_r", &[SCRATCH, SCRATCH + 0x40]).r0;
        assert_eq!(r, SCRATCH + 0x40);
        let words: Vec<i32> = (0..9).map(|i| e.mem().read_u32(SCRATCH + 0x40 + 4 * i).unwrap() as i32).collect();
        assert_eq!(words, [40, 46, 1, 9, 8, 101, 0, 251, 0]);
        let zone = e.mem().read_u32(SCRATCH + 0x40 + tm::ZONE).unwrap();
        assert_eq!(e.mem().read_cstr(zone).unwrap(), "UTC");
        assert_eq!(e.mem().read_u32(SCRATCH + 0x40 + tm::GMTOFF).unwrap(), 0);
        let p = e.call("gmtime", &[SCRATCH]).r0;
        let q = e.call("localtime", &[SCRATCH]).r0;
        assert_eq!(p, q, "static buffer");
        assert_eq!(e.mem().read_u32(p + tm::YEAR).unwrap(), 101);
        e.call("localtime_r", &[SCRATCH, SCRATCH + 0x80]);
        assert_eq!(e.mem().read_u32(SCRATCH + 0x80 + tm::HOUR).unwrap(), 1);
        // mktime normalises: sec 100, min 105, hour 0, mday 9, month 20 of year 100 is 2001-09-09 01:46:40.
        for (i, v) in [100u32, 105, 0, 9, 20, 100].iter().enumerate() {
            e.mem().write_u32(SCRATCH + 0x100 + 4 * i as u32, *v).unwrap();
        }
        assert_eq!(e.call("mktime", &[SCRATCH + 0x100]).r0, 1_000_000_000);
        assert_eq!(e.mem().read_u32(SCRATCH + 0x100 + tm::YEAR).unwrap(), 101);
        assert_eq!(e.mem().read_u32(SCRATCH + 0x100 + tm::WDAY).unwrap(), 0);
        // Far outside 32-bit time_t.
        e.mem().write_u32(SCRATCH + 0x100 + 20, 400).unwrap();
        assert_eq!(e.call("mktime", &[SCRATCH + 0x100]).r0 as i32, -1);
    }

    #[test]
    fn sleeping_goes_through_the_clock() {
        let mut e = Env::new();
        e.mem().write_u32(SCRATCH, 2).unwrap();
        e.mem().write_u32(SCRATCH + 4, 500).unwrap();
        e.mem().write_u32(SCRATCH + 8, 99).unwrap();
        assert_eq!(e.call("nanosleep", &[SCRATCH, SCRATCH + 8]).r0, 0);
        assert_eq!(e.mem().read_u32(SCRATCH + 8).unwrap(), 0);
        assert_eq!(e.call("nanosleep", &[SCRATCH, 0]).r0, 0);
        e.mem().write_u32(SCRATCH + 4, 1_000_000_000).unwrap();
        assert_eq!(e.call("nanosleep", &[SCRATCH, 0]).r0 as i32, -1);
        assert_eq!(e.errno(), 22);
        assert_eq!(e.call("usleep", &[1500]).r0, 0);
        assert_eq!(e.call("sleep", &[3]).r0, 0);
        e.call("tzset", &[]);
        assert_eq!(e.clock.slept(), vec![2_000_000_500, 2_000_000_500, 1_500_000, 3_000_000_000]);
    }

    #[test]
    fn bad_guest_pointers_fault() {
        let mut e = Env::new();
        for (f, args) in [
            ("time", vec![0x10]),
            ("clock_gettime", vec![0, 0x10]),
            ("gettimeofday", vec![0x10, 0]),
            ("gettimeofday", vec![SCRATCH, 0x10]),
            ("gmtime", vec![0x10]),
            ("gmtime_r", vec![0x10, SCRATCH]),
            ("gmtime_r", vec![SCRATCH, 0x10]),
            ("mktime", vec![0x10]),
            ("nanosleep", vec![0x10, 0]),
            ("nanosleep", vec![SCRATCH, 0x10]),
            ("clock_getres", vec![0, 0x10]),
        ] {
            let r = e.try_call(f, &args);
            assert!(matches!(r, Err(bhpc_cpu::Stop::Fault(_))), "{f} {args:?} -> {r:?}");
        }
    }

    #[test]
    fn mktime_write_back_fault() {
        let mut e = Env::new();
        // The tm struct ends right at the end of the mapped scratch area: reads succeed, the write-back of
        // tm_zone (offset 40) fails because the object is truncated by the end of the mapping.
        let end = SCRATCH + 0x10000;
        e.mem().write_u32(end - 4, 100).unwrap();
        let r = e.try_call("mktime", &[end - 24]);
        assert!(matches!(r, Err(bhpc_cpu::Stop::Fault(_))));
    }

    #[test]
    fn system_clock_is_sane() {
        let mut c = SystemClock::default();
        let (s, _) = c.realtime();
        assert!(s > 1_600_000_000);
        let (m1, _) = c.monotonic();
        assert!(m1 >= 1000);
        assert!(c.cpu_time().0 < 100);
        c.sleep(1000);
    }
}

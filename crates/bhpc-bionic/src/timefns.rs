//! `<time.h>` / `<sys/time.h>` / sleeping. `gmtime` is UTC; `localtime`, `mktime` and `tzset` use the host's local
//! time zone (through [`HostClock::local_zone`]) unless the guest environment sets `TZ` to UTC.

use crate::guest::{rd32, wr32};
use crate::layout::{clock, errno, timespec, tm};
use crate::misc::fail;
use crate::{lock, Impl, Shared};
use bhpc_cpu::Cpu;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The local time zone in effect at one instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneInfo {
    /// Seconds east of UTC (`tm_gmtoff`).
    pub offset: i32,
    /// Daylight saving time is in effect.
    pub dst: bool,
    /// Abbreviation (`tm_zone`), such as `CEST`.
    pub name: String,
}

impl ZoneInfo {
    /// Coordinated Universal Time.
    pub fn utc() -> ZoneInfo {
        ZoneInfo { offset: 0, dst: false, name: "UTC".to_string() }
    }
}

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
    /// The local time zone at the UTC instant `t` (seconds since the epoch). UTC unless overridden.
    fn local_zone(&mut self, _t: i64) -> ZoneInfo {
        ZoneInfo::utc()
    }
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
    fn local_zone(&mut self, t: i64) -> ZoneInfo {
        host_zone(t)
    }
}

/// The host's own local time zone, from the C library (honours the host `TZ` and `/etc/localtime`).
#[cfg(unix)]
fn host_zone(t: i64) -> ZoneInfo {
    use std::ffi::CStr;
    // SAFETY: `localtime_r` only writes into the zeroed `tm` we pass; `tm_zone` points to static zone data.
    unsafe {
        let secs = t as libc::time_t;
        let mut out: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&secs, &mut out).is_null() {
            return ZoneInfo::utc();
        }
        let name =
            if out.tm_zone.is_null() { "UTC".into() } else { CStr::from_ptr(out.tm_zone).to_string_lossy().into() };
        ZoneInfo { offset: out.tm_gmtoff as i32, dst: out.tm_isdst > 0, name }
    }
}

/// Windows: the CRT's `localtime_s` (the system time zone, daylight saving included). Its `tm` has no offset or
/// zone name, so both come from the broken-down local time ([`zone_of_local`]).
#[cfg(windows)]
fn host_zone(t: i64) -> ZoneInfo {
    // SAFETY: `localtime_s` only writes into the zeroed `tm` we pass.
    unsafe {
        let secs = t as libc::time_t;
        let mut out: libc::tm = std::mem::zeroed();
        if libc::localtime_s(&mut out, &secs) != 0 {
            return ZoneInfo::utc();
        }
        let local = (out.tm_year, out.tm_mon, out.tm_mday, out.tm_hour, out.tm_min, out.tm_sec);
        zone_of_local(t, local, out.tm_isdst > 0)
    }
}

/// The zone in effect when the UTC instant `t` reads as `local` (`tm` fields: years since 1900, month 0..=11, day,
/// hour, minute, second): its offset, and a name made from it (`UTC`, `UTC+03:00`, `UTC-09:30`).
#[cfg_attr(not(windows), allow(dead_code))]
fn zone_of_local(t: i64, local: (i32, i32, i32, i32, i32, i32), dst: bool) -> ZoneInfo {
    let (y, mon, d, h, m, s) = local;
    let days = days_from_civil(i64::from(y) + 1900, i64::from(mon) + 1, i64::from(d));
    let offset = (days * 86_400 + i64::from(h) * 3600 + i64::from(m) * 60 + i64::from(s) - t) as i32;
    let name = match offset {
        0 => "UTC".to_string(),
        o => format!("UTC{}{:02}:{:02}", if o < 0 { '-' } else { '+' }, o.abs() / 3600, o.abs() % 3600 / 60),
    };
    ZoneInfo { offset, dst, name }
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

/// Guest address of a NUL-terminated copy of the zone abbreviation `name` (kept for the life of the process).
fn zone_name(st: &Shared, name: &str) -> u32 {
    let mut s = lock(st);
    if let Some(p) = s.zone_names.get(name) {
        return *p;
    }
    let p = s.heap.alloc(name.len() as u32 + 1);
    let mut bytes = name.as_bytes().to_vec();
    bytes.push(0);
    // The heap block is always mapped.
    let _ = s.mem.write_bytes(p, &bytes);
    s.zone_names.insert(name.to_string(), p);
    p
}

/// The zone the guest sees at UTC instant `t`: UTC if its environment says `TZ=UTC` (or `GMT`), else the host's.
fn zone_at(st: &Shared, t: i64) -> ZoneInfo {
    let mut s = lock(st);
    let forced_utc = s.env.iter().find(|e| e.name == b"TZ").is_some_and(|e| {
        let val = s.mem.read_cstr(e.base).unwrap_or_default();
        let val = val.split_once('=').map_or("", |kv| kv.1).trim_start_matches(':');
        matches!(val, "" | "UTC" | "UTC0" | "GMT" | "GMT0")
    });
    if forced_utc {
        return ZoneInfo::utc();
    }
    s.clock.local_zone(t)
}

/// Write the broken-down time of `t`, in the local zone if `local`, into the `struct tm` at `at`.
fn write_tm(c: &mut Cpu, st: &Shared, at: u32, t: i64, local: bool) -> bool {
    if local && !lock(st).tz_ready {
        publish_tz(st);
    }
    let zone = if local { zone_at(st, t) } else { ZoneInfo::utc() };
    let name = if local { zone_name(st, &zone.name) } else { lock(st).utc };
    let mut f = break_down(t + i64::from(zone.offset));
    f[8] = i32::from(zone.dst);
    for (i, v) in f.iter().enumerate() {
        if !wr32(c, at + 4 * i as u32, *v as u32) {
            return false;
        }
    }
    wr32(c, at + tm::GMTOFF, zone.offset as u32) && wr32(c, at + tm::ZONE, name)
}

/// `tzset`: publish `timezone` (seconds west of UTC, standard time) and `tzname` of the local zone.
fn tzset(_: &mut Cpu, st: &Shared) {
    publish_tz(st);
}

/// Fill the `timezone` and `tzname` data symbols from the local zone.
fn publish_tz(st: &Shared) {
    lock(st).tz_ready = true;
    let now = lock(st).clock.realtime().0;
    let here = zone_at(st, now);
    // The other half of the year tells the standard name from the daylight one.
    let other = zone_at(st, now + 182 * 86_400);
    let (std, dst) = if here.dst { (other.clone(), here.clone()) } else { (here.clone(), other.clone()) };
    let dst = if dst.dst { dst } else { std.clone() };
    let (std_name, dst_name) = (zone_name(st, &std.name), zone_name(st, &dst.name));
    let s = lock(st);
    let (tz, names) = (s.data["timezone"], s.data["tzname"]);
    let _ = s.mem.write_u32(tz, (-std.offset) as u32);
    let _ = s.mem.write_u32(names, std_name);
    let _ = s.mem.write_u32(names + 4, dst_name);
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

fn broken_down_r(c: &mut Cpu, st: &Shared, local: bool) {
    let mut a = c.args();
    let (tp, out) = (a.u32(c), a.u32(c));
    let Some(t) = rd32(c, tp) else { return };
    if write_tm(c, st, out, t as i32 as i64, local) {
        c.ret_u32(out);
    }
}

fn broken_down(c: &mut Cpu, st: &Shared, local: bool) {
    let tp = c.args().u32(c);
    let buf = lock(st).tm_buf;
    let Some(t) = rd32(c, tp) else { return };
    if write_tm(c, st, buf, t as i32 as i64, local) {
        c.ret_u32(buf);
    }
}

fn gmtime_r(c: &mut Cpu, st: &Shared) {
    broken_down_r(c, st, false);
}

fn gmtime(c: &mut Cpu, st: &Shared) {
    broken_down(c, st, false);
}

fn localtime_r(c: &mut Cpu, st: &Shared) {
    broken_down_r(c, st, true);
}

fn localtime(c: &mut Cpu, st: &Shared) {
    broken_down(c, st, true);
}

/// `mktime`: the fields are local time; normalise them and return the UTC `time_t`.
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
    let naive = days * 86_400 + hour * 3600 + min * 60 + sec;
    // The offset depends on the instant, which depends on the offset: settle it in two steps.
    let first = i64::from(zone_at(st, naive).offset);
    let guess = naive - first;
    let t = naive - i64::from(zone_at(st, guess).offset);
    if i32::try_from(t).is_err() {
        return c.ret_u32(-1i32 as u32);
    }
    if write_tm(c, st, p, t, true) {
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
    crate::sched::sleep(c, st, sec as u64 * 1_000_000_000 + nsec as u64);
    if rem != 0 && !(wr32(c, rem + timespec::SEC, 0) && wr32(c, rem + timespec::NSEC, 0)) {
        return;
    }
    c.ret_u32(0);
}

fn usleep(c: &mut Cpu, st: &Shared) {
    let us = c.args().u32(c);
    crate::sched::sleep(c, st, us as u64 * 1000);
    c.ret_u32(0);
}

fn sleep(c: &mut Cpu, st: &Shared) {
    let s = c.args().u32(c);
    crate::sched::sleep(c, st, s as u64 * 1_000_000_000);
    c.ret_u32(0);
}

/// Read a `struct tm` from guest memory.
fn read_tm(c: &mut Cpu, at: u32) -> Option<crate::strftime::Tm> {
    let mut w = [0i32; 9];
    for (i, slot) in w.iter_mut().enumerate() {
        *slot = rd32(c, at + 4 * i as u32)? as i32;
    }
    let gmtoff = rd32(c, at + tm::GMTOFF)? as i32;
    let zone_ptr = rd32(c, at + tm::ZONE)?;
    let zone = if zone_ptr == 0 {
        String::new()
    } else {
        String::from_utf8_lossy(&crate::guest::cstr(c, zone_ptr)?).into_owned()
    };
    Some(crate::strftime::Tm {
        sec: w[0],
        min: w[1],
        hour: w[2],
        mday: w[3],
        mon: w[4],
        year: w[5],
        wday: w[6],
        yday: w[7],
        gmtoff,
        zone,
    })
}

/// `strftime(buf, max, format, tm)`: length written (without the NUL), or 0 if it does not fit.
fn strftime(c: &mut Cpu, _: &Shared) {
    let mut a = c.args();
    let (buf, max, fmt, tp) = (a.u32(c), a.u32(c), a.u32(c), a.u32(c));
    let (Some(f), Some(t)) = (crate::guest::cstr(c, fmt), read_tm(c, tp)) else { return };
    let out = crate::strftime::format_time(&f, &t);
    if out.len() as u64 + 1 > u64::from(max) {
        return c.ret_u32(0);
    }
    if crate::guest::wr_cstr(c, buf, &out) {
        c.ret_u32(out.len() as u32);
    }
}

pub(crate) const TABLE: &[(&str, Impl)] = &[
    ("time", time),
    ("clock_gettime", clock_gettime),
    ("clock_getres", clock_getres),
    ("gettimeofday", gettimeofday),
    ("clock", clock_fn),
    ("gmtime", gmtime),
    ("gmtime_r", gmtime_r),
    ("localtime", localtime),
    ("localtime_r", localtime_r),
    ("mktime", mktime),
    ("nanosleep", nanosleep),
    ("usleep", usleep),
    ("sleep", sleep),
    ("tzset", tzset),
    ("strftime", strftime),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests_env::{Env, SCRATCH};

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

    #[test]
    fn strftime_formats_a_guest_struct_tm() {
        let mut e = Env::new();
        // 2021-03-04 05:06:07 UTC via gmtime_r.
        e.mem().write_u32(SCRATCH, 1_614_834_367).unwrap();
        let tmp = SCRATCH + 0x80;
        e.call("gmtime_r", &[SCRATCH, tmp]);
        e.put_str(SCRATCH + 0x200, "%Y-%m-%d %H:%M:%S %Z");
        let n = e.call("strftime", &[SCRATCH + 0x300, 64, SCRATCH + 0x200, tmp]).r0;
        assert_eq!(n, 23);
        assert_eq!(e.get_str(SCRATCH + 0x300), "2021-03-04 05:06:07 UTC");
        // Exactly fitting and too small buffers.
        assert_eq!(e.call("strftime", &[SCRATCH + 0x300, 24, SCRATCH + 0x200, tmp]).r0, 23);
        assert_eq!(e.call("strftime", &[SCRATCH + 0x300, 23, SCRATCH + 0x200, tmp]).r0, 0);
        assert_eq!(e.call("strftime", &[SCRATCH + 0x300, 0, SCRATCH + 0x200, tmp]).r0, 0);
        // A tm without a zone name.
        e.mem().write_u32(tmp + crate::layout::tm::ZONE, 0).unwrap();
        e.put_str(SCRATCH + 0x200, "[%Z]");
        e.call("strftime", &[SCRATCH + 0x300, 64, SCRATCH + 0x200, tmp]);
        assert_eq!(e.get_str(SCRATCH + 0x300), "[]");
        // Faults.
        assert!(matches!(e.try_call("strftime", &[0x10, 64, SCRATCH + 0x200, tmp]), Err(bhpc_cpu::Stop::Fault(_))));
        assert!(matches!(e.try_call("strftime", &[SCRATCH + 0x300, 64, 0x10, tmp]), Err(bhpc_cpu::Stop::Fault(_))));
        assert!(matches!(
            e.try_call("strftime", &[SCRATCH + 0x300, 64, SCRATCH + 0x200, 0x10]),
            Err(bhpc_cpu::Stop::Fault(_))
        ));
        e.mem().write_u32(tmp + crate::layout::tm::ZONE, 0x10).unwrap();
        assert!(matches!(
            e.try_call("strftime", &[SCRATCH + 0x300, 64, SCRATCH + 0x200, tmp]),
            Err(bhpc_cpu::Stop::Fault(_))
        ));
    }

    /// A US-Eastern-like zone: EST, and EDT between 2021-03-14 07:00Z and 2021-11-07 06:00Z.
    fn eastern(t: i64) -> ZoneInfo {
        if (1_615_705_200..1_636_264_800).contains(&t) {
            ZoneInfo { offset: -14_400, dst: true, name: "EDT".into() }
        } else {
            ZoneInfo { offset: -18_000, dst: false, name: "EST".into() }
        }
    }

    fn tm_words(e: &Env, at: u32) -> Vec<i32> {
        (0..9).map(|i| e.mem().read_u32(at + 4 * i).unwrap() as i32).collect()
    }

    #[test]
    fn localtime_and_mktime_use_the_host_zone() {
        let mut e = Env::new();
        e.clock.set_zone(eastern);
        // 2021-07-01 16:00:00 UTC is 12:00 EDT.
        let t = (days_from_civil(2021, 7, 1) * 86_400 + 16 * 3600) as u32;
        e.mem().write_u32(SCRATCH, t).unwrap();
        let r = e.call("localtime_r", &[SCRATCH, SCRATCH + 0x40]).r0;
        assert_eq!(r, SCRATCH + 0x40);
        assert_eq!(&tm_words(&e, r)[..3], &[0, 0, 12]);
        assert_eq!(tm_words(&e, r)[8], 1, "tm_isdst");
        assert_eq!(e.mem().read_u32(r + tm::GMTOFF).unwrap() as i32, -14_400);
        assert_eq!(e.get_str(e.mem().read_u32(r + tm::ZONE).unwrap()), "EDT");
        // gmtime stays UTC.
        let g = e.call("gmtime_r", &[SCRATCH, SCRATCH + 0x80]).r0;
        assert_eq!(tm_words(&e, g)[2], 16);
        assert_eq!(e.get_str(e.mem().read_u32(g + tm::ZONE).unwrap()), "UTC");
        // Winter: EST, and the zone name string is shared between calls.
        e.mem().write_u32(SCRATCH, 1_610_000_000).unwrap();
        let w = e.call("localtime", &[SCRATCH]).r0;
        assert_eq!(tm_words(&e, w)[8], 0);
        assert_eq!(e.get_str(e.mem().read_u32(w + tm::ZONE).unwrap()), "EST");
        let z1 = e.mem().read_u32(w + tm::ZONE).unwrap();
        e.call("localtime", &[SCRATCH]);
        assert_eq!(e.mem().read_u32(w + tm::ZONE).unwrap(), z1);
        // mktime reads local fields: 2021-07-01 12:00:00 EDT is 16:00 UTC; and 12:00 on 2021-01-15 EST is 17:00 UTC.
        for (y, m, d, expect) in
            [(2021, 7, 1, t), (2021, 1, 15, (days_from_civil(2021, 1, 15) * 86_400 + 17 * 3600) as u32)]
        {
            for (i, v) in [0u32, 0, 12, d as u32, m as u32 - 1, y as u32 - 1900].iter().enumerate() {
                e.mem().write_u32(SCRATCH + 0x100 + 4 * i as u32, *v).unwrap();
            }
            assert_eq!(e.call("mktime", &[SCRATCH + 0x100]).r0, expect, "{y}-{m}-{d}");
            assert_eq!(e.mem().read_u32(SCRATCH + 0x100 + tm::HOUR).unwrap(), 12);
        }
    }

    #[test]
    fn tzset_publishes_timezone_and_tzname() {
        let mut e = Env::new();
        let (tz, names) = (e.data("timezone"), e.data("tzname"));
        let name = |e: &Env, i: u32| e.get_str(e.mem().read_u32(names + 4 * i).unwrap());
        e.clock.set_zone(eastern);
        // Summer: daylight time now, standard time half a year later.
        e.clock.set(1_625_000_000, 0);
        e.call("tzset", &[]);
        assert_eq!(e.mem().read_u32(tz).unwrap(), 18_000);
        assert_eq!((name(&e, 0), name(&e, 1)), ("EST".to_string(), "EDT".to_string()));
        // Winter.
        e.clock.set(1_610_000_000, 0);
        e.call("tzset", &[]);
        assert_eq!((name(&e, 0), name(&e, 1)), ("EST".to_string(), "EDT".to_string()));
        // A zone without daylight saving time repeats the standard name.
        e.clock.set_zone(|_| ZoneInfo { offset: 3 * 3600, dst: false, name: "+03".into() });
        e.call("tzset", &[]);
        assert_eq!(e.mem().read_u32(tz).unwrap() as i32, -3 * 3600);
        assert_eq!((name(&e, 0), name(&e, 1)), ("+03".to_string(), "+03".to_string()));
        // The first localtime publishes it too.
        let mut e = Env::new();
        e.clock.set_zone(eastern);
        e.call("localtime", &[SCRATCH]);
        assert_eq!(e.mem().read_u32(e.data("timezone")).unwrap(), 18_000);
    }

    #[test]
    fn tz_utc_in_the_guest_environment_forces_utc() {
        for val in ["UTC", ":UTC", "GMT0", ""] {
            let mut e = Env::new();
            e.clock.set_zone(eastern);
            crate::misc::env_set(&mut lock(&e.st), b"TZ", val.as_bytes(), true);
            e.mem().write_u32(SCRATCH, 1_625_000_000).unwrap();
            let r = e.call("localtime", &[SCRATCH]).r0;
            assert_eq!(e.mem().read_u32(r + tm::GMTOFF).unwrap(), 0, "TZ={val}");
        }
        // Another TZ value (or another variable) leaves the host zone alone.
        let mut e = Env::new();
        e.clock.set_zone(eastern);
        crate::misc::env_set(&mut lock(&e.st), b"TZ", b"America/New_York", true);
        crate::misc::env_set(&mut lock(&e.st), b"TZX", b"UTC", true);
        e.mem().write_u32(SCRATCH, 1_625_000_000).unwrap();
        let r = e.call("localtime", &[SCRATCH]).r0;
        assert_eq!(e.mem().read_u32(r + tm::GMTOFF).unwrap() as i32, -14_400);
    }

    #[test]
    fn zones_from_broken_down_local_times() {
        // 2021-06-29 20:40:00 UTC.
        let t = 1_624_999_200;
        assert_eq!(zone_of_local(t, (121, 5, 29, 20, 40, 0), false), ZoneInfo::utc());
        let z = zone_of_local(t, (121, 5, 29, 23, 40, 0), true);
        assert_eq!((z.offset, z.dst, z.name.as_str()), (3 * 3600, true, "UTC+03:00"));
        let z = zone_of_local(t, (121, 5, 29, 11, 10, 0), false);
        assert_eq!((z.offset, z.name.as_str()), (-(9 * 3600 + 30 * 60), "UTC-09:30"));
    }

    #[test]
    fn the_system_clock_reports_the_host_zone() {
        let mut c = SystemClock::default();
        let z = c.local_zone(1_625_000_000);
        assert!(z.offset.abs() <= 14 * 3600 && !z.name.is_empty(), "{z:?}");
        #[cfg(unix)]
        assert_eq!(c.local_zone(i64::MAX), ZoneInfo::utc(), "unrepresentable instants fall back to UTC");
        // A clock that does not know about zones is UTC.
        struct Plain;
        impl HostClock for Plain {
            fn realtime(&mut self) -> (i64, u32) {
                (0, 0)
            }
            fn monotonic(&mut self) -> (i64, u32) {
                (0, 0)
            }
            fn cpu_time(&mut self) -> (i64, u32) {
                (0, 0)
            }
            fn sleep(&mut self, _: u64) {}
        }
        assert_eq!(Plain.local_zone(5), ZoneInfo::utc());
        assert_eq!((Plain.realtime(), Plain.monotonic(), Plain.cpu_time()), ((0, 0), (0, 0), (0, 0)));
        Plain.sleep(1);
    }
}

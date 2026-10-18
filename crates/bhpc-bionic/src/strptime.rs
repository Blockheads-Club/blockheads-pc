//! `strptime` in the C locale: parse text into the date and time fields of a `struct tm`.

use crate::guest::{cstr, rd, wr};
use crate::timefns::days_from_civil;
use crate::{Impl, Shared};
use bhpc_cpu::Cpu;

const DAYS: [&str; 7] = ["sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"];
const MONTHS: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

/// The `struct tm` fields `strptime` sets (same order and meaning as in the C struct).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Fields {
    pub sec: i32,
    pub min: i32,
    pub hour: i32,
    pub mday: i32,
    pub mon: i32,
    pub year: i32,
    pub wday: i32,
    pub yday: i32,
}

/// What the format asked for, to finish the derived fields at the end.
#[derive(Default)]
struct Seen {
    /// A month or day of the month was parsed.
    month_day: bool,
    /// A year was parsed.
    year: bool,
    yday: bool,
    pm: Option<bool>,
    hour12: bool,
}

struct Parser<'a> {
    text: &'a [u8],
    pos: usize,
    f: Fields,
    seen: Seen,
}

impl Parser<'_> {
    fn skip_space(&mut self) {
        while self.text.get(self.pos).is_some_and(u8::is_ascii_whitespace) {
            self.pos += 1;
        }
    }

    /// A number of at most `max_digits` digits in `lo..=hi` (leading blanks allowed).
    fn number(&mut self, max_digits: usize, lo: i32, hi: i32) -> Option<i32> {
        self.skip_space();
        let digits = self.text[self.pos..].iter().take(max_digits).take_while(|b| b.is_ascii_digit()).count();
        let v: i32 = std::str::from_utf8(&self.text[self.pos..self.pos + digits]).ok()?.parse().ok()?;
        if !(lo..=hi).contains(&v) {
            return None;
        }
        self.pos += digits;
        Some(v)
    }

    /// Match one of `names` (full or three-letter abbreviation, any case); returns its index.
    fn name(&mut self, names: &[&str]) -> Option<usize> {
        let rest = &self.text[self.pos..];
        let lower = |n: usize| rest.get(..n).map(|s| s.to_ascii_lowercase());
        // Full names first so "March" is not read as "Mar" + "ch".
        for full in [true, false] {
            for (i, n) in names.iter().enumerate() {
                let n = if full { *n } else { &n[..3] };
                if lower(n.len()).as_deref() == Some(n.as_bytes()) {
                    self.pos += n.len();
                    return Some(i);
                }
            }
        }
        None
    }

    fn literal(&mut self, b: u8) -> Option<()> {
        (self.text.get(self.pos) == Some(&b)).then(|| self.pos += 1)
    }

    fn run(&mut self, fmt: &[u8]) -> Option<()> {
        let mut i = 0;
        while i < fmt.len() {
            let f = fmt[i];
            i += 1;
            if f.is_ascii_whitespace() {
                self.skip_space();
            } else if f != b'%' {
                self.literal(f)?;
            } else {
                // `E` and `O` modifiers select alternative locale forms, which the C locale does not have.
                while matches!(fmt.get(i), Some(b'E' | b'O')) {
                    i += 1;
                }
                let conv = *fmt.get(i)?;
                i += 1;
                self.conversion(conv)?;
            }
        }
        Some(())
    }

    fn conversion(&mut self, conv: u8) -> Option<()> {
        match conv {
            b'%' => self.literal(b'%')?,
            b'a' | b'A' => self.f.wday = self.name(&DAYS)? as i32,
            b'b' | b'B' | b'h' => {
                self.f.mon = self.name(&MONTHS)? as i32;
                self.seen.month_day = true;
            }
            b'd' | b'e' => {
                self.f.mday = self.number(2, 1, 31)?;
                self.seen.month_day = true;
            }
            b'm' => {
                self.f.mon = self.number(2, 1, 12)? - 1;
                self.seen.month_day = true;
            }
            b'Y' => {
                self.f.year = self.number(4, 0, 9999)? - 1900;
                self.seen.year = true;
            }
            b'y' => {
                let y = self.number(2, 0, 99)?;
                self.f.year = if y < 69 { y + 100 } else { y };
                self.seen.year = true;
            }
            b'j' => {
                self.f.yday = self.number(3, 1, 366)? - 1;
                self.seen.yday = true;
            }
            b'H' | b'k' => self.f.hour = self.number(2, 0, 23)?,
            b'I' | b'l' => {
                self.f.hour = self.number(2, 1, 12)? % 12;
                self.seen.hour12 = true;
            }
            b'M' => self.f.min = self.number(2, 0, 59)?,
            b'S' => self.f.sec = self.number(2, 0, 61)?,
            b'p' | b'P' => {
                self.skip_space();
                let word = self.text.get(self.pos..self.pos + 2).map(<[u8]>::to_ascii_lowercase);
                self.seen.pm = match word.as_deref() {
                    Some(b"am") => Some(false),
                    Some(b"pm") => Some(true),
                    _ => return None,
                };
                self.pos += 2;
            }
            b'n' | b't' => self.skip_space(),
            b'D' => self.run(b"%m/%d/%y")?,
            b'F' => self.run(b"%Y-%m-%d")?,
            b'T' => self.run(b"%H:%M:%S")?,
            b'R' => self.run(b"%H:%M")?,
            b'r' => self.run(b"%I:%M:%S %p")?,
            b'c' => self.run(b"%a %b %e %H:%M:%S %Y")?,
            b'x' => self.run(b"%m/%d/%y")?,
            b'X' => self.run(b"%H:%M:%S")?,
            b'Z' => {
                // A zone abbreviation: letters only; the value is not stored (`tm_zone` is not set by strptime).
                while self.text.get(self.pos).is_some_and(u8::is_ascii_alphabetic) {
                    self.pos += 1;
                }
            }
            b'z' => self.zone_offset()?,
            _ => return None,
        }
        Some(())
    }

    /// `+hhmm`, `+hh:mm`, `+hh` or `Z`; parsed and ignored.
    fn zone_offset(&mut self) -> Option<()> {
        self.skip_space();
        if self.literal(b'Z').is_some() {
            return Some(());
        }
        if self.literal(b'+').is_none() {
            self.literal(b'-')?;
        }
        self.number(2, 0, 23)?;
        let _ = self.literal(b':');
        if self.text.get(self.pos).is_some_and(u8::is_ascii_digit) {
            self.number(2, 0, 59)?;
        }
        Some(())
    }
}

/// Parse `text` according to `fmt`, updating `fields`. Returns the number of bytes consumed.
pub fn parse(text: &[u8], fmt: &[u8], fields: &mut Fields) -> Option<usize> {
    let mut p = Parser { text, pos: 0, f: *fields, seen: Seen::default() };
    p.run(fmt)?;
    let mut f = p.f;
    if p.seen.hour12 && p.seen.pm == Some(true) {
        f.hour += 12;
    }
    let year = i64::from(f.year) + 1900;
    if p.seen.yday && !p.seen.month_day {
        let (_, m, d) = crate::timefns::civil_from_days(days_from_civil(year, 1, 1) + i64::from(f.yday));
        f.mon = m as i32 - 1;
        f.mday = d as i32;
        p.seen.month_day = true;
    }
    if p.seen.month_day || p.seen.year {
        let days = days_from_civil(year, i64::from(f.mon) + 1, i64::from(f.mday.max(1)));
        f.wday = (days + 4).rem_euclid(7) as i32;
        f.yday = (days - days_from_civil(year, 1, 1)) as i32;
    }
    *fields = f;
    Some(p.pos)
}

/// `strptime(buf, format, tm)`: pointer to the first unparsed byte of `buf`, or NULL if the text does not match.
fn strptime(c: &mut Cpu, _: &Shared) {
    let mut a = c.args();
    let (buf, fmt, tmp) = (a.u32(c), a.u32(c), a.u32(c));
    let (Some(text), Some(format)) = (cstr(c, buf), cstr(c, fmt)) else { return };
    let Some(raw) = rd(c, tmp, 32) else { return };
    let w: Vec<i32> = raw.chunks(4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    let mut f = Fields { sec: w[0], min: w[1], hour: w[2], mday: w[3], mon: w[4], year: w[5], wday: w[6], yday: w[7] };
    let Some(used) = parse(&text, &format, &mut f) else { return c.ret_u32(0) };
    let out = [f.sec, f.min, f.hour, f.mday, f.mon, f.year, f.wday, f.yday];
    let bytes: Vec<u8> = out.iter().flat_map(|v| v.to_le_bytes()).collect();
    if wr(c, tmp, &bytes) {
        c.ret_u32(buf + used as u32);
    }
}

pub(crate) const TABLE: &[(&str, Impl)] = &[("strptime", strptime)];

#[cfg(test)]
mod tests {
    use crate::tests_env::{Env, SCRATCH};
    use bhpc_cpu::Stop;

    const IN: u32 = SCRATCH;
    const FMT: u32 = SCRATCH + 0x100;
    const TM: u32 = SCRATCH + 0x200;
    const BAD: u32 = 0x10;

    /// Run strptime on a tm whose fields start out as 90..97; returns (bytes consumed, fields).
    fn run(e: &mut Env, input: &str, fmt: &str) -> Option<(u32, Vec<i32>)> {
        e.put_str(IN, input);
        e.put_str(FMT, fmt);
        for i in 0..9 {
            e.mem().write_u32(TM + 4 * i, 90 + i).unwrap();
        }
        let r = e.call("strptime", &[IN, FMT, TM]).r0;
        (r != 0).then(|| (r - IN, (0..9).map(|i| e.mem().read_u32(TM + 4 * i).unwrap() as i32).collect()))
    }

    #[test]
    fn dates_and_times() {
        let mut e = Env::new();
        // 2021-03-04 is a Thursday, day 62 (0-based 62 - 1 = 62? March 4 = 31 + 28 + 4 - 1 = 62).
        let (n, f) = run(&mut e, "2021-03-04 05:06:07 tail", "%Y-%m-%d %H:%M:%S").unwrap();
        assert_eq!(n, 19);
        assert_eq!(f[..8], [7, 6, 5, 4, 2, 121, 4, 62]);
        assert_eq!(f[8], 98, "tm_isdst is left alone");
        let (_, f) = run(&mut e, "03/04/21", "%D").unwrap();
        assert_eq!(f[3..6], [4, 2, 121]);
        let (_, f) = run(&mut e, "70", "%y").unwrap();
        assert_eq!(f[5], 70);
        let (_, f) = run(&mut e, "68", "%y").unwrap();
        assert_eq!(f[5], 168);
        let (_, f) = run(&mut e, "2000-02-29", "%F").unwrap();
        assert_eq!((f[3], f[4], f[6], f[7]), (29, 1, 2, 59), "a leap day on a Tuesday");
        let (_, f) = run(&mut e, "12:34:56", "%T").unwrap();
        assert_eq!(f[..3], [56, 34, 12]);
        let (_, f) = run(&mut e, "07:08", "%R").unwrap();
        assert_eq!(f[1..3], [8, 7]);
        // 12-hour clock.
        for (input, hour) in [("12:00:00 AM", 0), ("01:30:00 pm", 13), ("12:00:00 PM", 12), ("11:59:59 am", 11)] {
            let (_, f) = run(&mut e, input, "%r").unwrap();
            assert_eq!(f[2], hour, "{input}");
        }
        let (_, f) = run(&mut e, "9 PM", "%l %p").unwrap();
        assert_eq!(f[2], 21);
        let (_, f) = run(&mut e, "pm", "%P").unwrap();
        assert_eq!(f[2], 92, "no 12-hour field: the hour is untouched");
        // Names.
        let (_, f) = run(&mut e, "Thu, 04 March 2021", "%a, %d %B %Y").unwrap();
        assert_eq!((f[3], f[4], f[5], f[6]), (4, 2, 121, 4));
        let (_, f) = run(&mut e, "friday Dec 25 2020", "%A %b %e %Y").unwrap();
        assert_eq!((f[4], f[6]), (11, 5));
        let (n, f) = run(&mut e, "may", "%h").unwrap();
        assert_eq!((n, f[4]), (3, 4));
        let (_, f) = run(&mut e, "Sat Jan  2 03:04:05 2021", "%c").unwrap();
        assert_eq!((f[3], f[4], f[2], f[6]), (2, 0, 3, 6));
        let (_, f) = run(&mut e, "11:22:33", "%X").unwrap();
        assert_eq!(f[..3], [33, 22, 11]);
        let (_, f) = run(&mut e, "01/02/03", "%x").unwrap();
        assert_eq!(f[3..6], [2, 0, 103]);
        // Day of the year fills in the date.
        let (_, f) = run(&mut e, "2021 060", "%Y %j").unwrap();
        assert_eq!((f[3], f[4], f[7]), (1, 2, 59), "day 60 of 2021 is March 1");
        let (_, f) = run(&mut e, "060", "%j").unwrap();
        assert_eq!((f[4], f[3], f[7]), (2, 1, 59), "the year field (1995) decides");
        // Literals, blanks, %%, modifiers, zones.
        let (n, _) = run(&mut e, "  x 5 %", "%n x%t%d %%").unwrap();
        assert_eq!(n, 7);
        let (_, f) = run(&mut e, "2021", "%EY").unwrap();
        assert_eq!(f[5], 121);
        let (n, _) = run(&mut e, "10:00 UTC rest", "%H:%M %Z").unwrap();
        assert_eq!(n, 9);
        for (input, used) in [("+0530", 5), ("-05:30", 6), ("+01", 3), ("Z", 1), (" +0100", 6)] {
            assert_eq!(run(&mut e, input, "%z").unwrap().0, used, "{input}");
        }
        // Format ends early: only what matched is consumed.
        assert_eq!(run(&mut e, "12 34", "%H").unwrap().0, 2);
    }

    #[test]
    fn rejects_text_that_does_not_match() {
        let mut e = Env::new();
        for (input, fmt) in [
            ("", "%Y"),
            ("abc", "%Y"),
            ("13", "%m"),
            ("0", "%m"),
            ("32", "%d"),
            ("24", "%H"),
            ("0", "%I"),
            ("60", "%M"),
            ("62", "%S"),
            ("367", "%j"),
            ("xyz", "%a"),
            ("Jan", "%A"),
            ("Ma", "%b"),
            ("xm", "%p"),
            ("p", "%p"),
            ("", "%p"),
            ("a", "b"),
            ("5", "%"),
            ("5", "%q"),
            ("5", "%E"),
            ("x", "%z"),
            ("+x", "%z"),
            ("+24", "%z"),
            ("+0160", "%z"),
            ("1 x", "%d %%"),
            ("2021", "%5Y%d"),
        ] {
            assert_eq!(run(&mut e, input, fmt), None, "{input:?} {fmt:?}");
        }
        // The pointers are checked.
        e.put_str(IN, "1");
        e.put_str(FMT, "%d");
        assert!(matches!(e.try_call("strptime", &[BAD, FMT, TM]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("strptime", &[IN, BAD, TM]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("strptime", &[IN, FMT, BAD]), Err(Stop::Fault(_))));
        // A tm that is only readable, not writable, at its tail.
        let end = SCRATCH + 0x10000;
        assert!(matches!(e.try_call("strptime", &[IN, FMT, end - 28]), Err(Stop::Fault(_))));
    }
}

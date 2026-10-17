//! `strftime` in the C locale.

use crate::timefns::days_from_civil;

const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Broken-down time as in `struct tm`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tm {
    pub sec: i32,
    pub min: i32,
    pub hour: i32,
    pub mday: i32,
    /// 0..=11
    pub mon: i32,
    /// Years since 1900.
    pub year: i32,
    /// 0 = Sunday.
    pub wday: i32,
    /// 0..=365
    pub yday: i32,
    pub gmtoff: i32,
    pub zone: String,
}

fn pick(table: &'static [&'static str], i: i32) -> &'static str {
    usize::try_from(i).ok().and_then(|i| table.get(i)).copied().unwrap_or("?")
}

/// ISO 8601 `(year, week)` of `t`.
fn iso_week(t: &Tm) -> (i64, i32) {
    let year = i64::from(t.year) + 1900;
    // Monday = 0 .. Sunday = 6
    let wd = (t.wday + 6) % 7;
    let week = (t.yday - wd + 10) / 7;
    let weeks_in = |y: i64| -> i32 {
        let jan1 = (days_from_civil(y, 1, 1) + 4).rem_euclid(7); // 0 = Sunday
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        // A year has 53 ISO weeks if it starts on Thursday, or on Wednesday in a leap year.
        if jan1 == 4 || (leap && jan1 == 3) {
            53
        } else {
            52
        }
    };
    if week < 1 {
        (year - 1, weeks_in(year - 1))
    } else if week > weeks_in(year) {
        (year + 1, 1)
    } else {
        (year, week)
    }
}

fn hour12(h: i32) -> i32 {
    match h % 12 {
        0 => 12,
        x => x,
    }
}

/// Seconds since the epoch for `t` taken as UTC (`%s`).
fn epoch(t: &Tm) -> i64 {
    let days = days_from_civil(i64::from(t.year) + 1900, i64::from(t.mon) + 1, i64::from(t.mday));
    days * 86_400 + i64::from(t.hour) * 3600 + i64::from(t.min) * 60 + i64::from(t.sec) - i64::from(t.gmtoff)
}

/// Format `t` according to `fmt`.
pub fn format_time(fmt: &[u8], t: &Tm) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < fmt.len() {
        if fmt[i] != b'%' {
            out.push(fmt[i]);
            i += 1;
            continue;
        }
        i += 1;
        // `E` and `O` modifiers select alternative representations; the C locale has none.
        while matches!(fmt.get(i), Some(b'E' | b'O')) {
            i += 1;
        }
        let Some(&conv) = fmt.get(i) else {
            out.push(b'%');
            break;
        };
        i += 1;
        let year = i64::from(t.year) + 1900;
        let s: String = match conv {
            b'a' => pick(&["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"], t.wday).to_string(),
            b'A' => pick(&DAYS, t.wday).to_string(),
            b'b' | b'h' => {
                pick(&["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"], t.mon)
                    .to_string()
            }
            b'B' => pick(&MONTHS, t.mon).to_string(),
            b'c' => String::from_utf8_lossy(&format_time(b"%a %b %e %H:%M:%S %Y", t)).into_owned(),
            b'C' => format!("{:02}", year.div_euclid(100)),
            b'd' => format!("{:02}", t.mday),
            b'D' | b'x' => String::from_utf8_lossy(&format_time(b"%m/%d/%y", t)).into_owned(),
            b'e' => format!("{:2}", t.mday),
            b'F' => String::from_utf8_lossy(&format_time(b"%Y-%m-%d", t)).into_owned(),
            b'g' => format!("{:02}", iso_week(t).0.rem_euclid(100)),
            b'G' => iso_week(t).0.to_string(),
            b'H' => format!("{:02}", t.hour),
            b'I' => format!("{:02}", hour12(t.hour)),
            b'j' => format!("{:03}", t.yday + 1),
            b'k' => format!("{:2}", t.hour),
            b'l' => format!("{:2}", hour12(t.hour)),
            b'm' => format!("{:02}", t.mon + 1),
            b'M' => format!("{:02}", t.min),
            b'n' => "\n".to_string(),
            b'p' => (if t.hour >= 12 { "PM" } else { "AM" }).to_string(),
            b'P' => (if t.hour >= 12 { "pm" } else { "am" }).to_string(),
            b'r' => String::from_utf8_lossy(&format_time(b"%I:%M:%S %p", t)).into_owned(),
            b'R' => String::from_utf8_lossy(&format_time(b"%H:%M", t)).into_owned(),
            b's' => epoch(t).to_string(),
            b'S' => format!("{:02}", t.sec),
            b't' => "\t".to_string(),
            b'T' | b'X' => String::from_utf8_lossy(&format_time(b"%H:%M:%S", t)).into_owned(),
            b'u' => (if t.wday == 0 { 7 } else { t.wday }).to_string(),
            b'U' => format!("{:02}", (t.yday + 7 - t.wday) / 7),
            b'V' => format!("{:02}", iso_week(t).1),
            b'w' => t.wday.to_string(),
            b'W' => format!("{:02}", (t.yday + 7 - (t.wday + 6) % 7) / 7),
            b'y' => format!("{:02}", year.rem_euclid(100)),
            b'Y' => year.to_string(),
            b'z' => {
                let (sign, a) = if t.gmtoff < 0 { ('-', -t.gmtoff) } else { ('+', t.gmtoff) };
                format!("{sign}{:02}{:02}", a / 3600, a / 60 % 60)
            }
            b'Z' => t.zone.clone(),
            b'%' => "%".to_string(),
            other => format!("%{}", other as char),
        };
        out.extend_from_slice(s.as_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timefns::break_down;

    fn tm(t: i64) -> Tm {
        let w = break_down(t);
        Tm {
            sec: w[0],
            min: w[1],
            hour: w[2],
            mday: w[3],
            mon: w[4],
            year: w[5],
            wday: w[6],
            yday: w[7],
            gmtoff: 0,
            zone: "UTC".into(),
        }
    }

    fn f(fmt: &str, t: &Tm) -> String {
        String::from_utf8(format_time(fmt.as_bytes(), t)).unwrap()
    }

    /// 2021-03-04 05:06:07 UTC, a Thursday, day 62 of the year.
    const T: i64 = 1_614_834_367;

    #[test]
    fn conversions_in_the_c_locale() {
        let t = tm(T);
        for (fmt, want) in [
            ("%a %A %b %h %B", "Thu Thursday Mar Mar March"),
            ("%c", "Thu Mar  4 05:06:07 2021"),
            ("%C %y %Y", "20 21 2021"),
            ("%d|%e|%j", "04| 4|063"),
            ("%D|%x|%F", "03/04/21|03/04/21|2021-03-04"),
            ("%H|%I|%k|%l|%M|%S", "05|05| 5| 5|06|07"),
            ("%p|%P|%r", "AM|am|05:06:07 AM"),
            ("%R|%T|%X", "05:06|05:06:07|05:06:07"),
            ("%m|%u|%w", "03|4|4"),
            ("%U|%W|%V|%g|%G", "09|09|09|21|2021"),
            ("%s", "1614834367"),
            ("%z|%Z", "+0000|UTC"),
            ("%n%t%%", "\n\t%"),
            ("plain text", "plain text"),
            ("%Q", "%Q"),
            ("%Ey %Od", "21 04"),
            ("100%", "100%"),
        ] {
            assert_eq!(f(fmt, &t), want, "{fmt}");
        }
    }

    #[test]
    fn afternoon_midnight_and_zones() {
        let mut t = tm(1_614_834_367 + 12 * 3600); // 17:06:07
        assert_eq!(f("%I %p %l", &t), "05 PM  5");
        t.hour = 0;
        assert_eq!(f("%I %p", &t), "12 AM");
        t.hour = 12;
        assert_eq!(f("%I %p", &t), "12 PM");
        t.gmtoff = -(5 * 3600 + 30 * 60);
        assert_eq!(f("%z", &t), "-0530");
        t.gmtoff = 3600;
        assert_eq!(f("%z", &t), "+0100");
        // %s honours the offset.
        let t = Tm { gmtoff: 3600, ..tm(T) };
        assert_eq!(f("%s", &t), (T - 3600).to_string());
        // Out-of-range fields print a placeholder.
        let bad = Tm { wday: 9, mon: -1, ..tm(T) };
        assert_eq!(f("%a %A %b %B", &bad), "? ? ? ?");
    }

    #[test]
    fn iso_weeks_cross_year_boundaries() {
        // (timestamp, expected "%G-W%V-%u")
        for (date, want) in [
            ((2021, 1, 1), "2020-W53-5"),   // Friday: belongs to the last week of 2020
            ((2018, 12, 31), "2019-W01-1"), // Monday: first week of 2019
            ((2020, 12, 31), "2020-W53-4"),
            ((2021, 1, 4), "2021-W01-1"),
            ((2015, 12, 31), "2015-W53-4"), // 2015 starts on a Thursday: 53 weeks
            ((2016, 1, 3), "2015-W53-7"),
            ((2016, 1, 4), "2016-W01-1"),
            ((2012, 12, 31), "2013-W01-1"),
            ((2004, 12, 31), "2004-W53-5"), // leap year starting on Thursday
            ((2008, 12, 29), "2009-W01-1"),
            ((2008, 12, 28), "2008-W52-7"), // 2008 starts on Tuesday: only 52 weeks
            ((2011, 1, 2), "2010-W52-7"),
            ((2000, 1, 1), "1999-W52-6"),
            ((2021, 7, 14), "2021-W28-3"),
        ] {
            let days = crate::timefns::days_from_civil(date.0, date.1, date.2);
            assert_eq!(f("%G-W%V-%u", &tm(days * 86_400 + 43_200)), want, "{date:?}");
        }
        // Sunday-first and Monday-first week numbers.
        let sun = tm(crate::timefns::days_from_civil(2017, 1, 1) * 86_400);
        assert_eq!((f("%U", &sun), f("%W", &sun)), ("01".to_string(), "00".to_string()));
    }
}

//! A bionic-compatible `printf` formatting engine (BSD `vfprintf` semantics).
//!
//! The engine is pure: arguments come from a [`FmtSource`], so the same code serves register
//! varargs, `va_list` pointers and unit tests. Deliberate bionic behaviours:
//! * `%p` prints `0x` plus lowercase hex, also for NULL (`0x0`), where glibc prints `(nil)`;
//! * `%s` with a NULL pointer prints `(null)`;
//! * `Inf` / `NaN` are spelled that way (`INF` / `NAN` for the upper-case conversions);
//! * the `0` flag zero-pads every conversion except inf/nan, strings included (BSD behaviour);
//! * `%n` is refused ([`FmtError::PercentN`]); an unknown conversion prints the character itself.

/// Longest output (and widest field) the engine will produce.
pub const MAX_OUT: usize = 1 << 24;

/// Where conversion arguments come from.
pub trait FmtSource {
    /// Next 32-bit argument.
    fn u32(&mut self) -> u32;
    /// Next 64-bit integer argument (8-byte aligned).
    fn u64(&mut self) -> u64;
    /// Next `double` argument.
    fn f64(&mut self) -> f64;
    /// NUL-terminated byte string at `addr` (`None` if unreadable).
    fn cstr(&mut self, addr: u32) -> Option<Vec<u8>>;
    /// NUL-terminated `wchar_t` string at `addr` (`None` if unreadable).
    fn wstr(&mut self, addr: u32) -> Option<Vec<u32>>;
}

/// Why formatting failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FmtError {
    /// The format used `%n`, which is refused.
    PercentN,
    /// A `%s` argument pointed at unreadable memory.
    Fault,
    /// A width, precision or the output exceeds [`MAX_OUT`].
    TooLong,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Len {
    None,
    H,
    HH,
    L,
    LL,
    J,
    Z,
    T,
    BigL,
}

#[derive(Default)]
struct Spec {
    minus: bool,
    plus: bool,
    space: bool,
    alt: bool,
    zero: bool,
    width: usize,
    prec: Option<usize>,
}

/// Format `fmt` with arguments from `src`.
pub fn format(fmt: &[u8], src: &mut dyn FmtSource) -> Result<Vec<u8>, FmtError> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < fmt.len() {
        if fmt[i] != b'%' {
            out.push(fmt[i]);
            i += 1;
            continue;
        }
        i += 1;
        let mut sp = Spec::default();
        while let Some(&f) = fmt.get(i) {
            match f {
                b'-' => sp.minus = true,
                b'+' => sp.plus = true,
                b' ' => sp.space = true,
                b'#' => sp.alt = true,
                b'0' => sp.zero = true,
                _ => break,
            }
            i += 1;
        }
        if fmt.get(i) == Some(&b'*') {
            let w = src.u32() as i32;
            sp.minus |= w < 0;
            sp.width = (w as i64).unsigned_abs() as usize;
            i += 1;
        } else {
            sp.width = digits(fmt, &mut i);
        }
        if fmt.get(i) == Some(&b'.') {
            i += 1;
            if fmt.get(i) == Some(&b'*') {
                let p = src.u32() as i32;
                sp.prec = (p >= 0).then_some(p as usize);
                i += 1;
            } else {
                sp.prec = Some(digits(fmt, &mut i));
            }
        }
        if sp.width > MAX_OUT || sp.prec.unwrap_or(0) > MAX_OUT {
            return Err(FmtError::TooLong);
        }
        let mut len = Len::None;
        loop {
            len = match (fmt.get(i), len) {
                (Some(b'h'), Len::None) => Len::H,
                (Some(b'h'), Len::H) => Len::HH,
                (Some(b'l'), Len::None) => Len::L,
                (Some(b'l'), Len::L) => Len::LL,
                (Some(b'q'), _) => Len::LL,
                (Some(b'j'), _) => Len::J,
                (Some(b'z'), _) => Len::Z,
                (Some(b't'), _) => Len::T,
                (Some(b'L'), _) => Len::BigL,
                _ => break,
            };
            i += 1;
        }
        let Some(&conv) = fmt.get(i) else { break };
        i += 1;
        match conv {
            b'%' => out.push(b'%'),
            b'n' => return Err(FmtError::PercentN),
            b'd' | b'i' => {
                let v = match len {
                    Len::LL | Len::J => src.u64() as i64,
                    Len::HH => src.u32() as i8 as i64,
                    Len::H => src.u32() as i16 as i64,
                    _ => src.u32() as i32 as i64,
                };
                int(&mut out, &sp, v < 0, v.unsigned_abs(), 10, false, true);
            }
            b'u' | b'o' | b'x' | b'X' => {
                let v = match len {
                    Len::LL | Len::J => src.u64(),
                    Len::HH => src.u32() as u8 as u64,
                    Len::H => src.u32() as u16 as u64,
                    _ => src.u32() as u64,
                };
                let base = match conv {
                    b'u' => 10,
                    b'o' => 8,
                    _ => 16,
                };
                int(&mut out, &sp, false, v, base, conv == b'X', false);
            }
            b'p' => {
                let digits = int_digits(src.u32() as u64, 16, false, sp.prec);
                finish(&mut out, &sp, b"0x", &digits, sp.prec.is_none());
            }
            b'c' => {
                let v = src.u32();
                let bytes = if len == Len::L { wide_to_utf8(&[v]) } else { vec![v as u8] };
                finish(&mut out, &sp, b"", &bytes, true);
            }
            b's' => {
                let addr = src.u32();
                let mut bytes = if addr == 0 {
                    b"(null)".to_vec()
                } else if len == Len::L {
                    wide_to_utf8(&src.wstr(addr).ok_or(FmtError::Fault)?)
                } else {
                    src.cstr(addr).ok_or(FmtError::Fault)?
                };
                if let Some(p) = sp.prec {
                    bytes.truncate(p);
                }
                finish(&mut out, &sp, b"", &bytes, true);
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' => {
                let v = src.f64();
                float(&mut out, &sp, conv, v);
            }
            b'\0' => break,
            other => out.push(other),
        }
        if out.len() > MAX_OUT {
            return Err(FmtError::TooLong);
        }
    }
    Ok(out)
}

fn digits(fmt: &[u8], i: &mut usize) -> usize {
    let mut n = 0usize;
    while let Some(d) = fmt.get(*i).filter(|d| d.is_ascii_digit()) {
        n = n.saturating_mul(10).saturating_add((d - b'0') as usize);
        *i += 1;
    }
    n
}

fn wide_to_utf8(w: &[u32]) -> Vec<u8> {
    let mut s = String::new();
    for c in w {
        s.push(char::from_u32(*c).unwrap_or('?'));
    }
    s.into_bytes()
}

/// Digits of `v` in `base`, honouring a precision (minimum digit count; 0 with precision 0 is empty).
fn int_digits(v: u64, base: u64, upper: bool, prec: Option<usize>) -> Vec<u8> {
    let mut d = Vec::new();
    let mut x = v;
    if v == 0 && prec != Some(0) {
        d.push(b'0');
    }
    while x > 0 {
        let r = (x % base) as u8;
        d.push(match r {
            0..=9 => b'0' + r,
            _ if upper => b'A' + r - 10,
            _ => b'a' + r - 10,
        });
        x /= base;
    }
    if let Some(p) = prec {
        while d.len() < p {
            d.push(b'0');
        }
    }
    d.reverse();
    d
}

fn int(out: &mut Vec<u8>, sp: &Spec, neg: bool, mag: u64, base: u64, upper: bool, signed: bool) {
    let mut digits = int_digits(mag, base, upper, sp.prec);
    let mut prefix: &[u8] = b"";
    if signed {
        prefix = if neg {
            b"-"
        } else if sp.plus {
            b"+"
        } else if sp.space {
            b" "
        } else {
            b""
        };
    } else if base == 16 && sp.alt && mag != 0 {
        prefix = if upper { b"0X" } else { b"0x" };
    } else if base == 8 && sp.alt && digits.first() != Some(&b'0') {
        digits.insert(0, b'0');
    }
    // An explicit precision disables the `0` flag for integers.
    let zero_ok = sp.prec.is_none();
    finish(out, sp, prefix, &digits, zero_ok);
}

/// Emit `prefix` + `body` padded to `sp.width`: spaces left/right, or zeros between prefix and body.
fn finish(out: &mut Vec<u8>, sp: &Spec, prefix: &[u8], body: &[u8], zero_ok: bool) {
    let total = prefix.len() + body.len();
    let pad = sp.width.saturating_sub(total);
    if sp.minus {
        out.extend_from_slice(prefix);
        out.extend_from_slice(body);
        out.resize(out.len() + pad, b' ');
    } else if sp.zero && zero_ok {
        out.extend_from_slice(prefix);
        out.resize(out.len() + pad, b'0');
        out.extend_from_slice(body);
    } else {
        out.resize(out.len() + pad, b' ');
        out.extend_from_slice(prefix);
        out.extend_from_slice(body);
    }
}

fn float(out: &mut Vec<u8>, sp: &Spec, conv: u8, v: f64) {
    let upper = conv.is_ascii_uppercase();
    let neg = v.is_sign_negative() && !v.is_nan();
    let a = v.abs();
    let finite = v.is_finite();
    let body = if v.is_infinite() {
        (if upper { "INF" } else { "Inf" }).to_string()
    } else if v.is_nan() {
        (if upper { "NAN" } else { "NaN" }).to_string()
    } else {
        match conv.to_ascii_lowercase() {
            b'f' => fixed(a, sp.prec.unwrap_or(6), sp.alt),
            b'e' => exponent(a, sp.prec.unwrap_or(6), sp.alt, upper),
            _ => general(a, sp.prec.unwrap_or(6), sp.alt, upper),
        }
    };
    let sign: &[u8] = if neg {
        b"-"
    } else if sp.plus {
        b"+"
    } else if sp.space {
        b" "
    } else {
        b""
    };
    finish(out, sp, sign, body.as_bytes(), finite);
}

fn fixed(a: f64, prec: usize, alt: bool) -> String {
    let mut s = format!("{a:.prec$}");
    if alt && prec == 0 {
        s.push('.');
    }
    s
}

/// `d.ddde+XX` with at least two exponent digits.
fn exponent(a: f64, prec: usize, alt: bool, upper: bool) -> String {
    let s = format!("{a:.prec$e}");
    let (mant, exp) = s.split_once('e').expect("rust formats `e` conversions with an exponent");
    let exp: i32 = exp.parse().expect("rust exponents are integers");
    let dot = if alt && prec == 0 { "." } else { "" };
    let e = if upper { 'E' } else { 'e' };
    format!("{mant}{dot}{e}{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
}

fn general(a: f64, prec: usize, alt: bool, upper: bool) -> String {
    let p = prec.max(1);
    // The exponent after rounding to P significant digits decides the style.
    let probe = format!("{a:.*e}", p - 1);
    let x: i32 = probe.split_once('e').expect("exponent").1.parse().expect("integer exponent");
    let x = if a == 0.0 { 0 } else { x };
    let s = if x >= -4 && (x as i64) < p as i64 {
        fixed(a, (p as i64 - 1 - x as i64) as usize, alt)
    } else {
        exponent(a, p - 1, alt, upper)
    };
    if alt {
        return s;
    }
    // Strip trailing zeros of the mantissa (and a dangling point).
    let (mant, tail) = match s.find(['e', 'E']) {
        Some(k) => s.split_at(k),
        None => (s.as_str(), ""),
    };
    let mant = if mant.contains('.') { mant.trim_end_matches('0').trim_end_matches('.') } else { mant };
    format!("{mant}{tail}")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Argument queue for tests.
    #[derive(Clone, Debug)]
    pub enum A {
        U(u32),
        Q(u64),
        D(f64),
    }
    pub struct Src {
        pub args: std::collections::VecDeque<A>,
        pub strs: HashMap<u32, Vec<u8>>,
        pub wstrs: HashMap<u32, Vec<u32>>,
    }
    impl Src {
        pub fn new(args: Vec<A>) -> Src {
            Src { args: args.into(), strs: HashMap::new(), wstrs: HashMap::new() }
        }
    }
    impl FmtSource for Src {
        fn u32(&mut self) -> u32 {
            let Some(A::U(v)) = self.args.pop_front() else { panic!("expected u32") };
            v
        }
        fn u64(&mut self) -> u64 {
            let Some(A::Q(v)) = self.args.pop_front() else { panic!("expected u64") };
            v
        }
        fn f64(&mut self) -> f64 {
            let Some(A::D(v)) = self.args.pop_front() else { panic!("expected f64") };
            v
        }
        fn cstr(&mut self, addr: u32) -> Option<Vec<u8>> {
            self.strs.get(&addr).cloned()
        }
        fn wstr(&mut self, addr: u32) -> Option<Vec<u32>> {
            self.wstrs.get(&addr).cloned()
        }
    }

    fn run(f: &str, args: Vec<A>) -> String {
        let mut s = Src::new(args);
        s.strs.insert(1, b"abc".to_vec());
        s.strs.insert(2, b"hello world".to_vec());
        String::from_utf8(format(f.as_bytes(), &mut s).unwrap()).unwrap()
    }

    use A::{D, Q, U};

    /// Expected bionic output, one row per format.
    #[test]
    fn bionic_expected_output_table() {
        let table: Vec<(&str, Vec<A>, &str)> = vec![
            ("plain", vec![], "plain"),
            ("100%%", vec![], "100%"),
            ("%d", vec![U(0)], "0"),
            ("%d", vec![U(-42i32 as u32)], "-42"),
            ("%+d % d", vec![U(5), U(5)], "+5  5"),
            ("%5d|%-5d|%05d", vec![U(42), U(42), U(42)], "   42|42   |00042"),
            ("%05d", vec![U(-42i32 as u32)], "-0042"),
            ("%.5d|%8.5d|%-8.5d|", vec![U(42), U(42), U(42)], "00042|   00042|00042   |"),
            ("%.0d|%.0d", vec![U(0), U(1)], "|1"),
            ("%hhd %hd", vec![U(300), U(70000)], "44 4464"),
            ("%hhu %hu", vec![U(300), U(70000)], "44 4464"),
            ("%u", vec![U(u32::MAX)], "4294967295"),
            ("%ld %lu", vec![U(-1i32 as u32), U(-1i32 as u32)], "-1 4294967295"),
            ("%zu %td", vec![U(7), U(-7i32 as u32)], "7 -7"),
            ("%lld", vec![Q(i64::MIN as u64)], "-9223372036854775808"),
            ("%qd", vec![Q(5)], "5"),
            ("%llu", vec![Q(u64::MAX)], "18446744073709551615"),
            ("%jd %llx", vec![Q(-5i64 as u64), Q(0xdead_beef_cafe)], "-5 deadbeefcafe"),
            ("%x %X %#x %#X", vec![U(255), U(255), U(255), U(255)], "ff FF 0xff 0XFF"),
            ("%#x", vec![U(0)], "0"),
            ("%#010x|%-#6x|", vec![U(255), U(255)], "0x000000ff|0xff  |"),
            ("%o %#o %#o", vec![U(8), U(8), U(0)], "10 010 0"),
            ("%#.3o", vec![U(8)], "010"),
            ("%c%c", vec![U('o' as u32), U('k' as u32)], "ok"),
            ("%3c|%-3c|", vec![U('a' as u32), U('b' as u32)], "  a|b  |"),
            ("%s", vec![U(1)], "abc"),
            ("%5s|%-5s|%.2s", vec![U(1), U(1), U(2)], "  abc|abc  |he"),
            ("%.*s", vec![U(3), U(2)], "hel"),
            ("%*d|%-*d|", vec![U(4), U(7), U(4), U(7)], "   7|7   |"),
            ("%*d", vec![U(-4i32 as u32), U(7)], "7   "),
            ("%.*d", vec![U(-1i32 as u32), U(7)], "7"),
            ("%s", vec![U(0)], "(null)"),
            ("%p", vec![U(0)], "0x0"),
            ("%p", vec![U(0x1234abcd)], "0x1234abcd"),
            ("%10p|%-10p|", vec![U(0x10), U(0x10)], "      0x10|0x10      |"),
            ("%05s", vec![U(1)], "00abc"),
            ("%f", vec![D(3.25)], "3.250000"),
            ("%.2f", vec![D(2.675)], "2.67"),
            ("%.0f %.0f %.0f", vec![D(0.5), D(1.5), D(2.5)], "0 2 2"),
            ("%#.0f", vec![D(3.0)], "3."),
            ("%8.3f|%-8.3f|%08.3f", vec![D(-1.5), D(-1.5), D(-1.5)], "  -1.500|-1.500  |-001.500"),
            ("%+f % f", vec![D(1.0), D(1.0)], "+1.000000  1.000000"),
            ("%f", vec![D(-0.0)], "-0.000000"),
            ("%f", vec![D(1e20)], "100000000000000000000.000000"),
            ("%e", vec![D(12345.678)], "1.234568e+04"),
            ("%E", vec![D(0.000123)], "1.230000E-04"),
            ("%.0e|%#.0e", vec![D(5.0), D(5.0)], "5e+00|5.e+00"),
            ("%e", vec![D(0.0)], "0.000000e+00"),
            ("%e", vec![D(1e100)], "1.000000e+100"),
            ("%g", vec![D(100000.0)], "100000"),
            ("%g", vec![D(1000000.0)], "1e+06"),
            ("%g", vec![D(0.0001)], "0.0001"),
            ("%g", vec![D(0.00001)], "1e-05"),
            ("%g %g", vec![D(0.5), D(123.456)], "0.5 123.456"),
            ("%g", vec![D(0.0)], "0"),
            ("%.3g", vec![D(1234.5)], "1.23e+03"),
            ("%.10g", vec![D(1234.5)], "1234.5"),
            ("%.0g", vec![D(7.0)], "7"),
            ("%#g", vec![D(1.5)], "1.50000"),
            ("%G", vec![D(1e-10)], "1E-10"),
            ("%g", vec![D(99999.95)], "99999.9"),
            ("%g", vec![D(999999.5)], "1e+06"),
            ("%f|%e|%g", vec![D(f64::INFINITY), D(f64::NEG_INFINITY), D(f64::INFINITY)], "Inf|-Inf|Inf"),
            ("%f|%F|%G", vec![D(f64::NAN), D(f64::NAN), D(f64::INFINITY)], "NaN|NAN|INF"),
            ("%08f", vec![D(f64::INFINITY)], "     Inf"),
            ("%lf %Lf", vec![D(1.5), D(2.5)], "1.500000 2.500000"),
            ("%y", vec![], "y"),
            ("trailing %", vec![], "trailing "),
            ("%5", vec![], ""),
            ("%d%%", vec![U(5)], "5%"),
        ];
        for (f, args, want) in table {
            assert_eq!(run(f, args.clone()), want, "format {f:?} args {args:?}");
        }
    }

    #[test]
    fn nul_char_and_wide_conversions() {
        let mut s = Src::new(vec![U(0), U(0x20ac), U(7), U(0)]);
        s.wstrs.insert(7, vec!['h' as u32, 0xe9, 0x110000]);
        let out = format(b"%c|%lc|%ls|%ls", &mut s).unwrap();
        assert_eq!(out, "\0|\u{20ac}|h\u{e9}?|(null)".as_bytes());
        let mut s = Src::new(vec![U(9)]);
        assert_eq!(format(b"%s", &mut s), Err(FmtError::Fault));
        let mut s = Src::new(vec![U(9)]);
        assert_eq!(format(b"%ls", &mut s), Err(FmtError::Fault));
    }

    #[test]
    fn percent_n_is_refused_and_limits_apply() {
        let mut s = Src::new(vec![U(0)]);
        assert_eq!(format(b"abc%n", &mut s), Err(FmtError::PercentN));
        assert_eq!(format(b"%99999999d", &mut Src::new(vec![U(0)])), Err(FmtError::TooLong));
        assert_eq!(format(b"%.99999999f", &mut Src::new(vec![D(0.0)])), Err(FmtError::TooLong));
        // Many small conversions that together exceed the cap.
        let big = "%.1000000d".repeat(20);
        let mut s = Src::new((0..20).map(|_| U(1)).collect());
        assert_eq!(format(big.as_bytes(), &mut s), Err(FmtError::TooLong));
    }

    #[test]
    fn nul_after_percent_stops_output() {
        let mut s = Src::new(vec![]);
        assert_eq!(format(b"a%\0b", &mut s).unwrap(), b"a");
    }

    // ---- differential tests against the host libc (glibc/musl) for conversions where the two agree ----

    fn host_i(f: &str, a: i32) -> String {
        let c = std::ffi::CString::new(f).unwrap();
        let mut buf = vec![0u8; 512];
        let n = unsafe { libc::snprintf(buf.as_mut_ptr().cast(), 512, c.as_ptr(), a) };
        String::from_utf8(buf[..n as usize].to_vec()).unwrap()
    }
    fn host_q(f: &str, a: i64) -> String {
        let c = std::ffi::CString::new(f).unwrap();
        let mut buf = vec![0u8; 512];
        let n = unsafe { libc::snprintf(buf.as_mut_ptr().cast(), 512, c.as_ptr(), a) };
        String::from_utf8(buf[..n as usize].to_vec()).unwrap()
    }
    fn host_d(f: &str, a: f64) -> String {
        let c = std::ffi::CString::new(f).unwrap();
        let mut buf = vec![0u8; 2048];
        let n = unsafe { libc::snprintf(buf.as_mut_ptr().cast(), 2048, c.as_ptr(), a) };
        String::from_utf8(buf[..n as usize].to_vec()).unwrap()
    }
    fn host_s(f: &str, a: &str) -> String {
        let c = std::ffi::CString::new(f).unwrap();
        let a = std::ffi::CString::new(a).unwrap();
        let mut buf = vec![0u8; 512];
        let n = unsafe { libc::snprintf(buf.as_mut_ptr().cast(), 512, c.as_ptr(), a.as_ptr()) };
        String::from_utf8(buf[..n as usize].to_vec()).unwrap()
    }

    #[test]
    fn differential_integers_against_host() {
        let ints = [0, 1, -1, 7, -7, 42, 255, 65535, 123456789, i32::MAX, i32::MIN];
        let specs = [
            "%d", "%i", "%5d", "%-5d|", "%05d", "%+d", "% d", "%.3d", "%8.3d", "%-8.3d|", "%x", "%X", "%#x", "%#X",
            "%08x", "%#010x", "%o", "%#o", "%u", "%10u", "%-10u|", "%.0d", "%+.5d", "%hhd", "%hd", "%hhu", "%hu",
            "%hhx",
        ];
        for f in specs {
            for v in ints {
                let mut s = Src::new(vec![U(v as u32)]);
                let got = String::from_utf8(format(f.as_bytes(), &mut s).unwrap()).unwrap();
                assert_eq!(got, host_i(f, v), "{f} {v}");
            }
        }
        let big = [0i64, 1, -1, i64::MAX, i64::MIN, 0x1_0000_0000, -0x1_0000_0000, 1234567890123];
        for f in
            ["%lld", "%llu", "%llx", "%llX", "%#llx", "%20lld", "%-20lld|", "%020lld", "%llo", "%.25lld", "%jd", "%ju"]
        {
            for v in big {
                let mut s = Src::new(vec![Q(v as u64)]);
                let got = String::from_utf8(format(f.as_bytes(), &mut s).unwrap()).unwrap();
                assert_eq!(got, host_q(f, v), "{f} {v}");
            }
        }
    }

    #[test]
    fn differential_floats_against_host() {
        let vals = [
            0.0,
            -0.0,
            1.0,
            -1.0,
            0.5,
            1.5,
            2.5,
            0.1,
            0.2,
            0.3,
            1.0 / 3.0,
            2.0 / 3.0,
            3.2515926,
            2.675,
            1e-5,
            1e-4,
            9.9999e-5,
            123456.0,
            1234567.0,
            99999.95,
            999999.5,
            1e15,
            1e16,
            1e21,
            1e100,
            1e-100,
            1.7976931348623157e308,
            5e-324,
            12345.678901234,
            0.000123456789,
            100.0,
            0.99999999,
            9.5,
            0.05,
        ];
        let specs = [
            "%f", "%.0f", "%.1f", "%.2f", "%.10f", "%#.0f", "%10.3f|", "%-10.3f|", "%010.3f", "%+f", "% f", "%e",
            "%.0e", "%#.0e", "%.3e", "%E", "%12.4e|", "%g", "%.0g", "%.1g", "%.3g", "%.10g", "%#g", "%#.3g", "%G",
            "%12g|", "%-12g|", "%012g", "%+g", "%.15g", "%.17g", "%.20f",
        ];
        for f in specs {
            for v in vals {
                // glibc drops the zeros of a rounded-up `%#g` ("1.e+06"); C and BSD keep them.
                if f.contains('#') && f.ends_with('g') && v == 999999.5 {
                    continue;
                }
                let mut s = Src::new(vec![D(v)]);
                let got = String::from_utf8(format(f.as_bytes(), &mut s).unwrap()).unwrap();
                assert_eq!(got, host_d(f, v), "{f} {v:e}");
            }
        }
    }

    #[test]
    fn differential_strings_against_host() {
        for f in ["%s", "%10s|", "%-10s|", "%.3s", "%10.3s|", "%-10.3s|", "%.0s", "%.20s"] {
            for v in ["", "a", "hello", "hello world, this is long"] {
                let mut s = Src::new(vec![U(1)]);
                s.strs.insert(1, v.as_bytes().to_vec());
                let got = String::from_utf8(format(f.as_bytes(), &mut s).unwrap()).unwrap();
                assert_eq!(got, host_s(f, v), "{f} {v}");
            }
        }
    }
}

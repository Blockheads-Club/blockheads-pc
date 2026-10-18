//! The `scanf` engine behind `sscanf`, `vsscanf`, `fscanf`, `vfscanf` and `scanf`: C99 conversions
//! (`d i u o x X p`, `e f g E G a`, `s c [set] n %`), assignment suppression (`*`), field widths and the length
//! modifiers `hh h l ll L j z t`. The engine is independent of where the text comes from ([`ScanHost`]).

/// What the engine needs from its caller.
pub(crate) trait ScanHost {
    /// The next input byte without consuming it; `None` at end of input.
    fn peek(&mut self) -> Option<u8>;
    /// Consume the byte returned by `peek`.
    fn bump(&mut self);
    /// The next pointer argument (a `va_list` or register/stack slot).
    fn arg_ptr(&mut self) -> u32;
    /// Store a converted value in the guest; false once the guest has been stopped.
    fn store(&mut self, addr: u32, bytes: &[u8]) -> bool;
    /// Store the low `size` bytes of `v` through the next pointer argument.
    fn store_int(&mut self, size: u32, v: u64) -> bool {
        let p = self.arg_ptr();
        self.store(p, &v.to_le_bytes()[..size as usize])
    }
}

/// Why a conversion stopped the scan.
enum Fail {
    /// Input ran out.
    Eof,
    /// Input did not match.
    Mismatch,
    /// A guest store failed (the guest is already stopped).
    Store,
}

/// Parsed `%` specification.
struct Spec {
    suppress: bool,
    width: usize,
    /// Size in bytes of the destination integer (hh = 1, h = 2, none/l/z/t = 4, ll/j = 8).
    int_size: u32,
    /// `l`/`L`: a double rather than a float.
    long_float: bool,
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

fn digit_value(b: u8) -> Option<u32> {
    char::from(b).to_digit(36)
}

/// Run `fmt` against the input. Returns the number of assigned conversions, or `-1` (`EOF`) if the input ended
/// before the first conversion completed.
pub(crate) fn scan<H: ScanHost>(h: &mut H, fmt: &[u8]) -> i32 {
    let mut assigned = 0;
    let mut consumed = 0usize;
    let mut i = 0;
    let outcome = loop {
        let Some(&f) = fmt.get(i) else { break Ok(()) };
        i += 1;
        if is_space(f) {
            while h.peek().is_some_and(is_space) {
                h.bump();
                consumed += 1;
            }
            continue;
        }
        if f != b'%' {
            match h.peek() {
                None => break Err(Fail::Eof),
                Some(b) if b == f => {
                    h.bump();
                    consumed += 1;
                    continue;
                }
                Some(_) => break Err(Fail::Mismatch),
            }
        }
        let (spec, conv, next) = parse_spec(fmt, i);
        i = next;
        let Some(conv) = conv else { break Ok(()) };
        if conv == b'%' {
            skip_space(h, &mut consumed);
            match h.peek() {
                None => break Err(Fail::Eof),
                Some(b'%') => {
                    h.bump();
                    consumed += 1;
                    continue;
                }
                Some(_) => break Err(Fail::Mismatch),
            }
        }
        if conv == b'n' {
            if !spec.suppress && !h.store_int(spec.int_size, consumed as u64) {
                break Err(Fail::Store);
            }
            continue;
        }
        let set = (conv == b'[').then(|| parse_set(fmt, &mut i));
        let r = match conv {
            b'd' | b'i' | b'u' | b'o' | b'x' | b'X' | b'p' => integer(h, &spec, conv, &mut consumed),
            b'e' | b'E' | b'f' | b'F' | b'g' | b'G' | b'a' | b'A' => float(h, &spec, &mut consumed),
            b's' => text(h, &spec, TextKind::Word, &mut consumed),
            b'c' => text(h, &spec, TextKind::Chars, &mut consumed),
            b'[' => text(h, &spec, TextKind::Set(set.unwrap_or_default()), &mut consumed),
            _ => Err(Fail::Mismatch),
        };
        match r {
            Ok(()) => assigned += i32::from(!spec.suppress),
            Err(e) => break Err(e),
        }
    };
    match outcome {
        Err(Fail::Eof) if assigned == 0 => -1,
        _ => assigned,
    }
}

fn skip_space<H: ScanHost>(h: &mut H, consumed: &mut usize) {
    while h.peek().is_some_and(is_space) {
        h.bump();
        *consumed += 1;
    }
}

/// Parse the part of a conversion after `%`: returns the spec, the conversion character and the index after it.
fn parse_spec(fmt: &[u8], mut i: usize) -> (Spec, Option<u8>, usize) {
    let mut spec = Spec { suppress: false, width: usize::MAX, int_size: 4, long_float: false };
    if fmt.get(i) == Some(&b'*') {
        spec.suppress = true;
        i += 1;
    }
    let digits_end = fmt[i..].iter().position(|b| !b.is_ascii_digit()).map_or(fmt.len(), |p| i + p);
    if digits_end > i {
        spec.width = std::str::from_utf8(&fmt[i..digits_end]).ok().and_then(|d| d.parse().ok()).unwrap_or(usize::MAX);
        i = digits_end;
    }
    let (mut hs, mut ls, mut big) = (0, 0, false);
    loop {
        match fmt.get(i) {
            Some(b'h') => hs += 1,
            Some(b'l') => ls += 1,
            Some(b'L' | b'j') => big = true,
            Some(b'z' | b't') => {}
            _ => break,
        }
        i += 1;
    }
    spec.int_size = match (big || ls >= 2, hs) {
        (true, _) => 8,
        (false, 0) => 4,
        (false, 1) => 2,
        (false, _) => 1,
    };
    spec.long_float = ls >= 1 || big;
    (spec, fmt.get(i).copied(), i + 1)
}

/// Parse a `[set]` after the `[`: returns a 256-entry membership table. `i` ends just after the `]`.
fn parse_set(fmt: &[u8], i: &mut usize) -> Vec<bool> {
    let negate = fmt.get(*i) == Some(&b'^');
    if negate {
        *i += 1;
    }
    let mut member = vec![false; 256];
    let mut first = true;
    while let Some(&b) = fmt.get(*i) {
        if b == b']' && !first {
            break;
        }
        first = false;
        if fmt.get(*i + 1) == Some(&b'-') && fmt.get(*i + 2).is_some_and(|e| *e != b']') {
            let end = fmt[*i + 2];
            for x in b.min(end)..=b.max(end) {
                member[usize::from(x)] = true;
            }
            *i += 3;
        } else {
            member[usize::from(b)] = true;
            *i += 1;
        }
    }
    *i += 1; // the closing bracket (or the end of the format)
    if negate {
        member.iter_mut().for_each(|m| *m = !*m);
    }
    member
}

/// Consume the byte `peek` returned, counting it against the field width.
fn take<H: ScanHost>(h: &mut H, consumed: &mut usize, width: &mut usize) -> u8 {
    let b = h.peek().unwrap_or(0);
    h.bump();
    *consumed += 1;
    *width -= 1;
    b
}

fn integer<H: ScanHost>(h: &mut H, spec: &Spec, conv: u8, consumed: &mut usize) -> Result<(), Fail> {
    skip_space(h, consumed);
    let mut width = spec.width;
    let mut negative = false;
    match h.peek() {
        None => return Err(Fail::Eof),
        Some(b @ (b'+' | b'-')) if width > 0 => {
            negative = b == b'-';
            take(h, consumed, &mut width);
        }
        Some(_) => {}
    }
    let mut base = match conv {
        b'o' => 8,
        b'x' | b'X' | b'p' => 16,
        b'i' => 0,
        _ => 10,
    };
    let mut digits = 0;
    if matches!(base, 0 | 16) && width > 0 && h.peek() == Some(b'0') {
        take(h, consumed, &mut width);
        digits = 1;
        if width > 0 && matches!(h.peek(), Some(b'x' | b'X')) {
            take(h, consumed, &mut width);
            base = 16;
            digits = 0;
        } else if base == 0 {
            base = 8;
        }
    }
    let base = if base == 0 { 10 } else { base };
    let mut value = 0u64;
    while width > 0 {
        let Some(d) = h.peek().and_then(digit_value).filter(|d| *d < base) else { break };
        value = value.wrapping_mul(u64::from(base)).wrapping_add(u64::from(d));
        take(h, consumed, &mut width);
        digits += 1;
    }
    if digits == 0 {
        return Err(no_match(h));
    }
    if negative {
        value = value.wrapping_neg();
    }
    stored(spec.suppress || h.store_int(spec.int_size, value))
}

/// The failure for a conversion that could not start or finish: end of input, or a mismatch.
fn no_match<H: ScanHost>(h: &mut H) -> Fail {
    if h.peek().is_none() {
        Fail::Eof
    } else {
        Fail::Mismatch
    }
}

fn stored(ok: bool) -> Result<(), Fail> {
    if ok {
        Ok(())
    } else {
        Err(Fail::Store)
    }
}

/// Consume `word` (lower case) case-insensitively from the input, as far as it matches.
fn eat_word<H: ScanHost>(h: &mut H, word: &[u8], width: &mut usize, consumed: &mut usize) -> bool {
    for w in word {
        if *width == 0 || h.peek().map(|b| b.to_ascii_lowercase()) != Some(*w) {
            return false;
        }
        h.bump();
        *consumed += 1;
        *width -= 1;
    }
    true
}

fn float<H: ScanHost>(h: &mut H, spec: &Spec, consumed: &mut usize) -> Result<(), Fail> {
    skip_space(h, consumed);
    if h.peek().is_none() {
        return Err(Fail::Eof);
    }
    let mut width = spec.width;
    let mut text = String::new();
    if matches!(h.peek(), Some(b'+' | b'-')) && width > 0 {
        text.push(char::from(take(h, consumed, &mut width)));
    }
    let special = match h.peek().map(|b| b.to_ascii_lowercase()) {
        Some(b'i') => Some(("inf", "inity")),
        Some(b'n') => Some(("nan", "")),
        _ => None,
    };
    if let Some((word, tail)) = special {
        if !eat_word(h, word.as_bytes(), &mut width, consumed) {
            return Err(Fail::Mismatch);
        }
        // "infinity": the rest of the word is optional, but all of it must be there if any is.
        let has_tail = !tail.is_empty() && h.peek().map(|b| b.to_ascii_lowercase()) == Some(tail.as_bytes()[0]);
        if has_tail && !eat_word(h, tail.as_bytes(), &mut width, consumed) {
            return Err(Fail::Mismatch);
        }
        text.push_str(word);
    } else {
        let mut digits = 0;
        digits += take_digits(h, consumed, &mut width, &mut text);
        if width > 0 && h.peek() == Some(b'.') {
            text.push(char::from(take(h, consumed, &mut width)));
            digits += take_digits(h, consumed, &mut width, &mut text);
        }
        if digits == 0 {
            return Err(no_match(h));
        }
        if width > 0 && matches!(h.peek(), Some(b'e' | b'E')) {
            text.push(char::from(take(h, consumed, &mut width)));
            if width > 0 && matches!(h.peek(), Some(b'+' | b'-')) {
                text.push(char::from(take(h, consumed, &mut width)));
            }
            if take_digits(h, consumed, &mut width, &mut text) == 0 {
                return Err(Fail::Mismatch);
            }
        }
    }
    let value: f64 = text.parse().unwrap_or(f64::NAN);
    if spec.suppress {
        return Ok(());
    }
    let p = h.arg_ptr();
    stored(if spec.long_float { h.store(p, &value.to_le_bytes()) } else { h.store(p, &(value as f32).to_le_bytes()) })
}

/// Append the run of decimal digits at the cursor (within the width) to `text`; returns how many.
fn take_digits<H: ScanHost>(h: &mut H, consumed: &mut usize, width: &mut usize, text: &mut String) -> usize {
    let mut n = 0;
    while *width > 0 && h.peek().is_some_and(|b| b.is_ascii_digit()) {
        text.push(char::from(take(h, consumed, width)));
        n += 1;
    }
    n
}

enum TextKind {
    /// `%s`: a run of non-space characters.
    Word,
    /// `%c`: exactly `width` (default 1) characters, no terminator.
    Chars,
    /// `%[...]`: a run of characters in the set.
    Set(Vec<bool>),
}

fn text<H: ScanHost>(h: &mut H, spec: &Spec, kind: TextKind, consumed: &mut usize) -> Result<(), Fail> {
    if !matches!(kind, TextKind::Chars | TextKind::Set(_)) {
        skip_space(h, consumed);
    }
    let (limit, terminated) = match kind {
        TextKind::Chars => (if spec.width == usize::MAX { 1 } else { spec.width }, false),
        _ => (spec.width, true),
    };
    let mut out = Vec::new();
    while out.len() < limit {
        let Some(b) = h.peek() else { break };
        let ok = match &kind {
            TextKind::Word => !is_space(b),
            TextKind::Chars => true,
            TextKind::Set(member) => member[usize::from(b)],
        };
        if !ok {
            break;
        }
        h.bump();
        *consumed += 1;
        out.push(b);
    }
    let complete = if matches!(kind, TextKind::Chars) { out.len() == limit } else { !out.is_empty() };
    if !complete {
        return Err(no_match(h));
    }
    if terminated {
        out.push(0);
    }
    if spec.suppress {
        return Ok(());
    }
    let p = h.arg_ptr();
    stored(h.store(p, &out))
}

#[cfg(test)]
mod tests {
    use crate::tests_env::{Env, SCRATCH};
    use bhpc_cpu::Stop;

    const IN: u32 = SCRATCH;
    const FMT: u32 = SCRATCH + 0x100;
    const OUT: u32 = SCRATCH + 0x400;
    const BAD: u32 = 0x10;
    const EOF: i32 = -1;

    /// `sscanf(input, fmt, OUT, OUT+0x40, OUT+0x80, ...)` with up to three result slots of 0x40 bytes each.
    fn scan(e: &mut Env, input: &str, fmt: &str) -> i32 {
        e.put_str(IN, input);
        e.put_str(FMT, fmt);
        e.mem().write_bytes(OUT, &[0xAA; 0x100]).unwrap();
        e.call("sscanf", &[IN, FMT, OUT, OUT + 0x40, OUT + 0x80, OUT + 0xC0]).r0 as i32
    }

    fn word(e: &Env, slot: u32) -> u32 {
        e.mem().read_u32(OUT + 0x40 * slot).unwrap()
    }

    fn text(e: &Env, slot: u32) -> String {
        e.get_str(OUT + 0x40 * slot)
    }

    #[test]
    fn integers_in_every_base_and_size() {
        let mut e = Env::new();
        assert_eq!(scan(&mut e, "12 -7", "%d %d"), 2);
        assert_eq!((word(&e, 0), word(&e, 1) as i32), (12, -7));
        assert_eq!(scan(&mut e, "+5", "%d"), 1);
        assert_eq!(word(&e, 0), 5);
        for (input, expect) in [("0x1F", 31), ("017", 15), ("42", 42), ("0", 0), ("-0x10", -16i32 as u32)] {
            assert_eq!(scan(&mut e, input, "%i"), 1, "{input}");
            assert_eq!(word(&e, 0), expect, "{input}");
        }
        assert_eq!(scan(&mut e, "08", "%i%c"), 2);
        assert_eq!((word(&e, 0), word(&e, 1) & 0xff), (0, '8' as u32), "octal 0 then the 8");
        for (input, fmt, expect) in
            [("ff", "%x", 255), ("FF", "%X", 255), ("0xff", "%x", 255), ("17", "%o", 15), ("0x10", "%p", 16)]
        {
            assert_eq!(scan(&mut e, input, fmt), 1, "{input} {fmt}");
            assert_eq!(word(&e, 0), expect, "{input} {fmt}");
        }
        assert_eq!(scan(&mut e, "-1", "%u"), 1);
        assert_eq!(word(&e, 0), u32::MAX);
        // Sizes: hh = 1 byte, h = 2, l = 4, ll and j = 8, z and t = 4.
        assert_eq!(scan(&mut e, "300", "%hhu"), 1);
        assert_eq!(e.mem().read_bytes(OUT, 2).unwrap(), [44, 0xAA]);
        assert_eq!(scan(&mut e, "70000", "%hu"), 1);
        assert_eq!(e.mem().read_bytes(OUT, 3).unwrap(), [0x70, 0x11, 0xAA]);
        assert_eq!(scan(&mut e, "5000000000", "%lld"), 1);
        assert_eq!(e.mem().read_u64(OUT).unwrap(), 5_000_000_000);
        assert_eq!(scan(&mut e, "-3", "%jd"), 1);
        assert_eq!(e.mem().read_u64(OUT).unwrap() as i64, -3);
        for fmt in ["%ld", "%zd", "%td"] {
            assert_eq!(scan(&mut e, "9", fmt), 1);
            assert_eq!(e.mem().read_bytes(OUT, 5).unwrap(), [9, 0, 0, 0, 0xAA], "{fmt}");
        }
        // Widths, suppression, %n.
        assert_eq!(scan(&mut e, "1234567", "%5d%d"), 2);
        assert_eq!((word(&e, 0), word(&e, 1)), (12345, 67));
        assert_eq!(scan(&mut e, "1 2", "%*d %d"), 1);
        assert_eq!(word(&e, 0), 2);
        assert_eq!(scan(&mut e, "ab 12", "ab %d%n"), 1);
        assert_eq!((word(&e, 0), word(&e, 1)), (12, 5));
        assert_eq!(scan(&mut e, "xyz", "xyz%n"), 0, "%n alone does not count");
        assert_eq!(word(&e, 0), 3);
        assert_eq!(scan(&mut e, "xyz", "xyz%*n"), 0, "suppressed %n stores nothing");
        assert_eq!(word(&e, 0), 0xAAAA_AAAA);
        // Failures.
        assert_eq!(scan(&mut e, "abc", "%d"), 0);
        assert_eq!(scan(&mut e, "-5", "%1d"), 0, "the sign used up the width");
        assert_eq!(scan(&mut e, "0xg", "%x"), 0, "a 0x prefix needs digits");
        assert_eq!(scan(&mut e, "-", "%d"), EOF);
        assert_eq!(scan(&mut e, "12", "%d%d"), 1);
        assert_eq!(scan(&mut e, "12 x", "%d %d"), 1);
    }

    #[test]
    fn floating_point() {
        let mut e = Env::new();
        let f32_at = |e: &Env, slot| f32::from_bits(word(e, slot));
        let f64_at = |e: &Env, slot: u32| e.mem().read_u64(OUT + 0x40 * slot).map(f64::from_bits).unwrap();
        assert_eq!(scan(&mut e, "3.5 -2", "%f %f"), 2);
        assert_eq!((f32_at(&e, 0), f32_at(&e, 1)), (3.5, -2.0));
        for (input, expect) in [("1e3", 1000.0), (".5", 0.5), ("5.", 5.0), ("-1.5E-1", -0.15), ("+7", 7.0)] {
            assert_eq!(scan(&mut e, input, "%lf"), 1, "{input}");
            assert_eq!(f64_at(&e, 0), expect, "{input}");
        }
        assert_eq!(scan(&mut e, "2.5", "%Lf"), 1);
        assert_eq!(f64_at(&e, 0), 2.5);
        assert_eq!(scan(&mut e, "1.2345", "%3lf"), 1);
        assert_eq!(f64_at(&e, 0), 1.2);
        for (input, check) in [("inf", f64::INFINITY), ("-Infinity", f64::NEG_INFINITY), ("INF", f64::INFINITY)] {
            assert_eq!(scan(&mut e, input, "%lf"), 1, "{input}");
            assert_eq!(f64_at(&e, 0), check, "{input}");
        }
        assert_eq!(scan(&mut e, "NaN", "%lg"), 1);
        assert!(f64_at(&e, 0).is_nan());
        assert_eq!(scan(&mut e, "infx", "%lf%c"), 2, "a partial 'infinity' leaves the rest");
        assert_eq!(scan(&mut e, "3.5", "%*f"), 0);
        for fmt in ["%e", "%E", "%g", "%G", "%a", "%A", "%F"] {
            assert_eq!(scan(&mut e, "1.25", fmt), 1, "{fmt}");
            assert_eq!(f32_at(&e, 0), 1.25, "{fmt}");
        }
        // Failures: no digits, bad exponent, partial words, end of input.
        assert_eq!(scan(&mut e, "e5", "%f"), 0);
        assert_eq!(scan(&mut e, ".", "%f"), EOF, "the input ends right after the dot");
        assert_eq!(scan(&mut e, ".x", "%f"), 0);
        assert_eq!(scan(&mut e, "1e", "%f"), 0);
        assert_eq!(scan(&mut e, "1e+", "%f"), 0);
        assert_eq!(scan(&mut e, "ind", "%f"), 0);
        assert_eq!(scan(&mut e, "infinit", "%f"), 0);
        assert_eq!(scan(&mut e, "nax", "%f"), 0);
        assert_eq!(scan(&mut e, "", "%f"), EOF);
        assert_eq!(scan(&mut e, "  ", "%f"), EOF);
        assert_eq!(scan(&mut e, "-", "%f"), EOF);
    }

    #[test]
    fn strings_characters_and_sets() {
        let mut e = Env::new();
        assert_eq!(scan(&mut e, "  hello world", "%s %s"), 2);
        assert_eq!((text(&e, 0), text(&e, 1)), ("hello".into(), "world".into()));
        assert_eq!(scan(&mut e, "abcdef", "%3s"), 1);
        assert_eq!(text(&e, 0), "abc");
        assert_eq!(scan(&mut e, "xy", "%c%c"), 2);
        assert_eq!(e.mem().read_bytes(OUT, 2).unwrap(), *b"x\xAA");
        assert_eq!(scan(&mut e, "abcd", "%3c"), 1);
        assert_eq!(e.mem().read_bytes(OUT, 4).unwrap(), *b"abc\xAA", "%c does not terminate");
        assert_eq!(scan(&mut e, " a", "%c"), 1, "%c does not skip blanks");
        assert_eq!(e.mem().read_bytes(OUT, 1).unwrap(), *b" ");
        assert_eq!(scan(&mut e, "abcd", "%[a-c]"), 1);
        assert_eq!(text(&e, 0), "abc");
        assert_eq!(scan(&mut e, "foo,5", "%[^,],%d"), 2);
        assert_eq!((text(&e, 0), word(&e, 1)), ("foo".into(), 5));
        assert_eq!(scan(&mut e, "]a]b", "%[]a]"), 1, "a leading ] is part of the set");
        assert_eq!(text(&e, 0), "]a]");
        assert_eq!(scan(&mut e, "a]b", "%[^]]"), 1);
        assert_eq!(text(&e, 0), "a");
        assert_eq!(scan(&mut e, "a-b", "%[a-]"), 1, "a trailing - is literal");
        assert_eq!(text(&e, 0), "a-");
        assert_eq!(scan(&mut e, "cba", "%[c-a]"), 1, "reversed ranges are normalised");
        assert_eq!(scan(&mut e, "abc", "%2[a-c]"), 1);
        assert_eq!(text(&e, 0), "ab");
        assert_eq!(scan(&mut e, "abc", "%[abc"), 1, "an unterminated set runs to the end of the format");
        assert_eq!(scan(&mut e, "ab", "%*s"), 0);
        assert_eq!(scan(&mut e, "ab", "%*[a]%c"), 1);
        // Failures.
        assert_eq!(scan(&mut e, "x", "%[a-c]"), 0);
        assert_eq!(scan(&mut e, "ab", "%3c"), EOF);
        assert_eq!(scan(&mut e, "", "%s"), EOF);
        assert_eq!(scan(&mut e, "  ", "%s"), EOF);
        assert_eq!(scan(&mut e, "", "%c"), EOF);
        assert_eq!(scan(&mut e, "", "%[a]"), EOF);
    }

    #[test]
    fn literals_whitespace_and_degenerate_formats() {
        let mut e = Env::new();
        assert_eq!(scan(&mut e, "x=5", "x=%d"), 1);
        assert_eq!(scan(&mut e, "y=5", "x=%d"), 0);
        assert_eq!(scan(&mut e, "1;2", "%d,%d"), 1);
        assert_eq!(scan(&mut e, "1,2", "%d,%d"), 2);
        assert_eq!(scan(&mut e, "   7", " %d"), 1);
        assert_eq!(scan(&mut e, "1 \t\n 2", "%d\n%d"), 2, "any format blank matches any run of blanks");
        assert_eq!(scan(&mut e, "  % 5", "%% %d"), 1);
        assert_eq!(scan(&mut e, "x", "%%"), 0);
        assert_eq!(scan(&mut e, "", "%%"), EOF);
        assert_eq!(scan(&mut e, "abc", ""), 0);
        assert_eq!(scan(&mut e, "", ""), 0);
        assert_eq!(scan(&mut e, "", "x"), EOF);
        assert_eq!(scan(&mut e, "5", "%d%"), 1, "a lone % ends the scan");
        assert_eq!(scan(&mut e, "5", "%q"), 0, "unknown conversion");
        assert_eq!(scan(&mut e, "5", "%5"), 0);
        assert_eq!(scan(&mut e, "5", "%99999999999999999999d"), 1, "absurd widths mean unlimited");
    }

    #[test]
    fn guest_memory_faults_stop_the_guest() {
        let mut e = Env::new();
        e.put_str(IN, "12 3.5 abc");
        for fmt in ["%d", "%f", "%s", "%n", "%c"] {
            e.put_str(FMT, fmt);
            assert!(matches!(e.try_call("sscanf", &[IN, FMT, BAD]), Err(Stop::Fault(_))), "{fmt}");
        }
        e.put_str(FMT, "%d");
        assert!(matches!(e.try_call("sscanf", &[BAD, FMT, OUT]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("sscanf", &[IN, BAD, OUT]), Err(Stop::Fault(_))));
        e.put_str(FMT, "%lf");
        assert!(matches!(e.try_call("sscanf", &[IN, FMT, BAD]), Err(Stop::Fault(_))));
    }

    #[test]
    fn va_list_forms() {
        let mut e = Env::new();
        e.put_str(IN, "7 abc");
        e.put_str(FMT, "%d %s");
        let ap = SCRATCH + 0x300;
        e.mem().write_u32(ap, OUT).unwrap();
        e.mem().write_u32(ap + 4, OUT + 0x40).unwrap();
        assert_eq!(e.call("vsscanf", &[IN, FMT, ap]).r0, 2);
        assert_eq!((word(&e, 0), text(&e, 1)), (7, "abc".into()));
        // vscanf / vfscanf on stdin: end of file.
        let sf = e.data("__sF");
        assert_eq!(e.call("vscanf", &[FMT, ap]).r0 as i32, EOF);
        assert_eq!(e.call("vfscanf", &[sf, FMT, ap]).r0 as i32, EOF);
        assert_eq!(e.call("scanf", &[FMT, OUT, OUT + 0x40]).r0 as i32, EOF);
        assert_eq!(e.call("fscanf", &[sf, FMT, OUT, OUT + 0x40]).r0 as i32, EOF);
        // Past the registers the arguments come from the stack.
        e.put_str(IN, "1 2 3 4 5");
        e.put_str(FMT, "%d %d %d %d %d");
        let slots: Vec<u32> = (0..5).map(|i| OUT + 4 * i).collect();
        let mut args = vec![IN, FMT];
        args.extend(&slots);
        assert_eq!(e.call("sscanf", &args).r0, 5);
        assert_eq!((0..5).map(|i| e.mem().read_u32(OUT + 4 * i).unwrap()).collect::<Vec<_>>(), [1, 2, 3, 4, 5]);
    }
}

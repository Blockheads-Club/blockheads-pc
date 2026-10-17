//! `<string.h>`, `<stdlib.h>` number parsing, wide-character classification and the ARM EABI
//! `__aeabi_mem*` helpers.

use crate::guest::{cstr, cstr_n, rd, rd32, wr, wr32, wr_cstr};
use crate::layout::errno;
use crate::misc::set_errno;
use crate::{lock, Impl, Shared};
use bhpc_cpu::Cpu;

macro_rules! args {
    ($c:expr; $($n:ident),+) => {
        let mut a = $c.args();
        $(let $n = a.u32($c);)+
    };
}

// ---------- memory ----------

fn memcpy(c: &mut Cpu, _: &Shared) {
    args!(c; d, s, n);
    copy(c, d, s, n);
    c.ret_u32(d);
}

fn copy(c: &mut Cpu, d: u32, s: u32, n: u32) {
    if let Some(b) = rd(c, s, n) {
        wr(c, d, &b);
    }
}

/// `__aeabi_memcpy(dst, src, n)`: same as memcpy, no result.
fn aeabi_memcpy(c: &mut Cpu, _: &Shared) {
    args!(c; d, s, n);
    copy(c, d, s, n);
}

fn fill(c: &mut Cpu, d: u32, v: u8, n: u32) {
    if rd(c, d, n).is_some() {
        wr(c, d, &vec![v; n as usize]);
    }
}

fn memset(c: &mut Cpu, _: &Shared) {
    args!(c; d, v, n);
    fill(c, d, v as u8, n);
    c.ret_u32(d);
}

/// `__aeabi_memset(dst, n, c)`: note the swapped argument order.
fn aeabi_memset(c: &mut Cpu, _: &Shared) {
    args!(c; d, n, v);
    fill(c, d, v as u8, n);
}

/// `__aeabi_memclr(dst, n)`.
fn aeabi_memclr(c: &mut Cpu, _: &Shared) {
    args!(c; d, n);
    fill(c, d, 0, n);
}

fn bzero(c: &mut Cpu, _: &Shared) {
    args!(c; d, n);
    fill(c, d, 0, n);
}

/// `bcopy(src, dst, n)`.
fn bcopy(c: &mut Cpu, _: &Shared) {
    args!(c; s, d, n);
    copy(c, d, s, n);
}

fn memcmp(c: &mut Cpu, _: &Shared) {
    args!(c; x, y, n);
    let (Some(bx), Some(by)) = (rd(c, x, n), rd(c, y, n)) else { return };
    c.ret_u32(diff(&bx, &by));
}

fn diff(a: &[u8], b: &[u8]) -> u32 {
    a.iter().zip(b).find(|(p, q)| p != q).map_or(0, |(p, q)| (*p as i32 - *q as i32) as u32)
}

fn memchr(c: &mut Cpu, _: &Shared) {
    args!(c; p, v, n);
    let Some(b) = rd(c, p, n) else { return };
    c.ret_u32(b.iter().position(|x| *x == v as u8).map_or(0, |i| p + i as u32));
}

fn memrchr(c: &mut Cpu, _: &Shared) {
    args!(c; p, v, n);
    let Some(b) = rd(c, p, n) else { return };
    c.ret_u32(b.iter().rposition(|x| *x == v as u8).map_or(0, |i| p + i as u32));
}

fn memmem(c: &mut Cpu, _: &Shared) {
    args!(c; h, hl, n, nl);
    let (Some(hb), Some(nb)) = (rd(c, h, hl), rd(c, n, nl)) else { return };
    c.ret_u32(find(&hb, &nb).map_or(0, |i| h + i as u32));
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    if n.is_empty() {
        return Some(0);
    }
    h.windows(n.len()).position(|w| w == n)
}

// ---------- strings ----------

fn strlen(c: &mut Cpu, _: &Shared) {
    let p = c.args().u32(c);
    if let Some(s) = cstr(c, p) {
        c.ret_u32(s.len() as u32);
    }
}

fn strnlen(c: &mut Cpu, _: &Shared) {
    args!(c; p, n);
    if let Some((s, _)) = cstr_n(c, p, n) {
        c.ret_u32(s.len() as u32);
    }
}

fn strcmp(c: &mut Cpu, _: &Shared) {
    args!(c; x, y);
    let (Some(sx), Some(sy)) = (cstr(c, x), cstr(c, y)) else { return };
    c.ret_u32(str_diff(&sx, &sy, false));
}

fn strncmp(c: &mut Cpu, _: &Shared) {
    args!(c; x, y, n);
    let (Some((sx, _)), Some((sy, _))) = (cstr_n(c, x, n), cstr_n(c, y, n)) else { return };
    c.ret_u32(str_diff(&sx, &sy, false));
}

fn strcasecmp(c: &mut Cpu, _: &Shared) {
    args!(c; x, y);
    let (Some(sx), Some(sy)) = (cstr(c, x), cstr(c, y)) else { return };
    c.ret_u32(str_diff(&sx, &sy, true));
}

fn strncasecmp(c: &mut Cpu, _: &Shared) {
    args!(c; x, y, n);
    let (Some((sx, _)), Some((sy, _))) = (cstr_n(c, x, n), cstr_n(c, y, n)) else { return };
    c.ret_u32(str_diff(&sx, &sy, true));
}

/// Difference of the first differing bytes, comparing the implicit terminating NULs as well.
fn str_diff(a: &[u8], b: &[u8], fold: bool) -> u32 {
    let at = |s: &[u8], i: usize| {
        let v = s.get(i).copied().unwrap_or(0);
        if fold {
            v.to_ascii_lowercase()
        } else {
            v
        }
    };
    for i in 0..=a.len().max(b.len()) {
        let (p, q) = (at(a, i), at(b, i));
        if p != q {
            return (p as i32 - q as i32) as u32;
        }
    }
    0
}

fn strcpy(c: &mut Cpu, _: &Shared) {
    args!(c; d, s);
    let Some(b) = cstr(c, s) else { return };
    if wr_cstr(c, d, &b) {
        c.ret_u32(d);
    }
}

fn strncpy(c: &mut Cpu, _: &Shared) {
    args!(c; d, s, n);
    let Some((mut b, _)) = cstr_n(c, s, n) else { return };
    b.resize(n as usize, 0);
    if wr(c, d, &b) {
        c.ret_u32(d);
    }
}

fn strcat(c: &mut Cpu, _: &Shared) {
    args!(c; d, s);
    let (Some(db), Some(sb)) = (cstr(c, d), cstr(c, s)) else { return };
    if wr_cstr(c, d + db.len() as u32, &sb) {
        c.ret_u32(d);
    }
}

fn strncat(c: &mut Cpu, _: &Shared) {
    args!(c; d, s, n);
    let (Some(db), Some((sb, _))) = (cstr(c, d), cstr_n(c, s, n)) else { return };
    if wr_cstr(c, d + db.len() as u32, &sb) {
        c.ret_u32(d);
    }
}

fn strlcpy(c: &mut Cpu, _: &Shared) {
    args!(c; d, s, size);
    let Some(sb) = cstr(c, s) else { return };
    if size > 0 {
        let keep = sb.len().min(size as usize - 1);
        if !wr_cstr(c, d, &sb[..keep]) {
            return;
        }
    }
    c.ret_u32(sb.len() as u32);
}

fn strlcat(c: &mut Cpu, _: &Shared) {
    args!(c; d, s, size);
    let (Some((db, _)), Some(sb)) = (cstr_n(c, d, size), cstr(c, s)) else { return };
    let dlen = db.len() as u32;
    if dlen >= size {
        return c.ret_u32(size.wrapping_add(sb.len() as u32));
    }
    let room = (size - dlen - 1) as usize;
    if wr_cstr(c, d + dlen, &sb[..sb.len().min(room)]) {
        c.ret_u32(dlen + sb.len() as u32);
    }
}

fn strchr(c: &mut Cpu, _: &Shared) {
    args!(c; s, ch);
    let Some(b) = cstr(c, s) else { return };
    let ch = ch as u8;
    c.ret_u32(if ch == 0 { s + b.len() as u32 } else { b.iter().position(|x| *x == ch).map_or(0, |i| s + i as u32) });
}

fn strrchr(c: &mut Cpu, _: &Shared) {
    args!(c; s, ch);
    let Some(b) = cstr(c, s) else { return };
    let ch = ch as u8;
    c.ret_u32(if ch == 0 { s + b.len() as u32 } else { b.iter().rposition(|x| *x == ch).map_or(0, |i| s + i as u32) });
}

fn strstr(c: &mut Cpu, _: &Shared) {
    args!(c; h, n);
    let (Some(hb), Some(nb)) = (cstr(c, h), cstr(c, n)) else { return };
    c.ret_u32(find(&hb, &nb).map_or(0, |i| h + i as u32));
}

fn strcasestr(c: &mut Cpu, _: &Shared) {
    args!(c; h, n);
    let (Some(hb), Some(nb)) = (cstr(c, h), cstr(c, n)) else { return };
    let (hb, nb) = (hb.to_ascii_lowercase(), nb.to_ascii_lowercase());
    c.ret_u32(find(&hb, &nb).map_or(0, |i| h + i as u32));
}

fn strpbrk(c: &mut Cpu, _: &Shared) {
    args!(c; s, set);
    let (Some(sb), Some(setb)) = (cstr(c, s), cstr(c, set)) else { return };
    c.ret_u32(sb.iter().position(|x| setb.contains(x)).map_or(0, |i| s + i as u32));
}

fn strspn(c: &mut Cpu, _: &Shared) {
    args!(c; s, set);
    let (Some(sb), Some(setb)) = (cstr(c, s), cstr(c, set)) else { return };
    c.ret_u32(sb.iter().take_while(|x| setb.contains(x)).count() as u32);
}

fn strcspn(c: &mut Cpu, _: &Shared) {
    args!(c; s, set);
    let (Some(sb), Some(setb)) = (cstr(c, s), cstr(c, set)) else { return };
    c.ret_u32(sb.iter().take_while(|x| !setb.contains(x)).count() as u32);
}

/// `strxfrm` in the C locale is a bounded `strcpy`; returns the full length.
fn strxfrm(c: &mut Cpu, _: &Shared) {
    args!(c; d, s, n);
    let Some(sb) = cstr(c, s) else { return };
    if n > 0 && !wr_cstr(c, d, &sb[..sb.len().min(n as usize - 1)]) {
        return;
    }
    c.ret_u32(sb.len() as u32);
}

fn strtok_r(c: &mut Cpu, _: &Shared) {
    args!(c; s, delim, save);
    let Some(db) = cstr(c, delim) else { return };
    let start = if s != 0 {
        s
    } else {
        match rd32(c, save) {
            Some(p) => p,
            None => return,
        }
    };
    if start == 0 {
        return c.ret_u32(0);
    }
    let Some(b) = cstr(c, start) else { return };
    let skip = b.iter().take_while(|x| db.contains(x)).count();
    if skip == b.len() {
        if wr32(c, save, 0) {
            c.ret_u32(0);
        }
        return;
    }
    let tok = start + skip as u32;
    let len = b[skip..].iter().take_while(|x| !db.contains(x)).count();
    let end = tok + len as u32;
    let next = if skip + len < b.len() {
        wr(c, end, &[0]);
        end + 1
    } else {
        0
    };
    if wr32(c, save, next) {
        c.ret_u32(tok);
    }
}

// ---------- error / signal names ----------

const ERRORS: &[(u32, &str)] = &[
    (0, "Success"),
    (1, "Operation not permitted"),
    (2, "No such file or directory"),
    (3, "No such process"),
    (4, "Interrupted system call"),
    (5, "I/O error"),
    (9, "Bad file descriptor"),
    (11, "Try again"),
    (12, "Out of memory"),
    (13, "Permission denied"),
    (14, "Bad address"),
    (16, "Device or resource busy"),
    (17, "File exists"),
    (20, "Not a directory"),
    (21, "Is a directory"),
    (22, "Invalid argument"),
    (24, "Too many open files"),
    (28, "No space left on device"),
    (32, "Broken pipe"),
    (34, "Math result not representable"),
    (35, "Deadlock condition"),
    (38, "Function not implemented"),
    (95, "Operation not supported on transport endpoint"),
    (110, "Connection timed out"),
];

pub(crate) fn error_text(n: u32) -> String {
    match ERRORS.iter().find(|(k, _)| *k == n) {
        Some((_, s)) => s.to_string(),
        None => format!("Unknown error {}", n as i32),
    }
}

fn strerror(c: &mut Cpu, st: &Shared) {
    let n = c.args().u32(c);
    let cached = lock(st).strerror.get(&n).copied();
    let p = match cached {
        Some(p) => p,
        None => {
            let text = error_text(n);
            let mut s = lock(st);
            let p = s.heap.alloc(text.len() as u32 + 1);
            let mut v = text.into_bytes();
            v.push(0);
            s.mem.write_bytes(p, &v).expect("heap block is mapped");
            s.strerror.insert(n, p);
            p
        }
    };
    c.ret_u32(p);
}

/// XSI `strerror_r(errnum, buf, len)`: copies the message, `ERANGE` if it does not fit.
fn strerror_r(c: &mut Cpu, st: &Shared) {
    args!(c; n, buf, len);
    let text = error_text(n);
    if len as usize <= text.len() {
        if len > 0 && !wr_cstr(c, buf, &text.as_bytes()[..len as usize - 1]) {
            return;
        }
        set_errno(c, st, errno::ERANGE);
        return c.ret_u32(errno::ERANGE);
    }
    if wr_cstr(c, buf, text.as_bytes()) {
        c.ret_u32(0);
    }
}

fn strsignal(c: &mut Cpu, st: &Shared) {
    let n = c.args().u32(c);
    let name = match n {
        1 => "Hangup",
        2 => "Interrupt",
        3 => "Quit",
        4 => "Illegal instruction",
        6 => "Aborted",
        8 => "Floating point exception",
        9 => "Killed",
        11 => "Segmentation fault",
        13 => "Broken pipe",
        14 => "Alarm clock",
        15 => "Terminated",
        _ => "",
    };
    let text = if name.is_empty() { format!("Unknown signal {}", n as i32) } else { name.to_string() };
    let key = 0x8000_0000 | n;
    let cached = lock(st).strerror.get(&key).copied();
    let p = match cached {
        Some(p) => p,
        None => {
            let mut s = lock(st);
            let p = s.heap.alloc(text.len() as u32 + 1);
            let mut v = text.into_bytes();
            v.push(0);
            s.mem.write_bytes(p, &v).expect("heap block is mapped");
            s.strerror.insert(key, p);
            p
        }
    };
    c.ret_u32(p);
}

// ---------- number parsing ----------

/// Result of scanning an integer the way `strtol` does.
#[derive(Debug, PartialEq, Eq)]
pub struct Scan {
    /// Bytes consumed (0 if no conversion was possible).
    pub used: usize,
    pub neg: bool,
    /// Magnitude, saturated at `u64::MAX`.
    pub mag: u64,
    pub overflow: bool,
}

/// Scan `[ws][+-][0x]digits` in `base` (0 = auto). `None` for an invalid base.
pub fn scan_int(s: &[u8], base: u32) -> Option<Scan> {
    if base == 1 || base > 36 {
        return None;
    }
    let mut i = s.iter().take_while(|b| matches!(**b, b' ' | b'\t'..=b'\r')).count();
    let mut neg = false;
    if let Some(&sign @ (b'+' | b'-')) = s.get(i) {
        neg = sign == b'-';
        i += 1;
    }
    let mut base = base;
    let has_hex_prefix = s.get(i) == Some(&b'0')
        && matches!(s.get(i + 1), Some(b'x' | b'X'))
        && s.get(i + 2).is_some_and(|d| d.is_ascii_hexdigit());
    if (base == 0 || base == 16) && has_hex_prefix {
        i += 2;
        base = 16;
    } else if base == 0 {
        base = if s.get(i) == Some(&b'0') { 8 } else { 10 };
    }
    let start = i;
    let (mut mag, mut overflow) = (0u64, false);
    while let Some(d) = s.get(i).and_then(|b| (*b as char).to_digit(36)).filter(|d| *d < base) {
        match mag.checked_mul(base as u64).and_then(|m| m.checked_add(d as u64)) {
            Some(m) => mag = m,
            None => {
                overflow = true;
                mag = u64::MAX;
            }
        }
        i += 1;
    }
    Some(Scan { used: if i == start { 0 } else { i }, neg, mag, overflow })
}

/// Shared body of the `strto*` functions. `bits` is the result width, `signed` the signedness.
fn strto(c: &mut Cpu, st: &Shared, bits: u32, signed: bool) {
    args!(c; s, endp, base);
    let Some(b) = cstr(c, s) else { return };
    let Some(sc) = scan_int(&b, base) else {
        set_errno(c, st, errno::EINVAL);
        if endp != 0 && !wr32(c, endp, s) {
            return;
        }
        return ret_n(c, 0, bits);
    };
    if endp != 0 && !wr32(c, endp, s + sc.used as u32) {
        return;
    }
    let max = if signed { (1u64 << (bits - 1)) - 1 } else { u64::MAX >> (64 - bits) };
    let (v, range) = if signed {
        if sc.neg {
            let lim = max + 1;
            if sc.overflow || sc.mag > lim {
                (lim.wrapping_neg(), true)
            } else {
                (sc.mag.wrapping_neg(), false)
            }
        } else if sc.overflow || sc.mag > max {
            (max, true)
        } else {
            (sc.mag, false)
        }
    } else if sc.overflow || sc.mag > max {
        (max, true)
    } else if sc.neg {
        (sc.mag.wrapping_neg() & max, false)
    } else {
        (sc.mag, false)
    };
    if range {
        set_errno(c, st, errno::ERANGE);
    }
    ret_n(c, v, bits);
}

fn ret_n(c: &mut Cpu, v: u64, bits: u32) {
    if bits == 64 {
        c.ret_u64(v);
    } else {
        c.ret_u32(v as u32);
    }
}

fn strtol(c: &mut Cpu, st: &Shared) {
    strto(c, st, 32, true)
}
fn strtoul(c: &mut Cpu, st: &Shared) {
    strto(c, st, 32, false)
}
fn strtoll(c: &mut Cpu, st: &Shared) {
    strto(c, st, 64, true)
}
fn strtoull(c: &mut Cpu, st: &Shared) {
    strto(c, st, 64, false)
}

fn atoi(c: &mut Cpu, _: &Shared) {
    let s = c.args().u32(c);
    let Some(b) = cstr(c, s) else { return };
    let v = scan_int(&b, 10).map_or(0, |sc| if sc.neg { sc.mag.wrapping_neg() } else { sc.mag });
    c.ret_u32(v as u32);
}

// ---------- wide characters ----------

fn wchar_of(c: &mut Cpu) -> Option<char> {
    char::from_u32(c.args().u32(c))
}

fn iswalnum(c: &mut Cpu, _: &Shared) {
    let r = wchar_of(c).is_some_and(char::is_alphanumeric);
    c.ret_u32(r as u32);
}
fn iswdigit(c: &mut Cpu, _: &Shared) {
    let r = wchar_of(c).is_some_and(|ch| ch.is_ascii_digit());
    c.ret_u32(r as u32);
}
fn iswspace(c: &mut Cpu, _: &Shared) {
    let r = wchar_of(c).is_some_and(char::is_whitespace);
    c.ret_u32(r as u32);
}
fn iswxdigit(c: &mut Cpu, _: &Shared) {
    let r = wchar_of(c).is_some_and(|ch| ch.is_ascii_hexdigit());
    c.ret_u32(r as u32);
}
fn towlower(c: &mut Cpu, _: &Shared) {
    let w = c.args().u32(c);
    let r = char::from_u32(w).map_or(w, |ch| single(ch.to_lowercase(), ch) as u32);
    c.ret_u32(r);
}
fn towupper(c: &mut Cpu, _: &Shared) {
    let w = c.args().u32(c);
    let r = char::from_u32(w).map_or(w, |ch| single(ch.to_uppercase(), ch) as u32);
    c.ret_u32(r);
}

/// The mapped character if the case mapping is one-to-one, else the original.
fn single(mut it: impl Iterator<Item = char>, orig: char) -> char {
    match (it.next(), it.next()) {
        (Some(m), None) => m,
        _ => orig,
    }
}

// ---------- misc ----------

fn rand_next(c: &mut Cpu, st: &Shared) {
    let mut s = lock(st);
    s.rand48 = (s.rand48.wrapping_mul(0x5_DEEC_E66D).wrapping_add(0xB)) & 0xFFFF_FFFF_FFFF;
    let v = (s.rand48 >> 17) as u32;
    drop(s);
    c.ret_u32(v);
}

fn srand48(c: &mut Cpu, st: &Shared) {
    let seed = c.args().u32(c);
    lock(st).rand48 = (seed as u64) << 16 | 0x330E;
}

pub(crate) const TABLE: &[(&str, Impl)] = &[
    ("memcpy", memcpy),
    ("memmove", memcpy),
    ("memset", memset),
    ("memcmp", memcmp),
    ("memchr", memchr),
    ("memrchr", memrchr),
    ("memmem", memmem),
    ("bcopy", bcopy),
    ("bzero", bzero),
    ("__aeabi_memcpy", aeabi_memcpy),
    ("__aeabi_memcpy4", aeabi_memcpy),
    ("__aeabi_memcpy8", aeabi_memcpy),
    ("__aeabi_memmove", aeabi_memcpy),
    ("__aeabi_memmove4", aeabi_memcpy),
    ("__aeabi_memmove8", aeabi_memcpy),
    ("__aeabi_memset", aeabi_memset),
    ("__aeabi_memset4", aeabi_memset),
    ("__aeabi_memset8", aeabi_memset),
    ("__aeabi_memclr", aeabi_memclr),
    ("__aeabi_memclr4", aeabi_memclr),
    ("__aeabi_memclr8", aeabi_memclr),
    ("strlen", strlen),
    ("strnlen", strnlen),
    ("strcmp", strcmp),
    ("strncmp", strncmp),
    ("strcoll", strcmp),
    ("strcasecmp", strcasecmp),
    ("strncasecmp", strncasecmp),
    ("strcpy", strcpy),
    ("strncpy", strncpy),
    ("strcat", strcat),
    ("strncat", strncat),
    ("strlcpy", strlcpy),
    ("strlcat", strlcat),
    ("strchr", strchr),
    ("strrchr", strrchr),
    ("strstr", strstr),
    ("strcasestr", strcasestr),
    ("strpbrk", strpbrk),
    ("strspn", strspn),
    ("strcspn", strcspn),
    ("strxfrm", strxfrm),
    ("strtok_r", strtok_r),
    ("strerror", strerror),
    ("strerror_r", strerror_r),
    ("strsignal", strsignal),
    ("strtol", strtol),
    ("strtoul", strtoul),
    ("strtoll", strtoll),
    ("strtoull", strtoull),
    ("atoi", atoi),
    ("atol", atoi),
    ("iswalnum", iswalnum),
    ("iswdigit", iswdigit),
    ("iswspace", iswspace),
    ("iswxdigit", iswxdigit),
    ("towlower", towlower),
    ("towupper", towupper),
    ("lrand48", rand_next),
    ("srand48", srand48),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests_env::{Env, SCRATCH};
    use bhpc_cpu::Stop;

    const A: u32 = SCRATCH;
    const B: u32 = SCRATCH + 0x100;
    const C: u32 = SCRATCH + 0x200;
    const D: u32 = SCRATCH + 0x300;
    const BAD: u32 = 0x10;

    fn sgn(v: u32) -> i32 {
        (v as i32).signum()
    }

    #[test]
    fn memory_functions() {
        let mut e = Env::new();
        e.put_str(A, "abcdef");
        e.put_str(B, "abcxef");
        assert_eq!(e.call("memcpy", &[C, A, 7]).r0, C);
        assert_eq!(e.get_str(C), "abcdef");
        e.call("memmove", &[A + 1, A, 3]);
        assert_eq!(e.get_str(A), "aabcef");
        e.call("memset", &[A, 'z' as u32, 2]);
        assert_eq!(e.get_str(A), "zzbcef");
        assert_eq!(e.call("memcmp", &[C, B, 3]).r0, 0);
        assert_eq!(e.call("memcmp", &[C, B, 6]).r0 as i32, 'd' as i32 - 'x' as i32);
        assert_eq!(e.call("memchr", &[C, 'd' as u32, 6]).r0, C + 3);
        assert_eq!(e.call("memchr", &[C, 'z' as u32, 6]).r0, 0);
        e.put_str(D, "abcabc");
        assert_eq!(e.call("memrchr", &[D, 'b' as u32, 6]).r0, D + 4);
        assert_eq!(e.call("memrchr", &[D, 'z' as u32, 6]).r0, 0);
        e.put_str(A, "hello world");
        e.put_str(B, "o w");
        assert_eq!(e.call("memmem", &[A, 11, B, 3]).r0, A + 4);
        assert_eq!(e.call("memmem", &[A, 11, B, 0]).r0, A);
        assert_eq!(e.call("memmem", &[A, 3, B, 3]).r0, 0);
        e.put_str(A, "xyz");
        e.call("bcopy", &[A, C, 4]);
        assert_eq!(e.get_str(C), "xyz");
        e.call("bzero", &[C, 2]);
        assert_eq!(e.mem().read_bytes(C, 3).unwrap(), vec![0, 0, b'z']);
        for n in [
            "__aeabi_memcpy",
            "__aeabi_memcpy4",
            "__aeabi_memcpy8",
            "__aeabi_memmove",
            "__aeabi_memmove4",
            "__aeabi_memmove8",
        ] {
            e.mem().write_bytes(C, &[0; 4]).unwrap();
            e.call(n, &[C, A, 4]);
            assert_eq!(e.get_str(C), "xyz", "{n}");
        }
        for n in ["__aeabi_memset", "__aeabi_memset4", "__aeabi_memset8"] {
            e.call(n, &[C, 3, 'q' as u32]);
            assert_eq!(e.mem().read_bytes(C, 3).unwrap(), b"qqq", "{n}");
            e.call("__aeabi_memclr", &[C, 3]);
            e.call("__aeabi_memset", &[C, 3, 'q' as u32]);
        }
        for n in ["__aeabi_memclr", "__aeabi_memclr4", "__aeabi_memclr8"] {
            e.call("__aeabi_memset", &[C, 3, 'q' as u32]);
            e.call(n, &[C, 3]);
            assert_eq!(e.mem().read_bytes(C, 3).unwrap(), vec![0; 3], "{n}");
        }
    }

    #[test]
    fn memory_functions_fault_on_bad_pointers() {
        let mut e = Env::new();
        for (f, args) in [
            ("memcpy", vec![A, BAD, 4]),
            ("memcpy", vec![BAD, A, 4]),
            ("memset", vec![BAD, 1, 4]),
            ("memcmp", vec![BAD, A, 4]),
            ("memchr", vec![BAD, 1, 4]),
            ("memrchr", vec![BAD, 1, 4]),
            ("memmem", vec![BAD, 4, A, 1]),
            ("bzero", vec![BAD, 4]),
            ("__aeabi_memclr", vec![BAD, 4]),
        ] {
            assert!(matches!(e.try_call(f, &args), Err(Stop::Fault(_))), "{f}");
        }
    }

    #[test]
    fn string_comparison_and_length() {
        let mut e = Env::new();
        e.put_str(A, "abc");
        e.put_str(B, "abd");
        e.put_str(C, "ABC");
        assert_eq!(e.call("strlen", &[A]).r0, 3);
        assert_eq!(e.call("strnlen", &[A, 2]).r0, 2);
        assert_eq!(e.call("strnlen", &[A, 10]).r0, 3);
        assert_eq!(e.call("strcmp", &[A, A]).r0, 0);
        assert_eq!(sgn(e.call("strcmp", &[A, B]).r0), -1);
        assert_eq!(e.call("strcmp", &[B, A]).r0 as i32, 1);
        assert_eq!(sgn(e.call("strcmp", &[A, C]).r0), 1);
        e.put_str(D, "ab");
        assert_eq!(sgn(e.call("strcmp", &[A, D]).r0), 1);
        assert_eq!(sgn(e.call("strcmp", &[D, A]).r0), -1);
        assert_eq!(e.call("strcoll", &[A, A]).r0, 0);
        assert_eq!(e.call("strncmp", &[A, B, 2]).r0, 0);
        assert_eq!(sgn(e.call("strncmp", &[A, B, 3]).r0), -1);
        assert_eq!(e.call("strcasecmp", &[A, C]).r0, 0);
        assert_eq!(sgn(e.call("strcasecmp", &[B, C]).r0), 1);
        assert_eq!(e.call("strncasecmp", &[A, C, 3]).r0, 0);
        assert_eq!(sgn(e.call("strncasecmp", &[B, C, 3]).r0), 1);
        assert!(matches!(e.try_call("strlen", &[BAD]), Err(Stop::Fault(_))));
        for f in [
            "strcmp",
            "strcasecmp",
            "strstr",
            "strcasestr",
            "strpbrk",
            "strspn",
            "strcspn",
            "strcpy",
            "strcat",
            "strtok_r",
        ] {
            assert!(matches!(e.try_call(f, &[BAD, A, A]), Err(Stop::Fault(_))), "{f}");
            assert!(matches!(e.try_call(f, &[A, BAD, A]), Err(Stop::Fault(_))), "{f}");
        }
        for f in ["strncmp", "strncasecmp", "strncpy", "strncat", "strlcpy", "strlcat", "strxfrm"] {
            assert!(matches!(e.try_call(f, &[BAD, A, 4]), Err(Stop::Fault(_))), "{f}");
            assert!(matches!(e.try_call(f, &[A, BAD, 4]), Err(Stop::Fault(_))), "{f}");
        }
        for f in ["strchr", "strrchr", "strnlen", "strdup", "atoi", "strtol", "strtoull"] {
            assert!(matches!(e.try_call(f, &[BAD, 10, 10]), Err(Stop::Fault(_))), "{f}");
        }
    }

    #[test]
    fn string_copy_and_concat() {
        let mut e = Env::new();
        e.put_str(A, "hello");
        assert_eq!(e.call("strcpy", &[B, A]).r0, B);
        assert_eq!(e.get_str(B), "hello");
        e.mem().write_bytes(C, &[b'x'; 10]).unwrap();
        e.call("strncpy", &[C, A, 8]);
        assert_eq!(e.mem().read_bytes(C, 10).unwrap(), b"hello\0\0\0xx");
        e.call("strncpy", &[C, A, 3]);
        assert_eq!(e.mem().read_bytes(C, 6).unwrap(), b"hello\0");
        e.put_str(D, "foo");
        assert_eq!(e.call("strcat", &[D, A]).r0, D);
        assert_eq!(e.get_str(D), "foohello");
        e.call("strncat", &[D, A, 2]);
        assert_eq!(e.get_str(D), "foohellohe");
        e.call("strncat", &[D, A, 50]);
        assert_eq!(e.get_str(D), "foohellohehello");
        // strlcpy
        assert_eq!(e.call("strlcpy", &[B, A, 4]).r0, 5);
        assert_eq!(e.get_str(B), "hel");
        assert_eq!(e.call("strlcpy", &[B, A, 0]).r0, 5);
        assert_eq!(e.get_str(B), "hel");
        assert_eq!(e.call("strlcpy", &[B, A, 100]).r0, 5);
        assert_eq!(e.get_str(B), "hello");
        // strlcat
        e.put_str(B, "ab");
        assert_eq!(e.call("strlcat", &[B, A, 5]).r0, 7);
        assert_eq!(e.get_str(B), "abhe");
        assert_eq!(e.call("strlcat", &[B, A, 100]).r0, 9);
        assert_eq!(e.get_str(B), "abhehello");
        assert_eq!(e.call("strlcat", &[B, A, 3]).r0, 8, "size smaller than the existing string");
        assert_eq!(e.call("strlcat", &[B, A, 0]).r0, 5);
        // strxfrm
        assert_eq!(e.call("strxfrm", &[C, A, 4]).r0, 5);
        assert_eq!(e.get_str(C), "hel");
        assert_eq!(e.call("strxfrm", &[C, A, 0]).r0, 5);
    }

    #[test]
    fn string_search() {
        let mut e = Env::new();
        e.put_str(A, "hello world");
        assert_eq!(e.call("strchr", &[A, 'o' as u32]).r0, A + 4);
        assert_eq!(e.call("strchr", &[A, 'z' as u32]).r0, 0);
        assert_eq!(e.call("strchr", &[A, 0]).r0, A + 11);
        assert_eq!(e.call("strrchr", &[A, 'o' as u32]).r0, A + 7);
        assert_eq!(e.call("strrchr", &[A, 'z' as u32]).r0, 0);
        assert_eq!(e.call("strrchr", &[A, 0]).r0, A + 11);
        e.put_str(B, "wor");
        e.put_str(C, "WOR");
        e.put_str(D, "");
        assert_eq!(e.call("strstr", &[A, B]).r0, A + 6);
        assert_eq!(e.call("strstr", &[A, C]).r0, 0);
        assert_eq!(e.call("strstr", &[A, D]).r0, A);
        assert_eq!(e.call("strcasestr", &[A, C]).r0, A + 6);
        assert_eq!(e.call("strcasestr", &[A, D]).r0, A);
        e.put_str(B, "xyzw");
        assert_eq!(e.call("strpbrk", &[A, B]).r0, A + 6);
        assert_eq!(e.call("strpbrk", &[A, D]).r0, 0);
        e.put_str(B, "hel");
        assert_eq!(e.call("strspn", &[A, B]).r0, 4);
        e.put_str(B, "o");
        assert_eq!(e.call("strcspn", &[A, B]).r0, 4);
        assert_eq!(e.call("strcspn", &[A, D]).r0, 11);
    }

    #[test]
    fn tokenizer() {
        let mut e = Env::new();
        let (save, delim) = (SCRATCH + 0x800, SCRATCH + 0x810);
        e.put_str(A, ",,ab,,cd,");
        e.put_str(delim, ",");
        let t1 = e.call("strtok_r", &[A, delim, save]).r0;
        assert_eq!((t1, e.get_str(t1).as_str()), (A + 2, "ab"));
        let t2 = e.call("strtok_r", &[0, delim, save]).r0;
        assert_eq!(e.get_str(t2), "cd");
        assert_eq!(e.call("strtok_r", &[0, delim, save]).r0, 0);
        assert_eq!(e.call("strtok_r", &[0, delim, save]).r0, 0, "exhausted");
        // Token at the very end of the string.
        e.put_str(B, "x y");
        e.put_str(delim, " ");
        let t = e.call("strtok_r", &[B, delim, save]).r0;
        assert_eq!(e.get_str(t), "x");
        let t = e.call("strtok_r", &[0, delim, save]).r0;
        assert_eq!(e.get_str(t), "y");
        assert_eq!(e.call("strtok_r", &[0, delim, save]).r0, 0);
        // Unreadable saved pointer.
        assert!(matches!(e.try_call("strtok_r", &[0, delim, BAD]), Err(Stop::Fault(_))));
        // Write faults: save pointer unmapped / terminator in read-only... just the save slot.
        e.put_str(B, "x y");
        assert!(matches!(e.try_call("strtok_r", &[B, delim, BAD]), Err(Stop::Fault(_))));
        e.put_str(B, " ");
        assert!(matches!(e.try_call("strtok_r", &[B, delim, BAD]), Err(Stop::Fault(_))));
    }

    #[test]
    fn error_and_signal_names() {
        let mut e = Env::new();
        let p = e.call("strerror", &[2]).r0;
        assert_eq!(e.get_str(p), "No such file or directory");
        assert_eq!(e.call("strerror", &[2]).r0, p, "cached");
        let p = e.call("strerror", &[9999]).r0;
        assert_eq!(e.get_str(p), "Unknown error 9999");
        assert_eq!(e.call("strerror_r", &[22, A, 100]).r0, 0);
        assert_eq!(e.get_str(A), "Invalid argument");
        assert_eq!(e.call("strerror_r", &[22, A, 5]).r0, 34);
        assert_eq!(e.get_str(A), "Inva");
        assert_eq!(e.errno(), 34);
        assert_eq!(e.call("strerror_r", &[22, A, 0]).r0, 34);
        assert!(matches!(e.try_call("strerror_r", &[22, BAD, 100]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("strerror_r", &[22, BAD, 3]), Err(Stop::Fault(_))));
        let p = e.call("strsignal", &[11]).r0;
        assert_eq!(e.get_str(p), "Segmentation fault");
        assert_eq!(e.call("strsignal", &[11]).r0, p);
        let p = e.call("strsignal", &[99]).r0;
        assert_eq!(e.get_str(p), "Unknown signal 99");
        for (n, text) in [
            (1, "Hangup"),
            (2, "Interrupt"),
            (3, "Quit"),
            (4, "Illegal instruction"),
            (6, "Aborted"),
            (8, "Floating point exception"),
            (9, "Killed"),
            (13, "Broken pipe"),
            (14, "Alarm clock"),
            (15, "Terminated"),
        ] {
            let p = e.call("strsignal", &[n]).r0;
            assert_eq!(e.get_str(p), text);
        }
    }

    #[test]
    fn integer_scanning() {
        let sc = |s: &str, b| scan_int(s.as_bytes(), b).unwrap();
        assert_eq!(sc("  -42xyz", 10), Scan { used: 5, neg: true, mag: 42, overflow: false });
        assert_eq!(sc("+7", 10).mag, 7);
        assert_eq!(sc("0x1F", 0).mag, 31);
        assert_eq!(sc("0x1F", 16).mag, 31);
        assert_eq!(sc("1F", 16).mag, 31);
        assert_eq!(sc("0x", 0), Scan { used: 1, neg: false, mag: 0, overflow: false });
        assert_eq!(sc("0xg", 16).used, 1);
        assert_eq!(sc("017", 0).mag, 15);
        assert_eq!(sc("0", 0).mag, 0);
        assert_eq!(sc("99", 0).mag, 99);
        assert_eq!(sc("zz", 36).mag, 35 * 36 + 35);
        assert_eq!(sc("12", 2).used, 1);
        assert_eq!(sc("abc", 10).used, 0);
        assert_eq!(sc("", 10).used, 0);
        assert_eq!(sc("-", 10).used, 0);
        let big = sc("99999999999999999999999", 10);
        assert!(big.overflow);
        assert_eq!(big.mag, u64::MAX);
        assert_eq!(sc("18446744073709551615", 10).mag, u64::MAX);
        assert!(!sc("18446744073709551615", 10).overflow);
        assert!(scan_int(b"1", 1).is_none());
        assert!(scan_int(b"1", 37).is_none());
    }

    #[test]
    fn strto_family() {
        let mut e = Env::new();
        let endp = SCRATCH + 0x900;
        e.put_str(A, "  123abc");
        assert_eq!(e.call("strtol", &[A, endp, 10]).r0, 123);
        assert_eq!(e.mem().read_u32(endp).unwrap(), A + 5);
        assert_eq!(e.call("strtol", &[A, 0, 10]).r0, 123);
        e.put_str(A, "-2147483648");
        assert_eq!(e.call("strtol", &[A, endp, 10]).r0 as i32, i32::MIN);
        assert_eq!(e.errno(), 0);
        e.put_str(A, "-2147483649");
        assert_eq!(e.call("strtol", &[A, endp, 10]).r0 as i32, i32::MIN);
        assert_eq!(e.errno(), 34);
        e.set_errno(0);
        e.put_str(A, "2147483648");
        assert_eq!(e.call("strtol", &[A, endp, 10]).r0 as i32, i32::MAX);
        assert_eq!(e.errno(), 34);
        e.set_errno(0);
        e.put_str(A, "4294967295");
        assert_eq!(e.call("strtoul", &[A, endp, 10]).r0, u32::MAX);
        assert_eq!(e.errno(), 0);
        e.put_str(A, "-1");
        assert_eq!(e.call("strtoul", &[A, endp, 10]).r0, u32::MAX);
        e.put_str(A, "4294967296");
        assert_eq!(e.call("strtoul", &[A, endp, 10]).r0, u32::MAX);
        assert_eq!(e.errno(), 34);
        e.set_errno(0);
        e.put_str(A, "-9223372036854775808");
        assert_eq!(e.call("strtoll", &[A, endp, 10]).as_u64() as i64, i64::MIN);
        assert_eq!(e.errno(), 0);
        e.put_str(A, "-9223372036854775809");
        assert_eq!(e.call("strtoll", &[A, endp, 10]).as_u64() as i64, i64::MIN);
        assert_eq!(e.errno(), 34);
        e.set_errno(0);
        e.put_str(A, "9223372036854775808");
        assert_eq!(e.call("strtoll", &[A, endp, 10]).as_u64() as i64, i64::MAX);
        e.put_str(A, "123456789012");
        assert_eq!(e.call("strtoll", &[A, endp, 10]).as_u64(), 123_456_789_012);
        e.put_str(A, "18446744073709551615");
        assert_eq!(e.call("strtoull", &[A, endp, 10]).as_u64(), u64::MAX);
        e.put_str(A, "-1");
        assert_eq!(e.call("strtoull", &[A, endp, 10]).as_u64(), u64::MAX);
        e.put_str(A, "18446744073709551616");
        assert_eq!(e.call("strtoull", &[A, endp, 10]).as_u64(), u64::MAX);
        assert_eq!(e.errno(), 34);
        // Invalid base, no digits.
        e.set_errno(0);
        assert_eq!(e.call("strtol", &[A, endp, 99]).r0, 0);
        assert_eq!(e.errno(), 22);
        assert_eq!(e.mem().read_u32(endp).unwrap(), A);
        assert_eq!(e.call("strtoll", &[A, 0, 1]).as_u64(), 0);
        e.put_str(A, "xyz");
        assert_eq!(e.call("strtol", &[A, endp, 10]).r0, 0);
        assert_eq!(e.mem().read_u32(endp).unwrap(), A);
        // endptr unwritable.
        assert!(matches!(e.try_call("strtol", &[A, BAD, 10]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("strtol", &[A, BAD, 99]), Err(Stop::Fault(_))));
        assert_eq!(e.call("atoi", &[A]).r0, 0);
        e.put_str(A, " -17x");
        assert_eq!(e.call("atoi", &[A]).r0 as i32, -17);
        assert_eq!(e.call("atol", &[A]).r0 as i32, -17);
    }

    #[test]
    fn wide_character_functions() {
        let mut e = Env::new();
        for (f, yes, no) in [
            ("iswalnum", 'a' as u32, '-' as u32),
            ("iswdigit", '5' as u32, 0x0663), // Arabic-indic digit is not a C digit
            ("iswspace", ' ' as u32, 'a' as u32),
            ("iswxdigit", 'F' as u32, 'g' as u32),
        ] {
            assert_eq!(e.call(f, &[yes]).r0, 1, "{f}");
            assert_eq!(e.call(f, &[no]).r0, 0, "{f}");
            assert_eq!(e.call(f, &[0xd800]).r0, 0, "{f} on a surrogate");
        }
        assert_eq!(e.call("towlower", &['Q' as u32]).r0, 'q' as u32);
        assert_eq!(e.call("towupper", &[0xe9]).r0, 0xc9);
        assert_eq!(e.call("towupper", &[0xdf]).r0, 0xdf, "sharp s has no single-char upper case");
        assert_eq!(e.call("towlower", &[0xd800]).r0, 0xd800);
        assert_eq!(e.call("towupper", &[0xd800]).r0, 0xd800);
    }

    #[test]
    fn rand48_matches_posix() {
        let mut e = Env::new();
        e.call("srand48", &[1]);
        // POSIX: srand48(1) then lrand48() yields 89400484, 976015093, 1792756325, ...
        let v: Vec<u32> = (0..3).map(|_| e.call("lrand48", &[]).r0).collect();
        assert_eq!(v, [89_400_484, 976_015_093, 1_792_756_325]);
    }
}

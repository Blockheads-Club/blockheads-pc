//! `<arpa/inet.h>` address conversions (`inet_aton`, `inet_addr`, `inet_ntoa`, `inet_pton`, `inet_ntop`) and the
//! `h_errno` cell behind `__get_h_errno`. Addresses are in network byte order, as in the guest.

use crate::guest::{cstr, rd, wr, wr_cstr};
use crate::layout::errno;
use crate::misc::{fail, set_errno};
use crate::{lock, Impl, Shared};
use bhpc_cpu::Cpu;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

/// `AF_INET` / `AF_INET6` as numbered by Linux (and bionic).
const AF_INET: u32 = 2;
const AF_INET6: u32 = 10;
/// `INADDR_NONE`, the error value of `inet_addr`.
const INADDR_NONE: u32 = u32::MAX;

/// One component of an `inet_aton` address: decimal, octal (`0` prefix) or hex (`0x` prefix).
fn component(s: &str) -> Option<u32> {
    let (digits, radix) = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => (h, 16),
        None if s.len() > 1 && s.starts_with('0') => (&s[1..], 8),
        None => (s, 10),
    };
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_digit(radix)) {
        return None;
    }
    u32::from_str_radix(digits, radix).ok()
}

/// Parse the BSD `a.b.c.d`, `a.b.c`, `a.b` and `a` forms; the last component fills the remaining bytes.
/// Returns the address in host order.
fn parse_aton(s: &str) -> Option<u32> {
    let parts: Vec<u32> = s.split('.').map(component).collect::<Option<_>>()?;
    let (last, head) = parts.split_last()?;
    if head.len() > 3 || head.iter().any(|p| *p > 255) {
        return None;
    }
    let room = 8 * (4 - head.len() as u32);
    if room < 32 && *last >= 1 << room {
        return None;
    }
    let high = head.iter().enumerate().fold(0u32, |acc, (i, p)| acc | p << (24 - 8 * i as u32));
    Some(high | last)
}

fn inet_aton(c: &mut Cpu, _: &Shared) {
    let mut a = c.args();
    let (src, out) = (a.u32(c), a.u32(c));
    let Some(text) = cstr(c, src) else { return };
    let Some(addr) = std::str::from_utf8(&text).ok().and_then(parse_aton) else { return c.ret_u32(0) };
    if out == 0 || wr(c, out, &addr.to_be_bytes()) {
        c.ret_u32(1);
    }
}

fn inet_addr(c: &mut Cpu, _: &Shared) {
    let src = c.args().u32(c);
    let Some(text) = cstr(c, src) else { return };
    let addr = std::str::from_utf8(&text).ok().and_then(parse_aton);
    // The result is stored as the bytes of the address, i.e. big-endian in memory; a register holds it as a
    // little-endian word, so convert.
    c.ret_u32(addr.map_or(INADDR_NONE, u32::to_be));
}

/// `inet_ntoa(in_addr)`: the argument is the struct passed by value (one word); the text lives in a static buffer.
fn inet_ntoa(c: &mut Cpu, st: &Shared) {
    let word = c.args().u32(c);
    let text = Ipv4Addr::from(word.to_le_bytes()).to_string();
    let mut s = lock(st);
    if s.inet_buf == 0 {
        s.inet_buf = s.heap.calloc(1, 16);
    }
    let buf = s.inet_buf;
    drop(s);
    if wr_cstr(c, buf, text.as_bytes()) {
        c.ret_u32(buf);
    }
}

/// Size in bytes of an address of family `af`; `None` if the family is unsupported.
fn family_len(af: u32) -> Option<u32> {
    match af {
        AF_INET => Some(4),
        AF_INET6 => Some(16),
        _ => None,
    }
}

fn inet_pton(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (af, src, dst) = (a.u32(c), a.u32(c), a.u32(c));
    if family_len(af).is_none() {
        return fail(c, st, errno::EAFNOSUPPORT);
    }
    let Some(text) = cstr(c, src) else { return };
    let text = String::from_utf8_lossy(&text);
    let bytes = if af == AF_INET {
        // Unlike `inet_aton`: exactly four decimal components.
        let strict = text.split('.').count() == 4
            && text.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
        Ipv4Addr::from_str(&text).ok().filter(|_| strict).map(|ip| ip.octets().to_vec())
    } else {
        Ipv6Addr::from_str(&text).ok().map(|ip| ip.octets().to_vec())
    };
    match bytes {
        Some(b) if wr(c, dst, &b) => c.ret_u32(1),
        Some(_) => {}
        None => c.ret_u32(0),
    }
}

fn inet_ntop(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (af, src, dst, size) = (a.u32(c), a.u32(c), a.u32(c), a.u32(c));
    let Some(len) = family_len(af) else {
        set_errno(c, st, errno::EAFNOSUPPORT);
        return c.ret_u32(0);
    };
    let Some(raw) = rd(c, src, len) else { return };
    let text = if af == AF_INET {
        Ipv4Addr::new(raw[0], raw[1], raw[2], raw[3]).to_string()
    } else {
        Ipv6Addr::from(<[u8; 16]>::try_from(raw.as_slice()).expect("16 bytes were read")).to_string()
    };
    if text.len() as u32 + 1 > size {
        set_errno(c, st, errno::ENOSPC);
        return c.ret_u32(0);
    }
    if wr_cstr(c, dst, text.as_bytes()) {
        c.ret_u32(dst);
    }
}

/// `__get_h_errno()`: the address of the resolver error cell behind the `h_errno` macro.
fn get_h_errno(c: &mut Cpu, st: &Shared) {
    let mut s = lock(st);
    if s.h_errno == 0 {
        s.h_errno = s.heap.calloc(1, 4);
    }
    let cell = s.h_errno;
    drop(s);
    c.ret_u32(cell);
}

pub(crate) const TABLE: &[(&str, Impl)] = &[
    ("inet_aton", inet_aton),
    ("inet_addr", inet_addr),
    ("inet_ntoa", inet_ntoa),
    ("inet_pton", inet_pton),
    ("inet_ntop", inet_ntop),
    ("__get_h_errno", get_h_errno),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests_env::{Env, SCRATCH};
    use bhpc_cpu::Stop;

    const SRC: u32 = SCRATCH;
    const OUT: u32 = SCRATCH + 0x100;
    const BAD: u32 = 0x10;

    fn aton(e: &mut Env, s: &str) -> Option<[u8; 4]> {
        e.put_str(SRC, s);
        e.mem().write_u32(OUT, 0xDEAD_BEEF).unwrap();
        (e.call("inet_aton", &[SRC, OUT]).r0 == 1).then(|| e.mem().read_u32(OUT).unwrap().to_le_bytes())
    }

    #[test]
    fn inet_aton_accepts_the_bsd_forms() {
        let mut e = Env::new();
        assert_eq!(aton(&mut e, "192.168.1.20"), Some([192, 168, 1, 20]));
        assert_eq!(aton(&mut e, "10.1.513"), Some([10, 1, 2, 1]));
        assert_eq!(aton(&mut e, "10.65537"), Some([10, 1, 0, 1]));
        assert_eq!(aton(&mut e, "0x7f.1"), Some([127, 0, 0, 1]));
        assert_eq!(aton(&mut e, "0177.0.0.01"), Some([127, 0, 0, 1]));
        assert_eq!(aton(&mut e, "0x7f000001"), Some([127, 0, 0, 1]));
        assert_eq!(aton(&mut e, "4294967295"), Some([255; 4]));
        assert_eq!(aton(&mut e, "0"), Some([0; 4]));
        for bad in [
            "",
            "1.2.3.4.5",
            "256.1.1.1",
            "1.2.3.256",
            "1.2.65536",
            "4294967296",
            "a.b.c.d",
            "1..2",
            "0x",
            "09",
            "1.2.3.",
        ] {
            assert_eq!(aton(&mut e, bad), None, "{bad:?}");
        }
        // No output pointer: only validate.
        e.put_str(SRC, "1.2.3.4");
        assert_eq!(e.call("inet_aton", &[SRC, 0]).r0, 1);
        assert!(matches!(e.try_call("inet_aton", &[BAD, OUT]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("inet_aton", &[SRC, BAD]), Err(Stop::Fault(_))));
        // Non-UTF-8 text is just invalid.
        e.mem().write_bytes(SRC, &[0xff, 0xfe, 0]).unwrap();
        assert_eq!(e.call("inet_aton", &[SRC, OUT]).r0, 0);
    }

    #[test]
    fn inet_addr_and_inet_ntoa() {
        let mut e = Env::new();
        e.put_str(SRC, "127.0.0.1");
        // In memory 7f 00 00 01; as a little-endian register word 0x0100007f.
        assert_eq!(e.call("inet_addr", &[SRC]).r0, 0x0100_007f);
        e.put_str(SRC, "nonsense");
        assert_eq!(e.call("inet_addr", &[SRC]).r0, u32::MAX);
        assert!(matches!(e.try_call("inet_addr", &[BAD]), Err(Stop::Fault(_))));
        let p = e.call("inet_ntoa", &[0x0201_a8c0]).r0;
        assert_eq!(e.get_str(p), "192.168.1.2");
        let q = e.call("inet_ntoa", &[0]).r0;
        assert_eq!((q, e.get_str(q).as_str()), (p, "0.0.0.0"), "one static buffer");
        let mut s = Env::small();
        while s.call("malloc", &[16]).r0 != 0 {}
        assert!(matches!(s.try_call("inet_ntoa", &[1]), Err(Stop::Fault(_))));
    }

    #[test]
    fn inet_pton_and_ntop_round_trip() {
        let mut e = Env::new();
        let pton = |e: &mut Env, af: u32, s: &str| {
            e.put_str(SRC, s);
            let r = e.call("inet_pton", &[af, SRC, OUT]).r0 as i32;
            (r, e.mem().read_bytes(OUT, 16).unwrap())
        };
        let (r, b) = pton(&mut e, 2, "10.0.0.255");
        assert_eq!((r, &b[..4]), (1, &[10u8, 0, 0, 255][..]));
        for bad in ["10.0.0", "10.0.0.256", "1.2.3.4.5", "0x1.2.3.4", "1.2.3.", "+1.2.3.4", ""] {
            assert_eq!(pton(&mut e, 2, bad).0, 0, "{bad:?}");
        }
        let (r, b) = pton(&mut e, 10, "2001:db8::1");
        assert_eq!(r, 1);
        assert_eq!(b, [0x20, 1, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(pton(&mut e, 10, "not an address").0, 0);
        assert_eq!(pton(&mut e, 1, "1.2.3.4").0, -1);
        assert_eq!(e.errno(), errno::EAFNOSUPPORT);
        assert!(matches!(e.try_call("inet_pton", &[2, BAD, OUT]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("inet_pton", &[2, SRC, BAD]), Err(Stop::Fault(_))));
        // ntop.
        let raw = SCRATCH + 0x200;
        e.mem().write_bytes(raw, &[1, 2, 3, 4]).unwrap();
        assert_eq!(e.call("inet_ntop", &[2, raw, OUT, 16]).r0, OUT);
        assert_eq!(e.get_str(OUT), "1.2.3.4");
        e.mem().write_bytes(raw, &[0x20, 1, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]).unwrap();
        assert_eq!(e.call("inet_ntop", &[10, raw, OUT, 64]).r0, OUT);
        assert_eq!(e.get_str(OUT), "2001:db8::1");
        assert_eq!(e.call("inet_ntop", &[2, raw, OUT, 7]).r0, 0, "needs 8 bytes for 32.1.13.184");
        assert_eq!(e.errno(), errno::ENOSPC);
        assert_eq!(e.call("inet_ntop", &[2, raw, OUT, 13]).r0, OUT);
        assert_eq!(e.call("inet_ntop", &[7, raw, OUT, 64]).r0, 0);
        assert_eq!(e.errno(), errno::EAFNOSUPPORT);
        assert!(matches!(e.try_call("inet_ntop", &[2, BAD, OUT, 16]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("inet_ntop", &[2, raw, BAD, 16]), Err(Stop::Fault(_))));
    }

    #[test]
    fn h_errno_is_one_zeroed_cell() {
        let mut e = Env::new();
        let p = e.call("__get_h_errno", &[]).r0;
        assert_eq!(e.mem().read_u32(p).unwrap(), 0);
        e.mem().write_u32(p, 1).unwrap();
        assert_eq!(e.call("__get_h_errno", &[]).r0, p);
        assert_eq!(e.mem().read_u32(p).unwrap(), 1);
    }
}

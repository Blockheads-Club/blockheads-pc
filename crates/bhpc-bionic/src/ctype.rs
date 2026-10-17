//! `<ctype.h>`: the old-bionic `_ctype_` / `_tolower_tab_` / `_toupper_tab_` tables (C locale) and the
//! `is*` / `to*` functions built on them.
//!
//! In that ABI `_ctype_` is a `const char *` variable pointing at a 257-entry table (index `c + 1`,
//! entry 0 is for EOF); `_tolower_tab_` / `_toupper_tab_` are `const short *` variables of the same shape.

use crate::layout::ctype::{B, C, L, N, P, S, TABLE_LEN, U, X};
use crate::{Impl, Shared};
use bhpc_cpu::Cpu;

/// The three tables: flags, lower-case map and upper-case map (all indexed by `c + 1`).
pub fn tables() -> (Vec<u8>, Vec<i16>, Vec<i16>) {
    let mut flags = vec![0u8; TABLE_LEN];
    let mut lower = vec![0i16; TABLE_LEN];
    let mut upper = vec![0i16; TABLE_LEN];
    lower[0] = -1;
    upper[0] = -1;
    for c in 0u16..256 {
        let i = c as usize + 1;
        let b = c as u8;
        flags[i] = match b {
            b'\t'..=b'\r' => C | S,
            0..=0x1f | 0x7f => C,
            b' ' => S | B,
            b'0'..=b'9' => N,
            b'A'..=b'F' => U | X,
            b'G'..=b'Z' => U,
            b'a'..=b'f' => L | X,
            b'g'..=b'z' => L,
            0x21..=0x7e => P,
            _ => 0,
        };
        lower[i] = b.to_ascii_lowercase() as i16;
        upper[i] = b.to_ascii_uppercase() as i16;
    }
    (flags, lower, upper)
}

fn flags_of(c: i32) -> u8 {
    static T: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    let t = T.get_or_init(|| tables().0);
    // The function forms accept EOF and any byte value; anything else is outside the table.
    match c {
        -1..=255 => t[(c + 1) as usize],
        _ => 0,
    }
}

macro_rules! classify {
    ($name:ident, $mask:expr) => {
        fn $name(c: &mut Cpu, _: &Shared) {
            let ch = c.args().u32(c) as i32;
            c.ret_u32((flags_of(ch) & $mask != 0) as u32);
        }
    };
}
classify!(isalpha, U | L);
classify!(isdigit, N);
classify!(isalnum, U | L | N);
classify!(isspace, S);
classify!(isupper, U);
classify!(islower, L);
classify!(isxdigit, N | X);
classify!(ispunct, P);
classify!(iscntrl, C);
classify!(isprint, P | U | L | N | B);
classify!(isgraph, P | U | L | N);

fn isblank(c: &mut Cpu, _: &Shared) {
    let ch = c.args().u32(c) as i32;
    c.ret_u32((ch == 0x20 || ch == 0x09) as u32);
}

fn isascii(c: &mut Cpu, _: &Shared) {
    let ch = c.args().u32(c);
    c.ret_u32((ch < 0x80) as u32);
}

fn tolower(c: &mut Cpu, _: &Shared) {
    let ch = c.args().u32(c) as i32;
    c.ret_u32(if (0..256).contains(&ch) { (ch as u8).to_ascii_lowercase() as u32 } else { ch as u32 });
}

fn toupper(c: &mut Cpu, _: &Shared) {
    let ch = c.args().u32(c) as i32;
    c.ret_u32(if (0..256).contains(&ch) { (ch as u8).to_ascii_uppercase() as u32 } else { ch as u32 });
}

pub(crate) const TABLE: &[(&str, Impl)] = &[
    ("isalpha", isalpha),
    ("isdigit", isdigit),
    ("isalnum", isalnum),
    ("isspace", isspace),
    ("isupper", isupper),
    ("islower", islower),
    ("isxdigit", isxdigit),
    ("ispunct", ispunct),
    ("iscntrl", iscntrl),
    ("isprint", isprint),
    ("isgraph", isgraph),
    ("isblank", isblank),
    ("isascii", isascii),
    ("tolower", tolower),
    ("toupper", toupper),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testenv::Env;

    #[test]
    fn table_shape_and_samples() {
        let (f, lo, up) = tables();
        assert_eq!((f.len(), lo.len(), up.len()), (257, 257, 257));
        assert_eq!((f[0], lo[0], up[0]), (0, -1, -1));
        let at = |c: u8| f[c as usize + 1];
        assert_eq!(at(b'A'), U | X);
        assert_eq!(at(b'G'), U);
        assert_eq!(at(b'a'), L | X);
        assert_eq!(at(b'z'), L);
        assert_eq!(at(b'5'), N);
        assert_eq!(at(b' '), S | B);
        assert_eq!(at(b'\n'), C | S);
        assert_eq!(at(0), C);
        assert_eq!(at(0x7f), C);
        assert_eq!(at(b'!'), P);
        assert_eq!(at(b'~'), P);
        assert_eq!(at(0x80), 0);
        assert_eq!(at(0xff), 0);
        assert_eq!(lo[b'Q' as usize + 1], b'q' as i16);
        assert_eq!(up[b'q' as usize + 1], b'Q' as i16);
        assert_eq!(lo[0xc4 + 1], 0xc4);
    }

    #[test]
    fn data_symbols_hold_pointers_to_the_tables() {
        let e = Env::new();
        let (f, lo, up) = tables();
        let var = e.data("_ctype_");
        let tab = e.mem().read_u32(var).unwrap();
        assert_eq!(e.mem().read_bytes(tab, 257).unwrap(), f);
        let tab = e.mem().read_u32(e.data("_tolower_tab_")).unwrap();
        assert_eq!(e.mem().read_u16(tab + 2 * (b'A' as u32 + 1)).unwrap(), lo[b'A' as usize + 1] as u16);
        assert_eq!(e.mem().read_u16(tab).unwrap(), 0xffff);
        let tab = e.mem().read_u32(e.data("_toupper_tab_")).unwrap();
        assert_eq!(e.mem().read_u16(tab + 2 * (b'a' as u32 + 1)).unwrap(), up[b'a' as usize + 1] as u16);
    }

    #[test]
    fn classification_functions() {
        let mut e = Env::new();
        let yes = |e: &mut Env, f: &str, c: i32| e.call(f, &[c as u32]).r0 != 0;
        for (f, good, bad) in [
            ("isalpha", 'a', '1'),
            ("isdigit", '7', 'a'),
            ("isalnum", 'Z', '-'),
            ("isspace", '\t', 'x'),
            ("isupper", 'Q', 'q'),
            ("islower", 'q', 'Q'),
            ("isxdigit", 'f', 'g'),
            ("ispunct", '!', 'a'),
            ("iscntrl", '\u{1}', 'a'),
            ("isprint", ' ', '\u{1}'),
            ("isgraph", 'a', ' '),
            ("isblank", '\t', '\n'),
            ("isascii", 'a', '\u{80}'),
        ] {
            assert!(yes(&mut e, f, good as i32), "{f}({good:?})");
            assert!(!yes(&mut e, f, bad as i32), "{f}({bad:?})");
        }
        assert!(!yes(&mut e, "isalpha", -1));
        assert!(!yes(&mut e, "isalpha", 1000));
        assert!(!yes(&mut e, "isalpha", -2));
        assert_eq!(e.call("tolower", &['Q' as u32]).r0, 'q' as u32);
        assert_eq!(e.call("toupper", &['q' as u32]).r0, 'Q' as u32);
        assert_eq!(e.call("tolower", &[-1i32 as u32]).r0 as i32, -1);
        assert_eq!(e.call("toupper", &[1000]).r0, 1000);
        assert_eq!(e.call("tolower", &[0xc4]).r0, 0xc4);
    }
}

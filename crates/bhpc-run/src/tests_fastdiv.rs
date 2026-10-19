//! Hardware division routines: exact helper semantics on edge cases and random inputs, the zero-divisor path, and
//! which entries get patched.

use crate::fastdiv::*;
use crate::new_env;
use bhpc_cpu::Cpu;
use std::collections::HashMap;

const CODE: u32 = 0x2000_0000;
/// Offset of each fake helper's zero-divisor path from its entry.
const ZERO: u32 = 0x40;
/// What a fake zero-divisor path returns (`movw r0, #0xdead; bx lr`).
const ZERO_PATH: [u32; 2] = [0xE30D_0EAD, 0xE12F_FF1E];
/// What the fake helpers' (never executed) bodies hold: `udf`.
const UDF: u32 = 0xE7F0_00F0;

/// The three prologues libgcc's helpers start with, each branching to `entry + ZERO` for a zero divisor.
fn prologue(kind: usize) -> Vec<u32> {
    // b<cond> at index i to entry + ZERO: imm24 = (ZERO - (4 * i + 8)) / 4.
    let b = |cond: u32, i: u32| (cond << 28) | 0x0A00_0000 | ((ZERO - (4 * i + 8)) / 4);
    match kind {
        0 => vec![0xE351_0000, b(0x0, 1)],              // cmp r1,#0; beq Z
        1 => vec![0xE251_2001, 0x012F_FF1E, b(0x3, 2)], // subs r2,r1,#1; bxeq lr; blo Z
        _ => vec![0xE251_2001, b(0x3, 1)],              // subs r2,r1,#1; blo Z
    }
}

/// A fake helper at `at`: the prologue, `udf` filler, and the zero-divisor path.
fn fake_helper(cpu: &Cpu, at: u32, kind: usize) {
    let mut words = prologue(kind);
    words.resize((ZERO / 4) as usize, UDF);
    words.extend(ZERO_PATH);
    for (i, w) in words.iter().enumerate() {
        cpu.mem().write_u32(at + 4 * i as u32, *w).unwrap();
    }
}

fn reference(op: Op, n: u32, d: u32) -> (u32, Option<u32>) {
    let (sn, sd) = (n as i32, d as i32);
    match op {
        Op::UDiv => (n / d, None),
        Op::SDiv => (sn.wrapping_div(sd) as u32, None),
        Op::UDivMod => (n / d, Some(n % d)),
        Op::SDivMod => (sn.wrapping_div(sd) as u32, Some(sn.wrapping_rem(sd) as u32)),
        Op::UMod => (n % d, None),
        Op::SMod => (sn.wrapping_rem(sd) as u32, None),
    }
}

/// Every helper as a fake at its own address (prologue kinds rotate), patched; returns the CPU and the entries.
fn patched() -> (Box<Cpu>, HashMap<&'static str, u32>, Vec<&'static str>) {
    let (cpu, _st) = new_env(Box::new(std::io::sink()), None).unwrap();
    cpu.mem().map(CODE, 0x10000).unwrap();
    let mut at = HashMap::new();
    for (i, (name, _)) in HELPERS.iter().enumerate() {
        let a = CODE + 0x100 * i as u32;
        fake_helper(&cpu, a, i % 3);
        at.insert(*name, a);
    }
    let names = install(cpu.mem(), &|n| at.get(n).copied()).unwrap();
    (cpu, at, names)
}

fn inputs() -> Vec<(u32, u32)> {
    let edges =
        [0u32, 1, 2, 3, 7, 10, 255, 0x7FFF_FFFF, 0x8000_0000, 0x8000_0001, 0xFFFF_FFFF, 0xFFFF_FFFE, 0xFFFF_FFF9];
    let mut v: Vec<(u32, u32)> = edges.iter().flat_map(|&n| edges.iter().map(move |&d| (n, d))).collect();
    let mut x = 0x1234_5678u32;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x
    };
    for i in 0..1500 {
        let (n, d) = (next(), next());
        // Mix full-width, small and shifted divisors.
        v.push((n, [d, d >> 24, d >> (i % 32)][i % 3]));
    }
    v
}

#[test]
fn helpers_match_libgcc_semantics() {
    let (mut cpu, at, names) = patched();
    assert_eq!(names.len(), HELPERS.len(), "every recognised helper is patched: {names:?}");
    for (name, op) in HELPERS {
        let entry = at[name];
        assert_eq!(cpu.mem().read_u32(entry).unwrap(), 0xE51F_F004, "{name} entry redirected");
        for (n, d) in inputs() {
            cpu.set_reg(1, 0x5151_5151);
            let r = cpu.call(entry, &[n, d]).unwrap();
            if d == 0 {
                assert_eq!(r.r0, 0xDEAD, "{name}({n:#x}, 0) takes the helper's own zero path");
                continue;
            }
            let (q, rem) = reference(op, n, d);
            assert_eq!(r.r0, q, "{name}({n:#x}, {d:#x})");
            if let Some(rem) = rem {
                assert_eq!(r.r1, rem, "{name}({n:#x}, {d:#x}) remainder");
            }
        }
    }
}

#[test]
fn aliases_share_one_routine_and_unknown_entries_are_skipped() {
    let (cpu, _st) = new_env(Box::new(std::io::sink()), None).unwrap();
    cpu.mem().map(CODE, 0x10000).unwrap();
    fake_helper(&cpu, CODE, 0);
    // __aeabi_idiv: Thumb entry; __divsi3: unknown prologue; __modsi3: subs prologue without a branch.
    cpu.mem().write_u32(CODE + 0x200, 0xE1A0_0000).unwrap();
    for (i, w) in [0xE251_2001u32, 0xE1A0_0000, 0xE1A0_0000].iter().enumerate() {
        cpu.mem().write_u32(CODE + 0x300 + 4 * i as u32, *w).unwrap();
    }
    let map: HashMap<&str, u32> = [
        ("__aeabi_uidiv", CODE),
        ("__udivsi3", CODE),
        ("__aeabi_idiv", (CODE + 0x100) | 1),
        ("__divsi3", CODE + 0x200),
        ("__modsi3", CODE + 0x300),
    ]
    .into();
    let names = install(cpu.mem(), &|n| map.get(n).copied()).unwrap();
    assert_eq!(names, ["__aeabi_uidiv", "__udivsi3"]);
    assert_eq!(cpu.mem().read_u32(CODE + 0x200).unwrap(), 0xE1A0_0000, "unknown prologue untouched");
    assert_eq!(cpu.mem().read_u32(CODE + 0x300).unwrap(), 0xE251_2001);
    // Nothing recognised: no page is mapped, nothing written.
    assert!(install(cpu.mem(), &|_| None).unwrap().is_empty());
    // An unreadable entry is skipped too.
    assert!(install(cpu.mem(), &|n| (n == "__modsi3").then_some(0x0BAD_0000)).unwrap().is_empty());
}

#[test]
fn routines_and_switch() {
    // The literal holding the zero path is the routine's last word, loaded by `ldreq pc` from 8 bytes ahead.
    for (_, op) in HELPERS {
        let r = routine(op, 0x1234);
        assert_eq!(*r.last().unwrap(), 0x1234);
        assert_eq!(r[1] & 0xFFF, (r.len() as u32 - 1 - 3) * 4, "{op:?}");
        assert!(r.len() * 4 <= 32);
    }
    let m = bhpc_mem::GuestMemory::new().unwrap();
    m.map(CODE, 0x1000).unwrap();
    m.write_u32(CODE, 0xE351_0000).unwrap();
    m.write_u32(CODE + 4, 0x1A00_0000).unwrap(); // bne, not beq
    assert_eq!(zero_path(&m, CODE), None);
    assert!(enabled(None));
    assert!(enabled(Some(std::ffi::OsStr::new("1"))));
    assert!(!enabled(Some(std::ffi::OsStr::new("0"))));
}

#[test]
fn install_all_covers_every_module_unless_disabled() {
    let (cpu, _st) = new_env(Box::new(std::io::sink()), None).unwrap();
    cpu.mem().map(CODE, 0x10000).unwrap();
    fake_helper(&cpu, CODE, 0);
    fake_helper(&cpu, CODE + 0x100, 2);
    // Module 0 exports __aeabi_uidiv, module 1 __umodsi3.
    let export = |i: usize, n: &str| match (i, n) {
        (0, "__aeabi_uidiv") => Some(CODE),
        (1, "__umodsi3") => Some(CODE + 0x100),
        _ => None,
    };
    assert_eq!(install_all(cpu.mem(), Some(std::ffi::OsStr::new("0")), 2, &export).unwrap(), 0);
    assert_eq!(cpu.mem().read_u32(CODE).unwrap(), 0xE351_0000, "disabled: untouched");
    assert_eq!(install_all(cpu.mem(), None, 2, &export).unwrap(), 2);
    assert_eq!(cpu.mem().read_u32(CODE + 0x100).unwrap(), 0xE51F_F004);
}

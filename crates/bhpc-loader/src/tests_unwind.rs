use crate::unwind::*;
use bhpc_mem::GuestMemory;

const BASE: u32 = 0x1000_0000;
const TABLE: u32 = BASE + 0x1000;
const EXTAB: u32 = BASE + 0x2000;

fn mem() -> GuestMemory {
    let m = GuestMemory::new().unwrap();
    m.map(BASE, 0x4000).unwrap();
    m
}

fn prel(from: u32, to: u32) -> u32 {
    to.wrapping_sub(from) & 0x7FFF_FFFF
}

/// Entry `i`: function at `func`, second word `word`.
fn entry(m: &GuestMemory, i: u32, func: u32, word: u32) {
    let at = TABLE + i * 8;
    m.write_u32(at, prel(at, func)).unwrap();
    m.write_u32(at + 4, word).unwrap();
}

fn regs_with(sp: u32, lr: u32, pc: u32) -> [u32; 16] {
    let mut r = [0u32; 16];
    r[13] = sp;
    r[14] = lr;
    r[15] = pc;
    r
}

fn ex() -> (u32, u32) {
    (TABLE, 4)
}

#[test]
fn opcodes_cover_every_table_format() {
    let m = mem();
    entry(&m, 0, BASE + 0x100, 0x80A8_B0B0); // inline Su16
    entry(&m, 1, BASE + 0x200, 1); // CANTUNWIND
    entry(&m, 2, BASE + 0x300, prel(TABLE + 20, EXTAB)); // compact in .ARM.extab, personality 0
    entry(&m, 3, BASE + 0x400, prel(TABLE + 28, EXTAB + 0x10)); // personality 1 with 2 extra words
    m.write_u32(EXTAB, 0x8000_0000 | 0x00AA_BBCC).unwrap();
    m.write_u32(EXTAB + 0x10, 0x8100_0000 | 2 << 16 | 0x1122).unwrap();
    m.write_u32(EXTAB + 0x14, 0x0304_0506).unwrap();
    m.write_u32(EXTAB + 0x18, 0x0708_090A).unwrap();
    assert_eq!(opcodes(&m, ex(), BASE + 0x100), Some(vec![0xA8, 0xB0, 0xB0]));
    assert_eq!(opcodes(&m, ex(), BASE + 0x1FF), Some(vec![0xA8, 0xB0, 0xB0]), "within the function");
    assert_eq!(opcodes(&m, ex(), BASE + 0x200), None, "CANTUNWIND");
    assert_eq!(opcodes(&m, ex(), BASE + 0x300), Some(vec![0xAA, 0xBB, 0xCC]));
    assert_eq!(opcodes(&m, ex(), BASE + 0x400), Some(vec![0x11, 0x22, 3, 4, 5, 6, 7, 8, 9, 10]));
    assert_eq!(opcodes(&m, ex(), BASE + 0x50), None, "before the first function");
    assert_eq!(opcodes(&m, (TABLE, 100_000), BASE + 0x500), None, "table runs off the mapping");
    assert_eq!(opcodes(&m, (0x2000_0000, 4), BASE), None, "unmapped table");
    // Generic model: personality pointer, then a length+3 opcode word and one more word.
    let m = mem();
    entry(&m, 0, BASE + 0x100, prel(TABLE + 4, EXTAB));
    m.write_u32(EXTAB, prel(EXTAB, BASE + 0x900)).unwrap();
    m.write_u32(EXTAB + 4, 1 << 24 | 0x0001_0203).unwrap();
    m.write_u32(EXTAB + 8, 0xB0B0_B0B0).unwrap();
    assert_eq!(opcodes(&m, (TABLE, 1), BASE + 0x100), Some(vec![1, 2, 3, 0xB0, 0xB0, 0xB0, 0xB0]));
    // Truncated tables.
    m.write_u32(TABLE + 4, 0x3FFF_F000).unwrap(); // extab pointer into unmapped memory
    assert_eq!(opcodes(&m, (TABLE, 1), BASE + 0x100), None);
    m.write_u32(TABLE + 4, prel(TABLE + 4, BASE + 0x3FFC)).unwrap();
    m.write_u32(BASE + 0x3FFC, 0x0000_0000).unwrap(); // generic, but the second word is unmapped
    assert_eq!(opcodes(&m, (TABLE, 1), BASE + 0x100), None);
    m.write_u32(TABLE + 4, prel(TABLE + 4, EXTAB + 0x3000 - 0x2000 - 8)).unwrap();
    let at = BASE + 0x4000 - 8; // generic header in the last mapped words, extra words unmapped
    m.write_u32(TABLE + 4, prel(TABLE + 4, at)).unwrap();
    m.write_u32(at, 0).unwrap();
    m.write_u32(at + 4, 3 << 24).unwrap();
    assert_eq!(opcodes(&m, (TABLE, 1), BASE + 0x100), None);
    // Compact with extra words running off the end.
    let at = BASE + 0x4000 - 4;
    m.write_u32(TABLE + 4, prel(TABLE + 4, at)).unwrap();
    m.write_u32(at, 0x8100_0000 | 3 << 16).unwrap();
    assert_eq!(opcodes(&m, (TABLE, 1), BASE + 0x100), None);
}

fn run(ops: &[u8], regs: &mut [u32; 16], stack: &[u32]) -> bool {
    let m = mem();
    for (i, w) in stack.iter().enumerate() {
        m.write_u32(BASE + 0x100 + 4 * i as u32, *w).unwrap();
    }
    regs[13] = BASE + 0x100;
    execute(&m, ops, regs)
}

#[test]
fn execute_stack_adjusts_and_pops() {
    let mut r = [0u32; 16];
    r[14] = 0xCAFE;
    assert!(run(&[0x01, 0x40, 0xB0], &mut r, &[]));
    assert_eq!((r[13], r[15]), (BASE + 0x100 + 8 - 4, 0xCAFE), "vsp += 8 then -= 4, pc = lr");
    // pop {r4-r6} (A2) then pop {r4, lr} (A8)
    let mut r = [0u32; 16];
    assert!(run(&[0xA2, 0xB0], &mut r, &[4, 5, 6]));
    assert_eq!((r[4], r[5], r[6], r[13]), (4, 5, 6, BASE + 0x100 + 12));
    let mut r = [0u32; 16];
    assert!(run(&[0xA9, 0xB0], &mut r, &[4, 5, 0x77]));
    assert_eq!((r[4], r[5], r[14], r[15]), (4, 5, 0x77, 0x77), "r4-r5 and lr");
    let mut r = [0u32; 16];
    assert!(run(&[0xAA, 0xB0], &mut r, &[1, 2, 3, 0x88]));
    assert_eq!(r[14], 0x88, "r4-r6 and lr");
    // mask pop with pc: 0x8 0x80 0x00 = r15 only -> pc popped, no lr copy
    let mut r = [0u32; 16];
    r[14] = 0xBAD;
    assert!(run(&[0x88, 0x00], &mut r, &[0x1234]));
    assert_eq!(r[15], 0x1234);
    // r4, lr and pc: 0x8C 0x01 -> mask 0xC010
    let mut r = [0u32; 16];
    assert!(run(&[0x8C, 0x01], &mut r, &[4, 14, 15]));
    assert_eq!((r[4], r[14], r[15]), (4, 14, 15));
    // pop r0-r3
    let mut r = [0u32; 16];
    assert!(run(&[0xB1, 0x05, 0xB0], &mut r, &[10, 12]));
    assert_eq!((r[0], r[2]), (10, 12));
    // vsp = r11
    let mut r = [0u32; 16];
    r[11] = 0xABC0;
    assert!(run(&[0x9B, 0xB0], &mut r, &[]));
    assert_eq!(r[13], 0xABC0);
}

#[test]
fn execute_variable_length_and_vfp_ops() {
    let base = BASE + 0x100;
    let mut r = [0u32; 16];
    assert!(run(&[0xB2, 0x02, 0xB0], &mut r, &[]));
    assert_eq!(r[13], base + 0x204 + 8);
    let mut r = [0u32; 16];
    assert!(run(&[0xB2, 0x81, 0x01, 0xB0], &mut r, &[]), "two-byte uleb128");
    assert_eq!(r[13], base + 0x204 + ((1 | 1 << 7) << 2));
    let mut r = [0u32; 16];
    assert!(run(&[0xB2, 0x80, 0x80, 0x80, 0x80, 0x80], &mut r, &[]), "uleb stops after five bytes");
    let mut r = [0u32; 16];
    assert!(run(&[0xB3, 0x01, 0xC9, 0x01, 0xC8, 0x00, 0xB0], &mut r, &[]));
    assert_eq!(r[13], base + 20 + 16 + 8);
    let mut r = [0u32; 16];
    assert!(run(&[0xBA, 0xD1, 0xB0], &mut r, &[]));
    assert_eq!(r[13], base + 28 + 16);
}

#[test]
fn execute_rejects_what_it_cannot_follow() {
    for ops in [
        &[0x80, 0x00][..], // refuse to unwind
        &[0x80],           // truncated mask
        &[0x9D],           // vsp = sp
        &[0x9F],           // reserved
        &[0xB1],           // truncated
        &[0xB1, 0x00],     // empty mask
        &[0xB1, 0x10],     // reserved mask bits
        &[0xB2],           // truncated uleb
        &[0xB3],           // truncated vfp
        &[0xC0],           // iWMMXt: not implemented
        &[0xA0],           // unreadable stack (below)
    ] {
        let mut r = [0u32; 16];
        let m = mem();
        r[13] = if ops == [0xA0] { 0x2000_0000 } else { BASE };
        assert!(!execute(&m, ops, &mut r), "{ops:x?}");
    }
    // Every pop form reports unreadable stack memory.
    for ops in [&[0xA8][..], &[0x88, 0x00], &[0xB1, 0x01]] {
        let mut r = [0u32; 16];
        r[13] = 0x2000_0000;
        assert!(!execute(&mem(), ops, &mut r), "{ops:x?}");
    }
}

#[test]
fn backtrace_walks_frames_and_stops_cleanly() {
    let m = mem();
    // fn A (0x100): push {r4, lr}; fn B (0x200): push {r4, r5, lr}; fn C (0x300): CANTUNWIND
    entry(&m, 0, BASE + 0x100, 0x80A8_B0B0);
    entry(&m, 1, BASE + 0x200, 0x80A9_B0B0);
    entry(&m, 2, BASE + 0x300, 1);
    entry(&m, 3, BASE + 0x400, 0x80B0_B0B0);
    let stack = BASE + 0x3000;
    // A's frame: [r4, lr -> inside B]; B's frame: [r4, r5, lr -> inside C]
    for (i, w) in [0u32, BASE + 0x204 + 1, 0, 0, BASE + 0x304 + 1].iter().enumerate() {
        m.write_u32(stack + 4 * i as u32, *w).unwrap();
    }
    let find = |pc: u32| (BASE..BASE + 0x1000).contains(&pc).then_some((TABLE, 4));
    let frames = backtrace(&m, &find, regs_with(stack, 0, BASE + 0x110), 16);
    assert_eq!(
        frames.iter().map(|f| f.pc).collect::<Vec<_>>(),
        [BASE + 0x110, BASE + 0x205, BASE + 0x305],
        "stops at the CANTUNWIND function after listing it"
    );
    assert_eq!(frames[0].sp, stack);
    assert_eq!(frames[1].sp, stack + 8);
    // Frame limit.
    assert_eq!(backtrace(&m, &find, regs_with(stack, 0, BASE + 0x110), 2).len(), 2);
    // Unknown module: only the stop frame.
    assert_eq!(backtrace(&m, &|_| None, regs_with(stack, 0, 0x9999), 5).len(), 1);
    // A function that only returns through an unchanged lr makes no progress: stop.
    let r = regs_with(stack, BASE + 0x404, BASE + 0x404);
    assert_eq!(backtrace(&m, &find, r, 8).len(), 1);
    // Returning to address 0 ends the walk.
    let r = regs_with(stack, 0, BASE + 0x404);
    assert_eq!(backtrace(&m, &find, r, 8).len(), 1);
    // Unreadable stack ends the walk.
    let r = regs_with(0x2000_0000, 0, BASE + 0x110);
    assert_eq!(backtrace(&m, &find, r, 8).len(), 1);
}

#[test]
fn linker_backtrace_uses_the_module_tables() {
    use crate::testelf::Builder;
    use crate::Linker;
    use std::sync::Arc;
    let mem = Arc::new(GuestMemory::new().unwrap());
    let mut l = Linker::new(mem.clone());
    let img = Builder { exidx: true, ..Default::default() }.build();
    l.load("x.so", &img.bytes, &mut |_: &str| None).unwrap();
    let m = l.module("x.so").unwrap();
    let frames = l.backtrace(regs_with(m.start, 0, m.start + 4), 4);
    assert!(!frames.is_empty());
    assert_eq!(l.backtrace(regs_with(0x2000_0000, 0, 0x1234), 4).len(), 1);
}

//! The stale timer fire patch: where it applies, and that a receive whose deadline lies ahead no longer sleeps.

use crate::mktimer::*;
use crate::new_env;
use bhpc_cpu::Cpu;
use bhpc_loader::{Segment, PF_X};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

const CODE: u32 = 0x2000_0000;
/// Where the loop starts (the code before it is `udf`, the loop's back branch lands there).
const AT: u32 = CODE + 0x100;
/// The stub every `bl` of the loop calls (`ldr pc, [pc, #-4]` to a counting thunk).
const STUB: u32 = CODE + 0x800;
const DATA: u32 = 0x2000_8000;
/// The receive's frame pointer, deadline, "now" and port.
const FP: u32 = DATA + 0x100;
const DEADLINE: u32 = DATA + 0x200;
const SP: u32 = DATA + 0x400;
const PORT: u32 = 0xCAFE;
const UDF: u32 = 0xE7F0_00F0;
const BX_LR: u32 = 0xE12F_FF1E;

/// An ARM `bl` from `from` to `to`.
fn bl(from: u32, to: u32) -> u32 {
    0xEB00_0000 | ((to.wrapping_sub(from + 8) >> 2) & 0x00FF_FFFF)
}

/// The loop's words at `at` with every `bl` aimed at `to`.
fn loop_words(at: u32, to: u32) -> Vec<u32> {
    LOOP.iter().enumerate().map(|(i, w)| w.unwrap_or_else(|| bl(at + 4 * i as u32, to))).collect()
}

fn bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// A CPU with the loop at [`AT`] (then `bx lr`) and its frame; the counter counts calls of the stub.
fn setup() -> (Box<Cpu>, Arc<AtomicU32>) {
    let (cpu, _st) = new_env(Box::new(std::io::sink()), None).unwrap();
    let m = cpu.mem();
    m.map(CODE, 0x10000).unwrap();
    m.write_bytes(CODE, &bytes(&[UDF; 0x200])).unwrap();
    let mut code = loop_words(AT, STUB);
    code.push(BX_LR);
    m.write_bytes(AT, &bytes(&code)).unwrap();
    let calls = Arc::new(AtomicU32::new(0));
    let c2 = calls.clone();
    let thunk = cpu
        .register_shared_thunk(
            "test_sleep",
            Arc::new(move |_: &mut Cpu| {
                c2.fetch_add(1, Ordering::SeqCst);
            }),
        )
        .unwrap();
    m.write_bytes(STUB, &bytes(&[0xE51F_F004, thunk])).unwrap();
    let w = |a: u32, v: u32| m.write_u32(a, v).unwrap();
    w(FP - 28, DEADLINE);
    w(DEADLINE, 5000);
    w(DEADLINE + 4, 0);
    w(FP - 40, 1000);
    w(FP - 36, 0);
    w(FP - 8, PORT);
    (cpu, calls)
}

/// Run the loop with the "deadline later than now" flag; the result register r0, or `None` if the guest stopped.
fn receive(cpu: &mut Cpu, later: bool) -> Option<u32> {
    cpu.mem().write_u32(SP + 28, u32::from(later)).unwrap();
    cpu.set_reg(11, FP);
    cpu.set_sp(SP);
    cpu.set_ticks(1_000_000);
    cpu.call(AT, &[]).ok().map(|r| r.r0)
}

fn deadline(cpu: &Cpu) -> (u32, u32) {
    (cpu.mem().read_u32(DEADLINE).unwrap(), cpu.mem().read_u32(DEADLINE + 4).unwrap())
}

#[test]
fn unpatched_the_receive_sleeps_towards_a_deadline_ahead() {
    let (mut cpu, calls) = setup();
    // The sleep body runs (its calls reach the stub), then branches back to the loop test (here: `udf`).
    assert_eq!(receive(&mut cpu, true), None);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[test]
fn patched_a_deadline_ahead_is_delivered_at_once_and_stays_armed() {
    let (mut cpu, calls) = setup();
    assert_eq!(install(cpu.mem(), CODE, 0x1000).unwrap(), Some(AT + 4 * PATCH_AT as u32));
    cpu.clear_cache();
    assert_eq!(receive(&mut cpu, true), Some(PORT));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(deadline(&cpu), (5000, 0));
}

#[test]
fn patched_a_deadline_that_passed_is_disarmed_as_before() {
    let (mut cpu, calls) = setup();
    install(cpu.mem(), CODE, 0x1000).unwrap();
    cpu.clear_cache();
    assert_eq!(receive(&mut cpu, false), Some(PORT));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(deadline(&cpu), (0xFFFF_FFFE, 0xFFFF_FFFF));
}

#[test]
fn the_loop_is_found_with_any_call_targets_only() {
    let words = loop_words(0x40, 0x9000);
    let mut code = bytes(&[UDF; 4]);
    code.extend(bytes(&words));
    assert_eq!(find(&code), Some(16));
    // Different call targets still match; anything but a `bl` in a call slot does not.
    assert_eq!(find(&bytes(&loop_words(0x40, 0x20))), Some(0));
    let mut other = words.clone();
    other[15] = 0xFB00_0000; // the unconditional space (`blx` immediate), not a `bl`
    assert_eq!(find(&bytes(&other)), None);
    other[15] = 0xE1A0_0000; // nop
    assert_eq!(find(&bytes(&other)), None);
    let mut changed = words;
    changed[PATCH_AT] = 0xE1A0_0000;
    assert_eq!(find(&bytes(&changed)), None);
    assert_eq!(find(&code[..code.len() - 4]), None);
}

#[test]
fn branches_are_encoded_relative_to_pc_plus_8() {
    assert_eq!(branch(0x1000, 0x1008), 0xEA00_0000);
    assert_eq!(branch(0x3B5A8, 0x3B604), 0xEA00_0015);
    assert_eq!(branch(0x1008, 0x1000), 0xEAFF_FFFC);
}

#[test]
fn only_executable_segments_of_libsystem_are_patched() {
    let (cpu, _) = setup();
    let exec = [Segment { start: CODE, len: 0x1000, prot: PF_X | 4 }];
    let data = [Segment { start: CODE, len: 0x1000, prot: 4 | 2 }];
    let mods = [("libfoo.so", &exec[..]), ("libSystem.so", &data[..])];
    assert_eq!(install_all(cpu.mem(), mods).unwrap(), Vec::<u32>::new());
    let mods = [("libv.so", &exec[..]), ("/lib/libSystem.so", &exec[..])];
    assert_eq!(install_all(cpu.mem(), mods).unwrap(), vec![AT + 4 * PATCH_AT as u32]);
    // Patched code no longer matches: a second pass changes nothing.
    assert_eq!(install(cpu.mem(), CODE, 0x1000).unwrap(), None);
    // Unreadable code is an error.
    let gone = [Segment { start: 0x1000, len: 0x1000, prot: PF_X }];
    assert!(install_all(cpu.mem(), [("libSystem.so", &gone[..])]).is_err());
}

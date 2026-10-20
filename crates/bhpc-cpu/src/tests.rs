//! Tests drive the JIT with hand-assembled ARM/Thumb code.
use super::*;
use std::sync::{Arc as StdArc, Mutex};

const CODE: u32 = 0x2000;
const STACK_TOP: u32 = 0x9000;
const DATA: u32 = 0x4000;

fn setup_with(fastmem: bool) -> Box<Cpu> {
    let mem = Arc::new(GuestMemory::new().unwrap());
    mem.map(CODE, 0x2000).unwrap();
    mem.map(DATA, 0x1000).unwrap();
    mem.map(0x8000, 0x1000).unwrap();
    let mut cpu = Cpu::with_options(
        mem,
        CpuOptions { fastmem, code_cache_bytes: 8 << 20, cycle_counting: false, ..CpuOptions::default() },
    )
    .unwrap();
    cpu.set_sp(STACK_TOP);
    cpu
}

fn setup() -> Box<Cpu> {
    setup_with(true)
}

/// Like [`setup_with`] with full control over the JIT options.
fn setup_opts(opts: CpuOptions) -> Box<Cpu> {
    let mem = Arc::new(GuestMemory::new().unwrap());
    mem.map(CODE, 0x2000).unwrap();
    mem.map(DATA, 0x1000).unwrap();
    mem.map(0x8000, 0x1000).unwrap();
    let mut cpu = Cpu::with_options(mem, CpuOptions { code_cache_bytes: 8 << 20, ..opts }).unwrap();
    cpu.set_sp(STACK_TOP);
    cpu
}

/// Every fastmem / inline-exclusive combination.
fn exclusive_variants() -> [CpuOptions; 3] {
    let d = CpuOptions::default();
    [
        CpuOptions { fastmem: true, fastmem_exclusive: true, ..d },
        CpuOptions { fastmem: true, fastmem_exclusive: false, ..d },
        CpuOptions { fastmem: false, fastmem_exclusive: true, ..d },
    ]
}

fn put(cpu: &Cpu, at: u32, words: &[u32]) {
    for (i, w) in words.iter().enumerate() {
        cpu.mem().write_u32(at + 4 * i as u32, *w).unwrap();
    }
}

/// `ldr r12,[pc]; bx r12; .word target`: an ARM tail-call trampoline.
fn tramp(cpu: &Cpu, at: u32, target: u32) {
    put(cpu, at, &[0xE59F_C000, 0xE12F_FF1C, target]);
}

const BX_LR: u32 = 0xE12F_FF1E;

#[test]
fn arm_function_returns_constant() {
    let mut cpu = setup();
    put(&cpu, CODE, &[0xE3A0_002A, BX_LR]); // mov r0,#42; bx lr
    let r = cpu.call(CODE, &[]).unwrap();
    assert_eq!(r.r0, 42);
    assert_eq!(cpu.sp(), STACK_TOP, "sp restored");
}

#[test]
fn thumb_function_via_bit0() {
    let mut cpu = setup();
    cpu.mem().write_u16(CODE + 0x100, 0x2007).unwrap(); // movs r0,#7
    cpu.mem().write_u16(CODE + 0x102, 0x4770).unwrap(); // bx lr
    assert_eq!(cpu.call(CODE + 0x101, &[]).unwrap().r0, 7);
    // Interworking back to ARM afterwards.
    put(&cpu, CODE, &[0xE3A0_002A, BX_LR]);
    assert_eq!(cpu.call(CODE, &[]).unwrap().r0, 42);
    assert_eq!(cpu.cpsr() & CPSR_T, 0);
}

#[test]
fn register_and_stack_arguments() {
    let mut cpu = setup();
    // r0 += r1 + r2 + r3 + [sp]; bx lr
    put(&cpu, CODE, &[0xE080_0001, 0xE080_0002, 0xE080_0003, 0xE59D_1000, 0xE080_0001, BX_LR]);
    assert_eq!(cpu.call(CODE, &[1, 2, 3, 4, 5]).unwrap().r0, 15);
    assert_eq!(cpu.sp(), STACK_TOP);
}

#[test]
fn call_result_helpers() {
    let r = CallResult { r0: 0x5555_6666, r1: 0x1111_2222 };
    assert_eq!(r.as_u64(), 0x1111_2222_5555_6666);
    let d = CallResult { r0: 0, r1: 0x3ff0_0000 };
    assert_eq!(d.as_f64(), 1.0);
}

#[test]
fn vfp_double_add_under_softfp() {
    let mut cpu = setup();
    put(&cpu, CODE, &[0xEC41_0B10, 0xEC43_2B11, 0xEE30_0B01, 0xEC51_0B10, BX_LR]);
    let a = 1.5f64.to_bits();
    let b = 2.25f64.to_bits();
    let r = cpu.call(CODE, &[a as u32, (a >> 32) as u32, b as u32, (b >> 32) as u32]).unwrap();
    assert_eq!(r.as_f64(), 3.75);
    cpu.set_d_bits(3, 0x4008_0000_0000_0000);
    assert_eq!(cpu.d_bits(3), 0x4008_0000_0000_0000);
    cpu.set_fpscr(0);
    assert_eq!(cpu.fpscr() & 0xf000_0000, 0);
}

#[test]
fn thunk_receives_aapcs_arguments_and_returns_u64() {
    let mut cpu = setup();
    let seen = StdArc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    let t = cpu
        .register_thunk(
            "mix",
            Box::new(move |cpu: &mut Cpu| {
                let mut a = cpu.args();
                let w = a.u32(cpu);
                let q = a.u64(cpu); // aligned pair r2:r3
                let c = a.u32(cpu); // stack
                let d = a.f64(cpu); // aligned stack slot
                let f = a.f32(cpu);
                s2.lock().unwrap().push((w, q, c, d, f));
                cpu.ret_u64(q + w as u64);
            }),
        )
        .unwrap();
    tramp(&cpu, CODE, t);
    let d = 2.5f64.to_bits();
    let args = [1, 0xdead, 0x44, 0x55, 7, 0, d as u32, (d >> 32) as u32, 1.5f32.to_bits()];
    let r = cpu.call(CODE, &args).unwrap();
    assert_eq!(r.as_u64(), 0x55_0000_0044u64 + 1);
    assert_eq!(seen.lock().unwrap()[0], (1, 0x55_0000_0044, 7, 2.5, 1.5));
}

#[test]
fn argument_cursor_edge_cases() {
    let mut cpu = setup();
    let out = StdArc::new(Mutex::new(Vec::<u64>::new()));
    let o = out.clone();
    let t = cpu
        .register_thunk(
            "edge",
            Box::new(move |cpu: &mut Cpu| {
                let mut a = cpu.args();
                // (u32, u32, u32, u64): r3 is skipped, the u64 goes to the stack.
                let v = [a.u32(cpu) as u64, a.u32(cpu) as u64, a.u32(cpu) as u64, a.u64(cpu)];
                // Past the end of everything mapped: reads yield 0 instead of panicking.
                a.stack = 0x7000_0000;
                let z = a.u32(cpu) as u64 + a.u64(cpu);
                o.lock().unwrap().extend_from_slice(&v);
                o.lock().unwrap().push(z);
                cpu.ret_f64(6.25);
            }),
        )
        .unwrap();
    tramp(&cpu, CODE, t);
    let r = cpu.call(CODE, &[1, 2, 3, 0, 0x1111_2222, 0x3333_4444]).unwrap();
    assert_eq!(r.as_f64(), 6.25);
    assert_eq!(*out.lock().unwrap(), vec![1, 2, 3, 0x3333_4444_1111_2222, 0]);
}

#[test]
fn thunk_registry_lookup_and_replacement() {
    let mut cpu = setup();
    assert_eq!(cpu.thunk_count(), 0);
    let a = cpu.register_thunk("a", Box::new(|c: &mut Cpu| c.ret_u32(1))).unwrap();
    let b = cpu.register_thunk("b", Box::new(|c: &mut Cpu| c.ret_u32(2))).unwrap();
    assert_eq!(b, a + THUNK_STRIDE);
    assert_eq!(cpu.thunk_addr("a"), Some(a));
    assert_eq!(cpu.thunk_addr("zz"), None);
    assert_eq!(cpu.thunk_name(b).as_deref(), Some("b"));
    assert_eq!(cpu.thunk_name(b + 1), None);
    assert_eq!(cpu.thunk_name(0x1000), None);
    assert_eq!(cpu.thunk_name(THUNK_BASE + THUNK_STRIDE * 5000), None);
    assert_eq!(cpu.thunk_name(THUNK_BASE).as_deref(), Some("<return>"));
    assert_eq!(cpu.thunk_count(), 2);
    // Calling a thunk address directly works (svc + bx lr).
    assert_eq!(cpu.call(a, &[]).unwrap().r0, 1);
    // Re-registering the same name rebinds in place.
    assert_eq!(cpu.register_thunk("a", Box::new(|c: &mut Cpu| c.ret_u32(10))).unwrap(), a);
    assert_eq!(cpu.call(a, &[]).unwrap().r0, 10);
    assert_eq!(cpu.call(b, &[]).unwrap().r0, 2, "closures persist across calls");
}

#[test]
fn thunk_page_fills_up() {
    let cpu = setup();
    for i in 0..MAX_THUNKS {
        cpu.register_thunk(&format!("f{i}"), Box::new(|_: &mut Cpu| {})).unwrap();
    }
    assert_eq!(cpu.register_thunk("one-too-many", Box::new(|_: &mut Cpu| {})), Err(CpuError::TooManyThunks));
    assert_eq!(cpu.thunk_count() as u32, MAX_THUNKS);
}

#[test]
fn host_can_stop_execution() {
    let mut cpu = setup();
    let t = cpu
        .register_thunk("boom", Box::new(|c: &mut Cpu| c.request_stop(Stop::Host("unimplemented import boom".into()))))
        .unwrap();
    assert_eq!(cpu.call(t, &[]), Err(Stop::Host("unimplemented import boom".into())));
}

#[test]
fn unknown_svc_stops() {
    let mut cpu = setup();
    put(&cpu, CODE, &[0xEF00_0005, BX_LR]); // svc #5 (no such thunk)
    assert_eq!(cpu.call(CODE, &[]), Err(Stop::BadSvc(5)));
    put(&cpu, CODE, &[0xEF00_0000 | 9000, BX_LR]); // beyond the table
    cpu.invalidate(CODE, 8);
    assert_eq!(cpu.call(CODE, &[]), Err(Stop::BadSvc(9000)));
}

#[test]
fn undefined_instruction_raises_exception() {
    let mut cpu = setup();
    put(&cpu, CODE, &[0xE7F0_00F0]); // udf #0
    assert_eq!(cpu.call(CODE, &[]), Err(Stop::Undefined { pc: CODE, word: 0xE7F0_00F0, thumb: false }));
}

#[test]
fn unpredictable_instruction_has_its_own_stop_reason() {
    let mut cpu = setup();
    put(&cpu, CODE, &[0xE180_0F90]); // strex r0,r0,[r0]: Rd == Rn is unpredictable
    assert_eq!(cpu.call(CODE, &[]), Err(Stop::Unpredictable { pc: CODE, word: 0xE180_0F90, thumb: false }));
}

#[test]
fn other_exceptions_keep_the_raw_kind() {
    let mut cpu = setup();
    put(&cpu, CODE, &[0xE120_0070]); // bkpt #0
    assert!(matches!(cpu.call(CODE, &[]), Err(Stop::Exception { pc: CODE, .. })));
}

#[test]
fn thumb_undefined_reports_16_and_32_bit_words() {
    let mut cpu = setup();
    cpu.mem().write_u16(CODE, 0xDE00).unwrap(); // udf #0 (T1)
    assert_eq!(cpu.call(CODE | 1, &[]), Err(Stop::Undefined { pc: CODE, word: 0xDE00, thumb: true }));
    cpu.mem().write_u16(CODE + 0x10, 0xF7F0).unwrap(); // udf.w #0 (T2)
    cpu.mem().write_u16(CODE + 0x12, 0xA000).unwrap();
    assert_eq!(
        cpu.call((CODE + 0x10) | 1, &[]),
        Err(Stop::Undefined { pc: CODE + 0x10, word: 0xF7F0_A000, thumb: true })
    );
}

fn check_faults(fastmem: bool) {
    let mut cpu = setup_with(fastmem);
    put(&cpu, CODE, &[0xE591_0000, BX_LR]); // ldr r0,[r1]
    let call = |cpu: &mut Cpu| {
        cpu.set_reg(1, 0x0005_0000);
        cpu.set_pc(CODE);
        cpu.set_reg(14, RETURN_ADDR);
        cpu.run()
    };
    assert_eq!(call(&mut cpu), Stop::Fault(Fault { addr: 0x0005_0000, size: 4, write: false }));
    put(&cpu, CODE + 0x100, &[0xE581_0000, BX_LR]); // str r0,[r1]
    cpu.invalidate(CODE, 0x200);
    cpu.set_reg(1, 0x0006_0000);
    cpu.set_pc(CODE + 0x100);
    cpu.set_reg(14, RETURN_ADDR);
    assert_eq!(cpu.run(), Stop::Fault(Fault { addr: 0x0006_0000, size: 4, write: true }));
}

#[test]
fn unmapped_access_faults_with_fastmem() {
    check_faults(true);
}

#[test]
fn unmapped_access_faults_through_callbacks() {
    check_faults(false);
}

#[test]
fn stop_display_strings() {
    let all = [
        Stop::Returned,
        Stop::Fault(Fault { addr: 1, size: 4, write: true }),
        Stop::Fault(Fault { addr: 1, size: 4, write: false }),
        Stop::Exception { pc: 4, kind: 9 },
        Stop::Undefined { pc: 4, word: 0xE7F0_00F0, thumb: false },
        Stop::Unpredictable { pc: 4, word: 0xDE00, thumb: true },
        Stop::Fallback { pc: 8 },
        Stop::BadSvc(3),
        Stop::Host("h".into()),
        Stop::Nested,
        Stop::Halted(2),
    ];
    let text: Vec<String> = all.iter().map(|s| s.to_string()).collect();
    assert_eq!(text[0], "returned");
    assert!(text[1].contains("write") && text[2].contains("read"));
    assert!(text[3].contains("exception 9"));
    assert_eq!(text[4], "undefined arm instruction 0xe7f000f0 at pc 0x00000004");
    assert_eq!(text[5], "unpredictable thumb instruction 0x0000de00 at pc 0x00000004");
    assert!(text[6].contains("fallback"));
    assert!(text[7].contains("svc #3"));
    assert!(text[8].contains("h"));
    assert!(text[9].contains("nested"));
    assert!(text[10].contains("halted"));
    let e: &dyn std::error::Error = &all[0];
    assert_eq!(e.to_string(), "returned");
    assert!(CpuError::TooManyThunks.to_string().contains("full"));
    assert!(CpuError::TooManyCpus.to_string().contains("threads"));
    assert!(CpuError::from(MemError::OutOfSpace).to_string().contains("exhausted"));
}

#[test]
fn second_cpu_on_same_memory_is_rejected() {
    let cpu = setup();
    let again = Cpu::new(cpu.mem().clone());
    assert!(matches!(again, Err(CpuError::Mem(MemError::Overlap { .. }))));
}

#[test]
fn stack_argument_push_failure_is_reported() {
    let mut cpu = setup();
    put(&cpu, CODE, &[BX_LR]);
    cpu.set_sp(0x7000_0000);
    match cpu.call(CODE, &[1, 2, 3, 4, 5]) {
        Err(Stop::Host(m)) => assert!(m.contains("stack argument")),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn cache_invalidation_and_clear() {
    let mut cpu = setup();
    put(&cpu, CODE, &[0xE3A0_0001, BX_LR]);
    assert_eq!(cpu.call(CODE, &[]).unwrap().r0, 1);
    put(&cpu, CODE, &[0xE3A0_0002, BX_LR]);
    cpu.invalidate(CODE, 8);
    assert_eq!(cpu.call(CODE, &[]).unwrap().r0, 2);
    put(&cpu, CODE, &[0xE3A0_0003, BX_LR]);
    cpu.clear_cache();
    assert_eq!(cpu.call(CODE, &[]).unwrap().r0, 3);
}

#[test]
fn invalidating_while_running_resumes_with_fresh_code() {
    // A cacheflush thunk rewrites code it is about to return into; the run loop resumes by itself.
    let mut cpu = setup();
    let value = StdArc::new(AtomicU32::new(1));
    let v2 = value.clone();
    let t = cpu
        .register_thunk(
            "flush",
            Box::new(move |c: &mut Cpu| {
                c.mem().write_u32(CODE + 0x40, 0xE3A0_0000 | v2.load(Ordering::SeqCst)).unwrap();
                c.invalidate(CODE + 0x40, 4);
            }),
        )
        .unwrap();
    put(&cpu, CODE, &[0xE92D_4000, 0xE59F_C00C, 0xE12F_FF3C, 0xEB00_000B, 0xE49D_F004, 0, t]);
    put(&cpu, CODE + 0x40, &[0xE3A0_0000, BX_LR]); // mov r0,#n; bx lr
    assert_eq!(cpu.call(CODE, &[]).unwrap().r0, 1);
    value.store(2, Ordering::SeqCst);
    assert_eq!(cpu.call(CODE, &[]).unwrap().r0, 2, "stale translation was dropped");
}

#[test]
fn memory_callbacks_cover_all_access_sizes() {
    // fastmem off: every guest access goes through the Rust callbacks.
    let mut cpu = setup_with(false);
    // strb r1,[r0]; strh r1,[r0,#2]; str r1,[r0,#4]; strd r2,r3,[r0,#8]; vldr d0,[r0,#8]; vstr d0,[r0,#16]; bx lr
    put(&cpu, CODE, &[0xE5C0_1000, 0xE1C0_10B2, 0xE580_1004, 0xE1C0_20F8, 0xED90_0B02, 0xED80_0B04, BX_LR]);
    cpu.call(CODE, &[DATA, 0xAABB_CCDD, 0x1122_3344, 0x5566_7788]).unwrap();
    let m = cpu.mem();
    assert_eq!(m.read_u8(DATA).unwrap(), 0xDD);
    assert_eq!(m.read_u16(DATA + 2).unwrap(), 0xCCDD);
    assert_eq!(m.read_u32(DATA + 4).unwrap(), 0xAABB_CCDD);
    assert_eq!(m.read_u64(DATA + 8).unwrap(), 0x5566_7788_1122_3344);
    assert_eq!(m.read_u64(DATA + 16).unwrap(), 0x5566_7788_1122_3344);
    // ldrb r1,[r0]; ldrh r2,[r0,#2]; ldr r3,[r0,#4]; str r1,[r0,#32]; str r2,[r0,#36]; str r3,[r0,#40]; bx lr
    put(&cpu, CODE + 0x100, &[0xE5D0_1000, 0xE1D0_20B2, 0xE590_3004, 0xE580_1020, 0xE580_2024, 0xE580_3028, BX_LR]);
    cpu.call(CODE + 0x100, &[DATA]).unwrap();
    let m = cpu.mem();
    assert_eq!(m.read_u32(DATA + 32).unwrap(), 0xDD);
    assert_eq!(m.read_u32(DATA + 36).unwrap(), 0xCCDD);
    assert_eq!(m.read_u32(DATA + 40).unwrap(), 0xAABB_CCDD);
}

#[test]
fn pc_lr_and_registers_are_accessible() {
    let mut cpu = setup();
    cpu.set_reg(5, 123);
    assert_eq!(cpu.reg(5), 123);
    cpu.set_pc(0x2001);
    assert_eq!(cpu.pc(), 0x2000);
    assert_ne!(cpu.cpsr() & CPSR_T, 0);
    cpu.set_pc(0x2000);
    assert_eq!(cpu.cpsr() & CPSR_T, 0);
    cpu.set_reg(14, 0x1234);
    assert_eq!(cpu.lr(), 0x1234);
}

#[test]
fn run_without_call_reports_halt_when_jit_stops_itself() {
    // A plain `run` from the return stub finishes immediately with Returned.
    let mut cpu = setup();
    cpu.set_pc(RETURN_ADDR);
    assert_eq!(cpu.run(), Stop::Returned);
}

#[test]
fn tail_call_redirects_the_return_path() {
    let mut cpu = setup();
    // Guest target: mov r0,#77; bx lr
    put(&cpu, CODE, &[0xE3A0_004D, BX_LR]);
    let t = cpu.register_thunk("jump", Box::new(|c: &mut Cpu| c.tail_call(CODE))).unwrap();
    assert_eq!(cpu.call(t, &[]).unwrap().r0, 77);
}

#[test]
fn thunk_tracing_does_not_disturb_execution() {
    let mut cpu = setup();
    cpu.set_trace_thunks(true);
    let a = cpu.register_thunk("traced", Box::new(|c: &mut Cpu| c.ret_u32(5))).unwrap();
    assert_eq!(cpu.call(a, &[]).unwrap().r0, 5);
    put(&cpu, CODE, &[0xEF00_0007, BX_LR]); // unknown slot is traced as "?"
    assert_eq!(cpu.call(CODE, &[]), Err(Stop::BadSvc(7)));
    put(&cpu, CODE + 0x40, &[0xEF00_0000 | 20000, BX_LR]);
    assert_eq!(cpu.call(CODE + 0x40, &[]), Err(Stop::BadSvc(20000)));
}

#[test]
fn tick_budget_stops_an_infinite_loop() {
    let mem = Arc::new(GuestMemory::new().unwrap());
    mem.map(CODE, 0x1000).unwrap();
    let mut cpu = Cpu::with_options(
        mem,
        CpuOptions { fastmem: true, code_cache_bytes: 8 << 20, cycle_counting: true, ..CpuOptions::default() },
    )
    .unwrap();
    put(&cpu, CODE, &[0xEAFF_FFFE]); // b .
    cpu.set_ticks(10_000);
    cpu.set_sp(0x8000_0000);
    match cpu.call(CODE, &[]) {
        Err(Stop::Halted(_)) => {}
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(cpu.pc(), CODE);
    assert!(cpu.ticks() < 10_000);
}

#[test]
fn traced_call_visits_every_instruction() {
    let mut cpu = setup();
    put(&cpu, CODE, &[0xE3A0_0001, 0xE280_0001, BX_LR]); // mov r0,#1; add r0,r0,#1; bx lr
    let mut pcs = Vec::new();
    let r = cpu.call_traced(CODE, &[], &mut |c: &Cpu| pcs.push(c.pc())).unwrap();
    assert_eq!(r.r0, 2);
    assert_eq!(&pcs[..3], &[CODE, CODE + 4, CODE + 8]);
    assert!(*pcs.last().unwrap() >= RETURN_ADDR, "tracing includes the return stub");
    // A stop (fault) ends tracing; invalidation-style halts are reported raw.
    put(&cpu, CODE + 0x40, &[0xE591_0000]); // ldr r0,[r1] with r1 unmapped
    cpu.set_reg(1, 0x0009_0000);
    assert!(matches!(cpu.call_traced(CODE + 0x40, &[], &mut |_| {}), Err(Stop::Fault(_))));
    // The CPU's own clear_cache is not replayed to it when a thunk returns, so a traced call still completes.
    let nop = cpu.register_thunk("nop", Box::new(|_: &mut Cpu| {})).unwrap();
    cpu.clear_cache();
    cpu.call_traced(nop, &[], &mut |_| {}).unwrap();
    let t = cpu.register_thunk("flush", Box::new(|c: &mut Cpu| c.invalidate(0x2000, 64))).unwrap();
    assert!(matches!(cpu.call_traced(t, &[], &mut |_| {}), Err(Stop::Halted(_))));
    // Nested use is refused here as well.
    let got = StdArc::new(Mutex::new(None));
    let g = got.clone();
    let t2 = cpu
        .register_thunk(
            "again",
            Box::new(move |c: &mut Cpu| *g.lock().unwrap() = Some(c.call_traced(0x2000, &[], &mut |_| {}))),
        )
        .unwrap();
    cpu.call_traced(t2, &[], &mut |_| {}).unwrap();
    assert_eq!(got.lock().unwrap().take(), Some(Err(Stop::Nested)));
}

#[test]
fn exclusive_load_store_pairs_succeed() {
    // ldrex/strex increment loops in every width, plus a failing strex to an unmapped address.
    for opts in exclusive_variants() {
        let fastmem = (opts.fastmem, opts.fastmem_exclusive);
        let mut cpu = setup_opts(opts);
        cpu.mem().write_u64(DATA, 0x0000_0001_0000_0041).unwrap();
        put(
            &cpu,
            CODE,
            &[
                0xE190_1F9F, // ldrex r1,[r0]
                0xE281_1001, // add r1,r1,#1
                0xE180_2F91, // strex r2,r1,[r0]
                0xE352_0000, // cmp r2,#0
                0x1AFF_FFFA, // bne retry
                0xE1D0_3F9F, // ldrexb r3,[r0]
                0xE283_3001, // add r3,r3,#1
                0xE1C0_4F93, // strexb r4,r3,[r0]
                0xE1F0_5F9F, // ldrexh r5,[r0]
                0xE285_5001, // add r5,r5,#1
                0xE1E0_6F95, // strexh r6,r5,[r0]
                0xE1B0_8F9F, // ldrexd r8,r9,[r0]
                0xE298_8001, // adds r8,r8,#1
                0xE2A9_9000, // adc r9,r9,#0
                0xE1A0_A004, // mov r10,r4 (keep strexb status in r10)
                0xE1A0_7006, // mov r7,r6
                0xE1A0_0002, // mov r0,r2 (strex status)
                BX_LR,
            ],
        );
        let r = cpu.call(CODE, &[DATA]).unwrap();
        assert_eq!(r.r0, 0, "strex succeeded (fastmem, inline={fastmem:?})");
        assert_eq!(cpu.mem().read_u32(DATA).unwrap(), 0x44, "word, byte and halfword increments");
    }
}

#[test]
fn exclusive_store_to_unmapped_memory_faults() {
    for opts in exclusive_variants() {
        let mut cpu = setup_opts(opts);
        put(&cpu, CODE, &[0xE190_1F9F, 0xE180_2F91, BX_LR]); // ldrex r1,[r0]; strex r2,r1,[r0]
        cpu.mem().map(0x6_0000, 0x1000).unwrap();
        // ldrex reads fine from a mapped page; unmapping is not possible, so fault via an unmapped read instead.
        assert_eq!(
            cpu.call(CODE, &[0x0007_0000]),
            Err(Stop::Fault(Fault { addr: 0x0007_0000, size: 4, write: false })),
            "{opts:?}"
        );
        // The faulting access was recompiled to the callback path; a mapped address still works afterwards.
        cpu.mem().write_u32(0x6_0000, 7).unwrap();
        assert_eq!(cpu.call(CODE, &[0x0006_0000]).unwrap().r0, 0x6_0000, "{opts:?}");
    }
}

#[test]
fn exclusive_monitor_is_shared_with_inline_exclusives() {
    // A store-exclusive fails when another CPU wrote the location after this CPU's ldrex (both inline).
    let mut a = setup();
    let b = a.new_thread().unwrap();
    assert!(a.options().fastmem_exclusive && b.options().fastmem_exclusive);
    // ldrex r1,[r0]; svc #n (thunk: other CPU's strex); strex r2,r1,[r0]; mov r0,r2; bx lr
    let b = StdArc::new(Mutex::new(b));
    let b2 = b.clone();
    let t = a
        .register_shared_thunk(
            "other_strex",
            Arc::new(move |_c: &mut Cpu| {
                let mut b = b2.lock().unwrap();
                b.set_sp(0x8800);
                // ldrex r1,[r0]; add r1,r1,#1; strex r2,r1,[r0]; mov r0,r2; bx lr
                put(&b, CODE + 0x200, &[0xE190_1F9F, 0xE281_1001, 0xE180_2F91, 0xE1A0_0002, BX_LR]);
                assert_eq!(b.call(CODE + 0x200, &[DATA]).unwrap().r0, 0, "other CPU's strex succeeds");
            }),
        )
        .unwrap();
    put(
        &a,
        CODE,
        &[0xE92D_4010, 0xE1A0_4000, 0xE194_1F9F, 0xE59F_C00C, 0xE12F_FF3C, 0xE184_2F91, 0xE1A0_0002, 0xE8BD_8010, t],
    );
    // push {r4,lr}; mov r4,r0; ldrex r1,[r4]; ldr r12,=t; blx r12; strex r2,r1,[r4]; mov r0,r2; pop {r4,pc}
    a.mem().write_u32(DATA, 5).unwrap();
    assert_eq!(a.call(CODE, &[DATA]).unwrap().r0, 1, "strex after a foreign write fails");
    assert_eq!(a.mem().read_u32(DATA).unwrap(), 6);
}

#[test]
fn jit_tuning_switch_and_flags() {
    assert!(tuning_enabled(None));
    assert!(tuning_enabled(Some(std::ffi::OsStr::new("0"))));
    assert!(!tuning_enabled(Some(std::ffi::OsStr::new("1"))));
    let d = CpuOptions::default();
    let all = CpuOptions { fastmem: true, fastmem_exclusive: true, fast_fp: true, ..d };
    assert_eq!(all.jit_flags(), ffi::JIT_FASTMEM_EXCLUSIVE | ffi::JIT_FAST_FP);
    assert_eq!(CpuOptions { fastmem: false, ..all }.jit_flags(), ffi::JIT_FAST_FP, "inline exclusives need fastmem");
    assert_eq!(CpuOptions { fastmem_exclusive: false, fast_fp: false, ..all }.jit_flags(), 0);
    assert_eq!(setup_opts(all).options().jit_flags(), all.jit_flags());
}

#[test]
fn fast_fp_keeps_ordinary_float_results() {
    // (a + b) * b in VFP, and a + b with NEON, for normal and subnormal inputs.
    let vfp = [0xEE00_0A10, 0xEE00_1A90, 0xEE30_1A20, 0xEE61_1A20, 0xEE11_0A90, BX_LR];
    let neon = [0xEEA0_0B10, 0xEEA2_1B10, 0xF200_4D42, 0xEE14_0B10, BX_LR];
    for fast_fp in [false, true] {
        let mut cpu = setup_opts(CpuOptions { fast_fp, ..CpuOptions::default() });
        put(&cpu, CODE, &vfp);
        put(&cpu, CODE + 0x100, &neon);
        let f = |cpu: &mut Cpu, at: u32, a: f32, b: f32| {
            f32::from_bits(cpu.call(at, &[a.to_bits(), b.to_bits()]).unwrap().r0)
        };
        assert_eq!(f(&mut cpu, CODE, 1.5, 2.25), (1.5f32 + 2.25) * 2.25);
        assert_eq!(f(&mut cpu, CODE, -3.0e7, 0.125), (-3.0e7f32 + 0.125) * 0.125);
        assert!(f(&mut cpu, CODE, f32::NAN, 1.0).is_nan(), "NaN stays NaN (payload may differ)");
        assert_eq!(f(&mut cpu, CODE + 0x100, 1.5, 2.25), 3.75);
        let tiny = f32::from_bits(0x0000_1000); // subnormal
                                                // ARM's "standard FPSCR value" flushes NEON subnormals; fast_fp honours the guest FPSCR (FZ clear) instead.
        let neon_tiny = f(&mut cpu, CODE + 0x100, tiny, 0.0);
        assert_eq!(neon_tiny, if fast_fp { tiny } else { 0.0 }, "fast_fp={fast_fp}");
    }
}

// ---------- nested host-to-guest calls ----------

use std::sync::atomic::{AtomicU32, Ordering};

/// `qsort`-style thunk: insertion sort of 32-bit elements through a guest comparator.
fn register_qsort(cpu: &Cpu) -> u32 {
    cpu.register_shared_thunk(
        "qsort",
        Arc::new(|c: &mut Cpu| {
            let mut a = c.args();
            let (base, n, size, cmp) = (a.u32(c), a.u32(c), a.u32(c), a.u32(c));
            assert_eq!(size, 4);
            for i in 1..n {
                let mut j = i;
                while j > 0 {
                    let (p, q) = (base + (j - 1) * 4, base + j * 4);
                    let Some(r) = c.call_or_stop(cmp, &[p, q]) else { return };
                    if (r.r0 as i32) <= 0 {
                        break;
                    }
                    let (x, y) = (c.mem().read_u32(p).unwrap(), c.mem().read_u32(q).unwrap());
                    c.mem().write_u32(p, y).unwrap();
                    c.mem().write_u32(q, x).unwrap();
                    j -= 1;
                }
            }
        }),
    )
    .unwrap()
}

/// `int cmp(const int *a, const int *b) { return *a - *b; }`
const CMP: [u32; 4] = [0xE590_0000, 0xE591_1000, 0xE040_0001, BX_LR];

#[test]
fn thunk_can_call_guest_comparator() {
    for fastmem in [true, false] {
        let mut cpu = setup_with(fastmem);
        let q = register_qsort(&cpu);
        put(&cpu, CODE + 0x100, &CMP);
        put(&cpu, DATA, &[5, 3, 9, 1, 7, 3]);
        cpu.set_reg(4, 0x4444);
        let sp = cpu.sp();
        cpu.call(q, &[DATA, 6, 4, CODE + 0x100]).unwrap();
        let out: Vec<u32> = (0..6).map(|i| cpu.mem().read_u32(DATA + 4 * i).unwrap()).collect();
        assert_eq!(out, [1, 3, 3, 5, 7, 9]);
        assert_eq!((cpu.sp(), cpu.reg(4)), (sp, 0x4444));
    }
}

#[test]
fn nested_call_runs_on_the_parents_stack_and_shares_the_processor_id() {
    let mut cpu = setup();
    let pid = cpu.processor_id();
    let seen = StdArc::new(Mutex::new((0, 0, 0)));
    let s2 = seen.clone();
    // Guest: str sp -> [DATA]; mov r0,#9 ; bx lr
    put(&cpu, CODE, &[0xE59F_1008, 0xE581_D000, 0xE3A0_0009, BX_LR, DATA]);
    let t = cpu
        .register_thunk(
            "outer",
            Box::new(move |c: &mut Cpu| {
                let sp = c.sp();
                let r = c.call(CODE, &[]).unwrap();
                *s2.lock().unwrap() = (sp, c.mem().read_u32(DATA).unwrap(), r.r0);
                c.ret_u32(r.r0 + 1);
            }),
        )
        .unwrap();
    assert_eq!(cpu.call(t, &[]).unwrap().r0, 10);
    let (sp, child_sp, r) = *seen.lock().unwrap();
    assert_eq!((sp, child_sp, r), (STACK_TOP, STACK_TOP, 9));
    assert_eq!(pid, cpu.processor_id());
}

#[test]
fn nested_calls_nest_up_to_the_depth_limit() {
    let mut cpu = setup();
    let count = StdArc::new(AtomicU32::new(0));
    let addr = StdArc::new(AtomicU32::new(0));
    let (c2, a2) = (count.clone(), addr.clone());
    let t = cpu
        .register_shared_thunk(
            "rec",
            Arc::new(move |c: &mut Cpu| {
                c2.fetch_add(1, Ordering::SeqCst);
                let _ = c.call_or_stop(a2.load(Ordering::SeqCst), &[]);
            }),
        )
        .unwrap();
    addr.store(t, Ordering::SeqCst);
    assert_eq!(cpu.call(t, &[]), Err(Stop::Nested));
    assert_eq!(count.load(Ordering::SeqCst), MAX_NEST_DEPTH + 1);
    // The chain of children is reusable afterwards.
    count.store(0, Ordering::SeqCst);
    assert_eq!(cpu.call(t, &[]), Err(Stop::Nested));
    assert_eq!(count.load(Ordering::SeqCst), MAX_NEST_DEPTH + 1);
}

#[test]
fn serial_thunk_cannot_reenter_itself() {
    let mut cpu = setup();
    let addr = StdArc::new(AtomicU32::new(0));
    let a2 = addr.clone();
    let t = cpu
        .register_thunk(
            "serial",
            Box::new(move |c: &mut Cpu| {
                let _ = c.call_or_stop(a2.load(Ordering::SeqCst), &[]);
            }),
        )
        .unwrap();
    addr.store(t, Ordering::SeqCst);
    assert_eq!(cpu.call(t, &[]), Err(Stop::Nested));
    // The reentrancy guard was released, so the thunk is usable again (and stops again).
    assert_eq!(cpu.call(t, &[]), Err(Stop::Nested));
}

#[test]
fn failures_in_nested_calls_propagate() {
    let mut cpu = setup();
    let q = register_qsort(&cpu);
    put(&cpu, CODE + 0x100, &[0xE590_0000, 0xE591_1000, 0xE040_0001, BX_LR]);
    // Element pointers run off the mapped data page: the comparator's load faults in the child.
    let r = cpu.call(q, &[0x0FFC, 3, 4, CODE + 0x100]);
    assert!(matches!(r, Err(Stop::Fault(_))), "{r:?}");
    // An undefined comparator reports the instruction.
    put(&cpu, CODE + 0x200, &[0xE7F0_00F0]);
    put(&cpu, DATA, &[2, 1]);
    assert_eq!(
        cpu.call(q, &[DATA, 2, 4, CODE + 0x200]),
        Err(Stop::Undefined { pc: CODE + 0x200, word: 0xE7F0_00F0, thumb: false })
    );
}

#[test]
fn stop_regs_capture_the_failing_registers_including_nested_ones() {
    let mut cpu = setup();
    assert_eq!(cpu.stop_regs(), None);
    // Top level: undefined instruction with a recognisable r5 and live sp/lr.
    put(&cpu, CODE, &[0xE3A0_5077, 0xE7F0_00F0]); // mov r5,#0x77 ; udf
    assert!(cpu.call(CODE, &[]).is_err());
    let r = cpu.stop_regs().unwrap();
    assert_eq!(r[5], 0x77);
    assert!((CODE + 4..=CODE + 8).contains(&r[15]), "{:#x}", r[15]);
    assert_eq!(r[14], RETURN_ADDR);
    // A later success clears it.
    put(&cpu, CODE, &[BX_LR, BX_LR]);
    cpu.clear_cache();
    cpu.call(CODE, &[]).unwrap();
    assert_eq!(cpu.stop_regs(), None);
    // Nested failure: the comparator's registers win over the qsort caller's.
    let q = register_qsort(&cpu);
    put(&cpu, CODE + 0x200, &[0xE3A0_6055, 0xE7F0_00F0]);
    put(&cpu, DATA, &[2, 1]);
    assert!(cpu.call(q, &[DATA, 2, 4, CODE + 0x200]).is_err());
    let r = cpu.stop_regs().unwrap();
    assert_eq!(r[6], 0x55);
    assert!((CODE + 0x204..=CODE + 0x208).contains(&r[15]), "{:#x}", r[15]);
}

#[test]
fn nested_call_shares_the_cycle_budget() {
    let mem = Arc::new(GuestMemory::new().unwrap());
    mem.map(CODE, 0x1000).unwrap();
    let mut cpu = Cpu::with_options(
        mem,
        CpuOptions { fastmem: true, code_cache_bytes: 8 << 20, cycle_counting: true, ..CpuOptions::default() },
    )
    .unwrap();
    cpu.set_sp(0x8000_0000);
    put(&cpu, CODE, &[0xE3A0_0001, BX_LR]); // mov r0,#1; bx lr
    put(&cpu, CODE + 0x100, &[0xEAFF_FFFE]); // b .
    let t = cpu
        .register_thunk(
            "n",
            Box::new(|c: &mut Cpu| {
                let t0 = c.ticks();
                let r = c.call_or_stop(CODE, &[]).unwrap();
                assert_eq!(r.r0, 1);
                assert!(c.ticks() < t0, "child consumption is charged to the parent");
                let _ = c.call_or_stop(CODE + 0x100, &[]);
            }),
        )
        .unwrap();
    cpu.set_ticks(100_000);
    assert!(matches!(cpu.call(t, &[]), Err(Stop::Halted(_))));
}

#[test]
fn nested_call_inherits_thunk_tracing_and_fpscr() {
    let mut cpu = setup();
    cpu.set_trace_thunks(true);
    cpu.set_fpscr(0x0300_0000);
    // Guest: vmrs r0,fpscr; bx lr
    put(&cpu, CODE, &[0xEEF1_0A10, BX_LR]);
    let inner = cpu.register_thunk("inner", Box::new(|c: &mut Cpu| c.ret_u32(1))).unwrap();
    put(&cpu, CODE + 0x20, &[0xE92D_4000, 0xE59F_C00C, 0xE12F_FF3C, 0xE8BD_8000, 0, inner]);
    let t = cpu
        .register_thunk(
            "outer",
            Box::new(move |c: &mut Cpu| {
                let r = c.call_or_stop(CODE, &[]).unwrap();
                assert_eq!(r.r0 & 0x0300_0000, 0x0300_0000);
                // The traced thunk call happens inside the child.
                assert_eq!(c.call_or_stop(inner, &[]).unwrap().r0, 1);
            }),
        )
        .unwrap();
    cpu.call(t, &[]).unwrap();
}

#[test]
fn cache_maintenance_from_a_nested_call_reaches_the_whole_chain() {
    let mut cpu = setup();
    put(&cpu, CODE, &[0xE3A0_0001, BX_LR]);
    let flush = cpu
        .register_thunk(
            "flush",
            Box::new(|c: &mut Cpu| {
                c.invalidate(CODE, 8);
                c.clear_cache();
            }),
        )
        .unwrap();
    let t = cpu
        .register_thunk(
            "outer",
            Box::new(move |c: &mut Cpu| {
                c.call_or_stop(flush, &[]);
                // The parent's cached translation of CODE was dropped as well.
                c.mem().write_u32(CODE, 0xE3A0_0007).unwrap();
                c.invalidate(CODE, 8);
                let r = c.call_or_stop(CODE, &[]).unwrap();
                c.ret_u32(r.r0);
            }),
        )
        .unwrap();
    assert_eq!(cpu.call(CODE, &[]).unwrap().r0, 1);
    assert_eq!(cpu.call(t, &[]).unwrap().r0, 7);
    assert_eq!(cpu.call(CODE, &[]).unwrap().r0, 7);
}

// ---------- shared thunk table, guest threads ----------

#[test]
fn threads_share_memory_and_thunks_but_not_registers() {
    let mut a = setup();
    let t = a.register_thunk("one", Box::new(|c: &mut Cpu| c.ret_u32(1))).unwrap();
    let mut b = a.new_thread().unwrap();
    assert!(Arc::ptr_eq(a.table(), b.table()));
    assert_ne!(a.processor_id(), b.processor_id());
    assert_eq!(b.thunk_addr("one"), Some(t));
    // Thunks registered through either CPU are visible to both.
    let t2 = b.register_shared_thunk("two", Arc::new(|c: &mut Cpu| c.ret_u32(2))).unwrap();
    assert_eq!(a.thunk_addr("two"), Some(t2));
    assert_eq!(a.thunk_name(t2).as_deref(), Some("two"));
    assert_eq!(a.thunk_count(), 2);
    b.set_sp(0x8800);
    b.set_reg(4, 99);
    assert_eq!(a.sp(), STACK_TOP);
    assert_ne!(a.reg(4), 99);
    assert_eq!(b.call(t, &[]).unwrap().r0, 1);
    assert_eq!(a.call(t2, &[]).unwrap().r0, 2);
}

#[test]
fn processor_ids_are_recycled_and_bounded() {
    let main = setup();
    let first = main.new_thread().unwrap();
    let id = first.processor_id();
    drop(first);
    assert_eq!(main.new_thread().unwrap().processor_id(), id, "freed id is reused");
    let mut keep = Vec::new();
    for _ in 0..MAX_CPUS - 1 {
        keep.push(main.new_thread().unwrap());
    }
    assert_eq!(main.new_thread().err(), Some(CpuError::TooManyCpus));
    drop(keep);
    assert!(main.new_thread().is_ok());
}

#[test]
fn exclusive_monitor_is_shared_between_os_threads() {
    // Four guest threads hammer one counter with ldrex/strex loops; no increment may be lost.
    const PER_THREAD: u32 = 50_000;
    let main = setup();
    main.mem().map(0x10_0000, 0x10000).unwrap();
    put(
        &main,
        CODE,
        &[
            0xE190_2F9F, // retry: ldrex r2,[r0]
            0xE282_2001, // add r2,r2,#1
            0xE180_3F92, // strex r3,r2,[r0]
            0xE353_0000, // cmp r3,#0
            0x1AFF_FFFA, // bne retry
            0xE251_1001, // subs r1,r1,#1
            0x1AFF_FFF8, // bne retry
            BX_LR,
        ],
    );
    let handles: Vec<_> = (0..4u32)
        .map(|i| {
            let mut cpu = main.new_thread().unwrap();
            cpu.set_sp(0x10_0000 + 0x1000 * (i + 1));
            std::thread::spawn(move || cpu.call(CODE, &[DATA, PER_THREAD]).map(|_| ()))
        })
        .collect();
    for h in handles {
        h.join().unwrap().unwrap();
    }
    assert_eq!(main.mem().read_u32(DATA).unwrap(), 4 * PER_THREAD);
}

#[test]
fn thunks_run_from_several_threads() {
    let main = setup();
    let hits = StdArc::new(AtomicU32::new(0));
    let h2 = hits.clone();
    let shared = main
        .register_shared_thunk("shared", Arc::new(move |c: &mut Cpu| c.ret_u32(h2.fetch_add(1, Ordering::SeqCst))))
        .unwrap();
    let serial = main
        .register_thunk(
            "serial",
            Box::new({
                let mut n = 0u32;
                move |c: &mut Cpu| {
                    n += 1;
                    c.ret_u32(n);
                }
            }),
        )
        .unwrap();
    let handles: Vec<_> = (0..3u32)
        .map(|i| {
            let mut cpu = main.new_thread().unwrap();
            cpu.set_sp(STACK_TOP - 0x400 * (i + 1));
            std::thread::spawn(move || {
                for _ in 0..200 {
                    cpu.call(shared, &[]).unwrap();
                    cpu.call(serial, &[]).unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(hits.load(Ordering::SeqCst), 600);
    let mut main = main;
    assert_eq!(main.call(serial, &[]).unwrap().r0, 601, "serial thunk state survived and was never raced");
}

#[test]
fn a_panic_poisoned_thunk_stays_usable() {
    let mut cpu = setup();
    let table = cpu.table().clone();
    // Poison the table's lock from another thread, then keep using it.
    let t2 = table.clone();
    let _ = std::thread::spawn(move || {
        let _g = t2.thunk_count();
        panic!("boom");
    })
    .join();
    let a = cpu.register_thunk("p", Box::new(|c: &mut Cpu| c.ret_u32(3))).unwrap();
    assert_eq!(cpu.call(a, &[]).unwrap().r0, 3);
}

#[test]
fn untranslatable_instruction_requests_interpreter_fallback() {
    let mut cpu = setup();
    put(&cpu, CODE, &[0xF100_0000, BX_LR]); // cpsie-class encoding dynarmic wants interpreted
    assert_eq!(cpu.call(CODE, &[]), Err(Stop::Fallback { pc: CODE }));
    // The other interpreted forms (no backend may see an Interpret terminal: arm64 has none), each after a
    // translated instruction so the stop lands mid-block on the right pc.
    let interpreted = [
        0xE8D0_0003, // ldmia r0, {r0, r1}^ (user registers)
        0xE8D0_8001, // ldmia r0, {r0, pc}^ (exception return)
        0xE8C0_0003, // stmia r0, {r0, r1}^
        0xF890_0A00, // rfeia r0
        0xF8CD_0513, // srsia sp, #0x13
    ];
    for word in interpreted {
        put(&cpu, CODE, &[0xE3A0_0000 | 0x10, word, BX_LR]); // mov r0, #16 ; <word>
        cpu.clear_cache();
        assert_eq!(cpu.call(CODE, &[]), Err(Stop::Fallback { pc: CODE + 4 }), "{word:#010x}");
    }
    // Neighbouring encodings stay translated: a plain stm/ldm and an unconditional pld.
    // mov r0, #DATA ; stmia r0, {r1} ; pld [r0] ; ldmia r0, {r0}
    put(&cpu, CODE, &[0xE3A0_0901, 0xE880_0002, 0xF5D0_F000, 0xE890_0001, BX_LR]);
    cpu.clear_cache();
    assert_eq!(cpu.call(CODE, &[0, 7]).unwrap().r0, 7);
}

fn counting_cpu() -> Box<Cpu> {
    let mem = Arc::new(GuestMemory::new().unwrap());
    mem.map(CODE, 0x1000).unwrap();
    let mut cpu = Cpu::with_options(
        mem,
        CpuOptions { fastmem: true, code_cache_bytes: 8 << 20, cycle_counting: true, ..CpuOptions::default() },
    )
    .unwrap();
    cpu.set_sp(0x8000_0000);
    put(&cpu, CODE, &[0xEAFF_FFFE]); // b .
    cpu
}

#[test]
fn time_slices_call_the_hook_until_the_budget_is_gone() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let mut cpu = counting_cpu();
    let calls = StdArc::new(AtomicU32::new(0));
    let c2 = calls.clone();
    cpu.set_ticks(10_000);
    cpu.set_slice(
        1_000,
        Arc::new(move |_| {
            c2.fetch_add(1, Ordering::Relaxed);
            None
        }),
    );
    assert_eq!(cpu.ticks(), 10_000, "installing a policy keeps the budget");
    assert!(matches!(cpu.call(CODE, &[]), Err(Stop::Halted(_))));
    let n = calls.load(Ordering::Relaxed);
    assert!((8..=10).contains(&n), "{n} slices");
    assert_eq!(cpu.ticks(), 0);
    // A budget smaller than one slice never calls the hook.
    calls.store(0, Ordering::Relaxed);
    cpu.set_ticks(500);
    assert!(matches!(cpu.call(CODE, &[]), Err(Stop::Halted(_))));
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

#[test]
fn a_slice_hook_can_stop_the_guest() {
    let mut cpu = counting_cpu();
    cpu.set_ticks(1 << 40);
    cpu.set_slice(1_000, Arc::new(|_| Some(Stop::Host("enough".into()))));
    assert_eq!(cpu.call(CODE, &[]), Err(Stop::Host("enough".into())));
}

#[test]
fn threads_and_nested_calls_inherit_the_slice_policy() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let mut cpu = counting_cpu();
    let calls = StdArc::new(AtomicU32::new(0));
    let c2 = calls.clone();
    cpu.set_ticks(1 << 40);
    cpu.set_slice(
        1_000,
        Arc::new(move |_| {
            // Stop on the third slice, wherever it runs.
            let n = c2.fetch_add(1, Ordering::Relaxed) + 1;
            (n % 3 == 0).then(|| Stop::Host("third".into()))
        }),
    );
    let mut other = cpu.new_thread().unwrap();
    other.set_sp(0x7000_0000);
    other.set_ticks(1 << 40);
    assert_eq!(other.call(CODE, &[]), Err(Stop::Host("third".into())));
    let t = cpu
        .register_thunk(
            "spin_nested",
            Box::new(|c: &mut Cpu| {
                let _ = c.call_or_stop(CODE, &[]);
            }),
        )
        .unwrap();
    assert_eq!(cpu.call(t, &[]), Err(Stop::Host("third".into())));
    assert!(calls.load(Ordering::Relaxed) >= 6);
}

#[test]
fn cycle_counting_reflects_options() {
    assert!(!setup().cycle_counting());
    let mem = Arc::new(GuestMemory::new().unwrap());
    let cpu = Cpu::with_options(
        mem,
        CpuOptions { fastmem: false, code_cache_bytes: 8 << 20, cycle_counting: true, ..CpuOptions::default() },
    )
    .unwrap();
    assert!(cpu.cycle_counting());
}

#[test]
fn default_fastmem_matches_platform() {
    assert_eq!(CpuOptions::default().fastmem, !cfg!(target_os = "macos"));
}

/// A host gate for one thread that records what the CPU does with it.
struct LogGate {
    held: Mutex<bool>,
    log: Mutex<Vec<&'static str>>,
}

impl HostGate for LogGate {
    fn enter(&self) -> bool {
        let mut h = self.held.lock().unwrap();
        if *h {
            return false;
        }
        *h = true;
        self.log.lock().unwrap().push("enter");
        true
    }

    fn leave(&self) -> bool {
        let mut h = self.held.lock().unwrap();
        if !*h {
            return false;
        }
        *h = false;
        self.log.lock().unwrap().push("leave");
        true
    }
}

#[test]
fn the_host_gate_is_released_for_guest_code_and_taken_for_thunks() {
    let mut cpu = setup();
    assert!(!cpu.has_gate());
    let gate = StdArc::new(LogGate { held: Mutex::new(true), log: Mutex::new(Vec::new()) });
    assert!(cpu.set_gate(gate.clone()));
    assert!(cpu.has_gate());
    assert!(!cpu.set_gate(gate.clone()), "a gate is set once");
    put(&cpu, CODE, &[0xE3A0_0009, BX_LR]); // mov r0,#9; bx lr
    let g2 = gate.clone();
    let t = cpu
        .register_shared_thunk(
            "outer",
            StdArc::new(move |c: &mut Cpu| {
                assert!(*g2.held.lock().unwrap(), "thunks run holding the gate");
                // A nested guest call gives it up again while the guest code runs.
                let r = c.call(CODE, &[]).unwrap();
                assert!(*g2.held.lock().unwrap());
                c.ret_u32(r.r0 + 1);
            }),
        )
        .unwrap();
    assert_eq!(cpu.call(t, &[]).unwrap().r0, 10);
    assert_eq!(*gate.log.lock().unwrap(), ["leave", "enter", "leave", "enter", "leave", "enter"]);
    assert!(*gate.held.lock().unwrap());

    // A thread that does not hold the gate takes it only for the thunk.
    *gate.held.lock().unwrap() = false;
    gate.log.lock().unwrap().clear();
    let leaf = cpu.register_thunk("leaf", Box::new(|c: &mut Cpu| c.ret_u32(3))).unwrap();
    assert_eq!(cpu.call(leaf, &[]).unwrap().r0, 3);
    assert_eq!(*gate.log.lock().unwrap(), ["enter", "leave"]);
    assert!(!*gate.held.lock().unwrap());

    // A gate-free thunk runs without it; registering the name again clears the mark.
    assert!(!cpu.table().set_gate_free("nope"));
    assert!(cpu.table().set_gate_free("leaf"));
    gate.log.lock().unwrap().clear();
    assert_eq!(cpu.call(leaf, &[]).unwrap().r0, 3);
    assert!(gate.log.lock().unwrap().is_empty());
    assert_eq!(cpu.register_thunk("leaf", Box::new(|c: &mut Cpu| c.ret_u32(4))).unwrap(), leaf);
    assert_eq!(cpu.call(leaf, &[]).unwrap().r0, 4);
    assert_eq!(*gate.log.lock().unwrap(), ["enter", "leave"]);
}

#[test]
fn thunk_statistics_count_calls_and_host_time() {
    let mut cpu = setup();
    let a = cpu.register_thunk("a", Box::new(|c: &mut Cpu| c.ret_u32(1))).unwrap();
    let b = cpu
        .register_thunk(
            "b",
            Box::new(|c: &mut Cpu| {
                std::thread::sleep(std::time::Duration::from_millis(2));
                c.ret_u32(2)
            }),
        )
        .unwrap();
    cpu.call(a, &[]).unwrap();
    assert!(cpu.table().stats().is_empty(), "nothing is counted while statistics are off");
    cpu.table().set_stats(true);
    for _ in 0..3 {
        cpu.call(a, &[]).unwrap();
    }
    cpu.call(b, &[]).unwrap();
    assert!(cpu.table().set_gate_free("a"));
    cpu.table().set_stats(false);
    cpu.call(b, &[]).unwrap();
    let st = cpu.table().stats();
    assert_eq!(st.len(), 2);
    assert_eq!((st[0].name.as_str(), st[0].calls, st[0].free), ("b", 1, false));
    assert!(st[0].nanos >= 2_000_000);
    assert_eq!((st[1].name.as_str(), st[1].calls, st[1].free), ("a", 3, true));
    // Rebinding starts over.
    cpu.register_thunk("a", Box::new(|c: &mut Cpu| c.ret_u32(1))).unwrap();
    assert_eq!(cpu.table().stats().len(), 1);
    cpu.table().record(9999, 1);
}

#[test]
fn code_invalidations_reach_the_other_threads_cpus() {
    let mut a = setup();
    let mut b = a.new_thread().unwrap();
    b.set_sp(STACK_TOP - 0x800);
    put(&a, CODE, &[0xE3A0_0001, BX_LR]); // mov r0,#1
    assert_eq!(a.call(CODE, &[]).unwrap().r0, 1);
    assert_eq!(b.call(CODE, &[]).unwrap().r0, 1);
    // A patches the code and flushes it; B's translation goes too.
    put(&a, CODE, &[0xE3A0_0002, BX_LR]);
    a.invalidate(CODE, 8);
    assert_eq!(b.call(CODE, &[]).unwrap().r0, 2);
    // The same for a whole-cache clear made from inside a thunk.
    let clear = a
        .register_thunk(
            "clear",
            Box::new(|c: &mut Cpu| {
                c.mem().write_u32(CODE, 0xE3A0_0003).unwrap();
                c.clear_cache();
            }),
        )
        .unwrap();
    a.call(clear, &[]).unwrap();
    assert_eq!(b.call(CODE, &[]).unwrap().r0, 3);
    // A CPU that fell more than the log behind drops everything.
    put(&a, CODE, &[0xE3A0_0004, BX_LR]);
    for i in 0..=CODE_LOG as u32 {
        a.invalidate(DATA + 4 * i, 4);
    }
    assert_eq!(b.call(CODE, &[]).unwrap().r0, 4);
    // A new CPU starts with nothing to catch up on.
    let mut c = a.new_thread().unwrap();
    c.set_sp(STACK_TOP - 0x400);
    assert_eq!(c.call(CODE, &[]).unwrap().r0, 4);
}

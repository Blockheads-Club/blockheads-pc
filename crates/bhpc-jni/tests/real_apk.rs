//! Bring-up exploration against the user's own APK. Ignored by default: set `BHPC_APK` and run
//! `cargo test -p bhpc-jni --test real_apk --release -- --ignored --nocapture`.
//!
//! It loads the 25 libraries, runs their constructors, calls every `JNI_OnLoad` against the fake
//! JVM and prints the Java surface that was looked up (class/method names only; nothing from the
//! APK is stored).

use bhpc_bionic::{Config, Hle, State};
use bhpc_cpu::{Cpu, CpuOptions};
use bhpc_jni::{native_env, new_vm, MemAssets, Platform, Stage, VmConfig};
use bhpc_loader::apk::Apk;
use bhpc_loader::init::{run_constructors, CallFailure, GuestCaller, InitOptions, NoTrace};
use bhpc_loader::Linker;
use bhpc_mem::{GuestHeap, GuestMemory};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

struct Caller<'a>(&'a mut Cpu);

impl GuestCaller for Caller<'_> {
    fn call(&mut self, entry: u32, args: [u32; 3], _max: u64) -> Result<u32, CallFailure> {
        self.0
            .call(entry, &args)
            .map(|r| r.r0)
            .map_err(|s| CallFailure::Fault { pc: self.0.pc(), reason: s.to_string() })
    }
}

#[test]
#[ignore]
fn jni_onload_surface() {
    let path = PathBuf::from(std::env::var("BHPC_APK").expect("set BHPC_APK"));
    let mut apk = Apk::open(&path).unwrap();
    apk.verify(true).unwrap();

    let mem = Arc::new(GuestMemory::new().unwrap());
    let heap = GuestHeap::new(mem.clone(), 0x4000_0000, 64 << 20).unwrap();
    mem.map(0x7000_0000, 8 << 20).unwrap();
    let st = State::new(mem.clone(), heap, Config::default()).unwrap();
    let mut cpu = Cpu::with_options(mem.clone(), CpuOptions::default()).unwrap();
    cpu.set_sp(0x7080_0000);

    let mut linker = Linker::new(mem.clone());
    linker.set_trap_unresolved(true);
    let sum = {
        let mut hle = Hle::new(&mut cpu, st.clone());
        linker.load_all(&["libApplication.so"], &mut apk, &mut hle).unwrap()
    };
    let report = run_constructors(
        &linker.modules()[sum.range.clone()],
        &mut Caller(&mut cpu),
        &mut NoTrace,
        &InitOptions { keep_going: true, ..InitOptions::default() },
    );
    eprintln!("constructors: ran {} failures {}", report.ran, report.failures.len());
    for f in report.failures.iter().take(5) {
        eprintln!("  ctor failure: {f}");
    }

    let jheap = GuestHeap::new(mem.clone(), 0x6000_0000, 32 << 20).unwrap();
    let mut cfg = VmConfig::new(Platform::default(), Box::new(MemAssets::default()));
    cfg.lenient = true;
    let vm = new_vm(mem.clone(), Box::new(jheap), cfg);
    vm.set_log(|m| eprintln!("[jvm] {m}"));
    let _layout = vm.install(&cpu).unwrap();

    let mut natives: HashMap<String, u32> = HashMap::new();
    for m in linker.modules() {
        for (name, addr) in m.exports.iter() {
            if name.starts_with("Java_") {
                natives.insert(name.clone(), *addr);
            }
        }
    }
    eprintln!("{} Java_* exports", natives.len());
    vm.set_symbol_resolver(move |n| natives.get(n).copied());

    for m in linker.modules() {
        if let Some(addr) = m.export("JNI_OnLoad") {
            let r = vm.call_onload(&mut cpu, addr);
            eprintln!("JNI_OnLoad {} -> {:?}", m.name, r.map(|v| format!("{v:#x}")));
        }
    }
    let platform = Platform::default();
    let init = Stage::LibraryInit;
    eprintln!("{init:?}: bound={} -> {:?}", vm.stage_bound(init), vm.run_stage(&mut cpu, init));
    eprintln!("setenv: {:?}", vm.set_native_env(&mut cpu, &native_env(&platform)));
    for stage in
        [Stage::ActivityCreate, Stage::ActivityStart, Stage::ActivityResume, Stage::RootGeometry, Stage::ContextValid]
    {
        eprintln!("{stage:?}: bound={} -> {:?}", vm.stage_bound(stage), vm.run_stage(&mut cpu, stage));
    }
    eprintln!("{}", vm.report().render());
}

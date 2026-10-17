//! Shared test scaffolding: a CPU with memory, stack, heap and a deterministic clock.

use crate::state::{Config, LogRecord};
use crate::{resolve, HostClock, Shared, State};
use bhpc_cpu::{CallResult, Cpu, CpuOptions, Stop};
use bhpc_mem::{GuestHeap, GuestMemory};
use std::io::Write;
use std::sync::{Arc, Mutex};

pub const HEAP_BASE: u32 = 0x4000_0000;
pub const SCRATCH: u32 = 0x5000_0000;
pub const STACK_TOP: u32 = 0x7010_0000;

#[derive(Clone, Default)]
pub struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().unwrap().extend_from_slice(b"<flush>");
        Ok(())
    }
}

impl Buf {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

struct FakeTime {
    real: (i64, u32),
    cpu: (i64, u32),
    slept: Vec<u64>,
}

/// Handle on the fake clock installed in the environment.
#[derive(Clone)]
pub struct ClockHandle(Arc<Mutex<FakeTime>>);

impl ClockHandle {
    fn new() -> ClockHandle {
        ClockHandle(Arc::new(Mutex::new(FakeTime { real: (1_600_000_000, 0), cpu: (7, 0), slept: Vec::new() })))
    }
    pub fn set(&self, sec: i64, nsec: u32) {
        self.0.lock().unwrap().real = (sec, nsec);
    }
    pub fn set_cpu(&self, sec: i64, nsec: u32) {
        self.0.lock().unwrap().cpu = (sec, nsec);
    }
    pub fn slept(&self) -> Vec<u64> {
        self.0.lock().unwrap().slept.clone()
    }
}

impl HostClock for ClockHandle {
    fn realtime(&mut self) -> (i64, u32) {
        self.0.lock().unwrap().real
    }
    fn monotonic(&mut self) -> (i64, u32) {
        (5000, 0)
    }
    fn cpu_time(&mut self) -> (i64, u32) {
        self.0.lock().unwrap().cpu
    }
    fn sleep(&mut self, ns: u64) {
        self.0.lock().unwrap().slept.push(ns);
    }
}

pub struct Env {
    pub cpu: Box<Cpu>,
    pub st: Shared,
    pub out: Buf,
    pub err: Buf,
    pub clock: ClockHandle,
}

impl Env {
    pub fn new() -> Env {
        Env::build(16 << 20, |_| {})
    }

    /// A 1 MiB heap, for exhaustion tests.
    pub fn small() -> Env {
        Env::build(1 << 20, |_| {})
    }

    pub fn with(f: impl FnOnce(&mut Config)) -> Env {
        Env::build(16 << 20, f)
    }

    fn build(heap: u32, f: impl FnOnce(&mut Config)) -> Env {
        let mem = Arc::new(GuestMemory::new().unwrap());
        let heap = GuestHeap::new(mem.clone(), HEAP_BASE, heap).unwrap();
        mem.map(STACK_TOP - (1 << 20), 1 << 20).unwrap();
        mem.map(SCRATCH, 0x10000).unwrap();
        let (out, err, clock) = (Buf::default(), Buf::default(), ClockHandle::new());
        let mut cfg = Config {
            stdout: Box::new(out.clone()),
            stderr: Box::new(err.clone()),
            clock: Box::new(clock.clone()),
            stack_guard: Some(0xC0FF_EE00),
            pid: 4242,
            ..Config::default()
        };
        f(&mut cfg);
        let st = State::new(mem.clone(), heap, cfg).unwrap();
        let mut cpu = Cpu::with_options(mem, CpuOptions::default()).unwrap();
        cpu.set_sp(STACK_TOP);
        Env { cpu, st, out, err, clock }
    }

    pub fn mem(&self) -> Arc<GuestMemory> {
        self.cpu.mem().clone()
    }

    pub fn addr(&mut self, name: &str) -> u32 {
        resolve(&mut self.cpu, &self.st, name).unwrap()
    }

    pub fn data(&self, name: &str) -> u32 {
        crate::lock(&self.st).data[name]
    }

    pub fn try_call(&mut self, name: &str, args: &[u32]) -> Result<CallResult, Stop> {
        let a = self.addr(name);
        self.cpu.call(a, args)
    }

    pub fn call(&mut self, name: &str, args: &[u32]) -> CallResult {
        self.try_call(name, args).unwrap_or_else(|s| panic!("{name}{args:x?} stopped: {s}"))
    }

    pub fn errno(&self) -> u32 {
        let a = crate::lock(&self.st).errno;
        self.mem().read_u32(a).unwrap()
    }

    pub fn set_errno(&self, v: u32) {
        let a = crate::lock(&self.st).errno;
        self.mem().write_u32(a, v).unwrap();
    }

    pub fn put_str(&self, at: u32, s: &str) {
        self.mem().write_cstr(at, s).unwrap();
    }

    pub fn get_str(&self, at: u32) -> String {
        self.mem().read_cstr(at).unwrap()
    }
}

/// Log sink that collects records.
pub fn collecting_sink() -> (Arc<Mutex<Vec<LogRecord>>>, crate::state::LogSink) {
    let v = Arc::new(Mutex::new(Vec::new()));
    let v2 = v.clone();
    (v, Box::new(move |r: &LogRecord| v2.lock().unwrap().push(r.clone())))
}

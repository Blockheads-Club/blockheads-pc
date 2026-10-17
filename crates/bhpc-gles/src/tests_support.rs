//! Test scaffolding: a recording GL backend, a fake window and a CPU + guest memory environment.

use crate::api::{Arg, FromRet};
use crate::state::{Gles, GlesConfig};
use crate::window::{GlWindow, HostContext};
use bhpc_cpu::{CallResult, Cpu, CpuOptions, Stop};
use bhpc_mem::GuestMemory;
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub type Call = (String, Vec<Arg>);
type Hook = Box<dyn FnMut(&str, &[Arg]) + Send>;

/// A [`crate::GlApi`] that records every call. Clones share the log.
#[derive(Clone, Default)]
pub struct Recorder {
    pub log: Arc<Mutex<Vec<Call>>>,
    pub rets: Arc<Mutex<HashMap<String, i64>>>,
    pub missing: Arc<Mutex<HashSet<String>>>,
    pub hook: Arc<Mutex<Option<Hook>>>,
}

impl Recorder {
    pub fn record<R: FromRet>(&mut self, name: &str, args: Vec<Arg>) -> R {
        if let Some(h) = self.hook.lock().unwrap().as_mut() {
            h(name, &args);
        }
        self.log.lock().unwrap().push((name.to_string(), args));
        R::from_ret(self.rets.lock().unwrap().get(name).copied().unwrap_or(0))
    }
    pub fn is_unsupported(&self, name: &str) -> bool {
        self.missing.lock().unwrap().contains(name)
    }
    pub fn set_ret(&self, name: &str, v: i64) {
        self.rets.lock().unwrap().insert(name.to_string(), v);
    }
    pub fn missing(&self, name: &str) {
        self.missing.lock().unwrap().insert(name.to_string());
    }
    pub fn on_call(&self, f: impl FnMut(&str, &[Arg]) + Send + 'static) {
        *self.hook.lock().unwrap() = Some(Box::new(f));
    }
    pub fn take(&self) -> Vec<Call> {
        std::mem::take(&mut *self.log.lock().unwrap())
    }
    pub fn names(&self) -> Vec<String> {
        self.log.lock().unwrap().iter().map(|c| c.0.clone()).collect()
    }
}

/// A [`GlWindow`] with switches and counters.
pub struct FakeWindow {
    pub next: AtomicU64,
    pub destroyed: Mutex<Vec<HostContext>>,
    pub made_current: Mutex<Vec<Option<HostContext>>>,
    pub swaps: AtomicU32,
    pub interval: AtomicU32,
    pub fail_create: AtomicBool,
    pub fail_make_current: AtomicBool,
    pub fail_interval: AtomicBool,
    pub shared_with: Mutex<Vec<Option<HostContext>>>,
    pub size: Mutex<(u32, u32)>,
}

impl FakeWindow {
    pub fn new() -> Arc<FakeWindow> {
        Arc::new(FakeWindow {
            next: AtomicU64::new(100),
            destroyed: Mutex::new(Vec::new()),
            made_current: Mutex::new(Vec::new()),
            swaps: AtomicU32::new(0),
            interval: AtomicU32::new(99),
            fail_create: AtomicBool::new(false),
            fail_make_current: AtomicBool::new(false),
            fail_interval: AtomicBool::new(false),
            shared_with: Mutex::new(Vec::new()),
            size: Mutex::new((800, 480)),
        })
    }
}

impl GlWindow for FakeWindow {
    fn create_context(&self, share: Option<HostContext>) -> Option<HostContext> {
        self.shared_with.lock().unwrap().push(share);
        if self.fail_create.load(Ordering::SeqCst) {
            return None;
        }
        Some(self.next.fetch_add(1, Ordering::SeqCst))
    }
    fn destroy_context(&self, ctx: HostContext) {
        self.destroyed.lock().unwrap().push(ctx);
    }
    fn make_current(&self, ctx: Option<HostContext>) -> bool {
        self.made_current.lock().unwrap().push(ctx);
        !self.fail_make_current.load(Ordering::SeqCst)
    }
    fn swap_buffers(&self) {
        self.swaps.fetch_add(1, Ordering::SeqCst);
    }
    fn set_swap_interval(&self, interval: i32) -> bool {
        if self.fail_interval.load(Ordering::SeqCst) {
            return false;
        }
        self.interval.store(interval as u32, Ordering::SeqCst);
        true
    }
    fn drawable_size(&self) -> (u32, u32) {
        *self.size.lock().unwrap()
    }
    fn proc_address(&self, name: &str) -> *const c_void {
        crate::api::dummy_driver::lookup(name)
    }
}

pub const ARENA: u32 = 0x6000_0000;
pub const SCRATCH: u32 = 0x5000_0000;
pub const STACK_TOP: u32 = 0x7010_0000;

pub struct Env {
    pub cpu: Box<Cpu>,
    pub gl: Gles,
    pub rec: Recorder,
    pub win: Arc<FakeWindow>,
    pub log: Arc<Mutex<Vec<u8>>>,
}

struct LogBuf(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for LogBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Env {
    pub fn new() -> Env {
        Env::with(|_| {})
    }

    pub fn with(f: impl FnOnce(&mut GlesConfig)) -> Env {
        let mem = Arc::new(GuestMemory::new().unwrap());
        mem.map(STACK_TOP - (1 << 20), 1 << 20).unwrap();
        mem.map(SCRATCH, 0x10_0000).unwrap();
        let (rec, win) = (Recorder::default(), FakeWindow::new());
        let mut cfg = GlesConfig::new(ARENA, 4 << 20);
        f(&mut cfg);
        let gl = Gles::new(cfg, Box::new(rec.clone()), win.clone(), mem.clone()).unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        gl.set_log(Box::new(LogBuf(log.clone())));
        let mut cpu = Cpu::with_options(mem, CpuOptions::default()).unwrap();
        cpu.set_sp(STACK_TOP);
        Env { cpu, gl, rec, win, log }
    }

    pub fn mem(&self) -> Arc<GuestMemory> {
        self.cpu.mem().clone()
    }

    pub fn addr(&mut self, name: &str) -> u32 {
        crate::resolve(&mut self.cpu, &self.gl, name).unwrap_or_else(|| panic!("{name} not provided"))
    }

    pub fn try_call(&mut self, name: &str, args: &[u32]) -> Result<CallResult, Stop> {
        let a = self.addr(name);
        self.cpu.call(a, args)
    }

    pub fn call(&mut self, name: &str, args: &[u32]) -> u32 {
        self.try_call(name, args).unwrap_or_else(|s| panic!("{name}{args:x?} stopped: {s}")).r0
    }

    /// Host address of guest address `a`.
    pub fn host(&self, a: u32) -> usize {
        self.mem().base() as usize + a as usize
    }

    pub fn put(&self, at: u32, bytes: &[u8]) {
        self.mem().write_bytes(at, bytes).unwrap();
    }

    pub fn put_words(&self, at: u32, w: &[u32]) {
        for (i, v) in w.iter().enumerate() {
            self.mem().write_u32(at + 4 * i as u32, *v).unwrap();
        }
    }

    pub fn word(&self, at: u32) -> u32 {
        self.mem().read_u32(at).unwrap()
    }

    pub fn log_text(&self) -> String {
        String::from_utf8_lossy(&self.log.lock().unwrap()).into_owned()
    }
}

/// `Arg::I`.
pub fn i(v: i64) -> Arg {
    Arg::I(v)
}
/// `Arg::F`.
pub fn f(v: f64) -> Arg {
    Arg::F(v)
}

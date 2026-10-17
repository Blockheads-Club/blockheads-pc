//! Shared test scaffolding: a CPU with memory, stack, heap and a VM with the built-in library.

use crate::{new_vm, GuestAlloc, Layout, MemAssets, Obj, Platform, Value, Vm, VmConfig};
use bhpc_cpu::{CallResult, Cpu, CpuOptions, Stop};
use bhpc_mem::{GuestHeap, GuestMemory};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const HEAP_BASE: u32 = 0x4000_0000;
pub const SCRATCH: u32 = 0x5000_0000;
pub const CODE: u32 = 0x5100_0000;
pub const STACK_TOP: u32 = 0x7010_0000;

struct Flaky {
    heap: GuestHeap,
    fail: Arc<AtomicBool>,
}

impl GuestAlloc for Flaky {
    fn alloc(&mut self, size: u32) -> u32 {
        if self.fail.load(Ordering::Relaxed) {
            0
        } else {
            self.heap.alloc(size)
        }
    }
    fn free(&mut self, ptr: u32) {
        self.heap.free(ptr)
    }
}

pub struct Env {
    pub vm: Vm,
    pub cpu: Box<Cpu>,
    pub layout: Layout,
    pub clock: Arc<AtomicU64>,
    pub logs: Arc<Mutex<Vec<String>>>,
    scratch: u32,
    code: u32,
}

pub fn test_platform() -> Platform {
    Platform {
        screen_width: 800,
        screen_height: 480,
        density_dpi: 240,
        locale: "tr_TR".into(),
        timezone: "Europe/Istanbul".into(),
        files_dir: std::env::temp_dir().join(format!("bhpc-jni-{}", std::process::id())).to_string_lossy().into_owned(),
        apk_path: "/tmp/test.apk".into(),
        ..Platform::default()
    }
}

impl Env {
    pub fn new() -> Env {
        Env::with(|_| {})
    }

    pub fn with(f: impl FnOnce(&mut VmConfig)) -> Env {
        Env::build(f, None)
    }

    /// An environment whose guest allocator can be switched to fail (`true` = out of memory).
    pub fn flaky() -> (Env, Arc<AtomicBool>) {
        let fail = Arc::new(AtomicBool::new(false));
        (Env::build(|_| {}, Some(fail.clone())), fail)
    }

    fn build(f: impl FnOnce(&mut VmConfig), flaky: Option<Arc<AtomicBool>>) -> Env {
        let mem = Arc::new(GuestMemory::new().unwrap());
        let heap = GuestHeap::new(mem.clone(), HEAP_BASE, 16 << 20).unwrap();
        mem.map(STACK_TOP - (1 << 20), 1 << 20).unwrap();
        mem.map(SCRATCH, 0x10000).unwrap();
        mem.map(CODE, 0x10000).unwrap();
        let assets = MemAssets(vec![
            ("hello.txt".into(), b"hello assets".to_vec()),
            ("dir/a.bin".into(), vec![1, 2, 3, 4, 5]),
            ("dir/b.bin".into(), vec![9]),
        ]);
        let clock = Arc::new(AtomicU64::new(1000));
        let c2 = clock.clone();
        let mut cfg = VmConfig::new(test_platform(), Box::new(assets));
        cfg.clock = Arc::new(move || c2.load(Ordering::Relaxed));
        f(&mut cfg);
        let alloc: Box<dyn GuestAlloc> = match flaky {
            Some(fail) => Box::new(Flaky { heap, fail }),
            None => Box::new(heap),
        };
        let vm = new_vm(mem.clone(), alloc, cfg);
        let logs = Arc::new(Mutex::new(Vec::new()));
        let l2 = logs.clone();
        vm.set_log(move |m| l2.lock().unwrap().push(m.to_string()));
        let mut cpu = Cpu::with_options(mem, CpuOptions::default()).unwrap();
        cpu.set_sp(STACK_TOP);
        let layout = vm.install(&cpu).unwrap();
        Env { vm, cpu, layout, clock, logs, scratch: SCRATCH, code: CODE }
    }

    pub fn mem(&self) -> Arc<GuestMemory> {
        self.cpu.mem().clone()
    }

    pub fn env_ptr(&self) -> u32 {
        self.vm.env_ptr(self.cpu.processor_id())
    }

    /// Write a NUL-terminated string into scratch memory.
    pub fn cstr(&mut self, s: &str) -> u32 {
        self.bytes(s.as_bytes(), 1)
    }

    pub fn bytes(&mut self, b: &[u8], extra: u32) -> u32 {
        let p = self.scratch;
        self.mem().write_bytes(p, b).unwrap();
        self.mem().write_u8(p + b.len() as u32, 0).unwrap();
        self.scratch += (b.len() as u32 + extra + 15) & !15;
        p
    }

    /// Reserve `n` zeroed bytes of scratch memory.
    pub fn buf(&mut self, n: u32) -> u32 {
        let p = self.scratch;
        self.mem().write_bytes(p, &vec![0; n as usize]).unwrap();
        self.scratch += (n + 15) & !15;
        p
    }

    pub fn local(&self, o: Obj) -> u32 {
        self.vm.new_ref(self.cpu.processor_id(), Some(o), crate::vm::RefKind::Local)
    }

    pub fn class_h(&self, name: &str) -> u32 {
        let c = self.vm.find_class(name).unwrap();
        self.local(self.vm.class_object(c))
    }

    pub fn thunk(&self, name: &str) -> u32 {
        self.cpu.thunk_addr(&format!("JNI:{name}")).unwrap_or_else(|| panic!("no thunk {name}"))
    }

    /// Call the JNI function `name` with `args` after the env pointer.
    pub fn jni(&mut self, name: &str, args: &[u32]) -> CallResult {
        self.try_jni(name, args).unwrap_or_else(|s| panic!("{name} stopped: {s}"))
    }

    pub fn try_jni(&mut self, name: &str, args: &[u32]) -> Result<CallResult, Stop> {
        let a = self.thunk(name);
        let mut all = vec![self.env_ptr()];
        all.extend_from_slice(args);
        self.cpu.call(a, &all)
    }

    pub fn pending_class(&self) -> Option<String> {
        let e = self.vm.pending(self.cpu.processor_id())?;
        Some(self.vm.class_name(self.vm.class_of(e)?))
    }

    pub fn clear_pending(&self) {
        self.vm.take_pending(self.cpu.processor_id());
    }

    pub fn log_text(&self) -> String {
        self.logs.lock().unwrap().join("\n")
    }

    /// Place ARM code in guest memory and return its address.
    pub fn code(&mut self, words: &[u32]) -> u32 {
        let p = self.code;
        for (i, w) in words.iter().enumerate() {
            self.mem().write_u32(p + 4 * i as u32, *w).unwrap();
        }
        self.code += 4 * words.len() as u32 + 16;
        p
    }

    pub fn s(&self, o: Obj) -> String {
        self.vm.string(o).unwrap()
    }

    pub fn obj(&self, v: crate::JResult) -> Obj {
        v.unwrap().as_ref().expect("non-null result")
    }

    pub fn call(&mut self, o: Obj, name: &str, sig: &str, args: &[Value]) -> crate::JResult {
        let vm = self.vm.clone();
        vm.call_method(&mut self.cpu, o, name, sig, args)
    }

    pub fn scall(&mut self, class: &str, name: &str, sig: &str, args: &[Value]) -> crate::JResult {
        let vm = self.vm.clone();
        vm.call_static(&mut self.cpu, class, name, sig, args)
    }

    pub fn activity(&mut self) -> Obj {
        let r = self.scall(
            "com/apportable/activity/VerdeActivity",
            "getActivity",
            "()Lcom/apportable/activity/VerdeActivity;",
            &[],
        );
        self.obj(r)
    }

    pub fn new_obj(&mut self, class: &str, sig: &str, args: &[Value]) -> Obj {
        let vm = self.vm.clone();
        vm.new_object(&mut self.cpu, class, sig, args).unwrap()
    }

    pub fn string(&self, s: &str) -> Value {
        Value::Ref(Some(self.vm.new_string(s)))
    }
}

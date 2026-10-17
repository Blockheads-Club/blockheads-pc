//! ARM32 guest CPU: dynarmic A32 JIT plus the SVC thunk ABI.
//!
//! Imported symbols resolve to *thunks*: 16-byte stubs `mov r12, lr; svc #n; bx r12` in a reserved
//! guest page. Because the stub returns through `r12`, a host function can redirect control
//! (tail-call a guest function, `longjmp`) with [`Cpu::tail_call`] without a nested JIT run.
//! When the guest executes one, the JIT calls back into [`Cpu`], which dispatches to the host
//! closure registered for `n`. Arguments follow the AAPCS **softfp** convention (r0-r3, then the
//! stack; 64-bit values in aligned register pairs), results go to r0 (and r1 for 64-bit values).
//!
//! # Threads and nested calls
//!
//! Everything a guest thread shares (memory, the [`ThunkTable`], the exclusive monitor) lives in
//! one `Arc`'d `Shared`. [`Cpu::new_thread`] makes a sibling CPU with its own JIT and
//! exclusive-monitor processor id. A host function that needs the result of a guest function
//! (`qsort` comparators, `atexit` handlers, `pthread_once` routines) calls [`Cpu::call`] from
//! inside the thunk: dynarmic forbids a second `Run()` on the same JIT, so the call runs on a
//! lazily created *child* CPU that shares memory, thunks and the monitor and continues on the
//! parent's stack. Use [`Cpu::tail_call`] instead when the guest function's result is the thunk's
//! result (no extra JIT, no depth limit).

#[cfg(test)]
mod buildcfg;
mod ffi;
mod table;

pub use table::{SharedThunkFn, ThunkFn, ThunkTable};

use bhpc_mem::{GuestMemory, MemError};
use std::ffi::c_void;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

/// Guest address of the thunk page.
pub const THUNK_BASE: u32 = 0xFFFC_0000;
/// Size of the thunk page.
pub const THUNK_REGION: u32 = 0x4_0000;
/// Bytes per thunk stub.
pub const THUNK_STRIDE: u32 = 16;
/// Maximum number of host thunks (slot 0 is the return-to-host stub).
pub const MAX_THUNKS: u32 = THUNK_REGION / THUNK_STRIDE - 1;
/// `lr` value used by [`Cpu::call`]; executing it ends the call.
pub const RETURN_ADDR: u32 = THUNK_BASE;
/// Maximum number of simultaneous guest threads (exclusive-monitor processors).
pub const MAX_CPUS: usize = 64;
/// Maximum depth of host-to-guest calls nested inside thunks.
pub const MAX_NEST_DEPTH: u32 = 16;
/// Code cache size of nested-call children (at most this, whatever the parent uses).
const CHILD_CODE_CACHE: usize = 16 << 20;

const R_IP: usize = 12;
const R_SP: usize = 13;
const R_LR: usize = 14;
const R_PC: usize = 15;
const CPSR_T: u32 = 1 << 5;
const CPSR_USER: u32 = 0x10;
/// dynarmic `HaltReason::CacheInvalidation`.
const HALT_CACHE_INVALIDATION: u32 = 2;
/// dynarmic `A32::Exception` values we give a dedicated stop reason.
const EXC_UNDEFINED: u32 = 0;
const EXC_UNPREDICTABLE: u32 = 1;

/// A guest memory access that hit an unmapped address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fault {
    pub addr: u32,
    pub size: u32,
    pub write: bool,
}

/// Why guest execution stopped without returning normally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// The guest reached [`RETURN_ADDR`].
    Returned,
    /// Unmapped memory access.
    Fault(Fault),
    /// The guest executed an undefined (unallocated) instruction. `word` is the instruction at
    /// `pc` (for Thumb the 16-bit or combined 32-bit encoding).
    Undefined { pc: u32, word: u32, thumb: bool },
    /// The guest executed an instruction with unpredictable behaviour (see [`Stop::Undefined`]).
    Unpredictable { pc: u32, word: u32, thumb: bool },
    /// Any other dynarmic exception, such as a breakpoint (`kind` is dynarmic's exception number).
    Exception { pc: u32, kind: u32 },
    /// The JIT asked for interpretation of an instruction it cannot translate.
    Fallback { pc: u32 },
    /// `svc #n` with no host function registered for `n`.
    BadSvc(u32),
    /// A host function requested the stop (for example an unimplemented import).
    Host(String),
    /// A nested guest call from a thunk is impossible: too deep ([`MAX_NEST_DEPTH`]), a traced
    /// call, or a serialised thunk re-entered itself (register it as a shared thunk instead).
    Nested,
    /// The JIT stopped for a reason other than the above (raw halt bits).
    Halted(u32),
}

impl fmt::Display for Stop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Stop::Returned => write!(f, "returned"),
            Stop::Fault(x) => {
                write!(
                    f,
                    "guest {} of {} byte(s) at unmapped {:#010x}",
                    if x.write { "write" } else { "read" },
                    x.size,
                    x.addr
                )
            }
            Stop::Undefined { pc, word, thumb } => write!(
                f,
                "undefined {} instruction {word:#010x} at pc {pc:#010x}",
                if *thumb { "thumb" } else { "arm" }
            ),
            Stop::Unpredictable { pc, word, thumb } => write!(
                f,
                "unpredictable {} instruction {word:#010x} at pc {pc:#010x}",
                if *thumb { "thumb" } else { "arm" }
            ),
            Stop::Exception { pc, kind } => write!(f, "guest exception {kind} at pc {pc:#010x}"),
            Stop::Fallback { pc } => write!(f, "interpreter fallback requested at pc {pc:#010x}"),
            Stop::BadSvc(n) => write!(f, "svc #{n} has no host function"),
            Stop::Host(m) => write!(f, "host stop: {m}"),
            Stop::Nested => write!(f, "nested guest call from a thunk is not possible here"),
            Stop::Halted(h) => write!(f, "jit halted ({h:#x})"),
        }
    }
}

impl std::error::Error for Stop {}

/// Errors creating a [`Cpu`] or registering thunks.
#[derive(Debug, PartialEq, Eq)]
pub enum CpuError {
    Mem(MemError),
    TooManyThunks,
    /// All [`MAX_CPUS`] exclusive-monitor processors are in use.
    TooManyCpus,
}

impl fmt::Display for CpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CpuError::Mem(e) => write!(f, "guest memory: {e}"),
            CpuError::TooManyThunks => write!(f, "thunk page is full"),
            CpuError::TooManyCpus => write!(f, "too many guest threads"),
        }
    }
}

impl std::error::Error for CpuError {}

impl From<MemError> for CpuError {
    fn from(e: MemError) -> Self {
        CpuError::Mem(e)
    }
}

/// Options for [`Cpu::with_options`].
#[derive(Debug, Clone, Copy)]
pub struct CpuOptions {
    /// Use the host-pointer fast path for guest memory (the default). When off, every access goes
    /// through the (slower) callbacks; used by tests to cover them.
    pub fastmem: bool,
    /// Size of the JIT code cache.
    pub code_cache_bytes: usize,
    /// Count guest cycles so [`Cpu::set_ticks`] can act as a watchdog (slightly slower).
    pub cycle_counting: bool,
}

/// Whether fastmem is on by default. On macOS the vendored dynarmic is built with its generic (no-op)
/// exception handler (the Mach handler needs `mig` and writes into the source tree), so a fastmem
/// fault would crash instead of reaching the callbacks; callbacks it is.
const FASTMEM_DEFAULT: bool = !cfg!(target_os = "macos");

impl Default for CpuOptions {
    fn default() -> Self {
        CpuOptions { fastmem: FASTMEM_DEFAULT, code_cache_bytes: 64 << 20, cycle_counting: false }
    }
}

/// Result of a guest call: r0 and r1 (the 32/64-bit AAPCS return registers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallResult {
    pub r0: u32,
    pub r1: u32,
}

impl CallResult {
    /// 64-bit integer return value (`r1:r0`).
    pub fn as_u64(&self) -> u64 {
        (self.r1 as u64) << 32 | self.r0 as u64
    }
    /// Double return value under softfp (`r1:r0`).
    pub fn as_f64(&self) -> f64 {
        f64::from_bits(self.as_u64())
    }
}

/// Owner of the global exclusive monitor.
struct Monitor(*mut ffi::bhpc_monitor);

// SAFETY: dynarmic's ExclusiveMonitor is internally locked and meant to be shared by all cores.
unsafe impl Send for Monitor {}
unsafe impl Sync for Monitor {}

impl Drop for Monitor {
    fn drop(&mut self) {
        // SAFETY: created by `bhpc_monitor_new`, freed once, after every JIT using it is gone
        // (each `Cpu` holds an `Arc<Shared>`).
        unsafe { ffi::bhpc_monitor_free(self.0) };
    }
}

/// State shared by all CPUs of one guest: memory, thunks, the exclusive monitor.
struct Shared {
    mem: Arc<GuestMemory>,
    table: Arc<ThunkTable>,
    monitor: Monitor,
    /// Bit `i` set = processor id `i` is taken.
    pids: Mutex<u64>,
}

impl Shared {
    fn alloc_pid(&self) -> Result<usize, CpuError> {
        let mut g = self.pids.lock().unwrap_or_else(PoisonError::into_inner);
        let pid = (!*g).trailing_zeros() as usize;
        if pid >= MAX_CPUS {
            return Err(CpuError::TooManyCpus);
        }
        *g |= 1 << pid;
        Ok(pid)
    }

    fn free_pid(&self, pid: usize) {
        *self.pids.lock().unwrap_or_else(PoisonError::into_inner) &= !(1u64 << pid);
    }
}

/// One guest hardware thread: a JIT instance plus handles to the state shared with its siblings.
pub struct Cpu {
    jit: *mut ffi::bhpc_jit,
    mem: Arc<GuestMemory>,
    shared: Arc<Shared>,
    opts: CpuOptions,
    pid: usize,
    /// Only the CPU that allocated `pid` releases it (nested-call children borrow the parent's).
    owns_pid: bool,
    /// 0 for a normal CPU, parent's depth + 1 for a nested-call child.
    depth: u32,
    parent: *mut Cpu,
    child: Option<Box<Cpu>>,
    stop: Option<Stop>,
    running: bool,
    trace: bool,
    /// r0..r15 at the moment the last top-level call failed (innermost nested call wins).
    stop_regs: Option<[u32; 16]>,
}

// SAFETY: a Cpu is only ever driven from one thread at a time (it owns its JIT instance); the
// raw `parent` pointer is only followed on that same thread.
unsafe impl Send for Cpu {}

impl Cpu {
    /// Create a CPU with default options.
    pub fn new(mem: Arc<GuestMemory>) -> Result<Box<Cpu>, CpuError> {
        Self::with_options(mem, CpuOptions::default())
    }

    /// Create the first CPU of a guest: maps the thunk page and writes the return stub. Use
    /// [`Cpu::new_thread`] for further CPUs on the same memory.
    pub fn with_options(mem: Arc<GuestMemory>, opts: CpuOptions) -> Result<Box<Cpu>, CpuError> {
        let table = ThunkTable::new(mem.clone())?;
        // SAFETY: plain allocation; freed by `Monitor::drop`.
        let monitor = Monitor(unsafe { ffi::bhpc_monitor_new(MAX_CPUS) });
        let shared = Arc::new(Shared { mem, table, monitor, pids: Mutex::new(0) });
        let pid = shared.alloc_pid()?;
        Ok(Self::build(shared, opts, pid, true, 0))
    }

    /// Create another guest thread: a CPU with its own JIT, registers and exclusive-monitor
    /// processor id that shares this CPU's memory, thunk table and monitor. The caller sets its
    /// stack pointer and entry point. May be moved to and driven from another OS thread.
    pub fn new_thread(&self) -> Result<Box<Cpu>, CpuError> {
        let pid = self.shared.alloc_pid()?;
        let mut cpu = Self::build(self.shared.clone(), self.opts, pid, true, 0);
        cpu.trace = self.trace;
        Ok(cpu)
    }

    fn build(shared: Arc<Shared>, opts: CpuOptions, pid: usize, owns_pid: bool, depth: u32) -> Box<Cpu> {
        let mem = shared.mem.clone();
        let mut cpu = Box::new(Cpu {
            jit: std::ptr::null_mut(),
            mem: mem.clone(),
            shared,
            opts,
            pid,
            owns_pid,
            depth,
            parent: std::ptr::null_mut(),
            child: None,
            stop: None,
            running: false,
            trace: false,
            stop_regs: None,
        });
        let cb = ffi::bhpc_callbacks {
            user: (&mut *cpu as *mut Cpu).cast::<c_void>(),
            read8: tramp::read8,
            read16: tramp::read16,
            read32: tramp::read32,
            read64: tramp::read64,
            write8: tramp::write8,
            write16: tramp::write16,
            write32: tramp::write32,
            write64: tramp::write64,
            write_excl8: tramp::write_excl8,
            write_excl16: tramp::write_excl16,
            write_excl32: tramp::write_excl32,
            write_excl64: tramp::write_excl64,
            svc: tramp::svc,
            exception: tramp::exception,
            interpreter_fallback: tramp::fallback,
        };
        let fast = if opts.fastmem { mem.base() } else { std::ptr::null_mut() };
        // SAFETY: callbacks outlive the JIT (both owned by the boxed Cpu); `fast` is our reservation;
        // the monitor outlives the JIT through `cpu.shared`.
        cpu.jit = unsafe {
            ffi::bhpc_jit_new(
                &cb,
                fast,
                opts.code_cache_bytes,
                i32::from(opts.cycle_counting),
                cpu.shared.monitor.0,
                pid,
            )
        };
        // SAFETY: jit was just created.
        unsafe { ffi::bhpc_jit_set_cpsr(cpu.jit, CPSR_USER) };
        cpu
    }

    /// The thunk table shared by this CPU and all its siblings.
    pub fn table(&self) -> &Arc<ThunkTable> {
        &self.shared.table
    }

    /// The instruction encoding at `pc` in the current instruction set (0 if unreadable), and
    /// whether it is Thumb. A 32-bit Thumb instruction is returned as `hw1 << 16 | hw2`.
    fn instruction_at(&self, pc: u32) -> (u32, bool) {
        if self.cpsr() & CPSR_T == 0 {
            return (self.mem.read_u32(pc).unwrap_or(0), false);
        }
        let hw1 = u32::from(self.mem.read_u16(pc).unwrap_or(0));
        if matches!(hw1 >> 11, 0b11101..=0b11111) {
            let hw2 = u32::from(self.mem.read_u16(pc.wrapping_add(2)).unwrap_or(0));
            (hw1 << 16 | hw2, true)
        } else {
            (hw1, true)
        }
    }

    /// Set the remaining cycle budget; `run` returns [`Stop::Halted`] when it is used up.
    /// Requires `CpuOptions::cycle_counting`.
    pub fn set_ticks(&mut self, ticks: u64) {
        // SAFETY: valid jit.
        unsafe { ffi::bhpc_jit_set_ticks(self.jit, ticks) };
    }

    /// Remaining cycle budget.
    pub fn ticks(&self) -> u64 {
        // SAFETY: valid jit.
        unsafe { ffi::bhpc_jit_ticks(self.jit) }
    }

    /// Log every thunk call (name, r0-r3, lr) to stderr.
    pub fn set_trace_thunks(&mut self, on: bool) {
        self.trace = on;
    }

    /// The guest memory this CPU runs in.
    pub fn mem(&self) -> &Arc<GuestMemory> {
        &self.mem
    }

    /// Exclusive-monitor processor id of this CPU (nested-call children share their parent's).
    pub fn processor_id(&self) -> usize {
        self.pid
    }

    // ---------- thunks ----------

    /// Register (or rebind) the serialised thunk for `name`; returns its guest address (ARM,
    /// bit 0 clear). The table is shared with sibling CPUs, so this takes `&self`.
    pub fn register_thunk(&self, name: &str, f: ThunkFn) -> Result<u32, CpuError> {
        self.shared.table.register_thunk(name, f)
    }

    /// Register (or rebind) a re-entrant thunk (see [`SharedThunkFn`]).
    pub fn register_shared_thunk(&self, name: &str, f: SharedThunkFn) -> Result<u32, CpuError> {
        self.shared.table.register_shared_thunk(name, f)
    }

    /// Guest address of an already registered thunk.
    pub fn thunk_addr(&self, name: &str) -> Option<u32> {
        self.shared.table.thunk_addr(name)
    }

    /// Name of the thunk at guest address `addr`, if any.
    pub fn thunk_name(&self, addr: u32) -> Option<String> {
        self.shared.table.thunk_name(addr)
    }

    /// Number of registered thunks (excluding the return stub).
    pub fn thunk_count(&self) -> usize {
        self.shared.table.thunk_count()
    }

    // ---------- registers ----------

    fn regs_ptr(&self) -> *mut u32 {
        // SAFETY: dynarmic returns a stable pointer to its 16-entry register file.
        unsafe { ffi::bhpc_jit_regs(self.jit) }
    }

    /// Read core register `i` (0..=15).
    pub fn reg(&self, i: usize) -> u32 {
        assert!(i < 16, "core register index out of range");
        // SAFETY: `i` is within the 16-entry register file.
        unsafe { *self.regs_ptr().add(i) }
    }
    /// Write core register `i` (0..=15).
    pub fn set_reg(&mut self, i: usize, v: u32) {
        assert!(i < 16, "core register index out of range");
        // SAFETY: `i` is within the 16-entry register file.
        unsafe { *self.regs_ptr().add(i) = v };
    }
    /// Stack pointer.
    pub fn sp(&self) -> u32 {
        self.reg(R_SP)
    }
    /// Set the stack pointer.
    pub fn set_sp(&mut self, v: u32) {
        self.set_reg(R_SP, v);
    }
    /// Link register.
    pub fn lr(&self) -> u32 {
        self.reg(R_LR)
    }
    /// Program counter.
    pub fn pc(&self) -> u32 {
        self.reg(R_PC)
    }
    /// Raw bits of double register `d{i}` (0..=31).
    pub fn d_bits(&self, i: usize) -> u64 {
        // SAFETY: 64 contiguous u32 slots.
        let e = unsafe { std::slice::from_raw_parts(ffi::bhpc_jit_ext_regs(self.jit), 64) };
        (e[2 * i + 1] as u64) << 32 | e[2 * i] as u64
    }
    /// Set raw bits of double register `d{i}`.
    pub fn set_d_bits(&mut self, i: usize, v: u64) {
        // SAFETY: 64 contiguous u32 slots.
        let e = unsafe { std::slice::from_raw_parts_mut(ffi::bhpc_jit_ext_regs(self.jit), 64) };
        e[2 * i] = v as u32;
        e[2 * i + 1] = (v >> 32) as u32;
    }
    /// CPSR.
    pub fn cpsr(&self) -> u32 {
        // SAFETY: valid jit.
        unsafe { ffi::bhpc_jit_cpsr(self.jit) }
    }
    /// FPSCR.
    pub fn fpscr(&self) -> u32 {
        // SAFETY: valid jit.
        unsafe { ffi::bhpc_jit_fpscr(self.jit) }
    }
    /// Set FPSCR.
    pub fn set_fpscr(&mut self, v: u32) {
        // SAFETY: valid jit.
        unsafe { ffi::bhpc_jit_set_fpscr(self.jit, v) }
    }

    /// Point execution at `addr`; bit 0 selects Thumb like `bx`.
    pub fn set_pc(&mut self, addr: u32) {
        let mut cpsr = self.cpsr() & !CPSR_T;
        if addr & 1 != 0 {
            cpsr |= CPSR_T;
        }
        // SAFETY: valid jit.
        unsafe { ffi::bhpc_jit_set_cpsr(self.jit, cpsr) };
        self.set_reg(R_PC, addr & !1);
    }

    /// Outermost CPU of this nested-call chain.
    fn root(&mut self) -> &mut Cpu {
        let mut p: *mut Cpu = self;
        // SAFETY: `parent` pointers form a chain of live boxed CPUs ending in a null pointer.
        unsafe {
            while !(*p).parent.is_null() {
                p = (*p).parent;
            }
            &mut *p
        }
    }

    /// Drop all translated code (of this CPU's whole nested-call chain).
    pub fn clear_cache(&mut self) {
        let mut c = Some(self.root());
        while let Some(cpu) = c {
            // SAFETY: valid jit.
            unsafe { ffi::bhpc_jit_clear_cache(cpu.jit) };
            c = cpu.child.as_deref_mut();
        }
    }

    /// Invalidate translated code for `[start, start+len)` (cacheflush) in this CPU's whole
    /// nested-call chain. Invalidating a running JIT makes it leave `Run()`; [`Cpu::run`] resumes
    /// transparently. Other threads' JITs are not touched (call this on each of them).
    pub fn invalidate(&mut self, start: u32, len: u32) {
        let mut c = Some(self.root());
        while let Some(cpu) = c {
            // SAFETY: valid jit.
            unsafe { ffi::bhpc_jit_invalidate(cpu.jit, start, len as usize) };
            c = cpu.child.as_deref_mut();
        }
    }

    // ---------- thunk argument/return ABI (AAPCS softfp) ----------

    /// Cursor over the arguments of the thunk being executed.
    pub fn args(&self) -> ArgCursor {
        ArgCursor { core: 0, stack: self.sp() }
    }

    /// Set the 32-bit return value (r0).
    pub fn ret_u32(&mut self, v: u32) {
        self.set_reg(0, v);
    }
    /// Set a 64-bit return value (r0 = low, r1 = high).
    pub fn ret_u64(&mut self, v: u64) {
        self.set_reg(0, v as u32);
        self.set_reg(1, (v >> 32) as u32);
    }
    /// Set a double return value (softfp: in r0:r1).
    pub fn ret_f64(&mut self, v: f64) {
        self.ret_u64(v.to_bits());
    }

    /// Make the stub of the running thunk continue at `target` instead of returning to the caller
    /// (bit 0 selects Thumb). `lr` is untouched, so set it first if the target should return
    /// somewhere else. Only meaningful from inside a thunk.
    pub fn tail_call(&mut self, target: u32) {
        self.set_reg(R_IP, target);
    }

    /// Ask the run loop to stop with `reason` after the current thunk returns.
    pub fn request_stop(&mut self, reason: Stop) {
        // The first stop reason wins; later events in the same block are consequences of it.
        if self.stop.is_none() {
            self.stop = Some(reason);
        }
        // SAFETY: valid jit; halting from a callback is the supported way to leave Run().
        unsafe { ffi::bhpc_jit_halt(self.jit) };
    }

    /// True if this CPU counts cycles (the watchdog is armed).
    pub fn cycle_counting(&self) -> bool {
        self.opts.cycle_counting
    }

    /// r0..r15 as they were when the last top-level [`Cpu::call`] failed (taken from the innermost
    /// nested call if the failure happened there); `None` if it succeeded or nothing ran.
    pub fn stop_regs(&self) -> Option<[u32; 16]> {
        self.stop_regs
    }

    // ---------- running ----------

    /// Run from the current pc until the guest stops.
    pub fn run(&mut self) -> Stop {
        self.stop = None;
        self.running = true;
        let stop = loop {
            // SAFETY: valid jit; callbacks use `self` through the raw pointer registered at creation.
            let halt = unsafe {
                ffi::bhpc_jit_clear_halt(self.jit);
                ffi::bhpc_jit_run(self.jit, (self as *mut Cpu).cast())
            };
            match self.stop.take() {
                Some(s) => break s,
                // A cacheflush inside a thunk only asks the JIT to re-enter with fresh code.
                None if halt == HALT_CACHE_INVALIDATION => continue,
                None => break Stop::Halted(halt),
            }
        };
        self.running = false;
        stop
    }

    /// Call the guest function at `addr` (bit 0 = Thumb) with word arguments, run it to completion
    /// and return r0/r1. The caller must have set up a valid stack pointer.
    pub fn call(&mut self, addr: u32, args: &[u32]) -> Result<CallResult, Stop> {
        self.call_with(addr, args, None)
    }

    /// Like [`Cpu::call`], but executes one instruction at a time and invokes `on_step` with the
    /// pc before each one (debugging aid; very slow).
    pub fn call_traced(&mut self, addr: u32, args: &[u32], on_step: &mut dyn FnMut(&Cpu)) -> Result<CallResult, Stop> {
        self.call_with(addr, args, Some(on_step))
    }

    fn call_with(
        &mut self,
        addr: u32,
        args: &[u32],
        on_step: Option<&mut dyn FnMut(&Cpu)>,
    ) -> Result<CallResult, Stop> {
        if self.running {
            return self.call_nested(addr, args, on_step.is_none());
        }
        self.stop_regs = None;
        let saved_sp = self.sp();
        let saved_lr = self.lr();
        let saved_cpsr = self.cpsr();
        let nstack = args.len().saturating_sub(4) as u32;
        // Keep the stack 8-byte aligned at the call, as AAPCS requires.
        let sp = (saved_sp - nstack * 4) & !7;
        for (i, a) in args.iter().take(4).enumerate() {
            self.set_reg(i, *a);
        }
        for (i, a) in args.iter().skip(4).enumerate() {
            if let Err(e) = self.mem.write_u32(sp + 4 * i as u32, *a) {
                return Err(Stop::Host(format!("cannot push stack argument: {e}")));
            }
        }
        self.set_sp(sp);
        self.set_reg(R_LR, RETURN_ADDR);
        self.set_pc(addr);
        let stop = match on_step {
            None => self.run(),
            Some(f) => self.run_stepping(f),
        };
        let res = CallResult { r0: self.reg(0), r1: self.reg(1) };
        if stop != Stop::Returned && self.stop_regs.is_none() {
            self.stop_regs = Some(std::array::from_fn(|i| self.reg(i)));
        }
        self.set_sp(saved_sp);
        self.set_reg(R_LR, saved_lr);
        // SAFETY: valid jit.
        unsafe { ffi::bhpc_jit_set_cpsr(self.jit, saved_cpsr) };
        match stop {
            Stop::Returned => Ok(res),
            other => Err(other),
        }
    }

    /// Run a guest call from inside a thunk on a child CPU (see the crate docs).
    fn call_nested(&mut self, addr: u32, args: &[u32], untraced: bool) -> Result<CallResult, Stop> {
        if !untraced || self.depth >= MAX_NEST_DEPTH {
            return Err(Stop::Nested);
        }
        if self.child.is_none() {
            let opts = CpuOptions { code_cache_bytes: self.opts.code_cache_bytes.min(CHILD_CODE_CACHE), ..self.opts };
            let mut child = Self::build(self.shared.clone(), opts, self.pid, false, self.depth + 1);
            child.parent = self;
            self.child = Some(child);
        }
        let (sp, fpscr, trace, ticks) = (self.sp(), self.fpscr(), self.trace, self.ticks());
        let counting = self.opts.cycle_counting;
        let child = self.child.as_deref_mut().expect("child created above");
        child.set_sp(sp);
        child.set_fpscr(fpscr);
        child.trace = trace;
        if counting {
            child.set_ticks(ticks);
        }
        let res = child.call_with(addr, args, None);
        self.stop_regs = child.stop_regs.take();
        if counting {
            let left = child.ticks();
            self.set_ticks(left);
        }
        res
    }

    /// Call a guest function from inside a thunk; if it fails, stop this CPU with the same
    /// reason and return `None`. The usual way for `qsort`-style thunks to call back.
    pub fn call_or_stop(&mut self, addr: u32, args: &[u32]) -> Option<CallResult> {
        match self.call(addr, args) {
            Ok(r) => Some(r),
            Err(stop) => {
                self.request_stop(stop);
                None
            }
        }
    }

    fn run_stepping(&mut self, on_step: &mut dyn FnMut(&Cpu)) -> Stop {
        self.stop = None;
        self.running = true;
        // SAFETY: valid jit.
        unsafe { ffi::bhpc_jit_clear_halt(self.jit) };
        let mut last = 0;
        while self.stop.is_none() {
            on_step(self);
            // SAFETY: valid jit; callbacks reach `self` through the registered raw pointer.
            last = unsafe { ffi::bhpc_jit_step(self.jit, (self as *mut Cpu).cast()) };
            // A plain single step reports HaltReason::Step (bit 0); anything else is a real halt.
            if last & !1 != 0 {
                break;
            }
        }
        self.running = false;
        self.stop.take().unwrap_or(Stop::Halted(last))
    }
}

impl Drop for Cpu {
    fn drop(&mut self) {
        // SAFETY: freeing the JIT we created, exactly once.
        unsafe { ffi::bhpc_jit_free(self.jit) };
        if self.owns_pid {
            self.shared.free_pid(self.pid);
        }
    }
}

/// Reads thunk arguments following AAPCS (softfp, so doubles are passed in core registers).
#[derive(Debug, Clone, Copy)]
pub struct ArgCursor {
    core: u32,
    stack: u32,
}

impl ArgCursor {
    /// Next 32-bit argument.
    pub fn u32(&mut self, cpu: &Cpu) -> u32 {
        if self.core < 4 {
            let v = cpu.reg(self.core as usize);
            self.core += 1;
            v
        } else {
            let v = cpu.mem.read_u32(self.stack).unwrap_or(0);
            self.stack += 4;
            v
        }
    }

    /// Next 64-bit argument (aligned to an even register / 8-byte stack slot).
    pub fn u64(&mut self, cpu: &Cpu) -> u64 {
        if self.core < 4 {
            self.core = (self.core + 1) & !1;
        }
        if self.core < 4 {
            let lo = cpu.reg(self.core as usize) as u64;
            let hi = cpu.reg(self.core as usize + 1) as u64;
            self.core += 2;
            hi << 32 | lo
        } else {
            self.core = 4;
            self.stack = (self.stack + 7) & !7;
            let v = cpu.mem.read_u64(self.stack).unwrap_or(0);
            self.stack += 8;
            v
        }
    }

    /// Next `double` argument.
    pub fn f64(&mut self, cpu: &Cpu) -> f64 {
        f64::from_bits(self.u64(cpu))
    }

    /// Next `float` argument (passed as its bits in a core register under softfp).
    pub fn f32(&mut self, cpu: &Cpu) -> f32 {
        f32::from_bits(self.u32(cpu))
    }
}

mod tramp {
    use super::*;

    fn cpu<'a>(user: *mut c_void) -> &'a mut Cpu {
        // SAFETY: `user` is the boxed Cpu registered in `with_options`; it outlives the JIT.
        unsafe { &mut *(user as *mut Cpu) }
    }

    fn fault(user: *mut c_void, addr: u32, size: u32, write: bool) {
        let c = cpu(user);
        c.request_stop(Stop::Fault(Fault { addr, size, write }));
    }

    macro_rules! rd {
        ($name:ident, $m:ident, $t:ty) => {
            pub extern "C" fn $name(user: *mut c_void, addr: u32) -> $t {
                match cpu(user).mem.$m(addr) {
                    Ok(v) => v,
                    Err(_) => {
                        fault(user, addr, std::mem::size_of::<$t>() as u32, false);
                        0
                    }
                }
            }
        };
    }
    macro_rules! wr {
        ($name:ident, $m:ident, $t:ty) => {
            pub extern "C" fn $name(user: *mut c_void, addr: u32, v: $t) {
                if cpu(user).mem.$m(addr, v).is_err() {
                    fault(user, addr, std::mem::size_of::<$t>() as u32, true);
                }
            }
        };
    }
    rd!(read8, read_u8, u8);
    rd!(read16, read_u16, u16);
    rd!(read32, read_u32, u32);
    rd!(read64, read_u64, u64);
    wr!(write8, write_u8, u8);
    wr!(write16, write_u16, u16);
    wr!(write32, write_u32, u32);
    wr!(write64, write_u64, u64);

    macro_rules! wx {
        ($name:ident, $m:ident, $t:ty) => {
            pub extern "C" fn $name(user: *mut c_void, addr: u32, v: $t, expected: $t) -> u8 {
                match cpu(user).mem.$m(addr, expected, v) {
                    Ok(ok) => u8::from(ok),
                    Err(_) => {
                        fault(user, addr, std::mem::size_of::<$t>() as u32, true);
                        0
                    }
                }
            }
        };
    }
    wx!(write_excl8, cas_u8, u8);
    wx!(write_excl16, cas_u16, u16);
    wx!(write_excl32, cas_u32, u32);
    wx!(write_excl64, cas_u64, u64);

    pub extern "C" fn svc(user: *mut c_void, swi: u32) {
        let c = cpu(user);
        if c.stop.is_some() {
            return; // already stopping (for example after a fault earlier in this block)
        }
        if swi == 0 {
            c.request_stop(Stop::Returned);
            return;
        }
        let slot = swi as usize;
        if c.trace {
            let name = c.shared.table.name_of_slot(slot).unwrap_or_else(|| "?".into());
            let th = std::thread::current();
            eprintln!(
                "[thunk{}] {name}({:#x}, {:#x}, {:#x}, {:#x}) lr={:#x}",
                th.name().filter(|n| *n != "main").map_or(String::new(), |n| format!(" {n}")),
                c.reg(0),
                c.reg(1),
                c.reg(2),
                c.reg(3),
                c.reg(12)
            );
        }
        match c.shared.table.handler(slot) {
            Some(h) => {
                if h.invoke(c).is_err() {
                    c.request_stop(Stop::Nested);
                }
            }
            None => c.request_stop(Stop::BadSvc(swi)),
        }
    }

    pub extern "C" fn exception(user: *mut c_void, pc: u32, kind: u32) {
        let c = cpu(user);
        let (word, thumb) = c.instruction_at(pc);
        c.request_stop(match kind {
            EXC_UNDEFINED => Stop::Undefined { pc, word, thumb },
            EXC_UNPREDICTABLE => Stop::Unpredictable { pc, word, thumb },
            _ => Stop::Exception { pc, kind },
        });
    }

    pub extern "C" fn fallback(user: *mut c_void, pc: u32, _n: usize) {
        cpu(user).request_stop(Stop::Fallback { pc });
    }
}

#[cfg(test)]
mod tests;

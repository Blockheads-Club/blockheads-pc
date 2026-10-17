//! ARM32 guest CPU: dynarmic A32 JIT plus the SVC thunk ABI.
//!
//! Imported symbols resolve to *thunks*: 16-byte stubs `mov r12, lr; svc #n; bx r12` in a reserved
//! guest page. Because the stub returns through `r12`, a host function can redirect control
//! (tail-call a guest function, `longjmp`) with [`Cpu::tail_call`] without a nested JIT run.
//! When the guest executes one, the JIT calls back into [`Cpu`], which dispatches to the host
//! closure registered for `n`. Arguments follow the AAPCS **softfp** convention (r0-r3, then the
//! stack; 64-bit values in aligned register pairs), results go to r0 (and r1 for 64-bit values).

mod ffi;

use bhpc_mem::{GuestMemory, MemError};
use std::collections::HashMap;
use std::ffi::c_void;
use std::fmt;
use std::sync::Arc;

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

const R_IP: usize = 12;
const R_SP: usize = 13;
const R_LR: usize = 14;
const R_PC: usize = 15;
const CPSR_T: u32 = 1 << 5;
const CPSR_USER: u32 = 0x10;

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
    /// Undefined/unpredictable instruction, breakpoint... (`kind` is dynarmic's exception number).
    Exception { pc: u32, kind: u32 },
    /// The JIT asked for interpretation of an instruction it cannot translate.
    Fallback { pc: u32 },
    /// `svc #n` with no host function registered for `n`.
    BadSvc(u32),
    /// A host function requested the stop (for example an unimplemented import).
    Host(String),
    /// [`Cpu::call`] was used re-entrantly from inside a thunk.
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
            Stop::Exception { pc, kind } => write!(f, "guest exception {kind} at pc {pc:#010x}"),
            Stop::Fallback { pc } => write!(f, "interpreter fallback requested at pc {pc:#010x}"),
            Stop::BadSvc(n) => write!(f, "svc #{n} has no host function"),
            Stop::Host(m) => write!(f, "host stop: {m}"),
            Stop::Nested => write!(f, "nested guest call from a thunk is not supported"),
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
}

impl fmt::Display for CpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CpuError::Mem(e) => write!(f, "guest memory: {e}"),
            CpuError::TooManyThunks => write!(f, "thunk page is full"),
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

impl Default for CpuOptions {
    fn default() -> Self {
        CpuOptions { fastmem: true, code_cache_bytes: 64 << 20, cycle_counting: false }
    }
}

/// A host function bound to an `svc` number.
pub type ThunkFn = Box<dyn FnMut(&mut Cpu) + Send>;

struct Thunk {
    name: String,
    f: Option<ThunkFn>,
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

/// One guest hardware thread: a JIT instance plus the thunk table.
pub struct Cpu {
    jit: *mut ffi::bhpc_jit,
    mem: Arc<GuestMemory>,
    thunks: Vec<Thunk>,
    by_name: HashMap<String, u32>,
    stop: Option<Stop>,
    running: bool,
    trace: bool,
}

// SAFETY: a Cpu is only ever driven from one thread at a time (it owns its JIT instance).
unsafe impl Send for Cpu {}

impl Cpu {
    /// Create a CPU with default options.
    pub fn new(mem: Arc<GuestMemory>) -> Result<Box<Cpu>, CpuError> {
        Self::with_options(mem, CpuOptions::default())
    }

    /// Create a CPU: maps the thunk page and writes the return stub.
    pub fn with_options(mem: Arc<GuestMemory>, opts: CpuOptions) -> Result<Box<Cpu>, CpuError> {
        mem.map(THUNK_BASE, THUNK_REGION)?;
        let mut cpu = Box::new(Cpu {
            jit: std::ptr::null_mut(),
            mem: mem.clone(),
            thunks: Vec::new(),
            by_name: HashMap::new(),
            stop: None,
            running: false,
            trace: false,
        });
        cpu.thunks.push(Thunk { name: "<return>".into(), f: None });
        cpu.write_stub(0)?;
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
        // SAFETY: callbacks outlive the JIT (both owned by the boxed Cpu); `fast` is our reservation.
        cpu.jit = unsafe { ffi::bhpc_jit_new(&cb, fast, opts.code_cache_bytes, i32::from(opts.cycle_counting)) };
        let cpsr = CPSR_USER;
        // SAFETY: jit was just created.
        unsafe { ffi::bhpc_jit_set_cpsr(cpu.jit, cpsr) };
        Ok(cpu)
    }

    fn write_stub(&self, slot: u32) -> Result<(), MemError> {
        let a = THUNK_BASE + slot * THUNK_STRIDE;
        self.mem.write_u32(a, 0xE1A0_C00E)?; // mov r12, lr
        self.mem.write_u32(a + 4, 0xEF00_0000 | slot)?; // svc #slot
        self.mem.write_u32(a + 8, 0xE12F_FF1C)?; // bx r12
        self.mem.write_u32(a + 12, 0xE7F0_00F0) // udf #0 (never reached)
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

    // ---------- thunks ----------

    /// Register (or look up) the thunk for `name`; returns its guest address (ARM, bit 0 clear).
    pub fn register_thunk(&mut self, name: &str, f: ThunkFn) -> Result<u32, CpuError> {
        if let Some(&slot) = self.by_name.get(name) {
            self.thunks[slot as usize].f = Some(f);
            return Ok(THUNK_BASE + slot * THUNK_STRIDE);
        }
        let slot = self.thunks.len() as u32;
        if slot > MAX_THUNKS {
            return Err(CpuError::TooManyThunks);
        }
        self.write_stub(slot)?;
        self.thunks.push(Thunk { name: name.to_string(), f: Some(f) });
        self.by_name.insert(name.to_string(), slot);
        Ok(THUNK_BASE + slot * THUNK_STRIDE)
    }

    /// Guest address of an already registered thunk.
    pub fn thunk_addr(&self, name: &str) -> Option<u32> {
        self.by_name.get(name).map(|s| THUNK_BASE + s * THUNK_STRIDE)
    }

    /// Name of the thunk at guest address `addr`, if any.
    pub fn thunk_name(&self, addr: u32) -> Option<&str> {
        let off = addr.checked_sub(THUNK_BASE)?;
        if off % THUNK_STRIDE != 0 {
            return None;
        }
        self.thunks.get((off / THUNK_STRIDE) as usize).map(|t| t.name.as_str())
    }

    /// Number of registered thunks (excluding the return stub).
    pub fn thunk_count(&self) -> usize {
        self.thunks.len() - 1
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

    /// Drop all translated code.
    pub fn clear_cache(&mut self) {
        // SAFETY: valid jit.
        unsafe { ffi::bhpc_jit_clear_cache(self.jit) };
    }

    /// Invalidate translated code for `[start, start+len)` (cacheflush).
    pub fn invalidate(&mut self, start: u32, len: u32) {
        // SAFETY: valid jit.
        unsafe { ffi::bhpc_jit_invalidate(self.jit, start, len as usize) };
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

    // ---------- running ----------

    /// Run from the current pc until the guest stops.
    pub fn run(&mut self) -> Stop {
        self.stop = None;
        self.running = true;
        // SAFETY: valid jit; callbacks use `self` through the raw pointer registered at creation.
        let halt = unsafe {
            ffi::bhpc_jit_clear_halt(self.jit);
            ffi::bhpc_jit_run(self.jit, (self as *mut Cpu).cast())
        };
        self.running = false;
        self.stop.take().unwrap_or(Stop::Halted(halt))
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
            return Err(Stop::Nested);
        }
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
        self.set_sp(saved_sp);
        self.set_reg(R_LR, saved_lr);
        // SAFETY: valid jit.
        unsafe { ffi::bhpc_jit_set_cpsr(self.jit, saved_cpsr) };
        match stop {
            Stop::Returned => Ok(res),
            other => Err(other),
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
            let name = c.thunks.get(slot).map_or("?", |t| t.name.as_str());
            eprintln!(
                "[thunk] {name}({:#x}, {:#x}, {:#x}, {:#x}) lr={:#x}",
                c.reg(0),
                c.reg(1),
                c.reg(2),
                c.reg(3),
                c.reg(12)
            );
        }
        let f = c.thunks.get_mut(slot).and_then(|t| t.f.take());
        match f {
            Some(mut f) => {
                f(c);
                // Keep a replacement the thunk may have registered for itself.
                if c.thunks[slot].f.is_none() {
                    c.thunks[slot].f = Some(f);
                }
            }
            None => c.request_stop(Stop::BadSvc(swi)),
        }
    }

    pub extern "C" fn exception(user: *mut c_void, pc: u32, kind: u32) {
        cpu(user).request_stop(Stop::Exception { pc, kind });
    }

    pub extern "C" fn fallback(user: *mut c_void, pc: u32, _n: usize) {
        cpu(user).request_stop(Stop::Fallback { pc });
    }
}

#[cfg(test)]
mod tests;

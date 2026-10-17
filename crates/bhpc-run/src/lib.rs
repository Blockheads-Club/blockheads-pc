//! M0 runner: load one ARM32 shared object, bind its imports to host thunks, run an entry point.

pub mod bench;
pub mod hle;

use bhpc_cpu::{Cpu, CpuError, CpuOptions, Stop};
use bhpc_loader::{Linker, LoadError};
use bhpc_mem::{GuestHeap, GuestMemory, MemError};
use std::fmt;
use std::io::Write;
use std::sync::Arc;

/// Guest address of the host-managed heap.
pub const HEAP_BASE: u32 = 0x4000_0000;
/// Size of the host-managed heap.
pub const HEAP_SIZE: u32 = 64 << 20;
/// Top of the main guest stack (grows down).
pub const STACK_TOP: u32 = 0x7010_0000;
/// Size of the main guest stack.
pub const STACK_SIZE: u32 = 1 << 20;

/// Anything that can go wrong while running a fixture.
#[derive(Debug)]
pub enum RunError {
    Io(std::io::Error),
    Mem(MemError),
    Cpu(CpuError),
    Load(LoadError),
    /// The module does not export the requested entry symbol.
    NoEntry(String),
    /// The guest stopped abnormally.
    Guest(Stop),
    /// The watchdog budget ran out; the guest was at `pc` with return address `lr`.
    Watchdog {
        pc: u32,
        lr: u32,
    },
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Io(e) => write!(f, "io: {e}"),
            RunError::Mem(e) => write!(f, "{e}"),
            RunError::Cpu(e) => write!(f, "{e}"),
            RunError::Load(e) => write!(f, "load: {e}"),
            RunError::NoEntry(s) => write!(f, "module does not export `{s}`"),
            RunError::Guest(s) => write!(f, "guest stopped: {s}"),
            RunError::Watchdog { pc, lr } => write!(f, "watchdog expired at pc {pc:#010x} (lr {lr:#010x})"),
        }
    }
}

impl std::error::Error for RunError {}

impl From<std::io::Error> for RunError {
    fn from(e: std::io::Error) -> Self {
        RunError::Io(e)
    }
}
impl From<MemError> for RunError {
    fn from(e: MemError) -> Self {
        RunError::Mem(e)
    }
}
impl From<CpuError> for RunError {
    fn from(e: CpuError) -> Self {
        RunError::Cpu(e)
    }
}
impl From<LoadError> for RunError {
    fn from(e: LoadError) -> Self {
        RunError::Load(e)
    }
}

/// Outcome of a fixture run.
#[derive(Debug, PartialEq, Eq)]
pub struct Report {
    /// Value returned by the entry function (number of failed checks for `hello_main`).
    pub exit_code: u32,
    /// `(name, passed)` for every `bhpc_check` the guest made.
    pub checks: Vec<(String, bool)>,
    /// Constructors executed before the entry point.
    pub constructors: usize,
    /// Relocations applied while loading.
    pub relocations: usize,
    /// Distinct host thunks bound.
    pub thunks: usize,
}

/// Create the standard guest environment: memory, heap, stack and a CPU.
pub fn new_env(out: Box<dyn Write + Send>, watchdog: Option<u64>) -> Result<(Box<Cpu>, hle::Shared), RunError> {
    let mem = Arc::new(GuestMemory::new()?);
    let heap = GuestHeap::new(mem.clone(), HEAP_BASE, HEAP_SIZE)?;
    mem.map(STACK_TOP - STACK_SIZE, STACK_SIZE)?;
    let mut cpu = Cpu::with_options(mem, CpuOptions { cycle_counting: watchdog.is_some(), ..CpuOptions::default() })?;
    if let Some(t) = watchdog {
        cpu.set_ticks(t);
    }
    cpu.set_sp(STACK_TOP);
    let st = hle::new_state(heap, out);
    Ok((cpu, st))
}

/// Load `so_bytes`, run its constructors, then call the exported `entry` with no arguments.
pub fn run_shared_object(
    so_bytes: &[u8],
    entry: &str,
    out: Box<dyn Write + Send>,
    trace: bool,
    watchdog: Option<u64>,
    pc_trace: Option<&mut dyn FnMut(&Cpu)>,
) -> Result<Report, RunError> {
    let (mut cpu, st) = new_env(out, watchdog)?;
    cpu.set_trace_thunks(trace);
    let mut linker = Linker::new(cpu.mem().clone());
    let module = {
        let mut hle = hle::Hle::new(&mut cpu, st.clone());
        linker.load("fixture.so", so_bytes, &mut hle)?.clone()
    };
    if let Some((table, count)) = module.exidx {
        st.lock().unwrap().exidx.push(hle::ExidxRange { start: module.start, size: module.size, table, count });
    }
    let ctors = module.constructors();
    for c in &ctors {
        cpu.call(*c, &[]).map_err(RunError::Guest)?;
    }
    let entry_addr = module.export(entry).ok_or_else(|| RunError::NoEntry(entry.to_string()))?;
    let result = match pc_trace {
        Some(f) => cpu.call_traced(entry_addr, &[], f),
        None => cpu.call(entry_addr, &[]),
    };
    let r = result.map_err(|e| match e {
        Stop::Halted(_) if watchdog.is_some() => RunError::Watchdog { pc: cpu.pc(), lr: cpu.lr() },
        other => RunError::Guest(other),
    })?;
    let checks = std::mem::take(&mut st.lock().unwrap().checks);
    Ok(Report {
        exit_code: r.r0,
        checks,
        constructors: ctors.len(),
        relocations: module.relocs_applied,
        thunks: cpu.thunk_count(),
    })
}

#[cfg(test)]
mod tests;

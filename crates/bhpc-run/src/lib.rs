//! M0 runner: load one ARM32 shared object, bind its imports to host thunks, run an entry point.

pub mod apkrun;
pub mod aspect;
pub mod bench;
pub mod desktop;
pub mod dl;
pub mod embed;
pub mod fastdiv;
pub mod fpsunlock;
pub mod framestats;
pub mod gamespeed;
pub mod gameui;
pub mod input;
pub mod mainq;
pub mod mktimer;
pub mod mmapio;
pub mod music;
pub mod overlay;
pub mod pace;
#[cfg(feature = "play")]
pub mod play;
pub mod png;
pub mod script;
pub mod tracks;
pub mod vfd;

use bhpc_cpu::{Cpu, CpuError, CpuOptions, Stop};
use bhpc_loader::{Linker, LoadError};
use bhpc_mem::{GuestHeap, GuestMemory, MemError};
use std::fmt;
use std::io::Write;
use std::sync::Arc;

/// Guest address of the host-managed heap.
pub const HEAP_BASE: u32 = 0x4000_0000;
/// Size of the host-managed heap.
pub const HEAP_SIZE: u32 = 0x2E00_0000;
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
    Apk(bhpc_loader::apk::ApkError),
    Jni(bhpc_jni::JniError),
    Vfs(bhpc_vfs::VfsError),
    /// The module does not export the requested entry symbol.
    NoEntry(String),
    /// The guest stopped abnormally.
    Guest(Stop),
    /// The watchdog budget ran out; the guest was at `pc` with return address `lr`.
    Watchdog {
        pc: u32,
        lr: u32,
    },
    /// In-window mode (`--embed`) could not get a GL context without a window of its own; the launcher then falls
    /// back to a separate game window.
    EmbedUnavailable(String),
}

impl RunError {
    /// The process exit code of a run that failed with this error.
    pub fn exit_code(&self) -> u8 {
        match self {
            RunError::EmbedUnavailable(_) => bhpc_launcher::embed::EMBED_UNAVAILABLE_EXIT as u8,
            _ => 1,
        }
    }
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Io(e) => write!(f, "io: {e}"),
            RunError::Mem(e) => write!(f, "{e}"),
            RunError::Cpu(e) => write!(f, "{e}"),
            RunError::Load(e) => write!(f, "load: {e}"),
            RunError::Apk(e) => write!(f, "{e}"),
            RunError::Jni(e) => write!(f, "jni: {e}"),
            RunError::Vfs(e) => write!(f, "vfs: {e}"),
            RunError::NoEntry(s) => write!(f, "module does not export `{s}`"),
            RunError::Guest(s) => write!(f, "guest stopped: {s}"),
            RunError::Watchdog { pc, lr } => write!(f, "watchdog expired at pc {pc:#010x} (lr {lr:#010x})"),
            RunError::EmbedUnavailable(e) => write!(f, "[embed] unavailable: {e}"),
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
impl From<bhpc_loader::apk::ApkError> for RunError {
    fn from(e: bhpc_loader::apk::ApkError) -> Self {
        RunError::Apk(e)
    }
}
impl From<bhpc_vfs::VfsError> for RunError {
    fn from(e: bhpc_vfs::VfsError) -> Self {
        RunError::Vfs(e)
    }
}

impl From<bhpc_jni::JniError> for RunError {
    fn from(e: bhpc_jni::JniError) -> Self {
        RunError::Jni(e)
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
pub fn new_env(out: Box<dyn Write + Send>, watchdog: Option<u64>) -> Result<(Box<Cpu>, bhpc_bionic::Shared), RunError> {
    new_env_with(bhpc_bionic::Config { stdout: out, ..bhpc_bionic::Config::default() }, watchdog)
}

/// Like [`new_env`] with a full bionic configuration (file system, log sink ...).
pub fn new_env_with(
    cfg: bhpc_bionic::Config,
    watchdog: Option<u64>,
) -> Result<(Box<Cpu>, bhpc_bionic::Shared), RunError> {
    let mem = Arc::new(GuestMemory::new()?);
    let heap = GuestHeap::new(mem.clone(), HEAP_BASE, HEAP_SIZE)?;
    let mem_for_state = mem.clone();
    mem.map(STACK_TOP - STACK_SIZE, STACK_SIZE)?;
    let mut cpu = Cpu::with_options(mem, CpuOptions { cycle_counting: watchdog.is_some(), ..CpuOptions::default() })?;
    if let Some(t) = watchdog {
        cpu.set_ticks(t);
    }
    cpu.set_sp(STACK_TOP);
    let st = bhpc_bionic::State::new(mem_for_state, heap, cfg)?;
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
        let mut hle = bhpc_bionic::Hle::new(&mut cpu, st.clone());
        linker.load("fixture.so", so_bytes, &mut hle)?.clone()
    };
    if let Some((table, count)) = module.exidx {
        bhpc_bionic::lock(&st).exidx.push(bhpc_bionic::ExidxRange {
            start: module.start,
            size: module.size,
            table,
            count,
        });
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
    let checks = std::mem::take(&mut bhpc_bionic::lock(&st).checks);
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
#[cfg(test)]
mod tests_aspect;
#[cfg(test)]
mod tests_desktop;
#[cfg(test)]
mod tests_dl;
#[cfg(test)]
mod tests_embed;
#[cfg(test)]
mod tests_fastdiv;
#[cfg(test)]
mod tests_fpsunlock;
#[cfg(test)]
mod tests_gamespeed;
#[cfg(test)]
mod tests_gameui;
#[cfg(test)]
mod tests_input;
#[cfg(test)]
mod tests_mktimer;
#[cfg(test)]
mod tests_mmapio;
#[cfg(test)]
mod tests_music;
#[cfg(all(test, unix))]
mod tests_net;
#[cfg(test)]
mod tests_overlay;
#[cfg(test)]
mod tests_script;
#[cfg(test)]
mod tests_timerfd;
#[cfg(test)]
mod tests_tracks;
#[cfg(test)]
mod tests_vfd;

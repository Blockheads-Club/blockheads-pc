//! Constructor runner: `DT_INIT` then `DT_INIT_ARRAY` of each module, in dependency (load) order.
//!
//! The loader does not execute guest code. The CPU side implements [`GuestCaller`]; the runner drives it,
//! applies the per-constructor watchdog and feeds [`InitTrace`] hooks. See `docs/m1/loader-api.md`.

use crate::Module;
use std::fmt;
use std::time::{Duration, Instant};

/// Why one guest call did not return normally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallFailure {
    /// The cycle budget expired; `pc` is where the guest was stopped.
    Watchdog {
        /// Guest program counter at expiry.
        pc: u32,
    },
    /// The guest faulted or hit an unsupported instruction.
    Fault {
        /// Guest program counter of the fault.
        pc: u32,
        /// Human-readable reason (symbolised by the caller if it can).
        reason: String,
    },
}

impl fmt::Display for CallFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallFailure::Watchdog { pc } => write!(f, "watchdog expired at pc {pc:#x}"),
            CallFailure::Fault { pc, reason } => write!(f, "fault at pc {pc:#x}: {reason}"),
        }
    }
}

/// Runs one guest function to completion. Implemented by the CPU layer (`bhpc-cpu` + thunk dispatch).
pub trait GuestCaller {
    /// Call the ARM function at `entry` with `args` in `r0..r2`, executing at most `max_cycles`
    /// guest cycles (`0` = unlimited). Returns the value of `r0` on return.
    fn call(&mut self, entry: u32, args: [u32; 3], max_cycles: u64) -> Result<u32, CallFailure>;
}

/// Which constructor entry of a module is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtorKind {
    /// `DT_INIT`.
    Init,
    /// Entry `n` of `DT_INIT_ARRAY`.
    InitArray(usize),
}

/// One constructor about to run or just finished.
#[derive(Debug, Clone, Copy)]
pub struct CtorInfo<'a> {
    /// Module load name.
    pub module: &'a str,
    /// Which entry.
    pub kind: CtorKind,
    /// Guest address of the function.
    pub entry: u32,
}

/// Trace hooks around every constructor. All methods default to no-ops.
pub trait InitTrace {
    /// Called right before the constructor runs.
    fn before(&mut self, _ctor: &CtorInfo<'_>) {}
    /// Called after it ran (or failed), with the wall-clock time it took.
    fn after(&mut self, _ctor: &CtorInfo<'_>, _result: &Result<u32, CallFailure>, _elapsed: Duration) {}
}

/// Trace sink that ignores everything.
pub struct NoTrace;

impl InitTrace for NoTrace {}

/// Knobs of [`run_constructors`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitOptions {
    /// Per-constructor cycle budget handed to the caller (`0` = unlimited).
    pub watchdog_cycles: u64,
    /// `r0..r2` for every constructor (Android passes `argc, argv, envp`).
    pub args: [u32; 3],
    /// Keep going after a failed constructor instead of stopping at the first failure.
    pub keep_going: bool,
}

impl Default for InitOptions {
    fn default() -> Self {
        InitOptions { watchdog_cycles: 2_000_000_000, args: [0; 3], keep_going: false }
    }
}

/// A constructor that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CtorFailure {
    /// Module load name.
    pub module: String,
    /// Which entry.
    pub kind: CtorKind,
    /// Guest address of the function.
    pub entry: u32,
    /// What went wrong.
    pub failure: CallFailure,
}

impl fmt::Display for CtorFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {:?} @ {:#x}: {}", self.module, self.kind, self.entry, self.failure)
    }
}

/// What [`run_constructors`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InitReport {
    /// Constructors that returned normally.
    pub ran: usize,
    /// Per module: name and number of constructors that returned normally, in run order.
    pub per_module: Vec<(String, usize)>,
    /// Failed constructors (at most one unless `keep_going`).
    pub failures: Vec<CtorFailure>,
}

impl InitReport {
    /// True when no constructor failed.
    pub fn ok(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Run the constructors of `modules` in the given order (pass `&linker.modules()[range]` from
/// [`crate::LoadSummary::range`]). Stops at the first failure unless `opts.keep_going`.
pub fn run_constructors(
    modules: &[Module],
    caller: &mut dyn GuestCaller,
    trace: &mut dyn InitTrace,
    opts: &InitOptions,
) -> InitReport {
    let mut report = InitReport::default();
    for m in modules {
        let mut ran = 0;
        let ctors = m
            .init
            .iter()
            .map(|e| (CtorKind::Init, *e))
            .chain(m.init_array.iter().enumerate().map(|(i, e)| (CtorKind::InitArray(i), *e)));
        for (kind, entry) in ctors {
            let info = CtorInfo { module: &m.name, kind, entry };
            trace.before(&info);
            let t0 = Instant::now();
            let result = caller.call(entry, opts.args, opts.watchdog_cycles);
            trace.after(&info, &result, t0.elapsed());
            match result {
                Ok(_) => {
                    ran += 1;
                    report.ran += 1;
                }
                Err(failure) => {
                    report.failures.push(CtorFailure { module: m.name.clone(), kind, entry, failure });
                    if !opts.keep_going {
                        report.per_module.push((m.name.clone(), ran));
                        return report;
                    }
                }
            }
        }
        report.per_module.push((m.name.clone(), ran));
    }
    report
}

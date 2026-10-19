//! Host re-implementation of the bionic libc subset that the Blockheads guest libraries import
//! (constructors and early initialisation first). See `docs/feasibility/A-native-imports.md`.
//!
//! Layout of the crate:
//! * [`state`]: the shared host state ([`State`], [`Shared`]) and its construction ([`Config`]);
//! * [`layout`]: guest-visible struct layouts and constants of the ARM bionic ABI;
//! * [`fmt`]: the bionic-compatible `printf` engine;
//! * one module per libc area (`string`, `heap`, `stdio`, `timefns`, `pthread`, `misc`, `ctype`,
//!   `fixture`), each exporting a table of `name -> host function`;
//! * this file: the registry ([`builtin`], [`resolve`], [`register_all`]) and the loader adapter [`Hle`].
//!
//! Every host function has the shape `fn(&mut Cpu, &Shared)` and reads its arguments with
//! [`Cpu::args`] (AAPCS softfp: doubles travel in core registers).

pub mod ctype;
pub mod fixture;
pub mod fmt;
pub mod guest;
pub mod heap;
pub mod inet;
pub mod layout;
pub mod libm;
pub mod misc;
pub mod pthread;
pub(crate) mod scan;
pub mod sched;
pub mod state;
pub mod stdio;
pub mod strftime;
pub mod string;
pub mod strptime;
pub mod sys;
pub mod timefns;
pub mod vfs;

#[cfg(test)]
pub(crate) mod tests_env;

pub use state::{new_state, AtExit, Config, ExidxRange, LogRecord, Shared, State};
pub use timefns::{HostClock, SystemClock};

use bhpc_cpu::{Cpu, SharedThunkFn, Stop};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, MutexGuard, OnceLock, PoisonError};

/// A host function bound to a libc symbol.
pub type Impl = fn(&mut Cpu, &Shared);

/// Lock the shared state, ignoring poisoning (a panicking thunk must not wedge later calls).
pub fn lock(st: &Shared) -> MutexGuard<'_, State> {
    st.lock().unwrap_or_else(PoisonError::into_inner)
}

fn tables() -> Vec<&'static [(&'static str, Impl)]> {
    let mut v = vec![
        string::TABLE,
        heap::TABLE,
        stdio::TABLE,
        timefns::TABLE,
        pthread::TABLE,
        misc::TABLE,
        ctype::TABLE,
        fixture::TABLE,
        libm::TABLE,
        inet::TABLE,
        strptime::TABLE,
    ];
    v.extend(sys::tables());
    v
}

fn registry() -> &'static HashMap<&'static str, Impl> {
    static REG: OnceLock<HashMap<&'static str, Impl>> = OnceLock::new();
    REG.get_or_init(|| tables().iter().flat_map(|t| t.iter().copied()).collect())
}

/// Every symbol this crate implements as a function, sorted.
pub fn builtin_names() -> Vec<&'static str> {
    let mut v: Vec<_> = registry().keys().copied().collect();
    v.sort_unstable();
    v
}

/// Names of the data symbols (`__sF`, `_ctype_`, ...) this crate provides.
pub const DATA_SYMBOLS: &[&str] = &[
    "__sF",
    "__stack_chk_guard",
    "_ctype_",
    "_tolower_tab_",
    "_toupper_tab_",
    "environ",
    "__page_size",
    "timezone",
    "tzname",
];

/// Built-ins that run without the host lock in parallel mode (`sched::enable_parallel`): pure functions of their
/// arguments and the guest memory they point to, and the synchronisation functions written for it (each does its
/// state update in one critical section and waits with `sched::Sched::key_seq`).
const GATE_FREE: &[&str] = &[
    // string.h, minus the ones that cache (strerror, strsignal) or keep global state (rand48)
    "memcpy",
    "memmove",
    "memset",
    "memcmp",
    "memchr",
    "memrchr",
    "memmem",
    "bcopy",
    "bzero",
    "__aeabi_memcpy",
    "__aeabi_memcpy4",
    "__aeabi_memcpy8",
    "__aeabi_memmove",
    "__aeabi_memmove4",
    "__aeabi_memmove8",
    "__aeabi_memset",
    "__aeabi_memset4",
    "__aeabi_memset8",
    "__aeabi_memclr",
    "__aeabi_memclr4",
    "__aeabi_memclr8",
    "strlen",
    "strnlen",
    "strcmp",
    "strncmp",
    "strcoll",
    "strcasecmp",
    "strncasecmp",
    "strcpy",
    "strncpy",
    "strcat",
    "strncat",
    "strlcpy",
    "strlcat",
    "strchr",
    "strrchr",
    "strstr",
    "strcasestr",
    "strpbrk",
    "strspn",
    "strcspn",
    "strxfrm",
    "strtok_r",
    "strtol",
    "strtoul",
    "strtoll",
    "strtoull",
    "atoi",
    "atol",
    "iswalnum",
    "iswdigit",
    "iswspace",
    "iswxdigit",
    "towlower",
    "towupper",
    // pthread
    "pthread_mutex_lock",
    "pthread_mutex_trylock",
    "pthread_mutex_unlock",
    "pthread_cond_wait",
    "pthread_cond_timedwait",
    "pthread_cond_timedwait_monotonic",
    "pthread_cond_timedwait_relative_np",
    "pthread_cond_signal",
    "pthread_cond_broadcast",
    "__futex_wait",
    "__futex_wake",
    "pthread_getspecific",
    "pthread_setspecific",
];

/// True if the built-in `name` may run without the host lock (see `GATE_FREE`; all of `libm` and `ctype`).
pub fn gate_free(name: &str) -> bool {
    GATE_FREE.contains(&name) || libm::TABLE.iter().chain(ctype::TABLE).any(|(n, _)| *n == name)
}

/// The built-in host function for `name`, wrapped so calls are counted in the thunk histogram.
pub fn builtin(name: &str, st: &Shared) -> Option<SharedThunkFn> {
    let f = *registry().get(name)?;
    let counter = Arc::new(AtomicU64::new(0));
    lock(st).histogram.push((name.to_string(), counter.clone()));
    let st = st.clone();
    Some(Arc::new(move |c: &mut Cpu| {
        counter.fetch_add(1, Ordering::Relaxed);
        f(c, &st)
    }))
}

/// A thunk that records the call and stops the guest loudly: bound to every unknown import.
fn trap(name: &str, st: &Shared) -> SharedThunkFn {
    let (name, st) = (name.to_string(), st.clone());
    Arc::new(move |c: &mut Cpu| {
        lock(&st).unimplemented_hits.push(name.clone());
        c.request_stop(Stop::Host(format!("unimplemented import {name}")));
    })
}

/// Bind `name` to a guest address: a data symbol, an already registered thunk, a built-in host
/// function, or (for unknown names) a trap thunk that is counted as unimplemented.
/// Returns `None` only when the thunk page is full.
pub fn resolve(cpu: &mut Cpu, st: &Shared, name: &str) -> Option<u32> {
    if let Some(a) = lock(st).data.get(name) {
        return Some(*a);
    }
    if let Some(a) = cpu.thunk_addr(name) {
        return Some(a);
    }
    let f = builtin(name, st).unwrap_or_else(|| {
        lock(st).unimplemented_bound.push(name.to_string());
        trap(name, st)
    });
    let addr = cpu.register_shared_thunk(name, f).ok()?;
    if gate_free(name) {
        cpu.table().set_gate_free(name);
    }
    Some(addr)
}

/// Pre-bind every built-in function (useful to fill the thunk table up front). Returns the count.
pub fn register_all(cpu: &mut Cpu, st: &Shared) -> usize {
    builtin_names().into_iter().filter(|n| resolve(cpu, st, n).is_some()).count()
}

/// Call counts per bound built-in, most called first (the "thunk histogram").
pub fn histogram(st: &Shared) -> Vec<(String, u64)> {
    let mut v: Vec<_> = lock(st).histogram.iter().map(|(n, c)| (n.clone(), c.load(Ordering::Relaxed))).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

/// Resolves guest imports for the loader: data symbols, built-in host functions, then trap thunks.
pub struct Hle<'a> {
    cpu: &'a mut Cpu,
    st: Shared,
}

impl<'a> Hle<'a> {
    pub fn new(cpu: &'a mut Cpu, st: Shared) -> Self {
        Hle { cpu, st }
    }
}

/// Imports the libraries declare weak and test against NULL before use (Apportable's Java-side hook).
/// The host leaves them unresolved so the linker binds weak references to 0.
pub const WEAK_NULL: &[&str] = &["__load_bridge"];

impl bhpc_loader::HostSymbols for Hle<'_> {
    fn resolve(&mut self, name: &str) -> Option<u32> {
        if WEAK_NULL.contains(&name) {
            return None;
        }
        resolve(self.cpu, &self.st, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_free_built_ins_exist_and_are_marked_when_bound() {
        for n in GATE_FREE {
            assert!(registry().contains_key(n), "{n} is not a built-in");
        }
        assert!(gate_free("sqrt") && gate_free("isalpha") && gate_free("memcpy"));
        assert!(!gate_free("pthread_create") && !gate_free("strerror"));
        let mut e = crate::tests_env::Env::new();
        e.addr("memcpy");
        e.addr("malloc");
        let stats: Vec<_> = {
            e.cpu.table().set_stats(true);
            let (a, b) = (e.addr("memcpy"), e.addr("malloc"));
            e.cpu.call(a, &[crate::tests_env::SCRATCH, crate::tests_env::SCRATCH, 0]).unwrap();
            e.cpu.call(b, &[8]).unwrap();
            e.cpu.table().stats()
        };
        let free = |n: &str| stats.iter().find(|t| t.name == n).unwrap().free;
        assert!(free("memcpy") && !free("malloc"));
    }
    use crate::tests_env::{Env, HEAP_BASE, SCRATCH};
    use bhpc_loader::HostSymbols;

    #[test]
    fn registry_is_consistent() {
        let names = builtin_names();
        let total: usize = tables().iter().map(|t| t.len()).sum();
        assert_eq!(names.len(), total, "a symbol is registered twice");
        assert!(names.windows(2).all(|w| w[0] < w[1]));
        for n in ["malloc", "snprintf", "pthread_once", "__cxa_atexit", "__android_log_print", "__system_property_get"]
        {
            assert!(names.contains(&n), "{n}");
        }
        let mut e = Env::new();
        assert_eq!(register_all(&mut e.cpu, &e.st), names.len());
        assert_eq!(e.cpu.thunk_count(), names.len());
        assert!(builtin("no_such_symbol", &e.st).is_none());
    }

    #[test]
    fn resolve_binds_data_symbols_builtins_and_traps() {
        let mut e = Env::new();
        for n in DATA_SYMBOLS {
            let a = e.addr(n);
            assert!(a >= HEAP_BASE, "{n}");
            assert_eq!(e.addr(n), a);
        }
        assert_eq!(e.cpu.thunk_count(), 0, "data symbols are not thunks");
        let m1 = e.addr("malloc");
        assert_eq!(e.addr("malloc"), m1, "thunks are created once");
        let f = e.addr("no_such_libc_function");
        assert_eq!(e.cpu.call(f, &[]), Err(Stop::Host("unimplemented import no_such_libc_function".into())));
        let _ = e.cpu.call(f, &[]);
        let s = lock(&e.st);
        assert_eq!(s.unimplemented_bound, ["no_such_libc_function"]);
        assert_eq!(s.unimplemented_hits, ["no_such_libc_function", "no_such_libc_function"]);
    }

    #[test]
    fn histogram_counts_calls() {
        let mut e = Env::new();
        e.call("malloc", &[8]);
        e.call("malloc", &[8]);
        e.call("getpid", &[]);
        e.call("getppid", &[]);
        let h = histogram(&e.st);
        assert_eq!(h[0], ("malloc".to_string(), 2));
        assert_eq!(h[1], ("getpid".to_string(), 1), "ties sort by name");
        assert_eq!(h[2], ("getppid".to_string(), 1));
        e.addr("abs_unbound_but_resolved");
        assert_eq!(histogram(&e.st).len(), 3, "traps are not in the histogram");
    }

    #[test]
    fn hle_adapter_implements_the_loader_trait() {
        let mut e = Env::new();
        let mut h = Hle::new(&mut e.cpu, e.st.clone());
        let a = h.resolve("strlen").unwrap();
        assert_eq!(h.resolve("strlen"), Some(a));
        assert!(h.resolve("__sF").is_some());
        assert_eq!(h.resolve("__load_bridge"), None, "weak hooks stay NULL");
    }

    #[test]
    fn thunk_page_exhaustion_makes_resolve_fail() {
        let mut e = Env::new();
        for i in 0..bhpc_cpu::MAX_THUNKS {
            assert!(resolve(&mut e.cpu, &e.st, &format!("sym{i}")).is_some() || i > 0);
        }
        assert_eq!(resolve(&mut e.cpu, &e.st, "one-more"), None);
    }

    #[test]
    fn a_panicking_thunk_does_not_wedge_the_state() {
        let e = Env::new();
        let st = e.st.clone();
        let _ = std::thread::spawn(move || {
            let _g = st.lock().unwrap();
            panic!("poison the mutex");
        })
        .join();
        assert!(e.st.is_poisoned());
        let g = lock(&e.st);
        assert!(g.checks.is_empty());
    }

    #[test]
    fn state_construction_variants() {
        // Compatibility constructor: custom stdout, everything else default.
        let mem = Arc::new(bhpc_mem::GuestMemory::new().unwrap());
        let heap = bhpc_mem::GuestHeap::new(mem.clone(), HEAP_BASE, 1 << 20).unwrap();
        let out = crate::tests_env::Buf::default();
        let st = new_state(mem.clone(), heap, Box::new(out.clone())).unwrap();
        {
            let mut s = lock(&st);
            std::io::Write::write_all(&mut s.stdout, b"hi").unwrap();
            assert_eq!(s.pid, std::process::id());
            assert_eq!(s.cpus, 4);
            assert!(s.props.contains_key("ro.build.version.sdk"));
            // Random guard values come from the host (not the fixed test value) and differ from zero most of the time.
            let g = mem.read_u32(s.data["__stack_chk_guard"]).unwrap();
            let _ = g;
        }
        assert_eq!(out.text(), "hi");
        // A heap too small for the data symbols is reported, not a panic.
        let mem = Arc::new(bhpc_mem::GuestMemory::new().unwrap());
        let heap = bhpc_mem::GuestHeap::new(mem.clone(), HEAP_BASE, 64).unwrap();
        assert!(State::new(mem, heap, Config::default()).is_err());
        let _ = SCRATCH;
    }
}

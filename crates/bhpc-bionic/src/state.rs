//! Shared host state and its construction.

use crate::sched::{Sched, ThreadRec};
use crate::{ctype, layout, timefns::HostClock, timefns::SystemClock};
use bhpc_mem::{GuestHeap, GuestMemory, MemError};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::Write;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

/// One loaded module's unwind table, for `__gnu_Unwind_Find_exidx`.
#[derive(Debug, Clone, Copy)]
pub struct ExidxRange {
    pub start: u32,
    pub size: u32,
    pub table: u32,
    pub count: u32,
}

/// A `__cxa_atexit` registration (recorded, never run).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtExit {
    pub func: u32,
    pub arg: u32,
    pub dso: u32,
}

/// One `__android_log_*` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    pub prio: i32,
    pub tag: String,
    pub msg: String,
}

pub type LogSink = Box<dyn FnMut(&LogRecord) + Send>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MutexState {
    pub kind: u32,
    pub count: u32,
    /// Thread id of the holder while `count > 0`.
    pub owner: u32,
    /// Threads blocked until it is unlocked (an unlock only notifies when there are some).
    pub waiters: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RwState {
    pub readers: u32,
    pub writer: bool,
}

/// An anonymous or private-file `mmap` region carved out of the guest heap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mapping {
    pub len: u32,
    /// Per page: still mapped (not yet `munmap`ed).
    pub live: Vec<bool>,
}

/// Host-side state of an open `FILE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileRec {
    pub fd: i32,
    /// Bytes pushed back with `ungetc` (last pushed first).
    pub unget: Vec<u8>,
}

pub(crate) struct EnvVar {
    pub name: Vec<u8>,
    /// Guest address of the `NAME=value` string.
    pub base: u32,
}

/// Knobs for [`State::new`].
pub struct Config {
    /// Where `stdout` output goes.
    pub stdout: Box<dyn Write + Send>,
    /// Where `stderr` output (and the default log sink) goes.
    pub stderr: Box<dyn Write + Send>,
    pub clock: Box<dyn HostClock>,
    /// Receives `__android_log_*` messages; `None` writes `P/tag: msg` lines to `stderr`.
    pub log: Option<crate::state::LogSink>,
    /// Initial environment.
    pub env: Vec<(String, String)>,
    /// System properties for `__system_property_get`.
    pub props: Vec<(String, String)>,
    pub cpus: u32,
    pub pid: u32,
    pub uid: u32,
    /// File-system view of the guest (`bhpc-vfs`: mount table, `/proc`, `/dev`, the Android path map).
    pub fs: Arc<dyn bhpc_vfs::FileSystem>,
    /// Value of `__stack_chk_guard`; random when `None`.
    pub stack_guard: Option<u32>,
}

impl Default for Config {
    fn default() -> Config {
        let s = |a: &str, b: &str| (a.to_string(), b.to_string());
        Config {
            stdout: Box::new(std::io::stdout()),
            stderr: Box::new(std::io::stderr()),
            clock: Box::new(SystemClock::new()),
            log: None,
            env: vec![s("ANDROID_ROOT", "/system"), s("ANDROID_DATA", "/data"), s("EXTERNAL_STORAGE", "/sdcard")],
            props: vec![
                s("ro.build.version.sdk", "19"),
                s("ro.build.version.release", "4.4.4"),
                s("ro.product.model", "bhpc"),
                s("ro.product.manufacturer", "bhpc"),
                s("ro.product.cpu.abi", "armeabi-v7a"),
                s("ro.product.cpu.abi2", "armeabi"),
            ],
            cpus: 4,
            pid: std::process::id(),
            uid: 10000,
            fs: crate::vfs::default_fs(),
            stack_guard: None,
        }
    }
}

/// State shared by all host functions.
pub struct State {
    pub mem: Arc<GuestMemory>,
    pub heap: GuestHeap,
    pub stdout: Box<dyn Write + Send>,
    pub stderr: Box<dyn Write + Send>,
    pub log: Option<LogSink>,
    pub clock: Box<dyn HostClock>,
    /// `(name, passed)` of every fixture `bhpc_check`.
    pub checks: Vec<(String, bool)>,
    pub exidx: Vec<ExidxRange>,
    /// Set by `exit` / `_exit`.
    pub exit_code: Option<i32>,
    /// `__cxa_atexit` registrations, in call order.
    pub atexit: Vec<AtExit>,
    pub props: BTreeMap<String, String>,
    pub cpus: u32,
    pub pid: u32,
    pub uid: u32,
    /// Call counters of the bound built-ins.
    pub histogram: Vec<(String, Arc<AtomicU64>)>,
    /// Names bound to a trap thunk because nothing implements them.
    pub unimplemented_bound: Vec<String>,
    /// Names of trap thunks that were actually called.
    pub unimplemented_hits: Vec<String>,
    /// The guest-thread scheduler (baton, blocking, deadlock detection).
    pub sched: Arc<Sched>,
    /// Virtual file system and descriptor table.
    pub vfs: crate::vfs::Vfs,
    /// Open `FILE`s other than the standard streams, by guest `FILE*`.
    pub(crate) files: HashMap<u32, FileRec>,
    /// Open `DIR`s: guest `DIR*` -> descriptor.
    pub(crate) dirs: HashMap<u32, i32>,
    /// Threads created by `pthread_create`, by `pthread_t`.
    pub(crate) threads: HashMap<u32, ThreadRec>,
    pub(crate) next_tid: u32,
    /// Thread-specific values: `(tid, key) -> value`.
    pub(crate) tls: HashMap<(u32, u32), u32>,
    /// Condition-variable wait queues (tickets) by `pthread_cond_t` address, and the tickets signalled.
    pub(crate) cond_q: HashMap<u32, VecDeque<u32>>,
    pub(crate) woken: HashSet<u32>,
    /// `__futex_wait` queues (tickets) by futex word address.
    pub(crate) futex_q: HashMap<u32, VecDeque<u32>>,
    pub(crate) next_wait: u32,
    pub(crate) data: HashMap<&'static str, u32>,
    pub(crate) errno: u32,
    pub(crate) main_thread: u32,
    pub(crate) tm_buf: u32,
    pub(crate) utc: u32,
    /// `inet_ntoa`'s static text buffer and the `h_errno` cell (allocated on first use).
    pub(crate) inet_buf: u32,
    pub(crate) h_errno: u32,
    /// Guest copies of zone abbreviations (`tm_zone`, `tzname`).
    /// `timezone` / `tzname` have been filled in (first `localtime` or `tzset`).
    pub(crate) tz_ready: bool,
    pub(crate) zone_names: HashMap<String, u32>,
    pub(crate) keys: HashMap<u32, (u32, u32)>,
    pub(crate) next_key: u32,
    pub(crate) mutexes: HashMap<u32, MutexState>,
    pub(crate) rwlocks: HashMap<u32, RwState>,
    pub(crate) once_return: u32,
    /// Wall-clock limit enforced at every guest time-slice boundary.
    pub(crate) wall_deadline: Option<std::time::Instant>,
    /// `pthread_once` controls whose initialiser is running: address -> tid of the running thread.
    pub(crate) once_owner: HashMap<u32, u32>,
    pub(crate) env: Vec<EnvVar>,
    pub(crate) environ_array: u32,
    pub(crate) strerror: HashMap<u32, u32>,
    pub(crate) rand48: u64,
    /// Anonymous `mmap` regions handed out from the guest heap: base -> length.
    pub(crate) mmaps: HashMap<u32, Mapping>,
}

pub type Shared = Arc<Mutex<State>>;

impl State {
    /// Number of threads created with `pthread_create` so far.
    pub fn next_tid_value(&self) -> u32 {
        self.next_tid
    }

    /// Create the state, allocating the guest-visible data symbols from `heap`.
    pub fn new(mem: Arc<GuestMemory>, mut heap: GuestHeap, cfg: Config) -> Result<Shared, MemError> {
        let mut data = HashMap::new();
        let sf = heap.calloc(1, 3 * layout::file::SIZE);
        data.insert("__sF", sf);
        for (i, (flags, fd)) in [
            (layout::file::SRD, 0u16),
            (layout::file::SWR | layout::file::SLBF, 1),
            (layout::file::SWR | layout::file::SNBF, 2),
        ]
        .into_iter()
        .enumerate()
        {
            let fp = sf + i as u32 * layout::file::SIZE;
            mem.write_u16(fp + layout::file::FLAGS, flags)?;
            mem.write_u16(fp + layout::file::FILE_NO, fd)?;
            mem.write_u32(fp + layout::file::COOKIE, fp)?;
            mem.write_u32(fp + layout::file::BLKSIZE, layout::PAGE_SIZE)?;
        }
        let guard = heap.calloc(1, 4);
        mem.write_u32(guard, cfg.stack_guard.unwrap_or_else(random_u32))?;
        data.insert("__stack_chk_guard", guard);

        let (ctype_t, lower_t, upper_t) = ctype::tables();
        let mut var = |name: &'static str, heap: &mut GuestHeap, v: u32| -> Result<u32, MemError> {
            let p = heap.calloc(1, 4);
            mem.write_u32(p, v)?;
            data.insert(name, p);
            Ok(p)
        };
        let blk = |heap: &mut GuestHeap, bytes: &[u8]| -> Result<u32, MemError> {
            let p = heap.calloc(1, bytes.len() as u32);
            mem.write_bytes(p, bytes)?;
            Ok(p)
        };
        let a = blk(&mut heap, &ctype_t)?;
        var("_ctype_", &mut heap, a)?;
        let lower: Vec<u8> = lower_t.iter().flat_map(|v| v.to_le_bytes()).collect();
        let upper: Vec<u8> = upper_t.iter().flat_map(|v| v.to_le_bytes()).collect();
        let a = blk(&mut heap, &lower)?;
        var("_tolower_tab_", &mut heap, a)?;
        let a = blk(&mut heap, &upper)?;
        var("_toupper_tab_", &mut heap, a)?;
        var("environ", &mut heap, 0)?;
        var("__page_size", &mut heap, layout::PAGE_SIZE)?;
        var("timezone", &mut heap, 0)?;
        let utc = blk(&mut heap, b"UTC\0")?;
        let tzname = heap.calloc(2, 4);
        mem.write_u32(tzname, utc)?;
        mem.write_u32(tzname + 4, utc)?;
        data.insert("tzname", tzname);

        let errno = heap.calloc(1, 4);
        let main_thread = heap.calloc(1, 256);
        let tm_buf = heap.calloc(1, layout::tm::SIZE);
        let st = State {
            mem,
            heap,
            stdout: cfg.stdout,
            stderr: cfg.stderr,
            log: cfg.log,
            clock: cfg.clock,
            checks: Vec::new(),
            exidx: Vec::new(),
            exit_code: None,
            atexit: Vec::new(),
            props: cfg.props.into_iter().collect(),
            cpus: cfg.cpus,
            pid: cfg.pid,
            uid: cfg.uid,
            histogram: Vec::new(),
            unimplemented_bound: Vec::new(),
            unimplemented_hits: Vec::new(),
            sched: Arc::new(Sched::new()),
            vfs: crate::vfs::Vfs::new(cfg.fs),
            files: HashMap::new(),
            dirs: HashMap::new(),
            threads: HashMap::new(),
            next_tid: 0,
            tls: HashMap::new(),
            cond_q: HashMap::new(),
            woken: HashSet::new(),
            futex_q: HashMap::new(),
            next_wait: 1,
            data,
            errno,
            main_thread,
            tm_buf,
            utc,
            inet_buf: 0,
            h_errno: 0,
            tz_ready: false,
            zone_names: HashMap::new(),
            keys: HashMap::new(),
            next_key: 1,
            mutexes: HashMap::new(),
            rwlocks: HashMap::new(),
            once_return: 0,
            wall_deadline: None,
            once_owner: HashMap::new(),
            env: Vec::new(),
            environ_array: 0,
            strerror: HashMap::new(),
            rand48: 0x1234_ABCD_330E,
            mmaps: HashMap::new(),
        };
        let shared = Arc::new(Mutex::new(st));
        {
            let mut s = shared.lock().expect("fresh mutex");
            for (k, v) in &cfg.env {
                crate::misc::env_set(&mut s, k.as_bytes(), v.as_bytes(), true);
            }
            crate::misc::env_sync(&mut s);
        }
        Ok(shared)
    }
}

/// Compatibility constructor: default configuration with `out` as the guest's stdout.
pub fn new_state(mem: Arc<GuestMemory>, heap: GuestHeap, out: Box<dyn Write + Send>) -> Result<Shared, MemError> {
    State::new(mem, heap, Config { stdout: out, ..Config::default() })
}

fn random_u32() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u32(std::process::id());
    h.finish() as u32
}

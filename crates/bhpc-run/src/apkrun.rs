//! `bhpc --apk <path> --init-only`: load every library of a user APK, bind bionic, relocate and run all
//! constructors; report what happened (thunk histogram, unimplemented imports, symbolised backtrace on a stop).

use crate::{new_env_with, RunError};
use bhpc_bionic::{histogram, lock, Hle, Shared};
use bhpc_cpu::{Cpu, Stop};
use bhpc_launcher::progress::{report, Phase};
use bhpc_loader::apk::{Apk, Release};
use bhpc_loader::init::{
    run_constructors, CallFailure, CtorFailure, CtorInfo, CtorKind, GuestCaller, InitOptions, InitTrace,
};
use bhpc_loader::Linker;
use bhpc_vfs::{DataDirs, Storage, VfsConfig};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

/// Library every APK is rooted at; all other libraries are reached through `DT_NEEDED`.
pub const ROOT_LIB: &str = "libApplication.so";
/// Frames kept in a backtrace.
pub const BACKTRACE_DEPTH: usize = 32;

/// Knobs of [`init_only`].
#[derive(Debug, Clone)]
pub struct InitConfig {
    pub apk: PathBuf,
    /// Host directory behind `/data/data/<package>` and `/sdcard` (see `bhpc_vfs::DataDirs`, created on demand);
    /// `None` uses the per-user default (`$XDG_DATA_HOME/blockheads-pc`, ...).
    pub data_dir: Option<PathBuf>,
    /// Android package name reported to the guest.
    pub package: String,
    /// Accept an APK whose `libApplication.so` hash is not in the known-builds table.
    pub force: bool,
    pub trace_thunks: bool,
    /// Log every guest path lookup to stderr.
    pub trace_fs: bool,
    /// Log every JNI call to stderr.
    pub trace_jni: bool,
    /// Also log every call into Java (needs `trace_jni`).
    pub trace_jni_calls: bool,
    /// Per-constructor guest cycle budget.
    pub watchdog_cycles: u64,
    /// Keep running constructors after a failed one.
    pub keep_going: bool,
    /// Log every constructor to stderr.
    pub verbose: bool,
    /// Single-step trace of every guest pc (very slow) to this file.
    pub pc_trace: Option<PathBuf>,
    /// Screen size reported to the game (`None` keeps the platform default).
    pub screen: Option<(i32, i32)>,
    /// Run guest threads in parallel (`bhpc_bionic::sched::enable_parallel`) instead of one at a time.
    pub parallel_guest: bool,
}

/// Whether guest threads run in parallel when nothing says otherwise (`--serial-guest` / `BHPC_PARALLEL_GUEST=0`
/// bring back the one-at-a-time scheduler).
pub const DEFAULT_PARALLEL_GUEST: bool = true;

/// The parallel-guest choice of a command line (`--serial-guest`, or `--parallel-guest`, which only undoes an earlier
/// `--serial-guest` now that parallel is the default; the later one wins) and the `BHPC_PARALLEL_GUEST` environment
/// variable (`1`/`on`/`true` or `0`/`off`/`false`), in that order of precedence.
pub fn parallel_guest(args: &[String], env: Option<&str>) -> bool {
    let flag = args.iter().rev().find_map(|a| match a.as_str() {
        "--parallel-guest" => Some(true),
        "--serial-guest" => Some(false),
        _ => None,
    });
    let env = env.and_then(|v| match v.trim().to_ascii_lowercase().as_str() {
        "1" | "on" | "true" | "yes" => Some(true),
        "0" | "off" | "false" | "no" => Some(false),
        _ => None,
    });
    flag.or(env).unwrap_or(DEFAULT_PARALLEL_GUEST)
}

impl InitConfig {
    pub fn new(apk: PathBuf) -> Self {
        InitConfig {
            apk,
            data_dir: None,
            package: bhpc_vfs::layout::DEFAULT_PACKAGE.to_string(),
            force: false,
            trace_thunks: false,
            trace_fs: false,
            trace_jni: false,
            trace_jni_calls: false,
            watchdog_cycles: 4_000_000_000,
            keep_going: false,
            verbose: false,
            pc_trace: None,
            screen: None,
            parallel_guest: DEFAULT_PARALLEL_GUEST,
        }
    }
}

/// A constructor failure with the guest backtrace at the stop.
#[derive(Debug, Clone)]
pub struct FailureReport {
    pub failure: CtorFailure,
    /// The `Stop` reason as printed by the CPU layer.
    pub stop: String,
    /// Symbolised frames, innermost first.
    pub backtrace: Vec<String>,
    /// r0..r15 at the stop.
    pub regs: Option<[u32; 16]>,
}

/// Summary of the guest's use of the fake JVM.
#[derive(Debug, Clone, Default)]
pub struct JniSummary {
    pub classes: usize,
    pub methods: usize,
    pub fields: usize,
    pub natives: usize,
    /// `Class.method` -> attempted calls into Java, most called first.
    pub java_calls: Vec<(String, u64)>,
    pub exception_pending: bool,
}

/// Result of [`init_only`].
#[derive(Debug, Clone)]
pub struct InitReport {
    pub release: Option<Release>,
    pub modules: usize,
    pub host_libs: Vec<String>,
    pub relocations: usize,
    pub thunks: usize,
    pub constructors_ran: usize,
    pub constructors_total: usize,
    pub per_module: Vec<(String, usize)>,
    pub failures: Vec<FailureReport>,
    /// Thunk call counts, most called first.
    pub histogram: Vec<(String, u64)>,
    /// Imports that were bound to a trap (nothing implements them).
    pub unimplemented_bound: Vec<String>,
    /// Trap thunks that were actually called.
    pub unimplemented_hits: Vec<String>,
    /// Threads the guest created / still running when init ended.
    pub threads_created: u32,
    pub threads_alive: u32,
    /// First failure of a non-main guest thread.
    pub thread_failure: Option<String>,
    /// What the guest did with the fake JVM.
    pub jni: JniSummary,
}

impl InitReport {
    /// True when every constructor returned and no unimplemented import was hit.
    pub fn ok(&self) -> bool {
        self.failures.is_empty()
            && self.thread_failure.is_none()
            && self.unimplemented_hits.is_empty()
            && self.constructors_ran == self.constructors_total
    }
}

/// `GuestCaller` over a real CPU: applies the cycle budget, optionally traces pcs, remembers the stop.
struct CpuCaller<'a> {
    cpu: &'a mut Cpu,
    pc_trace: Option<std::io::BufWriter<std::fs::File>>,
    last_stop: Option<Stop>,
}

impl GuestCaller for CpuCaller<'_> {
    fn call(&mut self, entry: u32, args: [u32; 3], max_cycles: u64) -> Result<u32, CallFailure> {
        if max_cycles > 0 {
            self.cpu.set_ticks(max_cycles);
        }
        let result = match self.pc_trace.as_mut() {
            Some(w) => self.cpu.call_traced(entry, &args, &mut |c: &Cpu| {
                let _ = writeln!(w, "{:08x}", c.pc());
            }),
            None => self.cpu.call(entry, &args),
        };
        match result {
            Ok(r) => Ok(r.r0),
            Err(Stop::Halted(_)) if max_cycles > 0 => Err(CallFailure::Watchdog { pc: self.stop_pc() }),
            Err(stop) => {
                let reason = stop.to_string();
                let pc = match &stop {
                    Stop::Undefined { pc, .. } | Stop::Unpredictable { pc, .. } | Stop::Exception { pc, .. } => *pc,
                    _ => self.stop_pc(),
                };
                self.last_stop = Some(stop);
                Err(CallFailure::Fault { pc, reason })
            }
        }
    }
}

impl CpuCaller<'_> {
    fn stop_pc(&self) -> u32 {
        self.cpu.stop_regs().map_or(self.cpu.pc(), |r| r[15])
    }
}

/// Counts finished constructors for the launcher's progress and forwards to another trace.
struct Counting<'a> {
    inner: &'a mut dyn InitTrace,
    done: u32,
    total: u32,
}

impl InitTrace for Counting<'_> {
    fn before(&mut self, c: &CtorInfo<'_>) {
        self.inner.before(c);
    }
    fn after(&mut self, c: &CtorInfo<'_>, r: &Result<u32, CallFailure>, t: Duration) {
        self.inner.after(c, r, t);
        self.done += 1;
        report(Phase::Constructors, self.done, self.total);
    }
}

/// Prints constructors as they run (`--verbose`).
struct VerboseTrace;

impl InitTrace for VerboseTrace {
    fn before(&mut self, c: &CtorInfo<'_>) {
        eprintln!("[init] {} {:?} @ {:#010x}", c.module, c.kind, c.entry);
    }
    fn after(&mut self, _c: &CtorInfo<'_>, r: &Result<u32, CallFailure>, t: Duration) {
        let outcome = match r {
            Ok(v) => format!("{v:#x}"),
            Err(e) => e.to_string(),
        };
        eprintln!("[init]   -> {outcome} in {:.1} ms", t.as_secs_f64() * 1e3);
    }
}

/// Name of the host function whose stub contains `addr` (the pc of a stopped thunk is inside its stub).
fn thunk_at(cpu: &Cpu, addr: u32) -> Option<String> {
    if addr < bhpc_cpu::THUNK_BASE {
        return None;
    }
    cpu.thunk_name(addr & !(bhpc_cpu::THUNK_STRIDE - 1))
}

/// `module+0xoff (symbol+0xoff)` for a guest address, or the thunk name for host functions.
pub fn symbolize(linker: &Linker, cpu: &Cpu, addr: u32) -> String {
    if let Some(n) = thunk_at(cpu, addr) {
        return format!("{addr:#010x} <host {n}>");
    }
    match linker.dladdr(addr) {
        None => format!("{addr:#010x}"),
        Some(i) => {
            let off = addr - i.base;
            match i.symbol {
                Some((name, at)) => format!("{addr:#010x} {}+{off:#x} ({name}+{:#x})", i.module, addr - (at & !1)),
                None => format!("{addr:#010x} {}+{off:#x}", i.module),
            }
        }
    }
}

/// What the Java-side loader runs right after a library's constructors, in order: `JNI_OnLoad(vm, NULL)` when
/// the library exports it (`System.load` does that), then `libv_init` for libv (`Library.initializeLibv`).
fn loader_hooks(m: &bhpc_loader::Module, vm: u32) -> Vec<(u32, [u32; 3])> {
    let mut v = Vec::new();
    if let Some(on_load) = m.export("JNI_OnLoad") {
        v.push((on_load, [vm, 0, 0]));
    }
    if m.name == "libv.so" {
        v.extend(m.export("libv_init").map(|e| (e, [0; 3])));
    }
    v
}

/// Host data directories: the configured root, else the per-user default, else a directory below the
/// system temp dir (a platform without a data directory).
pub fn data_dirs(configured: Option<&std::path::Path>, default: Option<DataDirs>) -> DataDirs {
    match (configured, default) {
        (Some(root), _) => DataDirs::new(root),
        (None, Some(dirs)) => dirs,
        (None, None) => DataDirs::new(std::env::temp_dir().join("blockheads-pc")),
    }
}

/// The bionic heap as the fake VM's allocator, so JNI buffers live in the guest's own heap.
pub struct BionicHeap(pub Shared);

impl bhpc_jni::vm::GuestAlloc for BionicHeap {
    fn alloc(&mut self, size: u32) -> u32 {
        // Zeroed: the fake VM relies on fresh blocks being clear (NUL terminators after lent strings).
        lock(&self.0).heap.calloc(1, size)
    }
    fn free(&mut self, ptr: u32) {
        lock(&self.0).heap.free(ptr);
    }
}

/// Hook run by [`Session::open_setup`] before linking.
pub type Setup<'a> = dyn FnMut(&mut Cpu, &Shared) -> Result<HostHooks, RunError> + 'a;

/// Window-system and audio back ends a setup hook can give the fake `VerdeActivity` / `AudioTrack`.
#[derive(Default)]
pub struct HostHooks {
    pub gfx: Option<std::sync::Arc<dyn bhpc_jni::GfxHooks>>,
    pub audio: Option<std::sync::Arc<dyn bhpc_jni::AudioHooks>>,
}

/// A loaded APK: guest memory, CPU, bionic state and the linked modules.
pub struct Session {
    pub cpu: Box<Cpu>,
    pub st: Shared,
    pub linker: Linker,
    /// The fake JavaVM/JNIEnv and what the guest did with it.
    pub jni: bhpc_jni::Jni,
    pub release: Option<Release>,
    pub host_libs: Vec<String>,
    /// What the Java side reports (paths, locale, ...).
    pub platform: bhpc_jni::Platform,
    /// Index range of this APK's modules in `linker.modules()`.
    pub range: std::ops::Range<usize>,
}

impl Session {
    /// Open the APK, map and relocate every library, bind bionic. Runs no guest code.
    pub fn open(cfg: &InitConfig, out: Box<dyn Write + Send>) -> Result<Session, RunError> {
        Self::open_with(cfg, out, None)
    }

    /// Like [`Session::open`], with a receiver for the guest's Android log (`NSLog`, `__android_log_*`).
    pub fn open_with(
        cfg: &InitConfig,
        out: Box<dyn Write + Send>,
        log: Option<bhpc_bionic::state::LogSink>,
    ) -> Result<Session, RunError> {
        Self::open_setup(cfg, out, log, &mut |_, _| Ok(HostHooks::default()))
    }

    /// Like [`Session::open_with`]; `setup` runs after the CPU exists and before the libraries are linked, so it
    /// can register thunks (GL, OpenSL ES, `dlopen`) that the imports then bind to. It may return the graphics
    /// hooks for the fake `VerdeActivity`.
    pub fn open_setup(
        cfg: &InitConfig,
        out: Box<dyn Write + Send>,
        log: Option<bhpc_bionic::state::LogSink>,
        setup: &mut Setup<'_>,
    ) -> Result<Session, RunError> {
        report(Phase::Apk, 0, 1);
        let mut apk = Apk::open(&cfg.apk)?;
        let release = apk.verify(cfg.force)?;
        report(Phase::Apk, 1, 1);
        let dirs = data_dirs(cfg.data_dir.as_deref(), DataDirs::platform_default());
        dirs.create_all()?;
        let mut vcfg = VfsConfig::new(&cfg.apk, dirs);
        vcfg.package = cfg.package.clone();
        let storage = Storage::open(&vcfg)?;
        let mut bcfg = bhpc_bionic::Config { stdout: out, log, ..bhpc_bionic::Config::default() };
        bcfg.env.extend(storage.guest.env());
        bcfg.fs = storage.vfs.clone();
        let (mut cpu, st) = new_env_with(bcfg, Some(cfg.watchdog_cycles))?;
        cpu.set_trace_thunks(cfg.trace_thunks);
        // Spinning guest threads must not starve the ones they wait for (threads inherit the policy).
        bhpc_bionic::sched::enable_time_slice(&mut cpu, &st, bhpc_bionic::sched::DEFAULT_SLICE_CYCLES);
        if cfg.parallel_guest {
            bhpc_bionic::sched::enable_parallel(&cpu, &st);
        }
        lock(&st).vfs.trace = cfg.trace_fs;
        let hooks = setup(&mut cpu, &st)?;
        let mut linker = Linker::new(cpu.mem().clone());
        report(Phase::Libraries, 0, 0);
        let summary = {
            let mut hle = Hle::new(&mut cpu, st.clone());
            linker.load_all(&[ROOT_LIB], &mut apk, &mut hle)?
        };
        register_exidx(&linker, &st);
        let env = std::env::var_os(crate::fastdiv::DISABLE_ENV);
        let mods = linker.modules();
        crate::fastdiv::install_all(cpu.mem(), env.as_deref(), mods.len(), &|i, name| mods[i].export(name))?;
        crate::mktimer::install_all(cpu.mem(), mods.iter().map(|m| (m.name.as_str(), &m.segments[..])))?;
        cpu.clear_cache();
        report(Phase::Libraries, 1, 1);
        let mut platform = bhpc_jni::Platform::for_guest(&storage.guest);
        platform.lan = crate::vfd::host_has_lan();
        if let Some((w, h)) = cfg.screen {
            (platform.screen_width, platform.screen_height) = (w, h);
        }
        let mem = cpu.mem().clone();
        let mut jcfg = bhpc_jni::VmConfig::new(platform.clone(), Box::new(storage.assets));
        if let Some(g) = hooks.gfx {
            jcfg.gfx = g;
        }
        if let Some(a) = hooks.audio {
            jcfg.audio = a;
        }
        let jni = bhpc_jni::install(&cpu, mem, Box::new(BionicHeap(st.clone())), jcfg)?;
        jni.set_trace(cfg.trace_jni);
        jni.set_trace_calls(cfg.trace_jni_calls);
        // `Java_*` exports bind lazily to the fake classes (natives not announced through RegisterNatives).
        let natives: std::collections::HashMap<String, u32> = linker.modules()[summary.range.clone()]
            .iter()
            .flat_map(|m| m.exports.iter().filter(|(n, _)| n.starts_with("Java_")).map(|(n, a)| (n.clone(), *a)))
            .collect();
        jni.vm.set_symbol_resolver(move |n| natives.get(n).copied());
        Ok(Session { cpu, st, linker, jni, release, host_libs: summary.host_libs, platform, range: summary.range })
    }

    /// Run every constructor in dependency order.
    pub fn run_constructors(
        &mut self,
        cfg: &InitConfig,
    ) -> Result<(bhpc_loader::init::InitReport, Vec<FailureReport>), RunError> {
        let pc_trace = match &cfg.pc_trace {
            Some(p) => Some(std::io::BufWriter::new(std::fs::File::create(p)?)),
            None => None,
        };
        let vm = self.jni.java_vm;
        let modules = &self.linker.modules()[self.range.clone()];
        let mut caller = CpuCaller { cpu: &mut self.cpu, pc_trace, last_stop: None };
        let opts =
            InitOptions { watchdog_cycles: cfg.watchdog_cycles, keep_going: cfg.keep_going, ..InitOptions::default() };
        let mut rep = bhpc_loader::init::InitReport::default();
        let mut verbose = VerboseTrace;
        let mut quiet = bhpc_loader::init::NoTrace;
        let mut counting = Counting {
            inner: if cfg.verbose { &mut verbose } else { &mut quiet },
            done: 0,
            total: modules.iter().map(|m| m.constructors().len() as u32).sum(),
        };
        report(Phase::Constructors, 0, counting.total);
        for m in modules {
            let one = std::slice::from_ref(m);
            let r = run_constructors(one, &mut caller, &mut counting, &opts);
            rep.ran += r.ran;
            rep.per_module.extend(r.per_module);
            let ok = r.failures.is_empty();
            rep.failures.extend(r.failures);
            if !ok && !cfg.keep_going {
                break;
            }
            // The Java loader initialises libv right after loading it, before any other library runs.
            let hooks = if ok { loader_hooks(m, vm) } else { Vec::new() };
            let mut hook_failed = false;
            let pid = caller.cpu.processor_id();
            for (entry, args) in hooks {
                self.jni.enter(pid);
                let r = caller.call(entry, args, opts.watchdog_cycles);
                self.jni.leave(pid);
                if let Err(failure) = r {
                    let f = CtorFailure { module: m.name.clone(), kind: CtorKind::Init, entry, failure };
                    rep.failures.push(f);
                    hook_failed = true;
                    break;
                }
            }
            if hook_failed && !cfg.keep_going {
                break;
            }
        }
        let stop = caller.last_stop.take();
        let failures = rep
            .failures
            .iter()
            .map(|f| {
                let backtrace =
                    self.cpu.stop_regs().map(|r| backtrace_lines(&self.linker, &self.cpu, r)).unwrap_or_default();
                let stop = stop.as_ref().map_or_else(|| f.failure.to_string(), ToString::to_string);
                FailureReport { failure: f.clone(), stop, backtrace, regs: self.cpu.stop_regs() }
            })
            .collect();
        Ok((rep, failures))
    }

    /// Stop all other guest threads for good.
    pub fn shutdown(&self) {
        let sched = lock(&self.st).sched.clone();
        sched.shutdown();
    }
}

/// Load the APK and run all constructors. `out` receives the guest's stdout.
pub fn init_only(cfg: &InitConfig, out: Box<dyn Write + Send>) -> Result<InitReport, RunError> {
    let mut sess = Session::open(cfg, out)?;
    let (rep, failures) = sess.run_constructors(cfg)?;
    let modules = &sess.linker.modules()[sess.range.clone()];
    let constructors_total = modules.iter().map(|m| m.constructors().len()).sum();
    let relocations = modules.iter().map(|m| m.relocs_applied).sum();
    let histogram = histogram_of(&sess.st);
    let s = lock(&sess.st);
    let report = InitReport {
        release: sess.release,
        modules: sess.range.len(),
        host_libs: sess.host_libs.clone(),
        relocations,
        thunks: sess.cpu.thunk_count(),
        constructors_ran: rep.ran,
        constructors_total,
        per_module: rep.per_module,
        failures,
        histogram,
        unimplemented_bound: s.unimplemented_bound.clone(),
        unimplemented_hits: s.unimplemented_hits.clone(),
        threads_created: s.next_tid_value(),
        threads_alive: s.sched.live().saturating_sub(1),
        thread_failure: s.sched.failure(),
        jni: {
            let u = sess.jni.usage();
            JniSummary {
                classes: u.classes,
                methods: u.methods,
                fields: u.fields,
                natives: u.natives,
                java_calls: u.java_calls,
                exception_pending: u.exception_pending,
            }
        },
    };
    drop(s);
    sess.shutdown();
    Ok(report)
}

/// Symbolised backtrace for the registers at a stop. A stop inside a host thunk is listed as the thunk,
/// followed by the unwound guest caller (starting at `lr`).
pub fn backtrace_lines(linker: &Linker, cpu: &Cpu, mut regs: [u32; 16]) -> Vec<String> {
    let mut lines = Vec::new();
    if thunk_at(cpu, regs[15]).is_some() {
        lines.push(symbolize(linker, cpu, regs[15]));
        regs[15] = regs[14];
        // The return address already points at the call; unwinding from here must not step back again.
    }
    // A stop outside any module (a jump into data or unmapped memory) has no unwind info: show it, then
    // continue from the caller, whose address is in `lr`.
    if linker.module_at(regs[15]).is_none() && thunk_at(cpu, regs[15]).is_none() {
        lines.push(symbolize(linker, cpu, regs[15]));
        regs[15] = regs[14];
    }
    lines.extend(linker.backtrace(regs, BACKTRACE_DEPTH).iter().map(|f| symbolize(linker, cpu, f.pc)));
    lines
}

fn histogram_of(st: &Shared) -> Vec<(String, u64)> {
    histogram(st)
}

/// Tell bionic where each module's unwind table is (`__gnu_Unwind_Find_exidx`).
fn register_exidx(linker: &Linker, st: &Shared) {
    let mut s = lock(st);
    for m in linker.modules() {
        if let Some((table, count)) = m.exidx {
            s.exidx.push(bhpc_bionic::ExidxRange { start: m.start, size: m.size, table, count });
        }
    }
}

/// Human-readable report (what `--init-only` prints).
pub fn print_report(r: &InitReport, top: usize, w: &mut dyn Write) -> std::io::Result<()> {
    writeln!(
        w,
        "build {}; {} modules ({} host libs: {}); {} relocations; {} thunks bound",
        r.release.map_or("unknown", Release::version),
        r.modules,
        r.host_libs.len(),
        r.host_libs.join(" "),
        r.relocations,
        r.thunks
    )?;
    writeln!(w, "constructors: {}/{} returned", r.constructors_ran, r.constructors_total)?;
    for (m, n) in &r.per_module {
        writeln!(w, "  {m:<28} {n}")?;
    }
    for f in &r.failures {
        writeln!(w, "FAILED {}", f.failure)?;
        writeln!(w, "  stop: {}", f.stop)?;
        if let Some(r) = f.regs {
            let line: Vec<String> = r.iter().enumerate().map(|(i, v)| format!("r{i}={v:08x}")).collect();
            writeln!(w, "  regs: {}", line.join(" "))?;
        }
        for (i, fr) in f.backtrace.iter().enumerate() {
            writeln!(w, "  #{i:<2} {fr}")?;
        }
    }
    writeln!(w, "thunk calls (top {top} of {}):", r.histogram.len())?;
    for (n, c) in r.histogram.iter().filter(|(_, c)| *c > 0).take(top) {
        writeln!(w, "  {c:>10}  {n}")?;
    }
    writeln!(w, "guest threads: {} created, {} still alive", r.threads_created, r.threads_alive)?;
    if let Some(f) = &r.thread_failure {
        writeln!(w, "  a guest thread failed: {f}")?;
    }
    writeln!(
        w,
        "jni: {} classes, {} methods, {} fields, {} natives registered, {} distinct Java methods called{}",
        r.jni.classes,
        r.jni.methods,
        r.jni.fields,
        r.jni.natives,
        r.jni.java_calls.len(),
        if r.jni.exception_pending { " (exception pending)" } else { "" }
    )?;
    for (m, n) in r.jni.java_calls.iter().take(top.min(10)) {
        writeln!(w, "  {n:>6}  {m}")?;
    }
    writeln!(
        w,
        "unimplemented imports: {} bound to traps, {} hit during init",
        r.unimplemented_bound.len(),
        r.unimplemented_hits.len()
    )?;
    for n in &r.unimplemented_hits {
        writeln!(w, "  HIT {n}")?;
    }
    if top > 0 && !r.unimplemented_hits.is_empty() {
        writeln!(w, "unimplemented imports still bound to traps ({}):", r.unimplemented_bound.len())?;
        for chunk in r.unimplemented_bound.chunks(8) {
            writeln!(w, "  {}", chunk.join(" "))?;
        }
    }
    Ok(())
}

//! Native client mods on PC: the `libbhmod` host for the emulated guest.
//!
//! A native mod is an armv7 `.so` (`client/native/armeabi-v7a/*.so`). It runs in the guest like the game: the host
//! loads it with the same linker as the game's libraries (imports bind to the host's bionic), then calls its
//! `bhmod_native_init(api)` on the calling guest thread. The `api` table lives in guest memory and its functions
//! are host thunks; they drive the shared [`Engine`] of `bhmod-native` through a [`Host`] over guest memory:
//!
//! - patching code = write the guest page (made writable for the moment), then invalidate the JIT's translation;
//! - trampolines = an executable arena mapped in the guest;
//! - modules, symbols and imports come from a snapshot of the linker's modules (the game's and the mods');
//! - Objective-C swizzles call the runtime's own `class_getInstanceMethod` / `method_setImplementation` in the guest,
//!   the same exports an in-process Android loader uses.
//!
//! So a mod installs the same hooks, by the same names, on PC and on Android. Hook handlers are the mod's own guest
//! functions: the game calls them directly, no host code runs in between.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use bhmod_native::abi::{self, Code};
use bhmod_native::engine::{Engine, HookError, Hooked, Host, ModuleInfo};
use bhpc_bionic::{Hle, Shared};
use bhpc_cpu::Cpu;
use bhpc_loader::{Linker, Module, PF_R, PF_W, PF_X};
use bhpc_mem::Prot;

use crate::objchook::Runtime;

/// The engine owner of the runner's own hooks (mods count from 0).
const INTERNAL: u32 = u32::MAX - 1;
/// Size of one executable arena for trampolines.
const ARENA: u32 = 64 << 10;
/// Page size of the guest.
const PAGE: u32 = 4096;

/// What the hook engine needs to know about a loaded module.
#[derive(Debug, Clone)]
pub struct ModSnap {
    pub name: String,
    pub base: u32,
    pub dynamic: u32,
    pub exports: HashMap<String, u32>,
    pub segments: Vec<bhpc_loader::Segment>,
}

impl ModSnap {
    pub fn of(m: &Module) -> ModSnap {
        ModSnap {
            name: m.name.clone(),
            base: m.bias,
            dynamic: m.dynamic,
            exports: m.exports.clone(),
            segments: m.segments.clone(),
        }
    }
}

fn prot_of_bits(bits: u32) -> Prot {
    let mut p = Prot::NONE;
    for (flag, bit) in [(PF_R, Prot::READ), (PF_W, Prot::WRITE), (PF_X, Prot::EXEC)] {
        if bits & flag != 0 {
            p |= bit;
        }
    }
    p
}

/// A mod's state in the host.
struct NativeMod {
    id: String,
    /// The table in guest memory (0 until the first library loads).
    api: u32,
    /// Loaded libraries by path: `bhmod_native_init` and `bhmod_native_shutdown` (0 when missing).
    libs: HashMap<PathBuf, (u32, u32)>,
    /// The libraries loaded for the current session, in order.
    active: Vec<PathBuf>,
}

struct Inner {
    rt: Runtime,
    st: Shared,
    engine: Mutex<Engine>,
    snap: RwLock<Vec<ModSnap>>,
    /// (next free address, end) of the trampoline arena.
    arena: Mutex<(u32, u32)>,
    mods: Mutex<Vec<NativeMod>>,
    /// The last failure message (`last_error`).
    error: Mutex<String>,
    /// Guest addresses of the table's functions, in [`abi::FUNCS`] order.
    funcs: Mutex<Vec<u32>>,
    logs: Mutex<Vec<(i32, String)>>,
}

static GLOBAL: std::sync::OnceLock<Natives> = std::sync::OnceLock::new();

/// Makes `n` the host the mod glue uses (first call wins).
pub fn set(n: Natives) {
    let _ = GLOBAL.set(n);
}

/// The host the mod glue uses, if native mods are on.
pub fn get() -> Option<&'static Natives> {
    GLOBAL.get()
}

/// The `libbhmod` host of the runner.
#[derive(Clone)]
pub struct Natives {
    inner: Arc<Inner>,
}

/// The engine's view of the guest while one thunk or load runs.
struct GuestHost<'a> {
    cpu: &'a mut Cpu,
    inner: &'a Inner,
}

impl GuestHost<'_> {
    fn prot_at(&self, addr: u32) -> Prot {
        let snap = self.inner.snap.read().unwrap_or_else(PoisonError::into_inner);
        snap.iter()
            .flat_map(|m| m.segments.iter())
            .find(|s| addr >= s.start && addr - s.start < s.len)
            .map_or(Prot::READ | Prot::WRITE, |s| prot_of_bits(s.prot))
    }
}

impl Host for GuestHost<'_> {
    fn read(&self, addr: u32, len: usize) -> Option<Vec<u8>> {
        self.cpu.mem().read_bytes(addr, len as u32).ok()
    }

    fn write_code(&mut self, addr: u32, bytes: &[u8]) -> Result<(), String> {
        self.write(addr, bytes)
    }

    fn write(&mut self, addr: u32, bytes: &[u8]) -> Result<(), String> {
        let mem = self.cpu.mem().clone();
        let start = addr & !(PAGE - 1);
        let end = (addr + bytes.len() as u32).next_multiple_of(PAGE);
        let original = self.prot_at(addr);
        // Writable and executable for the moment; the guest page keeps its protection afterwards.
        let r = mem
            .protect(start, end - start, Prot::READ | Prot::WRITE | Prot::EXEC)
            .and_then(|()| mem.write_bytes(addr, bytes));
        let _ = mem.protect(start, end - start, original);
        r.map_err(|e| e.to_string())?;
        self.cpu.invalidate(addr, bytes.len() as u32);
        Ok(())
    }

    fn alloc_exec(&mut self, len: usize) -> Result<u32, String> {
        let len = (len as u32).next_multiple_of(4);
        let mut arena = self.inner.arena.lock().unwrap_or_else(PoisonError::into_inner);
        if arena.0 + len > arena.1 {
            let mem = self.cpu.mem();
            let at = mem.map_anywhere(ARENA, PAGE).map_err(|e| e.to_string())?;
            let _ = mem.protect(at, ARENA, Prot::READ | Prot::WRITE | Prot::EXEC);
            *arena = (at, at + ARENA);
        }
        let at = arena.0;
        arena.0 += len;
        Ok(at)
    }

    fn modules(&self) -> Vec<ModuleInfo> {
        let snap = self.inner.snap.read().unwrap_or_else(PoisonError::into_inner);
        snap.iter().map(|m| ModuleInfo { name: m.name.clone(), base: m.base, dynamic: m.dynamic }).collect()
    }

    fn symbol(&self, module: Option<&str>, name: &str) -> Option<u32> {
        let snap = self.inner.snap.read().unwrap_or_else(PoisonError::into_inner);
        snap.iter()
            .filter(|m| module.is_none_or(|w| m.name == w || m.name.rsplit('/').next() == Some(w)))
            .find_map(|m| m.exports.get(name).copied())
    }

    fn objc_method(&mut self, class: &str, selector: &str, meta: bool) -> Option<u32> {
        let rt = self.inner.rt;
        let cls = rt.class(self.cpu, class, meta).ok().filter(|c| *c != 0)?;
        let sel = rt.sel(self.cpu, selector).ok()?;
        let m = self.cpu.call(rt.get_instance_method, &[cls, sel]).ok()?.r0;
        (m != 0).then_some(m)
    }

    fn objc_set_imp(&mut self, method: u32, imp: u32) -> Option<u32> {
        let old = self.cpu.call(self.inner.rt.set_imp, &[method, imp]).ok()?.r0;
        (old != 0).then_some(old)
    }
}

impl Natives {
    /// A host over the game's ObjC runtime `rt`; `modules` are the loaded modules (the game's libraries).
    pub fn new(rt: Runtime, st: Shared, modules: &[Module]) -> Natives {
        Natives {
            inner: Arc::new(Inner {
                rt,
                st,
                engine: Mutex::new(Engine::new()),
                snap: RwLock::new(modules.iter().map(ModSnap::of).collect()),
                arena: Mutex::new((0, 0)),
                mods: Mutex::new(Vec::new()),
                error: Mutex::new(String::new()),
                funcs: Mutex::new(Vec::new()),
                logs: Mutex::new(Vec::new()),
            }),
        }
    }

    /// A [`Host`] over `cpu`, for tests of the engine's view of the guest.
    #[cfg(test)]
    pub(crate) fn host<'a>(&'a self, cpu: &'a mut Cpu) -> impl Host + 'a {
        GuestHost { cpu, inner: &self.inner }
    }

    /// Lines mods logged since the last call: `(level, "[mod] text")`.
    pub fn take_logs(&self) -> Vec<(i32, String)> {
        std::mem::take(&mut *self.inner.logs.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Number of hooks mods hold.
    pub fn hooks(&self) -> usize {
        self.inner.engine.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    /// Registers the table's functions as host thunks (once per CPU environment). A function that cannot be
    /// registered stays 0 and `load` refuses to run mods.
    pub fn register(&self, cpu: &Cpu) {
        let mut addrs = Vec::new();
        for (name, f) in [
            ("log", thunk_log as ThunkFn),
            ("hook_objc", thunk_hook_objc),
            ("hook_symbol", thunk_hook_symbol),
            ("hook_address", thunk_hook_address),
            ("hook_import", thunk_hook_import),
            ("unhook", thunk_unhook),
            ("find_symbol", thunk_find_symbol),
            ("module_base", thunk_module_base),
            ("last_error", thunk_last_error),
        ] {
            let inner = self.inner.clone();
            let guarded = move |c: &mut Cpu| {
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&inner, c)));
                c.ret_u32(r.unwrap_or(Code::Memory.ret() as u32));
            };
            addrs.push(cpu.register_shared_thunk(&format!("bhmod:{name}"), Arc::new(guarded)).unwrap_or(0));
        }
        *self.inner.funcs.lock().unwrap_or_else(PoisonError::into_inner) = addrs;
    }

    /// Adds one module to the snapshot (the fake game of the tests).
    pub fn add_snap(&self, snap: ModSnap) {
        self.inner.snap.write().unwrap_or_else(PoisonError::into_inner).push(snap);
    }

    /// Inline-hooks the exported function `symbol` for the runner itself (not for a mod): `replacement` is a guest
    /// address (a host thunk) and the result is the trampoline that runs the original. Undone by
    /// [`Natives::unhook_internal`].
    pub fn hook_internal(&self, cpu: &mut Cpu, symbol: &str, replacement: u32) -> Result<u32, String> {
        self.hook_internal_as(cpu, INTERNAL, symbol, replacement)
    }

    /// Inline-hooks the ARM-mode function `offset` bytes into `module` for the runner itself, as `owner` (an id of
    /// the runner's own, so that one feature can undo its hooks without touching another's, see
    /// [`Natives::unhook_owner`]); `replacement` and the result are as in [`Natives::hook_internal`].
    pub fn hook_internal_at(
        &self,
        cpu: &mut Cpu,
        owner: u32,
        module: &str,
        offset: u32,
        replacement: u32,
    ) -> Result<u32, String> {
        let mut engine = self.inner.engine.lock().unwrap_or_else(PoisonError::into_inner);
        let mut host = GuestHost { cpu, inner: &self.inner };
        engine.hook_address(&mut host, owner, module, offset, replacement).map(|h| h.original).map_err(|e| e.message)
    }

    /// [`Natives::hook_internal`] as another runner-owned `owner`.
    pub fn hook_internal_as(&self, cpu: &mut Cpu, owner: u32, symbol: &str, replacement: u32) -> Result<u32, String> {
        let mut engine = self.inner.engine.lock().unwrap_or_else(PoisonError::into_inner);
        let mut host = GuestHost { cpu, inner: &self.inner };
        engine.hook_symbol(&mut host, owner, None, symbol, replacement).map(|h| h.original).map_err(|e| e.message)
    }

    /// Undoes the hooks the runner-owned `owner` made.
    pub fn unhook_owner(&self, cpu: &mut Cpu, owner: u32) {
        let mut engine = self.inner.engine.lock().unwrap_or_else(PoisonError::into_inner);
        let mut host = GuestHost { cpu, inner: &self.inner };
        engine.unhook_owner(&mut host, owner);
    }

    /// Undoes the hooks of [`Natives::hook_internal`].
    pub fn unhook_internal(&self, cpu: &mut Cpu) {
        self.unhook_owner(cpu, INTERNAL);
    }

    /// The address a module's import `symbol` currently resolves to (what the module calls), read from its GOT.
    pub fn import_target(&self, cpu: &mut Cpu, module: &str, symbol: &str) -> Option<u32> {
        let host = GuestHost { cpu, inner: &self.inner };
        let m = host.modules().into_iter().find(|m| m.name == module || m.name.rsplit('/').next() == Some(module))?;
        let imports = bhmod_native::elf::imports(&|a, n| host.read(a, n), m.base, m.dynamic).ok()?;
        let slot = imports.into_iter().find(|i| i.name == symbol)?.slot;
        host.read(slot, 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// `len` bytes of guest memory that stay valid (never freed), for out-parameters of calls into the game.
    pub fn scratch(&self, cpu: &mut Cpu, len: usize) -> Result<u32, String> {
        GuestHost { cpu, inner: &self.inner }.alloc_exec(len)
    }

    /// The base address of a module in the snapshot.
    pub fn module_base(&self, module: &str) -> Option<u32> {
        let snap = self.inner.snap.read().unwrap_or_else(PoisonError::into_inner);
        snap.iter().find(|m| m.name == module || m.name.rsplit('/').next() == Some(module)).map(|m| m.base)
    }

    /// An exported symbol of a module in the snapshot.
    pub fn symbol(&self, module: &str, name: &str) -> Option<u32> {
        let snap = self.inner.snap.read().unwrap_or_else(PoisonError::into_inner);
        snap.iter().filter(|m| m.name.ends_with(module)).find_map(|m| m.exports.get(name).copied())
    }

    /// Adds modules loaded later (a mod's own library is added by [`Natives::load`]).
    pub fn add_modules(&self, modules: &[Module]) {
        self.inner.snap.write().unwrap_or_else(PoisonError::into_inner).extend(modules.iter().map(ModSnap::of));
    }

    /// The table of `id` in guest memory (made on first use).
    fn table(&self, cpu: &Cpu, id: &str, data_dir: &Path) -> Result<u32, String> {
        let mut mods = self.inner.mods.lock().unwrap_or_else(PoisonError::into_inner);
        if !mods.iter().any(|m| m.id == id) {
            mods.push(NativeMod { id: id.to_string(), api: 0, libs: HashMap::new(), active: Vec::new() });
        }
        let m = mods.iter_mut().find(|m| m.id == id).expect("just pushed");
        if m.api != 0 {
            return Ok(m.api);
        }
        let funcs = self.inner.funcs.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if funcs.len() != abi::FUNCS.len() || funcs.contains(&0) {
            return Err("the libbhmod functions are not registered".into());
        }
        let mem = cpu.mem();
        let page = mem.map_anywhere(PAGE, PAGE).map_err(|e| e.to_string())?;
        // The page was just mapped: these writes cannot fail.
        let (id_at, dir_at) = (page + abi::TABLE_SIZE, page + abi::TABLE_SIZE + 256);
        let _ = mem.write_cstr(id_at, &id.chars().take(200).collect::<String>());
        let _ = mem.write_cstr(dir_at, &data_dir.to_string_lossy().chars().take(1000).collect::<String>());
        let mut words = vec![abi::ABI_VERSION, abi::TABLE_SIZE, id_at, dir_at];
        words.extend(funcs);
        for (i, w) in words.iter().enumerate() {
            let _ = mem.write_u32(page + 4 * i as u32, *w);
        }
        m.api = page;
        Ok(page)
    }

    /// Loads `lib` into the guest and runs its `bhmod_native_init`. `cpu` is the calling guest thread's CPU.
    pub fn load(&self, cpu: &mut Cpu, id: &str, lib: &Path, data_dir: &Path) -> Result<(), String> {
        let api = self.table(cpu, id, data_dir)?;
        let known = {
            let mods = self.inner.mods.lock().unwrap_or_else(PoisonError::into_inner);
            mods.iter().find(|m| m.id == id).and_then(|m| m.libs.get(lib).copied())
        };
        let (init, shutdown) = match known {
            Some(f) => f,
            None => self.link(cpu, id, lib)?,
        };
        {
            let mut mods = self.inner.mods.lock().unwrap_or_else(PoisonError::into_inner);
            let m = mods.iter_mut().find(|m| m.id == id).expect("table made it");
            m.libs.insert(lib.to_path_buf(), (init, shutdown));
            m.active.push(lib.to_path_buf());
        }
        let r = cpu.call(init, &[api]).map_err(|s| format!("bhmod_native_init stopped: {s}"))?.r0 as i32;
        if r != 0 {
            self.unload(cpu, id);
            return Err(format!("bhmod_native_init returned {r}"));
        }
        Ok(())
    }

    /// Links a library into the guest: its imports bind to the host's bionic, its constructors run.
    fn link(&self, cpu: &mut Cpu, id: &str, lib: &Path) -> Result<(u32, u32), String> {
        let bytes = std::fs::read(lib).map_err(|e| format!("{}: {e}", lib.display()))?;
        let name = format!("{id}/{}", lib.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned()));
        let mut linker = Linker::new(cpu.mem().clone());
        {
            let mut hle = Hle::new(cpu, self.inner.st.clone());
            linker.load(&name, &bytes, &mut hle).map_err(|e| e.to_string())?;
        }
        // Protections only tighten the pages; a constructor that stops the guest also fails `init`, which reports it.
        let _ = linker.apply_protections();
        let module = linker.modules()[0].clone();
        for ctor in module.constructors() {
            let _ = cpu.call(ctor, &[]);
        }
        self.add_modules(std::slice::from_ref(&module));
        let init = module.export(abi::INIT_SYMBOL).ok_or_else(|| format!("the library has no {}", abi::INIT_SYMBOL))?;
        Ok((init, module.export(abi::SHUTDOWN_SYMBOL).unwrap_or(0)))
    }

    /// Runs the mod's `bhmod_native_shutdown` functions and undoes its hooks. The libraries stay in the guest.
    pub fn unload(&self, cpu: &mut Cpu, id: &str) {
        let (owner, shutdowns) = {
            let mut mods = self.inner.mods.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(i) = mods.iter().position(|m| m.id == id) else { return };
            let m = &mut mods[i];
            let fns: Vec<u32> =
                m.active.drain(..).filter_map(|p| m.libs.get(&p)).map(|f| f.1).filter(|f| *f != 0).collect();
            (i as u32, fns)
        };
        for f in shutdowns {
            let _ = cpu.call(f, &[]);
        }
        let mut engine = self.inner.engine.lock().unwrap_or_else(PoisonError::into_inner);
        let mut host = GuestHost { cpu, inner: &self.inner };
        engine.unhook_owner(&mut host, owner);
    }
}

// ---- thunks ---------------------------------------------------------------------------------------------------

type ThunkFn = fn(&Inner, &mut Cpu) -> u32;

fn owner_of(inner: &Inner, api: u32) -> u32 {
    let mods = inner.mods.lock().unwrap_or_else(PoisonError::into_inner);
    mods.iter().position(|m| m.api == api).map_or(u32::MAX, |i| i as u32)
}

fn text(c: &Cpu, ptr: u32) -> Option<String> {
    (ptr != 0).then(|| c.mem().read_cstr(ptr).ok()).flatten()
}

fn set_error(inner: &Inner, msg: &str) {
    *inner.error.lock().unwrap_or_else(PoisonError::into_inner) = msg.to_string();
}

fn finish(inner: &Inner, c: &mut Cpu, r: Result<Hooked, HookError>, original: u32) -> u32 {
    match r {
        Ok(h) => {
            if original != 0 {
                let _ = c.mem().write_u32(original, h.original);
            }
            h.handle
        }
        Err(e) => {
            set_error(inner, &e.message);
            e.code.ret() as u32
        }
    }
}

fn bad_arg(inner: &Inner, what: &str) -> u32 {
    set_error(inner, &format!("{what}: a required argument is null"));
    Code::Arg.ret() as u32
}

fn thunk_log(inner: &Inner, c: &mut Cpu) -> u32 {
    let mut a = c.args();
    let (api, level, msg) = (a.u32(c), a.u32(c) as i32, a.u32(c));
    let Some(msg) = text(c, msg) else { return 0 };
    let id =
        inner.mods.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|m| m.api == api).map(|m| m.id.clone());
    inner
        .logs
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((level, format!("[{}] {msg}", id.unwrap_or_else(|| "native".into()))));
    0
}

fn thunk_hook_objc(inner: &Inner, c: &mut Cpu) -> u32 {
    let mut a = c.args();
    let (api, cls, sel, meta, repl, orig) = (a.u32(c), a.u32(c), a.u32(c), a.u32(c), a.u32(c), a.u32(c));
    let (Some(cls), Some(sel)) = (text(c, cls), text(c, sel)) else { return bad_arg(inner, "hook_objc") };
    let owner = owner_of(inner, api);
    let r = {
        let mut engine = inner.engine.lock().unwrap_or_else(PoisonError::into_inner);
        let mut host = GuestHost { cpu: c, inner };
        engine.hook_objc(&mut host, owner, &cls, &sel, meta != 0, repl)
    };
    finish(inner, c, r, orig)
}

fn thunk_hook_symbol(inner: &Inner, c: &mut Cpu) -> u32 {
    let mut a = c.args();
    let (api, module, symbol, repl, orig) = (a.u32(c), a.u32(c), a.u32(c), a.u32(c), a.u32(c));
    let (module, Some(symbol)) = (text(c, module), text(c, symbol)) else { return bad_arg(inner, "hook_symbol") };
    let owner = owner_of(inner, api);
    let r = {
        let mut engine = inner.engine.lock().unwrap_or_else(PoisonError::into_inner);
        let mut host = GuestHost { cpu: c, inner };
        engine.hook_symbol(&mut host, owner, module.as_deref(), &symbol, repl)
    };
    finish(inner, c, r, orig)
}

fn thunk_hook_address(inner: &Inner, c: &mut Cpu) -> u32 {
    let mut a = c.args();
    let (api, module, offset, repl, orig) = (a.u32(c), a.u32(c), a.u32(c), a.u32(c), a.u32(c));
    let Some(module) = text(c, module) else { return bad_arg(inner, "hook_address") };
    let owner = owner_of(inner, api);
    let r = {
        let mut engine = inner.engine.lock().unwrap_or_else(PoisonError::into_inner);
        let mut host = GuestHost { cpu: c, inner };
        engine.hook_address(&mut host, owner, &module, offset, repl)
    };
    finish(inner, c, r, orig)
}

fn thunk_hook_import(inner: &Inner, c: &mut Cpu) -> u32 {
    let mut a = c.args();
    let (api, module, symbol, repl, orig) = (a.u32(c), a.u32(c), a.u32(c), a.u32(c), a.u32(c));
    let (module, Some(symbol)) = (text(c, module), text(c, symbol)) else { return bad_arg(inner, "hook_import") };
    let owner = owner_of(inner, api);
    let r = {
        let mut engine = inner.engine.lock().unwrap_or_else(PoisonError::into_inner);
        let mut host = GuestHost { cpu: c, inner };
        engine.hook_import(&mut host, owner, module.as_deref(), &symbol, repl)
    };
    finish(inner, c, r, orig)
}

fn thunk_unhook(inner: &Inner, c: &mut Cpu) -> u32 {
    let mut a = c.args();
    let (_api, handle) = (a.u32(c), a.u32(c));
    let mut engine = inner.engine.lock().unwrap_or_else(PoisonError::into_inner);
    let mut host = GuestHost { cpu: c, inner };
    match engine.unhook(&mut host, handle) {
        Ok(()) => 0,
        Err(e) => {
            set_error(inner, &e.message);
            e.code.ret() as u32
        }
    }
}

fn thunk_find_symbol(inner: &Inner, c: &mut Cpu) -> u32 {
    let mut a = c.args();
    let (_api, module, symbol) = (a.u32(c), a.u32(c), a.u32(c));
    let Some(symbol) = text(c, symbol) else { return 0 };
    let module = text(c, module);
    let host = GuestHost { cpu: c, inner };
    host.symbol(module.as_deref(), &symbol).unwrap_or(0)
}

fn thunk_module_base(inner: &Inner, c: &mut Cpu) -> u32 {
    let mut a = c.args();
    let (_api, module) = (a.u32(c), a.u32(c));
    let Some(module) = text(c, module) else { return 0 };
    let host = GuestHost { cpu: c, inner };
    host.modules()
        .into_iter()
        .find(|m| m.name == module || m.name.rsplit('/').next() == Some(&module))
        .map_or(0, |m| m.base)
}

/// `last_error`: the message in a guest buffer behind the table (one per process, overwritten by the next call).
fn thunk_last_error(inner: &Inner, c: &mut Cpu) -> u32 {
    let mut a = c.args();
    let api = a.u32(c);
    let msg = inner.error.lock().unwrap_or_else(PoisonError::into_inner).clone();
    // The buffer is the second half of the mod's table page.
    let at = api + 2048;
    let _ = c.mem().write_cstr(at, &msg.chars().take(500).collect::<String>());
    at
}

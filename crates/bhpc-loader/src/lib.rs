//! Minimal ELF32/ARM shared-object loader.
//!
//! Maps the `PT_LOAD` segments of one `.so` into the guest window, applies the dynamic relocations
//! the Android toolchains emit (`RELATIVE`, `ABS32`, `GLOB_DAT`, `JUMP_SLOT`) and binds imports to
//! symbols of already loaded modules or to host-provided addresses (thunks).

pub mod apk;
pub mod dl;
mod elf;
pub mod init;
/// Tiny ELF builder for tests (also used by `bhpc-run` tests via the `test-elf` feature).
#[cfg(any(test, feature = "test-elf"))]
#[allow(missing_docs)]
pub mod testelf;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_apk;
#[cfg(test)]
mod tests_dl;
#[cfg(test)]
mod tests_init;
#[cfg(test)]
mod tests_multi;
#[cfg(test)]
mod tests_unwind;
pub mod unwind;

use bhpc_mem::{GuestMemory, MemError, Prot};
use elf::{Dynamic, Elf, Sym};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

pub use elf::{R_ARM_ABS32, R_ARM_GLOB_DAT, R_ARM_JUMP_SLOT, R_ARM_NONE, R_ARM_RELATIVE};

/// Loader failures.
#[derive(Debug, PartialEq, Eq)]
pub enum LoadError {
    /// Not an ELF file, or a header field is out of range.
    BadElf(&'static str),
    /// Valid ELF but a feature we do not implement.
    Unsupported(String),
    /// The file is shorter than its headers claim.
    Truncated,
    /// A segment or table lies outside the loaded image.
    BadAddress(u32),
    /// An imported symbol could not be bound.
    Unresolved(String),
    /// A requested library is not provided by the library source.
    NotFound(String),
    /// The library source failed (for example a corrupt APK entry).
    Source(String),
    /// Guest memory error.
    Mem(MemError),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::BadElf(m) => write!(f, "bad ELF: {m}"),
            LoadError::Unsupported(m) => write!(f, "unsupported: {m}"),
            LoadError::Truncated => write!(f, "truncated ELF file"),
            LoadError::BadAddress(a) => write!(f, "address {a:#x} is outside the image"),
            LoadError::Unresolved(s) => write!(f, "unresolved symbol {s}"),
            LoadError::NotFound(n) => write!(f, "library {n} not found"),
            LoadError::Source(m) => write!(f, "library source: {m}"),
            LoadError::Mem(e) => write!(f, "guest memory: {e}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<MemError> for LoadError {
    fn from(e: MemError) -> Self {
        LoadError::Mem(e)
    }
}

/// Supplies addresses (normally thunks) for symbols no loaded module defines.
pub trait HostSymbols {
    /// Guest address for `name`, or `None` if the host does not provide it.
    fn resolve(&mut self, name: &str) -> Option<u32>;

    /// Guest address of a counted trap thunk for the unresolvable import `name`.
    ///
    /// Called only when the linker runs in trap mode ([`Linker::set_trap_unresolved`]). The thunk should
    /// record `name` when the guest calls it. `None` (the default) makes the import a hard error.
    fn trap(&mut self, _name: &str) -> Option<u32> {
        None
    }
}

impl<F: FnMut(&str) -> Option<u32>> HostSymbols for F {
    fn resolve(&mut self, name: &str) -> Option<u32> {
        self(name)
    }
}

/// Supplies library bytes by `DT_NEEDED` name (an APK, a directory, a test map ...).
pub trait LibSource {
    /// Bytes of `name` (e.g. `libFoo.so`), or `Ok(None)` if the library is not bundled (a system library
    /// the host provides).
    fn fetch(&mut self, name: &str) -> Result<Option<Vec<u8>>, LoadError>;
}

impl<F: FnMut(&str) -> Option<Vec<u8>>> LibSource for F {
    fn fetch(&mut self, name: &str) -> Result<Option<Vec<u8>>, LoadError> {
        Ok(self(name))
    }
}

/// `PF_X` of a program header.
pub const PF_X: u32 = 1;
/// `PF_W` of a program header.
pub const PF_W: u32 = 2;
/// `PF_R` of a program header.
pub const PF_R: u32 = 4;

/// One `PT_LOAD` segment, rounded out to pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    /// Guest address of the first page.
    pub start: u32,
    /// Length in bytes (a multiple of the page size).
    pub len: u32,
    /// `PF_R | PF_W | PF_X` bits.
    pub prot: u32,
}

/// A defined dynamic symbol, kept sorted by address for [`Linker::dladdr`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymInfo {
    /// Guest address (bit 0 set for Thumb functions).
    pub addr: u32,
    /// `st_size` (0 if unknown).
    pub size: u32,
    /// Symbol name.
    pub name: String,
}

/// A loaded shared object.
#[derive(Debug, Clone)]
pub struct Module {
    /// Name given at load time.
    pub name: String,
    /// `DT_SONAME`, if present.
    pub soname: Option<String>,
    /// Load bias: guest address of ELF virtual address 0.
    pub bias: u32,
    /// Guest address of the first mapped byte.
    pub start: u32,
    /// Bytes mapped.
    pub size: u32,
    /// `DT_NEEDED` entries (the caller loads/binds them).
    pub needed: Vec<String>,
    /// Defined dynamic symbols (guest address; bit 0 set for Thumb functions).
    pub exports: HashMap<String, u32>,
    /// `.ARM.exidx` guest address and entry count (`PT_ARM_EXIDX`).
    pub exidx: Option<(u32, u32)>,
    /// `DT_INIT` function address.
    pub init: Option<u32>,
    /// `DT_INIT_ARRAY` function addresses.
    pub init_array: Vec<u32>,
    /// `DT_FINI_ARRAY` function addresses.
    pub fini_array: Vec<u32>,
    /// Number of relocations applied.
    pub relocs_applied: usize,
    /// `DT_SYMBOLIC` / `DF_SYMBOLIC`: defined symbols bind to this object first.
    pub symbolic: bool,
    /// `PT_LOAD` segments with their protections.
    pub segments: Vec<Segment>,
    /// Defined symbols sorted by address.
    pub symbols: Vec<SymInfo>,
}

impl Module {
    /// Look up a defined symbol.
    pub fn export(&self, name: &str) -> Option<u32> {
        self.exports.get(name).copied()
    }

    /// True if `addr` lies inside this module's image.
    pub fn contains(&self, addr: u32) -> bool {
        addr >= self.start && addr - self.start < self.size
    }

    /// Constructors in execution order: `DT_INIT` then `DT_INIT_ARRAY`.
    pub fn constructors(&self) -> Vec<u32> {
        self.init.iter().chain(self.init_array.iter()).copied().collect()
    }
}

/// Result of [`Linker::load_all`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadSummary {
    /// Indices (into [`Linker::modules`]) of the newly loaded modules, in dependency order.
    pub range: std::ops::Range<usize>,
    /// `DT_NEEDED` names that no [`LibSource`] provided; the host is expected to supply them.
    pub host_libs: Vec<String>,
}

/// A mapped but not yet relocated module.
struct Staged {
    module: Module,
    dynamic: Dynamic,
    syms: Vec<Sym>,
}

/// Outcome of relocating one module.
struct Relocated {
    count: usize,
    /// One entry per relocation that was bound to a trap thunk.
    traps: Vec<String>,
}

/// Holds loaded modules and links new ones against them.
pub struct Linker {
    mem: Arc<GuestMemory>,
    modules: Vec<Module>,
    trap_unresolved: bool,
    unresolved: BTreeMap<String, u32>,
    host_libs: BTreeSet<String>,
    dl_error: Option<String>,
}

impl Linker {
    /// New empty linker over `mem`.
    pub fn new(mem: Arc<GuestMemory>) -> Self {
        Linker {
            mem,
            modules: Vec::new(),
            trap_unresolved: false,
            unresolved: BTreeMap::new(),
            host_libs: BTreeSet::new(),
            dl_error: None,
        }
    }

    /// Guest memory the modules live in.
    pub fn mem(&self) -> &Arc<GuestMemory> {
        &self.mem
    }

    /// Modules in load order.
    pub fn modules(&self) -> &[Module] {
        &self.modules
    }

    /// When enabled, imports nobody provides are bound to [`HostSymbols::trap`] thunks and counted in
    /// [`Linker::unresolved`] instead of failing the load.
    pub fn set_trap_unresolved(&mut self, on: bool) {
        self.trap_unresolved = on;
    }

    /// Imports bound to trap thunks so far: symbol name to number of relocations that reference it.
    pub fn unresolved(&self) -> &BTreeMap<String, u32> {
        &self.unresolved
    }

    /// Find a module by load name or soname.
    pub fn module(&self, name: &str) -> Option<&Module> {
        self.module_index(name).map(|i| &self.modules[i])
    }

    /// Index of the module with load name or soname `name`.
    pub fn module_index(&self, name: &str) -> Option<usize> {
        self.modules.iter().position(|m| m.name == name || m.soname.as_deref() == Some(name))
    }

    /// The module whose image contains `addr`.
    pub fn module_at(&self, addr: u32) -> Option<&Module> {
        self.modules.iter().find(|m| m.contains(addr))
    }

    /// Search all loaded modules (in load order) for a defined symbol.
    pub fn lookup(&self, sym: &str) -> Option<u32> {
        self.modules.iter().find_map(|m| m.export(sym))
    }

    /// `__gnu_Unwind_Find_exidx`: the exidx table (guest address, entry count) of the module containing `pc`.
    pub fn find_exidx(&self, pc: u32) -> Option<(u32, u32)> {
        self.module_at(pc).and_then(|m| m.exidx)
    }

    /// Apply the `PT_LOAD` protections of every module ([`GuestMemory::protect`]).
    ///
    /// Mappings are read/write while loading and relocating; call this once relocations and constructors
    /// are done (code pages become read/execute, so later guest writes to code fault). Pages shared by
    /// several segments get the union of their protections.
    pub fn apply_protections(&self) -> Result<(), LoadError> {
        let mem = self.mem.clone();
        self.apply_protections_with(&mut |start, len, prot| mem.protect(start, len, prot))
    }

    /// [`Linker::apply_protections`] with the protect call injected.
    fn apply_protections_with(
        &self,
        protect: &mut dyn FnMut(u32, u32, Prot) -> Result<(), MemError>,
    ) -> Result<(), LoadError> {
        for m in &self.modules {
            for (start, len, prot) in coalesce_pages(&m.segments) {
                let mut p = Prot::NONE;
                for (flag, bit) in [(PF_R, Prot::READ), (PF_W, Prot::WRITE), (PF_X, Prot::EXEC)] {
                    if prot & flag != 0 {
                        p |= bit;
                    }
                }
                protect(start, len, p).map_err(LoadError::Mem)?;
            }
        }
        Ok(())
    }

    /// Load `bytes` as module `name`, relocate it and register it.
    ///
    /// Imports bind to modules loaded earlier (and to itself), so dependencies must be loaded first; use
    /// [`Linker::load_all`] to load a whole dependency graph.
    pub fn load(&mut self, name: &str, bytes: &[u8], host: &mut dyn HostSymbols) -> Result<&Module, LoadError> {
        let mut st = self.stage(name, bytes)?;
        let r = self.relocate(&st.module, &st.dynamic, &st.syms, host)?;
        finish(&self.mem, &mut st.module, &st.dynamic, &r)?;
        self.merge_traps(r.traps);
        self.modules.push(st.module);
        Ok(self.modules.last().expect("just pushed"))
    }

    /// Load `roots` and everything they `DT_NEEDED` that `src` provides, in dependency order.
    ///
    /// All modules are mapped first and relocated afterwards, so imports resolve against the complete set in
    /// load order (dependencies before dependents). Names `src` does not provide are reported in
    /// [`LoadSummary::host_libs`]. On error no module is registered.
    pub fn load_all(
        &mut self,
        roots: &[&str],
        src: &mut dyn LibSource,
        host: &mut dyn HostSymbols,
    ) -> Result<LoadSummary, LoadError> {
        let mut staged: Vec<Staged> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut order: Vec<usize> = Vec::new();
        let mut host_libs: Vec<String> = Vec::new();
        for root in roots {
            self.visit(root, true, src, &mut staged, &mut seen, &mut order, &mut host_libs)?;
        }

        let first = self.modules.len();
        let mut slots: Vec<Option<Staged>> = staged.into_iter().map(Some).collect();
        let mut pending = Vec::new();
        for i in order {
            let st = slots[i].take().expect("each module is ordered once");
            pending.push((st.dynamic, st.syms));
            self.modules.push(st.module);
        }
        let mut results = Vec::new();
        for (k, (dynamic, syms)) in pending.iter().enumerate() {
            match self.relocate(&self.modules[first + k], dynamic, syms, host) {
                Ok(r) => results.push(r),
                Err(e) => {
                    self.modules.truncate(first);
                    return Err(e);
                }
            }
        }
        for (k, ((dynamic, _), r)) in pending.into_iter().zip(results).enumerate() {
            if let Err(e) = finish(&self.mem, &mut self.modules[first + k], &dynamic, &r) {
                self.modules.truncate(first);
                return Err(e);
            }
            self.merge_traps(r.traps);
        }
        for h in &host_libs {
            self.host_libs.insert(h.clone());
        }
        Ok(LoadSummary { range: first..self.modules.len(), host_libs })
    }

    #[allow(clippy::too_many_arguments)]
    fn visit(
        &self,
        name: &str,
        required: bool,
        src: &mut dyn LibSource,
        staged: &mut Vec<Staged>,
        seen: &mut HashSet<String>,
        order: &mut Vec<usize>,
        host_libs: &mut Vec<String>,
    ) -> Result<(), LoadError> {
        if seen.contains(name) || self.module(name).is_some() {
            return Ok(());
        }
        seen.insert(name.to_string());
        let Some(bytes) = src.fetch(name)? else {
            if required {
                return Err(LoadError::NotFound(name.to_string()));
            }
            if !host_libs.iter().any(|h| h == name) {
                host_libs.push(name.to_string());
            }
            return Ok(());
        };
        let st = self.stage(name, &bytes)?;
        let needed = st.module.needed.clone();
        let idx = staged.len();
        staged.push(st);
        for n in &needed {
            self.visit(n, false, src, staged, seen, order, host_libs)?;
        }
        order.push(idx);
        Ok(())
    }

    /// Map `bytes` and read its dynamic metadata; no relocations yet.
    fn stage(&self, name: &str, bytes: &[u8]) -> Result<Staged, LoadError> {
        let elf = Elf::parse(bytes)?;
        let (min_v, max_v) = elf.load_span()?;
        let size = max_v - min_v;
        let start = self.mem.map_anywhere(size, 0x1_0000)?;
        let bias = start.wrapping_sub(min_v);
        for ph in elf.loads() {
            self.mem.write_bytes(bias.wrapping_add(ph.vaddr), elf.segment_bytes(ph)?)?;
        }

        let dynamic = match elf.dynamic_vaddr() {
            Some(v) => Dynamic::read(&self.mem, bias, v)?,
            None => Dynamic::default(),
        };
        let syms = dynamic.read_symbols(&self.mem, bias)?;

        let mut module = Module {
            name: name.to_string(),
            soname: dynamic.soname.clone(),
            bias,
            start,
            size,
            needed: dynamic.needed.clone(),
            exports: HashMap::new(),
            exidx: elf.exidx().map(|(v, memsz)| (bias.wrapping_add(v), memsz / 8)),
            init: dynamic.init.map(|v| bias.wrapping_add(v)),
            init_array: Vec::new(),
            fini_array: Vec::new(),
            relocs_applied: 0,
            symbolic: dynamic.symbolic,
            segments: elf.loads().map(|p| segment(bias, p)).collect(),
            symbols: Vec::new(),
        };
        for s in syms.iter().filter(|s| s.is_defined()) {
            let addr = bias.wrapping_add(s.value);
            module.exports.insert(s.name.clone(), addr);
            module.symbols.push(SymInfo { addr, size: s.size, name: s.name.clone() });
        }
        module.symbols.sort_by(|a, b| (a.addr & !1, &a.name).cmp(&(b.addr & !1, &b.name)));
        Ok(Staged { module, dynamic, syms })
    }

    fn merge_traps(&mut self, traps: Vec<String>) {
        for t in traps {
            *self.unresolved.entry(t).or_insert(0) += 1;
        }
    }

    /// Bind one symbol reference. Returns the address and whether it is a trap thunk.
    fn resolve(&self, sym: &Sym, module: &Module, host: &mut dyn HostSymbols) -> Result<(u32, bool), LoadError> {
        if sym.is_defined() {
            let local = module.bias.wrapping_add(sym.value);
            if module.symbolic {
                return Ok((local, false));
            }
            return Ok((self.lookup(&sym.name).unwrap_or(local), false));
        }
        if let Some(a) = self.lookup(&sym.name) {
            return Ok((a, false));
        }
        if let Some(a) = host.resolve(&sym.name) {
            return Ok((a, false));
        }
        if sym.is_weak() {
            return Ok((0, false));
        }
        if self.trap_unresolved {
            if let Some(a) = host.trap(&sym.name) {
                return Ok((a, true));
            }
        }
        Err(LoadError::Unresolved(sym.name.clone()))
    }

    fn relocate(
        &self,
        module: &Module,
        dynamic: &Dynamic,
        syms: &[Sym],
        host: &mut dyn HostSymbols,
    ) -> Result<Relocated, LoadError> {
        let mut out = Relocated { count: 0, traps: Vec::new() };
        // One symbol is usually referenced by several relocations; resolve it once.
        let mut cache: HashMap<u32, (u32, bool)> = HashMap::new();
        for (table, size) in [(dynamic.rel, dynamic.relsz), (dynamic.jmprel, dynamic.pltrelsz)] {
            let Some(table) = table else { continue };
            for i in 0..size / 8 {
                let ent = module.bias.wrapping_add(table).wrapping_add(i * 8);
                let r_offset = self.mem.read_u32(ent)?;
                let r_info = self.mem.read_u32(ent + 4)?;
                let place = module.bias.wrapping_add(r_offset);
                let sym_index = r_info >> 8;
                match r_info & 0xff {
                    elf::R_ARM_NONE => continue,
                    elf::R_ARM_RELATIVE => {
                        let a = self.mem.read_u32(place)?;
                        self.mem.write_u32(place, a.wrapping_add(module.bias))?;
                    }
                    elf::R_ARM_ABS32 => {
                        let s = self.sym_value(sym_index, syms, module, host, &mut cache, &mut out)?;
                        let a = self.mem.read_u32(place)?;
                        self.mem.write_u32(place, a.wrapping_add(s))?;
                    }
                    elf::R_ARM_GLOB_DAT | elf::R_ARM_JUMP_SLOT => {
                        let s = self.sym_value(sym_index, syms, module, host, &mut cache, &mut out)?;
                        self.mem.write_u32(place, s)?;
                    }
                    other => return Err(LoadError::Unsupported(format!("relocation type {other}"))),
                }
                out.count += 1;
            }
        }
        Ok(out)
    }

    fn sym_value(
        &self,
        index: u32,
        syms: &[Sym],
        module: &Module,
        host: &mut dyn HostSymbols,
        cache: &mut HashMap<u32, (u32, bool)>,
        out: &mut Relocated,
    ) -> Result<u32, LoadError> {
        let sym = syms.get(index as usize).ok_or(LoadError::BadAddress(index))?;
        let (v, trapped) = match cache.get(&index) {
            Some(c) => *c,
            None => {
                let c = self.resolve(sym, module, host)?;
                cache.insert(index, c);
                c
            }
        };
        if trapped {
            out.traps.push(sym.name.clone());
        }
        Ok(v)
    }
}

/// Fill in what is only known after relocation.
fn finish(mem: &GuestMemory, module: &mut Module, d: &Dynamic, r: &Relocated) -> Result<(), LoadError> {
    module.relocs_applied = r.count;
    // Constructor/destructor tables hold already-relocated function pointers.
    module.init_array = read_fn_array(mem, module.bias, d.init_array, d.init_arraysz)?;
    module.fini_array = read_fn_array(mem, module.bias, d.fini_array, d.fini_arraysz)?;
    Ok(())
}

fn segment(bias: u32, p: &elf::Phdr) -> Segment {
    let start = bias.wrapping_add(p.vaddr) & !0xfff;
    let end = (bias.wrapping_add(p.vaddr) as u64 + p.memsz as u64 + 0xfff) & !0xfff;
    Segment { start, len: (end - start as u64) as u32, prot: p.flags & (PF_R | PF_W | PF_X) }
}

/// Merge segments into maximal page runs with equal protection (shared pages get the union).
fn coalesce_pages(segs: &[Segment]) -> Vec<(u32, u32, u32)> {
    let mut pages: BTreeMap<u64, u32> = BTreeMap::new();
    for s in segs {
        for p in 0..(s.len as u64 / 4096) {
            *pages.entry(s.start as u64 / 4096 + p).or_insert(0) |= s.prot;
        }
    }
    let mut runs: Vec<(u32, u32, u32)> = Vec::new();
    for (page, prot) in pages {
        if let Some(last) = runs.last_mut() {
            if last.2 == prot && last.0 as u64 + last.1 as u64 == page * 4096 {
                last.1 += 4096;
                continue;
            }
        }
        runs.push(((page * 4096) as u32, 4096, prot));
    }
    runs
}

fn read_fn_array(mem: &GuestMemory, bias: u32, addr: Option<u32>, size: u32) -> Result<Vec<u32>, LoadError> {
    let Some(addr) = addr else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for i in 0..size / 4 {
        let f = mem.read_u32(bias.wrapping_add(addr).wrapping_add(i * 4))?;
        // 0 and -1 are conventional "no entry" markers.
        if f != 0 && f != u32::MAX {
            out.push(f);
        }
    }
    Ok(out)
}

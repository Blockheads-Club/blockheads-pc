//! Minimal ELF32/ARM shared-object loader.
//!
//! Maps the `PT_LOAD` segments of one `.so` into the guest window, applies the dynamic relocations
//! the Android toolchains emit (`RELATIVE`, `ABS32`, `GLOB_DAT`, `JUMP_SLOT`) and binds imports to
//! symbols of already loaded modules or to host-provided addresses (thunks).

mod elf;
/// Tiny ELF builder for tests (also used by `bhpc-run` tests via the `test-elf` feature).
#[cfg(any(test, feature = "test-elf"))]
#[allow(missing_docs)]
pub mod testelf;
#[cfg(test)]
mod tests;

use bhpc_mem::{GuestMemory, MemError};
use elf::{Dynamic, Elf, Sym};
use std::collections::HashMap;
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
}

impl<F: FnMut(&str) -> Option<u32>> HostSymbols for F {
    fn resolve(&mut self, name: &str) -> Option<u32> {
        self(name)
    }
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

/// Holds loaded modules and links new ones against them.
pub struct Linker {
    mem: Arc<GuestMemory>,
    modules: Vec<Module>,
}

impl Linker {
    /// New empty linker over `mem`.
    pub fn new(mem: Arc<GuestMemory>) -> Self {
        Linker { mem, modules: Vec::new() }
    }

    /// Modules in load order.
    pub fn modules(&self) -> &[Module] {
        &self.modules
    }

    /// Find a module by load name or soname.
    pub fn module(&self, name: &str) -> Option<&Module> {
        self.modules.iter().find(|m| m.name == name || m.soname.as_deref() == Some(name))
    }

    /// Search all loaded modules (in load order) for a defined symbol.
    pub fn lookup(&self, sym: &str) -> Option<u32> {
        self.modules.iter().find_map(|m| m.export(sym))
    }

    /// `__gnu_Unwind_Find_exidx`: the exidx table of the module containing `pc`.
    pub fn find_exidx(&self, pc: u32) -> Option<(u32, u32)> {
        self.modules.iter().find(|m| m.contains(pc)).and_then(|m| m.exidx)
    }

    /// Load `bytes` as module `name`, relocate it and register it.
    pub fn load(&mut self, name: &str, bytes: &[u8], host: &mut dyn HostSymbols) -> Result<&Module, LoadError> {
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
        };
        for s in syms.iter().filter(|s| s.is_defined()) {
            module.exports.insert(s.name.clone(), bias.wrapping_add(s.value));
        }

        module.relocs_applied = self.relocate(&module, &dynamic, &syms, host)?;

        // Constructor/destructor tables hold already-relocated function pointers.
        module.init_array = read_fn_array(&self.mem, bias, dynamic.init_array, dynamic.init_arraysz)?;
        module.fini_array = read_fn_array(&self.mem, bias, dynamic.fini_array, dynamic.fini_arraysz)?;

        self.modules.push(module);
        Ok(self.modules.last().expect("just pushed"))
    }

    fn resolve(&self, sym: &Sym, module: &Module, host: &mut dyn HostSymbols) -> Result<u32, LoadError> {
        if sym.is_defined() {
            return Ok(module.bias.wrapping_add(sym.value));
        }
        if let Some(a) = self.lookup(&sym.name) {
            return Ok(a);
        }
        if let Some(a) = host.resolve(&sym.name) {
            return Ok(a);
        }
        if sym.is_weak() {
            return Ok(0);
        }
        Err(LoadError::Unresolved(sym.name.clone()))
    }

    fn relocate(
        &self,
        module: &Module,
        dynamic: &Dynamic,
        syms: &[Sym],
        host: &mut dyn HostSymbols,
    ) -> Result<usize, LoadError> {
        let mut count = 0;
        // One symbol is usually referenced by several relocations; resolve it once.
        let mut cache: HashMap<u32, u32> = HashMap::new();
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
                        let s = self.sym_value(sym_index, syms, module, host, &mut cache)?;
                        let a = self.mem.read_u32(place)?;
                        self.mem.write_u32(place, a.wrapping_add(s))?;
                    }
                    elf::R_ARM_GLOB_DAT | elf::R_ARM_JUMP_SLOT => {
                        let s = self.sym_value(sym_index, syms, module, host, &mut cache)?;
                        self.mem.write_u32(place, s)?;
                    }
                    other => return Err(LoadError::Unsupported(format!("relocation type {other}"))),
                }
                count += 1;
            }
        }
        Ok(count)
    }

    fn sym_value(
        &self,
        index: u32,
        syms: &[Sym],
        module: &Module,
        host: &mut dyn HostSymbols,
        cache: &mut HashMap<u32, u32>,
    ) -> Result<u32, LoadError> {
        if let Some(v) = cache.get(&index) {
            return Ok(*v);
        }
        let sym = syms.get(index as usize).ok_or(LoadError::BadAddress(index))?;
        let v = self.resolve(sym, module, host)?;
        cache.insert(index, v);
        Ok(v)
    }
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

//! ELF32 little-endian ARM parsing (file headers from bytes; dynamic tables from mapped guest memory).

use crate::LoadError;
use bhpc_mem::GuestMemory;

pub const R_ARM_NONE: u32 = 0;
pub const R_ARM_ABS32: u32 = 2;
pub const R_ARM_GLOB_DAT: u32 = 21;
pub const R_ARM_JUMP_SLOT: u32 = 22;
pub const R_ARM_RELATIVE: u32 = 23;

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_ARM_EXIDX: u32 = 0x7000_0001;
const ET_DYN: u16 = 3;
const EM_ARM: u16 = 40;

const DT_NULL: u32 = 0;
const DT_NEEDED: u32 = 1;
const DT_PLTRELSZ: u32 = 2;
const DT_HASH: u32 = 4;
const DT_STRTAB: u32 = 5;
const DT_SYMTAB: u32 = 6;
const DT_RELA: u32 = 7;
const DT_SYMENT: u32 = 11;
const DT_INIT: u32 = 12;
const DT_SONAME: u32 = 14;
const DT_SYMBOLIC: u32 = 16;
const DT_REL: u32 = 17;
const DT_RELSZ: u32 = 18;
const DT_JMPREL: u32 = 23;
const DT_INIT_ARRAY: u32 = 25;
const DT_FINI_ARRAY: u32 = 26;
const DT_INIT_ARRAYSZ: u32 = 27;
const DT_FINI_ARRAYSZ: u32 = 28;
const DT_FLAGS: u32 = 30;
const DF_SYMBOLIC: u32 = 2;
const DT_GNU_HASH: u32 = 0x6fff_fef5;

const MAX_DYNAMIC_ENTRIES: u32 = 4096;

fn u16le(b: &[u8], o: usize) -> Result<u16, LoadError> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or(LoadError::Truncated)
}

fn u32le(b: &[u8], o: usize) -> Result<u32, LoadError> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or(LoadError::Truncated)
}

/// A program header.
#[derive(Debug, Clone, Copy)]
pub struct Phdr {
    pub kind: u32,
    pub offset: u32,
    pub vaddr: u32,
    pub filesz: u32,
    pub memsz: u32,
    pub flags: u32,
}

/// Parsed ELF file header and program headers.
pub struct Elf<'a> {
    data: &'a [u8],
    phdrs: Vec<Phdr>,
}

impl<'a> Elf<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, LoadError> {
        if data.len() < 52 || &data[..4] != b"\x7fELF" {
            return Err(LoadError::BadElf("missing ELF magic"));
        }
        if data[4] != 1 {
            return Err(LoadError::BadElf("not ELFCLASS32"));
        }
        if data[5] != 1 {
            return Err(LoadError::BadElf("not little endian"));
        }
        if u16le(data, 16)? != ET_DYN {
            return Err(LoadError::BadElf("not a shared object (ET_DYN)"));
        }
        if u16le(data, 18)? != EM_ARM {
            return Err(LoadError::BadElf("not EM_ARM"));
        }
        let phoff = u32le(data, 28)? as usize;
        let phentsize = u16le(data, 42)? as usize;
        let phnum = u16le(data, 44)? as usize;
        if phentsize != 32 {
            return Err(LoadError::BadElf("unexpected program header size"));
        }
        let mut phdrs = Vec::with_capacity(phnum);
        for i in 0..phnum {
            let o = phoff + i * 32;
            phdrs.push(Phdr {
                kind: u32le(data, o)?,
                offset: u32le(data, o + 4)?,
                vaddr: u32le(data, o + 8)?,
                filesz: u32le(data, o + 16)?,
                memsz: u32le(data, o + 20)?,
                flags: u32le(data, o + 24)?,
            });
        }
        Ok(Elf { data, phdrs })
    }

    pub fn loads(&self) -> impl Iterator<Item = &Phdr> {
        self.phdrs.iter().filter(|p| p.kind == PT_LOAD)
    }

    /// Page-aligned `[min_vaddr, max_vaddr_end)` covered by all PT_LOADs.
    pub fn load_span(&self) -> Result<(u32, u32), LoadError> {
        let mut min = u32::MAX;
        let mut max = 0u64;
        let mut any = false;
        for p in self.loads() {
            any = true;
            if p.filesz > p.memsz {
                return Err(LoadError::BadElf("filesz larger than memsz"));
            }
            min = min.min(p.vaddr);
            max = max.max(p.vaddr as u64 + p.memsz as u64);
        }
        if !any {
            return Err(LoadError::BadElf("no PT_LOAD segments"));
        }
        let min = min & !0xfff;
        let max = (max + 0xfff) & !0xfff;
        if max > u32::MAX as u64 {
            return Err(LoadError::BadElf("image too large"));
        }
        Ok((min, max as u32))
    }

    pub fn segment_bytes(&self, p: &Phdr) -> Result<&'a [u8], LoadError> {
        let start = p.offset as usize;
        let end = start.checked_add(p.filesz as usize).ok_or(LoadError::Truncated)?;
        self.data.get(start..end).ok_or(LoadError::Truncated)
    }

    pub fn dynamic_vaddr(&self) -> Option<u32> {
        self.phdrs.iter().find(|p| p.kind == PT_DYNAMIC).map(|p| p.vaddr)
    }

    /// `(vaddr, memsz)` of PT_ARM_EXIDX.
    pub fn exidx(&self) -> Option<(u32, u32)> {
        self.phdrs.iter().find(|p| p.kind == PT_ARM_EXIDX).map(|p| (p.vaddr, p.memsz))
    }
}

/// A dynamic symbol.
#[derive(Debug, Clone)]
pub struct Sym {
    pub name: String,
    pub value: u32,
    pub size: u32,
    info: u8,
    shndx: u16,
}

impl Sym {
    pub fn is_defined(&self) -> bool {
        self.shndx != 0 && !self.name.is_empty()
    }
    pub fn is_weak(&self) -> bool {
        self.info >> 4 == 2
    }
}

/// Entries of the `.dynamic` section that we care about (virtual addresses, not yet biased).
#[derive(Debug, Default, Clone)]
pub struct Dynamic {
    pub needed: Vec<String>,
    pub soname: Option<String>,
    pub init: Option<u32>,
    pub init_array: Option<u32>,
    pub init_arraysz: u32,
    pub fini_array: Option<u32>,
    pub fini_arraysz: u32,
    /// `DT_SYMBOLIC` or `DF_SYMBOLIC`: references to symbols defined in the object bind locally.
    pub symbolic: bool,
    pub rel: Option<u32>,
    pub relsz: u32,
    pub jmprel: Option<u32>,
    pub pltrelsz: u32,
    symtab: Option<u32>,
    strtab: Option<u32>,
    hash: Option<u32>,
    gnu_hash: Option<u32>,
}

impl Dynamic {
    /// Parse the dynamic section at virtual address `dyn_vaddr` of an image mapped at `bias`.
    pub fn read(mem: &GuestMemory, bias: u32, dyn_vaddr: u32) -> Result<Dynamic, LoadError> {
        let mut d = Dynamic::default();
        let mut needed_offsets = Vec::new();
        let mut soname_off = None;
        let mut syment = 16;
        let mut done = false;
        for i in 0..MAX_DYNAMIC_ENTRIES {
            let a = bias.wrapping_add(dyn_vaddr).wrapping_add(i * 8);
            let tag = mem.read_u32(a)?;
            let val = mem.read_u32(a + 4)?;
            match tag {
                DT_NULL => {
                    done = true;
                    break;
                }
                DT_NEEDED => needed_offsets.push(val),
                DT_SONAME => soname_off = Some(val),
                DT_PLTRELSZ => d.pltrelsz = val,
                DT_HASH => d.hash = Some(val),
                DT_GNU_HASH => d.gnu_hash = Some(val),
                DT_STRTAB => d.strtab = Some(val),
                DT_SYMTAB => d.symtab = Some(val),
                DT_SYMENT => syment = val,
                DT_RELA => return Err(LoadError::Unsupported("DT_RELA relocations".into())),
                DT_REL => d.rel = Some(val),
                DT_RELSZ => d.relsz = val,
                DT_JMPREL => d.jmprel = Some(val),
                DT_INIT => d.init = Some(val),
                DT_SYMBOLIC => d.symbolic = true,
                DT_FLAGS => d.symbolic |= val & DF_SYMBOLIC != 0,
                DT_INIT_ARRAY => d.init_array = Some(val),
                DT_INIT_ARRAYSZ => d.init_arraysz = val,
                DT_FINI_ARRAY => d.fini_array = Some(val),
                DT_FINI_ARRAYSZ => d.fini_arraysz = val,
                _ => {}
            }
        }
        if !done {
            return Err(LoadError::BadElf("unterminated dynamic section"));
        }
        if syment != 16 {
            return Err(LoadError::BadElf("unexpected DT_SYMENT"));
        }
        if let Some(strtab) = d.strtab {
            for off in needed_offsets {
                d.needed.push(mem.read_cstr(bias.wrapping_add(strtab).wrapping_add(off))?);
            }
            if let Some(off) = soname_off {
                d.soname = Some(mem.read_cstr(bias.wrapping_add(strtab).wrapping_add(off))?);
            }
        } else if !needed_offsets.is_empty() || soname_off.is_some() {
            return Err(LoadError::BadElf("DT_NEEDED without DT_STRTAB"));
        }
        Ok(d)
    }

    fn symbol_count(&self, mem: &GuestMemory, bias: u32) -> Result<u32, LoadError> {
        if let Some(h) = self.hash {
            // DT_HASH: { nbucket, nchain, ... }; nchain equals the number of symbols.
            return Ok(mem.read_u32(bias.wrapping_add(h).wrapping_add(4))?);
        }
        if let Some(h) = self.gnu_hash {
            let base = bias.wrapping_add(h);
            let nbuckets = mem.read_u32(base)?;
            let symoffset = mem.read_u32(base + 4)?;
            let bloom_size = mem.read_u32(base + 8)?;
            let buckets = base + 16 + bloom_size * 4;
            let chain = buckets + nbuckets * 4;
            let mut highest = 0;
            for i in 0..nbuckets {
                highest = highest.max(mem.read_u32(buckets + i * 4)?);
            }
            if highest < symoffset {
                return Ok(symoffset);
            }
            // Walk the chain of the highest bucket to its end marker (low bit set).
            let mut idx = highest;
            loop {
                let h = mem.read_u32(chain + (idx - symoffset) * 4)?;
                idx += 1;
                if h & 1 != 0 {
                    return Ok(idx);
                }
            }
        }
        Ok(0)
    }

    /// Read the whole dynamic symbol table (empty when the image has none).
    pub fn read_symbols(&self, mem: &GuestMemory, bias: u32) -> Result<Vec<Sym>, LoadError> {
        let (Some(symtab), Some(strtab)) = (self.symtab, self.strtab) else { return Ok(Vec::new()) };
        let n = self.symbol_count(mem, bias)?;
        let mut out = Vec::with_capacity(n as usize);
        for i in 0..n {
            let a = bias.wrapping_add(symtab).wrapping_add(i * 16);
            let name_off = mem.read_u32(a)?;
            let name = if name_off == 0 {
                String::new()
            } else {
                mem.read_cstr(bias.wrapping_add(strtab).wrapping_add(name_off))?
            };
            out.push(Sym {
                name,
                value: mem.read_u32(a + 4)?,
                size: mem.read_u32(a + 8)?,
                info: mem.read_u8(a + 12)?,
                shndx: mem.read_u16(a + 14)?,
            });
        }
        Ok(out)
    }
}

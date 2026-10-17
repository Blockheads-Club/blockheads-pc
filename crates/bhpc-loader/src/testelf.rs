//! Test-only builder of tiny ELF32/ARM shared objects.

#[derive(Clone)]
pub struct SymDef {
    pub name: &'static str,
    pub value: u32,
    pub defined: bool,
    pub weak: bool,
}

pub fn def(name: &'static str, value: u32) -> SymDef {
    SymDef { name, value, defined: true, weak: false }
}
pub fn undef(name: &'static str) -> SymDef {
    SymDef { name, value: 0, defined: false, weak: false }
}
pub fn weak_undef(name: &'static str) -> SymDef {
    SymDef { name, value: 0, defined: false, weak: true }
}

#[derive(Clone, Copy)]
pub struct Rel {
    pub off: u32,
    pub kind: u32,
    pub sym: u32,
}

#[derive(Clone, Copy, PartialEq)]
pub enum HashKind {
    None,
    Sysv,
    Gnu,
}

#[derive(Clone)]
pub struct Builder {
    pub syms: Vec<SymDef>,
    pub rels: Vec<Rel>,
    pub jmprels: Vec<Rel>,
    pub needed: Vec<&'static str>,
    pub soname: Option<&'static str>,
    pub init: Option<u32>,
    pub init_array: Vec<u32>,
    pub fini_array: Vec<u32>,
    pub hash: HashKind,
    pub exidx: bool,
    pub scratch: Vec<u32>,
    pub use_rela: bool,
    pub unterminated_dynamic: bool,
    pub syment: u32,
    pub omit_dynamic: bool,
    pub omit_strtab: bool,
    pub omit_symtab: bool,
    pub vaddr_base: u32,
    pub extra_bss: u32,
    pub etype: u16,
    pub machine: u16,
    pub class: u8,
    pub data: u8,
    pub phentsize: u16,
    pub no_load: bool,
    pub filesz_gt_memsz: bool,
    pub huge_memsz: bool,
    pub segment_past_eof: bool,
    pub truncate_to: Option<usize>,
}

impl Default for Builder {
    fn default() -> Self {
        Builder {
            syms: vec![],
            rels: vec![],
            jmprels: vec![],
            needed: vec![],
            soname: None,
            init: None,
            init_array: vec![],
            fini_array: vec![],
            hash: HashKind::Sysv,
            exidx: false,
            scratch: vec![0; 16],
            use_rela: false,
            unterminated_dynamic: false,
            syment: 16,
            omit_dynamic: false,
            omit_strtab: false,
            omit_symtab: false,
            vaddr_base: 0,
            extra_bss: 0,
            etype: 3,
            machine: 40,
            class: 1,
            data: 1,
            phentsize: 32,
            no_load: false,
            filesz_gt_memsz: false,
            huge_memsz: false,
            segment_past_eof: false,
            truncate_to: None,
        }
    }
}

fn p32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn align4(v: &mut Vec<u8>) {
    while v.len() % 4 != 0 {
        v.push(0);
    }
}

pub struct Image {
    pub bytes: Vec<u8>,
    /// ELF virtual address of `scratch[0]`.
    pub scratch_vaddr: u32,
}

impl Builder {
    pub fn build(&self) -> Image {
        let base = self.vaddr_base; // file offset == vaddr - base for the single PT_LOAD
        let nph = 1 + usize::from(!self.omit_dynamic) + usize::from(self.exidx);
        let mut b = vec![0u8; 52 + 32 * 3]; // always room for three program headers
        let vaddr = |off: usize| base + off as u32;

        // scratch data first, so its address does not depend on the table sizes
        let scratch_off = b.len();
        for w in &self.scratch {
            p32(&mut b, *w);
        }

        // strtab
        let mut strtab = vec![0u8];
        let mut add_str = |s: &str| -> u32 {
            let o = strtab.len() as u32;
            strtab.extend_from_slice(s.as_bytes());
            strtab.push(0);
            o
        };
        let name_offs: Vec<u32> = self.syms.iter().map(|s| add_str(s.name)).collect();
        let needed_offs: Vec<u32> = self.needed.iter().map(|s| add_str(s)).collect();
        let soname_off = self.soname.map(&mut add_str);
        let strtab_off = b.len();
        b.extend_from_slice(&strtab);
        align4(&mut b);

        // symtab
        let symtab_off = b.len();
        b.extend_from_slice(&[0u8; 16]);
        for (s, no) in self.syms.iter().zip(&name_offs) {
            p32(&mut b, *no);
            p32(&mut b, s.value);
            p32(&mut b, 0);
            let bind: u8 = if s.weak { 2 } else { 1 };
            b.push(bind << 4 | 2);
            b.push(0);
            let shndx: u16 = if s.defined { 1 } else { 0 };
            b.extend_from_slice(&shndx.to_le_bytes());
        }

        // hash
        let nsyms = self.syms.len() as u32 + 1;
        let hash_off = b.len();
        match self.hash {
            HashKind::None => {}
            HashKind::Sysv => {
                p32(&mut b, 1);
                p32(&mut b, nsyms);
                p32(&mut b, 0);
                for _ in 0..nsyms {
                    p32(&mut b, 0);
                }
            }
            HashKind::Gnu => {
                p32(&mut b, 1); // nbuckets
                p32(&mut b, 1); // symoffset
                p32(&mut b, 1); // bloom_size
                p32(&mut b, 0); // bloom_shift
                p32(&mut b, 0); // bloom[0]
                p32(&mut b, if nsyms > 1 { 1 } else { 0 }); // bucket 0
                for i in 1..nsyms {
                    p32(&mut b, if i == nsyms - 1 { 1 } else { 0 });
                }
            }
        }

        // relocation tables
        let rel_bytes = |rels: &[Rel]| {
            let mut v = Vec::new();
            for r in rels {
                p32(&mut v, r.off);
                p32(&mut v, r.sym << 8 | r.kind);
            }
            v
        };
        let rel_off = b.len();
        let rel = rel_bytes(&self.rels);
        b.extend_from_slice(&rel);
        let jmprel_off = b.len();
        let jr = rel_bytes(&self.jmprels);
        b.extend_from_slice(&jr);

        // init/fini arrays
        let init_off = b.len();
        for f in &self.init_array {
            p32(&mut b, *f);
        }
        let fini_off = b.len();
        for f in &self.fini_array {
            p32(&mut b, *f);
        }

        // exidx
        let exidx_off = b.len();
        if self.exidx {
            for w in [0u32, 1, 2, 3] {
                p32(&mut b, w);
            }
        }

        // dynamic
        let dyn_off = b.len();
        let mut dynv: Vec<(u32, u32)> = Vec::new();
        for o in &needed_offs {
            dynv.push((1, *o));
        }
        if let Some(o) = soname_off {
            dynv.push((14, o));
        }
        if !self.omit_strtab {
            dynv.push((5, vaddr(strtab_off)));
        }
        if !self.omit_symtab {
            dynv.push((6, vaddr(symtab_off)));
        }
        dynv.push((11, self.syment));
        match self.hash {
            HashKind::None => {}
            HashKind::Sysv => dynv.push((4, vaddr(hash_off))),
            HashKind::Gnu => dynv.push((0x6fff_fef5, vaddr(hash_off))),
        }
        if !self.rels.is_empty() {
            dynv.push((if self.use_rela { 7 } else { 17 }, vaddr(rel_off)));
            dynv.push((18, rel.len() as u32));
        }
        if !self.jmprels.is_empty() {
            dynv.push((23, vaddr(jmprel_off)));
            dynv.push((2, jr.len() as u32));
        }
        if let Some(i) = self.init {
            dynv.push((12, i));
        }
        if !self.init_array.is_empty() {
            dynv.push((25, vaddr(init_off)));
            dynv.push((27, self.init_array.len() as u32 * 4));
        }
        if !self.fini_array.is_empty() {
            dynv.push((26, vaddr(fini_off)));
            dynv.push((28, self.fini_array.len() as u32 * 4));
        }
        if !self.unterminated_dynamic {
            dynv.push((0, 0));
        }
        for (t, v) in dynv {
            p32(&mut b, t);
            p32(&mut b, v);
        }
        let total = b.len();
        if self.unterminated_dynamic {
            // Fill the rest of the (4096 entry) window with non-terminating tags so reads stay mapped.
            for _ in 0..4100 {
                p32(&mut b, 0x7000_0000);
                p32(&mut b, 0);
            }
        }
        let file_end = b.len();

        // ELF header
        b[0..4].copy_from_slice(b"\x7fELF");
        b[4] = self.class;
        b[5] = self.data;
        b[6] = 1;
        b[16..18].copy_from_slice(&self.etype.to_le_bytes());
        b[18..20].copy_from_slice(&self.machine.to_le_bytes());
        b[20..24].copy_from_slice(&1u32.to_le_bytes());
        b[28..32].copy_from_slice(&52u32.to_le_bytes());
        b[40..42].copy_from_slice(&52u16.to_le_bytes());
        b[42..44].copy_from_slice(&self.phentsize.to_le_bytes());
        b[44..46].copy_from_slice(&(nph as u16).to_le_bytes());

        let mut ph = Vec::new();
        let mut push_ph = |kind: u32, off: u32, va: u32, filesz: u32, memsz: u32| {
            for x in [kind, off, va, va, filesz, memsz, 7, 0x1000] {
                p32(&mut ph, x);
            }
        };
        if !self.no_load {
            let mut filesz = file_end as u32;
            let mut memsz = file_end as u32 + self.extra_bss;
            if self.filesz_gt_memsz {
                memsz = filesz - 1;
            }
            if self.huge_memsz {
                memsz = 0xffff_f000;
            }
            if self.segment_past_eof {
                filesz += 0x1000;
                memsz += 0x1000;
            }
            push_ph(1, 0, base, filesz, memsz);
        } else {
            push_ph(0x6474_e551, 0, 0, 0, 0); // PT_GNU_STACK only
        }
        if !self.omit_dynamic {
            push_ph(2, dyn_off as u32, vaddr(dyn_off), (total - dyn_off) as u32, (total - dyn_off) as u32);
        }
        if self.exidx {
            push_ph(0x7000_0001, exidx_off as u32, vaddr(exidx_off), 16, 16);
        }
        b[52..52 + ph.len()].copy_from_slice(&ph);

        if let Some(n) = self.truncate_to {
            b.truncate(n);
        }
        Image { bytes: b, scratch_vaddr: vaddr(scratch_off) }
    }
}

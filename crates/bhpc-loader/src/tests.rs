use crate::testelf::*;
use crate::*;

fn mem() -> Arc<GuestMemory> {
    Arc::new(GuestMemory::new().unwrap())
}

fn no_host() -> impl FnMut(&str) -> Option<u32> {
    |_| None
}

fn rel(off: u32, kind: u32, sym: u32) -> Rel {
    Rel { off, kind, sym }
}

/// Load `b` into a fresh linker; returns (linker, scratch guest address).
fn load(b: &Builder) -> (Linker, u32) {
    let img = b.build();
    let mut l = Linker::new(mem());
    let m = l.load("t.so", &img.bytes, &mut no_host()).unwrap().clone();
    (l, m.bias + img.scratch_vaddr)
}

fn load_err(b: &Builder) -> LoadError {
    let img = b.build();
    let mut l = Linker::new(mem());
    l.load("t.so", &img.bytes, &mut no_host()).unwrap_err()
}

#[test]
fn minimal_image_without_dynamic_section() {
    let b = Builder { omit_dynamic: true, scratch: vec![0xdead_beef], ..Default::default() };
    let (l, scratch) = load(&b);
    let m = &l.modules()[0];
    assert_eq!(m.name, "t.so");
    assert!(m.exports.is_empty() && m.needed.is_empty() && m.soname.is_none());
    assert_eq!(m.relocs_applied, 0);
    assert_eq!(l.modules().len(), 1);
    assert_eq!(l.mem.read_u32(scratch).unwrap(), 0xdead_beef);
}

#[test]
fn bss_is_zero_and_span_covers_memsz() {
    let b = Builder { extra_bss: 0x3000, ..Default::default() };
    let (l, _) = load(&b);
    let m = &l.modules()[0];
    assert!(m.size >= 0x3000);
    assert_eq!(l.mem.read_u32(m.start + m.size - 4).unwrap(), 0);
}

#[test]
fn nonzero_first_vaddr_is_handled() {
    let b = Builder { vaddr_base: 0x1000, scratch: vec![7], ..Default::default() };
    let img = b.build();
    let mut l = Linker::new(mem());
    let m = l.load("hi.so", &img.bytes, &mut no_host()).unwrap().clone();
    assert_eq!(m.bias, m.start - 0x1000);
    assert_eq!(l.mem.read_u32(m.bias + img.scratch_vaddr).unwrap(), 7);
}

#[test]
fn relative_relocation_adds_bias() {
    let img0 = Builder::default().build();
    let s = img0.scratch_vaddr;
    let b = Builder {
        scratch: vec![0x100, 0x200],
        rels: vec![rel(s, R_ARM_RELATIVE, 0), rel(s + 4, R_ARM_NONE, 0)],
        ..Default::default()
    };
    let (l, scratch) = load(&b);
    let m = &l.modules()[0];
    assert_eq!(l.mem.read_u32(scratch).unwrap(), m.bias + 0x100);
    assert_eq!(l.mem.read_u32(scratch + 4).unwrap(), 0x200, "R_ARM_NONE is skipped");
    assert_eq!(m.relocs_applied, 1);
}

#[test]
fn abs32_local_symbol_with_addend_and_thumb_bit() {
    let s = Builder::default().build().scratch_vaddr;
    let mut b =
        Builder { syms: vec![def("thumb_fn", 0x41), def("data", 0x80)], scratch: vec![4, 8], ..Default::default() };
    b.rels = vec![rel(s, R_ARM_ABS32, 1), rel(s + 4, R_ARM_ABS32, 2)];
    let (l, scratch) = load(&b);
    let m = &l.modules()[0];
    assert_eq!(l.mem.read_u32(scratch).unwrap(), m.bias + 0x41 + 4);
    assert_eq!(l.mem.read_u32(scratch + 4).unwrap(), m.bias + 0x80 + 8);
    assert_eq!(m.export("thumb_fn"), Some(m.bias + 0x41), "thumb bit preserved in exports");
    assert_eq!(m.export("missing"), None);
}

#[test]
fn glob_dat_and_jump_slot_bind_to_host_symbols_once() {
    let s = Builder::default().build().scratch_vaddr;
    let b = Builder {
        syms: vec![undef("puts"), undef("abort")],
        scratch: vec![0xaaaa; 4],
        rels: vec![rel(s, R_ARM_GLOB_DAT, 1), rel(s + 4, R_ARM_ABS32, 1)],
        jmprels: vec![rel(s + 8, R_ARM_JUMP_SLOT, 2), rel(s + 12, R_ARM_JUMP_SLOT, 1)],
        ..Default::default()
    };
    let img = b.build();
    let mut l = Linker::new(mem());
    let mut calls = Vec::new();
    let mut host = |n: &str| {
        calls.push(n.to_string());
        Some(if n == "puts" { 0xFFFF_0008 } else { 0xFFFF_0010 })
    };
    let m = l.load("t.so", &img.bytes, &mut host).unwrap().clone();
    let p = m.bias + img.scratch_vaddr;
    assert_eq!(l.mem.read_u32(p).unwrap(), 0xFFFF_0008, "GLOB_DAT writes S, not S+A");
    assert_eq!(l.mem.read_u32(p + 4).unwrap(), 0xaaaa + 0xFFFF_0008, "ABS32 adds the in-place addend");
    assert_eq!(l.mem.read_u32(p + 8).unwrap(), 0xFFFF_0010);
    assert_eq!(l.mem.read_u32(p + 12).unwrap(), 0xFFFF_0008);
    assert_eq!(calls, vec!["puts", "abort"], "each symbol is resolved once");
    assert_eq!(m.relocs_applied, 4);
}

#[test]
fn loaded_module_symbols_win_over_host_and_weak_falls_back_to_zero() {
    let mut l = Linker::new(mem());
    let lib = Builder { syms: vec![def("shared_fn", 0x20)], ..Default::default() };
    let lib_mod = l.load("libshared.so", &lib.build().bytes, &mut no_host()).unwrap().clone();

    let s = Builder::default().build().scratch_vaddr;
    let b = Builder {
        syms: vec![undef("shared_fn"), weak_undef("maybe")],
        scratch: vec![0; 2],
        rels: vec![rel(s, R_ARM_GLOB_DAT, 1), rel(s + 4, R_ARM_GLOB_DAT, 2)],
        ..Default::default()
    };
    let img = b.build();
    // The host must only be asked for the weak symbol nobody defines.
    let mut host2 = |n: &str| if n == "maybe" { None } else { panic!("host must not be asked for {n}") };
    let m = l.load("app.so", &img.bytes, &mut host2).unwrap().clone();
    let p = m.bias + img.scratch_vaddr;
    assert_eq!(l.mem.read_u32(p).unwrap(), lib_mod.bias + 0x20);
    assert_eq!(l.mem.read_u32(p + 4).unwrap(), 0);
    assert_eq!(l.lookup("shared_fn"), Some(lib_mod.bias + 0x20));
    assert_eq!(l.lookup("nope"), None);
}

#[test]
fn unresolved_strong_symbol_is_an_error() {
    let s = Builder::default().build().scratch_vaddr;
    let b = Builder { syms: vec![undef("nope")], rels: vec![rel(s, R_ARM_GLOB_DAT, 1)], ..Default::default() };
    assert_eq!(load_err(&b), LoadError::Unresolved("nope".into()));
}

#[test]
fn bad_symbol_index_and_unsupported_relocation() {
    let s = Builder::default().build().scratch_vaddr;
    let b = Builder { rels: vec![rel(s, R_ARM_ABS32, 9)], ..Default::default() };
    assert!(matches!(load_err(&b), LoadError::BadAddress(_)));
    let b = Builder { rels: vec![rel(s, 20, 0)], ..Default::default() };
    assert_eq!(load_err(&b), LoadError::Unsupported("relocation type 20".into()));
}

#[test]
fn relocation_target_outside_image_is_a_memory_error() {
    let b = Builder { rels: vec![rel(0x00ff_0000, R_ARM_RELATIVE, 0)], ..Default::default() };
    assert!(matches!(load_err(&b), LoadError::Mem(_)));
}

#[test]
fn needed_soname_and_lookup_by_either_name() {
    let b = Builder { needed: vec!["libc.so", "libm.so"], soname: Some("libfoo.so"), ..Default::default() };
    let (l, _) = load(&b);
    let m = &l.modules()[0];
    assert_eq!(m.needed, vec!["libc.so", "libm.so"]);
    assert_eq!(m.soname.as_deref(), Some("libfoo.so"));
    assert!(l.module("t.so").is_some());
    assert!(l.module("libfoo.so").is_some());
    assert!(l.module("other").is_none());
}

#[test]
fn constructors_skip_markers() {
    let b = Builder {
        init: Some(0x40),
        init_array: vec![0x1000, 0, u32::MAX, 0x2000],
        fini_array: vec![0x3000],
        ..Default::default()
    };
    let (l, _) = load(&b);
    let m = &l.modules()[0];
    assert_eq!(m.init, Some(m.bias + 0x40));
    assert_eq!(m.init_array, vec![0x1000, 0x2000], "raw (unrelocated) values: no RELATIVE relocs in this image");
    assert_eq!(m.fini_array, vec![0x3000]);
    assert_eq!(m.constructors(), vec![m.bias + 0x40, 0x1000, 0x2000]);
}

#[test]
fn exidx_and_module_containing_pc() {
    let b = Builder { exidx: true, ..Default::default() };
    let (l, _) = load(&b);
    let m = &l.modules()[0];
    let (addr, count) = m.exidx.unwrap();
    assert_eq!(count, 2);
    assert!(m.contains(addr));
    assert_eq!(l.find_exidx(addr), Some((addr, 2)));
    assert_eq!(l.find_exidx(0x10), None, "pc outside every module");
    assert!(!m.contains(m.start - 1));
    assert!(!m.contains(m.start + m.size));
    let plain = Builder::default();
    let (l2, _) = load(&plain);
    assert_eq!(l2.modules()[0].exidx, None);
    assert_eq!(l2.find_exidx(l2.modules()[0].start), None);
}

#[test]
fn symbol_table_counting_variants() {
    // Without any hash table the symbol count is unknown, so exports stay empty.
    let b = Builder { syms: vec![def("a", 4)], hash: HashKind::None, ..Default::default() };
    assert!(load(&b).0.modules()[0].exports.is_empty());
    // GNU hash only.
    let b = Builder { syms: vec![def("a", 4), def("b", 8), undef("c")], hash: HashKind::Gnu, ..Default::default() };
    let (l, _) = load(&b);
    let m = &l.modules()[0];
    assert_eq!((m.export("a"), m.export("b"), m.export("c")), (Some(m.bias + 4), Some(m.bias + 8), None));
    // GNU hash with no hashed symbols: only the null symbol exists.
    let b = Builder { hash: HashKind::Gnu, ..Default::default() };
    assert!(load(&b).0.modules()[0].exports.is_empty());
    // Missing DT_SYMTAB or DT_STRTAB: no symbols at all.
    let b = Builder { syms: vec![def("a", 4)], omit_symtab: true, ..Default::default() };
    assert!(load(&b).0.modules()[0].exports.is_empty());
    let b = Builder { syms: vec![def("a", 4)], omit_strtab: true, ..Default::default() };
    assert!(load(&b).0.modules()[0].exports.is_empty());
}

#[test]
fn rejects_malformed_dynamic_sections() {
    let b = Builder { use_rela: true, rels: vec![rel(0, R_ARM_RELATIVE, 0)], ..Default::default() };
    assert!(matches!(load_err(&b), LoadError::Unsupported(m) if m.contains("RELA")));
    let b = Builder { unterminated_dynamic: true, ..Default::default() };
    assert_eq!(load_err(&b), LoadError::BadElf("unterminated dynamic section"));
    let b = Builder { syment: 24, ..Default::default() };
    assert_eq!(load_err(&b), LoadError::BadElf("unexpected DT_SYMENT"));
    let b = Builder { needed: vec!["libc.so"], omit_strtab: true, ..Default::default() };
    assert_eq!(load_err(&b), LoadError::BadElf("DT_NEEDED without DT_STRTAB"));
    let b = Builder { soname: Some("x.so"), omit_strtab: true, ..Default::default() };
    assert_eq!(load_err(&b), LoadError::BadElf("DT_NEEDED without DT_STRTAB"));
}

#[test]
fn rejects_malformed_headers() {
    assert_eq!(
        load_err(&Builder { truncate_to: Some(20), ..Default::default() }),
        LoadError::BadElf("missing ELF magic")
    );
    let mut bad_magic = Builder::default().build().bytes;
    bad_magic[0] = 0;
    let mut l = Linker::new(mem());
    assert_eq!(l.load("x", &bad_magic, &mut no_host()).unwrap_err(), LoadError::BadElf("missing ELF magic"));
    assert_eq!(load_err(&Builder { class: 2, ..Default::default() }), LoadError::BadElf("not ELFCLASS32"));
    assert_eq!(load_err(&Builder { data: 2, ..Default::default() }), LoadError::BadElf("not little endian"));
    assert_eq!(
        load_err(&Builder { etype: 2, ..Default::default() }),
        LoadError::BadElf("not a shared object (ET_DYN)")
    );
    assert_eq!(load_err(&Builder { machine: 3, ..Default::default() }), LoadError::BadElf("not EM_ARM"));
    assert_eq!(
        load_err(&Builder { phentsize: 56, ..Default::default() }),
        LoadError::BadElf("unexpected program header size")
    );
    assert_eq!(load_err(&Builder { truncate_to: Some(60), ..Default::default() }), LoadError::Truncated);
    assert_eq!(load_err(&Builder { no_load: true, ..Default::default() }), LoadError::BadElf("no PT_LOAD segments"));
    assert_eq!(
        load_err(&Builder { filesz_gt_memsz: true, ..Default::default() }),
        LoadError::BadElf("filesz larger than memsz")
    );
    assert_eq!(
        load_err(&Builder { huge_memsz: true, vaddr_base: 0x2000, ..Default::default() }),
        LoadError::BadElf("image too large")
    );
    assert_eq!(load_err(&Builder { segment_past_eof: true, ..Default::default() }), LoadError::Truncated);
}

#[test]
fn huge_image_exhausts_the_arena() {
    let b = Builder { extra_bss: 0x4000_0000, ..Default::default() };
    assert_eq!(load_err(&b), LoadError::Mem(bhpc_mem::MemError::OutOfSpace));
}

#[test]
fn truncated_dynamic_table_is_a_memory_error() {
    // Dynamic section pointing past the mapped image.
    let img = Builder { omit_dynamic: false, ..Default::default() }.build();
    let mut bytes = img.bytes;
    // Patch PT_DYNAMIC (second program header) vaddr to an unmapped address.
    let ph2 = 52 + 32;
    bytes[ph2 + 8..ph2 + 12].copy_from_slice(&0x00f0_0000u32.to_le_bytes());
    let mut l = Linker::new(mem());
    assert!(matches!(l.load("x", &bytes, &mut no_host()).unwrap_err(), LoadError::Mem(_)));
}

#[test]
fn error_display_and_conversion() {
    let all: Vec<LoadError> = vec![
        LoadError::BadElf("x"),
        LoadError::Unsupported("y".into()),
        LoadError::Truncated,
        LoadError::BadAddress(0x10),
        LoadError::Unresolved("sym".into()),
        LoadError::NotFound("libx.so".into()),
        LoadError::Source("zip".into()),
        bhpc_mem::MemError::OutOfSpace.into(),
    ];
    let text: Vec<String> = all.iter().map(|e| e.to_string()).collect();
    assert_eq!(text[0], "bad ELF: x");
    assert_eq!(text[1], "unsupported: y");
    assert_eq!(text[2], "truncated ELF file");
    assert_eq!(text[3], "address 0x10 is outside the image");
    assert_eq!(text[4], "unresolved symbol sym");
    assert_eq!(text[5], "library libx.so not found");
    assert_eq!(text[6], "library source: zip");
    assert!(text[7].starts_with("guest memory:"));
    let _: &dyn std::error::Error = &all[0];
}

#[test]
fn host_symbols_trait_is_implemented_for_closures() {
    let mut f = |n: &str| if n == "a" { Some(1) } else { None };
    let h: &mut dyn HostSymbols = &mut f;
    assert_eq!(h.resolve("a"), Some(1));
    assert_eq!(h.resolve("b"), None);
}

#[test]
fn every_truncation_of_the_headers_is_an_error_not_a_panic() {
    let full = Builder { exidx: true, syms: vec![def("a", 4)], ..Default::default() }.build().bytes;
    let mut l = Linker::new(mem());
    for n in 0..52 + 32 * 4 {
        let err = l.load("t.so", &full[..n], &mut no_host()).unwrap_err();
        assert!(matches!(err, LoadError::BadElf(_) | LoadError::Truncated), "length {n}: {err:?}");
    }
}

//! Multi-module loading, symbol binding policy, trap thunks and protections.

use crate::testelf::*;
use crate::*;
use std::collections::BTreeMap;

fn linker() -> Linker {
    Linker::new(Arc::new(GuestMemory::new().unwrap()))
}

fn rel(off: u32, kind: u32, sym: u32) -> Rel {
    Rel { off, kind, sym }
}

fn scratch() -> u32 {
    Builder::default().build().scratch_vaddr
}

/// Library source over an in-memory map; records fetch order.
struct Map {
    libs: BTreeMap<&'static str, Vec<u8>>,
    asked: Vec<String>,
    fail: Option<&'static str>,
}

impl Map {
    fn new(libs: Vec<(&'static str, Builder)>) -> Map {
        Map { libs: libs.into_iter().map(|(n, b)| (n, b.build().bytes)).collect(), asked: vec![], fail: None }
    }
}

impl LibSource for Map {
    fn fetch(&mut self, name: &str) -> Result<Option<Vec<u8>>, LoadError> {
        self.asked.push(name.to_string());
        if self.fail == Some(name) {
            return Err(LoadError::Source("boom".into()));
        }
        Ok(self.libs.get(name).cloned())
    }
}

struct Traps(Vec<String>);

impl HostSymbols for Traps {
    fn resolve(&mut self, _: &str) -> Option<u32> {
        None
    }
    fn trap(&mut self, name: &str) -> Option<u32> {
        self.0.push(name.to_string());
        Some(0xF000_0000 + 8 * self.0.len() as u32)
    }
}

fn none() -> impl FnMut(&str) -> Option<u32> {
    |_| None
}

#[test]
fn load_all_orders_dependencies_first_and_reports_host_libs() {
    let s = scratch();
    let app = Builder {
        needed: vec!["libc.so", "libmid.so", "libleaf.so"],
        syms: vec![undef("leaf_fn"), undef("mid_fn")],
        scratch: vec![0; 2],
        rels: vec![rel(s, R_ARM_GLOB_DAT, 1), rel(s + 4, R_ARM_GLOB_DAT, 2)],
        ..Default::default()
    };
    let mid = Builder { needed: vec!["libleaf.so", "libm.so"], syms: vec![def("mid_fn", 0x10)], ..Default::default() };
    let leaf = Builder { syms: vec![def("leaf_fn", 0x20)], init_array: vec![0x40], ..Default::default() };
    let mut src = Map::new(vec![("libapp.so", app), ("libmid.so", mid), ("libleaf.so", leaf)]);
    let mut l = linker();
    let sum = l.load_all(&["libapp.so"], &mut src, &mut none()).unwrap();
    let names: Vec<&str> = l.modules().iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["libleaf.so", "libmid.so", "libapp.so"]);
    assert_eq!(sum.range, 0..3);
    assert_eq!(sum.host_libs, ["libc.so", "libm.so"]);
    assert_eq!(src.asked.iter().filter(|n| *n == "libleaf.so").count(), 1, "each library is fetched once");
    let app = l.module("libapp.so").unwrap();
    let p = app.bias + s;
    assert_eq!(l.mem().read_u32(p).unwrap(), l.module("libleaf.so").unwrap().bias + 0x20);
    assert_eq!(l.mem().read_u32(p + 4).unwrap(), l.module("libmid.so").unwrap().bias + 0x10);
    assert_eq!(app.relocs_applied, 2);
    assert_eq!(l.module("libleaf.so").unwrap().init_array.len(), 1);
}

#[test]
fn load_all_skips_loaded_modules_and_tolerates_cycles() {
    let a = Builder { needed: vec!["libb.so"], syms: vec![def("a_fn", 4), undef("b_fn")], ..Default::default() };
    let s = scratch();
    let a = Builder { scratch: vec![0], rels: vec![rel(s, R_ARM_GLOB_DAT, 2)], ..a };
    let b = Builder { needed: vec!["liba.so"], syms: vec![def("b_fn", 8)], ..Default::default() };
    let mut src = Map::new(vec![("liba.so", a), ("libb.so", b)]);
    let mut l = linker();
    let sum = l.load_all(&["liba.so", "liba.so"], &mut src, &mut none()).unwrap();
    assert_eq!(sum.range, 0..2);
    // A cyclic import resolves because every module is mapped before any is relocated.
    let a_mod = l.module("liba.so").unwrap();
    assert_eq!(l.mem().read_u32(a_mod.bias + s).unwrap(), l.module("libb.so").unwrap().bias + 8);
    // Loading again: roots and dependencies that are already loaded are skipped.
    let again = l.load_all(&["liba.so", "libb.so"], &mut src, &mut none()).unwrap();
    assert_eq!(again.range, 2..2);
    assert_eq!(l.modules().len(), 2);
}

#[test]
fn load_all_errors_leave_nothing_registered() {
    let mut l = linker();
    // Missing root.
    let mut src = Map::new(vec![]);
    assert_eq!(l.load_all(&["libx.so"], &mut src, &mut none()).unwrap_err(), LoadError::NotFound("libx.so".into()));
    // Source failure.
    let mut src = Map::new(vec![("liba.so", Builder::default())]);
    src.fail = Some("liba.so");
    assert_eq!(l.load_all(&["liba.so"], &mut src, &mut none()).unwrap_err(), LoadError::Source("boom".into()));
    // Bad ELF in a dependency.
    let mut src = Map::new(vec![("liba.so", Builder { needed: vec!["libbad.so"], ..Default::default() })]);
    src.libs.insert("libbad.so", vec![1, 2, 3]);
    assert!(matches!(l.load_all(&["liba.so"], &mut src, &mut none()).unwrap_err(), LoadError::BadElf(_)));
    // Unresolved import in the second module: the first stays out too.
    let s = scratch();
    let bad = Builder {
        needed: vec!["libgood.so"],
        syms: vec![undef("nope")],
        scratch: vec![0],
        rels: vec![rel(s, R_ARM_GLOB_DAT, 1)],
        ..Default::default()
    };
    let mut src = Map::new(vec![("libbad.so", bad), ("libgood.so", Builder::default())]);
    assert_eq!(l.load_all(&["libbad.so"], &mut src, &mut none()).unwrap_err(), LoadError::Unresolved("nope".into()));
    assert!(l.modules().is_empty());
    // Constructor table outside the image: reading it fails after relocation.
    let mut img = Builder { init_array: vec![1], ..Default::default() }.build().bytes;
    let pos = (0..img.len() - 8).rev().find(|&i| img[i..i + 8] == [27, 0, 0, 0, 4, 0, 0, 0]).unwrap();
    img[pos + 4..pos + 8].copy_from_slice(&0x0100_0000u32.to_le_bytes());
    let mut src = Map { libs: BTreeMap::from([("libc0.so", img)]), asked: vec![], fail: None };
    assert!(matches!(l.load_all(&["libc0.so"], &mut src, &mut none()).unwrap_err(), LoadError::Mem(_)));
    assert!(l.modules().is_empty());
    assert!(l.unresolved().is_empty());
}

#[test]
fn closures_are_library_sources() {
    let mut l = linker();
    let bytes = Builder::default().build().bytes;
    let mut src = |n: &str| (n == "libz.so").then(|| bytes.clone());
    assert!(l.load_all(&["libz.so"], &mut src, &mut none()).is_ok());
}

#[test]
fn unresolved_imports_become_counted_traps_in_trap_mode() {
    let s = scratch();
    let mut b = Builder {
        syms: vec![undef("missing"), weak_undef("bridge"), undef("other")],
        scratch: vec![0; 5],
        rels: vec![rel(s, R_ARM_GLOB_DAT, 1), rel(s + 4, R_ARM_ABS32, 1), rel(s + 8, R_ARM_GLOB_DAT, 2)],
        jmprels: vec![rel(s + 12, R_ARM_JUMP_SLOT, 1), rel(s + 16, R_ARM_JUMP_SLOT, 3)],
        ..Default::default()
    };
    let mut l = linker();
    let mut host = Traps(vec![]);
    // Off by default: the default `trap` hook says no, and so does trap mode with such a host.
    let img = b.build();
    let mut plain = |_: &str| None;
    assert_eq!(l.load("a.so", &img.bytes, &mut plain).unwrap_err(), LoadError::Unresolved("missing".into()));
    l.set_trap_unresolved(true);
    assert_eq!(l.load("a.so", &img.bytes, &mut plain).unwrap_err(), LoadError::Unresolved("missing".into()));
    assert!(l.unresolved().is_empty(), "failed loads record nothing");
    let m = l.load("a.so", &img.bytes, &mut host).unwrap().clone();
    let p = m.bias + img.scratch_vaddr;
    let mem = l.mem();
    assert_eq!(mem.read_u32(p).unwrap(), 0xF000_0008);
    assert_eq!(mem.read_u32(p + 4).unwrap(), 0xF000_0008, "ABS32 with a zero addend");
    assert_eq!(mem.read_u32(p + 8).unwrap(), 0, "weak imports stay null, no trap");
    assert_eq!(mem.read_u32(p + 12).unwrap(), 0xF000_0008);
    assert_eq!(mem.read_u32(p + 16).unwrap(), 0xF000_0010);
    assert_eq!(host.0, ["missing", "other"], "one trap per symbol");
    assert_eq!(l.unresolved().get("missing"), Some(&3));
    assert_eq!(l.unresolved().get("other"), Some(&1));
    assert_eq!(l.unresolved().len(), 2);
    b.syms.truncate(1);
}

#[test]
fn symbolic_libraries_bind_locally_others_bind_globally() {
    let s = scratch();
    let mk = |symbolic: bool| Builder {
        syms: vec![def("dup", 0x30)],
        scratch: vec![0],
        rels: vec![rel(s, R_ARM_GLOB_DAT, 1)],
        dt_symbolic: symbolic,
        ..Default::default()
    };
    for (symbolic, expect_global) in [(false, true), (true, false)] {
        let mut l = linker();
        let first = Builder { syms: vec![def("dup", 0x10)], ..Default::default() };
        let f = l.load("first.so", &first.build().bytes, &mut none()).unwrap().clone();
        let img = mk(symbolic).build();
        let m = l.load("second.so", &img.bytes, &mut none()).unwrap().clone();
        assert_eq!(m.symbolic, symbolic);
        let got = l.mem().read_u32(m.bias + img.scratch_vaddr).unwrap();
        assert_eq!(got, if expect_global { f.bias + 0x10 } else { m.bias + 0x30 });
    }
    // DF_SYMBOLIC in DT_FLAGS counts too; other flags do not.
    for (flags, symbolic) in [(2, true), (4, false)] {
        let b = Builder { dt_flags: Some(flags), ..Default::default() };
        let m = linker().load("f.so", &b.build().bytes, &mut none()).unwrap().clone();
        assert_eq!(m.symbolic, symbolic);
    }
    // A non-symbolic library whose own definition is the only one binds to itself.
    let mut l = linker();
    let img = mk(false).build();
    let m = l.load("only.so", &img.bytes, &mut none()).unwrap().clone();
    assert_eq!(l.mem().read_u32(m.bias + img.scratch_vaddr).unwrap(), m.bias + 0x30);
}

#[test]
fn segments_are_recorded_and_protections_applied() {
    let b = Builder { split_segments: true, extra_bss: 0x2000, exidx: true, ..Default::default() };
    let mut l = linker();
    let m = l.load("seg.so", &b.build().bytes, &mut none()).unwrap().clone();
    assert_eq!(m.segments.len(), 2);
    assert_eq!(m.segments[0], Segment { start: m.start, len: 0x1000, prot: PF_R | PF_X });
    assert_eq!(m.segments[1].start, m.start + 0x1000);
    assert_eq!(m.segments[1].prot, PF_R | PF_W);
    l.apply_protections().unwrap();
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    let perms_at = |guest: u32| {
        let host = l.mem().base() as u64 + guest as u64;
        maps.lines()
            .find_map(|line| {
                let (range, rest) = line.split_once(' ')?;
                let (lo, hi) = range.split_once('-')?;
                let (lo, hi) = (u64::from_str_radix(lo, 16).ok()?, u64::from_str_radix(hi, 16).ok()?);
                (lo <= host && host < hi).then(|| rest[..4].to_string())
            })
            .unwrap()
    };
    assert_eq!(perms_at(m.start), "r-xp");
    assert_eq!(perms_at(m.start + 0x1000), "rw-p");
}

#[test]
fn page_runs_merge_and_shared_pages_get_the_union() {
    let seg = |start, len, prot| Segment { start, len, prot };
    assert_eq!(coalesce_pages(&[]), vec![]);
    // Adjacent equal protections merge; a shared page unions the flags; gaps and changes split runs.
    let runs = coalesce_pages(&[
        seg(0x1000, 0x2000, PF_R),
        seg(0x2000, 0x1000, PF_W),
        seg(0x3000, 0x1000, PF_R),
        seg(0x5000, 0x1000, PF_R),
    ]);
    assert_eq!(
        runs,
        vec![(0x1000, 0x1000, PF_R), (0x2000, 0x1000, PF_R | PF_W), (0x3000, 0x1000, PF_R), (0x5000, 0x1000, PF_R)]
    );
    assert_eq!(coalesce_pages(&[seg(0x1000, 0x1000, PF_R), seg(0x2000, 0x2000, PF_R)]), vec![(0x1000, 0x3000, PF_R)]);
}

#[test]
fn every_protection_bit_maps_and_errors_surface() {
    let mut l = linker();
    let mut m = l.load("p.so", &Builder::default().build().bytes, &mut none()).unwrap().clone();
    m.segments = vec![
        Segment { start: 0x1000, len: 0x1000, prot: 0 },
        Segment { start: 0x2000, len: 0x1000, prot: PF_R },
        Segment { start: 0x3000, len: 0x1000, prot: PF_W },
        Segment { start: 0x4000, len: 0x1000, prot: PF_X },
    ];
    l.modules[0] = m;
    let mut seen = vec![];
    l.apply_protections_with(&mut |start, len, prot| {
        seen.push((start, len, prot));
        Ok(())
    })
    .unwrap();
    assert_eq!(
        seen,
        [
            (0x1000, 0x1000, Prot::NONE),
            (0x2000, 0x1000, Prot::READ),
            (0x3000, 0x1000, Prot::WRITE),
            (0x4000, 0x1000, Prot::EXEC)
        ]
    );
    let err = l.apply_protections_with(&mut |_, _, _| Err(MemError::Os(1))).unwrap_err();
    assert!(matches!(err, LoadError::Mem(MemError::Os(_))));
}

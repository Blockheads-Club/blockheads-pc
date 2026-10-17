use crate::init::*;
use crate::testelf::*;
use crate::*;

fn ctor_world() -> Linker {
    let mut l = Linker::new(Arc::new(GuestMemory::new().unwrap()));
    let mut src = |n: &str| {
        let b = match n {
            "libdep.so" => Builder { init: Some(0x10), init_array: vec![0x1000, 0x2000], ..Default::default() },
            "libapp.so" => Builder { needed: vec!["libdep.so"], init_array: vec![0x3000], ..Default::default() },
            _ => Builder::default(),
        };
        Some(b.build().bytes)
    };
    l.load_all(&["libapp.so", "libplain.so"], &mut src, &mut |_: &str| None).unwrap();
    l
}

struct Mock {
    calls: Vec<(u32, [u32; 3], u64)>,
    fail_at: Option<u32>,
    fault: bool,
}

impl GuestCaller for Mock {
    fn call(&mut self, entry: u32, args: [u32; 3], max: u64) -> Result<u32, CallFailure> {
        self.calls.push((entry, args, max));
        match self.fail_at {
            Some(e) if e == entry && self.fault => Err(CallFailure::Fault { pc: entry + 4, reason: "udf".into() }),
            Some(e) if e == entry => Err(CallFailure::Watchdog { pc: entry + 8 }),
            _ => Ok(7),
        }
    }
}

#[derive(Default)]
struct Rec {
    before: Vec<(String, CtorKind, u32)>,
    after_ok: Vec<bool>,
}

impl InitTrace for Rec {
    fn before(&mut self, c: &CtorInfo<'_>) {
        self.before.push((c.module.to_string(), c.kind, c.entry));
    }
    fn after(&mut self, _: &CtorInfo<'_>, r: &Result<u32, CallFailure>, _: Duration) {
        self.after_ok.push(r.is_ok());
    }
}

use std::time::Duration;

#[test]
fn constructors_run_in_dependency_order_with_init_before_array() {
    let l = ctor_world();
    let dep = l.module("libdep.so").unwrap();
    let mut mock = Mock { calls: vec![], fail_at: None, fault: false };
    let mut rec = Rec::default();
    let opts = InitOptions { watchdog_cycles: 123, args: [1, 2, 3], keep_going: false };
    let rep = run_constructors(l.modules(), &mut mock, &mut rec, &opts);
    assert!(rep.ok());
    assert_eq!(rep.ran, 4);
    assert_eq!(rep.per_module, [("libdep.so".into(), 3), ("libapp.so".into(), 1), ("libplain.so".into(), 0)]);
    let entries: Vec<u32> = mock.calls.iter().map(|c| c.0).collect();
    assert_eq!(entries, [dep.bias + 0x10, 0x1000, 0x2000, 0x3000]);
    assert!(mock.calls.iter().all(|c| c.1 == [1, 2, 3] && c.2 == 123));
    assert_eq!(rec.before[0], ("libdep.so".to_string(), CtorKind::Init, dep.bias + 0x10));
    assert_eq!(rec.before[1].1, CtorKind::InitArray(0));
    assert_eq!(rec.before[2].1, CtorKind::InitArray(1));
    assert_eq!(rec.after_ok, [true; 4]);
}

#[test]
fn first_failure_stops_unless_keep_going() {
    let l = ctor_world();
    let mut mock = Mock { calls: vec![], fail_at: Some(0x1000), fault: false };
    let rep = run_constructors(l.modules(), &mut mock, &mut NoTrace, &InitOptions::default());
    assert!(!rep.ok());
    assert_eq!(mock.calls.len(), 2, "stopped after the failing constructor");
    assert_eq!(rep.ran, 1);
    assert_eq!(rep.per_module, [("libdep.so".to_string(), 1)]);
    let f = &rep.failures[0];
    assert_eq!((f.module.as_str(), f.kind, f.entry), ("libdep.so", CtorKind::InitArray(0), 0x1000));
    assert_eq!(f.failure, CallFailure::Watchdog { pc: 0x1008 });
    assert_eq!(f.to_string(), "libdep.so InitArray(0) @ 0x1000: watchdog expired at pc 0x1008");

    let mut mock = Mock { calls: vec![], fail_at: Some(0x1000), fault: true };
    let opts = InitOptions { keep_going: true, ..Default::default() };
    let rep = run_constructors(l.modules(), &mut mock, &mut NoTrace, &opts);
    assert_eq!(mock.calls.len(), 4);
    assert_eq!((rep.ran, rep.failures.len()), (3, 1));
    assert_eq!(rep.per_module[0], ("libdep.so".to_string(), 2));
    assert_eq!(rep.failures[0].failure.to_string(), "fault at pc 0x1004: udf");
}

#[test]
fn default_options_are_sane() {
    let o = InitOptions::default();
    assert_eq!((o.args, o.keep_going), ([0; 3], false));
    assert!(o.watchdog_cycles > 0);
    assert!(run_constructors(&[], &mut Mock { calls: vec![], fail_at: None, fault: false }, &mut NoTrace, &o).ok());
}

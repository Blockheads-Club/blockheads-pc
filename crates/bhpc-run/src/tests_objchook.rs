//! Host ObjC hooks against a fake Objective-C runtime: the runtime functions are host thunks over method structures
//! in guest memory, `objc_msgSend` looks the IMP up and tail-calls it, and the original implementations are small
//! ARM functions, so every hooked call goes through real guest code, real thunk stubs and real nested calls.

use crate::new_env;
use crate::objchook::*;
use bhpc_cpu::{CallResult, Cpu, Stop};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

const CODE: u32 = 0x2000_0000;
const DATA: u32 = 0x2000_8000;
const BX_LR: u32 = 0xE12F_FF1E;
const UDF: u32 = 0xE7F0_00F0;

#[derive(Default)]
struct Model {
    classes: HashMap<String, (u32, u32)>,
    sels: HashMap<String, u32>,
    methods: HashMap<(u32, u32), u32>,
}

struct Fake {
    cpu: Box<Cpu>,
    _st: bhpc_bionic::Shared,
    rt: Runtime,
    model: Arc<Mutex<Model>>,
    code: u32,
    data: u32,
}

impl Fake {
    fn new() -> Fake {
        let (cpu, st) = new_env(Box::new(std::io::sink()), None).unwrap();
        cpu.mem().map(CODE, 0x10000).unwrap();
        let model = Arc::new(Mutex::new(Model::default()));
        let reg = |name: &str, f: Arc<dyn Fn(&mut Cpu) + Send + Sync>| cpu.register_shared_thunk(name, f).unwrap();
        let class_fn = |meta: bool| {
            let m = model.clone();
            Arc::new(move |c: &mut Cpu| {
                let name = c.mem().read_cstr(c.reg(0)).unwrap();
                let found = m.lock().unwrap().classes.get(&name).map_or(0, |(cl, me)| if meta { *me } else { *cl });
                c.set_reg(0, found);
            }) as Arc<dyn Fn(&mut Cpu) + Send + Sync>
        };
        let m = model.clone();
        let sel_register = Arc::new(move |c: &mut Cpu| {
            let name = c.mem().read_cstr(c.reg(0)).unwrap();
            let mut g = m.lock().unwrap();
            let n = g.sels.len() as u32;
            let s = *g.sels.entry(name).or_insert(0x5000_0000 + 4 * n);
            c.set_reg(0, s);
        });
        let m = model.clone();
        let get_method = Arc::new(move |c: &mut Cpu| {
            let found = m.lock().unwrap().methods.get(&(c.reg(0), c.reg(1))).copied().unwrap_or(0);
            c.set_reg(0, found);
        });
        let types = Arc::new(|c: &mut Cpu| {
            let t = c.mem().read_u32(c.reg(0) + 4).unwrap();
            c.set_reg(0, t);
        });
        let get_imp = Arc::new(|c: &mut Cpu| {
            let t = c.mem().read_u32(c.reg(0) + 8).unwrap();
            c.set_reg(0, t);
        });
        let set_imp = Arc::new(|c: &mut Cpu| {
            let (m, imp) = (c.reg(0), c.reg(1));
            let old = c.mem().read_u32(m + 8).unwrap();
            c.mem().write_u32(m + 8, imp).unwrap();
            c.set_reg(0, old);
        });
        let m = model.clone();
        let msg_send = Arc::new(move |c: &mut Cpu| {
            let isa = c.mem().read_u32(c.reg(0)).unwrap();
            let method = m.lock().unwrap().methods.get(&(isa, c.reg(1))).copied();
            match method {
                Some(at) => {
                    let imp = c.mem().read_u32(at + 8).unwrap();
                    c.tail_call(imp);
                }
                None => c.request_stop(Stop::Host("unrecognized selector".into())),
            }
        });
        let rt = Runtime {
            get_class: reg("fake:objc_getClass", class_fn(false)),
            get_meta_class: reg("fake:objc_getMetaClass", class_fn(true)),
            sel_register: reg("fake:sel_registerName", sel_register),
            get_instance_method: reg("fake:class_getInstanceMethod", get_method),
            type_encoding: reg("fake:method_getTypeEncoding", types),
            get_imp: reg("fake:method_getImplementation", get_imp),
            set_imp: reg("fake:method_setImplementation", set_imp),
            msg_send: reg("fake:objc_msgSend", msg_send),
        };
        Fake { cpu, _st: st, rt, model, code: CODE, data: DATA }
    }

    fn code(&mut self, words: &[u32]) -> u32 {
        let at = self.code;
        for (i, w) in words.iter().enumerate() {
            self.cpu.mem().write_u32(at + 4 * i as u32, *w).unwrap();
        }
        self.code += 4 * words.len() as u32 + 4;
        at
    }

    fn alloc(&mut self, bytes: &[u8]) -> u32 {
        let at = self.data;
        self.cpu.mem().write_bytes(at, bytes).unwrap();
        self.data += (bytes.len() as u32 + 7) & !7;
        at
    }

    fn cstr(&mut self, s: &str) -> u32 {
        let mut b = s.as_bytes().to_vec();
        b.push(0);
        self.alloc(&b)
    }

    /// A class object (whose isa is its metaclass) and its metaclass.
    fn class(&mut self, name: &str) -> u32 {
        let meta = self.alloc(&[0; 8]);
        let class = self.alloc(&meta.to_le_bytes());
        self.model.lock().unwrap().classes.insert(name.into(), (class, meta));
        class
    }

    fn meta(&self, class: u32) -> u32 {
        self.cpu.mem().read_u32(class).unwrap()
    }

    fn sel(&mut self, name: &str) -> u32 {
        self.rt.sel(&mut self.cpu, name).unwrap()
    }

    fn method(&mut self, class: u32, sel: &str, types: &str, imp: u32) -> u32 {
        let s = self.sel(sel);
        let t = self.cstr(types);
        let mut b = s.to_le_bytes().to_vec();
        b.extend(t.to_le_bytes());
        b.extend(imp.to_le_bytes());
        let m = self.alloc(&b);
        self.model.lock().unwrap().methods.insert((class, s), m);
        m
    }

    fn object(&mut self, class: u32) -> u32 {
        self.alloc(&class.to_le_bytes())
    }

    fn send(&mut self, obj: u32, sel: &str, args: &[u32]) -> Result<CallResult, Stop> {
        let s = self.sel(sel);
        let rt = self.rt;
        rt.send(&mut self.cpu, obj, s, args)
    }

    fn imp(&self, method: u32) -> u32 {
        self.cpu.mem().read_u32(method + 8).unwrap()
    }
}

/// `add r0, r2, r3; bx lr`: `-[Calc add:to:]`.
const ADD: [u32; 2] = [0xE082_0003, BX_LR];
/// r2 + r3 + [sp] + [sp, #4]: `-[Calc sum:b:c:d:]`.
const SUM4: [u32; 6] = [0xE59D_C000, 0xE082_000C, 0xE59D_C004, 0xE080_000C, 0xE080_0003, BX_LR];
/// The double in r2:r3 back in r0:r1: `-[Calc same:]`.
const SAME_D: [u32; 3] = [0xE1A0_0002, 0xE1A0_1003, BX_LR];
/// `{R=iii}` through the result pointer r0: (r3, r3 + 1, r3 + 2).
const TRIPLE: [u32; 6] = [0xE580_3000, 0xE283_C001, 0xE580_C004, 0xE283_C002, 0xE580_C008, BX_LR];

/// A class `Calc` with `add:to:` and an instance.
fn calc() -> (Fake, u32, u32) {
    let mut f = Fake::new();
    let class = f.class("Calc");
    let imp = f.code(&ADD);
    let m = f.method(class, "add:to:", "i12@0:4i8i12", imp);
    let obj = f.object(class);
    (f, obj, m)
}

fn add_target() -> Target {
    Target::parse("-[Calc add:to:]").unwrap()
}

#[test]
fn before_hook_sees_arguments_and_tail_calls_the_original() {
    let (mut f, obj, m) = calc();
    assert_eq!(f.send(obj, "add:to:", &[2, 3]).unwrap().r0, 5);
    let hooks = ObjcHooks::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    hooks.hook(add_target(), Some(Sig::parse("i@:ii").unwrap()), move |inv| {
        let (recv, sel, orig) = (inv.receiver(), inv.selector(), inv.original());
        let pc = inv.cpu().pc();
        s.lock().unwrap().push((recv, sel, orig, inv.arg(0), inv.arg(1), inv.arg(2), pc));
    });
    assert_eq!(hooks.waiting(), 1);
    let rt = f.rt;
    let r = hooks.install(&mut f.cpu, &rt);
    assert_eq!(r.installed, vec!["-[Calc add:to:]".to_string()]);
    assert!(r.waiting.is_empty() && r.failed.is_empty());
    assert_eq!(hooks.waiting(), 0);
    let orig = CODE;
    assert_ne!(f.imp(m), orig);
    assert_eq!(f.send(obj, "add:to:", &[2, 3]).unwrap().r0, 5);
    let sel = f.sel("add:to:");
    let got = seen.lock().unwrap()[0].clone();
    assert_eq!((got.0, got.1, got.2), (obj, sel, orig));
    assert_eq!((got.3, got.4, got.5), (Some(Val::Word(2)), Some(Val::Word(3)), None));
    // Installing again changes nothing.
    assert_eq!(hooks.install(&mut f.cpu, &rt), InstallReport::default());
    let st = &hooks.stats()[0];
    assert_eq!((st.name.as_str(), st.state, st.calls, st.panics), ("-[Calc add:to:]", "installed", 1, 0));
}

#[test]
fn before_hook_changes_register_and_stack_arguments() {
    let mut f = Fake::new();
    let class = f.class("Calc");
    let imp = f.code(&SUM4);
    f.method(class, "sum:b:c:d:", "i@:iiii", imp);
    let obj = f.object(class);
    let hooks = ObjcHooks::new();
    hooks.hook(Target::parse("-[Calc sum:b:c:d:]").unwrap(), None, |inv| {
        assert_eq!(inv.arg(3), Some(Val::Word(4)));
        inv.set_arg(0, Val::Word(10)).unwrap();
        inv.set_arg(3, Val::Word(40)).unwrap();
        assert_eq!(inv.set_arg(4, Val::Word(0)), Err(HookError::NoArg(4)));
        assert_eq!(inv.set_arg(1, Val::F32(1.0)), Err(HookError::Type));
    });
    let rt = f.rt;
    hooks.install(&mut f.cpu, &rt);
    assert_eq!(f.send(obj, "sum:b:c:d:", &[1, 2, 3, 4]).unwrap().r0, 10 + 2 + 3 + 40);
}

#[test]
fn after_hook_calls_the_original_and_changes_its_result() {
    let (mut f, obj, _) = calc();
    let hooks = ObjcHooks::new();
    hooks.hook(add_target(), None, |inv| {
        let r = inv.call_original().unwrap();
        assert_eq!(r, Val::Word(5));
        inv.set_return(Val::Word(r.word() * 100)).unwrap();
        assert_eq!(inv.set_return(Val::F64(1.0)), Err(HookError::Type));
    });
    let rt = f.rt;
    hooks.install(&mut f.cpu, &rt);
    assert_eq!(f.send(obj, "add:to:", &[2, 3]).unwrap().r0, 500);
}

#[test]
fn after_hook_that_keeps_the_result_and_changed_arguments() {
    let (mut f, obj, _) = calc();
    let hooks = ObjcHooks::new();
    hooks.hook(add_target(), None, |inv| {
        inv.set_arg(0, Val::Word(7)).unwrap();
        assert_eq!(inv.call_original(), Ok(Val::Word(10)));
    });
    let rt = f.rt;
    hooks.install(&mut f.cpu, &rt);
    assert_eq!(f.send(obj, "add:to:", &[2, 3]).unwrap().r0, 10);
}

#[test]
fn replace_hook_never_runs_the_original() {
    let mut f = Fake::new();
    let class = f.class("Calc");
    let imp = f.code(&[UDF]);
    f.method(class, "boom", "i@:", imp);
    let obj = f.object(class);
    let hooks = ObjcHooks::new();
    hooks.hook(Target::parse("-[Calc boom]").unwrap(), None, |inv| inv.set_return(Val::Word(42)).unwrap());
    let rt = f.rt;
    hooks.install(&mut f.cpu, &rt);
    assert_eq!(f.send(obj, "boom", &[]).unwrap().r0, 42);
}

#[test]
fn doubles_and_struct_results() {
    let mut f = Fake::new();
    let class = f.class("Calc");
    let same = f.code(&SAME_D);
    f.method(class, "same:", "d@:d", same);
    let triple = f.code(&TRIPLE);
    f.method(class, "triple:", "{R=iii}@:i", triple);
    let obj = f.object(class);
    let hooks = ObjcHooks::new();
    hooks.hook(Target::parse("-[Calc same:]").unwrap(), None, |inv| {
        assert_eq!(inv.arg(0), Some(Val::F64(1.5)));
        assert_eq!(inv.call_original(), Ok(Val::F64(1.5)));
        inv.set_return(Val::F64(3.0)).unwrap();
    });
    let calls = Arc::new(AtomicU32::new(0));
    let n = calls.clone();
    hooks.hook(Target::parse("-[Calc triple:]").unwrap(), None, move |inv| {
        assert_eq!(inv.arg(0), Some(Val::Word(5)));
        if n.fetch_add(1, Ordering::SeqCst) == 0 {
            assert_eq!(inv.call_original(), Ok(Val::Words(vec![5, 6, 7])));
        } else {
            inv.set_return(Val::Words(vec![1, 2, 3])).unwrap();
        }
    });
    let rt = f.rt;
    assert_eq!(hooks.install(&mut f.cpu, &rt).installed.len(), 2);
    let r = f.send(obj, "same:", &[1.5f64.to_bits() as u32, (1.5f64.to_bits() >> 32) as u32]).unwrap();
    assert_eq!(r.as_f64(), 3.0);
    let out = f.alloc(&[0; 12]);
    let s = f.sel("triple:");
    let read = |f: &Fake| (0..3).map(|i| f.cpu.mem().read_u32(out + 4 * i).unwrap()).collect::<Vec<_>>();
    // Struct results: the hidden result pointer comes before self (the game calls objc_msgSend_stret, which the
    // fake runtime does not have, so the test calls the hooked IMP directly).
    let m = *f.model.lock().unwrap().methods.get(&(class, s)).unwrap();
    let imp = f.imp(m);
    assert_eq!(f.cpu.call(imp, &[out, obj, s, 5]).unwrap().r0, out);
    assert_eq!(read(&f), vec![5, 6, 7]);
    assert_eq!(f.cpu.call(imp, &[out, obj, s, 5]).unwrap().r0, out);
    assert_eq!(read(&f), vec![1, 2, 3]);
}

#[test]
fn class_methods_hook_the_metaclass() {
    let mut f = Fake::new();
    let class = f.class("Calc");
    let meta = f.meta(class);
    let imp = f.code(&ADD);
    f.method(meta, "add:to:", "i@:ii", imp);
    let hooks = ObjcHooks::new();
    hooks.hook(Target::parse("+[Calc add:to:]").unwrap(), None, |inv| inv.set_arg(1, Val::Word(30)).unwrap());
    let rt = f.rt;
    hooks.install(&mut f.cpu, &rt);
    assert_eq!(f.send(class, "add:to:", &[2, 3]).unwrap().r0, 32);
}

#[test]
fn panics_fall_back_to_the_original() {
    let (mut f, obj, _) = calc();
    let hooks = ObjcHooks::new();
    let n = Arc::new(AtomicU32::new(0));
    let k = n.clone();
    hooks.hook(add_target(), None, move |inv| {
        inv.set_arg(0, Val::Word(1000)).unwrap();
        match k.fetch_add(1, Ordering::SeqCst) {
            // Panics 1, 2 and 4 are logged (powers of two), with each kind of payload.
            0 => panic!("plain"),
            1 => panic!("formatted {}", 1),
            2 => {
                inv.call_original().unwrap();
                inv.set_return(Val::Word(0)).unwrap();
                panic!("after the original");
            }
            _ => std::panic::panic_any(7u8),
        }
    });
    let rt = f.rt;
    hooks.install(&mut f.cpu, &rt);
    // The changed argument is dropped: the original sees what the caller passed.
    assert_eq!(f.send(obj, "add:to:", &[2, 3]).unwrap().r0, 5);
    assert_eq!(f.send(obj, "add:to:", &[2, 3]).unwrap().r0, 5);
    // A panic after the original returns the original's result (with the handler's arguments), not the handler's.
    assert_eq!(f.send(obj, "add:to:", &[2, 3]).unwrap().r0, 1003);
    assert_eq!(f.send(obj, "add:to:", &[2, 3]).unwrap().r0, 5);
    assert_eq!(hooks.stats()[0].panics, 4);
}

#[test]
fn disabled_hooks_pass_through() {
    let (mut f, obj, _) = calc();
    let hooks = ObjcHooks::new();
    let id = hooks.hook(add_target(), None, |inv| inv.set_return(Val::Word(0)).unwrap());
    let rt = f.rt;
    hooks.install(&mut f.cpu, &rt);
    hooks.set_enabled(id, false);
    assert_eq!(f.send(obj, "add:to:", &[2, 3]).unwrap().r0, 5);
    hooks.set_enabled(id, true);
    assert_eq!(f.send(obj, "add:to:", &[2, 3]).unwrap().r0, 0);
    let other = ObjcHooks::new();
    other.set_enabled(id, false);
}

#[test]
fn a_guest_stop_in_the_original_stops_the_caller() {
    let mut f = Fake::new();
    let class = f.class("Calc");
    let imp = f.code(&[UDF]);
    f.method(class, "boom", "v@:", imp);
    let obj = f.object(class);
    let hooks = ObjcHooks::new();
    hooks.hook(Target::parse("-[Calc boom]").unwrap(), None, |inv| {
        assert!(matches!(inv.call_original(), Err(HookError::Guest(Stop::Undefined { .. }))));
    });
    let rt = f.rt;
    hooks.install(&mut f.cpu, &rt);
    assert!(matches!(f.send(obj, "boom", &[]), Err(Stop::Undefined { .. })));
}

#[test]
fn too_deep_nesting_falls_back_to_a_before_hook() {
    let mut f = Fake::new();
    let class = f.class("Calc");
    // rec:n = n == 0 ? 0 : [self rec:n - 1] + 1, through objc_msgSend.
    let send = f.rt.msg_send;
    let rec = f.code(&[
        0xE92D_4010, // push {r4, lr}
        0xE352_0000, // cmp r2, #0
        0x03A0_0000, // moveq r0, #0
        0x08BD_8010, // popeq {r4, pc}
        0xE242_2001, // sub r2, r2, #1
        0xE59F_4008, // ldr r4, [pc, #8]
        0xE12F_FF34, // blx r4
        0xE280_0001, // add r0, r0, #1
        0xE8BD_8010, // pop {r4, pc}
        send,
    ]);
    f.method(class, "rec:", "i@:i", rec);
    let obj = f.object(class);
    let hooks = ObjcHooks::new();
    let deep = Arc::new(AtomicU32::new(0));
    let d = deep.clone();
    hooks.hook(Target::parse("-[Calc rec:]").unwrap(), None, move |inv| {
        if inv.call_original() == Err(HookError::TooDeep) {
            d.fetch_add(1, Ordering::SeqCst);
        }
    });
    let rt = f.rt;
    hooks.install(&mut f.cpu, &rt);
    assert_eq!(f.send(obj, "rec:", &[24]).unwrap().r0, 24);
    assert!(deep.load(Ordering::SeqCst) > 0);
}

#[test]
fn waiting_until_the_class_and_method_exist() {
    let mut f = Fake::new();
    let hooks = ObjcHooks::new();
    hooks.hook(add_target(), None, |inv| inv.set_return(Val::Word(9)).unwrap());
    let rt = f.rt;
    let r = hooks.install(&mut f.cpu, &rt);
    assert_eq!(r.waiting, vec![("-[Calc add:to:]".to_string(), "no class Calc".to_string())]);
    assert_eq!(hooks.stats()[0].note, "no class Calc");
    let class = f.class("Calc");
    let r = hooks.install(&mut f.cpu, &rt);
    assert_eq!(r.waiting[0].1, "no method -[Calc add:to:]");
    let imp = f.code(&ADD);
    f.method(class, "add:to:", "i@:ii", imp);
    assert_eq!(hooks.install(&mut f.cpu, &rt).installed.len(), 1);
    assert_eq!(hooks.stats()[0].note, "");
    let obj = f.object(class);
    assert_eq!(f.send(obj, "add:to:", &[2, 3]).unwrap().r0, 9);
}

#[test]
fn signatures_are_checked_against_the_runtime() {
    let mut f = Fake::new();
    let class = f.class("Calc");
    let imp = f.code(&ADD);
    f.method(class, "add:to:", "i@:ii", imp);
    f.method(class, "odd:", "i@:b4", imp);
    f.method(class, "odd2:", "i@:b4", imp);
    // Methods whose type encoding pointer cannot be read.
    for sel in ["nul:", "nul2:"] {
        let s = f.sel(sel);
        let mut b = s.to_le_bytes().to_vec();
        b.extend([0; 4]);
        b.extend(imp.to_le_bytes());
        let m = f.alloc(&b);
        f.model.lock().unwrap().methods.insert((class, s), m);
    }
    let hooks = ObjcHooks::new();
    hooks.hook(add_target(), Some(Sig::parse("i@:id").unwrap()), |_| {});
    hooks.hook(Target::parse("-[Calc nul:]").unwrap(), None, |_| {});
    hooks.hook(Target::parse("-[Calc nul2:]").unwrap(), Some(Sig::parse("i@:i").unwrap()), |_| {});
    hooks.hook(Target::parse("-[Calc odd:]").unwrap(), None, |_| {});
    // An explicit signature is used when the runtime's cannot be read.
    hooks.hook(Target::parse("-[Calc odd2:]").unwrap(), Some(Sig { ret: Ty::Word, args: vec![Ty::Word] }), |_| {});
    let rt = f.rt;
    let r = hooks.install(&mut f.cpu, &rt);
    assert_eq!(r.failed.len(), 3, "{r:?}");
    assert!(r.failed[0].1.contains("does not match"));
    assert!(r.failed[1].1.starts_with("no usable signature"));
    assert!(r.failed[2].1.starts_with("no usable signature"));
    assert_eq!(r.installed, vec!["-[Calc nul2:]".to_string(), "-[Calc odd2:]".to_string()]);
    assert_eq!(hooks.stats()[0].state, "failed");
}

#[test]
fn a_racing_swizzle_becomes_the_original() {
    let (mut f, obj, m) = calc();
    let other = f.code(&[0xE3A0_0063, BX_LR]); // mov r0, #99
                                               // method_setImplementation reports `other` as the implementation it replaced.
    let set = f
        .cpu
        .register_shared_thunk(
            "fake:racy_set",
            Arc::new(move |c: &mut Cpu| {
                c.mem().write_u32(c.reg(0) + 8, c.reg(1)).unwrap();
                c.set_reg(0, other);
            }),
        )
        .unwrap();
    let rt = Runtime { set_imp: set, ..f.rt };
    let hooks = ObjcHooks::new();
    hooks.hook(add_target(), None, |_| {});
    hooks.install(&mut f.cpu, &rt);
    assert_ne!(f.imp(m), CODE);
    assert_eq!(f.send(obj, "add:to:", &[2, 3]).unwrap().r0, 99);
}

#[test]
fn guest_stops_while_installing_keep_the_hook_waiting() {
    let (mut f, _, _) = calc();
    let udf = f.code(&[UDF]);
    let hooks = ObjcHooks::new();
    hooks.hook(add_target(), None, |_| {});
    let cases = [
        Runtime { get_class: udf, ..f.rt },
        Runtime { sel_register: udf, ..f.rt },
        Runtime { get_instance_method: udf, ..f.rt },
        Runtime { type_encoding: udf, ..f.rt },
        Runtime { get_imp: udf, ..f.rt },
        Runtime { set_imp: udf, ..f.rt },
    ];
    for rt in cases {
        let r = hooks.install(&mut f.cpu, &rt);
        assert!(r.waiting[0].1.starts_with("guest stopped: undefined"), "{r:?}");
    }
    // A string that cannot be written (stack pointer in unmapped memory).
    f.cpu.set_sp(0x1000);
    let rt = f.rt;
    assert!(matches!(rt.sel(&mut f.cpu, "x"), Err(Stop::Host(_))));
    let r = hooks.install(&mut f.cpu, &rt);
    assert!(r.waiting[0].1.contains("cannot write"), "{r:?}");
}

#[test]
fn a_full_thunk_table_fails_the_install() {
    let (mut f, _, _) = calc();
    let mut i = 0;
    while f.cpu.register_shared_thunk(&format!("filler{i}"), Arc::new(|_: &mut Cpu| {})).is_ok() {
        i += 1;
    }
    let hooks = ObjcHooks::new();
    hooks.hook(add_target(), None, |_| {});
    let rt = f.rt;
    let r = hooks.install(&mut f.cpu, &rt);
    assert!(r.waiting[0].1.starts_with("guest stopped: host stop"), "{r:?}");
}

#[test]
fn restore_puts_the_original_back() {
    let (mut f, obj, m) = calc();
    let hooks = ObjcHooks::new();
    let id = hooks.hook(add_target(), None, |inv| inv.set_return(Val::Word(0)).unwrap());
    let waiting = hooks.hook(Target::parse("-[Nope x]").unwrap(), None, |_| {});
    let rt = f.rt;
    hooks.install(&mut f.cpu, &rt);
    assert_eq!(hooks.restore(waiting, &mut f.cpu, &rt), Ok(false));
    assert_eq!(hooks.restore(id, &mut f.cpu, &rt), Ok(true));
    assert_eq!(f.imp(m), CODE);
    assert_eq!(f.send(obj, "add:to:", &[2, 3]).unwrap().r0, 5);
    assert_eq!(hooks.stats()[0].state, "restored");
    // Twice, or an unknown id: nothing to do.
    assert_eq!(hooks.restore(id, &mut f.cpu, &rt), Ok(false));
    assert_eq!(ObjcHooks::new().restore(id, &mut f.cpu, &rt), Ok(false));
}

#[test]
fn restore_keeps_a_later_swizzle() {
    let (mut f, obj, m) = calc();
    let hooks = ObjcHooks::new();
    let id = hooks.hook(add_target(), None, |inv| inv.set_return(Val::Word(0)).unwrap());
    let rt = f.rt;
    hooks.install(&mut f.cpu, &rt);
    let thunk = f.imp(m);
    // Someone else swizzles on top of the hook and chains to it.
    let later = f.code(&[0xE59F_C000, 0xE12F_FF1C, thunk]); // ldr r12, [pc]; bx r12
    f.cpu.mem().write_u32(m + 8, later).unwrap();
    assert_eq!(hooks.restore(id, &mut f.cpu, &rt), Ok(false));
    assert_eq!(f.imp(m), later);
    // The hook is disabled, so the chain reaches the original.
    assert_eq!(f.send(obj, "add:to:", &[2, 3]).unwrap().r0, 5);
    let udf = f.code(&[UDF]);
    f.cpu.mem().write_u32(m + 8, thunk).unwrap();
    let bad = Runtime { get_imp: udf, ..rt };
    assert!(hooks.restore(id, &mut f.cpu, &bad).is_err());
    let bad = Runtime { set_imp: udf, ..rt };
    assert!(hooks.restore(id, &mut f.cpu, &bad).is_err());
}

#[test]
fn runtime_resolve_send_and_sel() {
    let names = [
        "objc_getClass",
        "objc_getMetaClass",
        "sel_registerName",
        "class_getInstanceMethod",
        "method_getTypeEncoding",
        "method_getImplementation",
        "method_setImplementation",
        "objc_msgSend",
    ];
    let at = |n: &str| names.iter().position(|x| *x == n).map(|i| i as u32 * 4);
    let rt = Runtime::resolve(at).unwrap();
    assert_eq!((rt.get_class, rt.msg_send), (0, 28));
    assert_eq!(Runtime::resolve(|n| at(n).filter(|_| n != "objc_msgSend")), Err("no objc_msgSend export".into()));
    let (mut f, obj, _) = calc();
    let a = f.sel("add:to:");
    assert_eq!(f.sel("add:to:"), a);
    let rt = f.rt;
    assert_eq!(rt.send(&mut f.cpu, obj, a, &[4, 5]).unwrap().r0, 9);
}

#[test]
fn target_parse_and_display() {
    let t = Target::parse("+[UIScreen mainScreen]").unwrap();
    assert_eq!((t.class.as_str(), t.sel.as_str(), t.meta), ("UIScreen", "mainScreen", true));
    assert_eq!(t.to_string(), "+[UIScreen mainScreen]");
    for bad in ["", "[A b]", "-A b]", "-[A b", "-[Ab]", "-[ b]", "-[A ]", "-[A b c]", "-"] {
        assert_eq!(Target::parse(bad), None, "{bad}");
    }
}

#[test]
fn val_conversions() {
    assert_eq!(Val::F32(1.0).word(), 0);
    assert_eq!(Val::Word(3).word(), 3);
    let sig = Sig::parse("v@:fq{P=ff}").unwrap();
    assert_eq!(sig.args, vec![Ty::F32, Ty::Dword, Ty::Struct { words: 2, align8: false }]);
    // f, then q aligned to r4:r5 (stack words 0-1), then the struct on the stack.
    let mut f = Fake::new();
    let class = f.class("V");
    let imp = f.code(&[BX_LR]);
    f.method(class, "f:q:p:", "v@:fq{P=ff}", imp);
    let obj = f.object(class);
    let hooks = ObjcHooks::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    hooks.hook(Target::parse("-[V f:q:p:]").unwrap(), None, move |inv| {
        s.lock().unwrap().extend((0..3).map(|i| inv.arg(i).unwrap()));
        assert_eq!(inv.call_original(), Ok(Val::Void));
        assert_eq!(inv.set_return(Val::Void), Ok(()));
        inv.set_arg(0, Val::F32(2.5)).unwrap();
        inv.set_arg(1, Val::Dword(5)).unwrap();
        assert_eq!(inv.set_return(Val::Dword(1)), Err(HookError::Type));
        assert_eq!(inv.set_arg(2, Val::Words(vec![1])), Err(HookError::Type));
    });
    let rt = f.rt;
    hooks.install(&mut f.cpu, &rt);
    let q = 0x1_0000_0002u64;
    f.send(obj, "f:q:p:", &[2.5f32.to_bits(), 0, q as u32, (q >> 32) as u32, 7, 8]).unwrap();
    assert_eq!(*seen.lock().unwrap(), vec![Val::F32(2.5), Val::Dword(q), Val::Words(vec![7, 8])]);
}

#[test]
fn signature_parsing() {
    let p = |s: &str| Sig::parse(s);
    let s = p("v16@0:4@8@\"NSString\"12").unwrap();
    assert_eq!((s.ret, s.args.len()), (Ty::Void, 2));
    let s = p("Vv@:r*^v^?@?^{Opaque}^^{Deep={In=i}}#:").unwrap();
    assert_eq!(s.args, vec![Ty::Word; 8]);
    let s = p("c@:cCsSiIlLB").unwrap();
    assert_eq!((s.ret, s.args), (Ty::Word, vec![Ty::Word; 9]));
    let s = p("d@:qQdDf").unwrap();
    assert_eq!((s.ret, s.args), (Ty::F64, vec![Ty::Dword, Ty::Dword, Ty::F64, Ty::F64, Ty::F32]));
    let s = p("{CGRect={CGPoint=\"x\"f\"y\"f}{CGSize=ff}}@:{S=cid}{A=[3s]}{T=cs}").unwrap();
    assert_eq!(s.ret, Ty::Struct { words: 4, align8: false });
    assert_eq!(
        s.args,
        vec![
            Ty::Struct { words: 4, align8: true },
            Ty::Struct { words: 2, align8: false },
            Ty::Struct { words: 1, align8: false }
        ]
    );
    for bad in [
        "",
        "v@",
        "vf:",
        "v@f",
        "v@:v",
        "v@:{E=}",
        "v@:b4",
        "v@:(U=if)",
        "v@:{Opaque}",
        "v@:@\"Unterminated",
        "v@:^{Open",
        "v@:[4f",
        "v@:{S=i",
    ] {
        assert!(p(bad).is_err(), "{bad}");
    }
}

#[test]
fn struct_layout_alignment_and_small_struct_results() {
    let mut f = Fake::new();
    let class = f.class("V");
    // A one-word struct comes back in r0; the 8-aligned struct starts at r2.
    let imp = f.code(&[0xE1A0_0002, BX_LR]); // mov r0, r2
    f.method(class, "s:", "{W=s}@:{D=d}", imp);
    let obj = f.object(class);
    let hooks = ObjcHooks::new();
    hooks.hook(Target::parse("-[V s:]").unwrap(), None, |inv| {
        assert_eq!(inv.arg(0), Some(Val::Words(vec![11, 12])));
        assert_eq!(inv.call_original(), Ok(Val::Words(vec![11])));
        inv.set_return(Val::Words(vec![13])).unwrap();
    });
    let rt = f.rt;
    hooks.install(&mut f.cpu, &rt);
    assert_eq!(f.send(obj, "s:", &[11, 12]).unwrap().r0, 13);
}

/// Per-call cost of a hooked method (release build: `cargo test --release -p bhpc-run --lib objchook_overhead --
/// --ignored --nocapture`). A guest loop calls the implementation directly, as `objc_msgSend` would jump to it.
#[test]
#[ignore]
fn objchook_overhead() {
    let n: u32 = 2_000_000;
    let lp = |f: &mut Fake, target: u32| {
        f.code(&[
            0xE92D_4070, // push {r4, r5, r6, lr}
            0xE1A0_4000, // mov r4, r0
            0xE59F_501C, // ldr r5, [pc, #28]
            0xE1A0_0004, // loop: mov r0, r4
            0xE3A0_1000, // mov r1, #0
            0xE3A0_2001, // mov r2, #1
            0xE3A0_3002, // mov r3, #2
            0xE12F_FF35, // blx r5
            0xE254_4001, // subs r4, r4, #1
            0x1AFF_FFF8, // bne loop
            0xE8BD_8070, // pop {r4, r5, r6, pc}
            target,
        ])
    };
    let time = |f: &mut Fake, target: u32| {
        let at = lp(f, target);
        f.cpu.call(at, &[1000]).unwrap();
        let t0 = std::time::Instant::now();
        f.cpu.call(at, &[n]).unwrap();
        t0.elapsed().as_nanos() as f64 / f64::from(n)
    };
    let kinds: [(&str, Handler); 5] = [
        ("disabled", Box::new(|_| {})),
        ("before (empty handler)", Box::new(|_| {})),
        (
            "before (read + change an argument)",
            Box::new(|inv| inv.set_arg(0, Val::Word(inv.arg(1).unwrap().word())).unwrap()),
        ),
        (
            "after (call_original, nested)",
            Box::new(|inv| {
                inv.call_original().unwrap();
            }),
        ),
        ("replace (set_return)", Box::new(|inv| inv.set_return(Val::Word(1)).unwrap())),
    ];
    let mut f = Fake::new();
    let base = f.code(&ADD);
    let plain = time(&mut f, base);
    println!("direct IMP call: {plain:.1} ns/call");
    for (i, (name, h)) in kinds.into_iter().enumerate() {
        let (mut f, _, m) = calc();
        let hooks = ObjcHooks::new();
        let id = hooks.hook_boxed(add_target(), None, h);
        let rt = f.rt;
        hooks.install(&mut f.cpu, &rt);
        hooks.set_enabled(id, i != 0);
        let thunk = f.imp(m);
        let t = time(&mut f, thunk);
        println!("{name}: {t:.1} ns/call ({:+.1} ns over the direct call)", t - plain);
    }
}

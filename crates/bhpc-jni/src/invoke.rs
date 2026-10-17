//! Method dispatch: host handlers, guest natives, `Java_*` symbol lookup and AAPCS marshalling.

use crate::value::{ClassId, Obj, Sig, Ty, Value};
use crate::vm::{Call, Imp, JResult, RefKind, Throw, Vm};
use bhpc_cpu::Cpu;

/// Who a call is made on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recv {
    Static(ClassId),
    Instance(Obj),
}

/// JNI short-name mangling of one name component (`_` -> `_1`, `;` -> `_2`, `[` -> `_3`,
/// `/` -> `_`, non-alphanumerics -> `_0xxxx`).
pub fn mangle(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '/' | '.' => out.push('_'),
            '_' => out.push_str("_1"),
            ';' => out.push_str("_2"),
            '[' => out.push_str("_3"),
            c if c.is_ascii_alphanumeric() => out.push(c),
            c => {
                let mut b = [0u16; 2];
                for u in c.encode_utf16(&mut b) {
                    out.push_str(&format!("_0{u:04x}"));
                }
            }
        }
    }
    out
}

/// The two candidate export names of a native method: short and overloaded.
pub fn native_symbols(class: &str, name: &str, sig: &str) -> [String; 2] {
    let short = format!("Java_{}_{}", mangle(class), mangle(name));
    let args = sig.strip_prefix('(').and_then(|s| s.split_once(')')).map_or("", |(a, _)| a);
    let long = format!("{short}__{}", mangle(args));
    [short, long]
}

/// Words of an AAPCS call: `r0 = env`, `r1 = this/class`, then the arguments, 64-bit values
/// 8-byte aligned (an even word index, which also holds for the stack part).
pub fn marshal(env: u32, recv: u32, sig: &Sig, args: &[Value], handle: &mut dyn FnMut(Option<Obj>) -> u32) -> Vec<u32> {
    let mut w = vec![env, recv];
    for (ty, v) in sig.params.iter().zip(args) {
        match ty {
            Ty::Long | Ty::Double => {
                if w.len() % 2 == 1 {
                    w.push(0);
                }
                let bits = match v {
                    Value::Double(d) => d.to_bits(),
                    other => other.as_long() as u64,
                };
                w.push(bits as u32);
                w.push((bits >> 32) as u32);
            }
            Ty::Float => w.push(v.as_float().to_bits()),
            Ty::Ref => w.push(handle(v.as_ref())),
            _ => w.push(v.as_int() as u32),
        }
    }
    w
}

impl Vm {
    /// Invoke method `mid`. `virtual_` re-resolves the method on the receiver's class first.
    pub fn invoke(&self, cpu: &mut Cpu, mid: u32, recv: Recv, args: &[Value], virtual_: bool) -> JResult {
        let (mut mid, class, this) = match recv {
            Recv::Static(c) => (mid, c, Obj::NULL),
            Recv::Instance(o) => {
                let Some(c) = self.class_of(o) else {
                    return Err(self.throw_new("java/lang/NullPointerException", "null receiver"));
                };
                (mid, c, o)
            }
        };
        if virtual_ && this != Obj::NULL {
            let st = self.st();
            if let Some(m) = st.methods.get(mid as usize) {
                if let Some(over) = st.method_in_chain(class, &m.name, &m.sig_str) {
                    mid = over;
                }
            }
        }
        let (imp, sig, cname, name, sigs) = {
            let mut st = self.st();
            let Some(m) = st.methods.get_mut(mid as usize).filter(|_| mid != 0) else {
                drop(st);
                return Err(self.throw_new("java/lang/NoSuchMethodError", "bad method id"));
            };
            m.calls += 1;
            let (imp, sig, name, sigs, mc) = (m.imp.clone(), m.sig.clone(), m.name.clone(), m.sig_str.clone(), m.class);
            (imp, sig, st.classes[mc.0 as usize].name.clone(), name, sigs)
        };
        if self.st().trace_calls {
            self.log(&format!("call {cname}.{name}{sigs}"));
        }
        let imp = match imp {
            Imp::Stub => self.bind_symbol(mid, &cname, &name, &sigs).unwrap_or(Imp::Stub),
            other => other,
        };
        match imp {
            Imp::Host(f) => f(&mut Call { vm: self, cpu, this, class, args }),
            Imp::Stub => Ok(sig.ret.default_value()),
            Imp::Native(addr) => self.call_guest(cpu, addr, recv, &sig, args),
        }
    }

    /// Try to bind a native method through the symbol resolver (`Java_*` exports).
    fn bind_symbol(&self, mid: u32, class: &str, name: &str, sig: &str) -> Option<Imp> {
        let resolver = self.st().symbols.clone()?;
        let addr = native_symbols(class, name, sig).iter().find_map(|s| resolver(s))?;
        self.st().methods[mid as usize].imp = Imp::Native(addr);
        Some(Imp::Native(addr))
    }

    fn call_guest(&self, cpu: &mut Cpu, addr: u32, recv: Recv, sig: &Sig, args: &[Value]) -> JResult {
        let pid = cpu.processor_id();
        let env = self.env_ptr(pid);
        self.push_frame(pid);
        let recv_obj = match recv {
            Recv::Static(c) => self.class_object(c),
            Recv::Instance(o) => o,
        };
        let r = self.new_ref(pid, Some(recv_obj), RefKind::Local);
        let words = marshal(env, r, sig, args, &mut |o| self.new_ref(pid, o, RefKind::Local));
        let res = cpu.call(addr, &words);
        let out = match res {
            Err(stop) => {
                self.pop_frame(pid, None);
                return Err(Throw::Stop(stop));
            }
            Ok(r) => match sig.ret {
                Ty::Void => Value::Void,
                Ty::Bool => Value::Int((r.r0 & 0xff != 0) as i32),
                Ty::Byte => Value::Int(r.r0 as i8 as i32),
                Ty::Char => Value::Int(r.r0 as u16 as i32),
                Ty::Short => Value::Int(r.r0 as i16 as i32),
                Ty::Int => Value::Int(r.r0 as i32),
                Ty::Long => Value::Long(r.as_u64() as i64),
                Ty::Float => Value::Float(f32::from_bits(r.r0)),
                Ty::Double => Value::Double(r.as_f64()),
                Ty::Ref => Value::Ref(self.resolve(r.r0)),
            },
        };
        self.pop_frame(pid, None);
        match self.take_pending(pid) {
            Some(e) => Err(Throw::Exc(e)),
            None => Ok(out),
        }
    }

    /// Call an instance method by name (virtual dispatch on the receiver's class).
    pub fn call_method(&self, cpu: &mut Cpu, o: Obj, name: &str, sig: &str, args: &[Value]) -> JResult {
        let Some(c) = self.class_of(o) else {
            return Err(self.throw_new("java/lang/NullPointerException", name));
        };
        let mid = self.method_id(c, name, sig, false)?;
        self.invoke(cpu, mid, Recv::Instance(o), args, true)
    }

    /// Call a static method by class and member name.
    pub fn call_static(&self, cpu: &mut Cpu, class: &str, name: &str, sig: &str, args: &[Value]) -> JResult {
        let c = self.find_class(class)?;
        let mid = self.method_id(c, name, sig, true)?;
        self.invoke(cpu, mid, Recv::Static(c), args, false)
    }

    /// Construct an object: allocate, then run `<init>sig` with `args`.
    pub fn new_object(&self, cpu: &mut Cpu, class: &str, sig: &str, args: &[Value]) -> Result<Obj, Throw> {
        let c = self.find_class(class)?;
        let o = self.alloc_object(c);
        let mid = self.method_id(c, "<init>", sig, false)?;
        self.invoke(cpu, mid, Recv::Instance(o), args, false)?;
        Ok(o)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mangling() {
        assert_eq!(mangle("com/apportable/MainThread"), "com_apportable_MainThread");
        assert_eq!(mangle("a_b;[é"), "a_1b_2_3_000e9");
        let [s, l] = native_symbols("com/apportable/Lifecycle", "contextDidBecomeValid", "()V");
        assert_eq!(s, "Java_com_apportable_Lifecycle_contextDidBecomeValid");
        assert_eq!(l, format!("{s}__"));
        let [_, l] = native_symbols("a/B", "f", "(ILjava/lang/String;[J)V");
        assert_eq!(l, "Java_a_B_f__ILjava_lang_String_2_3J");
        let [_, l] = native_symbols("a/B", "f", "bogus");
        assert_eq!(l, "Java_a_B_f__");
    }

    #[test]
    fn marshalling_aligns_wide_values() {
        let sig =
            Sig { params: vec![Ty::Int, Ty::Long, Ty::Float, Ty::Double, Ty::Ref, Ty::Bool, Ty::Long], ret: Ty::Void };
        let args = [
            Value::Int(-1),
            Value::Long(0x1_0000_0002),
            Value::Float(1.0),
            Value::Double(2.0),
            Value::Ref(Some(Obj(9))),
            Value::Int(1),
            Value::Int(7),
        ];
        let w = marshal(0xE, 0xF, &sig, &args, &mut |o| o.map_or(0, |o| o.0 + 100));
        assert_eq!(w, [0xE, 0xF, 0xFFFF_FFFF, 0, 2, 1, 1.0f32.to_bits(), 0, 0, 0x4000_0000, 109, 1, 7, 0]);
    }
}

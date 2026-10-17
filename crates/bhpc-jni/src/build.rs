//! A small DSL to define fake Java classes.

use crate::value::{parse_field_ty, parse_sig, ClassId, Value};
use crate::vm::{Call, Data, HostFn, Imp, JResult, Vm};
use std::sync::Arc;

/// Defines (or extends) one class. Obtain it from [`Vm::define`].
pub struct ClassBuilder {
    vm: Vm,
    id: ClassId,
}

impl Vm {
    /// Start defining `name` (internal form, `a/b/C`). Re-defining an existing class adds to it.
    pub fn define(&self, name: &str) -> ClassBuilder {
        let mut st = self.st();
        let id = st.ensure_class(name);
        let c = &mut st.classes[id.0 as usize];
        c.defined = true;
        c.stub = false;
        drop(st);
        ClassBuilder { vm: self.clone(), id }
    }
}

impl ClassBuilder {
    pub fn id(&self) -> ClassId {
        self.id
    }

    /// Set the superclass.
    pub fn extends(self, name: &str) -> Self {
        {
            let mut st = self.vm.st();
            let s = st.ensure_class(name);
            st.classes[self.id.0 as usize].sup = Some(s);
        }
        self
    }

    /// Add implemented interfaces.
    pub fn implements(self, names: &[&str]) -> Self {
        {
            let mut st = self.vm.st();
            for n in names {
                let i = st.ensure_class(n);
                st.classes[self.id.0 as usize].ifaces.push(i);
            }
        }
        self
    }

    /// Mark the class as an interface.
    pub fn interface(self) -> Self {
        self.vm.st().classes[self.id.0 as usize].interface = true;
        self
    }

    /// Payload given to new instances (`AllocObject`/`NewObject`).
    pub fn payload(self, init: fn() -> Data) -> Self {
        self.vm.st().classes[self.id.0 as usize].init = Some(init);
        self
    }

    fn add(self, name: &str, sig: &str, is_static: bool, imp: Imp) -> Self {
        let parsed = parse_sig(sig).expect("bad method descriptor");
        self.vm.st().add_method(self.id, name, sig, parsed, is_static, imp);
        self
    }

    /// Instance method implemented by the host.
    pub fn method(self, name: &str, sig: &str, f: impl Fn(&mut Call) -> JResult + Send + Sync + 'static) -> Self {
        let f: HostFn = Arc::new(f);
        self.add(name, sig, false, Imp::Host(f))
    }

    /// Static method implemented by the host.
    pub fn static_method(
        self,
        name: &str,
        sig: &str,
        f: impl Fn(&mut Call) -> JResult + Send + Sync + 'static,
    ) -> Self {
        let f: HostFn = Arc::new(f);
        self.add(name, sig, true, Imp::Host(f))
    }

    /// A native method with no implementation yet (bound later by `RegisterNatives` or a
    /// `Java_*` export): calls before that are reported stubs.
    pub fn native(self, name: &str, sig: &str, is_static: bool) -> Self {
        self.add(name, sig, is_static, Imp::Stub)
    }

    /// Several [`ClassBuilder::native`] declarations: `(name, descriptor, is_static)`.
    pub fn natives(mut self, list: &[(&str, &str, bool)]) -> Self {
        for (name, sig, is_static) in list {
            self = self.native(name, sig, *is_static);
        }
        self
    }

    /// Declare an instance field.
    pub fn field(self, name: &str, desc: &str) -> Self {
        let id = self.id;
        self.vm.st().ensure_field(id, name, desc);
        self
    }

    /// Declare a static field with its value.
    pub fn static_field(self, name: &str, desc: &str, v: Value) -> Self {
        assert!(parse_field_ty(desc).is_some(), "bad field descriptor {desc}");
        let id = self.id;
        {
            let mut st = self.vm.st();
            let f = st.ensure_field(id, name, desc);
            st.fields[f as usize].value = v;
        }
        self
    }
}

//! The fake VM as the runner uses it: built-in Java library, permissive by default.
//!
//! Unknown classes, methods and fields are invented and reported (see [`crate::VmConfig::lenient`]):
//! calls into Java that nothing implements answer 0/NULL, so native code keeps running. [`Jni::usage`]
//! is the work list of what a more complete Java layer has to provide.

use crate::vm::{GuestAlloc, Kind, Vm, VmConfig};
use bhpc_cpu::Cpu;
use bhpc_mem::GuestMemory;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Failure to set up the fake VM.
#[derive(Debug, PartialEq, Eq)]
pub enum JniError {
    /// Building the guest-visible tables failed (guest heap exhausted, thunk table full, ...).
    Install(String),
}

impl std::fmt::Display for JniError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JniError::Install(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for JniError {}

/// What the guest did with the fake VM so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Usage {
    /// Distinct classes the guest looked up or called into (including invented ones).
    pub classes: usize,
    /// Methods looked up or called.
    pub methods: usize,
    /// Fields looked up.
    pub fields: usize,
    /// Methods bound to guest code (`RegisterNatives` or `Java_*` exports).
    pub natives: usize,
    /// `Class.method` -> calls made into Java, most called first.
    pub java_calls: Vec<(String, u64)>,
    /// A guest thread left an exception pending.
    pub exception_pending: bool,
}

/// The installed fake VM.
#[derive(Clone)]
pub struct Jni {
    /// The VM itself (object model, references, reports).
    pub vm: Vm,
    /// Guest address of the `JavaVM*` handed to `JNI_OnLoad`.
    pub java_vm: u32,
}

/// Build the fake VM with the built-in Java library and register its JNI tables on `cpu`.
/// `alloc` hands out guest memory for the VM's own buffers.
pub fn install(cpu: &Cpu, mem: Arc<GuestMemory>, alloc: Box<dyn GuestAlloc>, cfg: VmConfig) -> Result<Jni, JniError> {
    let vm = crate::new_vm(mem, alloc, cfg);
    let layout = vm.install(cpu).map_err(JniError::Install)?;
    Ok(Jni { vm, java_vm: layout.java_vm })
}

impl Jni {
    /// Print invented classes/members and pending exceptions to stderr as they happen.
    pub fn set_trace(&self, on: bool) {
        if on {
            self.vm.set_log(|m| eprintln!("[jni] {m}"));
        } else {
            self.vm.set_log(|_| {});
        }
    }

    /// Also log every call into Java (very verbose; implies [`Jni::set_trace`]).
    pub fn set_trace_calls(&self, on: bool) {
        self.vm.st().trace_calls = on;
    }

    /// Open the local-reference frame of a host-initiated call on guest thread `pid`.
    pub fn enter(&self, pid: usize) {
        self.vm.push_frame(pid);
    }

    /// Close the frame opened by [`Jni::enter`], releasing the local references the call created.
    pub fn leave(&self, pid: usize) {
        self.vm.pop_frame(pid, None);
    }

    /// Summary of the guest's use of the VM.
    pub fn usage(&self) -> Usage {
        let r = self.vm.report();
        let mut classes: BTreeSet<&str> = r.stub_classes.iter().map(String::as_str).collect();
        classes.extend(r.members.iter().map(|m| m.class.as_str()));
        classes.extend(r.fields.iter().map(|f| f.0.as_str()));
        let mut calls: BTreeMap<String, u64> = BTreeMap::new();
        for m in r.members.iter().filter(|m| m.calls > 0) {
            *calls.entry(format!("{}.{}", m.class, m.name)).or_insert(0) += m.calls;
        }
        let mut java_calls: Vec<_> = calls.into_iter().collect();
        java_calls.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        Usage {
            classes: classes.len(),
            methods: r.members.len(),
            fields: r.fields.len(),
            natives: r.members.iter().filter(|m| m.kind == Kind::Native).count(),
            java_calls,
            exception_pending: self.vm.exception_pending(),
        }
    }
}

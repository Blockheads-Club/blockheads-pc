//! The shared thunk table: host functions bound to `svc` slots, usable by many [`Cpu`]s.

use crate::{Cpu, CpuError, MAX_THUNKS, THUNK_BASE, THUNK_REGION, THUNK_STRIDE};
use bhpc_mem::{GuestMemory, MemError};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

/// A stateful host function bound to an `svc` number. It is serialised: only one guest thread
/// runs it at a time, and it cannot be re-entered from a nested guest call it makes itself (that
/// stops the guest). Use [`SharedThunkFn`] for functions that must be re-entrant or parallel.
pub type ThunkFn = Box<dyn FnMut(&mut Cpu) + Send>;

/// A stateless (or internally synchronised) host function: re-entrant and callable in parallel.
pub type SharedThunkFn = Arc<dyn Fn(&mut Cpu) + Send + Sync>;

/// What a thunk slot runs.
#[derive(Clone)]
pub(crate) enum Handler {
    Serial(Arc<Mutex<ThunkFn>>),
    Shared(SharedThunkFn),
}

thread_local! {
    /// Serial handlers this thread is currently inside (to refuse re-entering one).
    static ACTIVE: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

impl Handler {
    /// Run the handler; `Err` means a serial handler was re-entered on this thread.
    pub(crate) fn invoke(&self, cpu: &mut Cpu) -> Result<(), ()> {
        match self {
            Handler::Shared(f) => {
                f(cpu);
                Ok(())
            }
            Handler::Serial(m) => {
                let key = Arc::as_ptr(m) as usize;
                if ACTIVE.with(|a| a.borrow().contains(&key)) {
                    return Err(());
                }
                let mut f = m.lock().unwrap_or_else(PoisonError::into_inner);
                ACTIVE.with(|a| a.borrow_mut().push(key));
                (*f)(cpu);
                ACTIVE.with(|a| a.borrow_mut().pop());
                Ok(())
            }
        }
    }
}

struct Entry {
    name: String,
    handler: Option<Handler>,
}

struct Inner {
    entries: Vec<Entry>,
    by_name: HashMap<String, u32>,
}

/// Table of host thunks living in the reserved guest page at [`THUNK_BASE`]. Wrap it in an `Arc`
/// to share it between several [`Cpu`]s (guest threads, nested-call children).
pub struct ThunkTable {
    mem: Arc<GuestMemory>,
    inner: RwLock<Inner>,
}

impl ThunkTable {
    /// Map the thunk page in `mem` and write the return stub (slot 0). Fails if the page is
    /// already mapped, so build one table per guest memory.
    pub fn new(mem: Arc<GuestMemory>) -> Result<Arc<ThunkTable>, CpuError> {
        mem.map(THUNK_BASE, THUNK_REGION)?;
        let t = ThunkTable {
            mem,
            inner: RwLock::new(Inner {
                entries: vec![Entry { name: "<return>".into(), handler: None }],
                by_name: HashMap::new(),
            }),
        };
        t.write_stub(0)?;
        Ok(Arc::new(t))
    }

    fn write_stub(&self, slot: u32) -> Result<(), MemError> {
        let a = THUNK_BASE + slot * THUNK_STRIDE;
        self.mem.write_u32(a, 0xE1A0_C00E)?; // mov r12, lr
        self.mem.write_u32(a + 4, 0xEF00_0000 | slot)?; // svc #slot
        self.mem.write_u32(a + 8, 0xE12F_FF1C)?; // bx r12
        self.mem.write_u32(a + 12, 0xE7F0_00F0) // udf #0 (never reached)
    }

    fn register(&self, name: &str, h: Handler) -> Result<u32, CpuError> {
        let mut g = self.inner.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(&slot) = g.by_name.get(name) {
            g.entries[slot as usize].handler = Some(h);
            return Ok(THUNK_BASE + slot * THUNK_STRIDE);
        }
        let slot = g.entries.len() as u32;
        if slot > MAX_THUNKS {
            return Err(CpuError::TooManyThunks);
        }
        self.write_stub(slot)?;
        g.entries.push(Entry { name: name.to_string(), handler: Some(h) });
        g.by_name.insert(name.to_string(), slot);
        Ok(THUNK_BASE + slot * THUNK_STRIDE)
    }

    /// Register (or rebind) the serialised thunk `name`; returns its guest address.
    pub fn register_thunk(&self, name: &str, f: ThunkFn) -> Result<u32, CpuError> {
        self.register(name, Handler::Serial(Arc::new(Mutex::new(f))))
    }

    /// Register (or rebind) a re-entrant, thread-safe thunk `name`; returns its guest address.
    pub fn register_shared_thunk(&self, name: &str, f: SharedThunkFn) -> Result<u32, CpuError> {
        self.register(name, Handler::Shared(f))
    }

    /// Guest address of an already registered thunk.
    pub fn thunk_addr(&self, name: &str) -> Option<u32> {
        let g = self.inner.read().unwrap_or_else(PoisonError::into_inner);
        g.by_name.get(name).map(|s| THUNK_BASE + s * THUNK_STRIDE)
    }

    /// Name of the thunk at guest address `addr`, if any.
    pub fn thunk_name(&self, addr: u32) -> Option<String> {
        let off = addr.checked_sub(THUNK_BASE)?;
        if off % THUNK_STRIDE != 0 {
            return None;
        }
        self.name_of_slot((off / THUNK_STRIDE) as usize)
    }

    pub(crate) fn name_of_slot(&self, slot: usize) -> Option<String> {
        let g = self.inner.read().unwrap_or_else(PoisonError::into_inner);
        g.entries.get(slot).map(|e| e.name.clone())
    }

    /// Number of registered thunks (excluding the return stub).
    pub fn thunk_count(&self) -> usize {
        self.inner.read().unwrap_or_else(PoisonError::into_inner).entries.len() - 1
    }

    pub(crate) fn handler(&self, slot: usize) -> Option<Handler> {
        let g = self.inner.read().unwrap_or_else(PoisonError::into_inner);
        g.entries.get(slot).and_then(|e| e.handler.clone())
    }
}

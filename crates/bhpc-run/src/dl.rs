//! `dlopen` / `dlsym` / `dlerror` / `dlclose` for the guest, over a snapshot of the loaded modules.
//!
//! The game `dlopen`s system libraries at run time (`libOpenSLES.so`, `libEGL.so`, ...). Those are "host libs":
//! `dlopen` answers [`HOST_HANDLE`] and `dlsym` asks the host resolver (OpenSL ES, GL, libc thunks).

use bhpc_bionic::guest::{cstr, wr_cstr};
use bhpc_bionic::{lock, Shared};
use bhpc_cpu::Cpu;
use bhpc_loader::dl::{GLOBAL_HANDLE, HOST_HANDLE, RTLD_DEFAULT, RTLD_NEXT};
use bhpc_loader::Linker;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, PoisonError};

/// System libraries the host provides even when no loaded module lists them in `DT_NEEDED`.
pub const SYSTEM_LIBS: &[&str] = &[
    "libc.so",
    "libm.so",
    "libdl.so",
    "liblog.so",
    "libz.so",
    "libstdc++.so",
    "libandroid.so",
    "libEGL.so",
    "libGLESv1_CM.so",
    "libGLESv2.so",
    "libOpenSLES.so",
    "libjnigraphics.so",
];

/// Resolves a symbol for a host library (`dlsym` on [`HOST_HANDLE`] or the global scope).
pub type HostResolver = Box<dyn Fn(&mut Cpu, &str) -> Option<u32> + Send + Sync>;

struct Mod {
    names: Vec<String>,
    exports: HashMap<String, u32>,
    needed: Vec<String>,
}

/// Snapshot of the loader's module table plus the `dlerror` state.
pub struct DlTable {
    mods: Vec<Mod>,
    host_libs: BTreeSet<String>,
    host: HostResolver,
    error: Option<String>,
    /// Guest buffer `dlerror` copies its message to (0 until first used).
    err_buf: u32,
    trace: bool,
}

/// Size of the `dlerror` buffer in the guest.
const ERR_BUF: u32 = 256;

impl DlTable {
    /// Snapshot `linker`'s modules. `host_libs` are the libraries the host provides besides [`SYSTEM_LIBS`].
    pub fn new(linker: &Linker, host_libs: &[String], host: HostResolver) -> DlTable {
        let mods = linker
            .modules()
            .iter()
            .map(|m| Mod {
                names: std::iter::once(m.name.clone()).chain(m.soname.clone()).collect(),
                exports: m.exports.clone(),
                needed: m.needed.clone(),
            })
            .collect();
        let host_libs = host_libs.iter().cloned().chain(SYSTEM_LIBS.iter().map(ToString::to_string)).collect();
        DlTable { mods, host_libs, host, error: None, err_buf: 0, trace: false }
    }

    fn index(&self, name: &str) -> Option<usize> {
        self.mods.iter().position(|m| m.names.iter().any(|n| n == name))
    }

    /// Log every call to stderr (`--trace-fs` equivalent for the dynamic linker).
    pub fn set_trace(&mut self, on: bool) {
        self.trace = on;
    }

    /// `dlopen`: module handle (index + 1), [`HOST_HANDLE`], or 0 with the error set. `None` is the global scope.
    pub fn dlopen(&mut self, name: Option<&str>) -> u32 {
        if self.trace {
            eprintln!("[dl] dlopen({name:?})");
        }
        let Some(name) = name else { return GLOBAL_HANDLE };
        let base = name.rsplit('/').next().unwrap_or(name);
        if let Some(i) = self.index(name).or_else(|| self.index(base)) {
            return i as u32 + 1;
        }
        if self.host_libs.contains(base) {
            return HOST_HANDLE;
        }
        self.error = Some(format!("dlopen failed: library \"{name}\" not found"));
        0
    }

    fn find(&self, root: usize, name: &str) -> Option<u32> {
        let mut queue = vec![root];
        let mut seen = vec![false; self.mods.len()];
        seen[root] = true;
        let mut head = 0;
        while head < queue.len() {
            let m = &self.mods[queue[head]];
            head += 1;
            if let Some(a) = m.exports.get(name) {
                return Some(*a);
            }
            for i in m.needed.iter().filter_map(|n| self.index(n)) {
                if !std::mem::replace(&mut seen[i], true) {
                    queue.push(i);
                }
            }
        }
        None
    }

    /// `dlsym`: symbol address or `None` with the error set.
    pub fn dlsym(&mut self, cpu: &mut Cpu, handle: u32, name: &str) -> Option<u32> {
        if self.trace {
            eprintln!("[dl] dlsym({handle:#x}, {name})");
        }
        let found = match handle {
            RTLD_DEFAULT | GLOBAL_HANDLE => {
                self.mods.iter().find_map(|m| m.exports.get(name).copied()).or_else(|| (self.host)(cpu, name))
            }
            HOST_HANDLE | RTLD_NEXT => (self.host)(cpu, name),
            h => match (h as usize).checked_sub(1).filter(|i| *i < self.mods.len()) {
                Some(i) => self.find(i, name),
                None => {
                    self.error = Some(format!("dlsym failed: invalid handle {h:#x}"));
                    return None;
                }
            },
        };
        if found.is_none() {
            self.error = Some(format!("dlsym failed: undefined symbol: {name}"));
        }
        found
    }

    /// Take the pending error message.
    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }
}

/// Look `name` up among libc's data symbols, thunks already bound and bionic's built-ins (binding them on demand).
/// Unlike [`bhpc_bionic::resolve`] this never creates a trap thunk for an unknown name.
pub fn bionic_symbol(cpu: &mut Cpu, st: &Shared, name: &str) -> Option<u32> {
    let known = bhpc_bionic::DATA_SYMBOLS.contains(&name) || bhpc_bionic::builtin_names().binary_search(&name).is_ok();
    if known || cpu.thunk_addr(name).is_some() {
        return bhpc_bionic::resolve(cpu, st, name);
    }
    None
}

/// Register `dlopen`, `dlsym`, `dlclose` and `dlerror` on `cpu`. Call before the libraries are linked so their
/// imports bind to these thunks, then build the [`DlTable`] and store it in `table`'s slot.
pub fn install(cpu: &Cpu, st: &Shared, table: Arc<Mutex<Option<DlTable>>>) {
    let reg = |name: &str, f: Arc<dyn Fn(&mut Cpu) + Send + Sync>| {
        // The thunk page holds thousands of entries; a full page would already have broken the loader.
        let _ = cpu.register_shared_thunk(name, f);
    };
    let with = |table: &Arc<Mutex<Option<DlTable>>>, c: &mut Cpu, f: &dyn Fn(&mut DlTable, &mut Cpu) -> u32| {
        let r = lock_table_opt(table).as_mut().map_or(0, |t| f(t, c));
        c.ret_u32(r);
    };
    let t = table.clone();
    reg(
        "dlopen",
        Arc::new(move |c: &mut Cpu| {
            let p = c.args().u32(c);
            let name = match p {
                0 => None,
                p => match cstr(c, p) {
                    Some(b) => Some(String::from_utf8_lossy(&b).into_owned()),
                    None => return,
                },
            };
            with(&t, c, &|d, _| d.dlopen(name.as_deref()));
        }),
    );
    let t = table.clone();
    reg(
        "dlsym",
        Arc::new(move |c: &mut Cpu| {
            let mut a = c.args();
            let (h, n) = (a.u32(c), a.u32(c));
            let Some(name) = cstr(c, n) else { return };
            let name = String::from_utf8_lossy(&name).into_owned();
            with(&t, c, &|d, cpu| d.dlsym(cpu, h, &name).unwrap_or(0));
        }),
    );
    reg("dlclose", Arc::new(|c: &mut Cpu| c.ret_u32(0)));
    let (t, st) = (table, st.clone());
    reg(
        "dlerror",
        Arc::new(move |c: &mut Cpu| {
            let (msg, buf) = match lock_table_opt(&t).as_mut() {
                Some(d) => (d.take_error(), d.err_buf),
                None => (None, 0),
            };
            let Some(msg) = msg else { return c.ret_u32(0) };
            let buf = if buf != 0 { buf } else { lock(&st).heap.alloc(ERR_BUF) };
            if let Some(d) = lock_table_opt(&t).as_mut() {
                d.err_buf = buf;
            }
            let bytes = &msg.as_bytes()[..msg.len().min(ERR_BUF as usize - 1)];
            if buf != 0 && wr_cstr(c, buf, bytes) {
                c.ret_u32(buf);
            }
        }),
    );
}

fn lock_table_opt(t: &Mutex<Option<DlTable>>) -> std::sync::MutexGuard<'_, Option<DlTable>> {
    t.lock().unwrap_or_else(PoisonError::into_inner)
}

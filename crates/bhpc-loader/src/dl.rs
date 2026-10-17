//! `dlopen` / `dlsym` / `dladdr` / `dlerror` semantics over the loaded modules.
//!
//! These are plain Rust functions; `bhpc-bionic` wraps them in host thunks (read the C strings, call, write
//! the result to `r0`).

use crate::{HostSymbols, Linker};

/// `RTLD_DEFAULT`: search the global scope.
pub const RTLD_DEFAULT: u32 = 0;
/// `RTLD_NEXT`: search the modules loaded after the caller's.
pub const RTLD_NEXT: u32 = u32::MAX;
/// Handle returned for `dlopen(NULL)`; equivalent to [`RTLD_DEFAULT`].
pub const GLOBAL_HANDLE: u32 = 0xFFFF_FFFD;
/// Handle returned for a library the host provides (not loaded from the APK).
pub const HOST_HANDLE: u32 = 0xFFFF_FFFE;

/// Result of [`Linker::dladdr`] (the `Dl_info` fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DlInfo {
    /// `dli_fname`: load name of the containing module.
    pub module: String,
    /// `dli_fbase`: guest address of the first mapped byte.
    pub base: u32,
    /// `dli_sname` and `dli_saddr` of the nearest preceding symbol, if it covers the address.
    pub symbol: Option<(String, u32)>,
}

impl Linker {
    /// `dlopen`: a handle for an already loaded library (by load name, soname or path basename), the host
    /// handle for a library the host provides, or `0` with [`Linker::dlerror`] set. `None` is `dlopen(NULL)`.
    pub fn dlopen(&mut self, name: Option<&str>) -> u32 {
        let Some(name) = name else { return GLOBAL_HANDLE };
        let base = name.rsplit('/').next().unwrap_or(name);
        if let Some(i) = self.module_index(name).or_else(|| self.module_index(base)) {
            return i as u32 + 1;
        }
        if self.host_libs.contains(base) {
            return HOST_HANDLE;
        }
        self.dl_error = Some(format!("dlopen failed: library \"{name}\" not found"));
        0
    }

    /// `dlsym(handle, name)`; see [`Linker::dlsym_from`] for `RTLD_NEXT`.
    pub fn dlsym(&mut self, handle: u32, name: &str, host: &mut dyn HostSymbols) -> Option<u32> {
        let found = match handle {
            RTLD_DEFAULT | GLOBAL_HANDLE => self.lookup(name).or_else(|| host.resolve(name)),
            HOST_HANDLE => host.resolve(name),
            RTLD_NEXT => None,
            h => match (h as usize).checked_sub(1).filter(|i| *i < self.modules.len()) {
                Some(i) => self.lookup_in_tree(i, name),
                None => {
                    self.dl_error = Some(format!("dlsym failed: invalid handle {h:#x}"));
                    return None;
                }
            },
        };
        self.record_sym_result(found, name)
    }

    /// `dlsym(RTLD_NEXT, name)` called from guest code at `caller_pc`: search modules loaded after the
    /// caller's module.
    pub fn dlsym_from(&mut self, caller_pc: u32, name: &str) -> Option<u32> {
        let found = self
            .modules
            .iter()
            .position(|m| m.contains(caller_pc))
            .and_then(|i| self.modules[i + 1..].iter().find_map(|m| m.export(name)));
        self.record_sym_result(found, name)
    }

    fn record_sym_result(&mut self, found: Option<u32>, name: &str) -> Option<u32> {
        if found.is_none() {
            self.dl_error = Some(format!("dlsym failed: undefined symbol: {name}"));
        }
        found
    }

    /// Breadth-first search of module `root` and its loaded dependencies, like `dlsym` on a handle.
    fn lookup_in_tree(&self, root: usize, name: &str) -> Option<u32> {
        let mut queue = vec![root];
        let mut seen = vec![false; self.modules.len()];
        seen[root] = true;
        let mut head = 0;
        while head < queue.len() {
            let m = &self.modules[queue[head]];
            head += 1;
            if let Some(a) = m.export(name) {
                return Some(a);
            }
            for n in &m.needed {
                if let Some(i) = self.module_index(n) {
                    if !seen[i] {
                        seen[i] = true;
                        queue.push(i);
                    }
                }
            }
        }
        None
    }

    /// `dlerror`: the last failure message, cleared by reading it.
    pub fn dlerror(&mut self) -> Option<String> {
        self.dl_error.take()
    }

    /// `dladdr`: the module containing `addr` and the nearest preceding symbol that covers it.
    pub fn dladdr(&self, addr: u32) -> Option<DlInfo> {
        let m = self.module_at(addr)?;
        let n = m.symbols.partition_point(|s| s.addr & !1 <= addr);
        let symbol = n.checked_sub(1).and_then(|i| {
            // Aliases share an address; report the first one in name order.
            let key = m.symbols[i].addr & !1;
            let first = m.symbols[..=i].partition_point(|s| s.addr & !1 < key);
            let s = &m.symbols[first];
            let covers = s.size == 0 || addr < (s.addr & !1).wrapping_add(s.size);
            covers.then(|| (s.name.clone(), s.addr))
        });
        Some(DlInfo { module: m.name.clone(), base: m.start, symbol })
    }
}

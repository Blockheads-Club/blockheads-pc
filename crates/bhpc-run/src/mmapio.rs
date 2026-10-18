//! Positioned I/O and shared file mappings, which LMDB (the game's database) needs and the bionic file layer does
//! not have: `pread`/`pwrite` (and the `64` variants), and `mmap(MAP_SHARED)` of a file.
//!
//! A shared mapping is a copy of the file bytes in its own guest region. It stays coherent with writes made
//! through `pwrite`/`write` on the same descriptor (LMDB writes pages with `pwrite` and reads them through the
//! map); stores into a writable map are not written back (the only such map is LMDB's lock table, which is
//! process-local). Regions come from the module arena, so a map can be far larger than the guest heap.

use bhpc_bionic::guest::{rd, wr};
use bhpc_bionic::layout::errno;
use bhpc_bionic::misc::fail;
use bhpc_bionic::{lock, Shared};
use bhpc_cpu::Cpu;
use std::sync::{Arc, Mutex, PoisonError};

const MAP_SHARED: u32 = 1;
const MAP_ANONYMOUS: u32 = 0x20;
const PAGE: u32 = 4096;

struct Map {
    addr: u32,
    len: u32,
    fd: u32,
    off: u64,
}

/// Registry of the shared mappings.
#[derive(Clone, Default)]
pub struct SharedMaps(Arc<Mutex<Vec<Map>>>);

fn args4(c: &mut Cpu, wide: bool) -> (u32, u32, u32, u64) {
    let mut a = c.args();
    let (fd, buf, n) = (a.u32(c), a.u32(c), a.u32(c));
    let off = if wide { a.u64(c) } else { u64::from(a.u32(c)) };
    (fd, buf, n, off)
}

impl SharedMaps {
    fn maps(&self) -> std::sync::MutexGuard<'_, Vec<Map>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Copy `data`, written to `fd` at `off`, into every map of that descriptor it overlaps.
    fn refresh(&self, c: &mut Cpu, fd: u32, off: u64, data: &[u8]) {
        for m in self.maps().iter().filter(|m| m.fd == fd) {
            let (start, end) = (off.max(m.off), (off + data.len() as u64).min(m.off + u64::from(m.len)));
            if start < end {
                let src = &data[(start - off) as usize..(end - off) as usize];
                // The region is ours and mapped; a failure would only leave the map stale.
                let _ = c.mem().write_bytes(m.addr + (start - m.off) as u32, src);
            }
        }
    }

    fn pread(&self, c: &mut Cpu, st: &Shared, wide: bool) {
        let (fd, buf, n, off) = args4(c, wide);
        let r = {
            let mut s = lock(st);
            s.vfs.lseek(fd as i32, 0, 1).and_then(|saved| {
                let r = s.vfs.lseek(fd as i32, off as i64, 0).and_then(|_| s.vfs.read(fd as i32, n as usize));
                s.vfs.lseek(fd as i32, saved as i64, 0)?;
                r
            })
        };
        match r {
            Ok(data) => {
                if wr(c, buf, &data) {
                    c.ret_u32(data.len() as u32);
                }
            }
            Err(e) => fail(c, st, e),
        }
    }

    fn pwrite(&self, c: &mut Cpu, st: &Shared, wide: bool) {
        let (fd, buf, n, off) = args4(c, wide);
        let Some(data) = rd(c, buf, n) else { return };
        let r = {
            let mut s = lock(st);
            s.vfs.lseek(fd as i32, 0, 1).and_then(|saved| {
                let r = s.vfs.lseek(fd as i32, off as i64, 0).and_then(|_| s.vfs.write(fd as i32, &data));
                s.vfs.lseek(fd as i32, saved as i64, 0)?;
                r
            })
        };
        match r {
            Ok(k) => {
                self.refresh(c, fd, off, &data[..k]);
                c.ret_u32(k as u32);
            }
            Err(e) => fail(c, st, e),
        }
    }

    /// `mmap`; `Some(())` when this was a shared file mapping and the result is set.
    fn mmap(&self, c: &mut Cpu, st: &Shared) -> bool {
        let (len, flags, fd, off) = {
            let mut a = c.args();
            let (_addr, len, _prot, flags, fd, off) = (a.u32(c), a.u32(c), a.u32(c), a.u32(c), a.u32(c), a.u32(c));
            (len, flags, fd, off)
        };
        if flags & MAP_SHARED == 0 || flags & MAP_ANONYMOUS != 0 || fd == u32::MAX {
            return false;
        }
        let size = len.div_ceil(PAGE).max(1) * PAGE;
        let content = {
            let mut s = lock(st);
            s.vfs.fstat(fd as i32).and_then(|stat| {
                let avail = stat.size.saturating_sub(u64::from(off)).min(u64::from(len)) as usize;
                s.vfs.read_range(fd as i32, u64::from(off), avail)
            })
        };
        let content = match content {
            Ok(d) => d,
            Err(e) => {
                fail(c, st, e);
                return true;
            }
        };
        let Ok(addr) = c.mem().map_anywhere(size, PAGE) else {
            fail(c, st, errno::ENOMEM);
            return true;
        };
        if wr(c, addr, &content) {
            self.maps().push(Map { addr, len: size, fd, off: u64::from(off) });
            c.ret_u32(addr);
        }
        true
    }

    /// `munmap` of a shared mapping (the address space is not reused). False if `addr` is not one.
    fn munmap(&self, c: &mut Cpu) -> bool {
        let addr = c.args().u32(c);
        let mut maps = self.maps();
        let before = maps.len();
        maps.retain(|m| m.addr != addr);
        let hit = maps.len() != before;
        drop(maps);
        if hit {
            c.ret_u32(0);
        }
        hit
    }

    /// Register the thunks on `cpu`.
    pub fn install(&self, cpu: &Cpu, st: &Shared) {
        let reg = |name: &str, f: bhpc_cpu::SharedThunkFn| {
            let _ = cpu.register_shared_thunk(name, f);
        };
        for (name, wide) in [("pread", false), ("pread64", true)] {
            let (m, s) = (self.clone(), st.clone());
            reg(name, Arc::new(move |c: &mut Cpu| m.pread(c, &s, wide)));
        }
        for (name, wide) in [("pwrite", false), ("pwrite64", true)] {
            let (m, s) = (self.clone(), st.clone());
            reg(name, Arc::new(move |c: &mut Cpu| m.pwrite(c, &s, wide)));
        }
        let (m, s) = (self.clone(), st.clone());
        let inner = bhpc_bionic::builtin("mmap", st);
        reg(
            "mmap",
            Arc::new(move |c: &mut Cpu| {
                if !m.mmap(c, &s) {
                    if let Some(f) = &inner {
                        f(c);
                    }
                }
            }),
        );
        let m = self.clone();
        let inner = bhpc_bionic::builtin("munmap", st);
        reg(
            "munmap",
            Arc::new(move |c: &mut Cpu| {
                if !m.munmap(c) {
                    if let Some(f) = &inner {
                        f(c);
                    }
                }
            }),
        );
    }
}

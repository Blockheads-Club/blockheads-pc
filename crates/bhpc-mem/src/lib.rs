//! Guest address window: a 4 GiB (plus guard) host `mmap` reservation.
//!
//! Guest pointer `g` lives at host address `base + g`. Pages are reserved `PROT_NONE` and become
//! readable/writable only when mapped with [`GuestMemory::map`], so an access to an unmapped guest
//! address faults (dynarmic's fastmem path turns that into a callback; see `bhpc-cpu`).

mod heap;
#[cfg(unix)]
#[path = "sys_unix.rs"]
mod os;
#[cfg(windows)]
#[path = "sys_windows.rs"]
mod os;

pub use heap::GuestHeap;

use std::fmt;
use std::sync::atomic::{AtomicU16, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, RwLock};

/// Size of the guest address space.
pub const WINDOW_SIZE: u64 = 1 << 32;
/// Extra reserved bytes after the window so that `addr + small_offset` never leaves the mapping.
pub const GUARD_SIZE: usize = 1 << 16;
/// Guest page size.
pub const PAGE_SIZE: u32 = 4096;
/// Guest addresses handed out by [`GuestMemory::map_anywhere`] start here.
pub const MODULE_ARENA_START: u32 = 0x1000_0000;
/// End (exclusive) of the module arena.
pub const MODULE_ARENA_END: u32 = 0x4000_0000;

/// Errors from guest memory operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemError {
    /// The range `[addr, addr+len)` is not (fully) mapped.
    Unmapped { addr: u32, len: u32 },
    /// The range overlaps an existing mapping.
    Overlap { addr: u32, len: u32 },
    /// The range does not fit in the 32-bit window (or has zero length).
    BadRange { addr: u32, len: u32 },
    /// No free space left in the arena.
    OutOfSpace,
    /// A guest C string was not NUL-terminated inside mapped memory or was not UTF-8.
    BadString { addr: u32 },
    /// The host refused the operation (`errno`).
    Os(i32),
}

impl fmt::Display for MemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemError::Unmapped { addr, len } => write!(f, "guest range {addr:#x}+{len:#x} is not mapped"),
            MemError::Overlap { addr, len } => write!(f, "guest range {addr:#x}+{len:#x} overlaps a mapping"),
            MemError::BadRange { addr, len } => write!(f, "invalid guest range {addr:#x}+{len:#x}"),
            MemError::OutOfSpace => write!(f, "guest arena exhausted"),
            MemError::BadString { addr } => write!(f, "bad guest string at {addr:#x}"),
            MemError::Os(e) => write!(f, "host error {e}"),
        }
    }
}

impl std::error::Error for MemError {}

/// Host page protection of a guest range (the `PF_R`/`PF_W`/`PF_X` bits of an ELF `PT_LOAD`).
///
/// Guest code is never executed by the host (the JIT reads it), so `EXEC` only matters for keeping the host's
/// view identical to the guest's mapping flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Prot(u8);

impl Prot {
    /// No access: any touch faults.
    pub const NONE: Prot = Prot(0);
    /// Readable.
    pub const READ: Prot = Prot(1);
    /// Writable.
    pub const WRITE: Prot = Prot(2);
    /// Executable.
    pub const EXEC: Prot = Prot(4);

    /// True if every bit of `other` is set in `self`.
    pub fn contains(self, other: Prot) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for Prot {
    type Output = Prot;
    fn bitor(self, rhs: Prot) -> Prot {
        Prot(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Prot {
    fn bitor_assign(&mut self, rhs: Prot) {
        self.0 |= rhs.0;
    }
}

/// The `PAGE_*` constant `VirtualProtect` takes for `p`. Windows has no write-only pages: write implies read.
/// Lives here (not in `sys_windows.rs`) so its logic is tested on every host.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_page_protection(p: Prot) -> u32 {
    const PAGE_NOACCESS: u32 = 0x01;
    const PAGE_READONLY: u32 = 0x02;
    const PAGE_READWRITE: u32 = 0x04;
    const PAGE_EXECUTE: u32 = 0x10;
    const PAGE_EXECUTE_READ: u32 = 0x20;
    const PAGE_EXECUTE_READWRITE: u32 = 0x40;
    match (p.contains(Prot::READ), p.contains(Prot::WRITE), p.contains(Prot::EXEC)) {
        (_, true, true) => PAGE_EXECUTE_READWRITE,
        (true, false, true) => PAGE_EXECUTE_READ,
        (false, false, true) => PAGE_EXECUTE,
        (_, true, false) => PAGE_READWRITE,
        (true, false, false) => PAGE_READONLY,
        (false, false, false) => PAGE_NOACCESS,
    }
}

#[derive(Debug, Clone, Copy)]
struct Region {
    start: u64,
    end: u64,
}

/// The guest address window.
pub struct GuestMemory {
    base: *mut u8,
    regions: RwLock<Vec<Region>>,
    arena_cursor: Mutex<u64>,
}

// SAFETY: the window is a plain byte array; callers uphold guest-level synchronisation just like
// real hardware. All bookkeeping is behind locks.
unsafe impl Send for GuestMemory {}
unsafe impl Sync for GuestMemory {}

impl fmt::Debug for GuestMemory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GuestMemory").field("base", &self.base).finish()
    }
}

fn page_align_up(v: u64) -> u64 {
    (v + (PAGE_SIZE as u64 - 1)) & !(PAGE_SIZE as u64 - 1)
}

impl GuestMemory {
    /// Reserve the window.
    pub fn new() -> Result<Self, MemError> {
        let len = WINDOW_SIZE as usize + GUARD_SIZE;
        let base = sys::reserve(len).map_err(MemError::Os)?;
        Ok(GuestMemory { base, regions: RwLock::new(Vec::new()), arena_cursor: Mutex::new(MODULE_ARENA_START as u64) })
    }

    /// Host address of guest address 0.
    pub fn base(&self) -> *mut u8 {
        self.base
    }

    fn check_range(addr: u32, len: u32) -> Result<(u64, u64), MemError> {
        let start = addr as u64;
        let end = start + len as u64;
        if len == 0 || end > WINDOW_SIZE {
            return Err(MemError::BadRange { addr, len });
        }
        Ok((start, end))
    }

    /// Map `[addr, addr+len)` (rounded out to pages) read/write and zero-filled.
    pub fn map(&self, addr: u32, len: u32) -> Result<(), MemError> {
        let (start, end) = Self::check_range(addr, len)?;
        let start = start & !(PAGE_SIZE as u64 - 1);
        // `check_range` bounded `end` by the (page-aligned) window size, so rounding cannot overflow it.
        let end = page_align_up(end);
        let mut regions = self.regions.write().unwrap();
        if regions.iter().any(|r| start < r.end && r.start < end) {
            return Err(MemError::Overlap { addr, len });
        }
        // SAFETY: the range is inside our own reservation.
        unsafe { sys::make_accessible(self.base.add(start as usize), (end - start) as usize) }.map_err(MemError::Os)?;
        regions.push(Region { start, end });
        Ok(())
    }

    /// Map `len` bytes at a fresh address in the module arena aligned to `align` (a power of two).
    pub fn map_anywhere(&self, len: u32, align: u32) -> Result<u32, MemError> {
        let align = (align.max(PAGE_SIZE)) as u64;
        let size = page_align_up(len.max(1) as u64);
        let mut cur = self.arena_cursor.lock().unwrap();
        let start = (*cur + align - 1) & !(align - 1);
        if start + size > MODULE_ARENA_END as u64 {
            return Err(MemError::OutOfSpace);
        }
        self.map(start as u32, size as u32)?;
        *cur = start + size;
        Ok(start as u32)
    }

    /// Change the host protection of the mapped range `[addr, addr+len)` (page aligned).
    ///
    /// The range stays mapped for [`GuestMemory::is_mapped`]: the Rust-side accessors keep working, only direct
    /// host accesses (the JIT's fastmem path) see the protection.
    pub fn protect(&self, addr: u32, len: u32, prot: Prot) -> Result<(), MemError> {
        let (start, _) = Self::check_range(addr, len)?;
        if !self.is_mapped(addr, len) {
            return Err(MemError::Unmapped { addr, len });
        }
        // SAFETY: the range is mapped inside our own reservation.
        unsafe { sys::protect(self.base.add(start as usize), len as usize, prot) }.map_err(MemError::Os)
    }

    /// True if every byte of `[addr, addr+len)` is mapped.
    pub fn is_mapped(&self, addr: u32, len: u32) -> bool {
        let Ok((start, end)) = Self::check_range(addr, len) else { return false };
        let regions = self.regions.read().unwrap();
        let mut pos = start;
        // Regions never overlap but may be adjacent, so walk forward until the range is covered.
        while pos < end {
            match regions.iter().find(|r| r.start <= pos && pos < r.end) {
                Some(r) => pos = r.end,
                None => return false,
            }
        }
        true
    }

    fn require(&self, addr: u32, len: u32) -> Result<*mut u8, MemError> {
        if self.is_mapped(addr, len) {
            // SAFETY: range verified to be inside the reservation.
            Ok(unsafe { self.base.add(addr as usize) })
        } else {
            Err(MemError::Unmapped { addr, len })
        }
    }

    /// Copy bytes into the guest.
    pub fn write_bytes(&self, addr: u32, data: &[u8]) -> Result<(), MemError> {
        if data.is_empty() {
            return Ok(());
        }
        let p = self.require(addr, data.len() as u32)?;
        // SAFETY: `require` proved the destination is mapped and large enough.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len()) };
        Ok(())
    }

    /// Copy bytes out of the guest.
    pub fn read_bytes(&self, addr: u32, len: u32) -> Result<Vec<u8>, MemError> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let p = self.require(addr, len)?;
        let mut out = vec![0u8; len as usize];
        // SAFETY: `require` proved the source is mapped and large enough.
        unsafe { std::ptr::copy_nonoverlapping(p, out.as_mut_ptr(), len as usize) };
        Ok(out)
    }

    /// Read a NUL-terminated UTF-8 string (at most 64 KiB).
    pub fn read_cstr(&self, addr: u32) -> Result<String, MemError> {
        let mut bytes = Vec::new();
        for i in 0..0x10000u32 {
            let b = self.read_u8(addr.wrapping_add(i)).map_err(|_| MemError::BadString { addr })?;
            if b == 0 {
                return String::from_utf8(bytes).map_err(|_| MemError::BadString { addr });
            }
            bytes.push(b);
        }
        Err(MemError::BadString { addr })
    }

    /// Write a NUL-terminated string.
    pub fn write_cstr(&self, addr: u32, s: &str) -> Result<(), MemError> {
        let mut v = s.as_bytes().to_vec();
        v.push(0);
        self.write_bytes(addr, &v)
    }
}

macro_rules! rw_int {
    ($read:ident, $write:ident, $t:ty) => {
        impl GuestMemory {
            #[doc = concat!("Read a little-endian `", stringify!($t), "`.")]
            pub fn $read(&self, addr: u32) -> Result<$t, MemError> {
                let p = self.require(addr, std::mem::size_of::<$t>() as u32)?;
                // SAFETY: mapped; unaligned read of plain integer.
                Ok(unsafe { std::ptr::read_unaligned(p.cast::<$t>()) }.to_le())
            }
            #[doc = concat!("Write a little-endian `", stringify!($t), "`.")]
            pub fn $write(&self, addr: u32, v: $t) -> Result<(), MemError> {
                let p = self.require(addr, std::mem::size_of::<$t>() as u32)?;
                // SAFETY: mapped; unaligned write of plain integer.
                unsafe { std::ptr::write_unaligned(p.cast::<$t>(), v.to_le()) };
                Ok(())
            }
        }
    };
}
rw_int!(read_u8, write_u8, u8);
rw_int!(read_u16, write_u16, u16);
rw_int!(read_u32, write_u32, u32);
rw_int!(read_u64, write_u64, u64);

macro_rules! cas_int {
    ($name:ident, $t:ty, $atomic:ty) => {
        impl GuestMemory {
            #[doc = concat!("Atomically replace a `", stringify!($t), "` with `new` if it equals `expected` (exclusive-store support).")]
            ///
            /// Misaligned addresses fall back to a non-atomic compare and write.
            pub fn $name(&self, addr: u32, expected: $t, new: $t) -> Result<bool, MemError> {
                let p = self.require(addr, std::mem::size_of::<$t>() as u32)?;
                if (p as usize) % std::mem::align_of::<$t>() == 0 {
                    // SAFETY: mapped, aligned, and only accessed through atomics/plain guest accesses.
                    let a = unsafe { <$atomic>::from_ptr(p.cast::<$t>()) };
                    Ok(a.compare_exchange(expected, new, Ordering::SeqCst, Ordering::SeqCst).is_ok())
                } else {
                    // SAFETY: mapped; unaligned plain accesses.
                    unsafe {
                        if std::ptr::read_unaligned(p.cast::<$t>()) == expected {
                            std::ptr::write_unaligned(p.cast::<$t>(), new);
                            Ok(true)
                        } else {
                            Ok(false)
                        }
                    }
                }
            }
        }
    };
}
cas_int!(cas_u8, u8, AtomicU8);
cas_int!(cas_u16, u16, AtomicU16);
cas_int!(cas_u32, u32, AtomicU32);
cas_int!(cas_u64, u64, AtomicU64);

impl Drop for GuestMemory {
    fn drop(&mut self) {
        // SAFETY: unmapping our own reservation exactly once.
        unsafe { sys::release(self.base, WINDOW_SIZE as usize + GUARD_SIZE) };
    }
}

/// The host calls that can fail. Under `cfg(test)` they can be made to fail on demand.
///
/// The OS-specific part lives in `os` (`sys_unix.rs` or `sys_windows.rs`); this wrapper adds the
/// failure injection so both back ends share one tested surface.
mod sys {
    #[cfg(test)]
    thread_local! {
        /// Number of upcoming `reserve` calls / `make_accessible` calls to fail with `ERR_NOMEM`.
        pub static FAIL_RESERVE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        pub static FAIL_PROTECT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }

    use crate::os;

    #[cfg(test)]
    pub use os::ERR_NOMEM;
    #[cfg(all(test, unix))]
    pub use os::{host_page, ERR_INVAL};

    #[cfg(test)]
    fn injected(flag: &'static std::thread::LocalKey<std::cell::Cell<u32>>) -> bool {
        flag.with(|f| {
            let n = f.get();
            f.set(n.saturating_sub(1));
            n > 0
        })
    }

    /// Reserve `len` bytes of inaccessible address space.
    pub fn reserve(len: usize) -> Result<*mut u8, i32> {
        #[cfg(test)]
        if injected(&FAIL_RESERVE) {
            return Err(ERR_NOMEM);
        }
        os::reserve(len)
    }

    /// Make `[p, p+len)` readable and writable.
    ///
    /// # Safety
    /// The range must lie inside a reservation made by [`reserve`].
    pub unsafe fn make_accessible(p: *mut u8, len: usize) -> Result<(), i32> {
        #[cfg(test)]
        if injected(&FAIL_PROTECT) {
            return Err(ERR_NOMEM);
        }
        // SAFETY: forwarded from the caller.
        unsafe { os::make_accessible(p, len) }
    }

    /// Set the protection of committed pages.
    ///
    /// # Safety
    /// The range must lie inside a reservation made by [`reserve`] and be accessible already.
    pub unsafe fn protect(p: *mut u8, len: usize, prot: crate::Prot) -> Result<(), i32> {
        #[cfg(test)]
        if injected(&FAIL_PROTECT) {
            return Err(ERR_NOMEM);
        }
        // SAFETY: forwarded from the caller.
        unsafe { os::protect(p, len, prot) }
    }

    /// Give a whole reservation back.
    ///
    /// # Safety
    /// `p`/`len` must be exactly a reservation from [`reserve`], released once.
    pub unsafe fn release(p: *mut u8, len: usize) {
        // SAFETY: forwarded from the caller.
        unsafe { os::release(p, len) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_read_write_roundtrip() {
        let m = GuestMemory::new().unwrap();
        m.map(0x2000, 0x2000).unwrap();
        m.write_u8(0x2000, 0xab).unwrap();
        m.write_u16(0x2002, 0xbeef).unwrap();
        m.write_u32(0x2004, 0xdead_beef).unwrap();
        m.write_u64(0x2008, 0x0123_4567_89ab_cdef).unwrap();
        assert_eq!(m.read_u8(0x2000).unwrap(), 0xab);
        assert_eq!(m.read_u16(0x2002).unwrap(), 0xbeef);
        assert_eq!(m.read_u32(0x2004).unwrap(), 0xdead_beef);
        assert_eq!(m.read_u64(0x2008).unwrap(), 0x0123_4567_89ab_cdef);
        assert_eq!(m.read_u8(0x2004).unwrap(), 0xef, "little endian");
        // Fresh pages are zero.
        assert_eq!(m.read_u32(0x3ffc).unwrap(), 0);
        assert!(!m.base().is_null());
        assert!(format!("{m:?}").contains("GuestMemory"));
    }

    #[test]
    fn unmapped_and_bad_ranges_error() {
        let m = GuestMemory::new().unwrap();
        assert_eq!(m.read_u32(0x100), Err(MemError::Unmapped { addr: 0x100, len: 4 }));
        assert_eq!(m.write_bytes(0x100, &[1]), Err(MemError::Unmapped { addr: 0x100, len: 1 }));
        assert_eq!(m.read_bytes(0x100, 1), Err(MemError::Unmapped { addr: 0x100, len: 1 }));
        assert_eq!(m.map(0, 0), Err(MemError::BadRange { addr: 0, len: 0 }));
        assert_eq!(m.map(0xffff_f000, 0x2000), Err(MemError::BadRange { addr: 0xffff_f000, len: 0x2000 }));
        // Rounds past the end only after page alignment.
        assert!(!m.is_mapped(0xffff_ffff, 2));
        assert!(!m.is_mapped(0, 0));
        assert_eq!(m.write_bytes(0x100, &[]), Ok(()));
        assert_eq!(m.read_bytes(0x100, 0), Ok(vec![]));
    }

    #[test]
    fn overlap_detected_and_adjacent_regions_span() {
        let m = GuestMemory::new().unwrap();
        m.map(0x1000, 0x1000).unwrap();
        m.map(0x2000, 0x1000).unwrap();
        assert_eq!(m.map(0x1800, 0x100), Err(MemError::Overlap { addr: 0x1800, len: 0x100 }));
        assert!(m.is_mapped(0x1ffc, 8), "range crossing two adjacent regions");
        m.write_u64(0x1ffc, 0x1122_3344_5566_7788).unwrap();
        assert_eq!(m.read_u64(0x1ffc).unwrap(), 0x1122_3344_5566_7788);
        assert!(!m.is_mapped(0x2ffc, 8), "runs off the end");
    }

    #[test]
    fn map_rounds_to_pages() {
        let m = GuestMemory::new().unwrap();
        m.map(0x5010, 0x10).unwrap();
        assert!(m.is_mapped(0x5000, 0x1000));
        assert!(!m.is_mapped(0x6000, 1));
    }

    #[test]
    fn map_anywhere_allocates_aligned_increasing() {
        let m = GuestMemory::new().unwrap();
        let a = m.map_anywhere(0x1800, 0x10000).unwrap();
        assert_eq!(a, MODULE_ARENA_START);
        let b = m.map_anywhere(1, 0x10000).unwrap();
        assert_eq!(b, MODULE_ARENA_START + 0x10000);
        assert!(m.is_mapped(a, 0x1800));
        let c = m.map_anywhere(0, 0).unwrap();
        assert_eq!(c, MODULE_ARENA_START + 0x11000, "align 0 means page alignment");
        assert_eq!(m.map_anywhere(MODULE_ARENA_END - MODULE_ARENA_START, 0x1000), Err(MemError::OutOfSpace));
    }

    #[test]
    fn map_anywhere_propagates_overlap() {
        let m = GuestMemory::new().unwrap();
        m.map(MODULE_ARENA_START, 0x1000).unwrap();
        assert!(matches!(m.map_anywhere(0x1000, 0x1000), Err(MemError::Overlap { .. })));
    }

    #[test]
    fn cstr_roundtrip_and_errors() {
        let m = GuestMemory::new().unwrap();
        m.map(0x1000, 0x1000).unwrap();
        m.write_cstr(0x1000, "héllo").unwrap();
        assert_eq!(m.read_cstr(0x1000).unwrap(), "héllo");
        m.write_bytes(0x1100, &[0xff, 0xfe, 0]).unwrap();
        assert_eq!(m.read_cstr(0x1100), Err(MemError::BadString { addr: 0x1100 }));
        // Runs off the mapped page without a terminator.
        m.write_bytes(0x1ffe, b"ab").unwrap();
        assert_eq!(m.read_cstr(0x1ffe), Err(MemError::BadString { addr: 0x1ffe }));
        // Longer than 64 KiB without NUL.
        let big = GuestMemory::new().unwrap();
        big.map(0x10000, 0x20000).unwrap();
        big.write_bytes(0x10000, &vec![b'x'; 0x10000]).unwrap();
        assert_eq!(big.read_cstr(0x10000), Err(MemError::BadString { addr: 0x10000 }));
    }

    #[test]
    fn compare_and_swap_all_widths() {
        let m = GuestMemory::new().unwrap();
        m.map(0x1000, 0x1000).unwrap();
        m.write_u64(0x1000, 0x1111_2222_3333_4444).unwrap();
        assert_eq!(m.cas_u8(0x1000, 0x44, 0x55), Ok(true));
        assert_eq!(m.cas_u8(0x1000, 0x44, 0x66), Ok(false));
        assert_eq!(m.cas_u16(0x1000, 0x4444, 0x7777), Ok(false), "low byte is now 0x55");
        assert_eq!(m.cas_u16(0x1000, 0x4455, 0x9999), Ok(true));
        assert_eq!(m.cas_u32(0x1000, 0x3333_9999, 7), Ok(true));
        assert_eq!(m.cas_u32(0x1000, 0, 1), Ok(false));
        assert_eq!(m.cas_u64(0x1000, 0x1111_2222_0000_0007, 9), Ok(true));
        assert_eq!(m.read_u64(0x1000).unwrap(), 9);
        // Misaligned: non-atomic fallback.
        m.write_u32(0x1101, 5).unwrap();
        assert_eq!(m.cas_u32(0x1101, 5, 6), Ok(true));
        assert_eq!(m.cas_u32(0x1101, 5, 7), Ok(false));
        assert_eq!(m.read_u32(0x1101).unwrap(), 6);
        assert_eq!(m.cas_u32(0x9000, 0, 1), Err(MemError::Unmapped { addr: 0x9000, len: 4 }));
    }

    #[test]
    fn protect_changes_host_access_but_keeps_the_mapping() {
        let m = GuestMemory::new().unwrap();
        m.map(0x1000, 0x2000).unwrap();
        m.write_u32(0x1000, 7).unwrap();
        m.protect(0x1000, 0x1000, Prot::READ | Prot::EXEC).unwrap();
        // SAFETY: the page is mapped and readable.
        assert_eq!(unsafe { std::ptr::read_volatile(m.base().add(0x1000).cast::<u32>()) }, 7);
        m.protect(0x1000, 0x1000, Prot::READ | Prot::WRITE).unwrap();
        m.write_u32(0x1000, 8).unwrap();
        m.protect(0x2000, 0x1000, Prot::NONE).unwrap();
        assert!(m.is_mapped(0x2000, 0x1000));
        assert_eq!(m.protect(0x8000, 0x1000, Prot::READ), Err(MemError::Unmapped { addr: 0x8000, len: 0x1000 }));
        assert_eq!(m.protect(0x1000, 0, Prot::READ), Err(MemError::BadRange { addr: 0x1000, len: 0 }));
        sys::FAIL_PROTECT.with(|f| f.set(1));
        assert_eq!(m.protect(0x1000, 0x1000, Prot::READ), Err(MemError::Os(sys::ERR_NOMEM)));
        // POSIX rejects an unaligned start (Windows rounds it down to the page instead).
        #[cfg(unix)]
        assert!(m.protect(0x1001, 0x10, Prot::READ).is_err());
        let mut p = Prot::NONE;
        p |= Prot::WRITE;
        assert!(p.contains(Prot::WRITE) && !p.contains(Prot::READ) && Prot::default() == Prot::NONE);
    }

    #[test]
    fn windows_protections() {
        let (r, w, x) = (Prot::READ, Prot::WRITE, Prot::EXEC);
        let table = [
            (Prot::NONE, 0x01),
            (r, 0x02),
            (w, 0x04),
            (r | w, 0x04),
            (x, 0x10),
            (r | x, 0x20),
            (w | x, 0x40),
            (r | w | x, 0x40),
        ];
        for (p, want) in table {
            assert_eq!(windows_page_protection(p), want, "{p:?}");
        }
    }

    #[test]
    fn host_failures_are_reported() {
        sys::FAIL_RESERVE.with(|f| f.set(1));
        assert_eq!(GuestMemory::new().err(), Some(MemError::Os(sys::ERR_NOMEM)));
        let m = GuestMemory::new().unwrap();
        sys::FAIL_PROTECT.with(|f| f.set(1));
        assert_eq!(m.map(0x1000, 0x1000), Err(MemError::Os(sys::ERR_NOMEM)));
        assert!(!m.is_mapped(0x1000, 1), "a failed map leaves no region behind");
        m.map(0x1000, 0x1000).unwrap();
        // The real host call fails too when handed an impossible range (not page aligned / outside).
        // Windows rounds unaligned commits down instead of rejecting them, so this is POSIX only.
        #[cfg(unix)]
        {
            // SAFETY: with 4 KiB host pages mprotect rejects the unaligned address and touches nothing;
            // with larger host pages the range is widened to whole pages inside the reservation.
            let e = unsafe { sys::make_accessible(m.base().add(1), 4096) };
            assert_eq!(e == Err(sys::ERR_INVAL), sys::host_page() == 4096);
        }
        let huge = sys::reserve(usize::MAX / 2);
        assert!(huge.is_err());
    }

    #[test]
    fn error_display() {
        let errs = [
            MemError::Unmapped { addr: 1, len: 2 },
            MemError::Overlap { addr: 1, len: 2 },
            MemError::BadRange { addr: 1, len: 2 },
            MemError::OutOfSpace,
            MemError::BadString { addr: 1 },
            MemError::Os(12),
        ];
        let text: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
        assert!(text[0].contains("not mapped"));
        assert!(text[1].contains("overlaps"));
        assert!(text[2].contains("invalid"));
        assert!(text[3].contains("exhausted"));
        assert!(text[4].contains("string"));
        assert!(text[5].contains("12"));
        let _: &dyn std::error::Error = &errs[0];
    }
}

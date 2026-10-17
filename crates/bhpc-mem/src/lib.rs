//! Guest address window: a 4 GiB (plus guard) host `mmap` reservation.
//!
//! Guest pointer `g` lives at host address `base + g`. Pages are reserved `PROT_NONE` and become
//! readable/writable only when mapped with [`GuestMemory::map`], so an access to an unmapped guest
//! address faults (dynarmic's fastmem path turns that into a callback; see `bhpc-cpu`).

mod heap;

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
        // SAFETY: anonymous private reservation, no existing memory is touched.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
                -1,
                0,
            )
        };
        if p == libc::MAP_FAILED {
            return Err(MemError::Os(errno()));
        }
        Ok(GuestMemory {
            base: p.cast(),
            regions: RwLock::new(Vec::new()),
            arena_cursor: Mutex::new(MODULE_ARENA_START as u64),
        })
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
        let end = page_align_up(end);
        if end > WINDOW_SIZE {
            return Err(MemError::BadRange { addr, len });
        }
        let mut regions = self.regions.write().unwrap();
        if regions.iter().any(|r| start < r.end && r.start < end) {
            return Err(MemError::Overlap { addr, len });
        }
        // SAFETY: the range is inside our own reservation.
        let rc = unsafe {
            libc::mprotect(
                self.base.add(start as usize).cast(),
                (end - start) as usize,
                libc::PROT_READ | libc::PROT_WRITE,
            )
        };
        if rc != 0 {
            return Err(MemError::Os(errno()));
        }
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
        unsafe { libc::munmap(self.base.cast(), WINDOW_SIZE as usize + GUARD_SIZE) };
    }
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
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

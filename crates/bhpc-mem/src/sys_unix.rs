// SPDX-License-Identifier: GPL-3.0-only
//! POSIX back end (Linux, macOS): `mmap` reservation, `mprotect` commit, `munmap` release.

#[cfg(test)]
pub const ERR_NOMEM: i32 = libc::ENOMEM;
#[cfg(test)]
pub const ERR_INVAL: i32 = libc::EINVAL;

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `MAP_NORESERVE` does not exist on every POSIX (macOS ignores overcommit accounting anyway).
const EXTRA_FLAGS: i32 = {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        libc::MAP_NORESERVE
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        0
    }
};

pub fn reserve(len: usize) -> Result<*mut u8, i32> {
    // SAFETY: anonymous private reservation, no existing memory is touched.
    let p = unsafe {
        libc::mmap(std::ptr::null_mut(), len, libc::PROT_NONE, libc::MAP_PRIVATE | libc::MAP_ANON | EXTRA_FLAGS, -1, 0)
    };
    if p == libc::MAP_FAILED {
        Err(errno())
    } else {
        Ok(p.cast())
    }
}

/// Host page size (16 KiB on Apple Silicon and some arm64 Linux, 4 KiB elsewhere).
pub fn host_page() -> usize {
    // SAFETY: sysconf has no preconditions.
    usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).unwrap_or(4096)
}

/// Widen `[addr, addr+len)` outward to whole host pages (`page` is a power of two). Guest pages are
/// 4 KiB, so on a larger host page a neighbour inside the same host page becomes accessible too.
fn round_range(addr: usize, len: usize, page: usize) -> (usize, usize) {
    let start = addr & !(page - 1);
    let end = (addr + len + page - 1) & !(page - 1);
    (start, end - start)
}

/// # Safety
/// The range must lie inside a reservation made by [`reserve`].
pub unsafe fn make_accessible(p: *mut u8, len: usize) -> Result<(), i32> {
    // Exact multiples of 4 KiB host pages must stay untouched so the kernel still rejects bad ranges.
    let (start, len) = if host_page() == 4096 { (p as usize, len) } else { round_range(p as usize, len, host_page()) };
    // SAFETY: forwarded from the caller; rounding stays inside the reservation (window + guard).
    if unsafe { libc::mprotect(start as *mut libc::c_void, len, libc::PROT_READ | libc::PROT_WRITE) } != 0 {
        Err(errno())
    } else {
        Ok(())
    }
}

/// # Safety
/// Exactly a reservation from [`reserve`], released once.
pub unsafe fn release(p: *mut u8, len: usize) {
    // SAFETY: forwarded from the caller.
    unsafe { libc::munmap(p.cast(), len) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_range_widens_to_host_pages() {
        assert_eq!(round_range(0x1000, 0x1000, 0x4000), (0, 0x4000));
        assert_eq!(round_range(0x7000, 0x2000, 0x4000), (0x4000, 0x8000));
        assert_eq!(round_range(0x4000, 0x4000, 0x4000), (0x4000, 0x4000));
        assert_eq!(round_range(0x2000, 0x1000, 0x1000), (0x2000, 0x1000));
    }

    #[test]
    fn host_page_is_power_of_two() {
        assert!(host_page().is_power_of_two());
    }
}

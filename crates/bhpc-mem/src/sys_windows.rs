// SPDX-License-Identifier: GPL-3.0-only
//! Windows back end: `VirtualAlloc(MEM_RESERVE)` reservation, `MEM_COMMIT` to make pages accessible,
//! `VirtualFree(MEM_RELEASE)` to give the window back. Declared by hand to avoid a binding crate.
//! Not exercised on Linux CI; see crates/bhpc-cpu/README.md.

use std::ffi::c_void;

/// `ERROR_NOT_ENOUGH_MEMORY`.
#[cfg(test)]
pub const ERR_NOMEM: i32 = 8;

const MEM_COMMIT: u32 = 0x1000;
const MEM_RESERVE: u32 = 0x2000;
const MEM_RELEASE: u32 = 0x8000;
const PAGE_NOACCESS: u32 = 0x01;
const PAGE_READWRITE: u32 = 0x04;

extern "system" {
    fn VirtualAlloc(addr: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    fn VirtualFree(addr: *mut c_void, size: usize, kind: u32) -> i32;
}

fn last_error() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

pub fn reserve(len: usize) -> Result<*mut u8, i32> {
    // SAFETY: a fresh reservation at an OS-chosen address, no existing memory is touched.
    let p = unsafe { VirtualAlloc(std::ptr::null_mut(), len, MEM_RESERVE, PAGE_NOACCESS) };
    if p.is_null() {
        Err(last_error())
    } else {
        Ok(p.cast())
    }
}

/// # Safety
/// The range must lie inside a reservation made by [`reserve`].
pub unsafe fn make_accessible(p: *mut u8, len: usize) -> Result<(), i32> {
    // SAFETY: forwarded from the caller.
    let r = unsafe { VirtualAlloc(p.cast(), len, MEM_COMMIT, PAGE_READWRITE) };
    if r.is_null() {
        Err(last_error())
    } else {
        Ok(())
    }
}

/// # Safety
/// Exactly a reservation from [`reserve`], released once. `len` is unused: `MEM_RELEASE` needs 0.
pub unsafe fn release(p: *mut u8, _len: usize) {
    // SAFETY: forwarded from the caller.
    unsafe { VirtualFree(p.cast(), 0, MEM_RELEASE) };
}

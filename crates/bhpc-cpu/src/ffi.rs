//! Raw bindings to `shim/bhpc_shim.h`.
#![allow(non_camel_case_types)]

use std::ffi::c_void;

#[repr(C)]
pub struct bhpc_jit {
    _private: [u8; 0],
}

#[repr(C)]
pub struct bhpc_monitor {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct bhpc_callbacks {
    pub user: *mut c_void,
    pub read8: extern "C" fn(*mut c_void, u32) -> u8,
    pub read16: extern "C" fn(*mut c_void, u32) -> u16,
    pub read32: extern "C" fn(*mut c_void, u32) -> u32,
    pub read64: extern "C" fn(*mut c_void, u32) -> u64,
    pub write8: extern "C" fn(*mut c_void, u32, u8),
    pub write16: extern "C" fn(*mut c_void, u32, u16),
    pub write32: extern "C" fn(*mut c_void, u32, u32),
    pub write64: extern "C" fn(*mut c_void, u32, u64),
    pub write_excl8: extern "C" fn(*mut c_void, u32, u8, u8) -> u8,
    pub write_excl16: extern "C" fn(*mut c_void, u32, u16, u16) -> u8,
    pub write_excl32: extern "C" fn(*mut c_void, u32, u32, u32) -> u8,
    pub write_excl64: extern "C" fn(*mut c_void, u32, u64, u64) -> u8,
    pub svc: extern "C" fn(*mut c_void, u32),
    pub exception: extern "C" fn(*mut c_void, u32, u32),
    pub interpreter_fallback: extern "C" fn(*mut c_void, u32, usize),
}

/// `bhpc_jit_new` flags (see `bhpc_shim.h`).
pub const JIT_FASTMEM_EXCLUSIVE: u32 = 1;
pub const JIT_FAST_FP: u32 = 2;

extern "C" {
    pub fn bhpc_monitor_new(processors: usize) -> *mut bhpc_monitor;
    pub fn bhpc_monitor_free(m: *mut bhpc_monitor);
    pub fn bhpc_jit_new(
        cb: *const bhpc_callbacks,
        fastmem_base: *mut u8,
        code_cache_bytes: usize,
        cycle_counting: i32,
        monitor: *mut bhpc_monitor,
        processor_id: usize,
        flags: u32,
    ) -> *mut bhpc_jit;
    pub fn bhpc_jit_set_ticks(j: *mut bhpc_jit, ticks: u64);
    pub fn bhpc_jit_ticks(j: *mut bhpc_jit) -> u64;
    pub fn bhpc_jit_free(j: *mut bhpc_jit);
    pub fn bhpc_jit_run(j: *mut bhpc_jit, escape: *mut c_void) -> u32;
    pub fn bhpc_jit_step(j: *mut bhpc_jit, escape: *mut c_void) -> u32;
    pub fn bhpc_jit_halt(j: *mut bhpc_jit);
    pub fn bhpc_jit_clear_halt(j: *mut bhpc_jit);
    pub fn bhpc_jit_invalidate(j: *mut bhpc_jit, start: u32, len: usize);
    pub fn bhpc_jit_clear_cache(j: *mut bhpc_jit);
    pub fn bhpc_jit_regs(j: *mut bhpc_jit) -> *mut u32;
    pub fn bhpc_jit_ext_regs(j: *mut bhpc_jit) -> *mut u32;
    pub fn bhpc_jit_cpsr(j: *mut bhpc_jit) -> u32;
    pub fn bhpc_jit_set_cpsr(j: *mut bhpc_jit, v: u32);
    pub fn bhpc_jit_fpscr(j: *mut bhpc_jit) -> u32;
    pub fn bhpc_jit_set_fpscr(j: *mut bhpc_jit, v: u32);
}

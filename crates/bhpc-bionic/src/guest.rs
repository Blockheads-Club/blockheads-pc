//! Helpers for touching guest memory from host thunks. A failed access stops the guest with a
//! [`Stop::Fault`] (the same stop a JIT-executed bad access produces) and the helper reports `None`.

use bhpc_cpu::{Cpu, Fault, Stop};

/// Stop the guest with a fault at `addr`.
pub fn fault(c: &mut Cpu, addr: u32, size: u32, write: bool) {
    c.request_stop(Stop::Fault(Fault { addr, size, write }));
}

/// Read `len` bytes.
pub fn rd(c: &mut Cpu, addr: u32, len: u32) -> Option<Vec<u8>> {
    match c.mem().read_bytes(addr, len) {
        Ok(v) => Some(v),
        Err(_) => {
            fault(c, addr, len, false);
            None
        }
    }
}

/// Write bytes; false (and a guest stop) on failure.
pub fn wr(c: &mut Cpu, addr: u32, data: &[u8]) -> bool {
    if c.mem().write_bytes(addr, data).is_err() {
        fault(c, addr, data.len() as u32, true);
        return false;
    }
    true
}

/// Read a little-endian word.
pub fn rd32(c: &mut Cpu, addr: u32) -> Option<u32> {
    match c.mem().read_u32(addr) {
        Ok(v) => Some(v),
        Err(_) => {
            fault(c, addr, 4, false);
            None
        }
    }
}

/// Write a little-endian word.
pub fn wr32(c: &mut Cpu, addr: u32, v: u32) -> bool {
    wr(c, addr, &v.to_le_bytes())
}

/// Read a NUL-terminated string (without the NUL), at most `max` bytes. The flag tells whether the
/// NUL was found within `max`.
pub fn cstr_n(c: &mut Cpu, addr: u32, max: u32) -> Option<(Vec<u8>, bool)> {
    let mut out = Vec::new();
    let mut a = addr;
    let mut left = max;
    while left > 0 {
        let chunk = (4096 - (a & 4095)).min(left);
        let b = match c.mem().read_bytes(a, chunk) {
            Ok(b) => b,
            Err(_) => {
                fault(c, a, 1, false);
                return None;
            }
        };
        if let Some(i) = b.iter().position(|x| *x == 0) {
            out.extend_from_slice(&b[..i]);
            return Some((out, true));
        }
        out.extend_from_slice(&b);
        a = a.wrapping_add(chunk);
        left -= chunk;
    }
    Some((out, false))
}

/// Read a whole NUL-terminated string.
pub fn cstr(c: &mut Cpu, addr: u32) -> Option<Vec<u8>> {
    cstr_n(c, addr, u32::MAX).map(|(s, _)| s)
}

/// Write `s` followed by a NUL.
pub fn wr_cstr(c: &mut Cpu, addr: u32, s: &[u8]) -> bool {
    let mut v = s.to_vec();
    v.push(0);
    wr(c, addr, &v)
}

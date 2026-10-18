//! libc functions the game imports that `bhpc-bionic` does not provide yet: address text conversion and a few
//! tiny internals. Registered on the CPU before the libraries are linked, like the other host thunks.

use bhpc_bionic::guest::{cstr, rd, wr, wr32, wr_cstr};
use bhpc_bionic::misc::fail;
use bhpc_bionic::{lock, Shared};
use bhpc_cpu::Cpu;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

const AF_INET: u32 = 2;
const AF_INET6: u32 = 10;
const EAFNOSUPPORT: u32 = 97;
const ENOSPC: u32 = 28;

/// Text of an address: 4 bytes are IPv4, 16 bytes IPv6.
fn address_text(bytes: &[u8]) -> String {
    match <[u8; 16]>::try_from(bytes) {
        Ok(v6) => Ipv6Addr::from(v6).to_string(),
        Err(_) => Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]).to_string(),
    }
}

fn inet_pton(c: &mut Cpu, st: &Shared) {
    let (af, src, dst) = {
        let mut a = c.args();
        (a.u32(c), a.u32(c), a.u32(c))
    };
    let Some(text) = cstr(c, src) else { return };
    let text = String::from_utf8_lossy(&text).into_owned();
    let bytes = match af {
        AF_INET => Ipv4Addr::from_str(&text).ok().map(|a| a.octets().to_vec()),
        AF_INET6 => Ipv6Addr::from_str(&text).ok().map(|a| a.octets().to_vec()),
        _ => return fail(c, st, EAFNOSUPPORT),
    };
    match bytes {
        Some(b) => {
            if wr(c, dst, &b) {
                c.ret_u32(1);
            }
        }
        None => c.ret_u32(0),
    }
}

fn inet_ntop(c: &mut Cpu, st: &Shared) {
    let (af, src, dst, size) = {
        let mut a = c.args();
        (a.u32(c), a.u32(c), a.u32(c), a.u32(c))
    };
    let len = match af {
        AF_INET => 4,
        AF_INET6 => 16,
        _ => {
            fail(c, st, EAFNOSUPPORT);
            return c.ret_u32(0);
        }
    };
    let Some(raw) = rd(c, src, len) else { return };
    let text = address_text(&raw);
    if text.len() as u32 + 1 > size {
        bhpc_bionic::misc::set_errno(c, st, ENOSPC);
        return c.ret_u32(0);
    }
    if wr_cstr(c, dst, text.as_bytes()) {
        c.ret_u32(dst);
    }
}

/// Lazily allocated guest buffer (`inet_ntoa`'s result, `h_errno`).
fn buffer(st: &Shared, slot: &AtomicU32, size: u32) -> u32 {
    let cur = slot.load(Ordering::Relaxed);
    if cur != 0 {
        return cur;
    }
    let p = lock(st).heap.calloc(1, size);
    slot.store(p, Ordering::Relaxed);
    p
}

/// Register everything on `cpu`.
pub fn install(cpu: &Cpu, st: &Shared) {
    let reg = |name: &str, f: bhpc_cpu::SharedThunkFn| {
        let _ = cpu.register_shared_thunk(name, f);
    };
    let s = st.clone();
    reg("inet_pton", Arc::new(move |c: &mut Cpu| inet_pton(c, &s)));
    let s = st.clone();
    reg("inet_ntop", Arc::new(move |c: &mut Cpu| inet_ntop(c, &s)));
    let (s, slot) = (st.clone(), AtomicU32::new(0));
    reg(
        "inet_ntoa",
        Arc::new(move |c: &mut Cpu| {
            let addr = c.args().u32(c);
            let text = address_text(&addr.to_le_bytes());
            let p = buffer(&s, &slot, 20);
            if p != 0 && wr_cstr(c, p, text.as_bytes()) {
                c.ret_u32(p);
            }
        }),
    );
    let (s, slot) = (st.clone(), AtomicU32::new(0));
    reg(
        "__get_h_errno",
        Arc::new(move |c: &mut Cpu| {
            let p = buffer(&s, &slot, 4);
            if p != 0 && wr32(c, p, 0) {
                c.ret_u32(p);
            }
        }),
    );
    // `__isinf(double)`: the double travels in r0/r1.
    reg(
        "__isinf",
        Arc::new(|c: &mut Cpu| {
            let v = c.args().f64(c);
            c.ret_u32(u32::from(v.is_infinite()));
        }),
    );
}

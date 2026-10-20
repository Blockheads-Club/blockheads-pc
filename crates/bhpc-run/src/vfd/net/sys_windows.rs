//! Winsock host sockets: the same surface as `sys_unix.rs` over `ws2_32` (see docs/m7/windows-sockets.md).
//!
//! Sockets are non-blocking (`FIONBIO`) and not inherited by child processes. Errors are `WSAGetLastError()`
//! codes translated to the guest's Linux `errno` values by [`super::abi`]. Only built on Windows; its logic (the
//! tables) lives in `abi.rs`, tested on every host.

use super::abi::HOST;
use std::net::Ipv4Addr;
use std::sync::Once;
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
    IP_ADAPTER_ADDRESSES_LH,
};
use windows_sys::Win32::Networking::WinSock as ws;

/// `EINPROGRESS` / `EAGAIN` as the guest sees them.
const EINPROGRESS: u32 = 115;
const EAGAIN: u32 = 11;
/// `IF_TYPE_SOFTWARE_LOOPBACK` and `IfOperStatusUp`.
const IF_LOOPBACK: u32 = 24;
const OPER_UP: i32 = 1;
const STORAGE: usize = std::mem::size_of::<ws::SOCKADDR_STORAGE>();

/// One host socket, closed on drop.
pub(crate) struct Sock(ws::SOCKET);

fn startup() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // SAFETY: WSADATA is plain data that WSAStartup fills. Winsock stays initialised for the process.
        let mut data: ws::WSADATA = unsafe { std::mem::zeroed() };
        unsafe { ws::WSAStartup(0x0202, &mut data) };
    });
}

fn last() -> i32 {
    // SAFETY: no preconditions.
    unsafe { ws::WSAGetLastError() }
}

fn err() -> u32 {
    HOST.errno(last())
}

fn cvt(r: i32) -> Result<usize, u32> {
    if r == ws::SOCKET_ERROR {
        Err(err())
    } else {
        Ok(r as usize)
    }
}

fn wrap(s: ws::SOCKET) -> Result<Sock, u32> {
    if s == ws::INVALID_SOCKET {
        return Err(err());
    }
    let sock = Sock(s);
    let mut on = 1u32;
    // SAFETY: FIONBIO reads one u32.
    cvt(unsafe { ws::ioctlsocket(s, ws::FIONBIO, &mut on) })?;
    Ok(sock)
}

impl Drop for Sock {
    fn drop(&mut self) {
        // SAFETY: our own socket, closed once.
        unsafe { ws::closesocket(self.0) };
    }
}

impl Sock {
    /// A new socket of host `family`; `ty` is `SOCK_STREAM`/`SOCK_DGRAM`.
    pub fn open(family: i32, ty: i32, proto: i32) -> Result<Sock, u32> {
        startup();
        // SAFETY: plain call, no protocol info.
        let s = unsafe { ws::WSASocketW(family, ty, proto, std::ptr::null(), 0, ws::WSA_FLAG_NO_HANDLE_INHERIT) };
        let sock = wrap(s)?;
        if ty == ws::SOCK_DGRAM {
            // A datagram that hit a closed port must not make the next recvfrom fail with WSAECONNRESET (Linux
            // ignores the ICMP error on an unconnected UDP socket).
            let (off, mut got) = (0u32, 0u32);
            // SAFETY: SIO_UDP_CONNRESET reads one BOOL; no overlapped I/O.
            unsafe {
                ws::WSAIoctl(
                    sock.0,
                    ws::SIO_UDP_CONNRESET,
                    (&off as *const u32).cast(),
                    4,
                    std::ptr::null_mut(),
                    0,
                    &mut got,
                    std::ptr::null_mut(),
                    None,
                )
            };
        }
        Ok(sock)
    }

    pub fn bind(&self, addr: &[u8]) -> Result<(), u32> {
        // SAFETY: `addr` is a readable buffer of the given length.
        cvt(unsafe { ws::bind(self.0, addr.as_ptr().cast(), addr.len() as i32) }).map(drop)
    }

    pub fn listen(&self, backlog: i32) -> Result<(), u32> {
        // SAFETY: plain call.
        cvt(unsafe { ws::listen(self.0, backlog) }).map(drop)
    }

    /// Start connecting; a connection that is not made at once fails with `EINPROGRESS` (Winsock says
    /// `WSAEWOULDBLOCK`).
    pub fn connect(&self, addr: &[u8]) -> Result<(), u32> {
        // SAFETY: `addr` is a readable buffer of the given length.
        match cvt(unsafe { ws::connect(self.0, addr.as_ptr().cast(), addr.len() as i32) }) {
            Err(EAGAIN) => Err(EINPROGRESS),
            r => r.map(drop),
        }
    }

    /// A pending connection (non-blocking itself) and the peer's address.
    pub fn accept(&self) -> Result<(Sock, Vec<u8>), u32> {
        let mut ss = [0u8; STORAGE];
        let mut len = STORAGE as i32;
        // SAFETY: `ss`/`len` are a valid address buffer and its length.
        let s = unsafe { ws::accept(self.0, ss.as_mut_ptr().cast(), &mut len) };
        let sock = wrap(s)?;
        Ok((sock, ss[..(len as usize).min(STORAGE)].to_vec()))
    }

    /// Send `data` (to `to` when given) with host `flags`.
    pub fn send_to(&self, data: &[u8], flags: i32, to: Option<&[u8]>) -> Result<usize, u32> {
        let (dest, dlen) = to.map_or((std::ptr::null(), 0), |a| (a.as_ptr().cast(), a.len() as i32));
        let n = data.len().min(i32::MAX as usize) as i32;
        // SAFETY: `data` and `dest` are readable buffers of the given lengths (or null/0).
        cvt(unsafe { ws::sendto(self.0, data.as_ptr(), n, flags, dest, dlen) })
    }

    /// Receive into `buf` with host `flags`; returns the byte count and the sender's address. A datagram larger than
    /// `buf` is truncated (Winsock reports `WSAEMSGSIZE`, Linux just cuts it).
    pub fn recv_from(&self, buf: &mut [u8], flags: i32) -> Result<(usize, Vec<u8>), u32> {
        let mut ss = [0u8; STORAGE];
        let mut len = STORAGE as i32;
        let n = buf.len().min(i32::MAX as usize) as i32;
        // SAFETY: `buf` and `ss` are writable buffers of the given lengths.
        let r = unsafe { ws::recvfrom(self.0, buf.as_mut_ptr(), n, flags, ss.as_mut_ptr().cast(), &mut len) };
        let got = if r == ws::SOCKET_ERROR && last() == ws::WSAEMSGSIZE { Ok(buf.len()) } else { cvt(r) }?;
        Ok((got, ss[..(len.max(0) as usize).min(STORAGE)].to_vec()))
    }

    pub fn set_opt(&self, level: i32, name: i32, val: &[u8]) -> Result<(), u32> {
        // SAFETY: `val` is a readable buffer of the given length.
        cvt(unsafe { ws::setsockopt(self.0, level, name, val.as_ptr(), val.len() as i32) }).map(drop)
    }

    /// Read an option into `out`; returns its length.
    pub fn get_opt(&self, level: i32, name: i32, out: &mut [u8]) -> Result<usize, u32> {
        let mut len = out.len() as i32;
        // SAFETY: `out` is a writable buffer of `len` bytes.
        cvt(unsafe { ws::getsockopt(self.0, level, name, out.as_mut_ptr(), &mut len) })?;
        Ok(len.max(0) as usize)
    }

    /// The local (`peer` false) or remote address.
    pub fn name(&self, peer: bool) -> Result<Vec<u8>, u32> {
        let mut ss = [0u8; STORAGE];
        let mut len = STORAGE as i32;
        // SAFETY: `ss`/`len` are a valid address buffer and its length.
        let r = unsafe {
            if peer {
                ws::getpeername(self.0, ss.as_mut_ptr().cast(), &mut len)
            } else {
                ws::getsockname(self.0, ss.as_mut_ptr().cast(), &mut len)
            }
        };
        cvt(r)?;
        Ok(ss[..(len.max(0) as usize).min(STORAGE)].to_vec())
    }

    pub fn shutdown(&self, how: i32) -> Result<(), u32> {
        // SAFETY: plain call; SD_RECEIVE/SD_SEND/SD_BOTH are 0/1/2 like SHUT_*.
        cvt(unsafe { ws::shutdown(self.0, how) }).map(drop)
    }

    /// Readiness right now as epoll bits (`EPOLLIN` 1, `EPOLLOUT` 4, `EPOLLERR` 8). `select` rather than `WSAPoll`:
    /// older `WSAPoll` never reports a failed non-blocking connect, `select` puts it in the exception set.
    pub fn poll(&self, read: bool, write: bool) -> u32 {
        let one = || ws::FD_SET { fd_count: 1, fd_array: [self.0; 64] };
        let (mut r, mut w, mut e) = (one(), one(), one());
        let tv = ws::TIMEVAL { tv_sec: 0, tv_usec: 0 };
        let rp: *mut ws::FD_SET = if read { &mut r } else { std::ptr::null_mut() };
        let wp: *mut ws::FD_SET = if write { &mut w } else { std::ptr::null_mut() };
        // SAFETY: valid sets of one socket each (or null), zero timeout.
        let n = unsafe { ws::select(0, rp, wp, &mut e, &tv) };
        if n <= 0 {
            return 0;
        }
        let mut bits = 0;
        if read && r.fd_count == 1 {
            bits |= 1;
        }
        if write && w.fd_count == 1 {
            bits |= 4;
        }
        if e.fd_count == 1 {
            bits |= 8;
        }
        bits
    }

    /// Bytes waiting to be read (`FIONREAD`).
    pub fn pending(&self) -> u32 {
        let mut n = 0u32;
        // SAFETY: FIONREAD writes one u32.
        unsafe { ws::ioctlsocket(self.0, ws::FIONREAD, &mut n) };
        n
    }

    /// The pending socket error (`SO_ERROR`) as a guest errno; 0 for none.
    pub fn so_error(&self) -> u32 {
        let mut v = [0u8; 4];
        let _ = self.get_opt(ws::SOL_SOCKET, ws::SO_ERROR, &mut v);
        match i32::from_ne_bytes(v) {
            0 => 0,
            e => HOST.errno(e),
        }
    }
}

/// The host's IPv4 adapter addresses as `(up, loopback, address)` (`GetAdaptersAddresses`).
pub(crate) fn ipv4_interfaces() -> Vec<(bool, bool, Ipv4Addr)> {
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size = 16 * 1024u32;
    let mut buf: Vec<u64> = Vec::new();
    for _ in 0..4 {
        buf.resize((size as usize).div_ceil(8), 0);
        let p = buf.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY: `buf` (8-byte aligned) holds `size` bytes; ERROR_BUFFER_OVERFLOW updates `size`.
        match unsafe { GetAdaptersAddresses(u32::from(ws::AF_INET), flags, std::ptr::null(), p, &mut size) } {
            0 => return adapters(p),
            111 => continue, // ERROR_BUFFER_OVERFLOW
            _ => break,
        }
    }
    Vec::new()
}

fn adapters(mut p: *const IP_ADAPTER_ADDRESSES_LH) -> Vec<(bool, bool, Ipv4Addr)> {
    let mut out = Vec::new();
    // SAFETY: walking the linked lists GetAdaptersAddresses built inside the caller's buffer.
    while let Some(a) = unsafe { p.as_ref() } {
        let (up, lo) = (a.OperStatus == OPER_UP, a.IfType == IF_LOOPBACK);
        let mut u = a.FirstUnicastAddress;
        // SAFETY: as above.
        while let Some(ua) = unsafe { u.as_ref() } {
            let sa = ua.Address.lpSockaddr.cast::<ws::SOCKADDR_IN>();
            // SAFETY: as above; the family field is readable for every family.
            if let Some(sin) = unsafe { sa.as_ref() }.filter(|s| s.sin_family == ws::AF_INET) {
                // SAFETY: S_un is a plain union of the same 4 bytes.
                let raw = unsafe { sin.sin_addr.S_un.S_addr };
                out.push((up, lo, Ipv4Addr::from(u32::from_be(raw))));
            }
            u = ua.Next;
        }
        p = a.Next;
    }
    out
}

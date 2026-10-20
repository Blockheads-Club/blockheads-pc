//! POSIX host sockets (Linux, macOS): a non-blocking, close-on-exec descriptor per guest socket.
//!
//! Addresses are in the host's `sockaddr` layout and option numbers are the host's ([`super::abi`] translates);
//! errors come back as guest (Linux) `errno` values.

use super::abi::HOST;
use std::net::Ipv4Addr;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

/// One host socket, closed on drop.
pub(crate) struct Sock(OwnedFd);

fn err() -> u32 {
    HOST.errno(std::io::Error::last_os_error().raw_os_error().unwrap_or(0))
}

fn cvt(r: isize) -> Result<usize, u32> {
    if r < 0 {
        Err(err())
    } else {
        Ok(r as usize)
    }
}

fn storage() -> (libc::sockaddr_storage, libc::socklen_t) {
    // SAFETY: an all-zero sockaddr_storage is a valid (empty) address.
    (unsafe { std::mem::zeroed() }, std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t)
}

fn storage_bytes(ss: &libc::sockaddr_storage, len: libc::socklen_t) -> Vec<u8> {
    let n = (len as usize).min(std::mem::size_of::<libc::sockaddr_storage>());
    // SAFETY: `n` is within the storage.
    unsafe { std::slice::from_raw_parts((ss as *const libc::sockaddr_storage).cast::<u8>(), n) }.to_vec()
}

/// `send` flags every call gets: no `SIGPIPE` on a closed stream (macOS sets `SO_NOSIGPIPE` instead).
#[cfg(target_os = "linux")]
const SEND_FLAGS: i32 = libc::MSG_NOSIGNAL;
#[cfg(not(target_os = "linux"))]
const SEND_FLAGS: i32 = 0;

impl Sock {
    fn raw(&self) -> i32 {
        self.0.as_raw_fd()
    }

    /// A new socket of host `family`; `ty` is `SOCK_STREAM`/`SOCK_DGRAM` (equal on every host).
    pub fn open(family: i32, ty: i32, proto: i32) -> Result<Sock, u32> {
        #[cfg(target_os = "linux")]
        let ty = ty | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC;
        // SAFETY: plain system call.
        let fd = cvt(unsafe { libc::socket(family, ty, proto) } as isize)?;
        // SAFETY: a fresh descriptor nobody else owns.
        let s = Sock(unsafe { OwnedFd::from_raw_fd(fd as i32) });
        #[cfg(not(target_os = "linux"))]
        s.prepare();
        Ok(s)
    }

    /// Non-blocking, close-on-exec and no `SIGPIPE`: what Linux sets with `SOCK_*` flags and `MSG_NOSIGNAL`.
    #[cfg(not(target_os = "linux"))]
    fn prepare(&self) {
        let fd = self.raw();
        let one: libc::c_int = 1;
        // SAFETY: plain system calls on our own descriptor; `one` outlives the call.
        unsafe {
            libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK);
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_NOSIGPIPE, (&one as *const libc::c_int).cast(), 4);
        }
    }

    pub fn bind(&self, addr: &[u8]) -> Result<(), u32> {
        // SAFETY: `addr` is a readable buffer of the given length.
        cvt(unsafe { libc::bind(self.raw(), addr.as_ptr().cast(), addr.len() as libc::socklen_t) } as isize).map(drop)
    }

    pub fn listen(&self, backlog: i32) -> Result<(), u32> {
        // SAFETY: plain system call.
        cvt(unsafe { libc::listen(self.raw(), backlog) } as isize).map(drop)
    }

    /// Start connecting; a connection that is not made at once fails with `EINPROGRESS`.
    pub fn connect(&self, addr: &[u8]) -> Result<(), u32> {
        // SAFETY: `addr` is a readable buffer of the given length.
        cvt(unsafe { libc::connect(self.raw(), addr.as_ptr().cast(), addr.len() as libc::socklen_t) } as isize)
            .map(drop)
    }

    /// A pending connection (non-blocking itself) and the peer's address.
    pub fn accept(&self) -> Result<(Sock, Vec<u8>), u32> {
        let (mut ss, mut sl) = storage();
        let p = (&mut ss as *mut libc::sockaddr_storage).cast();
        // SAFETY: `ss`/`sl` are a valid address buffer and its length.
        #[cfg(target_os = "linux")]
        let fd =
            cvt(unsafe { libc::accept4(self.raw(), p, &mut sl, libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC) } as isize)?;
        // SAFETY: as above.
        #[cfg(not(target_os = "linux"))]
        let fd = cvt(unsafe { libc::accept(self.raw(), p, &mut sl) } as isize)?;
        // SAFETY: a fresh descriptor nobody else owns.
        let s = Sock(unsafe { OwnedFd::from_raw_fd(fd as i32) });
        #[cfg(not(target_os = "linux"))]
        s.prepare();
        Ok((s, storage_bytes(&ss, sl)))
    }

    /// Send `data` (to `to` when given) with host `flags`.
    pub fn send_to(&self, data: &[u8], flags: i32, to: Option<&[u8]>) -> Result<usize, u32> {
        let (dest, dlen) = to.map_or((std::ptr::null(), 0), |a| (a.as_ptr().cast(), a.len() as libc::socklen_t));
        // SAFETY: `data` and `dest` are readable buffers of the given lengths (or null/0).
        cvt(unsafe { libc::sendto(self.raw(), data.as_ptr().cast(), data.len(), flags | SEND_FLAGS, dest, dlen) })
    }

    /// Receive into `buf` with host `flags`; returns the byte count and the sender's address.
    pub fn recv_from(&self, buf: &mut [u8], flags: i32) -> Result<(usize, Vec<u8>), u32> {
        let (mut ss, mut sl) = storage();
        let p = (&mut ss as *mut libc::sockaddr_storage).cast();
        // SAFETY: `buf` and `ss` are writable buffers of the given lengths.
        let n = cvt(unsafe { libc::recvfrom(self.raw(), buf.as_mut_ptr().cast(), buf.len(), flags, p, &mut sl) })?;
        Ok((n, storage_bytes(&ss, sl)))
    }

    pub fn set_opt(&self, level: i32, name: i32, val: &[u8]) -> Result<(), u32> {
        // SAFETY: `val` is a readable buffer of the given length.
        let r = unsafe { libc::setsockopt(self.raw(), level, name, val.as_ptr().cast(), val.len() as libc::socklen_t) };
        cvt(r as isize).map(drop)
    }

    /// Read an option into `out`; returns its length.
    pub fn get_opt(&self, level: i32, name: i32, out: &mut [u8]) -> Result<usize, u32> {
        let mut len = out.len() as libc::socklen_t;
        // SAFETY: `out` is a writable buffer of `len` bytes.
        cvt(unsafe { libc::getsockopt(self.raw(), level, name, out.as_mut_ptr().cast(), &mut len) } as isize)?;
        Ok(len as usize)
    }

    /// The local (`peer` false) or remote address.
    pub fn name(&self, peer: bool) -> Result<Vec<u8>, u32> {
        let (mut ss, mut sl) = storage();
        let p = (&mut ss as *mut libc::sockaddr_storage).cast();
        // SAFETY: `ss`/`sl` are a valid address buffer and its length.
        let r = unsafe {
            if peer {
                libc::getpeername(self.raw(), p, &mut sl)
            } else {
                libc::getsockname(self.raw(), p, &mut sl)
            }
        };
        cvt(r as isize)?;
        Ok(storage_bytes(&ss, sl))
    }

    pub fn shutdown(&self, how: i32) -> Result<(), u32> {
        // SAFETY: plain system call.
        cvt(unsafe { libc::shutdown(self.raw(), how) } as isize).map(drop)
    }

    /// Readiness right now as epoll bits (`EPOLLIN` 1, `EPOLLOUT` 4, `EPOLLERR` 8, `EPOLLHUP` 0x10), asking for
    /// readability and/or writability.
    pub fn poll(&self, read: bool, write: bool) -> u32 {
        let events = if read { libc::POLLIN } else { 0 } | if write { libc::POLLOUT } else { 0 };
        let mut p = libc::pollfd { fd: self.raw(), events, revents: 0 };
        // SAFETY: one valid pollfd, zero timeout.
        unsafe { libc::poll(&mut p, 1, 0) };
        [(libc::POLLIN, 1), (libc::POLLOUT, 4), (libc::POLLERR, 8), (libc::POLLHUP, 0x10)]
            .iter()
            .filter(|(b, _)| p.revents & b != 0)
            .fold(0, |acc, (_, e)| acc | e)
    }

    /// Bytes waiting to be read (`FIONREAD`).
    pub fn pending(&self) -> u32 {
        let mut n: libc::c_int = 0;
        // SAFETY: FIONREAD writes one int.
        unsafe { libc::ioctl(self.raw(), libc::FIONREAD as _, &mut n) };
        n.max(0) as u32
    }

    /// The pending socket error (`SO_ERROR`) as a guest errno; 0 for none.
    pub fn so_error(&self) -> u32 {
        let mut v = [0u8; 4];
        let _ = self.get_opt(libc::SOL_SOCKET, libc::SO_ERROR, &mut v);
        match i32::from_ne_bytes(v) {
            0 => 0,
            e => HOST.errno(e),
        }
    }
}

/// The host's IPv4 interface addresses as `(up, loopback, address)`; empty when they cannot be listed.
pub(crate) fn ipv4_interfaces() -> Vec<(bool, bool, Ipv4Addr)> {
    let mut out = Vec::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `list`, released below with freeifaddrs.
    let ok = unsafe { libc::getifaddrs(&mut list) } == 0;
    let mut p = if ok { list } else { std::ptr::null_mut() };
    // SAFETY: walking the list getifaddrs made; every node and its address stay valid until freeifaddrs.
    while let Some(ifa) = unsafe { p.as_ref() } {
        let sa = ifa.ifa_addr as *const libc::sockaddr_in;
        // SAFETY: as above; a sockaddr's family field is readable for every family.
        if let Some(sin) = unsafe { sa.as_ref() }.filter(|a| i32::from(a.sin_family) == libc::AF_INET) {
            let flags = ifa.ifa_flags;
            out.push((
                flags & libc::IFF_UP as u32 != 0,
                flags & libc::IFF_LOOPBACK as u32 != 0,
                Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)),
            ));
        }
        p = ifa.ifa_next;
    }
    if ok {
        // SAFETY: the list from getifaddrs above, freed once.
        unsafe { libc::freeifaddrs(list) };
    }
    out
}

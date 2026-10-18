//! BSD sockets for the guest, backed by non-blocking host sockets.
//!
//! The ARM EABI and the host share the Linux socket ABI (family, option and flag numbers, `sockaddr_in` and
//! `sockaddr_in6` layouts), so addresses and option values pass through as bytes; what is rebuilt in guest memory
//! are the `msghdr`/`iovec` records and the resolver results (`addrinfo`, `hostent`) whose layouts differ.
//!
//! Host descriptors are always non-blocking. A blocking guest call retries in slices of [`SLICE`] and gives the
//! baton to other guest threads in between (`sched::block`), so it honours `SO_RCVTIMEO`/`SO_SNDTIMEO`. Readiness
//! for epoll/poll/select comes from a zero-timeout host `poll`; waiters on such descriptors wake every [`SLICE`].

use super::{Entry, Kind, Vfd, EMFILE, EPOLLERR, EPOLLHUP, EPOLLIN, EPOLLOUT};
use bhpc_bionic::guest::{cstr, rd, rd32, wr, wr32, wr_cstr};
use bhpc_bionic::layout::errno::{EBADF, EINTR, EINVAL};
use bhpc_bionic::misc::{fail, get_errno};
use bhpc_bionic::{lock, sched, Shared};
use bhpc_cpu::{Cpu, Stop};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Wake-up period of blocked socket calls and of epoll/poll/select over host sockets.
pub(super) const SLICE: Duration = Duration::from_millis(5);

const AF_INET: u32 = 2;
const AF_INET6: u32 = 10;
const SOCK_NONBLOCK: u32 = 0x800;
const SOCK_CLOEXEC: u32 = 0x80000;
const EFAULT: u32 = 14;
const ENOTSOCK: u32 = 88;
const EAFNOSUPPORT: u32 = 97;
const EINPROGRESS: u32 = 115;
const EAGAIN: u32 = 11;
const SOL_SOCKET: u32 = 1;
const SO_RCVTIMEO: u32 = 20;
const SO_SNDTIMEO: u32 = 21;
const MSG_DONTWAIT: u32 = 0x40;
/// `MSG_OOB | MSG_PEEK | MSG_TRUNC | MSG_DONTWAIT`: the flags passed on to the host.
const MSG_KEEP: u32 = 0x1 | 0x2 | 0x20 | 0x40;
const MSG_NOSIGNAL: i32 = 0x4000;
/// Largest single transfer.
const MAX_IO: u32 = 1 << 22;
const MAX_IOV: u32 = 1024;
const MAX_OPT: u32 = 256;
const STORAGE: u32 = 128;
const POLLIN: i16 = 1;
const POLLOUT: i16 = 4;
const EAI_FAMILY: u32 = 5;
const EAI_MEMORY: u32 = 6;
const EAI_NONAME: u32 = 8;
const EAI_SERVICE: u32 = 9;
const EAI_SOCKTYPE: u32 = 10;
const EAI_OVERFLOW: u32 = 14;
const AI_PASSIVE: u32 = 1;
const AI_CANONNAME: u32 = 2;
const AI_NUMERICHOST: u32 = 4;
/// `sizeof(struct addrinfo)` in bionic, followed in the same block by the `sockaddr`.
const AI_SIZE: u32 = 32;
const NODE_SIZE: u32 = AI_SIZE + 28;
const HOSTENT_BUF: u32 = 512;

type Res = Result<u32, u32>;
/// A decoded `msghdr` and its `(base, len)` vectors.
type Msg = ([u32; 7], Vec<(u32, u32)>);

/// A host socket owned by one or more descriptors (`dup`).
pub(super) struct HostSock {
    fd: OwnedFd,
    /// Receive and send timeouts in microseconds; 0 is "none".
    timeouts: [AtomicU64; 2],
}

fn errno() -> u32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32
}

fn cvt(r: isize) -> Res {
    if r < 0 {
        Err(errno())
    } else {
        Ok(r as u32)
    }
}

/// Finish a call: the value, or -1 with `errno`.
fn done(c: &mut Cpu, st: &Shared, r: Res) {
    match r {
        Ok(v) => c.ret_u32(v),
        Err(e) => fail(c, st, e),
    }
}

macro_rules! args {
    ($c:expr; $($n:ident),+) => {
        let mut a = $c.args();
        $(let $n = a.u32($c);)+
    };
}

impl HostSock {
    fn raw(&self) -> i32 {
        self.fd.as_raw_fd()
    }

    fn timeout(&self, write: bool) -> Option<Duration> {
        match self.timeouts[usize::from(write)].load(Ordering::Relaxed) {
            0 => None,
            us => Some(Duration::from_micros(us)),
        }
    }

    /// Zero-timeout host `poll` for `events`.
    fn poll(&self, events: i16) -> i16 {
        let mut p = libc::pollfd { fd: self.raw(), events, revents: 0 };
        unsafe { libc::poll(&mut p, 1, 0) };
        p.revents
    }

    /// Readiness as epoll bits (the `poll` and `epoll` flag values coincide).
    pub(super) fn ready(&self) -> u32 {
        self.poll(POLLIN | POLLOUT) as u32 & (EPOLLIN | EPOLLOUT | EPOLLERR | EPOLLHUP)
    }

    /// Bytes waiting to be read (`FIONREAD`).
    pub(super) fn pending(&self) -> u32 {
        let mut n: libc::c_int = 0;
        unsafe { libc::ioctl(self.raw(), libc::FIONREAD as _, &mut n) };
        n.max(0) as u32
    }

    fn so_error(&self) -> u32 {
        let (mut v, mut len) = (0 as libc::c_int, 4 as libc::socklen_t);
        unsafe {
            libc::getsockopt(
                self.raw(),
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&mut v as *mut libc::c_int).cast(),
                &mut len,
            )
        };
        v as u32
    }
}

/// Guest `sockaddr` bytes, checked for a family the host sockets serve.
fn guest_addr(c: &mut Cpu, p: u32, len: u32) -> Result<Vec<u8>, u32> {
    if !(2..=STORAGE).contains(&len) {
        return Err(EINVAL);
    }
    let a = rd(c, p, len).ok_or(EFAULT)?;
    match u32::from(u16::from_le_bytes([a[0], a[1]])) {
        AF_INET | AF_INET6 => Ok(a),
        _ => Err(EAFNOSUPPORT),
    }
}

/// Store a socket address through the guest's `(buf, *len)` pair: truncated to the room, the full size reported.
fn put_addr(c: &mut Cpu, buf: u32, lenp: u32, sa: &[u8]) -> Result<(), u32> {
    if buf == 0 {
        return Ok(());
    }
    let room = rd32(c, lenp).ok_or(EFAULT)?;
    let n = (room as usize).min(sa.len());
    if !wr(c, buf, &sa[..n]) || !wr32(c, lenp, sa.len() as u32) {
        return Err(EFAULT);
    }
    Ok(())
}

/// Give the baton to other guest threads until `until`; an error means the guest was stopped.
fn pause(c: &mut Cpu, st: &Shared, until: Instant, what: &str) -> Res {
    sched::block(c, st, Some(until), what).map(|_| 0).ok_or(EINTR)
}

fn storage() -> (libc::sockaddr_storage, libc::socklen_t) {
    (unsafe { std::mem::zeroed() }, std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t)
}

fn storage_bytes(ss: &libc::sockaddr_storage, len: libc::socklen_t) -> Vec<u8> {
    let n = (len as usize).min(STORAGE as usize);
    unsafe { std::slice::from_raw_parts((ss as *const libc::sockaddr_storage).cast::<u8>(), n) }.to_vec()
}

fn sockaddr_ptr(ss: &mut libc::sockaddr_storage) -> *mut libc::sockaddr {
    (ss as *mut libc::sockaddr_storage).cast()
}

impl Vfd {
    /// The host socket behind `fd` and the guest's `O_NONBLOCK` flag for it.
    pub(super) fn sock(&self, fd: u32) -> Result<(Arc<HostSock>, bool), u32> {
        match self.t().fds.get(&fd) {
            Some(Entry { kind: Kind::Net(s), nonblock, .. }) => Ok((s.clone(), *nonblock)),
            Some(_) => Err(ENOTSOCK),
            None => Err(EBADF),
        }
    }

    /// Run the non-blocking host call `op` until it stops answering `EAGAIN`; a blocking guest descriptor waits
    /// (up to its socket timeout) while other guest threads run.
    fn io(&self, c: &mut Cpu, st: &Shared, s: &HostSock, nb: bool, write: bool, op: &mut dyn FnMut() -> Res) -> Res {
        let limit = s.timeout(write).map(|d| Instant::now() + d);
        loop {
            match op() {
                Err(EAGAIN) if !nb => {
                    let now = Instant::now();
                    if limit.is_some_and(|l| now >= l) {
                        return Err(EAGAIN);
                    }
                    let until = limit.map_or(now + SLICE, |l| l.min(now + SLICE));
                    pause(c, st, until, "socket")?;
                }
                r => return r,
            }
        }
    }

    fn insert_sock(&self, fd: OwnedFd, nonblock: bool, cloexec: bool) -> Res {
        let s = Arc::new(HostSock { fd, timeouts: [AtomicU64::new(0), AtomicU64::new(0)] });
        self.t().insert(Kind::Net(s), nonblock, cloexec).ok_or(EMFILE)
    }

    pub(super) fn socket(&self, c: &mut Cpu, st: &Shared) {
        args!(c; domain, ty, proto);
        let r = (|| -> Res {
            if domain != AF_INET && domain != AF_INET6 {
                return Err(EAFNOSUPPORT);
            }
            let kind = (ty & 0xf) as i32 | SOCK_NONBLOCK as i32 | SOCK_CLOEXEC as i32;
            let h = cvt(unsafe { libc::socket(domain as i32, kind, proto as i32) } as isize)?;
            self.insert_sock(unsafe { OwnedFd::from_raw_fd(h as i32) }, ty & SOCK_NONBLOCK != 0, ty & SOCK_CLOEXEC != 0)
        })();
        done(c, st, r);
    }

    pub(super) fn bind(&self, c: &mut Cpu, st: &Shared) {
        args!(c; fd, addr, len);
        let r = (|| -> Res {
            let (s, _) = self.sock(fd)?;
            let a = guest_addr(c, addr, len)?;
            cvt(unsafe { libc::bind(s.raw(), a.as_ptr().cast(), a.len() as u32) } as isize)
        })();
        done(c, st, r);
    }

    pub(super) fn listen(&self, c: &mut Cpu, st: &Shared) {
        args!(c; fd, backlog);
        let r = self.sock(fd).and_then(|(s, _)| cvt(unsafe { libc::listen(s.raw(), backlog as i32) } as isize));
        done(c, st, r);
    }

    pub(super) fn connect(&self, c: &mut Cpu, st: &Shared) {
        args!(c; fd, addr, len);
        let r = (|| -> Res {
            let (s, nb) = self.sock(fd)?;
            let a = guest_addr(c, addr, len)?;
            if self.t().trace_net {
                eprintln!("[net] connect to {a:?}");
            }
            if unsafe { libc::connect(s.raw(), a.as_ptr().cast(), a.len() as u32) } == 0 {
                return Ok(0);
            }
            let e = errno();
            if e != EINPROGRESS || nb {
                return Err(e);
            }
            self.io(c, st, &s, false, true, &mut || if s.poll(POLLOUT) != 0 { Ok(0) } else { Err(EAGAIN) })?;
            match s.so_error() {
                0 => Ok(0),
                e => Err(e),
            }
        })();
        sched::notify(st);
        done(c, st, r);
    }

    pub(super) fn accept(&self, c: &mut Cpu, st: &Shared) {
        args!(c; fd, addr, alen);
        let r = self.accept_in(c, st, fd, addr, alen, 0);
        done(c, st, r);
    }

    pub(super) fn accept4(&self, c: &mut Cpu, st: &Shared) {
        args!(c; fd, addr, alen, flags);
        let r = self.accept_in(c, st, fd, addr, alen, flags);
        done(c, st, r);
    }

    fn accept_in(&self, c: &mut Cpu, st: &Shared, fd: u32, addr: u32, alen: u32, flags: u32) -> Res {
        let (s, nb) = self.sock(fd)?;
        let (mut ss, mut sl) = storage();
        let h = self.io(c, st, &s, nb, false, &mut || {
            cvt(
                unsafe { libc::accept4(s.raw(), sockaddr_ptr(&mut ss), &mut sl, (SOCK_NONBLOCK | SOCK_CLOEXEC) as i32) }
                    as isize,
            )
        })?;
        let owned = unsafe { OwnedFd::from_raw_fd(h as i32) };
        put_addr(c, addr, alen, &storage_bytes(&ss, sl))?;
        self.insert_sock(owned, flags & SOCK_NONBLOCK != 0, flags & SOCK_CLOEXEC != 0)
    }

    /// Send `data` (all of it on a blocking stream socket), to `to` when given.
    fn send_core(&self, c: &mut Cpu, st: &Shared, fd: u32, data: &[u8], flags: u32, to: Option<Vec<u8>>) -> Res {
        let (s, nb) = self.sock(fd)?;
        if self.t().trace_net {
            eprintln!("[net] send {} bytes to {to:?}: {:02x?}", data.len(), &data[..data.len().min(64)]);
        }
        let nb = nb || flags & MSG_DONTWAIT != 0;
        let fl = (flags & MSG_KEEP) as i32 | MSG_NOSIGNAL;
        let (dest, dlen) = to.as_ref().map_or((std::ptr::null(), 0), |a| (a.as_ptr().cast(), a.len() as u32));
        let (mut off, mut more, mut last) = (0usize, true, Ok(0));
        while more {
            let rest = &data[off..];
            last = self.io(c, st, &s, nb, true, &mut || {
                cvt(unsafe { libc::sendto(s.raw(), rest.as_ptr().cast(), rest.len(), fl, dest, dlen) })
            });
            if let Ok(n) = last {
                off += n as usize;
            }
            more = last.is_ok() && !nb && to.is_none() && off < data.len();
        }
        sched::notify(st);
        match last {
            Err(e) if off == 0 => Err(e),
            _ => Ok(off as u32),
        }
    }

    /// Receive up to `max` bytes; returns the data and the sender's address bytes.
    fn recv_core(&self, c: &mut Cpu, st: &Shared, fd: u32, max: u32, flags: u32) -> Result<(Vec<u8>, Vec<u8>), u32> {
        let (s, nb) = self.sock(fd)?;
        let nb = nb || flags & MSG_DONTWAIT != 0;
        let mut data = vec![0u8; max.min(MAX_IO) as usize];
        let (mut ss, mut sl) = storage();
        let n = self.io(c, st, &s, nb, false, &mut || {
            let (p, l) = (data.as_mut_ptr().cast(), data.len());
            cvt(unsafe { libc::recvfrom(s.raw(), p, l, (flags & MSG_KEEP) as i32, sockaddr_ptr(&mut ss), &mut sl) })
        })?;
        data.truncate(n as usize);
        Ok((data, storage_bytes(&ss, sl)))
    }

    /// `send`, `sendto` (`to` is the address argument position) share this.
    fn send_args(&self, c: &mut Cpu, st: &Shared, with_addr: bool) {
        args!(c; fd, buf, n, flags, addr, alen);
        if self.is_pair(fd) {
            return self.write(c, st);
        }
        let r = (|| -> Res {
            let data = rd(c, buf, n.min(MAX_IO)).ok_or(EFAULT)?;
            let to = if with_addr && addr != 0 { Some(guest_addr(c, addr, alen)?) } else { None };
            self.send_core(c, st, fd, &data, flags, to)
        })();
        done(c, st, r);
    }

    pub(super) fn send(&self, c: &mut Cpu, st: &Shared) {
        self.send_args(c, st, false);
    }

    pub(super) fn sendto(&self, c: &mut Cpu, st: &Shared) {
        self.send_args(c, st, true);
    }

    fn recv_args(&self, c: &mut Cpu, st: &Shared, with_addr: bool) {
        args!(c; fd, buf, n, flags, addr, alen);
        if self.is_pair(fd) {
            return self.read(c, st);
        }
        let r = (|| -> Res {
            let (data, from) = self.recv_core(c, st, fd, n, flags)?;
            if !wr(c, buf, &data) {
                return Err(EFAULT);
            }
            if with_addr {
                put_addr(c, addr, alen, &from)?;
            }
            Ok(data.len() as u32)
        })();
        done(c, st, r);
    }

    pub(super) fn recv(&self, c: &mut Cpu, st: &Shared) {
        self.recv_args(c, st, false);
    }

    pub(super) fn recvfrom(&self, c: &mut Cpu, st: &Shared) {
        self.recv_args(c, st, true);
    }

    /// Guest `msghdr` (name, namelen, iov, iovlen, ...) and its `iovec` array.
    fn msghdr(c: &mut Cpu, msg: u32) -> Result<Msg, u32> {
        let raw = rd(c, msg, 28).ok_or(EFAULT)?;
        let h: [u32; 7] =
            std::array::from_fn(|i| u32::from_le_bytes(raw[i * 4..i * 4 + 4].try_into().expect("4 bytes")));
        let cnt = h[3].min(MAX_IOV);
        let vecs = rd(c, h[2], cnt * 8).ok_or(EFAULT)?;
        let iov = vecs
            .chunks(8)
            .map(|v| {
                (u32::from_le_bytes(v[..4].try_into().expect("4")), u32::from_le_bytes(v[4..].try_into().expect("4")))
            })
            .collect();
        Ok((h, iov))
    }

    pub(super) fn sendmsg(&self, c: &mut Cpu, st: &Shared) {
        args!(c; fd, msg, flags);
        let r = (|| -> Res {
            let (h, iov) = Self::msghdr(c, msg)?;
            let mut data = Vec::new();
            for (base, len) in iov {
                data.extend(rd(c, base, len.min(MAX_IO)).ok_or(EFAULT)?);
            }
            let to = if h[0] != 0 { Some(guest_addr(c, h[0], h[1])?) } else { None };
            self.send_core(c, st, fd, &data, flags, to)
        })();
        done(c, st, r);
    }

    pub(super) fn recvmsg(&self, c: &mut Cpu, st: &Shared) {
        args!(c; fd, msg, flags);
        let r = (|| -> Res {
            let (h, iov) = Self::msghdr(c, msg)?;
            let total = iov.iter().fold(0u32, |t, v| t.saturating_add(v.1));
            let (data, from) = self.recv_core(c, st, fd, total, flags)?;
            let mut rest = &data[..];
            for (base, len) in iov {
                let k = rest.len().min(len as usize);
                if !wr(c, base, &rest[..k]) {
                    return Err(EFAULT);
                }
                rest = &rest[k..];
            }
            if h[0] != 0 {
                let k = from.len().min(h[1] as usize);
                if !wr(c, h[0], &from[..k]) {
                    return Err(EFAULT);
                }
            }
            // The record was readable above, so its tail is writable.
            wr32(c, msg + 4, if h[0] != 0 { from.len() as u32 } else { 0 });
            wr32(c, msg + 20, 0);
            wr32(c, msg + 24, 0);
            Ok(data.len() as u32)
        })();
        done(c, st, r);
    }

    /// True for one end of a virtual `socketpair`: stream I/O goes through `read`/`write`, options are accepted.
    fn is_pair(&self, fd: u32) -> bool {
        matches!(self.t().fds.get(&fd), Some(Entry { kind: Kind::Sock(..), .. }))
    }

    pub(super) fn setsockopt(&self, c: &mut Cpu, st: &Shared) {
        args!(c; fd, level, name, val, len);
        let r = (|| -> Res {
            if self.is_pair(fd) {
                return Ok(0);
            }
            let (s, _) = self.sock(fd)?;
            let v = rd(c, val, len.min(MAX_OPT)).ok_or(EFAULT)?;
            if level == SOL_SOCKET && (name == SO_RCVTIMEO || name == SO_SNDTIMEO) {
                if v.len() < 8 {
                    return Err(EINVAL);
                }
                let (sec, usec) = (
                    i32::from_le_bytes(v[..4].try_into().expect("4")),
                    i32::from_le_bytes(v[4..8].try_into().expect("4")),
                );
                let us = (i64::from(sec) * 1_000_000 + i64::from(usec)).max(0) as u64;
                s.timeouts[usize::from(name == SO_SNDTIMEO)].store(us, Ordering::Relaxed);
                return Ok(0);
            }
            cvt(unsafe { libc::setsockopt(s.raw(), level as i32, name as i32, v.as_ptr().cast(), v.len() as u32) }
                as isize)
        })();
        done(c, st, r);
    }

    pub(super) fn getsockopt(&self, c: &mut Cpu, st: &Shared) {
        args!(c; fd, level, name, val, lenp);
        let r = (|| -> Res {
            if self.is_pair(fd) {
                // Options of a virtual pair read as zero (`SO_ERROR` included).
                let room = rd32(c, lenp).ok_or(EFAULT)?.min(MAX_OPT);
                return if wr(c, val, &vec![0; room as usize]) { Ok(0) } else { Err(EFAULT) };
            }
            let (s, _) = self.sock(fd)?;
            let room = rd32(c, lenp).ok_or(EFAULT)?.min(MAX_OPT);
            let mut out = vec![0u8; room as usize];
            let mut got = room;
            if level == SOL_SOCKET && (name == SO_RCVTIMEO || name == SO_SNDTIMEO) {
                let us = s.timeouts[usize::from(name == SO_SNDTIMEO)].load(Ordering::Relaxed);
                let tv = [(us / 1_000_000) as u32, (us % 1_000_000) as u32];
                got = room.min(8);
                for (i, w) in tv.iter().enumerate().take(got as usize / 4) {
                    out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
                }
            } else {
                cvt(
                    unsafe { libc::getsockopt(s.raw(), level as i32, name as i32, out.as_mut_ptr().cast(), &mut got) }
                        as isize,
                )?;
            }
            if !wr(c, val, &out[..got as usize]) || !wr32(c, lenp, got) {
                return Err(EFAULT);
            }
            Ok(0)
        })();
        done(c, st, r);
    }

    fn name_of(&self, c: &mut Cpu, st: &Shared, peer: bool) {
        args!(c; fd, addr, lenp);
        let r = (|| -> Res {
            let (s, _) = self.sock(fd)?;
            let (mut ss, mut sl) = storage();
            let p = sockaddr_ptr(&mut ss);
            let res = unsafe {
                if peer {
                    libc::getpeername(s.raw(), p, &mut sl)
                } else {
                    libc::getsockname(s.raw(), p, &mut sl)
                }
            };
            cvt(res as isize)?;
            put_addr(c, addr, lenp, &storage_bytes(&ss, sl)).map(|()| 0)
        })();
        done(c, st, r);
    }

    pub(super) fn getsockname(&self, c: &mut Cpu, st: &Shared) {
        self.name_of(c, st, false);
    }

    pub(super) fn getpeername(&self, c: &mut Cpu, st: &Shared) {
        self.name_of(c, st, true);
    }

    pub(super) fn shutdown(&self, c: &mut Cpu, st: &Shared) {
        args!(c; fd, how);
        let r = match self.sock(fd) {
            Ok((s, _)) => cvt(unsafe { libc::shutdown(s.raw(), how as i32) } as isize),
            Err(_) if self.is_pair(fd) => Ok(0),
            Err(e) => Err(e),
        };
        sched::notify(st);
        done(c, st, r);
    }

    /// `read` on a socket descriptor; `None` when `fd` is not one.
    pub(super) fn net_read(&self, c: &mut Cpu, st: &Shared, fd: u32, buf: u32, n: u32) -> bool {
        if self.sock(fd).is_err() {
            return false;
        }
        let r = (|| -> Res {
            let (data, _) = self.recv_core(c, st, fd, n, 0)?;
            if wr(c, buf, &data) {
                Ok(data.len() as u32)
            } else {
                Err(EFAULT)
            }
        })();
        done(c, st, r);
        true
    }

    /// `write` on a socket descriptor; false when `fd` is not one.
    pub(super) fn net_write(&self, c: &mut Cpu, st: &Shared, fd: u32, buf: u32, n: u32) -> bool {
        if self.sock(fd).is_err() {
            return false;
        }
        let r = rd(c, buf, n.min(MAX_IO)).ok_or(EFAULT).and_then(|d| self.send_core(c, st, fd, &d, 0, None));
        done(c, st, r);
        true
    }

    /// `select`/`pselect` when every descriptor in the sets is ours; false means: not handled.
    fn select_in(&self, c: &mut Cpu, st: &Shared, pselect: bool) -> Option<bool> {
        args!(c; nfds, rp, wp, ep, tp);
        if nfds > 1024 {
            return Some(false);
        }
        let bytes = nfds.div_ceil(32) * 4;
        let mut sets: Vec<Vec<u8>> = Vec::new();
        for p in [rp, wp, ep] {
            if p == 0 {
                sets.push(vec![0; bytes as usize]);
            } else {
                sets.push(rd(c, p, bytes)?);
            }
        }
        let bit = |s: &Vec<u8>, i: u32| s[i as usize / 8] >> (i % 8) & 1 != 0;
        let wanted: Vec<u32> = (0..nfds).filter(|i| sets.iter().any(|s| bit(s, *i))).collect();
        if !wanted.iter().all(|fd| self.t().fds.contains_key(fd)) {
            return Some(false);
        }
        let deadline = if tp == 0 {
            None
        } else {
            let raw = rd(c, tp, 8)?;
            let sec = i32::from_le_bytes(raw[..4].try_into().expect("4")).max(0) as u64;
            let sub = i32::from_le_bytes(raw[4..].try_into().expect("4")).max(0) as u64;
            let ns = if pselect { sub } else { sub * 1000 };
            Some(Instant::now() + Duration::from_secs(sec) + Duration::from_nanos(ns))
        };
        let net = wanted.iter().any(|fd| self.t().is_net(*fd));
        loop {
            let mut out = [vec![0u8; bytes as usize], vec![0u8; bytes as usize], vec![0u8; bytes as usize]];
            let mut count = 0;
            {
                let t = self.t();
                for fd in &wanted {
                    let ready = t.ready(*fd);
                    for (k, want) in [EPOLLIN, EPOLLOUT].iter().enumerate() {
                        if bit(&sets[k], *fd) && ready & (want | EPOLLERR | EPOLLHUP) != 0 {
                            out[k][*fd as usize / 8] |= 1 << (fd % 8);
                            count += 1;
                        }
                    }
                }
            }
            let expired = deadline.is_some_and(|d| Instant::now() >= d);
            if count > 0 || expired {
                for (p, o) in [rp, wp, ep].iter().zip(&out) {
                    if *p != 0 {
                        wr(c, *p, o);
                    }
                }
                c.ret_u32(count);
                return Some(true);
            }
            sched::block(c, st, super::tick(deadline, net), "select")?;
        }
    }

    // ---------- resolver ----------

    pub(super) fn getaddrinfo(&self, c: &mut Cpu, st: &Shared) {
        args!(c; node, service, hints, out);
        let r = (|| -> Res {
            let host = if node == 0 {
                None
            } else {
                Some(String::from_utf8_lossy(&cstr(c, node).ok_or(EFAULT)?).into_owned())
            };
            let svc = if service == 0 {
                None
            } else {
                Some(String::from_utf8_lossy(&cstr(c, service).ok_or(EFAULT)?).into_owned())
            };
            let h =
                if hints == 0 { [0; 4] } else { std::array::from_fn(|i| rd32(c, hints + i as u32 * 4).unwrap_or(0)) };
            let list = resolve(host.as_deref(), svc.as_deref(), h);
            if self.t().trace_net {
                eprintln!("[net] getaddrinfo {host:?}:{svc:?} -> {:?}", list.as_ref().map(Vec::len));
            }
            let list = list?;
            let canon = if h[0] & AI_CANONNAME != 0 { host.as_deref().unwrap_or("").as_bytes() } else { &[] };
            let size = list.len() as u32 * NODE_SIZE + if canon.is_empty() { 0 } else { canon.len() as u32 + 1 };
            let base = Some(lock(st).heap.calloc(1, size)).filter(|b| *b != 0).ok_or(EAI_MEMORY)?;
            let cname = if canon.is_empty() { 0 } else { base + list.len() as u32 * NODE_SIZE };
            let mut mem = vec![0u8; size as usize];
            for (i, (ty, proto, sa)) in list.iter().enumerate() {
                let at = base + i as u32 * NODE_SIZE;
                let rec = &mut mem[i * NODE_SIZE as usize..][..NODE_SIZE as usize];
                let fam = u32::from(u16::from_le_bytes([sa[0], sa[1]]));
                let next = if i + 1 < list.len() { at + NODE_SIZE } else { 0 };
                let words =
                    [h[0], fam, *ty, *proto, sa.len() as u32, if i == 0 { cname } else { 0 }, at + AI_SIZE, next];
                for (k, w) in words.iter().enumerate() {
                    rec[k * 4..k * 4 + 4].copy_from_slice(&w.to_le_bytes());
                }
                rec[AI_SIZE as usize..][..sa.len()].copy_from_slice(sa);
            }
            mem[list.len() * NODE_SIZE as usize..][..canon.len()].copy_from_slice(canon);
            if !wr(c, base, &mem) || !wr32(c, out, base) {
                return Err(EFAULT);
            }
            Ok(0)
        })();
        c.ret_u32(match r {
            Ok(v) | Err(v) => v,
        });
    }

    pub(super) fn freeaddrinfo(&self, c: &mut Cpu, st: &Shared) {
        let p = c.args().u32(c);
        let r = catch_unwind(AssertUnwindSafe(|| lock(st).heap.free(p)));
        if r.is_err() {
            c.request_stop(Stop::Host("freeaddrinfo: pointer not from getaddrinfo".into()));
        }
    }

    /// `gethostbyname2`: one static `hostent` in guest memory, as in libc.
    fn hostent(&self, c: &mut Cpu, st: &Shared, name: u32, af: u32) {
        let Some(raw) = cstr(c, name) else { return };
        let text = String::from_utf8_lossy(&raw).into_owned();
        let found = match resolve(Some(&text), None, [0, af, 1, 0]) {
            Ok(l) => l.into_iter().next(),
            Err(_) => None,
        };
        let Some((_, _, sa)) = found else { return c.ret_u32(0) };
        let addr: &[u8] = if af == AF_INET { &sa[4..8] } else { &sa[8..24] };
        let buf = {
            let mut t = self.t();
            if t.hostent == 0 {
                t.hostent = lock(st).heap.calloc(1, HOSTENT_BUF);
            }
            t.hostent
        };
        let name_bytes = &text.as_bytes()[..text.len().min(HOSTENT_BUF as usize - 49)];
        let mut mem = vec![0u8; HOSTENT_BUF as usize];
        // hostent { name, aliases, addrtype, length, addr_list }, then aliases[1] @20, addr_list[2] @24,
        // the address @32 (16 bytes) and the name @48.
        let words = [buf + 48, buf + 20, af, addr.len() as u32, buf + 24, 0, buf + 32, 0];
        for (k, w) in words.iter().enumerate() {
            mem[k * 4..k * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        mem[32..32 + addr.len()].copy_from_slice(addr);
        mem[48..48 + name_bytes.len()].copy_from_slice(name_bytes);
        if wr(c, buf, &mem) {
            c.ret_u32(buf);
        }
    }

    pub(super) fn gethostbyname(&self, c: &mut Cpu, st: &Shared) {
        let name = c.args().u32(c);
        self.hostent(c, st, name, AF_INET);
    }

    pub(super) fn gethostbyname2(&self, c: &mut Cpu, st: &Shared) {
        args!(c; name, af);
        self.hostent(c, st, name, af);
    }

    pub(super) fn getnameinfo(&self, c: &mut Cpu, _st: &Shared) {
        args!(c; sa, salen, host, hostlen, serv, servlen);
        let r = (|| -> Res {
            let a = guest_addr(c, sa, salen).map_err(|_| EAI_FAMILY)?;
            let port = u16::from_be_bytes([a[2], a[3]]);
            let ip = if a[0] == AF_INET as u8 && a.len() >= 8 {
                IpAddr::V4(Ipv4Addr::new(a[4], a[5], a[6], a[7]))
            } else if a.len() >= 24 {
                IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(&a[8..24]).expect("16 bytes")))
            } else {
                return Err(EAI_FAMILY);
            };
            for (buf, len, text) in [(host, hostlen, ip.to_string()), (serv, servlen, port.to_string())] {
                if buf == 0 || len == 0 {
                    continue;
                }
                if text.len() as u32 + 1 > len {
                    return Err(EAI_OVERFLOW);
                }
                if !wr_cstr(c, buf, text.as_bytes()) {
                    return Err(EFAULT);
                }
            }
            Ok(0)
        })();
        c.ret_u32(match r {
            Ok(v) | Err(v) => v,
        });
    }
}

/// Ports of the few service names the resolver knows.
const SERVICES: &[(&str, u16)] = &[("http", 80), ("https", 443), ("ftp", 21), ("domain", 53), ("ntp", 123)];

/// Resolve a `getaddrinfo` request to `(socktype, protocol, sockaddr bytes)` entries; the error is an `EAI_*` code.
fn resolve(host: Option<&str>, service: Option<&str>, hints: [u32; 4]) -> Result<Vec<(u32, u32, Vec<u8>)>, u32> {
    let [flags, family, socktype, protocol] = hints;
    if !matches!(family, 0 | AF_INET | AF_INET6) {
        return Err(EAI_FAMILY);
    }
    let types: Vec<(u32, u32)> = match socktype {
        0 => vec![(1, 6), (2, 17)],
        1 => vec![(1, if protocol == 0 { 6 } else { protocol })],
        2 => vec![(2, if protocol == 0 { 17 } else { protocol })],
        _ => return Err(EAI_SOCKTYPE),
    };
    let port = match service.unwrap_or("") {
        "" => 0,
        s => match s.parse::<u16>() {
            Ok(p) => p,
            Err(_) => SERVICES.iter().find(|(n, _)| *n == s).map(|(_, p)| *p).ok_or(EAI_SERVICE)?,
        },
    };
    let ips: Vec<IpAddr> = match host {
        None if flags & AI_PASSIVE != 0 => vec![Ipv4Addr::UNSPECIFIED.into(), Ipv6Addr::UNSPECIFIED.into()],
        None => vec![Ipv4Addr::LOCALHOST.into(), Ipv6Addr::LOCALHOST.into()],
        Some(h) => match h.parse::<IpAddr>() {
            Ok(ip) => vec![ip],
            Err(_) if flags & AI_NUMERICHOST != 0 => return Err(EAI_NONAME),
            Err(_) => match (h, 0).to_socket_addrs() {
                Ok(it) => it.map(|s| s.ip()).collect(),
                Err(_) => return Err(EAI_NONAME),
            },
        },
    };
    let mut out = Vec::new();
    for ip in ips.iter().filter(|ip| family == 0 || (family == AF_INET) == ip.is_ipv4()) {
        for (ty, proto) in &types {
            out.push((*ty, *proto, sockaddr_bytes(*ip, port)));
        }
    }
    if out.is_empty() {
        return Err(EAI_NONAME);
    }
    Ok(out)
}

/// `sockaddr_in` (16 bytes) or `sockaddr_in6` (28 bytes) for `ip:port`.
fn sockaddr_bytes(ip: IpAddr, port: u16) -> Vec<u8> {
    let mut v = Vec::new();
    match ip {
        IpAddr::V4(a) => {
            v.extend((AF_INET as u16).to_le_bytes());
            v.extend(port.to_be_bytes());
            v.extend(a.octets());
            v.extend([0; 8]);
        }
        IpAddr::V6(a) => {
            v.extend((AF_INET6 as u16).to_le_bytes());
            v.extend(port.to_be_bytes());
            v.extend([0; 4]);
            v.extend(a.octets());
            v.extend([0; 4]);
        }
    }
    v
}

type NetFn = fn(&Vfd, &mut Cpu, &Shared);

const TABLE: &[(&str, NetFn)] = &[
    ("socket", Vfd::socket),
    ("bind", Vfd::bind),
    ("listen", Vfd::listen),
    ("connect", Vfd::connect),
    ("accept", Vfd::accept),
    ("accept4", Vfd::accept4),
    ("send", Vfd::send),
    ("sendto", Vfd::sendto),
    ("recv", Vfd::recv),
    ("recvfrom", Vfd::recvfrom),
    ("sendmsg", Vfd::sendmsg),
    ("recvmsg", Vfd::recvmsg),
    ("setsockopt", Vfd::setsockopt),
    ("getsockopt", Vfd::getsockopt),
    ("getsockname", Vfd::getsockname),
    ("getpeername", Vfd::getpeername),
    ("shutdown", Vfd::shutdown),
    ("getaddrinfo", Vfd::getaddrinfo),
    ("freeaddrinfo", Vfd::freeaddrinfo),
    ("gethostbyname", Vfd::gethostbyname),
    ("gethostbyname2", Vfd::gethostbyname2),
    ("getnameinfo", Vfd::getnameinfo),
];

impl Vfd {
    /// Register the socket thunks (they replace bionic's failing stubs).
    pub(super) fn install_net(&self, cpu: &Cpu, st: &Shared) {
        for &(name, f) in TABLE {
            let (v, s) = (self.clone(), st.clone());
            let thunk = move |c: &mut Cpu| {
                let args = [c.reg(0), c.reg(1), c.reg(2), c.reg(3)];
                f(&v, c, &s);
                if v.t().trace_net {
                    eprintln!("[net] {name}({args:x?}) = {} errno {}", c.reg(0) as i32, get_errno(c, &s));
                }
            };
            let _ = cpu.register_shared_thunk(name, Arc::new(thunk));
        }
        for (name, pselect) in [("select", false), ("pselect", true)] {
            let (v, s, inner) = (self.clone(), st.clone(), bhpc_bionic::builtin(name, st));
            let _ = cpu.register_shared_thunk(
                name,
                Arc::new(move |c: &mut Cpu| {
                    if v.select_in(c, &s, pselect) == Some(false) {
                        if let Some(f) = &inner {
                            f(c);
                        }
                    }
                }),
            );
        }
    }
}

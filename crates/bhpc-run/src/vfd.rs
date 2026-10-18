//! Virtual kernel objects the guest's libdispatch and CoreFoundation need and the file layer does not have:
//! pipes, socket pairs, eventfds and epoll instances.
//!
//! They live in a table of their own with descriptor numbers from [`FD_BASE`] up (below `FD_SETSIZE`), so they never
//! collide with the descriptors of `bhpc-bionic`'s file table. Thunks named like the libc functions are registered
//! before the libraries are linked; a call on a descriptor this table does not own is forwarded to the bionic
//! implementation of the same function. Blocking calls give the baton to other guest threads (`sched::block`).

use bhpc_bionic::guest::{rd, rd32, wr, wr32};
use bhpc_bionic::layout::errno;
use bhpc_bionic::misc::fail;
use bhpc_bionic::{sched, Shared};
use bhpc_cpu::Cpu;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

#[cfg_attr(not(unix), path = "vfd/net_stub.rs")]
mod net;

/// First descriptor number handed out.
pub const FD_BASE: u32 = 600;

const RLIMIT_NOFILE: u32 = 7;
const EPIPE: u32 = 32;
const EEXIST: u32 = 17;
const EMFILE: u32 = 24;
const O_NONBLOCK: u32 = 0x800;
const F_DUPFD: u32 = 0;
const F_GETFD: u32 = 1;
const F_SETFD: u32 = 2;
const F_GETFL: u32 = 3;
const F_SETFL: u32 = 4;
const FIONREAD: u32 = 0x541B;
const FIONBIO: u32 = 0x5421;
const EPOLL_CTL_ADD: u32 = 1;
const EPOLL_CTL_DEL: u32 = 2;
const EPOLL_CTL_MOD: u32 = 3;
const EPOLLIN: u32 = 1;
const EPOLLOUT: u32 = 4;
const EPOLLERR: u32 = 8;
const EPOLLHUP: u32 = 0x10;
const EPOLLONESHOT: u32 = 1 << 30;
/// `sizeof(struct epoll_event)` on ARM: `uint32 events; uint64 data` with 8-byte alignment.
const EPOLL_EVENT_SIZE: u32 = 16;
const POLLIN: u16 = 1;
const POLLOUT: u16 = 4;
const POLLHUP: u16 = 0x10;

#[derive(Default)]
struct Chan {
    buf: VecDeque<u8>,
    readers: u32,
    writers: u32,
}

#[derive(Clone)]
enum Kind {
    /// Read end of channel `n`.
    PipeR(usize),
    /// Write end of channel `n`.
    PipeW(usize),
    /// Socket pair end: reads channel `.0`, writes channel `.1`.
    Sock(usize, usize),
    /// Counter of an `eventfd`.
    Event(usize),
    Epoll,
    /// A host socket (see [`net`]).
    Net(Arc<net::HostSock>),
}

struct Entry {
    kind: Kind,
    nonblock: bool,
    cloexec: bool,
}

#[derive(Clone, Copy)]
struct Watch {
    events: u32,
    data: u64,
}

#[derive(Default)]
struct Table {
    fds: HashMap<u32, Entry>,
    chans: Vec<Chan>,
    counters: Vec<u64>,
    epolls: HashMap<u32, HashMap<u32, Watch>>,
    /// Guest address of the static `hostent` of `gethostbyname`.
    hostent: u32,
    /// Log every socket call (`BHPC_TRACE_NET` in the environment, or [`Vfd::set_trace_net`]).
    trace_net: bool,
}

/// Shared handle to the table.
#[derive(Clone, Default)]
pub struct Vfd(Arc<Mutex<Table>>);

impl Table {
    fn alloc_fd(&self, from: u32) -> Option<u32> {
        (from.max(FD_BASE)..1024).find(|n| !self.fds.contains_key(n))
    }

    fn new_chan(&mut self) -> usize {
        self.chans.push(Chan::default());
        self.chans.len() - 1
    }

    fn insert(&mut self, kind: Kind, nonblock: bool, cloexec: bool) -> Option<u32> {
        self.insert_from(FD_BASE, kind, nonblock, cloexec)
    }

    /// Like [`Table::insert`] with the lowest free descriptor number `>= from`.
    fn insert_from(&mut self, from: u32, kind: Kind, nonblock: bool, cloexec: bool) -> Option<u32> {
        let fd = self.alloc_fd(from)?;
        match &kind {
            Kind::PipeR(c) => self.chans[*c].readers += 1,
            Kind::PipeW(c) => self.chans[*c].writers += 1,
            Kind::Sock(r, w) => {
                self.chans[*r].readers += 1;
                self.chans[*w].writers += 1;
            }
            Kind::Event(_) | Kind::Epoll | Kind::Net(_) => {}
        }
        if matches!(kind, Kind::Epoll) {
            self.epolls.insert(fd, HashMap::new());
        }
        self.fds.insert(fd, Entry { kind, nonblock, cloexec });
        Some(fd)
    }

    /// Readiness bits (`EPOLLIN | EPOLLOUT | EPOLLHUP`) of `fd`; descriptors of other tables are always ready.
    fn ready(&self, fd: u32) -> u32 {
        let Some(e) = self.fds.get(&fd) else { return EPOLLIN | EPOLLOUT };
        match &e.kind {
            Kind::PipeR(c) => self.in_bits(*c),
            Kind::PipeW(c) => EPOLLOUT | if self.chans[*c].readers == 0 { EPOLLERR } else { 0 },
            Kind::Sock(r, w) => self.in_bits(*r) | EPOLLOUT | if self.chans[*w].readers == 0 { EPOLLERR } else { 0 },
            Kind::Event(n) => EPOLLOUT | if self.counters[*n] > 0 { EPOLLIN } else { 0 },
            Kind::Epoll => 0,
            Kind::Net(s) => s.ready(),
        }
    }

    fn is_net(&self, fd: u32) -> bool {
        matches!(self.fds.get(&fd), Some(Entry { kind: Kind::Net(_), .. }))
    }

    fn in_bits(&self, c: usize) -> u32 {
        let ch = &self.chans[c];
        (if ch.buf.is_empty() { 0 } else { EPOLLIN }) | if ch.writers == 0 { EPOLLHUP | EPOLLIN } else { 0 }
    }

    fn close(&mut self, fd: u32) -> bool {
        let Some(e) = self.fds.remove(&fd) else { return false };
        match e.kind {
            Kind::PipeR(c) => self.chans[c].readers -= 1,
            Kind::PipeW(c) => self.chans[c].writers -= 1,
            Kind::Sock(r, w) => {
                self.chans[r].readers -= 1;
                self.chans[w].writers -= 1;
            }
            Kind::Event(_) | Kind::Epoll | Kind::Net(_) => {}
        }
        self.epolls.remove(&fd);
        for w in self.epolls.values_mut() {
            w.remove(&fd);
        }
        true
    }
}

/// Wake-up deadline for a wait that may involve host sockets (nothing notifies the scheduler about those).
fn tick(deadline: Option<Instant>, net: bool) -> Option<Instant> {
    if !net {
        return deadline;
    }
    let next = Instant::now() + net::SLICE;
    Some(deadline.map_or(next, |d| d.min(next)))
}

fn deadline_of(timeout_ms: i32) -> Option<Instant> {
    (timeout_ms >= 0).then(|| Instant::now() + Duration::from_millis(timeout_ms as u64))
}

type Thunk = Arc<dyn Fn(&mut Cpu) + Send + Sync>;

impl Vfd {
    fn t(&self) -> std::sync::MutexGuard<'_, Table> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Switch the socket call log on or off.
    pub fn set_trace_net(&self, on: bool) {
        self.t().trace_net = on;
    }

    /// Number of descriptors this table owns (tests, diagnostics).
    pub fn open_count(&self) -> usize {
        self.t().fds.len()
    }

    /// Register the thunks. `forward` names the functions that fall back to the bionic implementation.
    pub fn install(&self, cpu: &Cpu, st: &Shared) {
        let reg = |name: &'static str, owned: Thunk| {
            let inner = bhpc_bionic::builtin(name, st);
            // A table without the bionic fallback still serves its own descriptors.
            let f: Thunk = match inner {
                Some(inner) => Arc::new(move |c: &mut Cpu| {
                    let fd = c.reg(0);
                    if (FD_BASE..1024).contains(&fd) {
                        owned(c)
                    } else {
                        inner(c)
                    }
                }),
                None => owned,
            };
            let _ = cpu.register_shared_thunk(name, f);
        };
        let new = |name: &'static str, f: Thunk| {
            let _ = cpu.register_shared_thunk(name, f);
        };
        self.install_net(cpu, st);
        let (v, s) = (self.clone(), st.clone());
        new("pipe", Arc::new(move |c| v.pipe(c, &s)));
        let (v, s) = (self.clone(), st.clone());
        new("pipe2", Arc::new(move |c| v.pipe(c, &s)));
        let (v, s) = (self.clone(), st.clone());
        new("socketpair", Arc::new(move |c| v.socketpair(c, &s)));
        let (v, s) = (self.clone(), st.clone());
        new("eventfd", Arc::new(move |c| v.eventfd(c, &s)));
        let (v, s) = (self.clone(), st.clone());
        new("epoll_create", Arc::new(move |c| v.epoll_create(c, &s)));
        let (v, s) = (self.clone(), st.clone());
        new("epoll_create1", Arc::new(move |c| v.epoll_create(c, &s)));
        let (v, s) = (self.clone(), st.clone());
        reg("epoll_ctl", Arc::new(move |c| v.epoll_ctl(c, &s)));
        let (v, s) = (self.clone(), st.clone());
        reg("epoll_wait", Arc::new(move |c| v.epoll_wait(c, &s)));
        let (v, s) = (self.clone(), st.clone());
        reg("epoll_pwait", Arc::new(move |c| v.epoll_wait(c, &s)));
        let (v, s) = (self.clone(), st.clone());
        reg("read", Arc::new(move |c| v.read(c, &s)));
        let (v, s) = (self.clone(), st.clone());
        reg("write", Arc::new(move |c| v.write(c, &s)));
        let (v, s) = (self.clone(), st.clone());
        reg("close", Arc::new(move |c| v.close(c, &s)));
        let (v, s) = (self.clone(), st.clone());
        reg("dup", Arc::new(move |c| v.dup(c, &s)));
        let (v, s) = (self.clone(), st.clone());
        reg("fcntl", Arc::new(move |c| v.fcntl(c, &s)));
        let (v, s) = (self.clone(), st.clone());
        reg("ioctl", Arc::new(move |c| v.ioctl(c, &s)));
        // Descriptor tables are sized by `RLIMIT_NOFILE`: "unlimited" would ask libkqueue for a 4 GiB map.
        let inner = bhpc_bionic::builtin("getrlimit", st);
        new(
            "getrlimit",
            Arc::new(move |c: &mut Cpu| {
                let (res, out) = {
                    let mut a = c.args();
                    (a.u32(c), a.u32(c))
                };
                if res == RLIMIT_NOFILE {
                    if wr32(c, out, 1024) && wr32(c, out + 4, 4096) {
                        c.ret_u32(0);
                    }
                } else if let Some(f) = &inner {
                    f(c);
                }
            }),
        );
        // `poll` is not forwarded by descriptor (its first argument is an array), see `poll`.
        let (v, s) = (self.clone(), st.clone());
        let inner = bhpc_bionic::builtin("poll", st);
        new(
            "poll",
            Arc::new(move |c| {
                if !v.poll(c, &s) {
                    if let Some(f) = &inner {
                        f(c);
                    }
                }
            }),
        );
    }

    fn pipe(&self, c: &mut Cpu, st: &Shared) {
        let (out, flags) = {
            let mut a = c.args();
            (a.u32(c), a.u32(c))
        };
        let mut t = self.t();
        let ch = t.new_chan();
        let nb = flags & O_NONBLOCK != 0;
        let (r, w) = (t.insert(Kind::PipeR(ch), nb, false), t.insert(Kind::PipeW(ch), nb, false));
        drop(t);
        match (r, w) {
            (Some(r), Some(w)) => {
                if wr32(c, out, r) && wr32(c, out + 4, w) {
                    c.ret_u32(0);
                }
            }
            _ => {
                self.t().close(r.unwrap_or(0));
                fail(c, st, EMFILE);
            }
        }
    }

    fn socketpair(&self, c: &mut Cpu, st: &Shared) {
        let out = {
            let mut a = c.args();
            let (_domain, _ty, _proto) = (a.u32(c), a.u32(c), a.u32(c));
            a.u32(c)
        };
        let mut t = self.t();
        let (ab, ba) = (t.new_chan(), t.new_chan());
        let (x, y) = (t.insert(Kind::Sock(ba, ab), false, false), t.insert(Kind::Sock(ab, ba), false, false));
        drop(t);
        match (x, y) {
            (Some(x), Some(y)) => {
                if wr32(c, out, x) && wr32(c, out + 4, y) {
                    c.ret_u32(0);
                }
            }
            _ => {
                self.t().close(x.unwrap_or(0));
                fail(c, st, EMFILE);
            }
        }
    }

    fn eventfd(&self, c: &mut Cpu, st: &Shared) {
        let (init, flags) = {
            let mut a = c.args();
            (a.u32(c), a.u32(c))
        };
        let mut t = self.t();
        t.counters.push(u64::from(init));
        let n = t.counters.len() - 1;
        let fd = t.insert(Kind::Event(n), flags & O_NONBLOCK != 0, false);
        drop(t);
        match fd {
            Some(fd) => c.ret_u32(fd),
            None => fail(c, st, EMFILE),
        }
    }

    fn epoll_create(&self, c: &mut Cpu, st: &Shared) {
        let fd = self.t().insert(Kind::Epoll, false, false);
        match fd {
            Some(fd) => c.ret_u32(fd),
            None => fail(c, st, EMFILE),
        }
    }

    fn epoll_ctl(&self, c: &mut Cpu, st: &Shared) {
        let (ep, op, fd, ev) = {
            let mut a = c.args();
            (a.u32(c), a.u32(c), a.u32(c), a.u32(c))
        };
        let watch = if op == EPOLL_CTL_DEL {
            Watch { events: 0, data: 0 }
        } else {
            let (Some(events), Some(lo), Some(hi)) = (rd32(c, ev), rd32(c, ev + 8), rd32(c, ev + 12)) else { return };
            Watch { events, data: u64::from(lo) | u64::from(hi) << 32 }
        };
        let mut t = self.t();
        let Some(set) = t.epolls.get_mut(&ep) else {
            drop(t);
            return fail(c, st, errno::EBADF);
        };
        let err = match op {
            EPOLL_CTL_ADD if set.contains_key(&fd) => Some(EEXIST),
            EPOLL_CTL_ADD => {
                set.insert(fd, watch);
                None
            }
            EPOLL_CTL_MOD if set.contains_key(&fd) => {
                set.insert(fd, watch);
                None
            }
            EPOLL_CTL_DEL if set.remove(&fd).is_some() => None,
            EPOLL_CTL_MOD | EPOLL_CTL_DEL => Some(errno::ENOENT),
            _ => Some(errno::EINVAL),
        };
        drop(t);
        match err {
            None => {
                sched::notify(st);
                c.ret_u32(0);
            }
            Some(e) => fail(c, st, e),
        }
    }

    fn epoll_wait(&self, c: &mut Cpu, st: &Shared) {
        let (ep, buf, max, timeout) = {
            let mut a = c.args();
            (a.u32(c), a.u32(c), a.u32(c), a.u32(c) as i32)
        };
        if max == 0 || max > 4096 {
            return fail(c, st, errno::EINVAL);
        }
        let deadline = deadline_of(timeout);
        loop {
            let mut ready = Vec::new();
            let net;
            {
                let mut t = self.t();
                let Some(set) = t.epolls.get(&ep).cloned() else {
                    drop(t);
                    return fail(c, st, errno::EBADF);
                };
                let mut fds: Vec<u32> = set.keys().copied().collect();
                fds.sort_unstable();
                net = fds.iter().any(|fd| t.is_net(*fd));
                for fd in fds {
                    let w = set[&fd];
                    let bits = t.ready(fd) & (w.events | EPOLLERR | EPOLLHUP);
                    if bits != 0 && ready.len() < max as usize {
                        ready.push((bits, w.data));
                        if w.events & EPOLLONESHOT != 0 {
                            if let Some(s) = t.epolls.get_mut(&ep) {
                                s.insert(fd, Watch { events: 0, data: w.data });
                            }
                        }
                    }
                }
            }
            if !ready.is_empty() {
                for (i, (bits, data)) in ready.iter().enumerate() {
                    let at = buf + i as u32 * EPOLL_EVENT_SIZE;
                    let mut rec = [0u8; EPOLL_EVENT_SIZE as usize];
                    rec[..4].copy_from_slice(&bits.to_le_bytes());
                    rec[8..].copy_from_slice(&data.to_le_bytes());
                    if !wr(c, at, &rec) {
                        return;
                    }
                }
                return c.ret_u32(ready.len() as u32);
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                return c.ret_u32(0);
            }
            if sched::block(c, st, tick(deadline, net), "epoll_wait").is_none() {
                return;
            }
        }
    }

    fn read(&self, c: &mut Cpu, st: &Shared) {
        let (fd, buf, n) = {
            let mut a = c.args();
            (a.u32(c), a.u32(c), a.u32(c))
        };
        if self.net_read(c, st, fd, buf, n) {
            return;
        }
        loop {
            let got: Result<Vec<u8>, u32> = {
                let mut t = self.t();
                let Some(e) = t.fds.get(&fd) else {
                    drop(t);
                    return fail(c, st, errno::EBADF);
                };
                let (kind, nb) = (e.kind.clone(), e.nonblock);
                match kind {
                    Kind::PipeR(ch) | Kind::Sock(ch, _) => {
                        let chan = &mut t.chans[ch];
                        if !chan.buf.is_empty() {
                            let k = (n as usize).min(chan.buf.len());
                            Ok(chan.buf.drain(..k).collect())
                        } else if chan.writers == 0 {
                            Ok(Vec::new())
                        } else if nb {
                            Err(errno::EAGAIN)
                        } else {
                            Err(0)
                        }
                    }
                    Kind::Event(i) => {
                        if n < 8 {
                            Err(errno::EINVAL)
                        } else if t.counters[i] > 0 {
                            let v = std::mem::take(&mut t.counters[i]);
                            Ok(v.to_le_bytes().to_vec())
                        } else if nb {
                            Err(errno::EAGAIN)
                        } else {
                            Err(0)
                        }
                    }
                    Kind::PipeW(_) | Kind::Epoll | Kind::Net(_) => Err(errno::EBADF),
                }
            };
            match got {
                Ok(data) => {
                    if !data.is_empty() {
                        sched::notify(st);
                    }
                    if wr(c, buf, &data) {
                        c.ret_u32(data.len() as u32);
                    }
                    return;
                }
                Err(0) => {
                    if sched::block(c, st, None, "pipe read").is_none() {
                        return;
                    }
                }
                Err(e) => return fail(c, st, e),
            }
        }
    }

    fn write(&self, c: &mut Cpu, st: &Shared) {
        let (fd, buf, n) = {
            let mut a = c.args();
            (a.u32(c), a.u32(c), a.u32(c))
        };
        if self.net_write(c, st, fd, buf, n) {
            return;
        }
        let Some(data) = rd(c, buf, n) else { return };
        let r: Result<(), u32> = {
            let mut t = self.t();
            match t.fds.get(&fd).map(|e| e.kind.clone()) {
                Some(Kind::PipeW(ch) | Kind::Sock(_, ch)) => {
                    if t.chans[ch].readers == 0 {
                        Err(EPIPE)
                    } else {
                        t.chans[ch].buf.extend(data.iter());
                        Ok(())
                    }
                }
                Some(Kind::Event(i)) => {
                    if n == 8 {
                        let v = u64::from_le_bytes(data[..8].try_into().expect("8 bytes"));
                        t.counters[i] = t.counters[i].saturating_add(v);
                        Ok(())
                    } else {
                        Err(errno::EINVAL)
                    }
                }
                _ => Err(errno::EBADF),
            }
        };
        match r {
            Ok(()) => {
                sched::notify(st);
                c.ret_u32(n);
            }
            Err(e) => fail(c, st, e),
        }
    }

    fn close(&self, c: &mut Cpu, st: &Shared) {
        let fd = c.args().u32(c);
        if self.t().close(fd) {
            sched::notify(st);
            c.ret_u32(0);
        } else {
            fail(c, st, errno::EBADF);
        }
    }

    fn dup(&self, c: &mut Cpu, st: &Shared) {
        let fd = c.args().u32(c);
        match self.dup_from(fd, FD_BASE) {
            Some(n) => c.ret_u32(n),
            None => fail(c, st, errno::EBADF),
        }
    }

    fn dup_from(&self, fd: u32, from: u32) -> Option<u32> {
        let mut t = self.t();
        let e = t.fds.get(&fd)?;
        let (kind, nb) = (e.kind.clone(), e.nonblock);
        if matches!(kind, Kind::Epoll) {
            return None;
        }
        t.insert_from(from, kind, nb, false)
    }

    fn fcntl(&self, c: &mut Cpu, st: &Shared) {
        let (fd, cmd, arg) = {
            let mut a = c.args();
            (a.u32(c), a.u32(c), a.u32(c))
        };
        let mut t = self.t();
        let Some(e) = t.fds.get_mut(&fd) else {
            drop(t);
            return fail(c, st, errno::EBADF);
        };
        let r = match cmd {
            F_GETFL => Ok(if e.nonblock { O_NONBLOCK } else { 0 }),
            F_SETFL => {
                e.nonblock = arg & O_NONBLOCK != 0;
                Ok(0)
            }
            F_GETFD => Ok(u32::from(e.cloexec)),
            F_SETFD => {
                e.cloexec = arg & 1 != 0;
                Ok(0)
            }
            F_DUPFD => {
                drop(t);
                return match self.dup_from(fd, arg) {
                    Some(n) => c.ret_u32(n),
                    None => fail(c, st, errno::EINVAL),
                };
            }
            _ => Err(errno::EINVAL),
        };
        drop(t);
        match r {
            Ok(v) => c.ret_u32(v),
            Err(e) => fail(c, st, e),
        }
    }

    fn ioctl(&self, c: &mut Cpu, st: &Shared) {
        let (fd, req, arg) = {
            let mut a = c.args();
            (a.u32(c), a.u32(c), a.u32(c))
        };
        let pending = {
            let t = self.t();
            match t.fds.get(&fd).map(|e| e.kind.clone()) {
                Some(Kind::PipeR(ch) | Kind::Sock(ch, _)) => Some(t.chans[ch].buf.len() as u32),
                Some(Kind::Net(s)) => Some(s.pending()),
                Some(_) => Some(0),
                None => None,
            }
        };
        let Some(pending) = pending else { return fail(c, st, errno::EBADF) };
        match req {
            FIONREAD => {
                if wr32(c, arg, pending) {
                    c.ret_u32(0);
                }
            }
            FIONBIO => {
                let Some(on) = rd32(c, arg) else { return };
                if let Some(e) = self.t().fds.get_mut(&fd) {
                    e.nonblock = on != 0;
                }
                c.ret_u32(0);
            }
            _ => fail(c, st, errno::EINVAL),
        }
    }

    /// `poll(struct pollfd *, nfds, timeout)` when every descriptor is ours. False means: not handled.
    fn poll(&self, c: &mut Cpu, st: &Shared) -> bool {
        let (fds, n, timeout) = {
            let mut a = c.args();
            (a.u32(c), a.u32(c), a.u32(c) as i32)
        };
        if n == 0 || n > 1024 {
            return false;
        }
        let Some(raw) = rd(c, fds, n * 8) else { return true };
        let list: Vec<(u32, u16)> = raw
            .chunks(8)
            .map(|r| (u32::from_le_bytes(r[..4].try_into().expect("4")), u16::from_le_bytes([r[4], r[5]])))
            .collect();
        if !list.iter().all(|(fd, _)| self.t().fds.contains_key(fd)) {
            return false;
        }
        let deadline = deadline_of(timeout);
        let net = list.iter().any(|(fd, _)| self.t().is_net(*fd));
        loop {
            let mut out = raw.clone();
            let mut count = 0;
            {
                let t = self.t();
                for (i, (fd, want)) in list.iter().enumerate() {
                    let bits = t.ready(*fd);
                    let mut rev = 0u16;
                    if bits & EPOLLIN != 0 && want & POLLIN != 0 {
                        rev |= POLLIN;
                    }
                    if bits & EPOLLOUT != 0 && want & POLLOUT != 0 {
                        rev |= POLLOUT;
                    }
                    if bits & EPOLLHUP != 0 {
                        rev |= POLLHUP;
                    }
                    out[i * 8 + 6..i * 8 + 8].copy_from_slice(&rev.to_le_bytes());
                    count += u32::from(rev != 0);
                }
            }
            if count > 0 {
                if wr(c, fds, &out) {
                    c.ret_u32(count);
                }
                return true;
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                c.ret_u32(0);
                return true;
            }
            if sched::block(c, st, tick(deadline, net), "poll").is_none() {
                return true;
            }
        }
    }
}

/// Host-side helper for the runner: the table plus the thunks, registered on `cpu`.
pub fn install(cpu: &Cpu, st: &Shared) -> Vfd {
    let v = Vfd::default();
    v.set_trace_net(std::env::var_os("BHPC_TRACE_NET").is_some());
    v.install(cpu, st);
    v
}

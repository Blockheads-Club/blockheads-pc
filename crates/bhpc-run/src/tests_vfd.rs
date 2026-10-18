//! Virtual pipes, socket pairs, eventfds and epoll, driven through the guest-visible thunks.
use crate::new_env;
use crate::vfd::{install, Vfd, FD_BASE};
use bhpc_bionic::layout::errno;

const NEG1: u32 = u32::MAX;
const O_NONBLOCK: u32 = 0x800;
const EPOLLIN: u32 = 1;
const EPOLLOUT: u32 = 4;
const EPOLLONESHOT: u32 = 1 << 30;

struct Rig {
    cpu: Box<bhpc_cpu::Cpu>,
    st: bhpc_bionic::Shared,
    vfd: Vfd,
    scratch: u32,
}

fn rig() -> Rig {
    let (cpu, st) = new_env(Box::new(std::io::sink()), None).unwrap();
    let vfd = install(&cpu, &st);
    let scratch = bhpc_bionic::lock(&st).heap.calloc(1, 8192);
    Rig { cpu, st, vfd, scratch }
}

impl Rig {
    fn call(&mut self, name: &str, args: &[u32]) -> u32 {
        let a = bhpc_bionic::resolve(&mut self.cpu, &self.st, name).unwrap_or_else(|| panic!("{name} not bound"));
        self.cpu.call(a, args).unwrap().r0
    }

    /// True if the call stops the guest (a bad guest pointer).
    fn faults(&mut self, name: &str, args: &[u32]) -> bool {
        let a = self.cpu.thunk_addr(name).unwrap();
        self.cpu.call(a, args).is_err()
    }

    /// Start a guest thread that waits `ms`, then writes one byte `z` to `wr`.
    fn delayed_writer(&mut self, wr: u32, ms: u64) {
        let write = self.cpu.thunk_addr("write").unwrap();
        let buf = self.put(3000, b"z");
        let n = self.cpu.thunk_count();
        let start = self
            .cpu
            .register_shared_thunk(
                &format!("delayed_writer_{n}"),
                std::sync::Arc::new(move |c: &mut bhpc_cpu::Cpu| {
                    std::thread::sleep(std::time::Duration::from_millis(ms));
                    c.call(write, &[wr, buf, 1]).unwrap();
                }),
            )
            .unwrap();
        let out = self.scratch + 3100;
        assert_eq!(self.call("pthread_create", &[out, 0, start, 0]), 0);
    }

    fn errno(&mut self) -> u32 {
        let a = bhpc_bionic::sched::errno_addr(&self.st);
        self.cpu.mem().read_u32(a).unwrap()
    }

    fn put(&self, at: u32, bytes: &[u8]) -> u32 {
        self.cpu.mem().write_bytes(self.scratch + at, bytes).unwrap();
        self.scratch + at
    }

    fn get(&self, at: u32, n: u32) -> Vec<u8> {
        self.cpu.mem().read_bytes(self.scratch + at, n).unwrap()
    }

    /// A pipe `(read fd, write fd)`.
    fn pipe(&mut self, flags: u32) -> (u32, u32) {
        let out = self.scratch + 4000;
        let name = if flags == 0 { "pipe" } else { "pipe2" };
        assert_eq!(self.call(name, &[out, flags]), 0);
        (self.cpu.mem().read_u32(out).unwrap(), self.cpu.mem().read_u32(out + 4).unwrap())
    }

    fn write(&mut self, fd: u32, bytes: &[u8]) -> u32 {
        let p = self.put(100, bytes);
        self.call("write", &[fd, p, bytes.len() as u32])
    }

    fn read(&mut self, fd: u32, n: u32) -> (u32, Vec<u8>) {
        let r = self.call("read", &[fd, self.scratch + 200, n]);
        let got = if r == NEG1 { Vec::new() } else { self.get(200, r) };
        (r, got)
    }

    fn event(&mut self, bits: u32, data: u64) -> u32 {
        let mut rec = [0u8; 16];
        rec[..4].copy_from_slice(&bits.to_le_bytes());
        rec[8..].copy_from_slice(&data.to_le_bytes());
        self.put(300, &rec)
    }

    fn wait(&mut self, ep: u32, max: u32, timeout: i32) -> (u32, Vec<(u32, u64)>) {
        let r = self.call("epoll_wait", &[ep, self.scratch + 1000, max, timeout as u32]);
        let evs = (0..if r == NEG1 { 0 } else { r })
            .map(|i| {
                let b = self.get(1000 + i * 16, 16);
                (u32::from_le_bytes(b[..4].try_into().unwrap()), u64::from_le_bytes(b[8..].try_into().unwrap()))
            })
            .collect();
        (r, evs)
    }
}

#[test]
fn pipes_move_bytes_and_report_eof_and_broken_pipes() {
    let mut r = rig();
    let (rd, wr) = r.pipe(0);
    assert!(rd >= FD_BASE && wr > rd);
    assert_eq!(r.write(wr, b"hello"), 5);
    assert_eq!(r.read(rd, 3), (3, b"hel".to_vec()));
    assert_eq!(r.read(rd, 10), (2, b"lo".to_vec()));
    // Direction and bad descriptors.
    assert_eq!(r.write(rd, b"x"), NEG1);
    assert_eq!(r.errno(), errno::EBADF);
    assert_eq!(r.read(wr, 1).0, NEG1);
    assert_eq!(r.read(FD_BASE + 300, 1).0, NEG1);
    assert_eq!(r.write(FD_BASE + 300, b"x"), NEG1);
    assert_eq!(r.call("close", &[FD_BASE + 300]), NEG1);
    // Closing the writer is end of file; closing the reader breaks the pipe.
    assert_eq!(r.call("close", &[wr]), 0);
    assert_eq!(r.read(rd, 4).0, 0);
    let (rd2, wr2) = r.pipe(0);
    assert_eq!(r.call("close", &[rd2]), 0);
    assert_eq!(r.write(wr2, b"x"), NEG1);
    assert_eq!(r.errno(), 32);
    assert_eq!(r.vfd.open_count(), 2);
}

#[test]
fn descriptor_controls_nonblocking_dup_and_ioctl() {
    let mut r = rig();
    let (rd, wr) = r.pipe(O_NONBLOCK);
    assert_eq!(r.read(rd, 1).0, NEG1);
    assert_eq!(r.errno(), errno::EAGAIN);
    assert_eq!(r.call("fcntl", &[rd, 3, 0]), O_NONBLOCK);
    assert_eq!(r.call("fcntl", &[rd, 4, 0]), 0);
    assert_eq!(r.call("fcntl", &[rd, 3, 0]), 0);
    assert_eq!(r.call("fcntl", &[rd, 2, 1]), 0);
    assert_eq!(r.call("fcntl", &[rd, 1, 0]), 1);
    assert_eq!(r.call("fcntl", &[rd, 99, 0]), NEG1);
    assert_eq!(r.call("fcntl", &[FD_BASE + 300, 3, 0]), NEG1);
    let d = r.call("fcntl", &[wr, 0, FD_BASE + 50]);
    assert!(d >= FD_BASE + 50, "F_DUPFD honours the minimum");
    assert_eq!(r.call("fcntl", &[wr, 0, FD_BASE + 5000]), NEG1);
    let d2 = r.call("dup", &[wr]);
    assert_eq!(r.write(d2, b"abc"), 3);
    assert_eq!(r.call("dup", &[FD_BASE + 300]), NEG1);
    // Closing every writer (original + two duplicates) gives end of file.
    for fd in [wr, d, d2] {
        assert_eq!(r.call("close", &[fd]), 0);
    }
    // FIONREAD / FIONBIO.
    let out = r.scratch + 500;
    assert_eq!(r.call("ioctl", &[rd, 0x541B, out]), 0);
    assert_eq!(r.cpu.mem().read_u32(out).unwrap(), 3);
    let on = r.put(504, &1u32.to_le_bytes());
    assert_eq!(r.call("ioctl", &[rd, 0x5421, on]), 0);
    assert_eq!(r.call("ioctl", &[rd, 0x1234, on]), NEG1);
    assert_eq!(r.call("ioctl", &[FD_BASE + 300, 0x541B, out]), NEG1);
    let ep = r.call("epoll_create", &[1]);
    assert_eq!(r.call("ioctl", &[ep, 0x541B, out]), 0);
    assert_eq!(r.call("dup", &[ep]), NEG1, "an epoll instance cannot be duplicated");
    // Not ours: forwarded to bionic (descriptor 1 is stdout).
    assert_eq!(r.call("close", &[1]), 0);
}

#[test]
fn socket_pairs_are_bidirectional_and_eventfds_count() {
    let mut r = rig();
    let out = r.scratch + 600;
    assert_eq!(r.call("socketpair", &[1, 1, 0, out]), 0);
    let (a, b) = (r.cpu.mem().read_u32(out).unwrap(), r.cpu.mem().read_u32(out + 4).unwrap());
    assert_eq!(r.write(a, b"ping"), 4);
    assert_eq!(r.read(b, 8), (4, b"ping".to_vec()));
    assert_eq!(r.write(b, b"pong"), 4);
    assert_eq!(r.read(a, 8), (4, b"pong".to_vec()));
    assert_eq!(r.call("close", &[b]), 0);
    assert_eq!(r.write(a, b"x"), NEG1, "peer gone");
    assert_eq!(r.read(a, 1).0, 0, "peer gone: end of file");
    // eventfd: writes add, a read returns and clears the total.
    let e = r.call("eventfd", &[2, O_NONBLOCK]);
    assert_eq!(r.write(e, &3u64.to_le_bytes()), 8);
    let (n, v) = r.read(e, 8);
    assert_eq!((n, u64::from_le_bytes(v.try_into().unwrap())), (8, 5));
    assert_eq!(r.read(e, 8).0, NEG1);
    assert_eq!(r.errno(), errno::EAGAIN);
    assert_eq!(r.read(e, 4).0, NEG1);
    assert_eq!(r.errno(), errno::EINVAL);
    assert_eq!(r.write(e, b"123"), NEG1);
    assert_eq!(r.errno(), errno::EINVAL);
    assert!(r.faults("write", &[a, 0xDEAD_0000, 4]));
    let (rd, wr) = r.pipe(0);
    assert_eq!(r.write(wr, b"x"), 1);
    assert!(r.faults("read", &[rd, 0xDEAD_0000, 4]));
}

#[test]
fn epoll_reports_readiness_and_validates_requests() {
    let mut r = rig();
    let (rd, wr) = r.pipe(0);
    let ep = r.call("epoll_create1", &[0]);
    let ev = r.event(EPOLLIN, 0xAABB_CCDD_1122_3344);
    assert_eq!(r.call("epoll_ctl", &[ep, 1, rd, ev]), 0);
    assert_eq!(r.call("epoll_ctl", &[ep, 1, rd, ev]), NEG1);
    assert_eq!(r.errno(), 17);
    assert_eq!(r.wait(ep, 4, 0), (0, vec![]));
    assert_eq!(r.write(wr, b"x"), 1);
    assert_eq!(r.wait(ep, 4, 0), (1, vec![(EPOLLIN, 0xAABB_CCDD_1122_3344)]));
    // Modify the interest set, then remove it.
    let ev2 = r.event(EPOLLOUT, 7);
    assert_eq!(r.call("epoll_ctl", &[ep, 3, rd, ev2]), 0);
    assert_eq!(r.wait(ep, 4, 0).0, 0, "a read end is never writable");
    assert_eq!(r.call("epoll_ctl", &[ep, 2, rd, 0]), 0);
    assert_eq!(r.call("epoll_ctl", &[ep, 2, rd, 0]), NEG1);
    assert_eq!(r.errno(), errno::ENOENT);
    assert_eq!(r.call("epoll_ctl", &[ep, 3, rd, ev2]), NEG1);
    assert_eq!(r.call("epoll_ctl", &[ep, 9, rd, ev2]), NEG1);
    assert_eq!(r.errno(), errno::EINVAL);
    assert_eq!(r.call("epoll_ctl", &[FD_BASE + 300, 1, rd, ev2]), NEG1);
    assert_eq!(r.errno(), errno::EBADF);
    assert!(r.faults("epoll_ctl", &[ep, 1, rd, 0xDEAD_0000]));
    // Write end: always writable. One-shot watches disarm after firing. Hang-up shows with the writer gone.
    let both = r.event(EPOLLOUT | EPOLLONESHOT, 9);
    assert_eq!(r.call("epoll_ctl", &[ep, 1, wr, both]), 0);
    assert_eq!(r.wait(ep, 4, 0), (1, vec![(EPOLLOUT, 9)]));
    assert_eq!(r.wait(ep, 4, 0).0, 0);
    let ev3 = r.event(EPOLLIN, 1);
    assert_eq!(r.call("epoll_ctl", &[ep, 1, rd, ev3]), 0);
    assert_eq!(r.call("close", &[wr]), 0);
    let (n, evs) = r.wait(ep, 1, 0);
    assert_eq!(n, 1);
    assert_eq!(evs[0].0 & EPOLLIN, EPOLLIN);
    assert_eq!(evs[0].0 & 0x10, 0x10, "EPOLLHUP");
    // Argument errors and a descriptor of the other table (always ready).
    assert_eq!(r.wait(ep, 0, 0).0, NEG1);
    assert_eq!(r.call("epoll_pwait", &[ep, r.scratch + 1000, 4, 0, 0]), 1, "epoll_pwait is epoll_wait");
    assert_eq!(r.wait(FD_BASE + 300, 1, 0).0, NEG1);
    assert_eq!(r.errno(), errno::EBADF);
    let other = r.event(EPOLLIN | EPOLLOUT, 5);
    assert_eq!(r.call("epoll_ctl", &[ep, 1, 3, other]), 0);
    assert!(r.wait(ep, 4, 0).0 >= 1);
    // A closed descriptor leaves every epoll set.
    assert_eq!(r.call("close", &[rd]), 0);
    assert_eq!(r.call("epoll_ctl", &[ep, 2, rd, 0]), NEG1);
    // Forwarded to bionic for foreign descriptors.
    assert_eq!(r.call("epoll_ctl", &[3, 1, 4, other]), NEG1);
}

#[test]
fn blocking_calls_wait_for_other_threads() {
    let mut r = rig();
    let (rd, wr) = r.pipe(0);
    let ep = r.call("epoll_create", &[1]);
    let ev = r.event(EPOLLIN, 77);
    assert_eq!(r.call("epoll_ctl", &[ep, 1, rd, ev]), 0);
    // A timeout that expires while blocked.
    let begun = std::time::Instant::now();
    assert_eq!(r.wait(ep, 1, 30).0, 0);
    assert!(begun.elapsed() >= std::time::Duration::from_millis(25));
    // Another guest thread writes: a blocked epoll_wait and a blocked read both wake up.
    r.delayed_writer(wr, 40);
    assert_eq!(r.wait(ep, 1, -1), (1, vec![(EPOLLIN, 77)]));
    assert_eq!(r.read(rd, 1), (1, b"z".to_vec()));
    r.delayed_writer(wr, 40);
    assert_eq!(r.read(rd, 1), (1, b"z".to_vec()));
    // poll(): ready, timed out, and the not-ours fallbacks.
    let rec = |fd: u32, ev: u16| {
        let mut b = fd.to_le_bytes().to_vec();
        b.extend_from_slice(&ev.to_le_bytes());
        b.extend_from_slice(&[0, 0]);
        b
    };
    let p = r.put(2000, &[rec(rd, 1), rec(wr, 4)].concat());
    assert_eq!(r.call("poll", &[p, 2, 0]), 1);
    assert_eq!(u16::from_le_bytes(r.get(2000 + 6, 2).try_into().unwrap()), 0, "nothing readable yet");
    assert_eq!(u16::from_le_bytes(r.get(2000 + 14, 2).try_into().unwrap()), 4, "writable");
    let p1 = r.put(2100, &rec(rd, 1));
    assert_eq!(r.call("poll", &[p1, 1, 10]), 0, "timeout");
    assert_eq!(r.write(wr, b"q"), 1);
    assert_eq!(r.call("poll", &[p1, 1, 0]), 1);
    assert_eq!(u16::from_le_bytes(r.get(2106, 2).try_into().unwrap()), 1);
    assert_eq!(r.call("close", &[wr]), 0);
    assert_eq!(r.call("poll", &[p1, 1, 0]), 1);
    let foreign = r.put(2200, &rec(3, 1));
    assert_eq!(r.call("poll", &[foreign, 1, 0]), 0, "forwarded to bionic's poll");
    assert_eq!(r.call("poll", &[foreign, 0, 0]), 0, "empty list is forwarded too");
    assert!(r.faults("poll", &[0xDEAD_0000, 1, 0]));
}

#[test]
fn getrlimit_reports_a_sane_descriptor_limit() {
    let mut r = rig();
    let out = r.scratch + 700;
    assert_eq!(r.call("getrlimit", &[7, out]), 0);
    assert_eq!((r.cpu.mem().read_u32(out).unwrap(), r.cpu.mem().read_u32(out + 4).unwrap()), (1024, 4096));
    assert_eq!(r.call("getrlimit", &[3, out]), 0, "other resources are bionic's");
    assert!(r.faults("getrlimit", &[7, 0xDEAD_0000]));
}

#[test]
fn running_out_of_descriptors_is_reported() {
    let mut r = rig();
    let mut fds = Vec::new();
    loop {
        let fd = r.call("eventfd", &[0, 0]);
        if fd == NEG1 {
            break;
        }
        fds.push(fd);
    }
    assert_eq!(r.errno(), 24);
    assert_eq!(fds.len() as u32, 1024 - FD_BASE);
    let out = r.scratch + 4000;
    assert_eq!(r.call("pipe", &[out, 0]), NEG1);
    assert_eq!(r.call("socketpair", &[1, 1, 0, out]), NEG1);
    assert_eq!(r.call("epoll_create", &[1]), NEG1);
    // One free slot: the pipe gets its read end but not its write end, and gives the read end back.
    r.call("close", &[fds[0]]);
    r.call("close", &[fds[1]]);
    let free_before = r.vfd.open_count();
    r.call("close", &[fds[2]]);
    assert_eq!(r.call("pipe", &[out, 0]), 0);
    assert_eq!(r.vfd.open_count(), free_before + 1);
    for fd in &fds[3..] {
        r.call("close", &[*fd]);
    }
    assert!(r.faults("socketpair", &[1, 1, 0, 0xDEAD_0000]));
    assert!(r.faults("pipe", &[0xDEAD_0000, 0]));
}

#[test]
fn every_kind_of_descriptor_can_be_watched_and_deadlocks_are_reported() {
    let mut r = rig();
    let ep = r.call("epoll_create", &[1]);
    // A socket pair end (writable, readable once the peer wrote), an eventfd and another epoll instance.
    let out = r.scratch + 600;
    assert_eq!(r.call("socketpair", &[1, 1, 0, out]), 0);
    let (a, b) = (r.cpu.mem().read_u32(out).unwrap(), r.cpu.mem().read_u32(out + 4).unwrap());
    let ev = r.event(EPOLLIN | EPOLLOUT, 1);
    assert_eq!(r.call("epoll_ctl", &[ep, 1, a, ev]), 0);
    assert_eq!(r.wait(ep, 4, 0), (1, vec![(EPOLLOUT, 1)]));
    assert_eq!(r.write(b, b"x"), 1);
    assert_eq!(r.wait(ep, 4, 0), (1, vec![(EPOLLIN | EPOLLOUT, 1)]));
    assert_eq!(r.call("close", &[b]), 0);
    assert_eq!(r.wait(ep, 4, 0).1[0].0 & 8, 8, "EPOLLERR once the peer cannot read any more");
    assert_eq!(r.call("epoll_ctl", &[ep, 2, a, 0]), 0);
    let e = r.call("eventfd", &[0, 0]);
    let ev2 = r.event(EPOLLIN, 2);
    assert_eq!(r.call("epoll_ctl", &[ep, 1, e, ev2]), 0);
    assert_eq!(r.wait(ep, 4, 0).0, 0);
    assert_eq!(r.write(e, &1u64.to_le_bytes()), 8);
    assert_eq!(r.wait(ep, 4, 0), (1, vec![(EPOLLIN, 2)]));
    assert_eq!(r.call("epoll_ctl", &[ep, 2, e, 0]), 0);
    let inner = r.call("epoll_create", &[1]);
    let ev3 = r.event(EPOLLIN, 3);
    assert_eq!(r.call("epoll_ctl", &[ep, 1, inner, ev3]), 0);
    assert_eq!(r.wait(ep, 4, 0).0, 0, "an epoll instance is never ready");
    // A bad event buffer stops the guest.
    let (rd, wr) = r.pipe(0);
    assert_eq!(r.write(wr, b"x"), 1);
    let ev4 = r.event(EPOLLIN, 4);
    assert_eq!(r.call("epoll_ctl", &[ep, 1, rd, ev4]), 0);
    assert!(r.faults("epoll_wait", &[ep, 0xDEAD_0000, 4, 0]));
    // With no other guest thread to wake us, waiting forever is a deadlock and stops the guest.
    let (rd2, _wr2) = r.pipe(0);
    let ep2 = r.call("epoll_create", &[1]);
    assert!(r.faults("epoll_wait", &[ep2, r.scratch + 1000, 1, NEG1]));
    let e2 = r.call("eventfd", &[0, 0]);
    assert!(r.faults("read", &[e2, r.scratch + 200, 8]));
    assert!(r.faults("read", &[rd2, r.scratch + 200, 8]));
    let rec = {
        let mut b = rd2.to_le_bytes().to_vec();
        b.extend_from_slice(&[1, 0, 0, 0]);
        b
    };
    let p = r.put(2000, &rec);
    assert!(r.faults("poll", &[p, 1, NEG1]));
}

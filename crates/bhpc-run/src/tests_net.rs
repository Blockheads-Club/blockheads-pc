//! Guest-side BSD sockets over host loopback, the resolver and `select`, driven through the registered thunks.
use crate::tests_vfd::{rig, Rig};
use bhpc_bionic::layout::errno;

const NEG1: u32 = u32::MAX;
const NB: u32 = 0x800;
const EINPROGRESS: u32 = 115;
const ECONNREFUSED: u32 = 111;
const ENOTSOCK: u32 = 88;
const EAFNOSUPPORT: u32 = 97;
const EFAULT_ADDR: u32 = 0xDEAD_0000;
const STREAM: u32 = 1;
const DGRAM: u32 = 2;
const EPOLLIN: u32 = 1;
const EPOLLOUT: u32 = 4;

fn sin(port: u16) -> Vec<u8> {
    let mut v = vec![2, 0];
    v.extend(port.to_be_bytes());
    v.extend([127, 0, 0, 1]);
    v.extend([0; 8]);
    v
}

impl Rig {
    fn sock(&mut self, ty: u32) -> u32 {
        let fd = self.call("socket", &[2, ty, 0]);
        assert!(fd < NEG1, "socket({ty}) failed: errno {}", self.errno());
        fd
    }

    /// Bind to 127.0.0.1 on an ephemeral port; returns the port.
    fn bind_lo(&mut self, fd: u32) -> u16 {
        let a = self.put(5000, &sin(0));
        assert_eq!(self.call("bind", &[fd, a, 16]), 0);
        self.port_of(fd, "getsockname")
    }

    fn port_of(&mut self, fd: u32, which: &str) -> u16 {
        let lenp = self.put(5100, &28u32.to_le_bytes());
        assert_eq!(self.call(which, &[fd, self.scratch + 5200, lenp]), 0);
        assert_eq!(self.get(5100, 4), 16u32.to_le_bytes());
        let a = self.get(5200, 16);
        assert_eq!(&a[..2], &[2, 0]);
        assert!(a[4..8] == [127, 0, 0, 1] || a[4..8] == [0; 4]);
        u16::from_be_bytes([a[2], a[3]])
    }

    fn sendto(&mut self, fd: u32, data: &[u8], port: u16) -> u32 {
        let (p, a) = (self.put(5300, data), self.put(5400, &sin(port)));
        self.call("sendto", &[fd, p, data.len() as u32, 0, a, 16])
    }

    /// `recvfrom` with the source address captured at 5600 (length cell at 5500).
    fn recvfrom(&mut self, fd: u32, n: u32, flags: u32) -> (u32, Vec<u8>) {
        self.put(5500, &28u32.to_le_bytes());
        let r = self.call("recvfrom", &[fd, self.scratch + 5700, n, flags, self.scratch + 5600, self.scratch + 5500]);
        (r, if r == NEG1 { vec![] } else { self.get(5700, r) })
    }

    fn watch(&mut self, ep: u32, fd: u32, events: u32) {
        let ev = self.event(events, u64::from(fd));
        assert_eq!(self.call("epoll_ctl", &[ep, 1, fd, ev]), 0);
    }

    fn tcp_pair(&mut self) -> (u32, u32, u32) {
        let l = self.sock(STREAM);
        let port = self.bind_lo(l);
        assert_eq!(self.call("listen", &[l, 4]), 0);
        let c = self.sock(STREAM);
        let a = self.put(5000, &sin(port));
        assert_eq!(self.call("connect", &[c, a, 16]), 0);
        let s = self.call("accept", &[l, 0, 0]);
        assert!(s < NEG1);
        (l, c, s)
    }

    fn timeout(&mut self, fd: u32, name: u32, ms: u32) {
        let tv = self.put(5000, &[(ms / 1000).to_le_bytes(), (ms % 1000 * 1000).to_le_bytes()].concat());
        assert_eq!(self.call("setsockopt", &[fd, 1, name, tv, 8]), 0);
    }
}

#[test]
fn udp_datagrams_with_addresses_flags_and_vectored_io() {
    let mut r = rig();
    let a = r.sock(DGRAM | NB);
    let pa = r.bind_lo(a);
    assert_eq!(r.recvfrom(a, 16, 0).0, NEG1);
    assert_eq!(r.errno(), errno::EAGAIN);
    let b = r.sock(DGRAM);
    assert_eq!(r.sendto(b, b"hello", pa), 5);
    let ep = r.call("epoll_create", &[1]);
    r.watch(ep, a, EPOLLIN);
    let (n, evs) = r.wait(ep, 4, 1000);
    assert_eq!((n, evs[0].0 & EPOLLIN, evs[0].1), (1, EPOLLIN, u64::from(a)));
    let pb = r.port_of(b, "getsockname");
    let mut out = r.scratch + 5000;
    assert_eq!(r.call("ioctl", &[a, 0x541B, out]), 0);
    assert_eq!(r.cpu.mem().read_u32(out).unwrap(), 5);
    let (n, data) = r.recvfrom(a, 16, 2);
    assert_eq!((n, data.as_slice()), (5, &b"hello"[..]), "MSG_PEEK");
    let (n, data) = r.recvfrom(a, 16, 0);
    assert_eq!((n, data.as_slice()), (5, &b"hello"[..]));
    let from = r.get(5600, 16);
    assert_eq!((from[0], u16::from_be_bytes([from[2], from[3]])), (2, pb));
    assert_eq!(r.get(5500, 4), 16u32.to_le_bytes());
    // Small address buffer: truncated, the real length is reported.
    assert_eq!(r.sendto(b, b"xy", pa), 2);
    r.put(5500, &4u32.to_le_bytes());
    r.put(5600, &[0xAA; 16]);
    assert_eq!(r.call("recvfrom", &[a, r.scratch + 5700, 16, 0, r.scratch + 5600, r.scratch + 5500]), 2);
    assert_eq!((r.get(5500, 4), r.get(5604, 1)), (16u32.to_le_bytes().to_vec(), vec![0xAA]));
    // recv without address, connected send, read and write.
    let ca = r.put(5400, &sin(pa));
    assert_eq!(r.call("connect", &[b, ca, 16]), 0);
    let p = r.put(5300, b"conn");
    assert_eq!(r.call("send", &[b, p, 4, 0]), 4);
    assert_eq!(r.call("recv", &[a, r.scratch + 5700, 16, 0]), 4);
    assert_eq!(r.port_of(b, "getpeername"), pa);
    assert_eq!(r.write(b, b"wr"), 2);
    assert_eq!(r.read(a, 8), (2, b"wr".to_vec()));
    // sendmsg / recvmsg with two vectors and a destination.
    let pb2 = r.sock(DGRAM);
    let pb2_port = r.bind_lo(pb2);
    r.put(6100, b"ab");
    r.put(6110, b"cdef");
    let iov = r.put(
        6200,
        &[(r.scratch + 6100).to_le_bytes(), 2u32.to_le_bytes(), (r.scratch + 6110).to_le_bytes(), 4u32.to_le_bytes()]
            .concat(),
    );
    let name = r.put(6300, &sin(pb2_port));
    let words = [name, 16, iov, 2, 0, 0, 0];
    let msg = r.put(6400, &words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>());
    let other = r.sock(DGRAM);
    assert_eq!(r.call("sendmsg", &[other, msg, 0]), 6);
    // Receive into 3 + 8 bytes, with the sender's name.
    let (b1, b2, nm) = (r.scratch + 6600, r.scratch + 6620, r.scratch + 6700);
    let iov2 = r.put(6500, &[b1.to_le_bytes(), 3u32.to_le_bytes(), b2.to_le_bytes(), 8u32.to_le_bytes()].concat());
    let words = [nm, 28, iov2, 2, 0, 0, 0];
    let msg2 = r.put(6450, &words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>());
    assert_eq!(r.call("recvmsg", &[pb2, msg2, 0]), 6);
    assert_eq!((r.get(6600, 3), r.get(6620, 3)), (b"abc".to_vec(), b"def".to_vec()));
    assert_eq!(r.get(6454, 4), 16u32.to_le_bytes());
    assert_eq!(r.get(6700, 2), vec![2, 0]);
    // recvmsg without a name, nothing pending in non-blocking mode.
    let words = [0, 0, iov2, 2, 0, 0, 0];
    let msg3 = r.put(6460, &words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>());
    assert_eq!(r.call("recvmsg", &[a, msg3, 0]), NEG1);
    assert_eq!(r.errno(), errno::EAGAIN);
    assert_eq!(r.sendto(other, b"q", pa), 1);
    assert_eq!(r.call("recvmsg", &[a, msg3, 0]), 1);
    // Readiness through poll(), and duplicated descriptors share the socket.
    let pf = r.put(7000, &[a.to_le_bytes().as_slice(), &[1, 0, 0, 0]].concat());
    assert_eq!(r.call("poll", &[pf, 1, 0]), 0);
    let d = r.call("dup", &[a]);
    assert_eq!(r.sendto(other, b"z", pa), 1);
    assert_eq!(r.call("poll", &[pf, 1, 1000]), 1);
    assert_eq!(r.read(d, 4), (1, b"z".to_vec()));
    out = r.scratch;
    let _ = out;
    for fd in [a, b, pb2, other, d] {
        assert_eq!(r.call("close", &[fd]), 0);
    }
    assert_eq!(r.vfd.open_count(), 1, "only the epoll instance is left");
}

#[test]
fn socket_options_and_timeouts() {
    let mut r = rig();
    r.vfd.set_trace_net(true);
    let s = r.sock(DGRAM);
    r.bind_lo(s);
    // Pass-through option and its read-back.
    let one = r.put(5000, &1u32.to_le_bytes());
    assert_eq!(r.call("setsockopt", &[s, 1, 2, one, 4]), 0);
    let lenp = r.put(5010, &4u32.to_le_bytes());
    assert_eq!(r.call("getsockopt", &[s, 1, 2, r.scratch + 5020, lenp]), 0);
    assert_eq!((r.get(5020, 4), r.get(5010, 4)), (1u32.to_le_bytes().to_vec(), 4u32.to_le_bytes().to_vec()));
    assert_eq!(r.call("setsockopt", &[s, 1, 9999, one, 4]), NEG1);
    assert_eq!(r.call("getsockopt", &[s, 1, 9999, r.scratch + 5020, lenp]), NEG1);
    // Receive timeout: stored, read back, and honoured by a blocking receive.
    assert_eq!(r.call("setsockopt", &[s, 1, 20, one, 4]), NEG1);
    assert_eq!(r.errno(), errno::EINVAL);
    r.timeout(s, 20, 1250);
    let lenp = r.put(5010, &8u32.to_le_bytes());
    assert_eq!(r.call("getsockopt", &[s, 1, 20, r.scratch + 5020, lenp]), 0);
    assert_eq!(
        (r.get(5020, 8), r.get(5010, 4)),
        ([1u32.to_le_bytes(), 250_000u32.to_le_bytes()].concat(), 8u32.to_le_bytes().to_vec())
    );
    let lenp = r.put(5010, &4u32.to_le_bytes());
    assert_eq!(r.call("getsockopt", &[s, 1, 21, r.scratch + 5020, lenp]), 0);
    assert_eq!(r.get(5010, 4), 4u32.to_le_bytes());
    r.timeout(s, 20, 30);
    let t = std::time::Instant::now();
    assert_eq!(r.recvfrom(s, 4, 0).0, NEG1);
    assert_eq!(r.errno(), errno::EAGAIN);
    assert!(t.elapsed() >= std::time::Duration::from_millis(30));
    // MSG_DONTWAIT on a blocking socket does not wait.
    r.timeout(s, 20, 0);
    assert_eq!(r.recvfrom(s, 4, 0x40).0, NEG1);
    // Errors and faults.
    assert_eq!(r.call("setsockopt", &[s, 1, 2, one, 4]), 0);
    assert!(r.faults("setsockopt", &[s, 1, 2, EFAULT_ADDR, 4]));
    assert!(r.faults("getsockopt", &[s, 1, 2, r.scratch + 5020, EFAULT_ADDR]));
    let lenp = r.put(5010, &4u32.to_le_bytes());
    assert!(r.faults("getsockopt", &[s, 1, 2, EFAULT_ADDR, lenp]));
    assert_eq!(r.call("setsockopt", &[999, 1, 2, one, 4]), NEG1);
    assert_eq!(r.errno(), errno::EBADF);
}

#[test]
fn tcp_connect_accept_stream_and_shutdown() {
    let mut r = rig();
    r.vfd.set_trace_net(true);
    let (l, c, s) = r.tcp_pair();
    assert_eq!(r.write(c, b"abc"), 3);
    assert_eq!(r.read(s, 8), (3, b"abc".to_vec()));
    let big = vec![7u8; 5000];
    assert_eq!(r.call("send", &[s, r.put(0, &big[..4000]), 4000, 0]), 4000);
    assert_eq!(r.call("recv", &[c, r.scratch + 100, 100, 0]), 100);
    assert_eq!(r.call("shutdown", &[c, 1]), 0);
    assert_eq!(r.read(s, 8).0, 0, "peer shut down its write side");
    assert_eq!(r.call("shutdown", &[999, 1]), NEG1);
    let _ = r.port_of(s, "getpeername");
    // accept4 with a peer address and SOCK_NONBLOCK; accept times out through SO_RCVTIMEO.
    let port = r.port_of(l, "getsockname");
    let c2 = r.sock(STREAM | NB);
    let a = r.put(5000, &sin(port));
    assert_eq!(r.call("connect", &[c2, a, 16]), NEG1);
    assert_eq!(r.errno(), EINPROGRESS);
    let ep = r.call("epoll_create1", &[0]);
    r.watch(ep, c2, EPOLLOUT);
    assert_eq!(r.wait(ep, 2, 1000).0, 1);
    let lenp = r.put(5100, &16u32.to_le_bytes());
    let s2 = r.call("accept4", &[l, r.scratch + 5200, lenp, NB]);
    assert!(s2 < NEG1);
    assert_eq!(r.call("fcntl", &[s2, 3, 0]), NB);
    assert_eq!(r.read(s2, 4).0, NEG1);
    assert_eq!(r.errno(), errno::EAGAIN);
    r.timeout(l, 20, 20);
    assert_eq!(r.call("accept", &[l, 0, 0]), NEG1);
    assert_eq!(r.errno(), errno::EAGAIN);
    let lnb = r.sock(STREAM | NB);
    r.bind_lo(lnb);
    assert_eq!(r.call("listen", &[lnb, 1]), 0);
    assert_eq!(r.call("accept", &[lnb, 0, 0]), NEG1);
    // Bad address cell for accept: the new descriptor is not leaked.
    let before = r.vfd.open_count();
    let c3 = r.sock(STREAM);
    let a = r.put(5000, &sin(port));
    assert_eq!(r.call("connect", &[c3, a, 16]), 0);
    assert!(r.faults("accept", &[l, r.scratch + 5200, EFAULT_ADDR]));
    assert_eq!(r.vfd.open_count(), before + 1);
    // A refused connection reports ECONNREFUSED, blocking or not.
    let dead = {
        let t = r.sock(STREAM);
        let p = r.bind_lo(t);
        assert_eq!(r.call("close", &[t]), 0);
        p
    };
    let cr = r.sock(STREAM);
    let a = r.put(5000, &sin(dead));
    assert_eq!(r.call("connect", &[cr, a, 16]), NEG1);
    assert_eq!(r.errno(), ECONNREFUSED);
    let crn = r.sock(STREAM | NB);
    assert_eq!(r.call("connect", &[crn, a, 16]), NEG1);
    let ep2 = r.call("epoll_create", &[1]);
    r.watch(ep2, crn, EPOLLIN);
    assert_eq!(r.wait(ep2, 2, 1000).0, 1, "a failed connect reports an error condition");
}

#[test]
fn blocking_receive_waits_for_another_guest_thread() {
    let mut r = rig();
    let (_l, c, s) = r.tcp_pair();
    r.delayed_writer(c, 40);
    let t = std::time::Instant::now();
    assert_eq!(r.read(s, 4), (1, b"z".to_vec()));
    assert!(t.elapsed() >= std::time::Duration::from_millis(30));
    // Poll with an infinite timeout is woken by the periodic tick when data arrives from the host side.
    r.delayed_writer(c, 40);
    let pf = r.put(7000, &[s.to_le_bytes().as_slice(), &[1, 0, 0, 0]].concat());
    assert_eq!(r.call("poll", &[pf, 1, NEG1]), 1);
    assert_eq!(r.call("poll", &[pf, 1, 20]), 1);
    assert_eq!(r.get(7006, 2), vec![1, 0]);
}

#[test]
fn socket_errors_and_faults() {
    let mut r = rig();
    assert_eq!(r.call("socket", &[1, 1, 0]), NEG1);
    assert_eq!(r.errno(), EAFNOSUPPORT);
    assert_eq!(r.call("socket", &[2, 99, 0]), NEG1);
    assert_ne!(r.errno(), 0);
    let (rd, _wr) = r.pipe(0);
    let a = r.put(5000, &sin(1));
    assert_eq!(r.call("bind", &[rd, a, 16]), NEG1);
    assert_eq!(r.errno(), ENOTSOCK);
    assert_eq!(r.call("bind", &[FD_NONE, a, 16]), NEG1);
    assert_eq!(r.errno(), errno::EBADF);
    let s = r.sock(DGRAM);
    let unix = r.put(5050, &[1, 0, 0, 0]);
    assert_eq!(r.call("bind", &[s, unix, 4]), NEG1);
    assert_eq!(r.errno(), EAFNOSUPPORT);
    assert_eq!(r.call("bind", &[s, a, 1]), NEG1);
    assert_eq!(r.errno(), errno::EINVAL);
    assert!(r.faults("bind", &[s, EFAULT_ADDR, 16]));
    assert!(r.faults("connect", &[s, EFAULT_ADDR, 16]));
    assert_eq!(r.call("listen", &[999, 1]), NEG1);
    assert_eq!(r.call("listen", &[s, 1]), NEG1, "listen on a datagram socket");
    assert_eq!(r.call("accept", &[999, 0, 0]), NEG1);
    // Data path faults.
    assert!(r.faults("sendto", &[s, EFAULT_ADDR, 4, 0, a, 16]));
    assert!(r.faults("sendto", &[s, a, 4, 0, EFAULT_ADDR, 16]));
    assert_eq!(r.call("sendto", &[s, a, 4, 0, unix, 4]), NEG1);
    assert_eq!(r.call("send", &[999, a, 4, 0]), NEG1);
    assert_eq!(r.call("send", &[s, a, 4, 0]), NEG1, "datagram socket without a destination");
    let peer = r.sock(DGRAM);
    let pp = r.bind_lo(peer);
    assert_eq!(r.sendto(s, b"data", pp), 4);
    assert!(r.faults("recv", &[peer, EFAULT_ADDR, 16, 0]));
    assert_eq!(r.sendto(s, b"data", pp), 4);
    r.put(5500, &28u32.to_le_bytes());
    assert!(r.faults("recvfrom", &[peer, r.scratch + 5700, 16, 0, r.scratch + 5600, EFAULT_ADDR]));
    assert_eq!(r.sendto(s, b"data", pp), 4);
    assert!(r.faults("recvfrom", &[peer, r.scratch + 5700, 16, 0, EFAULT_ADDR, r.scratch + 5500]));
    assert_eq!(r.sendto(s, b"data", pp), 4);
    assert!(r.faults("read", &[peer, EFAULT_ADDR, 4]));
    assert!(r.faults("write", &[peer, EFAULT_ADDR, 4]));
    assert!(r.faults("sendmsg", &[s, EFAULT_ADDR, 0]));
    assert!(r.faults("recvmsg", &[s, EFAULT_ADDR, 0]));
    let bad_iov =
        r.put(6400, &[0u32, 0, EFAULT_ADDR, 1, 0, 0, 0].iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>());
    assert!(r.faults("sendmsg", &[s, bad_iov, 0]));
    let iov = r.put(6200, &[EFAULT_ADDR.to_le_bytes(), 4u32.to_le_bytes()].concat());
    let words = [0, 0, iov, 1, 0, 0, 0];
    let bad_base = r.put(6450, &words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>());
    assert!(r.faults("sendmsg", &[s, bad_base, 0]));
    assert_eq!(r.sendto(s, b"data", pp), 4);
    assert!(r.faults("recvmsg", &[peer, bad_base, 0]));
    // sendmsg with a bad name, recvmsg with unwritable name/msghdr tail.
    let words = [unix, 4, 0, 0, 0, 0, 0];
    let bad_name = r.put(6500, &words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>());
    assert_eq!(r.call("sendmsg", &[s, bad_name, 0]), NEG1);
    assert_eq!(r.sendto(s, b"data", pp), 4);
    let ok_iov = r.put(6600, &[(r.scratch + 6700).to_le_bytes(), 16u32.to_le_bytes()].concat());
    let words = [EFAULT_ADDR, 16, ok_iov, 1, 0, 0, 0];
    let bad_nm = r.put(6550, &words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>());
    assert!(r.faults("recvmsg", &[peer, bad_nm, 0]));
    // name accessors
    assert_eq!(r.call("getsockname", &[999, 0, 0]), NEG1);
    assert_eq!(r.call("getpeername", &[s, r.scratch + 5200, r.put(5100, &16u32.to_le_bytes())]), NEG1);
    assert!(r.faults("getsockname", &[s, r.scratch + 5200, EFAULT_ADDR]));
    // Plain reads and writes on a socket descriptor that is not connected.
    assert_eq!(r.write(s, b"x"), NEG1);
}

const FD_NONE: u32 = crate::vfd::FD_BASE + 300;

#[test]
fn resolver_builds_addrinfo_hostent_and_nameinfo_in_guest_memory() {
    let mut r = rig();
    r.vfd.set_trace_net(true);
    let host = r.put(0, b"127.0.0.1\0");
    let svc = r.put(20, b"8080\0");
    let res = r.scratch + 40;
    assert_eq!(r.call("getaddrinfo", &[host, svc, 0, res]), 0);
    let head = r.cpu.mem().read_u32(res).unwrap();
    let rd = |r: &Rig, at: u32| r.cpu.mem().read_u32(at).unwrap();
    let mut seen = Vec::new();
    let mut p = head;
    while p != 0 {
        let sa = rd(&r, p + 24);
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&r.cpu.mem().read_bytes(sa, 8).unwrap());
        seen.push((rd(&r, p + 4), rd(&r, p + 8), rd(&r, p + 12), rd(&r, p + 16), bytes));
        p = rd(&r, p + 28);
    }
    let sa4 = [2, 0, 0x1F, 0x90, 127, 0, 0, 1];
    assert_eq!(seen, vec![(2, 1, 6, 16, sa4), (2, 2, 17, 16, sa4)]);
    r.call("freeaddrinfo", &[head]);
    assert!(r.faults("freeaddrinfo", &[0x1234_5679]));
    // Hints: family, socktype, canonical name; named service; v6 literal.
    let node = r.put(100, b"localhost\0");
    let http = r.put(120, b"http\0");
    // flags=CANONNAME(2), family=AF_INET, socktype=STREAM
    let hints = r.put(140, &[2u32, 2, 1, 0].iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>());
    assert_eq!(r.call("getaddrinfo", &[node, http, hints, res]), 0);
    let ai = r.cpu.mem().read_u32(res).unwrap();
    let canon = rd(&r, ai + 20);
    assert_eq!(r.cpu.mem().read_bytes(canon, 9).unwrap(), b"localhost");
    let sa = rd(&r, ai + 24);
    assert_eq!(r.cpu.mem().read_bytes(sa + 2, 2).unwrap(), [0, 80]);
    assert_eq!(rd(&r, ai + 28), 0);
    r.call("freeaddrinfo", &[ai]);
    let v6 = r.put(200, b"::1\0");
    assert_eq!(r.call("getaddrinfo", &[v6, 0, 0, res]), 0);
    let ai = rd(&r, res);
    assert_eq!(rd(&r, ai + 4), 10);
    assert_eq!(rd(&r, ai + 16), 28);
    r.call("freeaddrinfo", &[ai]);
    // No node: passive wildcard or loopback; datagram hint with its default protocol.
    r.put(140, &[1u32, 0, 2, 0].iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>());
    assert_eq!(r.call("getaddrinfo", &[0, svc, hints, res]), 0);
    let ai = rd(&r, res);
    assert_eq!((rd(&r, ai + 8), rd(&r, ai + 12)), (2, 17));
    let sa = rd(&r, ai + 24);
    assert_eq!(r.cpu.mem().read_bytes(sa + 4, 4).unwrap(), [0, 0, 0, 0]);
    r.call("freeaddrinfo", &[ai]);
    r.put(140, &[0u32; 4].iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>());
    assert_eq!(r.call("getaddrinfo", &[0, 0, hints, res]), 0);
    let ai = rd(&r, res);
    let sa = rd(&r, ai + 24);
    assert_eq!(r.cpu.mem().read_bytes(sa + 4, 4).unwrap(), [127, 0, 0, 1]);
    r.call("freeaddrinfo", &[ai]);
    // Errors, as EAI codes.
    let bad_host = r.put(300, b"no-such-host.invalid\0");
    let bad_svc = r.put(340, b"nosuchsvc\0");
    let hint = |r: &mut Rig, h: [u32; 4]| r.put(140, &h.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>());
    assert_eq!(r.call("getaddrinfo", &[bad_host, 0, 0, res]), 8);
    let h = hint(&mut r, [4, 0, 0, 0]);
    assert_eq!(r.call("getaddrinfo", &[node, 0, h, res]), 8, "AI_NUMERICHOST with a name");
    assert_eq!(r.call("getaddrinfo", &[host, bad_svc, 0, res]), 9);
    let h = hint(&mut r, [0, 99, 0, 0]);
    assert_eq!(r.call("getaddrinfo", &[host, 0, h, res]), 5);
    let h = hint(&mut r, [0, 0, 3, 0]);
    assert_eq!(r.call("getaddrinfo", &[host, 0, h, res]), 10);
    let h = hint(&mut r, [0, 10, 0, 0]);
    assert_eq!(r.call("getaddrinfo", &[host, 0, h, res]), 8, "an IPv4 literal has no IPv6 address");
    let h = hint(&mut r, [0, 0, 1, 6]);
    assert_eq!(r.call("getaddrinfo", &[host, 0, h, res]), 0);
    r.call("freeaddrinfo", &[rd(&r, res)]);
    assert!(r.faults("getaddrinfo", &[EFAULT_ADDR, 0, 0, res]));
    assert!(r.faults("getaddrinfo", &[host, EFAULT_ADDR, 0, res]));
    assert!(r.faults("getaddrinfo", &[host, 0, 0, EFAULT_ADDR]));
    // gethostbyname / gethostbyname2.
    let he = r.call("gethostbyname", &[host]);
    assert_ne!(he, 0);
    let (name, aliases, ty, len, list) = (rd(&r, he), rd(&r, he + 4), rd(&r, he + 8), rd(&r, he + 12), rd(&r, he + 16));
    assert_eq!((ty, len, rd(&r, aliases)), (2, 4, 0));
    assert_eq!(r.cpu.mem().read_bytes(name, 9).unwrap(), b"127.0.0.1");
    assert_eq!(r.cpu.mem().read_bytes(rd(&r, list), 4).unwrap(), [127, 0, 0, 1]);
    assert_eq!(rd(&r, list + 4), 0);
    assert_eq!(r.call("gethostbyname", &[bad_host]), 0);
    let he6 = r.call("gethostbyname2", &[v6, 10]);
    assert_eq!(he6, he, "one static buffer");
    assert_eq!((rd(&r, he6 + 8), rd(&r, he6 + 12)), (10, 16));
    assert_eq!(r.cpu.mem().read_bytes(rd(&r, rd(&r, he6 + 16)) + 15, 1).unwrap(), [1]);
    assert!(r.faults("gethostbyname", &[EFAULT_ADDR]));
    // getnameinfo: numeric host and service.
    let sa = r.put(400, &sin(4242));
    let (hb, sb) = (r.scratch + 500, r.scratch + 600);
    assert_eq!(r.call("getnameinfo", &[sa, 16, hb, 64, sb, 16, 0]), 0);
    assert_eq!((r.get(500, 10), r.get(600, 5)), (b"127.0.0.1\0".to_vec(), b"4242\0".to_vec()));
    let mut s6 = vec![10, 0, 0x1F, 0x90, 0, 0, 0, 0];
    s6.extend([0; 15]);
    s6.extend([1, 0, 0, 0, 0]);
    let sa6 = r.put(700, &s6);
    assert_eq!(r.call("getnameinfo", &[sa6, 28, hb, 64, 0, 0, 0]), 0);
    assert_eq!(r.get(500, 4), b"::1\0");
    assert_eq!(r.call("getnameinfo", &[sa, 16, hb, 4, sb, 16, 0]), 14);
    assert_eq!(r.call("getnameinfo", &[sa, 16, 0, 0, sb, 2, 0]), 14);
    assert_eq!(r.call("getnameinfo", &[sa, 1, hb, 64, sb, 16, 0]), 5);
    assert_eq!(r.call("getnameinfo", &[sa6, 16, hb, 64, sb, 16, 0]), 5);
    assert!(r.faults("getnameinfo", &[sa, 16, EFAULT_ADDR, 64, sb, 16, 0]));
}

#[test]
fn select_and_pselect_over_sockets() {
    let mut r = rig();
    let a = r.sock(DGRAM);
    let pa = r.bind_lo(a);
    let b = r.sock(DGRAM);
    let set = |r: &mut Rig, at: u32, fds: &[u32]| {
        let mut bits = vec![0u8; 128];
        for fd in fds {
            bits[*fd as usize / 8] |= 1 << (fd % 8);
        }
        r.put(at, &bits)
    };
    let nfds = b.max(a) + 1;
    let rs = set(&mut r, 8000, &[a]);
    let tv = r.put(7900, &[0u32, 0].iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>());
    assert_eq!(r.call("select", &[nfds, rs, 0, 0, tv]), 0);
    assert_eq!(r.get(8000 + a / 8, 1)[0] & (1 << (a % 8)), 0, "cleared: nothing to read");
    // Writable and readable: both sets count.
    assert_eq!(r.sendto(b, b"x", pa), 1);
    let (rs, ws, es) = (set(&mut r, 8000, &[a]), set(&mut r, 8200, &[a, b]), set(&mut r, 8400, &[a]));
    assert_eq!(r.call("pselect", &[nfds, rs, ws, es, tv, 0]), 3);
    assert_ne!(r.get(8000 + a / 8, 1)[0] & (1 << (a % 8)), 0);
    assert_eq!(r.get(8400 + a / 8, 1)[0] & (1 << (a % 8)), 0, "no exceptional conditions");
    // Timeout in microseconds / nanoseconds; waits when nothing is ready.
    assert_eq!(r.recvfrom(a, 8, 0).0, 1);
    let rs = set(&mut r, 8000, &[a]);
    let tv = r.put(7900, &[0u32, 20_000].iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>());
    let t = std::time::Instant::now();
    assert_eq!(r.call("select", &[nfds, rs, 0, 0, tv]), 0);
    assert!(t.elapsed() >= std::time::Duration::from_millis(20));
    let ts = r.put(7900, &[0u32, 20_000_000].iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>());
    let rs = set(&mut r, 8000, &[a]);
    assert_eq!(r.call("pselect", &[nfds, rs, 0, 0, ts, 0]), 0);
    // Without sets: a sleep. With no timeout and data arriving later it blocks until then.
    let tv = r.put(7900, &[0u32, 10_000].iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>());
    assert_eq!(r.call("select", &[0, 0, 0, 0, tv]), 0);
    let rs = set(&mut r, 8000, &[a]);
    assert_eq!(r.sendto(b, b"y", pa), 1);
    assert_eq!(r.call("select", &[nfds, rs, 0, 0, 0]), 1);
    // Descriptors this table does not own, or too many: bionic's answer (nothing ready).
    let rs = set(&mut r, 8000, &[3]);
    assert_eq!(r.call("select", &[4, rs, 0, 0, 0]), 0);
    assert_eq!(r.call("select", &[2000, rs, 0, 0, 0]), 0);
    // Faults.
    assert!(r.faults("select", &[nfds, EFAULT_ADDR, 0, 0, 0]));
    let rs = set(&mut r, 8000, &[a]);
    assert!(r.faults("select", &[nfds, rs, 0, 0, EFAULT_ADDR]));
    let tv0 = r.put(7900, &[0u32, 0].iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>());
    assert!(r.faults("select", &[nfds, rs, EFAULT_ADDR, 0, tv0]));
    let _ = EPOLLOUT;
}

#[test]
fn virtual_socket_pairs_speak_the_socket_api() {
    let mut r = rig();
    let out = r.scratch + 600;
    assert_eq!(r.call("socketpair", &[1, 1, 0, out]), 0);
    let (a, b) = (r.cpu.mem().read_u32(out).unwrap(), r.cpu.mem().read_u32(out + 4).unwrap());
    let p = r.put(100, b"ping");
    assert_eq!(r.call("send", &[a, p, 4, 0]), 4);
    assert_eq!(r.call("recv", &[b, r.scratch + 200, 16, 0]), 4);
    assert_eq!(r.get(200, 4), b"ping");
    assert_eq!(r.call("sendto", &[b, p, 2, 0, 0, 0]), 2);
    assert_eq!(r.call("recvfrom", &[a, r.scratch + 200, 16, 0, 0, 0]), 2);
    let one = r.put(300, &1u32.to_le_bytes());
    assert_eq!(r.call("setsockopt", &[a, 1, 2, one, 4]), 0);
    let lenp = r.put(310, &4u32.to_le_bytes());
    r.put(320, &[9; 4]);
    assert_eq!(r.call("getsockopt", &[a, 1, 4, r.scratch + 320, lenp]), 0);
    assert_eq!(r.get(320, 4), [0; 4]);
    assert!(r.faults("getsockopt", &[a, 1, 4, EFAULT_ADDR, lenp]));
    assert!(r.faults("getsockopt", &[a, 1, 4, r.scratch + 320, EFAULT_ADDR]));
    assert_eq!(r.call("shutdown", &[a, 2]), 0);
}

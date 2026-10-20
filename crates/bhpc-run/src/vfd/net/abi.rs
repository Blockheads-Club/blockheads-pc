//! The guest speaks the Linux socket ABI; this translates it to the host's (Linux, macOS or Windows).
//!
//! Everything that differs between the three is data in a [`HostAbi`]: the `AF_INET6` number, the `sockaddr`
//! header (BSD hosts start it with a length byte), socket option numbers, `MSG_*` flags and `errno` values. Each
//! host's table is plain numbers, so all three are tested on every host; [`HOST`] is the one this build talks to.
// The other hosts' tables are used by the tests only.
#![cfg_attr(not(test), allow(dead_code))]

/// Guest (Linux) constants used here and by `net.rs`.
pub(super) mod linux {
    pub const AF_INET: u32 = 2;
    pub const AF_INET6: u32 = 10;
    pub const SOL_SOCKET: u32 = 1;
    pub const SO_ERROR: u32 = 4;
    pub const MSG_OOB: u32 = 0x1;
    pub const MSG_PEEK: u32 = 0x2;
    pub const MSG_TRUNC: u32 = 0x20;
    pub const MSG_DONTWAIT: u32 = 0x40;
    pub const ENOPROTOOPT: u32 = 92;
    pub const EIO: u32 = 5;
}

const IPPROTO_IP: u32 = 0;
const IPPROTO_TCP: u32 = 6;
const IPPROTO_IPV6: u32 = 41;

/// How one host differs from the guest's Linux socket ABI.
#[derive(Debug)]
pub(crate) struct HostAbi {
    /// The host's `AF_INET6`.
    pub af_inet6: u16,
    /// `sockaddr` starts with `sa_len` (u8) and a one-byte family (BSD), not a 16-bit family.
    pub sa_len: bool,
    /// The host's `SOL_SOCKET`.
    pub sol_socket: i32,
    /// `(level, guest name, host name)`; `None` means every option passes through unchanged (Linux).
    pub opts: Option<&'static [(u32, u32, i32)]>,
    /// `(guest flag, host flag)` of the `MSG_*` flags the host's `send`/`recv` take.
    pub msg: &'static [(u32, i32)],
    /// `(host errno, guest errno)`; values not listed below `errno_same_below` are equal on both, anything else
    /// becomes `EIO`. `None` passes every value through (Linux).
    pub errno: Option<&'static [(i32, u32)]>,
    pub errno_same_below: i32,
}

/// Socket options both BSD-derived hosts know, by level: `(level, guest name, macOS name, Windows name)`.
/// A Windows name of -1 means "no such option".
const OPTS: &[(u32, u32, i32, i32)] = &[
    (linux::SOL_SOCKET, 1, 0x1, 0x1),        // SO_DEBUG
    (linux::SOL_SOCKET, 2, 0x4, 0x4),        // SO_REUSEADDR
    (linux::SOL_SOCKET, 3, 0x1008, 0x1008),  // SO_TYPE
    (linux::SOL_SOCKET, 4, 0x1007, 0x1007),  // SO_ERROR
    (linux::SOL_SOCKET, 5, 0x10, 0x10),      // SO_DONTROUTE
    (linux::SOL_SOCKET, 6, 0x20, 0x20),      // SO_BROADCAST
    (linux::SOL_SOCKET, 7, 0x1001, 0x1001),  // SO_SNDBUF
    (linux::SOL_SOCKET, 8, 0x1002, 0x1002),  // SO_RCVBUF
    (linux::SOL_SOCKET, 9, 0x8, 0x8),        // SO_KEEPALIVE
    (linux::SOL_SOCKET, 10, 0x100, 0x100),   // SO_OOBINLINE
    (linux::SOL_SOCKET, 13, 0x80, -1),       // SO_LINGER (Windows' `linger` has 16-bit fields)
    (linux::SOL_SOCKET, 15, 0x200, -1),      // SO_REUSEPORT
    (linux::SOL_SOCKET, 18, 0x1004, 0x1004), // SO_RCVLOWAT
    (linux::SOL_SOCKET, 19, 0x1003, 0x1003), // SO_SNDLOWAT
    (IPPROTO_IP, 1, 3, 3),                   // IP_TOS
    (IPPROTO_IP, 2, 4, 4),                   // IP_TTL
    (IPPROTO_IP, 32, 9, 9),                  // IP_MULTICAST_IF
    (IPPROTO_IP, 33, 10, 10),                // IP_MULTICAST_TTL
    (IPPROTO_IP, 34, 11, 11),                // IP_MULTICAST_LOOP
    (IPPROTO_IP, 35, 12, 12),                // IP_ADD_MEMBERSHIP
    (IPPROTO_IP, 36, 13, 13),                // IP_DROP_MEMBERSHIP
    (IPPROTO_IPV6, 16, 4, 4),                // IPV6_UNICAST_HOPS
    (IPPROTO_IPV6, 17, 9, 9),                // IPV6_MULTICAST_IF
    (IPPROTO_IPV6, 18, 10, 10),              // IPV6_MULTICAST_HOPS
    (IPPROTO_IPV6, 19, 11, 11),              // IPV6_MULTICAST_LOOP
    (IPPROTO_IPV6, 20, 12, 12),              // IPV6_ADD_MEMBERSHIP (JOIN_GROUP)
    (IPPROTO_IPV6, 21, 13, 13),              // IPV6_DROP_MEMBERSHIP (LEAVE_GROUP)
    (IPPROTO_IPV6, 26, 27, 27),              // IPV6_V6ONLY
    (IPPROTO_TCP, 1, 1, 1),                  // TCP_NODELAY
    (IPPROTO_TCP, 4, 0x10, 3),               // TCP_KEEPIDLE (macOS: TCP_KEEPALIVE)
    (IPPROTO_TCP, 5, 0x101, 17),             // TCP_KEEPINTVL
    (IPPROTO_TCP, 6, 0x102, 16),             // TCP_KEEPCNT
];

const fn bsd_opts<const N: usize>(windows: bool) -> [(u32, u32, i32); N] {
    let mut out = [(0, 0, -1); N];
    let mut i = 0;
    while i < N {
        let (level, name, mac, win) = OPTS[i];
        out[i] = (level, name, if windows { win } else { mac });
        i += 1;
    }
    out
}

const MAC_OPTS: [(u32, u32, i32); OPTS.len()] = bsd_opts(false);
const WIN_OPTS: [(u32, u32, i32); OPTS.len()] = bsd_opts(true);

/// Socket `errno` values of BSD (macOS) that differ from Linux. 1..=34 coincide except `EDEADLK`/`EAGAIN`.
const MAC_ERRNO: &[(i32, u32)] = &[
    (11, 35), // EDEADLK
    (35, 11), // EAGAIN / EWOULDBLOCK
    (36, 115),
    (37, 114),
    (38, 88),
    (39, 89),
    (40, 90),
    (41, 91),
    (42, 92),
    (43, 93),
    (44, 94),
    (45, 95),
    (46, 96),
    (47, 97),
    (48, 98),
    (49, 99),
    (50, 100),
    (51, 101),
    (52, 102),
    (53, 103),
    (54, 104),
    (55, 105),
    (56, 106),
    (57, 107),
    (58, 108),
    (60, 110),
    (61, 111),
    (62, 40), // ELOOP
    (63, 36), // ENAMETOOLONG
    (64, 112),
    (65, 113),
    (102, 95), // EOPNOTSUPP (macOS has it apart from ENOTSUP)
];

/// `WSAGetLastError()` codes and the few plain Win32 errors a socket call returns.
const WIN_ERRNO: &[(i32, u32)] = &[
    (6, 9),      // ERROR_INVALID_HANDLE -> EBADF
    (8, 12),     // ERROR_NOT_ENOUGH_MEMORY -> ENOMEM
    (10004, 4),  // WSAEINTR
    (10009, 9),  // WSAEBADF
    (10013, 13), // WSAEACCES
    (10014, 14), // WSAEFAULT
    (10022, 22), // WSAEINVAL
    (10024, 24), // WSAEMFILE
    (10035, 11), // WSAEWOULDBLOCK -> EAGAIN
    (10036, 115),
    (10037, 114),
    (10038, 88),
    (10039, 89),
    (10040, 90),
    (10041, 91),
    (10042, 92),
    (10043, 93),
    (10044, 94),
    (10045, 95),
    (10046, 96),
    (10047, 97),
    (10048, 98),
    (10049, 99),
    (10050, 100),
    (10051, 101),
    (10052, 102),
    (10053, 103),
    (10054, 104),
    (10055, 105),
    (10056, 106),
    (10057, 107),
    (10058, 108),
    (10060, 110),
    (10061, 111),
    (10064, 112),
    (10065, 113),
    (10093, 22), // WSANOTINITIALISED (cannot happen after start-up) -> EINVAL
];

pub(crate) const LINUX: HostAbi = HostAbi {
    af_inet6: 10,
    sa_len: false,
    sol_socket: 1,
    opts: None,
    msg: &[(linux::MSG_OOB, 0x1), (linux::MSG_PEEK, 0x2), (linux::MSG_TRUNC, 0x20), (linux::MSG_DONTWAIT, 0x40)],
    errno: None,
    errno_same_below: 0,
};

pub(crate) const MACOS: HostAbi = HostAbi {
    af_inet6: 30,
    sa_len: true,
    sol_socket: 0xffff,
    opts: Some(&MAC_OPTS),
    msg: &[(linux::MSG_OOB, 0x1), (linux::MSG_PEEK, 0x2), (linux::MSG_DONTWAIT, 0x80)],
    errno: Some(MAC_ERRNO),
    errno_same_below: 35,
};

pub(crate) const WINDOWS: HostAbi = HostAbi {
    af_inet6: 23,
    sa_len: false,
    sol_socket: 0xffff,
    opts: Some(&WIN_OPTS),
    // No MSG_DONTWAIT: host sockets are non-blocking anyway.
    msg: &[(linux::MSG_OOB, 0x1), (linux::MSG_PEEK, 0x2)],
    errno: Some(WIN_ERRNO),
    errno_same_below: 0,
};

/// The ABI of the host this build runs on.
#[cfg(target_os = "macos")]
pub(crate) const HOST: &HostAbi = &MACOS;
#[cfg(windows)]
pub(crate) const HOST: &HostAbi = &WINDOWS;
#[cfg(not(any(target_os = "macos", windows)))]
pub(crate) const HOST: &HostAbi = &LINUX;

impl HostAbi {
    /// The host family for a guest `AF_INET`/`AF_INET6`.
    pub fn family(&self, guest: u32) -> i32 {
        if guest == linux::AF_INET6 {
            i32::from(self.af_inet6)
        } else {
            guest as i32
        }
    }

    /// A guest `sockaddr_in`/`sockaddr_in6` in the host's layout (the bytes after the family are the same).
    pub fn addr_to_host(&self, guest: &[u8]) -> Vec<u8> {
        let mut v = guest.to_vec();
        let fam = u16::from_le_bytes([guest[0], guest[1]]);
        let host_fam = if u32::from(fam) == linux::AF_INET6 { self.af_inet6 } else { fam };
        if self.sa_len {
            v[0] = guest.len().min(255) as u8;
            v[1] = host_fam as u8;
        } else {
            v[..2].copy_from_slice(&host_fam.to_le_bytes());
        }
        v
    }

    /// A host socket address in the guest's (Linux) layout.
    pub fn addr_to_guest(&self, host: &[u8]) -> Vec<u8> {
        let mut v = host.to_vec();
        if v.len() < 2 {
            return v;
        }
        let fam = if self.sa_len { u16::from(host[1]) } else { u16::from_le_bytes([host[0], host[1]]) };
        let guest_fam = if fam == self.af_inet6 { linux::AF_INET6 as u16 } else { fam };
        v[..2].copy_from_slice(&guest_fam.to_le_bytes());
        v
    }

    /// The host's `(level, name)` for a guest socket option, or `ENOPROTOOPT`.
    pub fn opt(&self, level: u32, name: u32) -> Result<(i32, i32), u32> {
        let Some(table) = self.opts else { return Ok((level as i32, name as i32)) };
        let host_level = if level == linux::SOL_SOCKET { self.sol_socket } else { level as i32 };
        match table.iter().find(|(l, n, _)| *l == level && *n == name) {
            Some(&(_, _, h)) if h >= 0 => Ok((host_level, h)),
            _ => Err(linux::ENOPROTOOPT),
        }
    }

    /// The host `MSG_*` flags for the guest's (the ones the host lacks are dropped).
    pub fn msg_flags(&self, guest: u32) -> i32 {
        self.msg.iter().filter(|(g, _)| guest & g != 0).fold(0, |acc, (_, h)| acc | h)
    }

    /// A host `errno` (or `WSAGetLastError()` code) as the guest's.
    pub fn errno(&self, host: i32) -> u32 {
        let Some(table) = self.errno else { return host as u32 };
        match table.iter().find(|(h, _)| *h == host) {
            Some(&(_, g)) => g,
            None if (0..self.errno_same_below).contains(&host) => host as u32,
            None => linux::EIO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sin6() -> Vec<u8> {
        let mut v = vec![10, 0, 0x1f, 0x90];
        v.extend([0; 24]);
        v
    }

    #[test]
    fn addresses_follow_each_hosts_layout() {
        let sin = [2u8, 0, 0, 80, 127, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(LINUX.addr_to_host(&sin), sin);
        assert_eq!(LINUX.addr_to_host(&sin6()), sin6());
        assert_eq!(&WINDOWS.addr_to_host(&sin6())[..4], &[23, 0, 0x1f, 0x90]);
        assert_eq!(&WINDOWS.addr_to_host(&sin)[..2], &[2, 0]);
        assert_eq!(&MACOS.addr_to_host(&sin6())[..4], &[28, 30, 0x1f, 0x90]);
        assert_eq!(&MACOS.addr_to_host(&sin)[..2], &[16, 2]);
        for abi in [&LINUX, &MACOS, &WINDOWS] {
            assert_eq!(abi.addr_to_guest(&abi.addr_to_host(&sin)), sin, "{abi:?}");
            assert_eq!(abi.addr_to_guest(&abi.addr_to_host(&sin6())), sin6());
            assert_eq!(abi.addr_to_guest(&[7]), [7], "too short to carry a family");
            assert_eq!(abi.family(linux::AF_INET), 2);
            assert_eq!(abi.family(linux::AF_INET6), i32::from(abi.af_inet6));
        }
    }

    #[test]
    fn options_translate_or_are_refused() {
        assert_eq!(LINUX.opt(1, 9999), Ok((1, 9999)), "Linux passes everything through");
        assert_eq!(MACOS.opt(1, 2), Ok((0xffff, 4)));
        assert_eq!(WINDOWS.opt(1, 4), Ok((0xffff, 0x1007)));
        assert_eq!(MACOS.opt(41, 26), Ok((41, 27)));
        assert_eq!(WINDOWS.opt(6, 4), Ok((6, 3)));
        assert_eq!(MACOS.opt(6, 4), Ok((6, 0x10)));
        assert_eq!(MACOS.opt(1, 15), Ok((0xffff, 0x200)));
        assert_eq!(WINDOWS.opt(1, 15), Err(linux::ENOPROTOOPT));
        assert_eq!(WINDOWS.opt(0, 35), Ok((0, 12)));
        assert_eq!(MACOS.opt(1, 9999), Err(linux::ENOPROTOOPT));
        // Every row made it into both tables (also evaluated at run time, for coverage).
        assert_eq!(bsd_opts::<{ OPTS.len() }>(false), MAC_OPTS);
        assert_eq!(MAC_OPTS.len(), OPTS.len());
        assert!(OPTS.iter().zip(MAC_OPTS).all(|(o, m)| (o.0, o.1, o.2) == m));
        assert!(OPTS.iter().zip(WIN_OPTS).all(|(o, w)| (o.0, o.1, o.3) == w));
    }

    #[test]
    fn message_flags() {
        let all = linux::MSG_OOB | linux::MSG_PEEK | linux::MSG_TRUNC | linux::MSG_DONTWAIT | 0x4000;
        assert_eq!(LINUX.msg_flags(all), 0x63);
        assert_eq!(MACOS.msg_flags(all), 0x83);
        assert_eq!(WINDOWS.msg_flags(all), 0x3);
        assert_eq!(WINDOWS.msg_flags(0), 0);
    }

    #[test]
    fn errno_values() {
        assert_eq!(LINUX.errno(111), 111);
        assert_eq!(MACOS.errno(35), 11);
        assert_eq!(MACOS.errno(11), 35);
        assert_eq!(MACOS.errno(61), 111);
        assert_eq!(MACOS.errno(22), 22, "low values coincide");
        assert_eq!(MACOS.errno(99), linux::EIO, "unknown");
        assert_eq!(WINDOWS.errno(10035), 11);
        assert_eq!(WINDOWS.errno(10061), 111);
        assert_eq!(WINDOWS.errno(10054), 104);
        assert_eq!(WINDOWS.errno(22), linux::EIO, "Winsock codes only");
    }

    #[test]
    fn the_host_table_matches_the_build() {
        #[cfg(windows)]
        let want = 23;
        #[cfg(target_os = "macos")]
        let want = 30;
        #[cfg(not(any(windows, target_os = "macos")))]
        let want = 10;
        assert_eq!(HOST.af_inet6, want);
    }
}

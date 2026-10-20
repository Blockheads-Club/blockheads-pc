# Guest sockets on Windows and macOS

**Status: implemented.** `crates/bhpc-run/src/vfd/net/abi.rs` holds the guest (Linux) to host tables (families,
`sockaddr` header, option numbers, `MSG_*` flags, `errno`) for Linux, macOS and Windows, tested on every host;
`net/sys_unix.rs` (Linux, macOS) and `net/sys_windows.rs` (Winsock) are the thin host layers. Beyond the plan
below: Windows UDP sockets get `SIO_UDP_CONNRESET` off (an ICMP port-unreachable must not fail the next
`recvfrom`), `WSAEMSGSIZE` on a datagram larger than the buffer reads as a truncated datagram (Linux
semantics), readiness uses `select` (older `WSAPoll` misses failed connects), macOS sets `SO_NOSIGPIPE` and
`O_NONBLOCK`/`FD_CLOEXEC` with `fcntl` (no `SOCK_*` flags, no `accept4`). The original plan follows.

`crates/bhpc-run/src/vfd/net.rs` serves the guest's BSD socket calls with non-blocking host sockets through `libc`
(Linux only: it passes Linux constants and `accept4` through unchanged). On other hosts, macOS included,
`vfd/net_stub.rs` is compiled instead: bionic's failing stubs stay, so there is no
online play, and `host_has_lan()` is false. `ci-crossplat.yml` builds only `bhpc-mem`/`bhpc-cpu` on Windows, so
`bhpc-run` itself is not built there yet; that comes first.

## What the port needs

1. **Host socket type.** Replace `OwnedFd` + `libc::*` calls with a small `sys` layer (`sys_unix.rs`,
   `sys_windows.rs`) exposing: `socket`, `bind`, `listen`, `connect`, `accept`, `sendto`, `recvfrom`,
   `setsockopt`/`getsockopt` (raw bytes), `getsockname`/`getpeername`, `shutdown`, `poll(events)`,
   `pending()` (FIONREAD), `so_error()`. Windows: `windows-sys` `Win32_Networking_WinSock` (`WSAStartup` once,
   `SOCKET`, `ioctlsocket(FIONBIO/FIONREAD)`, `WSAPoll`, `closesocket` in `Drop`).
2. **Errors.** Map `WSAGetLastError()` to the Linux errno values the guest expects (`WSAEWOULDBLOCK` -> `EAGAIN`,
   `WSAEINPROGRESS`/`WSAEWOULDBLOCK` on connect -> `EINPROGRESS`, `WSAECONNREFUSED` -> 111, `WSAECONNRESET` -> 104,
   `WSAEADDRINUSE` -> 98, `WSAENOTCONN` -> 107, `WSAETIMEDOUT` -> 110, `WSAEAFNOSUPPORT` -> 97, `WSAEINVAL` -> 22).
3. **Constants differ** (the unix version passes them through unchanged): `AF_INET6` is 23, `SOL_SOCKET` is
   0xffff, `SO_REUSEADDR` 4, `SO_BROADCAST` 0x20, `SO_RCVBUF`/`SO_SNDBUF` 0x1002/0x1001, `SO_ERROR` 0x1007,
   `IP_MULTICAST_IF/TTL/LOOP`, `IP_ADD/DROP_MEMBERSHIP` 9..13, `IPV6_*` likewise; `TCP_NODELAY` is 1 on both.
   Translate guest (Linux) level/name pairs in one table; unknown options answer `ENOPROTOOPT`.
   `sockaddr_in6` and the family field must be rewritten in both directions (`AF_INET6` 10 <-> 23).
4. **Flags.** No `MSG_NOSIGNAL`/`MSG_DONTWAIT` on Winsock (no SIGPIPE anyway; non-blocking is per socket);
   `accept4` becomes `accept` + `FIONBIO`.
5. **Readiness.** `ready()`/`select_in` use `WSAPoll` with a zero timeout; the 5 ms slice stays.
6. **Interfaces.** `host_has_lan()` via `GetAdaptersAddresses` (any up IPv4 adapter that is not loopback).
7. **Tests.** `tests_net.rs` is written against loopback and should run unchanged on Windows; add
   `bhpc-run` (`cargo test -p bhpc-run --lib`) to the Windows job once it builds there. Coverage stays measured on
   Linux (the Windows layer is excluded like other `cfg(windows)` code).

Size: roughly 400-600 lines (sys layer, constant/errno tables, adapter listing). Unverified: nothing here has run on
Windows.

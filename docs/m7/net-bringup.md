# M7: guest networking and the first online join

## What the guest sees

`crates/bhpc-run/src/vfd/net.rs` implements BSD sockets over **non-blocking host sockets** (`libc`, unix only; other
platforms keep bionic's failing stubs through `vfd/net_stub.rs`). The guest descriptor numbers come from the same
table as pipes, socket pairs, eventfds and epoll (`FD_BASE` = 600 and up), so `epoll_ctl`/`epoll_wait`, `poll` and
`select`/`pselect` mix sockets with the other kinds.

* Calls: `socket bind listen connect accept accept4 send sendto recv recvfrom sendmsg recvmsg setsockopt getsockopt
  getsockname getpeername shutdown`, plus `read`/`write`/`close`/`dup`/`fcntl`/`ioctl(FIONREAD, FIONBIO)` on them.
  Families `AF_INET` and `AF_INET6`, types stream and datagram. Option, level and flag numbers and the
  `sockaddr_in`/`sockaddr_in6` layouts are the same on ARM EABI Linux and the host, so they pass through as bytes;
  `msghdr`/`iovec` (28 and 8 bytes on ARM32) are decoded and gathered/scattered by hand.
* Blocking guest calls retry every 5 ms and hand the baton to other guest threads in between
  (`sched::block`); `SO_RCVTIMEO`/`SO_SNDTIMEO` are kept per socket and honoured. epoll/poll/select over host
  sockets also wake every 5 ms (nothing notifies the scheduler about socket readiness).
* `send`/`recv`/`setsockopt`/`getsockopt`/`shutdown` also accept the two ends of a virtual `socketpair`
  (libkqueue's wake-up pipe uses them): data goes through the channel, options read as zero.
* Resolver: `getaddrinfo` (numeric hosts, `localhost`, real DNS through the host, `AI_PASSIVE`/`AI_CANONNAME`/
  `AI_NUMERICHOST`, service names `http https ftp domain ntp` or numbers) builds a bionic `addrinfo` chain in one guest
  heap block (`freeaddrinfo` frees it); `gethostbyname`/`gethostbyname2` fill one static `hostent`; `getnameinfo` is
  numeric. `AF_UNIX` sockets are not supported (`EAFNOSUPPORT`).
* `BHPC_TRACE_NET=1` logs every socket call (arguments, result, errno), `getaddrinfo` names, `connect` targets and
  the first 64 bytes of every datagram/segment sent.

Everything is covered by unit tests with guest-side calls over host loopback (`tests_net.rs`, no APK).

## Findings that were needed to get a game session going

1. `play.rs::watch_blocking` re-registered `read`, `poll` and `select` as plain bionic builtins *after* the descriptor
   table had registered its own, so those three never reached `vfd.rs` (and a thread spun in `select` with 6M calls
   in 25 s). It now runs before `vfd::install`.
2. The game handles input events on the **Android UI guest thread**, so `performSelector:afterDelay:` timers armed by
   button handlers land in that thread's CFRunLoop, which nobody ran: "Join Online World" did nothing. The UI loop
   now calls `CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0, true)` every iteration.
3. `--trace-objc` lines carry the guest thread id (`t0` = main, `t2` = UI).

## Online join against `blockheads-server` (1.7.4 client, Join World > Advanced > Online IP/Port)

```
blockheads_server --new networld --port 15160          # BLOCKHEADS_DATA_DIR=/tmp/...
blockheads_server --load <world id> --port 15160
SDL_AUDIODRIVER=dummy SDL_VIDEODRIVER=offscreen bhpc-run --apk <1.7.4 apk> --script join.script --shot-dir DIR
```

`join.script` (times in ms after the first frame; the port field is pre-filled with 15151, so it is cleared first):

```
5000 click 312 130          # Join World portal
+3000 click 512 360         # Advanced...
+3000 click 512 360         # Online IP/Port...
+3000 click 580 408         # server port field -> text alert
+2000 key backspace         # x5
+200  text 15160
+500  key enter
+1500 click 580 456         # nickname
+2000 text Tester
+500  key enter
+1500 click 512 552         # Join Online World
+3000 shot joined_1
+18000 shot in_world
```

The server logs `Player Connected TESTER | 127.0.0.1 | <id>` and the client shows the in-world HUD with the portal at the
spawn point.

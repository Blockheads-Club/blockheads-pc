# Parallel guest threads

Status: **on by default**. `--serial-guest` / `BHPC_PARALLEL_GUEST=0` (the launcher's "Run game threads in
parallel" switch, Window & graphics, off) bring back the old scheduler for troubleshooting; `--parallel-guest` is still
accepted. See "Results" and "Turning it on by default".

## Before: the baton

Every guest thread is a host thread with its own `Cpu` (its own dynarmic `Jit`), but only the holder of the scheduler
*baton* (`bhpc-bionic/src/sched.rs`, a FIFO ticket lock) ran guest code or host functions. A thread gave it up only
when it blocked, yielded, slept or its 2M-cycle time slice ran out. So at most one host core ran the guest, and every
host function could assume it ran alone.

Profile of the baseline (per-thread CPU from `/proc/<pid>/task/*/stat`, menu + world creation + 25 s in a world,
`--unlock-fps --fps-limit 0 --no-vsync`, Xvfb/llvmpipe):

* `guest-main` (the game's main thread: game logic, rendering) is 70-100 % of one core while the world is created
  and loaded; in the world the frame is dominated by llvmpipe (software GL under Xvfb).
* Ten libdispatch worker threads (`guest-3..12`) each burned **10-15 % of a core doing nothing**: every
  `sched::notify` (any mutex unlock, condition signal, futex wake, thread exit) bumped one global epoch and woke
  *every* blocked thread, which queued for the baton, re-checked its condition and went back to sleep.
* Thunk statistics (`BHPC_THUNK_STATS=1`, new) over one run: ~8 M host calls in ~110 s. The bulk are
  `pthread_mutex_lock/unlock` (2.0-2.5 M), `memcpy`/`memset`/`memmove`/`str*` (4.7 M), `pthread_getspecific`
  (0.8 M); `cacheflush` 11 k (!).

## Design

### 1. Targeted wake-ups (both modes)

Waits are keyed (`sched::Key`): `Addr(guest object)`, `Any` (epoll/poll/select/pipe reads: woken by every
notification, as before) or `Sleep` (only the deadline ends it). `notify_key(addr)` wakes only the waiters of that
address (and `Any` waiters); `notify()` wakes everyone but sleepers. Each waiter has its own condition variable, and the
deadlock check counts exactly the waiters without a deadline that nothing has woken (before: "blocked since the last
epoch"). The pthread HLE uses the mutex / condition variable / futex word / once control / rwlock / thread handle as
key. This alone removes the idle workers' CPU burn, in the serial mode too.

### 2. The baton becomes a host lock (parallel mode)

`bhpc_cpu::HostGate` is a hook the CPU calls around guest code: `Cpu::run` gives the gate up before it enters the JIT
and takes it back when the JIT returns; the `svc` callback takes it before a thunk and gives it up after. A thread
therefore holds the lock exactly while it runs host code, and guest code of all threads runs in parallel on host
cores. `Sched` implements the gate (`sched::enable_parallel`, called by `bhpc-run` right after the main CPU exists):

* the lock is an atomic owner word with a short spin (256 iterations, yielding every 64) and then parking on a
  condition variable. It is not FIFO: a convoy of hand-overs at every thunk would be much slower;
* `yield_now` releases only when another thread wants the lock, and waits until one got it (no time limit: a
  yielder that gave up and took the lock back could keep it for good, see "Turning it on by default");
* blocking (`wait_on`) releases the lock, sleeps, and takes it back; a host thread that does not hold it (the OpenSL ES
  pump, which drives its own guest CPU) is left alone (`leave` returns false), so it no longer races the guest threads
  in host code either: its thunks now take the lock;
* nested guest calls from inside a thunk (`qsort` comparators, JNI natives, the UI thread's whole life) release the
  lock while the nested guest code runs, exactly like a top-level call. That is safe because no host function holds a
  `std::sync` lock across a nested guest call (it would self-deadlock in the serial mode already);
* time slices no longer hand anything over (a spinning thread spins on its own core); they still enforce the
  wall-clock watchdog.

### 3. Gate-free host functions

With one lock for all host code, the hot pure functions and the pthread mutex fast path would convoy on it. Thunks can
therefore be marked gate-free (`ThunkTable::set_gate_free`; `bhpc_bionic::gate_free` lists the built-ins, `resolve`
marks them; rebinding a name clears the mark). They run without the host lock:

* `string.h` (minus `strerror`/`strsignal`, which cache, and `rand48`), all of `libm` and `ctype`: pure functions of
  their arguments and guest memory;
* `pthread_mutex_lock/trylock/unlock`, `pthread_cond_wait/timedwait*/signal/broadcast`, `__futex_wait/__futex_wake`,
  `pthread_getspecific/setspecific`. They were rewritten so that every check-and-update is **one** critical section
  of the `State` lock (before: separate `lock(st)` calls that only the baton made atomic), the condition-variable wait
  queues itself and releases the mutex in one step, the futex compares the word and queues in one step (like the
  kernel), and a waiter reads the key's notification counter (`Sched::key_seq`, hashed per address, plus a global
  one) in the same step that checks its condition; `Sched::wait_seq` returns at once if the counter moved, so a waker
  that does not hold the host lock cannot slip between check and sleep. A mutex counts its waiters, so an
  uncontended unlock does not touch the scheduler at all.

Everything else (stdio, file system, heap, JNI, GL/EGL, audio, sockets, `pthread_once`/rwlock/join/create) stays
behind the host lock and keeps its "one at a time" assumptions.

### 4. JIT code invalidation across threads (both modes)

`cacheflush` used to invalidate only the calling thread's JIT chain; other threads kept running stale translations
(a real bug already in the serial mode, the game calls it ~11 k times). `Cpu::invalidate`/`clear_cache` now also
publish the range in a shared log; every CPU applies what it has not seen when it enters the JIT and after every thunk
(dynarmic allows invalidation from inside a callback). A CPU more than 1024 invalidations behind drops its whole
cache. A thread that spins in guest code without calling a thunk sees the flush at its next thunk or JIT entry; real
code publishes new code through a lock or a futex, which are thunks.

## Thread-safety audit

| Area | Status |
| --- | --- |
| dynarmic `Jit` | one per guest thread (and per nested-call child); never shared |
| Exclusive monitor (`LDREX`/`STREX`) | one `Dynarmic::ExclusiveMonitor` for all CPUs (internally locked); `STREX` goes through `MemoryWriteExclusive*`, implemented as a host CAS on guest memory, so it is atomic across host threads (ABA-tolerant like every dynarmic user). Test: 4 threads x 65536 `ldrex/add/strex` increments, none lost |
| Memory ordering | guest ARMv7 is weaker than x86-64 TSO; dynarmic emits plain loads/stores and fences for `DMB`/`DSB`, so guest code sees at least the ordering it asks for. Rust host code that reads guest memory another thread writes is a data race in the Rust model (as it was with the JIT already); `GuestMemory` uses raw-pointer copies |
| Guest memory (`bhpc-mem`) | `Send + Sync`, fixed fastmem reservation; `map`/`unmap` change page protection only; reads of unmapped pages fault cleanly |
| Thunk table | `RwLock`; per-entry gate-free flag and call statistics |
| Host state (`State`, `Gles`, `Vm`, `Sles`) | every shared closure is `Send + Sync`, so the compiler already guarantees data-race freedom; what parallelism adds are logical races between separate critical sections. Those are excluded by the host lock for everything that is not gate-free, and by the single-section rewrite for what is |
| GL | contexts are per host thread (`eglMakeCurrent` -> `SDL_GL_MakeCurrent` on the calling thread); GL thunks are gated |
| JNI (`bhpc-jni`) | gated; local reference frames are per processor id |
| Audio | the OpenSL ES pump runs its own CPU outside the scheduler; its thunks now take the host lock |
| VFS / fds | gated |
| Signals | `sigaction`/`kill`/`raise`/`pthread_kill` are stubs; nothing to do |
| Blocking primitives | stay scheduler waits (`Sched::wait_on`/`wait_seq`), so the deadlock detector and the wall-clock watchdog keep working; in parallel mode a waiting thread does not hold the host lock |

## Tests

* `bhpc-cpu`: gate enter/leave around guest code, thunks and nested calls; gate-free thunks; per-thunk statistics;
  invalidations reaching sibling CPUs, the clear-all broadcast and the log overflow.
* `bhpc-bionic`: every scheduler/pthread test also runs in parallel mode (`tests_env::in_parallel`); guest code of two
  threads spinning/setting a flag at the same time without time slices (impossible with the baton); lost-update test
  for exclusive increments; the host lock (spin, park, yield hand-over, holder rules) and keyed waits.
* `bhpc-run`: a session in parallel mode; the flag/environment precedence.

## Results

Benchmark: `bhpc-run --apk ... --unlock-fps --fps-limit 0 --no-vsync --script bench.script` under
`dbus-run-session xvfb-run` (llvmpipe), fresh data dir per run: 5 s menu, create a world, wait for its "OK" popup,
25 s in the world. Three interleaved rounds of base (`dev` before this branch) / serial (this branch,
`--serial-guest`) / parallel (`--parallel-guest`), medians. **Caveat:** other agents kept the 16-core host at a load
average of 30-50 for the whole session, so wall-clock numbers swing by 2-5x between runs of the same binary; CPU
seconds are the steadier signal.

| median of 3 | base | serial | parallel |
| --- | --- | --- | --- |
| world created (s from the last click to the popup) | 51.8 (15-62) | 29.8 (12-63) | 25.4 (6-46) |
| menu frame time p50 (ms) | 93.6 | 92.2 | 82.7 |
| world FPS | 10.4 | 7.0 (6.3-19.1) | 11.5 |
| world frame time p50 / p99 (ms) | 88.5 / 242 | 135 / 383 | 82.3 / 223 |
| CPU of the guest threads (s per run) | 109.6 | 64.7 | 73.4 |
| CPU of the whole process (s per run) | 172.8 | 116.8 | 143.7 |
| runs reaching the world | 3/3 | 3/3 | 3/3 |

* Keyed wake-ups cut the guest threads' CPU by about 40 % (idle workers no longer spin through the baton), which is
  the one clear, load-independent win; it is on in both modes.
* Parallel mode works (boots, menu, world creation, 3-minute in-world sessions with input, no deadlock, no crash) but
  does not show a gain that stands out of the noise here: The Blockheads does almost all of its work on its main
  thread (game logic and GL), so there is little to run in parallel, and what is gained is partly eaten by host-lock
  traffic (`pthread_mutex_lock/unlock` alone are ~2.5 M calls per run) and lock-holder preemption on an overloaded
  host. That is why the flag first stayed off by default.

## Turning it on by default

Done on a quiet host (load average 4-10, mostly other desktop programs), same binary for both modes.

Stability, parallel mode:

* 10/10 boots to the menu (screenshot of the menu, exit 0).
* 5 scripted sessions under Xvfb of 6-9 minutes each (about 3.5 minutes in a first world, then about 2 in a second
  one): warp in, skip the tutorial, walk, dig, select an item and place, open the blockhead's inventory and the
  portal's crafting, pause and resume, pan; then pause, "Exit to menu" (the world is saved) and create and enter
  another world. All exited normally; no deadlock, no stall in the frame times (max frame 0.6 s, world creation).
  "Re-entering" means a new world: saved worlds are not listed on the world carousel in either mode, also with
  older builds (`--serial-guest` and the build before the parallel work show the same), a separate issue.
* The embedded path (`--embed` with a reader that takes frames like the launcher, EGL, 1024x768): one short run
  and one 6-minute session with the same script, exit 0.
* `cargo test --release -p bhpc-cpu -p bhpc-bionic -p bhpc-run` three times in a row, plus the ignored APK tests.
  This found one hang: `sched::tests::the_parallel_host_lock` hung in about 1 of 20 runs under load (7/150 in a
  loop). `yield_now` waited at most 256 `sched_yield`s for the contender to take the lock and then took it back; the
  woken contender, slow to be scheduled on a busy host, then parked again behind a holder that kept the lock (the test
  joins it). In the game a yielder gives the lock up again at its next guest code, so it cost latency there rather
  than a hang, but the hand-over did not do what it promised. The wait now lasts until a contender has the lock (every
  contender keeps trying until it does). 0 hangs in 550 runs since, and the suites passed three times.

Measurement: `bench.script` as above (5 s menu, create a world, 25 s in it), `--unlock-fps --fps-limit 0 --no-vsync`,
Xvfb/llvmpipe, five interleaved rounds of `--serial-guest` / default, medians (range):

| median of 5 | serial | parallel |
| --- | --- | --- |
| world created (s from the last click to the popup) | 0.6 | 0.6 |
| menu + world creation FPS | 79.3 (74-87) | 80.9 (66-89) |
| menu + world creation frame time p50 / p99 (ms) | 10.2 / 26.6 | 10.8 / 31.2 |
| world FPS | 91.5 (78-99) | 93.0 (83-99) |
| world frame time p50 / p99 (ms) | 9.9 / 29.9 | 10.1 / 30.2 |
| CPU of the guest threads (s per run) | 38.5 | 39.3 |
| CPU of the whole process (s per run) | 341 | 346 |
| runs reaching the world | 5/5 | 5/5 |

* On a quiet host the two modes are equal within the run-to-run spread. The world popup comes up within the
  script's 0.6 s poll in every run, so that number cannot tell them apart. Most of the process CPU is llvmpipe
  rendering uncapped frames. The guest threads use about 0.5 core in both modes, so there is little left to
  run in parallel (as above).
* Parallel mode is the default because it is as stable as the serial one in everything above. It is the basis for the
  remaining work (gate-free GL and heap), and with real GPU drivers more of the frame is guest work. The launcher's
  switch and `--serial-guest` stay for troubleshooting.

## Remaining work

* Make more of the hot gated host functions gate-free: GL draw calls (the `Gles` state has its own lock; EGL swap must
  stay gated because the frame hook delivers input natives), `malloc`/`free` (one `State` section each), `vsnprintf`,
  `qsort`/`bsearch`.
* Split the single `State` mutex: gate-free functions now contend on it.
* Run a long-session stress (hours in a world) and a TSan build of the runner on an idle machine.

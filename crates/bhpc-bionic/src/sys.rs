//! POSIX system-call surface of the sandbox: the three standard descriptors are real, everything that
//! would touch the host file system, network or processes fails with the `errno` a locked-down Android
//! app would see. Anonymous `mmap` is served from the guest heap; `setjmp`/`longjmp`, `qsort`,
//! `bsearch` and `fnmatch` are real implementations.

use crate::guest::{cstr, rd, rd32, wr, wr32, wr_cstr};
use crate::layout::{errno, PAGE_SIZE};
use crate::misc::{fail, set_errno};
use crate::vfs;
use crate::{lock, Impl, Shared};
use bhpc_cpu::Cpu;
use std::io::Write;

macro_rules! args {
    ($c:expr; $($n:ident),+) => {
        let mut a = $c.args();
        $(let $n = a.u32($c);)+
    };
}

/// Functions that return -1 with a fixed errno.
macro_rules! fails {
    ($($id:ident $name:literal $e:expr;)*) => {
        $(fn $id(c: &mut Cpu, st: &Shared) { fail(c, st, $e) })*
        const FAIL_TABLE: &[(&str, Impl)] = &[$(($name, $id)),*];
    };
}

/// Functions that return NULL (with `errno` when it is non-zero).
macro_rules! nulls {
    ($($id:ident $name:literal $e:expr;)*) => {
        $(fn $id(c: &mut Cpu, st: &Shared) {
            if $e != 0 {
                set_errno(c, st, $e);
            }
            c.ret_u32(0);
        })*
        const NULL_TABLE: &[(&str, Impl)] = &[$(($name, $id)),*];
    };
}

fails! {
    f_chmod "chmod" errno::ENOENT; f_utime "utime" errno::ENOENT; f_utimes "utimes" errno::ENOENT;
    f_symlink "symlink" errno::EACCES; f_statfs "statfs" errno::ENOENT; f_execve "execve" errno::ENOENT; f_execv "execv" errno::ENOENT;
    f_pwrite "pwrite" errno::EBADF; f_fchmod "fchmod" errno::EBADF;
    f_fchown "fchown" errno::EBADF; f_fchdir "fchdir" errno::EBADF; f_flock "flock" errno::EBADF;
    f_fstatfs "fstatfs" errno::EBADF; f_dup "dup" errno::EBADF; f_dup2 "dup2" errno::EBADF; f_pipe "pipe" errno::ENOSYS;
    f_socket "socket" errno::EAFNOSUPPORT; f_socketpair "socketpair" errno::EAFNOSUPPORT;
    f_bind "bind" errno::EBADF; f_listen "listen" errno::EBADF; f_accept "accept" errno::EBADF;
    f_connect "connect" errno::EBADF; f_shutdown "shutdown" errno::EBADF; f_send "send" errno::EBADF;
    f_recv "recv" errno::EBADF; f_sendto "sendto" errno::EBADF; f_recvfrom "recvfrom" errno::EBADF;
    f_sendmsg "sendmsg" errno::EBADF; f_recvmsg "recvmsg" errno::EBADF; f_setsockopt "setsockopt" errno::EBADF;
    f_getsockopt "getsockopt" errno::EBADF; f_getsockname "getsockname" errno::EBADF;
    f_getpeername "getpeername" errno::EBADF; f_epoll_create "epoll_create" errno::ENOSYS;
    f_epoll_ctl "epoll_ctl" errno::ENOSYS; f_epoll_wait "epoll_wait" errno::ENOSYS;
    f_inotify_init "inotify_init" errno::ENOSYS; f_inotify_add_watch "inotify_add_watch" errno::ENOSYS;
    f_syscall "syscall" errno::ENOSYS; f_fork "fork" errno::EAGAIN; f_waitpid "waitpid" errno::ECHILD;
    f_sigsuspend "sigsuspend" errno::EINTR;
}

nulls! { n_fgetln "fgetln" 0;
    n_gethostbyname2 "gethostbyname2" 0; n_gethostbyaddr "gethostbyaddr" 0;
    n_if_indextoname "if_indextoname" errno::ENOSYS;
}

/// Functions that simply report success.
fn ok(c: &mut Cpu, _: &Shared) {
    c.ret_u32(0);
}

/// `EAI_NONAME`: the sandbox resolves no names.
fn eai_noname(c: &mut Cpu, _: &Shared) {
    c.ret_u32(8);
}

fn is_std(fd: u32) -> bool {
    fd <= 2
}

/// Guest path argument.
fn path_arg(c: &mut Cpu, addr: u32) -> Option<String> {
    cstr(c, addr).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Finish a call that returns 0 or fails with an `errno` from the file system.
fn done(c: &mut Cpu, st: &Shared, r: Result<(), u32>) {
    match r {
        Ok(()) => c.ret_u32(0),
        Err(e) => fail(c, st, e),
    }
}

fn open(c: &mut Cpu, st: &Shared) {
    args!(c; p, flags, _mode);
    let Some(path) = path_arg(c, p) else { return };
    let r = lock(st).vfs.open(&path, flags);
    match r {
        Ok(fd) => c.ret_u32(fd as u32),
        Err(e) => fail(c, st, e),
    }
}

fn creat(c: &mut Cpu, st: &Shared) {
    args!(c; p, _mode);
    let Some(path) = path_arg(c, p) else { return };
    let r = lock(st).vfs.open(&path, vfs::O_CREAT | vfs::O_WRONLY | vfs::O_TRUNC);
    match r {
        Ok(fd) => c.ret_u32(fd as u32),
        Err(e) => fail(c, st, e),
    }
}

fn close(c: &mut Cpu, st: &Shared) {
    let fd = c.args().u32(c);
    if is_std(fd) {
        return c.ret_u32(0);
    }
    let r = lock(st).vfs.close(fd as i32);
    done(c, st, r);
}

fn read(c: &mut Cpu, st: &Shared) {
    args!(c; fd, buf, n);
    if fd == 0 {
        return c.ret_u32(0); // stdin is always at end of file
    }
    if is_std(fd) {
        return fail(c, st, errno::EBADF);
    }
    let r = lock(st).vfs.read(fd as i32, n as usize);
    match r {
        Ok(data) => {
            if wr(c, buf, &data) {
                c.ret_u32(data.len() as u32);
            }
        }
        Err(e) => fail(c, st, e),
    }
}

/// Write `data` to descriptor `fd`; the byte count or an `errno`.
pub(crate) fn write_fd(st: &Shared, fd: u32, data: &[u8]) -> Result<usize, u32> {
    let mut s = lock(st);
    match fd {
        1 => {
            let _ = s.stdout.write_all(data);
            Ok(data.len())
        }
        2 => {
            let _ = s.stderr.write_all(data);
            Ok(data.len())
        }
        0 => Err(errno::EBADF),
        _ => s.vfs.write(fd as i32, data),
    }
}

fn write(c: &mut Cpu, st: &Shared) {
    args!(c; fd, buf, n);
    let Some(data) = rd(c, buf, n) else { return };
    match write_fd(st, fd, &data) {
        Ok(k) => c.ret_u32(k as u32),
        Err(e) => fail(c, st, e),
    }
}

fn writev(c: &mut Cpu, st: &Shared) {
    args!(c; fd, iov, cnt);
    let mut total = 0u32;
    for i in 0..cnt {
        let (Some(base), Some(len)) = (rd32(c, iov + 8 * i), rd32(c, iov + 8 * i + 4)) else { return };
        let Some(data) = rd(c, base, len) else { return };
        match write_fd(st, fd, &data) {
            Ok(k) => total = total.wrapping_add(k as u32),
            Err(e) => return fail(c, st, e),
        }
    }
    c.ret_u32(total);
}

fn lseek(c: &mut Cpu, st: &Shared) {
    args!(c; fd, off, whence);
    if is_std(fd) {
        return fail(c, st, errno::ESPIPE);
    }
    let r = lock(st).vfs.lseek(fd as i32, i64::from(off as i32), whence);
    match r {
        Ok(p) => c.ret_u32(p as u32),
        Err(e) => fail(c, st, e),
    }
}

/// Size of the ARM bionic `struct stat`.
const STAT_SIZE: u32 = 104;

/// Encode `s` as the ARM bionic `struct stat`.
fn stat_bytes(s: &vfs::Stat, uid: u32) -> Vec<u8> {
    let mut b = vec![0u8; STAT_SIZE as usize];
    let mut put32 = |off: usize, v: u32| b[off..off + 4].copy_from_slice(&v.to_le_bytes());
    put32(0, 1); // st_dev
    put32(12, s.ino as u32); // __st_ino
    put32(16, s.mode);
    put32(20, s.nlink);
    put32(24, uid);
    put32(28, uid);
    put32(48, s.size as u32);
    put32(52, (s.size >> 32) as u32);
    put32(56, PAGE_SIZE); // st_blksize
    let blocks = s.size.div_ceil(512);
    put32(64, blocks as u32);
    put32(68, (blocks >> 32) as u32);
    for off in [72, 80, 88] {
        put32(off, s.mtime as u32);
    }
    put32(96, s.ino as u32);
    b
}

fn put_stat(c: &mut Cpu, st: &Shared, buf: u32, s: &vfs::Stat) {
    let uid = lock(st).uid;
    if wr(c, buf, &stat_bytes(s, uid)) {
        c.ret_u32(0);
    }
}

/// The `struct stat` of the standard streams: a character device.
fn std_stat() -> vfs::Stat {
    vfs::Stat { mode: vfs::S_IFCHR | 0o620, size: 0, mtime: 0, ino: 1, nlink: 1 }
}

fn fstat(c: &mut Cpu, st: &Shared) {
    args!(c; fd, buf);
    let r = if is_std(fd) { Ok(std_stat()) } else { lock(st).vfs.fstat(fd as i32) };
    match r {
        Ok(s) => put_stat(c, st, buf, &s),
        Err(e) => fail(c, st, e),
    }
}

/// `stat` and `lstat` (there are no symlinks).
fn stat(c: &mut Cpu, st: &Shared) {
    args!(c; p, buf);
    let Some(path) = path_arg(c, p) else { return };
    let r = lock(st).vfs.stat(&path);
    match r {
        Ok(s) => put_stat(c, st, buf, &s),
        Err(e) => fail(c, st, e),
    }
}

fn access(c: &mut Cpu, st: &Shared) {
    args!(c; p, mode);
    let Some(path) = path_arg(c, p) else { return };
    let r = lock(st).vfs.access(&path, mode);
    done(c, st, r);
}

fn unlink(c: &mut Cpu, st: &Shared) {
    let p = c.args().u32(c);
    let Some(path) = path_arg(c, p) else { return };
    let r = lock(st).vfs.unlink(&path);
    done(c, st, r);
}

fn rmdir(c: &mut Cpu, st: &Shared) {
    let p = c.args().u32(c);
    let Some(path) = path_arg(c, p) else { return };
    let r = lock(st).vfs.rmdir(&path);
    done(c, st, r);
}

/// `remove(path)`: a file or an empty directory.
fn remove(c: &mut Cpu, st: &Shared) {
    let p = c.args().u32(c);
    let Some(path) = path_arg(c, p) else { return };
    let mut s = lock(st);
    let r = match s.vfs.unlink(&path) {
        Err(vfs::EISDIR) => s.vfs.rmdir(&path),
        other => other,
    };
    drop(s);
    done(c, st, r);
}

fn mkdir(c: &mut Cpu, st: &Shared) {
    args!(c; p, _mode);
    let Some(path) = path_arg(c, p) else { return };
    let r = lock(st).vfs.mkdir(&path);
    done(c, st, r);
}

fn rename(c: &mut Cpu, st: &Shared) {
    args!(c; a, b);
    let (Some(from), Some(to)) = (path_arg(c, a), path_arg(c, b)) else { return };
    let r = lock(st).vfs.rename(&from, &to);
    done(c, st, r);
}

fn ftruncate(c: &mut Cpu, st: &Shared) {
    args!(c; fd, len);
    let r = lock(st).vfs.ftruncate(fd as i32, u64::from(len));
    done(c, st, r);
}

fn fsync(c: &mut Cpu, st: &Shared) {
    let fd = c.args().u32(c);
    if is_std(fd) {
        return c.ret_u32(0);
    }
    let r = lock(st).vfs.fsync(fd as i32);
    done(c, st, r);
}

fn readlink(c: &mut Cpu, st: &Shared) {
    args!(c; p, buf, n);
    let Some(path) = path_arg(c, p) else { return };
    let r = lock(st).vfs.readlink(&path);
    match r {
        Ok(target) => {
            let k = target.len().min(n as usize);
            if wr(c, buf, &target.as_bytes()[..k]) {
                c.ret_u32(k as u32);
            }
        }
        Err(e) => fail(c, st, e),
    }
}

// ---------- directories ----------

/// `struct dirent` (kernel `dirent64` layout): ino u64, off s64, reclen u16, type u8, name[256].
const DIRENT_SIZE: u32 = 280;
/// A `DIR` is `[fd][pad][dirent]`.
const DIR_BLOCK: u32 = 8 + DIRENT_SIZE;

fn opendir(c: &mut Cpu, st: &Shared) {
    let p = c.args().u32(c);
    let Some(path) = path_arg(c, p) else { return };
    let mut s = lock(st);
    let fd = match s.vfs.open(&path, vfs::O_DIRECTORY) {
        Ok(fd) => fd,
        Err(e) => {
            drop(s);
            set_errno(c, st, e);
            return c.ret_u32(0);
        }
    };
    let dir = s.heap.calloc(1, DIR_BLOCK);
    if dir == 0 {
        let _ = s.vfs.close(fd);
        drop(s);
        set_errno(c, st, errno::ENOMEM);
        return c.ret_u32(0);
    }
    s.dirs.insert(dir, fd);
    let mem = s.mem.clone();
    drop(s);
    // Libraries peek at the first word of a `DIR` for the descriptor (bionic's `int fd_` field).
    let _ = mem.write_u32(dir, fd as u32);
    c.ret_u32(dir);
}

/// Next entry of `dir` written into the dirent at `at`: `Some(false)` at the end (or on a bad `DIR`),
/// `None` if the guest faulted writing it.
fn next_entry(c: &mut Cpu, st: &Shared, dir: u32, at: u32) -> Option<bool> {
    let mut s = lock(st);
    let Some(fd) = s.dirs.get(&dir).copied() else { return Some(false) };
    let Ok(Some(e)) = s.vfs.readdir(fd) else { return Some(false) };
    drop(s);
    let mut b = vec![0u8; DIRENT_SIZE as usize];
    b[0..8].copy_from_slice(&e.ino.to_le_bytes());
    b[16..18].copy_from_slice(&(DIRENT_SIZE as u16).to_le_bytes());
    b[18] = if e.is_dir { 4 } else { 8 }; // DT_DIR / DT_REG
    let name = e.name.as_bytes();
    let k = name.len().min(255);
    b[19..19 + k].copy_from_slice(&name[..k]);
    wr(c, at, &b).then_some(true)
}

fn readdir(c: &mut Cpu, st: &Shared) {
    let dir = c.args().u32(c);
    // (A fault while writing the entry cannot happen: the dirent lives in the mapped DIR block.)
    let found = next_entry(c, st, dir, dir + 8) == Some(true);
    c.ret_u32(if found { dir + 8 } else { 0 });
}

fn readdir_r(c: &mut Cpu, st: &Shared) {
    args!(c; dir, entry, result);
    let Some(found) = next_entry(c, st, dir, entry) else { return };
    if wr32(c, result, if found { entry } else { 0 }) {
        c.ret_u32(0);
    }
}

fn closedir(c: &mut Cpu, st: &Shared) {
    let dir = c.args().u32(c);
    let mut s = lock(st);
    let Some(fd) = s.dirs.remove(&dir) else {
        drop(s);
        return fail(c, st, errno::EBADF);
    };
    let _ = s.vfs.close(fd);
    s.heap.free(dir);
    drop(s);
    c.ret_u32(0);
}

fn dirfd(c: &mut Cpu, st: &Shared) {
    let dir = c.args().u32(c);
    let fd = lock(st).dirs.get(&dir).copied();
    match fd {
        Some(fd) => c.ret_u32(fd as u32),
        None => fail(c, st, errno::EBADF),
    }
}

fn fcntl(c: &mut Cpu, st: &Shared) {
    let fd = c.args().u32(c);
    if is_std(fd) || lock(st).vfs.is_open(fd as i32) {
        c.ret_u32(0);
    } else {
        fail(c, st, errno::EBADF);
    }
}

fn isatty(c: &mut Cpu, st: &Shared) {
    set_errno(c, st, errno::ENOTTY);
    c.ret_u32(0);
}

fn ioctl(c: &mut Cpu, st: &Shared) {
    fail(c, st, errno::ENOTTY);
}

fn select(c: &mut Cpu, _: &Shared) {
    c.ret_u32(0); // nothing is ever ready
}

fn umask(c: &mut Cpu, _: &Shared) {
    c.ret_u32(0o022);
}

fn gethostname(c: &mut Cpu, _: &Shared) {
    args!(c; buf, len);
    if len < 10 {
        return c.ret_u32(-1i32 as u32);
    }
    if wr_cstr(c, buf, b"localhost") {
        c.ret_u32(0);
    }
}

fn uname(c: &mut Cpu, _: &Shared) {
    let buf = c.args().u32(c);
    let mut b = vec![0u8; 6 * 65];
    for (i, f) in ["Linux", "localhost", "3.4.0", "#1 SMP", "armv7l", "(none)"].iter().enumerate() {
        b[i * 65..i * 65 + f.len()].copy_from_slice(f.as_bytes());
    }
    if wr(c, buf, &b) {
        c.ret_u32(0);
    }
}

/// `getrlimit(res, struct rlimit *)`: everything is unlimited.
fn getrlimit(c: &mut Cpu, _: &Shared) {
    args!(c; _res, out);
    if wr(c, out, &[0xFF; 8]) {
        c.ret_u32(0);
    }
}

/// `getrusage(who, struct rusage *)`: all zero (72 bytes on ARM).
fn getrusage(c: &mut Cpu, _: &Shared) {
    args!(c; _who, out);
    if wr(c, out, &[0; 72]) {
        c.ret_u32(0);
    }
}

// ---------- memory mappings ----------

const MAP_SHARED: u32 = 0x01;
const MAP_ANONYMOUS: u32 = 0x20;
const PROT_WRITE: u32 = 0x2;
const MAP_FAILED: u32 = u32::MAX;

fn mmap(c: &mut Cpu, st: &Shared) {
    args!(c; _addr, len, prot, flags, fd, off);
    let len = len.div_ceil(PAGE_SIZE).max(1) * PAGE_SIZE;
    let anonymous = flags & MAP_ANONYMOUS != 0;
    // File mappings are private copies; a writable shared mapping could not be written back.
    if !anonymous && (flags & MAP_SHARED != 0 && prot & PROT_WRITE != 0) {
        set_errno(c, st, errno::ENODEV);
        return c.ret_u32(MAP_FAILED);
    }
    let mut s = lock(st);
    let content = if anonymous { Ok(Vec::new()) } else { s.vfs.read_range(fd as i32, u64::from(off), len as usize) };
    let content = match content {
        Ok(d) => d,
        Err(e) => {
            drop(s);
            set_errno(c, st, e);
            return c.ret_u32(MAP_FAILED);
        }
    };
    let p = s.heap.memalign(PAGE_SIZE, len);
    if p == 0 {
        drop(s);
        set_errno(c, st, errno::ENOMEM);
        return c.ret_u32(MAP_FAILED);
    }
    // Heap blocks are recycled: clear, then copy the file contents.
    let mut image = content;
    image.resize(len as usize, 0);
    let ok = s.mem.write_bytes(p, &image).is_ok();
    if ok {
        s.mmaps.insert(p, crate::state::Mapping { len, live: vec![true; (len / PAGE_SIZE) as usize] });
    }
    drop(s);
    c.ret_u32(if ok { p } else { MAP_FAILED });
}

/// `munmap`: pages of an anonymous mapping are released one range at a time (allocators such as
/// jemalloc trim a mapping to alignment); the heap block goes back once every page is gone.
fn munmap(c: &mut Cpu, st: &Shared) {
    args!(c; addr, len);
    let mut s = lock(st);
    let found = s.mmaps.iter().find(|(base, m)| addr >= **base && addr - **base < m.len).map(|(b, _)| *b);
    if let Some(base) = found {
        let m = s.mmaps.get_mut(&base).expect("found above");
        let first = ((addr - base) / PAGE_SIZE) as usize;
        let last = (((addr - base) as u64 + u64::from(len)).div_ceil(u64::from(PAGE_SIZE)) as usize).min(m.live.len());
        for p in &mut m.live[first..last] {
            *p = false;
        }
        if !m.live.iter().any(|p| *p) {
            s.mmaps.remove(&base);
            s.heap.free(base);
        }
    }
    drop(s);
    c.ret_u32(0);
}

// ---------- setjmp / longjmp ----------

/// Words of a guest `jmp_buf` used here: r4-r11, sp, lr, d8-d15 (as 64-bit halves).
const JB_R4: u32 = 0;
const JB_SP: u32 = 8 * 4;
const JB_LR: u32 = 9 * 4;
const JB_D8: u32 = 12 * 4;

fn setjmp(c: &mut Cpu, _: &Shared) {
    let buf = c.args().u32(c);
    let mut b = Vec::with_capacity(28 * 4);
    for r in 4..12 {
        b.extend_from_slice(&c.reg(r).to_le_bytes());
    }
    b.extend_from_slice(&c.sp().to_le_bytes());
    // The thunk stub copied the caller's return address into r12.
    b.extend_from_slice(&c.reg(12).to_le_bytes());
    b.extend_from_slice(&[0; 8]);
    for d in 8..16 {
        b.extend_from_slice(&c.d_bits(d).to_le_bytes());
    }
    if wr(c, buf, &b) {
        c.ret_u32(0);
    }
}

fn longjmp(c: &mut Cpu, _: &Shared) {
    args!(c; buf, val);
    let Some(b) = rd(c, buf, 28 * 4) else { return };
    let w = |off: u32| u32::from_le_bytes(b[off as usize..off as usize + 4].try_into().expect("4 bytes"));
    for (i, r) in (4..12).enumerate() {
        c.set_reg(r, w(JB_R4 + 4 * i as u32));
    }
    for (i, d) in (8..16).enumerate() {
        let lo = u64::from(w(JB_D8 + 8 * i as u32));
        let hi = u64::from(w(JB_D8 + 8 * i as u32 + 4));
        c.set_d_bits(d, hi << 32 | lo);
    }
    c.set_sp(w(JB_SP));
    c.ret_u32(if val == 0 { 1 } else { val });
    c.tail_call(w(JB_LR));
}

// ---------- qsort / bsearch ----------

/// Call the guest comparator on two guest pointers; `None` after a guest failure (the CPU is stopped).
fn compare(c: &mut Cpu, cmp: u32, a: u32, b: u32) -> Option<i32> {
    c.call_or_stop(cmp, &[a, b]).map(|r| r.r0 as i32)
}

fn qsort(c: &mut Cpu, st: &Shared) {
    args!(c; base, n, size, cmp);
    if n < 2 || size == 0 {
        return;
    }
    let Some(data) = rd(c, base, n * size) else { return };
    let mut elems: Vec<Vec<u8>> = data.chunks(size as usize).map(<[u8]>::to_vec).collect();
    // Two guest scratch slots so the comparator sees valid pointers.
    let scratch = lock(st).heap.alloc(2 * size);
    if scratch == 0 {
        return set_errno(c, st, errno::ENOMEM);
    }
    let (sa, sb) = (scratch, scratch + size);
    let mut stopped = false;
    let mut less_eq = |c: &mut Cpu, x: &[u8], y: &[u8]| -> bool {
        if stopped || !wr(c, sa, x) || !wr(c, sb, y) {
            stopped = true;
            return true;
        }
        match compare(c, cmp, sa, sb) {
            Some(r) => r <= 0,
            None => {
                stopped = true;
                true
            }
        }
    };
    // Stable bottom-up merge sort.
    let mut width = 1;
    while width < elems.len() {
        let mut merged = Vec::with_capacity(elems.len());
        for chunk in elems.chunks(2 * width) {
            let (l, r) = chunk.split_at(width.min(chunk.len()));
            let (mut i, mut j) = (0, 0);
            while i < l.len() && j < r.len() {
                if less_eq(c, &l[i], &r[j]) {
                    merged.push(l[i].clone());
                    i += 1;
                } else {
                    merged.push(r[j].clone());
                    j += 1;
                }
            }
            merged.extend_from_slice(&l[i..]);
            merged.extend_from_slice(&r[j..]);
        }
        elems = merged;
        width *= 2;
    }
    lock(st).heap.free(scratch);
    if stopped {
        return;
    }
    wr(c, base, &elems.concat());
}

fn bsearch(c: &mut Cpu, _: &Shared) {
    args!(c; key, base, n, size, cmp);
    let (mut lo, mut hi) = (0u32, n);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let elem = base.wrapping_add(mid.wrapping_mul(size));
        let Some(r) = compare(c, cmp, key, elem) else { return };
        match r.cmp(&0) {
            std::cmp::Ordering::Equal => return c.ret_u32(elem),
            std::cmp::Ordering::Less => hi = mid,
            std::cmp::Ordering::Greater => lo = mid + 1,
        }
    }
    c.ret_u32(0);
}

// ---------- dirname_r ----------

/// POSIX `dirname` of `path`.
pub(crate) fn dirname_of(path: &[u8]) -> Vec<u8> {
    let trimmed = match path.iter().rposition(|b| *b != b'/') {
        Some(i) => &path[..=i],
        None if path.is_empty() => return b".".to_vec(),
        None => return b"/".to_vec(),
    };
    let Some(slash) = trimmed.iter().rposition(|b| *b == b'/') else { return b".".to_vec() };
    match trimmed[..slash].iter().rposition(|b| *b != b'/') {
        Some(i) => trimmed[..=i].to_vec(),
        None => b"/".to_vec(),
    }
}

/// `dirname_r(path, buf, size)`: the length of the result plus its NUL; `buf` may be NULL to ask for it.
fn dirname_r(c: &mut Cpu, st: &Shared) {
    args!(c; p, buf, size);
    let path = if p == 0 {
        Vec::new()
    } else {
        let Some(v) = cstr(c, p) else { return };
        v
    };
    let out = dirname_of(&path);
    let need = out.len() as u32 + 1;
    if buf == 0 {
        return c.ret_u32(need);
    }
    if size < need {
        return fail(c, st, errno::ERANGE);
    }
    if wr_cstr(c, buf, &out) {
        c.ret_u32(need);
    }
}

// ---------- fnmatch ----------

const FNM_NOESCAPE: u32 = 0x01;
const FNM_PATHNAME: u32 = 0x02;
const FNM_PERIOD: u32 = 0x04;
const FNM_CASEFOLD: u32 = 0x10;

fn fold(b: u8, flags: u32) -> u8 {
    if flags & FNM_CASEFOLD != 0 {
        b.to_ascii_lowercase()
    } else {
        b
    }
}

/// Match a `[...]` class starting after the `[`; returns (matched, bytes consumed) or `None` if unterminated.
fn class(pat: &[u8], ch: u8, flags: u32) -> Option<(bool, usize)> {
    let mut i = 0;
    let negate = matches!(pat.first(), Some(b'!' | b'^'));
    if negate {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    loop {
        let &p = pat.get(i)?;
        if p == b']' && !first {
            return Some((matched != negate, i + 1));
        }
        first = false;
        let lo = if p == b'\\' && flags & FNM_NOESCAPE == 0 {
            i += 1;
            *pat.get(i)?
        } else {
            p
        };
        i += 1;
        if pat.get(i) == Some(&b'-') && pat.get(i + 1).is_some_and(|x| *x != b']') {
            let hi = pat[i + 1];
            i += 2;
            matched |= (fold(lo, flags)..=fold(hi, flags)).contains(&fold(ch, flags));
        } else {
            matched |= fold(lo, flags) == fold(ch, flags);
        }
    }
}

fn fnmatch_bytes(pat: &[u8], s: &[u8], flags: u32, at_start: bool) -> bool {
    let leading_period = |s: &[u8], at_start: bool| flags & FNM_PERIOD != 0 && s.first() == Some(&b'.') && at_start;
    match pat.split_first() {
        None => s.is_empty(),
        Some((b'*', rest)) => {
            if leading_period(s, at_start) {
                return false;
            }
            let mut rest = rest;
            while rest.first() == Some(&b'*') {
                rest = &rest[1..];
            }
            (0..=s.len()).any(|k| {
                if flags & FNM_PATHNAME != 0 && s[..k].contains(&b'/') {
                    return false;
                }
                fnmatch_bytes(rest, &s[k..], flags, k == 0 && at_start)
            })
        }
        Some((b'?', rest)) => match s.split_first() {
            Some((&ch, tail)) => {
                !(leading_period(s, at_start) || flags & FNM_PATHNAME != 0 && ch == b'/')
                    && fnmatch_bytes(rest, tail, flags, false)
            }
            None => false,
        },
        Some((b'[', rest)) => match (s.split_first(), class(rest, s.first().copied().unwrap_or(0), flags)) {
            (Some((&ch, tail)), Some((hit, used))) => {
                hit && !(leading_period(s, at_start) || flags & FNM_PATHNAME != 0 && ch == b'/')
                    && fnmatch_bytes(&rest[used..], tail, flags, false)
            }
            (Some((&ch, tail)), None) => ch == b'[' && fnmatch_bytes(rest, tail, flags, false),
            (None, _) => false,
        },
        Some((&p, rest)) => {
            let (lit, rest) = if p == b'\\' && flags & FNM_NOESCAPE == 0 && !rest.is_empty() {
                (rest[0], &rest[1..])
            } else {
                (p, rest)
            };
            match s.split_first() {
                Some((&ch, tail)) => {
                    fold(ch, flags) == fold(lit, flags)
                        && fnmatch_bytes(rest, tail, flags, flags & FNM_PATHNAME != 0 && ch == b'/')
                }
                None => false,
            }
        }
    }
}

fn fnmatch(c: &mut Cpu, _: &Shared) {
    args!(c; pat, s, flags);
    let (Some(pat), Some(s)) = (cstr(c, pat), cstr(c, s)) else { return };
    c.ret_u32(u32::from(!fnmatch_bytes(&pat, &s, flags, true)));
}

pub(crate) const TABLE: &[(&str, Impl)] = &[
    ("close", close),
    ("read", read),
    ("write", write),
    ("writev", writev),
    ("lseek", lseek),
    ("fstat", fstat),
    ("open", open),
    ("creat", creat),
    ("stat", stat),
    ("lstat", stat),
    ("access", access),
    ("unlink", unlink),
    ("rmdir", rmdir),
    ("remove", remove),
    ("mkdir", mkdir),
    ("rename", rename),
    ("ftruncate", ftruncate),
    ("fsync", fsync),
    ("readlink", readlink),
    ("opendir", opendir),
    ("readdir", readdir),
    ("closedir", closedir),
    ("dirfd", dirfd),
    ("isatty", isatty),
    ("ioctl", ioctl),
    ("fcntl", fcntl),
    ("select", select),
    ("pselect", select),
    ("poll", select),
    ("umask", umask),
    ("gethostname", gethostname),
    ("uname", uname),
    ("getrlimit", getrlimit),
    ("getrusage", getrusage),
    ("mmap", mmap),
    ("munmap", munmap),
    ("madvise", ok),
    ("msync", ok),
    ("setjmp", setjmp),
    ("_setjmp", setjmp),
    ("sigsetjmp", setjmp),
    ("longjmp", longjmp),
    ("_longjmp", longjmp),
    ("siglongjmp", longjmp),
    ("qsort", qsort),
    ("bsearch", bsearch),
    ("fnmatch", fnmatch),
    ("dirname_r", dirname_r),
    ("readdir_r", readdir_r),
    ("getaddrinfo", eai_noname),
    ("getnameinfo", eai_noname),
    ("sigaction", ok),
    ("sigprocmask", ok),
    ("getitimer", ok),
    ("setitimer", ok),
    ("prctl", ok),
    ("setpriority", ok),
    ("setuid", ok),
    ("initgroups", ok),
    ("setrlimit", ok),
    ("kill", ok),
    ("raise", ok),
    ("bsd_signal", ok),
    ("if_nametoindex", ok),
    ("vsyslog", ok),
    ("openlog", ok),
    ("closelog", ok),
];

/// Every table of this module (fixed-errno failures, NULL returns, real functions).
pub(crate) fn tables() -> [&'static [(&'static str, Impl)]; 3] {
    [FAIL_TABLE, NULL_TABLE, TABLE]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests_env::{Env, SCRATCH};
    use bhpc_cpu::Stop;

    const A: u32 = SCRATCH;
    const B: u32 = SCRATCH + 0x400;
    const OUT: u32 = SCRATCH + 0x800;
    const BIG: u32 = SCRATCH + 0x2000;
    const BAD: u32 = 0x10;
    const NEG1: u32 = u32::MAX;

    struct Tmp(std::path::PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// An environment with a writable `/data/data/pkg` mounted from a scratch directory.
    fn fs_env() -> (Env, Tmp) {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir =
            std::env::temp_dir().join(format!("bhpc-sys-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(dir.join("rw")).unwrap();
        let host = dir.join("rw");
        let e = Env::with(move |cfg| {
            cfg.fs = crate::vfs::test_fs(host);
        });
        (e, Tmp(dir))
    }

    fn put(e: &Env, at: u32, s: &str) -> u32 {
        e.put_str(at, s);
        at
    }

    fn call(e: &mut Env, f: &str, args: &[u32]) -> u32 {
        e.call(f, args).r0
    }

    #[test]
    fn failing_and_null_returning_stubs_report_their_errno() {
        let mut e = Env::new();
        for (name, _) in FAIL_TABLE {
            e.set_errno(0);
            assert_eq!(call(&mut e, name, &[0; 4]), NEG1, "{name}");
            assert_ne!(e.errno(), 0, "{name}");
        }
        assert_eq!(call(&mut e, "socket", &[2, 1, 0]), NEG1);
        assert_eq!(e.errno(), errno::EAFNOSUPPORT);
        assert_eq!(call(&mut e, "fork", &[]), NEG1);
        assert_eq!(e.errno(), errno::EAGAIN);
        for (name, _) in NULL_TABLE {
            assert_eq!(call(&mut e, name, &[0; 4]), 0, "{name}");
        }
        e.set_errno(0);
        call(&mut e, "if_indextoname", &[1, A]);
        assert_eq!(e.errno(), errno::ENOSYS);
        e.set_errno(0);
        call(&mut e, "fgetln", &[0, 0]);
        assert_eq!(e.errno(), 0, "NULL without errno");
        for name in [
            "sigaction",
            "sigprocmask",
            "getitimer",
            "setitimer",
            "prctl",
            "setpriority",
            "setuid",
            "initgroups",
            "setrlimit",
            "kill",
            "raise",
            "bsd_signal",
            "if_nametoindex",
            "vsyslog",
            "openlog",
            "closelog",
            "madvise",
            "msync",
        ] {
            assert_eq!(call(&mut e, name, &[0; 4]), 0, "{name}");
        }
        assert_eq!(call(&mut e, "getaddrinfo", &[0; 4]), 8);
        assert_eq!(call(&mut e, "getnameinfo", &[0; 4]), 8);
    }

    #[test]
    fn standard_descriptors() {
        let mut e = Env::new();
        e.put_str(A, "to stdout");
        assert_eq!(call(&mut e, "write", &[1, A, 9]), 9);
        assert_eq!(call(&mut e, "write", &[2, A, 3]), 3);
        assert_eq!(e.out.text(), "to stdout");
        assert_eq!(e.err.text(), "to ");
        assert_eq!(call(&mut e, "write", &[0, A, 1]), NEG1);
        assert_eq!(e.errno(), errno::EBADF);
        assert_eq!(call(&mut e, "write", &[77, A, 1]), NEG1);
        assert!(matches!(e.try_call("write", &[1, BAD, 4]), Err(Stop::Fault(_))));
        // writev
        let mem = e.mem();
        mem.write_u32(OUT, A).unwrap();
        mem.write_u32(OUT + 4, 2).unwrap();
        mem.write_u32(OUT + 8, A + 3).unwrap();
        mem.write_u32(OUT + 12, 3).unwrap();
        assert_eq!(call(&mut e, "writev", &[1, OUT, 2]), 5);
        assert_eq!(e.out.text(), "to stdouttostd");
        assert_eq!(call(&mut e, "writev", &[0, OUT, 2]), NEG1);
        assert!(matches!(e.try_call("writev", &[1, BAD, 1]), Err(Stop::Fault(_))));
        mem.write_u32(OUT + 16, BAD).unwrap();
        mem.write_u32(OUT + 20, 4).unwrap();
        assert!(matches!(e.try_call("writev", &[1, OUT + 16, 1]), Err(Stop::Fault(_))));
        // close, read, lseek, fstat, fcntl, fsync
        assert_eq!(call(&mut e, "close", &[1]), 0);
        assert_eq!(call(&mut e, "close", &[9]), NEG1);
        assert_eq!(call(&mut e, "read", &[0, A, 4]), 0);
        assert_eq!(call(&mut e, "read", &[1, A, 4]), NEG1);
        assert_eq!(call(&mut e, "read", &[9, A, 4]), NEG1);
        assert_eq!(call(&mut e, "lseek", &[1, 0, 0]), NEG1);
        assert_eq!(e.errno(), errno::ESPIPE);
        assert_eq!(call(&mut e, "lseek", &[9, 0, 0]), NEG1);
        assert_eq!(e.errno(), errno::EBADF);
        assert_eq!(call(&mut e, "fstat", &[1, OUT]), 0);
        assert_eq!(mem.read_u32(OUT + 16).unwrap(), vfs::S_IFCHR | 0o620);
        assert_eq!(mem.read_u32(OUT + 56).unwrap(), PAGE_SIZE);
        assert_eq!(call(&mut e, "fstat", &[9, OUT]), NEG1);
        assert!(matches!(e.try_call("fstat", &[1, BAD]), Err(Stop::Fault(_))));
        assert_eq!(call(&mut e, "fcntl", &[2, 3, 0]), 0);
        assert_eq!(call(&mut e, "fcntl", &[9, 3, 0]), NEG1);
        assert_eq!(call(&mut e, "fsync", &[1]), 0);
        assert_eq!(call(&mut e, "fsync", &[9]), NEG1);
        // terminal queries and process information
        e.set_errno(0);
        assert_eq!(call(&mut e, "isatty", &[1]), 0);
        assert_eq!(e.errno(), errno::ENOTTY);
        assert_eq!(call(&mut e, "ioctl", &[1, 0x5401, 0]), NEG1);
        for f in ["select", "pselect", "poll"] {
            assert_eq!(call(&mut e, f, &[0; 4]), 0, "{f}");
        }
        assert_eq!(call(&mut e, "umask", &[0]), 0o022);
        assert_eq!(call(&mut e, "gethostname", &[A, 64]), 0);
        assert_eq!(e.get_str(A), "localhost");
        assert_eq!(call(&mut e, "gethostname", &[A, 4]), NEG1);
        assert!(matches!(e.try_call("gethostname", &[BAD, 64]), Err(Stop::Fault(_))));
        assert_eq!(call(&mut e, "uname", &[B]), 0);
        assert_eq!(e.get_str(B), "Linux");
        assert_eq!(e.get_str(B + 4 * 65), "armv7l");
        assert!(matches!(e.try_call("uname", &[BAD]), Err(Stop::Fault(_))));
        assert_eq!(call(&mut e, "getrlimit", &[7, OUT]), 0);
        assert_eq!(mem.read_u32(OUT).unwrap(), u32::MAX);
        assert!(matches!(e.try_call("getrlimit", &[7, BAD]), Err(Stop::Fault(_))));
        assert_eq!(call(&mut e, "getrusage", &[0, OUT]), 0);
        assert!(matches!(e.try_call("getrusage", &[0, BAD]), Err(Stop::Fault(_))));
    }

    #[test]
    fn files_on_a_mounted_directory() {
        let (mut e, _tmp) = fs_env();
        let mem = e.mem();
        let path = put(&e, A, "/data/data/pkg/f.bin");
        e.put_str(B, "payload!");
        // creat + write + close, open + read + lseek + fstat + stat
        let fd = call(&mut e, "creat", &[path, 0o644]);
        assert!(fd >= 3);
        assert_eq!(call(&mut e, "write", &[fd, B, 8]), 8);
        assert_eq!(call(&mut e, "fsync", &[fd]), 0);
        assert_eq!(call(&mut e, "close", &[fd]), 0);
        assert_eq!(call(&mut e, "close", &[fd]), NEG1);
        let fd = call(&mut e, "open", &[path, 0, 0]);
        assert_eq!(call(&mut e, "read", &[fd, OUT, 100]), 8);
        assert_eq!(e.get_str(OUT), "payload!");
        assert_eq!(call(&mut e, "lseek", &[fd, 3, 0]), 3);
        assert_eq!(call(&mut e, "lseek", &[fd, NEG1, 1]), 2, "negative offsets are signed");
        assert_eq!(call(&mut e, "lseek", &[fd, NEG1, 0]), NEG1);
        assert_eq!(call(&mut e, "fstat", &[fd, OUT]), 0);
        assert_eq!(mem.read_u32(OUT + 48).unwrap(), 8, "st_size");
        assert_eq!(mem.read_u32(OUT + 16).unwrap() & 0o170000, vfs::S_IFREG);
        assert_eq!(mem.read_u32(OUT + 64).unwrap(), 1, "st_blocks");
        assert_eq!(call(&mut e, "fcntl", &[fd, 3, 0]), 0);
        assert!(matches!(e.try_call("read", &[fd, BAD, 4]), Err(Stop::Fault(_))));
        assert_eq!(call(&mut e, "ftruncate", &[fd, 2]), NEG1, "read-only descriptor");
        call(&mut e, "close", &[fd]);
        let rw = call(&mut e, "open", &[path, 2, 0]);
        assert_eq!(call(&mut e, "ftruncate", &[rw, 4]), 0);
        call(&mut e, "close", &[rw]);
        for f in ["stat", "lstat"] {
            assert_eq!(call(&mut e, f, &[path, OUT]), 0, "{f}");
            assert_eq!(mem.read_u32(OUT + 48).unwrap(), 4);
        }
        let __p = put(&e, B, "/missing");
        assert_eq!(call(&mut e, "stat", &[__p, OUT]), NEG1);
        assert_eq!(e.errno(), errno::ENOENT);
        assert!(matches!(e.try_call("stat", &[BAD, OUT]), Err(Stop::Fault(_))));
        assert_eq!(call(&mut e, "access", &[path, 4]), 0);
        assert_eq!(call(&mut e, "access", &[B, 0]), NEG1);
        assert!(matches!(e.try_call("open", &[BAD, 0, 0]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("creat", &[BAD, 0]), Err(Stop::Fault(_))));
        let missing = put(&e, B, "/data/data/pkg/no/dir/x");
        assert_eq!(call(&mut e, "creat", &[missing, 0]), NEG1);
        assert_eq!(e.errno(), errno::ENOENT);
        // mkdir / rename / unlink / rmdir / remove
        let dir = put(&e, B, "/data/data/pkg/d");
        assert_eq!(call(&mut e, "mkdir", &[dir, 0o755]), 0);
        assert_eq!(call(&mut e, "mkdir", &[dir, 0o755]), NEG1);
        let moved = put(&e, B + 0x100, "/data/data/pkg/d/f.bin");
        assert_eq!(call(&mut e, "rename", &[path, moved]), 0);
        assert_eq!(call(&mut e, "rename", &[path, moved]), NEG1);
        assert_eq!(call(&mut e, "rmdir", &[dir]), NEG1, "not empty");
        assert_eq!(call(&mut e, "unlink", &[moved]), 0);
        assert_eq!(call(&mut e, "unlink", &[moved]), NEG1);
        assert_eq!(call(&mut e, "rmdir", &[dir]), 0);
        assert_eq!(call(&mut e, "mkdir", &[dir, 0]), 0);
        assert_eq!(call(&mut e, "remove", &[dir]), 0, "remove falls back to rmdir");
        assert_eq!(call(&mut e, "remove", &[dir]), NEG1);
        let file = put(&e, B, "/data/data/pkg/g");
        let fd = call(&mut e, "creat", &[file, 0]);
        call(&mut e, "close", &[fd]);
        assert_eq!(call(&mut e, "remove", &[file]), 0);
        for f in ["unlink", "rmdir", "remove", "readlink", "access", "mkdir"] {
            assert!(matches!(e.try_call(f, &[BAD, 0, 0]), Err(Stop::Fault(_))), "{f}");
        }
        assert!(matches!(e.try_call("rename", &[BAD, A]), Err(Stop::Fault(_))));
    }

    #[test]
    fn readlink_reports_the_apk_for_the_executable() {
        let mut e = Env::new();
        let p = put(&e, A, "/proc/self/exe");
        let n = call(&mut e, "readlink", &[p, OUT, 200]);
        assert_eq!(e.mem().read_bytes(OUT, n).unwrap(), b"/data/app/com.noodlecake.blockheads-1/base.apk");
        assert_eq!(call(&mut e, "readlink", &[p, OUT, 5]), 5, "truncated to the buffer");
        let __p = put(&e, B, "/nope");
        assert_eq!(call(&mut e, "readlink", &[__p, OUT, 5]), NEG1);
        assert!(matches!(e.try_call("readlink", &[p, BAD, 50]), Err(Stop::Fault(_))));
    }

    #[test]
    fn directories() {
        let (mut e, tmp) = fs_env();
        std::fs::write(tmp.0.join("rw/one"), b"1").unwrap();
        std::fs::create_dir(tmp.0.join("rw/two")).unwrap();
        let __p = put(&e, A, "/data/data/pkg");
        let dir = call(&mut e, "opendir", &[__p]);
        assert_ne!(dir, 0);
        assert_eq!(e.mem().read_u32(dir).unwrap(), call(&mut e, "dirfd", &[dir]), "the first word is the descriptor");
        let mut names = Vec::new();
        loop {
            let ent = call(&mut e, "readdir", &[dir]);
            if ent == 0 {
                break;
            }
            let ty = e.mem().read_bytes(ent + 18, 1).unwrap()[0];
            names.push((e.get_str(ent + 19), ty));
        }
        assert_eq!(names, [(".".into(), 4), ("..".into(), 4), ("one".into(), 8), ("two".into(), 4)]);
        assert_eq!(call(&mut e, "readdir", &[dir]), 0);
        assert_eq!(call(&mut e, "closedir", &[dir]), 0);
        assert_eq!(call(&mut e, "closedir", &[dir]), NEG1);
        assert_eq!(call(&mut e, "readdir", &[dir]), 0, "a closed DIR yields nothing");
        assert_eq!(call(&mut e, "dirfd", &[dir]), NEG1);
        // readdir_r
        let __p = put(&e, A, "/data/data/pkg");
        let dir = call(&mut e, "opendir", &[__p]);
        let entry = SCRATCH + 0x1000;
        let mut count = 0;
        loop {
            assert_eq!(call(&mut e, "readdir_r", &[dir, entry, OUT]), 0);
            if e.mem().read_u32(OUT).unwrap() == 0 {
                break;
            }
            assert_eq!(e.mem().read_u32(OUT).unwrap(), entry);
            count += 1;
        }
        assert_eq!(count, 4);
        assert!(matches!(e.try_call("readdir_r", &[dir, entry, BAD]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("readdir_r", &[dir, BAD, OUT]), Ok(_) | Err(Stop::Fault(_))));
        // Failures.
        e.set_errno(0);
        let __p = put(&e, B, "/nowhere");
        assert_eq!(call(&mut e, "opendir", &[__p]), 0);
        assert_eq!(e.errno(), errno::ENOENT);
        let __p = put(&e, B, "/proc/cpuinfo");
        assert_eq!(call(&mut e, "opendir", &[__p]), 0);
        assert!(matches!(e.try_call("opendir", &[BAD]), Err(Stop::Fault(_))));
        // A heap that cannot hold a DIR block.
        let mut s = Env::small();
        let mut hold = Vec::new();
        loop {
            let p = call(&mut s, "malloc", &[0x8000]);
            if p == 0 {
                break;
            }
            hold.push(p);
        }
        while call(&mut s, "malloc", &[16]) != 0 {}
        s.set_errno(0);
        let __p = put(&s, A, "/");
        assert_eq!(call(&mut s, "opendir", &[__p]), 0);
        assert_eq!(s.errno(), errno::ENOMEM);
    }

    #[test]
    fn mmap_anonymous_file_and_munmap() {
        let (mut e, tmp) = fs_env();
        let mem = e.mem();
        let p = call(&mut e, "mmap", &[0, 10_000, 3, 0x22, NEG1, 0]);
        assert_ne!(p, MAP_FAILED);
        assert_eq!(p % PAGE_SIZE, 0);
        mem.write_u32(p, 0xDEAD).unwrap();
        mem.write_u32(p + 8192, 0xBEEF).unwrap();
        // Partial unmaps keep the block until every page is gone.
        assert_eq!(call(&mut e, "munmap", &[p, 4096]), 0);
        assert_eq!(mem.read_u32(p + 8192).unwrap(), 0xBEEF);
        assert_eq!(call(&mut e, "munmap", &[p + 4096, 100_000]), 0);
        assert_eq!(call(&mut e, "munmap", &[p, 4096]), 0, "already gone");
        assert_eq!(call(&mut e, "munmap", &[0x1234_0000, 4096]), 0, "not ours");
        let q = call(&mut e, "mmap", &[0, 4096, 3, 0x22, NEG1, 0]);
        assert_ne!(q, MAP_FAILED);
        assert_eq!(mem.read_u32(q).unwrap(), 0, "anonymous memory is zeroed even when recycled");
        // File mapping: contents, offset, zero padding.
        std::fs::write(tmp.0.join("rw/m.bin"), (0..=255u8).cycle().take(6000).collect::<Vec<_>>()).unwrap();
        let __p = put(&e, A, "/data/data/pkg/m.bin");
        let fd = call(&mut e, "open", &[__p, 0, 0]);
        let m = call(&mut e, "mmap", &[0, 8192, 1, 2, fd, 0]);
        assert_ne!(m, MAP_FAILED);
        assert_eq!(mem.read_bytes(m + 255, 2).unwrap(), [255, 0]);
        assert_eq!(mem.read_u32(m + 7000).unwrap(), 0, "past the end of the file");
        let m2 = call(&mut e, "mmap", &[0, 4096, 1, 1, fd, 4096]);
        assert_eq!(mem.read_bytes(m2, 1).unwrap(), [(4096 % 256) as u8]);
        // Not mappable: writable shared mapping, bad descriptor.
        e.set_errno(0);
        assert_eq!(call(&mut e, "mmap", &[0, 4096, 3, 1, fd, 0]), MAP_FAILED);
        assert_eq!(e.errno(), errno::ENODEV);
        assert_eq!(call(&mut e, "mmap", &[0, 4096, 1, 2, 99, 0]), MAP_FAILED);
        assert_eq!(e.errno(), errno::EBADF);
        // No memory left.
        let mut s = Env::small();
        assert_eq!(call(&mut s, "mmap", &[0, 1 << 30, 3, 0x22, NEG1, 0]), MAP_FAILED);
        assert_eq!(s.errno(), errno::ENOMEM);
    }

    /// `fn(buf) -> int`: `r = setjmp(buf); if r == 0 { longjmp(buf, VAL) } return r`, built by hand.
    fn setjmp_program(e: &mut Env, at: u32, val_insn: u32) {
        let setjmp = e.addr("setjmp");
        let longjmp = e.addr("longjmp");
        let words = [
            0xE92D_4010u32, // push {r4, lr}
            0xE1A0_4000,    // mov r4, r0
            0xE59F_C01C,    // ldr r12, [pc, #28]  -> setjmp
            0xE12F_FF3C,    // blx r12
            0xE350_0000,    // cmp r0, #0
            0x1A00_0003,    // bne done
            0xE1A0_0004,    // mov r0, r4
            val_insn,       // mov r1, #val
            0xE59F_C008,    // ldr r12, [pc, #8]   -> longjmp
            0xE12F_FF3C,    // blx r12
            0xE8BD_8010,    // done: pop {r4, pc}
            setjmp,
            longjmp,
        ];
        for (i, w) in words.iter().enumerate() {
            e.mem().write_u32(at + 4 * i as u32, *w).unwrap();
        }
    }

    #[test]
    fn setjmp_returns_twice_and_longjmp_restores_the_frame() {
        let mut e = Env::new();
        setjmp_program(&mut e, SCRATCH + 0x3000, 0xE3A0_1007); // mov r1, #7
                                                               // r5 is callee-saved: it must survive the non-local jump.
        e.cpu.set_reg(5, 0x1234);
        assert_eq!(e.cpu.call(SCRATCH + 0x3000, &[OUT]).unwrap().r0, 7);
        assert_eq!(e.cpu.reg(5), 0x1234);
        // longjmp(buf, 0) returns 1 from setjmp.
        setjmp_program(&mut e, SCRATCH + 0x3100, 0xE3A0_1000); // mov r1, #0
        assert_eq!(e.cpu.call(SCRATCH + 0x3100, &[OUT + 0x100]).unwrap().r0, 1);
        // The saved context holds sp and lr.
        assert_ne!(e.mem().read_u32(OUT + JB_SP).unwrap(), 0);
        // Faults in the buffer are reported.
        for f in ["setjmp", "_setjmp", "sigsetjmp"] {
            assert!(matches!(e.try_call(f, &[BAD]), Err(Stop::Fault(_))), "{f}");
        }
        for f in ["longjmp", "_longjmp", "siglongjmp"] {
            assert!(matches!(e.try_call(f, &[BAD, 1]), Err(Stop::Fault(_))), "{f}");
        }
    }

    /// A comparator thunk: compares the `i32` each pointer refers to.
    fn comparator(e: &mut Env, name: &str) -> u32 {
        e.cpu
            .register_shared_thunk(
                name,
                std::sync::Arc::new(|c: &mut Cpu| {
                    let mut a = c.args();
                    let (x, y) = (a.u32(c), a.u32(c));
                    let (x, y) = (c.mem().read_u32(x).unwrap() as i32, c.mem().read_u32(y).unwrap() as i32);
                    c.ret_u32(x.cmp(&y) as i32 as u32);
                }),
            )
            .unwrap()
    }

    #[test]
    fn qsort_sorts_stably_with_a_guest_comparator() {
        let mut e = Env::new();
        let cmp = comparator(&mut e, "cmp_i32");
        let mem = e.mem();
        let data = [5i32, -1, 9, 3, 3, 0, 7, 2, 8, -4, 6];
        for (i, v) in data.iter().enumerate() {
            mem.write_u32(BIG + 4 * i as u32, *v as u32).unwrap();
        }
        call(&mut e, "qsort", &[BIG, data.len() as u32, 4, cmp]);
        let got: Vec<i32> = (0..data.len()).map(|i| mem.read_u32(BIG + 4 * i as u32).unwrap() as i32).collect();
        let mut want = data.to_vec();
        want.sort();
        assert_eq!(got, want);
        // Trivial inputs are left alone.
        call(&mut e, "qsort", &[BIG, 1, 4, cmp]);
        call(&mut e, "qsort", &[BIG, 0, 4, cmp]);
        call(&mut e, "qsort", &[BIG, 5, 0, cmp]);
        // Equal keys keep their order (records of {key, tag}).
        let rec = e
            .cpu
            .register_shared_thunk(
                "cmp_key",
                std::sync::Arc::new(|c: &mut Cpu| {
                    let mut a = c.args();
                    let (x, y) = (a.u32(c), a.u32(c));
                    let (x, y) = (c.mem().read_u32(x).unwrap(), c.mem().read_u32(y).unwrap());
                    c.ret_u32((x as i32 - y as i32) as u32);
                }),
            )
            .unwrap();
        for (i, (k, t)) in [(2, 0), (1, 1), (2, 2), (1, 3), (2, 4)].iter().enumerate() {
            mem.write_u32(BIG + 8 * i as u32, *k).unwrap();
            mem.write_u32(BIG + 8 * i as u32 + 4, *t).unwrap();
        }
        call(&mut e, "qsort", &[BIG, 5, 8, rec]);
        let tags: Vec<u32> = (0..5).map(|i| mem.read_u32(BIG + 8 * i + 4).unwrap()).collect();
        assert_eq!(tags, [1, 3, 0, 2, 4]);
        // Unreadable array, failing comparator, no memory for the scratch slots.
        assert!(matches!(e.try_call("qsort", &[BAD, 2, 4, cmp]), Err(Stop::Fault(_))));
        let boom = e
            .cpu
            .register_shared_thunk("boom", std::sync::Arc::new(|c: &mut Cpu| c.request_stop(Stop::Host("boom".into()))))
            .unwrap();
        assert_eq!(e.try_call("qsort", &[BIG, 4, 4, boom]).unwrap_err(), Stop::Host("boom".into()));
        let mut s = Env::small();
        let scmp = comparator(&mut s, "cmp_i32");
        while call(&mut s, "malloc", &[16]) != 0 {}
        s.mem().write_u32(SCRATCH, 1).unwrap();
        s.set_errno(0);
        call(&mut s, "qsort", &[SCRATCH, 2, 4, scmp]);
        assert_eq!(s.errno(), errno::ENOMEM);
    }

    #[test]
    fn bsearch_finds_elements_through_the_comparator() {
        let mut e = Env::new();
        let cmp = comparator(&mut e, "cmp_i32");
        let mem = e.mem();
        for i in 0..10u32 {
            mem.write_u32(BIG + 4 * i, i * 10).unwrap();
        }
        let key = BIG + 0x100;
        for (k, want) in [(0u32, Some(0)), (50, Some(5)), (90, Some(9)), (45, None), (95, None), (NEG1, None)] {
            mem.write_u32(key, k).unwrap();
            let r = call(&mut e, "bsearch", &[key, BIG, 10, 4, cmp]);
            assert_eq!(r, want.map_or(0, |i| BIG + 4 * i), "{k}");
        }
        assert_eq!(call(&mut e, "bsearch", &[key, BIG, 0, 4, cmp]), 0);
        let boom = e
            .cpu
            .register_shared_thunk("boom", std::sync::Arc::new(|c: &mut Cpu| c.request_stop(Stop::Host("boom".into()))))
            .unwrap();
        assert!(e.try_call("bsearch", &[key, BIG, 10, 4, boom]).is_err());
    }

    #[test]
    fn dirname_follows_posix() {
        for (input, want) in [
            ("/usr/lib", "/usr"),
            ("/usr/", "/"),
            ("usr", "."),
            ("/", "/"),
            ("//", "/"),
            ("", "."),
            ("a/b//", "a"),
            ("/a//b", "/a"),
            ("a/b", "a"),
            ("///a", "/"),
        ] {
            assert_eq!(dirname_of(input.as_bytes()), want.as_bytes(), "{input:?}");
        }
        let mut e = Env::new();
        let p = put(&e, A, "/usr/lib");
        assert_eq!(call(&mut e, "dirname_r", &[p, OUT, 16]), 5, "length including the NUL");
        assert_eq!(e.get_str(OUT), "/usr");
        assert_eq!(call(&mut e, "dirname_r", &[p, 0, 0]), 5, "NULL buffer asks for the size");
        assert_eq!(call(&mut e, "dirname_r", &[0, OUT, 16]), 2, "NULL path is dot");
        assert_eq!(call(&mut e, "dirname_r", &[p, OUT, 3]), NEG1);
        assert_eq!(e.errno(), errno::ERANGE);
        assert!(matches!(e.try_call("dirname_r", &[BAD, OUT, 16]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("dirname_r", &[p, BAD, 16]), Err(Stop::Fault(_))));
    }

    #[test]
    fn fnmatch_patterns() {
        let mut e = Env::new();
        let mut m = |pat: &str, s: &str, flags: u32| -> u32 {
            let (p, q) = (put(&e, A, pat), put(&e, B, s));
            call(&mut e, "fnmatch", &[p, q, flags])
        };
        const NOESCAPE: u32 = 1;
        const PATHNAME: u32 = 2;
        const PERIOD: u32 = 4;
        const CASEFOLD: u32 = 0x10;
        for (pat, s, flags, want) in [
            ("*", "anything", 0, 0),
            ("*", "", 0, 0),
            ("a*c", "abbbc", 0, 0),
            ("a*c", "ab", 0, 1),
            ("a?c", "abc", 0, 0),
            ("a?c", "ac", 0, 1),
            ("?", "", 0, 1),
            ("[abc]x", "bx", 0, 0),
            ("[!abc]x", "dx", 0, 0),
            ("[^abc]x", "ax", 0, 1),
            ("[a-c]x", "bx", 0, 0),
            ("[a-c]x", "dx", 0, 1),
            ("[]a]x", "]x", 0, 0),
            ("[a-]x", "-x", 0, 0),
            ("[\\]]x", "]x", 0, 0),
            ("[\\]]x", "\\x", 0, 1),
            ("[a", "[a", 0, 0),
            ("[a", "x", 0, 1),
            ("[ab]", "", 0, 1),
            ("\\*", "*", 0, 0),
            ("\\*", "\\*", NOESCAPE, 0),
            ("\\", "\\", 0, 0),
            ("a/*", "a/b", PATHNAME, 0),
            ("a*", "a/b", PATHNAME, 1),
            ("a?b", "a/b", PATHNAME, 1),
            ("a[/]b", "a/b", PATHNAME, 1),
            ("a/b", "a/b", PATHNAME, 0),
            ("*", ".hidden", PERIOD, 1),
            ("?hidden", ".hidden", PERIOD, 1),
            ("[.]hidden", ".hidden", PERIOD, 1),
            (".*", ".hidden", PERIOD, 0),
            ("*", "visible", PERIOD, 0),
            ("a/*", "a/.x", PATHNAME | PERIOD, 1),
            ("A*", "abc", CASEFOLD, 0),
            ("[A-C]x", "bx", CASEFOLD, 0),
            ("abc", "abd", 0, 1),
            ("abc", "ab", 0, 1),
            ("ab", "abc", 0, 1),
            ("**x", "ax", 0, 0),
        ] {
            assert_eq!(m(pat, s, flags), want, "fnmatch({pat:?}, {s:?}, {flags})");
        }
        assert!(matches!(e.try_call("fnmatch", &[BAD, A, 0]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("fnmatch", &[A, BAD, 0]), Err(Stop::Fault(_))));
    }

    #[test]
    fn file_table_is_shared_with_stdio_style_users() {
        // Two opens of one file see each other's writes through the host file.
        let (mut e, _tmp) = fs_env();
        let path = put(&e, A, "/data/data/pkg/shared");
        e.put_str(B, "xyz");
        let w = call(&mut e, "open", &[path, vfs::O_WRONLY | vfs::O_CREAT, 0]);
        let r = call(&mut e, "open", &[path, 0, 0]);
        call(&mut e, "write", &[w, B, 3]);
        assert_eq!(call(&mut e, "read", &[r, OUT, 10]), 3);
        // Exhausting descriptors fails with EMFILE.
        loop {
            if call(&mut e, "open", &[path, 0, 0]) == NEG1 {
                break;
            }
        }
        assert_eq!(e.errno(), 24);
    }
}

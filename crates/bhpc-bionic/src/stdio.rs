//! `<stdio.h>`: the three standard streams (`__sF`) mapped to the host's stdout/stderr, the `printf`
//! family on top of [`crate::fmt`], and the bionic stdio internals (`__swbuf`, `__srget`, `__srefill`)
//! that the `putc`/`getc` macros call. Other streams are `fopen`ed files on the guest file system
//! ([`crate::vfs`]): the host keeps the descriptor and the pushed-back bytes, and the guest `FILE` struct
//! only carries the flags and the descriptor number (the buffer pointers stay zero, so every `getc` and
//! `putc` macro falls through to the host).

use crate::fmt::{self, FmtError, FmtSource};
use crate::guest::{cstr, rd, wr};
use crate::layout::{errno, file, PAGE_SIZE};
use crate::misc::{fail, set_errno};
use crate::state::FileRec;
use crate::string::error_text;
use crate::vfs;
use crate::{lock, Impl, Shared};
use bhpc_cpu::{ArgCursor, Cpu, Stop};
use std::io::Write;

/// Which standard stream a `FILE*` is (0 stdin, 1 stdout, 2 stderr).
fn stream_of(st: &Shared, fp: u32) -> Option<u32> {
    let sf = lock(st).data["__sF"];
    let off = fp.wrapping_sub(sf);
    (off < 3 * file::SIZE && off % file::SIZE == 0).then_some(off / file::SIZE)
}

/// The descriptor behind a file opened with `fopen`.
fn file_fd(st: &Shared, fp: u32) -> Option<i32> {
    lock(st).files.get(&fp).map(|f| f.fd)
}

/// A custom-stream callback of the guest `FILE` at `fp` (`_read`, `_write`, `_seek`, `_close`) and its
/// cookie; `None` for an ordinary stream.
fn callback(c: &Cpu, fp: u32, field: u32) -> Option<(u32, u32)> {
    let f = c.mem().read_u32(fp + field).ok().filter(|f| *f != 0)?;
    Some((f, c.mem().read_u32(fp + file::COOKIE).unwrap_or(0)))
}

/// Run a guest stream callback on `data` (written to a scratch buffer first for output).
/// Returns the callback's result, or `None` after a guest failure.
fn call_stream_io(c: &mut Cpu, st: &Shared, func: u32, cookie: u32, data: Option<&[u8]>, n: u32) -> Option<i32> {
    let buf = lock(st).heap.alloc(n.max(1));
    if buf == 0 {
        set_errno(c, st, errno::ENOMEM);
        return Some(-1);
    }
    if let Some(d) = data {
        // The scratch block comes from the guest heap, which is always mapped.
        let _ = c.mem().write_bytes(buf, d);
    }
    let r = c.call_or_stop(func, &[cookie, buf, n]).map(|r| r.r0 as i32);
    lock(st).heap.free(buf);
    r
}

/// Write to the stream `fp` (a standard stream, an `fopen`ed file or a `funopen` stream); false (with
/// `errno` set) for stdin and foreign `FILE`s.
fn write_stream(c: &mut Cpu, st: &Shared, fp: u32, data: &[u8]) -> bool {
    if let Some((func, cookie)) = callback(c, fp, file::WRITE) {
        let mut done = 0;
        while done < data.len() {
            let chunk = &data[done..];
            match call_stream_io(c, st, func, cookie, Some(chunk), chunk.len() as u32) {
                Some(n) if n > 0 => done += n as usize,
                Some(_) => {
                    set_flag(c, fp, file::SERR, true);
                    return false;
                }
                None => return false,
            }
        }
        return true;
    }
    let fd = match stream_of(st, fp) {
        Some(n) => n as i32,
        None => match file_fd(st, fp) {
            Some(fd) => fd,
            None => {
                set_errno(c, st, errno::EBADF);
                return false;
            }
        },
    };
    match crate::sys::write_fd(st, fd as u32, data) {
        Ok(_) => true,
        Err(e) => {
            set_errno(c, st, e);
            set_flag(c, fp, file::SERR, true);
            false
        }
    }
}

/// Set or clear bits in the guest `FILE`'s `_flags`.
fn set_flag(c: &mut Cpu, fp: u32, bits: u16, on: bool) {
    let Ok(f) = c.mem().read_u16(fp + file::FLAGS) else { return };
    let f = if on { f | bits } else { f & !bits };
    let _ = c.mem().write_u16(fp + file::FLAGS, f);
}

fn stdout_fp(st: &Shared) -> u32 {
    lock(st).data["__sF"] + file::SIZE
}

fn stderr_fp(st: &Shared) -> u32 {
    lock(st).data["__sF"] + 2 * file::SIZE
}

fn minus1(c: &mut Cpu) {
    c.ret_u32(-1i32 as u32);
}

// ---------- argument sources ----------

/// Variadic arguments: either from the registers/stack of the calling thunk, or from a `va_list`
/// (on ARM EABI a single pointer to the next 4-byte argument slot).
struct CpuSource<'a> {
    c: &'a mut Cpu,
    regs: Option<ArgCursor>,
    ap: u32,
}

impl FmtSource for CpuSource<'_> {
    fn u32(&mut self) -> u32 {
        match self.regs.as_mut() {
            Some(cur) => cur.u32(self.c),
            None => {
                let v = self.c.mem().read_u32(self.ap).unwrap_or(0);
                self.ap = self.ap.wrapping_add(4);
                v
            }
        }
    }
    fn u64(&mut self) -> u64 {
        match self.regs.as_mut() {
            Some(cur) => cur.u64(self.c),
            None => {
                self.ap = (self.ap.wrapping_add(7)) & !7;
                let v = self.c.mem().read_u64(self.ap).unwrap_or(0);
                self.ap = self.ap.wrapping_add(8);
                v
            }
        }
    }
    fn f64(&mut self) -> f64 {
        f64::from_bits(self.u64())
    }
    fn cstr(&mut self, addr: u32) -> Option<Vec<u8>> {
        cstr(self.c, addr)
    }
    fn wstr(&mut self, addr: u32) -> Option<Vec<u32>> {
        let mut out = Vec::new();
        let mut a = addr;
        loop {
            let w = crate::guest::rd32(self.c, a)?;
            if w == 0 {
                return Some(out);
            }
            out.push(w);
            a = a.wrapping_add(4);
        }
    }
}

/// Where the variadic arguments live.
#[derive(Clone, Copy)]
pub(crate) enum Args {
    Regs(ArgCursor),
    Va(u32),
}

/// Format the guest string at `fmt_addr`. On failure the guest has been stopped or `errno` set, and
/// `None` is returned.
pub(crate) fn format_guest(c: &mut Cpu, st: &Shared, fmt_addr: u32, args: Args) -> Option<Vec<u8>> {
    let f = cstr(c, fmt_addr)?;
    let mut src = match args {
        Args::Regs(cur) => CpuSource { c, regs: Some(cur), ap: 0 },
        Args::Va(ap) => CpuSource { c, regs: None, ap },
    };
    let r = fmt::format(&f, &mut src);
    match r {
        Ok(v) => Some(v),
        Err(FmtError::PercentN) => {
            c.request_stop(Stop::Host("printf: %n conversion refused".into()));
            None
        }
        // The `%s` argument was unreadable: the guest was already stopped with the fault.
        Err(FmtError::Fault) => None,
        Err(FmtError::TooLong) => {
            set_errno(c, st, errno::EINVAL);
            None
        }
    }
}

fn to_stream(c: &mut Cpu, st: &Shared, fp: u32, fmt_addr: u32, args: Args) {
    let Some(out) = format_guest(c, st, fmt_addr, args) else { return minus1(c) };
    if write_stream(c, st, fp, &out) {
        c.ret_u32(out.len() as u32);
    } else {
        minus1(c);
    }
}

fn to_buffer(c: &mut Cpu, st: &Shared, buf: u32, limit: Option<u32>, fmt_addr: u32, args: Args) {
    let Some(out) = format_guest(c, st, fmt_addr, args) else { return minus1(c) };
    let keep = match limit {
        None => out.len(),
        Some(0) => {
            return c.ret_u32(out.len() as u32);
        }
        Some(n) => out.len().min(n as usize - 1),
    };
    if crate::guest::wr_cstr(c, buf, &out[..keep]) {
        c.ret_u32(out.len() as u32);
    }
}

fn printf(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let f = a.u32(c);
    to_stream(c, st, stdout_fp(st), f, Args::Regs(a));
}

fn fprintf(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (fp, f) = (a.u32(c), a.u32(c));
    to_stream(c, st, fp, f, Args::Regs(a));
}

fn vprintf(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (f, ap) = (a.u32(c), a.u32(c));
    to_stream(c, st, stdout_fp(st), f, Args::Va(ap));
}

fn vfprintf(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (fp, f, ap) = (a.u32(c), a.u32(c), a.u32(c));
    to_stream(c, st, fp, f, Args::Va(ap));
}

fn sprintf(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (buf, f) = (a.u32(c), a.u32(c));
    to_buffer(c, st, buf, None, f, Args::Regs(a));
}

fn snprintf(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (buf, n, f) = (a.u32(c), a.u32(c), a.u32(c));
    to_buffer(c, st, buf, Some(n), f, Args::Regs(a));
}

fn vsprintf(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (buf, f, ap) = (a.u32(c), a.u32(c), a.u32(c));
    to_buffer(c, st, buf, None, f, Args::Va(ap));
}

fn vsnprintf(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (buf, n, f, ap) = (a.u32(c), a.u32(c), a.u32(c), a.u32(c));
    to_buffer(c, st, buf, Some(n), f, Args::Va(ap));
}

// ---------- plain output ----------

fn puts(c: &mut Cpu, st: &Shared) {
    let p = c.args().u32(c);
    let Some(mut s) = cstr(c, p) else { return };
    s.push(b'\n');
    let fp = stdout_fp(st);
    write_stream(c, st, fp, &s);
    c.ret_u32(1);
}

fn fputs(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (p, fp) = (a.u32(c), a.u32(c));
    let Some(s) = cstr(c, p) else { return };
    if write_stream(c, st, fp, &s) {
        c.ret_u32(1);
    } else {
        minus1(c);
    }
}

fn fputc(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (ch, fp) = (a.u32(c), a.u32(c));
    put_byte(c, st, ch, fp);
}

fn put_byte(c: &mut Cpu, st: &Shared, ch: u32, fp: u32) {
    if write_stream(c, st, fp, &[ch as u8]) {
        c.ret_u32(ch & 0xff);
    } else {
        minus1(c);
    }
}

fn putchar(c: &mut Cpu, st: &Shared) {
    let ch = c.args().u32(c);
    put_byte(c, st, ch, stdout_fp(st));
}

/// `__swbuf(ch, fp)`: the slow path of the `putc` macro.
fn swbuf(c: &mut Cpu, st: &Shared) {
    fputc(c, st)
}

fn fwrite(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (p, size, n, fp) = (a.u32(c), a.u32(c), a.u32(c), a.u32(c));
    let total = size.saturating_mul(n);
    let Some(b) = rd(c, p, total) else { return };
    if write_stream(c, st, fp, &b) {
        c.ret_u32(n);
    } else {
        c.ret_u32(0);
    }
}

fn fflush(c: &mut Cpu, st: &Shared) {
    let fp = c.args().u32(c);
    if fp == 0 {
        let mut s = lock(st);
        let _ = s.stdout.flush();
        let _ = s.stderr.flush();
        drop(s);
        return c.ret_u32(0);
    }
    match stream_of(st, fp) {
        Some(0) => c.ret_u32(0),
        Some(1) => {
            let _ = lock(st).stdout.flush();
            c.ret_u32(0);
        }
        Some(_) => {
            let _ = lock(st).stderr.flush();
            c.ret_u32(0);
        }
        // Files are written through, so there is nothing buffered.
        None if file_fd(st, fp).is_some() => c.ret_u32(0),
        None => fail(c, st, errno::EBADF),
    }
}

fn perror(c: &mut Cpu, st: &Shared) {
    let p = c.args().u32(c);
    let e = crate::misc::get_errno(c, st);
    let mut line = Vec::new();
    if p != 0 {
        let Some(prefix) = cstr(c, p) else { return };
        if !prefix.is_empty() {
            line.extend_from_slice(&prefix);
            line.extend_from_slice(b": ");
        }
    }
    line.extend_from_slice(error_text(e).as_bytes());
    line.push(b'\n');
    let fp = stderr_fp(st);
    write_stream(c, st, fp, &line);
}

// ---------- files ----------

/// `fopen` mode string -> open flags and `FILE` flags.
fn parse_mode(mode: &[u8]) -> Option<(u32, u16)> {
    let plus = mode.contains(&b'+');
    Some(match (mode.first()?, plus) {
        (b'r', false) => (0, file::SRD),
        (b'r', true) => (vfs::O_RDWR, file::SRW),
        (b'w', false) => (vfs::O_WRONLY | vfs::O_CREAT | vfs::O_TRUNC, file::SWR),
        (b'w', true) => (vfs::O_RDWR | vfs::O_CREAT | vfs::O_TRUNC, file::SRW),
        (b'a', false) => (vfs::O_WRONLY | vfs::O_CREAT | vfs::O_APPEND, file::SWR),
        (b'a', true) => (vfs::O_RDWR | vfs::O_CREAT | vfs::O_APPEND, file::SRW),
        _ => return None,
    })
}

fn fopen(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (p, m) = (a.u32(c), a.u32(c));
    let (Some(path), Some(mode)) = (cstr(c, p), cstr(c, m)) else { return };
    let Some((oflags, fflags)) = parse_mode(&mode) else {
        set_errno(c, st, errno::EINVAL);
        return c.ret_u32(0);
    };
    let mut s = lock(st);
    let fd = match s.vfs.open(&String::from_utf8_lossy(&path), oflags) {
        Ok(fd) => fd,
        Err(e) => {
            drop(s);
            set_errno(c, st, e);
            return c.ret_u32(0);
        }
    };
    let fp = s.heap.calloc(1, file::SIZE);
    if fp == 0 {
        let _ = s.vfs.close(fd);
        drop(s);
        set_errno(c, st, errno::ENOMEM);
        return c.ret_u32(0);
    }
    s.files.insert(fp, FileRec { fd, unget: Vec::new() });
    let mem = s.mem.clone();
    drop(s);
    // The block is calloc'ed and always mapped.
    let _ = mem.write_u16(fp + file::FLAGS, fflags);
    let _ = mem.write_u16(fp + file::FILE_NO, fd as u16);
    let _ = mem.write_u32(fp + file::COOKIE, fp);
    let _ = mem.write_u32(fp + file::BLKSIZE, PAGE_SIZE);
    c.ret_u32(fp);
}

/// `funopen(cookie, read, write, seek, close)`: a stream whose I/O is done by guest callbacks.
fn funopen(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (cookie, read, write, seek, close) = (a.u32(c), a.u32(c), a.u32(c), a.u32(c), a.u32(c));
    if read == 0 && write == 0 {
        return fail_null(c, st, errno::EINVAL);
    }
    let flags = match (read != 0, write != 0) {
        (true, true) => file::SRW,
        (true, false) => file::SRD,
        _ => file::SWR,
    };
    let fp = lock(st).heap.calloc(1, file::SIZE);
    if fp == 0 {
        return fail_null(c, st, errno::ENOMEM);
    }
    lock(st).files.insert(fp, FileRec { fd: -1, unget: Vec::new() });
    let m = c.mem().clone();
    // The block is calloc'ed and always mapped.
    let _ = m.write_u16(fp + file::FLAGS, flags);
    let _ = m.write_u16(fp + file::FILE_NO, u16::MAX);
    for (off, v) in
        [(file::COOKIE, cookie), (file::READ, read), (file::WRITE, write), (file::SEEK, seek), (file::CLOSE, close)]
    {
        let _ = m.write_u32(fp + off, v);
    }
    c.ret_u32(fp);
}

fn fail_null(c: &mut Cpu, st: &Shared, e: u32) {
    set_errno(c, st, e);
    c.ret_u32(0);
}

fn fclose(c: &mut Cpu, st: &Shared) {
    let fp = c.args().u32(c);
    let known = lock(st).files.contains_key(&fp);
    if let Some((func, cookie)) = callback(c, fp, file::CLOSE).filter(|_| known) {
        let r = c.call_or_stop(func, &[cookie]);
        let mut s = lock(st);
        s.files.remove(&fp);
        s.heap.free(fp);
        drop(s);
        if let Some(r) = r {
            c.ret_u32(r.r0);
        }
        return;
    }
    let mut s = lock(st);
    let Some(rec) = s.files.remove(&fp) else {
        drop(s);
        return if stream_of(st, fp).is_some() { c.ret_u32(0) } else { fail(c, st, errno::EBADF) };
    };
    // A `funopen` stream without a close callback has no descriptor to close.
    let r = if rec.fd < 0 { Ok(()) } else { s.vfs.close(rec.fd) };
    s.heap.free(fp);
    drop(s);
    match r {
        Ok(()) => c.ret_u32(0),
        Err(e) => fail(c, st, e),
    }
}

/// Up to `n` bytes from the stream (pushed-back bytes first). Sets `SEOF`/`SERR`. `None` for streams that
/// cannot be read (stdout, stderr, foreign `FILE`s: `errno` set).
fn read_stream(c: &mut Cpu, st: &Shared, fp: u32, n: usize) -> Option<Vec<u8>> {
    if let Some((func, cookie)) = callback(c, fp, file::READ) {
        return read_callback(c, st, fp, func, cookie, n);
    }
    if stream_of(st, fp) == Some(0) {
        set_flag(c, fp, file::SEOF, true);
        return Some(Vec::new());
    }
    let mut s = lock(st);
    let Some(rec) = s.files.get_mut(&fp) else {
        drop(s);
        set_errno(c, st, errno::EBADF);
        return None;
    };
    let mut out = Vec::new();
    while out.len() < n && !rec.unget.is_empty() {
        out.push(rec.unget.pop().expect("checked non-empty"));
    }
    let fd = rec.fd;
    let r = if out.len() < n { s.vfs.read(fd, n - out.len()) } else { Ok(Vec::new()) };
    drop(s);
    match r {
        Ok(more) => {
            let eof = out.len() + more.len() < n;
            out.extend_from_slice(&more);
            if eof {
                set_flag(c, fp, file::SEOF, true);
            }
            Some(out)
        }
        Err(e) => {
            set_errno(c, st, e);
            set_flag(c, fp, file::SERR, true);
            None
        }
    }
}

/// Read `n` bytes through a `funopen` read callback (pushed-back bytes first).
fn read_callback(c: &mut Cpu, st: &Shared, fp: u32, func: u32, cookie: u32, n: usize) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    while out.len() < n {
        let pushed = lock(st).files.get_mut(&fp).and_then(|f| f.unget.pop());
        if let Some(b) = pushed {
            out.push(b);
            continue;
        }
        let want = (n - out.len()) as u32;
        let buf = lock(st).heap.alloc(want);
        if buf == 0 {
            set_errno(c, st, errno::ENOMEM);
            return None;
        }
        let r = c.call_or_stop(func, &[cookie, buf, want]).map(|r| r.r0 as i32);
        let got = match r {
            Some(k) if k > 0 => c.mem().read_bytes(buf, k as u32).ok(),
            _ => None,
        };
        lock(st).heap.free(buf);
        match (r, got) {
            (None, _) => return None,
            (Some(k), Some(bytes)) if k > 0 => out.extend_from_slice(&bytes),
            (Some(k), _) => {
                set_flag(c, fp, if k < 0 { file::SERR } else { file::SEOF }, true);
                break;
            }
        }
    }
    Some(out)
}

fn fread(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (p, size, n, fp) = (a.u32(c), a.u32(c), a.u32(c), a.u32(c));
    let total = size.saturating_mul(n);
    if total == 0 {
        return c.ret_u32(0);
    }
    let Some(data) = read_stream(c, st, fp, total as usize) else { return c.ret_u32(0) };
    if wr(c, p, &data) {
        c.ret_u32(data.len() as u32 / size);
    }
}

fn fgets(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (buf, n, fp) = (a.u32(c), a.u32(c), a.u32(c));
    if n <= 1 {
        return c.ret_u32(0);
    }
    let mut line = Vec::new();
    while line.len() + 1 < n as usize {
        let Some(b) = read_stream(c, st, fp, 1) else { return c.ret_u32(0) };
        let Some(&byte) = b.first() else { break };
        line.push(byte);
        if byte == b'\n' {
            break;
        }
    }
    if line.is_empty() {
        return c.ret_u32(0);
    }
    if crate::guest::wr_cstr(c, buf, &line) {
        c.ret_u32(buf);
    }
}

/// `fgetc`, `getc` and the macro slow path `__srget`.
fn fgetc(c: &mut Cpu, st: &Shared) {
    let fp = c.args().u32(c);
    // The `getc` macro decrements `_r` before calling in; keep it from drifting.
    let _ = c.mem().write_u32(fp + file::R, 0);
    match read_stream(c, st, fp, 1).and_then(|b| b.first().copied()) {
        Some(b) => c.ret_u32(u32::from(b)),
        None => minus1(c),
    }
}

fn getchar(c: &mut Cpu, _: &Shared) {
    minus1(c); // stdin is at end of file
}

fn ungetc(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (ch, fp) = (a.u32(c), a.u32(c));
    if ch as i32 == -1 {
        return minus1(c);
    }
    let mut s = lock(st);
    match s.files.get_mut(&fp) {
        Some(rec) => rec.unget.push(ch as u8),
        None => {
            drop(s);
            return minus1(c);
        }
    }
    drop(s);
    set_flag(c, fp, file::SEOF, false);
    c.ret_u32(ch & 0xff);
}

fn fseek(c: &mut Cpu, st: &Shared) {
    args3(c, |c, fp, off, whence| {
        let Some(fd) = file_fd(st, fp) else { return fail(c, st, errno::EBADF) };
        let mut s = lock(st);
        // A relative seek is relative to the logical position, which is behind the pushed-back bytes.
        let pending = s.files.get(&fp).map_or(0, |f| f.unget.len() as i64);
        let off = i64::from(off as i32) - if whence == 1 { pending } else { 0 };
        let r = s.vfs.lseek(fd, off, whence);
        if r.is_ok() {
            if let Some(f) = s.files.get_mut(&fp) {
                f.unget.clear();
            }
        }
        drop(s);
        match r {
            Ok(_) => {
                set_flag(c, fp, file::SEOF, false);
                c.ret_u32(0);
            }
            Err(e) => fail(c, st, e),
        }
    });
}

fn args3(c: &mut Cpu, f: impl FnOnce(&mut Cpu, u32, u32, u32)) {
    let mut a = c.args();
    let (x, y, z) = (a.u32(c), a.u32(c), a.u32(c));
    f(c, x, y, z);
}

fn ftell(c: &mut Cpu, st: &Shared) {
    let fp = c.args().u32(c);
    let Some(fd) = file_fd(st, fp) else { return fail(c, st, errno::EBADF) };
    let mut s = lock(st);
    let pending = s.files.get(&fp).map_or(0, |f| f.unget.len() as u64);
    let r = s.vfs.lseek(fd, 0, 1);
    drop(s);
    match r {
        Ok(p) => c.ret_u32(p.saturating_sub(pending) as u32),
        Err(e) => fail(c, st, e),
    }
}

fn rewind(c: &mut Cpu, st: &Shared) {
    let fp = c.args().u32(c);
    let Some(fd) = file_fd(st, fp) else { return };
    let mut s = lock(st);
    let _ = s.vfs.lseek(fd, 0, 0);
    if let Some(f) = s.files.get_mut(&fp) {
        f.unget.clear();
    }
    drop(s);
    set_flag(c, fp, file::SEOF | file::SERR, false);
}

fn clearerr(c: &mut Cpu, _: &Shared) {
    let fp = c.args().u32(c);
    set_flag(c, fp, file::SEOF | file::SERR, false);
}

fn setvbuf(c: &mut Cpu, _: &Shared) {
    c.ret_u32(0);
}

fn nothing(_: &mut Cpu, _: &Shared) {}

pub(crate) const TABLE: &[(&str, Impl)] = &[
    ("printf", printf),
    ("fprintf", fprintf),
    ("vprintf", vprintf),
    ("vfprintf", vfprintf),
    ("sprintf", sprintf),
    ("snprintf", snprintf),
    ("vsprintf", vsprintf),
    ("vsnprintf", vsnprintf),
    ("puts", puts),
    ("fputs", fputs),
    ("fputc", fputc),
    ("putc", fputc),
    ("putchar", putchar),
    ("__swbuf", swbuf),
    ("fwrite", fwrite),
    ("fflush", fflush),
    ("perror", perror),
    ("fgetc", fgetc),
    ("getc", fgetc),
    ("getchar", getchar),
    ("ungetc", ungetc),
    ("__srget", fgetc),
    ("__srefill", fgetc),
    ("fgets", fgets),
    ("fread", fread),
    ("fopen", fopen),
    ("funopen", funopen),
    ("fclose", fclose),
    ("fseek", fseek),
    ("ftell", ftell),
    ("rewind", rewind),
    ("setvbuf", setvbuf),
    ("setbuf", nothing),
    ("clearerr", clearerr),
];

#[cfg(test)]
mod tests {
    use crate::layout::file;
    use crate::tests_env::{Env, SCRATCH};
    use bhpc_cpu::Stop;

    const A: u32 = SCRATCH;
    const B: u32 = SCRATCH + 0x100;
    const BUF: u32 = SCRATCH + 0x400;
    const BAD: u32 = 0x10;

    fn dbl(v: f64) -> [u32; 2] {
        let b = v.to_bits();
        [b as u32, (b >> 32) as u32]
    }

    #[test]
    fn std_streams_have_bionic_layout() {
        let e = Env::new();
        let sf = e.data("__sF");
        let m = e.mem();
        for (i, (flags, fd)) in [(0x4u16, 0u16), (0x9, 1), (0xa, 2)].into_iter().enumerate() {
            let fp = sf + i as u32 * file::SIZE;
            assert_eq!(m.read_u16(fp + file::FLAGS).unwrap(), flags);
            assert_eq!(m.read_u16(fp + file::FILE_NO).unwrap(), fd);
            assert_eq!(m.read_u32(fp + file::COOKIE).unwrap(), fp);
            assert_eq!(m.read_u32(fp + file::P).unwrap(), 0);
            assert_eq!(m.read_u32(fp + file::W).unwrap(), 0, "putc macro must take the __swbuf path");
            assert_eq!(m.read_u32(fp + file::R).unwrap(), 0, "getc macro must take the __srget path");
        }
        assert!(e.data("__stack_chk_guard") != 0);
        assert_eq!(m.read_u32(e.data("__stack_chk_guard")).unwrap(), 0xC0FF_EE00);
    }

    #[test]
    fn printf_family_to_streams() {
        let mut e = Env::new();
        let sf = e.data("__sF");
        e.put_str(A, "n=%d s=%s\n");
        e.put_str(B, "str");
        assert_eq!(e.call("printf", &[A, 42, B]).r0, 11);
        assert_eq!(e.out.text(), "n=42 s=str\n");
        e.put_str(A, "[%5.1f]");
        let [lo, hi] = dbl(3.11);
        assert_eq!(e.call("fprintf", &[sf + 2 * file::SIZE, A, lo, hi]).r0, 7);
        assert_eq!(e.err.text(), "[  3.1]");
        assert_eq!(e.call("fprintf", &[sf + file::SIZE, A, lo, hi]).r0, 7);
        assert!(e.out.text().ends_with("[  3.1]"));
        // stdin and foreign FILE pointers are not writable.
        assert_eq!(e.call("fprintf", &[sf, A, lo, hi]).r0 as i32, -1);
        assert_eq!(e.errno(), 9);
        e.set_errno(0);
        assert_eq!(e.call("fprintf", &[sf + 1, A, lo, hi]).r0 as i32, -1, "misaligned FILE");
        assert_eq!(e.call("fprintf", &[0x1234, A, lo, hi]).r0 as i32, -1);
        assert_eq!(e.errno(), 9);
        e.put_str(A, "%s");
        assert_eq!(e.call("printf", &[A, 0]).r0, 6);
    }

    #[test]
    fn sprintf_family() {
        let mut e = Env::new();
        e.put_str(A, "%d-%s-%lld");
        e.put_str(B, "xy");
        // sprintf(buf, fmt, 7, "xy", 5000000000LL): the 64-bit value starts on an even stack slot.
        let ll = 5_000_000_000u64;
        assert_eq!(e.call("sprintf", &[BUF, A, 7, B, ll as u32, (ll >> 32) as u32]).r0, 15);
        assert_eq!(e.get_str(BUF), "7-xy-5000000000");
        // snprintf: truncation reports the full length.
        e.put_str(A, "hello %s");
        assert_eq!(e.call("snprintf", &[BUF, 8, A, B]).r0, 8);
        assert_eq!(e.get_str(BUF), "hello x");
        assert_eq!(e.call("snprintf", &[BUF, 100, A, B]).r0, 8);
        assert_eq!(e.get_str(BUF), "hello xy");
        e.put_str(BUF, "keep");
        assert_eq!(e.call("snprintf", &[BUF, 0, A, B]).r0, 8);
        assert_eq!(e.get_str(BUF), "keep");
        assert_eq!(e.call("snprintf", &[0, 0, A, B]).r0, 8);
        // Unwritable destination.
        assert!(matches!(e.try_call("snprintf", &[BAD, 8, A, B]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("sprintf", &[BAD, A, B]), Err(Stop::Fault(_))));
    }

    #[test]
    fn va_list_variants() {
        let mut e = Env::new();
        // va_list for (int 5, double 2.5, char* "xy"): int at +0, double 8-aligned at +8, pointer at +16.
        let ap = SCRATCH + 0x800;
        let [lo, hi] = dbl(2.5);
        e.put_str(B, "xy");
        let m = e.mem();
        for (i, w) in [5, 0, lo, hi, B].iter().enumerate() {
            m.write_u32(ap + 4 * i as u32, *w).unwrap();
        }
        e.put_str(A, "%d %.1f %s");
        assert_eq!(e.call("vsnprintf", &[BUF, 100, A, ap]).r0, 8);
        assert_eq!(e.get_str(BUF), "5 2.5 xy");
        assert_eq!(e.call("vsprintf", &[BUF, A, ap]).r0, 8);
        assert_eq!(e.call("vsnprintf", &[BUF, 4, A, ap]).r0, 8);
        assert_eq!(e.get_str(BUF), "5 2");
        assert_eq!(e.call("vprintf", &[A, ap]).r0, 8);
        assert_eq!(e.out.text(), "5 2.5 xy");
        let sf = e.data("__sF");
        assert_eq!(e.call("vfprintf", &[sf + 2 * file::SIZE, A, ap]).r0, 8);
        assert_eq!(e.err.text(), "5 2.5 xy");
        // Unreadable va_list reads as zeros instead of faulting.
        e.put_str(A, "%d");
        assert_eq!(e.call("vsnprintf", &[BUF, 100, A, BAD]).r0, 1);
        e.put_str(A, "%f");
        assert_eq!(e.call("vsnprintf", &[BUF, 100, A, BAD]).r0, 8);
    }

    #[test]
    fn printf_failures() {
        let mut e = Env::new();
        e.put_str(A, "abc%n");
        let r = e.try_call("printf", &[A, BUF]);
        assert!(matches!(r, Err(Stop::Host(ref m)) if m.contains("%n")), "{r:?}");
        e.put_str(A, "%s");
        assert!(matches!(e.try_call("printf", &[A, BAD]), Err(Stop::Fault(_))));
        e.put_str(A, "%ls");
        assert!(matches!(e.try_call("printf", &[A, BAD]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("printf", &[BAD]), Err(Stop::Fault(_))));
        e.put_str(A, "%99999999d");
        assert_eq!(e.call("printf", &[A, 1]).r0 as i32, -1);
        assert_eq!(e.errno(), 22);
        e.put_str(A, "%ls|%lc");
        let ws = SCRATCH + 0x700;
        e.mem().write_u32(ws, 'h' as u32).unwrap();
        e.mem().write_u32(ws + 4, 0xe9).unwrap();
        e.mem().write_u32(ws + 8, 0).unwrap();
        assert_eq!(e.call("printf", &[A, ws, 120]).r0, 5);
        assert!(e.out.text().ends_with("h\u{e9}|x"));
    }

    #[test]
    fn plain_output_functions() {
        let mut e = Env::new();
        let sf = e.data("__sF");
        let (stdout, stderr, stdin) = (sf + file::SIZE, sf + 2 * file::SIZE, sf);
        e.put_str(A, "line");
        assert_eq!(e.call("puts", &[A]).r0, 1);
        assert_eq!(e.call("fputs", &[A, stdout]).r0, 1);
        assert_eq!(e.call("fputs", &[A, stderr]).r0, 1);
        assert_eq!(e.call("fputs", &[A, stdin]).r0 as i32, -1);
        assert_eq!(e.out.text(), "line\nline");
        assert_eq!(e.err.text(), "line");
        assert_eq!(e.call("fputc", &['a' as u32, stdout]).r0, 'a' as u32);
        assert_eq!(e.call("putc", &['b' as u32 + 256, stdout]).r0, 'b' as u32);
        assert_eq!(e.call("putchar", &['c' as u32]).r0, 'c' as u32);
        assert_eq!(e.call("__swbuf", &['d' as u32, stderr]).r0, 'd' as u32);
        assert_eq!(e.call("fputc", &['x' as u32, stdin]).r0 as i32, -1);
        assert_eq!(e.call("putchar", &[0]).r0, 0);
        assert!(e.out.text().ends_with("abc\0"));
        assert_eq!(e.err.text(), "lined");
        e.mem().write_bytes(B, b"0123456789").unwrap();
        assert_eq!(e.call("fwrite", &[B, 2, 3, stdout]).r0, 3);
        assert!(e.out.text().ends_with("012345"));
        assert_eq!(e.call("fwrite", &[B, 2, 3, stdin]).r0, 0);
        assert_eq!(e.call("fwrite", &[B, 0, 3, stdout]).r0, 3, "zero-size writes succeed");
        assert!(matches!(e.try_call("fwrite", &[BAD, 1, 3, stdout]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("puts", &[BAD]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("fputs", &[BAD, stdout]), Err(Stop::Fault(_))));
        e.try_call("puts", &[A]).unwrap();
    }

    #[test]
    fn puts_to_a_closed_stdout_is_not_an_error_for_the_guest() {
        // Host write errors are ignored; this documents that puts still reports success.
        let mut e = Env::new();
        e.put_str(A, "z");
        assert_eq!(e.call("puts", &[A]).r0, 1);
    }

    #[test]
    fn flush_and_perror() {
        let mut e = Env::new();
        let sf = e.data("__sF");
        assert_eq!(e.call("fflush", &[0]).r0, 0);
        assert_eq!(e.out.text(), "<flush>");
        assert_eq!(e.err.text(), "<flush>");
        assert_eq!(e.call("fflush", &[sf + file::SIZE]).r0, 0);
        assert_eq!(e.call("fflush", &[sf + 2 * file::SIZE]).r0, 0);
        assert_eq!(e.call("fflush", &[sf]).r0, 0);
        assert_eq!(e.out.text(), "<flush><flush>");
        assert_eq!(e.err.text(), "<flush><flush>");
        assert_eq!(e.call("fflush", &[0x77]).r0 as i32, -1);
        assert_eq!(e.errno(), 9);
        e.set_errno(2);
        e.put_str(A, "open");
        e.call("perror", &[A]);
        e.call("perror", &[0]);
        e.put_str(A, "");
        e.call("perror", &[A]);
        assert!(e
            .err
            .text()
            .ends_with("open: No such file or directory\nNo such file or directory\nNo such file or directory\n"));
        assert!(matches!(e.try_call("perror", &[BAD]), Err(Stop::Fault(_))));
    }

    #[test]
    fn stdin_is_always_at_eof() {
        let mut e = Env::new();
        let sf = e.data("__sF");
        for f in ["fgetc", "getc", "getchar", "ungetc", "__srget", "__srefill"] {
            assert_eq!(e.call(f, &[sf]).r0 as i32, -1, "{f}");
        }
        assert_eq!(e.call("fgets", &[BUF, 10, sf]).r0, 0);
        assert_eq!(e.call("fread", &[BUF, 1, 10, sf]).r0, 0);
        assert_eq!(e.call("setvbuf", &[sf, 0, 2, 0]).r0, 0);
        e.call("setbuf", &[sf, 0]);
        e.call("clearerr", &[sf]);
    }
}

#[cfg(test)]
mod file_tests {
    use crate::layout::file;
    use crate::tests_env::{Env, SCRATCH};
    use bhpc_cpu::{Cpu, Stop};
    use std::sync::{Arc, Mutex};

    const P: u32 = SCRATCH; // path
    const M: u32 = SCRATCH + 0x100; // mode
    const BUF: u32 = SCRATCH + 0x400;
    const BAD: u32 = 0x10;
    const NEG1: u32 = u32::MAX;

    struct Tmp(std::path::PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fs_env() -> (Env, Tmp) {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "bhpc-stdio-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(dir.join("rw")).unwrap();
        let host = dir.join("rw");
        let e = Env::with(move |cfg| {
            cfg.fs = crate::vfs::test_fs(host);
        });
        (e, Tmp(dir))
    }

    fn open(e: &mut Env, name: &str, mode: &str) -> u32 {
        e.put_str(P, &format!("/data/data/pkg/{name}"));
        e.put_str(M, mode);
        e.call("fopen", &[P, M]).r0
    }

    /// A guest callback that returns 0.
    fn ret_zero(c: &mut Cpu) {
        c.ret_u32(0);
    }

    fn flags(e: &Env, fp: u32) -> u16 {
        e.mem().read_u16(fp + file::FLAGS).unwrap()
    }

    #[test]
    fn fopen_modes_and_failures() {
        let (mut e, tmp) = fs_env();
        e.put_str(P, "/data/data/pkg/none");
        for bad in ["", "x", "z+"] {
            e.put_str(M, bad);
            e.set_errno(0);
            assert_eq!(e.call("fopen", &[P, M]).r0, 0, "{bad:?}");
            assert_eq!(e.errno(), 22);
        }
        e.put_str(M, "r");
        assert_eq!(e.call("fopen", &[P, M]).r0, 0);
        assert_eq!(e.errno(), 2);
        assert!(matches!(e.try_call("fopen", &[BAD, M]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("fopen", &[P, BAD]), Err(Stop::Fault(_))));
        for (mode, want) in [
            ("w", file::SWR),
            ("w+", file::SRW),
            ("a", file::SWR),
            ("r+", file::SRW),
            ("a+b", file::SRW),
            ("rb", file::SRD),
        ] {
            let fp = open(&mut e, "m.txt", mode);
            assert_ne!(fp, 0, "{mode}");
            assert_eq!(flags(&e, fp) & 0x1c, want, "{mode}");
            assert_eq!(e.mem().read_u16(fp + file::FILE_NO).unwrap(), 3);
            assert_eq!(e.mem().read_u32(fp + file::COOKIE).unwrap(), fp);
            assert_eq!(e.call("fclose", &[fp]).r0, 0);
            assert!(tmp.0.join("rw/m.txt").exists());
        }
        // Missing parent directory, read-only location, descriptor table exhausted.
        e.put_str(P, "/data/data/pkg/no/dir/f");
        e.put_str(M, "w");
        assert_eq!(e.call("fopen", &[P, M]).r0, 0);
        assert_eq!(e.errno(), 2);
        // No heap for the FILE: the descriptor is given back.
        let mut s = Env::small();
        s.put_str(P, "/proc/cpuinfo");
        s.put_str(M, "r");
        while s.call("malloc", &[16]).r0 != 0 {}
        s.set_errno(0);
        assert_eq!(s.call("fopen", &[P, M]).r0, 0);
        assert_eq!(s.errno(), 12);
        // fclose of something that is not a stream.
        assert_eq!(e.call("fclose", &[0x1234]).r0, NEG1);
        assert_eq!(e.errno(), 9);
        let sf = e.data("__sF");
        assert_eq!(e.call("fclose", &[sf + file::SIZE]).r0, 0);
    }

    #[test]
    fn writing_files_through_every_output_function() {
        let (mut e, tmp) = fs_env();
        let fp = open(&mut e, "out.txt", "w");
        e.put_str(BUF, "abc");
        assert_eq!(e.call("fwrite", &[BUF, 1, 3, fp]).r0, 3);
        assert_eq!(e.call("fputs", &[BUF, fp]).r0, 1);
        assert_eq!(e.call("fputc", &['!' as u32, fp]).r0, '!' as u32);
        assert_eq!(e.call("putc", &['?' as u32, fp]).r0, '?' as u32);
        assert_eq!(e.call("__swbuf", &['.' as u32, fp]).r0, '.' as u32);
        e.put_str(SCRATCH + 0x200, "<%d>");
        assert_eq!(e.call("fprintf", &[fp, SCRATCH + 0x200, 42]).r0, 4);
        assert_eq!(e.call("fflush", &[fp]).r0, 0);
        assert_eq!(std::fs::read_to_string(tmp.0.join("rw/out.txt")).unwrap(), "abcabc!?.<42>");
        // Reading a write-only stream fails and flags the error; writing a read-only one too.
        e.set_errno(0);
        assert_eq!(e.call("fgetc", &[fp]).r0, NEG1);
        assert_eq!(e.errno(), 9);
        assert_ne!(flags(&e, fp) & file::SERR, 0);
        e.call("clearerr", &[fp]);
        assert_eq!(flags(&e, fp) & file::SERR, 0);
        assert_eq!(e.call("fread", &[BUF, 1, 4, fp]).r0, 0);
        assert_eq!(e.call("fclose", &[fp]).r0, 0);
        let ro = open(&mut e, "out.txt", "r");
        e.set_errno(0);
        assert_eq!(e.call("fputc", &['x' as u32, ro]).r0, NEG1);
        assert_ne!(flags(&e, ro) & file::SERR, 0);
        // Append adds at the end.
        let ap = open(&mut e, "out.txt", "a");
        e.call("fputs", &[BUF, ap]);
        e.call("fclose", &[ap]);
        assert!(std::fs::read_to_string(tmp.0.join("rw/out.txt")).unwrap().ends_with("<42>abc"));
        // Closing the file twice: the FILE is gone.
        assert_eq!(e.call("fclose", &[ro]).r0, 0);
        assert_eq!(e.call("fclose", &[ro]).r0, NEG1);
    }

    #[test]
    fn reading_files_line_by_line_byte_by_byte_and_in_blocks() {
        let (mut e, tmp) = fs_env();
        std::fs::write(tmp.0.join("rw/in.txt"), "line one\nline two\nlast").unwrap();
        let fp = open(&mut e, "in.txt", "r");
        assert_eq!(e.call("fgets", &[BUF, 100, fp]).r0, BUF);
        assert_eq!(e.get_str(BUF), "line one\n");
        // Short buffers split a line.
        assert_eq!(e.call("fgets", &[BUF, 5, fp]).r0, BUF);
        assert_eq!(e.get_str(BUF), "line");
        assert_eq!(e.call("fgetc", &[fp]).r0, ' ' as u32);
        assert_eq!(e.call("getc", &[fp]).r0, 't' as u32);
        // ungetc pushes back (last in, first out) and clears EOF.
        assert_eq!(e.call("ungetc", &['Z' as u32, fp]).r0, 'Z' as u32);
        assert_eq!(e.call("ungetc", &['Y' as u32, fp]).r0, 'Y' as u32);
        assert_eq!(e.call("__srget", &[fp]).r0, 'Y' as u32);
        assert_eq!(e.call("fread", &[BUF, 1, 2, fp]).r0, 2);
        assert_eq!(e.mem().read_bytes(BUF, 2).unwrap(), b"Zw");
        assert_eq!(e.call("ungetc", &[NEG1, fp]).r0, NEG1);
        assert_eq!(e.call("ungetc", &['q' as u32, 0x1234]).r0, NEG1);
        // The getc macro leaves `_r` alone for us: it is reset on every slow-path call.
        e.mem().write_u32(fp + file::R, 0xFFFF_FF00).unwrap();
        e.call("__srefill", &[fp]);
        assert_eq!(e.mem().read_u32(fp + file::R).unwrap(), 0);
        // Blocks: items are counted whole.
        assert_eq!(e.call("fread", &[BUF, 3, 10, fp]).r0, 1, "7 bytes left: two full items of 3? no, 'o\\nlast' is 6");
    }

    #[test]
    fn end_of_file_errors_and_seeking() {
        let (mut e, tmp) = fs_env();
        std::fs::write(tmp.0.join("rw/s.txt"), "0123456789").unwrap();
        let fp = open(&mut e, "s.txt", "r+");
        assert_eq!(e.call("ftell", &[fp]).r0, 0);
        assert_eq!(e.call("fread", &[BUF, 1, 100, fp]).r0, 10);
        assert_ne!(flags(&e, fp) & file::SEOF, 0, "short read sets EOF");
        assert_eq!(e.call("fgetc", &[fp]).r0, NEG1);
        assert_eq!(e.call("fgets", &[BUF, 10, fp]).r0, 0);
        assert_eq!(e.call("ftell", &[fp]).r0, 10);
        assert_eq!(e.call("fseek", &[fp, 2, 0]).r0, 0);
        assert_eq!(flags(&e, fp) & file::SEOF, 0, "seeking clears EOF");
        assert_eq!(e.call("fgetc", &[fp]).r0, '2' as u32);
        assert_eq!(e.call("fseek", &[fp, 2, 1]).r0, 0);
        assert_eq!(e.call("fgetc", &[fp]).r0, '5' as u32);
        // Relative seeks and ftell account for pushed-back bytes.
        e.call("ungetc", &['x' as u32, fp]);
        assert_eq!(e.call("ftell", &[fp]).r0, 5);
        assert_eq!(e.call("fseek", &[fp, 1, 1]).r0, 0);
        assert_eq!(e.call("fgetc", &[fp]).r0, '6' as u32);
        assert_eq!(e.call("fseek", &[fp, NEG1, 2]).r0, 0);
        assert_eq!(e.call("fgetc", &[fp]).r0, '9' as u32);
        assert_eq!(e.call("fseek", &[fp, NEG1 - 100, 0]).r0, NEG1, "before the start");
        assert_eq!(e.errno(), 22);
        e.call("rewind", &[fp]);
        assert_eq!(e.call("ftell", &[fp]).r0, 0);
        e.call("ungetc", &['y' as u32, fp]);
        e.call("rewind", &[fp]);
        assert_eq!(e.call("fgetc", &[fp]).r0, '0' as u32, "rewind drops pushed-back bytes");
        // Write at a seek position (r+).
        e.call("fseek", &[fp, 3, 0]);
        e.put_str(BUF, "XY");
        e.call("fwrite", &[BUF, 1, 2, fp]);
        e.call("fclose", &[fp]);
        assert_eq!(std::fs::read_to_string(tmp.0.join("rw/s.txt")).unwrap(), "012XY56789");
        // Not a seekable file.
        let sf = e.data("__sF");
        for f in ["fseek", "ftell"] {
            assert_eq!(e.call(f, &[sf, 0, 0]).r0, NEG1, "{f}");
        }
        assert_eq!(e.errno(), 9);
        e.call("rewind", &[sf]);
        // zero-length reads and bad destinations
        let fp = open(&mut e, "s.txt", "r");
        assert_eq!(e.call("fread", &[BUF, 0, 5, fp]).r0, 0);
        assert_eq!(e.call("fread", &[BUF, 5, 0, fp]).r0, 0);
        assert!(matches!(e.try_call("fread", &[BAD, 1, 5, fp]), Err(Stop::Fault(_))));
        assert_eq!(e.call("fgets", &[BUF, 1, fp]).r0, 0);
        assert!(matches!(e.try_call("fgets", &[BAD, 5, fp]), Err(Stop::Fault(_))));
        // Reading a stream that is not readable at all.
        let out = e.data("__sF") + file::SIZE;
        e.set_errno(0);
        assert_eq!(e.call("fgetc", &[out]).r0, NEG1);
        assert_eq!(e.errno(), 9);
    }

    /// Streams backed by guest callbacks: output captured on the host, input served from a script.
    #[test]
    fn funopen_streams_call_back_into_the_guest() {
        let mut e = Env::new();
        let written = Arc::new(Mutex::new(Vec::<u8>::new()));
        let w2 = written.clone();
        let write_cb = e
            .cpu
            .register_shared_thunk(
                "wcb",
                Arc::new(move |c: &mut Cpu| {
                    let mut a = c.args();
                    let (cookie, buf, n) = (a.u32(c), a.u32(c), a.u32(c));
                    assert_eq!(cookie, 0xC00C);
                    let take = n.min(3); // partial writes force the host to loop
                    w2.lock().unwrap().extend(c.mem().read_bytes(buf, take).unwrap());
                    c.ret_u32(take);
                }),
            )
            .unwrap();
        let script = Arc::new(Mutex::new(vec![b"hello ".to_vec(), b"wor".to_vec(), vec![]]));
        let s2 = script.clone();
        let read_cb = e
            .cpu
            .register_shared_thunk(
                "rcb",
                Arc::new(move |c: &mut Cpu| {
                    let mut a = c.args();
                    let (_cookie, buf, n) = (a.u32(c), a.u32(c), a.u32(c));
                    let mut s = s2.lock().unwrap();
                    if s.is_empty() {
                        return c.ret_u32(-5i32 as u32); // I/O error once the script is used up
                    }
                    let chunk = s.remove(0);
                    let k = chunk.len().min(n as usize);
                    c.mem().write_bytes(buf, &chunk[..k]).unwrap();
                    c.ret_u32(k as u32);
                }),
            )
            .unwrap();
        let closed = Arc::new(Mutex::new(0u32));
        let c2 = closed.clone();
        let close_cb = e
            .cpu
            .register_shared_thunk(
                "ccb",
                Arc::new(move |c: &mut Cpu| {
                    *c2.lock().unwrap() += 1;
                    c.ret_u32(0);
                }),
            )
            .unwrap();
        // Neither read nor write: invalid.
        assert_eq!(e.call("funopen", &[1, 0, 0, 0, 0]).r0, 0);
        assert_eq!(e.errno(), 22);
        // Output.
        let out = e.call("funopen", &[0xC00C, 0, write_cb, 0, close_cb]).r0;
        assert_eq!(flags(&e, out) & 0x1c, file::SWR);
        e.put_str(BUF, "abcdefgh");
        assert_eq!(e.call("fwrite", &[BUF, 1, 8, out]).r0, 8);
        e.put_str(BUF, "ij");
        e.call("fputs", &[BUF, out]);
        assert_eq!(&*written.lock().unwrap(), b"abcdefghij");
        // Input: chunks, EOF, then an error; pushed-back bytes come first.
        let inp = e.call("funopen", &[0xC00C, read_cb, 0, 0, 0]).r0;
        assert_eq!(flags(&e, inp) & 0x1c, file::SRD);
        assert_eq!(e.call("fread", &[BUF, 1, 20, inp]).r0, 9, "stops at the zero-length read");
        assert_eq!(e.mem().read_bytes(BUF, 9).unwrap(), b"hello wor");
        assert_ne!(flags(&e, inp) & file::SEOF, 0);
        assert_eq!(e.call("fgetc", &[inp]).r0, NEG1);
        assert_ne!(flags(&e, inp) & file::SERR, 0, "negative result is an error");
        e.call("clearerr", &[inp]);
        e.call("ungetc", &['!' as u32, inp]);
        script.lock().unwrap().push(b"z".to_vec());
        assert_eq!(e.call("fgetc", &[inp]).r0, '!' as u32);
        assert_eq!(e.call("fgetc", &[inp]).r0, 'z' as u32);
        // Closing calls the callback, or just frees a stream without one.
        assert_eq!(e.call("fclose", &[out]).r0, 0);
        assert_eq!(*closed.lock().unwrap(), 1);
        assert_eq!(e.call("fclose", &[inp]).r0, 0);
        assert_eq!(e.call("fclose", &[out]).r0, NEG1, "already closed");
        // A write callback that fails flags the stream.
        let fail_cb = e.cpu.register_shared_thunk("fcb", Arc::new(ret_zero)).unwrap();
        let bad = e.call("funopen", &[0, 0, fail_cb, 0, 0]).r0;
        e.put_str(BUF, "x");
        assert_eq!(e.call("fputs", &[BUF, bad]).r0, NEG1);
        assert_ne!(flags(&e, bad) & file::SERR, 0);
        // The callback closing result is returned.
        let close_err = e.cpu.register_shared_thunk("cerr", Arc::new(|c: &mut Cpu| c.ret_u32(-1i32 as u32))).unwrap();
        let s = e.call("funopen", &[0, 0, write_cb, 0, close_err]).r0;
        assert_eq!(e.call("fclose", &[s]).r0, NEG1);
        // A failing guest callback stops the guest.
        let boom = e
            .cpu
            .register_shared_thunk("boom", Arc::new(|c: &mut Cpu| c.request_stop(Stop::Host("boom".into()))))
            .unwrap();
        let w = e.call("funopen", &[0, 0, boom, 0, 0]).r0;
        assert!(e.try_call("fputs", &[BUF, w]).is_err());
        let r = e.call("funopen", &[0, boom, 0, 0, 0]).r0;
        assert!(e.try_call("fgetc", &[r]).is_err());
        let both = e.call("funopen", &[0, 0, boom, 0, boom]).r0;
        assert!(e.try_call("fclose", &[both]).is_err());
        // A read callback that returns bytes the buffer cannot hold is the guest's problem; unreadable buffers fault.
        assert!(matches!(e.try_call("fwrite", &[BAD, 1, 4, bad]), Err(Stop::Fault(_))));
    }

    #[test]
    fn libv_style_redirection_of_stdout_through_a_copied_funopen_file() {
        // libv copies a funopen'ed FILE over stdout/stderr so every printf lands in a guest callback.
        let mut e = Env::new();
        let captured = Arc::new(Mutex::new(Vec::<u8>::new()));
        let c2 = captured.clone();
        let cb = e
            .cpu
            .register_shared_thunk(
                "sink",
                Arc::new(move |c: &mut Cpu| {
                    let mut a = c.args();
                    let (_cookie, buf, n) = (a.u32(c), a.u32(c), a.u32(c));
                    c2.lock().unwrap().extend(c.mem().read_bytes(buf, n).unwrap());
                    c.ret_u32(n);
                }),
            )
            .unwrap();
        let fp = e.call("funopen", &[0, 0, cb, 0, 0]).r0;
        let image = e.mem().read_bytes(fp, file::SIZE).unwrap();
        let stdout = e.data("__sF") + file::SIZE;
        e.mem().write_bytes(stdout, &image).unwrap();
        e.put_str(BUF, "redirected %d\n");
        e.call("printf", &[BUF, 7]);
        e.put_str(BUF, "puts line");
        e.call("puts", &[BUF]);
        assert_eq!(String::from_utf8(captured.lock().unwrap().clone()).unwrap(), "redirected 7\nputs line\n");
        assert_eq!(e.out.text(), "", "nothing reached the host stdout");
    }

    #[test]
    fn descriptors_closed_behind_the_streams_back_are_reported() {
        let (mut e, _tmp) = fs_env();
        let fp = open(&mut e, "gone.txt", "w+");
        let fd = e.mem().read_u16(fp + file::FILE_NO).unwrap() as u32;
        assert_eq!(e.call("close", &[fd]).r0, 0);
        e.set_errno(0);
        assert_eq!(e.call("ftell", &[fp]).r0, NEG1);
        assert_eq!(e.errno(), 9);
        assert_eq!(e.call("fclose", &[fp]).r0, NEG1);
        assert_eq!(e.errno(), 9);
        // fgetc on it reports the read error through the flags.
        let fp = open(&mut e, "gone.txt", "r");
        let fd = e.mem().read_u16(fp + file::FILE_NO).unwrap() as u32;
        e.call("close", &[fd]);
        assert_eq!(e.call("fgetc", &[fp]).r0, NEG1);
        assert_ne!(flags(&e, fp) & file::SERR, 0);
        // Writing through a stale descriptor fails.
        let fp = open(&mut e, "gone.txt", "w");
        let fd = e.mem().read_u16(fp + file::FILE_NO).unwrap() as u32;
        e.call("close", &[fd]);
        e.put_str(BUF, "x");
        assert_eq!(e.call("fputs", &[BUF, fp]).r0, NEG1);
        assert_ne!(flags(&e, fp) & file::SERR, 0);
    }

    #[test]
    fn funopen_with_both_directions_and_without_memory() {
        let mut e = Env::new();
        let cb = e.cpu.register_shared_thunk("cb", Arc::new(ret_zero)).unwrap();
        let rw = e.call("funopen", &[0, cb, cb, 0, 0]).r0;
        e.put_str(BUF, "x");
        assert_eq!(e.call("fputs", &[BUF, rw]).r0, NEG1, "the callback wrote nothing");
        assert_eq!(e.call("fgetc", &[rw]).r0, NEG1, "and read nothing");
        assert_eq!(flags(&e, rw) & 0x1c, file::SRW);
        // Out of heap: funopen itself, a write callback's scratch buffer, a read callback's buffer.
        let mut s = Env::small();
        let scb = s.cpu.register_shared_thunk("cb", Arc::new(ret_zero)).unwrap();
        let w = s.call("funopen", &[0, 0, scb, 0, 0]).r0;
        let r = s.call("funopen", &[0, scb, 0, 0, 0]).r0;
        s.put_str(BUF, "data");
        while s.call("malloc", &[16]).r0 != 0 {}
        s.set_errno(0);
        assert_eq!(s.call("funopen", &[0, scb, 0, 0, 0]).r0, 0);
        assert_eq!(s.errno(), 12);
        assert_eq!(s.call("fputs", &[BUF, w]).r0, NEG1, "no scratch buffer for the callback");
        assert_eq!(s.call("fgetc", &[r]).r0, NEG1);
        assert_eq!(s.errno(), 12);
    }
}

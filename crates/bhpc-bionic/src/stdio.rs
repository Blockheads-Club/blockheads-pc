//! `<stdio.h>`: the three standard streams (`__sF`) mapped to the host's stdout/stderr, the `printf`
//! family on top of [`crate::fmt`], and the bionic stdio internals (`__swbuf`, `__srget`, `__srefill`)
//! that the `putc`/`getc` macros call. Files other than the standard streams are not supported yet:
//! operations on them fail with `EBADF`.

use crate::fmt::{self, FmtError, FmtSource};
use crate::guest::{cstr, rd};
use crate::layout::{errno, file};
use crate::misc::{fail, set_errno};
use crate::string::error_text;
use crate::{lock, Impl, Shared};
use bhpc_cpu::{ArgCursor, Cpu, Stop};
use std::io::Write;

/// Which standard stream a `FILE*` is (0 stdin, 1 stdout, 2 stderr).
fn stream_of(st: &Shared, fp: u32) -> Option<u32> {
    let sf = lock(st).data["__sF"];
    let off = fp.wrapping_sub(sf);
    (off < 3 * file::SIZE && off % file::SIZE == 0).then_some(off / file::SIZE)
}

/// Write to the standard stream `fp`; false (with `errno` set) for stdin and foreign `FILE`s.
fn write_stream(c: &mut Cpu, st: &Shared, fp: u32, data: &[u8]) -> bool {
    let ok = match stream_of(st, fp) {
        Some(1) => {
            let mut s = lock(st);
            let _ = s.stdout.write_all(data);
            true
        }
        Some(2) => {
            let mut s = lock(st);
            let _ = s.stderr.write_all(data);
            true
        }
        _ => false,
    };
    if !ok {
        set_errno(c, st, errno::EBADF);
    }
    ok
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

// ---------- input: stdin is always at EOF ----------

fn eof(c: &mut Cpu, _: &Shared) {
    minus1(c);
}

fn null_or_zero(c: &mut Cpu, _: &Shared) {
    c.ret_u32(0);
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
    ("fgetc", eof),
    ("getc", eof),
    ("getchar", eof),
    ("ungetc", eof),
    ("__srget", eof),
    ("__srefill", eof),
    ("fgets", null_or_zero),
    ("fread", null_or_zero),
    ("setvbuf", setvbuf),
    ("setbuf", nothing),
    ("clearerr", nothing),
];

#[cfg(test)]
mod tests {
    use crate::layout::file;
    use crate::testenv::{Env, SCRATCH};
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

//! `malloc` family over the guest heap. (The game bundles jemalloc in `libv`, which sits on `mmap`;
//! its `__wrap_malloc` & co. do not reach these functions, but `calloc`/`free` are imported by
//! `libv` itself, and constructors of foreign code may use all of them.)

use crate::layout::errno;
use crate::misc::set_errno;
use crate::{lock, Impl, Shared};
use bhpc_cpu::{Cpu, Stop};
use bhpc_mem::GuestHeap;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Run `f` on the heap. `GuestHeap` asserts on pointers it does not own (a guest bug); that panic
/// is turned into a guest stop instead of unwinding through the JIT.
fn with_heap<R>(c: &mut Cpu, st: &Shared, what: &str, f: impl FnOnce(&mut GuestHeap) -> R) -> Option<R> {
    let mut s = lock(st);
    let r = catch_unwind(AssertUnwindSafe(|| f(&mut s.heap)));
    drop(s);
    if r.is_err() {
        c.request_stop(Stop::Host(format!("{what}: pointer not owned by the guest heap")));
    }
    r.ok()
}

fn ret_alloc(c: &mut Cpu, st: &Shared, size: u32, p: u32) {
    if p == 0 && size != 0 {
        set_errno(c, st, errno::ENOMEM);
    }
    c.ret_u32(p);
}

fn malloc(c: &mut Cpu, st: &Shared) {
    let n = c.args().u32(c);
    let p = lock(st).heap.alloc(n);
    ret_alloc(c, st, n, p);
}

fn calloc(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (n, s) = (a.u32(c), a.u32(c));
    let p = lock(st).heap.calloc(n, s);
    ret_alloc(c, st, n.max(1).saturating_mul(s.max(1)), p);
}

fn realloc(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (p, n) = (a.u32(c), a.u32(c));
    if let Some(q) = with_heap(c, st, "realloc", |h| h.realloc(p, n)) {
        ret_alloc(c, st, n, q);
    }
}

fn free(c: &mut Cpu, st: &Shared) {
    let p = c.args().u32(c);
    with_heap(c, st, "free", |h| h.free(p));
}

fn memalign(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (align, n) = (a.u32(c), a.u32(c));
    let p = lock(st).heap.memalign(align, n);
    ret_alloc(c, st, n.max(1), p);
}

fn valloc(c: &mut Cpu, st: &Shared) {
    let n = c.args().u32(c);
    let p = lock(st).heap.memalign(crate::layout::PAGE_SIZE, n);
    ret_alloc(c, st, n.max(1), p);
}

fn posix_memalign(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (out, align, n) = (a.u32(c), a.u32(c), a.u32(c));
    if !align.is_power_of_two() || align < 4 {
        return c.ret_u32(errno::EINVAL);
    }
    let p = lock(st).heap.memalign(align, n);
    if p == 0 {
        return c.ret_u32(errno::ENOMEM);
    }
    if crate::guest::wr32(c, out, p) {
        c.ret_u32(0);
    }
}

fn malloc_usable_size(c: &mut Cpu, st: &Shared) {
    let p = c.args().u32(c);
    if p == 0 {
        return c.ret_u32(0);
    }
    if let Some(n) = with_heap(c, st, "malloc_usable_size", |h| h.usable_size(p)) {
        c.ret_u32(n);
    }
}

fn strdup(c: &mut Cpu, st: &Shared) {
    let s = c.args().u32(c);
    let Some(b) = crate::guest::cstr(c, s) else { return };
    dup_bytes(c, st, &b);
}

fn strndup(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (s, n) = (a.u32(c), a.u32(c));
    let Some((b, _)) = crate::guest::cstr_n(c, s, n) else { return };
    dup_bytes(c, st, &b);
}

fn dup_bytes(c: &mut Cpu, st: &Shared, b: &[u8]) {
    let p = lock(st).heap.alloc(b.len() as u32 + 1);
    if p == 0 {
        return fail_ptr(c, st);
    }
    if crate::guest::wr_cstr(c, p, b) {
        c.ret_u32(p);
    }
}

fn fail_ptr(c: &mut Cpu, st: &Shared) {
    set_errno(c, st, errno::ENOMEM);
    c.ret_u32(0);
}

pub(crate) const TABLE: &[(&str, Impl)] = &[
    ("malloc", malloc),
    ("calloc", calloc),
    ("realloc", realloc),
    ("free", free),
    ("memalign", memalign),
    ("valloc", valloc),
    ("posix_memalign", posix_memalign),
    ("malloc_usable_size", malloc_usable_size),
    ("strdup", strdup),
    ("strndup", strndup),
];

#[cfg(test)]
mod tests {
    use crate::testenv::{Env, HEAP_BASE, SCRATCH};
    use bhpc_cpu::Stop;

    #[test]
    fn allocation_family() {
        let mut e = Env::new();
        let p = e.call("malloc", &[24]).r0;
        assert!(p >= HEAP_BASE);
        assert_eq!(e.call("malloc_usable_size", &[p]).r0, 32);
        assert_eq!(e.call("malloc_usable_size", &[0]).r0, 0);
        e.mem().write_u32(p, 0xfeed).unwrap();
        let q = e.call("realloc", &[p, 4096]).r0;
        assert_eq!(e.mem().read_u32(q).unwrap(), 0xfeed);
        e.call("free", &[q]);
        e.call("free", &[0]);
        let z = e.call("calloc", &[4, 4]).r0;
        assert_eq!(e.mem().read_bytes(z, 16).unwrap(), vec![0; 16]);
        assert!(e.call("realloc", &[0, 8]).r0 >= HEAP_BASE);
        let m = e.call("memalign", &[256, 10]).r0;
        assert_eq!(m % 256, 0);
        let v = e.call("valloc", &[10]).r0;
        assert_eq!(v % 4096, 0);
        assert_eq!(e.call("posix_memalign", &[SCRATCH, 64, 100]).r0, 0);
        assert_eq!(e.mem().read_u32(SCRATCH).unwrap() % 64, 0);
        assert_eq!(e.call("posix_memalign", &[SCRATCH, 3, 100]).r0, 22);
        assert_eq!(e.call("posix_memalign", &[SCRATCH, 2, 100]).r0, 22);
        assert_eq!(e.call("posix_memalign", &[SCRATCH, 64, 0x7fff_ffff]).r0, 12);
        assert!(matches!(e.try_call("posix_memalign", &[0x10, 64, 8]), Err(Stop::Fault(_))));
    }

    #[test]
    fn out_of_memory_sets_errno() {
        let mut e = Env::new();
        assert_eq!(e.call("malloc", &[0x7fff_ffff]).r0, 0);
        assert_eq!(e.errno(), 12);
        e.set_errno(0);
        assert_eq!(e.call("calloc", &[0x10000, 0x10000]).r0, 0);
        assert_eq!(e.errno(), 12);
        e.set_errno(0);
        assert_eq!(e.call("realloc", &[0, 0x7fff_ffff]).r0, 0);
        assert_eq!(e.errno(), 12);
        e.set_errno(0);
        assert_eq!(e.call("memalign", &[64, 0x7fff_ffff]).r0, 0);
        assert_eq!(e.errno(), 12);
        assert_eq!(e.call("valloc", &[0x7fff_ffff]).r0, 0);
        e.put_str(SCRATCH, "x");
        assert!(e.call("strdup", &[SCRATCH]).r0 != 0);
        // Exhaust a small heap, then strdup must fail with ENOMEM.
        let mut e = Env::small();
        e.put_str(SCRATCH, "x");
        while e.call("malloc", &[1 << 16]).r0 != 0 {}
        while e.call("malloc", &[16]).r0 != 0 {}
        e.set_errno(0);
        assert_eq!(e.call("strdup", &[SCRATCH]).r0, 0);
        assert_eq!(e.errno(), 12);
    }

    #[test]
    fn bad_pointers_stop_the_guest_instead_of_panicking() {
        let mut e = Env::new();
        for f in ["free", "malloc_usable_size"] {
            let r = e.try_call(f, &[SCRATCH + 0x100]);
            assert!(matches!(r, Err(Stop::Host(ref m)) if m.contains("not owned")), "{f}: {r:?}");
        }
        let r = e.try_call("realloc", &[SCRATCH + 0x100, 8]);
        assert!(matches!(r, Err(Stop::Host(_))));
        // The state mutex is still usable afterwards.
        assert!(e.call("malloc", &[8]).r0 >= HEAP_BASE);
    }

    #[test]
    fn strdup_and_strndup() {
        let mut e = Env::new();
        e.put_str(SCRATCH, "hello");
        let d = e.call("strdup", &[SCRATCH]).r0;
        assert_eq!(e.mem().read_cstr(d).unwrap(), "hello");
        let d = e.call("strndup", &[SCRATCH, 3]).r0;
        assert_eq!(e.mem().read_cstr(d).unwrap(), "hel");
        let d = e.call("strndup", &[SCRATCH, 30]).r0;
        assert_eq!(e.mem().read_cstr(d).unwrap(), "hello");
        assert!(matches!(e.try_call("strdup", &[0x10]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("strndup", &[0x10, 3]), Err(Stop::Fault(_))));
        let d = e.call("strdup", &[SCRATCH]).r0;
        // Free + duplicate again reuses memory.
        e.call("free", &[d]);
    }
}

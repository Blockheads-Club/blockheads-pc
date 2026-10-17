//! Host functions used only by our own test fixture (`fixtures/hello.so`) and the unwinder hook
//! libc++abi needs: `bhpc_*` helpers and `__gnu_Unwind_Find_exidx`.

use crate::{lock, Impl, Shared};
use bhpc_cpu::Cpu;
use std::io::Write;

fn bhpc_check(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (n, ok) = (a.u32(c), a.u32(c));
    let name = c.mem().read_cstr(n).unwrap_or_default();
    let mut s = lock(st);
    let _ = writeln!(s.stdout, "  [{}] {name}", if ok != 0 { "ok" } else { "FAIL" });
    s.checks.push((name, ok != 0));
}

fn bhpc_hypot(c: &mut Cpu, _: &Shared) {
    let mut a = c.args();
    let (x, y) = (a.f64(c), a.f64(c));
    c.ret_f64(x.hypot(y));
}

fn bhpc_mix(c: &mut Cpu, _: &Shared) {
    let mut a = c.args();
    let v: Vec<u32> = (0..6).map(|_| a.u32(c)).collect();
    let sum = v.iter().fold(0u32, |s, x| s.wrapping_add(*x));
    c.ret_u64((v[5] as u64) << 32 | sum as u64);
}

fn bhpc_nop(_: &mut Cpu, _: &Shared) {}

/// `__gnu_Unwind_Find_exidx(pc, &count)`: the `.ARM.exidx` table of the module containing `pc`.
fn find_exidx(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (pc, count_out) = (a.u32(c), a.u32(c));
    let found = lock(st).exidx.iter().find(|r| pc >= r.start && pc - r.start < r.size).copied();
    let (table, count) = found.map_or((0, 0), |r| (r.table, r.count));
    let _ = c.mem().write_u32(count_out, count);
    c.ret_u32(table);
}

pub(crate) const TABLE: &[(&str, Impl)] = &[
    ("bhpc_check", bhpc_check),
    ("bhpc_hypot", bhpc_hypot),
    ("bhpc_mix", bhpc_mix),
    ("bhpc_nop", bhpc_nop),
    ("__gnu_Unwind_Find_exidx", find_exidx),
    ("dl_unwind_find_exidx", find_exidx),
];

#[cfg(test)]
mod tests {
    use crate::state::ExidxRange;
    use crate::testenv::{Env, SCRATCH};

    #[test]
    fn check_hypot_mix_and_nop() {
        let mut e = Env::new();
        e.put_str(SCRATCH, "case");
        e.call("bhpc_check", &[SCRATCH, 1]);
        e.call("bhpc_check", &[SCRATCH, 0]);
        assert_eq!(e.out.text(), "  [ok] case\n  [FAIL] case\n");
        assert_eq!(crate::lock(&e.st).checks, vec![("case".to_string(), true), ("case".to_string(), false)]);
        let (a, b) = (3.0f64.to_bits(), 4.0f64.to_bits());
        let r = e.call("bhpc_hypot", &[a as u32, (a >> 32) as u32, b as u32, (b >> 32) as u32]);
        assert_eq!(r.as_f64(), 5.0);
        assert_eq!(e.call("bhpc_mix", &[1, 2, 3, 4, 5, 6]).as_u64(), 6 << 32 | 21);
        e.call("bhpc_nop", &[]);
    }

    #[test]
    fn unwind_table_lookup() {
        let mut e = Env::new();
        crate::lock(&e.st).exidx.push(ExidxRange { start: 0x1000_0000, size: 0x1000, table: 0x1000_0800, count: 9 });
        for name in ["__gnu_Unwind_Find_exidx", "dl_unwind_find_exidx"] {
            let r = e.call(name, &[0x1000_0010, SCRATCH]);
            assert_eq!((r.r0, e.mem().read_u32(SCRATCH).unwrap()), (0x1000_0800, 9));
            let r = e.call(name, &[0x2000_0000, SCRATCH]);
            assert_eq!((r.r0, e.mem().read_u32(SCRATCH).unwrap()), (0, 0));
        }
    }
}

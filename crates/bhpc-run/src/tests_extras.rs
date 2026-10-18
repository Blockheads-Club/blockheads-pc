//! The extra libc thunks, called through the CPU like guest code would.
use crate::extras::install;
use crate::new_env;

struct Rig {
    cpu: Box<bhpc_cpu::Cpu>,
    st: bhpc_bionic::Shared,
}

fn rig() -> Rig {
    let (cpu, st) = new_env(Box::new(std::io::sink()), None).unwrap();
    install(&cpu, &st);
    Rig { cpu, st }
}

fn alloc(r: &Rig, n: u32) -> u32 {
    bhpc_bionic::lock(&r.st).heap.calloc(1, n)
}

fn put(r: &Rig, s: &str) -> u32 {
    let p = alloc(r, s.len() as u32 + 1);
    r.cpu.mem().write_bytes(p, s.as_bytes()).unwrap();
    p
}

fn call(r: &mut Rig, name: &str, args: &[u32]) -> u32 {
    let a = r.cpu.thunk_addr(name).unwrap();
    r.cpu.call(a, args).unwrap().r0
}

#[test]
fn inet_pton_and_ntop_round_trip() {
    let mut r = rig();
    let (out, text) = (alloc(&r, 32), alloc(&r, 64));
    let ip = put(&r, "192.168.1.20");
    assert_eq!(call(&mut r, "inet_pton", &[2, ip, out]), 1);
    assert_eq!(r.cpu.mem().read_bytes(out, 4).unwrap(), [192, 168, 1, 20]);
    assert_eq!(call(&mut r, "inet_ntop", &[2, out, text, 64]), text);
    assert_eq!(r.cpu.mem().read_cstr(text).unwrap(), "192.168.1.20");
    let ip6 = put(&r, "::1");
    assert_eq!(call(&mut r, "inet_pton", &[10, ip6, out]), 1);
    assert_eq!(call(&mut r, "inet_ntop", &[10, out, text, 64]), text);
    assert_eq!(r.cpu.mem().read_cstr(text).unwrap(), "::1");
    // Failures: garbage text, unknown family, short buffer.
    let bad = put(&r, "not an address");
    assert_eq!(call(&mut r, "inet_pton", &[2, bad, out]), 0);
    assert_eq!(call(&mut r, "inet_pton", &[99, ip, out]), u32::MAX);
    assert_eq!(call(&mut r, "inet_ntop", &[99, out, text, 64]), 0);
    assert_eq!(call(&mut r, "inet_pton", &[2, ip, out]), 1);
    assert_eq!(call(&mut r, "inet_ntop", &[2, out, text, 4]), 0);
}

#[test]
fn small_internals() {
    let mut r = rig();
    // inet_ntoa takes the address by value (network byte order = memory order).
    let p = call(&mut r, "inet_ntoa", &[u32::from_le_bytes([10, 0, 0, 7])]);
    assert_eq!(r.cpu.mem().read_cstr(p).unwrap(), "10.0.0.7");
    assert_eq!(call(&mut r, "inet_ntoa", &[u32::from_le_bytes([1, 2, 3, 4])]), p, "static buffer");
    let h = call(&mut r, "__get_h_errno", &[]);
    assert_eq!(r.cpu.mem().read_u32(h).unwrap(), 0);
    assert_eq!(call(&mut r, "__get_h_errno", &[]), h);
    let inf = f64::INFINITY.to_bits();
    assert_eq!(call(&mut r, "__isinf", &[inf as u32, (inf >> 32) as u32]), 1);
    let one = 1.0f64.to_bits();
    assert_eq!(call(&mut r, "__isinf", &[one as u32, (one >> 32) as u32]), 0);
}

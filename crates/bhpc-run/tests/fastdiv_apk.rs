//! The hardware division routines against the game's own libgcc helpers (`libv.so`). Ignored by default: set
//! `BHPC_APK` to a user-owned Blockheads APK and run
//! `BHPC_APK=... cargo test -p bhpc-run --test fastdiv_apk -- --ignored --nocapture`.

use bhpc_run::apkrun::{InitConfig, Session};
use bhpc_run::fastdiv::{install, DISABLE_ENV, HELPERS};

fn inputs() -> Vec<(u32, u32)> {
    let edges = [1u32, 2, 3, 7, 10, 255, 0x7FFF_FFFF, 0x8000_0000, 0x8000_0001, 0xFFFF_FFFF, 0xFFFF_FFFE, 0xFFFF_FFF9];
    let mut v: Vec<(u32, u32)> =
        [0u32].iter().chain(&edges).flat_map(|&n| edges.iter().map(move |&d| (n, d))).collect();
    let mut x = 0x9E37_79B9u32;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x
    };
    for i in 0..20_000 {
        let (n, d) = (next(), next());
        let d = [d, d >> 24, d >> (i % 32), d & 0xFF][i % 4];
        if d != 0 {
            v.push((n, d));
        }
    }
    v
}

#[test]
#[ignore]
fn hardware_division_matches_libv() {
    let apk = std::env::var("BHPC_APK").expect("set BHPC_APK to a Blockheads APK");
    let dir = std::env::temp_dir().join(format!("bhpc-fastdiv-{}", std::process::id()));
    let mut cfg = InitConfig::new(apk.into());
    cfg.data_dir = Some(dir.clone());
    // Open unpatched (this test binary runs only this test, so setting the variable is not racy).
    std::env::set_var(DISABLE_ENV, "0");
    let mut sess = Session::open(&cfg, Box::new(std::io::sink())).expect("open APK");
    let libv = sess.linker.module("libv.so").expect("libv.so");
    let entries: Vec<(&str, u32)> = HELPERS.iter().filter_map(|(n, _)| Some((*n, libv.export(n)?))).collect();
    assert_eq!(entries.len(), HELPERS.len(), "libv exports every helper");
    let ins = inputs();
    let run = |sess: &mut Session, entry: u32| -> Vec<(u32, u32)> {
        ins.iter().map(|&(n, d)| sess.cpu.call(entry, &[n, d]).map(|r| (r.r0, r.r1)).expect("helper call")).collect()
    };
    let original: Vec<Vec<(u32, u32)>> = entries.iter().map(|&(_, e)| run(&mut sess, e)).collect();
    let libv = sess.linker.module("libv.so").expect("libv.so");
    let patched = install(sess.cpu.mem(), &|n| libv.export(n)).expect("install");
    assert_eq!(patched.len(), HELPERS.len(), "every helper is recognised: {patched:?}");
    sess.cpu.clear_cache();
    for (i, &(name, e)) in entries.iter().enumerate() {
        let fast = run(&mut sess, e);
        let divmod = name.ends_with("divmod");
        for (k, &(n, d)) in ins.iter().enumerate() {
            let (o, f) = (original[i][k], fast[k]);
            assert_eq!(o.0, f.0, "{name}({n:#x}, {d:#x}) result");
            if divmod {
                assert_eq!(o.1, f.1, "{name}({n:#x}, {d:#x}) remainder");
            }
        }
        eprintln!("{name}: {} inputs identical", ins.len());
    }
    let _ = std::fs::remove_dir_all(dir);
}

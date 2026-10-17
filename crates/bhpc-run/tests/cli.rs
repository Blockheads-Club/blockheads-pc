//! Runs the real `bhpc-run` binary (covers `main`).
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_bhpc-run"))
}

#[test]
fn binary_runs_the_fixture_and_prints_usage() {
    let fx = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/hello.so");
    let out = bin().arg(fx).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("13 checks, 0 failed"));
    let out = bin().output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage"));
}

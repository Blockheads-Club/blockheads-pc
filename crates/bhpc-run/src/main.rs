//! `bhpc-run`: run an ARM32 fixture `.so` (M0 spike).
//!
//! Usage: `bhpc-run <file.so> [--entry hello_main]` or `bhpc-run --bench-thunk [iterations]`.

use bhpc_run::{bench, run_shared_object};
use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ExitCode::from(run(&args, &mut std::io::stdout(), &mut std::io::stderr()))
}

fn run(args: &[String], stdout: &mut dyn std::io::Write, stderr: &mut dyn std::io::Write) -> u8 {
    run_with(args, stdout, stderr, &bench::measure)
}

type Measure<'a> = &'a dyn Fn(u32) -> Result<bench::ThunkTiming, bhpc_run::RunError>;

fn run_with(args: &[String], stdout: &mut dyn std::io::Write, stderr: &mut dyn std::io::Write, measure: Measure) -> u8 {
    match args.first().map(String::as_str) {
        Some("--bench-thunk") => {
            let n = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10_000_000);
            match measure(n) {
                Ok(t) => {
                    let _ = writeln!(
                        stdout,
                        "thunk round trip: {:.1} ns/call ({:.1} ns over a guest bx-lr call), {} iterations",
                        t.ns_per_call(),
                        t.ns_over_baseline(),
                        t.iterations
                    );
                    0
                }
                Err(e) => {
                    let _ = writeln!(stderr, "bhpc-run: {e}");
                    1
                }
            }
        }
        Some(path) if !path.starts_with("--") => {
            let entry = match args.iter().position(|a| a == "--entry") {
                Some(i) => args.get(i + 1).cloned().unwrap_or_default(),
                None => "hello_main".to_string(),
            };
            let bytes = match std::fs::read(path) {
                Ok(b) => b,
                Err(e) => {
                    let _ = writeln!(stderr, "bhpc-run: cannot read {path}: {e}");
                    return 2;
                }
            };
            let watchdog =
                args.iter().position(|a| a == "--watchdog").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok());
            let pc_file = args.iter().position(|a| a == "--trace-pc").and_then(|i| args.get(i + 1));
            let mut pc_out = pc_file.and_then(|p| std::fs::File::create(p).ok()).map(std::io::BufWriter::new);
            let mut trace_fn = |c: &bhpc_cpu::Cpu| {
                if let Some(w) = pc_out.as_mut() {
                    let _ = writeln!(w, "{:08x}", c.pc());
                }
            };
            let pc_trace: Option<&mut dyn FnMut(&bhpc_cpu::Cpu)> =
                if pc_file.is_some() { Some(&mut trace_fn) } else { None };
            match run_shared_object(
                &bytes,
                &entry,
                Box::new(std::io::stdout()),
                args.iter().any(|a| a == "--trace-thunks"),
                watchdog,
                pc_trace,
            ) {
                Ok(r) => {
                    let failed = r.checks.iter().filter(|(_, ok)| !ok).count();
                    let _ = writeln!(
                        stdout,
                        "{} checks, {failed} failed; {} relocations, {} thunks, {} constructors; {entry}() = {}",
                        r.checks.len(),
                        r.relocations,
                        r.thunks,
                        r.constructors,
                        r.exit_code
                    );
                    u8::from(r.exit_code != 0 || failed != 0)
                }
                Err(e) => {
                    let _ = writeln!(stderr, "bhpc-run: {e}");
                    1
                }
            }
        }
        _ => {
            let _ = writeln!(stderr, "usage: bhpc-run <file.so> [--entry SYMBOL] [--trace-thunks] [--watchdog CYCLES] [--trace-pc FILE] | --bench-thunk [N]");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn usage_on_bad_arguments() {
        let (mut o, mut e) = (Vec::new(), Vec::new());
        assert_eq!(run(&s(&[]), &mut o, &mut e), 2);
        assert_eq!(run(&s(&["--nope"]), &mut o, &mut e), 2);
        assert!(String::from_utf8(e).unwrap().contains("usage"));
    }

    #[test]
    fn bench_failure_is_reported() {
        let (mut o, mut e) = (Vec::new(), Vec::new());
        let failing = |_: u32| Err(bhpc_run::RunError::NoEntry("bench".into()));
        assert_eq!(run_with(&s(&["--bench-thunk"]), &mut o, &mut e, &failing), 1);
        assert!(String::from_utf8(e).unwrap().contains("bhpc-run: module does not export `bench`"));
    }

    #[test]
    fn missing_file_is_reported() {
        let (mut o, mut e) = (Vec::new(), Vec::new());
        assert_eq!(run(&s(&["/nonexistent/x.so"]), &mut o, &mut e), 2);
        assert!(String::from_utf8(e).unwrap().contains("cannot read"));
    }

    #[test]
    fn bench_mode_prints_timing() {
        let (mut o, mut e) = (Vec::new(), Vec::new());
        assert_eq!(run(&s(&["--bench-thunk", "5000"]), &mut o, &mut e), 0);
        assert!(String::from_utf8(o).unwrap().contains("ns/call"));
        // Default iteration count is used when none is given or parsing fails; keep it cheap by not running it.
    }

    #[test]
    fn pc_trace_and_watchdog_options() {
        let fx = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/hello.so");
        let trace = std::env::temp_dir().join(format!("bhpc-pc-trace-{}.txt", std::process::id()));
        let (mut o, mut e) = (Vec::new(), Vec::new());
        let t = trace.to_string_lossy().to_string();
        let code = run(&s(&[fx, "--trace-pc", &t, "--watchdog", "100000000000"]), &mut o, &mut e);
        assert_eq!((code, String::from_utf8_lossy(&e).into_owned()), (0, String::new()));
        let lines = std::fs::read_to_string(&trace).unwrap().lines().count();
        std::fs::remove_file(&trace).unwrap();
        assert!(lines > 1000);
        // A tiny watchdog budget aborts the run.
        let (mut o2, mut e2) = (Vec::new(), Vec::new());
        assert_eq!(run(&s(&[fx, "--watchdog", "50"]), &mut o2, &mut e2), 1);
        assert!(String::from_utf8(e2).unwrap().contains("watchdog"));
    }

    #[test]
    fn runs_fixture_and_reports_entry_errors() {
        let fx = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/hello.so");
        let (mut o, mut e) = (Vec::new(), Vec::new());
        assert_eq!(run(&s(&[fx]), &mut o, &mut e), 0, "{}", String::from_utf8_lossy(&e));
        assert!(String::from_utf8(o.clone()).unwrap().contains("0 failed"));
        assert_eq!(run(&s(&[fx, "--entry", "nope"]), &mut o, &mut e), 1);
        assert!(String::from_utf8(e.clone()).unwrap().contains("does not export"));
        // A non-ELF file fails to load.
        let (mut o2, mut e2) = (Vec::new(), Vec::new());
        assert_eq!(run(&s(&[concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")]), &mut o2, &mut e2), 1);
    }
}

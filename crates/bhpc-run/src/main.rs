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
    // `--config FILE` (the launcher's `launcher.toml`) stands for --apk/--data-dir/--window/... .
    match bhpc_launcher::config::expand_args(args) {
        Ok(args) => run_with(&args, stdout, stderr, &bench::measure),
        Err(e) => {
            let _ = writeln!(stderr, "bhpc-run: {e}");
            2
        }
    }
}

type Measure<'a> = &'a dyn Fn(u32) -> Result<bench::ThunkTiming, bhpc_run::RunError>;

fn run_with(args: &[String], stdout: &mut dyn std::io::Write, stderr: &mut dyn std::io::Write, measure: Measure) -> u8 {
    if args.iter().any(|a| a == "--apk") {
        return run_apk(args, stdout, stderr);
    }
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

fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1))
}

/// `--apk PATH --init-only [--force] [--trace-thunks] [--trace-pc FILE] [--watchdog CYCLES] [--keep-going] [-v]`.
fn run_apk(args: &[String], stdout: &mut dyn std::io::Write, stderr: &mut dyn std::io::Write) -> u8 {
    let Some(path) = opt(args, "--apk") else {
        let _ = writeln!(stderr, "bhpc-run: --apk needs a path");
        return 2;
    };
    let mut cfg = bhpc_run::apkrun::InitConfig::new(path.into());
    // Start-up progress lines for the launcher; a plain run prints none.
    if args.iter().any(|a| a == "--progress" || a == "--embed") {
        bhpc_launcher::progress::enable();
        // At once: video, audio and controller setup come first and take seconds, and the loading screen should
        // show the start has begun.
        bhpc_launcher::progress::report(bhpc_launcher::progress::Phase::Apk, 0, 1);
    }
    cfg.force = args.iter().any(|a| a == "--force");
    cfg.trace_thunks = args.iter().any(|a| a == "--trace-thunks");
    cfg.trace_fs = args.iter().any(|a| a == "--trace-fs");
    cfg.trace_jni = args.iter().any(|a| a == "--trace-jni");
    cfg.trace_jni_calls = args.iter().any(|a| a == "--trace-jni-calls");
    cfg.keep_going = args.iter().any(|a| a == "--keep-going");
    cfg.verbose = args.iter().any(|a| a == "-v" || a == "--verbose");
    cfg.parallel_guest = bhpc_run::apkrun::parallel_guest(args, std::env::var("BHPC_PARALLEL_GUEST").ok().as_deref());
    cfg.pc_trace = opt(args, "--trace-pc").map(Into::into);
    cfg.data_dir = opt(args, "--data-dir").map(Into::into);
    if let Some(p) = opt(args, "--package") {
        cfg.package = p.clone();
    }
    if let Some(w) = opt(args, "--watchdog").and_then(|v| v.parse().ok()) {
        cfg.watchdog_cycles = w;
    }
    let top = opt(args, "--top").and_then(|v| v.parse().ok()).unwrap_or(25);
    if !args.iter().any(|a| a == "--init-only") {
        return play_apk(args, cfg, stderr);
    }
    match bhpc_run::apkrun::init_only(&cfg, Box::new(std::io::stdout())) {
        Ok(r) => {
            let _ = bhpc_run::apkrun::print_report(&r, top, stdout);
            u8::from(!r.ok())
        }
        Err(e) => {
            let _ = writeln!(stderr, "bhpc-run: {e}");
            1
        }
    }
}

/// `--apk PATH [--window WxH] [--shot FILE] [--shot-at N] [--exit-after-shot] [--seconds N] [--check-gl]`.
#[cfg(feature = "play")]
fn play_apk(args: &[String], init: bhpc_run::apkrun::InitConfig, stderr: &mut dyn std::io::Write) -> u8 {
    use bhpc_run::play::{play, PlayConfig};
    let mut cfg = PlayConfig::new(init);
    // Camera photos land in the user's Pictures folder; `--open-uri` hands the game a link (a join link).
    cfg.init.pictures = bhpc_run::desktop::user_pictures_dir();
    cfg.open_uri = opt(args, "--open-uri").cloned();
    if let Some((w, h)) = opt(args, "--window").and_then(|v| v.split_once('x')) {
        cfg.window = (w.parse().unwrap_or(1024), h.parse().unwrap_or(768));
    }
    match bhpc_run::embed::parse_play_flags(args) {
        Ok(f) => {
            cfg.fullscreen = f.fullscreen;
            cfg.vsync = f.vsync;
            cfg.title = f.title.unwrap_or(cfg.title);
            cfg.bindings = f.bindings;
            cfg.embed = f.embed;
            cfg.fps_limit = f.fps_limit;
            cfg.show_fps = f.show_fps;
            cfg.fps_position = f.fps_position;
            cfg.unlock_fps = f.unlock_fps;
            cfg.game_speed = f.game_speed;
            cfg.zoom = f.zoom;
            cfg.ui_scale = f.ui_scale;
            cfg.controller = f.controller;
            cfg.hide_dpad = f.hide_dpad;
            cfg.pad_tile_cursor = f.pad_tile_cursor;
            cfg.lang = f.lang;
        }
        Err(e) => {
            let _ = writeln!(stderr, "bhpc-run: {e}");
            return 2;
        }
    }
    // The client mod loader: on unless `--no-mods`; files below the data folder.
    if !args.iter().any(|a| a == "--no-mods") {
        let data = bhpc_run::apkrun::data_dirs(cfg.init.data_dir.as_deref(), bhpc_vfs::DataDirs::platform_default());
        cfg.mods = Some(bhpc_run::modglue::ModsOptions {
            data_dir: data.root,
            dev: args
                .iter()
                .enumerate()
                .filter(|(_, a)| *a == "--mods-dev")
                .filter_map(|(i, _)| args.get(i + 1))
                .map(Into::into)
                .collect(),
            block_native: args.iter().any(|a| a == "--mods-block-native"),
            join: opt(args, "--join").cloned(),
            auto_accept: args.iter().any(|a| a == "--mods-auto-accept"),
            jvm: args.iter().any(|a| a == "--mods-jvm"),
            java_home: opt(args, "--mods-java-home").map(Into::into),
            features: Vec::new(),
        });
    }
    cfg.control = args.iter().any(|a| a == "--control");
    cfg.shot = opt(args, "--shot").map(Into::into);
    if let Some(n) = opt(args, "--shot-at").and_then(|v| v.parse().ok()) {
        cfg.shot_at = n;
    }
    cfg.exit_after_shot = args.iter().any(|a| a == "--exit-after-shot");
    cfg.seconds = opt(args, "--seconds").and_then(|v| v.parse().ok());
    cfg.check_gl = args.iter().any(|a| a == "--check-gl");
    let mut clicks = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if a == "--click" {
            // X,Y@MS
            clicks.extend(args.get(i + 1).and_then(|v| {
                let (xy, ms) = v.split_once('@')?;
                let (x, y) = xy.split_once(',')?;
                Some((x.parse().ok()?, y.parse().ok()?, ms.parse().ok()?))
            }));
        }
    }
    let file = opt(args, "--script").map(std::fs::read_to_string);
    let file = match file.transpose() {
        Ok(f) => f,
        Err(e) => {
            let _ = writeln!(stderr, "bhpc-run: cannot read the script: {e}");
            return 2;
        }
    };
    cfg.script = match bhpc_run::script::build(file.as_deref(), &clicks) {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(stderr, "bhpc-run: bad script: {e}");
            return 2;
        }
    };
    if let Some(d) = opt(args, "--shot-dir") {
        cfg.shot_dir = d.into();
    }
    cfg.alert_choice = opt(args, "--alert-choice").and_then(|v| v.parse().ok());
    cfg.auto_alerts = cfg.alert_choice.is_some() || args.iter().any(|a| a == "--auto-alerts");
    cfg.trace_objc = args.iter().any(|a| a == "--trace-objc");
    if let Some(list) = opt(args, "--trace-fn") {
        cfg.trace_fn = list.split(',').map(str::to_string).collect();
    }
    cfg.jni_report = opt(args, "--jni-report").map(Into::into);
    if let Some(list) = opt(args, "--symbolize") {
        cfg.symbolize =
            list.split(',').filter_map(|a| u32::from_str_radix(a.trim_start_matches("0x"), 16).ok()).collect();
    }
    match play(&cfg) {
        Ok(r) => {
            let _ = writeln!(stderr, "[play] ended: {} ({} frames presented)", r.stop, r.swaps);
            for l in &r.backtrace {
                let _ = writeln!(stderr, "  {l}");
            }
            for s in &r.java_stubs {
                let _ = writeln!(stderr, "  java stub called: {s}");
            }
            for (n, c) in &r.gl_unsupported {
                let _ = writeln!(stderr, "  unsupported GL call: {n} x{c}");
            }
            for n in &r.unimplemented_hits {
                let _ = writeln!(stderr, "  unimplemented import hit: {n}");
            }
            for (n, c) in r.top_thunks.iter().take(15).chain(r.top_thunks.iter().skip(15).filter(|(n, _)| {
                [
                    "sock", "connect", "getaddr", "gethost", "select", "poll", "epoll", "send", "recv", "bind",
                    "listen", "fcntl", "pipe", "kqueue", "open", "stat",
                ]
                .iter()
                .any(|k| n.contains(k))
            })) {
                let _ = writeln!(stderr, "  thunk {c:>10}  {n}");
            }
            u8::from(r.swaps == 0)
        }
        Err(e) => {
            let _ = writeln!(stderr, "bhpc-run: {e}");
            e.exit_code()
        }
    }
}

#[cfg(not(feature = "play"))]
fn play_apk(_args: &[String], _init: bhpc_run::apkrun::InitConfig, stderr: &mut dyn std::io::Write) -> u8 {
    let _ = writeln!(stderr, "bhpc-run: running the game needs a build with `--features play` (or pass --init-only)");
    2
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

    /// A throw-away APK with one `libApplication.so` whose `DT_INIT` is `code`.
    fn apk_with(code: &[u32]) -> (std::path::PathBuf, std::path::PathBuf) {
        use bhpc_loader::testelf::{build_zip, Builder};
        let mut b = Builder { scratch: code.to_vec(), ..Default::default() };
        let at = b.build().scratch_vaddr;
        b.init = Some(at);
        let lib = b.build().bytes;
        let dir = std::env::temp_dir().join(format!("bhpc-main-{}-{}", std::process::id(), code[0]));
        std::fs::create_dir_all(&dir).unwrap();
        let apk = dir.join("g.apk");
        std::fs::write(&apk, build_zip(&[("lib/armeabi-v7a/libApplication.so", &lib, false)])).unwrap();
        (apk, dir)
    }

    #[test]
    fn apk_mode_validates_its_arguments() {
        let (mut o, mut e) = (Vec::new(), Vec::new());
        assert_eq!(run(&s(&["--apk"]), &mut o, &mut e), 2);
        assert!(String::from_utf8_lossy(&e).contains("--apk needs a path"));
        let (mut o, mut e) = (Vec::new(), Vec::new());
        assert_eq!(run(&s(&["--apk", "x.apk"]), &mut o, &mut e), 2);
        assert!(String::from_utf8_lossy(&e).contains("--features play"));
        let (mut o, mut e) = (Vec::new(), Vec::new());
        assert_eq!(run(&s(&["--apk", "/nonexistent/x.apk", "--init-only"]), &mut o, &mut e), 1);
        assert!(String::from_utf8_lossy(&e).contains("bhpc-run:"));
    }

    #[test]
    fn config_file_stands_for_the_flags() {
        let (apk, dir) = apk_with(&[0xE320_F000, 0xE12F_FF1E]);
        let mut cfg = bhpc_launcher::LaunchConfig::new(&apk, dir.join("cfgdata"));
        cfg.force = true;
        let file = dir.join("launcher.toml");
        cfg.save(&file).unwrap();
        let (mut o, mut e) = (Vec::new(), Vec::new());
        let args = s(&["--config", &file.to_string_lossy(), "--init-only"]);
        assert_eq!(run(&args, &mut o, &mut e), 0, "{}", String::from_utf8_lossy(&e));
        assert!(String::from_utf8(o).unwrap().contains("constructors: 1/1 returned"));
        assert!(dir.join("cfgdata").exists());
        let (mut o, mut e) = (Vec::new(), Vec::new());
        assert_eq!(run(&s(&["--config", &dir.join("none.toml").to_string_lossy()]), &mut o, &mut e), 2);
        assert!(String::from_utf8_lossy(&e).contains("does not exist"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn apk_mode_runs_constructors_and_reports() {
        let (apk, dir) = apk_with(&[0xE12F_FF1E]);
        let apk = apk.to_string_lossy().to_string();
        let trace = dir.join("pc.txt").to_string_lossy().to_string();
        let data = dir.join("data").to_string_lossy().to_string();
        let (mut o, mut e) = (Vec::new(), Vec::new());
        let args = s(&[
            "--apk",
            &apk,
            "--init-only",
            "--force",
            "--trace-thunks",
            "--keep-going",
            "-v",
            "--trace-fs",
            "--trace-jni",
            "--trace-pc",
            &trace,
            "--data-dir",
            &data,
            "--progress",
            "--package",
            "org.example.g",
            "--watchdog",
            "1000000000",
            "--top",
            "3",
        ]);
        assert_eq!(run(&args, &mut o, &mut e), 0, "{}", String::from_utf8_lossy(&e));
        let out = String::from_utf8(o).unwrap();
        assert!(out.contains("constructors: 1/1 returned") && out.contains("thunk calls (top 3"), "{out}");
        assert!(std::path::Path::new(&trace).exists());
        // A constructor that dies gives exit code 1 and a diagnosis.
        let (bad, dir2) = apk_with(&[0xE7F0_00F0]);
        let (mut o, mut e) = (Vec::new(), Vec::new());
        let args = s(&["--apk", &bad.to_string_lossy(), "--init-only", "--force"]);
        assert_eq!(run(&args, &mut o, &mut e), 1);
        assert!(String::from_utf8(o).unwrap().contains("FAILED libApplication.so"));
        // Without --force the unknown build is refused.
        let (mut o, mut e) = (Vec::new(), Vec::new());
        assert_eq!(run(&s(&["--apk", &bad.to_string_lossy(), "--init-only"]), &mut o, &mut e), 1);
        assert!(String::from_utf8_lossy(&e).contains("unknown Blockheads build"));
        let _ = (std::fs::remove_dir_all(dir), std::fs::remove_dir_all(dir2));
    }
}

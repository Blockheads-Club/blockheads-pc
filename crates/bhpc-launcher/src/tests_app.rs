//! Tests for the app-facing modules: APK report, prefs, data folder helpers, config expansion, session.

use crate::apk::{self, ApkProblem, ApkStatus};
use crate::config::{expand_args, LaunchConfig, LauncherError};
use crate::datadir::{copy_dir, dir_size, keep_private_copy};
use crate::import::{import_saves, Existing};
use crate::keys::{Binding, Key};
use crate::prefs::{Paths, Prefs};
use crate::session::{sidecar_path, GameState, LogBuffer, Session, SessionError};
use bhpc_loader::apk::{sha256_hex, ApkError, Release};
use bhpc_vfs::DataDirs;
use std::io::Write;
use std::path::Path;

fn make_zip(path: &Path, files: &[(&str, &[u8])]) {
    let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (n, d) in files {
        z.start_file(*n, opts).unwrap();
        z.write_all(d).unwrap();
    }
    z.finish().unwrap();
}

#[test]
fn apk_report_classes() {
    let t = tempfile::tempdir().unwrap();
    let lib = b"synthetic libApplication";
    let good = t.path().join("good.apk");
    make_zip(&good, &[("lib/armeabi-v7a/libApplication.so", lib)]);
    let hash = sha256_hex(lib);
    let table = [(Release::V1_7_5, hash.as_str())];

    let r = apk::check_with(&good, &table, false);
    assert_eq!((r.status, r.version.as_deref(), r.problem), (ApkStatus::Supported, Some("1.7.5"), None));
    assert!(r.size_bytes > 0 && r.path.ends_with("good.apk"));

    // Unknown build: refused, or accepted as "unknown" when allowed.
    let r = apk::check(&good, false);
    assert_eq!((r.status, r.problem), (ApkStatus::Error, Some(ApkProblem::Unsupported)));
    assert!(r.detail.contains(&hash));
    let r = apk::check(&good, true);
    assert_eq!((r.status, r.version, r.problem), (ApkStatus::Unknown, None, None));

    // Another app, not a zip, damaged, unreadable.
    let other = t.path().join("other.apk");
    make_zip(&other, &[("classes.dex", b"x")]);
    assert_eq!(apk::check(&other, false).problem, Some(ApkProblem::NotBlockheads));
    let junk = t.path().join("junk.apk");
    std::fs::write(&junk, b"not a zip at all").unwrap();
    assert_eq!(apk::check(&junk, false).problem, Some(ApkProblem::NotBlockheads));
    let cut = t.path().join("cut.apk");
    let bytes = std::fs::read(&good).unwrap();
    std::fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
    assert!(matches!(apk::check(&cut, false).problem, Some(ApkProblem::Damaged | ApkProblem::NotBlockheads)));
    let gone = apk::check(&t.path().join("gone.apk"), false);
    assert_eq!((gone.problem, gone.size_bytes), (Some(ApkProblem::Unreadable), 0));

    for (e, p) in [
        (ApkError::Truncated, ApkProblem::Damaged),
        (ApkError::Corrupt("x".into()), ApkProblem::Damaged),
        (ApkError::Unsupported("zip64"), ApkProblem::Damaged),
        (ApkError::Missing("x".into()), ApkProblem::NotBlockheads),
    ] {
        assert_eq!(apk::problem_of(&e), p);
    }
}

#[test]
fn arrow_aliases_parse() {
    assert_eq!(Binding::parse("ArrowUp").unwrap().to_string(), "Up");
    assert_eq!(Binding::parse("arrowdown").unwrap().to_string(), "Down");
    assert_eq!(Key::Up.to_string(), "Up");
}

#[test]
fn prefs_roundtrip_and_sanitize() {
    let t = tempfile::tempdir().unwrap();
    let paths = Paths::new(t.path().join("cfg"));
    assert!(paths.config().ends_with("launcher.toml") && paths.prefs().ends_with("prefs.toml"));
    let _ = Paths::platform_default();
    assert_eq!(Prefs::load(&paths.prefs()).unwrap(), Prefs::default());
    let p = Prefs { language: "tr".into(), theme: "dark".into(), game_sound: false, ..Prefs::default() };
    p.save(&paths.prefs()).unwrap();
    assert_eq!(Prefs::load(&paths.prefs()).unwrap(), p);
    // Unknown values fall back, partial files take defaults, junk is an error.
    std::fs::write(paths.prefs(), "language = \"xx\"\ntheme = \"neon\"\n").unwrap();
    assert_eq!(Prefs::load(&paths.prefs()).unwrap(), Prefs::default());
    std::fs::write(paths.prefs(), "language = 3").unwrap();
    assert!(matches!(Prefs::load(&paths.prefs()), Err(LauncherError::Parse(_))));
    assert!(matches!(Prefs::load(t.path()), Err(LauncherError::Io(_))));
    assert!(matches!(p.save(&t.path().join("cfg/prefs.toml/x")), Err(LauncherError::Io(_))));
}

#[test]
fn data_folder_helpers() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src");
    std::fs::create_dir_all(src.join("a/b")).unwrap();
    std::fs::write(src.join("a/b/f1"), b"12345").unwrap();
    std::fs::write(src.join("f2"), b"123").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/hostname", src.join("link")).unwrap();
    assert_eq!(dir_size(&src), 8);
    assert_eq!(dir_size(&t.path().join("none")), 0);
    let dst = t.path().join("dst");
    assert_eq!(copy_dir(&src, &dst).unwrap(), 2);
    assert_eq!(std::fs::read(dst.join("a/b/f1")).unwrap(), b"12345");
    assert!(copy_dir(&t.path().join("none"), &dst).is_err());

    let apk = t.path().join("g.apk");
    std::fs::write(&apk, b"apk").unwrap();
    let kept = keep_private_copy(&apk, &dst).unwrap();
    assert_eq!(std::fs::read(&kept).unwrap(), b"apk");
    assert_eq!(keep_private_copy(&kept, &dst).unwrap(), kept);
    assert!(keep_private_copy(&t.path().join("none.apk"), &dst).is_err());
}

#[test]
fn import_summary_groups_items() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("pull");
    std::fs::create_dir_all(src.join("files/saves/w1")).unwrap();
    std::fs::create_dir_all(src.join("files/saves/w2")).unwrap();
    std::fs::create_dir_all(src.join("shared_prefs")).unwrap();
    std::fs::write(src.join("files/saves/w1/a"), b"1234").unwrap();
    std::fs::write(src.join("files/saves/w1/b"), b"12").unwrap();
    std::fs::write(src.join("files/saves/w2/a"), b"1").unwrap();
    std::fs::write(src.join("files/top"), b"1").unwrap();
    std::fs::write(src.join("shared_prefs/p.xml"), b"1").unwrap();
    let dirs = DataDirs::new(t.path().join("data"));
    let report = import_saves(&src, &dirs, Existing::Skip).unwrap();
    let s = report.summary(&dirs);
    assert_eq!((s.written, s.skipped_existing, s.ignored), (5, 0, 0));
    let items: Vec<_> = s.items.iter().map(|i| (i.name.as_str(), i.files, i.bytes)).collect();
    assert_eq!(items, [("saves/w1", 2, 6), ("saves/w2", 1, 1), ("shared_prefs/p.xml", 1, 1), ("top", 1, 1)]);
    // Paths outside the root are kept whole.
    let odd = crate::import::Report { written: vec!["/nowhere/x/y/z".into()], ..Default::default() };
    assert_eq!(odd.summary(&dirs).items[0].name, "nowhere/x");
}

#[test]
fn run_args_and_config_expansion() {
    let t = tempfile::tempdir().unwrap();
    let mut c = LaunchConfig::new("/x/game.apk", "/data");
    assert_eq!(
        c.run_args(),
        [
            "--apk",
            "/x/game.apk",
            "--data-dir",
            "/data",
            "--window",
            "1024x768",
            "--fps-limit",
            "60",
            "--unlock-fps",
            "--hide-dpad"
        ]
    );
    c.force = true;
    c.window.fullscreen = true;
    c.window.vsync = false;
    c.window.width = 1600;
    c.window.height = 900;
    c.window.show_fps = true;
    c.window.fps_limit = 0;
    c.controller = false;
    c.window.parallel_threads = false;
    c.show_dpad = true;
    c.pad_tile_cursor = true;
    assert_eq!(
        c.run_args()[4..],
        [
            "--window",
            "1600x900",
            "--fps-limit",
            "0",
            "--unlock-fps",
            "--force",
            "--fullscreen",
            "--no-vsync",
            "--show-fps",
            "--no-controller",
            "--serial-guest",
            "--pad-tile-cursor"
        ]
    );
    c.controller = true;
    c.window.parallel_threads = true;
    c.show_dpad = false;
    c.pad_tile_cursor = false;
    use crate::window::FpsPosition;
    for (p, flag) in [(FpsPosition::TopRight, "tr"), (FpsPosition::BottomLeft, "bl"), (FpsPosition::BottomRight, "br")]
    {
        c.window.fps_position = p;
        assert_eq!(c.run_args()[14..], ["--fps-position", flag]);
    }
    // The position only matters with the counter on; top-left is bhpc-run's default.
    c.window.show_fps = false;
    assert!(!c.run_args().contains(&"--fps-position".to_string()));
    c.window.show_fps = true;
    c.window.fps_position = FpsPosition::TopLeft;
    assert!(!c.run_args().contains(&"--fps-position".to_string()));
    // The game speed only when it is not normal (1 leaves the game untouched).
    assert!(!c.run_args().contains(&"--game-speed".to_string()));
    c.window.game_speed = 1.5;
    assert_eq!(c.run_args()[14..], ["--game-speed", "1.5"]);
    // The zoom only when it is not the phone-like default.
    c.window.game_speed = 1.0;
    assert!(!c.run_args().contains(&"--zoom".to_string()));
    use crate::window::GameZoom;
    for (z, flag) in [(GameZoom::Close, "close"), (GameZoom::Tablet, "tablet")] {
        c.window.zoom = z;
        assert_eq!(c.run_args()[14..], ["--zoom", flag]);
        assert_eq!(GameZoom::parse(flag), Some(z));
    }
    assert_eq!(GameZoom::parse("phone"), Some(GameZoom::Phone));
    assert_eq!(GameZoom::parse("huge"), None);
    c.window.zoom = GameZoom::Phone;
    // The game UI scale only when it is not 100 %.
    assert!(!c.run_args().contains(&"--ui-scale".to_string()));
    c.window.ui_scale = 1.15;
    assert_eq!(c.run_args()[14..], ["--ui-scale", "1.15"]);
    c.window.ui_scale = 1.0;
    c.window.game_speed = 1.5;

    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(expand_args(&s(&["--init-only"])).unwrap(), s(&["--init-only"]));
    let path = t.path().join("c.toml");
    c.save(&path).unwrap();
    let p = path.to_str().unwrap();
    let out = expand_args(&s(&["-v", "--config", p, "--seconds", "3"])).unwrap();
    assert_eq!(out[..3], s(&["-v", "--seconds", "3"])[..]);
    assert_eq!(out[3..], c.run_args()[..]);
    assert!(matches!(expand_args(&s(&["--config"])), Err(LauncherError::Io(_))));
    let missing = t.path().join("nope.toml");
    assert!(expand_args(&s(&["--config", missing.to_str().unwrap()]))
        .unwrap_err()
        .to_string()
        .contains("does not exist"));
}

#[test]
fn log_buffer_is_bounded_and_addressable() {
    let mut l = LogBuffer::new(3);
    for i in 0..5 {
        l.push(format!("l{i}"));
    }
    assert_eq!(l.since(0), (vec!["l2".into(), "l3".into(), "l4".into()], 5));
    assert_eq!(l.since(4), (vec!["l4".into()], 5));
    assert_eq!(l.since(9), (vec![], 5));
    l.clear();
    assert_eq!(l.since(0), (vec![], 0));
}

#[test]
fn sidecar_name() {
    let p = sidecar_path(Path::new("/opt/app"));
    assert!(p.starts_with("/opt/app") && p.to_string_lossy().contains("bhpc-run"));
}

#[test]
fn default_session_is_idle() {
    assert_eq!(Session::default().poll(0, true).state, GameState::Ready);
}

#[test]
fn session_error_text() {
    assert!(SessionError::AlreadyRunning.to_string().contains("already"));
    assert!(SessionError::Spawn("x".into()).to_string().contains("cannot start"));
}

#[cfg(unix)]
mod unix {
    use super::*;
    use crate::session::Snapshot;
    use std::time::Duration;

    fn sh(script: &str) -> (std::path::PathBuf, Vec<String>) {
        ("/bin/sh".into(), vec!["-c".into(), script.into()])
    }

    fn until_done(s: &mut Session, apk: bool) -> Snapshot {
        for _ in 0..500 {
            let snap = s.poll(0, apk);
            if !matches!(snap.state, GameState::Starting | GameState::Running) {
                return snap;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the process did not end");
    }

    #[test]
    fn clean_exit_is_stopped_and_log_is_kept() {
        let mut s = Session::new(Duration::ZERO);
        assert_eq!(s.poll(0, true).state, GameState::Ready);
        assert_eq!(s.poll(0, false).state, GameState::Missing);
        let (prog, args) = sh("echo out; echo err >&2; printf 'no newline'");
        s.start(&prog, &args, &[]).unwrap();
        let snap = until_done(&mut s, true);
        assert_eq!((snap.state, snap.exit_code, snap.signal), (GameState::Stopped, Some(0), None));
        assert!(snap.last_session_secs.is_some() && snap.running_secs.is_none());
        let mut lines = snap.lines.clone();
        lines.sort();
        assert_eq!(lines, ["err", "no newline", "out"]);
        assert_eq!(s.poll(snap.next, true).lines, Vec::<String>::new());
        // Missing file after a normal stop shows as missing.
        assert_eq!(s.poll(0, false).state, GameState::Missing);
        let t = tempfile::tempdir().unwrap();
        s.save_log(&t.path().join("logs/last.log")).unwrap();
        assert_eq!(std::fs::read_to_string(t.path().join("logs/last.log")).unwrap().lines().count(), 3);
        // The next start replaces the log.
        let (prog, args) = sh("echo second");
        s.start(&prog, &args, &[]).unwrap();
        assert_eq!(until_done(&mut s, true).lines, ["second"]);
    }

    #[test]
    fn lines_reach_the_childs_stdin_and_a_dead_child_drops_them() {
        let mut s = Session::new(Duration::ZERO);
        assert!(s.input().is_none(), "nothing runs");
        let (prog, args) = sh("read a; read b; echo \"got $a $b\"");
        s.start(&prog, &args, &[]).unwrap();
        let tx = s.input().unwrap();
        tx.send("one".into()).unwrap();
        tx.send("two".into()).unwrap();
        assert_eq!(until_done(&mut s, true).lines, ["got one two"]);
        assert!(s.input().is_none(), "the finished session has no input");
        // The child is gone: the writer thread stops at the first failed write and later sends fail.
        for _ in 0..200 {
            let _ = tx.send("late".into());
            std::thread::sleep(Duration::from_millis(5));
            if tx.send("later".into()).is_err() {
                return;
            }
        }
        panic!("lines to a dead child were not dropped");
    }

    #[test]
    fn environment_reaches_the_child() {
        let mut s = Session::new(Duration::ZERO);
        let (prog, args) = sh("echo \"$BHPC_TEST_VAR\"");
        s.start(&prog, &args, &[("BHPC_TEST_VAR".into(), "hello".into())]).unwrap();
        assert_eq!(until_done(&mut s, true).lines, ["hello"]);
    }

    #[test]
    fn nonzero_exit_and_signal_are_crashes() {
        let mut s = Session::new(Duration::ZERO);
        let (prog, args) = sh("echo boom; exit 3");
        s.start(&prog, &args, &[]).unwrap();
        let snap = until_done(&mut s, true);
        assert_eq!((snap.state, snap.exit_code), (GameState::Crashed, Some(3)));
        // A crash stays a crash even when the file is gone.
        assert_eq!(s.poll(0, false).state, GameState::Crashed);
        let (prog, args) = sh("kill -SEGV $$");
        s.start(&prog, &args, &[]).unwrap();
        let snap = until_done(&mut s, true);
        assert_eq!((snap.state, snap.exit_code, snap.signal), (GameState::Crashed, None, Some(11)));
    }

    #[test]
    fn stop_kills_and_counts_as_stopped() {
        let mut s = Session::new(Duration::from_secs(3600));
        s.stop(); // nothing to stop
        let (prog, args) = sh("exec sleep 30");
        s.start(&prog, &args, &[]).unwrap();
        let snap = s.poll(0, true);
        assert_eq!(snap.state, GameState::Starting);
        assert!(snap.running_secs.is_some());
        assert_eq!(s.start(&prog, &args, &[]), Err(SessionError::AlreadyRunning));
        s.stop();
        let snap = s.poll(0, true);
        assert_eq!((snap.state, snap.signal), (GameState::Stopped, Some(9)));
    }

    #[test]
    fn copy_lines_go_to_the_handler() {
        let mut s = Session::new(Duration::ZERO);
        // Without a handler the line is dropped all the same.
        let (prog, args) = sh("echo '@copy lost'; echo after");
        s.start(&prog, &args, &[]).unwrap();
        assert_eq!(until_done(&mut s, true).lines, ["after"]);
        let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let g = got.clone();
        s.set_copy_handler(move |t| g.lock().unwrap().push(t));
        let (prog, args) = sh("echo '@copy a%0Ab'; echo '@copy b' >&2; echo done");
        s.start(&prog, &args, &[]).unwrap();
        let snap = until_done(&mut s, true);
        let mut lines = snap.lines.clone();
        lines.sort();
        assert_eq!(lines, ["@copy b", "done"], "stderr is never parsed");
        assert_eq!(*got.lock().unwrap(), ["a\nb"]);
    }

    #[test]
    fn stop_asks_an_in_window_game_to_quit_first() {
        let mut s = Session::new(Duration::from_secs(3600));
        // It reads `quit` and ends by itself: stopped, not killed.
        let (prog, mut args) = sh("read l; [ \"$l\" = quit ] && exit 0; exit 5");
        args.push("--embed".into());
        s.start(&prog, &args, &[]).unwrap();
        s.stop();
        let snap = s.poll(0, true);
        assert_eq!((snap.state, snap.exit_code, snap.signal), (GameState::Stopped, Some(0), None));
        // One that does not end in time is killed.
        s.set_quit_grace(Duration::from_millis(100));
        let (prog, mut args) = sh("trap '' TERM; exec sleep 30");
        args.push("--embed".into());
        s.start(&prog, &args, &[]).unwrap();
        s.stop();
        let snap = s.poll(0, true);
        assert_eq!((snap.state, snap.signal), (GameState::Stopped, Some(9)));
    }

    #[test]
    fn progress_lines_drive_the_state_and_leave_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let hist = crate::progress::history_path(dir.path());
        let mut s = Session::new(Duration::ZERO);
        s.set_history(hist.clone());
        // Progress seen, no @ready yet: still starting although the grace period is zero.
        let (prog, args) = sh("echo '@progress constructors 5 10'; echo hello; exec sleep 30");
        s.start(&prog, &args, &[]).unwrap();
        let snap = loop {
            let snap = s.poll(0, true);
            if snap.progress.is_some() {
                break snap;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(snap.state, GameState::Starting);
        let info = snap.progress.unwrap();
        assert_eq!((info.phase, info.step, info.ready), ("constructors", 3, false));
        assert!(info.percent > 28.0 && info.percent < 50.0);
        assert!(!snap.lines.iter().any(|l| l.starts_with('@')), "{:?}", snap.lines);
        s.stop();
        // The half-way point is still known after the stop.
        assert_eq!(s.poll(0, true).progress.unwrap().phase, "constructors");
        // @ready: running, and the durations are remembered for the next start.
        let (prog, args) = sh("echo '@progress apk 0 1'; echo '@ready'; exec sleep 30");
        s.start(&prog, &args, &[]).unwrap();
        let snap = loop {
            let snap = s.poll(0, true);
            if snap.progress.as_ref().is_some_and(|i| i.ready) {
                break snap;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(snap.state, GameState::Running);
        s.poll(0, true);
        assert_eq!(crate::progress::History::load(&hist).starts.len(), 1, "learned once");
        s.stop();
        assert!(s.poll(0, true).progress.unwrap().ready, "the last session's end is kept while stopped");
        // Play again: before the new game's first line the old session's 100 % is not shown.
        let (prog, args) = sh("exec sleep 30");
        s.start(&prog, &args, &[]).unwrap();
        assert_eq!(s.poll(0, true).progress, None);
        s.stop();
        // Without a history file nothing is written, and a new start has a fresh tracker.
        let mut plain = Session::new(Duration::ZERO);
        plain.start(&prog, &args, &[]).unwrap();
        while plain.poll(0, true).state != GameState::Running {
            std::thread::sleep(Duration::from_millis(10));
        }
        plain.stop();
    }

    #[test]
    fn a_start_without_progress_for_too_long_is_killed() {
        let mut s = Session::new(Duration::from_secs(3600));
        s.set_stall(Duration::from_millis(100));
        let (prog, args) = sh("echo '@progress frame 0 0'; exec sleep 30");
        s.start(&prog, &args, &[]).unwrap();
        let snap = until_done(&mut s, true);
        assert_eq!((snap.state, snap.signal), (GameState::Crashed, Some(9)));
        let info = snap.progress.unwrap();
        assert!(info.stalled && info.phase == "frame");
    }

    #[test]
    fn running_after_grace_and_spawn_failure() {
        let mut s = Session::new(Duration::ZERO);
        let (prog, args) = sh("exec sleep 30");
        s.start(&prog, &args, &[]).unwrap();
        assert_eq!(s.poll(0, true).state, GameState::Running);
        s.stop();
        let err = s.start(Path::new("/nonexistent/bhpc-run"), &[], &[]).unwrap_err();
        assert!(matches!(err, SessionError::Spawn(ref m) if m.contains("/nonexistent")));
        // A process that ended by itself before stop() is reported as it ended.
        let (prog, args) = sh("exit 4");
        s.start(&prog, &args, &[]).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        s.stop();
        assert_eq!(s.poll(0, true).state, GameState::Crashed);
    }
}

mod ops_tests {
    use super::*;
    use crate::ops::{
        self, boot, data_dir_from, finish_setup, launch_plan, move_data_dir, normalize_binding, paths, paths_from,
        save_config, save_prefs, use_apk,
    };
    use crate::window::WindowOptions;
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn mods_live_below_the_data_folder() {
        let t = tempfile::tempdir().unwrap();
        let paths = crate::prefs::Paths::new(t.path().join("cfg"));
        assert_eq!(ops::mods_dirs(&paths, PathBuf::from("/def")).mods, PathBuf::from("/def/mods"));
        LaunchConfig::new("/a.apk", "/games/data").save(&paths.config()).unwrap();
        assert_eq!(ops::mods_dirs(&paths, PathBuf::from("/def")).cache, PathBuf::from("/games/data/mods-cache"));
        std::fs::write(paths.config(), "[[[").unwrap();
        assert_eq!(ops::mods_dirs(&paths, PathBuf::from("/def")).mods, PathBuf::from("/def/mods"));
    }

    fn fake_apk(dir: &Path) -> PathBuf {
        let p = dir.join("game.apk");
        make_zip(&p, &[("lib/armeabi-v7a/libApplication.so", b"synthetic libApplication")]);
        p
    }

    fn setup(t: &Path) -> (Paths, LaunchConfig) {
        let paths = Paths::new(t.join("cfg"));
        let mut c = LaunchConfig::new(fake_apk(t), t.join("data"));
        c.force = true;
        (paths, c)
    }

    #[test]
    fn directories_from_environment_or_platform() {
        let some = |s: &str| Some(OsString::from(s));
        assert_eq!(paths_from(some("/x"), None).unwrap(), Paths::new("/x"));
        assert_eq!(paths_from(some(""), Some(Paths::new("/p"))).unwrap(), Paths::new("/p"));
        assert_eq!(paths_from(None, Some(Paths::new("/p"))).unwrap(), Paths::new("/p"));
        assert_eq!(paths_from(None, None), Err(LauncherError::NoDefaultDir));
        let _ = paths();
        assert_eq!(data_dir_from(some("/d"), Err(LauncherError::NoDefaultDir)).unwrap(), PathBuf::from("/d"));
        assert_eq!(data_dir_from(None, Ok("/p".into())).unwrap(), PathBuf::from("/p"));
        assert_eq!(data_dir_from(some(""), Err(LauncherError::NoDefaultDir)), Err(LauncherError::NoDefaultDir));
    }

    #[test]
    fn boot_save_and_prefs() {
        let t = tempfile::tempdir().unwrap();
        let (paths, c) = setup(t.path());
        let b = boot(&paths, "/data".into()).unwrap();
        assert_eq!((b.config, b.defaults.data_dir, b.prefs), (None, PathBuf::from("/data"), Prefs::default()));
        assert!(!b.prefs_saved);
        let mut bad = c.clone();
        bad.window = WindowOptions { width: 1, ..WindowOptions::default() };
        assert!(matches!(save_config(&paths, bad), Err(LauncherError::Window(_))));
        let mut dup = c.clone();
        dup.keys.pan = dup.keys.touch; // conflicts are saved and shown, not refused
        assert_eq!(save_config(&paths, dup.clone()).unwrap(), dup);
        let p = save_prefs(&paths, Prefs { language: "zz".into(), ..Prefs::default() }).unwrap();
        assert_eq!(p.language, "en");
        let b = boot(&paths, "/data".into()).unwrap();
        assert_eq!(b.config, Some(dup));
        assert!(b.prefs_saved);
        for l in ["tr", "ru", "kk"] {
            assert_eq!(save_prefs(&paths, Prefs { language: l.into(), ..Prefs::default() }).unwrap().language, l);
        }
        std::fs::write(paths.prefs(), "language = 1").unwrap();
        assert!(boot(&paths, "/data".into()).is_err());
    }

    #[test]
    fn the_game_title_follows_the_language() {
        assert_eq!(ops::game_title("en", Some("1.7.6")), "Blockheads PC · Playing The Blockheads 1.7.6");
        assert_eq!(ops::game_title("tr", Some("1.7.4")), "Blockheads PC · The Blockheads 1.7.4 oynanıyor");
        assert_eq!(ops::game_title("kk", None), "Blockheads PC · Ойын жүріп жатыр: The Blockheads (белгісіз жинақ)");
        assert_eq!(ops::game_title("zz", None), "Blockheads PC · Playing The Blockheads (unknown build)");
    }

    #[test]
    fn setup_finishes_with_a_private_copy() {
        let t = tempfile::tempdir().unwrap();
        let (paths, c) = setup(t.path());
        let done = finish_setup(&paths, c.clone(), Prefs::default()).unwrap();
        assert_eq!(done.apk, t.path().join("data/game/TheBlockheads.apk"));
        assert!(done.apk.exists());
        assert_eq!(boot(&paths, "/d".into()).unwrap().config, Some(done));
        // Without a copy the original path is kept.
        let kept = finish_setup(&paths, c.clone(), Prefs { keep_private_copy: false, ..Prefs::default() }).unwrap();
        assert_eq!(kept.apk, c.apk);
        // Refusals: bad window, unknown build without force, unreadable folder for the copy.
        let mut bad = c.clone();
        bad.window.height = 1;
        assert!(matches!(finish_setup(&paths, bad, Prefs::default()), Err(LauncherError::Window(_))));
        let mut strict = c.clone();
        strict.force = false;
        assert!(matches!(finish_setup(&paths, strict, Prefs::default()), Err(LauncherError::Apk(_))));
        let mut blocked = c.clone();
        std::fs::write(t.path().join("file"), b"").unwrap();
        blocked.data_dir = t.path().join("file");
        assert!(matches!(finish_setup(&paths, blocked, Prefs::default()), Err(LauncherError::Io(_))));
    }

    #[test]
    fn switching_game_file_and_bindings() {
        let t = tempfile::tempdir().unwrap();
        let (paths, c) = setup(t.path());
        assert!(matches!(use_apk(&paths, &c.apk, true, false), Err(LauncherError::Io(_))));
        c.save(&paths.config()).unwrap();
        let other = t.path().join("other.apk");
        std::fs::copy(&c.apk, &other).unwrap();
        let r = use_apk(&paths, &other, true, false).unwrap();
        assert_eq!(r.apk, other);
        let r = use_apk(&paths, &other, true, true).unwrap();
        assert!(r.apk.ends_with("game/TheBlockheads.apk"));
        assert!(matches!(use_apk(&paths, &other, false, false), Err(LauncherError::Apk(_))));
        assert_eq!(normalize_binding(" ctrl + left ").unwrap(), "Ctrl+Left");
        assert_eq!(normalize_binding("ArrowUp").unwrap(), "Up");
        assert!(matches!(normalize_binding("Hyper+Q"), Err(LauncherError::Parse(_))));
    }

    #[test]
    fn import_and_move() {
        let t = tempfile::tempdir().unwrap();
        let (paths, mut c) = setup(t.path());
        let src = t.path().join("pull");
        std::fs::create_dir_all(src.join("files/s")).unwrap();
        std::fs::write(src.join("files/s/a"), b"1").unwrap();
        let s = ops::import_saves(&src, false, &c.data_dir).unwrap();
        assert_eq!(s.written, 1);
        assert_eq!(ops::import_saves(&src, false, &c.data_dir).unwrap().skipped_existing, 1);
        assert_eq!(ops::import_saves(&src, true, &c.data_dir).unwrap().written, 1);
        assert!(matches!(ops::import_saves(&t.path().join("none"), false, &c.data_dir), Err(LauncherError::Io(_))));

        assert!(matches!(move_data_dir(&paths, &t.path().join("new")), Err(LauncherError::Io(_))));
        c.apk = c.data_dir.join("game/TheBlockheads.apk");
        c.save(&paths.config()).unwrap();
        let same = move_data_dir(&paths, &c.data_dir).unwrap();
        assert_eq!(same, c);
        let to = t.path().join("new");
        let moved = move_data_dir(&paths, &to).unwrap();
        assert_eq!((moved.data_dir.clone(), moved.apk.clone()), (to.clone(), to.join("game/TheBlockheads.apk")));
        assert!(to.join("home/s/a").exists() && c.data_dir.join("home/s/a").exists());
        // An APK outside the data folder stays where it is; a missing source folder fails.
        let mut outside = moved.clone();
        outside.apk = t.path().join("game.apk");
        outside.save(&paths.config()).unwrap();
        assert_eq!(move_data_dir(&paths, &t.path().join("n2")).unwrap().apk, outside.apk);
        let mut gone = outside.clone();
        gone.data_dir = t.path().join("does-not-exist");
        gone.save(&paths.config()).unwrap();
        assert!(matches!(move_data_dir(&paths, &t.path().join("n3")), Err(LauncherError::Io(_))));
    }

    #[test]
    fn launch_plan_checks_everything_first() {
        let t = tempfile::tempdir().unwrap();
        let (paths, mut c) = setup(t.path());
        let exe = Path::new("/opt/app");
        assert!(matches!(launch_plan(&paths, exe), Err(LauncherError::Io(_))));
        c.save(&paths.config()).unwrap();
        let plan = launch_plan(&paths, exe).unwrap();
        assert!(plan.program.starts_with("/opt/app") && plan.env.is_empty());
        assert_eq!(plan.args[..2], ["--config", paths.config().to_str().unwrap()]);
        assert_eq!(plan.args[2], "--title");
        assert_eq!(plan.title, "Blockheads PC · Playing The Blockheads (unknown build)");
        assert_eq!(plan.args[3], plan.title);
        assert_eq!(plan.args[4..6], ["--lang", "en"]);
        assert_eq!(plan.window, c.window);
        assert_eq!(plan.keys, c.keys);
        assert_eq!(plan.args_for(None)[..plan.args.len()], plan.args);
        assert_eq!(plan.args_for(None)[plan.args.len()..], ["--progress"]);
        let embedded = plan.args_for(Some(Path::new("/run/f")));
        assert_eq!(embedded[plan.args.len()..], ["--progress", "--embed", "/run/f"]);
        let mut linked = plan.clone();
        linked.open_uri = Some("http://theblockheads.net/join.php?ip=1".into());
        assert_eq!(
            linked.args_for(None)[plan.args.len()..],
            ["--progress", "--open-uri", "http://theblockheads.net/join.php?ip=1"]
        );
        Prefs { game_sound: false, language: "ru".into(), ..Prefs::default() }.save(&paths.prefs()).unwrap();
        let ru = launch_plan(&paths, exe).unwrap();
        assert_eq!(ru.env, [("SDL_AUDIODRIVER".to_string(), "dummy".to_string())]);
        assert_eq!(
            (ru.title.as_str(), &ru.args[5]),
            ("Blockheads PC · Идёт игра: The Blockheads (неизвестная сборка)", &"ru".to_string())
        );
        c.window.width = 1;
        c.save(&paths.config()).unwrap();
        assert!(matches!(launch_plan(&paths, exe), Err(LauncherError::Window(_))));
        c.window = WindowOptions::default();
        c.apk = t.path().join("missing.apk");
        c.save(&paths.config()).unwrap();
        assert!(matches!(launch_plan(&paths, exe), Err(LauncherError::Apk(_))));
        c.apk = fake_apk(t.path());
        c.save(&paths.config()).unwrap();
        std::fs::write(paths.prefs(), "language = 1").unwrap();
        assert!(matches!(launch_plan(&paths, exe), Err(LauncherError::Parse(_))));
    }
}

mod embed_tests {
    use crate::embed::overwritten;
    use crate::embed::*;

    fn bgra(w: u32, h: u32, v: u8) -> Vec<u8> {
        vec![v; (w * h * 4) as usize]
    }

    #[test]
    fn frames_travel_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = frame_path(dir.path(), 7);
        assert!(path.to_string_lossy().contains("bhpc-frame-"));
        let mut r = FrameReader::create(&path).unwrap();
        let mut buf = Vec::new();
        assert_eq!(r.seq().unwrap(), 0);
        assert_eq!(r.read(&mut buf).unwrap(), None, "no frame yet");
        let mut w = FrameWriter::open(&path).unwrap();
        assert!(w.publish(4, 2, &bgra(4, 2, 9)).unwrap());
        assert_eq!(r.seq().unwrap(), 1);
        let info = r.read(&mut buf).unwrap().unwrap();
        assert_eq!(info, FrameInfo { width: 4, height: 2, seq: 1, layout: Layout::Bgra });
        assert_eq!(buf, bgra(4, 2, 9));
        assert_eq!(r.read(&mut buf).unwrap(), None, "already seen");
        // A different size, then the writer runs ahead by several frames while the reader sleeps.
        for i in 0..5 {
            assert!(w.publish(3, 3, &bgra(3, 3, i)).unwrap());
        }
        let info = r.read(&mut buf).unwrap().unwrap();
        assert_eq!((info.width, info.height, info.seq), (3, 3, 6));
        assert_eq!(buf, bgra(3, 3, 4));
        // Wrong length, empty and oversized frames are refused.
        assert!(!w.publish(2, 2, &[0; 3]).unwrap());
        assert!(!w.publish(0, 2, &[]).unwrap());
        assert!(!w.publish(9000, 5000, &[]).unwrap());
        // A writer that reopens the file carries on counting.
        let mut w2 = FrameWriter::open(&path).unwrap();
        w2.publish(1, 1, &[1, 2, 3, 255]).unwrap();
        assert_eq!(r.read(&mut buf).unwrap().unwrap().seq, 7);
        r.remove();
        assert!(!path.exists());
    }

    #[test]
    fn frames_say_their_layout_and_can_be_used_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        let mut r = FrameReader::create(&path).unwrap();
        let mut used = false;
        assert_eq!(r.read_with(|_, _| used = true).unwrap(), None, "no frame yet");
        let mut w = FrameWriter::open(&path).unwrap();
        assert!(w.publish_as(2, 1, Layout::GlRgba, &[1, 2, 3, 4, 5, 6, 7, 8]).unwrap());
        let mut seen = Vec::new();
        let info = r.read_with(|_, px| seen = px.to_vec()).unwrap().unwrap();
        assert_eq!(info, FrameInfo { width: 2, height: 1, seq: 1, layout: Layout::GlRgba });
        assert_eq!(seen, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(r.read_with(|_, _| used = true).unwrap(), None, "already seen");
        assert!(!used);
        // The mapping is made once and follows later frames.
        assert!(w.publish(1, 1, &[9, 9, 9, 255]).unwrap());
        let info = r.read_with(|_, px| assert_eq!(px, [9, 9, 9, 255])).unwrap().unwrap();
        assert_eq!(info.layout, Layout::Bgra);
        // The runner came round to the slot while the pixels were used: the frame does not count.
        assert!(w.publish(1, 1, &[1; 4]).unwrap());
        let mut w2 = FrameWriter::open(&path).unwrap();
        let late = r.read_with(|_, _| {
            w2.publish(1, 1, &[2; 4]).unwrap();
            w2.publish(1, 1, &[3; 4]).unwrap();
        });
        assert_eq!(late.unwrap(), None);
        assert_eq!(Layout::default(), Layout::Bgra);
        #[cfg(unix)]
        assert!(crate::embed::Map::new(&std::fs::File::open(&path).unwrap(), 0).is_err(), "an empty mapping");
    }

    #[test]
    fn a_frame_with_an_unknown_layout_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        let mut r = FrameReader::create(&path).unwrap();
        FrameWriter::open(&path).unwrap().publish(1, 1, &[0; 4]).unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        crate::embed::write_at(&f, &7u32.to_le_bytes(), 36).unwrap();
        assert_eq!(r.read_with(|_, _| {}).unwrap_err().kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_frame_is_overwritten_when_the_writer_comes_round() {
        assert!(!overwritten(5, 5));
        assert!(!overwritten(5, 6));
        assert!(overwritten(5, 7));
        assert!(overwritten(5, 90));
    }

    #[test]
    fn broken_files_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk");
        std::fs::write(&junk, vec![0u8; 200]).unwrap();
        assert!(FrameWriter::open(&junk).is_err());
        assert!(FrameWriter::open(&dir.path().join("missing")).is_err());
        let path = dir.path().join("f");
        let mut r = FrameReader::create(&path).unwrap();
        // A header that claims a frame bigger than the file allows.
        let mut raw = std::fs::read(&path).unwrap()[..64].to_vec();
        raw[16..24].copy_from_slice(&1u64.to_le_bytes());
        raw[24..28].copy_from_slice(&60000u32.to_le_bytes());
        raw[28..32].copy_from_slice(&60000u32.to_le_bytes());
        std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        crate::embed::write_at(&std::fs::OpenOptions::new().write(true).open(&path).unwrap(), &raw, 0).unwrap();
        assert_eq!(r.read(&mut Vec::new()).unwrap_err().kind(), std::io::ErrorKind::InvalidData);
        raw[24..28].copy_from_slice(&2u32.to_le_bytes());
        raw[28..32].copy_from_slice(&2u32.to_le_bytes());
        raw[32..36].copy_from_slice(&9u32.to_le_bytes());
        crate::embed::write_at(&std::fs::OpenOptions::new().write(true).open(&path).unwrap(), &raw, 0).unwrap();
        assert!(r.read(&mut Vec::new()).is_err(), "slot out of range");
        assert!(frame_dir().is_dir());
    }

    #[test]
    fn frame_files_of_dead_launchers_are_cleaned_up() {
        let dir = tempfile::tempdir().unwrap();
        for n in ["bhpc-frame-111-0", "bhpc-frame-222-3", "bhpc-frame-oops", "other-file"] {
            std::fs::write(dir.path().join(n), b"x").unwrap();
        }
        assert_eq!(clean_stale(dir.path(), &|pid| pid == 222), 1);
        assert!(!dir.path().join("bhpc-frame-111-0").exists());
        for kept in ["bhpc-frame-222-3", "bhpc-frame-oops", "other-file"] {
            assert!(dir.path().join(kept).exists(), "{kept}");
        }
        assert_eq!(clean_stale(&dir.path().join("missing"), &|_| false), 0);
        assert!(pid_alive(std::process::id()));
        assert_eq!(pid_alive(u32::MAX - 1), !cfg!(target_os = "linux"));
    }

    #[test]
    fn commands_round_trip_as_lines() {
        let all = [
            Cmd::Size { width: 1280, height: 720 },
            Cmd::Move { x: 1.5, y: -2.0 },
            Cmd::Button { button: 2, pressed: true, x: 10.0, y: 20.25, mods: MOD_CTRL | MOD_ALT },
            Cmd::Button { button: 0, pressed: false, x: 0.0, y: 0.0, mods: 0 },
            Cmd::Wheel { dx: 0.0, dy: -1.0, x: 3.0, y: 4.0, mods: MOD_SHIFT },
            Cmd::Key { hid: 68, pressed: true, repeat: false, mods: 0 },
            Cmd::Key { hid: 4, pressed: false, repeat: true, mods: 7 },
            Cmd::Text("héllo wörld\nline\\two\r".into()),
            Cmd::Text(String::new()),
            Cmd::Focus(true),
            Cmd::Focus(false),
            Cmd::Visible(false),
            Cmd::Layout(Layout::GlRgba),
            Cmd::Layout(Layout::Bgra),
            Cmd::Quit,
        ];
        for c in all {
            let line = c.encode();
            assert!(!line.contains('\n') && !line.contains('\r'), "{line:?}");
            assert_eq!(Cmd::parse(&line), Some(c.clone()), "{line}");
            assert_eq!(Cmd::parse(&format!("{line}\n")), Some(c));
        }
        for bad in [
            "",
            "dance",
            "size 1",
            "size a b",
            "move 1",
            "button 1 1 1 1",
            "key 1 1",
            "wheel 1 2 3 4",
            "text bad\\q",
            "text trailing\\",
            "focus",
            "focus 2",
            "key 1 x 0 0",
            "layout",
            "layout 5",
            "layout x",
        ] {
            assert_eq!(Cmd::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn evdev_codes_map_to_hid_usages() {
        // Physical positions: letters, digits, editing keys, function keys and modifiers.
        let table = [
            (1, 41),
            (2, 30),
            (10, 38),
            (11, 39),
            (12, 45),
            (13, 46),
            (14, 42),
            (15, 43),
            (16, 20),
            (25, 19),
            (26, 47),
            (27, 48),
            (28, 40),
            (29, 224),
            (30, 4),
            (38, 15),
            (39, 51),
            (40, 52),
            (41, 53),
            (42, 225),
            (43, 49),
            (44, 29),
            (50, 16),
            (51, 54),
            (52, 55),
            (53, 56),
            (54, 229),
            (56, 226),
            (57, 44),
            (58, 57),
            (59, 58),
            (68, 67),
            (87, 68),
            (88, 69),
            (97, 228),
            (100, 230),
            (102, 74),
            (103, 82),
            (104, 75),
            (105, 80),
            (106, 79),
            (107, 77),
            (108, 81),
            (109, 78),
            (110, 73),
            (111, 76),
        ];
        for (evdev, hid) in table {
            assert_eq!(hid_from_evdev(evdev), Some(hid), "evdev {evdev}");
        }
        assert_eq!(hid_from_evdev(0), None);
        assert_eq!(hid_from_evdev(55), None);
        assert_eq!(hid_from_evdev(500), None);
        // Every letter of the alphabet is reachable exactly once.
        let letters: std::collections::BTreeSet<u32> =
            (0..128).filter_map(hid_from_evdev).filter(|h| (4..=29).contains(h)).collect();
        assert_eq!(letters.len(), 26);
    }
}

#[test]
fn launcher_links_and_copy_lines() {
    let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(crate::ops::open_uri_arg(&a(&["--open-uri", "x://y"])).as_deref(), Some("x://y"));
    assert_eq!(crate::ops::open_uri_arg(&a(&["--open-uri"])), None);
    assert_eq!(
        crate::ops::open_uri_arg(&a(&["-v", "https://theblockheads.net/join"])).as_deref(),
        Some("https://theblockheads.net/join")
    );
    assert_eq!(crate::ops::open_uri_arg(&a(&["http://a"])).as_deref(), Some("http://a"));
    assert_eq!(crate::ops::open_uri_arg(&a(&["blockheads://join?ip=1"])).as_deref(), Some("blockheads://join?ip=1"));
    assert_eq!(crate::ops::open_uri_arg(&a(&["file.txt"])), None);
    use crate::embed::{copy_line, parse_copy};
    for t in ["", "plain text", "100% sure\nnext\r\tline", "ünïcode ✓"] {
        let l = copy_line(t);
        assert!(!l.contains('\n') && !l.contains('\r'), "{l:?}");
        assert_eq!(parse_copy(&l).as_deref(), Some(t));
        assert_eq!(parse_copy(&format!("{l}\r\n")).as_deref(), Some(t));
    }
    assert_eq!(copy_line("a%\nb"), "@copy a%25%0Ab");
    assert_eq!(parse_copy("@copy 50% %zz %4"), Some("50% %zz %4".to_string()));
    assert_eq!(parse_copy("@progress apk 0 1"), None);
}

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
use std::time::Duration;

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
    assert_eq!(c.run_args(), ["--apk", "/x/game.apk", "--data-dir", "/data", "--window", "1024x768"]);
    c.force = true;
    c.window.fullscreen = true;
    c.window.vsync = false;
    c.window.width = 1600;
    c.window.height = 900;
    assert_eq!(c.run_args()[4..], ["--window", "1600x900", "--force", "--fullscreen", "--no-vsync"]);

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

    fn sh(script: &str) -> (std::path::PathBuf, Vec<String>) {
        ("/bin/sh".into(), vec!["-c".into(), script.into()])
    }

    fn until_done(s: &mut Session, apk: bool) -> Snapshot {
        for _ in 0..500 {
            let snap = s.poll(0, apk);
            if !matches!(snap.state, GameState::Launching | GameState::Running) {
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
        assert_eq!(snap.state, GameState::Launching);
        assert!(snap.running_secs.is_some());
        assert_eq!(s.start(&prog, &args, &[]), Err(SessionError::AlreadyRunning));
        s.stop();
        let snap = s.poll(0, true);
        assert_eq!((snap.state, snap.signal), (GameState::Stopped, Some(9)));
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
        std::fs::write(paths.prefs(), "language = 1").unwrap();
        assert!(boot(&paths, "/data".into()).is_err());
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
        assert_eq!(plan.args, ["--config", paths.config().to_str().unwrap()]);
        Prefs { game_sound: false, ..Prefs::default() }.save(&paths.prefs()).unwrap();
        assert_eq!(launch_plan(&paths, exe).unwrap().env, [("SDL_AUDIODRIVER".to_string(), "dummy".to_string())]);
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

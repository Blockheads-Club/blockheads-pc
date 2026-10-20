use crate::apk;
use crate::config::{LaunchConfig, LauncherError};
use crate::import::{import_saves, map_path, Existing, ImportError};
use crate::keys::*;
use crate::window::WindowOptions;
use crate::wizard::{Step, Wizard};
use bhpc_loader::apk::{sha256_hex, ApkError, Release};
use bhpc_vfs::DataDirs;
use std::io::Write;
use std::path::{Path, PathBuf};

const LIB: &[u8] = b"synthetic libApplication";

fn make_zip(path: &Path, files: &[(&str, &[u8])]) {
    let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (n, d) in files {
        if n.ends_with('/') {
            z.add_directory(*n, opts).unwrap();
        } else {
            z.start_file(*n, opts).unwrap();
            z.write_all(d).unwrap();
        }
    }
    z.finish().unwrap();
}

fn fake_apk(dir: &Path) -> PathBuf {
    let p = dir.join("game.apk");
    make_zip(&p, &[("lib/armeabi-v7a/libApplication.so", LIB)]);
    p
}

#[test]
fn apk_validation() {
    let t = tempfile::tempdir().unwrap();
    let p = fake_apk(t.path());
    let hash = sha256_hex(LIB);
    let table = [(Release::V1_7_6, hash.as_str())];
    let info = apk::validate_with(&p, &table, false).unwrap();
    assert_eq!(info.release, Some(Release::V1_7_6));
    assert!(info.describe().contains("1.7.6"));
    // Not in the real table: refused, or accepted when forced.
    assert!(matches!(apk::validate(&p, false), Err(ApkError::UnknownBuild { .. })));
    let forced = apk::validate(&p, true).unwrap();
    assert_eq!(forced.release, None);
    assert!(forced.describe().contains("unknown"));
    assert!(apk::validate(&t.path().join("missing.apk"), false).is_err());
}

#[test]
fn binding_roundtrip() {
    for s in [
        "Left",
        "Right",
        "Middle",
        "Wheel",
        "Esc",
        "F11",
        "Q",
        "7",
        "Ctrl+Left",
        "Ctrl+Shift+Alt+Space",
        "Enter",
        "Backspace",
        "Delete",
        "Tab",
        "Up",
        "Down",
        "Home",
        "End",
        "ArrowLeft",
        "ArrowRight",
        "F1",
    ] {
        let b = Binding::parse(s).unwrap();
        assert_eq!(b.to_string(), s);
    }
    assert_eq!(Binding::parse(" ctrl + q ").unwrap().to_string(), "Ctrl+Q");
    for bad in ["", "Hyper+Q", "F13", "F0", "Fx", "QQ", "Ctrl+", "!"] {
        assert!(Binding::parse(bad).is_err(), "{bad}");
    }
    assert!(Binding::parse("zz").unwrap_err().to_string().contains("zz"));
    assert_eq!(Key::F(12).to_string(), "F12");
}

#[test]
fn host_keys() {
    let k = |s| match Binding::parse(s).unwrap().input {
        Input::Key(k) => k.host_key(),
        _ => panic!(),
    };
    use bhpc_input::HostKey as H;
    assert_eq!(k("Esc"), H::Escape);
    assert_eq!(k("Enter"), H::Enter);
    assert_eq!(k("Backspace"), H::Backspace);
    assert_eq!(k("Delete"), H::Delete);
    assert_eq!(k("Tab"), H::Tab);
    assert_eq!(k("Space"), H::Space);
    assert_eq!(k("ArrowLeft"), H::Left);
    assert_eq!(k("ArrowRight"), H::Right);
    assert_eq!(k("Up"), H::Up);
    assert_eq!(k("Down"), H::Down);
    assert_eq!(k("Home"), H::Home);
    assert_eq!(k("End"), H::End);
    assert_eq!(k("F11"), H::F(11));
    assert_eq!(k("F2"), H::F(2));
    assert_eq!(k("Q"), H::Char('q'));
}

#[test]
fn default_bindings_and_conflicts() {
    let mut kb = KeyBindings::default();
    assert!(kb.conflicts().is_empty());
    for a in Action::ALL {
        assert!(!a.label().is_empty());
        let b = kb.get(a);
        kb.set(a, b);
    }
    assert_eq!(kb.get(Action::Back).to_string(), "Esc");
    assert_eq!(kb.get(Action::Pinch).to_string(), "Ctrl+Left");
    assert_eq!(kb.get(Action::HideUi).to_string(), "H");
    assert_eq!(Action::HideUi.label(), "Hide game buttons");
    assert_eq!(Action::from_name("hide_ui"), Some(Action::HideUi));
    kb.set(Action::Pan, kb.touch);
    assert_eq!(kb.conflicts(), vec![(Action::Touch, Action::Pan)]);
}

#[test]
fn window_options() {
    let w = WindowOptions::default();
    assert!(w.validate().is_ok());
    assert_eq!(w.input_config().window, (1024, 768));
    assert!(WindowOptions { width: 10, ..w.clone() }.validate().is_err());
    assert!(WindowOptions { height: 99999, ..w.clone() }.validate().is_err());
    assert!(WindowOptions { ui_scale: 9.0, ..w.clone() }.validate().unwrap_err().contains("ui_scale"));
    assert_eq!((w.show_fps, w.fps_limit), (false, 60));
    for limit in crate::window::FPS_LIMITS {
        assert!(WindowOptions { fps_limit: limit, ..w.clone() }.validate().is_ok(), "{limit}");
    }
    for limit in [5, 1001] {
        assert!(WindowOptions { fps_limit: limit, ..w.clone() }.validate().unwrap_err().contains("fps_limit"));
    }
    assert_eq!(w.game_speed, 1.0);
    for speed in crate::window::GAME_SPEEDS {
        assert!(WindowOptions { game_speed: speed, ..w.clone() }.validate().is_ok(), "{speed}");
    }
    for speed in [0.1, 5.0, f64::NAN] {
        assert!(WindowOptions { game_speed: speed, ..w.clone() }.validate().unwrap_err().contains("game_speed"));
    }
}

#[test]
fn old_window_tables_get_the_new_defaults() {
    // A launcher.toml written before `show_fps` and `fps_limit` existed.
    let old = "apk = \"a\"\ndata_dir = \"d\"\n[window]\nwidth = 1600\nheight = 900\nfullscreen = true\nvsync = false\nui_scale = 1.25\n";
    let c = LaunchConfig::from_toml(old).unwrap();
    // The removed "Show the game's buttons" setting is ignored.
    assert_eq!(LaunchConfig::from_toml(&format!("show_game_buttons = false\n{old}")).unwrap(), c);
    assert_eq!((c.window.width, c.window.vsync, c.window.show_fps, c.window.fps_limit), (1600, false, false, 60));
    assert_eq!(c.window.fps_position, crate::window::FpsPosition::TopLeft);
    assert_eq!(c.window.game_speed, 1.0);
    assert!(c.window.parallel_threads, "game threads run in parallel unless switched off");
    assert!(c.controller, "controllers are on unless switched off");
    assert!(!c.show_dpad, "the game's D-pad is hidden unless switched on");
    assert!(!c.pad_tile_cursor, "the controller cursor glides unless switched on");
    assert_eq!(c.keys.hide_ui.to_string(), "H");
    assert_eq!(crate::window::FpsPosition::TopLeft.flag(), "tl");
    let mut c = LaunchConfig::new("a", "d");
    c.window.show_fps = true;
    c.window.fps_limit = 144;
    c.window.fps_position = crate::window::FpsPosition::BottomRight;
    c.window.game_speed = 0.75;
    c.controller = false;
    c.show_dpad = true;
    c.pad_tile_cursor = true;
    let text = c.to_toml();
    assert!(text.contains("pad_tile_cursor = true"), "{text}");
    assert!(text.contains("controller = false"), "{text}");
    assert!(text.contains("show_dpad = true") && !text.contains("show_game_buttons"), "{text}");
    assert!(text.contains("game_speed = 0.75"), "{text}");
    assert!(text.contains("fps_position = \"bottom-right\""), "{text}");
    assert!(text.contains("show_fps = true") && text.contains("fps_limit = 144"), "{text}");
    assert_eq!(LaunchConfig::from_toml(&text).unwrap(), c);
}

#[test]
fn config_toml_and_files() {
    let t = tempfile::tempdir().unwrap();
    let mut c = LaunchConfig::new("/x/game.apk", t.path().join("data"));
    c.force = true;
    c.keys.back = Binding::parse("Q").unwrap();
    let text = c.to_toml();
    assert!(text.contains("back = \"Q\""));
    assert_eq!(LaunchConfig::from_toml(&text).unwrap(), c);
    // Minimal file: defaults fill in.
    let min = LaunchConfig::from_toml("apk = \"a\"\ndata_dir = \"d\"\n").unwrap();
    assert_eq!(min, LaunchConfig::new("a", "d"));
    assert!(matches!(LaunchConfig::from_toml("apk = 3"), Err(LauncherError::Parse(_))));
    assert!(matches!(
        LaunchConfig::from_toml("apk=\"a\"\ndata_dir=\"d\"\n[keys]\nback=\"Hyper+Q\""),
        Err(LauncherError::Parse(_))
    ));

    let path = t.path().join("sub/launcher.toml");
    assert_eq!(LaunchConfig::load(&path).unwrap(), None);
    c.save(&path).unwrap();
    assert_eq!(LaunchConfig::load(&path).unwrap(), Some(c.clone()));
    // A directory in place of the file: read fails with Io; save to a path below a file fails too.
    assert!(matches!(LaunchConfig::load(t.path()), Err(LauncherError::Io(_))));
    assert!(matches!(c.save(&path.join("x/y.toml")), Err(LauncherError::Io(_))));

    assert_eq!(c.data_dirs().root, c.data_dir);
    assert_eq!(c.run_args()[..4], ["--apk", "/x/game.apk", "--data-dir", c.data_dir.to_str().unwrap()]);
    assert!(c.run_args().contains(&"--force".to_string()));
    let _ = LaunchConfig::default_path();
    let _ = LaunchConfig::default_data_dir();
}

#[test]
fn config_errors_display_and_validate() {
    let t = tempfile::tempdir().unwrap();
    let apk = fake_apk(t.path());
    let mut c = LaunchConfig::new(&apk, t.path().join("d"));
    assert!(matches!(c.validate(), Err(LauncherError::Apk(_))));
    c.force = true;
    assert_eq!(c.validate().unwrap().release, None);
    c.window.width = 1;
    assert!(matches!(c.validate(), Err(LauncherError::Window(_))));
    c.window = WindowOptions::default();
    c.keys.pan = c.keys.touch;
    assert!(matches!(c.validate(), Err(LauncherError::Conflict(_))));
    for e in [
        LauncherError::Io("a".into()),
        LauncherError::Parse("a".into()),
        LauncherError::Apk(ApkError::NotZip),
        LauncherError::Window("a".into()),
        LauncherError::Conflict("a".into()),
        LauncherError::NoDefaultDir,
    ] {
        assert!(!e.to_string().is_empty());
    }
}

#[test]
fn path_mapping() {
    let d = DataDirs::new("/r");
    let p = |s| map_path(&d, s);
    assert_eq!(p("files/saves/w1/data"), Some(PathBuf::from("/r/home/saves/w1/data")));
    assert_eq!(p("data/data/com.noodlecake.blockheads/files/a.txt"), Some(PathBuf::from("/r/home/a.txt")));
    assert_eq!(p("com.noodlecake.blockheads/shared_prefs/p.xml"), Some(PathBuf::from("/r/appdata/shared_prefs/p.xml")));
    assert_eq!(p("databases/x.db"), Some(PathBuf::from("/r/appdata/databases/x.db")));
    assert_eq!(p("saves\\w\\f"), Some(PathBuf::from("/r/home/saves/w/f")));
    assert_eq!(p("./a//b"), Some(PathBuf::from("/r/home/a/b")));
    for none in
        ["cache/x", "code_cache/x", "../x", "files/../../x", "C:/x", "", "files", "files/", "com.noodlecake.blockheads"]
    {
        assert_eq!(p(none), None, "{none}");
    }
}

#[test]
fn import_folder_and_zip() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("pull");
    std::fs::create_dir_all(src.join("files/saves/w1")).unwrap();
    std::fs::create_dir_all(src.join("cache")).unwrap();
    std::fs::write(src.join("files/saves/w1/data"), b"world").unwrap();
    std::fs::write(src.join("cache/junk"), b"x").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/hostname", src.join("files/link")).unwrap();
    let dirs = DataDirs::new(t.path().join("data"));

    let r = import_saves(&src, &dirs, Existing::Skip).unwrap();
    let dest = dirs.home.join("saves/w1/data");
    assert_eq!(std::fs::read(&dest).unwrap(), b"world");
    assert_eq!(r.written, vec![dest.clone()]);
    assert_eq!(r.ignored, if cfg!(unix) { 2 } else { 1 });

    std::fs::write(src.join("files/saves/w1/data"), b"newer").unwrap();
    let r = import_saves(&src, &dirs, Existing::Skip).unwrap();
    assert_eq!((r.skipped_existing, r.written.len()), (1, 0));
    assert_eq!(std::fs::read(&dest).unwrap(), b"world");
    let r = import_saves(&src, &dirs, Existing::Overwrite).unwrap();
    assert_eq!(r.written.len(), 1);
    assert_eq!(std::fs::read(&dest).unwrap(), b"newer");

    let z = t.path().join("pull.zip");
    make_zip(
        &z,
        &[
            ("com.noodlecake.blockheads/", b""),
            ("com.noodlecake.blockheads/files/s/a", b"A"),
            ("../evil", b"e"),
            ("cache/c", b"c"),
        ],
    );
    let dirs2 = DataDirs::new(t.path().join("data2"));
    let r = import_saves(&z, &dirs2, Existing::Skip).unwrap();
    assert_eq!(std::fs::read(dirs2.home.join("s/a")).unwrap(), b"A");
    assert_eq!(r.ignored, 2);
    assert!(!t.path().join("evil").exists());
}

#[test]
fn import_errors() {
    let t = tempfile::tempdir().unwrap();
    let dirs = DataDirs::new(t.path().join("data"));
    assert_eq!(import_saves(&t.path().join("none"), &dirs, Existing::Skip), Err(ImportError::Source));
    let bad = t.path().join("bad.zip");
    std::fs::write(&bad, b"not a zip").unwrap();
    assert!(matches!(import_saves(&bad, &dirs, Existing::Skip), Err(ImportError::NotZip(_))));
    // Corrupt entry data: a stored entry whose body is truncated cannot be read.
    let z = t.path().join("c.zip");
    make_zip(&z, &[("files/a", b"0123456789")]);
    let mut bytes = std::fs::read(&z).unwrap();
    let at = bytes.windows(10).position(|w| w == b"0123456789").unwrap();
    bytes[at] = b'X'; // CRC mismatch surfaces while copying
    std::fs::write(&z, bytes).unwrap();
    assert!(matches!(import_saves(&z, &dirs, Existing::Overwrite), Err(ImportError::Io(_))));
    // The data dir cannot be created below a file.
    let file = t.path().join("file");
    std::fs::write(&file, b"").unwrap();
    assert!(matches!(import_saves(&z, &DataDirs::new(file.join("x")), Existing::Skip), Err(ImportError::Io(_))));
    for e in [ImportError::Io("a".into()), ImportError::NotZip("a".into()), ImportError::Source] {
        assert!(!e.to_string().is_empty());
    }
}

#[test]
fn walk_unreadable_directory() {
    let t = tempfile::tempdir().unwrap();
    let dirs = DataDirs::new(t.path().join("data"));
    let src = t.path().join("src");
    std::fs::create_dir_all(src.join("files")).unwrap();
    // A file whose parent in the destination is a file: create_dir_all fails.
    std::fs::write(src.join("files/a"), b"1").unwrap();
    std::fs::create_dir_all(&dirs.home).unwrap();
    std::fs::write(dirs.home.join("a"), b"").unwrap();
    // dest exists -> skipped; remove to hit nothing special. Use a dir at the destination: create fails.
    std::fs::remove_file(dirs.home.join("a")).unwrap();
    std::fs::create_dir(dirs.home.join("a")).unwrap();
    assert!(matches!(import_saves(&src, &dirs, Existing::Overwrite), Err(ImportError::Io(_))));
    // Parent blocked by a file.
    std::fs::create_dir_all(src.join("files/b")).unwrap();
    std::fs::write(src.join("files/b/c"), b"1").unwrap();
    std::fs::remove_file(src.join("files/a")).unwrap();
    std::fs::write(dirs.home.join("b"), b"").unwrap();
    assert!(matches!(import_saves(&src, &dirs, Existing::Overwrite), Err(ImportError::Io(_))));
}

#[test]
fn wizard_flow() {
    let t = tempfile::tempdir().unwrap();
    let cfg_path = t.path().join("c/launcher.toml");
    let apk_path = fake_apk(t.path());
    let mut w = Wizard::new(None, cfg_path.clone(), t.path().join("data"));
    assert_eq!(w.step, Step::Apk);
    assert!(!w.can_advance());
    w.advance();
    assert_eq!(w.step, Step::Apk);
    w.back();
    assert_eq!(w.step, Step::Apk);

    assert!(!w.pick_apk(&apk_path));
    assert!(w.error.as_ref().unwrap().contains("unknown"));
    w.config.force = true;
    assert!(w.pick_apk(&apk_path));
    assert!(w.error.is_none() && w.apk_info.is_some());

    for s in [Step::DataDir, Step::Keys, Step::Window, Step::Import, Step::Done, Step::Done] {
        w.advance();
        assert_eq!(w.step, s);
    }
    for s in [Step::Import, Step::Window, Step::Keys, Step::DataDir, Step::Apk] {
        w.back();
        assert_eq!(w.step, s);
    }
    w.step = Step::Window;
    w.config.window.width = 1;
    assert!(!w.can_advance());
    w.config.window.width = 800;
    assert!(w.can_advance());

    w.set_data_dir(t.path().join("data2"));
    assert!(!w.rebind(Action::Back, "Hyper+Q"));
    assert!(w.error.as_ref().unwrap().contains("Hyper"));
    assert!(!w.rebind(Action::Back, "Left"));
    assert!(w.error.as_ref().unwrap().contains("already used"));
    assert!(w.rebind(Action::Back, "Q"));
    assert!(w.rebind(Action::Back, "Q")); // rebinding to its own value is fine

    assert!(!w.import(&t.path().join("nope"), Existing::Skip));
    assert!(w.error.is_some());
    let src = t.path().join("src");
    std::fs::create_dir_all(src.join("files")).unwrap();
    std::fs::write(src.join("files/x"), b"1").unwrap();
    assert!(w.import(&src, Existing::Skip));
    assert_eq!(w.import_report.as_ref().unwrap().written.len(), 1);

    let done = w.finish().unwrap();
    assert_eq!(LaunchConfig::load(&cfg_path).unwrap(), Some(done));

    let mut bad = Wizard::new(None, cfg_path, t.path().join("data"));
    assert!(bad.finish().is_none());
    assert!(bad.error.is_some());
    let again = Wizard::new(Some(bad.config.clone()), t.path().join("x"), PathBuf::new());
    assert_eq!(again.step, Step::Done);
}

#[test]
fn bindings_become_input_triggers_and_flags() {
    use bhpc_input::{Input as I, MouseButton as M};
    let d = KeyBindings::default();
    assert_eq!(d.to_bindings(), bhpc_input::Bindings::default(), "the defaults are the hard-wired mapping");
    assert!(d.bind_flags().is_empty());
    let mut k = d.clone();
    k.set(Action::Touch, Binding::parse("Middle").unwrap());
    k.set(Action::Pan, Binding::parse("Shift+Right").unwrap());
    k.set(Action::Zoom, Binding::parse("Ctrl+Wheel").unwrap());
    k.set(Action::Back, Binding::parse("Q").unwrap());
    let b = k.to_bindings();
    assert_eq!(b.touch.input, I::Mouse(M::Middle));
    assert!(b.pan.mods.shift && b.zoom.mods.ctrl && !b.zoom.mods.alt);
    assert_eq!(b.back.input, I::Key(bhpc_input::HostKey::Char('q')));
    let flags = k.bind_flags();
    assert_eq!(
        flags,
        ["--bind", "touch=Middle", "--bind", "pan=Shift+Right", "--bind", "zoom=Ctrl+Wheel", "--bind", "back=Q"]
    );
    // The runner reads them back, one at a time.
    let mut back = KeyBindings::default();
    for f in flags.iter().filter(|f| *f != "--bind") {
        back.apply_flag(f).unwrap();
    }
    assert_eq!(back, k);
    assert_eq!(Action::from_name("pinch"), Some(Action::Pinch));
    assert_eq!(Action::from_name("jump"), None);
    for a in Action::ALL {
        assert_eq!(Action::from_name(a.name()), Some(a));
    }
    assert!(back.apply_flag("touch").unwrap_err().contains("ACTION=BINDING"));
    assert!(back.apply_flag("jump=Q").unwrap_err().contains("unknown action"));
    assert!(back.apply_flag("touch=Hyper+Q").unwrap_err().contains("invalid binding"));
}

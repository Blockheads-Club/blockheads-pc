//! The desktop side of links, mail, clipboard, notices, pages and photos (`desktop.rs`).

use crate::desktop::*;
use bhpc_jni::SystemHooks;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("bhpc-desktop-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// An opener that appends its target to `log` instead of opening anything.
fn recorder(log: &Path) -> Opener {
    Opener { argv: vec!["sh".into(), "-c".into(), format!("echo \"$0\" >> '{}'", log.display())] }
}

fn wait_lines(log: &Path, n: usize) -> Vec<String> {
    for _ in 0..200 {
        let lines: Vec<String> = std::fs::read_to_string(log).unwrap_or_default().lines().map(str::to_string).collect();
        if lines.len() >= n {
            return lines;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the opener did not run");
}

#[test]
fn opener_choice() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| pairs.iter().find(|p| p.0 == k).map(|p| p.1.to_string())
    };
    assert_eq!(Opener::from_env(&env(&[])), Opener::platform());
    assert_eq!(Opener::from_env(&env(&[("BROWSER", " ")])), Opener::platform());
    assert_eq!(Opener::from_env(&env(&[("BROWSER", "true")])).argv, ["true"]);
    assert_eq!(Opener::from_env(&env(&[("BROWSER", "firefox --new-tab:chromium")])).argv, ["firefox", "--new-tab"]);
    assert_eq!(Opener::from_env(&env(&[("BROWSER", "true"), ("BHPC_OPENER", "rec x")])).argv, ["rec", "x"]);
    let o = Opener { argv: vec!["b".into(), "--url=%s".into()] };
    assert_eq!(o.command_line("http://x"), ["b", "--url=http://x"]);
    assert_eq!(Opener { argv: vec!["b".into()] }.command_line("u"), ["b", "u"]);
    assert!(!Opener::platform().argv.is_empty());
}

#[test]
fn opener_runs_without_waiting() {
    let d = scratch("open");
    let log = d.join("log");
    assert!(recorder(&log).open("https://theblockheads.net"));
    assert_eq!(wait_lines(&log, 1), ["https://theblockheads.net"]);
    assert!(!Opener { argv: vec![] }.open("x"));
    assert!(!Opener { argv: vec!["/nonexistent/opener".into()] }.open("x"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn pictures_folder() {
    let none = |_: &str| None;
    let xdg = |k: &str| (k == "XDG_PICTURES_DIR").then(|| "/x/Pics".to_string());
    let blank = |_: &str| Some(String::new());
    assert_eq!(pictures_dir(&xdg, Some("/p".into()), Some("/h".into())), Some(PathBuf::from("/x/Pics")));
    assert_eq!(pictures_dir(&blank, Some("/p".into()), Some("/h".into())), Some(PathBuf::from("/p")));
    assert_eq!(pictures_dir(&none, None, Some("/h".into())), Some(PathBuf::from("/h/Pictures")));
    assert_eq!(pictures_dir(&none, None, None), None);
    let _ = user_pictures_dir();
    let p = Path::new("/home/u/Pictures");
    assert_eq!(host_picture(p, "/sdcard/Pictures/The Blockheads/1.png"), Some(p.join("The Blockheads/1.png")));
    assert_eq!(host_picture(p, "/storage/emulated/0/DCIM/2.png"), Some(p.join("2.png")));
    assert_eq!(host_picture(p, "/sdcard/Download/3.png"), None);
    assert_eq!(host_picture(p, "/data/x.png"), None);
    assert_eq!(file_url(Path::new("/a b/c.html")), "file:///a%20b/c.html");
    assert_eq!(file_url(Path::new("C:\\x\\y.html")), "file:///C:/x/y.html");
}

#[test]
fn desktop_hooks() {
    let d = scratch("hooks");
    let log = d.join("log");
    let said = Arc::new(Mutex::new(Vec::<String>::new()));
    let (s1, s2) = (said.clone(), said.clone());
    let desk = Desktop::new(
        recorder(&log),
        d.join("pages"),
        Some(d.join("Pictures")),
        Box::new(move |t| s1.lock().unwrap().push(format!("copy {t}"))),
        Box::new(move |t| s2.lock().unwrap().push(format!("say {t}"))),
    );
    assert!(desk.open_url("mailto:a@b?subject=x"));
    assert!(desk.show_html("<img src=\"img/a.png\">", &[("img/a.png".into(), b"png".to_vec())]));
    // The openers run on their own: either may finish first.
    let lines = wait_lines(&log, 2);
    assert!(lines.iter().any(|l| l == "mailto:a@b?subject=x"), "{lines:?}");
    let page = lines.iter().find_map(|l| l.strip_prefix("file://")).unwrap().to_string();
    // `file:///C:/...` on Windows: the path starts after the third slash.
    let page = if cfg!(windows) { page.trim_start_matches('/').to_string() } else { page };
    assert!(page.ends_with("/index.html"), "{page}");
    assert_eq!(std::fs::read_to_string(&page).unwrap(), "<img src=\"img/a.png\">");
    assert_eq!(std::fs::read(Path::new(&page).parent().unwrap().join("img/a.png")).unwrap(), b"png");
    desk.set_clipboard("1.2.3.4");
    desk.notify("HELLO");
    desk.picture_saved("/sdcard/Pictures/The Blockheads/9.png");
    desk.picture_saved("/sdcard/elsewhere.png");
    let pics = d.join("Pictures").join("The Blockheads").join("9.png").display().to_string();
    assert_eq!(
        *said.lock().unwrap(),
        [
            "copy 1.2.3.4".to_string(),
            "say HELLO".into(),
            format!("say PHOTO SAVED: {pics}"),
            "say PHOTO SAVED: /sdcard/elsewhere.png".into(),
        ]
    );
    // A page that cannot be written is reported, not opened.
    std::fs::write(d.join("blocker"), b"").unwrap();
    let stuck = Desktop::new(recorder(&log), d.join("blocker/pages"), None, Box::new(|_| {}), Box::new(|_| {}));
    assert!(!stuck.show_html("x", &[]));
    stuck.picture_saved("/sdcard/Pictures/1.png");
    let bad_file = Desktop::new(recorder(&log), d.join("p2"), None, Box::new(|_| {}), Box::new(|_| {}));
    std::fs::create_dir_all(d.join("p2")).unwrap();
    assert!(bad_file.write_page("x", &[("../../../../../../../../proc/nope/x".into(), vec![])]).is_err());
    // Nothing outside the page's folder, on any host.
    for name in ["/etc/x", "a//b", "", "C:x", "a\\b", "img/../../x"] {
        let e = bad_file.write_page("x", &[(name.into(), vec![])]).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput, "{name:?}");
    }
    // The page folder itself cannot be made: a plain error.
    let blocked = Desktop::new(recorder(&log), d.join("blocker/p3"), None, Box::new(|_| {}), Box::new(|_| {}));
    assert!(blocked.write_page("x", &[]).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

//! The picture picker with fake dialog programs (no real dialog is ever started).

use crate::picker::{candidates, message, pick_image, result_of, run, Candidate, Pick, Pickers};
use bhpc_jni::{ActivityRequest, RESULT_CANCELED, RESULT_OK};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn sh(script: &str) -> Candidate {
    Candidate { program: "sh".into(), args: vec!["-c".into(), script.into()] }
}

fn missing() -> Candidate {
    Candidate { program: "/nonexistent/bhpc-no-dialog".into(), args: vec![] }
}

#[test]
fn the_first_dialog_that_starts_answers() {
    assert_eq!(run(&[missing(), sh("printf '/p/a b.png\\nmore'")]), Pick::File(PathBuf::from("/p/a b.png")));
    assert_eq!(run(&[sh("exit 1"), sh("echo /never")]), Pick::Cancelled, "a cancel does not try the next one");
    assert_eq!(run(&[sh("true")]), Pick::Cancelled, "no file chosen");
    assert_eq!(run(&[missing()]), Pick::Unavailable);
    assert_eq!(run(&[]), Pick::Unavailable);
}

#[test]
fn kde_gets_its_own_dialog_first() {
    let names = |d: &str| candidates(d).into_iter().map(|c| c.program).collect::<Vec<_>>();
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        assert_eq!(names("GNOME"), ["zenity", "kdialog"]);
        assert_eq!(names("KDE"), ["kdialog", "zenity"]);
    }
    #[cfg(target_os = "macos")]
    assert_eq!(names("KDE"), ["osascript"]);
    #[cfg(windows)]
    assert_eq!(names("KDE"), ["powershell"]);
    assert!(candidates("").iter().all(|c| c.args.iter().any(|a| a.contains("png"))));
}

#[test]
fn a_fixed_choice_replaces_the_dialog() {
    let never = [sh("echo /not/asked")];
    assert_eq!(pick_image(Some("/x/y.jpg".into()), &never), Pick::File(PathBuf::from("/x/y.jpg")));
    assert_eq!(pick_image(Some("".into()), &never), Pick::Cancelled);
    assert_eq!(pick_image(None, &[sh("echo /asked.png")]), Pick::File(PathBuf::from("/asked.png")));
}

#[test]
fn picks_become_activity_results() {
    let (code, uri) = result_of(&Pick::File(PathBuf::from("/home/u/my pic.png")));
    assert_eq!((code, uri.as_deref()), (RESULT_OK, Some("file:///home/u/my%20pic.png")));
    assert_eq!(result_of(&Pick::Cancelled), (RESULT_CANCELED, None));
    assert_eq!(result_of(&Pick::Unavailable), (RESULT_CANCELED, None));
    assert_eq!(message(&Pick::Cancelled), None);
    assert!(message(&Pick::Unavailable).unwrap().contains("ZENITY"));
}

fn request(code: i32, action: &str) -> ActivityRequest {
    ActivityRequest {
        code,
        action: action.into(),
        data: Some("content://media/internal/images/media".into()),
        mime: None,
    }
}

#[test]
fn pictures_are_picked_on_a_thread_and_other_activities_cancelled() {
    let p = Pickers::default();
    p.start(&request(1, "android.intent.action.VIEW"), Box::new(|| Pick::File(PathBuf::from("/not/asked"))));
    p.start(&request(9001, "android.intent.action.PICK"), Box::new(|| Pick::File(PathBuf::from("/a.png"))));
    let mut got = Vec::new();
    let until = Instant::now() + Duration::from_secs(10);
    while got.len() < 2 && Instant::now() < until {
        got.extend(p.poll());
        std::thread::sleep(Duration::from_millis(5));
    }
    got.sort_by_key(|g| g.0);
    assert_eq!(got, [(1, Pick::Cancelled), (9001, Pick::File(PathBuf::from("/a.png")))]);
    assert!(p.poll().is_empty());
}

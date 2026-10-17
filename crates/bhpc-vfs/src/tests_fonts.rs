//! Tests of the font mapping.

use super::tests::apk;
use crate::error::VfsError;
use crate::fonts::*;
use crate::fs::{FileKind, FileSystem, OpenFlags};
use std::sync::Arc;

fn read(fs: &FontFs, name: &str) -> Vec<u8> {
    let mut h = fs.open(&format!("/{name}"), OpenFlags::read_only()).unwrap();
    let mut b = vec![0u8; h.len().unwrap() as usize];
    assert_eq!(h.read(&mut b), Ok(b.len()));
    b
}

#[test]
fn bundled_fonts_are_real_ttf_and_shared() {
    for s in [Style::Regular, Style::Bold, Style::Italic] {
        let f = bundled(s);
        assert_eq!(&f[..4], &[0, 1, 0, 0]);
        assert!(Arc::ptr_eq(&f, &bundled(s)));
    }
    assert!(!Arc::ptr_eq(&bundled(Style::Regular), &bundled(Style::Bold)));
}

#[test]
fn style_from_name() {
    assert_eq!(Style::of("Roboto-Regular.ttf"), Style::Regular);
    assert_eq!(Style::of("Roboto-Bold.ttf"), Style::Bold);
    assert_eq!(Style::of("Roboto-BoldItalic.ttf"), Style::Bold);
    assert_eq!(Style::of("DroidSans-Italic.ttf"), Style::Italic);
}

#[test]
fn every_known_name_resolves_without_apk_or_overrides() {
    let fs = FontFs::new(None, None);
    for n in FONT_NAMES {
        let st = fs.stat(&format!("/{n}")).unwrap();
        assert_eq!(st.kind, FileKind::File);
        assert!(st.size > 100_000, "{n}");
    }
    assert_eq!(read(&fs, "Roboto-Bold.ttf"), &*bundled(Style::Bold));
    assert_eq!(read(&fs, "DroidSerif-BoldItalic.ttf"), &*bundled(Style::Bold));
    assert_eq!(fs.stat("/Unknown.ttf"), Err(VfsError::NotFound));
    assert!(fs.resolve("a/b.ttf").is_none());
    assert!(fs.resolve("a\\b.ttf").is_none());
}

#[test]
fn apk_fonts_win_over_bundled_and_aliases_work() {
    let fs = FontFs::new(Some(apk()), None);
    assert_eq!(read(&fs, "DroidSansThai.ttf"), b"THAI");
    assert_eq!(read(&fs, "DroidSansHebrew.ttf"), b"HEBREW");
    assert_eq!(read(&fs, "DroidSansArabic.ttf"), b"NASKH");
    assert_eq!(read(&fs, "DroidNaskh-Regular-SystemUI.ttf"), b"NASKH");
    // Not in the APK: bundled.
    assert_eq!(read(&fs, "Roboto-Regular.ttf"), &*bundled(Style::Regular));
    let names: Vec<_> = fs.read_dir("/").unwrap().into_iter().map(|e| e.name).collect();
    assert!(names.contains(&"DroidNaskh-Regular-SystemUI.ttf".to_string()));
    assert!(names.contains(&"DroidSansHebrew-Regular.ttf".to_string()));
    assert!(names.contains(&"Roboto-Regular.ttf".to_string()));
}

#[test]
fn user_overrides_win() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("Roboto-Regular.ttf"), b"MINE").unwrap();
    std::fs::write(d.path().join("Extra.ttf"), b"EXTRA").unwrap();
    std::fs::create_dir(d.path().join("subdir")).unwrap();
    let fs = FontFs::new(Some(apk()), Some(d.path().to_path_buf()));
    assert_eq!(read(&fs, "Roboto-Regular.ttf"), b"MINE");
    assert_eq!(read(&fs, "Extra.ttf"), b"EXTRA");
    let names = fs.names();
    assert!(names.contains(&"Extra.ttf".to_string()));
    assert!(!names.contains(&"subdir".to_string()));
    // A missing override directory is harmless.
    let none = FontFs::new(None, Some(d.path().join("missing")));
    assert!(none.names().contains(&"Roboto-Bold.ttf".to_string()));
}

#[test]
fn font_dir_semantics() {
    let fs = FontFs::new(None, None);
    assert_eq!(fs.stat("/").unwrap().kind, FileKind::Dir);
    assert_eq!(fs.open("/", OpenFlags::read_only()).err(), Some(VfsError::IsDir));
    assert_eq!(
        fs.open("/Roboto-Regular.ttf", OpenFlags { write: true, ..Default::default() }).err(),
        Some(VfsError::ReadOnly)
    );
    for f in [
        OpenFlags { create: true, ..Default::default() },
        OpenFlags { truncate: true, ..Default::default() },
        OpenFlags { append: true, ..Default::default() },
    ] {
        assert_eq!(fs.open("/Roboto-Regular.ttf", f).err(), Some(VfsError::ReadOnly));
    }
    assert_eq!(fs.open("/nope.ttf", OpenFlags::read_only()).err(), Some(VfsError::NotFound));
    assert_eq!(fs.open("/a/b", OpenFlags::read_only()).err(), Some(VfsError::NotFound));
    assert_eq!(fs.read_dir("/Roboto-Regular.ttf"), Err(VfsError::NotDir));
    assert_eq!(fs.read_dir("/").unwrap().len(), FONT_NAMES.len());
}

#[test]
fn generated_xml_lists_only_resolvable_files() {
    let fs = FontFs::new(None, None);
    let mut all = String::new();
    for x in [system_fonts_xml(), fallback_fonts_xml(), vendor_fallback_fonts_xml()] {
        assert!(x.starts_with("<?xml") && x.contains("<familyset version=\"1\">"));
        all.push_str(&x);
    }
    let sys = system_fonts_xml();
    assert!(sys.contains("<name>sans-serif</name>") && sys.contains("<file>Roboto-Bold.ttf</file>"));
    assert!(system_fonts_xml().matches("<family>").count() == 4);
    assert!(!vendor_fallback_fonts_xml().contains("<family>"));
    let mut files = 0;
    for part in all.split("<file>").skip(1) {
        let name = part.split("</file>").next().unwrap();
        assert!(fs.resolve(name).is_some(), "{name}");
        files += 1;
    }
    assert!(files >= 10);
    // The fonts Apportable's hard-coded fallback list names are all present.
    for n in [
        "DroidSans.ttf",
        "DroidSans-Bold.ttf",
        "DroidSansArabic.ttf",
        "DroidSansFallback.ttf",
        "DroidSansHebrew.ttf",
        "DroidSansMono.ttf",
        "DroidSansThai.ttf",
        "DroidSerif-Regular.ttf",
        "DroidSerif-Bold.ttf",
        "DroidSerif-Italic.ttf",
        "DroidSerif-BoldItalic.ttf",
    ] {
        assert!(fs.resolve(n).is_some(), "{n}");
    }
}

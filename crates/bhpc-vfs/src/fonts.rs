//! System fonts for Apportable's CoreGraphics (see `docs/fonts.md` in this crate).
//!
//! `libCoreGraphics.so` builds its font database (`_CGFontDBCreateSystem`) by parsing the
//! Android 4.x font configuration:
//!
//! * `/system/etc/system_fonts.xml`, `/system/etc/fallback_fonts.xml`, `/vendor/etc/fallback_fonts.xml`
//!   (read with libxml2's text reader; only `<name>` and `<file>` text matters), and
//! * the files named there, resolved as `/system/fonts/<file>`.
//!
//! When the XML is missing it logs `CGFONTDB: SYSTEM_FONTS path is not found! Using fallback
//! hardcoded system fonts` and tries the hard-coded `DroidSans*.ttf` / `DroidSerif-*.ttf` list;
//! on a device that lacks those files the IAP UI crashed. Here every name resolves, always:
//!
//! 1. a user override in `<data>/fonts/<name>`;
//! 2. the matching font shipped inside the user's APK (`com.apportable.system/fonts/`, Droid
//!    Naskh/Hebrew/Thai), through a small alias table;
//! 3. a bundled SIL-OFL font (Liberation Sans, regular/bold/italic) chosen by the style in the name.

use crate::archive::ApkArchive;
use crate::asset::Asset;
use crate::error::{Result, VfsError};
use crate::fs::{DirEntry, FileKind, FileSystem, OpenFlags, Stat, VFile};
use crate::path::components;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

static REGULAR: &[u8] = include_bytes!("../fonts/LiberationSans-Regular.ttf");
static BOLD: &[u8] = include_bytes!("../fonts/LiberationSans-Bold.ttf");
static ITALIC: &[u8] = include_bytes!("../fonts/LiberationSans-Italic.ttf");

/// Zip directory (inside the APK) that holds Apportable's own system fonts.
pub const APK_FONT_DIR: &str = "com.apportable.system/fonts";

/// Every file name the guest may ask for under `/system/fonts/`.
pub const FONT_NAMES: &[&str] = &[
    "DroidSans.ttf",
    "DroidSans-Bold.ttf",
    "DroidSans-Italic.ttf",
    "DroidSansArabic.ttf",
    "DroidSansFallback.ttf",
    "DroidSansHebrew.ttf",
    "DroidSansHebrew-Bold.ttf",
    "DroidSansMono.ttf",
    "DroidSansThai.ttf",
    "DroidSerif-Regular.ttf",
    "DroidSerif-Bold.ttf",
    "DroidSerif-Italic.ttf",
    "DroidSerif-BoldItalic.ttf",
    "Roboto-Regular.ttf",
    "Roboto-Bold.ttf",
    "Roboto-Italic.ttf",
    "Roboto-BoldItalic.ttf",
    "RobotoCondensed-Regular.ttf",
    "RobotoCondensed-Bold.ttf",
    "RobotoCondensed-Italic.ttf",
    "RobotoCondensed-BoldItalic.ttf",
];

/// Names that the APK ships under a different file name.
const APK_ALIASES: &[(&str, &str)] = &[
    ("DroidSansHebrew.ttf", "DroidSansHebrew-Regular.ttf"),
    ("DroidSansArabic.ttf", "DroidNaskh-Regular-SystemUI.ttf"),
];

/// Style picked from a font file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Regular weight.
    Regular,
    /// Bold (also used for bold italic: no bundled bold italic).
    Bold,
    /// Italic.
    Italic,
}

impl Style {
    /// `Bold` wins over `Italic`, everything else is regular.
    pub fn of(name: &str) -> Style {
        if name.contains("Bold") {
            Style::Bold
        } else if name.contains("Italic") {
            Style::Italic
        } else {
            Style::Regular
        }
    }
}

/// The bundled OFL font for a style, shared by every caller.
pub fn bundled(style: Style) -> Arc<[u8]> {
    static CELL: OnceLock<[Arc<[u8]>; 3]> = OnceLock::new();
    let all = CELL.get_or_init(|| [REGULAR.into(), BOLD.into(), ITALIC.into()]);
    match style {
        Style::Regular => all[0].clone(),
        Style::Bold => all[1].clone(),
        Style::Italic => all[2].clone(),
    }
}

/// One family of the generated `system_fonts.xml`.
struct Family {
    names: &'static [&'static str],
    files: &'static [&'static str],
}

const FAMILIES: &[Family] = &[
    Family {
        names: &["sans-serif", "arial", "helvetica", "tahoma", "verdana"],
        files: &["Roboto-Regular.ttf", "Roboto-Bold.ttf", "Roboto-Italic.ttf", "Roboto-BoldItalic.ttf"],
    },
    Family {
        names: &["sans-serif-condensed"],
        files: &[
            "RobotoCondensed-Regular.ttf",
            "RobotoCondensed-Bold.ttf",
            "RobotoCondensed-Italic.ttf",
            "RobotoCondensed-BoldItalic.ttf",
        ],
    },
    Family {
        names: &["serif", "times", "times new roman", "palatino", "georgia", "baskerville"],
        files: &["DroidSerif-Regular.ttf", "DroidSerif-Bold.ttf", "DroidSerif-Italic.ttf", "DroidSerif-BoldItalic.ttf"],
    },
    Family { names: &["monospace", "courier", "courier new", "monaco"], files: &["DroidSansMono.ttf"] },
];

const FALLBACK_FILES: &[&str] =
    &["DroidSansArabic.ttf", "DroidSansHebrew.ttf", "DroidSansThai.ttf", "DroidSansFallback.ttf"];

/// Guest path of the main font configuration.
pub const SYSTEM_FONTS_XML: &str = "/system/etc/system_fonts.xml";
/// Guest path of the fallback chain.
pub const FALLBACK_FONTS_XML: &str = "/system/etc/fallback_fonts.xml";
/// Guest path of the vendor fallback chain (empty here).
pub const VENDOR_FALLBACK_FONTS_XML: &str = "/vendor/etc/fallback_fonts.xml";

/// Generated `/system/etc/system_fonts.xml`.
pub fn system_fonts_xml() -> String {
    let mut s = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<familyset version=\"1\">\n");
    for fam in FAMILIES {
        s.push_str("  <family>\n    <nameset>\n");
        for n in fam.names {
            s.push_str(&format!("      <name>{n}</name>\n"));
        }
        s.push_str("    </nameset>\n    <fileset>\n");
        for f in fam.files {
            s.push_str(&format!("      <file>{f}</file>\n"));
        }
        s.push_str("    </fileset>\n  </family>\n");
    }
    s.push_str("</familyset>\n");
    s
}

/// Generated `/system/etc/fallback_fonts.xml`.
pub fn fallback_fonts_xml() -> String {
    let mut s = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<familyset version=\"1\">\n");
    for f in FALLBACK_FILES {
        s.push_str(&format!("  <family>\n    <fileset>\n      <file>{f}</file>\n    </fileset>\n  </family>\n"));
    }
    s.push_str("</familyset>\n");
    s
}

/// Generated `/vendor/etc/fallback_fonts.xml`: present but empty.
pub fn vendor_fallback_fonts_xml() -> String {
    "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<familyset version=\"1\">\n</familyset>\n".to_string()
}

/// The `/system/fonts` directory. Mount it at `/system/fonts`.
pub struct FontFs {
    apk: Option<Arc<ApkArchive>>,
    user_dir: Option<PathBuf>,
}

impl FontFs {
    /// `apk` supplies Apportable's own fonts, `user_dir` holds optional overrides.
    pub fn new(apk: Option<Arc<ApkArchive>>, user_dir: Option<PathBuf>) -> Self {
        FontFs { apk, user_dir }
    }

    fn user_file(&self, name: &str) -> Option<Arc<[u8]>> {
        let dir = self.user_dir.as_ref()?;
        fs::read(dir.join(name)).ok().map(Into::into)
    }

    fn apk_file(&self, name: &str) -> Option<Arc<[u8]>> {
        let apk = self.apk.as_ref()?;
        let real = APK_ALIASES.iter().find(|(a, _)| *a == name).map_or(name, |(_, r)| *r);
        apk.read(&format!("{APK_FONT_DIR}/{real}")).ok()
    }

    /// Resolves a file name using the three-step order of the module docs.
    pub fn resolve(&self, name: &str) -> Option<Arc<[u8]>> {
        if name.contains(['/', '\\']) {
            return None;
        }
        self.user_file(name)
            .or_else(|| self.apk_file(name))
            .or_else(|| FONT_NAMES.contains(&name).then(|| bundled(Style::of(name))))
    }

    /// Every name that [`FontFs::resolve`] can answer.
    pub fn names(&self) -> Vec<String> {
        let mut set: BTreeSet<String> = FONT_NAMES.iter().map(|s| s.to_string()).collect();
        if let Some(apk) = &self.apk {
            for (n, d) in apk.list(APK_FONT_DIR).unwrap_or_default() {
                if !d {
                    set.insert(n);
                }
            }
        }
        if let Some(dir) = &self.user_dir {
            for e in fs::read_dir(dir).into_iter().flatten().flatten() {
                if e.path().is_file() {
                    set.insert(e.file_name().to_string_lossy().into_owned());
                }
            }
        }
        set.into_iter().collect()
    }
}

impl FileSystem for FontFs {
    fn open(&self, path: &str, flags: OpenFlags) -> Result<Box<dyn VFile>> {
        if flags.write || flags.create || flags.truncate || flags.append {
            return Err(VfsError::ReadOnly);
        }
        match single(path)? {
            None => Err(VfsError::IsDir),
            Some(name) => self.resolve(name).map(|d| Box::new(Asset::new(d)) as _).ok_or(VfsError::NotFound),
        }
    }

    fn stat(&self, path: &str) -> Result<Stat> {
        match single(path)? {
            None => Ok(Stat::dir()),
            Some(name) => self.resolve(name).map(|d| Stat::file(d.len() as u64)).ok_or(VfsError::NotFound),
        }
    }

    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        match single(path)? {
            None => Ok(self.names().into_iter().map(|name| DirEntry { name, kind: FileKind::File }).collect()),
            Some(_) => Err(VfsError::NotDir),
        }
    }
}

/// `"/"` is the directory itself (`None`); `"/x"` is the file `x`; deeper paths do not exist.
fn single(path: &str) -> Result<Option<&str>> {
    let mut it = components(path);
    match (it.next(), it.next()) {
        (None, _) => Ok(None),
        (Some(n), None) => Ok(Some(n)),
        _ => Err(VfsError::NotFound),
    }
}

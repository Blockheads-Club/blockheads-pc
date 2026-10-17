//! What the fake Java world reports about the PC it runs on, and where assets come from.

/// Read access to the APK's `assets/` tree (and, if the host wants, the OBB). The VFS crate
/// implements this; the fake `AssetManager` is a thin wrapper around it. Paths are relative to
/// the asset root, without a leading slash, with `/` separators.
pub trait Assets: Send + Sync {
    /// The whole file, or `None` if it does not exist.
    fn read(&self, path: &str) -> Option<Vec<u8>>;
    /// Names (not paths) of the entries directly below `dir` (`""` is the root); `None` if `dir`
    /// is not a directory.
    fn list(&self, dir: &str) -> Option<Vec<String>>;
}

/// An asset tree with nothing in it.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoAssets;

impl Assets for NoAssets {
    fn read(&self, _path: &str) -> Option<Vec<u8>> {
        None
    }
    fn list(&self, _dir: &str) -> Option<Vec<String>> {
        None
    }
}

/// An in-memory asset tree (used by tests and by hosts that preload a few files).
#[derive(Debug, Default, Clone)]
pub struct MemAssets(pub Vec<(String, Vec<u8>)>);

impl Assets for MemAssets {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.iter().find(|(p, _)| p == path).map(|(_, d)| d.clone())
    }
    fn list(&self, dir: &str) -> Option<Vec<String>> {
        let prefix = if dir.is_empty() { String::new() } else { format!("{}/", dir.trim_end_matches('/')) };
        let mut names: Vec<String> = Vec::new();
        for (p, _) in &self.0 {
            if let Some(rest) = p.strip_prefix(&prefix) {
                let first = rest.split('/').next().unwrap_or_default().to_string();
                if !names.contains(&first) {
                    names.push(first);
                }
            }
        }
        if names.is_empty() && !dir.is_empty() {
            None
        } else {
            Some(names)
        }
    }
}

/// What `VerdeActivity` asks of the window system. The GL layer implements it; the defaults are
/// harmless so the fake JVM works headless.
pub trait GfxHooks: Send + Sync {
    /// Present the frame (`VerdeActivity.swapBuffers`).
    fn swap_buffers(&self) -> bool {
        true
    }
    /// Make the context with this id current on the calling thread (0 releases it).
    fn make_current(&self, _ctx: u32) -> bool {
        true
    }
    /// Create a context sharing resources with the primary one; returns its id (never 0).
    fn create_shared_context(&self, _version: i32) -> u32 {
        2
    }
    fn destroy_context(&self, _ctx: u32) -> bool {
        true
    }
    /// `VerdeActivity.setContentSize`: the game's logical content size changed.
    fn set_content_size(&self, _w: f32, _h: f32) {}
    /// `VerdeActivity.rektEGLContext`: the game gave up on the context.
    fn context_lost(&self) {}
}

/// [`GfxHooks`] with every default.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoGfx;

impl GfxHooks for NoGfx {}

/// PC-side values for everything the Java layer reports. All paths are host paths; the guest
/// libraries see them verbatim (the fake libc passes them to the host file system).
#[derive(Debug, Clone, PartialEq)]
pub struct Platform {
    /// Screen size in pixels (portrait: width < height is what the game was designed for).
    pub screen_width: i32,
    pub screen_height: i32,
    /// Android `densityDpi` (160 = mdpi, 320 = xhdpi).
    pub density_dpi: i32,
    pub refresh_rate: f32,
    /// `ll_CC` (the tag is split into `Locale.getLanguage()` and `getCountry()`).
    pub locale: String,
    pub timezone: String,
    pub package_name: String,
    pub version_name: String,
    pub version_code: i32,
    /// `Build.VERSION.SDK_INT`.
    pub sdk_int: i32,
    /// `Build.VERSION.RELEASE`.
    pub release: String,
    pub model: String,
    pub manufacturer: String,
    pub android_id: String,
    /// `Context.getFilesDir()`.
    pub files_dir: String,
    /// `Context.getCacheDir()`.
    pub cache_dir: String,
    /// `Context.getExternalFilesDir()` / `Environment.getExternalStorageDirectory()` base.
    pub external_dir: String,
    /// `Context.getObbDir()`.
    pub obb_dir: String,
    /// `Context.getPackageCodePath()` (the APK).
    pub apk_path: String,
}

impl Default for Platform {
    fn default() -> Platform {
        let home = std::env::var("HOME").unwrap_or("/tmp".to_string());
        let base = format!("{home}/.local/share/blockheads-pc");
        Platform {
            screen_width: 720,
            screen_height: 1280,
            density_dpi: 320,
            refresh_rate: 60.0,
            locale: "en_US".into(),
            timezone: "UTC".into(),
            package_name: "com.majicjungle.blockheads".into(),
            version_name: "1.7.4".into(),
            version_code: 0,
            sdk_int: 19,
            release: "4.4.4".into(),
            model: "bhpc".into(),
            manufacturer: "bhpc".into(),
            android_id: "0123456789abcdef".into(),
            files_dir: format!("{base}/files"),
            cache_dir: format!("{base}/cache"),
            external_dir: format!("{base}/sdcard"),
            obb_dir: format!("{base}/obb"),
            apk_path: String::new(),
        }
    }
}

impl Platform {
    /// `density` as a float scale factor (`densityDpi / 160`).
    pub fn density(&self) -> f32 {
        self.density_dpi as f32 / 160.0
    }

    /// `(language, country)` of [`Platform::locale`].
    pub fn language_country(&self) -> (String, String) {
        let tag = self.locale.split(['.', '@']).next().unwrap_or_default();
        match tag.split_once(['_', '-']) {
            Some((l, c)) => (l.to_string(), c.to_string()),
            None => (tag.to_string(), String::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assets_trees() {
        assert_eq!(NoAssets.read("a"), None);
        assert_eq!(NoAssets.list(""), None);
        let a = MemAssets(vec![
            ("a.txt".into(), b"A".to_vec()),
            ("d/b.txt".into(), b"B".to_vec()),
            ("d/e/c.txt".into(), b"C".to_vec()),
            ("d/e/f.txt".into(), b"F".to_vec()),
        ]);
        assert_eq!(a.read("d/b.txt"), Some(b"B".to_vec()));
        assert_eq!(a.read("zz"), None);
        assert_eq!(a.list(""), Some(vec!["a.txt".to_string(), "d".to_string()]));
        assert_eq!(a.list("d/"), Some(vec!["b.txt".to_string(), "e".to_string()]));
        assert_eq!(a.list("d/e"), Some(vec!["c.txt".to_string(), "f.txt".to_string()]));
        assert_eq!(a.list("nope"), None);
        assert_eq!(MemAssets::default().list(""), Some(vec![]));
    }

    #[test]
    fn platform_helpers() {
        let mut p = Platform::default();
        assert_eq!(p.density(), 2.0);
        assert_eq!(p.language_country(), ("en".into(), "US".into()));
        p.locale = "de".into();
        assert_eq!(p.language_country(), ("de".into(), String::new()));
        p.locale = "pt-BR.UTF-8".into();
        assert_eq!(p.language_country(), ("pt".into(), "BR".into()));
        assert!(p.files_dir.ends_with("/files"));
    }
}

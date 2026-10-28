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

/// The APK's `assets/` tree as served by `bhpc-vfs`.
impl Assets for bhpc_vfs::AssetManager {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.open(path).ok().map(|a| a.buffer().to_vec())
    }
    fn list(&self, dir: &str) -> Option<Vec<String>> {
        self.list(dir).ok().map(|v| v.into_iter().map(|(name, _)| name).collect())
    }
}

/// A shared asset tree.
impl<T: Assets + ?Sized> Assets for std::sync::Arc<T> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        (**self).read(path)
    }
    fn list(&self, dir: &str) -> Option<Vec<String>> {
        (**self).list(dir)
    }
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

/// What `android.media.AudioTrack` (MODE_STREAM) does on the host. Track ids are chosen by the implementation.
pub trait AudioHooks: Send + Sync {
    /// `AudioTrack.getMinBufferSize`; negative for an unsupported format.
    fn min_buffer_size(&self, rate: i32, channel_config: i32, encoding: i32) -> i32;
    /// `new AudioTrack(...)`: `None` makes the Java object report `STATE_UNINITIALIZED`.
    fn create(&self, rate: i32, channel_config: i32, encoding: i32, buffer_bytes: i32) -> Option<u32>;
    /// `play`/`pause`/`stop`/`flush`/`release` as [`TrackOp`]; 0 on success.
    fn control(&self, id: u32, op: TrackOp) -> i32;
    /// `write(byte[], off, size)`: may wait for room (and must give other guest threads the CPU while it does).
    /// Returns the bytes consumed.
    fn write(&self, cpu: &mut bhpc_cpu::Cpu, id: u32, data: &[u8]) -> i32;
    /// `getPlaybackHeadPosition`.
    fn head_position(&self, id: u32) -> i32;
    /// `com.apportable.media.AudioPlayer.initAudio(path)` (the `MediaPlayer` behind `AVAudioPlayer`, which plays the
    /// game's music). `asset` is the file when `path` is relative and names an APK asset (`GameResources/x.mp4`, as
    /// the game passes it); otherwise the host reads the guest path. The player id (never 0), or `None` if it
    /// cannot be opened.
    fn music_open(&self, _path: &str, _asset: Option<Vec<u8>>) -> Option<u32> {
        None
    }
    /// One command or query on a music player: 0 (or the queried value) on success, negative on failure.
    fn music(&self, _id: u32, _op: MusicOp) -> i32 {
        -1
    }
    /// `AudioPlayer.lifecycleSuspend` (`true`) / `lifecycleResume` (`false`): pause every playing player, and
    /// start again the ones that were paused that way.
    fn music_suspend(&self, _suspend: bool) {}
}

/// What `com.apportable.media.AudioPlayer` asks of one music player (milliseconds for positions).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MusicOp {
    Prepare,
    Start,
    Pause,
    Release,
    SeekTo(i32),
    /// `setNumberOfLoops`: Apportable loops forever for -1 only.
    Loops(i32),
    Volume(f32, f32),
    Duration,
    Position,
    IsPlaying,
}

/// Transport and volume commands of an `AudioTrack`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrackOp {
    Play,
    Pause,
    Stop,
    Flush,
    Release,
    Volume(f32),
}

/// [`AudioHooks`] that plays nothing but paces `write` like a real device would (so mixing threads do not spin).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoAudio;

impl AudioHooks for NoAudio {
    fn min_buffer_size(&self, rate: i32, _channel_config: i32, _encoding: i32) -> i32 {
        rate / 25 * 4
    }
    fn create(&self, _rate: i32, _channel_config: i32, _encoding: i32, _buffer_bytes: i32) -> Option<u32> {
        Some(1)
    }
    fn control(&self, _id: u32, _op: TrackOp) -> i32 {
        0
    }
    fn write(&self, _cpu: &mut bhpc_cpu::Cpu, _id: u32, data: &[u8]) -> i32 {
        // 44.1 kHz 16-bit stereo: 176400 bytes per second.
        std::thread::sleep(std::time::Duration::from_micros(data.len() as u64 * 1_000_000 / 176_400));
        data.len() as i32
    }
    fn head_position(&self, _id: u32) -> i32 {
        0
    }
}

/// What the game asks of the desktop outside its own window: links, mail, the clipboard, short notices, saved
/// pictures and documents. The defaults do nothing, so the fake JVM works headless and in tests.
pub trait SystemHooks: Send + Sync {
    /// Open `url` with the user's default program. The Java layer only passes `http`, `https` and `mailto` URLs.
    /// False if it could not be handed over.
    fn open_url(&self, _url: &str) -> bool {
        false
    }
    /// Show an HTML page (help, credits) outside the game. `files` are the resources it refers to by relative name
    /// (pictures, style sheets), found in the APK's assets. False if it could not be shown.
    fn show_html(&self, _html: &str, _files: &[(String, Vec<u8>)]) -> bool {
        false
    }
    /// [`SystemHooks::show_html`] shows pages in place of the game (the launcher's window), as the phone's web view
    /// does: then even the server's short welcome message goes there instead of an in-game dialog.
    fn shows_pages(&self) -> bool {
        false
    }
    /// Put `text` on the desktop clipboard.
    fn set_clipboard(&self, _text: &str) {}
    /// The desktop clipboard's text, if the host can read it.
    fn clipboard(&self) -> Option<String> {
        None
    }
    /// Show a short notice over the game.
    fn notify(&self, _text: &str) {}
    /// The game saved a picture at this guest path (`MediaScannerConnection.scanFile`).
    fn picture_saved(&self, _path: &str) {}
}

/// [`SystemHooks`] with every default.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoSystem;

impl SystemHooks for NoSystem {}

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
    /// The host is on a local network (a non-loopback IPv4 interface is up): `Reachability` then reports
    /// WiFi and the game offers LAN hosting and joining.
    pub lan: bool,
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
            lan: true,
        }
    }
}

/// The file in the launcher's data folder that holds this install's device id.
pub const DEVICE_ID_FILE: &str = "device-id";

fn is_hex(v: &str, len: std::ops::RangeInclusive<usize>) -> bool {
    len.contains(&v.len()) && v.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A fresh random 16-digit hex id (std's randomly keyed hasher over the time and the process id).
pub fn random_device_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    h.write_u32(std::process::id());
    format!("{:016x}", h.finish())
}

/// This install's `Settings.Secure.ANDROID_ID`: the game builds its device id (`udidNew`, through
/// `VerdeActivity.uuid()`, from the first 12 digits) from it, and a server binds a player's name to that device.
/// `env` (`BHPC_ANDROID_ID`, 1-16 hex digits) wins, for tests that run several games from one data folder; else
/// the id stored in `<root>/device-id`; else a new random one, stored there (a write failure keeps it for this run).
pub fn device_id(root: &std::path::Path, env: Option<&str>) -> String {
    if let Some(v) = env.filter(|v| is_hex(v, 1..=16)) {
        return v.to_ascii_lowercase();
    }
    let path = root.join(DEVICE_ID_FILE);
    if let Some(v) = std::fs::read_to_string(&path).ok().filter(|s| is_hex(s.trim(), 16..=16)) {
        return v.trim().to_ascii_lowercase();
    }
    let id = random_device_id();
    if let Err(e) = std::fs::write(&path, format!("{id}\n")) {
        eprintln!("[run] the device id is not kept ({}): {e}", path.display());
    }
    id
}

impl Platform {
    /// Platform values for a guest whose directories are the Android paths of `paths` (what
    /// `bhpc-vfs` mounts): the guest sees these strings, the VFS maps them to the host.
    pub fn for_guest(paths: &bhpc_vfs::GuestPaths) -> Platform {
        Platform {
            package_name: paths.package.clone(),
            files_dir: paths.files_dir(),
            cache_dir: paths.cache_dir(),
            external_dir: "/sdcard".to_string(),
            obb_dir: paths.obb_dir(),
            apk_path: paths.code_path(),
            ..Platform::default()
        }
    }

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

    #[test]
    fn device_id_per_install() {
        let dir = std::env::temp_dir().join(format!("bhpc-devid-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(device_id(&dir, Some("A1b2")), "a1b2", "the override wins");
        let first = device_id(&dir, None);
        assert!(is_hex(&first, 16..=16), "{first}");
        assert_eq!(std::fs::read_to_string(dir.join(DEVICE_ID_FILE)).unwrap(), format!("{first}\n"));
        for bad in ["", "xyz", "0123456789abcdef0"] {
            assert_eq!(device_id(&dir, Some(bad)), first, "bad override {bad:?}: the stored id");
        }
        std::fs::write(dir.join(DEVICE_ID_FILE), "ABCDEF0123456789").unwrap();
        assert_eq!(device_id(&dir, None), "abcdef0123456789", "kept");
        std::fs::write(dir.join(DEVICE_ID_FILE), "garbage").unwrap();
        let fresh = device_id(&dir, None);
        assert!(is_hex(&fresh, 16..=16) && fresh != "abcdef0123456789", "a broken file gets a new id");
        assert_ne!(random_device_id(), random_device_id());
        // A folder that cannot be written: the id still comes back.
        assert!(is_hex(&device_id(&dir.join("missing/sub"), None), 16..=16));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

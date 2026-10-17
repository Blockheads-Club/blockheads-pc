//! Host data directory layout and the standard Android-to-host mount table.

use crate::archive::ApkArchive;
use crate::asset::AssetManager;
use crate::error::Result;
use crate::fonts::{self, FontFs};
use crate::fs::FileSystem;
use crate::hostfs::{HostFs, SingleFile};
use crate::memfs::{MemFs, Node};
use crate::vfs::Vfs;
use crate::zipfs::ZipFs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Package name of the game (used for the Android-style directories).
pub const DEFAULT_PACKAGE: &str = "com.noodlecake.blockheads";

/// Per-user data directories on the host (PLAN 1.8).
///
/// Root: `$XDG_DATA_HOME/blockheads-pc` on Linux, `%APPDATA%\blockheads-pc` on Windows,
/// `~/Library/Application Support/blockheads-pc` on macOS (all from the `dirs` crate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDirs {
    /// The root; also holds `prefs.json` and `config.toml` (owned by other crates).
    pub root: PathBuf,
    /// Guest `HOME` and files dir: saves, worlds, `Documents`, `Library`.
    pub home: PathBuf,
    /// Guest cache dir.
    pub cache: PathBuf,
    /// Guest temporary files.
    pub tmp: PathBuf,
    /// The rest of `/data/data/<pkg>` (databases, shared_prefs files and so on).
    pub appdata: PathBuf,
    /// Emulated external storage (`/sdcard`).
    pub external: PathBuf,
    /// Expansion files the user placed (`/sdcard/Android/obb/<pkg>`), read-only for the guest.
    pub obb: PathBuf,
    /// Optional font overrides, served first under `/system/fonts`.
    pub fonts: PathBuf,
    /// Guest log and JNI trace.
    pub logs: PathBuf,
}

impl DataDirs {
    /// Lays out the sub-directories below `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        DataDirs {
            home: root.join("home"),
            cache: root.join("cache"),
            tmp: root.join("tmp"),
            appdata: root.join("appdata"),
            external: root.join("sdcard"),
            obb: root.join("obb"),
            fonts: root.join("fonts"),
            logs: root.join("logs"),
            root,
        }
    }

    /// `<base>/blockheads-pc`, or `None` when the platform has no data directory.
    pub fn root_in(base: Option<PathBuf>) -> Option<PathBuf> {
        base.map(|b| b.join("blockheads-pc"))
    }

    /// The default per-user location for this OS.
    pub fn platform_default() -> Option<Self> {
        Self::root_in(dirs::data_dir()).map(Self::new)
    }

    /// `prefs.json` (SharedPreferences backing store, owned by the JNI layer).
    pub fn prefs_file(&self) -> PathBuf {
        self.root.join("prefs.json")
    }

    /// Creates every directory, plus the iOS-style folders Apportable's `NSFileManager` expects
    /// below `HOME` (`Documents`, `Library/Caches`, `Library/Preferences`).
    pub fn create_all(&self) -> io::Result<()> {
        for d in [&self.home, &self.cache, &self.tmp, &self.appdata, &self.external, &self.obb, &self.fonts, &self.logs]
        {
            std::fs::create_dir_all(d)?;
        }
        for sub in ["Documents", "Library/Caches", "Library/Preferences"] {
            std::fs::create_dir_all(self.home.join(sub))?;
        }
        Ok(())
    }
}

/// Facts the guest reads from `/proc` and `/sys`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemInfo {
    /// CPU count (PLAN: 4).
    pub cpus: u32,
    /// `Features` line of `/proc/cpuinfo`; must match what the JIT really supports.
    pub features: String,
    /// ARM `CPU part` (0xc09 is a Cortex-A9).
    pub cpu_part: u32,
    /// `MemTotal` in KiB.
    pub mem_total_kb: u64,
    /// Content of `/etc/resolv.conf`.
    pub resolv_conf: String,
}

impl Default for SystemInfo {
    fn default() -> Self {
        SystemInfo {
            cpus: 4,
            features: "swp half thumb fastmult vfp edsp neon vfpv3 tls".to_string(),
            cpu_part: 0xc09,
            mem_total_kb: 2 * 1024 * 1024,
            resolv_conf: read_or(Path::new("/etc/resolv.conf"), "nameserver 1.1.1.1\n"),
        }
    }
}

/// Reads a host text file, falling back to `fallback`.
pub fn read_or(path: &Path, fallback: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|_| fallback.to_string())
}

impl SystemInfo {
    /// `/proc/cpuinfo` in the ARMv7 kernel format.
    pub fn cpuinfo(&self) -> String {
        let mut s = String::new();
        for i in 0..self.cpus {
            s.push_str(&format!(
                "processor\t: {i}\nmodel name\t: ARMv7 Processor rev 3 (v7l)\nBogoMIPS\t: 38.40\n\
                 Features\t: {}\nCPU implementer\t: 0x41\nCPU architecture: 7\nCPU variant\t: 0x0\n\
                 CPU part\t: {:#x}\nCPU revision\t: 3\n\n",
                self.features, self.cpu_part
            ));
        }
        s.push_str("Hardware\t: blockheads-pc\nRevision\t: 0000\nSerial\t\t: 0000000000000000\n");
        s
    }

    /// `/proc/meminfo` (the fields libc and jemalloc style probes read).
    pub fn meminfo(&self) -> String {
        let t = self.mem_total_kb;
        format!(
            "MemTotal:       {t} kB\nMemFree:        {} kB\nBuffers:        0 kB\nCached:         {} kB\n\
             SwapTotal:      0 kB\nSwapFree:       0 kB\n",
            t / 2,
            t / 4
        )
    }

    fn cpu_range(&self) -> String {
        format!("0-{}\n", self.cpus.saturating_sub(1))
    }
}

/// Guest-visible paths for one package, and the environment variables the game's
/// `LibraryLoader.setenv` normally provides (their names come from `libv.so`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestPaths {
    /// Package name.
    pub package: String,
}

impl GuestPaths {
    /// Paths for `package`.
    pub fn new(package: &str) -> Self {
        GuestPaths { package: package.to_string() }
    }

    /// `/data/data/<pkg>`
    pub fn data_dir(&self) -> String {
        format!("/data/data/{}", self.package)
    }
    /// `getFilesDir()`, also `HOME`.
    pub fn files_dir(&self) -> String {
        format!("{}/files", self.data_dir())
    }
    /// `getCacheDir()`.
    pub fn cache_dir(&self) -> String {
        format!("{}/cache", self.data_dir())
    }
    /// Temporary directory.
    pub fn tmp_dir(&self) -> String {
        format!("{}/tmp", self.data_dir())
    }
    /// The APK as the guest sees it (`PACKAGE_CODE_PATH`).
    pub fn code_path(&self) -> String {
        format!("/data/app/{}-1/base.apk", self.package)
    }
    /// Expansion file directory (`OBBDIR`).
    pub fn obb_dir(&self) -> String {
        format!("/sdcard/Android/obb/{}", self.package)
    }

    /// Environment for the guest. Values for names whose exact format is not confirmed against
    /// a device trace (`PACKAGE_RESOURCE_PATH`, `EXTERNAL_CACHEDIR`) follow the Android convention.
    pub fn env(&self) -> Vec<(String, String)> {
        let kv = |k: &str, v: String| (k.to_string(), v);
        vec![
            kv("PACKAGE_NAME", self.package.clone()),
            kv("PACKAGE_CODE_PATH", self.code_path()),
            kv("PACKAGE_RESOURCE_PATH", self.code_path()),
            kv("HOME", self.files_dir()),
            kv("TMPDIR", self.tmp_dir()),
            kv("TEMPDIR", self.tmp_dir()),
            kv("CACHEDIR", self.cache_dir()),
            kv("EXTERNAL_CACHEDIR", format!("/sdcard/Android/data/{}/cache", self.package)),
            kv("OBBDIR", self.obb_dir()),
            kv("ANDROID_DATA_DIRECTORY", "/data".to_string()),
            kv("ANDROID_EXTERNAL_STORAGE", "/sdcard".to_string()),
            kv("ANDROID_DOWNLOAD_DIRECTORY", "/sdcard/Download".to_string()),
        ]
    }
}

/// Everything [`build`] needs.
#[derive(Debug, Clone)]
pub struct VfsConfig {
    /// Host path of the user's APK (served raw at [`GuestPaths::code_path`]).
    pub apk_path: PathBuf,
    /// Package name.
    pub package: String,
    /// Host directories.
    pub dirs: DataDirs,
    /// `/proc` and `/sys` facts.
    pub system: SystemInfo,
}

impl VfsConfig {
    /// A config with the default package, system info and the given directories.
    pub fn new(apk_path: impl Into<PathBuf>, dirs: DataDirs) -> Self {
        VfsConfig {
            apk_path: apk_path.into(),
            package: DEFAULT_PACKAGE.to_string(),
            dirs,
            system: SystemInfo::default(),
        }
    }
}

/// The wired-up subsystem handed to `bhpc-run`.
pub struct Storage {
    /// The mount table; clone the `Arc` into the bionic `open`/`fopen`/`stat` layer.
    pub vfs: Arc<Vfs>,
    /// AAsset API for the JNI `AssetManager` (root `assets/`).
    pub assets: AssetManager,
    /// Guest-side paths and environment.
    pub guest: GuestPaths,
}

impl Storage {
    /// Opens the APK named in `cfg` and builds the mount table.
    pub fn open(cfg: &VfsConfig) -> Result<Storage> {
        Ok(build(cfg, Arc::new(ApkArchive::open(&cfg.apk_path)?)))
    }
}

/// The synthetic `/proc`, `/sys`, `/dev`, `/etc` and font configuration files.
pub fn system_files(sys: &SystemInfo) -> MemFs {
    let mut m = MemFs::new();
    m.insert_text("/proc/cpuinfo", &sys.cpuinfo());
    m.insert_text("/proc/meminfo", &sys.meminfo());
    for f in ["possible", "present", "online"] {
        m.insert_text(&format!("/sys/devices/system/cpu/{f}"), &sys.cpu_range());
    }
    m.insert_text("/system/etc/hosts", "127.0.0.1 localhost\n::1 ip6-localhost\n");
    m.insert_text("/etc/resolv.conf", &sys.resolv_conf);
    m.insert("/dev/null", Node::Null);
    m.insert("/dev/urandom", Node::Random);
    m.insert("/dev/random", Node::Random);
    m.insert_text(fonts::SYSTEM_FONTS_XML, &fonts::system_fonts_xml());
    m.insert_text(fonts::FALLBACK_FONTS_XML, &fonts::fallback_fonts_xml());
    m.insert_text(fonts::VENDOR_FALLBACK_FONTS_XML, &fonts::vendor_fallback_fonts_xml());
    m
}

/// Builds the standard Android-to-host mount table (documented in the crate README).
pub fn build(cfg: &VfsConfig, apk: Arc<ApkArchive>) -> Storage {
    let guest = GuestPaths::new(&cfg.package);
    let d = &cfg.dirs;
    let vfs = Vfs::new();
    let rw = |p: &Path| -> Arc<dyn FileSystem> { Arc::new(HostFs::new(p, true)) };
    let ro = |p: &Path| -> Arc<dyn FileSystem> { Arc::new(HostFs::new(p, false)) };
    let mount = |at: &str, fs: Arc<dyn FileSystem>| {
        vfs.mount(at, fs).expect("static mount points are valid");
    };

    mount("/", Arc::new(system_files(&cfg.system)));
    mount("/system/fonts", Arc::new(FontFs::new(Some(apk.clone()), Some(d.fonts.clone()))));
    mount("/System", Arc::new(ZipFs::new(apk.clone(), "com.apportable.system")));
    mount(&guest.code_path(), Arc::new(SingleFile::new(&cfg.apk_path)));

    let data = guest.data_dir();
    for root in [data.clone(), format!("/data/user/0/{}", cfg.package)] {
        mount(&root, rw(&d.appdata));
        mount(&format!("{root}/files"), rw(&d.home));
        mount(&format!("{root}/cache"), rw(&d.cache));
        mount(&format!("{root}/tmp"), rw(&d.tmp));
    }
    mount("/data/local/tmp", rw(&d.tmp));
    mount("/tmp", rw(&d.tmp));

    for ext in ["/sdcard", "/mnt/sdcard", "/storage/emulated/0", "/storage/emulated/legacy"] {
        mount(ext, rw(&d.external));
        mount(&format!("{ext}/Android/obb/{}", cfg.package), ro(&d.obb));
    }

    Storage { vfs: Arc::new(vfs), assets: AssetManager::android(apk), guest }
}

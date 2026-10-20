# bhpc-vfs

File storage for the PC port (PLAN 1.8): the user's APK as a lazy read-only asset tree, a writable per-user data
directory, the Android/Apportable path mapping, synthetic `/proc` `/sys` `/dev` files, and system fonts.
GPL-3.0-or-later. No game content is stored here; the APK is only ever read, never extracted.

## API

| Item | Use |
|---|---|
| `ApkArchive::open(path)` | Reads only the central directory. Entries are decompressed on first open; live opens share one `Arc<[u8]>`. |
| `AssetManager::android(apk)` | `AAsset` API for the JNI `AssetManager` (names relative to `assets/`): `open` -> `Asset` (`read`, `seek(off, Whence)`, `length`, `remaining`, `buffer`), `open_dir` -> `AssetDir` (`next_file_name`, `rewind`, files only), `list` (files and dirs, for Java `list()`), `exists`, `length`. |
| `trait FileSystem` + `trait VFile` | What bionic `open/fopen/stat/lstat/opendir/readdir/mkdir/unlink/rmdir/rename` call. `VFile` is one open file with its own position (the guest fd table owns them). |
| `OpenFlags::from_bionic(flags)`, `from_fopen_mode("rb")` | Translate guest `open`/`fopen` arguments. |
| `VfsError::errno()` | bionic `errno` value for every failure. |
| `Stat::mode()` | `st_mode` for the guest `struct stat`. |
| `Storage::open(&VfsConfig)` / `build(cfg, apk)` | Builds the standard mount table: `Storage { vfs: Arc<Vfs>, assets, guest }`. |
| `GuestPaths::env()` | The environment `LibraryLoader.setenv` normally sets (`HOME`, `TMPDIR`, `CACHEDIR`, `OBBDIR`, `PACKAGE_CODE_PATH`, ...). |
| `DataDirs::platform_default()` / `create_all()` | `$XDG_DATA_HOME/blockheads-pc`, `%APPDATA%\blockheads-pc`, `~/Library/Application Support/blockheads-pc` via the `dirs` crate; creates `home/{Documents,Library/Caches,Library/Preferences}` too. |
| `Vfs::take_misses()` | Paths the guest asked for that map to nothing (`open /system/lib/libfoo.so`). Dump them to the log: this is the work queue for missing mappings. |

## Standard mount table

Longest prefix wins; parents of mount points exist as virtual directories; anything unmapped is `ENOENT` and recorded.

| Guest path | Backend | Host |
|---|---|---|
| `/` | synthetic: `/proc/cpuinfo`, `/proc/meminfo`, `/sys/devices/system/cpu/{possible,present,online}`, `/system/etc/hosts`, `/etc/resolv.conf`, `/dev/{null,urandom,random}`, font XML | memory |
| `/system/fonts` | font mapping (below) | APK + bundled OFL + `<root>/fonts` |
| `/System` | `com.apportable.system/` inside the APK (`icu/icu.dat`, ...) | APK |
| `/data/app/<pkg>-1/base.apk` (= `PACKAGE_CODE_PATH`) | the raw APK file, read-only, so libv's zzip can parse it with `open/read/lseek` | the APK |
| `/data/data/<pkg>`, `/data/user/0/<pkg>` | writable | `<root>/appdata` |
| `.../files` (= `HOME`) | writable | `<root>/home` |
| `.../cache` | writable | `<root>/cache` |
| `.../tmp`, `/data/local/tmp`, `/tmp` | writable | `<root>/tmp` |
| `/sdcard`, `/mnt/sdcard`, `/storage/emulated/{0,legacy}` | writable | `<root>/sdcard` |
| `<sdcard>/Android/obb/<pkg>` (= `OBBDIR`) | read-only | `<root>/obb` |

Host names containing `\` or `:` are refused (`EINVAL`) so a guest path can never become a Windows drive/UNC path. Android is case
sensitive; Windows and macOS hosts usually are not.

## Wiring in bhpc-run / bhpc-bionic / bhpc-jni (done in the M2 merge)

bionic keeps only a descriptor table over `Arc<dyn FileSystem>` (`bhpc_bionic::vfs::Vfs`, `Config::fs`); `bhpc-run` builds `Storage` and
hands `storage.vfs` to bionic, `storage.assets` to the JNI `Assets` trait (implemented for `AssetManager`) and `storage.guest.env()` to the
guest environment. `readlink` (`/proc/self/exe`) and `/dev/zero` were added for bionic. The text below is the original plan.

1. Startup, before loading libs:
   ```rust
   let dirs = DataDirs::platform_default().unwrap(); dirs.create_all()?;
   let storage = Storage::open(&VfsConfig::new(apk_path, dirs))?;   // apk_path from config
   for (k, v) in storage.guest.env() { /* setenv in guest environ (bionic State) */ }
   ```
   Add `bhpc-vfs` to the `bhpc-bionic`/`bhpc-jni`/`bhpc-run` dependencies. `SystemInfo.features` must match what the JIT supports
   (default `swp half thumb fastmult vfp edsp neon vfpv3 tls`).
2. bionic: keep a guest fd table `Vec<Option<Box<dyn VFile>>>` (fd 0-2 reserved). `open(path, flags, mode)` ->
   `vfs.open(&path, OpenFlags::from_bionic(flags))`, `fopen` -> `OpenFlags::from_fopen_mode`; `read/write/lseek/fstat/ftruncate/fsync`
   map 1:1 to `VFile`; `stat/lstat/access` -> `vfs.stat` (`Stat::mode`, `size`, `mtime`); `opendir/readdir` -> `vfs.read_dir`
   snapshot (then `.`/`..` first); `mkdir/unlink/rmdir/rename` -> same names; `getcwd/chdir` -> `vfs.cwd()/chdir`.
   On `Err(e)` set guest `errno = e.errno()` and return -1. Pass `Arc<Vfs>` through the bionic `Config` (add one field).
   mmap of an APK/asset fd: read the bytes via `VFile::read` into the mapping (the files are small or read-only copies).
3. JNI `AssetManager` (`android.content.res.AssetManager`): keep a handle table of `Asset`/`AssetDir`; `open`/`openFd` ->
   `storage.assets.open(name)` (FileNotFoundException on `Err`), `list` -> `storage.assets.list`, `InputStream.read/skip/available/close`
   -> `Asset::read/seek/remaining`. `Context.getFilesDir/getCacheDir/getExternalFilesDir` return the strings of `GuestPaths`.
4. After a run (or on exit/crash) print `vfs.take_misses()`.
5. Optional: expose `DataDirs.fonts` in the launcher so users can drop replacement fonts.

## Fonts

See [`docs/fonts.md`](docs/fonts.md): what Apportable's CoreGraphics reads, how it is satisfied, and the bundled OFL fonts.

## Tests

No APK needed. `scripts/coverage.sh` with `COVERAGE_PACKAGES=bhpc-vfs` gates 100% lines/functions. A local smoke test against a
real APK is `#[ignore]`d: `BHPC_TEST_APK=/path/to.apk cargo test -p bhpc-vfs -- --ignored`.

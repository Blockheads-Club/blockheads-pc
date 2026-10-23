use super::cache::{self, server_key};
use super::dev;
use super::index::{self, browse, fetch, load_index, parse_index, plan_index, Fetch, IndexFile, MAX_PACKAGE};
use super::install::{
    self, auto_update, download_index, from_file, from_link, inspect, plan_candidate, write_packages, Choices,
};
use super::*;
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::Path;

/// A `.bhmod` with a client Luau entry; `extra` goes into `mod.toml` before `[sides]`.
fn pkg(id: &str, version: &str, extra: &str) -> Vec<u8> {
    pkg_sides(id, version, "client = true", extra)
}

fn pkg_sides(id: &str, version: &str, sides: &str, extra: &str) -> Vec<u8> {
    let mut entries = String::new();
    let mut files = Vec::new();
    for side in ["server", "client"] {
        if sides.contains(&format!("{side} = true")) {
            entries += &format!("[entry.{side}]\nruntime = \"lua\"\npath = \"{side}/main.luau\"\n");
            files.push(format!("{side}/main.luau"));
        }
    }
    let files: Vec<&str> = files.iter().map(String::as_str).collect();
    zip_files(
        &format!(
            "schema = 1\nid = \"{id}\"\nversion = \"{version}\"\nname = {{ en = \"Mod {id}\" }}\nauthors = [\"pixelpan\"]\n\
             license = \"MIT\"\napi = \"^0.2\"\n{extra}\n[sides]\n{sides}\n{entries}"
        ),
        &files,
    )
}

fn zip_with(toml: &str, entry: &str) -> Vec<u8> {
    zip_files(toml, &[entry])
}

fn zip_files(toml: &str, entries: &[&str]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    w.start_file("mod.toml", o).unwrap();
    w.write_all(toml.as_bytes()).unwrap();
    for e in entries {
        w.start_file(*e, o).unwrap();
        w.write_all(b"return {}\n").unwrap();
    }
    w.finish().unwrap().into_inner()
}

/// A client mod with its own `api` / `loader` lines.
fn pkg_req(id: &str, reqs: &str) -> Vec<u8> {
    zip_with(
        &format!("schema = 1\nid = \"{id}\"\nversion = \"1.0.0\"\nname = {{ en = \"X\" }}\nauthors = [\"a\"]\nlicense = \"MIT\"\n{reqs}\n[sides]\nclient = true\n[entry.client]\nruntime = \"lua\"\npath = \"client/main.luau\"\n"),
        "client/main.luau",
    )
}

fn sha(b: &[u8]) -> String {
    bhmod_manifest::pack::sha256_hex(b)
}

/// A fake network: url -> body (other urls fail).
#[derive(Default)]
struct Net(HashMap<String, Vec<u8>>);

impl Fetch for Net {
    fn get(&self, url: &str, _limit: u64) -> MRes<Vec<u8>> {
        self.0.get(url).cloned().ok_or_else(|| ModError::with("download", url))
    }
}

fn version_json(id: &str, v: &str, bytes: &[u8], extra: &str) -> String {
    let api = if extra.contains(r#""api""#) { "" } else { r#","api":"^0.2""# };
    format!(
        r#"{{"version":"{v}","url":"https://m.test/{id}-{v}.bhmod","sha256":"{}","size":{},"sides":["client"]{api},"published":"2026-10-01"{extra}}}"#,
        sha(bytes),
        bytes.len()
    )
}

fn index_json(mods: &[(&str, Vec<String>, &str)]) -> String {
    let ms: Vec<String> = mods
        .iter()
        .map(|(id, vs, tags)| {
            format!(r#"{{"id":"{id}","name":"Mod {id}","owners":["lumen"],"repo":"https://g.test/{id}","tags":[{tags}],"versions":[{}]}}"#, vs.join(","))
        })
        .collect();
    format!(r#"{{"schema":1,"generated":"2026-10-21T12:00:00Z","api":["0.2.0"],"mods":[{}]}}"#, ms.join(","))
}

fn index_with(mods: &[(&str, Vec<String>, &str)]) -> IndexFile {
    parse_index(index_json(mods).as_bytes()).unwrap()
}

fn temp() -> (tempfile::TempDir, ModsDirs) {
    let t = tempfile::tempdir().unwrap();
    let d = ModsDirs::new(t.path());
    (t, d)
}

fn put(dir: &Path, name: &str, bytes: &[u8]) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(name), bytes).unwrap();
}

fn cand(bytes: Vec<u8>) -> Candidate {
    Candidate { source: "file".into(), ..inspect(bytes, "x.bhmod").unwrap() }
}

#[cfg(unix)]
fn root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

#[cfg(unix)]
fn chmod(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn errors_print_and_dirs_are_below_the_data_folder() {
    assert_eq!(ModError::new("x").to_string(), "x");
    assert_eq!(ModError::with("io", "denied").to_string(), "io: denied");
    let d = ModsDirs::new(Path::new("/data"));
    assert_eq!(d.mods, Path::new("/data/mods"));
    assert_eq!(d.index_cache(), Path::new("/data/mods-cache/index.json"));
    assert!(now() > 1_700_000_000);
}

#[test]
fn permissions_have_bhmod_core_names() {
    use bhmod_manifest::manifest::{Permissions, PlayerPermission as P, WorldAccess};
    let p = Permissions {
        world: WorldAccess::Write,
        players: vec![P::Chat, P::Teleport, P::Kick, P::Ban, P::Inventory, P::Health],
        net_http: vec!["a.test".into()],
        channels: true,
        native: true,
    };
    assert_eq!(
        required_permissions(&p),
        [
            "world:write",
            "players:chat",
            "players:teleport",
            "players:kick",
            "players:ban",
            "players:inventory",
            "players:health",
            "net-http:a.test",
            "channels",
            "native"
        ]
    );
    assert_eq!(required_permissions(&Permissions { world: WorldAccess::Read, ..Default::default() }), ["world:read"]);
    assert!(required_permissions(&Permissions::default()).is_empty());
    assert!(missing_permissions(&["world:read".into()], &["world:write".into()]).is_empty());
    assert_eq!(missing_permissions(&["channels".into(), "native".into()], &["channels".into()]), ["native"]);
}

#[test]
fn config_round_trips_and_drops_defaults() {
    let (_t, d) = temp();
    assert!(read_config(&d.mods).unwrap().is_empty());
    edit_config(&d.mods, |c| {
        c.insert("a".into(), ModConfig { enabled: false, grant: vec![] });
        c.insert("b".into(), ModConfig::default());
        c.insert("c".into(), ModConfig { enabled: true, grant: vec!["channels".into()] });
    })
    .unwrap();
    let text = std::fs::read_to_string(d.mods.join(CONFIG_FILE)).unwrap();
    assert!(text.contains("[a]\nenabled = false") && !text.contains("[b]"), "{text}");
    let c = read_config(&d.mods).unwrap();
    assert_eq!(c.len(), 2);
    assert_eq!(c["c"].grant, ["channels"]);
    std::fs::write(d.mods.join(CONFIG_FILE), "[a]\nsurprise = 1\n").unwrap();
    assert_eq!(read_config(&d.mods).unwrap_err().code, "config");
    assert_eq!(edit_config(&d.mods, |_| {}).unwrap_err().code, "config");
    let (_t2, d2) = temp();
    std::fs::create_dir_all(d2.mods.join(CONFIG_FILE)).unwrap();
    assert_eq!(read_config(&d2.mods).unwrap_err().code, "io");
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("f"), "").unwrap();
    assert_eq!(edit_config(&t.path().join("f/mods"), |_| {}).unwrap_err().code, "io");
    assert_eq!(write_records(&t.path().join("f/mods"), &Records::new()).unwrap_err().code, "io");
}

#[cfg(unix)]
#[test]
fn config_write_failure() {
    if root() {
        return;
    }
    let (_t, d) = temp();
    std::fs::create_dir_all(&d.mods).unwrap();
    chmod(&d.mods, 0o555);
    let r = edit_config(&d.mods, |_| {});
    chmod(&d.mods, 0o755);
    assert_eq!(r.unwrap_err().code, "io");
}

#[test]
fn mod_prefs_are_sanitized() {
    use crate::prefs::Prefs;
    assert_eq!(Prefs { mods_cache_limit_mb: 7, ..Prefs::default() }.sanitized().mods_cache_limit_mb, 1024);
    assert_eq!(Prefs { mods_cache_limit_mb: 0, ..Prefs::default() }.sanitized().mods_cache_limit_mb, 0);
    let p: Prefs = toml::from_str("language = \"tr\"").unwrap();
    assert!(
        p.mods_auto_update
            && p.mods_dev_watch
            && !p.mods_developer
            && !p.mods_block_native
            && !p.mods_jvm
            && p.mods_java_home.is_empty()
    );
}

#[test]
fn records_are_optional() {
    let (_t, d) = temp();
    assert!(read_records(&d.mods).is_empty());
    put(&d.mods, META_FILE, b"not toml [");
    assert!(read_records(&d.mods).is_empty());
    let mut r = Records::new();
    r.insert(
        "a".into(),
        InstallRecord { source: "index".into(), origin: "u".into(), installed: 5, native_allowed: Some(6) },
    );
    write_records(&d.mods, &r).unwrap();
    assert_eq!(read_records(&d.mods), r);
}

#[test]
fn discover_checks_packages() {
    let (_t, d) = temp();
    assert!(discover(&d.mods).0.is_empty());
    put(&d.mods, "a-1.0.0.bhmod", &pkg("aaa", "1.0.0", ""));
    put(&d.mods, "b.bhmod", &pkg("aaa", "2.0.0", ""));
    put(&d.mods, "c.bhmod", b"not a zip");
    put(&d.mods, ".hidden.bhmod", b"x");
    put(&d.mods, "notes.txt", b"x");
    std::fs::create_dir_all(d.mods.join("dir.bhmod")).unwrap();
    let (pkgs, problems) = discover(&d.mods);
    assert_eq!(pkgs.len(), 1);
    assert_eq!(pkgs[0].manifest.version, "1.0.0");
    assert_eq!(problems.len(), 2);
    assert!(problems[0].message.contains("another file"), "{problems:?}");
    assert!(problems[1].message.starts_with("invalid"), "{problems:?}");
    assert_eq!(installed_versions(&d.mods)["aaa"], "1.0.0");
}

#[cfg(unix)]
#[test]
fn unreadable_packages_are_problems() {
    if root() {
        return;
    }
    let (_t, d) = temp();
    put(&d.mods, "y.bhmod", b"x");
    chmod(&d.mods.join("y.bhmod"), 0o000);
    let (_, problems) = discover(&d.mods);
    assert!(problems[0].message.starts_with("io"), "{problems:?}");
}

#[test]
fn inspect_rejects_bad_packages() {
    assert_eq!(inspect(vec![0; MAX_PACKAGE as usize + 1], "big").unwrap_err().code, "tooLarge");
    let e = inspect(b"nope".to_vec(), "f.bhmod").unwrap_err();
    assert_eq!((e.code.as_str(), e.file.as_deref()), ("invalid", Some("f.bhmod")));
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    w.start_file("readme.txt", zip::write::SimpleFileOptions::default()).unwrap();
    let e = inspect(w.finish().unwrap().into_inner(), "g").unwrap_err();
    assert!(e.detail.unwrap().contains("mod.toml"));
}

#[test]
fn overview_statuses() {
    let (_t, d) = temp();
    let dep = pkg("sky-core", "1.2.0", "");
    let sky = pkg(
        "day-sky",
        "1.0.0",
        "[dependencies]\nsky-core = \"^1\"\nmissing-dep = \"^1\"\nsrv-only = { version = \"^1\", side = \"server\" }\n\
         [soft-dependencies]\nsoft-one = \"^1\"\n[permissions]\nworld = \"read\"\n",
    );
    let native = pkg("shaders", "0.7.2", "[permissions]\nnative = true\n");
    let srv = pkg_sides("srv", "1.0.0", "server = true", "");
    let api = pkg_req("oldapi", "api = \"^9\"");
    let loader = pkg_req("newloader", "api = \"^0.2\"\nloader = \"^9\"");
    let off = pkg("offmod", "1.0.0", "");
    for (n, b) in [
        ("dep", &dep),
        ("sky", &sky),
        ("native", &native),
        ("srv", &srv),
        ("api", &api),
        ("loader", &loader),
        ("off", &off),
    ] {
        put(&d.mods, &format!("{n}.bhmod"), b);
    }
    edit_config(&d.mods, |c| {
        c.insert("offmod".into(), ModConfig { enabled: false, grant: vec![] });
        c.insert("sky-core".into(), ModConfig { enabled: false, grant: vec![] });
        c.insert("shaders".into(), ModConfig { enabled: true, grant: vec!["native".into()] });
    })
    .unwrap();
    let idx = index_with(&[
        ("sky-core", vec![version_json("sky-core", "1.3.0", &dep, "")], ""),
        (
            "offmod",
            vec![
                version_json("offmod", "1.0.0", &off, ""),
                version_json("offmod", "2.0.0", &off, r#","permissions":{"world":"read"}"#),
            ],
            "",
        ),
        ("missing-dep", vec![version_json("missing-dep", "1.0.0", &off, "")], ""),
    ]);
    let ov = overview(&d.mods, Some(&idx), false);
    assert_eq!(ov.problems.len(), 1, "the package for another API is refused: {:?}", ov.problems);
    let st: BTreeMap<&str, (&str, Option<&str>)> =
        ov.mods.iter().map(|m| (m.id.as_str(), (m.status.as_str(), m.reason.as_deref()))).collect();
    assert_eq!(st["offmod"], ("off", None));
    assert_eq!(st["sky-core"], ("off", None));
    assert_eq!(st["day-sky"], ("permissions", None));
    assert_eq!(st["shaders"], ("ready", None));
    assert_eq!(st["srv"], ("failed", Some("server-only")));
    assert_eq!(st["newloader"], ("failed", Some("loader")));
    assert_eq!(ov.attention, 3);
    assert!(ov.loader_ready);
    let off_m = ov.mods.iter().find(|m| m.id == "offmod").unwrap();
    assert!(off_m.reviewed);
    assert_eq!(off_m.update.as_deref(), Some("2.0.0"));
    assert!(off_m.update_asks_more);
    let core = ov.mods.iter().find(|m| m.id == "sky-core").unwrap();
    assert_eq!((core.update.as_deref(), core.update_asks_more, core.reviewed), (Some("1.3.0"), false, true));
    assert_eq!(ov.updates, 2);
    let sky_m = ov.mods.iter().find(|m| m.id == "day-sky").unwrap();
    assert_eq!(sky_m.runtime.as_deref(), Some("lua"));
    assert_eq!(sky_m.dependencies.len(), 3, "server-side dependency left out: {:?}", sky_m.dependencies);
    let core_dep = &sky_m.dependencies[1];
    assert_eq!(
        (core_dep.id.as_str(), core_dep.ok, core_dep.installed.as_deref(), core_dep.available.as_deref()),
        ("sky-core", false, Some("1.2.0"), Some("1.3.0"))
    );
    assert!(sky_m.dependencies[2].soft);
    let srv_m = ov.mods.iter().find(|m| m.id == "srv").unwrap();
    assert_eq!(srv_m.runtime, None);
    // Granting the permission: now it waits for its dependencies.
    set_grant(&d.mods, "day-sky", "world:read", true, 1).unwrap();
    let ov = overview(&d.mods, None, false);
    let sky_m = ov.mods.iter().find(|m| m.id == "day-sky").unwrap();
    assert_eq!((sky_m.status.as_str(), sky_m.detail.as_deref()), ("dependency", Some("missing-dep ^1")));
    let ov = overview(&d.mods, None, true);
    let n = ov.mods.iter().find(|m| m.id == "shaders").unwrap();
    assert_eq!((n.status.as_str(), n.reason.as_deref(), n.native), ("blocked", Some("native"), true));
}

#[test]
fn overview_ready_and_folder_errors() {
    let (_t, d) = temp();
    let ov = overview(&d.mods, None, false);
    assert!(ov.mods.is_empty() && ov.error.is_none());
    put(&d.mods, "a.bhmod", &pkg("aaa", "1.0.0", ""));
    let ov = overview(&d.mods, None, false);
    assert_eq!(ov.mods[0].status, "ready");
    assert_eq!(ov.mods[0].source, "");
    assert!(ov.size > 0);
    put(&d.mods, CONFIG_FILE, b"[[[");
    let ov = overview(&d.mods, None, false);
    assert_eq!(ov.error.unwrap().code, "config");
    assert_eq!(ov.mods.len(), 1);
    let t = tempfile::tempdir().unwrap();
    let f = t.path().join("mods");
    std::fs::write(&f, "").unwrap();
    assert_eq!(overview(&f, None, false).error.unwrap().code, "io");
}

#[test]
fn enable_grant_remove() {
    let (_t, d) = temp();
    assert_eq!(set_enabled(&d.mods, "aaa", false).unwrap_err().code, "missing");
    put(&d.mods, "a.bhmod", &pkg("aaa", "1.0.0", "[permissions]\nchannels = true\nnative = true\n"));
    set_enabled(&d.mods, "aaa", false).unwrap();
    assert!(!read_config(&d.mods).unwrap()["aaa"].enabled);
    set_enabled(&d.mods, "aaa", true).unwrap();
    assert_eq!(set_grant(&d.mods, "aaa", "world:write", true, 1).unwrap_err().code, "permission");
    assert_eq!(set_grant(&d.mods, "aaa", "native", true, 1).unwrap_err().code, "permission");
    set_grant(&d.mods, "aaa", "channels", true, 1).unwrap();
    assert_eq!(read_config(&d.mods).unwrap()["aaa"].grant, ["channels"]);
    set_grant(&d.mods, "aaa", "channels", false, 1).unwrap();
    assert!(!read_config(&d.mods).unwrap().contains_key("aaa"));
    // Revoking native turns the mod off and clears the allowed date.
    edit_config(&d.mods, |c| {
        c.insert("aaa".into(), ModConfig { enabled: true, grant: vec!["native".into()] });
    })
    .unwrap();
    let mut r = Records::new();
    r.insert("aaa".into(), InstallRecord { source: "file".into(), native_allowed: Some(3), ..Default::default() });
    write_records(&d.mods, &r).unwrap();
    set_grant(&d.mods, "aaa", "native", false, 9).unwrap();
    assert!(!read_config(&d.mods).unwrap()["aaa"].enabled);
    assert_eq!(read_records(&d.mods)["aaa"].native_allowed, None);
    std::fs::remove_file(d.mods.join(META_FILE)).unwrap();
    set_grant(&d.mods, "aaa", "native", false, 9).unwrap();
    assert_eq!(read_records(&d.mods)["aaa"].installed, 9);
    remove(&d.mods, "aaa").unwrap();
    assert!(discover(&d.mods).0.is_empty());
    assert!(read_records(&d.mods).is_empty());
    assert_eq!(remove(&d.mods, "aaa").unwrap_err().code, "missing");
    put(&d.mods, "b.bhmod", &pkg("bbb", "1.0.0", ""));
    remove(&d.mods, "bbb").unwrap();
    // .installed.toml is a directory: the native revoke cannot record it.
    put(&d.mods, "a.bhmod", &pkg("aaa", "1.0.0", "[permissions]\nnative = true\n"));
    let _ = std::fs::remove_file(d.mods.join(META_FILE));
    std::fs::create_dir_all(d.mods.join(META_FILE)).unwrap();
    assert_eq!(set_grant(&d.mods, "aaa", "native", false, 1).unwrap_err().code, "io");
    // mods.toml broken: set_grant fails before the records.
    put(&d.mods, CONFIG_FILE, b"[[[");
    assert_eq!(set_grant(&d.mods, "aaa", "native", false, 1).unwrap_err().code, "config");
}

#[cfg(unix)]
#[test]
fn remove_failures() {
    if root() {
        return;
    }
    let (_t, d) = temp();
    put(&d.mods, "a.bhmod", &pkg("aaa", "1.0.0", ""));
    chmod(&d.mods, 0o555);
    let r = remove(&d.mods, "aaa");
    chmod(&d.mods, 0o755);
    assert_eq!(r.unwrap_err().code, "io");
    put(&d.mods, CONFIG_FILE, b"[[[");
    assert_eq!(remove(&d.mods, "aaa").unwrap_err().code, "config");
    std::fs::remove_file(d.mods.join(CONFIG_FILE)).unwrap();
    put(&d.mods, "b.bhmod", &pkg("bbb", "1.0.0", ""));
    let mut r = Records::new();
    r.insert("bbb".into(), InstallRecord::default());
    write_records(&d.mods, &r).unwrap();
    std::fs::remove_file(d.mods.join(META_FILE)).unwrap();
    std::fs::create_dir_all(d.mods.join(META_FILE)).unwrap();
    // The records cannot be read (a directory): nothing to update, the remove succeeds.
    remove(&d.mods, "bbb").unwrap();
    std::fs::remove_dir(d.mods.join(META_FILE)).unwrap();
    put(&d.mods, "c.bhmod", &pkg("ccc", "1.0.0", ""));
    let mut r = Records::new();
    r.insert("ccc".into(), InstallRecord::default());
    write_records(&d.mods, &r).unwrap();
    chmod(&d.mods.join(META_FILE), 0o444);
    // A read-only file in a writable folder: std::fs::write fails.
    assert_eq!(remove(&d.mods, "ccc").unwrap_err().code, "io");
}

#[test]
fn fetch_local_https_and_limits() {
    let t = tempfile::tempdir().unwrap();
    let f = t.path().join("i.json");
    std::fs::write(&f, b"12345").unwrap();
    let net = Net::default();
    assert_eq!(fetch(&net, &f.to_string_lossy(), 10).unwrap(), b"12345");
    assert_eq!(fetch(&net, &format!("file://{}", f.display()), 10).unwrap(), b"12345");
    assert_eq!(fetch(&net, &f.to_string_lossy(), 3).unwrap_err().code, "tooLarge");
    assert_eq!(fetch(&net, "/nonexistent/x", 3).unwrap_err().code, "download");
    assert_eq!(fetch(&net, t.path().to_str().unwrap(), 3).unwrap_err().code, "download");
    assert_eq!(fetch(&net, "http://a.test/x", 3).unwrap_err().code, "insecure");
    assert_eq!(fetch(&net, "https://a.test/x", 3).unwrap_err().code, "download");
    let net = Net(HashMap::from([("https://a.test/x".to_string(), b"abcdef".to_vec())]));
    assert_eq!(fetch(&net, "https://a.test/x", 3).unwrap_err().code, "tooLarge");
    assert_eq!(fetch(&net, "https://a.test/x", 6).unwrap(), b"abcdef");
}

#[test]
fn index_parsing_and_offline_copy() {
    assert_eq!(parse_index(b"{").unwrap_err().code, "index");
    assert_eq!(
        parse_index(br#"{"schema":2,"generated":"","mods":[]}"#).unwrap_err().detail.unwrap(),
        "unsupported schema 2"
    );
    let (_t, d) = temp();
    let net = Net(HashMap::from([(index::INDEX_URL.to_string(), index_json(&[]).into_bytes())]));
    let (idx, offline, err) = load_index(&net, index::INDEX_URL, &d.index_cache()).unwrap();
    assert!(idx.mods.is_empty() && !offline && err.is_none());
    assert!(index::cached_index(&d.index_cache()).is_some());
    let (_, offline, err) = load_index(&Net::default(), index::INDEX_URL, &d.index_cache()).unwrap();
    assert!(offline);
    assert_eq!(err.unwrap().code, "download");
    let (_t2, d2) = temp();
    let e = load_index(&Net::default(), index::INDEX_URL, &d2.index_cache()).unwrap_err();
    assert_eq!((e.code.as_str(), e.detail.is_some()), ("offline", true));
    struct Bare;
    impl Fetch for Bare {
        fn get(&self, _: &str, _: u64) -> MRes<Vec<u8>> {
            Err(ModError::new("boom"))
        }
    }
    assert_eq!(load_index(&Bare, "https://x", &d2.index_cache()).unwrap_err().detail.as_deref(), Some("boom"));
    // A cache path without a parent: nothing is written.
    assert!(load_index(&net, index::INDEX_URL, Path::new("")).is_ok());
}

#[test]
fn browse_lists_newest_and_compatibility() {
    let a = pkg("aaa", "1.0.0", "");
    let idx = index_with(&[
        (
            "aaa",
            vec![
                version_json("aaa", "1.0.0", &a, ""),
                version_json("aaa", "2.0.0", &a, r#","api":"^9""#),
                version_json("aaa", "3.0.0", &a, r#","yanked":true"#),
            ],
            r#""graphics","fun""#,
        ),
        ("bbb", vec![version_json("bbb", "1.0.0", &a, r#","yanked":true"#)], ""),
        (
            "ccc",
            vec![version_json("ccc", "1.0.0", &a, r#","permissions":{"native":true},"loader":"^0.1""#)],
            r#""fun""#,
        ),
    ]);
    let installed = HashMap::from([("aaa".to_string(), "1.0.0".to_string())]);
    let b = browse(&idx, &installed, true, Some(ModError::new("download")));
    assert_eq!(b.mods.len(), 2);
    assert_eq!(b.tags, ["fun", "graphics"]);
    let am = &b.mods[0];
    assert_eq!((am.version.as_str(), am.compatible, am.compatible_version.as_deref()), ("2.0.0", false, Some("1.0.0")));
    assert_eq!(am.versions.len(), 2);
    assert_eq!(am.installed.as_deref(), Some("1.0.0"));
    assert!(b.mods[1].native && b.mods[1].compatible);
    assert!(b.offline);
}

#[test]
fn plans_from_the_index() {
    let a = pkg("aaa", "1.0.0", "");
    let idx = index_with(&[
        (
            "day-sky",
            vec![version_json(
                "day-sky",
                "1.0.0",
                &a,
                r#","depends":{"sky-core":"^1","gone":"^1","have":"^1","bad-req":"no"}"#,
            )],
            "",
        ),
        ("sky-core", vec![version_json("sky-core", "1.2.0", &a, r#","depends":{"day-sky":"*","base":"^1"}"#)], ""),
        ("base", vec![version_json("base", "1.0.0", &a, "")], ""),
        (
            "srv",
            vec![r#"{"version":"1.0.0","url":"https://m.test/s.bhmod","sha256":"00","sides":["server"],"api":"^0.2"}"#
                .to_string()],
            "",
        ),
        (
            "keys",
            vec![version_json("keys", "1.6.0", &a, ""), version_json("keys", "2.0.0", &a, r#","loader":"^1.4""#)],
            "",
        ),
        ("future", vec![version_json("future", "1.0.0", &a, r#","api":"^9""#)], ""),
    ]);
    let installed = HashMap::from([
        ("have".to_string(), "1.5.0".to_string()),
        ("base".to_string(), "0.9.0".to_string()),
        ("bad-req".to_string(), "1.0.0".to_string()),
    ]);
    let p = plan_index(&idx, &installed, "day-sky", None).unwrap();
    let ids: Vec<&str> = p.items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids, ["day-sky", "sky-core", "base"]);
    assert_eq!(p.items[1].needed_by.as_deref(), Some("day-sky"));
    assert_eq!(p.items[2].replaces.as_deref(), Some("0.9.0"));
    assert_eq!(p.satisfied.len(), 1);
    assert_eq!(p.missing.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["bad-req", "gone"]);
    assert_eq!(p.total_bytes, 3 * a.len() as u64);
    assert_eq!(plan_index(&idx, &installed, "nope", None).unwrap_err().code, "notListed");
    assert_eq!(plan_index(&idx, &installed, "day-sky", Some("9.9.9")).unwrap_err().code, "notListed");
    assert_eq!(plan_index(&idx, &installed, "srv", None).unwrap_err().code, "serverOnly");
    let e = plan_index(&idx, &installed, "keys", None).unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.expected.as_deref(), e.detail.as_deref()),
        ("newerLoader", Some("^1.4"), Some("1.6.0"))
    );
    assert_eq!(plan_index(&idx, &installed, "keys", Some("1.6.0")).unwrap().items[0].version, "1.6.0");
    let e = plan_index(&idx, &installed, "future", None).unwrap_err();
    assert_eq!((e.expected.as_deref(), e.detail), (Some("^9"), None));
}

#[test]
fn candidates_files_and_links() {
    let t = tempfile::tempdir().unwrap();
    let bytes = pkg(
        "pixel-shaders",
        "0.7.2",
        "[dependencies]\nsky-core = \"^1\"\nsrvdep = { version = \"^1\", side = \"server\" }\n[permissions]\nnative = true\n",
    );
    let f = t.path().join("ps.bhmod");
    std::fs::write(&f, &bytes).unwrap();
    let c = from_file(&f).unwrap();
    assert_eq!((c.source.as_str(), c.origin.as_str()), ("file", "ps.bhmod"));
    let e = from_file(&t.path().join("missing.zip")).unwrap_err();
    assert_eq!((e.code.as_str(), e.file.as_deref()), ("download", Some("missing.zip")));
    let net = Net(HashMap::from([("https://e.test/ps.bhmod".to_string(), bytes.clone())]));
    let l = from_link(&net, "https://e.test/ps.bhmod").unwrap();
    assert_eq!((l.source.as_str(), l.origin.as_str()), ("link", "https://e.test/ps.bhmod"));
    assert_eq!(from_link(&net, "http://e.test/ps.bhmod").unwrap_err().code, "insecure");
    assert_eq!(from_link(&net, "/etc/passwd").unwrap_err().code, "insecure");
    assert_eq!(from_link(&net, "https://e.test/none").unwrap_err().code, "download");
    let installed = HashMap::from([("pixel-shaders".to_string(), "0.7.0".to_string())]);
    let p = plan_candidate(&c, None, &installed).unwrap();
    assert!(!p.items[0].reviewed && p.items[0].native);
    assert_eq!(p.items[0].sides, ["client"]);
    assert_eq!(p.items[0].replaces.as_deref(), Some("0.7.0"));
    assert_eq!(p.token.as_deref(), Some(c.sha256.as_str()));
    assert_eq!(p.missing.len(), 1, "the server-side dependency is not planned");
    let idx = index_with(&[("pixel-shaders", vec![version_json("pixel-shaders", "0.7.2", &bytes, "")], "")]);
    assert!(plan_candidate(&c, Some(&idx), &HashMap::new()).unwrap().items[0].reviewed);
    let both = cand(pkg_sides("both", "1.0.0", "server = true\nclient = true", ""));
    assert_eq!(plan_candidate(&both, None, &HashMap::new()).unwrap().items[0].sides, ["server", "client"]);
    let srv = cand(pkg_sides("srv", "1.0.0", "server = true", ""));
    assert_eq!(plan_candidate(&srv, None, &HashMap::new()).unwrap_err().code, "serverOnly");
    let mut c2 = c.clone();
    c2.manifest.name = BTreeMap::from([("tr".to_string(), "Gölge".to_string())]);
    assert_eq!(plan_candidate(&c2, None, &HashMap::new()).unwrap().items[0].name, "Gölge");
    c2.manifest.name.clear();
    assert_eq!(plan_candidate(&c2, None, &HashMap::new()).unwrap().items[0].name, "pixel-shaders");
}

#[test]
fn downloads_check_size_hash_and_manifest() {
    let bytes = pkg("aaa", "1.0.0", "");
    let other = pkg("bbb", "1.0.0", "");
    let idx = index_with(&[
        (
            "aaa",
            vec![
                version_json("aaa", "1.0.0", &bytes, ""),
                version_json("aaa", "1.1.0", &bytes, r#","mirror":"https://mirror.test/a""#),
                format!(
                    r#"{{"version":"1.2.0","url":"https://m.test/aaa-1.2.0.bhmod","sha256":"{}","sides":["client"],"api":"^0.2"}}"#,
                    sha(&bytes)
                ),
                version_json("aaa", "1.3.0", &bytes, r#","mirror":"https://mirror.test/missing""#),
            ],
            "",
        ),
        ("bbb", vec![version_json("bbb", "1.0.0", &other, "")], ""),
    ]);
    let net = Net(HashMap::from([
        ("https://m.test/aaa-1.0.0.bhmod".to_string(), bytes.clone()),
        ("https://mirror.test/a".to_string(), bytes.clone()),
        ("https://m.test/aaa-1.2.0.bhmod".to_string(), bytes.clone()),
    ]));
    let c = download_index(&net, &idx, "aaa", "1.0.0").unwrap();
    assert_eq!((c.source.as_str(), c.origin.as_str()), ("index", "https://m.test/aaa-1.0.0.bhmod"));
    let e = download_index(&net, &idx, "aaa", "1.1.0").unwrap_err();
    assert_eq!((e.code.as_str(), e.got.as_deref()), ("mismatch", Some("aaa 1.0.0")), "the mirror was used");
    let e = download_index(&net, &idx, "aaa", "1.3.0").unwrap_err();
    assert_eq!(e.detail.as_deref(), Some("https://m.test/aaa-1.3.0.bhmod"), "the first error is reported");
    assert_eq!(download_index(&net, &idx, "aaa", "9.0.0").unwrap_err().code, "notListed");
    assert_eq!(download_index(&net, &idx, "aaa", "1.2.0").unwrap_err().code, "mismatch", "no size: only the hash");
    assert_eq!(download_index(&net, &idx, "bbb", "1.0.0").unwrap_err().code, "download");
    let v = idx.find("bbb").unwrap().versions[0].clone();
    let net2 = Net(HashMap::from([(v.url.clone(), vec![b'x'; other.len()])]));
    let e = install::download(&net2, "bbb", &v).unwrap_err();
    assert_eq!((e.code.as_str(), e.expected.as_deref()), ("checksum", Some(v.sha256.as_str())));
    let net3 = Net(HashMap::from([(v.url.clone(), b"abc".to_vec())]));
    let small = index::IndexVersion { size: Some(3), sha256: sha(b"abc"), ..v.clone() };
    assert_eq!(install::download(&net3, "bbb", &small).unwrap_err().code, "invalid");
    assert_eq!(
        install::download(&net3, "bbb", &index::IndexVersion { size: Some(2), ..small }).unwrap_err().code,
        "tooLarge"
    );
    let net4 = Net(HashMap::from([(v.url.clone(), other.clone())]));
    let e = install::download(&net4, "bbb", &index::IndexVersion { size: Some(other.len() as u64 + 1), ..v.clone() })
        .unwrap_err();
    assert_eq!(e.code, "size");
}

#[test]
fn write_packages_records_choices() {
    let (_t, d) = temp();
    put(&d.mods, "aaa-0.9.0.bhmod", &pkg("aaa", "0.9.0", ""));
    let a = cand(pkg("aaa", "1.0.0", "[permissions]\nworld = \"read\"\nchannels = true\n"));
    let n = cand(pkg("nat", "1.0.0", "[permissions]\nnative = true\n"));
    let srv = cand(pkg_sides("srv", "1.0.0", "server = true", ""));
    let grants = BTreeMap::from([
        ("aaa".to_string(), vec!["world:read".to_string(), "kick".to_string(), "world:read".to_string()]),
        ("nat".to_string(), vec!["native".to_string()]),
    ]);
    let ch = Choices { grants, enable: true, native_confirmed: false, block_native: false, now: 42 };
    assert_eq!(write_packages(&d.mods, &[a.clone(), srv], &ch).unwrap_err().code, "serverOnly");
    assert_eq!(write_packages(&d.mods, &[a.clone(), n.clone()], &ch).unwrap_err().code, "nativeConfirm");
    let blocked = Choices { block_native: true, native_confirmed: true, ..ch.clone() };
    assert_eq!(write_packages(&d.mods, std::slice::from_ref(&n), &blocked).unwrap_err().code, "nativeBlocked");
    assert!(discover(&d.mods).0.iter().all(|p| p.manifest.version == "0.9.0"), "nothing written on refusal");
    let w = write_packages(&d.mods, &[a, n], &Choices { native_confirmed: true, ..ch.clone() }).unwrap();
    assert_eq!(w, ["aaa", "nat"]);
    let files: Vec<String> = discover(&d.mods).0.iter().map(|p| file_name(&p.path)).collect();
    assert_eq!(files, ["aaa-1.0.0.bhmod", "nat-1.0.0.bhmod"]);
    let cfg = read_config(&d.mods).unwrap();
    assert_eq!(cfg["aaa"].grant, ["world:read"]);
    assert_eq!(cfg["nat"].grant, ["native"]);
    let rec = read_records(&d.mods);
    assert_eq!((rec["nat"].native_allowed, rec["aaa"].native_allowed, rec["aaa"].installed), (Some(42), None, 42));
    put(&d.mods, CONFIG_FILE, b"[[[");
    let b = cand(pkg("bbb", "1.0.0", ""));
    assert_eq!(write_packages(&d.mods, &[b], &ch).unwrap_err().code, "config");
}

#[cfg(unix)]
#[test]
fn write_packages_io_failures() {
    if root() {
        return;
    }
    let ch = Choices { enable: true, ..Choices::default() };
    let b = cand(pkg("bbb", "1.0.0", ""));
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("f"), "").unwrap();
    assert_eq!(write_packages(&t.path().join("f/mods"), std::slice::from_ref(&b), &ch).unwrap_err().code, "io");
    let (_t, d) = temp();
    std::fs::create_dir_all(&d.mods).unwrap();
    chmod(&d.mods, 0o555);
    let r = write_packages(&d.mods, std::slice::from_ref(&b), &ch);
    chmod(&d.mods, 0o755);
    assert_eq!(r.unwrap_err().code, "io", "the part file cannot be written");
    std::fs::create_dir_all(d.mods.join("bbb-1.0.0.bhmod/x")).unwrap();
    assert_eq!(
        write_packages(&d.mods, std::slice::from_ref(&b), &ch).unwrap_err().code,
        "io",
        "the target is a directory"
    );
    std::fs::remove_dir_all(d.mods.join("bbb-1.0.0.bhmod")).unwrap();
    std::fs::create_dir_all(d.mods.join(META_FILE)).unwrap();
    assert_eq!(
        write_packages(&d.mods, std::slice::from_ref(&b), &ch).unwrap_err().code,
        "io",
        "the records cannot be written"
    );
    std::fs::remove_dir_all(d.mods.join(META_FILE)).unwrap();
    write_packages(&d.mods, std::slice::from_ref(&b), &ch).unwrap();
    chmod(&d.mods.join(CONFIG_FILE), 0o444);
    let newer = cand(pkg("bbb", "1.1.0", ""));
    assert_eq!(write_packages(&d.mods, &[newer], &ch).unwrap_err().code, "io", "mods.toml is read-only");
}

#[cfg(unix)]
#[test]
fn an_old_version_behind_a_link_is_replaced() {
    if root() {
        return;
    }
    // The old package is a link into a read-only folder: the link goes, its target stays.
    let (_t, d) = temp();
    let ro = d.mods.parent().unwrap().join("ro");
    put(&ro, "old.bhmod", &pkg("bbb", "0.9.0", ""));
    std::fs::create_dir_all(&d.mods).unwrap();
    std::os::unix::fs::symlink(ro.join("old.bhmod"), d.mods.join("old.bhmod")).unwrap();
    chmod(&ro, 0o555);
    write_packages(&d.mods, &[cand(pkg("bbb", "1.0.0", ""))], &Choices::default()).unwrap();
    chmod(&ro, 0o755);
    assert!(!d.mods.join("old.bhmod").exists());
}

#[test]
fn auto_update_installs_only_quiet_updates() {
    let (_t, d) = temp();
    let a1 = pkg("aaa", "1.0.0", "[permissions]\nchannels = true\n");
    let a2 = pkg("aaa", "1.1.0", "[permissions]\nchannels = true\n");
    let b1 = pkg("bbb", "1.0.0", "");
    let b2 = pkg("bbb", "2.0.0", "[permissions]\nworld = \"read\"\n");
    let c1 = pkg("ccc", "1.0.0", "");
    let c2 = pkg("ccc", "1.1.0", "");
    let n1 = pkg("nnn", "1.0.0", "[permissions]\nnative = true\n");
    let q1 = pkg("qqq", "1.0.0", "");
    let q2 = pkg("qqq", "1.1.0", "[permissions]\nnative = true\n");
    for (n, b) in [("a", &a1), ("b", &b1), ("c", &c1), ("n", &n1), ("q", &q1)] {
        put(&d.mods, &format!("{n}.bhmod"), b);
    }
    edit_config(&d.mods, |c| {
        c.insert("aaa".into(), ModConfig { enabled: false, grant: vec!["channels".into()] });
    })
    .unwrap();
    let idx = index_with(&[
        ("aaa", vec![version_json("aaa", "1.1.0", &a2, r#","permissions":{"channels":true}"#)], ""),
        ("bbb", vec![version_json("bbb", "2.0.0", &b2, r#","permissions":{"world":"read"}"#)], ""),
        ("ccc", vec![version_json("ccc", "1.1.0", &c2, "")], ""),
        ("nnn", vec![version_json("nnn", "1.1.0", &n1, r#","permissions":{"native":true}"#)], ""),
        ("qqq", vec![version_json("qqq", "1.1.0", &q2, "")], ""),
    ]);
    let net = Net(HashMap::from([
        ("https://m.test/aaa-1.1.0.bhmod".to_string(), a2),
        ("https://m.test/qqq-1.1.0.bhmod".to_string(), q2),
    ]));
    let r = auto_update(&net, &d.mods, &idx, 7);
    assert_eq!(r.updated, [("aaa".to_string(), "1.1.0".to_string())]);
    let failed: Vec<(&str, &str)> = r.failed.iter().map(|(id, e)| (id.as_str(), e.code.as_str())).collect();
    assert_eq!(failed, [("ccc", "download"), ("qqq", "nativeConfirm")]);
    let cfg = read_config(&d.mods).unwrap();
    assert_eq!((cfg["aaa"].enabled, cfg["aaa"].grant.clone()), (false, vec!["channels".to_string()]));
    assert_eq!(installed_versions(&d.mods)["bbb"], "1.0.0");
    assert_eq!(install::installed(&d.mods)["aaa"], "1.1.0");
}

#[test]
fn server_cache_shares_files_and_clears() {
    let (_t, d) = temp();
    assert!(cache::list(&d).servers.is_empty());
    assert_eq!(server_key(" Mira.example.net:15151 "), "mira.example.net_15151");
    assert_eq!(server_key("..x"), "x");
    assert_eq!(server_key(""), "_");
    let mm = cand(pkg("minimap", "0.9.1", ""));
    let pet = cand(pkg("pets", "1.2.0", ""));
    let tl = cand(pkg("turbo", "2.0.1", "[permissions]\nnative = true\n"));
    let race = cand(pkg("race", "0.5.0", ""));
    let k1 = cache::record_join(&d, "Sunny Isle", "mira.example.net:15151", &[mm.clone(), pet, tl], 300).unwrap();
    let k2 = cache::record_join(&d, "Dodo Land", "192.168.1.20:15151", &[mm.clone(), race], 200).unwrap();
    assert_eq!(cache::cached_blob(&d, &mm.sha256).unwrap(), d.blobs().join(format!("{}.bhmod", mm.sha256)));
    assert!(cache::cached_blob(&d, "../etc").is_none());
    assert!(cache::cached_blob(&d, &"z".repeat(64)).is_none());
    assert!(cache::cached_blob(&d, &"0".repeat(64)).is_none());
    let ov = cache::list(&d);
    assert_eq!(ov.servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Sunny Isle", "Dodo Land"]);
    assert_eq!(ov.servers[1].mods[0].shared_with, ["Sunny Isle"]);
    assert!(ov.servers[0].mods[2].m.native);
    assert_eq!(std::fs::read_dir(d.blobs()).unwrap().count(), 4, "minimap stored once");
    assert_eq!(ov.total, crate::datadir::dir_size(&d.blobs()));
    put(&d.servers(), "junk.toml", b"[[[");
    put(&d.servers(), "readme.txt", b"x");
    assert_eq!(cache::list(&d).servers.len(), 2);
    cache::clear(&d, &k1).unwrap();
    assert!(cache::cached_blob(&d, &mm.sha256).is_some());
    assert_eq!(std::fs::read_dir(d.blobs()).unwrap().count(), 2);
    for bad in [k1.as_str(), "../x", "", ".x"] {
        assert_eq!(cache::clear(&d, bad).unwrap_err().code, "missing");
    }
    cache::record_join(&d, "Dodo Land", "192.168.1.20:15151", std::slice::from_ref(&mm), 400).unwrap();
    cache::clear(&d, &k2).unwrap();
    assert_eq!(std::fs::read_dir(d.blobs()).unwrap().count(), 0);
    cache::clear_all(&d).unwrap();
    cache::clear_all(&d).unwrap();
    cache::gc(&d).unwrap();
    let mut odd = mm.clone();
    odd.manifest.name = BTreeMap::from([("ru".to_string(), "Карта".to_string())]);
    let mut noname = mm.clone();
    noname.manifest.name.clear();
    cache::record_join(&d, "S", "s:1", &[odd, noname], 1).unwrap();
    let names: Vec<String> = cache::list(&d).servers[0].mods.iter().map(|m| m.m.name.clone()).collect();
    assert_eq!(names, ["Карта", "minimap"]);
}

#[test]
fn server_cache_limit_clears_oldest() {
    let (_t, d) = temp();
    let a = cand(pkg("aaa", "1.0.0", ""));
    let b = cand(pkg("bbb", "1.0.0", ""));
    let c = cand(pkg("ccc", "1.0.0", ""));
    cache::record_join(&d, "Old", "old:1", &[a], 1).unwrap();
    cache::record_join(&d, "Mid", "mid:1", &[b], 2).unwrap();
    cache::record_join(&d, "New", "new:1", std::slice::from_ref(&c), 3).unwrap();
    assert!(cache::enforce_limit(&d, 0).unwrap().is_empty());
    assert!(cache::enforce_limit(&d, u64::MAX).unwrap().is_empty());
    assert_eq!(cache::enforce_limit(&d, c.bytes.len() as u64 + 1).unwrap(), ["old_1", "mid_1"]);
    assert_eq!(cache::list(&d).servers[0].name, "New");
}

#[cfg(unix)]
#[test]
fn server_cache_io_failures() {
    if root() {
        return;
    }
    let a = cand(pkg("aaa", "1.0.0", ""));
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("f"), "").unwrap();
    let bad = ModsDirs::new(&t.path().join("f"));
    assert_eq!(cache::record_join(&bad, "S", "s:1", std::slice::from_ref(&a), 1).unwrap_err().code, "io");
    let (_t, d) = temp();
    std::fs::create_dir_all(d.blobs()).unwrap();
    std::fs::write(d.servers(), "").unwrap();
    assert_eq!(cache::record_join(&d, "S", "s:1", std::slice::from_ref(&a), 1).unwrap_err().code, "io");
    std::fs::remove_file(d.servers()).unwrap();
    chmod(&d.blobs(), 0o555);
    let r = cache::record_join(&d, "S", "s:1", std::slice::from_ref(&a), 1);
    chmod(&d.blobs(), 0o755);
    assert_eq!(r.unwrap_err().code, "io", "the part file cannot be written");
    std::fs::create_dir_all(d.blobs().join(format!("{}.part/x", a.sha256))).unwrap();
    assert_eq!(
        cache::record_join(&d, "S", "s:1", std::slice::from_ref(&a), 1).unwrap_err().code,
        "io",
        "the part path is a directory"
    );
    std::fs::remove_dir_all(d.blobs().join(format!("{}.part", a.sha256))).unwrap();
    std::fs::create_dir_all(d.servers().join("s_1.toml")).unwrap();
    assert_eq!(
        cache::record_join(&d, "S", "s:1", std::slice::from_ref(&a), 1).unwrap_err().code,
        "io",
        "the record path is a directory"
    );
    std::fs::remove_dir_all(d.servers().join("s_1.toml")).unwrap();
    cache::record_join(&d, "S", "s:1", std::slice::from_ref(&a), 1).unwrap();
    std::fs::create_dir_all(d.blobs().join("stray")).unwrap();
    assert_eq!(cache::clear(&d, "s_1").unwrap_err().code, "io", "gc cannot remove a directory");
    std::fs::remove_dir_all(d.blobs().join("stray")).unwrap();
    cache::record_join(&d, "S", "s:1", std::slice::from_ref(&a), 1).unwrap();
    std::fs::create_dir_all(d.blobs().join("stray")).unwrap();
    assert_eq!(cache::enforce_limit(&d, 1).unwrap_err().code, "io");
    std::fs::remove_dir_all(d.blobs().join("stray")).unwrap();
    cache::record_join(&d, "S", "s:1", std::slice::from_ref(&a), 1).unwrap();
    chmod(&d.servers(), 0o555);
    let r = cache::clear(&d, "s_1");
    chmod(&d.servers(), 0o755);
    assert_eq!(r.unwrap_err().code, "io", "the record cannot be removed");
    chmod(&d.cache, 0o555);
    let r = cache::clear_all(&d);
    chmod(&d.cache, 0o755);
    assert_eq!(r.unwrap_err().code, "io");
}

#[test]
fn developer_folders() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("my-mod");
    std::fs::create_dir_all(root.join("client")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("client/main.luau"), "return {}\n").unwrap();
    std::fs::write(
        root.join("mod.toml"),
        "schema = 1\nid = \"my-mod\"\nversion = \"0.1.0-dev\"\nname = { en = \"My mod\" }\nauthors = [\"me\"]\nlicense = \"MIT\"\n\
         api = \"^0.2\"\n[sides]\nclient = true\n[entry.client]\nruntime = \"lua\"\npath = \"client/main.luau\"\n[permissions]\nnative = true\n",
    )
    .unwrap();
    let m = dev::inspect(&root).unwrap();
    assert_eq!(
        (m.id.as_deref(), m.version.as_deref(), m.name.as_deref()),
        (Some("my-mod"), Some("0.1.0-dev"), Some("My mod"))
    );
    assert!(m.errors.is_empty() && m.native, "{m:?}");
    assert_eq!(m.permissions, ["native"]);
    std::fs::write(root.join("mod.toml"), "schema = 1\n").unwrap();
    let m = dev::inspect(&root).unwrap();
    assert!(m.id.is_none() && !m.errors.is_empty() && !m.native);
    assert_eq!(dev::inspect(&t.path().join("none")).unwrap_err().code, "io");
    let f = dev::add_folder(&[], "/a");
    assert_eq!(dev::add_folder(&f, "/a"), ["/a"]);
    assert_eq!(dev::add_folder(&f, "/b"), ["/a", "/b"]);
    // Reload: only with a running game in the launcher's window.
    assert_eq!(dev::reload(None).unwrap_err().code, "notRunning");
    let (tx, rx) = std::sync::mpsc::channel();
    dev::reload(Some(&tx)).unwrap();
    assert_eq!(rx.recv().unwrap(), "mods-reload");
    drop(rx);
    assert_eq!(dev::reload(Some(&tx)).unwrap_err().code, "notRunning");
    // The fingerprint follows the files.
    let a = dev::fingerprint(&root);
    assert_eq!(a, dev::fingerprint(&root));
    std::fs::create_dir_all(root.join("client/sub")).unwrap();
    std::fs::write(root.join("client/sub/x.luau"), "return 1").unwrap();
    let b = dev::fingerprint(&root);
    assert_ne!(a, b);
    std::fs::write(root.join("client/sub/x.luau"), "return 22").unwrap();
    assert_ne!(b, dev::fingerprint(&root));
    assert_eq!(dev::fingerprint(&t.path().join("none")), dev::fingerprint(&t.path().join("none2")));
}

#[test]
fn live_reports_from_the_game() {
    use super::live;
    let line = r#"@mods [{"id":"aaa","version":"1.0.0","state":"budget","origin":"mine","ms":1.5,"peak_ms":30.0,"calls":9,"reason":"3 faults within 60 s"},{"id":"srv","version":"1.0.0","state":"running","origin":"server","ms":0.0,"peak_ms":0.0,"calls":1}]"#;
    let l = live::parse(line).unwrap();
    assert_eq!((l.len(), l[1].reason.as_deref()), (2, None));
    assert_eq!(live::parse("@mods nope"), None);
    assert_eq!(live::parse("[mods] x"), None);
    let t = tempfile::tempdir().unwrap();
    let dir = t.path();
    std::fs::write(dir.join("aaa.bhmod"), pkg("aaa", "1.0.0", "")).unwrap();
    std::fs::write(dir.join("bbb.bhmod"), pkg("bbb", "1.0.0", "")).unwrap();
    let mut ov = overview(dir, None, false);
    assert_eq!(ov.attention, 0);
    let mut l2 = l.clone();
    l2.push(live::LiveMod { id: "bbb".into(), state: "loaded".into(), ..l[0].clone() });
    live::apply(&mut ov, &l2);
    let a = ov.mods.iter().find(|m| m.id == "aaa").unwrap();
    assert_eq!((a.status.as_str(), a.reason.as_deref()), ("budget", Some("3 faults within 60 s")));
    assert_eq!(a.live.as_ref().unwrap().calls, 9);
    let b = ov.mods.iter().find(|m| m.id == "bbb").unwrap();
    assert_eq!((b.status.as_str(), b.live.is_some()), ("ready", true));
    assert_eq!(ov.attention, 1);
    assert_eq!(live::args(&crate::prefs::Prefs::default()), Vec::<String>::new());
    // JVM mods: the flag, and the JRE only when one is named.
    let jvm = crate::prefs::Prefs { mods_jvm: true, ..Default::default() };
    assert_eq!(live::args(&jvm), ["--mods-jvm"]);
    let jvm = crate::prefs::Prefs { mods_jvm: true, mods_java_home: "/opt/jre".into(), ..Default::default() };
    assert_eq!(live::args(&jvm), ["--mods-jvm", "--mods-java-home", "/opt/jre"]);
    let home_only = crate::prefs::Prefs { mods_java_home: "/opt/jre".into(), ..Default::default() };
    assert!(live::args(&home_only).is_empty());
}

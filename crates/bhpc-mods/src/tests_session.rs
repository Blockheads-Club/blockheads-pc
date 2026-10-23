//! Native libraries of a session folder.

use std::io::Write;

use crate::session::{build, native_libs, NativeMod, Plan};

fn pkg(id: &str, native: bool, libs: &[&str]) -> Vec<u8> {
    let toml = format!(
        "schema = 1\nid = \"{id}\"\nversion = \"1.0.0\"\nname = {{ en = \"Mod {id}\" }}\nauthors = [\"a\"]\n\
         license = \"MIT\"\napi = \"^0.2\"\n[sides]\nclient = true\n[entry.client]\nruntime = \"lua\"\n\
         path = \"client/main.luau\"\n[permissions]\nnative = {native}\n"
    );
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    w.start_file("mod.toml", o).unwrap();
    w.write_all(toml.as_bytes()).unwrap();
    w.start_file("client/main.luau", o).unwrap();
    w.write_all(b"return {}\n").unwrap();
    for l in libs {
        w.start_file(*l, o).unwrap();
        w.write_all(b"ELF").unwrap();
    }
    w.finish().unwrap().into_inner()
}

#[test]
fn granted_native_mods_of_every_origin_give_their_libraries() {
    let t = tempfile::tempdir().unwrap();
    let own = t.path().join("own");
    std::fs::create_dir_all(&own).unwrap();
    let lib = "client/native/armeabi-v7a/libn.so";
    // mine: native and granted; plain: not native; off: native but not granted; empty: granted, no library
    for (id, native, libs) in [("nat", true, vec![lib]), ("plain", false, vec![]), ("empty", true, vec![])] {
        std::fs::write(own.join(format!("{id}.bhmod")), pkg(id, native, &libs)).unwrap();
    }
    std::fs::write(own.join("mods.toml"), "[nat]\ngrant = [\"native\"]\n[empty]\ngrant = [\"native\"]\n").unwrap();
    // a server mod with a library
    let srv = t.path().join("srv.bhmod");
    std::fs::write(&srv, pkg("srvnat", true, &[lib, "client/native/arm64-v8a/libn.so"])).unwrap();
    // a developer folder with a library
    let dev = t.path().join("devmod");
    std::fs::create_dir_all(dev.join("client/native/armeabi-v7a")).unwrap();
    std::fs::create_dir_all(dev.join("client")).unwrap();
    std::fs::write(dev.join("client/native/armeabi-v7a/libd.so"), b"ELF").unwrap();
    std::fs::write(dev.join("client/main.luau"), "return {}\n").unwrap();
    std::fs::write(
        dev.join("mod.toml"),
        "schema = 1\nid = \"devnat\"\nversion = \"1.0.0\"\nname = { en = \"Dev\" }\nauthors = [\"a\"]\nlicense = \"MIT\"\n\
         api = \"^0.2\"\n[sides]\nclient = true\n[entry.client]\nruntime = \"lua\"\npath = \"client/main.luau\"\n\
         [permissions]\nnative = true\n",
    )
    .unwrap();
    let plan = Plan {
        own: Some(own),
        dev: vec![dev],
        server: vec![crate::join::Accepted { id: "srvnat".into(), path: srv, grant: vec!["native".into()] }],
    };
    let session = t.path().join("session");
    let (placed, problems) = build(&session, &plan).unwrap();
    assert_eq!(placed.len(), 5, "{problems:?}");
    let (found, problems) = native_libs(&session, &t.path().join("unpacked"));
    let ids: Vec<(&str, usize)> = found.iter().map(|m| (m.id.as_str(), m.libs.len())).collect();
    assert_eq!(ids, [("devnat", 1), ("nat", 1), ("srvnat", 1)]);
    assert!(found.iter().all(|m: &NativeMod| m.libs.iter().all(|l| l.is_file())));
    assert!(found[0].libs[0].ends_with("libd.so"));
    assert!(found[1].libs[0].starts_with(t.path().join("unpacked")));
    assert_eq!(problems.len(), 1);
    assert!(problems[0].starts_with("empty: no armeabi-v7a library"), "{problems:?}");
    // a granted package that cannot be read
    std::fs::write(session.join("m-nat.bhmod"), b"junk").unwrap();
    let (_, problems) = native_libs(&session, &t.path().join("unpacked2"));
    assert!(problems.iter().any(|p| p.starts_with("nat: cannot unpack")), "{problems:?}");
    // an unreadable session folder has nothing
    assert!(native_libs(&t.path().join("none"), t.path()).0.is_empty());
}

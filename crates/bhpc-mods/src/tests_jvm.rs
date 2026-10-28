//! A Java client mod through the client host: the same events and effects as a Luau mod (needs a JDK and a JRE).

use std::io::Write;
use std::path::Path;
use std::process::Command;

use bhmod_core::jvm::SDK_JAR;

use crate::host::*;
use crate::join::Accepted;
use crate::session::{build, Plan};

const MOD: &str = r#"
package fx;

import club.blockheads.api.events.ChatEvent;
import club.blockheads.mod.*;

public final class Echo implements Mod {
    public void onEnable(Api api) {
        api.onChat(e -> {
            if (e.text.equals("hide")) {
                e.cancelled = true;
            } else if (e.text.equals("up")) {
                e.text = "UP";
            }
        });
        api.command("hello", "Says hello.", (sender, label, args) -> {
            api.players().broadcast("everyone");
            return Replies.reply("hi " + args.size());
        });
        api.warn("enabled");
    }
}
"#;

fn tool(name: &str) -> std::path::PathBuf {
    for var in ["BHMOD_JAVA_HOME", "JAVA_HOME"] {
        if let Some(home) = std::env::var_os(var) {
            let p = Path::new(&home).join("bin").join(name);
            if p.is_file() {
                return p;
            }
        }
    }
    name.into()
}

fn run(c: &mut Command) {
    let out = c.output().expect("run the JDK tool (the jvm tests need a JDK)");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

/// The package of the Java client mod `echo`.
fn jvm_pkg(work: &Path) -> Vec<u8> {
    let sdk = work.join("sdk.jar");
    std::fs::write(&sdk, SDK_JAR).unwrap();
    let src = work.join("Echo.java");
    std::fs::write(&src, MOD).unwrap();
    let classes = work.join("classes");
    std::fs::create_dir_all(&classes).unwrap();
    run(Command::new(tool("javac"))
        .args(["-nowarn", "-Xlint:-options", "--release", "8", "-cp"])
        .arg(&sdk)
        .arg("-d")
        .arg(&classes)
        .arg(&src));
    let mf = work.join("mf");
    std::fs::write(&mf, "Manifest-Version: 1.0\nBhmod-Main: fx.Echo\n").unwrap();
    let jar = work.join("mod.jar");
    run(Command::new(tool("jar")).arg("cfm").arg(&jar).arg(&mf).arg("-C").arg(&classes).arg("."));
    let toml = "schema = 1\nid = \"echo\"\nversion = \"1.0.0\"\nname = { en = \"Echo\" }\nauthors = [\"a\"]\nlicense = \"MIT\"\n\
                api = \"^0.2\"\n[sides]\nclient = true\n[entry.client]\nruntime = \"jvm\"\npath = \"client/mod.jar\"\n\
                [permissions]\nnative = true\nplayers = [\"chat\"]\n";
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    w.start_file("mod.toml", o).unwrap();
    w.write_all(toml.as_bytes()).unwrap();
    w.start_file("client/mod.jar", o).unwrap();
    w.write_all(&std::fs::read(&jar).unwrap()).unwrap();
    w.finish().unwrap().into_inner()
}

#[test]
fn a_java_client_mod_runs_like_a_lua_one() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("echo.bhmod");
    std::fs::write(&p, jvm_pkg(d.path())).unwrap();
    let plan = Plan {
        own: None,
        dev: vec![],
        server: vec![Accepted { id: "echo".into(), path: p, grant: vec!["native".into(), "players:chat".into()] }],
    };
    let mods = d.path().join("session");
    let (placed, problems) = build(&mods, &plan).unwrap();
    assert_eq!(placed.len(), 1);
    assert!(problems.is_empty(), "{problems:?}");
    let (runtimes, note) = session_runtimes(d.path(), true, None);
    assert!(note.unwrap().starts_with("JVM mods are on:"));
    let mut h = ClientHost::load(HostSetup {
        mods_dir: &mods,
        storage_dir: &d.path().join("storage"),
        runtimes,
        online: true,
        local_player: "ME".into(),
        voice: None,
        binds: None,
        client_settings: None,
        hud: None,
    });
    h.enable("World", "w1");
    let logs = h.take_new_logs();
    assert!(
        logs.iter().any(|l| l.text == "enabled" && l.level == "warn" && l.mod_id.as_deref() == Some("echo")),
        "{logs:?}"
    );
    assert_eq!(h.chat_received("BOB", "hide"), None);
    assert_eq!(h.chat_received("BOB", "up"), Some("UP".into()));
    assert_eq!(h.chat_received("BOB", "same"), Some("same".into()));
    assert!(h.typed("/hello a b"));
    assert_eq!(h.take_effects(), vec![Effect::Chat("everyone".into()), Effect::Chat("hi 2".into())]);
    let st = h.status();
    assert_eq!((st[0].id.as_str(), st[0].state), ("echo", "running"));
    h.stop();
}

//! Links, mail, sharing, the clipboard, saved pictures and the help page (`jlib/desktop.rs`).

use crate::jlib::desktop::{desktop_url, html_to_text, mailto, pct, relative_refs, COPIED, HOST_PACKAGE};
use crate::tests::Env;
use crate::*;
use std::sync::{Arc, Mutex};

/// Records what the game asked of the desktop.
#[derive(Default)]
struct Rec {
    log: Mutex<Vec<String>>,
    clip: Mutex<Option<String>>,
}

impl Rec {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.lock().unwrap())
    }
}

impl SystemHooks for Rec {
    fn open_url(&self, url: &str) -> bool {
        self.log.lock().unwrap().push(format!("open {url}"));
        true
    }
    fn show_html(&self, html: &str, files: &[(String, Vec<u8>)]) -> bool {
        let names: Vec<&str> = files.iter().map(|f| f.0.as_str()).collect();
        self.log.lock().unwrap().push(format!("html {} {names:?}", html.len()));
        true
    }
    fn set_clipboard(&self, text: &str) {
        self.log.lock().unwrap().push(format!("copy {text}"));
    }
    fn clipboard(&self) -> Option<String> {
        self.clip.lock().unwrap().clone()
    }
    fn notify(&self, text: &str) {
        self.log.lock().unwrap().push(format!("notify {text}"));
    }
    fn picture_saved(&self, path: &str) {
        self.log.lock().unwrap().push(format!("picture {path}"));
    }
}

fn env() -> (Env, Arc<Rec>) {
    let rec = Arc::new(Rec::default());
    let r = rec.clone();
    let e = Env::with(move |cfg| {
        cfg.system = r;
        cfg.assets = Box::new(MemAssets(vec![
            ("GameResources/instructions/style.css".into(), b"body { background: url(bg.jpg) }".to_vec()),
            ("GameResources/instructions/bg.jpg".into(), b"jpg".to_vec()),
            ("144Icon.png".into(), b"png".to_vec()),
        ]));
    });
    (e, rec)
}

const URI: &str = "android/net/Uri";
const INTENT: &str = "android/content/Intent";
const SI: &str = "(Ljava/lang/String;)Landroid/content/Intent;";

fn uri(e: &mut Env, s: &str) -> Obj {
    let a = e.string(s);
    let r = e.scall(URI, "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[a]);
    e.obj(r)
}

fn text(e: &mut Env, o: Obj, name: &str, sig: &str, args: &[Value]) -> Option<String> {
    let r = e.call(o, name, sig, args).unwrap();
    r.as_ref().map(|s| e.s(s))
}

fn view(e: &mut Env, url: &str) -> Obj {
    let u = uri(e, url);
    let a = e.string("android.intent.action.VIEW");
    e.new_obj(INTENT, "(Ljava/lang/String;Landroid/net/Uri;)V", &[a, Value::Ref(Some(u))])
}

fn start(e: &mut Env, i: Obj) {
    let a = e.activity();
    e.call(a, "startActivity", "(Landroid/content/Intent;)V", &[Value::Ref(Some(i))]).unwrap();
}

#[test]
fn helpers() {
    assert_eq!(desktop_url("https://x.y/a?b=1").as_deref(), Some("https://x.y/a?b=1"));
    assert_eq!(desktop_url("HTTP://x").as_deref(), Some("HTTP://x"));
    assert_eq!(desktop_url("mailto:a@b").as_deref(), Some("mailto:a@b"));
    assert_eq!(
        desktop_url("market://details?id=com.x").as_deref(),
        Some("https://play.google.com/store/apps/details?id=com.x")
    );
    assert_eq!(desktop_url("market://details").as_deref(), Some("https://play.google.com/store/apps/details?"));
    assert_eq!(desktop_url("fb://page/1"), None);
    assert_eq!(desktop_url("file:///etc/passwd"), None);
    assert_eq!(desktop_url("no scheme"), None);
    assert_eq!(pct("a b&c/é~", ""), "a%20b%26c%2F%C3%A9~");
    assert_eq!(pct("x@y", "@"), "x@y");
    assert_eq!(mailto("", "", "", "", ""), "mailto:");
    assert_eq!(
        mailto("a@b.c, d@e.f", "g@h", " ", "Hi there", "L1\nL2&"),
        "mailto:a@b.c,d@e.f?subject=Hi%20there&body=L1%0AL2%26&cc=g@h"
    );
    assert_eq!(mailto("a@b", "", "z@y", "", ""), "mailto:a@b?bcc=z@y");
    assert_eq!(html_to_text("<p>A &amp; B</p><br/>C&lt;D&gt;&quot;&#39;&nbsp;<div>x</div>"), "A & B\n\nC<D>\"' x\n");
    assert_eq!(html_to_text("open < tag"), "open < tag");
    assert_eq!(
        relative_refs("<img src=\"a.png\"><a href='b.html'><a href=\"https://x\"><img src=\"a.png\"> url('c.jpg') src=\"/abs\" href=\"#top\" src=\"../up\" src=\"\""),
        ["a.png", "b.html", "c.jpg"]
    );
}

#[test]
fn uri_parts() {
    let (mut e, _) = env();
    let u = uri(&mut e, "https://user@theblockheads.net:80/join.php?ip=1.2.3.4&port=15151&flag#frag");
    let s = "()Ljava/lang/String;";
    assert_eq!(text(&mut e, u, "getScheme", s, &[]).as_deref(), Some("https"));
    assert_eq!(text(&mut e, u, "getHost", s, &[]).as_deref(), Some("theblockheads.net"));
    assert_eq!(text(&mut e, u, "getPath", s, &[]).as_deref(), Some("/join.php"));
    assert_eq!(text(&mut e, u, "getQuery", s, &[]).as_deref(), Some("ip=1.2.3.4&port=15151&flag"));
    let q = "(Ljava/lang/String;)Ljava/lang/String;";
    for (k, v) in [("ip", Some("1.2.3.4")), ("flag", Some("")), ("nope", None)] {
        let a = e.string(k);
        assert_eq!(text(&mut e, u, "getQueryParameter", q, &[a]).as_deref(), v);
    }
    assert!(text(&mut e, u, "toString", s, &[]).unwrap().ends_with("#frag"));
    let m = uri(&mut e, "mailto:a@b");
    assert_eq!(text(&mut e, m, "getHost", s, &[]), None);
    assert_eq!(text(&mut e, m, "getPath", s, &[]).as_deref(), Some("a@b"));
    assert_eq!(text(&mut e, m, "getQuery", s, &[]), None);
    let rel = uri(&mut e, "a/b:c#x");
    assert_eq!(text(&mut e, rel, "getScheme", s, &[]), None);
    assert_eq!(text(&mut e, rel, "getPath", s, &[]).as_deref(), Some("a/b:c"));
    let a = e.string("a b");
    assert_eq!(
        text(&mut e, u, "toString", s, &[]).map(|_| ()).and_then(|_| {
            let r = e.scall(URI, "encode", "(Ljava/lang/String;)Ljava/lang/String;", &[a]).unwrap();
            r.as_ref().map(|o| e.s(o))
        }),
        Some("a%20b".to_string())
    );
    assert!(e.scall(URI, "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[Value::Ref(None)]).is_err());
    let f = e.new_obj("java/io/File", "(Ljava/lang/String;)V", &[e.string("/sdcard/x.png")]);
    let r = e.scall(URI, "fromFile", "(Ljava/io/File;)Landroid/net/Uri;", &[Value::Ref(Some(f))]);
    let fu = e.obj(r);
    assert_eq!(text(&mut e, fu, "toString", s, &[]).as_deref(), Some("file:///sdcard/x.png"));
    let r = e.scall(URI, "fromFile", "(Ljava/io/File;)Landroid/net/Uri;", &[Value::Ref(None)]);
    let fu = e.obj(r);
    assert_eq!(text(&mut e, fu, "toString", s, &[]).as_deref(), Some("file://"));
}

#[test]
fn intent_fields_and_extras() {
    let (mut e, _) = env();
    let i = e.new_obj(INTENT, "()V", &[]);
    let s = "()Ljava/lang/String;";
    assert_eq!(text(&mut e, i, "getDataString", s, &[]), None);
    for (set, get, v) in
        [("setAction", "getAction", "act"), ("setType", "getType", "t/x"), ("setPackage", "getPackage", "p")]
    {
        let a = e.string(v);
        assert_eq!(e.call(i, set, SI, &[a]), Ok(Value::Ref(Some(i))));
        assert_eq!(text(&mut e, i, get, s, &[]).as_deref(), Some(v));
    }
    let u = uri(&mut e, "http://a");
    e.call(i, "setData", "(Landroid/net/Uri;)Landroid/content/Intent;", &[Value::Ref(Some(u))]).unwrap();
    assert_eq!(e.call(i, "getData", "()Landroid/net/Uri;", &[]), Ok(Value::Ref(Some(u))));
    assert_eq!(text(&mut e, i, "getDataString", s, &[]).as_deref(), Some("http://a"));
    e.call(i, "setFlags", "(I)Landroid/content/Intent;", &[Value::Int(1)]).unwrap();
    e.call(i, "addFlags", "(I)Landroid/content/Intent;", &[Value::Int(4)]).unwrap();
    assert_eq!(e.call(i, "getFlags", "()I", &[]), Ok(Value::Int(5)));
    let c = e.string("cat");
    assert_eq!(e.call(i, "addCategory", SI, &[c]), Ok(Value::Ref(Some(i))));
    let k = |e: &Env, s: &str| e.string(s);
    let arr = e.vm.new_string_array(&["x@y".into()]);
    for (sig, v) in [
        ("(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;", k(&e, "one")),
        ("(Ljava/lang/String;Ljava/lang/CharSequence;)Landroid/content/Intent;", k(&e, "two")),
        ("(Ljava/lang/String;[Ljava/lang/String;)Landroid/content/Intent;", Value::Ref(Some(arr))),
        ("(Ljava/lang/String;Landroid/os/Parcelable;)Landroid/content/Intent;", Value::Ref(Some(u))),
        ("(Ljava/lang/String;Z)Landroid/content/Intent;", Value::Int(1)),
        ("(Ljava/lang/String;I)Landroid/content/Intent;", Value::Int(7)),
        ("(Ljava/lang/String;J)Landroid/content/Intent;", Value::Long(9)),
    ] {
        let key = k(&e, sig);
        assert_eq!(e.call(i, "putExtra", sig, &[key, v]), Ok(Value::Ref(Some(i))));
    }
    let key = k(&e, "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;");
    let v = k(&e, "again");
    e.call(i, "putExtra", "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;", &[key, v]).unwrap();
    let key = k(&e, "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;");
    assert_eq!(
        text(&mut e, i, "getStringExtra", "(Ljava/lang/String;)Ljava/lang/String;", &[key]).as_deref(),
        Some("again")
    );
    let key = k(&e, "(Ljava/lang/String;I)Landroid/content/Intent;");
    assert_eq!(e.call(i, "hasExtra", "(Ljava/lang/String;)Z", &[key]), Ok(Value::Int(1)));
    let key = k(&e, "missing");
    assert_eq!(e.call(i, "hasExtra", "(Ljava/lang/String;)Z", &[key]), Ok(Value::Int(0)));
    let a = e.activity();
    let cls = e.scall("java/lang/Class", "forName", "(Ljava/lang/String;)Ljava/lang/Class;", &[k(&e, "x.Y")]);
    let cls = e.obj(cls);
    e.new_obj(INTENT, "(Landroid/content/Context;Ljava/lang/Class;)V", &[Value::Ref(Some(a)), Value::Ref(Some(cls))]);
    // An intent whose payload is gone still takes extras (into nothing).
    e.vm.with_data(i, |d| *d = Data::None);
    let key = k(&e, "x");
    e.call(i, "putExtra", "(Ljava/lang/String;I)Landroid/content/Intent;", &[key, Value::Int(1)]).unwrap();
    let key = k(&e, "x");
    assert_eq!(e.call(i, "hasExtra", "(Ljava/lang/String;)Z", &[key]), Ok(Value::Int(0)));
    let one = e.new_obj(INTENT, "(Ljava/lang/String;)V", &[k(&e, "a.b")]);
    assert_eq!(text(&mut e, one, "getAction", s, &[]).as_deref(), Some("a.b"));
}

fn resolve(e: &mut Env, i: Obj) -> (bool, usize) {
    let pm = e.activity();
    let r = e.call(pm, "getPackageManager", "()Landroid/content/pm/PackageManager;", &[]);
    let pm = e.obj(r);
    let r = e
        .call(
            pm,
            "resolveActivity",
            "(Landroid/content/Intent;I)Landroid/content/pm/ResolveInfo;",
            &[Value::Ref(Some(i)), Value::Int(0)],
        )
        .unwrap();
    let l = e.call(
        pm,
        "queryIntentActivities",
        "(Landroid/content/Intent;I)Ljava/util/List;",
        &[Value::Ref(Some(i)), Value::Int(0)],
    );
    let l = e.obj(l);
    let n = e.call(l, "size", "()I", &[]).unwrap().as_int() as usize;
    if let Some(ri) = r.as_ref() {
        let ai = e.vm.get_field(ri, "activityInfo").as_ref().unwrap();
        let p = e.vm.get_field(ai, "packageName").as_ref().unwrap();
        assert_eq!(e.s(p), HOST_PACKAGE);
    }
    (r.as_ref().is_some(), n)
}

#[test]
fn links_open_on_the_desktop() {
    let (mut e, rec) = env();
    let i = view(&mut e, "https://forums.theblockheads.net");
    assert_eq!(resolve(&mut e, i), (true, 1));
    start(&mut e, i);
    let p = e.string(HOST_PACKAGE);
    e.call(i, "setPackage", SI, &[p]).unwrap();
    assert_eq!(resolve(&mut e, i), (true, 1));
    let p = e.string("com.facebook.katana");
    e.call(i, "setPackage", SI, &[p]).unwrap();
    assert_eq!(resolve(&mut e, i), (false, 0));
    let m = view(&mut e, "market://details?id=com.noodlecake.blockheads");
    start(&mut e, m);
    let bad = view(&mut e, "fb://profile/1");
    assert_eq!(resolve(&mut e, bad), (false, 0));
    start(&mut e, bad);
    let none = e.new_obj(INTENT, "(Ljava/lang/String;)V", &[e.string("android.intent.action.VIEW")]);
    assert_eq!(resolve(&mut e, none), (false, 0));
    let other = e.new_obj(INTENT, "(Ljava/lang/String;)V", &[e.string("android.intent.action.PICK")]);
    assert_eq!(resolve(&mut e, other), (false, 0));
    start(&mut e, other);
    let a = e.activity();
    e.call(a, "startActivity", "(Landroid/content/Intent;)V", &[Value::Ref(None)]).unwrap();
    assert_eq!(
        rec.take(),
        [
            "open https://forums.theblockheads.net",
            "open https://play.google.com/store/apps/details?id=com.noodlecake.blockheads"
        ]
    );
    assert!(e.log_text().contains("only http, https and mailto"), "{}", e.log_text());
    assert!(e.log_text().contains("nothing on the desktop handles"));
    let pm = e.activity();
    let r = e.call(pm, "getPackageManager", "()Landroid/content/pm/PackageManager;", &[]);
    let pm = e.obj(r);
    let sig = "(Landroid/content/Intent;I)Landroid/content/pm/ResolveInfo;";
    assert_eq!(e.call(pm, "resolveActivity", sig, &[Value::Ref(None), Value::Int(0)]), Ok(Value::Ref(None)));
}

#[test]
fn shares_and_mail() {
    let (mut e, rec) = env();
    let send = |e: &mut Env, ty: Option<&str>, extras: &[(&str, Value)]| {
        let i = e.new_obj(INTENT, "(Ljava/lang/String;)V", &[e.string("android.intent.action.SEND")]);
        if let Some(t) = ty {
            let t = e.string(t);
            e.call(i, "setType", SI, &[t]).unwrap();
        }
        for (k, v) in extras {
            let k = e.string(k);
            e.call(i, "putExtra", "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;", &[k, *v]).unwrap();
        }
        i
    };
    let t = e.string("join me");
    let s = e.string("Blockheads");
    let i = send(&mut e, Some("text/plain"), &[("android.intent.extra.TEXT", t), ("android.intent.extra.SUBJECT", s)]);
    assert_eq!(resolve(&mut e, i), (true, 1));
    let title = e.string("Share");
    let r = e.scall(
        INTENT,
        "createChooser",
        "(Landroid/content/Intent;Ljava/lang/CharSequence;)Landroid/content/Intent;",
        &[Value::Ref(Some(i)), title],
    );
    let ch = e.obj(r);
    assert_eq!(resolve(&mut e, ch), (true, 1));
    start(&mut e, ch);
    let i = send(&mut e, Some("message/rfc822"), &[("android.intent.extra.TEXT", t)]);
    start(&mut e, i);
    let arr = Value::Ref(Some(e.vm.new_string_array(&["a@b".into()])));
    let i = send(&mut e, None, &[("android.intent.extra.EMAIL", arr)]);
    start(&mut e, i);
    let bad = Value::Ref(Some(e.vm.new_string("not an array")));
    let i = send(&mut e, None, &[("android.intent.extra.EMAIL", bad)]);
    start(&mut e, i);
    // A chooser without a target is just an unknown action.
    let empty = e.new_obj(INTENT, "(Ljava/lang/String;)V", &[e.string("android.intent.action.CHOOSER")]);
    start(&mut e, empty);
    assert_eq!(
        rec.take(),
        [
            "copy Blockheads\njoin me".to_string(),
            format!("notify {COPIED}"),
            "open mailto:?body=join%20me".into(),
            "open mailto:a@b".into(),
        ]
    );
    // VerdeActivity's own mail and action queries.
    let a = e.activity();
    assert_eq!(e.call(a, "canSendEmail", "()Z", &[]), Ok(Value::Int(1)));
    for (act, yes) in [("android.intent.action.VIEW", 1), ("android.intent.action.SEND", 1), ("x", 0)] {
        let v = e.string(act);
        assert_eq!(e.call(a, "hasActivityForAction", "(Ljava/lang/String;)Z", &[v]), Ok(Value::Int(yes)));
    }
    let sig = "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;[B)V";
    let st = |e: &Env, s: &str| e.string(s);
    let bytes = e.vm.new_prim_array(crate::value::Ty::Byte, 3);
    let args = [
        st(&e, ".png"),
        st(&e, "Hi"),
        st(&e, "<b>Come</b><br>now"),
        st(&e, "text/html"),
        st(&e, "x@y"),
        st(&e, "c@d"),
        Value::Ref(None),
        Value::Ref(Some(bytes)),
    ];
    e.call(a, "sendEmail", sig, &args).unwrap();
    let args = [
        Value::Ref(None),
        st(&e, "S"),
        st(&e, "a<b"),
        st(&e, "text/plain"),
        Value::Ref(None),
        Value::Ref(None),
        Value::Ref(None),
        Value::Ref(None),
    ];
    e.call(a, "sendEmail", sig, &args).unwrap();
    assert_eq!(rec.take(), ["open mailto:x@y?subject=Hi&body=Come%0Anow&cc=c@d", "open mailto:?subject=S&body=a%3Cb"]);
    assert!(e.log_text().contains("attachment is dropped"));
}

#[test]
fn clipboard() {
    let (mut e, rec) = env();
    let a = e.activity();
    let n = e.string("clipboard");
    let r = e.call(a, "getSystemService", "(Ljava/lang/String;)Ljava/lang/Object;", &[n]);
    let cb = e.obj(r);
    assert_eq!(e.call(cb, "hasPrimaryClip", "()Z", &[]), Ok(Value::Int(0)));
    assert_eq!(e.call(cb, "getPrimaryClip", "()Landroid/content/ClipData;", &[]), Ok(Value::Ref(None)));
    assert_eq!(e.call(cb, "getText", "()Ljava/lang/CharSequence;", &[]), Ok(Value::Ref(None)));
    let (l, t) = (e.string("label"), e.string("1.2.3.4"));
    let r = e.scall(
        "android/content/ClipData",
        "newPlainText",
        "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;",
        &[l, t],
    );
    let clip = e.obj(r);
    e.call(cb, "setPrimaryClip", "(Landroid/content/ClipData;)V", &[Value::Ref(Some(clip))]).unwrap();
    e.call(cb, "setPrimaryClip", "(Landroid/content/ClipData;)V", &[Value::Ref(None)]).unwrap();
    let t2 = e.string("second");
    e.call(cb, "setText", "(Ljava/lang/CharSequence;)V", &[t2]).unwrap();
    assert_eq!(rec.take(), ["copy 1.2.3.4", "copy second"]);
    // Without a host clipboard the game reads back what it copied; with one, the host's text.
    assert_eq!(e.call(cb, "hasText", "()Z", &[]), Ok(Value::Int(1)));
    assert_eq!(text(&mut e, cb, "getText", "()Ljava/lang/CharSequence;", &[]).as_deref(), Some("second"));
    *rec.clip.lock().unwrap() = Some("from host".into());
    let r = e.call(cb, "getPrimaryClip", "()Landroid/content/ClipData;", &[]);
    let clip = e.obj(r);
    assert_eq!(e.call(clip, "getItemCount", "()I", &[]), Ok(Value::Int(1)));
    let r = e.call(clip, "getItemAt", "(I)Landroid/content/ClipData$Item;", &[Value::Int(0)]);
    let item = e.obj(r);
    assert_eq!(text(&mut e, item, "getText", "()Ljava/lang/CharSequence;", &[]).as_deref(), Some("from host"));
    assert_eq!(
        text(
            &mut e,
            item,
            "coerceToText",
            "(Landroid/content/Context;)Ljava/lang/CharSequence;",
            &[Value::Ref(Some(a))]
        )
        .as_deref(),
        Some("from host")
    );
}

#[test]
fn social_share() {
    let (mut e, rec) = env();
    let r = e.scall("com/apportable/SocialShareManager", "getInstance", "()Lcom/apportable/SocialShareManager;", &[]);
    let m = e.obj(r);
    assert_eq!(e.call(m, "canSendFacebook", "()Z", &[]), Ok(Value::Int(0)));
    assert_eq!(e.call(m, "canSendTweet", "()Z", &[]), Ok(Value::Int(0)));
    e.call(m, "sendShareWithSocialNetworks", "()V", &[]).unwrap();
    let t = e.string("Play with me");
    e.call(m, "addInitialText", "(Ljava/lang/String;)V", &[t]).unwrap();
    let u = e.string("http://theblockheads.net/join.php?ip=1.2.3.4");
    e.call(m, "addURL", "(Ljava/lang/String;)V", &[u]).unwrap();
    let b = Value::Ref(Some(e.vm.new_prim_array(crate::value::Ty::Byte, 1)));
    let (x, y) = (e.string("a"), e.string("b"));
    e.call(m, "addImage", "([BLjava/lang/String;Ljava/lang/String;)V", &[b, x, y]).unwrap();
    e.call(m, "removeAllImages", "()V", &[]).unwrap();
    e.call(m, "sendShareWithFacebook", "()V", &[]).unwrap();
    e.call(m, "removeAllURLs", "()V", &[]).unwrap();
    e.call(m, "sendShareWithTwitter", "()V", &[]).unwrap();
    e.vm.with_data(m, |d| *d = Data::None);
    let u = e.string("lost");
    e.call(m, "addURL", "(Ljava/lang/String;)V", &[u]).unwrap();
    e.call(m, "sendShareWithTwitter", "()V", &[]).unwrap();
    assert_eq!(
        rec.take(),
        [
            "copy Play with me http://theblockheads.net/join.php?ip=1.2.3.4".to_string(),
            format!("notify {COPIED}"),
            "copy Play with me".into(),
            format!("notify {COPIED}"),
            "copy Play with me".into(),
            format!("notify {COPIED}"),
        ]
    );
    assert!(e.log_text().contains("nothing to share"));
    assert!(e.log_text().contains("pictures are not shared"));
}

#[test]
fn pictures_and_pages() {
    let (mut e, rec) = env();
    let k = "android/media/MediaScannerConnection";
    let a = e.activity();
    let conn = e.new_obj(
        k,
        "(Landroid/content/Context;Landroid/media/MediaScannerConnection$MediaScannerConnectionClient;)V",
        &[Value::Ref(Some(a)), Value::Ref(None)],
    );
    e.call(conn, "connect", "()V", &[]).unwrap();
    assert_eq!(e.call(conn, "isConnected", "()Z", &[]), Ok(Value::Int(1)));
    let (p, t) = (e.string("/sdcard/Pictures/The Blockheads/1.png"), e.string("image/png"));
    e.call(conn, "scanFile", "(Ljava/lang/String;Ljava/lang/String;)V", &[p, t]).unwrap();
    e.call(conn, "scanFile", "(Ljava/lang/String;Ljava/lang/String;)V", &[Value::Ref(None), Value::Ref(None)]).unwrap();
    e.call(conn, "disconnect", "()V", &[]).unwrap();
    let arr = Value::Ref(Some(e.vm.new_string_array(&["/sdcard/DCIM/2.png".into()])));
    let sig = "(Landroid/content/Context;[Ljava/lang/String;[Ljava/lang/String;Landroid/media/MediaScannerConnection$OnScanCompletedListener;)V";
    e.scall(k, "scanFile", sig, &[Value::Ref(Some(a)), arr, Value::Ref(None), Value::Ref(None)]).unwrap();
    e.scall(k, "scanFile", sig, &[Value::Ref(Some(a)), Value::Ref(None), Value::Ref(None), Value::Ref(None)]).unwrap();
    let html = "<link href=\"style.css\"><img src=\"144Icon.png\"><img src=\"missing.png\"><img src=\"bg.jpg\">";
    let h = e.string(html);
    let w = "com/noodlecake/noodlewebview/NoodleWebView";
    e.scall(w, "showBlockheadsWebView", "(Ljava/lang/String;ZJ)V", &[h, Value::Int(0), Value::Long(0)]).unwrap();
    let h = e.string("<p>welcome</p>");
    e.scall(w, "showBlockheadsWebView", "(Ljava/lang/String;ZJ)V", &[h, Value::Int(1), Value::Long(5)]).unwrap();
    assert_eq!(
        rec.take(),
        [
            "picture /sdcard/Pictures/The Blockheads/1.png".to_string(),
            "picture /sdcard/DCIM/2.png".into(),
            format!("html {} [\"style.css\", \"bg.jpg\", \"144Icon.png\"]", html.len()),
            "html 14 []".into(),
        ]
    );
    assert!(e.log_text().contains("shown read-only"));
}

#[test]
fn default_hooks_do_nothing() {
    let s = NoSystem;
    assert!(!s.open_url("http://x"));
    assert!(!s.show_html("", &[]));
    s.set_clipboard("x");
    assert_eq!(s.clipboard(), None);
    s.notify("x");
    s.picture_saved("/x");
}

#[test]
fn handle_uri_calls_the_native() {
    let (mut e, _) = env();
    let vm = e.vm.clone();
    vm.handle_uri(&mut e.cpu, "http://theblockheads.net/join.php?ip=1.2.3.4").unwrap();
}

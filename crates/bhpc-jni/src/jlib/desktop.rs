//! What the game reaches the desktop through: `android.net.Uri` and `android.content.Intent` (links: `openURL:` ->
//! `Intent(ACTION_VIEW, uri)` -> `resolveActivity` -> `startActivity`), mail (`VerdeActivity.sendEmail`), sharing
//! (`SocialShareManager`), the clipboard (`ClipboardManager`), saved photos (`MediaScannerConnection`) and the
//! help/credits page (`NoodleWebView`). The host side is [`crate::SystemHooks`].
//!
//! Only `http`, `https` and `mailto` leave the game (`market://` becomes the Play Store web page); anything else is
//! logged and dropped.

use super::{boolean, int, object, string, void};
use crate::value::{ClassId, Obj, Value};
use crate::vm::{Call, Data, JResult, Vm};

pub(crate) const ACTION_VIEW: &str = "android.intent.action.VIEW";
const ACTION_SEND: &str = "android.intent.action.SEND";
const ACTION_SEND_MULTIPLE: &str = "android.intent.action.SEND_MULTIPLE";
const ACTION_CHOOSER: &str = "android.intent.action.CHOOSER";
const EXTRA_INTENT: &str = "android.intent.extra.INTENT";
const EXTRA_TEXT: &str = "android.intent.extra.TEXT";
const EXTRA_SUBJECT: &str = "android.intent.extra.SUBJECT";
const EXTRA_EMAIL: &str = "android.intent.extra.EMAIL";
/// Package of the one "activity" that takes everything the desktop can handle (`ResolveInfo.activityInfo`).
pub(crate) const HOST_PACKAGE: &str = "pc.blockheads.desktop";
/// Notice shown when shared text went to the clipboard.
pub(crate) const COPIED: &str = "COPIED TO CLIPBOARD";

fn new_obj(vm: &Vm, class: &str) -> Obj {
    let k = vm.find_class(class).unwrap_or(ClassId(0));
    vm.alloc_object(k)
}

fn str_field(vm: &Vm, o: Obj, name: &str) -> Option<String> {
    vm.get_field(o, name).as_ref().and_then(|s| vm.string(s))
}

fn set_str(vm: &Vm, o: Obj, name: &str, v: Option<&str>) {
    vm.set_field(o, name, Value::Ref(v.map(|v| vm.new_string(v))));
}

/// The URL the desktop opens for `url`, or `None` when it must stay in the game.
pub(crate) fn desktop_url(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once(':')?;
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" | "mailto" => Some(url.to_string()),
        "market" => {
            let query = rest.split_once('?').map_or("", |q| q.1);
            Some(format!("https://play.google.com/store/apps/details?{query}"))
        }
        _ => None,
    }
}

/// Hand `url` to the desktop if it may leave the game.
fn open_link(vm: &Vm, url: &str) -> bool {
    match desktop_url(url) {
        Some(u) => vm.0.system.open_url(&u),
        None => {
            vm.log(&format!("not opening {url:?}: only http, https and mailto links leave the game"));
            false
        }
    }
}

/// `%`-encoding of everything but RFC 3986's unreserved characters (and `keep`).
pub(crate) fn pct(s: &str, keep: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) || keep.as_bytes().contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A `mailto:` URL (RFC 6068). `to`, `cc` and `bcc` are comma-separated lists.
pub(crate) fn mailto(to: &str, cc: &str, bcc: &str, subject: &str, body: &str) -> String {
    let list = |s: &str| s.split(',').map(str::trim).filter(|a| !a.is_empty()).map(|a| pct(a, "@")).collect::<Vec<_>>();
    let mut q = Vec::new();
    for (k, v) in [("subject", subject), ("body", body)] {
        if !v.is_empty() {
            q.push(format!("{k}={}", pct(v, "")));
        }
    }
    for (k, v) in [("cc", cc), ("bcc", bcc)] {
        let l = list(v);
        if !l.is_empty() {
            q.push(format!("{k}={}", l.join(",")));
        }
    }
    let mut url = format!("mailto:{}", list(to).join(","));
    if !q.is_empty() {
        url = format!("{url}?{}", q.join("&"));
    }
    url
}

/// Plain text of a little HTML (mail bodies): tags dropped, `<br>`/`</p>` as line breaks, the common entities.
pub(crate) fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find('>') else {
            rest = &rest[i..];
            break;
        };
        let tag = rest[i + 1..i + j].trim().to_ascii_lowercase();
        if tag.starts_with("br") || tag == "/p" || tag == "/div" {
            out.push('\n');
        }
        rest = &rest[i + j + 1..];
    }
    out.push_str(rest);
    for (e, c) in [("&lt;", "<"), ("&gt;", ">"), ("&quot;", "\""), ("&#39;", "'"), ("&nbsp;", " "), ("&amp;", "&")] {
        out = out.replace(e, c);
    }
    out
}

/// The relative resources an HTML page refers to (`src="x"`, `href="x"`, `url(x)`), in order, without duplicates.
pub(crate) fn relative_refs(html: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |r: &str| {
        let r = r.trim().trim_matches(['"', '\'']);
        let external = r.contains(':') || r.starts_with('/') || r.starts_with('#') || r.contains("..");
        if !r.is_empty() && !external && !out.iter().any(|o| o == r) {
            out.push(r.to_string());
        }
    };
    for key in ["src=\"", "href=\"", "src='", "href='"] {
        let end = if key.ends_with('"') { '"' } else { '\'' };
        for part in html.split(key).skip(1) {
            add(part.split(end).next().unwrap_or_default());
        }
    }
    for part in html.split("url(").skip(1) {
        add(part.split(')').next().unwrap_or_default());
    }
    out
}

/// Asset folders the game's pages come from (the help page is `GameResources/instructions/index.html`).
const PAGE_DIRS: [&str; 3] = ["GameResources/instructions/", "GameResources/", ""];

/// The page's resources from the APK's assets, plus those its style sheets refer to.
fn page_files(vm: &Vm, html: &str) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut todo = relative_refs(html);
    while let Some(name) = todo.pop() {
        if files.iter().any(|f| f.0 == name) {
            continue;
        }
        let Some(data) = PAGE_DIRS.iter().find_map(|d| vm.assets().read(&format!("{d}{name}"))) else { continue };
        if name.ends_with(".css") {
            todo.extend(relative_refs(&String::from_utf8_lossy(&data)));
        }
        files.push((name, data));
    }
    files
}

pub(crate) fn install(vm: &Vm) {
    install_uri(vm);
    install_intent(vm);
    install_resolve(vm);
    install_clipboard(vm);
    install_share(vm);
    vm.define("android/media/MediaScannerConnection")
        .method("<init>", "(Landroid/content/Context;Landroid/media/MediaScannerConnection$MediaScannerConnectionClient;)V", |_| void())
        .method("connect", "()V", |_| void())
        .method("disconnect", "()V", |_| void())
        .method("isConnected", "()Z", |_| boolean(true))
        .method("scanFile", "(Ljava/lang/String;Ljava/lang/String;)V", |c| {
            if let Some(p) = c.string(0) {
                c.vm.0.system.picture_saved(&p);
            }
            void()
        })
        .static_method(
            "scanFile",
            "(Landroid/content/Context;[Ljava/lang/String;[Ljava/lang/String;Landroid/media/MediaScannerConnection$OnScanCompletedListener;)V",
            |c| {
                let paths = match c.obj(1).and_then(|a| c.vm.data(a)) {
                    Some(Data::Objs(v)) => v,
                    _ => Vec::new(),
                };
                for p in paths.into_iter().flatten().filter_map(|p| c.vm.string(p)) {
                    c.vm.0.system.picture_saved(&p);
                }
                void()
            },
        );
    vm.define("com/noodlecake/noodlewebview/NoodleWebView").static_method(
        "showBlockheadsWebView",
        "(Ljava/lang/String;ZJ)V",
        |c| {
            let html = c.string(0).unwrap_or_default();
            if c.int(1) != 0 {
                c.vm.log("NoodleWebView: the editable page (server welcome message) is shown read-only");
            }
            let files = page_files(c.vm, &html);
            c.vm.0.system.show_html(&html, &files);
            void()
        },
    );
}

fn install_uri(vm: &Vm) {
    fn new_uri(vm: &Vm, s: &str) -> Obj {
        let u = new_obj(vm, "android/net/Uri");
        set_str(vm, u, "uriString", Some(s));
        u
    }
    fn uri_of(c: &Call) -> String {
        str_field(c.vm, c.this, "uriString").unwrap_or_default()
    }
    /// `(scheme, authority, path, query)` of a hierarchical URI.
    fn parts(u: &str) -> (Option<&str>, Option<&str>, &str, Option<&str>) {
        let (u, query) = match u.split_once('#').map_or(u, |x| x.0).split_once('?') {
            Some((a, q)) => (a, Some(q)),
            None => (u.split_once('#').map_or(u, |x| x.0), None),
        };
        let (scheme, rest) = match u.split_once(':') {
            Some((s, r)) if !s.contains('/') => (Some(s), r),
            _ => (None, u),
        };
        match rest.strip_prefix("//") {
            Some(r) => {
                let (auth, path) = r.split_at(r.find('/').unwrap_or(r.len()));
                (scheme, Some(auth), path, query)
            }
            None => (scheme, None, rest, query),
        }
    }
    let opt = |c: &Call, v: Option<&str>| match v {
        Some(v) => string(c, v),
        None => object(None),
    };
    vm.define("android/net/Uri$HierarchicalUri").extends("android/net/Uri");
    vm.define("android/net/Uri")
        .field("uriString", "Ljava/lang/String;")
        .static_method("parse", "(Ljava/lang/String;)Landroid/net/Uri;", |c| match c.string(0) {
            Some(s) => object(Some(new_uri(c.vm, &s))),
            None => Err(c.vm.throw_new("java/lang/NullPointerException", "uriString")),
        })
        .static_method("fromFile", "(Ljava/io/File;)Landroid/net/Uri;", |c| {
            let path = c.obj(0).and_then(|f| str_field(c.vm, f, "path")).unwrap_or_default();
            object(Some(new_uri(c.vm, &format!("file://{path}"))))
        })
        .static_method("encode", "(Ljava/lang/String;)Ljava/lang/String;", |c| {
            let s = c.string(0).unwrap_or_default();
            string(c, &pct(&s, ""))
        })
        .method("toString", "()Ljava/lang/String;", |c| string(c, &uri_of(c)))
        .method("getScheme", "()Ljava/lang/String;", move |c| opt(c, parts(&uri_of(c)).0))
        .method("getHost", "()Ljava/lang/String;", move |c| {
            let u = uri_of(c);
            let host = parts(&u).1.map(|a| a.rsplit('@').next().unwrap_or(a).split(':').next().unwrap_or(a));
            opt(c, host)
        })
        .method("getPath", "()Ljava/lang/String;", |c| string(c, parts(&uri_of(c)).2))
        .method("getQuery", "()Ljava/lang/String;", move |c| opt(c, parts(&uri_of(c)).3))
        .method("getQueryParameter", "(Ljava/lang/String;)Ljava/lang/String;", move |c| {
            let (u, key) = (uri_of(c), c.string(0).unwrap_or_default());
            let v = parts(&u).3.and_then(|q| {
                q.split('&').find_map(|kv| match kv.split_once('=') {
                    Some((k, v)) if k == key => Some(v),
                    None if kv == key => Some(""),
                    _ => None,
                })
            });
            opt(c, v)
        });
}

fn intent_uri(vm: &Vm, intent: Obj) -> Option<String> {
    vm.get_field(intent, "mData").as_ref().and_then(|u| str_field(vm, u, "uriString"))
}

/// The intent's extra `key`.
fn extra(vm: &Vm, intent: Obj, key: &str) -> Option<Option<Obj>> {
    let Some(Data::Map(m)) = vm.data(intent) else { return None };
    m.into_iter().find(|(k, _)| k.and_then(|k| vm.string(k)).as_deref() == Some(key)).map(|e| e.1)
}

fn extra_string(vm: &Vm, intent: Obj, key: &str) -> Option<String> {
    extra(vm, intent, key).flatten().and_then(|o| vm.string(o))
}

fn put_extra(c: &mut Call, v: Option<Obj>) -> JResult {
    let key = c.string(0).unwrap_or_default();
    let k = c.vm.new_string(&key);
    // Key positions first: `with_data` holds the VM lock, so its closure must not read strings.
    let old = match c.vm.data(c.this) {
        Some(Data::Map(m)) => m.iter().position(|(k, _)| k.and_then(|k| c.vm.string(k)).as_deref() == Some(&key)),
        _ => None,
    };
    c.vm.with_data(c.this, |d| {
        if let Data::Map(m) = d {
            if let Some(i) = old {
                m.remove(i);
            }
            m.push((Some(k), v));
        }
    });
    object(Some(c.this))
}

fn put_prim(c: &mut Call, ty: crate::value::Ty) -> JResult {
    let b = c.vm.box_value(c.args[1], ty);
    put_extra(c, b)
}

fn install_intent(vm: &Vm) {
    let set = |field: &'static str| {
        move |c: &mut Call| {
            c.vm.set_field(c.this, field, c.args[0]);
            object(Some(c.this))
        }
    };
    let get = |field: &'static str| move |c: &mut Call| object(c.vm.get_field(c.this, field).as_ref());
    let s = "Landroid/content/Intent;";
    vm.define("android/content/Intent")
        .payload(|| Data::Map(Vec::new()))
        .field("mAction", "Ljava/lang/String;")
        .field("mData", "Landroid/net/Uri;")
        .field("mType", "Ljava/lang/String;")
        .field("mPackage", "Ljava/lang/String;")
        .field("mFlags", "I")
        .method("<init>", "()V", |_| void())
        .method("<init>", "(Ljava/lang/String;)V", |c| {
            c.vm.set_field(c.this, "mAction", c.args[0]);
            void()
        })
        .method("<init>", "(Ljava/lang/String;Landroid/net/Uri;)V", |c| {
            c.vm.set_field(c.this, "mAction", c.args[0]);
            c.vm.set_field(c.this, "mData", c.args[1]);
            void()
        })
        .method("<init>", "(Landroid/content/Context;Ljava/lang/Class;)V", |_| void())
        .method("setAction", &format!("(Ljava/lang/String;){s}"), set("mAction"))
        .method("getAction", "()Ljava/lang/String;", get("mAction"))
        .method("setData", &format!("(Landroid/net/Uri;){s}"), set("mData"))
        .method("getData", "()Landroid/net/Uri;", get("mData"))
        .method("getDataString", "()Ljava/lang/String;", |c| match intent_uri(c.vm, c.this) {
            Some(u) => string(c, &u),
            None => object(None),
        })
        .method("setType", &format!("(Ljava/lang/String;){s}"), set("mType"))
        .method("getType", "()Ljava/lang/String;", get("mType"))
        .method("setPackage", &format!("(Ljava/lang/String;){s}"), set("mPackage"))
        .method("getPackage", "()Ljava/lang/String;", get("mPackage"))
        .method("addFlags", &format!("(I){s}"), |c| {
            let f = c.vm.get_field(c.this, "mFlags").as_int() | c.int(0);
            c.vm.set_field(c.this, "mFlags", Value::Int(f));
            object(Some(c.this))
        })
        .method("setFlags", &format!("(I){s}"), |c| {
            c.vm.set_field(c.this, "mFlags", c.args[0]);
            object(Some(c.this))
        })
        .method("getFlags", "()I", |c| int(c.vm.get_field(c.this, "mFlags").as_int()))
        .method("addCategory", &format!("(Ljava/lang/String;){s}"), |c| object(Some(c.this)))
        .method("putExtra", &format!("(Ljava/lang/String;Ljava/lang/String;){s}"), |c| put_extra(c, c.obj(1)))
        .method("putExtra", &format!("(Ljava/lang/String;Ljava/lang/CharSequence;){s}"), |c| put_extra(c, c.obj(1)))
        .method("putExtra", &format!("(Ljava/lang/String;[Ljava/lang/String;){s}"), |c| put_extra(c, c.obj(1)))
        .method("putExtra", &format!("(Ljava/lang/String;Landroid/os/Parcelable;){s}"), |c| put_extra(c, c.obj(1)))
        .method("putExtra", &format!("(Ljava/lang/String;Z){s}"), |c| put_prim(c, crate::value::Ty::Bool))
        .method("putExtra", &format!("(Ljava/lang/String;I){s}"), |c| put_prim(c, crate::value::Ty::Int))
        .method("putExtra", &format!("(Ljava/lang/String;J){s}"), |c| put_prim(c, crate::value::Ty::Long))
        .method("hasExtra", "(Ljava/lang/String;)Z", |c| {
            let key = c.string(0).unwrap_or_default();
            boolean(extra(c.vm, c.this, &key).is_some())
        })
        .method("getStringExtra", "(Ljava/lang/String;)Ljava/lang/String;", |c| {
            let key = c.string(0).unwrap_or_default();
            object(extra(c.vm, c.this, &key).flatten())
        })
        .static_method("createChooser", &format!("({s}Ljava/lang/CharSequence;){s}"), |c| {
            let ch = new_obj(c.vm, "android/content/Intent");
            set_str(c.vm, ch, "mAction", Some(ACTION_CHOOSER));
            let k = c.vm.new_string(EXTRA_INTENT);
            let target = c.obj(0);
            c.vm.with_data(ch, |d| *d = Data::Map(vec![(Some(k), target)]));
            object(Some(ch))
        });
    for class in ["android/content/Context", "android/app/Activity"] {
        vm.define(class).method("startActivity", "(Landroid/content/Intent;)V", |c| {
            if let Some(i) = c.obj(0) {
                start_activity(c.vm, i);
            }
            void()
        });
    }
}

/// The intent a chooser wraps (or the intent itself).
fn unwrap_chooser(vm: &Vm, intent: Obj) -> Obj {
    if str_field(vm, intent, "mAction").as_deref() == Some(ACTION_CHOOSER) {
        if let Some(Some(inner)) = extra(vm, intent, EXTRA_INTENT) {
            return inner;
        }
    }
    intent
}

/// Whether the desktop takes `intent` (what `resolveActivity` answers).
fn handled(vm: &Vm, intent: Obj) -> bool {
    let intent = unwrap_chooser(vm, intent);
    let pkg = str_field(vm, intent, "mPackage");
    if pkg.is_some_and(|p| p != HOST_PACKAGE) {
        return false;
    }
    match str_field(vm, intent, "mAction").as_deref() {
        Some(ACTION_VIEW) => intent_uri(vm, intent).and_then(|u| desktop_url(&u)).is_some(),
        Some(ACTION_SEND | ACTION_SEND_MULTIPLE) => true,
        _ => false,
    }
}

/// `startActivity`: links go to the browser, mail to the mail program, other shares to the clipboard.
pub(crate) fn start_activity(vm: &Vm, intent: Obj) -> bool {
    let intent = unwrap_chooser(vm, intent);
    match str_field(vm, intent, "mAction").as_deref() {
        Some(ACTION_VIEW) => open_link(vm, &intent_uri(vm, intent).unwrap_or_default()),
        Some(ACTION_SEND | ACTION_SEND_MULTIPLE) => {
            let text = extra_string(vm, intent, EXTRA_TEXT).unwrap_or_default();
            let subject = extra_string(vm, intent, EXTRA_SUBJECT).unwrap_or_default();
            let to = match extra(vm, intent, EXTRA_EMAIL).flatten().and_then(|a| vm.data(a)) {
                Some(Data::Objs(v)) => v.into_iter().flatten().filter_map(|s| vm.string(s)).collect::<Vec<_>>(),
                _ => Vec::new(),
            };
            let mail = str_field(vm, intent, "mType").as_deref() == Some("message/rfc822");
            let shared =
                [subject.as_str(), text.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>();
            if mail || !to.is_empty() {
                vm.0.system.open_url(&mailto(&to.join(","), "", "", &subject, &text))
            } else if shared.is_empty() {
                vm.log("startActivity: nothing to share");
                false
            } else {
                copy_text(vm, &shared.join("\n"))
            }
        }
        other => {
            vm.log(&format!("startActivity: nothing on the desktop handles {other:?}"));
            false
        }
    }
}

/// Put shared text on the clipboard and say so.
fn copy_text(vm: &Vm, text: &str) -> bool {
    remember_clip(vm, text);
    vm.0.system.set_clipboard(text);
    vm.0.system.notify(COPIED);
    true
}

fn install_resolve(vm: &Vm) {
    fn resolve_info(vm: &Vm) -> Obj {
        vm.singleton("desktop-resolve-info", |vm| {
            let a = new_obj(vm, "android/content/pm/ActivityInfo");
            set_str(vm, a, "packageName", Some(HOST_PACKAGE));
            set_str(vm, a, "name", Some("Desktop"));
            let r = new_obj(vm, "android/content/pm/ResolveInfo");
            vm.set_field(r, "activityInfo", Value::Ref(Some(a)));
            r
        })
    }
    vm.define("android/content/pm/ActivityInfo")
        .field("packageName", "Ljava/lang/String;")
        .field("name", "Ljava/lang/String;");
    vm.define("android/content/pm/ResolveInfo").field("activityInfo", "Landroid/content/pm/ActivityInfo;");
    vm.define("android/content/pm/PackageManager")
        .method("resolveActivity", "(Landroid/content/Intent;I)Landroid/content/pm/ResolveInfo;", |c| {
            let ok = c.obj(0).is_some_and(|i| handled(c.vm, i));
            object(ok.then(|| resolve_info(c.vm)))
        })
        .method("queryIntentActivities", "(Landroid/content/Intent;I)Ljava/util/List;", |c| {
            let ok = c.obj(0).is_some_and(|i| handled(c.vm, i));
            let items = if ok { vec![Some(resolve_info(c.vm))] } else { Vec::new() };
            object(Some(super::util::new_list(c.vm, "java/util/ArrayList", items)))
        });
}

/// The text the game copied last (what the clipboard reads back when the host cannot).
fn remember_clip(vm: &Vm, text: &str) {
    set_str(vm, clipboard_service(vm), "mLast", Some(text));
}

fn clip_text(vm: &Vm) -> Option<String> {
    let cb = clipboard_service(vm);
    vm.0.system.clipboard().or_else(|| str_field(vm, cb, "mLast"))
}

/// The `clipboard` system service (the object `getSystemService("clipboard")` returns).
fn clipboard_service(vm: &Vm) -> Obj {
    vm.singleton("service:clipboard", |vm| new_obj(vm, "android/content/ClipboardManager"))
}

fn new_clip(vm: &Vm, text: &str) -> Obj {
    let d = new_obj(vm, "android/content/ClipData");
    set_str(vm, d, "mText", Some(text));
    d
}

fn install_clipboard(vm: &Vm) {
    let set_text = |c: &mut Call, t: Option<String>| {
        if let Some(t) = t {
            remember_clip(c.vm, &t);
            c.vm.0.system.set_clipboard(&t);
        }
        void()
    };
    vm.define("android/content/ClipboardManager")
        .field("mLast", "Ljava/lang/String;")
        .method("setPrimaryClip", "(Landroid/content/ClipData;)V", move |c| {
            let t = c.obj(0).and_then(|d| str_field(c.vm, d, "mText"));
            set_text(c, t)
        })
        .method("setText", "(Ljava/lang/CharSequence;)V", move |c| {
            let t = c.string(0);
            set_text(c, t)
        })
        .method("getPrimaryClip", "()Landroid/content/ClipData;", |c| {
            object(clip_text(c.vm).map(|t| new_clip(c.vm, &t)))
        })
        .method("getText", "()Ljava/lang/CharSequence;", |c| match clip_text(c.vm) {
            Some(t) => string(c, &t),
            None => object(None),
        })
        .method("hasPrimaryClip", "()Z", |c| boolean(clip_text(c.vm).is_some()))
        .method("hasText", "()Z", |c| boolean(clip_text(c.vm).is_some()));
    vm.define("android/content/ClipData")
        .field("mText", "Ljava/lang/String;")
        .static_method(
            "newPlainText",
            "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;",
            |c| {
                let t = c.string(1).unwrap_or_default();
                object(Some(new_clip(c.vm, &t)))
            },
        )
        .method("getItemCount", "()I", |_| int(1))
        .method("getItemAt", "(I)Landroid/content/ClipData$Item;", |c| {
            let it = new_obj(c.vm, "android/content/ClipData$Item");
            c.vm.set_field(it, "mText", c.vm.get_field(c.this, "mText"));
            object(Some(it))
        });
    vm.define("android/content/ClipData$Item")
        .field("mText", "Ljava/lang/CharSequence;")
        .method("getText", "()Ljava/lang/CharSequence;", |c| object(c.vm.get_field(c.this, "mText").as_ref()))
        .method("coerceToText", "(Landroid/content/Context;)Ljava/lang/CharSequence;", |c| {
            object(c.vm.get_field(c.this, "mText").as_ref())
        });
}

/// `com.apportable.SocialShareManager`: the game collects text, links and pictures, then asks to share them. The
/// desktop has no share sheet: the text and links go to the clipboard (with a notice); pictures are dropped.
fn install_share(vm: &Vm) {
    const SSM: &str = "com/apportable/SocialShareManager";
    fn texts(vm: &Vm, o: Obj) -> Vec<String> {
        match vm.data(o) {
            Some(Data::List(l)) => l.into_iter().flatten().filter_map(|s| vm.string(s)).collect(),
            _ => Vec::new(),
        }
    }
    let send = |c: &mut Call| {
        let mut parts: Vec<String> = str_field(c.vm, c.this, "initialText").into_iter().collect();
        parts.extend(texts(c.vm, c.this));
        let text = parts.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            c.vm.log("SocialShareManager: nothing to share");
        } else {
            copy_text(c.vm, &text);
        }
        c.vm.with_data(c.this, |d| *d = Data::List(Vec::new()));
        void()
    };
    vm.define(SSM)
        .payload(|| Data::List(Vec::new()))
        .field("initialText", "Ljava/lang/String;")
        .static_method("getInstance", "()Lcom/apportable/SocialShareManager;", |c| {
            object(Some(c.vm.singleton("social-share", |vm| new_obj(vm, SSM))))
        })
        .method("addInitialText", "(Ljava/lang/String;)V", |c| {
            c.vm.set_field(c.this, "initialText", c.args[0]);
            void()
        })
        .method("addURL", "(Ljava/lang/String;)V", |c| {
            let u = c.obj(0);
            c.vm.with_data(c.this, |d| {
                if let Data::List(l) = d {
                    l.push(u);
                }
            });
            void()
        })
        .method("addImage", "([BLjava/lang/String;Ljava/lang/String;)V", |c| {
            c.vm.log("SocialShareManager: pictures are not shared on the desktop");
            void()
        })
        .method("removeAllImages", "()V", |_| void())
        .method("removeAllURLs", "()V", |c| {
            c.vm.with_data(c.this, |d| *d = Data::List(Vec::new()));
            void()
        })
        .method("canSendFacebook", "()Z", |_| boolean(false))
        .method("canSendTweet", "()Z", |_| boolean(false))
        .method("sendShareWithSocialNetworks", "()V", send)
        .method("sendShareWithFacebook", "()V", send)
        .method("sendShareWithTwitter", "()V", send);
}

/// `VerdeActivity.sendEmail(attachmentSuffix, subject, body, mimeType, to, cc, bcc, attachment)`: a `mailto:` link
/// (attachments cannot travel in one and are dropped).
pub(crate) fn send_email(c: &mut Call) -> JResult {
    let a = |i: usize| c.string(i).unwrap_or_default();
    let (subject, mut body, mime, to, cc, bcc) = (a(1), a(2), a(3), a(4), a(5), a(6));
    if mime == "text/html" {
        body = html_to_text(&body);
    }
    if c.obj(7).is_some() {
        c.vm.log("sendEmail: the attachment is dropped (a mailto: link cannot carry it)");
    }
    c.vm.0.system.open_url(&mailto(&to, &cc, &bcc, &subject, &body));
    void()
}

/// `VerdeActivity.hasActivityForAction`.
pub(crate) fn has_activity_for(action: &str) -> bool {
    matches!(action, ACTION_VIEW | ACTION_SEND | ACTION_SEND_MULTIPLE)
}

//! `startActivityForResult` and `MediaStore.Images.Media.getBitmap`: the "Your pic" picture picker (`Uri` and
//! `Intent` themselves are in `desktop`).
//!
//! The game's `UIImagePickerController` starts `Intent(ACTION_PICK, content://media/internal/images/media)` with
//! request code 9001 and waits for `VerdeActivity.nativeOnActivityResult(9001, resultCode, data)`. On `RESULT_OK` it
//! asks `MediaStore.Images.Media.getBitmap(resolver, data.getData())` for the picture and copies its pixels. Here the
//! request is kept for the host ([`Vm::take_activity_requests`]), which shows a file dialog and answers with
//! [`Vm::activity_result`] (a `file://` URI of the chosen picture, or `RESULT_CANCELED`); `getBitmap` decodes the PNG
//! or JPEG file into a bitmap.

use super::{object, void};
use crate::value::{ClassId, Obj, Value};
use crate::vm::{Call, Throw, Vm};
use bhpc_cpu::Cpu;

/// `Activity.RESULT_OK`.
pub const RESULT_OK: i32 = -1;
/// `Activity.RESULT_CANCELED`.
pub const RESULT_CANCELED: i32 = 0;

const URI: &str = "android/net/Uri";
const INTENT: &str = "android/content/Intent";

/// An activity the game asked to start for a result (`Activity.startActivityForResult`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityRequest {
    pub code: i32,
    pub action: String,
    /// The intent's data URI.
    pub data: Option<String>,
    /// The intent's MIME type.
    pub mime: Option<String>,
}

impl ActivityRequest {
    /// The request picks a picture (`ACTION_PICK`/`ACTION_GET_CONTENT` on images).
    pub fn wants_image(&self) -> bool {
        let pick = matches!(self.action.as_str(), "android.intent.action.PICK" | "android.intent.action.GET_CONTENT");
        let images = self.data.as_deref().is_some_and(|d| d.contains("/images"))
            || self.mime.as_deref().is_some_and(|m| m.starts_with("image/"));
        pick && images
    }
}

/// `file://` URI of a host path (spaces and other unsafe bytes percent-encoded).
pub fn file_uri(path: &str) -> String {
    let mut out = String::from("file://");
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The host path a `file://` URI (or a plain absolute path) names; `None` for other schemes.
pub fn uri_path(uri: &str) -> Option<String> {
    let rest = match uri.strip_prefix("file://") {
        Some(r) => r,
        None if uri.starts_with('/') => uri,
        None => return None,
    };
    let b = rest.as_bytes();
    let (mut out, mut i) = (Vec::new(), 0);
    while i < b.len() {
        let hex = |j: usize| b.get(j).and_then(|c| (*c as char).to_digit(16));
        match (b[i], hex(i + 1), hex(i + 2)) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (c, _, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

fn new_uri(vm: &Vm, s: &str) -> Obj {
    let u = vm.alloc_object(vm.find_class(URI).unwrap_or(ClassId(0)));
    let text = vm.new_string(s);
    vm.set_field(u, "uriString", Value::Ref(Some(text)));
    u
}

fn uri_text(vm: &Vm, u: Option<Obj>) -> Option<String> {
    vm.get_field(u?, "uriString").as_ref().and_then(|s| vm.string(s))
}

fn field_text(vm: &Vm, o: Obj, name: &str) -> Option<String> {
    vm.get_field(o, name).as_ref().and_then(|s| vm.string(s))
}

impl Vm {
    /// Activities the game started for a result since the last call (oldest first).
    pub fn take_activity_requests(&self) -> Vec<ActivityRequest> {
        std::mem::take(&mut self.st().activity_requests)
    }

    /// Answer the activity started with request `code`: `nativeOnActivityResult(code, result, intent)`, the intent
    /// carrying `data` as its URI (no intent without one). Run it on the Android UI thread.
    pub fn activity_result(&self, cpu: &mut Cpu, code: i32, result: i32, data: Option<&str>) -> Result<(), Throw> {
        let intent = data.map(|d| {
            let i = self.alloc_object(self.find_class(INTENT).unwrap_or(ClassId(0)));
            self.set_field(i, "mData", Value::Ref(Some(new_uri(self, d))));
            i
        });
        let args = [Value::Int(code), Value::Int(result), Value::Ref(intent)];
        let sig = "(IILandroid/content/Intent;)V";
        self.call_static(cpu, "com/apportable/activity/VerdeActivity", "nativeOnActivityResult", sig, &args).map(drop)
    }
}

pub(crate) fn install(vm: &Vm) {
    vm.define("android/app/Activity").method("startActivityForResult", "(Landroid/content/Intent;I)V", |c| {
        let Some(i) = c.obj(0) else { return void() };
        let req = ActivityRequest {
            code: c.int(1),
            action: field_text(c.vm, i, "mAction").unwrap_or_default(),
            data: uri_text(c.vm, c.vm.get_field(i, "mData").as_ref()),
            mime: field_text(c.vm, i, "mType"),
        };
        c.vm.log(&format!("startActivityForResult {req:?}"));
        c.vm.st().activity_requests.push(req);
        void()
    });
    let get_bitmap = |c: &mut Call| {
        let path = uri_text(c.vm, c.obj(1)).and_then(|u| uri_path(&u));
        let bmp = path.as_deref().and_then(|p| super::graphics::bitmap_from_file(c.vm, p));
        if bmp.is_none() {
            // Android throws FileNotFoundException; the native caller copes better with a null picture.
            c.vm.log(&format!("getBitmap: no picture at {path:?}"));
        }
        object(bmp)
    };
    vm.define("android/provider/MediaStore$Images$Media").static_method(
        "getBitmap",
        "(Landroid/content/ContentResolver;Landroid/net/Uri;)Landroid/graphics/Bitmap;",
        get_bitmap,
    );
}

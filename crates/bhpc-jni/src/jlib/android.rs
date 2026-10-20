//! `android.*`: the parts of the framework the Apportable runtime touches at start-up.

use super::io::{new_file, new_stream};
use super::{boolean, int, long, object, string, void};
use crate::looper;
use crate::value::{ClassId, Obj, Value};
use crate::vm::{Call, Data, JResult, Vm};

fn new_obj(vm: &Vm, class: &str) -> Obj {
    let k = vm.find_class(class).unwrap_or(ClassId(0));
    vm.alloc_object(k)
}

fn str_val(vm: &Vm, s: &str) -> Value {
    Value::Ref(Some(vm.new_string(s)))
}

pub(crate) fn install(vm: &Vm) {
    install_os(vm);
    install_content(vm);
    install_res(vm);
    install_view(vm);
    install_misc(vm);
}

fn install_os(vm: &Vm) {
    let p = vm.platform().clone();
    let s = |v: &str| str_val(vm, v);
    vm.define("android/os/Build")
        .static_field("MODEL", "Ljava/lang/String;", s(&p.model))
        .static_field("MANUFACTURER", "Ljava/lang/String;", s(&p.manufacturer))
        .static_field("BRAND", "Ljava/lang/String;", s(&p.manufacturer))
        .static_field("DEVICE", "Ljava/lang/String;", s(&p.model))
        .static_field("PRODUCT", "Ljava/lang/String;", s(&p.model))
        .static_field("HARDWARE", "Ljava/lang/String;", s("pc"))
        .static_field("BOARD", "Ljava/lang/String;", s("pc"))
        .static_field("ID", "Ljava/lang/String;", s("BHPC"))
        .static_field("DISPLAY", "Ljava/lang/String;", s("BHPC"))
        .static_field("FINGERPRINT", "Ljava/lang/String;", s("bhpc/pc/pc:4.4.4/BHPC/0:user/release-keys"))
        .static_field("TYPE", "Ljava/lang/String;", s("user"))
        .static_field("TAGS", "Ljava/lang/String;", s("release-keys"))
        .static_field("HOST", "Ljava/lang/String;", s("bhpc"))
        .static_field("USER", "Ljava/lang/String;", s("bhpc"))
        .static_field("SERIAL", "Ljava/lang/String;", s("0123456789"))
        .static_field("CPU_ABI", "Ljava/lang/String;", s("armeabi-v7a"))
        .static_field("CPU_ABI2", "Ljava/lang/String;", s("armeabi"));
    vm.define("android/os/Build$VERSION")
        .static_field("SDK_INT", "I", Value::Int(p.sdk_int))
        .static_field("SDK", "Ljava/lang/String;", s(&p.sdk_int.to_string()))
        .static_field("RELEASE", "Ljava/lang/String;", s(&p.release))
        .static_field("INCREMENTAL", "Ljava/lang/String;", s("0"))
        .static_field("CODENAME", "Ljava/lang/String;", s("REL"));
    vm.define("android/os/Process")
        .static_method("myPid", "()I", |_| int(std::process::id() as i32))
        .static_method("myTid", "()I", |c| int(c.cpu.processor_id() as i32 + 1))
        .static_method("myUid", "()I", |_| int(10000))
        .static_method("killProcess", "(I)V", |c| {
            c.cpu.request_stop(bhpc_cpu::Stop::Host("Process.killProcess".into()));
            void()
        })
        .static_method("setThreadPriority", "(I)V", |_| void());
    vm.define("android/os/SystemClock")
        .static_method("uptimeMillis", "()J", |c| long(c.vm.now_ms() as i64))
        .static_method("elapsedRealtime", "()J", |c| long(c.vm.now_ms() as i64))
        .static_method("elapsedRealtimeNanos", "()J", |c| long(c.vm.now_ms() as i64 * 1_000_000))
        .static_method("currentThreadTimeMillis", "()J", |c| long(c.vm.now_ms() as i64))
        .static_method("sleep", "(J)V", |c| {
            std::thread::sleep(std::time::Duration::from_millis(c.long(0).max(0) as u64));
            void()
        });
    vm.define("android/os/Environment")
        .static_field("DIRECTORY_DOWNLOADS", "Ljava/lang/String;", s("Download"))
        .static_field("DIRECTORY_DOCUMENTS", "Ljava/lang/String;", s("Documents"))
        .static_field("DIRECTORY_PICTURES", "Ljava/lang/String;", s("Pictures"))
        .static_field("DIRECTORY_MUSIC", "Ljava/lang/String;", s("Music"))
        .static_field("MEDIA_MOUNTED", "Ljava/lang/String;", s("mounted"))
        .static_method("getExternalStorageDirectory", "()Ljava/io/File;", |c| {
            object(Some(new_file(c.vm, &c.vm.platform().external_dir)))
        })
        .static_method("getExternalStoragePublicDirectory", "(Ljava/lang/String;)Ljava/io/File;", |c| {
            let p = format!("{}/{}", c.vm.platform().external_dir, c.string(0).unwrap_or_default());
            object(Some(new_file(c.vm, &p)))
        })
        .static_method("getExternalStorageState", "()Ljava/lang/String;", |c| string(c, "mounted"))
        .static_method("getDataDirectory", "()Ljava/io/File;", |c| object(Some(new_file(c.vm, "/data"))))
        .static_method("getRootDirectory", "()Ljava/io/File;", |c| object(Some(new_file(c.vm, "/system"))));
    vm.define("android/os/Bundle")
        .payload(|| Data::Map(Vec::new()))
        .method("<init>", "()V", |_| void())
        .method("containsKey", "(Ljava/lang/String;)Z", |c| boolean(bundle_get(c).is_some()))
        .method("size", "()I", |c| {
            int(c.vm.with_data(c.this, |d| if let Data::Map(m) = d { m.len() } else { 0 }).unwrap_or(0) as i32)
        })
        .method("putString", "(Ljava/lang/String;Ljava/lang/String;)V", bundle_put)
        .method("putInt", "(Ljava/lang/String;I)V", |c| bundle_put_prim(c, crate::value::Ty::Int))
        .method("putBoolean", "(Ljava/lang/String;Z)V", |c| bundle_put_prim(c, crate::value::Ty::Bool))
        .method("getString", "(Ljava/lang/String;)Ljava/lang/String;", |c| object(bundle_get(c).flatten()))
        .method("getInt", "(Ljava/lang/String;)I", |c| bundle_int(c, 0))
        .method("getInt", "(Ljava/lang/String;I)I", |c| bundle_int(c, c.int(1)))
        .method("getBoolean", "(Ljava/lang/String;)Z", |c| bundle_int(c, 0))
        .method("getBoolean", "(Ljava/lang/String;Z)Z", |c| bundle_int(c, c.int(1)));
}

/// The stored value for the bundle key argument 0: `Some(value)` if the key exists.
fn bundle_get(c: &Call) -> Option<Option<Obj>> {
    let key = c.string(0)?;
    let Some(Data::Map(entries)) = c.vm.data(c.this) else { return None };
    entries.into_iter().find(|(k, _)| k.and_then(|k| c.vm.string(k)).as_deref() == Some(key.as_str())).map(|e| e.1)
}

/// Keys of a map-backed object as strings, in order.
fn map_keys(vm: &Vm, o: Obj) -> Vec<Option<String>> {
    match vm.data(o) {
        Some(Data::Map(m)) => m.iter().map(|(k, _)| k.and_then(|k| vm.string(k))).collect(),
        _ => Vec::new(),
    }
}

fn bundle_store(c: &Call, v: Option<Obj>) {
    let key = c.string(0).unwrap_or_default();
    let k = c.vm.new_string(&key);
    let existing = map_keys(c.vm, c.this).iter().position(|e| e.as_deref() == Some(key.as_str()));
    c.vm.with_data(c.this, |d| {
        if let Data::Map(m) = d {
            if let Some(i) = existing {
                m.remove(i);
            }
            m.push((Some(k), v));
        }
    });
}

fn bundle_put(c: &mut Call) -> JResult {
    bundle_store(c, c.obj(1));
    void()
}

fn bundle_put_prim(c: &mut Call, ty: crate::value::Ty) -> JResult {
    let b = c.vm.box_value(c.args[1], ty);
    bundle_store(c, b);
    void()
}

fn bundle_int(c: &mut Call, default: i32) -> JResult {
    match bundle_get(c).flatten().and_then(|o| c.vm.unbox(o)) {
        Some(v) => int(v.as_int()),
        None => int(default),
    }
}

fn install_content(vm: &Vm) {
    vm.define("android/content/Context")
        .method("getApplicationContext", "()Landroid/content/Context;", |c| object(Some(application(c.vm))))
        .method("getPackageName", "()Ljava/lang/String;", |c| string(c, &c.vm.platform().package_name.clone()))
        .method("getPackageCodePath", "()Ljava/lang/String;", |c| string(c, &c.vm.platform().apk_path.clone()))
        .method("getPackageResourcePath", "()Ljava/lang/String;", |c| string(c, &c.vm.platform().apk_path.clone()))
        .method("getFilesDir", "()Ljava/io/File;", |c| object(Some(new_file(c.vm, &c.vm.platform().files_dir))))
        .method("getCacheDir", "()Ljava/io/File;", |c| object(Some(new_file(c.vm, &c.vm.platform().cache_dir))))
        .method("getObbDir", "()Ljava/io/File;", |c| object(Some(new_file(c.vm, &c.vm.platform().obb_dir))))
        .method("getExternalCacheDir", "()Ljava/io/File;", |c| {
            object(Some(new_file(c.vm, &format!("{}/cache", c.vm.platform().external_dir))))
        })
        .method("getExternalFilesDir", "(Ljava/lang/String;)Ljava/io/File;", |c| {
            let mut p = format!("{}/files", c.vm.platform().external_dir);
            if let Some(t) = c.string(0) {
                p = format!("{p}/{t}");
            }
            object(Some(new_file(c.vm, &p)))
        })
        .method("getFileStreamPath", "(Ljava/lang/String;)Ljava/io/File;", |c| {
            let p = format!("{}/{}", c.vm.platform().files_dir, c.string(0).unwrap_or_default());
            object(Some(new_file(c.vm, &p)))
        })
        .method("getDatabasePath", "(Ljava/lang/String;)Ljava/io/File;", |c| {
            let p = format!("{}/databases/{}", c.vm.platform().files_dir, c.string(0).unwrap_or_default());
            object(Some(new_file(c.vm, &p)))
        })
        .method("getDir", "(Ljava/lang/String;I)Ljava/io/File;", |c| {
            let p = format!("{}/app_{}", c.vm.platform().files_dir, c.string(0).unwrap_or_default());
            let _ = std::fs::create_dir_all(&p);
            object(Some(new_file(c.vm, &p)))
        })
        .method("getAssets", "()Landroid/content/res/AssetManager;", |c| object(Some(assets(c.vm))))
        .method("getResources", "()Landroid/content/res/Resources;", |c| {
            object(Some(c.vm.singleton("resources", |vm| new_obj(vm, "android/content/res/Resources"))))
        })
        .method("getContentResolver", "()Landroid/content/ContentResolver;", |c| {
            object(Some(c.vm.singleton("resolver", |vm| new_obj(vm, "android/content/ContentResolver"))))
        })
        .method("getPackageManager", "()Landroid/content/pm/PackageManager;", |c| {
            object(Some(c.vm.singleton("pm", |vm| new_obj(vm, "android/content/pm/PackageManager"))))
        })
        .method("getMainLooper", "()Landroid/os/Looper;", |c| object(Some(looper::main_looper(c.vm))))
        .method("getClassLoader", "()Ljava/lang/ClassLoader;", |c| object(Some(super::lang::class_loader(c.vm))))
        .method("getApplicationInfo", "()Landroid/content/pm/ApplicationInfo;", |c| {
            let a = c.vm.singleton("appinfo", |vm| {
                let a = new_obj(vm, "android/content/pm/ApplicationInfo");
                let p = vm.platform();
                for (k, v) in [
                    ("dataDir", &p.files_dir),
                    ("sourceDir", &p.apk_path),
                    ("publicSourceDir", &p.apk_path),
                    ("nativeLibraryDir", &p.files_dir),
                    ("packageName", &p.package_name),
                ] {
                    vm.set_field(a, k, str_val(vm, v));
                }
                a
            });
            object(Some(a))
        })
        .method("getSystemService", "(Ljava/lang/String;)Ljava/lang/Object;", |c| {
            let name = c.string(0).unwrap_or_default();
            let class = match name.as_str() {
                "window" => "android/view/WindowManager",
                "audio" => "android/media/AudioManager",
                "connectivity" => "android/net/ConnectivityManager",
                "input_method" => "android/view/inputmethod/InputMethodManager",
                "phone" => "android/telephony/TelephonyManager",
                "sensor" => "android/hardware/SensorManager",
                "vibrator" => "android/os/Vibrator",
                "clipboard" => "android/content/ClipboardManager",
                "notification" => "android/app/NotificationManager",
                "power" => "android/os/PowerManager",
                "wifi" => "android/net/wifi/WifiManager",
                "location" => "android/location/LocationManager",
                "layout_inflater" => "android/view/LayoutInflater",
                "activity" => "android/app/ActivityManager",
                "alarm" => "android/app/AlarmManager",
                _ => return object(None),
            };
            object(Some(c.vm.singleton(&format!("service:{name}"), |vm| new_obj(vm, class))))
        })
        .method("getSharedPreferences", "(Ljava/lang/String;I)Landroid/content/SharedPreferences;", |c| {
            let name = c.string(0).unwrap_or_default();
            object(Some(
                c.vm.singleton(&format!("prefs:{name}"), |vm| new_obj(vm, "android/app/SharedPreferencesImpl")),
            ))
        });
    vm.define("android/content/ContextWrapper").extends("android/content/Context");
    vm.define("android/app/Application").extends("android/content/ContextWrapper");
    vm.define("android/app/Activity")
        .extends("android/content/ContextWrapper")
        .method("getApplication", "()Landroid/app/Application;", |c| object(Some(application(c.vm))))
        .method("getWindowManager", "()Landroid/view/WindowManager;", |c| {
            object(Some(c.vm.singleton("service:window", |vm| new_obj(vm, "android/view/WindowManager"))))
        })
        .method("runOnUiThread", "(Ljava/lang/Runnable;)V", |c| {
            if let Some(r) = c.obj(0) {
                looper::post(c.vm, looper::main_looper(c.vm), r, 0);
            }
            void()
        })
        .method("isFinishing", "()Z", |_| boolean(false))
        .method("finish", "()V", |_| void());

    vm.define("android/content/SharedPreferences").interface();
    vm.define("android/content/SharedPreferences$Editor").interface();
    vm.define("android/app/SharedPreferencesImpl")
        .implements(&["android/content/SharedPreferences"])
        .payload(|| Data::Map(Vec::new()))
        .method("contains", "(Ljava/lang/String;)Z", |c| boolean(bundle_get(c).is_some()))
        .method("getString", "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;", |c| match bundle_get(c) {
            Some(v) => object(v),
            None => object(c.obj(1)),
        })
        .method("getInt", "(Ljava/lang/String;I)I", |c| bundle_int(c, c.int(1)))
        .method("getBoolean", "(Ljava/lang/String;Z)Z", |c| bundle_int(c, c.int(1)))
        .method("getLong", "(Ljava/lang/String;J)J", |c| match bundle_get(c).flatten().and_then(|o| c.vm.unbox(o)) {
            Some(v) => long(v.as_long()),
            None => long(c.long(1)),
        })
        .method("getFloat", "(Ljava/lang/String;F)F", |c| match bundle_get(c).flatten().and_then(|o| c.vm.unbox(o)) {
            Some(v) => Ok(Value::Float(v.as_float())),
            None => Ok(Value::Float(c.float(1))),
        })
        .method("getAll", "()Ljava/util/Map;", |c| {
            let entries = match c.vm.data(c.this) {
                Some(Data::Map(m)) => m,
                _ => Vec::new(),
            };
            object(Some(c.vm.new_data_object("java/util/HashMap", Data::Map(entries))))
        })
        .method("edit", "()Landroid/content/SharedPreferences$Editor;", |c| {
            let e = new_obj(c.vm, "android/app/SharedPreferencesImpl$EditorImpl");
            c.vm.set_field(e, "prefs", Value::Ref(Some(c.this)));
            object(Some(e))
        });
    let editor = vm
        .define("android/app/SharedPreferencesImpl$EditorImpl")
        .implements(&["android/content/SharedPreferences$Editor"])
        .payload(|| Data::Map(Vec::new()))
        .field("prefs", "Landroid/content/SharedPreferences;")
        .field("clear", "Z");
    editor
        .method("putString", "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/SharedPreferences$Editor;", |c| {
            bundle_store(c, c.obj(1));
            object(Some(c.this))
        })
        .method("putInt", "(Ljava/lang/String;I)Landroid/content/SharedPreferences$Editor;", |c| {
            put_boxed(c, crate::value::Ty::Int)
        })
        .method("putLong", "(Ljava/lang/String;J)Landroid/content/SharedPreferences$Editor;", |c| {
            put_boxed(c, crate::value::Ty::Long)
        })
        .method("putFloat", "(Ljava/lang/String;F)Landroid/content/SharedPreferences$Editor;", |c| {
            put_boxed(c, crate::value::Ty::Float)
        })
        .method("putBoolean", "(Ljava/lang/String;Z)Landroid/content/SharedPreferences$Editor;", |c| {
            put_boxed(c, crate::value::Ty::Bool)
        })
        .method("remove", "(Ljava/lang/String;)Landroid/content/SharedPreferences$Editor;", |c| {
            bundle_store(c, None);
            object(Some(c.this))
        })
        .method("clear", "()Landroid/content/SharedPreferences$Editor;", |c| {
            c.vm.set_field(c.this, "clear", Value::Int(1));
            object(Some(c.this))
        })
        .method("commit", "()Z", commit_editor)
        .method("apply", "()V", |c| commit_editor(c).map(|_| Value::Void));
}

fn put_boxed(c: &mut Call, ty: crate::value::Ty) -> JResult {
    let b = c.vm.box_value(c.args[1], ty);
    bundle_store(c, b);
    object(Some(c.this))
}

/// Apply the editor's staged changes to its preferences object.
pub(crate) fn commit_editor(c: &mut Call) -> JResult {
    let prefs = c.vm.get_field(c.this, "prefs").as_ref();
    let clear = c.vm.get_field(c.this, "clear").as_int() != 0;
    let staged = match c.vm.data(c.this) {
        Some(Data::Map(m)) => m,
        _ => Vec::new(),
    };
    c.vm.with_data(c.this, |d| *d = Data::Map(Vec::new()));
    c.vm.set_field(c.this, "clear", Value::Int(0));
    let Some(prefs) = prefs else { return boolean(false) };
    let vm = c.vm.clone();
    let mut keys = map_keys(&vm, prefs);
    let mut entries = match vm.data(prefs) {
        Some(Data::Map(m)) => m,
        _ => return boolean(true),
    };
    if clear {
        entries.clear();
        keys.clear();
    }
    for (k, v) in staged {
        let name = k.and_then(|k| vm.string(k));
        if let Some(i) = keys.iter().position(|e| *e == name) {
            entries.remove(i);
            keys.remove(i);
        }
        if v.is_some() {
            entries.push((k, v));
            keys.push(name);
        }
    }
    vm.with_data(prefs, |d| *d = Data::Map(entries));
    boolean(true)
}

/// The single `AssetManager`.
fn assets(vm: &Vm) -> Obj {
    vm.singleton("assets", |vm| new_obj(vm, "android/content/res/AssetManager"))
}

/// The single `Application`/`Context` object.
pub(crate) fn application(vm: &Vm) -> Obj {
    vm.singleton("application", |vm| new_obj(vm, "com/apportable/app/VerdeApplication"))
}

fn install_res(vm: &Vm) {
    vm.define("android/content/res/Resources")
        .method("getDisplayMetrics", "()Landroid/util/DisplayMetrics;", |c| object(Some(display_metrics(c.vm))))
        .method("getConfiguration", "()Landroid/content/res/Configuration;", |c| {
            let cfg = new_obj(c.vm, "android/content/res/Configuration");
            let p = c.vm.platform().clone();
            let (sw, sh) = c.vm.screen_size();
            let d = p.density();
            c.vm.set_field(cfg, "orientation", Value::Int(if sw > sh { 2 } else { 1 }));
            c.vm.set_field(cfg, "screenWidthDp", Value::Int((sw as f32 / d) as i32));
            c.vm.set_field(cfg, "screenHeightDp", Value::Int((sh as f32 / d) as i32));
            c.vm.set_field(cfg, "densityDpi", Value::Int(p.density_dpi));
            c.vm.set_field(cfg, "fontScale", Value::Float(1.0));
            c.vm.set_field(cfg, "touchscreen", Value::Int(3));
            c.vm.set_field(cfg, "keyboard", Value::Int(1));
            c.vm.set_field(cfg, "screenLayout", Value::Int(0x22));
            object(Some(cfg))
        })
        .method("getAssets", "()Landroid/content/res/AssetManager;", |c| object(Some(assets(c.vm))))
        .method("getIdentifier", "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I", |_| int(0));
    vm.define("android/content/res/Configuration")
        .field("orientation", "I")
        .field("screenWidthDp", "I")
        .field("screenHeightDp", "I")
        .field("densityDpi", "I")
        .field("fontScale", "F")
        .field("touchscreen", "I")
        .field("keyboard", "I")
        .field("screenLayout", "I");
    vm.define("android/content/res/AssetManager")
        .method("open", "(Ljava/lang/String;)Ljava/io/InputStream;", asset_open)
        .method("open", "(Ljava/lang/String;I)Ljava/io/InputStream;", asset_open)
        .method("list", "(Ljava/lang/String;)[Ljava/lang/String;", |c| {
            let dir = c.string(0).unwrap_or_default();
            match c.vm.assets().list(dir.trim_matches('/')) {
                Some(names) => object(Some(c.vm.new_string_array(&names))),
                None => object(None),
            }
        })
        .method("close", "()V", |_| void());
}

fn asset_open(c: &mut Call) -> JResult {
    let path = c.string(0).unwrap_or_default();
    match c.vm.assets().read(path.trim_start_matches('/')) {
        Some(data) => object(Some(new_stream(c.vm, data))),
        None => Err(c.vm.throw_new("java/io/FileNotFoundException", &path)),
    }
}

/// A fresh `DisplayMetrics` describing the PC screen.
fn display_metrics(vm: &Vm) -> Obj {
    let m = new_obj(vm, "android/util/DisplayMetrics");
    fill_metrics(vm, m);
    m
}

fn fill_metrics(vm: &Vm, m: Obj) {
    let p = vm.platform();
    let d = p.density();
    let (sw, sh) = vm.screen_size();
    vm.set_field(m, "widthPixels", Value::Int(sw));
    vm.set_field(m, "heightPixels", Value::Int(sh));
    vm.set_field(m, "density", Value::Float(d));
    vm.set_field(m, "scaledDensity", Value::Float(d));
    vm.set_field(m, "densityDpi", Value::Int(p.density_dpi));
    vm.set_field(m, "xdpi", Value::Float(p.density_dpi as f32));
    vm.set_field(m, "ydpi", Value::Float(p.density_dpi as f32));
}

fn install_view(vm: &Vm) {
    vm.define("android/util/DisplayMetrics")
        .field("widthPixels", "I")
        .field("heightPixels", "I")
        .field("density", "F")
        .field("scaledDensity", "F")
        .field("densityDpi", "I")
        .field("xdpi", "F")
        .field("ydpi", "F")
        .method("<init>", "()V", |_| void());
    vm.define("android/graphics/Point").field("x", "I").field("y", "I").method("<init>", "()V", |_| void());
    vm.define("android/view/Display")
        .method("getWidth", "()I", |c| int(c.vm.screen_size().0))
        .method("getHeight", "()I", |c| int(c.vm.screen_size().1))
        .method("getRotation", "()I", |_| int(0))
        .method("getOrientation", "()I", |_| int(0))
        .method("getDisplayId", "()I", |_| int(0))
        .method("getRefreshRate", "()F", |c| Ok(Value::Float(c.vm.platform().refresh_rate)))
        .method("getMetrics", "(Landroid/util/DisplayMetrics;)V", metrics_into)
        .method("getRealMetrics", "(Landroid/util/DisplayMetrics;)V", metrics_into)
        .method("getSize", "(Landroid/graphics/Point;)V", size_into)
        .method("getRealSize", "(Landroid/graphics/Point;)V", size_into);
    vm.define("android/view/WindowManager").method("getDefaultDisplay", "()Landroid/view/Display;", |c| {
        object(Some(c.vm.singleton("display", |vm| new_obj(vm, "android/view/Display"))))
    });
}

fn metrics_into(c: &mut Call) -> JResult {
    match c.obj(0) {
        Some(m) => {
            fill_metrics(c.vm, m);
            void()
        }
        None => Err(c.vm.throw_new("java/lang/NullPointerException", "metrics")),
    }
}

fn size_into(c: &mut Call) -> JResult {
    match c.obj(0) {
        Some(p) => {
            let (w, h) = c.vm.screen_size();
            c.vm.set_field(p, "x", Value::Int(w));
            c.vm.set_field(p, "y", Value::Int(h));
            void()
        }
        None => Err(c.vm.throw_new("java/lang/NullPointerException", "point")),
    }
}

fn install_misc(vm: &Vm) {
    vm.define("android/util/Log")
        .static_method("v", "(Ljava/lang/String;Ljava/lang/String;)I", |c| log(c, 'V'))
        .static_method("d", "(Ljava/lang/String;Ljava/lang/String;)I", |c| log(c, 'D'))
        .static_method("i", "(Ljava/lang/String;Ljava/lang/String;)I", |c| log(c, 'I'))
        .static_method("w", "(Ljava/lang/String;Ljava/lang/String;)I", |c| log(c, 'W'))
        .static_method("e", "(Ljava/lang/String;Ljava/lang/String;)I", |c| log(c, 'E'));
    vm.define("android/provider/Settings$Secure").static_method(
        "getString",
        "(Landroid/content/ContentResolver;Ljava/lang/String;)Ljava/lang/String;",
        |c| match c.string(1).as_deref() {
            Some("android_id") => string(c, &c.vm.platform().android_id.clone()),
            _ => object(None),
        },
    );
    vm.define("android/content/pm/PackageManager")
        .method("hasSystemFeature", "(Ljava/lang/String;)Z", |c| boolean(has_feature(&c.string(0).unwrap_or_default())))
        .method("getPackageInfo", "(Ljava/lang/String;I)Landroid/content/pm/PackageInfo;", |c| {
            let p = c.vm.platform().clone();
            let i = new_obj(c.vm, "android/content/pm/PackageInfo");
            c.vm.set_field(i, "packageName", str_val(c.vm, &p.package_name));
            c.vm.set_field(i, "versionName", str_val(c.vm, &p.version_name));
            c.vm.set_field(i, "versionCode", Value::Int(p.version_code));
            object(Some(i))
        });
    vm.define("android/content/pm/PackageInfo")
        .field("packageName", "Ljava/lang/String;")
        .field("versionName", "Ljava/lang/String;")
        .field("versionCode", "I");
    install_audio_track(vm);
    install_audio_player(vm);
    vm.define("android/media/AudioManager")
        .static_field("STREAM_MUSIC", "I", Value::Int(3))
        .method("getStreamVolume", "(I)I", |_| int(8))
        .method("getStreamMaxVolume", "(I)I", |_| int(15))
        .method("requestAudioFocus", "(Landroid/media/AudioManager$OnAudioFocusChangeListener;II)I", |_| int(1))
        .method("abandonAudioFocus", "(Landroid/media/AudioManager$OnAudioFocusChangeListener;)I", |_| int(1))
        .method("getProperty", "(Ljava/lang/String;)Ljava/lang/String;", |c| match c.string(0).as_deref() {
            Some("android.media.property.OUTPUT_SAMPLE_RATE") => string(c, "44100"),
            Some("android.media.property.OUTPUT_FRAMES_PER_BUFFER") => string(c, "256"),
            _ => object(None),
        });
    vm.define("android/net/ConnectivityManager").method("getActiveNetworkInfo", "()Landroid/net/NetworkInfo;", |c| {
        object(Some(c.vm.singleton("netinfo", |vm| new_obj(vm, "android/net/NetworkInfo"))))
    });
    vm.define("android/net/NetworkInfo")
        .method("isConnected", "()Z", |_| boolean(true))
        .method("isAvailable", "()Z", |_| boolean(true))
        .method("isConnectedOrConnecting", "()Z", |_| boolean(true))
        .method("getType", "()I", |_| int(1))
        .method("getTypeName", "()Ljava/lang/String;", |c| string(c, "WIFI"));
}

/// `PackageManager.hasSystemFeature` for a desktop acting as a touch phone.
fn has_feature(name: &str) -> bool {
    matches!(
        name,
        "android.hardware.touchscreen"
            | "android.hardware.touchscreen.multitouch"
            | "android.hardware.touchscreen.multitouch.distinct"
            | "android.hardware.screen.portrait"
            | "android.hardware.wifi"
            | "android.hardware.audio.output"
    )
}

pub(crate) fn feature_flag(name: &str) -> i32 {
    has_feature(name) as i32
}

fn log(c: &mut Call, level: char) -> JResult {
    let (tag, msg) = (c.string(0).unwrap_or_default(), c.string(1).unwrap_or_default());
    c.vm.log(&format!("{level}/{tag}: {msg}"));
    int(0)
}

/// `com.apportable.media.AudioPlayer`, Apportable's `AVAudioPlayer` back end (one Android `MediaPlayer` per id): the
/// game's music. The player id is always the last argument; the host side is [`crate::AudioHooks::music`].
fn install_audio_player(vm: &Vm) {
    use crate::platform::MusicOp;
    fn run(c: &mut Call, id: i32, op: MusicOp) -> i32 {
        if id <= 0 {
            return -1;
        }
        c.vm.0.audio.music(id as u32, op)
    }
    fn cmd(op: MusicOp) -> impl Fn(&mut Call) -> JResult + Send + Sync + 'static {
        move |c| {
            let id = c.int(0);
            run(c, id, op);
            void()
        }
    }
    fn query(op: MusicOp) -> impl Fn(&mut Call) -> JResult + Send + Sync + 'static {
        move |c| {
            let id = c.int(0);
            int(run(c, id, op))
        }
    }
    vm.define("com/apportable/media/AudioPlayer")
        .method("<init>", "(Landroid/content/Context;)V", |_| void())
        .method("initAudio", "(Ljava/lang/String;)I", |c| {
            let path = c.string(0).unwrap_or_default();
            // Like `AssetManager.openFd`, a relative path is an asset of the APK.
            let asset = if path.starts_with('/') { None } else { c.vm.assets().read(&path) };
            let id = c.vm.0.audio.music_open(&path, asset);
            c.vm.log(&format!("AudioPlayer.initAudio({path:?}) -> {id:?}"));
            int(id.map_or(-1, |id| id as i32))
        })
        .method("prepareAudio", "(I)V", cmd(MusicOp::Prepare))
        .method("startAudio", "(I)V", cmd(MusicOp::Start))
        .method("pauseAudio", "(I)V", cmd(MusicOp::Pause))
        .method("releaseAudio", "(I)V", cmd(MusicOp::Release))
        .method("getDuration", "(I)I", query(MusicOp::Duration))
        .method("getCurrentPosition", "(I)I", query(MusicOp::Position))
        .method("isAudioPlaying", "(I)I", query(MusicOp::IsPlaying))
        .method("seekTo", "(II)V", |c| {
            let (ms, id) = (c.int(0), c.int(1));
            run(c, id, MusicOp::SeekTo(ms));
            void()
        })
        .method("setNumberOfLoops", "(II)V", |c| {
            let (loops, id) = (c.int(0), c.int(1));
            run(c, id, MusicOp::Loops(loops));
            void()
        })
        .method("setVolume", "(FFI)V", |c| {
            let (l, r, id) = (c.float(0), c.float(1), c.int(2));
            run(c, id, MusicOp::Volume(l, r));
            void()
        })
        .static_method("lifecycleSuspend", "()V", |c| {
            c.vm.0.audio.music_suspend(true);
            void()
        })
        .static_method("lifecycleResume", "()V", |c| {
            c.vm.0.audio.music_suspend(false);
            void()
        });
}

/// `android.media.AudioTrack` (stream mode) over [`crate::AudioHooks`].
fn install_audio_track(vm: &Vm) {
    use crate::platform::TrackOp;
    let op = |op: TrackOp| {
        move |c: &mut Call| {
            let id = c.vm.get_field(c.this, "id").as_int() as u32;
            let r = c.vm.0.audio.control(id, op);
            if matches!(op, TrackOp::Release) {
                c.vm.set_field(c.this, "id", Value::Int(0));
            }
            int(r)
        }
    };
    vm.define("android/media/AudioTrack")
        .field("id", "I")
        .static_method("getMinBufferSize", "(III)I", |c| {
            int(c.vm.0.audio.min_buffer_size(c.int(0), c.int(1), c.int(2)))
        })
        .static_method("getNativeOutputSampleRate", "(I)I", |_| int(44100))
        .method("<init>", "(IIIIII)V", |c| {
            let (rate, ch, enc, buf) = (c.int(1), c.int(2), c.int(3), c.int(4));
            let id = c.vm.0.audio.create(rate, ch, enc, buf);
            c.vm.set_field(c.this, "id", Value::Int(id.unwrap_or(0) as i32));
            void()
        })
        .method("getState", "()I", |c| int(i32::from(c.vm.get_field(c.this, "id").as_int() != 0)))
        .method("play", "()V", {
            let f = op(TrackOp::Play);
            move |c| f(c).map(|_| Value::Void)
        })
        .method("pause", "()V", {
            let f = op(TrackOp::Pause);
            move |c| f(c).map(|_| Value::Void)
        })
        .method("stop", "()V", {
            let f = op(TrackOp::Stop);
            move |c| f(c).map(|_| Value::Void)
        })
        .method("flush", "()V", {
            let f = op(TrackOp::Flush);
            move |c| f(c).map(|_| Value::Void)
        })
        .method("release", "()V", {
            let f = op(TrackOp::Release);
            move |c| f(c).map(|_| Value::Void)
        })
        .method("setStereoVolume", "(FF)I", |c| {
            let id = c.vm.get_field(c.this, "id").as_int() as u32;
            int(c.vm.0.audio.control(id, TrackOp::Volume(c.float(0).max(c.float(1)))))
        })
        .method("setVolume", "(F)I", |c| {
            let id = c.vm.get_field(c.this, "id").as_int() as u32;
            int(c.vm.0.audio.control(id, TrackOp::Volume(c.float(0))))
        })
        .method("getPlaybackHeadPosition", "()I", |c| {
            let id = c.vm.get_field(c.this, "id").as_int() as u32;
            int(c.vm.0.audio.head_position(id))
        })
        .method("write", "([BII)I", |c| {
            let id = c.vm.get_field(c.this, "id").as_int() as u32;
            let (off, size) = (c.int(1).max(0) as usize, c.int(2).max(0) as usize);
            let bytes = match c.obj(0).and_then(|a| c.vm.data(a)) {
                Some(crate::vm::Data::Prim(_, b)) => b,
                _ => return int(-2),
            };
            let end = (off + size).min(bytes.len());
            let data = &bytes[off.min(end)..end];
            int(c.vm.0.audio.write(c.cpu, id, data))
        });
}

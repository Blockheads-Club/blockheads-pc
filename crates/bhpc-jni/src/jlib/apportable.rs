//! `com.apportable.*`: the Java shell around the native game (activity, main thread, lifecycle).
//!
//! The classes here carry the *host* behaviour the native side calls (`VerdeActivity` screen
//! size, EGL callbacks, `MainThread` tick actions). The `native` methods of `Lifecycle`,
//! `VerdeActivity`, `MainThread`, `LibraryLoader` ... are declared so that `RegisterNatives` and
//! `Java_*` exports bind to them; the host calls them with [`crate::Vm::call_static`] et al.

use super::android::{commit_editor, feature_flag};
use super::{boolean, int, object, string, void};
use crate::looper;
use crate::value::{ClassId, Obj, Value};
use crate::vm::{Call, JResult, Vm};

/// The single `VerdeActivity` instance.
pub(crate) fn activity(vm: &Vm) -> Obj {
    vm.singleton("activity", |vm| {
        let k = vm.find_class("com/apportable/activity/VerdeActivity").unwrap_or(ClassId(0));
        vm.alloc_object(k)
    })
}

/// The Java `GLSurfaceView` the game's start-up natives are called on.
pub(crate) fn surface_view(vm: &Vm) -> Obj {
    vm.singleton("gl-surface-view", |vm| {
        let k = vm.find_class("com/apportable/gl/GLSurfaceView").unwrap_or(ClassId(0));
        vm.alloc_object(k)
    })
}

fn new_context(vm: &Vm, id: u32) -> Obj {
    let k = vm.find_class("javax/microedition/khronos/egl/EGLContext").unwrap_or(ClassId(0));
    let c = vm.alloc_object(k);
    vm.set_field(c, "id", Value::Int(id as i32));
    c
}

fn context_id(c: &Call, i: usize) -> u32 {
    c.obj(i).map_or(0, |o| c.vm.get_field(o, "id").as_int() as u32)
}

pub(crate) fn install(vm: &Vm) {
    vm.define("com/apportable/app/VerdeApplication").extends("android/app/Application");
    vm.define("android/widget/FrameLayout");
    vm.define("javax/microedition/khronos/egl/EGLContext").field("id", "I");
    let v = vm
        .define("com/apportable/activity/VerdeActivity")
        .extends("android/app/Activity")
        .static_method("androidId", "()Ljava/lang/String;", |c| string(c, &c.vm.platform().android_id.clone()))
        .static_method("getActivity", "()Lcom/apportable/activity/VerdeActivity;", |c| object(Some(activity(c.vm))))
        .static_method("terminateProcess", "()V", |c| {
            c.cpu.request_stop(bhpc_cpu::Stop::Host("VerdeActivity.terminateProcess".into()));
            void()
        })
        .method("canSendEmail", "()Z", |_| boolean(false))
        .method("getContentWidth", "()I", |c| int(c.vm.platform().screen_width))
        .method("getContentHeight", "()I", |c| int(c.vm.platform().screen_height))
        .method("getScreenWidth", "()I", |c| int(c.vm.platform().screen_width))
        .method("getScreenHeight", "()I", |c| int(c.vm.platform().screen_height))
        .method("getSplashScreenWidth", "()I", |c| int(c.vm.platform().screen_width))
        .method("getSplashScreenHeight", "()I", |c| int(c.vm.platform().screen_height))
        .method("hasActivityForAction", "(Ljava/lang/String;)Z", |_| boolean(false))
        .method("hasSystemFeature", "(Ljava/lang/String;)I", |c| int(feature_flag(&c.string(0).unwrap_or_default())))
        .method("hasWindowFocus", "()Z", |_| boolean(true))
        .method("getAttributionId", "()Ljava/lang/String;", |_| object(None))
        .method("uuid", "()Ljava/lang/String;", |c| {
            let id = c.vm.platform().android_id.clone();
            string(c, &format!("00000000-0000-4000-8000-{:0>12}", &id[..id.len().min(12)]))
        })
        .method("gc", "()V", |_| void())
        .method("homeScreen", "()V", |_| void())
        .method("_getRequestedOrientation", "()I", |_| int(1))
        .method("_setRequestedOrientation", "(I)V", |_| void())
        .method("primaryContainer", "()Landroid/widget/FrameLayout;", |c| {
            object(Some(c.vm.singleton("container", |vm| {
                let k = vm.find_class("android/widget/FrameLayout").unwrap_or(ClassId(0));
                vm.alloc_object(k)
            })))
        })
        .method("getPrimaryContext", "()Ljavax/microedition/khronos/egl/EGLContext;", |c| {
            object(Some(c.vm.singleton("egl-primary", |vm| new_context(vm, 1))))
        })
        .method("createSharedContext", "(ILjavax/microedition/khronos/egl/EGLContext;)Ljavax/microedition/khronos/egl/EGLContext;", |c| {
            let id = c.vm.0.gfx.create_shared_context(c.int(0));
            object(Some(new_context(c.vm, id)))
        })
        .method("makeCurrentContext", "(Ljavax/microedition/khronos/egl/EGLContext;)Z", |c| {
            boolean(c.vm.0.gfx.make_current(context_id(c, 0)))
        })
        .method("destroyContext", "(Ljavax/microedition/khronos/egl/EGLContext;)Z", |c| {
            boolean(c.vm.0.gfx.destroy_context(context_id(c, 0)))
        })
        .method("swapBuffers", "()Z", |c| boolean(c.vm.0.gfx.swap_buffers()))
        .method("setContentSize", "(FF)V", |c| {
            c.vm.0.gfx.set_content_size(c.float(0), c.float(1));
            void()
        })
        .method("rektEGLContext", "()V", |c| {
            c.vm.0.gfx.context_lost();
            void()
        })
        .method("queueEvent", "(Ljava/lang/Runnable;)V", post_main)
        .method("runOnThread", "(Ljava/lang/Runnable;I)V", post_main)
        .method("savePreferenceEditorHack", "(Landroid/content/SharedPreferences$Editor;)V", |c| match c.obj(0) {
            Some(e) => {
                c.this = e;
                commit_editor(c).map(|_| Value::Void)
            }
            None => void(),
        })
        .method("sendEmail", "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;[B)V", |_| void());
    v.natives(&ACTIVITY_NATIVES);

    vm.define("com/apportable/MainThread")
        .static_method("getHandler", "()Landroid/os/Handler;", |c| {
            let h = c.vm.singleton("main-handler", |vm| {
                let k = vm.find_class("android/os/Handler").unwrap_or(ClassId(0));
                let h = vm.alloc_object(k);
                vm.set_field(h, "looper", Value::Ref(Some(looper::main_looper(vm))));
                h
            });
            object(Some(h))
        })
        .static_method("getThread", "()Ljava/lang/Thread;", |c| {
            let main = looper::main_looper(c.vm);
            let pid = c.vm.st().loopers.iter().find(|(_, l)| **l == main).map_or(0, |(p, _)| *p);
            object(Some(super::thread_object(c.vm, pid)))
        })
        .static_method("isStarted", "()Z", |_| boolean(true))
        .static_method("start", "()V", |_| void())
        .static_method("goNative", "()V", |_| void())
        .static_method("addTickAction", "(Ljava/lang/Runnable;)V", |c| {
            if let Some(r) = c.obj(0) {
                c.vm.st().ticks.push(r);
            }
            void()
        })
        .static_method("removeTickAction", "(Ljava/lang/Runnable;)V", |c| {
            let r = c.obj(0);
            c.vm.st().ticks.retain(|t| Some(*t) != r);
            void()
        })
        .static_method("tick", "()V", |c| {
            c.vm.run_tick_actions(c.cpu)?;
            void()
        })
        .native("nativeRun", "()V", true);
    vm.define("com/apportable/utils/RunLoop")
        .static_method("isUnifiedThreadModel", "()Z", |_| boolean(true))
        .static_method("prepare", "()V", |c| {
            looper::attach_main(c.vm, c.cpu.processor_id());
            void()
        })
        .static_method("processMessages", "()V", |c| {
            let l = looper::main_looper(c.vm);
            let now = c.vm.now_ms();
            c.vm.pump(c.cpu, l, now)?;
            void()
        })
        .native("nativeNext", "(Landroid/os/MessageQueue;)Landroid/os/Message;", true);
    vm.define("com/apportable/Lifecycle")
        .static_method("isCreated", "()Z", |_| boolean(true))
        .static_method("isInBackground", "()Z", |_| boolean(false))
        .static_method("isNativeApplicationStarted", "()Z", |_| boolean(true))
        .natives(&LIFECYCLE_NATIVES);
    vm.define("com/apportable/LibraryLoader").native("debugWait", "()V", true).native("init", "()V", true).native(
        "setenv",
        "(Ljava/lang/String;Ljava/lang/String;)V",
        true,
    );
    vm.define("com/apportable/gl/GLSurfaceView").native("nativeInit", "()V", false).native(
        "nativeOnSurfaceChanged",
        "(II)V",
        false,
    );
    vm.define("com/apportable/Library").native("resolveLinkages", "(Ljava/lang/String;)[Ljava/lang/String;", false);
    vm.define("com/apportable/ui/Device")
        .static_method("isAutorotationOn", "()Z", |_| boolean(false))
        .static_method("macAddress", "()Ljava/lang/String;", |c| string(c, "02:00:00:00:00:00"))
        .static_method("create", "(Landroid/content/Context;I)Lcom/apportable/ui/Device;", |c| {
            object(Some(c.vm.alloc_object(c.class)))
        })
        .method("enable", "()V", |_| void())
        .method("disable", "()V", |_| void())
        .native("nativeOnOrientationChanged", "(II)V", false);
    install_widgets(vm);
}

/// The UIKit widgets of `com.apportable.ui`: `create(Context, handle[, RectF])` makes a plain object, every
/// other member is a reported stub (the game only keeps the objects around and sets properties).
pub(crate) const WIDGETS: [&str; 20] = [
    "View",
    "Window",
    "ActivityIndicatorView",
    "AlertView",
    "BarButtonItem",
    "BarItem",
    "Button",
    "ImageView",
    "Label",
    "MediaPlayer",
    "NavigationBar",
    "PickerView",
    "ProgressView",
    "ScrollView",
    "Slider",
    "Switch",
    "TextField",
    "TextView",
    "Toolbar",
    "WebView",
];

fn install_widgets(vm: &Vm) {
    // Views mark themselves dirty when the game changes their geometry or hierarchy; the host's layout pass
    // (`Vm::run_layout`) then calls the native `layoutSubviews`, as Android's measure/layout pass would.
    let dirty = |c: &mut Call| {
        let o = c.this;
        c.vm.mark_layout(o);
        void()
    };
    let view = vm
        .define("com/apportable/ui/View")
        .field("mObject", "I")
        .natives(&[
            ("callDrawRect", "(I)V", false),
            ("callLayoutSubviews", "(I)V", false),
            ("getDescription", "(I)Ljava/lang/String;", false),
            ("isDrawRectImplemented", "(I)Z", false),
            ("nativeSetFrame", "(IIII)V", false),
        ])
        .method("setFrame", "(Landroid/graphics/RectF;)V", dirty)
        .method("setBounds", "(Landroid/graphics/RectF;)V", dirty)
        .method("setNeedsLayout", "()V", dirty)
        .method("layoutIfNeeded", "()V", dirty)
        .method("addSubview", "(Lcom/apportable/ui/View;)V", |c| {
            for o in [Some(c.this), c.obj(0)].into_iter().flatten() {
                c.vm.mark_layout(o);
            }
            void()
        });
    drop(view);
    vm.define("com/apportable/ui/Window").method("makeKeyAndOrderFront", "()V", |c| {
        let o = c.this;
        c.vm.mark_layout(o);
        c.vm.st().focus_pending = true;
        void()
    });
    for w in WIDGETS {
        let class = format!("com/apportable/ui/{w}");
        let owner = if w == "MediaPlayer" { "Landroid/app/Activity;" } else { "Landroid/content/Context;" };
        let mut b = vm.define(&class);
        if w != "View" {
            b = b.extends("com/apportable/ui/View");
        }
        if w == "AlertView" {
            b = b
                .method("setTitle", "(Ljava/lang/String;)V", |c| {
                    c.vm.log(&format!("alert title: {:?}", c.string(0)));
                    void()
                })
                .method("setMessage", "(Ljava/lang/String;)V", |c| {
                    c.vm.log(&format!("alert message: {:?}", c.string(0)));
                    void()
                });
        }
        let make = |c: &mut Call| {
            let o = c.vm.alloc_object(c.class);
            c.vm.set_field(o, "mObject", Value::Int(c.int(1)));
            c.vm.mark_layout(o);
            object(Some(o))
        };
        b.static_method("create", &format!("({owner}I)L{class};"), make).static_method(
            "create",
            &format!("({owner}ILandroid/graphics/RectF;)L{class};"),
            make,
        );
    }
}

/// `queueEvent`/`runOnThread`: run on the main looper.
fn post_main(c: &mut Call) -> JResult {
    if let Some(r) = c.obj(0) {
        looper::post(c.vm, looper::main_looper(c.vm), r, 0);
    }
    void()
}

const LIFECYCLE_NATIVES: [(&str, &str, bool); 8] = [
    ("applicationDidBecomeActive", "()V", true),
    ("applicationDidEnterBackground", "()V", true),
    ("applicationDidReceiveMemoryWarning", "()V", true),
    ("applicationWillEnterForeground", "()V", true),
    ("applicationWillResignActive", "()V", true),
    ("applicationWillTerminate", "()V", true),
    ("contextDidBecomeInvalid", "()V", true),
    ("contextDidBecomeValid", "()V", true),
];

const ACTIVITY_NATIVES: [(&str, &str, bool); 21] = [
    ("nativeHandleUri", "(Ljava/lang/String;)V", true),
    ("nativeOnActivityResult", "(IILandroid/content/Intent;)V", true),
    ("nativeOnConfigurationChanged", "(Landroid/content/res/Configuration;)V", true),
    ("nativeOnCreate", "()V", true),
    ("nativeOnDestroy", "()V", true),
    ("nativeOnLowMemory", "()V", true),
    ("nativeOnNewIntent", "(Landroid/content/Intent;)V", true),
    ("nativeOnPause", "()V", true),
    ("nativeOnRestart", "()V", true),
    ("nativeOnResume", "()V", true),
    ("nativeOnSoftKeyboardChanged", "(II)V", true),
    ("nativeOnStart", "()V", true),
    ("nativeOnStop", "()V", true),
    ("nativeRootGeometryChanged", "()V", false),
    ("nativeSetEnv", "(Ljava/lang/String;Ljava/lang/String;)V", true),
    ("gpgNativeOnActivityCreated", "(Landroid/app/Activity;Landroid/os/Bundle;)V", true),
    ("gpgNativeOnActivityDestroyed", "(Landroid/app/Activity;)V", true),
    ("gpgNativeOnActivityPaused", "(Landroid/app/Activity;)V", true),
    ("gpgNativeOnActivityResult", "(Landroid/app/Activity;IILandroid/content/Intent;)V", true),
    ("gpgNativeOnActivityResumed", "(Landroid/app/Activity;)V", true),
    ("gpgNativeOnActivityStarted", "(Landroid/app/Activity;)V", true),
];

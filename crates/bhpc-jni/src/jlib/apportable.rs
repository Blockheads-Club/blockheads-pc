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
        .method("getContentWidth", "()I", |c| int(c.vm.content_pixels().0))
        .method("getContentHeight", "()I", |c| int(c.vm.content_pixels().1))
        .method("getScreenWidth", "()I", |c| int(c.vm.screen_size().0))
        .method("getScreenHeight", "()I", |c| int(c.vm.screen_size().1))
        .method("getSplashScreenWidth", "()I", |c| int(c.vm.screen_size().0))
        .method("getSplashScreenHeight", "()I", |c| int(c.vm.screen_size().1))
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
        .method("_getRequestedOrientation", "()I", |_| int(0))
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
            let (w, h) = (c.float(0), c.float(1));
            c.vm.set_content_size(w, h);
            c.vm.0.gfx.set_content_size(w, h);
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
        .method("setFrame", "(Landroid/graphics/RectF;)V", |c| {
            let (o, f) = (c.this, c.vm.rect_of(c.obj(0)));
            c.vm.ui_update(o, |v| v.frame = f);
            c.vm.mark_layout(o);
            void()
        })
        .method("setBounds", "(Landroid/graphics/RectF;)V", |c| {
            let (o, f) = (c.this, c.vm.rect_of(c.obj(0)));
            c.vm.ui_update(o, |v| v.bounds = [f[0], f[1]]);
            c.vm.mark_layout(o);
            void()
        })
        .method("setHidden", "(Z)V", |c| {
            let (o, h) = (c.this, c.int(0) != 0);
            c.vm.ui_update(o, |v| v.hidden = h);
            void()
        })
        .method("_setAlpha", "(F)V", |c| {
            let (o, a) = (c.this, c.float(0));
            c.vm.ui_update(o, |v| v.alpha = a);
            void()
        })
        .method("_setBackgroundColor", "(I)V", |c| {
            let (o, col) = (c.this, c.int(0) as u32);
            c.vm.ui_update(o, |v| v.bg = Some(col));
            void()
        })
        .method("setClipsToBounds", "(Z)V", |c| {
            let (o, on) = (c.this, c.int(0) != 0);
            c.vm.ui_update(o, |v| v.clips = on);
            void()
        })
        .method("setUserInteractionEnabled", "(Z)V", |c| {
            let (o, i) = (c.this, c.int(0) != 0);
            c.vm.ui_update(o, |v| v.interactive = i);
            void()
        })
        .method("insertSubview", "(Lcom/apportable/ui/View;I)V", |c| {
            if let Some(child) = c.obj(0) {
                c.vm.ui_attach(c.this, child, Some(c.int(1).max(0) as usize));
                c.vm.mark_layout(c.this);
            }
            void()
        })
        .method("removeSubview", "(Lcom/apportable/ui/View;)V", |c| {
            if let Some(child) = c.obj(0) {
                c.vm.ui_detach(child);
                c.vm.mark_layout(c.this);
            }
            void()
        })
        .method("setNeedsLayout", "()V", dirty)
        .method("layoutIfNeeded", "()V", dirty)
        .method("addSubview", "(Lcom/apportable/ui/View;)V", |c| {
            for o in [Some(c.this), c.obj(0)].into_iter().flatten() {
                c.vm.mark_layout(o);
            }
            if let Some(child) = c.obj(0) {
                c.vm.ui_attach(c.this, child, None);
            }
            // A text field added to an alert is the alert's input line.
            if let Some(f) = c.obj(0).filter(|f| c.vm.instance_of(*f, "com/apportable/ui/TextField")) {
                let alert = c.this;
                c.vm.with_alert(alert, |a| a.field = Some(f));
            }
            void()
        });
    drop(view);
    install_rect(vm);
    vm.define("com/apportable/ui/Window").method("makeKeyAndOrderFront", "()V", |c| {
        let o = c.this;
        c.vm.mark_layout(o);
        c.vm.st().focus_pending = true;
        void()
    });
    // The controls' events go to the native side through `Control.event(handle, type)`.
    vm.define("com/apportable/ui/Control").extends("com/apportable/ui/View").native("event", "(II)V", false);
    for w in WIDGETS {
        let class = format!("com/apportable/ui/{w}");
        let owner = if w == "MediaPlayer" { "Landroid/app/Activity;" } else { "Landroid/content/Context;" };
        let mut b = vm.define(&class);
        if w != "View" {
            let base = if matches!(w, "Button" | "TextField" | "Slider" | "Switch") { "Control" } else { "View" };
            b = b.extends(&format!("com/apportable/ui/{base}"));
        }
        if w == "AlertView" {
            b = b
                .method("setTitle", "(Ljava/lang/String;)V", |c| {
                    let t = c.string(0).unwrap_or_default();
                    c.vm.log(&format!("alert title: {t:?}"));
                    c.vm.with_alert(c.this, |a| a.title = t);
                    void()
                })
                .method("setMessage", "(Ljava/lang/String;)V", |c| {
                    let m = c.string(0).unwrap_or_default();
                    c.vm.log(&format!("alert message: {m:?}"));
                    c.vm.with_alert(c.this, |a| a.message = m);
                    void()
                })
                .method("addButtonWithTitle", "(Ljava/lang/String;I)V", |c| {
                    let (t, i) = (c.string(0).unwrap_or_default(), c.int(1));
                    c.vm.log(&format!("alert button {i}: {t:?}"));
                    c.vm.with_alert(c.this, |a| a.buttons.push((t, i)));
                    void()
                })
                .method("setCancelButtonTitle", "(Ljava/lang/String;)V", |c| {
                    let t = c.string(0).unwrap_or_default();
                    c.vm.with_alert(c.this, |a| a.cancel = Some((t, a.cancel.as_ref().map_or(0, |x| x.1))));
                    void()
                })
                .method("setCancelButtonIndex", "(I)V", |c| {
                    let i = c.int(0);
                    c.vm.with_alert(c.this, |a| a.cancel = Some((a.cancel.take().map(|x| x.0).unwrap_or_default(), i)));
                    void()
                })
                .method("show", "()V", |c| {
                    let a = c.vm.with_alert(c.this, |a| a.clone());
                    c.vm.log(&format!("alert shown: {} / {} buttons", a.title, a.buttons.len()));
                    if let Some(f) = a.field {
                        c.vm.set_text_focus(Some(f));
                        c.vm.open_dialog(a.clone());
                    }
                    c.vm.st().shown_alerts.push(a);
                    void()
                })
                .method("dismiss", "()V", |_| void())
                .natives(&[
                    ("nativeOnCancel", "(I)V", false),
                    ("nativeOnClick", "(II)V", false),
                    ("nativeOnDismiss", "(I)V", false),
                ]);
        }
        if w == "TextField" || w == "TextView" {
            b = b
                .field("mText", "Ljava/lang/String;")
                .method("setText", "(Ljava/lang/String;)V", |c| {
                    let t = c.string(0).unwrap_or_default();
                    c.vm.set_text_of(c.this, &t);
                    let o = c.this;
                    c.vm.ui_update(o, |v| v.text = t);
                    void()
                })
                .method("getText", "()Ljava/lang/String;", |c| {
                    let t = c.vm.text_of(c.this);
                    string(c, &t)
                })
                .method("becomeFirstResponder", "()V", |c| {
                    c.vm.set_text_focus(Some(c.this));
                    void()
                })
                .method("resignFirstResponder", "()V", |c| {
                    if c.vm.text_focus() == Some(c.this) {
                        c.vm.set_text_focus(None);
                    }
                    void()
                });
        }
        if matches!(w, "Label" | "Button" | "TextField" | "TextView") {
            b = b
                .method("setFont", "(Landroid/graphics/Typeface;F)V", |c| {
                    let (o, f, size) = (c.this, c.obj(0), c.float(1));
                    c.vm.ui_update(o, |v| (v.style.font, v.style.size) = (f, size));
                    void()
                })
                .method("setTextColor", "(I)V", |c| {
                    let (o, col) = (c.this, c.int(0) as u32);
                    c.vm.ui_update(o, |v| v.style.color = col);
                    void()
                })
                .method("setTextAlignment", "(I)V", |c| {
                    let (o, a) = (c.this, c.int(0));
                    c.vm.ui_update(o, |v| v.style.align = a);
                    void()
                })
                .method("setNumberOfLines", "(I)V", |c| {
                    let (o, n) = (c.this, c.int(0));
                    c.vm.ui_update(o, |v| v.style.lines = n);
                    void()
                })
                .method("setMinimumFontSize", "(F)V", |c| {
                    let (o, s) = (c.this, c.float(0));
                    c.vm.ui_update(o, |v| v.style.min_size = s);
                    void()
                })
                .method("setShadowColor", "(I)V", |c| {
                    let (o, col) = (c.this, c.int(0) as u32);
                    c.vm.ui_update(o, |v| v.style.shadow_color = col);
                    void()
                })
                .method("setShadowOffset", "(FF)V", |c| {
                    let (o, d) = (c.this, [c.float(0), c.float(1)]);
                    c.vm.ui_update(o, |v| v.style.shadow_offset = d);
                    void()
                });
        }
        if w == "ImageView" {
            b = b
                .method("setImage", "(Landroid/graphics/Bitmap;II)V", |c| {
                    let (o, img, w, h) = (c.this, c.obj(0), c.int(1), c.int(2));
                    c.vm.ui_update(o, |v| v.image = img.map(|i| (i, w, h)));
                    void()
                })
                .method("removeImage", "()V", |c| {
                    let o = c.this;
                    c.vm.ui_update(o, |v| v.image = None);
                    void()
                });
        }
        if w == "Label" {
            b = b.method("setText", "(Ljava/lang/String;)V", |c| {
                let (o, t) = (c.this, c.string(0).unwrap_or_default());
                c.vm.ui_update(o, |v| v.text = t);
                void()
            });
        }
        if w == "Button" {
            b = b
                .method("setBackgroundDrawable", "(Landroid/graphics/Bitmap;I)V", |c| {
                    let (o, bmp, state) = (c.this, c.obj(0), c.int(1));
                    c.vm.ui_update(o, |v| set_state_bitmap(&mut v.backgrounds, state, bmp));
                    void()
                })
                .method("setIconDrawable", "(Landroid/graphics/Bitmap;I)V", |c| {
                    let (o, bmp, state) = (c.this, c.obj(0), c.int(1));
                    c.vm.ui_update(o, |v| set_state_bitmap(&mut v.icons, state, bmp));
                    void()
                })
                .method("setHighlighted", "(Z)V", |c| {
                    let (o, on) = (c.this, c.int(0) != 0);
                    c.vm.ui_update(o, |v| v.highlighted = on);
                    void()
                })
                .method("setText", "(Ljava/lang/String;I)V", |c| {
                    let (o, t, state) = (c.this, c.string(0).unwrap_or_default(), c.int(1));
                    if state == 0 {
                        c.vm.ui_update(o, |v| v.text = t);
                    }
                    void()
                })
                .method("setEnabled", "(Z)V", |c| {
                    let (o, e) = (c.this, c.int(0) != 0);
                    c.vm.ui_update(o, |v| v.enabled = e);
                    void()
                });
        }
        let make = |c: &mut Call| {
            let o = c.vm.alloc_object(c.class);
            let (kind, frame) = (c.vm.class_name(c.class), c.vm.rect_of(c.obj(2)));
            c.vm.ui_create(o, kind.rsplit('/').next().unwrap_or_default(), c.int(1), frame);
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

/// Set (or clear) the bitmap a button shows in `state`.
fn set_state_bitmap(list: &mut Vec<(i32, Obj)>, state: i32, bmp: Option<Obj>) {
    list.retain(|b| b.0 != state);
    list.extend(bmp.map(|b| (state, b)));
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

/// `android.graphics.RectF`: four float fields and the few helpers the game calls.
fn install_rect(vm: &Vm) {
    use super::float;
    const FIELDS: [&str; 4] = ["left", "top", "right", "bottom"];
    fn set(c: &Call, o: Obj, v: [f32; 4]) {
        for (n, x) in FIELDS.iter().zip(v) {
            c.vm.set_field(o, n, Value::Float(x));
        }
    }
    fn get(c: &Call, o: Obj) -> [f32; 4] {
        c.vm.rect_of(Some(o))
    }
    let mut b = vm.define("android/graphics/RectF");
    for n in FIELDS {
        b = b.field(n, "F");
    }
    b.method("<init>", "()V", |_| void())
        .method("<init>", "(FFFF)V", |c| {
            set(c, c.this, [c.float(0), c.float(1), c.float(2), c.float(3)]);
            void()
        })
        .method("<init>", "(Landroid/graphics/RectF;)V", |c| {
            let src = c.vm.rect_of(c.obj(0));
            set(c, c.this, src);
            void()
        })
        .method("set", "(FFFF)V", |c| {
            set(c, c.this, [c.float(0), c.float(1), c.float(2), c.float(3)]);
            void()
        })
        .method("width", "()F", |c| float(get(c, c.this)[2] - get(c, c.this)[0]))
        .method("height", "()F", |c| float(get(c, c.this)[3] - get(c, c.this)[1]))
        .method("centerX", "()F", |c| float((get(c, c.this)[0] + get(c, c.this)[2]) / 2.0))
        .method("centerY", "()F", |c| float((get(c, c.this)[1] + get(c, c.this)[3]) / 2.0))
        .method("isEmpty", "()Z", |c| {
            let r = get(c, c.this);
            boolean(r[0] >= r[2] || r[1] >= r[3])
        })
        .method("offset", "(FF)V", |c| {
            let r = get(c, c.this);
            set(c, c.this, [r[0] + c.float(0), r[1] + c.float(1), r[2] + c.float(0), r[3] + c.float(1)]);
            void()
        })
        .method("contains", "(FF)Z", |c| {
            let (r, x, y) = (get(c, c.this), c.float(0), c.float(1));
            boolean(x >= r[0] && x < r[2] && y >= r[1] && y < r[3])
        });
}

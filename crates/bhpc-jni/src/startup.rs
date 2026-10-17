//! The Java-less start-up sequence: what `VerdeApplication`, `VerdeActivity`, `LibraryManager` and
//! `Lifecycle` would do, as plain calls the host makes in order.
//!
//! ```text
//! 1. vm.call_onload(cpu, JNI_OnLoad)        for every module that exports it, in load order
//! 2. vm.run_stage(cpu, Stage::LibraryInit)  LibraryLoader.init()
//! 3. vm.set_native_env(cpu, &native_env(..)) LibraryLoader.setenv(k, v) for each variable
//! 4. vm.run_stage(cpu, Stage::ActivityCreate / ActivityStart / ActivityResume / RootGeometry)
//! 5. vm.run_stage(cpu, Stage::ContextValid) once the GL context exists
//! 6. vm.run_stage(cpu, Stage::MainThreadRun) runs `UIApplicationMain` and does not return
//! ```

use crate::platform::Platform;
use crate::value::Value;
use crate::vm::{Throw, Vm};
use bhpc_cpu::{Cpu, Stop};

/// The environment the Java `LibraryManager` sets through `LibraryLoader.setenv` before the
/// native application starts. The names are the ones the game libraries read (`getenv` imports);
/// the value formats follow the Java side as far as it is known.
pub fn native_env(p: &Platform) -> Vec<(String, String)> {
    let (lang, _) = p.language_country();
    let v = |k: &str, val: &str| (k.to_string(), val.to_string());
    vec![
        v("DEBUG_BUILD", "0"),
        v("NSUnrecognizedForwardingDisabled", "1"),
        v("SDK_INT", &p.sdk_int.to_string()),
        v("VERSION_CODE", &p.version_code.to_string()),
        v("VERSION_NAME", &p.version_name),
        v("LOCALE", &p.locale),
        v("LANGUAGES", &lang),
        v("TZ", &p.timezone),
        v("PACKAGE_NAME", &p.package_name),
        v("PACKAGE_CODE_PATH", &p.apk_path),
        v("PACKAGE_RESOURCE_PATH", &p.apk_path),
        v("HOME", &p.files_dir),
        v("TMPDIR", &p.cache_dir),
        v("CACHEDIR", &p.cache_dir),
        v("EXTERNAL_CACHEDIR", &format!("{}/cache", p.external_dir)),
        v("OBBDIR", &p.obb_dir),
        v("ANDROID_EXTERNAL_STORAGE", &p.external_dir),
        v("ANDROID_DATA_DIRECTORY", &p.files_dir),
        v("ANDROID_DOWNLOAD_DIRECTORY", &format!("{}/Download", p.external_dir)),
    ]
}

/// One step of the start-up or life cycle, mapped to the native method that implements it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// `LibraryLoader.init()`.
    LibraryInit,
    /// `VerdeActivity.nativeOnCreate()`.
    ActivityCreate,
    ActivityStart,
    ActivityResume,
    ActivityPause,
    ActivityStop,
    ActivityDestroy,
    ActivityLowMemory,
    /// `VerdeActivity.nativeRootGeometryChanged()` (an instance method of the activity).
    RootGeometry,
    /// `Lifecycle.contextDidBecomeValid()`: the GL context exists.
    ContextValid,
    ContextInvalid,
    WillEnterForeground,
    DidBecomeActive,
    WillResignActive,
    DidEnterBackground,
    WillTerminate,
    MemoryWarning,
    /// `MainThread.nativeRun()`: `main` -> `UIApplicationMain`; returns only when the app ends.
    MainThreadRun,
}

impl Stage {
    /// `(class, method, descriptor, is_static)` of the native method.
    pub fn target(self) -> (&'static str, &'static str, &'static str, bool) {
        const ACT: &str = "com/apportable/activity/VerdeActivity";
        const LC: &str = "com/apportable/Lifecycle";
        match self {
            Stage::LibraryInit => ("com/apportable/LibraryLoader", "init", "()V", true),
            Stage::ActivityCreate => (ACT, "nativeOnCreate", "()V", true),
            Stage::ActivityStart => (ACT, "nativeOnStart", "()V", true),
            Stage::ActivityResume => (ACT, "nativeOnResume", "()V", true),
            Stage::ActivityPause => (ACT, "nativeOnPause", "()V", true),
            Stage::ActivityStop => (ACT, "nativeOnStop", "()V", true),
            Stage::ActivityDestroy => (ACT, "nativeOnDestroy", "()V", true),
            Stage::ActivityLowMemory => (ACT, "nativeOnLowMemory", "()V", true),
            Stage::RootGeometry => (ACT, "nativeRootGeometryChanged", "()V", false),
            Stage::ContextValid => (LC, "contextDidBecomeValid", "()V", true),
            Stage::ContextInvalid => (LC, "contextDidBecomeInvalid", "()V", true),
            Stage::WillEnterForeground => (LC, "applicationWillEnterForeground", "()V", true),
            Stage::DidBecomeActive => (LC, "applicationDidBecomeActive", "()V", true),
            Stage::WillResignActive => (LC, "applicationWillResignActive", "()V", true),
            Stage::DidEnterBackground => (LC, "applicationDidEnterBackground", "()V", true),
            Stage::WillTerminate => (LC, "applicationWillTerminate", "()V", true),
            Stage::MemoryWarning => (LC, "applicationDidReceiveMemoryWarning", "()V", true),
            Stage::MainThreadRun => ("com/apportable/MainThread", "nativeRun", "()V", true),
        }
    }
}

impl Vm {
    /// Call `JNI_OnLoad(JavaVM*, NULL)` at `addr` and return the JNI version it asked for.
    /// [`Vm::install`] must have run.
    pub fn call_onload(&self, cpu: &mut Cpu, addr: u32) -> Result<u32, Stop> {
        let vm = self.layout().map_or(0, |l| l.java_vm);
        let pid = cpu.processor_id();
        self.push_frame(pid);
        let r = cpu.call(addr, &[vm, 0]);
        self.pop_frame(pid, None);
        r.map(|r| r.r0)
    }

    /// `LibraryLoader.setenv(name, value)` for each pair.
    pub fn set_native_env(&self, cpu: &mut Cpu, vars: &[(String, String)]) -> Result<usize, Throw> {
        for (k, val) in vars {
            let (k, val) = (self.new_string(k), self.new_string(val));
            self.call_static(
                cpu,
                "com/apportable/LibraryLoader",
                "setenv",
                "(Ljava/lang/String;Ljava/lang/String;)V",
                &[Value::Ref(Some(k)), Value::Ref(Some(val))],
            )?;
        }
        Ok(vars.len())
    }

    /// Run one start-up/life-cycle native. If the native is not bound (no `RegisterNatives`, no
    /// `Java_*` export) the call is a reported stub and returns `Ok`: check [`Vm::stage_bound`].
    pub fn run_stage(&self, cpu: &mut Cpu, stage: Stage) -> Result<(), Throw> {
        let (class, name, sig, is_static) = stage.target();
        if is_static {
            self.call_static(cpu, class, name, sig, &[])?;
        } else {
            let a = crate::jlib::activity(self);
            self.call_method(cpu, a, name, sig, &[])?;
        }
        Ok(())
    }

    /// Whether the native behind `stage` is bound to guest code, by `RegisterNatives` or by a
    /// `Java_*` export the symbol resolver knows.
    pub fn stage_bound(&self, stage: Stage) -> bool {
        let (class, name, sig, _) = stage.target();
        if self.class_named(class).and_then(|c| self.native_addr(c, name, sig)).is_some() {
            return true;
        }
        let resolver = self.st().symbols.clone();
        resolver.is_some_and(|f| crate::invoke::native_symbols(class, name, sig).iter().any(|s| f(s).is_some()))
    }

    /// The `VerdeActivity` singleton (the receiver of instance natives and `getActivity()`).
    pub fn activity(&self) -> crate::value::Obj {
        crate::jlib::activity(self)
    }
}

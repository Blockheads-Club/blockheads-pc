//! Tests of the runner-facing facade: install, usage summary, `Assets` over `bhpc-vfs`.

use crate::platform::Assets;
use crate::{install, GuestAlloc, Jni, JniError, MemAssets, Platform, Usage, VmConfig};
use bhpc_cpu::{Cpu, CpuOptions};
use bhpc_mem::{GuestHeap, GuestMemory};
use std::io::{Cursor, Write};
use std::sync::Arc;

struct Exhausted;
impl GuestAlloc for Exhausted {
    fn alloc(&mut self, _: u32) -> u32 {
        0
    }
    fn free(&mut self, _: u32) {}
}

fn rig(alloc: Option<Box<dyn GuestAlloc>>) -> (Box<Cpu>, Result<Jni, JniError>) {
    let mem = Arc::new(GuestMemory::new().unwrap());
    let heap = GuestHeap::new(mem.clone(), 0x4000_0000, 8 << 20).unwrap();
    let cpu = Cpu::with_options(mem.clone(), CpuOptions::default()).unwrap();
    let cfg = VmConfig::new(Platform::default(), Box::new(MemAssets::default()));
    let jni = install(&cpu, mem, alloc.unwrap_or_else(|| Box::new(heap)), cfg);
    (cpu, jni)
}

#[test]
fn install_reports_exhaustion() {
    let (_cpu, r) = rig(Some(Box::new(Exhausted)));
    let e = r.err().unwrap();
    assert_eq!(e.to_string(), "guest heap exhausted while building the JNI tables");
    assert!(matches!(e, JniError::Install(_)));
    assert!(std::error::Error::source(&e).is_none());
}

#[test]
fn usage_starts_with_the_library_and_grows_with_lookups() {
    let (cpu, jni) = rig(None);
    let jni = jni.unwrap();
    assert_ne!(jni.java_vm, 0);
    assert_eq!(jni.vm.layout().unwrap().java_vm, jni.java_vm);
    let base = jni.usage();
    assert_eq!(base, Usage { classes: base.classes, ..Usage::default() });
    assert!(!base.exception_pending && base.java_calls.is_empty() && base.natives == 0);

    // Looking up something unknown invents (and reports) it; calling it counts.
    let c = jni.vm.find_class("com/example/Unknown").unwrap();
    let m = jni.vm.method_id(c, "go", "()V", true).unwrap();
    let f = jni.vm.jfield_id(c, "x", "I").unwrap();
    assert!(m != 0 && f != 0);
    assert!(jni.vm.register_native(c, "nat", "()V", 0x1000));
    let mut cpu = cpu;
    jni.vm.call_static(&mut cpu, "com/example/Unknown", "go", "()V", &[]).unwrap();
    jni.vm.call_static(&mut cpu, "com/example/Unknown", "go", "()V", &[]).unwrap();
    // Two methods with the same count are listed by name.
    jni.vm.call_static(&mut cpu, "com/example/Unknown", "alpha", "()V", &[]).unwrap();
    jni.vm.call_static(&mut cpu, "com/example/Unknown", "beta", "()V", &[]).unwrap();
    let u = jni.usage();
    let names: Vec<_> = u.java_calls.iter().map(|(n, c)| (n.as_str(), *c)).collect();
    assert_eq!(
        names,
        [("com/example/Unknown.go", 2), ("com/example/Unknown.alpha", 1), ("com/example/Unknown.beta", 1)]
    );
    assert!(u.classes > base.classes && u.methods >= 2 && u.fields == 1);
    assert_eq!(u.natives, 1);
    assert_eq!(u.java_calls[0], ("com/example/Unknown.go".to_string(), 2));
    assert!(!u.exception_pending);
}

#[test]
fn pending_exceptions_and_frames() {
    let (cpu, jni) = rig(None);
    let jni = jni.unwrap();
    let pid = cpu.processor_id();
    let before = jni.vm.live_refs();
    jni.enter(pid);
    let s = jni.vm.new_string("x");
    jni.vm.new_ref(pid, Some(s), crate::vm::RefKind::Local);
    assert_eq!(jni.vm.live_refs(), before + 1);
    jni.leave(pid);
    assert_eq!(jni.vm.live_refs(), before, "leaving the frame releases its locals");
    assert!(!jni.vm.exception_pending());
    let e = jni.vm.new_exception("java/lang/RuntimeException", "boom");
    jni.vm.set_pending(pid, e);
    assert!(jni.usage().exception_pending);
}

#[test]
fn trace_switches_the_log_sink() {
    let (_cpu, jni) = rig(None);
    let jni = jni.unwrap();
    let c = jni.vm.find_class("com/example/Traced").unwrap();
    jni.set_trace(true);
    jni.vm.method_id(c, "loud", "()V", true).unwrap();
    jni.set_trace(false);
    jni.vm.method_id(c, "quiet", "()V", true).unwrap();
}

#[test]
fn assets_come_from_the_vfs_asset_manager() {
    use zip::write::SimpleFileOptions;
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    w.start_file("assets/hello.txt", o).unwrap();
    w.write_all(b"hello").unwrap();
    w.start_file("assets/dir/b.bin", o).unwrap();
    w.write_all(b"b").unwrap();
    let apk = Arc::new(bhpc_vfs::ApkArchive::from_bytes(w.finish().unwrap().into_inner()).unwrap());
    let assets = Arc::new(bhpc_vfs::AssetManager::android(apk));
    assert_eq!(assets.read("hello.txt"), Some(b"hello".to_vec()));
    assert_eq!(assets.read("nope"), None);
    assert_eq!(Assets::list(&assets, "dir"), Some(vec!["b.bin".to_string()]));
    assert_eq!(Assets::list(&assets, "nope"), None);
}

#[test]
fn platform_for_guest_uses_android_paths() {
    let p = Platform::for_guest(&bhpc_vfs::GuestPaths::new("com.x"));
    assert_eq!(p.package_name, "com.x");
    assert_eq!(p.files_dir, "/data/data/com.x/files");
    assert_eq!(p.cache_dir, "/data/data/com.x/cache");
    assert_eq!(p.external_dir, "/sdcard");
    assert_eq!(p.obb_dir, "/sdcard/Android/obb/com.x");
    assert_eq!(p.apk_path, "/data/app/com.x-1/base.apk");
    assert_eq!(p.screen_width, Platform::default().screen_width);
}

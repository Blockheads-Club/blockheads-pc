use crate::tests_support::*;
use crate::*;

const SYMBOLS: &str = include_str!("../../../docs/feasibility/A-native-imports-symbols.txt");

fn imported() -> Vec<&'static str> {
    SYMBOLS
        .lines()
        .filter_map(|l| l.split('\t').next())
        .filter(|n| (n.starts_with("gl") || n.starts_with("egl")) && n.chars().all(|c| c.is_ascii_alphanumeric()))
        .collect()
}

#[test]
fn every_gl_and_egl_import_of_the_game_is_provided() {
    let imports = imported();
    assert_eq!(imports.len(), 381, "380 GLES + 1 EGL symbols");
    let missing: Vec<_> = imports.iter().filter(|n| !provides(n)).collect();
    assert!(missing.is_empty(), "{missing:?}");
}

#[test]
fn registry_is_consistent() {
    let names = names();
    assert!(names.windows(2).all(|w| w[0] < w[1]));
    for n in ["glDrawElements", "glActiveTexture", "eglSwapBuffers", "glOrthof", "glGenFramebuffersOES"] {
        assert!(names.contains(&n), "{n}");
    }
    assert!(!provides("glNoSuchFunction") && !provides("malloc"));
    // A function is implemented or ignored, never both.
    for n in stubs::IGNORED.iter().chain(stubs::FAILING) {
        assert!(n.starts_with("gl"));
    }
    for (alias, target) in ext::ALIASES {
        assert!(provides(alias) && provides(target), "{alias} -> {target}");
    }
    let mut seen = std::collections::HashSet::new();
    for t in [marshal::TABLE, ext::TABLE, egl::TABLE] {
        for (n, _) in t {
            assert!(seen.insert(*n), "{n} registered twice");
        }
    }
    for (n, _) in direct::TABLE.iter().flatten() {
        assert!(seen.insert(*n), "{n} registered twice");
    }
    for n in stubs::IGNORED.iter().chain(stubs::FAILING) {
        assert!(seen.insert(*n), "{n} is also implemented");
    }
}

#[test]
fn register_all_binds_every_name_once() {
    let mut e = Env::new();
    let n = register_all(&mut e.cpu, &e.gl);
    assert_eq!(n, names().len());
    assert_eq!(e.cpu.thunk_count(), n);
    let a = resolve(&mut e.cpu, &e.gl, "glClear").unwrap();
    assert_eq!(resolve(&mut e.cpu, &e.gl, "glClear"), Some(a));
    assert_eq!(resolve(&mut e.cpu, &e.gl, "memcpy"), None);
    assert!(builtin("memcpy", &e.gl).is_none());
}

#[test]
fn every_direct_thunk_reaches_the_backend() {
    for (name, _) in direct::TABLE.iter().flatten() {
        let mut e = Env::new();
        e.call(name, &[1, 2, 3, 4]);
        assert_eq!(e.rec.names(), [name.to_string()], "{name}");
    }
}

#[test]
fn direct_thunks_decode_floats_and_return_values() {
    let mut e = Env::new();
    e.call("glClearColor", &[0.25f32.to_bits(), 0.5f32.to_bits(), 0.75f32.to_bits(), 1.0f32.to_bits()]);
    e.call("glColorMask", &[1, 0, 1, 0x100]);
    e.call("glUniform4i", &[7, 1, 2, 3, 4]);
    e.rec.set_ret("glCreateShader", 42);
    assert_eq!(e.call("glCreateShader", &[0x8B31]), 42);
    e.rec.set_ret("glIsTexture", 1);
    assert_eq!(e.call("glIsTexture", &[3]), 1);
    let c = e.rec.take();
    assert_eq!(c[0].1, [f(0.25), f(0.5), f(0.75), f(1.0)]);
    assert_eq!(c[1].1, [i(1), i(0), i(1), i(0)]);
    assert_eq!(c[2].1, [i(7), i(1), i(2), i(3), i(4)], "fifth argument comes from the stack");
}

#[test]
fn ignored_and_failing_stubs_are_counted_and_logged_once() {
    let mut e = Env::new();
    assert_eq!(e.call("glOrthof", &[1, 2]), 0);
    e.call("glOrthof", &[1, 2]);
    e.call("glBeginPerfMonitorAMD", &[]);
    assert_eq!(e.call("glShaderBinary", &[]), 0);
    assert_eq!(e.call("glGetError", &[]), consts::GL_INVALID_OPERATION);
    assert_eq!(e.call("glGetError", &[]), 0, "the HLE error is cleared by reading it");
    let s = e.gl.stats();
    assert_eq!(s.unsupported["glOrthof"], 2);
    assert_eq!(s.unsupported["glShaderBinary"], 1);
    assert_eq!(s.calls, 6);
    assert_eq!(e.log_text().matches("unsupported call glOrthof").count(), 1);
    assert!(e.rec.names().iter().all(|n| n == "glGetError"), "stubs never reach the driver");
}

#[test]
fn check_errors_drains_the_host_queue_after_each_call() {
    let mut e = Env::with(|c| c.check_errors = true);
    e.call("glClear", &[0x4000]);
    assert_eq!(e.rec.names(), ["glClear", "glGetError"]);
    e.rec.take();
    e.rec.set_ret("glGetError", 0x501);
    e.call("glClear", &[0x4000]);
    assert_eq!(e.gl.stats().gl_errors.len(), 8, "bounded drain");
    assert!(e.log_text().contains("GL error 0x501 after glClear"));
    e.rec.take();
    e.call("glGetError", &[]);
    assert_eq!(e.rec.names(), ["glGetError"], "glGetError itself is not followed by another one");
}

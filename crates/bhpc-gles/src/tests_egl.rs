use crate::consts::*;
use crate::egl::*;
use crate::tests_support::*;
use crate::{GlWindow, Gles, GlesConfig};
use bhpc_cpu::Stop;
use std::sync::atomic::Ordering;
use std::sync::Arc;

const S: u32 = SCRATCH;
const BAD: u32 = 0x1000_0000;

fn fault(r: Result<bhpc_cpu::CallResult, Stop>) -> bool {
    matches!(r, Err(Stop::Fault(_)))
}

fn egl_err(e: &mut Env) -> u32 {
    e.call("eglGetError", &[])
}

fn init(e: &mut Env) {
    assert_eq!(e.call("eglInitialize", &[EGL_DISPLAY, 0, 0]), 1);
}

#[test]
fn config_tables() {
    assert_eq!(config_attrib(EGL_DEPTH_SIZE), Some(24));
    assert_eq!(config_attrib(EGL_SURFACE_TYPE), Some(EGL_WINDOW_BIT));
    assert_eq!(config_attrib(0x9999), None);
    for (a, v) in [
        (EGL_BUFFER_SIZE, 32),
        (EGL_STENCIL_SIZE, 8),
        (EGL_CONFIG_ID, 1),
        (EGL_NATIVE_VISUAL_ID, 1),
        (EGL_CONFIG_CAVEAT, EGL_NONE),
        (EGL_TRANSPARENT_TYPE, EGL_NONE),
        (EGL_NATIVE_VISUAL_TYPE, EGL_NONE),
        (EGL_LEVEL, 0),
        (EGL_NATIVE_RENDERABLE, 0),
        (EGL_RENDERABLE_TYPE, 5),
        (EGL_MAX_PBUFFER_WIDTH, 4096),
        (EGL_MAX_PBUFFER_HEIGHT, 4096),
        (EGL_MAX_PBUFFER_PIXELS, 4096 * 4096),
    ] {
        assert_eq!(config_attrib(a), Some(v), "{a:#x}");
    }
    assert!(config_matches(&[]));
    assert!(config_matches(&[(EGL_RED_SIZE, 8), (EGL_DEPTH_SIZE, 16), (EGL_STENCIL_SIZE, 0), (EGL_NONE + 1, 5)]));
    assert!(config_matches(&[(EGL_RED_SIZE, EGL_DONT_CARE as u32)]));
    assert!(!config_matches(&[(EGL_RED_SIZE, 10)]));
    assert!(!config_matches(&[(EGL_DEPTH_SIZE, 32)]));
    assert!(config_matches(&[(EGL_SAMPLES, 0), (EGL_SAMPLE_BUFFERS, EGL_DONT_CARE as u32)]));
    assert!(!config_matches(&[(EGL_SAMPLES, 4)]));
    assert!(config_matches(&[(EGL_SURFACE_TYPE, EGL_WINDOW_BIT)]));
    assert!(!config_matches(&[(EGL_SURFACE_TYPE, 0x1)]));
    assert!(config_matches(&[(EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT)]));
    assert!(config_matches(&[(EGL_RENDERABLE_TYPE, 0)]));
    assert!(config_matches(&[(EGL_RENDERABLE_TYPE, EGL_DONT_CARE as u32)]));
    assert!(!config_matches(&[(EGL_RENDERABLE_TYPE, 0x8)]));
}

#[test]
fn display_lifecycle_and_errors() {
    let mut e = Env::new();
    assert_eq!(e.call("eglGetDisplay", &[0]), EGL_DISPLAY);
    assert_eq!(egl_err(&mut e), EGL_SUCCESS);
    assert_eq!(e.call("eglGetConfigs", &[EGL_DISPLAY, 0, 0, S]), 0);
    assert_eq!(egl_err(&mut e), EGL_NOT_INITIALIZED);
    assert_eq!(egl_err(&mut e), EGL_SUCCESS, "reading clears the error");
    assert_eq!(e.call("eglInitialize", &[7, 0, 0]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_DISPLAY);
    assert_eq!(e.call("eglInitialize", &[EGL_DISPLAY, S, S + 4]), 1);
    assert_eq!((e.word(S), e.word(S + 4)), (1, 4));
    assert!(fault(e.try_call("eglInitialize", &[EGL_DISPLAY, BAD, 0])));
    assert!(fault(e.try_call("eglInitialize", &[EGL_DISPLAY, 0, BAD])));
    assert_eq!(e.call("eglGetConfigs", &[EGL_DISPLAY, 0, 0, S]), 1);
    assert_eq!(e.call("eglTerminate", &[7]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_DISPLAY);
    assert_eq!(e.call("eglTerminate", &[EGL_DISPLAY]), 1);
    assert_eq!(e.call("eglGetConfigs", &[EGL_DISPLAY, 0, 0, S]), 0);
    assert_eq!(egl_err(&mut e), EGL_NOT_INITIALIZED);
}

#[test]
fn query_string_is_cached() {
    let mut e = Env::new();
    for (name, want) in
        [(EGL_VENDOR, "bhpc"), (EGL_VERSION, "1.4 bhpc"), (EGL_CLIENT_APIS, "OpenGL_ES"), (EGL_EXTENSIONS, "")]
    {
        let a = e.call("eglQueryString", &[EGL_DISPLAY, name]);
        assert_eq!(e.mem().read_cstr(a).unwrap(), want);
        assert_eq!(e.call("eglQueryString", &[EGL_DISPLAY, name]), a);
    }
    assert_eq!(e.call("eglQueryString", &[EGL_DISPLAY, 0x1234]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_PARAMETER);
}

#[test]
fn query_string_out_of_memory_returns_null() {
    let mut e = Env::with(|c| c.arena_size = 0x1000);
    // Eat the arena with buffer shadows.
    for k in 1..1000 {
        e.call("glBindBuffer", &[GL_ARRAY_BUFFER, k]);
        e.call("glBufferData", &[GL_ARRAY_BUFFER, 16, 0, 0]);
        if e.call("glMapBufferOES", &[GL_ARRAY_BUFFER, GL_WRITE_ONLY_OES]) == 0 {
            break;
        }
    }
    assert_eq!(e.call("eglQueryString", &[EGL_DISPLAY, EGL_VERSION]), 0);
}

#[test]
fn configs() {
    let mut e = Env::new();
    init(&mut e);
    // Query only the count.
    assert_eq!(e.call("eglGetConfigs", &[EGL_DISPLAY, 0, 0, S]), 1);
    assert_eq!(e.word(S), 1);
    assert_eq!(e.call("eglGetConfigs", &[EGL_DISPLAY, S + 8, 4, S]), 1);
    assert_eq!(e.word(S + 8), EGL_CONFIG);
    assert_eq!(e.call("eglGetConfigs", &[EGL_DISPLAY, S, 1, 0]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_PARAMETER);
    assert!(fault(e.try_call("eglGetConfigs", &[EGL_DISPLAY, BAD, 1, S])));
    assert!(fault(e.try_call("eglGetConfigs", &[EGL_DISPLAY, S, 1, BAD])));
    // eglChooseConfig with a typical Android attribute list.
    e.put_words(S + 0x40, &[EGL_RED_SIZE, 8, EGL_DEPTH_SIZE, 16, EGL_RENDERABLE_TYPE, 4, EGL_NONE]);
    assert_eq!(e.call("eglChooseConfig", &[EGL_DISPLAY, S + 0x40, S + 0x100, 5, S + 0x104]), 1);
    assert_eq!((e.word(S + 0x100), e.word(S + 0x104)), (EGL_CONFIG, 1));
    e.put_words(S + 0x60, &[EGL_SAMPLES, 4, EGL_NONE]);
    e.put_words(S + 0x100, &[0xAA]);
    assert_eq!(e.call("eglChooseConfig", &[EGL_DISPLAY, S + 0x60, S + 0x100, 5, S + 0x104]), 1);
    assert_eq!((e.word(S + 0x100), e.word(S + 0x104)), (0xAA, 0), "nothing matches");
    assert_eq!(e.call("eglChooseConfig", &[EGL_DISPLAY, 0, S + 0x100, 0, S + 0x104]), 1);
    assert_eq!(e.word(S + 0x104), 0, "a size of 0 returns no configs");
    assert!(fault(e.try_call("eglChooseConfig", &[EGL_DISPLAY, BAD, S, 1, S])));
    // An attribute list without terminator is cut at 64 pairs.
    let long: Vec<u32> = (0..200).flat_map(|_| [EGL_LEVEL, 0]).collect();
    e.put_words(S + 0x200, &long);
    assert_eq!(e.call("eglChooseConfig", &[EGL_DISPLAY, S + 0x200, S + 0x100, 5, S + 0x104]), 1);
}

#[test]
fn config_attributes() {
    let mut e = Env::new();
    assert_eq!(e.call("eglGetConfigAttrib", &[EGL_DISPLAY, EGL_CONFIG, EGL_RED_SIZE, S]), 0);
    assert_eq!(egl_err(&mut e), EGL_NOT_INITIALIZED);
    init(&mut e);
    assert_eq!(e.call("eglGetConfigAttrib", &[EGL_DISPLAY, EGL_CONFIG, EGL_RED_SIZE, S]), 1);
    assert_eq!(e.word(S), 8);
    assert_eq!(e.call("eglGetConfigAttrib", &[EGL_DISPLAY, 9, EGL_RED_SIZE, S]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_CONFIG);
    assert_eq!(e.call("eglGetConfigAttrib", &[EGL_DISPLAY, EGL_CONFIG, 0x9999, S]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_ATTRIBUTE);
    assert!(fault(e.try_call("eglGetConfigAttrib", &[EGL_DISPLAY, EGL_CONFIG, EGL_RED_SIZE, BAD])));
}

#[test]
fn surfaces() {
    let mut e = Env::new();
    assert_eq!(e.call("eglCreateWindowSurface", &[EGL_DISPLAY, EGL_CONFIG, 0, 0]), 0);
    assert_eq!(egl_err(&mut e), EGL_NOT_INITIALIZED);
    init(&mut e);
    assert_eq!(e.call("eglCreateWindowSurface", &[EGL_DISPLAY, 3, 0, 0]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_CONFIG);
    let surf = e.call("eglCreateWindowSurface", &[EGL_DISPLAY, EGL_CONFIG, 0x1234, 0]);
    assert_ne!(surf, 0);
    assert_eq!(e.call("eglCreatePbufferSurface", &[EGL_DISPLAY, EGL_CONFIG, 0]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_MATCH);
    // eglQuerySurface
    *e.win.size.lock().unwrap() = (1920, 1080);
    for (attr, want) in
        [(EGL_WIDTH, 1920), (EGL_HEIGHT, 1080), (EGL_CONFIG_ID, EGL_CONFIG), (EGL_RENDER_BUFFER, EGL_BACK_BUFFER)]
    {
        assert_eq!(e.call("eglQuerySurface", &[EGL_DISPLAY, surf, attr, S]), 1);
        assert_eq!(e.word(S), want);
    }
    // The game's own surface size wins over the drawable.
    GlWindow::set_surface_size(&*e.win, 1024, 768);
    assert_eq!(e.call("eglQuerySurface", &[EGL_DISPLAY, surf, EGL_WIDTH, S]), 1);
    assert_eq!(e.word(S), 1024);
    assert_eq!(e.gl.surface_size(), (1024, 768));
    *e.win.surface.lock().unwrap() = None;
    assert_eq!(e.call("eglQuerySurface", &[EGL_DISPLAY, surf, 0x9999, S]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_ATTRIBUTE);
    assert_eq!(e.call("eglQuerySurface", &[EGL_DISPLAY, 77, EGL_WIDTH, S]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_SURFACE);
    assert_eq!(e.call("eglQuerySurface", &[9, surf, EGL_WIDTH, S]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_DISPLAY);
    assert!(fault(e.try_call("eglQuerySurface", &[EGL_DISPLAY, surf, EGL_WIDTH, BAD])));
    assert_eq!(e.gl.drawable_size(), (1920, 1080));
    // eglDestroySurface
    assert_eq!(e.call("eglDestroySurface", &[EGL_DISPLAY, surf]), 1);
    assert_eq!(e.call("eglDestroySurface", &[EGL_DISPLAY, surf]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_SURFACE);
    assert_eq!(e.call("eglDestroySurface", &[5, surf]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_DISPLAY);
}

#[test]
fn api_binding_and_waits() {
    let mut e = Env::new();
    assert_eq!(e.call("eglBindAPI", &[EGL_OPENGL_ES_API]), 1);
    assert_eq!(e.call("eglBindAPI", &[0x30A2]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_PARAMETER);
    assert_eq!(e.call("eglQueryAPI", &[]), EGL_OPENGL_ES_API);
    for n in ["eglWaitClient", "eglWaitGL", "eglWaitNative"] {
        assert_eq!(e.call(n, &[0x305B]), 1);
    }
    assert_eq!(e.call("eglReleaseThread", &[]), 1);
}

fn setup(e: &mut Env) -> (u32, u32) {
    init(e);
    let surf = e.call("eglCreateWindowSurface", &[EGL_DISPLAY, EGL_CONFIG, 0, 0]);
    e.put_words(S + 0x300, &[EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE]);
    let ctx = e.call("eglCreateContext", &[EGL_DISPLAY, EGL_CONFIG, 0, S + 0x300]);
    assert_ne!(ctx, 0);
    (surf, ctx)
}

#[test]
fn contexts_and_make_current() {
    let mut e = Env::new();
    assert_eq!(e.call("eglCreateContext", &[EGL_DISPLAY, EGL_CONFIG, 0, 0]), 0);
    assert_eq!(egl_err(&mut e), EGL_NOT_INITIALIZED);
    let (surf, ctx) = setup(&mut e);
    assert_eq!(e.call("eglCreateContext", &[EGL_DISPLAY, 4, 0, 0]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_CONFIG);
    e.put_words(S + 0x320, &[EGL_CONTEXT_CLIENT_VERSION, 3, EGL_NONE]);
    assert_eq!(e.call("eglCreateContext", &[EGL_DISPLAY, EGL_CONFIG, 0, S + 0x320]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_ATTRIBUTE);
    assert!(fault(e.try_call("eglCreateContext", &[EGL_DISPLAY, EGL_CONFIG, 0, BAD])));
    // No attribute list is fine; unknown share context is not.
    let shared = e.call("eglCreateContext", &[EGL_DISPLAY, EGL_CONFIG, ctx, 0]);
    assert_ne!(shared, 0);
    assert!(e.win.shared_with.lock().unwrap().last().unwrap().is_some());
    assert_eq!(e.call("eglCreateContext", &[EGL_DISPLAY, EGL_CONFIG, 0x7777, 0]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_CONTEXT);
    e.win.fail_create.store(true, Ordering::SeqCst);
    assert_eq!(e.call("eglCreateContext", &[EGL_DISPLAY, EGL_CONFIG, 0, 0]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_ALLOC);
    e.win.fail_create.store(false, Ordering::SeqCst);

    // queries
    assert_eq!(e.call("eglGetCurrentContext", &[]), 0);
    assert_eq!(e.call("eglGetCurrentSurface", &[0x3059]), 0);
    assert_eq!(e.call("eglGetCurrentDisplay", &[]), 0);
    // make current
    assert_eq!(e.call("eglMakeCurrent", &[EGL_DISPLAY, surf, surf, ctx]), 1);
    assert_eq!(e.call("eglGetCurrentContext", &[]), ctx);
    assert_eq!(e.call("eglGetCurrentSurface", &[0x3059]), surf);
    assert_eq!(e.call("eglGetCurrentDisplay", &[]), EGL_DISPLAY);
    assert_eq!(e.gl.egl_current_context(), ctx);
    assert!(e.win.made_current.lock().unwrap().last().unwrap().is_some());
    // Another thread sees no current context: eglGetCurrentContext is per thread.
    let g2 = e.gl.clone();
    assert_eq!(std::thread::spawn(move || g2.egl_current_context()).join().unwrap(), 0);
    assert_eq!(e.call("eglMakeCurrent", &[9, surf, surf, ctx]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_DISPLAY);
    assert_eq!(e.call("eglMakeCurrent", &[EGL_DISPLAY, surf, surf, 0x7777]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_CONTEXT);
    assert_eq!(e.call("eglMakeCurrent", &[EGL_DISPLAY, 0x7777, surf, ctx]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_SURFACE);
    assert_eq!(e.call("eglMakeCurrent", &[EGL_DISPLAY, surf, 0x7777, ctx]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_SURFACE);
    e.win.fail_make_current.store(true, Ordering::SeqCst);
    assert_eq!(e.call("eglMakeCurrent", &[EGL_DISPLAY, 0, 0, shared]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_ACCESS);
    e.win.fail_make_current.store(false, Ordering::SeqCst);
    // eglQueryContext
    for (attr, want) in [
        (EGL_CONTEXT_CLIENT_TYPE, EGL_OPENGL_ES_API),
        (EGL_CONTEXT_CLIENT_VERSION, 2),
        (EGL_CONFIG_ID, EGL_CONFIG),
        (EGL_RENDER_BUFFER, EGL_BACK_BUFFER),
    ] {
        assert_eq!(e.call("eglQueryContext", &[EGL_DISPLAY, ctx, attr, S]), 1);
        assert_eq!(e.word(S), want);
    }
    assert_eq!(e.call("eglQueryContext", &[EGL_DISPLAY, ctx, 0x9999, S]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_ATTRIBUTE);
    assert_eq!(e.call("eglQueryContext", &[EGL_DISPLAY, 0x7777, EGL_CONFIG_ID, S]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_CONTEXT);
    assert_eq!(e.call("eglQueryContext", &[9, ctx, EGL_CONFIG_ID, S]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_DISPLAY);
    assert!(fault(e.try_call("eglQueryContext", &[EGL_DISPLAY, ctx, EGL_CONFIG_ID, BAD])));
    // release
    assert_eq!(e.call("eglMakeCurrent", &[EGL_DISPLAY, 0, 0, 0]), 1);
    assert_eq!(e.call("eglGetCurrentContext", &[]), 0);
    assert_eq!(e.win.made_current.lock().unwrap().last(), Some(&None));
}

#[test]
fn destroying_contexts() {
    let mut e = Env::new();
    let (surf, ctx) = setup(&mut e);
    let other = e.gl.egl_create_context(0).unwrap();
    assert_eq!(e.call("eglDestroyContext", &[EGL_DISPLAY, other]), 1);
    assert_eq!(e.win.destroyed.lock().unwrap().len(), 1);
    assert_eq!(e.call("eglDestroyContext", &[EGL_DISPLAY, other]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_CONTEXT);
    assert_eq!(e.call("eglDestroyContext", &[9, ctx]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_DISPLAY);
    e.call("eglMakeCurrent", &[EGL_DISPLAY, surf, surf, ctx]);
    let before = e.win.made_current.lock().unwrap().len();
    assert_eq!(e.call("eglDestroyContext", &[EGL_DISPLAY, ctx]), 1);
    assert_eq!(e.win.made_current.lock().unwrap().len(), before + 1, "a current context is released first");
    assert_eq!(e.gl.egl_current_context(), 0);
    assert_eq!(e.gl.egl_current_surface(), 0);
}

#[test]
fn swapping_and_intervals() {
    let mut e = Env::new();
    let (surf, ctx) = setup(&mut e);
    assert_eq!(e.call("eglSwapBuffers", &[EGL_DISPLAY, surf]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_CONTEXT, "no current context");
    e.call("eglMakeCurrent", &[EGL_DISPLAY, surf, surf, ctx]);
    assert_eq!(e.call("eglSwapBuffers", &[EGL_DISPLAY, surf]), 1);
    assert_eq!(e.win.swaps.load(Ordering::SeqCst), 1);
    assert_eq!(e.call("eglSwapBuffers", &[EGL_DISPLAY, 0x7777]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_SURFACE);
    assert_eq!(e.call("eglSwapBuffers", &[9, surf]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_DISPLAY);
    assert_eq!(e.call("eglSwapInterval", &[EGL_DISPLAY, 0]), 1);
    assert_eq!(e.win.interval.load(Ordering::SeqCst), 0);
    assert_eq!(e.call("eglSwapInterval", &[EGL_DISPLAY, -4i32 as u32]), 1);
    e.win.fail_interval.store(true, Ordering::SeqCst);
    assert_eq!(e.call("eglSwapInterval", &[EGL_DISPLAY, 1]), 0);
    assert_eq!(e.call("eglSwapInterval", &[9, 1]), 0);
    assert_eq!(egl_err(&mut e), EGL_BAD_DISPLAY);
    assert_eq!(e.gl.lock().egl.swap_interval, 0);
}

#[test]
fn the_host_knows_where_a_context_is_current() {
    let mut e = Env::new();
    let (surf, ctx) = setup(&mut e);
    assert!(!e.gl.context_current());
    e.call("eglMakeCurrent", &[EGL_DISPLAY, surf, surf, ctx]);
    assert!(e.gl.context_current());
}

#[test]
fn frame_hook_runs_before_each_swap() {
    let mut e = Env::new();
    let (surf, ctx) = setup(&mut e);
    e.call("eglMakeCurrent", &[EGL_DISPLAY, surf, surf, ctx]);
    let n = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let n2 = n.clone();
    e.gl.set_frame_hook(Arc::new(move |_c| {
        n2.fetch_add(1, Ordering::SeqCst);
    }));
    assert_eq!(e.call("eglSwapBuffers", &[EGL_DISPLAY, surf]), 1);
    assert_eq!(n.load(Ordering::SeqCst), 1);
}

#[test]
fn get_proc_address_hands_out_thunks() {
    let mut e = Env::new();
    e.put(S, b"glClear\0");
    let a = e.call("eglGetProcAddress", &[S]);
    assert_eq!(a, e.addr("glClear"));
    e.put(S, b"glNoSuch\0");
    assert_eq!(e.call("eglGetProcAddress", &[S]), 0);
    e.put(S, &[0xFF, 0xFE, 0]);
    assert_eq!(e.call("eglGetProcAddress", &[S]), 0);
    assert!(fault(e.try_call("eglGetProcAddress", &[BAD])));
}

#[test]
fn with_host_resolves_the_driver_through_the_window() {
    let win = FakeWindow::new();
    let mem = Arc::new(bhpc_mem::GuestMemory::new().unwrap());
    let gl = Gles::with_host(GlesConfig::new(ARENA, 1 << 16), win, mem).unwrap();
    assert_eq!(gl.drawable_size(), (800, 480));
    assert_eq!(gl.stats().calls, 0);
    // Every entry point resolves on the dummy driver.
    assert!(gl.lock().api.supports("glClear"));
    // The arena region is mapped by `new`: a second instance over the same memory overlaps.
    let mem2 = Arc::new(bhpc_mem::GuestMemory::new().unwrap());
    mem2.map(ARENA, 0x1000).unwrap();
    assert!(Gles::with_host(GlesConfig::new(ARENA, 1 << 16), FakeWindow::new(), mem2).is_err());
}

#[test]
fn rust_api_for_the_java_side() {
    let mut e = Env::new();
    e.gl.egl_initialize();
    let primary = e.gl.egl_create_context(0).unwrap();
    let surface = e.gl.egl_create_surface();
    e.gl.egl_make_current(surface, surface, primary).unwrap();
    let shared = e.gl.egl_create_context(primary).unwrap();
    assert_ne!(shared, primary);
    assert_eq!(e.gl.egl_destroy_surface(99), Err(EGL_BAD_SURFACE));
    assert!(e.gl.egl_swap_interval(1));
    e.gl.egl_swap_buffers(surface).unwrap();
    assert_eq!(e.win.swaps.load(Ordering::SeqCst), 1);
    e.call("eglWaitGL", &[]);
    // A context the embedder made itself becomes an ordinary EGL context.
    let adopted = e.gl.egl_adopt_context(4242);
    e.gl.egl_make_current(surface, surface, adopted).unwrap();
    assert_eq!(e.win.made_current.lock().unwrap().last(), Some(&Some(4242)));
    e.gl.egl_destroy_context(adopted).unwrap();
    assert_eq!(*e.win.destroyed.lock().unwrap(), [4242]);
}

#[test]
fn the_hle_can_be_shared_between_threads() {
    fn is_send_sync<T: Send + Sync + Clone + 'static>() {}
    is_send_sync::<Gles>();
}

#[test]
fn a_window_without_an_offscreen_surface_renders_straight_into_itself() {
    use crate::HostContext;
    struct Bare;
    impl GlWindow for Bare {
        fn create_context(&self, _: Option<HostContext>) -> Option<HostContext> {
            None
        }
        fn destroy_context(&self, _: HostContext) {}
        fn make_current(&self, _: Option<HostContext>) -> bool {
            false
        }
        fn swap_buffers(&self) {}
        fn set_swap_interval(&self, _: i32) -> bool {
            false
        }
        fn drawable_size(&self) -> (u32, u32) {
            (640, 480)
        }
        fn proc_address(&self, _: &str) -> *const std::ffi::c_void {
            std::ptr::null()
        }
    }
    let w = Bare;
    w.set_surface_size(1024, 768);
    assert_eq!(w.surface_size(), (640, 480), "the surface is the drawable");
    assert_eq!(w.default_framebuffer(), 0);
    w.set_overlay(None);
    w.compose();
    assert!(!w.desktop_gl(), "OpenGL ES unless the window says otherwise");
}

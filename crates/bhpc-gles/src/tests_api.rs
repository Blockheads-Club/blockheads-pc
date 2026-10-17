use crate::api::*;
use crate::tests_support::*;
use bhpc_cpu::Cpu;

#[test]
fn recorder_records_every_function() {
    let mut r = Recorder::default();
    exercise_all(&mut r);
    assert_eq!(r.names().len(), API_NAMES.len());
    assert_eq!(r.names()[0], API_NAMES[0]);
    r.set_ret("glGetString", 0x1234);
    r.set_ret("glIsBuffer", 1);
    assert_eq!(r.glGetString(7) as usize, 0x1234);
    assert_eq!(r.glIsBuffer(1), 1);
    assert!(r.supports("glClear"));
    r.missing("glClear");
    assert!(!r.supports("glClear"));
}

#[test]
fn host_gl_forwards_through_function_pointers() {
    let mut gl = HostGl::load(&mut dummy_driver::lookup);
    assert!(API_NAMES.iter().all(|n| gl.supports(n)));
    assert!(!gl.supports("glNoSuchThing"));
    exercise_all(&mut gl);
    assert!(dummy_driver::lookup("glNoSuchThing").is_null());
}

#[test]
fn host_gl_without_entry_points_does_nothing() {
    let mut gl = HostGl::load(&mut |_| std::ptr::null());
    assert!(API_NAMES.iter().all(|n| !gl.supports(n)));
    exercise_all(&mut gl);
    assert_eq!(gl.glCreateShader(1), 0);
    assert!(gl.glGetString(1).is_null());
}

#[test]
fn host_gl_retries_oes_and_ext_names() {
    let mut asked = Vec::new();
    let gl = HostGl::load(&mut |n| {
        asked.push(n.to_string());
        match n {
            "glGenVertexArraysOES" | "glMultiDrawFooEXT" => dummy_driver::lookup("glGenVertexArrays"),
            _ => std::ptr::null(),
        }
    });
    assert!(gl.supports("glGenVertexArrays"));
    assert!(!gl.supports("glBindVertexArray"));
    assert!(asked.contains(&"glBindVertexArray".to_string()));
    assert!(asked.contains(&"glBindVertexArrayOES".to_string()));
    assert!(asked.contains(&"glBindVertexArrayEXT".to_string()));
    let gl = HostGl::load(&mut |n| if n == "glFlushEXT" { dummy_driver::lookup("glFlush") } else { std::ptr::null() });
    assert!(gl.supports("glFlush"));
}

#[test]
fn guest_value_conversions() {
    let mut env = Env::new();
    ToGuest::put(5i32, &mut env.cpu);
    assert_eq!(env.cpu.reg(0), 5);
    ToGuest::put(0x82u8, &mut env.cpu);
    assert_eq!(env.cpu.reg(0), 0x82);
    ToGuest::put((), &mut env.cpu);
    assert_eq!(env.cpu.reg(0), 0x82);
    let c: &Cpu = &env.cpu;
    let mut a = c.args();
    assert_eq!(<i32 as FromGuest>::take(&mut a, c), 0x82);
    assert_eq!(<u8 as Dummy>::dummy(), 0);
    assert!(<PM as Dummy>::dummy().is_null());
    assert_eq!(3.5f32.to_arg(), Arg::F(3.5));
    assert_eq!(<()>::from_ret(3), ());
    assert_eq!(<P>::from_ret(8) as usize, 8);
    assert_eq!(9u8.to_arg(), Arg::I(9));
    assert_eq!(<PM as ToArg>::to_arg(&(4usize as PM)), Arg::P(4));
}

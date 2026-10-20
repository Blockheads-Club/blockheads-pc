//! The fake JVM: classes, objects, references, exceptions and method dispatch.
//!
//! [`Vm`] is a cheap clone of an `Arc`. All mutable state sits behind one mutex that is never
//! held while host handlers or guest code run, so handlers may call back into the `Vm` and the
//! guest (nested calls) freely.

use crate::mutf8;
use crate::platform::{Assets, AudioHooks, GfxHooks, NoAudio, NoGfx, Platform};
use crate::value::{parse_field_ty, parse_sig, ClassId, Obj, Sig, Ty, Value};
use bhpc_cpu::{Cpu, Stop};
use bhpc_mem::GuestMemory;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

/// Why a Java call did not produce a value.
#[derive(Debug, Clone, PartialEq)]
pub enum Throw {
    /// A Java exception (a `Throwable` object).
    Exc(Obj),
    /// The guest stopped (fault, watchdog, ...): not catchable, propagates to the CPU run loop.
    Stop(Stop),
}

pub type JResult = Result<Value, Throw>;

/// A host implementation of a Java method.
pub type HostFn = Arc<dyn Fn(&mut Call) -> JResult + Send + Sync>;

/// Arguments of a host-implemented method call.
pub struct Call<'a> {
    pub vm: &'a Vm,
    pub cpu: &'a mut Cpu,
    /// Receiver (`Obj::NULL` for static methods).
    pub this: Obj,
    /// The class the call was made on (the receiver's class, or the static class).
    pub class: ClassId,
    pub args: &'a [Value],
}

impl Call<'_> {
    pub fn int(&self, i: usize) -> i32 {
        self.args.get(i).map_or(0, Value::as_int)
    }
    pub fn long(&self, i: usize) -> i64 {
        self.args.get(i).map_or(0, Value::as_long)
    }
    pub fn float(&self, i: usize) -> f32 {
        self.args.get(i).map_or(0.0, Value::as_float)
    }
    pub fn double(&self, i: usize) -> f64 {
        self.args.get(i).map_or(0.0, Value::as_double)
    }
    pub fn obj(&self, i: usize) -> Option<Obj> {
        self.args.get(i).and_then(Value::as_ref)
    }
    /// A `String` argument (None for null or non-strings).
    pub fn string(&self, i: usize) -> Option<String> {
        self.obj(i).and_then(|o| self.vm.string(o))
    }
}

/// What a method does.
#[derive(Clone)]
pub enum Imp {
    Host(HostFn),
    /// A guest function (ARM address), bound by `RegisterNatives` or found as a `Java_*` export.
    Native(u32),
    /// Unknown member invented on demand: returns the zero value and is reported.
    Stub,
}

/// Payload of a heap object.
#[derive(Debug, Clone, PartialEq)]
pub enum Data {
    None,
    /// `String`/`StringBuilder` contents (UTF-16).
    Str(Vec<u16>),
    /// Primitive array: element type and little-endian bytes.
    Prim(Ty, Vec<u8>),
    Objs(Vec<Option<Obj>>),
    /// The `java.lang.Class` mirror of a class.
    Class(ClassId),
    /// `ArrayList` and friends.
    List(Vec<Option<Obj>>),
    /// `HashMap` and friends (insertion ordered).
    Map(Vec<(Option<Obj>, Option<Obj>)>),
    /// An in-memory `InputStream`.
    Bytes {
        data: Vec<u8>,
        pos: usize,
    },
    /// A `MessageQueue`: pending messages ordered by `when`.
    Queue(Vec<Obj>),
}

pub(crate) struct Object {
    pub class: ClassId,
    pub data: Data,
    pub fields: HashMap<u32, Value>,
}

pub(crate) struct ClassDef {
    pub name: String,
    pub sup: Option<ClassId>,
    pub ifaces: Vec<ClassId>,
    pub interface: bool,
    pub defined: bool,
    /// Created on demand for an unknown name.
    pub stub: bool,
    pub methods: HashMap<(String, String), u32>,
    pub fields: HashMap<String, u32>,
    pub init: Option<fn() -> Data>,
    pub mirror: Option<Obj>,
}

pub(crate) struct MethodDef {
    pub class: ClassId,
    pub name: String,
    pub sig_str: String,
    pub sig: Sig,
    pub is_static: bool,
    pub imp: Imp,
    pub looked_up: bool,
    pub calls: u64,
}

pub(crate) struct FieldDef {
    pub class: ClassId,
    pub name: String,
    pub desc: String,
    pub ty: Ty,
    pub value: Value,
    pub looked_up: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RefKind {
    Local = 1,
    Global = 2,
    Weak = 3,
}

struct RefSlot {
    obj: Obj,
    kind: RefKind,
}

#[derive(Default)]
pub(crate) struct ThreadState {
    pub pending: Option<Obj>,
    pub locals: Vec<usize>,
    pub marks: Vec<usize>,
}

/// A guest buffer lent out by `Get*Chars/Elements`.
pub(crate) struct Lent {
    pub obj: Obj,
    pub len: u32,
}

#[derive(Default)]
pub(crate) struct State {
    pub objs: Vec<Option<Object>>,
    pub free_objs: Vec<u32>,
    pub classes: Vec<ClassDef>,
    pub by_name: HashMap<String, ClassId>,
    pub methods: Vec<MethodDef>,
    pub fields: Vec<FieldDef>,
    refs: Vec<Option<RefSlot>>,
    free_refs: Vec<usize>,
    pub threads: HashMap<usize, ThreadState>,
    pub lent: HashMap<u32, Lent>,
    pub pins: Vec<Obj>,
    pub envs: HashMap<usize, u32>,
    pub loopers: HashMap<usize, Obj>,
    pub main_looper: Option<Obj>,
    pub symbols: Option<SymbolFn>,
    pub log: Option<LogFn>,
    /// Log every Java call (bring-up aid).
    pub trace_calls: bool,
    /// UIKit views whose native `layoutSubviews` is due (see `Vm::run_layout`).
    pub dirty_views: Vec<Obj>,
    /// `makeKeyAndOrderFront` was called: the window gains focus on the next layout pass.
    pub focus_pending: bool,
    /// `AlertView` contents being built / shown (see `Vm::take_alerts`).
    pub alerts: Vec<Alert>,
    pub shown_alerts: Vec<Alert>,
    pub stubbed_classes: Vec<String>,
    /// Named long-lived objects of the fake library (class loader, locale, display, ...).
    pub singletons: HashMap<String, Obj>,
    pub thread_objs: HashMap<usize, Obj>,
    pub ticks: Vec<Obj>,
    /// The `TextField`/`TextView` that is first responder (receives the keyboard), see `Vm::text_focus`.
    pub text_focus: Option<Obj>,
    /// UIKit view tree (see `ui`).
    pub ui: crate::ui::UiState,
    /// Fonts and the last frame of the view renderer.
    pub render: crate::render::RenderCache,
    /// The shown input dialog of an alert with a text field (see `render::dialog`).
    pub dialog: Option<crate::render::dialog::Dialog>,
}

/// Resolver for exported native symbols: name -> guest address.
pub type SymbolFn = Arc<dyn Fn(&str) -> Option<u32> + Send + Sync>;
/// Diagnostics sink.
pub type LogFn = Arc<dyn Fn(&str) + Send + Sync>;

/// Anything that can hand out guest memory (the fake libc heap, a dedicated arena, ...).
pub trait GuestAlloc: Send {
    /// A block of `size` bytes, or 0 when out of memory.
    fn alloc(&mut self, size: u32) -> u32;
    fn free(&mut self, ptr: u32);
}

impl GuestAlloc for bhpc_mem::GuestHeap {
    fn alloc(&mut self, size: u32) -> u32 {
        bhpc_mem::GuestHeap::alloc(self, size)
    }
    fn free(&mut self, ptr: u32) {
        bhpc_mem::GuestHeap::free(self, ptr)
    }
}

/// Construction parameters of a [`Vm`].
pub struct VmConfig {
    pub platform: Platform,
    pub assets: Box<dyn Assets>,
    /// Unknown classes/members are invented (and reported) instead of throwing
    /// `NoClassDefFoundError`/`NoSuchMethodError`. Needed for boot: see the crate docs.
    pub lenient: bool,
    /// Milliseconds since an arbitrary start (`SystemClock.uptimeMillis`, `Looper` timers).
    pub clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    /// Display/EGL callbacks of `VerdeActivity` (swap buffers, contexts, content size).
    pub gfx: Arc<dyn GfxHooks>,
    /// `android.media.AudioTrack` back end.
    pub audio: Arc<dyn AudioHooks>,
}

impl VmConfig {
    pub fn new(platform: Platform, assets: Box<dyn Assets>) -> VmConfig {
        let start = std::time::Instant::now();
        VmConfig {
            platform,
            assets,
            lenient: true,
            clock: Arc::new(move || start.elapsed().as_millis() as u64),
            gfx: Arc::new(NoGfx),
            audio: Arc::new(NoAudio),
        }
    }
}

pub(crate) struct Inner {
    pub mem: Arc<GuestMemory>,
    st: Mutex<State>,
    alloc: Mutex<Box<dyn GuestAlloc>>,
    pub platform: Platform,
    pub assets: Box<dyn Assets>,
    pub lenient: bool,
    pub clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    pub gfx: Arc<dyn GfxHooks>,
    pub audio: Arc<dyn AudioHooks>,
    pub layout: OnceLock<crate::jni::Layout>,
    /// The display size the game sees (`getScreenWidth/Height`, display metrics); follows the host window.
    screen: [AtomicI32; 2],
    /// `VerdeActivity.setContentSize` as `f32` bits (0 = not announced yet).
    content: [AtomicU32; 2],
}

/// The fake JVM. Clone freely: it is a handle.
#[derive(Clone)]
pub struct Vm(pub(crate) Arc<Inner>);

pub const OBJECT: &str = "java/lang/Object";

impl Vm {
    /// An empty VM (no classes at all: see [`crate::new_vm`] for one with the built-in library).
    pub fn bare(mem: Arc<GuestMemory>, alloc: Box<dyn GuestAlloc>, cfg: VmConfig) -> Vm {
        let inner = Inner {
            mem,
            st: Mutex::new(State::default()),
            alloc: Mutex::new(alloc),
            platform: cfg.platform,
            assets: cfg.assets,
            lenient: cfg.lenient,
            clock: cfg.clock,
            gfx: cfg.gfx,
            audio: cfg.audio,
            layout: OnceLock::new(),
            screen: [AtomicI32::new(0), AtomicI32::new(0)],
            content: [AtomicU32::new(0), AtomicU32::new(0)],
        };
        let (w, h) = (inner.platform.screen_width, inner.platform.screen_height);
        inner.screen[0].store(w, Ordering::SeqCst);
        inner.screen[1].store(h, Ordering::SeqCst);
        let vm = Vm(Arc::new(inner));
        // Slot 0 of every table is reserved so that ids are never 0.
        {
            let mut st = vm.st();
            st.objs.push(None);
            st.methods.push(MethodDef {
                class: ClassId(0),
                name: String::new(),
                sig_str: "()V".into(),
                sig: Sig { params: vec![], ret: Ty::Void },
                is_static: false,
                imp: Imp::Stub,
                looked_up: false,
                calls: 0,
            });
            st.fields.push(FieldDef {
                class: ClassId(0),
                name: String::new(),
                desc: "I".into(),
                ty: Ty::Int,
                value: Value::Int(0),
                looked_up: false,
            });
            st.refs.push(None);
        }
        vm
    }

    pub(crate) fn st(&self) -> MutexGuard<'_, State> {
        self.0.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn platform(&self) -> &Platform {
        &self.0.platform
    }

    /// The display size in pixels the game sees (starts as `Platform::screen_*`).
    pub fn screen_size(&self) -> (i32, i32) {
        (self.0.screen[0].load(Ordering::SeqCst), self.0.screen[1].load(Ordering::SeqCst))
    }

    /// The host window changed size: later `getScreenWidth/Height` and display metrics calls see the new size.
    pub fn set_screen_size(&self, width: i32, height: i32) {
        self.0.screen[0].store(width, Ordering::SeqCst);
        self.0.screen[1].store(height, Ordering::SeqCst);
    }

    /// The content size the game announced with `setContentSize`, if it did.
    pub fn content_size(&self) -> Option<(f32, f32)> {
        let (w, h) = (self.0.content[0].load(Ordering::SeqCst), self.0.content[1].load(Ordering::SeqCst));
        (w != 0 && h != 0).then(|| (f32::from_bits(w), f32::from_bits(h)))
    }

    pub(crate) fn set_content_size(&self, w: f32, h: f32) {
        self.0.content[0].store(w.to_bits(), Ordering::SeqCst);
        self.0.content[1].store(h.to_bits(), Ordering::SeqCst);
    }

    /// `getContentWidth/Height`: the announced content size, else the screen size.
    pub fn content_pixels(&self) -> (i32, i32) {
        self.content_size().map_or_else(|| self.screen_size(), |(w, h)| (w.round() as i32, h.round() as i32))
    }

    /// Screen pixels per content pixel on each axis (the Java views are laid out in screen pixels, the game
    /// draws and receives touches in content pixels); `(1, 1)` until the content size is known.
    pub fn content_scale(&self) -> (f32, f32) {
        let (sw, sh) = self.screen_size();
        match self.content_size() {
            Some((cw, ch)) if cw > 0.0 && ch > 0.0 => (sw as f32 / cw, sh as f32 / ch),
            _ => (1.0, 1.0),
        }
    }

    pub fn assets(&self) -> &dyn Assets {
        &*self.0.assets
    }

    pub fn mem(&self) -> &Arc<GuestMemory> {
        &self.0.mem
    }

    /// Milliseconds on the VM clock.
    pub fn now_ms(&self) -> u64 {
        (self.0.clock)()
    }

    /// Install a sink for diagnostics (unknown classes/members, `android.util.Log`).
    pub fn set_log(&self, f: impl Fn(&str) + Send + Sync + 'static) {
        self.st().log = Some(Arc::new(f));
    }

    pub(crate) fn log(&self, msg: &str) {
        let f = self.st().log.clone();
        if let Some(f) = f {
            f(msg);
        }
    }

    /// Resolver for `Java_*` exports and similar: name -> guest address.
    pub fn set_symbol_resolver(&self, f: impl Fn(&str) -> Option<u32> + Send + Sync + 'static) {
        self.st().symbols = Some(Arc::new(f));
    }

    // ---------------------------------------------------------------- guest memory

    pub(crate) fn galloc(&self, size: u32) -> u32 {
        self.0.alloc.lock().unwrap_or_else(PoisonError::into_inner).alloc(size)
    }

    pub(crate) fn gfree(&self, ptr: u32) {
        self.0.alloc.lock().unwrap_or_else(PoisonError::into_inner).free(ptr)
    }

    /// Allocate and fill a guest block (0 on failure).
    pub(crate) fn galloc_bytes(&self, data: &[u8], extra: u32) -> u32 {
        let p = self.galloc(data.len() as u32 + extra);
        if p != 0 && self.0.mem.write_bytes(p, data).is_err() {
            self.gfree(p);
            return 0;
        }
        p
    }

    // ---------------------------------------------------------------- classes

    /// Look up a class by internal (`a/b/C`) or dotted name. Array classes are created on demand.
    pub fn class_named(&self, name: &str) -> Option<ClassId> {
        let name = name.replace('.', "/");
        let mut st = self.st();
        st.class_named(&name)
    }

    /// `FindClass` semantics: a missing class is invented (lenient) or a `Throw`.
    pub fn find_class(&self, name: &str) -> Result<ClassId, Throw> {
        if let Some(c) = self.class_named(name) {
            return Ok(c);
        }
        let name = name.replace('.', "/");
        if !self.0.lenient {
            return Err(self.throw_new("java/lang/NoClassDefFoundError", &name));
        }
        self.log(&format!("jni: stub class {name}"));
        let mut st = self.st();
        let id = st.ensure_class(&name);
        st.classes[id.0 as usize].stub = true;
        st.classes[id.0 as usize].defined = true;
        st.stubbed_classes.push(name);
        Ok(id)
    }

    pub fn class_name(&self, c: ClassId) -> String {
        self.st().classes[c.0 as usize].name.clone()
    }

    pub fn superclass(&self, c: ClassId) -> Option<ClassId> {
        self.st().classes[c.0 as usize].sup
    }

    /// `sub instanceof sup` on the class level.
    pub fn is_assignable(&self, sub: ClassId, sup: ClassId) -> bool {
        self.st().assignable(sub, sup)
    }

    /// The `java.lang.Class` object of `c` (created once).
    pub fn class_object(&self, c: ClassId) -> Obj {
        let mut st = self.st();
        if let Some(m) = st.classes[c.0 as usize].mirror {
            return m;
        }
        let cls = st.ensure_class("java/lang/Class");
        let m = st.new_obj(cls, Data::Class(c));
        st.classes[c.0 as usize].mirror = Some(m);
        m
    }

    /// The class a `java.lang.Class` object stands for.
    pub fn class_of_mirror(&self, o: Obj) -> Option<ClassId> {
        match self.st().obj(o)?.data {
            Data::Class(c) => Some(c),
            _ => None,
        }
    }

    // ---------------------------------------------------------------- objects

    pub fn class_of(&self, o: Obj) -> Option<ClassId> {
        self.st().obj(o).map(|x| x.class)
    }

    /// `instanceof` by class name.
    pub fn instance_of(&self, o: Obj, class: &str) -> bool {
        let mut st = self.st();
        let (Some(c), Some(k)) = (st.obj(o).map(|x| x.class), st.class_named(class)) else { return false };
        st.assignable(c, k)
    }

    /// A new instance without running a constructor (`AllocObject`).
    pub fn alloc_object(&self, class: ClassId) -> Obj {
        let mut st = self.st();
        let data = st.init_of(class).map_or(Data::None, |f| f());
        st.new_obj(class, data)
    }

    pub fn new_string(&self, s: &str) -> Obj {
        self.new_string_units(mutf8::units(s))
    }

    pub fn new_string_units(&self, u: Vec<u16>) -> Obj {
        let mut st = self.st();
        let c = st.ensure_class("java/lang/String");
        st.new_obj(c, Data::Str(u))
    }

    pub fn string(&self, o: Obj) -> Option<String> {
        self.string_units(o).map(|u| mutf8::to_string(&u))
    }

    pub fn string_units(&self, o: Obj) -> Option<Vec<u16>> {
        match &self.st().obj(o)?.data {
            Data::Str(u) => Some(u.clone()),
            _ => None,
        }
    }

    /// Create an object with the given payload and class name.
    pub fn new_data_object(&self, class: &str, data: Data) -> Obj {
        let mut st = self.st();
        let c = st.ensure_class(class);
        st.new_obj(c, data)
    }

    pub fn data(&self, o: Obj) -> Option<Data> {
        self.st().obj(o).map(|x| x.data.clone())
    }

    /// Run `f` on the payload of `o` (None if `o` is not a live object).
    pub fn with_data<R>(&self, o: Obj, f: impl FnOnce(&mut Data) -> R) -> Option<R> {
        let mut st = self.st();
        st.obj_mut(o).map(|x| f(&mut x.data))
    }

    pub fn new_prim_array(&self, ty: Ty, len: usize) -> Obj {
        let mut st = self.st();
        let c = st.ensure_class(&format!("[{}", ty.desc()));
        st.new_obj(c, Data::Prim(ty, vec![0; len * ty.size()]))
    }

    pub fn new_byte_array(&self, bytes: &[u8]) -> Obj {
        let mut st = self.st();
        let c = st.ensure_class("[B");
        st.new_obj(c, Data::Prim(Ty::Byte, bytes.to_vec()))
    }

    pub fn new_object_array(&self, elem: &str, len: usize) -> Obj {
        let mut st = self.st();
        let name = if elem.starts_with('[') { format!("[{elem}") } else { format!("[L{elem};") };
        let c = st.ensure_class(&name);
        st.new_obj(c, Data::Objs(vec![None; len]))
    }

    /// A `String[]` from Rust strings.
    pub fn new_string_array(&self, items: &[String]) -> Obj {
        let strs: Vec<Option<Obj>> = items.iter().map(|s| Some(self.new_string(s))).collect();
        let arr = self.new_object_array("java/lang/String", 0);
        self.with_data(arr, |d| *d = Data::Objs(strs));
        arr
    }

    /// Element count of any array.
    pub fn array_len(&self, o: Obj) -> Option<usize> {
        match &self.st().obj(o)?.data {
            Data::Prim(t, b) => Some(b.len() / t.size()),
            Data::Objs(v) => Some(v.len()),
            _ => None,
        }
    }

    /// Box a primitive: `Integer`, `Long`, `Boolean`, `Float`, `Double`.
    pub fn box_value(&self, v: Value, ty: Ty) -> Option<Obj> {
        let (class, v) = match (ty, v) {
            (Ty::Bool, v) => ("java/lang/Boolean", Value::Int(v.as_int())),
            (Ty::Byte, v) => ("java/lang/Byte", Value::Int(v.as_int())),
            (Ty::Char, v) => ("java/lang/Character", Value::Int(v.as_int())),
            (Ty::Short, v) => ("java/lang/Short", Value::Int(v.as_int())),
            (Ty::Int, v) => ("java/lang/Integer", Value::Int(v.as_int())),
            (Ty::Long, v) => ("java/lang/Long", Value::Long(v.as_long())),
            (Ty::Float, v) => ("java/lang/Float", Value::Float(v.as_float())),
            (Ty::Double, v) => ("java/lang/Double", Value::Double(v.as_double())),
            _ => return None,
        };
        let o = self.alloc_object(self.find_class(class).ok()?);
        self.set_field(o, "value", v);
        Some(o)
    }

    /// The primitive inside a box (`value` field), or `None` for non-boxes.
    pub fn unbox(&self, o: Obj) -> Option<Value> {
        let st = self.st();
        let cls = st.obj(o)?.class;
        let boxes = ["Boolean", "Byte", "Character", "Short", "Integer", "Long", "Float", "Double"];
        let name = &st.classes[cls.0 as usize].name;
        let simple = name.strip_prefix("java/lang/")?;
        if !boxes.contains(&simple) {
            return None;
        }
        let fid = st.field_in_chain(cls, "value")?;
        Some(st.obj(o)?.fields.get(&fid).copied().unwrap_or(st.fields[fid as usize].ty.default_value()))
    }

    /// Java `equals` for the types the fake library produces: identity, strings, boxes.
    pub fn equals(&self, a: Option<Obj>, b: Option<Obj>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(x), Some(y)) if x == y => true,
            (Some(x), Some(y)) => {
                if let (Some(s), Some(t)) = (self.string_units(x), self.string_units(y)) {
                    return s == t;
                }
                match (self.unbox(x), self.unbox(y)) {
                    (Some(p), Some(q)) => self.class_of(x) == self.class_of(y) && p == q,
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// A long-lived named object, created by `make` on first use and kept alive by the VM.
    pub fn singleton(&self, name: &str, make: impl FnOnce(&Vm) -> Obj) -> Obj {
        if let Some(o) = self.st().singletons.get(name) {
            return *o;
        }
        let o = make(self);
        self.st().singletons.insert(name.to_string(), o);
        o
    }

    // ---------------------------------------------------------------- fields

    /// Declare (or find) the field `name` of `class`'s chain; used by the host to read/write
    /// fields without going through JNI.
    pub fn field_id(&self, class: ClassId, name: &str, desc: &str) -> u32 {
        self.st().ensure_field(class, name, desc)
    }

    /// Set an instance field by name (declares it as `desc` of the value's kind if unknown).
    pub fn set_field(&self, o: Obj, name: &str, v: Value) {
        let mut st = self.st();
        let Some(cls) = st.obj(o).map(|x| x.class) else { return };
        let fid = st.field_in_chain(cls, name).unwrap_or_else(|| {
            let desc = match v {
                Value::Long(_) => "J",
                Value::Float(_) => "F",
                Value::Double(_) => "D",
                Value::Ref(_) => "Ljava/lang/Object;",
                _ => "I",
            };
            st.ensure_field(cls, name, desc)
        });
        if let Some(x) = st.obj_mut(o) {
            x.fields.insert(fid, v);
        }
    }

    /// Read an instance field by name (`Value::Int(0)`-like zero if unset or unknown).
    pub fn get_field(&self, o: Obj, name: &str) -> Value {
        let st = self.st();
        let Some(x) = st.obj(o) else { return Value::Int(0) };
        let Some(fid) = st.field_in_chain(x.class, name) else { return Value::Int(0) };
        x.fields.get(&fid).copied().unwrap_or(st.fields[fid as usize].ty.default_value())
    }

    pub fn get_static(&self, class: ClassId, name: &str) -> Value {
        let st = self.st();
        match st.field_in_chain(class, name) {
            Some(f) => st.fields[f as usize].value,
            None => Value::Int(0),
        }
    }

    pub fn set_static(&self, class: ClassId, name: &str, v: Value) {
        let mut st = self.st();
        let f = st.field_in_chain(class, name).unwrap_or_else(|| st.ensure_field(class, name, "I"));
        st.fields[f as usize].value = v;
    }

    // ---------------------------------------------------------------- references

    /// Create a reference of the given kind and return its guest handle (0 for null).
    pub(crate) fn new_ref(&self, pid: usize, o: Option<Obj>, kind: RefKind) -> u32 {
        let Some(o) = o else { return 0 };
        let mut st = self.st();
        let slot = RefSlot { obj: o, kind };
        let idx = match st.free_refs.pop() {
            Some(i) => {
                st.refs[i] = Some(slot);
                i
            }
            None => {
                st.refs.push(Some(slot));
                st.refs.len() - 1
            }
        };
        if kind == RefKind::Local {
            st.threads.entry(pid).or_default().locals.push(idx);
        }
        (idx as u32) << 3 | kind as u32
    }

    /// The object a handle refers to (None for 0, stale or unknown handles).
    pub fn resolve(&self, handle: u32) -> Option<Obj> {
        let idx = (handle >> 3) as usize;
        let st = self.st();
        match st.refs.get(idx)? {
            Some(s) if s.kind as u32 == handle & 7 => Some(s.obj),
            _ => None,
        }
    }

    /// `GetObjectRefType`: 0 invalid, 1 local, 2 global, 3 weak.
    pub(crate) fn ref_type(&self, handle: u32) -> u32 {
        let idx = (handle >> 3) as usize;
        match self.st().refs.get(idx) {
            Some(Some(s)) if s.kind as u32 == handle & 7 => s.kind as u32,
            _ => 0,
        }
    }

    pub(crate) fn delete_ref(&self, pid: usize, handle: u32) {
        let idx = (handle >> 3) as usize;
        let mut st = self.st();
        let ok = matches!(st.refs.get(idx), Some(Some(s)) if s.kind as u32 == handle & 7);
        if !ok {
            return;
        }
        st.refs[idx] = None;
        st.free_refs.push(idx);
        if let Some(t) = st.threads.get_mut(&pid) {
            t.locals.retain(|i| *i != idx);
        }
    }

    /// Open a local reference frame on this thread.
    pub fn push_frame(&self, pid: usize) {
        let mut st = self.st();
        let t = st.threads.entry(pid).or_default();
        t.marks.push(t.locals.len());
    }

    /// Close the innermost frame, releasing its local references. `keep` (if any) is returned as
    /// a fresh local reference of the enclosing frame.
    pub(crate) fn pop_frame(&self, pid: usize, keep: Option<Obj>) -> u32 {
        {
            let mut st = self.st();
            // An unbalanced pop (no frame open) releases nothing.
            let t = st.threads.entry(pid).or_default();
            let at = t.marks.pop().map_or(t.locals.len(), |m| m.min(t.locals.len()));
            let drop: Vec<usize> = t.locals.split_off(at);
            for idx in drop {
                st.refs[idx] = None;
                st.free_refs.push(idx);
            }
        }
        self.new_ref(pid, keep, RefKind::Local)
    }

    /// Number of live references of any kind (diagnostics, leak tests).
    pub fn live_refs(&self) -> usize {
        self.st().refs.iter().filter(|r| r.is_some()).count()
    }

    /// Keep `o` alive across [`Vm::gc`] until the VM is dropped.
    pub fn pin(&self, o: Obj) {
        self.st().pins.push(o);
    }

    /// Mark-and-sweep. Roots: live references, pins, static fields, loopers. Call it only when
    /// no host code holds an unreferenced `Obj` (for example between frames).
    pub fn gc(&self) -> usize {
        self.st().collect()
    }

    pub fn live_objects(&self) -> usize {
        self.st().objs.iter().filter(|o| o.is_some()).count()
    }

    // ---------------------------------------------------------------- exceptions

    /// Create an exception object of `class` with `message` (the class is invented if unknown).
    pub fn new_exception(&self, class: &str, message: &str) -> Obj {
        let c = self.find_class(class).unwrap_or(ClassId(0));
        let o = self.alloc_object(c);
        let m = self.new_string(message);
        self.set_field(o, "detailMessage", Value::Ref(Some(m)));
        o
    }

    pub fn throw_new(&self, class: &str, message: &str) -> Throw {
        Throw::Exc(self.new_exception(class, message))
    }

    /// `Throwable.getMessage()` of an exception object.
    pub fn exception_message(&self, e: Obj) -> Option<String> {
        self.get_field(e, "detailMessage").as_ref().and_then(|m| self.string(m))
    }

    pub(crate) fn set_pending(&self, pid: usize, e: Obj) {
        self.st().threads.entry(pid).or_default().pending = Some(e);
    }

    pub(crate) fn pending(&self, pid: usize) -> Option<Obj> {
        self.st().threads.get(&pid).and_then(|t| t.pending)
    }

    /// True while any guest thread has an exception pending.
    pub fn exception_pending(&self) -> bool {
        self.st().threads.values().any(|t| t.pending.is_some())
    }

    pub(crate) fn take_pending(&self, pid: usize) -> Option<Obj> {
        self.st().threads.get_mut(&pid).and_then(|t| t.pending.take())
    }

    // ---------------------------------------------------------------- members

    /// Resolve a method by walking the class chain (superclasses, then interfaces). In lenient
    /// mode a missing member is invented as a [`Imp::Stub`] and logged; otherwise a Java
    /// `NoSuchMethodError` is returned.
    pub fn method_id(&self, class: ClassId, name: &str, sig: &str, is_static: bool) -> Result<u32, Throw> {
        let Some(parsed) = parse_sig(sig) else {
            return Err(self.throw_new("java/lang/NoSuchMethodError", &format!("{name}{sig}")));
        };
        let mut st = self.st();
        if let Some(m) = st.method_in_chain(class, name, sig) {
            st.methods[m as usize].looked_up = true;
            return Ok(m);
        }
        if !self.0.lenient {
            let cn = st.classes[class.0 as usize].name.clone();
            drop(st);
            return Err(self.throw_new("java/lang/NoSuchMethodError", &format!("{cn}.{name}{sig}")));
        }
        let cn = st.classes[class.0 as usize].name.clone();
        let id = st.add_method(class, name, sig, parsed, is_static, Imp::Stub);
        st.methods[id as usize].looked_up = true;
        drop(st);
        self.log(&format!("jni: stub method {cn}.{name}{sig}"));
        Ok(id)
    }

    /// Resolve a field like [`Vm::method_id`].
    pub fn jfield_id(&self, class: ClassId, name: &str, desc: &str) -> Result<u32, Throw> {
        let mut st = self.st();
        if let Some(f) = st.field_in_chain(class, name) {
            st.fields[f as usize].looked_up = true;
            return Ok(f);
        }
        let cn = st.classes[class.0 as usize].name.clone();
        if !self.0.lenient || parse_field_ty(desc).is_none() {
            drop(st);
            return Err(self.throw_new("java/lang/NoSuchFieldError", &format!("{cn}.{name}")));
        }
        let id = st.ensure_field(class, name, desc);
        st.fields[id as usize].looked_up = true;
        drop(st);
        self.log(&format!("jni: stub field {cn}.{name} {desc}"));
        Ok(id)
    }

    pub(crate) fn method_info(&self, mid: u32) -> Option<(ClassId, bool, Sig, String, String)> {
        let st = self.st();
        let m = st.methods.get(mid as usize).filter(|_| mid != 0)?;
        Some((m.class, m.is_static, m.sig.clone(), m.name.clone(), m.sig_str.clone()))
    }

    /// Bind a guest function to `class.name sig` (`RegisterNatives`). The method is created if the
    /// fake class does not declare it.
    pub fn register_native(&self, class: ClassId, name: &str, sig: &str, addr: u32) -> bool {
        let Some(parsed) = parse_sig(sig) else { return false };
        let mut st = self.st();
        match st.classes[class.0 as usize].methods.get(&(name.to_string(), sig.to_string())).copied() {
            Some(m) => {
                st.methods[m as usize].imp = Imp::Native(addr);
                st.methods[m as usize].looked_up = true;
            }
            None => {
                let m = st.add_method(class, name, sig, parsed, false, Imp::Native(addr));
                st.methods[m as usize].looked_up = true;
            }
        }
        true
    }

    /// The guest address bound to `class.name sig`, if it is a native method.
    pub fn native_addr(&self, class: ClassId, name: &str, sig: &str) -> Option<u32> {
        let st = self.st();
        let m = st.method_in_chain(class, name, sig)?;
        match st.methods[m as usize].imp {
            Imp::Native(a) => Some(a),
            _ => None,
        }
    }

    // ---------------------------------------------------------------- reports

    /// Everything that was looked up or invented so far (for the bring-up trace).
    pub fn report(&self) -> Report {
        let st = self.st();
        let mut r = Report { stub_classes: st.stubbed_classes.clone(), ..Report::default() };
        for m in st.methods.iter().skip(1).filter(|m| m.looked_up || m.calls > 0) {
            let kind = match m.imp {
                Imp::Host(_) => Kind::Host,
                Imp::Native(_) => Kind::Native,
                Imp::Stub => Kind::Stub,
            };
            r.members.push(MemberReport {
                class: st.classes[m.class.0 as usize].name.clone(),
                name: m.name.clone(),
                sig: m.sig_str.clone(),
                is_static: m.is_static,
                kind,
                calls: m.calls,
            });
        }
        for f in st.fields.iter().skip(1).filter(|f| f.looked_up) {
            r.fields.push((st.classes[f.class.0 as usize].name.clone(), f.name.clone(), f.desc.clone()));
        }
        r.members.sort_by(|a, b| (&a.class, &a.name, &a.sig).cmp(&(&b.class, &b.name, &b.sig)));
        r
    }
}

/// A UIKit alert the game built: title, message and buttons `(title, index)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    /// The Java `AlertView` (its `mObject` is what the native click callbacks take).
    pub view: Obj,
    pub title: String,
    pub message: String,
    pub buttons: Vec<(String, i32)>,
    /// Title and index of the cancel button, if any.
    pub cancel: Option<(String, i32)>,
    /// The `TextField` of an alert with a text input line (`addSubview` of a text field).
    pub field: Option<Obj>,
}

/// How a looked-up member is implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Host,
    Native,
    Stub,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberReport {
    pub class: String,
    pub name: String,
    pub sig: String,
    pub is_static: bool,
    pub kind: Kind,
    /// Times the member was invoked.
    pub calls: u64,
}

/// Result of [`Vm::report`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Classes invented because nothing defines them.
    pub stub_classes: Vec<String>,
    /// Every method looked up through JNI or invoked, sorted.
    pub members: Vec<MemberReport>,
    /// `(class, name, descriptor)` of every field looked up through JNI.
    pub fields: Vec<(String, String, String)>,
}

impl Report {
    /// Members that are only stubs (the work list for the next iteration).
    pub fn stubs(&self) -> impl Iterator<Item = &MemberReport> {
        self.members.iter().filter(|m| m.kind == Kind::Stub)
    }

    /// Human-readable listing, one line per member.
    pub fn render(&self) -> String {
        let mut s = String::new();
        for c in &self.stub_classes {
            s.push_str(&format!("class-stub {c}\n"));
        }
        for m in &self.members {
            let k = match m.kind {
                Kind::Host => "host",
                Kind::Native => "native",
                Kind::Stub => "STUB",
            };
            let st = if m.is_static { "static " } else { "" };
            s.push_str(&format!("{k:6} {st}{}.{}{} x{}\n", m.class, m.name, m.sig, m.calls));
        }
        for (c, n, d) in &self.fields {
            s.push_str(&format!("field  {c}.{n} {d}\n"));
        }
        s
    }
}

// -------------------------------------------------------------------- State internals

impl State {
    pub(crate) fn obj(&self, o: Obj) -> Option<&Object> {
        self.objs.get(o.0 as usize).and_then(Option::as_ref).filter(|_| o != Obj::NULL)
    }

    pub(crate) fn obj_mut(&mut self, o: Obj) -> Option<&mut Object> {
        self.objs.get_mut(o.0 as usize).and_then(Option::as_mut).filter(|_| o != Obj::NULL)
    }

    /// Field `name` of `o` (declared default if unset), or None if unknown.
    pub(crate) fn field_named(&self, o: Obj, name: &str) -> Option<Value> {
        let x = self.obj(o)?;
        let f = self.field_in_chain(x.class, name)?;
        Some(x.fields.get(&f).copied().unwrap_or(self.fields[f as usize].ty.default_value()))
    }

    pub(crate) fn new_obj(&mut self, class: ClassId, data: Data) -> Obj {
        let ob = Object { class, data, fields: HashMap::new() };
        match self.free_objs.pop() {
            Some(i) => {
                self.objs[i as usize] = Some(ob);
                Obj(i)
            }
            None => {
                self.objs.push(Some(ob));
                Obj(self.objs.len() as u32 - 1)
            }
        }
    }

    /// Existing class or an (undefined) placeholder; arrays are defined on the spot.
    pub(crate) fn ensure_class(&mut self, name: &str) -> ClassId {
        if let Some(c) = self.by_name.get(name) {
            return *c;
        }
        let sup = if name == OBJECT { None } else { Some(self.ensure_class(OBJECT)) };
        let id = ClassId(self.classes.len() as u32);
        let is_array = name.starts_with('[');
        self.classes.push(ClassDef {
            name: name.to_string(),
            sup,
            ifaces: Vec::new(),
            interface: false,
            defined: is_array,
            stub: false,
            methods: HashMap::new(),
            fields: HashMap::new(),
            init: None,
            mirror: None,
        });
        self.by_name.insert(name.to_string(), id);
        id
    }

    /// Like [`State::ensure_class`] but only for defined classes (and arrays).
    pub(crate) fn class_named(&mut self, name: &str) -> Option<ClassId> {
        if name.starts_with('[') {
            return Some(self.ensure_class(name));
        }
        self.by_name.get(name).copied().filter(|c| self.classes[c.0 as usize].defined)
    }

    /// Payload constructor of `class` or its nearest superclass that has one.
    pub(crate) fn init_of(&self, class: ClassId) -> Option<fn() -> Data> {
        let c = &self.classes[class.0 as usize];
        c.init.or_else(|| c.sup.and_then(|s| self.init_of(s)))
    }

    pub(crate) fn assignable(&self, sub: ClassId, sup: ClassId) -> bool {
        if sub == sup {
            return true;
        }
        let c = &self.classes[sub.0 as usize];
        c.sup.is_some_and(|s| self.assignable(s, sup)) || c.ifaces.iter().any(|i| self.assignable(*i, sup)) || {
            // Every reference type is an Object.
            self.classes[sup.0 as usize].name == OBJECT
        }
    }

    pub(crate) fn method_in_chain(&self, class: ClassId, name: &str, sig: &str) -> Option<u32> {
        let c = &self.classes[class.0 as usize];
        if let Some(m) = c.methods.get(&(name.to_string(), sig.to_string())) {
            return Some(*m);
        }
        let from_super = c.sup.and_then(|s| self.method_in_chain(s, name, sig));
        from_super.or_else(|| c.ifaces.iter().find_map(|i| self.method_in_chain(*i, name, sig)))
    }

    pub(crate) fn add_method(
        &mut self,
        class: ClassId,
        name: &str,
        sig: &str,
        parsed: Sig,
        is_static: bool,
        imp: Imp,
    ) -> u32 {
        let id = self.methods.len() as u32;
        self.methods.push(MethodDef {
            class,
            name: name.to_string(),
            sig_str: sig.to_string(),
            sig: parsed,
            is_static,
            imp,
            looked_up: false,
            calls: 0,
        });
        self.classes[class.0 as usize].methods.insert((name.to_string(), sig.to_string()), id);
        id
    }

    pub(crate) fn field_in_chain(&self, class: ClassId, name: &str) -> Option<u32> {
        let c = &self.classes[class.0 as usize];
        if let Some(f) = c.fields.get(name) {
            return Some(*f);
        }
        let from_super = c.sup.and_then(|s| self.field_in_chain(s, name));
        from_super.or_else(|| c.ifaces.iter().find_map(|i| self.field_in_chain(*i, name)))
    }

    pub(crate) fn ensure_field(&mut self, class: ClassId, name: &str, desc: &str) -> u32 {
        if let Some(f) = self.field_in_chain(class, name) {
            return f;
        }
        let ty = parse_field_ty(desc).unwrap_or(Ty::Int);
        let id = self.fields.len() as u32;
        self.fields.push(FieldDef {
            class,
            name: name.to_string(),
            desc: desc.to_string(),
            ty,
            value: ty.default_value(),
            looked_up: false,
        });
        self.classes[class.0 as usize].fields.insert(name.to_string(), id);
        id
    }

    fn children(o: &Object, out: &mut Vec<Obj>) {
        out.extend(o.fields.values().filter_map(Value::as_ref));
        match &o.data {
            Data::Objs(v) | Data::List(v) => out.extend(v.iter().flatten()),
            Data::Map(m) => {
                for (k, v) in m {
                    out.extend(k.iter().chain(v.iter()));
                }
            }
            Data::Queue(q) => out.extend(q.iter()),
            _ => {}
        }
    }

    /// Mark-and-sweep; returns the number of objects freed.
    pub(crate) fn collect(&mut self) -> usize {
        let mut work: Vec<Obj> = Vec::new();
        work.extend(self.refs.iter().flatten().map(|r| r.obj));
        work.extend(self.pins.iter());
        work.extend(self.loopers.values());
        work.extend(self.main_looper);
        work.extend(self.singletons.values());
        work.extend(self.thread_objs.values());
        work.extend(self.ticks.iter());
        work.extend(self.classes.iter().filter_map(|c| c.mirror));
        work.extend(self.fields.iter().filter_map(|f| f.value.as_ref()));
        work.extend(self.lent.values().map(|l| l.obj));
        work.extend(self.ui.views.values().flat_map(crate::ui::View::refs));
        work.extend(self.threads.values().filter_map(|t| t.pending));
        let mut marked = vec![false; self.objs.len()];
        while let Some(o) = work.pop() {
            let i = o.0 as usize;
            if i == 0 || i >= marked.len() || marked[i] {
                continue;
            }
            if let Some(ob) = &self.objs[i] {
                marked[i] = true;
                Self::children(ob, &mut work);
            }
        }
        let mut freed = 0;
        for (i, live) in marked.iter().enumerate().skip(1) {
            if !live && self.objs[i].is_some() {
                self.objs[i] = None;
                self.free_objs.push(i as u32);
                freed += 1;
            }
        }
        freed
    }
}

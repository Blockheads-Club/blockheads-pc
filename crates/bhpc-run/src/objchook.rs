//! Objective-C method hooks from the host: the swizzling primitive of the mod system's PC `GameBridge`.
//!
//! [`ObjcHooks::hook`] asks for a method (`-[Class selector]` or `+[Class selector]`) to run a host handler.
//! [`ObjcHooks::install`] does what an in-process Android loader does with the same exports of Apportable's
//! runtime (`libSystem.so`): `objc_getClass`/`objc_getMetaClass`, `sel_registerName`, `class_getInstanceMethod`,
//! `method_getTypeEncoding`, `method_getImplementation`, then `method_setImplementation(method, thunk)`, where
//! `thunk` is a host thunk stub. The old implementation is kept as the *original*.
//!
//! When the game sends the message, the runtime jumps to the thunk, which builds an [`Invocation`] (receiver,
//! `_cmd` and the arguments described by a [`Sig`]) and runs the handler under `catch_unwind`. The handler may
//!
//! - only look at (or change, [`Invocation::set_arg`]) the arguments: the original then runs as a tail call from
//!   the thunk stub, with no extra JIT and no nesting (a *before* hook, the cheapest kind);
//! - call [`Invocation::call_original`], which runs the original on the CPU's nested child JIT (see `bhpc_cpu`) and
//!   returns its result, to act *after* it or change its result;
//! - answer with [`Invocation::set_return`] without running the original (*replace*).
//!
//! # Rules
//!
//! - **No JIT flush.** No code bytes change: the IMP is data in the runtime's method structure, and
//!   `method_setImplementation` flushes the runtime's method caches itself. Works for ARM and Thumb IMPs alike.
//! - **Threads.** A hook runs on whatever guest thread sent the message, holding the host gate like every thunk
//!   (`Cpu::set_gate`): other guest threads that enter a thunk wait meanwhile, so handlers must be short and must
//!   never block on `std::sync` primitives or sleep (use `bhpc_bionic::sched` waits, or post work elsewhere). The
//!   registry lock is never held while guest code runs, so handlers may add hooks; installs must run on a guest
//!   thread's CPU (at top level or inside a thunk).
//! - **Panics.** A panicking handler cannot unwind through the JIT frames (that aborts the process), so every
//!   handler runs under `catch_unwind`. After a panic the call continues as if the hook were not there: the original
//!   runs with the unchanged arguments, or, if the handler had already called it, its result is returned. Panics are
//!   counted ([`HookStat::panics`]) and logged.
//! - **Signatures.** The handler's view of the arguments comes from a [`Sig`] (an Objective-C type encoding such as
//!   `v@:@@@`). It is checked against the method's own encoding from the runtime at install time: a hook whose
//!   signature does not match is refused instead of reading the wrong registers. Without an explicit signature the
//!   runtime's is used.
//!
//! # Classes that are not realised yet, and classes that do not exist yet
//!
//! `class_getInstanceMethod` realises the class (and its superclasses) if needed: Apportable's runtime is objc4-532
//! era, whose `class_getInstanceMethod` calls `lookUpMethod(cls, sel, initialize = NO, cache = YES)`. Realisation
//! attaches categories and fixes selectors up, exactly as the first message would, but `+initialize` is *not* sent;
//! it still runs at the first real message. So hooking a class the game has not used yet is safe and forces nothing
//! the game would observe. A class the runtime does not know yet (`objc_getClass` returns nil: its library is not
//! loaded or its image not registered) or a method it does not have yet (a category from a later library) leaves
//! the hook *waiting*; call [`ObjcHooks::install`] again later (`bhpc-run` retries once per second from the main
//! thread's frame hook). Install after the libraries' constructors: before that the runtime has no classes at all.
//!
//! # Limits
//!
//! - A hook sees the method of the class it was installed on. A subclass that overrides the selector is not hooked
//!   (hook the override too); a subclass that inherits it is.
//! - `call_original` nests at most `bhpc_cpu::MAX_NEST_DEPTH` deep; past that it fails with
//!   [`HookError::TooDeep`] and the call falls back to a before hook.
//! - Methods with variadic arguments, unions, bit fields or opaque structs cannot be described by a [`Sig`].
//! - Objective-C exceptions that unwind through a nested `call_original` stop at the child JIT's return address
//!   (the game does not throw through the methods hooked so far).

use bhpc_cpu::{CallResult, Cpu, Stop};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// How one argument or the result of a hooked method is passed (AAPCS, softfp: floating point in core registers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    /// Nothing (results only).
    Void,
    /// One 32-bit word: objects, classes, selectors, pointers, integers up to 32 bits, `BOOL`.
    Word,
    /// `float`.
    F32,
    /// A 64-bit integer, 8-byte aligned.
    Dword,
    /// `double`, 8-byte aligned.
    F64,
    /// A struct of `words` words passed by value (`align8`: it holds 64-bit members). As a result, a struct of more
    /// than one word is returned through memory (a hidden pointer before `self`).
    Struct { words: u8, align8: bool },
}

impl Ty {
    fn words(self) -> u32 {
        match self {
            Ty::Void => 0,
            Ty::Word | Ty::F32 => 1,
            Ty::Dword | Ty::F64 => 2,
            Ty::Struct { words, .. } => u32::from(words),
        }
    }

    fn align8(self) -> bool {
        matches!(self, Ty::Dword | Ty::F64 | Ty::Struct { align8: true, .. })
    }
}

/// A value read from or given to a hooked call.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Void,
    Word(u32),
    F32(f32),
    Dword(u64),
    F64(f64),
    /// A struct's words.
    Words(Vec<u32>),
}

impl Val {
    /// The value as a 32-bit word (`0` for values of another kind).
    pub fn word(&self) -> u32 {
        match self {
            Val::Word(w) => *w,
            _ => 0,
        }
    }

    fn from_words(ty: Ty, w: &[u32]) -> Val {
        let dword = || u64::from(w[0]) | u64::from(w[1]) << 32;
        match ty {
            Ty::Void => Val::Void,
            Ty::Word => Val::Word(w[0]),
            Ty::F32 => Val::F32(f32::from_bits(w[0])),
            Ty::Dword => Val::Dword(dword()),
            Ty::F64 => Val::F64(f64::from_bits(dword())),
            Ty::Struct { .. } => Val::Words(w.to_vec()),
        }
    }

    /// The words of this value for `ty`, or `None` if it is not a value of that type.
    fn to_words(&self, ty: Ty) -> Option<Vec<u32>> {
        let split = |v: u64| vec![v as u32, (v >> 32) as u32];
        match (ty, self) {
            (Ty::Void, Val::Void) => Some(Vec::new()),
            (Ty::Word, Val::Word(w)) => Some(vec![*w]),
            (Ty::F32, Val::F32(f)) => Some(vec![f.to_bits()]),
            (Ty::Dword, Val::Dword(v)) => Some(split(*v)),
            (Ty::F64, Val::F64(f)) => Some(split(f.to_bits())),
            (Ty::Struct { words, .. }, Val::Words(w)) if w.len() == usize::from(words) => Some(w.clone()),
            _ => None,
        }
    }
}

/// A method signature: result and arguments after `self` and `_cmd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sig {
    pub ret: Ty,
    pub args: Vec<Ty>,
}

/// Size, alignment and kind of a type inside an encoding.
#[derive(Debug, Clone, Copy)]
struct Raw {
    size: u32,
    align: u32,
    kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Void,
    Int,
    Float,
    Double,
    Struct,
}

impl Raw {
    fn ty(self) -> Ty {
        match (self.kind, self.size) {
            (Kind::Void, _) => Ty::Void,
            (Kind::Float, _) => Ty::F32,
            (Kind::Double, _) => Ty::F64,
            (Kind::Int, 8) => Ty::Dword,
            (Kind::Int, _) => Ty::Word,
            (Kind::Struct, s) => Ty::Struct { words: s.div_ceil(4) as u8, align8: self.align == 8 },
        }
    }
}

impl Sig {
    /// Parse an Objective-C method type encoding (`v@:@@@`, `{CGPoint=ff}16@0:4`, ...): the result, then `self` and
    /// `_cmd` (which must be an object/class and a selector), then the arguments. Offsets and qualifiers are
    /// skipped. Unions, bit fields, opaque structs and unknown codes are refused.
    pub fn parse(enc: &str) -> Result<Sig, String> {
        let mut p = Parser { s: enc.as_bytes(), i: 0 };
        let mut all = Vec::new();
        while p.skip_noise() {
            all.push(p.ty()?);
        }
        let err = || format!("bad method encoding {enc:?}");
        if all.len() < 3 || !matches!(all[1].ty(), Ty::Word) || !matches!(all[2].ty(), Ty::Word) {
            return Err(err());
        }
        let args: Vec<Ty> = all[3..].iter().map(|r| r.ty()).collect();
        if args.contains(&Ty::Void) || args.iter().any(|t| matches!(t, Ty::Struct { words: 0, .. })) {
            return Err(err());
        }
        Ok(Sig { ret: all[0].ty(), args })
    }
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    /// Skip offsets and qualifiers; true if a type follows.
    fn skip_noise(&mut self) -> bool {
        while let Some(c) = self.peek() {
            if !(c.is_ascii_digit() || b"rnNoORV".contains(&c)) {
                return true;
            }
            self.i += 1;
        }
        false
    }

    /// Skip a quoted name (`"NSString"`, a struct member name) if one follows.
    fn skip_quoted(&mut self) -> Result<(), String> {
        if self.peek() == Some(b'"') {
            let end = self.s[self.i + 1..].iter().position(|&c| c == b'"').ok_or("unterminated name")?;
            self.i += end + 2;
        }
        Ok(())
    }

    fn number(&mut self) -> u32 {
        let mut n = 0u32;
        while let Some(d) = self.peek().filter(u8::is_ascii_digit) {
            n = n.saturating_mul(10).saturating_add(u32::from(d - b'0'));
            self.i += 1;
        }
        n
    }

    fn ty(&mut self) -> Result<Raw, String> {
        let c = self.peek().ok_or("encoding ends early")?;
        self.i += 1;
        let raw = |size, kind| Ok(Raw { size, align: size, kind });
        match c {
            b'v' => Ok(Raw { size: 0, align: 1, kind: Kind::Void }),
            b'c' | b'C' | b'B' => raw(1, Kind::Int),
            b's' | b'S' => raw(2, Kind::Int),
            b'i' | b'I' | b'l' | b'L' | b'#' | b':' | b'*' => raw(4, Kind::Int),
            b'q' | b'Q' => raw(8, Kind::Int),
            b'f' => raw(4, Kind::Float),
            b'd' | b'D' => raw(8, Kind::Double),
            b'@' => {
                // `@?` is a block, `@"Name"` an object of a class.
                if self.peek() == Some(b'?') {
                    self.i += 1;
                }
                self.skip_quoted()?;
                raw(4, Kind::Int)
            }
            b'^' => {
                // The pointee may be anything (also `?`, `v`, an opaque struct).
                if self.peek() == Some(b'?') {
                    self.i += 1;
                } else {
                    self.skip_type()?;
                }
                raw(4, Kind::Int)
            }
            b'[' => {
                let n = self.number();
                let e = self.ty()?;
                self.expect(b']')?;
                Ok(Raw { size: e.size * n, align: e.align, kind: Kind::Struct })
            }
            b'{' => self.structure(),
            other => Err(format!("unsupported type code {:?}", other as char)),
        }
    }

    fn expect(&mut self, c: u8) -> Result<(), String> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected {:?}", c as char))
        }
    }

    /// `{Name=members}` (the opening brace is consumed).
    fn structure(&mut self) -> Result<Raw, String> {
        while !matches!(self.peek(), Some(b'=') | Some(b'}') | None) {
            self.i += 1;
        }
        self.expect(b'=').map_err(|_| "opaque struct".to_string())?;
        let (mut size, mut align) = (0u32, 1u32);
        while self.peek() != Some(b'}') {
            self.skip_quoted()?;
            let m = self.ty()?;
            let a = m.align.max(1);
            size = size.next_multiple_of(a) + m.size;
            align = align.max(a);
        }
        self.i += 1;
        Ok(Raw { size: size.next_multiple_of(align), align, kind: Kind::Struct })
    }

    /// Skip one type whose layout does not matter (a pointee): structs may be opaque here.
    fn skip_type(&mut self) -> Result<(), String> {
        if self.peek() == Some(b'{') {
            let mut depth = 0;
            loop {
                match self.peek().ok_or("unterminated struct")? {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
                self.i += 1;
                if depth == 0 {
                    return Ok(());
                }
            }
        }
        self.ty().map(|_| ())
    }
}

/// Where each argument word of a call lives: words 0-3 are r0-r3, word `4 + n` is the stack word at `sp + 4n`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Layout {
    sig: Sig,
    /// The result goes through memory: word 0 is the hidden result pointer.
    sret: bool,
    /// First word of `self`, `_cmd`, then each argument.
    starts: Vec<u32>,
    words: u32,
}

impl Layout {
    fn new(sig: Sig) -> Layout {
        let sret = matches!(sig.ret, Ty::Struct { words, .. } if words > 1);
        let mut at = u32::from(sret);
        let mut starts = Vec::new();
        for t in [Ty::Word, Ty::Word].iter().chain(&sig.args) {
            if t.align8() {
                at = (at + 1) & !1;
            }
            starts.push(at);
            at += t.words();
        }
        Layout { sig, sret, starts, words: at }
    }

    fn read(&self, c: &Cpu) -> Vec<u32> {
        (0..self.words)
            .map(|i| if i < 4 { c.reg(i as usize) } else { c.mem().read_u32(c.sp() + 4 * (i - 4)).unwrap_or(0) })
            .collect()
    }

    fn write(&self, c: &mut Cpu, words: &[u32]) {
        for (i, w) in words.iter().enumerate() {
            if i < 4 {
                c.set_reg(i, *w);
            } else {
                let _ = c.mem().write_u32(c.sp() + 4 * (i as u32 - 4), *w);
            }
        }
    }

    /// Words of argument `i` (0 = `self`, 1 = `_cmd`, 2.. = the signature's arguments) and its type.
    fn slot(&self, i: usize) -> Option<(std::ops::Range<usize>, Ty)> {
        let t = if i < 2 { Ty::Word } else { *self.sig.args.get(i - 2)? };
        let s = self.starts[i] as usize;
        Some((s..s + t.words() as usize, t))
    }
}

/// What a hooked method is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub class: String,
    pub sel: String,
    /// A class method (`+`).
    pub meta: bool,
}

impl Target {
    /// `-[Class selector]` or `+[Class selector]`.
    pub fn parse(s: &str) -> Option<Target> {
        let meta = match s.as_bytes().first()? {
            b'-' => false,
            b'+' => true,
            _ => return None,
        };
        let (class, sel) = s.get(1..)?.strip_prefix('[')?.strip_suffix(']')?.split_once(' ')?;
        (!class.is_empty() && !sel.is_empty() && !sel.contains(' ')).then(|| Target {
            class: class.into(),
            sel: sel.into(),
            meta,
        })
    }
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}[{} {}]", if self.meta { '+' } else { '-' }, self.class, self.sel)
    }
}

/// Guest addresses of the runtime functions the hooks use (exports of Apportable's `libSystem.so`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Runtime {
    pub get_class: u32,
    pub get_meta_class: u32,
    pub sel_register: u32,
    pub get_instance_method: u32,
    pub type_encoding: u32,
    pub get_imp: u32,
    pub set_imp: u32,
    pub msg_send: u32,
}

impl Runtime {
    /// Find the functions with `export` (a lookup over the loaded libraries).
    pub fn resolve(export: impl Fn(&str) -> Option<u32>) -> Result<Runtime, String> {
        let f = |n: &str| export(n).ok_or(format!("no {n} export"));
        Ok(Runtime {
            get_class: f("objc_getClass")?,
            get_meta_class: f("objc_getMetaClass")?,
            sel_register: f("sel_registerName")?,
            get_instance_method: f("class_getInstanceMethod")?,
            type_encoding: f("method_getTypeEncoding")?,
            get_imp: f("method_getImplementation")?,
            set_imp: f("method_setImplementation")?,
            msg_send: f("objc_msgSend")?,
        })
    }

    /// `sel_registerName(name)`.
    pub fn sel(&self, c: &mut Cpu, name: &str) -> Result<u32, Stop> {
        with_cstr(c, name, |c, s| c.call(self.sel_register, &[s])).map(|r| r.r0)
    }

    /// `objc_msgSend(obj, sel, args...)` (word arguments).
    pub fn send(&self, c: &mut Cpu, obj: u32, sel: u32, args: &[u32]) -> Result<CallResult, Stop> {
        let mut all = vec![obj, sel];
        all.extend_from_slice(args);
        c.call(self.msg_send, &all)
    }

    /// The class (or metaclass) named `name`, 0 if the runtime does not know it.
    pub(crate) fn class(&self, c: &mut Cpu, name: &str, meta: bool) -> Result<u32, Stop> {
        let f = if meta { self.get_meta_class } else { self.get_class };
        with_cstr(c, name, |c, s| c.call(f, &[s])).map(|r| r.r0)
    }
}

/// Run `f` with `s` as a C string on the guest stack below `sp` (the stack pointer moves past it for the call).
fn with_cstr<T>(c: &mut Cpu, s: &str, f: impl FnOnce(&mut Cpu, u32) -> Result<T, Stop>) -> Result<T, Stop> {
    let sp = c.sp();
    let at = (sp - s.len() as u32 - 1) & !7;
    let mut bytes = s.as_bytes().to_vec();
    bytes.push(0);
    c.mem().write_bytes(at, &bytes).map_err(|e| Stop::Host(format!("cannot write {s:?}: {e}")))?;
    c.set_sp(at);
    let r = f(c, at);
    c.set_sp(sp);
    r
}

/// Why a step of a hooked call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookError {
    /// No argument with this index.
    NoArg(usize),
    /// The value does not have the argument's or the result's type.
    Type,
    /// The original cannot run nested here (too deep): the call continues as a before hook.
    TooDeep,
    /// The original stopped the guest (a crash, an exit); the thread stops with it once the handler returns.
    Guest(Stop),
}

/// One call of a hooked method, as its handler sees it.
pub struct Invocation<'a> {
    cpu: &'a mut Cpu,
    layout: &'a Layout,
    orig: u32,
    words: Vec<u32>,
    dirty: bool,
    ret: Option<Val>,
    result: Option<CallResult>,
    stop: Option<Stop>,
}

impl Invocation<'_> {
    /// The CPU of the calling guest thread: guest memory, and guest calls (which run nested).
    pub fn cpu(&mut self) -> &mut Cpu {
        self.cpu
    }

    /// `self`.
    pub fn receiver(&self) -> u32 {
        self.words[self.layout.starts[0] as usize]
    }

    /// `_cmd`.
    pub fn selector(&self) -> u32 {
        self.words[self.layout.starts[1] as usize]
    }

    /// The original implementation.
    pub fn original(&self) -> u32 {
        self.orig
    }

    /// Argument `i` (0 = the first after `_cmd`).
    pub fn arg(&self, i: usize) -> Option<Val> {
        let (r, t) = self.layout.slot(i + 2)?;
        Some(Val::from_words(t, &self.words[r]))
    }

    /// Change argument `i` for the original (both when it runs after the handler and for `call_original`).
    pub fn set_arg(&mut self, i: usize, v: Val) -> Result<(), HookError> {
        let (r, t) = self.layout.slot(i + 2).ok_or(HookError::NoArg(i))?;
        let w = v.to_words(t).ok_or(HookError::Type)?;
        self.words[r].copy_from_slice(&w);
        self.dirty = true;
        Ok(())
    }

    /// Run the original now (on the nested child JIT) with the current arguments and return its result. The hooked
    /// call then returns that result unless the handler sets another one.
    pub fn call_original(&mut self) -> Result<Val, HookError> {
        let r = match self.cpu.call(self.orig, &self.words) {
            Ok(r) => r,
            Err(Stop::Nested) => return Err(HookError::TooDeep),
            Err(s) => {
                self.stop = Some(s.clone());
                return Err(HookError::Guest(s));
            }
        };
        self.result = Some(r);
        let ret = self.layout.sig.ret;
        Ok(if self.layout.sret {
            let words: Vec<u32> =
                (0..ret.words()).map(|i| self.cpu.mem().read_u32(self.words[0] + 4 * i).unwrap_or(0)).collect();
            Val::Words(words)
        } else {
            Val::from_words(ret, &[r.r0, r.r1][..ret.words() as usize])
        })
    }

    /// Return `v` from the hooked call. The original does not run afterwards (it may have run already through
    /// `call_original`).
    pub fn set_return(&mut self, v: Val) -> Result<(), HookError> {
        v.to_words(self.layout.sig.ret).ok_or(HookError::Type)?;
        self.ret = Some(v);
        Ok(())
    }
}

/// What a hook's handler is.
pub type Handler = Box<dyn Fn(&mut Invocation<'_>) + Send + Sync>;

const WAITING: u8 = 0;
const INSTALLING: u8 = 1;
const INSTALLED: u8 = 2;
const FAILED: u8 = 3;
const RESTORED: u8 = 4;

struct Hook {
    id: usize,
    target: Target,
    sig: Option<Sig>,
    handler: Handler,
    state: AtomicU8,
    enabled: AtomicBool,
    orig: AtomicU32,
    method: AtomicU32,
    thunk: AtomicU32,
    calls: AtomicU64,
    panics: AtomicU64,
    note: Mutex<String>,
}

impl Hook {
    fn set_note(&self, s: String) {
        *self.note.lock().unwrap_or_else(PoisonError::into_inner) = s;
    }
}

/// Identifies a hook in its [`ObjcHooks`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookId(usize);

/// State and counters of a hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookStat {
    pub name: String,
    /// "waiting", "installed", "failed" or "restored".
    pub state: &'static str,
    /// Why it waits or failed.
    pub note: String,
    pub calls: u64,
    pub panics: u64,
}

/// What one [`ObjcHooks::install`] did: hook names, with the reason for those still waiting or failed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallReport {
    pub installed: Vec<String>,
    pub waiting: Vec<(String, String)>,
    pub failed: Vec<(String, String)>,
}

/// The host's Objective-C method hooks (see the module docs).
#[derive(Default)]
pub struct ObjcHooks {
    hooks: Mutex<Vec<Arc<Hook>>>,
}

enum Outcome {
    Installed,
    Waiting(String),
    Failed(String),
}

impl ObjcHooks {
    pub fn new() -> ObjcHooks {
        ObjcHooks::default()
    }

    fn list(&self) -> Vec<Arc<Hook>> {
        self.hooks.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Ask for `target` to run `handler`. `sig` describes its arguments (checked against the runtime's encoding at
    /// install time); `None` takes the runtime's. Nothing happens until [`ObjcHooks::install`].
    pub fn hook(
        &self,
        target: Target,
        sig: Option<Sig>,
        handler: impl Fn(&mut Invocation<'_>) + Send + Sync + 'static,
    ) -> HookId {
        self.hook_boxed(target, sig, Box::new(handler))
    }

    /// [`ObjcHooks::hook`] with a boxed handler.
    pub fn hook_boxed(&self, target: Target, sig: Option<Sig>, handler: Handler) -> HookId {
        let mut hooks = self.hooks.lock().unwrap_or_else(PoisonError::into_inner);
        let id = hooks.len();
        hooks.push(Arc::new(Hook {
            id,
            target,
            sig,
            handler,
            state: AtomicU8::new(WAITING),
            enabled: AtomicBool::new(true),
            orig: AtomicU32::new(0),
            method: AtomicU32::new(0),
            thunk: AtomicU32::new(0),
            calls: AtomicU64::new(0),
            panics: AtomicU64::new(0),
            note: Mutex::new(String::new()),
        }));
        HookId(id)
    }

    /// Number of hooks still waiting for their class or method.
    pub fn waiting(&self) -> usize {
        self.list().iter().filter(|h| h.state.load(Ordering::SeqCst) == WAITING).count()
    }

    /// Install every waiting hook the runtime can serve now, on the guest thread of `c`.
    pub fn install(&self, c: &mut Cpu, rt: &Runtime) -> InstallReport {
        let mut report = InstallReport::default();
        for h in self.list() {
            // Another thread may be installing the same hook.
            if h.state.compare_exchange(WAITING, INSTALLING, Ordering::SeqCst, Ordering::SeqCst).is_err() {
                continue;
            }
            let name = h.target.to_string();
            let (state, out) = match install_one(c, rt, &h) {
                Ok(Outcome::Installed) => (INSTALLED, None),
                Ok(Outcome::Waiting(why)) => (WAITING, Some((&mut report.waiting, why))),
                Ok(Outcome::Failed(why)) => (FAILED, Some((&mut report.failed, why))),
                Err(stop) => (WAITING, Some((&mut report.waiting, format!("guest stopped: {stop}")))),
            };
            match out {
                Some((list, why)) => {
                    h.set_note(why.clone());
                    list.push((name, why));
                }
                None => {
                    h.set_note(String::new());
                    report.installed.push(name);
                }
            }
            h.state.store(state, Ordering::SeqCst);
        }
        report
    }

    /// Turn a hook off (its method then goes straight to the original) or on again.
    pub fn set_enabled(&self, id: HookId, on: bool) {
        if let Some(h) = self.list().get(id.0) {
            h.enabled.store(on, Ordering::SeqCst);
        }
    }

    /// Put the original implementation back. Returns false if the method no longer has this hook's implementation
    /// (someone swizzled it after us): the hook then stays in place but disabled, so the chain stays intact.
    pub fn restore(&self, id: HookId, c: &mut Cpu, rt: &Runtime) -> Result<bool, Stop> {
        let Some(h) = self.list().get(id.0).cloned() else { return Ok(false) };
        h.enabled.store(false, Ordering::SeqCst);
        if h.state.load(Ordering::SeqCst) != INSTALLED {
            return Ok(false);
        }
        let m = h.method.load(Ordering::SeqCst);
        if c.call(rt.get_imp, &[m])?.r0 != h.thunk.load(Ordering::SeqCst) {
            return Ok(false);
        }
        c.call(rt.set_imp, &[m, h.orig.load(Ordering::SeqCst)])?;
        h.state.store(RESTORED, Ordering::SeqCst);
        Ok(true)
    }

    /// Every hook's state and counters.
    pub fn stats(&self) -> Vec<HookStat> {
        self.list()
            .iter()
            .map(|h| HookStat {
                name: h.target.to_string(),
                state: ["waiting", "waiting", "installed", "failed", "restored"]
                    [h.state.load(Ordering::SeqCst) as usize],
                note: h.note.lock().unwrap_or_else(PoisonError::into_inner).clone(),
                calls: h.calls.load(Ordering::Relaxed),
                panics: h.panics.load(Ordering::Relaxed),
            })
            .collect()
    }
}

fn install_one(c: &mut Cpu, rt: &Runtime, h: &Arc<Hook>) -> Result<Outcome, Stop> {
    let t = &h.target;
    let class = rt.class(c, &t.class, t.meta)?;
    if class == 0 {
        return Ok(Outcome::Waiting(format!("no class {}", t.class)));
    }
    let sel = rt.sel(c, &t.sel)?;
    // Realises the class if needed (no +initialize).
    let method = c.call(rt.get_instance_method, &[class, sel])?.r0;
    if method == 0 {
        return Ok(Outcome::Waiting(format!("no method {t}")));
    }
    let enc_at = c.call(rt.type_encoding, &[method])?.r0;
    let runtime_sig = c.mem().read_cstr(enc_at).map_err(|e| e.to_string()).and_then(|e| Sig::parse(&e));
    let sig = match (&h.sig, runtime_sig) {
        (Some(mine), Ok(theirs)) if Layout::new(mine.clone()) != Layout::new(theirs.clone()) => {
            return Ok(Outcome::Failed(format!("signature {mine:?} does not match the runtime's {theirs:?}")));
        }
        (Some(mine), _) => mine.clone(),
        (None, Ok(theirs)) => theirs,
        (None, Err(e)) => return Ok(Outcome::Failed(format!("no usable signature: {e}"))),
    };
    let layout = Arc::new(Layout::new(sig));
    let hook = h.clone();
    let name = format!("bhpc:objc:{}:{t}", h.id);
    let thunk = c
        .register_shared_thunk(&name, Arc::new(move |c: &mut Cpu| dispatch(c, &hook, &layout)))
        .map_err(|e| Stop::Host(e.to_string()))?;
    // The original is known before the method can reach the thunk.
    let current = c.call(rt.get_imp, &[method])?.r0;
    h.orig.store(current, Ordering::SeqCst);
    h.thunk.store(thunk, Ordering::SeqCst);
    h.method.store(method, Ordering::SeqCst);
    let old = c.call(rt.set_imp, &[method, thunk])?.r0;
    if old != current {
        // Swizzled by someone else in between: chain to what was really there.
        h.orig.store(old, Ordering::SeqCst);
    }
    Ok(Outcome::Installed)
}

/// The thunk of an installed hook (see the module docs for the before/after/replace rules).
fn dispatch(c: &mut Cpu, h: &Hook, layout: &Layout) {
    h.calls.fetch_add(1, Ordering::Relaxed);
    let orig = h.orig.load(Ordering::Acquire);
    if !h.enabled.load(Ordering::Relaxed) {
        c.tail_call(orig);
        return;
    }
    let words = layout.read(c);
    let mut inv = Invocation { cpu: c, layout, orig, words, dirty: false, ret: None, result: None, stop: None };
    if let Err(p) = catch_unwind(AssertUnwindSafe(|| (h.handler)(&mut inv))) {
        let n = h.panics.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_power_of_two() {
            let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned());
            eprintln!("[objc-hook] {}: handler panicked ({n} so far): {}", h.target, msg.unwrap_or_default());
        }
        // Continue as if the hook were not there.
        inv.ret = None;
        inv.dirty = false;
    }
    let Invocation { ret, result, stop, dirty, words, .. } = inv;
    if let Some(s) = stop {
        c.request_stop(s);
        return;
    }
    match (ret, result) {
        (Some(v), _) => {
            let w = v.to_words(layout.sig.ret).unwrap_or_default();
            if layout.sret {
                for (i, x) in w.iter().enumerate() {
                    let _ = c.mem().write_u32(words[0] + 4 * i as u32, *x);
                }
                c.set_reg(0, words[0]);
            } else {
                for (i, x) in w.iter().enumerate() {
                    c.set_reg(i, *x);
                }
            }
        }
        (None, Some(r)) => {
            c.set_reg(0, r.r0);
            c.set_reg(1, r.r1);
        }
        (None, None) => {
            if dirty {
                layout.write(c, &words);
            }
            c.tail_call(orig);
        }
    }
}

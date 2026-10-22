//! The client mod loader's glue to the game (play build): Objective-C hooks on the game's networking, chat and
//! world classes ([`crate::objchook`]) feed [`bhpc_mods::join::JoinFlow`] and [`bhpc_mods::host::ClientHost`], and
//! carry out what they ask for through guest calls.
//!
//! Threads: everything mod-related runs on the game's main thread (t0): the client's network poll timer
//! (`-[BHClient match:didReceiveData:fromPlayer:]`), the world update, chat, and the frame hook (inside
//! `eglSwapBuffers`), so the flow and the host live in a thread-local of t0. A hook that fires on another thread
//! leaves the call alone. The player's answers come from the window thread through [`Shared`] and are applied at
//! the next frame. Nothing ever waits: the 0x1f the game wants to send while the server's mod list is open is
//! *held* (the hook answers YES without sending it) and sent later from the frame hook.
//!
//! Hooks (1.7.x, signatures from the runtime):
//! - `-[BHClient match:didReceiveData:fromPlayer:]`: 0x23 starts a [`JoinFlow`] (`HELLO`), 0xBC goes to the flow
//!   and never reaches the game.
//! - `-[BHNetClientMatch sendData:toPlayers:withDataMode:error:]`: the 0x1f is held or gets `bhcModLoader`.
//! - `-[World update:accurateDT:pinchScale:dragInProgress:]`: starts the session's mods at the first update of a
//!   world and fires `tick`; `-[World dealloc]` stops them.
//! - `-[BHClient chatMessageRecieved:displayNotification:]` (incoming chat), `-[BHClient sendChatMessage:
//!   sendToClients:]` (typed lines and commands), `-[BHNetNode updatePlayerList:]` (players).
//! - `-[MainMenuUI render:projectionMatrix:]`: remembers the main menu for `--join HOST:PORT`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use bhpc_bionic::sched;
use bhpc_cpu::Cpu;
use bhpc_launcher::mods::ModsDirs;
use bhpc_mods::host::{ClientHost, Effect, HostSetup};
use bhpc_mods::join::{inject_mod_loader, Accepted, Action, JoinConfig, JoinFlow, PlayerInfo, Stage};
use bhpc_mods::session::{self, Plan};
use bhpc_mods::view::{JoinView, UiCmd, View};

use crate::objchook::{Invocation, ObjcHooks, Runtime, Target, Val};

/// How the loader runs (from the command line).
#[derive(Debug, Clone, Default)]
pub struct ModsOptions {
    /// The launcher's data folder (`mods/`, `mods-cache/` below it).
    pub data_dir: PathBuf,
    /// Developer folder mods.
    pub dev: Vec<PathBuf>,
    /// "Never allow native mods".
    pub block_native: bool,
    /// Join this server (`host:port`) from the main menu once it shows.
    pub join: Option<String>,
    /// Accept a join prompt without asking (tests).
    pub auto_accept: bool,
}

/// State shared with the window thread (drawing, input).
#[derive(Default)]
pub struct Shared {
    pub view: Mutex<View>,
    /// The mod UI's own state (open panel, native confirmation, clickable areas).
    pub ui: Mutex<bhpc_mods::ui::ModUi>,
    cmds: Mutex<Vec<UiCmd>>,
    /// Bumped when `view` changes.
    pub serial: std::sync::atomic::AtomicU64,
}

impl Shared {
    /// The player did something in the mod UI.
    pub fn post(&self, c: UiCmd) {
        self.cmds.lock().unwrap_or_else(PoisonError::into_inner).push(c);
    }

    fn take(&self) -> Vec<UiCmd> {
        std::mem::take(&mut *self.cmds.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// The connection a join runs on.
#[derive(Clone, Copy)]
struct Conn {
    /// The `BHNetClientMatch`.
    matcher: u32,
    /// The server's peer id (retained NSString).
    peer: u32,
    /// The `BHClient`.
    client: u32,
}

/// The 0x1f the game wanted to send.
struct Held {
    matcher: u32,
    bytes: Vec<u8>,
    players: u32,
    mode: u32,
}

#[derive(Default)]
struct Main {
    flow: Option<JoinFlow>,
    conn: Option<Conn>,
    held: Option<Held>,
    /// Server mods accepted for the coming world.
    accepted: Option<Vec<Accepted>>,
    host: Option<ClientHost>,
    /// The running session belongs to the joined server (not single-player).
    host_online: bool,
    placed: Vec<session::Placed>,
    menu: u32,
    /// Since when the main menu renders without a world in between.
    menu_since: Option<Instant>,
    joined_menu: bool,
    last_publish: Option<Instant>,
}

thread_local! {
    static MAIN: RefCell<Main> = RefCell::new(Main::default());
    /// Set while the glue itself calls a hooked method (the hook then passes the call through).
    static BYPASS: Cell<bool> = const { Cell::new(false) };
}

fn on_main() -> bool {
    sched::ctx().tid == 0
}

fn bypassed() -> bool {
    BYPASS.with(Cell::get)
}

fn bypass<T>(f: impl FnOnce() -> T) -> T {
    let was = BYPASS.with(|b| b.replace(true));
    let r = f();
    BYPASS.with(|b| b.set(was));
    r
}

/// Guest Objective-C calls with cached selectors and classes.
struct Objc {
    rt: Runtime,
    cache: Mutex<HashMap<String, u32>>,
}

/// Runs `f` with `bytes` (plus a NUL) on the guest stack below `sp`.
fn with_bytes<T>(c: &mut Cpu, bytes: &[u8], f: impl FnOnce(&mut Cpu, u32) -> T) -> T {
    let sp = c.sp();
    let at = (sp - bytes.len() as u32 - 16) & !7;
    let mut b = bytes.to_vec();
    b.push(0);
    let _ = c.mem().write_bytes(at, &b);
    c.set_sp(at);
    let r = f(c, at);
    c.set_sp(sp);
    r
}

impl Objc {
    fn cached(&self, key: String, f: impl FnOnce() -> u32) -> u32 {
        if let Some(v) = self.cache.lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
            return *v;
        }
        let v = f();
        if v != 0 {
            self.cache.lock().unwrap_or_else(PoisonError::into_inner).insert(key, v);
        }
        v
    }

    fn sel(&self, c: &mut Cpu, name: &str) -> u32 {
        self.cached(format!("sel:{name}"), || self.rt.sel(c, name).unwrap_or(0))
    }

    fn class(&self, c: &mut Cpu, name: &str) -> u32 {
        let f = self.rt.get_class;
        self.cached(format!("cls:{name}"), || {
            with_bytes(c, name.as_bytes(), |c, s| c.call(f, &[s]).map_or(0, |r| r.r0))
        })
    }

    /// `[obj sel args...]` (word arguments and result).
    fn send(&self, c: &mut Cpu, obj: u32, sel: &str, args: &[u32]) -> u32 {
        if obj == 0 {
            return 0;
        }
        let s = self.sel(c, sel);
        self.rt.send(c, obj, s, args).map_or(0, |r| r.r0)
    }

    fn data_bytes(&self, c: &mut Cpu, data: u32) -> Vec<u8> {
        let len = self.send(c, data, "length", &[]);
        let ptr = self.send(c, data, "bytes", &[]);
        if ptr == 0 {
            return Vec::new();
        }
        c.mem().read_bytes(ptr, len.min(16 << 20)).unwrap_or_default()
    }

    fn new_data(&self, c: &mut Cpu, bytes: &[u8]) -> u32 {
        let cls = self.class(c, "NSData");
        let s = self.sel(c, "dataWithBytes:length:");
        let rt = self.rt;
        with_bytes(c, bytes, |c, at| rt.send(c, cls, s, &[at, bytes.len() as u32]).map_or(0, |r| r.r0))
    }

    fn string(&self, c: &mut Cpu, s: u32) -> String {
        let p = self.send(c, s, "UTF8String", &[]);
        if p == 0 {
            return String::new();
        }
        c.mem().read_cstr(p).unwrap_or_default()
    }

    fn new_string(&self, c: &mut Cpu, text: &str) -> u32 {
        let cls = self.class(c, "NSString");
        let s = self.sel(c, "stringWithUTF8String:");
        let rt = self.rt;
        with_bytes(c, text.as_bytes(), |c, at| rt.send(c, cls, s, &[at]).map_or(0, |r| r.r0))
    }
}

/// The installed loader.
pub struct ModGlue {
    pub hooks: Arc<ObjcHooks>,
    pub rt: Runtime,
    pub shared: Arc<Shared>,
    objc: Arc<Objc>,
    opts: Arc<ModsOptions>,
}

fn log(msg: &str) {
    eprintln!("[mods] {msg}");
}

impl ModGlue {
    /// Registers the hooks (install them with [`ModGlue::install`]).
    pub fn new(rt: Runtime, opts: ModsOptions) -> ModGlue {
        let hooks = Arc::new(ObjcHooks::new());
        let objc = Arc::new(Objc { rt, cache: Mutex::new(HashMap::new()) });
        let shared = Arc::new(Shared::default());
        let opts = Arc::new(opts);
        let g = ModGlue { hooks, rt, shared, objc, opts };
        g.register();
        g
    }

    /// Installs waiting hooks (call again later for classes that are not loaded yet).
    pub fn install(&self, c: &mut Cpu) {
        let r = self.hooks.install(c, &self.rt);
        for (n, why) in r.failed.iter().chain(&r.waiting) {
            log(&format!("hook {n}: {why}"));
        }
    }

    fn register(&self) {
        let t = |s: &str| Target::parse(s).expect("valid target");
        let (o, op, sh) = (self.objc.clone(), self.opts.clone(), self.shared.clone());
        self.hooks.hook(t("-[BHClient match:didReceiveData:fromPlayer:]"), None, move |inv| {
            if on_main() && !bypassed() {
                receive(&o, &op, &sh, inv);
            }
        });
        let (o, sh) = (self.objc.clone(), self.shared.clone());
        self.hooks.hook(t("-[BHNetClientMatch sendData:toPlayers:withDataMode:error:]"), None, move |inv| {
            if on_main() && !bypassed() {
                send_data(&o, &sh, inv);
            }
        });
        let (o, op, sh) = (self.objc.clone(), self.opts.clone(), self.shared.clone());
        self.hooks.hook(t("-[World update:accurateDT:pinchScale:dragInProgress:]"), None, move |inv| {
            if on_main() {
                world_update(&o, &op, &sh, inv.cpu());
            }
        });
        let sh = self.shared.clone();
        self.hooks.hook(t("-[World dealloc]"), None, move |_inv| {
            if on_main() {
                stop_session(&sh, "left the world");
            }
        });
        let o = self.objc.clone();
        self.hooks.hook(t("-[BHClient chatMessageRecieved:displayNotification:]"), None, move |inv| {
            if on_main() && !bypassed() {
                chat_in(&o, inv);
            }
        });
        let (o, sh) = (self.objc.clone(), self.shared.clone());
        self.hooks.hook(t("-[BHClient sendChatMessage:sendToClients:]"), None, move |inv| {
            if on_main() && !bypassed() {
                chat_out(&o, &sh, inv);
            }
        });
        let o = self.objc.clone();
        self.hooks.hook(t("-[BHNetNode updatePlayerList:]"), None, move |inv| {
            if on_main() {
                players(&o, inv);
            }
        });
        // `v76@0:4f8(_GLKMatrix4=...)12`: the matrix is a union, which a runtime signature cannot describe.
        let render = crate::objchook::Sig {
            ret: crate::objchook::Ty::Void,
            args: vec![crate::objchook::Ty::F32, crate::objchook::Ty::Struct { words: 16, align8: false }],
        };
        self.hooks.hook(t("-[MainMenuUI render:projectionMatrix:]"), Some(render), move |inv| {
            let me = inv.receiver();
            MAIN.with(|m| {
                let mut m = m.borrow_mut();
                m.menu = me;
                m.menu_since.get_or_insert_with(Instant::now);
            });
        });
    }

    /// Once per frame on the main thread: timeouts, the player's answers, mod polls and effects.
    pub fn frame(&self, c: &mut Cpu) {
        if !on_main() {
            return;
        }
        let now = Instant::now();
        let cmds = self.shared.take();
        let actions = MAIN.with(|m| {
            let mut m = m.borrow_mut();
            let mut out = Vec::new();
            if let Some(f) = m.flow.as_mut() {
                out.extend(f.tick(now));
                for cmd in &cmds {
                    match cmd {
                        UiCmd::Decide { accept, native_ok } => out.extend(f.decide(*accept, *native_ok, now)),
                        UiCmd::Dismiss | UiCmd::Reload | UiCmd::Cancel => {}
                    }
                }
            }
            out
        });
        for cmd in &cmds {
            match cmd {
                UiCmd::Dismiss => {
                    let mut v = self.shared.view.lock().unwrap_or_else(PoisonError::into_inner);
                    v.join = None;
                    self.shared.serial.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                UiCmd::Reload => reload(&self.opts, &self.shared),
                UiCmd::Cancel => {
                    log("download cancelled by the player");
                    let conn = MAIN.with(|m| {
                        let mut m = m.borrow_mut();
                        m.flow = None;
                        m.held = None;
                        m.conn
                    });
                    if let Some(conn) = conn {
                        self.objc.send(c, conn.matcher, "disconnect", &[]);
                    }
                    self.shared.view.lock().unwrap_or_else(PoisonError::into_inner).join = None;
                    self.shared.serial.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                UiCmd::Decide { .. } => {}
            }
        }
        run_actions(&self.objc, &self.opts, &self.shared, c, actions);
        MAIN.with(|m| {
            if let Some(h) = m.borrow_mut().host.as_mut() {
                h.poll();
            }
        });
        apply_effects(&self.objc, c);
        self.join_from_menu(c);
        publish(&self.shared, false);
    }

    /// `--join HOST:PORT`: once the main menu renders, ask it to join (as a join link would).
    fn join_from_menu(&self, c: &mut Cpu) {
        let Some(target) = self.opts.join.as_deref() else { return };
        // Only once the menu has settled (a fresh start shows it briefly before the first world).
        let menu = MAIN.with(|m| {
            let m = m.borrow();
            let settled = m.menu_since.is_some_and(|t| t.elapsed() >= Duration::from_secs(3));
            if m.joined_menu || !settled {
                0
            } else {
                m.menu
            }
        });
        if menu == 0 {
            return;
        }
        MAIN.with(|m| m.borrow_mut().joined_menu = true);
        let (host, port) = target.rsplit_once(':').unwrap_or((target, "15151"));
        log(&format!("joining {host}:{port} from the main menu"));
        let h = self.objc.new_string(c, host);
        let p = self.objc.new_string(c, port);
        // As a join link would: the menu takes the address, then its "Join Online World" action connects.
        self.objc.send(c, menu, "selectJoinOption", &[]);
        self.objc.send(c, menu, "gameLaunchedWithUrlToJoinHost:port:", &[h, p]);
        self.objc.send(c, menu, "joinOnlineWorldButtonTapped", &[]);
    }
}

fn join_config(opts: &ModsOptions, address: String) -> JoinConfig {
    let dirs = ModsDirs::new(&opts.data_dir);
    let index = std::fs::read(dirs.index_cache()).ok().and_then(|b| bhpc_launcher::mods::index::parse_index(&b).ok());
    JoinConfig {
        loader: bhpc_mods::LOADER_VERSION.into(),
        platform: bhpc_mods::platform(),
        api: bhpc_mods::API_VERSION.into(),
        runtimes: vec!["lua".into(), "wasm".into()],
        dirs,
        index,
        block_native: opts.block_native,
        address,
    }
}

fn receive(o: &Objc, opts: &ModsOptions, sh: &Shared, inv: &mut Invocation<'_>) {
    let (matcher, data, peer) =
        (inv.arg(0).map_or(0, |v| v.word()), inv.arg(1).map_or(0, |v| v.word()), inv.arg(2).map_or(0, |v| v.word()));
    let client = inv.receiver();
    let c = inv.cpu();
    let len = o.send(c, data, "length", &[]);
    if len == 0 {
        return;
    }
    let ptr = o.send(c, data, "bytes", &[]);
    let kind = c.mem().read_u8(ptr).unwrap_or(0);
    let now = Instant::now();
    match kind {
        0x23 => {
            // The server's first packet: a new connection, a new join.
            o.send(c, peer, "retain", &[]);
            let host = o.send(c, client, "host", &[]);
            let port = o.send(c, client, "port", &[]);
            let address = format!("{}:{}", o.string(c, host), o.string(c, port));
            log(&format!("server contact from {address}: HELLO"));
            let mut flow = JoinFlow::new(join_config(opts, address));
            let actions = flow.contact(now);
            let old = MAIN.with(|m| {
                let mut m = m.borrow_mut();
                m.flow = Some(flow);
                m.held = None;
                m.accepted = None;
                m.conn.replace(Conn { matcher, peer, client })
            });
            if let Some(old) = old {
                o.send(c, old.peer, "release", &[]);
            }
            run_actions(o, opts, sh, c, actions);
        }
        0xBC => {
            let bytes = o.data_bytes(c, data);
            let actions =
                MAIN.with(|m| m.borrow_mut().flow.as_mut().map(|f| f.packet(&bytes, now)).unwrap_or_default());
            run_actions(o, opts, sh, c, actions);
            // 0xBC is ours: the game never sees it.
            let _ = inv.set_return(Val::Void);
        }
        _ => {}
    }
}

fn send_data(o: &Objc, sh: &Shared, inv: &mut Invocation<'_>) {
    let data = inv.arg(0).map_or(0, |v| v.word());
    let matcher = inv.receiver();
    let c = inv.cpu();
    let len = o.send(c, data, "length", &[]);
    let ptr = o.send(c, data, "bytes", &[]);
    if len == 0 || c.mem().read_u8(ptr).unwrap_or(0) != 0x1f {
        return;
    }
    let decision = MAIN.with(|m| m.borrow_mut().flow.as_mut().map(|f| f.player_info()));
    match decision {
        Some(PlayerInfo::Hold) => {
            let bytes = o.data_bytes(c, data);
            let players = inv.arg(1).map_or(0, |v| v.word());
            let mode = inv.arg(2).map_or(0, |v| v.word());
            let c = inv.cpu();
            o.send(c, players, "retain", &[]);
            log("holding the game's 0x1f until the mods are ready");
            MAIN.with(|m| m.borrow_mut().held = Some(Held { matcher, bytes, players, mode }));
            let _ = inv.set_return(Val::Word(1));
        }
        Some(PlayerInfo::Send(Some(hash))) => {
            let bytes = o.data_bytes(c, data);
            if let Some(new) = inject_mod_loader(&bytes, &hash) {
                let d = o.new_data(c, &new);
                let _ = inv.set_arg(0, Val::Word(d));
            }
        }
        _ => {}
    }
    let _ = sh;
}

/// Sends a packet to the server of the current connection (reliable).
fn send_packet(o: &Objc, c: &mut Cpu, conn: Conn, bytes: &[u8]) {
    let data = o.new_data(c, bytes);
    let arr_cls = o.class(c, "NSArray");
    let arr = o.send(c, arr_cls, "arrayWithObject:", &[conn.peer]);
    o.send(c, conn.matcher, "sendData:toPlayers:withDataMode:error:", &[data, arr, 0, 0]);
}

fn run_actions(o: &Objc, opts: &ModsOptions, sh: &Shared, c: &mut Cpu, actions: Vec<Action>) {
    if actions.is_empty() {
        return;
    }
    let conn = MAIN.with(|m| m.borrow().conn);
    for a in actions {
        match a {
            Action::Send(bytes) => {
                if let Some(conn) = conn {
                    send_packet(o, c, conn, &bytes);
                }
            }
            Action::Release(hash) => {
                let Some(held) = MAIN.with(|m| m.borrow_mut().held.take()) else { continue };
                let bytes = match &hash {
                    Some(h) => inject_mod_loader(&held.bytes, h).unwrap_or(held.bytes.clone()),
                    None => held.bytes.clone(),
                };
                log(&format!(
                    "sending the 0x1f{}",
                    if hash.is_some() { " with bhcModLoader" } else { " (vanilla server)" }
                ));
                let data = o.new_data(c, &bytes);
                bypass(|| {
                    o.send(
                        c,
                        held.matcher,
                        "sendData:toPlayers:withDataMode:error:",
                        &[data, held.players, held.mode, 0],
                    )
                });
                o.send(c, held.players, "release", &[]);
            }
            Action::Prompt => {
                log("the server uses mods: asking the player");
                if opts.auto_accept {
                    sh.post(UiCmd::Decide { accept: true, native_ok: false });
                }
            }
            Action::Disconnect => {
                log("leaving the server");
                if let Some(conn) = conn {
                    o.send(c, conn.matcher, "disconnect", &[]);
                }
            }
            Action::Run(mods) => {
                log(&format!("joined with {} server mod(s)", mods.len()));
                MAIN.with(|m| m.borrow_mut().accepted = Some(mods));
            }
            Action::Rejected(r) => {
                log(&format!("rejected by the server: {}", r.text));
                let server =
                    MAIN.with(|m| m.borrow().flow.as_ref().map(|f| f.server_name().to_string())).unwrap_or_default();
                let mut v = sh.view.lock().unwrap_or_else(PoisonError::into_inner);
                v.join = Some(JoinView::rejected(&server, &r.text, r.url.as_deref()));
            }
            Action::ChannelIn(name, data) => {
                MAIN.with(|m| {
                    if let Some(h) = m.borrow_mut().host.as_mut() {
                        h.channel_in(&name, data);
                    }
                });
            }
        }
    }
    // The prompt follows the flow.
    let jv = MAIN.with(|m| m.borrow().flow.as_ref().and_then(JoinView::of));
    let mut v = sh.view.lock().unwrap_or_else(PoisonError::into_inner);
    if jv.is_some() || !matches!(v.join.as_ref().map(|j| &j.stage), Some(bhpc_mods::view::JoinStage::Rejected(..))) {
        v.join = jv;
    }
    sh.serial.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// Starts the session's mods at the first update of a world, then fires `tick`.
fn world_update(o: &Objc, opts: &ModsOptions, sh: &Shared, c: &mut Cpu) {
    MAIN.with(|m| m.borrow_mut().menu_since = None);
    // A world that updates while a join is being negotiated is the one being left (the menu's): no session for it.
    let (started, joining) = MAIN.with(|m| {
        let m = m.borrow();
        let joining = m.flow.as_ref().is_some_and(|f| {
            matches!(f.stage(), Stage::Idle | Stage::Hello | Stage::Listing | Stage::Prompt | Stage::Downloading)
        });
        (m.host.is_some(), joining)
    });
    if joining {
        return;
    }
    if !started {
        start_session(o, opts, sh, c);
    }
    MAIN.with(|m| {
        if let Some(h) = m.borrow_mut().host.as_mut() {
            h.tick(Instant::now());
        }
    });
    apply_effects(o, c);
}

fn start_session(o: &Objc, opts: &ModsOptions, sh: &Shared, c: &mut Cpu) {
    let (server, conn) = MAIN.with(|m| {
        let mut m = m.borrow_mut();
        let joined = m.flow.as_ref().is_some_and(|f| *f.stage() == Stage::Joined || *f.stage() == Stage::Vanilla);
        let server = if joined { m.flow.as_ref().map(|f| f.server_name().to_string()) } else { None };
        let accepted = m.accepted.take().unwrap_or_default();
        (server.map(|s| (s, accepted)), m.conn)
    });
    let dirs = ModsDirs::new(&opts.data_dir);
    let plan = Plan {
        own: Some(dirs.mods.clone()),
        dev: opts.dev.clone(),
        server: server.as_ref().map(|(_, a)| a.clone()).unwrap_or_default(),
    };
    let dir = dirs.cache.join("session");
    let (placed, problems) = match session::build(&dir, &plan) {
        Ok(r) => r,
        Err(e) => {
            log(&format!("cannot prepare the session's mods: {e}"));
            (Vec::new(), Vec::new())
        }
    };
    for p in &problems {
        log(p);
    }
    let key = server.as_ref().map_or("local".to_string(), |(s, _)| bhpc_launcher::mods::cache::server_key(s));
    let local = conn.map(|cn| {
        let n = o.send(c, cn.client, "localPlayerName", &[]);
        o.string(c, n)
    });
    let mut host = ClientHost::load(HostSetup {
        mods_dir: &dir,
        storage_dir: &opts.data_dir.join("mods-data").join(&key),
        runtimes: bhpc_mods::host::default_runtimes(&dirs.cache.join("wasm")),
        online: server.is_some(),
        local_player: local.unwrap_or_default(),
    });
    let world = server.as_ref().map_or("single-player".to_string(), |(s, _)| s.clone());
    host.enable(&world, &key);
    log(&format!("session started ({world}): {} mod(s)", placed.len()));
    MAIN.with(|m| {
        let mut m = m.borrow_mut();
        m.host = Some(host);
        m.host_online = server.is_some();
        m.placed = placed;
    });
    {
        let mut v = sh.view.lock().unwrap_or_else(PoisonError::into_inner);
        v.server = server.map(|(s, _)| s);
    }
    publish(sh, true);
}

fn stop_session(sh: &Shared, why: &str) {
    let stopped = MAIN.with(|m| {
        let mut m = m.borrow_mut();
        m.placed.clear();
        // Leaving a server's world ends that join; leaving a single-player world keeps a join in progress.
        if m.host.is_some() && m.host_online {
            m.flow = None;
            m.conn = None;
            m.held = None;
            m.accepted = None;
        }
        m.host.take().map(|mut h| {
            h.stop();
            h
        })
    });
    if let Some(h) = stopped {
        for l in h.take_new_logs() {
            log_line(&l);
        }
        log(&format!("session stopped: {why}"));
        let mut v = sh.view.lock().unwrap_or_else(PoisonError::into_inner);
        v.mods.clear();
        v.server = None;
    }
}

/// Developer reload: the session's mods are stopped and loaded again from their folders.
fn reload(opts: &ModsOptions, sh: &Shared) {
    let _ = opts;
    stop_session(sh, "reload");
    // The next world update starts it again (with the same server mods).
    log("reloading mods at the next world update");
}

fn log_line(l: &bhpc_mods::host::LogLine) {
    eprintln!(
        "[mods] {:>8} ms {:5} {}{}",
        l.ms,
        l.level,
        l.mod_id.as_deref().map(|m| format!("{m}: ")).unwrap_or_default(),
        l.text
    );
}

/// Mods' chat lines and channel packets.
fn apply_effects(o: &Objc, c: &mut Cpu) {
    let (effects, logs) = MAIN.with(|m| {
        let m = m.borrow();
        m.host.as_ref().map(|h| (h.take_effects(), h.take_new_logs())).unwrap_or_default()
    });
    for l in &logs {
        log_line(l);
    }
    if effects.is_empty() {
        return;
    }
    let (conn, packets): (Option<Conn>, Vec<Option<Vec<u8>>>) = MAIN.with(|m| {
        let m = m.borrow();
        let p = effects
            .iter()
            .map(|e| match e {
                Effect::Channel { name, data } => m.flow.as_ref().and_then(|f| f.channel_packet(name, data)),
                Effect::Chat(_) => None,
            })
            .collect();
        (m.conn, p)
    });
    for (e, p) in effects.iter().zip(packets) {
        match (e, conn) {
            (Effect::Chat(text), Some(conn)) => {
                let d = chat_dict(o, c, "MOD", text);
                bypass(|| o.send(c, conn.client, "chatMessageRecieved:displayNotification:", &[d, 1]));
            }
            (Effect::Chat(text), None) => log(&format!("chat (no client to show it): {text}")),
            (Effect::Channel { name, .. }, Some(conn)) => match p {
                Some(bytes) => send_packet(o, c, conn, &bytes),
                None => log(&format!("channel {name}: the server has not opened it")),
            },
            (Effect::Channel { name, .. }, None) => log(&format!("channel {name}: not connected")),
        }
    }
}

/// A dictionary `{message, alias}` as the game's chat uses.
fn chat_dict(o: &Objc, c: &mut Cpu, alias: &str, text: &str) -> u32 {
    let cls = o.class(c, "NSMutableDictionary");
    let d = o.send(c, cls, "dictionary", &[]);
    let (m, mk) = (o.new_string(c, text), o.new_string(c, "message"));
    o.send(c, d, "setObject:forKey:", &[m, mk]);
    let (a, ak) = (o.new_string(c, alias), o.new_string(c, "alias"));
    o.send(c, d, "setObject:forKey:", &[a, ak]);
    d
}

fn chat_in(o: &Objc, inv: &mut Invocation<'_>) {
    // The message is a dictionary `{message, alias, playerID, date, ...}`.
    let dict = inv.arg(0).map_or(0, |v| v.word());
    if MAIN.with(|m| m.borrow().host.is_none()) {
        return;
    }
    let c = inv.cpu();
    let dict_sel = o.sel(c, "objectForKey:");
    if o.send(c, dict, "respondsToSelector:", &[dict_sel]) == 0 {
        return;
    }
    let (mk, ak) = (o.new_string(c, "message"), o.new_string(c, "alias"));
    let (m, a) = (o.send(c, dict, "objectForKey:", &[mk]), o.send(c, dict, "objectForKey:", &[ak]));
    let (text, alias) = (o.string(c, m), o.string(c, a));
    let out = MAIN.with(|mm| mm.borrow_mut().host.as_mut().map(|h| h.chat_received(&alias, &text)));
    match out {
        Some(None) => {
            let _ = inv.set_return(Val::Void);
        }
        Some(Some(new)) if new != text => {
            let c = inv.cpu();
            let copy = o.send(c, dict, "mutableCopy", &[]);
            let ns = o.new_string(c, &new);
            let mk = o.new_string(c, "message");
            o.send(c, copy, "setObject:forKey:", &[ns, mk]);
            o.send(c, copy, "autorelease", &[]);
            let _ = inv.set_arg(0, Val::Word(copy));
        }
        _ => {}
    }
    apply_effects(o, inv.cpu());
}

fn chat_out(o: &Objc, sh: &Shared, inv: &mut Invocation<'_>) {
    let s = inv.arg(0).map_or(0, |v| v.word());
    let c = inv.cpu();
    let line = o.string(c, s);
    let handled = MAIN.with(|m| m.borrow_mut().host.as_mut().is_some_and(|h| h.typed(&line)));
    if handled {
        let _ = inv.set_return(Val::Void);
    }
    apply_effects(o, inv.cpu());
    let _ = sh;
}

fn players(o: &Objc, inv: &mut Invocation<'_>) {
    let me = inv.receiver();
    let _ = inv.call_original();
    let is_client = MAIN.with(|m| {
        let m = m.borrow();
        m.host.is_some() && m.conn.is_some_and(|c| c.client == me)
    });
    if !is_client {
        return;
    }
    let c = inv.cpu();
    let list = o.send(c, me, "livePlayerInfos", &[]);
    let n = o.send(c, list, "count", &[]);
    let key = o.new_string(c, "alias");
    let mut names = Vec::new();
    let dict_sel = o.sel(c, "objectForKey:");
    for i in 0..n.min(256) {
        let e = o.send(c, list, "objectAtIndex:", &[i]);
        // Entries are player-info dicts on the server; a client may hold peer ids (strings) instead.
        let d = if o.send(c, e, "respondsToSelector:", &[dict_sel]) != 0 {
            e
        } else {
            o.send(c, me, "playerInfoForPeerID:", &[e])
        };
        let a = if d != 0 && o.send(c, d, "respondsToSelector:", &[dict_sel]) != 0 {
            o.send(c, d, "objectForKey:", &[key])
        } else {
            0
        };
        let utf8 = o.sel(c, "UTF8String");
        let name = if a != 0 {
            o.string(c, a)
        } else if o.send(c, e, "respondsToSelector:", &[utf8]) != 0 {
            o.string(c, e)
        } else {
            String::new()
        };
        if !name.is_empty() {
            names.push(name);
        }
    }
    MAIN.with(|m| {
        if let Some(h) = m.borrow_mut().host.as_mut() {
            h.players(names);
        }
    });
    apply_effects(o, c);
}

/// Copies the mod list and log into the shared view (at most twice a second unless `now`).
fn publish(sh: &Shared, force: bool) {
    let due = MAIN.with(|m| {
        let mut m = m.borrow_mut();
        let due = force || m.last_publish.is_none_or(|t| t.elapsed() >= Duration::from_millis(500));
        if due {
            m.last_publish = Some(Instant::now());
        }
        due
    });
    if !due {
        return;
    }
    let (mods, log) = MAIN.with(|m| {
        let m = m.borrow();
        match &m.host {
            Some(h) => {
                let origin: HashMap<&str, &'static str> = m.placed.iter().map(|p| (p.id.as_str(), p.origin)).collect();
                let mods = h.status().into_iter().map(|s| {
                    let o = origin.get(s.id.as_str()).copied().unwrap_or("mine");
                    (s, o)
                });
                (mods.collect(), h.log())
            }
            None => (Vec::new(), Vec::new()),
        }
    });
    let mut v = sh.view.lock().unwrap_or_else(PoisonError::into_inner);
    // Toasts for mods turned off since the last look.
    for (s, _) in &mods {
        let was = v.mods.iter().find(|(o, _)| o.id == s.id).map(|(o, _)| o.state);
        if was == Some("running") && s.state == "budget" {
            v.toasts.push((Instant::now(), format!("{} was turned off. It slowed the game.", s.id)));
        }
    }
    v.toasts.retain(|(t, _)| t.elapsed() < Duration::from_secs(4));
    let line = serde_status(&mods);
    v.mods = mods;
    v.log = log;
    drop(v);
    sh.serial.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if let Some(l) = line {
        println!("{l}");
    }
}

/// The `@mods` status line for the launcher, `None` when it did not change.
fn serde_status(mods: &[(bhpc_mods::host::ModStatus, &'static str)]) -> Option<String> {
    static LAST: Mutex<String> = Mutex::new(String::new());
    let line = bhpc_mods::view::status_line(mods);
    let mut last = LAST.lock().unwrap_or_else(PoisonError::into_inner);
    (*last != line).then(|| {
        *last = line.clone();
        line
    })
}

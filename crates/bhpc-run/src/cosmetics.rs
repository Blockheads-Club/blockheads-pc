//! Cosmetics on the game's blockheads (mod system M8, play build): the server's `COSMETICS` say who wears what, and
//! these Objective-C hooks make the blockheads show the mods' textures.
//!
//! What a blockhead shows is decided by `bhmod_content` (shared with the Android loader). Here: reading the
//! blockhead's ivars, making `CPTexture2D`s from the mods' pixels and writing them into the ivars the game drew
//! with.
//!
//! Hooks (all on the game's main thread, nothing is done while no mod declares a cosmetic). A cosmetic re-skins what
//! the blockhead wears (hair: always); an empty clothing slot shows nothing yet (see notes/resume/m8-content.md):
//! - `-[Blockhead updateClothingCubes]` (the game's own, for a blockhead of this client): runs the original, then
//!   replaces `hatTexture`, `shirtBodyTexture`, `shirtArmTexture`, `pantsTexture`, `shoesTexture`. While it runs,
//! - `-[Blockhead updateSkin]`: replaces `headHairTexture`.
//! - `-[Blockhead setupFromNetCreationData:]` (other players' blockheads): replaces the clothing and hair textures.
//! - `-[Blockhead update:accurateDT:isSimulation:]` (every frame): when the cosmetics changed since this blockhead was last looked
//!   at, refreshes it (the game's own update for this client's blockheads, the texture swap for the others).
//!
//! Dev aid: `BHPC_DEV_COSMETICS=hat=<key>,hair=<key>` makes every blockhead wear those cosmetics (single-player,
//! with `--mods-dev` folders; the ids come from an in-memory registry), for testing without a server.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use bhmod_content::registry::Wire;
use bhmod_content::{Content, Part, CLOTHING};
use bhmod_proto::Cosmetics;
use bhpc_bionic::sched;
use bhpc_cpu::Cpu;

use crate::modglue::Objc;
use crate::objchook::{ObjcHooks, Target};

/// Guest addresses of the ivar offset variables (`OBJC_IVAR_$_Class.name`) the hooks read and write.
const IVARS: [(&str, &str); 15] = [
    ("DynamicObject", "pos"),
    ("World", "macroTiles"),
    ("Blockhead", "inventoryItems"),
    ("Blockhead", "clientID"),
    ("DynamicObject", "isNet"),
    ("Blockhead", "hatItemType"),
    ("Blockhead", "shirtItemType"),
    ("Blockhead", "pantsItemType"),
    ("Blockhead", "shoesItemType"),
    ("Blockhead", "hatTexture"),
    ("Blockhead", "shirtBodyTexture"),
    ("Blockhead", "shirtArmTexture"),
    ("Blockhead", "pantsTexture"),
    ("Blockhead", "shoesTexture"),
    ("Blockhead", "headHairTexture"),
];

/// Ivars used when the game exports them (overhead icons).
const OPTIONAL_IVARS: [(&str, &str); 2] = [("DynamicObject", "floatPos"), ("Blockhead", "clientName")];

/// Exports of the game's libraries the hooks need.
#[derive(Clone)]
pub struct Ivars {
    store_strong: u32,
    addr: HashMap<&'static str, u32>,
}

impl Ivars {
    /// Looks the exports up (`export` finds a symbol in the loaded libraries).
    pub fn resolve(export: impl Fn(&str) -> Option<u32>) -> Result<Ivars, String> {
        let store_strong = export("objc_storeStrong").ok_or("no objc_storeStrong export")?;
        let mut addr = HashMap::new();
        for (class, name) in IVARS {
            let a = export(&format!("OBJC_IVAR_$_{class}.{name}")).ok_or(format!("no ivar {class}.{name}"))?;
            addr.insert(&*Box::leak(format!("{class}.{name}").into_boxed_str()), a);
        }
        // Optional: where a blockhead is and whose it is, for the voice chat's overhead icons.
        for (class, name) in OPTIONAL_IVARS {
            if let Some(a) = export(&format!("OBJC_IVAR_$_{class}.{name}")) {
                addr.insert(&*Box::leak(format!("{class}.{name}").into_boxed_str()), a);
            }
        }
        Ok(Ivars { store_strong, addr })
    }

    /// The offset of an ivar: `Class.name`, or just `name` for the ones of `Blockhead` and `DynamicObject`.
    pub(crate) fn has(&self, name: &str) -> bool {
        self.addr.contains_key(name)
    }

    pub(crate) fn offset(&self, c: &mut Cpu, name: &str) -> u32 {
        let a = [name.to_string(), format!("Blockhead.{name}"), format!("DynamicObject.{name}")]
            .iter()
            .find_map(|k| self.addr.get(k.as_str()).copied());
        a.and_then(|a| c.mem().read_u32(a).ok()).unwrap_or(0)
    }
}

#[derive(Default)]
struct State {
    content: Content,
    /// The last serial of the cosmetics each blockhead (by address) was refreshed for.
    seen: HashMap<u32, u64>,
    /// `CPTexture2D` objects made from the mods' textures.
    textures: BTreeMap<(u64, Part), u32>,
    /// `BHPC_DEV_COSMETICS`: slot to key.
    dev: Vec<(String, String)>,
    failed: BTreeMap<(u64, Part), ()>,
    /// The game's own texture of every ivar a cosmetic replaced (retained), by blockhead and ivar.
    originals: HashMap<(u32, &'static str), u32>,
    /// Vanilla `InventoryItem`s lent to empty slots, by item type.
    items: HashMap<u16, u32>,
    /// The item type each lent slot held before (by blockhead and slot), put back before the next rebuild.
    lent: HashMap<(u32, usize), u16>,
    /// This client's own blockheads (retained: the frame refreshes them on the main thread, where their update and
    /// drawing never run on PC), with the frames left before the next refresh.
    own: HashMap<u32, u32>,
    /// The cosmetics serial the own blockheads were last queued for.
    own_serial: u64,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn log(msg: &str) {
    eprintln!("[cosmetics] {msg}");
}

/// `BHPC_COSMETICS_TRACE=1`: log what the hooks do (a dev aid).
fn trace(msg: impl FnOnce() -> String) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("BHPC_COSMETICS_TRACE").is_some_and(|v| v == "1")) {
        eprintln!("[cosmetics] {}", msg());
    }
}

fn on_main() -> bool {
    sched::ctx().tid == 0
}

fn active() -> bool {
    STATE.with(|s| s.borrow().content.is_active())
}

/// Reads `BHPC_DEV_COSMETICS`.
pub fn dev_from_env() -> Vec<(String, String)> {
    let Some(v) = std::env::var_os("BHPC_DEV_COSMETICS") else { return Vec::new() };
    v.to_string_lossy()
        .split(',')
        .filter_map(|p| p.split_once('=').map(|(s, k)| (s.trim().to_string(), k.trim().to_string())))
        .collect()
}

/// A session starts: reads the cosmetics of the session's packages (`.bhmod` files and folders in `dir`), with the
/// server's registry; with none (single-player) and the dev aid on, ids come from an in-memory registry and the dev
/// cosmetics are put on every blockhead.
pub fn load(dir: &std::path::Path, server_registry: &Wire) {
    let mut packages: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_dir() || p.extension().is_some_and(|e| e == "bhmod"))
                .collect()
        })
        .unwrap_or_default();
    packages.sort();
    let dev = dev_from_env();
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let mut wire = server_registry.clone();
        if wire.is_empty() && !dev.is_empty() {
            wire = bhmod_content::client::local_registry(&packages);
        }
        let dev_items = crate::items::dev_from_env();
        if wire.is_empty() && (!dev_items.is_empty() || crate::blocks::dev_key().is_some()) {
            wire = bhmod_content::client::local_registry(&packages);
        }
        let problems = s.content.load(&wire, &packages);
        crate::items::load_dev(&s.content, &dev_items);
        for p in problems {
            log(&p);
        }
        s.seen.clear();
        s.dev = dev.clone();
        if !dev.is_empty() {
            let ids = wire.get("cosmetics");
            let slots = dev.iter().filter_map(|(slot, key)| Some((slot.clone(), *ids?.get(key)?))).collect();
            let msg = Cosmetics { full: true, players: [("*".to_string(), slots)].into() };
            s.content.apply(&msg);
            log(&format!("dev cosmetics: {dev:?}"));
        }
        if s.content.is_active() {
            log(&format!("{} cosmetic(s) loaded", s.content.table().all().count()));
        }
        crate::items::load(&mut s.content, &crate::items::language());
        crate::blocks::load(&mut s.content);
        crate::blocks::load_dev(&s.content, crate::blocks::dev_key().as_deref());
    });
}

/// `COSMETICS` from the server.
pub fn apply(msg: &Cosmetics) {
    STATE.with(|s| {
        let changed = s.borrow_mut().content.apply(msg);
        if !changed.is_empty() {
            log(&format!("{} player(s) changed their look", changed.len()));
        }
    });
}

/// The session ends: nobody wears anything, blockheads are looked at again next time.
pub fn clear() {
    crate::weapons::drawing(0);
    crate::items::clear();
    crate::blocks::clear();
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.content.clear();
        s.seen.clear();
        s.lent.clear();
        // Their retain is left to the ended session (a few objects).
        s.own.clear();
    });
}

/// The Objective-C helper and ivars for [`frame`] (set where the hooks are registered, maybe on another thread).
static OWN_GLUE: std::sync::OnceLock<(Arc<Objc>, Ivars)> = std::sync::OnceLock::new();

/// Frames between a blockhead's creation and its refresh: the game makes the clothing textures in between.
const OWN_DELAY: u32 = 3;

/// Once per frame on the main thread: refreshes this client's own blockheads a few frames after they appeared and
/// whenever the cosmetics changed (on PC their update and drawing run on other threads, where the hooks skip).
pub(crate) fn frame(c: &mut Cpu) {
    if !on_main() || !active() {
        return;
    }
    let Some((o, iv)) = OWN_GLUE.get().cloned() else { return };
    let due: Vec<u32> = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let serial = s.content.serial();
        if serial != s.own_serial {
            s.own_serial = serial;
            s.own.values_mut().for_each(|n| *n = (*n).max(1));
        }
        let mut due = Vec::new();
        for (bh, n) in s.own.iter_mut() {
            if *n > 0 {
                *n -= 1;
                if *n == 0 {
                    due.push(*bh);
                }
            }
        }
        due
    });
    for bh in due {
        // Its creation already counted as seen, before the game had made the textures: rebuild it anyway.
        STATE.with(|s| s.borrow_mut().seen.remove(&bh));
        refresh(&o, c, &iv, bh);
    }
}

/// Remembers one of this client's own blockheads for [`frame`] (retained once).
fn note_own(o: &Objc, c: &mut Cpu, iv: &Ivars, bh: u32) {
    if is_net(c, iv, bh) || STATE.with(|s| s.borrow().own.contains_key(&bh)) {
        return;
    }
    o.send(c, bh, "retain", &[]);
    STATE.with(|s| s.borrow_mut().own.insert(bh, OWN_DELAY));
}

/// Which parts of a blockhead to refresh.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Which {
    Clothing,
    Hair,
    Both,
}

const CLOTHING_IVARS: [&str; 5] = ["hatTexture", "shirtBodyTexture", "shirtArmTexture", "pantsTexture", "shoesTexture"];

impl Which {
    /// The texture ivars this part covers.
    fn ivars(self) -> &'static [&'static str] {
        match self {
            Which::Clothing => &CLOTHING_IVARS,
            Which::Hair => &["headHairTexture"],
            Which::Both => &[
                "hatTexture",
                "shirtBodyTexture",
                "shirtArmTexture",
                "pantsTexture",
                "shoesTexture",
                "headHairTexture",
            ],
        }
    }
}

/// The owner of a blockhead (`clientID`), or `*` with the dev aid.
fn player_of(o: &Objc, c: &mut Cpu, iv: &Ivars, bh: u32, dev: bool) -> String {
    if dev {
        return "*".into();
    }
    let off = iv.offset(c, "clientID");
    let id = c.mem().read_u32(bh + off).unwrap_or(0);
    let id = if id == 0 { String::new() } else { o.string(c, id) };
    if id.is_empty() && !is_net(c, iv, bh) {
        // The game leaves `clientID` empty on this client's own blockheads: they belong to the local player.
        return crate::modglue::local_player_id(o, c);
    }
    id
}

fn worn(c: &mut Cpu, iv: &Ivars, bh: u32) -> [Option<u16>; 4] {
    let names = ["shoesItemType", "pantsItemType", "shirtItemType", "hatItemType"];
    names.map(|n| {
        let off = iv.offset(c, n);
        let v = c.mem().read_u16(bh + off).unwrap_or(0);
        (v != 0).then_some(v)
    })
}

/// Replaces the texture in `ivar` with `obj`, keeping the game's own texture to put back later.
fn store(o: &Objc, c: &mut Cpu, iv: &Ivars, bh: u32, ivar: &'static str, obj: u32) {
    let off = iv.offset(c, ivar);
    if obj == 0 || off == 0 {
        return;
    }
    let first = STATE.with(|s| !s.borrow().originals.contains_key(&(bh, ivar)));
    if first {
        let cur = c.mem().read_u32(bh + off).unwrap_or(0);
        if cur != 0 {
            o.send(c, cur, "retain", &[]);
            STATE.with(|s| s.borrow_mut().originals.insert((bh, ivar), cur));
        }
    }
    let before = c.mem().read_u32(bh + off).unwrap_or(0);
    let _ = c.call(iv.store_strong, &[bh + off, obj]);
    let after = c.mem().read_u32(bh + off).unwrap_or(0);
    trace(|| format!("store {bh:#x}.{ivar} (+{off:#x}): {before:#x} -> {obj:#x}, now {after:#x}"));
}

/// Puts the game's own textures back into a blockhead (`keep`: leave them in place and only drop what was saved,
/// because the game just rebuilt them).
fn restore(o: &Objc, c: &mut Cpu, iv: &Ivars, bh: u32, which: Which, keep: bool) {
    let mine = which.ivars();
    let saved: Vec<(&'static str, u32)> = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let keys: Vec<_> = s.originals.keys().filter(|k| k.0 == bh && mine.contains(&k.1)).copied().collect();
        keys.into_iter().filter_map(|k| s.originals.remove(&k).map(|v| (k.1, v))).collect()
    });
    for (ivar, obj) in saved {
        let off = iv.offset(c, ivar);
        if !keep && off != 0 {
            let _ = c.call(iv.store_strong, &[bh + off, obj]);
        }
        o.send(c, obj, "release", &[]);
    }
}

/// A `CPTexture2D` for a cosmetic's texture (made once; 0 when the texture cannot be loaded).
fn texture(o: &Objc, c: &mut Cpu, id: u64, part: Part) -> u32 {
    if let Some(t) = STATE.with(|s| s.borrow().textures.get(&(id, part)).copied()) {
        return t;
    }
    let gpu = STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.failed.contains_key(&(id, part)) {
            return None;
        }
        match s.content.texture(id, part) {
            Ok(g) => g,
            Err(why) => {
                log(&why);
                s.failed.insert((id, part), ());
                None
            }
        }
    });
    let Some(gpu) = gpu else { return 0 };
    let tex = make_texture(o, c, &gpu);
    trace(|| {
        format!(
            "texture ({id},{part:?}) {}x{} (content {}x{}) = {tex:#x}",
            gpu.wide, gpu.high, gpu.content_wide, gpu.content_high
        )
    });
    if tex == 0 {
        log("CPTexture2D could not be created");
    }
    STATE.with(|s| s.borrow_mut().textures.insert((id, part), tex));
    tex
}

fn make_texture(o: &Objc, c: &mut Cpu, g: &Arc<bhmod_content::Gpu>) -> u32 {
    let data_cls = o.class(c, "NSMutableData");
    let alloc = o.send(c, data_cls, "alloc", &[]);
    let data = o.send(c, alloc, "initWithLength:", &[g.pixels.len() as u32]);
    let ptr = o.send(c, data, "mutableBytes", &[]);
    if ptr == 0 || c.mem().write_bytes(ptr, &g.pixels).is_err() {
        o.send(c, data, "release", &[]);
        return 0;
    }
    let cls = o.class(c, "CPTexture2D");
    let tex = o.send(c, cls, "alloc", &[]);
    // The game's own pixel format (1: premultiplied RGBA bytes), contentSize as two floats, no mipmaps.
    let tex = o.send(
        c,
        tex,
        "initWithData:pixelFormat:pixelsWide:pixelsHigh:contentSize:mipmap:",
        &[
            ptr,
            bhmod_content::PIXEL_FORMAT,
            g.wide,
            g.high,
            (g.content_wide as f32).to_bits(),
            (g.content_high as f32).to_bits(),
            0,
        ],
    );
    // The data object stays alive (one per texture): the game may read the pixels again later.
    tex
}

/// A vanilla `InventoryItem` of `item_type` (made once) to put in an empty clothing slot.
fn item(o: &Objc, c: &mut Cpu, item_type: u16) -> u32 {
    if let Some(i) = STATE.with(|s| s.borrow().items.get(&item_type).copied()) {
        return i;
    }
    let cls = o.class(c, "InventoryItem");
    let alloc = o.send(c, cls, "alloc", &[]);
    let it = o.send(
        c,
        alloc,
        "initWithType:dataA:dataB:subItems:dynamicObjectSaveDict:",
        &[u32::from(item_type), 0, 0, 0, 0],
    );
    STATE.with(|s| s.borrow_mut().items.insert(item_type, it));
    it
}

/// Lends plain vanilla items to the empty clothing slots a cosmetic needs a shape for (auto-wear). The game's
/// `updateClothingCubes` reads `inventoryItems[0]` (the clothing group item) `.subItems[slot][0]`; the item is put
/// into that slot's array for the duration of the call. Returns the slot arrays to empty again afterwards.
fn lend_items(o: &Objc, c: &mut Cpu, iv: &Ivars, bh: u32, player: &str) -> Vec<u32> {
    let names = ["shoesItemType", "pantsItemType", "shirtItemType", "hatItemType"];
    // The game keeps the item type of a slot that went empty: undo the last lending, or a removed cosmetic stays.
    let undo: Vec<_> = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let keys: Vec<_> = s.lent.keys().filter(|k| k.0 == bh).copied().collect();
        keys.into_iter().filter_map(|k| s.lent.remove(&k).map(|v| (k.1, v))).collect()
    });
    for (i, was) in undo {
        let off = iv.offset(c, names[i]);
        let _ = c.mem().write_u16(bh + off, was);
    }
    let off = iv.offset(c, "inventoryItems");
    let inv = c.mem().read_u32(bh + off).unwrap_or(0);
    if inv == 0 {
        return Vec::new();
    }
    let group_slot = o.send(c, inv, "objectAtIndex:", &[0]);
    if group_slot == 0 || o.send(c, group_slot, "count", &[]) == 0 {
        return Vec::new();
    }
    let group = o.send(c, group_slot, "objectAtIndex:", &[0]);
    let subs = o.send(c, group, "subItems", &[]);
    let n = o.send(c, subs, "count", &[]);
    let mut lent = Vec::new();
    for (i, slot) in CLOTHING.iter().enumerate() {
        if i as u32 >= n {
            break;
        }
        let Some(wear) = STATE.with(|s| s.borrow().content.resolve(player, slot, None).and_then(|r| r.wear)) else {
            continue;
        };
        let arr = o.send(c, subs, "objectAtIndex:", &[i as u32]);
        if arr == 0 || o.send(c, arr, "count", &[]) != 0 {
            continue;
        }
        let it = item(o, c, wear);
        o.send(c, arr, "addObject:", &[it]);
        let off = iv.offset(c, names[i]);
        let was = c.mem().read_u16(bh + off).unwrap_or(0);
        STATE.with(|s| s.borrow_mut().lent.insert((bh, i), was));
        let _ = c.mem().write_u16(bh + off, wear);
        trace(|| format!("lent item {wear} to {bh:#x} slot {i}"));
        lent.push(arr);
    }
    lent
}

/// Writes the cosmetics' textures into a blockhead.
fn apply_to(o: &Objc, c: &mut Cpu, iv: &Ivars, bh: u32, which: Which) {
    // The game just rebuilt (some of) these textures: what was saved of them is stale.
    restore(o, c, iv, bh, which, true);
    let dev = STATE.with(|s| !s.borrow().dev.is_empty());
    let player = player_of(o, c, iv, bh, dev);
    let w = worn(c, iv, bh);
    let (look, serial) = STATE.with(|s| {
        let s = s.borrow();
        (s.content.look(&player, w), s.content.serial())
    });
    STATE.with(|s| s.borrow_mut().seen.insert(bh, serial));
    if look.is_empty() {
        return;
    }
    static LOGGED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if LOGGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 12 {
        log(&format!("blockhead {bh:#x} of {player:?} wears {look:?} (worn {w:?})"));
    }
    if which != Which::Hair {
        for (i, slot) in CLOTHING.iter().enumerate() {
            let Some((id, _)) = look.clothing[i] else { continue };
            let (main, arm) = match *slot {
                "shirt" => ("shirtBodyTexture", Some("shirtArmTexture")),
                "hat" => ("hatTexture", None),
                "pants" => ("pantsTexture", None),
                _ => ("shoesTexture", None),
            };
            let t = texture(o, c, id, Part::Main);
            store(o, c, iv, bh, main, t);
            if let Some(arm) = arm {
                let t = texture(o, c, id, Part::Arm);
                store(o, c, iv, bh, arm, t);
            }
        }
    }
    if which != Which::Clothing {
        if let Some(id) = look.hair {
            let t = texture(o, c, id, Part::Main);
            store(o, c, iv, bh, "headHairTexture", t);
        }
    }
}

thread_local! {
    /// What the weapon-skin hooks need from the game (set by [`register`], used on the main thread).
    static GLUE: RefCell<Option<(Arc<Objc>, Ivars)>> = const { RefCell::new(None) };
}

/// True while a loaded mod declares a weapon cosmetic.
pub fn has_weapons() -> bool {
    STATE.with(|s| s.borrow().content.has_weapons())
}

/// The skin of a held sprite item (`item`, an `ItemType`) on blockhead `bh`: the GL name of the mod's texture and
/// its rectangle `[u, v, w, h]`. `dev_item`: the dev aid, the item the player's weapon cosmetic shows on whatever
/// its `base` says.
pub fn weapon_skin(c: &mut Cpu, bh: u32, item: u16, dev_item: Option<u16>) -> Option<(u32, [f32; 4])> {
    if !on_main() {
        return None;
    }
    let (o, iv) = GLUE.with(|g| g.borrow().clone())?;
    let dev = STATE.with(|s| !s.borrow().dev.is_empty());
    let player = player_of(&o, c, &iv, bh, dev);
    let id = STATE.with(|s| {
        let s = s.borrow();
        match dev_item {
            Some(d) if d == item => s.content.worn_in(&player, "weapon"),
            _ => s.content.weapon(&player, item),
        }
    })?;
    let tex = texture(&o, c, id, Part::Main);
    if tex == 0 {
        return None;
    }
    let uv = STATE.with(|s| s.borrow_mut().content.texture(id, Part::Main).ok().flatten().map(|g| g.uv()))?;
    let name = o.send(c, tex, "name", &[]);
    (name != 0).then_some((name, uv))
}

/// While weapon skins are installed the held item and portrait drawing calls runs inside its window: the original runs here and the
/// window is closed after it (the atlas bound back, no current blockhead), so nothing drawn afterwards (the dig-target
/// indicator, the HUD) is skinned. Without weapon skins the hook stays a plain before hook.
fn weapon_window(inv: &mut crate::objchook::Invocation<'_>) {
    if crate::weapons::active() {
        let _ = inv.call_original();
        crate::weapons::leave(inv.cpu());
    }
}

/// Registers the hooks.
pub(crate) fn register(hooks: &ObjcHooks, o: Arc<Objc>, iv: Ivars) {
    let t = |s: &str| Target::parse(s).expect("valid target");
    GLUE.with(|g| *g.borrow_mut() = Some((o.clone(), iv.clone())));
    let _ = OWN_GLUE.set((o.clone(), iv.clone()));
    // Which blockhead is drawing (weapon skins): before hooks, nothing is changed. The matrices are unions
    // (`v156@0:4f8(_GLKMatrix4=...)12(...)76i140i144i148i152`), which a runtime signature cannot describe.
    let matrix = crate::objchook::Ty::Struct { words: 16, align8: false };
    let draw = crate::objchook::Sig {
        ret: crate::objchook::Ty::Void,
        args: vec![
            crate::objchook::Ty::F32,
            matrix,
            matrix,
            crate::objchook::Ty::Word,
            crate::objchook::Ty::Word,
            crate::objchook::Ty::Word,
            crate::objchook::Ty::Word,
        ],
    };
    hooks.hook(
        t("-[Blockhead draw:projectionMatrix:modelViewMatrix:cameraMinXWorld:cameraMaxXWorld:cameraMinYWorld:cameraMaxYWorld:]"),
        Some(draw),
        move |inv| {
            if on_main() {
                // The body: a plain before hook (a nested original drops the body of the blockhead on screen; the
                // body never draws a sprite item, and the next call from outside ends the window).
                crate::weapons::entered("draw", inv.receiver());
                // Cosmetics: the blockhead's update runs on a game thread, its drawing here.
                if let Some((o, iv)) = GLUE.with(|g| g.borrow().clone()) {
                    let bh = inv.receiver();
                    refresh(&o, inv.cpu(), &iv, bh);
                    if crate::modglue::overheads_wanted() {
                        note_position(&o, inv.cpu(), &iv, bh);
                    }
                }
            }
        },
    );
    // The other entry points the game draws a blockhead with: the item in its hand while it is used (named like an
    // inventory preview in the game) and the portrait in the HUD.
    for target in [
        "-[Blockhead drawForButtonProjectionMatrix:modelViewMatrix:]",
        "-[Blockhead drawTransparentInventoryItem:projectionMatrix:modelViewMatrix:cameraMinXWorld:cameraMaxXWorld:cameraMinYWorld:cameraMaxYWorld:]",
    ] {
        hooks.hook(t(target), Some(other_draw(target)), move |inv| {
            if on_main() {
                crate::weapons::entered(if target.contains("Button") { "button" } else { "ghost" }, inv.receiver());
                weapon_window(inv);
            }
        });
    }
    // Dev trace: the first pixels the game itself hands to CPTexture2D (to compare with the PNG).
    hooks.hook(
        t("-[CPTexture2D initWithData:pixelFormat:pixelsWide:pixelsHigh:contentSize:mipmap:]"),
        None,
        move |inv| {
            if let Some(dir) = std::env::var_os("BHPC_DUMP_TEXTURES") {
                // Dev aid: the pixels the game hands to CPTexture2D (big ones), as raw RGBA next to a name that says
                // the size and the mipmap flag.
                let (p, w, h, m) = (
                    inv.arg(0).map_or(0, |v| v.word()),
                    inv.arg(2).map_or(0, |v| v.word()),
                    inv.arg(3).map_or(0, |v| v.word()),
                    inv.arg(5).map_or(0, |v| v.word()),
                );
                if w >= 256 && h >= 256 && w * h * 4 <= 64 << 20 {
                    if let Ok(px) = inv.cpu().mem().read_bytes(p, w * h * 4) {
                        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let f = std::path::Path::new(&dir).join(format!("tex{n:03}_{w}x{h}_m{m}.rgba"));
                        let _ = std::fs::write(f, px);
                    }
                }
            }
            crate::blocks::observe_texture(
                inv.receiver(),
                inv.arg(2).map_or(0, |v| v.word()),
                inv.arg(3).map_or(0, |v| v.word()),
                inv.arg(5).map_or(0, |v| v.word()),
            );
            crate::items::observe_texture(
                inv.receiver(),
                inv.arg(2).map_or(0, |v| v.word()),
                inv.arg(3).map_or(0, |v| v.word()),
                inv.arg(5).map_or(0, |v| v.word()),
            );
            let on = std::env::var_os("BHPC_COSMETICS_TRACE").is_some_and(|v| v == "2");
            if !on {
                return;
            }
            let (p, f, w, h) = (
                inv.arg(0).map_or(0, |v| v.word()),
                inv.arg(1).map_or(0, |v| v.word()),
                inv.arg(2).map_or(0, |v| v.word()),
                inv.arg(3).map_or(0, |v| v.word()),
            );
            let bytes = inv.cpu().mem().read_bytes(p, 48).unwrap_or_default();
            let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            eprintln!("[cosmetics] CPTexture2D init fmt {f} {w}x{h}: {hex}");
        },
    );
    let (oo, ii) = (o.clone(), iv.clone());
    hooks.hook(t("-[Blockhead updateClothingCubes]"), None, move |inv| {
        if !on_main() || !active() {
            return;
        }
        let bh = inv.receiver();
        let dev = STATE.with(|s| !s.borrow().dev.is_empty());
        let player = player_of(&oo, inv.cpu(), &ii, bh, dev);
        note_own(&oo, inv.cpu(), &ii, bh);
        let lent = lend_items(&oo, inv.cpu(), &ii, bh, &player);
        let r = inv.call_original();
        for arr in lent {
            oo.send(inv.cpu(), arr, "removeAllObjects", &[]);
        }
        trace(|| format!("updateClothingCubes of {bh:#x}: {r:?}"));
        apply_to(&oo, inv.cpu(), &ii, bh, Which::Clothing);
    });
    let (oo, ii) = (o.clone(), iv.clone());
    hooks.hook(t("-[Blockhead updateSkin]"), None, move |inv| {
        if !on_main() || !active() {
            return;
        }
        let bh = inv.receiver();
        let _ = inv.call_original();
        apply_to(&oo, inv.cpu(), &ii, bh, Which::Hair);
    });
    let (oo, ii) = (o.clone(), iv.clone());
    hooks.hook(t("-[Blockhead setupFromNetCreationData:]"), None, move |inv| {
        if !on_main() || !active() {
            return;
        }
        let bh = inv.receiver();
        let _ = inv.call_original();
        apply_to(&oo, inv.cpu(), &ii, bh, Which::Both);
    });
    // The per-frame triggers: a blockhead is refreshed the first time it is seen and whenever the cosmetics changed.
    // Its update runs on a game thread on PC (the hook then only does the dev aids), its drawing on the main thread.
    let (oo, ii) = (o.clone(), iv.clone());
    hooks.hook(t("-[Blockhead update:accurateDT:isSimulation:]"), None, move |inv| {
        if on_main() {
            let bh = inv.receiver();
            let net = is_net(inv.cpu(), &ii, bh);
            crate::items::dev_give(&oo, inv.cpu(), bh, net);
            crate::blocks::dev_place(&oo, inv.cpu(), &ii, bh, net);
            refresh(&oo, inv.cpu(), &ii, bh);
        }
    });
}

/// Brings a blockhead up to date with the cosmetics: the first time it is seen (its creation built the clothing
/// before the game could make the textures) and whenever they changed.
fn refresh(o: &Objc, c: &mut Cpu, iv: &Ivars, bh: u32) {
    if !active() {
        return;
    }
    let (serial, seen) = STATE.with(|s| {
        let s = s.borrow();
        (s.content.serial(), s.seen.get(&bh).copied())
    });
    if seen == Some(serial) {
        return;
    }
    STATE.with(|s| s.borrow_mut().seen.insert(bh, serial));
    let dev = STATE.with(|s| !s.borrow().dev.is_empty());
    let player = player_of(o, c, iv, bh, dev);
    let w = worn(c, iv, bh);
    let look = STATE.with(|s| s.borrow().content.look(&player, w));
    let had = STATE.with(|s| s.borrow().originals.keys().any(|k| k.0 == bh));
    trace(|| format!("refresh {bh:#x} of {player:?}: {look:?} (had overrides: {had}, net: {})", is_net(c, iv, bh)));
    if look.is_empty() && !had {
        return;
    }
    if is_net(c, iv, bh) {
        // Other players' blockheads are never rebuilt by the game: put the old textures back, then override.
        restore(o, c, iv, bh, Which::Both, false);
        apply_to(o, c, iv, bh, Which::Both);
    } else {
        o.send(c, bh, "updateClothingCubes", &[]);
        o.send(c, bh, "updateSkin", &[]);
    }
}

fn is_net(c: &mut Cpu, iv: &Ivars, bh: u32) -> bool {
    let off = iv.offset(c, "isNet");
    c.mem().read_u8(bh + off).unwrap_or(0) != 0
}

/// The signatures of the other two draw entry points (matrices are unions, so they are written by hand).
fn other_draw(target: &str) -> crate::objchook::Sig {
    use crate::objchook::{Sig, Ty};
    let matrix = Ty::Struct { words: 16, align8: false };
    if target.contains("drawForButton") {
        return Sig { ret: Ty::Void, args: vec![matrix, matrix] };
    }
    Sig { ret: Ty::Void, args: vec![Ty::F32, matrix, matrix, Ty::Word, Ty::Word, Ty::Word, Ty::Word] }
}

/// Where a drawn blockhead's head is (world tiles: `floatPos` is the middle of its feet, the body is two tiles tall)
/// and whose it is (`clientName`, the alias the PC bridge names players by; this client's own blockheads have none),
/// for the overhead icons.
fn note_position(o: &Objc, c: &mut Cpu, iv: &Ivars, bh: u32) {
    if !iv.has("DynamicObject.floatPos") || !iv.has("Blockhead.clientName") {
        return;
    }
    let off = iv.offset(c, "floatPos");
    let (Ok(x), Ok(y)) = (c.mem().read_u32(bh + off), c.mem().read_u32(bh + off + 4)) else { return };
    let (x, y) = (f32::from_bits(x), f32::from_bits(y));
    if !x.is_finite() || !y.is_finite() {
        return;
    }
    let name_off = iv.offset(c, "clientName");
    let name = c.mem().read_u32(bh + name_off).unwrap_or(0);
    let mut alias = if name == 0 { String::new() } else { o.string(c, name) };
    if alias.is_empty() && !is_net(c, iv, bh) {
        alias = crate::modglue::MAIN.with(|m| m.borrow().local.clone());
    }
    let head = (x, y + bhpc_mods::overhead::HEAD_LIFT);
    // Mods name players by alias on PC, by id (`clientID`) once voice chat lists them: both are kept.
    let id = player_of(o, c, iv, bh, false);
    if !id.is_empty() && id != alias {
        crate::modglue::note_head(id, head);
    }
    if !alias.is_empty() {
        crate::modglue::note_head(alias, head);
    }
}

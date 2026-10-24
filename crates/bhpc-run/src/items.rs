//! Items of content mods in the play build (mod system M8): what the game shows for an item type above the vanilla
//! ones, and that the game accepts such an item in an inventory.
//!
//! The ids come from the server's registry (`bhmod_content::Content`), the names from the mods' manifests. Two kinds
//! of hooks, all inline hooks of the runner (owner [`OWNER`]), installed while a loaded mod declares an item and only
//! on the game build whose function prologues match:
//!
//! - **Names.** The game builds an item's title with `NSStringFromItemType`, a static function the compiler
//!   duplicated into twelve compile units ([`NAME_FUNCS`], offsets in `libApplication.so` of the 1.7.6 build; the
//!   plural form calls it for every id above the vanilla ones). Each copy is replaced by a thunk that answers an
//!   `NSString` for a mod item and runs the original for everything else.
//! - **Icons.** The inventory asks `imageTypeForItemType` (exported) for the atlas cell of an item. For a mod item it
//!   answers a free cell of the item atlas (`Items.png`, [`bhmod_content::item_cell`]) and the mod's picture is
//!   copied into that cell of the atlas texture on first use (`glTexSubImage2D`, then the mipmaps again). The atlas
//!   is the `CPTexture2D` the game made with a 2:1 size and mipmaps, noticed when it is created.
//! - **Validity.** `itemTypeIsValidFillItem` (exported) is false for every id above the vanilla ones, so the game
//!   would refuse the item in an inventory; the replacement says yes for the mod's item ids.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};

use bhmod_content::Content;
use bhpc_cpu::Cpu;

use crate::modglue::Objc;

/// The engine owner of these hooks.
const OWNER: u32 = u32::MAX - 2;
/// `NSStringFromItemType` in each of the twelve compile units that have a copy.
const NAME_FUNCS: [u32; 12] = [
    0x4d_b268, 0x4e_430c, 0x5d_faa8, 0x63_e970, 0x66_a300, 0x69_ff94, 0x71_913c, 0x72_c10c, 0x74_a4a8, 0xa8_64d0,
    0xb7_6660, 0xce_c8e4,
];
/// The first two instructions of each copy (`sub sp, sp, #24; ldr r1, [pc, #0xffc]`).
const NAME_PROLOGUE: [u32; 2] = [0xe24d_d018, 0xe59f_1ffc];
const VALID_FILL: &str = "_Z23itemTypeIsValidFillItem8ItemType";

const TEXTURE_2D: u32 = 0xde1;
const TEXTURE_BINDING_2D: u32 = 0x8069;
const RGBA: u32 = 0x1908;
const UNSIGNED_BYTE: u32 = 0x1401;

/// What the icon upload needs: the game's GL functions, scratch memory and the pictures.
pub(crate) struct Gl {
    bind: u32,
    mipmap: u32,
    get_int: u32,
    sub_image: u32,
    scratch: u32,
}

impl Gl {
    /// The game's GL functions through its own imports (`None` when one is missing).
    pub(crate) fn new(c: &mut Cpu, n: &crate::native::Natives) -> Option<Gl> {
        Some(Gl {
            bind: n.import_target(c, "libApplication.so", "__wrap_glBindTexture")?,
            mipmap: n.import_target(c, "libApplication.so", "__wrap_glGenerateMipmap")?,
            get_int: n.import_target(c, "libApplication.so", "__wrap_glGetIntegerv")?,
            sub_image: c.thunk_addr("glTexSubImage2D")?,
            scratch: n.scratch(c, 64 * 64 * 4 + 16).ok()?,
        })
    }

    /// Copies `cells` (`(cell, 64 px picture, 16 px picture)`) into the atlas texture `name` that is `wide` pixels
    /// wide (32 cells per row), rebuilds its mipmaps and binds the texture that was bound before.
    pub(crate) fn blit(&self, c: &mut Cpu, name: u32, wide: u32, cells: &[(u32, Vec<u8>, Vec<u8>)]) {
        let columns = bhmod_content::items::ATLAS_COLUMNS;
        let size = wide / columns;
        let before = if c.call(self.get_int, &[TEXTURE_BINDING_2D, self.scratch]).is_ok() {
            c.mem().read_u32(self.scratch).unwrap_or(0)
        } else {
            0
        };
        let _ = c.call(self.bind, &[TEXTURE_2D, name]);
        for (cell, hd, sd) in cells {
            let px = if size == 64 { hd } else { sd };
            let (x, y) = ((cell % columns) * size, (cell / columns) * size);
            if c.mem().write_bytes(self.scratch, px).is_err() {
                continue;
            }
            let _ = c.call(self.sub_image, &[TEXTURE_2D, 0, x, y, size, size, RGBA, UNSIGNED_BYTE, self.scratch]);
        }
        let _ = c.call(self.mipmap, &[TEXTURE_2D]);
        let _ = c.call(self.bind, &[TEXTURE_2D, before]);
    }
}

/// The pictures of the mod items' icons: `(cell, premultiplied RGBA at 64 px, at 16 px)` (HD and SD atlas cells).
type Icons = Vec<(u32, Vec<u8>, Vec<u8>)>;
static ICONS: Mutex<Icons> = Mutex::new(Vec::new());
static GL: Mutex<Option<Gl>> = Mutex::new(None);
/// The `CPTexture2D` of the item atlas (0: not seen) and its size, and whether the pictures are in it.
static ATLAS: AtomicU32 = AtomicU32::new(0);
static ATLAS_WIDE: AtomicU32 = AtomicU32::new(0);
static UPLOADED: AtomicU32 = AtomicU32::new(0);

static NAMES: RwLock<Option<HashMap<u16, String>>> = RwLock::new(None);
/// Retained `NSString`s made for the names (by item type).
static STRINGS: Mutex<Option<HashMap<u16, u32>>> = Mutex::new(None);
static ORIGS: [AtomicU32; 12] = [const { AtomicU32::new(0) }; 12];
static VALID_ORIG: AtomicU32 = AtomicU32::new(0);
static OBJC: Mutex<Option<Arc<Objc>>> = Mutex::new(None);
static THUNKS: OnceLock<Vec<u32>> = OnceLock::new();
static STATE: AtomicU32 = AtomicU32::new(OFF);
const OFF: u32 = 0;
const ON: u32 = 1;
const GAVE_UP: u32 = 2;

fn log(msg: &str) {
    eprintln!("[items] {msg}");
}

/// `BHPC_DEV_ITEMS=<key>=<count>,...`: items of the `--mods-dev` folders put into the inventory of this client's
/// blockhead (single-player; the ids come from an in-memory registry), for testing without a server.
pub fn dev_from_env() -> Vec<(String, u32)> {
    let Some(v) = std::env::var_os("BHPC_DEV_ITEMS") else { return Vec::new() };
    v.to_string_lossy()
        .split(',')
        .filter_map(|p| {
            let (k, n) = p.split_once('=').unwrap_or((p, "1"));
            Some((k.trim().to_string(), n.trim().parse().ok()?))
        })
        .collect()
}

thread_local! {
    /// Blockheads (by address) that got their dev items.
    static GIVEN: std::cell::RefCell<std::collections::HashSet<u32>> = std::cell::RefCell::new(Default::default());
    /// `(item type, count)` of the dev items.
    static DEV: std::cell::RefCell<Vec<(u16, u32)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Remembers the dev items by id (called when a session's content loads).
pub fn load_dev(content: &Content, dev: &[(String, u32)]) {
    let ids = dev
        .iter()
        .filter_map(|(k, n)| Some((u16::try_from(content.items().all().find(|i| &i.key == k)?.id).ok()?, *n)))
        .collect();
    DEV.with(|d| *d.borrow_mut() = ids);
    GIVEN.with(|g| g.borrow_mut().clear());
}

/// The per-frame hook of a blockhead: its first time, a non-net blockhead gets the dev items.
pub(crate) fn dev_give(o: &Objc, c: &mut Cpu, bh: u32, is_net: bool) {
    if is_net || DEV.with(|d| d.borrow().is_empty()) || !GIVEN.with(|g| g.borrow_mut().insert(bh)) {
        return;
    }
    let cls = o.class(c, "InventoryItem");
    for (item_type, count) in DEV.with(|d| d.borrow().clone()) {
        for _ in 0..count {
            let alloc = o.send(c, cls, "alloc", &[]);
            let it = o.send(
                c,
                alloc,
                "initWithType:dataA:dataB:subItems:dynamicObjectSaveDict:",
                &[u32::from(item_type), 0, 0, 0, 0],
            );
            let slot = o.send(c, bh, "addItemToInventory:", &[it]);
            if slot as i32 == -1 {
                log(&format!("the inventory refused item {item_type}"));
            }
        }
    }
    log(&format!("dev items given to {bh:#x}"));
}

/// The language the player reads (`LC_ALL`, `LC_MESSAGES`, then `LANG`; `en` without any).
pub fn language() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty() && v != "C" && v != "POSIX")
        .map_or_else(|| "en".to_string(), |v| v.chars().take_while(|c| c.is_ascii_alphabetic()).collect())
}

/// A session loaded `content`: remembers the names of its items (empty: no hooks next frame).
pub fn load(content: &mut Content, lang: &str) {
    let names: HashMap<u16, String> = content
        .items()
        .all()
        .filter_map(|i| Some((u16::try_from(i.id).ok()?, content.item_name(i.id, lang)?)))
        .collect();
    if !names.is_empty() {
        log(&format!("{} item(s) loaded", names.len()));
    }
    let ids: Vec<u64> = content.items().all().map(|i| i.id).collect();
    let mut icons = Vec::new();
    for id in ids {
        let Some(cell) = bhmod_content::item_cell(id) else { continue };
        match (content.item_cell_pixels(id, 64), content.item_cell_pixels(id, 16)) {
            (Ok(Some(hd)), Ok(Some(sd))) => icons.push((cell, hd, sd)),
            (Err(why), _) | (_, Err(why)) => log(&format!("icon of item {id}: {why}")),
            _ => {}
        }
    }
    *ICONS.lock().unwrap_or_else(PoisonError::into_inner) = icons;
    UPLOADED.store(0, Ordering::Relaxed);
    *NAMES.write().unwrap_or_else(PoisonError::into_inner) = Some(names);
    *STRINGS.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

/// The session ended.
pub fn clear() {
    ICONS.lock().unwrap_or_else(PoisonError::into_inner).clear();
    *NAMES.write().unwrap_or_else(PoisonError::into_inner) = None;
    *STRINGS.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

/// The game made a texture (`-[CPTexture2D initWithData:...]` is about to run on `obj`): remembers the item atlas.
pub fn observe_texture(obj: u32, wide: u32, high: u32, mipmap: u32) {
    if mipmap == 1 && wide == 2 * high && (wide == 2048 || wide == 512) {
        ATLAS.store(obj, Ordering::Relaxed);
        ATLAS_WIDE.store(wide, Ordering::Relaxed);
        UPLOADED.store(0, Ordering::Relaxed);
    }
}

fn has_items() -> bool {
    NAMES.read().unwrap_or_else(PoisonError::into_inner).as_ref().is_some_and(|n| !n.is_empty())
}

fn name_of(item: u32) -> Option<String> {
    let id = u16::try_from(item).ok()?;
    NAMES.read().unwrap_or_else(PoisonError::into_inner).as_ref()?.get(&id).cloned()
}

fn is_mod_item(item: u32) -> bool {
    u16::try_from(item).ok().is_some_and(|id| {
        NAMES.read().unwrap_or_else(PoisonError::into_inner).as_ref().is_some_and(|n| n.contains_key(&id))
    })
}

/// Installs or removes the hooks as items are loaded (once per frame on the main thread).
pub(crate) fn sync(c: &mut Cpu, o: &Arc<Objc>) {
    match (has_items(), STATE.load(Ordering::Relaxed)) {
        (true, OFF) => install(c, o),
        (false, ON) => remove(c),
        _ => {}
    }
}

/// Takes the hooks out (the session ends).
pub fn remove(c: &mut Cpu) {
    if STATE.load(Ordering::Relaxed) == ON {
        if let Some(n) = crate::native::get() {
            n.unhook_owner(c, OWNER);
        }
        STATE.store(OFF, Ordering::Relaxed);
        log("hooks removed");
    }
}

fn give_up(why: &str) {
    log(&format!("mod items are off: {why}"));
    STATE.store(GAVE_UP, Ordering::Relaxed);
}

fn install(c: &mut Cpu, o: &Arc<Objc>) {
    let Some(n) = crate::native::get() else { return };
    let Some(base) = n.module_base("libApplication.so") else { return };
    for f in NAME_FUNCS {
        let words: Vec<u32> = (0..2).map(|i| c.mem().read_u32(base + f + 4 * i).unwrap_or(0)).collect();
        if words != NAME_PROLOGUE {
            return give_up("this is not the game build they know");
        }
    }
    *OBJC.lock().unwrap_or_else(PoisonError::into_inner) = Some(o.clone());
    let thunks = THUNKS.get_or_init(|| {
        let mut v: Vec<u32> = (0..NAME_FUNCS.len())
            .map(|i| {
                let f = move |c: &mut Cpu| item_name(c, i);
                c.register_shared_thunk(&format!("bhmod:item_name_{i}"), Arc::new(f)).unwrap_or(0)
            })
            .collect();
        v.push(c.register_shared_thunk("bhmod:item_valid", Arc::new(item_valid)).unwrap_or(0));
        v
    });
    if thunks.contains(&0) {
        return give_up("the thunks could not be registered");
    }
    for (i, f) in NAME_FUNCS.iter().enumerate() {
        match n.hook_internal_at(c, OWNER, "libApplication.so", *f, thunks[i]) {
            Ok(orig) => ORIGS[i].store(orig, Ordering::Relaxed),
            Err(e) => {
                n.unhook_owner(c, OWNER);
                return give_up(&e);
            }
        }
    }
    match n.hook_internal_as(c, OWNER, VALID_FILL, thunks[NAME_FUNCS.len()]) {
        Ok(orig) => VALID_ORIG.store(orig, Ordering::Relaxed),
        Err(e) => {
            n.unhook_owner(c, OWNER);
            return give_up(&e);
        }
    }
    let gl = Gl::new(c, &n);
    match gl {
        Some(gl) => *GL.lock().unwrap_or_else(PoisonError::into_inner) = Some(gl),
        None => log("mod item icons are off: the GL functions were not found"),
    }
    STATE.store(ON, Ordering::Relaxed);
    log("hooks installed (names, item validity)");
}

/// Replacement of `NSStringFromItemType(ItemType) -> NSString *` (copy `i`).
fn item_name(c: &mut Cpu, i: usize) {
    let item = c.reg(0);
    if let Some(text) = name_of(item) {
        let id = item as u16;
        let known = STRINGS.lock().unwrap_or_else(PoisonError::into_inner).as_ref().and_then(|m| m.get(&id).copied());
        let obj = known.unwrap_or_else(|| {
            let o = OBJC.lock().unwrap_or_else(PoisonError::into_inner).clone();
            let made = o.map_or(0, |o| {
                let s = o.new_string(c, &text);
                o.send(c, s, "retain", &[])
            });
            if made != 0 {
                STRINGS
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get_or_insert_with(HashMap::new)
                    .insert(id, made);
            }
            made
        });
        if obj != 0 {
            c.ret_u32(obj);
            return;
        }
    }
    c.tail_call(ORIGS[i].load(Ordering::Relaxed));
}

/// Replacement of `itemTypeIsValidFillItem(ItemType) -> bool`.
fn item_valid(c: &mut Cpu) {
    if is_mod_item(c.reg(0)) {
        c.ret_u32(1);
    } else {
        c.tail_call(VALID_ORIG.load(Ordering::Relaxed));
    }
}

/// The atlas rectangle `[u, v, width, height]` (origin top-left) of a mod item's icon, `None` for any other item.
/// The picture is copied into the atlas the first time this is asked.
pub fn icon_uv(c: &mut Cpu, item: u32) -> Option<[f32; 4]> {
    if !is_mod_item(item) {
        return None;
    }
    let cell = bhmod_content::item_cell(u64::from(item))?;
    if UPLOADED.load(Ordering::Relaxed) == 0 {
        upload(c);
    }
    if UPLOADED.load(Ordering::Relaxed) != 1 {
        return None;
    }
    let cols = bhmod_content::items::ATLAS_COLUMNS as f32;
    let rows = cols / 2.0;
    Some([(cell % 32) as f32 / cols, (cell / 32) as f32 / rows, 1.0 / cols, 1.0 / rows])
}

/// True while a loaded mod declares an item (the sprite hook of the weapon skins serves the icons too).
pub fn has_icons() -> bool {
    has_items()
}

/// Copies the icons into the item atlas texture and rebuilds its mipmaps. Sets [`UPLOADED`] to 1, or to 2 when it
/// cannot be done (no atlas seen, no GL); a new atlas or new content resets it to 0.
fn upload(c: &mut Cpu) {
    let (obj, wide) = (ATLAS.load(Ordering::Relaxed), ATLAS_WIDE.load(Ordering::Relaxed));
    let o = OBJC.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let gl = GL.lock().unwrap_or_else(PoisonError::into_inner);
    let (Some(o), Some(gl)) = (o, gl.as_ref()) else { return UPLOADED.store(2, Ordering::Relaxed) };
    if obj == 0 {
        return UPLOADED.store(2, Ordering::Relaxed);
    }
    let name = o.send(c, obj, "name", &[]);
    let icons = ICONS.lock().unwrap_or_else(PoisonError::into_inner);
    gl.blit(c, name, wide, &icons);
    log(&format!("{} icon(s) copied into the item atlas {name} ({wide} px wide)", icons.len()));
    UPLOADED.store(1, Ordering::Relaxed);
}

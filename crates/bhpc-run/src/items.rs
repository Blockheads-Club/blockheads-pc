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
pub fn load(content: &Content, lang: &str) {
    let names: HashMap<u16, String> = content
        .items()
        .all()
        .filter_map(|i| Some((u16::try_from(i.id).ok()?, content.item_name(i.id, lang)?)))
        .collect();
    if !names.is_empty() {
        log(&format!("{} item(s) loaded", names.len()));
    }
    *NAMES.write().unwrap_or_else(PoisonError::into_inner) = Some(names);
    *STRINGS.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

/// The session ended.
pub fn clear() {
    *NAMES.write().unwrap_or_else(PoisonError::into_inner) = None;
    *STRINGS.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

fn has_items() -> bool {
    NAMES.read().unwrap_or_else(PoisonError::into_inner).as_ref().is_some_and(|n| !n.is_empty())
}

fn name_of(item: u32) -> Option<String> {
    let id = u16::try_from(item).ok()?;
    NAMES.read().unwrap_or_else(PoisonError::into_inner).as_ref()?.get(&id).cloned()
}

fn is_mod_item(item: u32) -> bool {
    u16::try_from(item)
        .ok()
        .is_some_and(|id| NAMES.read().unwrap_or_else(PoisonError::into_inner).as_ref().is_some_and(|n| n.contains_key(&id)))
}

/// Installs or removes the hooks as items are loaded (once per frame on the main thread).
pub fn sync(c: &mut Cpu, o: &Arc<Objc>) {
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
                STRINGS.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert_with(HashMap::new).insert(id, made);
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

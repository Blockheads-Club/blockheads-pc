//! Variant blocks of content mods in the play build (mod system M8): a tile that carries a valid variant record in
//! its spare bytes (`Tile.padding`, see [`bhmod_content::variant`]) is drawn with the mod's texture.
//!
//! The game's tile geometry builder is one huge static function of `libApplication.so` (1.7.6): for every tile of a
//! macro tile it picks image indices into `TileMap.png` (a 32 x 32 grid of cells) from the tile's `typeIndex` (a jump
//! table), then from its foreground `contents`, and keeps the result in six stack slots (`[r11 - 0x538 ..
//! r11 - 0x54c]`: the base cells and the cells of the block's faces, the front face reads the first two). All paths meet at [`JOIN`]. One inline hook there reads the
//! record of the tile being built (the tile pointer is `[r11 - 0x1c0]`, its world position `[r11 - 0x1b8]` and
//! `[r11 - 0x1bc]`) and, for a valid variant, overwrites the four slots with the variant's cell of the tile atlas.
//!
//! The mod's picture lives in one of the atlas' free cells ([`bhmod_content::block_cell`], cells 864 and up are empty
//! in the game's atlas): it is copied in with `glTexSubImage2D` the first time a variant tile is built, like the
//! item icons. The atlas is the `CPTexture2D` the game made with a square size and mipmaps, noticed when it is
//! created.
//!
//! The hook is installed only while a loaded mod declares a variant block and only on the game build whose code at
//! [`JOIN`] matches [`JOIN_CODE`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};

use bhmod_content::{variant, Content};
use bhpc_cpu::Cpu;

use crate::modglue::Objc;

/// The engine owner of the hook.
const OWNER: u32 = u32::MAX - 3;
/// Where the cases of the type and contents switches meet again, as an offset in `libApplication.so`.
const JOIN: u32 = 0xa2_2d74;
/// The two instructions there (`ldr r0, [r11, #-0x540]; cmp r0, #256`).
const JOIN_CODE: [u32; 2] = [0xe51b_0540, 0xe350_0c01];
/// Where the tile pointer and its world position are, relative to `r11`.
const TILE: u32 = 0x1c0;
const POS_X: u32 = 0x1b8;
const POS_Y: u32 = 0x1bc;
/// The slots of the face cells, relative to `r11`.
const FACES: [u32; 6] = [0x538, 0x53c, 0x540, 0x544, 0x548, 0x54c];

/// `variant id -> cell` of the variant blocks of the loaded mods.
static CELLS: RwLock<Option<HashMap<u16, u32>>> = RwLock::new(None);
/// The pictures `(cell, 64 px, 16 px)`, copied into the atlas when the first variant tile is built.
type Pictures = Vec<(u32, Vec<u8>, Vec<u8>)>;
static PICTURES: Mutex<Pictures> = Mutex::new(Vec::new());
/// The tile atlas `CPTexture2D` (0: not seen yet) and its width; whether the pictures are in it (0 no, 1 yes, 2 never).
static ATLAS: AtomicU32 = AtomicU32::new(0);
static ATLAS_WIDE: AtomicU32 = AtomicU32::new(0);
static UPLOADED: AtomicU32 = AtomicU32::new(0);

static GL: Mutex<Option<crate::items::Gl>> = Mutex::new(None);
static OBJC: Mutex<Option<Arc<Objc>>> = Mutex::new(None);
static ORIG: AtomicU32 = AtomicU32::new(0);
static THUNK: OnceLock<u32> = OnceLock::new();
static STATE: AtomicU32 = AtomicU32::new(OFF);
const OFF: u32 = 0;
const ON: u32 = 1;
const GAVE_UP: u32 = 2;
/// Tiles drawn with a variant so far (for the log).
static DRAWN: AtomicU32 = AtomicU32::new(0);

fn log(msg: &str) {
    eprintln!("[blocks] {msg}");
}

/// A session loaded `content`: remembers the cells and pictures of its variant blocks.
pub fn load(content: &mut Content) {
    let ids: Vec<u64> = content.blocks().all().map(|b| b.id).collect();
    let (mut cells, mut pictures) = (HashMap::new(), Vec::new());
    for id in ids {
        let (Some(cell), Ok(variant)) = (bhmod_content::block_cell(id), u16::try_from(id)) else { continue };
        match (content.block_cell_pixels(id, 64), content.block_cell_pixels(id, 16)) {
            (Ok(Some(hd)), Ok(Some(sd))) => {
                cells.insert(variant, cell);
                pictures.push((cell, hd, sd));
            }
            (Err(why), _) | (_, Err(why)) => log(&format!("texture of block {id}: {why}")),
            _ => {}
        }
    }
    if !cells.is_empty() {
        log(&format!("{} variant block(s) loaded", cells.len()));
    }
    *PICTURES.lock().unwrap_or_else(PoisonError::into_inner) = pictures;
    UPLOADED.store(0, Ordering::Relaxed);
    *CELLS.write().unwrap_or_else(PoisonError::into_inner) = Some(cells);
}

/// The session ended.
pub fn clear() {
    PICTURES.lock().unwrap_or_else(PoisonError::into_inner).clear();
    *CELLS.write().unwrap_or_else(PoisonError::into_inner) = None;
}

/// The game made a texture (`-[CPTexture2D initWithData:...]` is about to run on `obj`): remembers the tile atlas
/// (square, mipmapped, 2048 px in HD and 512 px in SD).
pub fn observe_texture(obj: u32, wide: u32, high: u32, mipmap: u32) {
    if mipmap == 1 && wide == high && (wide == 2048 || wide == 512) {
        ATLAS.store(obj, Ordering::Relaxed);
        ATLAS_WIDE.store(wide, Ordering::Relaxed);
        UPLOADED.store(0, Ordering::Relaxed);
    }
}

fn has_blocks() -> bool {
    CELLS.read().unwrap_or_else(PoisonError::into_inner).as_ref().is_some_and(|c| !c.is_empty())
}

fn cell_of(id: u16) -> Option<u32> {
    CELLS.read().unwrap_or_else(PoisonError::into_inner).as_ref()?.get(&id).copied()
}

/// The atlas cell a tile with `padding` at `(x, y)` and `type_index` shows, `None` for a vanilla tile.
fn cell_for(padding: &[u8], type_index: u8, x: i32, y: i32) -> Option<u32> {
    let record: &[u8; variant::LEN] = padding.try_into().ok()?;
    let v = variant::decode(record, type_index, x, y)?;
    if v.layer != 0 {
        return None;
    }
    cell_of(v.id)
}

/// Installs or removes the hook as blocks are loaded (once per frame on the main thread).
pub(crate) fn sync(c: &mut Cpu, o: &Arc<Objc>) {
    match (has_blocks(), STATE.load(Ordering::Relaxed)) {
        (true, OFF) => install(c, o),
        (false, ON) => remove(c),
        _ => {}
    }
}

/// Takes the hook out (the session ends).
pub fn remove(c: &mut Cpu) {
    if STATE.load(Ordering::Relaxed) == ON {
        if let Some(n) = crate::native::get() {
            n.unhook_owner(c, OWNER);
        }
        STATE.store(OFF, Ordering::Relaxed);
        log("hook removed");
    }
}

fn give_up(why: &str) {
    log(&format!("variant blocks are not drawn: {why}"));
    STATE.store(GAVE_UP, Ordering::Relaxed);
}

fn install(c: &mut Cpu, o: &Arc<Objc>) {
    let Some(n) = crate::native::get() else { return };
    let Some(base) = n.module_base("libApplication.so") else { return };
    let words: Vec<u32> = (0..2).map(|i| c.mem().read_u32(base + JOIN + 4 * i).unwrap_or(0)).collect();
    if words != JOIN_CODE {
        return give_up("this is not the game build they know");
    }
    *OBJC.lock().unwrap_or_else(PoisonError::into_inner) = Some(o.clone());
    let Some(gl) = crate::items::Gl::new(c, n) else { return give_up("the GL functions were not found") };
    *GL.lock().unwrap_or_else(PoisonError::into_inner) = Some(gl);
    let thunk = *THUNK.get_or_init(|| c.register_shared_thunk("bhmod:tile_faces", Arc::new(tile_faces)).unwrap_or(0));
    if thunk == 0 {
        return give_up("the thunk could not be registered");
    }
    match n.hook_internal_at(c, OWNER, "libApplication.so", JOIN, thunk) {
        Ok(orig) => {
            ORIG.store(orig, Ordering::Relaxed);
            STATE.store(ON, Ordering::Relaxed);
            log("hook installed (tile geometry)");
        }
        Err(e) => give_up(&e),
    }
}

/// Runs at [`JOIN`] for every tile the geometry builder handles: a variant tile gets its cell in the face slots.
fn tile_faces(c: &mut Cpu) {
    let fp = c.reg(11);
    fn slot_at(c: &mut Cpu, fp: u32, off: u32) -> u32 {
        c.mem().read_u32(fp.wrapping_sub(off)).unwrap_or(0)
    }
    let (tile, x, y) = (slot_at(c, fp, TILE), slot_at(c, fp, POS_X) as i32, slot_at(c, fp, POS_Y) as i32);
    if tile != 0 {
        let head = c.mem().read_bytes(tile, 0x40).unwrap_or_default();
        if let Some(cell) = head.get(0x30..0x40).and_then(|p| cell_for(p, head[0], x, y)) {
            if upload(c) {
                for off in FACES {
                    let _ = c.mem().write_u32(fp.wrapping_sub(off), cell);
                }
                if DRAWN.fetch_add(1, Ordering::Relaxed) == 0 {
                    log(&format!("first variant tile at ({x}, {y}): cell {cell}"));
                }
            }
        }
    }
    c.tail_call(ORIG.load(Ordering::Relaxed));
}

/// Copies the pictures into the tile atlas the first time, true when they are in.
fn upload(c: &mut Cpu) -> bool {
    match UPLOADED.load(Ordering::Relaxed) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    let (mut obj, mut wide) = (ATLAS.load(Ordering::Relaxed), ATLAS_WIDE.load(Ordering::Relaxed));
    let o = OBJC.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let gl = GL.lock().unwrap_or_else(PoisonError::into_inner);
    let (Some(o), Some(gl)) = (o, gl.as_ref()) else { return false };
    // The world's own atlas texture (`World.tileMapTexture`) is the one that is drawn.
    let ivar = crate::native::get().and_then(|n| n.symbol("libApplication.so", "OBJC_IVAR_$_World.tileMapTexture"));
    let world = crate::modglue::current_world();
    if let (Some(ivar), true) = (ivar, world != 0) {
        let off = c.mem().read_u32(ivar).unwrap_or(0);
        let tex = c.mem().read_u32(world + off).unwrap_or(0);
        if tex != 0 {
            obj = tex;
            wide = o.send(c, tex, "pixelsWide", &[]);
        }
    }
    if obj == 0 || wide == 0 {
        return false;
    }
    let name = o.send(c, obj, "name", &[]);
    let pictures = PICTURES.lock().unwrap_or_else(PoisonError::into_inner);
    gl.blit(c, name, wide, &pictures);
    log(&format!("{} picture(s) copied into the tile atlas {name} ({wide} px wide)", pictures.len()));
    UPLOADED.store(1, Ordering::Relaxed);
    true
}

/// `BHPC_DEV_BLOCKS=<key>`: a variant block of a `--mods-dev` folder (single-player, ids from an in-memory registry)
/// is put on the three tiles under the ground two tiles to the right of this client's own blockhead, for testing
/// without a server. Returns the variant id.
pub fn dev_key() -> Option<String> {
    std::env::var("BHPC_DEV_BLOCKS").ok().filter(|k| !k.is_empty())
}

thread_local! {
    static DEV_ID: std::cell::Cell<Option<u16>> = const { std::cell::Cell::new(None) };
    static DEV_DONE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Remembers the id of the dev block (called when a session's content loads).
pub fn load_dev(content: &Content, key: Option<&str>) {
    let id = key.and_then(|k| content.blocks().all().find(|b| b.key == k)).and_then(|b| u16::try_from(b.id).ok());
    DEV_ID.with(|d| d.set(id));
    DEV_DONE.with(|d| d.set(0));
}

/// `tileAtWorldPosition(x, y, macroTiles, world)` and `reloadDrawBlockGeometryForTile(x, y, macroTiles, world, 1)`.
const TILE_AT: u32 = 0xa1_6e68;
const RELOAD: u32 = 0xa1_915c;

/// The per-frame hook of a blockhead: after a few frames its own (non-net) blockhead gets the dev block under it.
pub(crate) fn dev_place(_o: &Objc, c: &mut Cpu, iv: &crate::cosmetics::Ivars, bh: u32, is_net: bool) {
    let Some(id) = DEV_ID.with(std::cell::Cell::get) else { return };
    if is_net || DEV_DONE.with(|d| d.get()) >= 1000 {
        return;
    }
    // Wait until the blockhead has settled on the ground.
    if DEV_DONE.with(|d| d.replace(d.get() + 1)) != 120 {
        return;
    }
    let Some(base) = crate::native::get().and_then(|n| n.module_base("libApplication.so")) else { return };
    let (pos, macros) = (iv.offset(c, "pos"), iv.offset(c, "World.macroTiles"));
    let (px, py) = (c.mem().read_u32(bh + pos).unwrap_or(0) as i32, c.mem().read_u32(bh + pos + 4).unwrap_or(0) as i32);
    let world = crate::modglue::current_world();
    let macro_tiles = c.mem().read_u32(world + macros).unwrap_or(0);
    log(&format!("dev block {id}: blockhead at ({px}, {py}), world {world:#x}, macro tiles {macro_tiles:#x}"));
    if world == 0 || macro_tiles == 0 {
        return;
    }
    let x0 = px + 3;
    for x in x0..x0 + 3 {
        // The topmost tile that is not air or water: the exposed surface of the column.
        let mut found = None;
        for y in (py - 12..py + 14).rev() {
            let tile = c.call(base + TILE_AT, &[x as u32, y as u32, macro_tiles, world]).map_or(0, |r| r.r0);
            let kind = if tile == 0 { None } else { c.mem().read_u8(tile).ok() };
            if kind.is_some_and(|k| !matches!(k, 0 | 2 | 3 | 5)) {
                found = Some((tile, y, kind.unwrap_or(0)));
                break;
            }
        }
        let Some((tile, y, kind)) = found else { continue };
        let rec = variant::encode(&variant::Variant { id, base: kind, layer: 0, data: 0 }, x, y);
        let _ = c.mem().write_bytes(tile + 0x30, &rec);
        let _ = c.call(base + RELOAD, &[x as u32, y as u32, macro_tiles, world, 1]);
        log(&format!("dev block {id} on tile ({x}, {y}) of type {kind}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bhmod_content::variant::{encode, Variant};

    fn with_cells<R>(f: impl FnOnce() -> R) -> R {
        *CELLS.write().unwrap() = Some([(7u16, 870u32)].into());
        let r = f();
        *CELLS.write().unwrap() = None;
        r
    }

    #[test]
    fn a_valid_record_gives_the_variants_cell() {
        with_cells(|| {
            let pad = encode(&Variant { id: 7, base: 10, layer: 0, data: 0 }, 12, 34);
            assert_eq!(cell_for(&pad, 10, 12, 34), Some(870));
            // Wrong tile, wrong type, unknown id, other layer, short slice: vanilla.
            assert_eq!(cell_for(&pad, 10, 13, 34), None);
            assert_eq!(cell_for(&pad, 11, 12, 34), None);
            let other = encode(&Variant { id: 8, base: 10, layer: 0, data: 0 }, 12, 34);
            assert_eq!(cell_for(&other, 10, 12, 34), None);
            let layered = encode(&Variant { id: 7, base: 10, layer: 1, data: 0 }, 12, 34);
            assert_eq!(cell_for(&layered, 10, 12, 34), None);
            assert_eq!(cell_for(&pad[..8], 10, 12, 34), None);
            assert_eq!(cell_for(&[0; 16], 10, 12, 34), None);
        });
    }

    #[test]
    fn only_the_tile_atlas_is_noticed() {
        observe_texture(0x10, 2048, 1024, 1);
        assert_eq!(ATLAS.load(Ordering::Relaxed), 0);
        observe_texture(0x20, 2048, 2048, 0);
        assert_eq!(ATLAS.load(Ordering::Relaxed), 0);
        observe_texture(0x30, 2048, 2048, 1);
        assert_eq!((ATLAS.load(Ordering::Relaxed), ATLAS_WIDE.load(Ordering::Relaxed)), (0x30, 2048));
        ATLAS.store(0, Ordering::Relaxed);
    }
}

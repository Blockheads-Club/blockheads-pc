//! World events of the client mod loader (play build): the game's own methods that change or save the world, mapped
//! to the client API's events (`world-save`, `block-place`, `block-break`, `block-changed`, `object-place`,
//! `damage`, `death`, `player-move`) and to `player:pos()`. The hooks only watch: the server stays in charge of the
//! world, so nothing a mod answers is applied. Signatures come from the game's method lists
//! (`BHPC_DEV_OBJC_DUMP`); each hook is cheap when no mod listens.
//!
//! - `-[World saveAll]`: `world-save` before and after (the original runs in between).
//! - `-[World fillTile:atPos:withType:dataA:dataB:placedByClient:saveDict:placedByClientName:]` (and the variant with
//!   `placedByBlockhead:`): a tile was filled. `-[World removeTileAtWorldX:worldY:...]` (three variants) and
//!   `-[World removeBackWallAtPos:removeBlockhead:]`: a tile or back wall went. The variants call each other, so a
//!   call that repeats the one before in the same frame is dropped.
//! - `-[World placeWorkbenchOfType:...]`, `-[World placeInteractionObjectWithItem:...]`: `object-place`.
//! - `-[Blockhead sufferDamage:isSimulation:recoil:]` (real damage, not the prediction), `-[Blockhead die]`.
//! - `-[Blockhead updatePosition:]` of the active blockhead of a player: `player:pos()` and `player-move`.
//!
//! The player of an action is the blockhead's or the placing client's name; the local player's blockheads carry none,
//! so no name means the local player.

use std::cell::Cell;

use bhpc_cpu::Cpu;

use crate::modglue::{apply_effects, bypassed, log, on_main, with_host, ModGlue, Objc, MAIN};
use crate::objchook::{Invocation, Target, Val};

thread_local! {
    /// The call the last tile hook reported: `(frame, kind, x, y, type)` (the variants of one method chain).
    static LAST: Cell<(u64, u8, i32, i32, u32)> = const { Cell::new((u64::MAX, 0, 0, 0, 0)) };
    /// World updates so far (a frame counter for [`LAST`]).
    static FRAME: Cell<u64> = const { Cell::new(0) };
}

/// A new world update has begun.
pub(crate) fn next_frame() {
    FRAME.with(|f| f.set(f.get().wrapping_add(1)));
}

/// True when the same call was reported already in this frame (and remembers this one otherwise).
fn repeated(kind: u8, x: i32, y: i32, ty: u32) -> bool {
    let now = (FRAME.with(Cell::get), kind, x, y, ty);
    LAST.with(|l| l.replace(now) == now)
}

fn trace_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BHPC_MODS_TRACE").is_some())
}

fn trace(what: &str) {
    if trace_on() {
        log(&format!("event {what}"));
    }
}

/// True when a mod listens to a world event (so the hooks do their work).
fn watching() -> bool {
    MAIN.with(|m| m.borrow().host.as_ref().is_some_and(|h| h.watching()))
}

fn pos_arg(inv: &Invocation<'_>, i: usize) -> (i32, i32) {
    match inv.arg(i) {
        Some(Val::Words(w)) if w.len() >= 2 => (w[0] as i32, w[1] as i32),
        _ => (0, 0),
    }
}

fn word(inv: &Invocation<'_>, i: usize) -> u32 {
    inv.arg(i).map_or(0, |v| v.word())
}

/// The name of an NSString argument or of the object's `clientName`, else the local player.
fn player_of(o: &Objc, c: &mut Cpu, name_obj: u32) -> String {
    let n = o.string(c, name_obj);
    if n.is_empty() {
        MAIN.with(|m| m.borrow().local.clone())
    } else {
        n
    }
}

fn blockhead_player(o: &Objc, c: &mut Cpu, blockhead: u32) -> String {
    let name = o.send(c, blockhead, "clientName", &[]);
    player_of(o, c, name)
}

/// Registers the world hooks of `g`.
pub(crate) fn register(g: &ModGlue) {
    let t = |s: &str| Target::parse(s).expect("valid target");
    let hooks = &g.hooks;

    let o = g.objc.clone();
    hooks.hook(t("-[World saveAll]"), None, move |inv| {
        if !on_main() || bypassed() || !watching() {
            return;
        }
        trace("world-save pre");
        with_host(|h| h.world_saved(true));
        let _ = inv.call_original();
        trace("world-save post");
        with_host(|h| h.world_saved(false));
        apply_effects(&o, inv.cpu());
    });

    // Tiles filled: the long variants carry who placed it (index of the name: 7, or 8 with the blockhead).
    for (sel, name_at) in [
        ("-[World fillTile:atPos:withType:dataA:dataB:placedByClient:saveDict:placedByClientName:]", 7),
        ("-[World fillTile:atPos:withType:dataA:dataB:placedByClient:saveDict:placedByBlockhead:placedByClientName:]", 8),
    ] {
        let o = g.objc.clone();
        hooks.hook(t(sel), None, move |inv| {
            if !on_main() || bypassed() || !watching() {
                return;
            }
            let ((x, y), ty) = (pos_arg(inv, 1), word(inv, 2));
            if repeated(0, x, y, ty) {
                return;
            }
            let (a, b) = (word(inv, 3) as u16, word(inv, 4) as u16);
            let name = word(inv, name_at);
            let c = inv.cpu();
            let player = player_of(&o, c, name);
            trace(&format!("block-place {player} {x},{y} {ty}"));
            with_host(|h| h.block_placed(&player, (x, y), ty, (a, b)));
            apply_effects(&o, c);
        });
    }

    // Tiles removed: three variants of one method (the blockhead that removes it is argument 4).
    for sel in [
        "-[World removeTileAtWorldX:worldY:createContentsFreeblockCount:createForegroundContentsFreeblockCount:removeBlockhead:]",
        "-[World removeTileAtWorldX:worldY:createContentsFreeblockCount:createForegroundContentsFreeblockCount:removeBlockhead:onlyRemoveCOntents:onlyRemoveForegroundContents:]",
        "-[World removeTileAtWorldX:worldY:createContentsFreeblockCount:createForegroundContentsFreeblockCount:removeBlockhead:onlyRemoveCOntents:onlyRemoveForegroundContents:sendWorldChangedNotifcation:dontRemoveContents:]",
    ] {
        let o = g.objc.clone();
        hooks.hook(t(sel), None, move |inv| {
            if !on_main() || bypassed() || !watching() {
                return;
            }
            let (x, y) = (word(inv, 0) as i32, word(inv, 1) as i32);
            if repeated(1, x, y, 0) {
                return;
            }
            let by = word(inv, 4);
            let c = inv.cpu();
            let player = blockhead_player(&o, c, by);
            trace(&format!("block-break {player} {x},{y}"));
            with_host(|h| h.block_broken(&player, (x, y), false));
            apply_effects(&o, c);
        });
    }
    let o = g.objc.clone();
    hooks.hook(t("-[World removeBackWallAtPos:removeBlockhead:]"), None, move |inv| {
        if !on_main() || bypassed() || !watching() {
            return;
        }
        let ((x, y), by) = (pos_arg(inv, 0), word(inv, 1));
        if repeated(2, x, y, 0) {
            return;
        }
        let c = inv.cpu();
        let player = blockhead_player(&o, c, by);
        trace(&format!("block-break {player} {x},{y} back-wall"));
        with_host(|h| h.block_broken(&player, (x, y), true));
        apply_effects(&o, c);
    });

    // Objects: workbenches (type, pos) and interaction objects (an InventoryItem, pos); the name is argument 5.
    let o = g.objc.clone();
    hooks.hook(
        t("-[World placeWorkbenchOfType:atPos:saveDict:placedByClient:placedByBlockhead:placedByClientName:]"),
        None,
        move |inv| {
            if !on_main() || bypassed() || !watching() {
                return;
            }
            let (ty, (x, y), name) = (word(inv, 0), pos_arg(inv, 1), word(inv, 5));
            let c = inv.cpu();
            let player = player_of(&o, c, name);
            trace(&format!("object-place {player} workbench {ty} {x},{y}"));
            with_host(|h| h.object_placed(&player, (x, y), true, ty));
            apply_effects(&o, c);
        },
    );
    let o = g.objc.clone();
    hooks.hook(
        t("-[World placeInteractionObjectWithItem:atPos:saveDict:placedByClient:placedByBlockhead:placedByClientName:]"),
        None,
        move |inv| {
            if !on_main() || bypassed() || !watching() {
                return;
            }
            let (item, (x, y), name) = (word(inv, 0), pos_arg(inv, 1), word(inv, 5));
            let c = inv.cpu();
            let ty = o.send(c, item, "itemType", &[]);
            let player = player_of(&o, c, name);
            trace(&format!("object-place {player} object {ty} {x},{y}"));
            with_host(|h| h.object_placed(&player, (x, y), false, ty));
            apply_effects(&o, c);
        },
    );

    // Blockheads: damage (the real one, not the simulation), death and the tile the active one stands on.
    let o = g.objc.clone();
    hooks.hook(t("-[Blockhead sufferDamage:isSimulation:recoil:]"), None, move |inv| {
        if !on_main() || bypassed() || !watching() {
            return;
        }
        let (amount, simulated) = match (inv.arg(0), word(inv, 1) & 0xff) {
            (Some(Val::F32(a)), s) => (a, s != 0),
            _ => return,
        };
        if simulated {
            return;
        }
        let me = inv.receiver();
        let c = inv.cpu();
        let id = o.send_wide(c, me, "uniqueID");
        let owner = o.send(c, me, "clientName", &[]);
        let owner = player_of(&o, c, owner);
        trace(&format!("damage {owner} {id} {amount}"));
        with_host(|h| h.damaged(Some(&owner), id, amount));
        apply_effects(&o, c);
    });
    let o = g.objc.clone();
    hooks.hook(t("-[Blockhead die]"), None, move |inv| {
        if !on_main() || bypassed() || !watching() {
            return;
        }
        let me = inv.receiver();
        let c = inv.cpu();
        let id = o.send_wide(c, me, "uniqueID");
        let owner = blockhead_player(&o, c, me);
        trace(&format!("death {owner} {id}"));
        with_host(|h| h.died(Some(&owner), id));
        apply_effects(&o, c);
    });
    let o = g.objc.clone();
    hooks.hook(t("-[Blockhead updatePosition:]"), None, move |inv| {
        if !on_main() || bypassed() || MAIN.with(|m| m.borrow().host.is_none()) {
            return;
        }
        let (x, y) = pos_arg(inv, 0);
        let me = inv.receiver();
        let c = inv.cpu();
        // A player has several blockheads; the one that is active is where the player is. A net player's tells
        // itself; the local player's is the dynamic world's active blockhead.
        let name = o.send(c, me, "clientName", &[]);
        let active = if o.string(c, name).is_empty() {
            let world = MAIN.with(|m| m.borrow().world);
            let dynamic = o.send(c, world, "dynamicWorld", &[]);
            o.send(c, dynamic, "activeBlockhead", &[]) == me
        } else {
            o.send(c, me, "isCurrentlyActiveBlockheadAccordingToNetData", &[]) & 0xff != 0
        };
        if !active {
            return;
        }
        let player = player_of(&o, c, name);
        trace(&format!("player-move {player} {x},{y}"));
        with_host(|h| h.moved(&player, (x, y)));
        apply_effects(&o, c);
    });
}

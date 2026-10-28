//! Overhead icons (API 0.3.4 `ui.set-overhead`): one icon per (mod, player), drawn by the loader above the
//! player's blockhead. Several mods may mark one player; the most important icon shows. Icons the server's admin
//! turned off (`VOICE_STATE.icons`) are not drawn.

use std::collections::BTreeMap;

use bhmod_core::ui::OverheadIcon;
use bhmod_core::voice::Icons;

/// Which icon wins when several mods mark one player (first = most important).
pub const PRIORITY: [OverheadIcon; 5] = [
    OverheadIcon::AdminMute,
    OverheadIcon::Deafen,
    OverheadIcon::SelfMute,
    OverheadIcon::Speaking,
    OverheadIcon::NoVoice,
];

/// True when the admin lets `icon` show.
pub fn visible(icon: OverheadIcon, v: &Icons) -> bool {
    match icon {
        OverheadIcon::Speaking => v.speaking,
        OverheadIcon::NoVoice => v.no_voice,
        OverheadIcon::SelfMute => v.self_mute,
        OverheadIcon::Deafen => v.deafen,
        OverheadIcon::AdminMute => v.admin_mute,
    }
}

/// The icons the mods set.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Overheads {
    icons: BTreeMap<(String, String), OverheadIcon>,
}

impl Overheads {
    pub fn set(&mut self, mod_id: &str, player: &str, icon: OverheadIcon) {
        self.icons.insert((mod_id.to_string(), player.to_string()), icon);
    }

    /// False when the mod had no icon on that player.
    pub fn clear(&mut self, mod_id: &str, player: &str) -> bool {
        self.icons.remove(&(mod_id.to_string(), player.to_string())).is_some()
    }

    /// The mod is gone.
    pub fn clear_mod(&mut self, mod_id: &str) {
        self.icons.retain(|(m, _), _| m != mod_id);
    }

    pub fn is_empty(&self) -> bool {
        self.icons.is_empty()
    }

    /// The icon to draw per player (players in order), visible ones only.
    pub fn shown(&self, v: &Icons) -> Vec<(String, OverheadIcon)> {
        let mut best: BTreeMap<&str, OverheadIcon> = BTreeMap::new();
        let rank = |i: &OverheadIcon| PRIORITY.iter().position(|p| p == i).unwrap_or(PRIORITY.len());
        for ((_, player), icon) in self.icons.iter().filter(|(_, i)| visible(**i, v)) {
            let e = best.entry(player).or_insert(*icon);
            if rank(icon) < rank(e) {
                *e = *icon;
            }
        }
        best.into_iter().map(|(p, i)| (p.to_string(), i)).collect()
    }
}

/// The blockhead under a click: `heads` are `(player, head)` in world tiles (the head is [`HEAD_LIFT`] above the
/// feet, the body two tiles tall, half a tile to each side), `at` is the clicked world point. The closest wins.
pub fn pick(heads: &[(String, (f32, f32))], at: (f32, f32)) -> Option<String> {
    heads
        .iter()
        .filter(|(_, (x, y))| {
            let feet = y - HEAD_LIFT;
            (at.0 - x).abs() <= 0.6 && at.1 >= feet - 0.1 && at.1 <= feet + 2.2
        })
        .min_by(|a, b| (a.1 .0 - at.0).abs().total_cmp(&(b.1 .0 - at.0).abs()))
        .map(|(p, _)| p.clone())
}

/// Tiles from a blockhead's feet (`floatPos`) to where its overhead icon goes.
pub const HEAD_LIFT: f32 = 2.6;

//! The loader's settings screen of one mod (API 0.3.4 `ui.open-settings`, milestone V3) as plain data: the rows the
//! UI draws ([`crate::ui`]) built from the settings contract `{fields: [...]}` + values (mod-sdk `docs/settings.md`):
//! the voice chat's own rows, the mod's `scope = "client"` settings (stored on the device), its key bindings, and for
//! an admin the server settings the server sent (`SETTINGS_SCHEMA`; changes go back as `SETTINGS_SET`).
//!
//! `bool`, `int`, `float` and `enum` can be changed here; `string`, `roles`, `matrix` and `list` are shown (change
//! them with `/mod set` or in the host app).

use serde_json::{json, Value as Json};

use crate::binds::BindRow;
use crate::voice::Consent;

/// Where a row belongs (and where a change goes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// The loader's voice chat rows (`@master`, `@mic`).
    Voice,
    /// `scope = "client"`, on the device.
    Client,
    /// Key bindings of the mod's actions.
    Keys,
    /// `scope = "server"`, for admins.
    Server,
    /// Voice chat: each player's volume and local mute (`@vol:<id>`, `@mute:<id>`).
    Players,
    /// The mod's widgets on the HUD (`@widgets`).
    Hud,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::Voice => "Voice chat",
            Section::Client => "This device",
            Section::Keys => "Keys",
            Section::Server => "Server (admin)",
            Section::Players => "Players' voices",
            Section::Hud => "On screen",
        }
    }
}

/// The row key of the voice master volume.
pub const MASTER: &str = "@master";
/// The row key of the microphone permission.
pub const MIC: &str = "@mic";
/// The row key prefix of a player's volume.
pub const VOLUME: &str = "@vol:";
/// The row key prefix of a player's local mute.
pub const MUTE: &str = "@mute:";
/// The row key of "show the mod's widgets".
pub const WIDGETS: &str = "@widgets";

/// A player in the per-player volume list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerVolume {
    pub id: String,
    pub alias: String,
    /// 0..=200 %.
    pub volume: u16,
    pub muted: bool,
}

/// The rows of the per-player volumes: a volume and a mute switch per player.
pub fn player_rows(players: &[PlayerVolume]) -> Vec<Row> {
    players
        .iter()
        .flat_map(|p| {
            [
                Row {
                    section: Section::Players,
                    key: format!("{VOLUME}{}", p.id),
                    label: p.alias.clone(),
                    help: Some("Volume of this player, 0 to 200 %".into()),
                    value: RowValue::Number {
                        value: f64::from(p.volume),
                        min: Some(0.0),
                        max: Some(200.0),
                        step: 10.0,
                        int: true,
                    },
                    restart: false,
                },
                Row {
                    section: Section::Players,
                    key: format!("{MUTE}{}", p.id),
                    label: format!("Mute {}", p.alias),
                    help: Some("Only for you; the server does not know".into()),
                    value: RowValue::Bool(p.muted),
                    restart: false,
                },
            ]
        })
        .collect()
}

/// A row's value and how it can change.
#[derive(Debug, Clone, PartialEq)]
pub enum RowValue {
    Bool(bool),
    Number {
        value: f64,
        min: Option<f64>,
        max: Option<f64>,
        step: f64,
        int: bool,
    },
    Choice {
        value: String,
        values: Vec<String>,
    },
    /// A key binding (`None`: cleared).
    Key(Option<String>),
    /// Shown, not changed here.
    Fixed(String),
}

impl RowValue {
    /// What the row shows.
    pub fn text(&self) -> String {
        match self {
            RowValue::Bool(b) => if *b { "On" } else { "Off" }.to_string(),
            RowValue::Number { value, int: true, .. } => format!("{}", *value as i64),
            RowValue::Number { value, .. } => format!("{value:.2}"),
            RowValue::Choice { value, .. } => value.clone(),
            RowValue::Key(Some(k)) => k.to_uppercase(),
            RowValue::Key(None) => "—".to_string(),
            RowValue::Fixed(t) => t.clone(),
        }
    }
}

/// One row of the screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub section: Section,
    pub key: String,
    pub label: String,
    pub help: Option<String>,
    pub value: RowValue,
    /// A change takes effect after a restart.
    pub restart: bool,
}

impl Row {
    /// The value after one step (`dir` -1 or +1); `None` when the row cannot change that way.
    pub fn step(&self, dir: i32) -> Option<Json> {
        match &self.value {
            RowValue::Bool(b) => Some(Json::Bool(!b)),
            RowValue::Number { value, min, max, step, int } => {
                let mut v = value + step * f64::from(dir.signum());
                if let Some(lo) = min {
                    v = v.max(*lo);
                }
                if let Some(hi) = max {
                    v = v.min(*hi);
                }
                if v == *value {
                    return None;
                }
                Some(if *int { json!(v.round() as i64) } else { json!((v * 1e6).round() / 1e6) })
            }
            RowValue::Choice { value, values } => {
                let n = values.len() as i32;
                let at = values.iter().position(|x| x == value).map_or(0, |i| i as i32);
                Some(Json::String(values[(at + dir).rem_euclid(n) as usize].clone()))
            }
            RowValue::Key(_) | RowValue::Fixed(_) => None,
        }
    }
}

/// The text of a `label` / `help`: a string or `{en: ..., tr: ...}` (English, else any).
fn localized(j: Option<&Json>) -> Option<String> {
    match j? {
        Json::String(s) => Some(s.clone()),
        Json::Object(o) => o.get("en").or_else(|| o.values().next()).and_then(Json::as_str).map(str::to_string),
        _ => None,
    }
}

fn summary(v: &Json) -> String {
    match v {
        Json::String(s) => s.clone(),
        Json::Array(a) if a.iter().all(Json::is_string) => {
            a.iter().filter_map(Json::as_str).collect::<Vec<_>>().join(", ")
        }
        Json::Array(a) => format!("{} entries", a.len()),
        Json::Object(o) => format!("{} rows", o.len()),
        other => other.to_string(),
    }
}

/// The rows of a settings form `{fields: [...]}` with `values` (missing values: the field's default).
pub fn rows_of(section: Section, form: &Json, values: &Json) -> Vec<Row> {
    let fields = form.get("fields").and_then(Json::as_array).cloned().unwrap_or_default();
    fields
        .iter()
        .filter_map(|f| {
            let key = f.get("key")?.as_str()?.to_string();
            let ty = f.get("type").and_then(Json::as_str).unwrap_or("");
            let v = values.get(&key).or_else(|| f.get("default")).cloned().unwrap_or(Json::Null);
            let num = |k: &str| f.get(k).and_then(Json::as_f64);
            let value = match ty {
                "bool" => RowValue::Bool(v.as_bool().unwrap_or(false)),
                "int" | "float" => RowValue::Number {
                    value: v.as_f64().unwrap_or(0.0),
                    min: num("min"),
                    max: num("max"),
                    step: num("step").filter(|s| *s > 0.0).unwrap_or(if ty == "int" { 1.0 } else { 0.1 }),
                    int: ty == "int",
                },
                "enum" => RowValue::Choice {
                    value: v.as_str().unwrap_or("").to_string(),
                    values: f
                        .get("values")
                        .and_then(Json::as_array)
                        .map(|a| a.iter().filter_map(Json::as_str).map(str::to_string).collect())
                        .filter(|a: &Vec<String>| !a.is_empty())
                        .unwrap_or_else(|| vec![v.as_str().unwrap_or("").to_string()]),
                },
                _ => RowValue::Fixed(summary(&v)),
            };
            Some(Row {
                section,
                label: localized(f.get("label")).unwrap_or_else(|| key.clone()),
                help: localized(f.get("help")),
                restart: f.get("restart").and_then(Json::as_bool).unwrap_or(false),
                key,
                value,
            })
        })
        .collect()
}

/// The rows of a mod's key bindings.
pub fn key_rows(binds: &[BindRow]) -> Vec<Row> {
    binds
        .iter()
        .map(|b| Row {
            section: Section::Keys,
            key: b.action.clone(),
            label: b.action.replace(['-', '_'], " "),
            help: Some(format!("Default: {}", b.default_key.to_uppercase())),
            value: RowValue::Key(b.key.clone()),
            restart: false,
        })
        .collect()
}

/// The loader's voice rows: master volume and the microphone permission.
pub fn voice_rows(master: u16, mic: Consent) -> Vec<Row> {
    vec![
        Row {
            section: Section::Voice,
            key: MASTER.into(),
            label: "Voice volume".into(),
            help: Some("Every player's voice, 0 to 200 %".into()),
            value: RowValue::Number {
                value: f64::from(master),
                min: Some(0.0),
                max: Some(200.0),
                step: 10.0,
                int: true,
            },
            restart: false,
        },
        Row {
            section: Section::Voice,
            key: MIC.into(),
            label: "Microphone".into(),
            help: Some("Voice chat may record while a mod asks for it".into()),
            value: RowValue::Bool(mic == Consent::Granted),
            restart: false,
        },
    ]
}

/// The settings screen of one mod.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SettingsView {
    pub mod_id: String,
    /// The mod's name.
    pub title: String,
    pub rows: Vec<Row>,
    /// The last change's outcome: `(ok, text)`.
    pub status: Option<(bool, String)>,
}

/// What one settings screen is made of.
#[derive(Debug, Default)]
pub struct Sources<'a> {
    /// The voice client runs: `(master volume, microphone answer)`.
    pub voice: Option<(u16, Consent)>,
    /// The mod's client settings form and values.
    pub client: Option<(Json, Json)>,
    pub binds: &'a [BindRow],
    /// The server settings the server sent (admins only).
    pub server: Option<&'a (Json, Json)>,
    /// Voice chat's players (volumes, mutes).
    pub players: Vec<PlayerVolume>,
    /// The mod has widgets: whether they show.
    pub widgets: Option<bool>,
}

impl SettingsView {
    /// The screen of `mod_id`: voice rows, client settings, keys, server settings, in that order.
    pub fn build(mod_id: &str, title: &str, src: Sources<'_>, status: Option<(bool, String)>) -> SettingsView {
        let mut rows = Vec::new();
        if let Some((master, mic)) = src.voice {
            rows.extend(voice_rows(master, mic));
        }
        rows.extend(player_rows(&src.players));
        if let Some((form, values)) = &src.client {
            rows.extend(rows_of(Section::Client, form, values));
        }
        if let Some(shown) = src.widgets {
            rows.push(Row {
                section: Section::Hud,
                key: WIDGETS.into(),
                label: "Show this mod's widgets".into(),
                help: Some("Its boxes in the corners of the game".into()),
                value: RowValue::Bool(shown),
                restart: false,
            });
        }
        rows.extend(key_rows(src.binds));
        if let Some((form, values)) = src.server {
            rows.extend(rows_of(Section::Server, form, values));
        }
        SettingsView { mod_id: mod_id.to_string(), title: title.to_string(), rows, status }
    }
}

/// The server settings the server sent this admin (`SETTINGS_SCHEMA`), by mod id.
#[derive(Debug, Default)]
pub struct ServerForms {
    forms: std::collections::HashMap<String, (Json, Json)>,
}

impl ServerForms {
    /// Takes a `SETTINGS_SCHEMA` (CBOR form and values); false when it does not decode.
    pub fn receive(&mut self, s: &bhmod_proto::SettingsSchema) -> bool {
        let (Ok(form), Ok(values)) =
            (bhmod_core::settings::from_cbor(&s.schema), bhmod_core::settings::from_cbor(&s.values))
        else {
            return false;
        };
        self.forms.insert(s.mod_id.clone(), (form, values));
        true
    }

    pub fn get(&self, mod_id: &str) -> Option<&(Json, Json)> {
        self.forms.get(mod_id)
    }

    /// The connection is gone.
    pub fn clear(&mut self) {
        self.forms.clear();
    }
}

/// A value for `SETTINGS_SET` (canonical CBOR).
pub fn value_cbor(v: &Json) -> Vec<u8> {
    bhmod_core::settings::to_cbor(v)
}

/// What the settings screen says about a `SETTINGS_RESULT`.
pub fn result_status(r: &bhmod_proto::SettingsResult) -> (bool, String) {
    match &r.outcome {
        Ok(()) => (true, "Saved on the server".to_string()),
        Err(f) => {
            (false, f.message.clone().unwrap_or_else(|| format!("The server refused the change (error {})", f.code)))
        }
    }
}

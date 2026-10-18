//! Scripted input for play mode (`--script FILE`, `--click X,Y@MS`): a small line-based language that is expanded
//! into timed window events, screenshot requests and a quit request.
//!
//! One command per line, `#` starts a comment, blank lines are ignored. The first word is the time: `1500` is
//! milliseconds since the first presented frame, `+300` is milliseconds after the end of the previous command.
//!
//! ```text
//! 12000 click 512 552          # press and release the left button
//! +500  drag 100 400 700 400 800   # press, move for 800 ms, release
//! +0    hold 512 300 1500      # press, wait, release
//! +200  text My World          # committed text (the rest of the line, spaces included)
//! +100  key enter              # key down + up (a-z 0-9 enter escape backspace tab space delete left right up down)
//! +100  keydown left           # and keyup, for held keys
//! +100  shot menu              # screenshot `<name>.png` in the shot directory on the next presented frame
//! +100  mark world              # start a frame-time segment (p50/p99 are reported per segment)
//! +100  wait                   # no-op, only moves the clock
//! 60000 quit
//! ```
use bhpc_app::{Event, Key, Mods, MouseButton};

/// What a script step asks the player to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Event(Event),
    /// Take a screenshot named `<name>.png`.
    Shot(String),
    /// Start a frame-time segment with this label.
    Mark(String),
    Quit,
}

/// One scripted step at `at` milliseconds since the start.
pub type Step = (u64, Action);

/// Spacing of the press/move/release events of a click.
const CLICK_GAP: u64 = 60;
/// Spacing of the intermediate moves of a drag.
const DRAG_GAP: u64 = 30;

/// HID usage id of a key name.
pub fn key_code(name: &str) -> Option<Key> {
    let n = name.to_ascii_lowercase();
    let b = n.as_bytes();
    Some(Key(match n.as_str() {
        "enter" | "return" => 40,
        "escape" | "esc" => 41,
        "backspace" => 42,
        "tab" => 43,
        "space" => 44,
        "delete" => 76,
        "right" => 79,
        "left" => 80,
        "down" => 81,
        "up" => 82,
        _ if b.len() == 1 && b[0].is_ascii_lowercase() => 4 + (b[0] - b'a') as u32,
        _ if b.len() == 1 && (b'1'..=b'9').contains(&b[0]) => 30 + (b[0] - b'1') as u32,
        "0" => 39,
        _ => return None,
    }))
}

fn mouse(x: f32, y: f32, pressed: Option<bool>) -> Event {
    match pressed {
        None => Event::MouseMove { x, y },
        Some(pressed) => Event::MouseButton { button: MouseButton::Left, pressed, x, y },
    }
}

fn key_event(key: Key, pressed: bool) -> Event {
    Event::Key { key, pressed, repeat: false, mods: Mods::default() }
}

fn num<T: std::str::FromStr>(w: &[&str], i: usize, what: &str) -> Result<T, String> {
    w.get(i).and_then(|v| v.parse().ok()).ok_or_else(|| format!("missing or bad {what}"))
}

/// Expand one command (`words[0]` is its name) starting at `t`; returns the steps and the time it ends.
fn command(t: u64, words: &[&str], rest: &str) -> Result<(Vec<Step>, u64), String> {
    let mut out: Vec<Step> = Vec::new();
    let end = match words[0] {
        "click" => {
            let (x, y): (f32, f32) = (num(words, 1, "x")?, num(words, 2, "y")?);
            out.push((t, Action::Event(mouse(x, y, None))));
            out.push((t + CLICK_GAP, Action::Event(mouse(x, y, Some(true)))));
            out.push((t + 2 * CLICK_GAP, Action::Event(mouse(x, y, Some(false)))));
            t + 2 * CLICK_GAP
        }
        "hold" => {
            let (x, y, ms): (f32, f32, u64) = (num(words, 1, "x")?, num(words, 2, "y")?, num(words, 3, "ms")?);
            out.push((t, Action::Event(mouse(x, y, None))));
            out.push((t + CLICK_GAP, Action::Event(mouse(x, y, Some(true)))));
            out.push((t + CLICK_GAP + ms, Action::Event(mouse(x, y, Some(false)))));
            t + CLICK_GAP + ms
        }
        "drag" => {
            let p: [f32; 4] = [num(words, 1, "x1")?, num(words, 2, "y1")?, num(words, 3, "x2")?, num(words, 4, "y2")?];
            let ms: u64 = num(words, 5, "ms")?;
            out.push((t, Action::Event(mouse(p[0], p[1], None))));
            out.push((t + CLICK_GAP, Action::Event(mouse(p[0], p[1], Some(true)))));
            let begin = t + CLICK_GAP;
            let n = (ms / DRAG_GAP).max(1);
            for i in 1..=n {
                let f = i as f32 / n as f32;
                let (x, y) = (p[0] + (p[2] - p[0]) * f, p[1] + (p[3] - p[1]) * f);
                out.push((begin + ms * i / n, Action::Event(mouse(x, y, None))));
            }
            out.push((begin + ms + CLICK_GAP, Action::Event(mouse(p[2], p[3], Some(false)))));
            begin + ms + CLICK_GAP
        }
        "key" | "keydown" | "keyup" => {
            let name = words.get(1).copied().unwrap_or("");
            let k = key_code(name).ok_or_else(|| format!("unknown key {name:?}"))?;
            let down = words[0] != "keyup";
            let up = words[0] != "keydown";
            if down {
                out.push((t, Action::Event(key_event(k, true))));
            }
            if up {
                out.push((t + if down { 40 } else { 0 }, Action::Event(key_event(k, false))));
            }
            t + if down && up { 40 } else { 0 }
        }
        "text" => {
            let s = rest.strip_prefix("text").unwrap_or("").strip_prefix(' ').unwrap_or("");
            if s.is_empty() {
                return Err("text needs a string".into());
            }
            out.push((t, Action::Event(Event::Text(s.to_string()))));
            t
        }
        "shot" => {
            let name = words.get(1).copied().filter(|n| !n.contains(['/', '\\'])).ok_or("shot needs a plain name")?;
            out.push((t, Action::Shot(name.to_string())));
            t
        }
        "mark" => {
            let name = words.get(1).ok_or("mark needs a label")?;
            out.push((t, Action::Mark((*name).to_string())));
            t
        }
        "wait" => t,
        "quit" => {
            out.push((t, Action::Quit));
            t
        }
        other => return Err(format!("unknown command {other:?}")),
    };
    Ok((out, end))
}

/// Parse a script. Errors name the 1-based line.
pub fn parse(text: &str) -> Result<Vec<Step>, String> {
    let mut steps: Vec<Step> = Vec::new();
    let mut prev_end = 0u64;
    for (n, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let err = |e: String| format!("line {}: {e}", n + 1);
        let (time, rest) = line.split_once(char::is_whitespace).ok_or_else(|| err("missing command".into()))?;
        let rest = rest.trim_start();
        let at: u64 = match time.strip_prefix('+') {
            Some(d) => prev_end + d.parse::<u64>().map_err(|_| err(format!("bad time {time:?}")))?,
            None => time.parse().map_err(|_| err(format!("bad time {time:?}")))?,
        };
        let words: Vec<&str> = rest.split_whitespace().collect();
        let (mut s, end) = command(at, &words, rest).map_err(err)?;
        steps.append(&mut s);
        prev_end = end;
    }
    steps.sort_by_key(|s| s.0);
    Ok(steps)
}

/// `--click X,Y@MS` as a script line.
pub fn click_line(x: f32, y: f32, at: u64) -> String {
    format!("{at} click {x} {y}\n")
}

/// Build the full script of a run: the script file's text (if any) followed by `--click` shortcuts.
pub fn build(file_text: Option<&str>, clicks: &[(f32, f32, u64)]) -> Result<Vec<Step>, String> {
    let mut text = file_text.unwrap_or("").to_string();
    text.push('\n');
    for (x, y, at) in clicks {
        text.push_str(&click_line(*x, *y, *at));
    }
    parse(&text)
}

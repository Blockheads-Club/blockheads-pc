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
//! +100  waittext Welcome         # wait (up to 3 minutes) until a view shows this text; later times shift
//! +100  clicktext OK            # wait for the text, then click the button that shows it (also an alert's dialog)
//! +100  focus off              # window focus lost / gained (`focus on`); no lifecycle is sent
//! +100  minimize               # minimised / `restore`d: the game goes to the background and back
//! +100  resize 1600 900        # resize the window (logical pixels); `key f11` toggles fullscreen
//! +100  dumpviews               # print the view tree (kind, window rectangle, text) to stderr
//! +100  padadd 0                # a game controller with instance id 0 is plugged in (`padremove 0`: unplugged)
//! +100  pad a 80                # press controller button `a` for 80 ms (default 80; `paddown a` / `padup a`)
//! +100  stick right 0.5 -1      # tilt a stick (`left`/`right`) to x, y (-1..1); `stick right 0 0` centres it
//! +100  wait                   # no-op, only moves the clock
//! 60000 quit                   # close the game like the window's close button (background, terminate)
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
    /// Wait until a shown UIKit view displays this text (ignoring case); later steps shift by the time waited.
    WaitText(String),
    /// Wait for the text as above, then click the button that shows it.
    ClickText(String),
    /// Print the UIKit view tree (geometry, text) to stderr.
    DumpViews,
    /// Resize the window to this many logical pixels.
    Resize(u32, u32),
    Quit,
}

/// One scripted step at `at` milliseconds since the start.
pub type Step = (u64, Action);

/// Spacing of the press/move/release events of a click.
const CLICK_GAP: u64 = 60;
/// Spacing of the intermediate moves of a drag.
const DRAG_GAP: u64 = 30;

/// The events of a click at `(x, y)` with their offsets in milliseconds from its start.
pub fn click_events(x: f32, y: f32) -> [(u64, Event); 3] {
    [(0, mouse(x, y, None)), (CLICK_GAP, mouse(x, y, Some(true))), (2 * CLICK_GAP, mouse(x, y, Some(false)))]
}

/// How long a `waittext`/`clicktext` waits for its text.
pub const WAIT_TIMEOUT_MS: u64 = 180_000;

/// SDL game controller button number of a button name (`a b x y back guide start ls rs lb rb up down left right`).
pub fn pad_button(name: &str) -> Option<u8> {
    const NAMES: [&str; 15] =
        ["a", "b", "x", "y", "back", "guide", "start", "ls", "rs", "lb", "rb", "up", "down", "left", "right"];
    NAMES.iter().position(|n| n.eq_ignore_ascii_case(name)).map(|i| i as u8)
}

/// Scripted controllers are instance 0.
const PAD: u32 = 0;

fn pad_event(button: u8, pressed: bool) -> Event {
    Event::PadButton { id: PAD, button, pressed }
}

/// SDL's raw axis value of a -1..1 tilt.
fn raw_axis(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

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
        "f11" => 68,
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
            out.extend(click_events(x, y).map(|(d, e)| (t + d, Action::Event(e))));
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
        "wheel" => {
            let (dy, x, y): (f32, f32, f32) = (num(words, 1, "dy")?, num(words, 2, "x")?, num(words, 3, "y")?);
            out.push((t, Action::Event(mouse(x, y, None))));
            out.push((t + CLICK_GAP, Action::Event(Event::Wheel { dx: 0.0, dy })));
            t + CLICK_GAP
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
        "waittext" | "clicktext" => {
            let text = rest.split_once(char::is_whitespace).map_or("", |x| x.1).trim();
            if text.is_empty() {
                return Err(format!("{} needs a text", words[0]));
            }
            let a = if words[0] == "waittext" { Action::WaitText(text.into()) } else { Action::ClickText(text.into()) };
            out.push((t, a));
            t
        }
        "dumpviews" => {
            out.push((t, Action::DumpViews));
            t
        }
        "focus" => {
            let on = match words.get(1).copied() {
                Some("on") => true,
                Some("off") => false,
                _ => return Err("focus needs on or off".into()),
            };
            out.push((t, Action::Event(Event::Focus(on))));
            t
        }
        "resize" => {
            out.push((t, Action::Resize(num(words, 1, "width")?, num(words, 2, "height")?)));
            t
        }
        "minimize" | "restore" => {
            out.push((t, Action::Event(Event::Visible(words[0] == "restore"))));
            t
        }
        "padadd" | "padremove" => {
            let id = match words.get(1) {
                Some(_) => num(words, 1, "controller id")?,
                None => PAD,
            };
            let e = if words[0] == "padadd" { Event::PadAdded(id) } else { Event::PadRemoved(id) };
            out.push((t, Action::Event(e)));
            t
        }
        "pad" | "paddown" | "padup" => {
            let name = words.get(1).copied().unwrap_or("");
            let b = pad_button(name).ok_or_else(|| format!("unknown controller button {name:?}"))?;
            match words[0] {
                "pad" => {
                    let ms: u64 = if words.len() > 2 { num(words, 2, "ms")? } else { 80 };
                    out.push((t, Action::Event(pad_event(b, true))));
                    out.push((t + ms, Action::Event(pad_event(b, false))));
                    t + ms
                }
                which => {
                    out.push((t, Action::Event(pad_event(b, which == "paddown"))));
                    t
                }
            }
        }
        "stick" => {
            let first = match words.get(1).copied() {
                Some("left") => 0,
                Some("right") => 2,
                _ => return Err("stick needs left or right".into()),
            };
            let (x, y): (f32, f32) = (num(words, 2, "x")?, num(words, 3, "y")?);
            for (axis, v) in [(first, x), (first + 1, y)] {
                out.push((t, Action::Event(Event::PadAxis { id: PAD, axis, value: raw_axis(v) })));
            }
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

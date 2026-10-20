//! Start-up progress of the game runner. `bhpc-run` (with `--embed` or `--progress`) prints machine-readable lines on
//! stdout while it boots; the launcher parses them ([`parse_line`]) and turns them into one smooth percentage
//! ([`Tracker`]) for its loading screen.
//!
//! Lines (the `@` prefix keeps them apart from the game's own output):
//!
//! ```text
//! @progress <phase> <done> <total>     total 0 = the phase has no measurable steps (the percentage eases)
//! @ready                               the first frame has been presented
//! ```

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The boot phases, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// Reading and validating the APK.
    Apk,
    /// Mapping and relocating the game's libraries.
    Libraries,
    /// Running the libraries' constructors (the measurable bulk).
    Constructors,
    /// Starting the (emulated) Java VM and the activity.
    Runtime,
    /// Handing the app its lifecycle events: `UIApplicationMain`, `didFinishLaunching`.
    Launch,
    /// Waiting for the first presented frame.
    Frame,
}

/// Every phase in order.
pub const PHASES: [Phase; 6] =
    [Phase::Apk, Phase::Libraries, Phase::Constructors, Phase::Runtime, Phase::Launch, Phase::Frame];

impl Phase {
    /// The name used in the protocol (and as the key of the launcher's phase labels).
    pub fn name(self) -> &'static str {
        match self {
            Phase::Apk => "apk",
            Phase::Libraries => "libraries",
            Phase::Constructors => "constructors",
            Phase::Runtime => "runtime",
            Phase::Launch => "launch",
            Phase::Frame => "frame",
        }
    }

    fn from_name(s: &str) -> Option<Phase> {
        PHASES.into_iter().find(|p| p.name() == s)
    }

    /// Position in [`PHASES`] (the step number minus one).
    pub fn index(self) -> usize {
        self as usize
    }
}

/// One line of the protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    Progress { phase: Phase, done: u32, total: u32 },
    Ready,
}

impl Line {
    /// The text of the line (without the newline).
    pub fn render(&self) -> String {
        match self {
            Line::Progress { phase, done, total } => format!("@progress {} {done} {total}", phase.name()),
            Line::Ready => "@ready".to_string(),
        }
    }
}

/// Parses a runner output line; anything else (the game's own output, malformed lines) is `None`.
pub fn parse_line(line: &str) -> Option<Line> {
    let mut it = line.trim_end().split(' ');
    match it.next()? {
        "@ready" => it.next().is_none().then_some(Line::Ready),
        "@progress" => {
            let phase = Phase::from_name(it.next()?)?;
            let done = it.next()?.parse().ok()?;
            let total = it.next()?.parse().ok()?;
            it.next().is_none().then_some(Line::Progress { phase, done, total })
        }
        _ => None,
    }
}

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Turns the runner's progress output on (`--embed`, `--progress`).
pub fn enable() {
    ENABLED.store(true, Ordering::SeqCst);
}

/// Whether progress lines are printed.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

/// Prints `line` on stdout when progress output is on.
pub fn emit(line: Line) {
    if enabled() {
        println!("{}", line.render());
    }
}

/// `emit(Line::Progress { .. })`.
pub fn report(phase: Phase, done: u32, total: u32) {
    emit(Line::Progress { phase, done, total });
}

/// Default share of the bar per step (the loading screen's spec: 0-10, 10-28, 28-50, 50-72, 72-92, 92-100).
pub const DEFAULT_WEIGHTS: [f64; 6] = [0.10, 0.18, 0.22, 0.22, 0.20, 0.08];
/// A typical start, to derive the usual time of each step before any start was measured.
pub const DEFAULT_TOTAL_SECS: f64 = 20.0;
/// Starts remembered for the learned weights.
pub const HISTORY_LEN: usize = 5;
/// No step gets less than this share of the bar, however fast it was.
const MIN_WEIGHT: f64 = 0.02;
/// Without earlier starts, time left is shown after this many seconds and ...
pub const ETA_AFTER_SECS: f64 = 3.0;
/// ... at least this percentage.
pub const ETA_FROM_PERCENT: f64 = 15.0;
/// With earlier starts, the estimate scales their times by how this start compares with them so far, once this much
/// of a usual start has passed (before that the usual times are taken as they are) ...
const PACE_AFTER_SECS: f64 = 1.0;
/// ... and within these bounds.
const PACE_RANGE: (f64, f64) = (0.5, 3.0);
/// A step is slow after twice its usual time, but never sooner than this ...
pub const SLOW_MIN_SECS: f64 = 4.0;
/// ... and always after this long.
pub const SLOW_MAX_SECS: f64 = 15.0;
/// No progress line for this long counts as a stall.
pub const STALL: Duration = Duration::from_secs(60);
/// With earlier starts the time-based bar stops here until the first frame is up.
const TIMED_CAP: f64 = 99.0;
/// A step without measurable steps never fills more than this much of its share.
const EASE_CAP: f64 = 0.95;

/// How the bar is shared out among the steps, and how long each usually takes.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub weights: [f64; 6],
    /// The median time of each step over the remembered starts (one slow start does not skew it).
    pub usual_secs: [f64; 6],
    /// The times come from earlier starts on this machine, not from [`DEFAULT_TOTAL_SECS`].
    pub learned: bool,
}

impl Default for Model {
    fn default() -> Self {
        Model { weights: DEFAULT_WEIGHTS, usual_secs: DEFAULT_WEIGHTS.map(|w| w * DEFAULT_TOTAL_SECS), learned: false }
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

impl Model {
    /// Weights from the step durations (seconds) of earlier starts: each step's average share of the total. No
    /// history (or only zero durations) gives the defaults.
    pub fn learn(history: &[[f64; 6]]) -> Model {
        if history.is_empty() {
            return Model::default();
        }
        let mean: [f64; 6] = std::array::from_fn(|i| history.iter().map(|h| h[i]).sum::<f64>() / history.len() as f64);
        let total: f64 = mean.iter().sum();
        if total <= 0.0 {
            return Model::default();
        }
        let floored = mean.map(|m| (m / total).max(MIN_WEIGHT));
        let sum: f64 = floored.iter().sum();
        let usual_secs = std::array::from_fn(|i| median(history.iter().map(|h| h[i]).collect()));
        Model { weights: floored.map(|w| w / sum), usual_secs, learned: true }
    }

    /// Seconds left from earlier starts: the rest of step `idx` (`frac` of it done, or `in_step` seconds into a step
    /// without counts) and the usual time of every later step, scaled by how fast this start ran against them so far.
    fn eta(&self, idx: usize, frac: Option<f64>, in_step: f64, elapsed: f64) -> f64 {
        let usual = self.usual_secs[idx];
        let done_here = frac.map_or(in_step.min(usual), |f| usual * f);
        let left = usual - done_here + self.usual_secs[idx + 1..].iter().sum::<f64>();
        let expected = self.usual_secs[..idx].iter().sum::<f64>() + done_here;
        let pace =
            if expected >= PACE_AFTER_SECS { (elapsed / expected).clamp(PACE_RANGE.0, PACE_RANGE.1) } else { 1.0 };
        // A step without counts that runs past its usual time still has a moment to go.
        (left * pace).max(1.0)
    }

    fn base(&self, idx: usize) -> f64 {
        self.weights[..idx].iter().sum()
    }
}

/// The step durations of the last starts, kept in the launcher's config directory.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub starts: Vec<[f64; 6]>,
}

impl History {
    /// Reads `path`; a missing or unreadable file is an empty history (nothing here is worth failing for).
    pub fn load(path: &Path) -> History {
        std::fs::read_to_string(path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
    }

    /// Adds a finished start, forgetting the oldest beyond [`HISTORY_LEN`].
    pub fn push(&mut self, durations: [f64; 6]) {
        self.starts.push(durations);
        let extra = self.starts.len().saturating_sub(HISTORY_LEN);
        self.starts.drain(..extra);
    }

    /// Writes `path`, creating parent directories.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new("")))?;
        std::fs::write(path, toml::to_string_pretty(self).expect("History always serialises"))
    }

    /// The model these starts teach.
    pub fn model(&self) -> Model {
        Model::learn(&self.starts)
    }
}

/// Where the history file sits next to the launcher's other files.
pub fn history_path(dir: &Path) -> PathBuf {
    dir.join("boot-times.toml")
}

/// What the loading screen shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Info {
    /// 0..=100, never decreasing.
    pub percent: f64,
    /// The phase the runner reported last (its protocol name).
    pub phase: &'static str,
    /// Its number, 1 to 6.
    pub step: u32,
    /// Seconds since the start.
    pub elapsed_secs: f64,
    /// Seconds left: from the first progress line when earlier starts are known, else once the spec's conditions hold
    /// (3 s and 15 %); the screen rounds it.
    pub eta_secs: Option<f64>,
    /// The current step takes much longer than usual.
    pub slow: bool,
    /// No progress line for the stall time.
    pub stalled: bool,
    /// How long each finished step took.
    pub step_secs: Vec<Option<f64>>,
    /// The first frame is up.
    pub ready: bool,
}

impl Default for Tracker {
    fn default() -> Self {
        Tracker {
            model: Model::default(),
            stall: STALL,
            last: None,
            entered: [None; 6],
            last_line: Duration::ZERO,
            ready_at: None,
            shown: 0.0,
        }
    }
}

/// The percentage from the phase the runner is in; time (`now`, since the runner started) only moves steps without
/// measurable progress, and drives the estimate, the slow and the stall rules.
#[derive(Debug, Clone)]
pub struct Tracker {
    model: Model,
    stall: Duration,
    last: Option<(Phase, u32, u32)>,
    /// When each step began.
    entered: [Option<Duration>; 6],
    last_line: Duration,
    ready_at: Option<Duration>,
    shown: f64,
}

impl Tracker {
    pub fn new(model: Model) -> Self {
        Tracker { model, ..Tracker::default() }
    }

    /// A tracker that calls a start stalled after `stall` without a progress line.
    pub fn with_stall(model: Model, stall: Duration) -> Self {
        Tracker { model, stall, ..Tracker::default() }
    }

    /// Whether any progress line arrived.
    pub fn started(&self) -> bool {
        self.last.is_some() || self.ready_at.is_some()
    }

    /// Feeds one line; `now` is the time since the runner was started.
    pub fn feed(&mut self, line: Line, now: Duration) {
        self.last_line = now;
        let upto = match line {
            Line::Ready => {
                self.ready_at.get_or_insert(now);
                5
            }
            Line::Progress { phase, done, total } => {
                self.last = Some((phase, done, total));
                phase.index()
            }
        };
        // A step the runner never mentioned counts as done in no time.
        for e in &mut self.entered[..=upto] {
            e.get_or_insert(now);
        }
    }

    /// How long each step took, once the first frame is up.
    pub fn durations(&self) -> Option<[f64; 6]> {
        let ready = self.ready_at?;
        Some(std::array::from_fn(|i| {
            let end = self.entered.get(i + 1).copied().flatten().unwrap_or(ready);
            end.saturating_sub(self.entered[i].unwrap_or(ready)).as_secs_f64()
        }))
    }

    fn raw(&self, now: Duration) -> f64 {
        if self.ready_at.is_some() {
            return 100.0;
        }
        let Some((phase, done, total)) = self.last else { return 0.0 };
        let idx = phase.index();
        let inside = if total == 0 {
            let since = self.entered[idx].unwrap_or_default();
            let t = now.saturating_sub(since).as_secs_f64() / self.model.usual_secs[idx].max(0.5);
            EASE_CAP * (1.0 - (-t).exp())
        } else {
            f64::from(done.min(total)) / f64::from(total)
        };
        (self.model.base(idx) + self.model.weights[idx] * inside) * 100.0
    }

    /// The loading screen's numbers at time `now`.
    pub fn info(&mut self, now: Duration) -> Info {
        let ready = self.ready_at.is_some();
        // With earlier starts the bar follows the time estimate (below), so the percentage and the time left agree.
        if ready || !self.model.learned {
            self.shown = self.shown.max(self.raw(now)).min(100.0);
        }
        let phase = self.last.map_or(Phase::Apk, |l| l.0);
        let idx = phase.index();
        let secs = now.as_secs_f64();
        let in_step = now.saturating_sub(self.entered[idx].unwrap_or_default()).as_secs_f64();
        let slow_after = (2.0 * self.model.usual_secs[idx]).clamp(SLOW_MIN_SECS, SLOW_MAX_SECS);
        let eta = if ready {
            None
        } else if self.model.learned {
            self.last.map(|(_, done, total)| {
                let frac = (total > 0).then(|| f64::from(done.min(total)) / f64::from(total));
                self.model.eta(idx, frac, in_step, secs)
            })
        } else {
            (secs >= ETA_AFTER_SECS && self.shown >= ETA_FROM_PERCENT).then(|| secs * (100.0 - self.shown) / self.shown)
        };
        if let (false, true, Some(left)) = (ready, self.model.learned, eta) {
            // Elapsed over elapsed plus left: a start that runs long slows the bar instead of stalling at a high
            // percentage; it never goes back and leaves the last percent for the first frame.
            self.shown = self.shown.max(secs / (secs + left) * 100.0).min(TIMED_CAP);
        }
        let durations = self.durations();
        let step_secs = (0..6)
            .map(|i| match (durations, self.entered[i + 1..].first().copied().flatten()) {
                (Some(d), _) => Some(d[i]),
                (None, Some(next)) if i < idx => {
                    Some(next.saturating_sub(self.entered[i].unwrap_or_default()).as_secs_f64())
                }
                _ => None,
            })
            .collect();
        Info {
            percent: self.shown,
            phase: phase.name(),
            step: idx as u32 + 1,
            elapsed_secs: secs,
            eta_secs: eta,
            slow: !ready && self.started() && in_step >= slow_after,
            stalled: !ready && self.started() && now.saturating_sub(self.last_line) >= self.stall,
            step_secs,
            ready,
        }
    }
}

//! Spawning and supervising one `bhpc-run` process: state for the Home screen, a bounded log, crash detection.

use crate::progress::{self, History, Info, Tracker};
use serde::Serialize;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long a game that has printed no progress line yet counts as "starting" (as long as a start may go without
/// one, [`progress::STALL`]). The runner prints its first line only once its video, audio and controllers are set up,
/// which took 3.3 s on the dev machine: a shorter grace called the game running before that, so the loading screen
/// gave way to the empty black stage until the first line came.
pub const LAUNCH_GRACE: Duration = progress::STALL;
/// Log lines kept in memory.
pub const MAX_LINES: usize = 5000;

/// Where the `bhpc-run` sidecar sits next to the launcher executable.
pub fn sidecar_path(exe_dir: &Path) -> PathBuf {
    exe_dir.join(if cfg!(windows) { "bhpc-run.exe" } else { "bhpc-run" })
}

/// What the Home screen shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GameState {
    Ready,
    /// Booting: the runner has not presented its first frame yet.
    Starting,
    Running,
    Stopped,
    Crashed,
    /// Nothing is running and the game file is gone.
    Missing,
}

/// Why a session could not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    AlreadyRunning,
    Spawn(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::AlreadyRunning => write!(f, "the game is already running"),
            SessionError::Spawn(e) => write!(f, "cannot start the game: {e}"),
        }
    }
}

impl std::error::Error for SessionError {}

/// A bounded line buffer addressed by absolute line numbers, so a poller can ask for "everything after N".
#[derive(Debug)]
pub struct LogBuffer {
    cap: usize,
    lines: VecDeque<String>,
    dropped: usize,
}

impl LogBuffer {
    /// Keeps at most `cap` lines.
    pub fn new(cap: usize) -> Self {
        LogBuffer { cap, lines: VecDeque::new(), dropped: 0 }
    }

    /// Appends a line, dropping the oldest when full.
    pub fn push(&mut self, line: String) {
        if self.lines.len() == self.cap {
            self.lines.pop_front();
            self.dropped += 1;
        }
        self.lines.push_back(line);
    }

    /// Forgets everything (line numbers restart at 0).
    pub fn clear(&mut self) {
        self.lines.clear();
        self.dropped = 0;
    }

    /// The lines from absolute number `since` on, and the next number to ask for.
    pub fn since(&self, since: usize) -> (Vec<String>, usize) {
        let start = since.saturating_sub(self.dropped).min(self.lines.len());
        (self.lines.iter().skip(start).cloned().collect(), self.dropped + self.lines.len())
    }
}

/// State and new log lines, as polled by the UI.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Snapshot {
    pub state: GameState,
    /// Seconds since the game started (while it runs).
    pub running_secs: Option<u64>,
    /// Length of the last finished session.
    pub last_session_secs: Option<u64>,
    /// Exit code of the last finished session.
    pub exit_code: Option<i32>,
    /// Signal that ended the last session (Unix).
    pub signal: Option<i32>,
    /// Start-up progress (of the running game, or of the last one: where it got to before it ended).
    pub progress: Option<Info>,
    /// Log lines after the number the poller passed.
    pub lines: Vec<String>,
    /// Pass this next time.
    pub next: usize,
}

struct Running {
    child: Child,
    started: Instant,
    readers: Vec<JoinHandle<()>>,
    /// Lines for the game's stdin (see [`Session::input`]).
    input: Option<Sender<String>>,
    stopping: bool,
    /// The step durations of this start went into the history.
    learned: bool,
}

struct Finished {
    progress: Option<Info>,
    secs: u64,
    code: Option<i32>,
    signal: Option<i32>,
    crashed: bool,
}

/// One supervised game process at a time.
pub struct Session {
    run: Option<Running>,
    last: Option<Finished>,
    log: Arc<Mutex<LogBuffer>>,
    tracker: Arc<Mutex<Tracker>>,
    /// Where the durations of finished starts are kept (the loading screen's weights come from there).
    history: Option<PathBuf>,
    stall: Duration,
    grace: Duration,
    /// Called with the text of every `@copy` line of the game (see [`crate::embed::copy_line`]).
    on_copy: CopyHandler,
}

/// What the launcher does with text the game copied.
pub type CopyHandler = Arc<Mutex<Option<Box<dyn Fn(String) + Send>>>>;

impl Default for Session {
    fn default() -> Self {
        Session::new(LAUNCH_GRACE)
    }
}

/// Reads lines into the log. With `track`, progress lines (see [`progress`]) go to the tracker and `@copy` lines to the
/// copy handler instead of the log.
/// (Not generic, so that stdout and stderr share one instantiation.)
fn pump(
    source: Box<dyn Read + Send>,
    log: Arc<Mutex<LogBuffer>>,
    track: Option<(Arc<Mutex<Tracker>>, Instant, CopyHandler)>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(source);
        let mut buf = Vec::new();
        while reader.read_until(b'\n', &mut buf).map(|n| n > 0).unwrap_or(false) {
            let line = String::from_utf8_lossy(&buf).trim_end().to_string();
            buf.clear();
            if let Some((tracker, began, copy)) = &track {
                if let Some(parsed) = progress::parse_line(&line) {
                    tracker.lock().expect("tracker lock").feed(parsed, began.elapsed());
                    continue;
                }
                if let Some(text) = crate::embed::parse_copy(&line) {
                    if let Some(f) = copy.lock().expect("copy lock").as_ref() {
                        f(text);
                    }
                    continue;
                }
            }
            log.lock().expect("log lock").push(line);
        }
    })
}

/// Writes every line sent to the returned handle to `sink`, until the handle (and its clones) are dropped or the
/// sink fails. Closing the sink is how the game learns that the launcher is gone.
fn feed(mut sink: Box<dyn Write + Send>) -> Sender<String> {
    let (tx, rx) = channel::<String>();
    std::thread::spawn(move || {
        for line in rx {
            if writeln!(sink, "{line}").and_then(|()| sink.flush()).is_err() {
                break;
            }
        }
    });
    tx
}

/// The game runner is a console program: do not flash a console window when the (windowed) launcher starts it.
#[cfg(windows)]
fn hide_console(command: &mut Command) {
    std::os::windows::process::CommandExt::creation_flags(command, 0x0800_0000);
}

#[cfg(not(windows))]
fn hide_console(_command: &mut Command) {}

#[cfg(unix)]
fn signal_of(s: &ExitStatus) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(s)
}

#[cfg(not(unix))]
fn signal_of(_: &ExitStatus) -> Option<i32> {
    None
}

impl Session {
    /// A session whose process counts as starting for `grace`.
    pub fn new(grace: Duration) -> Self {
        Session {
            run: None,
            last: None,
            log: Arc::new(Mutex::new(LogBuffer::new(MAX_LINES))),
            tracker: Arc::default(),
            history: None,
            stall: progress::STALL,
            grace,
            on_copy: Arc::default(),
        }
    }

    /// What to do with text the game copies (in-window mode: put it on the desktop clipboard).
    pub fn set_copy_handler(&mut self, f: impl Fn(String) + Send + 'static) {
        *self.on_copy.lock().expect("copy lock") = Some(Box::new(f));
    }

    /// Keeps the step durations of every start in this file and derives the next start's weights from it.
    pub fn set_history(&mut self, path: PathBuf) {
        self.history = Some(path);
    }

    /// How long a starting game may print no progress before it counts as stuck (default [`progress::STALL`]).
    pub fn set_stall(&mut self, after: Duration) {
        self.stall = after;
    }

    /// Spawns `program args...` (with `env` added) with its output captured. The previous session's log is replaced.
    pub fn start(&mut self, program: &Path, args: &[String], env: &[(String, String)]) -> Result<(), SessionError> {
        self.refresh();
        if self.run.is_some() {
            return Err(SessionError::AlreadyRunning);
        }
        let mut command = Command::new(program);
        command
            .args(args)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_console(&mut command);
        let mut child = command.spawn().map_err(|e| SessionError::Spawn(format!("{}: {e}", program.display())))?;
        self.log.lock().expect("log lock").clear();
        let model = self.history.as_deref().map(History::load).unwrap_or_default().model();
        *self.tracker.lock().expect("tracker lock") = Tracker::with_stall(model, self.stall);
        let started = Instant::now();
        let mut readers = Vec::new();
        readers.extend(
            child.stdout.take().map(|s| {
                pump(Box::new(s), self.log.clone(), Some((self.tracker.clone(), started, self.on_copy.clone())))
            }),
        );
        readers.extend(child.stderr.take().map(|s| pump(Box::new(s), self.log.clone(), None)));
        let input = child.stdin.take().map(|s| feed(Box::new(s)));
        self.run = Some(Running { child, started, readers, input, stopping: false, learned: false });
        Ok(())
    }

    /// A handle for sending lines to the running game's stdin (input events of the in-window view, see
    /// `embed::Cmd`). Lines are written by a thread of their own, so sending never blocks; they are dropped once the
    /// game is gone.
    pub fn input(&self) -> Option<Sender<String>> {
        self.run.as_ref().and_then(|r| r.input.clone())
    }

    /// Kills the game if it runs; the session then counts as stopped by the user.
    pub fn stop(&mut self) {
        self.refresh();
        if let Some(mut run) = self.run.take() {
            run.stopping = true;
            self.kill(run);
        }
    }

    /// Kills the process, waits for it and records how it ended.
    fn kill(&mut self, mut run: Running) {
        let _ = run.child.kill();
        let waited = run.child.wait();
        let _ = waited.map(|status| self.finish(run, status));
    }

    fn finish(&mut self, run: Running, status: ExitStatus) {
        for r in run.readers {
            let _ = r.join();
        }
        let progress = self.progress_info(run.started);
        self.last = Some(Finished {
            progress,
            secs: run.started.elapsed().as_secs(),
            code: status.code(),
            signal: signal_of(&status),
            crashed: !status.success() && !run.stopping,
        });
    }

    /// The tracker's numbers for a game started at `started`, if it printed any progress.
    fn progress_info(&self, started: Instant) -> Option<Info> {
        let mut t = self.tracker.lock().expect("tracker lock");
        t.started().then(|| t.info(started.elapsed()))
    }

    /// Notices a process that ended by itself.
    fn refresh(&mut self) {
        let Some(status) = self.run.as_mut().and_then(|r| r.child.try_wait().ok().flatten()) else { return };
        let run = self.run.take().expect("a process that exited was running");
        self.finish(run, status);
    }

    /// A game that printed no progress for a minute while starting is stuck: it is killed and counts as crashed.
    fn stop_if_stalled(&mut self) {
        let stalled = self.run.as_ref().and_then(|r| self.progress_info(r.started)).is_some_and(|i| i.stalled);
        if let Some(run) = self.run.take_if(|_| stalled) {
            self.kill(run);
        }
    }

    /// Remembers how long each step took once the first frame is up.
    fn learn(&mut self) {
        let Some(run) = self.run.as_mut().filter(|r| !r.learned) else { return };
        let durations = self.tracker.lock().expect("tracker lock").durations();
        if let (Some(d), Some(path)) = (durations, &self.history) {
            run.learned = true;
            let mut h = History::load(path);
            h.push(d);
            let _ = h.save(path);
        }
    }

    /// Current state plus the log lines after `since`. `apk_present`: the game file still exists.
    pub fn poll(&mut self, since: usize, apk_present: bool) -> Snapshot {
        self.refresh();
        self.stop_if_stalled();
        self.learn();
        let running = self.run.as_ref().map(|r| (r.started, r.started.elapsed()));
        let tracker = self.run.as_ref().and_then(|r| self.progress_info(r.started));
        // A runner that prints no progress at all counts as starting for a short grace period.
        let booting = match (&tracker, running) {
            (Some(info), _) => !info.ready,
            (None, Some((_, elapsed))) => elapsed < self.grace,
            (None, None) => false,
        };
        let state = match (&self.run, &self.last) {
            (Some(_), _) if booting => GameState::Starting,
            (Some(_), _) => GameState::Running,
            (None, Some(f)) if f.crashed => GameState::Crashed,
            (None, Some(_)) if apk_present => GameState::Stopped,
            (None, None) if apk_present => GameState::Ready,
            (None, _) => GameState::Missing,
        };
        let (lines, next) = self.log.lock().expect("log lock").since(since);
        Snapshot {
            state,
            running_secs: self.run.as_ref().map(|r| r.started.elapsed().as_secs()),
            last_session_secs: self.last.as_ref().map(|f| f.secs),
            exit_code: self.last.as_ref().and_then(|f| f.code),
            signal: self.last.as_ref().and_then(|f| f.signal),
            // While a game runs only its own progress counts: before its first line the screen starts at 0 %, not at
            // where the previous session ended.
            progress: if self.run.is_some() { tracker } else { self.last.as_ref().and_then(|f| f.progress.clone()) },
            lines,
            next,
        }
    }

    /// The whole log as text.
    pub fn log_text(&self) -> String {
        let (lines, _) = self.log.lock().expect("log lock").since(0);
        lines.join("\n")
    }

    /// Writes the log to `path`, creating parent directories.
    pub fn save_log(&self, path: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new("")))?;
        std::fs::write(path, self.log_text())
    }
}

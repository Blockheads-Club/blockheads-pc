use crate::*;
use std::collections::VecDeque;
use std::sync::mpsc;
use std::time::Duration;

#[derive(Default)]
struct FakeBackend {
    script: VecDeque<Option<Event>>,
    timeouts: Vec<Duration>,
    presents: u32,
    vsync_ok: bool,
    vsync_calls: Vec<bool>,
    fullscreen_ok: bool,
    fullscreen_calls: Vec<bool>,
    titles: Vec<String>,
}

impl FakeBackend {
    fn new(script: Vec<Event>) -> FakeBackend {
        FakeBackend {
            script: script.into_iter().map(Some).collect(),
            vsync_ok: true,
            fullscreen_ok: true,
            ..Default::default()
        }
    }
}

impl Backend for FakeBackend {
    fn poll_event(&mut self, timeout: Duration) -> Option<Event> {
        self.timeouts.push(timeout);
        self.script.pop_front().flatten()
    }
    fn metrics(&self) -> Metrics {
        Metrics::new((800, 480), (1600, 960))
    }
    fn set_fullscreen(&mut self, on: bool) -> bool {
        self.fullscreen_calls.push(on);
        self.fullscreen_ok
    }
    fn present(&mut self) {
        self.presents += 1;
    }
    fn set_vsync(&mut self, on: bool) -> bool {
        self.vsync_calls.push(on);
        self.vsync_ok || !on
    }
    fn set_title(&mut self, title: &str) {
        self.titles.push(title.into());
    }
}

#[derive(Default)]
struct FakeClock {
    t: Duration,
    sleeps: Vec<Duration>,
    /// Work done per frame (added to the clock when polled for a frame).
    tick: Duration,
    /// Busy-wait steps; each one advances the clock by `spin_step` (100 µs when zero).
    spins: u32,
    spin_step: Duration,
}

impl Clock for FakeClock {
    fn now(&self) -> Duration {
        self.t
    }
    fn sleep(&mut self, d: Duration) {
        self.sleeps.push(d);
        self.t += d;
    }
    fn spin(&mut self) {
        self.spins += 1;
        self.t += if self.spin_step.is_zero() { Duration::from_micros(100) } else { self.spin_step };
    }
}

#[derive(Default)]
struct Rec {
    events: Vec<Event>,
    frames: Vec<Frame>,
    quit_at_frame: Option<u64>,
    quit_on_event: bool,
}

impl Handler for Rec {
    fn event(&mut self, e: &Event) -> Flow {
        self.events.push(e.clone());
        if self.quit_on_event {
            Flow::Quit
        } else {
            Flow::Continue
        }
    }
    fn render_frame(&mut self, f: &Frame) -> Flow {
        self.frames.push(*f);
        if Some(f.number) == self.quit_at_frame {
            Flow::Quit
        } else {
            Flow::Continue
        }
    }
}

fn key(key: Key, pressed: bool, repeat: bool, alt: bool) -> Event {
    Event::Key { key, pressed, repeat, mods: Mods { alt, ..Mods::default() } }
}

fn app(script: Vec<Event>, opts: AppOptions) -> App<FakeBackend, FakeClock> {
    App::new(FakeBackend::new(script), FakeClock::default(), opts)
}

#[test]
fn render_loop_presents_every_frame_and_forwards_events() {
    let m = Metrics::new((100, 50), (200, 100));
    let mut a = app(vec![Event::Resized(m), Event::Focus(true)], AppOptions::default());
    let mut h = Rec { quit_at_frame: Some(2), ..Rec::default() };
    let stats = a.run(&mut h);
    assert_eq!(stats, RunStats { frames: 3, events: 2 });
    assert_eq!(h.events, [Event::Resized(m), Event::Focus(true)]);
    assert_eq!(h.frames.iter().map(|f| f.number).collect::<Vec<_>>(), [0, 1, 2]);
    assert_eq!(h.frames[0].metrics.scale(), 2.0);
    assert_eq!(a.backend().presents, 3);
    assert!(a.vsync());
    assert!(a.backend().timeouts.iter().all(|t| t.is_zero()), "a rendering loop never blocks in the pump");
}

#[test]
fn without_vsync_the_fallback_rate_paces_the_loop() {
    let mut b = FakeBackend::new(vec![]);
    b.vsync_ok = false;
    let mut a = App::new(b, FakeClock::default(), AppOptions { fallback_fps: Some(50), ..AppOptions::default() });
    assert!(!a.vsync());
    assert_eq!(a.backend().vsync_calls, [true, false]);
    let mut h = Rec { quit_at_frame: Some(3), ..Rec::default() };
    a.run(&mut h);
    let dts: Vec<_> = h.frames.iter().map(|f| f.dt).collect();
    assert_eq!(dts[..2], [Duration::ZERO; 2]);
    assert!(dts[2..].iter().all(|d| *d == Duration::from_millis(20)), "{dts:?}");
    assert!(a.fps() > 40.0 && a.fps() < 60.0, "{}", a.fps());
    // vsync off and no cap: no pacing at all
    let mut b = FakeBackend::new(vec![]);
    b.vsync_ok = false;
    let mut a =
        App::new(b, FakeClock::default(), AppOptions { vsync: false, fallback_fps: None, ..AppOptions::default() });
    a.run(&mut Rec { quit_at_frame: Some(2), ..Rec::default() });
}

#[test]
fn fullscreen_keys_are_consumed() {
    let script = vec![
        key(Key::F11, true, false, false),
        key(Key::F11, true, true, false),
        key(Key::F11, false, false, false),
        key(Key::ENTER, true, false, false),
        key(Key::ENTER, true, false, true),
    ];
    let mut a = app(script, AppOptions::default());
    let mut h = Rec { quit_at_frame: Some(0), ..Rec::default() };
    a.run(&mut h);
    assert_eq!(a.backend().fullscreen_calls, [true, false]);
    assert!(!a.is_fullscreen());
    assert_eq!(
        h.events,
        [
            Event::Fullscreen(true),
            key(Key::F11, true, true, false),
            key(Key::F11, false, false, false),
            key(Key::ENTER, true, false, false),
            Event::Fullscreen(false),
        ]
    );
}

#[test]
fn fullscreen_keys_can_be_disabled_and_failures_are_reported_truthfully() {
    let mut a =
        app(vec![key(Key::F11, true, false, false)], AppOptions { fullscreen_keys: false, ..AppOptions::default() });
    let mut h = Rec { quit_at_frame: Some(0), ..Rec::default() };
    a.run(&mut h);
    assert_eq!(h.events, [key(Key::F11, true, false, false)]);
    assert!(a.backend().fullscreen_calls.is_empty());

    let mut b = FakeBackend::new(vec![key(Key::F11, true, false, false)]);
    b.fullscreen_ok = false;
    let mut a = App::new(b, FakeClock::default(), AppOptions { fullscreen: true, ..AppOptions::default() });
    assert!(!a.is_fullscreen(), "the initial request failed");
    let mut h = Rec { quit_at_frame: Some(0), ..Rec::default() };
    a.run(&mut h);
    assert_eq!(h.events, [Event::Fullscreen(false)]);
    assert!(!a.is_fullscreen());

    let a = app(vec![], AppOptions { fullscreen: true, ..AppOptions::default() });
    assert!(a.is_fullscreen());
}

#[test]
fn quit_event_and_handler_quit_end_the_loop() {
    let mut a = app(vec![Event::Quit, Event::Focus(true)], AppOptions::default());
    let mut h = Rec::default();
    let stats = a.run(&mut h);
    assert_eq!((stats.frames, stats.events), (0, 1));
    assert_eq!(h.events, [Event::Quit]);
    assert_eq!(a.backend().script.len(), 1);

    let mut a = app(vec![Event::Focus(true)], AppOptions::default());
    let mut h = Rec { quit_on_event: true, ..Rec::default() };
    assert_eq!(a.run(&mut h).frames, 0);
}

#[test]
fn event_pump_mode_blocks_briefly_and_never_renders() {
    let opts = AppOptions { render_in_loop: false, idle_wait: Duration::from_millis(7), ..AppOptions::default() };
    let mut a = app(vec![Event::Focus(true), Event::Focus(false)], opts);
    let (tx, rx) = mpsc::channel();
    let mut h = ForwardHandler::new(tx);
    // The pump exits when the receiver is gone: drop it after the first event arrives.
    drop(rx);
    let stats = a.run(&mut h);
    assert_eq!((stats.frames, stats.events), (0, 1));
    assert_eq!(a.backend().timeouts[0], Duration::from_millis(7));
    assert_eq!(a.backend().presents, 0);

    let opts = AppOptions { render_in_loop: false, ..AppOptions::default() };
    let mut a = app(vec![Event::Focus(true), Event::Quit], opts);
    let (tx, rx) = mpsc::channel();
    let mut h = ForwardHandler::new(tx);
    a.run(&mut h);
    assert_eq!(rx.try_iter().collect::<Vec<_>>(), [Event::Focus(true), Event::Quit]);
    assert_eq!(
        h.render_frame(&Frame { number: 0, dt: Duration::ZERO, metrics: Metrics::new((1, 1), (1, 1)), fps: 0.0 }),
        Flow::Continue
    );
    // After the first event the pump drains with a zero timeout.
    assert_eq!(a.backend().timeouts[..2], [Duration::from_millis(4), Duration::ZERO]);
}

#[test]
fn idle_pump_keeps_waiting() {
    let opts = AppOptions { render_in_loop: false, ..AppOptions::default() };
    let mut b = FakeBackend::new(vec![Event::Quit]);
    b.script.push_front(None);
    let mut a = App::new(b, FakeClock::default(), opts);
    let (tx, rx) = mpsc::channel();
    assert_eq!(a.run(&mut ForwardHandler::new(tx)), RunStats { frames: 0, events: 1 });
    assert_eq!(rx.try_iter().count(), 1);
    assert_eq!(a.backend().timeouts, [Duration::from_millis(4), Duration::from_millis(4)]);
}

#[test]
fn title_is_forwarded() {
    let mut a = app(vec![], AppOptions::default());
    a.set_title("Blockheads");
    assert_eq!(a.backend().titles, ["Blockheads"]);
}

#[test]
fn pacer_schedules_without_drift() {
    let mut clock = FakeClock::default();
    let mut p = FramePacer::new(Some(100));
    assert_eq!(p.period(), Some(Duration::from_millis(10)));
    assert_eq!(p.fps(), 0.0);
    // Frames that take 3 ms of work each.
    let mut total_slept = Duration::ZERO;
    for _ in 0..5 {
        clock.t += Duration::from_millis(3);
        let t = p.end_frame(&mut clock);
        total_slept += t.slept;
    }
    assert_eq!(clock.t, Duration::from_millis(43), "the first frame starts the schedule, 4 more at 100 Hz");
    assert_eq!(total_slept, Duration::from_millis(28));
    assert!((p.fps() - 100.0).abs() < 1.0);
    // A 15 ms frame is late by 5 ms: no sleep, schedule slips by less than a period.
    clock.t += Duration::from_millis(15);
    assert_eq!(p.end_frame(&mut clock).slept, Duration::ZERO);
    // A very late frame resets the schedule (no burst of catch-up frames).
    clock.t += Duration::from_millis(100);
    assert_eq!(p.end_frame(&mut clock).slept, Duration::ZERO);
    clock.t += Duration::from_millis(2);
    assert_eq!(p.end_frame(&mut clock).slept, Duration::from_millis(8));
    // Disabled pacing never sleeps; a zero rate is the same as none.
    for fps in [None, Some(0)] {
        let mut p = FramePacer::new(fps);
        assert_eq!(p.period(), None);
        assert_eq!(p.end_frame(&mut clock).slept, Duration::ZERO);
    }
}

#[test]
fn metrics_convert_between_logical_and_drawable() {
    let m = Metrics::new((800, 400), (1600, 800));
    assert_eq!(m.scale(), 2.0);
    assert_eq!(m.to_drawable(10.0, 20.0), (20.0, 40.0));
    assert_eq!(m.to_logical(20.0, 40.0), (10.0, 20.0));
    assert_eq!(m.aspect(), 2.0);
    let empty = Metrics::new((0, 0), (0, 0));
    assert_eq!((empty.scale(), empty.aspect()), (1.0, 1.0));
}

#[test]
fn small_types() {
    assert_eq!(Key::ENTER, Key(40));
    assert_ne!(Key::ESCAPE, Key::BACKSPACE);
    assert_eq!(WindowConfig::default().size, (1280, 720));
    assert_eq!(MouseButton::Other(7), MouseButton::Other(7));
    assert_ne!(TouchPhase::Down, TouchPhase::Up);
    let mut c = SystemClock::default();
    let t0 = c.now();
    c.sleep(Duration::from_millis(2));
    assert!(c.now() - t0 >= Duration::from_millis(2));
    // The default precise wait (sleep, then spin) on the real clock.
    let t1 = c.now() + Duration::from_millis(3);
    c.sleep_until(t1);
    assert!(c.now() >= t1);
    c.sleep_until(Duration::ZERO);
    let c = FakeClock { tick: Duration::ZERO, ..Default::default() };
    assert_eq!(c.tick, Duration::ZERO);
}

#[test]
fn precise_waits_sleep_most_of_the_way_then_spin() {
    let mut c = FakeClock::default();
    c.sleep_until(Duration::from_millis(10));
    assert_eq!(c.sleeps, [Duration::from_millis(10) - pacer::SPIN_MARGIN]);
    assert_eq!(c.spins, 15, "the last 1.5 ms in 100 µs steps");
    assert_eq!(c.t, Duration::from_millis(10));
    // A short wait only spins; a past deadline returns at once.
    let mut c = FakeClock::default();
    c.sleep_until(Duration::from_micros(300));
    assert!(c.sleeps.is_empty());
    assert_eq!(c.spins, 3);
    c.sleep_until(Duration::ZERO);
    assert_eq!(c.spins, 3);
    // An oversleeping OS: the spin phase absorbs nothing and the wait still ends past the deadline.
    let mut c = FakeClock { spin_step: Duration::from_millis(1), ..Default::default() };
    c.sleep_until(Duration::from_millis(5));
    assert!(c.t >= Duration::from_millis(5) && c.t < Duration::from_millis(6));
}

#[test]
fn a_frame_cap_holds_its_rate_on_an_absolute_schedule() {
    for (fps, work_ms) in [(30u32, 5u64), (60, 3), (144, 1)] {
        let mut clock = FakeClock::default();
        let mut p = FramePacer::new(Some(fps));
        let mut counter = FpsCounter::default();
        let mut rates = Vec::new();
        let mut presents = Vec::new();
        for _ in 0..(fps * 4) {
            clock.t += Duration::from_millis(work_ms);
            p.end_frame(&mut clock);
            presents.push(clock.t);
            rates.extend(counter.frame(clock.t));
        }
        // Never faster than the cap: every gap is at least one period (within the fake spin step).
        let period = Duration::from_secs_f64(1.0 / fps as f64);
        for w in presents.windows(2) {
            assert!(w[1] - w[0] + Duration::from_micros(100) >= period, "{fps}: gap {:?}", w[1] - w[0]);
        }
        // No drift: frame n is presented within one spin step of start + n periods.
        let n = presents.len() - 1;
        let ideal = presents[0] + period * n as u32;
        assert!(presents[n] >= ideal && presents[n] - ideal <= Duration::from_micros(100), "{fps}: drifted");
        assert!(rates.len() >= 7, "{fps}: about two updates a second, got {}", rates.len());
        for r in &rates[1..] {
            assert!((r - fps as f32).abs() < 0.6, "{fps}: measured {r}");
        }
    }
}

#[test]
fn the_fps_counter_reports_about_twice_a_second() {
    let mut c = FpsCounter::default();
    let ms = Duration::from_millis;
    assert_eq!(c.frame(ms(0)), None, "the first frame starts the window");
    for i in 1..25 {
        assert_eq!(c.frame(ms(i * 20)), None);
    }
    assert_eq!(c.frame(ms(500)), Some(50.0));
    // The next window starts at the report.
    assert_eq!(c.frame(ms(600)), None);
    assert_eq!(c.frame(ms(1000)), Some(4.0));
    // A stall reports its real rate; a zero window is clamped to 1 ms.
    assert_eq!(c.frame(ms(3000)), Some(0.5));
    let mut z = FpsCounter::new(Duration::ZERO);
    assert_eq!(z.frame(ms(0)), None);
    assert_eq!(z.frame(ms(0)), None);
    assert!((z.frame(ms(1)).unwrap() - 2000.0).abs() < 0.01);
}

#[test]
fn overhead_icons() {
    use crate::hud::{overhead_picture, Overhead};
    let mut seen = std::collections::HashSet::new();
    for k in Overhead::ALL {
        let p = overhead_picture(k, 32);
        assert_eq!((p.width, p.height, p.rgba.len()), (32, 32, 32 * 32 * 4));
        assert_eq!(&p.rgba[..4], &[0, 0, 0, 0], "outside the disc");
        assert!(seen.insert(p.rgba), "{k:?} looks like another icon");
    }
    assert_eq!(overhead_picture(Overhead::Speaking, 1).width, 12);
}

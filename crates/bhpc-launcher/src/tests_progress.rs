use crate::progress::*;
use std::time::Duration;

fn secs(s: f64) -> Duration {
    Duration::from_secs_f64(s)
}

fn p(phase: Phase, done: u32, total: u32) -> Line {
    Line::Progress { phase, done, total }
}

#[test]
fn lines_round_trip_and_garbage_is_ignored() {
    for phase in PHASES {
        let line = p(phase, 3, 9);
        assert_eq!(parse_line(&line.render()), Some(line));
        assert_eq!(phase, PHASES[phase.index()]);
    }
    assert_eq!(Line::Ready.render(), "@ready");
    assert_eq!(parse_line("@ready\n"), Some(Line::Ready));
    for bad in [
        "",
        "hello",
        "@ready now",
        "@progress",
        "@progress apk",
        "@progress apk 1",
        "@progress apk 1 x",
        "@progress apk x 1",
        "@progress nope 1 1",
        "@progress apk 1 1 1",
    ] {
        assert_eq!(parse_line(bad), None, "{bad:?}");
    }
}

#[test]
fn output_is_off_until_enabled() {
    // The flag is process wide: only ever turned on here.
    emit(Line::Ready);
    enable();
    assert!(enabled());
    report(Phase::Apk, 0, 1);
}

#[test]
fn default_weights_follow_the_spec() {
    let m = Model::default();
    assert!((m.weights.iter().sum::<f64>() - 1.0).abs() < 1e-9);
    assert_eq!(m.weights, DEFAULT_WEIGHTS);
    assert!((m.usual_secs.iter().sum::<f64>() - DEFAULT_TOTAL_SECS).abs() < 1e-9);
}

#[test]
fn learned_weights_are_shares_of_the_measured_time() {
    assert_eq!(Model::learn(&[]), Model::default());
    assert_eq!(Model::learn(&[[0.0; 6]]), Model::default());
    let m = Model::learn(&[[1.0, 1.0, 4.0, 2.0, 1.0, 1.0], [1.0, 1.0, 6.0, 2.0, 1.0, 1.0]]);
    assert!((m.weights[2] - 5.0 / 11.0).abs() < 1e-9 && (m.weights[0] - 1.0 / 11.0).abs() < 1e-9);
    assert_eq!(m.usual_secs[2], 5.0);
    // A step that took no time still gets a sliver, and the weights stay a whole.
    let m = Model::learn(&[[0.0, 0.0, 10.0, 0.0, 0.0, 0.0]]);
    assert!(m.weights[0] >= 0.017 && (m.weights.iter().sum::<f64>() - 1.0).abs() < 1e-9);
}

#[test]
fn history_keeps_the_last_five_starts_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = history_path(&dir.path().join("sub"));
    assert_eq!(History::load(&path), History::default());
    let mut h = History::default();
    for i in 0..7 {
        h.push([f64::from(i); 6]);
    }
    assert_eq!(h.starts.len(), HISTORY_LEN);
    assert_eq!(h.starts[0][0], 2.0);
    h.save(&path).unwrap();
    assert_eq!(History::load(&path), h);
    assert_eq!(h.model(), Model::learn(&h.starts));
    std::fs::write(&path, "starts = 7").unwrap();
    assert_eq!(History::load(&path), History::default());
}

#[test]
fn percent_follows_the_steps_and_never_goes_back() {
    let mut t = Tracker::default();
    assert!(!t.started());
    assert_eq!(t.info(secs(0.0)).percent, 0.0);
    t.feed(p(Phase::Apk, 0, 1), secs(0.0));
    assert!(t.started());
    t.feed(p(Phase::Apk, 1, 1), secs(0.4));
    assert!((t.info(secs(0.4)).percent - 10.0).abs() < 1e-9);
    t.feed(p(Phase::Constructors, 0, 100), secs(1.0));
    assert!((t.info(secs(1.0)).percent - 28.0).abs() < 1e-9);
    t.feed(p(Phase::Constructors, 50, 100), secs(2.0));
    let half = t.info(secs(2.0));
    assert!((half.percent - 39.0).abs() < 1e-9 && half.step == 3 && half.phase == "constructors");
    // Over-reported counts clamp, a lower report does not move the bar back.
    t.feed(p(Phase::Constructors, 150, 100), secs(2.5));
    assert!((t.info(secs(2.5)).percent - 50.0).abs() < 1e-9);
    t.feed(p(Phase::Constructors, 10, 100), secs(2.6));
    assert!((t.info(secs(2.6)).percent - 50.0).abs() < 1e-9);
}

#[test]
fn steps_without_counts_ease_towards_their_share() {
    let mut t = Tracker::default();
    t.feed(p(Phase::Libraries, 0, 0), secs(1.0));
    let early = t.info(secs(1.1)).percent;
    let later = t.info(secs(3.0)).percent;
    let much_later = t.info(secs(300.0)).percent;
    assert!(early < later && later < much_later);
    // Never past 95 % of the step's share (10 + 18 = 28).
    assert!(much_later < 28.0 && much_later > 26.0, "{much_later}");
}

#[test]
fn estimate_waits_for_three_seconds_and_fifteen_percent() {
    let mut t = Tracker::default();
    t.feed(p(Phase::Constructors, 90, 100), secs(1.0));
    assert_eq!(t.info(secs(2.0)).eta_secs, None, "too early");
    let i = t.info(secs(4.0));
    assert!(i.percent > 15.0);
    let eta = i.eta_secs.unwrap();
    assert!((eta - 4.0 * (100.0 - i.percent) / i.percent).abs() < 1e-9);
    let mut low = Tracker::default();
    low.feed(p(Phase::Apk, 0, 1), secs(0.0));
    assert_eq!(low.info(secs(10.0)).eta_secs, None, "too little progress");
}

#[test]
fn slow_and_stalled_steps() {
    let mut t = Tracker::default();
    t.feed(p(Phase::Apk, 0, 1), secs(0.0));
    // The apk step usually takes 2 s: slow after 4 s (the floor), not before.
    assert!(!t.info(secs(3.9)).slow);
    assert!(t.info(secs(4.1)).slow);
    // A step that usually takes long is slow at 15 s at the latest.
    let mut long = Tracker::new(Model { weights: DEFAULT_WEIGHTS, usual_secs: [100.0; 6], learned: false });
    long.feed(p(Phase::Frame, 0, 0), secs(0.0));
    assert!(!long.info(secs(14.0)).slow);
    assert!(long.info(secs(15.0)).slow);
    assert!(!Tracker::default().info(secs(100.0)).slow, "nothing reported yet");
    // 60 s without a line is a stall; any line resets it.
    let mut s = Tracker::default();
    s.feed(p(Phase::Frame, 0, 0), secs(1.0));
    assert!(!s.info(secs(60.9)).stalled);
    assert!(s.info(secs(61.0)).stalled);
    s.feed(p(Phase::Frame, 0, 0), secs(61.5));
    assert!(!s.info(secs(62.0)).stalled);
    assert!(!Tracker::default().info(secs(100.0)).stalled);
}

#[test]
fn ready_finishes_the_bar_and_yields_the_step_times() {
    let mut t = Tracker::with_stall(Model::default(), Duration::from_secs(5));
    assert_eq!(t.durations(), None);
    t.feed(p(Phase::Apk, 0, 1), secs(0.0));
    t.feed(p(Phase::Libraries, 0, 0), secs(1.0));
    t.feed(p(Phase::Constructors, 0, 2), secs(3.0));
    // The runtime step is never mentioned: it counts as instant.
    t.feed(p(Phase::Launch, 1, 5), secs(7.0));
    let mid = t.info(secs(7.5));
    assert_eq!(mid.step_secs[0], Some(1.0));
    assert_eq!(mid.step_secs[2], Some(4.0));
    assert_eq!(mid.step_secs[3], Some(0.0));
    assert_eq!(mid.step_secs[4], None);
    t.feed(Line::Ready, secs(9.0));
    let done = t.info(secs(9.0));
    assert!(done.ready && done.percent == 100.0 && done.eta_secs.is_none() && !done.slow && !done.stalled);
    assert_eq!(t.durations(), Some([1.0, 2.0, 4.0, 0.0, 2.0, 0.0]));
    assert_eq!(done.step_secs, vec![Some(1.0), Some(2.0), Some(4.0), Some(0.0), Some(2.0), Some(0.0)]);
    // Ready without any progress line (an old runner) is fine too.
    let mut bare = Tracker::default();
    bare.feed(Line::Ready, secs(2.0));
    assert!(bare.started() && bare.info(secs(2.0)).ready);
    // A second ready does not move the time.
    bare.feed(Line::Ready, secs(5.0));
    assert_eq!(bare.durations().unwrap().iter().sum::<f64>(), 0.0);
}

#[test]
fn estimate_from_earlier_starts() {
    // Three starts; the slow one does not move the medians (1, 2, 3, 4, 0, 10 = 20 s).
    let h = [[1.0, 2.0, 3.0, 4.0, 0.0, 10.0], [1.0, 2.0, 3.0, 4.0, 0.0, 10.0], [9.0, 9.0, 9.0, 9.0, 9.0, 60.0]];
    let m = Model::learn(&h);
    assert!(m.learned && !Model::default().learned);
    assert_eq!(m.usual_secs, [1.0, 2.0, 3.0, 4.0, 0.0, 10.0]);
    assert_eq!(Model::learn(&h[..2]).usual_secs, [1.0, 2.0, 3.0, 4.0, 0.0, 10.0], "even count");

    // Shown from the first line, before 3 s and 15 %: the usual 20 s minus the part of the apk step done.
    let mut t = Tracker::new(m.clone());
    t.feed(p(Phase::Apk, 1, 2), secs(0.5));
    assert!((t.info(secs(0.5)).eta_secs.unwrap() - 19.5).abs() < 1e-9);
    // Half way through the constructors after 4.5 s, as usual: 1.5 + 4 + 0 + 10 left.
    t.feed(p(Phase::Constructors, 50, 100), secs(4.5));
    assert!((t.info(secs(4.5)).eta_secs.unwrap() - 15.5).abs() < 1e-9);
    // Twice as slow as usual: twice the time left.
    let mut slow = Tracker::new(m.clone());
    slow.feed(p(Phase::Constructors, 50, 100), secs(9.0));
    assert!((slow.info(secs(9.0)).eta_secs.unwrap() - 31.0).abs() < 1e-9);
    // The pace is bounded: very fast reads as half the usual, very slow as three times.
    let mut fast = Tracker::new(m.clone());
    fast.feed(p(Phase::Constructors, 50, 100), secs(0.1));
    assert!((fast.info(secs(0.1)).eta_secs.unwrap() - 7.75).abs() < 1e-9);
    let mut crawl = Tracker::new(m.clone());
    crawl.feed(p(Phase::Constructors, 50, 100), secs(100.0));
    assert!((crawl.info(secs(100.0)).eta_secs.unwrap() - 46.5).abs() < 1e-9);
    // Waiting for the first frame (no counts) past its usual 10 s: a moment left, not zero or negative.
    let mut late = Tracker::new(m);
    late.feed(p(Phase::Frame, 0, 0), secs(10.0));
    assert!((late.info(secs(15.0)).eta_secs.unwrap() - 5.0).abs() < 1e-9);
    assert_eq!(late.info(secs(40.0)).eta_secs, Some(1.0));
    late.feed(Line::Ready, secs(41.0));
    assert_eq!(late.info(secs(41.0)).eta_secs, None);
}

use crate::music::decode::Decoder;
use crate::music::player::{MediaPlayer, PlayerEvent, PlayerState, MEDIA_ERROR_UNKNOWN};
use crate::music::source::SliceSource;
use crate::music::MusicError;
use crate::Mixer;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use symphonia::core::io::MediaSource;

// Self-made one second 440 Hz sine tones at 22.05 kHz (ffmpeg lavfi, no third-party audio).
const M4A: &[u8] = include_bytes!("music_fixtures/tone.m4a");
const MP3: &[u8] = include_bytes!("music_fixtures/tone.mp3");
const OGG: &[u8] = include_bytes!("music_fixtures/tone.ogg");

const RATE: u32 = 8000;

/// A 16-bit PCM WAV of a square-ish wave: `frames` frames at `RATE`, amplitude `amp`.
fn wav(channels: u16, frames: u32, amp: i16) -> Vec<u8> {
    let data_len = frames * channels as u32 * 2;
    let mut v = Vec::new();
    v.extend(b"RIFF");
    v.extend((36 + data_len).to_le_bytes());
    v.extend(b"WAVEfmt ");
    v.extend(16u32.to_le_bytes());
    v.extend(1u16.to_le_bytes());
    v.extend(channels.to_le_bytes());
    v.extend(RATE.to_le_bytes());
    v.extend((RATE * channels as u32 * 2).to_le_bytes());
    v.extend((channels * 2).to_le_bytes());
    v.extend(16u16.to_le_bytes());
    v.extend(b"data");
    v.extend(data_len.to_le_bytes());
    for i in 0..frames {
        for c in 0..channels {
            let s = if (i / 20) % 2 == 0 { amp } else { -amp };
            v.extend((if c == 0 { s } else { s / 2 }).to_le_bytes());
        }
    }
    v
}

fn peak(out: &[i16]) -> i16 {
    out.iter().map(|s| s.saturating_abs()).max().unwrap_or(0)
}

/// Play `ms` of mixer time in 20 ms steps, pumping the player each step. Returns the loudest sample.
fn run(m: &Mixer, p: &MediaPlayer, ms: u32) -> i16 {
    let mut loud = 0;
    for _ in 0..ms / 20 {
        p.pump();
        let mut out = vec![0i16; 44100 * 2 / 50];
        m.fill(&mut out);
        loud = loud.max(peak(&out));
    }
    loud
}

fn setup() -> (Mixer, MediaPlayer, Arc<Mutex<Vec<PlayerEvent>>>) {
    let m = Mixer::new(44100);
    let p = MediaPlayer::new(m.clone());
    let ev = Arc::new(Mutex::new(Vec::new()));
    let e2 = ev.clone();
    p.set_listener(move |e| e2.lock().unwrap().push(e));
    (m, p, ev)
}

fn ready(bytes: Vec<u8>) -> (Mixer, MediaPlayer, Arc<Mutex<Vec<PlayerEvent>>>) {
    let (m, p, ev) = setup();
    p.set_data_source_bytes(bytes).unwrap();
    p.prepare().unwrap();
    (m, p, ev)
}

fn illegal<T: std::fmt::Debug>(r: Result<T, MusicError>) {
    assert!(matches!(r, Err(MusicError::IllegalState(_))), "{r:?}");
}

// ---------- SliceSource ----------

#[test]
fn slice_source_is_a_window() {
    let data: Vec<u8> = (0..100).collect();
    let mut s = SliceSource::new(Cursor::new(data.clone()), 10, Some(20)).unwrap();
    assert_eq!((s.len(), s.is_empty(), s.is_seekable(), s.byte_len()), (20, false, true, Some(20)));
    let mut b = [0u8; 5];
    s.read_exact(&mut b).unwrap();
    assert_eq!(b, [10, 11, 12, 13, 14]);
    assert_eq!(s.seek(SeekFrom::Current(-2)).unwrap(), 3);
    s.read_exact(&mut b).unwrap();
    assert_eq!(b[0], 13);
    assert_eq!(s.seek(SeekFrom::End(-1)).unwrap(), 19);
    let mut rest = Vec::new();
    s.read_to_end(&mut rest).unwrap();
    assert_eq!(rest, [29]);
    // Past the end reads nothing; before the start is an error.
    assert_eq!(s.seek(SeekFrom::Start(500)).unwrap(), 500);
    assert_eq!(s.read(&mut b).unwrap(), 0);
    assert!(s.seek(SeekFrom::Current(-1000)).is_err());
}

#[test]
fn slice_source_clamps_and_defaults_the_length() {
    let s = SliceSource::new(Cursor::new(vec![0u8; 50]), 40, Some(1000)).unwrap();
    assert_eq!(s.len(), 10);
    let s = SliceSource::new(Cursor::new(vec![0u8; 50]), 10, None).unwrap();
    assert_eq!(s.len(), 40);
    let s = SliceSource::new(Cursor::new(vec![0u8; 5]), 10, None).unwrap();
    assert!(s.is_empty());
}

// ---------- Decoder ----------

#[test]
fn decodes_every_container() {
    for (name, bytes, ext) in [("m4a", M4A, None), ("mp3", MP3, Some("mp3")), ("ogg", OGG, None)] {
        let mut d = Decoder::open(Box::new(Cursor::new(bytes.to_vec())), ext).unwrap();
        assert_eq!(d.rate(), 22050, "{name}");
        let dur = d.duration_ms().unwrap_or(1000);
        assert!((900..=1100).contains(&dur), "{name}: {dur}");
        let (mut total, mut loud) = (0usize, 0i16);
        while let Some(f) = d.next_frames().unwrap() {
            total += f.len();
            loud = loud.max(f.iter().map(|x| x[0].saturating_abs()).max().unwrap());
        }
        assert!((20000..=24000).contains(&total), "{name}: {total}");
        assert!(loud > 2000, "{name}: {loud}");
        // Past the end the stream stays at its end.
        assert_eq!(d.next_frames().unwrap(), None);
    }
}

#[test]
fn wav_mono_is_duplicated_and_extra_channels_are_dropped() {
    let mut d = Decoder::open(Box::new(Cursor::new(wav(1, 100, 1000))), Some("wav")).unwrap();
    let f = d.next_frames().unwrap().unwrap();
    assert_eq!(f.len(), 100);
    assert!(f.iter().all(|x| x[0] == x[1]));
    assert_eq!(d.duration_ms(), Some(12));
    // Skipping packets of other tracks / empty packets happens inside the loop; a stereo file is plain.
    let mut d = Decoder::open(Box::new(Cursor::new(wav(2, 100, 1000))), None).unwrap();
    let f = d.next_frames().unwrap().unwrap();
    assert_eq!(f[0], [1000, 500]);
}

#[test]
fn seek_lands_near_the_target() {
    let mut d = Decoder::open(Box::new(Cursor::new(M4A.to_vec())), None).unwrap();
    d.seek_ms(500).unwrap();
    let mut left = 0;
    while let Some(f) = d.next_frames().unwrap() {
        left += f.len();
    }
    assert!((10500..=13000).contains(&left), "{left}");
    // A WAV has a timebase of 1/rate; seeking beyond the end is an error or clamps, not a panic.
    let mut d = Decoder::open(Box::new(Cursor::new(wav(2, 800, 1000))), None).unwrap();
    d.seek_ms(50).unwrap();
    let left: usize = std::iter::from_fn(|| d.next_frames().unwrap()).map(|f| f.len()).sum();
    assert_eq!(left, 800 - 400);
    let _ = d.seek_ms(60_000);
}

#[test]
fn errors_display() {
    for (e, text) in [
        (MusicError::IllegalState("x"), "illegal state: x"),
        (MusicError::Io("y".into()), "i/o error: y"),
        (MusicError::Decode("z".into()), "decode error: z"),
    ] {
        assert_eq!(e.to_string(), text);
    }
}

#[test]
fn garbage_is_rejected() {
    let r = Decoder::open(Box::new(Cursor::new(vec![7u8; 4096])), None);
    assert!(matches!(r, Err(MusicError::Decode(_))));
    let r = Decoder::open(Box::new(Cursor::new(Vec::new())), None);
    assert!(r.is_err());
}

/// A source that starts failing reads once `limit` bytes were handed out, and whose seek can be
/// made to fail.
struct Flaky {
    inner: Cursor<Vec<u8>>,
    limit: usize,
    seek_fails: Arc<AtomicBool>,
}

impl Read for Flaky {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.inner.position() as usize >= self.limit {
            return Err(std::io::Error::other("disk on fire"));
        }
        self.inner.read(buf)
    }
}

impl Seek for Flaky {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        if self.seek_fails.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("seek failed"));
        }
        self.inner.seek(to)
    }
}

impl MediaSource for Flaky {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.inner.get_ref().len() as u64)
    }
}

fn flaky(limit: usize) -> (Box<Flaky>, Arc<AtomicBool>) {
    let f = Arc::new(AtomicBool::new(false));
    (Box::new(Flaky { inner: Cursor::new(wav(2, 60_000, 3000)), limit, seek_fails: f.clone() }), f)
}

#[test]
fn read_errors_surface() {
    let (src, _) = flaky(100_000);
    let mut d = Decoder::open(src, None).unwrap();
    let err = loop {
        match d.next_frames() {
            Ok(Some(_)) => {}
            other => break other,
        }
    };
    assert!(matches!(err, Err(MusicError::Io(_))), "{err:?}");
    assert!(err.unwrap_err().to_string().contains("i/o error"));
}

// ---------- MediaPlayer ----------

#[test]
fn full_lifecycle_to_completion() {
    let (m, p, ev) = setup();
    assert_eq!(p.state(), PlayerState::Idle);
    assert_eq!(p.stream_id(), None);
    p.set_data_source_bytes(wav(2, RATE / 2, 8000)).unwrap();
    assert_eq!(p.state(), PlayerState::Initialized);
    p.prepare().unwrap();
    assert_eq!(p.state(), PlayerState::Prepared);
    assert_eq!(p.duration_ms(), 500);
    assert_eq!(p.position_ms(), 0);
    assert!(p.stream_id().is_some());
    // Prepared but not started: silence.
    assert_eq!(run(&m, &p, 100), 0);
    p.start().unwrap();
    p.start().unwrap();
    assert!(p.is_playing());
    assert!(run(&m, &p, 200) > 1000);
    let pos = p.position_ms();
    assert!((150..=250).contains(&pos), "{pos}");
    assert!(ev.lock().unwrap().is_empty());
    run(&m, &p, 400);
    assert_eq!(ev.lock().unwrap().as_slice(), [PlayerEvent::Completion]);
    assert_eq!(p.state(), PlayerState::PlaybackCompleted);
    assert!(!p.is_playing());
    assert_eq!(p.position_ms(), 500);
    // Nothing more happens.
    run(&m, &p, 100);
    assert_eq!(ev.lock().unwrap().len(), 1);
    // start() from PlaybackCompleted plays it again from the top.
    p.start().unwrap();
    assert!(run(&m, &p, 100) > 1000);
    assert!(p.position_ms() < 150);
    p.release();
    assert_eq!(p.state(), PlayerState::End);
    assert_eq!(m.stream_count(), 0);
}

#[test]
fn plays_real_codecs_through_the_mixer() {
    for bytes in [M4A, MP3, OGG] {
        let (m, p, ev) = ready(bytes.to_vec());
        let dur = p.duration_ms();
        assert!((900..=1100).contains(&dur) || dur == -1, "{dur}");
        p.start().unwrap();
        assert!(run(&m, &p, 600) > 3000);
        run(&m, &p, 700);
        assert_eq!(ev.lock().unwrap().as_slice(), [PlayerEvent::Completion]);
    }
}

#[test]
fn illegal_states() {
    let (_m, p, _) = setup();
    illegal(p.prepare());
    illegal(p.start());
    illegal(p.pause());
    illegal(p.stop());
    illegal(p.seek_to(0));
    p.set_data_source_bytes(wav(2, 800, 100)).unwrap();
    illegal(p.set_data_source_bytes(wav(2, 800, 100)));
    illegal(p.start());
    illegal(p.seek_to(0));
    p.prepare().unwrap();
    illegal(p.prepare());
    illegal(p.pause());
    // Idle after reset: data source gone, but a new one can be set.
    p.reset();
    assert_eq!(p.state(), PlayerState::Idle);
    assert_eq!(p.stream_id(), None);
    assert_eq!(p.position_ms(), 0);
    assert_eq!(p.duration_ms(), -1);
    p.set_data_source_bytes(wav(2, 800, 100)).unwrap();
    p.release();
    p.release();
    p.reset();
    assert_eq!(p.state(), PlayerState::End);
    illegal(p.start());
    illegal(p.stop());
    illegal(p.set_data_source_bytes(Vec::new()));
}

#[test]
fn pause_and_resume() {
    let (m, p, _) = ready(wav(2, RATE * 2, 8000));
    p.start().unwrap();
    run(&m, &p, 200);
    p.pause().unwrap();
    p.pause().unwrap();
    assert_eq!(p.state(), PlayerState::Paused);
    let at = p.position_ms();
    assert_eq!(run(&m, &p, 200), 0);
    assert_eq!(p.position_ms(), at);
    p.start().unwrap();
    assert!(run(&m, &p, 100) > 1000);
    assert!(p.position_ms() > at);
}

#[test]
fn stop_requires_prepare_again() {
    let (m, p, _) = ready(wav(2, RATE * 2, 8000));
    p.start().unwrap();
    run(&m, &p, 100);
    p.stop().unwrap();
    p.stop().unwrap();
    assert_eq!(p.state(), PlayerState::Stopped);
    illegal(p.start());
    assert_eq!(run(&m, &p, 100), 0);
    p.prepare().unwrap();
    assert_eq!(p.position_ms(), 0);
    p.start().unwrap();
    assert!(run(&m, &p, 100) > 1000);
    // stop() straight from Prepared works too.
    let (_m, p, _) = ready(wav(2, 800, 100));
    p.stop().unwrap();
}

#[test]
fn seeking() {
    let (m, p, ev) = ready(wav(2, RATE * 2, 8000));
    p.seek_to(1000).unwrap();
    assert_eq!(ev.lock().unwrap().as_slice(), [PlayerEvent::SeekComplete]);
    assert_eq!(p.position_ms(), 1000);
    p.start().unwrap();
    run(&m, &p, 200);
    let pos = p.position_ms();
    assert!((1150..=1250).contains(&pos), "{pos}");
    // Seek while paused stays paused.
    p.pause().unwrap();
    p.seek_to(100).unwrap();
    assert_eq!(p.state(), PlayerState::Paused);
    assert_eq!(run(&m, &p, 100), 0);
    assert_eq!(p.position_ms(), 100);
    // Seek to the end completes, then seeking a completed player parks it paused.
    p.start().unwrap();
    p.seek_to(1990).unwrap();
    run(&m, &p, 100);
    assert_eq!(p.state(), PlayerState::PlaybackCompleted);
    p.seek_to(500).unwrap();
    assert_eq!(p.state(), PlayerState::Paused);
    assert_eq!(p.position_ms(), 500);
}

#[test]
fn looping_wraps_around_and_can_be_switched_off() {
    let (m, p, ev) = ready(wav(2, RATE / 5, 8000));
    assert!(!p.is_looping());
    p.set_looping(true);
    assert!(p.is_looping());
    p.start().unwrap();
    assert!(run(&m, &p, 1000) > 1000);
    assert!(ev.lock().unwrap().is_empty());
    assert!(p.position_ms() < 200);
    assert!(p.is_playing());
    // Without looping the current pass runs out and the player completes.
    p.set_looping(false);
    run(&m, &p, 1500);
    assert_eq!(ev.lock().unwrap().as_slice(), [PlayerEvent::Completion]);
}

#[test]
fn looping_switched_on_at_the_end_revives_the_stream() {
    let (m, p, ev) = ready(wav(2, RATE / 5, 8000));
    p.start().unwrap();
    // Everything is queued and the decoder is at its end, but the mixer still has audio to play.
    p.pump();
    p.set_looping(true);
    assert!(run(&m, &p, 800) > 1000);
    assert!(ev.lock().unwrap().is_empty());
    // On a completed player the flag alone does not restart anything.
    p.set_looping(false);
    run(&m, &p, 1500);
    assert_eq!(p.state(), PlayerState::PlaybackCompleted);
    p.set_looping(true);
    run(&m, &p, 100);
    assert_eq!(p.state(), PlayerState::PlaybackCompleted);
}

#[test]
fn volume_scales_the_output() {
    let (m, p, _) = ready(wav(2, RATE * 4, 8000));
    p.start().unwrap();
    let full = run(&m, &p, 100);
    p.set_volume(0.5, 0.5);
    let half = run(&m, &p, 100);
    assert!((half as i32 * 2 - full as i32).abs() < 16, "{half} {full}");
    p.set_volume(-1.0, -1.0);
    assert_eq!(run(&m, &p, 100), 0);
    // Set before the stream exists, it is applied when the stream is created.
    let (m, p, _) = setup();
    p.set_volume(0.0, 0.0);
    p.set_data_source_bytes(wav(2, RATE, 8000)).unwrap();
    p.prepare().unwrap();
    p.start().unwrap();
    assert_eq!(run(&m, &p, 100), 0);
}

#[test]
fn files_paths_and_fds() {
    let dir = std::env::temp_dir().join(format!("bhpc-music-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tone.wav");
    std::fs::write(&path, wav(2, RATE, 8000)).unwrap();
    let (m, p, _) = setup();
    p.set_data_source_path(&path).unwrap();
    p.prepare().unwrap();
    assert_eq!(p.duration_ms(), 1000);
    p.start().unwrap();
    assert!(run(&m, &p, 100) > 1000);
    assert!(matches!(p.set_data_source_path(&dir.join("missing.wav")), Err(MusicError::Io(_))));

    // The game's case: an entry inside a larger archive file, addressed by fd + offset + length.
    let tone = wav(2, RATE / 2, 8000);
    let mut blob = vec![0xAAu8; 777];
    blob.extend(&tone);
    blob.extend(vec![0xBBu8; 333]);
    let big = dir.join("archive.bin");
    std::fs::write(&big, &blob).unwrap();
    for length in [tone.len() as i64, -1] {
        let (m, p, _) = setup();
        p.set_data_source_fd(std::fs::File::open(&big).unwrap(), 777, length).unwrap();
        p.prepare().unwrap();
        assert_eq!(p.duration_ms(), 500);
        p.start().unwrap();
        assert!(run(&m, &p, 100) > 1000);
    }
    // An unseekable descriptor (a pipe) is not a usable source. (A directory is not portable for this: tmpfs refuses
    // to seek it, btrfs and APFS do not.)
    #[cfg(unix)]
    {
        let (_m, p, _) = setup();
        let mut child = std::process::Command::new("true").stdout(std::process::Stdio::piped()).spawn().unwrap();
        let pipe = std::fs::File::from(std::os::fd::OwnedFd::from(child.stdout.take().unwrap()));
        assert!(matches!(p.set_data_source_fd(pipe, 0, 10), Err(MusicError::Io(_))));
        child.wait().unwrap();
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn prepare_failures_put_the_player_in_error() {
    let (_m, p, ev) = setup();
    p.set_data_source_bytes(vec![1u8; 2000]).unwrap();
    assert!(matches!(p.prepare(), Err(MusicError::Decode(_))));
    assert_eq!(p.state(), PlayerState::Error);
    assert_eq!(ev.lock().unwrap().as_slice(), [PlayerEvent::Error { what: MEDIA_ERROR_UNKNOWN, extra: 0 }]);
    illegal(p.start());
    p.reset();
    assert_eq!(p.state(), PlayerState::Idle);
    // prepare_async: no Prepared event on failure, one on success.
    p.set_data_source_bytes(vec![1u8; 2000]).unwrap();
    assert!(p.prepare_async().is_err());
    assert!(!ev.lock().unwrap().contains(&PlayerEvent::Prepared));
    p.reset();
    ev.lock().unwrap().clear();
    p.set_data_source_bytes(wav(2, 800, 100)).unwrap();
    p.prepare_async().unwrap();
    assert_eq!(ev.lock().unwrap().as_slice(), [PlayerEvent::Prepared]);
}

#[test]
fn prepare_without_a_source_after_a_stop_reuses_the_decoder() {
    // Covered by stop_requires_prepare_again; here the Stopped player fails to rewind.
    let (_m, p, ev) = setup();
    let (src, fails) = flaky(usize::MAX);
    p.set_data_source_reader(src, None).unwrap();
    p.prepare().unwrap();
    p.stop().unwrap();
    fails.store(true, Ordering::SeqCst);
    assert!(p.prepare().is_err());
    assert_eq!(p.state(), PlayerState::Error);
    assert!(ev.lock().unwrap().iter().any(|e| matches!(e, PlayerEvent::Error { .. })));
}

#[test]
fn seek_failures_put_the_player_in_error() {
    let (_m, p, ev) = setup();
    let (src, fails) = flaky(usize::MAX);
    p.set_data_source_reader(src, None).unwrap();
    p.prepare().unwrap();
    fails.store(true, Ordering::SeqCst);
    assert!(p.seek_to(100).is_err());
    assert_eq!(p.state(), PlayerState::Error);
    assert_eq!(ev.lock().unwrap().len(), 1);
}

#[test]
fn decode_errors_during_playback_are_reported_once() {
    let (m, p, ev) = setup();
    let (src, _) = flaky(50_000);
    p.set_data_source_reader(src, None).unwrap();
    p.prepare().unwrap();
    p.start().unwrap();
    run(&m, &p, 4000);
    assert_eq!(p.state(), PlayerState::Error);
    assert_eq!(ev.lock().unwrap().as_slice(), [PlayerEvent::Error { what: MEDIA_ERROR_UNKNOWN, extra: 0 }]);
    assert!(!p.is_playing());
    // pump on an errored player does nothing.
    run(&m, &p, 100);
    assert_eq!(ev.lock().unwrap().len(), 1);
}

#[test]
fn dropping_the_player_frees_its_stream() {
    let (m, p, _) = ready(wav(2, 800, 100));
    let p2 = p.clone();
    assert_eq!(m.stream_count(), 1);
    drop(p);
    assert_eq!(m.stream_count(), 1);
    drop(p2);
    assert_eq!(m.stream_count(), 0);
}

#[test]
fn the_pump_thread_drives_playback_to_completion() {
    let (m, p, _) = setup();
    let (tx, rx) = mpsc::channel();
    p.set_listener(move |e| {
        let _ = tx.send(e);
    });
    p.set_data_source_bytes(wav(2, RATE / 4, 8000)).unwrap();
    p.prepare_async().unwrap();
    assert_eq!(rx.recv().unwrap(), PlayerEvent::Prepared);
    let _guard = p.spawn_pump(Duration::from_millis(2));
    p.start().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut out = vec![0i16; 882 * 2];
    loop {
        m.fill(&mut out);
        if let Ok(e) = rx.try_recv() {
            assert_eq!(e, PlayerEvent::Completion);
            break;
        }
        assert!(Instant::now() < deadline, "no completion");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Against one of the game's own music files: `BHPC_MUSIC_MP4=/path/to/tikopia.mp4 cargo test -- --ignored`.
#[test]
#[ignore = "needs a game music file in BHPC_MUSIC_MP4"]
fn game_music_file_plays() {
    let path = std::env::var("BHPC_MUSIC_MP4").expect("BHPC_MUSIC_MP4");
    let (m, p, ev) = setup();
    p.set_data_source_path(std::path::Path::new(&path)).unwrap();
    p.prepare().unwrap();
    assert!(p.duration_ms() > 30_000, "{}", p.duration_ms());
    p.start().unwrap();
    assert!(run(&m, &p, 2000) > 1000);
    p.seek_to(p.duration_ms() as u32 - 1000).unwrap();
    run(&m, &p, 2000);
    assert_eq!(ev.lock().unwrap().last(), Some(&PlayerEvent::Completion));
}

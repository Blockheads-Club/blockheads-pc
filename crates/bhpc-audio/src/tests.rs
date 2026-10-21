use crate::mixer::*;
use crate::pcm::*;
use crate::track::*;
use std::time::Duration;

fn out(m: &Mixer, frames: usize) -> Vec<i16> {
    let mut v = vec![0i16; frames * 2];
    m.fill(&mut v);
    v
}

fn ramp(n: i16) -> Vec<Frame> {
    (1..=n).map(|i| [i * 100, -i * 100]).collect()
}

fn playing(m: &Mixer, rate: u32, max: usize) -> StreamId {
    let s = m.create_stream(rate, max);
    m.set_state(s, PlayState::Playing).unwrap();
    s
}

#[test]
fn pcm_decoding() {
    let f = |sample, channels| PcmFormat { rate_hz: 8000, channels, sample };
    assert_eq!(SampleFormat::U8.bytes() + SampleFormat::S16.bytes() + SampleFormat::F32.bytes(), 7);
    assert_eq!(f(SampleFormat::S16, 2).frame_bytes(), 4);
    assert_eq!(f(SampleFormat::U8, 1).decode(&[128, 255, 0]), vec![[0, 0], [127 << 8, 127 << 8], [-32768, -32768]]);
    assert_eq!(f(SampleFormat::S16, 2).decode(&[1, 0, 0xff, 0xff, 9]), vec![[1, -1]]);
    assert_eq!(f(SampleFormat::S16, 1).decode(&[0x34, 0x12]), vec![[0x1234, 0x1234]]);
    let fl = |a: f32, b: f32| [a.to_le_bytes(), b.to_le_bytes()].concat();
    assert_eq!(f(SampleFormat::F32, 2).decode(&fl(0.5, -2.0)), vec![[16383, -32767]]);
    assert_eq!(f(SampleFormat::F32, 2).decode(&fl(f32::NAN, 9.0)), vec![[0, 32767]]);
    assert!(f(SampleFormat::F32, 2).decode(&[]).is_empty());
}

#[test]
fn plays_a_queue_in_order_and_reports_events() {
    let m = Mixer::new(44100);
    assert_eq!(m.out_rate(), 44100);
    let s = m.create_stream(44100, 4);
    m.enqueue(s, ramp(3)).unwrap();
    m.enqueue(s, ramp(2)).unwrap();
    assert_eq!(m.queue_state(s), Ok((2, 0)));
    assert_eq!(m.queued_frames(s), Ok(5));
    // Stopped: silence, nothing consumed.
    assert_eq!(out(&m, 2), [0; 4]);
    m.set_state(s, PlayState::Playing).unwrap();
    assert_eq!(m.state(s), Ok(PlayState::Playing));
    assert_eq!(out(&m, 4), [100, -100, 200, -200, 300, -300, 100, -100]);
    assert_eq!(m.queue_state(s), Ok((1, 1)));
    assert_eq!(m.queued_frames(s), Ok(1));
    assert_eq!(m.events().try_pop(), Some(MixerEvent::BufferDone(s)));
    assert_eq!(m.events().try_pop(), None);
    // Run dry in the middle of a fill: one frame of data, then silence and an underrun.
    assert_eq!(out(&m, 3), [200, -200, 0, 0, 0, 0]);
    assert_eq!(m.events().try_pop(), Some(MixerEvent::BufferDone(s)));
    assert_eq!(m.events().try_pop(), Some(MixerEvent::QueueEmpty(s)));
    assert_eq!(m.stats(), Stats { frames_out: 2 + 4 + 3, underruns: 1 });
    assert_eq!(m.queue_state(s), Ok((0, 2)));
    // A fully dry fill is an underrun too.
    assert_eq!(out(&m, 1), [0, 0]);
    assert_eq!(m.stats().underruns, 2);
    // Ending exactly on the last frame is not an underrun.
    m.enqueue(s, ramp(1)).unwrap();
    assert_eq!(out(&m, 1), [100, -100]);
    assert_eq!(m.stats().underruns, 2);
}

#[test]
fn a_held_mixer_is_silent_and_keeps_every_stream_in_place() {
    let m = Mixer::new(44100);
    assert!(!m.held());
    m.set_held(true);
    assert!(m.held());
    let s = m.create_stream(44100, 4);
    m.enqueue(s, ramp(2)).unwrap();
    m.set_state(s, PlayState::Playing).unwrap();
    // Held (the game has not shown a frame yet): silence over whatever was in the buffer, nothing played.
    let mut buf = [7i16; 4];
    m.fill(&mut buf);
    assert_eq!(buf, [0; 4]);
    assert_eq!(m.queued_frames(s), Ok(2));
    assert_eq!(m.played_frames(s), Ok(0));
    assert_eq!(m.events().try_pop(), None);
    assert_eq!(m.stats(), Stats::default());
    // Released: the song starts from its first frame.
    m.set_held(false);
    assert_eq!(out(&m, 2), [100, -100, 200, -200]);
    assert_eq!(m.played_frames(s), Ok(2));
}

#[test]
fn mixes_streams_with_gain_master_and_clipping() {
    let m = Mixer::new(8000);
    let (a, b) = (playing(&m, 8000, 2), playing(&m, 8000, 2));
    m.enqueue(a, vec![[20000, -20000], [1000, 1000]]).unwrap();
    m.enqueue(b, vec![[20000, -20000], [1000, 1000]]).unwrap();
    assert_eq!(out(&m, 1), [32767, -32768]);
    m.set_gain(a, 0.5).unwrap();
    m.set_master(0.5);
    assert_eq!(out(&m, 1), [750, 750]);
    assert_eq!(m.stream_count(), 2);
}

#[test]
fn resamples_linearly() {
    let m = Mixer::new(44100);
    let s = playing(&m, 22050, 4);
    m.enqueue(s, vec![[0, 0], [1000, -1000]]).unwrap();
    assert_eq!(out(&m, 4), [0, 0, 500, -500, 1000, -1000, 1000, -1000]);
    // Interpolation looks into the next buffer.
    m.enqueue(s, vec![[0, 0]]).unwrap();
    m.enqueue(s, vec![[1000, 0]]).unwrap();
    assert_eq!(out(&m, 3), [0, 0, 500, 0, 1000, 0]);
    // Downsampling skips frames, popping buffers inside one output frame.
    let d = playing(&m, 88200, 4);
    m.set_state(s, PlayState::Paused).unwrap();
    m.enqueue(d, vec![[1, 1], [2, 2], [3, 3]]).unwrap();
    m.enqueue(d, vec![[4, 4], [5, 5]]).unwrap();
    assert_eq!(out(&m, 3), [1, 1, 3, 3, 5, 5]);
    assert_eq!(m.queue_state(d), Ok((0, 2)));
}

#[test]
fn stop_rewinds_pause_keeps_clear_resets() {
    let m = Mixer::new(1000);
    let s = playing(&m, 1000, 4);
    m.enqueue(s, ramp(4)).unwrap();
    assert_eq!(out(&m, 2), [100, -100, 200, -200]);
    assert_eq!(m.played_frames(s), Ok(2));
    assert_eq!(m.position_ms(s), Ok(2));
    m.set_state(s, PlayState::Paused).unwrap();
    assert_eq!(out(&m, 2), [0; 4]);
    assert_eq!(m.played_frames(s), Ok(2));
    m.set_state(s, PlayState::Stopped).unwrap();
    assert_eq!((m.played_frames(s), m.queued_frames(s)), (Ok(0), Ok(4)));
    m.set_state(s, PlayState::Playing).unwrap();
    assert_eq!(out(&m, 1), [100, -100]);
    m.clear(s).unwrap();
    assert_eq!((m.queue_state(s), m.queued_frames(s)), (Ok((0, 0)), Ok(0)));
    assert!(m.destroy_stream(s));
    assert!(!m.destroy_stream(s));
}

#[test]
fn errors() {
    let m = Mixer::new(1000);
    let s = m.create_stream(1000, 1);
    assert_eq!(m.enqueue(s, vec![]), Err(MixError::Empty));
    m.enqueue(s, ramp(1)).unwrap();
    assert_eq!(m.enqueue(s, ramp(1)), Err(MixError::QueueFull));
    let gone = 99;
    assert_eq!(m.enqueue(gone, ramp(1)), Err(MixError::NoSuchStream));
    assert_eq!(m.clear(gone), Err(MixError::NoSuchStream));
    assert_eq!(m.state(gone), Err(MixError::NoSuchStream));
    assert_eq!(m.set_gain(gone, 1.0), Err(MixError::NoSuchStream));
    for e in [MixError::Empty, MixError::QueueFull, MixError::NoSuchStream] {
        assert!(!e.to_string().is_empty());
    }
}

#[test]
fn event_queue_blocks_with_timeout() {
    let q = EventQueue::default();
    assert_eq!(q.wait(Duration::from_millis(1)), None);
    q.push(MixerEvent::Wake);
    assert_eq!(q.wait(Duration::from_secs(5)), Some(MixerEvent::Wake));
}

#[test]
fn wait_for_space_is_released_by_the_device() {
    let m = Mixer::new(1000);
    let s = playing(&m, 1000, 4);
    assert!(m.wait_for_space(s, 0, Duration::ZERO));
    m.enqueue(s, ramp(2)).unwrap();
    assert!(!m.wait_for_space(s, 0, Duration::from_millis(5)));
    assert!(m.wait_for_space(s, 2, Duration::ZERO));
    let m2 = m.clone();
    let t = std::thread::spawn(move || m2.wait_for_space(s, 0, Duration::from_secs(10)));
    // Keep playing until the waiter saw the queue drain.
    while !t.is_finished() {
        m.fill(&mut [0i16; 2]);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(t.join().unwrap());
    // A stream that vanishes counts as having space.
    m.enqueue(s, ramp(2)).unwrap();
    let m3 = m.clone();
    let t = std::thread::spawn(move || m3.wait_for_space(s, 0, Duration::from_secs(10)));
    std::thread::sleep(Duration::from_millis(20));
    m.destroy_stream(s);
    assert!(t.join().unwrap());
}

fn track(m: &Mixer, rate: i32, cfg: i32, enc: i32, bytes: i32) -> AudioTrack {
    AudioTrack::new(m, rate, cfg, enc, bytes).unwrap()
}

#[test]
fn track_validates_its_format() {
    let m = Mixer::new(44100);
    for (r, c, e) in [(100, 12, 2), (44100, 99, 2), (44100, 12, 99), (-1, 12, 2), (400_000, 12, 2)] {
        assert_eq!(min_buffer_size(r, c, e), None);
        assert_eq!(AudioTrack::new(&m, r, c, e, 100).err(), Some(TrackError::BadValue));
    }
    assert_eq!(min_buffer_size(44100, 12, ENCODING_PCM_16BIT), Some(1764 * 4));
    assert_eq!(min_buffer_size(44100, 4, ENCODING_PCM_8BIT), Some(1764));
    let t = track(&m, 44100, 3, ENCODING_PCM_FLOAT, 0);
    assert_eq!(t.format(), PcmFormat { rate_hz: 44100, channels: 2, sample: SampleFormat::F32 });
    assert_eq!(track(&m, 8000, 2, ENCODING_PCM_8BIT, 64).format().channels, 1);
}

#[test]
fn track_streams_pcm_through_the_mixer() {
    let m = Mixer::new(44100);
    let t = track(&m, 44100, 12, ENCODING_PCM_16BIT, 64);
    let pcm: Vec<u8> = [100i16, -100, 200, -200].iter().flat_map(|s| s.to_le_bytes()).collect();
    assert_eq!(t.write(&pcm), Ok(8));
    assert_eq!(t.write(&[]), Ok(0));
    assert_eq!(t.write(&[1]), Ok(1), "a partial frame is swallowed");
    assert_eq!(out(&m, 1), [0, 0], "not playing yet");
    t.play().unwrap();
    assert_eq!(out(&m, 2), [100, -100, 200, -200]);
    assert_eq!(t.playback_head_position(), Ok(2));
    t.pause().unwrap();
    t.set_volume(0.5).unwrap();
    t.write(&pcm).unwrap();
    t.play().unwrap();
    assert_eq!(out(&m, 1), [50, -50]);
    t.stop().unwrap();
    assert_eq!(t.playback_head_position(), Ok(0));
    assert_eq!(m.queued_frames(t.stream_id()), Ok(0));
    t.flush().unwrap();
}

#[test]
fn track_write_blocks_until_the_device_catches_up() {
    let m = Mixer::new(8000);
    let mut t = track(&m, 8000, 4, ENCODING_PCM_8BIT, 4); // 4 frames of room
    t.set_write_timeout(Duration::from_millis(10));
    assert_eq!(t.write(&[128; 4]), Ok(4));
    assert_eq!(t.write(&[128; 4]), Ok(0), "no room and the device is not draining: stall");
    t.play().unwrap();
    m.fill(&mut [0i16; 8]);
    assert_eq!(t.write(&[128; 4]), Ok(4));
    // Too many tiny writes in a row: reported as a stall, not an error.
    let t2 = track(&m, 8000, 4, ENCODING_PCM_8BIT, 100_000);
    let results: Vec<_> = (0..257).map(|_| t2.write(&[128])).collect();
    assert_eq!(results[255], Ok(1));
    assert_eq!(results[256], Ok(0));
}

#[test]
fn released_tracks_refuse_everything() {
    let m = Mixer::new(1000);
    let mut t = track(&m, 8000, 4, ENCODING_PCM_8BIT, 8);
    assert_eq!(m.stream_count(), 1);
    t.release();
    t.release();
    assert_eq!(m.stream_count(), 0);
    assert_eq!(t.play(), Err(TrackError::Released));
    assert_eq!(t.flush(), Err(TrackError::Released));
    assert_eq!(t.write(&[0]), Err(TrackError::Released));
    // A track whose stream was pulled out from under it behaves the same.
    let t = track(&m, 8000, 4, ENCODING_PCM_8BIT, 8);
    m.destroy_stream(t.stream_id());
    assert_eq!(t.pause(), Err(TrackError::Released));
    assert_eq!(t.set_volume(1.0), Err(TrackError::Released));
    assert_eq!(t.playback_head_position(), Err(TrackError::Released));
    assert_eq!(t.write(&[0]), Ok(0));
    drop(t);
    assert_eq!(m.stream_count(), 0);
}

#[test]
fn interface_ids_are_laid_out_like_the_c_struct() {
    use crate::sles::consts::{iid_of, uuid, Iid, IIDS};
    // time_low, time_mid, time_hi_and_version, clock_seq (all little endian), then the node bytes.
    let u = uuid(0x0102_0304, 0x0506, 0x0708, 0x090a, [11, 12, 13, 14, 15, 16]);
    assert_eq!(u, [4, 3, 2, 1, 6, 5, 8, 7, 10, 9, 11, 12, 13, 14, 15, 16]);
    assert_eq!(iid_of(&IIDS[3].2), Some(Iid::Play));
    assert_eq!(iid_of(&u), None);
}

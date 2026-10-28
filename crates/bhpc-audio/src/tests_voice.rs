//! Voice playback (per-speaker streams, pan, gain) and the microphone seam with a fake capture device.

use crate::capture::*;
use crate::mixer::*;
use crate::voice::*;
use std::any::Any;
use std::sync::{Arc, Mutex};

#[test]
fn constant_power_pan() {
    let (l, r) = pan_gains(0.0);
    assert!((l - r).abs() < 1e-6 && (l * l + r * r - 1.0).abs() < 1e-5, "centre: equal, unit power");
    assert_eq!(pan_gains(-1.0).0, 1.0);
    assert!(pan_gains(-1.0).1.abs() < 1e-6, "hard left");
    assert!(pan_gains(1.0).0.abs() < 1e-6 && (pan_gains(1.0).1 - 1.0).abs() < 1e-6, "hard right");
    assert_eq!(pan_gains(5.0), pan_gains(1.0), "clamped");
    assert_eq!(pan_gains(f32::NAN), pan_gains(0.0));
    for p in [-0.7f32, -0.2, 0.3, 0.9] {
        let (l, r) = pan_gains(p);
        assert!((l * l + r * r - 1.0).abs() < 1e-5, "constant power at {p}");
    }
}

#[test]
fn spatialise_applies_gain_pan_and_saturates() {
    assert_eq!(spatialise(&[1000, -1000], 1.0, -1.0), vec![[1000, 0], [-1000, 0]]);
    assert_eq!(spatialise(&[1000], 0.0, 0.0), vec![[0, 0]]);
    assert_eq!(spatialise(&[1000], f32::NAN, 1.0), vec![[0, 0]]);
    assert_eq!(spatialise(&[30000, -30000], 100.0, -1.0), vec![[32767, 0], [-32768, 0]], "gain clamped, saturating");
    let [l, r] = spatialise(&[10000], 2.0, 0.0)[0];
    assert_eq!(l, r);
    assert!((14140..=14143).contains(&l), "{l}");
}

#[test]
fn per_speaker_streams_mix_and_go_away() {
    let mixer = Mixer::new(VOICE_RATE);
    let out = VoiceOut::new(mixer.clone());
    let a = out.voice_stream(7);
    assert_eq!(out.voice_stream(7), a, "one stream per speaker");
    let b = out.voice_stream(3);
    assert_eq!(a.speaker, 7);
    assert_eq!(out.speakers(), vec![3, 7]);
    assert_eq!(mixer.stream_count(), 2);
    out.push(a, &[1000; 4], 1.0, -1.0).unwrap();
    out.push(b, &[500; 4], 2.0, 1.0).unwrap();
    assert_eq!(out.queued(a), Ok(4));
    let mut buf = vec![0i16; 8];
    mixer.fill(&mut buf);
    assert!(buf.chunks(2).all(|f| f == [1000, 1000]), "{buf:?}");
    assert!(mixer.events().try_pop().is_none(), "voice streams are quiet");
    assert_eq!(out.push(a, &[], 1.0, 0.0), Err(MixError::Empty));
    for _ in 0..MAX_BUFFERS {
        out.push(a, &[1; 2], 1.0, 0.0).unwrap();
    }
    assert_eq!(out.push(a, &[1; 2], 1.0, 0.0), Err(MixError::QueueFull), "bounded");
    assert!(out.drop_speaker(7));
    assert!(!out.drop_speaker(7));
    assert_eq!(out.push(a, &[1; 2], 1.0, 0.0), Err(MixError::NoSuchStream));
    assert_eq!(out.queued(a), Err(MixError::NoSuchStream));
    out.voice_stream(9);
    out.clear();
    assert!(out.speakers().is_empty());
    assert_eq!(mixer.stream_count(), 0);
    out.voice_stream(1);
    drop(out);
    assert_eq!(mixer.stream_count(), 0, "dropping the output removes its streams");
}

#[test]
fn quiet_is_per_stream() {
    let m = Mixer::new(100);
    let loud = m.create_stream(100, 2);
    m.set_state(loud, PlayState::Playing).unwrap();
    m.enqueue(loud, vec![[1, 1]]).unwrap();
    assert_eq!(m.set_quiet(99, true), Err(MixError::NoSuchStream));
    let mut buf = vec![0i16; 2];
    m.fill(&mut buf);
    assert_eq!(m.events().try_pop(), Some(MixerEvent::BufferDone(loud)));
    assert_eq!(m.events().try_pop(), Some(MixerEvent::QueueEmpty(loud)));
}

/// A capture device that records what it was asked and keeps the sinks so the test can "speak".
#[derive(Default)]
struct Fake {
    opened: Vec<(Option<String>, u32, u16)>,
    sinks: Arc<Mutex<Vec<CaptureSink>>>,
    fail: bool,
    closed: Arc<Mutex<u32>>,
}

struct Dev(Arc<Mutex<u32>>);
impl Drop for Dev {
    fn drop(&mut self) {
        *self.0.lock().unwrap() += 1;
    }
}

impl CaptureBackend for Fake {
    fn devices(&mut self) -> Vec<String> {
        vec!["Built-in".into(), "Headset".into()]
    }
    fn open(
        &mut self,
        device: Option<&str>,
        rate: u32,
        samples: u16,
        sink: CaptureSink,
    ) -> Result<Box<dyn Any>, String> {
        if self.fail {
            return Err("no mic".into());
        }
        self.opened.push((device.map(str::to_string), rate, samples));
        self.sinks.lock().unwrap().push(sink);
        Ok(Box::new(Dev(self.closed.clone())))
    }
}

fn speak(f: &Fake, samples: &[i16]) {
    (f.sinks.lock().unwrap().last_mut().unwrap())(samples);
}

#[test]
fn mic_opens_only_on_demand_and_chunks_20ms() {
    let ctl = Arc::new(MicControl::default());
    let mut drv = MicDriver::new(ctl.clone());
    let mut dev = Fake::default();
    drv.sync(&mut dev);
    assert!(!drv.is_open() && !ctl.live() && !ctl.wanted(), "off by default");
    assert!(dev.opened.is_empty());
    ctl.request_devices();
    drv.sync(&mut dev);
    assert_eq!(ctl.devices(), vec!["Built-in", "Headset"]);

    ctl.set_wanted(true);
    drv.sync(&mut dev);
    drv.sync(&mut dev);
    assert!(drv.is_open() && ctl.live());
    assert_eq!(dev.opened, vec![(None, 48_000, 960)], "48 kHz, 20 ms, opened once");
    speak(&dev, &[1; 500]);
    assert!(ctl.pop().is_none(), "not a whole chunk yet");
    speak(&dev, &[2; 1500]);
    let c = ctl.pop().unwrap();
    assert_eq!(c.len(), CHUNK_SAMPLES);
    assert_eq!((c[0], c[499], c[500]), (1, 1, 2));
    assert_eq!(ctl.pop().unwrap().len(), CHUNK_SAMPLES);
    assert!(ctl.pop().is_none(), "80 samples wait for the next buffer");

    // Another device: the old one closes, the new one opens.
    ctl.set_device(Some("Headset".into()));
    ctl.set_device(Some("Headset".into()));
    assert_eq!(ctl.device().as_deref(), Some("Headset"));
    drv.sync(&mut dev);
    assert_eq!(*dev.closed.lock().unwrap(), 1);
    assert_eq!(dev.opened.last().unwrap().0.as_deref(), Some("Headset"));

    // A full queue drops new chunks instead of blocking the device thread.
    speak(&dev, &vec![3; CHUNK_SAMPLES * (QUEUE_CHUNKS + 2)]);
    assert_eq!(ctl.dropped(), 2);

    // Turned off: closed, the indicator goes out and nothing old is left to send.
    ctl.set_wanted(false);
    drv.sync(&mut dev);
    assert!(!drv.is_open() && !ctl.live());
    assert_eq!(*dev.closed.lock().unwrap(), 2);
    assert!(ctl.pop().is_none());
    drv.sync(&mut dev);
}

#[test]
fn a_failed_open_is_reported_and_not_retried_every_frame() {
    let ctl = Arc::new(MicControl::default());
    let mut drv = MicDriver::new(ctl.clone());
    let mut dev = Fake { fail: true, ..Fake::default() };
    ctl.set_wanted(true);
    drv.sync(&mut dev);
    assert_eq!(ctl.last_error().as_deref(), Some("no mic"));
    assert!(!ctl.live());
    dev.fail = false;
    drv.sync(&mut dev);
    assert!(!drv.is_open(), "same wish: not retried");
    ctl.set_wanted(false);
    drv.sync(&mut dev);
    ctl.set_wanted(true);
    drv.sync(&mut dev);
    assert!(drv.is_open() && ctl.live(), "off and on again retries");
    assert_eq!(ctl.last_error(), None);
}

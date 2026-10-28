use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::capture::{CaptureBackend, MicControl, MicDriver, CHUNK_SAMPLES};
use crate::testmic::{parse_wav, Signal, TestMic, AMPLITUDE};

fn wav(format: u16, channels: u16, rate: u32, bits: u16, data: &[i16], extra: &[u8]) -> Vec<u8> {
    let mut b = b"RIFF\0\0\0\0WAVE".to_vec();
    // An odd-sized chunk before the format: skipped with its pad byte.
    b.extend_from_slice(b"LIST\x03\0\0\0abc\0");
    b.extend_from_slice(extra);
    b.extend_from_slice(b"fmt \x10\0\0\0");
    b.extend_from_slice(&format.to_le_bytes());
    b.extend_from_slice(&channels.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
    b.extend_from_slice(&(channels * 2).to_le_bytes());
    b.extend_from_slice(&bits.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&((data.len() * 2) as u32).to_le_bytes());
    for s in data {
        b.extend_from_slice(&s.to_le_bytes());
    }
    b
}

#[test]
fn specs_parse() {
    assert_eq!(Signal::parse("sine").unwrap(), Signal::Sine { hz: 440.0 });
    assert_eq!(Signal::parse("sine:1000").unwrap(), Signal::Sine { hz: 1000.0 });
    assert_eq!(Signal::parse("pulse").unwrap(), Signal::Pulse { hz: 440.0, on_ms: 200, period_ms: 2000 });
    assert_eq!(Signal::parse("pulse:300::1000").unwrap(), Signal::Pulse { hz: 300.0, on_ms: 200, period_ms: 1000 });
    assert!(Signal::parse("sine:x").unwrap_err().contains("bad frequency"));
    assert!(Signal::parse("pulse:440:x").unwrap_err().contains("bad pulse length"));
    assert!(Signal::parse("pulse:440:500:100").unwrap_err().contains("does not fit"));
    assert!(Signal::parse("pulse:440:0:0").unwrap_err().contains("does not fit"));
    assert!(Signal::parse("/nonexistent/x.wav").unwrap_err().contains("/nonexistent/x.wav"));
    let dir = std::env::temp_dir().join(format!("bhpc-testmic-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("a.wav");
    std::fs::write(&f, wav(1, 1, 48_000, 16, &[1, 2, 3], &[])).unwrap();
    assert_eq!(Signal::parse(f.to_str().unwrap()).unwrap(), Signal::Samples(vec![1, 2, 3]));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn wav_files() {
    assert_eq!(parse_wav(&wav(1, 2, 48_000, 16, &[10, 20, -4, -6], &[])).unwrap(), vec![15, -5], "stereo mixed down");
    for bad in [wav(3, 1, 48_000, 16, &[1], &[]), wav(1, 1, 44_100, 16, &[1], &[]), wav(1, 3, 48_000, 16, &[1], &[])] {
        assert!(parse_wav(&bad).unwrap_err().contains("must be 16-bit PCM"));
    }
    assert!(parse_wav(&wav(1, 1, 48_000, 8, &[1], &[])).is_err());
    assert!(parse_wav(b"RIFF").unwrap_err().contains("not a WAV"));
    assert!(parse_wav(b"RIFF\0\0\0\0WAVX").unwrap_err().contains("not a WAV"));
    assert!(parse_wav(b"RIFF\0\0\0\0WAVEdata\x02\0\0\0\0\0").unwrap_err().contains("before its format"));
    assert!(parse_wav(b"RIFF\0\0\0\0WAVEdata\x09\0\0\0\0\0").unwrap_err().contains("cut short"));
    assert!(parse_wav(b"RIFF\0\0\0\0WAVEfmt \x02\0\0\0\0\0").unwrap_err().contains("without data"));
}

#[test]
fn signals() {
    let rms = |s: &Signal, from: u64, n: u64| {
        let sum: f64 = (from..from + n).map(|i| f64::from(s.sample(i)).powi(2)).sum();
        (sum / n as f64).sqrt() / 32767.0
    };
    let sine = Signal::Sine { hz: 440.0 };
    let r = rms(&sine, 0, 48_000);
    assert!((r - f64::from(AMPLITUDE) / 2f64.sqrt()).abs() < 0.01, "{r}");
    let pulse = Signal::Pulse { hz: 440.0, on_ms: 200, period_ms: 1000 };
    assert!(rms(&pulse, 0, 9600) > 0.2, "on for the first 200 ms");
    assert_eq!(rms(&pulse, 9600, 38_400), 0.0, "then silent");
    assert!(rms(&pulse, 48_000, 960) > 0.1, "on again after a period");
    let samples = Signal::Samples(vec![5, 6]);
    let mut pos = 1;
    let mut out = [0; 3];
    samples.fill(&mut pos, &mut out);
    assert_eq!((out, pos), ([6, 5, 6], 4), "looped");
    assert_eq!(Signal::Samples(Vec::new()).sample(7), 0);
}

#[test]
fn the_test_mic_records_in_real_time_until_closed() {
    let mut mic = TestMic::new(Signal::Sine { hz: 440.0 });
    assert_eq!(mic.devices(), vec!["test"]);
    let got = Arc::new(Mutex::new(Vec::<usize>::new()));
    let g = got.clone();
    let dev = mic.open(None, 48_000, 960, Box::new(move |s| g.lock().unwrap().push(s.len()))).unwrap();
    std::thread::sleep(Duration::from_millis(110));
    drop(dev);
    let n = got.lock().unwrap().len();
    assert!((3..=8).contains(&n), "about one chunk per 20 ms, got {n}");
    assert!(got.lock().unwrap().iter().all(|l| *l == CHUNK_SAMPLES));
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(got.lock().unwrap().len(), n, "stopped when dropped");

    // Through the driver: the chunks reach the voice code.
    let ctl = Arc::new(MicControl::default());
    let mut drv = MicDriver::new(ctl.clone());
    ctl.set_wanted(true);
    drv.sync(&mut mic);
    assert!(ctl.live());
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(ctl.pop().unwrap().len(), CHUNK_SAMPLES);
    ctl.set_wanted(false);
    drv.sync(&mut mic);
}

#[test]
fn from_env() {
    // The only test touching this variable.
    std::env::remove_var("BHPC_VOICE_TEST_MIC");
    assert!(TestMic::from_env().is_none());
    std::env::set_var("BHPC_VOICE_TEST_MIC", "sine:x");
    assert!(TestMic::from_env().unwrap().is_err());
    std::env::set_var("BHPC_VOICE_TEST_MIC", "pulse");
    assert!(TestMic::from_env().unwrap().is_ok());
    std::env::remove_var("BHPC_VOICE_TEST_MIC");
}

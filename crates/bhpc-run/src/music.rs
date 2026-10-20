//! The game's music: `com.apportable.media.AudioPlayer` (behind `AVAudioPlayer`) over [`bhpc_audio::MediaPlayer`].
//!
//! The game hands `initAudio` an asset path (`GameResources/<track>.mp4`, read from the APK by the fake JVM); any
//! other path is read through the guest file system. Each player gets its own pump thread (a host thread, not a guest one) that keeps the mixer fed.

use bhpc_audio::{MediaPlayer, Mixer, PumpGuard};
use bhpc_jni::MusicOp;
use bhpc_vfs::{FileSystem, OpenFlags};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// How often a player's pump tops up the mixer (it keeps about a second decoded ahead).
const PUMP: Duration = Duration::from_millis(20);

struct Player {
    player: MediaPlayer,
    _pump: PumpGuard,
}

#[derive(Default)]
struct State {
    next: u32,
    players: HashMap<u32, Player>,
    /// Players paused by `lifecycleSuspend`, started again by `lifecycleResume`.
    suspended: Vec<u32>,
}

/// Live music players by id.
pub struct Music {
    mixer: Mixer,
    fs: Arc<dyn FileSystem>,
    state: Mutex<State>,
}

/// The whole file at a guest path (`file://` URLs too).
fn read_all(fs: &dyn FileSystem, path: &str) -> Option<Vec<u8>> {
    let path = path.strip_prefix("file://").unwrap_or(path);
    let mut f = fs.open(path, OpenFlags::read_only()).ok()?;
    let (mut out, mut buf) = (Vec::new(), vec![0; 1 << 16]);
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
    }
    (!out.is_empty()).then_some(out)
}

impl Music {
    pub fn new(mixer: Mixer, fs: Arc<dyn FileSystem>) -> Music {
        Music { mixer, fs, state: Mutex::default() }
    }

    fn st(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `initAudio(path)`: a player in the `Initialized` state, or `None` if the file is missing or empty. `asset` is
    /// the file already read from the APK's assets; without it the guest path is read.
    pub fn open(&self, path: &str, asset: Option<Vec<u8>>) -> Option<u32> {
        let bytes = asset.filter(|b| !b.is_empty()).or_else(|| read_all(&*self.fs, path))?;
        let player = MediaPlayer::new(self.mixer.clone());
        player.set_data_source_bytes(bytes).ok()?;
        let pump = player.spawn_pump(PUMP);
        let mut st = self.st();
        st.next += 1;
        let id = st.next;
        st.players.insert(id, Player { player, _pump: pump });
        Some(id)
    }

    /// One `AudioPlayer` call on player `id`; negative for an unknown player or a call the player refused.
    pub fn op(&self, id: u32, op: MusicOp) -> i32 {
        let found = {
            let mut st = self.st();
            if op == MusicOp::Release {
                st.players.remove(&id).map(|p| p.player.clone())
            } else {
                st.players.get(&id).map(|p| p.player.clone())
            }
        };
        let Some(p) = found else {
            return -1;
        };
        let done = |r: Result<(), bhpc_audio::MusicError>| if r.is_ok() { 0 } else { -1 };
        match op {
            MusicOp::Prepare => done(p.prepare()),
            MusicOp::Start => done(p.start()),
            MusicOp::Pause => done(p.pause()),
            MusicOp::SeekTo(ms) => done(p.seek_to(ms.max(0) as u32)),
            // As Apportable's Java side: only -1 loops (iOS would loop for any negative count).
            MusicOp::Loops(n) => {
                p.set_looping(n == -1);
                0
            }
            MusicOp::Volume(l, r) => {
                p.set_volume(l, r);
                0
            }
            MusicOp::Duration => p.duration_ms(),
            MusicOp::Position => p.position_ms(),
            MusicOp::IsPlaying => i32::from(p.is_playing()),
            MusicOp::Release => {
                p.release();
                0
            }
        }
    }

    /// `lifecycleSuspend` (`true`) pauses what plays; `lifecycleResume` (`false`) starts those again.
    pub fn suspend(&self, suspend: bool) {
        let mut st = self.st();
        if suspend {
            let playing: Vec<u32> =
                st.players.iter().filter(|(_, p)| p.player.is_playing()).map(|(&id, _)| id).collect();
            for id in &playing {
                let _ = st.players[id].player.pause();
            }
            st.suspended.extend(playing);
        } else {
            for id in std::mem::take(&mut st.suspended) {
                if let Some(p) = st.players.get(&id) {
                    let _ = p.player.start();
                }
            }
        }
    }
}

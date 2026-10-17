# bhpc-audio

Audio for the PC port (PLAN 1.6). Nothing in this crate links an audio library: a device-independent
`Mixer` produces stereo `i16` on demand, and three front ends feed it.

| Module | What it is |
|---|---|
| `mixer` | `Mixer::fill(&mut [i16])`: sums any number of queued-PCM streams (play/pause/stop, gain, linear resampling, buffer-done events, underrun counter). |
| `sles` | OpenSL ES HLE for the guest: `slCreateEngine`, the `SL_IID_*` variables and the Object, Engine, Play, BufferQueue (== AndroidSimpleBufferQueue) and Volume vtables, built in guest memory out of thunks. |
| `track` | `AudioTrack` (MODE_STREAM) model for the fake JVM; blocking `write` paces libOpenAL's mixing thread. |
| `pcm` | 8-bit unsigned, 16-bit signed and 32-bit float, mono/stereo, to stereo `i16`. |

## Why OpenSL ES first

PLAN 1.6 proposed making `dlopen("libOpenSLES.so")` fail so libOpenAL falls back to its AudioTrack backend (needs JNI).
This crate implements both, so the choice is one line in `bhpc-run`. Recommended default: **serve OpenSL ES**. It needs
no Java objects, and the buffer-queue callbacks arrive on a thread of our own, which is how the backend was written.
`libAudioUnit.so` (the other `dlopen` of `/system/lib/libOpenSLES.so`) asks for `slCreateEngine` and
`SL_IID_{ENGINE,PLAY,BUFFERQUEUE,ANDROIDSIMPLEBUFFERQUEUE,RECORD}`; all five resolve. No game library embeds its own
copy of an interface ID (checked in 1.7.4), so the guest only ever uses the pointers we hand out.

## Wiring needed in bhpc-run / bhpc-bionic / bhpc-app

1. **Mixer and device** (`bhpc-app`). Create one `Mixer::new(44100)` and hand a clone to the SDL audio callback:
   ```rust
   struct Out(Mixer);
   impl sdl2::audio::AudioCallback for Out {
       type Channel = i16;
       fn callback(&mut self, out: &mut [i16]) { self.0.fill(out) }
   }
   // AudioSpecDesired { freq: Some(44100), channels: Some(2), samples: Some(1024) }
   ```
   (`fill` overwrites the whole slice; give it an even length.) Streams of other rates are resampled.
2. **Install** (`bhpc-run`, after the CPU exists and before guest code runs):
   `let sles = Sles::install(&cpu, mixer.clone())?;` Every thunk is registered on the shared thunk table, so all
   guest threads can call it. It maps 64 KiB of guest memory for vtables, interface IDs and handles.
3. **dl\*** (`bhpc-bionic`/loader glue). `dlopen("libOpenSLES.so" | "/system/lib/libOpenSLES.so")` returns a sentinel
   handle instead of NULL; `dlsym(handle, name)` returns `sles.lookup(name).map(|(addr, _)| addr)`. `slCreateEngine`
   is a function thunk, `SL_IID_*` are addresses of data words (what `dlsym` of a C variable returns). If a direct
   import of `slCreateEngine` ever appears, `bhpc_bionic::resolve` already finds the thunk through
   `cpu.thunk_addr("slCreateEngine")`.
4. **Callbacks.** After guest init, start the pump: `let pump = sles.spawn_pump(&cpu, stack_top)?;` with a mapped stack
   (256 KiB is plenty) and keep the handle. `pump.stop()` joins it and returns the guest failure that ended it, and
   `pump.is_finished()` can be polled for that. For a single-threaded harness use `sles.pump_once(&mut cpu, wait)`.
5. **AudioTrack fallback** (`bhpc-jni`, only if dlopen returns NULL). `AudioTrack::new(&mixer, rate, channelConfig,
   encoding, bufferBytes)` is the constructor, `min_buffer_size` is the static `getMinBufferSize`, and the methods
   `play/pause/stop/flush/release/set_volume/playback_head_position/write(&[u8])` map one-to-one to the Java ones.
   Convert `byte[]` to `&[u8]` before `write`. `stop` discards queued data instead of playing it out.
6. **Music.** The 9 mp4 tracks play through `android.media.MediaPlayer` (PLAN 1.6 / B-jni), not OpenAL. Not part of this
   crate; `Mixer::create_stream` + `enqueue` is the intended sink for a decoded stream.

## Semantics worth knowing

* `Enqueue` copies the buffer, so the guest may reuse it at once. Sizes must be whole frames (`PARAMETER_INVALID`),
  and a full queue is `BUFFER_INSUFFICIENT`.
* A buffer-queue callback fires after the buffer finished and the state (count, play index) was updated.
  `Clear` resets the play index. `Stop` rewinds the current buffer but keeps the queue (as Android does).
* `HEADATEND` (play callback, event mask bit 0) fires when the last queued buffer finishes.
* Asynchronous `Realize` completes through the same pump.
* Unsupported (return `SL_RESULT_FEATURE_UNSUPPORTED`): recorders, MIDI, LED/vibra, listeners/3D, metadata, effects,
  markers and position-update periods, stereo position, `slQuery*EngineInterfaces`. Optional interfaces that are
  unavailable are skipped; required ones fail the `Create*` call.
* `GetInterface` is allowed before `Realize` (more lenient than Android).
* Objects are never suspended, so `Resume` is always `PRECONDITIONS_VIOLATED`.

## Verification

`COVERAGE_PACKAGES="bhpc-audio bhpc-input" scripts/coverage.sh` gates 100 % line and function coverage. The tests
drive the vtables through real guest calls (`Cpu::call` on the thunk addresses, including a 4-instruction ARM
callback that re-enqueues through the vtable on the pump thread) and need no game files.

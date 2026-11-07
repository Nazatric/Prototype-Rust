//! gameplay_audio — block 57: the GAMEPLAY audio DEVICE edge (feature
//! `audio-cpal`), the block-55 NEXT-UP (d) "same shape when it comes"
//! follow-up to the boot movie's [`crate::movie::MovieAudioEdge`].
//!
//! ## What this closes
//!
//! The gameplay slice's audio output stage
//! (`prototype_engine::audio_out::AudioOut`) has ticked once per frame
//! since block 38 — mixer → interleaved i16 → ring — but nothing ever
//! DRAINED that ring: the device edge existed only for the boot movie.
//! Block 57 puts the gameplay ring behind `Arc<Mutex<…>>`
//! (`AudioOut::ring_handle`) and opens a REAL CPAL output stream over
//! it: the device callback drains the very buffer the per-frame tick
//! fills — one buffer, one writer, one reader, zero copies (the exact
//! shape the movie player has used since block 54).
//!
//! ## Lifetime contract (DESIGN — the original's gameplay audio
//! device architecture is not recoverable from the DLL; the
//! dataflow shape is the PROVEN callback-drains-ring model,
//! `cpal_out` module docs)
//!
//! * OPEN once per boot, in `try_boot` next to the movie edge —
//!   non-fatal: a device-less host (or a refused stream) logs once
//!   and the game keeps ticking silent (never a boot failure).
//! * HOLD for the app's lifetime: NO tail-drain close, unlike the
//!   movie edge — the mixer produces EVERY frame (silence when no
//!   voice plays is real audio state, not a drain signal), so the
//!   only correct close is an explicit cut.
//! * CUT at teardown (suspend / close) with the rest of the device
//!   state; the next boot re-opens.  [`gameplay_audio_edge_should_cut`]
//!   pins the policy: a live session NEVER cuts mid-frame.
//! * Re-open is guarded (`if self.gameplay_audio.is_none()`) so a
//!   re-run `try_boot` never double-opens over the same ring.
//!
//! ## Honest unknowns
//!
//! * Sample-rate policy: the stage mixes at its fixed rate (the app
//!   uses 48 kHz stereo) and the device opens at its DEFAULT config
//!   with NO resampling (`audio::SampleRateConverter` is not
//!   reconstructed) — a 44.1 kHz host shifts pitch, exactly the
//!   movie edge's documented limitation (movie.rs honest unknowns).
//! * The Android backend (Oboe/AAudio) is compile-gated behind the
//!   NDK and has never run in this sandbox — the host Linux backend
//!   (ALSA) carries the behavioural proof here.
//! * The clip ↔ gameplay-event mapping is the block-38 UNKNOWN: the
//!   stage mixes silence until voices land; this block wires the
//!   DEVICE half of that path, not the content half.

#![forbid(unsafe_code)]

/// The CUT decision (block 57, pure so it is testable without any
/// device): the gameplay stream is cut ONLY when the session is gone
/// (teardown / suspend / close).  A live session NEVER cuts — there
/// is no "drained" state to wait for (the mixer produces every
/// frame), so the movie edge's tail-drain policy has no gameplay
/// counterpart; the cut is always explicit.
pub fn gameplay_audio_edge_should_cut(session_alive: bool) -> bool {
    !session_alive
}

/// The gameplay audio's live device stream (feature `audio-cpal`): a
/// real CPAL output opened over
/// [`prototype_audio::cpal_out::SharedRing::from_arc`] of THE ring the
/// per-frame [`AudioOut::tick`](prototype_engine::audio_out::AudioOut)
/// fills.  Dropping the edge closes the stream (audio stops).
#[cfg(feature = "audio-cpal")]
pub struct GameplayAudioEdge {
    out: prototype_audio::cpal_out::CpalOut,
    ring: prototype_audio::cpal_out::SharedRing,
}

#[cfg(feature = "audio-cpal")]
impl GameplayAudioEdge {
    /// Open the host's default output device over `ring`.  `Err` when
    /// no device exists or the stream is refused — the caller logs
    /// and the game keeps ticking silent (never a boot failure).
    pub fn open(ring: prototype_audio::cpal_out::SharedRing) -> std::result::Result<Self, String> {
        Ok(Self {
            out: prototype_audio::cpal_out::CpalOut::open_default(ring.clone())?,
            ring,
        })
    }

    /// The device config actually in effect: (sample rate Hz, channels).
    pub fn config(&self) -> (u32, u16) {
        (self.out.config.sample_rate, self.out.config.channels)
    }

    /// Samples still buffered in the ring as the device sees it.
    /// (`SharedRing::lock` recovers a poisoned mutex itself.)
    pub fn ring_buffered(&self) -> usize {
        self.ring.lock().buffered()
    }

    /// Device-callback invocations so far (a live stream ticks this).
    pub fn device_callbacks(&self) -> u64 {
        self.out.stats.callbacks()
    }

    /// Device-callback underruns so far (silence padding the device
    /// needed — the audible symptom of a starved ring).
    pub fn device_underruns(&self) -> u64 {
        self.out.stats.underruns()
    }

    /// CPAL stream errors so far (device loss, format change, ...).
    pub fn device_errors(&self) -> u64 {
        self.out.stats.errors()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The cut policy's two cells: a live session never cuts; a dead
    /// session always does.  (No third cell — unlike the movie edge
    /// there is no drained/undrained axis: the mixer produces every
    /// frame and the only close is the explicit cut.)
    #[test]
    fn the_cut_policy_never_fires_mid_session() {
        assert!(!gameplay_audio_edge_should_cut(true), "a live session never cuts");
        assert!(
            gameplay_audio_edge_should_cut(false),
            "a dead session cuts — the only close is explicit"
        );
    }

    /// THE BLOCK-57 E2E on REAL device + REAL mixed audio: an
    /// `AudioOut` plays a tone voice, the edge opens over its ring
    /// handle, and the device callback drains the mixed audio.
    /// Device-dependent by nature: where no device exists the test
    /// skips (the engine-side shared-buffer tests carry the
    /// correctness burden there).
    #[cfg(feature = "audio-cpal")]
    #[test]
    fn the_edge_drains_the_gameplay_ring_end_to_end() {
        use prototype_engine::audio_out::AudioOut;
        use prototype_math::Vec3;

        // A 0.5 s stereo tone voice (the gameplay content half stays
        // the block-38 UNKNOWN — any voice proves the DEVICE path).
        let mut audio = AudioOut::new(1 << 16, 2);
        let sr = 48_000u32;
        let frames = (sr / 2) as usize;
        let voice = prototype_audio::mixer::Voice {
            samples: (0..frames * 2)
                .map(|i| {
                    let s =
                        (2.0 * std::f32::consts::PI * 440.0 * (i as f32 / 2.0) / sr as f32).sin();
                    (s * 16384.0) as i16
                })
                .collect(),
            channels: 2,
            cursor: 0,
            loop_voice: false,
            gain: 1.0,
            position: Vec3::new(0.0, 0.0, 0.0),
            tag: None,
        };
        audio.mixer().play(voice);
        let stats = audio.tick(frames);
        assert_eq!(stats.samples_routed, frames * 2, "stereo: 2 samples per frame");
        assert_eq!(audio.buffered(), frames * 2);

        let edge = match GameplayAudioEdge::open(prototype_audio::cpal_out::SharedRing::from_arc(
            audio.ring_handle(),
        )) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("gameplay edge e2e: no device / stream refused ({e}) — skipped");
                return;
            }
        };
        let (rate, ch) = edge.config();
        eprintln!("gameplay edge e2e: device open at {rate} Hz × {ch} ch");

        // The stream drains the mixed audio in real time (~0.5 s at
        // the device rate; the sandbox's null PCM may consume faster).
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(3_000);
        while std::time::Instant::now() < deadline {
            if edge.device_callbacks() > 0 && edge.ring_buffered() == 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            edge.device_callbacks() > 0,
            "device callback never ran — the gameplay audio path is dead"
        );
        assert_eq!(edge.ring_buffered(), 0, "the mixed 0.5 s never drained");
        assert_eq!(edge.device_errors(), 0, "the stream reported errors");
        // The app keeps ticking UNDER the stream (the same ring takes
        // the next frame's mix without any reopen).
        audio.tick(64);
        assert_eq!(audio.samples_routed_total(), (frames * 2 + 128) as u64);
        eprintln!(
            "gameplay edge e2e: {} callback(s), {} underrun(s), drained clean",
            edge.device_callbacks(),
            edge.device_underruns()
        );
    }

    /// A device-less host plays SILENT, never fails the boot:
    /// `GameplayAudioEdge::open` surfaces `Err(String)` (the app
    /// logs it and continues) — compile-shape pin, mirroring the
    /// block-55 movie-edge test.
    #[cfg(feature = "audio-cpal")]
    #[test]
    fn the_edge_error_path_is_a_string_not_a_panic() {
        fn takes_err(e: String) -> String {
            e
        }
        let ring = prototype_audio::cpal_out::SharedRing::new(
            prototype_audio::sink::RingSink::new(16),
        );
        match GameplayAudioEdge::open(ring) {
            Ok(_) => {} // device present: the drain test covers it
            Err(e) => {
                // The error is a plain loggable String.
                let _logged = takes_err(e);
            }
        }
    }
}

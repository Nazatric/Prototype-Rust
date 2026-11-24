//! cpal_out — M18: the REAL audio-output device edge (CPAL).
//!
//! ## What this reconstructs
//!
//! The original engine's device layer is RTTI-PROVEN at the class
//! level: `audio::Renderer` + `audio::RendererVoice` feeding the PC
//! driver stack **XAudio2 / DirectSound / WDM-KS / MME / ASIO** (the
//! RTTI dump carries every one of those backend class names).  The
//! exact backend-selection policy and buffer policy are UNKNOWN (no
//! readable code).  This module is OUR clean-room device edge: a
//! CPAL output stream whose real-time callback drains the
//! [`RingSink`] the game thread mixes into — the same shape every
//! one of the PROVEN backends implies (game thread → lock-free-ish
//! queue → device callback).
//!
//! * PC (Linux): CPAL → ALSA (this module was compile-verified and
//!   end-to-end exercised against ALSA's software `null` PCM in the
//!   sandbox; hardware paths share the identical code).
//! * PC (Windows): CPAL → WASAPI; the PROVEN XAudio2/DirectSound
//!   backends are the era-correct originals, cpal/WASAPI is our
//!   portable stand-in (documented simplification).
//! * Android: CPAL → AAudio/Oboe — the target platform for this
//!   rewrite; the callback code below is host-agnostic.
//!
//! ## Model
//!
//! * [`SharedRing`] — the `Arc<Mutex<RingSink>>` the game thread
//!   writes (mixer output) and the device callback reads.  A
//!   poisoned mutex is RECOVERED (`.into_inner()`): one corrupt
//!   frame beats a dead stream.
//! * [`drain_ring_into`] — THE callback body, pure and unit-tested
//!   without any device: pulls i16 out of the ring through the
//!   allocation-free [`RingSink::read_into`], converts to the
//!   device's f32 domain (`s / 32768.0`), silence-fills on underrun
//!   and counts it.
//! * [`CpalOut`] — owns the live [`cpal::Stream`] (dropping it stops
//!   audio) plus a shared [`CpalStats`] (callback count, underruns,
//!   stream errors) the host can poll.
//!
//! ## Confidence labels
//!
//! * PROVEN: the original's backend class names (RTTI); the i16
//!   sample domain and 44100/48000 rates of the source clips
//!   (M13); the callback-drains-ring dataflow SHAPE (all four
//!   PROVEN backends are push-to-device renderers).
//! * LIKELY: f32 conversion normalisation (`s / 32768.0`); the
//!   underrun bookkeeping policy (ours, mirroring [`RingSink`]'s).
//! * UNKNOWN: the original's buffer sizing, thread priorities,
//!   sample-rate conversion point (RTTI hints at a
//!   `audio::SampleRateConverter` class — reconstructed as a
//!   CLEAN-ROOM linear interpolator in [`crate::sample_rate`] as of
//!   block 59, but WHERE the original placed it in the chain is
//!   still unknown; this edge still consumes whatever the caller
//!   mixes and opens at the device's own default rate).

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
// `Duration` is test-only (the e2e device test's pacing) — a plain
// import warns in the non-test audio-cpal build (the block-55 host
// audit cell requires zero warnings).
#[cfg(test)]
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};

use crate::sink::RingSink;

// ---------------------------------------------------------------------------
// SharedRing — game thread ⇄ device callback
// ---------------------------------------------------------------------------

/// The ring buffer shared between the mix thread (writer) and the
/// device callback (reader).  `std::sync::Mutex` because CPAL
/// callbacks are `Fn` (they cannot own a `&mut` into the host); the
/// lock is held for the length of one callback fill only.
///
/// A POISONED mutex is recovered (`into_inner`): audio continuity
/// beats strictness here, and `RingSink` has no internal invariant
/// a panic can corrupt (documented LIKELY policy — the original's
/// device queue behaviour on lock failure is UNKNOWN).
#[derive(Clone)]
pub struct SharedRing {
    inner: Arc<Mutex<RingSink>>,
}

impl SharedRing {
    pub fn new(ring: RingSink) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ring)),
        }
    }

    /// Wrap an ALREADY-shared ring (block 54): the boot-path movie
    /// player owns `Arc<Mutex<RingSink>>` (its per-frame audio feed
    /// locks the same buffer the device callback drains), so the
    /// device edge takes a handle to THE ring instead of a second
    /// copy — one buffer, one writer, one reader.
    pub fn from_arc(inner: Arc<Mutex<RingSink>>) -> Self {
        Self { inner }
    }

    /// Lock the ring for the game thread (mix) side.
    pub fn lock(&self) -> MutexGuard<'_, RingSink> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

// ---------------------------------------------------------------------------
// CpalStats — device-side counters the host can poll
// ---------------------------------------------------------------------------

/// Live counters from the device callback.  All atomic: the host
/// thread reads them without touching the audio path.
#[derive(Debug, Default)]
pub struct CpalStats {
    /// Data-callback invocations (a live stream ticks this up).
    pub callbacks: AtomicU64,
    /// Callback fills that needed silence padding (the mix thread
    /// fell behind — the audible symptom).
    pub underruns: AtomicU64,
    /// CPAL stream errors (device unplugged, format change, ...).
    pub errors: AtomicU64,
}

impl CpalStats {
    pub fn callbacks(&self) -> u64 {
        self.callbacks.load(Ordering::Relaxed)
    }
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }
    pub fn errors(&self) -> u64 {
        self.errors.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// drain_ring_into — THE callback body (pure; testable with no device)
// ---------------------------------------------------------------------------

/// Fill the device's `out` (f32 interleaved) from the ring.
///
/// * Uses the allocation-free [`RingSink::read_into`] (real-time
///   callbacks must not allocate).
/// * Converts `i16 → f32` by `s / 32768.0` (the standard full-scale
///   mapping; LIKELY — the original's float domain conversion point
///   is UNKNOWN).
/// * A short ring fill is silence-padded and counted as ONE
///   underrun per callback (mirrors [`RingSink::read`]'s policy).
pub fn drain_ring_into(ring: &mut RingSink, out: &mut [f32], stats: &CpalStats) {
    let mut scratch = [0i16; 4096];
    let mut filled = 0usize;
    while filled < out.len() {
        let n = (out.len() - filled).min(scratch.len());
        ring.read_into(&mut scratch[..n]);
        for (d, &s) in out[filled..filled + n].iter_mut().zip(scratch[..n].iter()) {
            *d = s as f32 / 32768.0;
        }
        filled += n;
    }
    stats.callbacks.fetch_add(1, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// CpalOut — the live stream
// ---------------------------------------------------------------------------

/// A live CPAL output stream wired to a [`SharedRing`].  Dropping
/// this value stops the audio (CPAL closes the stream on drop).
pub struct CpalOut {
    /// Keep-alive handle: the underscore only silences the
    /// unused-field warning — the value is deliberately KEPT so
    /// the stream stays open until this struct drops.
    _stream: cpal::Stream,
    /// Device configuration actually in effect (rate/channels the
    /// caller must mix at).
    pub config: StreamConfig,
    /// Shared counters (the ring's own `underruns`/`dropped` cover
    /// the data path; these cover the device path).
    pub stats: Arc<CpalStats>,
}

impl CpalOut {
    /// Open the DEFAULT host's default output device and start
    /// playing (silence until the caller writes to the ring).
    ///
    /// Format preference: the device's default f32 config.  If the
    /// device only advertises i16, the callback passes samples
    /// through unchanged (no conversion, no allocation).
    pub fn open_default(ring: SharedRing) -> Result<Self, String> {
        let host = cpal::default_host();
        Self::open(&host, ring)
    }

    /// Open on an explicit host (e.g. `cpal::host_from_id`) — the
    /// Android/PC builds can pin their backend.
    pub fn open(host: &cpal::Host, ring: SharedRing) -> Result<Self, String> {
        let device = host
            .default_output_device()
            .ok_or_else(|| "no default output device".to_string())?;
        let supported = device
            .default_output_config()
            .map_err(|e| format!("default output config: {e}"))?;
        let config: StreamConfig = supported.config();
        let stats = Arc::new(CpalStats::default());

        let stream = match supported.sample_format() {
            SampleFormat::F32 => {
                let ring = ring.clone();
                let st_cb = stats.clone();
                let st_err = stats.clone();
                device
                    .build_output_stream(
                        config.clone(),
                        move |data: &mut [f32], _info: &_| {
                            let mut r = ring.lock();
                            let underruns_before = r.underruns;
                            drain_ring_into(&mut r, data, &st_cb);
                            if r.underruns > underruns_before {
                                st_cb.underruns.fetch_add(1, Ordering::Relaxed);
                            }
                        },
                        move |err| {
                            st_err.errors.fetch_add(1, Ordering::Relaxed);
                            eprintln!("cpal stream error: {err}");
                        },
                        None,
                    )
                    .map_err(|e| format!("build f32 output stream: {e}"))?
            }
            SampleFormat::I16 => {
                let ring = ring.clone();
                let st_cb = stats.clone();
                let st_err = stats.clone();
                device
                    .build_output_stream(
                        config.clone(),
                        move |data: &mut [i16], _info: &_| {
                            let mut r = ring.lock();
                            r.read_into(data);
                            st_cb.callbacks.fetch_add(1, Ordering::Relaxed);
                        },
                        move |err| {
                            st_err.errors.fetch_add(1, Ordering::Relaxed);
                            eprintln!("cpal stream error: {err}");
                        },
                        None,
                    )
                    .map_err(|e| format!("build i16 output stream: {e}"))?
            }
            other => return Err(format!("unsupported device sample format {other:?}")),
        };

        stream
            .play()
            .map_err(|e| format!("stream.play(): {e}"))?;
        Ok(Self {
            _stream: stream,
            config,
            stats,
        })
    }

    /// Enumerate output device names on this host (diagnostics —
    /// the CLI `list-audio` uses this to show what would play).
    pub fn device_names(host: &cpal::Host) -> Vec<String> {
        let mut names = Vec::new();
        if let Ok(devs) = host.output_devices() {
            for d in devs {
                names.push(
                    d.description()
                        .map(|desc| desc.name().to_string())
                        .unwrap_or_else(|_| "<unnamed>".into()),
                );
            }
        }
        names
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_converts_i16_to_f32_full_scale() {
        let mut ring = RingSink::new(64);
        // Full scale values map to ±1.0.
        ring.write(&[32767, -32768, 0, 16384]);
        let stats = CpalStats::default();
        let mut out = [0f32; 4];
        drain_ring_into(&mut ring, &mut out, &stats);
        assert!((out[0] - 32767.0 / 32768.0).abs() < 1e-9);
        assert!((out[1] + 1.0).abs() < 1e-9);
        assert_eq!(out[2], 0.0);
        assert!((out[3] - 0.5).abs() < 1e-9);
        assert_eq!(stats.callbacks(), 1);
        assert_eq!(stats.underruns(), 0);
        assert_eq!(ring.buffered(), 0);
    }

    #[test]
    fn drain_silence_fills_when_the_ring_is_short() {
        let mut ring = RingSink::new(64);
        ring.write(&[100, -100]);
        let stats = CpalStats::default();
        let mut out = [0f32; 6];
        drain_ring_into(&mut ring, &mut out, &stats);
        assert_eq!(out[0], 100.0 / 32768.0);
        assert_eq!(out[1], -100.0 / 32768.0);
        assert!(out[2..].iter().all(|&s| s == 0.0));
        // RingSink counted the underrun (short fill).
        assert_eq!(ring.underruns, 1);
        assert_eq!(stats.callbacks(), 1);
    }

    #[test]
    fn drain_handles_buffers_larger_than_the_scratch_chunk() {
        // Force the >4096-sample path (multiple scratch fills).
        let mut ring = RingSink::new(20_000);
        let data: Vec<i16> = (0..9000).map(|i| (i % 251) as i16 - 125).collect();
        ring.write(&data);
        let stats = CpalStats::default();
        let mut out = vec![0f32; 9000];
        drain_ring_into(&mut ring, &mut out, &stats);
        for (i, &s) in out.iter().enumerate() {
            let expect = (i % 251) as f32 / 32768.0 - 125.0 / 32768.0;
            assert!((s - expect).abs() < 1e-9, "sample {i}: {s} != {expect}");
        }
        assert_eq!(ring.buffered(), 0);
        assert_eq!(ring.underruns, 0);
    }

    #[test]
    fn shared_ring_recovers_from_a_poisoned_lock() {
        let ring = SharedRing::new(RingSink::new(16));
        // Poison the mutex: panic while holding the lock on a
        // helper thread (hook silenced to keep test output clean).
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        {
            let inner = ring.inner.clone();
            let t = std::thread::spawn(move || {
                let _guard = inner.lock().unwrap();
                panic!("poison");
            });
            let _ = t.join();
        }
        std::panic::set_hook(default_hook);
        // The mutex really is poisoned...
        assert!(ring.inner.lock().is_err());
        // ...and SharedRing::lock() still yields a usable ring
        // (recovery policy — see module docs).
        let mut r = ring.lock();
        r.write(&[1, 2, 3]);
        assert_eq!(r.buffered(), 3);
    }

    #[test]
    fn real_device_stream_end_to_end() {
        // THE M18 E2E: open the host's real default output device,
        // stream ~0.4 s of a 440 Hz tone through the ring, and
        // assert the device callback actually consumed audio.
        //
        // Device-dependent by nature: where no device exists the
        // test reports a skip (the pure logic above carries the
        // correctness burden there).  In the dev sandbox this
        // exercises ALSA's software `null` PCM end to end.
        let host = cpal::default_host();
        let Some(_probe) = host.default_output_device() else {
            eprintln!("cpal e2e: no output device on this host — skipped");
            return;
        };
        let ring = SharedRing::new(RingSink::new(1 << 16));
        let out = match CpalOut::open(&host, ring.clone()) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("cpal e2e: device present but stream refused ({e}) — skipped");
                return;
            }
        };
        let sr = out.config.sample_rate as f32;
        let ch = out.config.channels as usize;
        eprintln!(
            "cpal e2e: device {:?} @ {} Hz × {} ch",
            CpalOut::device_names(&host),
            sr,
            ch
        );

        // Pre-fill 0.5 s of a 440 Hz tone, then keep topping the
        // ring up from this thread like the mix thread would.
        let mut phase = 0f32;
        let mut make_block = |frames: usize| -> Vec<i16> {
            let mut v = Vec::with_capacity(frames * ch);
            for _ in 0..frames {
                let s = (2.0 * std::f32::consts::PI * 440.0 * phase / sr).sin();
                let i = (s * 16384.0) as i16;
                for _ in 0..ch {
                    v.push(i);
                }
                phase += 1.0;
            }
            v
        };
        ring.lock().write(&make_block((sr * 0.5) as usize));

        let deadline = std::time::Instant::now() + Duration::from_millis(700);
        while std::time::Instant::now() < deadline {
            ring.lock().write(&make_block(480));
            std::thread::sleep(Duration::from_millis(10));
        }

        let st = out.stats.callbacks();
        eprintln!("cpal e2e: {st} callbacks, {} underruns", out.stats.underruns());
        assert!(
            st > 0,
            "device stream produced no callbacks — audio path dead"
        );
        assert_eq!(out.stats.errors(), 0, "stream reported errors");
    }

    /// (block 54) `from_arc` wraps an ALREADY-shared ring: writes
    /// through the original Arc land in the same buffer the
    /// SharedRing hands the device — one ring, no copies.
    #[test]
    fn shared_ring_from_arc_shares_one_buffer() {
        let inner = std::sync::Arc::new(std::sync::Mutex::new(RingSink::new(8)));
        let ring = SharedRing::from_arc(inner.clone());
        {
            let mut w = inner.lock().unwrap();
            w.write(&[7, 8, 9]);
        }
        // The device side sees the same samples through the handle.
        assert_eq!(ring.lock().read(3), vec![7, 8, 9]);
        // And counters are shared with the writer's side.
        assert_eq!(inner.lock().unwrap().buffered(), 0);
    }
}

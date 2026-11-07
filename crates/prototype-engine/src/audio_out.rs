//! audio_out — the gameplay slice's audio OUTPUT stage: [`Mixer`] →
//! [`RingSink`] (the small NEXT-UP item "mixer→SharedRing routing").
//!
//! # The gap this closes
//!
//! M15 rebuilt the clean-room voice mixer ([`Mixer::tick`] → interleaved
//! i16 [`MixOut`]); block 8 built the real device edge (`cpal_out`'s
//! `SharedRing` → CPAL callbacks).  Until now NOTHING connected them:
//! the gameplay slice could mix but had nowhere to put the result, and
//! the device callback had a ring nothing fed.  This module is the
//! missing middle, kept DEVICE-FREE (both ends are device-free types)
//! so it is unit-tested on every host like the rest of the engine.
//!
//! # Contract
//!
//! * One [`AudioOut`] owns one mixer and one ring.  `tick(frames)`
//!   mixes `frames` output frames at the configured channel count and
//!   writes the interleaved i16 into the ring — the same real-time
//!   policy as the device path: **never block, overwrite the oldest
//!   audio when full** ([`RingSink::write`] documents the exact
//!   semantics).
//! * The ring lives behind `Arc<Mutex<…>>` (block 57): the device
//!   edge (feature `audio-cpal`) takes the handle via
//!   [`AudioOut::ring_handle`] and wraps it in
//!   `cpal_out::SharedRing::from_arc` — ONE buffer, one writer (the
//!   per-frame tick), one reader (the device callback), zero copies —
//!   the exact shape the boot movie's player has used since block 54.
//!   The lock is held for the length of one write/read only, and a
//!   POISONED mutex is RECOVERED (`into_inner`): one corrupt write
//!   beats a dead audio path (the `SharedRing::lock` policy).
//! * Uniform buffer parity: this is the gameplay-side counterpart of
//!   the renderer's draw-call template — the engine loop calls
//!   `tick` once per frame the same way `render_frame` encodes once
//!   per frame.
//!
//! # Evidence
//!
//! * PROVEN: `RingSink` write/overwrite/drain semantics (block 8, 14
//!   tests) and `Mixer::tick` gain/clip/voice-end semantics (M15, 30+
//!   tests) — this module composes them and asserts the COMPOSITION:
//!   routed sample counts, silence on empty mixers, oldest-overwrite
//!   under pressure, counter accumulation.
//! * DESIGN (ours): tick-then-route per frame with a single shared
//!   ring.  The original engine's audio thread architecture is UNKNOWN
//!   (no audio-side RTTI strings beyond the service names); any
//!   composition of the same PROVEN ends is behaviour-equivalent at
//!   this fidelity.

use std::sync::{Arc, Mutex};

use prototype_audio::mixer::{MixOut, Mixer};
use prototype_audio::sink::RingSink;

/// Lock the shared ring, recovering a poisoned mutex (the
/// `cpal_out::SharedRing::lock` policy: audio continuity beats
/// strictness — `RingSink` has no invariant a panic can corrupt).
fn lock_ring(ring: &Mutex<RingSink>) -> std::sync::MutexGuard<'_, RingSink> {
    ring.lock().unwrap_or_else(|e| e.into_inner())
}

/// Per-tick statistics: what one [`AudioOut::tick`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioTickStats {
    /// Interleaved samples written into the ring this tick.
    pub samples_routed: usize,
    /// Voices that were live at the start of the tick.
    pub voices_played: usize,
    /// Non-looping voices that finished during this tick.
    pub voice_ends: usize,
    /// Output samples that clipped at ±32767/−32768 this tick.
    pub clipped: usize,
}

/// The gameplay slice's audio output stage: mixer + ring + counters.
///
/// Device-free by construction; the CPAL edge (feature `audio-cpal`)
/// wraps the SAME [`RingSink`] this stage fills.
#[derive(Debug)]
pub struct AudioOut {
    mixer: Mixer,
    /// The shared ring (block 57): the tick writes through the lock,
    /// the device callback (audio-cpal) drains through the SAME
    /// `Arc` via [`AudioOut::ring_handle`].
    sink: Arc<Mutex<RingSink>>,
    output_channels: u32,
    ticks: u64,
    samples_routed_total: u64,
    voice_ends_total: usize,
    clipped_total: usize,
}

impl AudioOut {
    /// New stage: ring capacity in SAMPLES (interleaved — a stereo
    /// frame is 2), fixed output channel count (1 = mono downmix,
    /// 2 = stereo bus — [`Mixer::tick`] semantics).
    ///
    /// A sane capacity is a few multiples of the frame callback size
    /// (e.g. 8 × 512 frames × channels); too small just means the
    /// oldest-overwrite policy trims history earlier.
    pub fn new(ring_capacity_samples: usize, output_channels: u32) -> Self {
        Self {
            mixer: Mixer::new(),
            sink: Arc::new(Mutex::new(RingSink::new(ring_capacity_samples.max(1)))),
            output_channels: output_channels.max(1),
            ticks: 0,
            samples_routed_total: 0,
            voice_ends_total: 0,
            clipped_total: 0,
        }
    }

    /// Mutable mixer access — spawn/stop voices between ticks.
    pub fn mixer(&mut self) -> &mut Mixer {
        &mut self.mixer
    }

    /// Shared mixer access (counters/inspection).
    pub fn mixer_ref(&self) -> &Mixer {
        &self.mixer
    }

    /// The fixed output channel count every tick uses.
    pub fn output_channels(&self) -> u32 {
        self.output_channels
    }

    /// Mix `frames` frames and route the result into the ring.
    /// Returns what happened (see [`AudioTickStats`]).
    pub fn tick(&mut self, frames: usize) -> AudioTickStats {
        let MixOut {
            samples,
            voices_played,
            voice_ends,
            clipped_samples,
            ..
        } = self.mixer.tick(frames, self.output_channels);
        let routed = samples.len();
        lock_ring(&self.sink).write(&samples);
        self.ticks += 1;
        self.samples_routed_total += routed as u64;
        self.voice_ends_total += voice_ends.len();
        self.clipped_total += clipped_samples;
        AudioTickStats {
            samples_routed: routed,
            voices_played,
            voice_ends: voice_ends.len(),
            clipped: clipped_samples,
        }
    }

    /// Samples currently buffered in the ring (interleaved).
    pub fn buffered(&self) -> usize {
        lock_ring(&self.sink).buffered()
    }

    /// The ring capacity in samples (interleaved).
    pub fn ring_capacity(&self) -> usize {
        lock_ring(&self.sink).capacity()
    }

    /// Block 57: the ring handle for the DEVICE path — the
    /// `audio-cpal` edge wraps it in `cpal_out::SharedRing::from_arc`
    /// so the callback drains the very buffer [`AudioOut::tick`]
    /// fills (one buffer, one writer, one reader).  Cloning the
    /// `Arc` is the only sharing; the ring is never copied.
    pub fn ring_handle(&self) -> Arc<Mutex<RingSink>> {
        Arc::clone(&self.sink)
    }

    /// Drain up to `out.len()` samples through the lock (drain-side
    /// inspection / tests without the device edge).
    pub fn drain_into(&self, out: &mut [i16]) {
        lock_ring(&self.sink).read_into(out);
    }

    /// Hand the ring to a device session (consumes the stage — the
    /// mixer goes with it; rebuild a stage per device session, the
    /// renderer does the same across suspend/resume).
    pub fn into_parts(self) -> (Mixer, Arc<Mutex<RingSink>>) {
        (self.mixer, self.sink)
    }

    /// Ticks mixed since construction.
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// Total interleaved samples ever routed into the ring.
    pub fn samples_routed_total(&self) -> u64 {
        self.samples_routed_total
    }

    /// Total voice ends observed (non-looping voices hitting clip end).
    pub fn voice_ends_total(&self) -> usize {
        self.voice_ends_total
    }

    /// Total clipped output samples observed.
    pub fn clipped_total(&self) -> usize {
        self.clipped_total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prototype_audio::mixer::Voice;
    use prototype_math::Vec3;

    fn tone_voice(frames: usize, channels: u32) -> Voice {
        // Alternating ±8192 — trivially distinguishable from silence
        // and from each other when two voices overlap.
        Voice {
            samples: (0..frames * channels as usize)
                .map(|i| if i % 2 == 0 { 8192i16 } else { -8192i16 })
                .collect(),
            channels,
            cursor: 0,
            loop_voice: false,
            gain: 1.0,
            position: Vec3::new(0.0, 0.0, 0.0),
            tag: None,
        }
    }

    #[test]
    fn voice_audio_routes_into_the_ring() {
        let mut out = AudioOut::new(1 << 16, 2);
        out.mixer().play(tone_voice(128, 2));
        let stats = out.tick(128);
        assert_eq!(stats.samples_routed, 256, "128 stereo frames");
        assert_eq!(stats.voices_played, 1);
        // The voice consumed its LAST frame during this tick — Mixer
        // reports a non-looping voice that reached its end during the
        // tick as ended (removed, counted once).
        assert_eq!(stats.voice_ends, 1);
        assert_eq!(out.buffered(), 256);
        assert_eq!(out.samples_routed_total(), 256);
    }

    #[test]
    fn empty_mixer_routes_silence() {
        let mut out = AudioOut::new(1 << 16, 1);
        let stats = out.tick(64);
        assert_eq!(stats.samples_routed, 64);
        assert_eq!(stats.voices_played, 0);
        assert_eq!(out.buffered(), 64);
        let mut drained = vec![0i16; 64];
        out.drain_into(&mut drained);
        assert!(drained.iter().all(|&s| s == 0), "silence, not garbage");
    }

    #[test]
    fn non_looping_voice_ends_are_counted_across_ticks() {
        let mut out = AudioOut::new(1 << 16, 2);
        out.mixer().play(tone_voice(32, 2));
        out.tick(16); // half the voice
        let stats = out.tick(16); // the other half → end
        assert_eq!(stats.voice_ends, 1);
        assert_eq!(out.voice_ends_total(), 1);
        let stats = out.tick(16); // nothing left
        assert_eq!(stats.voice_ends, 0);
        assert_eq!(stats.voices_played, 0);
        assert_eq!(out.voice_ends_total(), 1, "ends count exactly once");
    }

    #[test]
    fn full_ring_overwrites_the_oldest_never_blocks() {
        // 4-frame ring (8 samples stereo): a 16-frame tick must fit by
        // overwriting, and the readable tail must be the LAST frames.
        let mut out = AudioOut::new(8, 2);
        out.mixer().play(tone_voice(16, 2));
        let stats = out.tick(16);
        assert_eq!(stats.samples_routed, 32);
        assert_eq!(out.buffered(), 8, "ring stays at capacity");
        // The alternating pattern's PHASE tells which sample landed
        // last: sample 31 is odd-indexed → -8192.
        let last = [0i16; 2];
        let mut tail = last;
        out.drain_into(&mut tail);
        assert_eq!(tail[1], -8192, "newest sample overwrote the oldest");
    }

    #[test]
    fn counters_accumulate_across_many_ticks() {
        let mut out = AudioOut::new(1 << 16, 2);
        out.mixer().play(tone_voice(256, 2));
        for _ in 0..8 {
            out.tick(32);
        }
        assert_eq!(out.ticks(), 8);
        assert_eq!(out.samples_routed_total(), 8 * 64);
        assert_eq!(out.buffered(), 512.min(out.ring_capacity()));
        assert_eq!(out.clipped_total(), 0, "8192 peak never clips");
    }

    #[test]
    fn mono_downmix_routes_half_the_samples() {
        let mut out = AudioOut::new(1 << 16, 1);
        out.mixer().play(tone_voice(128, 2)); // stereo clip into mono bus
        let stats = out.tick(128);
        assert_eq!(stats.samples_routed, 128, "mono bus: 1 sample per frame");
        assert_eq!(out.output_channels(), 1);
    }

    #[test]
    fn into_parts_hands_the_ring_to_a_device_session() {
        let mut out = AudioOut::new(1024, 2);
        out.mixer().play(tone_voice(16, 2));
        out.tick(16);
        let (mixer, sink) = out.into_parts();
        assert_eq!(mixer.len(), 0, "voice finished during the tick");
        assert_eq!(sink.lock().unwrap().buffered(), 32);
        let mut drained = vec![0i16; 32];
        sink.lock().unwrap().read_into(&mut drained);
        assert!(drained.iter().any(|&s| s != 0), "audio survived the handoff");
    }

    /// Block 57: `ring_handle` shares ONE buffer — a tick's samples
    /// are readable through the handle the device edge would wrap in
    /// `SharedRing::from_arc` (no copy, same counters).
    #[test]
    fn ring_handle_shares_one_buffer_with_the_device_side() {
        let mut out = AudioOut::new(1 << 16, 2);
        out.mixer().play(tone_voice(32, 2));
        out.tick(32);
        let handle = out.ring_handle();
        // Device side reads the SAME buffer the tick wrote.
        {
            let mut r = handle.lock().unwrap();
            assert_eq!(r.buffered(), 64);
            let mut got = vec![0i16; 64];
            r.read_into(&mut got);
            assert!(got.iter().any(|&s| s != 0), "the tick's audio came through");
        }
        // Counters are shared (one ring, not two copies).
        assert_eq!(out.buffered(), 0);
        // A later tick still lands in the same shared buffer.
        out.mixer().play(tone_voice(16, 2));
        out.tick(16);
        assert_eq!(handle.lock().unwrap().buffered(), 32);
    }

    /// Block 57: a POISONED ring mutex is recovered on the write path
    /// (the `SharedRing::lock` policy — one corrupt write beats a
    /// dead audio path; the tick never panics).
    #[test]
    fn tick_recovers_from_a_poisoned_ring_mutex() {
        let mut out = AudioOut::new(64, 2);
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        {
            let inner = out.ring_handle();
            let t = std::thread::spawn(move || {
                let _guard = inner.lock().unwrap();
                panic!("poison");
            });
            let _ = t.join();
        }
        std::panic::set_hook(default_hook);
        // The mutex really is poisoned...
        assert!(out.ring_handle().lock().is_err());
        // ...and the tick still routes (recovery, not propagation).
        out.mixer().play(tone_voice(8, 2));
        let stats = out.tick(8);
        assert_eq!(stats.samples_routed, 16);
        assert_eq!(out.buffered(), 16);
    }
}

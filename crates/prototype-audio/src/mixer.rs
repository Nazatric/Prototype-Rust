//! mixer — a clean-room mix graph for decoded RADP clips (M15).
//!
//! ## What this reconstructs
//!
//! The engine's real mix code is UNKNOWN (the RTTI dump names the
//! shape of the subsystem — `audio::Mix`, `audio::MixStrip`,
//! `audio::MixVoice`, `audio::Renderer`, `audio::RendererVoice`,
//! plus `CompressorLimiter` / `Reverb` / `Filter` effects and the
//! `FoldDownType` / `MixType` / `Focus` meta-enums — but not its
//! arithmetic).  This module is OUR documented clean-room model of
//! the part every audio path needs: voices that play decoded PCM,
//! advance per tick, loop, attenuate with listener distance, and
//! sum into an output frame with clipping.
//!
//! ## Model
//!
//! * [`Voice`] — one playing clip instance: interleaved PCM, its
//!   own frame cursor, loop flag, linear gain, and (for mono
//!   downmix) a per-channel equal mix.  A voice whose cursor
//!   reaches the end either wraps (loop) or finishes (removed on
//!   the tick that consumed it).
//! * [`Listener`] — position + reference/max distances and rolloff
//!   (DirectSound DS3D-style inverse distance model, LIKELY — the
//!   RTTI `BufferLocation`/`Focus` enums imply 3D voices; the
//!   original's exact curve is UNKNOWN).
//! * [`Mixer`] — owns voices; [`Mixer::tick`] sums every live
//!   voice's next `frames` frames per output channel, applies
//!   listener attenuation, clamps to i16, and returns a [`MixOut`]
//!   snapshot (interleaved samples + bookkeeping: voices played,
//!   finished, clipped-count).  Deterministic and allocation-light.
//!
//! ## Confidence labels
//!
//! * PROVEN: the INPUT — decoded RADP PCM (M13); the i16 sample
//!   domain; sample rates 44100/48000.
//! * LIKELY: voice/mix/strip layering exists (RTTI-proven class
//!   names), the inverse-distance attenuation shape (DirectSound's
//!   documented default, and the era's PC default), linear-mix +
//!   clip summarisation.
//! * UNKNOWN: the engine's exact attenuation curve, its
//!   compressor/limiter and reverb settings, mix-strip routing,
//!   `Focus`/`FoldDownType` semantics, voice limits.

#![forbid(unsafe_code)]

use prototype_math::Vec3;

// ---------------------------------------------------------------------------
// Listener — position + distance model
// ---------------------------------------------------------------------------

/// The listener for 3D attenuation (DirectSound DS3D-style inverse
/// distance model — LIKELY; see module docs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Listener {
    /// World position (same axes as the gameplay scene — +Y up).
    pub position: Vec3,
    /// Inside `min_distance` a voice plays at full gain.
    pub min_distance: f32,
    /// Beyond `max_distance` a voice is silent.
    pub max_distance: f32,
    /// Rolloff factor (0 = no attenuation; 1 = the DS3D default).
    pub rolloff: f32,
}

impl Default for Listener {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            min_distance: 1.0,
            max_distance: 1_000_000.0,
            rolloff: 1.0,
        }
    }
}

impl Listener {
    /// Linear gain in `0.0..=1.0` for a voice at `source`:
    ///
    /// * `d <= min` → 1.0
    /// * `d >= max` → 0.0
    /// * otherwise  → `min / (min + rolloff * (d - min))` — the
    ///   inverse-distance curve, normalised so the `d = min` edge
    ///   is exactly 1.0 and continuous with the clamp region.
    pub fn gain_at(&self, source: Vec3) -> f32 {
        let d = (source - self.position).length();
        if d <= self.min_distance {
            return 1.0;
        }
        if d >= self.max_distance {
            return 0.0;
        }
        let denom = self.min_distance + self.rolloff * (d - self.min_distance);
        if denom <= 0.0 {
            return 1.0;
        }
        (self.min_distance / denom).clamp(0.0, 1.0)
    }
}

// ---------------------------------------------------------------------------
// Voice — one playing clip
// ---------------------------------------------------------------------------

/// One playing clip instance.  `samples` is INTERLEAVED PCM
/// ([`prototype_audio::radp::DecodedAudio::samples`]); `channels`
/// is 1 or 2 (the RADP decode's channel count).
#[derive(Debug, Clone)]
pub struct Voice {
    /// Interleaved PCM frames (L,R per frame for stereo).
    pub samples: Vec<i16>,
    /// 1 (mono) or 2 (stereo) — must match the interleave of
    /// `samples`.
    pub channels: u32,
    /// Next frame index to play.
    pub cursor: usize,
    /// Wrap at the end instead of finishing.
    pub loop_voice: bool,
    /// Linear gain multiplier applied BEFORE listener attenuation.
    pub gain: f32,
    /// World position of the emitter (used by the listener model).
    pub position: Vec3,
    /// Optional LIFECYCLE TAG (block 67): voices spawned as one
    /// logical unit (e.g. one NIS movie's mix clip) share a tag so the
    /// whole unit can be stopped without holding mixer indices (which
    /// shift on every removal).  `None` for untagged voices — the
    /// historical shape; [`Mixer::stop_tagged`] never touches them.
    pub tag: Option<u64>,
}

impl Voice {
    /// Frames available in the clip (`samples.len() / channels`).
    pub fn frame_count(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }

    /// Is the cursor past the end (and not looping)?
    pub fn finished(&self) -> bool {
        !self.loop_voice && self.cursor >= self.frame_count()
    }
}

// ---------------------------------------------------------------------------
// Mixer
// ---------------------------------------------------------------------------

/// Why a voice stopped during a tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceEnd {
    /// Reached the end of a non-looping clip.
    ClipEnd,
}

/// Bookkeeping for one [`Mixer::tick`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MixOut {
    /// Interleaved mixed samples: `frames * output_channels`.
    pub samples: Vec<i16>,
    /// Voices that were live at the start of the tick.
    pub voices_played: usize,
    /// Voices that finished during this tick (non-looping, hit the
    /// end) and were removed.
    pub voice_ends: Vec<(usize, VoiceEnd)>,
    /// Number of output samples that clipped at ±32767/−32768.
    pub clipped_samples: usize,
    /// The linear gain the listener applied per voice index
    /// (audit/visualisation).
    pub voice_gains: Vec<f32>,
}

/// A deterministic clean-room mixer: sums live voices into i16
/// frames with listener attenuation and clipping.
///
/// NOT the original engine's mix code (UNKNOWN); the shape
/// (voices → sum → clip) is the minimal model every audio path
/// needs, and it is fully deterministic for tests.
#[derive(Debug, Clone)]
pub struct Mixer {
    pub listener: Listener,
    voices: Vec<Voice>,
}

impl Default for Mixer {
    fn default() -> Self {
        Self {
            listener: Listener::default(),
            voices: Vec::new(),
        }
    }
}

impl Mixer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_listener(listener: Listener) -> Self {
        Self { listener, voices: Vec::new() }
    }

    /// Spawn a voice; returns its handle index.  The index is the
    /// position in the current voice list — later removals shift
    /// indices (see [`Mixer::voice_ends`] bookkeeping instead of
    /// storing indices long-term).
    pub fn play(&mut self, voice: Voice) -> usize {
        self.voices.push(voice);
        self.voices.len() - 1
    }

    /// Live voice count.
    pub fn len(&self) -> usize {
        self.voices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.voices.is_empty()
    }

    /// Borrow a voice by index.
    pub fn voice(&self, idx: usize) -> Option<&Voice> {
        self.voices.get(idx)
    }

    /// Manually stop a voice by index; returns true if it existed.
    /// Stopped voices are removed immediately (they contribute no
    /// further audio).
    pub fn stop(&mut self, idx: usize) -> bool {
        if idx < self.voices.len() {
            self.voices.remove(idx);
            true
        } else {
            false
        }
    }

    /// Stop every voice carrying `tag` (block 67); returns how many
    /// were removed.  Index-free by design — the mixer owns the
    /// shifting, the caller holds only the tag.  Untagged voices
    /// (`tag: None`) are never touched.
    pub fn stop_tagged(&mut self, tag: u64) -> usize {
        let before = self.voices.len();
        self.voices.retain(|v| v.tag != Some(tag));
        before - self.voices.len()
    }

    /// How many live voices carry `tag` (block 67 diagnostics).
    pub fn tagged_len(&self, tag: u64) -> usize {
        self.voices.iter().filter(|v| v.tag == Some(tag)).count()
    }

    /// Advance the mix by `frames` output frames at `output_channels`
    /// channels (1 = downmix everything to mono, 2 = stereo bus).
    ///
    /// Every live voice contributes its next `frames` frames: its
    /// samples are multiplied by `voice.gain * listener.gain_at(...)`,
    /// accumulated into f32 accumulators, and clamped to i16 at the
    /// end.  Looping voices wrap; non-looping voices that hit the
    /// end are reported in [`MixOut::voice_ends`] and removed.
    pub fn tick(&mut self, frames: usize, output_channels: u32) -> MixOut {
        let out_ch = output_channels.max(1) as usize;
        let mut acc = vec![0.0f32; frames * out_ch];
        let mut out = MixOut::default();
        out.voices_played = self.voices.len();

        let listener = self.listener;
        let mut i = 0usize;
        while i < self.voices.len() {
            let voice = &mut self.voices[i];
            let ch = voice.channels.max(1) as usize;
            let total = voice.frame_count();
            let gain = voice.gain * listener.gain_at(voice.position);
            out.voice_gains.push(gain);

            for f in 0..frames {
                if voice.cursor >= total {
                    if voice.loop_voice && total > 0 {
                        voice.cursor = 0;
                    } else {
                        out.voice_ends.push((i, VoiceEnd::ClipEnd));
                        break;
                    }
                }
                let frame_idx = voice.cursor;
                voice.cursor += 1;
                for c in 0..out_ch {
                    // Voice channel → output channel: mono voice
                    // feeds every output channel equally; stereo
                    // voice maps L→0, R→1 (extra outputs take the
                    // mono sum of the voice's channels — LIKELY
                    // fold-down, matches `FoldDownType` existing as
                    // a concept).
                    let s = match (ch, out_ch) {
                        (1, _) => voice.samples[frame_idx] as f32,
                        (_, 1) => {
                            // stereo → mono: average the frame.
                            let l = voice.samples[frame_idx * 2] as f32;
                            let r = voice.samples[frame_idx * 2 + 1] as f32;
                            (l + r) * 0.5
                        }
                        _ => voice.samples[frame_idx * ch + c.min(ch - 1)] as f32,
                    };
                    let slot = f * out_ch + c;
                    acc[slot] += s * gain;
                }
            }
            if voice.finished() {
                // Already reported in voice_ends this tick (or ended
                // exactly at the boundary) — remove silently if not
                // reported.
                if !out.voice_ends.iter().any(|(idx, _)| *idx == i) {
                    out.voice_ends.push((i, VoiceEnd::ClipEnd));
                }
                self.voices.remove(i);
                // Do NOT advance i: the next voice slid into slot i.
            } else {
                i += 1;
            }
        }

        // Clamp to i16, counting clips.
        out.samples.reserve(acc.len());
        for a in acc {
            let clamped = a.clamp(-32768.0, 32767.0);
            if clamped != a {
                out.clipped_samples += 1;
            }
            out.samples.push(clamped as i16);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn mono_voice(samples: Vec<i16>, gain: f32) -> Voice {
        Voice {
            samples,
            channels: 1,
            cursor: 0,
            loop_voice: false,
            gain,
            position: Vec3::ZERO,
            tag: None,
        }
    }

    #[test]
    fn stop_tagged_removes_only_the_tagged_unit() {
        let mut m = Mixer::new();
        let mut a = mono_voice(vec![10, 0], 1.0);
        let b = mono_voice(vec![20, 0], 1.0);
        let c = mono_voice(vec![30, 0], 1.0);
        a.tag = Some(7);
        m.play(a);
        m.play(b);
        m.play(c);
        m.play({
            let mut d = mono_voice(vec![40, 0], 1.0);
            d.tag = Some(7);
            d
        });
        assert_eq!(m.len(), 4);
        assert_eq!(m.tagged_len(7), 2);
        assert_eq!(m.stop_tagged(7), 2, "both tagged voices removed");
        assert_eq!(m.len(), 2, "untagged voices survive");
        assert_eq!(m.tagged_len(7), 0);
        assert_eq!(m.stop_tagged(7), 0, "stopping again is a clean no-op");
        // The survivors still mix (order preserved, no drift).
        let out = m.tick(1, 1);
        assert_eq!(out.samples, vec![20 + 30]);
    }

    #[test]
    fn stop_tagged_leaves_none_untouched() {
        let mut m = Mixer::new();
        m.play(mono_voice(vec![5, 0], 1.0)); // tag: None
        m.play(mono_voice(vec![6, 0], 1.0)); // tag: None
        assert_eq!(m.stop_tagged(1), 0);
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn listener_gain_shapes_are_documented() {
        let l = Listener {
            position: Vec3::ZERO,
            min_distance: 10.0,
            max_distance: 110.0,
            rolloff: 1.0,
        };
        // At/below min: full gain.
        assert_eq!(l.gain_at(Vec3::new(0.0, 0.0, 0.0)), 1.0);
        assert_eq!(l.gain_at(Vec3::new(10.0, 0.0, 0.0)), 1.0);
        // At/above max: silent.
        assert_eq!(l.gain_at(Vec3::new(110.0, 0.0, 0.0)), 0.0);
        assert_eq!(l.gain_at(Vec3::new(1000.0, 0.0, 0.0)), 0.0);
        // Inverse curve at d = min + min (rolloff 1): min/(min + min) = 0.5.
        let g = l.gain_at(Vec3::new(20.0, 0.0, 0.0));
        assert!((g - 0.5).abs() < 1e-6, "g = {g}");
        // Monotonically non-increasing with distance.
        let mut prev = 1.0;
        for d in [10.0, 15.0, 20.0, 40.0, 80.0, 109.0] {
            let g = l.gain_at(Vec3::new(d, 0.0, 0.0));
            assert!(g <= prev + 1e-6, "gain rose at d={d}: {g} > {prev}");
            prev = g;
        }
    }

    #[test]
    fn single_voice_mixes_with_gain_and_advances() {
        let mut m = Mixer::new();
        m.play(mono_voice(vec![100, -100, 200, -200], 0.5));
        let out = m.tick(2, 1);
        assert_eq!(out.voices_played, 1);
        assert_eq!(out.samples, vec![50, -50]);
        assert_eq!(out.clipped_samples, 0);
        // The voice advanced; two more frames finish it.
        let out = m.tick(2, 1);
        assert_eq!(out.samples, vec![100, -100]);
        assert_eq!(out.voice_ends, vec![(0, VoiceEnd::ClipEnd)]);
        assert_eq!(m.len(), 0);
        // Mixing an empty mixer is silence.
        let out = m.tick(2, 1);
        assert_eq!(out.samples, vec![0, 0]);
        assert_eq!(out.voices_played, 0);
    }

    #[test]
    fn two_voices_sum_and_clip_is_counted() {
        let mut m = Mixer::new();
        m.play(mono_voice(vec![20000, 20000], 1.0));
        m.play(mono_voice(vec![20000, -20000], 1.0));
        let out = m.tick(2, 1);
        // 20000 + 20000 = 40000 → clamps to 32767 (clipped).
        assert_eq!(out.samples[0], 32767);
        // 20000 + -20000 = 0 (no clip).
        assert_eq!(out.samples[1], 0);
        assert_eq!(out.clipped_samples, 1);
        assert_eq!(out.voices_played, 2);
        // Both 2-frame voices consumed their clips → removed.
        assert_eq!(m.len(), 0);
    }

    #[test]
    fn looping_voice_wraps_and_never_finishes() {
        let mut v = mono_voice(vec![10, 20, 30], 1.0);
        v.loop_voice = true;
        let mut m = Mixer::new();
        m.play(v);
        let out = m.tick(4, 1);
        // 3-frame clip looped: frames 10, 20, 30, 10.
        assert_eq!(out.samples, vec![10, 20, 30, 10]);
        assert!(out.voice_ends.is_empty());
        assert_eq!(m.len(), 1);
        let out = m.tick(2, 1);
        assert_eq!(out.samples, vec![20, 30]);
    }

    #[test]
    fn stereo_voice_downmixes_to_mono_bus() {
        let mut v = Voice {
            samples: vec![100, 300, -200, 400], // frames (100,300), (-200,400)
            channels: 2,
            cursor: 0,
            loop_voice: false,
            gain: 1.0,
            position: Vec3::ZERO,
            tag: None,
        };
        v.channels = 2;
        let mut m = Mixer::new();
        m.play(v);
        let out = m.tick(2, 1);
        // (100+300)/2 = 200; (-200+400)/2 = 100.
        assert_eq!(out.samples, vec![200, 100]);
    }

    #[test]
    fn stereo_voice_maps_to_stereo_bus() {
        let v = Voice {
            samples: vec![100, 300, -200, 400],
            channels: 2,
            cursor: 0,
            loop_voice: false,
            gain: 1.0,
            position: Vec3::ZERO,
            tag: None,
        };
        let mut m = Mixer::new();
        m.play(v);
        let out = m.tick(2, 2);
        assert_eq!(out.samples, vec![100, 300, -200, 400]);
    }

    #[test]
    fn listener_attenuates_the_mix() {
        let near = || Voice {
            samples: vec![1000; 4],
            channels: 1,
            cursor: 0,
            loop_voice: false,
            gain: 1.0,
            position: Vec3::new(20.0, 0.0, 0.0), // gain 0.5
            tag: None,
        };
        let far = || Voice {
            samples: vec![1000; 4],
            channels: 1,
            cursor: 0,
            loop_voice: false,
            gain: 1.0,
            position: Vec3::new(2000.0, 0.0, 0.0), // past max → 0.0
            tag: None,
        };

        // A voice past max_distance contributes exactly nothing.
        let mut far_only = Mixer::with_listener(Listener {
            position: Vec3::ZERO,
            min_distance: 10.0,
            max_distance: 1000.0,
            rolloff: 1.0,
        });
        far_only.play(far());
        let out = far_only.tick(1, 1);
        assert_eq!(out.samples[0], 0);
        assert_eq!(out.voice_gains[0], 0.0);

        // A voice at distance 2×min with rolloff 1 plays at 0.5.
        let mut near_only = Mixer::with_listener(Listener {
            position: Vec3::ZERO,
            min_distance: 10.0,
            max_distance: 1000.0,
            rolloff: 1.0,
        });
        near_only.play(near());
        let out = near_only.tick(1, 1);
        assert!((out.samples[0] as i32 - 500).abs() <= 1, "got {}", out.samples[0]);
        assert!((out.voice_gains[0] - 0.5).abs() < 1e-6);

        // Together: the sum is just the near voice (far is silent).
        let mut both = Mixer::with_listener(Listener {
            position: Vec3::ZERO,
            min_distance: 10.0,
            max_distance: 1000.0,
            rolloff: 1.0,
        });
        both.play(near());
        both.play(far());
        let out = both.tick(1, 1);
        assert_eq!(out.voices_played, 2);
        assert_eq!(out.voice_gains[1], 0.0);
        assert_eq!(out.samples[0], 500);
    }

    #[test]
    fn determinism_same_inputs_same_mix() {
        let build = || {
            let mut m = Mixer::new();
            m.play(mono_voice(vec![500, -500, 1000, -1000, 32767, -32768], 0.9));
            m.play(mono_voice(vec![100, 200, 300, 400, 500, 600], 0.4));
            m
        };
        let mut a = build();
        let mut b = build();
        let ra = a.tick(3, 1);
        let rb = b.tick(3, 1);
        assert_eq!(ra, rb);
        let ra = a.tick(3, 1);
        let rb = b.tick(3, 1);
        assert_eq!(ra, rb);
    }

    #[test]
    fn real_clip_plays_through_the_mixer() {
        // End-to-end: decode the real pink_noise clip and mix it.
        use crate::AudioClipCatalog;
        use std::path::Path;
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join("boot_strap.p3d");
        let data = std::fs::read(&p).unwrap();
        let file = prototype_p3d::P3dFile::parse(&data).unwrap();
        let cat = AudioClipCatalog::from_p3d(&file).unwrap();
        let clip = cat.find("pink_noise").unwrap();
        let audio = clip.decode().unwrap();
        assert_eq!(audio.channels, 1);

        let mut m = Mixer::new();
        m.play(Voice {
            samples: audio.samples.clone(),
            channels: 1,
            cursor: 0,
            loop_voice: false,
            gain: 1.0,
            tag: None,
            position: Vec3::ZERO,
        });
        // Mix 100 frames twice; the RMS of the first window must be
        // substantial (it's pink noise at full gain).
        let out = m.tick(100, 1);
        assert_eq!(out.voices_played, 1);
        assert_eq!(out.samples.len(), 100);
        let rms: f64 = out
            .samples
            .iter()
            .map(|s| (*s as i64 * *s as i64) as f64)
            .sum::<f64>()
            .sqrt();
        assert!(rms > 5000.0, "pink noise rms {rms} too quiet");
        // And the clip eventually finishes exactly: 100 frames up
        // front, then ceil(48128 − 100)/1000 = 49 loop ticks → 50
        // tick calls total.
        let mut ticks = 1usize;
        while m.len() > 0 {
            m.tick(1000, 1);
            ticks += 1;
            assert!(ticks < 100, "voice never finished");
        }
        assert_eq!(ticks, 50);
    }
}

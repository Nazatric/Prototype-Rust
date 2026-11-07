//! sample_rate — block 59: the `audio::SampleRateConverter`
//! reconstruction, promoted out of the CLI (where a private
//! `resample_linear` served `play-audio` alone) into the library so
//! the ENGINE paths can convert too.
//!
//! # The gap this closes
//!
//! The gameplay registry (`prototype-engine::gameplay_sounds`) holds
//! the engine's REAL audio vocabulary — 70 clips in the user's own
//! `boot_strap.p3d`, of which **25 decode at 44 100 Hz** (the
//! movement/powers pair `air_brake` + `big_heavy_whoosh_01`, the
//! frontend nav/select clips `ui_fe_main_nav`/`ui_fe_ht_nav`/
//! `ui_fe_ht_select`, the HUD loops, …) while the app's session mix
//! runs at 48 kHz.  Until this module the engine could only play the
//! 48 kHz half rate-clean; a 44.1 kHz voice consumed at 48 kHz shifts
//! pitch by 8.84 % (48000/44100) — the documented SRC UNKNOWN.  The
//! CLI's `play-audio` had a private converter, but a BINARY-PRIVATE
//! fn is unreachable from the engine — the block-58 note "the
//! sample-rate converter is still unreconstructed" meant exactly
//! that the LIBRARY had none.
//!
//! # Evidence
//!
//! * PROVEN: the class exists in the original engine — RTTI names
//!   `audio::SampleRateConverter` (the same hint the device edge's
//!   docs record); the two clip rates 44 100 / 48 000 are PROVEN
//!   across the shipped fixture; the i16 sample domain is PROVEN
//!   (the RADP decode).
//! * UNKNOWN: the original's resampling KERNEL (no disassembly of
//!   the class body has been done; no string/RTTI evidence of a
//!   windowed-sinc vs linear vs polyphase design).
//! * DESIGN (ours, documented): linear interpolation between the two
//!   neighbouring input frames, per channel, with a hold at the last
//!   frame — the smallest deterministic, testable kernel that keeps
//!   duration and DC level exact.  Honesty about quality: linear
//!   interpolation attenuates the first imaging image only ~13 dB
//!   for full-band material and rolls off near Nyquist; that is
//!   adequate for one-shot sound effects (every clip in the shipped
//!   fixture is one), and the ORIGINAL kernel remains UNKNOWN — if
//!   the engine's real converter is ever disassembled, this module's
//!   API is the seam to replace.
//!
//! # Contract
//!
//! * [`resample`] is ONE-SHOT `&[i16] → Vec<i16>` (clip-scale: the
//!   largest shipped clip is ~8.5 s ≈ 408 k samples; a streaming
//!   variant is a mechanical extension if long music ever needs it).
//! * Duration-preserving: the output frame count is the exact CEIL
//!   of `frames_in · to / from` in u64 (the CLI's private copy used
//!   FLOOR and could drop up to one frame — corrected here, and the
//!   CLI now delegates to this function).
//! * Deterministic and panic-free: empty input → empty output;
//!   `channels == 0` or a zero rate return the input unchanged
//!   (documented degenerate guards, never a division by zero); f64
//!   position arithmetic is exact far beyond clip scale (< 2⁵³).
//! * Output values are clamped into `i16` — interpolation between
//!   two in-range samples never overshoots them (pinned by test).

#![forbid(unsafe_code)]

/// The output frame count for `frames_in` frames converted
/// `from` → `to` Hz: the exact CEILING of `frames_in · to / from`
/// computed in u64 (no float rounding), so the converted clip is
/// never SHORTER than the source (one input frame at 44 100 → 48 000
/// is still 2 output frames, not 1).
pub fn resampled_frame_count(frames_in: usize, from: u32, to: u32) -> usize {
    if from == 0 || to == 0 {
        return frames_in;
    }
    if frames_in == 0 {
        return 0;
    }
    ((frames_in as u64 * to as u64 + from as u64 - 1) / from as u64) as usize
}

/// Resample INTERLEAVED i16 PCM from `from` Hz to `to` Hz (linear
/// interpolation between neighbouring frames, per channel — see the
/// module docs for the evidence and quality notes).
///
/// * `channels` — samples per frame (1 = mono, 2 = stereo; the RADP
///   decode's shapes).  `0` is treated as "no interleave known" and
///   returns the input unchanged.
/// * A zero `from`/`to` rate returns the input unchanged (a
///   degenerate rate must not divide by zero — the caller's rate
///   bug must not panic the audio path).
/// * `from == to` returns an exact copy (the hot path for the 45
///   48 kHz clips in the shipped vocabulary).
pub fn resample(samples: &[i16], channels: u32, from: u32, to: u32) -> Vec<i16> {
    if from == to || channels == 0 || from == 0 || to == 0 || samples.is_empty() {
        return samples.to_vec();
    }
    let ch = channels as usize;
    let in_frames = samples.len() / ch;
    let out_frames = resampled_frame_count(in_frames, from, to);
    let from_f = from as f64;
    let to_f = to as f64;
    let mut out = Vec::with_capacity(out_frames * ch);
    for f in 0..out_frames {
        // Source position in input frames (fractional).  f64 keeps
        // this exact for any clip-scale input (n·from < 2⁵³).
        let pos = f as f64 * from_f / to_f;
        let i0 = pos.floor() as usize;
        let i1 = (i0 + 1).min(in_frames - 1);
        let frac = (pos - i0 as f64) as f32;
        for c in 0..ch {
            let a = samples[i0 * ch + c] as f32;
            let b = samples[i1 * ch + c] as f32;
            let v = a + (b - a) * frac;
            out.push(v.round().clamp(-32768.0, 32767.0) as i16);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests — synthetic math pins (the REAL-fixture pins live in
// prototype-engine::gameplay_sounds, which owns the fixture path)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_rate_is_an_exact_copy() {
        let v = vec![1i16, -2, 3, -4, 5, -6];
        assert_eq!(resample(&v, 1, 44_100, 44_100), v);
        assert_eq!(resample(&v, 2, 48_000, 48_000), v);
        assert_eq!(resampled_frame_count(3, 48_000, 48_000), 3);
    }

    #[test]
    fn degenerate_guards_return_the_input_without_panicking() {
        let v = vec![5i16, 6, 7, 8];
        // Unknown interleave.
        assert_eq!(resample(&v, 0, 44_100, 48_000), v);
        // Zero rates (the caller's bug — must not divide by zero).
        assert_eq!(resample(&v, 1, 0, 48_000), v);
        assert_eq!(resample(&v, 1, 44_100, 0), v);
        // Empty input.
        let empty: Vec<i16> = Vec::new();
        assert!(resample(&empty, 2, 44_100, 48_000).is_empty());
        assert_eq!(resampled_frame_count(0, 44_100, 48_000), 0);
    }

    #[test]
    fn two_times_upscale_lands_on_exact_midpoints() {
        // 100 → 200 Hz doubles the frame count; interior points are
        // the midpoints of the input; the tail HOLDS the last frame.
        let v = vec![0i16, 100];
        let up = resample(&v, 1, 100, 200);
        assert_eq!(up, vec![0, 50, 100, 100]);
    }

    #[test]
    fn three_frame_ramp_pins_the_hold_at_the_last_frame() {
        // Positions for [0,10,20] at 1→2: 0, .5, 1, 1.5, 2, 2.5 —
        // the last position is past the end and HOLDS 20 (no
        // out-of-bounds read, no invented sample).
        let v = vec![0i16, 10, 20];
        assert_eq!(resample(&v, 1, 100, 200), vec![0, 5, 10, 15, 20, 20]);
    }

    #[test]
    fn an_impulse_becomes_the_exact_linear_triangle() {
        let v = vec![0i16, 100, 0];
        assert_eq!(resample(&v, 1, 100, 200), vec![0, 50, 100, 50, 0, 0]);
    }

    #[test]
    fn duration_arithmetic_is_ceil_exact_in_u64() {
        // 1 second at 44.1 kHz → exactly 48 000 frames at 48 kHz.
        assert_eq!(resampled_frame_count(44_100, 44_100, 48_000), 48_000);
        // One input frame still produces TWO output frames (ceil —
        // the old floor version dropped it to 1).
        assert_eq!(resampled_frame_count(1, 44_100, 48_000), 2);
        // The REAL movement clip air_brake: 2001 RADP blocks × 32 =
        // 64 032 mono frames → 69 695 output frames (this exact
        // number is pinned by the real-fixture test in
        // gameplay_sounds; here the arithmetic must agree).
        assert_eq!(resampled_frame_count(64_032, 44_100, 48_000), 69_695);
        // Down-conversion: 48 000 @48k → 44 100 keeps 44 100.
        assert_eq!(resampled_frame_count(48_000, 48_000, 44_100), 44_100);
    }

    #[test]
    fn a_constant_level_stays_constant_no_ripple() {
        let v = vec![1234i16; 4410]; // 0.1 s mono
        let out = resample(&v, 1, 44_100, 48_000);
        assert_eq!(out.len(), 4800);
        assert!(out.iter().all(|&s| s == 1234), "DC must not ripple");
    }

    #[test]
    fn interpolation_never_overshoots_the_input_range() {
        // Deterministic pseudo-random sweep (LCG): linear
        // interpolation is a convex combination, so |out| can never
        // exceed max|in|.
        let mut x: u32 = 0x1234_5678;
        let mut v = Vec::with_capacity(10_000);
        for _ in 0..10_000 {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            // The cast wraps into the FULL i16 range (−32768..=32767).
            v.push((x % 65_536) as u16 as i16);
        }
        let peak_in = v.iter().map(|&s| s.unsigned_abs()).max().unwrap();
        let out = resample(&v, 1, 48_000, 44_100);
        let peak_out = out.iter().map(|&s| s.unsigned_abs()).max().unwrap();
        assert!(peak_out <= peak_in, "{peak_out} > {peak_in}");
    }

    #[test]
    fn stereo_channels_resample_independently() {
        // Frame pairs (L,R): L climbs, R falls — odd frame count so
        // the ceil/hold path is exercised too.
        let v = vec![0i16, 100, 100, 0, 40, 60];
        let up = resample(&v, 2, 100, 200);
        let l = resample(&[0i16, 100, 40], 1, 100, 200);
        let r = resample(&[100i16, 0, 60], 1, 100, 200);
        let inter: Vec<i16> = l
            .into_iter()
            .zip(r)
            .flat_map(|(a, b)| [a, b])
            .collect();
        assert_eq!(up.len(), 12); // 3 frames in → 6 frames out → 12 samples
        assert_eq!(up, inter, "stereo must be per-channel independent");
        // The last input frame (40,60) HOLDS across the two final
        // output frames (positions 2.0 and 2.5 clamp to the end).
        assert_eq!(&up[8..12], &[40, 60, 40, 60]);
    }

    #[test]
    fn extremes_clamp_instead_of_overflowing() {
        let v = vec![32767i16, -32768];
        let up = resample(&v, 1, 100, 300);
        assert_eq!(up.len(), 6);
        assert!(up.iter().all(|&s| (-32768..=32767).contains(&s)));
    }
}

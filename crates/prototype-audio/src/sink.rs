//! sink — PCM output plumbing for a future real-time audio device.
//!
//! The engine's real output path is RTTI-PROVEN at the class level
//! (`audio::Renderer`, `RendererVoice`, and the PC driver stack
//! XAudio2 / DirectSound / WDM-KS / MME / ASIO), but this sandbox
//! has no audio device.  This module provides the device-facing
//! piece that IS testable here: a [`PcmSink`] trait and a
//! [`RingSink`] — the fixed-capacity ring buffer an audio callback
//! (CPAL on PC/Android, later) will pull from while the game thread
//! mixes into it.
//!
//! ## Model
//!
//! * [`RingSink::write`] (game thread): append interleaved i16
//!   frames from a [`crate::mixer::MixOut`]; drops the OLDEST
//!   audio when full (never blocks the mix — the same policy
//!   real-time engines use; a dropped-audio counter is kept).
//! * [`RingSink::read`] (device callback): take up to `n` frames;
//!   if fewer are buffered, the remainder is silence and an
//!   `underruns` counter increments (the audible symptom of the
//!   mix thread falling behind).
//! * Both sides are `&mut self` here — single-threaded model.  A
//!   real host wraps the ring in its own synchronization (CPAL
//!   gives the callback; the host chooses the primitive), which is
//!   why this crate does not pick a locking strategy for you.

#![forbid(unsafe_code)]

/// A consumer of interleaved 16-bit PCM frames.
pub trait PcmSink {
    /// Append `samples.len()` interleaved samples.
    fn write(&mut self, samples: &[i16]);
    /// Buffered frame count (`samples`, not channels).
    fn buffered(&self) -> usize;
}

/// Fixed-capacity interleaved PCM ring buffer with real-time
/// policies: overwrite-oldest on the write side, silence-fill on
/// the read side.
#[derive(Debug)]
pub struct RingSink {
    buf: Vec<i16>,
    /// Read cursor (absolute, wrapping via modulo capacity).
    read_pos: usize,
    /// Write cursor (absolute).
    write_pos: usize,
    /// Live sample count (`<= capacity`).
    len: usize,
    /// Samples dropped on the write side (buffer full).
    pub dropped: u64,
    /// Read requests served partially with silence.
    pub underruns: u64,
}

impl RingSink {
    /// Capacity in SAMPLES (interleaved — a stereo frame is 2).
    /// The buffer always behaves as if fully allocated.
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: vec![0; capacity],
            read_pos: 0,
            write_pos: 0,
            len: 0,
            dropped: 0,
            underruns: 0,
        }
    }

    /// Capacity in samples.
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// Append samples; if they exceed capacity, the OLDEST buffered
    /// audio is overwritten (real-time policy — never block) and
    /// `dropped` counts the overwritten samples.
    pub fn write(&mut self, samples: &[i16]) {
        for &s in samples {
            if self.len == self.buf.len() {
                // Full: overwrite the oldest sample.
                self.read_pos = (self.read_pos + 1) % self.buf.len();
                self.len -= 1;
                self.dropped += 1;
            }
            self.buf[self.write_pos] = s;
            self.write_pos = (self.write_pos + 1) % self.buf.len();
            self.len += 1;
        }
    }

    /// Take up to `n` samples; a short buffer is padded with
    /// silence (and `underruns` increments ONCE per short read —
    /// the audible symptom of the mix thread falling behind).
    /// Returns exactly `n` samples.
    pub fn read(&mut self, n: usize) -> Vec<i16> {
        let mut out = Vec::with_capacity(n);
        let mut had_underrun = false;
        for _ in 0..n {
            if self.len > 0 {
                out.push(self.buf[self.read_pos]);
                self.read_pos = (self.read_pos + 1) % self.buf.len();
                self.len -= 1;
            } else {
                out.push(0);
                had_underrun = true;
            }
        }
        if had_underrun {
            self.underruns += 1;
        }
        out
    }

    /// Allocation-free variant of [`Self::read`]: fill `out`
    /// entirely (silence-padded), consuming up to `out.len()`
    /// buffered samples.  Real-time audio callbacks must not
    /// allocate, so the device edge (CPAL) uses THIS path, not
    /// [`Self::read`].  `underruns` increments once per short fill.
    pub fn read_into(&mut self, out: &mut [i16]) {
        let mut had_underrun = false;
        for slot in out.iter_mut() {
            if self.len > 0 {
                *slot = self.buf[self.read_pos];
                self.read_pos = (self.read_pos + 1) % self.buf.len();
                self.len -= 1;
            } else {
                *slot = 0;
                had_underrun = true;
            }
        }
        if had_underrun {
            self.underruns += 1;
        }
    }

    /// Buffered samples.
    pub fn buffered(&self) -> usize {
        self.len
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_write_then_read_round_trips() {
        let mut r = RingSink::new(8);
        r.write(&[1, 2, 3, 4, 5]);
        assert_eq!(r.buffered(), 5);
        assert_eq!(r.read(3), vec![1, 2, 3]);
        assert_eq!(r.buffered(), 2);
        r.write(&[6, 7, 8, 9]);
        assert_eq!(r.read(4), vec![4, 5, 6, 7]);
        assert_eq!(r.dropped, 0);
        assert_eq!(r.read(1), vec![8]);
        assert_eq!(r.buffered(), 1); // one sample (9) remains
        assert_eq!(r.read(1), vec![9]);
        assert_eq!(r.buffered(), 0);
    }

    #[test]
    fn full_ring_overwrites_the_oldest_audio() {
        let mut r = RingSink::new(4);
        r.write(&[1, 2, 3, 4]);
        assert_eq!(r.dropped, 0);
        r.write(&[5, 6]); // overwrites 1, 2
        assert_eq!(r.dropped, 2);
        assert_eq!(r.buffered(), 4);
        assert_eq!(r.read(4), vec![3, 4, 5, 6]);
    }

    #[test]
    fn short_read_pads_with_silence_and_counts_underrun() {
        let mut r = RingSink::new(8);
        r.write(&[10, 20]);
        let out = r.read(4);
        assert_eq!(out, vec![10, 20, 0, 0]);
        assert_eq!(r.underruns, 1);
        // A read on an empty ring is also an underrun.
        let out = r.read(2);
        assert_eq!(out, vec![0, 0]);
        assert_eq!(r.underruns, 2);
    }

    #[test]
    fn ring_wraps_around_the_capacity_boundary() {
        let mut r = RingSink::new(4);
        // Push 10 samples through a 4-slot ring: every window of the
        // last 4 written samples must read back in order.
        let data: Vec<i16> = (0..10).map(|i| i * 11).collect();
        r.write(&data);
        assert_eq!(r.read(4), vec![66, 77, 88, 99]);
    }

    #[test]
    fn read_into_is_allocation_free_and_matches_read() {
        let mut a = RingSink::new(8);
        let mut b = RingSink::new(8);
        for r in [&mut a, &mut b] {
            r.write(&[10, 20, 30]);
        }
        let mut out = [0i16; 5];
        b.read_into(&mut out);
        assert_eq!(out, [10, 20, 30, 0, 0]);
        assert_eq!(b.underruns, 1);
        assert_eq!(a.read(5), out.to_vec());
        // Exact-size fill has no underrun.
        let mut r = RingSink::new(8);
        r.write(&[1, 2, 3, 4]);
        let mut out = [0i16; 4];
        r.read_into(&mut out);
        assert_eq!(out, [1, 2, 3, 4]);
        assert_eq!(r.underruns, 0);
        assert_eq!(r.buffered(), 0);
    }

    #[test]
    fn mixer_feeds_the_sink_like_a_real_session() {
        use crate::mixer::{Mixer, Voice};

        // A tiny "session": mix 4 frames of 16 samples each into a
        // 32-sample ring, drain 16 per "callback", never lose audio.
        let mut m = Mixer::new();
        m.play(Voice {
            samples: (0..64).map(|i| (i as i16) * 3).collect(),
            channels: 1,
            cursor: 0,
            loop_voice: false,
            gain: 1.0,
            position: prototype_math::Vec3::ZERO,
            tag: None,
        });
        let mut sink = RingSink::new(32);
        for _ in 0..4 {
            let out = m.tick(16, 1);
            sink.write(&out.samples);
            // The callback drains what the mix just produced.
            let played = sink.read(16);
            assert_eq!(played.len(), 16);
            assert!(played.iter().any(|s| *s != 0));
        }
        assert_eq!(sink.underruns, 0);
        assert_eq!(sink.dropped, 0);
    }
}

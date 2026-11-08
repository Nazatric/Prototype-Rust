//! Bink AUDIO decoder — the DCT variant (the only one this game's
//! movies use), the codec's third real decoder layer.
//!
//! Decodes one track's audio packets to float PCM with FFmpeg-exact
//! semantics, then validates against real static ffmpeg: every block
//! of the intro movie's four real tracks (2 stereo + 2 mono) decodes
//! to s16 PCM within 1 LSB of `ffmpeg -f s32le`-grade reference output
//! (correlation 1.0000000, float max diff ~2e-8 — see PROGRESS.md).
//!
//! ## Reference (block 52, fetched to `scripts/ffmpeg_src/`)
//!
//! * `libavcodec/binkaudio.c` (master + n7.0.2) — `decode_block`'s
//!   coefficient parse, the packet drain loop (`ch_offset`/align32/
//!   `get_bits_left`), `get_float` (5-bit power / 23-bit mantissa /
   //! 1-bit sign, `ldexpf(m, power - 23)`), the quant table
//!   (`expf(i * 0.15289164787221953823f) * root`), the bands from
//!   `ff_wma_critical_freqs`, and the overlap crossfade
//!   `(prev*(count-j) + cur*j)/count` with `j = ch + i*channels`.
//! * `libavcodec/wma_freqs.c` — the 25 critical frequencies
//!   (mechanically copied below).
//!
//! ## The transform contract — PROVEN empirically (scripts/m52_probe*.py)
//!
//! Master inits `AV_TX_FLOAT_DCT` with len `1 << (frame_len_bits-1)`
//! while the parser fills `frame_len` coefficients; synthetic packets
//! decoded by real ffmpeg pin the actual contract:
//!
//! ```text
//! out[k] = (2/frame_len) * sum_{n<frame_len} c[n] * cos(pi*(2k+1)*n/(2*frame_len))
//! ```
//!
//! — a plain DCT-III with kernel scale 2/frame_len (1/1024 at
//! frame_len 2048), for every n INCLUDING a full-weight DC at the RAW
//! (undoubled) `c[0]`: FFmpeg's tx carries a built-in half-weight DC
//! term which `decode_block`'s `coeffs[0] /= 0.5` cancels — a
//! plain-kernel implementation must NOT double the DC.  The probe
//! additionally proved the width-0 run branch's band/quant advance
//! DOES update `q` (master's source is correct) and the crossfade
//! formula is exact to 1.6e-12.
//!
//! ## Scope
//!
//! DCT tracks only (`BINK_AUD_USEDCT`, flag 0x1000 — block 60's
//! census proves ALL 326 audio tracks of the game's 157 shipped
//! movies set it, so this covers every track the game can play).
//! RDFT tracks fail closed at construction: the transform contract
//! could not be pinned against the reference binary (the block-60
//! probe scripts, `scripts/m60_*.py`, show the binary's RDFT path
//! deviating from the fetched FFmpeg source on synthetic RLE-run
//! packets — deterministic nonzero output for all-zero coefficient
//! blocks — so the honest state stays UNKNOWN, and the gate is
//! unreachable by this game's content).  Mono and stereo tracks are
//! both decoded (the intro carries two of each); like FFmpeg, one
//! `decode_block` call carries `min(2, channels - ch_offset)`
//! channels and the packet drains block by block until its bits are
//! exhausted (each block = 1920 samples per channel at 48 kHz,
//! 32-bit aligned between blocks).

use crate::binkvideo::LeBitReader;
use prototype_common::{P3dError, Result};

/// `ff_wma_critical_freqs` (libavcodec/wma_freqs.c, mechanically copied).
const WMA_CRITICAL_FREQS: [u16; 25] = [
    100, 200, 300, 400, 510, 630, 770, 920, 1080, 1270, 1480, 1720, 2000, 2320, 2700, 3150, 3700,
    4400, 5300, 6400, 7700, 9500, 12000, 15500, 24500,
];

/// `rle_length_tab` — the run lengths behind the 4-bit RLE selector
/// (x8 coefficients per unit).
const RLE_LEN_TAB: [usize; 16] = [2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14, 15, 16, 32, 64];

/// The quant-table exponent (`0.066399999/log10(M_E)`).
const QUANT_EXP: f32 = 0.152_891_65_f32;

/// The DCT-III transform size ratio: out[k] = (2/frame_len) * sum(...).
const DCT_KERNEL_GAIN: f64 = 2.0;

/// One DCT block carries at most 2 channels (FFmpeg's MAX_CHANNELS).
const BLOCK_CHANNELS: usize = 2;

// ---------------------------------------------------------------------------
// Radix-2 complex FFT (unnormalized, inverse convention: e^{+2pi i k/N})
// ---------------------------------------------------------------------------

struct Fft {
    n: usize,
    /// Bit-reversal permutation of 0..n.
    rev: Vec<usize>,
    /// Twiddles for e^{+2 pi i k / n}: cos/sin, computed in f64.
    cos: Vec<f32>,
    sin: Vec<f32>,
}

impl Fft {
    fn new(n: usize) -> Self {
        assert!(n.is_power_of_two());
        let bits = n.trailing_zeros();
        let rev = (0..n)
            .map(|i| i.reverse_bits() >> (usize::BITS - bits))
            .collect();
        let mut cos = Vec::with_capacity(n / 2);
        let mut sin = Vec::with_capacity(n / 2);
        for k in 0..n / 2 {
            let a = 2.0 * std::f64::consts::PI * (k as f64) / (n as f64);
            cos.push(a.cos() as f32);
            sin.push(a.sin() as f32);
        }
        Self { n, rev, cos, sin }
    }

    /// In-place unnormalized inverse FFT (e^{+2 pi i k n / N}).
    fn fft(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.n;
        for i in 0..n {
            let j = self.rev[i];
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            for base in (0..n).step_by(len) {
                for k in 0..half {
                    let tw = k * step;
                    let (wr, wi) = (self.cos[tw], self.sin[tw]);
                    let (xr, xi) = (re[base + k + half], im[base + k + half]);
                    let tr = xr * wr - xi * wi;
                    let ti = xr * wi + xi * wr;
                    re[base + k + half] = re[base + k] - tr;
                    im[base + k + half] = im[base + k] - ti;
                    re[base + k] += tr;
                    im[base + k] += ti;
                }
            }
            len *= 2;
        }
    }
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

/// One decoded audio block: `channels` planes of `frame_len - overlap`
/// samples (1920 per channel at 48 kHz).
pub struct AudioBlock {
    /// Interleaved samples: `ch0[0], ch1[0], ch0[1], ...`.
    pub samples: Vec<f32>,
    pub channels: usize,
}

/// A stateful Bink Audio (DCT) decoder for ONE track.
pub struct BinkAudioDecoder {
    channels: usize,
    frame_len: usize,
    overlap_len: usize,
    root: f32,
    num_bands: usize,
    bands: Vec<usize>,
    quant_table: [f32; 96],
    previous: Vec<Vec<f32>>,
    first: bool,
    fft: Fft,
}

impl BinkAudioDecoder {
    /// `stereo` selects the 2-channel (vs mono) DCT track layout.
    /// `dct` must be true — RDFT tracks are rejected (unimplemented;
    /// the real files never exercise them, flag bit 0x1000 always set).
    pub fn new(sample_rate: u32, stereo: bool, dct: bool) -> Result<Self> {
        if !dct {
            return Err(P3dError::Other(
                "binkaudio: RDFT tracks are not implemented (this game's \
                 files are all DCT)"
                    .into(),
            ));
        }
        if !(8000..=96000).contains(&sample_rate) {
            return Err(P3dError::Other(format!(
                "binkaudio: implausible sample rate {sample_rate}"
            )));
        }
        let frame_len_bits: u32 = if sample_rate < 22050 {
            9
        } else if sample_rate < 44100 {
            10
        } else {
            11
        };
        let frame_len = 1usize << frame_len_bits;
        let overlap_len = frame_len / 16;
        let channels = if stereo { 2 } else { 1 };
        // FFmpeg: root = frame_len / (sqrt(frame_len) * 32768) for DCT.
        let root = frame_len as f32 / ((frame_len as f32).sqrt() * 32768.0);
        let mut quant_table = [0f32; 96];
        for (i, q) in quant_table.iter_mut().enumerate() {
            *q = ((i as f32) * QUANT_EXP).exp() * root;
        }
        let sample_rate_half = ((sample_rate as i64) + 1) / 2;
        let mut num_bands = 1usize;
        while num_bands < 25 {
            if sample_rate_half <= WMA_CRITICAL_FREQS[num_bands - 1] as i64 {
                break;
            }
            num_bands += 1;
        }
        let mut bands = Vec::with_capacity(num_bands + 1);
        bands.push(2usize);
        for i in 1..num_bands {
            let v = (WMA_CRITICAL_FREQS[i - 1] as u64 * frame_len as u64
                / sample_rate_half as u64) as usize;
            bands.push(v & !1);
        }
        bands.push(frame_len);
        Ok(Self {
            channels,
            frame_len,
            overlap_len,
            root,
            num_bands,
            bands,
            quant_table,
            previous: vec![vec![0f32; overlap_len]; channels],
            first: true,
            fft: Fft::new(frame_len * 2),
        })
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn frame_len(&self) -> usize {
        self.frame_len
    }

    /// `get_float` — 5-bit power, 23-bit mantissa, 1-bit sign:
    /// `ldexpf(mantissa, power - 23)`, negated when the sign bit is set.
    fn get_float(gb: &mut LeBitReader) -> Result<f32> {
        let power = gb.bits(5)? as i32;
        let mantissa = gb.bits(23)? as f32;
        let f = mantissa * (2.0f32).powi(power - 23);
        if gb.bit()? != 0 {
            Ok(-f)
        } else {
            Ok(f)
        }
    }

    /// One channel's coefficient parse (FFmpeg's `decode_block` body,
    /// version_b = 0 / version_2 = 0: 58-bit float pair, 8-bit quant
    /// indices, RLE-or-8 runs, the width-0 branch's band/quant advance).
    fn parse_coeffs(&self, gb: &mut LeBitReader, coeffs: &mut [f32]) -> Result<()> {
        if gb.bits_left() < 58 {
            return Err(P3dError::Other(
                "binkaudio: fewer than 58 bits left for the float pair".into(),
            ));
        }
        coeffs[0] = Self::get_float(gb)? * self.root;
        coeffs[1] = Self::get_float(gb)? * self.root;
        if gb.bits_left() < self.num_bands * 8 {
            return Err(P3dError::Other(
                "binkaudio: fewer bits left than the quant table needs".into(),
            ));
        }
        let mut quant = [0f32; 25];
        for i in 0..self.num_bands {
            let value = gb.bits(8)? as usize;
            quant[i] = self.quant_table[value.min(95)];
        }
        let mut k = 0usize;
        let mut q = quant[0];
        let mut i = 2usize;
        while i < self.frame_len {
            let j = if gb.bit()? != 0 {
                let v = gb.bits(4)? as usize;
                i + RLE_LEN_TAB[v] * 8
            } else {
                i + 8
            };
            let j = j.min(self.frame_len);
            let width = gb.bits(4)? as usize;
            if width == 0 {
                for c in coeffs[i..j].iter_mut() {
                    *c = 0.0;
                }
                i = j;
                while self.bands[k] < i {
                    q = quant[k];
                    k += 1;
                }
            } else {
                while i < j {
                    if self.bands[k] == i {
                        q = quant[k];
                        k += 1;
                    }
                    let coeff = gb.bits(width)? as f32;
                    if coeff != 0.0 {
                        if gb.bit()? != 0 {
                            coeffs[i] = -q * coeff;
                        } else {
                            coeffs[i] = q * coeff;
                        }
                    } else {
                        coeffs[i] = 0.0;
                    }
                    i += 1;
                }
            }
        }
        Ok(())
    }

    /// The PROVEN transform: plain DCT-III, kernel scale 2/frame_len,
    /// RAW (undoubled) DC — via a padded inverse FFT of 2*frame_len.
    fn dctiii(&self, coeffs: &[f32], out: &mut [f32]) {
        let n = self.frame_len;
        let mut re = vec![0f32; 2 * n];
        let mut im = vec![0f32; 2 * n];
        for m in 0..n {
            let phase = std::f64::consts::PI * (m as f64) / ((2 * n) as f64);
            re[m] = coeffs[m] * (phase.cos() as f32);
            im[m] = coeffs[m] * (phase.sin() as f32);
        }
        self.fft.fft(&mut re, &mut im);
        let gain = DCT_KERNEL_GAIN / (n as f64);
        for k in 0..n {
            out[k] = (re[k] as f64 * gain) as f32;
        }
    }

    /// Decode one track packet (`[u32 decoded_count][bitstream...]` —
    /// the leading u32 is skipped exactly like FFmpeg) to interleaved
    /// f32 samples: `blocks * (frame_len - overlap) * channels` values.
    /// Fails closed on any parse deviation (FFmpeg's "Incomplete
    /// packet" kills the whole packet too).
    pub fn decode_packet(&mut self, packet: &[u8]) -> Result<Vec<f32>> {
        if packet.len() < 4 {
            return Err(P3dError::Other("binkaudio: packet too small".into()));
        }
        let mut gb = LeBitReader::new(packet);
        gb.bits(16)?;
        gb.bits(16)?; // skip the reported (decompressed) size
        let mut out = Vec::new();
        loop {
            if self.dct_packet(&mut gb, &mut out)? {
                break;
            }
        }
        Ok(out)
    }

    /// One decode_block round (all track channels) + crossfade + emit.
    /// Returns true when the packet's bits are exhausted.
    fn dct_packet(&mut self, gb: &mut LeBitReader, out: &mut Vec<f32>) -> Result<bool> {
        gb.bits(2)?; // use_dct: the 2 skipped bits
        let n = self.frame_len;
        let mut transformed = Vec::with_capacity(BLOCK_CHANNELS);
        for _ch in 0..self.channels {
            let mut coeffs = vec![0f32; n];
            self.parse_coeffs(gb, &mut coeffs)?;
            let mut plane = vec![0f32; n];
            self.dctiii(&coeffs, &mut plane);
            transformed.push(plane);
        }
        // overlap crossfade: out[i] = (prev*(count-j) + cur*j)/count,
        // j = ch + i*channels, count = overlap*channels (FFmpeg-exact,
        // float arithmetic in the same order).
        let count = (self.overlap_len * self.channels) as f32;
        for (ch, plane) in transformed.iter_mut().enumerate() {
            if !self.first {
                let mut j = ch as f32;
                for i in 0..self.overlap_len {
                    plane[i] = (self.previous[ch][i] * (count - j) + plane[i] * j) / count;
                    j += self.channels as f32;
                }
            }
            self.previous[ch].copy_from_slice(&plane[n - self.overlap_len..]);
        }
        self.first = false;
        // emit the first frame_len - overlap samples, interleaved
        let emit = n - self.overlap_len;
        for i in 0..emit {
            for plane in &transformed {
                out.push(plane[i]);
            }
        }
        gb.align32()?;
        Ok(gb.bits_left() == 0)
    }
}

// ---------------------------------------------------------------------------
// Unit tests — primitives + REAL packets vs real static-ffmpeg PCM
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    /// FFmpeg-CLI-style float -> s16 (lrintf + clip).
    fn to_s16(x: f32) -> i16 {
        let v = (x * 32768.0).round_ties_even();
        v.clamp(-32768.0, 32767.0) as i16
    }

    /// The band/quant setup at 48 kHz (the intro's tracks): 2048-sample
    /// frames, 128-sample overlap, 25 bands, and the FFmpeg-exact band
    /// boundaries `(crit[i-1] * frame_len / 24000) & !1`.
    #[test]
    fn setup_at_48khz_matches_ffmpeg() {
        let dec = BinkAudioDecoder::new(48000, true, true).unwrap();
        assert_eq!(dec.frame_len, 2048);
        assert_eq!(dec.overlap_len, 128);
        assert_eq!(dec.channels, 2);
        assert_eq!(dec.num_bands, 25);
        assert_eq!(
            dec.bands,
            vec![
                2, 8, 16, 24, 34, 42, 52, 64, 78, 92, 108, 126, 146, 170, 196, 230, 268, 314, 374,
                452, 546, 656, 810, 1024, 1322, 2048
            ]
        );
        let root = 2048.0f32.sqrt() / 32768.0;
        assert!((dec.root - root).abs() < 1e-9);
        assert!((dec.quant_table[0] - root).abs() < 1e-12);
        let q95 = root * (95.0f32 * QUANT_EXP).exp();
        assert!((dec.quant_table[95] - q95).abs() < 1e-9);
    }

    /// Mono tracks (the intro's tracks 1/2) construct with 1 channel
    /// and the same band setup.
    #[test]
    fn mono_setup() {
        let dec = BinkAudioDecoder::new(48000, false, true).unwrap();
        assert_eq!(dec.channels, 1);
        assert_eq!(dec.num_bands, 25);
        assert_eq!(dec.bands[1], 8);
    }

    /// RDFT tracks fail closed (the transform contract is an honest
    /// UNKNOWN — see the module docs; the block-60 census proves no
    /// shipped track reaches this path).
    #[test]
    fn rdft_fails_closed() {
        assert!(BinkAudioDecoder::new(48000, true, false).is_err());
    }

    /// The PROVEN transform contract, end to end: a crafted packet
    /// with one coefficient (value 4, band-0 quant = root) at position
    /// 2 must decode to `4*root*(2/2048)*cos(pi*(2k+1)*2/4096)` — the
    /// exact amplitude and kernel real ffmpeg produces (m52_probe).
    #[test]
    fn crafted_packet_transform_is_exact() {
        // bit writer (LSB-first)
        fn put(bits: &mut Vec<bool>, v: u32, n: usize) {
            for i in 0..n {
                bits.push((v >> i) & 1 != 0);
            }
        }
        fn write_block(bits: &mut Vec<bool>, bursts: &[(usize, u32)]) {
            put(bits, 0, 2); // use_dct skip
            for _ch in 0..2 {
                put(bits, 0, 5); // float: power 0
                put(bits, 0, 23); // mantissa 0
                put(bits, 0, 1); // sign 0
                put(bits, 0, 5);
                put(bits, 0, 23);
                put(bits, 0, 1);
                for _ in 0..25 {
                    put(bits, 0, 8); // quant index 0 -> q = root
                }
                put(bits, 0, 1); // no RLE -> 8 coefficients
                put(bits, 4, 4); // width 4
                for i in 2..10 {
                    if let Some((_, v)) = bursts.iter().find(|b| b.0 == i) {
                        put(bits, 4, 4); // coefficient value 4
                        put(bits, v & 1, 1); // sign
                    } else {
                        put(bits, 0, 4);
                    }
                }
                let mut i = 10;
                while i < 2048 {
                    put(bits, 1, 1); // RLE
                    put(bits, 14, 4); // rle_length_tab[14] = 32 -> 256
                    put(bits, 0, 4); // width 0
                    i += 256;
                }
            }
        }
        let mut bits: Vec<bool> = Vec::new();
        write_block(&mut bits, &[(2, 0)]);
        // (the analytic assertions below use only the low-k samples
        // where the sparse-input f32 FFT noise is ~1e-9; the DC weight
        // gets its own cancellation-free probe in the next test)
        while bits.len() % 32 != 0 {
            bits.push(false);
        }
        let mut packet = Vec::new();
        packet.extend_from_slice(&15360u32.to_le_bytes());
        let mut byte = 0u8;
        for (i, b) in bits.iter().enumerate() {
            if *b {
                byte |= 1 << (i & 7);
            }
            if i & 7 == 7 {
                packet.push(byte);
                byte = 0;
            }
        }
        let mut dec = BinkAudioDecoder::new(48000, true, true).unwrap();
        let pcm = dec.decode_packet(&packet).unwrap();
        assert_eq!(pcm.len(), 1920 * 2);
        for k in 0..8 {
            let expected = 4.0 * dec.root * (2.0 / 2048.0)
                * (std::f32::consts::PI * (2 * k + 1) as f32 / 4096.0).cos();
            // both channels carry the same burst (the writer applies it
            // to every channel of the block, like m52_probe)
            // 2e-8 absolute: the f32 FFT rounding floor on this SPARSE
            // input (|X[k]| ~ 5.5e-3 cancels to Re ~ 5.4e-6).  The s16
            // fixture tests pin the exact ffmpeg parity; this one pins
            // the kernel shape and the 2/n scale at good SNR.
            assert!(
                (pcm[2 * k] - expected).abs() < 2e-8,
                "ch0 k={k}: {} vs {expected}",
                pcm[2 * k]
            );
            assert!(
                (pcm[2 * k + 1] - expected).abs() < 2e-8,
                "ch1 k={k}: {} vs {expected}",
                pcm[2 * k + 1]
            );
        }
    }

    /// The DC weight probe (cancellation-free): a packet whose only
    /// nonzero coefficient is coeffs[0] = 1.0*root (mantissa 2^23,
    /// power 23) must decode to the CONSTANT root*2/2048 on every
    /// sample — the raw (undoubled) DC at full kernel weight, exactly
    /// as real ffmpeg renders it (the m52 DC finding).
    #[test]
    fn dc_weight_is_raw_and_full() {
        let mut bits: Vec<bool> = Vec::new();
        let put = |bits: &mut Vec<bool>, v: u32, n: usize| {
            for i in 0..n {
                bits.push((v >> i) & 1 != 0);
            }
        };
        put(&mut bits, 0, 2); // use_dct skip
        for _ch in 0..2 {
            // coeffs[0]: ldexp(2^22, 1 - 23) = 1.0 -> c0 = root
            // (the mantissa field is 23 bits: 2^23 would not fit)
            put(&mut bits, 1, 5); // power
            put(&mut bits, 1 << 22, 23); // mantissa 0x400000
            put(&mut bits, 0, 1); // positive
            // coeffs[1] = 0
            put(&mut bits, 0, 5);
            put(&mut bits, 0, 23);
            put(&mut bits, 0, 1);
            for _ in 0..25 {
                put(&mut bits, 0, 8); // q = root
            }
            // zeros from i=2 to 2048 (RLE runs)
            let mut i = 2;
            while i < 2048 {
                put(&mut bits, 1, 1);
                put(&mut bits, 14, 4);
                put(&mut bits, 0, 4);
                i += 256;
            }
        }
        while bits.len() % 32 != 0 {
            bits.push(false);
        }
        let mut packet = Vec::new();
        packet.extend_from_slice(&15360u32.to_le_bytes());
        let mut byte = 0u8;
        for (i, b) in bits.iter().enumerate() {
            if *b {
                byte |= 1 << (i & 7);
            }
            if i & 7 == 7 {
                packet.push(byte);
                byte = 0;
            }
        }
        let mut dec = BinkAudioDecoder::new(48000, true, true).unwrap();
        let pcm = dec.decode_packet(&packet).unwrap();
        let expected = dec.root * (2.0f64 / 2048.0) as f32;
        for (i, &s) in pcm.iter().enumerate() {
            assert!(
                (s - expected).abs() <= expected.abs() * 1e-5,
                "sample {i}: {} vs {expected}",
                s
            );
        }
    }

    /// REAL packet (intro frame 1, track 0, stereo): decodes to the
    /// real static-ffmpeg s16 PCM within 1 LSB per sample.
    #[test]
    fn stereo_packet_matches_ffmpeg_s16() {
        let packet = fixture("movies_bik_1e218528_f1_t0_packet.bin");
        let reference: Vec<i16> = fixture("movies_bik_1e218528_f1_t0_ref.s16")
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        let mut dec = BinkAudioDecoder::new(48000, true, true).unwrap();
        let pcm = dec.decode_packet(&packet).unwrap();
        assert_eq!(pcm.len(), 1920 * 2);
        let mut worst = 0i32;
        for (i, &s) in pcm.iter().enumerate() {
            let mine = to_s16(s) as i32;
            let r = reference[i] as i32;
            worst = worst.max((mine - r).abs());
        }
        assert!(worst <= 1, "s16 divergence {worst} LSB");
    }

    /// REAL packet (intro frame 1, track 1, MONO): same 1-LSB contract.
    #[test]
    fn mono_packet_matches_ffmpeg_s16() {
        let packet = fixture("movies_bik_1e218528_f1_t1_packet.bin");
        let reference: Vec<i16> = fixture("movies_bik_1e218528_f1_t1_ref.s16")
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        let mut dec = BinkAudioDecoder::new(48000, false, true).unwrap();
        let pcm = dec.decode_packet(&packet).unwrap();
        assert_eq!(pcm.len(), 1920);
        let mut worst = 0i32;
        for (i, &s) in pcm.iter().enumerate() {
            let d = (to_s16(s) as i32 - reference[i] as i32).abs();
            worst = worst.max(d);
        }
        assert!(worst <= 1, "s16 divergence {worst} LSB");
    }

    /// Truncated / empty packets fail closed.
    #[test]
    fn truncated_packets_fail_closed() {
        let mut dec = BinkAudioDecoder::new(48000, true, true).unwrap();
        assert!(dec.decode_packet(&[]).is_err());
        assert!(dec.decode_packet(&[0, 0, 0]).is_err());
        let full = fixture("movies_bik_1e218528_f1_t0_packet.bin");
        for cut in [8usize, 64, 256, full.len() - 8] {
            assert!(
                dec.decode_packet(&full[..cut]).is_err(),
                "truncated to {cut} B must fail"
            );
        }
        // the full packet still decodes after all the failures
        assert!(dec.decode_packet(&full).is_ok());
    }
}

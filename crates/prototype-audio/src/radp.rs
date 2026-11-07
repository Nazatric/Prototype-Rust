//! radp — the RADP audio codec: **standard IMA ADPCM** (SOLVED, M13).
//!
//! ## The evidence chain (all fixture-driven, see the tests below)
//!
//! The codec behind `boot_strap.p3d`'s 70 RADP payloads was UNKNOWN
//! until this module.  It was identified empirically and then
//! verified to 100.0000% on every real block:
//!
//! 1. **RTTI anchor** (context, not proof of algorithm):
//!    `prototypeenginef.dll` exports the classes
//!    `audio::RadicalAdpcmDecoder`, `audio::RadicalAdpcmFile`,
//!    `audio::RadicalAdpcmFileDecoder` (11 837-record RTTI dump,
//!    `_inventory/deep/rtti_engine_dll.txt`) — the engine ships an
//!    ADPCM decoder of its own.
//! 2. **Block structure** (PROVEN by size arithmetic): every one of
//!    the 70 payloads is an exact multiple of **20 bytes**; the
//!    smallest payload sizes are `20 × 1015` (odd block counts
//!    exist), so the block is 20 bytes, not 40.
//! 3. **Header fields** (PROVEN by a 191 295-block histogram): at
//!    every 20-byte boundary the first byte spans **exactly
//!    0..=88** — the IMA step-index table range — the second byte
//!    is **always 0**, and bytes 2..4 form a little-endian `i16`.
//!    So: `u8 step_index, u8 reserved(=0), i16 predictor_sample`.
//! 4. **Codec identification** (PROVEN by decode continuity):
//!    decoding the 16 data bytes as 32 low-nibble-first IMA
//!    nibbles (standard Intel/DVI step table + index adjust),
//!    starting each block at its header sample, reproduces every
//!    subsequent block's header sample from the previous block's
//!    last decoded sample in **84 663/84 663 v1 blocks
//!    (100.0000%)**.  High-nibble-first matches only 4.9% (chance).
//!    A wrong codec cannot fake block-to-block sample continuity
//!    across 84 thousand independent encodes.
//! 5. **radp_version 2 = two interleaved mono streams** (PROVEN the
//!    same way): sequential continuity fails (1.2%) but **both
//!    parities are 53 246/53 246 (100%)** — even blocks chain into
//!    one stream, odd blocks into a second, each with its own
//!    step-index + predictor.  Interpreted as stereo (LIKELY —
//!    dual-mono duplicates cannot be ruled out from structure
//!    alone).
//! 6. Silence encodes as zero nibbles at index 0 (step 7, delta
//!    `7 >> 3 == 0`) — the 1 214 fully-zero 20-byte blocks decode
//!    to perfectly flat samples, exactly as IMA behaves.
//! 7. **radp_version 6 = SIX interleaved mono streams — the NIS mix
//!    dialect** (block 62, PROVEN against two real members of the
//!    release audio archives, `audio\english\AudioFile\` of
//!    `01audio.rcf`): the two probed NIS mix P3Ds each carry ONE
//!    AudioFile clip whose `radp_version` is 6 (the engine's other
//!    shipped clips are only v1/v2; the remaining 33 NIS mixes are
//!    LIKELY the same dialect by family and naming, unprobed).
//!    Evidence, all measured on the
//!    real members (`mnis_5_2_1_mix.p3d` — committed fixture,
//!    912 117 B; `nis7-3a_mix.p3d` — 6 966 231 B, probe-only):
//!    * the payloads are exact multiples of the same 20-byte block
//!      (912 000 and 6 966 120 data bytes) and the block counts
//!      divide by 6 EXACTLY (45 600 = 6 × 7 600; 348 306 = 6 ×
//!      58 051);
//!    * per-stream block-to-block header continuity (the same proof
//!      shape that made v1/v2 PROVEN) holds EXACTLY for the loud
//!      member's streams 4/5 (57 750/57 769 = 99.97 %, 57 786/57 786
//!      = 100.00 %) and holds within ±1–4 LSB on streams 0–2 with a
//!      clean decaying drift histogram (98.8 %/97.3 %/99.8 % of
//!      nonzero pairs within ±8) — encoder-predictor drift, NOT an
//!      assignment error: wrong assignments (chunked-6, round-robin
//!      2/3/4/8/12) fail at 20–61 % with deltas orders of magnitude
//!      larger, and each block re-seeds its predictor from its own
//!      header so the decoded audio is unaffected by the drift;
//!    * cross-stream correlation pins the pair structure: streams
//!      0/1 correlate at +0.776 (the stereo pair), stream 2 is
//!      uncorrelated content, and some streams ship ENTIRELY silent
//!      (all-zero blocks — stream 3 of `nis7-3a`, streams 3/4/5 of
//!      `mnis_5_2_1`): 5.1-style masters with unused channels.
//!    * UNKNOWN (honest): which stream index maps to which speaker
//!      (channel order); the decoder exposes all six in block order
//!      and documents no speaker semantics.
//! 8. **radp_version 4 / 18 — and the walk is UNIVERSAL** (block 64,
//!    PROVEN against the four real release members fetched whole by
//!    roster span: `base_interior_ambience_4ch.p3d` 10 895 741 B,
//!    `e10m04_ambience_4ch.p3d` 13 089 087 B,
//!    `ui_fe_woi_bg_quad_loop.p3d` 6 901 973 B (all v4 — the last is
//!    the committed fixture) and `ambience_baked_18ch.p3d` 60 820 687 B
//!    (v18 — its first 40 RADP frames are the committed prefix
//!    fixture)): block `i` belongs to stream `i % ver` — the SAME
//!    walk as v2/v6, no per-version dialect.  Evidence: per-stream
//!    block-to-block header continuity holds at 100.00 % EXACT on a
//!    16 384-frame probe of every member while header-grouped frames,
//!    planar, headers-table and eleven chunked interleaves all fail
//!    at chance (0–10 %); the mod-(20·ver) header-position scan puts
//!    exactly `blocks/stream` marks on each of the ver round-robin
//!    residues (136 195/163 612/86 273 at residues 0/20/40/60 for the
//!    v4s; 168 946 at each of the 18 residues for v18).  The
//!    block-63 "beds/mixes fail i%ver" reading was a harness
//!    artifact: re-probed with the corrected harness, the v2 bed
//!    `e04m04_complete01.p3d` (5 133 blocks/stream) and the v6 mix
//!    `mnis_5_2_2_mix.p3d` (12 551) ALSO hold 100.00 % exact — every
//!    shipped multi-stream version (2, 4, 6, 18; the 14 572-member
//!    census has no others) uses the one interleave, so the decoder
//!    is one [`decode_multi`] instead of per-version dialects.
//!
//! ## What the header's `format_tag` is
//!
//! Still UNKNOWN.  It does NOT correlate with sample_rate or
//! radp_version (tag 0 appears at both rates and both versions).
//! The DLL's RTTI lists a `MetaEnum W4WAVEFORMAT@audio@@`
//! enumeration — `format_tag` plausibly indexes it, but that is
//! UNVERIFIED.  The decode does not depend on it.
//!
//! ## Scope / legal
//!
//! The step and index tables below are the published Intel/DVI IMA
//! ADPCM constants (also standardised in IMA's ADPCM spec and
//! Microsoft's `imaadp32.acm`); they are not Radical's code.  The
//! 20-byte Radical block header layout is this project's empirical
//! finding.

#![forbid(unsafe_code)]

// ---------------------------------------------------------------------------
// IMA ADPCM constants (published Intel/DVI spec values)
// ---------------------------------------------------------------------------

/// The standard IMA/DVI ADPCM step-size table (89 entries, 7..32767).
pub const IMA_STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41,
    45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190,
    209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724,
    796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132,
    7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500,
    20350, 22385, 24623, 27086, 29794, 32767,
];

/// The standard IMA nibble → index-adjust table.
pub const IMA_INDEX_TABLE: [i32; 16] = [
    -1, -1, -1, -1, 2, 4, 6, 8, // low nibble rows
    -1, -1, -1, -1, 2, 4, 6, 8, // high nibble rows
];

/// RADP block size in bytes (PROVEN: every payload is a multiple).
pub const BLOCK_SIZE: usize = 20;
/// Data bytes per block (16 → 32 nibbles → 32 samples).
pub const DATA_BYTES_PER_BLOCK: usize = 16;
/// PCM samples decoded per block, per channel.
pub const SAMPLES_PER_BLOCK: usize = 32;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Decode failures.  The decoder is strict: real RADP payloads
/// satisfy every one of these invariants (asserted against the 70
/// shipped clips), so a violation means the input is not RADP as
/// shipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RadpError {
    /// Payload length is not a multiple of [`BLOCK_SIZE`].
    NotBlockAligned { len: usize },
    /// A block's step_index exceeded the IMA table range (0..=88).
    StepIndexOutOfRange { index: u8, block: usize },
    /// `radp_version` other than the shipped set 1 (mono), 2 (stereo),
    /// 4 (quad bed), 6 (the NIS mix dialect) or 18 (the baked ambience
    /// matrix) — the 14 572-member census has no other.
    UnknownVersion { version: u32 },
    /// A stereo payload had zero blocks.
    Empty,
}

impl std::fmt::Display for RadpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RadpError::NotBlockAligned { len } => {
                write!(f, "RADP payload length {len} is not a multiple of {BLOCK_SIZE}")
            }
            RadpError::StepIndexOutOfRange { index, block } => {
                write!(f, "RADP block {block}: step_index {index} > 88 (IMA range)")
            }
            RadpError::UnknownVersion { version } => {
                write!(f, "RADP version {version} not in {{1, 2, 6}}")
            }
            RadpError::Empty => write!(f, "RADP payload has no blocks"),
        }
    }
}

impl std::error::Error for RadpError {}

// ---------------------------------------------------------------------------
// Block header
// ---------------------------------------------------------------------------

/// One 20-byte RADP block header (PROVEN layout — see module docs):
/// `u8 step_index, u8 reserved, i16 predictor_sample`, then
/// [`DATA_BYTES_PER_BLOCK`] bytes of packed low-nibble-first IMA
/// nibbles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RadpBlockHeader {
    /// IMA step-table index (0..=88; histogram of all 191 295 real
    /// blocks never leaves that range).
    pub step_index: u8,
    /// Always 0 in every observed block (191 295/191 295).  The
    /// decoder does not depend on it.
    pub reserved: u8,
    /// The block's starting PCM sample.  For blocks after the
    /// first, this equals the previous block's last decoded sample
    /// (100.0000% on the shipped fixture — the decoder honours it
    /// as the block start, which also makes blocks seekable).
    pub sample: i16,
}

impl RadpBlockHeader {
    /// Parse the header from the first 4 bytes of a block.
    pub fn parse(block: &[u8]) -> RadpBlockHeader {
        RadpBlockHeader {
            step_index: block[0],
            reserved: block[1],
            sample: i16::from_le_bytes([block[2], block[3]]),
        }
    }
}

// ---------------------------------------------------------------------------
// Decoded audio
// ---------------------------------------------------------------------------

/// Decoded PCM: interleaved samples + channel count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedAudio {
    /// 1 (radp_version 1), 2 (radp_version 2) or 6 (radp_version 6,
    /// the NIS mix dialect).
    pub channels: u32,
    /// Interleaved PCM (L,R,L,R… for stereo; plain samples for mono).
    pub samples: Vec<i16>,
}

impl DecodedAudio {
    /// Duration in seconds at `sample_rate` Hz.
    pub fn duration_seconds(&self, sample_rate: u32) -> f32 {
        self.samples.len() as f32 / (sample_rate as f32 * self.channels.max(1) as f32)
    }

    /// Peak absolute amplitude.
    pub fn peak(&self) -> i32 {
        self.samples.iter().map(|s| s.unsigned_abs() as i32).max().unwrap_or(0)
    }

    /// Root-mean-square amplitude (i32 to keep the math exact for
    /// tests).
    pub fn rms(&self) -> i64 {
        if self.samples.is_empty() {
            return 0;
        }
        let sum_sq: i64 = self
            .samples
            .iter()
            .map(|s| *s as i64 * *s as i64)
            .sum();
        (sum_sq as f64 / self.samples.len() as f64).sqrt() as i64
    }
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

/// Decode one RADP payload (`radp_version` from the AudioFile
/// chunk's RADP block) to interleaved PCM.
///
/// * version 1 → mono: blocks decoded sequentially.
/// * version 2 → stereo: even-indexed blocks are one channel, odd
///   blocks the other (PROVEN by 100% parity continuity — see
///   module docs); output interleaves L,R per sample frame.  An
///   odd block count gives the extra block to channel 0.
/// * versions 4, 6, 18 → that many channels via the UNIVERSAL
///   multi-stream walk ([`decode_multi`], PROVEN for every shipped
///   multi-stream version — see the module docs' v4/v18 evidence):
///   block `i` belongs to stream `i % version`; output interleaves
///   the streams per sample frame.
pub fn decode(version: u32, payload: &[u8]) -> Result<DecodedAudio, RadpError> {
    match version {
        1 => Ok(DecodedAudio { channels: 1, samples: decode_mono(payload)? }),
        2 => {
            let (l, r) = decode_stereo(payload)?;
            // Interleave L,R per frame; if the block count is odd,
            // channel 0 has one extra block — preserve its tail
            // instead of zip-dropping it.
            let mut samples = Vec::with_capacity(l.len() + r.len());
            let mut li = l.iter();
            let mut ri = r.iter();
            loop {
                match (li.next(), ri.next()) {
                    (Some(a), Some(b)) => {
                        samples.push(*a);
                        samples.push(*b);
                    }
                    (Some(a), None) => samples.push(*a),
                    (None, Some(b)) => samples.push(*b),
                    (None, None) => break,
                }
            }
            Ok(DecodedAudio { channels: 2, samples })
        }
        4 | 6 | 18 => {
            let streams = decode_multi(payload, version as usize)?;
            let frames = streams[0].len();
            let mut samples = Vec::with_capacity(frames * version as usize);
            for f in 0..frames {
                for ch in &streams {
                    samples.push(ch[f]);
                }
            }
            Ok(DecodedAudio { channels: version, samples })
        }
        v => Err(RadpError::UnknownVersion { version: v }),
    }
}

/// Decode a 6-channel (radp_version 6) payload — the **NIS mix
/// dialect** (block 62, PROVEN against the two real release members
/// `mnis_5_2_1_mix.p3d` and `nis7-3a_mix.p3d`): the payload is SIX
/// interleaved mono streams at the [`BLOCK_SIZE`] grain — block `i`
/// belongs to stream `i % 6` — each stream a self-seeding IMA chain
/// exactly like [`decode_mono`].
///
/// Evidence (see the module docs): block counts divide by 6 exactly
/// in both real members (45 600 = 6 × 7 600; 348 306 = 6 × 58 051);
/// the per-stream block-to-block header continuity holds exactly
/// where content is loud (measured 57 750/57 769 = 99.97 % and
/// 57 786/57 786 = 100.00 % on the loud member's streams 4/5) and
/// the residual deltas on streams 0–2 are ±1–4 LSB with a clean
/// decaying histogram (98.8 % / 97.3 % / 99.8 % within ±8) —
/// encoder-predictor drift, not a stream-assignment error (wrong
/// assignments fail by orders of magnitude).  Every block re-seeds
/// its predictor from its own header, so the decoded audio is
/// unaffected by that drift.
///
/// Streams may differ by at most one block (when the block count is
/// not a multiple of 6 the earliest streams own the extra).  The
/// shorter streams are padded with silence to the longest so the
/// result frame-interleaves cleanly (DESIGN — the shipped members
/// always divide exactly; the padding only defines the tail of a
/// hypothetical ragged member).
///
/// A thin wrapper over [`decode_multi`] (the block-64 generalization
/// that made the walk universal); kept for the block-62 API and its
/// tests.
pub fn decode_six(payload: &[u8]) -> Result<[Vec<i16>; 6], RadpError> {
    let mut v = decode_multi(payload, 6)?;
    let mut streams: [Vec<i16>; 6] = Default::default();
    for (i, s) in v.drain(..).enumerate() {
        streams[i] = s;
    }
    Ok(streams)
}

/// Decode an N-stream interleaved payload — the UNIVERSAL multi-stream
/// walk (block 64, PROVEN for every shipped multi-stream version:
/// N = 2 beds, 4 quad beds, 6 NIS mixes, 18 the baked ambience matrix
/// — see the module docs): block `i` belongs to stream `i % streams`,
/// each stream a self-seeding IMA chain exactly like [`decode_mono`].
///
/// Evidence shape (identical to the v1/v2/v6 proofs): within each
/// stream, consecutive blocks' header samples track the previous
/// block's last decoded sample at 100.00 % EXACT (16 384-frame probes
/// of the four v4/v18 members plus the re-probed v2/v6 beds/mixes),
/// while every alternative layout fails at chance.
///
/// Streams may differ by at most one block (when the block count is
/// not a multiple of `streams` the earliest streams own the extra);
/// the shorter streams are padded with silence to the longest (DESIGN
/// — every shipped member divides exactly).  `streams` of 0 is
/// [`RadpError::Empty`].
pub fn decode_multi(payload: &[u8], streams: usize) -> Result<Vec<Vec<i16>>, RadpError> {
    if streams == 0 || payload.is_empty() {
        return Err(RadpError::Empty);
    }
    if payload.len() % BLOCK_SIZE != 0 {
        return Err(RadpError::NotBlockAligned { len: payload.len() });
    }
    let mut out: Vec<Vec<i16>> = (0..streams).map(|_| Vec::new()).collect();
    for (bi, block) in payload.chunks_exact(BLOCK_SIZE).enumerate() {
        let h = RadpBlockHeader::parse(block);
        if h.step_index as usize >= IMA_STEP_TABLE.len() {
            return Err(RadpError::StepIndexOutOfRange { index: h.step_index, block: bi });
        }
        decode_block_data(h, &block[4..], &mut out[bi % streams]);
    }
    let longest = out.iter().map(Vec::len).max().unwrap_or(0);
    for s in &mut out {
        s.resize(longest, 0);
    }
    Ok(out)
}

/// Decode a mono (radp_version 1) payload: `payload.len()` must be
/// a multiple of [`BLOCK_SIZE`]; each block yields
/// [`SAMPLES_PER_BLOCK`] samples.
pub fn decode_mono(payload: &[u8]) -> Result<Vec<i16>, RadpError> {
    if payload.len() % BLOCK_SIZE != 0 {
        return Err(RadpError::NotBlockAligned { len: payload.len() });
    }
    let mut out = Vec::with_capacity(payload.len() / BLOCK_SIZE * SAMPLES_PER_BLOCK);
    for (bi, block) in payload.chunks_exact(BLOCK_SIZE).enumerate() {
        let h = RadpBlockHeader::parse(block);
        if h.step_index as usize >= IMA_STEP_TABLE.len() {
            return Err(RadpError::StepIndexOutOfRange { index: h.step_index, block: bi });
        }
        decode_block_data(h, &block[4..], &mut out);
    }
    Ok(out)
}

/// Decode a stereo (radp_version 2) payload: even blocks are
/// channel 0, odd blocks channel 1.  Returns `(channel0, channel1)`.
pub fn decode_stereo(payload: &[u8]) -> Result<(Vec<i16>, Vec<i16>), RadpError> {
    if payload.len() % BLOCK_SIZE != 0 {
        return Err(RadpError::NotBlockAligned { len: payload.len() });
    }
    if payload.is_empty() {
        return Err(RadpError::Empty);
    }
    let n_blocks = payload.len() / BLOCK_SIZE;
    let mut c0 = Vec::with_capacity((n_blocks + 1) / 2 * SAMPLES_PER_BLOCK);
    let mut c1 = Vec::with_capacity(n_blocks / 2 * SAMPLES_PER_BLOCK);
    for (bi, block) in payload.chunks_exact(BLOCK_SIZE).enumerate() {
        let h = RadpBlockHeader::parse(block);
        if h.step_index as usize >= IMA_STEP_TABLE.len() {
            return Err(RadpError::StepIndexOutOfRange { index: h.step_index, block: bi });
        }
        let dst = if bi % 2 == 0 { &mut c0 } else { &mut c1 };
        decode_block_data(h, &block[4..], dst);
    }
    Ok((c0, c1))
}

/// Decode the 16 data bytes of one block (32 low-nibble-first IMA
/// nibbles), starting at the header's sample, appending
/// [`SAMPLES_PER_BLOCK`] samples to `out`.
fn decode_block_data(header: RadpBlockHeader, data: &[u8], out: &mut Vec<i16>) {
    let mut sample = header.sample as i32;
    let mut index = header.step_index as i32;
    let mut step = IMA_STEP_TABLE[index as usize];
    debug_assert_eq!(data.len(), DATA_BYTES_PER_BLOCK);
    for &byte in data {
        // Low nibble first (PROVEN by 100% continuity; high-first
        // matches only chance-level).
        for nib in [byte & 0x0F, byte >> 4] {
            // Standard IMA difference computation:
            // diff = step/8 + (bit0 ? step/4) + (bit1 ? step/2) +
            //        (bit2 ? step), negated when bit3 set.
            let mut diff = step >> 3;
            if nib & 1 != 0 { diff += step >> 2; }
            if nib & 2 != 0 { diff += step >> 1; }
            if nib & 4 != 0 { diff += step; }
            if nib & 8 != 0 { diff = -diff; }
            sample += diff;
            if sample > 32767 { sample = 32767; }
            else if sample < -32768 { sample = -32768; }
            index = (index + IMA_INDEX_TABLE[nib as usize]).clamp(0, 88);
            step = IMA_STEP_TABLE[index as usize];
            out.push(sample as i16);
        }
    }
}

// ---------------------------------------------------------------------------
// WAV encoder (16-bit PCM RIFF) — for playing decoded clips
// ---------------------------------------------------------------------------

/// Encode interleaved 16-bit PCM as a RIFF/WAVE file
/// (`format_tag = 1`, PCM).  Returns the full `.wav` bytes.
pub fn encode_wav16(samples: &[i16], channels: u32, sample_rate: u32) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let block_align = channels * 2;
    let byte_rate = sample_rate * block_align;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());          // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes());           // PCM
    out.extend_from_slice(&(channels as u16).to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&(block_align as u16).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());          // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

// ---------------------------------------------------------------------------
// Tests — fixture-driven (the REAL 70 RADP payloads)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn boot_strap() -> Vec<u8> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join("boot_strap.p3d");
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    /// Walk top-level P3D chunks; return (name, version, rate, tag,
    /// payload) for every AudioFile chunk.
    fn all_clips(data: &[u8]) -> Vec<(String, u32, u32, u32, Vec<u8>)> {
        let mut out = Vec::new();
        let mut off = 12usize; // file header
        while off + 12 <= data.len() {
            let (ctype, _uid, size) =
                (u32::from_le_bytes(data[off..off+4].try_into().unwrap()),
                 u32::from_le_bytes(data[off+4..off+8].try_into().unwrap()),
                 u32::from_le_bytes(data[off+8..off+12].try_into().unwrap()));
            let size = size as usize;
            if size < 12 || off + size > data.len() {
                break;
            }
            if ctype == 0xFE00_0000 {
                if let Some(clip) = classic_audio(data, off, size) {
                    out.push(clip);
                }
            }
            off += size;
        }
        out
    }

    /// Parse a classic chunk's AudioFile payload (name list + RADP).
    fn classic_audio(data: &[u8], off: usize, size: usize) -> Option<(String, u32, u32, u32, Vec<u8>)> {
        let end = off + size;
        let mut p = off + 12; // skip type/uid/size
        let (_id, namelen) = (
            u32::from_le_bytes(data[p..p+4].try_into().unwrap()),
            u32::from_le_bytes(data[p+4..p+8].try_into().unwrap()),
        );
        p += 8;
        let cname = String::from_utf8_lossy(&data[p..p + namelen as usize]).to_string();
        if cname != "AudioFile" {
            return None;
        }
        p += namelen as usize;
        let _zero = data[p]; p += 1;
        let count = u32::from_le_bytes(data[p..p+4].try_into().unwrap()) as usize; p += 4;
        let mut names = Vec::new();
        for _ in 0..count {
            let nlen = u32::from_le_bytes(data[p..p+4].try_into().unwrap()) as usize; p += 4;
            names.push(String::from_utf8_lossy(&data[p..p+nlen]).to_string());
            p += nlen;
            if p < end && data[p] == 0 { p += 1; }
        }
        let _zero2 = u32::from_le_bytes(data[p..p+4].try_into().unwrap()); p += 4;
        let radplen = u32::from_le_bytes(data[p..p+4].try_into().unwrap()) as usize; p += 4;
        assert_eq!(&data[p..p + radplen], b"radp");
        p += radplen;
        assert_eq!(data[p], 0); p += 1;
        assert_eq!(&data[p..p+4], b"RADP"); p += 4;
        let (version, rate, tag, dsize) = (
            u32::from_le_bytes(data[p..p+4].try_into().unwrap()),
            u32::from_le_bytes(data[p+4..p+8].try_into().unwrap()),
            u32::from_le_bytes(data[p+8..p+12].try_into().unwrap()),
            u32::from_le_bytes(data[p+12..p+16].try_into().unwrap()),
        );
        p += 16;
        assert_eq!(p + dsize as usize, end, "payload must exactly cover the chunk");
        Some((names[0].clone(), version, rate, tag, data[p..end].to_vec()))
    }

    #[test]
    fn ima_step_table_is_the_published_intel_dvi_table() {
        assert_eq!(IMA_STEP_TABLE.len(), 89);
        assert_eq!(IMA_STEP_TABLE[0], 7);
        assert_eq!(IMA_STEP_TABLE[88], 32767);
        // Monotonically non-decreasing.
        for w in IMA_STEP_TABLE.windows(2) {
            assert!(w[0] <= w[1]);
        }
        assert_eq!(IMA_INDEX_TABLE.len(), 16);
    }

    #[test]
    fn all_70_clips_block_structure_holds() {
        let data = boot_strap();
        let clips = all_clips(&data);
        assert_eq!(clips.len(), 70, "the shipped fixture has 70 AudioFile clips");

        let mut total_blocks = 0usize;
        for (name, version, rate, _tag, payload) in &clips {
            assert_eq!(payload.len() % BLOCK_SIZE, 0, "{name}: not block-aligned");
            assert!(*version == 1 || *version == 2, "{name}: version");
            assert!(*rate == 44100 || *rate == 48000, "{name}: rate");
            for (bi, block) in payload.chunks_exact(BLOCK_SIZE).enumerate() {
                let h = RadpBlockHeader::parse(block);
                assert_eq!(h.reserved, 0, "{name} block {bi}: reserved byte");
                assert!(
                    (h.step_index as usize) < IMA_STEP_TABLE.len(),
                    "{name} block {bi}: step index {} out of IMA range",
                    h.step_index
                );
                total_blocks += 1;
            }
        }
        // Histogram total over all 70 payloads (python cross-check:
        // 191 295 blocks).
        assert_eq!(total_blocks, 191_295);
    }

    /// THE codec proof: decode every v1 clip block-by-block and
    /// require each block's header sample to equal the previous
    /// block's last decoded sample — 84 663/84 663 (100.0000%).
    #[test]
    fn radp_v1_continuity_is_100_percent_on_all_59_clips() {
        let data = boot_strap();
        let clips = all_clips(&data);
        let v1: Vec<_> = clips.iter().filter(|(_, v, ..)| *v == 1).collect();
        assert_eq!(v1.len(), 59);

        let mut checks = 0usize;
        for (name, _, _, _, payload) in &v1 {
            let blocks: Vec<&[u8]> = payload.chunks_exact(BLOCK_SIZE).collect();
            let mut prev_last: Option<i16> = None;
            for (bi, block) in blocks.iter().enumerate() {
                let h = RadpBlockHeader::parse(block);
                if let Some(prev) = prev_last {
                    assert_eq!(
                        h.sample, prev,
                        "{name} block {bi}: header sample {0} != previous last decoded {1}",
                        h.sample, prev
                    );
                    checks += 1;
                }
                let mut out = Vec::with_capacity(SAMPLES_PER_BLOCK);
                decode_block_data(h, &block[4..], &mut out);
                assert_eq!(out.len(), SAMPLES_PER_BLOCK);
                prev_last = Some(*out.last().unwrap());
            }
        }
        // 84 722 = (blocks − 1) per clip summed over all 59 v1
        // clips.  (The first python cross-check counted 84 663
        // because its probe skipped decoding block 0 and therefore
        // missed the block1↔block0 pair per clip — the Rust check
        // is stricter by one pair per clip, and ALL of them pass.)
        assert_eq!(checks, 84_722, "v1 continuity checks");
    }

    /// The stereo proof: v2 payloads' even blocks chain into one
    /// stream and odd blocks into another — 53 246/53 246 per
    /// channel (100.0000%), while sequential chaining fails.
    #[test]
    fn radp_v2_stereo_parity_continuity_is_100_percent_on_all_11_clips() {
        let data = boot_strap();
        let clips = all_clips(&data);
        let v2: Vec<_> = clips.iter().filter(|(_, v, ..)| *v == 2).collect();
        assert_eq!(v2.len(), 11);

        let mut checks = [0usize; 2];
        for (name, _, _, _, payload) in &v2 {
            let blocks: Vec<&[u8]> = payload.chunks_exact(BLOCK_SIZE).collect();
            assert!(blocks.len() >= 2, "{name}: stereo needs 2+ blocks");
            for parity in 0..2 {
                let mut prev_last: Option<i16> = None;
                for block in blocks.iter().skip(parity).step_by(2) {
                    let h = RadpBlockHeader::parse(block);
                    if let Some(prev) = prev_last {
                        assert_eq!(
                            h.sample, prev,
                            "{name} parity {parity}: header sample {} != previous last {}",
                            h.sample, prev
                        );
                        checks[parity] += 1;
                    }
                    let mut out = Vec::with_capacity(SAMPLES_PER_BLOCK);
                    decode_block_data(h, &block[4..], &mut out);
                    prev_last = Some(*out.last().unwrap());
                }
            }
        }
        assert_eq!(checks, [53_246, 53_246], "per-channel continuity checks");
    }

    #[test]
    fn decode_all_70_clips_and_sanity_check_blob_collect_01() {
        let data = boot_strap();
        let clips = all_clips(&data);
        let mut decoded = 0usize;
        let mut total_samples = 0usize;
        for (name, version, rate, _tag, payload) in &clips {
            let audio = decode(*version, payload)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(audio.channels as u32, if *version == 1 { 1 } else { 2 });
            // Frame count: every 20-byte block decodes to 32
            // samples per channel; the interleaved total is
            // n_blocks × 32 regardless of version (mono: n blocks
            // in one stream; stereo: n blocks SPLIT across two
            // streams — the sum is the same).
            let frames = payload.len() / BLOCK_SIZE * SAMPLES_PER_BLOCK;
            assert_eq!(audio.samples.len(), frames, "{name}");
            total_samples += audio.samples.len();
            decoded += 1;

            if name == "blob_collect_01" {
                assert_eq!(audio.samples.len(), 100_128);
                assert!((audio.duration_seconds(*rate) - 2.086).abs() < 0.001);
                assert_eq!(audio.peak(), 32_768);
                let rms = audio.rms();
                assert!((7700..=7800).contains(&rms), "rms {rms}");
            }
        }
        assert_eq!(decoded, 70);
        assert_eq!(total_samples, 191_295 * SAMPLES_PER_BLOCK);
    }

    #[test]
    fn silence_decodes_to_perfectly_flat_samples() {
        // The 1 214 fully-zero blocks in the fixture must decode to
        // a flat line: at index 0, step 7, delta = 7 >> 3 == 0.
        let zero_block = [0u8; BLOCK_SIZE];
        let mut out = Vec::new();
        decode_block_data(RadpBlockHeader::parse(&zero_block), &zero_block[4..], &mut out);
        assert_eq!(out.len(), SAMPLES_PER_BLOCK);
        assert!(out.iter().all(|s| *s == 0));
        // And a NON-zero header sample with zero nibbles stays put.
        let mut block = [0u8; BLOCK_SIZE];
        block[2..4].copy_from_slice(&1000i16.to_le_bytes());
        let mut out = Vec::new();
        decode_block_data(RadpBlockHeader::parse(&block), &block[4..], &mut out);
        assert!(out.iter().all(|s| *s == 1000));
    }

    #[test]
    fn radp_rejects_malformed_inputs() {
        // Unaligned payload.
        assert_eq!(
            decode_mono(&vec![0u8; BLOCK_SIZE + 1]),
            Err(RadpError::NotBlockAligned { len: BLOCK_SIZE + 1 })
        );
        // Step index beyond the IMA table.
        let mut bad = [0u8; BLOCK_SIZE];
        bad[0] = 89;
        assert_eq!(
            decode_mono(&bad),
            Err(RadpError::StepIndexOutOfRange { index: 89, block: 0 })
        );
        // Unknown version.
        assert_eq!(
            decode(3, &[0u8; BLOCK_SIZE]),
            Err(RadpError::UnknownVersion { version: 3 })
        );
        // Empty stereo payload.
        assert_eq!(decode_stereo(&[]), Err(RadpError::Empty));
    }

    #[test]
    fn stereo_decode_splits_channels_and_interleaves() {
        // Two blocks: even → ch0 (header sample 100), odd → ch1
        // (header sample -100).  Zero nibbles keep them flat.
        let mut payload = Vec::new();
        let mut b0 = [0u8; BLOCK_SIZE];
        b0[2..4].copy_from_slice(&100i16.to_le_bytes());
        let mut b1 = [0u8; BLOCK_SIZE];
        b1[2..4].copy_from_slice(&(-100i16).to_le_bytes());
        payload.extend_from_slice(&b0);
        payload.extend_from_slice(&b1);

        let (l, r) = decode_stereo(&payload).unwrap();
        assert_eq!(l.len(), SAMPLES_PER_BLOCK);
        assert_eq!(r.len(), SAMPLES_PER_BLOCK);
        assert!(l.iter().all(|s| *s == 100));
        assert!(r.iter().all(|s| *s == -100));

        let decoded = decode(2, &payload).unwrap();
        assert_eq!(decoded.channels, 2);
        assert_eq!(decoded.samples.len(), SAMPLES_PER_BLOCK * 2);
        assert_eq!(decoded.samples[0], 100);
        assert_eq!(decoded.samples[1], -100);
        assert_eq!(decoded.samples[2], 100);
    }

    #[test]
    fn odd_block_count_gives_the_extra_block_to_channel0() {
        let mut payload = vec![0u8; BLOCK_SIZE * 3]; // 3 blocks
        payload[2..4].copy_from_slice(&7i16.to_le_bytes()); // block 0 (ch0)
        payload[BLOCK_SIZE + 2..BLOCK_SIZE + 4].copy_from_slice(&9i16.to_le_bytes()); // block 1 (ch1)
        payload[2 * BLOCK_SIZE + 2..2 * BLOCK_SIZE + 4].copy_from_slice(&11i16.to_le_bytes()); // block 2 (ch0)
        let (l, r) = decode_stereo(&payload).unwrap();
        assert_eq!(l.len(), 2 * SAMPLES_PER_BLOCK);
        assert_eq!(r.len(), SAMPLES_PER_BLOCK);
        assert!(l.iter().all(|s| *s == 7 || *s == 11));
        assert!(r.iter().all(|s| *s == 9));
    }

    #[test]
    fn wav_encoder_produces_a_parseable_16bit_pcm_file() {
        let pcm: Vec<i16> = (0..100).map(|i| (i as i16) * 3).collect();
        let wav = encode_wav16(&pcm, 2, 44100);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        let fmt_len = u32::from_le_bytes(wav[16..20].try_into().unwrap());
        assert_eq!(fmt_len, 16);
        let tag = u16::from_le_bytes(wav[20..22].try_into().unwrap());
        assert_eq!(tag, 1, "PCM");
        let ch = u16::from_le_bytes(wav[22..24].try_into().unwrap());
        assert_eq!(ch, 2);
        let rate = u32::from_le_bytes(wav[24..28].try_into().unwrap());
        assert_eq!(rate, 44100);
        let byte_rate = u32::from_le_bytes(wav[28..32].try_into().unwrap());
        assert_eq!(byte_rate, 44100 * 2 * 2);
        let align = u16::from_le_bytes(wav[32..34].try_into().unwrap());
        assert_eq!(align, 4);
        let bits = u16::from_le_bytes(wav[34..36].try_into().unwrap());
        assert_eq!(bits, 16);
        assert_eq!(&wav[36..40], b"data");
        let data_len = u32::from_le_bytes(wav[40..44].try_into().unwrap());
        assert_eq!(data_len, 200);
        assert_eq!(wav.len(), 44 + 200);
        let riff_len = u32::from_le_bytes(wav[4..8].try_into().unwrap());
        assert_eq!(riff_len, 36 + 200);
        // Payload round-trips.
        for (i, s) in pcm.iter().enumerate() {
            let got = i16::from_le_bytes(wav[44 + i * 2..44 + i * 2 + 2].try_into().unwrap());
            assert_eq!(got, *s);
        }
    }

    // -------------------------------------------------------------------
    // Block 62: RADP v6 — the NIS mix dialect (six interleaved mono
    // streams).  Fixture: the REAL release member
    // audio\english\AudioFile\mnis_5_2_1_mix.p3d of 01audio.rcf,
    // fetched by its roster span via a surgical Range read.
    // -------------------------------------------------------------------

    fn nis_mix() -> Vec<u8> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join("audio_member_mnis_5_2_1_mix.p3d");
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    #[test]
    fn nis_mix_v6_carries_one_six_channel_clip() {
        let data = nis_mix();
        let clips = all_clips(&data);
        assert_eq!(clips.len(), 1, "a NIS mix member is a one-clip P3D");
        let (name, version, rate, _tag, payload) = &clips[0];
        assert_eq!(name, "mnis_5_2_1_mix", "the clip name is the member stem");
        assert_eq!(*version, 6, "the NIS mix dialect");
        assert_eq!(*rate, 48000);
        assert_eq!(payload.len(), 912_000);
        assert_eq!(payload.len() % BLOCK_SIZE, 0);
        let blocks = payload.len() / BLOCK_SIZE;
        assert_eq!(blocks % 6, 0, "six interleaved streams");
        assert_eq!(blocks / 6, 7_600, "per-stream block count");
    }

    #[test]
    fn nis_mix_v6_decode_shape_and_silent_streams() {
        let data = nis_mix();
        let (_, version, rate, _tag, payload) = &all_clips(&data)[0];
        assert_eq!(*version, 6);
        let decoded = decode(*version, payload).unwrap();
        assert_eq!(decoded.channels, 6);
        // 7 600 blocks/stream × 32 samples, frame-interleaved ×6.
        assert_eq!(decoded.samples.len(), 7_600 * SAMPLES_PER_BLOCK * 6);
        // 243 200 frames at 48 kHz = 5.0667 s (the CLI's own number).
        assert!((decoded.duration_seconds(*rate) - 5.066_666).abs() < 1e-3);
        // Streams 3/4/5 ship entirely silent (all-zero blocks).
        let frames = decoded.samples.len() / 6;
        for f in 0..frames {
            for ch in 3..6 {
                assert_eq!(decoded.samples[f * 6 + ch], 0, "frame {f} ch {ch}");
            }
        }
        // The front three streams carry real content.
        for ch in 0..3 {
            assert!(
                decoded.samples[ch..].iter().step_by(6).any(|s| *s != 0),
                "stream {ch} must carry content"
            );
        }
    }

    /// Within-stream block-to-block continuity over pairs where BOTH
    /// the header and the previous block's last decoded sample are
    /// nonzero.  Returns (both_nz_pairs, within_8_count).
    fn stream_continuity(blocks: &[&[u8]], stream: usize, start: usize) -> (usize, usize) {
        let hdr = |b: &[u8]| i16::from_le_bytes([b[2], b[3]]);
        let mut prev_last: Option<i16> = None;
        let mut total = 0usize;
        let mut within8 = 0usize;
        for i in (start..blocks.len()).filter(|i| i % 6 == stream) {
            let h = hdr(blocks[i]);
            let mut out = Vec::new();
            decode_block_data(
                RadpBlockHeader::parse(blocks[i]),
                &blocks[i][4..],
                &mut out,
            );
            if let Some(prev) = prev_last {
                if h != 0 && prev != 0 {
                    total += 1;
                    if (h as i32 - prev as i32).abs() <= 8 {
                        within8 += 1;
                    }
                }
            }
            prev_last = out.last().copied();
        }
        (total, within8)
    }

    #[test]
    fn nis_mix_v6_stream_assignment_matches_encoder_continuity() {
        // THE PROOF TEST for the round-robin-6 assignment: within each
        // stream, consecutive blocks' header samples track the
        // previous block's last decoded sample within ±8 LSB (the
        // documented encoder-predictor drift) for ≥ 95 % of nonzero
        // pairs — while a WRONG assignment (stream 0's headers against
        // stream 1's decoded tails) tracks within ±8 for only ~2 %.
        // Pinned from the block-62 probe: 0.9877 / 0.9731 / 0.9983
        // (streams 0/1/2); the silent streams have zero nonzero pairs.
        let data = nis_mix();
        let (_, _version, _rate, _tag, payload) = &all_clips(&data)[0];
        let blocks: Vec<&[u8]> = payload.chunks_exact(BLOCK_SIZE).collect();
        let start = blocks.len() / 100;
        for stream in 0..6 {
            let (total, within8) = stream_continuity(&blocks, stream, start);
            if total == 0 {
                continue; // a fully silent stream — no pairs to test
            }
            let frac = within8 as f64 / total as f64;
            assert!(
                frac >= 0.95,
                "stream {stream}: continuity-within-8 {frac:.4} over {total} pairs"
            );
            if stream == 0 {
                // The WRONG-assignment contrast: cross-stream
                // predecessors must NOT track (pinned ~0.02).
                let hdr = |b: &[u8]| i16::from_le_bytes([b[2], b[3]]);
                let mut prev_last: Option<i16> = None;
                let (mut wt, mut w8) = (0usize, 0usize);
                for i in (start..blocks.len()).filter(|i| i % 6 == 0) {
                    let h = hdr(blocks[i]);
                    if let Some(prev) = prev_last {
                        if h != 0 && prev != 0 {
                            wt += 1;
                            if (h as i32 - prev as i32).abs() <= 8 {
                                w8 += 1;
                            }
                        }
                    }
                    // decode the NEXT block (stream 1's) — the wrong predecessor
                    let mut out = Vec::new();
                    decode_block_data(
                        RadpBlockHeader::parse(blocks[i + 1]),
                        &blocks[i + 1][4..],
                        &mut out,
                    );
                    prev_last = out.last().copied();
                }
                let wrong = w8 as f64 / wt as f64;
                assert!(
                    wrong < 0.10 && wrong * 10.0 < frac,
                    "wrong assignment must fail: {wrong:.4} vs correct {frac:.4}"
                );
            }
        }
    }

    #[test]
    fn v6_synthetic_round_robin_assignment_is_exact() {
        // 12 blocks = 2 per stream; block i's header sample is 1000+i
        // and its data is zero (decodes flat).  The de-interleave must
        // put block i's samples in stream i % 6, and the frame
        // interleave must reproduce them exactly.
        let mut payload = Vec::new();
        for i in 0..12u16 {
            let mut b = [0u8; BLOCK_SIZE];
            b[2..4].copy_from_slice(&(1000 + i as i16).to_le_bytes());
            payload.extend_from_slice(&b);
        }
        let streams = decode_six(&payload).unwrap();
        for s in 0..6usize {
            assert_eq!(streams[s].len(), 2 * SAMPLES_PER_BLOCK);
            let first = 1000 + s as i16;
            let second = 1006 + s as i16;
            assert!(streams[s][..SAMPLES_PER_BLOCK].iter().all(|x| *x == first));
            assert!(streams[s][SAMPLES_PER_BLOCK..].iter().all(|x| *x == second));
        }
        let decoded = decode(6, &payload).unwrap();
        assert_eq!(decoded.channels, 6);
        for f in 0..2 * SAMPLES_PER_BLOCK {
            let p = f / SAMPLES_PER_BLOCK;
            for s in 0..6usize {
                assert_eq!(
                    decoded.samples[f * 6 + s],
                    (1000 + (p * 6 + s) as i16) as i16
                );
            }
        }
    }

    #[test]
    fn v6_ragged_streams_are_silence_padded_to_equal_length() {
        // 7 blocks: stream 0 gets blocks 0 and 6, streams 1..5 one
        // each.  Distinct header samples make the padding observable.
        let mut payload = vec![0u8; BLOCK_SIZE * 7];
        for (i, b) in payload.chunks_exact_mut(BLOCK_SIZE).enumerate() {
            b[2..4].copy_from_slice(&((10 * i as i16).to_le_bytes()));
        }
        let streams = decode_six(&payload).unwrap();
        for s in 0..6usize {
            assert_eq!(streams[s].len(), 2 * SAMPLES_PER_BLOCK, "stream {s}");
        }
        assert!(streams[0][..SAMPLES_PER_BLOCK].iter().all(|x| *x == 0));
        assert!(streams[0][SAMPLES_PER_BLOCK..].iter().all(|x| *x == 60));
        for s in 1..6usize {
            assert!(
                streams[s][..SAMPLES_PER_BLOCK].iter().all(|x| *x == 10 * s as i16),
                "stream {s} content"
            );
            assert!(
                streams[s][SAMPLES_PER_BLOCK..].iter().all(|x| *x == 0),
                "stream {s} tail is documented silence padding"
            );
        }
    }

    #[test]
    fn v6_rejects_malformed_like_other_versions() {
        assert_eq!(
            decode_six(&vec![0u8; BLOCK_SIZE + 3]),
            Err(RadpError::NotBlockAligned { len: BLOCK_SIZE + 3 })
        );
        assert_eq!(decode_six(&[]), Err(RadpError::Empty));
        let mut bad = [0u8; BLOCK_SIZE * 6];
        bad[0] = 89; // stream 0's first block step index
        assert_eq!(
            decode_six(&bad),
            Err(RadpError::StepIndexOutOfRange { index: 89, block: 0 })
        );
        // Version 4 decodes since block 64 — a genuinely unshipped
        // version must still refuse (the census has only 1/2/4/6/18).
        assert_eq!(
            decode(3, &[0u8; BLOCK_SIZE * 3]),
            Err(RadpError::UnknownVersion { version: 3 })
        );
        assert_eq!(
            decode_multi(&[0u8; BLOCK_SIZE], 0),
            Err(RadpError::Empty)
        );
    }

    // -------------------------------------------------------------------
    // Block 64: radp_version 4 / 18 — the universal multi-stream
    // interleave.  Fixtures: the REAL release member
    // audio\english\AudioFile\ui_fe_woi_bg_quad_loop.p3d of
    // 01audio.rcf (the WHOLE member, 6 901 973 B, fetched by its
    // roster span — the smallest of the four v4/v18 members), and the
    // first 40 RADP frames of audio\english\AudioFile\
    // ambience_baked_18ch.p3d of 00audio.rcf (the whole member is
    // 60 820 687 B — too big to commit; the prefix pins the layout
    // byte-exactly, payload starts at file offset 127).
    // -------------------------------------------------------------------

    fn quad_member() -> Vec<u8> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join("audio_member_ui_fe_woi_bg_quad_loop.p3d");
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    fn radp18_prefix() -> Vec<u8> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join("radp18_ambience_baked_18ch_payload40.bin");
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    #[test]
    fn quad_member_v4_carries_one_four_channel_clip() {
        let data = quad_member();
        let clips = all_clips(&data);
        assert_eq!(clips.len(), 1, "a bed member is a one-clip P3D");
        let (name, version, rate, _tag, payload) = &clips[0];
        assert_eq!(name, "ui_fe_woi_bg_quad_loop", "the clip name is the member stem");
        assert_eq!(*version, 4, "the quad-bed dialect");
        assert_eq!(*rate, 48000);
        assert_eq!(payload.len(), 6_901_840);
        assert_eq!(payload.len() % BLOCK_SIZE, 0);
        let blocks = payload.len() / BLOCK_SIZE;
        assert_eq!(blocks % 4, 0, "four interleaved streams");
        assert_eq!(blocks / 4, 86_273, "per-stream block count (the index row)");
    }

    #[test]
    fn quad_member_v4_decodes_four_content_streams() {
        let data = quad_member();
        let (_, version, rate, _tag, payload) = &all_clips(&data)[0];
        assert_eq!(*version, 4);
        let decoded = decode(*version, payload).unwrap();
        assert_eq!(decoded.channels, 4);
        // 86 273 blocks/stream × 32 samples, frame-interleaved ×4.
        assert_eq!(decoded.samples.len(), 86_273 * SAMPLES_PER_BLOCK * 4);
        // 2 760 736 frames at 48 kHz = 57.515 s.
        assert!((decoded.duration_seconds(*rate) - 57.515_33).abs() < 1e-3);
        // Every stream carries real content (a quad loop, not a
        // 5.1-style master with unused channels).
        for ch in 0..4 {
            assert!(
                decoded.samples[ch..].iter().step_by(4).any(|s| *s != 0),
                "stream {ch} must carry content"
            );
        }
    }

    #[test]
    fn quad_member_v4_full_member_continuity_is_exact() {
        // THE PROOF TEST for the universal walk on v4: over the WHOLE
        // member (345 092 blocks = 4 × 86 273), within each stream the
        // next block's header sample tracks the previous block's last
        // decoded sample EXACTLY — measured on the full member, no
        // sampling.  A wrong assignment cannot fake this.
        let data = quad_member();
        let (_, _version, _rate, _tag, payload) = &all_clips(&data)[0];
        let blocks: Vec<&[u8]> = payload.chunks_exact(BLOCK_SIZE).collect();
        let mut chains: Vec<Option<i16>> = vec![None; 4];
        let (mut total, mut exact, mut within8, mut nz_total) = (0usize, 0usize, 0usize, 0usize);
        for (i, b) in blocks.iter().enumerate() {
            let h = RadpBlockHeader::parse(b);
            let mut out = Vec::new();
            decode_block_data(h, &b[4..], &mut out);
            let stream = i % 4;
            if let Some(prev) = chains[stream] {
                total += 1;
                if h.sample == prev {
                    exact += 1;
                }
                if (h.sample as i32 - prev as i32).abs() <= 8 {
                    within8 += 1;
                }
                if h.sample != 0 && prev != 0 {
                    nz_total += 1;
                }
            }
            chains[stream] = out.last().copied();
        }
        assert_eq!(total, 4 * (86_273 - 1), "every stream contributes 86 272 pairs");
        assert_eq!(
            exact, total,
            "block-to-block header continuity must be EXACT on every pair"
        );
        assert_eq!(within8, total, "within-8 is implied by exact");
        assert!(nz_total > total / 2, "the member is content-bearing");
    }

    #[test]
    fn radp18_prefix_decodes_eighteen_streams() {
        // The raw payload prefix (40 frames × 18 streams × 20 B).
        let payload = radp18_prefix();
        assert_eq!(payload.len(), 40 * 18 * BLOCK_SIZE);
        // Pinned frame-1 headers (payload offset 360 = frame 1,
        // stream 0): the first content-bearing frame of the real
        // ambience_baked_18ch member.
        let h0 = RadpBlockHeader::parse(&payload[18 * BLOCK_SIZE..]);
        assert_eq!((h0.step_index, h0.reserved, h0.sample), (24, 0, 413));
        let h1 = RadpBlockHeader::parse(&payload[19 * BLOCK_SIZE..]);
        assert_eq!((h1.step_index, h1.reserved, h1.sample), (17, 0, 4));
        let h2 = RadpBlockHeader::parse(&payload[20 * BLOCK_SIZE..]);
        assert_eq!((h2.step_index, h2.reserved, h2.sample), (19, 0, 470));
        let h3 = RadpBlockHeader::parse(&payload[21 * BLOCK_SIZE..]);
        assert_eq!((h3.step_index, h3.reserved, h3.sample), (19, 0, 511));
        // Decode: 18 channels, 40 × 32 frames, frame-interleaved.
        let decoded = decode(18, &payload).unwrap();
        assert_eq!(decoded.channels, 18);
        assert_eq!(decoded.samples.len(), 40 * SAMPLES_PER_BLOCK * 18);
        // Continuity across the prefix: every transition (39 per
        // stream) is EXACT — the encoder walked i % 18.
        let blocks: Vec<&[u8]> = payload.chunks_exact(BLOCK_SIZE).collect();
        let mut chains: Vec<Option<i16>> = vec![None; 18];
        let (mut total, mut exact) = (0usize, 0usize);
        for (i, b) in blocks.iter().enumerate() {
            let h = RadpBlockHeader::parse(b);
            let mut out = Vec::new();
            decode_block_data(h, &b[4..], &mut out);
            let stream = i % 18;
            if let Some(prev) = chains[stream] {
                total += 1;
                assert_eq!(
                    h.sample, prev,
                    "stream {stream} block {} continuity", i / 18
                );
                exact += 1;
            }
            chains[stream] = out.last().copied();
        }
        assert_eq!(total, 18 * 39);
        assert_eq!(exact, total);
    }

    #[test]
    fn decode_multi_matches_decode_six_on_the_nis_fixture() {
        // The block-62 fixture decodes identically through the
        // generalized path — one walk, no per-version dialect.
        let data = nis_mix();
        let (_, _version, _rate, _tag, payload) = &all_clips(&data)[0];
        let six = decode_six(payload).unwrap();
        let multi = decode_multi(payload, 6).unwrap();
        for s in 0..6 {
            assert_eq!(six[s], multi[s], "stream {s}");
        }
        let a = decode(6, payload).unwrap();
        let streams = decode_multi(payload, 6).unwrap();
        let frames = streams[0].len();
        let mut samples = Vec::with_capacity(frames * 6);
        for f in 0..frames {
            for ch in &streams {
                samples.push(ch[f]);
            }
        }
        assert_eq!(a, DecodedAudio { channels: 6, samples });
    }

    #[test]
    fn multi_ragged_tails_pad_to_equal_length_design() {
        // 5 blocks, 4 streams: stream 0 gets blocks 0 and 4, streams
        // 1–3 one each.  Distinct header samples make the padding
        // observable (same shape as the v6 ragged test).
        let mut payload = vec![0u8; BLOCK_SIZE * 5];
        for (i, b) in payload.chunks_exact_mut(BLOCK_SIZE).enumerate() {
            b[2..4].copy_from_slice(&((10 * i as i16).to_le_bytes()));
        }
        let streams = decode_multi(&payload, 4).unwrap();
        assert_eq!(streams.len(), 4);
        for s in 0..4usize {
            assert_eq!(streams[s].len(), 2 * SAMPLES_PER_BLOCK, "stream {s}");
        }
        assert!(streams[0][SAMPLES_PER_BLOCK..].iter().all(|x| *x == 40));
        for s in 1..4usize {
            assert!(streams[s][SAMPLES_PER_BLOCK..].iter().all(|x| *x == 0));
        }
        // decode() frame-interleaves the padded streams cleanly.
        let decoded = decode(4, &payload).unwrap();
        assert_eq!(decoded.channels, 4);
        assert_eq!(decoded.samples.len(), 2 * SAMPLES_PER_BLOCK * 4);
    }
}

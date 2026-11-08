//! prototype-audio
//! =================
//! Clean-room reconstruction of the Prototype (2009) **audio path**.
//!
//! ## What this crate provides
//!
//! * [`AudioClip`] — a typed handle to one decoded
//!   [`prototype_p3d::AudioFile`]: clip names, RADP version, sample
//!   rate, opaque codec tag, and the compressed payload bytes.
//! * [`AudioClipCatalog`] — a catalog of all audio clips in a P3D
//!   file, indexed by name; produces a [`FormatDistribution`] census
//!   showing which (sample_rate, format_tag, version) combinations
//!   appear in the shipped `boot_strap.p3d` fixture (70 clips).
//! * [`radp`] — **the RADP codec, SOLVED (M13)**: the payloads are
//!   standard IMA ADPCM in 20-byte blocks
//!   (`u8 step_index, u8 0, i16 predictor_sample, 16 data bytes`,
//!   low-nibble-first); `radp_version` 1 = mono, 2 = two
//!   interleaved mono streams (stereo).  Verified at 100.0000%
//!   block-continuity over all 191 295 real blocks — see the
//!   [`radp`] module docs for the full evidence chain.  Decode via
//!   [`AudioClip::decode`] and emit `.wav` bytes with
//!   [`radp::encode_wav16`].
//! * [`sample_rate`] — the `audio::SampleRateConverter`
//!   reconstruction (block 59): duration-preserving linear
//!   interpolation of interleaved i16 PCM between the two PROVEN
//!   clip rates (44 100 / 48 000 Hz).  The class NAME is PROVEN
//!   (RTTI); the original's kernel is UNKNOWN — the linear design
//!   here is documented as ours.
//!
//! ## What this crate does NOT provide
//!
//! * The engine's ORIGINAL backend policy (buffer sizing, thread
//!   priorities, backend selection) — UNKNOWN; the device edge
//!   here is our clean-room CPAL stream (see [`cpal_out`], behind
//!   the `audio-cpal` feature).
//! * The full meaning of `format_tag` (0..=31) — block 38 walked the
//!   engine's RADP open path in the DLL (`RadicalAdpcmFileDecoder::open`
//!   `0x10aaebc0`, resolved through the 4-class audio decoder registry
//!   `FileDecoder` / `RadicalAdpcmFileDecoder` / `RadicalAdpcmFile` /
//!   `DataDecoder` at vtables `0x10ea28e8..0x10ea290c`): the header
//!   parse reads version / sample_rate / format_tag / data_size as
//!   four u32s after the `RADP` magic, validates each, and pushes
//!   format_tag UNCHANGED as the 6th argument of a decoder-factory
//!   virtual (`vtable+0x14`) — no dispatch, no table, no compare on
//!   the tag anywhere in the parse, and the `RadicalAdpcmDecoder`
//!   init (`0x10aaf240`) takes no tag either (its config comes from
//!   the stream wrapper).  The old "indexes `MetaEnum
//!   W4WAVEFORMAT@audio@@`" hypothesis is DISSOLVED: that enum is the
//!   `MmeRendererDriver` OUTPUT-format property (`Set/GetWAVEFORMAT`
//!   MetaMethods), not a clip-side id.  The corpus additionally proves
//!   the tag is NOT the codec (all 70 clips decode as IMA ADPCM
//!   across tags 0..=31).  Remaining UNKNOWN (one virtual layer
//!   deeper): which decoder-factory implementation consumes it —
//!   LIKELY a wave-format/mixing configuration for the decode chain.
//!   Decode does not depend on it.
//!
//! ## Confidence labels
//!
//! * PROVEN: the `AudioFile` chunk's byte layout (PROVEN 70 of 70
//!   in `boot_strap.p3d`; see `prototype_p3d::ClassicChunk::audio_file`).
//! * PROVEN: RADP version is 1 or 2; sample_rate is 44100 or
//!   48000 in the shipped fixture (asserted in tests below).
//! * PROVEN: the RADP payload codec is standard IMA ADPCM
//!   (M13 — 100.0000% decode-continuity over every real block;
//!   see [`radp`] docs).
//! * PROVEN (block 38, engine disasm): `version` is the INTERLEAVED
//!   STREAM COUNT — the engine computes
//!   `blocks_per_stream = data_size / (20 · version)` and
//!   `decoded_samples_per_stream = blocks_per_stream · 32`
//!   (16 data bytes = 32 IMA nibble samples) inside the same parse
//!   (`0x10aaebc0`), matching our v1-mono / v2-two-interleaved-streams
//!   decoder exactly.  Pinned corpus-wide by
//!   `radp_header_arithmetic_matches_the_engine_parse` in
//!   prototype-bin's integration tests.
//! * UNKNOWN: `format_tag`'s exact consumer (see above — pass-through
//!   to the decoder factory, not the codec, not WAVEFORMAT);
//!   clip name ↔ gameplay event mapping; whether version-2 payloads
//!   are stereo or dual-mono (structurally two independent mono
//!   streams either way).

#![forbid(unsafe_code)]

pub mod mixer;
pub mod sample_rate;
pub mod sink;
pub mod radp;

/// M18 — the real audio output device edge (CPAL).  Behind the
/// `audio-cpal` feature so the default build stays
/// dependency-free; the Linux backend needs libasound headers.
/// PC (WASAPI) and Android (AAudio/Oboe) share the same
/// host-agnostic callback code.
#[cfg(feature = "audio-cpal")]
pub mod cpal_out;

use std::collections::BTreeMap;

use prototype_p3d::{AudioFile, P3dFile};

// ---------------------------------------------------------------------------
// FormatTag — opaque decoder-factory argument
// ---------------------------------------------------------------------------

/// The opaque RADP header tag (PROVEN: 0..=31 in the shipped
/// fixture).  Block 38's engine walk (see the crate docs) proves it
/// is NOT the codec (every tag decodes as IMA ADPCM) and NOT the
/// `MmeRendererDriver` WAVEFORMAT enum — the header parse pushes it
/// UNCHANGED to a decoder-factory virtual and never dispatches on
/// it.  The exact consumer is one virtual layer deeper (UNKNOWN,
/// LIKELY a wave-format/mixing config); decode does not depend on
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FormatTag(pub u32);

impl FormatTag {
    pub fn raw(self) -> u32 {
        self.0
    }
}

impl std::fmt::Display for FormatTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "format_tag={}", self.0)
    }
}

// ---------------------------------------------------------------------------
// AudioClip
// ---------------------------------------------------------------------------

/// A typed handle to one decoded AudioFile chunk: its clip name(s),
/// RADP block metadata, and the compressed payload bytes.
#[derive(Debug, Clone)]
pub struct AudioClip<'a> {
    /// Clip names (the observed chunks list each name twice; we
    /// deduplicate on construction).
    pub names: Vec<String>,
    /// Primary name (first entry of `names`).
    pub name: String,
    /// RADP block version — block 38 engine-PROVEN to be the
    /// INTERLEAVED STREAM COUNT (1 = one mono stream, 2 = two
    /// interleaved streams; the engine divides data_size by
    /// `20 · version` for blocks per stream).
    pub radp_version: u32,
    /// Sample rate (44100 or 48000 in the shipped fixture).
    pub sample_rate: u32,
    /// Opaque decoder-factory argument (0..=31 in the shipped
    /// fixture; NOT the codec, pass-through per the block-38 engine
    /// walk — decode does not depend on it).
    pub format_tag: FormatTag,
    /// Compressed audio payload (IMA ADPCM — decode via
    /// [`AudioClip::decode`]).
    pub data: &'a [u8],
}

impl<'a> AudioClip<'a> {
    /// Build from a parsed [`AudioFile`].
    pub fn from_audio_file(af: AudioFile<'a>) -> Self {
        // The observed chunks list the same name twice; deduplicate.
        let mut names = af.names.clone();
        names.sort();
        names.dedup();
        let name = names
            .first()
            .cloned()
            .unwrap_or_else(|| "<unnamed>".to_string());
        Self {
            names,
            name,
            radp_version: af.radp_version,
            sample_rate: af.sample_rate,
            format_tag: FormatTag(af.format_tag),
            data: af.data,
        }
    }

    pub fn payload_len(&self) -> usize {
        self.data.len()
    }

    /// Decode this clip's RADP payload to interleaved PCM
    /// (M13 — the codec is PROVEN standard IMA ADPCM; see
    /// [`radp`] for the evidence chain).
    pub fn decode(&self) -> Result<radp::DecodedAudio, radp::RadpError> {
        radp::decode(self.radp_version, self.data)
    }

    /// Decode this clip and encode the PCM as a `.wav` file
    /// (16-bit RIFF/WAVE at the clip's own sample rate) — the
    /// quickest end-to-end proof that the audio path works.
    pub fn decode_to_wav(&self) -> Result<Vec<u8>, radp::RadpError> {
        let audio = self.decode()?;
        Ok(radp::encode_wav16(&audio.samples, audio.channels, self.sample_rate))
    }
}

// ---------------------------------------------------------------------------
// AudioClipCatalog
// ---------------------------------------------------------------------------

/// A catalog of all audio clips in a P3D file, indexed by name.
///
/// Built by walking every `AudioFile` classic chunk in the file and
/// converting each to an [`AudioClip`].  Produces a
/// [`FormatDistribution`] census of the (sample_rate, format_tag,
/// version) combinations observed.
#[derive(Debug, Default)]
pub struct AudioClipCatalog<'a> {
    clips: Vec<AudioClip<'a>>,
    by_name: BTreeMap<String, usize>,
}

impl<'a> AudioClipCatalog<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a catalog by walking every `AudioFile` classic chunk
    /// in the parsed [`P3dFile`].
    pub fn from_p3d(file: &'a P3dFile<'a>) -> Result<Self, prototype_common::P3dError> {
        let mut cat = Self::new();
        for c in &file.chunks {
            if c.family != prototype_p3d::ChunkFamily::Classic {
                continue;
            }
            if let Some(classic) = c.classic()? {
                if let Some(af) = classic.audio_file()? {
                    let clip = AudioClip::from_audio_file(af);
                    cat.add(clip);
                }
            }
        }
        Ok(cat)
    }

    /// Add one clip.  First-insertion wins for the by-name index
    /// (the boot_strap fixture's clip names are unique per file,
    /// so collisions don't occur; this matches the PROVEN shape).
    pub fn add(&mut self, clip: AudioClip<'a>) {
        let i = self.clips.len();
        self.by_name.entry(clip.name.clone()).or_insert(i);
        self.clips.push(clip);
    }

    pub fn len(&self) -> usize {
        self.clips.len()
    }

    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }

    pub fn clips(&self) -> &[AudioClip<'a>] {
        &self.clips
    }

    pub fn find(&self, name: &str) -> Option<&AudioClip<'a>> {
        self.by_name.get(name).map(|&i| &self.clips[i])
    }

    /// Census of (sample_rate, format_tag, version) combinations.
    pub fn format_distribution(&self) -> FormatDistribution {
        let mut dist = FormatDistribution::default();
        for c in &self.clips {
            *dist
                .by_combo
                .entry((c.sample_rate, c.format_tag, c.radp_version))
                .or_insert(0usize) += 1;
            *dist.by_sample_rate.entry(c.sample_rate).or_insert(0usize) += 1;
            *dist.by_format_tag.entry(c.format_tag).or_insert(0usize) += 1;
            *dist.by_version.entry(c.radp_version).or_insert(0usize) += 1;
        }
        dist
    }
}

// ---------------------------------------------------------------------------
// FormatDistribution
// ---------------------------------------------------------------------------

/// Census of (sample_rate, format_tag, version) combinations across
/// a catalog.  Built by [`AudioClipCatalog::format_distribution`].
#[derive(Debug, Default, Clone)]
pub struct FormatDistribution {
    /// (sample_rate, format_tag, version) → clip count.
    pub by_combo: BTreeMap<(u32, FormatTag, u32), usize>,
    /// sample_rate → clip count.
    pub by_sample_rate: BTreeMap<u32, usize>,
    /// format_tag → clip count.
    pub by_format_tag: BTreeMap<FormatTag, usize>,
    /// RADP version → clip count.
    pub by_version: BTreeMap<u32, usize>,
}

impl FormatDistribution {
    pub fn total_clips(&self) -> usize {
        self.by_sample_rate.values().sum()
    }
    pub fn distinct_sample_rates(&self) -> usize {
        self.by_sample_rate.len()
    }
    pub fn distinct_format_tags(&self) -> usize {
        self.by_format_tag.len()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fixture(name: &str) -> Vec<u8> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    #[test]
    fn boot_strap_catalog_has_seventy_clips() {
        // THE M10 AUDIO ACCEPTANCE: the shipped boot_strap.p3d
        // carries 70 AudioFile chunks (M6 PROVEN); this test
        // reconstructs a typed catalog of all 70 from the parsed
        // P3D file.  Each clip has a non-empty compressed payload
        // (codec UNKNOWN; we don't decode — see module docs).
        let data = fixture("boot_strap.p3d");
        let f = P3dFile::parse(&data).unwrap();
        let cat = AudioClipCatalog::from_p3d(&f).unwrap();
        assert_eq!(cat.len(), 70);
        assert!(!cat.is_empty());
        for c in cat.clips() {
            assert!(!c.name.is_empty());
            assert!(c.payload_len() > 0, "{}: empty payload", c.name);
            // PROVEN ranges:
            assert!(
                c.radp_version == 1 || c.radp_version == 2,
                "{}: radp_version {} not in {{1, 2}}",
                c.name,
                c.radp_version
            );
            assert!(
                c.sample_rate == 44100 || c.sample_rate == 48000,
                "{}: sample_rate {} not in {{44100, 48000}}",
                c.name,
                c.sample_rate
            );
            assert!(
                c.format_tag.raw() <= 31,
                "{}: format_tag {} > 31",
                c.name,
                c.format_tag.raw()
            );
        }
    }

    #[test]
    fn boot_strap_distribution_reports_format_census() {
        let data = fixture("boot_strap.p3d");
        let f = P3dFile::parse(&data).unwrap();
        let cat = AudioClipCatalog::from_p3d(&f).unwrap();
        let dist = cat.format_distribution();
        // Total == clip count.
        assert_eq!(dist.total_clips(), 70);
        // Sample rates observed: 44100 and/or 48000 (PROVEN).
        for &sr in dist.by_sample_rate.keys() {
            assert!(sr == 44100 || sr == 48000, "unexpected sample rate {sr}");
        }
        // RADP versions observed: 1 and/or 2 (PROVEN).
        for &v in dist.by_version.keys() {
            assert!(v == 1 || v == 2, "unexpected RADP version {v}");
        }
        // Every distinct format_tag is in 0..=31 (PROVEN).
        for &tag in dist.by_format_tag.keys() {
            assert!(tag.raw() <= 31, "unexpected format_tag {}", tag.raw());
        }
        // The by_combo counts sum to 70.
        let sum: usize = dist.by_combo.values().sum();
        assert_eq!(sum, 70);
    }

    #[test]
    fn clip_dedupes_repeated_names() {
        // The AudioFile chunk lists each clip name twice (PROVEN
        // observation).  Our AudioClip::from_audio_file deduplicates.
        let af = AudioFile {
            names: vec!["music_intro".into(), "music_intro".into()],
            radp_version: 1,
            sample_rate: 44100,
            format_tag: 5,
            data: &[0u8; 16],
        };
        let clip = AudioClip::from_audio_file(af);
        assert_eq!(clip.names, vec!["music_intro".to_string()]);
        assert_eq!(clip.name, "music_intro");
        assert_eq!(clip.payload_len(), 16);
    }

    #[test]
    fn catalog_finds_clips_by_name() {
        let data = fixture("boot_strap.p3d");
        let f = P3dFile::parse(&data).unwrap();
        let cat = AudioClipCatalog::from_p3d(&f).unwrap();
        // Pick any clip; we should be able to look it up by name.
        let any_clip = &cat.clips()[0];
        let found = cat.find(&any_clip.name).unwrap();
        assert_eq!(found.name, any_clip.name);
        assert_eq!(found.radp_version, any_clip.radp_version);
        // Missing name → None.
        assert!(cat.find("nonexistent_clip").is_none());
    }
}

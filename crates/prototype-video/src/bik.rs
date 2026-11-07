//! Bink (`BIK?`) movie container parser — device-free, fail-closed.
//!
//! Everything here is validated against REAL `movies.rcf` members
//! (see the crate doc).  The parse shape is STREAMING-friendly: a
//! player reads the head ONCE (`head_len` bytes — 44 + audio block +
//! frame table), then seeks per frame; nothing ever requires the whole
//! movie in memory.

use prototype_common::{P3dError, Reader, Result};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// The Bink revision letters observed in the wild (FFmpeg's probe set
/// for Bink 1).  Prototype's movies are all `BIKi`.
pub const BIK_REVISIONS: &[u8] = b"bfghik";

/// Audio flag bits (FFmpeg `BINK_AUD_*` naming — field semantics
/// LIKELY, byte positions PROVEN).
pub const BIK_AUD_USEDCT: u16 = 0x1000;
pub const BIK_AUD_STEREO: u16 = 0x2000;
pub const BIK_AUD_16BITS: u16 = 0x4000;

/// Loader cap on audio tracks (DESIGN: the real movies carry 0 or 4;
/// FFmpeg's own cap is 256 — ours is what the app's mixer will accept,
/// and a larger count on real data would mean the parse has already
/// gone off the rails).
pub const MAX_AUDIO_TRACKS: u32 = 8;

/// Fixed size of the Bink file header (11 u32s).
pub const BIK_HEADER_SIZE: usize = 44;

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

/// The 44-byte Bink file header (see the crate doc for the evidence).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BikHeader {
    /// 4-byte magic, e.g. `b"BIKi"`.
    pub tag: [u8; 4],
    /// Real file length (the stored field is `file_size - 8`; PROVEN
    /// equal to the actual member length on 9/9 movies).
    pub file_size: u64,
    /// Number of frames.
    pub frame_count: u32,
    /// Largest frame span (== the exact table-computed max on the
    /// silent 48-frame movie; every observed span ≤ this).
    pub largest_frame_size: u32,
    /// The `@16` field: a second copy of `frame_count` (5/5 movies —
    /// the field FFmpeg skips as unknown).
    pub frame_count_copy: u32,
    /// Video width in pixels.
    pub width: u32,
    /// Video height in pixels.
    pub height: u32,
    /// Frame-rate numerator (2997 in all observed movies).
    pub fps_num: u32,
    /// Frame-rate denominator (100 in all observed movies).
    pub fps_den: u32,
    /// 4-byte codec extradata (0 in every observed movie).
    pub extradata: [u8; 4],
    /// Number of audio tracks (0 or 4 observed).
    pub audio_track_count: u32,
}

impl BikHeader {
    /// Parse and validate the 44-byte header from a prefix slice.
    /// On success also returns `head_len` — the byte length of the
    /// whole head (header + audio block + frame table), i.e. the
    /// offset of frame 0.
    pub fn parse(prefix: &[u8]) -> Result<(Self, u64)> {
        if prefix.len() < BIK_HEADER_SIZE {
            return Err(P3dError::Eof {
                offset: prefix.len(),
                needed: BIK_HEADER_SIZE - prefix.len(),
            });
        }
        let mut r = Reader::new(prefix);
        let tag_raw = r.read_bytes(4)?;
        let tag = [tag_raw[0], tag_raw[1], tag_raw[2], tag_raw[3]];
        // Bink 1: "BIK" + a revision letter from the observed set.
        // Bink 2 ("KB2") and SMUSH wrappers were never observed in this
        // game's movies and are rejected fail-closed.
        if !tag.starts_with(b"BIK") || !BIK_REVISIONS.contains(&tag[3]) {
            return Err(P3dError::BadMagicAscii {
                offset: 0,
                expected: "BIK[bfgik] (Bink 1; observed in this game: BIKi)",
                got: String::from_utf8_lossy(&tag).into_owned(),
            });
        }
        let file_size_field = r.read_u32()?;
        let frame_count = r.read_u32()?;
        let largest_frame_size = r.read_u32()?;
        let frame_count_copy = r.read_u32()?;
        let width = r.read_u32()?;
        let height = r.read_u32()?;
        let fps_num = r.read_u32()?;
        let fps_den = r.read_u32()?;
        let extradata_raw = r.read_bytes(4)?;
        let extradata = [
            extradata_raw[0],
            extradata_raw[1],
            extradata_raw[2],
            extradata_raw[3],
        ];
        let audio_track_count = r.read_u32()?;

        let file_size = file_size_field as u64 + 8;
        // The stored size must match the real member length — enforced
        // by the caller via [`BikIndex::parse_head`] (PROVEN 9/9).
        if frame_count == 0 || frame_count > 1_000_000 {
            return Err(P3dError::Other(format!(
                "bik frame_count {frame_count} outside 1..=1_000_000"
            )));
        }
        if largest_frame_size as u64 > file_size {
            return Err(P3dError::Other(format!(
                "bik largest_frame_size {largest_frame_size} exceeds file size {file_size}"
            )));
        }
        // PROVEN 5/5: the @16 field repeats the frame count.  A mismatch
        // means the parse has drifted off the real layout.
        if frame_count_copy != frame_count {
            return Err(P3dError::Other(format!(
                "bik frame_count copy {frame_count_copy} != frame_count {frame_count}"
            )));
        }
        if width == 0 || height == 0 {
            return Err(P3dError::Other(format!("bik degenerate dims {width}x{height}")));
        }
        if fps_num == 0 || fps_den == 0 {
            return Err(P3dError::Other(format!(
                "bik invalid fps {fps_num}/{fps_den}"
            )));
        }
        if audio_track_count > MAX_AUDIO_TRACKS {
            return Err(P3dError::Other(format!(
                "bik audio_track_count {} exceeds the loader cap {MAX_AUDIO_TRACKS}",
                audio_track_count
            )));
        }
        let head_len = BIK_HEADER_SIZE as u64
            + 12 * audio_track_count as u64
            + 4 * (frame_count as u64 + 1);
        Ok((
            Self {
                tag,
                file_size,
                frame_count,
                largest_frame_size,
                frame_count_copy,
                width,
                height,
                fps_num,
                fps_den,
                extradata,
                audio_track_count,
            },
            head_len,
        ))
    }

    /// Frame duration in seconds (fps_num / fps_den are both
    /// validated non-zero).
    pub fn duration_secs(&self) -> f64 {
        self.frame_count as f64 * self.fps_den as f64 / self.fps_num as f64
    }
}

// ---------------------------------------------------------------------------
// Audio tracks
// ---------------------------------------------------------------------------

/// One audio track of the 12-bytes-per-track block (PROVEN positions;
/// flag semantics LIKELY per FFmpeg's naming).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioTrack {
    /// Bound on the decoded size of one track packet.
    pub max_decoded_size: u32,
    /// Sample rate in Hz (48 000 observed).
    pub sample_rate: u16,
    /// `BIK_AUD_*` flag bits.
    pub flags: u16,
    /// Stream id (observed 0, 1, 2, 3).
    pub stream_id: u32,
}

impl AudioTrack {
    pub fn is_stereo(&self) -> bool {
        self.flags & BIK_AUD_STEREO != 0
    }

    /// `true` when the track uses the DCT codec variant (else RDFT —
    /// the block-60 census proves it is set on ALL 326 audio tracks
    /// of the game's 157 shipped movies, and FFmpeg's demuxer maps
    /// the bit to `binkaudio_dct` vs `binkaudio_rdft`).
    pub fn uses_dct(&self) -> bool {
        self.flags & BIK_AUD_USEDCT != 0
    }

    pub fn prefers_16bit(&self) -> bool {
        self.flags & BIK_AUD_16BITS != 0
    }
}

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

/// One frame's byte span in the movie file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSpan {
    /// `entry & !1` — the keyframe bit is stripped (PROVEN).
    pub start: u64,
    /// Exclusive end (`next entry & !1`; the last frame ends at the
    /// file size sentinel).
    pub end: u64,
    /// Keyframe flag (frame 0 is a keyframe by construction, matching
    /// FFmpeg; entry[0] also carried the bit in 4/4 real tables).
    pub keyframe: bool,
}

impl FrameSpan {
    pub fn size(&self) -> u64 {
        self.end - self.start
    }
}

// ---------------------------------------------------------------------------
// Whole-head index
// ---------------------------------------------------------------------------

/// Parsed Bink head: header + audio block + frame table.  Every frame
/// span is available without touching the payload region — the player
/// seeks to `frame.start` and reads `frame.size()` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BikIndex {
    pub header: BikHeader,
    pub audio: Vec<AudioTrack>,
    pub frames: Vec<FrameSpan>,
    /// Byte length of the whole head == frame 0's start offset (PROVEN
    /// equal on 4/4 fully-read tables).
    pub head_len: u64,
}

impl BikIndex {
    /// Parse the head from a PREFIX of the real movie.
    ///
    /// `prefix` must contain at least `head_len` bytes (the caller can
    /// learn the needed length cheaply via [`BikHeader::parse`] first).
    /// `real_file_size` is the actual member length on disk — the
    /// stored `file_size - 8` field must agree with it exactly (PROVEN
    /// 9/9) and the table's sentinel entry must equal it exactly
    /// (PROVEN 4/4).
    pub fn parse_head(prefix: &[u8], real_file_size: u64) -> Result<Self> {
        let (header, head_len) = BikHeader::parse(prefix)?;
        if real_file_size != header.file_size {
            return Err(P3dError::Other(format!(
                "bik stored file size {} != real member length {real_file_size}",
                header.file_size
            )));
        }
        if (head_len as usize) > prefix.len() {
            return Err(P3dError::Eof {
                offset: prefix.len(),
                needed: head_len as usize - prefix.len(),
            });
        }
        let mut r = Reader::new(&prefix[..head_len as usize]);
        r.pos = BIK_HEADER_SIZE;

        // Audio block, in FFmpeg's PLANAR order (PROVEN positions on
        // the 4-track movie: all max_decoded u32s first, then all
        // (rate, flags) u16 pairs, then all stream ids).
        let mut audio = Vec::with_capacity(header.audio_track_count as usize);
        for _ in 0..header.audio_track_count {
            let max_decoded_size = r.read_u32()?;
            audio.push(AudioTrack {
                max_decoded_size,
                sample_rate: 0,
                flags: 0,
                stream_id: 0,
            });
        }
        for t in audio.iter_mut() {
            t.sample_rate = r.read_u16()?;
            t.flags = r.read_u16()?;
            if t.sample_rate == 0 {
                return Err(P3dError::Other("bik audio track with sample_rate 0".into()));
            }
        }
        for t in audio.iter_mut() {
            t.stream_id = r.read_u32()?;
        }

        // Frame table: (frame_count + 1) u32s (PROVEN 4/4).
        let n = header.frame_count as usize;
        let mut entries = Vec::with_capacity(n + 1);
        for _ in 0..=n {
            entries.push(r.read_u32()?);
        }
        let mut frames = Vec::with_capacity(n);
        for i in 0..n {
            let start = (entries[i] & !1) as u64;
            let end = (entries[i + 1] & !1) as u64;
            if end <= start {
                return Err(P3dError::Other(format!(
                    "bik frame index not strictly increasing at frame {i}: {start} -> {end}"
                )));
            }
            let keyframe = i == 0 || entries[i] & 1 == 1;
            frames.push(FrameSpan { start, end, keyframe });
        }
        // Sentinel: the last entry equals the real file size exactly
        // (PROVEN 4/4: 1 603 968 / 1 986 012 / 5 138 984 / 311 658 208).
        if entries[n] as u64 != real_file_size {
            return Err(P3dError::Other(format!(
                "bik frame-table sentinel {} != real file size {real_file_size}",
                entries[n]
            )));
        }
        // Frame 0 begins exactly at the end of the head (PROVEN 4/4).
        if frames[0].start != head_len {
            return Err(P3dError::Other(format!(
                "bik frame 0 offset {} != head end {head_len}",
                frames[0].start
            )));
        }
        Ok(Self {
            header,
            audio,
            frames,
            head_len,
        })
    }

    /// Frame i's span (i < frame_count; enforced by the caller-facing
    /// signature via panic-free indexing — returns None when out of
    /// range).
    pub fn frame(&self, i: usize) -> Option<&FrameSpan> {
        self.frames.get(i)
    }

    /// Number of keyframes.
    pub fn keyframe_count(&self) -> usize {
        self.frames.iter().filter(|f| f.keyframe).count()
    }

    /// Largest frame span — compare against
    /// [`BikHeader::largest_frame_size`] (the stored bound is observed
    /// ≥ this value; the exact max is computed here from the table).
    pub fn max_frame_size(&self) -> u64 {
        self.frames.iter().map(|f| f.size()).max().unwrap_or(0)
    }

    /// Iterate the keyframe indices (seek targets for a player).
    pub fn keyframes(&self) -> impl Iterator<Item = usize> + '_ {
        self.frames
            .iter()
            .enumerate()
            .filter(|(_, f)| f.keyframe)
            .map(|(i, _)| i)
    }
}

// ---------------------------------------------------------------------------
// Unit tests against REAL movies.rcf members (block 46)
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

    /// The four committed heads are REAL byte ranges of REAL movies:
    ///  17a2ee41 — 48 frames, 1280x582, tracks=0 (silent), 1.6 MB member
    ///  e3025c1b — 33 frames, 1280x720, tracks=0, 2.0 MB member
    ///  04b837ad — 171 frames, 1280x720, tracks=0, 5.1 MB member
    ///  1e218528 — 5 721 frames, 1280x544, tracks=4, the 311 MB intro
    const FIXTURES: &[(&str, u64)] = &[
        ("movies_bik_17a2ee41_head256.bin", 1_603_968),
        ("movies_bik_e3025c1b_head256.bin", 1_986_012),
        ("movies_bik_04b837ad_head736.bin", 5_138_984),
        ("movies_bik_1e218528_head22980.bin", 311_658_208),
    ];

    #[test]
    fn header_parse_needs_44_bytes() {
        let head = fixture("movies_bik_17a2ee41_head256.bin");
        assert!(BikHeader::parse(&head[..43]).is_err());
        let (h, head_len) = BikHeader::parse(&head).unwrap();
        assert_eq!(h.tag, *b"BIKi");
        assert_eq!(h.file_size, 1_603_968);
        assert_eq!(h.frame_count, 48);
        assert_eq!(h.frame_count_copy, 48);
        assert_eq!(h.largest_frame_size, 62_752);
        assert_eq!((h.width, h.height), (1280, 582));
        assert_eq!((h.fps_num, h.fps_den), (2997, 100));
        assert_eq!(h.extradata, [0; 4]);
        assert_eq!(h.audio_track_count, 0);
        assert_eq!(head_len, 240);
        // 48 frames @ 2997/100 fps
        let d = h.duration_secs();
        assert!((d - 4800.0 / 2997.0).abs() < 1e-9);
    }

    #[test]
    fn parses_silent_48_frame_movie() {
        let head = fixture("movies_bik_17a2ee41_head256.bin");
        let idx = BikIndex::parse_head(&head, 1_603_968).unwrap();
        assert_eq!(idx.head_len, 240);
        assert!(idx.audio.is_empty());
        assert_eq!(idx.frames.len(), 48);
        let f0 = idx.frame(0).unwrap();
        assert_eq!((f0.start, f0.end), (240, 49_172));
        assert_eq!(f0.size(), 48_932);
        assert!(f0.keyframe);
        let f47 = idx.frame(47).unwrap();
        assert_eq!((f47.start, f47.end), (1_603_704, 1_603_968));
        // the stored demux bound == the exact max span on this movie
        // (62 752; frame 0's 48 932 is not the largest)
        assert_eq!(idx.max_frame_size(), 62_752);
        assert!(idx.keyframe_count() >= 1);
    }

    #[test]
    fn parses_720p_33_frame_movie() {
        let head = fixture("movies_bik_e3025c1b_head256.bin");
        let idx = BikIndex::parse_head(&head, 1_986_012).unwrap();
        assert_eq!((idx.header.width, idx.header.height), (1280, 720));
        assert_eq!(idx.header.frame_count, 33);
        assert_eq!(idx.head_len, 180);
        let f0 = idx.frame(0).unwrap();
        assert_eq!((f0.start, f0.end), (180, 94_784));
        assert_eq!(f0.size(), 94_604);
        assert!(idx.max_frame_size() <= idx.header.largest_frame_size as u64);
    }

    #[test]
    fn parses_171_frame_movie_with_full_table() {
        let head = fixture("movies_bik_04b837ad_head736.bin");
        let idx = BikIndex::parse_head(&head, 5_138_984).unwrap();
        assert_eq!(idx.header.frame_count, 171);
        assert_eq!(idx.head_len, 732);
        assert_eq!(idx.frame(0).unwrap().start, 732);
        assert_eq!(idx.frames.last().unwrap().end, 5_138_984);
        assert!(idx.max_frame_size() <= idx.header.largest_frame_size as u64);
    }

    #[test]
    fn parses_4_track_cinematic() {
        let head = fixture("movies_bik_1e218528_head22980.bin");
        let idx = BikIndex::parse_head(&head, 311_658_208).unwrap();
        assert_eq!((idx.header.width, idx.header.height), (1280, 544));
        assert_eq!(idx.header.frame_count, 5_721);
        assert_eq!(idx.head_len, 22_980);
        // the head fixture ends EXACTLY at frame 0 (head_len == prefix len)
        assert_eq!(idx.frame(0).unwrap().start, 22_980);
        let f0 = idx.frame(0).unwrap();
        assert_eq!((f0.start, f0.end), (22_980, 42_304));
        assert_eq!(f0.size(), 19_324);
        // audio block (PROVEN byte values)
        assert_eq!(idx.audio.len(), 4);
        let maxd: Vec<u32> = idx.audio.iter().map(|t| t.max_decoded_size).collect();
        assert_eq!(maxd, [145_920, 72_960, 72_960, 145_920]);
        for t in &idx.audio {
            assert_eq!(t.sample_rate, 48_000);
            assert!(t.uses_dct());
        }
        let flags: Vec<u16> = idx.audio.iter().map(|t| t.flags).collect();
        assert_eq!(flags, [0x7000, 0x5000, 0x5000, 0x7000]);
        assert!(idx.audio[0].is_stereo() && idx.audio[3].is_stereo());
        assert!(!idx.audio[1].is_stereo() && !idx.audio[2].is_stereo());
        let ids: Vec<u32> = idx.audio.iter().map(|t| t.stream_id).collect();
        assert_eq!(ids, [0, 1, 2, 3]);
        // duration ~190.9 s for the 311 MB intro
        assert!((idx.header.duration_secs() - 572_100.0 / 2997.0).abs() < 1e-9);
    }

    #[test]
    fn spans_tile_exactly_on_every_real_table() {
        for (name, size) in FIXTURES {
            let head = fixture(name);
            let idx = BikIndex::parse_head(&head, *size).unwrap();
            assert_eq!(idx.frames[0].start, idx.head_len, "{name}");
            for w in idx.frames.windows(2) {
                assert_eq!(w[0].end, w[1].start, "{name}: frames must tile");
                assert!(w[0].size() > 0, "{name}");
            }
            assert_eq!(idx.frames.last().unwrap().end, *size, "{name}: sentinel");
            // every frame inside the file
            assert!(idx.max_frame_size() <= idx.header.largest_frame_size as u64, "{name}");
        }
    }

    #[test]
    fn keyframe_flag_rides_the_low_bit() {
        let head = fixture("movies_bik_17a2ee41_head256.bin");
        let idx = BikIndex::parse_head(&head, 1_603_968).unwrap();
        // frame 0 is a keyframe by construction (FFmpeg semantics; the
        // real entry also carries the bit).
        assert!(idx.frames[0].keyframe);
        assert_eq!(idx.keyframes().next(), Some(0));
    }

    #[test]
    fn fails_closed_on_truncated_prefix() {
        let head = fixture("movies_bik_17a2ee41_head256.bin");
        // shorter than the fixed header
        assert!(BikIndex::parse_head(&head[..43], 1_603_968).is_err());
        // shorter than the full head (table needs 240 B)
        assert!(BikIndex::parse_head(&head[..239], 1_603_968).is_err());
        // exactly the head length parses
        assert!(BikIndex::parse_head(&head[..240], 1_603_968).is_ok());
    }

    #[test]
    fn fails_closed_on_bad_magic() {
        let head = fixture("movies_bik_17a2ee41_head256.bin");
        for (tag, why) in [
            (*b"BIKz", "unknown revision letter"),
            (*b"KB2i", "Bink 2 never observed in this game"),
            (*b"SMUS", "SMUSH wrapper is a different container"),
        ] {
            let mut bad = head.clone();
            bad[..4].copy_from_slice(&tag);
            assert!(
                BikIndex::parse_head(&bad, 1_603_968).is_err(),
                "{why} must fail closed"
            );
        }
    }

    #[test]
    fn fails_closed_on_stored_size_mismatch() {
        let head = fixture("movies_bik_17a2ee41_head256.bin");
        assert!(BikIndex::parse_head(&head, 1_603_969).is_err());
        assert!(BikIndex::parse_head(&head, 1_603_967).is_err());
    }

    #[test]
    fn fails_closed_on_header_gate_violations() {
        let head = fixture("movies_bik_17a2ee41_head256.bin");
        // @16 frame-count copy broken
        let mut bad = head.clone();
        bad[16..20].copy_from_slice(&49u32.to_le_bytes());
        assert!(BikIndex::parse_head(&bad, 1_603_968).is_err());
        // @12 largest_frame_size beyond the file
        let mut bad = head.clone();
        bad[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(BikIndex::parse_head(&bad, 1_603_968).is_err());
        // @28 fps numerator 0
        let mut bad = head.clone();
        bad[28..32].copy_from_slice(&0u32.to_le_bytes());
        assert!(BikIndex::parse_head(&bad, 1_603_968).is_err());
        // @40 audio track count over the loader cap (real file: 0)
        let mut bad = head.clone();
        bad[40..44].copy_from_slice(&9u32.to_le_bytes());
        assert!(BikIndex::parse_head(&bad, 1_603_968).is_err());
        // @8 frame count 0
        let mut bad = head.clone();
        bad[8..12].copy_from_slice(&0u32.to_le_bytes());
        assert!(BikIndex::parse_head(&bad, 1_603_968).is_err());
    }

    #[test]
    fn fails_closed_on_broken_frame_table() {
        let head = fixture("movies_bik_17a2ee41_head256.bin");
        // non-increasing: entry[1] duplicated to entry[0]'s offset
        let mut bad = head.clone();
        bad[48..52].copy_from_slice(&241u32.to_le_bytes());
        assert!(BikIndex::parse_head(&bad, 1_603_968).is_err());
        // sentinel no longer equals the real file size
        let mut bad = head.clone();
        bad[236..240].copy_from_slice(&(1_603_967u32).to_le_bytes());
        assert!(BikIndex::parse_head(&bad, 1_603_968).is_err());
    }
}

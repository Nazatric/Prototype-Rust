//! Bink frame-span layout — the DEMUX layer between the container
//! ([`crate::bik`]) and the codec (the video bitstream + audio packets
//! themselves are still out of scope here).
//!
//! ## Evidence (block 48, all against REAL `movies.rcf` frames)
//!
//! The layout follows FFmpeg's `bink` demuxer (`libavformat/bink.c`)
//! and was verified byte-exact on real frames of two movies:
//!
//! * the 4-track 311 MB intro `movies\NIS_0_1_FMV.bik` (frames 0 and 1
//!   fetched surgically by their exact index spans — 19 324 B and
//!   7 644 B), and
//! * the silent 48-frame 1.6 MB movie `0x17a2ee41` (frames 0 and 47
//!   read from the local member).
//!
//! ```text
//! frame span (from the index, PROVEN in block 46):
//!   per audio track (in track order):
//!     u32  compressed packet length  n      (FFmpeg's exact gate:
//!                                            n > remain - 4 -> reject)
//!     n bytes of Bink-audio packet data; when n >= 4 the packet's
//!       first u32 is the DECOMPRESSED byte count this packet yields
//!       (FFmpeg accumulates it for the audio PTS) — verified
//!       <= the track's head max_decoded_size on every real frame,
//!       with equality on the intro's frame 0 (front-loaded audio)
//!   [Bink revision >= 'i' only — all of this game's movies]:
//!     u32  UNKNOWN word — FFmpeg's decoder skips 32 bits here too
//!       (`if (c->version >= 'i') skip_bits_long(&gb, 32)`); observed
//!       values range 0xac .. 0xa6b0 on the intro and 132 .. 44712 on
//!       the silent movie — semantics UNKNOWN, never interpreted
//!   video bitstream = the remaining bytes of the span (a Bink
//!       "packet" the codec consumes; 312 B on the intro's all-black
//!       fade-in frame 0 up to 53 396 B on frame 100 — sizes vary
//!       with content exactly as a block-coded frame does)
//! ```

use prototype_common::{P3dError, Result};

use crate::bik::{BikIndex, MAX_AUDIO_TRACKS};

/// One audio packet of a frame (PROVEN layout; see the module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameAudioPacket {
    /// The `u32` length that prefixes the packet in the frame span.
    pub compressed_len: u32,
    /// The packet's first u32 — how many DECOMPRESSED bytes this
    /// packet yields (FFmpeg's audio-PTS accumulator).  `None` when
    /// the packet is shorter than 4 bytes (FFmpeg then treats the
    /// slot as empty and skips `compressed_len` bytes).
    pub decoded_bytes: Option<u32>,
    /// Offset of the packet payload within the frame span.
    pub offset: usize,
}

/// The parsed head of one frame span: the per-track audio packets,
/// the revision-gated skipped word, and where the video bitstream
/// starts.  The codec layers (Bink audio DCT/RDFT, the Bink video
/// plane decoder) consume `video_start .. video_start + video_len`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameHead {
    /// One packet per audio track, in track order.
    pub audio: Vec<FrameAudioPacket>,
    /// The revision-gated 32-bit word every `BIKi` frame carries
    /// between the audio packets and the video bitstream (semantics
    /// UNKNOWN — FFmpeg skips it too; never interpreted here).
    pub skip_word: Option<u32>,
    /// Offset of the video bitstream within the span.
    pub video_start: usize,
    /// Length of the video bitstream (span length minus everything
    /// consumed before it).
    pub video_len: usize,
}

/// Parse the head of one frame span (the whole span bytes, exactly
/// `FrameSpan::size()` of them).
///
/// * `tracks` — the header's `audio_track_count`.
/// * `max_decoded` — the tracks' head `max_decoded_size` bounds (the
///   decoded-count gate; pass `&[]` for a silent movie).
/// * `revision` — the magic's revision letter (`b'i'` for this
///   game's movies); revisions `>= 'i'` carry the skipped word.
pub fn parse_frame_head(
    span: &[u8],
    tracks: usize,
    max_decoded: &[u32],
    revision: u8,
) -> Result<FrameHead> {
    if tracks > MAX_AUDIO_TRACKS as usize {
        return Err(P3dError::Other(format!(
            "bik frame head: track count {tracks} exceeds the loader cap {MAX_AUDIO_TRACKS}"
        )));
    }
    if max_decoded.len() < tracks {
        return Err(P3dError::Other(format!(
            "bik frame head: {} max_decoded bounds for {tracks} tracks",
            max_decoded.len()
        )));
    }
    let mut remain = span.len();
    let mut pos = 0usize;
    let mut audio = Vec::with_capacity(tracks);
    for t in 0..tracks {
        if remain < 4 {
            return Err(P3dError::Eof {
                offset: pos,
                needed: 4 - remain,
            });
        }
        let compressed_len = u32::from_le_bytes([
            span[pos],
            span[pos + 1],
            span[pos + 2],
            span[pos + 3],
        ]);
        // FFmpeg's exact demuxer gate: audio_size > remain - 4 -> error.
        if compressed_len as usize > remain - 4 {
            return Err(P3dError::Other(format!(
                "bik frame head: track {t} audio size {compressed_len} exceeds the {remain} B left in the span"
            )));
        }
        pos += 4;
        remain -= 4;
        let mut decoded_bytes = None;
        if compressed_len >= 4 {
            if (compressed_len as usize) > remain {
                return Err(P3dError::Eof {
                    offset: pos,
                    needed: compressed_len as usize - remain,
                });
            }
            if remain < 4 {
                return Err(P3dError::Eof {
                    offset: pos,
                    needed: 4 - remain,
                });
            }
            let decoded = u32::from_le_bytes([
                span[pos],
                span[pos + 1],
                span[pos + 2],
                span[pos + 3],
            ]);
            if decoded > max_decoded[t] {
                return Err(P3dError::Other(format!(
                    "bik frame head: track {t} decoded byte count {decoded} exceeds the head bound {}",
                    max_decoded[t]
                )));
            }
            decoded_bytes = Some(decoded);
        }
        audio.push(FrameAudioPacket {
            compressed_len,
            decoded_bytes,
            offset: pos,
        });
        pos += compressed_len as usize;
        remain -= compressed_len as usize;
    }
    // Bink revision >= 'i': one skipped u32 between audio and video
    // (FFmpeg's decoder skips 32 bits at exactly this point; the
    // value's semantics are UNKNOWN and never interpreted here).
    let skip_word = if revision >= b'i' {
        if remain < 4 {
            return Err(P3dError::Eof {
                offset: pos,
                needed: 4 - remain,
            });
        }
        let w = u32::from_le_bytes([span[pos], span[pos + 1], span[pos + 2], span[pos + 3]]);
        pos += 4;
        remain -= 4;
        Some(w)
    } else {
        None
    };
    // A frame with no video bitstream at all is degenerate (every
    // observed real frame carries one, from the 260-byte all-skip tail
    // frame up to the 53 KB busy frames).
    if remain == 0 {
        return Err(P3dError::Other(
            "bik frame head: nothing left of the span for the video bitstream".into(),
        ));
    }
    Ok(FrameHead {
        audio,
        skip_word,
        video_start: pos,
        video_len: remain,
    })
}

impl BikIndex {
    /// Parse frame `i`'s head from its exact span bytes — the
    /// convenience binding of [`crate::bik::FrameSpan`] (spans, track
    /// count, revision) with [`parse_frame_head`].  `span_bytes` must
    /// be exactly `self.frame(i).unwrap().size()` long.
    pub fn frame_head(&self, i: usize, span_bytes: &[u8]) -> Result<FrameHead> {
        let f = self.frame(i).ok_or_else(|| {
            P3dError::Other(format!(
                "bik frame_head: frame {i} out of range ({})",
                self.frames.len()
            ))
        })?;
        if span_bytes.len() as u64 != f.size() {
            return Err(P3dError::Other(format!(
                "bik frame_head: got {} bytes for frame {i} but its span is {}",
                span_bytes.len(),
                f.size()
            )));
        }
        let max: Vec<u32> = self.audio.iter().map(|t| t.max_decoded_size).collect();
        parse_frame_head(
            span_bytes,
            self.header.audio_track_count as usize,
            &max,
            self.header.tag[3],
        )
    }
}

// ---------------------------------------------------------------------------
// Unit tests against REAL frames (block 48)
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

    const INTRO_MAX: [u32; 4] = [145_920, 72_960, 72_960, 145_920];

    /// The intro's frame 0 — real 19 324 B span: 4 audio packets whose
    /// decoded counts sit EXACTLY at the head bounds (front-loaded
    /// audio), then the skipped word, then the tiny all-black video.
    #[test]
    fn intro_frame0_layout() {
        let span = fixture("movies_bik_1e218528_frame0_19324.bin");
        let h = parse_frame_head(&span, 4, &INTRO_MAX, b'i').expect("frame 0 must parse");
        let sizes: Vec<u32> = h.audio.iter().map(|a| a.compressed_len).collect();
        assert_eq!(sizes, [14_520, 1_796, 764, 1_912]);
        let decoded: Vec<u32> = h.audio.iter().map(|a| a.decoded_bytes.unwrap()).collect();
        assert_eq!(decoded, INTRO_MAX, "frame 0 front-loads every track to max");
        assert_eq!(h.skip_word, Some(0xac));
        assert_eq!(h.video_start, 19_012);
        assert_eq!(h.video_len, 312);
        // packet offsets chain exactly (each offset = payload start,
        // i.e. after its own u32 length)
        assert_eq!(h.audio[0].offset, 4);
        assert_eq!(h.audio[1].offset, 4 + 14_520 + 4);
        assert_eq!(h.audio[2].offset, 4 + 14_520 + 4 + 1_796 + 4);
        assert_eq!(h.audio[3].offset, 4 + 14_520 + 4 + 1_796 + 4 + 764 + 4);
    }

    /// The intro's frame 1 — real 7 644 B span: decoded counts strictly
    /// BELOW the head bounds, a real-content video bitstream.
    #[test]
    fn intro_frame1_layout() {
        let span = fixture("movies_bik_1e218528_frame1_7644.bin");
        let h = parse_frame_head(&span, 4, &INTRO_MAX, b'i').expect("frame 1 must parse");
        let sizes: Vec<u32> = h.audio.iter().map(|a| a.compressed_len).collect();
        assert_eq!(sizes, [1_108, 132, 44, 168]);
        let decoded: Vec<u32> = h.audio.iter().map(|a| a.decoded_bytes.unwrap()).collect();
        assert_eq!(decoded, [7_680, 3_840, 3_840, 7_680]);
        assert_eq!(h.skip_word, Some(0x17d8));
        assert_eq!(h.video_start, 1_472);
        assert_eq!(h.video_len, 6_172);
    }

    /// The silent movie's frame 0: no audio at all — the span begins
    /// with the skipped word, then the video bitstream.
    #[test]
    fn silent_frame0_layout() {
        let span = fixture("movies_bik_17a2ee41_frame0_48932.bin");
        let h = parse_frame_head(&span, 0, &[], b'i').expect("frame 0 must parse");
        assert!(h.audio.is_empty());
        assert_eq!(h.skip_word, Some(44_712));
        assert_eq!(h.video_start, 4);
        assert_eq!(h.video_len, 48_928);
    }

    /// The silent movie's LAST frame (264 B): the tiny all-skip tail —
    /// video still present, just minimal.
    #[test]
    fn silent_frame47_layout() {
        let span = fixture("movies_bik_17a2ee41_frame47_264.bin");
        let h = parse_frame_head(&span, 0, &[], b'i').expect("frame 47 must parse");
        assert!(h.audio.is_empty());
        assert_eq!(h.skip_word, Some(132));
        assert_eq!(h.video_start, 4);
        assert_eq!(h.video_len, 260);
    }

    /// Revisions below 'i' carry no skipped word (FFmpeg's version
    /// gate) — the video bitstream would then start at the span head.
    /// No real rev<-'i' movie exists in this game (all BIKi); this
    /// pins the FFmpeg-derived branch so a future rev-'f' asset would
    /// parse consistently.
    #[test]
    fn revision_below_i_has_no_skip_word() {
        let span = fixture("movies_bik_17a2ee41_frame0_48932.bin");
        let h = parse_frame_head(&span, 0, &[], b'f').expect("rev f parse");
        assert_eq!(h.skip_word, None);
        assert_eq!(h.video_start, 0);
        assert_eq!(h.video_len, 48_932);
    }

    /// A decoded byte count above the track's head bound is rejected
    /// (the bound is the head's own max_decoded_size).
    #[test]
    fn decoded_count_above_bound_rejected() {
        let mut span = fixture("movies_bik_1e218528_frame1_7644.bin");
        // track 0's decoded count lives at offset 4 (packet payload start)
        span[4..8].copy_from_slice(&145_921u32.to_le_bytes());
        let err = parse_frame_head(&span, 4, &INTRO_MAX, b'i').unwrap_err();
        assert!(format!("{err}").contains("exceeds the head bound"), "{err}");
    }

    /// FFmpeg's exact demuxer gate: an audio size bigger than the
    /// remaining span (minus 4) is rejected.
    #[test]
    fn audio_size_over_remain_rejected() {
        let mut span = fixture("movies_bik_1e218528_frame1_7644.bin");
        span[0..4].copy_from_slice(&7641u32.to_le_bytes()); // span 7644 - 4 < 7641+4
        let err = parse_frame_head(&span, 4, &INTRO_MAX, b'i').unwrap_err();
        assert!(format!("{err}").contains("exceeds"), "{err}");
    }

    /// Truncation fails closed: cutting a track's length u32 in half
    /// is an EOF; cutting inside a packet region trips FFmpeg's exact
    /// size gate (the packet interior is unreachable after that gate,
    /// so the EOF branch behind it is purely defensive).
    #[test]
    fn truncated_frame_rejected() {
        let span = fixture("movies_bik_1e218528_frame1_7644.bin");
        // track 1's length u32 sits at 1112..1116 — cut inside it
        let err = parse_frame_head(&span[..1_114], 4, &INTRO_MAX, b'i').unwrap_err();
        assert!(matches!(err, P3dError::Eof { .. }), "{err}");
        // cut at 1200: track 1's 132-byte packet cannot fit in the 88 B left
        let err = parse_frame_head(&span[..1_200], 4, &INTRO_MAX, b'i').unwrap_err();
        assert!(format!("{err}").contains("exceeds the 88 B left"), "{err}");
    }

    /// A span that ends right after the skipped word has no video —
    /// degenerate, rejected.
    #[test]
    fn zero_video_rejected() {
        let span = fixture("movies_bik_17a2ee41_frame47_264.bin");
        let err = parse_frame_head(&span[..4], 0, &[], b'i').unwrap_err();
        assert!(format!("{err}").contains("video bitstream"), "{err}");
    }

    /// The full cross-layer path on REAL data: the intro's committed
    /// head fixture (header + audio block + the complete 5 722-entry
    /// table) pins frame 0's span, and the frame fixture parses
    /// through `BikIndex::frame_head` — the exact streaming shape a
    /// player uses.
    #[test]
    fn intro_head_and_frame0_through_the_index() {
        let head = fixture("movies_bik_1e218528_head22980.bin");
        let idx = BikIndex::parse_head(&head, 311_658_208).expect("head must parse");
        assert_eq!(idx.frames.len(), 5_721);
        assert_eq!(idx.frames[0].size(), 19_324);
        assert_eq!(idx.frames[1].size(), 7_644);
        let f0 = fixture("movies_bik_1e218528_frame0_19324.bin");
        let h = idx.frame_head(0, &f0).expect("frame 0 must parse");
        assert_eq!(h.audio.iter().map(|a| a.compressed_len).collect::<Vec<_>>(),
                   [14_520, 1_796, 764, 1_912]);
        assert_eq!(h.skip_word, Some(0xac));
        assert_eq!(h.video_len, 312);
        // wrong span length fails closed
        assert!(idx.frame_head(0, &f0[..100]).is_err());
        assert!(idx.frame_head(5_721, &f0).is_err());
    }

    /// Same cross-layer path for the silent movie (head fixture +
    /// both real frames).
    #[test]
    fn silent_head_and_frames_through_the_index() {
        let head = fixture("movies_bik_17a2ee41_head256.bin");
        let idx = BikIndex::parse_head(&head, 1_603_968).expect("head must parse");
        assert_eq!(idx.frames.len(), 48);
        let f0 = fixture("movies_bik_17a2ee41_frame0_48932.bin");
        let h = idx.frame_head(0, &f0).expect("frame 0 must parse");
        assert_eq!(h.skip_word, Some(44_712));
        assert_eq!(h.video_len, 48_928);
        let f47 = fixture("movies_bik_17a2ee41_frame47_264.bin");
        let h = idx.frame_head(47, &f47).expect("frame 47 must parse");
        assert_eq!(h.skip_word, Some(132));
        assert_eq!(h.video_len, 260);
    }
}

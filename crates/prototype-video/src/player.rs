//! The Bink PLAYER — the streaming walk that surfaces decoded A/V in
//! player-shaped forms (block 53, NEXT-UP item (a)'s library half).
//!
//! `BinkPlayer<R: Read + Seek>` composes the four PROVEN layers into
//! the exact I/O shape the APK player and `bik-play` use:
//!
//! * [`crate::bik`] — the head/index (read ONCE; per-frame seeks only),
//! * [`crate::frames`] — each frame span's audio/video split,
//! * [`crate::binkpixels::BinkVideoDecoder`] — the video bitstream →
//!   YUV420 planes (byte-exact vs real static ffmpeg, blocks 50–51),
//! * [`crate::binkaudio::BinkAudioDecoder`] — each track's packets →
//!   interleaved f32 PCM (FFmpeg-verified to ≤1 LSB s16, block 52),
//!   with the decoder held ACROSS frames so the 128-sample overlap
//!   crossfade state carries exactly as the codec requires.
//!
//! ## Output contract (all shapes proven upstream)
//!
//! * video — [`crate::binkpixels::BinkPlanes::yuv420_visible`]'s
//!   planar `w*h luma + (w/2)*(h/2) U + (w/2)*(h/2) V` bytes (even
//!   dims only — the visible-region extraction is fail-closed on the
//!   odd-height movie, an honestly-open gap);
//! * audio — one [`TrackPcm`] per track in track order, interleaved
//!   f32 in `[+1, -1]` codec scale (the container's per-packet
//!   `decoded_bytes` is the 16-BIT PCM byte count — flag 0x4000 — so
//!   f32 samples = bytes / 2; the committed ffmpeg s16 references pin
//!   the ratio); [`pcm_f32_to_s16`] applies the FFmpeg-CLI rounding
//!   (`round_ties_even` + clamp) when a device wants i16 (the shape
//!   [`prototype_audio::RingSink`] consumes).
//!
//! ## Honest UNKNOWN / policy notes
//!
//! * RDFT audio tracks fail closed at OPEN — and that gate is now
//!   PROVEN UNREACHABLE for this game: the block-60 census read the
//!   audio flags of every shipped movie (19 movies.rcf + 138 WoI =
//!   157 movies, 326 tracks) and EVERY track sets the USEDCT bit.
//!   The RDFT transform itself was investigated against the reference
//!   binary (scripts/m60_*.py probes): its bitstream PARSE deviates
//!   from the fetched FFmpeg source on synthetic RLE-run packets
//!   (all-zero blocks that per the source should decode silent
//!   produce deterministic nonzero output), so the transform contract
//!   stays an honest UNKNOWN — unreachable by this game's content.
//! * a 0-length audio packet slot yields empty PCM (FFmpeg's demuxer
//!   skips it too); a 1..3-B slot cannot carry the decompressed-size
//!   prefix so it is skipped the same way (FFmpeg reads the decoded
//!   count only when `audio_size >= 4`);
//! * PTS is `frame_index * fps_den / fps_num` — the container has no
//!   per-frame timestamps (FFmpeg derives audio PTS the same way,
//!   from the decoded byte counts).

use std::io::{Read, Seek, SeekFrom};

use prototype_common::{P3dError, Result};

use crate::bik::{BikHeader, BikIndex};
use crate::binkaudio::BinkAudioDecoder;
use crate::binkpixels::BinkVideoDecoder;
use crate::frames::FrameHead;

/// One audio track's description, from the container head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerAudioTrack {
    /// Track index (== its position in the head's audio block).
    pub index: usize,
    /// Head sample rate (48 000 on every observed movie).
    pub sample_rate: u32,
    /// `flags & 0x2000` — the channel count follows it (2/1).
    pub stereo: bool,
    /// Channels implied by `stereo` (2 or 1).
    pub channels: usize,
}

/// One track's decoded audio for ONE frame: interleaved f32 samples
/// (L R L R… for stereo; L L… for mono), codec scale (full-scale sine
/// ≈ ±1.0 — see [`crate::binkaudio`]).
#[derive(Debug, Clone, PartialEq)]
pub struct TrackPcm {
    /// The track this PCM belongs to.
    pub track: usize,
    /// Channels the samples interleave (== [`PlayerAudioTrack::channels`]).
    pub channels: usize,
    /// Interleaved f32 samples (`len() % channels == 0`).
    pub samples: Vec<f32>,
}

impl TrackPcm {
    /// Sample FRAMES in this PCM (samples / channels).
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }
}

/// One decoded movie frame: the visible YUV420 planes plus every
/// audio track's PCM for the SAME frame (a Bink frame multiplexes
/// its audio packets ahead of the video bitstream — see
/// [`crate::frames`]).
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerFrame {
    /// Frame index (0-based, in container order).
    pub index: usize,
    /// The container's keyframe bit for this frame.
    pub keyframe: bool,
    /// `index * fps_den / fps_num` seconds.
    pub pts_secs: f64,
    /// Visible YUV420 planar bytes (`w*h + 2*(w/2)*(h/2)`).
    pub yuv420: Vec<u8>,
    /// One entry per audio track, in track order (empty `samples`
    /// when the frame carries no packet for that track).
    pub audio: Vec<TrackPcm>,
}

/// A streaming Bink player: holds the open source, the parsed head,
/// the video decoder and one audio decoder PER TRACK (the overlap
/// crossfade state lives in the decoders and MUST persist across
/// frames — block 52's contract).
pub struct BinkPlayer<R: Read + Seek> {
    source: R,
    index: BikIndex,
    video: BinkVideoDecoder,
    /// Track descriptions (track order — `audio_tracks()` hands out).
    track_descs: Vec<PlayerAudioTrack>,
    /// One decoder PER TRACK — the overlap crossfade state lives
    /// here and persists across frames (block 52's contract).
    decoders: Vec<BinkAudioDecoder>,
    next: usize,
    /// Reused span buffer (one frame span at a time — the movie is
    /// never in RAM; the same I/O shape as the member stream).
    span: Vec<u8>,
}

impl<R: Read + Seek> BinkPlayer<R> {
    /// Open a movie stream.  `file_size` is the stream's exact byte
    /// length (the container's stored size must agree with it —
    /// PROVEN 9/9).  RDFT audio tracks fail closed here — a gate the
    /// block-60 census proves this game's content never triggers
    /// (326/326 shipped tracks are DCT).
    pub fn open(mut source: R, file_size: u64) -> Result<Self> {
        let mut head44 = [0u8; 44];
        read_exact_at(&mut source, &mut head44, 0)?;
        let (_, head_len) = BikHeader::parse(&head44)?;
        let mut head = vec![0u8; head_len as usize];
        read_exact_at(&mut source, &mut head, 0)?;
        let index = BikIndex::parse_head(&head, file_size)?;
        let h = &index.header;
        // swap_planes for revision >= 'h' (FFmpeg's decode_init);
        // every movie in this game is revision 'i'.
        let swap = h.tag[3] >= b'h';
        let video = BinkVideoDecoder::new(h.width, h.height, swap)?;
        let mut audio = Vec::new();
        let mut decoders = Vec::new();
        for (i, t) in index.audio.iter().enumerate() {
            // uses_dct() is the flag bit FFmpeg's demuxer maps to
            // binkaudio_dct (else binkaudio_rdft — mapping PROVEN,
            // block 60).  Every shipped track sets it (census:
            // 326/326); an RDFT track would need the unexercised
            // transform, whose contract could not be pinned against
            // the reference binary (scripts/m60_*.py) — fail closed
            // with the census in the message.
            if !t.uses_dct() {
                return Err(P3dError::Other(format!(
                    "bik player: audio track {i} is RDFT (flags {:#06x}) — the RDFT transform is unexercised and the block-60 census proves every shipped track (157 movies, 326 tracks) is DCT; refusing",
                    t.flags
                )));
            }
            let dec = BinkAudioDecoder::new(u32::from(t.sample_rate), t.is_stereo(), true)?;
            audio.push(PlayerAudioTrack {
                index: i,
                sample_rate: u32::from(t.sample_rate),
                stereo: t.is_stereo(),
                channels: if t.is_stereo() { 2 } else { 1 },
            });
            decoders.push(dec);
        }
        Ok(Self {
            source,
            index,
            video,
            track_descs: audio,
            decoders,
            next: 0,
            span: Vec::new(),
        })
    }

    /// Frame width (px).
    pub fn width(&self) -> u32 {
        self.index.header.width
    }

    /// Frame height (px).
    pub fn height(&self) -> u32 {
        self.index.header.height
    }

    /// Frames per second as `fps_num / fps_den` (2997/100 on every
    /// observed movie).
    pub fn fps(&self) -> (u32, u32) {
        (self.index.header.fps_num, self.index.header.fps_den)
    }

    /// Total frame count (from the index).
    pub fn frame_count(&self) -> usize {
        self.index.frames.len()
    }

    /// The next frame to decode (how many [`Self::next_frame`] calls
    /// until [`PlayerFrame`]s are exhausted).
    pub fn position(&self) -> usize {
        self.next
    }

    /// Audio track descriptions (track order).
    pub fn audio_tracks(&self) -> &[PlayerAudioTrack] {
        &self.track_descs
    }

    /// Movie duration in seconds (header arithmetic — see
    /// [`BikHeader::duration_secs`]).
    pub fn duration_secs(&self) -> f64 {
        self.index.header.duration_secs()
    }

    /// Decode and return the NEXT frame, or `None` at end.  Errors
    /// name the frame index; a consumed frame cannot be re-decoded
    /// (the decoders' state has advanced).
    pub fn next_frame(&mut self) -> Result<Option<PlayerFrame>> {
        let i = self.next;
        if i >= self.index.frames.len() {
            return Ok(None);
        }
        self.next += 1;
        let frame = self.index.frames[i];
        let size = frame.size() as usize;
        self.span.clear();
        self.span.resize(size, 0);
        read_exact_at(&mut self.source, &mut self.span, frame.start)?;

        let head: FrameHead = self.index.frame_head(i, &self.span)?;
        // Video: the span's tail after the audio packets and the
        // rev-'i' skipped word.
        let video_start = head.video_start;
        self.video.decode_frame(&self.span[video_start..])?;
        let planes = self
            .video
            .last()
            .ok_or_else(|| P3dError::Other("bik player: decoder produced no planes".into()))?;
        let yuv420 = planes.yuv420_visible()?;

        // Audio: one packet per track in track order; the DECODERS are
        // the persistent state (crossfade overlap), not the packets.
        let mut out = Vec::with_capacity(self.track_descs.len());
        for (t, desc) in self.track_descs.iter().enumerate() {
            let pkt = &head.audio[t];
            let samples = if pkt.compressed_len >= 4 {
                let lo = pkt.offset;
                let hi = lo + pkt.compressed_len as usize;
                self.decoders[t].decode_packet(&self.span[lo..hi])?
            } else {
                // 0-length slot (or 1..3 B — no decompressed-size
                // prefix): FFmpeg's demuxer skips the slot; the track
                // simply contributes no audio to this frame.
                Vec::new()
            };
            debug_assert_eq!(samples.len() % desc.channels, 0);
            out.push(TrackPcm {
                track: desc.index,
                channels: desc.channels,
                samples,
            });
        }

        let (num, den) = self.fps();
        let pts_secs = i as f64 * den as f64 / num as f64;
        Ok(Some(PlayerFrame {
            index: i,
            keyframe: frame.keyframe,
            pts_secs,
            yuv420,
            audio: out,
        }))
    }
}

/// FFmpeg-CLI-style f32 → s16 (`round_ties_even` + clamp) — the
/// rounding every block-52 ffmpeg comparison used.  The device edge
/// ([`prototype_audio`]'s `RingSink` is i16) consumes this.
pub fn pcm_f32_to_s16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&x| {
            let v = (x * 32768.0).round_ties_even();
            v.clamp(-32768.0, 32767.0) as i16
        })
        .collect()
}

impl<R: Read + Seek> std::fmt::Debug for BinkPlayer<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BinkPlayer")
            .field("dims", &format!("{}x{}", self.width(), self.height()))
            .field("frames", &self.frame_count())
            .field("position", &self.next)
            .field("audio_tracks", &self.track_descs.len())
            .finish()
    }
}

/// `read_exact` at an absolute offset (seek + read_exact, positions
/// restored by the seek itself — the member-stream shape).
fn read_exact_at<R: Read + Seek>(src: &mut R, buf: &mut [u8], at: u64) -> Result<()> {
    src.seek(SeekFrom::Start(at))
        .map_err(|e| P3dError::Other(format!("bik player: seek {at}: {e}")))?;
    src.read_exact(buf)
        .map_err(|e| P3dError::Other(format!("bik player: read {} B at {at}: {e}", buf.len())))
}

// ---------------------------------------------------------------------------
// Unit tests — committed REAL fixtures; the crafted biks are byte
// surgery on the REAL intro head (the same construction block 51/52
// used for their ffmpeg-verified references).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn fixture(name: &str) -> Vec<u8> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    /// Craft a bik from the REAL intro head: patch the frame count to
    /// `n`, rewrite the table to n spans, append the given span bytes.
    /// The spans are the REAL frame fixtures; the surgery is exactly
    /// what the block-51/52 ffmpeg-verified crafted files did.
    fn craft_bik(head22980: &[u8], spans: &[&[u8]]) -> Vec<u8> {
        assert_eq!(head22980.len(), 22_980);
        let mut head = head22980.to_vec();
        // frame_count (offset 8), largest_frame_size (12) and its copy
        // (16): the crafted file's own census (the index gate re-checks
        // largest <= file_size against the CRAFTED size).
        let largest = spans.iter().map(|s| s.len()).max().unwrap() as u32;
        head[8..12].copy_from_slice(&(spans.len() as u32).to_le_bytes());
        head[12..16].copy_from_slice(&largest.to_le_bytes());
        head[16..20].copy_from_slice(&(spans.len() as u32).to_le_bytes());
        // file_size-8 (offset 4) is patched AFTER total is known below.
        head.truncate(92);
        let mut out = head;
        let head_len = (44usize + 12 * 4 + 4 * (spans.len() + 1)) as u64;
        let mut total = head_len;
        for s in spans {
            total += s.len() as u64;
        }
        // the stored field is (real file size - 8) — PROVEN 9/9
        out[4..8].copy_from_slice(&((total - 8) as u32).to_le_bytes());
        // table: n spans + the sentinel (== crafted file size);
        // frame 0 keeps the keyframe bit (FFmpeg's construction).
        let mut off = head_len;
        for (i, s) in spans.iter().enumerate() {
            let entry = if i == 0 { off | 1 } else { off };
            out.extend_from_slice(&(entry as u32).to_le_bytes());
            off += s.len() as u64;
        }
        out.extend_from_slice(&(total as u32).to_le_bytes());
        for s in spans {
            out.extend_from_slice(s);
        }
        assert_eq!(out.len() as u64, total);
        out
    }

    /// The intro's real frame-0 span → its per-track audio packets
    /// (sizes PROVEN in frames.rs tests) — used to compare the
    /// player's audio path against a DIRECT decoder fed the same
    /// packet sequence.
    fn intro_track_packets(span: &[u8]) -> Vec<Vec<u8>> {
        let h = crate::frames::parse_frame_head(span, 4, &[145_920, 72_960, 72_960, 145_920], b'i')
            .expect("intro frame head");
        h.audio
            .iter()
            .map(|p| span[p.offset..p.offset + p.compressed_len as usize].to_vec())
            .collect()
    }

    /// The crafted 2-frame intro bik walks: dims/fps/tracks from the
    /// REAL head, frame 0 = MPEG black everywhere, frame 1 = real
    /// inter content, PTS arithmetic, keyframe flags, then end.
    #[test]
    fn crafted_two_frame_intro_walk() {
        let head = fixture("movies_bik_1e218528_head22980.bin");
        let s0 = fixture("movies_bik_1e218528_frame0_19324.bin");
        let s1 = fixture("movies_bik_1e218528_frame1_7644.bin");
        let bik = craft_bik(&head, &[&s0, &s1]);
        let mut p = BinkPlayer::open(Cursor::new(bik.clone()), bik.len() as u64)
            .expect("crafted bik opens");
        assert_eq!(p.width(), 1280);
        assert_eq!(p.height(), 544);
        assert_eq!(p.fps(), (2997, 100));
        assert_eq!(p.frame_count(), 2);
        assert_eq!(p.audio_tracks().len(), 4);
        assert_eq!(p.audio_tracks()[0], PlayerAudioTrack {
            index: 0, sample_rate: 48_000, stereo: true, channels: 2
        });

        // Frame 0 — all-black fade-in: Y16/UV128 everywhere (the
        // block-51 ffmpeg ground truth), front-loaded audio at the
        // head bounds.
        let f0 = p.next_frame().expect("f0").expect("f0 present");
        assert_eq!(f0.index, 0);
        assert!(f0.keyframe);
        assert_eq!(f0.pts_secs, 0.0);
        assert_eq!(f0.yuv420.len(), 1280 * 544 * 3 / 2);
        assert!(f0.yuv420.iter().all(|&v| v == 16 || v == 128));
        let counts: Vec<usize> = f0.audio.iter().map(|t| t.samples.len()).collect();
        // decoded_bytes is the 16-BIT PCM byte count (flag 0x4000):
        // f32 samples = bytes / 2 — pinned by the committed ffmpeg s16
        // references (t0: 7 680 B == 3 840 samples on frame 1).
        assert_eq!(counts, [72_960, 36_480, 36_480, 72_960]);
        for t in &f0.audio {
            assert_eq!(t.samples.len() % t.channels, 0);
        }

        // Frame 1 — real inter content (NOT all black), decoded from
        // frame 0's planes; audio below the bounds.
        let f1 = p.next_frame().expect("f1").expect("f1 present");
        assert_eq!(f1.index, 1);
        assert!(!f1.keyframe);
        assert!((f1.pts_secs - 100.0 / 2997.0).abs() < 1e-12);
        assert_eq!(f1.yuv420.len(), 1280 * 544 * 3 / 2);
        assert!(f1.yuv420.iter().any(|&v| v != 16));
        let counts: Vec<usize> = f1.audio.iter().map(|t| t.samples.len()).collect();
        assert_eq!(counts, [3_840, 1_920, 1_920, 3_840]);

        assert!(p.next_frame().expect("end").is_none());
        assert_eq!(p.position(), 2);
    }

    /// The player's audio path == a DIRECT BinkAudioDecoder fed the
    /// same packet sequence, INCLUDING the cross-state carry: track
    /// f32s are byte-identical for both frames (the decoders are the
    /// state, the player only routes packets).
    #[test]
    fn player_audio_matches_direct_decoder_including_state_carry() {
        let head = fixture("movies_bik_1e218528_head22980.bin");
        let s0 = fixture("movies_bik_1e218528_frame0_19324.bin");
        let s1 = fixture("movies_bik_1e218528_frame1_7644.bin");
        let p0 = intro_track_packets(&s0);
        let p1 = intro_track_packets(&s1);
        let bik = craft_bik(&head, &[&s0, &s1]);
        let mut player = BinkPlayer::open(Cursor::new(bik.clone()), bik.len() as u64).unwrap();

        // Direct decoders, one per track, fed frame 0's then frame 1's
        // packets — the same sequence the player must reproduce.  The
        // stereo flag per track comes from the REAL head (t0/t3 stereo,
        // t1/t2 mono).
        let stereo = [true, false, false, true];
        let mut direct: Vec<BinkAudioDecoder> = stereo
            .iter()
            .map(|&s| BinkAudioDecoder::new(48_000, s, true).unwrap())
            .collect();
        let mut direct_out: Vec<Vec<f32>> = vec![Vec::new(); 4];
        for frame_packets in [&p0, &p1] {
            for (t, pkt) in frame_packets.iter().enumerate() {
                direct_out[t].extend(direct[t].decode_packet(pkt).unwrap());
            }
        }

        let mut player_out: Vec<Vec<f32>> = vec![Vec::new(); 4];
        for _ in 0..2 {
            let f = player.next_frame().unwrap().unwrap();
            for t in &f.audio {
                player_out[t.track].extend_from_slice(&t.samples);
            }
        }
        for t in 0..4 {
            assert_eq!(
                player_out[t], direct_out[t],
                "track {t}: player path != direct decoder path"
            );
        }
    }

    /// Fresh-state ground truth: a crafted ONE-frame bik carrying the
    /// intro's REAL frame 1 decodes track 0/1 to s16 within the
    /// committed ffmpeg references (≤1 LSB stereo / EXACT mono — the
    /// block-52 contracts, now through the full player path).
    #[test]
    fn crafted_one_frame_intro_audio_matches_ffmpeg_s16() {
        let head = fixture("movies_bik_1e218528_head22980.bin");
        let s1 = fixture("movies_bik_1e218528_frame1_7644.bin");
        let bik = craft_bik(&head, &[&s1]);
        let mut p = BinkPlayer::open(Cursor::new(bik.clone()), bik.len() as u64).unwrap();
        let f = p.next_frame().unwrap().unwrap();

        for (t, name) in [(0, "t0"), (1, "t1")] {
            let reference = fixture(&format!("movies_bik_1e218528_f1_{name}_ref.s16"));
            let reference: Vec<i16> = reference
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect();
            let mine = pcm_f32_to_s16(&f.audio[t].samples);
            assert_eq!(mine.len(), reference.len(), "track {t} sample count");
            let worst = mine
                .iter()
                .zip(&reference)
                .map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs())
                .max()
                .unwrap();
            if t == 0 {
                assert!(worst <= 1, "track 0 s16 divergence {worst} LSB");
            } else {
                assert_eq!(worst, 0, "track 1 (mono) must be s16-EXACT");
            }
        }
    }

    /// An RDFT track (the DCT flag cleared in the REAL head's audio
    /// block) fails closed at OPEN with a player-shaped message.
    #[test]
    fn rdft_track_fails_closed_at_open() {
        let head = fixture("movies_bik_1e218528_head22980.bin");
        let s0 = fixture("movies_bik_1e218528_frame0_19324.bin");
        let mut bik = craft_bik(&head, &[&s0]);
        // The audio block sits at offset 44: [max_decoded ×4][rate,flags ×4][stream_id ×4].
        // Track 0's flags u16 = 44 + 16 + 4*0 + 2 = 62.
        let fl = 44 + 16 + 2;
        assert_eq!((bik[fl] as u16) | ((bik[fl + 1] as u16) << 8), 0x7000);
        bik[fl] = 0x00;
        bik[fl + 1] = 0x60; // 0x6000: 16-bit | stereo, DCT cleared
        let len = bik.len() as u64;
        let err = BinkPlayer::open(Cursor::new(bik), len).unwrap_err();
        assert!(format!("{err}").contains("RDFT"), "{err}");
    }

    /// The REAL silent movie (local, un-committable 1.6 MB member):
    /// the full streaming walk — 48 frames, no audio, per-frame YUV
    /// sizes, monotonic PTS, keyframe census.  Skips silently when the
    /// bulk file is absent (it lives under source/, never committed).
    #[test]
    fn real_silent_movie_full_walk_when_present() {
        let path = std::path::Path::new("/home/z/my-project/source/movies/movies_17a2ee41.bik");
        if !path.exists() {
            eprintln!("skip: real silent movie not present (bulk data stays uncommitted)");
            return;
        }
        let data = std::fs::read(path).unwrap();
        let mut p = BinkPlayer::open(Cursor::new(data.clone()), data.len() as u64)
            .expect("real silent movie opens");
        assert_eq!(p.width(), 1280);
        assert_eq!(p.height(), 582);
        assert_eq!(p.frame_count(), 48);
        assert!(p.audio_tracks().is_empty());
        let mut last_pts = -1.0;
        let mut keyframes = 0;
        for i in 0..48 {
            let f = p.next_frame().unwrap().expect("frame present");
            assert_eq!(f.index, i);
            assert!(f.pts_secs > last_pts);
            last_pts = f.pts_secs;
            assert_eq!(f.yuv420.len(), 1280 * 582 * 3 / 2);
            assert!(f.audio.is_empty());
            keyframes += f.keyframe as usize;
        }
        assert_eq!(keyframes, 1, "only frame 0 is a keyframe");
        assert!(p.next_frame().unwrap().is_none());
    }

    /// A stored-size mismatch fails closed (the index gate the player
    /// inherits — a truncated stream cannot open).
    #[test]
    fn truncated_stream_fails_closed() {
        let head = fixture("movies_bik_1e218528_head22980.bin");
        let s0 = fixture("movies_bik_1e218528_frame0_19324.bin");
        let bik = craft_bik(&head, &[&s0]);
        let err = BinkPlayer::open(Cursor::new(bik.clone()), bik.len() as u64 + 1)
            .unwrap_err();
        assert!(format!("{err}").contains("file size"), "{err}");
    }

    /// `pcm_f32_to_s16` pins the FFmpeg rounding: ties go to even,
    /// the clip rails at ±32768/32767, full-scale maps to rails.
    #[test]
    fn pcm_f32_to_s16_rounding_contract() {
        assert_eq!(pcm_f32_to_s16(&[0.0, -0.0]), [0, 0]);
        // 0.5 LSB boundaries: x*32768 == ±0.5 rounds to EVEN.
        assert_eq!(pcm_f32_to_s16(&[0.5 / 32768.0]), [0]); // +0.5 → 0
        assert_eq!(pcm_f32_to_s16(&[1.5 / 32768.0]), [2]); // +1.5 → 2
        assert_eq!(pcm_f32_to_s16(&[-0.5 / 32768.0]), [0]); // -0.5 → 0
        assert_eq!(pcm_f32_to_s16(&[1.0]), [32767]); // rail (clip)
        assert_eq!(pcm_f32_to_s16(&[-1.0]), [-32768]);
        assert_eq!(pcm_f32_to_s16(&[2.0, -2.0]), [32767, -32768]);
    }
}

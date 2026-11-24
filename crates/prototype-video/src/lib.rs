//! prototype-video
//! ===============
//! Movie subsystem for the Prototype (2009) reconstruction — the Bink
//! (RAD Game Tools `.bik`) CONTAINER format the game's `movies.rcf`
//! members use.
//!
//! ## Evidence (block 46, all against REAL `movies.rcf` members from the
//! user's GitHub release; the archive is 982 MB and its members were
//! read SURGICALLY with HTTP Range requests — head + directory first,
//! then exact member spans only)
//!
//! `movies.rcf` holds **26 members**: seven small Pure3D files
//! (568–5 366 B — FMV *subtitle text bibles* keyed
//! `engine::FmvSubtitleManager`, subtitle ids like `fmv0-1_01`,
//! `nis6-3_01`, `nukefmv_01`, one entry per language) and **nineteen
//! Bink movies** (568 B … 311 MB) whose payloads start with the magic
//! `BIKi`.  Header + frame-table facts below were verified byte-exact
//! on NINE movies; the frame TABLE (all entries) on FOUR of them
//! (48 / 33 / 171 / 5 721 frames).
//!
//! ### Header — 44 bytes, 11 little-endian u32s (PROVEN, 9/9 movies)
//!
//! ```text
//! +  0.. 4 : magic  "BIKi"  (Bink revision letter — see [`bik::BIK_REVISIONS`])
//! +  4.. 8 : u32 file_size - 8      (== real member length - 8, 9/9)
//! +  8..12 : u32 frame_count        (48 / 33 / 171 / 1 073 / 5 721 / …)
//! + 12..16 : u32 largest_frame_size — the largest frame span (== the
//!            exact table-computed max on the silent 48-frame movie,
//!            62 752; FFmpeg validates only ≤ file_size)
//! + 16..20 : u32 frame_count COPY   (== frame_count, 5/5 movies — the
//!            field FFmpeg's demuxer skips as "unknown")
//! + 20..24 : u32 width              (1280; one 1264-wide movie)
//! + 24..28 : u32 height             (720 / 582 / 576 / 575 / 544)
//! + 28..32 : u32 fps_num            (2997 — all 9 movies)
//! + 32..36 : u32 fps_den            (100  — all 9 movies) → 29.97 fps
//! + 36..40 : 4-byte extradata       (0 in every observed movie)
//! + 40..44 : u32 audio_track_count  (0 or 4 in observed movies)
//! ```
//!
//! ### Audio-track block (PROVEN positions on the 4-track movie)
//!
//! When `audio_track_count > 0` (the big cinematics: the 311 MB intro
//! `1e218528`, `26030f4f`, `0122a0f3`), the 44-byte head is followed by
//! 12 bytes per track — exactly the layout FFmpeg's `bink` demuxer
//! reads (LIKELY field semantics, PROVEN byte positions):
//!
//! * `u32 max_decoded_size` × tracks — observed
//!   `[145920, 72960, 72960, 145920]` (stereo tracks get 2× mono);
//! * `(u16 sample_rate, u16 flags)` × tracks — observed
//!   `(48 000, 0x7000)` twice and `(48 000, 0x5000)` twice: flag bits
//!   `0x1000` DCT / `0x2000` stereo / `0x4000` 16-bit.  The flag
//!   SEMANTICS are PROVEN two ways (block 60): FFmpeg's `bink`
//!   demuxer (libavformat/bink.c, read verbatim) selects
//!   `binkaudio_dct` iff `flags & 0x1000`, else `binkaudio_rdft` —
//!   and a crafted head with the bit cleared was observed selecting
//!   `binkaudio_rdft` live; AND the block-60 content census proves
//!   every shipped track (157 movies / 326 tracks across movies.rcf
//!   + the four WoI archives) sets the bit — the game is DCT-only,
//!   the RDFT transform never plays (the player fails closed; the
//!   block-60 probe scripts document why its contract stays
//!   UNKNOWN).
//! * `u32 stream_id` × tracks — observed `0, 1, 2, 3`.
//!
//! ### Frame index table — (frame_count + 1) u32s (PROVEN, 4/4 tables)
//!
//! * entry[i] low bit = frame i **keyframe** flag (entry[0] was odd —
//!   keyframe — in 4/4 real tables; FFmpeg additionally forces frame 0
//!   to be a keyframe by construction);
//! * real offset = `entry[i] & !1`; frame i spans
//!   `[entry[i] & !1, entry[i+1] & !1)`;
//! * the last entry is the SENTINEL and equals the real file size
//!   **exactly** (1 603 968 / 1 986 012 / 5 138 984 / 311 658 208);
//! * frame 0 starts exactly at the end of the head
//!   (`44 + 12·tracks + 4·(frame_count+1)`) — 4/4 movies.
//!
//! ### The frame-span head (block 48, PROVEN on real frames) — see
//! [`frames`]
//!
//! Each frame span parses as: per audio track `u32` compressed length
//! + packet (the packet's own first `u32` = its DECOMPRESSED byte
//! count, observed ≤ the track's head `max_decoded_size`, with
//! equality on the intro's front-loaded frame 0); then — for Bink
//! revision ≥ 'i', i.e. every movie in this game — one skipped u32
//! whose semantics are UNKNOWN (FFmpeg's decoder skips 32 bits at the
//! same point); the remainder is the video bitstream (312 B on the
//! intro's all-black fade-in frame 0, up to 53 KB on busy frames).
//! The block-46 "frame's first u32 is smaller than its span" mystery
//! is thereby RESOLVED: that u32 is the first audio packet length (or
//! the skipped word on silent movies), never a whole-span length.
//!
//! ### Codec layers (blocks 49–51)
//!
//! The Bink VIDEO codec is now DECODED end to end: the SYMBOL layer
//! ([`binkvideo`], block 50 — bit-exact FFmpeg frame walks) and the
//! PIXEL layer ([`binkpixels`], block 51 — renders the captured
//! records into YUV420 planes with FFmpeg-exact arithmetic).  The
//! combined decoder is byte-identical to real static ffmpeg on every
//! frame of the real silent movie (48/48, 53 637 120 B) and on the
//! intro's real frames 0+1 — see [`binkpixels`] and PROGRESS.md.
//! The Bink AUDIO packets (DCT/RDFT bands) remain the codec's open
//! layer; everything a player's demuxer needs (header, tracks,
//! per-frame spans, keyframes, the per-frame audio/video split) is
//! proven above.

#![forbid(unsafe_code)]

pub mod bik;
pub mod binkaudio;
pub mod binkdata;
pub mod binkpixels;
pub mod binkvideo;
pub mod frames;
pub mod members;
pub mod png;
pub mod player;
pub mod subtitle;
pub mod woi;
pub mod yuv;

pub use bik::{AudioTrack, BikHeader, BikIndex, FrameSpan};
pub use binkdata::{BINK_INTER_QUANT, BINK_INTRA_QUANT, BINK_PATTERNS, BINK_SCAN};
pub use binkaudio::{AudioBlock, BinkAudioDecoder};
pub use binkpixels::{BinkPlanes, BinkVideoDecoder};
pub use frames::{parse_frame_head, FrameAudioPacket, FrameHead};
pub use members::{
    bible_by_hash, bink_by_hash, BibleMember, MovieMember, BINK_MOVIES,
    SUBTITLE_BIBLES,
};
pub use png::png_encode_rgba;
pub use player::{pcm_f32_to_s16, BinkPlayer, PlayerAudioTrack, PlayerFrame, TrackPcm};
pub use subtitle::{FmvLanguage, FmvSubtitle, FmvSubtitles};
pub use woi::{woi_by_hash, WoiMember, WOI_BIBLES, WOI_BIKS, WOI_MEMBERS};
pub use yuv::yuv420_to_rgba8;

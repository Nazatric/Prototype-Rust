//! movie — the BOOT-PATH MOVIE hookup (block 54, NEXT-UP (a)): a real
//! Bink movie streams out of the user's own `*.rcf` archive, decodes
//! through the four PROVEN layers, and reaches the screen and the
//! audio ring in player shapes.
//!
//! # The pipeline this module composes (every stage already PROVEN)
//!
//! ```text
//! find_boot_movie      — movies.rcf (any *movie*.rcf) → smallest bik
//!                        member, identified by its REAL parsed head
//! MoviePlayerState     — BinkPlayer<MemberSpanReader<File>> (the 311 MB
//!                        intro is never in RAM: one seek + one span
//!                        read per frame, straight out of the archive)
//!   .advance()         — wall-clock pacing at the container's fps;
//!                        per frame:
//!   yuv420_to_rgba8    — BT.601 limited (pinned ≤2/255 vs ffmpeg)
//!   → upload_video_frame → the fullscreen quad (prototype-render)
//!   pcm_f32_to_s16     — FFmpeg-CLI rounding (≤1 LSB, block 52)
//!   → Arc<Mutex<RingSink>> — the SharedRing handoff (prototype-audio);
//!                        the CPAL device edge consumes exactly this
//!                        shape (proven end-to-end in prototype-bin).
//! ```
//!
//! # Evidence labels
//!
//! * PROVEN: the container/codec layers (blocks 46–53), the span
//!   window (block 54, `prototype_rcf::MemberSpanReader`), the yuv
//!   and s16 conversions, the ring semantics.
//! * DESIGN (ours): WHICH movie plays at boot and its selection rule
//!   (smallest bik member of any `*movie*.rcf` — the engine's own
//!   boot-movie script order is not recovered; the smallest member
//!   gives the fastest first pixel on a phone).  Track 0 is the
//!   audio route (the container's track semantics beyond the flag
//!   bits are UNKNOWN — 4 tracks of 2 stereo + 2 mono could be
//!   languages or channel sets; track 0 is the first and stereo on
//!   every observed movie).
//! * DESIGN (ours): skipping — any key press, mouse click or touch
//!   start ends playback (the frontend's movies are skippable in the
//!   original; the input binding is ours).  The world scene resumes
//!   the same frame the movie ends.
//! * UNKNOWN: the on-device audio output rate policy (movies are
//!   48 kHz; the device edge opens at the host default — the engine
//!   hints an `audio::SampleRateConverter` class the reconstruction
//!   does not carry yet).  The ring never blocks its writer: when
//!   the device drains slower than the decode feeds, the oldest
//!   samples are overwritten and counted (`RingSink::dropped`).

use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use prototype_audio::sink::RingSink;
use prototype_common::{P3dError, Result};
use prototype_rcf::{parse_directory, MemberSpanReader, RcfDirEntry};
use prototype_video::{pcm_f32_to_s16, yuv420_to_rgba8, BinkPlayer, BikHeader};

/// The 44-byte Bink head prefix [`BikHeader::parse`] needs to identify
/// a movie member (re-exported from the container layer).
const BIK_HEAD_LEN: usize = prototype_video::bik::BIK_HEADER_SIZE;

/// Hard decode budget per redraw (frames): bounds one frame's CPU cost
/// so a movie that falls behind cannot starve the surface.  A movie
/// that cannot keep up degrades to slow motion (the decoder cannot
/// skip inter-predicted frames — by design; the container's frame
/// dependency chain is PROVEN).
const MAX_FRAMES_PER_REDRAW: usize = 4;

/// Movie audio ring capacity, interleaved SAMPLES (~2.7 s of 48 kHz
/// stereo): comfortably over the intro's front-loaded first frame
/// (72 960 samples) so a healthy device edge never drops.
const MOVIE_RING_CAPACITY: usize = 1 << 17;

/// One boot movie's identity: the archive it lives in, its directory
/// entry (offset + size — the span the player windows) and its hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootMovie {
    /// The archive file the member was found in.
    pub archive: PathBuf,
    /// The directory entry (byte span inside the archive).
    pub entry: RcfDirEntry,
    /// The entry's `name_hash` (the member's identity, as everywhere).
    pub member_hash: u32,
}

/// Find the boot movie: the SMALLEST Bink member of the first
/// `*movie*.rcf` archive that carries one (case-insensitive stem match;
/// other archives are not scanned — art/cells/audio/woi hold no boot
/// movies and their directories can hold tens of thousands of entries).
///
/// A member IS a movie when its first 44 bytes parse as a real Bink
/// head ([`BikHeader::parse`] — magic, dims, fps, the PROVEN field
/// gates) — not a name guess: `movies.rcf`'s six still-unnamed
/// members (block 53's honest UNKNOWN) are first-class candidates.
pub fn find_boot_movie(rcf_archives: &[PathBuf]) -> Option<BootMovie> {
    for path in rcf_archives {
        let is_movie_archive = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_ascii_lowercase().contains("movie"))
            .unwrap_or(false);
        if !is_movie_archive {
            continue;
        }
        let mut file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) => {
                log::warn!("boot movie: {} unreadable ({e}) — skipping", path.display());
                continue;
            }
        };
        let entries = match parse_directory(&mut file) {
            Ok((_, entries)) => entries,
            Err(e) => {
                log::warn!(
                    "boot movie: {} directory parse failed ({e}) — skipping",
                    path.display()
                );
                continue;
            }
        };
        let mut best: Option<&RcfDirEntry> = None;
        for e in &entries {
            if e.file_size < BIK_HEAD_LEN as u32 {
                continue;
            }
            if file.seek(SeekFrom::Start(e.file_offset as u64)).is_err() {
                continue;
            }
            let mut head = [0u8; BIK_HEAD_LEN];
            if file.read_exact(&mut head).is_err() {
                continue;
            }
            if BikHeader::parse(&head).is_ok() && best.map_or(true, |b| e.file_size < b.file_size)
            {
                best = Some(e);
            }
        }
        if let Some(e) = best {
            return Some(BootMovie {
                archive: path.clone(),
                entry: *e,
                member_hash: e.name_hash,
            });
        }
    }
    None
}

/// The candidate member paths of a NAMED movie (block 67).  The
/// convention is PROVEN to be `<path>\<file>.bik` with the directory
/// hash-consistent under [`prototype_rcf::hash_name`]:
/// `movies\woi\woi_ec1.bik` hashes to the real 00woi.rcf directory
/// entry `0x32824477` (block 67 empirical cross-check — the WOI
/// archives carry FULL name tables, `movies\woi\woi_ec1..7.bik` +
/// `woi_node_001..209.bik` split across the four assets).
///
/// What is NOT proven is WHERE the 46 NIS cutscene movies live: the
/// block-67 dictionary attack (46 `boot_strap.p3d` `NIS_Player`
/// object names × these prefixes × {`.bik`, bare}) found NO hit in
/// movies.rcf's 26 members or 00woi's 24, and block 69's STRUCTURAL
/// attack (7 497 696 candidates: family-prefix rewrites, underscore
/// insertion, separator swaps, `_FMV`, the m/s/p grids over all
/// three NIS families) closed the space — ZERO hits on the six
/// unnamed movies.rcf members and no object→movie rule
/// (`prototype-android::nis` pins it in-repo).  The movie name is
/// EXTERNAL DATA (the engine's own `nis_SetFMVFileName` is a setter);
/// the candidates below are the honest convention set, tried in
/// order, fail-closed to `None` (the caller stays silent, exactly
/// like a missing boot movie).
pub fn movie_member_hashes(name: &str) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for candidate in [
        format!("movies\\woi\\{name}.bik"),
        format!("movies\\{name}.bik"),
        format!("movies\\nis\\{name}.bik"),
        format!("{name}.bik"),
    ] {
        let h = prototype_rcf::hash_name(&candidate);
        if !out.contains(&h) {
            out.push(h);
        }
    }
    out
}

/// Find a NAMED movie member: the by-name counterpart of
/// [`find_boot_movie`] (block 67).  Scans the same archive family
/// the boot scan does (`*movie*.rcf`) PLUS the WOI archives
/// (`*woi*.rcf` — PROVEN to carry bik members with full name
/// tables), matches directory `name_hash` against
/// [`movie_member_hashes`], and verifies the member IS a real Bink
/// head before returning it (a hash hit on a non-movie is refused —
/// fail-closed, never a guess).
///
/// The archive directory is parsed per call (the boot path's own
/// I/O shape; a phone has no reason to hold 26..thousands of
/// entries hot).
pub fn find_movie_by_name(rcf_archives: &[PathBuf], name: &str) -> Option<BootMovie> {
    let candidates = movie_member_hashes(name);
    for path in rcf_archives {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let is_candidate_archive = stem.contains("movie") || stem.contains("woi");
        if !is_candidate_archive {
            continue;
        }
        let mut file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) => {
                log::warn!("movie by name: {} unreadable ({e}) — skipping", path.display());
                continue;
            }
        };
        let entries = match parse_directory(&mut file) {
            Ok((_, entries)) => entries,
            Err(e) => {
                log::warn!(
                    "movie by name: {} directory parse failed ({e}) — skipping",
                    path.display()
                );
                continue;
            }
        };
        for e in &entries {
            if e.file_size < BIK_HEAD_LEN as u32 || !candidates.contains(&e.name_hash) {
                continue;
            }
            if file.seek(SeekFrom::Start(e.file_offset as u64)).is_err() {
                continue;
            }
            let mut head = [0u8; BIK_HEAD_LEN];
            if file.read_exact(&mut head).is_err() {
                continue;
            }
            if BikHeader::parse(&head).is_ok() {
                return Some(BootMovie {
                    archive: path.clone(),
                    entry: *e,
                    member_hash: e.name_hash,
                });
            }
        }
    }
    None
}

/// What one [`MoviePlayerState::advance`] did — the app's per-redraw
/// decision inputs (upload the RGBA when `frames_decoded > 0`; draw
/// the quad and come back unless `ended`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MovieAdvance {
    /// Frames decoded THIS advance (0 = not due yet).
    pub frames_decoded: usize,
    /// True once the movie is over (skipped or exhausted).
    pub ended: bool,
    /// Interleaved s16 samples written into the ring this advance.
    pub audio_samples: usize,
}

/// The streaming boot-movie player state — the app owns one while a
/// movie is up.  Device-free: the video half exposes RGBA bytes (the
/// app uploads them through the renderer's video texture), the audio
/// half exposes the shared ring handle (the device edge drains it).
pub struct MoviePlayerState {
    player: BinkPlayer<MemberSpanReader<std::fs::File>>,
    ring: Arc<Mutex<RingSink>>,
    /// The last decoded frame, tightly packed RGBA8 (`4·w·h`).
    rgba: Vec<u8>,
    started: Option<Instant>,
    frames_done: usize,
    ended: bool,
    /// Audio samples (interleaved s16) written since open.
    audio_samples_written: u64,
    /// The audio route's channel count (track 0 — module docs).
    audio_channels: usize,
}

impl MoviePlayerState {
    /// Open the boot movie for streaming playback: the archive file is
    /// opened and WINDOWED to the member's span (nothing is extracted);
    /// the Bink head/index is read once at open, every frame later is
    /// one seek + one span read.  Fail-closed: RDFT audio tracks (the
    /// unexercised transform — a gate the block-60 census proves no
    /// shipped movie can trigger: 326/326 tracks are DCT) refuse to
    /// open — the app plays without the movie rather than decode
    /// garbage.
    pub fn open(movie: &BootMovie) -> Result<Self> {
        let file = std::fs::File::open(&movie.archive).map_err(|e| {
            P3dError::Other(format!("boot movie: open {}: {e}", movie.archive.display()))
        })?;
        let span = MemberSpanReader::new(
            file,
            movie.entry.file_offset as u64,
            movie.entry.file_size as u64,
        )?;
        let player = BinkPlayer::open(span, movie.entry.file_size as u64)?;
        let audio_channels = player
            .audio_tracks()
            .first()
            .map(|t| t.channels)
            .unwrap_or(0);
        Ok(Self {
            player,
            ring: Arc::new(Mutex::new(RingSink::new(MOVIE_RING_CAPACITY))),
            rgba: Vec::new(),
            started: None,
            frames_done: 0,
            ended: false,
            audio_samples_written: 0,
            audio_channels,
        })
    }

    /// Frame width (px).
    pub fn width(&self) -> u32 {
        self.player.width()
    }

    /// Frame height (px).
    pub fn height(&self) -> u32 {
        self.player.height()
    }

    /// Total frame count (container index).
    pub fn frame_count(&self) -> usize {
        self.player.frame_count()
    }

    /// Frames decoded so far.
    pub fn frames_done(&self) -> usize {
        self.frames_done
    }

    /// Container fps as `num / den` (2997/100 on every observed movie).
    pub fn fps(&self) -> (u32, u32) {
        self.player.fps()
    }

    /// Track 0's channel count (0 = the movie is silent).
    pub fn audio_channels(&self) -> usize {
        self.audio_channels
    }

    /// The last decoded frame, tightly packed RGBA8 (`4·w·h` bytes).
    /// Valid after any `advance` that reported `frames_decoded > 0`.
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// The shared audio ring — the handle a CPAL device edge
    /// (`cpal_out::SharedRing::from_arc`) wraps; drain it and the movie
    /// plays through the speakers.
    pub fn ring_handle(&self) -> Arc<Mutex<RingSink>> {
        self.ring.clone()
    }

    /// Interleaved s16 samples written into the ring since open.
    pub fn audio_samples_written(&self) -> u64 {
        self.audio_samples_written
    }

    /// Samples dropped by the ring's overwrite-oldest policy (the
    /// device edge draining slower than the decode feeds — audible
    /// only through that edge; 0 while nothing drains and the ring's
    /// capacity still covers a frame's audio).
    pub fn audio_dropped(&self) -> u64 {
        self.ring.lock().unwrap_or_else(|e| e.into_inner()).dropped
    }

    /// Is playback over (skipped or exhausted)?
    pub fn is_ended(&self) -> bool {
        self.ended
    }

    /// Skip the rest of the movie (any user input — module docs).  The
    /// next `advance` reports `ended`.
    pub fn skip(&mut self) {
        self.ended = true;
    }

    /// The per-redraw step: decode every frame the wall clock is due
    /// for (capped at [`MAX_FRAMES_PER_REDRAW`]), convert the last one
    /// to RGBA, and feed track 0's PCM (s16) into the ring.  The first
    /// call starts the clock and decodes frame 0 immediately.
    pub fn advance(&mut self, now: Instant) -> Result<MovieAdvance> {
        let mut adv = MovieAdvance { frames_decoded: 0, ended: self.ended, audio_samples: 0 };
        if self.ended {
            return Ok(adv);
        }
        let started = *self.started.get_or_insert(now);
        let elapsed = now.duration_since(started).as_secs_f64();
        let (num, den) = self.fps();
        while adv.frames_decoded < MAX_FRAMES_PER_REDRAW {
            // Wall-clock pacing: the next frame is due when its PTS is
            // behind the movie clock (PTS = index · den/num — the
            // container has no per-frame timestamps).
            let next_pts = self.player.position() as f64 * den as f64 / num as f64;
            if next_pts > elapsed {
                break;
            }
            match self.player.next_frame()? {
                Some(frame) => {
                    self.rgba = yuv420_to_rgba8(self.player.width(), self.player.height(), &frame.yuv420)?;
                    adv.frames_decoded += 1;
                    self.frames_done += 1;
                    // Audio: track 0's packets (module docs — DESIGN).
                    // Empty slots contribute nothing (the codec's own
                    // demux policy, block 53).
                    if let Some(track) = frame.audio.first() {
                        if !track.samples.is_empty() {
                            let s16 = pcm_f32_to_s16(&track.samples);
                            adv.audio_samples += s16.len();
                            self.audio_samples_written += s16.len() as u64;
                            self.ring
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .write(&s16);
                        }
                    }
                }
                None => {
                    self.ended = true;
                    break;
                }
            }
        }
        adv.ended = self.ended;
        Ok(adv)
    }
}

impl std::fmt::Debug for MoviePlayerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MoviePlayerState")
            .field("dims", &format!("{}x{}", self.width(), self.height()))
            .field("frames", &format!("{}/{}", self.frames_done, self.frame_count()))
            .field("fps", &self.fps())
            .field("ended", &self.ended)
            .field("audio_channels", &self.audio_channels)
            .field("audio_samples_written", &self.audio_samples_written)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// MovieAudioEdge — block 55 (NEXT-UP (a)): the boot movie's audio DEVICE
// edge (feature `audio-cpal`)
// ---------------------------------------------------------------------------

/// The tail-drain CLOSE decision (block 55, pure so it is testable
/// without any device): the device stream closes only when the player
/// is GONE (ended / dropped / failed) AND the ring has fully drained.
/// A LIVE player with a transiently empty ring (a mid-movie underrun)
/// must NEVER close the edge — the next `advance` refills the ring.
#[cfg(feature = "audio-cpal")]
pub fn movie_audio_edge_should_close(player_alive: bool, tail_drained: bool) -> bool {
    !player_alive && tail_drained
}

/// The boot movie's live audio device stream (block 55, feature
/// `audio-cpal`): a real CPAL output opened over
/// [`prototype_audio::cpal_out::SharedRing::from_arc`] of THE ring the
/// player writes — one buffer, one writer
/// ([`MoviePlayerState::advance`]), one reader (the device callback).
/// Dropping the edge closes the stream (audio stops).
///
/// # Lifetime contract (DESIGN — the original's boot-movie audio
/// teardown is not recoverable from the DLL; the dataflow shape is the
/// PROVEN callback-drains-ring model, `cpal_out` module docs)
///
/// * OPEN at movie open — non-fatal: a device-less host (or a refused
///   stream) logs once and the movie plays silent.
/// * HOLD through the last frame: after the movie ends, whatever is
///   left in the ring keeps draining in real time (the tail — at most
///   the ring capacity, ~2.7 s of 48 kHz) instead of being cut; the
///   world scene draws underneath while it drains.
/// * CLOSE when [`movie_audio_edge_should_close`] first says so (the
///   app checks per frame, and the check only ever CLOSES — never
///   re-opens mid-movie), or immediately on user skip / teardown
///   (explicit cuts bypass the drain: the user asked for silence).
///
/// # Honest unknowns
///
/// * Sample-rate policy: the movies are 48 kHz; the device is opened
///   at its DEFAULT rate and no resampling happens
///   (`audio::SampleRateConverter` is not reconstructed) — on a host
///   whose default is not 48 kHz the pitch shifts.  The original's
///   device-rate policy is UNKNOWN (cpal_out module docs).
/// * The Android backend (Oboe/AAudio) is compile-gated behind the NDK
///   and has never run in this sandbox — the host Linux backend (ALSA)
///   carries the behavioural proof here.
#[cfg(feature = "audio-cpal")]
pub struct MovieAudioEdge {
    out: prototype_audio::cpal_out::CpalOut,
    ring: prototype_audio::cpal_out::SharedRing,
}

#[cfg(feature = "audio-cpal")]
impl MovieAudioEdge {
    /// Open the host's default output device over `ring`.  `Err` when
    /// no device exists or the stream is refused — the caller logs and
    /// the movie plays silent (never a boot failure).
    pub fn open(ring: prototype_audio::cpal_out::SharedRing) -> std::result::Result<Self, String> {
        Ok(Self {
            out: prototype_audio::cpal_out::CpalOut::open_default(ring.clone())?,
            ring,
        })
    }

    /// The device config actually in effect: (sample rate Hz, channels).
    /// (`SampleRate` is a plain `u32` alias in cpal 0.18.)
    pub fn config(&self) -> (u32, u16) {
        (self.out.config.sample_rate, self.out.config.channels)
    }

    /// Samples still buffered in the ring as the device sees it.
    /// (`SharedRing::lock` recovers a poisoned mutex itself — no
    /// `unwrap_or_else` here, unlike a raw `Arc<Mutex<_>>`.)
    pub fn ring_buffered(&self) -> usize {
        self.ring.lock().buffered()
    }

    /// TRUE when the ring is empty — the tail-drain close signal
    /// (meaningful only while no player is writing; see the contract).
    pub fn tail_drained(&self) -> bool {
        self.ring_buffered() == 0
    }

    /// Device-callback invocations so far (a live stream ticks this).
    pub fn device_callbacks(&self) -> u64 {
        self.out.stats.callbacks()
    }

    /// Device-callback underruns so far (silence padding the device
    /// needed — the audible symptom of a starved ring).
    pub fn device_underruns(&self) -> u64 {
        self.out.stats.underruns()
    }

    /// CPAL stream errors so far (device loss, format change, ...).
    pub fn device_errors(&self) -> u64 {
        self.out.stats.errors()
    }
}

// ---------------------------------------------------------------------------
// Unit tests — REAL fixtures end to end: the crafted biks are the
// committed intro/silent frame spans (ffmpeg-verified upstream), the
// archives are real RCF bytes written to disk, the player streams
// through `MemberSpanReader` exactly as the app will.
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

    /// Craft a bik from a REAL head + REAL frame spans — the same
    /// surgery the block-51/52/53 ffmpeg-verified references used:
    /// patch frame count (8), largest frame (12) + its copy (16), the
    /// stored file-size field (4, = real size − 8), truncate the head
    /// to 44 + 12·tracks, append the frame table (n spans + sentinel)
    /// and the spans.  Frame 0 keeps its keyframe bit.
    fn craft_bik(head: &[u8], spans: &[&[u8]]) -> Vec<u8> {
        // The audio track count sits at offset 40 (u32): the head's
        // audio block is 12 B per track (maxsize, rate u16, flags u16).
        let tracks = u32::from_le_bytes([head[40], head[41], head[42], head[43]]) as usize;
        let head_len = 44 + 12 * tracks;
        let mut out = head[..head_len].to_vec();
        let largest = spans.iter().map(|s| s.len()).max().unwrap() as u32;
        out[8..12].copy_from_slice(&(spans.len() as u32).to_le_bytes());
        out[12..16].copy_from_slice(&largest.to_le_bytes());
        out[16..20].copy_from_slice(&(spans.len() as u32).to_le_bytes());
        let table_len = 4 * (spans.len() + 1);
        let mut total = (head_len + table_len) as u64;
        for s in spans {
            total += s.len() as u64;
        }
        out[4..8].copy_from_slice(&((total - 8) as u32).to_le_bytes());
        let mut off = (head_len + table_len) as u64;
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

    /// Build a real-layout RCF archive around given members: 60-B
    /// header (magic, PROVEN field shape, version 0x0100_0102 — the
    /// real archives' value), payloads right after the header, the
    /// 12-B/entry directory at the end (sorted ascending by hash —
    /// the real archives' order).  Returns the bytes.
    fn craft_rcf(members: &[(u32, &[u8])]) -> Vec<u8> {
        let mut sorted: Vec<(u32, &[u8])> = members.to_vec();
        sorted.sort_by_key(|(h, _)| *h);
        let mut entries = Vec::new();
        let mut blob = Vec::new();
        let mut off = 60u64;
        for (hash, payload) in &sorted {
            entries.push((*hash, off as u32, payload.len() as u32));
            blob.extend_from_slice(payload);
            off += payload.len() as u64;
        }
        let dir_offset = off as u32;
        let dir_size = (12 * sorted.len()) as u32;
        let mut out = Vec::with_capacity(60 + blob.len() + dir_size as usize);
        out.extend_from_slice(b"ATG CORE CEMENT LIBRARY");
        out.extend_from_slice(&[0u8; 9]); // magic NUL padding to 32 B
        assert_eq!(out.len(), 32);
        for v in [
            0x0100_0102u32,
            dir_offset,
            dir_size,
            2048, // sector size (the movies/audio archives' value)
            0,    // name table offset (zero region in observed samples)
            0,    // reserved
            sorted.len() as u32,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(out.len(), 60);
        out.extend_from_slice(&blob);
        for (hash, off, size) in entries {
            out.extend_from_slice(&hash.to_le_bytes());
            out.extend_from_slice(&off.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
        }
        out
    }

    /// A temp dir per test (the game_files tests' pattern — real disk
    /// I/O, isolated).
    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "proto-android-movie-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The intro's REAL head + its REAL frame-1 span as a one-frame
    /// bik member (frame 1 = real inter content with real audio —
    /// 3840 f32 on track 0, pinned to ffmpeg's s16 in block 52).
    fn intro_one_frame() -> Vec<u8> {
        let head = fixture("movies_bik_1e218528_head22980.bin");
        let s1 = fixture("movies_bik_1e218528_frame1_7644.bin");
        craft_bik(&head, &[&s1])
    }

    const INTRO_BIK_HASH: u32 = 0x1E21_8528;

    // ------------------------------------------------------------------
    // Block 67: the NAMED movie finder
    // ------------------------------------------------------------------

    /// The corpus anchor of the name convention (block 67, PROVEN):
    /// `movies\woi\woi_ec1.bik` must hash to the REAL 00woi.rcf
    /// directory entry 0x32824477 (the 84 MB member) — verified
    /// empirically against the release asset's directory and its
    /// embedded name table (the WOI archives carry real name
    /// records; the record's 4th field is a build timestamp, not a
    /// hash).
    #[test]
    fn movie_member_hashes_reproduce_the_real_woi_entry() {
        assert_eq!(
            prototype_rcf::hash_name("movies\\woi\\woi_ec1.bik"),
            0x3282_4477
        );
        let hashes = movie_member_hashes("woi_ec1");
        assert!(hashes.contains(&0x3282_4477), "the PROVEN candidate first");
        assert!(hashes.len() >= 2, "the other conventions ride along");
        // Distinct paths hash distinctly (no accidental dedup).
        assert_ne!(
            prototype_rcf::hash_name("movies\\woi\\woi_ec1.bik"),
            prototype_rcf::hash_name("movies\\woi_ec1.bik")
        );
    }

    #[test]
    fn find_movie_by_name_serves_a_named_member_from_a_woi_archive() {
        let root = temp_root("byname");
        // A REAL bik head under the PROVEN WOI member path, in a
        // 00woi.rcf-styled archive (craft_rcf hashes exactly what the
        // real loader would).
        let head = fixture("movies_bik_17a2ee41_head256.bin");
        let path = root.join("00woi.rcf");
        std::fs::write(
            &path,
            craft_rcf(&[(prototype_rcf::hash_name("movies\\woi\\woi_ec1.bik"), &head)]),
        )
        .unwrap();
        let found = find_movie_by_name(&[path.clone()], "woi_ec1")
            .expect("the PROVEN member path resolves");
        assert_eq!(found.member_hash, 0x3282_4477);
        assert_eq!(found.archive, path);
        assert_eq!(found.entry.file_size as usize, head.len());

        // An unknown name is an honest None — no guess, no panic.
        assert!(find_movie_by_name(&[path], "no_such_movie").is_none());
    }

    #[test]
    fn find_movie_by_name_refuses_non_movie_members_and_archives() {
        let root = temp_root("byname2");
        // The hash hits, but the payload is NOT a bik → refused.
        let junk_path = root.join("00woi_junk.rcf");
        std::fs::write(
            &junk_path,
            craft_rcf(&[(prototype_rcf::hash_name("movies\\woi\\woi_ec1.bik"), b"not a bik")]),
        )
        .unwrap();
        // A non-movie/woi archive is never scanned (the art/audio
        // contract — its directory can hold tens of thousands).
        let real_head = fixture("movies_bik_17a2ee41_head256.bin");
        let art_path = root.join("00art.rcf");
        std::fs::write(
            &art_path,
            craft_rcf(&[(
                prototype_rcf::hash_name("movies\\woi\\woi_ec1.bik"),
                &real_head,
            )]),
        )
        .unwrap();
        assert!(find_movie_by_name(&[junk_path, art_path], "woi_ec1").is_none());
    }

    #[test]
    fn find_boot_movie_finds_the_smallest_bik_in_a_movie_archive() {
        let root = temp_root("find");
        let bik = intro_one_frame();
        // Two members: a fake non-bik payload and the real bik.  The
        // fake hash sorts AFTER the bik's (the directory is hash-sorted
        // like the real archives), so the bik's payload sits at 60.
        let rcf = craft_rcf(&[
            (0xFFFF_FFF1, b"not a movie payload at all"),
            (INTRO_BIK_HASH, &bik),
        ]);
        let path = root.join("movies_boot_test.rcf");
        std::fs::write(&path, &rcf).unwrap();
        let found = find_boot_movie(&[path.clone()]).expect("the bik member must be found");
        assert_eq!(found.archive, path);
        assert_eq!(found.member_hash, INTRO_BIK_HASH);
        assert_eq!(found.entry.file_offset as u64, 60, "payload starts after the header");
        assert_eq!(found.entry.file_size as usize, bik.len());
    }

    #[test]
    fn find_boot_movie_prefers_the_smallest_of_two_biks() {
        let root = temp_root("smallest");
        let intro = intro_one_frame();
        // A second REAL movie: the silent movie's head + frame 0 span
        // (a larger member than the intro one-frame).
        let silent_head = fixture("movies_bik_17a2ee41_head256.bin");
        let silent_f0 = fixture("movies_bik_17a2ee41_frame0_48932.bin");
        let silent = craft_bik(&silent_head, &[&silent_f0]);
        assert!(silent.len() > intro.len());
        let big_hash = 0x17A2_EE41u32;
        let rcf = craft_rcf(&[(big_hash, &silent), (INTRO_BIK_HASH, &intro)]);
        let path = root.join("movies_two.rcf");
        std::fs::write(&path, &rcf).unwrap();
        let found = find_boot_movie(&[path]).expect("a bik must be found");
        assert_eq!(
            found.member_hash, INTRO_BIK_HASH,
            "the smallest bik member wins (fastest first pixel)"
        );
    }

    #[test]
    fn find_boot_movie_ignores_non_movie_archives() {
        let root = temp_root("policy");
        let bik = intro_one_frame();
        // The SAME content under a non-movie name — policy says the
        // boot scan does not open art/cells/audio archives.
        let path = root.join("00art.rcf");
        std::fs::write(&path, craft_rcf(&[(INTRO_BIK_HASH, &bik)])).unwrap();
        assert!(find_boot_movie(&[path]).is_none());
        // And an archive whose stem merely contains "movies" matches
        // (the real archive is movies.rcf).
        let path2 = root.join("movies_two.rcf");
        std::fs::write(&path2, craft_rcf(&[(INTRO_BIK_HASH, &bik)])).unwrap();
        assert!(find_boot_movie(&[path2]).is_some());
    }

    #[test]
    fn find_boot_movie_no_candidates_is_none() {
        let root = temp_root("none");
        let path = root.join("movies_empty.rcf");
        std::fs::write(&path, craft_rcf(&[(1, b"just a p3d-ish payload")])).unwrap();
        assert!(find_boot_movie(&[path]).is_none());
        // A missing archive file is skipped, not a panic.
        let missing = root.join("movies_gone.rcf");
        assert!(find_boot_movie(&[missing]).is_none());
    }

    /// The full APP path on REAL bytes: find → open (span window, no
    /// extraction) → advance → RGBA + ring, twice (frame 0 then end).
    #[test]
    fn player_state_walks_the_real_intro_frame_through_rgba_and_ring() {
        let root = temp_root("walk");
        let bik = intro_one_frame();
        let path = root.join("movies_walk.rcf");
        std::fs::write(&path, craft_rcf(&[(INTRO_BIK_HASH, &bik)])).unwrap();
        let movie = find_boot_movie(&[path]).unwrap();
        let mut state = MoviePlayerState::open(&movie).expect("the member opens");
        assert_eq!(state.width(), 1280);
        assert_eq!(state.height(), 544);
        assert_eq!(state.frame_count(), 1);
        assert_eq!(state.audio_channels(), 2, "intro track 0 is stereo");

        // First advance: starts the clock, decodes frame 0.
        let t0 = Instant::now();
        let adv = state.advance(t0).expect("frame 0 decodes");
        assert_eq!(adv.frames_decoded, 1);
        assert!(!adv.ended);
        assert_eq!(adv.audio_samples, 3_840, "frame 1's track-0 f32 count → s16");
        assert_eq!(state.rgba().len(), 1280 * 544 * 4);
        assert_eq!(state.frames_done(), 1);
        assert_eq!(state.audio_samples_written(), 3_840);
        // The ring received exactly what was written — asserted via
        // `produced` (block 75): the write-side counter a live device
        // edge can never reduce, so the check is race-free even if a
        // device ever attaches to this state.
        assert_eq!(state.ring_handle().lock().unwrap().produced(), 3_840);
        assert_eq!(state.audio_dropped(), 0);

        // Second advance (same instant): frame 1's PTS (100/2997 ≈ 33 ms)
        // is still in the future — the movie is PLAYING, nothing decodes.
        let adv = state.advance(t0).expect("not due is not an error");
        assert_eq!(adv.frames_decoded, 0);
        assert!(!adv.ended, "a one-frame movie still plays for its duration");
        // Past the last frame's PTS: the index is exhausted → end.
        let adv = state
            .advance(t0 + std::time::Duration::from_millis(40))
            .expect("end is not an error");
        assert_eq!(adv.frames_decoded, 0);
        assert!(adv.ended);
        assert!(state.is_ended());
    }

    /// Wall-clock pacing: a TWO-frame real bik decodes frame 0 at t0,
    /// NOTHING at t0+1 ms (frame 1's PTS is 100/2997 s away), frame 1
    /// once due, then ends.
    #[test]
    fn player_state_paces_frames_by_wall_clock() {
        let root = temp_root("pace");
        let head = fixture("movies_bik_1e218528_head22980.bin");
        let s0 = fixture("movies_bik_1e218528_frame0_19324.bin");
        let s1 = fixture("movies_bik_1e218528_frame1_7644.bin");
        let path = root.join("movies_pace.rcf");
        std::fs::write(&path, craft_rcf(&[(INTRO_BIK_HASH, &craft_bik(&head, &[&s0, &s1]))]))
            .unwrap();
        let movie = find_boot_movie(&[path]).unwrap();
        let mut state = MoviePlayerState::open(&movie).unwrap();
        assert_eq!(state.frame_count(), 2);

        let t0 = Instant::now();
        let adv = state.advance(t0).unwrap();
        assert_eq!(adv.frames_decoded, 1, "frame 0 (PTS 0) is due immediately");
        // 1 ms later: frame 1's PTS (100/2997 ≈ 33.4 ms) is in the future.
        let adv = state.advance(t0 + std::time::Duration::from_millis(1)).unwrap();
        assert_eq!(adv.frames_decoded, 0, "not due yet");
        assert!(!adv.ended);
        // Past frame 1's PTS: decode, then the index is exhausted —
        // but the movie STAYS up until its full duration (2 frames ≈
        // 66.7 ms) has elapsed.
        let adv = state.advance(t0 + std::time::Duration::from_millis(40)).unwrap();
        assert_eq!(adv.frames_decoded, 1);
        assert!(!adv.ended, "mid-movie: frame 2's PTS is still ahead");
        assert_eq!(state.frames_done(), 2);
        // Both frames' track-0 audio landed (72 960 + 3 840 s16).
        assert_eq!(state.audio_samples_written(), 72_960 + 3_840);
        // The ring never dropped: 76 800 < capacity 131 072.
        assert_eq!(state.audio_dropped(), 0);
        // Past the LAST frame's PTS: exhaustion.
        let adv = state.advance(t0 + std::time::Duration::from_millis(80)).unwrap();
        assert_eq!(adv.frames_decoded, 0);
        assert!(adv.ended);
    }

    /// Skip: any user input ends playback on the next advance.
    #[test]
    fn skip_ends_playback_on_the_next_advance() {
        let root = temp_root("skip");
        let path = root.join("movies_skip.rcf");
        std::fs::write(&path, craft_rcf(&[(INTRO_BIK_HASH, &intro_one_frame())])).unwrap();
        let movie = find_boot_movie(&[path]).unwrap();
        let mut state = MoviePlayerState::open(&movie).unwrap();
        state.skip();
        let adv = state.advance(Instant::now()).unwrap();
        assert_eq!(adv.frames_decoded, 0);
        assert!(adv.ended);
        // Nothing decoded, nothing fed.
        assert_eq!(state.frames_done(), 0);
        assert_eq!(state.audio_samples_written(), 0);
    }

    /// The RDFT fail-closed gate survives the app path: a movie whose
    /// track 0 lost the DCT flag refuses to open (the app plays
    /// without a movie rather than decode garbage).
    #[test]
    fn rdft_track_fails_closed_through_the_app_path() {
        let root = temp_root("rdft");
        let mut bik = intro_one_frame();
        // Track 0's flags u16 = 44 + 16 + 0 + 2 = 62 (the block-53
        // player test's proven offset).
        let fl = 62usize;
        assert_eq!((bik[fl] as u16) | ((bik[fl + 1] as u16) << 8), 0x7000);
        bik[fl] = 0x00;
        bik[fl + 1] = 0x60; // 16-bit | stereo, DCT cleared → RDFT
        let path = root.join("movies_rdft.rcf");
        std::fs::write(&path, craft_rcf(&[(INTRO_BIK_HASH, &bik)])).unwrap();
        let movie = find_boot_movie(&[path]).unwrap();
        let err = MoviePlayerState::open(&movie).unwrap_err();
        assert!(format!("{err}").contains("RDFT"), "{err}");
    }

    // --- block 55: the audio DEVICE edge (feature `audio-cpal`) -----
    //    The close policy is device-free; the drain test runs the REAL
    //    device path (skipped where no output device exists — the pure
    //    cpal_out tests carry the correctness burden there).

    #[cfg(feature = "audio-cpal")]
    mod audio_edge {
        use super::*;

        /// The close policy: a LIVE player never closes the edge — not
        /// even on a transiently empty ring (a mid-movie underrun; the
        /// next advance refills).  A GONE player closes exactly when
        /// the ring has fully drained.
        #[test]
        fn the_close_policy_never_cuts_a_live_movie() {
            assert!(!movie_audio_edge_should_close(true, false));
            assert!(
                !movie_audio_edge_should_close(true, true),
                "a live player + empty ring is an underrun, not a close"
            );
            assert!(!movie_audio_edge_should_close(false, false), "tail not drained yet");
            assert!(movie_audio_edge_should_close(false, true), "gone + drained → close");
        }

        /// The FULL app audio path on REAL bytes: find → open → one
        /// advance (3840 s16 on track 0) → the device edge opened over
        /// THE player's ring drains it in real time (callbacks tick,
        /// no stream errors, the ring ends empty).  Skips where the
        /// host has no output device or refuses the stream.
        #[test]
        fn the_edge_drains_the_players_real_ring_end_to_end() {
            let root = temp_root("edge");
            let path = root.join("movies_edge.rcf");
            std::fs::write(&path, craft_rcf(&[(INTRO_BIK_HASH, &intro_one_frame())])).unwrap();
            let movie = find_boot_movie(&[path]).unwrap();
            let mut state = MoviePlayerState::open(&movie).expect("the member opens");
            let adv = state.advance(Instant::now()).expect("frame 0 decodes");
            assert_eq!(adv.audio_samples, 3_840, "frame 1's track-0 f32 count → s16");
            assert_eq!(state.audio_samples_written(), 3_840);

            let edge = match MovieAudioEdge::open(prototype_audio::cpal_out::SharedRing::from_arc(
                state.ring_handle(),
            )) {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("audio edge e2e: no device / stream refused ({e}) — skipped");
                    return;
                }
            };
            let (rate, ch) = edge.config();
            eprintln!("audio edge e2e: device open at {rate} Hz × {ch} ch");

            // The player is gone (dropped here — the app would advance
            // no further); the tail drains in real time.  3840 stereo
            // samples = ~40 ms at 48 kHz; 3 s is a generous bound.
            let deadline = Instant::now() + std::time::Duration::from_millis(3_000);
            while Instant::now() < deadline {
                if movie_audio_edge_should_close(false, edge.tail_drained())
                    && edge.device_callbacks() > 0
                {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(
                edge.device_callbacks() > 0,
                "device callback never ran — the audio path is dead"
            );
            assert!(edge.tail_drained(), "the player's 3840 samples never drained");
            assert_eq!(edge.device_errors(), 0, "the stream reported errors");
            eprintln!(
                "audio edge e2e: {} callback(s), {} underrun(s), drained clean",
                edge.device_callbacks(),
                edge.device_underruns()
            );
        }

        /// A device-less host plays SILENT, never fails the boot:
        /// `MovieAudioEdge::open` surfaces `Err` (probed by forcing a
        /// closed device through an explicit host that has none —
        /// here we only pin that `open`'s error path is `String`, so
        /// the app's non-fatal handling type-checks against it).
        #[test]
        fn the_edge_error_path_is_a_string_not_a_panic() {
            // Compile-shape pin: the error type is String (the app logs
            // it).  On a device-less host the same call returns Err and
            // the boot continues — the drain test above skips there.
            fn takes_err(e: String) -> String {
                e
            }
            let ring = prototype_audio::cpal_out::SharedRing::new(
                prototype_audio::sink::RingSink::new(16),
            );
            match MovieAudioEdge::open(ring) {
                Ok(_) => {} // device present: the drain test covers it
                Err(e) => {
                    let _ = takes_err(e); // the app's non-fatal path
                }
            }
        }
    }
}

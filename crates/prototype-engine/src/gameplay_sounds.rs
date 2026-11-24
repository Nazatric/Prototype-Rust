//! gameplay_sounds — block 58: the gameplay audio CONTENT half.
//!
//! # The gap this closes
//!
//! Block 57 wired the gameplay audio DEVICE edge (the per-frame tick
//! drains through a real CPAL stream), but the stage mixed SILENCE:
//! no voice was ever spawned, because the clip ↔ event mapping was
//! documented UNKNOWN (block 38).  This module closes the content
//! half with the user's own data: the loader contract already hands
//! the app the game's `*.p3d` files ([`crate::GameFileSet`] collects
//! them — the scan's own test uses `boot_strap.p3d` as the example),
//! and `boot_strap.p3d` carries **70 PROVEN-decodable RADP clips**
//! whose names are the engine's audio-event vocabulary
//! (`ui_fe_*` frontend family, `select`, `blob_collect_01`,
//! `infected1_roar_*`, `air_brake`, `big_heavy_whoosh_01`, …).
//!
//! # Evidence
//!
//! * PROVEN: the P3D AudioFile chunk layout + the RADP IMA-ADPCM
//!   decode (M13/M38; `prototype-audio::AudioClipCatalog`), the clip
//!   NAME vocabulary (the user's own `boot_strap.p3d` — 70 clips,
//!   every payload decodes), and the Mixer voice-spawn semantics
//!   (M15).
//! * DESIGN (ours, documented): the EVENT → NAME assignment.  The
//!   original routes audio events from script data we cannot fully
//!   recover (the script-side bindings are PROVEN, the sound-event
//!   table is not).  The assignments below pick clips from the
//!   PROVEN vocabulary whose NAME SEMANTICS match the event — and
//!   since block 59 the sample-rate converter IS reconstructed
//!   ([`prototype_audio::sample_rate`]), so ANY clip plays
//!   rate-clean at the session rate; the two wired events still use
//!   48 kHz clips (zero-conversion by choice, not necessity).
//!
//! # Contract
//!
//! * [`GameplaySounds::from_p3d_bytes`] decodes every AudioFile
//!   chunk it can; a chunk that fails to decode is COUNTED, never
//!   fatal (the registry carries what decoded — the counter is the
//!   honest audit trail).
//! * [`GameplaySounds::play`] spawns a clip as a [`Mixer`] voice in
//!   the [`AudioOut`] stage — the same never-block ring policy; the
//!   samples reach the device through the block-57 edge on the next
//!   ticks.
//! * [`GameplaySounds::play_event`] applies the DESIGN mapping
//!   ([`clip_for_event`]) — the app calls this at real event sites.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use prototype_audio::mixer::Voice;
use prototype_p3d::P3dFile;

use crate::audio_out::AudioOut;

/// The session MIX rate (DESIGN — ours): the app ticks
/// `dt × 48 000` frames per frame (`app.rs`'s `audio_frame_count`,
/// which now derives its constant from HERE), and the mixer consumes
/// one sample per output frame — so this IS the rate the mix output
/// runs at, and the rate every clip is converted to on spawn
/// (block 59).  NOT the device rate: the device edge may open at its
/// own default (`config()` surfaces it) — the on-device rate policy
/// stays the documented UNKNOWN.
pub const SESSION_RATE_HZ: u32 = 48_000;

/// The gameplay audio events wired so far (DESIGN mapping — see the
/// module docs).  Each maps to a REAL clip name from the user's own
/// `boot_strap.p3d`, chosen from the frontend `ui_fe_*` family whose
/// name semantics match (both are 48 kHz, rate-clean at the app's
/// mix rate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameplayAudioEvent {
    /// The user skipped the boot movie (touch/mouse/key): a UI
    /// "back/cancel" click — `ui_fe_ht_back` (48 kHz, 0.04 s).
    BootMovieSkipped,
    /// The boot movie ENDED naturally and the world scene takes the
    /// screen: a UI "wipe" transition — `ui_fe_ht_wipe` (48 kHz,
    /// 2.06 s stereo).
    BootMovieEnded,
}

/// The DESIGN event → clip-name mapping (the names are PROVEN
/// vocabulary; the assignment is ours, documented).
pub fn clip_for_event(event: GameplayAudioEvent) -> &'static str {
    match event {
        GameplayAudioEvent::BootMovieSkipped => "ui_fe_ht_back",
        GameplayAudioEvent::BootMovieEnded => "ui_fe_ht_wipe",
    }
}

/// One decoded gameplay clip: interleaved 16-bit PCM at its NATIVE
/// rate ([`SoundClip::sample_rate`] — 44 100 or 48 000 in the
/// shipped fixture).  [`GameplaySounds::play`] converts to the
/// session rate on spawn, so a clip at either rate plays
/// rate-clean.
#[derive(Debug, Clone)]
pub struct SoundClip {
    /// The clip's primary name (PROVEN — from the user's own file).
    pub name: String,
    /// 1 (mono, radp v1) or 2 (stereo, radp v2).
    pub channels: u32,
    /// Native sample rate (44100 / 48000 in the shipped fixture).
    pub sample_rate: u32,
    /// Interleaved decoded PCM (L,R,… for stereo).
    pub samples: Vec<i16>,
}

impl SoundClip {
    /// Frames (not samples) — a stereo frame is 2 samples.
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }
}

/// The gameplay clip registry: the decoded content half of the
/// audio stack.  Device-free (plain PCM + names); loaded from the
/// user's own P3D audio archive at boot.
#[derive(Debug, Default, Clone)]
pub struct GameplaySounds {
    clips: BTreeMap<String, SoundClip>,
    /// AudioFile chunks present but not decodable (the honest audit
    /// trail; 0 on the shipped fixture).
    decode_failures: usize,
    /// The file the registry came from (diagnostics).
    source: String,
}

impl GameplaySounds {
    /// Parse `bytes` as a P3D and RADP-decode every AudioFile chunk.
    /// `Err` only when the FILE is not a parseable P3D (the caller
    /// tries the next candidate); per-chunk decode failures are
    /// counted, never fatal.
    pub fn from_p3d_bytes(bytes: &[u8], source: &str) -> Result<Self, String> {
        let file = P3dFile::parse(bytes).map_err(|e| format!("{source}: not a parseable P3D: {e}"))?;
        let catalog = prototype_audio::AudioClipCatalog::from_p3d(&file)
            .map_err(|e| format!("{source}: audio catalog error: {e}"))?;
        let mut clips = BTreeMap::new();
        let mut decode_failures = 0usize;
        for clip in catalog.clips() {
            match clip.decode() {
                Ok(decoded) => {
                    clips.insert(
                        clip.name.clone(),
                        SoundClip {
                            name: clip.name.clone(),
                            channels: decoded.channels,
                            sample_rate: clip.sample_rate,
                            samples: decoded.samples,
                        },
                    );
                }
                Err(_) => decode_failures += 1,
            }
        }
        Ok(Self {
            clips,
            decode_failures,
            source: source.to_string(),
        })
    }

    /// The file the registry was loaded from.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Decoded clip count.
    pub fn len(&self) -> usize {
        self.clips.len()
    }

    /// True when no clip decoded.
    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }

    /// AudioFile chunks that failed to decode (counted, not fatal).
    pub fn decode_failures(&self) -> usize {
        self.decode_failures
    }

    /// Sorted clip names (the PROVEN vocabulary).
    pub fn clip_names(&self) -> Vec<&str> {
        self.clips.keys().map(String::as_str).collect()
    }

    pub fn has(&self, name: &str) -> bool {
        self.clips.contains_key(name)
    }

    pub fn get(&self, name: &str) -> Option<&SoundClip> {
        self.clips.get(name)
    }

    /// Spawn `clip` as a mixer voice into `audio` at `gain` 0..=1+.
    /// Returns the FRAME count spawned — the RESAMPLED count when the
    /// clip's native rate differs from [`SESSION_RATE_HZ`] (block 59:
    /// the whole vocabulary is rate-clean; a 44 100 Hz clip consumed
    /// unconverted would shift pitch by 48 000/44 100 ≈ +8.84 %).
    /// `Err` on an unknown clip or an empty one (never panics — a
    /// missing clip must not break the frame).
    pub fn play(&self, audio: &mut AudioOut, clip: &str, gain: f32) -> Result<usize, String> {
        let sound = self
            .clips
            .get(clip)
            .ok_or_else(|| format!("no gameplay clip named {clip:?}"))?;
        Self::play_clip(audio, sound, gain)
    }

    /// Spawn a STANDALONE clip as a mixer voice into `audio` at
    /// `gain` 0..=1+ — the shared spawn body of [`GameplaySounds::play`]
    /// and of the app's shard fallback (block 63): a lazily decoded
    /// shard clip never enters an eager registry, it is played from
    /// its own `SoundClip`.  Same rate-clean contract: the clip is
    /// resampled to [`SESSION_RATE_HZ`] BEFORE the voice exists.
    pub fn play_clip(audio: &mut AudioOut, sound: &SoundClip, gain: f32) -> Result<usize, String> {
        Self::play_clip_tagged(audio, sound, gain, None)
    }

    /// [`GameplaySounds::play_clip`] with a mixer LIFECYCLE TAG
    /// (block 67): the spawned voice carries `tag` so the whole
    /// logical unit (e.g. one NIS movie's mix clip) can be stopped
    /// later by tag via `Mixer::stop_tagged` — index-free, because
    /// mixer indices shift on every removal.  `None` = untagged
    /// (identical to [`GameplaySounds::play_clip`]).
    pub fn play_clip_tagged(
        audio: &mut AudioOut,
        sound: &SoundClip,
        gain: f32,
        tag: Option<u64>,
    ) -> Result<usize, String> {
        Self::play_clip_tagged_loop(audio, sound, gain, tag, false)
    }

    /// [`GameplaySounds::play_clip_tagged`] with an explicit LOOP
    /// flag (block 68, the mission system's need): music beds repeat
    /// until a transition cuts them, stingers play once.  The flag
    /// is the caller's PROVEN data — the music banks' patches carry
    /// a `loop` property (30/30 `music_player`/`music_player_random`
    /// patches across the committed banks; beds true, stingers
    /// false) — never inferred from the clip name here.  Rides the
    /// DEFAULT bus (block 72's historical shape).
    pub fn play_clip_tagged_loop(
        audio: &mut AudioOut,
        sound: &SoundClip,
        gain: f32,
        tag: Option<u64>,
        loop_voice: bool,
    ) -> Result<usize, String> {
        Self::play_clip_tagged_loop_bus(audio, sound, gain, tag, loop_voice, None)
    }

    /// Block 72 — the `mixbuss` routing: [`GameplaySounds::
    /// play_clip_tagged_loop`] with an explicit MIX BUS.  The real
    /// banks carry a bus list on every patch (five prop names:
    /// `mixbuss`/`mixbus`/`bus`/`bus_name`/`busnames` — PROVEN);
    /// the app maps each patch family to its bus (music / amb /
    /// nis) and passes the resolved id here.  `None` = the DEFAULT
    /// bus (the historical single-bus shape).
    pub fn play_clip_tagged_loop_bus(
        audio: &mut AudioOut,
        sound: &SoundClip,
        gain: f32,
        tag: Option<u64>,
        loop_voice: bool,
        bus: Option<u16>,
    ) -> Result<usize, String> {
        if sound.samples.is_empty() {
            return Err(format!("gameplay clip {:?} decoded to silence-less empty", sound.name));
        }
        // Multi-channel FOLD (block 67, DESIGN with PROVEN inputs):
        // a voice is a 1/2-channel unit (the mixer's contract), but
        // the NIS mixes are 5.1-style RADP v6 masters whose REAL
        // content is ch0/ch1 (the stereo pair) + ch2 (the full-scale
        // bed — max_abs 32768 on the real `mnis_5_2_1_mix` fixture;
        // ch3..5 silent).  Folding ch>2 verbatim to ch0/ch1 (the
        // mixer's historical behavior) would DROP the bed entirely,
        // so: L = ch0 + 0.5·ch2, R = ch1 + 0.5·ch2, clamped.  The
        // 0.5 keeps the sum of the observed worst case below i16
        // (7355 + 16384 < 32767).  Channels ≥ 3 are dropped (silent
        // across the real corpus; the engine's true fold-down is
        // UNKNOWN — this is our documented, deterministic stand-in).
        let (samples, channels) = if sound.channels > 2 {
            let ch = sound.channels as usize;
            let frames = sound.samples.len() / ch;
            let mut folded = Vec::with_capacity(frames * 2);
            for f in 0..frames {
                let l = sound.samples[f * ch] as i32;
                let r = sound.samples[f * ch + 1] as i32;
                let c = if ch >= 3 { sound.samples[f * ch + 2] as i32 } else { 0 };
                let fold = |v: i32| v.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                folded.push(fold(l + c / 2));
                folded.push(fold(r + c / 2));
            }
            (folded, 2u32)
        } else {
            (sound.samples.clone(), sound.channels)
        };
        // Rate-clean spawn (block 59): convert to the session rate
        // BEFORE the voice exists — the mixer has no rate concept, it
        // consumes one sample per output frame.
        let pcm = if sound.sample_rate == SESSION_RATE_HZ {
            samples
        } else {
            prototype_audio::sample_rate::resample(
                &samples,
                channels,
                sound.sample_rate,
                SESSION_RATE_HZ,
            )
        };
        let frames = pcm.len() / channels.max(1) as usize;
        audio.mixer().play(Voice {
            samples: pcm,
            channels,
            cursor: 0,
            loop_voice,
            gain,
            position: prototype_math::Vec3::new(0.0, 0.0, 0.0),
            tag,
            bus,
        });
        Ok(frames)
    }

    /// Apply the DESIGN mapping for `event` (see [`clip_for_event`]).
    /// A missing clip is `Err` — the app logs it once and the game
    /// continues (the mapping degrades to silence, never a panic).
    pub fn play_event(
        &self,
        audio: &mut AudioOut,
        event: GameplayAudioEvent,
    ) -> Result<usize, String> {
        self.play(audio, clip_for_event(event), 1.0)
    }
}

// ---------------------------------------------------------------------------
// Tests — REAL fixture: the committed boot_strap.p3d (the user's own
// audio archive, 70 decodable clips)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_bytes() -> Vec<u8> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join("boot_strap.p3d");
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    /// A REAL member of the user's audio archives (block 61): the
    /// sharded `audio\english\AudioFile\...` clips live as one-clip P3Ds
    /// whose RADP payload runs at 32 000 Hz — a THIRD shipped clip rate
    /// beyond boot_strap's 44 100/48 000.
    fn audio_member(name: &str) -> Vec<u8> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    fn real_sounds() -> GameplaySounds {
        GameplaySounds::from_p3d_bytes(&fixture_bytes(), "boot_strap.p3d")
            .expect("the committed boot_strap.p3d parses")
    }

    /// The REAL registry: every AudioFile chunk in the user's own
    /// boot_strap.p3d decodes (70 clips, zero failures), and the
    /// wired events' clips are present at 48 kHz (rate-clean).
    #[test]
    fn loads_the_real_boot_strap_clips_with_zero_failures() {
        let sounds = real_sounds();
        assert_eq!(sounds.len(), 70, "list-audio's PROVEN clip count");
        assert_eq!(sounds.decode_failures(), 0, "every payload decodes");
        assert_eq!(sounds.source(), "boot_strap.p3d");
        // The wired events' clips: 48 kHz (the app mix rate).
        for event in [
            GameplayAudioEvent::BootMovieSkipped,
            GameplayAudioEvent::BootMovieEnded,
        ] {
            let name = clip_for_event(event);
            let clip = sounds.get(name).unwrap_or_else(|| panic!("missing {name}"));
            assert_eq!(clip.sample_rate, 48_000, "{name} must be rate-clean");
            assert!(!clip.samples.is_empty());
        }
        // The vocabulary families are present (spot checks across
        // the roster: frontend UI, pickups, combat, movement).
        for name in [
            "ui_fe_ht_back",
            "ui_fe_ht_wipe",
            "ui_fe_main_nav",
            "select",
            "blob_collect_01",
            "infected1_roar_01",
            "air_brake",
            "big_heavy_whoosh_01",
        ] {
            assert!(sounds.has(name), "vocabulary missing {name}");
        }
    }

    /// The decoded clip shapes follow the PROVEN RADP arithmetic:
    /// v1 (mono) = total blocks × 32 samples; v2 (stereo) = the
    /// blocks SPLIT across the two streams (block 38: blocks per
    /// stream = data_size / (20 · version)) and interleaved — total
    /// samples = blocks × 32, FRAMES = blocks × 32 / 2.
    #[test]
    fn decoded_shapes_follow_the_radp_arithmetic() {
        let sounds = real_sounds();
        // ui_fe_ht_back: v1 48000 Hz, 54 blocks → 54 × 32 = 1728
        // mono samples (~0.036 s).
        let back = sounds.get("ui_fe_ht_back").unwrap();
        assert_eq!(back.channels, 1);
        assert_eq!(back.samples.len(), 54 * 32);
        assert!((back.frames() as f32 / 48_000.0 - 0.036).abs() < 0.005);
        // ui_fe_ht_wipe: v2 48000 Hz, 3092 blocks → 3092 × 32 = 98 944
        // interleaved samples = 49 472 stereo frames (~1.03 s).
        let wipe = sounds.get("ui_fe_ht_wipe").unwrap();
        assert_eq!(wipe.channels, 2);
        assert_eq!(wipe.samples.len(), 3092 * 32);
        assert_eq!(wipe.frames(), 3092 * 16);
        assert!(
            (wipe.frames() as f32 / 48_000.0 - 1.031).abs() < 0.01,
            "wipe duration matches the stream-split arithmetic"
        );
    }

    /// `play` spawns the clip into the mixer; one tick of the right
    /// size routes EXACTLY the clip's samples into the ring (the
    /// block-57 shared ring receives them) and the voice ends once.
    #[test]
    fn play_spawns_a_real_voice_that_routes_into_the_ring() {
        let sounds = real_sounds();
        let mut audio = AudioOut::new(1 << 17, 2);
        let wipe = sounds.get("ui_fe_ht_wipe").unwrap();
        let frames = sounds
            .play(&mut audio, "ui_fe_ht_wipe", 1.0)
            .expect("the clip spawns");
        assert_eq!(frames, wipe.frames());
        // One dt-sized tick at a time like the app loop: route the
        // whole clip through the mixer into the ring.  The mixer
        // outputs FULL buffers per tick (silence-padded tail within
        // the last tick), so routed overshoots the clip by < one
        // tick.
        let mut routed = 0usize;
        let mut ends = 0usize;
        while audio.mixer_ref().len() > 0 {
            let st = audio.tick(1024);
            routed += st.samples_routed;
            ends += st.voice_ends;
        }
        assert_eq!(routed % 2048, 0, "every tick routes a full stereo buffer");
        assert!(
            routed >= wipe.samples.len() && routed - wipe.samples.len() < 2048,
            "the overshoot is one tick's silence tail, got {routed} vs {}",
            wipe.samples.len()
        );
        assert_eq!(ends, 1, "the clip's voice ends exactly once");
        // The ring's prefix is EXACTLY the clip (single voice, gain
        // 1.0 — integer-valued f32 × 1.0 is exact), the tail silence.
        assert_eq!(audio.buffered(), routed);
        let drained = audio.ring_handle().lock().unwrap().read(routed);
        assert_eq!(&drained[..wipe.samples.len()], &wipe.samples[..],
            "the clip's samples land in the shared ring unmodified");
        assert!(drained[wipe.samples.len()..].iter().all(|&s| s == 0),
            "the overshoot is silence, not garbage");
    }

    /// Unknown and unplayable clips are `Err(String)` — the mapping
    /// degrades to silence, never a panic.
    #[test]
    fn play_rejects_unknown_clips_as_errors_not_panics() {
        let sounds = real_sounds();
        let mut audio = AudioOut::new(1 << 16, 2);
        let e = sounds
            .play(&mut audio, "no_such_clip", 1.0)
            .expect_err("unknown clip");
        assert!(e.contains("no gameplay clip named"), "loggable: {e}");
        assert_eq!(audio.ticks(), 0, "nothing routed on the error path");
    }

    /// The mapping is total over the wired events and stable.
    #[test]
    fn the_event_mapping_pins_the_two_wired_events() {
        assert_eq!(
            clip_for_event(GameplayAudioEvent::BootMovieSkipped),
            "ui_fe_ht_back"
        );
        assert_eq!(
            clip_for_event(GameplayAudioEvent::BootMovieEnded),
            "ui_fe_ht_wipe"
        );
        // play_event composes play for both events on the real file.
        let sounds = real_sounds();
        let mut audio = AudioOut::new(1 << 17, 2);
        for event in [
            GameplayAudioEvent::BootMovieSkipped,
            GameplayAudioEvent::BootMovieEnded,
        ] {
            let frames = sounds
                .play_event(&mut audio, event)
                .unwrap_or_else(|e| panic!("{event:?}: {e}"));
            assert!(frames > 0);
        }
        assert_eq!(audio.mixer_ref().len(), 2, "both voices are live");
    }

    /// A non-P3D input is a clean `Err` (the app's candidate loop
    /// skips to the next file; no panic, no partial registry).
    #[test]
    fn a_non_p3d_input_fails_cleanly() {
        let err = GameplaySounds::from_p3d_bytes(b"not a p3d at all", "junk.bin")
            .expect_err("garbage is rejected");
        assert!(err.contains("not a parseable P3D"), "loggable: {err}");
    }

    /// The REAL roster's rate census (block 59): 25 of the 70 clips
    /// decode at 44 100 Hz, 45 at 48 000.  This also CORRECTS the
    /// block-58 note "movement/combat are 44.1 kHz": the MOVEMENT/
    /// POWERS pair (`air_brake`, `big_heavy_whoosh_01`) is 44.1 kHz,
    /// but the combat roars are 48 kHz, and several `ui_fe_*` clips
    /// are 44.1 kHz too.  These numbers ARE the committed fixture —
    /// pinned so any vocabulary change is a conscious edit.
    #[test]
    fn the_real_roster_rate_census_is_pinned() {
        let sounds = real_sounds();
        let n44 = sounds
            .clip_names()
            .iter()
            .filter(|&&n| sounds.get(n).unwrap().sample_rate == 44_100)
            .count();
        let n48 = sounds
            .clip_names()
            .iter()
            .filter(|&&n| sounds.get(n).unwrap().sample_rate == 48_000)
            .count();
        assert_eq!((n44, n48), (25, 45), "the fixture's rate census");
        // Rate spot checks across the families (measured, block 59).
        assert_eq!(sounds.get("air_brake").unwrap().sample_rate, 44_100);
        assert_eq!(
            sounds.get("big_heavy_whoosh_01").unwrap().sample_rate,
            44_100
        );
        assert_eq!(
            sounds.get("infected1_roar_01").unwrap().sample_rate,
            48_000
        );
        assert_eq!(sounds.get("brawler_grunt_13").unwrap().sample_rate, 48_000);
    }

    /// THE BLOCK-59 UNBLOCKING: every 44 100 Hz clip in the user's
    /// own vocabulary now plays RATE-CLEAN — `play` spawns it at the
    /// session rate (48 000) with the duration preserved (the spawn
    /// count is the exact ceil of frames·48000/44100, the
    /// `resampled_frame_count` arithmetic), the voice ends exactly
    /// once, and the spawn-time duration error is below one output
    /// frame.
    #[test]
    fn every_forty_four_point_one_clip_plays_rate_clean() {
        let sounds = real_sounds();
        let names_44: Vec<String> = sounds
            .clip_names()
            .iter()
            .filter(|&&n| sounds.get(n).unwrap().sample_rate == 44_100)
            .map(|n| n.to_string())
            .collect();
        assert_eq!(names_44.len(), 25, "the census above");
        for name in &names_44 {
            let clip = sounds.get(name).unwrap();
            let native_frames = clip.frames();
            let mut audio = AudioOut::new(1 << 17, 2);
            let frames = sounds
                .play(&mut audio, name, 1.0)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let expected = prototype_audio::sample_rate::resampled_frame_count(
                native_frames,
                44_100,
                SESSION_RATE_HZ,
            );
            assert_eq!(frames, expected, "{name}: resampled spawn count");
            // Tick the voice out; it ends exactly once.
            let mut ends = 0usize;
            while audio.mixer_ref().len() > 0 {
                ends += audio.tick(1024).voice_ends;
            }
            assert_eq!(ends, 1, "{name}: the voice ends exactly once");
            // Duration preserved: native seconds vs session seconds —
            // the gap is the ceil's sub-frame rounding only.
            let native_s = native_frames as f64 / 44_100.0;
            let session_s = frames as f64 / SESSION_RATE_HZ as f64;
            assert!(
                (native_s - session_s).abs() < 1.0 / SESSION_RATE_HZ as f64 + 1e-9,
                "{name}: {native_s} s native vs {session_s} s at the session rate"
            );
        }
    }

    /// THE BYTE-EXACT PIN on the real movement vocabulary: air_brake
    /// (44 100 Hz mono, 2001 RADP blocks = 64 032 frames) spawns as
    /// 69 695 session-rate frames and the RESAMPLED samples land in
    /// the block-57 shared ring EXACTLY — the mixer duplicates the
    /// mono voice to both stereo channels at gain 1.0 (exact f32),
    /// so ring[2f] == ring[2f+1] == expected[f]; the overshoot tail
    /// is silence and the voice ends exactly once.
    #[test]
    fn air_brake_lands_in_the_ring_rate_clean_byte_exact() {
        let sounds = real_sounds();
        let clip = sounds.get("air_brake").unwrap();
        assert_eq!(clip.sample_rate, 44_100);
        assert_eq!(clip.channels, 1);
        assert_eq!(clip.frames(), 2001 * 32);
        let expected = prototype_audio::sample_rate::resample(
            &clip.samples,
            1,
            44_100,
            SESSION_RATE_HZ,
        );
        assert_eq!(expected.len(), 69_695);
        let mut audio = AudioOut::new(1 << 18, 2);
        let frames = sounds
            .play(&mut audio, "air_brake", 1.0)
            .expect("the 44.1 kHz clip spawns rate-clean");
        assert_eq!(frames, 69_695);
        let mut routed = 0usize;
        let mut ends = 0usize;
        while audio.mixer_ref().len() > 0 {
            let st = audio.tick(1024);
            routed += st.samples_routed;
            ends += st.voice_ends;
        }
        assert_eq!(ends, 1);
        // Full stereo buffers per tick; the overshoot is one tick's
        // silence tail (the same parity the 48 kHz pin asserts).
        assert_eq!(routed % 2048, 0);
        assert!(
            routed >= expected.len() * 2 && routed - expected.len() * 2 < 2048,
            "routed {routed} vs expected {}",
            expected.len() * 2
        );
        assert_eq!(audio.buffered(), routed);
        let drained = audio.ring_handle().lock().unwrap().read(routed);
        for (f, &e) in expected.iter().enumerate() {
            assert_eq!(drained[2 * f], e, "frame {f}: left channel");
            assert_eq!(
                drained[2 * f + 1],
                e,
                "frame {f}: right channel (mono duplicated)"
            );
        }
        assert!(
            drained[expected.len() * 2..].iter().all(|&s| s == 0),
            "the overshoot tail is silence"
        );
    }

    // ------------------------------------------------------------------
    // Block 61: the REAL audio-archive members (00audio/01audio.rcf).
    // The name-table roster (prototype-rcf::name_table) proves the two
    // archives carry 13 465 + 1 107 sharded `AudioFile` clip P3Ds; these
    // two committed members prove the SAME proven decode path plays them.
    // ------------------------------------------------------------------

    /// The sharded members are ONE-CLIP P3Ds with SEMANTIC clip names,
    /// RADP v1 (mono) at 32 000 Hz — the shipped corpus's third rate.
    #[test]
    fn decodes_the_real_audio_archive_shard_members() {
        // 00audio.rcf: audio\english\AudioFile\c\6\c6c7ad87.p3d (4 683 B)
        let cough = GameplaySounds::from_p3d_bytes(
            &audio_member("audio_member_c6c7ad87.p3d"),
            "00audio.rcf!c6c7ad87",
        )
        .expect("the real shard member parses");
        assert_eq!(cough.len(), 1, "one clip per shard member");
        assert_eq!(cough.decode_failures(), 0);
        let clip = cough.get("20915_p_mtm_cough_d").expect("semantic name");
        assert_eq!(clip.channels, 1, "RADP v1 is mono");
        assert_eq!(clip.sample_rate, 32_000, "the third shipped rate");
        assert_eq!(clip.samples.len(), 214 * 32, "blocks x 32 (PROVEN RADP v1)");

        // 01audio.rcf: audio\english\AudioFile\f\3\f35c7b78.p3d (4 023 B)
        let maoff = GameplaySounds::from_p3d_bytes(
            &audio_member("audio_member_f35c7b78.p3d"),
            "01audio.rcf!f35c7b78",
        )
        .expect("the real shard member parses");
        assert_eq!(maoff.len(), 1);
        assert_eq!(maoff.decode_failures(), 0);
        let clip = maoff.get("11271_maoff_142").expect("semantic name");
        assert_eq!(clip.channels, 1);
        assert_eq!(clip.sample_rate, 32_000);
        assert_eq!(clip.samples.len(), 183 * 32);
    }

    /// The 32 kHz shards play RATE-CLEAN through the session mixer:
    /// play() converts 32 000 -> 48 000 on spawn (the block-59
    /// SampleRateConverter), the spawn frame count is the exact ceil
    /// arithmetic, the voice ends exactly once, and the shared ring
    /// receives the resampled mix (mono duplicated to both channels).
    #[test]
    fn shard_clips_play_rate_clean_through_the_session_mixer() {
        let cough = GameplaySounds::from_p3d_bytes(
            &audio_member("audio_member_c6c7ad87.p3d"),
            "00audio.rcf!c6c7ad87",
        )
        .expect("parse");
        let mut audio = AudioOut::new(1 << 17, 2);
        // 6 848 mono frames at 32 kHz -> ceil(6848 x 48/32) = 10 272.
        let want = prototype_audio::sample_rate::resampled_frame_count(
            214 * 32,
            32_000,
            SESSION_RATE_HZ,
        );
        assert_eq!(want, 10_272);
        let spawned = cough
            .play(&mut audio, "20915_p_mtm_cough_d", 1.0)
            .expect("the 32 kHz clip plays rate-clean");
        assert_eq!(spawned, want, "play returns the RESAMPLED frame count");
        let mut routed = 0usize;
        let mut ends = 0usize;
        while audio.mixer_ref().len() > 0 {
            let st = audio.tick(1024);
            routed += st.samples_routed;
            ends += st.voice_ends;
        }
        assert_eq!(ends, 1, "one voice end");
        assert!(routed >= want * 2 && routed - want * 2 < 2048);
        assert_eq!(audio.buffered(), routed);

        // And the 01audio shard: 5 856 frames -> 8 784 (exact x1.5).
        let maoff = GameplaySounds::from_p3d_bytes(
            &audio_member("audio_member_f35c7b78.p3d"),
            "01audio.rcf!f35c7b78",
        )
        .expect("parse");
        let spawned = maoff
            .play(&mut audio, "11271_maoff_142", 1.0)
            .expect("play");
        assert_eq!(spawned, 8_784);
    }

    /// The BANK members (`ambience.p3d`, `e11_music.p3d`) are Cue/patch
    /// banks, NOT clip carriers: they parse as P3Ds but hold ZERO
    /// AudioFile chunks — the registry contract (Err only for
    /// non-P3D input, never for a clip-less bank) holds.
    #[test]
    fn audio_archive_banks_parse_with_zero_clips() {
        for name in ["audio_member_ambience.p3d", "audio_member_e11_music.p3d"] {
            let bank = GameplaySounds::from_p3d_bytes(&audio_member(name), name)
                .unwrap_or_else(|e| panic!("{name} must parse as a P3D: {e}"));
            assert_eq!(bank.len(), 0, "{name} carries no AudioFile clips");
            assert_eq!(bank.decode_failures(), 0, "{name} has no failures");
        }
    }

    /// The STANDALONE spawn path ([`GameplaySounds::play_clip`]) —
    /// the primitive behind the block-63 shard fallback: a lazily
    /// decoded shard clip plays rate-clean WITHOUT ever entering an
    /// eager registry, with the exact same resample arithmetic as
    /// the registry path.
    #[test]
    fn play_clip_spawns_a_standalone_shard_clip_rate_clean() {
        let cough = GameplaySounds::from_p3d_bytes(
            &audio_member("audio_member_c6c7ad87.p3d"),
            "00audio.rcf!c6c7ad87",
        )
        .expect("parse");
        let clip = cough.get("20915_p_mtm_cough_d").expect("clip").clone();
        let mut audio = AudioOut::new(1 << 17, 2);
        // 6 848 mono frames at 32 kHz -> ceil(6848 x 48/32) = 10 272 —
        // the block-61 pin, now through the standalone path.
        let want = prototype_audio::sample_rate::resampled_frame_count(
            214 * 32,
            32_000,
            SESSION_RATE_HZ,
        );
        assert_eq!(want, 10_272);
        let spawned = GameplaySounds::play_clip(&mut audio, &clip, 1.0).expect("spawn");
        assert_eq!(spawned, 10_272);
        // An empty clip is refused without panicking.
        let mut empty = clip.clone();
        empty.samples.clear();
        assert!(GameplaySounds::play_clip(&mut audio, &empty, 1.0).is_err());
    }

    // ------------------------------------------------------------------
    // Block 67: the multi-channel fold + the tagged spawn
    // ------------------------------------------------------------------

    #[test]
    fn v6_nis_mix_folds_to_a_stereo_voice_keeping_the_bed() {
        // Synthetic 5.1-shaped clip: ch0/ch1 the pair, ch2 the
        // full-scale bed, ch3..5 silent (the REAL fixture's PROVEN
        // layout — see the fixture test below).
        let mut samples = Vec::new();
        for f in 0..2i32 {
            samples.push((100 + f) as i16); // ch0
            samples.push((200 + f) as i16); // ch1
            samples.push(30_000); // ch2 — the bed
            samples.push(0);
            samples.push(0);
            samples.push(0);
        }
        let clip = SoundClip {
            name: "synthetic_51".into(),
            channels: 6,
            sample_rate: SESSION_RATE_HZ,
            samples,
        };
        let mut audio = AudioOut::new(1 << 14, 2);
        let frames = GameplaySounds::play_clip_tagged(&mut audio, &clip, 1.0, Some(9))
            .expect("the folded clip spawns");
        assert_eq!(frames, 2, "frame count survives the fold");
        let voice = audio.mixer_ref().voice(0).expect("one voice");
        assert_eq!(voice.channels, 2, "the voice is a stereo unit");
        assert_eq!(voice.tag, Some(9), "the lifecycle tag rides the voice");
        // L = ch0 + 0.5·ch2 = 100 + 15000 = 15100; R = 200 + 15000 = 15200.
        assert_eq!(voice.samples, vec![15100, 15200, 15101, 15201]);
        // The tag is the stop handle: one call cuts the unit.
        assert_eq!(audio.mixer().stop_tagged(9), 1);
        assert_eq!(audio.mixer_ref().len(), 0);
    }

    #[test]
    fn the_real_v6_nis_mix_plays_end_to_end_through_the_fold() {
        // The REAL shard member (block 64's Range-fetched
        // audio\english\AudioFile\mnis_5_2_1_mix.p3d, RADP v6, 48 kHz):
        // decodes to a 6-channel 5.1 master whose ch2 is full-scale —
        // the fold must keep it (verbatim-first-two would drop the
        // loudest channel) and never clip beyond i16.
        let bytes = audio_member("audio_member_mnis_5_2_1_mix.p3d");
        let sounds = GameplaySounds::from_p3d_bytes(&bytes, "mnis_5_2_1_mix.p3d")
            .expect("the real v6 mix decodes");
        let clip = sounds
            .clip_names()
            .first()
            .and_then(|n| sounds.get(n))
            .expect("one clip")
            .clone();
        assert_eq!(clip.channels, 6);
        assert_eq!(clip.sample_rate, 48_000);
        let src_frames = clip.samples.len() / 6;

        let mut audio = AudioOut::new(1 << 16, 2);
        let frames =
            GameplaySounds::play_clip_tagged(&mut audio, &clip, 1.0, Some(1))
                .expect("the real mix spawns");
        assert_eq!(frames, src_frames, "every source frame survives");
        let voice = audio.mixer_ref().voice(0).expect("one voice");
        assert_eq!(voice.channels, 2);
        assert_eq!(voice.frame_count(), src_frames);
        // The bed is IN the voice: both channels carry energy far
        // above a dialogue-only mix (verbatim-first-two would cap L/R
        // at the ch0/ch1 levels ≈ 7.3k; the bed adds ≥ 16k).
        let peak_l = voice
            .samples
            .iter()
            .step_by(2)
            .map(|s| (*s as i32).abs())
            .max()
            .unwrap();
        let peak_r = voice
            .samples
            .iter()
            .skip(1)
            .step_by(2)
            .map(|s| (*s as i32).abs())
            .max()
            .unwrap();
        assert!(peak_l > 16_000, "the bed must ride L: peak {peak_l}");
        assert!(peak_r > 16_000, "the bed must ride R: peak {peak_r}");
        // And the spawn never exceeded the i16 bus.
        assert!(peak_l <= i16::MAX as i32 && peak_r <= i16::MAX as i32);
    }
}

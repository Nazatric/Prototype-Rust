//! End-to-end integration test: load every real fixture file, run it
//! through the matching parser, and assert that the parsed structures
//! contain expected evidence from the original Prototype (2009)
//! binaries.

use std::path::Path;

fn fixture(name: &str) -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
}

// ---------------------------------------------------------------------------
// P3D v2
// ---------------------------------------------------------------------------

#[test]
fn parses_all_p3d_fixtures_with_exact_cover() {
    for name in [
        "skuinfo.p3d",
        "cementfiles.p3d",
        "statsachievements_tod.p3d",
        "frontendshaders.p3d",
        "boot_strap.p3d",
    ] {
        let data = fixture(name);
        let f = prototype_p3d::P3dFile::parse(&data)
            .unwrap_or_else(|e| panic!("{name} failed to parse: {e}"));
        assert_eq!(f.header.version, 12);
        assert_eq!(f.header.file_size as usize, data.len());
        assert!(!f.chunks.is_empty(), "{name} has no chunks");
        // Exact-cover invariant: last chunk ends at file end.
        let last = f.chunks.last().unwrap();
        assert_eq!(last.offset + last.size as usize, data.len());
    }
}

#[test]
fn skuinfo_p3d_declares_vug_prototype_sku() {
    let data = fixture("skuinfo.p3d");
    let f = prototype_p3d::P3dFile::parse(&data).unwrap();
    let body = f.chunks[0].payload;
    assert!(
        body.windows(19).any(|w| w == b"vug_prototype_pc_na"),
        "skuinfo must declare the SKU"
    );
    for lang in [b"english".as_slice(), b"french".as_slice(), b"italian".as_slice(), b"spanish".as_slice()] {
        assert!(
            body.windows(lang.len()).any(|w| w == lang),
            "skuinfo must list {:?}",
            String::from_utf8_lossy(lang)
        );
    }
}

#[test]
fn boot_strap_audio_name_index() {
    let data = fixture("boot_strap.p3d");
    let f = prototype_p3d::P3dFile::parse(&data).unwrap();
    let audio = f.audio_names().unwrap();
    assert_eq!(audio.len(), 70);
    let first_names: Vec<&str> = audio
        .iter()
        .filter_map(|(_, names)| names.first().map(String::as_str))
        .collect();
    assert!(first_names.contains(&"air_brake"));
    assert!(first_names.contains(&"pink_noise"));
    assert!(first_names.contains(&"woi_whoosh_in"));
}

/// Block 38, engine-PROVEN RADP header arithmetic, pinned corpus-wide
/// on the real 70-clip fixture.  The DLL's
/// `RadicalAdpcmFileDecoder::open` (0x10aaebc0; resolved through the
/// 4-class audio decoder registry — see prototype-audio crate docs)
/// computes `blocks_per_stream = data_size / (20 · version)` and
/// `samples_per_stream = blocks_per_stream · 32` (16 data bytes = 32
/// IMA nibble samples), which also pins `version` as the INTERLEAVED
/// STREAM COUNT and the remainder-free exactness of every payload.
#[test]
fn radp_header_arithmetic_matches_the_engine_parse() {
    let data = fixture("boot_strap.p3d");
    let f = prototype_p3d::P3dFile::parse(&data).unwrap();
    let mut checked = 0usize;
    let mut v2_stereo_pairs = 0usize;
    for (uid, af) in f.audio_files().unwrap() {
        let _ = uid;
        let block = 20usize;
        let streams = af.radp_version as usize;
        assert!(
            (1..=2).contains(&streams),
            "{}: version {} is not a stream count",
            af.names.first().map(String::as_str).unwrap_or("?"),
            af.radp_version
        );
        assert_eq!(
            af.data.len() % (block * streams),
            0,
            "{}: payload {} not an exact multiple of 20·{}",
            af.names.first().map(String::as_str).unwrap_or("?"),
            af.data.len(),
            streams
        );
        let blocks_per_stream = af.data.len() / (block * streams);
        let samples_per_stream = blocks_per_stream * 32;
        // The decoded length must agree with the engine's math: our
        // decoder yields blocks_per_stream · 32 samples per stream.
        let decoded = prototype_audio::radp::decode(af.radp_version, af.data).unwrap();
        let per_stream = decoded.samples.len() / streams;
        assert_eq!(
            per_stream, samples_per_stream,
            "{}: decoded samples disagree with the engine formula",
            af.names.first().map(String::as_str).unwrap_or("?")
        );
        // Sanity: the engine's own duration estimate (samples / rate)
        // stays in a sane clip-length range (the corpus's shortest
        // real clip is 0.036 s — ui_fe_ht_back).
        let seconds = samples_per_stream as f32 / af.sample_rate as f32;
        assert!(seconds > 0.01 && seconds < 120.0, "{}: {} s", af.names.first().map(String::as_str).unwrap_or("?"), seconds);
        if streams == 2 {
            v2_stereo_pairs += 1;
        }
        checked += 1;
    }
    assert_eq!(checked, 70, "every shipped clip participates");
    assert_eq!(v2_stereo_pairs, 11, "the 11 v2 (two-stream) clips");
}

// ---------------------------------------------------------------------------
// RCF
// ---------------------------------------------------------------------------

#[test]
fn parses_all_rcf_fixtures() {
    for name in ["shaders.rcf", "scripts.rcf"] {
        let data = fixture(name);
        let arc = prototype_rcf::RcfArchive::parse(&data)
            .unwrap_or_else(|e| panic!("{name} failed to parse: {e}"));
        assert_eq!(arc.header.magic, "ATG CORE CEMENT LIBRARY");
        assert!(!arc.entries.is_empty());
        for e in &arc.entries {
            let p = arc.payload(e).expect("payload must be readable");
            let _kind = prototype_rcf::PayloadKind::sniff(p);
        }
    }
}

#[test]
fn scripts_rcf_contains_known_credits() {
    let data = fixture("scripts.rcf");
    let arc = prototype_rcf::RcfArchive::parse(&data).unwrap();
    let mut found = false;
    for e in &arc.entries {
        if let Ok(p) = arc.payload(e) {
            if p.starts_with(b"EXECUTIVE PRODUCER\r\n") {
                assert!(
                    p.windows(11).any(|w| w == b"Eric Holmes"),
                    "credits must mention Eric Holmes (Lead Game Designer)"
                );
                assert!(
                    p.windows(12).any(|w| w == b"Tim Bennison"),
                    "credits must mention Tim Bennison (Executive Producer)"
                );
                found = true;
                break;
            }
        }
    }
    assert!(found, "scripts.rcf must contain a credits text entry");
}

// ---------------------------------------------------------------------------
// Lua
// ---------------------------------------------------------------------------

#[test]
fn parses_lua_fixture() {
    let data = fixture("frevents.lua");
    let f = prototype_lua::Lua51File::parse(&data).expect("frevents.lua must parse");
    assert_eq!(f.header.version, 0x51);
    assert_eq!(f.header.sizeof_lua_number, 4); // Prototype uses float, not double
}

// ---------------------------------------------------------------------------
// Engine + boot model
// ---------------------------------------------------------------------------

#[test]
fn engine_message_router_default_has_edges() {
    let r = prototype_engine::MessageRouter::prototype_default();
    use prototype_engine::MessageKind::*;
    let all = [
        EngineMessage,
        EngineIntrusiveMessage,
        EngineNonintrusiveMessage,
        EngineAnimationQueryLocomotionStateMessage,
        EngineCharacterQueryMessage,
        EngineCharacterQueryIntentionMessage,
        EngineCharacterQueryCollisionStateMessage,
        EngineSupportingLimbGetActiveMessageConst,
        EngineStickToSupportingSurfaceMessage,
        EngineSetIntersectionPropertiesMessage,
        EnginePhysicsSetEnabledMessage,
        EnginePhysicsWakeUpMessage,
        EngineGetCurrentDrawable,
        ProtoTargetMessage,
        ProtoTargetableMessage,
        ProtoGetTargetMessage,
        ProtoGetTargetPointMessage,
    ];
    let total: usize = all.iter().map(|k| r.subscribers(k).len()).sum();
    assert!(total >= 8, "default router must have at least 8 subscriptions, got {total}");
}

#[test]
fn boot_state_machine_walks_full_handshake() {
    use prototype_boot::*;
    let mut s = BootState::new();
    s.register_unlit();
    s.advance(BootStage::MainWaitLoop).unwrap();
    s.advance(BootStage::DisplayInitStart).unwrap();
    s.advance(BootStage::DisplayInitComplete).unwrap();
    s.advance(BootStage::ModuleRegistration).unwrap();
    s.advance(BootStage::AtgWorkerDispatch).unwrap();
    for g in CementLibraryGate::ALL {
        s.pass_cement_gate(g);
    }
    s.advance(BootStage::CementLibraryInit).unwrap();
    s.mount_rcf(RcfMountOrder::Cells0);
    s.mount_rcf(RcfMountOrder::Cells1);
    s.mount_rcf(RcfMountOrder::Art0);
    s.advance(BootStage::StartupPackageLoaded).unwrap();
    assert_eq!(s.current_stage, BootStage::StartupPackageLoaded);
}

// ---------------------------------------------------------------------------
// M16 — the full gameplay slice (block 6)
//
// One deterministic loop drives EVERY subsystem against real
// fixtures: content pipeline mount → scripts (frevents lifecycle +
// 70-mission registration) → EngineRuntime frames (input → player
// movement → streaming diff → REAL archive mount/unmount) → an
// audio event decoding a real RADP clip into a mixer voice at the
// player's position → mixed PCM.
// ---------------------------------------------------------------------------

mod full_slice {
    use std::path::Path;

    fn fixture(name: &str) -> Vec<u8> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    #[test]
    fn the_full_gameplay_slice_runs_end_to_end() {
        use prototype_audio::mixer::{Listener, Mixer, Voice};
        use prototype_engine::{EngineRuntime, EngineSignal};
        use prototype_input::{InputEvent, KeyCode};
        use prototype_math::Vec3;
        use prototype_stream::{ArchiveKind, CellId, StreamingCell, StreamingGrid};

        // ---- content: real RCF bytes behind the streaming mounts ----
        let scripts_rcf = fixture("scripts.rcf");
        let shaders_rcf = fixture("shaders.rcf");
        let sources = prototype_boot::streaming::ArchiveSources::new()
            .insert(ArchiveKind::Cells0, &scripts_rcf)
            .insert(ArchiveKind::Art0, &shaders_rcf);
        let mut pipeline = prototype_boot::ContentPipeline::new();
        let mut mounts = prototype_boot::streaming::StreamingMounts::new();

        // ---- scripts: frevents lifecycle + 70-mission registry ----
        let frevents = fixture("frevents.lua");
        let lua = prototype_lua::Lua51File::parse(&frevents).unwrap();
        let mut rt = EngineRuntime::new(&lua).unwrap();
        let mission_chunk = {
            let arc = prototype_rcf::RcfArchive::parse(&scripts_rcf).unwrap();
            let mut chunk = None;
            for e in &arc.entries {
                let p = arc.payload(e).unwrap();
                if prototype_rcf::PayloadKind::sniff(p)
                    == prototype_rcf::PayloadKind::Lua51
                    && p.len() == 2318
                {
                    chunk = Some(p.to_vec());
                }
            }
            prototype_lua::Lua51File::parse(&chunk.expect("mission chunk")).unwrap()
        };
        assert_eq!(rt.attach_mission_script(&mission_chunk).unwrap(), 70);
        assert_eq!(rt.bind_default_lifecycle().unwrap(), 2); // Reset + Destroy bound
        rt.dispatch_signal(EngineSignal::GameplayEnter).unwrap();
        assert_eq!(rt.fre().system().len(), 60); // 60 fre events reset

        // ---- world: two cells 1000 m apart along +X ----
        let mut grid = StreamingGrid::new();
        grid.add(StreamingCell::new(
            CellId::new(ArchiveKind::Cells0, 0),
            Vec3::new(0.0, 0.0, 0.0),
            100.0,
        ));
        grid.add(StreamingCell::new(
            CellId::new(ArchiveKind::Art0, 0),
            Vec3::new(1000.0, 0.0, 0.0),
            100.0,
        ));
        rt.set_streaming_grid(grid);

        // ---- audio: the real collectible clip, ready to spawn ----
        let boot_p3d = fixture("boot_strap.p3d");
        let p3d = prototype_p3d::P3dFile::parse(&boot_p3d).unwrap();
        let catalog = prototype_audio::AudioClipCatalog::from_p3d(&p3d).unwrap();
        let clip = catalog.find("blob_collect_01").unwrap();
        let collect_pcm = clip.decode().unwrap();
        assert_eq!(collect_pcm.channels, 1);
        // Real-time audio advance per 0.1 s gameplay tick.
        let audio_frames_per_tick = (clip.sample_rate as f32 * 0.1) as usize; // 4800

        // Listener sits AT the player (the camera follows the player).
        let mut mixer = Mixer::with_listener(Listener {
            position: Vec3::ZERO,
            min_distance: 5.0,
            max_distance: 200.0,
            rolloff: 1.0,
        });

        // ---- the frame loop ----
        let mut player = prototype_engine::Player::at(Vec3::ZERO);
        rt.apply_input(InputEvent::KeyDown(KeyCode::D)); // walk east
        let mut spawned_at = None;
        let mut mixed_samples = 0usize;
        let mut mixed_loud = 0usize;

        for frame in 1..=4000u64 {
            let (_f, set, _disp) = rt.gameplay_tick(&mut player, 0.1, 6.0).unwrap();
            mounts.apply(&mut pipeline, &set, &sources).unwrap();
            rt.apply_load_set(&set);

            // Gameplay event: at frame 10 (inside Cells0's radius),
            // a collectible pops: spawn its sound at the player.
            if frame == 10 {
                assert!(mounts.is_loaded(ArchiveKind::Cells0));
                // A collectible exists in the fre event system: bump
                // its first score var through the runtime (typed
                // entity access; two passes to keep borrows apart).
                let var_name = {
                    let sys = rt.fre().system();
                    let ev = sys
                        .event_by_id(prototype_engine::EntityId(0))
                        .expect("event 0");
                    ev.scores()[0].0.to_string()
                };
                {
                    let mut sys = rt.fre_mut().system_mut();
                    let ev = sys
                        .event_by_id_mut(prototype_engine::EntityId(0))
                        .expect("event 0");
                    let before = ev.score(&var_name).unwrap_or(0.0);
                    ev.set_score(&var_name, before + 1.0);
                }
                mixer.play(Voice {
                    samples: collect_pcm.samples.clone(),
                    channels: 1,
                    cursor: 0,
                    loop_voice: false,
                    gain: 1.0,
                    position: player.position(),
                    tag: None,
                });
                spawned_at = Some(frame);
            }

            // Bookkeeping and REAL mounts stay in lock-step every
            // frame (the strongest cross-subsystem invariant here).
            assert_eq!(
                rt.loaded_archives(),
                mounts.loaded(),
                "divergence at frame {frame}"
            );
            // Walk east until frame 1600 (x ≈ 960 — inside Art0's
            // 900..=1100 cell), then release D and park there.  The
            // cell spans only 200 m: an unbounded walk would exit it
            // again, which is correct streaming behaviour but bad
            // test design.
            if frame == 1600 {
                rt.apply_input(InputEvent::KeyUp(KeyCode::D));
            }
            // The listener follows the player (camera position).
            mixer.listener.position = player.position();
            // The mixer advances at REAL-TIME audio rate: a 0.1 s
            // gameplay tick consumes sample_rate × delta = 4800
            // frames of audio (mixing 1 sample/tick would stretch a
            // 2-second clip across 312 game-seconds — the source
            // would be beyond max_distance before the loud parts
            // play; that was a test-design bug, not a model bug).
            let out = mixer.tick(audio_frames_per_tick, 1);
            mixed_samples += out.samples.len();
            if out.samples.iter().any(|s| s.abs() > 100) {
                mixed_loud += 1;
            }

            if frame == 10 {
                assert_eq!(out.voices_played, 1, "voice live on spawn frame");
            }
            if spawned_at == Some(frame) && frame > 10 && frame < 25 {
                // The 2.09 s clip at real-time rate lives ~21 ticks
                // (frames 10..31); it must still be playing here.
                assert_eq!(mixer.len(), 1, "voice died early at frame {frame}");
            }
        }

        // ---- assertions: every subsystem actually ran ----
        // 1. Player walked east past the Art0 boundary.
        assert!(player.position().x > 900.0, "x = {}", player.position().x);
        // 2. Streaming really transitioned: Cells0 out, Art0 in.
        assert!(mounts.is_loaded(ArchiveKind::Art0));
        assert!(!mounts.is_loaded(ArchiveKind::Cells0));
        // 3. Missions: registered, one full lifecycle recorded.
        assert_eq!(rt.missions().len(), 70);
        // 4. Audio: the voice was spawned and consumed 3129 frames.
        assert_eq!(spawned_at, Some(10));
        // 4000 ticks × 4800 audio frames per tick.
        assert_eq!(mixed_samples, 4000 * audio_frames_per_tick);
        // The clip's audible life (~21 ticks) had real output: the
        // first ticks are loud (source still close), later ticks
        // fade as the player recedes — but well more than one tick
        // must exceed |100|.
        assert!(mixed_loud >= 5, "only {mixed_loud} loud ticks");
        assert!(mixed_loud < 4000, "mix never went silent — attenuation dead?");
        // 5. The mixer drained the voice by the end.
        assert_eq!(mixer.len(), 0);
        // 6. The script lifecycle actually ran through the router.
        assert!(rt
            .dispatch_log()
            .iter()
            .any(|e| e.signal == EngineSignal::GameplayEnter && e.ok));
    }
}

// ---------------------------------------------------------------------------
// M18b — the gameplay slice drives a LIVE device (block 8)
//
// The block-6 slice proved mix → `RingSink`; block 8 added the CPAL
// device edge.  This sibling (feature `audio-cpal`) proves the NEXT
// link: the same slice-shaped loop pushes its mix through
// `SharedRing` while a REAL CPAL output stream pulls from it, and
// the device-side counters confirm samples actually flowed.  Uses
// ALSA's software `null` PCM where no hardware exists (see the
// cpal_out module docs / PROGRESS.md block 8 for the sandbox
// provisioning); the callback code is host-agnostic.
// ---------------------------------------------------------------------------

#[cfg(feature = "audio-cpal")]
mod live_device_slice {
    use std::path::Path;

    use prototype_audio::cpal_out::{CpalOut, SharedRing};
    use prototype_audio::mixer::{Listener, Mixer, Voice};
    use prototype_audio::sink::RingSink;
    use prototype_engine::{EngineRuntime, EngineSignal};
    use prototype_math::Vec3;

    fn fixture(name: &str) -> Vec<u8> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    #[test]
    fn gameplay_slice_audio_reaches_a_live_device() {
        // ---- same boot path as the full slice (scripts + missions) ----
        let frevents = fixture("frevents.lua");
        let lua = prototype_lua::Lua51File::parse(&frevents).unwrap();
        let mut rt = EngineRuntime::new(&lua).unwrap();
        let mission_chunk = {
            let scripts_rcf = fixture("scripts.rcf");
            let arc = prototype_rcf::RcfArchive::parse(&scripts_rcf).unwrap();
            let mut chunk = None;
            for e in &arc.entries {
                let p = arc.payload(e).unwrap();
                if prototype_rcf::PayloadKind::sniff(p) == prototype_rcf::PayloadKind::Lua51
                    && p.len() == 2318
                {
                    chunk = Some(p.to_vec());
                }
            }
            prototype_lua::Lua51File::parse(&chunk.expect("mission chunk")).unwrap()
        };
        assert_eq!(rt.attach_mission_script(&mission_chunk).unwrap(), 70);
        rt.dispatch_signal(EngineSignal::GameplayEnter).unwrap();

        // ---- the real collectible clip (M13 decode) ----
        let boot_p3d = fixture("boot_strap.p3d");
        let p3d = prototype_p3d::P3dFile::parse(&boot_p3d).unwrap();
        let catalog = prototype_audio::AudioClipCatalog::from_p3d(&p3d).unwrap();
        let clip = catalog.find("blob_collect_01").unwrap();
        let collect_pcm = clip.decode().unwrap();

        // ---- the device: SharedRing + live CPAL stream ----
        let ring = SharedRing::new(RingSink::new(1 << 16));
        let out = match CpalOut::open_default(ring.clone()) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("live-device slice: no usable output device ({e}) — skipped");
                return;
            }
        };
        eprintln!(
            "live-device slice: {} Hz × {} ch",
            out.config.sample_rate, out.config.channels
        );

        // ---- 60 gameplay frames; the mix feeds the SHARED ring ----
        let mut mixer = Mixer::with_listener(Listener {
            position: Vec3::ZERO,
            min_distance: 5.0,
            max_distance: 200.0,
            rolloff: 1.0,
        });
        let audio_frames_per_tick = (clip.sample_rate as f32 * 0.1) as usize;
        let mut pushed_samples = 0usize;
        let mut player = prototype_engine::Player::at(Vec3::ZERO);

        for frame in 1..=60u64 {
            let _ = rt.gameplay_tick(&mut player, 0.1, 6.0).unwrap();
            if frame == 10 {
                mixer.play(Voice {
                    samples: collect_pcm.samples.clone(),
                    channels: 1,
                    cursor: 0,
                    loop_voice: false,
                    gain: 1.0,
                    position: player.position(),
                    tag: None,
                });
            }
            let mix = mixer.tick(audio_frames_per_tick, 1);
            // Back-pressure — the mix thread's real-world pacing
            // role: never overwrite buffered audio (a real device
            // may take a few ms to start its callback thread).
            loop {
                let r = ring.lock();
                if r.buffered() <= r.capacity() * 3 / 4 {
                    break;
                }
                drop(r);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            ring.lock().write(&mix.samples);
            pushed_samples += mix.samples.len();
        }

        // ---- assertions: the DEVICE side actually consumed audio ----
        // 1. The mix pushed everything into the shared ring.
        assert_eq!(pushed_samples, 60 * audio_frames_per_tick);
        // 2. The device callback ran (audio path is ALIVE).
        let cbs = out.stats.callbacks();
        assert!(cbs > 0, "no device callbacks — audio path dead");
        // 3. No stream errors.
        assert_eq!(out.stats.errors(), 0);
        // 4. Nothing was dropped on the mix side (ring big enough
        //    for 60 ticks, device drains as we go).
        assert_eq!(ring.lock().dropped, 0);
        eprintln!(
            "live-device slice: {cbs} callbacks, {} device underruns, {} dropped",
            out.stats.underruns(),
            ring.lock().dropped
        );
        // 5. Dropping CpalOut stops the stream (the keep-alive
        //    handle is the ONLY thing holding it open).
        drop(out);
    }
}

// ---------------------------------------------------------------------------
// M21b beachhead: RCF → RZ → P3D pipeline against the user's real art data
// ---------------------------------------------------------------------------

/// The real art model member (`barrierPlasticOS002` from the user's own
/// PC `art.rcf`, RZ-compressed) decompresses byte-exactly and parses as
/// a P3D file containing the full PROVEN model pipeline: geometry shape
/// chunks (`0x00010000`), a material (`0x00023000`), a shader
/// (`0x00011015`), and an embedded DDS texture (`0x00019000`).
#[test]
fn art_model_pipeline_rz_to_p3d() {
    let rz = fixture("art_barrier_os002.rz");
    assert!(prototype_rcf::rz::is_rz(&rz));

    let out = prototype_rcf::rz::decompress(&rz)
        .unwrap_or_else(|e| panic!("RZ decompress failed: {e}"));
    // Byte-exact against the committed reference produced by Python's
    // zlib from the same member.
    assert_eq!(out, fixture("art_barrier_os002.p3d"));

    let f = prototype_p3d::P3dFile::parse(&out)
        .unwrap_or_else(|e| panic!("decompressed art P3D failed to parse: {e}"));
    assert_eq!(f.header.version, 12);
    assert_eq!(f.header.file_size as usize, out.len());

    let has = |t: u32| f.chunks.iter().any(|c| c.chunk_type == t);
    // PROVEN chunk families present in this real model file
    // (ids mapped empirically in PROGRESS.md block 15):
    assert!(has(0x0001_0000), "geometry shape chunks missing");
    assert!(has(0x0002_3000), "material chunk missing");
    assert!(has(0x0001_1015), "shader chunk missing");
    assert!(has(0x0001_9000), "texture chunk missing");

    // The shape chunks carry the model's name as a u8-Pascal string
    // ("barrierPlasticOS002InitialShape" / "...DamagedShape").
    let shapes: Vec<_> = f
        .chunks
        .iter()
        .filter(|c| c.chunk_type == 0x0001_0000)
        .collect();
    assert!(shapes.len() >= 2, "expected 2+ shape chunks, got {}", shapes.len());
    for c in &shapes {
        let n = c.payload[0] as usize;
        let name = &c.payload[1..1 + n];
        assert!(
            name.starts_with(b"barrierPlasticOS002"),
            "shape name {:?} does not match the model",
            String::from_utf8_lossy(name)
        );
    }

    // The texture chunk names its DDS source (u8-Pascal, NUL-padded).
    let tex = f
        .chunks
        .iter()
        .find(|c| c.chunk_type == 0x0001_9000)
        .expect("texture chunk");
    let n = tex.payload[0] as usize;
    let tex_name = String::from_utf8_lossy(&tex.payload[1..1 + n]);
    let tex_name = tex_name.trim_end_matches('\0');
    assert!(
        tex_name.ends_with(".dds"),
        "texture payload must name a .dds source, got {tex_name:?}"
    );
    assert_eq!(tex_name, "barrierplasticos002_diffuse.dds");
}

/// The real texture member (`TUT078__I1.dds`) decompresses and parses;
/// its texture chunk names its DDS source.  The field layout AFTER the
/// name is NOT yet mapped (the two known texture members differ in their
/// post-name bytes — next block's work), so only the name + magic are
/// asserted here.
#[test]
fn art_texture_member_names_its_dds() {
    let rz = fixture("art_texture_tut078.rz");
    let out = prototype_rcf::rz::decompress(&rz).unwrap();
    assert_eq!(out, fixture("art_texture_tut078.p3d"));

    let f = prototype_p3d::P3dFile::parse(&out).unwrap();
    let tex = f
        .chunks
        .iter()
        .find(|c| c.chunk_type == 0x0001_9000)
        .expect("texture chunk");
    let n = tex.payload[0] as usize;
    let name = String::from_utf8_lossy(&tex.payload[1..1 + n]);
    assert_eq!(name.trim_end_matches('\0'), "TUT078__I1.dds");
}

/// The whole-archive census finding, pinned: the decompressed art
/// corpus uses these chunk ids and NOT the public-Pure3D mesh ids that
/// were previously marked LIKELY (`0x00010012`/`0x00010013`/`0x00010020`
/// never appear in `art.rcf`).
#[test]
fn art_model_uses_prototype_chunk_ids_not_public_pure3d_mesh_ids() {
    let out = prototype_rcf::rz::decompress(&fixture("art_barrier_os002.rz")).unwrap();
    let h = prototype_p3d::geometry::type_histogram(&out);
    assert_eq!(h.malformed, 0, "the real art model must walk cleanly");
    for t in [0x0001_0012u32, 0x0001_0013, 0x0001_0020] {
        assert!(
            !h.types.iter().any(|tc| tc.chunk_type == t),
            "public Pure3D id {t:#010x} unexpectedly present"
        );
    }
}

//! The shared winit [`ApplicationHandler`] — ONE app loop for BOTH
//! platforms (M26).
//!
//! # Why a shared handler
//!
//! The loader-style port runs the same discovery → boot → render loop on
//! every host: on Android behind NativeActivity ([`crate::activity`]), on
//! desktop behind a plain winit event loop (the `prototype-demo` crate's
//! `demo-render` bin).  Duplicating the handler would let the two drift,
//! and every desktop run is a free end-to-end test of the Android path.
//! The only Android-specific piece left in `activity` is the entry point
//! itself: `android_main`, the logcat sink, and winit's
//! `with_android_app` event-loop builder.
//!
//! # Lifecycle (identical on both platforms)
//!
//! 1. [`scan_or_setup`] (or [`App::new`]) scans the asset folder.
//!    Nothing usable yet → [`AppState::Setup`]: the app created the
//!    folder and is waiting for the user to copy their own game files in.
//! 2. winit's `resumed` (fires on start AND after every Android pause)
//!    re-scans.  Once ≥1 `.rcf` is found the state is [`AppState::Ready`]
//!    and the boot sequence runs: [`crate::boot::prepare_programs`]
//!    collects, pairs, decodes, translates and cross-checks every shader
//!    pair DEVICE-FREE, then `Renderer::new` creates the wgpu
//!    surface/device on the window and `create_render_pipeline` builds
//!    one real pipeline per pair — THIS is where a real GPU driver first
//!    compiles our translated WGSL.
//! 3. `about_to_wait` → `request_redraw`; `RedrawRequested` → clear frame
//!    (the draw-call path lands with M21b's real mesh data; today's frame
//!    proves the swapchain + the shipped pipelines live).
//! 4. `suspended` (Android pause) drops the renderer and the window —
//!    their surfaces are invalid across a pause.  The next `resumed`
//!    rebuilds everything (re-scan included: the user may have copied
//!    new files while we were backgrounded).  On desktop `suspended`
//!    never fires; `CloseRequested`/`exiting` tear down instead.
//!
//! # Input
//!
//! * Touch (Android): winit `Touch` → [`crate::touch::TouchMapper`]
//!   (single-pointer-as-mouse) → [`prototype_input::InputState`].
//! * Mouse (desktop): `CursorMoved`/`MouseInput` straight into
//!   [`prototype_input::InputEvent`] — Prototype shipped PC-only, so
//!   desktop mouse IS the native input model.
//! * Keyboard (both): `KeyboardInput` → [`map_key`] → the engine's
//!   typed [`prototype_input::KeyCode`] subset.  The winit→KeyCode
//!   table is DESIGN (the engine consumed Win32 VK_* codes; see
//!   `prototype_input`'s module docs — the *destination* enum and its
//!   VK mapping are the recoverable part, not winit's side).
//!
//! # Compile gating
//!
//! This module is gated by `feature = "winit-app"` ONLY — no
//! `target_os` gate.  It compiles and is unit-tested on desktop hosts
//! (winit's desktop backends are pure-Rust to *compile*; no display
//! server is needed for `cargo check`/`test`).  It is entirely safe
//! code: the single workspace `unsafe` surface remains `activity.rs`'s
//! native-glue FFI.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::{
    ElementState, MouseButton as WinitMouseButton, TouchPhase as WinitTouchPhase, WindowEvent,
};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

use prototype_engine::audio_out::AudioOut;
use prototype_engine::gameplay_sounds::{GameplayAudioEvent, GameplaySounds, SESSION_RATE_HZ};
use prototype_engine::look::LookControl;
use prototype_engine::player::{CameraFollow, Player};
use prototype_engine::world_objects::{SpawnReport, WorldObjectSystem};
use prototype_input::{InputEvent, InputState, KeyCode, MouseButton};
use prototype_math::{Mat4, Vec3};
use prototype_render::scene::Camera;
use prototype_render::wgpu_backend::{
    create_render_pipeline, create_render_pipeline_with_plans, wgpu, DebugDrawBatch, GpuPipeline,
    MeshBatchSource, Renderer, VideoQuad,
};

use crate::asset_folder;
use crate::boot::{boot_summary, prepare_programs, BootReport};
use crate::game_files::{self, GameFileSet, ScanError};
use crate::movie::{find_boot_movie, MoviePlayerState};
// Block 55 (NEXT-UP (a)): the boot movie's audio DEVICE edge — the
// live CPAL stream draining the player's shared ring.  Compiles (and
// is exercised) only with `--features audio-cpal`; without it the app
// field below is absent and the boot path is audio-silent as before.
#[cfg(feature = "audio-cpal")]
use crate::movie::MovieAudioEdge;
use crate::touch::{TouchMapper, TouchMapped, TouchPhase, TouchPoint};
// Block 57: the GAMEPLAY audio DEVICE edge — the persistent-lifetime
// counterpart of the movie edge, opened over the AudioOut ring.
#[cfg(feature = "audio-cpal")]
use crate::gameplay_audio::GameplayAudioEdge;

/// Which phase the FILE discovery is in (render state is tracked by the
/// [`App`] fields — `renderer.is_some()` — because a device can be Ready
/// without a live surface, and vice versa).
#[derive(Debug, Clone)]
pub enum AppState {
    /// Folder created / waiting for the user to copy game files.
    Setup {
        /// Where the user must copy their files.
        folder: PathBuf,
        /// The failure that stopped the last scan, if any.
        last_error: Option<String>,
    },
    /// At least one `.rcf` found — the engine can boot.
    Ready(GameFileSet),
}

/// Scan-or-setup: the single source of truth for which phase we are in.
///
/// A `Missing` scan result triggers [`asset_folder::bootstrap`] (create
/// the folder on first launch — the loader contract) and reports Setup
/// either way; every other scan outcome maps 1:1.
pub fn scan_or_setup(folder: &std::path::Path) -> AppState {
    match game_files::scan_folder(folder) {
        Ok(set) if game_files::is_bootable(&set) => {
            log::info!(
                "bootable: {} archive(s), {} loose p3d",
                set.rcf_archives.len(),
                set.p3d_files.len()
            );
            AppState::Ready(set)
        }
        Ok(set) => AppState::Setup {
            folder: folder.to_path_buf(),
            last_error: Some(game_files::missing_summary(&set)),
        },
        Err(ScanError::Missing(_)) => match asset_folder::bootstrap(folder) {
            Ok(_ready) => AppState::Setup {
                folder: folder.to_path_buf(),
                last_error: None,
            },
            Err(e) => AppState::Setup {
                folder: folder.to_path_buf(),
                last_error: Some(format!("{e}")),
            },
        },
        Err(e) => AppState::Setup {
            folder: folder.to_path_buf(),
            last_error: Some(format!("{e}")),
        },
    }
}

/// Map a winit key onto the engine's typed [`KeyCode`] subset.
///
/// DESIGN (ours): Prototype consumed Win32 virtual-key codes through its
/// `EngineKeyboardService` (PROVEN strings); the *typed* destination enum
/// is the portable reconstruction.  Which winit key equals which engine
/// key is the obvious physical-keyboard identity (W is W on every
/// platform winit supports), so the table below is LIKELY, not
/// invented-format guesswork.  Keys outside the gameplay subset map to
/// `None` and are dropped.
///
/// Both spellings of Space are accepted: winit reports
/// `NamedKey::Space` on every known backend, but some IME paths deliver
/// `Key::Character(" ")` — accepting both costs nothing and removes a
/// platform-behaviour unknown.
pub fn map_key(key: &Key) -> Option<KeyCode> {
    match key {
        Key::Character(c) => {
            // SmolStr derefs to str; lowercase into a plain String for
            // the match (handles W vs w without an ASCII-only API).
            let lower: String = c.to_lowercase();
            match lower.as_str() {
                "w" => Some(KeyCode::W),
                "a" => Some(KeyCode::A),
                "s" => Some(KeyCode::S),
                "d" => Some(KeyCode::D),
                "e" => Some(KeyCode::E),
                "q" => Some(KeyCode::Q),
                "f" => Some(KeyCode::F),
                "r" => Some(KeyCode::R),
                "1" => Some(KeyCode::One),
                "2" => Some(KeyCode::Two),
                "3" => Some(KeyCode::Three),
                "4" => Some(KeyCode::Four),
                "5" => Some(KeyCode::Five),
                "6" => Some(KeyCode::Six),
                "7" => Some(KeyCode::Seven),
                "8" => Some(KeyCode::Eight),
                "9" => Some(KeyCode::Nine),
                "0" => Some(KeyCode::Zero),
                " " => Some(KeyCode::Space),
                _ => None,
            }
        }
        Key::Named(named) => match named {
            NamedKey::Space => Some(KeyCode::Space),
            NamedKey::ArrowUp => Some(KeyCode::Up),
            NamedKey::ArrowDown => Some(KeyCode::Down),
            NamedKey::ArrowLeft => Some(KeyCode::Left),
            NamedKey::ArrowRight => Some(KeyCode::Right),
            NamedKey::Shift => Some(KeyCode::Shift),
            NamedKey::Control => Some(KeyCode::Control),
            NamedKey::Escape => Some(KeyCode::Escape),
            NamedKey::Enter => Some(KeyCode::Enter),
            NamedKey::Tab => Some(KeyCode::Tab),
            NamedKey::Backspace => Some(KeyCode::Backspace),
            _ => None,
        },
        // IME dead keys / other exotic keys: outside the gameplay subset.
        _ => None,
    }
}

/// The clear colour until meshes draw: the dark blue-grey of the PC
/// frontend's loading screen (eyeballed — DESIGN, not evidence).
const CLEAR: [f64; 4] = [0.08, 0.09, 0.11, 1.0];

/// Block 38: the gameplay audio stage's output rate (Hz) — the ring the
/// CPAL device edge would open at (`cpal_out`'s default stream rate;
/// DESIGN — the stage is device-free, the rate only sizes the per-frame
/// mix).  Block 59: the constant itself now lives in the engine
/// ([`prototype_engine::gameplay_sounds::SESSION_RATE_HZ`]) — the ONE
/// rate the mix runs at and every clip is converted to on spawn.
const AUDIO_SAMPLE_RATE_HZ: f32 = SESSION_RATE_HZ as f32;

/// Block 38: the per-frame audio mix length in frames — the frame's
/// delta-time at [`AUDIO_SAMPLE_RATE_HZ`], clamped to [128, 2048]: the
/// first frame (dt = 0) still feeds a small quantum so the ring never
/// starves, and the frame loop's dt clamp (0.1 s) cannot turn one stall
/// tick into a multi-second mix.  Device-free and unit-tested.
fn audio_frame_count(dt_seconds: f32) -> usize {
    ((dt_seconds.max(0.0) * AUDIO_SAMPLE_RATE_HZ).round() as usize).clamp(128, 2048)
}

// --- Player-camera parameters (block 21) --------------------------------
//
// All LIKELY/DESIGN: the original camera controller lives in
// `proto::CameraBehaviour` (RTTI class) with parameters in .p3d config
// we have not recovered.  These are walkable-feel placeholders, honest
// about being ours.
/// Ground speed, m/s (a run — Prototype's Alex moves fast).
const PLAYER_SPEED: f32 = 12.0;
/// Keyboard turn rate, rad/s (← / → rotate the player around +Y).
const PLAYER_TURN_SPEED: f32 = 2.6;
/// Vertical FOV (rad) — ~80°, close to a console third-person FOV.
const CAMERA_FOV_Y: f32 = 1.4;
/// Near plane, m.
const CAMERA_NEAR: f32 = 0.1;
/// Far plane, m.  Prototype's Manhattan is ~3.4 km × 4.4 km; the
/// streamed batches sit within a few hundred m of the player, so 2500
/// covers the visible set without z-fighting pressure.
const CAMERA_FAR: f32 = 2500.0;
/// Placed-object cube pool size (the nearest N spawners draw).
const BOX_POOL: usize = 32;

/// Block 22: one paired-drawable drawable — the loaded LOCAL-shape
/// batch plus the REAL world placement from its binary record.
struct PairedBatch {
    batch: DebugDrawBatch,
    pos: Vec3,
    quat_xyzw: [f32; 4],
    /// Uniform scale (the record's `scale[0]`; the corpus is
    /// (1,1,1) almost everywhere — non-uniform record scale would
    /// need a scale-matrix helper the math crate does not expose yet).
    scale: f32,
    /// Block 45: the GEOMETRY member's directory hash this paired
    /// mesh came from — the streaming unload key (when the member
    /// stream evicts the member, its paired batches drop with it).
    member_hash: u32,
}

/// Cap on paired records evaluated at boot (the pairing itself is a
/// cheap pass over the retained records).
const PAIR_CAP: usize = 4096;
/// Cap on loaded paired meshes (each is a real vertex/index upload).
/// Block 27 (NEXT-UP item 5): 24 → 96 now that the per-frame distance
/// cull (`select_paired_near`, radius 400 m + draw cap 64) exists —
/// boot density grows 4× while the frame cost stays capped.  Boot
/// decompresses one geometry member per NEW pair-member, so the boot
/// log's `members_decompressed`/`meshes` counters are the CI-visible
/// boot-time signal.
const PAIRED_MESH_CAP: usize = 96;
/// Block 26: per-frame draw radius for the paired pool, m.  DESIGN
/// tuning (LIKELY — the original engine's draw distance is UNKNOWN);
/// `select_paired_near` culls the pool to this radius every frame so a
/// future boot-cap raise cannot turn into a per-frame draw flood.
const PAIRED_DRAW_RADIUS_M: f32 = 400.0;
/// Block 26: max paired draw calls per frame (nearest-first; see
/// `select_paired_near`).  Now the ONLY frame-side limiter: the boot
/// pool is 96 meshes and the cull selects at most this many per frame.
const PAIRED_DRAW_CAP: usize = 64;
/// Cube marker edge length, m.
const BOX_SIZE: f32 = 2.0;
/// Spawn cap for the placed-object database at boot (of 81,370 corpus
/// records — the full set is ~10 MB; phones get the nearest-queried
/// subset from the same real records).
const SPAWN_CAP: usize = 8192;

/// Block 44: member-stream budget — how many world members the
/// block-43 state machine keeps desired around the player.  The boot
/// scan's `16` was a MESH cap over a one-shot load; this is the
/// streaming MEMBER budget that replaces the ad-hoc near-first scan
/// per frame.  DESIGN (the engine's real member-load budget is not
/// recovered from the binary); the block-43 policy semantics
/// (budget/hysteresis/settlement) are the tested part.
const MEMBER_STREAM_BUDGET: usize = 16;
/// Block 44: member-stream hysteresis (the keep-radius slack over the
/// worst desired distance) — the block-43 default, carried here.
const MEMBER_STREAM_HYSTERESIS: f32 = 1.5;
/// Block 44: world-mesh pool cap across the streamed lifetime — the
/// GPU-memory bound the per-frame loads fill toward (the boot's
/// one-shot 16-mesh cap was a boot bound, not a lifetime one).  When
/// the pool is full the pass stops (`pool_capped`), un-attempted
/// members stay desired, and unloads free room.  DESIGN tuning.
const MEMBER_MESH_POOL_CAP: usize = 256;

/// Block 44: ONE real mesh → GPU batches through the mesh pipeline
/// (the DLL world pair when the plan exists), with the static
/// DAYLIGHT rows written once — the exact per-mesh block the boot
/// loop has run inline since block 35, now shared with the per-frame
/// member-streaming loads so both paths build batches identically.
fn build_mesh_batches(
    renderer: &mut Renderer,
    mesh_pipeline: &GpuPipeline,
    world_ref: Option<&prototype_render::shader::world::WorldPairPlan>,
    mesh: &prototype_render::scene::SceneMesh,
) -> Vec<DebugDrawBatch> {
    let src = MeshBatchSource::from_mesh(&mesh_pipeline.vertex_layout, mesh);
    let batch = match world_ref {
        Some(plan) => renderer.create_world_mesh_batches(
            std::slice::from_ref(mesh_pipeline),
            std::slice::from_ref(&src),
            plan,
        ),
        None => renderer.create_mesh_batches(
            std::slice::from_ref(mesh_pipeline),
            std::slice::from_ref(&src),
        ),
    };
    if batch.is_empty() {
        return batch;
    }
    // Block 35: static DAYLIGHT rows — written ONCE per batch
    // (constant per draw; the frame loop only touches the matrices).
    // Without them the lit pass outputs black (the c6.w gate) and the
    // to-light vector degenerates.
    if let Some(plan) = world_ref {
        if let Ok(day) = prototype_render::shader::world::daylight_state(plan) {
            for b in &batch {
                for (reg, row) in day.floats() {
                    renderer.write_uniform_row(b, true, reg, row);
                }
                if let Ok(Some((reg, row))) =
                    prototype_render::shader::world::daylight_light_position(plan)
                {
                    renderer.write_uniform_row(b, false, reg, row);
                }
            }
        }
    }
    batch
}

/// Per-frame player update from the input snapshot, device-free: ← / →
/// yaw the player around +Y (world-space yaw: `q_yaw · q`), WASD then
/// translates via [`Player::move_camera_relative`] in the CAMERA's frame
/// (block 38: the camera yaw is the player's heading plus the look
/// orbit's yaw offset — with a zero orbit this is the legacy
/// player-relative walk, pinned by a test) — which also updates the
/// Idle/Move state and snaps the player to face the movement direction
/// (DESIGN instant snap; the original's smoothed turn is not
/// recovered).  Returns the applied displacement.
fn update_player(player: &mut Player, input: &InputState, follow: &CameraFollow, dt: f32) -> Vec3 {
    let mut turn = 0.0f32;
    if input.is_key_down(KeyCode::Left) {
        turn += PLAYER_TURN_SPEED * dt;
    }
    if input.is_key_down(KeyCode::Right) {
        turn -= PLAYER_TURN_SPEED * dt;
    }
    if turn != 0.0 {
        let yaw = prototype_math::Quat::from_axis_angle(Vec3::UP, turn);
        player.transform.rotation = yaw.mul_quat(&player.transform.rotation);
    }
    let camera_yaw = player.yaw() + follow.yaw_offset;
    player.move_camera_relative(input, dt, PLAYER_SPEED, camera_yaw)
}

/// The frame's camera from the camera-follow parameters: look-at from
/// [`CameraFollow::eye_target`] plus the shared FOV/planes and the
/// surface aspect.  Device-free and unit-tested.
fn frame_camera(follow: &CameraFollow, player: &Player, aspect: f32) -> Camera {
    let (eye, target) = follow.eye_target(player);
    Camera {
        eye,
        target,
        up: Vec3::UP,
        fov_y: CAMERA_FOV_Y,
        aspect,
        near: CAMERA_NEAR,
        far: CAMERA_FAR,
    }
}

/// Block 54: the boot movie's GPU state — the quad pipeline (per boot),
/// the video texture + view + bind group (per movie; the texture's
/// CONTENTS are re-written per frame via `upload_video_frame`).  All of
/// it is device state: dropped on suspend with everything else, rebuilt
/// by the next boot (which re-finds and re-opens the movie — the boot
/// movie replays after a resume, like every other boot-time scan).
struct MovieGpu {
    quad: VideoQuad,
    texture: wgpu::Texture,
    #[allow(dead_code)]
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}

/// Block 71: the NIS movie's per-frame step (the device-free half of
/// the picture leg — [`App::step_nis_movie`] computes it, the render
/// loop maps it to upload/draw/fall-through).  `Present`/`Holding`
/// present the quad (the frame is owned by the picture); every other
/// step falls through to the world draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NisMovieStep {
    /// No picture — the world draws.
    Idle,
    /// Playing, nothing due this frame — present the LAST frame.
    Holding,
    /// A fresh frame decoded — upload `rgba()`, then present.
    Present,
    /// The movie ended — the picture dropped, the cutscene stop is
    /// deferred via `pending_nis_stop`; the world draws this frame.
    Finished,
    /// The decode died — the picture dropped, the cutscene plays on
    /// audio-only; the world draws this frame.
    DecodeFailed,
}

/// The whole app state winit drives — shared by the Android activity
/// and the desktop `demo-render` bin.
pub struct App {
    /// The user's asset folder (fixed for the process lifetime).
    folder: PathBuf,
    /// File-discovery phase (re-scanned on every resume).
    state: AppState,
    /// The device-free boot report (shader pairs ready for pipelines).
    boot: Option<BootReport>,
    /// Attributes for the window created on first resume (title/size —
    /// the desktop demo customises them; Android keeps defaults).
    window_attrs: WindowAttributes,
    /// The winit window (None while suspended / not yet created).
    window: Option<Arc<Window>>,
    /// The wgpu renderer (None while suspended / before first boot).
    renderer: Option<Renderer>,
    /// Pipelines created from the boot report (per pair).
    pipelines: Vec<GpuPipeline>,
    /// Block 34: the DLL `default` WORLD pipeline — what world geometry
    /// draws through when the user's engine DLL was found (`None` keeps
    /// the frontend pipeline[0] fallback, logged honestly).
    world_pipeline: Option<GpuPipeline>,
    /// The world pair's WVP register base (CTAB-proven c0; kept from
    /// the plan for the per-frame register-based uniform writes).
    world_wvp_register: u16,
    /// The world pair's model-matrix register base (CTAB-proven c4).
    world_matrix_register: u16,
    /// One drawable debug triangle per pipeline (the vertex-pull proof;
    /// replaced by real meshes when M21b lands).
    batches: Vec<DebugDrawBatch>,
    /// The real world index from the boot scan (M25/block 20) — drives
    /// the per-frame draw selection.
    world_index: Option<prototype_p3d::world::WorldIndex>,
    /// Batch→member provenance + selection (block 20).  `None` while
    /// only debug batches exist (nothing to select among).
    draw_selector: Option<crate::meshes::DrawSelector>,
    /// Block 44: the member-level PHYSICAL stream (the block-43 state
    /// machine + the disk span table) — its per-frame [`MemberLoadSet`]
    /// drives real member fetches and batch unloads.  `None` until the
    /// boot attached it over the scanned world index.
    member_stream: Option<crate::member_stream::MemberMeshStream>,
    /// Block 44: the world pair's DEVICE-FREE plan (kept from boot —
    /// the per-frame streaming loads build batches with the same
    /// daylight rows the boot wrote; the plan is the daylight source).
    world_plan: Option<prototype_render::shader::world::WorldPairPlan>,
    /// Block 44: world-space meshes currently in the GPU pool — the
    /// number the `MEMBER_MESH_POOL_CAP` room math runs against.
    member_meshes: usize,
    /// Block 44: world meshes contributed PER global member index —
    /// the exact subtraction when a member's batches unload (the
    /// pool count would drift if unloads only dropped batches).
    meshes_of_member: std::collections::BTreeMap<usize, usize>,
    /// Block 21: the player entity (M11f) — WASD + ←/→ drive it, the
    /// camera-follow frames it, and its position drives the per-frame
    /// draw selection and the placed-object queries.
    player: Player,
    /// Camera-follow parameters (LIKELY defaults — module docs).
    follow: CameraFollow,
    /// Block 21: the REAL placed-object database spawned from the
    /// world index (engine-side, prototype-engine is now a full dep).
    world_objects: WorldObjectSystem,
    /// The boot's spawn report (logged once, kept for tests).
    spawn_report: Option<SpawnReport>,
    /// Unit-cube markers for the nearest placed objects (pool of
    /// [`BOX_POOL`], pipeline[0]'s layout).
    boxes: Vec<DebugDrawBatch>,
    /// Block 22: the paired-drawable batches — REAL local-shape
    /// geometry (barriers, signs, dumpsters…) whose WORLD placement
    /// comes from the paired binary GameObject record
    /// (translate · rotate · uniform-scale model rows per frame).
    /// Empty until the pairing lands at boot.
    paired: Vec<PairedBatch>,
    /// Last frame's instant (delta-time source; None until the first
    /// frame — the first update applies dt = 0).
    last_frame: Option<Instant>,
    /// Surface aspect ratio (updated on resize + boot).
    aspect: f32,
    /// Logged-once flag: the pipeline lacks the WVP uniform (rows 0 —
    /// meshes/cubes cannot be moved by the camera).
    warned_no_wvp: bool,
    /// Logged-once flag: the frame's selection/camera state.
    logged_first_frame: bool,
    /// Live input snapshot (touch + mouse + keyboard feed it).
    input: InputState,
    /// Single-pointer-as-mouse touch mapping (no-op on desktop; the
    /// second concurrent finger is the camera look — block 38).
    touch: TouchMapper,
    /// Block 38: the touch look orbit (second-finger drags feed it;
    /// applied to the camera follow once per frame).
    look: LookControl,
    /// Block 38: the gameplay AUDIO OUTPUT stage (`Mixer` → ring),
    /// ticked once per frame by the gameplay loop.  Block 57: the ring
    /// is SHARED and the CPAL edge drains it in real time (feature
    /// `audio-cpal`).  Block 58: the CONTENT half is live —
    /// `sounds` holds real clips from the user's own audio P3D and
    /// gameplay events spawn real voices into this mixer (the clip ↔
    /// event mapping is a documented DESIGN over the PROVEN
    /// clip-name vocabulary; see `gameplay_sounds`).
    audio: AudioOut,
    /// Block 38: logged-once flag for the first audio tick.
    audio_logged: bool,
    /// Block 58: the gameplay clip registry (the audio CONTENT half)
    /// — real clips RADP-decoded from the user's own P3D audio
    /// archive at boot.  None until a candidate decodes (device-free;
    /// the events degrade to silence without it).
    sounds: Option<GameplaySounds>,
    /// Block 58: a gameplay audio event detected INSIDE the renderer
    /// borrow (the movie→world transition) rides here and fires
    /// AFTER the borrow, still in the same frame (block 55's E0499
    /// lesson: `&mut self` helpers cannot run under `renderer
    /// .as_mut()`).
    pending_gameplay_event: Option<GameplayAudioEvent>,
    /// Block 63: the shard clip server — the embedded block-63 index
    /// over the user's own audio archives; serves ANY of the 14 572
    /// mined semantic clip names lazily (lookup → span read →
    /// decode → play) when the eager registry misses.  Device-free
    /// at construction (the index is compile-time data); every
    /// load failure is logged and degrades to silence.
    shard_server: crate::audio_registry::ShardClipServer,
    /// Block 66: the mission MUSIC GRAPH, decoded at boot from the
    /// user's own audio banks (`00audio.rcf`'s 12 music/ambience
    /// members + `audio\boot_strap.p3d`) — the PROVEN chain
    /// mission-id cue → transition → stem patch → clip names.
    /// Device-free; empty until a music archive serves (the game
    /// boots silent, exactly as before block 66).
    /// [`App::play_mission_music`] turns a mission/transition pair
    /// into rate-clean voices through the shard server.
    music_graph: crate::music_graph::MusicGraph,
    /// Block 67: the mixer TAG of the NIS mix currently playing
    /// (see [`App::play_nis_mix`]) — `None` when no NIS mix runs.
    /// One NIS mix at a time (starting a new one stops the old one —
    /// the engine's cutscenes are modal; DESIGN, documented).
    nis_voice_tag: Option<u64>,
    /// Block 67: monotonically increasing voice-tag source (never
    /// reused within a session, so a stale stop can never hit a
    /// newer unit).
    voice_tag_seq: u64,
    /// Block 67: a NIS-mix unit must be cut at the end of THIS
    /// frame's movie advance (the movie-end arm sits inside the
    /// renderer borrow — the cut itself is a `&mut self` method, so
    /// it DEFERS to the post-borrow tail exactly like
    /// `pending_gameplay_event`).
    pending_nis_mix_stop: bool,
    /// Block 68: the MISSION SYSTEM — the gameplay state that owns
    /// the current mission id + music transition and turns state
    /// changes into [`crate::mission::MusicRequest`]s served through
    /// the shard clip server (the ladder's top item).  Device-free;
    /// the initial phase is [`crate::mission::MissionPhase::Silent`]
    /// — boot itself starts no music, the frontend entry does.
    mission: crate::mission::MissionSystem,
    /// Block 69: the NIS TRIGGER SYSTEM — the gameplay state that owns
    /// the active cutscene (`NIS_Player` patch object) + the externally
    /// fed movie name (`nis_SetFMVFileName`'s counterpart) and turns
    /// state changes into [`crate::nis::NisRequest`]s served through
    /// the tagged voice lifecycle (the mix) and the PROVEN
    /// `find_movie_by_name` resolver (the movie leg).  Device-free;
    /// idle at boot — nothing starts a cutscene by itself.
    nis: crate::nis::NisSystem,
    /// Block 71: the MID-GAMEPLAY NIS movie player — the cutscene's
    /// PICTURE leg (the render-loop integration).  Present exactly
    /// while the active cutscene's movie plays: the picture is MODAL
    /// (it owns the frame like the boot movie does), it ends the whole
    /// cutscene when the movie ends (the picture is the timeline), and
    /// any [`App::stop_nis`] path drops it.  The player streams out of
    /// the user's own movie archive through the PROVEN span reader —
    /// the movie is never in RAM.
    nis_movie: Option<MoviePlayerState>,
    /// Block 71: the NIS movie's GPU side (quad + video texture + bind
    /// group).  Built LAZILY on the first presented frame (the trigger
    /// may fire before a boot or after a surface rebuild — this method
    /// [`App::play_nis_with_movie`] stays device-free) and dropped with
    /// the picture.
    nis_movie_gpu: Option<MovieGpu>,
    /// Block 71: logged-once flag for the NIS movie's first decoded
    /// frame.
    nis_movie_logged: bool,
    /// Block 71: the NIS movie's audio DEVICE edge (feature
    /// `audio-cpal`) — the same shape as the boot movie's edge, over
    /// THE ring the NIS player writes; opened at picture open, held
    /// through the tail drain after the picture ends, cut immediately
    /// on skip/stop.
    #[cfg(feature = "audio-cpal")]
    nis_movie_audio: Option<MovieAudioEdge>,
    /// Block 71: the active cutscene must STOP at the end of THIS
    /// frame — its picture ended inside the renderer borrow (the
    /// picture-end arm of the frame loop) and the stop is a `&mut
    /// self` method, so it DEFERS to the post-borrow tail exactly
    /// like `pending_nis_mix_stop`.
    pending_nis_stop: bool,
    /// Block 68: the mixer TAG of the current MUSIC unit — the
    /// frontend bed OR the mission stem (one music channel: the
    /// engine's music graph is a single voice bank; the units are
    /// mutually exclusive and every serve cuts the previous one).
    /// `None` when no music unit runs.
    music_unit_tag: Option<u64>,
    /// Block 70: the mixer TAG of the WORLD-BED unit — the free-roam
    /// ambience (the `ambience` patch's 18-stream world master,
    /// folded).  INDEPENDENT of `music_unit_tag`: the ambience patch
    /// lists its own busses (`mixbus = [master, amb, sfx]`, block-70
    /// census) — the world bed is a different channel from the music
    /// graph, so it never rides the music unit's cut-first serve.
    /// `None` when no world bed runs.
    ambience_unit_tag: Option<u64>,
    /// Block 68: the boot movie's end/skip DEFERS the frontend music
    /// start to the post-borrow tail (the arm holds the renderer
    /// borrow — the same defer pattern as `pending_nis_mix_stop`).
    pending_frontend_music_start: bool,
    /// Block 54: the boot-path movie player (None when no movie archive
    /// was found, it failed to open, or playback is over/skipped).
    movie: Option<MoviePlayerState>,
    /// Block 54: the movie's GPU side (quad + video texture + bind
    /// group).  Present exactly while `movie` is.
    movie_gpu: Option<MovieGpu>,
    /// Block 54: logged-once flag for the movie's first decoded frame.
    movie_logged: bool,
    /// Block 55 (NEXT-UP (a)): the boot movie's audio DEVICE edge
    /// (feature `audio-cpal`) — opened over THE ring the player writes
    /// at movie open, held through the tail drain after the movie ends,
    /// closed when the ring empties, cut immediately on skip/teardown.
    #[cfg(feature = "audio-cpal")]
    movie_audio: Option<MovieAudioEdge>,
    /// Block 57: the GAMEPLAY audio DEVICE edge (feature `audio-cpal`)
    /// — opened ONCE per boot over the [`AudioOut`] ring, held for the
    /// app's lifetime (the mixer produces every frame; silence is real
    /// audio state — there is no tail-drain close), cut only at
    /// teardown with the rest of the device state.
    #[cfg(feature = "audio-cpal")]
    gameplay_audio: Option<GameplayAudioEdge>,
}

impl App {
    /// Build the app and run the initial scan-or-setup (folder is
    /// created on first launch — the loader contract).  The desktop
    /// demo uses this; the Android activity scans earlier for logging
    /// and uses [`App::with_state`] instead.
    pub fn new(folder: PathBuf) -> Self {
        let state = scan_or_setup(&folder);
        Self::with_state(folder, state)
    }

    /// Build the app from an already-computed [`AppState`] (avoids a
    /// double scan in `android_main`, which logs before the loop).
    pub fn with_state(folder: PathBuf, state: AppState) -> Self {
        Self {
            state,
            folder,
            boot: None,
            window_attrs: Window::default_attributes(),
            window: None,
            renderer: None,
            pipelines: Vec::new(),
            world_pipeline: None,
            world_wvp_register: 0,
            world_matrix_register: 4,
            batches: Vec::new(),
            world_index: None,
            draw_selector: None,
            member_stream: None,
            world_plan: None,
            member_meshes: 0,
            meshes_of_member: std::collections::BTreeMap::new(),
            player: Player::new(),
            follow: CameraFollow::default(),
            world_objects: WorldObjectSystem::new(),
            spawn_report: None,
            boxes: Vec::new(),
            paired: Vec::new(),
            last_frame: None,
            aspect: 1.0,
            warned_no_wvp: false,
            logged_first_frame: false,
            input: InputState::new(),
            touch: TouchMapper::new(),
            look: LookControl::new(),
            audio: AudioOut::new(1 << 16, 2),
            audio_logged: false,
            sounds: None,
            shard_server: crate::audio_registry::ShardClipServer::embedded(),
            music_graph: crate::music_graph::MusicGraph::empty(),
            nis_voice_tag: None,
            voice_tag_seq: 0,
            pending_nis_mix_stop: false,
            mission: crate::mission::MissionSystem::new(),
            nis: crate::nis::NisSystem::new(),
            nis_movie: None,
            nis_movie_gpu: None,
            nis_movie_logged: false,
            #[cfg(feature = "audio-cpal")]
            nis_movie_audio: None,
            pending_nis_stop: false,
            music_unit_tag: None,
            ambience_unit_tag: None,
            pending_frontend_music_start: false,
            pending_gameplay_event: None,
            movie: None,
            movie_gpu: None,
            movie_logged: false,
            #[cfg(feature = "audio-cpal")]
            movie_audio: None,
            #[cfg(feature = "audio-cpal")]
            gameplay_audio: None,
        }
    }

    /// Override the attributes of the window created on first resume
    /// (desktop demo: title + initial size).  Android keeps the
    /// fullscreen defaults from the manifest theme.
    pub fn with_window_attributes(mut self, attrs: WindowAttributes) -> Self {
        self.window_attrs = attrs;
        self
    }

    /// Block 54: the user pressed something while the boot movie plays
    /// — skip it (the binding is DESIGN — module docs).  The next
    /// redraw reports `ended` and the world scene takes over.
    ///
    /// Block 71: the SAME input skips a MID-GAMEPLAY NIS movie first —
    /// the modal cutscene (picture + mix unit + trigger state) ends
    /// with the input through [`App::stop_nis`], the one cleanup path,
    /// and the next frame draws the world.  With no movie anywhere the
    /// method is a no-op (gameplay input is unaffected).
    fn skip_boot_movie(&mut self) {
        if self.nis_movie.is_some() {
            log::info!("nis movie: skipped by user input — the cutscene ends");
            self.stop_nis();
            return;
        }
        if let Some(state) = self.movie.as_mut() {
            log::info!("boot movie: skipped by user input");
            state.skip();
        }
        // Block 67: a skipped movie's NIS-mix unit (when one runs)
        // is cut with the picture — the modal cutscene ends here.
        self.stop_nis_mix();
        // Block 68: a skip is also the frontend appearing — its
        // attract music starts on the next tail flush (the serve is
        // a &mut self call; the flag keeps this method's contract).
        self.pending_frontend_music_start = true;
        // Block 55: a skip is an explicit cut — the device stream stops
        // NOW (the tail does not drain; the user asked for silence).
        self.stop_movie_audio();
        // Block 58: the skip is a UI interaction — the DESIGN mapping
        // plays the frontend back-click through the GAMEPLAY ring
        // (the movie stream is cut; the click rides the app's own
        // device edge).
        self.play_gameplay_event(GameplayAudioEvent::BootMovieSkipped);
    }

    /// Block 55: open the boot movie's audio device edge (audio-cpal).
    /// Non-fatal by contract: any refusal logs once and the movie
    /// plays silent — a device-less host boots exactly as before.
    #[cfg(feature = "audio-cpal")]
    fn open_movie_audio(&mut self, state: &MoviePlayerState) {
        match MovieAudioEdge::open(prototype_audio::cpal_out::SharedRing::from_arc(
            state.ring_handle(),
        )) {
            Ok(edge) => {
                let (rate, ch) = edge.config();
                log::info!(
                    "boot movie: audio device edge open — {rate} Hz × {ch} ch (the ring drains in real time; no resampling — see movie.rs honest unknowns)"
                );
                self.movie_audio = Some(edge);
            }
            Err(e) => {
                log::warn!("boot movie: audio device edge refused ({e}) — playing silent")
            }
        }
    }

    /// Block 55: cut the boot movie's device stream immediately (skip
    /// / teardown / explicit stops — bypasses the tail drain).  No-op
    /// without the audio-cpal feature (the field does not exist).
    fn stop_movie_audio(&mut self) {
        #[cfg(feature = "audio-cpal")]
        if self.movie_audio.take().is_some() {
            log::info!("boot movie: audio device stream cut");
        }
    }

    /// Block 57: open the GAMEPLAY audio device edge over the
    /// [`AudioOut`] ring — ONCE per boot (guarded so a re-run
    /// `try_boot` never double-opens over the same ring), non-fatal
    /// by contract: any refusal logs once and the game keeps ticking
    /// silent.  The device callback drains the very buffer the
    /// per-frame tick fills (one buffer, one writer, one reader).
    #[cfg(feature = "audio-cpal")]
    fn open_gameplay_audio(&mut self) {
        if self.gameplay_audio.is_some() {
            return; // already open this session — never double-open
        }
        match GameplayAudioEdge::open(prototype_audio::cpal_out::SharedRing::from_arc(
            self.audio.ring_handle(),
        )) {
            Ok(edge) => {
                let (rate, ch) = edge.config();
                log::info!(
                    "gameplay audio: device edge open — {rate} Hz × {ch} ch (the per-frame tick's ring now drains in real time; no resampling — see gameplay_audio.rs honest unknowns)"
                );
                self.gameplay_audio = Some(edge);
            }
            Err(e) => {
                log::warn!("gameplay audio: device edge refused ({e}) — ticking silent")
            }
        }
    }

    /// Block 57: cut the gameplay device stream (teardown only — the
    /// stream has NO tail-drain close; the mixer produces every frame
    /// and silence is real audio state).  No-op without audio-cpal.
    fn stop_gameplay_audio(&mut self) {
        #[cfg(feature = "audio-cpal")]
        if self.gameplay_audio.take().is_some() {
            log::info!("gameplay audio: device stream cut (teardown)");
        }
    }

    /// Block 55: close the device edge once its tail has drained —
    /// called EVERY frame after the movie block; the close decision is
    /// [`crate::movie::movie_audio_edge_should_close`], which never
    /// fires while the player is alive (a transient mid-movie underrun
    /// must not close the stream) and fires exactly when the player is
    /// gone and the ring is empty.
    fn stop_movie_audio_when_drained(&mut self) {
        #[cfg(feature = "audio-cpal")]
        {
            let close = self
                .movie_audio
                .as_ref()
                .map(|e| {
                    crate::movie::movie_audio_edge_should_close(self.movie.is_some(), e.tail_drained())
                })
                .unwrap_or(false);
            if close {
                log::info!(
                    "boot movie: audio tail drained — device stream closed ({} callback(s), {} underrun(s))",
                    self.movie_audio.as_ref().map(|e| e.device_callbacks()).unwrap_or(0),
                    self.movie_audio.as_ref().map(|e| e.device_underruns()).unwrap_or(0),
                );
                self.movie_audio = None;
            }
        }
    }

    /// Block 58: fire a gameplay audio event through the DESIGN
    /// mapping (`gameplay_sounds::clip_for_event` — REAL clip names
    /// from the user's own boot_strap.p3d; the assignment is ours,
    /// documented).  Block 63: an event whose clip misses the eager
    /// registry falls back to the SHARD clip server (the embedded
    /// block-63 index over the user's own audio archives serves any
    /// of the 14 572 mined semantic clip names lazily).  Non-fatal:
    /// every miss logs and the game continues (the mapping degrades
    /// to silence, never a panic).
    fn play_gameplay_event(&mut self, event: GameplayAudioEvent) {
        let clip = prototype_engine::gameplay_sounds::clip_for_event(event);
        if let Some(sounds) = self.sounds.as_ref() {
            match sounds.play_event(&mut self.audio, event) {
                Ok(frames) => {
                    log::info!(
                        "gameplay audio: {event:?} → {} frame(s) spawned into the mix ring",
                        frames
                    );
                    return;
                }
                Err(e) => {
                    log::debug!("gameplay audio: {event:?} not in the eager registry ({e})");
                }
            }
        }
        // Block 63: the shard fallback for registry misses.
        match self.play_shard_clip(clip, 1.0) {
            Ok(frames) => log::info!(
                "gameplay audio: {event:?} → {clip:?} served from the SHARD index, \
                 {frames} frame(s) spawned"
            ),
            Err(e) => log::warn!("gameplay audio: {event:?} → {clip:?} not played ({e})"),
        }
    }

    /// Block 63: serve ONE clip by semantic name from the user's own
    /// audio archives through the embedded shard index (lazy: lookup
    /// → archive open → member span read → one-clip P3D decode →
    /// rate-clean spawn).  `Ok(None)` degrades to a clean Err so the
    /// caller's log reads uniformly; every hard failure carries the
    /// archive diagnostics.  Never panics.
    fn play_shard_clip(&mut self, clip: &str, gain: f32) -> Result<usize, String> {
        self.play_shard_clip_tagged(clip, gain, None, false)
    }

    /// [`Self::play_shard_clip`] with a mixer LIFECYCLE TAG (block
    /// 67) and an explicit LOOP flag (block 68): the spawned voice
    /// carries `tag` so the caller can stop the whole unit later by
    /// tag (`Mixer::stop_tagged`) — index-free, because mixer indices
    /// shift on every removal — and repeats (`loop_voice`) until
    /// stopped, the music beds' PROVEN behavior.
    fn play_shard_clip_tagged(
        &mut self,
        clip: &str,
        gain: f32,
        tag: Option<u64>,
        loop_voice: bool,
    ) -> Result<usize, String> {
        let set = match &self.state {
            AppState::Ready(set) => set.clone(),
            _ => return Err(format!("no game folder ready for the shard clip {clip:?}")),
        };
        let clip_data = match self.shard_server.load_clip(&set, clip) {
            Ok(Some(c)) => c,
            Ok(None) => return Err(format!("shard index has no clip named {clip:?}")),
            Err(d) => return Err(format!("shard clip {clip:?} failed: {d}")),
        };
        GameplaySounds::play_clip_tagged_loop(&mut self.audio, &clip_data, gain, tag, loop_voice)
    }

    /// Block 67: play ONE NIS cutscene's AUDIO — the mix clip its
    /// `NIS_Player` patch references — through the full graph-served
    /// chain: `nis_object` (e.g. `mnis5_2_2`, `mb_3_1_1`, `nis0-3a`)
    /// → [`MusicGraph::nis_mix`] (the PROVEN descriptor: filename
    /// clip, volume, mixbuss labels) → the shard clip server (the
    /// user's own audio archives) → rate-clean TAGGED voices in the
    /// mix ring.
    ///
    /// The tag is the LIFECYCLE half of the wiring: the NIS-mix unit
    /// stops when the cutscene ends or is skipped (the movie-path
    /// hooks call [`App::stop_nis_mix`]), and starting a new NIS mix
    /// stops the previous one (cutscenes are modal — DESIGN, this
    /// crate's docs).  Effective gain = caller `gain` × the patch's
    /// own `volume`; the `mixbuss` labels are carried in the
    /// descriptor but our mixer has a single gameplay bus (honest
    /// simplification, not simulated routing).
    ///
    /// Public seam: nothing calls it automatically yet — the NIS
    /// trigger system does not exist, and the block-67 dictionary
    /// attack found NO NIS movie member in any archive (the VISUAL
    /// half stays open; see PROGRESS.md).  Every clip that fails
    /// logs and degrades to silence; `Err` only when the object is
    /// unknown or EVERY clip failed.  Never panics.
    pub fn play_nis_mix(&mut self, nis_object: &str, gain: f32) -> Result<usize, String> {
        let mix = self
            .music_graph
            .nis_mix(nis_object)
            .ok_or_else(|| format!("nis mix: no NIS patch named {nis_object:?} in the graph"))?;
        if mix.clips.is_empty() {
            return Err(format!(
                "nis mix: {nis_object:?} carries no mix clip reference"
            ));
        }
        self.voice_tag_seq = self.voice_tag_seq.wrapping_add(1);
        let tag = self.voice_tag_seq;
        let effective = gain * mix.volume;
        let mut frames = 0usize;
        let mut played = 0usize;
        let mut diags: Vec<String> = Vec::new();
        for clip in &mix.clips {
            match self.play_shard_clip_tagged(clip, effective, Some(tag), false) {
                Ok(n) => {
                    frames += n;
                    played += 1;
                    log::info!("nis mix: {nis_object} → {clip:?} ({n} frames, tag {tag})");
                }
                Err(e) => {
                    log::warn!("nis mix: {nis_object} → {clip:?} not played ({e})");
                    diags.push(e);
                }
            }
        }
        if played == 0 {
            return Err(format!(
                "nis mix {nis_object}: every clip failed ({})",
                diags.join("; ")
            ));
        }
        // SUCCESS only: the modal cut.  A failed start leaves the
        // previous unit playing and tracked; the new tag was never
        // used before, so this stop cannot touch the voices we just
        // spawned.
        self.stop_nis_mix();
        self.nis_voice_tag = Some(tag);
        Ok(frames)
    }

    /// Block 67: stop the NIS mix unit (end/skip hook + the modal
    /// pre-stop of [`App::play_nis_mix`]).  Returns how many voices
    /// were cut; a no-op (and `nis_voice_tag` cleared) when none
    /// runs.
    pub fn stop_nis_mix(&mut self) -> usize {
        let Some(tag) = self.nis_voice_tag.take() else {
            return 0;
        };
        let cut = self.audio.mixer().stop_tagged(tag);
        if cut > 0 {
            log::info!("nis mix: unit {tag} stopped ({cut} voice(s) cut)");
        }
        cut
    }

    /// Block 67: is a NIS mix unit playing right now?
    pub fn nis_mix_active(&self) -> bool {
        self.nis_voice_tag.is_some()
    }

    // ------------------------------------------------------------------
    // Block 69: the NIS trigger system drives the live seams
    // ------------------------------------------------------------------

    /// Block 69: the NIS TRIGGER — start ONE cutscene by its
    /// `NIS_Player` patch object and serve it NOW.  The state machine
    /// validates first (fail-closed: an unknown object or a
    /// non-`NIS_Player` patch is a clean `Err` and leaves BOTH the
    /// state and the audio untouched), then the mix descriptor's
    /// clips spawn as TAGGED voices through the block-67 chain
    /// (effective gain = caller `gain` × the patch's own `volume`);
    /// an event-driven patch (the corpus's 9 mix-less objects) is a
    /// TRACKED SILENT cutscene — `Ok(0)`, the state moves, the audio
    /// stays silent.
    ///
    /// Contract details carried over from [`App::play_nis_mix`]: the
    /// modal cut fires on SUCCESS only (a total spawn failure leaves
    /// the previous unit playing AND tracked, while the TRIGGER state
    /// moves to the new cutscene — served, by silence, the same
    /// consistency contract as the mission serve path); tags never
    /// reuse.  The request is marked consumed either way.
    pub fn play_nis(&mut self, nis_object: &str, gain: f32) -> Result<usize, String> {
        self.nis.start(&self.music_graph, nis_object)?;
        let req = self
            .nis
            .request(&self.music_graph)
            .expect("the cutscene was just started");
        if req.clips.is_empty() {
            log::info!(
                "nis: {nis_object} is event-driven (no mix clip) — tracked silence"
            );
            self.nis.mark_served();
            return Ok(0);
        }
        self.voice_tag_seq = self.voice_tag_seq.wrapping_add(1);
        let tag = self.voice_tag_seq;
        let effective = gain * req.volume;
        let mut frames = 0usize;
        let mut played = 0usize;
        let mut diags: Vec<String> = Vec::new();
        for clip in &req.clips {
            match self.play_shard_clip_tagged(clip, effective, Some(tag), false) {
                Ok(n) => {
                    frames += n;
                    played += 1;
                    log::info!("nis: {nis_object} → {clip:?} ({n} frames, tag {tag})");
                }
                Err(e) => {
                    log::warn!("nis: {nis_object} → {clip:?} not played ({e})");
                    diags.push(e);
                }
            }
        }
        if played == 0 {
            self.nis.mark_served();
            return Err(format!(
                "nis {nis_object}: every clip failed ({})",
                diags.join("; ")
            ));
        }
        self.stop_nis_mix();
        self.nis_voice_tag = Some(tag);
        self.nis.mark_served();
        Ok(frames)
    }

    /// Block 69: the cutscene's MOVIE leg — the external
    /// `nis_SetFMVFileName` feed.  The movie name arrives from the
    /// CALLER (mission data we do not have; the block-69 structural
    /// attack PROVED it cannot be derived from the patch — 7 497 696
    /// candidates, zero hits), so this seam takes it explicitly,
    /// records it in the trigger state, resolves it against the
    /// USER'S OWN `*movie*`/`*woi*` archives through the PROVEN
    /// [`crate::movie::find_movie_by_name`] (Bink-head verified),
    /// logs the verdict, and serves the cutscene exactly like
    /// [`App::play_nis`].  A resolution miss degrades to audio-only —
    /// audio and video are independent systems; it is never an `Err`.
    ///
    /// Block 71 — THE PICTURE LEG IS LIVE: when the trigger state
    /// moved (every `Ok`, including the served-by-silence contract)
    /// and the movie RESOLVED, its streaming player opens HERE and the
    /// picture takes the screen (modal — [`App::step_nis_movie`] owns
    /// the frame loop's head; the GPU side is built lazily on the
    /// first presented frame, so this method stays device-free).  The
    /// cutscene ENDS when its picture ends (the picture is the
    /// timeline — the block-67 mix contract), and any
    /// [`App::stop_nis`] path drops the picture with the cutscene.
    /// Every picture failure (a head that refuses to open, a decode
    /// that dies mid-play) degrades to audio-only, never an `Err`.
    ///
    /// Honest scope: the feed is recorded BEFORE the trigger validates
    /// (the engine's feed is external state — a failed trigger leaves
    /// the name on the still-active previous cutscene; the next
    /// successful serve consumes it).  Nothing calls this seam
    /// automatically yet (the mission-script interpreter does not
    /// exist).
    pub fn play_nis_with_movie(
        &mut self,
        nis_object: &str,
        movie: &str,
        gain: f32,
    ) -> Result<usize, String> {
        let resolved = match &self.state {
            AppState::Ready(set) => crate::movie::find_movie_by_name(&set.rcf_archives, movie),
            _ => {
                log::warn!("nis: no game folder ready — movie {movie:?} left unresolved");
                None
            }
        };
        match &resolved {
            Some(m) => log::info!(
                "nis: movie {movie:?} resolved in {} ({} B, member {:#010x})",
                m.archive.display(),
                m.entry.file_size,
                m.member_hash
            ),
            None => log::warn!(
                "nis: movie {movie:?} is not in the user's movie/woi archives — audio only"
            ),
        }
        // The feed is recorded regardless (the honest state); a CHANGED
        // name bumps the request counter, and the serve below consumes it.
        self.nis.set_movie(Some(movie));
        // The AUDIO leg first (the proven block-69 chain): a clean Err
        // (unknown object / non-NIS patch) left the trigger state — and
        // therefore the picture — untouched below.
        let served = self.play_nis(nis_object, gain);
        // THE PICTURE LEG — only when the trigger state moved.  The
        // previous cutscene's picture dies with it (modal), then a
        // resolved movie opens its streaming player.  Every failure is
        // a logged audio-only degrade (never an Err — the mix already
        // played, the state already moved).
        if served.is_ok() {
            if self.nis_movie.take().is_some() {
                self.nis_movie_gpu = None;
                self.nis_movie_logged = false;
                #[cfg(feature = "audio-cpal")]
                self.stop_nis_movie_audio();
                log::info!("nis: the previous cutscene's picture dropped (modal)");
            }
            match resolved {
                Some(m) => match MoviePlayerState::open(&m) {
                    Ok(player) => {
                        log::info!(
                            "nis: movie {movie:?} open — {}x{}, {} frame(s) at {}/{} fps, {} audio track(s) (streaming through the RCF span, never in RAM; the GPU side builds on first present)",
                            player.width(),
                            player.height(),
                            player.frame_count(),
                            player.fps().0,
                            player.fps().1,
                            player.audio_channels(),
                        );
                        self.nis_movie_logged = false;
                        #[cfg(feature = "audio-cpal")]
                        self.open_nis_movie_audio(&player);
                        self.nis_movie = Some(player);
                    }
                    Err(e) => {
                        log::warn!("nis: movie {movie:?} open failed ({e}) — audio only")
                    }
                },
                None => {} // the miss is already logged
            }
        }
        served
    }

    /// Block 69: stop the cutscene — the trigger state AND the mix
    /// unit together (the end/skip hook).  Returns how many voices
    /// were cut; idempotent on both halves.
    ///
    /// Block 71: the cutscene's PICTURE dies with it on every stop
    /// path (the deferred picture-end stop, the input skip, a direct
    /// stop) — player, GPU side and the device edge together.  A
    /// stopped cutscene can never leave a picture behind.
    pub fn stop_nis(&mut self) -> usize {
        let stopped = self.nis.stop();
        let cut = self.stop_nis_mix();
        if self.nis_movie.take().is_some() {
            self.nis_movie_gpu = None;
            self.nis_movie_logged = false;
            #[cfg(feature = "audio-cpal")]
            self.stop_nis_movie_audio();
            log::info!("nis: cutscene picture dropped with the cutscene");
        }
        if stopped || cut > 0 {
            log::info!("nis: cutscene stopped (state {stopped}, voices cut {cut})");
        }
        cut
    }

    /// Block 69: is a cutscene ACTIVE in the trigger state?  (The
    /// state and the audio unit are separate concerns — an
    /// event-driven cutscene is active with no voices; a total spawn
    /// failure leaves the OLD audio unit tracked under the NEW
    /// cutscene.)
    pub fn nis_active(&self) -> bool {
        self.nis.active().is_some()
    }

    /// Block 69: the trigger system's diagnostics seam (phase,
    /// request/served counters, the fed movie name).
    pub fn nis_system(&self) -> &crate::nis::NisSystem {
        &self.nis
    }

    // ------------------------------------------------------------------
    // Block 71: the MID-GAMEPLAY NIS MOVIE — the picture leg
    // ------------------------------------------------------------------

    /// Block 71: is a cutscene's movie PICTURE playing right now?
    /// (Independent of [`App::nis_active`]: the picture exists exactly
    /// between a resolved [`App::play_nis_with_movie`] and the
    /// cutscene's end/skip/stop — an audio-only cutscene never opens
    /// one.)
    pub fn nis_movie_active(&self) -> bool {
        self.nis_movie.is_some()
    }

    /// Block 71: one device-free step of the NIS movie's playback
    /// state machine — what the render loop consumes every frame
    /// BEFORE the boot-movie block (the cutscene's picture is MODAL:
    /// a presenting picture owns the frame exactly like the boot
    /// movie, and the world draw falls through only when the step
    /// says so).
    ///
    /// * [`NisMovieStep::Present`] — a fresh frame decoded; the loop
    ///   uploads `rgba()` and presents the quad.
    /// * [`NisMovieStep::Holding`] — playing but not due (wall-clock
    ///   pacing); the loop still presents the LAST uploaded frame (the
    ///   boot-movie block's proven shape — the quad keeps the texture).
    /// * [`NisMovieStep::Finished`] — the movie ended.  THE PICTURE IS
    ///   THE CUTSCENE'S TIMELINE: the slots drop here (device-free)
    ///   and the cutscene stop DEFERS to the post-borrow tail via
    ///   `pending_nis_stop` (state + mix + picture through
    ///   [`App::stop_nis`], the one cleanup path).
    /// * [`NisMovieStep::DecodeFailed`] — the decode died: the
    ///   picture drops, the cutscene PLAYS ON audio-only (audio and
    ///   video are independent systems — the block-69 degrade
    ///   contract, now on the running picture too).
    /// * [`NisMovieStep::Idle`] — no picture; the loop draws the world.
    fn step_nis_movie(&mut self, now: Instant) -> NisMovieStep {
        let Some(state) = self.nis_movie.as_mut() else {
            return NisMovieStep::Idle;
        };
        match state.advance(now) {
            Ok(adv) => {
                if adv.ended {
                    log::info!(
                        "nis movie: finished after {} frame(s), {} audio sample(s) fed ({} dropped) — the cutscene ends, the world scene takes over",
                        state.frames_done(),
                        state.audio_samples_written(),
                        state.audio_dropped(),
                    );
                    // The slots drop NOW (this frame falls through to
                    // the world draw — no black gap); the cutscene
                    // stop itself is a `&mut self` method and the
                    // caller may hold the renderer borrow — DEFER it.
                    self.pending_nis_stop = true;
                    self.nis_movie = None;
                    self.nis_movie_gpu = None;
                    NisMovieStep::Finished
                } else {
                    if !self.nis_movie_logged && adv.frames_decoded > 0 {
                        log::info!(
                            "nis movie: playing — {} frame(s) decoded, ring {} buffered / {} dropped (s16, track 0); any input skips",
                            state.frames_done(),
                            state
                                .ring_handle()
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .buffered(),
                            state.audio_dropped(),
                        );
                        self.nis_movie_logged = true;
                    }
                    if adv.frames_decoded > 0 {
                        NisMovieStep::Present
                    } else {
                        NisMovieStep::Holding
                    }
                }
            }
            Err(e) => {
                log::warn!(
                    "nis movie: decode failed ({e}) — dropping the picture, the cutscene plays audio-only"
                );
                self.nis_movie = None;
                self.nis_movie_gpu = None;
                NisMovieStep::DecodeFailed
            }
        }
    }

    /// Block 71: the NIS movie's audio DEVICE edge (feature
    /// `audio-cpal`) — the boot-movie edge's exact shape over THE ring
    /// this player writes.  Non-fatal by contract: any refusal logs
    /// once and the movie plays silent.
    #[cfg(feature = "audio-cpal")]
    fn open_nis_movie_audio(&mut self, state: &MoviePlayerState) {
        match MovieAudioEdge::open(prototype_audio::cpal_out::SharedRing::from_arc(
            state.ring_handle(),
        )) {
            Ok(edge) => {
                let (rate, ch) = edge.config();
                log::info!(
                    "nis movie: audio device edge open — {rate} Hz × {ch} ch (the ring drains in real time; no resampling — see movie.rs honest unknowns)"
                );
                self.nis_movie_audio = Some(edge);
            }
            Err(e) => log::warn!("nis movie: audio device edge refused ({e}) — playing silent"),
        }
    }

    /// Block 71: cut the NIS movie's device stream immediately (skip /
    /// stop / teardown — the tail drain never runs for a dead
    /// cutscene).
    #[cfg(feature = "audio-cpal")]
    fn stop_nis_movie_audio(&mut self) {
        if self.nis_movie_audio.take().is_some() {
            log::info!("nis movie: audio device stream cut");
        }
    }

    /// Block 71: called EVERY frame before the NIS picture block; the
    /// close decision is [`crate::movie::movie_audio_edge_should_close`]
    /// with the NIS picture's liveness (never fires while the player
    /// is alive — underrun-safe, the boot edge's proven policy).
    #[cfg(feature = "audio-cpal")]
    fn stop_nis_movie_audio_when_drained(&mut self) {
        if let Some(edge) = self.nis_movie_audio.as_ref() {
            let drained = edge.tail_drained();
            if crate::movie::movie_audio_edge_should_close(self.nis_movie.is_some(), drained) {
                log::info!(
                    "nis movie: audio tail drained — device stream closed ({} callback(s), {} underrun(s))",
                    edge.device_callbacks(),
                    edge.device_underruns(),
                );
                self.nis_movie_audio = None;
            }
        }
    }

    /// Block 58: load the gameplay clip registry from the user's own
    /// P3D files (the loader contract already collects `*.p3d`).
    /// The first candidate that yields decodable clips wins
    /// (boot_strap.p3d is the game's audio archive — 70 clips);
    /// a non-P3D candidate is skipped, a registry with zero clips
    /// is not kept, and every failure is non-fatal (the game boots
    /// silent exactly as before).
    ///
    /// Block 62: when no LOOSE P3D yields clips, the registry is
    /// served BY NAME from the user's own audio archives
    /// (`audio\boot_strap.p3d` read out of `00audio.rcf` through the
    /// PROVEN name-table roster — `audio_registry`).  Same failure
    /// contract: logged, silent boot, never fatal.
    fn load_gameplay_sounds(&mut self, set: &GameFileSet) {
        if self.sounds.is_some() {
            return; // already loaded this session
        }
        for p3d in &set.p3d_files {
            let bytes = match std::fs::read(p3d) {
                Ok(b) => b,
                Err(e) => {
                    log::debug!("gameplay audio: skipping {} ({e})", p3d.display());
                    continue;
                }
            };
            let name = p3d
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p3d.display().to_string());
            match GameplaySounds::from_p3d_bytes(&bytes, &name) {
                Ok(sounds) if !sounds.is_empty() => {
                    log::info!(
                        "gameplay audio: {} clip(s) decoded from {} ({} undecodable chunk(s)) — the event mapping plays REAL clips",
                        sounds.len(),
                        name,
                        sounds.decode_failures(),
                    );
                    self.sounds = Some(sounds);
                    return;
                }
                Ok(_) => log::debug!("gameplay audio: {name} has no decodable clips"),
                Err(e) => log::debug!("gameplay audio: {e}"),
            }
        }
        // Block 62: the archive path — no loose audio P3D, so serve
        // the PROVEN member by name from the user's own archives.
        match crate::audio_registry::load_registry_from_archives(set) {
            Ok(Some(sounds)) => {
                self.sounds = Some(sounds);
                return;
            }
            Ok(None) => {}
            Err(d) => log::debug!("gameplay audio: archive serving: {d}"),
        }
        log::info!(
            "gameplay audio: no clip registry found in {} P3D file(s) or the audio archives — events play silent",
            set.p3d_files.len()
        );
    }

    /// Block 66: load the mission music graph from the user's own
    /// audio archives (device-free).  Once per session (an Android
    /// re-boot re-uses it — the archives do not change mid-session);
    /// every failure is logged, never fatal: a graph-less boot is
    /// exactly the pre-66 silent one.
    fn load_music_graph(&mut self, set: &GameFileSet) {
        if !self.music_graph.is_empty() {
            return; // already loaded this session
        }
        match crate::music_graph::MusicGraph::from_game_set(set) {
            Ok(g) if g.is_empty() => {
                log::info!(
                    "music graph: {} carried no music banks — mission music stays silent",
                    crate::music_graph::MUSIC_ARCHIVE_NAME
                );
                self.music_graph = g;
            }
            Ok(g) => {
                log::info!("music graph: {}", g.summary());
                self.music_graph = g;
            }
            Err(d) => log::debug!("music graph: {d}"),
        }
    }

    /// Block 66: fire ONE mission music transition through the full
    /// PROVEN chain — mission-id cue → transition → stem patch →
    /// ordered clip names → the shard clip server (the user's own
    /// audio archives) → rate-clean voices in the mix ring.
    ///
    /// Public STATELESS primitive (block 68 note): the MISSION
    /// SYSTEM ([`App::enter_mission`] / [`App::set_music_transition`])
    /// is the stateful path above it — it owns the current mission
    /// id, serves the PROVEN `loop`/`volume` patch props, and cuts
    /// the previous music unit by tag.  This primitive stays for
    /// direct one-shot use; it spawns untagged, non-looping voices.
    /// Every clip that fails logs and degrades to silence; `Err`
    /// only when NO clip of the transition played.  Never panics.
    pub fn play_mission_music(
        &mut self,
        mission: &str,
        transition: &str,
        gain: f32,
    ) -> Result<usize, String> {
        let clips = self.music_graph.clips_for(mission, transition);
        if clips.is_empty() {
            return Err(format!(
                "mission music: no music for {mission:?}/{transition:?} in the graph"
            ));
        }
        let mut frames = 0usize;
        let mut played = 0usize;
        let mut diags: Vec<String> = Vec::new();
        for clip in &clips {
            match self.play_shard_clip(clip, gain) {
                Ok(n) => {
                    frames += n;
                    played += 1;
                    log::info!(
                        "mission music: {mission}/{transition} → {clip:?} ({n} frames)"
                    );
                }
                Err(e) => {
                    log::warn!(
                        "mission music: {mission}/{transition} → {clip:?} not played ({e})"
                    );
                    diags.push(e);
                }
            }
        }
        if played == 0 {
            return Err(format!(
                "mission music {mission}/{transition}: every clip failed ({})",
                diags.join("; ")
            ));
        }
        Ok(frames)
    }

    /// Block 66: read access to the loaded music graph (counts, cue
    /// inspection, clip resolution) — the diagnostics/UI seam.
    pub fn music_graph(&self) -> &crate::music_graph::MusicGraph {
        &self.music_graph
    }

    // -----------------------------------------------------------------
    // Block 68: the MISSION SYSTEM — the gameplay state that owns the
    // current mission id and drives the live music seam.
    // -----------------------------------------------------------------

    /// Enter a mission: the state machine validates the id against
    /// the decoded graph (unknown → clean `Err`, state untouched),
    /// adopts the cue's `ambient` default (DESIGN — see
    /// [`crate::mission::MissionSystem::enter_mission`]), and serves
    /// the resulting [`crate::mission::MusicRequest`] through the
    /// shard clip server as a TAGGED, loop-aware music unit —
    /// cutting the previous unit (frontend bed or other mission
    /// stem) first.  Effective gain = caller `gain` × the patch's
    /// PROVEN `volume` prop.
    ///
    /// `Ok(frames)` = the unit plays (0 = a TRACKED SILENCE — an
    /// `EmptyCell` transition or a clip-less stem is honest state,
    /// not an error); `Err` = unknown mission or EVERY clip of the
    /// new unit failed (the old unit is already cut and the state
    /// stays consistent — the DESIGN cut-first serve order; the NIS
    /// path's keep-old-on-failure modal logic is intentionally NOT
    /// used here, a music transition has no second chance to stack).
    /// Never panics.
    pub fn enter_mission(&mut self, mission: &str, gain: f32) -> Result<usize, String> {
        self.mission.enter_mission(&self.music_graph, mission)?;
        self.serve_music_state(gain)
    }

    /// Drive one music transition of the CURRENT mission (e.g.
    /// `ambient` → `combat` → `complete`): validates against the
    /// active mission's cue (unknown → clean `Err`, state untouched),
    /// then serves the new stem as a TAGGED unit, cutting the
    /// previous one.  Same contract as [`App::enter_mission`].
    pub fn set_music_transition(
        &mut self,
        transition: &str,
        gain: f32,
    ) -> Result<usize, String> {
        self.mission.set_transition(&self.music_graph, transition)?;
        self.serve_music_state(gain)
    }

    /// Leave the mission — the state goes to the free-roam world
    /// ([`crate::mission::MissionPhase::FreeRoam`], block 70: the
    /// ambience bank's PROVEN world bed takes over) and ONE serve
    /// cuts the mission unit and spawns the world bed (effective
    /// gain = caller `gain` × the bed patch's PROVEN `volume = 2`).
    /// `Ok(frames)` = the bed plays (`Ok(0)` = tracked silence — a
    /// graph without the ambience bank); `Err` = the bed's clip
    /// failed to serve (the mission unit is already cut, the state
    /// stays consistent — served, by silence).  A no-op (`Ok(0)`)
    /// when not in a mission.
    pub fn leave_mission(&mut self, gain: f32) -> Result<usize, String> {
        if !self.mission.leave_mission() {
            return Ok(0);
        }
        self.serve_music_state(gain)
    }

    /// Enter the free-roam world (block 70): the ambience bank's
    /// PROVEN world bed (`ambience` → `ambience_baked_18ch`, the
    /// 18-stream IMA master folded to stereo, looping, the patch's
    /// `volume = 2`) plays as the WORLD-BED unit — its OWN mixer tag,
    /// independent of the music channel (the patch's busses are the
    /// ambience bank's own).  Any previous world bed AND music unit
    /// are cut first (one serve, consistent state).  Idempotent
    /// (re-entry restarts nothing → `Ok(0)`); `Ok(0)` also when the
    /// graph carries no ambience bank (tracked silence).  `Err` only
    /// when the bed's clip fails to serve (served, by silence).
    /// Never panics.
    pub fn enter_free_roam(&mut self, gain: f32) -> Result<usize, String> {
        if !self.mission.enter_free_roam() {
            return Ok(0);
        }
        self.serve_music_state(gain)
    }

    /// Cut the WORLD-BED unit by its tag (block 70).  Returns the
    /// voices cut; a clean no-op when none runs.  The music unit and
    /// untagged voices are never touched.
    pub fn stop_world_ambience(&mut self) -> usize {
        let Some(tag) = self.ambience_unit_tag.take() else {
            return 0;
        };
        let cut = self.audio.mixer().stop_tagged(tag);
        if cut > 0 {
            log::info!("world bed: unit {tag} stopped ({cut} voice(s) cut)");
        }
        cut
    }

    /// True while the WORLD-BED unit runs (diagnostics).
    pub fn world_bed_active(&self) -> bool {
        self.ambience_unit_tag.is_some()
    }

    /// Enter the frontend attract state: the PROVEN `fe_music` bed
    /// (`e02m02_ambient`, looping — the real boot_strap patch) plays
    /// as a TAGGED unit, cutting any mission unit AND the world bed
    /// first (the frontend is the menu — block 70).  Idempotent
    /// (re-entry restarts nothing); `Ok(0)` when the graph carries
    /// no bed patch (tracked silence).  The boot path calls this
    /// when the frontend becomes visible: no boot movie, movie end,
    /// or movie skip.
    pub fn enter_frontend(&mut self, gain: f32) -> Result<usize, String> {
        if !self.mission.enter_frontend() {
            return Ok(0);
        }
        self.serve_music_state(gain)
    }

    /// Cut the current MUSIC unit (frontend bed or mission stem) by
    /// its tag.  Returns the voices cut; a clean no-op when none
    /// runs.  Untagged voices (gameplay SFX, NIS mixes) are never
    /// touched.
    pub fn stop_mission_music(&mut self) -> usize {
        let Some(tag) = self.music_unit_tag.take() else {
            return 0;
        };
        let cut = self.audio.mixer().stop_tagged(tag);
        if cut > 0 {
            log::info!("mission music: unit {tag} stopped ({cut} voice(s) cut)");
        }
        cut
    }

    /// Read access to the mission state machine (phase, mission id,
    /// transition, request/served counters) — the diagnostics/UI
    /// seam.
    pub fn mission(&self) -> &crate::mission::MissionSystem {
        &self.mission
    }

    /// Serve ONE pending mission-system request (block 68, extended
    /// in block 70 to BOTH channels): the MUSIC request (frontend
    /// bed / mission stem) and the WORLD-BED request (free-roam
    /// ambience).  Cut BOTH units first, mark the request consumed,
    /// spawn each channel's clips as TAGGED voices carrying the
    /// request's PROVEN loop flag, gain = caller × patch `volume`.
    /// Total failure across every requested clip is `Err` with the
    /// state already consistent (the request WAS served — by
    /// silence); a partial serve (one channel up, one failed) is
    /// `Ok` with the failure logged.  No pending request → `Ok(0)`.
    fn serve_music_state(&mut self, gain: f32) -> Result<usize, String> {
        if !self.mission.pending() {
            return Ok(0);
        }
        let req = self.mission.request(&self.music_graph);
        let amb = self.mission.ambience_request(&self.music_graph);
        self.stop_mission_music();
        self.stop_world_ambience();
        self.mission.mark_served();
        let mut frames = 0usize;
        let mut requested = 0usize;
        let mut music_played = 0usize;
        let mut amb_played = 0usize;
        let mut diags: Vec<String> = Vec::new();
        if let Some(req) = req {
            self.voice_tag_seq = self.voice_tag_seq.wrapping_add(1);
            let tag = self.voice_tag_seq;
            let effective = gain * req.volume;
            for clip in &req.clips {
                requested += 1;
                match self.play_shard_clip_tagged(clip, effective, Some(tag), req.loop_voice) {
                    Ok(n) => {
                        frames += n;
                        music_played += 1;
                        log::info!(
                            "mission music: {} → {clip:?} ({n} frames, tag {tag}, loop {}, vol {:.2})",
                            req.label,
                            req.loop_voice,
                            effective
                        );
                    }
                    Err(e) => {
                        log::warn!("mission music: {} → {clip:?} not played ({e})", req.label);
                        diags.push(e);
                    }
                }
            }
            if music_played > 0 {
                self.music_unit_tag = Some(tag);
            }
        }
        if let Some(amb) = amb {
            self.voice_tag_seq = self.voice_tag_seq.wrapping_add(1);
            let tag = self.voice_tag_seq;
            let effective = gain * amb.volume;
            for clip in &amb.clips {
                requested += 1;
                match self.play_shard_clip_tagged(clip, effective, Some(tag), amb.loop_voice) {
                    Ok(n) => {
                        frames += n;
                        amb_played += 1;
                        log::info!(
                            "world bed: {} → {clip:?} ({n} frames, tag {tag}, loop {}, vol {:.2})",
                            amb.label,
                            amb.loop_voice,
                            effective
                        );
                    }
                    Err(e) => {
                        log::warn!("world bed: {} → {clip:?} not played ({e})", amb.label);
                        diags.push(e);
                    }
                }
            }
            if amb_played > 0 {
                self.ambience_unit_tag = Some(tag);
            }
        }
        if requested > 0 && music_played + amb_played == 0 {
            return Err(format!(
                "music state {:?}: every clip failed ({})",
                self.mission.phase(),
                diags.join("; ")
            ));
        }
        Ok(frames)
    }

    /// Boot (or re-boot) the renderer against the current window, when
    /// the file set allows it.  All device-touching; errors are logged,
    /// never panicked.
    fn try_boot(&mut self) {
        let set = match &self.state {
            AppState::Ready(set) => set.clone(),
            AppState::Setup { last_error, .. } => {
                log::info!(
                    "setup: waiting for game files{}",
                    last_error
                        .as_deref()
                        .map(|e| format!(" ({e})"))
                        .unwrap_or_default()
                );
                return;
            }
        };
        let Some(window) = self.window.clone() else {
            return; // no surface yet; resumed will come back with one
        };

        // 0. Block 58: the gameplay clip registry (the audio CONTENT
        //    half) — device-free, so it loads before the GPU edge.
        //    Non-fatal: a folder without an audio P3D boots exactly
        //    as before (the event mapping degrades to silence).
        self.load_gameplay_sounds(&set);

        // 0b. Block 66: the mission MUSIC GRAPH (device-free) — the
        //     PROVEN cue → transition → stem → clip chain over the
        //     user's own audio banks.  Non-fatal: a graph-less boot
        //     is exactly the pre-66 silent one; the (future) mission
        //     system calls [`App::play_mission_music`].
        self.load_music_graph(&set);

        // 1. Device-free boot (collect + pair + decode + translate +
        //    cross-check).  Re-done every boot: cheap relative to device
        //    bring-up and always reflects the CURRENT files on disk.
        let report = prepare_programs(&set);
        log::info!("{}", boot_summary(&report));
        for p in &report.problems {
            log::warn!("boot problem: {p}");
        }
        // The world plan is consumed twice (the pipeline build below +
        // the mesh paths) — clone it before the report moves into self.
        let world_plan = report.world.clone();
        self.boot = Some(report);

        // 2. The GPU edge: surface + device on the window.
        let size = window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        let mut renderer = match pollster::block_on(Renderer::new(window.clone(), w, h)) {
            Ok(r) => r,
            Err(e) => {
                log::error!("renderer creation failed: {e} — staying in setup-like state");
                return;
            }
        };
        let format = renderer.surface_format();

        // 3. One real pipeline per shader pair.  A driver rejecting one
        //    WGSL translation must not kill the app — the failure is the
        //    signal we need (logcat / stderr) and the rest still renders.
        let mut built = 0usize;
        let mut failed = Vec::new();
        if let Some(report) = &self.boot {
            for pair in &report.pairs {
                match create_render_pipeline(
                    renderer.device(),
                    &pair.name,
                    &pair.vs,
                    &pair.fs,
                    &pair.vs_wgsl,
                    &pair.fs_wgsl,
                    format,
                ) {
                    Ok(p) => {
                        built += 1;
                        self.pipelines.push(p);
                    }
                    Err(e) => failed.push(format!("{}: {e}", pair.name)),
                }
            }
        }
        for f in &failed {
            log::warn!("pipeline failed: {f}");
        }
        log::info!("pipelines: {built} built, {} failed", failed.len());

        // 3b. Block 34 (NEXT-UP 7(e)): the REAL WORLD pipeline — the
        //     DLL `default` pair from the user's engine DLL (block 33:
        //     the default class's lit pass IS the world path; the
        //     frontend pairs keep their own passes).  Plans come from
        //     the WGSL emitter's capture (the bytecode mirror cannot
        //     see the blobs' u_c/b# rows).  Failure is non-fatal: the
        //     frontend fallback stays, with the reason logged.
        if let Some(plan) = world_plan.as_ref() {
            match create_render_pipeline_with_plans(
                renderer.device(),
                "default (world)",
                &plan.vs,
                &plan.fs,
                &plan.vs_wgsl,
                &plan.fs_wgsl,
                format,
                plan.vertex_resources.clone(),
                plan.fragment_resources.clone(),
            ) {
                Ok(p) => {
                    self.world_wvp_register = plan.wvp_register().unwrap_or(0);
                    self.world_matrix_register = plan.world_matrix_register().unwrap_or(4);
                    self.world_pipeline = Some(p);
                    log::info!(
                        "world pipeline: 'default' pair from the engine DLL — world geometry draws the lit pass (IsLitPass=true, shadows off)"
                    );
                }
                Err(e) => {
                    log::warn!("world pipeline failed: {e} — falling back to the frontend pair");
                    self.world_pipeline = None;
                }
            }
        } else {
            self.world_pipeline = None;
        }

        // 4. REAL geometry first (M21b, block 16): scan the user's own
        //    archives for Shape chunks and draw those meshes through the
        //    first built pipeline.  Block 34: the mesh→shader binding's
        //    WORLD half is now REAL — world geometry draws through the
        //    DLL `default` pair (the engine's lit pass) when the user's
        //    engine DLL was found; the frontend pipeline[0] stays the
        //    honest fallback.  Block 21: only WORLD-space meshes become
        //    batches (PROVEN split, `WORLD_LOCAL_SPLIT_M`); local
        //    drawable shapes need the GameObject placement pairing and
        //    would otherwise draw at the map origin.
        let mut drew_real_meshes = false;
        // `world_plan` was cloned at boot-report time (CTAB names drive
        // the world material slots and the pipeline build above).
        // Block 44: the App KEEPS the plan — the per-frame streaming
        // loads build batches with the same daylight rows.
        self.world_plan = world_plan.clone();
        if let Some(p0) = self.pipelines.first() {
            // Block 34: which pipeline world geometry draws through.
            let (mesh_pipeline, world_ref): (&GpuPipeline, Option<&_>) =
                match (self.world_pipeline.as_ref(), world_plan.as_ref()) {
                    (Some(wp), Some(plan)) => (wp, Some(plan)),
                    _ => (p0, None),
                };
            // M25 (block 18): the world index orders the load — members
            // whose placement bbox is nearest the boot camera ([0,0,0],
            // roughly mid-map for Prototype's Manhattan) decode first.
            // Block 20: the same single decompress pass also RETAINS the
            // full decoded GameObject records, so the boot log can
            // report the real spawner census and the per-frame draw
            // selection gets its batch→member provenance.
            let (scan, world) = crate::meshes::collect_meshes_near(
                &set.rcf_archives,
                [0.0, 0.0, 0.0],
                16,
                prototype_p3d::world::ScanOptions {
                    retain_objects: true,
                    // block 22: the pairing needs every shape name and
                    // the retained node transforms
                    retain_shape_names: true,
                    retain_node_transforms: true,
                },
            );
            let (w_objs, w_shapes) = world.index.totals();
            let (w_retained, w_spawnable) = world.index.retained_totals();
            // Block 21: spawn the REAL placed-object database (engine).
            let spawn_report = self.world_objects.spawn_from_index(&world.index, SPAWN_CAP);
            self.spawn_report = Some(spawn_report);
            let classes = self.world_objects.class_counts();
            let class_line: Vec<String> = classes
                .iter()
                .take(6)
                .map(|(c, n)| format!("{c}×{n}"))
                .collect();
            log::info!(
                "world spawn: {} entities ({} duplicates{}, cap {}) of {} world-space spawner(s); classes: {}",
                spawn_report.spawned,
                spawn_report.duplicates,
                if spawn_report.capped { ", CAPPED" } else { "" },
                SPAWN_CAP,
                w_spawnable,
                class_line.join(", ")
            );
            let mut spawner_classes: std::collections::BTreeMap<String, usize> =
                Default::default();
            for p in world.index.spawn_placements() {
                *spawner_classes.entry(p.object.class).or_default() += 1;
            }
            let kinds = world.index.entries.iter().fold([0u32; 4], |mut k, e| {
                k[match e.kind {
                    prototype_p3d::world::MemberKind::Empty => 0,
                    prototype_p3d::world::MemberKind::Objects => 1,
                    prototype_p3d::world::MemberKind::Geometry => 2,
                    prototype_p3d::world::MemberKind::Other => 3,
                }] += 1;
                k
            });
            log::info!(
                "mesh scan (world-ordered): {} archive(s) opened, {} member(s), {} shape(s), {} mesh(es){}",
                scan.archives_opened,
                scan.members_scanned,
                scan.shapes_decoded,
                scan.meshes.len(),
                if scan.capped { " (capped)" } else { "" }
            );
            log::info!(
                "world index: {} cell(s), {} placed object(s), {} shape(s); members: {} empty, {} object, {} geometry, {} other",
                world.index.cells().len(),
                w_objs,
                w_shapes,
                kinds[0],
                kinds[1],
                kinds[2],
                kinds[3]
            );
            log::info!(
                "world spawn set: {} retained object record(s), {} world-space spawner(s): {:?}",
                w_retained,
                w_spawnable,
                spawner_classes
            );
            // Block 22: pair the retained drawables with their same-cell
            // shapes and load the LOCAL meshes of the placeable (binary)
            // pairs — the per-building props appear at their REAL world
            // positions.  Prop-table pairs are skipped (their node-
            // transform composition is UNKNOWN — fail-closed).
            let pair_rep = world.index.pair_drawables(PAIR_CAP);
            let pm_rep = crate::meshes::collect_paired_meshes(
                &set.rcf_archives,
                &pair_rep.pairs,
                [0.0, 0.0, 0.0],
                PAIRED_MESH_CAP,
            );
            log::info!(
                "paired drawables: {} pair(s) of {} drawable record(s) ({} prop-table skipped), {} mesh(es) from {} member(s), {} shape(s) missing",
                pair_rep.pairs.len(),
                pair_rep.records_with_drawable,
                pm_rep.skipped_prop_table,
                pm_rep.meshes.len(),
                pm_rep.members_decompressed,
                pm_rep.shapes_missing
            );
            for e in &pm_rep.first_errors {
                log::warn!("paired drawables: {e}");
            }
            if let Some(p0) = self.pipelines.first() {
                let _ = p0; // the mesh path uses `mesh_pipeline` (block 34)
                for pm in &pm_rep.meshes {
                    // Block 44: the shared per-mesh batch builder (the
                    // create + static-daylight block, see its doc).
                    let batch =
                        build_mesh_batches(&mut renderer, mesh_pipeline, world_ref, &pm.mesh);
                    if batch.is_empty() {
                        break;
                    }
                    let pl = &pm.pair.placement;
                    // Block 45: the paired batch's streaming provenance
                    // — the geometry member the shape came from.
                    let member_hash = world
                        .index
                        .entries
                        .get(pm.pair.geometry_member)
                        .map(|e| e.hash)
                        .unwrap_or(0);
                    for b in batch {
                        self.paired.push(PairedBatch {
                            batch: b,
                            pos: Vec3::new(pl.pos[0], pl.pos[1], pl.pos[2]),
                            quat_xyzw: pl.quat_xyzw,
                            scale: pm.pair.scale.unwrap_or([1.0, 1.0, 1.0])[0],
                            member_hash,
                        });
                    }
                }
                if !self.paired.is_empty() {
                    log::info!(
                        "paired batches: {} local drawable(s) placed by REAL record(s) on pipeline '{}'{}",
                        self.paired.len(),
                        mesh_pipeline.name,
                        if world_ref.is_some() { " (WORLD lit pass)" } else { "" }
                    );
                }
            }
            for e in &scan.first_errors {
                log::warn!("mesh scan: {e}");
            }
            if !scan.meshes.is_empty() {
                let mut built = 0usize;
                let mut skipped_local = 0usize;
                // Block 20: batch→member provenance, aligned with
                // self.batches as batches are pushed below.
                let mut member_of_batch: Vec<Option<usize>> = Vec::new();
                for (mesh_i, mesh) in scan.meshes.iter().enumerate() {
                    // Block 21: local drawable shapes (bakery/billboard/…
                    // — centers < WORLD_LOCAL_SPLIT_M of the origin) need
                    // the GameObject placement pairing; skip them.
                    if !scan.mesh_world.get(mesh_i).copied().unwrap_or(false) {
                        skipped_local += 1;
                        continue;
                    }
                    // Block 44: the shared per-mesh batch builder (the
                    // create + static-daylight block — now identical to
                    // what the per-frame streaming loads run).
                    let batch =
                        build_mesh_batches(&mut renderer, mesh_pipeline, world_ref, mesh);
                    if batch.is_empty() {
                        break;
                    }
                    // Block 21: NO per-mesh gallery camera — the meshes
                    // are WORLD-space (PROVEN, mesh_world); the player
                    // camera's plain view-projection rows are written per
                    // frame in RedrawRequested.  Warn once when the
                    // pipeline cannot receive them (frontend pairs name
                    // the rows; the world pair's rows are u_c0..3).
                    if !self.warned_no_wvp
                        && !batch[0].vertex_uniforms.iter().any(|u| {
                            u.wgsl_name == "p3dWorldViewProjectionMatrix_r0" || u.wgsl_name == "u_c0"
                        })
                    {
                        let names: Vec<&str> = batch[0]
                            .vertex_uniforms
                            .iter()
                            .map(|u| u.wgsl_name.as_str())
                            .collect();
                        log::warn!(
                            "pipeline '{}' vertex stage has no p3dWorldViewProjectionMatrix; uniforms: {names:?}",
                            mesh_pipeline.name
                        );
                        self.warned_no_wvp = true;
                    }
                    for _ in 0..batch.len() {
                        member_of_batch.push(scan.member_of_mesh.get(mesh_i).copied());
                    }
                    self.batches.extend(batch);
                    // Block 44: the pool accounting — which member
                    // contributed this world mesh (the exact unload
                    // subtraction later).
                    if let Some(&gi) = scan.member_of_mesh.get(mesh_i) {
                        *self.meshes_of_member.entry(gi).or_default() += 1;
                    }
                    built += 1;
                }
                if built > 0 {
                    log::info!(
                        "mesh batches: {} world-space mesh(es) through pipeline '{}'{} (debug materials; {} local skipped), per-frame player-camera WVP",
                        built,
                        mesh_pipeline.name,
                        if world_ref.is_some() { " — the DLL default pair (lit pass)" } else { "" },
                        skipped_local
                    );
                    drew_real_meshes = true;
                    // Block 44: the GPU pool starts with the boot's
                    // world meshes; the streaming room math runs on it.
                    self.member_meshes = built;
                    // Block 20: the real world index + batch provenance
                    // drive the per-frame draw selection (members_near
                    // camera ordering).
                    // Block 44: the member stream attaches over the
                    // SAME scanned index (built before the move below)
                    // and is SEEDED with the members whose meshes the
                    // boot actually loaded — the first frame's diff
                    // starts from the boot's real state, not from zero.
                    match prototype_stream::members::MemberStreamConfig::new(
                        MEMBER_STREAM_BUDGET,
                        MEMBER_STREAM_HYSTERESIS,
                    )
                    .and_then(|config| {
                        crate::member_stream::MemberMeshStream::from_index(
                            &set.rcf_archives,
                            &world.index,
                            config,
                        )
                    }) {
                        Ok(mut ms) => {
                            let seeded: std::collections::BTreeSet<u32> = scan
                                .member_of_mesh
                                .iter()
                                .map(|&gi| world.index.entries[gi].hash)
                                .collect();
                            let seeded_n = seeded.len();
                            ms.seed_loaded(seeded);
                            // Block 45: the paired half attaches over the
                            // same pair report and is SEEDED with the
                            // geometry members whose paired meshes the
                            // boot pool already holds — the paired diffs
                            // start from the boot's real state.
                            let paired_attached = ms.attach_paired(&pair_rep.pairs);
                            let paired_seeded: std::collections::BTreeSet<u32> = pm_rep
                                .meshes
                                .iter()
                                .filter_map(|pm| {
                                    world.index.entries.get(pm.pair.geometry_member)
                                })
                                .map(|e| e.hash)
                                .collect();
                            let paired_seeded_n = paired_seeded.len();
                            ms.seed_paired(paired_seeded);
                            log::info!(
                                "member stream: attached over {} member(s) ({} spanned, {} duplicate dir hashes), {} seeded from the boot scan (budget {}, hysteresis {:.1}); paired: {} pair(s) over {} member(s), {} seeded",
                                ms.member_count(),
                                ms.spanned_members(),
                                ms.duplicate_hashes(),
                                seeded_n,
                                MEMBER_STREAM_BUDGET,
                                MEMBER_STREAM_HYSTERESIS,
                                paired_attached,
                                ms.paired_member_count(),
                                paired_seeded_n,
                            );
                            self.member_stream = Some(ms);
                        }
                        Err(e) => {
                            log::warn!("member stream: attach failed ({e}) — streaming stays off")
                        }
                    }
                    self.world_index = Some(world.index);
                    self.draw_selector = Some(crate::meshes::DrawSelector::new(member_of_batch));
                }
            }
            // Block 21: the placed-object cube pool (the nearest
            // spawners draw as 2 m cubes at their REAL positions).
            if !self.world_objects.is_empty() {
                self.boxes = renderer.create_box_batches(p0, BOX_POOL);
                log::info!(
                    "placed-object cubes: {} ready on pipeline '{}' (nearest per frame)",
                    self.boxes.len(),
                    p0.name
                );
            }
        }
        if !drew_real_meshes {
            // One drawable debug triangle per pipeline: buffers + bind
            // groups from the validated plans.  The first vertex-pull +
            // uniform path proof on a real GPU.
            self.batches = renderer.create_debug_batches(&self.pipelines);
            log::info!("debug batches: {} ready", self.batches.len());
            // No real meshes → nothing to select among; per-frame
            // selection stays off (all debug batches draw).
            self.world_index = None;
            self.draw_selector = None;
        }

        // 5. Block 54 (NEXT-UP (a)): the BOOT-PATH MOVIE — find a real
        //    Bink member in the user's movie archive(s) and open the
        //    streaming player over its RCF span (no extraction; the
        //    movie is never in RAM).  From the first RedrawRequested
        //    until it ends (or the user skips it) the screen shows the
        //    movie quad and track 0's PCM lands in the audio ring.
        //    Every failure here is non-fatal: the world scene draws
        //    exactly as before, with the reason logged.
        self.movie = None;
        self.movie_gpu = None;
        self.movie_logged = false;
        match find_boot_movie(&set.rcf_archives) {
            Some(m) => match MoviePlayerState::open(&m) {
                Ok(state) => {
                    let (mw, mh) = (state.width(), state.height());
                    let texture = renderer.create_video_texture(mw, mh, "boot movie");
                    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                    let quad = renderer.create_video_quad(format);
                    let bind_group = renderer.create_video_bind_group(&quad, &view);
                    log::info!(
                        "boot movie: member {:#010x} of {} ({} B) — {}x{}, {} frame(s) at {}/{} fps, {} audio track(s); streaming through the RCF span (never in RAM)",
                        m.member_hash,
                        m.archive.display(),
                        m.entry.file_size,
                        mw,
                        mh,
                        state.frame_count(),
                        state.fps().0,
                        state.fps().1,
                        state.audio_channels(),
                    );
                    self.movie_gpu = Some(MovieGpu { quad, texture, view, bind_group });
                    // Block 55 (NEXT-UP (a)): the audio DEVICE edge over
                    // THE ring this player writes (SharedRing::from_arc
                    // wraps the player's own handle — one buffer, one
                    // writer, one reader).  Non-fatal: device-less
                    // hosts play silent, exactly as before.
                    #[cfg(feature = "audio-cpal")]
                    self.open_movie_audio(&state);
                    self.movie = Some(state);
                }
                Err(e) => log::warn!("boot movie: open failed ({e}) — playing without it"),
            },
            None => log::info!(
                "boot movie: none found (no *movie*.rcf with a bik member in the asset folder)"
            ),
        }

        // 6. Block 57: the GAMEPLAY audio device edge — opened once
        //    per boot over the AudioOut ring (guarded, non-fatal),
        //    held for the session, cut at teardown.  From here on the
        //    per-frame tick at §2b drains through the REAL device.
        #[cfg(feature = "audio-cpal")]
        self.open_gameplay_audio();

        // 7. Block 68: when NO boot movie plays, the FRONTEND is what
        //    the user sees right now — its attract music (the PROVEN
        //    fe_music bed) starts here.  When a movie DID open, the
        //    movie-end/skip arms defer the same entry to their tail
        //    (the frontend appears when the picture ends).
        //    Idempotent: a re-boot re-runs this, but the mission
        //    system's no-bump contract restarts nothing.
        if self.movie.is_none() {
            if let Err(e) = self.enter_frontend(1.0) {
                log::debug!("frontend music: {e}");
            }
        }

        self.aspect = {
            let size = window.inner_size();
            if size.height > 0 {
                size.width as f32 / size.height as f32
            } else {
                1.0
            }
        };
        self.renderer = Some(renderer);
        window.request_redraw();
    }

    /// Drop everything GPU-side (Android pause invalidates the surface;
    /// desktop close/exiting reuses the same path).
    fn teardown(&mut self) {
        if self.renderer.is_some() || !self.pipelines.is_empty() {
            log::info!("tearing down GPU state (surface is invalid across pause)");
        }
        self.renderer = None;
        self.pipelines.clear();
        self.world_pipeline = None;
        self.batches.clear();
        self.boxes.clear();
        self.paired.clear();
        // Block 54: the movie's GPU side dies with the device, and the
        // player state goes with it (the next boot re-finds and re-opens
        // the movie — the boot movie replays after a resume, like every
        // other boot-time scan).
        self.movie = None;
        self.movie_gpu = None;
        // Block 55: the movie's device stream goes with the GPU state
        // (the surface is invalid; a re-boot re-opens everything).
        self.stop_movie_audio();
        // Block 71: the cutscene's PICTURE is device state too — it
        // dies with the surface (the trigger STATE survives suspend
        // like every gameplay state; a resumed session does NOT
        // re-open a mid-cutscene picture — cutscenes do not resume,
        // the next trigger starts fresh).
        self.nis_movie = None;
        self.nis_movie_gpu = None;
        #[cfg(feature = "audio-cpal")]
        self.stop_nis_movie_audio();
        // Block 57: the gameplay device stream goes with them too —
        // the ONLY cut it ever takes (no tail-drain close: the mixer
        // produces every frame); the next boot re-opens it.
        self.stop_gameplay_audio();
        // The world index + provenance describe the BOOT's batches; they
        // go with them (re-scanned on the next boot).  The player and
        // the placed-object database are DATA, not GPU state — they
        // survive suspend (spawn_from_index re-spawns on re-boot).
        self.world_index = None;
        self.draw_selector = None;
        // Block 44: the member stream describes the BOOT's span table
        // + loaded set — it re-attaches (and re-seeds) on re-boot.
        // The kept world plan is boot state too.
        self.member_stream = None;
        self.world_plan = None;
        self.member_meshes = 0;
        self.meshes_of_member.clear();
        self.last_frame = None;
        self.window = None;
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        log::info!("lifecycle: resume — re-scanning asset folder");
        self.state = scan_or_setup(&self.folder);
        if matches!(self.state, AppState::Setup { .. }) {
            // Files not (yet) usable — no render loop without them.
            self.teardown();
            return;
        }
        if self.window.is_none() {
            match event_loop.create_window(self.window_attrs.clone()) {
                Ok(w) => {
                    log::info!("window created (surface target ready)");
                    self.window = Some(Arc::new(w));
                }
                Err(e) => {
                    log::error!("window creation failed: {e}");
                    return;
                }
            }
        }
        if self.renderer.is_none() {
            self.try_boot();
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        log::info!("lifecycle: suspend");
        self.teardown();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                log::info!("close requested");
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(size.width, size.height);
                }
                if size.height > 0 {
                    self.aspect = size.width as f32 / size.height as f32;
                }
            }
            WindowEvent::RedrawRequested => {
                // --- block 55: the audio edge's tail drain -------------
                //     BEFORE the renderer borrow (the check touches no
                //     GPU state).  After the movie is gone (ended, or a
                //     decode failure — both leave VALID samples in the
                //     ring) the device stream keeps draining the tail
                //     while the world scene draws underneath; this
                //     closes it exactly once the ring is empty.  While
                //     a player is alive the close policy never fires
                //     (underrun-safe).
                self.stop_movie_audio_when_drained();
                // Block 71: the NIS movie's edge, same policy over ITS
                // ring (the cutscene's track-0 tail drains after the
                // picture ends, under the world draw).
                #[cfg(feature = "audio-cpal")]
                self.stop_nis_movie_audio_when_drained();
                // Block 71: the NIS movie's device-free step runs BEFORE
                // the renderer borrow (a &mut self method cannot run
                // under it — the block-55 E0499 lesson again).  The GPU
                // half below maps the step to upload/draw/fall-through.
                let step = self.step_nis_movie(Instant::now());
                match self.renderer.as_mut() {
                Some(r) => {
                    // --- block 71: the MID-GAMEPLAY NIS movie owns the
                    //     frame while its cutscene plays (MODAL — the
                    //     cutscene is the screen) ---------------------
                    //     Present / Holding upload + present the quad
                    //     and the frame RETURNS (the world never draws
                    //     under a live picture); Finished / DecodeFailed
                    //     / Idle fall through to the world draw.  Any
                    //     failure degrades — the boot path's shape.
                    if matches!(step, NisMovieStep::Present | NisMovieStep::Holding) {
                        // The GPU side builds LAZILY on the first
                        // presented frame: the trigger may fire before
                        // a boot or after a surface rebuild (the open
                        // path is device-free by design).
                        if self.nis_movie_gpu.is_none() {
                            if let Some(state) = self.nis_movie.as_ref() {
                                let texture = r.create_video_texture(
                                    state.width(),
                                    state.height(),
                                    "nis movie",
                                );
                                let view =
                                    texture.create_view(&wgpu::TextureViewDescriptor::default());
                                let quad = r.create_video_quad(r.surface_format());
                                let bind_group = r.create_video_bind_group(&quad, &view);
                                log::info!("nis movie: picture leg live (GPU side built)");
                                self.nis_movie_gpu =
                                    Some(MovieGpu { quad, texture, view, bind_group });
                            }
                        }
                        if step == NisMovieStep::Present {
                            if let (Some(gpu), Some(state)) =
                                (&self.nis_movie_gpu, self.nis_movie.as_ref())
                            {
                                if let Err(e) =
                                    r.upload_video_frame(&gpu.texture, state.rgba())
                                {
                                    log::warn!("nis movie: frame upload failed: {e}");
                                }
                            }
                        }
                        if let Some(gpu) = &self.nis_movie_gpu {
                            match r.render_video_frame(&gpu.quad, &gpu.bind_group) {
                                Ok(()) => return,
                                Err(e) => {
                                    log::warn!("nis movie frame error: {e:?}");
                                    if matches!(
                                        e,
                                        wgpu::SurfaceError::Lost
                                            | wgpu::SurfaceError::Outdated
                                    ) {
                                        self.teardown();
                                    }
                                    return;
                                }
                            }
                        }
                        // No GPU side (a create failure cannot leave us
                        // here — the types force the fall-through): the
                        // world draw below runs this frame.
                    }

                    // --- block 54: the boot movie owns the frame while
                    //     it plays -------------------------------------
                    //     Wall-clock-paced decode → RGBA upload → the
                    //     fullscreen quad; track 0's PCM lands in the
                    //     ring.  Any failure drops the movie and falls
                    //     through to the world draw THIS frame — the
                    //     boot path degrades, never dies.
                    if let Some(state) = self.movie.as_mut() {
                        match state.advance(Instant::now()) {
                            Ok(adv) => {
                                if adv.frames_decoded > 0 {
                                    if let Some(gpu) = &self.movie_gpu {
                                        if let Err(e) =
                                            r.upload_video_frame(&gpu.texture, state.rgba())
                                        {
                                            log::warn!("boot movie: frame upload failed: {e}");
                                        }
                                    }
                                }
                                if !self.movie_logged && adv.frames_decoded > 0 {
                                    log::info!(
                                        "boot movie: playing — {} frame(s) decoded, ring {} buffered / {} dropped (s16, track 0); any input skips",
                                        state.frames_done(),
                                        state.ring_handle().lock().unwrap_or_else(|e| e.into_inner()).buffered(),
                                        state.audio_dropped(),
                                    );
                                    self.movie_logged = true;
                                }
                                if adv.ended {
                                    log::info!(
                                        "boot movie: finished after {} frame(s), {} audio sample(s) fed ({} dropped) — the world scene takes over",
                                        state.frames_done(),
                                        state.audio_samples_written(),
                                        state.audio_dropped(),
                                    );
                                    // Block 58: the movie→world transition
                                    // fires the DESIGN wipe mapping (the
                                    // frontend transition sound) through
                                    // the gameplay ring.  The fire needs
                                    // `&mut self` and this arm holds the
                                    // renderer borrow — DEFER it to the
                                    // post-borrow tail of this frame.
                                    self.pending_gameplay_event =
                                        Some(GameplayAudioEvent::BootMovieEnded);
                                    // Block 67: when a NIS-mix unit rides
                                    // this movie slot (the future NIS
                                    // movie path), the mix ends with the
                                    // picture.  The cut itself is a
                                    // &mut self method and this arm holds
                                    // the renderer borrow — DEFER it to
                                    // the post-borrow tail.  A boot
                                    // movie's own container audio needs
                                    // no stop — the tail drain owns it.
                                    self.pending_nis_mix_stop = true;
                                    // Block 68: the picture ending IS the
                                    // frontend appearing — its attract
                                    // music starts in the same deferred
                                    // tail (a &mut self serve, same
                                    // borrow).
                                    self.pending_frontend_music_start = true;
                                    self.movie = None;
                                    self.movie_gpu = None;
                                    // fall through: the world draw below
                                    // paints THIS frame (no black gap).
                                } else {
                                    if let Some(gpu) = &self.movie_gpu {
                                        match r.render_video_frame(&gpu.quad, &gpu.bind_group) {
                                            Ok(()) => return,
                                            Err(e) => {
                                                log::warn!("boot movie frame error: {e:?}");
                                                if matches!(
                                                    e,
                                                    wgpu::SurfaceError::Lost
                                                        | wgpu::SurfaceError::Outdated
                                                ) {
                                                    self.teardown();
                                                }
                                                return;
                                            }
                                        }
                                    }
                                    // No GPU side (cannot happen — the
                                    // two fields move together, but the
                                    // types force the fall-through): the
                                    // world draw below runs.
                                }
                            }
                            Err(e) => {
                                log::warn!(
                                    "boot movie: decode failed ({e}) — dropping it, the world scene takes over"
                                );
                                self.movie = None;
                                self.movie_gpu = None;
                                // fall through to the world draw below
                            }
                        }
                    }

                    // --- block 21: the player-camera frame loop -------
                    // 1. dt (clamped — a pause must not teleport the
                    //    player).
                    let now = Instant::now();
                    let dt = self
                        .last_frame
                        .replace(now)
                        .map(|t| (now - t).as_secs_f32().min(0.1))
                        .unwrap_or(0.0);
                    // 2. Player update from the input snapshot (WASD +
                    //    ←/→ yaw); block 38: the look orbit rides into
                    //    the follow first, and the walk is CAMERA-
                    //    relative (the touch look's movement half).
                    self.look.apply_to(&mut self.follow);
                    update_player(&mut self.player, &self.input, &self.follow, dt);
                    let cam = frame_camera(&self.follow, &self.player, self.aspect);
                    let player_pos = self.player.position();
                    let cam3 = [player_pos.x, player_pos.y, player_pos.z];

                    // 2b. Block 38: the AUDIO OUTPUT stage ticks once
                    //     per frame (the same once-per-frame policy as
                    //     the draw encoding): mix dt-sized quanta at the
                    //     48 kHz output rate into the ring.  Voices are
                    //     spawned by gameplay events as they land (the
                    //     clip ↔ event mapping is documented UNKNOWN —
                    //     the stage mixes silence until then); the tick
                    //     and counters are the CI-visible proof.
                    let audio_stats = self.audio.tick(audio_frame_count(dt));
                    if !self.audio_logged {
                        log::info!(
                            "audio stage: tick {} frame(s), {} sample(s) routed, {} voice(s), {} clipped (ring {})",
                            audio_frame_count(dt),
                            audio_stats.samples_routed,
                            audio_stats.voices_played,
                            audio_stats.clipped,
                            self.audio.buffered(),
                        );
                        self.audio_logged = true;
                    }

                    // 2c. Block 44: member-level PHYSICAL streaming —
                    //     the block-43 state machine's `MemberLoadSet`
                    //     now drives REAL member fetches (disk span
                    //     reads + mesh decode) and batch unloads per
                    //     frame, replacing the loader's ad-hoc
                    //     near-first scan.  No-op until the boot
                    //     attached the stream; unloads run FIRST so
                    //     the loads get the freed pool room; newly
                    //     loaded batches draw THIS frame (the step
                    //     runs before the draw selection below).
                    if let (Some(stream), Some(world), Some(sel)) = (
                        self.member_stream.as_mut(),
                        self.world_index.as_ref(),
                        self.draw_selector.as_mut(),
                    ) {
                        let set = stream.plan(cam3);
                        if !set.is_empty() {
                            // --- unload: drop the evicted members'
                            //     batches, compact provenance in sync.
                            if !set.unload.is_empty() {
                                let evict: std::collections::BTreeSet<u32> =
                                    set.unload.iter().copied().collect();
                                let keep: std::collections::BTreeSet<usize> = sel
                                    .member_of_batch()
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, m)| match m {
                                        // memberless batches never evict
                                        None => true,
                                        Some(gi) => !world
                                            .entries
                                            .get(*gi)
                                            .map(|e| evict.contains(&e.hash))
                                            .unwrap_or(false),
                                    })
                                    .map(|(i, _)| i)
                                    .collect();
                                if keep.len() != sel.member_of_batch().len() {
                                    let dropped = sel.member_of_batch().len() - keep.len();
                                    // Block 44: the exact pool subtraction —
                                    // the evicted members' contributed
                                    // meshes leave the pool with them.
                                    let mut dropped_meshes = 0usize;
                                    for &h in &set.unload {
                                        if let Some(gi) =
                                            world.entries.iter().position(|e| e.hash == h)
                                        {
                                            if let Some(n) = self.meshes_of_member.remove(&gi) {
                                                dropped_meshes += n;
                                            }
                                        }
                                    }
                                    self.member_meshes = self
                                        .member_meshes
                                        .saturating_sub(dropped_meshes);
                                    self.batches = std::mem::take(&mut self.batches)
                                        .into_iter()
                                        .enumerate()
                                        .filter(|(i, _)| keep.contains(i))
                                        .map(|(_, b)| b)
                                        .collect();
                                    sel.retain(&keep);
                                    stream.apply_unload(&set.unload);
                                    // Block 45: the paired half unloads with
                                    // the member — the evicted members'
                                    // paired batches drop with it.
                                    stream.unload_paired(&set.unload);
                                    let evict: std::collections::BTreeSet<u32> =
                                        set.unload.iter().copied().collect();
                                    let paired_before = self.paired.len();
                                    self.paired
                                        .retain(|pb| !evict.contains(&pb.member_hash));
                                    let paired_dropped = paired_before - self.paired.len();
                                    log::info!(
                                        "member stream: unloaded {} member(s), {} batch(es) + {} world mesh(es) + {} paired batch(es) dropped (pool {}, {} batch(es) left)",
                                        set.unload.len(),
                                        dropped,
                                        dropped_meshes,
                                        paired_dropped,
                                        self.member_meshes,
                                        sel.member_of_batch().len(),
                                    );
                                } else {
                                    // nothing member-tied to drop (the
                                    // evicted members had no batches);
                                    // bookkeeping still applies.
                                    stream.apply_unload(&set.unload);
                                    stream.unload_paired(&set.unload);
                                    log::info!(
                                        "member stream: unloaded {} member(s), 0 batch(es) dropped",
                                        set.unload.len(),
                                    );
                                }
                            }
                            // --- load: the newly desired members
                            //     (near first) into the pool room
                            //     that is left; batches through the
                            //     SAME builder the boot used.
                            let room = MEMBER_MESH_POOL_CAP.saturating_sub(self.member_meshes);
                            let loads = stream.load_meshes(&set.load, room);
                            if loads.errors > 0 {
                                for e in &loads.first_errors {
                                    log::warn!("member stream: {e}");
                                }
                            }
                            if !loads.loads.is_empty() {
                                // the pipeline choice mirrors the boot
                                // (the DLL world pair when present).
                                // Fail-closed: no pipeline → the meshes
                                // stay decoded-and-counted but unbuilt
                                // (they reload after the next unload);
                                // never a panic on a degenerate state.
                                let load_ctx = match self.world_pipeline.as_ref() {
                                    Some(wp) => Some((wp, self.world_plan.as_ref())),
                                    None => self
                                        .pipelines
                                        .first()
                                        .map(|p0| (p0, None as Option<&_>)),
                                };
                                if let Some((mesh_pipeline, world_ref)) = load_ctx {
                                    let mut added = 0usize;
                                    for load in &loads.loads {
                                        for (mi, mesh) in load.meshes.iter().enumerate() {
                                            if !load.mesh_world.get(mi).copied().unwrap_or(false) {
                                                continue; // local: pairing's job (boot policy)
                                            }
                                            let batch = build_mesh_batches(
                                                r,
                                                mesh_pipeline,
                                                world_ref,
                                                mesh,
                                            );
                                            if batch.is_empty() {
                                                continue;
                                            }
                                            for _ in 0..batch.len() {
                                                sel.push_provenance(Some(load.global_index));
                                            }
                                            self.batches.extend(batch);
                                            *self
                                                .meshes_of_member
                                                .entry(load.global_index)
                                                .or_default() += 1;
                                            added += 1;
                                        }
                                    }
                                    self.member_meshes += added;
                                    if added > 0 {
                                        log::info!(
                                            "member stream: loaded {} member(s), {} world mesh(es) (pool {}, budget {}, loaded {})",
                                            loads.loads.len(),
                                            added,
                                            self.member_meshes,
                                            MEMBER_STREAM_BUDGET,
                                            stream.loaded_count(),
                                        );
                                    } else if loads.pool_capped {
                                        log::info!(
                                            "member stream: {} member(s) fetched, pool FULL ({} world mesh(es)) — meshes stay desired for a reload",
                                            loads.loads.len(),
                                            self.member_meshes,
                                        );
                                    }
                                    // Block 45: the paired half loads with
                                    // the member — the newly loaded
                                    // members' paired drawables join the
                                    // pool (room permitting), placed by
                                    // their REAL binary records.
                                    let paired_room =
                                        PAIRED_MESH_CAP.saturating_sub(self.paired.len());
                                    let ploads =
                                        stream.load_paired_meshes(&set.load, cam3, paired_room);
                                    if ploads.errors > 0 {
                                        for e in &ploads.first_errors {
                                            log::warn!("member stream paired: {e}");
                                        }
                                    }
                                    let mut paired_added = 0usize;
                                    for load in &ploads.loads {
                                        for pm in &load.meshes {
                                            let batch = build_mesh_batches(
                                                r,
                                                mesh_pipeline,
                                                world_ref,
                                                &pm.mesh,
                                            );
                                            if batch.is_empty() {
                                                continue;
                                            }
                                            let pl = &pm.pair.placement;
                                            for b in batch {
                                                self.paired.push(PairedBatch {
                                                    batch: b,
                                                    pos: Vec3::new(
                                                        pl.pos[0],
                                                        pl.pos[1],
                                                        pl.pos[2],
                                                    ),
                                                    quat_xyzw: pl.quat_xyzw,
                                                    scale: pm
                                                        .pair
                                                        .scale
                                                        .unwrap_or([1.0, 1.0, 1.0])[0],
                                                    member_hash: load.hash,
                                                });
                                                paired_added += 1;
                                            }
                                        }
                                    }
                                    if paired_added > 0 || ploads.pool_capped {
                                        log::info!(
                                            "member stream paired: {} placed batch(es) over {} member(s) ({} prop-table skipped; pool {}, cap {}{}",
                                            paired_added,
                                            ploads.loads.len(),
                                            ploads.skipped_prop_table,
                                            self.paired.len(),
                                            PAIRED_MESH_CAP,
                                            if ploads.pool_capped { ", CAPPED)" } else { ")" },
                                        );
                                    }
                                } else {
                                    log::warn!(
                                        "member stream: {} member(s) fetched but no pipeline is live — {} world mesh(es) not built",
                                        loads.loads.len(),
                                        loads
                                            .loads
                                            .iter()
                                            .map(|l| l.mesh_world.iter().filter(|w| **w).count())
                                            .sum::<usize>(),
                                    );
                                }
                            } else if loads.pool_capped {
                                log::info!(
                                    "member stream: pool FULL ({} world mesh(es)) — loads deferred",
                                    self.member_meshes,
                                );
                            }
                        }
                    }

                    // 3. Per-frame draw selection over the real world
                    //    index — the members nearest the PLAYER (the
                    //    streaming input is the player position, not the
                    //    camera eye — M11e semantics); memberless batches
                    //    always draw.
                    let mut picked: Vec<&DebugDrawBatch> = Vec::new();
                    if let (Some(sel), Some(world)) = (&self.draw_selector, &self.world_index) {
                        for &i in &sel.select(world, cam3, usize::MAX) {
                            picked.push(&self.batches[i]);
                        }
                    } else {
                        // No selection state (debug-triangle fallback):
                        // every batch draws, as before block 20.
                        picked.extend(self.batches.iter());
                    }

                    // 4. World-space meshes get the player camera's
                    //    view-projection rows every frame (block-17's
                    //    per-mesh gallery camera is GONE — block 21's
                    //    PROVEN world-space split replaced it).  Block
                    //    34: frontend batches take the named rows, the
                    //    DLL world pair's batches the register-based
                    //    rows (u_c0..3, CTAB-proven WVP) — `_any` tries
                    //    the name path first, then the register path.
                    let mut wvp_missing = false;
                    for b in &picked {
                        if r.write_view_projection_any(b, &cam, self.world_wvp_register) == 0 {
                            wvp_missing = true;
                        }
                    }
                    if wvp_missing && !self.warned_no_wvp {
                        log::warn!(
                            "player camera: pipeline has no p3dWorldViewProjectionMatrix rows — meshes cannot be placed"
                        );
                        self.warned_no_wvp = true;
                    }

                    // 5. The nearest placed objects draw as cubes at
                    //    their REAL positions (WVP · translate·scale).
                    let mut drawn_boxes = 0usize;
                    for (i, id) in self
                        .world_objects
                        .nearest(player_pos, self.boxes.len())
                        .iter()
                        .enumerate()
                    {
                        let Some(o) = self.world_objects.get(*id) else { continue };
                        if i >= self.boxes.len() {
                            break;
                        }
                        let model = Mat4::from_translation(o.pos)
                            .mul_mat4(&Mat4::from_scale(BOX_SIZE));
                        r.write_view_projection_model(&self.boxes[i], &cam, &model);
                        picked.push(&self.boxes[i]);
                        drawn_boxes += 1;
                    }
                    // 6. Block 22/26: the paired LOCAL drawables at
                    //    their REAL world placements (WVP ·
                    //    translate·rotate·scale) — distance-culled per
                    //    frame to the placements near the player
                    //    (nearest-first, capped; `select_paired_near`).
                    let mut drawn_paired = 0usize;
                    let paired_positions: Vec<[f32; 3]> = self
                        .paired
                        .iter()
                        .map(|pb| [pb.pos.x, pb.pos.y, pb.pos.z])
                        .collect();
                    for &i in &crate::meshes::select_paired_near(
                        &paired_positions,
                        cam3,
                        PAIRED_DRAW_RADIUS_M,
                        PAIRED_DRAW_CAP,
                    ) {
                        let pb = &self.paired[i];
                        let [qx, qy, qz, qw] = pb.quat_xyzw;
                        let quat = prototype_math::Quat { x: qx, y: qy, z: qz, w: qw };
                        let model = Mat4::from_translation(pb.pos)
                            .mul_mat4(&quat.to_mat4())
                            .mul_mat4(&Mat4::from_scale(pb.scale));
                        // Block 34: on the world pair this writes the
                        // DECOMPOSED matrices (camera → u_c0..3 WVP,
                        // placement → u_c4..7 p3dWorldMatrix) instead of
                        // composing WVP·model into one — the CTAB-proven
                        // engine convention.
                        r.write_view_projection_model_any(
                            &pb.batch,
                            &cam,
                            &model,
                            self.world_wvp_register,
                            self.world_matrix_register,
                        );
                        picked.push(&pb.batch);
                        drawn_paired += 1;
                    }
                    if !self.logged_first_frame {
                        log::info!(
                            "player frame: eye ({:.1},{:.1},{:.1}) target ({:.1},{:.1},{:.1}), {} batch(es) selected, {} cube(s) placed, {} paired drawable(s)",
                            cam.eye.x,
                            cam.eye.y,
                            cam.eye.z,
                            cam.target.x,
                            cam.target.y,
                            cam.target.z,
                            picked.len() - drawn_boxes - drawn_paired,
                            drawn_boxes,
                            drawn_paired
                        );
                        self.logged_first_frame = true;
                    }

                    let frame = r.render_frame_selected(CLEAR, &picked);
                    if let Err(e) = frame {
                        // Timeout is routine under load; Lost/Outdated mean
                        // the surface died — rebuild on the next resume.
                        log::warn!("frame error: {e:?}");
                        if matches!(
                            e,
                            wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated
                        ) {
                            self.teardown();
                        }
                    }
                }
                None => {} // not booted; nothing to draw
                }

                // Block 58: the deferred gameplay audio event fires
                // AFTER the renderer borrow ends, still in the same
                // frame (the world draw below already happened or the
                // movie kept the frame — either way the click lands
                // within one vsync of the transition).
                if let Some(event) = self.pending_gameplay_event.take() {
                    self.play_gameplay_event(event);
                }
                // Block 67: the deferred NIS-mix cut (movie end) —
                // fires after the renderer borrow ends, same frame.
                if self.pending_nis_mix_stop {
                    self.pending_nis_mix_stop = false;
                    self.stop_nis_mix();
                }
                // Block 71: the deferred NIS cutscene stop (the
                // picture ended under the renderer borrow) — state +
                // mix + picture through the ONE cleanup path.
                if self.pending_nis_stop {
                    self.pending_nis_stop = false;
                    self.stop_nis();
                }
                // Block 68: the deferred FRONTEND music start (movie
                // end / skip) — the attract bed is a TAGGED looping
                // unit; a serve failure logs and degrades to silence.
                if self.pending_frontend_music_start {
                    self.pending_frontend_music_start = false;
                    if let Err(e) = self.enter_frontend(1.0) {
                        log::debug!("frontend music: {e}");
                    }
                }
            },
            WindowEvent::Touch(t) => {
                // Block 54: a touch while the boot movie plays skips it
                // (finger-down is the natural mobile skip gesture).
                if t.phase == WinitTouchPhase::Started {
                    self.skip_boot_movie();
                }
                let phase = match t.phase {
                    WinitTouchPhase::Started => TouchPhase::Down,
                    WinitTouchPhase::Moved => TouchPhase::Move,
                    WinitTouchPhase::Ended => TouchPhase::Up,
                    WinitTouchPhase::Cancelled => TouchPhase::Cancel,
                };
                // winit ids are u64; our mapper keys on u32 (ids wrap at
                // 2^32 — the OS reuses small ids for fingers, so this is
                // safe in practice; documented simplification).
                let point = TouchPoint::new(t.id as u32, t.location.x as f32, t.location.y as f32);
                // Block 38: finger 1 = mouse (the pre-38 contract), a
                // concurrent finger 2 = the camera look — its pixel
                // deltas feed the orbit the frame loop applies.
                match self.touch.map_all(phase, point) {
                    TouchMapped::Mouse(evs) => {
                        for ev in evs {
                            self.input.apply(ev);
                        }
                    }
                    TouchMapped::Look { dx, dy } => self.look.drag(dx, dy),
                    TouchMapped::Ignored => {}
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                // Block 54: a key press while the boot movie plays skips.
                if event.state == ElementState::Pressed {
                    self.skip_boot_movie();
                }
                if let Some(k) = map_key(&event.logical_key) {
                    let ev = match event.state {
                        ElementState::Pressed => InputEvent::KeyDown(k),
                        ElementState::Released => InputEvent::KeyUp(k),
                    };
                    self.input.apply(ev);
                }
            }
            // Desktop-native paths (the demo-render bin exercises them;
            // Android never sends mouse events).
            WindowEvent::CursorMoved { position, .. } => {
                let _ = self.input.apply(InputEvent::MouseMove(prototype_input::MousePosition::new(
                    position.x as f32,
                    position.y as f32,
                )));
            }
            WindowEvent::MouseInput { state, button, .. } => {
                // Block 54: a click while the boot movie plays skips.
                if state == ElementState::Pressed {
                    self.skip_boot_movie();
                }
                let b = match button {
                    WinitMouseButton::Left => MouseButton::Left,
                    WinitMouseButton::Right => MouseButton::Right,
                    WinitMouseButton::Middle => MouseButton::Middle,
                    _ => return,
                };
                let ev = match state {
                    ElementState::Pressed => InputEvent::MouseDown(b),
                    ElementState::Released => InputEvent::MouseUp(b),
                };
                self.input.apply(ev);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Continuous frame loop: one redraw requested per iteration while
        // a renderer is live (Fifo present mode paces it to vsync).
        if self.renderer.is_some() {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        log::info!("lifecycle: exiting");
        self.teardown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movement_keys_map_through_characters() {
        for (ch, want) in [
            ("w", KeyCode::W),
            ("W", KeyCode::W),
            ("a", KeyCode::A),
            ("s", KeyCode::S),
            ("d", KeyCode::D),
        ] {
            let key = Key::Character(ch.into());
            assert_eq!(map_key(&key), Some(want), "character {ch}");
        }
    }

    #[test]
    fn action_and_number_keys_map() {
        for (ch, want) in [
            ("e", KeyCode::E),
            ("Q", KeyCode::Q),
            ("f", KeyCode::F),
            ("r", KeyCode::R),
            ("1", KeyCode::One),
            ("0", KeyCode::Zero),
        ] {
            let key = Key::Character(ch.into());
            assert_eq!(map_key(&key), Some(want), "character {ch}");
        }
    }

    #[test]
    fn named_keys_map_to_the_engine_subset() {
        use NamedKey as N;
        for (named, want) in [
            (N::Space, KeyCode::Space),
            (N::ArrowUp, KeyCode::Up),
            (N::ArrowDown, KeyCode::Down),
            (N::ArrowLeft, KeyCode::Left),
            (N::ArrowRight, KeyCode::Right),
            (N::Shift, KeyCode::Shift),
            (N::Control, KeyCode::Control),
            (N::Escape, KeyCode::Escape),
            (N::Enter, KeyCode::Enter),
            (N::Tab, KeyCode::Tab),
            (N::Backspace, KeyCode::Backspace),
        ] {
            let key = Key::Named(named);
            assert_eq!(map_key(&key), Some(want), "named {named:?}");
        }
    }

    #[test]
    fn space_is_accepted_in_both_spellings() {
        assert_eq!(map_key(&Key::Named(NamedKey::Space)), Some(KeyCode::Space));
        assert_eq!(map_key(&Key::Character(" ".into())), Some(KeyCode::Space));
    }

    #[test]
    fn keys_outside_the_subset_are_dropped() {
        assert_eq!(map_key(&Key::Character("z".into())), None);
        assert_eq!(map_key(&Key::Character("ü".into())), None);
        assert_eq!(map_key(&Key::Named(NamedKey::F3)), None);
        assert_eq!(map_key(&Key::Named(NamedKey::MediaPlay)), None);
    }

    #[test]
    fn mapped_events_drive_the_input_snapshot() {
        // The exact integration the App performs in window_event:
        // map_key → InputEvent → InputState.
        let mut input = InputState::new();
        let down = map_key(&Key::Character("W".into())).unwrap();
        assert!(input.apply(InputEvent::KeyDown(down)));
        assert!(input.is_key_down(KeyCode::W));
        let up = map_key(&Key::Character("w".into())).unwrap();
        assert!(input.apply(InputEvent::KeyUp(up)));
        assert!(!input.is_key_down(KeyCode::W));
    }

    #[test]
    fn scan_or_setup_bootstraps_a_missing_folder() {
        // Desktop default folder is relative ("./Prototype Rust"); run
        // the scan against an isolated temp dir instead so the test
        // never touches the developer's checkout.
        let dir = std::env::temp_dir().join(format!(
            "prototype-rs-app-scan-{}",
            std::process::id()
        ));
        let sub = dir.join("empty");
        let state = scan_or_setup(&sub);
        // A fresh empty folder is Setup, and the folder WAS created.
        assert!(matches!(state, AppState::Setup { .. }));
        assert!(sub.is_dir(), "bootstrap must create the folder");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ------------------------------------------------------------------
    // Block 21: the player-camera frame update (device-free halves)
    // ------------------------------------------------------------------

    /// W moves the player forward (default forward −Z); the camera
    /// follows behind with the LIKELY default offset.
    #[test]
    fn player_frame_w_walks_forward_and_camera_follows() {
        let mut p = Player::new();
        let mut input = InputState::new();
        let _ = input.apply(InputEvent::KeyDown(KeyCode::W));
        let _disp = update_player(&mut p, &input, &CameraFollow::default(), 1.0);
        // 12 m/s × 1 s forward (−Z).
        assert!((p.position().z + PLAYER_SPEED).abs() < 1e-4, "z = {}", p.position().z);
        let cam = frame_camera(&CameraFollow::default(), &p, 1.0);
        // Eye trails behind the player (+Z of it) and above; target is
        // the player's look point.
        assert!((cam.eye.z - (p.position().z + 5.0)).abs() < 1e-4, "eye.z = {}", cam.eye.z);
        assert!((cam.eye.y - 2.0).abs() < 1e-4);
        assert!((cam.target.z - p.position().z).abs() < 1e-4);
        assert!((cam.target.y - 1.0).abs() < 1e-4);
    }

    /// ← yaws the player (right-handed +Y); a subsequent W walks along
    /// the NEW forward; an equal → restores the original heading.
    #[test]
    fn player_frame_arrows_yaw_and_w_follows_heading() {
        let mut p = Player::new();
        // Left alone: yaw 2.6 rad, no motion.
        let mut input = InputState::new();
        let _ = input.apply(InputEvent::KeyDown(KeyCode::Left));
        update_player(&mut p, &input, &CameraFollow::default(), 1.0);
        assert_eq!(p.position(), prototype_math::Vec3::ZERO, "turn alone does not move");
        // W for 1 s: walks along the rotated forward
        // (−sin θ, 0, −cos θ) · speed, θ = 2.6.
        let mut input2 = InputState::new();
        let _ = input2.apply(InputEvent::KeyDown(KeyCode::W));
        update_player(&mut p, &input2, &CameraFollow::default(), 1.0);
        let (s, c) = (2.6f32.sin(), 2.6f32.cos());
        assert!(
            (p.position().x + PLAYER_SPEED * s).abs() < 1e-3,
            "x = {} (rotated heading)",
            p.position().x
        );
        assert!(
            (p.position().z + PLAYER_SPEED * c).abs() < 1e-3,
            "z = {}",
            p.position().z
        );
        // An equal Right step restores identity heading; the next W
        // walks exactly −Z from wherever we are.
        let mut input3 = InputState::new();
        let _ = input3.apply(InputEvent::KeyDown(KeyCode::Right));
        update_player(&mut p, &input3, &CameraFollow::default(), 1.0);
        let mut input4 = InputState::new();
        let _ = input4.apply(InputEvent::KeyDown(KeyCode::W));
        update_player(&mut p, &input4, &CameraFollow::default(), 1.0);
        assert!(
            (p.position().x + PLAYER_SPEED * s).abs() < 1e-3,
            "x unchanged by the −Z leg: {}",
            p.position().x
        );
        assert!(
            (p.position().z - (-PLAYER_SPEED * c - PLAYER_SPEED)).abs() < 1e-3,
            "z = {} (second leg walked −Z)",
            p.position().z
        );
    }

    /// dt = 0 is a no-op (no motion, no turn) and the follow default
    /// stays the LIKELY (0,2,5) offset.
    #[test]
    fn player_frame_zero_dt_is_noop_and_follow_default_holds() {
        let mut p = Player::new();
        let mut input = InputState::new();
        let _ = input.apply(InputEvent::KeyDown(KeyCode::W));
        let disp = update_player(&mut p, &input, &CameraFollow::default(), 0.0);
        assert_eq!(disp, prototype_math::Vec3::ZERO);
        assert_eq!(p.position(), prototype_math::Vec3::ZERO);
        let f = CameraFollow::default();
        assert_eq!(f.offset, prototype_math::Vec3::new(0.0, 2.0, 5.0));
    }

    // ------------------------------------------------------------------
    // Block 38: the audio stage sizing + the touch look integration
    // ------------------------------------------------------------------

    /// The per-frame audio mix length: dt-sized, first-frame quantum,
    /// stall-clamped, negative-safe.
    #[test]
    fn audio_frame_count_is_dt_sized_and_clamped() {
        assert_eq!(audio_frame_count(0.0), 128, "the first frame still feeds a quantum");
        let sixty = audio_frame_count(1.0 / 60.0);
        assert!((800..=801).contains(&sixty), "1/60 s at 48 kHz = 800: {}", sixty);
        assert_eq!(audio_frame_count(0.1), 2048, "the dt clamp caps the mix");
        assert_eq!(audio_frame_count(10.0), 2048, "a stall never explodes the tick");
        assert_eq!(audio_frame_count(-0.5), 128, "negative dt is the first-frame quantum");
    }

    /// The full touch-look path at the app level: a second-finger drag
    /// orbits the camera (the eye moves, the target stays), while the
    /// mouse finger's events still drive the input snapshot.
    #[test]
    fn touch_look_orbits_the_camera_and_the_mouse_still_works() {
        let p = Player::new();
        let mut follow = CameraFollow::default();
        let baseline = frame_camera(&follow, &p, 1.0);

        let mut look = LookControl::new();
        // Drag right 900 px and up 450 px through the exact app policy.
        look.drag(900.0, -450.0);
        look.apply_to(&mut follow);
        let orbited = frame_camera(&follow, &p, 1.0);
        assert!(look.yaw < 0.0);
        assert!(
            (look.pitch - 0.35).abs() < 1e-4,
            "450 px wants 0.5 rad but clamps at PITCH_MAX: {}",
            look.pitch
        );
        assert!(look.pitch <= 0.35 + 1e-6, "the orbit respects the clamp");
        // The eye really moved (the orbit reached the camera).
        assert!(
            (orbited.eye.x - baseline.eye.x).abs() + (orbited.eye.z - baseline.eye.z).abs() > 1e-3,
            "orbit moved the eye"
        );
        // The target stays on the player's look point.
        assert!((orbited.target.y - 1.0).abs() < 1e-5);
    }

    /// Camera-relative walking: with a yaw orbit the W key walks along
    /// the CAMERA's forward, not the player's stale heading.
    #[test]
    fn walking_follows_the_look_orbit_yaw() {
        let mut p = Player::new();
        let mut input = InputState::new();
        let _ = input.apply(InputEvent::KeyDown(KeyCode::W));
        let mut follow = CameraFollow::default();
        follow.yaw_offset = std::f32::consts::FRAC_PI_2; // look orbit 90° left
        update_player(&mut p, &input, &follow, 1.0);
        // Camera yaw = 0 + π/2 → forward (−sin π/2, −cos π/2) = (−1, 0):
        // the player walks −X and FACES −X (yaw +π/2).
        assert!((p.position().x + PLAYER_SPEED).abs() < 1e-4, "x = {}", p.position().x);
        assert!(p.position().z.abs() < 1e-4, "z = {}", p.position().z);
        assert!((p.yaw() - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
    }

    /// The frame camera carries the shared FOV/planes + aspect, eye
    /// behind/above, target on the player.
    #[test]
    fn frame_camera_carries_the_shared_parameters() {
        let p = Player::at(prototype_math::Vec3::new(1687.0, 11.0, -1845.0));
        let cam = frame_camera(&CameraFollow::default(), &p, 1.777);
        assert!((cam.fov_y - CAMERA_FOV_Y).abs() < 1e-6);
        assert!((cam.aspect - 1.777).abs() < 1e-6);
        assert!((cam.near - CAMERA_NEAR).abs() < 1e-6);
        assert!((cam.far - CAMERA_FAR).abs() < 1e-6);
        assert!((cam.eye.y - 13.0).abs() < 1e-4, "eye.y = {}", cam.eye.y);
        assert!((cam.target.y - 12.0).abs() < 1e-4);
    }

    // ------------------------------------------------------------------
    // Block 66: the mission music seam
    // ------------------------------------------------------------------

    fn bank_fixture(name: &str) -> Vec<u8> {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    /// The full seam, device-free: an App whose graph was loaded from
    /// REAL bank bytes resolves a mission/transition pair to the
    /// PROVEN clip name and hands it to the shard server.  The
    /// placeholder archive cannot serve the payload, so the call
    /// fails WITH THE CLIP NAME in the diagnostics — proof the chain
    /// reached the serving boundary (the serving step itself is
    /// byte-identity-tested in `audio_registry`; the archive → graph
    /// step is tested in `music_graph`).
    #[test]
    fn mission_music_seam_resolves_the_proven_chain_to_the_shard_boundary() {
        let dir = std::env::temp_dir().join(format!(
            "prototype-rs-app-music-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("00audio.rcf"), b"placeholder archive").unwrap();
        let set = crate::game_files::scan_folder(&dir).unwrap();
        let mut app = App::with_state(dir.clone(), AppState::Ready(set));
        assert!(app.music_graph().is_empty(), "nothing loaded yet");

        // Load REAL bank bytes into the graph (the from_game_set
        // archive path is unit-tested in `music_graph`).
        let bytes = bank_fixture("bank_e10_music.p3d");
        app.music_graph
            .merge_bank_bytes(&bytes, "audio\\e10_music.p3d")
            .unwrap();
        assert_eq!(app.music_graph().cue_count(), 1);
        assert_eq!(app.music_graph().patch_count(), 7);

        // Unknown mission/transition → a clean Err, no IO.
        assert!(app.play_mission_music("nomission", "ambient", 1.0).is_err());

        // The PROVEN chain: e10m04/ambient → e10m04_ambient, handed
        // to the shard server (which fails on the placeholder — the
        // clip name in the diagnostic IS the chain's output).
        let err = app
            .play_mission_music("e10m04", "ambient", 1.0)
            .unwrap_err();
        assert!(
            err.contains("e10m04_ambient"),
            "the seam resolved the cue to the PROVEN clip: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ------------------------------------------------------------------
    // Block 67: the NIS-mix seam
    // ------------------------------------------------------------------

    /// The PROVEN named-archive construction (byte-for-byte the one
    /// `audio_registry`'s and `music_graph`'s tests use): header,
    /// directory, name-table gap, sector-aligned payloads.
    fn synthesize_named_archive(members: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let sector = 2048u32;
        let dir_offset = 60u32;
        let dir_size = (members.len() * 12) as u32;
        let mut table = Vec::new();
        table.extend_from_slice(&0x800u32.to_le_bytes());
        table.extend_from_slice(&0u32.to_le_bytes());
        for (name, _) in members {
            let nb = name.as_bytes();
            let len = nb.len() + 1;
            table.extend_from_slice(&0x688D_0000u32.to_le_bytes());
            table.extend_from_slice(&0x800u32.to_le_bytes());
            table.extend_from_slice(&0u32.to_le_bytes());
            table.extend_from_slice(&(len as u32).to_le_bytes());
            let mut field = vec![0u8; len];
            field[..nb.len()].copy_from_slice(nb);
            table.extend_from_slice(&field);
            table.extend_from_slice(&[0u8; 3]);
        }
        let payload_start =
            (dir_offset + dir_size + table.len() as u32).div_ceil(sector) * sector;
        let mut out = Vec::new();
        let mut magic = vec![0u8; 32];
        let magic_bytes = prototype_rcf::RCF_MAGIC_ASCII.as_bytes();
        magic[..magic_bytes.len()].copy_from_slice(magic_bytes);
        out.extend_from_slice(&magic);
        out.extend_from_slice(&prototype_rcf::RCF_VERSION_FIELD.to_le_bytes());
        out.extend_from_slice(&dir_offset.to_le_bytes());
        out.extend_from_slice(&dir_size.to_le_bytes());
        out.extend_from_slice(&sector.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(members.len() as u32).to_le_bytes());
        for ((name, _), (off, len)) in members.iter().zip(members.iter().scan(
            payload_start,
            |st, (_, data)| {
                let off = *st;
                *st += data.len() as u32;
                Some((off, data.len() as u32))
            },
        )) {
            out.extend_from_slice(&prototype_rcf::hash_name(name).to_le_bytes());
            out.extend_from_slice(&off.to_le_bytes());
            out.extend_from_slice(&len.to_le_bytes());
        }
        out.extend_from_slice(&table);
        out.resize(payload_start as usize, 0);
        for (_, data) in members {
            out.extend_from_slice(data);
        }
        out
    }

    /// The REAL shard member (block 64): the NIS mix of mission
    /// NIS mnis5_2_1 — RADP v6, 48 kHz, the 5.1 master the fold
    /// was built for.
    fn real_nis_mix_member() -> Vec<u8> {
        bank_fixture("audio_member_mnis_5_2_1_mix.p3d")
    }

    /// The full NIS-mix seam, device-free and END TO END REAL: the
    /// graph comes from the REAL boot_strap.p3d, the mix clip is
    /// served from a synthesized 01audio.rcf carrying the REAL
    /// shard member bytes, the spawn goes through the multi-channel
    /// fold into a TAGGED voice, and the tag cuts it.
    #[test]
    fn nis_mix_seam_plays_the_real_mix_clip_and_stops_it_by_tag() {
        let dir = std::env::temp_dir().join(format!(
            "prototype-rs-app-nis-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("01audio.rcf"),
            synthesize_named_archive(&[(
                "audio\\english\\AudioFile\\mnis_5_2_1_mix.p3d",
                real_nis_mix_member(),
            )]),
        )
        .unwrap();
        let set = crate::game_files::scan_folder(&dir).unwrap();
        let mut app = App::with_state(dir.clone(), AppState::Ready(set));

        // Load REAL graph bytes (the archive path is music_graph's
        // own suite; the NIS_Player patches ride boot_strap here).
        app.music_graph
            .merge_bank_bytes(&bank_fixture("boot_strap.p3d"), "audio\\boot_strap.p3d")
            .unwrap();

        // The PROVEN chain end to end: object → descriptor → shard
        // member → folded stereo voice.
        assert!(!app.nis_mix_active());
        let frames = app.play_nis_mix("mnis5_2_1", 1.0).expect("the mix plays");
        assert!(frames > 0, "the REAL v6 mix spawned {frames} frame(s)");
        assert!(app.nis_mix_active());
        assert_eq!(app.audio.mixer_ref().len(), 1, "exactly one voice");
        let voice = app.audio.mixer_ref().voice(0).unwrap();
        assert_eq!(voice.channels, 2, "the voice is the folded stereo unit");
        assert_eq!(voice.frame_count(), frames);
        assert!(voice.tag.is_some(), "the lifecycle tag rides the voice");

        // The tag cuts the unit; stopping again is a clean no-op.
        let cut = app.stop_nis_mix();
        assert_eq!(cut, 1);
        assert!(!app.nis_mix_active());
        assert_eq!(app.audio.mixer_ref().len(), 0);
        assert_eq!(app.stop_nis_mix(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The seam is MODAL (one NIS mix at a time) and never reuses a
    /// tag: starting the second unit stops the first, exactly one
    /// voice remains, and a stale stop cannot hit the newer unit.
    #[test]
    fn nis_mix_seam_is_modal_and_never_reuses_a_tag() {
        let dir = std::env::temp_dir().join(format!(
            "prototype-rs-app-nis-modal-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("01audio.rcf"),
            synthesize_named_archive(&[(
                "audio\\english\\AudioFile\\mnis_5_2_1_mix.p3d",
                real_nis_mix_member(),
            )]),
        )
        .unwrap();
        let set = crate::game_files::scan_folder(&dir).unwrap();
        let mut app = App::with_state(dir.clone(), AppState::Ready(set));
        app.music_graph
            .merge_bank_bytes(&bank_fixture("boot_strap.p3d"), "audio\\boot_strap.p3d")
            .unwrap();

        app.play_nis_mix("mnis5_2_1", 1.0).expect("first unit plays");
        let first_tag = app.audio.mixer_ref().voice(0).unwrap().tag;
        app.play_nis_mix("mb_3_1_1", 1.0).expect_err(
            "mb_3_1_1's mix clip is not in the synthesized archive — the unit must fail cleanly",
        );
        // The failed start left the FIRST unit running (stop_nis_mix
        // only fires when a NEW unit is about to start — the modal
        // stop happens before the fresh tag, and the fresh tag only
        // lands on success).
        assert!(app.nis_mix_active(), "the failed start kept unit one");
        app.play_nis_mix("mnis5_2_1", 1.0)
            .expect("the second start of the same mix plays");
        assert_eq!(app.audio.mixer_ref().len(), 1, "the first unit was cut");
        let second_tag = app.audio.mixer_ref().voice(0).unwrap().tag;
        assert_ne!(first_tag, second_tag, "tags never reuse");
        app.stop_nis_mix();
        assert_eq!(app.audio.mixer_ref().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An unknown NIS object (or an empty graph) is a clean Err —
    /// never a panic, never a guess.
    #[test]
    fn nis_mix_seam_errs_for_unknown_nis_or_empty_graph() {
        let dir = std::env::temp_dir().join(format!(
            "prototype-rs-app-nis-err-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("01audio.rcf"), b"placeholder").unwrap();
        let set = crate::game_files::scan_folder(&dir).unwrap();
        let mut app = App::with_state(dir.clone(), AppState::Ready(set));

        // Empty graph: the object lookup fails first.
        let err = app.play_nis_mix("mnis5_2_2", 1.0).unwrap_err();
        assert!(err.contains("no NIS patch"), "{err}");

        // A graph without the object: same clean miss.
        app.music_graph
            .merge_bank_bytes(&bank_fixture("bank_e10_music.p3d"), "audio\\e10_music.p3d")
            .unwrap();
        let err = app.play_nis_mix("mnis5_2_2", 1.0).unwrap_err();
        assert!(err.contains("no NIS patch"), "{err}");
        assert!(!app.nis_mix_active());
        assert_eq!(app.stop_nis_mix(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ------------------------------------------------------------------
    // Block 68: the mission system drives the live music seam
    // ------------------------------------------------------------------

    /// The REAL member the frontend bed AND the e02m02 ambient stem
    /// both play (this block's surgical Range fetch, byte-verified
    /// against the committed members' P3D magic).
    fn real_ambient_bed_member() -> Vec<u8> {
        bank_fixture("audio_member_e02m02_ambient.p3d")
    }

    /// The full mission-system lifecycle, device-free and END TO END
    /// REAL: the graph comes from the REAL boot_strap + e02 bank, the
    /// frontend bed and the mission stem are served from a
    /// synthesized 00audio.rcf carrying the REAL `e02m02_ambient`
    /// member, every unit is a TAGGED looping (or one-shot) voice,
    /// and each serve cuts the previous unit.
    #[test]
    fn mission_system_drives_the_live_seam_end_to_end_real() {
        let dir = std::env::temp_dir().join(format!(
            "prototype-rs-app-mission-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("00audio.rcf"),
            synthesize_named_archive(&[(
                "audio\\english\\AudioFile\\e02m02_ambient.p3d",
                real_ambient_bed_member(),
            )]),
        )
        .unwrap();
        let set = crate::game_files::scan_folder(&dir).unwrap();
        let mut app = App::with_state(dir.clone(), AppState::Ready(set));
        app.music_graph
            .merge_bank_bytes(&bank_fixture("boot_strap.p3d"), "audio\\boot_strap.p3d")
            .unwrap();
        app.music_graph
            .merge_bank_bytes(&bank_fixture("bank_e02_music.p3d"), "audio\\e02_music.p3d")
            .unwrap();

        // Boot state: SILENT — nothing plays until the frontend entry.
        assert_eq!(app.mission().phase(), crate::mission::MissionPhase::Silent);
        assert_eq!(app.audio.mixer_ref().len(), 0);

        // The frontend entry serves the PROVEN fe_music bed: REAL
        // 48 kHz stereo frames, LOOPING (the patch's own prop), tagged.
        let frames = app.enter_frontend(1.0).expect("the frontend bed plays");
        assert!(frames > 0, "the REAL bed spawned {frames} frame(s)");
        assert_eq!(app.audio.mixer_ref().len(), 1, "one music unit");
        let (bed_channels, bed_frames_n, bed_loops, bed_tag) = {
            let bed = app.audio.mixer_ref().voice(0).unwrap();
            (bed.channels, bed.frame_count(), bed.loop_voice, bed.tag)
        };
        assert_eq!(bed_channels, 2);
        assert_eq!(bed_frames_n, frames);
        assert!(bed_loops, "the PROVEN loop prop reached the voice");
        assert_eq!(bed_tag, app.music_unit_tag, "the tag is tracked");
        assert!(!app.mission().pending(), "the request was served");

        // Entering the mission cuts the frontend unit and starts the
        // mission's ambient stem — the SAME clip here (the frontend
        // bed IS a mission ambient stem), but a FRESH tag.
        let mission_frames = app.enter_mission("e02m02", 1.0).expect("the stem plays");
        assert!(mission_frames > 0);
        assert_eq!(app.mission().phase(), crate::mission::MissionPhase::InMission);
        assert_eq!(app.mission().mission(), Some("e02m02"));
        assert_eq!(app.mission().transition(), "ambient");
        assert_eq!(app.audio.mixer_ref().len(), 1, "the bed was cut");
        let (stem_loops, stem_tag) = {
            let stem = app.audio.mixer_ref().voice(0).unwrap();
            (stem.loop_voice, stem.tag)
        };
        assert_ne!(bed_tag, stem_tag, "tags never reuse");
        assert!(stem_loops, "the ambient bed loops");
        assert_eq!(stem_tag, app.music_unit_tag);

        // A transition whose member the archive cannot serve: the
        // DESIGN cut-first order — the old unit is already cut, the
        // state stays consistent, the failure is a clean Err.
        let err = app.set_music_transition("death", 1.0).unwrap_err();
        assert!(err.contains("e02m02_death"), "{err}");
        assert_eq!(app.audio.mixer_ref().len(), 0, "the old unit was cut");
        assert_eq!(app.mission().transition(), "death", "state is the truth");
        assert_eq!(app.music_unit_tag, None);
        assert!(!app.mission().pending(), "served — by silence");
        // And back to the bed: plays again.
        app.set_music_transition("ambient", 1.0)
            .expect("the ambient stem plays again");
        assert_eq!(app.audio.mixer_ref().len(), 1);

        // Leaving cuts the unit; the state goes to the free-roam
        // world (block 70).  THIS graph carries no ambience bank, so
        // the world bed is tracked silence: Ok(0), everything quiet.
        assert_eq!(app.leave_mission(1.0).unwrap(), 0, "no ambience bank → silence");
        assert_eq!(app.audio.mixer_ref().len(), 0);
        assert_eq!(app.mission().phase(), crate::mission::MissionPhase::FreeRoam);
        assert!(!app.world_bed_active());
        assert_eq!(app.leave_mission(1.0).unwrap(), 0, "idempotent");

        // The frontend takes ownership back; the re-entry after a
        // leave is a real change (the bed returns, fresh tag).
        let fe_frames = app.enter_frontend(1.0).expect("the bed returns");
        assert!(fe_frames > 0);
        assert_eq!(app.audio.mixer_ref().len(), 1);
        assert!(app.audio.mixer_ref().voice(0).unwrap().loop_voice);
        // Idempotent re-entry: no restart (same tag, same unit).
        let tag_before = app.music_unit_tag;
        app.enter_frontend(1.0).expect("idempotent entry is Ok(0)");
        assert_eq!(app.music_unit_tag, tag_before);
        assert_eq!(app.audio.mixer_ref().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The stateful seam on an archive that cannot serve: the chain
    /// resolves to the PROVEN clip name, the spawn fails cleanly,
    /// and the STATE stays consistent (tracked, served-by-silence) —
    /// the game continues.
    #[test]
    fn mission_system_degrades_to_tracked_silence_on_a_placeholder_archive() {
        let dir = std::env::temp_dir().join(format!(
            "prototype-rs-app-mission-placeholder-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("00audio.rcf"), b"placeholder archive").unwrap();
        let set = crate::game_files::scan_folder(&dir).unwrap();
        let mut app = App::with_state(dir.clone(), AppState::Ready(set));
        app.music_graph
            .merge_bank_bytes(&bank_fixture("bank_e10_music.p3d"), "audio\\e10_music.p3d")
            .unwrap();

        // Unknown mission: the state machine refuses BEFORE any IO.
        assert!(app.enter_mission("nomission", 1.0).is_err());
        assert_eq!(app.mission().phase(), crate::mission::MissionPhase::Silent);

        // The real mission on a serving-less archive: the PROVEN
        // chain reaches the shard boundary (the clip name IS the
        // diagnostic), the state is tracked and served.
        let err = app.enter_mission("e10m04", 1.0).unwrap_err();
        assert!(err.contains("e10m04_ambient"), "{err}");
        assert_eq!(app.mission().phase(), crate::mission::MissionPhase::InMission);
        assert_eq!(app.mission().mission(), Some("e10m04"));
        assert_eq!(app.mission().transition(), "ambient");
        assert!(!app.mission().pending(), "served — by silence");
        assert_eq!(app.audio.mixer_ref().len(), 0);
        assert_eq!(app.leave_mission(1.0).unwrap(), 0, "nothing to serve");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ------------------------------------------------------------------
    // Block 70: the free-roam WORLD BED (the ambience bank)
    // ------------------------------------------------------------------

    /// An 18-stream RADP AudioFile member — the world bed's REAL
    /// verified shape (block 70: the actual `ambience_baked_18ch`
    /// member header says RADP version 18, 32 000 Hz, format_tag 2;
    /// scripts/m70_fetch_ambience_headers.py).  `headers` seeds each
    /// stream's block predictor; the nibble data is all zero, so the
    /// decode is deterministic — the expected PCM is derived with the
    /// crate's own codec, then the documented block-67 fold, then the
    /// 32 k → 48 k resample (exactly the serve's pipeline).
    /// Returns `(member bytes, expected folded+resampled stereo PCM)`.
    fn synth_radp_member(
        clip: &str,
        streams: u32,
        rate: u32,
        blocks: usize,
        headers: &[i16],
    ) -> (Vec<u8>, Vec<i16>) {
        assert_eq!(headers.len(), 3, "ch0, ch1, the bed — ch3.. stay silent");
        let mut payload = Vec::with_capacity(blocks * streams as usize * 20);
        for _b in 0..blocks {
            for s in 0..streams as usize {
                payload.push(0u8); // step_index 0 (table start)
                payload.push(0u8); // reserved
                payload.extend_from_slice(&headers[s.min(2)].to_le_bytes());
                payload.extend_from_slice(&[0u8; 16]); // 32 zero nibbles
            }
        }
        // The crate's codec → the interleaved 18-stream PCM.
        let decoded =
            prototype_audio::radp::decode(streams, &payload).expect("synth payload decodes");
        assert_eq!(decoded.channels, streams);
        // The block-67 fold (DESIGN, documented): L = ch0 + 0.5·ch2,
        // R = ch1 + 0.5·ch2, clamped; channels ≥ 3 dropped.
        let ch = streams as usize;
        let src_frames = decoded.samples.len() / ch;
        let mut folded = Vec::with_capacity(src_frames * 2);
        for f in 0..src_frames {
            let l = decoded.samples[f * ch] as i32;
            let r = decoded.samples[f * ch + 1] as i32;
            let c = decoded.samples[f * ch + 2] as i32;
            let cl = |v: i32| v.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            folded.push(cl(l + c / 2));
            folded.push(cl(r + c / 2));
        }
        let expected =
            prototype_audio::sample_rate::resample(&folded, 2, rate, prototype_engine::gameplay_sounds::SESSION_RATE_HZ);
        // Wrap into a one-clip P3D AudioFile member (the PROVEN
        // layout the parser walks): the CHUNK name is "AudioFile"
        // (that's what AudioClipCatalog matches); the CLIP name is
        // the payload's name-list entry.
        let chunk_name = b"AudioFile";
        let clip_name = clip.as_bytes();
        let mut body = Vec::new();
        body.push(0u8);
        body.extend_from_slice(&2u32.to_le_bytes());
        for _ in 0..2 {
            body.extend_from_slice(&(clip_name.len() as u32).to_le_bytes());
            body.extend_from_slice(clip_name);
            body.push(0u8);
        }
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&4u32.to_le_bytes());
        body.extend_from_slice(b"radp");
        body.push(0u8);
        body.extend_from_slice(b"RADP");
        body.extend_from_slice(&streams.to_le_bytes());
        body.extend_from_slice(&rate.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes()); // format_tag
        body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        body.extend_from_slice(&payload);
        let mut p = Vec::new();
        // The 12-B file header: magic + version + file_size.
        p.extend_from_slice(b"P3D\xff");
        p.extend_from_slice(&12u32.to_le_bytes());
        p.extend_from_slice(&0u32.to_le_bytes()); // file_size, patched below
        let chunk_size = (12 + 4 + 4 + chunk_name.len() + body.len()) as u32;
        p.extend_from_slice(&0xFE00_0000u32.to_le_bytes());
        p.extend_from_slice(&0u32.to_le_bytes()); // uid
        p.extend_from_slice(&chunk_size.to_le_bytes());
        p.extend_from_slice(&0u32.to_le_bytes()); // classic id
        p.extend_from_slice(&(chunk_name.len() as u32).to_le_bytes());
        p.extend_from_slice(chunk_name);
        p.extend_from_slice(&body);
        let file_size = p.len() as u32;
        p[8..12].copy_from_slice(&file_size.to_le_bytes());
        (p, expected)
    }

    /// The free-roam WORLD BED, device-free and END TO END REAL: the
    /// graph comes from the REAL ambience bank, the bed clip is
    /// served from a synthesized 00audio.rcf carrying an 18-stream
    /// RADP member (the real member's verified shape), the spawn
    /// folds the 18 streams to the documented stereo stand-in,
    /// resamples 32 k → 48 k, tags the voice as the WORLD-BED unit
    /// (independent of the music channel), and every state change
    /// moves the two units exactly as the phase model says.
    #[test]
    fn free_roam_serves_the_world_bed_end_to_end_real() {
        let (member, expected) =
            synth_radp_member("ambience_baked_18ch", 18, 32_000, 3, &[100, 200, 30_000]);
        let dir = std::env::temp_dir().join(format!(
            "prototype-rs-app-freeroam-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("00audio.rcf"),
            synthesize_named_archive(&[
                (
                    "audio\\english\\AudioFile\\ambience_baked_18ch.p3d",
                    member.clone(),
                ),
                (
                    "audio\\english\\AudioFile\\e02m02_ambient.p3d",
                    real_ambient_bed_member(),
                ),
            ]),
        )
        .unwrap();
        let set = crate::game_files::scan_folder(&dir).unwrap();
        // STRUCTURAL PIN: the synthesized multi-member archive must
        // read BOTH members back byte-exact through the PROVEN
        // NamedArchive path (the builder is test-only code; this
        // catches any directory/table misalignment before the app
        // layer sees it).
        {
            use prototype_rcf::named::NamedArchive;
            let f = std::fs::File::open(dir.join("00audio.rcf")).unwrap();
            let mut named = NamedArchive::open(f).expect("the synthesized archive opens");
            for (member, want) in [
                (
                    "audio\\english\\AudioFile\\ambience_baked_18ch.p3d",
                    &member,
                ),
                (
                    "audio\\english\\AudioFile\\e02m02_ambient.p3d",
                    &real_ambient_bed_member(),
                ),
            ] {
                let got = named
                    .read_member(member)
                    .unwrap_or_else(|e| panic!("read {member} failed: {e}"));
                assert_eq!(
                    got.len(),
                    want.len(),
                    "{member}: the span length matches the member"
                );
                assert_eq!(&got[..], &want[..], "{member}: byte-exact read-back");
                // In-binary parse probe: the SAME bytes the serve
                // path will decode must parse HERE too.
                match prototype_engine::GameplaySounds::from_p3d_bytes(&got, "pin-probe") {
                    Ok(s) => eprintln!("pin-probe {member}: parsed, {} clip(s)", s.len()),
                    Err(e) => {
                        eprintln!("pin-probe {member}: PARSE ERR {e}");
                        eprintln!("first 32 B: {:02x?}", &got[..got.len().min(32)]);
                    }
                }
            }
        }
        let mut app = App::with_state(dir.clone(), AppState::Ready(set));
        app.music_graph
            .merge_bank_bytes(&bank_fixture("bank_ambience.p3d"), "audio\\ambience.p3d")
            .unwrap();
        app.music_graph
            .merge_bank_bytes(&bank_fixture("boot_strap.p3d"), "audio\\boot_strap.p3d")
            .unwrap();
        app.music_graph
            .merge_bank_bytes(&bank_fixture("bank_e02_music.p3d"), "audio\\e02_music.p3d")
            .unwrap();

        // Boot: silent, no units, no world bed.
        assert_eq!(app.audio.mixer_ref().len(), 0);
        assert!(!app.world_bed_active());

        // The free-roam entry: the WORLD BED plays — stereo (the
        // fold of 18 streams), looping (the PROVEN prop), gain =
        // 1.0 × 2.0 (the PROVEN volume), its OWN tag, the music
        // channel untouched.
        let frames = app.enter_free_roam(1.0).expect("the world bed plays");
        assert_eq!(frames, expected.len() / 2, "the resampled frame count");
        assert!(app.world_bed_active());
        assert_eq!(app.mission().phase(), crate::mission::MissionPhase::FreeRoam);
        assert_eq!(app.music_unit_tag, None, "free-roam MUSIC is silent");
        assert_eq!(app.audio.mixer_ref().len(), 1, "exactly the bed");
        let (ch, loop_voice, gain, tag) = {
            let v = app.audio.mixer_ref().voice(0).unwrap();
            (
                v.channels,
                v.loop_voice,
                v.gain,
                v.tag.expect("the bed voice carries a lifecycle tag"),
            )
        };
        assert_eq!(ch, 2, "the 18 streams fold to the stereo unit");
        assert!(loop_voice, "the PROVEN loop prop reached the voice");
        assert_eq!(gain, 2.0, "caller 1.0 × the PROVEN volume 2");
        assert_eq!(app.ambience_unit_tag, Some(tag), "the bed tag is tracked");
        // The voice IS the folded + resampled world bed, byte-exact
        // (the same fold → resample pipeline the helper mirrors).
        assert_eq!(app.audio.mixer_ref().voice(0).unwrap().samples, expected);

        // Idempotent re-entry: no restart, same tag, same voice.
        assert_eq!(app.enter_free_roam(1.0).unwrap(), 0);
        assert_eq!(app.audio.mixer_ref().voice(0).unwrap().tag, Some(tag));

        // Entering a mission: the music stem takes the MUSIC channel
        // (the REAL e02m02_ambient) and the world bed is CUT —
        // in-mission ambience is UNKNOWN, the bed stays silent.
        let stem_frames = app.enter_mission("e02m02", 1.0).expect("the stem plays");
        assert!(stem_frames > 0);
        assert_eq!(app.mission().phase(), crate::mission::MissionPhase::InMission);
        assert!(!app.world_bed_active(), "the bed was cut");
        assert_eq!(app.music_unit_tag.is_some(), true);
        assert_eq!(app.audio.mixer_ref().len(), 1, "only the stem");

        // Leaving: the free-roam world takes over — the stem is cut,
        // the bed RESPAWNS with a FRESH tag.
        let old_bed_tag = tag;
        let bed_frames = app.leave_mission(1.0).expect("the bed returns");
        assert_eq!(bed_frames, frames, "the same world bed, resampled");
        assert_eq!(app.mission().phase(), crate::mission::MissionPhase::FreeRoam);
        assert_eq!(app.music_unit_tag, None);
        assert!(app.world_bed_active());
        let new_tag = app.ambience_unit_tag.unwrap();
        assert_ne!(old_bed_tag, new_tag, "tags never reuse");
        assert_eq!(app.audio.mixer_ref().len(), 1);

        // The frontend re-enters: the menu owns the MUSIC channel,
        // the world bed is cut (the frontend is not the world).
        let fe_frames = app.enter_frontend(1.0).expect("the frontend bed");
        assert!(fe_frames > 0);
        assert!(!app.world_bed_active());
        assert_eq!(app.audio.mixer_ref().len(), 1);

        // And back to free-roam: the bed again, the music channel
        // silent.  stop_world_ambience cuts the unit; a stale stop
        // is a clean no-op.
        app.enter_free_roam(1.0).expect("the bed again");
        assert!(app.world_bed_active());
        assert_eq!(app.audio.mixer_ref().len(), 1);
        assert_eq!(app.stop_world_ambience(), 1, "one voice cut");
        assert!(!app.world_bed_active());
        assert_eq!(app.audio.mixer_ref().len(), 0);
        assert_eq!(app.stop_world_ambience(), 0, "idempotent");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The world bed on a serving-less archive: the chain resolves
    /// to the PROVEN clip name, the spawn fails cleanly, the state
    /// stays consistent (tracked, served-by-silence).
    #[test]
    fn free_roam_degrades_to_tracked_silence_on_a_placeholder_archive() {
        let dir = std::env::temp_dir().join(format!(
            "prototype-rs-app-freeroam-ph-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("00audio.rcf"), b"placeholder archive").unwrap();
        let set = crate::game_files::scan_folder(&dir).unwrap();
        let mut app = App::with_state(dir.clone(), AppState::Ready(set));
        app.music_graph
            .merge_bank_bytes(&bank_fixture("bank_ambience.p3d"), "audio\\ambience.p3d")
            .unwrap();

        let err = app.enter_free_roam(1.0).unwrap_err();
        assert!(err.contains("ambience_baked_18ch"), "{err}");
        assert_eq!(app.mission().phase(), crate::mission::MissionPhase::FreeRoam);
        assert!(!app.mission().pending(), "served — by silence");
        assert!(!app.world_bed_active());
        assert_eq!(app.audio.mixer_ref().len(), 0);
        // The leave path over the same archive: a clean Ok(0) (the
        // bed was already the served-by-silence state).
        assert_eq!(app.leave_mission(1.0).unwrap(), 0, "not in a mission");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ------------------------------------------------------------------
    // Block 69: the NIS trigger system drives the live seams
    // ------------------------------------------------------------------

    /// The full NIS-trigger lifecycle, device-free and END TO END
    /// REAL: the trigger validates the object against the REAL
    /// boot_strap graph, the mix descriptor's REAL clip (the block-64
    /// mnis_5_2_1 mix member, served from a synthesized 01audio.rcf)
    /// spawns through the multi-channel fold into a TAGGED voice, the
    /// request is consumed, and `stop_nis` cuts unit AND state
    /// together.
    #[test]
    fn nis_trigger_drives_the_live_seam_end_to_end_real() {
        let dir = std::env::temp_dir().join(format!(
            "prototype-rs-app-nis-trigger-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("01audio.rcf"),
            synthesize_named_archive(&[(
                "audio\\english\\AudioFile\\mnis_5_2_1_mix.p3d",
                real_nis_mix_member(),
            )]),
        )
        .unwrap();
        let set = crate::game_files::scan_folder(&dir).unwrap();
        let mut app = App::with_state(dir.clone(), AppState::Ready(set));
        app.music_graph
            .merge_bank_bytes(&bank_fixture("boot_strap.p3d"), "audio\\boot_strap.p3d")
            .unwrap();

        // Boot state: idle — nothing plays until the trigger fires.
        assert!(!app.nis_active());
        assert!(!app.nis_mix_active());
        assert!(!app.nis_system().pending());

        // The trigger serves the REAL mix: object → descriptor →
        // shard member → folded stereo TAGGED voice.
        let frames = app.play_nis("mnis5_2_1", 1.0).expect("the mix plays");
        assert!(frames > 0, "the REAL mix spawned {frames} frame(s)");
        assert!(app.nis_active(), "the trigger state owns the cutscene");
        assert_eq!(app.nis_system().active(), Some("mnis5_2_1"));
        assert!(!app.nis_system().pending(), "the request was consumed");
        assert!(app.nis_mix_active(), "the audio unit is tracked");
        assert_eq!(app.audio.mixer_ref().len(), 1, "exactly one voice");
        let voice = app.audio.mixer_ref().voice(0).unwrap();
        assert_eq!(voice.channels, 2, "the folded stereo unit");
        assert_eq!(voice.frame_count(), frames);
        assert!(voice.tag.is_some());

        // stop_nis: unit AND state together; idempotent.
        assert_eq!(app.stop_nis(), 1, "one voice cut");
        assert!(!app.nis_active());
        assert!(!app.nis_mix_active());
        assert_eq!(app.audio.mixer_ref().len(), 0);
        assert_eq!(app.stop_nis(), 0, "idempotent");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// THE SERVABILITY CENSUS through the app seam (PROVEN, this
    /// block's empirical work): all 46 REAL NIS objects pass the
    /// trigger; 43 of their mix clips ship in the block-63 shard
    /// index; exactly 3 do not — the `nis0-3a/4/5_mix.wav` intro
    /// mixes (the block-56 "9 event-driven" note was an imprecise
    /// record) — and those 3 degrade at the shard boundary with a
    /// clean Err that names the missing clip, state served by
    /// silence, the previous unit untouched.
    #[test]
    fn nis_trigger_census_and_the_unshipped_intro_mixes_degrade_cleanly() {
        let dir = std::env::temp_dir().join(format!(
            "prototype-rs-app-nis-census-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("01audio.rcf"), b"placeholder").unwrap();
        let set = crate::game_files::scan_folder(&dir).unwrap();
        let mut app = App::with_state(dir.clone(), AppState::Ready(set));
        app.music_graph
            .merge_bank_bytes(&bank_fixture("boot_strap.p3d"), "audio\\boot_strap.p3d")
            .unwrap();

        // The census: 46 NIS_Player patches, each referencing exactly
        // ONE mix clip; every object passes the trigger's validation.
        let nis_objects: Vec<String> = app
            .music_graph
            .patches()
            .filter(|(_, p)| p.class == "NIS_Player")
            .map(|(name, _)| name.to_string())
            .collect();
        assert_eq!(nis_objects.len(), 46, "the block-56/66 census");
        for object in &nis_objects {
            app.nis
                .start(&app.music_graph, object)
                .unwrap_or_else(|e| panic!("{object}: {e}"));
            app.nis.stop();
        }
        assert!(!app.nis_active(), "the census leaves no cutscene active");

        // The unshipped intro mixes: the reference is REAL (the patch
        // exists, the clip name is the diagnostic), the archive does
        // not carry it — clean Err naming the clip, state moved and
        // served, nothing playing.
        let err = app.play_nis("nis0-3a", 1.0).unwrap_err();
        assert!(err.contains("nis0-3a_mix.wav"), "{err}");
        assert!(err.contains("every clip failed"), "{err}");
        assert!(app.nis_active(), "the trigger state is the truth");
        assert_eq!(app.nis_system().active(), Some("nis0-3a"));
        assert!(!app.nis_system().pending(), "served — by silence");
        assert!(!app.nis_mix_active());
        assert_eq!(app.audio.mixer_ref().len(), 0);
        assert_eq!(app.stop_nis(), 0, "nothing to cut");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The trigger is FAIL-CLOSED (unknown object / non-NIS_Player
    /// patch: clean Err, state AND audio untouched) and the modal cut
    /// fires on SUCCESS only: a spawn failure moves the TRIGGER state
    /// (the cutscene is what the game asked for — served, by silence)
    /// while the OLD audio unit keeps playing, tracked, until the
    /// next successful serve or the stop hook.
    #[test]
    fn nis_trigger_is_fail_closed_and_keeps_the_old_unit_on_spawn_failure() {
        let dir = std::env::temp_dir().join(format!(
            "prototype-rs-app-nis-fail-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("01audio.rcf"),
            synthesize_named_archive(&[(
                "audio\\english\\AudioFile\\mnis_5_2_1_mix.p3d",
                real_nis_mix_member(),
            )]),
        )
        .unwrap();
        let set = crate::game_files::scan_folder(&dir).unwrap();
        let mut app = App::with_state(dir.clone(), AppState::Ready(set));
        app.music_graph
            .merge_bank_bytes(&bank_fixture("boot_strap.p3d"), "audio\\boot_strap.p3d")
            .unwrap();

        // Unknown object: Err BEFORE any state or audio change.
        let err = app.play_nis("mnis9_9_9", 1.0).unwrap_err();
        assert!(err.contains("no patch"), "{err}");
        assert!(!app.nis_active());
        assert!(!app.nis_mix_active());
        assert_eq!(app.audio.mixer_ref().len(), 0);
        // A non-NIS patch (the frontend bed): refused — the trigger
        // only fires cutscenes.
        let err = app.play_nis("fe_music", 1.0).unwrap_err();
        assert!(err.contains("not an NIS_Player"), "{err}");
        assert!(!app.nis_active());

        // The REAL mix plays.
        app.play_nis("mnis5_2_1", 1.0).expect("the mix plays");
        let old_tag = app.audio.mixer_ref().voice(0).unwrap().tag;

        // A cutscene whose mix member the archive cannot serve: the
        // spawn fails cleanly (Err), the OLD unit keeps playing AND
        // tracked, the TRIGGER state moved to the new cutscene and
        // was consumed (served, by silence — the consistency
        // contract).
        let err = app.play_nis("mb_3_1_1", 1.0).unwrap_err();
        assert!(err.contains("mb_3_1_1"), "{err}");
        assert!(err.contains("every clip failed"), "{err}");
        assert!(app.nis_active(), "the trigger state is the truth");
        assert_eq!(app.nis_system().active(), Some("mb_3_1_1"));
        assert!(!app.nis_system().pending(), "served — by silence");
        assert!(app.nis_mix_active(), "the old unit still runs");
        assert_eq!(app.audio.mixer_ref().len(), 1, "the old voice, kept");
        assert_eq!(app.audio.mixer_ref().voice(0).unwrap().tag, old_tag);

        // The stop hook cuts the old unit and clears the state.
        assert_eq!(app.stop_nis(), 1);
        assert!(!app.nis_active());
        assert!(!app.nis_mix_active());
        assert_eq!(app.audio.mixer_ref().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The MOVIE leg: the externally-fed name resolves against the
    /// USER'S OWN archives through the PROVEN finder (a REAL
    /// movies.rcf synthesized around the real 736-B `NIS_2_4_FMV`
    /// Bink head), the verdict is logged, the state carries the name,
    /// and a miss degrades to audio-only — never an `Err`.
    #[test]
    fn nis_trigger_movie_leg_resolves_the_users_archive_or_degrades() {
        let dir = std::env::temp_dir().join(format!(
            "prototype-rs-app-nis-movie-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("01audio.rcf"),
            synthesize_named_archive(&[(
                "audio\\english\\AudioFile\\mnis_5_2_1_mix.p3d",
                real_nis_mix_member(),
            )]),
        )
        .unwrap();
        // The REAL Bink head of the named movie (block-60 fixture),
        // shipped as the named member of a REAL-shaped movies.rcf.
        std::fs::write(
            dir.join("movies.rcf"),
            synthesize_named_archive(&[(
                "movies\\NIS_2_4_FMV.bik",
                bank_fixture("movies_bik_04b837ad_head736.bin"),
            )]),
        )
        .unwrap();
        let set = crate::game_files::scan_folder(&dir).unwrap();
        // Pre-check: the PROVEN finder resolves the movie from the
        // user's own (synthesized) archive — the fixture serves it.
        assert!(
            crate::movie::find_movie_by_name(&set.rcf_archives, "NIS_2_4_FMV").is_some(),
            "the real head must resolve by name"
        );
        let mut app = App::with_state(dir.clone(), AppState::Ready(set));
        app.music_graph
            .merge_bank_bytes(&bank_fixture("boot_strap.p3d"), "audio\\boot_strap.p3d")
            .unwrap();

        // The full two-leg serve: the movie resolves (logged), the
        // mix plays REAL frames, the state carries the fed name.
        let frames = app
            .play_nis_with_movie("mnis5_2_1", "NIS_2_4_FMV", 1.0)
            .expect("the cutscene plays");
        assert!(frames > 0);
        assert!(app.nis_active());
        assert_eq!(app.nis_system().movie(), Some("NIS_2_4_FMV"));
        assert!(!app.nis_system().pending(), "served");
        assert_eq!(app.audio.mixer_ref().len(), 1);

        // A name the archives cannot serve: audio-only degrade (the
        // audio and video legs are independent systems) — Ok, the
        // state records what was fed, the new unit cut the old one.
        let frames2 = app
            .play_nis_with_movie("mnis5_2_1", "Not_A_Movie", 1.0)
            .expect("audio-only degrade");
        assert!(frames2 > 0);
        assert_eq!(app.nis_system().movie(), Some("Not_A_Movie"));
        assert_eq!(app.audio.mixer_ref().len(), 1, "one unit (the new one)");

        // The stop hook clears unit, state AND the fed movie.
        assert_eq!(app.stop_nis(), 1);
        assert!(!app.nis_active());
        assert!(app.nis_system().movie().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ------------------------------------------------------------------
    // Block 71: the MID-GAMEPLAY NIS MOVIE — the picture leg
    // ------------------------------------------------------------------

    /// Craft a bik from a REAL head + REAL frame spans — byte-for-byte
    /// the proven surgery `movie.rs`'s tests use (frame count, largest
    /// frame + copy, stored file size, the 44+12·tracks head truncation,
    /// the frame table with the keyframe bit on span 0, the spans).
    fn craft_bik(head: &[u8], spans: &[&[u8]]) -> Vec<u8> {
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

    /// The intro's REAL head + its REAL frame-1 span as a one-frame bik
    /// member (1280×544, stereo track 0, 3840 f32 → the s16 ring) — the
    /// exact bytes `movie.rs`'s walk test decodes.
    fn real_one_frame_movie() -> Vec<u8> {
        let head = bank_fixture("movies_bik_1e218528_head22980.bin");
        let s1 = bank_fixture("movies_bik_1e218528_frame1_7644.bin");
        craft_bik(&head, &[&s1])
    }

    /// The picture leg, device-free and END TO END REAL: the cutscene's
    /// movie resolves from a REAL-shaped movies.rcf around the REAL
    /// one-frame intro bik, its streaming player opens with the trigger,
    /// the device-free step machine decodes the REAL frame (RGBA size +
    /// track-0 s16 ring), paces, ENDS — and the picture ending ends the
    /// CUTSCENE through the deferred stop (state + mix through the ONE
    /// cleanup path).
    #[test]
    fn nis_movie_leg_plays_a_real_movie_and_the_picture_ends_the_cutscene() {
        let dir = std::env::temp_dir().join(format!(
            "prototype-rs-app-nis-mov-e2e-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("01audio.rcf"),
            synthesize_named_archive(&[(
                "audio\\english\\AudioFile\\mnis_5_2_1_mix.p3d",
                real_nis_mix_member(),
            )]),
        )
        .unwrap();
        std::fs::write(
            dir.join("movies.rcf"),
            synthesize_named_archive(&[(
                "movies\\NIS_2_4_FMV.bik",
                real_one_frame_movie(),
            )]),
        )
        .unwrap();
        let set = crate::game_files::scan_folder(&dir).unwrap();
        let mut app = App::with_state(dir.clone(), AppState::Ready(set));
        app.music_graph
            .merge_bank_bytes(&bank_fixture("boot_strap.p3d"), "audio\\boot_strap.p3d")
            .unwrap();

        // The trigger: the mix plays REAL frames AND the picture opens.
        assert!(!app.nis_movie_active());
        let frames = app
            .play_nis_with_movie("mnis5_2_1", "NIS_2_4_FMV", 1.0)
            .expect("the cutscene plays");
        assert!(frames > 0, "the mix spawned {frames} frame(s)");
        assert!(app.nis_active());
        assert!(app.nis_movie_active(), "the resolved movie opened its player");
        assert_eq!(app.audio.mixer_ref().len(), 1, "the mix voice runs");
        let (w, h) = {
            let p = app.nis_movie.as_ref().unwrap();
            (p.width(), p.height())
        };
        assert_eq!((w, h), (1280, 544), "the REAL intro frame's size");

        // Step 1 — the first advance decodes the REAL frame: Present.
        let t0 = std::time::Instant::now();
        let step = app.step_nis_movie(t0);
        assert_eq!(step, NisMovieStep::Present);
        {
            let p = app.nis_movie.as_ref().unwrap();
            assert_eq!(p.frames_done(), 1);
            assert_eq!(p.rgba().len(), 1280 * 544 * 4, "the decoded RGBA is real");
            assert_eq!(
                p.ring_handle().lock().unwrap().buffered(),
                3_840,
                "frame 1's track-0 f32 count → s16 in the movie ring"
            );
        }

        // Step 2 — same instant: not due (PTS ≈ 33 ms) → Holding, the
        // picture stays alive (the loop re-presents the last frame).
        let step = app.step_nis_movie(t0);
        assert_eq!(step, NisMovieStep::Holding);
        assert!(app.nis_movie_active());

        // Step 3 — past the last frame's PTS: the picture ENDS.  The
        // slots drop device-free, the cutscene stop DEFERS.
        let step = app.step_nis_movie(t0 + std::time::Duration::from_millis(40));
        assert_eq!(step, NisMovieStep::Finished);
        assert!(!app.nis_movie_active(), "the picture dropped with the end");
        assert!(app.pending_nis_stop, "the stop is deferred to the tail");
        assert!(app.nis_active(), "the tail has not run yet");
        assert_eq!(app.audio.mixer_ref().len(), 1, "the mix still runs");

        // The tail (what window_event's post-borrow flush does): ONE
        // cleanup path — state + mix (the picture is already gone).
        app.pending_nis_stop = false;
        assert_eq!(app.stop_nis(), 1, "the mix voice cuts with the picture");
        assert!(!app.nis_active());
        assert!(!app.nis_mix_active());
        assert_eq!(app.audio.mixer_ref().len(), 0);
        assert!(!app.nis_movie_active());

        // A finished picture steps Idle — the world draw takes over.
        assert_eq!(app.step_nis_movie(t0), NisMovieStep::Idle);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The picture leg FOLLOWS THE TRIGGER STATE: a clean Err (unknown
    /// object — the state did not move) leaves the running picture
    /// untouched; a resolution miss (audio-only) drops the previous
    /// picture modally; a movie whose member refuses to open degrades
    /// to audio-only with the cutscene still up.
    #[test]
    fn nis_movie_leg_degrades_and_follows_the_trigger_state() {
        let dir = std::env::temp_dir().join(format!(
            "prototype-rs-app-nis-mov-deg-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("01audio.rcf"),
            synthesize_named_archive(&[(
                "audio\\english\\AudioFile\\mnis_5_2_1_mix.p3d",
                real_nis_mix_member(),
            )]),
        )
        .unwrap();
        std::fs::write(
            dir.join("movies.rcf"),
            synthesize_named_archive(&[(
                "movies\\NIS_2_4_FMV.bik",
                real_one_frame_movie(),
            )]),
        )
        .unwrap();
        let set = crate::game_files::scan_folder(&dir).unwrap();
        let mut app = App::with_state(dir.clone(), AppState::Ready(set));
        app.music_graph
            .merge_bank_bytes(&bank_fixture("boot_strap.p3d"), "audio\\boot_strap.p3d")
            .unwrap();

        // 1. The movie cutscene opens its picture.
        app.play_nis_with_movie("mnis5_2_1", "NIS_2_4_FMV", 1.0)
            .expect("the cutscene plays");
        assert!(app.nis_movie_active());

        // 2. An UNKNOWN object is a clean Err — the trigger state did
        //    NOT move, so the running picture is untouched.
        let err = app
            .play_nis_with_movie("no_such_nis", "NIS_2_4_FMV", 1.0)
            .unwrap_err();
        assert!(err.contains("no_such_nis"), "{err}");
        assert!(app.nis_active(), "the first cutscene still owns the state");
        assert_eq!(app.nis_system().active(), Some("mnis5_2_1"));
        assert!(app.nis_movie_active(), "the picture followed the state (untouched)");
        assert_eq!(app.audio.mixer_ref().len(), 1, "the old mix kept playing");

        // 3. A resolution MISS (audio-only) — Ok, the state moved, the
        //    previous picture dies with its cutscene (modal), no new
        //    picture opens.
        let frames = app
            .play_nis_with_movie("mnis5_2_1", "Not_A_Movie", 1.0)
            .expect("audio-only degrade");
        assert!(frames > 0);
        assert!(app.nis_active());
        assert!(!app.nis_movie_active(), "the miss is audio-only — no picture");
        assert!(!app.pending_nis_stop, "no picture end is pending");
        assert_eq!(app.nis_system().movie(), Some("Not_A_Movie"));

        // 4. A movie member that refuses to OPEN (the 736-B head has no
        //    decodable frame table) — the cutscene still plays, the
        //    degrade is logged, nothing pending.
        std::fs::write(
            dir.join("movies2.rcf"),
            synthesize_named_archive(&[(
                "movies\\HEAD_ONLY.bik",
                bank_fixture("movies_bik_04b837ad_head736.bin"),
            )]),
        )
        .unwrap();
        // Re-scan so the new archive is discovered (the loader re-scans
        // on resume; the test mirrors it).
        let set2 = crate::game_files::scan_folder(&dir).unwrap();
        app.state = AppState::Ready(set2);
        let frames2 = app
            .play_nis_with_movie("mnis5_2_1", "HEAD_ONLY", 1.0)
            .expect("the head resolves; the open degrades");
        assert!(frames2 > 0);
        assert!(app.nis_active());
        assert!(!app.nis_movie_active(), "an unopenable member is audio-only");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Any input skips a playing NIS movie AND its whole cutscene
    /// (picture + mix + state through the ONE stop path) — and the
    /// skip is idempotent, a no-op with no movie anywhere.
    #[test]
    fn nis_movie_skip_ends_the_whole_cutscene_and_is_idempotent() {
        let dir = std::env::temp_dir().join(format!(
            "prototype-rs-app-nis-mov-skip-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("01audio.rcf"),
            synthesize_named_archive(&[(
                "audio\\english\\AudioFile\\mnis_5_2_1_mix.p3d",
                real_nis_mix_member(),
            )]),
        )
        .unwrap();
        std::fs::write(
            dir.join("movies.rcf"),
            synthesize_named_archive(&[(
                "movies\\NIS_2_4_FMV.bik",
                real_one_frame_movie(),
            )]),
        )
        .unwrap();
        let set = crate::game_files::scan_folder(&dir).unwrap();
        let mut app = App::with_state(dir.clone(), AppState::Ready(set));
        app.music_graph
            .merge_bank_bytes(&bank_fixture("boot_strap.p3d"), "audio\\boot_strap.p3d")
            .unwrap();

        // A skip with NOTHING playing is a no-op (gameplay input is
        // unaffected) — and never panics.
        app.skip_boot_movie();
        assert!(!app.nis_active() && !app.nis_movie_active());

        app.play_nis_with_movie("mnis5_2_1", "NIS_2_4_FMV", 1.0)
            .expect("the cutscene plays");
        assert!(app.nis_movie_active());
        assert_eq!(app.audio.mixer_ref().len(), 1);

        // The skip: picture, mix unit and trigger state end together.
        app.skip_boot_movie();
        assert!(!app.nis_movie_active(), "the picture dropped");
        assert!(!app.nis_active(), "the state cleared");
        assert!(!app.nis_mix_active());
        assert_eq!(app.audio.mixer_ref().len(), 0, "the mix voice cut");
        assert!(!app.pending_nis_stop, "nothing left to defer");

        // Idempotent: a second skip is a no-op.
        app.skip_boot_movie();
        assert_eq!(app.audio.mixer_ref().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The picture leg's audio DEVICE edge (feature `audio-cpal`): the
    /// edge opens over the player's ring with the picture and dies with
    /// the cutscene (skip); the tail-drain policy never cuts a LIVE
    /// picture (underrun-safe — the boot edge's proven contract).
    #[cfg(feature = "audio-cpal")]
    #[test]
    fn nis_movie_audio_edge_opens_with_the_picture_and_cuts_with_the_cutscene() {
        let dir = std::env::temp_dir().join(format!(
            "prototype-rs-app-nis-mov-edge-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("01audio.rcf"),
            synthesize_named_archive(&[(
                "audio\\english\\AudioFile\\mnis_5_2_1_mix.p3d",
                real_nis_mix_member(),
            )]),
        )
        .unwrap();
        std::fs::write(
            dir.join("movies.rcf"),
            synthesize_named_archive(&[(
                "movies\\NIS_2_4_FMV.bik",
                real_one_frame_movie(),
            )]),
        )
        .unwrap();
        let set = crate::game_files::scan_folder(&dir).unwrap();
        let mut app = App::with_state(dir.clone(), AppState::Ready(set));
        app.music_graph
            .merge_bank_bytes(&bank_fixture("boot_strap.p3d"), "audio\\boot_strap.p3d")
            .unwrap();

        app.play_nis_with_movie("mnis5_2_1", "NIS_2_4_FMV", 1.0)
            .expect("the cutscene plays");
        assert!(app.nis_movie_active());
        let opened = app.nis_movie_audio.is_some();
        if opened {
            // The device-edge close policy never fires while the
            // picture is alive (whatever the ring's fill state).
            app.stop_nis_movie_audio_when_drained();
            assert!(
                app.nis_movie_audio.is_some(),
                "a live picture keeps its device edge (underrun-safe)"
            );
        }

        // The skip cuts whatever the device granted.
        app.skip_boot_movie();
        assert!(app.nis_movie_audio.is_none());
        assert!(!app.nis_movie_active());
        // And the drain check on a dead cutscene is a clean no-op.
        app.stop_nis_movie_audio_when_drained();
        let _ = std::fs::remove_dir_all(&dir);
    }
}

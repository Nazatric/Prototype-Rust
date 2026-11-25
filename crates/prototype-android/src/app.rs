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
use std::sync::mpsc;
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
// Block 102: the platform UI services — storage grant flow, toasts and
// the Setup status colours (see the module's docs for the black-screen
// story that motivated it).
use crate::platform_ui::{status_clear_color, PlatformUi, StatusScreen, StorageState};
// Block 103: the boot journal + crash reporter (boot-log.txt in the
// asset folder — the ONE artifact a user without adb can share).
use crate::boot_log;
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
/// Block 108: the Android virtual move stick.  The left 45% of the
/// screen (in-world) is the walk zone; a finger drag there drives the
/// SAME [`InputState`] key set WASD does — the frame path's
/// [`update_player`] is untouched, and one code path walks on both
/// platforms.  Key reconcile happens per frame (KeyDown for newly
/// desired keys, KeyUp for released ones) against the shared InputState.
/// Android-only in practice (no touch, no stick) — documented honestly:
/// a physical key held while the stick releases the same code loses the
/// race; irrelevant on a phone.
struct VirtualStick {
    /// The capturing finger: (id, origin px, current px).
    active: Option<(u32, [f32; 2], [f32; 2])>,
    /// The normalized drag vector (-1..1; UP is forward).
    vector: [f32; 2],
    /// The key set currently held on the stick's behalf.
    held: std::collections::BTreeSet<prototype_input::KeyCode>,
}

impl VirtualStick {
    /// Stick travel radius in px (the drag distance that reads full).
    const RADIUS: f32 = 90.0;
    /// Below this magnitude the stick is neutral (thumb rest).
    const DEAD: f32 = 0.18;

    fn new() -> Self {
        Self { active: None, vector: [0.0, 0.0], held: Default::default() }
    }

    fn begin(&mut self, id: u32, x: f32, y: f32) {
        self.active = Some((id, [x, y], [x, y]));
    }

    fn move_to(&mut self, id: u32, x: f32, y: f32) {
        if let Some((fid, origin, cur)) = &mut self.active {
            if *fid == id {
                *cur = [x, y];
                let dx = (x - origin[0]) / Self::RADIUS;
                let dy = (y - origin[1]) / Self::RADIUS;
                let len = (dx * dx + dy * dy).sqrt();
                self.vector = if len > 1.0 { [dx / len, dy / len] } else { [dx, dy] };
            }
        }
    }

    fn end(&mut self, id: u32) {
        if let Some((fid, _, _)) = self.active {
            if fid == id {
                self.active = None;
                self.vector = [0.0, 0.0];
            }
        }
    }

    fn is_capturing(&self, id: u32) -> bool {
        self.active.as_ref().map(|(fid, _, _)| *fid) == Some(id)
    }

    /// The desired key set from the drag vector (with the dead zone).
    fn desired_keys(&self) -> std::collections::BTreeSet<prototype_input::KeyCode> {
        use prototype_input::KeyCode;
        let mut set = std::collections::BTreeSet::new();
        let [vx, vy] = self.vector;
        let len = (vx * vx + vy * vy).sqrt();
        if len >= Self::DEAD {
            if vy < -Self::DEAD {
                set.insert(KeyCode::W);
            }
            if vy > Self::DEAD {
                set.insert(KeyCode::S);
            }
            if vx < -Self::DEAD {
                set.insert(KeyCode::A);
            }
            if vx > Self::DEAD {
                set.insert(KeyCode::D);
            }
        }
        set
    }

    /// Per-frame reconcile against the shared InputState.
    fn apply(&mut self, input: &mut InputState) {
        let want = self.desired_keys();
        for k in want.difference(&self.held).copied().collect::<Vec<_>>() {
            input.apply(InputEvent::KeyDown(k));
        }
        for k in self.held.difference(&want).copied().collect::<Vec<_>>() {
            input.apply(InputEvent::KeyUp(k));
        }
        self.held = want;
    }
}

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

/// Block 74: the result of ONE
/// [`App::apply_audio_seam`] batch — the driver's audit, in the
/// fire-and-forget shape the engine's C++ callers used (a failed
/// request never aborts the batch; it lands in `failures` instead).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioSeamApply {
    /// Requests that drove a real serve path (including TRACKED
    /// SILENCE serves — `Ok(0)` is a served state, not a failure).
    pub applied: usize,
    /// Summed frames the successful serves spawned.
    pub frames: usize,
    /// `(native name, diagnostic)` per failed request, in order.
    pub failures: Vec<(&'static str, String)>,
}

/// The whole app state winit drives — shared by the Android activity
/// and the desktop `demo-render` bin.
pub struct App {
    /// The user's asset folder (fixed for the process lifetime).
    folder: PathBuf,
    /// File-discovery phase (re-scanned on every resume).
    state: AppState,
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
    /// Block 102: the platform UI (Android: JNI toasts + the storage
    /// grant flow; `None` on the desktop demo and in most tests).
    platform: Option<Box<dyn PlatformUi>>,
    /// Block 102: the Setup status surface is live — clear-only frames
    /// whose colour IS the message; torn down when the boot takes over.
    status_render: bool,
    /// Block 102: the last storage check result (drives the status
    /// colour; re-checked on every resume).
    storage: StorageState,
    /// Block 102: the last Setup toast text (dedupe — `resumed` fires on
    /// every return from Settings; the same message must not re-toast).
    last_setup_toast: Option<String>,
    /// Block 103: the boot phase machine (worker thread + chunked GPU
    /// stages).  The old synchronous `try_boot` inside `resumed` wedged
    /// the event loop past Android's ~10 s ANR limit and the OS killed
    /// the app mid-boot.
    boot_phase: BootPhase,
    /// Block 104: when the current boot attempt started — drives the
    /// pulsing boot tint (static grey reads as a hang) and the
    /// "booted in Ns" toast.  None = nothing in flight.
    boot_started: Option<Instant>,
    /// Block 107: the REAL menu system (screen + selection) — every
    /// non-world state renders through it; the flat debug tints are
    /// gone.
    menu: crate::menu::MenuState,
    /// Block 107: the menu font atlas (built with the renderer).
    atlas: Option<prototype_render::FontAtlas>,
    /// Block 107: the menu hit regions (rebuilt every menu frame;
    /// pointer input hit-tests the LAST frame's layout).
    menu_hits: crate::menu::HitRegions,
    /// Block 107: the pointer position (menu row hit-testing).
    cursor: [f32; 2],
    /// Block 107: the world is being PLAYED (menus hidden; the frame
    /// path streams + renders the world).
    in_world: bool,
    /// Block 107: persisted settings (streaming/audio/controls knobs
    /// wired to real engine behaviour).
    settings: crate::settings::Settings,
    /// Block 107: quit requested by the menu (executed in window_event).
    quit_requested: bool,
    /// Block 108: the Android virtual walk stick.
    stick: VirtualStick,
    /// Block 108: the surface size in px (the stick zone + UI layout).
    screen_px: [f32; 2],
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

// --- Block 103: the boot phase machine ----------------------------------
//
// The block-102 APK's boot ran synchronously inside `resumed`: on the
// user's phone the archive decompress passes held the event-loop thread
// past Android's ~10 s ANR limit and the OS killed the app mid-boot
// (black screen -> death ~10 s in — the 2026-11-24 report).  The boot is
// now split:
//
// * CPU stages (shader prep + the two archive decompress passes + the
//   movie scan) run on a WORKER THREAD — they touch no GPU/window — and
//   ship their results through an mpsc channel (panics are CAUGHT and
//   become a `BootPhase::Failed`, never a silent hang or a dead
//   process);
// * GPU stages run on the main thread, ONE stage (or a small chunk) per
//   loop iteration, so winit keeps pumping events between them;
// * the status surface stays up as the backdrop: the booting tint while
//   the machine runs, the failure red if it dies, and `boot-log.txt` in
//   the asset folder records every stage (the user has no adb).

/// GPU bring-up chunk size (block 103): each mesh upload can be tens
/// of MB — the chunk keeps every loop iteration far below any watchdog
/// threshold.  (The pipeline stage stays one tick: even ~50 WGSL
/// translations measure well under a second on a phone.)
const MESHES_PER_TICK: usize = 2;

/// The CPU worker's finished boot artifacts — everything the GPU stages
/// need, produced off the main thread (plain data, `Send`).
struct CpuBootArtifacts {
    /// The scanned file set (the member stream attaches over its
    /// archives on the main thread later).
    set: GameFileSet,
    /// The device-free boot report (shader pairs + problems).
    report: BootReport,
    /// The world pair's plan (cloned from the report).
    world_plan: Option<prototype_render::shader::world::WorldPairPlan>,
    /// The world scan (meshes + world/local split + provenance).
    scan: crate::meshes::MeshLoadReport,
    /// The world scan report (the real index + retained records).
    world: crate::meshes::WorldScanReport,
    /// The block-22 drawable pairing report.
    pair_rep: prototype_p3d::world::PairReport,
    /// The paired meshes loaded from those pairs.
    pm_rep: crate::meshes::PairedMeshReport,
    /// The boot movie candidate (if the user's archives carry one).
    movie: Option<crate::movie::BootMovie>,
}

/// The GPU bring-up stages (block 103) — one per loop iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GpuStage {
    /// Surface + device + the shader-pair pipelines.
    Pipelines,
    /// The placed-object spawn (once) + the paired-drawable batches.
    Paired,
    /// The world mesh batches (chunked).
    World,
    /// Stream attach + fallbacks + movie + frontend (one tick).
    Attach,
}

/// The boot phase machine (block 103 — see the module docs above).
enum BootPhase {
    /// Nothing in flight (cold start, or torn down with a suspend).
    Idle,
    /// The CPU worker is reading the archives (the booting tint shows).
    Cpu {
        /// The worker's result channel (one message, then it closes).
        rx: mpsc::Receiver<Result<CpuBootArtifacts, String>>,
    },
    /// GPU bring-up, one stage per loop iteration.
    Gpu {
        /// The CPU artifacts being consumed.
        art: CpuBootArtifacts,
        /// The stage the next `pump_boot` tick runs.
        stage: GpuStage,
        /// Next paired-mesh index (the chunk cursor).
        next_paired: usize,
        /// Next world-mesh index (the chunk cursor).
        next_world: usize,
        /// The block-21 spawn ran (once, before the paired chunks).
        spawned: bool,
        /// Block 20's batch->member provenance, accumulated across the
        /// world-stage ticks (aligned with `self.batches`).
        member_of_batch: Vec<Option<usize>>,
        /// Total world-space meshes built (the old boot's `built`).
        world_built: usize,
    },
    /// Boot complete — the world draw path owns the screen.
    Done,
    /// Boot failed (red status + toast; retried on the next resume).
    Failed(String),
}

/// The CPU boot worker body (block 103): device-free prep + the two
/// archive decompress passes + the movie scan.  Panics are CAUGHT and
/// reported through the channel — a worker death must surface as a
/// `Failed` phase, not a dead process or a silent hang.
/// Block 105: the CACHED + FILTERED + PARALLEL world index.
///
/// * FILTERED — audio/woi wave banks are pure sound: 85% of the
///   block-104 device pass's members came from them and none can hold
///   a placement.  They stay on disk for the audio paths (hash lookups
///   on demand); they leave the index pass.
/// * PARALLEL — `lean_index_archives` splits the remaining archives
///   across the CPU count, merged in archive order (identical to a
///   sequential scan).
/// * CACHED — `world-index.cache` next to the archives stores the
///   built index bound to a corpus fingerprint (file names + lengths +
///   scan options); boot 2+ reads it back in milliseconds.  Any
///   corpus/options change invalidates it and rebuilds honestly.
///
/// Still lean (block 104): NO mesh decode — the per-frame stream owns
/// geometry.  Returns (world index report, always-empty mesh report, a
/// one-line note for the boot log).
fn index_world_cached(
    set: &GameFileSet,
    opts: prototype_p3d::world::ScanOptions,
) -> (crate::meshes::WorldScanReport, crate::meshes::MeshLoadReport, String) {
    let index_paths: Vec<PathBuf> = set
        .rcf_archives
        .iter()
        .filter(|p| {
            match p.file_stem().map(|s| s.to_string_lossy().to_string()) {
                Some(stem) => {
                    let worthy = crate::game_files::index_worthy_archive(&stem);
                    if !worthy {
                        let line = format!(
                            "index: skip {} (sound bank — no world data)",
                            p.display()
                        );
                        log::info!("{line}");
                        crate::boot_log::bootlog(&line);
                    }
                    worthy
                }
                None => true,
            }
        })
        .cloned()
        .collect();
    // Fingerprint over the FULL archive list — skipped sound banks are
    // still corpus identity (swapping one must invalidate the cache).
    let lens: Vec<(String, u64)> = set
        .rcf_archives
        .iter()
        .map(|p| {
            (
                p.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                p.metadata().map(|m| m.len()).unwrap_or(0),
            )
        })
        .collect();
    boot_log::progress_plan(index_paths.len());
    let fp = prototype_p3d::index_fingerprint(&lens, opts);
    let cache_path = set
        .rcf_archives
        .first()
        .and_then(|p| p.parent())
        .map(|d| d.join("world-index.cache"));
    if let (Some(cache), Some(_)) = (&cache_path, index_paths.first()) {
        let started = std::time::Instant::now();
        if let Some(cached) = prototype_p3d::read_index_cache(cache, fp) {
            let mut scan = crate::meshes::WorldScanReport::default();
            scan.members_scanned = cached.member_count;
            scan.archives_opened = cached.archive_count;
            let line = format!(
                "index: CACHE HIT — {} member(s) in {:.2}s",
                cached.member_count,
                started.elapsed().as_secs_f32()
            );
            log::info!("{line}");
            crate::boot_log::bootlog(&line);
            return (scan_with_index(scan, cached.index), crate::meshes::MeshLoadReport::default(), "cache hit".to_string());
        }
    }
    if index_paths.is_empty() {
        return (
            crate::meshes::WorldScanReport::default(),
            crate::meshes::MeshLoadReport::default(),
            "no index-worthy archives".to_string(),
        );
    }
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);
    let started = std::time::Instant::now();
    let world = crate::meshes::lean_index_archives(&index_paths, opts, workers);
    let skipped = set.rcf_archives.len() - index_paths.len();
    let elapsed = started.elapsed().as_secs_f32();
    let mut note = format!(
        "built fresh on {workers} worker(s), {elapsed:.1}s, {} sound bank(s) skipped",
        skipped
    );
    if let Some(cache) = &cache_path {
        match prototype_p3d::write_index_cache(
            cache,
            fp,
            world.members_scanned,
            world.archives_opened,
            &world.index,
        ) {
            Ok(()) => {
                let bytes = std::fs::metadata(cache).map(|m| m.len()).unwrap_or(0);
                let line = format!(
                    "index: cache written ({} bytes) — boot 2+ skips this pass",
                    bytes
                );
                log::info!("{line}");
                crate::boot_log::bootlog(&line);
            }
            Err(e) => {
                note.push_str(&format!(", cache write FAILED (non-fatal): {e}"));
                log::warn!("index cache write failed (non-fatal): {e}");
            }
        }
    }
    (world, crate::meshes::MeshLoadReport::default(), note)
}

/// Swap the loaded index into a scan report without losing the honest
/// counters (cache-hit path).
fn scan_with_index(
    mut scan: crate::meshes::WorldScanReport,
    index: prototype_p3d::world::WorldIndex,
) -> crate::meshes::WorldScanReport {
    scan.index = index;
    scan
}

fn cpu_boot_work(set: GameFileSet, tx: mpsc::Sender<Result<CpuBootArtifacts, String>>) {    boot_log::stage("cpu: shader prep (prepare_programs)");
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let report = prepare_programs(&set);
        log::info!("{}", boot_summary(&report));
        for p in &report.problems {
            log::warn!("boot problem: {p}");
        }
        boot_log::bootlog(&format!(
            "shader prep: {} pair(s), {} problem(s)",
            report.pairs.len(),
            report.problems.len()
        ));
        let world_plan = report.world.clone();

        boot_log::stage("cpu: world archive index (LEAN, parallel, cached)");
        // Block 105: filtered (audio/woi wave banks out — they hold no
        // world data), parallel (one worker per archive chunk), and
        // cached (world-index.cache next to the archives — boot 2+
        // skips the pass entirely).  Still lean (block 104): NO mesh
        // decode — the per-frame streaming owns geometry.
        const INDEX_OPTS: prototype_p3d::world::ScanOptions =
            prototype_p3d::world::ScanOptions {
                retain_objects: true,
                // block 22: the pairing needs every shape name and the
                // retained node transforms
                retain_shape_names: true,
                retain_node_transforms: true,
            };
        let (world, _mesh, index_note) = index_world_cached(&set, INDEX_OPTS);
        boot_log::bootlog(&format!(
            "world index: {} archive(s), {} member(s), {} indexed, {} error(s) — {index_note}",
            world.archives_opened,
            world.members_scanned,
            world.index.entries.len(),
            world.errors,
        ));
        for e in &world.first_errors {
            log::warn!("mesh scan: {e}");
        }

        boot_log::stage("cpu: real menu-screen census");
        // Block 107/110: the REAL Scaleform menu screens live in the
        // user's corpus as art\hud\*.gfx members; the proven hash
        // locates them WITHOUT any decompress.  The journal line tells
        // the next session exactly which real screens we can extract.
        let (found, names) = crate::menu_assets::census(&set.rcf_archives);
        boot_log::bootlog(&format!(
            "menu census: {found}/25 real .gfx screen(s) present{}",
            if names.is_empty() {
                String::new()
            } else {
                format!(" — {}", names.join(", "))
            }
        ));

        boot_log::stage("cpu: pair drawables");
        let pair_rep = world.index.pair_drawables(PAIR_CAP);
        // Block 104: NO boot-time paired mesh decode — the stream's
        // paired half loads them per frame.  The report stays empty
        // (its fields default to 0 and the logs say so honestly).
        let pm_rep = crate::meshes::PairedMeshReport::default();
        boot_log::bootlog(&format!(
            "paired: {} pair(s) of {} drawable record(s) ({} prop-table skipped), {} mesh(es) from {} member(s), {} shape(s) missing",
            pair_rep.pairs.len(),
            pair_rep.records_with_drawable,
            pm_rep.skipped_prop_table,
            pm_rep.meshes.len(),
            pm_rep.members_decompressed,
            pm_rep.shapes_missing
        ));
        for e in &pm_rep.first_errors {
            log::warn!("paired drawables: {e}");
        }

        boot_log::stage("cpu: boot movie scan");
        let movie = find_boot_movie(&set.rcf_archives);

        CpuBootArtifacts {
            set,
            report,
            world_plan,
            scan: _mesh,
            world,
            pair_rep,
            pm_rep,
            movie,
        }
    }));
    match outcome {
        Ok(art) => {
            boot_log::progress_done();
            boot_log::bootlog("cpu worker: artifacts ready");
            let _ = tx.send(Ok(art));
        }
        Err(payload) => {
            let msg = boot_log::panic_message(&payload);
            boot_log::bootlog(&format!("cpu worker PANIC: {msg}"));
            let _ = tx.send(Err(format!("cpu boot panicked: {msg}")));
        }
    }
}

/// Toasts are two lines max — clip a boot-failure reason for one.
fn truncate_toast(msg: &str) -> &str {
    match msg.char_indices().nth(72) {
        Some((i, _)) => &msg[..i],
        None => msg,
    }
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
        // Block 112: the persisted settings load HERE (the tap-to-adjust
        // loop saves back into the same file).
        let settings = crate::settings::load(&folder);
        Self {
            state,
            folder,
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
            platform: None,
            status_render: false,
            storage: StorageState::Granted,
            last_setup_toast: None,
            boot_phase: BootPhase::Idle,
            boot_started: None,
            menu: crate::menu::MenuState::new(),
            atlas: None,
            menu_hits: crate::menu::HitRegions::default(),
            cursor: [0.0, 0.0],
            in_world: false,
            settings,
            quit_requested: false,
            stick: VirtualStick::new(),
            screen_px: [1280.0, 720.0],
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

    /// Block 72 — the `mixbuss` routing: the MIX BUS names for the
    /// real patch families.  The committed real banks carry a bus
    /// list on every patch under five prop names
    /// (`mixbuss`/`mixbus`/`bus`/`bus_name`/`busnames` — PROVEN;
    /// see [`crate::music_graph::bus_list`]); the same family uses
    /// the same SET in different orders (PROVEN rotation), and
    /// every list contains `master` (PROVEN).  The engine's own
    /// attachment rule and per-bus volumes are UNKNOWN (runtime
    /// state we do not have) — our DESIGN maps each family to ONE
    /// bus and carries the full label list in the descriptors:
    /// the music_player family (mission stems + frontend bed) →
    /// `music`; the Ambience_all world bed → `amb`; the NIS_Player
    /// mixes → `nis`.  The mixer's master gain is the `master`
    /// label's model.
    const MUSIC_BUS: &str = "music";
    const AMBIENCE_BUS: &str = "amb";
    const NIS_BUS: &str = "nis";

    /// Block 63: serve ONE clip by semantic name from the user's own
    /// audio archives through the embedded shard index (lazy: lookup
    /// → archive open → member span read → one-clip P3D decode →
    /// rate-clean spawn).  `Ok(None)` degrades to a clean Err so the
    /// caller's log reads uniformly; every hard failure carries the
    /// archive diagnostics.  Never panics.
    fn play_shard_clip(&mut self, clip: &str, gain: f32) -> Result<usize, String> {
        self.play_shard_clip_tagged(clip, gain, None, false, None)
    }

    /// [`Self::play_shard_clip`] with a mixer LIFECYCLE TAG (block
    /// 67), an explicit LOOP flag (block 68), and a MIX BUS (block
    /// 72): the spawned voice carries `tag` so the caller can stop
    /// the whole unit later by tag (`Mixer::stop_tagged`) —
    /// index-free, because mixer indices shift on every removal —
    /// repeats (`loop_voice`) until stopped, the music beds' PROVEN
    /// behavior, and rides `bus` through the mixbus layer (`None` =
    /// the DEFAULT bus, the historical single-bus shape).
    fn play_shard_clip_tagged(
        &mut self,
        clip: &str,
        gain: f32,
        tag: Option<u64>,
        loop_voice: bool,
        bus: Option<u16>,
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
        GameplaySounds::play_clip_tagged_loop_bus(
            &mut self.audio,
            &clip_data,
            gain,
            tag,
            loop_voice,
            bus,
        )
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
    /// descriptor, and the mix rides the `nis` bus (block 72) —
    /// the family's real set is `[nis, master, dialog]`.
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
        let nis_bus = self.audio.mixer().bus_id_or_add(Self::NIS_BUS);
        let mut frames = 0usize;
        let mut played = 0usize;
        let mut diags: Vec<String> = Vec::new();
        for clip in &mix.clips {
            match self.play_shard_clip_tagged(clip, effective, Some(tag), false, Some(nis_bus)) {
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
        let nis_bus = self.audio.mixer().bus_id_or_add(Self::NIS_BUS);
        let mut frames = 0usize;
        let mut played = 0usize;
        let mut diags: Vec<String> = Vec::new();
        for clip in &req.clips {
            match self.play_shard_clip_tagged(clip, effective, Some(tag), false, Some(nis_bus)) {
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

    /// Block 72: set one mix bus's gain (`0.0..=1.0+` linear) — the
    /// `mixbuss` routing's runtime seam (the options screen the
    /// original had; the real per-bus volumes are runtime state we
    /// do not have, so every bus starts at 1.0).  The bus is created
    /// if it does not exist yet, so a gain set BEFORE the first serve
    /// still applies when that serve creates its voices.  `false`
    /// only for the empty name (the default bus is gain-tracked via
    /// [`prototype_audio::mixer::Mixer::set_bus_gain`] directly).
    /// Recognised names today: `"music"`, `"amb"`, `"nis"`.
    pub fn set_audio_bus_gain(&mut self, bus_name: &str, gain: f32) -> bool {
        if bus_name.is_empty() {
            return false;
        }
        let id = self.audio.mixer().bus_id_or_add(bus_name);
        self.audio.mixer().set_bus_gain(id, gain)
    }

    /// Block 72: read one mix bus's current gain (`None` when the
    /// bus has never been created).
    pub fn audio_bus_gain(&self, bus_name: &str) -> Option<f32> {
        self.audio
            .mixer_ref()
            .bus_id(bus_name)
            .and_then(|id| self.audio.mixer_ref().bus_gain(id))
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
    /// Block 72: the music leg rides the `music` bus, the world-bed
    /// leg the `amb` bus (the real families' label sets, carried in
    /// the descriptors — see [`crate::music_graph::bus_list`]).
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
            let music_bus = self.audio.mixer().bus_id_or_add(Self::MUSIC_BUS);
            for clip in &req.clips {
                requested += 1;
                match self.play_shard_clip_tagged(clip, effective, Some(tag), req.loop_voice, Some(music_bus)) {
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
            let amb_bus = self.audio.mixer().bus_id_or_add(Self::AMBIENCE_BUS);
            for clip in &amb.clips {
                requested += 1;
                match self.play_shard_clip_tagged(clip, effective, Some(tag), amb.loop_voice, Some(amb_bus)) {
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

    // -----------------------------------------------------------------
    // Block 74: the AUDIO-SEAM DRIVER — the engine's snd natives
    // (`snd_SetMission` / `snd_SetFreeRoamMission`, the M7 registry
    // names, registered on the real script host) become REAL serve
    // legs here.  This is the ONE call-through from the engine's
    // native surface to the mixer: `snd_SetMission(m)` drives the
    // mission MUSIC leg (`enter_mission`), `snd_SetFreeRoamMission`
    // drives the WORLD-BED leg (`enter_free_roam`).  The block-72
    // bus routing applies unchanged because the driver goes through
    // the SAME serve paths (music → `music`, bed → `amb`), not
    // around them.
    // -----------------------------------------------------------------

    /// Apply the engine's typed audio-seam requests IN ORDER, each
    /// through the real serve path at `gain`.  Requests apply
    /// last-write-wins on the audible channel (each serve cuts the
    /// previous unit — the same contract the direct seams have); a
    /// failed request is CAPTURED in [`AudioSeamApply::failures`]
    /// and never aborts the batch (the engine's C++ drivers were
    /// fire-and-forget; we keep the audit).  An empty slice is a
    /// no-op.  Never panics.
    ///
    /// Honest scope (block 74): the SHIPPED corpus never produces
    /// these requests (PROVEN, block 73 — re-pinned at the typed
    /// level); the historical drivers were the engine's own C++
    /// subsystems, and the native signatures are UNKNOWN — the
    /// argument rules live in `prototype_engine::audio_seam`
    /// (fail-closed DESIGN).  The `SetFreeRoamMission` mission id,
    /// when carried, is diagnostic only (the world bed resolves
    /// from the ambience bank, not a mission cue).
    pub fn apply_audio_seam(
        &mut self,
        requests: &[prototype_engine::AudioSeamRequest],
        gain: f32,
    ) -> AudioSeamApply {
        let mut out = AudioSeamApply::default();
        for req in requests {
            let native = req.native_name();
            let res = match req {
                prototype_engine::AudioSeamRequest::SetMission { mission } => {
                    self.enter_mission(mission, gain)
                }
                prototype_engine::AudioSeamRequest::SetFreeRoamMission { .. } => {
                    self.enter_free_roam(gain)
                }
            };
            match res {
                Ok(frames) => {
                    out.applied += 1;
                    out.frames += frames;
                }
                Err(e) => out.failures.push((native, e)),
            }
        }
        if out.applied > 0 {
            log::info!(
                "audio seam: {} request(s) applied ({out:?})",
                requests.len()
            );
        }
        out
    }

    /// Spawn the CPU boot worker (block 103).  The heavy archive reads
    /// leave the main thread entirely; the GPU stages start when the
    /// artifacts arrive (see [`App::pump_boot`]).
    /// Block 107: is the menu up right now (it owns all input)?
    fn menu_consumes_input(&self) -> bool {
        !self.in_world
    }

    /// Block 107: the loading screen's boot line + honest fraction.
    fn menu_boot_info(&self) -> (String, Option<f32>) {
        let frac = boot_log::progress_fraction();
        let stage = boot_log::current_stage().unwrap_or_default();
        match &self.boot_phase {
            BootPhase::Done => ("world ready".to_string(), Some(1.0)),
            BootPhase::Failed(e) => (
                // Block 115: the failure line stays short and readable —
                // the FULL error lives in boot-log.txt.
                {
                    let e = e.clone();
                    let short = if e.len() > 60 { format!("{}… (see boot-log.txt)", &e[..60]) } else { e };
                    format!("boot failed: {short}")
                },
                None,
            ),
            BootPhase::Idle => ("preparing…".to_string(), None),
            _ => {
                // Block 115: THE LOADING TEXT FIX — the screen showed the
                // raw journal stage (paths, "member(s)", 292-second timers:
                // the user read it as broken text).  Human stages + a short
                // progress tail instead.
                let total = boot_log::progress_archives_total();
                let done = boot_log::progress_archives_done();
                let line = if stage.contains("shader prep") {
                    "Preparing shaders…".to_string()
                } else if stage.contains("index") {
                    format!("Reading game files — {} of {} archives", done, total)
                } else if stage.contains("census") {
                    "Checking menu art…".to_string()
                } else if stage.contains("pair") {
                    "Linking render shaders…".to_string()
                } else if stage.contains("gpu") || stage.contains("attach") {
                    "Preparing renderer…".to_string()
                } else if stage.contains("frontend") || stage.contains("art") {
                    "Extracting menu art…".to_string()
                } else if stage.is_empty() {
                    "Preparing…".to_string()
                } else {
                    format!("Working… ({})", &stage[..stage.len().min(40)])
                };
                (line, frac)
            }
        }
    }

    /// Block 107: run one menu navigation step and execute the action
    /// it hands back (the menu never touches winit/JNI itself).
    fn menu_nav(&mut self, action: crate::menu::MenuAction) {
        let (boot_line, _) = self.menu_boot_info();
        let ctx = crate::menu::MenuContext {
            boot_line: &boot_line,
            progress: boot_log::progress_fraction(),
            storage_denied: self.storage == StorageState::Denied,
            files_ok: matches!(self.state, AppState::Ready(_)),
            folder: "",
            stream_budget_mb: self.settings.stream_budget_mb,
            master_volume: self.settings.master_volume,
            look_sensitivity: self.settings.look_sensitivity,
        };
        let a = self.menu.nav(action, &ctx);
        match a {
            crate::menu::MenuAction::None => {}
            crate::menu::MenuAction::EnterWorld => self.menu_enter_world(),
            crate::menu::MenuAction::ExitWorld => {
                // Block 110: pause's MAIN MENU — the menu surface takes
                // over (in_world is already false; the boot stays Done).
                self.menu.screen = crate::menu::MenuScreen::Main;
                self.selected_reset();
                self.status_render = true;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            crate::menu::MenuAction::QuitApp => self.quit_requested = true,
            crate::menu::MenuAction::OpenStorageSettings => {
                if let Some(ui) = self.platform.as_mut() {
                    let _ = ui.ensure_storage();
                }
            }
            crate::menu::MenuAction::Adjust { delta } => {
                // Block 112: map (screen, row, step) onto the knob,
                // apply, persist (the real setting is live immediately:
                // the stream budget re-reads at the next attach).
                let s = &mut self.settings;
                match self.menu.screen {
                    crate::menu::MenuScreen::OptionsVideo => match self.menu.selected {
                        0 => {
                            s.stream_budget_mb = (s.stream_budget_mb as i32 + delta * 8)
                                .clamp(8, 64) as u32;
                        }
                        _ => {
                            s.mesh_cap_per_frame =
                                if s.mesh_cap_per_frame > 3 { 3 } else { 6 };
                        }
                    },
                    crate::menu::MenuScreen::OptionsAudio => {
                        s.master_volume =
                            (s.master_volume + delta as f32 * 0.1).clamp(0.0, 1.0);
                    }
                    crate::menu::MenuScreen::OptionsControls => {
                        s.look_sensitivity =
                            (s.look_sensitivity + delta as f32 * 0.25).clamp(0.25, 3.0);
                    }
                    _ => {}
                }
                crate::settings::save(&self.folder, s);
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            crate::menu::MenuAction::RetryScan => {
                self.state = scan_or_setup(&self.folder);
                if let AppState::Ready(set) = &self.state {
                    if matches!(self.boot_phase, BootPhase::Idle | BootPhase::Failed(_)) {
                        self.spawn_boot(set.clone());
                    }
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }

    /// Block 110: reset the menu selection for a fresh screen.
    fn selected_reset(&mut self) {
        self.menu.selected = 0;
    }

    /// Block 107: enter the streamed world (Play / difficulty accept).
    fn menu_enter_world(&mut self) {
        if matches!(self.boot_phase, BootPhase::Done) {
            self.menu.screen = crate::menu::MenuScreen::InGame;
            self.in_world = true;
            self.status_render = false;
        } else {
            self.menu.screen = crate::menu::MenuScreen::Loading;
        }
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    /// Block 107: keyboard navigation (arrows/WASD, enter, esc; the
    /// press-any-button screens take ANY key).
    fn menu_key(&mut self, key: &winit::keyboard::Key) {
        use winit::keyboard::{Key, NamedKey};
        if matches!(
            self.menu.screen,
            crate::menu::MenuScreen::Start | crate::menu::MenuScreen::Legal
        ) {
            self.menu_nav(crate::menu::MenuAction::Accept);
            return;
        }
        let action = match key {
            Key::Named(NamedKey::ArrowUp) => crate::menu::MenuAction::Move(-1),
            Key::Named(NamedKey::ArrowDown) => crate::menu::MenuAction::Move(1),
            Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space) => {
                crate::menu::MenuAction::Accept
            }
            // Block 115: the ANDROID BACK button/gesture — winit's
            // android backend surfaces it as Backspace (KEYCODE_BACK),
            // and desktop backspace reads as "back" too.  Both route to
            // the same Back action (in-world: exit to the menu).
            Key::Named(NamedKey::Escape) | Key::Named(NamedKey::Backspace) => {
                crate::menu::MenuAction::Back
            }
            Key::Character(c) => match c.as_str() {
                "w" | "W" => crate::menu::MenuAction::Move(-1),
                "s" | "S" => crate::menu::MenuAction::Move(1),
                "e" | "E" => crate::menu::MenuAction::Accept,
                _ => crate::menu::MenuAction::None,
            },
            _ => crate::menu::MenuAction::None,
        };
        if action != crate::menu::MenuAction::None {
            self.menu_nav(action);
        }
    }

    /// Block 107: pointer click on the menu (row hit-testing against
    /// the LAST drawn frame's layout; the press-any screens accept any
    /// tap).  Android taps arrive here through the block-38 touch→mouse
    /// mapping.
    fn menu_click(&mut self) {
        if self.menu_hits.any
            || matches!(
                self.menu.screen,
                crate::menu::MenuScreen::Start | crate::menu::MenuScreen::Legal
            )
        {
            self.menu_nav(crate::menu::MenuAction::Accept);
            return;
        }
        let [x, y] = self.cursor;
        for (row, r) in &self.menu_hits.rows {
            if *row == crate::menu::ROW_BACK
                && x >= r[0]
                && x < r[0] + r[2]
                && y >= r[1]
                && y < r[1] + r[3]
            {
                self.menu_nav(crate::menu::MenuAction::Back);
                return;
            }
            if x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3] {
                self.menu.selected = *row;
                self.menu_nav(crate::menu::MenuAction::Accept);
                return;
            }
        }
    }

    /// Block 107: build + render one menu/UI frame (the status surface).
    /// `tint` is the phase colour used as the clear under the menu field.
    fn draw_menu_frame(&mut self, tint: [f64; 4]) {
        // Auto-enter: the loading screen hands to the world the moment
        // the boot lands (the "boot directly into the game" contract).
        if self.menu.screen == crate::menu::MenuScreen::Loading
            && matches!(self.boot_phase, BootPhase::Done)
        {
            self.menu.screen = crate::menu::MenuScreen::InGame;
            self.in_world = true;
            self.status_render = false;
            if let Some(w) = &self.window {
                w.request_redraw();
            }
            return;
        }
        let Some((w, h)) = self
            .renderer
            .as_ref()
            .map(|r| r.surface_size())
        else {
            return;
        };
        if self.atlas.is_none() {
            if let Some(r) = self.renderer.as_mut() {
                self.atlas = Some(r.init_ui());
            }
        }
        let (boot_line, progress) = self.menu_boot_info();
        let ctx = crate::menu::MenuContext {
            boot_line: &boot_line,
            progress,
            storage_denied: self.storage == StorageState::Denied,
            files_ok: matches!(self.state, AppState::Ready(_)),
            folder: &self.folder.display().to_string(),
            stream_budget_mb: self.settings.stream_budget_mb,
            master_volume: self.settings.master_volume,
            look_sensitivity: self.settings.look_sensitivity,
        };
        let mut list = prototype_render::UiDrawList {
            screen: [w as f32, h as f32],
            ..Default::default()
        };
        let mut hits = crate::menu::HitRegions::default();
        {
            let atlas = self.atlas.as_ref().expect("just built");
            crate::menu::menu_draw(&self.menu, &ctx, atlas, &mut list, &mut hits);
        }
        self.menu_hits = hits;
        if let Some(r) = self.renderer.as_mut() {
            if let Err(e) = r.render_frame_with_ui(tint, &[], &list) {
                log::warn!("menu frame failed: {e:?}");
            }
        }
    }

    fn spawn_boot(&mut self, set: GameFileSet) {
        boot_log::stage("spawn cpu worker");
        self.boot_started = Some(Instant::now());
        let (tx, rx) = mpsc::channel();
        match std::thread::Builder::new()
            .name("prototype-boot".into())
            .spawn(move || cpu_boot_work(set, tx))
        {
            Ok(_join) => self.boot_phase = BootPhase::Cpu { rx },
            Err(e) => self.boot_fail(format!("worker spawn failed: {e}")),
        }
    }

    /// Record a boot failure (block 103): the failure red + a toast +
    /// the journal line.  The next resume retries the boot (the user
    /// may have fixed their files meanwhile).
    fn boot_fail(&mut self, msg: String) {
        log::error!("boot failed: {msg}");
        boot_log::bootlog(&format!("BOOT FAILED: {msg}"));
        self.boot_phase = BootPhase::Failed(msg.clone());
        if let Some(ui) = self.platform.as_ref() {
            ui.toast(&format!(
                "Boot failed: {} — send boot-log.txt",
                truncate_toast(&msg)
            ));
        }
    }

    /// Advance the boot phase machine ONE step (block 103; called from
    /// `about_to_wait` every loop iteration).  Every GPU stage must be
    /// SHORT — the loop must keep pumping events or Android ANR-kills
    /// the app (the exact bug this machine replaces).
    fn pump_boot(&mut self) {
        // mem::replace keeps the borrow checker happy: the stage calls
        // need `&mut self` (renderer, pipelines, paired) while the phase
        // owns the artifacts — the block-55 E0499 lesson again.
        match std::mem::replace(&mut self.boot_phase, BootPhase::Idle) {
            BootPhase::Cpu { rx } => match rx.try_recv() {
                Ok(Ok(art)) => {
                    self.boot_phase = BootPhase::Gpu {
                        art,
                        stage: GpuStage::Pipelines,
                        next_paired: 0,
                        next_world: 0,
                        spawned: false,
                        member_of_batch: Vec::new(),
                        world_built: 0,
                    };
                }
                Ok(Err(msg)) => self.boot_fail(msg),
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.boot_fail("cpu worker died without a report".into());
                }
                Err(mpsc::TryRecvError::Empty) => {
                    // Still reading the archives — keep the tint up.
                    self.boot_phase = BootPhase::Cpu { rx };
                }
            },
            BootPhase::Gpu {
                mut art,
                stage,
                next_paired,
                next_world,
                spawned,
                member_of_batch,
                world_built,
            } => match stage {
                GpuStage::Pipelines => {
                    if let Some(next) = self.gpu_stage_pipelines(&mut art) {
                        self.boot_phase = BootPhase::Gpu {
                            art,
                            stage: next,
                            next_paired,
                            next_world,
                            spawned,
                            member_of_batch,
                            world_built,
                        };
                    } // None -> Failed already set; artifacts drop
                }
                GpuStage::Paired => {
                    if let Some((next, np, sp)) =
                        self.gpu_stage_paired(&mut art, next_paired, spawned)
                    {
                        self.boot_phase = BootPhase::Gpu {
                            art,
                            stage: next,
                            next_paired: np,
                            next_world,
                            spawned: sp,
                            member_of_batch,
                            world_built,
                        };
                    }
                }
                GpuStage::World => {
                    if let Some((next, nw, mob, wb)) =
                        self.gpu_stage_world(&mut art, next_world, member_of_batch, world_built)
                    {
                        self.boot_phase = BootPhase::Gpu {
                            art,
                            stage: next,
                            next_paired,
                            next_world: nw,
                            spawned,
                            member_of_batch: mob,
                            world_built: wb,
                        };
                    }
                }
                GpuStage::Attach => {
                    self.gpu_stage_attach(art, member_of_batch, world_built);
                    // Done (or Failed) is set inside — nothing to
                    // re-store: the artifacts are consumed.
                }
            },
            other => self.boot_phase = other,
        }
    }

    /// GPU stage 1 (block 103): surface + device + the shader-pair
    /// pipelines + the world pipeline.  One tick for the whole stage —
    /// the WGSL translations are the shortest GPU pole; the archive
    /// reads were the long one and already finished off-thread.
    fn gpu_stage_pipelines(&mut self, art: &mut CpuBootArtifacts) -> Option<GpuStage> {
        boot_log::stage("gpu: surface + device");
        // The status renderer must go first — one live surface per
        // window keeps the Android compositor honest.
        self.renderer = None;
        let Some(window) = self.window.clone() else {
            self.boot_fail("window gone during boot".into());
            return None;
        };
        let size = window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        let renderer = match pollster::block_on(Renderer::new(window.clone(), w, h)) {
            Ok(r) => r,
            Err(e) => {
                self.boot_fail(format!("renderer creation failed: {e}"));
                return None;
            }
        };
        let format = renderer.surface_format();

        boot_log::stage("gpu: shader pipelines");
        let mut built = 0usize;
        let mut failed = Vec::new();
        for pair in &art.report.pairs {
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
        for f in &failed {
            log::warn!("pipeline failed: {f}");
        }
        log::info!("pipelines: {built} built, {} failed", failed.len());
        boot_log::bootlog(&format!("pipelines: {built} built, {} failed", failed.len()));

        // Block 34: the REAL world pipeline — the DLL `default` pair
        // from the user's engine DLL.  Failure is non-fatal: the
        // frontend fallback stays, with the reason logged.
        if let Some(plan) = art.world_plan.as_ref() {
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

        self.aspect = {
            let size = window.inner_size();
            if size.height > 0 {
                size.width as f32 / size.height as f32
            } else {
                1.0
            }
        };
        self.renderer = Some(renderer);
        Some(GpuStage::Paired)
    }

    /// Which pipeline world geometry draws through (the block-34 rule)
    /// — the DLL `default` pair when it built, else pipeline[0].
    fn mesh_pipeline_ref(&self) -> &GpuPipeline {
        if self.world_pipeline.is_some() {
            self.world_pipeline.as_ref().unwrap()
        } else {
            &self.pipelines[0]
        }
    }

    /// GPU stage 2 (block 103): the placed-object spawn (once) + the
    /// paired-drawable batches, [`MESHES_PER_TICK`] per iteration.
    fn gpu_stage_paired(
        &mut self,
        art: &mut CpuBootArtifacts,
        next: usize,
        spawned: bool,
    ) -> Option<(GpuStage, usize, bool)> {
        let mut spawned = spawned;
        if !spawned {
            boot_log::stage("gpu: world spawn");
            let (_w_objs, _w_shapes) = art.world.index.totals();
            let (_w_retained, w_spawnable) = art.world.index.retained_totals();
            let spawn_report = self
                .world_objects
                .spawn_from_index(&art.world.index, SPAWN_CAP);
            self.spawn_report = Some(spawn_report);
            let classes = self.world_objects.class_counts();
            let class_line: Vec<String> = classes
                .iter()
                .take(6)
                .map(|(c, n)| format!("{c}×{n}"))
                .collect();
            let spawned_n = self.spawn_report.as_ref().map(|r| r.spawned).unwrap_or(0);
            log::info!(
                "world spawn: {} entities ({} duplicates{}, cap {}) of {} world-space spawner(s); classes: {}",
                spawned_n,
                self.spawn_report.as_ref().map(|r| r.duplicates).unwrap_or(0),
                if self.spawn_report.as_ref().map(|r| r.capped).unwrap_or(false) { ", CAPPED" } else { "" },
                SPAWN_CAP,
                w_spawnable,
                class_line.join(", ")
            );
            boot_log::bootlog(&format!("world spawn: {spawned_n} entities"));
            spawned = true;
        }
        // No pipelines at all -> nothing draws through; skip straight
        // to the world stage (its fallback path covers this).
        if self.pipelines.is_empty() {
            return Some((GpuStage::World, 0, spawned));
        }
        let mut i = next;
        let mut processed = 0usize;
        while i < art.pm_rep.meshes.len() && processed < MESHES_PER_TICK {
            // Borrow-split (all disjoint fields): the renderer mutably,
            // the pipeline immutably, the plan from `art`, `self.paired`
            // mutably.
            let plan = art.world_plan.as_ref();
            // Direct FIELD access only: an &self helper call would
            // collide with the renderer's &mut borrow below.
            let pipeline: &GpuPipeline = if self.world_pipeline.is_some() {
                self.world_pipeline.as_ref().unwrap()
            } else {
                &self.pipelines[0]
            };
            let Some(r) = self.renderer.as_mut() else {
                self.boot_fail("renderer gone during paired stage".into());
                return None;
            };
            let batch = build_mesh_batches(r, pipeline, plan, &art.pm_rep.meshes[i].mesh);
            if batch.is_empty() {
                break; // the old semantics: stop the paired pass early
            }
            let pm = &art.pm_rep.meshes[i];
            let pl = &pm.pair.placement;
            let member_hash = art
                .world
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
            i += 1;
            processed += 1;
        }
        if i >= art.pm_rep.meshes.len() {
            if !self.paired.is_empty() {
                log::info!(
                    "paired batches: {} local drawable(s) placed by REAL record(s) on pipeline '{}'",
                    self.paired.len(),
                    self.mesh_pipeline_ref().name
                );
            }
            boot_log::bootlog(&format!("paired batches: {}", self.paired.len()));
            Some((GpuStage::World, 0, spawned))
        } else {
            Some((GpuStage::Paired, i, spawned))
        }
    }

    /// GPU stage 3 (block 103): the world mesh batches, chunked; the
    /// provenance accumulates across ticks (block 20's contract: it
    /// stays aligned with `self.batches`).
    fn gpu_stage_world(
        &mut self,
        art: &mut CpuBootArtifacts,
        next: usize,
        member_of_batch: Vec<Option<usize>>,
        world_built: usize,
    ) -> Option<(GpuStage, usize, Vec<Option<usize>>, usize)> {
        let mut member_of_batch = member_of_batch;
        let mut world_built = world_built;
        // No pipelines -> the honest old fallback path (zero batches;
        // the attach stage's debug fallback covers the screen).
        if self.pipelines.is_empty() {
            return Some((GpuStage::Attach, 0, member_of_batch, world_built));
        }
        let mut i = next;
        let mut processed = 0usize;
        while i < art.scan.meshes.len() && processed < MESHES_PER_TICK {
            // Block 21: local drawable shapes (bakery/billboard/… —
            // centers < WORLD_LOCAL_SPLIT_M of the origin) need the
            // GameObject placement pairing; skip them.
            if !art.scan.mesh_world.get(i).copied().unwrap_or(false) {
                i += 1;
                continue;
            }
            let plan = art.world_plan.as_ref();
            // Direct FIELD access only (see the paired stage's note).
            let pipeline: &GpuPipeline = if self.world_pipeline.is_some() {
                self.world_pipeline.as_ref().unwrap()
            } else {
                &self.pipelines[0]
            };
            let Some(r) = self.renderer.as_mut() else {
                self.boot_fail("renderer gone during world stage".into());
                return None;
            };
            let batch = build_mesh_batches(r, pipeline, plan, &art.scan.meshes[i]);
            if batch.is_empty() {
                break; // the old semantics: stop the world pass early
            }
            // Block 21: warn once when the pipeline cannot receive the
            // per-frame player-camera WVP rows.
            if !self.warned_no_wvp
                && !batch[0]
                    .vertex_uniforms
                    .iter()
                    .any(|u| {
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
                    pipeline.name
                );
                self.warned_no_wvp = true;
            }
            for _ in 0..batch.len() {
                member_of_batch.push(art.scan.member_of_mesh.get(i).copied());
            }
            self.batches.extend(batch);
            // Block 44: the pool accounting — which member contributed
            // this world mesh (the exact unload subtraction later).
            if let Some(&gi) = art.scan.member_of_mesh.get(i) {
                *self.meshes_of_member.entry(gi).or_default() += 1;
            }
            world_built += 1;
            i += 1;
            processed += 1;
        }
        if i >= art.scan.meshes.len() {
            boot_log::bootlog(&format!("world mesh batches: {world_built}"));
            Some((GpuStage::Attach, 0, member_of_batch, world_built))
        } else {
            Some((GpuStage::World, i, member_of_batch, world_built))
        }
    }

    /// GPU stage 4 (block 103, one tick): the member-stream attach, the
    /// box pool, the debug fallback, the boot movie and the frontend
    /// entry — everything after the mesh chunks.  All non-fatal by the
    /// old boot's contract.  Consumes the artifacts.
    fn gpu_stage_attach(
        &mut self,
        art: CpuBootArtifacts,
        member_of_batch: Vec<Option<usize>>,
        world_built: usize,
    ) {
        boot_log::stage("gpu: stream attach + boot movie + frontend");
        let mesh_pipe_name = self.mesh_pipeline_ref().name.clone();
        let Some(r) = self.renderer.as_mut() else {
            self.boot_fail("renderer gone before attach".into());
            return;
        };
        // Block 104: the streaming gate is the INDEX, not the boot's
        // mesh count — the lean boot decodes NO meshes at boot and the
        // per-frame stream owns geometry.  Only a world with NO indexed
        // members at all (nothing parsed) falls back to debug triangles.
        let index_members = art.world.index.entries.len();
        if index_members > 0 {
            log::info!(
                "world index: {} member(s) (pipeline '{}'); the boot decodes no meshes — the per-frame stream loads geometry",
                index_members,
                mesh_pipe_name
            );
            // Block 44: the member stream attaches over the SAME scanned
            // index and is SEEDED with the members whose meshes the boot
            // actually loaded — the first frame's diff starts from the
            // boot's real state, not from zero.
            // Block 107: the streaming budget is the DISPLAY setting's
            // real knob (game-settings.json → the per-frame member
            // budget the stream actually honours).
            match prototype_stream::members::MemberStreamConfig::new(
                self.settings.stream_budget_mb as usize * 1024 * 1024,
                MEMBER_STREAM_HYSTERESIS,
            )
            .and_then(|config| {
                crate::member_stream::MemberMeshStream::from_index(
                    &art.set.rcf_archives,
                    &art.world.index,
                    config,
                )
            }) {
                Ok(mut ms) => {
                    // The old boot seeded from EVERY scanned member's
                    // provenance entry; `.get` (not `[gi]`) keeps a
                    // scan/index desync from panicking the worker era.
                    let seeded: std::collections::BTreeSet<u32> = art
                        .scan
                        .member_of_mesh
                        .iter()
                        .filter_map(|&gi| art.world.index.entries.get(gi).map(|e| e.hash))
                        .collect();
                    let seeded_n = seeded.len();
                    ms.seed_loaded(seeded);
                    // Block 45: the paired half attaches over the same
                    // pair report and is SEEDED with the geometry members
                    // whose paired meshes the boot pool already holds.
                    let paired_attached = ms.attach_paired(&art.pair_rep.pairs);
                    let paired_seeded: std::collections::BTreeSet<u32> = art
                        .pm_rep
                        .meshes
                        .iter()
                        .filter_map(|pm| art.world.index.entries.get(pm.pair.geometry_member))
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
            self.world_index = Some(art.world.index);
            self.draw_selector = Some(crate::meshes::DrawSelector::new(member_of_batch));
            self.member_meshes = world_built;
        } else {
            // No real meshes -> one drawable debug triangle per
            // pipeline: buffers + bind groups from the validated plans.
            self.batches = r.create_debug_batches(&self.pipelines);
            log::info!("debug batches: {} ready", self.batches.len());
            self.world_index = None;
            self.draw_selector = None;
        }

        // Block 21: the placed-object cube pool (the nearest spawners
        // draw as 2 m cubes at their REAL positions).
        if !self.world_objects.is_empty() {
            if let Some(p0) = self.pipelines.first() {
                self.boxes = r.create_box_batches(p0, BOX_POOL);
                log::info!(
                    "placed-object cubes: {} ready on pipeline '{}' (nearest per frame)",
                    self.boxes.len(),
                    p0.name
                );
            }
        }

        // 5. Block 54: the BOOT-PATH MOVIE — find a real Bink member in
        //    the user's movie archive(s) and open the streaming player
        //    over its RCF span.  Every failure here is non-fatal.
        self.movie = None;
        self.movie_gpu = None;
        self.movie_logged = false;
        let format = r.surface_format();
        match &art.movie {
            Some(m) => match MoviePlayerState::open(&m) {
                Ok(state) => {
                    let (mw, mh) = (state.width(), state.height());
                    let texture = r.create_video_texture(mw, mh, "boot movie");
                    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                    let quad = r.create_video_quad(format);
                    let bind_group = r.create_video_bind_group(&quad, &view);
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
                    self.movie_gpu = Some(MovieGpu {
                        quad,
                        texture,
                        view,
                        bind_group,
                    });
                    // Block 55: the audio DEVICE edge over THE ring this
                    // player writes.  Non-fatal: device-less hosts play
                    // silent, exactly as before.
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

        // 6. Block 57: the GAMEPLAY audio device edge — opened once per
        //    boot over the AudioOut ring (guarded, non-fatal).
        #[cfg(feature = "audio-cpal")]
        self.open_gameplay_audio();

        // 7. Block 68: when NO boot movie plays, the FRONTEND is what
        //    the user sees — its attract music starts here.
        if self.movie.is_none() {
            if let Err(e) = self.enter_frontend(1.0) {
                log::debug!("frontend music: {e}");
            }
        }

        self.boot_phase = BootPhase::Done;
        self.status_render = false;
        boot_log::bootlog("BOOT COMPLETE");
        if let Some(ui) = self.platform.as_ref() {
            let secs = self
                .boot_started
                .map(|t| t.elapsed().as_secs_f32())
                .unwrap_or(0.0);
            ui.toast(&format!("Engine booted in {:.0}s", secs));
        }
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    /// Drop everything GPU-side (Android pause invalidates the surface;
    /// desktop close/exiting reuses the same path).
    /// Install the platform UI (block 102) — Android's JNI toasts +
    /// storage grant flow.  The desktop demo runs with `None`.
    pub fn set_platform_ui(&mut self, ui: Box<dyn PlatformUi>) {
        self.platform = Some(ui);
    }

    /// The Setup-state status surface (block 102): window + a minimal
    /// clear-only renderer, so the screen carries the loader state
    /// instead of going black.  Cheap to rebuild on every resume.
    fn ensure_status_surface(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            match event_loop.create_window(self.window_attrs.clone()) {
                Ok(w) => {
                    log::info!("status window created (loader state on screen)");
                    self.window = Some(Arc::new(w));
                }
                Err(e) => {
                    log::error!("status window creation failed: {e}");
                    return;
                }
            }
        }
        self.status_render = true;
        if self.renderer.is_none() {
            if let Some(w) = self.window.clone() {
                let size = w.inner_size();
                let (w_px, h_px) = (size.width.max(1), size.height.max(1));
                match pollster::block_on(Renderer::new(w, w_px, h_px)) {
                    Ok(r) => self.renderer = Some(r),
                    // A not-yet-laid-out surface (0-size at creation on
                    // some drivers) lands here — the next Resized gives
                    // a real size and the colour waits one frame.
                    Err(e) => log::error!("status renderer creation failed: {e}"),
                }
            }
        }
    }

    /// Setup-state guidance (block 102): toast the ONE action the user
    /// must take, only when it CHANGES (`resumed` fires on every return
    /// from Settings — the same message must not re-toast).  The
    /// storage-denied case toasts nothing here: the grant flow's own
    /// prompt just fired.
    fn notify_setup(&mut self) {
        let AppState::Setup { folder, last_error } = &self.state else {
            return;
        };
        let message = match self.storage {
            StorageState::Denied => None,
            StorageState::Granted => Some(match last_error {
                Some(_) => format!(
                    "Prototype Rust folder problem — copy your game files to {}",
                    folder.display()
                ),
                None => format!("Copy your game files to {}", folder.display()),
            }),
        };
        let Some(message) = message else { return };
        if self.last_setup_toast.as_deref() != Some(message.as_str()) {
            if let Some(ui) = self.platform.as_ref() {
                ui.toast(&message);
            }
            log::info!("setup: {message}");
            self.last_setup_toast = Some(message);
        }
    }

    fn teardown(&mut self) {
        if self.renderer.is_some() || !self.pipelines.is_empty() {
            log::info!("tearing down GPU state (surface is invalid across pause)");
        }
        self.renderer = None;
        // Block 103: a boot in flight dies with the suspend too — the
        // next resume spawns a fresh worker (a detached old worker's
        // artifacts drop on its failed send; CPU-only, harmless).
        self.boot_phase = BootPhase::Idle;
        self.boot_started = None;
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
        // Block 106: window presentation BEFORE anything else — the
        // bars hide on every resume (returning from Settings or any
        // system UI re-applies them for free).
        if let Some(ui) = self.platform.as_ref() {
            ui.apply_window_presentation();
        }
        // Block 102: storage access FIRST — the scan below (and the
        // loader's folder creation) is useless without it on Android 8+.
        self.storage = match self.platform.as_mut() {
            Some(ui) => ui.ensure_storage(),
            None => StorageState::Granted,
        };
        log::info!(
            "lifecycle: resume — re-scanning asset folder (storage {:?})",
            self.storage
        );
        self.state = scan_or_setup(&self.folder);
        if let AppState::Setup { .. } = self.state {
            // Files not (yet) usable — but the loader must not go dark:
            // a status surface stays up (block 102), with the colour +
            // toast saying exactly what to do next.
            self.ensure_status_surface(event_loop);
            self.notify_setup();
            return;
        }
        // Block 103: the boot is a PHASE MACHINE (worker thread + GPU
        // stages) and the status surface stays up as its backdrop — the
        // screen shows the booting tint instead of a frozen window, and
        // winit keeps pumping events (the old synchronous try_boot
        // inside `resumed` was the ~10 s ANR that killed the app).
        if matches!(self.boot_phase, BootPhase::Done) && self.renderer.is_none() {
            // The suspend tore the GPU state down but the phase still
            // says Done — a fresh boot is the only renderer factory.
            self.boot_phase = BootPhase::Idle;
        }
        if !matches!(self.boot_phase, BootPhase::Done) {
            self.ensure_status_surface(event_loop);
        }
        if let AppState::Ready(set) = &self.state {
            let set = set.clone();
            // Block 58/66: the audio CONTENT half — device-free, quick
            // and self-guarding (a re-boot never re-loads what is
            // loaded).  These were the old try_boot's step 0/0b.
            self.load_gameplay_sounds(&set);
            self.load_music_graph(&set);
            if matches!(self.boot_phase, BootPhase::Idle | BootPhase::Failed(_)) {
                if let BootPhase::Failed(prev) = &self.boot_phase {
                    log::warn!("boot retry after failure: {prev}");
                }
                self.spawn_boot(set);
            }
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        log::info!("lifecycle: suspend");
        self.teardown();
        // Block 102: the status surface dies with the suspend too — the
        // next resume re-decides the state and rebuilds it.
        self.status_render = false;
        self.last_setup_toast = None;
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {
        // Block 107: the menu's QUIT GAME lands here — the event loop
        // is the only place an exit is legal.
        if self.quit_requested {
            event_loop.exit();
            return;
        }
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
                // Block 108: the stick zone + UI layout basis.
                self.screen_px = [size.width as f32, size.height as f32];
            }
            WindowEvent::RedrawRequested => {
                // Block 102: the Setup status frame — clear-only, the
                // colour IS the message (red = grant storage, amber =
                // copy game files).  Short-circuits the whole gameplay
                // path: nothing is booted while the loader waits.
                if self.status_render {
                    // The screen enum decouples this module from
                    // AppState's feature gate (see platform_ui docs).
                    // Block 103: a Ready state can still be BOOTING or
                    // BOOT-FAILED — the phase machine owns the colour.
                    let screen = match (&self.state, &self.boot_phase) {
                        (AppState::Setup { .. }, _) => StatusScreen::Setup,
                        (_, BootPhase::Failed(_)) => StatusScreen::BootFailed,
                        (AppState::Ready(_), BootPhase::Done) => StatusScreen::Ready,
                        _ => StatusScreen::Boot,
                    };
                    let color = match status_clear_color(screen, self.storage) {
                        Some(c) => c,
                        None => return,
                    };
                    // Block 104: PULSE the booting tint — a slow breath
                    // on brightness.  A static grey is indistinguishable
                    // from a hang; the user must SEE the boot working.
                    // Block 107: the status surface is the REAL menu
                    // system now — the flat yellow/green/grey/red debug
                    // tints are dead.  The phase tint stays as the CLEAR
                    // colour under the menu's own field (visible on
                    // slow devices before the first menu frame lands).
                    let _ = screen;
                    self.draw_menu_frame(color);
                    return;
                }
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
                    // Block 108: the stick's key reconcile feeds the
                    // SAME InputState the keyboard does.
                    self.stick.apply(&mut self.input);
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
                                                        // Block 115: budgeted — 250 ms of loading per
                            // frame, the settings cap as the member limit;
                            // the rest streams over the following frames
                            // (the near-set re-plans every frame).
                            let deadline =
                                std::time::Instant::now() + std::time::Duration::from_millis(250);
                            let cap = self.settings.mesh_cap_per_frame.max(1) as usize;
                            let loads = stream.load_meshes(&set.load, room, Some(deadline), cap);
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

                    // Block 108: the world frame + the stick overlay
                    // (one UI pass; an empty list = no overlay cost).
                    let ui = stick_overlay(self.screen_px, self.in_world, self.stick.active);
                    let frame = if ui.verts.is_empty() {
                        r.render_frame_selected(CLEAR, &picked)
                    } else {
                        r.render_frame_selected_with_ui(CLEAR, &picked, &ui)
                    };
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
                // Block 108: the walk zone — a finger STARTING in the
                // left 45% of the screen (in-world) captures the stick;
                // that finger never reaches the block-38 mapping.  Menu
                // time keeps the mapper untouched (taps = clicks).
                if self.in_world && !matches!(self.state, AppState::Setup { .. }) {
                    let in_zone = t.phase == WinitTouchPhase::Started
                        && (t.location.x as f32) < self.screen_px[0] * 0.45;
                    if in_zone {
                        self.stick.begin(t.id as u32, t.location.x as f32, t.location.y as f32);
                        return;
                    }
                    if self.stick.is_capturing(t.id as u32) {
                        match t.phase {
                            WinitTouchPhase::Moved => {
                                self.stick.move_to(t.id as u32, t.location.x as f32, t.location.y as f32);
                            }
                            WinitTouchPhase::Ended | WinitTouchPhase::Cancelled => {
                                self.stick.end(t.id as u32);
                            }
                            _ => {}
                        }
                        return;
                    }
                }
                // Block 113: THE MENU-TAP FIX — the cursor position is
                // ONLY updated by winit's CursorMoved, which a finger
                // never fires: menu taps hit-tested (0,0) and no row
                // ever matched.  Sync the pointer from the touch FIRST
                // (every menu click uses the real tap position now).
                self.cursor = [point.x, point.y];
                // Block 38: finger 1 = mouse (the pre-38 contract), a
                // concurrent finger 2 = the camera look — its pixel
                // deltas feed the orbit the frame loop applies.
                match self.touch.map_all(phase, point) {
                    TouchMapped::Mouse(evs) => {
                        // Block 107: finger-1 taps ARE the menu clicks on
                        // Android (the mapper synthesises mouse events).
                        if self.menu_consumes_input() {
                            for ev in evs {
                                if matches!(ev, InputEvent::MouseDown(_)) {
                                    self.menu_click();
                                }
                            }
                        } else {
                            for ev in evs {
                                self.input.apply(ev);
                            }
                        }
                    }
                    TouchMapped::Look { dx, dy } => {
                        // Block 107: the CONTROLS setting is a real knob.
                        let s = self.settings.look_sensitivity;
                        self.look.drag(dx * s, dy * s);
                    }
                    TouchMapped::Ignored => {}
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                // Block 54: a key press while the boot movie plays skips.
                if event.state == ElementState::Pressed {
                    self.skip_boot_movie();
                }
                // Block 107: the menu owns the keyboard whenever it is
                // up; in-world gameplay input flows as before.
                if self.menu_consumes_input() {
                    if event.state == ElementState::Pressed {
                        self.menu_key(&event.logical_key);
                    }
                } else if let Some(k) = map_key(&event.logical_key) {
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
                // Block 107: the menu hit-tests against this.
                self.cursor = [position.x as f32, position.y as f32];
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
                // Block 107: menu clicks (row hit-testing) — gameplay
                // input otherwise.
                if self.menu_consumes_input() {
                    if state == ElementState::Pressed && b == MouseButton::Left {
                        self.menu_click();
                    }
                } else {
                    self.input.apply(ev);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Block 103: advance the boot phase machine one step per loop
        // iteration — every stage must return FAST so winit keeps
        // pumping events (the ~10 s ANR that killed the first on-device
        // boot was the event loop wedged inside a synchronous boot).
        self.pump_boot();
        // Continuous frame loop: one redraw requested per iteration while
        // a renderer is live (Fifo present mode paces it to vsync).  The
        // status surface keeps this alive during the boot phases too.
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

    /// Block 72 — the `mixbuss` routing, END TO END REAL: the world
    /// bed rides the `amb` bus and the mission-music leg the `music`
    /// bus (the real families' label sets), per-bus gains shape ONLY
    /// their own channel (bit-exact, mirrored independently), the
    /// master gain scales the whole sum, and the lifecycle tags still
    /// cut per unit.  The state machine serves one channel per state
    /// by DESIGN; this test drives BOTH channels through the SAME
    /// serve path directly so the isolation arithmetic is observable
    /// in one mix.
    #[test]
    fn bus_routing_isolates_the_real_channels_end_to_end() {
        let (member, bed_expected) =
            synth_radp_member("ambience_baked_18ch", 18, 32_000, 3, &[100, 200, 30_000]);
        let dir = std::env::temp_dir().join(format!(
            "prototype-rs-app-bus-{}",
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

        // The music leg's expected PCM, computed INDEPENDENTLY: the
        // REAL member decoded through the same GameplaySounds path
        // the serve uses (stereo 48 kHz — no fold, no resample).
        let gs = prototype_engine::GameplaySounds::from_p3d_bytes(
            &real_ambient_bed_member(),
            "bus-pin",
        )
        .unwrap();
        let music_clip = gs.get("e02m02_ambient").unwrap();
        assert_eq!(music_clip.channels, 2);
        assert_eq!(music_clip.sample_rate, 48_000);
        let music_expected: Vec<i16> = music_clip.samples.clone();

        // Free-roam: the world bed rides the AMB bus (the real
        // Ambience_all set is [master, amb, sfx] — our family bus).
        app.enter_free_roam(1.0).expect("the bed plays");
        let amb_id = app
            .audio
            .mixer_ref()
            .bus_id("amb")
            .expect("the amb bus exists");
        assert_eq!(
            app.audio.mixer_ref().voice(0).unwrap().bus,
            Some(amb_id),
            "the bed voice rides the amb bus"
        );
        assert_eq!(app.audio_bus_gain("amb"), Some(1.0));

        // The music leg through the SAME serve path (fresh untracked
        // tag, the music bus) — both channels live at once.
        let music_id = app.audio.mixer().bus_id_or_add("music");
        app.play_shard_clip_tagged("e02m02_ambient", 1.0, Some(999_999), true, Some(music_id))
            .expect("the music leg spawns");
        assert_eq!(app.audio.mixer_ref().len(), 2, "bed + music");
        assert_eq!(
            app.audio.mixer_ref().voice(1).unwrap().bus,
            Some(music_id),
            "the music voice rides the music bus"
        );
        assert_eq!(app.audio_bus_gain("music"), Some(1.0));

        // The ACTUAL voice gains: the bed's is the patch's PROVEN
        // volume (caller 1.0 × 2.0), the music leg's is the direct
        // call's 1.0 — the mirror must use them exactly.
        let (bed_gain, mus_gain) = {
            let m = app.audio.mixer_ref();
            (m.voice(0).unwrap().gain, m.voice(1).unwrap().gain)
        };
        assert_eq!(bed_gain, 2.0, "the PROVEN patch volume reached the bed voice");
        assert_eq!(mus_gain, 1.0);

        // Tick 32 stereo frames PER CALL — the voices' cursors
        // advance, so window `w` mixes samples [w·64, w·64+64) of
        // each voice (the one clamp included).
        let mirror = |w: usize, music_bus: f32, master: f32| -> Vec<i16> {
            (0..64)
                .map(|j| {
                    let v = bed_expected[w * 64 + j] as f32 * bed_gain
                        + music_expected[w * 64 + j] as f32 * mus_gain * music_bus;
                    (v * master).clamp(-32768.0, 32767.0) as i16
                })
                .collect()
        };
        let out = app.audio.mixer().tick(32, 2);
        assert_eq!(
            out.samples,
            mirror(0, 1.0, 1.0),
            "both buses at 1.0: the plain sum (bed at its PROVEN volume)"
        );
        // Music bus at 0.5: ONLY the music leg halves — the bed's
        // contribution is bit-identical to the line above.
        app.set_audio_bus_gain("music", 0.5);
        let out = app.audio.mixer().tick(32, 2);
        assert_eq!(
            out.samples,
            mirror(1, 0.5, 1.0),
            "the music bus gain shapes only the music leg"
        );
        // Master at 0.5: the WHOLE sum halves (every real list
        // carries master — the output stage).
        app.audio.mixer().set_master_gain(0.5);
        let out = app.audio.mixer().tick(32, 2);
        assert_eq!(
            out.samples,
            mirror(2, 0.5, 0.5),
            "the master gain scales both buses"
        );
        app.audio.mixer().set_master_gain(1.0);
        // The bed cut BY TAG: only the music voice remains, still on
        // its bus, still at its gain.
        assert_eq!(app.stop_world_ambience(), 1, "the bed unit dies");
        assert_eq!(app.audio.mixer_ref().len(), 1);
        let out = app.audio.mixer().tick(32, 2);
        let music_only: Vec<i16> = (0..64)
            .map(|j| (music_expected[192 + j] as f32 * 0.5).clamp(-32768.0, 32767.0) as i16)
            .collect();
        assert_eq!(out.samples, music_only, "the music leg alone, at 0.5");
        // The empty name is the default bus — the seam refuses it.
        assert!(!app.set_audio_bus_gain("", 0.1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Block 72 — the NIS mixes ride the `nis` bus (the real
    /// NIS_Player set `[nis, master, dialog]`): the folded mix spawns
    /// on it, the bus gain shapes ONLY it (bit-exact against the
    /// voice's own samples), and the seam round-trips.
    #[test]
    fn nis_mixes_ride_the_nis_bus() {
        let dir = std::env::temp_dir().join(format!(
            "prototype-rs-app-nisbus-{}",
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

        app.play_nis_mix("mnis5_2_1", 1.0).expect("the mix plays");
        let nis_id = app
            .audio
            .mixer_ref()
            .bus_id("nis")
            .expect("the nis bus exists");
        assert_eq!(
            app.audio.mixer_ref().voice(0).unwrap().bus,
            Some(nis_id),
            "the NIS mix rides the nis bus"
        );
        assert_eq!(app.audio_bus_gain("nis"), Some(1.0));
        // The folded mix's own samples, halved by the bus gain —
        // bit-exact (the fold itself is pinned by the block-67 suite).
        let s = app.audio.mixer_ref().voice(0).unwrap().samples.clone();
        app.set_audio_bus_gain("nis", 0.5);
        let out = app.audio.mixer().tick(16, 2);
        let expected: Vec<i16> = (0..32)
            .map(|j| (s[j] as f32 * 0.5).clamp(-32768.0, 32767.0) as i16)
            .collect();
        assert_eq!(out.samples, expected, "the nis bus gain shapes only the mix");
        // The round-trip seam.
        assert_eq!(app.audio_bus_gain("nis"), Some(0.5));
        assert_eq!(app.audio_bus_gain("never-created"), None);
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
                p.ring_handle().lock().unwrap().produced(),
                3_840,
                "frame 1's track-0 f32 count → s16 written into the movie ring (produced — the race-free write-side counter; a live device edge may drain `buffered()` before this assert runs)"
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

    // ------------------------------------------------------------------
    // Block 74: the audio-seam DRIVER (snd_SetMission /
    // snd_SetFreeRoamMission → the real serve legs)
    // ------------------------------------------------------------------

    /// The driver's failure contract: an empty batch is a no-op and
    /// a failed request (unknown mission) is CAPTURED, not fatal —
    /// the app state stays consistent and the batch continues.
    #[test]
    fn audio_seam_driver_noops_and_captures_failures() {
        let dir = std::env::temp_dir().join(format!(
            "prototype-rs-app-seamfail-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let set = crate::game_files::scan_folder(&dir).unwrap();
        let mut app = App::with_state(dir.clone(), AppState::Ready(set));
        app.music_graph
            .merge_bank_bytes(&bank_fixture("bank_e02_music.p3d"), "audio\\e02_music.p3d")
            .unwrap();

        // Empty batch: a pure no-op.
        let out = app.apply_audio_seam(&[], 1.0);
        assert_eq!(out, AudioSeamApply::default());
        assert_eq!(app.mission().phase(), crate::mission::MissionPhase::Silent);
        assert_eq!(app.audio.mixer_ref().len(), 0);

        // An unknown mission: captured per-request, batch continues,
        // state consistent (the DESIGN cut-first serve already ran —
        // nothing was playing, nothing is playing).
        let reqs = vec![
            prototype_engine::AudioSeamRequest::SetMission {
                mission: "not-a-mission".into(),
            },
            prototype_engine::AudioSeamRequest::SetFreeRoamMission { mission: None },
        ];
        let out = app.apply_audio_seam(&reqs, 1.0);
        assert_eq!(out.applied, 1, "only the free-roam leg applied");
        assert_eq!(out.failures.len(), 1);
        assert_eq!(
            out.failures[0].0,
            prototype_engine::SND_SET_MISSION,
            "the failure names its native"
        );
        assert!(
            out.failures[0].1.contains("not-a-mission"),
            "the diagnostic carries the mission id: {}",
            out.failures[0].1
        );
        // The free-roam leg still served (tracked silence — no
        // ambience bank in this graph).
        assert_eq!(app.mission().phase(), crate::mission::MissionPhase::FreeRoam);
        assert_eq!(app.audio.mixer_ref().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// THE BLOCK-74 E2E, all real: the REAL frevents.lua corpus runs
    /// on the script host that now carries the snd natives (zero
    /// requests from the corpus itself — block 73's pinned-empty
    /// intersection, re-pinned through the app path), the natives are
    /// driven THROUGH the VM exactly the way the engine's own C++
    /// subsystems historically drove them, the typed requests cross
    /// into the app, and each one lands on its REAL serve leg — the
    /// mission stem on the `music` bus, the ambience bank's PROVEN
    /// world bed on the `amb` bus — with last-write-wins semantics.
    #[test]
    fn snd_call_through_drives_the_real_serve_paths_end_to_end() {
        use prototype_engine::{ScriptedFreeRoam, SND_SET_FREE_ROAM_MISSION, SND_SET_MISSION};
        use prototype_lua::vm::Value;

        let lua_str = |s: &str| Value::Str(std::rc::Rc::from(s.as_bytes()));

        // ---- 1. The REAL corpus on the REAL host --------------------
        let frevents = bank_fixture("frevents.lua");
        let f = prototype_lua::Lua51File::parse(&frevents).unwrap();
        let mut fre = ScriptedFreeRoam::new(&f).expect("the corpus runs");
        assert!(
            fre.audio_seam_requests().unwrap().is_empty(),
            "the corpus never drives the audio seam (PROVEN, block 73)"
        );
        assert_eq!(fre.system().events().len(), 60, "the corpus flow intact");

        // ---- 2. The natives, driven THROUGH the VM ------------------
        fre.call_global(SND_SET_MISSION, &[lua_str("e02m02")])
            .expect("snd_SetMission is callable on the corpus host");
        fre.call_global(SND_SET_FREE_ROAM_MISSION, &[lua_str("freKill")])
            .expect("snd_SetFreeRoamMission is callable on the corpus host");
        let reqs = fre.audio_seam_requests().expect("args are valid");
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].native_name(), SND_SET_MISSION);
        assert_eq!(reqs[0].mission(), Some("e02m02"));
        assert_eq!(reqs[1].native_name(), SND_SET_FREE_ROAM_MISSION);

        // ---- 3. The REAL app: real graph, real banks ----------------
        let (member, _expected) =
            synth_radp_member("ambience_baked_18ch", 18, 32_000, 3, &[100, 200, 30_000]);
        let dir = std::env::temp_dir().join(format!(
            "prototype-rs-app-seam-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("00audio.rcf"),
            synthesize_named_archive(&[
                (
                    "audio\\english\\AudioFile\\ambience_baked_18ch.p3d",
                    member,
                ),
                (
                    "audio\\english\\AudioFile\\e02m02_ambient.p3d",
                    real_ambient_bed_member(),
                ),
            ]),
        )
        .unwrap();
        let set = crate::game_files::scan_folder(&dir).unwrap();
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

        // ---- 4. The call-through, both legs -------------------------
        let out = app.apply_audio_seam(&reqs, 1.0);
        assert_eq!(out.applied, 2, "both requests drove a serve path");
        assert!(out.failures.is_empty(), "{out:?}");
        assert!(out.frames > 0, "the stem AND the bed spawned frames");
        // LAST-WRITE-WINS: the free-roam request came second, so the
        // WORLD BED owns the audible channel and the mission stem is
        // cut — the same contract the direct seams have.
        assert_eq!(
            app.mission().phase(),
            crate::mission::MissionPhase::FreeRoam
        );
        assert!(app.world_bed_active());
        assert_eq!(app.music_unit_tag, None, "the mission stem was cut");
        assert_eq!(app.audio.mixer_ref().len(), 1, "exactly one unit");
        let (bus, tag, loop_voice, channels) = {
            let v = app.audio.mixer_ref().voice(0).unwrap();
            (v.bus, v.tag, v.loop_voice, v.channels)
        };
        assert_eq!(
            bus.map(|id| app.audio.mixer_ref().bus_name(id).unwrap().to_string()),
            Some("amb".to_string()),
            "the world-bed leg rides the amb bus (block 72 routing, unchanged)"
        );
        assert!(loop_voice, "the PROVEN loop prop reached the voice");
        assert_eq!(channels, 2, "the 18-stream fold is the stereo unit");
        assert_eq!(
            app.ambience_unit_tag, tag,
            "the bed tag is tracked for the lifecycle"
        );

        // ---- 5. A THIRD request through the same host: the mission
        //         leg takes the channel back (the bed is cut) --------
        fre.call_global(SND_SET_MISSION, &[lua_str("e02m02")])
            .expect("the native is callable again");
        let reqs = fre.audio_seam_requests().expect("args are valid");
        assert_eq!(reqs.len(), 3);
        let out = app.apply_audio_seam(&reqs[2..], 1.0);
        assert_eq!(out.applied, 1);
        assert!(out.failures.is_empty(), "{out:?}");
        assert!(out.frames > 0, "the stem plays");
        assert_eq!(
            app.mission().phase(),
            crate::mission::MissionPhase::InMission
        );
        assert_eq!(app.mission().mission(), Some("e02m02"));
        assert!(!app.world_bed_active(), "the bed was cut by the serve");
        assert_eq!(app.audio.mixer_ref().len(), 1);
        let (bus, tag) = {
            let v = app.audio.mixer_ref().voice(0).unwrap();
            (v.bus, v.tag)
        };
        assert_eq!(
            bus.map(|id| app.audio.mixer_ref().bus_name(id).unwrap().to_string()),
            Some("music".to_string()),
            "the mission leg rides the music bus (block 72 routing, unchanged)"
        );
        assert_eq!(app.music_unit_tag, tag, "the music unit is tracked");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Block 108: the on-screen stick overlay — a rest-position ring hint
/// (bottom-left) always visible in-world, plus the live ring + knob
/// while a finger drives the stick.  Pure layout (no GPU here); takes
/// explicit fields because the world draw holds the renderer borrow.
fn stick_overlay(
    screen_px: [f32; 2],
    in_world: bool,
    active: Option<(u32, [f32; 2], [f32; 2])>,
) -> prototype_render::UiDrawList {
    let mut list = prototype_render::UiDrawList {
        screen: screen_px,
        ..Default::default()
    };
    if !in_world {
        return list;
    }
    let [w, h] = screen_px;
    let rest = [w * 0.20, h * 0.72];
    let ring = 96.0f32;
    // The rest-position hint ring (dim, dashed by gaps).
    for i in 0..24 {
        if i % 2 == 1 {
            continue;
        }
        let a = (i as f32) / 24.0 * std::f32::consts::TAU;
        let x = rest[0] + a.cos() * ring;
        let y = rest[1] + a.sin() * ring;
        list.rect(x - 4.0, y - 4.0, 8.0, 8.0, [0.6, 0.6, 0.6, 0.35]);
    }
    // The live stick: origin ring + knob at the clamped drag position.
    if let Some((_, origin, cur)) = active {
        for i in 0..24 {
            let a = (i as f32) / 24.0 * std::f32::consts::TAU;
            let x = origin[0] + a.cos() * ring;
            let y = origin[1] + a.sin() * ring;
            list.rect(x - 5.0, y - 5.0, 10.0, 10.0, [0.9, 0.9, 0.9, 0.8]);
        }
        let dx = (cur[0] - origin[0]).clamp(-ring, ring);
        let dy = (cur[1] - origin[1]).clamp(-ring, ring);
        list.rect(origin[0] + dx - 18.0, origin[1] + dy - 18.0, 36.0, 36.0, [0.55, 0.12, 0.10, 0.9]);
    }
    list
}

#[cfg(test)]
mod stick_tests {
    use super::*;

    /// Block 108: the stick's key mapping — dead zone neutral, up =
    /// forward (W), diagonals combine, release clears everything.
    #[test]
    fn stick_maps_the_walk_zone_to_wasd() {
        use prototype_input::KeyCode;
        let mut s = VirtualStick::new();
        s.begin(1, 100.0, 100.0);
        // Dead zone: barely moved.
        s.move_to(1, 105.0, 102.0);
        assert!(s.desired_keys().is_empty());
        // Up = forward.
        s.move_to(1, 100.0, 100.0 - 60.0);
        assert_eq!(s.desired_keys(), std::iter::once(KeyCode::W).collect());
        // Diagonal up-right.
        s.move_to(1, 100.0 + 55.0, 100.0 - 55.0);
        let want: std::collections::BTreeSet<_> = [KeyCode::W, KeyCode::D].into_iter().collect();
        assert_eq!(s.desired_keys(), want);
        // Left.
        s.move_to(1, 100.0 - 70.0, 100.0);
        assert_eq!(s.desired_keys(), std::iter::once(KeyCode::A).collect());
        // Release clears.
        s.end(1);
        assert!(s.desired_keys().is_empty());
        assert!(s.active.is_none());
    }

    /// A different finger id never moves or releases the capture.
    #[test]
    fn stick_ignores_other_fingers() {
        use prototype_input::KeyCode;
        let mut s = VirtualStick::new();
        s.begin(7, 100.0, 100.0);
        s.move_to(9, 500.0, 500.0);
        assert_eq!(s.vector, [0.0, 0.0]);
        s.end(9);
        s.move_to(7, 100.0, 100.0 - 80.0);
        assert_eq!(s.desired_keys(), std::iter::once(KeyCode::W).collect());
    }

    /// The reconcile emits KeyDown only on newly-desired keys (the
    /// second apply with the same vector is a no-op).
    #[test]
    fn reconcile_is_edge_triggered() {
        let mut s = VirtualStick::new();
        s.begin(1, 0.0, 0.0);
        s.move_to(1, 0.0, -70.0);
        let mut input = InputState::new();
        s.apply(&mut input);
        assert!(input.is_key_down(prototype_input::KeyCode::W));
        // Same vector again: the held set matches, no events fire.
        s.apply(&mut input);
        assert!(input.is_key_down(prototype_input::KeyCode::W));
        // Release.
        s.end(1);
        s.apply(&mut input);
        assert!(!input.is_key_down(prototype_input::KeyCode::W));
    }
}

//! prototype-android — the Android platform layer of prototype-rs.
//!
//! # Architecture (loader-style, cinnabar/horizon model)
//!
//! The APK ships **no game assets**.  On first launch the app:
//!
//! 1. Shows a first-launch screen saying **ALLOW ALL FILE ACCESS**; a tap
//!    opens Android's per-app all-files-access Settings page
//!    ([`permission`]).  `MANAGE_EXTERNAL_STORAGE` is declared in the APK
//!    manifest — declaring it alone grants nothing, hence the Settings hop.
//! 2. ONLY AFTER the grant does it create `/sdcard/Prototype Rust/`
//!    (= `/storage/emulated/0/Prototype Rust/`), via [`asset_folder`].
//! 3. A second screen tells the user to copy their OWN legally-owned files
//!    into that folder: `Prototype.exe`, `prototypeenginef.dll`, `*.rcf`,
//!    `*.p3d`, `scripts/`...
//! 4. On the NEXT app entry/resume, [`game_files`] discovers and orders
//!    everything the engine needs and the game boots AUTOMATICALLY — there
//!    is no Play button and no extra tap ([`app`], winit `resumed`).
//! 5. The status screens are landscape RGBA frames drawn through the
//!    wgpu fullscreen quad, with a GPU-independent `ANativeWindow`
//!    fallback ([`software_screen`]) so a broken Vulkan driver can never
//!    leave a black screen.  The engine itself renders through the wgpu
//!    edge in `prototype-render`.  See `activity` for NativeActivity.
//!
//! # Testability
//!
//! Everything except the thin NativeActivity glue is plain std filesystem
//! logic and is unit-tested on **every host** — the same tests run in CI on
//! Linux as part of `cargo test --workspace`.  Only `activity` is gated
//! behind `feature = "android-activity"` + `target_os = "android"`, because
//! the NativeActivity FFI cannot even be linked on a desktop host.
//!
//! # Safety
//!
//! The whole crate is `#![deny(unsafe_code)]` — DENY, not FORBID,
//! because the `activity` module (the single documented unsafe surface
//! of the workspace: Android native-glue FFI, it cannot be written
//! safely by definition) carries a module-level `allow` override.  Every
//! other module is unsafe-free in practice; any new unsafe use outside
//! `activity` fails CI.

#![deny(unsafe_code)]
#![deny(missing_docs)]

pub mod asset_folder;
pub mod audio_registry;
pub mod boot;
pub mod game_files;
#[cfg(feature = "audio-cpal")]
pub mod gameplay_audio;
pub mod member_stream;
pub mod meshes;
pub mod mission;
pub mod movie;
pub mod music_graph;
pub mod nis;
pub mod permission;
pub mod touch;

// The five pre-rendered landscape status screens (RGBA8 frames).  Only
// the shared winit App needs them; the desktop demo compiles the module
// too so `cargo test --workspace` exercises the pixel-size assertions.
#[cfg(feature = "winit-app")]
pub mod permission_ui;

// GPU-independent status-screen FALLBACK: writes those RGBA frames
// straight into the ANativeWindow when wgpu is unavailable.  On desktop
// it is a documented no-op stub so the shared App compiles unchanged.
#[cfg(feature = "winit-app")]
pub mod software_screen;

// The shared winit ApplicationHandler: gated by `winit-app` only (no
// target gate) — it compiles and is unit-tested on desktop hosts too,
// where the `prototype-demo` crate's `demo-render` bin drives it through
// winit's desktop backends.  See `app`'s module docs.
#[cfg(feature = "winit-app")]
pub mod app;

#[cfg(feature = "android-activity")]
#[cfg(target_os = "android")]
pub mod activity;

pub use asset_folder::{default_folder, ensure_folder, validate_folder, FolderError};
pub use audio_registry::{load_registry_from_archives, AUDIO_BOOTSTRAP_MEMBER};
pub use boot::{boot_summary, prepare_programs, BootReport, BootPlan, PreparedPair};
pub use game_files::{scan_folder, GameFileSet, ScanError};
pub use movie::{find_boot_movie, BootMovie, MovieAdvance, MoviePlayerState};
// Block 66: the mission music graph — cue → transition → stem →
// shard clip, decoded from the user's own audio banks.
pub use music_graph::{MusicGraph, MUSIC_ARCHIVE_NAME, MUSIC_BANK_MEMBERS};
// Block 68: the mission system — the gameplay state that owns the
// current mission id and turns it into music requests.
pub use mission::{MissionPhase, MissionSystem, MusicRequest};
// Block 55 (NEXT-UP (a)): the boot movie's audio device edge — behind
// `audio-cpal` like its implementation.
#[cfg(feature = "audio-cpal")]
pub use movie::{movie_audio_edge_should_close, MovieAudioEdge};
// Block 57: the GAMEPLAY audio device edge — the movie edge's
// persistent-lifetime counterpart over the AudioOut ring.
#[cfg(feature = "audio-cpal")]
pub use gameplay_audio::{gameplay_audio_edge_should_cut, GameplayAudioEdge};
pub use touch::{TouchMapper, TouchPhase, TouchPoint};
pub use permission::PermissionBridge;

#[cfg(feature = "winit-app")]
pub use app::{map_key, scan_or_setup, App, AppState};

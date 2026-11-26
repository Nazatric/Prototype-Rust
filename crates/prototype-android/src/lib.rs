//! prototype-android — the Android platform layer of prototype-rs.
//!
//! # Architecture (loader-style, cinnabar/horizon model)
//!
//! The APK ships **no game assets**.  On first launch the app:
//!
//! 1. Requests `MANAGE_EXTERNAL_STORAGE` (all-files-access) so it can read
//!    the user's own game files wherever they were copied.
//! 2. Creates `/sdcard/Prototype Rust/` (= `/storage/emulated/0/Prototype Rust/`).
//! 3. The user copies their OWN legally-owned files into that folder:
//!    `Prototype.exe`, `prototypeenginef.dll`, `*.rcf`, `*.p3d`, `scripts/`...
//! 4. [`asset_folder`] bootstraps and validates the folder.
//! 5. [`game_files`] discovers and orders everything the engine needs.
//! 6. The engine boots from the discovered set (see `activity` for the
//!    NativeActivity entry point; the render loop arrives with the wgpu
//!    integration in the `prototype-render` crate).
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
//! because two modules carry module-level `allow` overrides: `activity`
//! and (block 102) `jni_ui` — the two documented unsafe surfaces of the
//! workspace (Android native-glue FFI and JNI calls to the platform's
//! Java side; neither can be written safely by definition).  Every
//! other module is unsafe-free in practice; any new unsafe use outside
//! those two fails CI.

#![deny(unsafe_code)]
#![deny(missing_docs)]

pub mod archive_mmap;
pub mod asset_folder;
pub mod audio_registry;
pub mod boot;
// Block 117: the content-side line's android modules, ported as
// available building blocks (the app wiring is the next pass):
// the storage-permission flow, the touch-control plumbing and the
// in-game UI draw-list builders.  Their richer platform-default
// settings live in `display_settings` (main's persisted
// game-settings.json stays in `settings`).
pub mod display_settings;
pub mod permission;
pub mod touch_controls;
pub mod ui;

// Block 103: the boot journal + panic crash-reporter (host-portable).
pub mod boot_log;
pub mod game_files;
// Block 107: the REAL menu system (the engine DLL's `art\hud\*.gfx`
// screen catalog driven natively), its asset locator (proven name
// hashes) and the persisted settings.
pub mod menu;
pub mod menu_assets;
pub mod settings;
// Block 102: the JNI platform UI — Android-only (same gate as
// `activity`; everything host-portable lives in `platform_ui`).
#[cfg(feature = "android-activity")]
#[cfg(target_os = "android")]
pub mod jni_ui;
#[cfg(feature = "audio-cpal")]
pub mod gameplay_audio;
pub mod member_stream;
pub mod meshes;
pub mod mission;
pub mod movie;
pub mod music_graph;
pub mod nis;
pub mod platform_ui;
pub mod touch;

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
// Block 102: the platform UI surface (host-portable half; the JNI half
// is `jni_ui`, gated like `activity`).
pub use platform_ui::{
    status_clear_color, storage_plan, PlatformUi, StatusScreen, StoragePlan, StorageState,
};
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

#[cfg(feature = "winit-app")]
pub use app::{map_key, scan_or_setup, App, AppState};

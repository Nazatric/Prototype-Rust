//! Platform UI services the shared App handler can call on ANY host.
//!
//! Block 102 — the first real on-device launch (2026-11-24) exposed the
//! gap: the loader sat in [`AppState::Setup`] with NO window, NO storage
//! request and NO visible signal — the user saw a black screen.  The
//! loader contract needs three things the bare render loop cannot
//! express by itself:
//!
//! 1. a check + request for storage access (Android 11+'s all-files
//!    access, or the classic runtime dialog on 8-10 — see
//!    [`storage_plan`]);
//! 2. transient on-screen messages (toasts) that say exactly what to do
//!    while the app waits for the user's files;
//! 3. a STATUS COLOUR for the Setup clear-frame — the app has no text
//!    UI, so the screen itself is the signal (see [`status_clear_color`]:
//!    deep red = grant storage, amber = copy your game files).
//!
//! The shared `App` (app.rs) holds `Option<Box<dyn PlatformUi>>`: the
//! Android activity installs the JNI implementation ([`crate::jni_ui`]),
//! the desktop demo and the unit tests run with `None` or the log-only
//! stub below.  Everything in THIS module compiles on every host (and
//! with default features — no `winit-app` dependency); the JNI half does
//! not.

/// Which loader screen the frame belongs to — deliberately decoupled
/// from `AppState` so this module stays compilable with default
/// features (`app.rs` only exists behind `winit-app`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusScreen {
    /// The loader is waiting (folder / files / permission).
    Setup,
    /// The engine boot is running (block 103's phase machine — the CPU
    /// worker reads the archives off-thread; the screen shows the
    /// booting tint instead of a frozen window).
    Boot,
    /// The boot failed — the same red as storage-denied; the toast (and
    /// `boot-log.txt`) say what actually went wrong.
    BootFailed,
    /// Game files found and boot finished — the normal render owns the
    /// screen.
    Ready,
}

/// Whether the process may read the user's shared storage RIGHT NOW.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageState {
    /// Storage access granted (all-files access on Android 11+, or the
    /// classic WRITE permission granted on 8-10).
    Granted,
    /// Not granted.  The impl has (best-effort) fired the system UI that
    /// grants it — the grant usually lands after the user returns, and
    /// `App::resumed` re-runs the check on every entry.
    Denied,
}

/// The host-side UI surface the App talks to (Android: JNI toasts +
/// settings intents; every other host: logging only).
pub trait PlatformUi {
    /// Check storage access and, when missing, fire the system UI that
    /// grants it plus an explanatory toast.  Returns the state RIGHT
    /// NOW — the grant usually lands after the user returns from
    /// Settings, and `resumed` re-checks on every entry.
    fn ensure_storage(&mut self) -> StorageState;

    /// Show a short transient message (toast on Android; log elsewhere).
    fn toast(&self, message: &str);
}

/// Which system UI grants storage at a given API level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoragePlan {
    /// Already granted — no UI to fire.
    Granted,
    /// Android 11+ (API 30+): open the all-files-access settings page
    /// (`Environment.isExternalStorageManager()` is the gate).
    AllFilesSettings,
    /// Android 8-10 (API 26-29): the classic runtime permission dialog
    /// (`Activity.requestPermissions` with WRITE_EXTERNAL_STORAGE).
    RuntimeDialog,
}

/// The decision table for [`PlatformUi::ensure_storage`].
///
/// * API 30+: `WRITE_EXTERNAL_STORAGE` is dead (ignored by the OS) —
///   the only path is the all-files-access toggle.
/// * API 26-29: the runtime dialog is the path; the all-files page does
///   not exist yet.
/// * API < 26: unreachable (the manifest floor is 26); maps to
///   [`StoragePlan::RuntimeDialog`] as the least-wrong answer.
pub fn storage_plan(api_level: i32, all_files_granted: bool) -> StoragePlan {
    if api_level >= 30 {
        if all_files_granted {
            StoragePlan::Granted
        } else {
            StoragePlan::AllFilesSettings
        }
    } else {
        StoragePlan::RuntimeDialog
    }
}

/// The Setup screen's clear colour — the app has no text UI, so the
/// frame IS the message.  DESIGN values (ours, not evidence), chosen to
/// be unmistakable at any brightness:
///
/// * storage denied → deep red — action required in system settings;
/// * storage granted, no usable game files → amber — copy your files
///   into the asset folder;
/// * boot running → the PC loading screen's dark blue-grey (the
///   "something is happening" tint — not black, not an error);
/// * boot failed → the deep red again (the toast names the failure);
/// * a `Ready` screen returns `None` — the normal boot render owns the
///   screen from there.
pub fn status_clear_color(screen: StatusScreen, storage: StorageState) -> Option<[f64; 4]> {
    match screen {
        StatusScreen::Setup => Some(match storage {
            StorageState::Denied => [0.45, 0.10, 0.10, 1.0],
            StorageState::Granted => [0.62, 0.42, 0.08, 1.0],
        }),
        StatusScreen::Boot => Some([0.08, 0.09, 0.11, 1.0]),
        StatusScreen::BootFailed => Some([0.45, 0.10, 0.10, 1.0]),
        StatusScreen::Ready => None,
    }
}

/// Log-only implementation for hosts and tests — never requests
/// anything and never claims a grant it did not verify.
pub struct LogOnlyUi;

impl PlatformUi for LogOnlyUi {
    fn ensure_storage(&mut self) -> StorageState {
        log::info!("platform ui: storage check (log-only) — assuming granted");
        StorageState::Granted
    }

    fn toast(&self, message: &str) {
        log::info!("platform ui toast: {message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_plan_is_the_api_level_decision_table() {
        // Android 11+ gates on the all-files manager.
        assert_eq!(storage_plan(30, true), StoragePlan::Granted);
        assert_eq!(storage_plan(30, false), StoragePlan::AllFilesSettings);
        assert_eq!(storage_plan(34, false), StoragePlan::AllFilesSettings);
        // Android 8-10: the classic runtime dialog (all-files page does
        // not exist there, whatever the manager flag says).
        assert_eq!(storage_plan(29, false), StoragePlan::RuntimeDialog);
        assert_eq!(storage_plan(26, false), StoragePlan::RuntimeDialog);
        assert_eq!(storage_plan(26, true), StoragePlan::RuntimeDialog);
        // Below the manifest floor: least-wrong answer.
        assert_eq!(storage_plan(24, false), StoragePlan::RuntimeDialog);
    }

    #[test]
    fn status_clear_color_covers_setup_and_yields_to_ready() {
        let denied = status_clear_color(StatusScreen::Setup, StorageState::Denied);
        let granted = status_clear_color(StatusScreen::Setup, StorageState::Granted);
        assert_eq!(denied, Some([0.45, 0.10, 0.10, 1.0]), "red = go to Settings");
        assert_eq!(granted, Some([0.62, 0.42, 0.08, 1.0]), "amber = copy files");
        assert!(
            denied.unwrap()[0] > denied.unwrap()[1],
            "denied must read as RED (r dominates)"
        );
        assert!(
            granted.unwrap()[0] > granted.unwrap()[2] && granted.unwrap()[1] > granted.unwrap()[2],
            "waiting must read as AMBER (r,g dominate b)"
        );
        // The boot tint and the boot-failure red are the phase
        // machine's states (block 103); Ready owns the frame.
        assert!(status_clear_color(StatusScreen::Boot, StorageState::Granted).is_some());
        assert_eq!(
            status_clear_color(StatusScreen::BootFailed, StorageState::Granted),
            status_clear_color(StatusScreen::Setup, StorageState::Denied),
            "boot failure reads as the same red"
        );
        assert_eq!(status_clear_color(StatusScreen::Ready, StorageState::Granted), None);
    }
}

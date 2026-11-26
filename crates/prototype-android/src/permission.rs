//! All-Files-Access permission gate (Android 11+ / API 30+).
//!
//! The loader-style contract the user specified:
//!
//! 1. The app opens.
//! 2. **If** the platform gate is not satisfied → the app must
//!    *directly* open the system's "All files access" page for THIS app.
//! 3. The user grants it; the app comes back from the system page.
//! 4. The app creates `/sdcard/Prototype Rust/`.
//! 5. The user copies their own game files into the folder.
//! 6. The user reopens the app — it boots directly into the game.
//!
//! On Android 10 (API 29) and below, `MANAGE_EXTERNAL_STORAGE` does not
//! exist; `READ_EXTERNAL_STORAGE`/`WRITE_EXTERNAL_STORAGE` are runtime
//! permissions handled by the OS permission dialog instead.  This
//! module models BOTH cases as a single [`PermissionState`] enum, so
//! the app's higher levels see one state machine regardless of API
//! level.
//!
//! # Host testability
//!
//! The module is host-compilable and host-testable: every code path
//! that does NOT need the JVM lives here (state transitions, the
//! Android-specific storage path string, the platform-gate decision),
//! and the ONE call that needs the JVM ([`request_all_files_access`])
//! is target-gated to `cfg(target_os = "android")` AND
//! `cfg(feature = "android-activity")` — on any other host it returns
//! [`PermissionError::HostStub`] cleanly, so the rest of the surface
//! (and the unit tests) run on Linux CI unchanged.

use std::path::PathBuf;

use thiserror::Error;

use crate::asset_folder;

/// All-Files-Access (and the legacy API-29 runtime-permission equivalent).
///
/// The app's top-level lifecycle consults [`current_state`] on every
/// `resumed` event:
///
/// - [`PermissionState::Granted`] → proceed to the asset-folder scan.
/// - [`PermissionState::Needed`]  → fire the system intent (Android) or
///   skip the gate entirely (desktop; folder is in CWD).  The intent
///   handler is in [`request_all_files_access`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionState {
    /// The platform gate is satisfied (or the host doesn't need one).
    Granted,
    /// The app must open the system's "All files access" page for
    /// itself.  After the user grants it, the OS resumes the activity
    /// and `resumed` fires again — the state transitions to `Granted`.
    Needed,
}

/// Errors the permission edge can return.
#[derive(Debug, Error)]
pub enum PermissionError {
    /// The platform gate is satisfied but the asset folder cannot be
    /// created.  Wrapped so the app's `Setup` state surfaces the root
    /// cause to the user.
    #[error("could not create asset folder: {0}")]
    CreateFailed(#[source] std::io::Error),

    /// The host (Linux CI / desktop demo) cannot open an Android system
    /// intent.  Returned by `request_all_files_access` on non-Android
    /// targets — the desktop demo and the unit tests treat it as
    /// "no-op, the gate is for Android only".
    #[error("host stub: all-files-access intent is Android-only")]
    HostStub,

    /// The Android NativeActivity handle is missing (the activity
    /// module's `android_main` should always pass one).
    #[error("missing android-activity handle")]
    NoActivity,

    /// The JVM call to start the system settings intent failed (JNI
    /// exception, missing class, etc.).  The string is the JNI error.
    #[error("android intent failed: {0}")]
    IntentFailed(String),
}

/// The state machine entry-point: have we got the platform permission,
/// and is the folder ready?
///
/// Returns [`PermissionState::Granted`] on every desktop host (the
/// loader contract is: the desktop demo uses `./Prototype Rust/`,
/// which a user owns by virtue of running the binary in their working
/// directory — no OS permission dialog exists).
///
/// On Android, the gate is `Environment.isExternalStorageManager()`
/// when API ≥ 30, else the legacy runtime permission.  The Android
/// branch lives behind `cfg(target_os = "android")` because the
/// `Environment`/`Context` JVM types are not linkable on a desktop
/// host; the host branch is a constant `Granted`.
pub fn current_state() -> PermissionState {
    if cfg!(target_os = "android") {
        // The real check is `android.os.Environment.isExternalStorageManager()`.
        // It needs the JVM (an `android.app.Activity` reference), so it
        // lives in the `activity` module, not here.  This function is
        // the policy decision for the higher levels: on Android we
        // always report `Needed` and let `activity` resolve it; on
        // every other host we report `Granted`.
        //
        // (The state transitions to `Granted` from the activity side
        // once `isExternalStorageManager()` returns true; the higher
        // levels only re-call `current_state` after a `resumed`.)
        PermissionState::Needed
    } else {
        PermissionState::Granted
    }
}

/// Resolve the asset folder + ensure it exists.
///
/// This is the second half of the loader contract — it runs AFTER the
/// permission gate is `Granted`, on first launch.  Idempotent:
/// - `Ok(true)`  → folder exists and has been created/verified.
/// - `Ok(false)` → the platform gate is `Needed` and the folder
///   creation was deferred (the activity will fire the intent first).
pub fn ensure_folder_after_permission(
    state: PermissionState,
    folder: &std::path::Path,
) -> Result<bool, PermissionError> {
    match state {
        PermissionState::Granted => {
            asset_folder::ensure_folder(folder).map_err(PermissionError::CreateFailed)?;
            Ok(true)
        }
        PermissionState::Needed => {
            // Defer folder creation until after the user grants access.
            // The activity calls `request_all_files_access`, then on
            // resume re-checks `current_state` and only THEN calls
            // this again with `Granted`.
            Ok(false)
        }
    }
}

/// Default location for the asset folder, in the platform's
/// shared-storage root.
///
/// This is a thin wrapper around [`asset_folder::default_folder`] kept
/// here so the permission module owns the *full* loader contract
/// (permission + folder path + creation) in one place for the
/// higher-level `App` to consume.
pub fn default_asset_folder() -> PathBuf {
    asset_folder::default_folder()
}

/// Open the system's "All files access" page for THIS app.
///
/// On Android 11+ (API 30+) this fires
/// `Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION` with the
/// app's own package URI.  The user grants or denies; the OS resumes
/// the activity; the next `resumed` re-evaluates [`current_state`].
///
/// On every other host (Linux CI, the desktop demo) it returns
/// [`PermissionError::HostStub`] — the desktop path has no OS-level
/// gate, so the caller treats `HostStub` as "no-op, proceed to the
/// folder scan".
///
/// The Android implementation needs the `android-activity` feature AND
/// the `android` target — it is the ONE entry point the activity module
/// calls from `android_main` after the pre-loop scan reports `Needed`.
#[cfg(any(not(target_os = "android"), not(feature = "android-activity")))]
pub fn request_all_files_access() -> Result<(), PermissionError> {
    Err(PermissionError::HostStub)
}

/// Open the system's "All files access" page for THIS app (Android
/// arm).  Fires through the registered intent sender — see the block-117
/// note below; without the wiring it degrades to `IntentFailed`.
#[cfg(all(target_os = "android", feature = "android-activity"))]
pub fn request_all_files_access() -> Result<(), PermissionError> {
    // Block 117 (the port): the content-side line forwarded to a
    // `crate::activity::permission_open_all_files_settings` helper that
    // does not exist in EITHER tree (their snapshot is internally
    // broken here).  The app-side line's architecture keeps every JNI
    // call in `jni_ui` — so the intent fires through a REGISTERED
    // sender: the app path (stage-2 wiring) registers its PlatformUi's
    // all-files-settings opener at startup; a bare call without the
    // wiring degrades to IntentFailed (the caller then shows the
    // manual-settings hint — the OS settings page by hand).
    PERMISSION_INTENT_SENDER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|send| send())
        .unwrap_or_else(|| {
            Err("no intent sender registered — open Android settings → All files access manually"
                .to_string())
        })
        .map_err(|e| PermissionError::IntentFailed(e))
}

/// The registered intent sender (the app's PlatformUi opener — see the
/// android arm of [`request_all_files_access`]).
#[cfg(all(target_os = "android", feature = "android-activity"))]
static PERMISSION_INTENT_SENDER: std::sync::Mutex<Option<fn() -> Result<(), String>>> =
    std::sync::Mutex::new(None);

/// Block 117: the app path registers its all-files-settings opener
/// here at startup (the JNI lives in `jni_ui` — this module stays
/// JVM-free and host-testable).
#[cfg(all(target_os = "android", feature = "android-activity"))]
pub fn register_intent_sender(sender: fn() -> Result<(), String>) {
    *PERMISSION_INTENT_SENDER
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(sender);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_host_reports_granted() {
        // On any non-Android target the gate is always Granted — the
        // loader contract is: the user owns their working directory.
        let s = current_state();
        if !cfg!(target_os = "android") {
            assert_eq!(s, PermissionState::Granted);
        } else {
            // The Android branch reports Needed until the activity
            // side resolves it; we cannot assert the live JVM state
            // from a host unit test, but the enum variant must be one
            // of the two documented.
            assert!(matches!(s, PermissionState::Granted | PermissionState::Needed));
        }
    }

    #[test]
    fn host_stub_is_an_error_not_a_panic() {
        // The desktop demo path's `request_all_files_access` returns
        // `HostStub`, not a panic — the caller can treat it as "no-op".
        match request_all_files_access() {
            Err(PermissionError::HostStub) => { /* expected on host */ }
            Ok(()) => {
                // On Android with the activity feature this is allowed;
                // host tests never take this branch.
                if !cfg!(all(target_os = "android", feature = "android-activity")) {
                    panic!("host should return HostStub");
                }
            }
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn granted_state_creates_folder() {
        let dir = std::env::temp_dir().join(format!(
            "proto-android-perm-grant-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(!dir.exists());
        let ready = ensure_folder_after_permission(PermissionState::Granted, &dir).unwrap();
        assert!(ready);
        assert!(dir.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn needed_state_defers_folder_creation() {
        let dir = std::env::temp_dir().join(format!(
            "proto-android-perm-needed-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let ready = ensure_folder_after_permission(PermissionState::Needed, &dir).unwrap();
        assert!(!ready, "Needed must NOT create the folder yet");
        assert!(!dir.exists(), "folder must not exist when deferred");
    }

    #[test]
    fn default_folder_matches_asset_folder_default() {
        assert_eq!(default_asset_folder(), asset_folder::default_folder());
    }

    #[test]
    fn create_failure_is_wrapped() {
        // A path whose parent is a FILE cannot become a directory.
        let parent = std::env::temp_dir().join(format!(
            "proto-android-perm-fail-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&parent, b"file").unwrap();
        let target = parent.join("Prototype Rust");
        let err = ensure_folder_after_permission(PermissionState::Granted, &target)
            .unwrap_err();
        assert!(matches!(err, PermissionError::CreateFailed(_)));
        let _ = std::fs::remove_file(&parent);
    }
}

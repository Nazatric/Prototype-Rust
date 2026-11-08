//! Asset-folder bootstrap: create and validate `/sdcard/Prototype Rust/`.
//!
//! The loader-style model (cinnabar/horizon) puts the user's own game files
//! in one well-known folder.  This module owns everything about that folder:
//! its name, its default location per platform, creation, and validation.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// The folder name the APK creates on the device's shared storage.
///
/// Exactly `Prototype Rust` — with a space — matching the user-stated spec.
pub const FOLDER_NAME: &str = "Prototype Rust";

/// Default location of the asset folder on the current platform.
///
/// * Android: `/storage/emulated/0/Prototype Rust` (same file as the
///   user-visible `/sdcard/Prototype Rust`).
/// * Desktop (tests / development): `./Prototype Rust` relative to the
///   current directory, so a developer can lay out a folder identically.
pub fn default_folder() -> PathBuf {
    if cfg!(target_os = "android") {
        PathBuf::from("/storage/emulated/0").join(FOLDER_NAME)
    } else {
        PathBuf::from(".").join(FOLDER_NAME)
    }
}

/// Create the asset folder (and any missing parents).  Idempotent.
pub fn ensure_folder(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

/// Everything that can go wrong while validating an asset folder.
#[derive(Debug, Error)]
pub enum FolderError {
    /// The folder does not exist (user has not copied anything yet).
    #[error("asset folder does not exist: {0}")]
    Missing(PathBuf),
    /// The path exists but is not a directory.
    #[error("asset path is not a directory: {0}")]
    NotADirectory(PathBuf),
    /// The folder exists but is completely empty.
    #[error("asset folder is empty: {0}")]
    Empty(PathBuf),
    /// The folder has files but no `.rcf` archive — the minimum required
    /// input for booting the engine.
    #[error("asset folder contains no .rcf archives: {0}")]
    NoArchives(PathBuf),
    /// The folder exists but cannot be read (permissions, storage removed...).
    #[error("asset folder cannot be read: {0}: {1}")]
    Unreadable(PathBuf, #[source] io::Error),
    /// Creating the folder failed (permissions, storage full, SAF denial...).
    #[error("asset folder could not be created: {1}")]
    CreateFailed(#[source] io::Error, PathBuf),
}

/// Validate that `path` is a usable asset folder.
///
/// Checks, in order:
/// 1. the path exists,
/// 2. it is a directory,
/// 3. it contains at least one entry,
/// 4. it contains at least one `.rcf` archive (case-insensitive).
///
/// Note step 4 inspects only the top level — exactly where the user is told
/// to put the archives.  Deeper layout is [`crate::game_files::scan_folder`]'s
/// business.
pub fn validate_folder(path: &Path) -> Result<(), FolderError> {
    if !path.exists() {
        return Err(FolderError::Missing(path.to_path_buf()));
    }
    if !path.is_dir() {
        return Err(FolderError::NotADirectory(path.to_path_buf()));
    }
    let entries = fs::read_dir(path)
        .map_err(|e| FolderError::Unreadable(path.to_path_buf(), e))?;

    let mut any_entry = false;
    for entry in entries {
        let entry = entry.map_err(|e| FolderError::Unreadable(path.to_path_buf(), e))?;
        any_entry = true;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.to_ascii_lowercase().ends_with(".rcf") {
            return Ok(());
        }
    }

    if !any_entry {
        Err(FolderError::Empty(path.to_path_buf()))
    } else {
        Err(FolderError::NoArchives(path.to_path_buf()))
    }
}

/// One-time first-launch bootstrap: create the folder if missing and report
/// whether it is ready to boot from.
///
/// Returns `Ok(true)` when the folder is bootable, `Ok(false)` when it was
/// just created and still needs the user to copy their files, and the
/// underlying error if creation itself failed.
pub fn bootstrap(path: &Path) -> Result<bool, FolderError> {
    match validate_folder(path) {
        Ok(()) => Ok(true),
        Err(FolderError::Empty(_) | FolderError::NoArchives(_)) => Ok(false),
        Err(FolderError::Missing(_)) => {
            ensure_folder(path)
                .map_err(|e| FolderError::CreateFailed(e, path.to_path_buf()))?;
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Fresh temp dir helper (std-only, no external dev-dependency).
    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "proto-android-asset-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn folder_name_is_exact() {
        assert_eq!(FOLDER_NAME, "Prototype Rust");
        assert!(FOLDER_NAME.contains(' '));
    }

    #[test]
    fn default_folder_is_sdcard_on_android_else_local() {
        let f = default_folder();
        if cfg!(target_os = "android") {
            assert_eq!(f, PathBuf::from("/storage/emulated/0/Prototype Rust"));
        } else {
            assert_eq!(f, PathBuf::from("./Prototype Rust"));
        }
    }

    #[test]
    fn ensure_folder_creates_nested_and_is_idempotent() {
        let root = temp_root("ensure");
        let path = root.join("deep/nested/Prototype Rust");
        ensure_folder(&path).unwrap();
        assert!(path.is_dir());
        // second call must not fail
        ensure_folder(&path).unwrap();
    }

    #[test]
    fn validate_missing_folder_reports_missing() {
        let root = temp_root("missing");
        let err = validate_folder(&root.join("nope")).unwrap_err();
        assert!(matches!(err, FolderError::Missing(_)));
    }

    #[test]
    fn validate_empty_folder_reports_empty() {
        let root = temp_root("empty");
        let err = validate_folder(&root).unwrap_err();
        assert!(matches!(err, FolderError::Empty(_)));
    }

    #[test]
    fn validate_folder_without_rcf_reports_no_archives() {
        let root = temp_root("norcf");
        fs::write(root.join("readme.txt"), b"hello").unwrap();
        let err = validate_folder(&root).unwrap_err();
        assert!(matches!(err, FolderError::NoArchives(_)));
    }

    #[test]
    fn validate_folder_accepts_rcf_case_insensitively() {
        for name in ["00cells.rcf", "01CELLS.RCF", "Art.Rcf"] {
            let root = temp_root("rcf-case");
            fs::write(root.join(name), b"RADIANCMENT").unwrap();
            assert!(validate_folder(&root).is_ok(), "{name} should validate");
        }
    }

    #[test]
    fn bootstrap_on_missing_folder_creates_it_and_reports_not_ready() {
        let root = temp_root("bootstrap");
        let path = root.join("Prototype Rust");
        assert!(!path.exists());
        let ready = bootstrap(&path).unwrap();
        assert!(!ready, "freshly created folder cannot be bootable");
        assert!(path.is_dir());
    }

    #[test]
    fn bootstrap_on_ready_folder_reports_ready() {
        let root = temp_root("bootstrap-ready");
        fs::write(root.join("00art.rcf"), b"x").unwrap();
        assert!(bootstrap(&root).unwrap());
    }

    #[test]
    fn bootstrap_on_partial_folder_reports_not_ready_but_keeps_files() {
        let root = temp_root("bootstrap-partial");
        fs::write(root.join("notes.txt"), b"x").unwrap();
        assert!(!bootstrap(&root).unwrap());
        assert!(root.join("notes.txt").exists(), "must not wipe user files");
    }

    #[test]
    fn bootstrap_create_failure_is_wrapped_as_create_failed() {
        // A path whose *parent* is a file cannot become a directory.
        let root = temp_root("bootstrap-fail");
        fs::write(root.join("blocker"), b"file").unwrap();
        let path = root.join("blocker/Prototype Rust");
        match bootstrap(&path) {
            Err(FolderError::CreateFailed(_, p)) => assert_eq!(p, path),
            other => panic!("expected CreateFailed, got {other:?}"),
        }
    }
}

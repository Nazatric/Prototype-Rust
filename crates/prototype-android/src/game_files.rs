//! Game-file discovery inside the asset folder.
//!
//! Scans `Prototype Rust/` and classifies everything the engine can use:
//! the launcher exe, the engine DLL, the `.rcf` archives (ordered by the
//! **PROVEN canonical PC mount order**), loose `.p3d` files, and scripts.
//!
//! # Evidence (PROVEN against the real PC install)
//!
//! The canonical mount order of the five shipping archives on the PC disc /
//! Steam build was recovered empirically in the earlier autonomous run by
//! boot-tracing which archive the engine opens first (see PROGRESS.md
//! "Formats" and `prototype-boot`'s timeline test).  It is:
//!
//! ```text
//! 00cells.rcf -> 01cells.rcf -> 00art.rcf -> 01art.rcf -> 02art.rcf
//! ```
//!
//! `scan_folder` therefore always emits known archives in that order,
//! regardless of how the user's file manager laid them down on disk, and
//! appends any *unknown* `.rcf` (mods, language packs) alphabetically after
//! them — mirroring what a load-order list should do.

use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// The five canonical PC archives in their PROVEN mount order.
pub const CANONICAL_PC_ARCHIVES: [&str; 5] = [
    "00cells.rcf",
    "01cells.rcf",
    "00art.rcf",
    "01art.rcf",
    "02art.rcf",
];

/// Block 105: `true` when an archive can hold WORLD data (cells,
/// placements, drawables, geometry) and therefore belongs in the boot
/// index; `false` for the pure sound banks.  The block-104 device log
/// measured the cost of not asking this question: 85% of the indexed
/// members came from `00audio.rcf` + `01audio.rcf` + the four `.woi`
/// wave-bank archives — none of which can ever hold a placement.
///
/// Rule (conservative, stem-based): an archive is SKIPPED only when its
/// file stem contains `audio` or `woi` (case-insensitive).  Everything
/// else — cells, art, unknown archives, mods — is indexed, because the
/// user's folder is NOT the canonical five and being clever here loses
/// real world data (the honest cost of one unskipped archive is minutes
/// in the cached build; the cost of a skipped cells archive is an
/// empty world).
pub fn index_worthy_archive(stem: &str) -> bool {
    let s = stem.to_ascii_lowercase();
    !(s.contains("audio") || s.contains("woi"))
}

#[cfg(test)]
mod index_skip_tests {
    use super::*;

    /// Block 105: the sound banks leave the index; everything else —
    /// including unknown archives and mods — stays in.
    #[test]
    fn sound_banks_leave_the_index() {
        assert!(index_worthy_archive("00cells"));
        assert!(index_worthy_archive("art"));
        assert!(index_worthy_archive("frontend"));
        assert!(index_worthy_archive("cellsarchive"));
        assert!(index_worthy_archive("Text_english"));
        assert!(!index_worthy_archive("00audio"));
        assert!(!index_worthy_archive("01AUDIO"));
        assert!(!index_worthy_archive("00woi"));
        assert!(!index_worthy_archive("03Woi"));
        assert!(!index_worthy_archive("my_music_audio_pack"));
    }
}

/// Everything discovered in one asset-folder scan.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GameFileSet {
    /// `Prototype.exe` / `prototypef.exe` if present (used for version sniffing).
    pub exe: Option<PathBuf>,
    /// The engine DLL — `prototypeenginef.dll` preferred over any other
    /// `prototypeengine*.dll`.
    pub dll: Option<PathBuf>,
    /// All `*.rcf` archives: the five canonical ones first (mount order),
    /// then unknown ones alphabetically.
    pub rcf_archives: Vec<PathBuf>,
    /// Loose `*.p3d` files (top level only).
    pub p3d_files: Vec<PathBuf>,
    /// Loose `*.lua` files (top level only).
    pub lua_files: Vec<PathBuf>,
    /// Anything else — kept so "what did the user bring" is always answerable.
    pub other: Vec<PathBuf>,
}

/// Scan failures.
#[derive(Debug, Error)]
pub enum ScanError {
    /// The root does not exist.
    #[error("asset folder does not exist: {0}")]
    Missing(PathBuf),
    /// The root is not a directory.
    #[error("asset folder is not a directory: {0}")]
    NotADirectory(PathBuf),
    /// The directory could not be listed.
    #[error("asset folder could not be listed: {0}: {1}")]
    Unreadable(PathBuf, #[source] std::io::Error),
}

/// True when the lowercase file name ends with `ext` (ext must be lowercase,
/// with or without the leading dot — normalised internally).
fn has_ext(name: &str, ext: &str) -> bool {
    let ext = ext.trim_start_matches('.');
    name.to_ascii_lowercase()
        .ends_with(&format!(".{ext}"))
}

/// Rank of a known RCF in the canonical mount order, or `None` if unknown.
fn canonical_rank(file_name: &str) -> Option<usize> {
    CANONICAL_PC_ARCHIVES
        .iter()
        .position(|a| a.eq_ignore_ascii_case(file_name))
}

/// Is this the preferred engine DLL name?
fn is_preferred_dll(file_name: &str) -> bool {
    file_name.eq_ignore_ascii_case("prototypeenginef.dll")
}

/// Is this *an* engine DLL at all (`prototypeengine*.dll`)?
fn is_engine_dll(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    lower.starts_with("prototypeengine") && lower.ends_with(".dll")
}

/// Scan `root` (top level only — subdirectories are intentionally skipped:
/// the engine's own loader only consults the install root and `scripts/`,
/// and `scripts/` lua is picked up by `prototype-boot`, not by this scan).
pub fn scan_folder(root: &Path) -> Result<GameFileSet, ScanError> {
    if !root.exists() {
        return Err(ScanError::Missing(root.to_path_buf()));
    }
    if !root.is_dir() {
        return Err(ScanError::NotADirectory(root.to_path_buf()));
    }
    let entries = fs::read_dir(root)
        .map_err(|e| ScanError::Unreadable(root.to_path_buf(), e))?;

    let mut set = GameFileSet::default();
    let mut known_rcf: Vec<Option<PathBuf>> = vec![None; CANONICAL_PC_ARCHIVES.len()];
    let mut unknown_rcf: Vec<PathBuf> = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|e| ScanError::Unreadable(root.to_path_buf(), e))?;
        let path = entry.path();
        if !path.is_file() {
            continue; // subdirectories skipped by design
        }
        let name = entry.file_name();
        let name = name.to_string_lossy().to_string();

        if has_ext(&name, "rcf") {
            match canonical_rank(&name) {
                Some(rank) => known_rcf[rank] = Some(path),
                None => unknown_rcf.push(path),
            }
        } else if has_ext(&name, "p3d") {
            set.p3d_files.push(path);
        } else if has_ext(&name, "lua") {
            set.lua_files.push(path);
        } else if has_ext(&name, "exe") {
            if set.exe.is_none() {
                set.exe = Some(path);
            }
        } else if is_preferred_dll(&name) {
            // Preferred engine DLL always wins, even if a generic one was seen first.
            set.dll = Some(path);
        } else if is_engine_dll(&name) && set.dll.is_none() {
            set.dll = Some(path);
        } else {
            set.other.push(path);
        }
    }

    set.rcf_archives = known_rcf.into_iter().flatten().collect();
    unknown_rcf.sort(); // deterministic: alphabetical by full path
    set.rcf_archives.extend(unknown_rcf);
    Ok(set)
}

/// A set is bootable when it has at least one archive — the loader model
/// keeps everything in `.rcf`s, so one archive is enough to start.
pub fn is_bootable(set: &GameFileSet) -> bool {
    !set.rcf_archives.is_empty()
}

/// Human-readable summary of what is present / missing, for the first-launch
/// UI ("Waiting for game files: ...") and for CI logs.
pub fn missing_summary(set: &GameFileSet) -> String {
    let mut missing: Vec<&str> = Vec::new();
    let have: Vec<String> = set
        .rcf_archives
        .iter()
        .map(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        })
        .collect();
    for archive in CANONICAL_PC_ARCHIVES {
        let found = have
            .iter()
            .any(|h| h.eq_ignore_ascii_case(archive));
        if !found {
            missing.push(archive);
        }
    }
    if set.exe.is_none() {
        missing.push("Prototype.exe");
    }
    if set.dll.is_none() {
        missing.push("prototypeenginef.dll");
    }
    if missing.is_empty() {
        format!(
            "ready: {} archive(s) mounted in canonical order",
            set.rcf_archives.len()
        )
    } else {
        format!(
            "missing {} item(s): {} ({} archive(s) found)",
            missing.len(),
            missing.join(", "),
            set.rcf_archives.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "proto-android-files-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(root: &Path, name: &str) -> PathBuf {
        let p = root.join(name);
        fs::write(&p, b"fixture").unwrap();
        p
    }

    #[test]
    fn canonical_order_constant_is_stable() {
        assert_eq!(
            CANONICAL_PC_ARCHIVES,
            [
                "00cells.rcf",
                "01cells.rcf",
                "00art.rcf",
                "01art.rcf",
                "02art.rcf"
            ]
        );
    }

    #[test]
    fn scan_orders_canonical_rcfs_over_disk_order() {
        let root = temp_root("order");
        // Write the five archives in REVERSE disk order on purpose.
        for name in CANONICAL_PC_ARCHIVES.iter().rev() {
            touch(&root, name);
        }
        let set = scan_folder(&root).unwrap();
        let names: Vec<String> = set
            .rcf_archives
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, CANONICAL_PC_ARCHIVES.to_vec());
    }

    #[test]
    fn scan_appends_unknown_rcfs_after_canonical_ones_alphabetically() {
        let root = temp_root("unknown");
        touch(&root, "zz_mods.rcf");
        touch(&root, "00cells.rcf");
        touch(&root, "aa_lang.rcf");
        let set = scan_folder(&root).unwrap();
        let names: Vec<String> = set
            .rcf_archives
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["00cells.rcf", "aa_lang.rcf", "zz_mods.rcf"]);
    }

    #[test]
    fn scan_recognises_uppercase_extensions() {
        let root = temp_root("case");
        touch(&root, "00CELLS.RCF");
        touch(&root, "PROTOTYPE.EXE");
        let set = scan_folder(&root).unwrap();
        assert_eq!(set.rcf_archives.len(), 1);
        assert!(set.exe.is_some());
    }

    #[test]
    fn scan_prefers_prototypeenginef_dll_over_generic_engine_dll() {
        let root = temp_root("dll");
        touch(&root, "prototypeengine.dll"); // generic, seen FIRST on disk
        touch(&root, "prototypeenginef.dll"); // preferred, seen SECOND
        let set = scan_folder(&root).unwrap();
        let dll = set.dll.unwrap();
        assert_eq!(dll.file_name().unwrap().to_string_lossy(), "prototypeenginef.dll");
    }

    #[test]
    fn scan_falls_back_to_generic_engine_dll_when_f_missing() {
        let root = temp_root("dll-fallback");
        touch(&root, "prototypeengine64.dll");
        let set = scan_folder(&root).unwrap();
        assert!(set.dll.is_some());
    }

    #[test]
    fn scan_ignores_non_engine_dlls() {
        let root = temp_root("dll-other");
        touch(&root, "d3d9.dll");
        let set = scan_folder(&root).unwrap();
        assert!(set.dll.is_none());
        assert_eq!(set.other.len(), 1);
    }

    #[test]
    fn scan_skips_subdirectories() {
        let root = temp_root("subdirs");
        fs::create_dir_all(root.join("backup")).unwrap();
        touch(&root.join("backup"), "00cells.rcf");
        let set = scan_folder(&root).unwrap();
        assert!(set.rcf_archives.is_empty(), "top-level scan must not descend");
    }

    #[test]
    fn scan_classifies_p3d_lua_and_extras() {
        let root = temp_root("classify");
        touch(&root, "boot_strap.p3d");
        touch(&root, "main.lua");
        touch(&root, "readme.txt");
        let set = scan_folder(&root).unwrap();
        assert_eq!(set.p3d_files.len(), 1);
        assert_eq!(set.lua_files.len(), 1);
        assert_eq!(set.other.len(), 1);
    }

    #[test]
    fn scan_missing_root_errors() {
        let root = temp_root("scan-missing");
        let err = scan_folder(&root.join("nope")).unwrap_err();
        assert!(matches!(err, ScanError::Missing(_)));
    }

    #[test]
    fn scan_file_as_root_errors() {
        let root = temp_root("scan-file");
        let f = touch(&root, "afile.txt");
        let err = scan_folder(&f).unwrap_err();
        assert!(matches!(err, ScanError::NotADirectory(_)));
    }

    #[test]
    fn bootable_requires_one_archive() {
        let mut set = GameFileSet::default();
        assert!(!is_bootable(&set));
        set.rcf_archives.push(PathBuf::from("00art.rcf"));
        assert!(is_bootable(&set));
    }

    #[test]
    fn missing_summary_lists_what_is_absent() {
        let mut set = GameFileSet::default();
        let s = missing_summary(&set);
        assert!(s.contains("00cells.rcf"));
        assert!(s.contains("Prototype.exe"));
        assert!(s.contains("prototypeenginef.dll"));

        // Partial: one archive present -> still missing the other four.
        set.rcf_archives.push(PathBuf::from("00cells.rcf"));
        let s = missing_summary(&set);
        assert!(s.contains("01cells.rcf"), "got: {s}");
        assert!(s.contains("1 archive(s)"), "got: {s}");

        // Complete: all five canonical archives + exe + dll -> ready.
        for name in CANONICAL_PC_ARCHIVES.iter().skip(1) {
            set.rcf_archives.push(PathBuf::from(name));
        }
        set.exe = Some(PathBuf::from("Prototype.exe"));
        set.dll = Some(PathBuf::from("prototypeenginef.dll"));
        let s = missing_summary(&set);
        assert!(s.contains("ready"), "got: {s}");
        assert!(s.contains("5 archive(s)"), "got: {s}");
    }

    #[test]
    fn full_simulated_install_is_ready() {
        let root = temp_root("full");
        for name in CANONICAL_PC_ARCHIVES {
            touch(&root, name);
        }
        touch(&root, "Prototype.exe");
        touch(&root, "prototypeenginef.dll");
        let set = scan_folder(&root).unwrap();
        assert!(is_bootable(&set));
        assert!(missing_summary(&set).starts_with("ready"));
        // And the mount order survived the file-manager's alphabetical listing:
        let names: Vec<String> = set
            .rcf_archives
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, CANONICAL_PC_ARCHIVES.to_vec());
    }
}

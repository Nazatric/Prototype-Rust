//! Block 107 — the REAL menu asset locator.  Block 120: STREAMING.
//!
//! The game's menus are Scaleform GFx members (`art\hud\*.gfx`) living
//! inside the art archives.  The engine looks members up by
//! `name_hash` — and the hash algorithm is **PROVEN**
//! (`prototype_rcf::hash_name`).  So without the user's corpus in this
//! sandbox we can still:
//!
//! 1. compute the exact member hash of every real menu screen,
//! 2. at boot, CENSUS which of the 25 real `.gfx` screens exist in
//!    the user's archives (the worker journals it into boot-log.txt),
//! 3. EXTRACT the ones we render (block 120: the members' bytes come
//!    out through the PROVEN span reader — one seek + one member
//!    decompress per hit).
//!
//! # Block 120 — the whole-archive read is GONE
//!
//! The block-107 census did `std::fs::read` on EVERY archive to parse
//! its directory: on the user's phone that pulled art.rcf (576 MB),
//! cells.rcf (632 MB) and movies.rcf (981 MB) into RAM **on every
//! boot** — a ~1 GB transient allocation under which Android stalls
//! everything (the "isn't responding" report's second suspect).  The
//! directory is a few KB and `parse_directory` reads it from a File
//! handle — the census now streams, and so does the extraction
//! (`read_member_payload` touches only the two members we render).

use std::collections::HashMap;
use prototype_rcf::hash_name;

/// `(real .gfx path, its PROVEN-hash member id)`.
pub fn gfx_hashes() -> Vec<(&'static str, u32)> {
    crate::menu::REAL_SCREENS
        .iter()
        .map(|p| (*p, hash_name(p)))
        .collect()
}

/// Block 120: the screens the REAL menu renders this block — the
/// press-start splash and the main menu (the census still covers all
/// 25; extraction touches only these two members).  Bytes come out of
/// the user's own art.rcf, parse through
/// `prototype_render::frontend_gfx`, and become the menu's frame.
pub const RENDER_SCREENS: &[&str] = &[
    "art\\hud\\protostart.gfx",
    "art\\hud\\protomainmenu.gfx",
];

/// Census the corpus: which real menu screens are present in the given
/// archives?  STREAMING (block 120): the directory table only — never
/// the member data, never a whole-archive read.
pub fn census(archives: &[std::path::PathBuf]) -> (usize, Vec<&'static str>) {
    let names: Vec<&'static str> = extract_screens_impl(archives, crate::menu::REAL_SCREENS, false)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    (names.len(), names)
}

/// Extract the raw `.gfx` member bytes for the named screens (block
/// 120): one streaming directory parse per archive, one span read +
/// RZ decompress per hit.  Missing screens are simply absent from the
/// result; unreadable archives are skipped (the caller journals both).
pub fn extract_screens(
    archives: &[std::path::PathBuf],
    wanted: &[&'static str],
) -> Vec<(&'static str, Vec<u8>)> {
    extract_screens_impl(archives, wanted, true)
}

fn extract_screens_impl(
    archives: &[std::path::PathBuf],
    wanted: &[&'static str],
    read_payload: bool,
) -> Vec<(&'static str, Vec<u8>)> {
    let hashes: Vec<(u32, &'static str)> =
        wanted.iter().map(|p| (hash_name(p), *p)).collect();
    let mut out = Vec::new();
    for path in archives {
        let Ok(mut file) = std::fs::File::open(path) else { continue };
        let Ok((_, entries)) = prototype_rcf::parse_directory(&mut file) else {
            continue;
        };
        // Hash → entry (the LAST hit wins, as the whole-file census did).
        let mut by_hash: HashMap<u32, prototype_rcf::RcfDirEntry> = HashMap::new();
        for e in entries {
            by_hash.insert(e.name_hash, e);
        }
        for (h, name) in &hashes {
            let name = *name;
            let Some(entry) = by_hash.get(h) else { continue };
            if !read_payload {
                out.push((name, Vec::new()));
                continue;
            }
            // The span reader re-checks the entry's bounds against the
            // real stream length (fail-closed), then RZ-decompresses.
            match prototype_rcf::read_member_payload(&mut file, entry) {
                Ok(bytes) => out.push((name, bytes)),
                Err(e) => log::warn!("menu art: {} payload failed ({e})", path.display()),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hash bridge: the proven algorithm maps the real DLL string
    /// to a stable member id, and the catalog hashes are distinct.
    #[test]
    fn gfx_hashes_are_stable_and_distinct() {
        let hs = gfx_hashes();
        assert_eq!(hs.len(), crate::menu::REAL_SCREENS.len());
        for (path, hash) in &hs {
            assert_eq!(*hash, hash_name(path), "hash drift on {path}");
        }
        let mut sorted: Vec<u32> = hs.iter().map(|(_, h)| *h).collect();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), hs.len(), "hash collisions in the catalog");
    }

    /// The census runs against a REAL archive (the committed
    /// scripts.rcf) and simply reports honestly (scripts holds no
    /// menu .gfx — the census says 0 without exploding).
    #[test]
    fn census_is_honest_on_a_real_archive() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join("scripts.rcf");
        let (n, found) = census(&[path]);
        assert_eq!(n, 0);
        assert!(found.is_empty());
    }

    /// Block 120: the STREAMING census reads directories, not member
    /// data — assert by construction (a nonexistent archive is
    /// skipped; a real one returns the same answer the old whole-file
    /// census gave).
    #[test]
    fn census_skips_missing_archives_fail_open() {
        let bogus = std::path::PathBuf::from("/nonexistent/nothing.rcf");
        let (n, found) = census(&[bogus]);
        assert_eq!((n, found.len()), (0, 0));
    }
}

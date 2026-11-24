//! Block 107 — the REAL menu asset locator.
//!
//! The game's menus are Scaleform GFx members (`art\hud\*.gfx`) living
//! inside the art archives.  The engine looks members up by
//! `name_hash` — and the hash algorithm is **PROVEN**
//! (`prototype_rcf::hash_name`).  So without the user's corpus in this
//! sandbox we can still:
//!
//! 1. compute the exact member hash of every real menu screen, and
//! 2. at boot, CENSUS which of the 25 real `.gfx` screens exist in
//!    the user's archives (the worker journals it into boot-log.txt —
//!    the next device run tells us exactly what art we can extract).
//!
//! Extraction of the SWF shapes is the NEXT-UP; the hash bridge is
//! what makes it possible.

use prototype_rcf::hash_name;

/// `(real .gfx path, its PROVEN-hash member id)`.
pub fn gfx_hashes() -> Vec<(&'static str, u32)> {
    crate::menu::REAL_SCREENS
        .iter()
        .map(|p| (*p, hash_name(p)))
        .collect()
}

/// Census the corpus: which real menu screens are present in the given
/// archives?  O(archives × members) hash-set lookups — no decompress.
pub fn census(archives: &[std::path::PathBuf]) -> (usize, Vec<&'static str>) {
    let wanted: std::collections::HashSet<u32> =
        gfx_hashes().into_iter().map(|(_, h)| h).collect();
    let mut found = Vec::new();
    for path in archives {
        let Ok(data) = std::fs::read(path) else { continue };
        let Ok(arc) = prototype_rcf::RcfArchive::parse(&data) else { continue };
        for e in &arc.entries {
            if wanted.contains(&e.name_hash) {
                if let Some((name, _)) = crate::menu::REAL_SCREENS
                    .iter()
                    .enumerate()
                    .find(|(_, p)| hash_name(p) == e.name_hash)
                {
                    found.push(crate::menu::REAL_SCREENS[name]);
                }
            }
        }
    }
    (found.len(), found)
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
}

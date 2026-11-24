//! The world-index CACHE (block 105) — the one-time boot killer.
//!
//! The lean index (block 104) decompresses every world-relevant member
//! of every archive to build the [`WorldIndex`].  On the user's phone
//! that measured minutes (the block-104 log: 6.2 minutes and still
//! going).  The index is PLAIN DATA — every field of every entry is
//! derived from the corpus and never from wall-clock state — so it is
//! written once after a successful build and read back (milliseconds)
//! on every later boot.  The cache lives in the user's game folder
//! (`world-index.cache`), next to the archives it describes.
//!
//! # Format
//!
//! ```text
//! [u32 magic "PRWI"][u32 format version][u64 fingerprint][u32 member count]
//! [u32 archive count][u64 bincode payload length][bincode(WorldIndex)]
//! ```
//!
//! * The FINGERPRINT binds the cache to the corpus it was built from:
//!   every archive's file name + byte length (stable across USB copies,
//!   unlike mtimes) + the scan options + this format version.  ANY
//!   mismatch (corpus edited, options changed, crate version bumped)
//!   is a cache MISS — rebuild honestly, never guess.
//! * Reads are fail-open: ANY error (missing file, bad magic, version
//!   mismatch, fingerprint mismatch, truncated/corrupt payload) returns
//!   `None` and the caller rebuilds.  A cache must never take a boot
//!   down.
//!
//! Serialization is serde+bincode over the existing world types (they
//! are plain data; the derives were added in block 105).

use crate::world::{ScanOptions, WorldIndex};
use std::path::Path;

/// Cache magic ("PRototype World Index").
pub const CACHE_MAGIC: &[u8; 4] = b"PRWI";
/// Cache format version — bump whenever the world types change shape.
pub const CACHE_VERSION: u32 = 1;

/// What a successful cache read hands back: the index plus the honest
/// counters the boot log reports (they were measured at build time).
#[derive(Debug, Clone)]
pub struct CachedIndex {
    pub index: WorldIndex,
    pub member_count: u32,
    pub archive_count: u32,
}

/// FNV-1a 64 — the corpus fingerprint hash (no external hasher needed
/// for a cache-validity signal).
fn fnv1a(bytes: &[u8], state: u64) -> u64 {
    let mut h = state;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3_dc9f);
    }
    h
}

/// The corpus fingerprint: scan options + format version + every
/// archive's file name and byte length, in the order the boot indexes
/// them.  `archive_lens` is (file name, byte length) pairs.
pub fn index_fingerprint(archive_lens: &[(String, u64)], opts: ScanOptions) -> u64 {
    let mut h = fnv1a(CACHE_MAGIC, 0xcbf2_9ce4_8422_2325);
    h = fnv1a(&CACHE_VERSION.to_le_bytes(), h);
    h = fnv1a(
        &[
            opts.retain_objects,
            opts.retain_shape_names,
            opts.retain_node_transforms,
        ]
        .map(|b| b as u8),
        h,
    );
    h = fnv1a(&(archive_lens.len() as u32).to_le_bytes(), h);
    for (name, len) in archive_lens {
        h = fnv1a(name.as_bytes(), h);
        h = fnv1a(&[0], h);
        h = fnv1a(&len.to_le_bytes(), h);
    }
    h
}

/// Write the cache (atomic: `.<name>.tmp` + rename, so a killed boot
/// never leaves a half-written cache).
pub fn write_index_cache(
    path: &Path,
    fingerprint: u64,
    member_count: u32,
    archive_count: u32,
    index: &WorldIndex,
) -> std::io::Result<()> {
    let payload = bincode::serialize(index).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("bincode: {e}"))
    })?;
    let mut out = Vec::with_capacity(payload.len() + 40);
    out.extend_from_slice(CACHE_MAGIC);
    out.extend_from_slice(&CACHE_VERSION.to_le_bytes());
    out.extend_from_slice(&fingerprint.to_le_bytes());
    out.extend_from_slice(&member_count.to_le_bytes());
    out.extend_from_slice(&archive_count.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    out.extend_from_slice(&payload);
    let tmp = path.with_file_name(format!(
        ".{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&tmp, &out)?;
    // Rename over any previous cache (same volume → atomic on Linux
    // and on Android's emulated storage).
    std::fs::rename(&tmp, path)
}

/// Read the cache; `None` on ANY problem (missing, wrong magic,
/// version/fingerprint mismatch, corrupt payload).  Fail-open by
/// contract — the caller rebuilds the index honestly.
pub fn read_index_cache(path: &Path, fingerprint: u64) -> Option<CachedIndex> {
    let data = std::fs::read(path).ok()?;
    // Header: 4 magic + 4 version + 8 fp + 4 members + 4 archives + 8 len.
    if data.len() < 32 || &data[0..4] != CACHE_MAGIC {
        return None;
    }
    let version = u32::from_le_bytes(data[4..8].try_into().ok()?);
    if version != CACHE_VERSION {
        return None;
    }
    let fp = u64::from_le_bytes(data[8..16].try_into().ok()?);
    if fp != fingerprint {
        return None;
    }
    let member_count = u32::from_le_bytes(data[16..20].try_into().ok()?);
    let archive_count = u32::from_le_bytes(data[20..24].try_into().ok()?);
    let payload_len = u64::from_le_bytes(data[24..32].try_into().ok()?) as usize;
    if data.len() != 32 + payload_len {
        return None;
    }
    let index: WorldIndex = bincode::deserialize(&data[32..]).ok()?;
    // The member count is an honest-record check: entries must match it.
    if index.entries.len() != member_count as usize {
        return None;
    }
    Some(CachedIndex {
        index,
        member_count,
        archive_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{CellObject, MemberKind, ObjectVariant, Placement, WorldIndex, WorldIndexEntry};

    fn sample_index() -> WorldIndex {
        let mut e = WorldIndexEntry::default();
        e.hash = 0x51A2_CA3B;
        e.kind = MemberKind::Objects;
        e.objects = 12;
        e.cells.push(82);
        e.bbox = Some([0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
        e.drawables.push("signHorizontal001".to_string());
        e.retained_objects.push(CellObject {
            full_name: "proto::CloudSpawner(1):Cell_65Collectible004_38".into(),
            short_name: "Cell_65Collectible004_38".into(),
            class: "proto::CloudSpawner".into(),
            variant: ObjectVariant::Binary,
            placement: Some(Placement {
                pos: [1.0, 2.0, 3.0],
                quat_xyzw: [0.0, 0.0, 0.0, 1.0],
            }),
            drawable: Some("signHorizontal001".into()),
            scale: None,
            state: None,
            payload: None,
        });
        WorldIndex { entries: vec![e] }
    }

    fn opts() -> ScanOptions {
        ScanOptions {
            retain_objects: true,
            retain_shape_names: true,
            retain_node_transforms: true,
        }
    }

    fn cache_dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join("prwi-block105");
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The cache roundtrip preserves every field the boot consumes —
    /// counters, bbox, cells, and the full retained object records —
    /// and the loaded index still drives the pairing machinery.
    #[test]
    fn cache_roundtrip_preserves_entries() {
        let path = cache_dir().join("roundtrip.cache");
        let lens = vec![("00cells.rcf".to_string(), 12_345u64)];
        let fp = index_fingerprint(&lens, opts());
        let idx = sample_index();
        write_index_cache(&path, fp, 1, 1, &idx).unwrap();
        let back = read_index_cache(&path, fp).expect("cache must read back");
        assert_eq!(back.member_count, 1);
        assert_eq!(back.archive_count, 1);
        assert_eq!(back.index.entries.len(), 1);
        let a = &idx.entries[0];
        let b = &back.index.entries[0];
        assert_eq!(a.hash, b.hash);
        assert_eq!(a.kind, b.kind);
        assert_eq!(a.objects, b.objects);
        assert_eq!(a.bbox, b.bbox);
        assert_eq!(a.retained_objects, b.retained_objects);
        // The loaded index still feeds the runtime's consumers.
        let _ = back.index.pair_drawables(usize::MAX);
        let _ = back.index.totals();
    }

    /// ANY fingerprint change (a corpus byte count, the scan options)
    /// invalidates the cache — fail-open to a rebuild, never guess.
    #[test]
    fn cache_invalidates_on_fingerprint_change() {
        let path = cache_dir().join("invalidate.cache");
        let lens = vec![("00cells.rcf".to_string(), 12_345u64)];
        let fp = index_fingerprint(&lens, opts());
        write_index_cache(&path, fp, 1, 1, &sample_index()).unwrap();
        // A changed archive length...
        let grown = vec![("00cells.rcf".to_string(), 99_999u64)];
        assert!(read_index_cache(&path, index_fingerprint(&grown, opts())).is_none());
        // ...changed scan options...
        let lean_opts = ScanOptions {
            retain_objects: false,
            retain_shape_names: false,
            retain_node_transforms: false,
        };
        assert!(read_index_cache(&path, index_fingerprint(&lens, lean_opts)).is_none());
        // ...and a corrupt file.
        std::fs::write(&path, b"garbage-not-a-cache").unwrap();
        assert!(read_index_cache(&path, fp).is_none());
        // A missing file is simply a miss.
        assert!(read_index_cache(&path.with_extension("missing"), fp).is_none());
    }
}

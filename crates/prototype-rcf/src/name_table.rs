// ---------------------------------------------------------------------------
// The RCF name table (block 61): member names for the hash-keyed directory
// ---------------------------------------------------------------------------

//! The RCF **name table** — the member-name side of the cement directory.
//!
//! The cement directory stores no names: every member is keyed by its
//! [`hash_name`] alone.  The two audio archives shipped on the user's
//! GitHub release (`00audio.rcf`, `01audio.rcf`, tag v1.0) are the FIRST
//! observed archives that also carry a NAME TABLE — a region between the
//! directory table and the first payload that enumerates every member's
//! archive path in plain text.  This module parses it and maps it onto a
//! parsed directory, turning a hash-keyed cement archive into a fully
//! named roster.
//!
//! ## Evidence (all PROVEN against the real remote archives, block 61)
//!
//! Everything below was measured by surgical Range reads against
//! `https://github.com/Nazatric/PrototypeRecomp/releases/download/v1.0/`
//! (`00audio.rcf` = 1 036 156 325 B, `01audio.rcf` = 1 026 052 106 B) and
//! is committed as the `audio_00_dir.bin` / `audio_00_nametable.bin` /
//! `audio_01_dir.bin` / `audio_01_nametable.bin` fixtures.
//!
//! * **Placement**: the header's `name_table_offset` field (offset 48)
//!   does NOT hold the name-table start for these archives (the observed
//!   value points INSIDE the table region).  The table occupies the whole
//!   gap between `dir_offset + dir_size_in_bytes` and the smallest
//!   directory `file_offset`, zero-filled at both ends.
//! * **Prologue**: the first non-zero bytes are the 8 bytes
//!   `[u32 0x0000_0800][u32 0]` — the record marker + a zero word — then
//!   records begin.
//! * **Record layout** (byte-packed, no alignment):
//!   `[u32 id][u32 0x0000_0800][u32 0][u32 len][len bytes: name\0
//!   zero-filled][3 zero bytes]` — stride `16 + len + 3`.  `len` counts
//!   the NUL (name length + 1).  The trailing 3 bytes are always zero in
//!   both real tables (pinned by test).
//! * **Order = member build order = payload-offset order** (PROVEN): the
//!   i-th name record corresponds to the i-th directory entry when the
//!   directory is sorted by `file_offset`.  Anchored by byte identity:
//!   record 7 of `00audio.rcf` names `audio\boot_strap.p3d`, the
//!   offset-sorted directory entry 7 is
//!   `(hash 0xBD4FDDAC, offset 6 817 792, size 3 969 452)`, and a Range
//!   fetch of exactly that span returned the committed
//!   `boot_strap.p3d` fixture **byte-for-byte** (3 969 452 B, the
//!   block-58 audio-clip fixture).
//! * **The `id` field is UNKNOWN** (documented honestly): every observed
//!   id shares the top half-word `0x688D____`, ids are NOT unique (4 140
//!   distinct values across 13 486 records — different names collide),
//!   and the value matches neither [`hash_name`] nor CRC-32/FNV-1/FNV-1a
//!   of the name.  It is carried through as data; the roster key is the
//!   PROVEN positional mapping.
//!
//! ## What the two audio archives contain (the block-61 census)
//!
//! * `00audio.rcf`: 13 486 members, 1 021 404 388 B of payload — 21
//!   top-level bank P3Ds (`audio\alex*.p3d`, `ambience`,
//!   `apc_m2_marine`, `boot_strap`, `brawler`, `e01m01_sfx` and the
//!   ELEVEN mission music banks `e01_music` .. `e11_music`) plus 13 465
//!   sharded clip files `audio\english\AudioFile\<x>\<y>\<8hex>.p3d`.
//! * `01audio.rcf`: 1 128 members, 1 024 825 415 B — 20 top-level banks
//!   (`front_end_sounds`, `infected`, `infected2`, `leader_hunter`,
//!   `marine`, `mother`, `physics_shared`, `sfx_shared`, `specialist`,
//!   `supersoldier`, `supreme_hunter`, the vehicle/aircraft set, ...),
//!   1 107 sharded clips, and the **35 NIS mix files** —
//!   `audio\english\AudioFile\nis*_mix.p3d` / `mnis_*_mix.p3d` — which
//!   resolve block 56's boot_strap audio-patch ids (the patch table says
//!   `nis0-3a_mix.wav`; the shipped cement member is the `.p3d`
//!   container of the same name stem).
//!
//! ## Confidence labels
//!
//! * PROVEN: the record layout, the prologue, the zero tails, the
//!   offset-order mapping (boot_strap byte identity), and every number
//!   in the census (pinned by tests against the committed tables).
//!   (`00audio.rcf`: 13 486 records, 1 021 404 388 B; `01audio.rcf`:
//!   1 128 records, 1 024 825 415 B.)
//! * UNKNOWN: the `id` field's semantics; the `name_table_offset`
//!   header field's exact meaning for these archives; whether OTHER
//!   archives (art/cells/movies) also carry name tables — their
//!   directory-to-payload gaps are zero-filled (checked for movies.rcf
//!   in earlier blocks), so they are keyed by hash alone.

use crate::{P3dError, RcfDirEntry, Result};

/// The record marker word (every record's second u32).
const RECORD_MARKER: u32 = 0x0000_0800;

/// The prologue's first word, observed in both real tables: the record
/// marker `0x800` followed by a zero word (an empty-record-shaped 8-byte
/// preamble).
pub const PROLOGUE_FIRST: u32 = RECORD_MARKER;

/// One name-table record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameTableEntry {
    /// The record's `id` word — always `0x688D____` in the two real
    /// tables, NOT unique per name, semantics UNKNOWN (see module docs).
    pub id: u32,
    /// The member's archive path, e.g. `audio\boot_strap.p3d`.
    /// Backslash-separated, lowercase, exactly as shipped.
    pub name: String,
}

/// A parsed RCF name table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameTable {
    /// The 8 prologue bytes, verbatim (`[0x0000_0800][0]` observed).
    pub prologue: [u8; 8],
    /// The records, in table order (= member build order).
    pub entries: Vec<NameTableEntry>,
}

impl NameTable {
    /// Parse a name-table region (the bytes between the directory table
    /// and the first payload).  Fails closed on any malformation: a
    /// record must carry the `0x800` marker, a zero word, a plausible
    /// length (1..=4095), a NUL-terminated ASCII name that fills the
    /// length, three trailing zero bytes, and everything after the last
    /// record must be zero fill.
    pub fn parse(data: &[u8]) -> Result<Self> {
        // Find the first record: the first 4-aligned position whose
        // [id][marker][zero][len] header validates and whose name
        // round-trips (printable ASCII, NUL-terminated, fills `len`).
        let first_nz = data
            .iter()
            .position(|&b| b != 0)
            .ok_or_else(|| P3dError::Other("rcf name table is all zero".into()))?;
        let scan_from = first_nz.saturating_sub(8) & !3;
        let scan_to = (first_nz + 16).min(data.len());
        let mut start = None;
        for p in (scan_from..scan_to).step_by(4) {
            if p + 20 > data.len() {
                break;
            }
            if Self::record_ok(data, p) {
                start = Some(p);
                break;
            }
        }
        let start = start.ok_or_else(|| {
            P3dError::Other(format!(
                "rcf name table: no valid record near first nonzero byte {first_nz}"
            ))
        })?;
        if start < 8 {
            return Err(P3dError::Other(
                "rcf name table: records begin before the 8-byte prologue".into(),
            ));
        }
        let mut prologue = [0u8; 8];
        prologue.copy_from_slice(&data[start - 8..start]);
        if u32::from_le_bytes(prologue[0..4].try_into().unwrap()) != PROLOGUE_FIRST
            || u32::from_le_bytes(prologue[4..8].try_into().unwrap()) != 0
        {
            return Err(P3dError::Other(format!(
                "rcf name table: unexpected prologue {:?}",
                prologue
            )));
        }

        let mut entries = Vec::new();
        let mut pos = start;
        while pos + 20 <= data.len() {
            if !Self::record_ok(data, pos) {
                break;
            }
            let id = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
            let len =
                u32::from_le_bytes(data[pos + 12..pos + 16].try_into().unwrap()) as usize;
            let field = &data[pos + 16..pos + 16 + len];
            let nul = field
                .iter()
                .position(|&b| b == 0)
                .ok_or_else(|| {
                    P3dError::Other(format!("rcf name table: record at {pos} has no NUL"))
                })?;
            let name = std::str::from_utf8(&field[..nul])
                .map_err(|e| {
                    P3dError::Other(format!("rcf name table: record at {pos}: {e}"))
                })?
                .to_string();
            if name.bytes().any(|b| !b.is_ascii_graphic() && b != b' ') {
                return Err(P3dError::Other(format!(
                    "rcf name table: record at {pos} has a non-printable name byte"
                )));
            }
            // the field is zero-filled after the NUL ...
            if field[nul..].iter().any(|&b| b != 0) {
                return Err(P3dError::Other(format!(
                    "rcf name table: record at {pos} has nonzero fill bytes"
                )));
            }
            // ... and exactly 3 zero bytes follow the field.
            if data[pos + 16 + len..pos + 16 + len + 3].iter().any(|&b| b != 0) {
                return Err(P3dError::Other(format!(
                    "rcf name table: record at {pos} lacks the 3 zero tail bytes"
                )));
            }
            entries.push(NameTableEntry { id, name });
            pos += 16 + len + 3;
        }
        if data[pos..].iter().any(|&b| b != 0) {
            return Err(P3dError::Other(format!(
                "rcf name table: nonzero bytes after the last record at {pos}"
            )));
        }
        if entries.is_empty() {
            return Err(P3dError::Other("rcf name table: zero records".into()));
        }
        Ok(Self { prologue, entries })
    }

    /// Validate the fixed header of the record at `pos`.
    fn record_ok(data: &[u8], pos: usize) -> bool {
        if pos + 20 > data.len() {
            return false;
        }
        let marker = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap());
        let zero = u32::from_le_bytes(data[pos + 8..pos + 12].try_into().unwrap());
        let len = u32::from_le_bytes(data[pos + 12..pos + 16].try_into().unwrap());
        if marker != RECORD_MARKER || zero != 0 || !(1..4096).contains(&len) {
            return false;
        }
        if pos + 16 + len as usize + 3 > data.len() {
            return false;
        }
        let field = &data[pos + 16..pos + 16 + len as usize];
        match field.iter().position(|&b| b == 0) {
            Some(n) if n > 0 => {
                field[..n].iter().all(|&b| (0x20..0x7F).contains(&b))
            }
            _ => false,
        }
    }
}

/// One fully named member: the directory entry joined with its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedMember {
    /// The directory's [`hash_name`] of [`Self::name`].
    pub dir_hash: u32,
    /// The archive path from the name table.
    pub name: String,
    /// Absolute payload byte offset.
    pub file_offset: u32,
    /// Exact payload byte length.
    pub file_size: u32,
}

/// Join a parsed directory with a parsed name table.
///
/// The name table is in **member build order**; the directory is stored
/// hash-sorted.  The mapping (PROVEN — see the module docs' boot_strap
/// byte-identity anchor) pairs the i-th name record with the i-th
/// directory entry sorted by `file_offset`.  Fails closed when the
/// counts differ or the directory has duplicate offsets.
pub fn roster(entries: &[RcfDirEntry], table: &NameTable) -> Result<Vec<NamedMember>> {
    if entries.len() != table.entries.len() {
        return Err(P3dError::Other(format!(
            "rcf roster: directory has {} entries but the name table has {} records",
            entries.len(),
            table.entries.len()
        )));
    }
    let mut by_offset: Vec<&RcfDirEntry> = entries.iter().collect();
    by_offset.sort_by_key(|e| e.file_offset);
    for w in by_offset.windows(2) {
        if w[0].file_offset == w[1].file_offset {
            return Err(P3dError::Other(format!(
                "rcf roster: duplicate payload offset {} in directory",
                w[0].file_offset
            )));
        }
    }
    Ok(by_offset
        .iter()
        .zip(table.entries.iter())
        .map(|(e, n)| NamedMember {
            dir_hash: e.name_hash,
            name: n.name.clone(),
            file_offset: e.file_offset,
            file_size: e.file_size,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fixture(name: &str) -> Vec<u8> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    /// Parse `<archive>.dir.bin` into directory entries (the raw
    /// 12-byte table, no payload).
    fn dir_entries(name: &str) -> Vec<RcfDirEntry> {
        let data = fixture(name);
        let mut out = Vec::with_capacity(data.len() / 12);
        for chunk in data.chunks_exact(12) {
            let name_hash = u32::from_le_bytes(chunk[0..4].try_into().unwrap());
            let file_offset = u32::from_le_bytes(chunk[4..8].try_into().unwrap());
            let file_size = u32::from_le_bytes(chunk[8..12].try_into().unwrap());
            out.push(RcfDirEntry { name_hash, file_offset, file_size });
        }
        out
    }

    #[test]
    fn parses_01audio_nametable() {
        let nt = NameTable::parse(&fixture("audio_01_nametable.bin")).expect("parse");
        assert_eq!(nt.entries.len(), 1128);
        let mut want_pro = [0u8; 8];
        want_pro[0..4].copy_from_slice(&PROLOGUE_FIRST.to_le_bytes());
        assert_eq!(nt.prologue, want_pro);
        assert!(nt.entries.iter().all(|e| e.name.starts_with("audio\\")));
        assert!(nt
            .entries
            .iter()
            .all(|e| e.name.rsplit('.').next() == Some("p3d")));
        // first record: the first-built 01audio member (offset 83 968)
        assert_eq!(nt.entries[0].name, "audio\\english\\AudioFile\\e03m02_combat.p3d");
    }

    #[test]
    fn parses_00audio_nametable() {
        let nt = NameTable::parse(&fixture("audio_00_nametable.bin")).expect("parse");
        assert_eq!(nt.entries.len(), 13486);
        // the first records are the top-level banks in build order
        assert_eq!(nt.entries[0].name, "audio\\alex.p3d");
        assert_eq!(nt.entries[1].name, "audio\\alex_blades.p3d");
        let boot = nt.entries.iter().find(|e| e.name == "audio\\boot_strap.p3d");
        assert!(boot.is_some(), "boot_strap must be named");
    }

    #[test]
    fn roster_01audio_full_census() {
        let nt = NameTable::parse(&fixture("audio_01_nametable.bin")).expect("parse");
        let dir = dir_entries("audio_01_dir.bin");
        let r = roster(&dir, &nt).expect("roster");
        assert_eq!(r.len(), 1128);
        // 1:1 with the directory's hash set
        let mut a: Vec<u32> = dir.iter().map(|e| e.name_hash).collect();
        let mut b: Vec<u32> = r.iter().map(|m| m.dir_hash).collect();
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b);
        // 20 top-level banks
        let banks: Vec<&str> = r
            .iter()
            .map(|m| m.name.as_str())
            .filter(|n| !n.contains("english\\"))
            .collect();
        assert_eq!(banks.len(), 20);
        assert!(banks.contains(&"audio\\front_end_sounds.p3d"));
        assert!(banks.contains(&"audio\\supreme_hunter.p3d"));
        // the 35 NIS mix files resolve block 56's patch ids
        let nis: Vec<&str> = r
            .iter()
            .map(|m| m.name.as_str())
            .filter(|n| {
                let leaf = n.rsplit('\\').next().unwrap();
                leaf.contains("nis") && leaf.contains("_mix")
            })
            .collect();
        assert_eq!(nis.len(), 35);
        assert!(nis.contains(&"audio\\english\\AudioFile\\nis7-3a_mix.p3d"));
        assert!(nis.contains(&"audio\\english\\AudioFile\\nis7-3b_mix.p3d"));
        assert!(nis.contains(&"audio\\english\\AudioFile\\mnis_5_2_1_mix.p3d"));
        // total payload
        let total: u64 = r.iter().map(|m| m.file_size as u64).sum();
        assert_eq!(total, 1_024_825_415);
    }

    #[test]
    fn roster_00audio_boot_strap_anchor() {
        let nt = NameTable::parse(&fixture("audio_00_nametable.bin")).expect("parse");
        let dir = dir_entries("audio_00_dir.bin");
        let r = roster(&dir, &nt).expect("roster");
        assert_eq!(r.len(), 13486);
        let boot = r
            .iter()
            .find(|m| m.name == "audio\\boot_strap.p3d")
            .expect("boot_strap named");
        assert_eq!(boot.dir_hash, 0xBD4F_DDAC);
        assert_eq!(boot.file_offset, 6_817_792);
        assert_eq!(boot.file_size, 3_969_452);
        // THE ANCHOR: the span is byte-identical to the committed
        // boot_strap.p3d fixture (verified by a Range fetch of the real
        // archive during block 61) — so the name->entry mapping and the
        // fixture length must agree.
        let local = fixture("boot_strap.p3d");
        assert_eq!(local.len() as u32, boot.file_size);
        // 21 top-level banks incl. the eleven music banks e01..e11
        let banks: Vec<&str> = r
            .iter()
            .map(|m| m.name.as_str())
            .filter(|n| !n.contains("english\\"))
            .collect();
        assert_eq!(banks.len(), 21);
        for e in 1..=11u8 {
            let want = format!("audio\\e{e:02}_music.p3d");
            assert!(banks.contains(&want.as_str()), "missing {want}");
        }
        let total: u64 = r.iter().map(|m| m.file_size as u64).sum();
        assert_eq!(total, 1_021_404_388);
    }

    #[test]
    fn name_table_fails_closed_on_corruption() {
        let clean = fixture("audio_01_nametable.bin");
        assert!(NameTable::parse(&clean).is_ok());
        // locate the first record deterministically: the first 4-aligned
        // position whose [marker=0x800][zero] header follows a nonzero byte
        let fnz = clean.iter().position(|&b| b != 0).unwrap();
        let mut start = None;
        for p in (fnz.saturating_sub(8) & !3..fnz + 16).step_by(4) {
            if p + 12 > clean.len() {
                break;
            }
            let marker = u32::from_le_bytes(clean[p + 4..p + 8].try_into().unwrap());
            let zero = u32::from_le_bytes(clean[p + 8..p + 12].try_into().unwrap());
            if marker == 0x800 && zero == 0 {
                start = Some(p);
                break;
            }
        }
        let start = start.expect("fixture has a first record");
        // 1. a corrupted record marker
        let mut data = clean.clone();
        data[start + 4..start + 8].copy_from_slice(&0xDEAD_u32.to_le_bytes());
        assert!(NameTable::parse(&data).is_err());
        // 2. truncated region (no records at all)
        assert!(NameTable::parse(&[0u8; 64]).is_err());
        // 3. a broken prologue
        let mut data = clean.clone();
        data[start - 8..start - 4].copy_from_slice(&0x1234_u32.to_le_bytes());
        assert!(NameTable::parse(&data).is_err());
    }

    #[test]
    fn roster_fails_closed_on_count_mismatch() {
        let nt = NameTable::parse(&fixture("audio_01_nametable.bin")).expect("parse");
        let mut dir = dir_entries("audio_01_dir.bin");
        dir.pop();
        assert!(roster(&dir, &nt).is_err());
        // duplicate offsets fail closed
        let dir = dir_entries("audio_01_dir.bin");
        let mut dup = dir.clone();
        dup[1].file_offset = dup[0].file_offset;
        assert!(roster(&dup, &nt).is_err());
    }
}

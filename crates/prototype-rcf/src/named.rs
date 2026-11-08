// ---------------------------------------------------------------------------
// The named archive (block 62): serve a cement archive's members BY NAME
// ---------------------------------------------------------------------------

//! The **named archive** — a `Read + Seek` source (typically a `File`
//! over one of the user's own `.rcf` archives) joined with its
//! directory and NAME TABLE, exposing member payloads by archive path.
//!
//! This is the serving half of block 61's discovery.  The cement
//! directory keys members by [`hash_name`] alone; the two audio
//! archives shipped on the user's GitHub release (`00audio.rcf`,
//! `01audio.rcf`) are the first observed archives that ALSO carry a
//! name table (see [`crate::name_table`]).  `NamedArchive` parses the
//! header + directory WITHOUT loading any payload
//! ([`crate::parse_directory`]), reads the name-table region (the gap
//! between the directory table and the first payload — block 61's
//! PROVEN placement), builds the PROVEN roster
//! ([`crate::name_table::roster`]) and then serves:
//!
//! * [`Self::lookup`] — the roster entry for an archive path
//!   (`audio\boot_strap.p3d`);
//! * [`Self::read_member`] — the full member payload (span read +
//!   RZ decompression, bit-identical to the whole-file parse);
//! * [`Self::open_member`] — a [`MemberSpanReader`] window over the
//!   member's span (the streaming shape the Bink player uses).
//!
//! ## Lookup normalization (DESIGN, documented)
//!
//! The shipped tables are lowercase with backslash separators.  The
//! original engine's name handling for these archives is UNKNOWN (no
//! disassembly), so the exact-match path is the primary key and a
//! normalized fallback (case-insensitive, `/` ≡ `\`) is provided for
//! caller convenience — clearly marked as our choice, not evidence.
//!
//! ## Fail-closed
//!
//! Construction fails when the header/directory is malformed, when
//! the name-table region does not parse, or when the directory and
//! table counts disagree ([`roster`] already fails closed).  Member
//! reads bounds-check spans against the stream's real length.

use crate::name_table::{roster, NameTable, NamedMember};
use crate::{parse_directory, MemberSpanReader, P3dError, RcfDirEntry, Result};
use std::collections::BTreeMap;
use std::io::Read;
use std::io::Seek;

/// A named cement archive over any seekable source.
pub struct NamedArchive<F> {
    source: F,
    /// Parsed header (retained for diagnostics).
    pub header: crate::RcfHeader,
    /// Directory entries, archive order (hash-sorted as shipped).
    pub entries: Vec<RcfDirEntry>,
    /// The name table (verbatim parse).
    pub table: NameTable,
    /// name → roster entry (exact shipped names).
    by_name: BTreeMap<String, NamedMember>,
    /// normalized(name) → exact shipped name (the DESIGN fallback
    /// index; see [`Self::lookup_normalized`]).
    by_norm: BTreeMap<String, String>,
}

/// Normalized name key (DESIGN): backslash separators, ASCII
/// lowercase.  Used only by the convenience fallback index.
fn normalize_name(name: &str) -> String {
    name.bytes()
        .map(|b| match b {
            b'/' => b'\\',
            b'A'..=b'Z' => b + 32,
            _ => b,
        })
        .map(|b| b as char)
        .collect()
}

impl<F: Read + Seek> NamedArchive<F> {
    /// Open `source`: parse header + directory, read the name-table
    /// region (the whole gap between the directory table's end and
    /// the smallest payload offset), parse the table and join the
    /// roster.  Fail-closed at every step.
    pub fn open(mut source: F) -> Result<Self> {
        let (header, entries) = parse_directory(&mut source)?;
        let table = Self::read_name_table(&mut source, &header, &entries)?;
        Self::from_parts(source, header, entries, table)
    }

    /// Assemble from an already-parsed header/directory/table (the
    /// offline test seam; `source` must still be the full archive).
    pub fn from_parts(source: F, header: crate::RcfHeader, entries: Vec<RcfDirEntry>, table: NameTable) -> Result<Self> {
        let roster = roster(&entries, &table)?;
        let mut by_name = BTreeMap::new();
        let mut by_norm = BTreeMap::new();
        for m in roster {
            if by_name.insert(m.name.clone(), m.clone()).is_some() {
                return Err(P3dError::Other(format!(
                    "rcf named: duplicate member name in table"
                )));
            }
            by_norm.insert(normalize_name(&m.name), m.name);
        }
        Ok(Self { source, header, entries, table, by_name, by_norm })
    }

    /// Read the name-table region: the gap between
    /// `dir_offset + dir_size_in_bytes` and the smallest directory
    /// `file_offset` (block 61's PROVEN placement — the header's own
    /// `name_table_offset` field does NOT hold the table start for
    /// these archives).  The gap is capped at 64 MiB to keep a
    /// corrupt directory from demanding an absurd allocation.
    fn read_name_table(
        source: &mut F,
        header: &crate::RcfHeader,
        entries: &[RcfDirEntry],
    ) -> Result<NameTable> {
        use std::io::SeekFrom;
        let dir_end = header
            .dir_offset
            .checked_add(header.dir_size_in_bytes)
            .ok_or_else(|| P3dError::Other("rcf named: dir range overflows u32".into()))?
            as u64;
        let first_payload = entries
            .iter()
            .map(|e| e.file_offset as u64)
            .min()
            .ok_or_else(|| P3dError::Other("rcf named: empty directory".into()))?;
        if first_payload <= dir_end {
            return Err(P3dError::Other(format!(
                "rcf named: no name-table gap (dir ends at {dir_end}, first payload at {first_payload})"
            )));
        }
        let gap = first_payload - dir_end;
        if gap > 64 * 1024 * 1024 {
            return Err(P3dError::Other(format!(
                "rcf named: name-table gap of {gap} B exceeds the 64 MiB sanity cap"
            )));
        }
        source
            .seek(SeekFrom::Start(dir_end))
            .map_err(|e| P3dError::Other(format!("rcf named: gap seek failed: {e}")))?;
        let mut buf = vec![0u8; gap as usize];
        source
            .read_exact(&mut buf)
            .map_err(|e| P3dError::Other(format!("rcf named: gap read failed: {e}")))?;
        NameTable::parse(&buf)
    }

    /// Member count (== directory entry count == table record count).
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// Sorted member names (the archive's full roster).
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }

    /// The roster entry for an exact shipped archive path
    /// (`audio\boot_strap.p3d`).
    pub fn lookup(&self, name: &str) -> Option<&NamedMember> {
        self.by_name.get(name)
    }

    /// Normalized lookup (DESIGN): case-insensitive, `/` ≡ `\`, on
    /// BOTH the query and the stored names (the shipped tables carry
    /// capitals like `AudioFile`).  The exact table is consulted
    /// first; normalization is our convenience layer, not engine
    /// evidence.
    pub fn lookup_normalized(&self, name: &str) -> Option<&NamedMember> {
        if let Some(m) = self.by_name.get(name) {
            return Some(m);
        }
        self.by_norm
            .get(&normalize_name(name))
            .and_then(|exact| self.by_name.get(exact))
    }

    /// The underlying directory entry for a named member (the shape
    /// [`crate::read_member_payload`] consumes).
    pub fn dir_entry(&self, name: &str) -> Option<RcfDirEntry> {
        self.lookup(name).map(|m| RcfDirEntry {
            name_hash: m.dir_hash,
            file_offset: m.file_offset,
            file_size: m.file_size,
        })
    }

    /// Read ONE member's full payload by name (span read +
    /// bounds-check + RZ decompression — bit-identical to the
    /// whole-file parse).
    pub fn read_member(&mut self, name: &str) -> Result<Vec<u8>> {
        let entry = self
            .dir_entry(name)
            .ok_or_else(|| P3dError::Other(format!("rcf named: no member {name:?}")))?;
        crate::read_member_payload(&mut self.source, &entry)
    }

    /// Read ONE member's full payload by normalized name (see
    /// [`Self::lookup_normalized`]).
    pub fn read_member_normalized(&mut self, name: &str) -> Result<Vec<u8>> {
        let m = self
            .lookup_normalized(name)
            .ok_or_else(|| P3dError::Other(format!("rcf named: no member {name:?}")))?;
        let entry = RcfDirEntry {
            name_hash: m.dir_hash,
            file_offset: m.file_offset,
            file_size: m.file_size,
        };
        crate::read_member_payload(&mut self.source, &entry)
    }

    /// A member-relative `Read + Seek` window over the member's span
    /// (the streaming shape — a 1 GB archive never enters RAM).
    pub fn open_member(&mut self, name: &str) -> Result<MemberSpanReader<&mut F>> {
        let m = self
            .lookup(name)
            .ok_or_else(|| P3dError::Other(format!("rcf named: no member {name:?}")))?;
        let (start, len) = (m.file_offset as u64, m.file_size as u64);
        MemberSpanReader::new(&mut self.source, start, len)
    }
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

    /// Build a valid cement archive in memory from the PROVEN layout:
    /// 60-byte header, 12-byte directory entries, the name table in
    /// the dir→payload gap, sector-aligned payloads.
    fn synthesize(members: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let sector = 2048u32;
        // header: 60 B; dir: 12 × n; table: records; payload at the
        // next sector boundary after the table.
        let dir_offset = 60u32;
        let dir_size = (members.len() * 12) as u32;
        let mut table = Vec::new();
        table.extend_from_slice(&0x800u32.to_le_bytes());
        table.extend_from_slice(&0u32.to_le_bytes());
        for (name, _) in members {
            let name_bytes = name.as_bytes();
            let len = name_bytes.len() + 1; // counts the NUL
            table.extend_from_slice(&0x688D_0000u32.to_le_bytes()); // id shape
            table.extend_from_slice(&0x800u32.to_le_bytes());
            table.extend_from_slice(&0u32.to_le_bytes());
            table.extend_from_slice(&(len as u32).to_le_bytes());
            let mut field = vec![0u8; len];
            field[..name_bytes.len()].copy_from_slice(name_bytes);
            table.extend_from_slice(&field);
            table.extend_from_slice(&[0u8; 3]);
        }
        let payload_start = (dir_offset + dir_size + table.len() as u32).div_ceil(sector) * sector;
        let mut out = Vec::new();
        // header
        let mut magic = vec![0u8; 32];
        magic[..crate::RCF_MAGIC.len()].copy_from_slice(crate::RCF_MAGIC);
        out.extend_from_slice(&magic);
        out.extend_from_slice(&crate::RCF_VERSION_FIELD.to_le_bytes());
        out.extend_from_slice(&dir_offset.to_le_bytes());
        out.extend_from_slice(&dir_size.to_le_bytes());
        out.extend_from_slice(&sector.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // name_table_offset (unused)
        out.extend_from_slice(&0u32.to_le_bytes()); // reserved
        out.extend_from_slice(&(members.len() as u32).to_le_bytes());
        assert_eq!(out.len(), 60);
        // directory (hash order irrelevant here; any order parses)
        for ((name, _), (payload_off, payload_len)) in members.iter().zip(
            members
                .iter()
                .scan(payload_start, |st, (_, data)| {
                    let off = *st;
                    *st += data.len() as u32;
                    Some((off, data.len() as u32))
                }),
        ) {
            out.extend_from_slice(&crate::hash_name(name).to_le_bytes());
            out.extend_from_slice(&payload_off.to_le_bytes());
            out.extend_from_slice(&payload_len.to_le_bytes());
        }
        // gap = name table (zero-extend to the payload boundary)
        out.extend_from_slice(&table);
        out.resize(payload_start as usize, 0);
        // payloads
        for (_, data) in members {
            out.extend_from_slice(data);
        }
        out
    }

    /// A tiny but REAL member payload: a one-clip P3D is overkill for
    /// the serving tests — the committed shard member is used as
    /// opaque bytes; its content semantics belong to the audio tests.
    fn shard_member() -> Vec<u8> {
        fixture("audio_member_c6c7ad87.p3d")
    }

    #[test]
    fn serves_real_members_by_name_from_a_synthesized_archive() {
        let boot = fixture("boot_strap.p3d");
        let shard = shard_member();
        let names = [
            "audio\\boot_strap.p3d",
            "audio\\english\\AudioFile\\c6\\c7\\c6c7ad87.p3d",
        ];
        let archive = synthesize(&[
            (names[0], boot.clone()),
            (names[1], shard.clone()),
        ]);
        let mut named = NamedArchive::open(std::io::Cursor::new(archive)).expect("open");
        assert_eq!(named.len(), 2);
        assert_eq!(named.lookup(names[0]).unwrap().file_size as usize, boot.len());
        assert_eq!(named.lookup(names[1]).unwrap().file_size as usize, shard.len());
        // byte-identity both ways
        assert_eq!(named.read_member(names[0]).unwrap(), boot);
        assert_eq!(named.read_member(names[1]).unwrap(), shard);
        // roster integrity: the served hash matches hash_name
        for n in names {
            let m = named.lookup(n).unwrap();
            assert_eq!(m.dir_hash, crate::hash_name(n));
        }
    }

    #[test]
    fn normalized_lookup_finds_slash_and_case_variants() {
        let shard = shard_member();
        let name = "audio\\english\\AudioFile\\c6\\c7\\c6c7ad87.p3d";
        let archive = synthesize(&[(name, shard.clone())]);
        let mut named = NamedArchive::open(std::io::Cursor::new(archive)).expect("open");
        // exact first
        assert!(named.lookup_normalized(name).is_some());
        // slashes + case (DESIGN convenience layer)
        assert!(named
            .lookup_normalized("Audio/English/audiofile/C6/C7/C6C7AD87.p3d")
            .is_some());
        // served bytes identical through the normalized path
        let got = named
            .read_member_normalized("Audio/English/audiofile/C6/C7/C6C7AD87.p3d")
            .unwrap();
        assert_eq!(got, shard);
        assert!(named.lookup("no\\such\\member.p3d").is_none());
        assert!(named.read_member("no\\such\\member.p3d").is_err());
    }

    #[test]
    fn open_member_streams_the_exact_span() {
        let boot = fixture("boot_strap.p3d");
        let name = "audio\\boot_strap.p3d";
        let archive = synthesize(&[(name, boot.clone())]);
        let mut named = NamedArchive::open(std::io::Cursor::new(archive)).expect("open");
        let mut win = named.open_member(name).expect("window");
        assert_eq!(win.member_len() as usize, boot.len());
        // read the whole member through the window: byte-identical
        let mut got = Vec::new();
        std::io::Read::read_to_end(&mut win, &mut got).unwrap();
        assert_eq!(got, boot);
        // seek semantics: window position 0 re-reads the member head
        std::io::Seek::seek(&mut win, std::io::SeekFrom::Start(0)).unwrap();
        let mut head = [0u8; 16];
        win.read_exact(&mut head).unwrap();
        assert_eq!(&head, &boot[..16]);
        assert_eq!(win.window_position(), 16);
    }

    #[test]
    fn census_lookups_match_the_block61_roster_numbers() {
        // The REAL committed audio-01 tables: the roster must place
        // the 35 NIS mix files and the top-level banks at the spans
        // measured live in block 61/62.
        let dir = fixture("audio_01_dir.bin");
        let mut entries = Vec::with_capacity(dir.len() / 12);
        for c in dir.chunks_exact(12) {
            entries.push(RcfDirEntry {
                name_hash: u32::from_le_bytes(c[0..4].try_into().unwrap()),
                file_offset: u32::from_le_bytes(c[4..8].try_into().unwrap()),
                file_size: u32::from_le_bytes(c[8..12].try_into().unwrap()),
            });
        }
        let table = NameTable::parse(&fixture("audio_01_nametable.bin")).expect("table");
        // An EMPTY source stands in for the archive: lookups never
        // touch it (read_member would bounds-fail on a 1 GB member,
        // which is exactly the fail-closed contract).
        let named = NamedArchive::from_parts(
            std::io::Cursor::new(Vec::new()),
            crate::RcfHeader {
                magic: crate::RCF_MAGIC_ASCII.to_string(),
                version_field: crate::RCF_VERSION_FIELD,
                dir_offset: 60,
                dir_size_in_bytes: (entries.len() * 12) as u32,
                sector_size: crate::RCF_DEFAULT_SECTOR_SIZE,
                name_table_offset: 0,
                reserved: 0,
                entry_count: entries.len() as u32,
            },
            entries,
            table,
        )
        .expect("roster");
        assert_eq!(named.len(), 1128);
        let nis = named
            .lookup("audio\\english\\AudioFile\\nis7-3a_mix.p3d")
            .expect("nis7-3a named");
        // measured live at block 62 (curl -r 900409344-907375574)
        assert_eq!(nis.file_offset, 900_409_344);
        assert_eq!(nis.file_size, 6_966_231);
        let mnis = named
            .lookup("audio\\english\\AudioFile\\mnis_5_2_1_mix.p3d")
            .expect("mnis named");
        // measured live at block 62 (curl -r 654192640-655104756)
        assert_eq!(mnis.file_offset, 654_192_640);
        assert_eq!(mnis.file_size, 912_117);
        // the mix fixture IS the live span (byte-identity anchor)
        assert_eq!(
            fixture("audio_member_mnis_5_2_1_mix.p3d").len() as u32,
            mnis.file_size
        );
        let nis_count = named
            .names()
            .filter(|n| {
                let leaf = n.rsplit('\\').next().unwrap();
                leaf.contains("nis") && leaf.contains("_mix")
            })
            .count();
        assert_eq!(nis_count, 35, "the 35 NIS mix members");
    }

    #[test]
    fn fails_closed_on_a_corrupt_table_or_count_mismatch() {
        let shard = shard_member();
        let mut archive = synthesize(&[("audio\\x.p3d", shard.clone())]);
        // corrupt the record marker (first record's second u32)
        let dir_end = 60 + 12;
        let marker_at = dir_end + 8 + 4;
        archive[marker_at..marker_at + 4].copy_from_slice(&0xDEADu32.to_le_bytes());
        assert!(NamedArchive::open(std::io::Cursor::new(archive)).is_err());

        // a directory whose entry count disagrees with the table:
        // build a two-member archive, then zero the table's second
        // record (parse ends early → count mismatch → fail closed).
        let two = synthesize(&[
            ("audio\\a.p3d", shard.clone()),
            ("audio\\b.p3d", shard),
        ]);
        let mut cut = two.clone();
        // second record = dir_end + prologue(8) + record 0's stride
        // (16 + len + 3, len counts the NUL: "audio\a.p3d" → 12)
        let rec0_len = "audio\\a.p3d".len() + 1;
        let rec1 = 60 + 24 + 8 + 16 + rec0_len + 3;
        cut[rec1..rec1 + 20].fill(0);
        assert!(NamedArchive::open(std::io::Cursor::new(cut)).is_err());
    }

    #[test]
    fn fails_closed_on_a_zero_filled_gap_classic_hash_only_archive() {
        // A classic hash-only archive (scripts.rcf shape): the gap
        // between the directory and the first payload is ZERO-FILLED
        // (no name table) → the named open must refuse rather than
        // guess (NameTable::parse fails on an all-zero region).
        let mut out = Vec::new();
        let mut magic = vec![0u8; 32];
        magic[..crate::RCF_MAGIC.len()].copy_from_slice(crate::RCF_MAGIC);
        out.extend_from_slice(&magic);
        out.extend_from_slice(&crate::RCF_VERSION_FIELD.to_le_bytes());
        out.extend_from_slice(&60u32.to_le_bytes()); // dir_offset
        out.extend_from_slice(&12u32.to_le_bytes()); // one entry
        out.extend_from_slice(&crate::RCF_DEFAULT_SECTOR_SIZE.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&crate::hash_name("scripts\\x.lua").to_le_bytes());
        out.extend_from_slice(&2048u32.to_le_bytes()); // payload at 2048
        out.extend_from_slice(&4u32.to_le_bytes());
        out.resize(2048, 0);
        out.extend_from_slice(b"data");
        assert!(NamedArchive::open(std::io::Cursor::new(out)).is_err());
    }
}

//! The SHARD CLIP-NAME INDEX (block 63)
//! ====================================
//!
//! Block 61 proved that the two release audio archives carry 14 572
//! one-clip sharded members (`audio\english\AudioFile\...`) whose
//! SEMANTIC clip names live inside the shards, invisible to the
//! cement directory (which keys members by hash alone).  Block 63
//! mined every shard: a surgical 4 096-byte Range fetch of each
//! member's header region (the RADP payload tail is irrelevant to
//! naming) — 14 572 / 14 572 parsed with zero failures — produced the
//! committed index
//! `prototype-bin/tests/fixtures/audio_shard_index.tsv`:
//!
//! * **14 572 unique clip names, zero duplicates** — a bijective
//!   clip→member map over both archives.
//! * Every `AudioFile` header field Rust-verified identical on 12
//!   full-fetch samples (4 023 B … 60 820 687 B).
//! * The AudioFile chunk carries TWO names: the semantic clip name
//!   and (for the hash-sharded members) a `KEY:<c1>\<c2>\<hex>`
//!   self-description — all 14 318 KEYed rows match their roster
//!   member path exactly (a free whole-archive cross-check).
//! * RADP `ver` is the interleaved stream count (block-62 PROVEN
//!   semantics) and reads like a channel census: all 14 318
//!   hash-shards are v1 (mono SFX @ 32 kHz); the 254 semantic-named
//!   beds are v2 (stereo, 48 kHz), v6 (5.1), v4 (`…_4ch`), v18
//!   (`ambience_baked_18ch`).  Every row's `data_size` divides by
//!   `20·ver` exactly.
//!
//! This module parses that TSV fail-closed and serves lookups.  The
//! index itself is PROVEN data (parsed from the real archives with
//! the PROVEN `prototype-p3d` layouts); the TSV container and the
//! normalized lookup are documented DESIGN.

use std::collections::{BTreeMap, HashSet};

/// The exact column header the parser requires.
pub const INDEX_COLUMNS: &str =
    "clip\tarchive\tmember\toffset\tsize\tver\trate\ttag\tblocks";

/// The member-path prefix every indexed shard must carry (PROVEN
/// family shape from the block-61 census).
pub const SHARD_FAMILY_PREFIX: &str = "audio\\english\\AudioFile\\";

/// Which release archive a shard lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveId {
    /// `00audio.rcf` (13 465 shards, incl. the boot_strap bank).
    A00,
    /// `01audio.rcf` (1 107 shards, incl. the 35 NIS mixes).
    A01,
}

impl ArchiveId {
    /// The shipped archive file name.
    pub fn file_name(self) -> &'static str {
        match self {
            ArchiveId::A00 => "00audio.rcf",
            ArchiveId::A01 => "01audio.rcf",
        }
    }

    /// Reverse of [`ArchiveId::file_name`] (case-insensitive, as the
    /// app sees user-provided paths).
    pub fn from_file_name(name: &str) -> Option<Self> {
        if name.eq_ignore_ascii_case("00audio.rcf") {
            Some(ArchiveId::A00)
        } else if name.eq_ignore_ascii_case("01audio.rcf") {
            Some(ArchiveId::A01)
        } else {
            None
        }
    }
}

/// One indexed shard: where the clip lives and what the AudioFile
/// header says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardEntry {
    /// The semantic clip name (`AudioFile` names[0]).
    pub clip: String,
    /// Which archive carries the member.
    pub archive: ArchiveId,
    /// The cement member path, e.g. `audio\english\AudioFile\c\6\c6c7ad87.p3d`.
    pub member: String,
    /// Member payload offset in the archive (the directory span).
    pub offset: u32,
    /// Member payload size in bytes.
    pub size: u32,
    /// RADP interleaved stream count (block-62 PROVEN semantics).
    pub radp_version: u32,
    /// Native clip sample rate (Hz).
    pub sample_rate: u32,
    /// Opaque per-clip format tag (pass-through to the decoder
    /// factory — block 38; NOT the codec).
    pub format_tag: u32,
    /// RADP blocks per stream = `data_size / (20 · ver)` (integral on
    /// every real row — enforced by construction, trusted here).
    pub blocks: u32,
}

/// The parsed index: entries in file order plus a clip multi-map.
#[derive(Debug, Clone)]
pub struct ShardIndex {
    entries: Vec<ShardEntry>,
    by_clip: BTreeMap<String, Vec<usize>>,
}

impl Default for ShardIndex {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            by_clip: BTreeMap::new(),
        }
    }
}

impl ShardIndex {
    /// Parse the committed TSV (or any document in the same shape).
    /// Fail-closed: comment lines (`# …`) are skipped, the first
    /// content line must be [`INDEX_COLUMNS`], every row must carry
    /// exactly 9 well-formed fields, members must sit in the PROVEN
    /// shard family, and `(archive, member)` must be unique.  The
    /// same clip name may legitimately map to several members (the
    /// data model is a multi-map; the real archive happens to be a
    /// bijection — pinned by test).
    pub fn parse_tsv(text: &str) -> Result<Self, String> {
        let mut entries: Vec<ShardEntry> = Vec::new();
        let mut seen_members: HashSet<(String, String)> = HashSet::new();
        let mut header_seen = false;
        for (lineno, raw) in text.lines().enumerate() {
            let line = raw.trim_end_matches('\r');
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if !header_seen {
                if line != INDEX_COLUMNS {
                    return Err(format!(
                        "line {}: expected column header {INDEX_COLUMNS:?}, got {line:?}",
                        lineno + 1
                    ));
                }
                header_seen = true;
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() != 9 {
                return Err(format!(
                    "line {}: expected 9 tab-separated columns, got {}",
                    lineno + 1,
                    f.len()
                ));
            }
            if f[0].is_empty() {
                return Err(format!("line {}: empty clip name", lineno + 1));
            }
            let archive = ArchiveId::from_file_name(f[1]).ok_or_else(|| {
                format!("line {}: unknown archive {:?}", lineno + 1, f[1])
            })?;
            if !f[2].starts_with(SHARD_FAMILY_PREFIX) || !f[2].ends_with(".p3d") {
                return Err(format!(
                    "line {}: member {:?} is not a {SHARD_FAMILY_PREFIX}*.p3d path",
                    lineno + 1,
                    f[2]
                ));
            }
            let member = f[2].to_string();
            if !seen_members.insert((f[1].to_ascii_lowercase(), member.clone())) {
                return Err(format!(
                    "line {}: duplicate member {:?} {:?}",
                    lineno + 1,
                    f[1],
                    f[2]
                ));
            }
            let numeric = |field: &str, what: &str| -> Result<u32, String> {
                field
                    .parse::<u32>()
                    .map_err(|_| format!("line {}: bad {what} {field:?}", lineno + 1))
            };
            let offset = numeric(f[3], "offset")?;
            let size = numeric(f[4], "size")?;
            let radp_version = numeric(f[5], "ver")?;
            let sample_rate = numeric(f[6], "rate")?;
            let format_tag = numeric(f[7], "tag")?;
            let blocks = numeric(f[8], "blocks")?;
            if offset == 0 || size == 0 || radp_version == 0 || sample_rate == 0 || blocks == 0 {
                return Err(format!("line {}: zero span/rate/ver/blocks", lineno + 1));
            }
            entries.push(ShardEntry {
                clip: f[0].to_string(),
                archive,
                member,
                offset,
                size,
                radp_version,
                sample_rate,
                format_tag,
                blocks,
            });
        }
        if !header_seen {
            return Err("no column header found".to_string());
        }
        let mut by_clip: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, e) in entries.iter().enumerate() {
            by_clip.entry(e.clip.clone()).or_default().push(i);
        }
        Ok(Self { entries, by_clip })
    }

    /// Number of indexed members.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when the index carries no members.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// All entries in file order.
    pub fn entries(&self) -> &[ShardEntry] {
        &self.entries
    }

    /// Exact-name lookup (the real vocabulary is lowercase; this is
    /// the fast path).  Returns the hits in file order — a `Vec` of
    /// entries rather than a slice because the multi-map indices are
    /// not required to be contiguous by the data model (they are, in
    /// the real archive, by construction).
    pub fn lookup(&self, clip: &str) -> Option<Vec<&ShardEntry>> {
        self.by_clip
            .get(clip)
            .map(|idx| idx.iter().map(|&i| &self.entries[i]).collect())
    }

    /// Case-insensitive lookup — documented DESIGN (the shipped
    /// vocabulary is lowercase; the engine's own name handling is
    /// UNKNOWN, so the app treats this as a convenience fallback,
    /// never as format evidence).
    pub fn lookup_normalized(&self, clip: &str) -> Option<Vec<&ShardEntry>> {
        if let Some(hit) = self.lookup(clip) {
            return Some(hit);
        }
        let lower = clip.to_ascii_lowercase();
        self.by_clip
            .iter()
            .find(|(name, _)| name.to_ascii_lowercase() == lower)
            .and_then(|(name, _)| self.lookup(name))
    }

    /// Members per archive (census helper).
    pub fn archive_counts(&self) -> [(ArchiveId, usize); 2] {
        let mut a00 = 0;
        let mut a01 = 0;
        for e in &self.entries {
            match e.archive {
                ArchiveId::A00 => a00 += 1,
                ArchiveId::A01 => a01 += 1,
            }
        }
        [(ArchiveId::A00, a00), (ArchiveId::A01, a01)]
    }

    /// RADP stream-count census (`ver` → member count).
    pub fn version_census(&self) -> BTreeMap<u32, usize> {
        let mut out: BTreeMap<u32, usize> = BTreeMap::new();
        for e in &self.entries {
            *out.entry(e.radp_version).or_default() += 1;
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Tests — the COMMITTED REAL INDEX (mined from the live release archives)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../prototype-bin/tests/fixtures/audio_shard_index.tsv"
    ));

    fn real_index() -> ShardIndex {
        ShardIndex::parse_tsv(INDEX).expect("the committed index parses")
    }

    /// The whole-archive census, pinned from the block-63 sweep (the
    /// numbers are the sweep's output, not invented).
    #[test]
    fn parses_the_committed_real_index_with_the_swept_census() {
        let idx = real_index();
        assert_eq!(idx.len(), 14_572, "every shard member is indexed");
        let counts = idx.archive_counts();
        assert_eq!(counts[0], (ArchiveId::A00, 13_465), "00audio shards (block-61 census)");
        assert_eq!(counts[1], (ArchiveId::A01, 1_107), "01audio shards (block-61 census)");
        assert_eq!(
            idx.version_census(),
            BTreeMap::from([
                (1u32, 14_318usize),
                (2, 203),
                (6, 47),
                (4, 3),
                (18, 1),
            ]),
            "RADP stream-count census: all hash-shards v1 mono; the semantic-named beds are the multi-stream dialects"
        );
        // Bijective: 14 572 unique clip names over 14 572 members.
        let unique: HashSet<&str> = idx.entries().iter().map(|e| e.clip.as_str()).collect();
        assert_eq!(unique.len(), idx.len(), "no duplicate clip names");
        // The 32 kHz mono SFX vocabulary dominates; the beds are 48 kHz.
        let rate_32k = idx
            .entries()
            .iter()
            .filter(|e| e.sample_rate == 32_000)
            .count();
        let rate_48k = idx
            .entries()
            .iter()
            .filter(|e| e.sample_rate == 48_000)
            .count();
        assert_eq!((rate_32k, rate_48k), (14_321, 251), "rate census");
    }

    /// The two byte-identity anchor shards (committed as fixtures and
    /// Rust-parsed since block 61) keep their exact spans and header
    /// fields in the index.
    #[test]
    fn serves_the_two_byte_identity_anchor_shards() {
        let idx = real_index();
        let cough = idx.lookup("20915_p_mtm_cough_d").expect("anchor present");
        assert_eq!(cough.len(), 1);
        assert_eq!(cough[0].archive, ArchiveId::A00);
        assert_eq!(cough[0].member, "audio\\english\\AudioFile\\c\\6\\c6c7ad87.p3d");
        assert_eq!(cough[0].offset, 702_814_208);
        assert_eq!(cough[0].size, 4_683);
        assert_eq!(cough[0].radp_version, 1);
        assert_eq!(cough[0].sample_rate, 32_000);
        assert_eq!(cough[0].blocks, 214);

        let maoff = idx.lookup("11271_maoff_142").expect("anchor present");
        assert_eq!(maoff.len(), 1);
        assert_eq!(maoff[0].archive, ArchiveId::A01);
        assert_eq!(maoff[0].member, "audio\\english\\AudioFile\\f\\3\\f35c7b78.p3d");
        assert_eq!(maoff[0].offset, 542_126_080);
        assert_eq!(maoff[0].size, 4_023);
        assert_eq!(maoff[0].blocks, 183);
    }

    /// The exotic members are reachable: the 18-channel ambience bake,
    /// a 5.1 mix, and a stereo music bed.
    #[test]
    fn serves_the_exotic_multi_stream_beds() {
        let idx = real_index();
        let baked = idx.lookup("ambience_baked_18ch").expect("present");
        assert_eq!(baked[0].radp_version, 18, "the name says it");
        assert_eq!(baked[0].sample_rate, 32_000);
        assert_eq!(baked[0].archive, ArchiveId::A00);
        assert!(baked[0].size > 60_000_000, "the big bake: {}", baked[0].size);

        let bed = idx.lookup("pause_menu_music").expect("present");
        assert_eq!(bed[0].radp_version, 2, "stereo bed");
        assert_eq!(bed[0].sample_rate, 48_000);
        assert_eq!(bed[0].archive, ArchiveId::A01);
        assert_eq!(bed[0].blocks, 96_000);
    }

    /// A miss is a clean None; the case-insensitive fallback finds
    /// what exact lookup misses (documented DESIGN, not format
    /// evidence).
    #[test]
    fn misses_are_clean_and_the_normalized_fallback_is_design() {
        let idx = real_index();
        assert!(idx.lookup("no_such_clip_anywhere").is_none());
        assert!(
            idx.lookup("20915_P_MTM_COUGH_D").is_none(),
            "exact lookup is case-sensitive"
        );
        let norm = idx
            .lookup_normalized("20915_P_MTM_COUGH_D")
            .expect("normalized fallback hits");
        assert_eq!(norm[0].clip, "20915_p_mtm_cough_d");
    }

    /// Fail-closed parsing: the malformed documents are refused with
    /// a line-numbered diagnostic.
    #[test]
    fn refuses_malformed_documents() {
        let header = INDEX_COLUMNS;
        let good_row = "20915_p_mtm_cough_d\t00audio.rcf\t"
            .to_string()
            + "audio\\english\\AudioFile\\c\\6\\c6c7ad87.p3d\t702814208\t4683\t1\t32000\t17\t214";
        let cases: Vec<(&str, String, &str)> = vec![
            (
                "wrong header",
                format!("clip\tarchive\n{good_row}"),
                "expected column header",
            ),
            ("short row", format!("{header}\na\tb\n"), "9 tab-separated"),
            (
                "unknown archive",
                format!("{header}\n20915_x\t02audio.rcf\taudio\\english\\AudioFile\\a.p3d\t1\t2\t1\t32000\t0\t1"),
                "unknown archive",
            ),
            (
                "member outside the family",
                format!("{header}\n20915_x\t00audio.rcf\tsome\\other.p3d\t1\t2\t1\t32000\t0\t1"),
                "not a audio\\english",
            ),
            (
                "bad number",
                format!("{header}\n20915_x\t00audio.rcf\taudio\\english\\AudioFile\\a.p3d\tX\t2\t1\t32000\t0\t1"),
                "bad offset",
            ),
            (
                "duplicate member",
                format!("{header}\n{good_row}\n{good_row}"),
                "duplicate member",
            ),
            (
                "no header at all",
                "just some text".to_string(),
                "expected column header",
            ),
        ];
        for (tag, doc, expect) in cases {
            let err = ShardIndex::parse_tsv(&doc).expect_err(tag);
            assert!(err.contains(expect), "{tag}: {err}");
        }
        // A comment-only document has no header either…
        let err = ShardIndex::parse_tsv("# provenance header only\n").expect_err("comments");
        assert!(err.contains("no column header"), "{err}");
        // …but a header with zero rows is a valid (empty) index.
        let empty = ShardIndex::parse_tsv(INDEX_COLUMNS).expect("empty index");
        assert!(empty.is_empty());
        // The good row alone parses.
        let ok = ShardIndex::parse_tsv(&format!("{header}\n{good_row}")).expect("ok");
        assert_eq!(ok.len(), 1);
    }
}

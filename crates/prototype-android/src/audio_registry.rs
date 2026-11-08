//! Serving the gameplay clip registry from the user's own AUDIO
//! ARCHIVES by NAME (block 62).
//! ============================================================
//!
//! Block 58's `GameplaySounds` registry loads from LOOSE `*.p3d`
//! files (`GameFileSet::p3d_files`).  But the loader contract ships
//! everything inside `.rcf` archives — a user who copies only the
//! game's own archives gets no audio at all today.  Block 61 named
//! the two audio archives (`00audio.rcf`, `01audio.rcf`: the first
//! observed archives with a NAME TABLE) and block 61/62 proved the
//! serving path (`prototype-rcf::named::NamedArchive` over
//! `parse_directory` + the PROVEN roster + per-member span reads).
//!
//! This module is the boot-path glue: when no loose P3D yields
//! clips, open the user's audio archives and read the PROVEN member
//! `audio\boot_strap.p3d` (the block-61 byte-identity anchor — the
//! same 3 969 452-byte P3D with 70 decodable clips the loose path
//! would find) BY NAME from inside `00audio.rcf`.  Device-free and
//! fail-closed: every failure logs and returns `None` (the game
//! boots silent exactly as before, never panics).
//!
//! ## Why boot_strap only (honest scope)
//!
//! * `audio\boot_strap.p3d` is the only member PROVEN to carry the
//!   frontend/event clip vocabulary as decodable AudioFile chunks
//!   (70 clips, block 58).
//! * The 41 other top-level bank P3Ds (`ambience`, `e01..e11_music`,
//!   `front_end_sounds`, ...) parse with ZERO AudioFile chunks —
//!   Cue/patch banks in an undecoded dialect (block 61).
//! * The 14 572 sharded clip files are keyed by content hash
//!   (`<8hex>.p3d`), not by event name; their semantic clip names
//!   live INSIDE the shards.  Block 63 MINED them: the committed
//!   shard clip-name index (`prototype-rcf::shard_index`) maps all
//!   14 572 semantic names to their archive + member span, and
//!   [`ShardClipServer`] serves any of them LAZILY — lookup → span
//!   read → decode → play — from the user's own archives.

use std::path::Path;

use crate::game_files::GameFileSet;
use prototype_engine::gameplay_sounds::{GameplaySounds, SoundClip};
use prototype_rcf::named::NamedArchive;
use prototype_rcf::shard_index::ShardIndex;

/// The PROVEN archive path of the gameplay clip bank inside
/// `00audio.rcf` (block 61: name-table record 7, directory entry
/// (0xBD4FDDAC, 6 817 792, 3 969 452), byte-identity anchored
/// against the committed fixture).
pub const AUDIO_BOOTSTRAP_MEMBER: &str = "audio\\boot_strap.p3d";

/// The two audio archives by their shipped file names.
pub const AUDIO_ARCHIVE_NAMES: [&str; 2] = ["00audio.rcf", "01audio.rcf"];

/// The block-63 shard clip-name index, embedded in the binary (a
/// 1.44 MiB TSV mined from the live release archives — derived
/// metadata, NOT a game asset; the payloads still come only from the
/// user's own files).
pub const SHARD_INDEX_TSV: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../prototype-bin/tests/fixtures/audio_shard_index.tsv"
));

/// Lazy serving of ANY indexed shard clip by SEMANTIC name from the
/// user's own audio archives (block 63).  The index rides in the
/// binary; each [`ShardClipServer::load_clip`] opens the named
/// archive, reads the member span, and decodes the one-clip P3D
/// through the PROVEN registry path.  Fail-closed: `Ok(None)` when
/// the clip is not indexed or nothing decoded (the caller degrades
/// to silence), `Err(String)` carries accumulated diagnostics.
pub struct ShardClipServer {
    index: ShardIndex,
}

impl ShardClipServer {
    /// Build from the embedded index (parse once, ~ms for 14 572 rows).
    pub fn embedded() -> Self {
        Self::from_tsv(SHARD_INDEX_TSV)
    }

    /// Build from any TSV in the committed shape (fail-closed).
    pub fn from_tsv(tsv: &str) -> Self {
        match ShardIndex::parse_tsv(tsv) {
            Ok(index) => Self { index },
            // The embedded index is compile-time data that its own
            // tests pin; a caller-supplied document that fails to
            // parse degrades to an empty index rather than a panic.
            Err(e) => {
                log::warn!("shard clip server: index refused ({e}) — serving empty");
                Self {
                    index: ShardIndex::default(),
                }
            }
        }
    }

    /// The number of indexed shards.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// True when the index is empty.
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Whether a clip is indexed (no IO).
    pub fn knows(&self, clip: &str) -> bool {
        self.index.lookup(clip).is_some()
    }

    /// Load `clip` from the user's own archives.  `Ok(None)` when the
    /// clip is not indexed; `Ok(Some(..))` when a member decoded;
    /// `Err` when the clip IS indexed but every attempt failed (the
    /// diagnostics name the archive).
    pub fn load_clip(&self, set: &GameFileSet, clip: &str) -> Result<Option<SoundClip>, String> {
        let entries = match self.index.lookup(clip) {
            Some(e) => e,
            None => return Ok(None),
        };
        let mut diagnostics: Vec<String> = Vec::new();
        for entry in entries {
            // Locate the user's archive file for this entry.
            let path = set.rcf_archives.iter().find(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().eq_ignore_ascii_case(entry.archive.file_name()))
                    .unwrap_or(false)
            });
            let Some(path) = path else {
                diagnostics.push(format!(
                    "{} not present in the game folder",
                    entry.archive.file_name()
                ));
                continue;
            };
            let file = match std::fs::File::open(path) {
                Ok(f) => f,
                Err(e) => {
                    diagnostics.push(format!("{}: open failed ({e})", entry.archive.file_name()));
                    continue;
                }
            };
            let mut named = match NamedArchive::open(file) {
                Ok(n) => n,
                Err(e) => {
                    diagnostics.push(format!(
                        "{}: not a named archive ({e})",
                        entry.archive.file_name()
                    ));
                    continue;
                }
            };
            let bytes = match named.read_member(&entry.member) {
                Ok(b) => b,
                Err(e) => {
                    diagnostics.push(format!(
                        "{}: read {} failed ({e})",
                        entry.archive.file_name(),
                        entry.member
                    ));
                    continue;
                }
            };
            match GameplaySounds::from_p3d_bytes(&bytes, &entry.member) {
                Ok(sounds) => match sounds.get(clip) {
                    Some(c) if !c.samples.is_empty() => {
                        log::info!(
                            "shard clip: {clip:?} served from {} ({} {}, {} B, {} Hz x {} ch, rate-clean on play)",
                            entry.archive.file_name(),
                            entry.member,
                            entry.offset,
                            bytes.len(),
                            entry.sample_rate,
                            entry.radp_version,
                        );
                        return Ok(Some(c.clone()));
                    }
                    _ => diagnostics.push(format!(
                        "{}: {} decoded without clip {clip:?}",
                        entry.archive.file_name(),
                        entry.member
                    )),
                },
                Err(e) => diagnostics.push(format!(
                    "{}: {} failed to parse ({e})",
                    entry.archive.file_name(),
                    entry.member
                )),
            }
        }
        Err(diagnostics.join("; "))
    }
}

/// Try to load the gameplay clip registry from the user's own audio
/// archives, served BY NAME.  `Ok(Some(..))` when a member decoded
/// into clips; `Ok(None)` when nothing usable was found (the caller
/// boots silent); `Err(String)` carries the last diagnostic.
pub fn load_registry_from_archives(set: &GameFileSet) -> Result<Option<GameplaySounds>, String> {
    let mut diagnostics: Vec<String> = Vec::new();
    for rcf in &set.rcf_archives {
        let fname = file_name_lower(rcf);
        if !AUDIO_ARCHIVE_NAMES.iter().any(|a| fname.eq_ignore_ascii_case(a)) {
            continue;
        }
        let file = match std::fs::File::open(rcf) {
            Ok(f) => f,
            Err(e) => {
                diagnostics.push(format!("{}: open failed ({e})", fname));
                continue;
            }
        };
        let mut named = match NamedArchive::open(file) {
            Ok(n) => n,
            Err(e) => {
                diagnostics.push(format!("{}: not a named archive ({e})", fname));
                continue;
            }
        };
        let span = named.lookup(AUDIO_BOOTSTRAP_MEMBER).map(|m| {
            format!("offset {} size {}", m.file_offset, m.file_size)
        });
        let bytes = match named.read_member(AUDIO_BOOTSTRAP_MEMBER) {
            Ok(b) => b,
            Err(e) => {
                diagnostics.push(format!("{}: {AUDIO_BOOTSTRAP_MEMBER} read failed ({e})", fname));
                continue;
            }
        };
        match GameplaySounds::from_p3d_bytes(&bytes, &fname) {
            Ok(sounds) if !sounds.is_empty() => {
                log::info!(
                    "gameplay audio: {} clip(s) served BY NAME from {} ({AUDIO_BOOTSTRAP_MEMBER}, {} B, {} undecodable chunk(s))",
                    sounds.len(),
                    fname,
                    bytes.len(),
                    sounds.decode_failures(),
                );
                return Ok(Some(sounds));
            }
            Ok(_) => diagnostics.push(format!(
                "{fname}: {AUDIO_BOOTSTRAP_MEMBER} decoded to zero clips"
            )),
            Err(e) => diagnostics.push(format!("{fname}: {AUDIO_BOOTSTRAP_MEMBER}: {e}")),
        }
        let _ = span; // the span is logged only on success above via bytes.len()
    }
    if diagnostics.is_empty() {
        Ok(None)
    } else {
        Err(diagnostics.join("; "))
    }
}

fn file_name_lower(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn fixture(name: &str) -> Vec<u8> {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    /// Build a minimal but VALID named cement archive (the PROVEN
    /// layout — the same construction `prototype-rcf::named`'s tests
    /// use): header, directory, name-table gap, payloads.
    fn synthesize_named_archive(members: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let sector = 2048u32;
        let dir_offset = 60u32;
        let dir_size = (members.len() * 12) as u32;
        let mut table = Vec::new();
        table.extend_from_slice(&0x800u32.to_le_bytes());
        table.extend_from_slice(&0u32.to_le_bytes());
        for (name, _) in members {
            let nb = name.as_bytes();
            let len = nb.len() + 1;
            table.extend_from_slice(&0x688D_0000u32.to_le_bytes());
            table.extend_from_slice(&0x800u32.to_le_bytes());
            table.extend_from_slice(&0u32.to_le_bytes());
            table.extend_from_slice(&(len as u32).to_le_bytes());
            let mut field = vec![0u8; len];
            field[..nb.len()].copy_from_slice(nb);
            table.extend_from_slice(&field);
            table.extend_from_slice(&[0u8; 3]);
        }
        let payload_start =
            (dir_offset + dir_size + table.len() as u32).div_ceil(sector) * sector;
        let mut out = Vec::new();
        let mut magic = vec![0u8; 32];
        let magic_bytes = prototype_rcf::RCF_MAGIC_ASCII.as_bytes();
        magic[..magic_bytes.len()].copy_from_slice(magic_bytes);
        out.extend_from_slice(&magic);
        out.extend_from_slice(&prototype_rcf::RCF_VERSION_FIELD.to_le_bytes());
        out.extend_from_slice(&dir_offset.to_le_bytes());
        out.extend_from_slice(&dir_size.to_le_bytes());
        out.extend_from_slice(&sector.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(members.len() as u32).to_le_bytes());
        for ((name, _), (off, len)) in members.iter().zip(members.iter().scan(
            payload_start,
            |st, (_, data)| {
                let off = *st;
                *st += data.len() as u32;
                Some((off, data.len() as u32))
            },
        )) {
            out.extend_from_slice(&prototype_rcf::hash_name(name).to_le_bytes());
            out.extend_from_slice(&off.to_le_bytes());
            out.extend_from_slice(&len.to_le_bytes());
        }
        out.extend_from_slice(&table);
        out.resize(payload_start as usize, 0);
        for (_, data) in members {
            out.extend_from_slice(data);
        }
        out
    }

    fn folder_with(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = root.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "proto-audio-registry-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn serves_the_clip_registry_from_00audio_by_name() {
        let root = temp_root("serve");
        let boot = fixture("boot_strap.p3d");
        folder_with(
            &root,
            "00audio.rcf",
            &synthesize_named_archive(&[(AUDIO_BOOTSTRAP_MEMBER, boot)]),
        );
        folder_with(&root, "01audio.rcf", b"placeholder");
        let set = crate::game_files::scan_folder(&root).unwrap();
        assert_eq!(set.rcf_archives.len(), 2);
        let reg = load_registry_from_archives(&set)
            .expect("no error")
            .expect("registry served BY NAME from the archive");
        assert_eq!(reg.len(), 70, "boot_strap's PROVEN clip count");
        assert_eq!(reg.source(), "00audio.rcf");
        assert!(reg.has("ui_fe_ht_back"), "the event vocabulary rides along");
    }

    #[test]
    fn non_audio_archives_are_ignored() {
        let root = temp_root("ignore");
        folder_with(&root, "00cells.rcf", b"junk");
        folder_with(&root, "movies.rcf", b"junk");
        let set = crate::game_files::scan_folder(&root).unwrap();
        assert!(
            matches!(load_registry_from_archives(&set), Ok(None)),
            "non-audio archives must be a clean miss"
        );
    }

    #[test]
    fn a_corrupt_archive_fails_closed_with_diagnostics() {
        let root = temp_root("corrupt");
        folder_with(&root, "00audio.rcf", b"not a cement archive at all");
        let set = crate::game_files::scan_folder(&root).unwrap();
        let err = load_registry_from_archives(&set).unwrap_err();
        assert!(err.contains("00audio.rcf"), "diagnostic names the file: {err}");
    }

    #[test]
    fn an_archive_without_the_member_reports_and_misses() {
        let root = temp_root("nomember");
        folder_with(
            &root,
            "00audio.rcf",
            &synthesize_named_archive(&[("audio\\other.p3d", b"placeholder".to_vec())]),
        );
        let set = crate::game_files::scan_folder(&root).unwrap();
        let err = load_registry_from_archives(&set).unwrap_err();
        assert!(
            err.contains("no member"),
            "the diagnostic names the missing member: {err}"
        );
    }

    // -------------------------------------------------------------------
    // Block 63: the shard clip server (the embedded block-63 index)
    // -------------------------------------------------------------------

    /// The embedded index carries the full mined vocabulary.
    #[test]
    fn the_embedded_index_serves_the_full_vocabulary() {
        let server = ShardClipServer::embedded();
        assert_eq!(server.len(), 14_572, "the block-63 sweep census");
        assert!(server.knows("20915_p_mtm_cough_d"));
        assert!(server.knows("11271_maoff_142"));
        assert!(server.knows("pause_menu_music"));
        assert!(!server.knows("definitely_not_a_clip"));
    }

    /// A REAL shard member is served lazily from the user's own
    /// archive: the byte-identity anchor fixture rides a synthesized
    /// named archive at its REAL roster path, and the server returns
    /// the decoded clip with its PROVEN header fields.
    #[test]
    fn load_clip_serves_a_real_shard_from_the_users_archive() {
        let root = temp_root("shard");
        let shard_bytes = fixture("audio_member_c6c7ad87.p3d");
        folder_with(
            &root,
            "00audio.rcf",
            &synthesize_named_archive(&[(
                "audio\\english\\AudioFile\\c\\6\\c6c7ad87.p3d",
                shard_bytes,
            )]),
        );
        let set = crate::game_files::scan_folder(&root).unwrap();
        let server = ShardClipServer::embedded();
        let clip = server
            .load_clip(&set, "20915_p_mtm_cough_d")
            .expect("no error")
            .expect("the clip is served");
        assert_eq!(clip.name, "20915_p_mtm_cough_d");
        assert_eq!(clip.channels, 1, "RADP v1 is mono");
        assert_eq!(clip.sample_rate, 32_000, "the shard rate");
        assert_eq!(clip.samples.len(), 214 * 32, "blocks x 32 (PROVEN v1)");
    }

    /// A clip that is not in the index is a clean `Ok(None)` — no IO.
    #[test]
    fn an_unindexed_clip_is_a_clean_none() {
        let root = temp_root("unindexed");
        folder_with(&root, "00audio.rcf", b"placeholder");
        let set = crate::game_files::scan_folder(&root).unwrap();
        let server = ShardClipServer::embedded();
        assert!(matches!(
            server.load_clip(&set, "definitely_not_a_clip"),
            Ok(None)
        ));
    }

    /// An INDEXED clip whose archive is missing fails closed with the
    /// archive named in the diagnostics (never a panic, never silence
    /// disguised as success).
    #[test]
    fn an_indexed_clip_without_its_archive_fails_closed() {
        let root = temp_root("noarchive");
        folder_with(&root, "01audio.rcf", b"placeholder");
        let set = crate::game_files::scan_folder(&root).unwrap();
        let server = ShardClipServer::embedded();
        let err = server
            .load_clip(&set, "20915_p_mtm_cough_d")
            .expect_err("indexed but the archive is absent");
        assert!(
            err.contains("00audio.rcf") && err.contains("not present"),
            "the diagnostic names the absent archive: {err}"
        );
    }
}

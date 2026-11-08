//! The mission MUSIC GRAPH, decoded from the user's own audio banks
//! (block 66).
//! ============================================================
//!
//! Block 65 PROVED the Cue/Patch descriptor dialect of the game's
//! audio banks byte-exact on all 325 bodies of the 12 music/ambience
//! banks, and pinned the full mission music chain:
//!
//! ```text
//! mission-id Cue (e.g. "e10m04")
//!   └─ transitions[i]  ↔  cells[i]          (len equality PROVEN)
//!        └─ cell.stem  (e.g. "e10m04_ambient"; EmptyCell → none)
//!             └─ BankPatch with that object name
//!                  └─ filename / filenames property values
//!                       = EXACT clip names in the block-63 shard
//!                         index (245/245 PROVEN)
//! ```
//!
//! This module is the boot-path glue for that chain: open the
//! user's own `00audio.rcf` (case-insensitive, the loader contract),
//! read the 12 PROVEN music/ambience bank members by name, decode
//! every `Patch`/`Cue` classic chunk through
//! `prototype_p3d::bank::parse_body`, and expose the graph:
//!
//! * [`MusicGraph::stem_for`] — mission id + transition name → stem
//!   (or `None` for an `EmptyCell`, honestly).
//! * [`MusicGraph::clips_for`] — mission id + transition → the
//!   ordered clip names the stem's patch plays (single `filename`
//!   or the numbered `filenames` variants of a `music_player_random`
//!   player, shipped order preserved).
//!
//! The clips themselves are NOT shipped or decoded here: the app
//! serves them from the user's own archives through
//! [`crate::audio_registry::ShardClipServer`] (block 63) and spawns
//! them rate-clean through `GameplaySounds::play_clip` (block 58/59)
//! — this module only turns a mission/transition pair into the
//! right clip names.
//!
//! # Fail-closed discipline
//!
//! * No `00audio.rcf` in the set → [`MusicGraph::from_game_set`]
//!   returns `Err` (the caller logs and the game stays silent, as
//!   before block 66).
//! * A corrupt archive → `Err`.  A corrupt BANK or an unparseable
//!   chunk inside a bank → counted in [`MusicGraph::parse_failures`]
//!   and skipped, never fatal, never guessed.
//! * Missing members degrade to a partial graph (a user with a
//!   partial install still gets every bank that is present).
//! * `boot_strap.p3d` IS part of the graph (read last by
//!   [`MusicGraph::from_game_set`] — the app's PROVEN audio member):
//!   its 152 `Patch` chunks and TWO music cues (`front_end` = the
//!   frontend attract graph, `clear_cue`) decode with the SAME dialect
//!   — block 66 extended the bank grammar with the three cases the
//!   music-only corpus never exercised (the `Vector` type, empty
//!   String values / empty String Lists, the head flag 0x00|0x01), and
//!   ALL 154 boot_strap bodies now decode byte-exact, zero refusals.
//!
//! Everything here is device-free std-file-system logic and is
//! unit-tested on every host against the three REAL bank fixtures
//! committed in block 65 (`bank_e10_music`, `bank_e02_music`,
//! `bank_ambience`).

use std::collections::BTreeMap;
use std::path::Path;

use crate::game_files::GameFileSet;
use prototype_p3d::bank::{
    parse_body, BankChunk, BankCue, BankPatch, BankValue, CUE_CHUNK_NAME, PATCH_CHUNK_NAME,
};
use prototype_p3d::{ChunkFamily, P3dFile};
use prototype_rcf::named::NamedArchive;

/// The archive that carries the music/ambience banks (block 61/65:
/// the 12 members below are all in `00audio.rcf`; the 20 members of
/// `01audio.rcf` are shard payloads only).
pub const MUSIC_ARCHIVE_NAME: &str = "00audio.rcf";

/// The PROVEN member paths of the 12 music/ambience banks inside
/// [`MUSIC_ARCHIVE_NAME`] (block 61 name table; block 65 fetched all
/// 12 whole by these exact spans with `file_size` self-validation).
pub const MUSIC_BANK_MEMBERS: [&str; 12] = [
    "audio\\ambience.p3d",
    "audio\\e01_music.p3d",
    "audio\\e02_music.p3d",
    "audio\\e03_music.p3d",
    "audio\\e04_music.p3d",
    "audio\\e05_music.p3d",
    "audio\\e06_music.p3d",
    "audio\\e07_music.p3d",
    "audio\\e08_music.p3d",
    "audio\\e09_music.p3d",
    "audio\\e10_music.p3d",
    "audio\\e11_music.p3d",
];

/// The AUDIO descriptor of one NIS cutscene ([`MusicGraph::nis_mix`],
/// block 67): the mix clip names to serve, the patch's own `volume`,
/// and the `mixbuss` routing labels.  Block 72: the labels are no
/// longer informational only — the app routes the mix to the `nis`
/// bus (the family's set `[nis, master, dialog]`; see
/// [`bus_list`] for the census and [`crate::app`] for the family
/// mapping).
#[derive(Debug, Clone, PartialEq)]
pub struct NisMix {
    /// The patch's `filename` clip references, shipped order
    /// (single clip for every NIS patch in the corpus).
    pub clips: Vec<String>,
    /// The patch's `volume` Float32 (1.0 across the 46 NIS patches).
    pub volume: f32,
    /// The patch's `mixbuss` StrList (e.g. `[nis, master, dialog]`).
    pub buses: Vec<String>,
}

/// The decoded mission music graph of the user's own audio banks.
///
/// Cues are keyed by mission id (`e10m04`), patches by object/stem
/// name (`e10m04_ambient`).  Duplicate object names across banks
/// overwrite by insertion order (member order below) — the corpus
/// has no cross-bank cue collisions (block 65 mined 51 distinct cue
/// objects), and a collision would be visible in the boot log
/// counts either way.
#[derive(Debug, Clone, Default)]
pub struct MusicGraph {
    cues: BTreeMap<String, BankCue>,
    patches: BTreeMap<String, BankPatch>,
    banks_loaded: usize,
    chunks_decoded: usize,
    parse_failures: Vec<String>,
}

impl MusicGraph {
    /// An empty graph (the pre-block-66 state).
    pub fn empty() -> Self {
        Self::default()
    }

    /// True when nothing decoded (no cues AND no patches).
    pub fn is_empty(&self) -> bool {
        self.cues.is_empty() && self.patches.is_empty()
    }

    /// Number of cue chunks decoded (one per mission music graph —
    /// 5 over the three committed fixtures).
    pub fn cue_count(&self) -> usize {
        self.cues.len()
    }

    /// Number of patch chunks decoded (stem players + ambience
    /// players — 30 over the three committed fixtures).
    pub fn patch_count(&self) -> usize {
        self.patches.len()
    }

    /// Number of banks successfully opened and scanned (P3D parse
    /// passed), regardless of how many chunks each yielded.
    pub fn banks_loaded(&self) -> usize {
        self.banks_loaded
    }

    /// Total `Patch` + `Cue` chunks decoded across all banks.
    pub fn chunks_decoded(&self) -> usize {
        self.chunks_decoded
    }

    /// Diagnostics for chunks that were present but refused —
    /// counted, never fatal, never silent (the boot log carries
    /// these).
    pub fn parse_failures(&self) -> &[String] {
        &self.parse_failures
    }

    /// The cue of one mission id (e.g. `"e10m04"`).
    pub fn cue(&self, mission: &str) -> Option<&BankCue> {
        self.cues.get(mission)
    }

    /// All decoded cues, ordered by mission id.
    pub fn cues(&self) -> impl Iterator<Item = (&str, &BankCue)> {
        self.cues.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// The patch (stem player) with this object name.
    pub fn patch(&self, stem: &str) -> Option<&BankPatch> {
        self.patches.get(stem)
    }

    /// All decoded patches, ordered by object name.
    pub fn patches(&self) -> impl Iterator<Item = (&str, &BankPatch)> {
        self.patches.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// The stem one mission transition plays: `mission` → cue →
    /// `transition` (case-insensitive position match, PROVEN
    /// `transitions[i] ↔ cells[i]` pairing) → cell stem.
    /// `None` for an unknown mission/transition pair and for an
    /// `EmptyCell` (e.g. `e02m02_fr`'s `init` — PROVEN).
    pub fn stem_for(&self, mission: &str, transition: &str) -> Option<&str> {
        let cue = self.cues.get(mission)?;
        let idx = cue
            .transitions
            .iter()
            .position(|t| t.eq_ignore_ascii_case(transition))?;
        cue.cells.get(idx)?.stem.as_deref()
    }

    /// The ordered clip names one stem's patch plays: its
    /// `filename` property (single player) or its `filenames` list
    /// (a `music_player_random` player — shipped order preserved,
    /// PROVEN `e10m04_complete → [complete02, complete01]`).
    /// Empty when the stem is unknown or its patch carries no clip
    /// reference (an honest degrade, never a guess).
    pub fn clips_for_stem(&self, stem: &str) -> Vec<String> {
        let Some(patch) = self.patches.get(stem) else {
            return Vec::new();
        };
        let mut out: Vec<String> = Vec::new();
        for clip in patch.clip_names() {
            if !out.iter().any(|c| c == clip) {
                out.push(clip.to_string());
            }
        }
        out
    }

    /// Does one stem's patch carry the `loop` property set (block
    /// 68)?  PROVEN data: every `music_player`/`music_player_random`
    /// patch across the committed banks (e02, e10, boot_strap's
    /// `fe_music`) carries an explicit `loop` Bool — beds
    /// (ambient/suspense/combat) true, stingers (death/complete/
    /// establish) false.  `false` when the stem is unknown or the
    /// prop is absent: a missing claim plays once, never loops on a
    /// guess.
    pub fn stem_loops(&self, stem: &str) -> bool {
        match self.patches.get(stem).and_then(|p| p.prop("loop")) {
            Some(BankValue::Bool(b)) => *b,
            _ => false,
        }
    }

    /// The ordered clip names one mission transition plays — the
    /// full PROVEN chain end to end:
    /// `mission-id cue → transition → stem → patch → clip names`.
    pub fn clips_for(&self, mission: &str, transition: &str) -> Vec<String> {
        match self.stem_for(mission, transition) {
            Some(stem) => self.clips_for_stem(stem),
            None => Vec::new(),
        }
    }

    /// The AUDIO descriptor of one NIS cutscene (block 67): its
    /// `NIS_Player` patch by object name (`mnis5_2_2`, `mb_3_1_1`,
    /// `nis0-3a` — the 46 objects of `boot_strap.p3d`, block 66).
    ///
    /// PROVEN shape of every NIS patch (46/46, block 66 + this
    /// block's fixture cross-check): `volume` (Float32), `filename`
    /// (the movie's pre-mixed clip — `mnis_5_2_2_mix`, with `.wav`
    /// on the three audio-carrying intro movies), `mixbuss`
    /// (StrList, e.g. `[nis, master, dialog]`).  The clip names are
    /// exact shard-index entries (block 63/64: RADP v6 mixes,
    /// 48 kHz) served by [`crate::audio_registry::ShardClipServer`].
    ///
    /// `None` when the object is not a decoded patch — an honest
    /// miss, never a guess.  `volume` defaults to 1.0 and `buses`
    /// to empty when the properties are absent (the corpus always
    /// carries both; the defaults only cover a partial patch).
    pub fn nis_mix(&self, nis_object: &str) -> Option<NisMix> {
        let patch = self.patch(nis_object)?;
        let volume = match patch.prop("volume") {
            Some(BankValue::Float32(v)) => *v,
            _ => 1.0,
        };
        let buses = match patch.prop("mixbuss") {
            Some(BankValue::StrList(v)) => v.clone(),
            _ => Vec::new(),
        };
        Some(NisMix {
            clips: self.clips_for_stem(nis_object),
            volume,
            buses,
        })
    }

    /// One-line boot-log summary.
    pub fn summary(&self) -> String {
        format!(
            "{} cue(s), {} stem patch(es) from {} bank(s) ({} chunk(s), {} parse refusal(s))",
            self.cues.len(),
            self.patches.len(),
            self.banks_loaded,
            self.chunks_decoded,
            self.parse_failures.len(),
        )
    }

    /// Decode one bank P3D (in-memory bytes) into the graph.
    ///
    /// Returns the number of `Patch`/`Cue` chunks decoded.  Per-chunk
    /// refusals are recorded in [`MusicGraph::parse_failures`] and
    /// skipped; a bank whose P3D header itself does not parse returns
    /// `Err` and leaves the graph untouched (the caller decides
    /// whether that is fatal — [`MusicGraph::from_game_set`] counts
    /// it and moves on).
    pub fn merge_bank_bytes(&mut self, bytes: &[u8], source: &str) -> Result<usize, String> {
        let file =
            P3dFile::parse(bytes).map_err(|e| format!("music graph: {source}: bad P3D: {e}"))?;
        let mut decoded = 0usize;
        for chunk in &file.chunks {
            if chunk.family != ChunkFamily::Classic {
                continue;
            }
            let classic = match chunk.classic() {
                Ok(Some(c)) => c,
                Ok(None) => continue,
                Err(e) => {
                    self.parse_failures
                        .push(format!("{source}: classic decode failed: {e}"));
                    continue;
                }
            };
            let is_patch = classic.name == PATCH_CHUNK_NAME;
            let is_cue = classic.name == CUE_CHUNK_NAME;
            if !is_patch && !is_cue {
                continue;
            }
            match parse_body(classic.data) {
                Ok(BankChunk::Patch(p)) => {
                    self.patches.insert(p.object.clone(), p);
                    decoded += 1;
                }
                Ok(BankChunk::Cue(c)) => {
                    self.cues.insert(c.object.clone(), c);
                    decoded += 1;
                }
                Err(e) => {
                    self.parse_failures
                        .push(format!("{source}: {} body refused: {e}", classic.name));
                }
            }
        }
        self.banks_loaded += 1;
        self.chunks_decoded += decoded;
        Ok(decoded)
    }

    /// Build the graph from the user's own game files: find
    /// [`MUSIC_ARCHIVE_NAME`] in the set (case-insensitive), open it
    /// as a named archive, and read every member of
    /// [`MUSIC_BANK_MEMBERS`] (exact path first, normalized lookup as
    /// the fallback).  Missing members are skipped (partial install
    /// → partial graph); a missing or corrupt archive is `Err`.
    pub fn from_game_set(set: &GameFileSet) -> Result<Self, String> {
        let path = set
            .rcf_archives
            .iter()
            .find(|p| {
                p.file_name()
                    .map(|n| {
                        n.to_string_lossy()
                            .eq_ignore_ascii_case(MUSIC_ARCHIVE_NAME)
                    })
                    .unwrap_or(false)
            })
            .ok_or_else(|| {
                format!("music graph: {MUSIC_ARCHIVE_NAME} not present in the game folder")
            })?;
        let file = std::fs::File::open(path)
            .map_err(|e| format!("music graph: {} open failed ({e})", path.display()))?;
        let mut named = NamedArchive::open(file).map_err(|e| {
            format!(
                "music graph: {} is not a named archive ({e})",
                path.display()
            )
        })?;
        let mut graph = Self::empty();
        // The 12 music/ambience banks, then the app's own audio
        // member — boot_strap.p3d's 152 patches + 2 music cues
        // (`front_end`, `clear_cue`) join the graph (block 66's
        // grammar extensions decode ALL of its descriptors).
        for member in MUSIC_BANK_MEMBERS.iter().copied().chain([BOOTSTRAP_MUSIC_MEMBER]) {
            let bytes = match named.read_member(member) {
                Ok(b) => b,
                Err(exact) => match named.read_member_normalized(member) {
                    Ok(b) => b,
                    Err(_) => {
                        log::debug!("music graph: {member} not in the archive ({exact}) — skipped");
                        continue;
                    }
                },
            };
            if let Err(e) = graph.merge_bank_bytes(&bytes, member) {
                // A bank that fails its own P3D parse is recorded and
                // the rest of the graph still loads.
                graph.parse_failures.push(e);
            }
        }
        Ok(graph)
    }
}

/// The app's PROVEN audio member inside [`MUSIC_ARCHIVE_NAME`]
/// (block 61 byte-identity anchor) — read by
/// [`MusicGraph::from_game_set`] after the 12 music banks so the
/// frontend music cue (`front_end`) and the clear/strike cue join
/// the graph.  Same path [`crate::audio_registry`] serves clips
/// from.
pub const BOOTSTRAP_MUSIC_MEMBER: &str = "audio\\boot_strap.p3d";

/// True when `path`'s file name is the music archive
/// (case-insensitive) — shared by the boot path and tests.
pub fn is_music_archive(path: &Path) -> bool {
    path.file_name()
        .map(|n| {
            n.to_string_lossy()
                .eq_ignore_ascii_case(MUSIC_ARCHIVE_NAME)
        })
        .unwrap_or(false)
}

/// Block 72 — the `mixbuss` routing evidence layer: the bus list a
/// patch carries, decoded from whichever of the FIVE prop names the
/// real banks use for it.  The PROVEN census (committed fixtures):
///
/// * `Ambience_all` (the world bed, `ambience`) — `mixbus` =
///   `[master, amb, sfx]`.
/// * `AtCamera_Player_Quad` (interior beds) — `bus` =
///   `[amb, sfx, master]`.
/// * `music_player` (mission stems) — `bus_name` =
///   `[master, music, igmusic]`.
/// * `music_player_random` (stingers) — `busnames` =
///   `[master, music, igmusic]`, and `[music, igmusic, master]` for
///   `e02m05_complete`/`e10m04_complete` — the SAME SET in a
///   different order: the ROTATION PROOF (order is not semantic;
///   the lists are sets).
/// * `NIS_Player` mixes (boot_strap) — `mixbuss` =
///   `[nis, master, dialog]` (block 66/69).
///
/// Every observed list contains `master` (PROVEN) — the mixer's
/// master gain is its model.  First prop name carrying a StrList
/// wins; a patch with none returns an empty vec — an honest miss,
/// never a guess.
pub fn bus_list(patch: &BankPatch) -> Vec<String> {
    for name in ["mixbuss", "mixbus", "bus", "bus_name", "busnames"] {
        if let Some(BankValue::StrList(v)) = patch.prop(name) {
            return v.clone();
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_registry::ShardClipServer;
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

    /// Build a minimal but VALID named cement archive — the PROVEN
    /// construction, byte-for-byte the one `audio_registry`'s tests
    /// use (header, directory, name-table gap, payloads).
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
            "proto-music-graph-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // -------------------------------------------------------------------
    // The pinned e10 graph (block 65: radius 150.0, 6 transitions,
    // 6 stems, 7 stem patches — every number below is from the REAL
    // committed fixture, decoded by the block-65 byte-exact parser).
    // -------------------------------------------------------------------

    #[test]
    fn e10_pins_the_proven_music_graph() {
        let bytes = fixture("bank_e10_music.p3d");
        let mut g = MusicGraph::empty();
        let decoded = g.merge_bank_bytes(&bytes, "audio\\e10_music.p3d").unwrap();
        assert_eq!(decoded, 8, "1 Cue + 7 Patch chunks");
        assert_eq!(g.cue_count(), 1);
        assert_eq!(g.patch_count(), 7);
        assert_eq!(g.chunks_decoded(), 8);
        assert!(g.parse_failures().is_empty());

        let cue = g.cue("e10m04").expect("the mission cue");
        assert_eq!(cue.object, "e10m04");
        assert!((cue.radius - 150.0).abs() < 1e-4, "radius {}", cue.radius);
        assert_eq!(
            cue.transitions,
            vec!["ambient", "suspense", "combat", "death", "complete", "complete_long"]
        );
        assert_eq!(cue.cells.len(), cue.transitions.len(), "PROVEN pairing");

        // Every MusicMGraphCell stem is the transition-named patch.
        for (t, cell) in cue.transitions.iter().zip(&cue.cells) {
            assert!(!cell.is_empty, "e10m04 has no EmptyCell");
            assert_eq!(
                cell.stem.as_deref(),
                Some(format!("e10m04_{t}").as_str()),
                "transition {t}"
            );
        }

        // Single-player stems resolve to exactly their own clip.
        assert_eq!(
            g.clips_for("e10m04", "ambient"),
            vec!["e10m04_ambient".to_string()]
        );
        assert_eq!(
            g.clips_for("e10m04", "death"),
            vec!["e10m04_death".to_string()]
        );
        // The random player carries the numbered variants IN SHIPPED
        // ORDER (block 65: filenames = [complete02, complete01]).
        assert_eq!(
            g.clips_for("e10m04", "complete"),
            vec![
                "e10m04_complete02".to_string(),
                "e10m04_complete01".to_string()
            ]
        );

        // Case-insensitive transition lookup; unknowns are empty.
        assert_eq!(g.stem_for("e10m04", "AMBIENT"), Some("e10m04_ambient"));
        assert_eq!(g.stem_for("e10m04", "bogus"), None);
        assert!(g.clips_for("e10m04", "bogus").is_empty());
        assert!(g.clips_for("e99m99", "ambient").is_empty());
    }

    #[test]
    fn e02_carries_the_empty_cell_and_cross_named_stems() {
        let bytes = fixture("bank_e02_music.p3d");
        let mut g = MusicGraph::empty();
        let decoded = g.merge_bank_bytes(&bytes, "audio\\e02_music.p3d").unwrap();
        assert_eq!(decoded, 24, "4 Cues + 20 Patches");
        assert_eq!(g.cue_count(), 4);
        assert_eq!(g.patch_count(), 20);

        // The localized cue keeps its own dialect: `event_start` plays
        // the COMBAT stem and `event_end` the DEATH stem (PROVEN on the
        // fixture — transition and stem names diverge), and `init` is
        // an EmptyCell with no stem at all.
        assert_eq!(g.stem_for("e02m02_fr", "event_start"), Some("e02m02_fr_combat"));
        assert_eq!(g.stem_for("e02m02_fr", "event_end"), Some("e02m02_fr_death"));
        assert_eq!(g.stem_for("e02m02_fr", "init"), None, "EmptyCell");
        assert!(g.clips_for("e02m02_fr", "init").is_empty());
        // The ambient stem's patch plays the STING variant, not its
        // own name (REAL fixture data — the block-65 variant-base
        // phenomenon: stem e02m02_fr_ambient → filename
        // e02m02_fr_ambient_sting).
        assert_eq!(
            g.clips_for("e02m02_fr", "ambient"),
            vec!["e02m02_fr_ambient_sting".to_string()]
        );
    }

    #[test]
    fn ambience_is_a_cueless_patch_bank() {
        let bytes = fixture("bank_ambience.p3d");
        let mut g = MusicGraph::empty();
        let decoded = g.merge_bank_bytes(&bytes, "audio\\ambience.p3d").unwrap();
        assert_eq!(decoded, 3, "3 Patches, no Cue");
        assert_eq!(g.cue_count(), 0);
        assert_eq!(g.patch_count(), 3);

        let amb = g.patch("ambience").expect("the world ambience patch");
        assert_eq!(amb.class, "Ambience_all");
        assert_eq!(amb.props.len(), 59, "PROVEN prop count");
        assert_eq!(
            g.clips_for_stem("ambience"),
            vec!["ambience_baked_18ch".to_string()]
        );
        // The two AtCamera quad players ride along.
        assert!(g.patch("base_interior_ambience").is_some());
        assert!(g.patch("e10m04_ambience").is_some());
    }

    /// Block 65's headline, re-checked through THIS module: every
    /// clip name the decoded graph can produce is an exact clip in
    /// the committed block-63 shard index — the mission music chain
    /// needs no guessing at serving time.
    #[test]
    fn every_music_graph_clip_is_an_indexed_shard_clip() {
        let mut g = MusicGraph::empty();
        for (member, file) in [
            ("audio\\e10_music.p3d", "bank_e10_music.p3d"),
            ("audio\\e02_music.p3d", "bank_e02_music.p3d"),
            ("audio\\ambience.p3d", "bank_ambience.p3d"),
        ] {
            let bytes = fixture(file);
            g.merge_bank_bytes(&bytes, member)
                .expect("fixture banks parse");
        }
        let server = ShardClipServer::embedded();
        let mut clips = 0usize;
        for (_, patch) in g.patches() {
            for clip in patch.clip_names() {
                clips += 1;
                assert!(
                    server.knows(clip),
                    "graph clip {clip:?} must be an indexed shard clip"
                );
            }
        }
        assert!(
            clips >= 30,
            "the three fixtures carry a meaningful vocabulary (got {clips})"
        );
    }

    // -------------------------------------------------------------------
    // Fail-closed edges
    // -------------------------------------------------------------------

    #[test]
    fn junk_bank_bytes_fail_closed_without_touching_the_graph() {
        let mut g = MusicGraph::empty();
        let err = g
            .merge_bank_bytes(b"not a p3d at all", "junk")
            .expect_err("junk is refused");
        assert!(err.contains("bad P3D"), "{err}");
        assert_eq!(g.banks_loaded(), 0, "the graph is untouched");
        assert_eq!(g.chunks_decoded(), 0);
        assert!(g.is_empty());
    }

    #[test]
    fn boot_strap_fully_decodes_into_the_graph() {
        // Block 66: with the grammar extensions, EVERY boot_strap
        // descriptor decodes — 152 Patches + 2 music cues, ZERO
        // refusals (the block-65 "command-script Cue" note was an
        // error; the 24 formerly-refused chunks were Patches hitting
        // the Vector/empty-value/head-flag gaps, now shipped in the
        // block-66 parser).
        let bytes = fixture("boot_strap.p3d");
        let mut g = MusicGraph::empty();
        let decoded = g
            .merge_bank_bytes(&bytes, "audio\\boot_strap.p3d")
            .expect("the file itself is a valid P3D");
        assert_eq!(decoded, 154, "152 Patches + 2 Cues");
        assert_eq!(g.patch_count(), 152);
        assert_eq!(g.cue_count(), 2);
        assert_eq!(g.parse_failures().len(), 0, "full byte-exact decode");

        // The frontend attract graph: drone + music transitions.
        assert_eq!(g.stem_for("front_end", "drone"), Some("frontend_drone"));
        assert_eq!(g.stem_for("front_end", "music"), Some("frontend_music"));
        // Their stem patches are NOT among the decoded objects — an
        // honest empty (the stems stay unresolved, exactly as mined).
        assert!(g.clips_for("front_end", "music").is_empty());

        // But the frontend PLAYERS are here with real clips: the
        // fe_music loop plays the e02m02 ambient bed (PROVEN on the
        // fixture), and pause_menu_music references the indexed
        // 3.84 MB shard clip of the same name.
        assert_eq!(
            g.clips_for_stem("fe_music"),
            vec!["e02m02_ambient".to_string()]
        );
        assert_eq!(
            g.clips_for_stem("pause_menu_music"),
            vec!["pause_menu_music".to_string()]
        );
        // The clear/strike cue keeps its cross-named stems.
        assert_eq!(g.stem_for("clear_cue", "strike_start"), Some("event_start"));

        // The NIS mix feed (block 64's v6 dialect lives here): every
        // NIS_Player patch references its `<movie>_mix` clip.  NOTE the
        // object spelling: the patch is `mnis5_2_2`, its clip is
        // `mnis_5_2_2_mix` — the underscore differs (REAL data).
        let nis = g.patch("mnis5_2_2").expect("a decoded NIS_Player");
        assert_eq!(nis.class, "NIS_Player");
        assert_eq!(
            g.clips_for_stem("mnis5_2_2"),
            vec!["mnis_5_2_2_mix".to_string()]
        );
    }

    // -------------------------------------------------------------------
    // Block 67: the NIS audio descriptor (`nis_mix`)
    // -------------------------------------------------------------------

    #[test]
    fn nis_mix_carries_the_proven_descriptor_of_every_nis_shape() {
        let mut g = MusicGraph::empty();
        g.merge_bank_bytes(&fixture("boot_strap.p3d"), BOOTSTRAP_MUSIC_MEMBER)
            .unwrap_or_else(|e| panic!("boot_strap must decode: {e}"));

        // A mission NIS: clip without extension, volume 1, the
        // PROVEN bus triple.
        let m = g.nis_mix("mnis5_2_2").expect("a decoded NIS patch");
        assert_eq!(m.clips, vec!["mnis_5_2_2_mix".to_string()]);
        assert!((m.volume - 1.0).abs() < 1e-6);
        assert_eq!(m.buses, vec!["nis", "master", "dialog"]);

        // A mission briefing (`mb_*`) follows the same shape with its
        // own underscore-differing clip.
        let mb = g.nis_mix("mb_3_1_1").expect("a decoded NIS patch");
        assert_eq!(mb.clips, vec!["mb_3_1_1_mix".to_string()]);
        assert_eq!(mb.buses, vec!["nis", "master", "dialog"]);

        // The audio-carrying intro NIS: the clip keeps its `.wav`
        // (block 66 addendum, REAL data).
        let intro = g.nis_mix("nis0-3a").expect("a decoded NIS patch");
        assert_eq!(intro.clips, vec!["nis0-3a_mix.wav".to_string()]);

        // An unknown object is an honest None.
        assert!(g.nis_mix("nosuchnis").is_none());
        // And a non-NIS patch (a music stem) still resolves as a
        // patch but is not what nis_mix is for — it works because it
        // shares the descriptor shape, which the count below pins:
        // exactly 46 NIS_Player patches decode from boot_strap.
        assert_eq!(
            g.patches()
                .filter(|(_, p)| p.class == "NIS_Player")
                .count(),
            46
        );
    }

    #[test]
    fn empty_graph_is_empty_and_counts_zero() {
        let g = MusicGraph::empty();
        assert!(g.is_empty());
        assert_eq!(g.cue_count(), 0);
        assert_eq!(g.patch_count(), 0);
        assert_eq!(g.banks_loaded(), 0);
        assert_eq!(g.chunks_decoded(), 0);
        assert!(g.parse_failures().is_empty());
        assert!(g.summary().contains("0 cue(s)"));
    }

    // -------------------------------------------------------------------
    // from_game_set over a synthesized 00audio.rcf
    // -------------------------------------------------------------------

    #[test]
    fn from_game_set_loads_the_graph_from_a_synthesized_archive() {
        let root = temp_root("serve");
        folder_with(
            &root,
            "00audio.rcf",
            &synthesize_named_archive(&[
                ("audio\\ambience.p3d", fixture("bank_ambience.p3d")),
                ("audio\\e02_music.p3d", fixture("bank_e02_music.p3d")),
                ("audio\\e10_music.p3d", fixture("bank_e10_music.p3d")),
                (BOOTSTRAP_MUSIC_MEMBER, fixture("boot_strap.p3d")),
            ]),
        );
        folder_with(&root, "01audio.rcf", b"placeholder");
        let set = crate::game_files::scan_folder(&root).unwrap();
        let g = MusicGraph::from_game_set(&set).expect("the archive serves the banks");
        assert_eq!(g.banks_loaded(), 4);
        assert_eq!(g.cue_count(), 7, "e10m04 + four e02 cues + front_end + clear_cue");
        assert_eq!(g.patch_count(), 182, "7 + 20 + 3 + boot_strap's 152");
        assert_eq!(g.chunks_decoded(), 189);
        assert_eq!(
            g.parse_failures().len(),
            0,
            "block 66's grammar decodes every descriptor"
        );
        // The full chain works through the loaded graph.
        assert_eq!(
            g.clips_for("e10m04", "suspense"),
            vec!["e10m04_suspense".to_string()]
        );
        // The frontend attract graph rode along from boot_strap.
        assert_eq!(g.stem_for("front_end", "drone"), Some("frontend_drone"));
    }

    #[test]
    fn missing_members_degrade_to_a_partial_graph() {
        let root = temp_root("partial");
        folder_with(
            &root,
            "00audio.rcf",
            &synthesize_named_archive(&[(
                "audio\\e10_music.p3d",
                fixture("bank_e10_music.p3d"),
            )]),
        );
        let set = crate::game_files::scan_folder(&root).unwrap();
        let g = MusicGraph::from_game_set(&set).expect("partial installs still serve");
        assert_eq!(g.banks_loaded(), 1);
        assert_eq!(g.cue_count(), 1);
        assert_eq!(g.patch_count(), 7);
        assert!(g.parse_failures().is_empty());
    }

    #[test]
    fn from_game_set_without_00audio_is_err() {
        let root = temp_root("noarchive");
        folder_with(&root, "01audio.rcf", b"placeholder");
        let set = crate::game_files::scan_folder(&root).unwrap();
        let err = MusicGraph::from_game_set(&set).unwrap_err();
        assert!(
            err.contains("00audio.rcf") && err.contains("not present"),
            "the diagnostic names the absent archive: {err}"
        );
    }

    #[test]
    fn a_corrupt_music_archive_fails_closed() {
        let root = temp_root("corrupt");
        folder_with(&root, "00audio.rcf", b"not a cement archive at all");
        let set = crate::game_files::scan_folder(&root).unwrap();
        let err = MusicGraph::from_game_set(&set).unwrap_err();
        assert!(
            err.contains("not a named archive"),
            "the diagnostic says why: {err}"
        );
    }

    #[test]
    fn is_music_archive_is_case_insensitive() {
        assert!(is_music_archive(Path::new("/x/00audio.rcf")));
        assert!(is_music_archive(Path::new("/x/00AUDIO.RCF")));
        assert!(!is_music_archive(Path::new("/x/01audio.rcf")));
        assert!(!is_music_archive(Path::new("/x/00art.rcf")));
    }

    /// Block 72 — the REAL bus lists pinned across the committed
    /// banks (the `mixbuss` routing evidence): five prop names, one
    /// set per class family, PROVEN rotation within a family, and
    /// `master` in every list.
    #[test]
    fn the_real_bus_lists_are_pinned_across_the_banks() {
        let mut g = MusicGraph::empty();
        for bank in ["bank_ambience.p3d", "bank_e02_music.p3d", "bank_e10_music.p3d"] {
            g.merge_bank_bytes(&fixture(bank), &format!("audio\\{bank}"))
                .unwrap();
        }
        let list = |obj: &str| bus_list(g.patch(obj).unwrap());
        // The world bed (Ambience_all) — `mixbus`.
        assert_eq!(list("ambience"), vec!["master", "amb", "sfx"]);
        // Interior beds (AtCamera_Player_Quad) — `bus`.
        assert_eq!(list("base_interior_ambience"), vec!["amb", "sfx", "master"]);
        assert_eq!(list("e10m04_ambience"), vec!["amb", "sfx", "master"]);
        // Mission stems (music_player) — `bus_name`.
        assert_eq!(list("e02m02_ambient"), vec!["master", "music", "igmusic"]);
        assert_eq!(list("e10m04_ambient"), vec!["master", "music", "igmusic"]);
        // Stingers (music_player_random) — `busnames`; the SAME SET
        // in different orders = the ROTATION PROOF (order is not
        // semantic; the lists are sets).
        assert_eq!(list("e02m01_complete"), vec!["master", "music", "igmusic"]);
        assert_eq!(list("e02m05_complete"), vec!["music", "igmusic", "master"]);
        assert_eq!(list("e10m04_complete"), vec!["music", "igmusic", "master"]);
        // The SET is the same across the rotated orders.
        let mut a = list("e02m01_complete");
        let mut b = list("e10m04_complete");
        a.sort();
        b.sort();
        assert_eq!(a, b, "same set, different order");
        // Every list carries `master` — the output stage (the
        // mixer's master gain is its model).
        for obj in [
            "ambience",
            "base_interior_ambience",
            "e10m04_ambience",
            "e02m02_ambient",
            "e02m01_complete",
            "e10m04_complete",
        ] {
            assert!(list(obj).contains(&"master".to_string()), "{obj} carries master");
        }
    }
}

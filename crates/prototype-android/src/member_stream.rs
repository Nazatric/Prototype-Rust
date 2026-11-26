//! Member-level PHYSICAL streaming (M25c wiring, block 44).
//! ============================================================
//!
//! Block 43 landed the member-level streaming STATE MACHINE
//! (`prototype-stream::members` + the engine's `attach_member_stream`
//! glue) — but nothing PHYSICAL consumed its [`MemberLoadSet`]s: the
//! loader's one-shot boot scan (`meshes::collect_meshes_near`) stayed
//! the only real member-loading path, and its near-first ordering was
//! the ad-hoc pass-2 scan the NEXT-UP said to replace.
//!
//! This module is the physical half, device-free so the default test
//! build exercises it against the committed REAL archive fixtures:
//!
//! * the member TABLE comes from the scanned world index (the same
//!   `WorldIndex` the boot builds — hash + placement bbox per member,
//!   distances via the block-43-deduped
//!   [`prototype_p3d::world::bbox_center_distance`]);
//! * the member SPANS come from each archive's DIRECTORY ONLY
//!   ([`prototype_rcf::parse_directory`] — no payload region is read
//!   to learn where members live);
//! * a member LOAD reads exactly that member's span from disk
//!   ([`prototype_rcf::read_member_payload`] — seek + read + RZ
//!   decompress; the phone cannot hold the 633 MB `cells.rcf` in RAM)
//!   and decodes its meshes through the SAME `collect_from_p3d` the
//!   boot scan uses;
//! * the load/unload BOOKKEEPING is the block-43 [`MemberStream`]
//!   state machine — plan → physically load → apply what actually
//!   happened (a failed decode is NOT marked loaded; it stays desired
//!   and is retried next frame, honestly counted).
//!
//! ## The wiring contract (what the App does with this)
//!
//! * Boot keeps the one-shot `collect_meshes_near` scan — it builds
//!   the world index, the spawn database, and the FIRST frame's
//!   batches in one pass (the boot's visual state is unchanged).  The
//!   members whose meshes the boot actually loaded are seeded into
//!   the stream's loaded set, so the first frame's diff starts from
//!   the boot's real state, not from zero.
//! * Per frame the App plans with the PLAYER's position (the M11e
//!   streaming focal point), unloads the evicted members' batches
//!   (provenance compaction via `DrawSelector::retain`), loads the
//!   newly desired members' meshes into batches (the same world-pair
//!   batch creation + static daylight rows the boot uses), and
//!   applies the bookkeeping for what actually happened.
//!
//! ## Confidence labels
//!
//! * PROVEN (data): the world-index member kinds/bboxes/hashes
//!   (blocks 18/41 corpus census); the archive directory layout and
//!   span reads (bit-identity-tested against the committed real
//!   archives in `prototype-rcf`); mesh decode via `collect_from_p3d`
//!   (the boot path, block 16+).
//! * DESIGN (this module, documented): the mesh-pool cap and the
//!   per-frame load budget are clean-room streaming tuning — the
//!   engine's real member loader constants are not recovered from
//!   the binary.  The load-set SEMANTICS (budget, hysteresis,
//!   near-first, farthest-first) are block 43's tested policy.
//! * UNKNOWN: what the engine does with an object member's
//!   non-mesh content on load (spawn-side streaming) — out of scope
//!   here; the App's spawn database stays boot-built.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use prototype_stream::members::{MemberLoadSet, MemberRef, MemberStream, MemberStreamConfig};

// ---------------------------------------------------------------------------
// The physical member stream
// ---------------------------------------------------------------------------

/// One member's disk span inside its archive (from the directory).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberSpan {
    /// Index into the archive-path list the stream was built with.
    pub archive_slot: usize,
    /// Absolute payload byte offset (the directory's value).
    pub file_offset: u32,
    /// Exact payload byte length (the directory's value).
    pub file_size: u32,
}

/// One member's physical load result: the decoded WORLD meshes (the
/// App turns these into batches) with their world/local split, plus
/// the provenance that lets the App extend the batch→member table.
#[derive(Debug, Clone)]
pub struct MemberMeshLoad {
    /// The member's directory hash (the load-set key).
    pub hash: u32,
    /// The member's GLOBAL INDEX in the world index the stream was
    /// built from — the batch→member provenance value.
    pub global_index: usize,
    /// Decoded meshes (world-space AND local — the App filters by
    /// `mesh_world`, the same policy as the boot scan).
    pub meshes: Vec<prototype_render::scene::SceneMesh>,
    /// Per-mesh world/local split (aligned with `meshes`).
    pub mesh_world: Vec<bool>,
    /// Raw bytes the member's payload decompressed to (the physical
    /// fetch size — an honest per-load cost counter).
    pub decompressed_bytes: usize,
}

/// What one physical load pass did (the App's per-frame delta).
#[derive(Debug, Clone, Default)]
pub struct MemberMeshLoads {
    /// Block 115: the tick stopped on the member cap (more work pending).
    pub member_capped: bool,
    /// Block 115: the tick stopped on the wall-clock deadline (more work
    /// pending — the caller keeps streaming next frame).
    pub time_capped: bool,
    /// Members whose payload was fetched AND decoded (loaded
    /// bookkeeping applies to exactly these, in attempt order).
    pub loads: Vec<MemberMeshLoad>,
    /// Members actually marked loaded this pass.
    pub attempted: Vec<u32>,
    /// Fetch/decode failures (bounded messages in `first_errors`).
    pub errors: u32,
    /// First error messages (bounded, same policy as the scan).
    pub first_errors: Vec<String>,
    /// The mesh-pool cap stopped the pass early: the remaining
    /// load-set hashes were NOT attempted (they stay desired and the
    /// next frame retries after unloads free room).
    pub pool_capped: bool,
    /// Members in the load set with no span in any archive directory
    /// (fail-closed: nothing to fetch, never marked loaded).
    pub missing_spans: u32,
}

impl MemberMeshLoads {
    fn note_error(&mut self, msg: String) {
        self.errors += 1;
        if self.first_errors.len() < 4 {
            self.first_errors.push(msg);
        }
    }

    /// True when nothing was loaded and nothing failed.
    pub fn is_empty(&self) -> bool {
        self.loads.is_empty() && self.errors == 0 && self.missing_spans == 0
    }
}

/// The App-side member stream: the block-43 state machine PLUS the
/// physical span table that turns a [`MemberLoadSet`] into real
/// member fetches.  Device-free (no wgpu, no winit) — the App's
/// renderer turns [`MemberMeshLoad`]s into batches.
#[derive(Debug, Clone)]
pub struct MemberMeshStream {
    /// The block-43 bookkeeping state machine.
    stream: MemberStream,
    /// Per world-index global member position: (hash, bbox) — the
    /// member table the plan distances come from (bbox-less members
    /// pass INFINITY and never auto-load, the block-43 rule).
    members: Vec<(u32, Option<[f32; 6]>)>,
    /// hash → global member index (first index wins; duplicates
    /// counted in `duplicate_hashes` — the format's directory is
    /// observed to emit unique hashes, so a duplicate is data, not
    /// an error).
    hash_to_gi: BTreeMap<u32, usize>,
    /// Global member index → disk span (None = the directory has no
    /// such hash: the boot scan's decompress-failed members still
    /// have directory entries, so this stays rare; fail-closed).
    spans: Vec<Option<MemberSpan>>,
    /// The archive paths, in the SAME order the world index was
    /// scanned (collect_meshes_near's archive order — the provenance
    /// contract depends on it).
    archive_paths: Vec<PathBuf>,
    /// Duplicate directory hashes observed at build time.
    duplicate_hashes: usize,
    /// Block 45: the paired drawables keyed by their GEOMETRY member
    /// (the member whose decompress yields the shapes — the I/O unit;
    /// `PairedDrawable::geometry_member`).  ALL pairs are stored (the
    /// loader owns the prop-table skip policy with its honest
    /// counter); empty until `attach_paired`.
    pairs_of_member: BTreeMap<usize, Vec<prototype_p3d::world::PairedDrawable>>,
    /// Block 45: member hashes whose PAIRED meshes are in the App's
    /// pool — the dedup that keeps the boot-seeded pool from
    /// re-loading, and the unload key that drops them.
    paired_loaded: BTreeSet<u32>,
}

impl MemberMeshStream {
    /// Build over a scanned world index: the member table from the
    /// index entries, the span table from each archive's directory
    /// (directory reads ONLY — no payload region is touched).
    ///
    /// `archive_paths` must be the SAME list (and order) the boot
    /// scan used, or the global member indices the App uses as batch
    /// provenance would point at the wrong members.
    pub fn from_index<I, P>(
        archive_paths: I,
        index: &prototype_p3d::world::WorldIndex,
        config: MemberStreamConfig,
    ) -> Result<Self, String>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let archive_paths: Vec<PathBuf> =
            archive_paths.into_iter().map(|p| p.as_ref().to_path_buf()).collect();
        // Span table: hash → (archive slot, offset, size), first wins.
        let mut span_by_hash: BTreeMap<u32, MemberSpan> = BTreeMap::new();
        let mut duplicate_hashes = 0usize;
        for (slot, path) in archive_paths.iter().enumerate() {
            let mut file = std::fs::File::open(path)
                .map_err(|e| format!("{}: open failed: {e}", path.display()))?;
            let (_, entries) = prototype_rcf::parse_directory(&mut file)
                .map_err(|e| format!("{}: directory parse failed: {e}", path.display()))?;
            for e in entries {
                let span = MemberSpan {
                    archive_slot: slot,
                    file_offset: e.file_offset,
                    file_size: e.file_size,
                };
                if span_by_hash.insert(e.name_hash, span).is_some() {
                    duplicate_hashes += 1;
                }
            }
        }
        // Member table + spans parallel to the world index entries.
        let mut members = Vec::with_capacity(index.entries.len());
        let mut spans = Vec::with_capacity(index.entries.len());
        let mut hash_to_gi = BTreeMap::new();
        for (gi, entry) in index.entries.iter().enumerate() {
            members.push((entry.hash, entry.bbox));
            spans.push(span_by_hash.get(&entry.hash).copied());
            // First index wins (duplicates counted, never an error).
            hash_to_gi.entry(entry.hash).or_insert(gi);
        }
        Ok(Self {
            stream: MemberStream::new(config)?,
            members,
            hash_to_gi,
            spans,
            archive_paths,
            duplicate_hashes,
            pairs_of_member: BTreeMap::new(),
            paired_loaded: BTreeSet::new(),
        })
    }

    /// Duplicate directory hashes observed at build time (honest
    /// census; the first index wins the span lookup).
    pub fn duplicate_hashes(&self) -> usize {
        self.duplicate_hashes
    }

    /// Members known to the table (the world index's entry count).
    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    /// Members with a usable disk span (a span-less member can be
    /// planned but never physically fetched — fail-closed).
    pub fn spanned_members(&self) -> usize {
        self.spans.iter().filter(|s| s.is_some()).count()
    }

    /// The loaded hash set (read-only view of the state machine).
    pub fn loaded(&self) -> &std::collections::BTreeSet<u32> {
        self.stream.loaded()
    }

    /// Number of logically-loaded members.
    pub fn loaded_count(&self) -> usize {
        self.stream.loaded_count()
    }

    /// The active policy.
    pub fn config(&self) -> &MemberStreamConfig {
        self.stream.config()
    }

    /// The member table as [`MemberRef`]s for `cam` — the SAME
    /// construction the engine's `member_refs` glue does (bbox-center
    /// distance via the block-43-deduped formula; INFINITY for
    /// bbox-less members).
    pub fn member_refs(&self, cam: [f32; 3]) -> Vec<MemberRef> {
        self.members
            .iter()
            .map(|&(hash, bbox)| MemberRef {
                hash,
                distance: match bbox {
                    Some(b) => prototype_p3d::world::bbox_center_distance(b, cam),
                    None => f32::INFINITY,
                },
            })
            .collect()
    }

    /// Compute the member-level diff for `cam` WITHOUT touching
    /// state (the App plans, physically applies, then applies
    /// bookkeeping for what actually happened).
    pub fn plan(&self, cam: [f32; 3]) -> MemberLoadSet {
        let refs = self.member_refs(cam);
        self.stream.plan(&refs)
    }

    /// PHYSICALLY load the requested hashes (the load set's
    /// near-first order): fetch each member's span from disk, decode
    /// its meshes, and mark it loaded — up to `pool_room` meshes
    /// (the App's GPU-mesh budget; when the room runs out the pass
    /// stops with `pool_capped` and the un-attempted hashes stay
    /// desired).  A fetch/decode failure is NOT marked loaded: it
    /// stays desired and is retried next frame (bounded per frame,
    /// honestly counted).
    // Block 115: THE ANR FIX — the user's phone froze (grey screen +
    // "isn't responding") because the first in-world frame loaded the
    // WHOLE near-set synchronously on the MAIN thread: one giant cells
    // member is a 0.5-2 s flash read + decode, and the entry set can be
    // dozens of them.  The load is now budgeted: a wall-clock deadline
    // AND a per-tick member cap; whatever doesn't fit carries to the
    // next frame (the stream state machine re-plans the same set —
    // progress is continuous, the frame loop stays live).
    pub fn load_meshes(
        &mut self,
        hashes: &[u32],
        pool_room: usize,
        deadline: Option<std::time::Instant>,
        max_members: usize,
    ) -> MemberMeshLoads {
        let mut out = MemberMeshLoads::default();
        let mut room = pool_room;
        let mut loaded_members = 0usize;
        for &hash in hashes {
            if room == 0 {
                out.pool_capped = true;
                break;
            }
            if loaded_members >= max_members {
                out.member_capped = true;
                break;
            }
            if let Some(d) = deadline {
                if std::time::Instant::now() >= d {
                    out.time_capped = true;
                    break;
                }
            }
            loaded_members += 1;
            let Some(&gi) = self.hash_to_gi.get(&hash) else {
                // Not in the member table — a caller-visible invariant
                // break; count it and move on (fail-closed).
                out.note_error(format!("hash {hash:#x}: not in the member table"));
                continue;
            };
            let Some(span) = self.spans[gi] else {
                out.missing_spans += 1;
                continue;
            };
            let Some(path) = self.archive_paths.get(span.archive_slot) else {
                out.note_error(format!("hash {hash:#x}: archive slot out of range"));
                continue;
            };
            let mut file = match std::fs::File::open(path) {
                Ok(f) => f,
                Err(e) => {
                    out.note_error(format!("{}: open failed: {e}", path.display()));
                    continue;
                }
            };
            let raw = match prototype_rcf::read_member_payload(&mut file, &prototype_rcf::RcfDirEntry {
                name_hash: hash,
                file_offset: span.file_offset,
                file_size: span.file_size,
            }) {
                Ok(r) => r,
                Err(e) => {
                    out.note_error(format!("hash {hash:#x}: span read failed: {e}"));
                    continue;
                }
            };
            // Decode the member's meshes through the boot path.
            let mut rep = crate::meshes::MeshLoadReport::default();
            crate::meshes::collect_from_p3d(&raw, room, &mut rep);
            for e in &rep.first_errors {
                out.note_error(format!("hash {hash:#x}: {e}"));
            }
            let mut meshes = std::mem::take(&mut rep.meshes);
            let mut mesh_world = std::mem::take(&mut rep.mesh_world);
            if meshes.len() > room {
                // collect_from_p3d honors the cap; this is paranoia.
                meshes.truncate(room);
                mesh_world.truncate(room);
            }
            room -= meshes.len();
            out.loads.push(MemberMeshLoad {
                hash,
                global_index: gi,
                decompressed_bytes: raw.len(),
                meshes,
                mesh_world,
            });
            out.attempted.push(hash);
        }
        // Bookkeeping for what actually happened (and nothing else —
        // failed/missing/un-attempted hashes stay in the desired set).
        self.stream.apply(&MemberLoadSet {
            load: out.attempted.clone(),
            unload: Vec::new(),
        });
        out
    }

    /// PHYSICALLY unload bookkeeping (the App drops the evicted
    /// members' batches around this call).
    pub fn apply_unload(&mut self, hashes: &[u32]) {
        self.stream.apply(&MemberLoadSet {
            load: Vec::new(),
            unload: hashes.to_vec(),
        });
    }

    /// SEED the loaded set with hashes that are ALREADY physically
    /// present (the boot scan's members) without touching the
    /// desired-set logic — the first plan then diffs from the boot's
    /// real state instead of from zero.
    pub fn seed_loaded<I: IntoIterator<Item = u32>>(&mut self, hashes: I) {
        let load: Vec<u32> = hashes.into_iter().collect();
        self.stream.apply(&MemberLoadSet {
            load,
            unload: Vec::new(),
        });
    }
}

// ---------------------------------------------------------------------------
// Block 45: the PAIRED half — paired drawables stream WITH their
// geometry member (the member stream is the one decision-maker)
// ---------------------------------------------------------------------------

/// One member's paired load result: the decoded LOCAL-shape meshes
/// with their REAL placements (the App turns these into placed
/// batches), keyed by the geometry member that produced them.
#[derive(Debug, Clone)]
pub struct MemberPairedLoad {
    /// The geometry member's directory hash (the load-set key).
    pub hash: u32,
    /// The geometry member's global index in the world index.
    pub global_index: usize,
    /// Decoded paired meshes (LOCAL shapes + REAL binary placements —
    /// prop-table pairs were skipped by the loader, honestly counted).
    pub meshes: Vec<crate::meshes::PairedMesh>,
}

/// What one physical paired load pass did (the App's per-frame delta).
#[derive(Debug, Clone, Default)]
pub struct MemberPairedMeshes {
    /// Block 121: the tick stopped on the member cap (more work pending).
    pub member_capped: bool,
    /// Block 121: the tick stopped on the wall-clock deadline (more
    /// work pending — the caller keeps streaming next frame).
    pub time_capped: bool,
    /// Members whose paired content was fetched and decoded.
    pub loads: Vec<MemberPairedLoad>,
    /// Members actually marked paired-loaded this pass (in attempt
    /// order — includes members with NO pairs: their paired content
    /// is trivially complete).
    pub attempted: Vec<u32>,
    /// Fetch/decode failures (bounded messages).
    pub errors: u32,
    /// First error messages (bounded).
    pub first_errors: Vec<String>,
    /// The paired-pool cap stopped the pass early: the remaining
    /// hashes were NOT attempted (they stay pending and the next
    /// frame retries after unloads free room).
    pub pool_capped: bool,
    /// Prop-table pairs the loader skipped (the honest counter,
    /// summed over the pass).
    pub skipped_prop_table: u32,
}

impl MemberPairedMeshes {
    fn note_error(&mut self, msg: String) {
        self.errors += 1;
        if self.first_errors.len() < 4 {
            self.first_errors.push(msg);
        }
    }
}

impl MemberMeshStream {
    /// Attach the paired-drawable table (block 45): groups the pair
    /// report's pairs by their GEOMETRY member — the member whose
    /// decompress yields the shapes, i.e. the I/O unit the member
    /// stream already load/unloads.  Returns the paired-pair count.
    pub fn attach_paired(
        &mut self,
        pairs: &[prototype_p3d::world::PairedDrawable],
    ) -> usize {
        self.pairs_of_member.clear();
        for p in pairs {
            self.pairs_of_member
                .entry(p.geometry_member)
                .or_default()
                .push(p.clone());
        }
        self.pairs_of_member.values().map(|v| v.len()).sum()
    }

    /// SEED the paired-loaded set with hashes whose paired meshes are
    /// ALREADY in the App's pool (the boot scan's members) — the
    /// first paired diffs start from the boot's real state.
    pub fn seed_paired<I: IntoIterator<Item = u32>>(&mut self, hashes: I) {
        self.paired_loaded.extend(hashes);
    }

    /// Members currently paired-loaded (read-only view).
    pub fn paired_loaded(&self) -> &std::collections::BTreeSet<u32> {
        &self.paired_loaded
    }

    /// Members with at least one paired drawable keyed to them (the
    /// paired-attach census).
    pub fn paired_member_count(&self) -> usize {
        self.pairs_of_member.len()
    }

    /// PHYSICALLY load the paired meshes of the requested members
    /// (near-first order preserved from the load set): each member's
    /// pairs run through the SAME `collect_paired_meshes` the boot
    /// uses (one decompress per member, prop-table pairs skipped by
    /// the loader, capped at `pool_room` meshes).  Members already
    /// paired-loaded are skipped (no re-fetch); members with no pairs
    /// for their geometry member are marked loaded with zero meshes
    /// (their paired content is trivially complete).  A member marked
    /// here stays paired-loaded until `unload_paired` — a fetch that
    /// yields zero meshes is a COMPLETE load, not a failure.
    ///
    /// Block 121: BUDGETED like the mesh half — the block-115 fix
    /// bounded only `load_meshes`; the paired half still ran over the
    /// WHOLE entry load set in one call, and `collect_paired_meshes`
    /// decompresses a member's geometry per pair (0.5-2 s per member
    /// on a phone).  Entering the world desired dozens of members →
    /// the first in-world frame blocked the main thread for 30+ s →
    /// the "make a new game → isn't responding" ANR (the 2026-11-26
    /// journal's silence after the frontend hand-off).  Same policy
    /// as the mesh half: a wall-clock deadline AND a per-tick member
    /// cap, checked BETWEEN members; whatever doesn't fit retries
    /// next frame (the state machine re-plans the same set).
    pub fn load_paired_meshes(
        &mut self,
        hashes: &[u32],
        cam: [f32; 3],
        pool_room: usize,
        deadline: Option<std::time::Instant>,
        max_members: usize,
    ) -> MemberPairedMeshes {
        let mut out = MemberPairedMeshes::default();
        let mut room = pool_room;
        let mut loaded_members = 0usize;
        for &hash in hashes {
            if room == 0 {
                out.pool_capped = true;
                break;
            }
            if loaded_members >= max_members {
                out.member_capped = true;
                break;
            }
            if let Some(d) = deadline {
                if std::time::Instant::now() >= d {
                    out.time_capped = true;
                    break;
                }
            }
            if self.paired_loaded.contains(&hash) {
                continue; // already in the App's pool (boot seed or earlier load)
            }
            loaded_members += 1;
            let Some(&gi) = self.hash_to_gi.get(&hash) else {
                out.note_error(format!("paired: hash {hash:#x}: not in the member table"));
                continue;
            };
            let pairs = self.pairs_of_member.get(&gi).cloned().unwrap_or_default();
            let mut rep = crate::meshes::collect_paired_meshes(
                self.archive_paths.iter().map(|p| p.as_path()),
                &pairs,
                cam,
                room,
            );
            out.skipped_prop_table += rep.skipped_prop_table;
            for e in &rep.first_errors {
                out.note_error(format!("paired hash {hash:#x}: {e}"));
            }
            if rep.meshes.len() > room {
                rep.meshes.truncate(room); // paranoia; the collector honors the cap
            }
            room -= rep.meshes.len();
            out.loads.push(MemberPairedLoad {
                hash,
                global_index: gi,
                meshes: std::mem::take(&mut rep.meshes),
            });
            out.attempted.push(hash);
        }
        // Bookkeeping for what actually happened (and nothing else —
        // un-attempted hashes stay pending and retry next frame).
        for &h in &out.attempted {
            self.paired_loaded.insert(h);
        }
        out
    }

    /// PHYSICALLY unload the paired meshes of the evicted members
    /// (the App drops the evicted members' paired batches around this
    /// call); the hashes become eligible again on a later reload.
    pub fn unload_paired(&mut self, hashes: &[u32]) {
        for &h in hashes {
            self.paired_loaded.remove(&h);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — the physical half against the committed REAL archive fixtures
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    fn config(budget: usize, hysteresis: f32) -> MemberStreamConfig {
        MemberStreamConfig::new(budget, hysteresis).unwrap()
    }

    /// The cells_mini.rcf world index (3 members: 62-object objects
    /// member, geometry member, empty member) + the stream over it.
    fn mini_stream(budget: usize, hysteresis: f32) -> (MemberMeshStream, prototype_p3d::world::WorldIndex) {
        let path = fixture("cells_mini.rcf");
        let (_, world) = crate::meshes::collect_meshes_near(
            [&path],
            [0.0; 3],
            16,
            prototype_p3d::world::ScanOptions::default(),
        );
        assert_eq!(world.index.entries.len(), 3, "{:?}", world.first_errors);
        let stream = MemberMeshStream::from_index(
            [&path],
            &world.index,
            config(budget, hysteresis),
        )
        .unwrap();
        (stream, world.index)
    }

    /// (block 44 probe) The REAL member table facts the streaming
    /// policy runs on: every member's hash, bbox presence and bbox
    /// center — printed once, then pinned by the policy tests below.
    #[test]
    fn mini_fixture_member_table_probe() {
        let (stream, index) = mini_stream(4, 1.5);
        assert_eq!(stream.member_count(), 3);
        assert_eq!(stream.spanned_members(), 3, "all directory entries span");
        assert_eq!(stream.duplicate_hashes(), 0);
        for (gi, e) in index.entries.iter().enumerate() {
            let bbox = e
                .bbox
                .map(|b| {
                    let c = [
                        (b[0] + b[3]) * 0.5,
                        (b[1] + b[4]) * 0.5,
                        (b[2] + b[5]) * 0.5,
                    ];
                    format!("center ({:.1}, {:.1}, {:.1})", c[0], c[1], c[2])
                })
                .unwrap_or_else(|| "NONE".to_string());
            println!("gi {gi}: hash {:#010x} kind {:?} bbox {bbox}", e.hash, e.kind);
        }
        // The world-index hashes and the member table agree 1:1.
        for (gi, e) in index.entries.iter().enumerate() {
            let refs = stream.member_refs([0.0; 3]);
            assert_eq!(refs[gi].hash, e.hash);
            let want = e.distance_from([0.0; 3]);
            assert_eq!(refs[gi].distance.to_bits(), want.to_bits());
        }
    }

    /// THE streaming slice on the REAL mini archive: plan → physical
    /// load → bookkeeping → settlement.  The mini table has exactly
    /// ONE streamable member (the morgue objects member, PROVEN by
    /// the probe test above: the geometry + empty members carry no
    /// placement bbox), so the policy pins it there: it is desired at
    /// EVERY focal point (budget 4 ≥ streamable count 1 — it never
    /// falls out of the desired set) and never evicts.  The
    /// load/unload cycle BETWEEN members lives on the pairing fixture
    /// (two streamable members) in the next test.
    #[test]
    fn member_stream_loads_and_settles_on_real_archive() {
        let (mut stream, index) = mini_stream(4, 1.5);
        let morgue = [1687.0, 10.0, -1845.0];
        let set = stream.plan(morgue);
        assert_eq!(set.load, vec![0x0049_b319], "the morgue member is desired");
        assert!(set.unload.is_empty());
        // Physical load: every desired hash fetches from disk.
        let loads = stream.load_meshes(&set.load, 64, None, usize::MAX);
        assert_eq!(loads.errors, 0, "{:?}", loads.first_errors);
        assert_eq!(loads.missing_spans, 0);
        assert!(!loads.pool_capped);
        assert_eq!(loads.attempted, set.load);
        for l in &loads.loads {
            // provenance is in range and the payload was real
            assert!(l.global_index < index.entries.len());
            assert_eq!(index.entries[l.global_index].hash, l.hash);
            assert!(l.decompressed_bytes > 0);
        }
        assert_eq!(stream.loaded_count(), 1);
        // Settlement: the same focal point plans to NOTHING.
        let settle = stream.plan(morgue);
        assert!(settle.is_empty(), "{settle:?}");
        // A 2.6 km hop: the member is STILL the desired set (it is
        // the table's only streamable member) and stays inside the
        // keep radius — zero churn, the settlement property holds at
        // every focal point.
        let far = [morgue[0] + 2600.0, morgue[1], morgue[2]];
        let away = stream.plan(far);
        assert!(away.is_empty(), "{away:?}");
        assert_eq!(stream.loaded_count(), 1);
    }

    /// THE load/unload cycle BETWEEN members on the REAL pairing
    /// archive (two streamable members ~234 m apart, PROVEN by the
    /// probe): with budget 1, standing at member A desires A only;
    /// walking to B evicts A (it falls out of the desired set) and
    /// loads B; walking back does the reverse.  The physical loads
    /// carry the REAL payload facts: the objects member decodes 0
    /// meshes (records only), the geometry member decodes 9 meshes
    /// of which 1 is world-space.
    #[test]
    fn member_stream_evicts_and_reloads_between_real_members() {
        let path = fixture("cells_pairing.rcf");
        let (_, world) = crate::meshes::collect_meshes_near(
            [&path],
            [0.0; 3],
            16,
            prototype_p3d::world::ScanOptions::default(),
        );
        assert_eq!(world.index.entries.len(), 2);
        let mut stream =
            MemberMeshStream::from_index([&path], &world.index, config(1, 1.5)).unwrap();
        let a_center = [446.0, 113.1, 333.0]; // objects member (gi 0)
        let b_center = [250.9, 27.0, 235.9]; // geometry member (gi 1)

        // --- at A: desire + load A (0 meshes — records only) ------
        let at_a = stream.plan(a_center);
        assert_eq!(at_a.load, vec![0x51a2_ca3b], "A desired, near first");
        assert!(at_a.unload.is_empty());
        let load_a = stream.load_meshes(&at_a.load, 64, None, usize::MAX);
        assert_eq!(load_a.errors, 0, "{:?}", load_a.first_errors);
        assert_eq!(load_a.loads.len(), 1);
        assert_eq!(load_a.loads[0].global_index, 0);
        assert!(load_a.loads[0].meshes.is_empty(), "object records, no shapes");
        assert_eq!(stream.loaded_count(), 1);
        assert!(stream.plan(a_center).is_empty(), "settled at A");

        // --- walk to B: A evicts, B loads (9 meshes, 1 world) -----
        let at_b = stream.plan(b_center);
        assert_eq!(at_b.unload, vec![0x51a2_ca3b], "A fell out of budget 1");
        assert_eq!(at_b.load, vec![0x2268_de7e], "B desired");
        // bookkeeping first (the App would drop A's batches here)
        stream.apply_unload(&at_b.unload);
        assert_eq!(stream.loaded_count(), 0);
        let load_b = stream.load_meshes(&at_b.load, 64, None, usize::MAX);
        assert_eq!(load_b.errors, 0, "{:?}", load_b.first_errors);
        assert_eq!(load_b.loads.len(), 1);
        let b = &load_b.loads[0];
        assert_eq!(b.global_index, 1);
        assert_eq!(b.meshes.len(), b.mesh_world.len());
        assert_eq!(b.meshes.len(), 9, "the REAL geometry member's meshes");
        assert_eq!(b.mesh_world.iter().filter(|w| **w).count(), 1, "1 world mesh");
        assert!(b.decompressed_bytes > 100_000);
        assert_eq!(stream.loaded_count(), 1);
        assert!(stream.plan(b_center).is_empty(), "settled at B");

        // --- walk back: B evicts, A reloads (the cycle is closed) -
        let back = stream.plan(a_center);
        assert_eq!(back.unload, vec![0x2268_de7e]);
        assert_eq!(back.load, vec![0x51a2_ca3b]);
        stream.apply_unload(&back.unload);
        let reload_a = stream.load_meshes(&back.load, 64, None, usize::MAX);
        assert_eq!(reload_a.errors, 0);
        assert_eq!(stream.loaded_count(), 1);
        assert!(stream.plan(a_center).is_empty(), "settled at A again");
    }

    /// The mesh-pool cap fails CLOSED: a zero-room pass attempts
    /// NOTHING (everything stays desired, nothing is marked loaded)
    /// and reports `pool_capped`; a bounded pass takes meshes up to
    /// the room it has and the truncated member stays loaded (its
    /// payload WAS fetched — the same semantics as the boot scan's
    /// capped one-shot pass; the reload path gives the truncated
    /// meshes a second chance after unloads free room).
    #[test]
    fn mesh_pool_cap_stops_the_pass_and_stays_desired() {
        // Zero room on the mini archive: nothing attempted, nothing
        // loaded, still desired.
        let (mut stream, _) = mini_stream(4, 1.5);
        let morgue = [1687.0, 10.0, -1845.0];
        let set = stream.plan(morgue);
        let none = stream.load_meshes(&set.load, 0, None, usize::MAX);
        assert!(none.is_empty());
        assert!(none.pool_capped);
        assert_eq!(stream.loaded_count(), 0, "zero room must not load");
        assert_eq!(stream.plan(morgue).load, set.load, "still desired");

        // Bounded room on the pairing archive (budget 4: BOTH real
        // members are desired): the objects member takes no meshes,
        // the geometry member takes exactly the room it gets (1 of
        // its 9 real meshes) and is marked loaded — the payload was
        // fetched and decoded.
        let path = fixture("cells_pairing.rcf");
        let (_, world) = crate::meshes::collect_meshes_near(
            [&path],
            [0.0; 3],
            16,
            prototype_p3d::world::ScanOptions::default(),
        );
        let mut pool =
            MemberMeshStream::from_index([&path], &world.index, config(4, 1.5)).unwrap();
        let set = pool.plan([0.0; 3]);
        assert_eq!(set.load.len(), 2, "both real members are desired");
        // Near-first at the origin: the geometry member (345 m) is
        // desired before the objects member (568 m) — it fills the
        // room first (1 of its 9 real meshes), the pass reports
        // pool_capped, and the un-attempted objects member stays
        // desired (next frame, after unloads free room).
        assert_eq!(set.load, vec![0x2268_de7e, 0x51a2_ca3b], "near first");
        let loads = pool.load_meshes(&set.load, 1, None, usize::MAX);
        assert_eq!(loads.errors, 0, "{:?}", loads.first_errors);
        assert!(loads.pool_capped);
        assert_eq!(loads.attempted, vec![0x2268_de7e], "the near member only");
        let b = &loads.loads[0];
        assert_eq!(b.meshes.len(), 1, "exactly the room it got");
        assert_eq!(b.mesh_world.len(), 1);
        assert_eq!(pool.loaded_count(), 1);
        let again = pool.plan([0.0; 3]);
        assert_eq!(again.load, vec![0x51a2_ca3b], "the rest stays desired");
    }

    /// Seeding: the boot scan's members are pre-marked loaded, so the
    /// first plan diffs from the boot's real state (here: the boot
    /// loaded the geometry member's meshes → that member is loaded →
    /// the first plan at the same camera must not re-load it).
    #[test]
    fn boot_seeded_stream_does_not_reload_the_boot_members() {
        let path = fixture("cells_mini.rcf");
        let (scan, world) = crate::meshes::collect_meshes_near(
            [&path],
            [0.0; 3],
            16,
            prototype_p3d::world::ScanOptions::default(),
        );
        let mut stream =
            MemberMeshStream::from_index([&path], &world.index, config(4, 1.5)).unwrap();
        assert!(!scan.meshes.is_empty(), "the boot scan loads real meshes");
        let boot_members: std::collections::BTreeSet<u32> = scan
            .member_of_mesh
            .iter()
            .map(|&gi| world.index.entries[gi].hash)
            .collect();
        stream.seed_loaded(boot_members.iter().copied());
        assert_eq!(stream.loaded_count(), boot_members.len());
        // The first plan at the boot camera must not re-load any
        // seeded member (they may ADD up to budget, never re-load).
        let first = stream.plan([0.0; 3]);
        for h in &first.load {
            assert!(
                !boot_members.contains(h),
                "seeded hash {h:#x} must not be re-loaded"
            );
        }
    }

    /// A load-set hash that is physically un-fetchable (span missing)
    /// is never marked loaded and stays desired — fail-closed.  Built
    /// by pointing the stream at an archive that lacks the member's
    /// directory entry (the empty-fixture world index over a
    /// different archive).
    #[test]
    fn span_missing_members_fail_closed_and_stay_desired() {
        let path = fixture("cells_mini.rcf");
        let (_, world) = crate::meshes::collect_meshes_near(
            [&path],
            [0.0; 3],
            16,
            prototype_p3d::world::ScanOptions::default(),
        );
        // A DIFFERENT real archive hosts the spans: the mini index's
        // hashes are (almost certainly) not in it — every plan-load
        // attempt must miss its span and never mark loaded.
        let other = fixture("cells_pairing.rcf");
        let mut stream =
            MemberMeshStream::from_index([&other], &world.index, config(4, 1.5)).unwrap();
        let morgue = [1687.0, 10.0, -1845.0];
        let set = stream.plan(morgue);
        if set.load.is_empty() {
            // cross-archive hashes collided into the desired set?  No:
            // with no span the desired set is still computed from the
            // member TABLE (which exists) — this branch means the
            // morgue member is absent from THIS table, impossible.
            panic!("the member table is archive-independent; the load set must exist");
        }
        let loads = stream.load_meshes(&set.load, 64, None, usize::MAX);
        assert_eq!(stream.loaded_count(), 0, "nothing can load cross-archive");
        assert!(
            loads.missing_spans > 0 || loads.errors > 0,
            "every attempt must fail closed: {loads:?}"
        );
        // and the desired set is UNCHANGED (retry next frame).
        let again = stream.plan(morgue);
        assert_eq!(again.load, set.load);
    }

    // ---- block 45: the PAIRED half (paired drawables stream with ----
    // ---- their geometry member; the member stream is the one      ----
    // ---- decision-maker)                                          ----

    /// The REAL pairing fixture's pair table facts (pinned by the
    /// block-22 test): 4 pairs, ALL keyed to the geometry member
    /// (2 binary placeable + 2 prop-table, which the loader skips
    /// with its honest counter).
    fn pairing_stream(budget: usize) -> (MemberMeshStream, prototype_p3d::world::WorldIndex) {
        let path = fixture("cells_pairing.rcf");
        let (_, world) = crate::meshes::collect_meshes_near(
            [&path],
            [0.0; 3],
            16,
            prototype_p3d::world::ScanOptions {
                retain_objects: true,
                retain_shape_names: true,
                retain_node_transforms: true,
            },
        );
        let pairs = world.index.pair_drawables(usize::MAX);
        assert_eq!(pairs.pairs.len(), 4);
        let mut stream =
            MemberMeshStream::from_index([&path], &world.index, config(budget, 1.5)).unwrap();
        assert_eq!(stream.attach_paired(&pairs.pairs), 4);
        (stream, world.index)
    }

    /// THE paired streaming slice: the paired drawables ride the
    /// member cycle.  At the objects member A nothing paired is keyed
    /// to it (all 4 pairs' geometry member is B) — A pairs-load
    /// TRIVIALLY (marked loaded, zero meshes).  Walking to the
    /// geometry member B evicts A and loads B's paired meshes: the
    /// REAL binary placements (the sign at its world position) with
    /// LOCAL geometry, prop-table pairs skipped.  Walking back unloads
    /// B's paired meshes and they reload on return.
    #[test]
    fn paired_pool_streams_with_the_member_cycle() {
        let (mut stream, _) = pairing_stream(1);
        let a_center = [446.0, 113.1, 333.0]; // objects member A
        let b_center = [250.9, 27.0, 235.9]; // geometry member B
        let sign = [199.99554, 27.7564, 331.50858]; // the sign record's REAL placement

        // --- at A: the member + paired halves both load (the App's
        //     step 2c sequence: load_meshes then load_paired_meshes) -
        let at_a = stream.plan(a_center);
        assert_eq!(at_a.load, vec![0x51a2_ca3b]);
        stream.load_meshes(&at_a.load, 64, None, usize::MAX);
        let paired_a = stream.load_paired_meshes(&at_a.load, a_center, 64, None, usize::MAX);
        assert_eq!(paired_a.errors, 0, "{:?}", paired_a.first_errors);
        assert_eq!(paired_a.attempted, at_a.load, "A marked paired-loaded");
        assert!(paired_a.loads[0].meshes.is_empty(), "no pairs keyed to A");
        assert_eq!(stream.paired_loaded().len(), 1);

        // --- walk to B: A's paired state unloads, B's pairs decode --
        let at_b = stream.plan(b_center);
        assert_eq!(at_b.unload, vec![0x51a2_ca3b]);
        assert_eq!(at_b.load, vec![0x2268_de7e]);
        stream.apply_unload(&at_b.unload);
        stream.unload_paired(&at_b.unload);
        stream.load_meshes(&at_b.load, 64, None, usize::MAX);
        assert!(stream.paired_loaded().is_empty());
        let paired_b = stream.load_paired_meshes(&at_b.load, sign, 64, None, usize::MAX);
        assert_eq!(paired_b.errors, 0, "{:?}", paired_b.first_errors);
        assert_eq!(paired_b.attempted, at_b.load);
        assert_eq!(paired_b.skipped_prop_table, 2, "prop-table pairs skipped");
        assert_eq!(paired_b.loads.len(), 1);
        let b = &paired_b.loads[0];
        assert_eq!(b.global_index, 1, "the geometry member");
        assert!(b.meshes.len() >= 2, "the REAL binary pairs decoded");
        for pm in &b.meshes {
            assert_eq!(pm.pair.variant, prototype_p3d::world::ObjectVariant::Binary);
            assert!(pm.mesh.vertex_count() > 0, "real sections, not stubs");
        }
        // nearest-first within the member: the camera sits at the
        // sign's REAL world position — its mesh loads first
        assert_eq!(b.meshes[0].pair.drawable, "signHorizontal001");
        let pos = b.meshes[0].pair.placement.pos;
        assert!((pos[0] - 199.99554).abs() < 1e-3);
        assert!((pos[1] - 27.7564).abs() < 1e-3);
        assert!((pos[2] - 331.50858).abs() < 1e-3);
        assert_eq!(stream.paired_loaded().len(), 1);
        // settle: a second pass at B is a no-op (already loaded)
        let settle = stream.load_paired_meshes(&at_b.load, sign, 64, None, usize::MAX);
        assert!(settle.loads.is_empty() && settle.attempted.is_empty());

        // --- walk back: B's paired meshes unload and reload cleanly -
        let back = stream.plan(a_center);
        assert_eq!(back.unload, vec![0x2268_de7e]);
        assert_eq!(back.load, vec![0x51a2_ca3b]);
        stream.apply_unload(&back.unload);
        stream.unload_paired(&back.unload);
        assert!(stream.paired_loaded().is_empty());
        stream.load_meshes(&back.load, 64, None, usize::MAX);
        let reload = stream.load_paired_meshes(&back.load, a_center, 64, None, usize::MAX);
        assert_eq!(reload.errors, 0);
        assert_eq!(reload.attempted, back.load);
        assert!(reload.loads[0].meshes.is_empty(), "A's trivial paired load again");
    }

    /// The boot seed prevents re-fetch: members whose paired meshes
    /// are already in the App's pool are skipped by the paired loads
    /// (no double decode), and unloading makes them eligible again.
    #[test]
    fn paired_seed_prevents_refetch_until_unload() {
        let (mut stream, _) = pairing_stream(4);
        let b_hash = 0x2268_de7e;
        stream.seed_paired([b_hash]);
        assert_eq!(stream.paired_loaded().len(), 1);
        // a load pass over the seeded member does NOTHING
        let none = stream.load_paired_meshes(&[b_hash], [0.0; 3], 64, None, usize::MAX);
        assert!(none.loads.is_empty() && none.attempted.is_empty());
        assert!(none.first_errors.is_empty());
        // unload → eligible again → reloads
        stream.unload_paired(&[b_hash]);
        let again = stream.load_paired_meshes(&[b_hash], [0.0; 3], 64, None, usize::MAX);
        assert_eq!(again.attempted, vec![b_hash]);
        assert!(again.loads[0].meshes.len() >= 2);
    }

    /// The paired-pool cap: a one-mesh room takes exactly one of the
    /// REAL paired meshes (nearest-first: the sign at the sign
    /// camera), marks the member loaded (its paired content WAS
    /// fetched), and the rest of that member's meshes are truncated —
    /// the same capped-pool semantics as the world pool.
    #[test]
    fn paired_pool_cap_takes_room_and_marks_loaded() {
        let (mut stream, _) = pairing_stream(4);
        let b_hash = 0x2268_de7e;
        let sign = [199.99554, 27.7564, 331.50858];
        let capped = stream.load_paired_meshes(&[b_hash], sign, 1, None, usize::MAX);
        assert_eq!(capped.errors, 0, "{:?}", capped.first_errors);
        assert_eq!(capped.attempted, vec![b_hash]);
        assert_eq!(capped.loads[0].meshes.len(), 1, "exactly the room");
        assert_eq!(capped.loads[0].meshes[0].pair.drawable, "signHorizontal001");
        assert_eq!(stream.paired_loaded().len(), 1);
        // the truncated meshes come back only after unload + reload
        stream.unload_paired(&[b_hash]);
        let full = stream.load_paired_meshes(&[b_hash], sign, 64, None, usize::MAX);
        assert!(full.loads[0].meshes.len() >= 2);
    }

    /// Block 121: the paired budget fails OPEN for progress and CLOSED
    /// for the frame: a deadline in the past attempts NOTHING (all
    /// pending, time_capped reported), and a member cap of 1 takes
    /// exactly one member per tick (the rest retries next tick).
    #[test]
    fn paired_loads_are_budgeted_like_the_mesh_half() {
        let (mut stream, _) = pairing_stream(4);
        let b_hash = 0x2268_de7e;
        let sign = [199.99554, 27.7564, 331.50858];
        // Deadline in the past: nothing attempted, time_capped set,
        // nothing marked loaded (the whole set stays pending).
        let past = std::time::Instant::now() - std::time::Duration::from_secs(1);
        let none = stream.load_paired_meshes(&[b_hash], sign, 64, Some(past), usize::MAX);
        assert!(none.loads.is_empty() && none.attempted.is_empty());
        assert!(none.time_capped);
        assert_eq!(stream.paired_loaded().len(), 0);
        // Member cap 1 with two pending members... the pairing fixture
        // has one paired member — pin the cap on it: 1 tick loads it,
        // and a further tick over the same set is a no-op (settled).
        let one = stream.load_paired_meshes(&[b_hash], sign, 64, None, 1);
        assert_eq!(one.attempted, vec![b_hash], "cap 1 takes the member");
        assert!(!one.member_capped, "the set had exactly one member");
        assert_eq!(stream.paired_loaded().len(), 1);
        let settle = stream.load_paired_meshes(&[b_hash], sign, 64, None, 1);
        assert!(settle.attempted.is_empty(), "already loaded, nothing to do");
    }
}
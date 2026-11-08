//! meshes — load REAL geometry from the user's own `.rcf` archives at
//! boot (M21b block 16).
//!
//! Device-free and fail-soft: a decode failure on one member must never
//! kill the boot — it is counted, the first few are logged, the rest of
//! the archive continues.  This is the *load* half of the mesh path; the
//! shader binding half (mesh name → `pure3dShader` → pipeline) is still
//! unresolved (the art-side `0x00011015` shader chunks are not decoded
//! into VS/FS pairs yet), so callers draw the meshes through whatever
//! pipeline they have and log that honestly.
//!
//! ## PROVEN vs LIKELY
//!
//! * PROVEN: every decode step (`prototype_rcf::payload_decompressed` RZ
//!   unwrap, `prototype_p3d::geometry` Shape/Mesh decoders) is validated
//!   corpus-wide on the real `art.rcf` (see PROGRESS.md block 16).
//! * UNKNOWN: how the original engine chooses which members to load per
//!   cell (that lives in `cells.rcf` — M25).  This module therefore scans
//!   a bounded number of members in file order and caps the mesh count.

#![forbid(unsafe_code)]

use prototype_p3d::geometry as geo;
use prototype_render::scene::SceneMesh;
use std::path::Path;

/// What one collection pass found.  Every counter is honest — nothing is
/// hidden behind an `Ok(())`.
#[derive(Debug, Clone, Default)]
pub struct MeshLoadReport {
    /// Decoded meshes (capped at the caller's `max_meshes`).
    pub meshes: Vec<SceneMesh>,
    /// RCF archives opened successfully.
    pub archives_opened: u32,
    /// Archives that failed to open (missing / malformed).
    pub archives_failed: u32,
    /// Members examined (any payload kind).
    pub members_scanned: u32,
    /// Members that decompressed to a `P3D\xff` file.
    pub members_with_p3d: u32,
    /// Shape chunks decoded successfully.
    pub shapes_decoded: u32,
    /// Mesh chunks decoded successfully (before the cap).
    pub meshes_decoded: u32,
    /// Decode failures (per-mesh or per-shape), with the first messages.
    pub decode_errors: u32,
    /// First error messages (bounded).
    pub first_errors: Vec<String>,
    /// True when the cap stopped collection early.
    pub capped: bool,
    /// Block 20: global member index (into the paired
    /// `WorldScanReport::index.entries`) per collected mesh — aligned
    /// with `meshes` ONLY when this report came from
    /// [`collect_meshes_near`] (the only pass that knows the member
    /// each mesh came from); empty for the other collectors.
    pub member_of_mesh: Vec<usize>,
    /// Block 21: per-mesh WORLD/LOCAL classification, aligned with
    /// `meshes`.  Evidence (two real cells.rcf members, block 21):
    /// baked geometry (light decals, merged drawable roots) carries
    /// WORLD-space vertices (centers 180–450 m out, street-level y),
    /// while per-building drawable shapes (bakeryShape, billBoard*, …)
    /// are LOCAL (centers < 7 m of the origin, radius < 6.5 m) and are
    /// placed by the object members' GameObject records — a pairing
    /// that is NOT decoded yet.  `true` = world-space (the player
    /// camera's plain view-projection applies); `false` = local
    /// (skipped by the boot's batch build until the GameObject
    /// drawable pairing lands).
    pub mesh_world: Vec<bool>,
    /// Count of classified-LOCAL meshes (kept in `meshes`, excluded
    /// from batch build by the boot).
    pub local_meshes: u32,
}

/// The world/local split threshold (block 21 evidence, see
/// [`MeshLoadReport::mesh_world`]).  Both measured members separate
/// cleanly: local props < 7 m, world geometry > 100 m; 32 m sits
/// mid-gap and survives float noise.
pub const WORLD_LOCAL_SPLIT_M: f32 = 32.0;

impl MeshLoadReport {
    fn note_error(&mut self, msg: String) {
        self.decode_errors += 1;
        if self.first_errors.len() < 4 {
            self.first_errors.push(msg);
        }
    }
}

/// Collect up to `max_meshes` real meshes from `archive_paths` (in order,
/// members in file order).  Never panics on bad data; never reads beyond
/// what `std::fs::read` returns.
pub fn collect_meshes<I, P>(archive_paths: I, max_meshes: usize) -> MeshLoadReport
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut rep = MeshLoadReport::default();
    for path in archive_paths {
        if rep.meshes.len() >= max_meshes {
            rep.capped = true;
            break;
        }
        let data = match std::fs::read(path.as_ref()) {
            Ok(d) => d,
            Err(e) => {
                rep.archives_failed += 1;
                rep.note_error(format!("{}: read failed: {e}", path.as_ref().display()));
                continue;
            }
        };
        match prototype_rcf::RcfArchive::parse(&data) {
            Ok(archive) => {
                rep.archives_opened += 1;
                collect_from_archive(&archive, max_meshes, &mut rep);
            }
            Err(e) => {
                rep.archives_failed += 1;
                rep.note_error(format!("{}: parse failed: {e}", path.as_ref().display()));
            }
        }
    }
    rep
}

/// Scan one opened archive for Shape chunks.
pub fn collect_from_archive(
    archive: &prototype_rcf::RcfArchive<'_>,
    max_meshes: usize,
    rep: &mut MeshLoadReport,
) {
    for entry in &archive.entries {
        if rep.meshes.len() >= max_meshes {
            rep.capped = true;
            return;
        }
        rep.members_scanned += 1;
        let p3d = match archive.payload_decompressed(entry) {
            Ok(bytes) => bytes,
            Err(e) => {
                rep.note_error(format!("entry {}: decompress failed: {e}", entry.file_offset));
                continue;
            }
        };
        collect_from_p3d(&p3d, max_meshes, rep);
    }
}

/// Scan one P3D buffer for Shape/Mesh chunks (also usable directly on
/// loose `.p3d` files).  After collecting the meshes, the member's own
/// shader/texture chunks resolve each mesh's diffuse texture through the
/// proven chain: mesh name == pure3dShader name → its `0x11016` `color`
/// param names a texture file → the member's Texture chunk of that name.
/// Every link is proven individually (block 16/17); a miss at any link
/// simply leaves `texture_dds` None.
///
/// Block 24: the shader's OTHER texture params (`normal`, `grime`,
/// `glass_shatter`, `reflection`, …) resolve into
/// [`SceneMesh::extra_textures`] in param order (capped at
/// [`MAX_EXTRA_TEXTURES`]) — real evidence: 123 of the 197 shaders in
/// the cell-82 member name ≥2 texture slots, and the committed
/// `cells82_multitex.p3d` fixture carries a real `color`+`normal`
/// shader with BOTH textures in-member.
pub fn collect_from_p3d(p3d: &[u8], max_meshes: usize, rep: &mut MeshLoadReport) {
    let Some(top) = geo::top_level_chunks(p3d) else {
        return; // not a P3D file (GFX, raw DDS, …) — not an error
    };
    rep.members_with_p3d += 1;

    let (shaders, textures) = member_shader_texture_maps(p3d, &top);

    for chunk in &top {
        if rep.meshes.len() >= max_meshes {
            rep.capped = true;
            return;
        }
        if chunk.chunk_type != geo::CHUNK_TYPE_SHAPE {
            continue;
        }
        match geo::decode_shape(p3d, chunk.offset, chunk.size) {
            Ok(shape) => {
                rep.shapes_decoded += 1;
                for mesh in &shape.meshes {
                    if rep.meshes.len() >= max_meshes {
                        rep.capped = true;
                        return;
                    }
                    rep.meshes_decoded += 1;
                    let mut scene_mesh = SceneMesh::from_p3d_mesh(mesh);
                    let (diffuse, extras) = resolve_member_textures(
                        &shaders,
                        &textures,
                        mesh.name.trim_end_matches('\u{0}'),
                    );
                    scene_mesh.texture_dds = diffuse;
                    scene_mesh.extra_textures = extras;
                    // Block 21 world/local classification (evidence in
                    // MeshLoadReport::mesh_world): bounds-center distance
                    // from the origin decides whether the player camera's
                    // plain VP applies (world) or the mesh needs a
                    // placement this port has not decoded yet (local).
                    let world = scene_mesh
                        .bounds_center()
                        .map(|c| (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt() >= WORLD_LOCAL_SPLIT_M)
                        .unwrap_or(false);
                    if !world {
                        rep.local_meshes += 1;
                    }
                    rep.mesh_world.push(world);
                    rep.meshes.push(scene_mesh);
                }
            }
            Err(e) => rep.note_error(format!(
                "member {}: shape decode failed: {e}",
                entry_name_of(p3d, chunk)
            )),
        }
    }
}

/// Cap on [`SceneMesh::extra_textures`] (bounds per-mesh texture memory;
/// the observed world-shader maximum is 4 texture slots).
pub const MAX_EXTRA_TEXTURES: usize = 4;

/// Member-indexed shaders (by chunk name, NUL-trimmed) and textures (by
/// chunk name → DDS bytes) for the texture chain — shared by
/// [`collect_from_p3d`] and [`collect_paired_meshes`] so BOTH load paths
/// resolve the section→shader→texture chain (block 25).
fn member_shader_texture_maps(
    p3d: &[u8],
    top: &[geo::ChildChunk],
) -> (
    std::collections::BTreeMap<String, geo::ShaderChunk>,
    std::collections::BTreeMap<String, Vec<u8>>,
) {
    let mut shaders: std::collections::BTreeMap<String, geo::ShaderChunk> = Default::default();
    let mut textures: std::collections::BTreeMap<String, Vec<u8>> = Default::default();
    for chunk in top {
        match chunk.chunk_type {
            geo::CHUNK_TYPE_SHADER => {
                if let Ok(sh) = geo::decode_shader_chunk(p3d, chunk.offset, chunk.size) {
                    shaders.insert(sh.name.trim_end_matches('\u{0}').to_string(), sh);
                }
            }
            geo::CHUNK_TYPE_TEXTURE => {
                if let Ok(tex) = geo::decode_texture(p3d, chunk.offset, chunk.size) {
                    textures
                        .insert(tex.name.trim_end_matches('\u{0}').to_string(), tex.dds);
                }
            }
            _ => {}
        }
    }
    (shaders, textures)
}

/// The proven texture chain, refactored out of [`collect_from_p3d`] so
/// it is unit-testable against the real multi-texture fixture
/// (`cells82_multitex.p3d`): the shader named like the mesh supplies a
/// `color` param (→ the diffuse DDS) plus any further `0x11016`
/// texture params (→ named extra DDS files, param order, capped).
/// Returns `(diffuse, extras)`; any unresolved link degrades that part
/// only (fail-soft, never an error).
pub fn resolve_member_textures(
    shaders: &std::collections::BTreeMap<String, geo::ShaderChunk>,
    textures: &std::collections::BTreeMap<String, Vec<u8>>,
    mesh_name: &str,
) -> (Option<Vec<u8>>, Vec<prototype_render::scene::NamedDds>) {
    let Some(sh) = shaders.get(mesh_name) else {
        return (None, Vec::new());
    };
    let mut diffuse = None;
    let mut extras = Vec::new();
    for p in &sh.params {
        if p.chunk_type != 0x0001_1016 {
            continue;
        }
        let Some(val) = p.value.as_deref() else {
            continue;
        };
        let Some(dds) = textures.get(val) else {
            continue;
        };
        if p.name.as_deref() == Some("color") && diffuse.is_none() {
            diffuse = Some(dds.clone());
        } else if extras.len() < MAX_EXTRA_TEXTURES {
            extras.push(prototype_render::scene::NamedDds {
                slot: p.name.clone().unwrap_or_default(),
                dds: dds.clone(),
            });
        }
    }
    (diffuse, extras)
}

/// Best-effort member identification for error messages: the shape name,
/// or the offset when the name is unreadable.
fn entry_name_of(p3d: &[u8], chunk: &geo::ChildChunk) -> String {
    let start = chunk.payload();
    if start < p3d.len() {
        let n = p3d[start] as usize;
        if start + 1 + n <= p3d.len() {
            return String::from_utf8_lossy(&p3d[start + 1..start + 1 + n])
                .trim_end_matches('\u{0}')
                .to_string();
        }
    }
    format!("@{:#x}", chunk.offset)
}

// ---------------------------------------------------------------------------
// M25 world index + camera-near loading (block 18)
// ---------------------------------------------------------------------------

/// What one world-index pass found.
#[derive(Debug, Default)]
pub struct WorldScanReport {
    /// The per-member index (cells, kinds, bboxes, counters), in
    /// archive/member order.
    pub index: prototype_p3d::world::WorldIndex,
    /// RCF archives opened successfully.
    pub archives_opened: u32,
    /// Archives that failed to open.
    pub archives_failed: u32,
    /// Members examined.
    pub members_scanned: u32,
    /// Decompress failures.
    pub errors: u32,
    /// First error messages (bounded).
    pub first_errors: Vec<String>,
}

/// Build the streamed-world index over the user's `.rcf` archives.
///
/// Decompresses every member once and runs the cheap
/// [`prototype_p3d::world::scan_world_member`] pass (chunk preludes +
/// names + placements — no mesh decode).  On the real cells.rcf this is
/// one full zlib pass over ~660 MB compressed; the loader design
/// accepts that because a partial index is useless (cell membership is
/// inside the compressed streams — there is no sidecar directory).
///
/// `retain_objects` (block 20) switches the scan to
/// [`prototype_p3d::world::ScanOptions { retain_objects: true }`] —
/// the index then carries the full decoded GameObjects, which
/// [`prototype_engine::WorldObjectSystem::spawn_from_index`] consumes
/// to spawn the REAL placed-object database.  Same single decompress
/// pass, ~10 MB extra at corpus scale.
pub fn collect_world_index<I, P>(
    archive_paths: I,
    opts: prototype_p3d::world::ScanOptions,
) -> WorldScanReport
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut rep = WorldScanReport::default();
    for path in archive_paths {
        let data = match std::fs::read(path.as_ref()) {
            Ok(d) => d,
            Err(e) => {
                rep.archives_failed += 1;
                if rep.first_errors.len() < 4 {
                    rep.first_errors
                        .push(format!("{}: read failed: {e}", path.as_ref().display()));
                }
                continue;
            }
        };
        match prototype_rcf::RcfArchive::parse(&data) {
            Ok(archive) => {
                rep.archives_opened += 1;
                for entry in &archive.entries {
                    rep.members_scanned += 1;
                    match archive.payload_decompressed(entry) {
                        Ok(p3d) => rep
                            .index
                            .entries
                            .push(prototype_p3d::world::scan_world_member_opts(
                                entry.name_hash, &p3d, opts,
                            )),
                        Err(e) => {
                            rep.errors += 1;
                            if rep.first_errors.len() < 4 {
                                rep.first_errors.push(format!(
                                    "entry hash {:#x}: decompress failed: {e}",
                                    entry.name_hash
                                ));
                            }
                        }
                    }
                }
            }
            Err(e) => {
                rep.archives_failed += 1;
                if rep.first_errors.len() < 4 {
                    rep.first_errors
                        .push(format!("{}: parse failed: {e}", path.as_ref().display()));
                }
            }
        }
    }
    rep
}

/// Collect real meshes from cells.rcf-style archives with the world
/// index ordering the load: members whose placement bbox is nearest
/// `cam` are decoded FIRST (the streamed-world behavior), no-bbox
/// members last in index order.  Pass 1 indexes (decompress + scan),
/// pass 2 re-decompresses members one at a time in near order — bounded
/// memory, and the mesh cap applies across the whole pass.
///
/// `retain_objects` (block 20): scan with full-object retention so the
/// returned index can feed the engine crate's
/// `WorldObjectSystem::spawn_from_index` — ONE decompress pass gives
/// both the meshes and the real placed-object database.  Every
/// collected mesh also records its global member index
/// (`MeshLoadReport::member_of_mesh`), the provenance the per-frame
/// [`DrawSelector`] selection consumes.
pub fn collect_meshes_near<I, P>(
    archive_paths: I,
    cam: [f32; 3],
    max_meshes: usize,
    opts: prototype_p3d::world::ScanOptions,
) -> (MeshLoadReport, WorldScanReport)
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let paths: Vec<std::path::PathBuf> =
        archive_paths.into_iter().map(|p| p.as_ref().to_path_buf()).collect();
    let scan_opts = opts;
    let mut world = WorldScanReport::default();
    // pass 1: parse every archive once, index every member; remember
    // (global member index) -> (archive slot, local entry index).
    let mut archive_data: Vec<Vec<u8>> = Vec::new();
    let mut member_locs: Vec<(usize, usize)> = Vec::new();
    for path in &paths {
        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(e) => {
                world.archives_failed += 1;
                if world.first_errors.len() < 4 {
                    world.first_errors.push(format!("{}: read failed: {e}", path.display()));
                }
                continue;
            }
        };
        // The archive borrows `data`, so the owned copy is stored AFTER
        // the index pass below; pass 2 re-parses from the owned bytes
        // (the header+directory parse is O(entries), negligible).
        let archive = match prototype_rcf::RcfArchive::parse(&data) {
            Ok(a) => a,
            Err(e) => {
                world.archives_failed += 1;
                if world.first_errors.len() < 4 {
                    world.first_errors.push(format!("{}: parse failed: {e}", path.display()));
                }
                continue;
            }
        };
        world.archives_opened += 1;
        let slot = archive_data.len();
        for (li, entry) in archive.entries.iter().enumerate() {
            world.members_scanned += 1;
            match archive.payload_decompressed(entry) {
                Ok(p3d) => {
                    world.index.entries.push(prototype_p3d::world::scan_world_member_opts(
                        entry.name_hash,
                        &p3d,
                        scan_opts,
                    ));
                    member_locs.push((slot, li));
                }
                Err(e) => {
                    world.errors += 1;
                    if world.first_errors.len() < 4 {
                        world.first_errors.push(format!(
                            "entry hash {:#x}: decompress failed: {e}",
                            entry.name_hash
                        ));
                    }
                }
            }
        }
        archive_data.push(data);
    }
    // pass 2: near order over (distance, global index)
    let mut order: Vec<usize> = (0..world.index.entries.len()).collect();
    order.sort_by_key(|&gi| {
        let d = world.index.entries[gi].distance_from(cam);
        (d.is_infinite(), d.to_bits())
    });
    let mut rep = MeshLoadReport::default();
    for gi in order {
        if rep.meshes.len() >= max_meshes {
            rep.capped = true;
            break;
        }
        let (slot, li) = member_locs[gi];
        let data = &archive_data[slot];
        let Ok(archive) = prototype_rcf::RcfArchive::parse(data) else {
            continue; // parsed fine in pass 1; cannot fail here
        };
        let Some(entry) = archive.entries.get(li) else { continue };
        rep.members_scanned += 1;
        match archive.payload_decompressed(entry) {
            Ok(p3d) => collect_from_p3d(&p3d, max_meshes, &mut rep),
            Err(e) => rep.note_error(format!(
                "entry hash {:#x}: decompress failed: {e}",
                entry.name_hash
            )),
        }
        // provenance: every mesh this member yielded belongs to it
        while rep.member_of_mesh.len() < rep.meshes.len() {
            rep.member_of_mesh.push(gi);
        }
        debug_assert_eq!(rep.member_of_mesh.len(), rep.meshes.len());
    }
    (rep, world)
}

// ---------------------------------------------------------------------------
// M25b (block 20): per-frame draw selection over the loaded batches
// ---------------------------------------------------------------------------

/// Per-frame draw selection — the draw path's streaming step.
///
/// Boot loads a bounded set of members' meshes near-first
/// ([`collect_meshes_near`]); per frame, [`DrawSelector::select`]
/// answers "which batches draw for THIS camera" using the same real
/// world index: members are ordered by [`members_near`]
/// (`WorldIndex::members_near` — bbox-center distance to the camera,
/// no-bbox members last) and only the `max_members` nearest members'
/// batches encode draw calls.  Memberless batches (the debug
/// triangles) always draw.
///
/// The camera is static in today's boot path ([0,0,0] — mid-map), so
/// the selection is currently stable; the mechanism exists so the
/// player-camera wiring (NEXT) gets per-frame culling for free.
///
/// [`members_near`]: prototype_p3d::world::WorldIndex::members_near
#[derive(Debug, Clone, Default)]
pub struct DrawSelector {
    /// Global member index per batch (aligned with the app's batch
    /// vector), `None` for batches not tied to a member.
    member_of_batch: Vec<Option<usize>>,
}

impl DrawSelector {
    /// Build from the batch→member provenance.  For real-mesh batches
    /// that is `MeshLoadReport::member_of_mesh[gi]` (one entry per
    /// batch a member yielded); debug batches push `None`.
    pub fn new(member_of_batch: Vec<Option<usize>>) -> Self {
        Self { member_of_batch }
    }

    /// The provenance (aligned with the batch vector).
    pub fn member_of_batch(&self) -> &[Option<usize>] {
        &self.member_of_batch
    }

    /// Keep only the batches whose CURRENT index is in `keep` — the
    /// streaming unload's provenance compaction (block 44).  The App
    /// drops the evicted members' batches (`Vec::retain` over the
    /// SAME keep set) and calls this with it; provenance stays
    /// aligned with the surviving batch order, and every surviving
    /// memberless batch keeps drawing.
    pub fn retain(&mut self, keep: &std::collections::BTreeSet<usize>) {
        self.member_of_batch = self
            .member_of_batch
            .iter()
            .enumerate()
            .filter(|(i, _)| keep.contains(i))
            .map(|(_, m)| *m)
            .collect();
    }

    /// Append one provenance row for a newly added batch — the
    /// streaming load path keeps the selector aligned with the App's
    /// batch vector as it GROWS (`retain` compacts it as it shrinks).
    pub fn push_provenance(&mut self, member: Option<usize>) {
        self.member_of_batch.push(member);
    }

    /// Batch indices to draw for `cam`, in batch order: every batch
    /// whose member is among the `max_members` nearest (the
    /// `WorldIndex::members_near_indices` ordering); memberless batches
    /// (debug triangles) always draw.  Batches tied to members outside
    /// the selected set — including any out-of-range member index — are
    /// skipped this frame: draw selection fails CLOSED (the honest
    /// counters live in the scan reports).
    pub fn select(&self, world: &prototype_p3d::world::WorldIndex, cam: [f32; 3], max_members: usize) -> Vec<usize> {
        let near: std::collections::BTreeSet<usize> = world
            .members_near_indices(cam)
            .into_iter()
            .take(max_members)
            .collect();
        self.member_of_batch
            .iter()
            .enumerate()
            .filter(|(_, m)| match m {
                None => true,
                Some(gi) => near.contains(gi),
            })
            .map(|(i, _)| i)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Block 22: paired-drawable mesh loading (GameObject -> drawable -> Shape)
// ---------------------------------------------------------------------------

/// One loaded LOCAL drawable mesh plus the REAL record that places it
/// (block 22).  The mesh vertices are LOCAL (the member's per-building
/// shape geometry); `pair.placement` (binary records) is the WORLD
/// position the boot composes model rows from.
#[derive(Debug, Clone)]
pub struct PairedMesh {
    /// The LOCAL-shape mesh (vertices near the origin).
    pub mesh: SceneMesh,
    /// The REAL record that places it (binary → world placement).
    pub pair: prototype_p3d::world::PairedDrawable,
}

/// What one paired-drawable load pass found.
#[derive(Debug, Clone, Default)]
pub struct PairedMeshReport {
    /// Loaded meshes (capped at `max_meshes`), nearest-member-first.
    pub meshes: Vec<PairedMesh>,
    /// Pairs requested (input length).
    pub pairs_requested: usize,
    /// Prop-table pairs skipped — their placements are LOCAL and the
    /// record→node-transform composition is UNKNOWN (fail-closed).
    pub skipped_prop_table: u32,
    /// Geometry members decompressed (each once).
    pub members_decompressed: u32,
    /// Pairs whose shape chunk was found and decoded.
    pub shapes_decoded: u32,
    /// Pairs whose shape chunk was NOT found in the member (name
    /// mismatch — honest counter).
    pub shapes_missing: u32,
    /// Decode failures (bounded messages).
    pub errors: u32,
    /// First error messages (bounded).
    pub first_errors: Vec<String>,
}

impl PairedMeshReport {
    fn note_error(&mut self, msg: String) {
        self.errors += 1;
        if self.first_errors.len() < 4 {
            self.first_errors.push(msg);
        }
    }
}

// ---------------------------------------------------------------------------
// Block 26: per-frame distance culling for the paired-batch pool
// ---------------------------------------------------------------------------

/// Per-frame draw selection for the paired pool — the distance half of
/// the streaming contract (the member half is [`DrawSelector`]).
///
/// Boot caps the paired pool at load time (`PAIRED_MESH_CAP` in the
/// app) to bound GPU memory, but every pooled batch used to encode a
/// draw call EVERY frame.  This selector culls per frame: only
/// placements within `radius_m` of `player` draw, nearest-first, at
/// most `cap` of them.
///
/// * PROVEN inputs: the positions are the REAL binary-record world
///   placements (block 22 — the same values the per-frame
///   `WVP · translate·rotate·scale` model rows translate by).
/// * DESIGN (documented tuning, LIKELY): the original engine's draw
///   distance / per-frame draw budget is UNKNOWN (engine-side
///   streaming constants); 400 m + cap 64 are clean-room defaults the
///   device run can tune.
///
/// Returns pool indices, nearest-first; distance ties keep boot order
/// (stable).  Squared distances throughout — no sqrt in the hot path.
pub fn select_paired_near(
    positions: &[[f32; 3]],
    player: [f32; 3],
    radius_m: f32,
    cap: usize,
) -> Vec<usize> {
    let r2 = radius_m * radius_m;
    let mut scored: Vec<(f32, usize)> = positions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let d = [p[0] - player[0], p[1] - player[1], p[2] - player[2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2], i)
        })
        .filter(|(d2, _)| *d2 <= r2)
        .collect();
    scored.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
    scored.truncate(cap);
    scored.into_iter().map(|(_, i)| i).collect()
}

/// Load the meshes of the paired drawables (block 22) — the LOCAL half
/// of the mesh path.  For every BINARY-variant pair (world placement
/// PROVEN), decompress its geometry member once, find the Shape chunk
/// by the paired name, decode it, and emit each mesh with the record's
/// placement.  Members are processed grouped (one decompress each),
/// nearest-camera-first within the member; the cap applies to meshes.
///
/// Prop-table pairs are counted and SKIPPED: their transforms are
/// stage-local and the node-transform composition is not decoded yet —
/// drawing them unplaced would litter the map origin (block 21 policy).
pub fn collect_paired_meshes<I, P>(
    archive_paths: I,
    pairs: &[prototype_p3d::world::PairedDrawable],
    cam: [f32; 3],
    max_meshes: usize,
) -> PairedMeshReport
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let paths: Vec<std::path::PathBuf> =
        archive_paths.into_iter().map(|p| p.as_ref().to_path_buf()).collect();
    let mut rep = PairedMeshReport {
        pairs_requested: pairs.len(),
        ..Default::default()
    };

    // Rebuild the global-member-index → (archive slot, entry) map in
    // the SAME order collect_meshes_near built the index (archives in
    // input order, entries in file order).
    let mut archive_data: Vec<Vec<u8>> = Vec::new();
    let mut member_locs: Vec<Option<(usize, usize)>> = Vec::new();
    for path in &paths {
        let Ok(data) = std::fs::read(path) else {
            continue;
        };
        let Ok(archive) = prototype_rcf::RcfArchive::parse(&data) else {
            continue;
        };
        let slot = archive_data.len();
        for (li, _) in archive.entries.iter().enumerate() {
            member_locs.push(Some((slot, li)));
        }
        archive_data.push(data);
    }

    // Only placeable (binary) pairs; nearest first, grouped per member
    // so each geometry member decompresses exactly once.
    let mut ordered: Vec<&prototype_p3d::world::PairedDrawable> = pairs
        .iter()
        .filter(|p| {
            let keep = p.variant == prototype_p3d::world::ObjectVariant::Binary;
            if !keep {
                rep.skipped_prop_table += 1;
            }
            keep
        })
        .collect();
    ordered.sort_by_key(|p| {
        let dx = p.placement.pos[0] - cam[0];
        let dy = p.placement.pos[1] - cam[1];
        let dz = p.placement.pos[2] - cam[2];
        (p.geometry_member, (dx * dx + dy * dy + dz * dz).to_bits())
    });

    let mut cur_member: Option<usize> = None;
    let mut cur_p3d: Vec<u8> = Vec::new();
    for pair in ordered {
        if rep.meshes.len() >= max_meshes {
            break; // cap hit: remaining pairs simply stay unloaded
        }
        // decompress on member change
        if cur_member != Some(pair.geometry_member) {
            cur_member = None;
            let Some(Some((slot, li))) = member_locs.get(pair.geometry_member).copied() else {
                rep.note_error(format!(
                    "pair {}: geometry member {} out of range",
                    pair.shape, pair.geometry_member
                ));
                continue;
            };
            let data = &archive_data[slot];
            let Ok(archive) = prototype_rcf::RcfArchive::parse(data) else {
                rep.note_error(format!("pair {}: member reparse failed", pair.shape));
                continue;
            };
            let Some(entry) = archive.entries.get(li) else {
                continue;
            };
            match archive.payload_decompressed(entry) {
                Ok(p3d) => {
                    cur_p3d = p3d;
                    cur_member = Some(pair.geometry_member);
                    rep.members_decompressed += 1;
                }
                Err(e) => {
                    rep.note_error(format!("pair {}: decompress failed: {e}", pair.shape));
                    continue;
                }
            }
        }
        // find the paired Shape chunk by name
        let Some(top) = geo::top_level_chunks(&cur_p3d) else {
            rep.note_error(format!("pair {}: member not a P3D", pair.shape));
            continue;
        };
        let mut found = false;
        for chunk in &top {
            if chunk.chunk_type != geo::CHUNK_TYPE_SHAPE {
                continue;
            }
            let name = geo_shape_name(&cur_p3d, chunk);
            if name.as_deref() != Some(pair.shape.as_str()) {
                continue;
            }
            found = true;
            match geo::decode_shape(&cur_p3d, chunk.offset, chunk.size) {
                Ok(shape) => {
                    rep.shapes_decoded += 1;
                    // Block 25: resolve the section→shader→texture chain
                    // for paired meshes too (same maps as
                    // collect_from_p3d; rebuilt per member change).
                    let (shaders, textures) = member_shader_texture_maps(&cur_p3d, &top);
                    for mesh in &shape.meshes {
                        if rep.meshes.len() >= max_meshes {
                            break;
                        }
                        let mut scene_mesh = SceneMesh::from_p3d_mesh(mesh);
                        let (diffuse, extras) = resolve_member_textures(
                            &shaders,
                            &textures,
                            mesh.name.trim_end_matches('\u{0}'),
                        );
                        scene_mesh.texture_dds = diffuse;
                        scene_mesh.extra_textures = extras;
                        rep.meshes.push(PairedMesh {
                            mesh: scene_mesh,
                            pair: pair.clone(),
                        });
                    }
                }
                Err(e) => {
                    rep.note_error(format!("pair {}: shape decode failed: {e}", pair.shape));
                }
            }
            break;
        }
        if !found {
            rep.shapes_missing += 1;
        }
    }
    rep
}

/// The u8-pascal name at the start of a Shape chunk (NUL-trimmed) —
/// same layout as `prototype_p3d::world`'s private helper.
fn geo_shape_name(p3d: &[u8], chunk: &geo::ChildChunk) -> Option<String> {
    let start = chunk.payload();
    let n = *p3d.get(start)? as usize;
    if n == 0 || start + 1 + n > p3d.len() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&p3d[start + 1..start + 1 + n])
            .trim_end_matches('\u{0}')
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Block 22: the paired-drawable load path on the REAL Cell_82
    /// members (cells_pairing.rcf = real object member 0x51A2CA3B +
    /// rebuilt real-shape member of 0x2268DE7E).  4 pairs come out of
    /// the index (2 binary + 2 prop-table); the collector loads ONLY
    /// the binary ones' shapes, from ONE member decompress, nearest
    /// camera first, and every loaded mesh is LOCAL (bounds center
    /// < WORLD_LOCAL_SPLIT_M of the origin) with the record's WORLD
    /// placement attached.
    #[test]
    fn collect_paired_meshes_places_local_shapes() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join("cells_pairing.rcf");
        let (_, world) = collect_meshes_near(
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

        // camera at the sign record's real world position: its mesh
        // must load first
        let cam = [199.99554, 27.7564, 331.50858];
        let rep = collect_paired_meshes([&path], &pairs.pairs, cam, 16);
        assert_eq!(rep.pairs_requested, 4);
        assert_eq!(rep.skipped_prop_table, 2);
        assert_eq!(rep.members_decompressed, 1);
        assert_eq!(rep.shapes_decoded, 2);
        assert_eq!(rep.shapes_missing, 0);
        assert_eq!(rep.errors, 0);
        assert!(rep.meshes.len() >= 2);
        assert_eq!(rep.meshes[0].pair.drawable, "signHorizontal001");
        let pos = rep.meshes[0].pair.placement.pos;
        assert!((pos[0] - 199.99554).abs() < 1e-3);
        assert!((pos[1] - 27.7564).abs() < 1e-3);
        assert!((pos[2] - 331.50858).abs() < 1e-3);
        // every loaded mesh is LOCAL geometry placed by a WORLD record
        for pm in &rep.meshes {
            let c = pm.mesh.bounds_center().expect("paired mesh has bounds");
            let r = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
            assert!(r < WORLD_LOCAL_SPLIT_M, "paired mesh not local: r={r}");
            assert_eq!(
                pm.pair.variant,
                prototype_p3d::world::ObjectVariant::Binary
            );
        }
        // Block 25: the paired shapes member carries REAL cells sections
        // (billBoard001InitialShape = 6 sections, signHorizontal001 …) —
        // every paired mesh must carry REAL vertices, and the sign's
        // section→shader→texture chain resolves in-member (block-25
        // rebuild appended the 0x11015 shader 67742f11… + its color DDS).
        let mut textured = 0;
        for pm in &rep.meshes {
            assert!(
                pm.mesh.vertex_count() > 0,
                "paired mesh has no vertices — sections did not decode"
            );
            if pm.mesh.texture_dds.is_some() {
                textured += 1;
            }
        }
        assert!(textured >= 1, "sign texture chain must resolve in the paired path");
        // cap stops collection
        let rep1 = collect_paired_meshes([&path], &pairs.pairs, cam, 1);
        assert_eq!(rep1.meshes.len(), 1);
    }

    fn fixture(name: &str) -> Vec<u8> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    /// The real barrier member yields both shapes' meshes through the
    /// direct-P3D path.
    #[test]
    fn collect_from_real_barrier_p3d() {
        let buf = fixture("art_barrier_os002.p3d");
        let mut rep = MeshLoadReport::default();
        collect_from_p3d(&buf, 64, &mut rep);
        assert_eq!(rep.members_with_p3d, 1);
        assert_eq!(rep.shapes_decoded, 2);
        assert_eq!(rep.meshes_decoded, 2);
        assert_eq!(rep.meshes.len(), 2);
        assert_eq!(rep.decode_errors, 0);
        assert_eq!(rep.meshes[0].vertex_count(), 576);
        assert_eq!(rep.meshes[1].vertex_count(), 640);
    }

    /// The cap stops collection at exactly `max_meshes` and reports it.
    #[test]
    fn cap_stops_collection_and_flags_capped() {
        let buf = fixture("art_barrier_os002.p3d");
        let mut rep = MeshLoadReport::default();
        collect_from_p3d(&buf, 1, &mut rep);
        assert_eq!(rep.meshes.len(), 1);
        assert!(rep.capped);
    }

    /// Loose .p3d fixture files (not archives) also flow through.  The
    /// barrier member carries TWO shapes (Initial + Damaged), the rest
    /// one each.
    #[test]
    fn every_real_art_fixture_yields_meshes() {
        for (name, want) in [
            ("art_barrier_os002.p3d", 2),
            ("art_mesh_a031.p3d", 1),
            ("art_mesh_b031.p3d", 1),
            ("art_mesh_nouvmesh2010.p3d", 1),
            ("art_mesh_skin6000.p3d", 1),
        ] {
            let buf = fixture(name);
            let mut rep = MeshLoadReport::default();
            collect_from_p3d(&buf, 64, &mut rep);
            assert_eq!(rep.meshes_decoded, want, "{name}");
            assert_eq!(rep.decode_errors, 0, "{name}");
        }
    }

    /// A real (mesh-less) archive scans cleanly: shaders.rcf opens, has
    /// no P3D meshes, and reports zero errors.
    #[test]
    fn meshless_archive_scans_clean() {
        let data = fixture("shaders.rcf");
        let archive = prototype_rcf::RcfArchive::parse(&data).unwrap();
        let mut rep = MeshLoadReport::default();
        collect_from_archive(&archive, 64, &mut rep);
        assert!(rep.members_scanned > 0);
        assert_eq!(rep.decode_errors, 0);
        // Shader members are P3D but carry no Shape chunks.
        assert_eq!(rep.meshes.len(), 0);
    }

    /// Garbage input is silent (not a P3D — not an error, just skipped).
    #[test]
    fn garbage_p3d_is_skipped_silently() {
        let mut rep = MeshLoadReport::default();
        collect_from_p3d(b"not a p3d at all", 64, &mut rep);
        assert_eq!(rep.meshes.len(), 0);
        assert_eq!(rep.decode_errors, 0);
        assert_eq!(rep.members_with_p3d, 0);
    }

    /// THE texture chain, on the real barrier member: mesh '131185ef…'
    /// → shader (same name) → 'color' param → in-member Texture
    /// 'barrierplasticos002_diffuse.dds' → its DDS bytes (32 896 B).
    #[test]
    fn barrier_texture_chain_resolves_in_member() {
        let buf = fixture("art_barrier_os002.p3d");
        let mut rep = MeshLoadReport::default();
        collect_from_p3d(&buf, 64, &mut rep);
        assert_eq!(rep.meshes.len(), 2);
        for m in &rep.meshes {
            let dds = m.texture_dds.as_ref().expect("barrier texture resolved");
            assert_eq!(dds.len(), 32_896);
            assert_eq!(&dds[..4], b"DDS ");
        }
        // and it decodes to a full RGBA8 surface via the render crate
        let dds = rep.meshes[0].texture_dds.clone().unwrap();
        let px = prototype_render::dxt::decode_dds(&dds).unwrap();
        assert_eq!(px.len(), 256 * 256 * 4);
    }

    /// A member without a resolvable chain leaves texture_dds None and
    /// stays error-free (the art_mesh_* fixtures carry no Texture chunk).
    #[test]
    fn mesh_without_texture_chain_is_none() {
        let buf = fixture("art_mesh_b031.p3d");
        let mut rep = MeshLoadReport::default();
        collect_from_p3d(&buf, 64, &mut rep);
        assert_eq!(rep.meshes_decoded, 1);
        assert!(rep.meshes[0].texture_dds.is_none());
        assert_eq!(rep.decode_errors, 0);
    }

    /// Block 25 — M28: a REAL cells geometry-member sliver flows through
    /// `collect_from_p3d` end to end: both `0x00010020` sections decode
    /// via the art mesh path (V=8/I=12 + V=92/I=162, desc 0xB031), the
    /// first section's shader-hash name resolves the member's `color`
    /// texture, the DDS decodes to RGBA, and the sections classify LOCAL
    /// (per-building drawable placed by an object-member record — the
    /// 32 m split, block 21).
    #[test]
    fn cells_section_member_yields_real_meshes_and_texture() {
        let buf = fixture("cells_section_textured.p3d");
        let mut rep = MeshLoadReport::default();
        collect_from_p3d(&buf, 64, &mut rep);
        assert_eq!(rep.members_with_p3d, 1);
        assert_eq!(rep.shapes_decoded, 1);
        assert_eq!(rep.meshes_decoded, 2);
        assert_eq!(rep.decode_errors, 0);
        assert_eq!(rep.meshes.len(), 2);

        // Real vertices, straight from the member's vertex pools.
        assert_eq!(rep.meshes[0].vertex_count(), 8);
        assert_eq!(rep.meshes[0].triangle_count(), 4);
        assert_eq!(rep.meshes[1].vertex_count(), 92);
        assert_eq!(rep.meshes[1].triangle_count(), 54);

        // Both sections are LOCAL geometry (centers within 32 m).
        assert_eq!(rep.mesh_world, vec![false, false]);
        assert_eq!(rep.local_meshes, 2);

        // Texture chain on the first section's shader hash.
        let dds = rep.meshes[0].texture_dds.as_ref().expect("color texture resolved");
        assert_eq!(&dds[..4], b"DDS ");
        let px = prototype_render::dxt::decode_dds(dds).unwrap();
        assert!(!px.is_empty(), "DDS decodes to pixels");
        // The second section's shader (b2ec7bc9…) is not in the sliver —
        // fail-soft: no texture, no error.
        assert!(rep.meshes[1].texture_dds.is_none());
    }

    /// Block 24 — the MULTI-texture chain on REAL cell-82 data.  The
    /// fixture holds three verbatim chunks from the real geometry
    /// member: shader `0bb78845…` (family `zCBV2_env_color_glass`) with
    /// `color`→gen_c1_02_win2_diffuse AND `normal`→gen_c1_02_win2_normal
    /// (both textures present in the fixture).  The resolver must map
    /// color → `texture_dds` and normal → `extra_textures[0]`.
    #[test]
    fn cells82_multitex_chain_resolves_color_and_normal() {
        let buf = fixture("cells82_multitex.p3d");
        let top = prototype_p3d::geometry::top_level_chunks(&buf).unwrap();
        let mut shaders = std::collections::BTreeMap::new();
        let mut textures = std::collections::BTreeMap::new();
        for c in &top {
            match c.chunk_type {
                prototype_p3d::geometry::CHUNK_TYPE_SHADER => {
                    let sh = prototype_p3d::geometry::decode_shader_chunk(&buf, c.offset, c.size)
                        .unwrap();
                    assert_eq!(sh.family, "zCBV2_env_color_glass");
                    shaders.insert(sh.name.trim_end_matches('\u{0}').to_string(), sh);
                }
                prototype_p3d::geometry::CHUNK_TYPE_TEXTURE => {
                    let tex = prototype_p3d::geometry::decode_texture(&buf, c.offset, c.size)
                        .unwrap();
                    textures.insert(tex.name.trim_end_matches('\u{0}').to_string(), tex.dds);
                }
                _ => {}
            }
        }
        assert_eq!(shaders.len(), 1);
        assert_eq!(textures.len(), 2, "diffuse + normal both in-member");
        let mesh_name = shaders.keys().next().unwrap().clone();
        let (diffuse, extras) = resolve_member_textures(&shaders, &textures, &mesh_name);
        let diffuse = diffuse.expect("color resolves");
        assert_eq!(&diffuse[..4], b"DDS ");
        // DXT1 256²: full mip chain 43 704 B + 128 B header (the DDS
        // ships base+mips; the resolver hands the file over verbatim).
        assert_eq!(diffuse.len(), 43_832);
        assert_eq!(extras.len(), 1, "normal slot resolved");
        assert_eq!(extras[0].slot, "normal");
        assert_eq!(&extras[0].dds[..4], b"DDS ");
        // DXT5 256² 87 408 B data + 128 B header.
        assert_eq!(extras[0].dds.len(), 87_536);
        // both decode through the render crate's DXT path
        let px = prototype_render::dxt::decode_dds(&diffuse).unwrap();
        assert_eq!(px.len(), 256 * 256 * 4);
        let px = prototype_render::dxt::decode_dds(&extras[0].dds).unwrap();
        assert_eq!(px.len(), 256 * 256 * 4);
    }

    /// The resolver is total: an unknown mesh name degrades to
    /// (None, []) without panicking (fail-soft contract).
    #[test]
    fn resolver_unknown_mesh_is_none() {
        let shaders = std::collections::BTreeMap::new();
        let textures = std::collections::BTreeMap::new();
        let (diffuse, extras) = resolve_member_textures(&shaders, &textures, "nope");
        assert!(diffuse.is_none());
        assert!(extras.is_empty());
    }

    /// A missing archive path is counted, not fatal.
    #[test]
    fn missing_archive_is_counted() {
        let rep = collect_meshes(["/nonexistent/nope.rcf"], 8);
        assert_eq!(rep.archives_failed, 1);
        assert_eq!(rep.archives_opened, 0);
        assert_eq!(rep.first_errors.len(), 1);
    }

    // ------------------------------------------------------------------
    // Block 21: the PROVEN world/local vertex-space split
    // ------------------------------------------------------------------

    /// REAL cells geometry (the mini cells.rcf's merged-root shape) is
    /// WORLD-space: centers ~1270 m out, street-level y — the player
    /// camera's plain VP applies.  Classification marks every mesh
    /// world and skips none.
    #[test]
    fn real_cells_geometry_classifies_world() {
        let buf = fixture("cells_mini.rcf");
        let path = std::env::temp_dir().join("prototype_rs_test_cells_world.rcf");
        std::fs::write(&path, &buf).unwrap();
        let (rep, _world) = collect_meshes_near(
            [&path],
            [0.0; 3],
            16,
            prototype_p3d::world::ScanOptions::default(),
        );
        std::fs::remove_file(&path).ok();
        assert!(rep.meshes_decoded >= 3, "meshes {}", rep.meshes_decoded);
        assert_eq!(rep.mesh_world.len(), rep.meshes.len());
        assert!(rep.mesh_world.iter().all(|w| *w), "all cells meshes are world-space");
        assert_eq!(rep.local_meshes, 0);
        // And the coordinates really are Manhattan-scale.
        for m in &rep.meshes {
            let c = m.bounds_center().unwrap();
            assert!(c[0].abs() > WORLD_LOCAL_SPLIT_M, "center {c:?}");
        }
    }

    /// REAL art drawables (the barrier member) are LOCAL: both meshes
    /// sit within ~1.2 m of the origin — they need the GameObject
    /// placement pairing (NEXT), so they classify local and the boot
    /// skips them.
    #[test]
    fn real_art_drawables_classify_local() {
        let buf = fixture("art_barrier_os002.p3d");
        let mut rep = MeshLoadReport::default();
        collect_from_p3d(&buf, 64, &mut rep);
        assert_eq!(rep.meshes.len(), 2);
        assert_eq!(rep.mesh_world.len(), 2);
        assert!(!rep.mesh_world[0]);
        assert!(!rep.mesh_world[1]);
        assert_eq!(rep.local_meshes, 2);
        for m in &rep.meshes {
            let c = m.bounds_center().unwrap();
            assert!(
                (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt() < WORLD_LOCAL_SPLIT_M,
                "center {c:?}"
            );
        }
    }

    // ------------------------------------------------------------------
    // M25: world index + camera-near loading on the real mini cells.rcf
    // ------------------------------------------------------------------

    /// The mini cells.rcf (3 real members: objects/geometry/empty)
    /// indexes with the right kinds, cells, counts and hashes.
    #[test]
    fn world_index_on_real_mini_cells_rcf() {
        let data = fixture("cells_mini.rcf");
        let path = std::env::temp_dir().join("prototype_rs_test_cells_mini.rcf");
        std::fs::write(&path, &data).unwrap();
        let rep = collect_world_index([&path], prototype_p3d::world::ScanOptions::default());
        std::fs::remove_file(&path).ok();
        assert_eq!(rep.archives_opened, 1);
        assert_eq!(rep.archives_failed, 0);
        assert_eq!(rep.errors, 0, "{:?}", rep.first_errors);
        assert_eq!(rep.index.entries.len(), 3);
        let hashes: Vec<u32> = rep.index.entries.iter().map(|e| e.hash).collect();
        assert!(hashes.contains(&0x0049_b319), "hashes {hashes:08x?}");
        assert!(hashes.contains(&0x213a_bf76));
        assert!(hashes.contains(&0x010d_9ce1));
        let objects = rep
            .index
            .entries
            .iter()
            .find(|e| e.hash == 0x0049_b319)
            .expect("object member");
        assert_eq!(objects.kind, prototype_p3d::world::MemberKind::Objects);
        assert_eq!(objects.objects, 62);
        assert!(objects.cells.contains(&141), "{:?}", objects.cells);
        assert_eq!(objects.object_errors, 0, "{:?}", objects.error_notes);
        let geometry = rep
            .index
            .entries
            .iter()
            .find(|e| e.hash == 0x213a_bf76)
            .expect("geometry member");
        assert_eq!(geometry.kind, prototype_p3d::world::MemberKind::Geometry);
        assert_eq!(geometry.shapes, 1);
        assert_eq!(geometry.object_errors, 0);
        let (tot_objs, tot_shapes) = rep.index.totals();
        assert_eq!((tot_objs, tot_shapes), (62, 1));
    }

    /// Near-first loading: the geometry member (the only one with
    /// meshes) is loaded, the camera ordering places it before the
    /// object member, and the report is error-free.
    #[test]
    fn near_first_load_on_real_mini_cells_rcf() {
        let data = fixture("cells_mini.rcf");
        let path = std::env::temp_dir().join("prototype_rs_test_cells_mini2.rcf");
        std::fs::write(&path, &data).unwrap();
        let (rep, world) = collect_meshes_near(
            [&path],
            [1687.0, 11.0, -1845.0],
            16,
            prototype_p3d::world::ScanOptions::default(),
        );
        std::fs::remove_file(&path).ok();
        assert_eq!(rep.decode_errors, 0, "{:?}", rep.first_errors);
        // the geometry member's merged-root shape decodes into meshes
        assert!(rep.meshes_decoded >= 1, "meshes_decoded {}", rep.meshes_decoded);
        assert!(rep.meshes.iter().all(|m| m.vertex_count() > 0));
        // ordering: a member with a bbox near the camera sorts before
        // the geometry member (which has no bbox here)
        let order = world.index.members_near([1687.0, 11.0, -1845.0]);
        assert_eq!(order.len(), 3);
        assert_eq!(order[0].hash, 0x0049_b319, "object member is nearest");
    }

    /// An unreadable archive is counted by BOTH passes and yields no
    /// meshes.
    #[test]
    fn near_load_with_missing_archive() {
        let (rep, world) = collect_meshes_near(
            ["/nonexistent/nope.rcf"],
            [0.0; 3],
            8,
            prototype_p3d::world::ScanOptions::default(),
        );
        assert_eq!(rep.meshes.len(), 0);
        assert_eq!(world.archives_failed, 1);
        assert_eq!(world.archives_opened, 0);
    }

    // ------------------------------------------------------------------
    // Block 20: retention pass + per-frame draw selection
    // ------------------------------------------------------------------

    /// The retention pass: ONE decompress pass yields both the mesh
    /// load and the full decoded GameObject database — on the real
    /// mini cells.rcf, 62 retained records of which EXACTLY the 43
    /// binary CloudSpawners are spawnable; the 19 prop-table NIS stage
    /// props never reach spawn.
    #[test]
    fn retained_index_feeds_spawn_set() {
        let data = fixture("cells_mini.rcf");
        let path = std::env::temp_dir().join("prototype_rs_test_cells_mini3.rcf");
        std::fs::write(&path, &data).unwrap();
        let rep = collect_world_index(
            [&path],
            prototype_p3d::world::ScanOptions { retain_objects: true, ..Default::default() },
        );
        std::fs::remove_file(&path).ok();
        assert_eq!(rep.archives_opened, 1);
        assert_eq!(rep.errors, 0, "{:?}", rep.first_errors);
        let (retained, spawnable) = rep.index.retained_totals();
        assert_eq!(retained, 62);
        assert_eq!(spawnable, 43);
        assert!(rep
            .index
            .spawn_placements()
            .iter()
            .all(|p| p.object.variant == prototype_p3d::world::ObjectVariant::Binary));
        // without retention the same archive yields no spawn set
        let path = std::env::temp_dir().join("prototype_rs_test_cells_mini4.rcf");
        std::fs::write(&path, &data).unwrap();
        let rep = collect_world_index([&path], prototype_p3d::world::ScanOptions::default());
        std::fs::remove_file(&path).ok();
        assert_eq!(rep.index.retained_totals(), (0, 0));
    }

    /// collect_meshes_near records the batch→member provenance: every
    /// collected mesh maps back to the global index of the member it
    /// came from (here: the geometry member's meshes).
    #[test]
    fn mesh_provenance_records_member() {
        let data = fixture("cells_mini.rcf");
        let path = std::env::temp_dir().join("prototype_rs_test_cells_mini5.rcf");
        std::fs::write(&path, &data).unwrap();
        let (rep, world) = collect_meshes_near(
            [&path],
            [1687.0, 11.0, -1845.0],
            16,
            prototype_p3d::world::ScanOptions { retain_objects: true, ..Default::default() },
        );
        std::fs::remove_file(&path).ok();
        assert_eq!(rep.member_of_mesh.len(), rep.meshes.len());
        assert!(!rep.meshes.is_empty());
        for gi in &rep.member_of_mesh {
            let e = world.index.entries.get(*gi).expect("member in range");
            assert_eq!(e.hash, 0x213a_bf76, "the geometry member's meshes");
        }
    }

    /// DrawSelector: with the REAL 3-member index, selecting 1 member
    /// keeps only that member's batches (plus memberless debug
    /// batches); selecting all keeps everything; a far camera that
    /// ranks the object member first drops the geometry batches.
    #[test]
    fn draw_selector_selects_near_members_only() {
        let data = fixture("cells_mini.rcf");
        let path = std::env::temp_dir().join("prototype_rs_test_cells_mini6.rcf");
        std::fs::write(&path, &data).unwrap();
        let (rep, world) = collect_meshes_near(
            [&path],
            [1687.0, 11.0, -1845.0],
            16,
            prototype_p3d::world::ScanOptions::default(),
        );
        std::fs::remove_file(&path).ok();
        let n_real = rep.meshes.len();
        assert!(n_real >= 1);
        // batches: one per real mesh + one memberless debug batch
        let mut prov: Vec<Option<usize>> = rep.member_of_mesh.iter().map(|&g| Some(g)).collect();
        prov.push(None);
        let sel = DrawSelector::new(prov);
        // the near camera ranks the object member (bbox at the morgue)
        // FIRST; with 1 member selected only... the geometry member's
        // meshes have no bbox → ranked LAST → dropped
        let near = sel.select(&world.index, [1687.0, 11.0, -1845.0], 1);
        assert_eq!(near, vec![n_real], "only the memberless debug batch draws");
        // with every member selected, everything draws
        let all = sel.select(&world.index, [1687.0, 11.0, -1845.0], usize::MAX);
        assert_eq!(all, (0..n_real + 1).collect::<Vec<_>>());
        // camera ranking is the real members_near ordering: the object
        // member (bbox at the morgue) ranks FIRST; the no-bbox members
        // follow in stable (member) order — geometry (1) before empty (2)
        let order = world.index.members_near_indices([1687.0, 11.0, -1845.0]);
        assert_eq!(order, vec![0, 1, 2], "bbox member first, then stable no-bbox order");
        assert_eq!(rep.member_of_mesh[0], 1, "meshes came from the geometry member");
        let two = sel.select(&world.index, [1687.0, 11.0, -1845.0], 2);
        assert_eq!(two.len(), n_real + 1, "both mesh-bearing members in set");
    }

    /// Block 44 — the streaming unload's provenance compaction:
    /// `retain` drops exactly the evicted members' provenance rows and
    /// the surviving rows STAY ALIGNED with the surviving batch order,
    /// so `select` keeps answering with the right batches after the
    /// batch vector shifts.  Synthetic provenance (pure bookkeeping
    /// type); the real streaming unload runs in member_stream's tests.
    #[test]
    fn draw_selector_retain_compacts_provenance_in_sync() {
        // 3 member-tied batches (members 2, 1, 2) + 1 memberless.
        let sel0 = DrawSelector::new(vec![Some(2), Some(1), Some(2), None]);
        let mut sel = sel0.clone();
        // Drop batches 1 and 2 (keep 0 and 3): provenance [Some(2), None].
        let keep: std::collections::BTreeSet<usize> = [0usize, 3].into_iter().collect();
        sel.retain(&keep);
        assert_eq!(sel.member_of_batch(), &[Some(2), None]);
        // The order survives (no re-sorting): a memberless batch keeps
        // drawing after compaction.
        let data = fixture("cells_mini.rcf");
        let path = std::env::temp_dir().join("prototype_rs_test_cells_mini7.rcf");
        std::fs::write(&path, &data).unwrap();
        let (_, world) = collect_meshes_near(
            [&path],
            [0.0; 3],
            16,
            prototype_p3d::world::ScanOptions::default(),
        );
        std::fs::remove_file(&path).ok();
        let picked = sel.select(&world.index, [0.0; 3], usize::MAX);
        assert_eq!(picked, vec![0, 1], "survivor order + memberless draw");
        // Dropping EVERYTHING member-tied leaves the memberless batch.
        let mut sel2 = sel0;
        sel2.retain(&[3usize].into_iter().collect());
        assert_eq!(sel2.member_of_batch(), &[None]);
        let picked2 = sel2.select(&world.index, [0.0; 3], 1);
        assert_eq!(picked2, vec![0], "the memberless batch always draws");
    }

    /// Block 26 — the FULL material chain on real data, resolver → GPU
    /// source: `cells82_multitex.p3d` through the PRODUCTION map
    /// builders (`member_shader_texture_maps` + `resolve_member_textures`
    /// — the exact functions `collect_from_p3d`/`collect_paired_meshes`
    /// call) into a SceneMesh shaped exactly as the collectors write it,
    /// then `MeshBatchSource::from_mesh`, which must decode BOTH slots
    /// to uploadable RGBA8.  This is the seam the wgpu bind path
    /// consumes (`create_mesh_batches` slot 0 = diffuse, slot 1.. =
    /// extras in ascending-s# order).
    #[cfg(feature = "winit-app")]
    #[test]
    fn multitex_chain_flows_into_mesh_batch_source() {
        use prototype_render::scene::NamedDds;
        use prototype_render::wgpu_backend::{MeshBatchSource, MeshVertexLayout};

        let buf = fixture("cells82_multitex.p3d");
        let top = prototype_p3d::geometry::top_level_chunks(&buf).unwrap();
        let (shaders, textures) = member_shader_texture_maps(&buf, &top);
        let mesh_name = shaders
            .keys()
            .next()
            .expect("the fixture's one real shader")
            .clone();
        let (diffuse_dds, extras) = resolve_member_textures(&shaders, &textures, &mesh_name);
        let diffuse_dds = diffuse_dds.expect("color resolves in-member");
        assert_eq!(extras.len(), 1, "normal slot resolved");
        assert_eq!(extras[0].slot, "normal");

        // SceneMesh exactly as the collectors write it (fields beyond
        // the material chain are zero/empty — from_mesh only consumes
        // name/indices/vertex data + the DDS fields).
        let m = SceneMesh {
            name: mesh_name,
            desc: 0,
            positions: Vec::new(),
            normals: Vec::new(),
            uvs: Vec::new(),
            tangents: Vec::new(),
            indices: Vec::new(),
            bound_box: None,
            bound_sphere: None,
            texture_dds: Some(diffuse_dds),
            extra_textures: extras.into_iter().map(|NamedDds { slot, dds }| NamedDds { slot, dds }).collect(),
        };

        // A pipeline-shaped layout (position + uv + normal rows); the
        // layout only shapes vertex_bytes here — the material slots are
        // layout-independent.
        let layout = MeshVertexLayout {
            stride: 48,
            attributes: vec![
                prototype_render::wgpu_backend::wgpu::VertexAttribute {
                    format: prototype_render::wgpu_backend::wgpu::VertexFormat::Float32x4,
                    offset: 0,
                    shader_location: 0,
                },
                prototype_render::wgpu_backend::wgpu::VertexAttribute {
                    format: prototype_render::wgpu_backend::wgpu::VertexFormat::Float32x4,
                    offset: 16,
                    shader_location: 1,
                },
                prototype_render::wgpu_backend::wgpu::VertexAttribute {
                    format: prototype_render::wgpu_backend::wgpu::VertexFormat::Float32x4,
                    offset: 32,
                    shader_location: 19,
                },
            ],
        };
        let src = MeshBatchSource::from_mesh(&layout, &m);
        let diffuse = src.texture.as_ref().expect("diffuse decodes to RGBA8");
        assert_eq!(diffuse.width, 256);
        assert_eq!(diffuse.height, 256);
        assert_eq!(diffuse.rgba.len(), 256 * 256 * 4);
        assert_eq!(src.extra_textures.len(), 1, "normal decodes to RGBA8");
        let normal = &src.extra_textures[0];
        assert_eq!((normal.width, normal.height), (256, 256));
        assert_eq!(normal.rgba.len(), 256 * 256 * 4);
    }

    /// Block 26 — the paired-pool distance cull: nearest-first order,
    /// tie stability (boot order), radius drop, cap, and the zero-radius
    /// edge.  The frame loop feeds it the REAL placements (block-22
    /// provenance) and draws exactly the returned pool indices.
    #[test]
    fn select_paired_near_orders_culls_and_caps() {
        // Pool: #0 at 30 m, #1 at 10 m, #2 at 500 m (out of a 400 m
        // radius), #3 at 10 m (distance tie with #1 — later boot order).
        let pos = [
            [30.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [500.0, 0.0, 0.0],
            [0.0, 10.0, 0.0],
        ];
        let sel = select_paired_near(&pos, [0.0, 0.0, 0.0], 400.0, usize::MAX);
        assert_eq!(sel, vec![1, 3, 0], "nearest-first, tie keeps boot order, >radius dropped");
        // The cap keeps the NEAREST.
        assert_eq!(select_paired_near(&pos, [0.0, 0.0, 0.0], 400.0, 2), vec![1, 3]);
        // Exact boundary counts as inside (d == radius).
        assert_eq!(
            select_paired_near(&[[100.0, 0.0, 0.0]], [0.0, 0.0, 0.0], 100.0, usize::MAX),
            vec![0]
        );
        // Zero radius: nothing draws (fail-closed).
        assert_eq!(
            select_paired_near(&pos, [0.0, 0.0, 0.0], 0.0, usize::MAX),
            Vec::<usize>::new()
        );
        // Empty pool → empty selection.
        assert_eq!(
            select_paired_near(&[], [1.0, 2.0, 3.0], 400.0, 8),
            Vec::<usize>::new()
        );
        // Off-origin player: distance is measured to the PLAYER (the
        // streaming input), not the world origin.
        let off = select_paired_near(&[[210.0, 0.0, 0.0]], [200.0, 0.0, 0.0], 50.0, usize::MAX);
        assert_eq!(off, vec![0], "distance to the player, not the origin");
    }
}

//! world_objects — the REAL placed-object database (M25b, block 20).
//!
//! Until this module the runtime's world entities were SYNTHETIC: the
//! M11e streaming grid invented mega-cells and the boot path invented
//! grid entities.  Block 18 proved the real thing inside `cells.rcf`
//! (GameObject `0x07F00000` — 81,370 corpus-wide placed objects with
//! names, classes, world positions and quaternions); block 20 wires
//! those REAL placements into entity spawn.
//!
//! ## What spawns and what does not
//!
//! [`prototype_p3d::world::WorldIndex::spawn_placements`] yields the
//! spawn set: binary-variant GameObjects with a placement — the
//! spawner database (`proto::CloudSpawner`, `proto::ParkedCarSpawner`,
//! `proto::EffectsPropSpawner`, …).  Prop-table objects (`GameObject`,
//! `proto::LightDecalInfo`, `proto::ShaderPalette`) are EXCLUDED:
//! their placements are frequently stage-local NIS coordinates, not
//! world-space (PROVEN on the real Cell_141 member:
//! `GameObject(1):NIS_BoxStack1` at (0.1848, -0.15, -4.3822) is a
//! stage, not Manhattan).  Spawner-class behaviour itself (respawn
//! timers, cloud density) is UNKNOWN — this module stores the REAL
//! placement records only; the classes are inert data until a
//! behaviour layer lands.
//!
//! ## PROVEN vs LIKELY vs UNKNOWN
//!
//! * PROVEN: every spawned record's bytes (names, classes, positions,
//!   unit-ish quaternions, drawables, states) come from the decoded
//!   real chunks (corpus-validated, see `prototype_p3d::world`).
//! * LIKELY: per-cell grouping (`PlacedObject::cells`) is where the
//!   original engine's spawn gating lives (the RTTI dump lists
//!   spawner activators per region).
//! * UNKNOWN: the original engine's EntityId scheme — ours is the
//!   clean-room [`EntityId`] from M9, as for the free-roam events.

#![forbid(unsafe_code)]

use crate::entity::{Entity, EntityId, EntityStore};
use prototype_math::Vec3;
use prototype_p3d::world::WorldIndex;
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// WorldObject
// ---------------------------------------------------------------------------

/// One spawned world entity: the REAL record of a placed object.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldObject {
    /// Typed handle (set on spawn — see [`EntityStore`]).
    id: EntityId,
    /// First pascal, e.g. `proto::CloudSpawner(1):Cell_65Collectible004_38`.
    pub full_name: String,
    /// Class pascal, e.g. `proto::CloudSpawner`.
    pub class: String,
    /// World position (binary-variant placement).
    pub pos: Vec3,
    /// Quaternion x/y/z/w (the binary variant's order — note the node
    /// transform chunk uses w-first; the two layouts are NOT the same).
    pub quat_xyzw: [f32; 4],
    /// Drawable/model name when the record carries one (empty pascals
    /// — the ParkedCarSpawners — stay `None`).
    pub drawable: Option<String>,
    /// Scale vector.
    pub scale: [f32; 3],
    /// State string (usually `initial`).
    pub state: Option<String>,
    /// RCF hash of the member this record came from.
    pub member_hash: u32,
    /// The member's `Cell_NN` ids.
    pub cells: Vec<u32>,
}

impl Entity for WorldObject {
    fn id(&self) -> EntityId {
        self.id
    }
    fn name(&self) -> &str {
        &self.full_name
    }
}

impl WorldObject {
    /// Distance from `p` to the object's position.
    pub fn distance_from(&self, p: Vec3) -> f32 {
        (self.pos - p).length()
    }
}

// ---------------------------------------------------------------------------
// SpawnReport
// ---------------------------------------------------------------------------

/// Honest result of one spawn pass ([`WorldObjectSystem::spawn_from_index`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SpawnReport {
    /// Entities actually inserted.
    pub spawned: usize,
    /// Records skipped because their full name was already live
    /// (cross-member name collisions are possible; the store keeps
    /// names unique, so a duplicate is COUNTED, not hidden).
    pub duplicates: usize,
    /// `true` when `max` stopped the pass early (the rest of the
    /// spawn set was NOT loaded this boot).
    pub capped: bool,
}

// ---------------------------------------------------------------------------
// WorldObjectSystem
// ---------------------------------------------------------------------------

/// The runtime's real placed-object database.  Spawned once at boot
/// from a scanned `cells.rcf` index (device boot) or per-test from a
/// hand-built index; queried by the streaming/spawn logic
/// (`near`/`nearest`) and by the boot log (`class_counts`).
#[derive(Default)]
pub struct WorldObjectSystem {
    store: EntityStore<WorldObject>,
    /// Cumulative duplicates across every spawn pass.
    duplicate_names: u32,
    /// `true` when any spawn pass hit its cap.
    capped: bool,
}

impl std::fmt::Debug for WorldObjectSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorldObjectSystem")
            .field("entities", &self.store.len())
            .field("duplicate_names", &self.duplicate_names)
            .field("capped", &self.capped)
            .finish()
    }
}

impl WorldObjectSystem {
    /// New empty system.
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn every binary-variant placement from `index` (in member
    /// order, objects in chunk order), capped at `max` entities
    /// (Android memory: the corpus carries 81,370 objects; a phone
    /// boot spawns the spawner database once, ~10 MB of records).
    /// Replaces any previous set — a new index means a NEW world.
    pub fn spawn_from_index(&mut self, index: &WorldIndex, max: usize) -> SpawnReport {
        self.clear();
        self.spawn_more_from_index(index, max)
    }

    /// Spawn WITHOUT clearing (append semantics for tests and future
    /// per-cell streaming); the cap counts this pass alone.
    pub fn spawn_more_from_index(&mut self, index: &WorldIndex, max: usize) -> SpawnReport {
        let mut rep = SpawnReport::default();
        for placed in index.spawn_placements() {
            if rep.spawned >= max {
                rep.capped = true;
                self.capped = true;
                break;
            }
            let o = placed.object;
            let Some(pl) = &o.placement else { continue }; // spawn set never yields these
            let name = o.full_name.clone();
            let class = o.class.clone();
            let drawable = o.drawable.clone();
            let state = o.state.clone();
            let scale = o.scale.unwrap_or([1.0, 1.0, 1.0]);
            let member_hash = placed.member_hash;
            let cells = placed.cells.clone();
            let inserted = self.store.insert_with(&name, |id| WorldObject {
                id,
                full_name: name.clone(),
                class,
                pos: Vec3::new(pl.pos[0], pl.pos[1], pl.pos[2]),
                quat_xyzw: pl.quat_xyzw,
                drawable,
                scale,
                state,
                member_hash,
                cells,
            });
            match inserted {
                Some(_) => rep.spawned += 1,
                None => {
                    rep.duplicates += 1;
                    self.duplicate_names += 1;
                }
            }
        }
        rep
    }

    /// Drop every entity and reset the counters.
    pub fn clear(&mut self) {
        self.store = EntityStore::new();
        self.duplicate_names = 0;
        self.capped = false;
    }

    /// Live entity count.
    pub fn len(&self) -> usize {
        self.store.len()
    }

    pub fn is_empty(&self) -> bool {
        self.store.len() == 0
    }

    /// Cumulative duplicate-name skips.
    pub fn duplicate_count(&self) -> u32 {
        self.duplicate_names
    }

    /// `true` when a spawn pass hit its cap.
    pub fn capped(&self) -> bool {
        self.capped
    }

    /// Lookup by id.
    pub fn get(&self, id: EntityId) -> Option<&WorldObject> {
        self.store.get(id)
    }

    /// Lookup by full name.
    pub fn find(&self, name: &str) -> Option<&WorldObject> {
        self.store.find(name)
    }

    /// Iterate live entities (insertion order).
    pub fn iter(&self) -> impl Iterator<Item = &WorldObject> {
        self.store.iter()
    }

    /// Ids of entities within `radius` of `p`, in insertion order.
    pub fn near(&self, p: Vec3, radius: f32) -> Vec<EntityId> {
        self.store
            .iter()
            .filter(|o| o.distance_from(p) <= radius)
            .map(|o| o.id())
            .collect()
    }

    /// The `n` nearest entities to `p` (ties: insertion order — a
    /// stable sort by distance).  This is the camera-ordered spawn
    /// selection the streamed world uses.
    pub fn nearest(&self, p: Vec3, n: usize) -> Vec<EntityId> {
        let mut v: Vec<(f32, EntityId)> = self
            .store
            .iter()
            .map(|o| (o.distance_from(p), o.id()))
            .collect();
        v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        v.truncate(n);
        v.into_iter().map(|(_, id)| id).collect()
    }

    /// Class census (the boot log's "what did we really spawn" line).
    pub fn class_counts(&self) -> BTreeMap<&str, usize> {
        let mut m: BTreeMap<&str, usize> = BTreeMap::new();
        for o in self.store.iter() {
            *m.entry(o.class.as_str()).or_default() += 1;
        }
        m
    }
}

// ---------------------------------------------------------------------------
// Tests — real fixtures from the cells.rcf corpus
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use prototype_p3d::world::{scan_world_member_opts, ScanOptions};

    fn fixture(name: &str) -> Vec<u8> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    /// A real index holding the standalone CloudSpawner member.
    fn cloudspawner_index() -> WorldIndex {
        let buf = fixture("cells_go_v2_cloudspawner.p3d");
        WorldIndex {
            entries: vec![scan_world_member_opts(
                0x0049_B319,
                &buf,
                ScanOptions { retain_objects: true, ..Default::default() },
            )],
        }
    }

    /// Spawn the REAL CloudSpawner: entity fields round-trip the
    /// corpus-proven values; the entity is queryable by id, name,
    /// near and nearest.
    #[test]
    fn spawns_real_cloudspawner_entity() {
        let mut sys = WorldObjectSystem::new();
        let rep = sys.spawn_from_index(&cloudspawner_index(), 1000);
        assert_eq!(rep, SpawnReport { spawned: 1, duplicates: 0, capped: false });
        assert_eq!(sys.len(), 1);
        let o = sys.find("proto::CloudSpawner(1):Cell_65Collectible004_38").expect("by full name");
        assert_eq!(o.class, "proto::CloudSpawner");
        assert_eq!(o.drawable.as_deref(), Some("Collectible004"));
        assert_eq!(o.state.as_deref(), Some("initial"));
        assert_eq!(o.scale, [1.0, 1.0, 1.0]);
        assert_eq!(o.pos, Vec3::new(-244.10567, 15.696654, -62.826157));
        assert_eq!(o.quat_xyzw, [0.0, 0.0, 0.0, -1.0]);
        assert_eq!(o.member_hash, 0x0049_B319);
        assert!(o.cells.contains(&65), "cells {:?}", o.cells);
        assert_eq!(sys.class_counts(), BTreeMap::from([("proto::CloudSpawner", 1)]));
        // spatial queries
        let p = Vec3::new(-244.0, 15.0, -62.0);
        assert_eq!(sys.near(p, 5.0).len(), 1);
        assert_eq!(sys.near(Vec3::new(1000.0, 0.0, 0.0), 5.0).len(), 0);
        let near_ids = sys.nearest(p, 1);
        assert_eq!(near_ids.len(), 1);
        assert_eq!(sys.get(near_ids[0]).unwrap().full_name, o.full_name);
    }

    /// The REAL Cell_141 objects member: 62 records retained, EXACTLY
    /// the 43 binary CloudSpawners spawn; the 19 prop-table
    /// `GameObject`s (NIS stage props) never reach the store.
    #[test]
    fn nis_member_spawns_binary_only() {
        let buf = fixture("cells_member_objects_nis.p3d");
        let idx = WorldIndex {
            entries: vec![scan_world_member_opts(
                0x49B319,
                &buf,
                ScanOptions { retain_objects: true, ..Default::default() },
            )],
        };
        let mut sys = WorldObjectSystem::new();
        let rep = sys.spawn_from_index(&idx, 1000);
        assert_eq!(rep, SpawnReport { spawned: 43, duplicates: 0, capped: false });
        assert_eq!(sys.class_counts(), BTreeMap::from([("proto::CloudSpawner", 43)]));
        // every spawned entity sits at world scale (the morgue cluster)
        for o in sys.iter() {
            let r = o.distance_from(Vec3::ZERO);
            assert!(r > 1000.0, "spawned entity {r} not world-scale: {o:?}");
        }
        // spatial queries answer with the real cluster
        let morgue = Vec3::new(1687.0, 10.0, -1845.0);
        assert_eq!(sys.near(morgue, 50.0).len(), 43);
        // the first spawned entity is the member's first binary object
        let first = sys.get(EntityId(0)).unwrap();
        assert!(first.full_name.starts_with("proto::CloudSpawner"), "{first:?}");
    }

    /// The cap stops the pass and reports it honestly; duplicates
    /// (the same full name twice across passes) are counted, never
    /// hidden, and the first insertion wins.
    #[test]
    fn cap_and_duplicate_accounting() {
        let idx = cloudspawner_index();
        let mut sys = WorldObjectSystem::new();
        let rep = sys.spawn_from_index(&idx, 0);
        assert_eq!(rep, SpawnReport { spawned: 0, duplicates: 0, capped: true });
        assert!(sys.capped() && sys.is_empty());
        // one real spawn, then a re-append of the same index: the
        // second pass is all duplicates
        assert_eq!(sys.spawn_from_index(&idx, 10).spawned, 1);
        let rep = sys.spawn_more_from_index(&idx, 10);
        assert_eq!(rep, SpawnReport { spawned: 0, duplicates: 1, capped: false });
        assert_eq!(sys.len(), 1);
        assert_eq!(sys.duplicate_count(), 1);
        // clear resets everything
        sys.clear();
        assert!(sys.is_empty() && sys.duplicate_count() == 0 && !sys.capped());
    }

    /// nearest() is camera-ordered: with several real entities the
    /// closest record comes first (stable among ties).
    #[test]
    fn nearest_orders_by_real_distance() {
        // two REAL standalone spawner members at different places
        let buf = fixture("cells_go_v2_cloudspawner.p3d");
        let mut a = scan_world_member_opts(0xAA, &buf, ScanOptions { retain_objects: true, ..Default::default() });
        a.hash = 0xAA;
        // the NIS member's 43 morgue spawners give a real cluster
        let nis = fixture("cells_member_objects_nis.p3d");
        let b = scan_world_member_opts(0xBB, &nis, ScanOptions { retain_objects: true, ..Default::default() });
        let idx = WorldIndex { entries: vec![a, b] };
        let mut sys = WorldObjectSystem::new();
        let rep = sys.spawn_from_index(&idx, 1000);
        assert_eq!(rep.spawned, 44, "1 collectible + 43 morgue spawners");
        // nearest to the morgue: morgue records first, the collectible
        // (x≈-244, z≈-62) last
        let ids = sys.nearest(Vec3::new(1687.0, 10.0, -1845.0), 44);
        assert_eq!(ids.len(), 44);
        let last = sys.get(*ids.last().unwrap()).unwrap();
        assert_eq!(last.drawable.as_deref(), Some("Collectible004"));
        // and truncation keeps the closest only
        let one = sys.nearest(Vec3::new(1687.0, 10.0, -1845.0), 1);
        let o = sys.get(one[0]).unwrap();
        assert!(o.distance_from(Vec3::new(1687.0, 10.0, -1845.0)) < 10.0);
    }
}

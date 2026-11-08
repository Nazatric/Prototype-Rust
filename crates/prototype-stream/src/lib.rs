//! prototype-stream
//! =================
//! Clean-room reconstruction of the Prototype (2009) **world
//! streaming** model: typed cells + a [`StreamingGrid`] that
//! loads/unloads archives around the player position, plus the
//! member-level streaming state machine ([`members::MemberStream`],
//! M25c) that loads/unloads MEMBERS inside a mounted archive.
//!
//! ## What this crate provides
//!
//! * [`ArchiveKind`] — typed wrapper for one of the 5 PROVEN RCF
//!   archives the engine mounts at boot (`00cells`, `01cells`,
//!   `00art`, `01art`, `02art`).  Includes the canonical mount
//!   order and the `game:\`-prefixed path strings.
//! * [`CellId`] — typed identifier for one streaming cell (a
//!   rectangular region of the world owned by one archive).
//! * [`StreamingCell`] — one cell's metadata: id, owning archive,
//!   AABB center + radius.
//! * [`StreamingGrid`] — the cell registry; given a player
//!   position, returns the set of cells that should be loaded,
//!   in canonical archive mount order.
//! * [`LoadSet`] — the archive-level diff between the currently
//!   loaded set and the desired set: which archives to mount and
//!   which to unmount.
//! * [`members::MemberStream`] (M25c, block 43) — the MEMBER-level
//!   half: a bounded, deterministic budget/hysteresis state machine
//!   over the member table scanned out of a real `cells.rcf` index
//!   (near-first load, farthest-first unload, settlement-guaranteed
//!   diffs).  The real engine keeps the streamed archive mounted
//!   and streams members INSIDE it — this is that layer.
//!
//! ## What this crate does NOT provide
//!
//! * Actual archive file IO — the [`prototype_rcf`] crate parses
//!   archives; this crate computes the desired load/unload set
//!   from the player's position.  A future `prototype-boot`
//!   integration would feed the load set into
//!   `ContentPipeline::mount_archive`.
//! * The actual cell grid layout — we know the 5 archive names
//!   and the canonical mount order (PROVEN), but the spatial
//!   layout (which cell lives at which world coordinate) is
//!   UNKNOWN.  The first version of [`StreamingGrid`] therefore
//!   uses a single mega-cell per archive, all loaded at once —
//!   the load set is the full 5-archive mount list for any player
//!   position.  Spatial filtering will land when we recover the
//!   cell-grid layout from the archive contents.
//!
//! ## Confidence labels
//!
//! * PROVEN: the 5 archive names and the canonical mount order
//!   (`00cells → 01cells → 00art → 01art → 02art`) — Phase 2F
//!   recovery from `prototypef.exe`'s strings + Xenia's reference
//!   log.  See [`ArchiveKind::ALL_IN_MOUNT_ORDER`].
//! * LIKELY: a cell-based streaming grid is the standard pattern
//!   for open-world games of this era (GTA, Spider-Man 2, Hulk
//!   UD); Prototype's use of separate `cells` and `art` archives
//!   strongly implies a cell-grid streaming model.
//! * UNKNOWN: the cell grid's spatial layout (per-cell coordinates
//!   and radii), the streaming radius distance, the per-archive
//!   cell partition (which cells live in `00cells` vs `01cells`).
//!
//! ## Platform reconciliation — the PC install names archives
//! differently (M17)
//!
//! The 5-archive 360 mount order above is PROVEN for the XBOX 360
//! build (Phase 2F exe strings + Xenia log).  The PC install's OWN
//! `cementfiles.p3d` (338 bytes, parsed by
//! `prototype_boot::cement_files::CementFileList`) names the world
//! archives UNPREFIXED — `cells.rcf`, `art.rcf` — and lists 11
//! archives total (plus `00audio`/`01audio`, `movies`, four `woi`
//! archives and an empty `cached` group).  Both observations are
//! PROVEN for their own platforms; a PC host should mount what the
//! PC `cementfiles.p3d` says.  [`ArchiveKind`] stays 360-flavoured
//! because that is where the mount-ORDER evidence lives.

#![forbid(unsafe_code)]

pub mod members;

pub use members::{MemberLoadSet, MemberRef, MemberStream, MemberStreamConfig};

use prototype_math::Vec3;

// ---------------------------------------------------------------------------
// ArchiveKind — the 5 PROVEN RCF archives
// ---------------------------------------------------------------------------

/// One of the 5 PROVEN RCF archives the engine mounts at boot.
/// Variants are listed in canonical mount order (PROVEN — see
/// `prototypef.exe`'s strings + Xenia reference log; mirrors
/// `prototype_boot::RcfMountOrder`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ArchiveKind {
    /// `game:\00cells.rcf` — mounted first.
    Cells0,
    /// `game:\01cells.rcf` — mounted second.
    Cells1,
    /// `game:\00art.rcf` — mounted third (contains the startup
    /// package + the `unlit` material).
    Art0,
    /// `game:\01art.rcf` — mounted fourth.
    Art1,
    /// `game:\02art.rcf` — mounted fifth.
    Art2,
}

impl ArchiveKind {
    /// All 5 archives in canonical mount order (PROVEN).
    pub const ALL_IN_MOUNT_ORDER: [ArchiveKind; 5] = [
        ArchiveKind::Cells0,
        ArchiveKind::Cells1,
        ArchiveKind::Art0,
        ArchiveKind::Art1,
        ArchiveKind::Art2,
    ];

    /// The 0-based mount index (PROVEN: 0..4).
    pub fn mount_index(self) -> u32 {
        match self {
            ArchiveKind::Cells0 => 0,
            ArchiveKind::Cells1 => 1,
            ArchiveKind::Art0 => 2,
            ArchiveKind::Art1 => 3,
            ArchiveKind::Art2 => 4,
        }
    }

    /// The `game:\`-prefixed mount path (PROVEN — exact match to
    /// the strings in `prototypef.exe`'s `.rdata`).
    pub fn game_path(self) -> &'static str {
        match self {
            ArchiveKind::Cells0 => "game:\\00cells.rcf",
            ArchiveKind::Cells1 => "game:\\01cells.rcf",
            ArchiveKind::Art0 => "game:\\00art.rcf",
            ArchiveKind::Art1 => "game:\\01art.rcf",
            ArchiveKind::Art2 => "game:\\02art.rcf",
        }
    }

    /// The bare archive file name (no `game:\` prefix).
    pub fn file_name(self) -> &'static str {
        match self {
            ArchiveKind::Cells0 => "00cells.rcf",
            ArchiveKind::Cells1 => "01cells.rcf",
            ArchiveKind::Art0 => "00art.rcf",
            ArchiveKind::Art1 => "01art.rcf",
            ArchiveKind::Art2 => "02art.rcf",
        }
    }

    /// Whether this is a `*cells.rcf` archive (the cell-grid
    /// archives, mounted before the `*art.rcf` art archives).
    pub fn is_cells(self) -> bool {
        matches!(self, ArchiveKind::Cells0 | ArchiveKind::Cells1)
    }

    /// Whether this is a `*art.rcf` archive.
    pub fn is_art(self) -> bool {
        matches!(self, ArchiveKind::Art0 | ArchiveKind::Art1 | ArchiveKind::Art2)
    }

    /// Parse from the bare archive file name (e.g. `"00cells.rcf"`).
    pub fn from_file_name(s: &str) -> Option<ArchiveKind> {
        match s {
            "00cells.rcf" => Some(ArchiveKind::Cells0),
            "01cells.rcf" => Some(ArchiveKind::Cells1),
            "00art.rcf" => Some(ArchiveKind::Art0),
            "01art.rcf" => Some(ArchiveKind::Art1),
            "02art.rcf" => Some(ArchiveKind::Art2),
            _ => None,
        }
    }
}

/// The PC install's cement-library map, verbatim from the real
/// `cementfiles.p3d` (PROVEN — byte-decoded 2026-11-03; see
/// `prototype_boot::cement_files::CementFileList` for the parser
/// and the nested group/file layout).
///
/// The `default` group's 11 archives in file order.  Note the
/// platform difference against [`ArchiveKind::ALL_IN_MOUNT_ORDER`]
/// (the 360 set): PC uses unprefixed `cells.rcf`/`art.rcf` and adds
/// audio/movie/woi archives.  `cached` (an empty group in the real
/// file) is NOT an archive and is not listed here.
pub const PC_CEMENT_FILES_DEFAULT: [&str; 11] = [
    "scripts.rcf",
    "00audio.rcf",
    "01audio.rcf",
    "shaders.rcf",
    "art.rcf",
    "cells.rcf",
    "movies.rcf",
    "00woi.rcf",
    "01woi.rcf",
    "02woi.rcf",
    "03woi.rcf",
];

// ---------------------------------------------------------------------------
// CellId + StreamingCell
// ---------------------------------------------------------------------------

/// Typed identifier for one streaming cell (a rectangular region
/// of the world owned by one archive).  For the first version of
/// the streaming grid (one mega-cell per archive), this is just
/// the owning archive kind; future versions will add per-cell
/// coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CellId {
    pub archive: ArchiveKind,
    /// 0-based cell index within the archive.  For the first
    /// version, this is always 0 (one mega-cell per archive);
    /// future versions will use this for sub-cell partitioning.
    pub index: u32,
}

impl CellId {
    pub const fn new(archive: ArchiveKind, index: u32) -> Self {
        Self { archive, index }
    }
}

/// One streaming cell's metadata: id, owning archive, center
/// position, and load radius.  A cell is "loaded" when the player
/// is within `radius` meters of `center`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamingCell {
    pub id: CellId,
    pub center: Vec3,
    /// Load radius in meters.  Cells with the player inside this
    /// radius are in the desired load set.
    pub radius: f32,
}

impl StreamingCell {
    pub fn new(id: CellId, center: Vec3, radius: f32) -> Self {
        Self { id, center, radius }
    }

    /// Is `player` within this cell's load radius?
    pub fn contains(&self, player: Vec3) -> bool {
        (self.center - player).length() <= self.radius
    }
}

// ---------------------------------------------------------------------------
// StreamingGrid
// ---------------------------------------------------------------------------

/// The streaming-grid model: a registry of cells.  Given a player
/// position, returns the set of cells that should be loaded, in
/// canonical archive mount order.
///
/// First-version implementation: one mega-cell per archive, all
/// loaded for any player position (because we don't yet know the
/// per-cell spatial layout — see module docs).  Future versions
/// will partition each archive into multiple cells with distinct
/// AABBs.
#[derive(Debug, Default, Clone)]
pub struct StreamingGrid {
    cells: Vec<StreamingCell>,
}

impl StreamingGrid {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build the canonical first-version grid: one mega-cell per
    /// archive, all at the world origin with effectively infinite
    /// radius (so every player position loads every archive, in
    /// canonical mount order).
    pub fn canonical_full_grid() -> Self {
        let mut g = Self::new();
        for archive in ArchiveKind::ALL_IN_MOUNT_ORDER {
            g.add(StreamingCell::new(
                CellId::new(archive, 0),
                Vec3::ZERO,
                f32::INFINITY,
            ));
        }
        g
    }

    /// Add a cell to the grid.
    pub fn add(&mut self, cell: StreamingCell) {
        self.cells.push(cell);
    }

    /// Build a REAL grid from per-cell spatial specs — the M25b
    /// replacement for the synthetic mega-cells once a real
    /// `WorldIndex` (cells.rcf) has been scanned.
    ///
    /// Each spec becomes one [`StreamingCell`] whose `CellId::index` is
    /// the REAL cell number (`Cell_NN`), center/radius derived from the
    /// cell's placement bbox.  EVERY archive in `always_loaded` also
    /// gets a canonical infinite mega-cell — including the streamed
    /// one, because the real engine streams members INSIDE an archive
    /// (archive mount/unmount stays coarse; the member-level
    /// near-first selection lives in the loader).  Callers that want
    /// archive-level streaming should pass only the truly-base
    /// archives here.
    pub fn from_cell_specs<I>(specs: I, always_loaded: &[ArchiveKind]) -> Self
    where
        I: IntoIterator<Item = (ArchiveKind, u32, prototype_math::Vec3, f32)>,
    {
        let mut g = Self::new();
        for (archive, index, center, radius) in specs {
            g.add(StreamingCell::new(CellId::new(archive, index), center, radius));
        }
        for &a in always_loaded {
            g.add(StreamingCell::new(
                CellId::new(a, 0),
                prototype_math::Vec3::ZERO,
                f32::INFINITY,
            ));
        }
        g
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn cells(&self) -> &[StreamingCell] {
        &self.cells
    }

    /// Compute the desired load set: every cell whose radius
    /// contains `player`, sorted in canonical archive mount order.
    /// The result is the set of cells that should be loaded at
    /// this player position.
    pub fn desired_load_set(&self, player: Vec3) -> Vec<&StreamingCell> {
        let mut hits: Vec<&StreamingCell> = self
            .cells
            .iter()
            .filter(|c| c.contains(player))
            .collect();
        hits.sort_by_key(|c| c.id.archive.mount_index());
        hits
    }

    /// The archives that should be loaded at `player`, in canonical
    /// mount order.  Returns each archive once even if multiple
    /// cells in the same archive are loaded.
    pub fn desired_archives(&self, player: Vec3) -> Vec<ArchiveKind> {
        let mut out: Vec<ArchiveKind> = Vec::new();
        for cell in self.desired_load_set(player) {
            if !out.contains(&cell.id.archive) {
                out.push(cell.id.archive);
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// LoadSet / UnloadSet — the diff the host applies
// ---------------------------------------------------------------------------

/// The diff between the currently-loaded archive set and the
/// desired archive set: which archives to mount (in canonical
/// order) and which to unmount (in reverse canonical order, so
/// dependent cells unmount before the archives they depend on).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LoadSet {
    pub to_mount: Vec<ArchiveKind>,
    pub to_unmount: Vec<ArchiveKind>,
}

impl LoadSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Compute the load/unload diff: archives in `desired` not in
    /// `current` are to_mount; archives in `current` not in
    /// `desired` are to_unmount.
    pub fn diff(current: &[ArchiveKind], desired: &[ArchiveKind]) -> Self {
        let mut to_mount = Vec::new();
        for &a in desired {
            if !current.contains(&a) {
                to_mount.push(a);
            }
        }
        // Sort the mount list in canonical order.
        to_mount.sort_by_key(|a| a.mount_index());

        let mut to_unmount = Vec::new();
        for &a in current {
            if !desired.contains(&a) {
                to_unmount.push(a);
            }
        }
        // Reverse canonical order for unmount: cells before art
        // (cells depend on art's startup package — load art first,
        // unmount cells first).
        to_unmount.sort_by_key(|a| a.mount_index());
        to_unmount.reverse();

        LoadSet { to_mount, to_unmount }
    }

    pub fn is_empty(&self) -> bool {
        self.to_mount.is_empty() && self.to_unmount.is_empty()
    }

    pub fn mount_count(&self) -> usize {
        self.to_mount.len()
    }

    pub fn unmount_count(&self) -> usize {
        self.to_unmount.len()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// from_cell_specs (M25b): real cells drive containment; the
    /// always-loaded archives keep canonical mega-cells; the desired
    /// set stays in canonical mount order.
    #[test]
    fn from_cell_specs_real_cells_and_base() {
        let specs = [
            (ArchiveKind::Cells0, 141u32, Vec3::new(1687.0, 11.0, -1845.0), 30.0f32),
            (ArchiveKind::Cells0, 65, Vec3::new(-244.0, 15.0, -62.0), 25.0),
        ];
        let g = StreamingGrid::from_cell_specs(specs, &ArchiveKind::ALL_IN_MOUNT_ORDER);
        // at cell 141's center: that real cell + every base archive
        let hits = g.desired_load_set(Vec3::new(1687.0, 11.0, -1845.0));
        assert!(hits.iter().any(|c| c.id.index == 141));
        let archives = g.desired_archives(Vec3::new(1687.0, 11.0, -1845.0));
        assert_eq!(archives.len(), ArchiveKind::ALL_IN_MOUNT_ORDER.len());
        let idxs: Vec<u32> = archives.iter().map(|a| a.mount_index()).collect();
        let mut sorted = idxs.clone();
        sorted.sort();
        assert_eq!(idxs, sorted);
        // far away: no real cell hits, base archives remain
        let far = g.desired_load_set(Vec3::new(0.0, 0.0, 0.0));
        assert!(far.iter().all(|c| c.id.index == 0), "far hits {far:?}");
        // cell 65 only when near it
        let hits65 = g.desired_load_set(Vec3::new(-244.0, 15.0, -62.0));
        assert!(hits65.iter().any(|c| c.id.index == 65));
    }

    #[test]
    fn archive_kind_canonical_mount_order_is_proven() {
        // PROVEN: the 5 archives in this exact order (Phase 2F).
        let order: Vec<ArchiveKind> = ArchiveKind::ALL_IN_MOUNT_ORDER.to_vec();
        assert_eq!(order.len(), 5);
        assert_eq!(order[0], ArchiveKind::Cells0);
        assert_eq!(order[1], ArchiveKind::Cells1);
        assert_eq!(order[2], ArchiveKind::Art0);
        assert_eq!(order[3], ArchiveKind::Art1);
        assert_eq!(order[4], ArchiveKind::Art2);
        // Mount indices are 0..4 in order.
        for (i, a) in order.iter().enumerate() {
            assert_eq!(a.mount_index() as usize, i);
        }
    }

    #[test]
    fn archive_kind_game_paths_match_prototypef_exe_strings() {
        // PROVEN — exact match to the strings in the binary's
        // .rdata section (Phase 2F recovery).
        assert_eq!(ArchiveKind::Cells0.game_path(), "game:\\00cells.rcf");
        assert_eq!(ArchiveKind::Cells1.game_path(), "game:\\01cells.rcf");
        assert_eq!(ArchiveKind::Art0.game_path(), "game:\\00art.rcf");
        assert_eq!(ArchiveKind::Art1.game_path(), "game:\\01art.rcf");
        assert_eq!(ArchiveKind::Art2.game_path(), "game:\\02art.rcf");
    }

    #[test]
    fn archive_kind_from_file_name_round_trips() {
        for a in ArchiveKind::ALL_IN_MOUNT_ORDER {
            let parsed = ArchiveKind::from_file_name(a.file_name()).unwrap();
            assert_eq!(parsed, a);
        }
        assert!(ArchiveKind::from_file_name("nonexistent.rcf").is_none());
    }

    #[test]
    fn archive_kind_is_cells_is_art_split() {
        assert!(ArchiveKind::Cells0.is_cells());
        assert!(ArchiveKind::Cells1.is_cells());
        assert!(!ArchiveKind::Art0.is_cells());
        assert!(ArchiveKind::Art0.is_art());
        assert!(ArchiveKind::Art1.is_art());
        assert!(ArchiveKind::Art2.is_art());
        assert!(!ArchiveKind::Cells0.is_art());
    }

    #[test]
    fn canonical_full_grid_loads_every_archive_at_origin() {
        let g = StreamingGrid::canonical_full_grid();
        assert_eq!(g.len(), 5);
        // Player at origin: every cell is loaded (infinite radius).
        let archives = g.desired_archives(Vec3::ZERO);
        assert_eq!(archives, ArchiveKind::ALL_IN_MOUNT_ORDER.to_vec());
    }

    #[test]
    fn canonical_full_grid_loads_every_archive_anywhere() {
        let g = StreamingGrid::canonical_full_grid();
        // Player far from origin: still every cell, because the
        // canonical grid uses infinite radius (UNKNOWN layout).
        let archives = g.desired_archives(Vec3::new(1000.0, 0.0, -500.0));
        assert_eq!(archives.len(), 5);
        // Canonical mount order preserved.
        assert_eq!(archives[0], ArchiveKind::Cells0);
        assert_eq!(archives[4], ArchiveKind::Art2);
    }

    #[test]
    fn custom_grid_filters_by_radius() {
        // Build a custom grid with 3 cells at distinct positions
        // and finite radii.
        let mut g = StreamingGrid::new();
        g.add(StreamingCell::new(
            CellId::new(ArchiveKind::Cells0, 0),
            Vec3::new(0.0, 0.0, 0.0),
            100.0,
        ));
        g.add(StreamingCell::new(
            CellId::new(ArchiveKind::Art0, 0),
            Vec3::new(500.0, 0.0, 0.0),
            100.0,
        ));
        g.add(StreamingCell::new(
            CellId::new(ArchiveKind::Art2, 0),
            Vec3::new(1000.0, 0.0, 0.0),
            100.0,
        ));
        // Player at origin: only Cells0 is loaded.
        let archives = g.desired_archives(Vec3::ZERO);
        assert_eq!(archives, vec![ArchiveKind::Cells0]);
        // Player at (505, 0, 0): Cells0 (distance 505 > 100 — NOT loaded)
        // and Art0 (distance 5 ≤ 100 — loaded).
        let archives = g.desired_archives(Vec3::new(505.0, 0.0, 0.0));
        assert_eq!(archives, vec![ArchiveKind::Art0]);
        // Player at (1000, 0, 0): only Art2.
        let archives = g.desired_archives(Vec3::new(1000.0, 0.0, 0.0));
        assert_eq!(archives, vec![ArchiveKind::Art2]);
        // Player at (250, 0, 0): none (all cells are > 100m away).
        let archives = g.desired_archives(Vec3::new(250.0, 0.0, 0.0));
        assert!(archives.is_empty());
    }

    #[test]
    fn desired_load_set_sorts_by_mount_index() {
        // Build cells in NON-canonical order; the desired_load_set
        // must still return them sorted by archive mount index.
        let mut g = StreamingGrid::new();
        g.add(StreamingCell::new(
            CellId::new(ArchiveKind::Art2, 0),
            Vec3::ZERO,
            f32::INFINITY,
        ));
        g.add(StreamingCell::new(
            CellId::new(ArchiveKind::Cells0, 0),
            Vec3::ZERO,
            f32::INFINITY,
        ));
        g.add(StreamingCell::new(
            CellId::new(ArchiveKind::Art0, 0),
            Vec3::ZERO,
            f32::INFINITY,
        ));
        let load = g.desired_load_set(Vec3::ZERO);
        let archives: Vec<ArchiveKind> = load.iter().map(|c| c.id.archive).collect();
        assert_eq!(
            archives,
            vec![ArchiveKind::Cells0, ArchiveKind::Art0, ArchiveKind::Art2]
        );
    }

    #[test]
    fn load_set_diff_computes_mount_and_unmount() {
        // Start: nothing mounted.  Desired: all 5 archives.
        let desired = ArchiveKind::ALL_IN_MOUNT_ORDER.to_vec();
        let set = LoadSet::diff(&[], &desired);
        assert_eq!(set.mount_count(), 5);
        assert!(set.unmount_count() == 0);
        // Mount order is canonical.
        assert_eq!(set.to_mount, desired);

        // Now: all 5 mounted.  Desired: only Cells0 + Art0.
        let current = ArchiveKind::ALL_IN_MOUNT_ORDER.to_vec();
        let desired = vec![ArchiveKind::Cells0, ArchiveKind::Art0];
        let set = LoadSet::diff(&current, &desired);
        assert!(set.mount_count() == 0);
        // Unmount: Cells1, Art1, Art2 in REVERSE canonical order
        // (Art2 first, Cells1 last — cells depend on art).
        assert_eq!(set.to_unmount, vec![ArchiveKind::Art2, ArchiveKind::Art1, ArchiveKind::Cells1]);
    }

    #[test]
    fn load_set_no_op_when_unchanged() {
        let current = ArchiveKind::ALL_IN_MOUNT_ORDER.to_vec();
        let desired = current.clone();
        let set = LoadSet::diff(&current, &desired);
        assert!(set.is_empty());
    }

    #[test]
    fn load_set_mounts_in_canonical_order_regardless_of_desired_order() {
        // Desired given in reverse order — diff must still mount
        // in canonical order.
        let desired = vec![
            ArchiveKind::Art2,
            ArchiveKind::Art1,
            ArchiveKind::Art0,
            ArchiveKind::Cells1,
            ArchiveKind::Cells0,
        ];
        let set = LoadSet::diff(&[], &desired);
        assert_eq!(
            set.to_mount,
            vec![
                ArchiveKind::Cells0,
                ArchiveKind::Cells1,
                ArchiveKind::Art0,
                ArchiveKind::Art1,
                ArchiveKind::Art2,
            ]
        );
    }

    #[test]
    fn pc_cement_map_is_the_unprefixed_world_archive_set() {
        // The PC map (cementfiles.p3d) vs the 360 mount order: both
        // PROVEN for their platform, deliberately different here.
        assert_eq!(PC_CEMENT_FILES_DEFAULT.len(), 11);
        assert!(PC_CEMENT_FILES_DEFAULT.contains(&"cells.rcf"));
        assert!(PC_CEMENT_FILES_DEFAULT.contains(&"art.rcf"));
        assert!(PC_CEMENT_FILES_DEFAULT.contains(&"scripts.rcf"));
        assert!(PC_CEMENT_FILES_DEFAULT.contains(&"shaders.rcf"));
        // No 360-prefixed names in the PC list.
        assert!(!PC_CEMENT_FILES_DEFAULT
            .iter()
            .any(|f| f.starts_with("00cells")
                || f.starts_with("01cells")
                || f.starts_with("00art")
                || f.starts_with("01art")
                || f.starts_with("02art")));
        // And the 360 set keeps its five prefixed names.
        let xbox: Vec<&str> =
            ArchiveKind::ALL_IN_MOUNT_ORDER.iter().map(|a| a.file_name()).collect();
        assert_eq!(
            xbox,
            ["00cells.rcf", "01cells.rcf", "00art.rcf", "01art.rcf", "02art.rcf"]
        );
    }

    #[test]
    fn streaming_player_walks_through_a_custom_grid() {
        // Simulate a player walking along the +X axis through 3
        // cells.  At each position, compute the desired archive
        // set and the diff from the previous frame's set.
        let mut g = StreamingGrid::new();
        g.add(StreamingCell::new(
            CellId::new(ArchiveKind::Cells0, 0),
            Vec3::new(0.0, 0.0, 0.0),
            50.0,
        ));
        g.add(StreamingCell::new(
            CellId::new(ArchiveKind::Cells1, 0),
            Vec3::new(100.0, 0.0, 0.0),
            50.0,
        ));
        g.add(StreamingCell::new(
            CellId::new(ArchiveKind::Art0, 0),
            Vec3::new(200.0, 0.0, 0.0),
            50.0,
        ));
        // Frame 1: player at (0, 0, 0).  Desired: [Cells0].
        let mut current = g.desired_archives(Vec3::ZERO);
        assert_eq!(current, vec![ArchiveKind::Cells0]);

        // Frame 2: player moves to (100, 0, 0).  Desired: [Cells1].
        // Diff: mount Cells1, unmount Cells0.
        let desired = g.desired_archives(Vec3::new(100.0, 0.0, 0.0));
        assert_eq!(desired, vec![ArchiveKind::Cells1]);
        let set = LoadSet::diff(&current, &desired);
        assert_eq!(set.to_mount, vec![ArchiveKind::Cells1]);
        assert_eq!(set.to_unmount, vec![ArchiveKind::Cells0]);
        current = desired;

        // Frame 3: player moves to (200, 0, 0).  Desired: [Art0].
        let desired = g.desired_archives(Vec3::new(200.0, 0.0, 0.0));
        assert_eq!(desired, vec![ArchiveKind::Art0]);
        let set = LoadSet::diff(&current, &desired);
        assert_eq!(set.to_mount, vec![ArchiveKind::Art0]);
        assert_eq!(set.to_unmount, vec![ArchiveKind::Cells1]);
    }
}

//! Member-level streaming (M25c, block 43).
//! ============================================================
//!
//! [`super::StreamingGrid`] decides which **archives** to mount;
//! the real engine keeps a streamed archive mounted and loads/unloads
//! **members inside it** around the player (PROVEN by the archive
//! model itself: `cells.rcf` holds the whole streamed world — 520
//! members spanning ~145 cells — in ONE archive, so member-level
//! selection is where the per-frame streaming actually happens).
//!
//! This module is the member-level half: a bounded, deterministic
//! state machine over the member table produced from a scanned
//! [`prototype_p3d::world::WorldIndex`].
//!
//! ## The model
//!
//! * Input: one [`MemberRef`] per known member — its directory hash
//!   and its distance from the focal point (the player/camera).
//!   Distance comes from the member's placement bbox (the
//!   [`WorldIndexEntry::distance_from`] formula); members without a
//!   bbox (the 224/520 real empty members, or art-only members) are
//!   passed as [`f32::INFINITY`].
//! * Desired set: the `budget` nearest finite-distance members.
//!   Infinite-distance members NEVER auto-load (fail-closed: a
//!   member with no known placement has no streaming evidence).
//! * Load order: near-first (ascending distance, ties broken by
//!   hash — fully deterministic).
//! * Unload: loaded members beyond the **hysteresis** radius
//!   (`worst_desired_distance × hysteresis`), farthest-first.
//!   Hysteresis is the anti-thrash slack: a member that just fell
//!   out of the desired set is only evicted once it is hysteresis
//!   times farther than the current load radius.  Loaded members
//!   may therefore transiently exceed `budget` (documented DESIGN —
//!   bounded by the hysteresis geometry, and preferable to
//!   load/unload churn at cell boundaries).
//! * Unknown hashes: a loaded hash that is absent from the passed
//!   table is evicted (fail-closed — no evidence to keep it).
//! * Settlement: applying a returned [`MemberLoadSet`] through
//!   [`MemberStream::apply`] and then planning again with the SAME
//!   input yields the empty set (no churn) — the load/unload diff
//!   is idempotent per focal point.
//!
//! ## Confidence labels
//!
//! * PROVEN (data): member kinds, bboxes, cells and the 520-member
//!   census of the real `cells.rcf` — blocks 18/41 (`cells.rcf`
//!   corpus: 147 object / 149 geometry / 224 empty members, 0
//!   multi-cell members).
//! * DESIGN (this module): budget/hysteresis policy, near-first
//!   load and farthest-first unload ordering, fail-closed
//!   infinite-distance handling.  The engine's exact member
//!   eviction policy is not recovered from the binary — these are
//!   the standard streaming-system choices, chosen deterministic
//!   and tested for settlement.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;

// ---------------------------------------------------------------------------
// MemberRef — one member's stream-relevant view
// ---------------------------------------------------------------------------

/// One known member, as the streaming state machine sees it: its RCF
/// directory hash and its distance from the focal point.
///
/// `distance` uses the placement-bbox center distance (the
/// `prototype_p3d::world::WorldIndexEntry::distance_from` formula —
/// the engine keeps an identical copy via
/// `prototype_p3d::world::bbox_center_distance` so the two can never
/// drift).  [`f32::INFINITY`] = no placement evidence (empty or
/// art-only member): never auto-loaded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemberRef {
    /// RCF directory hash of the member.
    pub hash: u32,
    /// Distance from the focal point (>= 0 and finite = streamable).
    pub distance: f32,
}

impl MemberRef {
    /// A streamable member reference.
    pub fn new(hash: u32, distance: f32) -> Self {
        Self { hash, distance }
    }

    /// Fail-closed streamability gate: finite AND non-negative.
    /// (NaN and negative distances are caller errors — treated as
    /// "no evidence", i.e. infinite.)
    pub fn streamable(&self) -> bool {
        self.distance.is_finite() && self.distance >= 0.0
    }
}

// ---------------------------------------------------------------------------
// MemberStreamConfig
// ---------------------------------------------------------------------------

/// Policy knobs of [`MemberStream`].  Construction is fail-closed:
/// `budget >= 1` and `hysteresis >= 1.0` (a hysteresis below 1.0
/// would evict members inside the desired set — guaranteed churn).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemberStreamConfig {
    /// Maximum members the desired set holds (the nearest-N rule).
    pub budget: usize,
    /// Eviction slack multiplier: a loaded member is kept while
    /// `distance <= worst_desired_distance * hysteresis`.
    pub hysteresis: f32,
}

impl MemberStreamConfig {
    /// The block-43 default policy (DESIGN, not evidence): a 64-member
    /// working set and 50% eviction slack.  The real corpus needs at
    /// most ~3 members per cell (object + geometry + textures) and a
    /// camera-ordered neighbourhood spans a handful of cells, so 64
    /// comfortably covers a dense neighbourhood without holding the
    /// whole 296-member non-empty corpus.
    pub const DEFAULT: Self = Self {
        budget: 64,
        hysteresis: 1.5,
    };

    /// Validated construction (fail-closed).
    pub fn new(budget: usize, hysteresis: f32) -> Result<Self, String> {
        if budget == 0 {
            return Err("member-stream budget must be >= 1".to_string());
        }
        if !hysteresis.is_finite() || hysteresis < 1.0 {
            return Err(format!(
                "member-stream hysteresis must be finite and >= 1.0 (got {hysteresis})"
            ));
        }
        Ok(Self { budget, hysteresis })
    }
}

impl Default for MemberStreamConfig {
    fn default() -> Self {
        Self::DEFAULT
    }
}

// ---------------------------------------------------------------------------
// MemberLoadSet — the per-update diff
// ---------------------------------------------------------------------------

/// The streaming diff for one update: which members to load (NEAR
/// first) and which to unload (FAR first).  Mirrors the archive-level
/// [`super::LoadSet`] contract: the host performs the physical work,
/// the state machine applies bookkeeping via [`MemberStream::apply`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemberLoadSet {
    /// Member hashes to load, nearest first.
    pub load: Vec<u32>,
    /// Member hashes to unload, farthest first (unknown hashes last).
    pub unload: Vec<u32>,
}

impl MemberLoadSet {
    /// True when nothing has to change.
    pub fn is_empty(&self) -> bool {
        self.load.is_empty() && self.unload.is_empty()
    }
}

// ---------------------------------------------------------------------------
// MemberStream — the state machine
// ---------------------------------------------------------------------------

/// Bounded member-level streaming state: which members are logically
/// loaded, and the deterministic diff that moves the loaded set
/// toward the desired set as the focal point moves.
///
/// The engine counterpart lives on `EngineRuntime`
/// (`attach_member_stream` / `update_member_streaming` /
/// `apply_member_load_set`); this type is engine-free and tested
/// standalone.
#[derive(Debug, Clone)]
pub struct MemberStream {
    config: MemberStreamConfig,
    /// Logically-loaded member hashes (bookkeeping only — the
    /// physical member load lives in the host loader).
    loaded: BTreeSet<u32>,
}

impl MemberStream {
    /// Validated construction (fail-closed via
    /// [`MemberStreamConfig::new`]).
    pub fn new(config: MemberStreamConfig) -> Result<Self, String> {
        // MemberStreamConfig::new validates; this just re-routes the
        // error so callers of MemberStream::new fail closed too.
        let cfg = MemberStreamConfig::new(config.budget, config.hysteresis)?;
        Ok(Self {
            config: cfg,
            loaded: BTreeSet::new(),
        })
    }

    /// The active policy.
    pub fn config(&self) -> &MemberStreamConfig {
        &self.config
    }

    /// Number of logically-loaded members.
    pub fn loaded_count(&self) -> usize {
        self.loaded.len()
    }

    /// Is this hash logically loaded?
    pub fn is_loaded(&self, hash: u32) -> bool {
        self.loaded.contains(&hash)
    }

    /// The loaded set (read-only).
    pub fn loaded(&self) -> &BTreeSet<u32> {
        &self.loaded
    }

    /// Compute the diff for `members` WITHOUT touching state.
    ///
    /// Ordering guarantees:
    /// * `load` — ascending distance (near first), ties by hash.
    /// * `unload` — descending distance (far first); hashes absent
    ///   from the table sort after all known ones (alphabetical, for
    ///   determinism).
    pub fn plan(&self, members: &[MemberRef]) -> MemberLoadSet {
        // 1. Deterministic candidate order: streamable first, then
        //    ascending distance (bit order — distances are >= 0, so
        //    IEEE bits order equals numeric order), ties by hash.
        let mut order: Vec<usize> = (0..members.len()).collect();
        order.sort_by(|&a, &b| {
            let (sa, sb) = (members[a].streamable(), members[b].streamable());
            sb.cmp(&sa)
                .then_with(|| members[a].distance.to_bits().cmp(&members[b].distance.to_bits()))
                .then_with(|| members[a].hash.cmp(&members[b].hash))
        });

        // 2. Desired set = nearest `budget` streamable members
        //    (deduped by hash — first occurrence wins, which the sort
        //    makes the nearest one).
        let mut desired: Vec<(u32, f32)> = Vec::with_capacity(self.config.budget);
        let mut taken: BTreeSet<u32> = BTreeSet::new();
        for &i in &order {
            if !members[i].streamable() {
                break; // sorted: everything after is unstreamable
            }
            let h = members[i].hash;
            if taken.insert(h) {
                desired.push((h, members[i].distance));
                if desired.len() == self.config.budget {
                    break;
                }
            }
        }
        // The load radius = distance of the FARTHEST desired member.
        let worst = desired.last().map(|&(_, d)| d).unwrap_or(0.0);
        let keep_radius = worst * self.config.hysteresis;

        // 3. Unload: loaded members outside the keep radius
        //    (farthest first); unknown hashes (not in `members`) or
        //    unstreamable ones evict too, sorted last.
        let known: std::collections::HashMap<u32, f32> = members
            .iter()
            .filter(|m| m.streamable())
            .map(|m| (m.hash, m.distance))
            .collect();
        let mut unload_known: Vec<(u32, f32)> = Vec::new();
        let mut unload_unknown: Vec<u32> = Vec::new();
        for &h in &self.loaded {
            match known.get(&h) {
                Some(&d) if d <= keep_radius => {} // kept (hysteresis slack)
                Some(&d) => unload_known.push((h, d)),
                None => unload_unknown.push(h),
            }
        }
        // Known evictions: descending distance = farthest first.
        // (Bits-compare on the negated distance keeps a total order
        // without floats-in-cmp pitfalls: distances are >= 0, so
        // (-d) is <= 0 and IEEE bit order matches numeric order for
        // the non-positive range too.)
        unload_known.sort_by(|a, b| {
            (-b.1).to_bits().cmp(&(-a.1).to_bits()).then_with(|| a.0.cmp(&b.0))
        });

        // 4. Load: desired members not already loaded, near first
        //    (`desired` is already in that order).
        let load: Vec<u32> = desired
            .iter()
            .map(|&(h, _)| h)
            .filter(|h| !self.loaded.contains(h))
            .collect();

        MemberLoadSet {
            load,
            unload: unload_known
                .into_iter()
                .map(|(h, _)| h)
                .chain(unload_unknown)
                .collect(),
        }
    }

    /// Apply a diff to the bookkeeping state (mirrors the host
    /// applying it physically).  Unloads first, then loads.
    pub fn apply(&mut self, set: &MemberLoadSet) {
        for &h in &set.unload {
            self.loaded.remove(&h);
        }
        for &h in &set.load {
            self.loaded.insert(h);
        }
    }

    /// Convenience: [`MemberStream::plan`] + [`MemberStream::apply`]
    /// in one call (the engine's per-frame path uses this after the
    /// host-audit needs are satisfied by the returned set).
    pub fn update(&mut self, members: &[MemberRef]) -> MemberLoadSet {
        let set = self.plan(members);
        self.apply(&set);
        set
    }
}

// ---------------------------------------------------------------------------
// Tests — policy semantics (synthetic tables); real-fixture slices
// live in prototype-engine and prototype-bin integration tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(pairs: &[(u32, f32)]) -> Vec<MemberRef> {
        pairs.iter().map(|&(h, d)| MemberRef::new(h, d)).collect()
    }

    /// A validated stream (tests use valid configs only).
    fn stream(budget: usize, hysteresis: f32) -> MemberStream {
        MemberStream::new(MemberStreamConfig::new(budget, hysteresis).unwrap()).unwrap()
    }

    #[test]
    fn config_is_fail_closed() {
        assert!(MemberStreamConfig::new(0, 1.5).is_err(), "budget 0");
        assert!(MemberStreamConfig::new(4, 0.999).is_err(), "hysteresis < 1");
        assert!(MemberStreamConfig::new(4, f32::NAN).is_err(), "NaN hysteresis");
        assert!(MemberStreamConfig::new(4, f32::INFINITY).is_err(), "inf hysteresis");
        // The stream constructor re-validates too (no `.unwrap()`
        // panic — route the config error through the stream ctor).
        assert!(MemberStreamConfig::new(0, 1.5).and_then(MemberStream::new).is_err());
        let ok = MemberStreamConfig::new(1, 1.0).unwrap();
        assert_eq!(ok.budget, 1);
        assert_eq!(ok.hysteresis, 1.0);
        assert_eq!(MemberStreamConfig::default(), MemberStreamConfig::DEFAULT);
        // The stream constructor re-validates too.
        assert!(stream(1, 1.0).config().budget == 1);
    }

    #[test]
    fn loads_nearest_first_up_to_budget() {
        let mut s = stream(2, 1.5);
        let m = refs(&[(10, 30.0), (11, 5.0), (12, 20.0), (13, 1.0)]);
        let set = s.update(&m);
        // nearest two: 13 (1 m) then 11 (5 m)
        assert_eq!(set.load, vec![13, 11]);
        assert!(set.unload.is_empty());
        assert_eq!(s.loaded_count(), 2);
        assert!(s.is_loaded(13) && s.is_loaded(11) && !s.is_loaded(10));
    }

    #[test]
    fn infinite_distance_never_loads() {
        let mut s = stream(8, 1.5);
        // 90/91 are the real corpus shape: empty members have no bbox.
        let m = refs(&[(7, 100.0), (90, f32::INFINITY), (91, f32::INFINITY)]);
        let set = s.update(&m);
        assert_eq!(set.load, vec![7], "only the finite member loads");
        assert!(!s.is_loaded(90) && !s.is_loaded(91));
    }

    #[test]
    fn negative_and_nan_distances_are_fail_closed() {
        let mut s = stream(8, 1.5);
        let m = refs(&[(7, 100.0), (42, -1.0), (43, f32::NAN)]);
        let set = s.update(&m);
        assert_eq!(set.load, vec![7]);
        assert!(!s.is_loaded(42) && !s.is_loaded(43));
    }

    #[test]
    fn distance_ties_break_by_hash_deterministically() {
        let mut s = stream(2, 1.0);
        let m = refs(&[(30, 10.0), (10, 10.0), (20, 10.0)]);
        let set = s.update(&m);
        assert_eq!(set.load, vec![10, 20], "equal distances → hash order");
    }

    #[test]
    fn duplicate_hash_keeps_nearest_occurrence() {
        let mut s = stream(1, 1.0);
        let m = refs(&[(9, 50.0), (9, 4.0)]);
        let set = s.update(&m);
        assert_eq!(set.load, vec![9]);
        assert_eq!(s.loaded_count(), 1, "deduped to one logical member");
    }

    #[test]
    fn walking_away_unloads_farthest_first_and_never_below_one_load() {
        let mut s = stream(2, 1.5);
        // Three clusters 1 km apart; budget 2.
        let at_a = refs(&[(1, 1.0), (2, 2.0), (3, 1_000.0)]);
        let set = s.update(&at_a);
        assert_eq!(set.load, vec![1, 2]);
        // Move to cluster B: 1 and 2 are now ~1 km away, 3 is near.
        let at_b = refs(&[(1, 1_000.0), (2, 1_001.0), (3, 3.0), (4, 4.0)]);
        let set = s.update(&at_b);
        // desired = {3, 4}; keep radius = 4 * 1.5 = 6 m → 1 and 2 evict.
        assert_eq!(set.load, vec![3, 4]);
        // farthest first: 2 (1001 m) before 1 (1000 m)
        assert_eq!(set.unload, vec![2, 1]);
        assert_eq!(s.loaded_count(), 2);
    }

    #[test]
    fn hysteresis_keeps_boundary_members_no_thrash() {
        let mut s = stream(1, 2.0);
        // Load member 2 first (it is the nearest at that focal point).
        s.update(&refs(&[(1, 100.0), (2, 2.0)]));
        assert!(s.is_loaded(2) && !s.is_loaded(1));
        // Move: 1 becomes desired (10 m), 2 just outside (11 m) but
        // inside the keep radius (10 * 2.0 = 20 m) → 2 is KEPT even
        // though the budget-1 desired set is {1}: no churn for 2.
        let set = s.update(&refs(&[(1, 10.0), (2, 11.0)]));
        assert_eq!(set.load, vec![1], "the newly-desired member loads");
        assert!(set.unload.is_empty(), "hysteresis keeps the boundary member: {set:?}");
        // Transiently 2 > budget 1 — the documented anti-thrash slack.
        assert!(s.is_loaded(1) && s.is_loaded(2));
    }

    #[test]
    fn beyond_hysteresis_radius_evicts() {
        let mut s = stream(1, 2.0);
        // Load member 2 first.
        s.update(&refs(&[(1, 100.0), (2, 2.0)]));
        // Move so 1 is desired at 10 m and 2 sits at 25 m > 10*2.0.
        let set = s.update(&refs(&[(1, 10.0), (2, 25.0)]));
        assert_eq!(set.load, vec![1]);
        assert_eq!(set.unload, vec![2]);
        assert!(!s.is_loaded(2) && s.is_loaded(1));
    }

    #[test]
    fn unknown_loaded_hash_is_evicted_fail_closed() {
        let mut s = stream(4, 1.5);
        s.update(&refs(&[(1, 1.0), (2, 2.0)]));
        assert!(s.is_loaded(1) && s.is_loaded(2));
        // The table "forgot" member 2 (e.g. re-scan without it).
        let set = s.update(&refs(&[(1, 1.0)]));
        assert_eq!(set.unload, vec![2], "unknown loaded hash evicts");
        assert!(s.is_loaded(1) && !s.is_loaded(2));
    }

    #[test]
    fn plan_then_apply_settles_to_no_churn() {
        let mut s = stream(3, 1.5);
        let m = refs(&[(5, 9.0), (3, 1.0), (8, 100.0), (1, 2.0), (90, f32::INFINITY)]);
        let first = s.update(&m);
        assert_eq!(first.load, vec![3, 1, 5]);
        let again = s.plan(&m);
        assert!(again.is_empty(), "settled immediately: {again:?}");
        // And through the explicit apply path too.
        let mut s2 = stream(3, 1.5);
        let set = s2.plan(&m);
        s2.apply(&set);
        assert!(s2.plan(&m).is_empty());
        assert_eq!(s2.loaded(), s.loaded());
    }

    #[test]
    fn budget_larger_than_corpus_loads_every_finite_member() {
        let mut s = stream(64, 1.5);
        let m = refs(&[(1, 30.0), (2, 20.0), (3, 10.0)]);
        let set = s.update(&m);
        assert_eq!(set.load, vec![3, 2, 1]);
        assert_eq!(s.loaded_count(), 3);
    }

    #[test]
    fn desired_members_are_never_unloaded_when_hysteresis_is_one() {
        // Property check at the hysteresis floor: keep radius ==
        // load radius, so every desired member is kept by
        // construction (distance <= worst).
        let mut s = stream(3, 1.0);
        let m = refs(&[(1, 1.0), (2, 2.0), (3, 3.0), (4, 4.0)]);
        let set = s.update(&m);
        assert_eq!(set.load, vec![1, 2, 3]);
        assert!(set.unload.is_empty(), "no desired member evicts at h=1: {set:?}");
        let set2 = s.update(&m);
        assert!(set2.is_empty());
    }

    #[test]
    fn empty_table_unloads_everything() {
        let mut s = stream(4, 1.5);
        s.update(&refs(&[(1, 1.0), (2, 2.0)]));
        let set = s.update(&[]);
        assert_eq!(set.load, Vec::<u32>::new());
        assert_eq!(set.unload.len(), 2, "both members evict (unknown)");
        assert_eq!(s.loaded_count(), 0);
    }
}

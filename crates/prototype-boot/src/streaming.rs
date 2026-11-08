//! streaming — M12: the bridge between the world-streaming model
//! ([`prototype_stream::StreamingGrid`] / [`prototype_stream::LoadSet`])
//! and the content pipeline ([`crate::ContentPipeline`]'s ordered RCF
//! mounts).
//!
//! ## The gap this closes
//!
//! M11e gave [`prototype_engine::EngineRuntime`] a streaming grid:
//! `update_streaming(player)` returns a [`prototype_stream::LoadSet`]
//! (which archives to mount, which to unmount) and the runtime
//! tracks its *virtual* loaded set.  But the diff was purely
//! bookkeeping — nothing actually mounted or unmounted archive
//! bytes, so streamed-out assets stayed resolvable and streamed-in
//! ones had no bytes behind them.
//!
//! This module supplies the host-side IO layer:
//!
//! * [`ArchiveSources`] — maps each [`ArchiveKind`] to the raw RCF
//!   bytes behind it.  In a real host these are mmap'd
//!   `game:\*.rcf` files; in tests they are fixture bytes.  This
//!   crate never reads the filesystem itself (no IO in the model).
//! * [`StreamingMounts`] — the live mount bookkeeping: which
//!   [`ArchiveKind`]s are currently mounted, the mount name each
//!   one was registered under, and an append-only [`MountEvent`]
//!   audit trail.  [`StreamingMounts::apply`] executes a
//!   [`prototype_stream::LoadSet`] against a
//!   [`crate::ContentPipeline`]: unmounts first (reverse canonical
//!   order), then mounts (canonical order).
//!
//! ## The host loop this module documents
//!
//! ```text
//! each frame:
//!   let set = runtime.update_streaming(player.position());   // diff
//!   mounts.apply(&mut pipeline, &set, &sources)?;            // REAL IO
//!   runtime.apply_load_set(&set);                            // bookkeeping
//! ```
//!
//! The integration test below drives exactly this loop through a
//! real `frevents.lua` + real RCF fixture bytes, walking a player
//! across a custom grid and asserting that assets appear and
//! disappear from the pipeline as the diff is applied.
//!
//! ## PROVEN vs LIKELY vs UNKNOWN
//!
//! * PROVEN: the 5 archive names + canonical mount order
//!   (`prototype-stream::ArchiveKind`, Phase 2F); that RCF archives
//!   parse and serve hash-named files through
//!   [`crate::ContentPipeline`] (Phase 2C); `LoadSet::diff`'s
//!   canonical mount / reverse-canonical unmount ordering is OUR
//!   documented policy consistent with the engine mounting cells
//!   before art.
//! * LIKELY: that the original engine unmounts world archives when
//!   the player leaves their streaming cells (the standard
//!   cell-streaming pattern of Radical's earlier engines — Hulk
//!   UD, Spider-Man 2 — and the existence of separate cells/art
//!   archives implies exactly this lifecycle).
//! * UNKNOWN: the original engine's per-cell spatial layout, its
//!   streaming radius, and whether it remounts or pins any archive
//!   (e.g. `00art.rcf`'s startup package) for the whole session.
//!   The mount names used here are the PROVEN archive file names.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use prototype_stream::ArchiveKind;

use crate::ContentPipeline;

// ---------------------------------------------------------------------------
// ArchiveSources — ArchiveKind → raw RCF bytes
// ---------------------------------------------------------------------------

/// Maps each [`ArchiveKind`] to the raw RCF archive bytes that back
/// it.  The streaming mounter's source of truth for "what bytes do
/// I hand to [`crate::ContentPipeline::mount_rcf`] when this archive
/// streams in".
///
/// In the shipping engine these bytes come from the filesystem
/// (`game:\00cells.rcf` …).  This model deliberately does NO IO: the
/// host provides the bytes (mmap'd in a real host; fixture bytes in
/// tests), and this crate only routes them.
#[derive(Debug, Default, Clone)]
pub struct ArchiveSources<'a> {
    map: HashMap<ArchiveKind, &'a [u8]>,
}

impl<'a> ArchiveSources<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder-style registration of one archive's bytes.  Overwrites
    /// a previous registration for the same kind.
    pub fn insert(mut self, kind: ArchiveKind, data: &'a [u8]) -> Self {
        self.map.insert(kind, data);
        self
    }

    /// The bytes behind `kind`, if registered.
    pub fn get(&self, kind: ArchiveKind) -> Option<&'a [u8]> {
        self.map.get(&kind).copied()
    }

    /// Is `kind` registered?
    pub fn contains(&self, kind: ArchiveKind) -> bool {
        self.map.contains_key(&kind)
    }

    /// How many archives are registered.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// The registered kinds, in canonical mount order.
    pub fn kinds_in_mount_order(&self) -> Vec<ArchiveKind> {
        ArchiveKind::ALL_IN_MOUNT_ORDER
            .iter()
            .copied()
            .filter(|k| self.map.contains_key(k))
            .collect()
    }

    /// Which of `kinds` are NOT registered (used by
    /// [`StreamingMounts::apply`] to pre-validate before mutating a
    /// pipeline).
    pub fn missing(&self, kinds: &[ArchiveKind]) -> Vec<ArchiveKind> {
        kinds
            .iter()
            .copied()
            .filter(|k| !self.map.contains_key(k))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// MountEvent — the audit trail
// ---------------------------------------------------------------------------

/// What happened to one archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountAction {
    /// Mounted into the pipeline (canonical order position).
    Mounted,
    /// Unmounted from the pipeline; its files no longer resolve.
    Unmounted,
}

/// One mount/unmount event, recorded by [`StreamingMounts::apply`].
///
/// `mount_index` snapshots the pipeline mount position AT EVENT TIME
/// (`Some` for [`MountAction::Mounted`], `None` for unmounts).  It is
/// historical only: [`crate::ContentPipeline::unmount_rcf`] shifts
/// later archives' positions, so a stored index is valid only for
/// the pipeline state that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEvent {
    pub archive: ArchiveKind,
    pub action: MountAction,
    /// The mount name used in the pipeline (the archive's PROVEN
    /// file name, e.g. `"00cells.rcf"`).
    pub mount_name: String,
    pub mount_index: Option<usize>,
}

// ---------------------------------------------------------------------------
// StreamingMounts — the live mount bookkeeping
// ---------------------------------------------------------------------------

/// The live streaming mount state: which archives are currently
/// mounted (canonical order), the mount name each was registered
/// under, and the append-only audit trail of every mount/unmount
/// this bookkeeping performed.
///
/// The engine-side twin is
/// [`prototype_engine::EngineRuntime::loaded_archives`] (virtual
/// bookkeeping only); this struct is the side that actually touches
/// a [`crate::ContentPipeline`].  The integration test asserts the
/// two stay in lock-step through a walked session.
#[derive(Debug, Default, Clone)]
pub struct StreamingMounts {
    loaded: Vec<ArchiveKind>,
    mount_names: HashMap<ArchiveKind, String>,
    audit: Vec<MountEvent>,
}

impl StreamingMounts {
    pub fn new() -> Self {
        Self::default()
    }

    /// Boot-time full mount: mount EVERY registered source archive,
    /// in canonical order — the Phase-2C/2F boot behaviour (the
    /// engine mounts all five RCFs up front; world streaming then
    /// manages them from there).
    ///
    /// Equivalent to `apply` with an empty current set and the full
    /// canonical desired set.  Errors (before mutating anything) if
    /// a registered archive fails to parse.
    pub fn mount_all<'a>(
        &mut self,
        pipeline: &mut ContentPipeline<'a>,
        sources: &ArchiveSources<'a>,
    ) -> Result<(), String> {
        let kinds = sources.kinds_in_mount_order();
        self.apply(pipeline, &prototype_stream::LoadSet {
            to_mount: kinds,
            to_unmount: Vec::new(),
        }, sources)
    }

    /// The currently-mounted archives, in canonical mount order.
    pub fn loaded(&self) -> &[ArchiveKind] {
        &self.loaded
    }

    /// Is `kind` currently mounted (per this bookkeeping)?
    pub fn is_loaded(&self, kind: ArchiveKind) -> bool {
        self.loaded.contains(&kind)
    }

    /// The mount name `kind` was registered under (present iff
    /// loaded).
    pub fn mount_name(&self, kind: ArchiveKind) -> Option<&str> {
        self.mount_names.get(&kind).map(String::as_str)
    }

    /// The full audit trail (mounts + unmounts, in execution order).
    pub fn audit(&self) -> &[MountEvent] {
        &self.audit
    }

    /// Number of recorded events.
    pub fn audit_len(&self) -> usize {
        self.audit.len()
    }

    /// Execute a [`prototype_stream::LoadSet`] against `pipeline`:
    ///
    /// 1. **Pre-validate** — every to_mount archive must have a
    ///    registered source, and no to_mount archive may collide
    ///    with a pipeline mount name registered OUTSIDE this
    ///    bookkeeping.  On any violation the pipeline is left
    ///    untouched (`Err` before mutation).
    /// 2. **Unmount** — `to_unmount` in REVERSE canonical order
    ///    (re-sorted here so hand-built load sets get the same
    ///    guarantee `LoadSet::diff` provides: cells unmount before
    ///    the art archives they were mounted after).
    /// 3. **Mount** — `to_mount` in canonical order, registering
    ///    each archive under its PROVEN file name.  If an archive
    ///    fails to parse at mount time, every archive THIS call
    ///    mounted is rolled back (unmounted in reverse order, its
    ///    audit events removed) before the `Err` returns — the
    ///    unmounts of step 2 have already committed by then (they
    ///    cannot fail), so a failed `apply` may be retried.
    ///
    /// Idempotent-safe: unmounting an archive that isn't loaded and
    /// mounting one that already is, are both no-ops.  No-ops
    /// produce NO events — the audit trail records only state
    /// CHANGES.
    pub fn apply<'a>(
        &mut self,
        pipeline: &mut ContentPipeline<'a>,
        set: &prototype_stream::LoadSet,
        sources: &ArchiveSources<'a>,
    ) -> Result<(), String> {
        // ---- 1. pre-validate (no mutation before this passes) ----
        let missing = sources.missing(&set.to_mount);
        if !missing.is_empty() {
            let names: Vec<&str> = missing.iter().map(|k| k.file_name()).collect();
            return Err(format!(
                "streaming mount: no source bytes registered for: {}",
                names.join(", ")
            ));
        }
        for &k in &set.to_mount {
            let name = k.file_name();
            if pipeline.is_mounted(name) && !self.is_loaded(k) {
                return Err(format!(
                    "streaming mount: {name} is already mounted outside the \
                     streaming bookkeeping — refusing to double-mount"
                ));
            }
        }

        // ---- 2. unmount (reverse canonical order) ----
        let mut to_unmount = set.to_unmount.clone();
        to_unmount.sort_by_key(|k| std::cmp::Reverse(k.mount_index()));
        for k in to_unmount {
            if !self.is_loaded(k) {
                continue; // not ours / already gone — no-op, no event
            }
            let name = self
                .mount_names
                .get(&k)
                .cloned()
                .unwrap_or_else(|| k.file_name().to_string());
            if pipeline.unmount_rcf(&name) {
                self.loaded.retain(|x| *x != k);
                self.mount_names.remove(&k);
                self.audit.push(MountEvent {
                    archive: k,
                    action: MountAction::Unmounted,
                    mount_name: name,
                    mount_index: None,
                });
            }
        }

        // ---- 3. mount (canonical order, with rollback) ----
        let mut to_mount = set.to_mount.clone();
        to_mount.sort_by_key(|k| k.mount_index());
        let pipeline_count_before = pipeline.mount_count();
        let mut mounted_this_call: Vec<ArchiveKind> = Vec::new();
        for k in to_mount {
            if self.is_loaded(k) {
                continue; // already mounted — no-op, no event
            }
            let name = k.file_name();
            let data = sources.get(k).expect("pre-validated above");
            match pipeline.mount_rcf(name, data) {
                Ok(idx) => {
                    self.loaded.push(k);
                    self.mount_names.insert(k, name.to_string());
                    mounted_this_call.push(k);
                    self.audit.push(MountEvent {
                        archive: k,
                        action: MountAction::Mounted,
                        mount_name: name.to_string(),
                        mount_index: Some(idx),
                    });
                }
                Err(e) => {
                    // Roll back every archive THIS call mounted, in
                    // reverse mount order, and drop their audit
                    // events (they were the last N pushed).
                    for rk in mounted_this_call.iter().rev() {
                        let rname = self
                            .mount_names
                            .get(rk)
                            .cloned()
                            .unwrap_or_default();
                        let _ = pipeline.unmount_rcf(&rname);
                        self.loaded.retain(|x| x != rk);
                        self.mount_names.remove(rk);
                        self.audit.pop();
                    }
                    debug_assert_eq!(pipeline.mount_count(), pipeline_count_before);
                    return Err(format!("streaming mount: {}: {e}", name));
                }
            }
        }

        // Keep the loaded list in canonical order after mounts.
        self.loaded.sort_by_key(|k| k.mount_index());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests — fixture-driven (real RCF bytes from prototype-bin fixtures)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use prototype_stream::{CellId, LoadSet, StreamingCell, StreamingGrid};
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

    /// Stand-in sources: the two real RCF fixtures stand in for
    /// world archives whose real bytes are not in the repo.  The
    /// MECHANISM under test (mount/unmount/resolve) is independent
    /// of which archive content plays which ArchiveKind role —
    /// documented honestly here rather than disguised.
    fn two_archive_sources<'a>(
        scripts: &'a [u8],
        shaders: &'a [u8],
    ) -> ArchiveSources<'a> {
        ArchiveSources::new()
            .insert(ArchiveKind::Cells0, scripts)
            .insert(ArchiveKind::Art0, shaders)
    }

    // ----- ArchiveSources ------------------------------------------------

    #[test]
    fn archive_sources_builder_and_mount_order() {
        let scripts = fixture("scripts.rcf");
        let shaders = fixture("shaders.rcf");
        let src = two_archive_sources(&scripts, &shaders);
        assert_eq!(src.len(), 2);
        assert_eq!(
            src.kinds_in_mount_order(),
            vec![ArchiveKind::Cells0, ArchiveKind::Art0]
        );
        assert!(src.contains(ArchiveKind::Cells0));
        assert!(!src.contains(ArchiveKind::Art2));
        assert_eq!(src.get(ArchiveKind::Art0), Some(shaders.as_slice()));
        // missing() reports exactly the unregistered kinds.
        let missing = src.missing(&ArchiveKind::ALL_IN_MOUNT_ORDER);
        assert_eq!(
            missing,
            vec![
                ArchiveKind::Cells1,
                ArchiveKind::Art1,
                ArchiveKind::Art2
            ]
        );
    }

    // ----- mount_all (boot behaviour) ------------------------------------

    #[test]
    fn mount_all_mounts_every_source_in_canonical_order() {
        let scripts = fixture("scripts.rcf");
        let shaders = fixture("shaders.rcf");
        let src = two_archive_sources(&scripts, &shaders);
        let mut pipeline = ContentPipeline::new();
        let mut mounts = StreamingMounts::new();
        mounts.mount_all(&mut pipeline, &src).expect("mount_all");
        assert_eq!(
            mounts.loaded(),
            vec![ArchiveKind::Cells0, ArchiveKind::Art0]
        );
        assert_eq!(pipeline.mount_count(), 2);
        assert_eq!(
            pipeline.mounted_rcf_names(),
            vec!["00cells.rcf", "00art.rcf"]
        );
        // Audit: two mounts, with positional indices 0 then 1.
        assert_eq!(mounts.audit_len(), 2);
        assert_eq!(mounts.audit()[0].action, MountAction::Mounted);
        assert_eq!(mounts.audit()[0].archive, ArchiveKind::Cells0);
        assert_eq!(mounts.audit()[0].mount_index, Some(0));
        assert_eq!(mounts.audit()[1].archive, ArchiveKind::Art0);
        assert_eq!(mounts.audit()[1].mount_index, Some(1));
        // An archive mounted by mount_all resolves under its PROVEN name.
        assert!(pipeline.is_mounted("00cells.rcf"));
    }

    #[test]
    fn mount_all_rejects_before_mutation_when_a_source_fails_to_parse() {
        let scripts = fixture("scripts.rcf");
        // A source registered with INVALID RCF bytes.
        let bad = b"this is definitely not a valid rcf archive......";
        let src = ArchiveSources::new()
            .insert(ArchiveKind::Cells0, &scripts)
            .insert(ArchiveKind::Art0, bad.as_slice());
        let mut pipeline = ContentPipeline::new();
        let mut mounts = StreamingMounts::new();
        let err = mounts
            .mount_all(&mut pipeline, &src)
            .expect_err("bad archive must fail");
        assert!(err.contains("00art.rcf"), "error names the archive: {err}");
        // No partial state: nothing mounted, nothing audited.
        assert_eq!(pipeline.mount_count(), 0);
        assert!(mounts.loaded().is_empty());
        assert_eq!(mounts.audit_len(), 0);
    }

    // ----- apply: unmount + mount semantics -------------------------------

    #[test]
    fn apply_unmounts_in_reverse_canonical_order_and_audits() {
        let scripts = fixture("scripts.rcf");
        let shaders = fixture("shaders.rcf");
        let src = two_archive_sources(&scripts, &shaders);
        let mut pipeline = ContentPipeline::new();
        let mut mounts = StreamingMounts::new();
        mounts.mount_all(&mut pipeline, &src).unwrap();

        // Everything streams out.
        let set = LoadSet {
            to_mount: vec![],
            to_unmount: vec![ArchiveKind::Cells0, ArchiveKind::Art0],
        };
        mounts.apply(&mut pipeline, &set, &src).unwrap();
        assert!(mounts.loaded().is_empty());
        assert_eq!(pipeline.mount_count(), 0);
        // Reverse canonical order: Art0 (index 2) unmounts before
        // Cells0 (index 0), regardless of the input list order.
        assert_eq!(mounts.audit().len(), 4); // 2 mounts + 2 unmounts
        assert_eq!(mounts.audit()[2].action, MountAction::Unmounted);
        assert_eq!(mounts.audit()[2].archive, ArchiveKind::Art0);
        assert_eq!(mounts.audit()[2].mount_index, None);
        assert_eq!(mounts.audit()[3].action, MountAction::Unmounted);
        assert_eq!(mounts.audit()[3].archive, ArchiveKind::Cells0);
    }

    #[test]
    fn apply_is_idempotent_no_ops_produce_no_events() {
        let scripts = fixture("scripts.rcf");
        let src = ArchiveSources::new().insert(ArchiveKind::Cells0, &scripts);
        let mut pipeline = ContentPipeline::new();
        let mut mounts = StreamingMounts::new();
        mounts.mount_all(&mut pipeline, &src).unwrap();
        let events_after_boot = mounts.audit_len();

        // Mounting an already-mounted archive: no-op, no event.
        let set = LoadSet {
            to_mount: vec![ArchiveKind::Cells0],
            to_unmount: vec![],
        };
        mounts.apply(&mut pipeline, &set, &src).unwrap();
        assert_eq!(mounts.audit_len(), events_after_boot);

        // Unmounting a never-mounted archive: no-op, no event.
        let set = LoadSet {
            to_mount: vec![],
            to_unmount: vec![ArchiveKind::Art2],
        };
        mounts.apply(&mut pipeline, &set, &src).unwrap();
        assert_eq!(mounts.audit_len(), events_after_boot);
        assert!(mounts.is_loaded(ArchiveKind::Cells0));
    }

    #[test]
    fn apply_prevalidates_missing_sources_before_any_mutation() {
        let scripts = fixture("scripts.rcf");
        let src = ArchiveSources::new().insert(ArchiveKind::Cells0, &scripts);
        let mut pipeline = ContentPipeline::new();
        let mut mounts = StreamingMounts::new();
        mounts.mount_all(&mut pipeline, &src).unwrap();
        let count_before = pipeline.mount_count();

        // Ask to mount Art2 (unregistered) — must fail BEFORE the
        // unmount of Cells0 in the same set is executed.
        let set = LoadSet {
            to_mount: vec![ArchiveKind::Art2],
            to_unmount: vec![ArchiveKind::Cells0],
        };
        let err = mounts.apply(&mut pipeline, &set, &src).unwrap_err();
        assert!(err.contains("02art.rcf"), "{err}");
        assert_eq!(pipeline.mount_count(), count_before);
        assert!(mounts.is_loaded(ArchiveKind::Cells0));
    }

    #[test]
    fn apply_rejects_double_mounting_a_name_registered_outside_bookkeeping() {
        let scripts = fixture("scripts.rcf");
        let src = ArchiveSources::new().insert(ArchiveKind::Cells0, &scripts);
        let mut pipeline = ContentPipeline::new();
        // Mount 00cells.rcf DIRECTLY on the pipeline (outside the
        // streaming bookkeeping).
        pipeline.mount_rcf("00cells.rcf", &scripts).unwrap();

        let mut mounts = StreamingMounts::new();
        let set = LoadSet {
            to_mount: vec![ArchiveKind::Cells0],
            to_unmount: vec![],
        };
        let err = mounts.apply(&mut pipeline, &set, &src).unwrap_err();
        assert!(
            err.contains("outside the streaming bookkeeping"),
            "{err}"
        );
    }

    // ----- the archive actually streams: resolution follows mounts ----

    #[test]
    fn streamed_out_assets_stop_resolving_streamed_in_assets_resolve() {
        // scripts.rcf's bytes serve `scripts\frevents.lua`; mount its
        // bytes as the Cells0 source and prove that resolution follows
        // the mount lifecycle.
        let scripts = fixture("scripts.rcf");
        let src = ArchiveSources::new().insert(ArchiveKind::Cells0, &scripts);
        let mut pipeline = ContentPipeline::new();
        let mut mounts = StreamingMounts::new();

        // Not mounted yet: the path does NOT resolve.
        assert!(pipeline.load(r"scripts\frevents.lua").is_err());

        // Stream in: now it resolves, sourced from the streamed archive.
        let set = LoadSet {
            to_mount: vec![ArchiveKind::Cells0],
            to_unmount: vec![],
        };
        mounts.apply(&mut pipeline, &set, &src).unwrap();
        let a = pipeline.load(r"scripts\frevents.lua").expect("resolves");
        assert!(matches!(a.source, crate::AssetSource::Rcf { mount_index: 0 }));

        // Stream out: resolution is gone again.
        let set = LoadSet {
            to_mount: vec![],
            to_unmount: vec![ArchiveKind::Cells0],
        };
        mounts.apply(&mut pipeline, &set, &src).unwrap();
        assert!(pipeline.load(r"scripts\frevents.lua").is_err());
    }

    #[test]
    fn mount_index_snapshots_shift_after_unmount() {
        // Documented ContentPipeline semantics: mount indices are
        // positional and shift after an unmount.  The audit trail
        // snapshots them at event time.
        //
        // Setup: Cells0 + Art0 both carry the scripts.rcf bytes
        // (same content, two roles), Cells1 carries shaders.rcf.
        let scripts = fixture("scripts.rcf");
        let shaders = fixture("shaders.rcf");
        let src = ArchiveSources::new()
            .insert(ArchiveKind::Cells0, &scripts)
            .insert(ArchiveKind::Cells1, &shaders)
            .insert(ArchiveKind::Art0, &scripts);
        let mut pipeline = ContentPipeline::new();
        let mut mounts = StreamingMounts::new();
        mounts.mount_all(&mut pipeline, &src).unwrap();
        assert_eq!(
            pipeline.mounted_rcf_names(),
            vec!["00cells.rcf", "01cells.rcf", "00art.rcf"]
        );

        // A scripts-served path resolves at position 0 (Cells0).
        let a = pipeline.load(r"scripts\frevents.lua").unwrap();
        assert_eq!(a.source, crate::AssetSource::Rcf { mount_index: 0 });

        // Unmount Cells0: everything after it shifts down one slot.
        let set = LoadSet {
            to_mount: vec![],
            to_unmount: vec![ArchiveKind::Cells0],
        };
        mounts.apply(&mut pipeline, &set, &src).unwrap();
        assert_eq!(
            pipeline.mounted_rcf_names(),
            vec!["01cells.rcf", "00art.rcf"]
        );
        // The same path STILL resolves (Art0 carries the same bytes)
        // — but now at the shifted position 1.
        let a = pipeline.load(r"scripts\frevents.lua").unwrap();
        assert_eq!(a.source, crate::AssetSource::Rcf { mount_index: 1 });
        // The unmount event snapshots no index; the earlier mount
        // events keep their event-time indices (0 for Cells0).
        let unmount = mounts.audit().iter().rev().find(|e| e.action == MountAction::Unmounted).unwrap();
        assert_eq!(unmount.archive, ArchiveKind::Cells0);
        assert_eq!(unmount.mount_index, None);
        assert_eq!(mounts.audit()[0].mount_index, Some(0));
        assert_eq!(mounts.audit()[2].mount_index, Some(2)); // Art0 booted at slot 2
    }

    #[test]
    fn remount_reuses_the_proven_name_and_gets_the_freed_slot() {
        // After Cells0 unmounts, remounting it takes the FIRST freed
        // pipeline position — the engine re-mounts streaming archives
        // the same way (append to the archive list).
        let scripts = fixture("scripts.rcf");
        let shaders = fixture("shaders.rcf");
        let src = two_archive_sources(&scripts, &shaders);
        let mut pipeline = ContentPipeline::new();
        let mut mounts = StreamingMounts::new();
        mounts.mount_all(&mut pipeline, &src).unwrap();

        let out = LoadSet {
            to_mount: vec![],
            to_unmount: vec![ArchiveKind::Cells0],
        };
        mounts.apply(&mut pipeline, &out, &src).unwrap();
        assert_eq!(pipeline.mounted_rcf_names(), vec!["00art.rcf"]);

        let back = LoadSet {
            to_mount: vec![ArchiveKind::Cells0],
            to_unmount: vec![],
        };
        mounts.apply(&mut pipeline, &back, &src).unwrap();
        // Remount appended at position 1 (after Art0).
        assert_eq!(
            pipeline.mounted_rcf_names(),
            vec!["00art.rcf", "00cells.rcf"]
        );
        // …but the BOOKKEEPING list stays canonical.
        assert_eq!(
            mounts.loaded(),
            vec![ArchiveKind::Cells0, ArchiveKind::Art0]
        );
        // The remount event snapshot: index 1 (event-time position).
        let last = mounts.audit().last().unwrap();
        assert_eq!(last.action, MountAction::Mounted);
        assert_eq!(last.archive, ArchiveKind::Cells0);
        assert_eq!(last.mount_index, Some(1));
    }

    // ----- end-to-end: EngineRuntime drives the real mounts -------------

    #[test]
    fn engine_runtime_walk_drives_real_mount_unmount_cycles() {
        use prototype_engine::player::Player;
        use prototype_engine::EngineRuntime;
        use prototype_input::{InputEvent, KeyCode};

        let scripts_rcf = fixture("scripts.rcf");
        let shaders_rcf = fixture("shaders.rcf");
        let frevents = fixture("frevents.lua");

        // The scripted runtime (parses + runs the real frevents.lua).
        let lua = prototype_lua::Lua51File::parse(&frevents).expect("parse frevents");
        let mut rt = EngineRuntime::new(&lua).expect("runtime");

        // A custom grid: Cells0 near the origin, Art0 1000 m east.
        // (The real per-cell layout is UNKNOWN — this exercises the
        // DIFF mechanics the real layout will drive.)
        let mut grid = StreamingGrid::new();
        grid.add(StreamingCell::new(
            CellId::new(ArchiveKind::Cells0, 0),
            prototype_math::Vec3::new(0.0, 0.0, 0.0),
            100.0,
        ));
        grid.add(StreamingCell::new(
            CellId::new(ArchiveKind::Art0, 0),
            prototype_math::Vec3::new(1000.0, 0.0, 0.0),
            100.0,
        ));
        rt.set_streaming_grid(grid);

        // Content: the two real RCF fixtures stand in for the two
        // archive kinds (mechanism test — see two_archive_sources).
        let src = ArchiveSources::new()
            .insert(ArchiveKind::Cells0, &scripts_rcf)
            .insert(ArchiveKind::Art0, &shaders_rcf);
        let mut pipeline = ContentPipeline::new();
        let mut mounts = StreamingMounts::new();

        // Frame 1: player at origin → Cells0 streams in.
        let mut player = Player::at(prototype_math::Vec3::ZERO);
        rt.apply_input(InputEvent::KeyDown(KeyCode::D)); // walk east
        let (_, set, _) = rt
            .gameplay_tick(&mut player, 0.016, 6.0)
            .expect("tick 1");
        mounts.apply(&mut pipeline, &set, &src).expect("apply 1");
        rt.apply_load_set(&set);
        assert_eq!(mounts.loaded(), &[ArchiveKind::Cells0]);
        assert_eq!(
            rt.loaded_archives(),
            mounts.loaded(),
            "runtime bookkeeping == real mounts"
        );
        // A Cells0-served path resolves NOW.
        assert!(pipeline.load(r"scripts\frevents.lua").is_ok());

        // Frames 2..N: walk east at 6 m/s (0.6 m per 0.1 s frame).
        // Cells0 streams OUT once the player leaves its 100 m radius
        // (x > 100); Art0 streams IN when the player enters Art0's
        // radius (x >= 900).  At 0.6 m/frame that is ~frame 170 and
        // ~frame 1500 — the walk runs until BOTH transitions are
        // observed (hard cap 4000 frames as a runaway guard).
        let mut art0_mounted_at_frame = None;
        let mut cells0_unmounted_at_frame = None;
        for frame in 2..=4000u64 {
            let (_, set, _) = rt
                .gameplay_tick(&mut player, 0.1, 6.0)
                .expect("tick");
            mounts.apply(&mut pipeline, &set, &src)
                .unwrap_or_else(|e| panic!("apply at frame {frame}: {e}"));
            rt.apply_load_set(&set);
            assert_eq!(
                rt.loaded_archives(),
                mounts.loaded(),
                "bookkeeping divergence at frame {frame}"
            );
            if cells0_unmounted_at_frame.is_none()
                && !mounts.is_loaded(ArchiveKind::Cells0)
            {
                cells0_unmounted_at_frame = Some(frame);
                // Cells0 streamed OUT → its files no longer resolve.
                assert!(
                    pipeline.load(r"scripts\frevents.lua").is_err(),
                    "streamed-out archive must stop resolving (frame {frame})"
                );
            }
            if art0_mounted_at_frame.is_none()
                && mounts.is_loaded(ArchiveKind::Art0)
            {
                art0_mounted_at_frame = Some(frame);
                // Art0 streamed IN — a Cells0-role path still does
                // NOT resolve through it (different bytes), but the
                // Art0-role archive (shaders.rcf bytes) is mounted
                // under its PROVEN name.
                assert!(pipeline.is_mounted("00art.rcf"));
            }
            // Early exit once both transitions observed.
            if art0_mounted_at_frame.is_some() && cells0_unmounted_at_frame.is_some() {
                break;
            }
        }
        assert!(
            art0_mounted_at_frame.is_some(),
            "Art0 never streamed in across the walk"
        );
        assert!(
            cells0_unmounted_at_frame.is_some(),
            "Cells0 never streamed out"
        );
        // Leaving Cells0's radius (x > 100) happens long before
        // entering Art0's (x >= 900): the unmount MUST land on an
        // earlier frame than the mount.
        assert!(
            cells0_unmounted_at_frame.unwrap() < art0_mounted_at_frame.unwrap(),
            "Cells0 out @ {:?}, Art0 in @ {:?}",
            cells0_unmounted_at_frame,
            art0_mounted_at_frame
        );

        // The audit trail recorded exactly: Cells0 in, Cells0 out,
        // Art0 in — nothing else (no-op frames produce no events).
        let actions: Vec<(ArchiveKind, MountAction)> = mounts
            .audit()
            .iter()
            .map(|e| (e.archive, e.action))
            .collect();
        assert_eq!(
            actions,
            vec![
                (ArchiveKind::Cells0, MountAction::Mounted),
                (ArchiveKind::Cells0, MountAction::Unmounted),
                (ArchiveKind::Art0, MountAction::Mounted),
            ]
        );

        // Player actually walked east past the 900 m boundary
        // (1000 - 100 radius).
        assert!(player.position().x > 900.0, "x = {}", player.position().x);
    }
}

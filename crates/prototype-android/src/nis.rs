//! nis — the gameplay state that owns the active NIS cutscene and
//! turns it into audio + movie requests (block 69).
//!
//! ## What this reconstructs
//!
//! The engine's real NIS trigger code is UNKNOWN (no mission-script
//! interpreter exists in this rewrite — the loader renders the world
//! and plays the real audio; it does not simulate the game's
//! objectives).  What IS PROVEN is the DATA side: `boot_strap.p3d`
//! carries 46 `NIS_Player` patches — the NIS AUDIO event family
//! (`mb_3_1_1` … briefings, `mnis4_3_1` … mission NISes,
//! `nis0-3a` … scene ids) — each with exactly three properties:
//! `volume` (Float32, 1.0 across the corpus), `filename` (the mix
//! clip reference, e.g. `mnis_5_2_1_mix`), and `mixbuss` (a string
//! list, `[nis, master, dialog]`) — blocks 56/66/67.
//!
//! The MOVIE half is PROVEN ABSENT from every shipped plain-text
//! source: block 56 mined cells.rcf (all 520 members), art.rcf (all
//! 2601 members) and boot_strap itself — zero movie-name strings for
//! the NIS family; block 67's dictionary attack (46 raw names ×
//! prefixes × extensions) found nothing; and block 69's STRUCTURAL
//! attack (7 497 696 candidates: underscore insertion, `mnis`→
//! `NIS_`/`MNIS_` rewrites, separator swaps, the observed corpus
//! suffix vocabulary, three-component briefing grids, `.p3d` bibles)
//! closed the space — ZERO hits on the six unnamed movies.rcf members
//! and no object→movie rule of any kind.  The engine's own API shape
//! agrees: `nis_SetFMVFileName` is a SETTER — the movie name arrives
//! from mission data OUTSIDE the patch, by hash (block 56's recorded
//! conclusion).
//!
//! This module is OUR documented model of the missing gameplay half,
//! mirroring [`crate::mission::MissionSystem`]: a small state machine
//! that OWNS the active cutscene, validates the object against the
//! decoded [`MusicGraph`] (fail-closed — an unknown object or a
//! non-`NIS_Player` patch is a clean `Err`, never a guess), carries
//! the movie name EXACTLY as it was fed in (`set_movie` — the
//! `nis_SetFMVFileName` counterpart; never inferred from the object
//! name), and exposes ONE kind of output: a [`NisRequest`] describing
//! what the app's live seams must serve now.  The app
//! (prototype-android::app) consumes it through the tagged voice
//! lifecycle and the PROVEN `find_movie_by_name` resolver; nothing
//! here touches audio devices or GPUs.
//!
//! ## Model
//!
//! * [`NisSystem`] — the active cutscene object (an exact
//!   `NIS_Player` patch key) + the externally-fed movie name + a
//!   monotonic request counter.  Every STATE CHANGE bumps
//!   [`NisSystem::request_seq`]; the app serves the request and marks
//!   it consumed ([`NisSystem::pending`]).  Mutators are IDEMPOTENT
//!   (re-starting the same cutscene → no bump → no restart).
//! * [`NisRequest`] — one cutscene to serve: the patch's PROVEN mix
//!   descriptor (ordered clip names, `volume`, `mixbuss`) + the
//!   movie name as fed.  `clips == []` is a TRACKED SILENT cutscene
//!   (the defensive honest-miss contract: a graph that cannot
//!   resolve the active patch serves silence, the state stays —
//!   every REAL corpus patch references exactly one mix; 43 of the
//!   46 mixes ship in the block-63 shard index, 3 do not — the
//!   `nis0-3a/4/5_mix.wav` intro mixes — and those degrade at the
//!   shard boundary, never here); `movie == None` is a tracked
//!   video-less cutscene (the honest default — the feed is external
//!   and our mission data does not exist).
//!
//! ## Confidence labels
//!
//! * PROVEN: the 46-patch NIS_Player census and every patch's
//!   `volume`/`filename`/`mixbuss` (blocks 56/66/67, re-verified here
//!   against the committed fixture); the mix clip names' resolution
//!   through the shard index (block 63/64).
//! * DESIGN (documented): the movie name is an EXTERNAL feed
//!   (`set_movie`), never inferred — the block-69 structural attack
//!   is the evidence; starting a cutscene does not require a movie;
//!   the movie name is resolved against the user's own archives by
//!   the app (`find_movie_by_name`, PROVEN) and a miss degrades to
//!   audio-only, logged.
//! * UNKNOWN: what the real engine's mission code feeds and when —
//!   this module exposes the seam; the trigger system (mission
//!   scripts) does not exist here, and mid-gameplay movie PLAYBACK
//!   (the render-loop integration) lands with the NIS player task.

#![forbid(unsafe_code)]

use crate::music_graph::MusicGraph;

/// What the app's live NIS seams must serve RIGHT NOW: one cutscene's
/// PROVEN mix descriptor (the `NIS_Player` patch's own properties)
/// plus the externally-fed movie name, carried through exactly as
/// set — never inferred, never guessed.
#[derive(Debug, Clone, PartialEq)]
pub struct NisRequest {
    /// Diagnostics label: `nis/mnis5_2_1`.
    pub label: String,
    /// The `NIS_Player` patch object (the exact graph key, e.g.
    /// `mnis5_2_1`, `mb_3_1_1`, `nis0-3a`).
    pub object: String,
    /// The patch's `filename` mix clip references, shipped order.
    /// EMPTY = the corpus's event-driven patches (no mix file): a
    /// tracked silent cutscene, never an error.
    pub clips: Vec<String>,
    /// The patch's `volume` Float32 (PROVEN prop; 1.0 when absent).
    /// The app multiplies its caller gain by this.
    pub volume: f32,
    /// The patch's `mixbuss` StrList (e.g. `[nis, master, dialog]`).
    /// Carried for diagnostics; our mixer has one gameplay bus (the
    /// honest simplification, block 67).
    pub mixbuss: Vec<String>,
    /// The externally-fed movie name (`nis_SetFMVFileName`'s
    /// counterpart), exactly as set — `None` until the caller feeds
    /// one.  NEVER inferred from the object name (the block-69
    /// structural attack PROVED no such rule exists).
    pub movie: Option<String>,
}

/// The NIS state machine: owns the active cutscene + the external
/// movie feed, validates against the decoded graph, and produces
/// [`NisRequest`]s.  Device-free; fully deterministic; every mutation
/// is idempotent (same state → no bump → the app restarts nothing).
#[derive(Debug, Clone, Default)]
pub struct NisSystem {
    /// The active cutscene's `NIS_Player` patch object (exact key).
    active: Option<String>,
    /// The externally-fed movie name (`nis_SetFMVFileName`'s
    /// counterpart).  Survives a stop?  NO — a stop ends the
    /// cutscene unit; the next cutscene needs its own feed (the
    /// engine sets the FMV per cutscene).  Cleared by [`Self::stop`].
    movie: Option<String>,
    /// Bumped on every STATE CHANGE; the app serves the request and
    /// marks it consumed.  Monotonic within a session.
    request_seq: u64,
    /// The [`Self::request_seq`] value the app last consumed.
    served_seq: u64,
}

impl NisSystem {
    /// The initial state: idle with NO movie feed and NO pending
    /// request — boot itself starts no cutscene.
    pub fn new() -> Self {
        Self::default()
    }

    /// The active cutscene's patch object, if any.
    pub fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    /// The externally-fed movie name (cleared by [`Self::stop`]).
    pub fn movie(&self) -> Option<&str> {
        self.movie.as_deref()
    }

    /// Monotonic request counter (diagnostics).
    pub fn request_seq(&self) -> u64 {
        self.request_seq
    }

    /// The counter value the app last served (diagnostics).
    pub fn served_seq(&self) -> u64 {
        self.served_seq
    }

    /// True when the state changed since the app last served it.
    pub fn pending(&self) -> bool {
        self.request_seq > self.served_seq
    }

    /// The app's receipt: mark the current request consumed.
    pub fn mark_served(&mut self) {
        self.served_seq = self.request_seq;
    }

    /// Start an NIS cutscene by its `NIS_Player` patch object.  The
    /// object must decode as a patch of class EXACTLY `NIS_Player`
    /// in the graph (fail-closed — the 46-object census is the
    /// vocabulary; a music stem, a frontend bed or an unknown name
    /// is a clean `Err`, state untouched).
    ///
    /// Idempotent: starting the SAME object while it is already
    /// active → `Ok(false)`, no bump (the app restarts nothing).  A
    /// different object (or starting from idle) → `Ok(true)`, the
    /// counter bumps.
    pub fn start(&mut self, graph: &MusicGraph, object: &str) -> Result<bool, String> {
        let patch = graph.patch(object).ok_or_else(|| {
            format!("nis: no patch named {object:?} in the graph (the NIS vocabulary is boot_strap.p3d's 46 NIS_Player objects)")
        })?;
        if patch.class != "NIS_Player" {
            return Err(format!(
                "nis: {object:?} is a {} patch, not an NIS_Player — refusing (fail-closed)",
                patch.class
            ));
        }
        if self.active.as_deref() == Some(object) {
            return Ok(false);
        }
        self.active = Some(object.to_string());
        self.request_seq = self.request_seq.wrapping_add(1);
        Ok(true)
    }

    /// The external movie feed — the engine's own `nis_SetFMVFileName`
    /// shape: the movie name arrives from mission data OUTSIDE the
    /// patch (PROVEN: the patch carries only `volume`/`filename`/
    /// `mixbuss`, and the block-69 structural attack found no
    /// object→movie rule).  Setting the SAME name → `false`, no bump;
    /// a change → `true`, the counter bumps (the app re-serves: the
    /// movie leg resolves the new name).
    ///
    /// The feed may be set while idle (the engine sets the FMV before
    /// the cutscene triggers) — it is stored and rides into the next
    /// [`Self::request`].
    pub fn set_movie(&mut self, movie: Option<&str>) -> bool {
        let movie = movie.map(str::to_string);
        if self.movie == movie {
            return false;
        }
        self.movie = movie;
        self.request_seq = self.request_seq.wrapping_add(1);
        true
    }

    /// Stop the cutscene.  Idempotent: `false` when already idle.
    /// The movie feed clears with it (a new cutscene needs its own
    /// feed — the engine sets the FMV per cutscene).
    pub fn stop(&mut self) -> bool {
        if self.active.is_none() && self.movie.is_none() {
            return false;
        }
        self.active = None;
        self.movie = None;
        self.request_seq = self.request_seq.wrapping_add(1);
        true
    }

    /// The request the app must serve NOW: `None` when idle.  The mix
    /// descriptor comes from the graph (PROVEN props); `movie` rides
    /// through as fed.  A graph that cannot resolve the active patch
    /// serves TRACKED SILENCE (`clips == []`, state stays) — the
    /// defensive honest-miss contract; every REAL corpus patch
    /// references exactly one mix.
    pub fn request(&self, graph: &MusicGraph) -> Option<NisRequest> {
        let object = self.active.as_deref()?;
        let mix = graph.nis_mix(object);
        Some(NisRequest {
            label: format!("nis/{object}"),
            object: object.to_string(),
            clips: mix.as_ref().map(|m| m.clips.clone()).unwrap_or_default(),
            volume: mix.as_ref().map(|m| m.volume).unwrap_or(1.0),
            mixbuss: mix.as_ref().map(|m| m.buses.clone()).unwrap_or_default(),
            movie: self.movie.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Tests — REAL fixtures: the committed boot_strap.p3d (the user's own
// audio data, decoded by the byte-exact parser).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music_graph::MusicGraph;

    fn fixture_bytes(name: &str) -> Vec<u8> {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    /// A graph merged from the REAL boot_strap.p3d — the NIS
    /// vocabulary itself (46 NIS_Player patches + the music banks).
    fn boot_graph() -> MusicGraph {
        let mut g = MusicGraph::empty();
        let bytes = fixture_bytes("boot_strap.p3d");
        g.merge_bank_bytes(&bytes, "audio\\boot_strap.p3d")
            .unwrap_or_else(|e| panic!("boot_strap must decode: {e}"));
        g
    }

    /// THE CENSUS (PROVEN, re-verified through this module's own
    /// validation): boot_strap carries exactly 46 NIS_Player patches,
    /// and every one of them passes `NisSystem::start` — the module's
    /// vocabulary IS the real corpus, byte-for-byte.
    #[test]
    fn every_real_nis_player_patch_passes_start() {
        let g = boot_graph();
        let nis_objects: Vec<&str> = g
            .patches()
            .filter(|(_, p)| p.class == "NIS_Player")
            .map(|(name, _)| name)
            .collect();
        assert_eq!(nis_objects.len(), 46, "the block-56/66 census");
        let mut sys = NisSystem::new();
        for object in &nis_objects {
            assert!(
                sys.start(&g, object).unwrap_or_else(|e| panic!("{object}: {e}")),
                "{object}: first start must bump"
            );
            sys.stop();
        }
    }

    /// Start is FAIL-CLOSED: unknown objects and non-NIS_Player
    /// patches are clean Errs with the state untouched; the SAME
    /// object twice is idempotent (no bump); a different object
    /// bumps; stop clears and is idempotent.
    #[test]
    fn start_is_fail_closed_idempotent_and_stopped_cleanly() {
        let g = boot_graph();
        let mut sys = NisSystem::new();
        assert!(sys.active().is_none());
        assert!(!sys.pending());

        // Unknown object: clean Err, state untouched.
        let err = sys.start(&g, "mnis9_9_9").unwrap_err();
        assert!(err.contains("no patch"), "{err}");
        let err = sys.start(&g, "e10m04_ambient").unwrap_err();
        assert!(err.contains("no patch"), "{err}");
        assert!(sys.active().is_none());
        assert_eq!(sys.request_seq(), 0);

        // A NON-NIS patch (the frontend bed, class music_player):
        // refused — the trigger only fires cutscenes.
        let err = sys.start(&g, "fe_music").unwrap_err();
        assert!(err.contains("music_player"), "{err}");
        assert!(err.contains("not an NIS_Player"), "{err}");
        assert!(sys.active().is_none());

        // First start: bump.  Same object: idempotent, no bump.
        assert!(sys.start(&g, "mnis5_2_1").unwrap());
        assert_eq!(sys.active(), Some("mnis5_2_1"));
        assert!(sys.pending());
        sys.mark_served();
        let seq = sys.request_seq();
        assert!(!sys.start(&g, "mnis5_2_1").unwrap(), "same object: no bump");
        assert_eq!(sys.request_seq(), seq);
        assert!(!sys.pending());

        // A different object IS a change: bump.
        assert!(sys.start(&g, "mb_3_1_1").unwrap());
        assert_eq!(sys.request_seq(), seq + 1);
        assert_eq!(sys.active(), Some("mb_3_1_1"));

        // Stop: clears, bumps once, idempotent, clears the feed too.
        sys.set_movie(Some("NIS_3_1_FMV.bik"));
        let seq = sys.request_seq();
        assert!(sys.stop());
        assert_eq!(sys.request_seq(), seq + 1);
        assert!(sys.active().is_none());
        assert!(sys.movie().is_none());
        assert!(!sys.stop(), "stop twice is a no-op");
        assert_eq!(sys.request_seq(), seq + 1);
    }

    /// The request carries the PROVEN mix descriptor of the REAL
    /// patch (volume/filename/mixbuss), and the movie rides through
    /// EXACTLY as fed — never inferred, even for an object whose
    /// scene id corresponds to a NAMED movies.rcf member (`nis2-4` ↔
    /// `NIS_2_4_FMV` is a corpus correspondence, NOT a rule: the
    /// block-69 structural attack PROVED no derivation exists, so
    /// the request's movie stays `None` until the caller feeds one).
    #[test]
    fn request_carries_the_proven_descriptor_and_the_external_movie_feed() {
        let g = boot_graph();
        let mut sys = NisSystem::new();
        assert!(sys.request(&g).is_none(), "idle → no request");

        sys.start(&g, "mnis5_2_1").unwrap();
        let req = sys.request(&g).expect("the active cutscene requests");
        assert_eq!(req.label, "nis/mnis5_2_1");
        assert_eq!(req.object, "mnis5_2_1");
        assert_eq!(req.clips, vec!["mnis_5_2_1_mix".to_string()]);
        assert!((req.volume - 1.0).abs() < 1e-6, "the PROVEN volume prop");
        assert_eq!(req.mixbuss, vec!["nis", "master", "dialog"]);
        assert!(req.movie.is_none(), "nothing was fed — never inferred");

        // The external feed rides through exactly as set (the
        // nis_SetFMVFileName counterpart), and the feed can be set
        // while idle.
        sys.set_movie(Some("NIS_5_2_1.bik"));
        assert_eq!(
            sys.request(&g).unwrap().movie.as_deref(),
            Some("NIS_5_2_1.bik"),
            "the fed name, byte-for-byte"
        );

        // The SERVABILITY census (PROVEN here against the committed
        // shard index's own companion data): every one of the 46
        // NIS_Player patches carries EXACTLY ONE mix clip reference —
        // none is mix-less (the block-56 "9 event-driven" note was an
        // imprecise record; the REAL split is 43 mixes shipped in the
        // block-63 index and 3 absent — the `nis0-3a/4/5_mix.wav`
        // intro mixes, which degrade at the shard boundary).  The
        // request's empty-clips arm stays as the defensive contract
        // for a graph that cannot resolve the active patch:
        for (name, patch) in g.patches().filter(|(_, p)| p.class == "NIS_Player") {
            assert_eq!(
                patch.clip_names().len(),
                1,
                "{name}: the corpus's NIS patches all reference one mix"
            );
        }

        // The defensive arm: a graph that cannot resolve the active
        // patch serves TRACKED SILENCE (state stays, request honest).
        let empty = MusicGraph::empty();
        let req = sys.request(&empty).expect("the state still requests");
        assert_eq!(req.object, "mnis5_2_1");
        assert!(req.clips.is_empty(), "unresolvable → tracked silence");
        assert!((req.volume - 1.0).abs() < 1e-6, "the honest default");
        assert!(req.mixbuss.is_empty());
        assert!(sys.pending(), "the state still changed");
    }

    /// THE BLOCK-69 STRUCTURAL ATTACK, pinned in-repo.  Every
    /// structural transform of the 46 REAL `NIS_Player` object names
    /// — underscore insertion after the family prefix, `mnis`→
    /// `NIS_`/`MNIS_`/`mnis_` rewrites, `mb_`→`MB_`/`NIS_` rewrites,
    /// `_`↔`-` separator swaps, the `_FMV` suffix — under the
    /// `movies\`/`movies/`/bare path prefixes and the
    /// `.bik`/`.rmv`/`.p3d`/bare extensions, hashes to NO UNNAMED
    /// member of the committed movies.rcf directory: the six silent
    /// stings stay nameless under the ENTIRE structural space (the
    /// full 7 497 696-candidate sweep incl. the m/s/p grids and the
    /// observed corpus suffix vocabulary is
    /// `scripts/m69_nis_transform_attack.py` — same result, zero
    /// cracks).  The movie name fed to `nis_SetFMVFileName` is DATA,
    /// never derivable — which is exactly why [`NisRequest::movie`]
    /// is an external feed.
    ///
    /// The attack's POSITIVE half is pinned too: where a patch object
    /// and a cracked name coincide the transforms DO reproduce the
    /// real directory entry (`nis2-4` →
    /// `movies\NIS_2_4_FMV.bik`) — a mapping confirmation, not a
    /// complete rule (the part letters drop lossily — `nis7-3a` and
    /// `nis7-3b` both play under the ONE `NIS_7_3_FMV`, and the
    /// `NIS_10-3a` correspondence belongs to the SCENE-ID list of
    /// block 47, not to the 46 audio patch objects, none of which
    /// spells `nis10-3a`).
    #[test]
    fn the_structural_movie_name_space_stays_closed() {
        let g = boot_graph();
        let objects: Vec<String> = g
            .patches()
            .filter(|(_, p)| p.class == "NIS_Player")
            .map(|(name, _)| name.to_string())
            .collect();
        assert_eq!(objects.len(), 46, "the corpus census");

        let unnamed: Vec<u32> = prototype_video::members::BINK_MOVIES
            .iter()
            .filter(|m| m.name.is_none())
            .map(|m| m.name_hash)
            .collect();
        assert_eq!(unnamed.len(), 6, "the honest unknowns");

        let mut candidates = 0usize;
        let mut hit_named: Vec<String> = Vec::new();
        for obj in &objects {
            for base in structural_variants(obj) {
                for pre in ["movies\\", "movies/", ""] {
                    for ext in [".bik", ".rmv", ".p3d", ""] {
                        let name = format!("{pre}{base}{ext}");
                        let h = prototype_rcf::hash_name(&name);
                        candidates += 1;
                        assert!(
                            !unnamed.contains(&h),
                            "{name:?} would CRACK an unnamed member — update members.rs!"
                        );
                        if prototype_video::members::bink_by_hash(h)
                            .map_or(false, |m| m.name.is_some())
                            || prototype_video::members::bible_by_hash(h).is_some()
                        {
                            hit_named.push(name);
                        }
                    }
                }
            }
        }
        assert!(
            candidates > 10_000,
            "the structural space must be substantial: {candidates}"
        );
        // The ONE mechanical correspondence (and ONLY the scene-id
        // family maps — the mnis*/mb_* briefing families hit nothing,
        // pinned by hit_named's exact content below):
        assert!(
            hit_named.iter().any(|n| n == "movies\\NIS_2_4_FMV.bik"),
            "nis2-4 must map to the named movie: {hit_named:?}"
        );
        assert!(
            hit_named.iter().all(|n| {
                let lower = n.to_ascii_lowercase();
                lower.contains("nis_2_4_fmv")
            }),
            "no OTHER family maps: {hit_named:?}"
        );
    }

    /// The movie feed's own semantics: same name → no bump, a change
    /// → bump, `None` clears, and the feed survives an idempotent
    /// re-start but not a stop.
    #[test]
    fn movie_feed_is_an_explicit_external_state() {
        let g = boot_graph();
        let mut sys = NisSystem::new();

        // Feed while idle: stored, rides into the next start.
        assert!(sys.set_movie(Some("NIS_2_4_FMV.bik")));
        assert_eq!(sys.movie(), Some("NIS_2_4_FMV.bik"));
        assert!(sys.pending(), "the feed is a state change");

        // Same name: no bump.
        sys.mark_served();
        let seq = sys.request_seq();
        assert!(!sys.set_movie(Some("NIS_2_4_FMV.bik")));
        assert_eq!(sys.request_seq(), seq);

        // A change bumps; None clears and bumps too.
        assert!(sys.set_movie(Some("NIS_2_4_FMV.RMV.bik")));
        assert_eq!(sys.request_seq(), seq + 1);
        assert!(sys.set_movie(None));
        assert!(sys.movie().is_none());

        // The feed survives an idempotent re-start…
        sys.set_movie(Some("NIS_2_4_FMV.bik"));
        sys.start(&g, "nis2-4").unwrap();
        assert!(!sys.start(&g, "nis2-4").unwrap(), "idempotent re-start");
        assert_eq!(sys.movie(), Some("NIS_2_4_FMV.bik"));
        // …and clears with a stop.
        sys.stop();
        assert!(sys.movie().is_none());
    }

    /// The object-level structural transform space of the block-69
    /// attack (the python sweep's exact generator, mirrored here so
    /// the pin and the sweep cannot drift): case is NOT a dimension
    /// (the hash folds uppercase one-way); the dimensions are the
    /// family-prefix rewrites, the `_FMV` suffix, the separator
    /// swaps.
    fn structural_variants(obj: &str) -> Vec<String> {
        let mut out: Vec<String> = vec![obj.to_string(), format!("{obj}_FMV")];
        let push = |v: String, out: &mut Vec<String>| {
            if !out.contains(&v) {
                out.push(v);
            }
        };
        let (prefixes, rest): (Vec<&str>, &str) = if let Some(rest) = obj.strip_prefix("mnis") {
            (vec!["mnis_", "MNIS_", "MNIS", "NIS_", "NIS", "nis_", "nis"], rest)
        } else if let Some(rest) = obj.strip_prefix("mb_") {
            (vec!["mb", "MB_", "MB", "NIS_", "NIS", "nis_", "nis"], rest)
        } else if let Some(rest) = obj.strip_prefix("nis") {
            (vec!["nis_", "NIS_", "NIS"], rest)
        } else {
            (vec![], obj)
        };
        for pre in prefixes {
            push(format!("{pre}{rest}"), &mut out);
            push(format!("{pre}{rest}_FMV"), &mut out);
        }
        // separator swaps on every variant collected so far
        let swapped: Vec<String> = out
            .iter()
            .flat_map(|v| {
                [
                    v.clone(),
                    v.replace('_', "-"),
                    v.replace('-', "_"),
                ]
            })
            .collect();
        for v in swapped {
            push(v, &mut out);
        }
        out
    }
}

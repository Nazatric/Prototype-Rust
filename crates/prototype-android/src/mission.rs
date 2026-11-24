//! mission — the gameplay state that owns the current mission id and
//! turns it into music requests (block 68) plus the free-roam world
//! bed (block 70).
//!
//! ## What this reconstructs
//!
//! The engine's real mission/music trigger code is UNKNOWN (no
//! mission-script interpreter exists in this rewrite — the loader
//! renders the world and plays the real audio, it does not simulate
//! the game's objectives).  What IS PROVEN is the DATA side: every
//! music bank carries one `Cue` chunk per mission id whose
//! `transitions` ↔ `cells` pairing names the music states the game
//! drives (`ambient`, `suspense`, `combat`, `death`, `complete`, …
//! — block 65/66), and every stem patch carries an explicit `loop`
//! Bool (30/30 `music_player`/`music_player_random` patches across
//! the committed banks: beds true, stingers false) and a `volume`
//! Float32.
//!
//! ## The free-roam world bed (block 70, PROVEN census)
//!
//! The ambience bank (`audio\ambience.p3d`, 0 cues, 3 patches) is
//! the engine's world bed.  The `ambience` patch — class
//! `Ambience_all` — carries `loop = true`, `volume = 2` and
//! `filename = "ambience_baked_18ch"`, the 18-stream IMA master
//! whose real member header was Range-verified this block
//! (scripts/m70_fetch_ambience_headers.py: RADP version 18,
//! 32 000 Hz, 168 946 blocks/stream ≈ 168.9 s).  Its 59 props name
//! the baked layers (city/park/rooftop volumes, traffic
//! idle/panic, crowd panic, infected envelopes) — the engine's
//! dynamic layer routing over those 18 streams is UNKNOWN and is
//! NOT simulated; our serve folds the streams with the documented
//! block-67 fold and plays the result as ONE world bed.
//! The two `AtCamera_Player_Quad` patches (`base_interior_ambience`,
//! `e10m04_ambience`, RADP v4 interior beds with 1000 ms fades) are
//! the interior layer — the interior/exterior crossfade trigger is
//! gameplay-state data this rewrite does not have, so they stay
//! decoded-but-unserved (honest scope, pinned by test).
//!
//! This module is OUR documented model of the missing gameplay half:
//! a small state machine that OWNS the current mission id + music
//! transition, validates both against the decoded
//! [`MusicGraph`](crate::music_graph::MusicGraph) (fail-closed — an
//! unknown mission or transition is a clean `Err`, never a guess),
//! and exposes ONE kind of output: a [`MusicRequest`] describing what
//! the app's live `play_mission_music` seam must serve now.  The app
//! (prototype-android::app) consumes requests through the tagged
//! voice lifecycle; nothing here touches audio devices.
//!
//! ## Model
//!
//! * [`MissionPhase`] — who owns the music: [`MissionPhase::Silent`]
//!   (boot only — nothing plays until an explicit entry),
//!   [`MissionPhase::Frontend`] (the frontend attract state — the
//!   PROVEN `fe_music` bed), [`MissionPhase::InMission`] (a mission's
//!   cue drives the states), [`MissionPhase::FreeRoam`] (the world —
//!   the ambience bank's PROVEN world bed plays; block 70).
//! * The MUSIC channel and the WORLD-BED channel are separate
//!   requests: [`MissionSystem::request`] (music) is `None` in
//!   [`MissionPhase::FreeRoam`] — the banks carry no free-roam MUSIC
//!   bed — while [`MissionSystem::ambience_request`] serves the
//!   world bed there.  In-mission ambience is UNKNOWN (the cue
//!   system never references the ambience bank) and stays silent.
//!   The app keeps the two units on independent mixer tags so the
//!   world bed would survive a music change without being cut by
//!   the music channel's bookkeeping.
//! * [`MissionSystem`] — the state + a monotonic request counter:
//!   every state CHANGE bumps [`MissionSystem::request_seq`]; the app
//!   serves the request and marks it consumed
//!   ([`MissionSystem::pending`]).  Mutators are IDEMPOTENT (re-enter
//!   the same state → no bump → no restart).
//! * [`MusicRequest`] — one stem to serve: the patch object, its
//!   ordered clip names, the patch's PROVEN `loop` and `volume`
//!   props.  `request() == None` is a TRACKED SILENCE (an
//!   `EmptyCell` transition, a stem with no clip reference, or a
//!   graph without the frontend bed) — the state stays honest, the
//!   audio stays silent.
//!
//! ## Confidence labels
//!
//! * PROVEN: the graph data (cues, transitions, stems, clip names —
//!   block 65/66); the `loop`/`volume` patch props (this block's
//!   census); the frontend bed `fe_music → e02m02_ambient` (the
//!   real boot_strap patch); the world bed `ambience →
//!   ambience_baked_18ch` with `loop = true`, `volume = 2` (the
//!   real ambience bank, decoded byte-exact; the member header
//!   Range-verified — 18 streams at 32 kHz).
//! * DESIGN (documented): mission ENTRY plays the cue's `ambient`
//!   transition when present, else the cue's first transition; the
//!   frontend bed is the `fe_music` patch (the `front_end` cue's own
//!   stems `frontend_drone`/`frontend_music` do not resolve to
//!   patches in the decoded graph — cross-bank objects — so the
//!   frontend request uses the player the frontend code provably
//!   drives); leaving a mission goes to [`MissionPhase::FreeRoam`]
//!   (the world bed takes over — block 70's upgrade of the block-68
//!   honest silence); the world bed plays in free-roam ONLY (the
//!   real engine's in-mission ambience behavior is UNKNOWN).
//! * UNKNOWN: what the real engine's mission code passes and when —
//!   this module exposes the seam; the trigger system (mission
//!   scripts) does not exist here; the 18-stream layer routing and
//!   the interior/exterior crossfade are not simulated.

#![forbid(unsafe_code)]

use crate::music_graph::MusicGraph;

/// The frontend attract state's music stem: the REAL `fe_music`
/// patch of `audio\boot_strap.p3d` (class `music_player`, `loop =
/// true`, `volume = 1`, `filename = "e02m02_ambient"` — the frontend
/// attract bed IS a mission ambient stem, block 66 + this block's
/// census).
pub const FRONTEND_MUSIC_STEM: &str = "fe_music";

/// The free-roam WORLD BED: the REAL `ambience` patch of
/// `audio\ambience.p3d` (class `Ambience_all`, `loop = true`,
/// `volume = 2`, `filename = "ambience_baked_18ch"` — the 18-stream
/// IMA world master, block 70's census; the member header
/// Range-verified at RADP version 18 / 32 000 Hz).  The patch's
/// other 57 props (city/park/rooftop volumes, traffic/crowd/infected
/// envelopes) describe the engine's dynamic layer mix over the 18
/// baked streams — that routing is UNKNOWN and is not simulated.
pub const WORLD_AMBIENCE_STEM: &str = "ambience";

/// Who owns the music right now.  `Default` = [`MissionPhase::Silent`]
/// (the initial phase: boot itself starts no music).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MissionPhase {
    /// No music (boot before the frontend).  The initial phase:
    /// nothing plays until an explicit state entry.
    #[default]
    Silent,
    /// The frontend attract state — the PROVEN `fe_music` bed.
    Frontend,
    /// A mission is active; its cue drives the transitions.
    InMission,
    /// The free-roam world — the ambience bank's PROVEN world bed
    /// plays (block 70).  Reached explicitly or by leaving a mission.
    FreeRoam,
}

/// What the app's live music seam must serve RIGHT NOW: one stem of
/// the decoded graph, resolved to its ordered clip names with the
/// patch's own PROVEN `loop` and `volume` properties carried
/// through.  The app turns this into tagged, loop-aware voices via
/// the shard clip server.
#[derive(Debug, Clone, PartialEq)]
pub struct MusicRequest {
    /// Diagnostics label: `frontend/fe_music` or `e10m04/ambient`.
    pub label: String,
    /// The stem patch object (`e10m04_ambient`, `fe_music`).
    pub stem: String,
    /// The ordered clip filenames (shipped order, block 65/66).
    pub clips: Vec<String>,
    /// The patch's `loop` Bool (PROVEN data; `false` when the patch
    /// carries no such prop — a missing claim plays once, never
    /// loops on a guess).
    pub loop_voice: bool,
    /// The patch's `volume` Float32 (PROVEN prop; 1.0 when absent).
    /// The app multiplies its caller gain by this.
    pub volume: f32,
}

/// The mission state machine: owns the current mission id + music
/// transition, validates against the decoded graph, and produces
/// [`MusicRequest`]s.  Device-free; fully deterministic; every
/// mutation is idempotent (same state → no bump → the app restarts
/// nothing).
#[derive(Debug, Clone, Default)]
pub struct MissionSystem {
    phase: MissionPhase,
    /// The current mission id (exact graph key, e.g. `e10m04`).
    /// `None` outside [`MissionPhase::InMission`].
    mission: Option<String>,
    /// The current music transition (canonical cue spelling, e.g.
    /// `ambient`).  Empty outside a mission.
    transition: String,
    /// Bumped on every STATE CHANGE; the app serves the request and
    /// marks it consumed.  Monotonic within a session.
    request_seq: u64,
    /// The [`Self::request_seq`] value the app last consumed.
    served_seq: u64,
}

impl MissionSystem {
    /// The initial state: [`MissionPhase::Silent`] with NO pending
    /// request — boot itself starts no music; the frontend entry
    /// does.
    pub fn new() -> Self {
        Self::default()
    }

    /// The current phase.
    pub fn phase(&self) -> MissionPhase {
        self.phase
    }

    /// The current mission id (exact key) inside a mission.
    pub fn mission(&self) -> Option<&str> {
        self.mission.as_deref()
    }

    /// The current transition (canonical cue spelling) inside a
    /// mission.
    pub fn transition(&self) -> &str {
        &self.transition
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

    /// Enter the frontend attract state.  Idempotent: already
    /// frontend → no change, `false`.  Any other phase → the music
    /// ownership moves (a mission bed stops via the app's serve
    /// path), `true`.
    pub fn enter_frontend(&mut self) -> bool {
        if self.phase == MissionPhase::Frontend {
            return false;
        }
        self.phase = MissionPhase::Frontend;
        self.mission = None;
        self.transition.clear();
        self.request_seq = self.request_seq.wrapping_add(1);
        true
    }

    /// Enter a mission by id.  The entry transition is the cue's
    /// `ambient` when present, else the cue's first transition
    /// (DESIGN — documented at the module head).  Unknown mission →
    /// `Err`, state untouched.  Entering the SAME mission already
    /// active on the SAME default transition → idempotent `Ok` with
    /// no bump (the app restarts nothing).
    ///
    /// Mission ids are EXACT graph keys (`e10m04`) — the cues are
    /// stored lower-case and the lookup does not case-fold (the
    /// engine's own mission vocabulary; a wrong-case id is a clean
    /// miss, pinned by test).
    pub fn enter_mission(&mut self, graph: &MusicGraph, mission: &str) -> Result<String, String> {
        let cue = graph
            .cue(mission)
            .ok_or_else(|| format!("mission: no cue named {mission:?} in the graph"))?;
        let default = if let Some(t) = cue
            .transitions
            .iter()
            .find(|t| t.eq_ignore_ascii_case("ambient"))
        {
            t.clone()
        } else {
            cue.transitions[0].clone()
        };
        if self.phase == MissionPhase::InMission
            && self.mission.as_deref() == Some(mission)
            && self.transition == default
        {
            return Ok(default);
        }
        self.phase = MissionPhase::InMission;
        self.mission = Some(mission.to_string());
        self.transition = default.clone();
        self.request_seq = self.request_seq.wrapping_add(1);
        Ok(default)
    }

    /// Drive one music transition of the CURRENT mission (the cue's
    /// canonical spelling is stored; lookup case-insensitive, the
    /// PROVEN `stem_for` behavior).  Unknown transition (or no
    /// mission active) → `Err`, state untouched.  The SAME
    /// transition → idempotent, no bump.
    pub fn set_transition(
        &mut self,
        graph: &MusicGraph,
        transition: &str,
    ) -> Result<String, String> {
        let mission = self
            .mission
            .as_deref()
            .ok_or_else(|| "mission: no mission is active".to_string())?;
        let cue = graph.cue(mission).ok_or_else(|| {
            format!(
                "mission: the active mission {mission:?} vanished from the graph"
            )
        })?;
        let canonical = cue
            .transitions
            .iter()
            .find(|t| t.eq_ignore_ascii_case(transition))
            .ok_or_else(|| {
                format!(
                    "mission: {mission:?} has no transition {transition:?} (has: {})",
                    cue.transitions.join(", ")
                )
            })?;
        let canonical = canonical.clone();
        if self.transition == canonical {
            return Ok(canonical);
        }
        self.transition = canonical.clone();
        self.request_seq = self.request_seq.wrapping_add(1);
        Ok(canonical)
    }

    /// Leave the mission — back to the free-roam world
    /// ([`MissionPhase::FreeRoam`], block 70: the ambience bank's
    /// PROVEN world bed takes over; the block-68 honest silence was
    /// upgraded when the census proved the world bed).  Idempotent:
    /// `false` when not in a mission.
    pub fn leave_mission(&mut self) -> bool {
        if self.phase != MissionPhase::InMission {
            return false;
        }
        self.phase = MissionPhase::FreeRoam;
        self.mission = None;
        self.transition.clear();
        self.request_seq = self.request_seq.wrapping_add(1);
        true
    }

    /// Enter the free-roam world ([`MissionPhase::FreeRoam`]): the
    /// state where the ambience bank's PROVEN world bed plays.
    /// Idempotent: already free-roam → no change, `false`.  Any other
    /// phase → the ownership moves (a mission/frontend unit stops via
    /// the app's serve path; the world bed starts), `true`.  The
    /// graph is NOT consulted here — a graph without the ambience
    /// bank enters free-roam into TRACKED SILENCE (the request is
    /// `None`), exactly the frontend pattern.
    pub fn enter_free_roam(&mut self) -> bool {
        if self.phase == MissionPhase::FreeRoam {
            return false;
        }
        self.phase = MissionPhase::FreeRoam;
        self.mission = None;
        self.transition.clear();
        self.request_seq = self.request_seq.wrapping_add(1);
        true
    }

    /// The WORLD-BED request the app must serve NOW: Some exactly
    /// when the phase is [`MissionPhase::FreeRoam`] AND the graph
    /// resolves [`WORLD_AMBIENCE_STEM`] to clips.  `None` in every
    /// other phase (boot/frontend/mission — the real engine's
    /// in-mission ambience behavior is UNKNOWN, so the bed plays in
    /// free-roam only, DESIGN documented at the module head) and in
    /// free-roam when the graph carries no ambience bank (tracked
    /// silence — the state stays honest).
    pub fn ambience_request(&self, graph: &MusicGraph) -> Option<MusicRequest> {
        if self.phase != MissionPhase::FreeRoam {
            return None;
        }
        let clips = graph.clips_for_stem(WORLD_AMBIENCE_STEM);
        if clips.is_empty() {
            return None;
        }
        Some(MusicRequest {
            label: format!("freeroam/{WORLD_AMBIENCE_STEM}"),
            stem: WORLD_AMBIENCE_STEM.to_string(),
            clips,
            loop_voice: graph.stem_loops(WORLD_AMBIENCE_STEM),
            volume: patch_volume(graph, WORLD_AMBIENCE_STEM),
        })
    }

    /// The request the app must serve NOW: `None` = tracked silence
    /// (no phase, an `EmptyCell` transition, a stem whose patch
    /// carries no clip reference, or a graph that cannot resolve the
    /// frontend bed).  The STATE stays exactly as set — silence is a
    /// first-class outcome, never an error.
    pub fn request(&self, graph: &MusicGraph) -> Option<MusicRequest> {
        match self.phase {
            // Boot AND free-roam: the MUSIC channel is silent.  In
            // free-roam the world bed is the sound, and it rides the
            // separate [`Self::ambience_request`] channel (the banks
            // carry no free-roam MUSIC bed — the ambience bank is not
            // a cue bank at all: 0 cues, block 70 census).
            MissionPhase::Silent | MissionPhase::FreeRoam => None,
            MissionPhase::Frontend => {
                let clips = graph.clips_for_stem(FRONTEND_MUSIC_STEM);
                if clips.is_empty() {
                    return None;
                }
                Some(MusicRequest {
                    label: format!("frontend/{FRONTEND_MUSIC_STEM}"),
                    stem: FRONTEND_MUSIC_STEM.to_string(),
                    clips,
                    loop_voice: graph.stem_loops(FRONTEND_MUSIC_STEM),
                    volume: patch_volume(graph, FRONTEND_MUSIC_STEM),
                })
            }
            MissionPhase::InMission => {
                let mission = self.mission.as_deref()?;
                let stem = graph.stem_for(mission, &self.transition)?;
                let clips = graph.clips_for_stem(stem);
                if clips.is_empty() {
                    return None;
                }
                Some(MusicRequest {
                    label: format!("{mission}/{}", self.transition),
                    stem: stem.to_string(),
                    clips,
                    loop_voice: graph.stem_loops(stem),
                    volume: patch_volume(graph, stem),
                })
            }
        }
    }
}

/// The patch's `volume` Float32 (PROVEN prop; 1.0 when the patch or
/// the prop is absent — the corpus always carries it).
fn patch_volume(graph: &MusicGraph, stem: &str) -> f32 {
    match graph
        .patch(stem)
        .and_then(|p| p.prop("volume"))
    {
        Some(prototype_p3d::bank::BankValue::Float32(v)) => *v,
        _ => 1.0,
    }
}

// ---------------------------------------------------------------------------
// Tests — REAL fixtures: the committed block-65 banks + boot_strap
// (the user's own audio data, decoded by the byte-exact parser).
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

    /// A graph merged from REAL bank bytes (the from_game_set archive
    /// path is music_graph's own suite).
    fn graph(names: &[&str]) -> MusicGraph {
        let mut g = MusicGraph::empty();
        for name in names {
            let bytes = fixture_bytes(name);
            let source = format!("audio\\{name}");
            g.merge_bank_bytes(&bytes, &source)
                .unwrap_or_else(|e| panic!("{name} must decode: {e}"));
        }
        g
    }

    // -----------------------------------------------------------------
    // The frontend phase
    // -----------------------------------------------------------------

    #[test]
    fn frontend_request_serves_the_proven_fe_music_bed() {
        let g = graph(&["boot_strap.p3d"]);
        let mut m = MissionSystem::new();
        // Boot itself is SILENT — no request until an explicit entry.
        assert_eq!(m.phase(), MissionPhase::Silent);
        assert!(m.request(&g).is_none());
        assert!(!m.pending());

        assert!(m.enter_frontend());
        assert_eq!(m.phase(), MissionPhase::Frontend);
        let req = m.request(&g).expect("the frontend bed resolves");
        assert_eq!(req.stem, "fe_music");
        assert_eq!(req.clips, vec!["e02m02_ambient".to_string()]);
        assert!(req.loop_voice, "the PROVEN loop prop: the bed repeats");
        assert!((req.volume - 1.0).abs() < 1e-6);
        assert_eq!(req.label, "frontend/fe_music");
        assert!(m.pending(), "the app owes a serve");
        m.mark_served();
        assert!(!m.pending());

        // Idempotent re-entry: no bump, no restart.
        let seq = m.request_seq();
        assert!(!m.enter_frontend());
        assert_eq!(m.request_seq(), seq);
        assert!(!m.pending());
    }

    #[test]
    fn frontend_without_the_bed_patch_is_tracked_silence() {
        // A graph WITHOUT boot_strap carries no fe_music patch — the
        // frontend state still tracks, the request is honest silence.
        let g = graph(&["bank_e10_music.p3d"]);
        let mut m = MissionSystem::new();
        m.enter_frontend();
        assert_eq!(m.phase(), MissionPhase::Frontend);
        assert!(m.pending(), "the state still changed");
        assert!(m.request(&g).is_none(), "no bed patch → no request");
        m.mark_served();
        assert!(!m.pending());
    }

    // -----------------------------------------------------------------
    // The mission phase: entry default, transitions, idempotence
    // -----------------------------------------------------------------

    #[test]
    fn mission_entry_defaults_to_ambient_and_walks_the_proven_states() {
        let g = graph(&["bank_e10_music.p3d"]);
        let mut m = MissionSystem::new();

        // Unknown mission: clean Err, state untouched.
        let err = m.enter_mission(&g, "e99m99").unwrap_err();
        assert!(err.contains("no cue"), "{err}");
        assert_eq!(m.phase(), MissionPhase::Silent);

        // Wrong-case id: cues are EXACT keys — a clean miss, not a
        // case-fold (the graph's mission vocabulary is lower-case).
        let err = m.enter_mission(&g, "E10M04").unwrap_err();
        assert!(err.contains("no cue"), "{err}");
        assert_eq!(m.phase(), MissionPhase::Silent);

        // The PROVEN entry: e10m04 defaults to `ambient` (the cue's
        // first transition AND the ambient bed), loop = true.
        let t = m.enter_mission(&g, "e10m04").expect("the cue exists");
        assert_eq!(t, "ambient");
        assert_eq!(m.phase(), MissionPhase::InMission);
        assert_eq!(m.mission(), Some("e10m04"));
        let req = m.request(&g).expect("the ambient stem resolves");
        assert_eq!(req.stem, "e10m04_ambient");
        assert_eq!(req.clips, vec!["e10m04_ambient".to_string()]);
        assert!(req.loop_voice, "PROVEN: the ambient bed loops");
        assert_eq!(req.label, "e10m04/ambient");

        // combat: a looping state bed.
        let t = m.set_transition(&g, "COMBAT").expect("case-insensitive");
        assert_eq!(t, "combat", "the canonical cue spelling is stored");
        let req = m.request(&g).unwrap();
        assert_eq!(req.stem, "e10m04_combat");
        assert!(req.loop_voice, "PROVEN: combat loops");

        // complete: the random player's two stingers, shipped order,
        // loop = FALSE (PROVEN census — a stinger plays once).
        let t = m.set_transition(&g, "complete").unwrap();
        assert_eq!(t, "complete");
        let req = m.request(&g).unwrap();
        assert_eq!(
            req.clips,
            vec!["e10m04_complete02".to_string(), "e10m04_complete01".to_string()]
        );
        assert!(!req.loop_voice, "PROVEN: the complete stinger does not loop");

        // Unknown transition: Err, state unchanged, no extra bump.
        let seq = m.request_seq();
        let err = m.set_transition(&g, "stealth").unwrap_err();
        assert!(err.contains("no transition"), "{err}");
        assert_eq!(m.transition(), "complete");
        assert_eq!(m.request_seq(), seq);
        assert_eq!(m.request(&g).unwrap().stem, "e10m04_complete");
    }

    #[test]
    fn mission_mutators_are_idempotent_and_track_bumps() {
        let g = graph(&["bank_e10_music.p3d"]);
        let mut m = MissionSystem::new();
        m.enter_mission(&g, "e10m04").unwrap();
        let seq = m.request_seq();
        // Same mission on the same default: no bump, no restart.
        assert_eq!(m.enter_mission(&g, "e10m04").unwrap(), "ambient");
        assert_eq!(m.request_seq(), seq);
        // Same transition: no bump.
        assert_eq!(m.set_transition(&g, "ambient").unwrap(), "ambient");
        assert_eq!(m.request_seq(), seq);
        // A different transition IS a change (the state moves).
        assert!(m.set_transition(&g, "death").is_ok());
        assert_eq!(m.request_seq(), seq + 1);
        // Leaving: the free-roam world (block 70), tracked music
        // silence (the world bed rides the ambience channel, and
        // this e10-only graph carries none), idempotent.
        assert!(m.leave_mission());
        assert_eq!(m.phase(), MissionPhase::FreeRoam);
        assert!(m.mission().is_none());
        assert!(m.request(&g).is_none(), "free-roam MUSIC is silent");
        assert!(m.ambience_request(&g).is_none(), "no ambience bank here");
        assert!(!m.leave_mission(), "leaving twice is a no-op");
        // set_transition without a mission: clean Err.
        let err = m.set_transition(&g, "combat").unwrap_err();
        assert!(err.contains("no mission"), "{err}");
    }

    #[test]
    fn free_roam_serves_the_proven_world_bed() {
        // REAL ambience bank (the committed fixture): the `ambience`
        // patch resolves to the 18-stream world master with its own
        // PROVEN loop/volume props.
        let g = graph(&["bank_ambience.p3d"]);
        let mut m = MissionSystem::new();

        // Boot: no world bed, no music.
        assert!(m.ambience_request(&g).is_none());
        assert!(m.request(&g).is_none());
        assert!(!m.pending());

        // The explicit free-roam entry: the state moves, the request
        // resolves the PROVEN world bed.
        assert!(m.enter_free_roam());
        assert_eq!(m.phase(), MissionPhase::FreeRoam);
        let req = m.ambience_request(&g).expect("the world bed resolves");
        assert_eq!(req.stem, "ambience");
        assert_eq!(
            req.clips,
            vec!["ambience_baked_18ch".to_string()],
            "the PROVEN filename prop"
        );
        assert!(req.loop_voice, "PROVEN: the world bed loops");
        assert_eq!(req.volume, 2.0, "PROVEN: the patch's volume prop is 2");
        assert_eq!(req.label, "freeroam/ambience");
        assert!(m.pending(), "the app owes a serve");
        assert!(m.request(&g).is_none(), "free-roam MUSIC stays silent");
        m.mark_served();
        assert!(!m.pending());

        // Idempotent re-entry: no bump, no restart.
        let seq = m.request_seq();
        assert!(!m.enter_free_roam());
        assert_eq!(m.request_seq(), seq);
        assert!(!m.pending());

        // The interior beds are decoded but NEVER serve the free-roam
        // request: the world bed is the `ambience` patch (the
        // interior/exterior crossfade is UNKNOWN, documented).
        assert!(g.patch("base_interior_ambience").is_some());
        assert!(g.patch("e10m04_ambience").is_some());
        assert_ne!(req.stem, "base_interior_ambience");
    }

    #[test]
    fn free_roam_without_the_bank_is_tracked_silence() {
        // A graph WITHOUT the ambience bank: the state still tracks,
        // both channels are honest silence.
        let g = graph(&["bank_e10_music.p3d"]);
        let mut m = MissionSystem::new();
        m.enter_free_roam();
        assert_eq!(m.phase(), MissionPhase::FreeRoam);
        assert!(m.pending(), "the state still changed");
        assert!(m.ambience_request(&g).is_none(), "no bed → no request");
        m.mark_served();
        assert!(!m.pending());
    }

    #[test]
    fn the_world_bed_plays_in_free_roam_only() {
        let g = graph(&["bank_ambience.p3d", "bank_e10_music.p3d", "boot_strap.p3d"]);
        let mut m = MissionSystem::new();

        // Every non-free-roam phase: no world-bed request.
        assert!(m.ambience_request(&g).is_none(), "boot");
        m.enter_frontend();
        assert!(m.ambience_request(&g).is_none(), "frontend");
        m.mark_served();
        m.enter_mission(&g, "e10m04").unwrap();
        assert!(m.ambience_request(&g).is_none(), "in-mission: UNKNOWN");
        m.mark_served();

        // Leaving lands in free-roam and the bed takes over; the
        // MUSIC request is gone (the mission bed was served by the
        // state change itself).
        assert!(m.leave_mission());
        assert_eq!(m.phase(), MissionPhase::FreeRoam);
        assert!(m.request(&g).is_none());
        assert!(m.ambience_request(&g).is_some());

        // The frontend re-enters over free-roam: the bed's channel
        // goes silent again (the frontend is the menu).
        assert!(m.enter_frontend());
        assert!(m.ambience_request(&g).is_none());
        assert!(m.request(&g).is_some(), "the frontend bed is music");
    }

    #[test]
    fn e02_exercises_the_empty_cell_and_the_variant_sting() {
        let g = graph(&["bank_e02_music.p3d"]);
        let mut m = MissionSystem::new();

        // The localized cue: ambient present → default ambient; the
        // stem's patch is the VARIANT STING (e02m02_fr_ambient_sting)
        // and its PROVEN loop prop is FALSE — plays once.
        m.enter_mission(&g, "e02m02_fr").unwrap();
        assert_eq!(m.transition(), "ambient");
        let req = m.request(&g).expect("the sting resolves");
        assert_eq!(req.stem, "e02m02_fr_ambient");
        assert_eq!(req.clips, vec!["e02m02_fr_ambient_sting".to_string()]);
        assert!(!req.loop_voice, "PROVEN census: this sting does not loop");

        // The EmptyCell: `init` is a real transition with no stem —
        // a TRACKED SILENCE (state stays, request None).
        m.set_transition(&g, "init").unwrap();
        assert_eq!(m.phase(), MissionPhase::InMission);
        assert_eq!(m.transition(), "init");
        assert!(m.pending());
        assert!(m.request(&g).is_none(), "EmptyCell → tracked silence");
        m.mark_served();
        assert!(!m.pending());
    }

    #[test]
    fn mission_switch_reuses_the_entry_contract() {
        let g = graph(&["bank_e02_music.p3d", "bank_e10_music.p3d"]);
        let mut m = MissionSystem::new();
        m.enter_mission(&g, "e10m04").unwrap();
        assert_eq!(m.mission(), Some("e10m04"));
        // A different mission: the cue switches, the default applies.
        let t = m.enter_mission(&g, "e02m02").unwrap();
        assert_eq!(t, "ambient");
        assert_eq!(m.mission(), Some("e02m02"));
        let req = m.request(&g).unwrap();
        assert_eq!(req.stem, "e02m02_ambient");
        assert!(req.loop_voice, "PROVEN: e02m02_ambient loops");
        // And the frontend takes ownership back over the mission bed.
        assert!(m.enter_frontend());
        assert_eq!(m.phase(), MissionPhase::Frontend);
        assert!(m.mission().is_none());
    }
}

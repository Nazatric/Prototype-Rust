//! seam_survey — THE MISSION-SCRIPT SEAM SURVEY (block 73).
//!
//! Block 72's recurring honest-scope note was "nothing calls the app
//! seams automatically (no mission scripts)".  This module answers the
//! question **with data**: what DO the game's shipped scripts call,
//! and which of prototype-rs's seams would serve each call?
//!
//! ## The corpus (all real bytes, committed fixture `scripts.rcf`)
//!
//! The archive the user's own game install ships (`scripts.rcf`,
//! 186 638 B) carries exactly SEVEN members:
//!
//! | # | kind  | size    | identity (PROVEN)                                   |
//! |---|-------|---------|-----------------------------------------------------|
//! | 1 | Text  | 39 686  | credits roll ("EXECUTIVE PRODUCER" …), EN tail      |
//! | 2 | Text  | 39 686  | credits roll — BYTE-IDENTICAL to #1                 |
//! | 3 | Text  | 39 687  | credits roll, Italian tail ("zlib versione …")      |
//! | 4 | Text  | 39 686  | credits roll, Italian tail, distinct bytes          |
//! | 5 | Lua51 | 4 383   | `scripts\frevents.lua` (60 free-roam events)        |
//! | 6 | P3D   | 8 683   | `proto::StatsManager` + `proto::AchievementsManager`|
//! | 7 | Lua51 | 2 318   | the mission-definition chunk (`AddValidMission`×70) |
//!
//! The four ~39.7 KB text members are the localized CREDITS (the DLL
//! names the pattern `scripts/credits_%s.txt`), not scripts.  The P3D
//! member is the DLL's `scripts/statsachievements.tod/_tod.p3d` pair:
//! two content-definition chunks whose META sections carry the stat
//! labels (29) and the achievement names (40).  **The only compiled
//! Lua in the game data we hold is #5 and #7.**
//!
//! ## The interpreter verdict (PROVEN, run on the real bytes)
//!
//! Across the whole shipped corpus the scripts import exactly SIX
//! engine natives: `AddValidMission`, `fre_RegisterEvent`,
//! `fre_RegisterCollectibles`, `fre_ResetEvent`, `fre_DestroyEvent`,
//! `util_GetRandomFloat`.  Call counts: 70 mission registrations,
//! 60 event registrations + 1 collectibles batch.  **Zero** calls
//! land in the audio / movie / options / camera / streaming families
//! (`snd_*`, `fmv_*`, `nis_*`, `nism_*`, `opt_*`, `woi_*`, `cm_*`,
//! `sc_*`, `tod_*`, `sgm_*`, `stream_*`, `fe_*`) — pinned by test.
//! So the app-level AV seams have NO script-side caller in the
//! shipped data: their historical drivers are the engine's own C++
//! subsystems (the same conclusion the block-70/71/72 honest-scope
//! notes guessed — now it is evidence, not a guess).  The part that
//! IS script-driven — the mission registry and the free-roam event
//! lifecycle — is exactly the part prototype-rs already serves
//! (`missions_load` + the fre-event registry + the lifecycle
//! dispatch).
//!
//! ## The seam coverage table (what COULD be called)
//!
//! [`seam_coverage`] walks the M7 registry (all 1 119 recovered
//! `prototypeengine.dll` Lua-host registrations) and attaches a
//! [`SeamStatus`] to every subsystem prefix: families prototype-rs
//! serves today, families with named natives but no seam, and the
//! gameplay-simulation families that are out of the loader's scope.
//! The table is complete by construction (every registration lands
//! in exactly one row — pinned by test).

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use prototype_p3d::P3dFile;
use prototype_rcf::{PayloadKind, RcfArchive};

use crate::lua_api::LuaApiRegistry;
use crate::script_host::{classify_script, run_script};

// ---------------------------------------------------------------------------
// The script corpus census
// ---------------------------------------------------------------------------

/// What kind of payload a scripts.rcf member carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberKind {
    /// Compiled Lua 5.1 bytecode (`\x1bLua\x51`).
    Lua51,
    /// Plain text (the localized credits rolls).
    Text,
    /// A Pure3D file (the stats/achievements content definitions).
    P3d,
    /// Anything else (none observed in the real archive).
    Other,
}

/// The survey of one compiled-Lua member: what it imports, what it
/// exports, which engine natives it calls and how often.
#[derive(Debug, Clone)]
pub struct ScriptSurvey {
    /// Member payload size (bytes).
    pub size: u32,
    /// Globals the script reads (whole proto tree).
    pub imports: Vec<String>,
    /// Globals the script writes.
    pub exports: Vec<String>,
    /// Imports that resolved to M7-registered engine natives.
    pub engine_hosts: Vec<String>,
    /// Imports that resolved to nothing (empty for every shipped
    /// script — PROVEN, asserted by test).
    pub unresolved_imports: Vec<String>,
    /// Exported globals holding closures (lifecycle hooks).
    pub closure_exports: Vec<String>,
    /// Host calls actually made when the script runs on the safe VM,
    /// `(name, call_count)` sorted by count descending then name.
    pub host_call_counts: Vec<(String, usize)>,
}

/// One scripts.rcf member, fully characterized.
#[derive(Debug, Clone)]
pub struct CorpusMember {
    /// The directory entry's name hash (the archive stores no plain
    /// names; the mission chunk's hash stays UNCRACKED — block 39).
    pub name_hash: u32,
    /// Member payload size (bytes).
    pub size: u32,
    /// Payload kind (sniffed).
    pub kind: MemberKind,
    /// Full survey, for Lua51 members.
    pub script: Option<ScriptSurvey>,
    /// First line, for Text members.
    pub head_line: Option<String>,
    /// The `zlib vers(ion|ione)` attribution line, for Text members
    /// (the shipped rolls embed the library credits too).
    pub zlib_marker: Option<String>,
    /// Content-definition names, for P3D members.
    pub content_names: Vec<String>,
    /// META-section record counts, for P3D members (the stat labels
    /// and achievement names live there).
    pub meta_counts: Vec<u32>,
}

/// The complete census of the real scripts.rcf.
#[derive(Debug, Clone)]
pub struct CorpusCensus {
    /// One record per archive member, in directory order.
    pub members: Vec<CorpusMember>,
    /// Groups of members whose payloads are byte-identical (the two
    /// English credits rolls ship twice).  Each inner list holds the
    /// members' name hashes; groups of one are omitted.
    pub byte_identical_groups: Vec<Vec<u32>>,
}

impl CorpusCensus {
    /// The Lua51 members' surveys, in directory order.
    pub fn scripts(&self) -> impl Iterator<Item = &ScriptSurvey> {
        self.members.iter().filter_map(|m| m.script.as_ref())
    }

    /// The UNION of engine-native names the shipped scripts import
    /// (sorted, deduped) — the six-name verdict set.
    pub fn shipped_engine_hosts(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .scripts()
            .flat_map(|s| s.engine_hosts.iter().cloned())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Total call count of one host name across the whole corpus.
    pub fn host_call_total(&self, name: &str) -> usize {
        self.scripts()
            .map(|s| {
                s.host_call_counts
                    .iter()
                    .filter(|(n, _)| n == name)
                    .map(|(_, c)| *c)
                    .sum::<usize>()
            })
            .sum()
    }
}

/// Survey the real scripts.rcf archive bytes.
///
/// Runs BOTH shipped Lua chunks on the safe VM (record-only stubs) —
/// the same proven path [`run_script`] uses — and decodes the P3D
/// member's content definitions.  Fails closed on archive/Lua/P3D
/// errors; per-member text extraction is fail-soft (lossy UTF-8).
pub fn survey_scripts_rcf(rcf_bytes: &[u8]) -> Result<CorpusCensus, prototype_common::P3dError> {
    let arc = RcfArchive::parse(rcf_bytes)?;

    // Byte-identity groups over raw payloads (small archive — pairwise
    // equality against already-seen payloads is plenty).
    let mut payloads: Vec<(u32, Vec<u8>)> = Vec::new();
    for e in &arc.entries {
        payloads.push((e.name_hash, arc.payload(e)?.to_vec()));
    }
    let mut byte_identical_groups: Vec<Vec<u32>> = Vec::new();
    let mut claimed: Vec<bool> = vec![false; payloads.len()];
    for i in 0..payloads.len() {
        if claimed[i] {
            continue;
        }
        let mut group = vec![payloads[i].0];
        for j in (i + 1)..payloads.len() {
            if !claimed[j] && payloads[j].1 == payloads[i].1 {
                group.push(payloads[j].0);
                claimed[j] = true;
            }
        }
        if group.len() > 1 {
            byte_identical_groups.push(group);
        }
    }

    let mut members = Vec::new();
    for e in &arc.entries {
        let payload = arc.payload(e)?;
        let kind = match PayloadKind::sniff(payload) {
            PayloadKind::Lua51 => MemberKind::Lua51,
            PayloadKind::P3D => MemberKind::P3d,
            PayloadKind::Text => MemberKind::Text,
            _ => MemberKind::Other,
        };

        let mut script = None;
        let mut head_line = None;
        let mut zlib_marker = None;
        let mut content_names: Vec<String> = Vec::new();
        let mut meta_counts: Vec<u32> = Vec::new();

        match kind {
            MemberKind::Lua51 => {
                let file = prototype_lua::Lua51File::parse(payload)?;
                let mut report = classify_script(&file);
                let run = run_script(&file)
                    .map_err(|e| prototype_common::P3dError::Other(format!("script run: {e}")))?;
                // Keep the static classification but take the REAL
                // host-call log from the run (they agree — pinned by
                // test for frevents in script_host).
                report.closure_exports = run.closure_exports;
                let mut counts: BTreeMap<String, usize> = BTreeMap::new();
                for call in &run.host_calls {
                    *counts.entry(call.name.clone()).or_default() += 1;
                }
                let mut host_call_counts: Vec<(String, usize)> =
                    counts.into_iter().collect();
                host_call_counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                script = Some(ScriptSurvey {
                    size: e.file_size,
                    imports: report.imports,
                    exports: report.exports,
                    engine_hosts: report.engine_hosts,
                    unresolved_imports: report.unresolved_imports,
                    closure_exports: report.closure_exports,
                    host_call_counts,
                });
            }
            MemberKind::Text => {
                let text = String::from_utf8_lossy(payload);
                head_line = text.lines().next().map(str::to_owned);
                zlib_marker = text
                    .lines()
                    .find(|l| l.starts_with("zlib vers"))
                    .map(str::to_owned);
            }
            MemberKind::P3d => {
                let file = P3dFile::parse(payload)?;
                for chunk in &file.chunks {
                    if let Ok(Some(content)) = chunk.content() {
                        content_names.extend(content.names.iter().cloned());
                        if let Some(meta) = &content.meta {
                            meta_counts.push(meta.count);
                        }
                    }
                }
            }
            MemberKind::Other => {}
        }

        members.push(CorpusMember {
            name_hash: e.name_hash,
            size: e.file_size,
            kind,
            script,
            head_line,
            zlib_marker,
            content_names,
            meta_counts,
        });
    }

    let lua_count = members.iter().filter(|m| m.kind == MemberKind::Lua51).count();
    let text_count = members.iter().filter(|m| m.kind == MemberKind::Text).count();
    let _ = (lua_count, text_count); // derived on demand below

    Ok(CorpusCensus {
        members,
        byte_identical_groups,
    })
}

// ---------------------------------------------------------------------------
// The seam coverage table
// ---------------------------------------------------------------------------

/// How much of one native family prototype-rs serves today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SeamStatus {
    /// A working seam serves (at least the script-called subset of)
    /// the family end to end.
    Served,
    /// Real seams exist for part of the family; the rest is named but
    /// unserved (the honest middle).
    Partial,
    /// The family is named in the registry but no seam exists.
    Unserved,
    /// The family belongs to a subsystem the loader-style rewrite does
    /// not reproduce at all (gameplay simulation, FE UI, tooling).
    OutOfScope,
}

/// One coverage row: a registry subsystem prefix and our seam status.
#[derive(Debug, Clone)]
pub struct SeamCoverageRow {
    /// Subsystem prefix (`"nis_SetFMVFileName" -> "nis"`).
    pub prefix: String,
    /// Registrations in the M7 registry carrying this prefix.
    pub registrations: usize,
    /// The verdict.
    pub status: SeamStatus,
    /// The prototype-rs seam path serving it (or `"-"`).
    pub seam: &'static str,
    /// The evidence/limit note.
    pub note: &'static str,
}

/// The documented verdicts.  Every prefix NOT listed here falls to
/// [`SeamStatus::OutOfScope`] with the generic note (completeness is
/// by construction; the interesting rows are explicit).
fn documented(prefix: &str) -> Option<(SeamStatus, &'static str, &'static str)> {
    Some(match prefix {
        "AddValidMission" => (
            SeamStatus::Served,
            "prototype-engine::missions_load + engine_runtime",
            "the shipped mission chunk calls it 70x; the registry attaches all 70 missions from the real bytes (PROVEN)",
        ),
        "fre" => (
            SeamStatus::Partial,
            "prototype-engine::runtime fre-event registry + script lifecycle",
            "the 4 natives frevents.lua calls (RegisterEvent/RegisterCollectibles/ResetEvent/DestroyEvent) are served (60 events + Reset/Destroy dispatch, PROVEN); the other 72 fre_* natives have no seam",
        ),
        "util" => (
            SeamStatus::Partial,
            "prototype-engine::script_host",
            "util_GetRandomFloat is stubbed deterministically (0.5) for the seed path — the engine RNG is not reconstructed; the other 3 util_* are unserved",
        ),
        "snd" => (
            SeamStatus::Partial,
            "prototype-audio (banks/patches/mixer/buses) + app seams",
            "snd_PlayPatch3D/Overlapped ~ tagged-voice spawn, snd_SetMission/snd_SetFreeRoamMission ~ serve_music_state legs, snd_EnterFEMenu ~ the frontend bed, snd_SetVolume ~ set_audio_bus_gain, snd_StopMusic ~ tagged stop; exact per-native semantics NOT claimed",
        ),
        "fmv" => (
            SeamStatus::Partial,
            "prototype-video + App movie seams",
            "fmv_Skip ~ skip_boot_movie (NIS-first), fmv_Stop ~ the stop paths; there is NO fv_Play in the registry — movies START via nis_SetFMVFileName (the block-71 proof)",
        ),
        "nis" => (
            SeamStatus::Partial,
            "App::play_nis/play_nis_mix/play_nis_with_movie/stop_nis",
            "nis_PlayNIS ~ the proven mix serve, nis_SetFMVFileName's screen half ~ the block-71 picture leg, nis_Stop ~ stop_nis; slot machinery (SetNumRequestedSlots/TakeFMVSlotFromNISSlots) and TeleportNIS unserved",
        ),
        "stream" => (
            SeamStatus::Partial,
            "prototype-stream + the M25 streaming grid",
            "PackageLoad/UnLoad ~ REAL mount/unmount (the full gameplay slice), BeginWorldStreaming ~ camera-ordered boot; slot groups (GetPackageSlotGroup/CombineDynamicSlots) unserved",
        ),
        "stats" => (
            SeamStatus::Unserved,
            "-",
            "the StatsManager content DEF is decoded in the corpus census (29 meta records); no stats system exists",
        ),
        "ach" => (
            SeamStatus::Unserved,
            "-",
            "the AchievementsManager def's 40 meta records name the achievements; no achievements system exists",
        ),
        "opt" => (
            SeamStatus::Unserved,
            "-",
            "the ONLY options native is a GETTER (opt_GetKeyValue) — the options WRITE path is engine-side; the app exposes set_audio_bus_gain as the modern hook",
        ),
        "woi" => (
            SeamStatus::Unserved,
            "-",
            "Web of Intrigue; 0xwoi.rcf is a hash-only FMV-subtitle sidecar (block 66 dead end)",
        ),
        "nism" => (
            SeamStatus::Unserved,
            "-",
            "letterbox/fade presentation managers — no presentation seam",
        ),
        "amb" => (
            SeamStatus::Unserved,
            "-",
            "the ambient PEDESTRIAN/vehicle manager — NOT audio: the name collides with the 'amb' MIXER BUS label in Ambience_all's bus list (different layers)",
        ),
        "cm" => (
            SeamStatus::Unserved,
            "-",
            "camera manager — the renderer has no camera system yet (debug/world mesh draws)",
        ),
        "sc" => (
            SeamStatus::Unserved,
            "-",
            "scripted camera splines — no camera system",
        ),
        "tod" => (
            SeamStatus::Unserved,
            "-",
            "time-of-day — the renderer's daylight fills are the disasm-grounded DESIGN values, no tod system",
        ),
        "sgm" => (
            SeamStatus::Unserved,
            "-",
            "save/load game — no persistence layer",
        ),
        "sm" => (
            SeamStatus::Unserved,
            "-",
            "screen/state flow (GoToFrontEnd/GoToWOI) — the app's screen flow is the boot state machine, not this API",
        ),
        _ => return None,
    })
}

/// Build the full coverage table over the registry's observed
/// prefixes (registration counts descending — the registry's own
/// order).  Complete by construction: the row counts sum to the
/// registry's registration total.
pub fn seam_coverage(reg: &LuaApiRegistry) -> Vec<SeamCoverageRow> {
    reg.prefix_counts()
        .into_iter()
        .map(|(prefix, registrations)| {
            let (status, seam, note) = documented(prefix).unwrap_or((
                SeamStatus::OutOfScope,
                "-",
                "gameplay-simulation/UI/tooling subsystem the loader-style rewrite does not reproduce (and the shipped scripts never call it — see the verdict)",
            ));
            SeamCoverageRow {
                prefix: prefix.to_string(),
                registrations,
                status,
                seam,
                note,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The verdict
// ---------------------------------------------------------------------------

/// The families whose natives would drive the app-level AV seams
/// (audio, movies, cutscenes, options, camera, streaming, FE).
pub const AV_FAMILIES: &[&str] = &[
    "snd", "fmv", "nis", "nism", "opt", "woi", "amb", "cm", "sc", "tod", "sgm", "stream", "fe",
];

/// The survey's headline answer: what would call the seams
/// automatically?
#[derive(Debug, Clone)]
pub struct Verdict {
    /// The union of engine natives the shipped scripts import
    /// (sorted) — six names.
    pub unique_engine_hosts: Vec<String>,
    /// Per host: its family prefix and that family's seam status.
    pub host_family_status: Vec<(String, String, SeamStatus)>,
    /// Shipped-script hosts whose family is an AV family — pinned
    /// EMPTY by test: no script in the shipped corpus calls the
    /// audio/movie/options/streaming/FE surface.
    pub av_callers_in_scripts: Vec<String>,
}

/// Cross-reference the corpus census against the registry.
pub fn interpreter_verdict(census: &CorpusCensus, reg: &LuaApiRegistry) -> Verdict {
    let hosts = census.shipped_engine_hosts();
    let mut host_family_status = Vec::new();
    let mut av_callers = Vec::new();
    for host in &hosts {
        let prefix = LuaApiRegistry::prefix_of(host).to_string();
        let status = seam_status_of(reg, &prefix);
        if AV_FAMILIES.contains(&prefix.as_str()) {
            av_callers.push(host.clone());
        }
        host_family_status.push((host.clone(), prefix, status));
    }
    Verdict {
        unique_engine_hosts: hosts,
        host_family_status,
        av_callers_in_scripts: av_callers,
    }
}

fn seam_status_of(reg: &LuaApiRegistry, prefix: &str) -> SeamStatus {
    let n = reg.group(prefix).count();
    if n == 0 {
        // Not a registry family at all (e.g. AddValidMission is its
        // own prefix and IS registered under that exact name).
        if reg.lookup(prefix).is_some() {
            return documented(prefix).map(|(s, _, _)| s).unwrap_or(SeamStatus::Unserved);
        }
        return SeamStatus::Unserved;
    }
    documented(prefix).map(|(s, _, _)| s).unwrap_or(SeamStatus::OutOfScope)
}

// ---------------------------------------------------------------------------
// Tests — every number pinned on the REAL committed bytes
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn scripts_rcf() -> Vec<u8> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join("scripts.rcf");
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    const MISSION_HASH: u32 = 0x0ffd75fa;
    const FREVENTS_HASH: u32 = 0x34e40dc3;

    #[test]
    fn corpus_census_pins_the_real_scripts_rcf() {
        let census = survey_scripts_rcf(&scripts_rcf()).unwrap();
        assert_eq!(census.members.len(), 7, "the archive has exactly 7 members");

        let lua: Vec<&CorpusMember> = census
            .members
            .iter()
            .filter(|m| m.kind == MemberKind::Lua51)
            .collect();
        assert_eq!(lua.len(), 2, "exactly two compiled Lua members");
        let mut lua_sizes: Vec<u32> = lua.iter().map(|m| m.size).collect();
        lua_sizes.sort_unstable();
        assert_eq!(lua_sizes, vec![2_318, 4_383], "mission chunk + frevents.lua");
        let by_hash = |h: u32| census.members.iter().find(|m| m.name_hash == h).unwrap();
        assert_eq!(by_hash(MISSION_HASH).size, 2_318, "the mission chunk (block-39 marker selector)");
        assert_eq!(by_hash(FREVENTS_HASH).size, 4_383, "frevents.lua");

        let text: Vec<&CorpusMember> = census
            .members
            .iter()
            .filter(|m| m.kind == MemberKind::Text)
            .collect();
        assert_eq!(text.len(), 4, "the four localized credits rolls");
        for t in &text {
            assert_eq!(t.head_line.as_deref(), Some("EXECUTIVE PRODUCER"));
            assert!(t.zlib_marker.is_some(), "every roll embeds the zlib credit");
        }
        // Two distinct EN-ish rolls byte-identical; two IT-ish rolls
        // distinct from everything (sizes 39 686 x3 + 39 687 x1).
        let mut text_sizes: Vec<u32> = text.iter().map(|m| m.size).collect();
        text_sizes.sort_unstable();
        assert_eq!(text_sizes, vec![39_686, 39_686, 39_686, 39_687]);
        assert_eq!(census.byte_identical_groups.len(), 1, "exactly one duplicate pair");
        let dup = &census.byte_identical_groups[0];
        assert_eq!(dup.len(), 2);
        let en_markers = text
            .iter()
            .filter(|m| dup.contains(&m.name_hash))
            .filter(|m| m.zlib_marker.as_deref().unwrap_or("").starts_with("zlib version "))
            .count();
        assert_eq!(en_markers, 2, "the duplicated pair is the EN-tailed rolls");

        let p3d: Vec<&CorpusMember> = census
            .members
            .iter()
            .filter(|m| m.kind == MemberKind::P3d)
            .collect();
        assert_eq!(p3d.len(), 1);
        assert_eq!(p3d[0].size, 8_683);
        // The DLL's statsachievements pair: StatsManager + AchievementsManager.
        assert!(p3d[0]
            .content_names
            .iter()
            .any(|n| n.starts_with("proto::StatsManager")));
        assert!(p3d[0]
            .content_names
            .iter()
            .any(|n| n.starts_with("proto::AchievementsManager")));
        assert_eq!(p3d[0].meta_counts, vec![29, 40], "29 stat labels + 40 achievement names");
    }

    #[test]
    fn shipped_lua_calls_exactly_six_engine_natives() {
        let census = survey_scripts_rcf(&scripts_rcf()).unwrap();
        assert_eq!(
            census.shipped_engine_hosts(),
            vec![
                "AddValidMission",
                "fre_DestroyEvent",
                "fre_RegisterCollectibles",
                "fre_RegisterEvent",
                "fre_ResetEvent",
                "util_GetRandomFloat",
            ],
            "the whole shipped corpus imports exactly these six engine natives"
        );
        // Every import resolves (no unresolved imports anywhere).
        for s in census.scripts() {
            assert!(s.unresolved_imports.is_empty(), "unresolved: {:?}", s.unresolved_imports);
        }
        // The call counts on the real bytes.
        assert_eq!(census.host_call_total("AddValidMission"), 70);
        assert_eq!(census.host_call_total("fre_RegisterEvent"), 60);
        assert_eq!(census.host_call_total("fre_RegisterCollectibles"), 1);
        assert_eq!(census.host_call_total("math.randomseed"), 1);
        // The frevents lifecycle closures are exported.
        let frevents = census
            .members
            .iter()
            .find(|m| m.name_hash == FREVENTS_HASH)
            .unwrap()
            .script
            .as_ref()
            .unwrap();
        assert!(frevents.closure_exports.contains(&"ResetFreeRoamEvents".to_string()));
        assert!(frevents.closure_exports.contains(&"DestroyFreeRoamEvents".to_string()));
        // The mission chunk defines NO closures and exports nothing.
        let mission = census
            .members
            .iter()
            .find(|m| m.name_hash == MISSION_HASH)
            .unwrap()
            .script
            .as_ref()
            .unwrap();
        assert!(mission.exports.is_empty());
        assert!(mission.closure_exports.is_empty());
    }

    #[test]
    fn verdict_no_script_calls_the_av_surface() {
        let census = survey_scripts_rcf(&scripts_rcf()).unwrap();
        let reg = LuaApiRegistry::new();
        let v = interpreter_verdict(&census, &reg);
        assert_eq!(v.unique_engine_hosts.len(), 6);
        assert!(
            v.av_callers_in_scripts.is_empty(),
            "no shipped script may call snd_*/fmv_*/nis_*/opt_*/stream_*/fe_*: {:?}",
            v.av_callers_in_scripts
        );
        // Every host resolves to a registry record (the M7<->M8
        // closure, re-proved from the survey path).
        for host in &v.unique_engine_hosts {
            assert!(reg.lookup(host).is_some(), "{host} must be registered");
        }
        // Family statuses: the mission native is SERVED; the
        // fre_* hosts ride the PARTIAL fre family; the RNG stub is
        // PARTIAL util.
        for (host, family, status) in &v.host_family_status {
            match host.as_str() {
                "AddValidMission" => {
                    assert_eq!(family, "AddValidMission");
                    assert_eq!(*status, SeamStatus::Served);
                }
                h if h.starts_with("fre_") => {
                    assert_eq!(family, "fre");
                    assert_eq!(*status, SeamStatus::Partial);
                }
                "util_GetRandomFloat" => {
                    assert_eq!(family, "util");
                    assert_eq!(*status, SeamStatus::Partial);
                }
                other => panic!("unexpected host {other}"),
            }
        }
    }

    #[test]
    fn coverage_table_covers_every_registration() {
        let reg = LuaApiRegistry::new();
        let rows = seam_coverage(&reg);
        let total: usize = rows.iter().map(|r| r.registrations).sum();
        assert_eq!(total, reg.registration_count(), "every registration lands in one row");
        assert_eq!(total, 1_119, "the M7 registry total");
        assert_eq!(rows.len(), reg.prefix_counts().len(), "one row per observed prefix");

        let by_prefix = |p: &str| rows.iter().find(|r| r.prefix == p).unwrap();
        // SERVED: the mission registry.
        assert_eq!(by_prefix("AddValidMission").status, SeamStatus::Served);
        // PARTIAL: the families with real seams.
        for (p, n) in [("fre", 76), ("snd", 29), ("fmv", 4), ("nis", 17), ("stream", 18), ("util", 4)] {
            let row = by_prefix(p);
            assert_eq!(row.status, SeamStatus::Partial, "{p}");
            assert_eq!(row.registrations, n, "{p} registration count");
            assert_ne!(row.seam, "-", "{p} must name its seam");
        }
        // UNSERVED: named families with no seam (with specific notes).
        for (p, n) in [("opt", 1), ("woi", 26), ("nism", 8), ("cm", 26), ("sc", 14), ("tod", 8), ("sgm", 7), ("sm", 4)] {
            let row = by_prefix(p);
            assert_eq!(row.status, SeamStatus::Unserved, "{p}");
            assert_eq!(row.registrations, n, "{p} registration count");
            assert_eq!(row.seam, "-");
        }
        // The amb name-collision note is pinned (pedestrians, not audio).
        let amb = by_prefix("amb");
        assert_eq!(amb.status, SeamStatus::Unserved);
        assert!(amb.note.contains("NOT audio"));
        // The gameplay-simulation families fall to OutOfScope.
        for p in ["ai", "go", "fsm", "spn", "disguise", "fe"] {
            assert_eq!(by_prefix(p).status, SeamStatus::OutOfScope, "{p}");
        }
        // The ai family is the largest single block of natives.
        assert_eq!(by_prefix("ai").registrations, 258);
        assert_eq!(by_prefix("go").registrations, 157);
        assert_eq!(by_prefix("fe").registrations, 143);
    }

    #[test]
    fn seam_relevant_native_names_are_pinned() {
        // The API-name evidence the module docs cite, locked to the
        // registry's real table.
        let reg = LuaApiRegistry::new();
        let mut fmv: Vec<&str> = reg.group("fmv").map(|r| r.name).collect();
        fmv.sort_unstable();
        assert_eq!(fmv, vec!["fmv_IsLoaded", "fmv_IsPlaying", "fmv_Skip", "fmv_Stop"]);

        let nis: Vec<&str> = reg.group("nis").map(|r| r.name).collect();
        for name in ["nis_PlayNIS", "nis_SetFMVFileName", "nis_Stop", "nis_LoadNIS"] {
            assert!(nis.contains(&name), "{name} must be in the nis family");
        }
        assert_eq!(nis.len(), 17);

        let snd: Vec<&str> = reg.group("snd").map(|r| r.name).collect();
        for name in [
            "snd_PlayPatch3D",
            "snd_PlayPatchOverlapped",
            "snd_SetMission",
            "snd_SetFreeRoamMission",
            "snd_EnterFEMenu",
            "snd_SetVolume",
            "snd_StopMusic",
        ] {
            assert!(snd.contains(&name), "{name} must be in the snd family");
        }
        assert_eq!(snd.len(), 29);

        let mut opt: Vec<&str> = reg.group("opt").map(|r| r.name).collect();
        assert_eq!(opt.len(), 1);
        assert_eq!(opt.pop().unwrap(), "opt_GetKeyValue");
    }
}

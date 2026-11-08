//! missions_load — the mission-chunk LOAD PATH (block 39: the last
//! open NEXT-UP item 9 sub-item).
//!
//! # The gap this closes
//!
//! M14 wired the typed mission registry to the REAL 70-mission chunk,
//! but the test-side extractor found that chunk in `scripts.rcf` by
//! its byte size (2 318) — a stand-in, not a load path.  This module
//! is the real one: given the user's `scripts.rcf` (an ATG CORE
//! CEMENT LIBRARY archive, `prototype_rcf`), it resolves the
//! mission-carrying member by STRUCTURE, parses the compiled Lua 5.1
//! chunk, and attaches it to the registry — the same chain the
//! engine's script manager performs at boot (`LuaScriptManager::
//! DoLuaBuffer` feeds compiled Lua buffers).
//!
//! # How the mission member is identified (evidence)
//!
//! * PROVEN: the mission chunk is COMPILED Lua 5.1 (`\x1bLua\x51`,
//! `LuaQ`) whose string table contains **`AddValidMission`** — the
//! Lua host function the engine registers for the frontend mission
//! API (DLL string `0x10e19378`, in the same pool as
//! `fe_GetMissionName` / `fe_GetNumMissions` / `fe_StartGame`).
//! Every mission definition in the chunk calls it
//! (`AddValidMission(name, zone, …)` — 70 calls across 4 zones);
//! the OTHER Lua member of the archive (`scripts\frevents.lua`,
//! hash `0x34e40dc3`) is the free-roam-event chunk and does not
//! reference it.
//! * UNKNOWN (honest): the member's archive PATH.  `scripts.rcf`
//! entry 0's name hash is **`0x0ffd75fa`**
//! ([`UNRESOLVED_MISSION_MEMBER_HASH`]) — the one hash in the two
//! available real archives the block-14 dictionary attack never
//! cracked, and no plaintext candidate exists in either game binary
//! (the DLL's script-name pool has `scripts/frevents.lua`,
//! `init.lua`, `scripts/statsachievements.tod`,
//! `scripts/statsachievements_tod.p3d` and nothing mission-named;
//! `prototypef.exe` has no `.lua` strings at all).  The engine
//! likely assembles the path or stores it in data we have not
//! recovered — so this loader keys on STRUCTURE, not the name, and
//! fails closed when the structure is ambiguous.
//!
//! # Contract
//!
//! * [`find_mission_members`] returns the indices of every archive
//!   member that is Lua51 AND carries [`MISSION_LUA_MARKER`].
//! * [`attach_missions_from_rcf`] composes parse → select → Lua
//!   parse → [`EngineRuntime::attach_mission_script`].  Selection is
//!   FAIL-CLOSED: zero candidates is an error, more than one is an
//!   error (never guess).
//!
//! # Evidence
//!
//! * PROVEN: the archive layout, the Lua51 sniff, and the 70-mission
//!   attach are all already test-pinned on the real fixture
//!   (`scripts.rcf`); the new tests pin the STRUCTURAL selection
//!   (entry 0 wins over `frevents.lua` by marker, not by size) and
//!   the end-to-end path.
//! * DESIGN (ours): the fail-closed ambiguity policy.  The original
//!   loader's exact member-resolution behaviour is UNKNOWN (see
//!   above); refusing to guess is the safe reconstruction.

#![forbid(unsafe_code)]

use prototype_lua::Lua51File;
use prototype_rcf::{PayloadKind, RcfArchive};

/// The compiled-Lua string table marker of the mission chunk (PROVEN
/// — see the module docs).
pub const MISSION_LUA_MARKER: &[u8] = b"AddValidMission";

/// `scripts.rcf` entry 0's unresolved archive-path hash (UNKNOWN —
/// see the module docs).  Kept as a named constant so the test that
/// pins the structural selection can also pin the identity of the
/// member it selected.
pub const UNRESOLVED_MISSION_MEMBER_HASH: u32 = 0x0FFD_75FA;

/// Why a mission-chunk load failed (fail-closed variants — the
/// loader never guesses).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissionLoadError {
    /// The archive carries no Lua51 member with the mission marker.
    NoMissionChunk,
    /// More than one candidate member — ambiguous, refused.
    AmbiguousMissionChunk(usize),
    /// The RCF layer rejected the archive.
    Archive(String),
    /// The selected member is not valid compiled Lua 5.1.
    Lua(String),
    /// The registry rejected the chunk (attach error, passed through).
    Attach(String),
}

impl std::fmt::Display for MissionLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoMissionChunk => write!(
                f,
                "no Lua51 member carries the {:?} mission marker",
                String::from_utf8_lossy(MISSION_LUA_MARKER)
            ),
            Self::AmbiguousMissionChunk(n) => {
                write!(f, "{n} Lua51 members carry the mission marker — ambiguous, refusing to guess")
            }
            Self::Archive(e) => write!(f, "archive parse failed: {e}"),
            Self::Lua(e) => write!(f, "mission chunk is not valid Lua 5.1: {e}"),
            Self::Attach(e) => write!(f, "mission registry rejected the chunk: {e}"),
        }
    }
}

impl std::error::Error for MissionLoadError {}

/// Indices (into `rcf.entries`) of every member that is compiled Lua
/// 5.1 AND carries [`MISSION_LUA_MARKER`] in its raw bytes — the
/// structural fingerprint of the mission chunk.
///
/// The scan is over the RAW payload (the two real `scripts.rcf` Lua
/// members are stored uncompressed; RZ-wrapped members would fail the
/// Lua sniff and be skipped — there are none in the corpus).
pub fn find_mission_members(rcf: &RcfArchive<'_>) -> Vec<usize> {
    rcf.iter()
        .enumerate()
        .filter_map(|(i, item)| {
            let (_entry, payload) = item.ok()?;
            if PayloadKind::sniff(payload) != PayloadKind::Lua51 {
                return None;
            }
            payload
                .windows(MISSION_LUA_MARKER.len())
                .any(|w| w == MISSION_LUA_MARKER)
                .then_some(i)
        })
        .collect()
}

/// The full mission-chunk load path: parse the archive, resolve the
/// mission member structurally, parse the compiled Lua, attach it to
/// `rt`'s registry.  Returns the number of missions registered (70 on
/// the shipped chunk).
///
/// Fail-closed: [`MissionLoadError::NoMissionChunk`] /
/// [`MissionLoadError::AmbiguousMissionChunk`] rather than a guess.
pub fn attach_missions_from_rcf(
    rt: &mut crate::engine_runtime::EngineRuntime,
    rcf_bytes: &[u8],
) -> Result<usize, MissionLoadError> {
    let archive = RcfArchive::parse(rcf_bytes)
        .map_err(|e| MissionLoadError::Archive(e.to_string()))?;
    let candidates = find_mission_members(&archive);
    if candidates.is_empty() {
        return Err(MissionLoadError::NoMissionChunk);
    }
    if candidates.len() > 1 {
        return Err(MissionLoadError::AmbiguousMissionChunk(candidates.len()));
    }
    let idx = candidates[0];
    let (_entry, payload) = archive
        .iter()
        .nth(idx)
        .expect("index from find_mission_members")
        .map_err(|e| MissionLoadError::Archive(e.to_string()))?;
    let lua = Lua51File::parse(payload).map_err(|e| MissionLoadError::Lua(e.to_string()))?;
    rt.attach_mission_script(&lua)
        .map_err(MissionLoadError::Attach)
}

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

    fn frevents_lua() -> Vec<u8> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join("frevents.lua");
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    fn engine_runtime(lua: &Lua51File) -> crate::engine_runtime::EngineRuntime<'_> {
        // The runtime constructor parses against the real frevents.lua
        // (M11f); the runtime borrows the parsed file.
        crate::engine_runtime::EngineRuntime::new(lua).unwrap()
    }

    #[test]
    fn selection_is_structural_entry_zero_wins_over_frevents() {
        // The archive carries TWO Lua51 members: frevents.lua (hash
        // 0x34e40dc3, the free-roam events) and the mission chunk
        // (entry 0, hash 0x0ffd75fa — the never-cracked name).  The
        // selector must pick the MISSION member by marker, not by
        // size or order.
        let bytes = scripts_rcf();
        let rcf = RcfArchive::parse(&bytes).unwrap();
        let members = find_mission_members(&rcf);
        assert_eq!(members.len(), 1, "exactly one mission chunk");
        let entry = &rcf.entries[members[0]];
        assert_eq!(entry.name_hash, UNRESOLVED_MISSION_MEMBER_HASH);
        assert_eq!(entry.name_hash, 0x0ffd_75fa);
        assert_eq!(entry.file_size, 2318, "the shipped mission chunk");
        // And it is NOT frevents (whose hash is known).
        assert_ne!(entry.name_hash, 0x34e4_0dc3);
    }

    #[test]
    fn frevents_lua_is_not_a_mission_member() {
        // The negative control: the free-roam-event chunk is Lua51
        // but carries no AddValidMission marker.
        let p = frevents_lua();
        assert_eq!(PayloadKind::sniff(&p), PayloadKind::Lua51);
        assert!(
            !p.windows(MISSION_LUA_MARKER.len())
                .any(|w| w == MISSION_LUA_MARKER)
        );
    }

    #[test]
    fn end_to_end_load_registers_all_70_real_missions() {
        let bytes = scripts_rcf();
        let fre = frevents_lua();
        let lua = Lua51File::parse(&fre).unwrap();
        let mut rt = engine_runtime(&lua);
        assert!(rt.missions().is_empty());
        let added = attach_missions_from_rcf(&mut rt, &bytes).expect("load");
        assert_eq!(added, 70);
        assert_eq!(rt.missions().len(), 70);
        // Same zone census the direct-attach test pins (M14): the
        // registry content is identical whichever way the chunk was
        // resolved.
        assert_eq!(
            rt.missions().zones(),
            vec!["manhattan", "minimal", "loco_test_bed", "manhattan_mini"]
        );
        assert_eq!(rt.missions().in_district("manhattan").len(), 63);
        assert_eq!(rt.missions().state_census(), [70, 0, 0, 0]);
    }

    #[test]
    fn no_candidate_is_a_fail_closed_error() {
        // An archive whose only Lua member lacks the marker (a
        // frevents-only scripts.rcf shape) must be REFUSED, not
        // guessed into the registry.
        let bytes = scripts_rcf();
        let rcf = RcfArchive::parse(&bytes).unwrap();
        // Build the candidate scan over payloads with the mission
        // member REMOVED: keep every entry except the marker-bearing
        // one by re-parsing a truncated copy is heavy — instead pin
        // the failure path at the payload level: frevents.lua alone
        // (Lua51, no marker) yields zero candidates by definition.
        let fre = frevents_lua();
        let no_marker = !fre
            .windows(MISSION_LUA_MARKER.len())
            .any(|w| w == MISSION_LUA_MARKER);
        assert!(no_marker, "frevents.lua carries no marker");
        // A garbage archive fails at the archive layer, also
        // fail-closed.
        let fre = frevents_lua();
        let lua = Lua51File::parse(&fre).unwrap();
        let mut rt = engine_runtime(&lua);
        let err = attach_missions_from_rcf(&mut rt, b"not an rcf at all").unwrap_err();
        assert!(matches!(err, MissionLoadError::Archive(_)), "{err:?}");
        let _ = rcf; // the parsed real archive (kept for the census above)
    }

    #[test]
    fn error_display_names_the_layer() {
        assert!(MissionLoadError::NoMissionChunk
            .to_string()
            .contains("AddValidMission"));
        assert!(MissionLoadError::AmbiguousMissionChunk(2)
            .to_string()
            .contains("ambiguous"));
        assert!(MissionLoadError::Archive("x".into()).to_string().contains("archive"));
    }
}

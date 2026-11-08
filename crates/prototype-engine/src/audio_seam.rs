//! audio_seam — the `snd_SetMission` / `snd_SetFreeRoamMission`
//! call-through (block 74).
//!
//! ## What this closes
//!
//! Block 73's survey PROVED two things at once: the shipped script
//! corpus (`scripts.rcf`, the whole compiled-Lua corpus the game
//! carries) NEVER calls the `snd` native family (the AV intersection
//! is pinned-empty), and yet the engine's M7 native registry really
//! carries the two snd natives that name the music-state surface:
//! `snd_SetMission` and `snd_SetFreeRoamMission` (both in the
//! NamedMap registrar 0x1049ec60 — `lua_api_data.rs` lines 613-614).
//! Block 73's verdict: those seams' historical drivers are the
//! engine's OWN C++ subsystems (the mission FSM), calling through
//! the same registered-native surface a script would use.
//!
//! This module makes that surface REAL on our side, end to end:
//!
//! * [`install`] registers both natives as host fns on a
//!   [`prototype_lua::vm::Vm`] — the SAME host mechanism the four
//!   `fre_*` natives use, and (via [`crate::runtime::ScriptedFreeRoam`])
//!   the SAME host that runs the real `frevents.lua` corpus.
//! * The natives are RECORD-ONLY: every call lands in the VM's
//!   host-call audit (`Vm::host_calls`), exactly how the fre natives
//!   were first proven (M8).  Raw evidence is preserved verbatim;
//!   there is exactly ONE validation point.
//! * [`AudioSeamRequest::from_host_calls`] maps the audited calls
//!   into typed requests, FAIL-CLOSED on a bad argument (the fre
//!   hosts' documented pattern): a `snd_SetMission` whose first
//!   argument is missing or not a string is an `Err`, never a
//!   silent guess.
//! * The consumer side lives in the app (`prototype-android`
//!   `App::apply_audio_seam`): each typed request drives the REAL
//!   serve paths — `snd_SetMission(m)` → the mission MUSIC leg
//!   (`enter_mission` → `serve_music_state`), `snd_SetFreeRoamMission`
//!   → the WORLD-BED leg (`enter_free_roam` → the ambience bank's
//!   PROVEN bed).  Block 70's legs, block 72's buses, one driver.
//!
//! ## Evidence and honest scope
//!
//! * PROVEN (registry): both native names, their registrar and
//!   mechanism (`lua_api_data::HOST_FN_RECORDS`, the NamedMap rows).
//! * PROVEN (absence): NO call to either native exists anywhere in
//!   the shipped corpus (block 73, pinned by test).  Re-pinned here
//!   at the typed level: a real `ScriptedFreeRoam` over the real
//!   `frevents.lua` yields ZERO audio-seam requests.
//! * UNKNOWN (and not invented): the exact native signatures.  No
//!   call site exists to read.  The argument rules below are DESIGN,
//!   documented and fail-closed:
//!   - `snd_SetMission(mission)` — exactly ONE string argument (a
//!     mission-less "set mission" is meaningless; missing or
//!     non-string → `Err`).
//!   - `snd_SetFreeRoamMission([mission])` — ZERO or ONE string
//!     argument (free-roam entry plausibly carries no mission id;
//!     `nil` counts as absent; a non-string non-nil → `Err`).
//! * DESIGN: the request mapping (which native drives which serve
//!   leg) follows the NAMES — `SetMission` = the mission music
//!   channel, `SetFreeRoamMission` = the free-roam world.  The
//!   block-72 bus routing (music leg → `music`, world bed → `amb`)
//!   applies unchanged because the driver goes through the SAME
//!   serve paths, not around them.

use prototype_common::Result;
use prototype_lua::vm::{HostCall, Vm, Value};

/// The M7 registry name of the mission-music native (PROVEN,
/// `lua_api_data.rs` line 613, NamedMap registrar 0x1049ec60).
pub const SND_SET_MISSION: &str = "snd_SetMission";

/// The M7 registry name of the free-roam-music native (PROVEN,
/// `lua_api_data.rs` line 614, the same registrar).
pub const SND_SET_FREE_ROAM_MISSION: &str = "snd_SetFreeRoamMission";

/// One audio request recovered from the registered native surface.
///
/// The engine-side typed form of a `snd_SetMission` /
/// `snd_SetFreeRoamMission` host call; the app maps these onto the
/// real serve legs (`App::apply_audio_seam`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioSeamRequest {
    /// `snd_SetMission(<mission>)` — the MISSION MUSIC leg: the
    /// named mission's cue chain becomes the music channel (the app
    /// drives `enter_mission`, the block-68/72 path).
    SetMission {
        /// The mission id the call carried (e.g. `"e02m02"`).
        mission: String,
    },
    /// `snd_SetFreeRoamMission([mission])` — the WORLD-BED leg: the
    /// free-roam state takes over (the app drives `enter_free_roam`,
    /// the block-70 path — the ambience bank's PROVEN world bed).
    SetFreeRoamMission {
        /// The mission id the call carried, when any (diagnostic —
        /// the world bed resolves from the ambience bank, not from
        /// a mission cue; DESIGN, documented).
        mission: Option<String>,
    },
}

impl AudioSeamRequest {
    /// The native name that produces this request.
    pub fn native_name(&self) -> &'static str {
        match self {
            AudioSeamRequest::SetMission { .. } => SND_SET_MISSION,
            AudioSeamRequest::SetFreeRoamMission { .. } => SND_SET_FREE_ROAM_MISSION,
        }
    }

    /// The mission id the call carried (`None` when absent).
    pub fn mission(&self) -> Option<&str> {
        match self {
            AudioSeamRequest::SetMission { mission } => Some(mission),
            AudioSeamRequest::SetFreeRoamMission { mission } => mission.as_deref(),
        }
    }

    /// Map ONE audited host call.  Foreign names → `Ok(None)` (the
    /// mapping walks a mixed audit log); the two snd names → the
    /// typed request, FAIL-CLOSED on a bad argument.
    fn from_call(name: &str, args: &[Value]) -> std::result::Result<Option<Self>, String> {
        // The shared string rule: a leading string argument (or, for
        // the optional form, nil/absent) decodes lossily-relevant —
        // mission ids in the corpus are ASCII (`e02m02`, `freKill`).
        let first_str = |args: &[Value]| -> Option<Option<String>> {
            match args.first() {
                None => Some(None),
                Some(Value::Nil) => Some(None),
                Some(Value::Str(s)) => Some(Some(String::from_utf8_lossy(s).into_owned())),
                Some(_) => None, // a non-string non-nil argument
            }
        };
        match name {
            SND_SET_MISSION => match first_str(args) {
                Some(Some(mission)) => Ok(Some(AudioSeamRequest::SetMission { mission })),
                Some(None) => Err(format!(
                    "{SND_SET_MISSION}: mission string expected, got {} argument(s)",
                    args.len()
                )),
                None => Err(format!(
                    "{SND_SET_MISSION}: mission string expected, got a non-string value"
                )),
            },
            SND_SET_FREE_ROAM_MISSION => match first_str(args) {
                Some(mission) => Ok(Some(AudioSeamRequest::SetFreeRoamMission { mission })),
                None => Err(format!(
                    "{SND_SET_FREE_ROAM_MISSION}: optional mission string expected, got a \
                     non-string value"
                )),
            },
            _ => Ok(None),
        }
    }

    /// Map the recorded host calls (a MIXED audit log — library
    /// calls and other natives ride along) into typed requests, in
    /// call order.  Fail-closed: the first bad snd argument aborts
    /// the whole mapping with a diagnostic naming the call.  An
    /// empty (or snd-free) log maps to an empty vec.
    pub fn from_host_calls(calls: &[HostCall]) -> std::result::Result<Vec<Self>, String> {
        let mut out = Vec::new();
        for (i, call) in calls.iter().enumerate() {
            if call.name != SND_SET_MISSION && call.name != SND_SET_FREE_ROAM_MISSION {
                continue;
            }
            match Self::from_call(&call.name, &call.args) {
                Ok(Some(req)) => out.push(req),
                Ok(None) => unreachable!("snd names are handled above"),
                Err(e) => return Err(format!("host call #{i}: {e}")),
            }
        }
        Ok(out)
    }
}

/// Register BOTH snd natives on `vm` as RECORD-ONLY host fns (the
/// same mechanism the `fre_*` natives use).  Every call lands in the
/// VM's host-call audit verbatim — raw evidence preserved, ONE
/// validation point ([`AudioSeamRequest::from_host_calls`]).  The
/// registered names are the M7 registry's own (PROVEN); the
/// record-only return mirrors how the fre natives were first proven
/// (M8) before they grew behaviour.
pub fn install(vm: &mut Vm<'_>) -> Result<()> {
    vm.register_host(SND_SET_MISSION, |_vm, _args| Ok(Vec::new()))?;
    vm.register_host(SND_SET_FREE_ROAM_MISSION, |_vm, _args| Ok(Vec::new()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::ScriptedFreeRoam;
    use prototype_lua::Lua51File;
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

    fn lua_str(s: &str) -> Value {
        Value::Str(std::rc::Rc::from(s.as_bytes()))
    }

    /// A fresh VM over the REAL frevents.lua corpus with the seam
    /// installed — the same host shape ScriptedFreeRoam builds.
    fn seam_vm() -> Vm<'static> {
        // Leak the fixture so the Vm can borrow it for the test's
        // lifetime (tests are short-lived; the corpus is 4.3 KB).
        let data: &'static [u8] = Box::leak(fixture("frevents.lua").into_boxed_slice());
        let file: &'static Lua51File = Box::leak(Box::new(
            Lua51File::parse(data).expect("the real frevents.lua parses"),
        ));
        let mut vm = Vm::new(file).expect("the VM constructs");
        install(&mut vm).expect("the seam installs");
        vm
    }

    fn count(vm: &Vm<'_>, name: &str) -> usize {
        vm.host_calls.iter().filter(|c| c.name == name).count()
    }

    #[test]
    fn install_registers_both_registry_names_callable_through_the_vm() {
        let mut vm = seam_vm();
        // Drive BOTH natives through the VM exactly the way a caller
        // (script or the engine's own C++ subsystems) would — via
        // the global function values the registration installed.
        vm.call_global(SND_SET_MISSION, &[lua_str("e02m02")])
            .expect("snd_SetMission is callable");
        vm.call_global(SND_SET_FREE_ROAM_MISSION, &[lua_str("freKill")])
            .expect("snd_SetFreeRoamMission is callable");
        assert_eq!(count(&vm, SND_SET_MISSION), 1);
        assert_eq!(count(&vm, SND_SET_FREE_ROAM_MISSION), 1);
        // The typed mapping, over the MIXED audit log, in order.
        let reqs = AudioSeamRequest::from_host_calls(&vm.host_calls).expect("args are valid");
        assert_eq!(
            reqs,
            vec![
                AudioSeamRequest::SetMission {
                    mission: "e02m02".into()
                },
                AudioSeamRequest::SetFreeRoamMission {
                    mission: Some("freKill".into())
                },
            ]
        );
        assert_eq!(reqs[0].native_name(), SND_SET_MISSION);
        assert_eq!(reqs[0].mission(), Some("e02m02"));
        assert_eq!(reqs[1].mission(), Some("freKill"));
    }

    #[test]
    fn snd_set_mission_args_are_fail_closed() {
        // Missing argument → Err at the mapping (the call itself
        // records — raw evidence first).
        let mut vm = seam_vm();
        vm.call_global(SND_SET_MISSION, &[]).expect("records fine");
        let e = AudioSeamRequest::from_host_calls(&vm.host_calls).unwrap_err();
        assert!(e.contains(SND_SET_MISSION), "{e}");
        assert!(e.contains("mission string expected"), "{e}");

        // A non-string argument → Err, same shape.
        let mut vm = seam_vm();
        vm.call_global(SND_SET_MISSION, &[Value::Num(7.0)])
            .expect("records fine");
        let e = AudioSeamRequest::from_host_calls(&vm.host_calls).unwrap_err();
        assert!(e.contains("non-string"), "{e}");
    }

    #[test]
    fn snd_set_free_roam_mission_argument_is_optional() {
        let mut vm = seam_vm();
        vm.call_global(SND_SET_FREE_ROAM_MISSION, &[])
            .expect("no-arg call is legal");
        vm.call_global(SND_SET_FREE_ROAM_MISSION, &[Value::Nil])
            .expect("nil counts as absent");
        vm.call_global(SND_SET_FREE_ROAM_MISSION, &[lua_str("freColl")])
            .expect("string decodes");
        let reqs = AudioSeamRequest::from_host_calls(&vm.host_calls).expect("all valid");
        assert_eq!(reqs.len(), 3);
        assert_eq!(
            reqs[0],
            AudioSeamRequest::SetFreeRoamMission { mission: None }
        );
        assert_eq!(
            reqs[1],
            AudioSeamRequest::SetFreeRoamMission { mission: None }
        );
        assert_eq!(reqs[2].mission(), Some("freColl"));

        // A non-string non-nil argument stays fail-closed.
        let mut vm = seam_vm();
        vm.call_global(SND_SET_FREE_ROAM_MISSION, &[Value::Bool(true)])
            .expect("records fine");
        assert!(AudioSeamRequest::from_host_calls(&vm.host_calls).is_err());
    }

    #[test]
    fn foreign_calls_in_the_audit_log_are_ignored() {
        let mut vm = seam_vm();
        // Library + foreign-native noise around the two real calls.
        vm.call_global("print", &[lua_str("noise")]).expect("print");
        vm.call_global(SND_SET_FREE_ROAM_MISSION, &[lua_str("freGlide")])
            .expect("call 1");
        vm.call_global("print", &[Value::Num(1.0)]).expect("print");
        vm.call_global(SND_SET_MISSION, &[lua_str("e10m04")])
            .expect("call 2");
        let reqs = AudioSeamRequest::from_host_calls(&vm.host_calls).expect("all valid");
        assert_eq!(reqs.len(), 2, "only the snd calls map");
        assert_eq!(reqs[0].mission(), Some("freGlide"));
        assert_eq!(reqs[1].mission(), Some("e10m04"));
    }

    #[test]
    fn the_real_corpus_still_never_calls_the_natives() {
        // Block 73's pinned-empty AV intersection, re-pinned at the
        // TYPED level: the real frevents.lua runs on a host that now
        // carries the snd natives — and produces ZERO requests.
        let data = fixture("frevents.lua");
        let f = Lua51File::parse(&data).unwrap();
        let fre = ScriptedFreeRoam::new(&f).expect("the corpus runs");
        let reqs = fre.audio_seam_requests().expect("no bad args exist");
        assert!(
            reqs.is_empty(),
            "the shipped corpus must not drive the audio seam"
        );
        assert_eq!(fre.host_call_count(SND_SET_MISSION), 0);
        assert_eq!(fre.host_call_count(SND_SET_FREE_ROAM_MISSION), 0);
        // The registration did not disturb the corpus flow: 60
        // events still register.
        assert_eq!(fre.system().events().len(), 60);
    }

    #[test]
    fn script_free_roam_drives_the_natives_through_the_vm() {
        // The end-to-end engine-side driver: the SAME host that ran
        // the real corpus accepts a native call THROUGH the VM and
        // surfaces the typed request.
        let data = fixture("frevents.lua");
        let f = Lua51File::parse(&data).unwrap();
        let mut fre = ScriptedFreeRoam::new(&f).expect("the corpus runs");
        fre.call_global(SND_SET_MISSION, &[lua_str("e01m01")])
            .expect("the native is callable on the corpus host");
        fre.call_global(SND_SET_FREE_ROAM_MISSION, &[])
            .expect("the native is callable on the corpus host");
        let reqs = fre.audio_seam_requests().expect("args are valid");
        assert_eq!(reqs.len(), 2);
        assert_eq!(
            reqs[0],
            AudioSeamRequest::SetMission {
                mission: "e01m01".into()
            }
        );
        assert_eq!(
            reqs[1],
            AudioSeamRequest::SetFreeRoamMission { mission: None }
        );
    }
}

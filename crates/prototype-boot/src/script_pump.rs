//! script_pump — bridge the PROVEN boot-stage chain to the script
//! lifecycle (M9).
//!
//! ## What is PROVEN vs LIKELY here
//!
//! PROVEN (Phase 2D / 2C):
//! * The 11-stage boot chain and its order (`BootStage::ALL`).
//! * The 5 cement archives mount during `CementLibraryInit`, and the
//!   startup package loads at `StartupPackageLoaded`.
//! * The shipped scripts live in `scripts.rcf` (separate archive;
//!   its exact mount point in the chain is NOT in the proven set).
//! * `ResetFreeRoamEvents` and `DestroyFreeRoamEvents` are Lua
//!   closures exported by `frevents.lua` (M8 asserted; re-asserted
//!   in `engine_runtime::tests::lifecycle_closures_are_visible`).
//!
//! LIKELY (policy encoded here, adjustable):
//! * Script MAIN chunks execute once content is up — modelled at
//!   `StartupPackageLoaded` (registration phase; the fre event defs
//!   must exist before gameplay queries them).
//! * A gameplay session RESETS the free-roam event set — modelled at
//!   `Gameplay` entry, dispatched via
//!   [`EngineRuntime::dispatch_signal`]`(GameplayEnter)`, which
//!   invokes the bound `ResetFreeRoamEvents` closure.
//! * Session shutdown destroys the events — modelled via
//!   [`ScriptPump::shutdown`], dispatched via
//!   [`EngineRuntime::dispatch_signal`]`(SessionEnd)`.
//!
//! The pump records every (stage → action) pair it takes, so tests
//! can assert the full boot walk deterministically.
//!
//! ## M9 wiring
//!
//! The pump owns an [`EngineRuntime`] (not a raw
//! `ScriptedFreeRoam`).  At construction it calls
//! [`EngineRuntime::bind_default_lifecycle`], which subscribes the
//! script's `ResetFreeRoamEvents` / `DestroyFreeRoamEvents`
//! closures to the `GameplayEnter` / `SessionEnd` signals.
//! Boot-stage transitions then drive the runtime via
//! [`EngineRuntime::dispatch_signal`] — every reset/destroy now
//! flows through the message router (the M9 milestone's "message
//! router + behaviours wired to scripts" deliverable).

use prototype_engine::engine_runtime::{EngineRuntime, EngineSignal};
use prototype_engine::runtime::FreeRoamEventSystem;
use prototype_lua::Lua51File;

use crate::BootStage;

/// What the pump did at a stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PumpAction {
    /// Nothing — stage is below the script-content threshold.
    Skipped,
    /// Ran the script main chunk (event registration).
    RegisteredEvents(usize),
    /// Called the script's reset closure (event count).
    ResetEvents(usize),
    /// Called the script's destroy closure (event count).
    DestroyedEvents(usize),
}

/// Drives an [`EngineRuntime`] from boot-stage transitions.
///
/// Constructed from a parsed `frevents.lua` chunk; the runtime's
/// main chunk has already run by the time the constructor returns
/// (so all 60 events are registered and the lifecycle closures
/// are visible as `Value::Closure` globals).
pub struct ScriptPump<'a> {
    /// The cohesive M9 runtime — owns the [`MessageRouter`] and the
    /// [`ScriptedFreeRoam`].  All reset/destroy dispatch flows
    /// through here.
    rt: Option<EngineRuntime<'a>>,
    /// Set once the main chunk has run (registration done).
    registered_at: Option<BootStage>,
    history: Vec<(BootStage, PumpAction)>,
}

impl<'a> ScriptPump<'a> {
    /// Create a pump around a parsed `frevents.lua` chunk.  Runs
    /// the script's main chunk and binds the default lifecycle
    /// subscriptions (`ResetFreeRoamEvents` → `GameplayEnter`,
    /// `DestroyFreeRoamEvents` → `SessionEnd`).
    pub fn new(file: &'a Lua51File) -> Result<Self, String> {
        let mut rt = EngineRuntime::new(file)?;
        // Bind the canonical lifecycle subscriptions BEFORE any
        // stage transitions: dispatch is a no-op until a signal
        // has subscribers.
        rt.bind_default_lifecycle()?;
        Ok(ScriptPump {
            rt: Some(rt),
            registered_at: None,
            history: Vec::new(),
        })
    }

    /// Feed one boot-stage transition; returns the action taken.
    pub fn on_stage(&mut self, stage: BootStage) -> Result<PumpAction, String> {
        use BootStage::*;
        let action = match stage {
            // Before content is mounted, scripts cannot even load.
            InitialGate | MainWaitLoop | DisplayInitStart | DisplayInitComplete
            | ModuleRegistration | AtgWorkerDispatch | CementLibraryInit => {
                PumpAction::Skipped
            }
            // LIKELY: registration once the content pipeline is up.
            StartupPackageLoaded => match &mut self.rt {
                Some(rt) => {
                    let n = rt.fre().system().len();
                    self.registered_at = Some(stage);
                    PumpAction::RegisteredEvents(n)
                }
                None => PumpAction::Skipped,
            },
            // Frames and loading screen: events stay as registered.
            FirstFramePresented | LoadingScreen => PumpAction::Skipped,
            // LIKELY: entering gameplay resets the event set for the
            // new session.  Dispatch flows through the engine runtime
            // (the M9 router↔script bridge).
            Gameplay => match &mut self.rt {
                Some(rt) if self.registered_at.is_some() => {
                    // Pre-dispatch host-call count for the reset path.
                    let before = rt.host_call_count("fre_ResetEvent");
                    rt.dispatch_signal(EngineSignal::GameplayEnter)?;
                    let after = rt.host_call_count("fre_ResetEvent");
                    PumpAction::ResetEvents(after - before)
                }
                _ => PumpAction::Skipped,
            },
        };
        self.history.push((stage, action.clone()));
        Ok(action)
    }

    /// Walk the entire proven boot chain, returning the actions
    /// taken per stage.
    pub fn walk_boot_chain(&mut self) -> Result<Vec<(BootStage, PumpAction)>, String> {
        for stage in BootStage::ALL {
            self.on_stage(stage)?;
        }
        Ok(self.history.clone())
    }

    /// Session shutdown (not part of the boot chain): destroy all
    /// events by dispatching `SessionEnd` through the runtime,
    /// which invokes the bound `DestroyFreeRoamEvents` closure.
    pub fn shutdown(&mut self) -> Result<PumpAction, String> {
        match &mut self.rt {
            Some(rt) if self.registered_at.is_some() => {
                let before = rt.host_call_count("fre_DestroyEvent");
                rt.dispatch_signal(EngineSignal::SessionEnd)?;
                let after = rt.host_call_count("fre_DestroyEvent");
                let a = PumpAction::DestroyedEvents(after - before);
                self.history.push((BootStage::Gameplay, a.clone()));
                Ok(a)
            }
            _ => Ok(PumpAction::Skipped),
        }
    }

    /// Borrow the engine-side event system (requires registration to
    /// have happened).
    pub fn system(&self) -> Result<std::cell::Ref<'_, FreeRoamEventSystem>, String> {
        match &self.rt {
            Some(rt) => Ok(rt.fre().system()),
            None => Err("script runtime dropped".into()),
        }
    }

    /// Borrow the [`EngineRuntime`] (M9 — exposes the router, the
    /// dispatch log, and the scripted free-roam system).
    pub fn runtime(&self) -> Result<&EngineRuntime<'a>, String> {
        match &self.rt {
            Some(rt) => Ok(rt),
            None => Err("script runtime dropped".into()),
        }
    }

    /// The (stage → action) history.
    pub fn history(&self) -> &[(BootStage, PumpAction)] {
        &self.history
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn pump_walks_the_full_boot_chain() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut pump = ScriptPump::new(&f).unwrap();

        // Before the content stages: nothing happens and the system
        // is still fully populated (registration happens at
        // construction; the stage gates only the RESET/DESTROY
        // phases).
        let a = pump.on_stage(BootStage::InitialGate).unwrap();
        assert_eq!(a, PumpAction::Skipped);
        let a = pump.on_stage(BootStage::CementLibraryInit).unwrap();
        assert_eq!(a, PumpAction::Skipped);

        // Registration acknowledged at the startup-package stage.
        let a = pump.on_stage(BootStage::StartupPackageLoaded).unwrap();
        assert_eq!(a, PumpAction::RegisteredEvents(60));

        // Frame stages are quiet.
        let a = pump.on_stage(BootStage::FirstFramePresented).unwrap();
        assert_eq!(a, PumpAction::Skipped);

        // Gameplay entry resets all 60 events through the runtime's
        // dispatch_signal(GameplayEnter) → ResetFreeRoamEvents().
        let a = pump.on_stage(BootStage::Gameplay).unwrap();
        assert_eq!(a, PumpAction::ResetEvents(60));
        assert!(pump
            .system()
            .unwrap()
            .events()
            .iter()
            .all(|e| e.state == prototype_engine::runtime::EventState::Reset));

        // Shutdown destroys them via dispatch_signal(SessionEnd) →
        // DestroyFreeRoamEvents().
        let a = pump.shutdown().unwrap();
        assert_eq!(a, PumpAction::DestroyedEvents(60));
        assert!(pump
            .system()
            .unwrap()
            .events()
            .iter()
            .all(|e| e.state == prototype_engine::runtime::EventState::Destroyed));

        // History reflects the walk: 5 stage transitions + shutdown.
        let h = pump.history();
        assert_eq!(h.len(), 6);
        assert_eq!(h[2].0, BootStage::StartupPackageLoaded);
        assert_eq!(h[2].1, PumpAction::RegisteredEvents(60));
        assert_eq!(h[4].1, PumpAction::ResetEvents(60));
        assert_eq!(h[5].1, PumpAction::DestroyedEvents(60));
    }

    #[test]
    fn pump_walk_boot_chain_helper_covers_all_eleven_stages() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut pump = ScriptPump::new(&f).unwrap();
        let history = pump.walk_boot_chain().unwrap();
        assert_eq!(history.len(), BootStage::ALL.len());
        // Exactly one registration and one reset in a full walk.
        let regs = history
            .iter()
            .filter(|(_, a)| matches!(a, PumpAction::RegisteredEvents(60)))
            .count();
        assert_eq!(regs, 1);
        let resets = history
            .iter()
            .filter(|(_, a)| matches!(a, PumpAction::ResetEvents(60)))
            .count();
        assert_eq!(resets, 1);
        // Every pre-content stage was skipped.
        assert_eq!(
            history
                .iter()
                .take(6)
                .filter(|(_, a)| *a == PumpAction::Skipped)
                .count(),
            6
        );
    }

    #[test]
    fn pump_dispatch_flows_through_router_audit_log() {
        // M9: the pump's reset/destroy now flow through the engine
        // runtime's dispatch, leaving an audit trail.
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut pump = ScriptPump::new(&f).unwrap();
        pump.on_stage(BootStage::StartupPackageLoaded).unwrap();
        pump.on_stage(BootStage::Gameplay).unwrap();
        let log = pump.runtime().unwrap().dispatch_log();
        // Exactly one dispatch entry so far: GameplayEnter → ResetFreeRoamEvents.
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].signal, EngineSignal::GameplayEnter);
        assert_eq!(log[0].global, "ResetFreeRoamEvents");
        assert!(log[0].ok);
        // After shutdown: 2 entries.
        pump.shutdown().unwrap();
        let log = pump.runtime().unwrap().dispatch_log();
        assert_eq!(log.len(), 2);
        assert_eq!(log[1].signal, EngineSignal::SessionEnd);
        assert_eq!(log[1].global, "DestroyFreeRoamEvents");
        assert!(log[1].ok);
    }

    #[test]
    fn pump_default_lifecycle_is_bound_at_construction() {
        // PROVEN at construction time: the canonical Prototype
        // lifecycle is wired into the runtime before any stage fires.
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let pump = ScriptPump::new(&f).unwrap();
        let rt = pump.runtime().unwrap();
        assert_eq!(
            rt.script_subscribers(EngineSignal::GameplayEnter),
            &["ResetFreeRoamEvents".to_string()]
        );
        assert_eq!(
            rt.script_subscribers(EngineSignal::SessionEnd),
            &["DestroyFreeRoamEvents".to_string()]
        );
    }
}

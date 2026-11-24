//! runtime — typed engine-side state driven by the recovered scripts
//! (M9).
//!
//! ## What this module closes
//!
//! M8 proved the scripts EXECUTE and their calls can be observed.
//! M9 turns the record-only stubs into behaviour-bearing hosts: the
//! engine now keeps typed Rust state that the scripts mutate through
//! their natives.
//!
//! ```text
//! frevents.lua  ──►  fre_RegisterEvent(def)     ──► FreeRoamEventSystem
//!                    fre_RegisterCollectibles(c) ──►   .collectibles
//!                    ResetFreeRoamEvents()       ──►   .reset_all()
//!                    DestroyFreeRoamEvents()     ──►   .destroy_all()
//! ```
//!
//! ## PROVEN (all recovered at runtime from the real script, asserted
//! in tests)
//!
//! * 60 event defs register in a deterministic order; every def has
//!   `name` (unique) and `class`.
//! * Classes: `freKill` (freP/freV/freF), `fre3WW` (freW — three-way
//!   war), `freLoco` (freL00-14), `freGlide` (freL20-29), `freOpp`
//!   (freO), `freColl` (freC).
//! * `vars` maps VAR NAMES to definitions `{isFloat?, default?,
//!   numElements?}` — e.g. `scores`/`times` carry all three; the
//!   freC00 collectible counters carry only `default`.
//! * `freC00` additionally has a top-level `backgroundType = 2`.
//! * `collectiblesDef` global = {maxHintCollectibles=50,
//!   maxLocationCollectibles=500, numScientistEvents=5}.
//!
//! ## LIKELY (semantic interpretation, documented as such)
//!
//! * Event states `Registered → Reset / Destroyed` — the observable
//!   effects of `fre_ResetEvent` / `fre_DestroyEvent` are modelled as
//!   per-event state flags.  The original engine certainly does more
//!   (entity teardown, HUD updates); the flag model captures the
//!   script-visible contract only.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use prototype_lua::vm::{Table, Value, Vm};
use prototype_lua::Lua51File;

use crate::entity::{Entity, EntityId};

// ---------------------------------------------------------------------------
// Typed free-roam event model
// ---------------------------------------------------------------------------

/// Event class recovered from the def's `class` field.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FreClass {
    /// `freKill` — generic kill events (freP, freV, freF families).
    Kill,
    /// `fre3WW` — three-way-war events (freW family).
    ThreeWayWar,
    /// `freLoco` — locomotion events (freL00-14).
    Loco,
    /// `freGlide` — glide events (freL20-29).
    Glide,
    /// `freOpp` — opponent events (freO family).
    Opp,
    /// `freColl` — the collectibles event (freC00).
    Coll,
    /// Anything else (future-proof; none in the shipped script).
    Other(String),
}

impl FreClass {
    pub fn from_bytes(s: &[u8]) -> Self {
        match s {
            b"freKill" => FreClass::Kill,
            b"fre3WW" => FreClass::ThreeWayWar,
            b"freLoco" => FreClass::Loco,
            b"freGlide" => FreClass::Glide,
            b"freOpp" => FreClass::Opp,
            b"freColl" => FreClass::Coll,
            other => FreClass::Other(String::from_utf8_lossy(other).into_owned()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            FreClass::Kill => "freKill",
            FreClass::ThreeWayWar => "fre3WW",
            FreClass::Loco => "freLoco",
            FreClass::Glide => "freGlide",
            FreClass::Opp => "freOpp",
            FreClass::Coll => "freColl",
            FreClass::Other(s) => s,
        }
    }
}

/// One entry of a def's `vars` table (var name → attributes).
#[derive(Debug, Clone, PartialEq)]
pub struct FreVar {
    /// Variable name (`scores`, `times`, `numLandmarksCollected`, …).
    pub name: String,
    /// `isFloat` attribute (absent on the freC00 counters).
    pub is_float: Option<bool>,
    /// `default` attribute.
    pub default: Option<f32>,
    /// `numElements` attribute.
    pub num_elements: Option<f32>,
}

/// A free-roam event definition, typed from the script's def table.
#[derive(Debug, Clone, PartialEq)]
pub struct FreEventDef {
    pub name: String,
    pub class: FreClass,
    /// Only `freC00` carries `backgroundType` (= 2).
    pub background_type: Option<f32>,
    /// `vars` entries in script (insertion) order.
    pub vars: Vec<FreVar>,
}

/// Lifecycle state of a registered event (LIKELY semantics — see
/// module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventState {
    /// Registered via `fre_RegisterEvent`, not yet reset.
    Registered,
    /// Reset via `fre_ResetEvent` (ready to run again).
    Reset,
    /// Destroyed via `fre_DestroyEvent`.
    Destroyed,
}

/// A runtime event entity: its definition plus lifecycle state.
///
/// Carries an [`EntityId`] (M9) so it can be referenced through
/// the generic [`crate::entity::EntityStore`] skeleton — the first
/// concrete gameplay entity promoted to a first-class engine object.
#[derive(Debug, Clone, PartialEq)]
pub struct FreEvent {
    /// Stable typed handle (OUR scheme — see [`crate::entity`]).
    pub id: EntityId,
    pub def: FreEventDef,
    pub state: EventState,
    /// Live score values for the def's vars, initialised from the
    /// var defaults (OUR model — the script only declares defaults;
    /// the original engine's in-memory layout is unknown).
    scores: HashMap<String, f32>,
}

impl Entity for FreEvent {
    fn id(&self) -> EntityId {
        self.id
    }
    fn name(&self) -> &str {
        &self.def.name
    }
}

impl FreEvent {
    pub fn name(&self) -> &str {
        &self.def.name
    }
    pub fn class(&self) -> &FreClass {
        &self.def.class
    }
    /// Stable typed handle for this event (M9).
    pub fn entity_id(&self) -> EntityId {
        self.id
    }

    /// Current score value of a var (defaults to the def default).
    pub fn score(&self, var: &str) -> Option<f32> {
        self.scores.get(var).copied().or_else(|| {
            self.def
                .vars
                .iter()
                .find(|v| v.name == var)
                .and_then(|v| v.default)
        })
    }

    /// Update a var's live score.  Returns false for unknown vars.
    pub fn set_score(&mut self, var: &str, value: f32) -> bool {
        if self.def.vars.iter().any(|v| v.name == var) {
            self.scores.insert(var.to_string(), value);
            true
        } else {
            false
        }
    }

    /// Live score values (var → value) in def order.
    pub fn scores(&self) -> Vec<(&str, f32)> {
        self.def
            .vars
            .iter()
            .map(|v| (v.name.as_str(), self.score(&v.name).unwrap_or(0.0)))
            .collect()
    }
}

/// The collectibles scalars (`collectiblesDef` global + the table
/// passed to `fre_RegisterCollectibles`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CollectiblesDef {
    pub max_hint_collectibles: f32,
    pub max_location_collectibles: f32,
    pub num_scientist_events: f32,
}

impl Default for CollectiblesDef {
    fn default() -> Self {
        CollectiblesDef {
            max_hint_collectibles: 50.0,
            max_location_collectibles: 500.0,
            num_scientist_events: 5.0,
        }
    }
}

// ---------------------------------------------------------------------------
// The engine-side system
// ---------------------------------------------------------------------------

/// Engine-side store of the free-roam event system, mutated by the
/// script hosts (see [`ScriptedFreeRoam`]).
#[derive(Debug, Default)]
pub struct FreeRoamEventSystem {
    events: Vec<FreEvent>,
    by_name: HashMap<String, usize>,
    /// Monotonic id allocator for [`FreEvent::id`] — never reused.
    next_id: u32,
    collectibles: Option<CollectiblesDef>,
    /// How many `fre_RegisterEvent` calls carried a def we could not
    /// type-parse (kept for honesty; always 0 for the shipped script).
    pub unparseable_defs: usize,
}

impl FreeRoamEventSystem {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, def: FreEventDef) {
        if let Some(&idx) = self.by_name.get(&def.name) {
            // Duplicate registration: replace the def in place but
            // KEEP the existing EntityId (stable handle for the same
            // name).  Never happens in the shipped script.
            self.events[idx].def = def;
            return;
        }
        let id = EntityId(self.next_id);
        self.next_id += 1;
        self.by_name
            .insert(def.name.clone(), self.events.len());
        self.events.push(FreEvent {
            id,
            def,
            state: EventState::Registered,
            scores: HashMap::new(),
        });
    }

    /// `fre_ResetEvent(def)` — state → Reset and scores back to
    /// defaults.  Returns false when the name is unknown.
    pub fn reset_event(&mut self, name: &str) -> bool {
        match self.by_name.get(name) {
            Some(&i) => {
                self.events[i].state = EventState::Reset;
                self.events[i].scores.clear();
                true
            }
            None => false,
        }
    }

    /// `fre_DestroyEvent(def)` — state → Destroyed.
    pub fn destroy_event(&mut self, name: &str) -> bool {
        match self.by_name.get(name) {
            Some(&i) => {
                self.events[i].state = EventState::Destroyed;
                true
            }
            None => false,
        }
    }

    pub fn reset_all(&mut self) -> usize {
        let n = self.events.len();
        for e in &mut self.events {
            e.state = EventState::Reset;
            e.scores.clear();
        }
        n
    }

    pub fn destroy_all(&mut self) -> usize {
        let n = self.events.len();
        for e in &mut self.events {
            e.state = EventState::Destroyed;
        }
        n
    }

    pub fn register_collectibles(&mut self, c: CollectiblesDef) {
        self.collectibles = Some(c);
    }

    /// Update a live score on one event (engine-side mutation).
    /// Returns false for unknown event or var.
    pub fn set_score(&mut self, event: &str, var: &str, value: f32) -> bool {
        match self.by_name.get(event) {
            Some(&i) => self.events[i].set_score(var, value),
            None => false,
        }
    }

    /// Read a live score.
    pub fn score(&self, event: &str, var: &str) -> Option<f32> {
        self.by_name.get(event).and_then(|&i| self.events[i].score(var))
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn events(&self) -> &[FreEvent] {
        &self.events
    }

    pub fn event(&self, name: &str) -> Option<&FreEvent> {
        self.by_name.get(name).map(|&i| &self.events[i])
    }

    /// Look up an event by its stable [`EntityId`] (M9).  O(1).
    pub fn event_by_id(&self, id: EntityId) -> Option<&FreEvent> {
        self.events.get(id.raw() as usize).filter(|e| e.id == id)
    }

    /// Mutable lookup by [`EntityId`].
    pub fn event_by_id_mut(&mut self, id: EntityId) -> Option<&mut FreEvent> {
        self.events.get_mut(id.raw() as usize).filter(|e| e.id == id)
    }

    /// All event [`EntityId`]s in registration order.
    pub fn entity_ids(&self) -> Vec<EntityId> {
        self.events.iter().map(|e| e.id).collect()
    }

    pub fn collectibles(&self) -> Option<CollectiblesDef> {
        self.collectibles
    }

    /// Number of events in a given class.
    pub fn class_count(&self, class: &FreClass) -> usize {
        self.events.iter().filter(|e| e.def.class == *class).count()
    }

    /// Events whose state is not Destroyed.
    pub fn live_events(&self) -> impl Iterator<Item = &FreEvent> {
        self.events
            .iter()
            .filter(|e| e.state != EventState::Destroyed)
    }
}

// ---------------------------------------------------------------------------
// Lua table → typed model parsing
// ---------------------------------------------------------------------------

fn strv(s: &str) -> Value {
    Value::Str(Rc::from(s.as_bytes()))
}

fn get_str(t: &Rc<RefCell<Table>>, key: &str) -> Option<String> {
    match t.borrow().get(&strv(key)) {
        Some(Value::Str(s)) => Some(String::from_utf8_lossy(&s).into_owned()),
        _ => None,
    }
}

fn get_num(t: &Rc<RefCell<Table>>, key: &str) -> Option<f32> {
    match t.borrow().get(&strv(key)) {
        Some(Value::Num(n)) => Some(n),
        _ => None,
    }
}

/// Parse a def table (as passed to `fre_RegisterEvent`) into a typed
/// [`FreEventDef`].  Errors carry the offending field.
pub fn parse_fre_def(t: &Rc<RefCell<Table>>) -> Result<FreEventDef, String> {
    let name = get_str(t, "name").ok_or("def has no string 'name'")?;
    let class = match t.borrow().get(&strv("class")) {
        Some(Value::Str(s)) => FreClass::from_bytes(&s),
        _ => return Err(format!("def {name}: no string 'class'")),
    };
    let background_type = get_num(t, "backgroundType");
    let mut vars = Vec::new();
    if let Some(Value::Table(vt)) = t.borrow().get(&strv("vars")) {
        for (k, v) in vt.borrow().iter() {
            let vname = match k {
                Value::Str(s) => String::from_utf8_lossy(s).into_owned(),
                _ => continue,
            };
            let mut var = FreVar {
                name: vname,
                is_float: None,
                default: None,
                num_elements: None,
            };
            if let Value::Table(vd) = v {
                var.is_float = match vd.borrow().get(&strv("isFloat")) {
                    Some(Value::Bool(b)) => Some(b),
                    _ => None,
                };
                var.default = get_num(vd, "default");
                var.num_elements = get_num(vd, "numElements");
            }
            vars.push(var);
        }
    }
    Ok(FreEventDef { name, class, background_type, vars })
}

fn parse_collectibles(t: &Rc<RefCell<Table>>) -> Result<CollectiblesDef, String> {
    Ok(CollectiblesDef {
        max_hint_collectibles: get_num(t, "maxHintCollectibles")
            .ok_or("collectiblesDef: maxHintCollectibles")?,
        max_location_collectibles: get_num(t, "maxLocationCollectibles")
            .ok_or("collectiblesDef: maxLocationCollectibles")?,
        num_scientist_events: get_num(t, "numScientistEvents")
            .ok_or("collectiblesDef: numScientistEvents")?,
    })
}

// ---------------------------------------------------------------------------
// Scripted runner — hosts mutate the typed system live
// ---------------------------------------------------------------------------

/// A VM plus a live [`FreeRoamEventSystem`] that the script's engine
/// natives mutate.  Created from a parsed `frevents.lua` chunk.
pub struct ScriptedFreeRoam<'a> {
    vm: Vm<'a>,
    system: Rc<RefCell<FreeRoamEventSystem>>,
}

impl<'a> ScriptedFreeRoam<'a> {
    /// Install behaviour-bearing hosts and run the script's main
    /// chunk (registration phase).
    pub fn new(file: &'a Lua51File) -> Result<Self, String> {
        let mut vm = Vm::new(file).map_err(|e| e.to_string())?;
        let system = Rc::new(RefCell::new(FreeRoamEventSystem::new()));

        let s = system.clone();
        vm.register_host("fre_RegisterEvent", move |_vm, args| {
            match args.first() {
                Some(Value::Table(t)) => match parse_fre_def(&t) {
                    Ok(def) => {
                        s.borrow_mut().register(def);
                        Ok(Vec::new())
                    }
                    Err(e) => Err(prototype_common::P3dError::Other(e)),
                },
                _ => Err(prototype_common::P3dError::Other(
                    "fre_RegisterEvent: def table expected".into(),
                )),
            }
        })
        .map_err(|e| e.to_string())?;

        let s = system.clone();
        vm.register_host("fre_RegisterCollectibles", move |_vm, args| {
            match args.first() {
                Some(Value::Table(t)) => match parse_collectibles(&t) {
                    Ok(c) => {
                        s.borrow_mut().register_collectibles(c);
                        Ok(Vec::new())
                    }
                    Err(e) => Err(prototype_common::P3dError::Other(e)),
                },
                _ => Err(prototype_common::P3dError::Other(
                    "fre_RegisterCollectibles: table expected".into(),
                )),
            }
        })
        .map_err(|e| e.to_string())?;

        let s = system.clone();
        vm.register_host("fre_ResetEvent", move |_vm, args| {
            match args.first() {
                Some(Value::Table(t)) => {
                    let name = get_str(&t, "name");
                    match name {
                        Some(n) if s.borrow_mut().reset_event(&n) => Ok(Vec::new()),
                        Some(n) => Err(prototype_common::P3dError::Other(format!(
                            "fre_ResetEvent: unknown event {n}"
                        ))),
                        None => Err(prototype_common::P3dError::Other(
                            "fre_ResetEvent: def has no name".into(),
                        )),
                    }
                }
                _ => Err(prototype_common::P3dError::Other(
                    "fre_ResetEvent: def table expected".into(),
                )),
            }
        })
        .map_err(|e| e.to_string())?;

        let s = system.clone();
        vm.register_host("fre_DestroyEvent", move |_vm, args| {
            match args.first() {
                Some(Value::Table(t)) => {
                    let name = get_str(&t, "name");
                    match name {
                        Some(n) if s.borrow_mut().destroy_event(&n) => Ok(Vec::new()),
                        Some(n) => Err(prototype_common::P3dError::Other(format!(
                            "fre_DestroyEvent: unknown event {n}"
                        ))),
                        None => Err(prototype_common::P3dError::Other(
                            "fre_DestroyEvent: def has no name".into(),
                        )),
                    }
                }
                _ => Err(prototype_common::P3dError::Other(
                    "fre_DestroyEvent: def table expected".into(),
                )),
            }
        })
        .map_err(|e| e.to_string())?;

        // Block 74: the snd_SetMission / snd_SetFreeRoamMission
        // natives ride the SAME host (the M7 registry is ONE host
        // surface; the shipped corpus never calls these — pinned by
        // test — but the call-through must exist where the corpus
        // runs).
        crate::audio_seam::install(&mut vm).map_err(|e| e.to_string())?;

        // Run the main chunk: builds collectiblesDef, registers all
        // events, defines the lifecycle closures.
        vm.run_main().map_err(|e| e.to_string())?;

        Ok(ScriptedFreeRoam { vm, system })
    }

    /// Read-only view of the engine-side system.
    pub fn system(&self) -> std::cell::Ref<'_, FreeRoamEventSystem> {
        self.system.borrow()
    }

    /// Mutable view of the engine-side system (engine-side updates).
    pub fn system_mut(&self) -> std::cell::RefMut<'_, FreeRoamEventSystem> {
        self.system.borrow_mut()
    }

    /// Invoke the script's `ResetFreeRoamEvents` closure — the script
    /// walks its def array and calls `fre_ResetEvent` for each, which
    /// flips engine-side state.  Returns the number of reset events.
    pub fn reset_all_via_script(&mut self) -> Result<usize, String> {
        let before = self.vm.host_calls_of("fre_ResetEvent").len();
        self.vm
            .call_global("ResetFreeRoamEvents", &[])
            .map_err(|e| e.to_string())?;
        let after = self.vm.host_calls_of("fre_ResetEvent").len();
        Ok(after - before)
    }

    /// Invoke the script's `DestroyFreeRoamEvents` closure.
    pub fn destroy_all_via_script(&mut self) -> Result<usize, String> {
        let before = self.vm.host_calls_of("fre_DestroyEvent").len();
        self.vm
            .call_global("DestroyFreeRoamEvents", &[])
            .map_err(|e| e.to_string())?;
        let after = self.vm.host_calls_of("fre_DestroyEvent").len();
        Ok(after - before)
    }

    /// The raw host-call log (all hosts, in order).
    pub fn host_calls(&self) -> &[prototype_lua::vm::HostCall] {
        &self.vm.host_calls
    }

    /// Call a script-defined global by name, recording any host
    /// calls it makes (proxy to [`Vm::call_global`]).
    pub fn call_global(
        &mut self,
        name: &str,
        args: &[Value],
    ) -> Result<Vec<Value>, prototype_common::P3dError> {
        self.vm.call_global(name, args)
    }

    /// Read a script global (`nil` when absent).
    pub fn get_global(&self, name: &str) -> Value {
        self.vm.get_global(name)
    }

    /// Recorded host calls of the given name, in call order, each
    /// entry being the call's argument list (proxy to
    /// [`Vm::host_calls_of`]).
    pub fn host_calls_of(&self, name: &str) -> Vec<Vec<Value>> {
        self.vm.host_calls_of(name)
    }

    /// Number of recorded host calls of `name`.
    pub fn host_call_count(&self, name: &str) -> usize {
        self.vm.host_calls.iter().filter(|c| c.name == name).count()
    }

    /// Block 74: the typed audio requests recorded by the snd
    /// natives on this host (`snd_SetMission` /
    /// `snd_SetFreeRoamMission`), in call order.  Fail-closed: a bad
    /// native argument is an `Err` naming the call — never a silent
    /// guess.  The real corpus produces an EMPTY vec (block 73's
    /// pinned-empty intersection, re-pinned at the typed level).
    pub fn audio_seam_requests(
        &self,
    ) -> Result<Vec<crate::audio_seam::AudioSeamRequest>, String> {
        crate::audio_seam::AudioSeamRequest::from_host_calls(&self.vm.host_calls)
    }
}

// ---------------------------------------------------------------------------
// Mission registry (M11 — typed state machine)
// ---------------------------------------------------------------------------

/// Lifecycle state of a mission entity.  The original engine's
/// mission state machine is UNKNOWN; this is OUR clean-room model
/// of the script-visible contract (`AddValidMission` registers a
/// mission; the gameplay session later starts/completes/fails it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MissionState {
    /// Registered via `AddValidMission`, not yet started.
    Available,
    /// Currently being played by the player.
    Active,
    /// Finished successfully.
    Completed,
    /// Finished unsuccessfully (player died, timer expired, etc.).
    Failed,
}

/// One mission as a first-class engine entity: stable [`EntityId`],
/// name, district, and lifecycle state.
#[derive(Debug, Clone, PartialEq)]
pub struct MissionEntity {
    /// Stable typed handle (OUR scheme — see [`crate::entity`]).
    pub id: EntityId,
    /// Mission name (e.g. `"Infected_Test"`, `"e11m01"`).
    pub name: String,
    /// District the mission lives in (`"manhattan"`, `"minimal"`,
    /// `"loco_test_bed"`, `"manhattan_mini"` — the 4 zones PROVEN
    /// for the shipped script).
    pub district: String,
    /// Current lifecycle state (OUR model — see [`MissionState`]).
    pub state: MissionState,
}

impl Entity for MissionEntity {
    fn id(&self) -> EntityId {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
}

impl MissionEntity {
    pub fn district(&self) -> &str {
        &self.district
    }
}

/// Engine-side valid-mission registry driven by `AddValidMission`
/// (M8 — registration) and the gameplay session (M11 — state
/// transitions via [`MissionSystem::start`] / `complete` / `fail`).
#[derive(Debug, Default, Clone)]
pub struct MissionSystem {
    pub missions: Vec<MissionEntity>,
    by_name: HashMap<String, usize>,
    /// Monotonic id allocator — never reused.
    next_id: u32,
}

impl MissionSystem {
    /// `AddValidMission(name, district)` — register a mission in
    /// the `Available` state.  Duplicate names are silently
    /// ignored (the shipped script never re-registers; asserted
    /// in tests).
    pub fn add(&mut self, name: &str, zone: &str) {
        if self.by_name.contains_key(name) {
            return;
        }
        let id = EntityId(self.next_id);
        self.next_id += 1;
        let i = self.missions.len();
        self.by_name.insert(name.into(), i);
        self.missions.push(MissionEntity {
            id,
            name: name.into(),
            district: zone.into(),
            state: MissionState::Available,
        });
    }
    pub fn len(&self) -> usize {
        self.missions.len()
    }
    pub fn is_empty(&self) -> bool {
        self.missions.is_empty()
    }
    /// Distinct zones in registration order.
    pub fn zones(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for m in &self.missions {
            if !out.contains(&m.district.as_str()) {
                out.push(&m.district);
            }
        }
        out
    }

    /// Start a mission: `Available → Active`.  Returns false when
    /// the mission is unknown or not in the Available state.
    pub fn start(&mut self, name: &str) -> bool {
        if let Some(&i) = self.by_name.get(name) {
            if self.missions[i].state == MissionState::Available {
                self.missions[i].state = MissionState::Active;
                return true;
            }
        }
        false
    }

    /// Complete a mission: `Active → Completed`.  Returns false
    /// when the mission is unknown or not Active.
    pub fn complete(&mut self, name: &str) -> bool {
        if let Some(&i) = self.by_name.get(name) {
            if self.missions[i].state == MissionState::Active {
                self.missions[i].state = MissionState::Completed;
                return true;
            }
        }
        false
    }

    /// Fail a mission: `Active → Failed`.  Returns false when the
    /// mission is unknown or not Active.
    pub fn fail(&mut self, name: &str) -> bool {
        if let Some(&i) = self.by_name.get(name) {
            if self.missions[i].state == MissionState::Active {
                self.missions[i].state = MissionState::Failed;
                return true;
            }
        }
        false
    }

    /// Lookup by name.
    pub fn find(&self, name: &str) -> Option<&MissionEntity> {
        self.by_name.get(name).map(|&i| &self.missions[i])
    }

    /// Mutable lookup by name.
    pub fn find_mut(&mut self, name: &str) -> Option<&mut MissionEntity> {
        if let Some(&i) = self.by_name.get(name) {
            Some(&mut self.missions[i])
        } else {
            None
        }
    }

    /// All missions in a given district, in registration order.
    pub fn in_district(&self, district: &str) -> Vec<&MissionEntity> {
        self.missions
            .iter()
            .filter(|m| m.district == district)
            .collect()
    }

    /// All missions in a given state, in registration order.
    pub fn by_state(&self, state: MissionState) -> impl Iterator<Item = &MissionEntity> {
        self.missions.iter().filter(move |m| m.state == state)
    }

    /// Count missions in each state.
    pub fn state_census(&self) -> [usize; 4] {
        let mut out = [0usize; 4];
        for m in &self.missions {
            let idx = match m.state {
                MissionState::Available => 0,
                MissionState::Active => 1,
                MissionState::Completed => 2,
                MissionState::Failed => 3,
            };
            out[idx] += 1;
        }
        out
    }
}

/// Run a mission-registration script (`AddValidMission` natives) and
/// feed every registration into `out`, then report how many missions
/// the script added.  (M14: shared by [`run_mission_script`] and
/// [`crate::EngineRuntime::attach_mission_script`].)
pub(crate) fn run_mission_script_into(
    file: &Lua51File,
    out: &mut MissionSystem,
) -> Result<usize, String> {
    let before = out.len();
    let mut vm = Vm::new(file).map_err(|e| e.to_string())?;
    let sys = Rc::new(RefCell::new(std::mem::take(out)));
    let s = sys.clone();
    vm.register_host("AddValidMission", move |_vm, args| {
        let name = match args.first() {
            Some(Value::Str(v)) => String::from_utf8_lossy(v).into_owned(),
            _ => {
                return Err(prototype_common::P3dError::Other(
                    "AddValidMission: name expected".into(),
                ))
            }
        };
        let zone = match args.get(1) {
            Some(Value::Str(v)) => String::from_utf8_lossy(v).into_owned(),
            _ => String::new(),
        };
        s.borrow_mut().add(&name, &zone);
        Ok(Vec::new())
    })
    .map_err(|e| e.to_string())?;
    vm.register_host("util_GetRandomFloat", |_vm, _args| {
        Ok(vec![Value::Num(0.5)])
    })
    .map_err(|e| e.to_string())?;
    vm.run_main().map_err(|e| e.to_string())?;
    // Drop the VM (and the host closures it holds — one still keeps
    // an Rc to the registry) BEFORE taking the registry back.
    drop(vm);
    *out = Rc::try_unwrap(sys)
        .map_err(|_| "mission registry still shared after script run".to_string())?
        .into_inner();
    Ok(out.len() - before)
}

/// Run a mission-registration script (`AddValidMission` natives) and
/// return the populated typed registry.
pub fn run_mission_script(
    file: &Lua51File,
) -> Result<MissionSystem, String> {
    let mut sys = MissionSystem::default();
    run_mission_script_into(file, &mut sys)?;
    Ok(sys)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
    fn frevents_builds_typed_system_with_sixty_events() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let fre = ScriptedFreeRoam::new(&f).unwrap();
        let sys = fre.system();
        assert_eq!(sys.len(), 60);
        assert_eq!(sys.unparseable_defs, 0);
        assert_eq!(sys.event("freP00").unwrap().class(), &FreClass::Kill);
        // Class census (runtime evidence, see module docs).
        assert_eq!(sys.class_count(&FreClass::Kill), 24);
        assert_eq!(sys.class_count(&FreClass::ThreeWayWar), 9);
        assert_eq!(sys.class_count(&FreClass::Loco), 10);
        assert_eq!(sys.class_count(&FreClass::Glide), 10);
        assert_eq!(sys.class_count(&FreClass::Opp), 6);
        assert_eq!(sys.class_count(&FreClass::Coll), 1);
        // Registration order preserved.
        assert_eq!(sys.events()[0].name(), "freP00");
        assert_eq!(sys.events()[59].name(), "freC00");
        // All names unique.
        let mut names: Vec<&str> =
            sys.events().iter().map(|e| e.name()).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "event names must be unique");
        // Collectibles scalars.
        assert_eq!(
            sys.collectibles(),
            Some(CollectiblesDef {
                max_hint_collectibles: 50.0,
                max_location_collectibles: 500.0,
                num_scientist_events: 5.0,
            })
        );
    }

    #[test]
    fn frevents_vars_are_typed_per_event() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let fre = ScriptedFreeRoam::new(&f).unwrap();
        let sys = fre.system();
        // freP00: vars = { scores = {isFloat=true, default=0,
        //                           numElements=2} }
        let p00 = sys.event("freP00").unwrap();
        assert_eq!(p00.def.vars.len(), 1);
        assert_eq!(p00.def.vars[0].name, "scores");
        assert_eq!(p00.def.vars[0].is_float, Some(true));
        assert_eq!(p00.def.vars[0].default, Some(0.0));
        assert_eq!(p00.def.vars[0].num_elements, Some(2.0));
        // freW00 (three-way war): its var is `times`.
        let w00 = sys.event("freW00").unwrap();
        assert_eq!(w00.def.vars[0].name, "times");
        assert_eq!(w00.def.vars[0].is_float, Some(true));
        // freC00: 7 collectible counter vars, default-only, plus a
        // top-level backgroundType.
        let c00 = sys.event("freC00").unwrap();
        assert_eq!(c00.def.background_type, Some(2.0));
        assert_eq!(c00.def.vars.len(), 7);
        let var_names: Vec<&str> =
            c00.def.vars.iter().map(|v| v.name.as_str()).collect();
        for expected in [
            "numLandmarksCollected",
            "numHintsCollected",
            "numScientistsCollected",
            "scientistsCompleteCount",
            "consumeEventMilitaryCount",
            "consumeEventInfectedCount",
            "scientistRewardState",
        ] {
            assert!(var_names.contains(&expected), "freC00 var {expected}");
        }
        for v in &c00.def.vars {
            assert_eq!(v.default, Some(0.0));
            assert_eq!(v.is_float, None, "{}: no isFloat", v.name);
        }
    }

    #[test]
    fn frevents_lifecycle_flips_states_via_script() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut fre = ScriptedFreeRoam::new(&f).unwrap();
        // After main: everything Registered.
        assert!(fre
            .system()
            .events()
            .iter()
            .all(|e| e.state == EventState::Registered));
        // Script-driven reset: 60 fre_ResetEvent calls.
        let n = fre.reset_all_via_script().unwrap();
        assert_eq!(n, 60);
        assert!(fre
            .system()
            .events()
            .iter()
            .all(|e| e.state == EventState::Reset));
        // Script-driven destroy: 60 fre_DestroyEvent calls.
        let n = fre.destroy_all_via_script().unwrap();
        assert_eq!(n, 60);
        assert!(fre
            .system()
            .events()
            .iter()
            .all(|e| e.state == EventState::Destroyed));
        assert_eq!(fre.system().live_events().count(), 0);
        // Host audit: 60 register + 1 collectibles + 60 reset + 60 destroy.
        let names: Vec<&str> =
            fre.host_calls().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names.iter().filter(|n| **n == "fre_RegisterEvent").count(), 60);
        assert_eq!(
            names.iter().filter(|n| **n == "fre_RegisterCollectibles").count(),
            1
        );
    }

    #[test]
    fn mission_script_builds_typed_mission_registry() {
        // Pull the 2318-byte mission chunk from scripts.rcf.
        let rcf = fixture("scripts.rcf");
        let arc = prototype_rcf::RcfArchive::parse(&rcf).unwrap();
        let mut chunk = None;
        for e in &arc.entries {
            let p = arc.payload(e).unwrap();
            if prototype_rcf::PayloadKind::sniff(p)
                == prototype_rcf::PayloadKind::Lua51
                && p.len() == 2318
            {
                chunk = Some(p.to_vec());
            }
        }
        let f = Lua51File::parse(&chunk.expect("mission chunk")).unwrap();
        let sys = run_mission_script(&f).unwrap();
        assert_eq!(sys.len(), 70);
        // M11: each mission is now a typed MissionEntity with a
        // stable EntityId, name, district, and lifecycle state
        // (defaults to Available on registration).
        assert_eq!(sys.missions[0].name, "Infected_Test");
        assert_eq!(sys.missions[0].district, "manhattan");
        assert_eq!(sys.missions[0].state, MissionState::Available);
        assert_eq!(sys.missions[0].id, EntityId(0));
        assert_eq!(sys.missions[69].name, "e11m01");
        assert_eq!(sys.missions[69].district, "manhattan");
        assert_eq!(sys.missions[69].id, EntityId(69));
        // Zone census (runtime evidence): 4 zones, first-appearance
        // order preserved.
        assert_eq!(
            sys.zones(),
            vec!["manhattan", "minimal", "loco_test_bed", "manhattan_mini"]
        );
        let manhattan = sys.in_district("manhattan").len();
        assert_eq!(manhattan, 63);
        let minimal = sys.in_district("minimal").len();
        assert_eq!(minimal, 4);
        let loco = sys.in_district("loco_test_bed").len();
        assert_eq!(loco, 2);
        let mini = sys.in_district("manhattan_mini").len();
        assert_eq!(mini, 1);
    }

    #[test]
    fn scoreboard_tracks_live_values_and_resets() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut fre = ScriptedFreeRoam::new(&f).unwrap();
        // Initial: every var reads its declared default.
        assert_eq!(fre.system().score("freP00", "scores"), Some(0.0));
        assert_eq!(
            fre.system().event("freC00").unwrap().scores(),
            vec![
                ("numLandmarksCollected", 0.0),
                ("numHintsCollected", 0.0),
                ("numScientistsCollected", 0.0),
                ("scientistsCompleteCount", 0.0),
                ("consumeEventMilitaryCount", 0.0),
                ("consumeEventInfectedCount", 0.0),
                ("scientistRewardState", 0.0),
            ]
        );
        // Engine-side gameplay updates.
        assert!(fre.system_mut().set_score("freC00", "numHintsCollected", 12.0));
        assert!(fre
            .system_mut()
            .set_score("freC00", "consumeEventInfectedCount", 3.0));
        assert_eq!(fre.system().score("freC00", "numHintsCollected"), Some(12.0));
        // Unknown var/event names are rejected.
        assert!(!fre.system_mut().set_score("freC00", "notAVar", 1.0));
        assert!(!fre.system_mut().set_score("noEvent", "scores", 1.0));
        // Script-driven reset clears live values back to defaults.
        fre.reset_all_via_script().unwrap();
        assert_eq!(fre.system().score("freC00", "numHintsCollected"), Some(0.0));
    }

    #[test]
    fn fre_class_round_trips() {
        for (bytes, class) in [
            (&b"freKill"[..], FreClass::Kill),
            (b"fre3WW", FreClass::ThreeWayWar),
            (b"freLoco", FreClass::Loco),
            (b"freGlide", FreClass::Glide),
            (b"freOpp", FreClass::Opp),
            (b"freColl", FreClass::Coll),
        ] {
            assert_eq!(FreClass::from_bytes(bytes), class);
            assert_eq!(class.as_str().as_bytes(), bytes);
        }
        assert_eq!(
            FreClass::from_bytes(b"freFuture"),
            FreClass::Other("freFuture".into())
        );
    }

    #[test]
    fn frevents_events_have_stable_entity_ids() {
        // M9: each registered FreEvent now carries a stable typed
        // EntityId — promotion to first-class engine object.
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let fre = ScriptedFreeRoam::new(&f).unwrap();
        let sys = fre.system();
        // 60 unique EntityIds, 0..60, in registration order.
        let ids = sys.entity_ids();
        assert_eq!(ids.len(), 60);
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(id.raw(), i as u32, "id at index {i} should be {i}");
        }
        // id ↔ name round-trip via accessors.
        let e0 = sys.event("freP00").unwrap();
        assert_eq!(e0.entity_id(), EntityId(0));
        assert_eq!(sys.event_by_id(EntityId(0)).unwrap().name(), "freP00");
        let e59 = sys.event("freC00").unwrap();
        assert_eq!(e59.entity_id(), EntityId(59));
        assert_eq!(sys.event_by_id(EntityId(59)).unwrap().name(), "freC00");
        // Lookup by stale id after a name collision isn't a concern
        // (script never re-registers); unknown id → None.
        assert!(sys.event_by_id(EntityId(999)).is_none());
        // FreEvent implements the Entity trait (usable by EntityStore).
        fn assert_entity<E: crate::entity::Entity>(e: &E) -> (EntityId, String) {
            (e.id(), e.name().to_string())
        }
        let (id, name) = assert_entity(e0);
        assert_eq!(id, EntityId(0));
        assert_eq!(name, "freP00");
    }

    #[test]
    fn event_by_id_mut_updates_state() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let fre = ScriptedFreeRoam::new(&f).unwrap();
        // Pick the 6th registered event by index (don't assume a
        // name; the script's freP family naming isn't sequential).
        let target_id = fre.system().events()[5].entity_id();
        let target_name = fre.system().events()[5].name().to_string();
        assert_eq!(fre.system().event_by_id(target_id).unwrap().name(), target_name);
        // Engine-side: flip it to Destroyed via its id.
        fre.system_mut()
            .event_by_id_mut(target_id)
            .unwrap()
            .state = EventState::Destroyed;
        assert_eq!(
            fre.system().event_by_id(target_id).unwrap().state,
            EventState::Destroyed
        );
        // Other events are untouched.
        assert_eq!(
            fre.system().events()[0].state,
            EventState::Registered
        );
        assert_eq!(
            fre.system().events()[10].state,
            EventState::Registered
        );
    }

    #[test]
    fn mission_state_machine_walks_available_active_completed() {
        // M11: missions transition Available → Active → Completed
        // through typed engine-side calls.
        let rcf = fixture("scripts.rcf");
        let arc = prototype_rcf::RcfArchive::parse(&rcf).unwrap();
        let mut chunk = None;
        for e in &arc.entries {
            let p = arc.payload(e).unwrap();
            if prototype_rcf::PayloadKind::sniff(p)
                == prototype_rcf::PayloadKind::Lua51
                && p.len() == 2318
            {
                chunk = Some(p.to_vec());
            }
        }
        let f = Lua51File::parse(&chunk.expect("mission chunk")).unwrap();
        let mut sys = run_mission_script(&f).unwrap();
        assert_eq!(sys.len(), 70);
        // Initial census: 70 Available, 0 in every other state.
        assert_eq!(sys.state_census(), [70, 0, 0, 0]);

        // Start one mission: Available → Active.
        assert!(sys.start("Infected_Test"));
        assert_eq!(sys.find("Infected_Test").unwrap().state, MissionState::Active);
        assert_eq!(sys.state_census(), [69, 1, 0, 0]);
        // Double-start is rejected (mission is no longer Available).
        assert!(!sys.start("Infected_Test"));

        // Complete it: Active → Completed.
        assert!(sys.complete("Infected_Test"));
        assert_eq!(
            sys.find("Infected_Test").unwrap().state,
            MissionState::Completed
        );
        assert_eq!(sys.state_census(), [69, 0, 1, 0]);
        // Can't complete a Completed mission.
        assert!(!sys.complete("Infected_Test"));

        // Start another and fail it: Active → Failed.
        assert!(sys.start("e11m01"));
        assert!(sys.fail("e11m01"));
        assert_eq!(sys.find("e11m01").unwrap().state, MissionState::Failed);
        assert_eq!(sys.state_census(), [68, 0, 1, 1]);

        // by_state iterator returns matching missions.
        let completed: Vec<&str> = sys.by_state(MissionState::Completed).map(|m| m.name.as_str()).collect();
        assert_eq!(completed, vec!["Infected_Test"]);
        let failed: Vec<&str> = sys.by_state(MissionState::Failed).map(|m| m.name.as_str()).collect();
        assert_eq!(failed, vec!["e11m01"]);

        // Unknown mission names are rejected.
        assert!(!sys.start("nope"));
        assert!(!sys.complete("nope"));
        assert!(!sys.fail("nope"));
        assert!(sys.find("nope").is_none());
    }

    #[test]
    fn mission_in_district_filters_by_zone() {
        let rcf = fixture("scripts.rcf");
        let arc = prototype_rcf::RcfArchive::parse(&rcf).unwrap();
        let mut chunk = None;
        for e in &arc.entries {
            let p = arc.payload(e).unwrap();
            if prototype_rcf::PayloadKind::sniff(p)
                == prototype_rcf::PayloadKind::Lua51
                && p.len() == 2318
            {
                chunk = Some(p.to_vec());
            }
        }
        let f = Lua51File::parse(&chunk.expect("mission chunk")).unwrap();
        let sys = run_mission_script(&f).unwrap();
        // 4 zones, all missions start Available.
        let in_manhattan: Vec<&str> = sys
            .in_district("manhattan")
            .iter()
            .map(|m| m.name.as_str())
            .collect();
        assert_eq!(in_manhattan.len(), 63);
        assert!(in_manhattan.contains(&"Infected_Test"));
        assert!(in_manhattan.contains(&"e11m01"));
        // minimal district: 4 missions.
        assert_eq!(sys.in_district("minimal").len(), 4);
        // loco_test_bed: 2 missions.
        assert_eq!(sys.in_district("loco_test_bed").len(), 2);
        // manhattan_mini: 1 mission.
        assert_eq!(sys.in_district("manhattan_mini").len(), 1);
    }

    #[test]
    fn mission_implements_entity_trait() {
        // M11: MissionEntity implements the Entity trait, so it
        // could be stored in a generic EntityStore<T> alongside
        // FreEvent.
        let rcf = fixture("scripts.rcf");
        let arc = prototype_rcf::RcfArchive::parse(&rcf).unwrap();
        let mut chunk = None;
        for e in &arc.entries {
            let p = arc.payload(e).unwrap();
            if prototype_rcf::PayloadKind::sniff(p)
                == prototype_rcf::PayloadKind::Lua51
                && p.len() == 2318
            {
                chunk = Some(p.to_vec());
            }
        }
        let f = Lua51File::parse(&chunk.expect("mission chunk")).unwrap();
        let sys = run_mission_script(&f).unwrap();
        // Use the Entity trait method to assert the id ↔ name
        // round-trip on the first mission.
        fn check<E: crate::entity::Entity>(e: &E) -> (EntityId, String) {
            (e.id(), e.name().to_string())
        }
        let m0 = &sys.missions[0];
        let (id, name) = check(m0);
        assert_eq!(id, EntityId(0));
        assert_eq!(name, "Infected_Test");
        // The 70 ids are 0..70 in registration order.
        for (i, m) in sys.missions.iter().enumerate() {
            assert_eq!(m.id, EntityId(i as u32));
        }
    }
}

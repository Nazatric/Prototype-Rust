//! event_system — engine-side, state-bearing types for the recovered
//! Prototype free-roam-event script surface (M9).
//!
//! ## What this closes
//!
//! M8 ran the shipped scripts with *record-only* stubs: every
//! `fre_RegisterEvent(def)` call was logged and thrown away.  This
//! module turns those logs into a typed engine object model: the
//! def tables the scripts build become first-class
//! [`FreeRoamEvent`] engine objects with per-event runtime state
//! ([`EventState`]), owned by a [`FreeRoamEventSystem`].
//!
//! ## Evidence for the data model (all recovered at RUNTIME from the
//! real `frevents.lua` via the M8 VM, cross-checked against the
//! constant pool)
//!
//! * PROVEN: every `fre_RegisterEvent` arg is a table with
//!   `name: string` (60/60), `class: string` (60/60) and
//!   `vars: table` (60/60); 7 defs also carry `backgroundType`
//!   (the six `freO*` = 1, `freC00` = 2).
//! * PROVEN: distinct class strings = `freKill`, `fre3WW`,
//!   `freLoco`, `freGlide`, `freOpp`, `freColl`.
//! * PROVEN: the `vars` table maps var-name → var-def; across the
//!   60 defs there are 72 var-defs with 12 distinct var names
//!   (`scores`, `times`, `distances`, `missionCounts`,
//!   `missionStates`, `numLandmarksCollected`, `numHintsCollected`,
//!   `numScientistsCollected`, `scientistsCompleteCount`,
//!   `consumeEventMilitaryCount`, `consumeEventInfectedCount`,
//!   `scientistRewardState`).  Every var-def has `default: number`
//!   (72/72); `isFloat: boolean` (53/72) and `numElements: number`
//!   (65/72) are optional.
//! * PROVEN: `fre_RegisterCollectibles` receives a 3-field table
//!   `{maxHintCollectibles=50, maxLocationCollectibles=500,
//!   numScientistEvents=5}`.
//! * PROVEN: the script exports lifecycle globals
//!   `ResetFreeRoamEvents` / `DestroyFreeRoamEvents`; calling them
//!   issues `fre_ResetEvent` / `fre_DestroyEvent` once per def with
//!   the SAME def tables that were registered (M8, asserted).
//! * LIKELY: `numElements` means "this var is an array of that
//!   length" (field name + values: 2 on every array-style var, and
//!   array-style names are plural: scores/times/distances/…);
//!   a var-def without `numElements` is a scalar.  Var *semantics*
//!   (what a "score" counts) are outside format recovery.
//! * LIKELY: `fre_ResetEvent` re-initialises an event's live vars
//!   from the declared defaults; `fre_DestroyEvent` tears the event
//!   down (the host names say reset/destroy; the shipped script
//!   calls them with no extra arguments, so the operation must be
//!   expressible from the def alone).
//!
//! No decompiled code is used anywhere: the types below are a
//! clean-room Rust model of data the scripts hand to the engine.

use std::collections::BTreeMap;
use std::rc::Rc;

use prototype_common::{P3dError, Result};
use prototype_lua::vm::{Table, Value};

// ---------------------------------------------------------------------------
// Typed event model
// ---------------------------------------------------------------------------

/// Handle for an event object inside the runtime entity store.
///
/// This is OUR runtime's identifier scheme (registration order,
/// 0-based) — the original engine's handle layout is UNKNOWN.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntityId(pub u32);

/// The six event classes the shipped script registers.  Each variant
/// is a verbatim `class` string from a real def table (PROVEN).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventClass {
    /// `freKill` — rides the freP/freV/freF families.
    FreKill,
    /// `fre3WW` — rides the freW family (three-way war).
    Fre3WW,
    /// `freLoco` — rides freL00–freL14.
    FreLoco,
    /// `freGlide` — rides freL20–freL29.
    FreGlide,
    /// `freOpp` — rides the freO family (the only family with
    /// `backgroundType`).
    FreOpp,
    /// `freColl` — freC00, the collectibles event.
    FreColl,
}

impl EventClass {
    /// Parse a verbatim `class` string from a def table.
    pub fn from_name(s: &str) -> Option<EventClass> {
        match s {
            "freKill" => Some(EventClass::FreKill),
            "fre3WW" => Some(EventClass::Fre3WW),
            "freLoco" => Some(EventClass::FreLoco),
            "freGlide" => Some(EventClass::FreGlide),
            "freOpp" => Some(EventClass::FreOpp),
            "freColl" => Some(EventClass::FreColl),
            _ => None,
        }
    }

    /// The verbatim class string.
    pub fn as_str(&self) -> &'static str {
        match self {
            EventClass::FreKill => "freKill",
            EventClass::Fre3WW => "fre3WW",
            EventClass::FreLoco => "freLoco",
            EventClass::FreGlide => "freGlide",
            EventClass::FreOpp => "freOpp",
            EventClass::FreColl => "freColl",
        }
    }
}

/// A var-declaration inside a def's `vars` table (PROVEN shape; see
/// module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct EventVarDef {
    /// `default` — present on 72/72 var-defs.
    pub default: f32,
    /// `isFloat` — present on 53/72.
    pub is_float: Option<bool>,
    /// `numElements` — present on 65/72; LIKELY: the var is an array
    /// of this length.
    pub num_elements: Option<u32>,
}

/// A declared var's live value.  `numElements` present → array
/// filled with the default; absent → scalar (LIKELY, see module
/// docs).
#[derive(Debug, Clone, PartialEq)]
pub enum VarValue {
    Scalar(f32),
    Array(Vec<f32>),
}

impl VarValue {
    /// Live numeric view: scalars yield themselves, arrays yield
    /// element 0 (or 0.0 when empty).
    pub fn first(&self) -> f32 {
        match self {
            VarValue::Scalar(v) => *v,
            VarValue::Array(a) => a.first().copied().unwrap_or(0.0),
        }
    }
}

/// Live per-event state machine.  The three states mirror the three
/// host calls the shipped script makes per def (`fre_RegisterEvent`
/// / `fre_ResetEvent` / `fre_DestroyEvent` — PROVEN names); the
/// transitions are our runtime's model (LIKELY).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventState {
    /// Registered via `fre_RegisterEvent`, not yet reset/destroyed.
    Registered,
    /// Reset via `fre_ResetEvent` (live vars re-initialised).
    Reset,
    /// Destroyed via `fre_DestroyEvent`.
    Destroyed,
}

/// One free-roam event as a first-class engine object: the declared
/// def (from the script) plus live runtime state (this crate's).
#[derive(Debug, Clone)]
pub struct FreeRoamEvent {
    /// Entity handle in our runtime store.
    pub id: EntityId,
    /// `name` field (PROVEN, e.g. `freP00`).
    pub name: String,
    /// `class` field (PROVEN).
    pub class: EventClass,
    /// `backgroundType` — only on the six `freO*` defs (=1) and
    /// `freC00` (=2) (PROVEN distribution).
    pub background_type: Option<f32>,
    /// `vars` declarations in script order.
    pub vars: Vec<(String, EventVarDef)>,
    /// Live state (our model, LIKELY transitions).
    pub state: EventState,
    /// Live var values, (re-)initialised from the defaults on
    /// register/reset.
    pub current_vars: BTreeMap<String, VarValue>,
}

impl FreeRoamEvent {
    /// Build the initial live-var map from the declared defaults.
    fn initial_vars(&self) -> BTreeMap<String, VarValue> {
        self.vars
            .iter()
            .map(|(name, def)| {
                let v = match def.num_elements {
                    Some(n) => VarValue::Array(vec![def.default; n as usize]),
                    None => VarValue::Scalar(def.default),
                };
                (name.clone(), v)
            })
            .collect()
    }
}

/// The collectibles configuration the script registers via
/// `fre_RegisterCollectibles` (PROVEN values from the shipped
/// script's `collectiblesDef` global).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CollectiblesConfig {
    pub max_hint_collectibles: f32,
    pub max_location_collectibles: f32,
    pub num_scientist_events: f32,
}

// ---------------------------------------------------------------------------
// The system
// ---------------------------------------------------------------------------

/// Engine-side owner of all registered free-roam events.
///
/// The four `fre_*` host calls the shipped script makes map 1:1 onto
/// the methods below; the [`prototype_lua::vm`] host closures built
/// in [`crate::runtime`] delegate here.
#[derive(Debug, Default)]
pub struct FreeRoamEventSystem {
    events: Vec<FreeRoamEvent>,
    by_name: BTreeMap<String, usize>,
    collectibles: Option<CollectiblesConfig>,
    next_id: u32,
}

impl FreeRoamEventSystem {
    pub fn new() -> Self {
        Self::default()
    }

    /// `fre_RegisterEvent(def)` — parse a def table into a
    /// [`FreeRoamEvent`] and store it (state [`EventState::Registered`],
    /// live vars from defaults).
    pub fn register_event(&mut self, def: &Value) -> Result<EntityId> {
        let table = match def {
            Value::Table(t) => t.clone(),
            other => {
                return Err(P3dError::Other(format!(
                    "fre_RegisterEvent: def must be a table, got {}",
                    other.type_name()
                )))
            }
        };
        let name = string_field(&table, "name")?;
        let class_str = string_field(&table, "class")?;
        let class = EventClass::from_name(&class_str).ok_or_else(|| {
            P3dError::Other(format!(
                "fre_RegisterEvent: unknown event class '{class_str}'"
            ))
        })?;
        let background_type = match num_field(&table, "backgroundType")? {
            Some(v) => Some(v),
            None => None,
        };
        let vars = match field(&table, "vars")? {
            Some(Value::Table(vt)) => {
                let mut vars = Vec::new();
                for (k, v) in vt.borrow().iter() {
                    let var_name = match k {
                        Value::Str(s) => String::from_utf8_lossy(s).into_owned(),
                        other => {
                            return Err(P3dError::Other(format!(
                                "fre_RegisterEvent {name}: var key must be a string, got {}",
                                other.type_name()
                            )))
                        }
                    };
                    vars.push((var_name, parse_var_def(&name, v)?));
                }
                vars
            }
            Some(other) => {
                return Err(P3dError::Other(format!(
                    "fre_RegisterEvent {name}: 'vars' must be a table, got {}",
                    other.type_name()
                )))
            }
            None => {
                return Err(P3dError::Other(format!(
                    "fre_RegisterEvent {name}: def has no 'vars' table"
                )))
            }
        };

        if self.by_name.contains_key(&name) {
            return Err(P3dError::Other(format!(
                "fre_RegisterEvent: event '{name}' registered twice"
            )));
        }
        let id = EntityId(self.next_id);
        self.next_id += 1;
        let ev = FreeRoamEvent {
            id,
            name: name.clone(),
            class,
            background_type,
            vars,
            state: EventState::Registered,
            current_vars: BTreeMap::new(),
        };
        let idx = self.events.len();
        self.events.push(ev);
        self.by_name.insert(name, idx);
        let ev = &mut self.events[idx];
        ev.current_vars = ev.initial_vars();
        Ok(id)
    }

    /// `fre_RegisterCollectibles(def)` — store the collectibles
    /// configuration (PROVEN 3-field table).
    pub fn register_collectibles(&mut self, def: &Value) -> Result<()> {
        let table = match def {
            Value::Table(t) => t.clone(),
            other => {
                return Err(P3dError::Other(format!(
                    "fre_RegisterCollectibles: def must be a table, got {}",
                    other.type_name()
                )))
            }
        };
        let cfg = CollectiblesConfig {
            max_hint_collectibles: num_field(&table, "maxHintCollectibles")?
                .ok_or_else(|| P3dError::Other("collectiblesDef: no maxHintCollectibles".into()))?,
            max_location_collectibles: num_field(
                &table,
                "maxLocationCollectibles",
            )?
            .ok_or_else(|| {
                P3dError::Other("collectiblesDef: no maxLocationCollectibles".into())
            })?,
            num_scientist_events: num_field(&table, "numScientistEvents")?
                .ok_or_else(|| P3dError::Other("collectiblesDef: no numScientistEvents".into()))?,
        };
        self.collectibles = Some(cfg);
        Ok(())
    }

    /// `fre_ResetEvent(def)` — flip the matching event to
    /// [`EventState::Reset`] and re-initialise its live vars from the
    /// declared defaults (LIKELY semantics, see module docs).
    pub fn reset_event(&mut self, def: &Value) -> Result<()> {
        let name = def_name(def)?;
        let idx = self.lookup(&name)?;
        let ev = &mut self.events[idx];
        ev.state = EventState::Reset;
        ev.current_vars = ev.initial_vars();
        Ok(())
    }

    /// `fre_DestroyEvent(def)` — flip the matching event to
    /// [`EventState::Destroyed`] (live vars are kept for inspection
    /// but the event is torn down; our model, LIKELY).
    pub fn destroy_event(&mut self, def: &Value) -> Result<()> {
        let name = def_name(def)?;
        let idx = self.lookup(&name)?;
        self.events[idx].state = EventState::Destroyed;
        Ok(())
    }

    fn lookup(&self, name: &str) -> Result<usize> {
        self.by_name.get(name).copied().ok_or_else(|| {
            P3dError::Other(format!("fre_*: unknown event '{name}'"))
        })
    }

    /// All events in registration order (60 for the shipped script).
    pub fn events(&self) -> &[FreeRoamEvent] {
        &self.events
    }

    /// Typed accessor by name.
    pub fn event(&self, name: &str) -> Option<&FreeRoamEvent> {
        self.by_name.get(name).map(|&i| &self.events[i])
    }

    /// Mutable typed accessor by name — the game updates live vars
    /// through this (our runtime's model).
    pub fn event_mut(&mut self, name: &str) -> Option<&mut FreeRoamEvent> {
        let idx = *self.by_name.get(name)?;
        self.events.get_mut(idx)
    }

    /// Entity-handle accessor (our ID scheme).
    pub fn entity(&self, id: EntityId) -> Option<&FreeRoamEvent> {
        self.events.iter().find(|e| e.id == id)
    }

    /// The registered collectibles config, if
    /// `fre_RegisterCollectibles` ran.
    pub fn collectibles(&self) -> Option<CollectiblesConfig> {
        self.collectibles
    }

    /// Count events per class, in enum order (registration data).
    pub fn class_counts(&self) -> BTreeMap<&'static str, usize> {
        let mut m = BTreeMap::new();
        for e in &self.events {
            *m.entry(e.class.as_str()).or_insert(0usize) += 1;
        }
        m
    }
}

// ---------------------------------------------------------------------------
// Mission registry (the other shipped script's surface)
// ---------------------------------------------------------------------------

/// One `AddValidMission(name, district)` registration (PROVEN host
/// call shape: two strings, e.g. `("e11m01", "manhattan")`).
#[derive(Debug, Clone, PartialEq)]
pub struct MissionEntry {
    pub name: String,
    pub district: String,
}

/// Engine-side store for the mission script's registrations.
#[derive(Debug, Default)]
pub struct MissionRegistry {
    entries: Vec<MissionEntry>,
}

impl MissionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// `AddValidMission(name, district)`.
    pub fn add(&mut self, name: &Value, district: &Value) -> Result<()> {
        let name = value_string(name, "AddValidMission arg #1")?;
        let district = value_string(district, "AddValidMission arg #2")?;
        self.entries.push(MissionEntry { name, district });
        Ok(())
    }

    /// All registrations in call order (70 for the shipped script).
    pub fn entries(&self) -> &[MissionEntry] {
        &self.entries
    }

    /// Typed lookup by mission name.
    pub fn find(&self, name: &str) -> Option<&MissionEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// All missions in one district, in registration order.
    pub fn in_district(&self, district: &str) -> Vec<&MissionEntry> {
        self.entries
            .iter()
            .filter(|e| e.district == district)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Lua-table helpers (shared by the runtime host closures)
// ---------------------------------------------------------------------------

pub(crate) fn field(t: &Rc<std::cell::RefCell<Table>>, key: &str) -> Result<Option<Value>> {
    Ok(t.borrow()
        .get(&Value::Str(Rc::from(key.as_bytes()))))
}

fn string_field(t: &Rc<std::cell::RefCell<Table>>, key: &str) -> Result<String> {
    match field(t, key)? {
        Some(Value::Str(s)) => Ok(String::from_utf8_lossy(&s).into_owned()),
        Some(other) => Err(P3dError::Other(format!(
            "def field '{key}' must be a string, got {}",
            other.type_name()
        ))),
        None => Err(P3dError::Other(format!("def has no '{key}' field"))),
    }
}

pub(crate) fn num_field(t: &Rc<std::cell::RefCell<Table>>, key: &str) -> Result<Option<f32>> {
    match field(t, key)? {
        Some(Value::Num(n)) => Ok(Some(n)),
        Some(other) => Err(P3dError::Other(format!(
            "def field '{key}' must be a number, got {}",
            other.type_name()
        ))),
        None => Ok(None),
    }
}

fn parse_var_def(event: &str, v: &Value) -> Result<EventVarDef> {
    let t = match v {
        Value::Table(t) => t,
        other => {
            return Err(P3dError::Other(format!(
                "fre event {event}: var def must be a table, got {}",
                other.type_name()
            )))
        }
    };
    let default = num_field(t, "default")?
        .ok_or_else(|| P3dError::Other(format!("fre event {event}: var def has no 'default'")))?;
    let is_float = match field(t, "isFloat")? {
        Some(Value::Bool(b)) => Some(b),
        Some(other) => {
            return Err(P3dError::Other(format!(
                "fre event {event}: 'isFloat' must be a boolean, got {}",
                other.type_name()
            )))
        }
        None => None,
    };
    let num_elements = match num_field(t, "numElements")? {
        Some(n) if n >= 0.0 => Some(n as u32),
        Some(n) => {
            return Err(P3dError::Other(format!(
                "fre event {event}: negative numElements {n}"
            )))
        }
        None => None,
    };
    Ok(EventVarDef { default, is_float, num_elements })
}

fn value_string(v: &Value, what: &str) -> Result<String> {
    match v {
        Value::Str(s) => Ok(String::from_utf8_lossy(s).into_owned()),
        other => Err(P3dError::Other(format!(
            "{what} must be a string, got {}",
            other.type_name()
        ))),
    }
}

fn def_name(def: &Value) -> Result<String> {
    match def {
        Value::Table(t) => string_field(t, "name"),
        other => Err(P3dError::Other(format!(
            "fre_*: def must be a table, got {}",
            other.type_name()
        ))),
    }
}

// ---------------------------------------------------------------------------
// Tests — synthetic model checks (real-fixture runs live in runtime.rs)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn rc_str(s: &str) -> Value {
        Value::Str(Rc::from(s.as_bytes()))
    }

    fn table(pairs: Vec<(&str, Value)>) -> Value {
        let t = Table::new();
        for (k, v) in pairs {
            t.borrow_mut().set(rc_str(k), v).unwrap();
        }
        Value::Table(Rc::new(RefCell::new(t)))
    }

    #[test]
    fn event_class_parses_the_six_proven_strings() {
        for (s, want) in [
            ("freKill", EventClass::FreKill),
            ("fre3WW", EventClass::Fre3WW),
            ("freLoco", EventClass::FreLoco),
            ("freGlide", EventClass::FreGlide),
            ("freOpp", EventClass::FreOpp),
            ("freColl", EventClass::FreColl),
        ] {
            assert_eq!(EventClass::from_name(s), Some(want));
            assert_eq!(want.as_str(), s);
        }
        assert_eq!(EventClass::from_name("freNope"), None);
    }

    #[test]
    fn register_event_parses_scalars_and_arrays() {
        // Mirrors the PROVEN def shape: scalars without numElements,
        // arrays with it (dump of freC00 / freP00).
        let def = table(vec![
            ("name", rc_str("freT00")),
            ("class", rc_str("freColl")),
            ("backgroundType", Value::Num(2.0)),
            (
                "vars",
                table(vec![
                    (
                        "numLandmarksCollected",
                        table(vec![("default", Value::Num(0.0))]),
                    ),
                    (
                        "scores",
                        table(vec![
                            ("default", Value::Num(0.0)),
                            ("isFloat", Value::Bool(true)),
                            ("numElements", Value::Num(2.0)),
                        ]),
                    ),
                ]),
            ),
        ]);
        let mut sys = FreeRoamEventSystem::new();
        let id = sys.register_event(&def).unwrap();
        assert_eq!(id, EntityId(0));
        let ev = sys.event("freT00").unwrap();
        assert_eq!(ev.class, EventClass::FreColl);
        assert_eq!(ev.background_type, Some(2.0));
        assert_eq!(ev.vars.len(), 2);
        assert_eq!(ev.state, EventState::Registered);
        // live vars initialised from defaults: scalar + array of 2
        assert_eq!(
            ev.current_vars.get("numLandmarksCollected"),
            Some(&VarValue::Scalar(0.0))
        );
        assert_eq!(
            ev.current_vars.get("scores"),
            Some(&VarValue::Array(vec![0.0, 0.0]))
        );
        assert_eq!(sys.entity(EntityId(0)).unwrap().name, "freT00");
        assert_eq!(sys.entity(EntityId(1)), None);
    }

    #[test]
    fn register_event_rejects_bad_defs_loudly() {
        let mut sys = FreeRoamEventSystem::new();
        // non-table arg
        assert!(sys.register_event(&Value::Num(1.0)).is_err());
        // missing class
        let no_class = table(vec![
            ("name", rc_str("x")),
            ("vars", table(vec![])),
        ]);
        assert!(sys.register_event(&no_class).is_err());
        // unknown class
        let bad_class = table(vec![
            ("name", rc_str("x")),
            ("class", rc_str("freNope")),
            ("vars", table(vec![])),
        ]);
        assert!(sys.register_event(&bad_class).is_err());
        // no vars table
        let no_vars = table(vec![
            ("name", rc_str("x")),
            ("class", rc_str("freKill")),
        ]);
        assert!(sys.register_event(&no_vars).is_err());
        // var def without default
        let bad_var = table(vec![
            ("name", rc_str("x")),
            ("class", rc_str("freKill")),
            (
                "vars",
                table(vec![("v", table(vec![("isFloat", Value::Bool(true))]))]),
            ),
        ]);
        assert!(sys.register_event(&bad_var).is_err());
        // duplicate registration
        let ok = table(vec![
            ("name", rc_str("x")),
            ("class", rc_str("freKill")),
            ("vars", table(vec![])),
        ]);
        sys.register_event(&ok).unwrap();
        assert!(sys.register_event(&ok).is_err());
    }

    #[test]
    fn reset_and_destroy_flip_state_and_reinit_vars() {
        let def = table(vec![
            ("name", rc_str("freT00")),
            ("class", rc_str("freLoco")),
            (
                "vars",
                table(vec![(
                    "times",
                    table(vec![
                        ("default", Value::Num(7.5)),
                        ("isFloat", Value::Bool(true)),
                        ("numElements", Value::Num(2.0)),
                    ]),
                )]),
            ),
        ]);
        let mut sys = FreeRoamEventSystem::new();
        sys.register_event(&def).unwrap();

        // host code mutates the live vars (our model: the game plays).
        if let Some(VarValue::Array(a)) =
            sys.event_mut("freT00").unwrap().current_vars.get_mut("times")
        {
            a[0] = 100.0;
        }
        assert_eq!(
            sys.event("freT00").unwrap().current_vars.get("times"),
            Some(&VarValue::Array(vec![100.0, 7.5])),
            "index 0 mutated, index 1 keeps the default"
        );

        sys.reset_event(&def).unwrap();
        let ev = sys.event("freT00").unwrap();
        assert_eq!(ev.state, EventState::Reset);
        assert_eq!(
            ev.current_vars.get("times"),
            Some(&VarValue::Array(vec![7.5, 7.5])),
            "reset must re-init from defaults"
        );

        sys.destroy_event(&def).unwrap();
        assert_eq!(sys.event("freT00").unwrap().state, EventState::Destroyed);

        // unknown event names are loud errors
        assert!(sys.reset_event(&table(vec![("name", rc_str("freZZ"))])).is_err());
        assert!(sys.destroy_event(&Value::Nil).is_err());
    }

    #[test]
    fn collectibles_config_parses_the_proven_three_fields() {
        let def = table(vec![
            ("maxHintCollectibles", Value::Num(50.0)),
            ("maxLocationCollectibles", Value::Num(500.0)),
            ("numScientistEvents", Value::Num(5.0)),
        ]);
        let mut sys = FreeRoamEventSystem::new();
        sys.register_collectibles(&def).unwrap();
        assert_eq!(
            sys.collectibles(),
            Some(CollectiblesConfig {
                max_hint_collectibles: 50.0,
                max_location_collectibles: 500.0,
                num_scientist_events: 5.0,
            })
        );
        // missing field is loud
        let mut sys2 = FreeRoamEventSystem::new();
        assert!(sys2.register_collectibles(&table(vec![])).is_err());
    }

    #[test]
    fn mission_registry_typed_accessors() {
        let mut m = MissionRegistry::new();
        m.add(&rc_str("Infected_Test"), &rc_str("manhattan")).unwrap();
        m.add(&rc_str("e11m01"), &rc_str("manhattan")).unwrap();
        m.add(&rc_str("warzone_x"), &rc_str("military")).unwrap();
        assert_eq!(m.entries().len(), 3);
        assert_eq!(m.find("e11m01").unwrap().district, "manhattan");
        assert_eq!(m.find("nope"), None);
        assert_eq!(m.in_district("manhattan").len(), 2);
        assert_eq!(m.in_district("military")[0].name, "warzone_x");
        // non-string args are loud
        assert!(m.add(&Value::Num(1.0), &rc_str("manhattan")).is_err());
    }
}

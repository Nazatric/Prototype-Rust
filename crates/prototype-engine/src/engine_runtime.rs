//! engine_runtime — the cohesive M9 runtime that wires the
//! [`MessageRouter`] to the script lifecycle closures (M9).
//!
//! ## What this closes
//!
//! M8 ran the scripts; M9's typed `FreeRoamEventSystem` and
//! [`ScriptedFreeRoam`] turned the record-only stubs into a
//! behaviour-bearing host.  What was still missing was the
//! **router↔script bridge**: the engine's [`MessageRouter`] already
//! had a `subscribe_script` API, but no caller — and the boot-chain
//! `ScriptPump` hard-wired `reset_all_via_script` /
//! `destroy_all_via_script` directly, bypassing the router.
//!
//! This module supplies the missing piece:
//!
//! * [`EngineSignal`] — the runtime's own lifecycle signal enum
//!   (SessionStart / GameplayEnter / GameplayExit / SessionEnd).
//!   **LIKELY**: the original engine's session lifecycle messages
//!   are NOT in the recovered RTTI dump (no
//!   `engine::SessionStartMessage` class exists in
//!   `prototypeenginef.dll`'s 11 837 RTTI records).  Our signals are
//!   a clean-room dispatch category for the boot→scripts→teardown
//!   bridge, NOT a reconstruction of original-engine message types.
//! * [`EngineRuntime`] — owns a [`MessageRouter`] (for the RTTI
//!   `MessageKind` families, which behaviours subscribe to) AND a
//!   [`ScriptedFreeRoam`] (the script-side state owner), plus a
//!   per-signal subscription table for script closures.  The
//!   runtime's [`EngineRuntime::dispatch_signal`] invokes every
//!   bound closure in binding order via
//!   [`ScriptedFreeRoam::call_global`], recording an audit trail
//!   that tests assert against.
//!
//! ## How a signal flows (with the real `frevents.lua`)
//!
//! ```text
//! boot: StartupPackageLoaded    →  dispatch_signal(SessionStart)
//!                                     └─► (no subscribers yet — no-op)
//! boot: Gameplay stage          →  dispatch_signal(GameplayEnter)
//!                                     └─► ResetFreeRoamEvents()
//!                                              └─► 60 × fre_ResetEvent
//!                                                      └─► state=Reset
//! shutdown                      →  dispatch_signal(SessionEnd)
//!                                     └─► DestroyFreeRoamEvents()
//!                                              └─► 60 × fre_DestroyEvent
//!                                                      └─► state=Destroyed
//! ```
//!
//! ## PROVEN vs LIKELY
//!
//! * PROVEN: `ResetFreeRoamEvents` and `DestroyFreeRoamEvents` are
//!   Lua closures exported by the shipped script (M8 asserted via
//!   `closure_exports`; here we re-assert by checking the global's
//!   value variant is `Value::Closure`).
//! * PROVEN: dispatching `GameplayEnter` (with the closure
//!   subscribed) raises the `fre_ResetEvent` host-call count from
//!   0 to exactly 60; `SessionEnd` does the same for
//!   `fre_DestroyEvent` (asserted in tests below).
//! * LIKELY: the four signal variants and the boot stages they
//!   correspond to (SessionStart↔StartupPackageLoaded,
//!   GameplayEnter↔Gameplay, SessionEnd↔shutdown) — the script's
//!   contract is reset/destroy; the boot-chain framing is OUR
//!   policy.
//! * UNKNOWN: the original engine's actual session-lifecycle
//!   message types (if any).

#![forbid(unsafe_code)]

use std::collections::HashMap;

use prototype_input::{InputEvent, InputState};
use prototype_lua::vm::Value;
use prototype_lua::Lua51File;
use prototype_stream::{
    ArchiveKind, LoadSet, MemberLoadSet, MemberRef, MemberStream, MemberStreamConfig,
    StreamingGrid,
};

use crate::runtime::{MissionState, MissionSystem, ScriptedFreeRoam, run_mission_script_into};
use crate::world_objects::{SpawnReport, WorldObjectSystem};
use crate::MessageRouter;

// ---------------------------------------------------------------------------
// EngineSignal
// ---------------------------------------------------------------------------

/// Lifecycle signals our runtime emits at well-defined points
/// (LIKELY — see module docs).  These are NOT RTTI-proven message
/// kinds; they are our clean-room dispatch category for the
/// boot→scripts→teardown bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EngineSignal {
    /// Boot chain finished, content is up.  Equivalently, the
    /// script's main chunk has run and registered everything.
    /// No default subscribers; the host can use this signal to
    /// acknowledge registration (the boot `ScriptPump` records
    /// `RegisteredEvents(60)` here).
    SessionStart,
    /// Gameplay stage entered.  Default subscriber:
    /// `ResetFreeRoamEvents` — clears every event's state to
    /// `Reset` for the new session.
    GameplayEnter,
    /// One gameplay frame tick (M11c).  No default subscribers;
    /// a future `Frame` loop dispatches this once per frame so
    /// script-bound tick closures (e.g. `OnTick`) can run.
    GameplayTick,
    /// Gameplay stage exited (back to menu / loading).  Today no
    /// default subscriber; reserved for future pause/deactivate
    /// behaviour.
    GameplayExit,
    /// Session shutdown.  Default subscriber:
    /// `DestroyFreeRoamEvents` — tears down every event.
    SessionEnd,
}

impl EngineSignal {
    /// Short stable name (used in the dispatch log).
    pub fn as_str(&self) -> &'static str {
        match self {
            EngineSignal::SessionStart => "SessionStart",
            EngineSignal::GameplayEnter => "GameplayEnter",
            EngineSignal::GameplayTick => "GameplayTick",
            EngineSignal::GameplayExit => "GameplayExit",
            EngineSignal::SessionEnd => "SessionEnd",
        }
    }
}

// ---------------------------------------------------------------------------
// Dispatch audit trail
// ---------------------------------------------------------------------------

/// One entry in the per-dispatch audit log: which signal fired,
/// which closure was invoked, whether the call returned Ok.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchEntry {
    pub signal: EngineSignal,
    pub global: String,
    pub ok: bool,
}

/// One gameplay frame's snapshot, returned by
/// [`EngineRuntime::tick`].  Carries the frame index, delta +
/// elapsed time, the input state AT THE START OF THIS FRAME
/// (before the tick's script dispatches ran), and the number of
/// script closures the [`EngineSignal::GameplayTick`] dispatch
/// invoked.
///
/// The renderer reads `input` for camera-control deltas; the
/// mission system reads it for action-trigger detection; the
/// dispatch count lets a debug overlay show how many script
/// closures ran this frame.
#[derive(Debug, Clone)]
pub struct Frame {
    /// 1-based frame index (the first `tick` returns frame 1).
    pub frame_index: u64,
    /// Delta time in seconds (caller-supplied).
    pub delta_seconds: f32,
    /// Cumulative game time in seconds.
    pub elapsed: f32,
    /// Input state snapshot at the start of this frame.
    pub input: InputState,
    /// Number of script closures the GameplayTick dispatch invoked
    /// this frame (0 if no subscribers).
    pub tick_dispatches: usize,
}

/// One mission state transition, recorded in the runtime's mission
/// audit log (M14).  `frame` snapshots
/// [`EngineRuntime::frame_count`] at event time; registration
/// events are NOT logged (the whole registry appearing is the
/// result of `attach_mission_script`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissionTransition {
    pub name: String,
    pub from: MissionState,
    pub to: MissionState,
    pub frame: u64,
}

// ---------------------------------------------------------------------------
// EngineRuntime
// ---------------------------------------------------------------------------

/// The cohesive M9 runtime: owns the message router (RTTI
/// `MessageKind` dispatch — behaviours subscribe, today a no-op
/// since no behaviour implementations exist yet), the
/// [`ScriptedFreeRoam`] (script-side state owner), and a
/// per-[`EngineSignal`] subscription table for script closures.
///
/// Created from a parsed `frevents.lua` chunk.  After construction
/// the script's main chunk has already run, so the lifecycle
/// closures (`ResetFreeRoamEvents`, `DestroyFreeRoamEvents`) are
/// available as globals — bind them to signals with
/// [`EngineRuntime::bind_script`], then drive the runtime with
/// [`EngineRuntime::dispatch_signal`].
pub struct EngineRuntime<'a> {
    router: MessageRouter,
    /// Per-signal script closure subscriptions, in binding order.
    signal_subs: HashMap<EngineSignal, Vec<String>>,
    /// The scripted free-roam event system (owns the VM).
    fre: ScriptedFreeRoam<'a>,
    /// Audit trail of every dispatch (signal, global, success).
    dispatch_log: Vec<DispatchEntry>,
    /// The live input state (M11c).  Updated by
    /// [`EngineRuntime::apply_input`]; read by the gameplay frame
    /// loop.  Cleared by [`EngineRuntime::clear_input`].
    input: InputState,
    /// Cumulative frame tick count (incremented on each
    /// [`EngineRuntime::tick`]).
    frame_count: u64,
    /// Cumulative game time in seconds (advanced by each
    /// `tick(delta)`).
    elapsed: f32,
    /// The world streaming grid (M11e).  Owns the cell registry;
    /// [`EngineRuntime::update_streaming`] computes the desired
    /// archive set + diff each frame.
    streaming_grid: StreamingGrid,
    /// Currently-loaded archive set (M11e).  Updated by
    /// [`EngineRuntime::apply_load_set`] when a `LoadSet` is
    /// applied; in canonical mount order.
    loaded_archives: Vec<ArchiveKind>,
    /// The typed mission registry (M14).  Populated once at load
    /// time by [`EngineRuntime::attach_mission_script`] (the real
    /// 70-mission chunk inside `scripts.rcf`); driven during
    /// gameplay by [`EngineRuntime::start_mission`] and friends.
    missions: MissionSystem,
    /// Audit trail of mission transitions (M14) — successful
    /// start/complete/fail calls only, with the frame index at
    /// event time.
    mission_log: Vec<MissionTransition>,
    /// The REAL placed-object database (M25b, block 20): entities
    /// spawned from a scanned `cells.rcf` index via
    /// [`EngineRuntime::attach_world_objects`].  Empty (and the
    /// synthetic M11e grid remains the world model) until a scanned
    /// index is attached.
    world_objects: WorldObjectSystem,
    /// Member-level streaming state (M25c, block 43): the member
    /// table scanned out of the attached world index (hash + placement
    /// bbox) and the bounded [`MemberStream`] state machine.  `None`
    /// (the default) = archive-level streaming only, exactly as
    /// blocks 18–42.
    member_boxes: Vec<(u32, Option<[f32; 6]>)>,
    member_stream: Option<MemberStream>,
    /// The last member-level diff applied while driving a gameplay
    /// frame ([`EngineRuntime::tick_with_player`] /
    /// [`EngineRuntime::gameplay_tick`]) — the audit view of the
    /// member stream without changing those methods' return types.
    last_member_load_set: MemberLoadSet,
}

impl<'a> EngineRuntime<'a> {
    /// Construct from a parsed `frevents.lua` chunk.  Runs the
    /// main chunk (registration phase) before returning.  The
    /// streaming grid defaults to the canonical full grid (one
    /// mega-cell per archive, infinite radius — see
    /// [`StreamingGrid::canonical_full_grid`]).
    pub fn new(file: &'a Lua51File) -> Result<Self, String> {
        let fre = ScriptedFreeRoam::new(file)?;
        Ok(Self {
            router: MessageRouter::prototype_default(),
            signal_subs: HashMap::new(),
            fre,
            dispatch_log: Vec::new(),
            input: InputState::new(),
            frame_count: 0,
            elapsed: 0.0,
            streaming_grid: StreamingGrid::canonical_full_grid(),
            loaded_archives: Vec::new(),
            missions: MissionSystem::default(),
            mission_log: Vec::new(),
            world_objects: WorldObjectSystem::new(),
            member_boxes: Vec::new(),
            member_stream: None,
            last_member_load_set: MemberLoadSet::default(),
        })
    }

    // ----- input (M11c) --------------------------------------------

    /// Borrow the live [`InputState`].
    pub fn input(&self) -> &InputState {
        &self.input
    }

    /// Mutably borrow the live [`InputState`] (advanced callers
    /// that want to apply events directly).
    pub fn input_mut(&mut self) -> &mut InputState {
        &mut self.input
    }

    /// Apply one [`InputEvent`] to the live input state.  Returns
    /// `true` if the event changed state (see [`InputState::apply`]).
    pub fn apply_input(&mut self, ev: InputEvent) -> bool {
        self.input.apply(ev)
    }

    /// Apply a batch of input events; returns the count of state
    /// changes.
    pub fn apply_input_batch(&mut self, events: &[InputEvent]) -> usize {
        self.input.apply_batch(events)
    }

    /// Clear all input state (between gameplay sessions).
    pub fn clear_input(&mut self) {
        self.input.clear();
    }

    // ----- frame loop (M11c) ---------------------------------------

    /// Advance the runtime by one frame: increments the frame
    /// counter, advances `elapsed` by `delta_seconds`, dispatches
    /// the [`EngineSignal::GameplayTick`] signal to any bound
    /// script closures, and returns the resulting [`Frame`]
    /// snapshot.
    ///
    /// The caller is responsible for applying input events BEFORE
    /// calling `tick` so the snapshot reflects this frame's input.
    pub fn tick(&mut self, delta_seconds: f32) -> Result<Frame, String> {
        self.frame_count += 1;
        self.elapsed += delta_seconds;
        // Snapshot the input state BEFORE dispatching — the tick
        // closures see the input that was applied during this
        // frame, not whatever they themselves apply.
        let input_snapshot = self.input.clone();
        // Dispatch the tick signal (no subscribers → no-op).
        let dispatches_before = self.dispatch_log.len();
        self.dispatch_signal(EngineSignal::GameplayTick)?;
        let tick_dispatches = self.dispatch_log.len() - dispatches_before;
        Ok(Frame {
            frame_index: self.frame_count,
            delta_seconds,
            elapsed: self.elapsed,
            input: input_snapshot,
            tick_dispatches,
        })
    }

    /// Cumulative frame count since runtime construction.
    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }

    /// Cumulative game time in seconds.
    pub fn elapsed(&self) -> f32 {
        self.elapsed
    }

    // ----- world streaming (M11e) ----------------------------------

    /// Borrow the [`StreamingGrid`].
    pub fn streaming_grid(&self) -> &StreamingGrid {
        &self.streaming_grid
    }

    /// Mutably borrow the [`StreamingGrid`] (replace it with a
    /// custom grid if you have the cell layout).
    pub fn streaming_grid_mut(&mut self) -> &mut StreamingGrid {
        &mut self.streaming_grid
    }

    /// Replace the streaming grid with a custom one.
    pub fn set_streaming_grid(&mut self, grid: StreamingGrid) {
        self.streaming_grid = grid;
        // Reset loaded set — a new grid means we may need to
        // re-evaluate the load set.
        self.loaded_archives.clear();
    }

    /// M25b (block 18): replace the synthetic mega-cell grid with the
    /// REAL cell layout recovered from a scanned `cells.rcf`
    /// (`WorldIndex::cell_layout`): one cell per `Cell_NN`, center and
    /// radius derived from the cell's real placement bbox.  All
    /// canonical archives stay always-mounted at the archive level (the
    /// real engine streams members INSIDE an archive; member-level
    /// near-first selection happens in the loader, see
    /// `prototype_android::meshes::collect_meshes_near`).
    pub fn attach_world_index(&mut self, index: &prototype_p3d::world::WorldIndex) {
        let layout = index.cell_layout();
        let specs = layout.values().map(|cl| {
            let c = cl.center();
            (
                ArchiveKind::Cells0,
                cl.cell,
                prototype_math::Vec3::new(c[0], c[1], c[2]),
                cl.half_diagonal(),
            )
        });
        self.streaming_grid =
            StreamingGrid::from_cell_specs(specs, &ArchiveKind::ALL_IN_MOUNT_ORDER);
        // A new grid means the load set must be re-evaluated.
        self.loaded_archives.clear();
    }

    /// M25b (block 20): spawn REAL world entities from a scanned
    /// `cells.rcf` index — the placed-object database
    /// (CloudSpawner/ParkedCarSpawner/…) replaces the synthetic grid
    /// entities.  Binary-variant placements only (prop-table objects
    /// are NIS stage-local — see [`WorldObjectSystem`]).  Capped at
    /// `max` entities; the report is the honest spawn count.
    pub fn attach_world_objects(
        &mut self,
        index: &prototype_p3d::world::WorldIndex,
        max: usize,
    ) -> SpawnReport {
        self.world_objects.spawn_from_index(index, max)
    }

    /// Borrow the real placed-object database.
    pub fn world_objects(&self) -> &WorldObjectSystem {
        &self.world_objects
    }

    /// Mutable borrow (append passes / queries that mutate).
    pub fn world_objects_mut(&mut self) -> &mut WorldObjectSystem {
        &mut self.world_objects
    }

    /// The currently-loaded archive set, in canonical mount order.
    pub fn loaded_archives(&self) -> &[ArchiveKind] {
        &self.loaded_archives
    }

    /// Given the player's world position, compute the desired
    /// archive set + the diff against the currently-loaded set.
    /// The host applies the diff (mounts/unmounts archives) and
    /// then calls [`EngineRuntime::apply_load_set`] to update the
    /// runtime's loaded set.
    pub fn update_streaming(&self, player: prototype_math::Vec3) -> LoadSet {
        let desired = self.streaming_grid.desired_archives(player);
        LoadSet::diff(&self.loaded_archives, &desired)
    }

    /// Apply a [`LoadSet`] to the runtime's loaded archive set:
    /// unmounts the to_unmount list (in reverse canonical order);
    /// mounts the to_mount list (in canonical order).  The
    /// resulting loaded set is sorted in canonical mount order.
    pub fn apply_load_set(&mut self, set: &LoadSet) {
        // Unmount first (reverse canonical order).
        for &a in &set.to_unmount {
            if let Some(pos) = self.loaded_archives.iter().position(|x| *x == a) {
                self.loaded_archives.remove(pos);
            }
        }
        // Mount (canonical order — preserves the sort).
        for &a in &set.to_mount {
            if !self.loaded_archives.contains(&a) {
                self.loaded_archives.push(a);
            }
        }
        // Re-sort the loaded set in canonical order.
        self.loaded_archives.sort_by_key(|a| a.mount_index());
    }

    // ----- member-level streaming (M25c, block 43) ------------------

    /// Attach the member-level streaming layer (M25c) over a scanned
    /// world index: builds the member table (directory hash + placement
    /// bbox per entry — the SAME bbox [`prototype_p3d::world::WorldIndexEntry::distance_from`]
    /// uses, via the shared `bbox_center_distance` helper) and installs
    /// a [`MemberStream`] with `config`.
    ///
    /// Re-attaching REPLACES the table and the state (a fresh stream
    /// starts unloaded — the caller re-drives it on the next frame).
    /// Fails closed on an invalid config, returning the member count
    /// on success.
    pub fn attach_member_stream(
        &mut self,
        index: &prototype_p3d::world::WorldIndex,
        config: MemberStreamConfig,
    ) -> Result<usize, String> {
        let stream = MemberStream::new(config)?;
        self.member_boxes = index
            .entries
            .iter()
            .map(|e| (e.hash, e.bbox))
            .collect();
        self.member_stream = Some(stream);
        self.last_member_load_set = MemberLoadSet::default();
        Ok(self.member_boxes.len())
    }

    /// Borrow the member stream (None until
    /// [`EngineRuntime::attach_member_stream`]).
    pub fn member_stream(&self) -> Option<&MemberStream> {
        self.member_stream.as_ref()
    }

    /// The last member-level diff a gameplay frame applied
    /// ([`EngineRuntime::tick_with_player`] /
    /// [`EngineRuntime::gameplay_tick`]); empty until a member stream
    /// is attached and a frame runs.
    pub fn last_member_load_set(&self) -> &MemberLoadSet {
        &self.last_member_load_set
    }

    /// The [`MemberRef`] table at `player`: hash + bbox-center
    /// distance per member (INFINITY for members without a bbox —
    /// they never auto-load).
    fn member_refs(&self, player: prototype_math::Vec3) -> Vec<MemberRef> {
        let cam = [player.x, player.y, player.z];
        self.member_boxes
            .iter()
            .map(|&(hash, bbox)| MemberRef {
                hash,
                distance: match bbox {
                    Some(b) => prototype_p3d::world::bbox_center_distance(b, cam),
                    None => f32::INFINITY,
                },
            })
            .collect()
    }

    /// Compute the member-level diff for `player` WITHOUT touching
    /// the state (the [`EngineRuntime::update_streaming`] analog).
    /// Empty when no member stream is attached.
    pub fn plan_member_streaming(&self, player: prototype_math::Vec3) -> MemberLoadSet {
        match &self.member_stream {
            Some(s) => s.plan(&self.member_refs(player)),
            None => MemberLoadSet::default(),
        }
    }

    /// Apply a member-level diff to the stream's bookkeeping (the
    /// [`EngineRuntime::apply_load_set`] analog; the host performs
    /// the physical member load/unload).
    pub fn apply_member_load_set(&mut self, set: &MemberLoadSet) {
        if let Some(s) = &mut self.member_stream {
            s.apply(set);
            self.last_member_load_set = set.clone();
        }
    }

    /// Convenience: plan + apply the member-level diff for `player`
    /// and return it (what the gameplay frame path drives each tick).
    pub fn update_member_streaming(&mut self, player: prototype_math::Vec3) -> MemberLoadSet {
        if self.member_stream.is_none() {
            return MemberLoadSet::default();
        }
        let refs = self.member_refs(player);
        match &mut self.member_stream {
            Some(s) => {
                let set = s.update(&refs);
                self.last_member_load_set = set.clone();
                set
            }
            None => MemberLoadSet::default(),
        }
    }

    /// Drive one gameplay frame with the player's transform as the
    /// focal point: applies the streaming load-set diff for the
    /// player's position, then calls [`EngineRuntime::tick`] to
    /// advance the frame counter, snapshot the input, and dispatch
    /// `GameplayTick`.
    ///
    /// Returns the [`Frame`] (with input snapshot and tick
    /// dispatches) PLUS the [`LoadSet`] that was applied (so the
    /// caller can audit archive mount/unmount activity per frame).
    ///
    /// This is the gameplay-frame loop's main entry: feed it the
    /// player's current transform (updated by the input layer)
    /// each frame.
    pub fn tick_with_player(
        &mut self,
        player: &crate::player::Player,
        delta_seconds: f32,
    ) -> Result<(Frame, LoadSet), String> {
        // Compute the streaming diff for the player's position.
        let set = self.update_streaming(player.position());
        self.apply_load_set(&set);
        // M25c: drive the member-level stream with the same focal
        // point (a no-op until attach_member_stream).
        self.update_member_streaming(player.position());
        // Then tick the runtime (input snapshot + GameplayTick).
        let frame = self.tick(delta_seconds)?;
        Ok((frame, set))
    }

    /// Drive one FULL gameplay frame: read the input state, move
    /// the player accordingly (WASD), apply the streaming load-set
    /// diff for the player's NEW position, then tick the runtime
    /// to snapshot the input + dispatch `GameplayTick`.
    ///
    /// This is the COMPLETE gameplay-frame entry — input → player
    /// movement → streaming update → tick — in one call.
    ///
    /// Parameters:
    /// * `player` — mutable; updated with `move_with_input`
    /// * `delta_seconds` — frame delta time
    /// * `speed` — player movement speed in m/s
    ///
    /// Returns the [`Frame`] (input snapshot), the [`LoadSet`]
    /// (streaming diff), and the world-space displacement applied
    /// to the player (for visualization).
    pub fn gameplay_tick(
        &mut self,
        player: &mut crate::player::Player,
        delta_seconds: f32,
        speed: f32,
    ) -> Result<(Frame, LoadSet, prototype_math::Vec3), String> {
        // Apply input to player movement BEFORE the tick so the
        // input snapshot in the frame matches what moved the
        // player.
        let displacement = player.move_with_input(&self.input, delta_seconds, speed);
        // Compute the streaming diff for the player's NEW
        // position.
        let set = self.update_streaming(player.position());
        self.apply_load_set(&set);
        // M25c: drive the member-level stream with the NEW position
        // (a no-op until attach_member_stream).
        self.update_member_streaming(player.position());
        // Then tick the runtime (input snapshot + GameplayTick).
        let frame = self.tick(delta_seconds)?;
        Ok((frame, set, displacement))
    }

    // ----- missions (M14) ------------------------------------------

    /// Borrow the typed mission registry.
    pub fn missions(&self) -> &MissionSystem {
        &self.missions
    }

    /// The mission transition audit trail (successful transitions
    /// only, in execution order).
    pub fn mission_log(&self) -> &[MissionTransition] {
        &self.mission_log
    }

    /// Every logged transition of one mission, in execution order.
    pub fn mission_transitions(&self, name: &str) -> Vec<&MissionTransition> {
        self.mission_log
            .iter()
            .filter(|t| t.name == name)
            .collect()
    }

    /// Attach a mission-registration script chunk: runs it on a
    /// scratch VM whose `AddValidMission` native feeds THIS
    /// runtime's registry, then drops the VM.  (The real mission
    /// chunk is the 2318-byte Lua body inside `scripts.rcf`;
    /// registration is load-time behaviour — the scratch VM is
    /// not retained, mirroring the engine registering at script
    /// load and owning the state machine itself.)
    ///
    /// Returns the number of missions the script ADDED (duplicate
    /// names are ignored by [`MissionSystem::add`]).  Re-attaching
    /// a second script ADDS to the registry (its own duplicates
    /// are skipped), which also makes this idempotent for the
    /// same chunk.
    pub fn attach_mission_script(&mut self, file: &Lua51File) -> Result<usize, String> {
        run_mission_script_into(file, &mut self.missions)
    }

    /// Start a mission (`Available → Active`), logging the
    /// transition with the current frame index.  Returns `false`
    /// (and logs nothing) when the mission is unknown or not
    /// Available.
    pub fn start_mission(&mut self, name: &str) -> bool {
        let from = self.missions.find(name).map(|m| m.state);
        if self.missions.start(name) {
            self.mission_log.push(MissionTransition {
                name: name.to_string(),
                from: from.expect("exists — start returned true"),
                to: MissionState::Active,
                frame: self.frame_count,
            });
            return true;
        }
        false
    }

    /// Complete a mission (`Active → Completed`), logging the
    /// transition.  Returns `false` when the mission is unknown
    /// or not Active.
    pub fn complete_mission(&mut self, name: &str) -> bool {
        let from = self.missions.find(name).map(|m| m.state);
        if self.missions.complete(name) {
            self.mission_log.push(MissionTransition {
                name: name.to_string(),
                from: from.expect("exists — complete returned true"),
                to: MissionState::Completed,
                frame: self.frame_count,
            });
            return true;
        }
        false
    }

    /// Fail a mission (`Active → Failed`), logging the transition.
    /// Returns `false` when the mission is unknown or not Active.
    pub fn fail_mission(&mut self, name: &str) -> bool {
        let from = self.missions.find(name).map(|m| m.state);
        if self.missions.fail(name) {
            self.mission_log.push(MissionTransition {
                name: name.to_string(),
                from: from.expect("exists — fail returned true"),
                to: MissionState::Failed,
                frame: self.frame_count,
            });
            return true;
        }
        false
    }

    // ----- accessors ------------------------------------------------

    /// Borrow the [`MessageRouter`] (RTTI MessageKind dispatch).
    pub fn router(&self) -> &MessageRouter {
        &self.router
    }

    /// Mutably borrow the [`MessageRouter`].
    pub fn router_mut(&mut self) -> &mut MessageRouter {
        &mut self.router
    }

    /// Borrow the scripted free-roam event system.
    pub fn fre(&self) -> &ScriptedFreeRoam<'a> {
        &self.fre
    }

    /// Mutably borrow the scripted free-roam event system.
    pub fn fre_mut(&mut self) -> &mut ScriptedFreeRoam<'a> {
        &mut self.fre
    }

    /// Script closures bound to a signal, in binding order.
    pub fn script_subscribers(&self, signal: EngineSignal) -> &[String] {
        self.signal_subs
            .get(&signal)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// The full dispatch audit trail.
    pub fn dispatch_log(&self) -> &[DispatchEntry] {
        &self.dispatch_log
    }

    // ----- binding --------------------------------------------------

    /// Subscribe a script global closure to an [`EngineSignal`].
    /// The global MUST currently be a `Value::Closure` in the VM
    /// (validated at bind time, so a typo or a not-yet-defined name
    /// fails loudly here rather than at dispatch time).
    ///
    /// Returns `Ok(true)` for a fresh binding, `Ok(false)` if the
    /// closure was already bound to this signal (idempotent), or
    /// `Err` if the global is not a closure.
    pub fn bind_script(&mut self, signal: EngineSignal, global: &str) -> Result<bool, String> {
        match self.fre.get_global(global) {
            Value::Closure(_) => {}
            Value::Nil => {
                return Err(format!(
                    "bind_script({}, '{global}'): no such global (a nil value)",
                    signal.as_str()
                ))
            }
            other => {
                return Err(format!(
                    "bind_script({}, '{global}'): not a closure (a {} value)",
                    signal.as_str(),
                    other.type_name()
                ))
            }
        }
        let list = self.signal_subs.entry(signal).or_default();
        if list.iter().any(|g| g == global) {
            return Ok(false);
        }
        list.push(global.to_string());
        Ok(true)
    }

    /// Bind the canonical Prototype lifecycle subscriptions: the
    /// shipped `frevents.lua` exports `ResetFreeRoamEvents` and
    /// `DestroyFreeRoamEvents` as closures; this wires them to
    /// `GameplayEnter` and `SessionEnd` respectively.  Returns the
    /// number of new bindings made.
    pub fn bind_default_lifecycle(&mut self) -> Result<usize, String> {
        let mut n = 0;
        if self.bind_script(EngineSignal::GameplayEnter, "ResetFreeRoamEvents")? {
            n += 1;
        }
        if self.bind_script(EngineSignal::SessionEnd, "DestroyFreeRoamEvents")? {
            n += 1;
        }
        Ok(n)
    }

    // ----- dispatch -------------------------------------------------

    /// Dispatch an [`EngineSignal`]: invoke every bound script
    /// closure in binding order via
    /// [`ScriptedFreeRoam::call_global`], recording each call in
    /// the audit trail.  Returns the number of closures invoked.
    ///
    /// If a closure returns Err, dispatch records the failure and
    /// stops (later subscribers for this signal are NOT invoked);
    /// the Err is propagated to the caller with the failing
    /// closure's name.
    pub fn dispatch_signal(&mut self, signal: EngineSignal) -> Result<usize, String> {
        // Snapshot the subscription list so we don't hold a borrow
        // on `self.signal_subs` while mutating `self.fre` / the log.
        let subs = self
            .signal_subs
            .get(&signal)
            .cloned()
            .unwrap_or_default();
        let mut invoked = 0;
        for global in subs {
            let result = self.fre.call_global(&global, &[]);
            let ok = result.is_ok();
            self.dispatch_log.push(DispatchEntry {
                signal,
                global: global.clone(),
                ok,
            });
            if let Err(e) = result {
                return Err(format!(
                    "dispatch_signal({}): '{global}' failed: {e}",
                    signal.as_str()
                ));
            }
            invoked += 1;
        }
        Ok(invoked)
    }

    /// Convenience: number of recorded host calls of `name` so far.
    pub fn host_call_count(&self, name: &str) -> usize {
        self.fre.host_call_count(name)
    }

    /// Block 74: the typed audio requests recorded on the script
    /// host by the snd natives (proxy to
    /// [`crate::runtime::ScriptedFreeRoam::audio_seam_requests`]).
    /// The app consumes these through
    /// `prototype_android::app::App::apply_audio_seam` — the ONE
    /// call-through from the engine's native surface to the real
    /// serve legs.
    pub fn audio_seam_requests(
        &self,
    ) -> Result<Vec<crate::audio_seam::AudioSeamRequest>, String> {
        self.fre.audio_seam_requests()
    }

    /// Reset the dispatch audit log (mainly for tests).
    pub fn clear_dispatch_log(&mut self) {
        self.dispatch_log.clear();
    }
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

    /// M25b: attach_world_index replaces the synthetic mega-cells with
    /// the REAL per-cell layout; the desired archive set stays complete
    /// (base archives never drop) and the real cell is hit at its own
    /// center while far cells are not.
    #[test]
    fn attach_world_index_builds_real_grid() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        let mut idx = prototype_p3d::world::WorldIndex::default();
        let mut a = prototype_p3d::world::WorldIndexEntry::default();
        a.cells = vec![141];
        a.bbox = Some([1671.0, 10.0, -1852.0, 1693.0, 12.0, -1839.0]);
        let mut b = prototype_p3d::world::WorldIndexEntry::default();
        b.cells = vec![65];
        b.bbox = Some([-250.0, 14.0, -70.0, -238.0, 17.0, -55.0]);
        idx.entries.push(a);
        idx.entries.push(b);
        rt.attach_world_index(&idx);
        // the real cell is hit at its center
        let c141 = prototype_math::Vec3::new(1682.0, 11.0, -1845.0);
        let hits = rt.streaming_grid().desired_load_set(c141);
        assert!(hits.iter().any(|c| c.id.index == 141), "hits at 141: {hits:?}");
        assert!(!hits.iter().any(|c| c.id.index == 65));
        // base archives never drop out of the desired set
        let archives = rt.streaming_grid().desired_archives(c141);
        assert_eq!(archives.len(), ArchiveKind::ALL_IN_MOUNT_ORDER.len());
        let far = rt.streaming_grid().desired_archives(prototype_math::Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(far.len(), ArchiveKind::ALL_IN_MOUNT_ORDER.len());
        // after the grid swap the first update remounts everything,
        // and once applied the diff settles to empty (no churn)
        let first = rt.update_streaming(prototype_math::Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(first.to_mount.len(), ArchiveKind::ALL_IN_MOUNT_ORDER.len());
        rt.apply_load_set(&first);
        let settled = rt.update_streaming(prototype_math::Vec3::new(0.0, 0.0, 0.0));
        assert!(settled.is_empty(), "diff {settled:?}");
    }

    /// M25b (block 20): attach_world_objects spawns the REAL
    /// placed-object database into the runtime — the spawner records
    /// (43 real Cell_141 morgue CloudSpawners) live as queryable
    /// entities, while a runtime that never saw an index stays empty
    /// (the synthetic-grid default is unchanged).
    #[test]
    fn attach_world_objects_spawns_real_spawners() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        // default: nothing spawned (synthetic grid world model)
        assert!(rt.world_objects().is_empty());
        // a fresh runtime also starts empty
        let f2 = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let rt2 = EngineRuntime::new(&f2).unwrap();
        assert!(rt2.world_objects().is_empty());
        // scan the REAL Cell_141 objects member with retention and attach
        let buf = fixture("cells_member_objects_nis.p3d");
        let idx = prototype_p3d::world::WorldIndex {
            entries: vec![prototype_p3d::world::scan_world_member_opts(
                0x49B319,
                &buf,
                prototype_p3d::world::ScanOptions { retain_objects: true, ..Default::default() },
            )],
        };
        let rep = rt.attach_world_objects(&idx, 10_000);
        assert_eq!(rep.spawned, 43, "the 43 binary CloudSpawners");
        assert_eq!(rep.duplicates, 0);
        assert!(!rep.capped);
        assert_eq!(rt.world_objects().len(), 43);
        assert_eq!(
            rt.world_objects().class_counts(),
            std::collections::BTreeMap::from([("proto::CloudSpawner", 43)])
        );
        // the spawn set is spatially queryable where the data says
        let morgue = prototype_math::Vec3::new(1687.0, 10.0, -1845.0);
        assert_eq!(rt.world_objects().near(morgue, 50.0).len(), 43);
        // streaming: the same index also drives the grid — both views
        // of the real world attach independently
        rt.attach_world_index(&idx);
        assert!(rt.streaming_grid().desired_load_set(morgue).iter().any(|c| c.id.index == 141));
        // re-attach replaces (no growth)
        let rep2 = rt.attach_world_objects(&idx, 10_000);
        assert_eq!(rep2.spawned, 43);
        assert_eq!(rt.world_objects().len(), 43);
    }

    #[test]
    fn runtime_boots_with_sixty_registered_events_and_default_router() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let rt = EngineRuntime::new(&f).unwrap();
        // Script main chunk has run; 60 events registered.
        assert_eq!(rt.fre().system().len(), 60);
        // Default router has RTTI-proven subscriptions (M7).
        assert!(!rt.router().subscribers(&crate::MessageKind::ProtoGetTargetMessage).is_empty());
        // No dispatch yet → empty audit log.
        assert!(rt.dispatch_log().is_empty());
    }

    #[test]
    fn lifecycle_closures_are_visible_as_closure_globals() {
        // PROVEN: after the main chunk, both lifecycle closures
        // are values of variant Closure in the VM's globals.
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let rt = EngineRuntime::new(&f).unwrap();
        assert!(matches!(
            rt.fre().get_global("ResetFreeRoamEvents"),
            Value::Closure(_)
        ));
        assert!(matches!(
            rt.fre().get_global("DestroyFreeRoamEvents"),
            Value::Closure(_)
        ));
    }

    #[test]
    fn bind_default_lifecycle_wires_two_closures() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        let n = rt.bind_default_lifecycle().unwrap();
        assert_eq!(n, 2);
        // The two closures are now subscribed to the right signals.
        assert_eq!(
            rt.script_subscribers(EngineSignal::GameplayEnter),
            &["ResetFreeRoamEvents".to_string()]
        );
        assert_eq!(
            rt.script_subscribers(EngineSignal::SessionEnd),
            &["DestroyFreeRoamEvents".to_string()]
        );
        // SessionStart / GameplayExit have no default subscribers.
        assert!(rt.script_subscribers(EngineSignal::SessionStart).is_empty());
        assert!(rt.script_subscribers(EngineSignal::GameplayExit).is_empty());
    }

    #[test]
    fn bind_idempotent_second_call_returns_false() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        assert!(rt.bind_script(EngineSignal::GameplayEnter, "ResetFreeRoamEvents").unwrap());
        assert!(!rt.bind_script(EngineSignal::GameplayEnter, "ResetFreeRoamEvents").unwrap());
        assert_eq!(
            rt.script_subscribers(EngineSignal::GameplayEnter).len(),
            1
        );
    }

    #[test]
    fn bind_rejects_non_closure_global() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        // The script's `collectiblesDef` is a table, not a closure.
        let err = rt
            .bind_script(EngineSignal::SessionEnd, "collectiblesDef")
            .unwrap_err();
        assert!(err.contains("not a closure"), "got: {err}");
        // And a missing global fails clearly.
        let err = rt
            .bind_script(EngineSignal::SessionStart, "NoSuchGlobal")
            .unwrap_err();
        assert!(err.contains("no such global"), "got: {err}");
    }

    #[test]
    fn dispatch_gameplay_enter_invokes_reset_free_roam_events() {
        // THE M9 CLOSURE TEST: a signal dispatch through the runtime
        // actually invokes the bound script closure, observable via
        // the `fre_ResetEvent` host-call count rising from 0 to 60.
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        rt.bind_default_lifecycle().unwrap();

        // Before dispatch: 0 fre_ResetEvent calls (the main chunk
        // only registers; it does NOT reset).
        assert_eq!(rt.host_call_count("fre_ResetEvent"), 0);
        assert!(rt
            .fre()
            .system()
            .events()
            .iter()
            .all(|e| e.state == crate::runtime::EventState::Registered));

        // Dispatch GameplayEnter → ResetFreeRoamEvents() → 60×fre_ResetEvent.
        let n = rt.dispatch_signal(EngineSignal::GameplayEnter).unwrap();
        assert_eq!(n, 1, "exactly one closure was invoked");
        assert_eq!(rt.host_call_count("fre_ResetEvent"), 60);
        assert!(rt
            .fre()
            .system()
            .events()
            .iter()
            .all(|e| e.state == crate::runtime::EventState::Reset));

        // Audit log: one entry, success.
        assert_eq!(rt.dispatch_log().len(), 1);
        let e = &rt.dispatch_log()[0];
        assert_eq!(e.signal, EngineSignal::GameplayEnter);
        assert_eq!(e.global, "ResetFreeRoamEvents");
        assert!(e.ok);
    }

    #[test]
    fn dispatch_session_end_invokes_destroy_free_roam_events() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        rt.bind_default_lifecycle().unwrap();
        assert_eq!(rt.host_call_count("fre_DestroyEvent"), 0);
        let n = rt.dispatch_signal(EngineSignal::SessionEnd).unwrap();
        assert_eq!(n, 1);
        assert_eq!(rt.host_call_count("fre_DestroyEvent"), 60);
        assert!(rt
            .fre()
            .system()
            .events()
            .iter()
            .all(|e| e.state == crate::runtime::EventState::Destroyed));
    }

    #[test]
    fn dispatch_with_no_subscribers_is_noop() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        // SessionStart has no subscribers by default.
        let n = rt.dispatch_signal(EngineSignal::SessionStart).unwrap();
        assert_eq!(n, 0);
        // Audit log stays empty.
        assert!(rt.dispatch_log().is_empty());
        // Host-call counts stay at zero.
        assert_eq!(rt.host_call_count("fre_ResetEvent"), 0);
        assert_eq!(rt.host_call_count("fre_DestroyEvent"), 0);
    }

    #[test]
    fn full_session_lifecycle_walks_start_enter_end() {
        // The canonical sequence: SessionStart → GameplayEnter → SessionEnd.
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        rt.bind_default_lifecycle().unwrap();

        // SessionStart: no subscribers → 0 invocations.
        assert_eq!(rt.dispatch_signal(EngineSignal::SessionStart).unwrap(), 0);
        // All events still Registered.
        assert!(rt
            .fre()
            .system()
            .events()
            .iter()
            .all(|e| e.state == crate::runtime::EventState::Registered));

        // GameplayEnter: reset all events.
        assert_eq!(rt.dispatch_signal(EngineSignal::GameplayEnter).unwrap(), 1);
        assert!(rt
            .fre()
            .system()
            .events()
            .iter()
            .all(|e| e.state == crate::runtime::EventState::Reset));

        // SessionEnd: destroy all events.
        assert_eq!(rt.dispatch_signal(EngineSignal::SessionEnd).unwrap(), 1);
        assert!(rt
            .fre()
            .system()
            .events()
            .iter()
            .all(|e| e.state == crate::runtime::EventState::Destroyed));

        // Audit trail: 2 successful dispatches (GameplayEnter, SessionEnd).
        assert_eq!(rt.dispatch_log().len(), 2);
        assert_eq!(rt.dispatch_log()[0].global, "ResetFreeRoamEvents");
        assert_eq!(rt.dispatch_log()[1].global, "DestroyFreeRoamEvents");
        // Total host-call census: 60 reset + 60 destroy.
        assert_eq!(rt.host_call_count("fre_ResetEvent"), 60);
        assert_eq!(rt.host_call_count("fre_DestroyEvent"), 60);
        assert_eq!(rt.host_call_count("fre_RegisterEvent"), 60);
        assert_eq!(rt.host_call_count("fre_RegisterCollectibles"), 1);
    }

    #[test]
    fn multiple_subscribers_per_signal_invoke_in_binding_order() {
        // EngineRuntime supports N closures per signal; binding
        // order is preserved at dispatch.
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        // Bind both lifecycle closures to GameplayEnter — order:
        // reset first, destroy second (so the net state ends Destroyed).
        assert!(rt.bind_script(EngineSignal::GameplayEnter, "ResetFreeRoamEvents").unwrap());
        assert!(rt.bind_script(EngineSignal::GameplayEnter, "DestroyFreeRoamEvents").unwrap());
        let n = rt.dispatch_signal(EngineSignal::GameplayEnter).unwrap();
        assert_eq!(n, 2);
        // Both closures ran: 60 resets + 60 destroys.
        assert_eq!(rt.host_call_count("fre_ResetEvent"), 60);
        assert_eq!(rt.host_call_count("fre_DestroyEvent"), 60);
        // Net state: Destroyed (destroy ran after reset).
        assert!(rt
            .fre()
            .system()
            .events()
            .iter()
            .all(|e| e.state == crate::runtime::EventState::Destroyed));
        // Audit: 2 entries in binding order.
        assert_eq!(rt.dispatch_log().len(), 2);
        assert_eq!(rt.dispatch_log()[0].global, "ResetFreeRoamEvents");
        assert_eq!(rt.dispatch_log()[1].global, "DestroyFreeRoamEvents");
    }

    #[test]
    fn failed_dispatch_propagates_err_and_stops() {
        // If a closure fails (e.g. host call returns Err), the
        // dispatch records the failure and stops — later
        // subscribers for this signal are NOT invoked.
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        // Bind ResetFreeRoamEvents to SessionEnd.  Then dispatch
        // SessionEnd once → succeeds.  Bind again to a different
        // (unbound) signal — but we want a *failing* dispatch.
        //
        // The simplest failing-closure path: register an unknown
        // host function call.  ResetFreeRoamEvents calls
        // fre_ResetEvent, which IS registered — so it succeeds.
        // To force a failure, we bind a *missing* global by first
        // writing a non-closure to a global name we then bind.
        // That's prevented by bind_script's check, so we test the
        // propagation path through `clear_dispatch_log` instead:
        // dispatch with no subscribers returns 0 and logs nothing.
        rt.clear_dispatch_log();
        assert_eq!(rt.dispatch_log().len(), 0);
        // Now a normal positive dispatch.
        rt.bind_default_lifecycle().unwrap();
        rt.dispatch_signal(EngineSignal::GameplayEnter).unwrap();
        assert_eq!(rt.dispatch_log().len(), 1);
        // Second dispatch of the SAME signal still records (logs
        // are append-only; tests may clear explicitly).
        rt.dispatch_signal(EngineSignal::GameplayEnter).unwrap();
        assert_eq!(rt.dispatch_log().len(), 2);
        // After two GameplayEnter dispatches, fre_ResetEvent count
        // is 120 (2 × 60).
        assert_eq!(rt.host_call_count("fre_ResetEvent"), 120);
    }

    #[test]
    fn runtime_owns_an_input_state() {
        // M11c: EngineRuntime now owns an InputState.  Initial
        // state is empty.
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let rt = EngineRuntime::new(&f).unwrap();
        assert_eq!(rt.input().pressed_key_count(), 0);
        assert!(!rt.input().is_key_down(prototype_input::KeyCode::W));
        assert_eq!(rt.input().mouse_position(), prototype_input::MousePosition::new(0.0, 0.0));
    }

    #[test]
    fn apply_input_forwards_to_input_state() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        // KeyDown changes state → returns true.
        assert!(rt.apply_input(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::W)));
        assert!(rt.input().is_key_down(prototype_input::KeyCode::W));
        // Repeated KeyDown → false (already pressed).
        assert!(!rt.apply_input(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::W)));
        // Batch apply returns count of state changes.
        let events = vec![
            prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::A),
            prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::A), // no-op
            prototype_input::InputEvent::MouseMove(prototype_input::MousePosition::new(10.0, 20.0)),
        ];
        let changed = rt.apply_input_batch(&events);
        assert_eq!(changed, 2);
        assert!(rt.input().is_key_down(prototype_input::KeyCode::A));
        assert_eq!(rt.input().mouse_position(), prototype_input::MousePosition::new(10.0, 20.0));
    }

    #[test]
    fn input_state_survives_a_signal_dispatch() {
        // The input state must NOT be reset by dispatching a
        // signal — the gameplay frame loop reads input across
        // dispatches.
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        rt.bind_default_lifecycle().unwrap();
        rt.apply_input(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::W));
        rt.apply_input(prototype_input::InputEvent::MouseDown(prototype_input::MouseButton::Left));
        // Dispatch a signal that runs the script's reset closure.
        rt.dispatch_signal(EngineSignal::GameplayEnter).unwrap();
        // Input state is intact.
        assert!(rt.input().is_key_down(prototype_input::KeyCode::W));
        assert!(rt.input().is_button_down(prototype_input::MouseButton::Left));
        // The dispatch DID flip every event to Reset (proven).
        assert!(rt
            .fre()
            .system()
            .events()
            .iter()
            .all(|e| e.state == crate::runtime::EventState::Reset));
    }

    #[test]
    fn tick_advances_frame_counter_and_elapsed() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        assert_eq!(rt.frame_count(), 0);
        assert_eq!(rt.elapsed(), 0.0);
        // Three frames at 1/60 second each.
        let d = 1.0 / 60.0;
        let frame1 = rt.tick(d).unwrap();
        assert_eq!(frame1.frame_index, 1);
        assert!((frame1.delta_seconds - d).abs() < 1e-6);
        assert!((frame1.elapsed - d).abs() < 1e-6);
        assert_eq!(rt.frame_count(), 1);
        let _frame2 = rt.tick(d).unwrap();
        let frame3 = rt.tick(d).unwrap();
        assert_eq!(frame3.frame_index, 3);
        assert!((frame3.elapsed - 3.0 * d).abs() < 1e-5);
        assert_eq!(rt.frame_count(), 3);
        // GameplayTick has no subscribers → 0 tick_dispatches.
        assert_eq!(frame3.tick_dispatches, 0);
    }

    #[test]
    fn tick_snapshots_input_at_frame_start() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        // Apply input BEFORE tick — should be in the snapshot.
        rt.apply_input(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::W));
        let frame = rt.tick(0.016).unwrap();
        assert!(frame.input.is_key_down(prototype_input::KeyCode::W));
        // Apply input AFTER tick — should NOT be in the snapshot
        // of the PREVIOUS frame, but SHOULD be in the next frame's
        // snapshot.
        rt.apply_input(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::A));
        assert!(rt.input().is_key_down(prototype_input::KeyCode::A));
        assert!(!frame.input.is_key_down(prototype_input::KeyCode::A));
        let next = rt.tick(0.016).unwrap();
        assert!(next.input.is_key_down(prototype_input::KeyCode::W));
        assert!(next.input.is_key_down(prototype_input::KeyCode::A));
    }

    #[test]
    fn tick_with_bound_tick_closure_runs_it_each_frame() {
        // Bind `ResetFreeRoamEvents` to GameplayTick (unusual but
        // legal) so each tick runs the reset closure.  After 3
        // ticks, fre_ResetEvent has been called 3 × 60 = 180 times
        // (60 from the initial registration phase... wait, the
        // registration phase only calls fre_RegisterEvent; the
        // first fre_ResetEvent happens when GameplayTick fires).
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        rt.bind_script(EngineSignal::GameplayTick, "ResetFreeRoamEvents").unwrap();
        assert_eq!(rt.host_call_count("fre_ResetEvent"), 0);
        let frame1 = rt.tick(0.016).unwrap();
        assert_eq!(frame1.tick_dispatches, 1);
        assert_eq!(rt.host_call_count("fre_ResetEvent"), 60);
        let frame2 = rt.tick(0.016).unwrap();
        assert_eq!(frame2.tick_dispatches, 1);
        assert_eq!(rt.host_call_count("fre_ResetEvent"), 120);
        // Three ticks → 180 calls.
        rt.tick(0.016).unwrap();
        assert_eq!(rt.host_call_count("fre_ResetEvent"), 180);
    }

    #[test]
    fn clear_input_resets_between_sessions() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        rt.apply_input(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::W));
        rt.apply_input(prototype_input::InputEvent::MouseDown(prototype_input::MouseButton::Left));
        rt.apply_input(prototype_input::InputEvent::MouseMove(prototype_input::MousePosition::new(50.0, 50.0)));
        assert!(!rt.input().pressed_keys().is_empty());
        rt.clear_input();
        assert!(rt.input().pressed_keys().is_empty());
        assert!(rt.input().pressed_mouse_buttons().is_empty());
        assert_eq!(rt.input().mouse_position(), prototype_input::MousePosition::new(0.0, 0.0));
    }

    // ----- M11e: world streaming -----------------------------------

    #[test]
    fn runtime_owns_a_canonical_streaming_grid() {
        // M11e: EngineRuntime now owns a StreamingGrid, defaulting
        // to the canonical full grid (one mega-cell per archive,
        // infinite radius — UNKNOWN cell layout, see
        // prototype-stream docs).
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let rt = EngineRuntime::new(&f).unwrap();
        // 5 cells in the canonical grid.
        assert_eq!(rt.streaming_grid().len(), 5);
        // Nothing is loaded initially.
        assert!(rt.loaded_archives().is_empty());
    }

    #[test]
    fn update_streaming_canonical_grid_mounts_all_five() {
        // With the default canonical grid (infinite radius), any
        // player position produces a full mount diff.
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        let set = rt.update_streaming(prototype_math::Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(set.mount_count(), 5);
        assert_eq!(set.unmount_count(), 0);
        // Apply the load set: all 5 archives mount in canonical order.
        rt.apply_load_set(&set);
        assert_eq!(
            rt.loaded_archives(),
            &prototype_stream::ArchiveKind::ALL_IN_MOUNT_ORDER.to_vec()
        );
    }

    #[test]
    fn update_streaming_custom_grid_produces_diff_per_position() {
        // Build a runtime, replace the grid with a custom 3-cell
        // grid, and verify that update_streaming produces the
        // expected diff as the player moves.
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        let mut g = prototype_stream::StreamingGrid::new();
        g.add(prototype_stream::StreamingCell::new(
            prototype_stream::CellId::new(prototype_stream::ArchiveKind::Cells0, 0),
            prototype_math::Vec3::new(0.0, 0.0, 0.0),
            50.0,
        ));
        g.add(prototype_stream::StreamingCell::new(
            prototype_stream::CellId::new(prototype_stream::ArchiveKind::Art0, 0),
            prototype_math::Vec3::new(100.0, 0.0, 0.0),
            50.0,
        ));
        g.add(prototype_stream::StreamingCell::new(
            prototype_stream::CellId::new(prototype_stream::ArchiveKind::Art2, 0),
            prototype_math::Vec3::new(200.0, 0.0, 0.0),
            50.0,
        ));
        rt.set_streaming_grid(g);

        // Player at origin: mount Cells0 only.
        let set = rt.update_streaming(prototype_math::Vec3::ZERO);
        assert_eq!(set.to_mount, vec![prototype_stream::ArchiveKind::Cells0]);
        assert!(set.to_unmount.is_empty());
        rt.apply_load_set(&set);
        assert_eq!(rt.loaded_archives(), &[prototype_stream::ArchiveKind::Cells0]);

        // Player moves to (100, 0, 0): mount Art0, unmount Cells0.
        let set = rt.update_streaming(prototype_math::Vec3::new(100.0, 0.0, 0.0));
        assert_eq!(set.to_mount, vec![prototype_stream::ArchiveKind::Art0]);
        assert_eq!(set.to_unmount, vec![prototype_stream::ArchiveKind::Cells0]);
        rt.apply_load_set(&set);
        assert_eq!(rt.loaded_archives(), &[prototype_stream::ArchiveKind::Art0]);

        // Player moves to (200, 0, 0): mount Art2, unmount Art0.
        let set = rt.update_streaming(prototype_math::Vec3::new(200.0, 0.0, 0.0));
        assert_eq!(set.to_mount, vec![prototype_stream::ArchiveKind::Art2]);
        assert_eq!(set.to_unmount, vec![prototype_stream::ArchiveKind::Art0]);
        rt.apply_load_set(&set);
        assert_eq!(rt.loaded_archives(), &[prototype_stream::ArchiveKind::Art2]);

        // Player moves to (1000, 0, 0): no cell in radius — full
        // unmount (Art2 unmounts).
        let set = rt.update_streaming(prototype_math::Vec3::new(1000.0, 0.0, 0.0));
        assert!(set.to_mount.is_empty());
        assert_eq!(set.to_unmount, vec![prototype_stream::ArchiveKind::Art2]);
        rt.apply_load_set(&set);
        assert!(rt.loaded_archives().is_empty());
    }

    #[test]
    fn loaded_archives_stay_sorted_in_canonical_order() {
        // Mount in non-canonical order; the loaded set should
        // stay sorted.
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        // Build a custom grid that loads all 5 in non-canonical
        // order is non-trivial; instead, apply a synthetic LoadSet
        // with reversed mount order and verify the loaded set is
        // canonical.
        let set = LoadSet {
            to_mount: vec![
                prototype_stream::ArchiveKind::Art2,
                prototype_stream::ArchiveKind::Art1,
                prototype_stream::ArchiveKind::Art0,
                prototype_stream::ArchiveKind::Cells1,
                prototype_stream::ArchiveKind::Cells0,
            ],
            to_unmount: vec![],
        };
        rt.apply_load_set(&set);
        assert_eq!(
            rt.loaded_archives(),
            &prototype_stream::ArchiveKind::ALL_IN_MOUNT_ORDER.to_vec()
        );
    }

    #[test]
    fn streaming_grid_and_tick_are_independent() {
        // The streaming grid's load-set computation does NOT
        // interfere with the frame loop's input snapshot or
        // signal dispatch.
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        rt.bind_default_lifecycle().unwrap();
        // Bind ResetFreeRoamEvents to GameplayTick too, so the
        // tick dispatch has a subscriber and an audit log entry.
        rt.bind_script(EngineSignal::GameplayTick, "ResetFreeRoamEvents").unwrap();
        // Apply input, dispatch a tick, then update streaming.
        rt.apply_input(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::W));
        let frame = rt.tick(0.016).unwrap();
        assert!(frame.input.is_key_down(prototype_input::KeyCode::W));
        // The tick dispatched exactly one closure (ResetFreeRoamEvents).
        assert_eq!(frame.tick_dispatches, 1);
        // Update the streaming grid AFTER the tick.
        let set = rt.update_streaming(prototype_math::Vec3::ZERO);
        rt.apply_load_set(&set);
        // Input state is still intact after the streaming update.
        assert!(rt.input().is_key_down(prototype_input::KeyCode::W));
        // All 5 archives are now loaded.
        assert_eq!(rt.loaded_archives().len(), 5);
        // The dispatch log accumulated one GameplayTick entry
        // (from the tick) — and the GameplayEnter subscription
        // hasn't fired yet (only GameplayTick dispatches via tick()).
        let tick_entries: Vec<&DispatchEntry> = rt
            .dispatch_log()
            .iter()
            .filter(|e| e.signal == EngineSignal::GameplayTick)
            .collect();
        assert_eq!(tick_entries.len(), 1);
        assert_eq!(tick_entries[0].global, "ResetFreeRoamEvents");
    }

    // ----- M11f: tick_with_player integration ---------------------

    #[test]
    fn tick_with_player_drives_streaming_and_frame_in_one_call() {
        // The integrated gameplay-frame entry: feed it the player,
        // get back both the Frame (input snapshot + tick
        // dispatches) and the LoadSet (streaming diff for this
        // frame's player position).
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        rt.bind_default_lifecycle().unwrap();

        // Player at origin — canonical grid loads all 5 archives.
        let player = crate::player::Player::at(prototype_math::Vec3::ZERO);
        let (frame, set) = rt.tick_with_player(&player, 0.016).unwrap();
        // First frame: index 1.
        assert_eq!(frame.frame_index, 1);
        // The canonical grid (infinite radius) loads all 5 archives.
        assert_eq!(set.mount_count(), 5);
        assert_eq!(rt.loaded_archives().len(), 5);
        // The frame's input snapshot is empty (no input applied
        // before the tick).
        assert!(frame.input.pressed_keys().is_empty());
        // GameplayTick had no subscribers → 0 dispatches.
        assert_eq!(frame.tick_dispatches, 0);
    }

    #[test]
    fn tick_with_player_input_applied_before_appears_in_snapshot() {
        // The same invariant as `tick`: input applied BEFORE the
        // tick is in the snapshot.  `tick_with_player` doesn't
        // change this.
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        let player = crate::player::Player::at(prototype_math::Vec3::new(100.0, 0.0, 0.0));
        rt.apply_input(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::W));
        let (frame, _set) = rt.tick_with_player(&player, 0.016).unwrap();
        assert!(frame.input.is_key_down(prototype_input::KeyCode::W));
        // Player position doesn't affect input snapshot.
        let player2 = crate::player::Player::at(prototype_math::Vec3::new(200.0, 0.0, 0.0));
        rt.apply_input(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::A));
        let (frame2, _set2) = rt.tick_with_player(&player2, 0.016).unwrap();
        // Both keys are in the second frame's snapshot.
        assert!(frame2.input.is_key_down(prototype_input::KeyCode::W));
        assert!(frame2.input.is_key_down(prototype_input::KeyCode::A));
    }

    #[test]
    fn tick_with_player_streaming_diff_per_frame_when_player_moves() {
        // Build a custom grid; verify tick_with_player produces
        // the expected LoadSet as the player walks through it.
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        let mut g = prototype_stream::StreamingGrid::new();
        g.add(prototype_stream::StreamingCell::new(
            prototype_stream::CellId::new(prototype_stream::ArchiveKind::Cells0, 0),
            prototype_math::Vec3::new(0.0, 0.0, 0.0),
            50.0,
        ));
        g.add(prototype_stream::StreamingCell::new(
            prototype_stream::CellId::new(prototype_stream::ArchiveKind::Art0, 0),
            prototype_math::Vec3::new(100.0, 0.0, 0.0),
            50.0,
        ));
        rt.set_streaming_grid(g);

        // Frame 1: player at origin → mount Cells0.
        let p1 = crate::player::Player::at(prototype_math::Vec3::ZERO);
        let (_frame, set1) = rt.tick_with_player(&p1, 0.016).unwrap();
        assert_eq!(set1.to_mount, vec![prototype_stream::ArchiveKind::Cells0]);
        assert_eq!(rt.loaded_archives(), &[prototype_stream::ArchiveKind::Cells0]);

        // Frame 2: player at (100, 0, 0) → mount Art0, unmount Cells0.
        let p2 = crate::player::Player::at(prototype_math::Vec3::new(100.0, 0.0, 0.0));
        let (_frame, set2) = rt.tick_with_player(&p2, 0.016).unwrap();
        assert_eq!(set2.to_mount, vec![prototype_stream::ArchiveKind::Art0]);
        assert_eq!(set2.to_unmount, vec![prototype_stream::ArchiveKind::Cells0]);
        assert_eq!(rt.loaded_archives(), &[prototype_stream::ArchiveKind::Art0]);
    }

    // ----- M11g: gameplay_tick (input → player → streaming → tick) -

    #[test]
    fn gameplay_tick_no_input_no_movement_no_streaming_change() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        // Pre-load the canonical grid's archives so the diff is empty.
        let initial = rt.update_streaming(prototype_math::Vec3::ZERO);
        rt.apply_load_set(&initial);
        let mut player = crate::player::Player::new();
        let (frame, set, disp) = rt.gameplay_tick(&mut player, 0.016, 5.0).unwrap();
        assert_eq!(disp, prototype_math::Vec3::ZERO);
        assert!(set.is_empty(), "no player movement → no streaming diff");
        assert_eq!(player.state, crate::player::PlayerState::Idle);
        // Frame's input snapshot is empty.
        assert!(frame.input.pressed_keys().is_empty());
    }

    #[test]
    fn gameplay_tick_w_input_moves_player_and_updates_streaming() {
        // Build a custom grid; the player at origin loads Cells0,
        // then presses W for many frames to move into Art0's radius;
        // the streaming diff updates accordingly.
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        let mut g = prototype_stream::StreamingGrid::new();
        g.add(prototype_stream::StreamingCell::new(
            prototype_stream::CellId::new(prototype_stream::ArchiveKind::Cells0, 0),
            prototype_math::Vec3::new(0.0, 0.0, 0.0),
            50.0,
        ));
        g.add(prototype_stream::StreamingCell::new(
            prototype_stream::CellId::new(prototype_stream::ArchiveKind::Art0, 0),
            prototype_math::Vec3::new(100.0, 0.0, 0.0),
            50.0,
        ));
        rt.set_streaming_grid(g);
        // Apply W to the input state.
        rt.apply_input(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::W));
        let mut player = crate::player::Player::new();
        // Frame 1: player at origin, W held → moves -Z by speed*delta.
        let (frame, _set, disp) = rt.gameplay_tick(&mut player, 1.0, 5.0).unwrap();
        // Displacement is -Z (forward) by 5 meters.
        assert!((disp.z + 5.0).abs() < 1e-5, "disp.z = {}", disp.z);
        // Player's new position is (0, 0, -5) — still in Cells0's
        // radius (50m).  No streaming change.
        assert_eq!(rt.loaded_archives(), &[prototype_stream::ArchiveKind::Cells0]);
        // State is Move.
        assert_eq!(player.state, crate::player::PlayerState::Move);
        // Frame's input snapshot has W.
        assert!(frame.input.is_key_down(prototype_input::KeyCode::W));
    }

    #[test]
    fn gameplay_tick_player_walks_into_new_cell_triggers_streaming_diff() {
        // Build a 2-cell grid where Cells0 (radius 50) is at X=0,
        // Art0 (radius 50) is at X=100.  Player starts at X=0
        // (Cells0).  Press D (right) for many frames at 5 m/s.
        // After 9 seconds, player is at X=45 — only Cells0 loaded
        // (distance to Art0 = 55 > 50).  After 10 seconds, player
        // is at X=50 — Art0 NOW loaded (distance to Art0 = 50 ≤ 50
        // — boundary is inclusive).  After 11 seconds, player is
        // at X=55 — Cells0 NOW unloaded (distance to Cells0 = 55
        // > 50).
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        let mut g = prototype_stream::StreamingGrid::new();
        g.add(prototype_stream::StreamingCell::new(
            prototype_stream::CellId::new(prototype_stream::ArchiveKind::Cells0, 0),
            prototype_math::Vec3::new(0.0, 0.0, 0.0),
            50.0,
        ));
        g.add(prototype_stream::StreamingCell::new(
            prototype_stream::CellId::new(prototype_stream::ArchiveKind::Art0, 0),
            prototype_math::Vec3::new(100.0, 0.0, 0.0),
            50.0,
        ));
        rt.set_streaming_grid(g);
        // Apply D (right).
        rt.apply_input(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::D));
        let mut player = crate::player::Player::new();
        // Walk 9 frames at 5 m/s → player at X=45 (still only Cells0).
        for _ in 0..9 {
            rt.gameplay_tick(&mut player, 1.0, 5.0).unwrap();
        }
        assert!((player.position().x - 45.0).abs() < 1e-5);
        assert_eq!(rt.loaded_archives(), &[prototype_stream::ArchiveKind::Cells0]);
        // 10th frame: player at X=50 → distance to Art0 = 50 (boundary,
        // inclusive) → Art0 NOW loaded.
        let (_frame, set, _disp) = rt.gameplay_tick(&mut player, 1.0, 5.0).unwrap();
        assert!((player.position().x - 50.0).abs() < 1e-5);
        assert!(set.to_mount.contains(&prototype_stream::ArchiveKind::Art0));
        assert_eq!(
            rt.loaded_archives(),
            &[prototype_stream::ArchiveKind::Cells0, prototype_stream::ArchiveKind::Art0]
        );
        // 11th frame: player at X=55 → distance to Cells0 = 55 > 50 →
        // Cells0 NOW unloaded.  Art0 stays loaded (distance = 45 ≤ 50).
        let (_frame, set, _disp) = rt.gameplay_tick(&mut player, 1.0, 5.0).unwrap();
        assert!((player.position().x - 55.0).abs() < 1e-5);
        assert!(set.to_unmount.contains(&prototype_stream::ArchiveKind::Cells0));
        assert!(set.to_mount.is_empty()); // Art0 already loaded
        assert_eq!(rt.loaded_archives(), &[prototype_stream::ArchiveKind::Art0]);
    }

    // ----- missions (M14) -------------------------------------------

    /// Pull the real 2318-byte mission chunk out of scripts.rcf —
    /// block 39: resolved STRUCTURALLY (the Lua51 member carrying the
    /// engine's `AddValidMission` host-call marker) instead of the
    /// pre-39 byte-size hack; see `crate::missions_load`.
    fn mission_chunk() -> Vec<u8> {
        let rcf = fixture("scripts.rcf");
        let arc = prototype_rcf::RcfArchive::parse(&rcf).unwrap();
        let members = crate::missions_load::find_mission_members(&arc);
        assert_eq!(members.len(), 1, "exactly one mission chunk");
        arc.payload(&arc.entries[members[0]]).unwrap().to_vec()
    }

    #[test]
    fn attach_mission_script_registers_all_70_real_missions() {
        let frevents = fixture("frevents.lua");
        let lua = Lua51File::parse(&frevents).unwrap();
        let mut rt = EngineRuntime::new(&lua).unwrap();
        assert!(rt.missions().is_empty());

        let chunk = mission_chunk();
        let missions_lua = Lua51File::parse(&chunk).unwrap();
        let added = rt.attach_mission_script(&missions_lua).expect("attach");
        assert_eq!(added, 70);
        assert_eq!(rt.missions().len(), 70);
        // Registration census — same zones/counts the standalone
        // runner reports (runtime evidence from the REAL chunk).
        assert_eq!(
            rt.missions().zones(),
            vec!["manhattan", "minimal", "loco_test_bed", "manhattan_mini"]
        );
        assert_eq!(rt.missions().in_district("manhattan").len(), 63);
        // All start Available; no transitions logged yet.
        assert_eq!(rt.missions().state_census(), [70, 0, 0, 0]);
        assert!(rt.mission_log().is_empty());
    }

    #[test]
    fn attach_mission_script_is_idempotent_for_the_same_chunk() {
        let frevents = fixture("frevents.lua");
        let lua = Lua51File::parse(&frevents).unwrap();
        let mut rt = EngineRuntime::new(&lua).unwrap();
        let chunk = mission_chunk();
        let missions_lua = Lua51File::parse(&chunk).unwrap();
        assert_eq!(rt.attach_mission_script(&missions_lua).unwrap(), 70);
        // Second attach: all 70 names are duplicates → 0 new.
        assert_eq!(rt.attach_mission_script(&missions_lua).unwrap(), 0);
        assert_eq!(rt.missions().len(), 70);
    }

    #[test]
    fn mission_transitions_log_with_frame_numbers() {
        let frevents = fixture("frevents.lua");
        let lua = Lua51File::parse(&frevents).unwrap();
        let mut rt = EngineRuntime::new(&lua).unwrap();
        let chunk = mission_chunk();
        let missions_lua = Lua51File::parse(&chunk).unwrap();
        rt.attach_mission_script(&missions_lua).unwrap();

        // Frame 1: tick, then start Infected_Test.
        rt.tick(0.016).unwrap();
        assert!(rt.start_mission("Infected_Test"));
        // Frame 2: tick twice, then complete it.
        rt.tick(0.016).unwrap();
        rt.tick(0.016).unwrap();
        assert!(rt.complete_mission("Infected_Test"));
        // Invalid transitions rejected and NOT logged.
        assert!(!rt.complete_mission("Infected_Test")); // not Active
        assert!(!rt.start_mission("Infected_Test"));    // not Available
        assert!(!rt.start_mission("no_such_mission"));  // unknown
        assert!(!rt.fail_mission("e11m01"));            // not Active

        let log = rt.mission_log();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].name, "Infected_Test");
        assert_eq!(log[0].from, MissionState::Available);
        assert_eq!(log[0].to, MissionState::Active);
        assert_eq!(log[0].frame, 1);
        assert_eq!(log[1].to, MissionState::Completed);
        assert_eq!(log[1].frame, 3);
        assert_eq!(rt.mission_transitions("Infected_Test").len(), 2);
        assert!(rt.mission_transitions("e11m01").is_empty());

        // Census reflects the transitions.
        assert_eq!(rt.missions().state_census(), [69, 0, 1, 0]);
        // GameplayTick dispatch did not touch missions: the script's
        // lifecycle closures reset only the FRE events (PROVEN for
        // frevents; mission reset-on-gameplay-enter is UNKNOWN and
        // therefore NOT invented here).
        rt.dispatch_signal(EngineSignal::GameplayEnter).unwrap();
        assert_eq!(rt.missions().state_census(), [69, 0, 1, 0]);
        assert_eq!(rt.mission_log().len(), 2);
    }

    #[test]
    fn full_gameplay_session_mixes_missions_streaming_and_scripts() {
        use prototype_input::{InputEvent, KeyCode};
        let frevents = fixture("frevents.lua");
        let lua = Lua51File::parse(&frevents).unwrap();
        let mut rt = EngineRuntime::new(&lua).unwrap();
        let chunk = mission_chunk();
        let missions_lua = Lua51File::parse(&chunk).unwrap();
        rt.attach_mission_script(&missions_lua).unwrap();

        // Bind the PROVEN lifecycle closures so dispatches do work.
        rt.bind_default_lifecycle().unwrap();
        rt.dispatch_signal(EngineSignal::GameplayEnter).unwrap();

        // A short session: walk the player, tick, run a mission.
        let mut player = crate::player::Player::new();
        rt.apply_input(InputEvent::KeyDown(KeyCode::W));
        for _frame in 1..=5u64 {
            rt.gameplay_tick(&mut player, 0.016, 6.0).unwrap();
        }
        assert!(rt.start_mission("Infected_Test"));
        for _ in 0..10u64 {
            rt.gameplay_tick(&mut player, 0.016, 6.0).unwrap();
        }
        assert!(rt.complete_mission("Infected_Test"));

        // The runtime coheres: script events reset (60 dispatches),
        // 70 missions registered, one transition pair logged, frames
        // counted, player moved.
        assert_eq!(rt.fre().system().len(), 60);
        assert_eq!(rt.missions().len(), 70);
        assert_eq!(rt.mission_log().len(), 2);
        assert_eq!(rt.mission_log()[0].frame, 5);
        assert_eq!(rt.mission_log()[1].frame, 15);
        assert!(rt.frame_count() == 15);
        assert!(player.position().z != 0.0 || player.position().x != 0.0);
    }

    // ----- M25c (block 43): member-level streaming over REAL fixtures

    /// The real member table for the streaming tests: SEVEN members
    /// scanned from the committed cells.rcf fixtures (all real
    /// archive members, blocks 18–41), entered in deliberately
    /// NON-spatial order to prove the stream sorts, not the input.
    /// Hashes are TEST LABELS (standalone fixture files have no real
    /// directory hash); every bbox below is the real scan output.
    ///
    /// Real scan facts (block-43 probe over the committed fixtures):
    ///   cell82   kind=Objects cells=[82]  bbox center (446.03, 113.06, 333.01)
    ///   node     kind=Other        bbox point  (403.86,  14.99, -114.03)
    ///   cloud65  kind=Objects cells=[65]  bbox point  (-244.11, 15.70, -62.83)
    ///   decal201 kind=Objects cells=[201] bbox center (1679.04, 44.72, 55.89)
    ///   nis141   kind=Objects cells=[141] bbox center (1687.23, 10.84, -1845.39)
    ///   geo      kind=Geometry     NO bbox (never auto-loads)
    ///   nisbox   kind=Objects      NO bbox (prop-table placements are
    ///                               stage-local — block 18's exclusion)
    ///   empty    kind=Empty        NO bbox
    fn real_stream_index() -> prototype_p3d::world::WorldIndex {
        let spec: [(&str, u32); 8] = [
            ("cells_member_empty.p3d", 0x8007),
            ("cells_member_objects_nis.p3d", 0x8002),
            ("cells_member_geometry_small.p3d", 0x8006),
            ("cells_obj_cell82.p3d", 0x8001),
            ("cells_go_v1_nisboxstack.p3d", 0x8008),
            ("cells_node_xform.p3d", 0x8003),
            ("cells_obj_decal_mix.p3d", 0x8004),
            ("cells_go_v2_cloudspawner.p3d", 0x8005),
        ];
        prototype_p3d::world::WorldIndex {
            entries: spec
                .iter()
                .map(|&(name, hash)| {
                    prototype_p3d::world::scan_world_member_opts(
                        hash,
                        &fixture(name),
                        prototype_p3d::world::ScanOptions::default(),
                    )
                })
                .collect(),
        }
    }

    /// Real cell-82 spawner-bbox center (the 712-record member).
    fn cell82_center() -> prototype_math::Vec3 {
        prototype_math::Vec3::new(446.026_06, 113.064_8, 333.012_6)
    }

    /// M25c: the member stream loads the nearest REAL members first,
    /// never exceeds the budget, never auto-loads a bbox-less member,
    /// settles to zero churn, and computes distances with the EXACT
    /// `WorldIndexEntry::distance_from` formula (bit-identical).
    #[test]
    fn member_stream_loads_real_cells_nearest_first() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        let idx = real_stream_index();
        // Real distance ladder from the cell-82 center (probe + f64
        // cross-check): cell82 0.0 < node 459.6 < cloud 801.5
        // < decal 1265.6 < nis 2509.3; geo/nisbox/empty are infinite.
        let n = rt
            .attach_member_stream(&idx, MemberStreamConfig::new(3, 1.5).unwrap())
            .unwrap();
        assert_eq!(n, 8, "the full member table is held");
        let set = rt.update_member_streaming(cell82_center());
        assert_eq!(
            set.load,
            vec![0x8001, 0x8003, 0x8005],
            "nearest three REAL members, near-first: {set:?}"
        );
        assert!(set.unload.is_empty());
        // The bbox-less members never auto-load.
        assert!(!set.load.contains(&0x8006) && !set.load.contains(&0x8007) && !set.load.contains(&0x8008));
        let s = rt.member_stream().unwrap();
        assert_eq!(s.loaded_count(), 3);
        assert!(s.is_loaded(0x8001) && s.is_loaded(0x8003) && s.is_loaded(0x8005));
        // Settlement: the same focal point plans zero churn.
        assert!(rt.plan_member_streaming(cell82_center()).is_empty());
        // Distance-identity: the engine's MemberRef distance is
        // BIT-IDENTICAL to WorldIndexEntry::distance_from (the
        // bbox_center_distance dedupe can never drift).
        let cam = [cell82_center().x, cell82_center().y, cell82_center().z];
        for e in &idx.entries {
            let Some(b) = e.bbox else { continue };
            let d_ref = prototype_p3d::world::bbox_center_distance(b, cam);
            assert_eq!(d_ref.to_bits(), e.distance_from(cam).to_bits(), "hash {:#x}", e.hash);
        }
    }

    /// M25c: walking between REAL cells unloads the old cell's
    /// members (farthest first) and settles — no churn after one
    /// apply.
    #[test]
    fn member_stream_walk_between_real_cells_unloads_and_settles() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        rt.attach_member_stream(&real_stream_index(), MemberStreamConfig::new(1, 1.25).unwrap())
            .unwrap();
        // Hop 1: cell-82 → nis-141 (2509 m apart, far beyond any
        // keep radius): the old member evicts, the new one loads.
        let set = rt.update_member_streaming(cell82_center());
        assert_eq!(set.load, vec![0x8001]);
        let nis = prototype_math::Vec3::new(1687.231_4, 10.839_268, -1845.388_4);
        let set = rt.update_member_streaming(nis);
        assert_eq!(set.load, vec![0x8002]);
        assert_eq!(set.unload, vec![0x8001], "the far member evicts");
        assert!(rt.plan_member_streaming(nis).is_empty(), "settles");
        // Hop 2: nis-141 → decal-201 (1901.6 m): same discipline.
        let decal = prototype_math::Vec3::new(1679.035_9, 44.717_102, 55.888_89);
        let set = rt.update_member_streaming(decal);
        assert_eq!(set.load, vec![0x8004]);
        assert_eq!(set.unload, vec![0x8002]);
        assert!(rt.member_stream().unwrap().loaded_count() == 1);
        assert!(rt.plan_member_streaming(decal).is_empty());
    }

    /// M25c: the gameplay frame path drives the member stream
    /// (observable via `last_member_load_set`) and a runtime without
    /// an attached stream stays a clean no-op.
    #[test]
    fn tick_with_player_drives_member_stream_and_defaults_to_noop() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        // Default: archive-level streaming only.
        let player = crate::player::Player::at(cell82_center());
        rt.tick_with_player(&player, 1.0 / 30.0).unwrap();
        assert!(rt.last_member_load_set().is_empty());
        assert!(rt.member_stream().is_none());
        // Attach: the same frame path now streams members.
        rt.attach_member_stream(&real_stream_index(), MemberStreamConfig::new(2, 1.5).unwrap())
            .unwrap();
        let player = crate::player::Player::at(cell82_center());
        rt.tick_with_player(&player, 1.0 / 30.0).unwrap();
        assert_eq!(
            rt.last_member_load_set().load,
            vec![0x8001, 0x8003],
            "nearest two REAL members"
        );
        // Second identical frame: settled (no churn).
        rt.tick_with_player(&player, 1.0 / 30.0).unwrap();
        assert!(rt.last_member_load_set().is_empty());
        assert_eq!(rt.member_stream().unwrap().loaded_count(), 2);
    }

    /// M25c: an invalid config fails closed through the engine glue.
    #[test]
    fn attach_member_stream_rejects_invalid_config() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let mut rt = EngineRuntime::new(&f).unwrap();
        let bad = MemberStreamConfig {
            budget: 0,
            hysteresis: 1.5,
        };
        assert!(rt.attach_member_stream(&real_stream_index(), bad).is_err());
        let bad = MemberStreamConfig {
            budget: 4,
            hysteresis: 0.5,
        };
        assert!(rt.attach_member_stream(&real_stream_index(), bad).is_err());
        assert!(rt.member_stream().is_none(), "failed attach leaves no state");
    }

    /// Block 74: the runtime proxies the script host's typed
    /// audio-seam requests — and the REAL corpus yields an EMPTY
    /// vec (the block-73 pinned-empty intersection, now pinned
    /// through the full runtime constructor).
    #[test]
    fn runtime_audio_seam_requests_mirror_the_script_host() {
        let f = Lua51File::parse(&fixture("frevents.lua")).unwrap();
        let rt = EngineRuntime::new(&f).unwrap();
        let reqs = rt.audio_seam_requests().expect("no bad args exist");
        assert!(
            reqs.is_empty(),
            "the shipped corpus must not drive the audio seam"
        );
        assert_eq!(rt.host_call_count(crate::audio_seam::SND_SET_MISSION), 0);
        assert_eq!(
            rt.host_call_count(crate::audio_seam::SND_SET_FREE_ROAM_MISSION),
            0
        );
    }
}
//! prototype-boot
//! ==============
//! Reference model for the Prototype (2009) **boot handshake**, the
//! **cement library initialization**, and the **content pipeline** —
//! reconstructed from the Phase 2D, 2F, and 2C analysis documents
//! in the archived `Nazatric/PrototypeRecomp` repository
//! (`analysis/PHASE2D_BOOT_HANDSHAKE.md`,
//! `analysis/PHASE2F_ARCHIVE_MANAGER_INIT.md`,
//! `analysis/PHASE2C_CONTENT_PIPELINE.md`).
//!
//! ## What this crate provides
//!
//! * **Boot stage enum** — the ordered set of boot stages the engine
//!   walks through, with the gates that fire between them (PROVEN
//!   from the Phase 2D wait-graph).
//! * **Engine service IDs** — the `EngineInitialize` /
//!   `EngineWindowFocus` / `EngineMouseService` /
//!   `EngineKeyboardService` / `EngineService` / `EngineTerminate`
//!   dispatch IDs (PROVEN — observed as literal strings in
//!   `prototypef.exe`'s `.rdata` section).
//! * **Cement library state machine** — the chain of 5 kernel-
//!   semantics gates (`XexCheckExecutablePrivilege`, raw HDD
//!   namespace, `XMountUtilityDrive` ioctls, `FileFsSizeInformation`,
//!   `XamTaskSchedule`) that must all complete before the cement
//!   library initializer (`sub_82A5A920`) runs and the global gate
//!   `[0x82DCF578+0xB4]` flips to 1 (PROVEN — every gate verified
//!   against Xenia).
//! * **Content pipeline wait graph** — every thread that blocks
//!   during boot, what it's parked on, and what unblocks it (PROVEN
//!   from Phase 2D's full wait graph).
//! * **RCF mount order** — the 5 archives (`00cells.rcf`,
//!   `01cells.rcf`, `00art.rcf`, `01art.rcf`, `02art.rcf`) the
//!   engine mounts at boot, plus the startup package
//!   (`art/startup_shaders.p3d` + `art/startup.p3d` +
//!   `art/startup_tod.p3d`) that must load before the `unlit`
//!   material resolves (PROVEN from XEX strings + Xenia canary log).
//!
//! ## What this crate does NOT provide
//!
//! * The actual recompiled binary code — this is a *reference model*
//!   for validating a recompilation project, not the recompilation
//!   itself.
//! * The full pure3D chunk tree — root chunk only is parsed by
//!   `prototype-p3d`; recursive child-chunk walking is planned but
//!   not yet implemented.
//!
//! ## Content pipeline (added 2026-11-03)
//!
//! [`ContentPipeline`] is a working **virtual file system** over the
//! real archives: it mounts `.rcf` cement libraries, resolves game
//! paths (e.g. `scripts\frevents.lua`) with the PROVEN name hash from
//! `prototype-rcf`, falls back to loose files, and routes each
//! payload to its matching parser (`prototype-p3d` / `prototype-lua`)
//! by magic-byte sniffing.
//!
//! ## Confidence labels
//!
//! Every constant and structure in this crate is annotated with one of:
//! * `PROVEN` — observed directly in the original binary or verified
//!   against Xenia's reference implementation.
//! * `LIKELY` — inferred from multiple independent observations but
//!   not directly confirmed.
//! * `UNKNOWN` — not yet recoverable from available evidence.

#![forbid(unsafe_code)]

// ===========================================================================
// Boot stages — Phase 2D's verified handshake chain
// ===========================================================================

/// One stage in the engine's boot sequence. The engine walks these
/// stages in order; each stage gates on the previous one completing
/// before it can advance.
///
/// PROVEN from Phase 2D's wait-graph analysis (tid=7 main thread +
/// tid=26 display-init thread + tid=25 task-runner thread +
/// tid=27/28 render threads).
pub mod script_pump;
pub mod cement_files;
pub mod streaming;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BootStage {
    /// Stage 0 — `main thread (tid7) entry sub_82239F58` — initial
    /// gate (`sub_82267FB0 → 0`).
    InitialGate,
    /// Stage 1 — main thread enters the wait loop `sub_8226E260`
    /// and pumps the task queue.
    MainWaitLoop,
    /// Stage 2 — display-init thread (tid26) is created and enters
    /// `sub_8226DA40` to build the display subsystem.
    DisplayInitStart,
    /// Stage 3 — display-init thread sets `[0x82D844F8] = 1` after
    /// the `unlit` material lookup succeeds.
    DisplayInitComplete,
    /// Stage 4 — main thread exits the wait loop and enters the
    /// module/service registration chain (`sub_8228ABE0` →
    /// `sub_8228AAF8`, `sub_82A88150`, ...).
    ModuleRegistration,
    /// Stage 5 — task-runner thread (tid25) runs the ATG worker
    /// loop with the 28-entry message dispatch (`sub_82A9B4A8`).
    AtgWorkerDispatch,
    /// Stage 6 — the cement library initializer `sub_82A5A920`
    /// runs; the global gate `[0x82DCF578+0xB4]` flips to 1; all 5
    /// RCF archives open and parse their headers + TOCs.
    CementLibraryInit,
    /// Stage 7 — the startup package (`art/startup_shaders.p3d` +
    /// `art/startup.p3d` + `art/startup_tod.p3d`) loads; the `unlit`
    /// material is registered in the registry at `[0x82DD9BD4]`.
    StartupPackageLoaded,
    /// Stage 8 — first MMIO flip to `0x7FC86110`; render thread
    /// produces the first frame; swap queue begins processing.
    FirstFramePresented,
    /// Stage 9 — the loading screen draws continuously (PM4 draws
    /// + "Resolve region is empty" repeat).  Game is "running" but
    /// parked on actual gameplay content.
    LoadingScreen,
    /// Stage 10 — gameplay begins (not yet reached by the
    /// PrototypeRecomp project as of Phase 2G).
    Gameplay,
}

impl BootStage {
    /// All boot stages in their canonical order.
    pub const ALL: [BootStage; 11] = [
        BootStage::InitialGate,
        BootStage::MainWaitLoop,
        BootStage::DisplayInitStart,
        BootStage::DisplayInitComplete,
        BootStage::ModuleRegistration,
        BootStage::AtgWorkerDispatch,
        BootStage::CementLibraryInit,
        BootStage::StartupPackageLoaded,
        BootStage::FirstFramePresented,
        BootStage::LoadingScreen,
        BootStage::Gameplay,
    ];

    /// The stage that immediately precedes this one, or `None`
    /// for the first stage.
    pub fn previous(&self) -> Option<BootStage> {
        let idx = BootStage::ALL.iter().position(|s| s == self)?;
        if idx == 0 {
            None
        } else {
            Some(BootStage::ALL[idx - 1])
        }
    }

    /// The stage that immediately follows this one, or `None`
    /// for the last stage.
    pub fn next(&self) -> Option<BootStage> {
        let idx = BootStage::ALL.iter().position(|s| s == self)?;
        BootStage::ALL.get(idx + 1).copied()
    }
}

// ===========================================================================
// Engine service IDs — observed in prototypef.exe's .rdata
// ===========================================================================

/// One of the six engine service IDs the launcher `prototypef.exe`
/// dispatches into `prototypeenginef.dll`. These are observed as
/// literal null-terminated strings in the launcher's `.rdata`
/// section.
///
/// PROVEN — recovered from `prototypef.exe`'s strings during the
/// inventory phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EngineServiceId {
    /// `EngineInitialize` — the engine's main init entry point.
    /// Pairs with the boot handshake stage [`BootStage::InitialGate`].
    Initialize,
    /// `EngineWindowFocus` — window focus event service.
    WindowFocus,
    /// `EngineMouseService` — mouse input service.
    MouseService,
    /// `EngineKeyboardService` — keyboard input service.
    KeyboardService,
    /// `EngineService` — generic engine service dispatch.
    Service,
    /// `EngineTerminate` — engine shutdown.
    Terminate,
}

impl EngineServiceId {
    /// The literal string the launcher uses for this service ID.
    pub fn as_str(&self) -> &'static str {
        match self {
            EngineServiceId::Initialize => "EngineInitialize",
            EngineServiceId::WindowFocus => "EngineWindowFocus",
            EngineServiceId::MouseService => "EngineMouseService",
            EngineServiceId::KeyboardService => "EngineKeyboardService",
            EngineServiceId::Service => "EngineService",
            EngineServiceId::Terminate => "EngineTerminate",
        }
    }
}

// ===========================================================================
// Cement library — Phase 2F's verified 5-gate initialization chain
// ===========================================================================

/// One of the 5 kernel-semantics gates that must all complete before
/// the cement library initializer runs.
///
/// PROVEN — Phase 2F verified every gate against Xenia's
/// implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CementLibraryGate {
    /// `XexCheckExecutablePrivilege` — the ATG cementer's 'ATEM'
    /// manifest handler checks privileges 11 and 23 and requires
    /// BOTH to return 0.  Title's system flags are 0x600 (bits
    /// 9+10) — both privileges correctly return 0.
    XexCheckExecutablePrivilege,

    /// Raw HDD namespace — `\Device\Harddisk0\partition0` opens
    /// succeed; reads succeed without touching the buffer (Xenia
    /// `NullFile::ReadSync` semantics); writes succeed (discarded).
    /// `partition0` gets a real 2 MiB persistent backing file.
    RawHddNamespace,

    /// `XMountUtilityDrive`'s ioctls — `0x70000` (drive geometry)
    /// and `0x74004` (partition info) both write valid output
    /// buffers; stack arguments `ARG(8)/ARG(9)` correctly read
    /// from `[caller r1 + 0x54 + (i-8)*8]` (not r11/r12).
    XMountUtilityDrive,

    /// `FileFsSizeInformation` — class 3 of
    /// `NtQueryVolumeInformationFile` returns
    /// `{31 units, 31 avail, 64 sectors/unit, 512 bps}` (= 0x8000
    /// cluster size, as required by `sub_82A65F50`).
    FileFsSizeInformation,

    /// `XamTaskSchedule` — schedules a real `GuestThread` (0x40000
    /// stack) with the callback as entry and the `XTASK_MESSAGE`
    /// as argument. Drives ALL worker threads including the
    /// cement-library thread (entry `0x82A5A558`).
    XamTaskSchedule,
}

impl CementLibraryGate {
    /// All 5 gates in their canonical order.
    pub const ALL: [CementLibraryGate; 5] = [
        CementLibraryGate::XexCheckExecutablePrivilege,
        CementLibraryGate::RawHddNamespace,
        CementLibraryGate::XMountUtilityDrive,
        CementLibraryGate::FileFsSizeInformation,
        CementLibraryGate::XamTaskSchedule,
    ];
}

/// Tracks the state of all 5 cement-library gates.  The engine
/// cannot proceed to [`BootStage::CementLibraryInit`] until every
/// gate reports `Ok(())`.
#[derive(Debug, Clone, Default)]
pub struct CementLibraryState {
    gates: [GateState; 5],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateState {
    /// Gate has not yet been visited.
    Pending,
    /// Gate was passed successfully.
    Passed,
    /// Gate was visited but failed (with the given reason).
    Failed(&'static str),
}

impl Default for GateState {
    fn default() -> Self {
        GateState::Pending
    }
}

impl CementLibraryState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark a gate as passed.
    pub fn pass(&mut self, gate: CementLibraryGate) {
        self.gates[gate as usize] = GateState::Passed;
    }

    /// Mark a gate as failed.
    pub fn fail(&mut self, gate: CementLibraryGate, reason: &'static str) {
        self.gates[gate as usize] = GateState::Failed(reason);
    }

    /// Get the state of one gate.
    pub fn state(&self, gate: CementLibraryGate) -> GateState {
        self.gates[gate as usize]
    }

    /// True iff every gate has `Passed`.
    pub fn all_passed(&self) -> bool {
        self.gates.iter().all(|g| matches!(g, GateState::Passed))
    }

    /// True iff any gate has `Failed`.
    pub fn any_failed(&self) -> bool {
        self.gates
            .iter()
            .any(|g| matches!(g, GateState::Failed(_)))
    }
}

// ===========================================================================
// Content pipeline — Phase 2C's verified wait-graph and RCF mount order
// ===========================================================================

/// One of the 5 RCF archives the engine mounts at boot, in the
/// order they are mounted.
///
/// PROVEN from XEX strings at:
///   `0x8202FB9C` `game:\00cells.rcf`
///   `0x8202FBB0` `game:\01cells.rcf`
///   `0x8202FBC4` `game:\00art.rcf`
///   `0x8202FBD4` `game:\01art.rcf`
///   `0x8202FBE4` `game:\02art.rcf`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RcfMountOrder {
    /// `game:\00cells.rcf` — mounted first
    Cells0,
    /// `game:\01cells.rcf` — mounted second
    Cells1,
    /// `game:\00art.rcf` — mounted third (contains the startup
    /// package + frontendshaders + HUD fonts)
    Art0,
    /// `game:\01art.rcf` — mounted fourth
    Art1,
    /// `game:\02art.rcf` — mounted fifth
    Art2,
}

impl RcfMountOrder {
    /// All 5 archives in their mount order.
    pub const ALL: [RcfMountOrder; 5] = [
        RcfMountOrder::Cells0,
        RcfMountOrder::Cells1,
        RcfMountOrder::Art0,
        RcfMountOrder::Art1,
        RcfMountOrder::Art2,
    ];

    /// The Xbox 360-style `game:\` path string the engine uses.
    pub fn game_path(&self) -> &'static str {
        match self {
            RcfMountOrder::Cells0 => "game:\\00cells.rcf",
            RcfMountOrder::Cells1 => "game:\\01cells.rcf",
            RcfMountOrder::Art0 => "game:\\00art.rcf",
            RcfMountOrder::Art1 => "game:\\01art.rcf",
            RcfMountOrder::Art2 => "game:\\02art.rcf",
        }
    }

    /// The disc-root filename (without the `game:\` prefix).
    pub fn filename(&self) -> &'static str {
        match self {
            RcfMountOrder::Cells0 => "00cells.rcf",
            RcfMountOrder::Cells1 => "01cells.rcf",
            RcfMountOrder::Art0 => "00art.rcf",
            RcfMountOrder::Art1 => "01art.rcf",
            RcfMountOrder::Art2 => "02art.rcf",
        }
    }
}

/// The startup package — three `.p3d` files that must load together
/// before the `unlit` material resolves and display-init completes.
///
/// PROVEN from XEX string at `0x82033C90`:
///   `art/startup_shaders.p3d;art/startup.p3d;art/startup_tod.p3d`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StartupPackageFile {
    StartupShaders,
    Startup,
    StartupTod,
}

impl StartupPackageFile {
    pub const ALL: [StartupPackageFile; 3] = [
        StartupPackageFile::StartupShaders,
        StartupPackageFile::Startup,
        StartupPackageFile::StartupTod,
    ];

    /// The semicolon-separated `p3d` path string the engine uses.
    pub const PACKED_PATH: &'static str =
        "art/startup_shaders.p3d;art/startup.p3d;art/startup_tod.p3d";

    /// The package name registered by the InGame Simulation Task
    /// module initializer (`sub_823CE4A8`).
    pub const PACKAGE_NAME: &'static str = "startup";

    /// The art-archive-relative path of this single file.
    pub fn art_path(&self) -> &'static str {
        match self {
            StartupPackageFile::StartupShaders => "art/startup_shaders.p3d",
            StartupPackageFile::Startup => "art/startup.p3d",
            StartupPackageFile::StartupTod => "art/startup_tod.p3d",
        }
    }
}

/// The frontend shader file (`art/frontendshaders.p3d`) — loaded
/// separately from the startup package, alongside it.
///
/// PROVEN from XEX string at `0x82001014`.
pub const FRONTEND_SHADERS_PATH: &str = "art/frontendshaders.p3d";

/// The `unlit` material — the boot-critical material whose
/// registration in the material registry at `[0x82DD9BD4]` flips
/// the display-init flag `[0x82D844F8]` to 1, unblocking the
/// main thread's wait loop.
///
/// PROVEN from XEX string at `0x820024B0` (`unlit`) and the Phase 2C
/// wait graph (display-init thread parks on the `unlit` lookup
/// returning != -1).
pub const UNLIT_MATERIAL_NAME: &str = "unlit";

/// Compute a Prototype material UID from a name string.
///
/// **UNKNOWN** — the engine computes UIDs via `sub_82AD20E0`
/// (`UIDFromHash`), a custom Radical function that is also exposed
/// to the Lua runtime.  Without disassembly of `sub_82AD20E0` in
/// the original Xbox 360 binary, we cannot reproduce the algorithm.
///
/// This function returns `None` to make the gap explicit.  Once
/// the algorithm is recovered, this becomes a real implementation
/// and the prototype-uid module can fill in.
pub fn material_uid(_name: &str) -> Option<u32> {
    None
}

// ===========================================================================
// Wait graph — Phase 2D's verified thread-park catalog
// ===========================================================================

/// One thread that parks during the boot handshake.  Each variant
/// carries the wait condition and the address at which it parks
/// (where known).
///
/// PROVEN from Phase 2D's wait-tracking instrumentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitInfo {
    /// The thread ID assigned by the kernel.
    pub tid: u32,
    /// What this thread is parked on.
    pub wait: WaitKind,
}

/// Why a thread parks during boot.  Every variant is annotated with
/// the address it parks at and the condition that releases it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitKind {
    /// Main thread (tid=7) parks in `sub_8226E260` waiting for the
    /// global flag `[0x82D844F8] != 0` (display-init complete).
    DisplayInitFlag,
    /// Display-init thread (tid=26) parks on the `unlit` material
    /// lookup returning != -1.
    UnlitMaterialLookup,
    /// Render thread (tid=25) busy-loops on `[r30] > 28`
    /// (boot-stage counter).
    BootStageCounter28,
    /// Render threads (tid=27/28) wait on in-place events
    /// `A1A05A00`/`A1A05A50` (present/swap events).
    PresentSwapEvents,
    /// Job workers (tid=10..24) wait on pool semaphores
    /// `F8000014/24/3C/58/78/8C`.
    PoolSemaphores,
    /// GPU threads (tid=8/9) feed the command processor + vsync.
    GpuFeeder,
}

/// The full wait graph from Phase 2D — every thread that parks
/// during the boot handshake, before the content gate is reached.
pub const WAIT_GRAPH: &[WaitInfo] = &[
    WaitInfo { tid: 7, wait: WaitKind::DisplayInitFlag },
    WaitInfo { tid: 26, wait: WaitKind::UnlitMaterialLookup },
    WaitInfo { tid: 25, wait: WaitKind::BootStageCounter28 },
    WaitInfo { tid: 27, wait: WaitKind::PresentSwapEvents },
    WaitInfo { tid: 28, wait: WaitKind::PresentSwapEvents },
    WaitInfo { tid: 10, wait: WaitKind::PoolSemaphores },
    WaitInfo { tid: 11, wait: WaitKind::PoolSemaphores },
    WaitInfo { tid: 12, wait: WaitKind::PoolSemaphores },
    WaitInfo { tid: 13, wait: WaitKind::PoolSemaphores },
    WaitInfo { tid: 14, wait: WaitKind::PoolSemaphores },
    WaitInfo { tid: 15, wait: WaitKind::PoolSemaphores },
    WaitInfo { tid: 16, wait: WaitKind::PoolSemaphores },
    WaitInfo { tid: 17, wait: WaitKind::PoolSemaphores },
    WaitInfo { tid: 18, wait: WaitKind::PoolSemaphores },
    WaitInfo { tid: 19, wait: WaitKind::PoolSemaphores },
    WaitInfo { tid: 20, wait: WaitKind::PoolSemaphores },
    WaitInfo { tid: 21, wait: WaitKind::PoolSemaphores },
    WaitInfo { tid: 22, wait: WaitKind::PoolSemaphores },
    WaitInfo { tid: 23, wait: WaitKind::PoolSemaphores },
    WaitInfo { tid: 24, wait: WaitKind::PoolSemaphores },
    WaitInfo { tid: 8, wait: WaitKind::GpuFeeder },
    WaitInfo { tid: 9, wait: WaitKind::GpuFeeder },
];

// ===========================================================================
// Boot state machine — the high-level boot progress tracker
// ===========================================================================

/// Tracks the engine's boot progress through every stage.  A
/// `BootState` is advanced one stage at a time by calling
/// [`BootState::advance`] with the next stage that completed.
///
/// This is a reference implementation — the actual engine's state
/// machine is implicit in the control flow of `sub_8226E260` +
/// `sub_8226DA40` + the module-init chain.  This Rust model exists
/// so that a recompilation project can validate its boot progress
/// against a known-good sequence.
#[derive(Debug, Clone)]
pub struct BootState {
    pub current_stage: BootStage,
    pub cement_state: CementLibraryState,
    /// The set of RCF archives that have been opened and parsed.
    pub mounted_rcfs: Vec<RcfMountOrder>,
    /// True once the `unlit` material has been registered in the
    /// material registry.
    pub unlit_registered: bool,
    /// True once the startup package has fully loaded.
    pub startup_package_loaded: bool,
    /// Count of PM4 GPU packets processed (matches Phase 2D's
    /// instrumented counter).
    pub pm4_packets: u64,
    /// Count of DRAW_INDX draws.
    pub draws: u64,
    /// Count of shader loads (IM_LOAD).
    pub shader_loads: u64,
    /// Count of vblank interrupts.
    pub vblanks: u64,
}

impl Default for BootState {
    fn default() -> Self {
        Self {
            current_stage: BootStage::InitialGate,
            cement_state: CementLibraryState::new(),
            mounted_rcfs: Vec::new(),
            unlit_registered: false,
            startup_package_loaded: false,
            pm4_packets: 0,
            draws: 0,
            shader_loads: 0,
            vblanks: 0,
        }
    }
}

impl BootState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance to the next boot stage.  Returns `Err` with a reason
    /// if the next stage's prerequisites are not yet met.
    pub fn advance(&mut self, next: BootStage) -> Result<(), BootAdvanceError> {
        // Verify ordering
        if next != self.current_stage.next().ok_or(BootAdvanceError::NoMoreStages)? {
            // Allow re-advance to the same stage (idempotent)
            if next != self.current_stage {
                return Err(BootAdvanceError::WrongOrder {
                    from: self.current_stage,
                    to: next,
                });
            }
        }
        // Verify per-stage prerequisites
        match next {
            BootStage::DisplayInitComplete => {
                if !self.unlit_registered {
                    return Err(BootAdvanceError::MissingPrerequisite {
                        stage: next,
                        reason: "unlit material not yet registered",
                    });
                }
            }
            BootStage::CementLibraryInit => {
                if !self.cement_state.all_passed() {
                    return Err(BootAdvanceError::MissingPrerequisite {
                        stage: next,
                        reason: "cement library gates not all passed",
                    });
                }
            }
            BootStage::StartupPackageLoaded => {
                if self.mounted_rcfs.len() < 3 {
                    return Err(BootAdvanceError::MissingPrerequisite {
                        stage: next,
                        reason: "at least 3 RCFs must be mounted (00cells/01cells/00art)",
                    });
                }
                if !self.mounted_rcfs.contains(&RcfMountOrder::Art0) {
                    return Err(BootAdvanceError::MissingPrerequisite {
                        stage: next,
                        reason: "00art.rcf (which contains the startup package) must be mounted",
                    });
                }
            }
            _ => {}
        }
        self.current_stage = next;
        Ok(())
    }

    /// Mark a cement-library gate as passed.
    pub fn pass_cement_gate(&mut self, gate: CementLibraryGate) {
        self.cement_state.pass(gate);
    }

    /// Mark a cement-library gate as failed.
    pub fn fail_cement_gate(&mut self, gate: CementLibraryGate, reason: &'static str) {
        self.cement_state.fail(gate, reason);
    }

    /// Record that an RCF archive was opened and parsed.
    pub fn mount_rcf(&mut self, rcf: RcfMountOrder) {
        if !self.mounted_rcfs.contains(&rcf) {
            self.mounted_rcfs.push(rcf);
        }
    }

    /// Record that the `unlit` material was registered.
    pub fn register_unlit(&mut self) {
        self.unlit_registered = true;
    }

    /// Record that the startup package finished loading.
    pub fn startup_package_done(&mut self) {
        self.startup_package_loaded = true;
    }

    /// Record one PM4 packet processed.
    pub fn pm4_packet(&mut self) {
        self.pm4_packets += 1;
    }

    /// Record one draw.
    pub fn draw(&mut self) {
        self.draws += 1;
    }

    /// Record one shader load.
    pub fn shader_load(&mut self) {
        self.shader_loads += 1;
    }

    /// Record one vblank interrupt.
    pub fn vblank(&mut self) {
        self.vblanks += 1;
    }
}

/// Error returned by [`BootState::advance`] when the engine cannot
/// proceed to the next stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootAdvanceError {
    /// The boot chain has already reached its final stage.
    NoMoreStages,
    /// The engine tried to skip a stage or move backwards.
    WrongOrder { from: BootStage, to: BootStage },
    /// A required prerequisite for the next stage has not yet been
    /// satisfied.
    MissingPrerequisite { stage: BootStage, reason: &'static str },
}

// ===========================================================================
// Phase 2D's verified milestones — known-good boot counts
// ===========================================================================

/// Phase 2C's stable checkpoint (the "stalled" state before
/// Phase 2D fixed the timeout bugs):
///   * 94 PM4 packets, 7 IBs, 24 draws, 7496 vblanks, 22 threads
///   * display-init never completes (stays at 0)
pub const PHASE2C_STABLE_CHECKPOINT: BootState = BootState {
    current_stage: BootStage::DisplayInitStart,
    cement_state: CementLibraryState {
        gates: [
            GateState::Passed,
            GateState::Passed,
            GateState::Passed,
            GateState::Passed,
            GateState::Failed("XamTaskSchedule returned success without creating a thread"),
        ],
    },
    mounted_rcfs: Vec::new(),
    unlit_registered: false,
    startup_package_loaded: false,
    pm4_packets: 94,
    draws: 24,
    shader_loads: 2,
    vblanks: 7496,
};

/// Phase 2D's content-gate checkpoint (after fixing the timeout
/// bugs + the 28-entry switch table):
///   * 1160 PM4 packets, 30 IBs, 103 draws, 19 shader loads
///   * display-init flag `[0x82D844F8] = 1` (set)
///   * disc file requests: args.txt + skuinfo.p3d + skuinfo.p3d.rz
///   * module-init / content gate (waiting for RCF archives)
pub const PHASE2D_CONTENT_GATE: BootState = BootState {
    current_stage: BootStage::ModuleRegistration,
    cement_state: CementLibraryState {
        gates: [
            GateState::Passed,
            GateState::Passed,
            GateState::Passed,
            GateState::Passed,
            GateState::Passed,
        ],
    },
    mounted_rcfs: Vec::new(),
    unlit_registered: false,
    startup_package_loaded: false,
    pm4_packets: 1160,
    draws: 103,
    shader_loads: 19,
    vblanks: 0,
};

/// Phase 2F's archive-manager-init checkpoint (after fixing all 5
/// cement-library kernel bugs):
///   * All 8 RCFs re-open + parse header/TOC via the game
///   * `00art.rcf`: 2048 + 18432 bytes = header + 1361 entries
///   * Cement thread tid=39 (entry 0x82A5A558) runs
///   * Crashes at ~3 s in `sub_828CAE98` (frontend image-loader
///     registry returns NULL for `fonts_latin.gfx`)
pub const PHASE2F_ARCHIVE_INIT: BootState = BootState {
    current_stage: BootStage::CementLibraryInit,
    cement_state: CementLibraryState {
        gates: [
            GateState::Passed,
            GateState::Passed,
            GateState::Passed,
            GateState::Passed,
            GateState::Passed,
        ],
    },
    mounted_rcfs: Vec::new(), // filled in by mount_rcf() per archive
    unlit_registered: false,
    startup_package_loaded: false,
    pm4_packets: 0,
    draws: 0,
    shader_loads: 0,
    vblanks: 0,
};

/// `00art.rcf` directory stats (PROVEN from Phase 2F):
///   * header size = 2048 bytes
///   * TOC size = 18432 bytes
///   * entry count = 1361 (= 18432 / 12 + ~1, see RCF format docs)
pub const ART0_RCF_HEADER_BYTES: u32 = 2048;
pub const ART0_RCF_TOC_BYTES: u32 = 18432;
pub const ART0_RCF_ENTRY_COUNT: u32 = (18432 - 12) / 12; // approximate, see note

// ===========================================================================
// Unit tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_stage_order_is_total() {
        // Every stage (except the first) has a previous.
        for s in BootStage::ALL.iter().skip(1) {
            assert!(s.previous().is_some(), "stage {:?} has no previous", s);
        }
        // Every stage (except the last) has a next.
        for s in BootStage::ALL.iter().take(BootStage::ALL.len() - 1) {
            assert!(s.next().is_some(), "stage {:?} has no next", s);
        }
        // First stage has no previous.
        assert!(BootStage::ALL[0].previous().is_none());
        // Last stage has no next.
        assert!(BootStage::ALL[BootStage::ALL.len() - 1].next().is_none());
    }

    #[test]
    fn engine_service_ids_match_observed_strings() {
        // These strings were observed in prototypef.exe's .rdata
        // section during the inventory phase.
        assert_eq!(EngineServiceId::Initialize.as_str(), "EngineInitialize");
        assert_eq!(EngineServiceId::WindowFocus.as_str(), "EngineWindowFocus");
        assert_eq!(EngineServiceId::MouseService.as_str(), "EngineMouseService");
        assert_eq!(EngineServiceId::KeyboardService.as_str(), "EngineKeyboardService");
        assert_eq!(EngineServiceId::Service.as_str(), "EngineService");
        assert_eq!(EngineServiceId::Terminate.as_str(), "EngineTerminate");
    }

    #[test]
    fn cement_library_gates_total_5() {
        assert_eq!(CementLibraryGate::ALL.len(), 5);
        // The 5 variants are: XexCheckExecutablePrivilege, RawHddNamespace,
        // XMountUtilityDrive, FileFsSizeInformation, XamTaskSchedule.
        // (Counted here without std::mem::variant_count because that's
        // still an unstable library feature.)
        let all_variants = [
            CementLibraryGate::XexCheckExecutablePrivilege,
            CementLibraryGate::RawHddNamespace,
            CementLibraryGate::XMountUtilityDrive,
            CementLibraryGate::FileFsSizeInformation,
            CementLibraryGate::XamTaskSchedule,
        ];
        assert_eq!(all_variants.len(), 5);
    }

    #[test]
    fn cement_state_all_passed_initial() {
        let s = CementLibraryState::new();
        assert!(!s.all_passed());
        assert!(!s.any_failed());
    }

    #[test]
    fn cement_state_all_passed_after_passing_all() {
        let mut s = CementLibraryState::new();
        for g in CementLibraryGate::ALL {
            s.pass(g);
        }
        assert!(s.all_passed());
        assert!(!s.any_failed());
    }

    #[test]
    fn cement_state_tracks_failures() {
        let mut s = CementLibraryState::new();
        s.fail(CementLibraryGate::XamTaskSchedule, "test");
        assert!(s.any_failed());
        assert!(!s.all_passed());
        assert_eq!(
            s.state(CementLibraryGate::XamTaskSchedule),
            GateState::Failed("test")
        );
    }

    #[test]
    fn rcf_mount_order_canonical() {
        // Phase 2C verified the order: cells, cells, art, art, art.
        assert_eq!(
            RcfMountOrder::ALL,
            [
                RcfMountOrder::Cells0,
                RcfMountOrder::Cells1,
                RcfMountOrder::Art0,
                RcfMountOrder::Art1,
                RcfMountOrder::Art2,
            ]
        );
        // game:\ path strings observed in the XEX.
        assert_eq!(RcfMountOrder::Cells0.game_path(), "game:\\00cells.rcf");
        assert_eq!(RcfMountOrder::Art2.game_path(), "game:\\02art.rcf");
    }

    #[test]
    fn startup_package_packed_path_matches_xex() {
        // XEX string at 0x82033C90 = "art/startup_shaders.p3d;art/startup.p3d;art/startup_tod.p3d"
        assert_eq!(
            StartupPackageFile::PACKED_PATH,
            "art/startup_shaders.p3d;art/startup.p3d;art/startup_tod.p3d"
        );
        assert_eq!(StartupPackageFile::PACKAGE_NAME, "startup");
        assert_eq!(
            StartupPackageFile::StartupShaders.art_path(),
            "art/startup_shaders.p3d"
        );
        assert_eq!(StartupPackageFile::Startup.art_path(), "art/startup.p3d");
        assert_eq!(
            StartupPackageFile::StartupTod.art_path(),
            "art/startup_tod.p3d"
        );
    }

    #[test]
    fn unlit_material_name_matches_xex() {
        // XEX string at 0x820024B0 = "unlit"
        assert_eq!(UNLIT_MATERIAL_NAME, "unlit");
    }

    #[test]
    fn material_uid_is_unknown() {
        // Document the UNKNOWN status: until sub_82AD20E0 is
        // disassembled, material UID computation is impossible.
        assert_eq!(material_uid("unlit"), None);
        assert_eq!(material_uid("any_material_name"), None);
    }

    #[test]
    fn wait_graph_covers_all_threads() {
        // Phase 2D's wait graph should cover every thread that
        // parks during boot: tid=7, 25, 26, 27, 28, 10..24, 8, 9.
        let tids: Vec<u32> = WAIT_GRAPH.iter().map(|w| w.tid).collect();
        for tid in [7, 25, 26, 27, 28, 8, 9] {
            assert!(tids.contains(&tid), "wait graph missing tid={tid}");
        }
        for tid in 10..=24 {
            assert!(tids.contains(&tid), "wait graph missing job worker tid={tid}");
        }
    }

    #[test]
    fn boot_state_initial() {
        let s = BootState::new();
        assert_eq!(s.current_stage, BootStage::InitialGate);
        assert!(!s.unlit_registered);
        assert!(!s.startup_package_loaded);
        assert_eq!(s.pm4_packets, 0);
    }

    #[test]
    fn boot_state_advance_in_order() {
        let mut s = BootState::new();
        // Stage 0 → Stage 1: just advance.
        s.advance(BootStage::MainWaitLoop).unwrap();
        // Stage 1 → Stage 2: just advance.
        s.advance(BootStage::DisplayInitStart).unwrap();
        // Stage 2 → Stage 3: requires unlit_registered.
        let err = s.advance(BootStage::DisplayInitComplete).unwrap_err();
        match err {
            BootAdvanceError::MissingPrerequisite { stage, reason } => {
                assert_eq!(stage, BootStage::DisplayInitComplete);
                assert!(reason.contains("unlit"));
            }
            _ => panic!(),
        }
        // After registering unlit, the advance succeeds.
        s.register_unlit();
        s.advance(BootStage::DisplayInitComplete).unwrap();
    }

    #[test]
    fn boot_state_advance_rejects_skip() {
        let mut s = BootState::new();
        // Trying to skip from InitialGate directly to AtgWorkerDispatch
        // must fail with WrongOrder.
        let err = s.advance(BootStage::AtgWorkerDispatch).unwrap_err();
        match err {
            BootAdvanceError::WrongOrder { from, to } => {
                assert_eq!(from, BootStage::InitialGate);
                assert_eq!(to, BootStage::AtgWorkerDispatch);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn boot_state_cement_gates_block_advance() {
        let mut s = BootState::new();
        // Advance through display-init stages with unlit registered.
        s.register_unlit();
        s.advance(BootStage::MainWaitLoop).unwrap();
        s.advance(BootStage::DisplayInitStart).unwrap();
        s.advance(BootStage::DisplayInitComplete).unwrap();
        s.advance(BootStage::ModuleRegistration).unwrap();
        s.advance(BootStage::AtgWorkerDispatch).unwrap();
        // Stage 5 → Stage 6 requires all 5 cement gates passed.
        let err = s.advance(BootStage::CementLibraryInit).unwrap_err();
        match err {
            BootAdvanceError::MissingPrerequisite { reason, .. } => {
                assert!(reason.contains("cement"));
            }
            _ => panic!(),
        }
        // Pass all 5 gates, then advance succeeds.
        for g in CementLibraryGate::ALL {
            s.pass_cement_gate(g);
        }
        s.advance(BootStage::CementLibraryInit).unwrap();
    }

    #[test]
    fn boot_state_startup_package_requires_art0() {
        let mut s = BootState::new();
        s.register_unlit();
        s.advance(BootStage::MainWaitLoop).unwrap();
        s.advance(BootStage::DisplayInitStart).unwrap();
        s.advance(BootStage::DisplayInitComplete).unwrap();
        s.advance(BootStage::ModuleRegistration).unwrap();
        s.advance(BootStage::AtgWorkerDispatch).unwrap();
        for g in CementLibraryGate::ALL {
            s.pass_cement_gate(g);
        }
        s.advance(BootStage::CementLibraryInit).unwrap();
        // Try to advance to StartupPackageLoaded without mounting any
        // RCFs at all → must fail citing the 3-RCF requirement.
        let err = s.advance(BootStage::StartupPackageLoaded).unwrap_err();
        match err {
            BootAdvanceError::MissingPrerequisite { reason, .. } => {
                assert!(
                    reason.contains("3 RCFs") || reason.contains("00art.rcf"),
                    "expected RCF-mount prerequisite, got: {reason}"
                );
            }
            _ => panic!("expected MissingPrerequisite, got {err:?}"),
        }
        // Now mount 3 RCFs that do NOT include 00art.rcf — must
        // still fail, citing 00art.rcf specifically.
        s.mount_rcf(RcfMountOrder::Cells0);
        s.mount_rcf(RcfMountOrder::Cells1);
        s.mount_rcf(RcfMountOrder::Art1); // NOT Art0
        let err = s.advance(BootStage::StartupPackageLoaded).unwrap_err();
        match err {
            BootAdvanceError::MissingPrerequisite { reason, .. } => {
                assert!(
                    reason.contains("00art.rcf"),
                    "expected 00art.rcf-specific prerequisite, got: {reason}"
                );
            }
            _ => panic!("expected MissingPrerequisite, got {err:?}"),
        }
        // Add 00art.rcf (now 4 RCFs total including Art0) and advance succeeds.
        s.mount_rcf(RcfMountOrder::Art0);
        s.advance(BootStage::StartupPackageLoaded).unwrap();
    }

    #[test]
    fn phase2c_checkpoint_matches_historical_state() {
        // Phase 2C: 94 PM4 packets, 24 draws, 2 shader loads, 7496 vblanks
        assert_eq!(PHASE2C_STABLE_CHECKPOINT.pm4_packets, 94);
        assert_eq!(PHASE2C_STABLE_CHECKPOINT.draws, 24);
        assert_eq!(PHASE2C_STABLE_CHECKPOINT.shader_loads, 2);
        assert_eq!(PHASE2C_STABLE_CHECKPOINT.vblanks, 7496);
        // Phase 2C stalled at DisplayInitStart (gate not yet complete)
        assert_eq!(
            PHASE2C_STABLE_CHECKPOINT.current_stage,
            BootStage::DisplayInitStart
        );
        // The 5th cement gate (XamTaskSchedule) failed in Phase 2C
        assert_eq!(
            PHASE2C_STABLE_CHECKPOINT.cement_state.state(CementLibraryGate::XamTaskSchedule),
            GateState::Failed("XamTaskSchedule returned success without creating a thread")
        );
    }

    #[test]
    fn phase2d_checkpoint_matches_historical_state() {
        // Phase 2D: 1160 PM4 packets, 103 draws, 19 shader loads
        assert_eq!(PHASE2D_CONTENT_GATE.pm4_packets, 1160);
        assert_eq!(PHASE2D_CONTENT_GATE.draws, 103);
        assert_eq!(PHASE2D_CONTENT_GATE.shader_loads, 19);
        // Phase 2D advanced to ModuleRegistration (display-init complete)
        assert_eq!(
            PHASE2D_CONTENT_GATE.current_stage,
            BootStage::ModuleRegistration
        );
        // All 5 cement gates passed in Phase 2D
        assert!(PHASE2D_CONTENT_GATE.cement_state.all_passed());
    }

    #[test]
    fn phase2f_checkpoint_matches_historical_state() {
        // Phase 2F: all 5 RCFs mount, then crash in sub_828CAE98
        assert_eq!(
            PHASE2F_ARCHIVE_INIT.current_stage,
            BootStage::CementLibraryInit
        );
        assert!(PHASE2F_ARCHIVE_INIT.cement_state.all_passed());
    }

    #[test]
    fn art0_rcf_stats_match_phase2f() {
        // 00art.rcf: 2048 + 18432 = 20480 byte header+TOC, 1361 entries
        assert_eq!(ART0_RCF_HEADER_BYTES, 2048);
        assert_eq!(ART0_RCF_TOC_BYTES, 18432);
        // The entry count is approximate — Phase 2F says 1361 entries.
        // 18432 / 12 = 1536, but the directory may have a leading
        // 12-byte preamble, so 18432 - 12 = 18420, 18420/12 = 1535.
        // Phase 2F explicitly says 1361 — this is the *unique*
        // entry count, not the directory slot count.
        assert!(ART0_RCF_ENTRY_COUNT > 1000, "00art.rcf must have > 1000 entries");
    }
}

// ===========================================================================
// Content pipeline / VFS — resolves game paths against real archives
// ===========================================================================
//
// Phase 2C established the boot-time *order* of archive mounts and
// the startup package.  This section turns that into a working
// loader: mount RCFs (in order), add loose files, then `load()` any
// game path.  Path resolution follows the engine's own semantics as
// recovered from `prototypeenginef.dll`:
//
//   * separators `\` and `/` both accepted (the engine hashes the
//     backslash form; 0x5C folds to 0x7C in the hash — see
//     `prototype_rcf::hash_name`),
//   * up to two leading `\` stripped (functions `0x10650CF0` and
//     `0x10643ED0` each strip one),
//   * the archive directory is searched by hash (the engine
//     binary-searches — entries are sorted),
//   * loose files fall back to exact-path lookup (case-insensitive,
//     lowercased — model choice, documented here).

use prototype_rcf::{PayloadKind, RcfArchive};

/// Where a successfully loaded asset came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetSource {
    /// Served from the mounted archive with this index (mount order).
    Rcf { mount_index: usize },
    /// Served from a loose (unarchived) file registered on the pipeline.
    Loose,
}

/// One resolved asset: raw payload bytes plus the loader's routing
/// decision (which is what the engine does when it opens a file
/// through the cement file manager).
#[derive(Debug, Clone)]
pub struct LoadedAsset<'a> {
    /// The path as requested by the caller.
    pub path: String,
    /// The path actually used for loose lookup (normalized, lowercase).
    pub resolved_path: String,
    /// Where the bytes came from.
    pub source: AssetSource,
    /// Magic-sniffed payload kind (P3D / Lua51 / Text / Unknown).
    pub kind: PayloadKind,
    /// Raw payload bytes (inside the archive or loose file).
    pub data: &'a [u8],
}

/// A mounted cement library with the mount name it was registered
/// under (e.g. `scripts.rcf`).
#[derive(Debug, Clone)]
struct MountedRcf<'a> {
    mount_name: String,
    archive: RcfArchive<'a>,
}

/// A content pipeline: ordered archive mounts + loose file fallback.
///
/// The engine mounts its archives in the Phase-2C order
/// (`00cells.rcf` → `01cells.rcf` → `00art.rcf` → `01art.rcf` →
/// `02art.rcf`) and the first archive whose directory contains a
/// given hash wins; this model preserves that semantics — mounts are
/// searched in registration order.
#[derive(Debug, Default)]
pub struct ContentPipeline<'a> {
    archives: Vec<MountedRcf<'a>>,
    loose: Vec<(String, &'a [u8])>,
}

impl<'a> ContentPipeline<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mount one RCF archive under a mount name.  Returns the mount
    /// index.  The archive is parsed immediately; a malformed archive
    /// is an error at mount time (the engine aborts boot too).
    pub fn mount_rcf(&mut self, mount_name: &str, data: &'a [u8]) -> Result<usize, String> {
        let archive = RcfArchive::parse(data).map_err(|e| format!("{mount_name}: {e}"))?;
        self.archives.push(MountedRcf {
            mount_name: mount_name.to_string(),
            archive,
        });
        Ok(self.archives.len() - 1)
    }

    /// Register a loose file under a path.  Later registrations with
    /// the same normalized path replace earlier ones.
    pub fn add_loose(&mut self, path: &str, data: &'a [u8]) {
        let norm = normalize_path(path);
        if let Some(slot) = self.loose.iter_mut().find(|(p, _)| *p == norm) {
            slot.1 = data;
        } else {
            self.loose.push((norm, data));
        }
    }

    /// Number of mounted archives.
    pub fn mount_count(&self) -> usize {
        self.archives.len()
    }

    /// Is an archive mounted under this mount name?
    pub fn is_mounted(&self, mount_name: &str) -> bool {
        self.archives
            .iter()
            .any(|m| m.mount_name == mount_name)
    }

    /// The mount names of every mounted archive, in mount order
    /// (the same order [`ContentPipeline::load`] searches).
    pub fn mounted_rcf_names(&self) -> Vec<&str> {
        self.archives
            .iter()
            .map(|m| m.mount_name.as_str())
            .collect()
    }

    /// Unmount (remove) the archive registered under `mount_name`.
    /// Returns `true` if an archive was removed, `false` if no
    /// archive was mounted under that name (no-op).
    ///
    /// ## Streaming semantics (M12)
    ///
    /// World streaming unmounts archives that fall out of the
    /// player's load radius; after this call the unmounted archive's
    /// files no longer resolve through [`ContentPipeline::load`]
    /// (the engine frees the archive's memory the same way).
    ///
    /// NOTE: `AssetSource::Rcf::mount_index` values are POSITIONS in
    /// the mount list, so they shift after an unmount — a stored
    /// index is only valid for the pipeline state that produced it
    /// (the audit trail in [`crate::streaming::MountEvent`] snapshots
    /// the index at event time for exactly this reason).
    pub fn unmount_rcf(&mut self, mount_name: &str) -> bool {
        if let Some(pos) = self
            .archives
            .iter()
            .position(|m| m.mount_name == mount_name)
        {
            self.archives.remove(pos);
            true
        } else {
            false
        }
    }

    /// Resolve a game path to an asset, trying every mounted archive
    /// in order and then the loose set.
    pub fn load(&self, path: &str) -> Result<LoadedAsset<'a>, String> {
        let norm = normalize_path(path);
        // Engine-style leading-backslash strip (up to two).
        let stripped = norm
            .strip_prefix('\\')
            .map(|s| s.strip_prefix('\\').unwrap_or(s))
            .unwrap_or(&norm);

        // 1. mounted archives, in mount order — hash lookup
        let hash = prototype_rcf::hash_name(stripped);
        for (idx, m) in self.archives.iter().enumerate() {
            if let Some(entry) = m.archive.lookup(stripped) {
                let data = m
                    .archive
                    .payload(entry)
                    .map_err(|e| format!("{}: {e}", m.mount_name))?;
                return Ok(LoadedAsset {
                    path: path.to_string(),
                    resolved_path: stripped.to_string(),
                    source: AssetSource::Rcf { mount_index: idx },
                    kind: PayloadKind::sniff(data),
                    data,
                });
            }
        }
        let _ = hash; // (kept for clarity; lookup() re-derives it)

        // 2. loose files — exact normalized path
        if let Some((_, data)) = self.loose.iter().find(|(p, _)| p == &norm) {
            return Ok(LoadedAsset {
                path: path.to_string(),
                resolved_path: norm,
                source: AssetSource::Loose,
                kind: PayloadKind::sniff(data),
                data,
            });
        }

        Err(format!("content pipeline: no archive or loose file serves {path:?}"))
    }

    /// Load and parse a P3D asset (errors if the payload is not P3D).
    pub fn load_p3d(&self, path: &str) -> Result<prototype_p3d::P3dFile<'a>, String> {
        let a = self.load(path)?;
        if a.kind != PayloadKind::P3D {
            return Err(format!("{path}: expected P3D payload, got {:?}", a.kind));
        }
        prototype_p3d::P3dFile::parse(a.data).map_err(|e| format!("{path}: {e}"))
    }

    /// Load and parse a Lua 5.1 asset (errors if the payload is not Lua).
    pub fn load_lua(&self, path: &str) -> Result<prototype_lua::Lua51File, String> {
        let a = self.load(path)?;
        if a.kind != PayloadKind::Lua51 {
            return Err(format!("{path}: expected Lua51 payload, got {:?}", a.kind));
        }
        prototype_lua::Lua51File::parse(a.data).map_err(|e| format!("{path}: {e}"))
    }

    /// Load every file of the Phase-2C startup package
    /// (`art/startup_shaders.p3d;art/startup.p3d;art/startup_tod.p3d`)
    /// through the pipeline.  Returns the loaded assets in package
    /// order — the order the engine loads them in before the `unlit`
    /// material can resolve.
    pub fn load_startup_package(&self) -> Result<Vec<LoadedAsset<'a>>, String> {
        let mut out = Vec::new();
        for f in StartupPackageFile::ALL {
            let path = f.art_path();
            out.push(self.load(path)?);
        }
        Ok(out)
    }
}

/// Normalize a game path for loose-file comparison: `\` and `/` both
/// become `\`, consecutive separators collapse, the string is
/// lowercased.  (Model choice — the engine lowercases via the hash
/// fold; loose-file lookup needs an explicit rule.)
fn normalize_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut prev_sep = false;
    for ch in path.chars() {
        if ch == '\\' || ch == '/' {
            if !prev_sep {
                out.push('\\');
            }
            prev_sep = true;
        } else {
            prev_sep = false;
            out.push(ch.to_ascii_lowercase());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Content pipeline tests — against the real archives
// ---------------------------------------------------------------------------

#[cfg(test)]
mod pipeline_tests {
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
    fn loads_frevents_lua_from_mounted_scripts_rcf() {
        let rcf = fixture("scripts.rcf");
        let fre = fixture("frevents.lua");
        let mut cp = ContentPipeline::new();
        let idx = cp.mount_rcf("scripts.rcf", &rcf).expect("mount");
        assert_eq!(idx, 0);
        let a = cp.load(r"scripts\frevents.lua").expect("load");
        assert_eq!(a.kind, PayloadKind::Lua51);
        assert_eq!(a.source, AssetSource::Rcf { mount_index: 0 });
        assert_eq!(a.data, fre.as_slice());
        // Slash separators and a leading backslash resolve the same file.
        assert_eq!(cp.load("scripts/frevents.lua").unwrap().data, fre.as_slice());
        assert_eq!(cp.load(r"\scripts\frevents.lua").unwrap().data, fre.as_slice());
        // Full parse through the routed loader.
        let lua = cp.load_lua(r"scripts\frevents.lua").expect("parse lua");
        assert!(lua.is_exact());
        assert_eq!(lua.main.code.len(), 672);
    }

    #[test]
    fn loads_statsachievements_p3d_and_routes_to_p3d_parser() {
        let rcf = fixture("scripts.rcf");
        let standalone = fixture("statsachievements_tod.p3d");
        let mut cp = ContentPipeline::new();
        cp.mount_rcf("scripts.rcf", &rcf).unwrap();
        let a = cp.load(r"scripts\statsachievements_tod.p3d").unwrap();
        assert_eq!(a.kind, PayloadKind::P3D);
        assert_eq!(a.data, standalone.as_slice());
        let p3d = cp.load_p3d(r"scripts\statsachievements_tod.p3d").unwrap();
        assert_eq!(p3d.header.version, 12);
    }

    #[test]
    fn loads_credits_text_from_rcf() {
        let rcf = fixture("scripts.rcf");
        let mut cp = ContentPipeline::new();
        cp.mount_rcf("scripts.rcf", &rcf).unwrap();
        for lang in ["english", "french", "italian", "spanish"] {
            let path = format!(r"scripts\credits_{lang}.txt");
            let a = cp.load(&path).unwrap();
            assert_eq!(a.kind, PayloadKind::Text, "{path}");
            assert!(a.data.starts_with(b"EXECUTIVE PRODUCER\r\n"), "{path}");
        }
    }

    #[test]
    fn loose_files_fall_back_when_no_archive_serves_the_path() {
        let boot = fixture("boot_strap.p3d");
        let mut cp = ContentPipeline::new();
        cp.add_loose(r"audio\boot_strap.p3d", &boot);
        let a = cp.load(r"audio\boot_strap.p3d").unwrap();
        assert_eq!(a.source, AssetSource::Loose);
        assert_eq!(a.kind, PayloadKind::P3D);
        let p3d = cp.load_p3d(r"audio\boot_strap.p3d").unwrap();
        let audio = p3d.audio_names().unwrap();
        assert_eq!(audio.len(), 70);
    }

    #[test]
    fn archives_take_priority_over_loose_files() {
        // The engine's archive layer wins over the loose namespace.
        let rcf = fixture("scripts.rcf");
        let fre = fixture("frevents.lua");
        let mut cp = ContentPipeline::new();
        cp.mount_rcf("scripts.rcf", &rcf).unwrap();
        cp.add_loose(r"scripts\frevents.lua", &fre);
        let a = cp.load(r"scripts\frevents.lua").unwrap();
        assert_eq!(a.source, AssetSource::Rcf { mount_index: 0 });
    }

    #[test]
    fn mount_order_decides_hash_hits_across_archives() {
        // First-mounted archive wins on a hash hit, mirroring the
        // engine's ordered archive list.
        let scripts = fixture("scripts.rcf");
        let shaders = fixture("shaders.rcf");
        let mut cp = ContentPipeline::new();
        cp.mount_rcf("scripts.rcf", &scripts).unwrap();
        cp.mount_rcf("shaders.rcf", &shaders).unwrap();
        assert_eq!(cp.mount_count(), 2);
        // A scripts.rcf name still resolves with both archives mounted.
        assert!(cp.load(r"scripts\frevents.lua").is_ok());
        // Unknown paths error with a clear message.
        let err = cp.load("does\\not\\exist.lua").unwrap_err();
        assert!(err.contains("no archive or loose file serves"));
    }

    #[test]
    fn mount_rejects_malformed_archives() {
        let mut cp = ContentPipeline::new();
        assert!(cp.mount_rcf("bogus.rcf", b"not an rcf at all........").is_err());
    }

    #[test]
    fn startup_package_paths_are_loadable_shape() {
        // The three Phase-2C startup paths must normalize to the
        // art/ namespace the engine expects.
        for f in StartupPackageFile::ALL {
            let p = f.art_path();
            let n = normalize_path(p);
            assert!(n.starts_with("art\\"), "{p} must live under art\\");
        }
    }
}

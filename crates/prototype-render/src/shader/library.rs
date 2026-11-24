//! library — the DLL's embedded world-shader PROGRAM LIBRARY and the
//! engine's shader-FAMILY registry, extracted from
//! `prototypeenginef.dll` by capstone analysis (block 32; NEXT-UP item
//! 7(d), the shader-selection call-site RE).
//!
//! ## What was proven this block (all addresses VA, image base 0x10000000)
//!
//! 1. **The program library registration (PROVEN, byte-exact)**: the
//!    constructor at VA `0x10627820` issues exactly 45 calls to
//!    `0x10627380` — one per SM3.0 blob — each
//!    `RegisterProgram(name, isVS, blob, slot?, variant?)` with the
//!    program NAME as a literal string.  The 45 blobs tile the cluster
//!    at file offsets `0x00e34b08..0x00e45250` (block-30 census), so
//!    every blob now carries a program identity.  23 VS rows + 22 PS
//!    rows across 23 names; `flatcolour` is VS-only (block-30's "VS
//!    only" observation is here explained: no PS blob is registered
//!    under that name).  The stage of every row was cross-checked
//!    against the blob's own version token (`0xFFFE0300` vs_3_0 /
//!    `0xFFFF0300` ps_3_0) — 45/45 agree with the `isVS` push.
//! 2. **Storage (PROVEN)**: the register callee stores into two
//!    name-keyed std::maps — VS map at `0x11276df0`, PS map at
//!    `0x11276e0c` (the callee branches on `isVS`, then maps-find with
//!    the name).  Draw-time lookups go through the getter pair
//!    `0x10627d20` / `0x10627e10`; the only direct callers are the
//!    wrapper layer at `0x1061b700` / `0x1061b93e` / `0x1061bcd0` (the
//!    0x2f0-byte shader-instance factory — the size equals the
//!    0x2e8-byte state block the family dispatch copies plus a header).
//! 3. **The family registry (PROVEN, byte-exact)**: 30 static objects
//!    at `0x11282c80..0x11282cf8`, each initialised by a 32-byte stub
//!    in `.text` (`push 0; push <name>; mov ecx, <obj>; call
//!    0x10039842; push <init-thunk>; call 0x10a882da`) with a family
//!    NAME string.  The 30 names live in one packed pool at VA
//!    `0x10e63600..0x10e63834`.  The registry check
//!    (`0x1054d960`) is POINTER EQUALITY of interned names — the
//!    dispatch at `0x1079c650..0x1079cb51` copies the shader instance's
//!    0x2e8-byte state, tests it against the family objects, and
//!    branches per family.  `state.field0` is the pure3dShader family
//!    string pointer (interned by the same allocator `0x1063a5d0`).
//! 4. **Case-insensitive lookup (LIKELY)**: the p3d-side loaders push
//!    camelCase program names (`depthRestore`, `FrameBuffRestore`,
//!    `flatColour`, `pointSprite`, `particleDebug`) through the same
//!    factory that the library registered lowercase names under — the
//!    map comparator must fold case (not directly disassembled).
//! 5. **Slot globals (PROVEN addresses, semantics LIKELY)**: most rows
//!    push a second global (`slot`) in `.data` — 23 VS slots in
//!    `0x110bb984..0x110bbc08` and 22 PS slots in
//!    `0x110bbc28..0x110bc258`.  No .text immediate reads them, so they
//!    are written-for-observers (debug/inspection) or read via computed
//!    addresses; the registration store path is the proven writer.
//!    The `variant` number's semantics remain UNKNOWN (it differs
//!    between the VS and PS rows of the same program).
//!
//! ## What was proven in block 33 (the item 7(d) capstone)
//!
//! 6. **The selection-edge premise DISSOLVED (PROVEN)**: shader FAMILIES
//!    never select shader CLASSES or programs.  The evidence chain:
//!    (a) the nine built-in classes are created ONCE at DLL init (the
//!    one-shot wrapper `0x107063a0` → `0x107052a0`) with hardcoded
//!    literal names — no family argument exists on that path; (b) the
//!    instance factory `0x1061bcd0` interns its NAME argument into
//!    `state.field0` (init fn `0x1061a210` stores it at `this+0x2e8`),
//!    which is the exact field the family registry check compares — an
//!    instance only matches a family when it is NAMED like one; (c) the
//!    family predicates' only consumer is the feature-flag initializer
//!    `0x1079cb60`, which writes TEN boolean flags (keys 5..14) into the
//!    instance's map at `this+0x28` (via `0x1001b0cc`), and the
//!    per-element dispatch `0x1079c650` then READS those flags (via
//!    `0x106dea40`) into per-element keys 0xb..0x1d.  Families are
//!    FEATURE FLAGS, not program selectors — see
//!    [`FAMILY_FEATURE_FLAGS`].
//! 7. **The nine built-in classes (PROVEN, byte-exact)**: extracted
//!    from `0x107052a0` (3 446 B) — `default` (12 passes: lit→default,
//!    zfill, zfillnotex, cheapLit0..7→error, clear→fastclear), `error`,
//!    `pointSprite`, `unlit`, `textureGutter`, `occlusionblur`,
//!    `legacy` (11 passes incl. its own cheapLit set), `bink`,
//!    `particleDebug`.  There is NO `zfill` class and NO `fastclear`
//!    class (block 32's list was wrong in detail — both are PROGRAMS
//!    referenced from other classes' pass tables); the cheapLit0..7
//!    fallback passes belong to the `default` and `legacy` classes.
//!    The factory's third argument is the pass-table capacity and
//!    equals the row count for 8 of 9 classes (`particleDebug`: 5 rows
//!    vs capacity 11 — pre-allocated spare slots, honest note).  See
//!    [`BUILTIN_CLASSES`].
//! 8. **The art-side shader source (PROVEN on real art data)**: the
//!    family strings ride in the art .p3d `0x00011015` pure3dShader
//!    chunks as their second pascal string (45 chunks censused over 7
//!    re-downloaded probe members; 18 distinct families observed, only
//!    5 of them inside the 30-entry registry — the registry is the
//!    feature-map subset, not the family universe).  Short feature
//!    codes (`1cv`, `1c1a`, `1c1n`, `1c1n1a`) appear as family strings
//!    exactly as block 30 hypothesised.  The chunk loader class is
//!    named `proto::ShaderConfigureFixup` (type-name string at
//!    `0x10d96db8`, vtable `0x10eafa30`).
//!
//! ## What was proven in block 37 (the loader family disentangled)
//!
//! 9. **The factory xref is COMPLETE and the loader family is SPLIT
//!    (PROVEN)**: an exhaustive raw E8/E9 rel32 scan over all 14.2 MB
//!    of `.text` (validated by positive controls: 2709/8588/1038 hits
//!    on known-hot callees; the earlier capstone-walk negatives were an
//!    alignment artifact) finds exactly **13 direct call sites** of the
//!    instance factory [`addrs::FACTORY_FN`]: 12 are the engine-side
//!    class definitions (the nine built-in classes + depthRestore's
//!    passes, static literal names) and **1 is the content-side site
//!    `0x10b95948`** inside the parse `0x10b95780` — slot 7 of the
//!    `0x00011009` loader vtable `0x10eafc50` (ctor `0x10b95740` pins
//!    chunk id 0x11009 at object+8).  That parse reads the chunk's
//!    FIRST pascal string into a frame buffer and passes THAT buffer
//!    as the factory's NAME argument (buffer identity proven by
//!    symbolic-ESP tracking: the same frame slot is lea'd at
//!    `0x10b957b2` for the stream read and at `0x10b95940` for the
//!    factory call; args 2/3 are u32 stream fields).  Real
//!    `0x11009` chunks (`frontendshaders.p3d`, 12×) carry READABLE
//!    instance names (`CxformGauraud`, `hud_2dmarker`, …).
//! 10. **The `0x00011015` fixup parse NEVER factories (PROVEN)**: its
//!    own loader (`0x10eafcb8`, ctor `0x10b961d0` pins chunk id
//!    0x11015; distinct from the `0x10eafa30` vtable block 34 walked)
//!    has parse `0x10b96280` — 505 instructions, 35 stream reads
//!    (name pascal → u32 → family pascal → ~30 fields), 8 string
//!    interns, ZERO factory calls.  It allocates a 0x30-B wrapper from
//!    the FAMILY string, virtual-calls into a state object, touches
//!    state+0x2e8 (the instance name field [`addrs::FIELD_NAME`]),
//!    runs the family registry check (the ONLY caller chain of
//!    `0x1054d960`, via its unique ILT thunk `0x100493e1`) and gates a
//!    callback on the result.  **Conclusion**: shader instances are
//!    NAME-keyed (created by the 0x11009 loader under the chunk's name
//!    string); the `0x11015` chunk is per-name configuration — the
//!    block-34 "hash name vs family string" question DISSOLVED (the
//!    fixup loader does neither: it does not create instances at all).
//!
//! ## What stays UNKNOWN (honest)
//!
//! - The exact micro-semantics of the fixup parse's `state+0x2e8`
//!   string-assign (the callee `0x1008cc20` is a 2-arg interned-string
//!   assign, but the call site pushes a third value — register-level
//!   detail unresolved; candidates: error-fallback name write, or a
//!   name COPY into the state).  The architecture around it is proven.
//! - The meaning of each feature KEY (what the renderer does with flag
//!   5 vs flag 12) — the family→key mapping is proven, the key→render
//!   behaviour is not.
//! - The `variant` field semantics; the exact role of the 4
//!   non-registry stubs (`brightness`/`contrast`/`midpoint`/
//!   `saturation` — param-name setter stubs in the same shape, objects
//!   at `0x11282d78+`, excluded from [`FAMILY_REGISTRY`]).
//! - The two int args of most AddPass rows are (0, 0); every `zfill`
//!   row carries (0, 1) — one bit of per-pass state, meaning unknown.

/// The pipeline stage a registered blob serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Vertex shader (`vs_3_0`, version token `0xFFFE0300`).
    Vs,
    /// Fragment/pixel shader (`ps_3_0`, version token `0xFFFF0300`).
    Ps,
}

impl Stage {
    /// The blob version token that marks this stage.
    pub fn version_token(self) -> u32 {
        match self {
            Stage::Vs => 0xFFFE_0300,
            Stage::Ps => 0xFFFF_0300,
        }
    }
}

/// One `RegisterProgram(name, isVS, blob, slot?, variant?)` call in the
/// DLL's shader-library constructor (VA `0x10627820`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgramRow {
    /// Program name as registered (lowercase; the p3d-side loaders use
    /// camelCase spellings that must fold — see the module docs).
    pub name: &'static str,
    /// Pipeline stage (cross-checked against the blob version token).
    pub stage: Stage,
    /// Blob file offset inside `prototypeenginef.dll` (the blob cluster
    /// `0x00e34b08..0x00e45250`; equals VA - image base for that range).
    pub blob_offset: u32,
    /// The `.data` slot global pushed by the registration block, when
    /// present (`None` for 8 PS rows registered without one).
    pub slot_va: Option<u32>,
    /// The extra per-row number pushed by the registration block —
    /// semantics UNKNOWN (differs between the VS and PS rows of one
    /// program; e.g. `default` VS 6 vs PS 30).
    pub variant: u32,
}

/// One shader-family registry entry (the 30 static objects the family
/// dispatch matches the pure3dShader family string against).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FamilyEntry {
    /// Family name (one packed string pool at VA `0x10e63600`).
    pub name: &'static str,
    /// The static object VA the init stub writes the name into.
    pub obj_va: u32,
}

/// The 45 registration rows, in constructor issue order (19 VS rows for
/// the first 19 programs, then 18 PS rows, then the 4 late VS/PS pairs
/// gbuffer/lightstencil/pointlight/spotlight).
pub const PROGRAM_LIBRARY: &[ProgramRow] = &[
    row("bink", Stage::Vs, 0x00e34b08, 0x110bb984, 0x1),
    row("default", Stage::Vs, 0x00e34e60, 0x110bb998, 0x6),
    row("depth", Stage::Vs, 0x00e358e8, 0x110bb9f8, 0x1),
    row("depthrestore", Stage::Vs, 0x00e35c80, 0x110bba08, 0x1),
    row("error", Stage::Vs, 0x00e36010, 0x110bba18, 0x1),
    row("fastclear", Stage::Vs, 0x00e36278, 0x110bba28, 0x1),
    row("framebuffrestore", Stage::Vs, 0x00e36580, 0x110bba38, 0x1),
    row("legacy", Stage::Vs, 0x00e36918, 0x110bba48, 0x5),
    row("lightcount", Stage::Vs, 0x00e37238, 0x110bba98, 0x5),
    row("orthonormalbasis", Stage::Vs, 0x00e37858, 0x110bbae8, 0x3),
    row("particledebug", Stage::Vs, 0x00e37e70, 0x110bbb18, 0x1),
    row("shadowblur", Stage::Vs, 0x00e38258, 0x110bbb28, 0x2),
    row("shadowdepthfloat", Stage::Vs, 0x00e393f8, 0x110bbbb8, 0x1),
    row("shadowdebug", Stage::Vs, 0x00e38790, 0x110bbb48, 0x7),
    row("unlit", Stage::Vs, 0x00e39850, 0x110bbbc8, 0x1),
    row("zfill", Stage::Vs, 0x00e39bd8, 0x110bbbd8, 0x1),
    row("zfillnotex", Stage::Vs, 0x00e3a018, 0x110bbbe8, 0x1),
    row("pointsprite", Stage::Vs, 0x00e3a3a8, 0x110bbbf8, 0x1),
    row("flatcolour", Stage::Vs, 0x00e3aa70, 0x110bbc08, 0x2),
    row("bink", Stage::Ps, 0x00e3adf8, 0x110bbc28, 0x4),
    row("default", Stage::Ps, 0x00e3b308, 0x110bbc68, 0x1e),
    row("depth", Stage::Ps, 0x00e3cdc8, 0x110bbe48, 0x1),
    row("depthrestore", Stage::Ps, 0x00e3d088, 0x110bbe58, 0x2),
    row("error", Stage::Ps, 0x00e3d3d0, 0, 0x0),
    row("fastclear", Stage::Ps, 0x00e3d570, 0, 0x0),
    row("framebuffrestore", Stage::Ps, 0x00e3d730, 0x110bbe78, 0x1),
    row("legacy", Stage::Ps, 0x00e3d9b0, 0x110bbe88, 0xf),
    row("lightcount", Stage::Ps, 0x00e3e880, 0, 0x0),
    row("orthonormalbasis", Stage::Ps, 0x00e3ea68, 0x110bbf78, 0x2),
    row("particledebug", Stage::Ps, 0x00e3f2d0, 0x110bbf98, 0x3),
    row("shadowblur", Stage::Ps, 0x00e3f858, 0x110bbfc8, 0x5),
    row("shadowdepthfloat", Stage::Ps, 0x00e41420, 0, 0x0),
    row("shadowdebug", Stage::Ps, 0x00e405f0, 0x110bc018, 0x12),
    row("unlit", Stage::Ps, 0x00e41670, 0x110bc138, 0x1),
    row("zfill", Stage::Ps, 0x00e418d8, 0, 0x0),
    row("zfillnotex", Stage::Ps, 0x00e41b10, 0, 0x0),
    row("pointsprite", Stage::Ps, 0x00e41cf0, 0x110bc148, 0x1),
    row("gbuffer", Stage::Vs, 0x00e42230, 0x110bc158, 0x2),
    row("gbuffer", Stage::Ps, 0x00e42690, 0x110bc178, 0x1),
    row("lightstencil", Stage::Vs, 0x00e42b60, 0x110bc188, 0x1),
    row("lightstencil", Stage::Ps, 0x00e42df8, 0, 0x0),
    row("pointlight", Stage::Vs, 0x00e42fc0, 0x110bc198, 0x1),
    row("pointlight", Stage::Ps, 0x00e432f0, 0x110bc1a8, 0xa),
    row("spotlight", Stage::Vs, 0x00e440a0, 0x110bc248, 0x1),
    row("spotlight", Stage::Ps, 0x00e443c8, 0x110bc258, 0xc),
];

/// The 30 shader-family registry entries (stub issue order, which is
/// NOT the pool order — the objects were assigned addresses per class
/// group; see the module docs for the dispatch evidence).
pub const FAMILY_REGISTRY: &[FamilyEntry] = &[
    fam("char_hair", 0x11282c88),
    fam("env_sky_dynamic", 0x11282cb0),
    fam("env_road", 0x11282cac),
    fam("zCBV2_env_road", 0x11282cdc),
    fam("env_sidewalk", 0x11282c9c),
    fam("zCBV2_env_sidewalk", 0x11282cb8),
    fam("env_decal_bias_nm", 0x11282c8c),
    fam("env_water", 0x11282cc8),
    fam("env_midground_traffic", 0x11282c84),
    fam("env_midground_tree", 0x11282ce8),
    fam("env_vehicle_colorvar", 0x11282ccc),
    fam("env_vehicle_glass_fade", 0x11282cd0),
    fam("env_car_tire", 0x11282cc0),
    fam("pp_obj_distortion", 0x11282cc4),
    fam("pfx_obj_distortion", 0x11282c80),
    fam("env_decal", 0x11282ca8),
    fam("env_decal_nm", 0x11282ca0),
    fam("env_light_decal_spot", 0x11282cec),
    fam("env_midground_lights", 0x11282ce4),
    fam("pp_fog_buffer", 0x11282cbc),
    fam("env_haze", 0x11282c90),
    fam("env_midground", 0x11282ce0),
    fam("env_hive_exterior", 0x11282cf4),
    fam("env_hive_decal", 0x11282c98),
    fam("env_hive_interior", 0x11282cb4),
    fam("env_midground_road", 0x11282c94),
    fam("env_midground_shoreline", 0x11282cf0),
    fam("env_midground_shorelinebuildings", 0x11282cd8),
    fam("fx_cbvcolor_alpha", 0x11282ca4),
    fam("hud_icon", 0x11282cd4),
];

/// The blob cluster's file-offset range inside `prototypeenginef.dll`
/// (block-30: 45 blobs tile it with exactly one zero DWORD between
/// neighbours).
pub const BLOB_CLUSTER: (u32, u32) = (0x00e3_4b08, 0x00e4_5250);

/// Key addresses the extraction relied on (VA, image base 0x10000000).
/// Kept as named constants so future RE passes can pin against them.
pub mod addrs {
    /// The registration constructor issuing the 45 `RegisterProgram`
    /// calls.
    pub const REGISTRATION_FN: u32 = 0x1062_7820;
    /// The register callee (std::map insert; branches on isVS).
    pub const REGISTER_FN: u32 = 0x1062_7380;
    /// The VS program map (name-keyed std::map header in .data).
    pub const VS_MAP: u32 = 0x1127_6df0;
    /// The PS program map.
    pub const PS_MAP: u32 = 0x1127_6e0c;
    /// Program getter A (used for the VS stage among others).
    pub const GETTER_A: u32 = 0x1062_7d20;
    /// Program getter B.
    pub const GETTER_B: u32 = 0x1062_7e10;
    /// The 0x2f0-byte shader-instance factory (name-keyed).
    pub const FACTORY_FN: u32 = 0x1061_bcd0;
    /// The interned-name pointer-equality registry check.
    pub const FAMILY_CHECK_FN: u32 = 0x1054_d960;
    /// The family dispatch function (state copy + per-family branches).
    pub const DISPATCH_FN: u32 = 0x1079_c650;
    /// The interned family-name string pool start (VA).
    pub const NAME_POOL: u32 = 0x10e6_3600;

    // --- block 33: the class/feature layer -----------------------------
    /// The one-shot init wrapper: `call 0x10704b70; call MASTER9; set
    /// byte [0x11281338] = 1; ret` (the built-in classes are created
    /// exactly once, at DLL init).
    pub const CLASSDEF_INIT_WRAPPER: u32 = 0x1070_63a0;
    /// The MASTER class-definition function (3 446 B): creates the nine
    /// built-in shader instances via the factory and registers their
    /// hardcoded pass tables with `AddPass(pass, requested, canonical,
    /// int, int)` virtual calls (vtable slot 4).
    pub const CLASSDEF_MASTER_FN: u32 = 0x1070_52a0;
    /// The shader-instance constructor thunk (assigns vtable
    /// `0x10e453ec`, delegates to [`INSTANCE_INIT_FN`]).
    pub const INSTANCE_CTOR_THUNK: u32 = 0x1062_8380;
    /// The instance init: interns the NAME argument to `this+0x2e8`
    /// (the field the family registry check compares), stores the
    /// pass-table capacity at `this+0x2dc` and allocates the pass
    /// array at `this+0x2d0` (`(arg2+8) * 4` bytes).
    pub const INSTANCE_INIT_FN: u32 = 0x1061_a210;
    /// The 0x2e8-byte state-copy callee used by both dispatch layers.
    pub const STATE_COPY_FN: u32 = 0x1061_1f30;
    /// The feature-flag initializer (called per instance): evaluates
    /// the 30 family predicates against the instance name and writes
    /// ten boolean flags (keys 5..14) into the map at `this+0x28`.
    pub const FEATURE_FLAG_INIT_FN: u32 = 0x1079_cb60;
    /// The map-write callee (`0x1001b0cc`; this=key/value map).
    pub const MAP_WRITE_FN: u32 = 0x1001_b0cc;
    /// The map-read callee (`0x106dea40`; returns the bool for a key).
    pub const MAP_GET_FN: u32 = 0x106d_ea40;
    /// JMP-thunk page entry for the registry check (one `jmp
    /// 0x1054d960`; the ICFS thunk page at `0x100493e1`).
    pub const REGISTRY_CHECK_THUNK: u32 = 0x1004_93e1;
    /// The shader-instance vtable assigned by [`INSTANCE_INIT_FN`]
    /// (the ctor thunk then overwrites with `0x10e453ec`).
    pub const INSTANCE_VTABLE: u32 = 0x10e3_3d50;
    /// The art-side `0x00011015` pure3dShader chunk-loader vtable (the
    /// loader object also serves chunk types 0x7000/0x7001/0x1001d).
    /// Block 37: the ctor (`0x10b91e20`) stores chunk id 0 at object+8
    /// (generic), and the slot-7 parse `0x10b91e40` never calls the
    /// factory.
    pub const SHADER_CHUNK_LOADER_VT: u32 = 0x10ea_fa30;
    /// Type-name string of the shader-chunk loader class
    /// (`proto::ShaderConfigureFixup…`).
    pub const SHADER_CHUNK_LOADER_TYPENAME: u32 = 0x10d9_6db8;

    // --- block 37: the loader family disentangled ----------------------
    /// The loader-family base vtable (slots 0/1 shared by every p3d
    /// loader class; the ctor writes it first, then the derived vtable).
    pub const LOADER_BASE_VT: u32 = 0x10d9_6e60;
    /// The `0x00011009` shader-INSTANCE loader vtable (ctor `0x10b95740`
    /// stores chunk id 0x11009 at object+8; slot 7 = parse).
    pub const INSTANCE_LOADER_VT: u32 = 0x10ea_fc50;
    /// The `0x00011015` pure3dShader loader vtable (ctor `0x10b961d0`
    /// stores chunk id 0x11015 at object+8; slot 7 = parse).  Distinct
    /// from [`SHADER_CHUNK_LOADER_VT`] — block 37 split the family.
    pub const FIXUP_LOADER_VT: u32 = 0x10eaf_cb8;
    /// The content-side instance-creation parse (slot 7 of
    /// [`INSTANCE_LOADER_VT`]): reads the first pascal string, calls
    /// [`FACTORY_FN`] at [`CONTENT_FACTORY_SITE`], then walks child
    /// chunks 0x1100A/0x1100E.
    pub const INSTANCE_PARSE_FN: u32 = 0x10b9_5780;
    /// The exact call site of the factory inside [`INSTANCE_PARSE_FN`]
    /// (one of exactly 13 direct call sites in the whole .text).
    pub const CONTENT_FACTORY_SITE: u32 = 0x10b9_5948;
    /// The shader-manager singleton the content factory call targets
    /// (`this` = the global at `0x1127f358`).
    pub const MANAGER_GLOBAL: u32 = 0x1127_f358;
    /// The `0x00011015` fixup parse (slot 7 of [`FIXUP_LOADER_VT`]):
    /// reads name pascal, u32, family pascal, ~30 more fields; interns
    /// 8 strings; NEVER calls the factory; runs the family registry
    /// check (via [`REGISTRY_CHECK_THUNK`]) and gates a callback.
    pub const FIXUP_PARSE_FN: u32 = 0x10b9_6280;
    /// The stream-read primitive every parse uses (`(this, buf, n, a,
    /// b)`, stdcall `ret 0x10`).
    pub const STREAM_READ_FN: u32 = 0x1060_b260;
    /// Instance-object field offsets proven this block.
    pub const FIELD_FEATURE_MAP: u32 = 0x28;
    /// The pass-table array field (allocated `(capacity+8) * 4` B).
    pub const FIELD_PASS_TABLE: u32 = 0x2d0;
    /// The pass-table capacity field.
    pub const FIELD_PASS_CAPACITY: u32 = 0x2dc;
    /// The interned instance-name field — `state.field0` of the
    /// dispatch state block, the family-registry comparison target.
    pub const FIELD_NAME: u32 = 0x2e8;
}

/// Constructor helper (keeps the table rows single-line).
#[allow(clippy::too_many_arguments)]
const fn row(
    name: &'static str,
    stage: Stage,
    blob_offset: u32,
    slot_va: u32,
    variant: u32,
) -> ProgramRow {
    ProgramRow { name, stage, blob_offset, slot_va: if slot_va == 0 { None } else { Some(slot_va) }, variant }
}

/// Constructor helper for family entries.
const fn fam(name: &'static str, obj_va: u32) -> FamilyEntry {
    FamilyEntry { name, obj_va }
}

/// All distinct program names in registration order (23).
pub fn program_names() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for r in PROGRAM_LIBRARY {
        if !out.contains(&r.name) {
            out.push(r.name);
        }
    }
    out
}

/// The (VS, PS) rows registered under `name`, if any.  `flatcolour`
/// returns `(Some(vs), None)`; unknown names return `None`.
pub fn program_pair(name: &str) -> Option<(Option<&'static ProgramRow>, Option<&'static ProgramRow>)> {
    let lower = name.to_ascii_lowercase();
    let mut vs = None;
    let mut ps = None;
    for r in PROGRAM_LIBRARY {
        if r.name == lower {
            match r.stage {
                Stage::Vs => vs = Some(r),
                Stage::Ps => ps = Some(r),
            }
        }
    }
    if vs.is_none() && ps.is_none() {
        None
    } else {
        Some((vs, ps))
    }
}

/// The family registry entry whose name matches (case-sensitive — the
/// registry names carry the exact corpus spellings, including the
/// `zCBV2_` prefix).
pub fn family_entry(name: &str) -> Option<&'static FamilyEntry> {
    FAMILY_REGISTRY.iter().find(|f| f.name == name)
}

// ---------------------------------------------------------------------------
// Block 33: the family→feature-flag layer and the nine built-in classes.
// ---------------------------------------------------------------------------

/// Feature-map keys written by the flag initializer `0x1079cb60` into
/// every shader instance (`this+0x28`, ten entries, keys 5..14 — all
/// written for EVERY instance; the family determines the values).
/// Key 10 is a runtime query (`vcall(...0x112817ac) != -1`), not
/// family-derived, so it has no row in [`FAMILY_FEATURE_FLAGS`].
pub mod feature_keys {
    /// Key 5: distortion post/prefix effects.
    pub const DISTORTION: u32 = 5;
    /// Key 6: fog buffer.
    pub const FOG_BUFFER: u32 = 6;
    /// Key 7: midground group membership (1 iff any of the seven
    /// `MIDGROUND_FAMILIES`, else 0 — the only key with an explicit
    /// else-write).
    pub const MIDGROUND: u32 = 7;
    /// Key 8: decals.
    pub const DECAL: u32 = 8;
    /// Key 9: light decals.
    pub const LIGHT_DECAL: u32 = 9;
    /// Key 10: runtime query, not family-static.
    pub const RUNTIME_QUERY: u32 = 10;
    /// Key 11: colour-alpha constant buffer / HUD icons.
    pub const CBV_COLOR_ALPHA: u32 = 11;
    /// Key 12: vehicle family group.
    pub const VEHICLE: u32 = 12;
    /// Key 13: haze.
    pub const HAZE: u32 = 13;
    /// Key 14: hive interior/exterior/decal group.
    pub const HIVE: u32 = 14;
}

/// The seven midground families — the key-7 disjunction (the per-element
/// dispatch at `0x1079c650` also ORs these seven for its own group test).
pub const MIDGROUND_FAMILIES: &[&str] = &[
    "env_midground",
    "env_midground_lights",
    "env_midground_tree",
    "env_midground_road",
    "env_midground_shoreline",
    "env_midground_shorelinebuildings",
    "env_midground_traffic",
];

/// The feature keys each registry family asserts (value 1).  Families
/// not listed here — `char_hair`, `env_sky_dynamic`, `env_road`,
/// `zCBV2_env_road`, `env_sidewalk`, `zCBV2_env_sidewalk`,
/// `env_water` — assert NO keys (all ten flags false; key 7 = 0).
/// Derived byte-exact from the `0x1079cb60` initializer.
pub const FAMILY_FEATURE_FLAGS: &[(&str, &[u32])] = &[
    ("pp_obj_distortion", &[feature_keys::DISTORTION]),
    ("pfx_obj_distortion", &[feature_keys::DISTORTION]),
    ("pp_fog_buffer", &[feature_keys::FOG_BUFFER]),
    ("env_decal", &[feature_keys::DECAL]),
    ("env_decal_nm", &[feature_keys::DECAL]),
    ("env_decal_bias_nm", &[feature_keys::DECAL]),
    ("env_light_decal_spot", &[feature_keys::LIGHT_DECAL]),
    ("env_midground_lights", &[feature_keys::LIGHT_DECAL, feature_keys::MIDGROUND]),
    ("fx_cbvcolor_alpha", &[feature_keys::CBV_COLOR_ALPHA]),
    ("hud_icon", &[feature_keys::CBV_COLOR_ALPHA]),
    ("env_vehicle_colorvar", &[feature_keys::VEHICLE]),
    ("env_vehicle_glass_fade", &[feature_keys::VEHICLE]),
    ("env_car_tire", &[feature_keys::VEHICLE]),
    ("env_haze", &[feature_keys::HAZE]),
    ("env_hive_exterior", &[feature_keys::HIVE]),
    ("env_hive_decal", &[feature_keys::HIVE]),
    ("env_hive_interior", &[feature_keys::HIVE]),
    ("env_midground", &[feature_keys::MIDGROUND]),
    ("env_midground_tree", &[feature_keys::MIDGROUND]),
    ("env_midground_road", &[feature_keys::MIDGROUND]),
    ("env_midground_shoreline", &[feature_keys::MIDGROUND]),
    ("env_midground_shorelinebuildings", &[feature_keys::MIDGROUND]),
    ("env_midground_traffic", &[feature_keys::MIDGROUND]),
];

/// The keys a family asserts (empty for ungrouped families; unknown
/// family names yield an empty list too — the engine's default is
/// all-false plus key 7 = 0).
pub fn family_feature_keys(name: &str) -> Vec<u32> {
    FAMILY_FEATURE_FLAGS
        .iter()
        .filter(|(fam, _)| *fam == name)
        .flat_map(|(_, keys)| keys.iter().copied())
        .collect()
}

/// One `AddPass(pass, requested, canonical, flag1, flag2)` row of a
/// built-in class pass table (the virtual-call argument quintuple,
/// pushed in reverse; extracted byte-exact from `0x107052a0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassPassRow {
    /// The pass name the render path requests.
    pub pass: &'static str,
    /// The program name the pass was defined with (camelCase spellings
    /// preserved from the DLL literals).
    pub requested: &'static str,
    /// The canonical (registered-library) program name it resolves to.
    pub canonical: &'static str,
    /// First int arg (0 on every extracted row).
    pub flag1: u32,
    /// Second int arg (1 on `zfill` rows, 0 elsewhere; meaning UNKNOWN).
    pub flag2: u32,
}

/// One built-in shader class: a named program-instance prototype with
/// a hardcoded pass table, created once at DLL init by
/// `0x107052a0` via `factory(name, 0, capacity)`.
#[derive(Debug, Clone, Copy)]
pub struct ShaderClass {
    /// The instance name the factory registers under.
    pub name: &'static str,
    /// The factory's capacity argument (the init allocates
    /// `(0 + 8) * 4` bytes for the pass array regardless; the capacity
    /// rides at `this+0x2dc`).  Equals the row count for every class
    /// except `particleDebug` (5 rows, capacity 11).
    pub pass_capacity: u32,
    /// The pass rows in registration order.
    pub passes: &'static [ClassPassRow],
}

/// Constructor helper.
const fn pass(
    p: &'static str,
    req: &'static str,
    canon: &'static str,
    f2: u32,
) -> ClassPassRow {
    ClassPassRow { pass: p, requested: req, canonical: canon, flag1: 0, flag2: f2 }
}

/// The nine built-in classes, byte-exact from the DLL.  NOTE: there is
/// NO `zfill` class and NO `fastclear` class — both are PROGRAMS the
/// classes reference; the cheapLit0..7 fallback passes belong to
/// `default` and `legacy`.  Two referenced programs are NOT blob-backed
/// in the 45-row library (`texturegutter`, `occlusionblur` — the
/// engine binds them from p3d-shipped shader data instead).
pub const BUILTIN_CLASSES: &[ShaderClass] = &[
    ShaderClass {
        name: "default",
        pass_capacity: 0xc,
        passes: &[
            pass("lit", "default", "default", 0),
            pass("zfill", "zfill", "zfill", 1),
            pass("zfillnotex", "zfillnotex", "zfillnotex", 1),
            pass("cheapLit0", "error", "error", 0),
            pass("cheapLit1", "error", "error", 0),
            pass("cheapLit2", "error", "error", 0),
            pass("cheapLit3", "error", "error", 0),
            pass("cheapLit4", "error", "error", 0),
            pass("cheapLit5", "error", "error", 0),
            pass("cheapLit6", "error", "error", 0),
            pass("cheapLit7", "error", "error", 0),
            pass("clear", "fastclear", "fastclear", 0),
        ],
    },
    ShaderClass {
        name: "error",
        pass_capacity: 4,
        passes: &[
            pass("lit", "error", "error", 0),
            pass("unlit", "error", "error", 0),
            pass("clear", "error", "error", 0),
            pass("zfill", "zfill", "zfill", 1),
        ],
    },
    ShaderClass {
        name: "pointSprite",
        pass_capacity: 5,
        passes: &[
            pass("lit", "pointSprite", "pointSprite", 0),
            pass("pointSprite", "pointSprite", "pointSprite", 0),
            pass("unlit", "pointSprite", "pointSprite", 0),
            pass("clear", "fastclear", "fastclear", 0),
            pass("zfill", "zfill", "zfill", 1),
        ],
    },
    ShaderClass {
        name: "unlit",
        pass_capacity: 5,
        passes: &[
            pass("lit", "unlit", "unlit", 0),
            pass("unlit", "unlit", "unlit", 0),
            pass("zfill", "zfill", "zfill", 1),
            pass("error", "error", "error", 0),
            pass("clear", "fastclear", "fastclear", 0),
        ],
    },
    ShaderClass {
        name: "textureGutter",
        pass_capacity: 6,
        passes: &[
            pass("textureGutter", "texturegutter", "texturegutter", 0),
            pass("lit", "unlit", "unlit", 0),
            pass("unlit", "unlit", "unlit", 0),
            pass("zfill", "zfill", "zfill", 1),
            pass("error", "error", "error", 0),
            pass("clear", "fastclear", "fastclear", 0),
        ],
    },
    ShaderClass {
        name: "occlusionblur",
        pass_capacity: 1,
        passes: &[pass("occlusionblur", "occlusionblur", "occlusionblur", 0)],
    },
    ShaderClass {
        name: "legacy",
        pass_capacity: 0xb,
        passes: &[
            pass("lit", "legacy", "legacy", 0),
            pass("zfill", "zfill", "zfill", 1),
            pass("cheapLit0", "error", "error", 0),
            pass("cheapLit1", "error", "error", 0),
            pass("cheapLit2", "error", "error", 0),
            pass("cheapLit3", "error", "error", 0),
            pass("cheapLit4", "error", "error", 0),
            pass("cheapLit5", "error", "error", 0),
            pass("cheapLit6", "error", "error", 0),
            pass("cheapLit7", "error", "error", 0),
            pass("clear", "fastclear", "fastclear", 0),
        ],
    },
    ShaderClass {
        name: "bink",
        pass_capacity: 1,
        passes: &[pass("lit", "bink", "bink", 0)],
    },
    ShaderClass {
        name: "particleDebug",
        pass_capacity: 0xb,
        passes: &[
            pass("lit", "particledebug", "particledebug", 0),
            pass("unlit", "particledebug", "particledebug", 0),
            pass("zfill", "zfill", "zfill", 1),
            pass("error", "error", "error", 0),
            pass("clear", "fastclear", "fastclear", 0),
        ],
    },
];

/// A built-in class by name (exact spelling — the class names are the
/// factory literals: `pointSprite`/`textureGutter`/`particleDebug` are
/// camelCase).
pub fn builtin_class(name: &str) -> Option<&'static ShaderClass> {
    BUILTIN_CLASSES.iter().find(|c| c.name == name)
}

/// The 18 distinct family strings observed in the art data's
/// `0x00011015` pure3dShader chunks (45 chunks over the 7 re-downloaded
/// probe members: van/carOS005/truckOS001/sedanOS001/bloodtoxFacility/
/// tankerTruck/beam-skyLight).  Only the 5 marked in tests are inside
/// [`FAMILY_REGISTRY`] — the registry is the feature-map subset, not
/// the family universe.
pub const ART_OBSERVED_FAMILIES: &[&str] = &[
    "1c1a",
    "1c1n",
    "1c1n1a",
    "1cv",
    "env_building_noglass",
    "env_car_tire",
    "env_car_tire_no_vertex_blend",
    "env_color_noglass",
    "env_decal",
    "env_decal_nm",
    "env_debris",
    "env_glass",
    "env_vehicle_colorvar",
    "env_vehicle_glass_fade",
    "env_vehicle_lod",
    "env_vehicle_militarybw",
    "fx_add_soft",
    "prop_damage_2nm_vtx",
];

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_DIR: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../prototype-bin/tests/fixtures"
    );

    /// 45 rows: 23 VS + 22 PS across 23 distinct names; only
    /// `flatcolour` lacks a PS row.
    #[test]
    fn program_library_census() {
        assert_eq!(PROGRAM_LIBRARY.len(), 45);
        let vs = PROGRAM_LIBRARY.iter().filter(|r| r.stage == Stage::Vs).count();
        let ps = PROGRAM_LIBRARY.iter().filter(|r| r.stage == Stage::Ps).count();
        assert_eq!(vs, 23);
        assert_eq!(ps, 22);
        let names = program_names();
        assert_eq!(names.len(), 23);
        for name in names {
            let (vs, ps) = program_pair(name).expect("pair");
            match name {
                "flatcolour" => {
                    assert!(vs.is_some() && ps.is_none(), "flatcolour is VS-only")
                }
                _ => assert!(vs.is_some() && ps.is_some(), "{name} needs both stages"),
            }
        }
    }

    /// Every blob offset sits inside the block-30 cluster and no offset
    /// repeats (the 45 blobs tile the cluster one-to-one).
    #[test]
    fn blob_offsets_tile_the_cluster() {
        let (lo, hi) = BLOB_CLUSTER;
        let mut offs: Vec<u32> = PROGRAM_LIBRARY.iter().map(|r| r.blob_offset).collect();
        assert_eq!(offs.len(), 45);
        offs.sort_unstable();
        offs.dedup();
        assert_eq!(offs.len(), 45, "offsets must be unique");
        for r in PROGRAM_LIBRARY {
            assert!(r.blob_offset >= lo && r.blob_offset < hi, "{:x}", r.blob_offset);
        }
    }

    /// The registration block's `isVS` push was cross-checked against
    /// the blob version token 45/45 during extraction; the table must
    /// stay self-consistent with the stage tokens the decoder knows.
    #[test]
    fn stage_tokens_match_decoder_convention() {
        assert_eq!(Stage::Vs.version_token(), 0xFFFE_0300);
        assert_eq!(Stage::Ps.version_token(), 0xFFFF_0300);
        // The 9 committed fixtures carry these tokens on disk.
        for (file, want) in [
            ("dll_default_vp_e34e60.sm30", Stage::Vs),
            ("dll_default_fp_e3b308.sm30", Stage::Ps),
            ("dll_bink_fp_e3adf8.sm30", Stage::Ps),
            ("dll_error_vp_e36010.sm30", Stage::Vs),
            ("dll_lightcount_vp_e37238.sm30", Stage::Vs),
            ("dll_pointsprite_fp_e41cf0.sm30", Stage::Ps),
            ("dll_shadowblur_fp_e3f858.sm30", Stage::Ps),
            ("dll_shadowdebug_fp_e405f0.sm30", Stage::Ps),
            ("dll_unlit_fp_e41670.sm30", Stage::Ps),
        ] {
            let path = format!("{FIXTURE_DIR}/{file}");
            let bytes = std::fs::read(&path)
                .unwrap_or_else(|e| panic!("fixture {file}: {e}"));
            assert!(bytes.len() > 4, "{file} truncated");
            let tok = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            assert_eq!(tok, want.version_token(), "{file} stage token");
        }
    }

    /// Each committed blob fixture's file-name offset picks exactly one
    /// library row whose name+stage give the fixture its program
    /// identity (the flagship pair: default_vp/default_fp = "default").
    #[test]
    fn fixtures_map_to_library_rows() {
        for (file, want_name, want_stage) in [
            ("dll_default_vp_e34e60.sm30", "default", Stage::Vs),
            ("dll_default_fp_e3b308.sm30", "default", Stage::Ps),
            ("dll_bink_fp_e3adf8.sm30", "bink", Stage::Ps),
            ("dll_error_vp_e36010.sm30", "error", Stage::Vs),
            ("dll_lightcount_vp_e37238.sm30", "lightcount", Stage::Vs),
            ("dll_pointsprite_fp_e41cf0.sm30", "pointsprite", Stage::Ps),
            ("dll_shadowblur_fp_e3f858.sm30", "shadowblur", Stage::Ps),
            ("dll_shadowdebug_fp_e405f0.sm30", "shadowdebug", Stage::Ps),
            ("dll_unlit_fp_e41670.sm30", "unlit", Stage::Ps),
        ] {
            // parse the hex offset out of the fixture file name
            let stem = file.strip_prefix("dll_").unwrap().strip_suffix(".sm30").unwrap();
            let hex = stem.rsplit('_').next().unwrap().trim_start_matches('e');
            let off = u32::from_str_radix(&format!("e{hex}"), 16).expect("hex offset");
            let rows: Vec<&ProgramRow> = PROGRAM_LIBRARY
                .iter()
                .filter(|r| r.blob_offset == off)
                .collect();
            assert_eq!(rows.len(), 1, "{file}: offset {off:08x} must hit one row");
            assert_eq!(rows[0].name, want_name, "{file}");
            assert_eq!(rows[0].stage, want_stage, "{file}");
        }
    }

    /// The flagship world pair pins end-to-end.
    #[test]
    fn flagship_default_pair() {
        let (vs, ps) = program_pair("default").expect("default registered");
        assert_eq!(vs.unwrap().blob_offset, 0x00e3_4e60);
        assert_eq!(ps.unwrap().blob_offset, 0x00e3_b308);
        assert_eq!(vs.unwrap().slot_va, Some(0x110b_b998));
        assert_eq!(ps.unwrap().slot_va, Some(0x110b_bc68));
    }

    /// Registry census: 30 entries, unique names AND unique object VAs,
    /// case-sensitive lookup, the corpus-observed families present.
    #[test]
    fn family_registry_census() {
        assert_eq!(FAMILY_REGISTRY.len(), 30);
        let mut names: Vec<&str> = FAMILY_REGISTRY.iter().map(|f| f.name).collect();
        names.sort_unstable();
        let n = names.len();
        names.dedup();
        assert_eq!(names.len(), n, "family names unique");
        let mut objs: Vec<u32> = FAMILY_REGISTRY.iter().map(|f| f.obj_va).collect();
        objs.sort_unstable();
        let o = objs.len();
        objs.dedup();
        assert_eq!(objs.len(), o, "family object VAs unique");
        for f in FAMILY_REGISTRY {
            assert!((0x1128_2c80..=0x1128_2cf8).contains(&f.obj_va), "{:x}", f.obj_va);
        }
        assert!(family_entry("env_car_tire").is_some());
        assert!(family_entry("zCBV2_env_road").is_some());
        assert!(family_entry("char_hair").is_some());
        assert!(family_entry("env_car_tires").is_none(), "exact match only");
        // the 139-family art corpus ALSO carries short codes (1c1s1n)
        // and long names outside this registry — those resolve through
        // a path that is still UNKNOWN (documented in the module docs).
        assert!(family_entry("1c1s1n").is_none());
    }

    /// Slot layout: all slots unique and inside the .data window the
    /// constructor writes; the first 19 VS rows sit in the low window
    /// (the 4 late VS pairs gbuffer/lightstencil/pointlight/spotlight
    /// interleave into the high window — observed, semantics unknown);
    /// the rows without a slot are exactly the 7 observed PS rows.
    #[test]
    fn slot_windows() {
        let mut slots: Vec<u32> = PROGRAM_LIBRARY.iter().filter_map(|r| r.slot_va).collect();
        assert_eq!(slots.len(), 38, "45 rows - 7 slotless PS rows");
        slots.sort_unstable();
        slots.dedup();
        assert_eq!(slots.len(), 38, "slots unique");
        for r in PROGRAM_LIBRARY {
            if let Some(s) = r.slot_va {
                assert!((0x110b_b984..=0x110b_c258).contains(&s), "{:x}", s);
            } else {
                assert_eq!(r.stage, Stage::Ps, "only PS rows lack slots");
            }
        }
        // low window: the first 19 rows are exactly the VS rows of the
        // first 19 programs, in constructor order.
        for (i, r) in PROGRAM_LIBRARY.iter().enumerate() {
            if i < 19 {
                assert_eq!(r.stage, Stage::Vs);
                assert!(r.slot_va.unwrap() < 0x110b_bc20, "{}", r.name);
            }
        }
        let no_slot: Vec<&str> = PROGRAM_LIBRARY
            .iter()
            .filter(|r| r.slot_va.is_none())
            .map(|r| r.name)
            .collect();
        assert_eq!(
            no_slot,
            ["error", "fastclear", "lightcount", "shadowdepthfloat", "zfill", "zfillnotex", "lightstencil"]
        );
    }

    // ------------------------------------------------------------------
    // Block 33: the family→feature-flag layer and the built-in classes.
    // ------------------------------------------------------------------

    /// Every registry family is classified exactly once across the
    /// feature table: the 23 listed families assert 1-2 keys, the 7
    /// remaining registry families assert none; each key's family
    /// group matches the disjunction extracted from `0x1079cb60`.
    #[test]
    fn family_feature_flags_partition_the_registry() {
        assert_eq!(FAMILY_REGISTRY.len(), 30);
        let listed: Vec<&str> = FAMILY_FEATURE_FLAGS.iter().map(|(f, _)| *f).collect();
        for fam in listed {
            assert!(
                family_entry(fam).is_some(),
                "feature row {fam} must be a registry family"
            );
        }
        let ungrouped: Vec<&str> = FAMILY_REGISTRY
            .iter()
            .map(|f| f.name)
            .filter(|n| family_feature_keys(n).is_empty())
            .collect();
        assert_eq!(
            ungrouped,
            ["char_hair", "env_sky_dynamic", "env_road", "zCBV2_env_road", "env_sidewalk", "zCBV2_env_sidewalk", "env_water"],
            "the 7 ungrouped families"
        );
        // key groups
        let key_group = |key: u32| -> Vec<&str> {
            FAMILY_REGISTRY
                .iter()
                .map(|f| f.name)
                .filter(|n| family_feature_keys(n).contains(&key))
                .collect()
        };
        assert_eq!(key_group(feature_keys::DISTORTION), ["pp_obj_distortion", "pfx_obj_distortion"]);
        assert_eq!(key_group(feature_keys::FOG_BUFFER), ["pp_fog_buffer"]);
        assert_eq!(key_group(feature_keys::DECAL), ["env_decal_bias_nm", "env_decal", "env_decal_nm"]);
        assert_eq!(key_group(feature_keys::LIGHT_DECAL), ["env_light_decal_spot", "env_midground_lights"]);
        assert_eq!(key_group(feature_keys::CBV_COLOR_ALPHA), ["fx_cbvcolor_alpha", "hud_icon"]);
        assert_eq!(
            key_group(feature_keys::VEHICLE),
            ["env_vehicle_colorvar", "env_vehicle_glass_fade", "env_car_tire"]
        );
        assert_eq!(key_group(feature_keys::HAZE), ["env_haze"]);
        assert_eq!(
            key_group(feature_keys::HIVE),
            ["env_hive_exterior", "env_hive_decal", "env_hive_interior"]
        );
        // the midground key is exactly the 7-family disjunction
        let mid = key_group(feature_keys::MIDGROUND);
        assert_eq!(mid.len(), MIDGROUND_FAMILIES.len());
        for f in MIDGROUND_FAMILIES {
            assert!(mid.contains(f), "{f} must assert MIDGROUND");
        }
        // env_midground_lights is the only family asserting two keys
        let multi: Vec<&str> = FAMILY_REGISTRY
            .iter()
            .map(|f| f.name)
            .filter(|n| family_feature_keys(n).len() > 1)
            .collect();
        assert_eq!(multi, ["env_midground_lights"]);
        // unknown / non-registry names yield no keys (all-false default)
        assert!(family_feature_keys("1c1s1n").is_empty());
        assert!(family_feature_keys("env_building_noglass").is_empty());
    }

    /// Nine classes, unique names, capacities match the factory
    /// immediates; the `default` class's full 12-row table is pinned
    /// (lit→default, zfill/zfillnotex with flag2=1, cheapLit0..7→error,
    /// clear→fastclear); bink/occlusionblur are single-pass.
    #[test]
    fn builtin_class_census_and_default_table() {
        assert_eq!(BUILTIN_CLASSES.len(), 9);
        let names: Vec<&str> = BUILTIN_CLASSES.iter().map(|c| c.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 9, "class names unique");
        assert_eq!(
            names,
            ["default", "error", "pointSprite", "unlit", "textureGutter", "occlusionblur", "legacy", "bink", "particleDebug"]
        );
        let d = builtin_class("default").expect("default");
        assert_eq!(d.pass_capacity, 0xc);
        assert_eq!(d.passes.len(), 12);
        assert_eq!(d.passes[0], ClassPassRow {
            pass: "lit", requested: "default", canonical: "default", flag1: 0, flag2: 0
        });
        assert_eq!(d.passes[1].canonical, "zfill");
        assert_eq!(d.passes[1].flag2, 1);
        assert_eq!(d.passes[2].canonical, "zfillnotex");
        for (i, row) in d.passes.iter().enumerate().skip(3).take(8) {
            assert_eq!(row.pass, format!("cheapLit{}", i - 3));
            assert_eq!(row.canonical, "error");
        }
        assert_eq!(d.passes[11].canonical, "fastclear");
        for single in ["occlusionblur", "bink"] {
            let c = builtin_class(single).unwrap();
            assert_eq!(c.passes.len(), 1);
            assert_eq!(c.pass_capacity, 1);
        }
        // legacy carries its own cheapLit set (11 rows)
        let legacy = builtin_class("legacy").unwrap();
        assert_eq!(legacy.passes.len(), 11);
        assert_eq!(legacy.pass_capacity, 0xb);
        // honest note: particleDebug's capacity (11) exceeds its rows (5)
        let pd = builtin_class("particleDebug").unwrap();
        assert_eq!(pd.passes.len(), 5);
        assert_eq!(pd.pass_capacity, 0xb);
        assert!(builtin_class("zfill").is_none(), "zfill is a program, not a class");
        assert!(builtin_class("fastclear").is_none(), "fastclear is a program, not a class");
        assert!(builtin_class("depthRestore").is_none(), "depthRestore is p3d-loader-side (0x106205e0), not built-in");
    }

    /// Every canonical program name of every class pass row either
    /// resolves in the 45-row program library (case-folded) or is one
    /// of the two NON-blob programs (`texturegutter`, `occlusionblur`
    /// — registered nowhere in the DLL's 45-blob constructor; the
    /// engine must bind them from p3d-shipped shader data, the same
    /// channel the M10 frontend shaders arrive through).  zfill-family
    /// rows are exactly the flag2=1 rows.
    #[test]
    fn class_pass_canonicals_resolve_in_program_library() {
        const NON_BLOB: [&str; 2] = ["texturegutter", "occlusionblur"];
        let mut non_blob_seen: Vec<&str> = Vec::new();
        for c in BUILTIN_CLASSES {
            for row in c.passes {
                match program_pair(row.canonical) {
                    Some(pair) => assert!(pair.0.is_some() || pair.1.is_some()),
                    None => {
                        assert!(
                            NON_BLOB.contains(&row.canonical),
                            "{}.pass {} -> {} resolves nowhere",
                            c.name,
                            row.pass,
                            row.canonical
                        );
                        if !non_blob_seen.contains(&row.canonical) {
                            non_blob_seen.push(row.canonical);
                        }
                    }
                }
                // the canonical literal folds onto the registered
                // (lowercase) library name — and requested == canonical
                // on every extracted row
                assert_eq!(row.requested.to_ascii_lowercase(), row.canonical.to_ascii_lowercase());
                if row.canonical == "zfill" || row.canonical == "zfillnotex" {
                    assert_eq!(row.flag2, 1, "{}/{row:?} zfill-family rows carry flag2=1", c.name);
                } else {
                    assert_eq!(row.flag2, 0);
                }
                assert_eq!(row.flag1, 0);
            }
        }
        // the flagship wiring: the default class's lit pass IS the
        // committed (default_vp, default_fp) pair
        let d = builtin_class("default").unwrap();
        let lit = d.passes.iter().find(|r| r.pass == "lit").unwrap();
        let (vs, ps) = program_pair(lit.canonical).unwrap();
        assert_eq!(vs.unwrap().blob_offset, 0x00e3_4e60);
        assert_eq!(ps.unwrap().blob_offset, 0x00e3_b308);
        // both non-blob programs really occur (textureGutter's own pass
        // and occlusionblur's single pass)
        non_blob_seen.sort_unstable();
        assert_eq!(non_blob_seen, ["occlusionblur", "texturegutter"]);
    }

    /// The art-observed family universe vs the 30-entry registry: the
    /// 5 vehicle/decal families hit the registry; the other 13
    /// observed families (and the 4 short codes) fall outside it.
    #[test]
    fn art_observed_families_cross_reference() {
        assert_eq!(ART_OBSERVED_FAMILIES.len(), 18);
        let in_registry: Vec<&str> = ART_OBSERVED_FAMILIES
            .iter()
            .filter(|f| family_entry(f).is_some())
            .copied()
            .collect();
        assert_eq!(
            in_registry,
            ["env_car_tire", "env_decal", "env_decal_nm", "env_vehicle_colorvar", "env_vehicle_glass_fade"]
        );
        // short codes exist as art family strings but not in the registry
        for code in ["1c1a", "1c1n", "1c1n1a", "1cv"] {
            assert!(ART_OBSERVED_FAMILIES.contains(&code));
            assert!(family_entry(code).is_none());
        }
    }
}

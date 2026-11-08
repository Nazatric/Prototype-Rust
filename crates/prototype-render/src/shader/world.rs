//! world — the DLL `default` shader pair as THE real world pipeline
//! (NEXT-UP item 7(e), block 34 — the device-free wiring half).
//!
//! Block 33 closed item 7(d): shader families are feature flags and the
//! world path draws through the `default` class's `lit` pass — the
//! committed `(default_vp, default_fp)` program pair.  This module wires
//! that pair into a pipeline plan WITHOUT a GPU:
//!
//! * [`WorldPairPlan::from_blobs`] / [`WorldPairPlan::from_dll`] —
//!   parse, translate, plan (captured inside the WGSL emitter — see
//!   `shader::wgsl::translate_program_with_plan`), renumber the
//!   fragment stage to bind group 1, and cross-validate both ways.
//! * CTAB name maps — the blobs' compiler constant tables carry the
//!   EXACT engine uniform names per register (`p3dWorldViewProjectionMatrix`
//!   c0..c3, `p3dWorldMatrix` c4..c7, `p3dLight_*`, `p3dShadowMap*`,
//!   material rows) and the exact BOOL names (`b0 = p3dLight_IsLitPass`,
//!   `b1 = p3dShadowEnabled`, `b2 = p3dShadowIsLinearDepth`) and the
//!   exact SAMPLER names (`s4 = diffuseMap` — NOT s0!).
//! * [`world_texture_assignment`] — the fragment material slots BY
//!   CTAB NAME: `diffuseMap` gets the diffuse upload, `normalMap` /
//!   `specularMap` the extras; the four engine light/shadow textures
//!   stay on the shared white fallback (their real feeds are engine
//!   render targets this loader does not render yet — block 42 typed
//!   the feeds themselves in [`super::texture_feeds`]: the roster is
//!   PROVEN twice (CTAB × the engine's .rdata pass table), all four
//!   names are type-6 standard-constant rows, and the shadow feed is
//!   the engine's child-cascade render target toggled by wrapper
//!   dirty bits 0x1/0x8 around the shadow draw).
//! * bool policy — the three booleans are per-draw ENGINE state, and
//!   the CTAB names give their meaning: the default draw is the LIT
//!   pass with shadows off (`b0 = true`, `b1 = b2 = false`).
//!   Note the relationship to block 33's family feature flags (map
//!   keys 5..14): those live in the shader INSTANCE's feature map, NOT
//!   in the D3D9 bool register file — conflating them would be
//!   invention, so this module does not.
//!
//! # Evidence (all from the committed fixtures, block 34 re-dump)
//!
//! * PROVEN (CTAB, `dll_default_vp_e34e60.sm30`): c0..c3
//!   `p3dWorldViewProjectionMatrix`, c4..c7 `p3dWorldMatrix`, c8..c11
//!   `p3dLight_ProjectedTextureMatrix`, c12..c15 `p3dShadowMapMatrix`,
//!   c16 `p3dEyePositionObjectSpace`, c17 `p3dLight_PositionObjectSpace`;
//!   inputs position0/normal0/texcoord0/texcoord6/texcoord7; 18 float
//!   rows read (c0..c17), no bools, no samplers.
//! * PROVEN (CTAB, `dll_default_fp_e3b308.sm30`): p3dLight_Direction c0,
//!   Attenuation c1, Ambient c2, Colour c3, LightType c4,
//!   SpotlightConstant c5, DecayConstant c6, materialShininess c7,
//!   materialDiffuse c8, materialSpecular c9, p3dShadowEnabledChildren
//!   c10, p3dShadowMapInfo c11, p3dShadowMapChildBox00/01/03 c12..c14,
//!   p3dShadowMapChildInfo00..04 c15..c19; bools b0 `p3dLight_IsLitPass`,
//!   b1 `p3dShadowEnabled`, b2 `p3dShadowIsLinearDepth`; samplers s0
//!   `p3dLight_DecayTexture`, s1 `p3dLight_SpotlightTexture`, s2
//!   `p3dLight_ProjectedTexture`, s3 `p3dShadowMap`, s4 `diffuseMap`, s5
//!   `normalMap`, s6 `specularMap`; def'd c20/c21 (baked, no bindings).
//! * PROVEN (block 33): blob offsets inside `prototypeenginef.dll` —
//!   `default` VS at file offset 0x00e34e60, PS at 0x00e3b308
//!   ([`super::library::PROGRAM_LIBRARY`]), first DWORD of a blob = the
//!   stage version token.
//! * LIKELY: `b0 = true` default (the boot path draws the lit pass —
//!   the class pass table's `lit` row is the world path, block 33);
//!   `p3dWorldMatrix` = the model transform (name + the engine's
//!   decomposed matrices convention; our paired-drawable path writes
//!   the placement there).

use std::collections::BTreeMap;

use super::library::PROGRAM_LIBRARY;
use super::plan::{renumber_group, validate_plan_matches_wgsl, StageResources, StageValues};
use super::sm30::Sm30Shader;
use super::wgsl::translate_program_with_plan;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// World-pipeline planning errors.  Every variant is fail-closed: the
/// boot path logs it and the frontend pipelines keep rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorldError {
    /// The DLL slice did not decode as SM 3.0 bytecode.
    Parse(String),
    /// The WGSL translation failed (an untranslatable instruction).
    Translate(String),
    /// The captured plan disagreed with the emitted WGSL.
    Plan(String),
    /// A blob's version token did not match its registered stage.
    StageToken { expected: u32, got: u32 },
    /// The blob is not fully inside the DLL image.
    BlobRange { offset: u32, len: usize, dll_len: usize },
    /// A CTAB name lookup failed (unknown uniform/sampler/bool name).
    UnknownName(String),
}

impl std::fmt::Display for WorldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorldError::Parse(e) => write!(f, "world pair parse: {e}"),
            WorldError::Translate(e) => write!(f, "world pair translate: {e}"),
            WorldError::Plan(e) => write!(f, "world pair plan: {e}"),
            WorldError::StageToken { expected, got } => write!(
                f,
                "world pair stage token mismatch: expected {expected:#010x}, got {got:#010x}"
            ),
            WorldError::BlobRange { offset, len, dll_len } => write!(
                f,
                "world pair blob range: offset {offset:#x} len {len} outside DLL of {dll_len} B"
            ),
            WorldError::UnknownName(n) => write!(f, "world pair: unknown CTAB name {n:?}"),
        }
    }
}

impl std::error::Error for WorldError {}

// ---------------------------------------------------------------------------
// CTAB name maps
// ---------------------------------------------------------------------------

/// The CTAB-derived name maps of one stage (float registers, and for
/// fragment stages also the bool + sampler names).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CtabNames {
    /// float c# rows: register → engine name (matrices map to their
    /// FIRST register row; the row span is in [`CtabNames::spans`]).
    pub floats: BTreeMap<u16, String>,
    /// bool b# rows: register → engine name.
    pub bools: BTreeMap<u16, String>,
    /// sampler s# registers: register → engine name.
    pub samplers: BTreeMap<u16, String>,
    /// register_count per float name (4 for a matrix, 1 otherwise).
    pub spans: BTreeMap<u16, u16>,
}

impl CtabNames {
    /// Build from a stage's CTAB constants (set 2 = float, 0 = bool,
    /// 3 = sampler — the observed D3DX register-set encoding, proven in
    /// `sm30`'s CTAB analysis).
    pub fn from_ctab(ctab: Option<&super::sm30::Ctab>) -> Self {
        let mut out = Self::default();
        let Some(ctab) = ctab else { return out };
        for c in &ctab.constants {
            match c.register_set {
                2 => {
                    out.floats.entry(c.register_index).or_insert_with(|| c.name.clone());
                    out.spans.insert(c.register_index, c.register_count);
                }
                0 => {
                    out.bools.entry(c.register_index).or_insert_with(|| c.name.clone());
                }
                3 => {
                    out.samplers.entry(c.register_index).or_insert_with(|| c.name.clone());
                }
                _ => {}
            }
        }
        out
    }

    /// The first register of a float uniform by engine name.
    pub fn float_register(&self, name: &str) -> Option<u16> {
        self.floats
            .iter()
            .find(|(_, n)| n.as_str() == name)
            .map(|(r, _)| *r)
    }

    /// The register of a bool by engine name.
    pub fn bool_register(&self, name: &str) -> Option<u16> {
        self.bools
            .iter()
            .find(|(_, n)| n.as_str() == name)
            .map(|(r, _)| *r)
    }

    /// The register of a sampler by engine name.
    pub fn sampler_register(&self, name: &str) -> Option<u16> {
        self.samplers
            .iter()
            .find(|(_, n)| n.as_str() == name)
            .map(|(r, _)| *r)
    }
}

// ---------------------------------------------------------------------------
// World pair plan
// ---------------------------------------------------------------------------

/// The complete device-free plan of the DLL `default` world pair.
#[derive(Debug, Clone)]
pub struct WorldPairPlan {
    /// Decoded vertex stage (`vs_3_0`).
    pub vs: Sm30Shader,
    /// Decoded fragment stage (`ps_3_0`).
    pub fs: Sm30Shader,
    /// Final vertex WGSL (resources in bind group 0).
    pub vs_wgsl: String,
    /// Final fragment WGSL (resources renumbered to bind group 1).
    pub fs_wgsl: String,
    /// Vertex-stage binding plan (validated against `vs_wgsl`).
    pub vertex_resources: StageResources,
    /// Fragment-stage binding plan (validated against `fs_wgsl`).
    pub fragment_resources: StageResources,
    /// Vertex CTAB name maps.
    pub vertex_names: CtabNames,
    /// Fragment CTAB name maps.
    pub fragment_names: CtabNames,
}

impl WorldPairPlan {
    /// Plan the world pair from the two raw SM 3.0 blobs (VS first).
    pub fn from_blobs(vs_blob: &[u8], fs_blob: &[u8]) -> Result<Self, WorldError> {
        let vs = Sm30Shader::parse_program(vs_blob, false)
            .map_err(|e| WorldError::Parse(format!("vs: {e}")))?;
        let fs = Sm30Shader::parse_program(fs_blob, true)
            .map_err(|e| WorldError::Parse(format!("fs: {e}")))?;
        Self::build(vs, fs)
    }

    /// Plan the world pair from a loaded `prototypeenginef.dll` image:
    /// the `default` rows of [`PROGRAM_LIBRARY`] give the blob offsets
    /// (VS `0x00e34e60`, PS `0x00e3b308`), each slice is bounds-checked
    /// and its first DWORD checked against the stage's version token.
    pub fn from_dll(dll: &[u8]) -> Result<Self, WorldError> {
        let vs_row = PROGRAM_LIBRARY
            .iter()
            .find(|r| r.name == "default" && r.stage == crate::shader::library::Stage::Vs)
            .expect("PROGRAM_LIBRARY pins a default VS row");
        let fs_row = PROGRAM_LIBRARY
            .iter()
            .find(|r| r.name == "default" && r.stage == crate::shader::library::Stage::Ps)
            .expect("PROGRAM_LIBRARY pins a default PS row");
        let vs_blob = slice_blob(dll, vs_row.blob_offset)?;
        let fs_blob = slice_blob(dll, fs_row.blob_offset)?;
        check_stage_token(vs_blob, super::library::Stage::Vs.version_token())?;
        check_stage_token(fs_blob, super::library::Stage::Ps.version_token())?;
        Self::from_blobs(vs_blob, fs_blob)
    }

    fn build(vs: Sm30Shader, fs: Sm30Shader) -> Result<Self, WorldError> {
        let (vs_wgsl, vertex_resources) =
            translate_program_with_plan(&vs, "default_vp").map_err(|e| WorldError::Translate(format!("vs: {e}")))?;
        let (fs_wgsl0, fragment_resources) =
            translate_program_with_plan(&fs, "default_fp").map_err(|e| WorldError::Translate(format!("fs: {e}")))?;
        // The fragment stage moves to bind group 1 BEFORE validation —
        // the validated text is exactly what the GPU will compile.
        let fs_wgsl = renumber_group(&fs_wgsl0, 1);
        validate_plan_matches_wgsl(&vertex_resources, &vs_wgsl)
            .map_err(|e| WorldError::Plan(format!("vs: {e}")))?;
        validate_plan_matches_wgsl(&fragment_resources, &fs_wgsl)
            .map_err(|e| WorldError::Plan(format!("fs: {e}")))?;
        let vertex_names = CtabNames::from_ctab(vs.ctab.as_ref());
        let fragment_names = CtabNames::from_ctab(fs.ctab.as_ref());
        Ok(Self {
            vs,
            fs,
            vs_wgsl,
            fs_wgsl,
            vertex_resources,
            fragment_resources,
            vertex_names,
            fragment_names,
        })
    }

    /// The WVP matrix's first c# register in the VERTEX stage —
    /// `p3dWorldViewProjectionMatrix` (PROVEN: c0).  The per-frame
    /// writer puts the four camera rows at this register base.
    pub fn wvp_register(&self) -> Result<u16, WorldError> {
        self.vertex_names
            .float_register("p3dWorldViewProjectionMatrix")
            .ok_or_else(|| WorldError::UnknownName("p3dWorldViewProjectionMatrix".into()))
    }

    /// The world (model) matrix's first c# register in the VERTEX
    /// stage — `p3dWorldMatrix` (PROVEN: c4).  The paired-drawable
    /// path writes the placement transform here instead of composing
    /// it into the WVP (the engine's decomposed-matrices convention).
    pub fn world_matrix_register(&self) -> Result<u16, WorldError> {
        self.vertex_names
            .float_register("p3dWorldMatrix")
            .ok_or_else(|| WorldError::UnknownName("p3dWorldMatrix".into()))
    }
}

/// Slice one blob out of the DLL image, ending at its END token.
///
/// The blobs have no length field; the proven extent rule is the m33
/// extraction walk (`scripts/m33_dll_blobs.py`, which produced the
/// committed fixtures): from the blob offset, step DWORDs —
///
/// * first DWORD — the version token (validated by
///   [`check_stage_token`]; the walk starts after it);
/// * `0x0000FFFF` — the END token: the blob ends AFTER it (PROVEN in
///   `sm30`'s own `END_TOKEN` and in the m33 script);
/// * low word `0xFFFE` (comment token; count = high word): skip
///   4 + `count × 4` bytes;
/// * bit 31 clear (instruction): length nibble = operand DWORDs after
///   the token, so advance `4 + ln × 4` (zero for else/endif/ret);
/// * bit 31 set in instruction position — a param token where an
///   instruction belongs: fail closed.
///
/// (The blobs' post-END bytes — the one zero-DWORD separator before
/// the next blob — are NOT part of the blob; `parse_program` rejects
/// trailing bytes, as the committed fixtures prove.)
fn slice_blob<'a>(dll: &'a [u8], offset: u32) -> Result<&'a [u8], WorldError> {
    const END: u32 = 0x0000_FFFF;
    let start = offset as usize;
    if start >= dll.len() || start % 4 != 0 {
        return Err(WorldError::BlobRange { offset, len: 0, dll_len: dll.len() });
    }
    let mut cur = start + 4; // skip the version token (checked by the caller)
    let dword = |i: usize| -> u32 {
        u32::from_le_bytes([dll[i], dll[i + 1], dll[i + 2], dll[i + 3]])
    };
    while cur + 4 <= dll.len() {
        let t = dword(cur);
        if t == END {
            return Ok(&dll[start..cur + 4]);
        }
        if t & 0x8000_0000 == 0 && (t & 0xFFFF) == 0xFFFE {
            // Comment token: low word 0xFFFE, HIGH16 = body dword count.
            let cnt = ((t >> 16) & 0xFFFF) as usize;
            cur += 4 + cnt * 4;
            continue;
        }
        if t & 0x8000_0000 == 0 {
            let ln = ((t >> 24) & 0xF) as usize;
            cur += 4 + ln * 4;
            continue;
        }
        return Err(WorldError::Parse(format!(
            "param token {t:#010x} in instruction position at {cur:#x} (blob {offset:#x})"
        )));
    }
    Err(WorldError::Parse(format!(
        "no END token before end of image (blob {offset:#x})"
    )))
}

/// Check a blob's first DWORD against the expected stage token.
fn check_stage_token(blob: &[u8], expected: u32) -> Result<(), WorldError> {
    let got = blob
        .first_chunk::<4>()
        .map(|b| u32::from_le_bytes(*b))
        .unwrap_or(0);
    if got == expected {
        Ok(())
    } else {
        Err(WorldError::StageToken { expected, got })
    }
}

// ---------------------------------------------------------------------------
// Per-draw values
// ---------------------------------------------------------------------------

/// The default per-draw uniform values of the world pair: the vertex
/// stage gets an IDENTITY `p3dWorldMatrix` (and zeros elsewhere), the
/// fragment stage gets white `materialDiffuse`, zero `materialSpecular`
/// / `materialShininess`, zero light rows, and the bool policy
/// `p3dLight_IsLitPass = true` / `p3dShadowEnabled = false` /
/// `p3dShadowIsLinearDepth = false` (the lit-pass draw with shadows
/// off — see the module docs for the LIKELY label).
///
/// The camera rows (`p3dWorldViewProjectionMatrix`) are NOT defaulted —
/// a zero WVP would collapse every draw to a point, so the caller MUST
/// set them (via [`WorldPairPlan::wvp_register`]); the encoding keeps
/// them zero until then, which is at least honest.
pub fn default_draw_values(plan: &WorldPairPlan) -> (StageValues, StageValues) {
    let mut vs = StageValues::default();
    if let Some(r) = plan.vertex_names.float_register("p3dWorldMatrix") {
        vs.set_float(r, [1.0, 0.0, 0.0, 0.0]);
        vs.set_float(r + 1, [0.0, 1.0, 0.0, 0.0]);
        vs.set_float(r + 2, [0.0, 0.0, 1.0, 0.0]);
        vs.set_float(r + 3, [0.0, 0.0, 0.0, 1.0]);
    }
    let mut fs = StageValues::default();
    if let Some(r) = plan.fragment_names.float_register("materialDiffuse") {
        fs.set_float(r, [1.0, 1.0, 1.0, 1.0]);
    }
    if let Some(r) = plan.fragment_names.bool_register("p3dLight_IsLitPass") {
        fs.set_bool(r, true);
    }
    (vs, fs)
}

/// Set the four rows of a CTAB-named float matrix in a stage's values.
pub fn set_matrix_by_name(
    values: &mut StageValues,
    names: &CtabNames,
    name: &str,
    rows: &[[f32; 4]; 4],
) -> Result<(), WorldError> {
    let base = names
        .float_register(name)
        .ok_or_else(|| WorldError::UnknownName(name.to_string()))?;
    for (i, row) in rows.iter().enumerate() {
        values.set_float(base + i as u16, *row);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Daylight draw state (block 35) — DESIGN defaults grounded line-by-line
// in the default_fp disasm, chosen so the FIRST real execution shows a
// lit world instead of the black/NaN output the all-zero rows produce.
// ---------------------------------------------------------------------------

/// The normalized DESIGN sun direction (high-noon-ish, +z up-ish).
/// WORLD-UP CONVENTION UNKNOWN (z-up vs y-up) — a diagonal direction
/// lights faces under either convention.  Values are (0.35, 0.35, 0.87)
/// normalized (the test pins unit length; `sqrt` is not const).
pub const SUN_DIRECTION: [f32; 3] = [0.349_668_2, 0.349_668_2, 0.869_172_4];

/// The fragment rows that make the lit pass (`b0 = true`) VISIBLE.
/// Evidence per row (default_fp disasm, block-35 dump):
///
/// * `p3dLight_DecayConstant.w` (c6.w) = **1e6 — THE MASTER LIT GATE
///   (PROVEN structure)**: the tail `cmp r0.xyz, -c6.w, r1.y, r1.xzw`
///   outputs the constant `c21.y = 0` (BLACK) whenever `-c6.w >= 0`,
///   i.e. whenever c6.w <= 0; and the head `mad r1, r1.x, -r1.w, c6.w`
///   + `texkill r1` discards every pixel whose to-light length exceeds
///   c6.w.  c6.w = 1e6 keeps both open for world-scale positions (with
///   the far light position below, |to-light| ~ 1e5 < 1e6).
/// * `p3dLight_Direction` (c0.xyz) = **−SUN_DIRECTION**: the head runs
///   `dp3 r1.w, -c0, normalize(v5)` — v5 is the TO-LIGHT vector
///   (PROVEN: the VS builds texcoord5 = M·(c17 − pos)), so −c0 = the
///   sun direction makes the cone test positive for sun-facing
///   geometry.  The following `texkill` clips the rest — with the far
///   light below, to-light ≈ up for the whole map, so nothing clips.
/// * `p3dLight_Attenuation.xyz` (c1.xyz) = **+SUN_DIRECTION**: the tail
///   `dp3 r2.x, c1, r2.yzw` dots c1 with the perturbed NORMAL — the
///   directional N·L term.  (CTAB name notwithstanding — the shader's
///   math is what the register must carry.)
/// * `p3dLight_Ambient` (c2) = **(0.35, 0.36, 0.40, 1.0)**: the mid
///   `mul r3.yzw, r0.xyz, c2.xyz` builds the ambient floor
///   diffuse × ambient × materialDiffuse — zero rows would zero the
///   ambient half of the sum.
/// * `p3dLight_Colour` (c3) = **(1.0, 0.96, 0.88, 1.0)** (warm sun):
///   multiplies both the specular and the projected-texture lit term.
/// * `p3dLight_PositionObjectSpace` (c17, VERTEX stage) = **(0, 0, 1e5,
///   0)**: the far sun position the VS subtracts per vertex to build
///   the to-light vector (identity world ⇒ object space = world space).
///   DESIGN — the engine's real per-light fill model is UNKNOWN.
///
/// # Block 40 update — the fill-side RE result
///
/// The engine's constant METADATA layer is now PROVEN
/// ([`super::light_constants`]): every `p3dLight_*` name exists in the
/// DLL pool, the engine types are PROVEN (`Direction`/`Attenuation`/
/// `PositionObjectSpace` = VEC3 — the `.w` on those rows is filler;
/// `DecayConstant` is the vec4-class row whose `.w` is the master
/// gate — consistent with this DESIGN), the metaclass registry
/// exposes the light state under the EXACT constant names (1:1), and
/// the multi-light pool caps concurrency at FOUR indexed lights per
/// pass.  The live per-light VALUE FORMULAS are NOT in this
/// `prototypeenginef.dll` build: the PC-D3D9 fill is dead code there
/// (dead `Direct3DCreate9` import, unreachable by-name resolver block
/// 0x106f4850..0x106f8f00 — evidence in [`super::light_constants`]).
/// The rows below therefore stay the shader-side DESIGN, now anchored
/// by the proven metadata (types + names + 1:1 identity).
///
/// Everything else stays ZERO: c4 (LightType) = 0 selects the
/// directional branches and skips the spotlight texture (PROVEN select
/// structure), c5/c6.xyz make the attenuation/decay texture lookups
/// hit their white-fallback coordinates (PROVEN mad/add chain), c7/c9
/// zero the specular (pow(x, 0) = 1 × materialSpecular 0), and the
/// shadow bool b1 stays false (the shadow-map path is skipped).
pub fn daylight_state(plan: &WorldPairPlan) -> Result<StageValues, WorldError> {
    let mut fs = StageValues::default();
    let sun = SUN_DIRECTION;
    if let Some(r) = plan.fragment_names.float_register("p3dLight_Direction") {
        fs.set_float(r, [-sun[0], -sun[1], -sun[2], 0.0]);
    }
    if let Some(r) = plan.fragment_names.float_register("p3dLight_Attenuation") {
        fs.set_float(r, [sun[0], sun[1], sun[2], 0.0]);
    }
    if let Some(r) = plan.fragment_names.float_register("p3dLight_Ambient") {
        fs.set_float(r, [0.35, 0.36, 0.40, 1.0]);
    }
    if let Some(r) = plan.fragment_names.float_register("p3dLight_Colour") {
        fs.set_float(r, [1.0, 0.96, 0.88, 1.0]);
    }
    if let Some(r) = plan.fragment_names.float_register("p3dLight_DecayConstant") {
        fs.set_float(r + 0, [0.0, 0.0, 0.0, 1.0e6]);
    }
    Ok(fs)
}

/// The VERTEX-stage far sun position row (`p3dLight_PositionObjectSpace`,
/// c17): the DESIGN light position the to-light vector points at.
pub fn daylight_light_position(plan: &WorldPairPlan) -> Result<Option<(u16, [f32; 4])>, WorldError> {
    match plan.vertex_names.float_register("p3dLight_PositionObjectSpace") {
        Some(r) => Ok(Some((r, [0.0, 0.0, 1.0e5, 0.0]))),
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// Material slots (fragment textures) — by CTAB NAME
// ---------------------------------------------------------------------------

/// The upload index feeding each texture-binding register of the world
/// fragment stage, resolved BY CTAB NAME (block 34):
///
/// * `diffuseMap` (PROVEN s4) ← upload 0 (the mesh's diffuse);
/// * `normalMap` (s5) ← upload 1, `specularMap` (s6) ← upload 2 (the
///   extras in param order) when the uploads exist;
/// * the four engine light/shadow textures (`p3dLight_DecayTexture`,
///   `p3dLight_SpotlightTexture`, `p3dLight_ProjectedTexture`,
///   `p3dShadowMap`) ← `None` — the shared white fallback (their real
///   feeds are engine render targets this loader does not render yet).
///
/// This REPLACES the ascending-s# slot policy of the frontend path
/// (`texture_slot_registers` + `assign_texture_uploads`): on the world
/// pair the lowest s# is a LIGHT texture, so slot 0 would have eaten
/// the diffuse.  Returns one entry per texture binding in
/// ascending-register order (== plan order).
pub fn world_texture_assignment(
    plan: &WorldPairPlan,
    upload_count: usize,
) -> Vec<Option<usize>> {
    // name → feed (block 42: the PROVEN lit-pass roster in
    // [`super::texture_feeds`]); material feeds map to their upload
    // slot, engine-managed feeds (s0..s3) stay white until the engine
    // render-target chain lands.
    plan.fragment_resources
        .bindings
        .iter()
        .filter_map(|b| match b {
            super::plan::StageBinding::Texture { register, .. } => Some(*register),
            _ => None,
        })
        .map(|reg| {
            let engine_name = plan.fragment_names.samplers.get(&reg).map(|s| s.as_str()).unwrap_or("");
            let feed = super::texture_feeds::lit_feed_of(engine_name);
            // An unclassified sampler is not a material slot — white.
            // Name matched: an engine-managed texture or an upload
            // past the available count both fall back to white.
            feed.and_then(|f| f.upload_slot())
                .filter(|k| *k < upload_count)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests — device-free, against the committed real blobs
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shader::plan::StageBinding;
    use crate::shader::sm30::{CtabConstant, RegType, Sm30Op};
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

    fn real_plan() -> WorldPairPlan {
        WorldPairPlan::from_blobs(
            &fixture("dll_default_vp_e34e60.sm30"),
            &fixture("dll_default_fp_e3b308.sm30"),
        )
        .expect("the committed default pair must plan")
    }

    /// Vertex stage census (block-34 dump): 18 float rows (c0..c17),
    /// no bools, no samplers; bindings dense from 0.
    #[test]
    fn vertex_plan_census_matches_the_real_blob() {
        let p = real_plan();
        assert_eq!(p.vertex_resources.uniform_count(), 18);
        assert_eq!(p.vertex_resources.bool_count(), 0);
        assert_eq!(p.vertex_resources.sampler_count(), 0);
        let regs: Vec<u16> = p
            .vertex_resources
            .uniforms()
            .filter_map(|b| match b {
                StageBinding::Uniform { register, .. } => Some(*register),
                _ => None,
            })
            .collect();
        assert_eq!(regs, (0..18).collect::<Vec<_>>(), "c0..c17 in register order");
        for (i, b) in p.vertex_resources.bindings.iter().enumerate() {
            assert_eq!(b.binding(), i as u32, "bindings dense from 0");
        }
        // WGSL names: the blobs carry no parameter records → u_c{n}.
        assert!(p.vs_wgsl.contains("var<uniform> u_c0: vec4<f32>; // c0"));
        assert!(p.vs_wgsl.contains("var<uniform> u_c17: vec4<f32>; // c17"));
        assert!(!p.vs_wgsl.contains("var<uniform> b0"), "vs has no bools");
        // Vertex inputs: position (dedicated location 32 — block 34's
        // collision fix), normal, texcoords 0/6/7.
        assert!(p.vs_wgsl.contains("@location(32) position0: vec4<f32>"), "{}", p.vs_wgsl);
        assert!(p.vs_wgsl.contains("@location(19) normal0: vec4<f32>"));
        assert!(p.vs_wgsl.contains("@location(0) texcoord0: vec4<f32>"));
    }

    /// Fragment stage census (block-34 dump): 20 float rows (c0..c19),
    /// 3 bool rows (b0..b2), 7 sampler pairs (s0..s6); def'd c20/c21
    /// are baked and MUST NOT appear; bools bind between floats and
    /// samplers; the fragment stage is group 1.
    #[test]
    fn fragment_plan_census_matches_the_real_blob() {
        let p = real_plan();
        assert_eq!(p.fragment_resources.uniform_count(), 20);
        assert_eq!(p.fragment_resources.bool_count(), 3);
        assert_eq!(p.fragment_resources.sampler_count(), 7);
        // def'd registers baked, not bound.
        assert!(!p.fs_wgsl.contains("u_c20"), "def'd c20 must be baked");
        assert!(!p.fs_wgsl.contains("u_c21"), "def'd c21 must be baked");
        // Binding layout: floats 0..19, bools 20..22, texture/sampler
        // pairs 23..36 — every stage-local number, in group 1.
        for b in p.fragment_resources.bindings.iter() {
            assert!(p.fs_wgsl.contains(&format!("@group(1) @binding({})", b.binding())));
        }
        assert!(p.fs_wgsl.contains("@group(1) @binding(20) var<uniform> b0: vec4<f32>; // boolean constant b0"));
        assert!(p.fs_wgsl.contains("@group(1) @binding(22) var<uniform> b2: vec4<f32>; // boolean constant b2"));
        assert!(p.fs_wgsl.contains("@group(1) @binding(23) var s0_tex: texture_2d<f32>; // s0"));
        assert!(p.fs_wgsl.contains("@group(1) @binding(35) var s6_tex: texture_2d<f32>; // s6"));
        assert!(!p.fs_wgsl.contains("@group(0)"), "fragment stage fully renumbered");
        // The branch structure survived translation (ifs on b0..b2; the
        // condition text is re-inlined in every flattened select, so
        // presence — not a count — is the contract).
        assert!(p.fs_wgsl.contains("b0.x != 0.0"));
        assert!(p.fs_wgsl.contains("b1.x != 0.0"));
        assert!(p.fs_wgsl.contains("b2.x != 0.0"));
    }

    /// The CTAB name maps carry the EXACT engine names (block-34 dump).
    #[test]
    fn ctab_names_are_the_engine_uniform_names() {
        let p = real_plan();
        // Vertex matrices.
        assert_eq!(p.vertex_names.float_register("p3dWorldViewProjectionMatrix"), Some(0));
        assert_eq!(p.vertex_names.float_register("p3dWorldMatrix"), Some(4));
        assert_eq!(p.vertex_names.float_register("p3dLight_ProjectedTextureMatrix"), Some(8));
        assert_eq!(p.vertex_names.float_register("p3dShadowMapMatrix"), Some(12));
        assert_eq!(p.vertex_names.float_register("p3dEyePositionObjectSpace"), Some(16));
        assert_eq!(p.vertex_names.float_register("p3dLight_PositionObjectSpace"), Some(17));
        // Matrix spans.
        assert_eq!(p.vertex_names.spans.get(&0), Some(&4));
        assert_eq!(p.vertex_names.spans.get(&16), Some(&1));
        // Fragment floats.
        assert_eq!(p.fragment_names.float_register("p3dLight_Direction"), Some(0));
        assert_eq!(p.fragment_names.float_register("p3dLight_Ambient"), Some(2));
        assert_eq!(p.fragment_names.float_register("p3dLight_Colour"), Some(3));
        assert_eq!(p.fragment_names.float_register("materialDiffuse"), Some(8));
        assert_eq!(p.fragment_names.float_register("materialSpecular"), Some(9));
        assert_eq!(p.fragment_names.float_register("p3dShadowMapChildInfo04"), Some(19));
        // Fragment BOOLS — the per-draw feature semantics.
        assert_eq!(p.fragment_names.bool_register("p3dLight_IsLitPass"), Some(0));
        assert_eq!(p.fragment_names.bool_register("p3dShadowEnabled"), Some(1));
        assert_eq!(p.fragment_names.bool_register("p3dShadowIsLinearDepth"), Some(2));
        // Fragment SAMPLERS — s4 is the DIFFUSE, not s0.
        assert_eq!(p.fragment_names.sampler_register("p3dLight_DecayTexture"), Some(0));
        assert_eq!(p.fragment_names.sampler_register("p3dLight_SpotlightTexture"), Some(1));
        assert_eq!(p.fragment_names.sampler_register("p3dLight_ProjectedTexture"), Some(2));
        assert_eq!(p.fragment_names.sampler_register("p3dShadowMap"), Some(3));
        assert_eq!(p.fragment_names.sampler_register("diffuseMap"), Some(4));
        assert_eq!(p.fragment_names.sampler_register("normalMap"), Some(5));
        assert_eq!(p.fragment_names.sampler_register("specularMap"), Some(6));
        // Register resolution helpers.
        assert_eq!(p.wvp_register().unwrap(), 0);
        assert_eq!(p.world_matrix_register().unwrap(), 4);
    }

    /// Encoding: every declared row appears exactly once, in plan
    /// order; set values land byte-exact; unset rows are zeros; the
    /// bool policy encodes as [1,0,0,0] / [0,0,0,0]; a value for an
    /// undeclared register is a hard error (drift guard).
    #[test]
    fn draw_values_encode_every_declared_row() {
        let p = real_plan();
        let (vs, fs) = default_draw_values(&p);
        let f32s = |bytes: &[u8; 16]| -> [f32; 4] {
            std::array::from_fn(|i| {
                f32::from_le_bytes([bytes[i * 4], bytes[i * 4 + 1], bytes[i * 4 + 2], bytes[i * 4 + 3]])
            })
        };
        let idx = |rows: &[(String, [u8; 16])], name: &str| -> usize {
            rows.iter()
                .position(|(n, _)| n == name)
                .expect("row name present")
        };
        let vs_rows = vs.encode(&p.vertex_resources).unwrap();
        assert_eq!(vs_rows.len(), 18);
        // World matrix identity at c4..c7.
        assert_eq!(f32s(&vs_rows[idx(&vs_rows, "u_c4")].1), [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(f32s(&vs_rows[idx(&vs_rows, "u_c7")].1), [0.0, 0.0, 0.0, 1.0]);
        // WVP untouched by the defaults (zeros until the caller sets it).
        assert_eq!(f32s(&vs_rows[idx(&vs_rows, "u_c0")].1), [0.0; 4]);

        let fs_rows = fs.encode(&p.fragment_resources).unwrap();
        assert_eq!(fs_rows.len(), 20 + 3);
        // materialDiffuse white at c8; light rows zero; bools.
        assert_eq!(f32s(&fs_rows[idx(&fs_rows, "u_c8")].1), [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(f32s(&fs_rows[idx(&fs_rows, "u_c0")].1), [0.0; 4]);
        assert_eq!(f32s(&fs_rows[idx(&fs_rows, "b0")].1), [1.0, 0.0, 0.0, 0.0], "IsLitPass=true");
        assert_eq!(f32s(&fs_rows[idx(&fs_rows, "b1")].1), [0.0; 4], "ShadowEnabled=false");
        assert_eq!(f32s(&fs_rows[idx(&fs_rows, "b2")].1), [0.0; 4], "ShadowIsLinearDepth=false");

        // Drift guard: a value for an undeclared register errors.
        let mut bad = StageValues::default();
        bad.set_float(99, [1.0; 4]);
        assert!(bad.encode(&p.vertex_resources).is_err());
        let mut badb = StageValues::default();
        badb.set_bool(0, true);
        assert!(badb.encode(&p.vertex_resources).is_err(), "vs declares no bools");

        // CTAB-named matrix setter.
        let mut vals = StageValues::default();
        set_matrix_by_name(&mut vals, &p.vertex_names, "p3dWorldViewProjectionMatrix", &[[1.0; 4]; 4])
            .unwrap();
        assert_eq!(f32s(&vals.encode(&p.vertex_resources).unwrap()[0].1), [1.0; 4]);
        assert!(set_matrix_by_name(&mut vals, &p.vertex_names, "nope", &[[0.0; 4]; 4]).is_err());
    }

    /// Material slots by CTAB NAME: diffuse → s4, extras → s5/s6,
    /// engine light/shadow textures → white; and the ascending-s#
    /// policy would have been WRONG here (s0 is a light texture).
    #[test]
    fn world_texture_assignment_is_name_driven() {
        let p = real_plan();
        // Full feed: diffuse + 2 extras.
        let a3 = world_texture_assignment(&p, 3);
        let regs: Vec<u16> = p
            .fragment_resources
            .bindings
            .iter()
            .filter_map(|b| match b {
                StageBinding::Texture { register, .. } => Some(*register),
                _ => None,
            })
            .collect();
        assert_eq!(regs, vec![0, 1, 2, 3, 4, 5, 6]);
        assert_eq!(a3, vec![None, None, None, None, Some(0), Some(1), Some(2)]);
        // Diffuse only: engine textures AND normal/specular stay white.
        let a1 = world_texture_assignment(&p, 1);
        assert_eq!(a1, vec![None, None, None, None, Some(0), None, None]);
    }

    /// `from_dll` on a SYNTHESIZED image (zeros + the two blobs at the
    /// PROGRAM_LIBRARY offsets): slicing + stage-token checks work; a
    /// wrong token or a truncated image fails with the exact error.
    #[test]
    fn from_dll_slices_blobs_at_the_registered_offsets() {
        let vs = fixture("dll_default_vp_e34e60.sm30");
        let fs = fixture("dll_default_fp_e3b308.sm30");
        let vs_off = PROGRAM_LIBRARY
            .iter()
            .find(|r| r.name == "default" && r.stage == crate::shader::library::Stage::Vs)
            .unwrap()
            .blob_offset as usize;
        let fs_off = PROGRAM_LIBRARY
            .iter()
            .find(|r| r.name == "default" && r.stage == crate::shader::library::Stage::Ps)
            .unwrap()
            .blob_offset as usize;
        let mut dll = vec![0u8; fs_off + fs.len()];
        dll[vs_off..vs_off + vs.len()].copy_from_slice(&vs);
        dll[fs_off..fs_off + fs.len()].copy_from_slice(&fs);
        let p = WorldPairPlan::from_dll(&dll).expect("synthetic dll plans");
        assert_eq!(p.wvp_register().unwrap(), 0);

        // Stage-token guard: corrupt the PS token → exact error.
        let mut bad = dll.clone();
        bad[fs_off] ^= 0xFF;
        match WorldPairPlan::from_dll(&bad) {
            Err(WorldError::StageToken { expected, got }) => {
                assert_eq!(expected, 0xFFFF_0300);
                assert_ne!(got, 0xFFFF_0300);
            }
            other => panic!("expected StageToken error, got {other:?}"),
        }
        // Range guard: a truncated image fails with BlobRange (the VS
        // blob is sliced first, so its offset is the one reported).
        match WorldPairPlan::from_dll(&dll[..vs_off]) {
            Err(WorldError::BlobRange { offset, .. }) => {
                assert_eq!(offset as usize, vs_off)
            }
            other => panic!("expected BlobRange error, got {other:?}"),
        }
    }

    /// The REAL DLL when it is present on this machine (sandbox:
    /// source/ADM/prototypeenginef.dll) must plan end-to-end; skipped
    /// (not failed) elsewhere — CI only carries the fixtures.
    #[test]
    fn real_dll_plans_when_present() {
        let dll_path = Path::new("/home/z/my-project/source/ADM/prototypeenginef.dll");
        if !dll_path.exists() {
            return; // honest skip: the sandbox may have been recycled
        }
        let dll = std::fs::read(dll_path).unwrap();
        let p = WorldPairPlan::from_dll(&dll).expect("the real DLL must plan");
        assert_eq!(p.vertex_resources.uniform_count(), 18);
        assert_eq!(p.fragment_resources.uniform_count(), 20);
        assert_eq!(p.fragment_resources.bool_count(), 3);
        assert_eq!(p.fragment_resources.sampler_count(), 7);
    }

    /// The default_vs declares NO bool branches and the default_ps
    /// branches only on b0..b2 — pin the instruction-level fact (the
    /// block-30 census) against the committed pair.
    #[test]
    fn bool_branch_census_is_b0_b2() {
        let p = real_plan();
        for inst in &p.vs.instructions {
            if let crate::shader::sm30::Instruction::Flow { op, .. } = inst {
                assert!(!matches!(op, Sm30Op::If), "vs must not branch");
            }
        }
        let mut fs_bools = Vec::new();
        for inst in &p.fs.instructions {
            if let crate::shader::sm30::Instruction::Flow {
                op: Sm30Op::If,
                reg: Some(r),
            } = inst
            {
                assert_eq!(r.ty, RegType::ConstBool);
                fs_bools.push(r.reg);
            }
        }
        assert_eq!(fs_bools, vec![0, 1, 2]);
    }

    /// Daylight state (block 35): every DESIGN row lands on the
    /// CTAB-proven register, the c6.w gate is OPEN (1e6), the sun
    /// direction is normalized and appears negated on c0 (the to-light
    /// cone dot) and positive on c1 (the N·L dot), and the encoding
    /// covers exactly those rows on the real fragment plan.
    #[test]
    fn daylight_state_fills_the_lit_pass_rows() {
        let p = real_plan();
        let day = daylight_state(&p).unwrap();
        let rows = day.encode(&p.fragment_resources).unwrap();
        let f32s = |b: &[u8; 16]| -> [f32; 4] {
            std::array::from_fn(|i| {
                f32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]])
            })
        };
        let get = |name: &str| -> [f32; 4] {
            let i = rows
                .iter()
                .position(|(n, _)| n == name)
                .expect("daylight row present");
            f32s(&rows[i].1)
        };
        // c0 = -sun (Direction), c1 = +sun (Attenuation slot, the N·L dot).
        let sun = SUN_DIRECTION;
        let c0 = get("u_c0");
        let c1 = get("u_c1");
        assert_eq!(c0, [-sun[0], -sun[1], -sun[2], 0.0]);
        assert_eq!(c1, [sun[0], sun[1], sun[2], 0.0]);
        // Sun is unit length.
        let l = (sun[0] * sun[0] + sun[1] * sun[1] + sun[2] * sun[2]).sqrt();
        assert!((l - 1.0).abs() < 1e-5);
        // Ambient / colour / the c6.w gate.
        assert_eq!(get("u_c2"), [0.35, 0.36, 0.40, 1.0]);
        assert_eq!(get("u_c3"), [1.0, 0.96, 0.88, 1.0]);
        assert_eq!(get("u_c6")[3], 1.0e6, "c6.w must be open (the lit gate)");
        assert_eq!(get("u_c6")[0], 0.0);
        // Untouched rows stay zero (specular c9, shadow info c11...).
        assert_eq!(get("u_c9"), [0.0; 4]);
        // The vertex-stage far light position resolves to c17.
        let (reg, row) = daylight_light_position(&p).unwrap().unwrap();
        assert_eq!(reg, 17);
        assert_eq!(row, [0.0, 0.0, 1.0e5, 0.0]);
    }

    /// Block 40 cross-check: the proven engine TYPE of each row
    /// ([`super::light_constants`]) agrees with what `daylight_state`
    /// writes.  Direction/Attenuation are VEC3 in the engine's own
    /// metadata — their `.w` is filler, so the DESIGN keeps 0.0 there;
    /// DecayConstant is vec4-class and its `.w` IS the gate.
    #[test]
    fn daylight_rows_respect_the_proven_engine_types() {
        use super::super::light_constants::{StandardConstantKind, SINGLE_LIGHT_CONSTANTS};
        let kind_of = |n: &str| {
            SINGLE_LIGHT_CONSTANTS
                .iter()
                .find(|s| s.name == n)
                .map(|s| s.kind)
                .expect("light constant in the proven table")
        };
        assert_eq!(kind_of("p3dLight_Direction"), StandardConstantKind::Vec3);
        assert_eq!(kind_of("p3dLight_Attenuation"), StandardConstantKind::Vec3);
        assert_eq!(kind_of("p3dLight_DecayConstant"), StandardConstantKind::Vec4);

        let p = real_plan();
        let day = daylight_state(&p).unwrap();
        let rows = day.encode(&p.fragment_resources).unwrap();
        let f32s = |b: &[u8; 16]| -> [f32; 4] {
            std::array::from_fn(|i| {
                f32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]])
            })
        };
        let get = |name: &str| -> [f32; 4] {
            let i = rows.iter().position(|(n, _)| n == name).expect("row");
            f32s(&rows[i].1)
        };
        // vec3 rows: the shader only masks .xyz (block-35 disasm), so
        // w == 0 is a valid filler.
        assert_eq!(get("u_c0")[3], 0.0);
        assert_eq!(get("u_c1")[3], 0.0);
        // vec4-class row: the gate lives in .w.
        assert_eq!(get("u_c6")[3], 1.0e6);
    }

    /// Unused-import guard: CtabConstant is re-exported for callers.
    #[test]
    fn ctab_constant_is_public_api() {
        fn accepts(_: Option<&CtabConstant>) {}
        accepts(None);
    }
}

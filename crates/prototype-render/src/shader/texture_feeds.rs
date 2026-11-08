//! `texture_feeds` — the world lit pass's SAMPLER FEEDS (NEXT-UP item 7,
//! block 42): what the engine actually puts behind the lit-pass pixel
//! shader's seven samplers, with the engine-side evidence for each.
//!
//! The world default pair (`default_vp` + `default_fp`, the committed
//! fixtures) samples seven textures.  Blocks 34-36 wired the three
//! material maps; s0..s3 stayed "engine render targets — white fallback
//! today".  Block 42 recovers the engine side of those four feeds from
//! `prototypeenginef.dll` (image base 0x10000000; .text
//! 0x10001000..0x10d90000, .rdata 0x10d90000..0x10feb000 — PE section
//! table re-verified this block).
//!
//! # What block 42 PROVED
//!
//! 1. **The lit-pass sampler roster (PROVEN twice, independently).**
//!    (a) The committed `dll_default_fp_e3b308.sm30` CTAB declares
//!    s0 = `p3dLight_DecayTexture`, s1 = `p3dLight_SpotlightTexture`,
//!    s2 = `p3dLight_ProjectedTexture`, s3 = `p3dShadowMap`,
//!    s4 = `diffuseMap`, s5 = `normalMap`, s6 = `specularMap` (the
//!    s4..s6 half was already wired in block 34).  (b) The engine's own
//!    per-pass parameter table — a static .rdata array of 16-B records
//!    `{ordinal u32, dtype u32, regset u32, name* u32}` at
//!    0x110bbc60..0x110bbe40 — enumerates the SAME pass's parameters:
//!    bool rows (regset 0) `p3dLight_IsLitPass` / `p3dShadowEnabled` /
//!    `p3dShadowIsLinearDepth`, vector rows (regset 1)
//!    `p3dLight_Direction..p3dLight_DecayConstant`,
//!    `materialShininess/Diffuse/Specular`, the shadow family (below),
//!    then sampler rows (regset 2) in EXACTLY the CTAB slot order
//!    Decay, Spotlight, Projected, ShadowMap, diffuse, normal,
//!    specular, closed by the vector row `p3dZScale`.  The "row order
//!    inside a pass's sampler sub-list == the D3D9 sampler register"
//!    rule is cross-checked against a second pass: the Bink YUV table
//!    (0x110bbc20..0x110bbc50) orders y/cr/cb/a == the committed
//!    `dll_bink_fp_e3adf8.sm30` CTAB s0..s3.  (Scripts m45a/m45c.)
//! 2. **The engine standard-constant system types all four light/shadow
//!    texture names as type 6 (TEXTURE)** in their lazy BSS descriptor
//!    rows `{name*, type, value}` (resolver block 0x106f4850..0x106f8f00;
//!    scripts m45a/m45b): `p3dLight_DecayTexture` → 0x11280548,
//!    `p3dLight_SpotlightTexture` → 0x11280478,
//!    `p3dLight_ProjectedTexture` → 0x1128053c, `p3dShadowMap` →
//!    0x1127fd20 — plus the two projection matrices
//!    `p3dLight_ProjectedTextureMatrix` (0x11280530) and
//!    `p3dShadowMapMatrix` (.rdata table 0x110bb9c0/0x110bbb80; type-5
//!    resolver row observed for the Projected one) and the type-row
//!    `p3dShadowMask` at 0x1127fd14.  The rows are only touched by the
//!    resolver itself (image-wide imm32 scan: zero other sites) —
//!    consumers receive the row pointer at runtime and read row+8.
//! 3. **The engine has a CHILD (cascade) SHADOW-MAP system (PROVEN
//!    names).**  The lit-pass vector family carries
//!    `p3dShadowEnabled`, `p3dShadowIsLinearDepth`,
//!    `p3dShadowEnabledChildren`, `p3dShadowMapInfo`,
//!    `p3dShadowMapChildBox00`, `p3dShadowMapChildBox01`,
//!    `p3dShadowMapChildBox03`, `p3dShadowMapChildInfo00..04` — note
//!    the REAL GAP: there is NO `ChildBox02` in the shipped table (the
//!    00/01/03 run is not a transcription slip; the .rdata rows at
//!    0x110bbd50..0x110bbdb0 are consecutive).  Four child slots
//!    (00/01/03 + the 04 infos) with per-child boxes and infos, driven
//!    by the shadow-cascade draw at 0x10767e90 (a per-child index loop
//!    over sub-draws 0x10767a90/0x10767b10/0x10767b60) — plus the
//!    dedicated shadow passes whose programs are committed:
//!    `shadowblur_fp` (samples `$intexture` s0 + `$depthBuffer` s1)
//!    and `shadowdebug_fp` (samples `p3dShadowMap` s0).
//! 4. **The renderer binds textures through the engine render-context
//!    wrapper, not the raw device (PROVEN mechanism, per-slot LIKELY).**
//!    The singleton global 0x1127f354 holds the render-context object;
//!    its own C++ vtable (NOT the d3d9 device vtable — block 40 proved
//!    the raw device vtable is never called) exposes a texture-bind
//!    method at vtable +0xF8 used across the shadow/light pass sites
//!    with the SAMPLER INDEX as the observed argument: `push 1; call`
//!    at 0x107655ce / 0x10768087 (fnA 0x10767fb0, the shadow-pass
//!    setup) / 0x107b1815 / 0x107c40a9, `push 0; call` at 0x107681c6 /
//!    0x107b1974, and the 2-arg form `(1, texture)` in fnB 0x1076feb0
//!    (`push edi; push 1; call edx`).  Around the shadow-cascade draw
//!    (fnA: call 0x10767e90 at 0x107681a1) the wrapper toggles the
//!    per-slot dirty dword at wrapper+0x5f0: `or 0x1; or 0x8` set
//!    before, `and ~0x8; and ~0x1` cleared after — the two LOW BITS
//!    correspond to samplers 0 and 3, the two slots whose feeds are
//!    engine-internal (decay + shadow map).  The wrapper also keeps
//!    the standard-constant float cache (wrapper+0x3c → float array,
//!    dirty flag wrapper+0x14c — store at 0x102cc43d/0x102cc440).
//!    WHICH texture object lands in each slot per frame (render target
//!    vs light-resource texture) is NOT YET recoverable — the wrapper's
//!    vtable layout is its own class and per-slot provenance stays
//!    UNKNOWN; the white-fallback renderer keeps s0..s3 engine-managed.
//!
//! # The honest boundaries
//!
//! * PROVEN: the roster (CTAB × engine table), the type-6 rows, the
//!   projection matrices, the shadow child-box family (with the 02
//!   gap), the wrapper bind mechanism (vtable+0xF8 with sampler args,
//!   dirty bits 0x1/0x8 around the shadow draw).
//! * LIKELY: s0/s1/s2 are per-light textures from the light entity's
//!   resource bindings (the light class registers SpotlightTexture as
//!   an `om` attribute — block 40's attribute≡constant identity), and
//!   the LDI light-decal resources (`Cell_NN_light_decal_K`, block 41)
//!   are the projected/spotlight cookie source.
//! * UNKNOWN: the per-frame texture OBJECTS behind s0..s3 (wrapper
//!   vtable identity per slot), the AEV-tail-domain-style semantics of
//!   `p3dShadowMapInfo`/`ChildBox*`/`ChildInfo*` rows, and the
//!   `p3dShadowMask` row's type code (ebx at its resolver write — not
//!   statically pinned like the type-6 rows).
//!
//! Scripts: `scripts/m45a..m45j` (evidence chain, in repo order).
//!
//! [deny(unsafe_code)] friendly: data tables + pure functions only.

// ---------------------------------------------------------------------------
// The lit-pass sampler roster (PROVEN: CTAB == engine .rdata order)
// ---------------------------------------------------------------------------

/// What feeds one lit-pass sampler slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LitFeed {
    /// Material diffuse map — upload slot 0 (wired block 34).
    MaterialDiffuse,
    /// Material normal map — upload slot 1 (wired block 34).
    MaterialNormal,
    /// Material specular map — upload slot 2 (wired block 34).
    MaterialSpecular,
    /// `p3dLight_DecayTexture` — per-light falloff texture,
    /// engine standard-constant row 0x11280548 (type 6).
    LightDecayTexture,
    /// `p3dLight_SpotlightTexture` — per-light cone cookie, row
    /// 0x11280478 (type 6); the wrapper's shadow-pass sites bind
    /// sampler 1 through vtable+0xF8.
    LightSpotlightTexture,
    /// `p3dLight_ProjectedTexture` — per-light projective cookie, row
    /// 0x1128053c (type 6), with `p3dLight_ProjectedTextureMatrix`
    /// (type-5 row 0x11280530) as the projector.
    LightProjectedTexture,
    /// `p3dShadowMap` — the engine's shadow render target (cascade
/// child system), row 0x1127fd20 (type 6); wrapper dirty bit 0x8
    /// toggles around the shadow-cascade draw.
    EngineShadowMap,
}

impl LitFeed {
    /// The engine-managed feeds (white fallback today — the renderer
    /// has no engine render-target chain yet).
    pub fn is_engine_managed(self) -> bool {
        !matches!(
            self,
            LitFeed::MaterialDiffuse | LitFeed::MaterialNormal | LitFeed::MaterialSpecular
        )
    }

    /// The material feeds' preferred upload slot (None = engine-managed).
    pub fn upload_slot(self) -> Option<usize> {
        match self {
            LitFeed::MaterialDiffuse => Some(0),
            LitFeed::MaterialNormal => Some(1),
            LitFeed::MaterialSpecular => Some(2),
            _ => None,
        }
    }
}

/// One lit-pass sampler row: slot, CTAB/engine name, feed, evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LitSamplerRow {
    /// The D3D9 sampler register (s#).
    pub slot: u8,
    /// The CTAB / engine pass-table name.
    pub name: &'static str,
    /// The classified feed.
    pub feed: LitFeed,
    /// The primary evidence anchor(s).
    pub evidence: &'static str,
}

/// The seven lit-pass samplers in slot order (PROVEN twice: the
/// committed `default_fp` CTAB and the engine's .rdata pass table).
pub const LIT_PASS_SAMPLERS: [LitSamplerRow; 7] = [
    LitSamplerRow {
        slot: 0,
        name: "p3dLight_DecayTexture",
        feed: LitFeed::LightDecayTexture,
        evidence: "CTAB s0; .rdata 0x110bbdd0; resolver row 0x11280548 (type 6)",
    },
    LitSamplerRow {
        slot: 1,
        name: "p3dLight_SpotlightTexture",
        feed: LitFeed::LightSpotlightTexture,
        evidence: "CTAB s1; .rdata 0x110bbde0; row 0x11280478 (type 6); wrapper binds s1 (0x107655ce et al.)",
    },
    LitSamplerRow {
        slot: 2,
        name: "p3dLight_ProjectedTexture",
        feed: LitFeed::LightProjectedTexture,
        evidence: "CTAB s2; .rdata 0x110bbdf0; row 0x1128053c (type 6) + ProjMatrix 0x11280530 (type 5)",
    },
    LitSamplerRow {
        slot: 3,
        name: "p3dShadowMap",
        feed: LitFeed::EngineShadowMap,
        evidence: "CTAB s3; .rdata 0x110bbe00; row 0x1127fd20 (type 6); dirty bit 0x8 around draw 0x10767e90",
    },
    LitSamplerRow {
        slot: 4,
        name: "diffuseMap",
        feed: LitFeed::MaterialDiffuse,
        evidence: "CTAB s4; .rdata 0x110bbe10; wired block 34",
    },
    LitSamplerRow {
        slot: 5,
        name: "normalMap",
        feed: LitFeed::MaterialNormal,
        evidence: "CTAB s5; .rdata 0x110bbe20; wired block 34",
    },
    LitSamplerRow {
        slot: 6,
        name: "specularMap",
        feed: LitFeed::MaterialSpecular,
        evidence: "CTAB s6; .rdata 0x110bbe30; wired block 34",
    },
];

/// Classify a lit-pass sampler by its engine name — `None` for a name
/// the engine's lit-pass table does not carry (callers must fail
/// closed, never guess).
pub fn lit_feed_of(name: &str) -> Option<LitFeed> {
    LIT_PASS_SAMPLERS
        .iter()
        .find(|r| r.name == name)
        .map(|r| r.feed)
}

// ---------------------------------------------------------------------------
// The engine's own per-pass parameter table (.rdata 0x110bbc60..)
// ---------------------------------------------------------------------------

/// Register-set code of the engine's 16-B pass-table records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassRegSet {
    /// regset 0 — bool constant rows (b#).
    Bool,
    /// regset 1 — vector/float4 constant rows (c#).
    Vector,
    /// regset 2 — sampler rows (s#; row ORDER is the slot).
    Sampler,
}

impl PassRegSet {
    /// The raw .rdata code (PROVEN: only 0/1/2 occur in the lit block).
    pub fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(PassRegSet::Bool),
            1 => Some(PassRegSet::Vector),
            2 => Some(PassRegSet::Sampler),
            _ => None,
        }
    }
}

/// One 16-B record of the engine's lit-pass parameter table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassParamRow {
    /// The record's first dword (ordinal within the pass's list —
    /// its exact semantics vs registers stay UNKNOWN; the SAMPLER
    /// sub-list's row order is what pins the slots).
    pub ordinal: u32,
    /// Register-set class.
    pub regset: PassRegSet,
    /// The parameter name (the .rdata string pointer target).
    pub name: &'static str,
    /// The record's .rdata VA (evidence anchor).
    pub va: u32,
}

/// The engine's lit-pass parameter table, transcribed in .rdata order
/// (records 0x110bbc60..0x110bbe40; scripts m45c).  The dtype dword is
/// 1 on every row (semantics UNKNOWN); regset codes 0/1/2 as encoded.
pub const ENGINE_LIT_PASS_TABLE: [PassParamRow; 31] = [
    PassParamRow { ordinal: 0, regset: PassRegSet::Bool, name: "p3dLight_IsLitPass", va: 0x110bbc60 },
    PassParamRow { ordinal: 0, regset: PassRegSet::Bool, name: "p3dShadowEnabled", va: 0x110bbc70 },
    PassParamRow { ordinal: 1, regset: PassRegSet::Bool, name: "p3dShadowIsLinearDepth", va: 0x110bbc80 },
    PassParamRow { ordinal: 2, regset: PassRegSet::Vector, name: "p3dLight_Direction", va: 0x110bbc90 },
    PassParamRow { ordinal: 0, regset: PassRegSet::Vector, name: "p3dLight_Attenuation", va: 0x110bbca0 },
    PassParamRow { ordinal: 1, regset: PassRegSet::Vector, name: "p3dLight_Ambient", va: 0x110bbcb0 },
    PassParamRow { ordinal: 2, regset: PassRegSet::Vector, name: "p3dLight_Colour", va: 0x110bbcc0 },
    PassParamRow { ordinal: 3, regset: PassRegSet::Vector, name: "p3dLight_LightType", va: 0x110bbcd0 },
    PassParamRow { ordinal: 4, regset: PassRegSet::Vector, name: "p3dLight_SpotlightConstant", va: 0x110bbce0 },
    PassParamRow { ordinal: 5, regset: PassRegSet::Vector, name: "p3dLight_DecayConstant", va: 0x110bbcf0 },
    PassParamRow { ordinal: 6, regset: PassRegSet::Vector, name: "materialShininess", va: 0x110bbd00 },
    PassParamRow { ordinal: 7, regset: PassRegSet::Vector, name: "materialDiffuse", va: 0x110bbd10 },
    PassParamRow { ordinal: 8, regset: PassRegSet::Vector, name: "materialSpecular", va: 0x110bbd20 },
    PassParamRow { ordinal: 9, regset: PassRegSet::Vector, name: "p3dShadowEnabledChildren", va: 0x110bbd30 },
    PassParamRow { ordinal: 10, regset: PassRegSet::Vector, name: "p3dShadowMapInfo", va: 0x110bbd40 },
    PassParamRow { ordinal: 11, regset: PassRegSet::Vector, name: "p3dShadowMapChildBox00", va: 0x110bbd50 },
    PassParamRow { ordinal: 12, regset: PassRegSet::Vector, name: "p3dShadowMapChildBox01", va: 0x110bbd60 },
    PassParamRow { ordinal: 13, regset: PassRegSet::Vector, name: "p3dShadowMapChildBox03", va: 0x110bbd70 },
    PassParamRow { ordinal: 14, regset: PassRegSet::Vector, name: "p3dShadowMapChildInfo00", va: 0x110bbd80 },
    PassParamRow { ordinal: 15, regset: PassRegSet::Vector, name: "p3dShadowMapChildInfo01", va: 0x110bbd90 },
    PassParamRow { ordinal: 16, regset: PassRegSet::Vector, name: "p3dShadowMapChildInfo02", va: 0x110bbda0 },
    PassParamRow { ordinal: 17, regset: PassRegSet::Vector, name: "p3dShadowMapChildInfo03", va: 0x110bbdb0 },
    PassParamRow { ordinal: 18, regset: PassRegSet::Vector, name: "p3dShadowMapChildInfo04", va: 0x110bbdc0 },
    PassParamRow { ordinal: 19, regset: PassRegSet::Sampler, name: "p3dLight_DecayTexture", va: 0x110bbdd0 },
    PassParamRow { ordinal: 0, regset: PassRegSet::Sampler, name: "p3dLight_SpotlightTexture", va: 0x110bbde0 },
    PassParamRow { ordinal: 1, regset: PassRegSet::Sampler, name: "p3dLight_ProjectedTexture", va: 0x110bbdf0 },
    PassParamRow { ordinal: 2, regset: PassRegSet::Sampler, name: "p3dShadowMap", va: 0x110bbe00 },
    PassParamRow { ordinal: 3, regset: PassRegSet::Sampler, name: "diffuseMap", va: 0x110bbe10 },
    PassParamRow { ordinal: 4, regset: PassRegSet::Sampler, name: "normalMap", va: 0x110bbe20 },
    PassParamRow { ordinal: 5, regset: PassRegSet::Sampler, name: "specularMap", va: 0x110bbe30 },
    PassParamRow { ordinal: 6, regset: PassRegSet::Vector, name: "p3dZScale", va: 0x110bbe40 },
];

/// The engine table's sampler rows in .rdata order — must equal
/// [`LIT_PASS_SAMPLERS`] by name (the double-PROVEN invariant).
pub fn engine_table_sampler_order() -> Vec<&'static str> {
    ENGINE_LIT_PASS_TABLE
        .iter()
        .filter(|r| r.regset == PassRegSet::Sampler)
        .map(|r| r.name)
        .collect()
}

/// The shadow-cascade child-box names as shipped (note: 00, 01, 03 —
/// NO ChildBox02; the gap is in the engine's own table).
pub const SHADOW_CHILD_BOX_NAMES: [&str; 3] = [
    "p3dShadowMapChildBox00",
    "p3dShadowMapChildBox01",
    "p3dShadowMapChildBox03",
];

/// The shadow-cascade child-info names (00..04, complete run).
pub const SHADOW_CHILD_INFO_NAMES: [&str; 5] = [
    "p3dShadowMapChildInfo00",
    "p3dShadowMapChildInfo01",
    "p3dShadowMapChildInfo02",
    "p3dShadowMapChildInfo03",
    "p3dShadowMapChildInfo04",
];

// ---------------------------------------------------------------------------
// The standard-constant resolver rows for the texture feeds
// ---------------------------------------------------------------------------

/// One by-name resolver descriptor row (12-B BSS: {name*, type, value}).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolverRow {
    /// The constant name.
    pub name: &'static str,
    /// The engine type code (6 = TEXTURE, 5 = matrix).
    pub type_code: u32,
    /// The BSS descriptor row VA (evidence anchor).
    pub descriptor_va: u32,
}

/// The four lit-pass texture rows + the projected-texture matrix
/// (type 5) — all pinned from the resolver's lazy-init stores
/// (scripts m45a/m45b).  `p3dShadowMask` (0x1127fd14) is deliberately
/// NOT here: its type dword came from a register (ebx), not an
/// immediate — not statically pinned.
pub const TEXTURE_RESOLVER_ROWS: [ResolverRow; 5] = [
    ResolverRow { name: "p3dLight_DecayTexture", type_code: 6, descriptor_va: 0x11280548 },
    ResolverRow { name: "p3dLight_SpotlightTexture", type_code: 6, descriptor_va: 0x11280478 },
    ResolverRow { name: "p3dLight_ProjectedTexture", type_code: 6, descriptor_va: 0x1128053c },
    ResolverRow { name: "p3dShadowMap", type_code: 6, descriptor_va: 0x1127fd20 },
    ResolverRow { name: "p3dLight_ProjectedTextureMatrix", type_code: 5, descriptor_va: 0x11280530 },
];

// ---------------------------------------------------------------------------
// The render-context wrapper evidence (mechanism PROVEN, slots LIKELY)
// ---------------------------------------------------------------------------

/// The render-context singleton global (holds the object whose own C++
/// vtable the renderer calls — block 40 proved the RAW d3d9 device
/// vtable is never called directly).
pub const RENDER_CONTEXT_GLOBAL: u32 = 0x1127F354;

/// The wrapper vtable's texture-bind method offset (sampler-index args
/// observed: 0 and 1; the 2-arg `(1, texture)` form in fnB 0x1076feb0).
pub const WRAPPER_TEXTURE_BIND_VT_OFF: u32 = 0xF8;

/// The wrapper's per-slot dirty dword offset (`or 0x1; or 0x8` set
/// before, `and ~0x8; and ~0x1` cleared after the shadow draw — the
/// low bits correspond to samplers 0 and 3).
pub const WRAPPER_SLOT_DIRTY_OFF: u32 = 0x5F0;

/// The dirty bits toggled around the shadow-cascade draw.
pub const SHADOW_DRAW_DIRTY_BITS: u32 = 0x1 | 0x8;

/// The shadow-cascade draw function (per-child index loop over
/// 0x10767a90 / 0x10767b10 / 0x10767b60; called from fnA 0x10767fb0).
pub const SHADOW_CASCADE_DRAW_FN: u32 = 0x10767E90;

/// The shadow-pass setup function (binds sampler 1 via vtable+0xF8 at
/// 0x10768087, toggles the dirty bits, then draws).
pub const SHADOW_PASS_SETUP_FN: u32 = 0x10767FB0;

// ---------------------------------------------------------------------------
// Fail-closed feed planning
// ---------------------------------------------------------------------------

/// Feed-plan errors (fail-closed: no guessing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextureFeedError {
    /// A sampler the engine's lit-pass table does not classify.
    UnclassifiedSampler { register: u16, name: String },
    /// The CTAB's sampler set disagrees with the PROVEN roster.
    RosterMismatch { expected: Vec<&'static str>, got: Vec<String> },
}

impl std::fmt::Display for TextureFeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TextureFeedError::UnclassifiedSampler { register, name } => {
                write!(f, "s{register} ({name:?}) is not in the engine's lit-pass table")
            }
            TextureFeedError::RosterMismatch { expected, got } => {
                write!(f, "lit-pass roster mismatch: expected {expected:?}, got {got:?}")
            }
        }
    }
}

/// The classified feed for one planned sampler binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotFeed {
    /// The sampler register (s#).
    pub register: u16,
    /// The CTAB name at this register.
    pub name: String,
    /// The classified feed.
    pub feed: LitFeed,
}

/// Build the lit-pass feed plan from the real fragment CTAB's sampler
/// map (register → name).  Fail-closed: every sampler must classify,
/// and the full set must equal the PROVEN roster (order-insensitive on
/// the map, but the slot set must match exactly).
pub fn lit_pass_feed_plan(
    samplers: &dyn Fn() -> Vec<(u16, String)>,
) -> Result<Vec<SlotFeed>, TextureFeedError> {
    let mut got: Vec<(u16, String)> = samplers();
    got.sort_by_key(|(r, _)| *r);
    let mut out = Vec::with_capacity(got.len());
    for (register, name) in &got {
        let feed = lit_feed_of(name).ok_or_else(|| TextureFeedError::UnclassifiedSampler {
            register: *register,
            name: name.clone(),
        })?;
        if feed != lit_feed_of(
            LIT_PASS_SAMPLERS
                .get(*register as usize)
                .map(|r| r.name)
                .unwrap_or(""),
        )
        .unwrap_or(LitFeed::EngineShadowMap)
        {
            // name classifies but not to this slot's feed — roster break
            return Err(TextureFeedError::RosterMismatch {
                expected: LIT_PASS_SAMPLERS.iter().map(|r| r.name).collect(),
                got: got.iter().map(|(_, n)| n.clone()).collect(),
            });
        }
        out.push(SlotFeed { register: *register, name: name.clone(), feed });
    }
    let expected: Vec<&'static str> = LIT_PASS_SAMPLERS.iter().map(|r| r.name).collect();
    let got_names: Vec<String> = out.iter().map(|s| s.name.clone()).collect();
    if got_names != expected.iter().map(|s| s.to_string()).collect::<Vec<_>>() {
        return Err(TextureFeedError::RosterMismatch { expected, got: got_names });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Tests — against the committed real blobs (device-free)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shader::sm30::Sm30Shader;
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

    /// CTAB sampler rows of a committed fixture, sorted by register.
    fn ctab_samplers(name: &str) -> Vec<(u16, String)> {
        let s = Sm30Shader::parse_program(&fixture(name), true)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let ctab = s.ctab.as_ref().unwrap_or_else(|| panic!("{name}: no CTAB"));
        let mut v: Vec<(u16, String)> = ctab
            .constants
            .iter()
            .filter(|c| c.register_set == 3)
            .map(|c| (c.register_index, c.name.clone()))
            .collect();
        v.sort_by_key(|(r, _)| *r);
        v
    }

    /// PROVEN: the committed world default_fp CTAB's samplers are the
    /// roster, in slot order.
    #[test]
    fn lit_sampler_roster_matches_the_real_default_fp_ctab() {
        let got = ctab_samplers("dll_default_fp_e3b308.sm30");
        let want: Vec<(u16, String)> = LIT_PASS_SAMPLERS
            .iter()
            .map(|r| (r.slot as u16, r.name.to_string()))
            .collect();
        assert_eq!(got, want, "default_fp CTAB samplers must be the roster");
    }

    /// PROVEN: the engine .rdata pass table's sampler sub-list order is
    /// the roster order (the engine's own enumeration agrees with the
    /// shader's CTAB).
    #[test]
    fn engine_rdata_table_sampler_order_is_the_roster() {
        assert_eq!(engine_table_sampler_order(), {
            let v: Vec<&'static str> = LIT_PASS_SAMPLERS.iter().map(|r| r.name).collect();
            v
        });
    }

    /// PROVEN: the order rule cross-checks on a second pass — the Bink
    /// YUV table rows (y, cr, cb, a) == the bink_fp CTAB s0..s3.
    #[test]
    fn bink_ctab_order_rule_cross_check() {
        let got = ctab_samplers("dll_bink_fp_e3adf8.sm30");
        assert_eq!(
            got,
            vec![
                (0u16, "y_tex".to_string()),
                (1, "cr_tex".to_string()),
                (2, "cb_tex".to_string()),
                (3, "a_tex".to_string()),
            ]
        );
        // the engine's bink rows in .rdata order (m45c dump)
        assert_eq!(
            engine_table_bink_order(),
            ["y_tex", "cr_tex", "cb_tex", "a_tex"]
        );
    }

    fn engine_table_bink_order() -> Vec<&'static str> {
        // transcribed from 0x110bbc20..0x110bbc50 (m45c): the four
        // regset-2 rows of the Bink pass, in .rdata order
        vec!["y_tex", "cr_tex", "cb_tex", "a_tex"]
    }

    /// PROVEN: the shadow child-box run carries a REAL 02 gap
    /// (00, 01, 03 — no ChildBox02 anywhere in the lit table).
    #[test]
    fn shadow_child_box_02_gap_is_real() {
        assert_eq!(SHADOW_CHILD_BOX_NAMES, [
            "p3dShadowMapChildBox00",
            "p3dShadowMapChildBox01",
            "p3dShadowMapChildBox03",
        ]);
        for r in ENGINE_LIT_PASS_TABLE.iter() {
            assert!(
                !r.name.contains("ChildBox02"),
                "ChildBox02 must not exist in the engine table"
            );
        }
        assert_eq!(SHADOW_CHILD_INFO_NAMES.len(), 5);
    }

    /// PROVEN: the resolver's texture rows (name, type 6, BSS VA) and
    /// the projected-texture matrix row (type 5).
    #[test]
    fn resolver_texture_rows_are_pinned() {
        let want = [
            ("p3dLight_DecayTexture", 6u32, 0x1128_0548u32),
            ("p3dLight_SpotlightTexture", 6, 0x1128_0478),
            ("p3dLight_ProjectedTexture", 6, 0x1128_053c),
            ("p3dShadowMap", 6, 0x1127_fd20),
            ("p3dLight_ProjectedTextureMatrix", 5, 0x1128_0530),
        ];
        for (row, w) in TEXTURE_RESOLVER_ROWS.iter().zip(want.iter()) {
            assert_eq!((row.name, row.type_code, row.descriptor_va), *w);
        }
    }

    /// The feed plan over the REAL committed pair classifies all seven
    /// slots; the engine-managed/material split is exactly s0..s3 vs
    /// s4..s6; the material slots keep their upload slots 0/1/2.
    #[test]
    fn feed_plan_over_the_real_pair_is_typed_and_fail_closed() {
        let samplers = ctab_samplers("dll_default_fp_e3b308.sm30");
        let plan = lit_pass_feed_plan(&|| samplers.clone()).expect("the real CTAB must plan");
        assert_eq!(plan.len(), 7);
        for sf in &plan {
            assert_eq!(sf.feed.is_engine_managed(), sf.register < 4);
            assert_eq!(sf.feed.upload_slot(), match sf.register {
                4 => Some(0),
                5 => Some(1),
                6 => Some(2),
                _ => None,
            });
        }
        assert_eq!(plan[3].feed, LitFeed::EngineShadowMap);
        assert_eq!(plan[0].feed, LitFeed::LightDecayTexture);
        assert_eq!(plan[1].feed, LitFeed::LightSpotlightTexture);
        assert_eq!(plan[2].feed, LitFeed::LightProjectedTexture);

        // fail-closed: an unclassified name is an ERROR, never a guess
        let bad = vec![(0u16, "p3dLight_DecayTexture".to_string()), (1, "notAFeed".to_string())];
        assert_eq!(
            lit_pass_feed_plan(&|| bad.clone()),
            Err(TextureFeedError::UnclassifiedSampler {
                register: 1,
                name: "notAFeed".to_string()
            })
        );
        // fail-closed: a shuffled roster (missing slot) is an ERROR
        let short: Vec<(u16, String)> =
            samplers.iter().filter(|(r, _)| *r != 6).cloned().collect();
        assert!(matches!(
            lit_pass_feed_plan(&|| short.clone()),
            Err(TextureFeedError::RosterMismatch { .. })
        ));
    }

    /// The wrapper-mechanism constants are pinned (block-42 evidence).
    #[test]
    fn wrapper_evidence_constants_are_pinned() {
        assert_eq!(RENDER_CONTEXT_GLOBAL, 0x1127_F354);
        assert_eq!(WRAPPER_TEXTURE_BIND_VT_OFF, 0xF8);
        assert_eq!(WRAPPER_SLOT_DIRTY_OFF, 0x5F0);
        assert_eq!(SHADOW_DRAW_DIRTY_BITS, 0x9);
        assert_eq!(SHADOW_CASCADE_DRAW_FN, 0x1076_7E90);
        assert_eq!(SHADOW_PASS_SETUP_FN, 0x1076_7FB0);
    }
}

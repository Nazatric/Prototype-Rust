//! light_constants — the engine's standard light/shader-constant
//! METADATA layer, recovered from `prototypeenginef.dll` (block 40).
//!
//! The world lit pass consumes rows c0..c9 (+ VS c17) whose per-light
//! VALUES are written by the engine each frame.  Block 35 pinned the
//! shader-side consumption from the disasm; block 40 went after the
//! fill side in the engine DLL.  Result: the fill FORMULAS stay
//! UNKNOWN (see the dead-path note below), but the constant METADATA
//! layer is now PROVEN end to end — names, engine-side types, the
//! 4-light indexed family, and the attribute≡constant identity:
//!
//! # What block 40 PROVED (all addresses in `prototypeenginef.dll`,
//! image base 0x10000000, file offset == RVA for .text/.rdata)
//!
//! 1. **The complete standard-constant name pool** (.rdata
//!    0x10e33f34..0x10e34ad0 single-light + 0x10e5805c..0x10e58640
//!    multi-light/Scatter): every `p3dLight_*` name the shaders' CTABs
//!    use, the `p3dShadow*` family, the `p3dWorld*/View/Projection*`
//!    matrices and `p3dEyePositionObjectSpace`.  (Scripts m43a/m43b.)
//! 2. **Engine-side TYPE CODES** from the by-name resolver's lazy-init
//!    descriptor inits (each BSS descriptor = {name*, type, fixed-ptr};
//!    resolver block 0x106f4850..0x106f8f00, strcmp dispatch per name):
//!    `p3dLight_Direction` / `Attenuation` / `PositionObjectSpace` /
//!    `Position` = **3**; `Colour` / `Ambient` / `DecayConstant` /
//!    `LightType` / `SpotlightConstant` = the resolver default (4-class,
//!    vec4); `SpotlightTexture` = **6**; every matrix name = **5**.
//!    The LIVE metaclass registrations (`om` attribute registry:
//!    helper 0x106fc9d0(map, type, descriptor) + default-set
//!    0x1073d980) push the SAME ids (4 and 6 observed at
//!    0x106fd54b/0x106fd599).  Type 3 = VEC3 means the engine writes
//!    THREE floats for Direction/Attenuation/PositionObjectSpace —
//!    the `.w` the shaders read on those rows is unconstrained filler
//!    (the shader only ever masks .xyz — see the block-35 rows).
//!    DecayConstant IS vec4-class, consistent with the `.w` texkill
//!    gate.  (Scripts m43c/m43i/m43j.)
//! 3. **The 4-light indexed family** (`p3dLight_PositionObjectSpace0..3`,
//!    `p3dLight_DecayConstant0..3`, same for Attenuation/Colour/... in
//!    the 0x10e5805c..0x10e58640 pool): the engine supports FOUR
//!    concurrent lights per pass in the multi-light program family.
//!    The single-light pool has no suffixes — the default world pair
//!    is the single-light program.  (Scripts m43a/m43b.)
//! 4. **Attribute ≡ constant identity**: the `om` MetaClass system
//!    registers 307 attribute names via a constructor table
//!    (.rdata 0x10d91d34, 307 × 0x40-stride thunks); the CONSECUTIVE
//!    run of thunks for the single-light pool inserts
//!    `p3dLight_Ambient`, `Attenuation`, `Colour`, `Position`,
//!    `Direction`, `LightType`, `SpotlightTexture`, `SpotlightConstant`,
//!    `DecayConstant`, `PositionObjectSpace`, ... into ONE per-class
//!    attribute map (BSS 0x11280fe8) — the light class exposes its
//!    properties under the EXACT names of the shader constants, 1:1.
//!    (Scripts m43g/m43h context, m43p/m43s chain.)
//! 5. **The standard-matrix state object**: one registration method
//!    (0x1061e8f0) binds 15 matrices (p3dWorldMatrix, View, Projection
//!    and their Inverse/Transpose variants) into per-object slots
//!    (0x48-B stride, +0x90..+0x480) plus `p3dEyePositionObjectSpace`
//!    (vec3, +0x494) — the per-draw matrix+eye source.  (Script m43g.)
//!
//! # Why the fill FORMULAS stay UNKNOWN (dead-path evidence)
//!
//! * `d3d9.dll` is imported ONLY for `Direct3DCreate9` (IAT
//!   0x1133242c) and that import is DEAD: exactly one `jmp [IAT]`
//!   thunk (0x10a89fd0) with ZERO E8/E9/pointer references in the
//!   whole image; no `call [device+0x190]` (SetPixelShaderConstantF)
//!   or `[device+0x16C]` (SetVertexShaderConstantF) sites exist
//!   anywhere in .text (m43k/m43m/m43o/m43r).
//! * The by-name constant resolver block (0x106f4850..0x106f8f00) is
//!   UNREACHABLE dead code: no E8/E9 target lands inside the range and
//!   no .rdata/.data dword points into it (m43j/m43o/m43s follow-ups).
//!
//! ⇒ This `prototypeenginef.dll` build does not contain the live
//!   PC-D3D9 constant fill; the live renderer fills constants through
//!   the metaclass property walk identified in (4), whose per-property
//!   value formulas (what Colour/Attenuation/DecayConstant MEAN per
//!   light state) are not recoverable from this binary.  The block-35
//!   shader-side DESIGN (`daylight_state`) therefore remains the
//!   ground truth for values, now with the metadata layer PROVEN
//!   around it.
//!
//! Scripts: `scripts/m43a`..`m43s` (evidence chain, in repo order).

/// The engine-side type of a standard shader constant (block 40, PROVEN
/// from the resolver descriptor inits + metaclass registrations).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandardConstantKind {
    /// Engine type id 3 — three floats are written; `.w` is filler.
    Vec3,
    /// The resolver default class (vec4-family) — full row meaningful.
    Vec4,
    /// Engine type id 6 — a texture binding, not a float row.
    Texture,
    /// Engine type id 5 — a 4x4 matrix (4 float4 rows).
    Matrix44,
}

impl StandardConstantKind {
    /// The numeric id the engine's descriptor/metaclass layer carries
    /// (PROVEN for Vec3=3 / Texture=6 / Matrix44=5; Vec4 is the
    /// resolver's default class — its numeric id was a live register
    /// value, so the mapping "default ⇒ 4" is LIKELY, not pinned).
    pub fn engine_type_id(self) -> Option<u32> {
        match self {
            StandardConstantKind::Vec3 => Some(3),
            StandardConstantKind::Vec4 => None, // resolver default (ebx)
            StandardConstantKind::Texture => Some(6),
            StandardConstantKind::Matrix44 => Some(5),
        }
    }
}

/// One standard light/shader constant of the engine's metadata layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StandardConstantSpec {
    /// The exact name in the DLL's .rdata pool (== the CTAB name ==
    /// the metaclass attribute name — block 40, PROVEN identity).
    pub name: &'static str,
    /// The engine-side type (block 40, PROVEN per row below).
    pub kind: StandardConstantKind,
    /// The .rdata address of the name string (evidence anchor).
    pub name_va: u32,
}

/// The SINGLE-light standard constants that feed the world lit pass,
/// in the resolver's own priority order (block 40, script m43c chain
/// order: the strcmp-walk visits them exactly in this sequence).
pub const SINGLE_LIGHT_CONSTANTS: &[StandardConstantSpec] = &[
    StandardConstantSpec { name: "p3dLight_Ambient", kind: StandardConstantKind::Vec4, name_va: 0x10e349ec },
    StandardConstantSpec { name: "p3dLight_Attenuation", kind: StandardConstantKind::Vec3, name_va: 0x10e34a00 },
    StandardConstantSpec { name: "p3dLight_Colour", kind: StandardConstantKind::Vec4, name_va: 0x10e349dc },
    StandardConstantSpec { name: "p3dLight_Position", kind: StandardConstantKind::Vec3, name_va: 0x10e58614 },
    StandardConstantSpec { name: "p3dLight_Direction", kind: StandardConstantKind::Vec3, name_va: 0x10e34a18 },
    StandardConstantSpec { name: "p3dLight_LightType", kind: StandardConstantKind::Vec4, name_va: 0x10e349c8 },
    StandardConstantSpec { name: "p3dLight_SpotlightTexture", kind: StandardConstantKind::Texture, name_va: 0x10e34838 },
    StandardConstantSpec { name: "p3dLight_SpotlightConstant", kind: StandardConstantKind::Vec4, name_va: 0x10e349ac },
    StandardConstantSpec { name: "p3dLight_DecayConstant", kind: StandardConstantKind::Vec4, name_va: 0x10e34994 },
    StandardConstantSpec { name: "p3dLight_PositionObjectSpace", kind: StandardConstantKind::Vec3, name_va: 0x10e34a9c },
];

/// The resolver chain continues with these after the light rows
/// (`p3dLight_IsLitPass` gates the lit pass; the texture rows are the
/// engine render targets; `p3dShadowEnabled`/`IsLinearDepth` gate the
/// shadow path).
pub const SINGLE_LIGHT_TAIL_CONSTANTS: &[StandardConstantSpec] = &[
    StandardConstantSpec { name: "p3dLight_DecayTexture", kind: StandardConstantKind::Texture, name_va: 0x10e34854 },
    StandardConstantSpec { name: "p3dLight_ProjectedTexture", kind: StandardConstantKind::Texture, name_va: 0x10e3481c },
    StandardConstantSpec { name: "p3dLight_IsLitPass", kind: StandardConstantKind::Vec4, name_va: 0x10e34a58 },
    StandardConstantSpec { name: "p3dShadowEnabled", kind: StandardConstantKind::Vec4, name_va: 0x10e34a44 },
    StandardConstantSpec { name: "p3dShadowIsLinearDepth", kind: StandardConstantKind::Vec4, name_va: 0x10e34a2c },
    StandardConstantSpec { name: "p3dShadowMap", kind: StandardConstantKind::Texture, name_va: 0x10e3480c },
    StandardConstantSpec { name: "p3dShadowMapMatrix", kind: StandardConstantKind::Matrix44, name_va: 0x10e34abc },
    StandardConstantSpec { name: "p3dEyePositionObjectSpace", kind: StandardConstantKind::Vec3, name_va: 0x10e33f34 },
];

/// The engine's maximum concurrent lights per pass (block 40, PROVEN):
/// the multi-light pool carries indexed variants `...0`..`...3`.
pub const ENGINE_MAX_LIGHTS_PER_PASS: usize = 4;

/// The indexed multi-light family for a base single-light name, as the
/// engine's own pool spells it (`p3dLight_PositionObjectSpace0..3`,
/// `p3dLight_DecayConstant0..3` — PROVEN pool 0x10e5805c..0x10e58640).
/// Only names the pool ACTUALLY indexes return entries; the caller can
/// match on `None` to stay honest for names the engine does not index
/// (the default world pair is the single-light program anyway).
pub fn multi_light_family(base: &str) -> Option<[String; ENGINE_MAX_LIGHTS_PER_PASS]> {
    const INDEXED: &[&str] = &[
        "p3dLight_PositionObjectSpace",
        "p3dLight_DecayConstant",
        "p3dLight_Attenuation",
        "p3dLight_Colour",
        "p3dLight_Ambient",
    ];
    if !INDEXED.contains(&base) {
        return None;
    }
    Some(core::array::from_fn(|i| format!("{base}{i}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The single-light rows are exactly the CTAB names of the world
    /// lit pass (block 34 PROVEN register map) — the metadata layer
    /// and the shader must agree 1:1.
    #[test]
    fn single_light_constants_cover_the_world_lit_pass() {
        let names: Vec<_> = SINGLE_LIGHT_CONSTANTS.iter().map(|s| s.name).collect();
        for required in [
            "p3dLight_Direction",
            "p3dLight_Attenuation",
            "p3dLight_Ambient",
            "p3dLight_Colour",
            "p3dLight_DecayConstant",
            "p3dLight_LightType",
            "p3dLight_SpotlightConstant",
            "p3dLight_PositionObjectSpace",
        ] {
            assert!(names.contains(&required), "missing {required}");
        }
    }

    /// Type codes: the vec3 rows are EXACTLY the three the resolver
    /// init'd with immediate 3 (Direction / Attenuation /
    /// PositionObjectSpace — and `p3dLight_Position` in the tail);
    /// textures are 6; the vec4-class rows stay the default class.
    #[test]
    fn type_codes_match_the_resolver_inits() {
        let kind_of = |n: &str| {
            SINGLE_LIGHT_CONSTANTS
                .iter()
                .chain(SINGLE_LIGHT_TAIL_CONSTANTS)
                .find(|s| s.name == n)
                .map(|s| s.kind)
                .unwrap_or_else(|| panic!("{n} not in the tables"))
        };
        assert_eq!(kind_of("p3dLight_Direction"), StandardConstantKind::Vec3);
        assert_eq!(kind_of("p3dLight_Attenuation"), StandardConstantKind::Vec3);
        assert_eq!(kind_of("p3dLight_PositionObjectSpace"), StandardConstantKind::Vec3);
        assert_eq!(kind_of("p3dEyePositionObjectSpace"), StandardConstantKind::Vec3);
        assert_eq!(kind_of("p3dLight_DecayConstant"), StandardConstantKind::Vec4);
        assert_eq!(kind_of("p3dLight_Colour"), StandardConstantKind::Vec4);
        assert_eq!(kind_of("p3dLight_Ambient"), StandardConstantKind::Vec4);
        assert_eq!(kind_of("p3dLight_LightType"), StandardConstantKind::Vec4);
        assert_eq!(kind_of("p3dLight_IsLitPass"), StandardConstantKind::Vec4);
        assert_eq!(kind_of("p3dLight_SpotlightTexture"), StandardConstantKind::Texture);
        assert_eq!(kind_of("p3dLight_DecayTexture"), StandardConstantKind::Texture);
        assert_eq!(kind_of("p3dShadowMap"), StandardConstantKind::Texture);
        assert_eq!(kind_of("p3dShadowMapMatrix"), StandardConstantKind::Matrix44);
        // numeric ids: only the pinned ones carry an id (honest None
        // for the resolver-default class)
        assert_eq!(StandardConstantKind::Vec3.engine_type_id(), Some(3));
        assert_eq!(StandardConstantKind::Texture.engine_type_id(), Some(6));
        assert_eq!(StandardConstantKind::Matrix44.engine_type_id(), Some(5));
        assert_eq!(StandardConstantKind::Vec4.engine_type_id(), None);
    }

    /// The 4-light family: PROVEN names only, exact spellings from
    /// the pool.
    #[test]
    fn multi_light_family_matches_the_dll_pool() {
        let pos = multi_light_family("p3dLight_PositionObjectSpace").unwrap();
        assert_eq!(
            pos,
            [
                "p3dLight_PositionObjectSpace0".to_string(),
                "p3dLight_PositionObjectSpace1".to_string(),
                "p3dLight_PositionObjectSpace2".to_string(),
                "p3dLight_PositionObjectSpace3".to_string(),
            ]
        );
        let dec = multi_light_family("p3dLight_DecayConstant").unwrap();
        assert_eq!(dec[0], "p3dLight_DecayConstant0");
        assert_eq!(dec[3], "p3dLight_DecayConstant3");
        // names the pool does NOT index fail closed
        assert!(multi_light_family("p3dLight_Direction").is_none());
        assert!(multi_light_family("p3dLight_IsLitPass").is_none());
        assert_eq!(ENGINE_MAX_LIGHTS_PER_PASS, 4);
    }

    /// Name-pool addresses are unique and inside .rdata (sanity on the
    /// evidence anchors).
    #[test]
    fn evidence_anchors_are_unique_rdata_addresses() {
        let mut seen = std::collections::BTreeSet::new();
        for spec in SINGLE_LIGHT_CONSTANTS.iter().chain(SINGLE_LIGHT_TAIL_CONSTANTS) {
            assert!(
                (0x10D90000..0x10FEB000).contains(&spec.name_va),
                "{} anchor 0x{:08x} outside .rdata",
                spec.name,
                spec.name_va
            );
            assert!(seen.insert(spec.name_va), "duplicate anchor for {}", spec.name);
        }
    }
}

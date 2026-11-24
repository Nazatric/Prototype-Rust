//! plan — the DEVICE-FREE half of the wgpu mapping (moved out of the
//! feature-gated `wgpu_backend` so the boot path can use it without
//! pulling wgpu).
//!
//! Everything here is pure derivation from decoded SM 3.0 bytecode +
//! emitted WGSL text — no wgpu types, no GPU, testable on every host:
//!
//! * [`plan_stage_resources`] — the exact per-stage binding list (one
//!   16-byte uniform row per c# register row, texture+sampler pairs per
//!   s#) using the SAME arithmetic as the WGSL translator
//!   ([`super::wgsl`]).
//! * [`validate_plan_matches_wgsl`] — cross-checks a plan against
//!   emitted WGSL text both ways (every planned declaration present,
//!   every `@binding(` accounted for).
//! * [`renumber_group`] — the fragment WGSL moves to bind group 1 (the
//!   translator hardcodes `@group(0)` per stage; two stages in one
//!   pipeline need separate groups).
//! * [`uniform_row_bytes`] / [`matrix_rows`] — D3D9 c# row packing.
//!
//! The wgpu-typed half (bind-group entries, vertex buffer layouts,
//! pipeline + renderer) stays in `crate::wgpu_backend`, which re-exports
//! this module's items for path compatibility.
//!
//! # Evidence
//!
//! * PROVEN: the binding arithmetic mirrors `shader::wgsl`'s emitted
//!   declarations (cross-checked per program by tests against the real
//!   shipped shaders + the shaders.rcf member) — and after the split
//!   those tests run in DEFAULT builds, not only under `render-wgpu`.
//! * PROVEN: uniform rows are 16-byte vec4 registers (D3D9 c# file
//!   semantics, proven in `sm30`'s CTAB analysis).
//! * LIKELY: group assignment (vs=0, fs=1) — we deliberately renumber
//!   instead of guessing the original engine's slot layout (UNKNOWN).

use std::collections::BTreeMap;

use super::sm30::Sm30Shader;
use super::wgsl::sanitize;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Device-free planning errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// The binding plan disagreed with the emitted WGSL — a bug in the
    /// mirror arithmetic, never ship a pipeline in this state.
    Mismatch(String),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::Mismatch(e) => write!(f, "binding plan mismatch: {e}"),
        }
    }
}

impl std::error::Error for PlanError {}

// ---------------------------------------------------------------------------
// Stage resource plan (mirror of shader::wgsl's binding arithmetic)
// ---------------------------------------------------------------------------

/// One declared resource binding of one shader stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageBinding {
    /// One 16-byte uniform row: `var<uniform> {wgsl_name}: vec4<f32>`.
    Uniform {
        /// The c# register this row backs.
        register: u16,
        /// The exact WGSL variable name.
        wgsl_name: String,
        /// Binding number within the stage's bind group.
        binding: u32,
    },
    /// One 16-byte uniform row backing a D3D9 BOOL-constant register
    /// `b#` (block 34).  GPU-identical to [`StageBinding::Uniform`] (a
    /// 16-byte uniform buffer; the WGSL tests `bN.x != 0.0`) but typed
    /// apart so the write path can address `c0` vs `b0` unambiguously —
    /// they are distinct registers that can share an index.
    BoolUniform {
        /// The b# register this row backs.
        register: u16,
        /// The exact WGSL variable name (`b{n}`).
        wgsl_name: String,
        /// Binding number within the stage's bind group.
        binding: u32,
    },
    /// `texture_2d<f32>` for a sampler register s#.
    Texture {
        /// The s# register.
        register: u16,
        /// The exact WGSL variable name (`{sampler}_tex`).
        wgsl_name: String,
        /// Binding number.
        binding: u32,
    },
    /// `sampler` for a sampler register s#.
    Sampler {
        /// The s# register.
        register: u16,
        /// The exact WGSL variable name (`{sampler}_smp`).
        wgsl_name: String,
        /// Binding number.
        binding: u32,
    },
}

impl StageBinding {
    /// The binding number (all variants carry one).
    pub fn binding(&self) -> u32 {
        match self {
            StageBinding::Uniform { binding, .. }
            | StageBinding::BoolUniform { binding, .. }
            | StageBinding::Texture { binding, .. }
            | StageBinding::Sampler { binding, .. } => *binding,
        }
    }

    /// The D3D9 register this binding backs: `c#`/`b#` for the uniform
    /// rows, `s#` for textures/samplers (the number the translated
    /// WGSL's constant tables were built from — block 76's harness
    /// addresses uniform buffers per row by this number).
    pub fn register(&self) -> u16 {
        match self {
            StageBinding::Uniform { register, .. }
            | StageBinding::BoolUniform { register, .. }
            | StageBinding::Texture { register, .. }
            | StageBinding::Sampler { register, .. } => *register,
        }
    }

    /// True for the two uniform-row variants (float + bool).
    pub fn is_uniform_row(&self) -> bool {
        matches!(self, StageBinding::Uniform { .. } | StageBinding::BoolUniform { .. })
    }
}

/// The complete per-stage binding plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageResources {
    /// Bindings in binding-number order.
    pub bindings: Vec<StageBinding>,
}

/// Derive the binding plan for one shader stage, using EXACTLY the
/// arithmetic of `shader::wgsl`'s globals emitter:
///
/// 1. uniform parameter records (type_id != 6) expand to one entry per
///    register row; multi-row uniforms get `{name}_r{reg}` names;
/// 2. `def`'d registers are baked constants — removed from uniforms;
/// 3. sampler records (type_id == 6) become texture+sampler pairs;
/// 4. bindings number ascending: all uniform rows first (by register),
///    then sampler pairs (by register).
pub fn plan_stage_resources(prog: &Sm30Shader) -> StageResources {
    let mut uniforms: BTreeMap<u16, String> = BTreeMap::new();
    let mut samplers: BTreeMap<u16, String> = BTreeMap::new();
    for p in &prog.params {
        if p.type_id == 6 {
            samplers.insert(p.register_index, sanitize(&p.name));
        } else {
            for r in 0..p.register_count as u16 {
                let reg = p.register_index + r;
                let name = if p.register_count > 1 {
                    format!("{}_r{}", sanitize(&p.name), reg)
                } else {
                    sanitize(&p.name)
                };
                uniforms.insert(reg, name);
            }
        }
    }
    for (reg, _) in &prog.defs {
        uniforms.remove(reg);
    }

    let mut bindings = Vec::new();
    let mut next = 0u32;
    for (reg, name) in &uniforms {
        bindings.push(StageBinding::Uniform {
            register: *reg,
            wgsl_name: name.clone(),
            binding: next,
        });
        next += 1;
    }
    for (reg, name) in &samplers {
        bindings.push(StageBinding::Texture {
            register: *reg,
            wgsl_name: format!("{name}_tex"),
            binding: next,
        });
        next += 1;
        bindings.push(StageBinding::Sampler {
            register: *reg,
            wgsl_name: format!("{name}_smp"),
            binding: next,
        });
        next += 1;
    }
    StageResources { bindings }
}

impl StageResources {
    /// Uniform-row bindings only (float `c#` rows; bools are separate —
    /// see [`StageResources::bool_uniforms`]).
    pub fn uniforms(&self) -> impl Iterator<Item = &StageBinding> {
        self.bindings
            .iter()
            .filter(|b| matches!(b, StageBinding::Uniform { .. }))
    }

    /// Bool-row bindings (`b#` registers, block 34).
    pub fn bool_uniforms(&self) -> impl Iterator<Item = &StageBinding> {
        self.bindings
            .iter()
            .filter(|b| matches!(b, StageBinding::BoolUniform { .. }))
    }

    /// Number of float uniform rows.
    pub fn uniform_count(&self) -> usize {
        self.bindings
            .iter()
            .filter(|b| matches!(b, StageBinding::Uniform { .. }))
            .count()
    }

    /// Number of bool uniform rows.
    pub fn bool_count(&self) -> usize {
        self.bindings
            .iter()
            .filter(|b| matches!(b, StageBinding::BoolUniform { .. }))
            .count()
    }

    /// Number of sampler PAIRS.
    pub fn sampler_count(&self) -> usize {
        self.bindings
            .iter()
            .filter(|b| matches!(b, StageBinding::Texture { .. }))
            .count()
    }
}

/// The expected WGSL declaration for one binding (for cross-checking).
///
/// Float and bool rows emit the IDENTICAL uniform declaration text
/// (only the trailing comment differs, which is not part of the
/// matched prefix) — the check below intentionally accepts both.
fn expected_declaration(b: &StageBinding) -> String {
    match b {
        StageBinding::Uniform { wgsl_name, binding, .. }
        | StageBinding::BoolUniform { wgsl_name, binding, .. } => format!(
            "@binding({binding}) var<uniform> {wgsl_name}: vec4<f32>;"
        ),
        StageBinding::Texture { wgsl_name, binding, .. } => {
            format!("@binding({binding}) var {wgsl_name}: texture_2d<f32>;")
        }
        StageBinding::Sampler { wgsl_name, binding, .. } => {
            format!("@binding({binding}) var {wgsl_name}: sampler;")
        }
    }
}

/// Cross-check a plan against emitted WGSL text.
///
/// Every planned binding must appear verbatim, and every `@binding(`
/// occurrence in the WGSL must be accounted for by the plan — the two
/// derivations (bytecode-level plan vs text emitter) must agree exactly.
pub fn validate_plan_matches_wgsl(res: &StageResources, wgsl: &str) -> Result<(), PlanError> {
    for b in &res.bindings {
        let want = expected_declaration(b);
        if !wgsl.contains(&want) {
            return Err(PlanError::Mismatch(format!(
                "expected declaration not found in WGSL: {want}"
            )));
        }
    }
    let declared = wgsl.matches("@binding(").count();
    if declared != res.bindings.len() {
        return Err(PlanError::Mismatch(format!(
            "WGSL declares {declared} binding(s), plan has {}",
            res.bindings.len()
        )));
    }
    Ok(())
}

/// Move a stage's WGSL resources from `@group(0)` to `@group(new_group)`.
///
/// The translator hardcodes `@group(0)` on every resource global; when a
/// vertex and fragment stage share one pipeline, the fragment's resources
/// move to their own bind group (group 1) so both stages' `@binding`
/// numbers stay untouched.  Mechanical, auditable, and the validation
/// above runs AFTER the rewrite so nothing can drift.
pub fn renumber_group(wgsl: &str, new_group: u32) -> String {
    if new_group == 0 {
        return wgsl.to_owned();
    }
    wgsl.replace("@group(0)", &format!("@group({new_group})"))
}

// ---------------------------------------------------------------------------
// Per-draw value rows (block 34 — the world-pipeline uniform encoding)
// ---------------------------------------------------------------------------

/// The per-draw uniform values of one shader stage: `c#` float rows and
/// `b#` bool rows keyed by register, with a drift guard — encoding
/// FAILS when a value names a register the stage does not declare.
/// Device-free; the GPU edge uploads the encoded rows verbatim.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StageValues {
    floats: BTreeMap<u16, [f32; 4]>,
    bools: BTreeMap<u16, bool>,
}

impl StageValues {
    /// Set one float row.
    pub fn set_float(&mut self, register: u16, row: [f32; 4]) {
        self.floats.insert(register, row);
    }

    /// Set one bool row (the WGSL tests `bN.x != 0.0`, so `true` encodes
    /// as `[1, 0, 0, 0]`).
    pub fn set_bool(&mut self, register: u16, value: bool) {
        self.bools.insert(register, value);
    }

    /// The float rows (register order).
    pub fn floats(&self) -> impl Iterator<Item = (u16, [f32; 4])> + '_ {
        self.floats.iter().map(|(r, v)| (*r, *v))
    }

    /// The bool rows (register order).
    pub fn bools(&self) -> impl Iterator<Item = (u16, bool)> + '_ {
        self.bools.iter().map(|(r, v)| (*r, *v))
    }

    /// Encode every uniform row (float + bool) the plan declares, in
    /// plan order: `(wgsl_name, 16 bytes)`.
    ///
    /// Rows never set encode as ZEROS (a declared-but-unset engine
    /// uniform is a legitimate default — e.g. the shadow rows of an
    /// unshadowed draw).  A value whose register the plan does NOT
    /// declare is a caller/plan disagreement — hard error, never
    /// silently dropped.
    pub fn encode(
        &self,
        res: &StageResources,
    ) -> Result<Vec<(String, [u8; 16])>, PlanError> {
        for r in self.floats.keys() {
            if !res.bindings.iter().any(|b| {
                matches!(b, StageBinding::Uniform { register, .. } if register == r)
            }) {
                return Err(PlanError::Mismatch(format!(
                    "value set for c{r} but the stage plan has no such float row"
                )));
            }
        }
        for r in self.bools.keys() {
            if !res.bindings.iter().any(|b| {
                matches!(b, StageBinding::BoolUniform { register, .. } if register == r)
            }) {
                return Err(PlanError::Mismatch(format!(
                    "bool set for b{r} but the stage plan has no such bool row"
                )));
            }
        }
        Ok(res
            .bindings
            .iter()
            .filter_map(|b| match b {
                StageBinding::Uniform { register, wgsl_name, .. } => {
                    let row = self.floats.get(register).copied().unwrap_or([0.0; 4]);
                    Some((wgsl_name.clone(), uniform_row_bytes(row)))
                }
                StageBinding::BoolUniform { register, wgsl_name, .. } => {
                    let on = self.bools.get(register).copied().unwrap_or(false);
                    let row: [f32; 4] = if on { [1.0, 0.0, 0.0, 0.0] } else { [0.0; 4] };
                    Some((wgsl_name.clone(), uniform_row_bytes(row)))
                }
                _ => None,
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Uniform data packing (D3D9 c# rows == 16-byte vec4 registers)
// ---------------------------------------------------------------------------

/// Pack one uniform row (one c# register) into its 16-byte UBO image.
///
/// f32 little-endian — every wgpu target we ship (Android arm64/armv7/
/// x86_64, desktop x86_64) is little-endian; wgpu itself treats buffer
/// f32s as native-endian.
pub fn uniform_row_bytes(row: [f32; 4]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (i, v) in row.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

/// Pack a column-major 4x4 matrix (e.g. `prototype_math::Mat4 { cols }`)
/// into its four 16-byte uniform rows, row 0 first.
pub fn matrix_rows(m: &[[f32; 4]; 4]) -> [[u8; 16]; 4] {
    let mut out = [[0u8; 16]; 4];
    for (i, col) in m.iter().enumerate() {
        out[i] = uniform_row_bytes(*col);
    }
    out
}

// ---------------------------------------------------------------------------
// Tests — the device-free cross-checks run in EVERY build (they used to
// live under the feature-gated wgpu_backend; the split moves them here so
// default `cargo test` keeps proving the plan/WGSL agreement).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shader::sm30::Sm30Shader;
    use crate::shader::wgsl::translate_program;
    use prototype_p3d::P3dFile;
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

    /// All real shader programs (frontendshaders.p3d + shaders.rcf member).
    fn all_programs() -> Vec<(String, bool, Vec<u8>)> {
        let data = fixture("frontendshaders.p3d");
        let f = P3dFile::parse(&data).unwrap();
        let mut v: Vec<(String, bool, Vec<u8>)> = f
            .shader_programs()
            .unwrap()
            .into_iter()
            .map(|p| (p.name.clone(), p.is_fragment, p.d3d9_tokens.to_vec()))
            .collect();
        let rcf = fixture("shaders.rcf");
        let arc = prototype_rcf::RcfArchive::parse(&rcf).unwrap();
        for e in &arc.entries {
            let payload = arc.payload(e).unwrap();
            if payload.starts_with(b"P3D\xff") {
                let f = P3dFile::parse(payload).unwrap();
                for p in f.shader_programs().unwrap() {
                    v.push((p.name.clone(), p.is_fragment, p.d3d9_tokens.to_vec()));
                }
            }
        }
        v
    }

    #[test]
    fn plans_match_emitted_wgsl_for_every_real_shader() {
        // THE cross-check: for all real programs, the plan derived from
        // decoded bytecode must exactly match the WGSL the translator
        // emits.  Any drift between the two derivations fails here.
        let programs = all_programs();
        assert!(programs.len() >= 26, "expected the full shipped set");
        let mut checked = 0usize;
        for (name, is_fragment, tokens) in &programs {
            let s = Sm30Shader::parse(tokens, *is_fragment).unwrap();
            let res = plan_stage_resources(&s);
            let wgsl = translate_program(&s, name).unwrap();
            validate_plan_matches_wgsl(&res, &wgsl)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            checked += 1;
        }
        assert!(checked >= 26);
    }

    #[test]
    fn fs_renumbered_to_group1_still_validates() {
        let programs = all_programs();
        for (name, is_fragment, tokens) in programs.iter().filter(|(_, f, _)| *f) {
            let s = Sm30Shader::parse(tokens, *is_fragment).unwrap();
            let res = plan_stage_resources(&s);
            let wgsl = translate_program(&s, name).unwrap();
            let g1 = renumber_group(&wgsl, 1);
            assert!(g1.contains("@group(1)"), "{name}: no group(1) after rewrite");
            assert!(!g1.contains("@group(0)"), "{name}: group(0) survived rewrite");
            validate_plan_matches_wgsl(&res, &g1)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }

    #[test]
    fn renumber_group_zero_is_identity() {
        let wgsl = "@group(0) @binding(0) var<uniform> x: vec4<f32>;";
        assert_eq!(renumber_group(wgsl, 0), wgsl);
    }

    #[test]
    fn uniform_rows_are_16_byte_le_vec4s() {
        assert_eq!(
            uniform_row_bytes([1.0, 2.0, 3.0, 4.0]),
            {
                let mut b = [0u8; 16];
                b[0..4].copy_from_slice(&1.0f32.to_le_bytes());
                b[4..8].copy_from_slice(&2.0f32.to_le_bytes());
                b[8..12].copy_from_slice(&3.0f32.to_le_bytes());
                b[12..16].copy_from_slice(&4.0f32.to_le_bytes());
                b
            }
        );
        let m = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
        let rows = matrix_rows(&m);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[3], uniform_row_bytes([0.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn def_registers_are_baked_not_bound() {
        // Some shipped shaders def constant registers; the plan must NOT
        // allocate bindings for them (the WGSL has no declarations for
        // them either — the cross-check test already proves the pair
        // agrees; this test documents the rule explicitly).
        let programs = all_programs();
        let mut found_def = false;
        for (name, is_fragment, tokens) in &programs {
            let s = Sm30Shader::parse(tokens, *is_fragment).unwrap();
            if !s.defs.is_empty() {
                found_def = true;
                let res = plan_stage_resources(&s);
                for (reg, _) in &s.defs {
                    assert!(
                        res.uniforms().all(|b| match b {
                            StageBinding::Uniform { register, .. } => register != reg,
                            _ => true,
                        }),
                        "{name}: def'd c{reg} must not be a uniform binding"
                    );
                }
            }
        }
        assert!(found_def, "expected at least one shipped shader with def constants");
    }

    #[test]
    fn plan_mismatch_is_detectable() {
        // A plan whose WGSL is missing a declaration must fail, and the
        // failure must name the missing declaration.
        let wgsl = "@group(0) @binding(0) var<uniform> other_name: vec4<f32>;";
        let res = StageResources {
            bindings: vec![StageBinding::Uniform {
                register: 0,
                wgsl_name: "expected_name".into(),
                binding: 0,
            }],
        };
        let err = validate_plan_matches_wgsl(&res, wgsl).unwrap_err();
        assert!(format!("{err}").contains("expected_name"));
    }

    #[test]
    fn multi_row_uniforms_expand_per_register_row() {
        // A 2-row uniform (register_count 2) must yield two bindings with
        // the `_r{reg}` suffix scheme, in ascending register order.
        // Built from a REAL program (an empty token stream does not
        // parse — by design): take the first shipped program, replace
        // its parameter records with one synthetic 2-row record.
        let programs = all_programs();
        let (_name, is_fragment, tokens) = programs.first().expect("at least one program");
        let mut prog = Sm30Shader::parse(tokens, *is_fragment).unwrap();
        prog.defs.clear();
        prog.params.clear();
        prog.params.push(crate::shader::sm30::ShaderParamRecord {
            name: "Two Rows".into(),
            uid: 29,
            type_id: 4, // Float4
            register_index: 10,
            register_sub: 0,
            register_flag: 0,
            magic: 0x0001_1115, // PARAM_MAGIC (private in sm30; the observed constant)
            pad: [16, 16],
            register_count: 2,
        });
        let res = plan_stage_resources(&prog);
        let regs: Vec<u16> = res.uniforms().filter_map(|b| match b {
            StageBinding::Uniform { register, .. } => Some(*register),
            _ => None,
        }).collect();
        assert_eq!(regs, vec![10, 11]);
        let names: Vec<&str> = res.uniforms().filter_map(|b| match b {
            StageBinding::Uniform { wgsl_name, .. } => Some(wgsl_name.as_str()),
            _ => None,
        }).collect();
        // sanitize maps the space to '_' (shader::wgsl::sanitize).
        assert_eq!(names, vec!["Two_Rows_r10", "Two_Rows_r11"]);
        // Bindings are dense from 0.
        assert_eq!(res.uniform_count(), 2);
    }
}

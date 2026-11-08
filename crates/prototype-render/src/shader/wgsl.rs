//! wgsl — clean-room translation of the decoded SM 3.0 instruction
//! stream into WGSL source.
//!
//! The translator executes the decoded instructions SYMBOLICALLY,
//! one float4 register at a time (per-component expression trees), and
//! emits one WGSL statement per masked component of every instruction,
//! preceded by the instruction's disassembly as a comment.  That makes
//! every emitted line checkable against the original bytecode.
//!
//! ## Coverage
//!
//! The shipped programs (26/26, see `sm30.rs`) use exactly ten
//! opcodes: mov/add/mad/mul/dp4/lrp/dcl/tex/def/cmp.  All ten are
//! translated.  The 45 DLL world-shader blobs (block 30/31) add fifteen
//! more — rcp/rsq/frc/abs/nrm/sincos/min/max/slt/pow/dp2add/if/else/
//! endif/texkill — ALL translated since block 31: the six committed
//! blob fixtures (error_vp, unlit_fp, lightcount_vp, bink_fp,
//! pointsprite_fp, default_fp) plus default_vp, shadowdebug_fp and
//! shadowblur_fp translate END TO END, including the flagship world
//! fragment shader `default_fp` (3× nested if/else/endif, 2× texkill,
//! `_dw` projective divides, pow/nrm/rsq).
//!
//! ## Flow control is FLATTENED, not emitted
//!
//! `if b# / else / endif` never becomes WGSL control flow.  Both arms
//! execute symbolically and every register that ends up with diverging
//! values is re-joined with `select(else_value, if_value, <cond>)` —
//! exactly D3D9's branch-state semantics.  Consequences: every
//! `textureSample` stays outside control flow (uniformity-legal in
//! WGSL); both arms' math executes at runtime (the observed else arms
//! are tiny; the inner shadow branch of default_fp costs extra samples
//! — a documented trade-off until a block emits real control flow);
//! `texkill` becomes one guarded `discard` over branch-gated
//! accumulated conditions.
//!
//! ## What is PROVEN vs LIKELY
//!
//! * PROVEN: the arithmetic — each WGSL statement is a mechanical
//!   rendering of a decoded instruction (swizzles, masks, modifiers).
//! * PROVEN: uniform names come from the CTAB / post-END parameter
//!   records (the two agree — cross-checked).
//! * PROVEN: `if` conditions are boolean-constant registers (`b#`,
//!   register type 14, 6/6 census); the flattened merge
//!   `select(else, if, cond)` mirrors D3D9 branch-state semantics.
//! * LIKELY: the b# values themselves are engine feature flags set per
//!   draw (`SetPixelShaderConstantB` — the item-7(d) family-code
//!   hypothesis); the translator models them as `b{n}` vec4<f32>
//!   uniforms tested with `b{n}.x != 0.0`.
//! * LIKELY: `_dw`/`_dz` read the register's physical w/z lane — the
//!   7 observed cases (5 identity-swizzle, 1 `.yyyy`, 1 `.wzzw` with
//!   lane 3 = w) coincide under every reading.
//! * LIKELY: `@location` / `@binding` numbering.  We use one
//!   deterministic scheme shared by both stages of a pair so the
//!   outputs of the vertex stage always line up with the inputs of the
//!   fragment stage: `texcoord{i}` → location `i`, `color{i}` →
//!   location `8+i`; uniforms bind in register order (then the b#
//!   flags, then one texture+sampler binding pair per sampler).  The
//!   original D3D9 slot layout is UNKNOWN.
//! * DEF'd registers (`def c4, 1, 0, 0, 0` — e.g. `p3dBuiltIn_One`)
//!   are compile-time constants baked into the bytecode; they get no
//!   uniform declaration and resolve to their literal values.  Blob
//!   constants read but absent from parameter records (blobs carry
//!   none) get deterministic `u_c{n}` uniforms.
//! * DETERMINISTIC DEVIATION: a register written in only one arm of a
//!   branch and never before is undefined in D3D9 in the other arm;
//!   the merge takes the written value unconditionally (real shaders
//!   never read such a register in the un-taken arm).

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;

use super::sm30::{
    Instruction, ParamToken, RegType, Sm30Op, Sm30Shader, SrcMod, Usage,
};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Translation failures.  All of them indicate bytecode shapes outside
/// the observed shipped set — the translator refuses to guess.
#[derive(Debug, Clone, PartialEq)]
pub enum TranslateError {
    Sm30(super::sm30::Sm30Error),
    /// A source operand carries a modifier we have never observed.
    UnsupportedSrcMod(SrcMod),
    /// A register was read before being written (temp) or declared.
    UninitializedRead { reg: String },
    /// An output component was never written.
    IncompleteOutput { reg: String },
    /// A vertex program without a `dcl_position o#` output.
    NoPositionOutput,
    /// A register of a type the translator cannot place.
    UnsupportedRegister { reg: String },
    /// An instruction the translator has no rule for — the DLL-blob
    /// world-shader opcodes (rcp/rsq/min/max/slt/frc/pow/abs/nrm/
    /// sincos/if/else/endif/texkill/dp2add) and any future addition.
    /// The 26 shipped programs never hit this (fail-closed, never a
    /// panic).
    UnsupportedInstruction { desc: String },
}

impl fmt::Display for TranslateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TranslateError::Sm30(e) => write!(f, "sm30: {e}"),
            TranslateError::UnsupportedSrcMod(m) => {
                write!(f, "unsupported source modifier {m} (only none/- observed)")
            }
            TranslateError::UninitializedRead { reg } => {
                write!(f, "read of uninitialized register {reg}")
            }
            TranslateError::IncompleteOutput { reg } => {
                write!(f, "output register {reg} has unwritten components")
            }
            TranslateError::NoPositionOutput => {
                f.write_str("vertex program lacks a dcl_position output")
            }
            TranslateError::UnsupportedRegister { reg } => {
                write!(f, "unsupported register {reg}")
            }
            TranslateError::UnsupportedInstruction { desc } => {
                write!(f, "instruction not translatable (DLL-blob world shader): {desc}")
            }
        }
    }
}

impl std::error::Error for TranslateError {}

impl From<super::sm30::Sm30Error> for TranslateError {
    fn from(e: super::sm30::Sm30Error) -> Self {
        TranslateError::Sm30(e)
    }
}

// ---------------------------------------------------------------------------
// Symbolic values
// ---------------------------------------------------------------------------

const COMP: [char; 4] = ['x', 'y', 'z', 'w'];

/// A scalar expression.
#[derive(Debug, Clone, PartialEq)]
enum Scalar {
    /// A literal.
    Con(f32),
    /// A named expression (an input component, a uniform component, a
    /// texture-sample component, ...), emitted verbatim.
    Term(String),
    Add(Box<Scalar>, Box<Scalar>),
    /// Subtraction — reserved for source modifiers outside the
    /// observed set (never constructed today; kept for model
    /// completeness alongside [`Scalar::Neg`]).
    #[allow(dead_code)]
    Sub(Box<Scalar>, Box<Scalar>),
    Mul(Box<Scalar>, Box<Scalar>),
    /// Division — `rcp` (`1.0 / x`) and the `_dz`/`_dw` source
    /// modifiers (`x / z` resp. `x / w`).
    Div(Box<Scalar>, Box<Scalar>),
    /// A WGSL builtin call over one scalar — `sin`/`cos` (sincos),
    /// `fract` (frc), `abs` (abs, `_abs`), `inverseSqrt` (rsq).
    Fn(String, Box<Scalar>),
    /// A WGSL builtin call over two scalars — `min`/`max`/`pow`.
    Fn2(String, Box<Scalar>, Box<Scalar>),
    /// WGSL `mix(b, c, a)` = `a*b + (1-a)*c` = D3D `lrp dst, a, b, c`.
    Mix(Box<Scalar>, Box<Scalar>, Box<Scalar>),
    /// WGSL `select(c, b, a >= 0.0)` = D3D `cmp dst, a, b, c`.
    Select(Box<Scalar>, Box<Scalar>, Box<Scalar>),
    /// WGSL `select(0.0, 1.0, a < b)` = D3D `slt dst, a, b`.
    Slt(Box<Scalar>, Box<Scalar>),
    /// D3D `dp4` over two vec4s, emitted as
    /// `dot(vec4<f32>(...), vec4<f32>(...))`.
    Dot(Box<[Scalar; 4]>, Box<[Scalar; 4]>),
    /// D3D `dp3` over the xyz of two vec4s, emitted as
    /// `dot(vec3<f32>(...), vec3<f32>(...))` (bink shaders).
    Dot3(Box<[Scalar; 3]>, Box<[Scalar; 3]>),
    /// D3D `dp2add dst, a, b, c`: `dot(vec2(a.x, a.y), vec2(b.x, b.y))`
    /// (the addend arrives through [`Scalar::Add`]).
    Dot2(Box<[Scalar; 2]>, Box<[Scalar; 2]>),
    /// A value produced by a flattened `if/else` merge:
    /// `select(else_expr, if_expr, <cond>)` where `<cond>` is a WGSL
    /// boolean expression over the branch's boolean-constant uniform
    /// (e.g. `b0.x != 0.0`).
    ///
    /// Constructed only on the dynamic branch path; the corpus's
    /// committed programs all flatten through the other variants
    /// today, so the dead-code lint is silenced explicitly (the
    /// variant stays — it models the observed `if_*` instructions).
    #[allow(dead_code)]
    Branch {
        cond: String,
        if_true: Box<Scalar>,
        if_false: Box<Scalar>,
    },
    /// Numeric negation (the only observed source modifier).
    Neg(Box<Scalar>),
}

impl Scalar {
    fn write(&self, out: &mut String) {
        match self {
            Scalar::Con(v) => {
                if v.fract() == 0.0 && v.abs() < 1e9 {
                    out.push_str(&format!("{v:.1}"));
                } else {
                    out.push_str(&format!("{v}"));
                }
            }
            Scalar::Term(t) => out.push_str(t),
            Scalar::Add(a, b) => {
                out.push('(');
                a.write(out);
                out.push_str(" + ");
                b.write(out);
                out.push(')');
            }
            Scalar::Sub(a, b) => {
                out.push('(');
                a.write(out);
                out.push_str(" - ");
                b.write(out);
                out.push(')');
            }
            Scalar::Mul(a, b) => {
                out.push('(');
                a.write(out);
                out.push_str(" * ");
                b.write(out);
                out.push(')');
            }
            Scalar::Div(a, b) => {
                out.push('(');
                a.write(out);
                out.push_str(" / ");
                b.write(out);
                out.push(')');
            }
            Scalar::Fn(name, a) => {
                out.push_str(name);
                out.push('(');
                a.write(out);
                out.push(')');
            }
            Scalar::Fn2(name, a, b) => {
                out.push_str(name);
                out.push('(');
                a.write(out);
                out.push_str(", ");
                b.write(out);
                out.push(')');
            }
            Scalar::Mix(a, b, c) => {
                out.push_str("mix(");
                b.write(out);
                out.push_str(", ");
                c.write(out);
                out.push_str(", ");
                a.write(out);
                out.push(')');
            }
            Scalar::Select(a, b, c) => {
                out.push_str("select(");
                c.write(out);
                out.push_str(", ");
                b.write(out);
                out.push_str(", ");
                a.write(out);
                out.push_str(" >= 0.0)");
            }
            Scalar::Slt(a, b) => {
                out.push_str("select(0.0, 1.0, (");
                a.write(out);
                out.push_str(") < (");
                b.write(out);
                out.push_str("))");
            }
            Scalar::Dot2(a, b) => {
                out.push_str("dot(vec2<f32>(");
                for (i, s) in a.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    s.write(out);
                }
                out.push_str("), vec2<f32>(");
                for (i, s) in b.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    s.write(out);
                }
                out.push_str("))");
            }
            Scalar::Branch {
                cond,
                if_true,
                if_false,
            } => {
                out.push_str("select(");
                if_false.write(out);
                out.push_str(", ");
                if_true.write(out);
                out.push_str(", ");
                out.push_str(cond);
                out.push(')');
            }
            Scalar::Dot(a, b) => {
                out.push_str("dot(vec4<f32>(");
                for (i, s) in a.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    s.write(out);
                }
                out.push_str("), vec4<f32>(");
                for (i, s) in b.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    s.write(out);
                }
                out.push_str("))");
            }
            Scalar::Dot3(a, b) => {
                out.push_str("dot(vec3<f32>(");
                for (i, s) in a.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    s.write(out);
                }
                out.push_str("), vec3<f32>(");
                for (i, s) in b.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    s.write(out);
                }
                out.push_str("))");
            }
            Scalar::Neg(a) => {
                // Neg over a literal folds to the negated literal — a
                // naive "(-" + "-2.0" + ")" would emit the INVALID WGSL
                // `(--2.0)` (63 occurrences in default_fp otherwise).
                // The literal can arrive as `Term("-2.0")` too: def'd
                // (baked) constant registers are rendered as TEXT by
                // `uniform_comp` (block-36 naga harness found this — the
                // neutral-map decode `mad r5, nm, -c21.x, -c21.y` reads
                // def'd constants through SrcMod::Neg).
                let numeric: Option<f32> = match a.as_ref() {
                    Scalar::Con(v) => Some(*v),
                    Scalar::Term(t) => t.parse::<f32>().ok(),
                    _ => None,
                };
                if let Some(v) = numeric {
                    out.push_str(&Scalar::Con(-v).render());
                } else {
                    out.push_str("(-");
                    a.write(out);
                    out.push(')');
                }
            }
        }
    }

    fn render(&self) -> String {
        let mut s = String::new();
        self.write(&mut s);
        s
    }
}

/// A register value: four component expressions, or a texture-sample
/// call (whose components are selected with `.x` .. `.w`).
#[derive(Debug, Clone, PartialEq)]
enum Val {
    V([Scalar; 4]),
    /// A `textureSample(...)` call; component i renders as `<call>.<xyzw>`.
    Call(String),
}

fn comps_of(v: &Val) -> [Scalar; 4] {
    match v {
        Val::V(c) => c.clone(),
        Val::Call(call) => std::array::from_fn(|i| Scalar::Term(format!("{call}.{}", COMP[i]))),
    }
}

// ---------------------------------------------------------------------------
// Context: register naming
// ---------------------------------------------------------------------------

/// Deterministic location for a (usage, index) interpolant, shared by
/// the two stages of a shader pair so they always link.  LIKELY
/// mapping (see module docs).
/// The `@location` of a vertex-stage INPUT for usage `Position`.
///
/// RESOLVED (block 34): this was 0 — colliding with `texcoord0` (also
/// 0), which made every vertex shader declaring position + texcoord0
/// (including the DLL `default` world pair) un-compilable: two `VsIn`
/// members with the same `@location` is a WGSL module error, and two
/// vertex attributes sharing a `shader_location` is a wgpu layout
/// error.  Position now gets a dedicated slot outside every
/// interpolant range (texcoords 0..7, colors 8..15, normals/other
/// 16..31).  The VS OUTPUT position is unaffected — it is emitted as
/// `@builtin(position)` before any location lookup.
pub const VERTEX_POSITION_LOCATION: usize = 32;

pub fn interpolant_location(usage: &Usage, index: u8) -> usize {
    match usage {
        Usage::TexCoord => index as usize,
        Usage::Color => 8 + index as usize,
        Usage::Position => VERTEX_POSITION_LOCATION,
        // DLL-blob-only usages (normal vertex inputs, pointsprite size
        // output): placed in the Other bucket so the mapping stays
        // total.  Such programs are not translatable today anyway —
        // translate_program fails on their first blob-only instruction.
        Usage::Normal => 16 + 3,
        Usage::PSize => 16 + 4,
        Usage::Other(u) => 16 + *u as usize,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct RegKey(u8, u16);

const KIND_TEMP: u8 = 0;
const KIND_INPUT: u8 = 1;
const KIND_CONST: u8 = 2;
const KIND_OUTPUT: u8 = 3;
const KIND_COLOROUT: u8 = 4;
const KIND_DEPTHOUT: u8 = 5;

fn key(t: &ParamToken) -> RegKey {
    let k = match t.ty {
        RegType::Temp => KIND_TEMP,
        RegType::Input => KIND_INPUT,
        RegType::Const => KIND_CONST,
        RegType::Output => KIND_OUTPUT,
        RegType::ColorOut => KIND_COLOROUT,
        RegType::DepthOut => KIND_DEPTHOUT,
        _ => 7,
    };
    RegKey(k, t.reg)
}

/// Naming/def context resolved from the program's DCLs, parameter
/// records and DEF instructions.
struct Ctx {
    /// v# -> input member name (e.g. `position0`).
    inputs: BTreeMap<u16, String>,
    /// c# -> WGSL uniform var name (from parameter records).
    uniforms: BTreeMap<u16, String>,
    /// c# registers READ by the symbolic execution but absent from
    /// `uniforms` (the DLL blobs carry no parameter records) — they get
    /// deterministic `u_c{n}` uniform declarations.
    used_consts: BTreeMap<u16, String>,
    /// b# boolean-constant registers used as `if` conditions (the DLL
    /// world-shader feature flags) — uniform-backed.
    bools: BTreeMap<u16, String>,
    /// def'd constant registers (baked-in values, no uniform).
    defs: BTreeMap<u16, [f32; 4]>,
    /// s# -> sanitized sampler name.
    samplers: BTreeMap<u16, String>,
}

pub(crate) fn sanitize(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    if s.is_empty() || s.chars().next().unwrap().is_ascii_digit() {
        s.insert(0, 'p');
    }
    s
}

fn usage_member(usage: &Usage, index: u8) -> String {
    format!("{usage}{index}")
}

impl Ctx {
    fn uniform_comp(&self, reg: u16, i: usize) -> String {
        if let Some(vals) = self.defs.get(&reg) {
            return Scalar::Con(vals[i]).render();
        }
        match self.uniforms.get(&reg) {
            Some(name) => format!("{name}.{}", COMP[i]),
            None => format!("u_c{reg}.{}", COMP[i]),
        }
    }

    /// The uniform variable name for constant register `reg`, or `None`
    /// when the register is def'd (baked in, no uniform needed).
    /// Side effect: unnamed registers are remembered in `used_consts`
    /// so the declaration pass emits a `u_c{n}` uniform for them.
    fn note_const_read(&mut self, reg: u16) -> Option<String> {
        if self.defs.contains_key(&reg) {
            return None;
        }
        let name = self
            .uniforms
            .get(&reg)
            .cloned()
            .unwrap_or_else(|| format!("u_c{reg}"));
        self.used_consts.entry(reg).or_insert_with(|| name.clone());
        Some(name)
    }

    /// The uniform variable name for a boolean-constant register `b#`
    /// (used as an `if` condition), remembering it for declaration.
    fn note_bool_read(&mut self, reg: u16) -> String {
        self.bools
            .entry(reg)
            .or_insert_with(|| format!("b{reg}"))
            .clone()
    }
}

// ---------------------------------------------------------------------------
// Symbolic machine
// ---------------------------------------------------------------------------

struct Machine {
    regs: BTreeMap<RegKey, Val>,
    /// (disasm, wgsl lines) pairs, in instruction order.
    stmts: Vec<(String, Vec<String>)>,
    /// Sequence counter for unique texture-sample `let` names (a
    /// sampler can be sampled many times, e.g. default_fp's five
    /// texld from s3).
    call_seq: usize,
    /// Accumulated kill (texkill) conditions — WGSL boolean
    /// expressions; emitted as one guarded `discard` at the end.
    kill: Vec<String>,
}

impl Machine {
    fn read(&mut self, t: &ParamToken, ctx: &mut Ctx) -> Result<[Scalar; 4], TranslateError> {
        let pre: [Scalar; 4] = match t.ty {
            RegType::Input => {
                let name = ctx.inputs.get(&t.reg).ok_or_else(|| {
                    TranslateError::UninitializedRead { reg: format!("v{}", t.reg) }
                })?;
                // NOTE (block-31 fix): the source swizzle applies to
                // inputs too — the blobs read inputs with real
                // swizzles (texld temp5, input7.xyzz, s2); the shipped
                // programs only ever used replicated swizzles whose
                // .x lane masked the bug in the pinned tests.
                std::array::from_fn(|i| {
                    Scalar::Term(format!("in.{}.{}", name, COMP[t.swizzle[i] as usize]))
                })
            }
            RegType::Const => {
                ctx.note_const_read(t.reg);
                std::array::from_fn(|i| Scalar::Term(ctx.uniform_comp(t.reg, t.swizzle[i] as usize)))
            }
            RegType::Temp | RegType::Output | RegType::ColorOut | RegType::DepthOut => {
                let v = self.regs.get(&key(t)).ok_or_else(|| {
                    TranslateError::UninitializedRead { reg: format!("{}{}", t.ty, t.reg) }
                })?;
                let c = comps_of(v);
                std::array::from_fn(|i| c[t.swizzle[i] as usize].clone())
            }
            other => {
                return Err(TranslateError::UnsupportedRegister {
                    reg: format!("{other}{}", t.reg),
                })
            }
        };
        // `pre` is the SWIZZLED read.  The Dz/Dw modifiers divide by the
        // source register's physical z/w lane, recovered here as
        // `pre[swizzle[2/3]]`: for the identity swizzles observed 6/7
        // (and the .wzzw const case, whose lane 3 is w) every reading
        // coincides; a hypothetical swizzle whose lane 3 is not w would
        // make the two readings diverge — none is observed in the
        // corpus (m35 census).
        match t.src_mod {
            SrcMod::None => Ok(pre),
            SrcMod::Neg => Ok(pre.map(|c| Scalar::Neg(Box::new(c)))),
            SrcMod::Dz => {
                let z = pre[t.swizzle[2] as usize].clone();
                Ok(pre.map(|c| Scalar::Div(Box::new(c), Box::new(z.clone()))))
            }
            SrcMod::Dw => {
                let w = pre[t.swizzle[3] as usize].clone();
                Ok(pre.map(|c| Scalar::Div(Box::new(c), Box::new(w.clone()))))
            }
            SrcMod::Abs => Ok(pre.map(|c| Scalar::Fn("abs".into(), Box::new(c)))),
            other => Err(TranslateError::UnsupportedSrcMod(other)),
        }
    }

    fn write(&mut self, dst: &ParamToken, value: Val) {
        let k = key(dst);
        let full = dst.writemask == [true, true, true, true];
        match value {
            Val::Call(_) if full => {
                self.regs.insert(k, value);
            }
            Val::Call(call) => {
                // Partial-mask call write: materialize per-component so
                // unwritten components keep their previous value.
                let old = self
                    .regs
                    .get(&k)
                    .cloned()
                    .unwrap_or(Val::V(std::array::from_fn(|_| Scalar::Con(0.0))));
                let oldc = comps_of(&old);
                let newc = std::array::from_fn(|i| {
                    if dst.writemask[i] {
                        Scalar::Term(format!("{call}.{}", COMP[i]))
                    } else {
                        oldc[i].clone()
                    }
                });
                self.regs.insert(k, Val::V(newc));
            }
            Val::V(newc) => {
                // Block-31 fix: a register holding a texture-sample
                // `Call` must be materialized to its per-component terms
                // first, or a masked write over it was SILENTLY DROPPED
                // (default_fp's `mov temp0.xyz` in the b0 else arm never
                // landed — both arms' rewrites of the sampled temp0 were
                // lost and the merge saw them as equal).
                if let Some(Val::Call(_)) = self.regs.get(&k) {
                    let c = comps_of(self.regs.get(&k).unwrap());
                    self.regs.insert(k, Val::V(c));
                }
                let entry = self.regs.entry(k).or_insert_with(|| {
                    Val::V(std::array::from_fn(|_| Scalar::Con(0.0)))
                });
                if let Val::V(cur) = entry {
                    for i in 0..4 {
                        if dst.writemask[i] {
                            cur[i] = newc[i].clone();
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Translation
// ---------------------------------------------------------------------------

/// Translate one decoded shader into WGSL source.
pub fn translate_program(prog: &Sm30Shader, name: &str) -> Result<String, TranslateError> {
    translate_program_with_plan(prog, name).map(|(wgsl, _)| wgsl)
}

/// Translate AND capture the exact binding plan the emitted WGSL
/// declares (block 34, NEXT-UP 7(e) device-free half).
///
/// The plan rows are pushed INSIDE the globals emitter below — the same
/// loop that numbers the `@binding`s — so the two derivations cannot
/// drift (the pre-existing bytecode-only mirror `plan::plan_stage_resources`
/// cannot see the DLL blobs' `u_c{n}` constant reads or their `b#`
/// feature-flag rows; this capture covers every program).
///
/// Float-constant and boolean rows both become 16-byte uniform buffers
/// on the GPU edge; booleans are typed [`StageBinding::BoolUniform`]
/// (register = the b# index) so the write path can address them
/// unambiguously — `c0` and `b0` are distinct registers.
pub fn translate_program_with_plan(
    prog: &Sm30Shader,
    name: &str,
) -> Result<(String, super::plan::StageResources), TranslateError> {
    let target = prog
        .ctab
        .as_ref()
        .map(|c| c.target.clone())
        .unwrap_or_default();

    // ---- context ------------------------------------------------------
    let mut ctx = Ctx {
        inputs: BTreeMap::new(),
        uniforms: BTreeMap::new(),
        used_consts: BTreeMap::new(),
        bools: BTreeMap::new(),
        defs: BTreeMap::new(),
        samplers: BTreeMap::new(),
    };
    for d in prog.dcls() {
        if let Instruction::Dcl { usage, usage_index, reg, .. } = d {
            match reg.ty {
                RegType::Input => {
                    ctx.inputs
                        .insert(reg.reg, usage_member(usage, *usage_index));
                }
                RegType::Output | RegType::ColorOut | RegType::Sampler => {}
                other => {
                    return Err(TranslateError::UnsupportedRegister {
                        reg: format!("{other}{}", reg.reg),
                    })
                }
            }
        }
    }
    for p in &prog.params {
        if p.type_id == 6 {
            ctx.samplers.insert(p.register_index, sanitize(&p.name));
        } else {
            for r in 0..p.register_count as u16 {
                let reg = p.register_index + r;
                let name = if p.register_count > 1 {
                    format!("{}_r{}", sanitize(&p.name), reg)
                } else {
                    sanitize(&p.name)
                };
                ctx.uniforms.insert(reg, name);
            }
        }
    }
    for (reg, vals) in &prog.defs {
        ctx.defs.insert(*reg, *vals);
        // A def'd register is baked in — no uniform declaration.
        ctx.uniforms.remove(reg);
    }

    // ---- symbolic execution -------------------------------------------
    //
    // Flow control (the DLL world shaders' `if b# / else / endif`) is
    // FLATTENED: both arms execute symbolically and every register that
    // ends up with diverging values is re-joined with
    // `select(else_value, if_value, <bool-const cond>)` — exactly the
    // D3D9 branch-state semantics (a register written in one arm holds
    // the taken arm's value afterwards, and the un-taken arm's value
    // defaults to the pre-if state).  Flattening keeps every
    // `textureSample` outside control flow (uniformity-legal in WGSL)
    // at the cost of executing both arms' math (the else arms here are
    // tiny; the observed inner shadow branch costs extra samples —
    // documented trade-off).
    let mut m = Machine {
        regs: BTreeMap::new(),
        stmts: Vec::new(),
        call_seq: 0,
        kill: Vec::new(),
    };
    struct FlowFrame {
        cond: String,
        preif: BTreeMap<RegKey, Val>,
        arm1: Option<BTreeMap<RegKey, Val>>,
    }
    let mut frames: Vec<FlowFrame> = Vec::new();
    for inst in &prog.instructions {
        let mut lines: Vec<String> = Vec::new();
        match inst {
            Instruction::Dcl { .. } => continue,
            Instruction::Def { .. } => {
                // Values were seeded into ctx.defs before execution.
                m.stmts.push((format!("{inst}"), vec!["// (def constant baked in)".to_string()]));
                continue;
            }
            Instruction::Op2 { op: Sm30Op::Mov, dst, a } => {
                let comps = m.read(a, &mut ctx)?;
                for i in 0..4 {
                    if dst.writemask[i] {
                        lines.push(format!(
                            "{} = {};",
                            lhs(dst, i),
                            comps[i].render()
                        ));
                    }
                }
                m.write(dst, Val::V(comps));
            }
            Instruction::Op3 { op, dst, a, b } => {
                match op {
                    Sm30Op::Tex => {
                        // dst, texcoord, sampler
                        // Register the fallback s# name so the
                        // declaration pass emits the texture+sampler
                        // pair (the blobs carry no parameter records).
                        let sname = ctx
                            .samplers
                            .entry(b.reg)
                            .or_insert_with(|| format!("s{}", b.reg))
                            .clone();
                        let tcv = m.read(a, &mut ctx)?;
                        let mut tc = String::from("vec4<f32>(");
                        for i in 0..4 {
                            if i > 0 {
                                tc.push_str(", ");
                            }
                            tcv[i].write(&mut tc);
                        }
                        tc.push(')');
                        let call = format!(
                            "textureSample({sname}_tex, {sname}_smp, {tc}.xy)"
                        );
                        // Unique let name — a sampler can be sampled
                        // many times (default_fp: five texld from s3).
                        let seq = m.call_seq;
                        m.call_seq += 1;
                        let let_name = if seq == 0 {
                            format!("s{}_sample", b.reg)
                        } else {
                            format!("s{}_sample_{}", b.reg, seq + 1)
                        };
                        lines.push(format!("let {let_name} = {call};"));
                        m.write(dst, Val::Call(let_name));
                    }
                    Sm30Op::Add | Sm30Op::Mul | Sm30Op::Dp3 | Sm30Op::Dp4 => {
                        let ac = m.read(a, &mut ctx)?;
                        let bc = m.read(b, &mut ctx)?;
                        let v: [Scalar; 4] = match op {
                            Sm30Op::Add => std::array::from_fn(|i| {
                                Scalar::Add(Box::new(ac[i].clone()), Box::new(bc[i].clone()))
                            }),
                            Sm30Op::Mul => std::array::from_fn(|i| {
                                Scalar::Mul(Box::new(ac[i].clone()), Box::new(bc[i].clone()))
                            }),
                            Sm30Op::Dp3 => std::array::from_fn(|_| Scalar::Dot3(
                                Box::new([ac[0].clone(), ac[1].clone(), ac[2].clone()]),
                                Box::new([bc[0].clone(), bc[1].clone(), bc[2].clone()]),
                            )),
                            _ => std::array::from_fn(|_| {
                                Scalar::Dot(
                                    Box::new([ac[0].clone(), ac[1].clone(), ac[2].clone(), ac[3].clone()]),
                                    Box::new([bc[0].clone(), bc[1].clone(), bc[2].clone(), bc[3].clone()]),
                                )
                            }),
                        };
                        for i in 0..4 {
                            if dst.writemask[i] {
                                lines.push(format!("{} = {};", lhs(dst, i), v[i].render()));
                            }
                        }
                        m.write(dst, Val::V(v));
                    }
                    Sm30Op::Min | Sm30Op::Max | Sm30Op::Slt | Sm30Op::Pow => {
                        let ac = m.read(a, &mut ctx)?;
                        let bc = m.read(b, &mut ctx)?;
                        let v: [Scalar; 4] = std::array::from_fn(|i| match op {
                            Sm30Op::Min => {
                                Scalar::Fn2("min".into(), Box::new(ac[i].clone()), Box::new(bc[i].clone()))
                            }
                            Sm30Op::Max => {
                                Scalar::Fn2("max".into(), Box::new(ac[i].clone()), Box::new(bc[i].clone()))
                            }
                            Sm30Op::Slt => {
                                Scalar::Slt(Box::new(ac[i].clone()), Box::new(bc[i].clone()))
                            }
                            _ => {
                                Scalar::Fn2("pow".into(), Box::new(ac[i].clone()), Box::new(bc[i].clone()))
                            }
                        });
                        for i in 0..4 {
                            if dst.writemask[i] {
                                lines.push(format!("{} = {};", lhs(dst, i), v[i].render()));
                            }
                        }
                        m.write(dst, Val::V(v));
                    }
                    _ => {
                        return Err(TranslateError::UnsupportedInstruction {
                            desc: format!("{op} (DLL-blob opcode)"),
                        })
                    }
                }
            }
            Instruction::Op4 { op, dst, a, b, c } => {
                let ac = m.read(a, &mut ctx)?;
                let bc = m.read(b, &mut ctx)?;
                let cc = m.read(c, &mut ctx)?;
                let v: [Scalar; 4] = match op {
                    Sm30Op::Mad => std::array::from_fn(|i| {
                        Scalar::Add(
                            Box::new(Scalar::Mul(Box::new(ac[i].clone()), Box::new(bc[i].clone()))),
                            Box::new(cc[i].clone()),
                        )
                    }),
                    Sm30Op::Lrp => std::array::from_fn(|i| {
                        Scalar::Mix(
                            Box::new(ac[i].clone()),
                            Box::new(bc[i].clone()),
                            Box::new(cc[i].clone()),
                        )
                    }),
                    Sm30Op::Cmp => std::array::from_fn(|i| {
                        Scalar::Select(
                            Box::new(ac[i].clone()),
                            Box::new(bc[i].clone()),
                            Box::new(cc[i].clone()),
                        )
                    }),
                    Sm30Op::Dp2add => {
                        // PROVEN signature (block 30, real instructions):
                        // dst = dot2(src0.xy, src1.xy) + src2 (replicated
                        // scalar — the observed src2 swizzles are .zzzz).
                        let d = Scalar::Dot2(
                            Box::new([ac[0].clone(), ac[1].clone()]),
                            Box::new([bc[0].clone(), bc[1].clone()]),
                        );
                        std::array::from_fn(|i| {
                            Scalar::Add(Box::new(d.clone()), Box::new(cc[i].clone()))
                        })
                    }
                    _ => {
                        return Err(TranslateError::UnsupportedInstruction {
                            desc: format!("{op} (DLL-blob opcode)"),
                        })
                    }
                };
                for i in 0..4 {
                    if dst.writemask[i] {
                        lines.push(format!("{} = {};", lhs(dst, i), v[i].render()));
                    }
                }
                m.write(dst, Val::V(v));
            }
            Instruction::Flow { op, reg } => match op {
                Sm30Op::If => {
                    // PROVEN (block 30/31): the blobs branch ONLY on
                    // boolean-constant registers (`if b0..b2`, register
                    // type 14, identity swizzle — 6/6 census).  The
                    // engine sets these per draw (feature flags — the
                    // item-7(d) hypothesis); here they become uniform-
                    // backed WGSL booleans, condition `bN.x != 0.0`.
                    let reg = reg.as_ref().ok_or_else(|| TranslateError::UnsupportedInstruction {
                        desc: "if without condition operand".into(),
                    })?;
                    if reg.ty != RegType::ConstBool {
                        return Err(TranslateError::UnsupportedInstruction {
                            desc: format!("if on non-boolean register {reg} (only b# observed)"),
                        });
                    }
                    let bname = ctx.note_bool_read(reg.reg);
                    frames.push(FlowFrame {
                        cond: format!("{bname}.x != 0.0"),
                        preif: m.regs.clone(),
                        arm1: None,
                    });
                }
                Sm30Op::Else => {
                    let frame = frames.last_mut().ok_or_else(|| {
                        TranslateError::UnsupportedInstruction { desc: "else without if".into() }
                    })?;
                    frame.arm1 = Some(m.regs.clone());
                    // The else arm starts from the PRE-IF state (D3D9
                    // branch semantics), not from the if arm's result.
                    m.regs = frame.preif.clone();
                }
                Sm30Op::Endif => {
                    let frame = frames.pop().ok_or_else(|| {
                        TranslateError::UnsupportedInstruction { desc: "endif without if".into() }
                    })?;
                    let (arm1, arm2) = match &frame.arm1 {
                        Some(_) => (frame.arm1.clone().unwrap_or_default(), m.regs.clone()),
                        None => (m.regs.clone(), frame.preif.clone()),
                    };
                    merge_branch(&mut m, &frame.cond, &arm1, &arm2, &frame.preif);
                }
                other => {
                    return Err(TranslateError::UnsupportedInstruction {
                        desc: format!("flow control {other}"),
                    })
                }
            },
            Instruction::Texkill { reg } => {
                // PROVEN (census 4/4): dst is a TEMP with the full xyzw
                // mask; the pixel dies if ANY tested component < 0.
                // Flattened form: the kill condition is gated by every
                // enclosing branch condition, accumulated, and emitted
                // as one guarded `discard` before the output writes.
                if reg.ty != RegType::Temp {
                    return Err(TranslateError::UnsupportedInstruction {
                        desc: format!("texkill on non-temp register {reg}"),
                    });
                }
                if reg.writemask != [true, true, true, true] {
                    return Err(TranslateError::UnsupportedInstruction {
                        desc: format!("texkill with partial mask {} (only xyzw observed)", reg.writemask_str()),
                    });
                }
                let comps = m.read(reg, &mut ctx)?;
                let mut any = String::from("any(vec4<f32>(");
                for i in 0..4 {
                    if i > 0 {
                        any.push_str(", ");
                    }
                    comps[i].write(&mut any);
                }
                any.push_str(") < vec4<f32>(0.0))");
                let mut cond = String::new();
                for f in &frames {
                    cond.push_str(&format!("({}) && ", f.cond));
                }
                cond.push_str(&any);
                m.kill.push(cond);
            }
            // Op2 math — rcp/rsq/frc/abs/nrm/sincos, all PROVEN present
            // in the DLL blobs (length-field census, block 30/31).
            Instruction::Op2 {
                op: op @ (Sm30Op::Rcp | Sm30Op::Rsq | Sm30Op::Frc | Sm30Op::Abs),
                dst,
                a,
            } => {
                let ac = m.read(a, &mut ctx)?;
                let v: [Scalar; 4] = std::array::from_fn(|i| match op {
                    Sm30Op::Rcp => {
                        Scalar::Div(Box::new(Scalar::Con(1.0)), Box::new(ac[i].clone()))
                    }
                    // D3D rsq = 1/sqrt(|x|) — the abs keeps WGSL's
                    // inverseSqrt total for negative inputs.
                    Sm30Op::Rsq => Scalar::Fn(
                        "inverseSqrt".into(),
                        Box::new(Scalar::Fn("abs".into(), Box::new(ac[i].clone()))),
                    ),
                    Sm30Op::Frc => Scalar::Fn("fract".into(), Box::new(ac[i].clone())),
                    _ => Scalar::Fn("abs".into(), Box::new(ac[i].clone())),
                });
                for i in 0..4 {
                    if dst.writemask[i] {
                        lines.push(format!("{} = {};", lhs(dst, i), v[i].render()));
                    }
                }
                m.write(dst, Val::V(v));
            }
            Instruction::Op2 { op: Sm30Op::Nrm, dst, a } => {
                // D3D nrm normalizes the 3-component vector: every
                // masked component = src_component / length(src.xyz).
                // Only .xyz destination masks are observed (15/15) —
                // anything else fails closed.
                if dst.writemask != [true, true, true, false] {
                    return Err(TranslateError::UnsupportedInstruction {
                        desc: format!(
                            "nrm with mask {} (only .xyz observed)",
                            dst.writemask_str()
                        ),
                    });
                }
                let ac = m.read(a, &mut ctx)?;
                let len = Scalar::Fn(
                    "sqrt".into(),
                    Box::new(Scalar::Dot3(
                        Box::new([ac[0].clone(), ac[1].clone(), ac[2].clone()]),
                        Box::new([ac[0].clone(), ac[1].clone(), ac[2].clone()]),
                    )),
                );
                let v: [Scalar; 4] = std::array::from_fn(|i| {
                    Scalar::Div(Box::new(ac[i].clone()), Box::new(len.clone()))
                });
                for i in 0..4 {
                    if dst.writemask[i] {
                        lines.push(format!("{} = {};", lhs(dst, i), v[i].render()));
                    }
                }
                m.write(dst, Val::V(v));
            }
            Instruction::Op2 { op: Sm30Op::Sincos, dst, a } => {
                // PROVEN 2-operand form (length census 2/2 in the
                // blobs; the standard's 4-operand sincos never occurs):
                // dest.x = sin(src.x), dest.y = cos(src.x).  Masks over
                // z/w are unobserved and fail closed.
                if dst.writemask[2] || dst.writemask[3] {
                    return Err(TranslateError::UnsupportedInstruction {
                        desc: format!(
                            "sincos with mask {} (only .x/.xy observed)",
                            dst.writemask_str()
                        ),
                    });
                }
                let ac = m.read(a, &mut ctx)?;
                let src = ac[0].clone();
                let v: [Scalar; 4] = std::array::from_fn(|i| match i {
                    0 => Scalar::Fn("sin".into(), Box::new(src.clone())),
                    1 => Scalar::Fn("cos".into(), Box::new(src.clone())),
                    _ => Scalar::Con(0.0),
                });
                for i in 0..4 {
                    if dst.writemask[i] {
                        lines.push(format!("{} = {};", lhs(dst, i), v[i].render()));
                    }
                }
                m.write(dst, Val::V(v));
            }
            // Op2 is only ever Mov among the translatable ops; the
            // DLL-only two-operand math (rcp/rsq/frc/abs/nrm/sincos)
            // has no WGSL rule yet — an explicit error, never a panic.
            Instruction::Op2 { op, .. } => {
                return Err(TranslateError::UnsupportedInstruction {
                    desc: format!("{op} (DLL-blob opcode)"),
                });
            }
        }
        let comment = format!("{inst}");
        m.stmts.push((comment, lines));
    }
    if !frames.is_empty() {
        return Err(TranslateError::UnsupportedInstruction {
            desc: "unterminated if block (missing endif)".into(),
        });
    }

    // ---- variable declarations -----------------------------------------
    let mut decls = String::new();
    for (RegKey(kind, reg), _) in &m.regs {
        match *kind {
            KIND_TEMP => decls.push_str(&format!("  var r{reg}: vec4<f32> = vec4<f32>(0.0);\n")),
            KIND_OUTPUT | KIND_COLOROUT => {
                decls.push_str(&format!("  var o{reg}: vec4<f32> = vec4<f32>(0.0);\n"))
            }
            KIND_DEPTHOUT => {
                decls.push_str(&format!("  var d{reg}: vec4<f32> = vec4<f32>(0.0);\n"))
            }
            _ => {}
        }
    }

    // ---- body -----------------------------------------------------------
    let mut body = String::new();
    for (comment, lines) in &m.stmts {
        body.push_str(&format!("  // {comment}\n"));
        for l in lines {
            body.push_str(&format!("  {l}\n"));
        }
    }
    // texkill (Cg `clip()`) — one guarded discard over every
    // accumulated, branch-gated kill condition (WGSL `discard` is
    // fragment-stage only, so vertex programs can never have kills).
    if !m.kill.is_empty() {
        body.push_str(&format!(
            "  // texkill (Cg clip()) — accumulated kill conditions\n  if ({}) {{ discard; }}\n",
            m.kill.join(" || ")
        ));
    }

    // ---- inputs struct ---------------------------------------------------
    let in_struct_name = if prog.is_fragment { "FsIn" } else { "VsIn" };
    let mut in_members = String::new();
    // Deterministic location scheme (shared with the other stage).
    for d in prog.dcls() {
        if let Instruction::Dcl { usage, usage_index, reg, .. } = d {
            if reg.ty == RegType::Input {
                let loc = interpolant_location(usage, *usage_index);
                let member = usage_member(usage, *usage_index);
                in_members.push_str(&format!("  @location({loc}) {member}: vec4<f32>,\n"));
            }
        }
    }
    // An EMPTY input struct is invalid WGSL IO (naga rejects it — block-36
    // harness, the DLL `error` FS reads no interpolants): omit BOTH the
    // struct and the function parameter entirely (`fn fs_main() -> FsOut`).
    let in_struct = if in_members.is_empty() {
        String::new()
    } else {
        format!("struct {in_struct_name} {{\n{in_members}}}\n")
    };
    let in_param = if in_members.is_empty() {
        "()".to_string()
    } else {
        format!("(in: {in_struct_name})")
    };

    // ---- outputs struct + final assignment --------------------------------
    let out_struct_name = if prog.is_fragment { "FsOut" } else { "VsOut" };
    let mut out_members = String::new();
    let mut out_assigns = String::new();
    if prog.is_fragment {
        // ps_3_0 does NOT declare oC# with a dcl — collect the color
        // output registers the program actually WROTE.
        let mut oc: Vec<ParamToken> = m
            .regs
            .keys()
            .filter(|k| k.0 == KIND_COLOROUT)
            .map(|k| ParamToken {
                reg: k.1,
                ty: RegType::ColorOut,
                writemask: [true; 4],
                swizzle: [0, 1, 2, 3],
                src_mod: SrcMod::None,
                saturate: false,
                partial_precision: false,
                is_dst: true,
            })
            .collect();
        oc.sort_by_key(|t| t.reg);
        for (n, reg) in oc.iter().enumerate() {
            let comps = output_comps(&m, reg, false)?;
            out_members.push_str(&format!("  @location({n}) color{n}: vec4<f32>,\n"));
            out_assigns.push_str(&assign_vec4(&format!("out.color{n}"), &comps));
        }
        if oc.is_empty() {
            return Err(TranslateError::IncompleteOutput { reg: "oC*".into() });
        }
        // ps_3_0 oDepth (one mov, no dcl — census: 1 occurrence,
        // shadowdepthfloat_fp) → @builtin(frag_depth).
        let mut depths: Vec<u16> = m
            .regs
            .keys()
            .filter(|k| k.0 == KIND_DEPTHOUT)
            .map(|k| k.1)
            .collect();
        depths.sort();
        for reg in depths {
            let v = m.regs.get(&RegKey(KIND_DEPTHOUT, reg)).ok_or_else(|| {
                TranslateError::IncompleteOutput { reg: format!("oDepth{reg}") }
            })?;
            let comps = comps_of(v);
            out_members.push_str("  @builtin(frag_depth) depth: f32,\n");
            out_assigns.push_str(&format!("  out.depth = {};\n", comps[0].render()));
        }
    } else {
        let outs = prog.outputs();
        if !outs.iter().any(|(u, _, _)| **u == Usage::Position) {
            return Err(TranslateError::NoPositionOutput);
        }
        for (usage, idx, reg) in &outs {
            let strict = **usage == Usage::Position;
            let comps = output_comps(&m, reg, strict)?;
            let member = usage_member(usage, *idx);
            if **usage == Usage::Position {
                out_members.push_str("  @builtin(position) position0: vec4<f32>,\n");
                out_assigns.push_str(&assign_vec4("out.position0", &comps));
            } else {
                let loc = interpolant_location(usage, *idx);
                out_members.push_str(&format!("  @location({loc}) {member}: vec4<f32>,\n"));
                out_assigns.push_str(&assign_vec4(&format!("out.{member}"), &comps));
            }
        }
    }

    // ---- resource globals ---------------------------------------------------
    let mut globals = String::new();
    let mut binding = 0usize;
    let mut plan_bindings: Vec<super::plan::StageBinding> = Vec::new();
    // Float-constant uniforms in register order: every parameter-record
    // register PLUS every register the symbolic execution actually read
    // (the DLL blobs carry no parameter records — their constants get
    // deterministic `u_c{n}` names).  Def'd registers are excluded
    // (baked in).  Original names as comments.
    let mut uniform_names: BTreeMap<u16, String> = ctx.uniforms.clone();
    for (r, n) in &ctx.used_consts {
        uniform_names.entry(*r).or_insert_with(|| n.clone());
    }
    let mut uniform_names: Vec<(u16, String)> = uniform_names.into_iter().collect();
    uniform_names.sort_by_key(|(r, _)| *r);
    for (reg, name) in &uniform_names {
        globals.push_str(&format!(
            "@group(0) @binding({binding}) var<uniform> {name}: vec4<f32>; // c{reg}\n"
        ));
        plan_bindings.push(super::plan::StageBinding::Uniform {
            register: *reg,
            wgsl_name: name.clone(),
            binding: binding as u32,
        });
        binding += 1;
    }
    // Boolean-constant uniforms (the `if b#` feature flags) — bound
    // after the float constants, before the samplers.
    let mut bool_names: Vec<(u16, String)> = ctx.bools.iter().map(|(r, n)| (*r, n.clone())).collect();
    bool_names.sort_by_key(|(r, _)| *r);
    for (reg, name) in &bool_names {
        globals.push_str(&format!(
            "@group(0) @binding({binding}) var<uniform> {name}: vec4<f32>; // boolean constant b{reg}\n"
        ));
        plan_bindings.push(super::plan::StageBinding::BoolUniform {
            register: *reg,
            wgsl_name: name.clone(),
            binding: binding as u32,
        });
        binding += 1;
    }
    for (reg, name) in &ctx.samplers {
        globals.push_str(&format!(
            "@group(0) @binding({binding}) var {name}_tex: texture_2d<f32>; // s{reg}\n"
        ));
        plan_bindings.push(super::plan::StageBinding::Texture {
            register: *reg,
            wgsl_name: format!("{name}_tex"),
            binding: binding as u32,
        });
        binding += 1;
        globals.push_str(&format!(
            "@group(0) @binding({binding}) var {name}_smp: sampler;\n"
        ));
        plan_bindings.push(super::plan::StageBinding::Sampler {
            register: *reg,
            wgsl_name: format!("{name}_smp"),
            binding: binding as u32,
        });
        binding += 1;
    }

    let entry_attr = if prog.is_fragment { "@fragment" } else { "@vertex" };
    let entry_fn = if prog.is_fragment { "fs_main" } else { "vs_main" };

    let wgsl = format!(
        "// prototype-rs clean-room translation of {name:?} ({target}).\n\
         // Every statement below derives from one decoded SM 3.0 instruction\n\
         // (the `//` comments quote the original disassembly).  The arithmetic\n\
         // is PROVEN; location/binding numbering is LIKELY (deterministic,\n\
         // shared by both stages of a shader pair).\n\
         \n\
         {globals}
         {in_struct}
         struct {out_struct_name} {{\n{out_members}}}\n\
         \n\
         {entry_attr}\n\
         fn {entry_fn}{in_param} -> {out_struct_name} {{\n\
         {decls}  var out: {out_struct_name};\n\
         {body}\n\
         {out_assigns}  return out;\n\
         }}\n"
    );
    Ok((wgsl, super::plan::StageResources { bindings: plan_bindings }))
}

fn output_comps(
    m: &Machine,
    reg: &ParamToken,
    strict: bool,
) -> Result<[Scalar; 4], TranslateError> {
    let v = m.regs.get(&key(reg)).ok_or_else(|| TranslateError::IncompleteOutput {
        reg: format!("{}{}", reg.ty, reg.reg),
    })?;
    let comps = comps_of(v);
    let zero = Scalar::Con(0.0);
    let filled = std::array::from_fn(|i| {
        if reg.writemask[i] {
            comps[i].clone()
        } else {
            // D3D leaves unwritten interpolant components undefined;
            // we deterministically emit 0.0 (documented deviation).
            // Position outputs stay strict.
            if strict {
                comps[i].clone()
            } else {
                zero.clone()
            }
        }
    });
    if strict {
        for i in 0..4 {
            if !reg.writemask[i] {
                return Err(TranslateError::IncompleteOutput {
                    reg: format!("{}{}.{}", reg.ty, reg.reg, COMP[i]),
                });
            }
        }
    }
    Ok(filled)
}

fn assign_vec4(lhs: &str, comps: &[Scalar; 4]) -> String {
    format!(
        "  {lhs} = vec4<f32>({}, {}, {}, {});\n",
        comps[0].render(),
        comps[1].render(),
        comps[2].render(),
        comps[3].render()
    )
}

/// The assignment LHS for component `i` of `dst`.
fn lhs(dst: &ParamToken, i: usize) -> String {
    match dst.ty {
        RegType::Temp => format!("r{}.{}", dst.reg, COMP[i]),
        RegType::Output | RegType::ColorOut => format!("o{}.{}", dst.reg, COMP[i]),
        RegType::DepthOut => format!("d{}.{}", dst.reg, COMP[i]),
        other => format!("{}{}.{}", other, dst.reg, COMP[i]),
    }
}

/// Flatten one `if/else/endif` boundary: join the two arms' register
/// states with `select(else_value, if_value, cond)` — exactly the D3D9
/// branch-state semantics (a register written in one arm holds the
/// taken arm's value; the un-taken arm's value defaults to the pre-if
/// state; a register no arm wrote keeps the pre-if value).  Registers
/// whose values agree in both arms need no select.  For each differing
/// component one `rK.c = select(...)` statement is emitted and the
/// merged value becomes a read of that variable, so later instructions
/// do not re-inline the select.
fn merge_branch(
    m: &mut Machine,
    cond: &str,
    arm1: &BTreeMap<RegKey, Val>,
    arm2: &BTreeMap<RegKey, Val>,
    preif: &BTreeMap<RegKey, Val>,
) {
    let mut keys: Vec<RegKey> = arm1.keys().chain(arm2.keys()).copied().collect();
    keys.sort();
    keys.dedup();
    let mut lines: Vec<String> = Vec::new();
    let mut merged: BTreeMap<RegKey, Val> = BTreeMap::new();
    for k in &keys {
        if !matches!(k.0, KIND_TEMP | KIND_OUTPUT | KIND_COLOROUT | KIND_DEPTHOUT) {
            continue;
        }
        let a = arm1.get(&k).or_else(|| preif.get(&k));
        let b = arm2.get(&k).or_else(|| preif.get(&k));
        let joined = match (a, b) {
            (Some(va), Some(vb)) if va == vb => Some(va.clone()),
            (Some(va), Some(vb)) => {
                let ca = comps_of(va);
                let cb = comps_of(vb);
                let var = match k.0 {
                    KIND_TEMP => format!("r{}", k.1),
                    KIND_OUTPUT | KIND_COLOROUT => format!("o{}", k.1),
                    _ => format!("d{}", k.1),
                };
                let mut mc: [Scalar; 4] = std::array::from_fn(|_| Scalar::Con(0.0));
                for i in 0..4 {
                    if ca[i] == cb[i] {
                        mc[i] = ca[i].clone();
                    } else {
                        lines.push(format!(
                            "{var}.{} = select({}, {}, {cond});",
                            COMP[i],
                            cb[i].render(),
                            ca[i].render()
                        ));
                        mc[i] = Scalar::Term(format!("{var}.{}", COMP[i]));
                    }
                }
                Some(Val::V(mc))
            }
            // Written in only one arm and never before: D3D leaves the
            // other arm's value undefined; we deterministically take the
            // written value (real shaders never read such a register in
            // the un-taken arm — documented deviation).
            (Some(va), None) | (None, Some(va)) => Some(va.clone()),
            (None, None) => None,
        };
        if let Some(v) = joined {
            merged.insert(*k, v);
        }
    }
    if !lines.is_empty() {
        m.stmts
            .push((format!("// flatten: if/else merge (cond {cond})"), lines));
    }
    m.regs = merged;
}

// ---------------------------------------------------------------------------
// Batch translation
// ---------------------------------------------------------------------------

/// One program's translation result inside a [`TranslationReport`].
#[derive(Debug, Clone, PartialEq)]
pub struct TranslatedShader {
    pub name: String,
    pub is_fragment: bool,
    /// WGSL source on success.
    pub wgsl: Option<String>,
    /// Error text on failure.
    pub error: Option<String>,
    /// Decoded instruction count.
    pub instructions: usize,
}

/// Aggregate statistics over a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TranslationStats {
    pub total: usize,
    pub translated: usize,
    pub failed: usize,
    /// Sum of decoded instructions across all translated programs.
    pub instructions_total: usize,
}

/// The result of [`translate_programs`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TranslationReport {
    pub stats: TranslationStats,
    pub shaders: Vec<TranslatedShader>,
}

/// Translate raw `(name, is_fragment, d3d9_tokens)` triples — the shape
/// `P3dFile::shader_programs()` produces.  Decode failures and
/// translation failures are recorded per shader, never fatal.
pub fn translate_programs<I>(programs: I) -> TranslationReport
where
    I: IntoIterator<Item = (String, bool, Vec<u8>)>,
{
    let mut report = TranslationReport::default();
    for (name, is_fragment, tokens) in programs {
        report.stats.total += 1;
        let outcome = Sm30Shader::parse(&tokens, is_fragment).map_err(TranslateError::Sm30).and_then(|s| {
            let n = s.instructions.len();
            translate_program(&s, &name).map(|w| (w, n))
        });
        match outcome {
            Ok((wgsl, n)) => {
                report.stats.translated += 1;
                report.stats.instructions_total += n;
                report.shaders.push(TranslatedShader {
                    name,
                    is_fragment,
                    wgsl: Some(wgsl),
                    error: None,
                    instructions: n,
                });
            }
            Err(e) => {
                report.stats.failed += 1;
                report.shaders.push(TranslatedShader {
                    name,
                    is_fragment,
                    wgsl: None,
                    error: Some(e.to_string()),
                    instructions: 0,
                });
            }
        }
    }
    report
}

// ---------------------------------------------------------------------------
// Fixture-driven tests (inline — see sm30.rs for why)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shader::sm30::Sm30Shader;
    use prototype_p3d::P3dFile;

    fn fixture(name: &str) -> Vec<u8> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../prototype-bin/tests/fixtures")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

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
    fn translates_all_26_shipped_programs_to_wgsl() {
        let report = translate_programs(all_programs());
        assert_eq!(report.stats.total, 26);
        assert_eq!(
            report.stats.translated, 26,
            "failures: {:?}",
            report
                .shaders
                .iter()
                .filter_map(|s| s.error.as_ref())
                .collect::<Vec<_>>()
        );
        assert_eq!(report.stats.failed, 0);
        // 262 decoded instructions total (matches the sm30 decoder test).
        assert_eq!(report.stats.instructions_total, 262);
    }


    #[test]
    fn vertex_wgsl_has_uniforms_inputs_and_position_output() {
        let (_, is_fragment, tokens) = all_programs()
            .into_iter()
            .find(|(n, _, _)| n == "cxformsolidcolour - lit - Vertex Program")
            .unwrap();
        assert!(!is_fragment);
        let s = Sm30Shader::parse(&tokens, false).unwrap();
        let wgsl = translate_program(&s, "cxformsolidcolour").unwrap();

        // Entry points and structs.
        assert!(wgsl.contains("@vertex"), "{wgsl}");
        assert!(wgsl.contains("fn vs_main(in: VsIn) -> VsOut"), "{wgsl}");
        assert!(wgsl.contains("@builtin(position) position0: vec4<f32>"), "{wgsl}");
        // Block 34: the vertex INPUT position moved to the dedicated
        // location 32 (it used to collide with texcoord0's @location(0),
        // making every position+texcoord0 pipeline uncompilable).
        assert!(wgsl.contains("@location(32) position0: vec4<f32>"), "{wgsl}");

        // The WVP matrix rows are named uniforms (c0..c3 of the
        // float4x4 parameter record).
        for r in 0..4 {
            assert!(
                wgsl.contains(&format!("p3dWorldViewProjectionMatrix_r{r}")),
                "{wgsl}"
            );
        }
        // The uniform list comes from the post-END parameter records —
        // i.e. the registers the ENGINE actually binds for this program
        // (cxformsolidcolour binds only the WVP matrix; its CTAB also
        // lists c4/c5 but they are unused and unbound here).
        assert!(wgsl.contains("p3dWorldViewProjectionMatrix_r0: vec4<f32>"), "{wgsl}");
        assert!(!wgsl.contains("p3dLight_ObjectSpaceLightBounds"), "{wgsl}");

        // The m4x4 unroll becomes per-row products over the input
        // position (bytecode order: mul c1*v.y, mad +c0*v.x, mad +c2*v.z,
        // add +c3 — the w-row folded under w=1).
        assert!(
            wgsl.contains("p3dWorldViewProjectionMatrix_r0.x * in.position0.x"),
            "{wgsl}"
        );
        assert!(wgsl.contains("out.position0 = vec4<f32>("), "{wgsl}");
        // Block-36 (naga harness): the emit must DECLARE the `out` var —
        // `out.* = ...` + `return out;` used to reference it with no
        // definition, which naga rejects for EVERY translated program.
        assert!(wgsl.contains("var out: VsOut;"), "{wgsl}");
        // No leftover D3D-isms.
        assert!(!wgsl.contains("oPos"), "{wgsl}");
        assert!(!wgsl.contains("register("), "{wgsl}");
    }

    #[test]
    fn textured_fragment_wgsl_emits_texture_sample() {
        let (_, is_fragment, tokens) = all_programs()
            .into_iter()
            .find(|(n, _, _)| n == "cxformtexture - lit - Fragment Program")
            .unwrap();
        assert!(is_fragment);
        let s = Sm30Shader::parse(&tokens, true).unwrap();
        let wgsl = translate_program(&s, "cxformtexture").unwrap();

        assert!(wgsl.contains("@fragment"), "{wgsl}");
        assert!(wgsl.contains("fn fs_main(in: FsIn) -> FsOut"), "{wgsl}");
        // Sampler resources from the parameter records.
        assert!(wgsl.contains("diffuseMap_tex: texture_2d<f32>"), "{wgsl}");
        assert!(wgsl.contains("diffuseMap_smp: sampler"), "{wgsl}");
        // The TEX instruction becomes textureSample over the texcoord.
        assert!(
            wgsl.contains("textureSample(diffuseMap_tex, diffuseMap_smp,"),
            "{wgsl}"
        );
        // Color modulation with cxmul/cxadd reaches oC0 → color0.
        assert!(wgsl.contains("out.color0 = vec4<f32>("), "{wgsl}");
        assert!(wgsl.contains("cxmul"), "{wgsl}");
        assert!(wgsl.contains("cxadd"), "{wgsl}");
    }

    #[test]
    fn bink_fragment_wgsl_uses_dp3() {
        let (_, is_fragment, tokens) = all_programs()
            .into_iter()
            .find(|(n, f, _)| n.contains("bink") && *f)
            .unwrap();
        assert!(is_fragment);
        let s = Sm30Shader::parse(&tokens, true).unwrap();
        let wgsl = translate_program(&s, "bink").unwrap();
        // dp3 renders as a 3-component dot.
        assert!(wgsl.contains("dot(vec3<f32>("), "{wgsl}");
        assert!(wgsl.contains("@fragment"), "{wgsl}");
    }

    #[test]
    fn shader_pair_locations_line_up() {
        // The deterministic location scheme must give the vertex
        // outputs and the fragment inputs of the SAME pair identical
        // locations, or the pipeline would not link.
        let progs = all_programs();
        for name in [
            "CxformGauraud - lit",
            "cxformtexture - lit",
            "CxformGauraudMultiplyTexture - lit",
        ] {
            let vs = progs
                .iter()
                .find(|(n, f, _)| n.starts_with(name) && !*f)
                .unwrap();
            let fs = progs
                .iter()
                .find(|(n, f, _)| n.starts_with(name) && *f)
                .unwrap();
            let sv = Sm30Shader::parse(&vs.2, false).unwrap();
            let sf = Sm30Shader::parse(&fs.2, true).unwrap();
            let wv = translate_program(&sv, &vs.0).unwrap();
            let wf = translate_program(&sf, &fs.0).unwrap();
            // Extract (location, member) pairs from both structs.
            let locs = |src: &str, marker: &str| -> Vec<(usize, String)> {
                let start = src.find(marker).unwrap();
                let section = &src[start..];
                section
                    .lines()
                    .filter_map(|l| {
                        let l = l.trim();
                        let rest = l.strip_prefix("@location(")?;
                        let close = rest.find(')')?;
                        let loc: usize = rest[..close].parse().ok()?;
                        let member = rest[close + 1..]
                            .split(':')
                            .next()?
                            .trim()
                            .to_string();
                        Some((loc, member))
                    })
                    .collect()
            };
            let out_locs = locs(&wv, "struct VsOut");
            let in_locs = locs(&wf, "struct FsIn");
            assert!(
                !out_locs.is_empty() && !in_locs.is_empty(),
                "{name}: {out_locs:?} vs {in_locs:?}"
            );
            for (loc, member) in &out_locs {
                assert!(
                    in_locs.iter().any(|(l, m)| l == loc && m == member),
                    "{name}: vertex output {member}@{loc} has no matching fragment input (have {in_locs:?})"
                );
            }
        }
    }

    // -------------------------------------------------------------------
    // Block 31 — the DLL world-shader blobs translate END TO END
    // -------------------------------------------------------------------

    #[test]
    fn translates_all_nine_dll_blob_fixtures_end_to_end() {
        let blobs = [
            ("dll_error_vp_e36010.sm30", false),
            ("dll_lightcount_vp_e37238.sm30", false),
            ("dll_default_vp_e34e60.sm30", false),
            ("dll_unlit_fp_e41670.sm30", true),
            ("dll_bink_fp_e3adf8.sm30", true),
            ("dll_pointsprite_fp_e41cf0.sm30", true),
            ("dll_default_fp_e3b308.sm30", true),
            ("dll_shadowdebug_fp_e405f0.sm30", true),
            ("dll_shadowblur_fp_e3f858.sm30", true),
        ];
        let mut total_instructions = 0;
        for (name, is_frag) in blobs {
            let bytes = fixture(name);
            let s = Sm30Shader::parse_program(&bytes, is_frag)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            total_instructions += s.instructions.len();
            let wgsl = translate_program(&s, name)
                .unwrap_or_else(|e| panic!("{name} failed to translate: {e}"));
            assert!(
                wgsl.contains(if is_frag { "@fragment" } else { "@vertex" }),
                "{name}: entry point missing"
            );
            assert!(wgsl.contains("return out;"), "{name}");
        }
        // 6+26+57+5+20+24+133+63+83 — pinned against silent fixture or
        // decoder drift (matches the sm30 census test).
        assert_eq!(total_instructions, 417);
    }

    #[test]
    fn default_fp_wgsl_carries_the_world_shader_contract() {
        let bytes = fixture("dll_default_fp_e3b308.sm30");
        let s = Sm30Shader::parse_program(&bytes, true).unwrap();
        let wgsl = translate_program(&s, "default_fp").unwrap();

        // The three flattened branch conditions over the boolean-constant
        // uniforms (the engine's per-draw feature flags).
        for b in ["b0", "b1", "b2"] {
            assert!(wgsl.contains(&format!("{b}.x != 0.0")), "{wgsl}");
            assert!(
                wgsl.contains(&format!("var<uniform> {b}: vec4<f32>; // boolean constant {b}")),
                "{wgsl}"
            );
        }
        // Exactly one WGSL `if (` — the texkill discard; the branch
        // structure itself is flattened into selects.
        assert_eq!(wgsl.match_indices("if (").count(), 1, "{wgsl}");
        assert!(
            wgsl.contains("(b0.x != 0.0) && any(vec4<f32>("),
            "texkill must be gated by the enclosing branch: {wgsl}"
        );
        assert!(wgsl.contains("{ discard; }"), "{wgsl}");
        assert_eq!(wgsl.matches("// flatten: if/else merge").count(), 3, "{wgsl}");

        // New-opcode renderings on the real instructions.
        assert!(wgsl.contains("inverseSqrt(abs("), "rsq: {wgsl}");
        assert!(wgsl.contains("pow("), "{wgsl}");
        assert!(wgsl.contains("fract("), "frc (PCF taps): {wgsl}");
        assert!(wgsl.contains("max("), "{wgsl}");
        // The `_dw` projective divide: `mul r7.xyzw, _dw(r7).xyzw, r1.zzzz`
        // folds to (r7 component / r7.w) * scale over the shadow coords.
        assert!(
            wgsl.contains(") / (u_c13.w + (-in.texcoord1.y))"),
            "_dw divide: {wgsl}"
        );

        // 7 samplers (misc0..6) with no parameter records → s# names.
        for sreg in 0..7 {
            assert!(
                wgsl.contains(&format!("var s{sreg}_tex: texture_2d<f32>; // s{sreg}")),
                "{wgsl}"
            );
        }
        // Constants: u_c0..u_c19 declared from the reads; the def'd
        // c20/c21 are baked in (no uniform).
        assert!(wgsl.contains("var<uniform> u_c0: vec4<f32>; // c0"), "{wgsl}");
        assert!(wgsl.contains("var<uniform> u_c19: vec4<f32>; // c19"), "{wgsl}");
        assert!(!wgsl.contains("u_c20"), "def'd c20 must be baked in: {wgsl}");
        assert!(!wgsl.contains("u_c21"), "def'd c21 must be baked in: {wgsl}");
        // Eight texcoord interpolants in, one color out.
        assert!(wgsl.contains("@location(7) texcoord7: vec4<f32>"), "{wgsl}");
        assert!(wgsl.contains("@location(0) texcoord0: vec4<f32>"), "{wgsl}");
        // The final `mov oC0, temp0` carries the MERGED temp0: the b0
        // select variables for xyz (the light path vs the else black)
        // and the untouched diffuse-sample alpha for w.
        assert!(
            wgsl.contains("out.color0 = vec4<f32>(r0.x, r0.y, r0.z, s4_sample.w);"),
            "{wgsl}"
        );
    }

    #[test]
    fn default_fp_unique_sample_let_names() {
        // default_fp samples s3 five times (the PCF taps); every texld
        // must get a distinct `let` name or the WGSL would redeclare.
        let bytes = fixture("dll_default_fp_e3b308.sm30");
        let s = Sm30Shader::parse_program(&bytes, true).unwrap();
        let wgsl = translate_program(&s, "default_fp").unwrap();
        assert_eq!(
            wgsl.matches("textureSample(s3_tex, s3_smp").count(),
            5,
            "{wgsl}"
        );
        assert_eq!(wgsl.matches("let s3_sample").count(), 5, "{wgsl}");
        // The first s3 texld is the 5th sample overall → s3_sample_5.
        assert!(wgsl.contains("let s3_sample_5 = textureSample("), "{wgsl}");
        assert!(wgsl.contains("let s3_sample_9 = textureSample("), "{wgsl}");
    }

    #[test]
    fn lightcount_vp_wgsl_pins_nrm_slt_and_max() {
        let bytes = fixture("dll_lightcount_vp_e37238.sm30");
        let s = Sm30Shader::parse_program(&bytes, false).unwrap();
        let wgsl = translate_program(&s, "lightcount_vp").unwrap();
        assert!(wgsl.contains("@vertex"), "{wgsl}");
        // nrm → per-component divide by the length of the swizzled xyz.
        assert!(wgsl.contains("/ sqrt(dot(vec3<f32>("), "{wgsl}");
        // slt → select(0.0, 1.0, a < b).
        assert!(wgsl.contains("select(0.0, 1.0, ("), "{wgsl}");
        assert!(wgsl.contains("max("), "{wgsl}");
        // Outputs: o0 = position, o1 = color0 (usage 10).
        assert!(wgsl.contains("@builtin(position) position0: vec4<f32>"), "{wgsl}");
        assert!(wgsl.contains("@location(8) color0: vec4<f32>"), "{wgsl}");
    }

    #[test]
    fn pointsprite_fp_wgsl_pins_sincos_dp2add_min_and_frc() {
        let bytes = fixture("dll_pointsprite_fp_e41cf0.sm30");
        let s = Sm30Shader::parse_program(&bytes, true).unwrap();
        let wgsl = translate_program(&s, "pointsprite_fp").unwrap();
        assert!(wgsl.contains("sin("), "{wgsl}");
        assert!(wgsl.contains("cos("), "{wgsl}");
        assert!(wgsl.contains("dot(vec2<f32>("), "dp2add: {wgsl}");
        assert!(wgsl.contains("min("), "{wgsl}");
        assert!(wgsl.contains("fract("), "{wgsl}");
        // The `cmp` still renders through the >= 0.0 select.
        assert!(wgsl.contains(" >= 0.0)"), "{wgsl}");
    }

    // ---- synthetic flatten/modifier tests -----------------------------

    fn tok_dst(ty: RegType, reg: u16, mask: [bool; 4]) -> ParamToken {
        ParamToken {
            reg,
            ty,
            writemask: mask,
            swizzle: [0, 1, 2, 3],
            src_mod: SrcMod::None,
            saturate: false,
            partial_precision: false,
            is_dst: true,
        }
    }

    fn tok_src(ty: RegType, reg: u16, swz: [u8; 4], m: SrcMod) -> ParamToken {
        ParamToken {
            reg,
            ty,
            writemask: [false; 4],
            swizzle: swz,
            src_mod: m,
            saturate: false,
            partial_precision: false,
            is_dst: false,
        }
    }

    fn dcl_in(u: Usage, usage_index: u8, reg: u16) -> Instruction {
        Instruction::Dcl {
            usage_token: 0,
            usage: u,
            usage_index,
            reg: tok_dst(RegType::Input, reg, [true; 4]),
        }
    }

    fn frag(instructions: Vec<Instruction>) -> Sm30Shader {
        Sm30Shader {
            is_fragment: true,
            version_token: 0xFFFF_0300,
            ctab: None,
            instructions,
            defs: vec![],
            params: vec![],
            comments: vec![],
        }
    }

    const SW: [u8; 4] = [0, 1, 2, 3];

    #[test]
    fn flattened_if_else_joins_with_select() {
        let prog = frag(vec![
            dcl_in(Usage::TexCoord, 0, 0),
            dcl_in(Usage::Color, 0, 1),
            Instruction::Flow {
                op: Sm30Op::If,
                reg: Some(tok_src(RegType::ConstBool, 0, SW, SrcMod::None)),
            },
            Instruction::Op2 {
                op: Sm30Op::Mov,
                dst: tok_dst(RegType::Temp, 0, [true; 4]),
                a: tok_src(RegType::Input, 0, SW, SrcMod::None),
            },
            Instruction::Flow { op: Sm30Op::Else, reg: None },
            Instruction::Op2 {
                op: Sm30Op::Mov,
                dst: tok_dst(RegType::Temp, 0, [true; 4]),
                a: tok_src(RegType::Input, 1, SW, SrcMod::None),
            },
            Instruction::Flow { op: Sm30Op::Endif, reg: None },
            Instruction::Op2 {
                op: Sm30Op::Mov,
                dst: tok_dst(RegType::ColorOut, 0, [true; 4]),
                a: tok_src(RegType::Temp, 0, SW, SrcMod::None),
            },
        ]);
        let wgsl = translate_program(&prog, "flow_join").unwrap();
        // select(else_value, if_value, cond) — D3D9 branch-state merge.
        assert!(
            wgsl.contains("r0.x = select(in.color0.x, in.texcoord0.x, b0.x != 0.0);"),
            "{wgsl}"
        );
        assert!(wgsl.contains("var<uniform> b0: vec4<f32>; // boolean constant b0"), "{wgsl}");
        assert!(wgsl.contains("out.color0 = vec4<f32>(r0.x, r0.y, r0.z, r0.w);"), "{wgsl}");
        // No real control flow was emitted.
        assert_eq!(wgsl.match_indices("if (").count(), 0, "{wgsl}");
    }

    #[test]
    fn flattened_if_without_else_joins_against_the_preif_value() {
        let prog = frag(vec![
            dcl_in(Usage::TexCoord, 0, 0),
            Instruction::Op2 {
                op: Sm30Op::Mov,
                dst: tok_dst(RegType::Temp, 0, [true; 4]),
                a: tok_src(RegType::Input, 0, SW, SrcMod::None),
            },
            Instruction::Flow {
                op: Sm30Op::If,
                reg: Some(tok_src(RegType::ConstBool, 0, SW, SrcMod::None)),
            },
            // mov r0.x, c0.x — partial mask; c0 read → u_c0 uniform.
            Instruction::Op2 {
                op: Sm30Op::Mov,
                dst: tok_dst(RegType::Temp, 0, [true, false, false, false]),
                a: tok_src(RegType::Const, 0, SW, SrcMod::None),
            },
            Instruction::Flow { op: Sm30Op::Endif, reg: None },
            Instruction::Op2 {
                op: Sm30Op::Mov,
                dst: tok_dst(RegType::ColorOut, 0, [true; 4]),
                a: tok_src(RegType::Temp, 0, SW, SrcMod::None),
            },
        ]);
        let wgsl = translate_program(&prog, "flow_noelse").unwrap();
        // x diverges (select against the pre-if v0 value); y/z/w agree.
        assert!(
            wgsl.contains("r0.x = select(in.texcoord0.x, u_c0.x, b0.x != 0.0);"),
            "{wgsl}"
        );
        assert!(wgsl.contains("var<uniform> u_c0: vec4<f32>; // c0"), "{wgsl}");
        assert!(!wgsl.contains("u_c1"), "{wgsl}");
    }

    #[test]
    fn nested_ifs_flatten_to_nested_selects() {
        let prog = frag(vec![
            dcl_in(Usage::TexCoord, 0, 0),
            dcl_in(Usage::TexCoord, 1, 1),
            dcl_in(Usage::TexCoord, 2, 2),
            dcl_in(Usage::TexCoord, 3, 3),
            Instruction::Flow {
                op: Sm30Op::If,
                reg: Some(tok_src(RegType::ConstBool, 0, SW, SrcMod::None)),
            },
            Instruction::Flow {
                op: Sm30Op::If,
                reg: Some(tok_src(RegType::ConstBool, 1, SW, SrcMod::None)),
            },
            Instruction::Op2 {
                op: Sm30Op::Mov,
                dst: tok_dst(RegType::Temp, 0, [true; 4]),
                a: tok_src(RegType::Input, 1, SW, SrcMod::None),
            },
            Instruction::Flow { op: Sm30Op::Else, reg: None },
            Instruction::Op2 {
                op: Sm30Op::Mov,
                dst: tok_dst(RegType::Temp, 0, [true; 4]),
                a: tok_src(RegType::Input, 2, SW, SrcMod::None),
            },
            Instruction::Flow { op: Sm30Op::Endif, reg: None },
            Instruction::Op2 {
                op: Sm30Op::Mov,
                dst: tok_dst(RegType::ColorOut, 0, [true; 4]),
                a: tok_src(RegType::Temp, 0, SW, SrcMod::None),
            },
            Instruction::Flow { op: Sm30Op::Else, reg: None },
            Instruction::Op2 {
                op: Sm30Op::Mov,
                dst: tok_dst(RegType::ColorOut, 0, [true; 4]),
                a: tok_src(RegType::Input, 3, SW, SrcMod::None),
            },
            Instruction::Flow { op: Sm30Op::Endif, reg: None },
        ]);
        let wgsl = translate_program(&prog, "flow_nested").unwrap();
        // Inner join first (b1), then the outer join (b0) reading r0.
        assert!(
            wgsl.contains("r0.x = select(in.texcoord2.x, in.texcoord1.x, b1.x != 0.0);"),
            "{wgsl}"
        );
        assert!(
            wgsl.contains("o0.x = select(in.texcoord3.x, r0.x, b0.x != 0.0);"),
            "{wgsl}"
        );
    }

    #[test]
    fn texkill_becomes_a_branch_gated_discard() {
        let prog = frag(vec![
            dcl_in(Usage::TexCoord, 0, 0),
            Instruction::Op2 {
                op: Sm30Op::Mov,
                dst: tok_dst(RegType::Temp, 0, [true; 4]),
                a: tok_src(RegType::Input, 0, SW, SrcMod::None),
            },
            Instruction::Flow {
                op: Sm30Op::If,
                reg: Some(tok_src(RegType::ConstBool, 0, SW, SrcMod::None)),
            },
            Instruction::Texkill {
                reg: tok_dst(RegType::Temp, 0, [true; 4]),
            },
            Instruction::Flow { op: Sm30Op::Endif, reg: None },
            Instruction::Op2 {
                op: Sm30Op::Mov,
                dst: tok_dst(RegType::ColorOut, 0, [true; 4]),
                a: tok_src(RegType::Input, 0, SW, SrcMod::None),
            },
        ]);
        let wgsl = translate_program(&prog, "texkill").unwrap();
        // The kill tests the register's VALUE expressions (r0 holds the
        // v0 terms), gated by the enclosing branch condition.
        assert!(
            wgsl.contains(
                "if ((b0.x != 0.0) && any(vec4<f32>(in.texcoord0.x, in.texcoord0.y, in.texcoord0.z, in.texcoord0.w) < vec4<f32>(0.0))) { discard; }"
            ),
            "{wgsl}"
        );
    }

    #[test]
    fn dw_source_modifier_divides_by_the_w_lane() {
        let mut b = tok_src(RegType::Input, 0, SW, SrcMod::Dw);
        b.is_dst = false;
        let prog = frag(vec![
            dcl_in(Usage::TexCoord, 0, 0),
            Instruction::Op3 {
                op: Sm30Op::Mul,
                dst: tok_dst(RegType::ColorOut, 0, [true; 4]),
                a: tok_src(RegType::Input, 0, SW, SrcMod::None),
                b,
            },
        ]);
        let wgsl = translate_program(&prog, "dw").unwrap();
        assert!(
            wgsl.contains("o0.x = (in.texcoord0.x * (in.texcoord0.x / in.texcoord0.w));"),
            "{wgsl}"
        );
    }

    #[test]
    fn dz_source_modifier_divides_by_the_z_lane() {
        // UNOBSERVED in the corpus (public-enum semantics); pinned so a
        // future stream with _dz translates deterministically.
        let b = tok_src(RegType::Input, 0, SW, SrcMod::Dz);
        let prog = frag(vec![
            dcl_in(Usage::TexCoord, 0, 0),
            Instruction::Op3 {
                op: Sm30Op::Mul,
                dst: tok_dst(RegType::ColorOut, 0, [true; 4]),
                a: tok_src(RegType::Input, 0, SW, SrcMod::None),
                b,
            },
        ]);
        let wgsl = translate_program(&prog, "dz").unwrap();
        assert!(wgsl.contains("(in.texcoord0.x / in.texcoord0.z)"), "{wgsl}");
    }

    #[test]
    fn abs_source_modifier_wraps_in_abs() {
        let b = tok_src(RegType::Input, 0, SW, SrcMod::Abs);
        let prog = frag(vec![
            dcl_in(Usage::TexCoord, 0, 0),
            Instruction::Op3 {
                op: Sm30Op::Add,
                dst: tok_dst(RegType::ColorOut, 0, [true; 4]),
                a: tok_src(RegType::Input, 0, SW, SrcMod::None),
                b,
            },
        ]);
        let wgsl = translate_program(&prog, "absmod").unwrap();
        assert!(wgsl.contains("abs(in.texcoord0.x)"), "{wgsl}");
    }

    #[test]
    fn new_opcode_matrix_renders_each_wgsl_form() {
        let v = |swz: [u8; 4]| tok_src(RegType::Input, 0, swz, SrcMod::None);
        let t = |reg: u16, mask: [bool; 4]| tok_dst(RegType::Temp, reg, mask);
        let full = [true; 4];
        let xyz = [true, true, true, false];
        let x = [true, false, false, false];
        let xy = [true, true, false, false];
        let prog = frag(vec![
            dcl_in(Usage::TexCoord, 0, 0),
            Instruction::Op2 { op: Sm30Op::Frc, dst: t(0, x), a: v(SW) },
            Instruction::Op2 { op: Sm30Op::Sincos, dst: t(1, xy), a: v(SW) },
            Instruction::Op4 {
                op: Sm30Op::Dp2add,
                dst: t(2, x),
                a: v(SW),
                b: v([2, 3, 0, 1]),
                c: v([2, 2, 2, 2]),
            },
            Instruction::Op3 { op: Sm30Op::Min, dst: t(3, x), a: v(SW), b: v([1, 1, 1, 1]) },
            Instruction::Op3 { op: Sm30Op::Max, dst: t(4, x), a: v(SW), b: v([1, 1, 1, 1]) },
            Instruction::Op3 { op: Sm30Op::Slt, dst: t(5, x), a: v(SW), b: v([1, 1, 1, 1]) },
            Instruction::Op2 { op: Sm30Op::Rcp, dst: t(6, x), a: v(SW) },
            Instruction::Op2 { op: Sm30Op::Rsq, dst: t(7, x), a: v(SW) },
            Instruction::Op2 { op: Sm30Op::Nrm, dst: t(8, xyz), a: v(SW) },
            Instruction::Op2 { op: Sm30Op::Abs, dst: t(9, x), a: v(SW) },
            Instruction::Op3 { op: Sm30Op::Pow, dst: t(10, x), a: v(SW), b: v([1, 1, 1, 1]) },
            Instruction::Op2 {
                op: Sm30Op::Mov,
                dst: tok_dst(RegType::ColorOut, 0, full),
                a: v(SW),
            },
        ]);
        let wgsl = translate_program(&prog, "opcode_matrix").unwrap();
        assert!(wgsl.contains("r0.x = fract(in.texcoord0.x);"), "{wgsl}");
        assert!(wgsl.contains("r1.x = sin(in.texcoord0.x);"), "{wgsl}");
        assert!(wgsl.contains("r1.y = cos(in.texcoord0.x);"), "{wgsl}");
        assert!(
            wgsl.contains(
                "r2.x = (dot(vec2<f32>(in.texcoord0.x, in.texcoord0.y), vec2<f32>(in.texcoord0.z, in.texcoord0.w)) + in.texcoord0.z);"
            ),
            "{wgsl}"
        );
        assert!(wgsl.contains("r3.x = min(in.texcoord0.x, in.texcoord0.y);"), "{wgsl}");
        assert!(wgsl.contains("r4.x = max(in.texcoord0.x, in.texcoord0.y);"), "{wgsl}");
        assert!(
            wgsl.contains("r5.x = select(0.0, 1.0, (in.texcoord0.x) < (in.texcoord0.y));"),
            "{wgsl}"
        );
        assert!(wgsl.contains("r6.x = (1.0 / in.texcoord0.x);"), "{wgsl}");
        assert!(wgsl.contains("r7.x = inverseSqrt(abs(in.texcoord0.x));"), "{wgsl}");
        assert!(
            wgsl.contains(
                "r8.x = (in.texcoord0.x / sqrt(dot(vec3<f32>(in.texcoord0.x, in.texcoord0.y, in.texcoord0.z), vec3<f32>(in.texcoord0.x, in.texcoord0.y, in.texcoord0.z))));"
            ),
            "{wgsl}"
        );
        assert!(wgsl.contains("r9.x = abs(in.texcoord0.x);"), "{wgsl}");
        assert!(wgsl.contains("r10.x = pow(in.texcoord0.x, in.texcoord0.y);"), "{wgsl}");
    }

    #[test]
    fn odepth_write_becomes_the_frag_depth_builtin() {
        let prog = frag(vec![
            dcl_in(Usage::TexCoord, 0, 0),
            Instruction::Op2 {
                op: Sm30Op::Mov,
                dst: tok_dst(RegType::DepthOut, 0, [true; 4]),
                a: tok_src(RegType::Input, 0, SW, SrcMod::None),
            },
            Instruction::Op2 {
                op: Sm30Op::Mov,
                dst: tok_dst(RegType::ColorOut, 0, [true; 4]),
                a: tok_src(RegType::Input, 0, SW, SrcMod::None),
            },
        ]);
        let wgsl = translate_program(&prog, "odepth").unwrap();
        assert!(wgsl.contains("@builtin(frag_depth) depth: f32,"), "{wgsl}");
        assert!(wgsl.contains("out.depth = in.texcoord0.x;"), "{wgsl}");
        assert!(wgsl.contains("var d0: vec4<f32> = vec4<f32>(0.0);"), "{wgsl}");
    }

    #[test]
    fn unobserved_shapes_fail_closed() {
        // nrm with a non-.xyz mask (only .xyz observed 15/15).
        let prog = frag(vec![
            dcl_in(Usage::TexCoord, 0, 0),
            Instruction::Op2 {
                op: Sm30Op::Nrm,
                dst: tok_dst(RegType::Temp, 0, [true; 4]),
                a: tok_src(RegType::Input, 0, SW, SrcMod::None),
            },
            Instruction::Op2 {
                op: Sm30Op::Mov,
                dst: tok_dst(RegType::ColorOut, 0, [true; 4]),
                a: tok_src(RegType::Input, 0, SW, SrcMod::None),
            },
        ]);
        let err = translate_program(&prog, "nrm_bad").unwrap_err();
        assert!(err.to_string().contains("nrm with mask"), "{err}");

        // sincos with z/w in the mask (only .x/.xy observed).
        let prog = frag(vec![
            dcl_in(Usage::TexCoord, 0, 0),
            Instruction::Op2 {
                op: Sm30Op::Sincos,
                dst: tok_dst(RegType::Temp, 0, [true; 4]),
                a: tok_src(RegType::Input, 0, SW, SrcMod::None),
            },
            Instruction::Op2 {
                op: Sm30Op::Mov,
                dst: tok_dst(RegType::ColorOut, 0, [true; 4]),
                a: tok_src(RegType::Input, 0, SW, SrcMod::None),
            },
        ]);
        let err = translate_program(&prog, "sincos_bad").unwrap_err();
        assert!(err.to_string().contains("sincos with mask"), "{err}");

        // texkill on a non-temp register (only temps observed 4/4).
        let prog = frag(vec![
            dcl_in(Usage::TexCoord, 0, 0),
            Instruction::Texkill {
                reg: tok_dst(RegType::Input, 0, [true; 4]),
            },
            Instruction::Op2 {
                op: Sm30Op::Mov,
                dst: tok_dst(RegType::ColorOut, 0, [true; 4]),
                a: tok_src(RegType::Input, 0, SW, SrcMod::None),
            },
        ]);
        let err = translate_program(&prog, "texkill_bad").unwrap_err();
        assert!(err.to_string().contains("texkill on non-temp"), "{err}");

        // `if` on a register that is not a boolean constant (only b#
        // observed 6/6).
        let prog = frag(vec![
            dcl_in(Usage::TexCoord, 0, 0),
            Instruction::Flow {
                op: Sm30Op::If,
                reg: Some(tok_src(RegType::Temp, 0, SW, SrcMod::None)),
            },
            Instruction::Op2 {
                op: Sm30Op::Mov,
                dst: tok_dst(RegType::ColorOut, 0, [true; 4]),
                a: tok_src(RegType::Input, 0, SW, SrcMod::None),
            },
        ]);
        let err = translate_program(&prog, "if_temp").unwrap_err();
        assert!(err.to_string().contains("if on non-boolean register"), "{err}");

        // An unbalanced if (no endif).
        let prog = frag(vec![
            dcl_in(Usage::TexCoord, 0, 0),
            Instruction::Flow {
                op: Sm30Op::If,
                reg: Some(tok_src(RegType::ConstBool, 0, SW, SrcMod::None)),
            },
            Instruction::Op2 {
                op: Sm30Op::Mov,
                dst: tok_dst(RegType::ColorOut, 0, [true; 4]),
                a: tok_src(RegType::Input, 0, SW, SrcMod::None),
            },
        ]);
        let err = translate_program(&prog, "if_unbalanced").unwrap_err();
        assert!(err.to_string().contains("unterminated if block"), "{err}");
    }

}

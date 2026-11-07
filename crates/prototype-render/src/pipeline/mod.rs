//! pipeline — a typed render-pipeline descriptor reconstructed from a
//! decoded SM 3.0 shader pair.
//!
//! The descriptor captures what the bytecode + CTAB + post-END
//! parameter records PROVE about a shader pair: the declared vertex
//! inputs / outputs, the uniform (c#) and sampler (s#) resources with
//! their original names, and the register spans.  Actual GPU binding
//! slot assignment is a LIKELY mapping (documented per field) — the
//! original D3D9 runtime's slot layout is UNKNOWN.

#![forbid(unsafe_code)]

use crate::shader::sm30::{Sm30Error, Sm30Op, Sm30Shader, Usage};

/// What kind of uniform a parameter record / CTAB constant describes.
///
/// The mapping from the record's `type_id` (and the CTAB class/type
/// pair) is empirical — see the table in `sm30.rs` module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UniformType {
    Float,
    Float4,
    Float4x4,
    /// Observed only on `p3dLight_IsLitPass` (type_id 10).
    Bool,
    /// Any other observed type id.
    Unknown(u32),
}

impl UniformType {
    fn from_record(type_id: u32) -> Self {
        match type_id {
            1 => UniformType::Float,
            4 => UniformType::Float4,
            5 => UniformType::Float4x4,
            10 => UniformType::Bool,
            other => UniformType::Unknown(other),
        }
    }

    /// WGSL type name for the uniform.
    pub fn wgsl_type(self) -> &'static str {
        match self {
            UniformType::Float => "f32",
            UniformType::Float4 => "vec4<f32>",
            UniformType::Float4x4 => "mat4x4<f32>",
            UniformType::Bool => "u32",
            UniformType::Unknown(_) => "vec4<f32>",
        }
    }
}

/// A uniform (constant-register) resource.
#[derive(Debug, Clone, PartialEq)]
pub struct UniformDesc {
    /// Original shader name (CTAB / parameter record), e.g.
    /// `"p3dWorldViewProjectionMatrix"`.
    pub name: String,
    /// First constant register (c#).
    pub register: u16,
    /// Register span.
    pub count: u32,
    /// Empirical type.
    pub ty: UniformType,
}

/// A sampler resource.
#[derive(Debug, Clone, PartialEq)]
pub struct SamplerDesc {
    /// Original name, e.g. `"diffuseMap"`.
    pub name: String,
    /// Sampler register (s#).
    pub register: u16,
}

/// One declared vertex input / interpolant.
#[derive(Debug, Clone, PartialEq)]
pub struct VertexInputDesc {
    pub usage: Usage,
    pub usage_index: u8,
    /// Register number (v#).
    pub reg: u16,
}

/// One declared vertex-stage output (interpolant passed to the
/// fragment stage).
#[derive(Debug, Clone, PartialEq)]
pub struct VertexOutputDesc {
    pub usage: Usage,
    pub usage_index: u8,
    /// Register number (o#).
    pub reg: u16,
}

/// A fully described pipeline (shader pair + resources).
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineDescriptor {
    /// The shader-pair name (the P3D ShaderProgram chunk name minus the
    /// `" - Vertex/Fragment Program"` suffix, e.g. `"cxformtexture"`).
    pub name: String,
    /// Uniforms merged from both stages (sorted by register).
    pub uniforms: Vec<UniformDesc>,
    /// Samplers (fragment stage only).
    pub samplers: Vec<SamplerDesc>,
    /// Declared vertex inputs.
    pub vertex_inputs: Vec<VertexInputDesc>,
    /// Declared vertex outputs (interpolants).
    pub vertex_outputs: Vec<VertexOutputDesc>,
    /// Whether the fragment stage samples a texture.
    pub textured: bool,
}

/// Errors while building a descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineError {
    Sm30(Sm30Error),
    /// The fragment shader is missing (a pipeline needs both stages).
    MissingFragment,
    /// The vertex shader is missing.
    MissingVertex,
    /// The same register was declared with two different names/types.
    RegisterConflict(String),
}

impl From<Sm30Error> for PipelineError {
    fn from(e: Sm30Error) -> Self {
        PipelineError::Sm30(e)
    }
}

impl std::fmt::Display for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PipelineError::Sm30(e) => write!(f, "sm30: {e}"),
            PipelineError::MissingFragment => f.write_str("missing fragment shader"),
            PipelineError::MissingVertex => f.write_str("missing vertex shader"),
            PipelineError::RegisterConflict(why) => write!(f, "register conflict: {why}"),
        }
    }
}

impl std::error::Error for PipelineError {}

fn record_type(s: &Sm30Shader) -> impl Iterator<Item = UniformDesc> + '_ {
    s.params.iter().filter_map(|p| {
        // Samplers are handled separately.
        if p.type_id == 6 {
            return None;
        }
        Some(UniformDesc {
            name: p.name.clone(),
            register: p.register_index,
            count: p.register_count,
            ty: UniformType::from_record(p.type_id),
        })
    })
}

impl PipelineDescriptor {
    /// Build a descriptor from a decoded shader pair.
    ///
    /// The uniforms come from the post-END parameter records (whose
    /// agreement with the CTAB is proven — see
    /// [`Sm30Shader::cross_check`]); the vertex inputs/outputs from the
    /// DCL instructions.
    pub fn from_shader_pair(
        name: &str,
        vertex: &Sm30Shader,
        fragment: &Sm30Shader,
    ) -> Result<Self, PipelineError> {
        if vertex.is_fragment {
            return Err(PipelineError::MissingVertex);
        }
        if !fragment.is_fragment {
            return Err(PipelineError::MissingFragment);
        }
        vertex.cross_check().map_err(|e| PipelineError::RegisterConflict(format!("vertex cross-check: {e}")))?;
        fragment.cross_check().map_err(|e| PipelineError::RegisterConflict(format!("fragment cross-check: {e}")))?;

        // Merge uniforms from both stages; the same register must carry
        // the same name and type (they always do — the two programs of
        // a pass are compiled against one constant layout).
        let mut uniforms: Vec<UniformDesc> = Vec::new();
        for u in record_type(vertex).chain(record_type(fragment)) {
            match uniforms.iter_mut().find(|x| x.register == u.register) {
                Some(x) => {
                    if x.name != u.name || x.count != u.count {
                        return Err(PipelineError::RegisterConflict(format!(
                            "register c{}: {:?}/{:?} vs {:?}/{:?}",
                            u.register, x.name, x.count, u.name, u.count
                        )));
                    }
                }
                None => uniforms.push(u),
            }
        }
        uniforms.sort_by_key(|u| u.register);

        let mut samplers: Vec<SamplerDesc> = fragment
            .params
            .iter()
            .filter(|p| p.type_id == 6)
            .map(|p| SamplerDesc { name: p.name.clone(), register: p.register_index })
            .collect();
        samplers.sort_by_key(|s| s.register);
        // Samplers also come from the sampler DCLs; the record names
        // are authoritative, but verify the register sets agree.
        let mut dcl_samplers: Vec<u16> = fragment.samplers().iter().map(|t| t.reg).collect();
        dcl_samplers.sort_unstable();
        let rec_samplers: Vec<u16> = samplers.iter().map(|s| s.register).collect();
        if dcl_samplers != rec_samplers {
            return Err(PipelineError::RegisterConflict(format!(
                "sampler dcls {dcl_samplers:?} != sampler records {rec_samplers:?}"
            )));
        }

        let vertex_inputs = vertex
            .inputs()
            .into_iter()
            .map(|(usage, idx, reg)| VertexInputDesc { usage: *usage, usage_index: idx, reg: reg.reg })
            .collect();
        let vertex_outputs = vertex
            .outputs()
            .into_iter()
            .map(|(usage, idx, reg)| VertexOutputDesc { usage: *usage, usage_index: idx, reg: reg.reg })
            .collect();

        let textured = fragment
            .instructions
            .iter()
            .any(|i| i.op() == Sm30Op::Tex);

        Ok(Self {
            name: name.to_owned(),
            uniforms,
            samplers,
            vertex_inputs,
            vertex_outputs,
            textured,
        })
    }
}

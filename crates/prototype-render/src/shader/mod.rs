//! shader — SM 3.0 bytecode decoding (see [`sm30`]) and WGSL
//! translation (see [`wgsl`]) for the recovered P3D ShaderProgram
//! chunks, plus the DLL's embedded program library and shader-family
//! registry (see [`library`]), the real world-pipeline plan
//! (see [`world`]) and the lit-pass sampler feeds (see
//! [`texture_feeds`]).

#![forbid(unsafe_code)]

pub mod library;
pub mod light_constants;
pub mod plan;
pub mod sm30;
pub mod texture_feeds;
pub mod wgsl;
pub mod world;

pub use library::{program_names, program_pair, FamilyEntry, ProgramRow, Stage, FAMILY_REGISTRY, PROGRAM_LIBRARY};
pub use light_constants::{
    multi_light_family, StandardConstantKind, StandardConstantSpec, ENGINE_MAX_LIGHTS_PER_PASS,
    SINGLE_LIGHT_CONSTANTS, SINGLE_LIGHT_TAIL_CONSTANTS,
};
pub use plan::{
    matrix_rows, plan_stage_resources, renumber_group, uniform_row_bytes, validate_plan_matches_wgsl,
    PlanError, StageBinding, StageResources,
};
pub use texture_feeds::{
    lit_feed_of, lit_pass_feed_plan, engine_table_sampler_order, LitFeed, LitSamplerRow,
    PassParamRow, PassRegSet, ResolverRow, SlotFeed, TextureFeedError, ENGINE_LIT_PASS_TABLE,
    LIT_PASS_SAMPLERS, SHADOW_CHILD_BOX_NAMES, SHADOW_CHILD_INFO_NAMES, TEXTURE_RESOLVER_ROWS,
};
pub use sm30::{
    scan_blobs, BlobRef, Ctab, CtabConstant, Instruction, ParamToken, RegType, ShaderComment,
    ShaderParamRecord, Sm30Error, Sm30Op, Sm30Shader, SrcMod, Usage,
};
pub use wgsl::{
    translate_program, translate_program_with_plan, translate_programs, TranslationReport,
    TranslationStats, VERTEX_POSITION_LOCATION,
};
pub use world::{
    daylight_light_position, daylight_state, default_draw_values, world_texture_assignment,
    CtabNames, WorldError, WorldPairPlan,
};

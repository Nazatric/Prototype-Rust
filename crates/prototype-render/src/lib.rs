//! prototype-render
//! =================
//! Clean-room reconstruction of the Prototype (2009) **render path**:
//! a tiny scene graph (built on [`prototype_math`]), a D3D9 SM 3.0
//! **bytecode** decoder + WGSL translator (so the recovered shader
//! programs can run on a modern API like wgpu), and a render-pipeline
//! descriptor that describes the bindings / vertex layout a future
//! wgpu integration would feed.
//!
//! ## What this crate is, and what it is NOT
//!
//! * **IS** a decoder for the compiled D3D9 SM 3.0 token streams
//!   embedded in every parsed [`prototype_p3d::ShaderProgram`] chunk
//!   (`shader::sm30`) — the instruction-level ground truth of what
//!   each shipped shader does (11 opcodes, 26 programs, all
//!   cross-checked against the inline HLSL + CTAB tables).
//! * **IS** a symbolic translator from that decoded bytecode to WGSL
//!   (`shader::wgsl`) — every emitted statement derives from one
//!   decoded instruction, quoted as a comment above it.
//! * **IS** a tiny scene-graph + camera model (PROVEN math from
//!   `prototype_math::Mat4::perspective_rh_zo` /
//!   `look_at_rh` / `Transform`) that a real renderer can drive.
//! * **IS NOT** a wgpu integration — actually creating a wgpu
//!   device / swapchain / render pass is OUT OF SCOPE for this
//!   crate (would require a GPU; the workspace is sandboxed).  The
//!   crate produces WGSL SOURCE STRINGS + a typed pipeline
//!   descriptor; a separate `prototype-bin` example can pick those
//!   up when a GPU is available.
//!
//! ## Confidence labels
//!
//! * PROVEN: the SM 3.0 token layout, the 11-opcode shipped
//!   instruction set, the CTAB constant-table layout and the
//!   post-END `0x0001100D` parameter records — all derived
//!   empirically from the 26 shipped programs and cross-checked
//!   against the inline HLSL sources (`shader::sm30` docs).
//! * PROVEN: every shader's `version_token` is `0xFFFE0300`
//!   (`vs_3_0`) or `0xFFFF0300` (`ps_3_0`) — SM 3.0 is the floor.
//! * LIKELY: the WGSL `@location` / `@binding` numbering (one
//!   deterministic scheme shared by both stages of a pair); the
//!   original D3D9 slot layout is UNKNOWN.
//! * UNKNOWN: the semantics of the parameter records' `register_sub`
//!   byte (0, or 1 on `p3dLight_IsLitPass`) and the constant
//!   `{16, 16}` fields.

#![forbid(unsafe_code)]

pub mod scene;
pub mod shader;
pub mod pipeline;
pub mod dxt;
pub mod video_quad;

/// Block 107 — the menu/UI text layer: bundled-font atlas + ortho
/// quad renderer.  The native draw path for the REAL `art\hud\*.gfx`
/// screen catalog (proven from the engine DLL strings) until the
/// Scaleform shapes themselves are extracted; the frontend sprite
/// shader contract it will ride on is block 101's
/// [`render_frontend_cpu`] (the REAL cxformtexture/cxformsolidcolour
/// pairs from `frontendshaders.p3d`).
pub mod ui;

/// Root re-exports: the menu layer's CPU types (every caller wants
/// them without the `ui::` hop).
pub use ui::{FontAtlas, UiDrawList, UiVertex};
/// Block 76 — the RENDER_ART_WORLD_PATCH harness's device-free CPU
/// mirror: real `SceneMesh` + proven camera math + daylight fragment
/// rows → RGBA8 → PNG evidence (sandbox-runnable; the GPU path stays
/// USER-SIDE-first because the sandbox has no wgpu adapter).
pub mod render_art_world_cpu;
/// Block 78 — the REAL decoded NIS camera takes (block 77) drive the
/// render path: real `CAM_NisCam_Take` TRAN/LOOK keyframes, mounted
/// scene-local → art space (one uniform scale + translation), feed
/// [`Camera`] per frame → CPU-mirror RGBA8 → contact-sheet evidence.
/// A real in-engine cutscene camera walkthrough — NOT synthetic paths.
pub mod cutscene_walkthrough;
/// Block 101 — the device-free CPU mirror of the FRONTEND sprite
/// pipeline: the REAL shipped `cxformtexture` / `cxformsolidcolour`
/// shader pairs (register contract pinned against the committed
/// `frontendshaders.p3d` bytes) rasterized on the CPU — the sandbox
/// reference for the loader's frontend menu/HUD path (NEXT-UP #7's
/// last piece: a frontend render test).
pub mod render_frontend_cpu;
#[cfg(feature = "render-wgpu")]
pub mod wgpu_backend;

pub use pipeline::PipelineDescriptor;
pub use scene::{Camera, Scene, SceneNode};
pub use shader::{translate_program, translate_programs, TranslationReport, TranslationStats};

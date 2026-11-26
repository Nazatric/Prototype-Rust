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

//// Block 102 — the BINK movie shader pair's CPU port: consumes the
/// REAL shipped `dll_bink_fp_e3adf8.sm30` register contract
/// (y_tex@s0, cr_tex@s1, cb_tex@s2, a_tex@s3) and produces RGBA8 via
/// the BT.601 limited-range matrix the FP prescribes (block 117: the
/// content-side line's renderer, ported).
pub mod render_bink_shader;
/// Block 103 — the LEGACY world shader pair's CPU port: the REAL
/// shipped VS/PS register contracts and the FULL Blinn-Phong chain
/// (block 117: ported).
pub mod render_legacy_shader;
/// Block 104 — the FRONTEND TEXT shader pair's CPU port: the REAL
/// `gfxtextshader` register contract, rasterized on the CPU with alpha
/// blending — the first real text/HUD glyph rendering (block 117:
/// ported).
pub mod render_gfxtext_cpu;

/// world_map — CPU rendering of the REAL streamed-world placement
/// database (blocks 18/41 proven decoders): the real map top-down +
/// 3D flythrough (block 117: ported).
pub mod world_map;

// Root re-exports: the menu layer's CPU types (every caller wants
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
/// Block 120 — the REAL Scaleform `.gfx` menu screens: the SWF
/// container parser (the `art\hud\*.gfx` members the boot census
/// proves present on-device) + the first-frame CPU renderer through
/// the REAL frontend shader contract.  Bitmaps (DefineBitsLossless2)
/// and placements render; shapes/fonts/ActionScript are recorded and
/// skipped — honest scopes, never guesses.
pub mod frontend_gfx;
/// Block 121 — the VECTOR half of the real screens: DefineShape
/// 1/2/3 + DefineEditText + DefineSprite parsing, the fill-side edge
/// chaining, the even-odd scanline filler and the real-text
/// rasterizer.  The block-120 journal proved the real screens carry
/// ZERO bitmaps — this is what actually draws them.
pub mod frontend_gfx_shapes;
#[cfg(feature = "render-wgpu")]
pub mod wgpu_backend;

pub use pipeline::PipelineDescriptor;
pub use scene::{Camera, Scene, SceneNode};
pub use shader::{translate_program, translate_programs, TranslationReport, TranslationStats};

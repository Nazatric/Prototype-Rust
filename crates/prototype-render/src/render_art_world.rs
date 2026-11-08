//! render_art_world — the GPU edge of the RENDER_ART_WORLD_PATCH
//! harness (block 80; rebuilt byte-faithfully after the block-80/81
//! sandbox recycle rollback — same API, same pins, plus one fix: the
//! offscreen pass now attaches a Depth32Float buffer so GPU occlusion
//! matches the CPU mirror's z-buffer).
//!
//! ## What this module is
//!
//! The DEVICE side of [`super::render_art_world_cpu`]: the SAME inputs
//! — a real [`SceneMesh`] (block-16 M21b decode), the SAME
//! [`Camera::view_projection`] rows, the mesh's REAL decoded diffuse
//! DDS, and the SAME daylight fragment DESIGN rows
//! ([`super::shader::world::daylight_state`]) — but drawn through the
//! REAL translated DLL `default` shader pair (the
//! [`super::shader::world::WorldPairPlan`]'s WGSL) on a real wgpu
//! device, into an offscreen RGBA8 target.
//!
//! The CPU mirror stays the sandbox evidence path (this host has no
//! wgpu adapter — the offscreen test below SKIPS here and runs on the
//! first GPU host, USER-SIDE first).
//!
//! ## Public surface (the harness tools)
//!
//! * [`OffscreenDevice`] / [`request_offscreen_device`] — a
//!   device+queue pair with NO surface (the only async edge; powered
//!   by `pollster::block_on`, the same helper the android app uses).
//! * [`WorldPipeline`] — the `default` pair compiled ONCE for a target
//!   format, carrying the validated bind-group layouts, the vertex
//!   layout (DLL default VS: 5 inputs × 16 B = 80 B stride) and the
//!   full binding plans.
//! * [`offscreen_render`] — one real mesh + camera + optional diffuse
//!   through the pipeline → [`OffscreenFrame`] (tight RGBA8 + the SAME
//!   [`super::render_art_world_cpu::FrameStats`] non-blank proof the
//!   CPU mirror reports).
//! * [`padded_bytes_per_row`] — the 256-B row alignment every RGBA8
//!   texture↔buffer copy must respect (pinned by test).
//!
//! ## CTAB pins (PROVEN by test against the committed blobs)
//!
//! * WVP at c0 (`p3dWorldViewProjectionMatrix`), world matrix at c4
//!   (`p3dWorldMatrix`) — the vertex rows `offscreen_render` writes.
//! * Samplers `diffuseMap` s4 / `normalMap` s5 / `specularMap` s6 —
//!   NOT s0..s3: the engine light/shadow textures sit there and stay
//!   on the shared white fallback (see
//!   [`super::shader::world::world_texture_assignment`]).
//! * Vertex locations: position@32, normal@19, texcoord0@0,
//!   texcoord6@6, texcoord7@7 (the deterministic
//!   [`super::shader::wgsl::interpolant_location`] scheme).
//!
//! ## Documented approximations
//!
//! * No MSAA, no mipmaps (base surface only — same as the CPU mirror).
//! * Secondary UV sets (texcoord6/7) upload ZEROS: the engine feeds
//!   them light-projection coordinates this loader does not compute.
//! * Meshes without normals get a constant [0, 0, 1] vertex normal
//!   (the CPU mirror computes FACE normals per triangle instead).
//! * `FrameStats::triangles_clipped` is always 0 here: the GPU clipper
//!   does not report rejects; `triangles_drawn` counts the submitted
//!   index triples.
//! * The vertex/index/uniform buffers and the bind groups are created
//!   per call — the harness renders single frames, not scenes; the
//!   engine-side batch path (wgpu_backend's `create_world_mesh_batches`)
//!   is the production shape.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use crate::render_art_world_cpu::FrameStats;
use crate::scene::{Camera, SceneMesh};
use crate::shader::plan::{uniform_row_bytes, StageBinding, StageResources, StageValues};
use crate::shader::sm30::Usage;
use crate::shader::world::{
    daylight_light_position, daylight_state, default_draw_values, world_texture_assignment,
    WorldPairPlan,
};
use crate::wgpu_backend::{bind_group_layout_entries, plan_vertex_layout, MeshVertexLayout, WgpuError};

// ---------------------------------------------------------------------------
// The offscreen device
// ---------------------------------------------------------------------------

/// A wgpu `Device` + `Queue` pair with NO surface — everything the
/// offscreen harness needs (the surface-bound [`crate::wgpu_backend::Renderer`]
/// is the on-screen edge).  The only async edge in this module; created
/// via [`request_offscreen_device`].
pub struct OffscreenDevice {
    /// The device (adapter-agnostic; created with default limits).
    pub device: wgpu::Device,
    /// The queue submits the harness's single encoded pass.
    pub queue: wgpu::Queue,
}

/// Request the offscreen device, blocking on the wgpu futures with
/// `pollster` (the same helper the android app's boot path uses).
///
/// Fails with [`WgpuError::NoAdapter`] / [`WgpuError::Adapter`] when
/// the host has no usable GPU — the caller decides whether that is a
/// skip (tests) or a fatal boot error (the app).
pub fn request_offscreen_device() -> Result<OffscreenDevice, WgpuError> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = match pollster::block_on(instance.request_adapter(
        &wgpu::RequestAdapterOptions::default(),
    )) {
        Ok(a) => a,
        Err(e) => return Err(WgpuError::Adapter(e.to_string())),
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("prototype-rs render-art-world offscreen"),
        ..Default::default()
    }))?;
    Ok(OffscreenDevice { device, queue })
}

// ---------------------------------------------------------------------------
// The compiled world pair
// ---------------------------------------------------------------------------

/// The DLL `default` shader pair compiled ONCE for one target format:
/// the render pipeline plus everything [`offscreen_render`] needs to
/// build bind groups per draw — the validated bind-group layouts, the
/// full binding plans, the vertex layout, and the originating plan
/// (for the CTAB name maps and the daylight rows).
pub struct WorldPipeline {
    /// The compiled render pipeline (vs group 0, fs group 1).
    pub pipeline: wgpu::RenderPipeline,
    /// The vertex-stage bind group layout (group 0).
    pub vertex_bgl: wgpu::BindGroupLayout,
    /// The fragment-stage bind group layout (group 1).
    pub fragment_bgl: wgpu::BindGroupLayout,
    /// The full vertex-stage binding plan (uniform rows in plan order).
    pub vertex_resources: StageResources,
    /// The full fragment-stage binding plan (uniforms + textures + samplers).
    pub fragment_resources: StageResources,
    /// The vertex buffer layout (stride 80 B on the DLL default pair).
    pub vertex_layout: MeshVertexLayout,
    /// The originating world plan (CTAB names + final WGSL text).
    pub plan: WorldPairPlan,
}

impl WorldPipeline {
    /// Compile the pair's FINAL WGSL (`plan.vs_wgsl` group 0,
    /// `plan.fs_wgsl` group 1 — already renumbered and cross-validated
    /// by [`WorldPairPlan::from_blobs`]) into one pipeline for
    /// `target_format` (the offscreen harness uses `Rgba8Unorm`).
    ///
    /// The pipeline differs from the debug path's
    /// [`crate::wgpu_backend::create_render_pipeline`] in exactly the
    /// ways the harness needs: `cull_mode: None` (the CPU mirror also
    /// draws both windings) and a Depth32Float attachment so
    /// self-occluding meshes occlude correctly (the mirror's z-buffer).
    pub fn new(
        device: &wgpu::Device,
        plan: WorldPairPlan,
        target_format: wgpu::TextureFormat,
    ) -> Self {
        let vs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("prototype-rs world pair — vs"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(&plan.vs_wgsl)),
        });
        let fs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("prototype-rs world pair — fs"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(&plan.fs_wgsl)),
        });
        let vertex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("prototype-rs world pair — vertex resources"),
            entries: &bind_group_layout_entries(&plan.vertex_resources, wgpu::ShaderStages::VERTEX),
        });
        let fragment_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("prototype-rs world pair — fragment resources"),
            entries: &bind_group_layout_entries(
                &plan.fragment_resources,
                wgpu::ShaderStages::FRAGMENT,
            ),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("prototype-rs world pair — layout"),
            bind_group_layouts: &[&vertex_bgl, &fragment_bgl],
            push_constant_ranges: &[],
        });
        let vertex_layout = plan_vertex_layout(&plan.vs);
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("prototype-rs world pair pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &vs_module,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[vertex_layout.as_layout()],
            },
            fragment: Some(wgpu::FragmentState {
                module: &fs_module,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    // Opaque replace — the world draw IS the frame.
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                // The CPU mirror draws both windings; so does the
                // harness (the engine's own cull policy for the pair
                // is UNKNOWN).
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        Self {
            pipeline,
            vertex_bgl,
            fragment_bgl,
            vertex_resources: plan.vertex_resources.clone(),
            fragment_resources: plan.fragment_resources.clone(),
            vertex_layout,
            plan,
        }
    }
}

// ---------------------------------------------------------------------------
// Row alignment
// ---------------------------------------------------------------------------

/// Bytes per row of an RGBA8 `width`-wide texture↔buffer copy, padded
/// up to wgpu's 256-byte `COPY_BYTES_PER_ROW_ALIGNMENT` (8 px → 256 B,
/// 65 px → 512 B, 320 px → 1280 B — pinned by test).
pub fn padded_bytes_per_row(width: u32) -> u32 {
    let tight = width.saturating_mul(4);
    tight.div_ceil(256).saturating_mul(256)
}

// ---------------------------------------------------------------------------
// Uniform rows + bind-group entries (borrow-scoped helpers)
// ---------------------------------------------------------------------------

/// One 16-byte uniform buffer per uniform row of a stage's plan,
/// filled from `values` (rows never set upload ZEROS — the documented
/// [`StageValues::encode`] default; bools encode `[1,0,0,0]`/zeros).
///
/// Buffers are created `mapped_at_creation`, filled, and unmapped —
/// the caller owns them for the lifetime of the bind group (keeping
/// this helper free of self-referential borrows).
fn build_uniform_row_buffers(
    device: &wgpu::Device,
    res: &StageResources,
    values: &StageValues,
    label_stage: &str,
) -> Vec<wgpu::Buffer> {
    let float_rows: BTreeMap<u16, [u8; 16]> =
        values.floats().map(|(r, v)| (r, uniform_row_bytes(v))).collect();
    let bool_rows: BTreeMap<u16, [u8; 16]> = values
        .bools()
        .map(|(r, v)| {
            let row: [f32; 4] = if v { [1.0, 0.0, 0.0, 0.0] } else { [0.0; 4] };
            (r, uniform_row_bytes(row))
        })
        .collect();
    let mut out = Vec::new();
    for (i, b) in res.bindings.iter().enumerate() {
        let bytes: [u8; 16] = match b {
            StageBinding::Uniform { register, .. } => {
                float_rows.get(register).copied().unwrap_or([0u8; 16])
            }
            StageBinding::BoolUniform { register, .. } => {
                bool_rows.get(register).copied().unwrap_or([0u8; 16])
            }
            _ => continue,
        };
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!(
                "prototype-rs world pair — {label_stage} uniform row #{i}"
            )),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: true,
        });
        {
            let mut mapping = buffer.slice(..).get_mapped_range_mut();
            mapping.copy_from_slice(&bytes);
        }
        buffer.unmap();
        out.push(buffer);
    }
    out
}

/// Bind-group entries for a stage's plan, in plan order:
///
/// * `Uniform` / `BoolUniform` → the per-row uniform buffer
///   (`uniform_buffers` parallel to the plan's rows — built by
///   [`build_uniform_row_buffers`]);
/// * `Texture` → the override view for that s# register when
///   `texture_overrides` has one, else the shared white fallback;
/// * `Sampler` → the one shared sampler.
///
/// All borrows come from the CALLER's scope (buffers, views, sampler
/// live in `offscreen_render`'s frame), so no self-referential struct
/// is needed.
fn build_bind_group_entries<'a>(
    res: &StageResources,
    uniform_buffers: &'a [&wgpu::Buffer],
    white_view: &'a wgpu::TextureView,
    sampler: &'a wgpu::Sampler,
    texture_overrides: &BTreeMap<u16, &'a wgpu::TextureView>,
) -> Vec<wgpu::BindGroupEntry<'a>> {
    let mut uniform_iter = uniform_buffers.iter();
    let mut out = Vec::new();
    for b in &res.bindings {
        let entry = match b {
            StageBinding::Uniform { binding, .. } | StageBinding::BoolUniform { binding, .. } => {
                let buffer = uniform_iter
                    .next()
                    .expect("uniform_buffers parallel to the plan's uniform rows");
                wgpu::BindGroupEntry {
                    binding: *binding,
                    resource: buffer.as_entire_binding(),
                }
            }
            StageBinding::Texture { register, binding, .. } => wgpu::BindGroupEntry {
                binding: *binding,
                resource: wgpu::BindingResource::TextureView(
                    texture_overrides.get(register).copied().unwrap_or(white_view),
                ),
            },
            StageBinding::Sampler { binding, .. } => wgpu::BindGroupEntry {
                binding: *binding,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        };
        out.push(entry);
    }
    out
}

// ---------------------------------------------------------------------------
// One offscreen frame
// ---------------------------------------------------------------------------

/// One rendered GPU frame plus its non-blank statistics — the GPU
/// counterpart of [`crate::render_art_world_cpu::CpuMirrorFrame`] (the
/// SAME [`FrameStats`] shape, so one verifier covers both paths).
pub struct OffscreenFrame {
    /// Texel width.
    pub width: u32,
    /// Texel height.
    pub height: u32,
    /// Packed RGBA8, row-major, tight rows, `width*height*4` B.
    pub rgba: Vec<u8>,
    /// The non-blank proof (distinct values + channel extrema +
    /// submitted triangle count).
    pub stats: FrameStats,
}

/// Render one real mesh through the REAL `default` pair.
///
/// * `pipeline` — the pair compiled for `Rgba8Unorm` targets
///   ([`WorldPipeline::new`]).
/// * `mesh` — the REAL decoded art mesh; positions/normals/first UV
///   set interleave into the DLL default VS's 80-B vertex rows.
/// * `camera` — the IN-GAME camera model; its
///   [`Camera::view_projection_rows`] land on `c0..c3`
///   (`p3dWorldViewProjectionMatrix`), identity on `c4..c7`
///   (`p3dWorldMatrix`), the far sun position on the VS's
///   `p3dLight_PositionObjectSpace` row.
/// * `texture` — the diffuse RGBA8 `((bytes, width, height))`; `None`
///   binds the shared 1×1 white fallback (the engine light/shadow
///   textures ALWAYS bind white — see
///   [`world_texture_assignment`]).
/// * `clear` — the background RGBA8 (the same `[u8; 4]` the CPU
///   mirror takes, so the two frames are comparable).
///
/// The fragment stage runs the LIT pass: `default_draw_values`
/// (white `materialDiffuse`, `p3dLight_IsLitPass = true`) merged with
/// the [`daylight_state`] DESIGN rows — the same values the CPU mirror
/// applies, so the first GPU PNG and the shipped CPU proxy must show
/// the same lit world.
pub fn offscreen_render(
    pipeline: &WorldPipeline,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    mesh: &SceneMesh,
    camera: &Camera,
    texture: Option<(&[u8], u32, u32)>,
    width: u32,
    height: u32,
    clear: [u8; 4],
) -> Result<OffscreenFrame, WgpuError> {
    if width == 0 || height == 0 {
        return Err(WgpuError::PlanMismatch(
            "render_art_world: width/height must be non-zero".into(),
        ));
    }
    let n_verts = mesh.positions.len();
    if mesh.indices.is_empty() || n_verts == 0 {
        return Err(WgpuError::PlanMismatch(format!(
            "render_art_world: mesh {:?} has no drawable geometry",
            mesh.name
        )));
    }

    // --- per-draw uniform values --------------------------------------
    let (mut vs_values, mut fs_values) = default_draw_values(&pipeline.plan);
    let wvp_base = pipeline
        .plan
        .wvp_register()
        .map_err(|e| WgpuError::PlanMismatch(format!("wvp register: {e}")))?;
    for (i, row) in camera.view_projection_rows().iter().enumerate() {
        vs_values.set_float(wvp_base + i as u16, *row);
    }
    if let Ok(Some((reg, row))) = daylight_light_position(&pipeline.plan) {
        vs_values.set_float(reg, row);
    }
    let daylight = daylight_state(&pipeline.plan)
        .map_err(|e| WgpuError::PlanMismatch(format!("daylight rows: {e}")))?;
    for (reg, row) in daylight.floats() {
        fs_values.set_float(reg, row);
    }
    for (reg, on) in daylight.bools() {
        fs_values.set_bool(reg, on);
    }

    // --- vertex rows: interleave per the DLL default VS's DCL order ---
    let stride = pipeline.vertex_layout.stride as usize;
    let inputs = pipeline.plan.vs.inputs();
    let mut vertex_bytes = vec![0u8; n_verts * stride];
    for (vi, pos) in mesh.positions.iter().enumerate() {
        let base = vi * stride;
        for (ai, (usage, usage_index, _reg)) in inputs.iter().enumerate() {
            let off = base + ai * 16;
            let row: [f32; 4] = match (usage, *usage_index) {
                (Usage::Position, 0) => [pos[0], pos[1], pos[2], 1.0],
                (Usage::Normal, 0) => {
                    if vi < mesh.normals.len() {
                        let n = mesh.normals[vi];
                        [n[0], n[1], n[2], 0.0]
                    } else {
                        [0.0, 0.0, 1.0, 0.0]
                    }
                }
                (Usage::TexCoord, 0) => {
                    if vi < mesh.uvs.len() {
                        let uv = mesh.uvs[vi];
                        [uv[0], uv[1], 0.0, 0.0]
                    } else {
                        [0.5, 0.5, 0.0, 0.0]
                    }
                }
                // Secondary UV sets: the engine feeds light-projection
                // coordinates here — zeros are the documented fallback.
                _ => [0.0; 4],
            };
            vertex_bytes[off..off + 16].copy_from_slice(&uniform_row_bytes(row));
        }
    }
    let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("render-art-world harness — vertices"),
        size: vertex_bytes.len() as u64,
        usage: wgpu::BufferUsages::VERTEX,
        mapped_at_creation: true,
    });
    {
        let mut mapping = vertex_buffer.slice(..).get_mapped_range_mut();
        mapping.copy_from_slice(&vertex_bytes);
    }
    vertex_buffer.unmap();

    // --- indices (SceneMesh indices are u16) ---------------------------
    let mut index_bytes = Vec::with_capacity(mesh.indices.len() * 2);
    for ix in &mesh.indices {
        index_bytes.extend_from_slice(&ix.to_le_bytes());
    }
    let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("render-art-world harness — indices"),
        size: index_bytes.len() as u64,
        usage: wgpu::BufferUsages::INDEX,
        mapped_at_creation: true,
    });
    {
        let mut mapping = index_buffer.slice(..).get_mapped_range_mut();
        mapping.copy_from_slice(&index_bytes);
    }
    index_buffer.unmap();

    // --- textures: white fallback + the real diffuse override ----------
    let white_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("render-art-world harness — white fallback"),
        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &white_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &[255u8, 255, 255, 255],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: None, // 1×1: a single row — tight is valid
            rows_per_image: None,
        },
        wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
    );
    let white_view = white_texture.create_view(&wgpu::TextureViewDescriptor::default());

    // The diffuse upload, when the caller has one: rows padded to the
    // 256-B copy alignment (a no-op memcpy per row for the 256² art
    // diffuse — 1024 B rows are already aligned).
    let mut diffuse_view: Option<wgpu::TextureView> = None;
    if let Some((bytes, tw, th)) = texture {
        if tw > 0 && th > 0 && bytes.len() >= (tw as usize) * (th as usize) * 4 {
            let diffuse = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("render-art-world harness — diffuse"),
                size: wgpu::Extent3d { width: tw, height: th, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let padded = padded_bytes_per_row(tw) as usize;
            let tight = (tw as usize) * 4;
            let staging: Vec<u8> = if padded == tight {
                bytes[..(tw as usize) * (th as usize) * 4].to_vec()
            } else {
                let mut s = vec![0u8; padded * th as usize];
                for y in 0..th as usize {
                    s[y * padded..y * padded + tight]
                        .copy_from_slice(&bytes[y * tight..y * tight + tight]);
                }
                s
            };
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &diffuse,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &staging,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded as u32),
                    rows_per_image: None,
                },
                wgpu::Extent3d { width: tw, height: th, depth_or_array_layers: 1 },
            );
            diffuse_view = Some(diffuse.create_view(&wgpu::TextureViewDescriptor::default()));
        }
    }

    // Which s# registers get the diffuse: by CTAB NAME (s4 = diffuseMap;
    // the engine light/shadow textures stay white) — the PROVEN
    // world-pair assignment, with one upload available.
    let mut texture_overrides: BTreeMap<u16, &wgpu::TextureView> = BTreeMap::new();
    if let Some(view) = diffuse_view.as_ref() {
        let tex_regs: Vec<u16> = pipeline
            .plan
            .fragment_resources
            .bindings
            .iter()
            .filter_map(|b| match b {
                StageBinding::Texture { register, .. } => Some(*register),
                _ => None,
            })
            .collect();
        for (reg, slot) in tex_regs.iter().zip(world_texture_assignment(&pipeline.plan, 1)) {
            if slot == Some(0) {
                texture_overrides.insert(*reg, view);
            }
        }
    }

    // --- the shared sampler (nearest + wrap: the CPU mirror's model) ---
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("render-art-world harness — sampler"),
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::FilterMode::Nearest,
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        ..Default::default()
    });

    // --- uniform buffers + bind groups ---------------------------------
    let vertex_uniforms =
        build_uniform_row_buffers(device, &pipeline.vertex_resources, &vs_values, "vs");
    let fragment_uniforms =
        build_uniform_row_buffers(device, &pipeline.fragment_resources, &fs_values, "fs");
    let vertex_uniform_refs: Vec<&wgpu::Buffer> = vertex_uniforms.iter().collect();
    let fragment_uniform_refs: Vec<&wgpu::Buffer> = fragment_uniforms.iter().collect();
    let vertex_entries = build_bind_group_entries(
        &pipeline.vertex_resources,
        &vertex_uniform_refs,
        &white_view,
        &sampler,
        &BTreeMap::new(), // the vertex stage declares no textures
    );
    let fragment_entries = build_bind_group_entries(
        &pipeline.fragment_resources,
        &fragment_uniform_refs,
        &white_view,
        &sampler,
        &texture_overrides,
    );
    let vertex_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("render-art-world harness — vertex group"),
        layout: &pipeline.vertex_bgl,
        entries: &vertex_entries,
    });
    let fragment_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("render-art-world harness — fragment group"),
        layout: &pipeline.fragment_bgl,
        entries: &fragment_entries,
    });

    // --- the offscreen target + depth -----------------------------------
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("render-art-world harness — colour target"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("render-art-world harness — depth"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());

    // --- the pass: one indexed draw through the REAL pair ---------------
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("render-art-world harness — pass"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("render-art-world harness — world draw"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: clear[0] as f64 / 255.0,
                        g: clear[1] as f64 / 255.0,
                        b: clear[2] as f64 / 255.0,
                        a: clear[3] as f64 / 255.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, &vertex_group, &[]);
        pass.set_bind_group(1, &fragment_group, &[]);
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint16);
        pass.draw_indexed(0..mesh.indices.len() as u32, 0, 0..1);
    }

    // --- readback: 256-B-aligned rows → tight RGBA8 ---------------------
    let padded = padded_bytes_per_row(width);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("render-art-world harness — readback"),
        size: padded as u64 * height as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    queue.submit(Some(encoder.finish()));

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    // The poll reports queue-empty state; the mapped range below is
    // the actual result channel (the proven video-quad readback form).
    let _ = device.poll(wgpu::PollType::Wait);
    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    {
        let data = slice.get_mapped_range();
        let tight = (width as usize) * 4;
        for y in 0..height as usize {
            let src = y * padded as usize;
            rgba[y * tight..y * tight + tight]
                .copy_from_slice(&data[src..src + tight]);
        }
    }
    readback.unmap();

    // The GPU clipper does not report rejects — triangles_drawn counts
    // the submitted index triples (the harness submits one draw).
    let stats =
        crate::render_art_world_cpu::frame_stats(&rgba, mesh.indices.len() / 3, 0);
    Ok(OffscreenFrame { width, height, rgba, stats })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::SceneMesh;
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

    /// The DEVICE-FREE pins (runnable on every host, GPU or not): the
    /// CTAB registers the harness writes (WVP c0, world c4), the
    /// sampler registers the harness feeds (s4/s5/s6 — NOT the engine
    /// light/shadow s0..s3), the 5 vertex locations of the DLL default
    /// VS's 80-B rows, and the two bind group layouts' visibility and
    /// entry classes.
    #[test]
    fn device_free_world_pair_plan_pins_vertex_layout_and_samplers() {
        let plan = real_plan();

        // The rows offscreen_render writes.
        assert_eq!(plan.wvp_register().unwrap(), 0, "WVP at c0");
        assert_eq!(plan.world_matrix_register().unwrap(), 4, "world matrix at c4");

        // The material slots by CTAB name (s4/s5/s6 — the engine
        // light/shadow textures sit at s0..s3 and stay white).
        assert_eq!(plan.fragment_names.sampler_register("diffuseMap").unwrap(), 4);
        assert_eq!(plan.fragment_names.sampler_register("normalMap").unwrap(), 5);
        assert_eq!(plan.fragment_names.sampler_register("specularMap").unwrap(), 6);

        // The DLL default VS declares 5 inputs → 80-B vertex rows.
        let layout = plan_vertex_layout(&plan.vs);
        assert_eq!(layout.stride, 80, "5 inputs × 16 B");
        let mut locations: Vec<u32> =
            layout.attributes.iter().map(|a| a.shader_location).collect();
        locations.sort_unstable();
        assert_eq!(locations, vec![0, 6, 7, 19, 32],
            "texcoord0@0, texcoord6@6, texcoord7@7, normal@19, position@32");

        // The vertex BGL: all VERTEX-visible, all buffers.
        let vbgl = bind_group_layout_entries(&plan.vertex_resources, wgpu::ShaderStages::VERTEX);
        assert!(!vbgl.is_empty());
        assert!(vbgl.iter().all(|e| e.visibility == wgpu::ShaderStages::VERTEX));
        assert!(vbgl.iter().all(|e| matches!(
            e.ty,
            wgpu::BindingType::Buffer { .. }
        )));

        // The fragment BGL: all FRAGMENT-visible, at least one buffer
        // (the material/light rows) and one sampler (diffuseMap & co).
        let fbgl =
            bind_group_layout_entries(&plan.fragment_resources, wgpu::ShaderStages::FRAGMENT);
        assert!(!fbgl.is_empty());
        assert!(fbgl.iter().all(|e| e.visibility == wgpu::ShaderStages::FRAGMENT));
        assert!(fbgl
            .iter()
            .any(|e| matches!(e.ty, wgpu::BindingType::Buffer { .. })));
        assert!(fbgl
            .iter()
            .any(|e| matches!(e.ty, wgpu::BindingType::Sampler(_))));
    }

    /// The RGBA8 row-alignment arithmetic (every width the harness can
    /// render): 8 px → 256 B, 64 px → 256 B, 65 px → 512 B, 320 px →
    /// 1280 B, 321 px → 1536 B.
    #[test]
    fn padded_bytes_per_row_aligns_to_256() {
        assert_eq!(padded_bytes_per_row(8), 256);
        assert_eq!(padded_bytes_per_row(64), 256);
        assert_eq!(padded_bytes_per_row(65), 512);
        assert_eq!(padded_bytes_per_row(320), 1280);
        assert_eq!(padded_bytes_per_row(321), 1536);
    }

    /// THE GPU PATH: the real barrier mesh + its real in-member 256²
    /// diffuse through the REAL translated `default` pair, offscreen,
    /// with the same camera framing + clear colour the CPU-mirror
    /// evidence uses — the first-run PNG must be non-blank and encode
    /// through the proven zero-dependency PNG encoder.
    ///
    /// SKIPS on hosts without a wgpu adapter (this sandbox: the
    /// request fails with a verbose "no suitable adapter" error — both
    /// the `NoAdapter` and `Adapter(_)` arms skip cleanly).
    #[test]
    fn offscreen_render_through_real_default_pair_skips_without_an_adapter() {
        let offscreen = match request_offscreen_device() {
            Ok(d) => d,
            Err(WgpuError::NoAdapter) => {
                eprintln!("skip: no wgpu adapter on this host (render-art-world GPU test)");
                return;
            }
            Err(WgpuError::Adapter(msg)) => {
                eprintln!(
                    "skip: no wgpu adapter on this host (render-art-world GPU test): {msg}"
                );
                return;
            }
            Err(e) => panic!("unexpected offscreen device error: {e}"),
        };

        // The REAL barrier mesh (576 verts / 358 tris) + diffuse.
        let buf = fixture("art_barrier_os002.p3d");
        let top = prototype_p3d::geometry::top_level_chunks(&buf).unwrap();
        let shape = top
            .iter()
            .find(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_SHAPE)
            .map(|c| prototype_p3d::geometry::decode_shape(&buf, c.offset, c.size).unwrap())
            .unwrap();
        let decoded = shape.meshes.first().expect("the barrier shape has a mesh");
        let mesh = SceneMesh::from_p3d_mesh(decoded);
        assert_eq!(mesh.triangle_count(), 358);

        let dds = top
            .iter()
            .find(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_TEXTURE)
            .map(|c| prototype_p3d::geometry::decode_texture(&buf, c.offset, c.size).unwrap().dds)
            .expect("the barrier member embeds its diffuse");
        let surfaces = crate::dxt::decode_dds_mips(&dds).expect("diffuse decodes");
        let base = surfaces.first().unwrap();
        assert_eq!((base.width, base.height), (256, 256));

        // The pipeline: the REAL pair compiled once for RGBA8.
        let pipeline =
            WorldPipeline::new(&offscreen.device, real_plan(), wgpu::TextureFormat::Rgba8Unorm);

        // The same framing + clear the CPU-mirror evidence uses.
        let mut cam = crate::render_art_world_cpu::frame_camera_for(&mesh);
        cam.aspect = 320.0 / 180.0;
        let frame = offscreen_render(
            &pipeline,
            &offscreen.device,
            &offscreen.queue,
            &mesh,
            &cam,
            Some((&base.rgba, base.width, base.height)),
            320,
            180,
            [10, 12, 16, 255],
        )
        .expect("the barrier renders through the real pair on a real GPU");

        assert!(frame.stats.distinct > 1, "non-blank: {:?}", frame.stats);
        assert!(frame.stats.triangles_drawn > 0, "triangles submitted");
        let png = prototype_video::png::png_encode_rgba(frame.width, frame.height, &frame.rgba)
            .expect("PNG encode");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert!(png.len() > 4_000, "a real frame PNG, {} B", png.len());
    }
}

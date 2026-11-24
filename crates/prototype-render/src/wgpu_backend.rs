//! wgpu_backend — maps the decoded SM 3.0 shader pairs to REAL wgpu
//! pipelines.
//!
//! # Architecture
//!
//! The layer is split so that everything that can be verified without a
//! GPU **is verified without a GPU**, and the device-touching edge is
//! thin and compile-gated:
//!
//! * [`plan_stage_resources`] — derives the exact per-stage binding list
//!   (one 16-byte uniform row per c# register row, texture+sampler pairs
//!   per s#) using the SAME arithmetic as the WGSL translator
//!   (`shader::wgsl`), and [`validate_plan_matches_wgsl`] cross-checks a
//!   plan against actually-emitted WGSL text.  Tested against the real
//!   frontend shaders.
//! * [`plan_vertex_layout`] — the vertex buffer layout: one
//!   Float32x4 attribute per declared vs input, located with the same
//!   deterministic [`interpolant_location`] scheme the WGSL uses.
//! * [`bind_group_layout_entries`] — wgpu BGL entries from a plan.
//! * [`renumber_group`] — the fragment WGSL moves from group 0 to group
//!   1 (the translator hardcodes `@group(0)` per stage; with two stages
//!   in one pipeline the fragment resources must live in their own bind
//!   group).  LIKELY mapping, documented here, applied mechanically.
//! * [`create_render_pipeline`] / [`Renderer`] — the GPU edge.  This
//!   sandbox has no GPU: these are compile-proven by
//!   `cargo check --features render-wgpu` (CI runs it) and will first
//!   EXECUTE on the user's machine / an Android device.
//!
//! # Evidence
//!
//! * PROVEN: the binding arithmetic mirrors `shader::wgsl`'s emitted
//!   declarations (cross-checked per program by tests against the real
//!   26 shipped shaders + the shaders.rcf member).
//! * PROVEN: uniform rows are 16-byte vec4 registers (D3D9 c# file
//!   semantics, proven in `sm30`'s CTAB analysis).
//! * LIKELY: group assignment (vs=0, fs=1), alpha blending
//!   (SRCALPHA/INVSRCALPHA ≈ `BlendState::ALPHA_BLENDING`), no depth
//!   test, TriangleList culling None — clean-room defaults for the
//!   frontend shader family; the original D3D9 render states are
//!   UNKNOWN until the game's render-state records are decoded.
//! * UNKNOWN: the original engine's slot layout — we deliberately
//!   renumber instead of guessing it.

// The device-free planning core lives in `shader::plan` (always compiled);
// re-exported here so existing `wgpu_backend::` paths keep working.
pub use crate::shader::plan::{
    matrix_rows, plan_stage_resources, renumber_group, uniform_row_bytes, validate_plan_matches_wgsl,
    PlanError, StageBinding, StageResources,
};

// Re-export wgpu itself: the Android glue (prototype-android, feature
// `android-activity`) needs `SurfaceError` types but must not declare its
// own wgpu dependency — one version, one source.
pub use wgpu;

use crate::shader::sm30::Sm30Shader;
use crate::scene::{Camera, SceneMesh};
use crate::shader::wgsl::interpolant_location;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Backend errors.
#[derive(Debug)]
pub enum WgpuError {
    /// No compatible GPU adapter was found.
    NoAdapter,
    /// Device request failed.
    Device(wgpu::RequestDeviceError),
    /// Surface creation failed.
    SurfaceCreation(String),
    /// Adapter request failed.
    Adapter(String),
    /// The binding plan disagreed with the emitted WGSL — a bug in the
    /// mirror arithmetic, never ship a pipeline in this state.
    PlanMismatch(String),
}
impl std::fmt::Display for WgpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WgpuError::NoAdapter => f.write_str("no compatible GPU adapter"),
            WgpuError::Device(e) => write!(f, "device request failed: {e}"),
            WgpuError::SurfaceCreation(e) => write!(f, "surface creation failed: {e}"),
            WgpuError::Adapter(e) => write!(f, "adapter request failed: {e}"),
            WgpuError::PlanMismatch(e) => write!(f, "binding plan mismatch: {e}"),
        }
    }
}
impl std::error::Error for WgpuError {}
impl From<wgpu::RequestDeviceError> for WgpuError {
    fn from(e: wgpu::RequestDeviceError) -> Self {
        WgpuError::Device(e)
    }
}
impl From<wgpu::CreateSurfaceError> for WgpuError {
    fn from(e: wgpu::CreateSurfaceError) -> Self {
        WgpuError::SurfaceCreation(e.to_string())
    }
}
impl From<PlanError> for WgpuError {
    fn from(e: PlanError) -> Self {
        match e {
            PlanError::Mismatch(s) => WgpuError::PlanMismatch(s),
        }
    }
}

// ---------------------------------------------------------------------------
// Vertex layout
// ---------------------------------------------------------------------------

/// The vertex buffer layout for a vertex stage: one Float32x4 attribute
/// per declared input, packed compactly (offset = attribute_index * 16),
/// located by the same deterministic scheme as the WGSL `VsIn` struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshVertexLayout {
    /// Bytes per vertex: 16 * number of attributes.
    pub stride: u64,
    /// Attributes in declaration order.
    pub attributes: Vec<wgpu::VertexAttribute>,
}

impl MeshVertexLayout {
    /// Borrow as a wgpu layout (attributes are borrowed from `self`).
    pub fn as_layout(&self) -> wgpu::VertexBufferLayout<'_> {
        wgpu::VertexBufferLayout {
            array_stride: self.stride,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &self.attributes,
        }
    }
}

/// Plan the vertex buffer layout from the vertex stage's input DCLs.
pub fn plan_vertex_layout(vs: &Sm30Shader) -> MeshVertexLayout {
    let mut attributes = Vec::new();
    for (usage, index, reg) in vs.inputs() {
        if reg.ty != crate::shader::sm30::RegType::Input {
            continue;
        }
        attributes.push(wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: (attributes.len() as u64) * 16,
            shader_location: interpolant_location(usage, index) as u32,
        });
    }
    MeshVertexLayout {
        stride: 16 * attributes.len() as u64,
        attributes,
    }
}

// ---------------------------------------------------------------------------
// Bind group layout entries
// ---------------------------------------------------------------------------

/// One 16-byte uniform row's buffer binding size.
const ROW_SIZE: u64 = 16;

/// wgpu BGL entries for a stage's plan.
///
/// `visibility` is the stage's shader stage (VERTEX for the vs group,
/// FRAGMENT for the fs group).
pub fn bind_group_layout_entries(
    res: &StageResources,
    visibility: wgpu::ShaderStages,
) -> Vec<wgpu::BindGroupLayoutEntry> {
    res.bindings
        .iter()
        .map(|b| wgpu::BindGroupLayoutEntry {
            binding: b.binding(),
            visibility,
            ty: match b {
                // Float c# rows AND bool b# rows are the same GPU
                // resource: one 16-byte uniform buffer (block 34).
                StageBinding::Uniform { .. } | StageBinding::BoolUniform { .. } => {
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(ROW_SIZE),
                    }
                }
                StageBinding::Texture { .. } => wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                StageBinding::Sampler { .. } => {
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)
                }
            },
            count: None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// GPU edge (compile-proven here; executes on a real device)
// ---------------------------------------------------------------------------

/// A pipeline created on a device: the wgpu pipeline plus the bind group
/// layouts and the FULL binding plans needed to make bind groups for it
/// (the debug-triangle path builds its groups from exactly these plans,
/// which `create_render_pipeline` already cross-checked against the
/// compiled WGSL — see `validate_plan_matches_wgsl`).
pub struct GpuPipeline {
    /// Which shader pair this pipeline came from (logs / batch labels).
    pub name: String,
    /// The render pipeline (vs group 0, fs group 1).
    pub pipeline: wgpu::RenderPipeline,
    /// The vertex-stage bind group layout.
    pub vertex_bgl: wgpu::BindGroupLayout,
    /// The fragment-stage bind group layout.
    pub fragment_bgl: wgpu::BindGroupLayout,
    /// The vertex buffer layout (stride + attribute semantics).
    pub vertex_layout: MeshVertexLayout,
    /// Full vertex-stage binding plan (uniform rows, in binding order).
    pub vertex_resources: StageResources,
    /// Full fragment-stage binding plan (uniforms, textures, samplers).
    pub fragment_resources: StageResources,
}

/// The fullscreen movie quad's GPU state ([`crate::video_quad`] holds the
/// WGSL + the evidence labels): the compiled pipeline, its single-group
/// bind layout and the stretch sampler.  Created once per boot via
/// [`Renderer::create_video_quad`]; the video texture + bind group are
/// per-movie ([`Renderer::create_video_bind_group`]).
pub struct VideoQuad {
    /// The compiled fullscreen-triangle pipeline.
    pipeline: wgpu::RenderPipeline,
    /// The one bind group layout (texture @0, sampler @1 — group 0).
    bind_layout: wgpu::BindGroupLayout,
    /// Linear clamp-to-edge stretch sampler (DESIGN — module docs).
    sampler: wgpu::Sampler,
}

/// Free-function form of [`Renderer::create_video_quad`] (the offscreen
/// device test drives it with a bare device — no surface needed to
/// compile the quad's pipeline).
pub fn create_video_quad(
    device: &wgpu::Device,
    target_format: wgpu::TextureFormat,
) -> VideoQuad {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("prototype-rs video quad"),
        source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(
            crate::video_quad::VIDEO_QUAD_WGSL,
        )),
    });
    let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("prototype-rs video quad bindings"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("prototype-rs video quad layout"),
        bind_group_layouts: &[&bind_layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("prototype-rs video quad pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[], // the fullscreen triangle is vertex_index-generated
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                // Opaque replace: the quad IS the frame while a movie
                // plays (no blending with whatever is under).
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("prototype-rs video quad sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Nearest,
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        ..Default::default()
    });
    VideoQuad { pipeline, bind_layout, sampler }
}

/// Free-function form of [`Renderer::create_video_bind_group`].
pub fn create_video_bind_group(
    device: &wgpu::Device,
    quad: &VideoQuad,
    video_view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("prototype-rs video quad bind group"),
        layout: &quad.bind_layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(video_view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&quad.sampler) },
        ],
    })
}

/// Create a render pipeline from a decoded shader pair + their translated
/// WGSL.
///
/// LIKELY render-state defaults (documented; original D3D9 states are
/// UNKNOWN): alpha blending (`BlendState::ALPHA_BLENDING` ≈
/// SRCALPHA/INVSRCALPHA), no depth test, TriangleList, no culling.
pub fn create_render_pipeline(
    device: &wgpu::Device,
    name: &str,
    vs: &Sm30Shader,
    fs: &Sm30Shader,
    vs_wgsl: &str,
    fs_wgsl: &str,
    target_format: wgpu::TextureFormat,
) -> Result<GpuPipeline, WgpuError> {
    let vs_res = plan_stage_resources(vs);
    let fs_res = plan_stage_resources(fs);
    create_render_pipeline_with_plans(
        device, name, vs, fs, vs_wgsl, fs_wgsl, target_format, vs_res, fs_res,
    )
}

/// [`create_render_pipeline`] with PRE-DERIVED binding plans — the DLL
/// world pair's path (block 34): its plans must come from the WGSL
/// emitter's capture (`shader::wgsl::translate_program_with_plan`, via
/// `shader::world::WorldPairPlan`), because the bytecode-only mirror
/// cannot see the blobs' `u_c{n}` reads or `b#` feature rows and would
/// fail the plan↔WGSL validation below.
pub fn create_render_pipeline_with_plans(
    device: &wgpu::Device,
    name: &str,
    vs: &Sm30Shader,
    _fs: &Sm30Shader,
    vs_wgsl: &str,
    fs_wgsl: &str,
    target_format: wgpu::TextureFormat,
    vs_res: StageResources,
    fs_res: StageResources,
) -> Result<GpuPipeline, WgpuError> {
    // 1. plans — supplied by the caller (validated against the WGSL
    //    in step 3, so a wrong plan cannot reach the GPU).

    // 2. the fragment text moves to group 1 BEFORE validation, so the
    //    validation sees exactly what the GPU will see.
    let fs_wgsl_g1 = renumber_group(fs_wgsl, 1);

    // 3. cross-check both plans against the text the GPU will compile.
    validate_plan_matches_wgsl(&vs_res, vs_wgsl)?;
    validate_plan_matches_wgsl(&fs_res, &fs_wgsl_g1)?;

    // 4. bind group layouts: group 0 = vertex, group 1 = fragment.
    let vertex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(&format!("{name} — vertex resources")),
        entries: &bind_group_layout_entries(&vs_res, wgpu::ShaderStages::VERTEX),
    });
    let fragment_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(&format!("{name} — fragment resources")),
        entries: &bind_group_layout_entries(&fs_res, wgpu::ShaderStages::FRAGMENT),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(&format!("{name} — layout")),
        bind_group_layouts: &[&vertex_bgl, &fragment_bgl],
        push_constant_ranges: &[],
    });

    // 5. shader modules.
    let vs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&format!("{name} — vs")),
        source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(vs_wgsl)),
    });
    let fs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&format!("{name} — fs")),
        source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(&fs_wgsl_g1)),
    });

    // 6. the pipeline.
    let vertex_layout = plan_vertex_layout(vs);
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(name),
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
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });

    // 7. carry the FULL validated plans forward for bind-group creation
    //    at draw time (the debug-triangle path consumes these verbatim).
    Ok(GpuPipeline {
        name: name.to_string(),
        pipeline,
        vertex_bgl,
        fragment_bgl,
        vertex_layout,
        vertex_resources: vs_res,
        fragment_resources: fs_res,
    })
}

/// A minimal real renderer: device + queue + configured surface, ready to
/// create pipelines and present frames.
///
/// The draw-call path (scene → vertex/index buffers → bind groups →
/// draw) lands with M21b's real mesh data; this object is the frame
/// skeleton that path plugs into.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    /// Block 107: the menu/UI pass (built lazily via [`Renderer::init_ui`];
    /// None until then — a plain world frame needs nothing from it).
    pub ui: Option<crate::ui::gpu::UiPass>,
}

impl Renderer {
    /// Create a renderer for a window/display-handle target.
    ///
    /// Async (adapter/device requests); on Android call this from the
    /// winit/android-activity resumed event with the raw window handle.
    pub async fn new(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
    ) -> Result<Self, WgpuError> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let surface = instance.create_surface(target)?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|e| WgpuError::Adapter(e.to_string()))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("prototype-rs renderer"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
            })
            .await?;
        // Block 103: wgpu's DEFAULT uncaptured-error handler PANICS — on
        // a phone, one validation error from a driver quirk or an
        // oversized real-game mesh would kill the whole app mid-boot.
        // Log it instead: the frame degrades, the journal records the
        // cause, the process stays alive.
        device.on_uncaptured_error(Box::new(|e| {
            log::error!("wgpu uncaptured error (frame degraded, app alive): {e}");
        }));

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .first()
            .copied()
            .ok_or_else(|| WgpuError::SurfaceCreation("no surface formats".into()))?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo, // vsync; Fifo is always supported
            alpha_mode: caps
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        Ok(Self { device, queue, surface, config, ui: None })
    }

    /// Block 107: build the menu/UI pass (font atlas upload + pipeline)
    /// against the current surface size.  Returns the CPU atlas for the
    /// caller's text layout.  Call once after `new` (and never re-call —
    /// the atlas is static; resizes just update the ortho uniform).
    pub fn init_ui(&mut self) -> crate::FontAtlas {
        let (pass, atlas) = crate::ui::gpu::UiPass::new(
            &self.device,
            &self.queue,
            self.config.format,
            [self.config.width as f32, self.config.height as f32],
        );
        self.ui = Some(pass);
        atlas
    }

    /// The surface's chosen format — create pipelines with this.
    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    /// Resize (re-configure the surface).
    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
        // Block 107: the UI ortho basis follows the surface.
        if let Some(ui) = &self.ui {
            ui.set_screen(
                &self.queue,
                [self.config.width as f32, self.config.height as f32],
            );
        }
    }

    /// The current surface size (the menu layout's ortho basis).
    pub fn surface_size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Borrow the device (bind groups, buffers, pipelines).
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// Borrow the queue (uploads).
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// One clear + present frame — the minimal real frame loop.
    ///
    /// `batches` are the debug triangles built by
    /// [`Renderer::create_debug_batches`] and/or real-mesh batches;
    /// empty slice = the plain clear (the pre-M21b state).
    pub fn render_frame(
        &mut self,
        clear: [f64; 4],
        batches: &[DebugDrawBatch],
    ) -> Result<(), wgpu::SurfaceError> {
        self.encode_frame(clear, batches.iter())
    }

    /// Block 107: the same frame with the MENU/UI layer composited on
    /// top (a second, alpha-blended pass — load:Load so the world or
    /// clear color shows through).  `list` must have been laid out for
    /// the CURRENT surface size.  `init_ui` must have run.
    pub fn render_frame_with_ui(
        &mut self,
        clear: [f64; 4],
        batches: &[DebugDrawBatch],
        list: &crate::ui::UiDrawList,
    ) -> Result<(), wgpu::SurfaceError> {
        let Some(ui) = self.ui.as_mut() else {
            return self.encode_frame(clear, batches.iter());
        };
        ui.upload(&self.device, &self.queue, list);
        self.encode_frame(clear, batches.iter())?;
        // Second pass: UI over the world (same texture, load:Load).
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("prototype-rs ui frame"),
            });
        let ui = self.ui.as_ref().expect("checked above");
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("prototype-rs ui pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            ui.draw(&mut pass, list);
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }

    /// M25b (block 20): the same frame with a per-frame SELECTED
    /// subset — the draw path's streaming step.  The caller picks the
    /// batches (via `prototype_android::meshes::DrawSelector`, which
    /// orders members by the real `WorldIndex::members_near` camera
    /// distance) and passes references; unselected batches simply do
    /// not encode a draw call this frame.  The encode sequence is
    /// byte-for-byte the owned-slice path.
    pub fn render_frame_selected(
        &mut self,
        clear: [f64; 4],
        batches: &[&DebugDrawBatch],
    ) -> Result<(), wgpu::SurfaceError> {
        self.encode_frame(clear, batches.iter().copied())
    }

    /// The shared encode sequence (pipeline → groups → bindings →
    /// draw_indexed per batch) behind both frame entries.
    fn encode_frame<'b, I>(&mut self, clear: [f64; 4], batches: I) -> Result<(), wgpu::SurfaceError>
    where
        I: Iterator<Item = &'b DebugDrawBatch>,
    {
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("prototype-rs frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("prototype-rs clear pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear[0],
                            g: clear[1],
                            b: clear[2],
                            a: clear[3],
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            // The draw-call template: pipeline → groups → bindings →
            // draw_indexed.  Real meshes (M21b) reuse exactly this.
            for batch in batches {
                pass.set_pipeline(&batch.pipeline);
                pass.set_bind_group(0, &batch.vertex_bind_group, &[]);
                pass.set_bind_group(1, &batch.fragment_bind_group, &[]);
                pass.set_vertex_buffer(0, batch.vertex_buffer.slice(..));
                pass.set_index_buffer(batch.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(0..batch.index_count, 0, 0..1);
            }
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }

    /// Build one ready-to-draw [`DebugDrawBatch`] per pipeline: the
    /// synthetic triangle (from that pipeline's own planned vertex
    /// layout) plus bind groups assembled from the pipeline's validated
    /// binding plans — zero-filled 16-byte uniform rows, the shared 1×1
    /// white texture view for texture bindings, the shared filtering
    /// sampler for sampler bindings.
    ///
    /// Call once per boot (after the pipelines are created); pass the
    /// result to [`Renderer::render_frame`] every frame.  Dropping the
    /// returned batches releases their GPU resources.
    pub fn create_debug_batches(&self, pipelines: &[GpuPipeline]) -> Vec<DebugDrawBatch> {
        // Shared debug resources: one white texture + one sampler serve
        // every texture/sampler binding of every pipeline.
        let white = self.device.create_texture(&debug_texture_descriptor());
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &white,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255u8, 255, 255, 255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                // Packed rows: a 1×1 write cannot use Some(4) — wgpu
                // requires 256-B row alignment when Some is used.
                bytes_per_row: None,
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let white_view = white.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = self.device.create_sampler(&debug_sampler_descriptor());

        let mut batches = Vec::with_capacity(pipelines.len());
        for p in pipelines {
            // -- vertex buffer + index buffer ----------------------------
            let vbytes = debug_triangle_vertices(&p.vertex_layout);
            let vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("prototype-rs debug triangle vertices"),
                size: vbytes.len() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: true,
            });
            vertex_buffer.get_mapped_range_mut(..)[..].copy_from_slice(&vbytes);
            vertex_buffer.unmap();

            let ibytes: Vec<u8> = debug_triangle_indices()
                .iter()
                .flat_map(|i| i.to_le_bytes())
                .collect();
            let index_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("prototype-rs debug triangle indices"),
                size: ibytes.len() as u64,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: true,
            });
            index_buffer.get_mapped_range_mut(..)[..].copy_from_slice(&ibytes);
            index_buffer.unmap();

            // -- bind groups from the validated plans --------------------
            let (vertex_bind_group, vertex_uniforms) = self.debug_bind_group(
                "vs",
                &p.vertex_bgl,
                &p.vertex_resources,
                &white_view,
                &sampler,
            );
            let (fragment_bind_group, fragment_uniforms) = self.debug_bind_group(
                "fs",
                &p.fragment_bgl,
                &p.fragment_resources,
                &white_view,
                &sampler,
            );

            batches.push(DebugDrawBatch {
                name: p.name.clone(),
                pipeline: p.pipeline.clone(),
                vertex_bind_group,
                fragment_bind_group,
                vertex_buffer,
                index_buffer,
                index_count: 3,
                vertex_uniforms,
                fragment_uniforms,
            });
        }
        batches
    }

    /// Build `count` ready-to-draw [`DebugDrawBatch`] unit-cube markers
    /// from `pipeline` (block 21).  Each batch carries its OWN uniform
    /// rows, so per frame the app writes `WVP · (translate · scale)` for
    /// the nearest placed objects ([`debug_cube_vertices`] geometry,
    /// `index_count` 36).  The pool is created once per boot; a cube
    /// whose rows are never written sits at the unit cube (harmless —
    /// the app only draws the cubes it wrote this frame).
    pub fn create_box_batches(&self, pipeline: &GpuPipeline, count: usize) -> Vec<DebugDrawBatch> {
        // Shared debug resources (same contract as the triangle path).
        let white = self.device.create_texture(&debug_texture_descriptor());
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &white,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255u8, 255, 255, 255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                // Packed rows: a 1×1 write cannot use Some(4) — wgpu
                // requires 256-B row alignment when Some is used.
                bytes_per_row: None,
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let white_view = white.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = self.device.create_sampler(&debug_sampler_descriptor());

        let vbytes = debug_cube_vertices(&pipeline.vertex_layout);
        let vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("prototype-rs debug cube vertices"),
            size: vbytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: true,
        });
        vertex_buffer.get_mapped_range_mut(..)[..].copy_from_slice(&vbytes);
        vertex_buffer.unmap();

        let ibytes: Vec<u8> = debug_cube_indices()
            .iter()
            .flat_map(|i| i.to_le_bytes())
            .collect();
        let index_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("prototype-rs debug cube indices"),
            size: ibytes.len() as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: true,
        });
        index_buffer.get_mapped_range_mut(..)[..].copy_from_slice(&ibytes);
        index_buffer.unmap();

        let mut batches = Vec::with_capacity(count);
        for i in 0..count {
            let (vertex_bind_group, vertex_uniforms) = self.debug_bind_group(
                "vs",
                &pipeline.vertex_bgl,
                &pipeline.vertex_resources,
                &white_view,
                &sampler,
            );
            let (fragment_bind_group, fragment_uniforms) = self.debug_bind_group(
                "fs",
                &pipeline.fragment_bgl,
                &pipeline.fragment_resources,
                &white_view,
                &sampler,
            );
            batches.push(DebugDrawBatch {
                name: format!("{}#cube{i}", pipeline.name),
                pipeline: pipeline.pipeline.clone(),
                vertex_bind_group,
                fragment_bind_group,
                vertex_buffer: vertex_buffer.clone(),
                index_buffer: index_buffer.clone(),
                index_count: 36,
                vertex_uniforms,
                fragment_uniforms,
            });
        }
        batches
    }

    /// [`Self::write_view_projection`] composed with a model matrix —
    /// the placed-cube upload (`WVP · translate·scale` rows).  Returns
    /// rows written (0 when the shader lacks the WVP uniform).
    pub fn write_view_projection_model(
        &self,
        batch: &DebugDrawBatch,
        cam: &Camera,
        model: &prototype_math::Mat4,
    ) -> usize {
        let rows = view_projection_model_rows(cam, model);
        self.write_matrix(batch, false, "p3dWorldViewProjectionMatrix", &rows)
    }

    /// Upload one decoded RGBA8 texture and return its bindable view —
    /// the per-slot GPU edge of the material path (block 26; the label
    /// identifies mesh + slot for GPU diagnostics).  Block 29: when the
    /// upload carries a full mip chain the texture is created with the
    /// real level count and every level is written (levels are tightly
    /// packed `bytes_per_row: None`, which also avoids the 256-B
    /// row-alignment constraint the small tail levels would break).
    fn upload_rgba_texture(
        &self,
        mesh_label: &str,
        slot: &str,
        t: &TextureUpload,
    ) -> wgpu::TextureView {
        let mip_level_count = 1 + t.mip_levels.len() as u32;
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(Box::leak(
                format!("prototype-rs mesh {mesh_label} {slot}").into_boxed_str(),
            )),
            size: wgpu::Extent3d {
                width: t.width,
                height: t.height,
                depth_or_array_layers: 1,
            },
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let level = |mip: u32, w: u32, h: u32, rgba: &[u8]| {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: mip,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                rgba,
                // Packed rows (None): valid for every level size — the
                // Some(width·4) form would need 256-B alignment the
                // 4- and 1-texel tail levels cannot provide.
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: None,
                    rows_per_image: None,
                },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
        };
        level(0, t.width, t.height, &t.rgba);
        for (i, m) in t.mip_levels.iter().enumerate() {
            level(i as u32 + 1, m.width, m.height, &m.rgba);
        }
        tex.create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// Create a reusable VIDEO-frame texture — the dynamic counterpart
    /// of [`Self::upload_rgba_texture`] (block 53, NEXT-UP (a)): one
    /// RGBA8 plane, no mip chain, created ONCE per movie with
    /// `TEXTURE_BINDING | COPY_DST` and re-written every frame via
    /// [`Self::upload_video_frame`].  The returned texture is
    /// view-ready (`create_view(&default())` binds directly).
    pub fn create_video_texture(
        &self,
        width: u32,
        height: u32,
        label: &str,
    ) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(Box::leak(
                format!("prototype-rs video {label}").into_boxed_str(),
            )),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    /// Replace a video texture's full contents with one RGBA8 frame —
    /// `rgba` must be exactly `4·width·height` of the CREATION dims
    /// (the same tight `bytes_per_row: None` copy the mesh path uses;
    /// a movie frame is never a partial update).  Fails closed on a
    /// size mismatch — a wrong-size upload would smear on the GPU.
    pub fn upload_video_frame(
        &self,
        texture: &wgpu::Texture,
        rgba: &[u8],
    ) -> Result<(), String> {
        let size = texture.size();
        let need = (size.width * size.height * 4) as usize;
        if rgba.len() != need {
            return Err(format!(
                "video frame upload: got {} B, texture {}x{} needs {need}",
                rgba.len(),
                size.width,
                size.height
            ));
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: None,
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: size.width,
                height: size.height,
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }

    /// Create the FULLSCREEN MOVIE QUAD draw state ([`crate::video_quad`]
    /// for the WGSL + evidence) — one pipeline + one bind-group layout +
    /// the linear clamp-to-edge sampler, created ONCE per boot like the
    /// shader-pair pipelines.  `target_format` is the surface format.
    pub fn create_video_quad(&self, target_format: wgpu::TextureFormat) -> VideoQuad {
        create_video_quad(&self.device, target_format)
    }

    /// Bind one video texture into the quad's single bind group (created
    /// once per movie; the texture's CONTENTS are re-written per frame
    /// via [`Renderer::upload_video_frame`], the bind group stays).
    pub fn create_video_bind_group(
        &self,
        quad: &VideoQuad,
        video_view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        create_video_bind_group(&self.device, quad, video_view)
    }

    /// Encode the movie quad into ONE render pass over `target_view`
    /// (cleared to BLACK first — the quad covers the screen, the clear
    /// only defines the degenerate-frame edges).  Shared by
    /// [`Renderer::render_video_frame`] and the offscreen device test.
    fn encode_video_pass(
        encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        quad: &VideoQuad,
        bind_group: &wgpu::BindGroup,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("prototype-rs video quad pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&quad.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Present ONE movie frame: acquire the surface, draw the fullscreen
    /// quad sampling the video texture, present.  The boot path calls
    /// this INSTEAD of [`Renderer::render_frame`] while a movie plays.
    /// Error semantics are the surface's (Lost/Outdated → rebuild).
    pub fn render_video_frame(
        &mut self,
        quad: &VideoQuad,
        bind_group: &wgpu::BindGroup,
    ) -> Result<(), wgpu::SurfaceError> {
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("prototype-rs video frame"),
            });
        Self::encode_video_pass(&mut encoder, &view, quad, bind_group);
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }

    /// Build one ready-to-draw [`DebugDrawBatch`] per (pipeline, source)
    /// pair — the REAL-mesh variant of [`Renderer::create_debug_batches`].
    /// Sources are zipped with `pipelines` in order; the caller pairs a
    /// mesh with its pipeline by name (a mesh's name IS its
    /// `pure3dShader` name, proven in `prototype_p3d::geometry`).
    ///
    /// Bind groups: zeroed uniform rows and the shared sampler as on the
    /// debug path; the fragment stage's TEXTURE bindings are filled from
    /// the source's material slots (block 26) — slot 0 (lowest s#) gets
    /// the resolved diffuse, slots 1.. get `extra_textures` in param
    /// order, and any binding past the available uploads keeps the
    /// shared white view (draw stays valid, gap is visible as white).
    pub fn create_mesh_batches(
        &self,
        pipelines: &[GpuPipeline],
        sources: &[MeshBatchSource],
    ) -> Vec<DebugDrawBatch> {
        self.build_mesh_batches(pipelines, sources, &|res, uploads| {
            // The frontend ascending-s# slot policy (block 26).
            let regs = texture_slot_registers(res);
            assign_texture_uploads(&regs, uploads)
        })
    }

    /// [`Self::create_mesh_batches`] with the DLL world pair's
    /// NAME-driven material slots (block 34): the assignment comes from
    /// `shader::world::world_texture_assignment` (diffuse → `diffuseMap`
    /// s4, extras → normalMap/specularMap, engine light/shadow textures
    /// → white) — on this pair the lowest s# is a LIGHT texture, so the
    /// ascending policy would have fed the diffuse to the wrong slot.
    /// `world_plan` supplies the CTAB sampler names.
    pub fn create_world_mesh_batches(
        &self,
        pipelines: &[GpuPipeline],
        sources: &[MeshBatchSource],
        world_plan: &crate::shader::world::WorldPairPlan,
    ) -> Vec<DebugDrawBatch> {
        self.build_mesh_batches(pipelines, sources, &|res, uploads| {
            let _ = res; // the world policy reads the plan's CTAB names
            crate::shader::world::world_texture_assignment(world_plan, uploads)
        })
    }

    /// The shared body: per (pipeline, source) pair — vertex/index
    /// buffers, per-slot texture views, bind groups from the validated
    /// plans.  `slot_policy` decides which upload feeds each texture
    /// binding (frontend ascending-s# vs world name-driven).
    fn build_mesh_batches(
        &self,
        pipelines: &[GpuPipeline],
        sources: &[MeshBatchSource],
        slot_policy: &dyn Fn(&StageResources, usize) -> Vec<Option<usize>>,
    ) -> Vec<DebugDrawBatch> {
        let white = self.device.create_texture(&debug_texture_descriptor());
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &white,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255u8, 255, 255, 255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                // Packed rows: a 1×1 write cannot use Some(4) — wgpu
                // requires 256-B row alignment when Some is used.
                bytes_per_row: None,
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let white_view = white.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = self.device.create_sampler(&debug_sampler_descriptor());

        let mut batches = Vec::with_capacity(pipelines.len().min(sources.len()));
        for (p, src) in pipelines.iter().zip(sources.iter()) {
            // The material slot chain (block 26): slot 0 = the resolved
            // diffuse (`color` param), slots 1.. = the shader's other
            // texture params in param order.  Each upload becomes a real
            // RGBA8 texture + view; the fragment stage's texture
            // bindings are filled from the slots in ascending s# order
            // (slot i ← upload i — `assign_texture_uploads`), the
            // shared white view covering any binding past the uploads.
            let mut upload_views: Vec<wgpu::TextureView> = Vec::new();
            if let Some(t) = &src.texture {
                upload_views.push(self.upload_rgba_texture(&src.label, "diffuse", t));
            }
            for (k, t) in src.extra_textures.iter().enumerate() {
                upload_views.push(self.upload_rgba_texture(&src.label, &format!("extra{k}"), t));
            }
            let slot_regs = texture_slot_registers(&p.fragment_resources);
            let assignment = slot_policy(&p.fragment_resources, upload_views.len());
            let mut per_register: std::collections::BTreeMap<u16, &wgpu::TextureView> =
                std::collections::BTreeMap::new();
            for (reg, slot) in slot_regs.iter().zip(&assignment) {
                let view = slot.map(|k| &upload_views[k]).unwrap_or(&white_view);
                per_register.insert(*reg, view);
            }
            let vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(Box::leak(format!("prototype-rs mesh {} vertices", src.label).into_boxed_str())),
                size: src.vertex_bytes.len() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: true,
            });
            vertex_buffer.get_mapped_range_mut(..)[..].copy_from_slice(&src.vertex_bytes);
            vertex_buffer.unmap();

            let ibytes: Vec<u8> = src.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
            let index_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(Box::leak(format!("prototype-rs mesh {} indices", src.label).into_boxed_str())),
                size: ibytes.len() as u64,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: true,
            });
            index_buffer.get_mapped_range_mut(..)[..].copy_from_slice(&ibytes);
            index_buffer.unmap();

            let (vertex_bind_group, vertex_uniforms) = self.debug_bind_group(
                "vs",
                &p.vertex_bgl,
                &p.vertex_resources,
                &white_view,
                &sampler,
            );
            let (fragment_bind_group, fragment_uniforms) = self.bind_group_with_overrides(
                "fs",
                &p.fragment_bgl,
                &p.fragment_resources,
                &white_view,
                &sampler,
                &per_register,
            );

            batches.push(DebugDrawBatch {
                name: format!("{} ← {}", src.label, p.name),
                pipeline: p.pipeline.clone(),
                vertex_bind_group,
                fragment_bind_group,
                vertex_buffer,
                index_buffer,
                index_count: src.indices.len() as u32,
                vertex_uniforms,
                fragment_uniforms,
            });
        }
        batches
    }

    // -----------------------------------------------------------------
    // REAL uniform uploads (block 17) — the visibility half
    // -----------------------------------------------------------------

    /// Upload one 16-byte row into the batch's uniform buffer backing
    /// `register` (vertex stage, or fragment stage when `fragment`).
    /// Float rows only — bool rows need [`Renderer::write_bool_row`].
    /// Returns `false` (and writes nothing) when that stage has no such
    /// register — the caller logs the gap instead of guessing.
    pub fn write_uniform_row(
        &self,
        batch: &DebugDrawBatch,
        fragment: bool,
        register: u16,
        row: [f32; 4],
    ) -> bool {
        let list = if fragment { &batch.fragment_uniforms } else { &batch.vertex_uniforms };
        match list.iter().find(|u| u.register == register && !u.is_bool) {
            Some(u) => {
                self.queue.write_buffer(&u.buffer, 0, &uniform_row_bytes(row));
                true
            }
            None => false,
        }
    }

    /// Upload one BOOL row (`b#`): `true` encodes as `[1, 0, 0, 0]` —
    /// the translated WGSL tests `bN.x != 0.0` (block 34).  Returns
    /// `false` when the stage has no such bool register.
    pub fn write_bool_row(
        &self,
        batch: &DebugDrawBatch,
        fragment: bool,
        register: u16,
        value: bool,
    ) -> bool {
        let list = if fragment { &batch.fragment_uniforms } else { &batch.vertex_uniforms };
        match list.iter().find(|u| u.register == register && u.is_bool) {
            Some(u) => {
                let row: [f32; 4] = if value { [1.0, 0.0, 0.0, 0.0] } else { [0.0; 4] };
                self.queue.write_buffer(&u.buffer, 0, &uniform_row_bytes(row));
                true
            }
            None => false,
        }
    }

    /// Upload a 4×4 matrix (as D3D row-major rows) into the four
    /// CONSECUTIVE uniform rows starting at register `base` (vertex
    /// stage, or fragment stage when `fragment`) — the register-based
    /// addressing the DLL world pair needs (its WGSL names are `u_c{n}`;
    /// the CTAB proves which register a name means, e.g.
    /// `p3dWorldViewProjectionMatrix` = c0, `p3dWorldMatrix` = c4).
    /// Returns the number of rows written (4 = complete).
    pub fn write_matrix_at(
        &self,
        batch: &DebugDrawBatch,
        fragment: bool,
        base: u16,
        rows: &[[f32; 4]; 4],
    ) -> usize {
        let list = if fragment { &batch.fragment_uniforms } else { &batch.vertex_uniforms };
        let mut written = 0;
        for (i, row) in rows.iter().enumerate() {
            if let Some(u) = list
                .iter()
                .find(|u| u.register == base + i as u16 && !u.is_bool)
            {
                self.queue.write_buffer(&u.buffer, 0, &uniform_row_bytes(*row));
                written += 1;
            }
        }
        written
    }

    /// Upload a 4×4 matrix (as D3D row-major rows — see
    /// [`crate::scene::Camera::view_projection_rows`]) into the four
    /// consecutive uniform rows named `{base}_r0.._r3` of the vertex
    /// stage (fragment stage when `fragment`).  Returns the number of
    /// rows written (0 = the shader has no such uniform; 4 = complete).
    pub fn write_matrix(
        &self,
        batch: &DebugDrawBatch,
        fragment: bool,
        base: &str,
        rows: &[[f32; 4]; 4],
    ) -> usize {
        let list = if fragment { &batch.fragment_uniforms } else { &batch.vertex_uniforms };
        let mut written = 0;
        for (r, row) in rows.iter().enumerate() {
            let name = format!("{base}_r{r}");
            if let Some(u) = list.iter().find(|u| u.wgsl_name == name) {
                self.queue.write_buffer(&u.buffer, 0, &uniform_row_bytes(*row));
                written += 1;
            }
        }
        written
    }

    /// Convenience: the camera's view-projection (D3D rows) into the
    /// vertex stage's `p3dWorldViewProjectionMatrix` — THE standard
    /// shipped uniform (60 occurrences in frontendshaders.p3d alone).
    /// Returns rows written (0 when this shader lacks it).
    pub fn write_view_projection(
        &self,
        batch: &DebugDrawBatch,
        cam: &Camera,
    ) -> usize {
        let rows = cam.view_projection_rows();
        self.write_matrix(batch, false, "p3dWorldViewProjectionMatrix", &rows)
    }

    /// [`Self::write_view_projection`] with the DLL world-pair fallback
    /// (block 34): when the param-name rows are absent, write the four
    /// camera rows at the plan-provided register base instead (the
    /// world pair's WGSL names are `u_c{n}`; the CTAB proves
    /// `p3dWorldViewProjectionMatrix` = c0).  Returns rows written.
    pub fn write_view_projection_any(
        &self,
        batch: &DebugDrawBatch,
        cam: &Camera,
        world_wvp_register: u16,
    ) -> usize {
        let rows = cam.view_projection_rows();
        let by_name = self.write_matrix(batch, false, "p3dWorldViewProjectionMatrix", &rows);
        if by_name > 0 {
            return by_name;
        }
        self.write_matrix_at(batch, false, world_wvp_register, &rows)
    }

    /// [`Self::write_view_projection_model`] with the DLL world-pair
    /// fallback (block 34): the frontend path composes WVP·model into
    /// the four WVP rows; the world pair carries DECOMPOSED matrices
    /// (CTAB-proven: `p3dWorldViewProjectionMatrix` c0..c3,
    /// `p3dWorldMatrix` c4..c7), so there the camera rows go to the
    /// WVP base and the model rows to the world-matrix base.  Returns
    /// rows written (0 = neither path exists).
    pub fn write_view_projection_model_any(
        &self,
        batch: &DebugDrawBatch,
        cam: &Camera,
        model: &prototype_math::Mat4,
        world_wvp_register: u16,
        world_matrix_register: u16,
    ) -> usize {
        let vp = cam.view_projection_rows();
        let by_name = self.write_matrix(batch, false, "p3dWorldViewProjectionMatrix", &vp);
        if by_name > 0 {
            let rows = view_projection_model_rows(cam, model);
            return self.write_matrix(batch, false, "p3dWorldViewProjectionMatrix", &rows);
        }
        let vp_rows = self.write_matrix_at(batch, false, world_wvp_register, &vp);
        // Column-major Mat4 → D3D row-major rows (same convention as
        // view_projection_model_rows: rows[i][j] = cols[j][i]).
        let mut m = [[0.0f32; 4]; 4];
        for i in 0..4 {
            for j in 0..4 {
                m[i][j] = model.cols[j][i];
            }
        }
        let model_rows = self.write_matrix_at(batch, false, world_matrix_register, &m);
        vp_rows + model_rows
    }

    /// One bind group assembled from a validated [`StageResources`]
    /// plan: one zeroed 16-byte uniform buffer per uniform row, the
    /// shared white view per texture binding, the shared sampler per
    /// sampler binding.  Binding NUMBERS come from the plan (the same
    /// numbers the BGL was built from), so plan ↔ layout agreement is
    /// by construction.
    fn debug_bind_group(
        &self,
        stage: &str,
        bgl: &wgpu::BindGroupLayout,
        res: &StageResources,
        white_view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
    ) -> (wgpu::BindGroup, Vec<BatchUniform>) {
        self.bind_group_with_overrides(
            stage,
            bgl,
            res,
            white_view,
            sampler,
            &std::collections::BTreeMap::new(),
        )
    }

    /// [`Self::debug_bind_group`] with per-s#-register texture views: a
    /// Texture binding whose register is in `texture_overrides` binds
    /// that view instead of the shared white view (the material-slot
    /// path, block 26).  Binding numbers still come from the plan, so
    /// plan ↔ layout agreement stays by construction.
    fn bind_group_with_overrides(
        &self,
        stage: &str,
        bgl: &wgpu::BindGroupLayout,
        res: &StageResources,
        white_view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        texture_overrides: &std::collections::BTreeMap<u16, &wgpu::TextureView>,
    ) -> (wgpu::BindGroup, Vec<BatchUniform>) {
        // Per-uniform-row buffers must outlive create_bind_group; keep
        // them (with register + WGSL-name provenance for later uploads)
        // and only put references into the entries.
        let uniforms: Vec<BatchUniform> = res
            .bindings
            .iter()
            .filter_map(|b| {
                let (register, wgsl_name, is_bool) = match b {
                    StageBinding::Uniform { register, wgsl_name, .. } => {
                        (*register, wgsl_name, false)
                    }
                    StageBinding::BoolUniform { register, wgsl_name, .. } => {
                        (*register, wgsl_name, true)
                    }
                    _ => return None,
                };
                let kind = if is_bool { "b" } else { "c" };
                let buf = self
                    .device
                    .create_buffer(&uniform_row_buffer_descriptor(&format!(
                        "prototype-rs debug {stage} {kind}{register} row"
                    )));
                // Zero-filled: get_mapped_range_mut starts zeroed per
                // wgpu's contract; unmap immediately.
                buf.get_mapped_range_mut(..)[..].fill(0);
                buf.unmap();
                Some(BatchUniform {
                    register,
                    wgsl_name: wgsl_name.clone(),
                    is_bool,
                    buffer: buf,
                })
            })
            .collect();
        let uniform_buffers: Vec<&wgpu::Buffer> = uniforms.iter().map(|u| &u.buffer).collect();
        let mut uniform_iter = uniform_buffers.into_iter();
        let entries: Vec<wgpu::BindGroupEntry<'_>> = res
            .bindings
            .iter()
            .map(|b| {
                let resource = match b {
                    StageBinding::Uniform { .. } | StageBinding::BoolUniform { .. } => {
                        let buf = uniform_iter.next().expect("uniform buffers match plan");
                        wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: buf,
                            offset: 0,
                            size: Some(wgpu::BufferSize::new(ROW_SIZE).expect("16 > 0")),
                        })
                    }
                    StageBinding::Texture { register, .. } => {
                        let view = texture_overrides.get(register).copied().unwrap_or(white_view);
                        wgpu::BindingResource::TextureView(view)
                    }
                    StageBinding::Sampler { .. } => wgpu::BindingResource::Sampler(sampler),
                };
                wgpu::BindGroupEntry {
                    binding: b.binding(),
                    resource,
                }
            })
            .collect();
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("prototype-rs debug bind group"),
            layout: bgl,
            entries: &entries,
        });
        (group, uniforms)
    }
}

// ---------------------------------------------------------------------------
// Debug triangle — the synthetic draw that proves the vertex-pull path
// on a real GPU BEFORE M21b's mesh data exists.
//
// The contract is device-free and unit-tested: byte-exact vertex data
// shaped to any planned [`MeshVertexLayout`], plus a fixed u16 index
// buffer.  The GPU wiring (buffers + bind group + `draw`) reuses this
// verbatim in `render_frame`; when M21b lands, only the buffer SOURCE
// changes (decoded meshes instead of these corners).
//
// Evidence notes:
// * TriangleList order and CCW corners are DESIGN (wgpu's default
//   primitive state — `create_render_pipeline` sets
//   `PrimitiveState::default()`); front-face winding does not matter
//   while `cull_mode` is also `default()` = None, so the triangle draws
//   either way.
// * Every attribute gets the SAME Float32x4 corner row `[x, y, 0.5, 1]`
//   so the position attribute is correct wherever the planner located
//   it, and varying interpolants stay finite (a fragment shader sampling
//   them sees a smooth 0→1-ish gradient, never garbage).
// ---------------------------------------------------------------------------

/// Clip-space corners of the debug triangle: centred, ~150% of half the
/// NDC square, CCW in wgpu's NDC (+y up).
pub const DEBUG_TRIANGLE_CORNERS: [[f32; 2]; 3] = [
    [-0.75, -0.75],
    [0.75, -0.75],
    [0.0, 0.75],
];

/// Vertex bytes for a synthetic debug triangle shaped to `layout`:
/// one `Float32x4` row per attribute per corner = `[x, y, 0.5, 1.0]`.
///
/// Output length is exactly `3 * layout.stride` — hand this straight to
/// `create_buffer_init` (or feed a staging map) at draw time.
pub fn debug_triangle_vertices(layout: &MeshVertexLayout) -> Vec<u8> {
    // Attribute rows per vertex: stride is 16 bytes per Float32x4 row.
    let rows = (layout.stride / 16) as usize;
    let mut out = Vec::with_capacity(3 * layout.stride as usize);
    for corner in DEBUG_TRIANGLE_CORNERS {
        // One attribute row: [x, y, z=0.5, w=1.0] — all attributes share
        // it, so location order is irrelevant by construction.
        for _ in 0..rows {
            for v in [corner[0], corner[1], 0.5f32, 1.0f32] {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    debug_assert_eq!(out.len() as u64, 3 * layout.stride);
    out
}

/// Index buffer for the debug triangle: one CCW TriangleList triangle,
/// u16 indices (wgpu `IndexFormat::Uint16`).
pub fn debug_triangle_indices() -> Vec<u16> {
    vec![0, 1, 2]
}

// ---------------------------------------------------------------------------
// Debug cube (block 21) — the placed-object marker.
//
// The real world database (M25b) carries placed spawners at PROVEN world
// positions; per frame the nearest ones draw as unit cubes transformed by
// WVP · (translate · scale) — the visual proof that the player camera and
// the world placements share one coordinate system.
// ---------------------------------------------------------------------------

/// Unit-cube triangle corners in [-0.5, 0.5]³: 12 triangles × 3 corners,
/// outward-facing (CCW seen from outside).  36 corner rows, indices 0..35.
pub const DEBUG_CUBE_CORNERS: [[f32; 3]; 36] = [
    // +Z face
    [-0.5, -0.5, 0.5],
    [0.5, -0.5, 0.5],
    [0.5, 0.5, 0.5],
    [-0.5, -0.5, 0.5],
    [0.5, 0.5, 0.5],
    [-0.5, 0.5, 0.5],
    // -Z face
    [0.5, -0.5, -0.5],
    [-0.5, -0.5, -0.5],
    [-0.5, 0.5, -0.5],
    [0.5, -0.5, -0.5],
    [-0.5, 0.5, -0.5],
    [0.5, 0.5, -0.5],
    // +X face
    [0.5, -0.5, 0.5],
    [0.5, -0.5, -0.5],
    [0.5, 0.5, -0.5],
    [0.5, -0.5, 0.5],
    [0.5, 0.5, -0.5],
    [0.5, 0.5, 0.5],
    // -X face
    [-0.5, -0.5, -0.5],
    [-0.5, -0.5, 0.5],
    [-0.5, 0.5, 0.5],
    [-0.5, -0.5, -0.5],
    [-0.5, 0.5, 0.5],
    [-0.5, 0.5, -0.5],
    // +Y face
    [-0.5, 0.5, 0.5],
    [0.5, 0.5, 0.5],
    [0.5, 0.5, -0.5],
    [-0.5, 0.5, 0.5],
    [0.5, 0.5, -0.5],
    [-0.5, 0.5, -0.5],
    // -Y face
    [-0.5, -0.5, -0.5],
    [0.5, -0.5, -0.5],
    [0.5, -0.5, 0.5],
    [-0.5, -0.5, -0.5],
    [0.5, -0.5, 0.5],
    [-0.5, -0.5, 0.5],
];

/// Vertex bytes for the debug cube shaped to `layout`: one `Float32x4`
/// row per attribute per corner = `[x, y, z, 1.0]` (unlike the clip-space
/// debug triangle, the cube carries WORLD positions — its WVP uniform
/// moves it).  Output length is exactly `36 * layout.stride`.
pub fn debug_cube_vertices(layout: &MeshVertexLayout) -> Vec<u8> {
    let mut out = Vec::with_capacity(36 * layout.stride as usize);
    for corner in DEBUG_CUBE_CORNERS {
        // One attribute row per corner; every attribute shares the
        // position row (finite interpolants wherever the planner put
        // the attribute — same contract as the debug triangle).
        for _ in 0..(layout.stride / 16) {
            for v in [corner[0], corner[1], corner[2], 1.0f32] {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    debug_assert_eq!(out.len() as u64, 36 * layout.stride);
    out
}

/// Index buffer for the debug cube: identity run 0..36 (u16).
pub fn debug_cube_indices() -> Vec<u16> {
    (0..36u16).collect()
}

/// The camera's view-projection composed with a model matrix, as the
/// four D3D row-major rows the translated WGSL consumes
/// (`p3dWorldViewProjectionMatrix_r0..r3`).
///
/// Convention (matches [`Camera::view_projection_rows`]): the shader
/// computes `out = M · v`, so the model transform (applied to the
/// vertex FIRST) is `M = M_vp · M_model` in column-major terms.
/// Device-free and unit-tested.
pub fn view_projection_model_rows(cam: &Camera, model: &prototype_math::Mat4) -> [[f32; 4]; 4] {
    let vp = cam.view_projection(); // column-major
    let m = vp.mul_mat4(model);
    let mut rows = [[0.0f32; 4]; 4];
    for (i, row) in rows.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = m.cols[j][i];
        }
    }
    rows
}

// ---------------------------------------------------------------------------
// REAL mesh geometry — the M21b draw path (block 16)
// ---------------------------------------------------------------------------

/// Pack a real [`SceneMesh`] into a pipeline's vertex-layout shape: one
/// `Float32x4` row per planned attribute, filled by the same
/// deterministic location scheme the WGSL translator emits:
///
/// * location `VERTEX_POSITION_LOCATION` (32) — `position0`:
///   `[x, y, z, 1]`.
/// * location 0 — `texcoord0`: the mesh's UVs `[u, v, 0, 1]`
///   (missing UVs degrade to `[0, 0, 0, 1]`).
/// * locations 1..8 — `texcoord#` sets 1..7: this decoder carries only
///   the FIRST UV set, so they degrade to opaque black rows (multi-set
///   packing needs the full layout table — future work, documented).
/// * locations 8..16 — `color#`: opaque white `[1, 1, 1, 1]`.
/// * locations 16.. — `usage#` interpolants; location 19 (`usage3`, the
///   D3D9 NORMAL usage) receives the mesh normal `[nx, ny, nz, 0]`, the
///   rest zero.
///
/// RESOLVED (block 34): location 0 used to be POSITION — colliding
/// with texcoord0's @location(0) (both in the WGSL struct and the
/// vertex layout).  Position moved to the dedicated location 32 and
/// texcoord0 owns location 0; the packing follows.
pub fn mesh_vertex_bytes(layout: &MeshVertexLayout, mesh: &SceneMesh) -> Vec<u8> {
    let rows = (layout.stride / 16) as usize;
    let n = mesh.vertex_count();
    let mut out = Vec::with_capacity(n * layout.stride as usize);
    for i in 0..n {
        for attr in &layout.attributes {
            let row: [f32; 4] = match attr.shader_location {
                l if l == crate::shader::wgsl::VERTEX_POSITION_LOCATION as u32 => {
                    let p = mesh.positions[i];
                    [p[0], p[1], p[2], 1.0]
                }
                0 => {
                    // texcoord0 (block 34: location 0 is the FIRST UV
                    // set; position moved to the dedicated location 32).
                    mesh.uvs
                        .get(i)
                        .map(|uv| [uv[0], uv[1], 0.0, 1.0])
                        .unwrap_or([0.0, 0.0, 0.0, 1.0])
                }
                l if (1..8).contains(&l) => {
                    // texcoord1..7 — BUT locations 6/7 are the WORLD
                    // lit-pass's TBN basis inputs (block 35, PROVEN from
                    // the default pair's VS: out.texcoord3 = M·texcoord7,
                    // out.texcoord4 = M·texcoord6; the FS normalizes
                    // BOTH — a zero row there makes `nrm` produce NaN
                    // and the lit output garbage).  Feed:
                    //   texcoord7 (loc 7) ← the mesh TANGENT when the
                    //     layout carries one, else the normal;
                    //   texcoord6 (loc 6) ← N × T (the classic
                    //     right-handed binormal; the Tangent element's
                    //     ±1.0 pad word is the handedness but its sign
                    //     mapping is UNKNOWN — DESIGN), else the normal.
                    // A neutral normal map weights T/B by its r/g (0 at
                    // neutral) and the geometric normal by its blue — so
                    // the duplicated basis keeps the direction right and
                    // never NaNs.
                    if l == 7 || l == 6 {
                        let n = mesh
                            .normals
                            .get(i)
                            .copied()
                            .unwrap_or([0.0, 0.0, 1.0]);
                        let t = mesh.tangents.get(i).copied().unwrap_or(n);
                        let row = if l == 7 {
                            let len = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt();
                            if len > 1e-6 { [t[0] / len, t[1] / len, t[2] / len, 0.0] } else { [n[0], n[1], n[2], 0.0] }
                        } else {
                            let tlen = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt();
                            let nlen = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                            let tangent_is_real = tlen > 1e-6 && nlen > 1e-6;
                            if tangent_is_real {
                                // B = N × T (right-handed DESIGN; the
                                // handedness word's sign mapping is
                                // UNKNOWN — flip risk documented).
                                let b = [
                                    n[1] * t[2] - n[2] * t[1],
                                    n[2] * t[0] - n[0] * t[2],
                                    n[0] * t[1] - n[1] * t[0],
                                ];
                                let blen =
                                    (b[0] * b[0] + b[1] * b[1] + b[2] * b[2]).sqrt();
                                if blen > 1e-6 {
                                    [b[0] / blen, b[1] / blen, b[2] / blen, 0.0]
                                } else {
                                    [n[0], n[1], n[2], 0.0]
                                }
                            } else {
                                [n[0], n[1], n[2], 0.0]
                            }
                        };
                        row
                    } else {
                        // texcoord1..5: only the FIRST UV set exists —
                        // degrade to an opaque black row (multi-set
                        // packing needs the full layout table — future
                        // work, documented).
                        [0.0, 0.0, 0.0, 1.0]
                    }
                }
                l if (8..16).contains(&l) => [1.0, 1.0, 1.0, 1.0],
                l if l >= 16 && l - 16 == 3 => {
                    let nrm = mesh
                        .normals
                        .get(i)
                        .copied()
                        .unwrap_or([0.0, 0.0, 0.0]);
                    // NOTE: `nrm` is [f32;3] copied out — index freely.
                    let r = nrm[0];
                    let g = nrm[1];
                    let b = nrm[2];
                    [r, g, b, 0.0]
                }
                _ => [0.0, 0.0, 0.0, 0.0],
            };
            for v in row {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        let _ = rows; // rows == layout.attributes.len() by construction
    }
    debug_assert_eq!(out.len() as u64, n as u64 * layout.stride);
    out
}

/// A decoded texture ready for GPU upload (block 17: the material path's
/// texture half — `dxt::decode_dds` output; block 29: the full mip
/// chain when the DDS carries one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureUpload {
    /// RGBA8, row-major, `width * height * 4` bytes — mip level 0.
    pub rgba: Vec<u8>,
    /// Texel width (level 0).
    pub width: u32,
    /// Texel height (level 0).
    pub height: u32,
    /// Mip levels 1.., largest first (block 29) — empty for the
    /// base-only art textures (the DDS `dwMipMapCount` word decides;
    /// the P3D mip field is proven unreliable).  Each level is exactly
    /// `max(1, level0 >> i)` per side with packed RGBA8 bytes.
    pub mip_levels: Vec<super::dxt::DecodedSurface>,
}

/// Geometry for one real-mesh batch: layout-shaped vertex bytes (from
/// [`mesh_vertex_bytes`]) plus u16 indices, and optionally the mesh's
/// resolved diffuse texture (the proven chain mesh→shader→`color` param→
/// in-member texture lands it here already decoded).
#[derive(Debug, Clone)]
pub struct MeshBatchSource {
    /// Log label (usually the mesh name — which binds to the shader).
    pub label: String,
    /// Layout-shaped vertex bytes for the paired pipeline.
    pub vertex_bytes: Vec<u8>,
    /// u16 indices (each < vertex_count, enforced upstream).
    pub indices: Vec<u16>,
    /// The mesh's diffuse texture, when the chain resolved (else the
    /// shared white texture keeps the draw valid).
    pub texture: Option<TextureUpload>,
    /// The shader's other texture params, decoded (block 26 — the
    /// material slot chain now reaches the GPU: slot 0 = `texture`,
    /// slots 1.. = these, in `SceneMesh::extra_textures` param order).
    /// Per-slot decode failures are dropped (fail-soft, same policy as
    /// the diffuse slot).
    pub extra_textures: Vec<TextureUpload>,
}

impl MeshBatchSource {
    /// Build a source from a real mesh + its target pipeline's layout.
    /// When the mesh carries a DDS (`SceneMesh::texture_dds`), it is
    /// decoded to RGBA8 here — the FULL mip chain when the DDS ships
    /// one (block 29), else the base level; a decode failure leaves
    /// `texture` None (fail-soft, the draw stays valid with white).
    /// The same decode runs per entry of `SceneMesh::extra_textures`.
    pub fn from_mesh(layout: &MeshVertexLayout, mesh: &SceneMesh) -> MeshBatchSource {
        let texture = mesh.texture_dds.as_ref().and_then(|dds| texture_upload(dds));
        let extra_textures = mesh
            .extra_textures
            .iter()
            .filter_map(|named| texture_upload(&named.dds))
            .collect();
        MeshBatchSource {
            label: mesh.name.clone(),
            vertex_bytes: mesh_vertex_bytes(layout, mesh),
            indices: mesh.indices.clone(),
            texture,
            extra_textures,
        }
    }
}

/// Decode a raw DDS into a [`TextureUpload`] — level 0 in the flat
/// fields, levels 1.. in `mip_levels` (block 29).  `None` on any parse
/// or decode failure (the caller's fail-soft contract: the draw stays
/// valid with the shared white texture).
fn texture_upload(dds: &[u8]) -> Option<TextureUpload> {
    let surfaces = super::dxt::decode_dds_mips(dds).ok()?;
    let base = surfaces.first()?.clone();
    let mip_levels = surfaces.into_iter().skip(1).collect();
    Some(TextureUpload {
        rgba: base.rgba,
        width: base.width,
        height: base.height,
        mip_levels,
    })
}

// ---------------------------------------------------------------------------
// Material slot assignment (block 26 — device-free core of the
// multi-texture bind path)
// ---------------------------------------------------------------------------

/// The ascending s# registers of a stage's Texture bindings — the
/// material-slot order the bind path fills.
///
/// * PROVEN: `plan_stage_resources` lists sampler pairs in ascending
///   s# order, so this equals the plan's Texture-binding order (the
///   tests re-verify both properties against the REAL fragment
///   programs, including the three-slot bink program).
/// * LIKELY (the modeled decision): slot 0 (lowest s#) consumes the
///   diffuse texture (the `color` param), slot k ≥ 1 consumes
///   `SceneMesh::extra_textures[k-1]` (the shader's other texture
///   params in param order).  The original engine's actual
///   sampler-register → param binding lives engine-side (DLL); this is
///   the natural D3D9 convention and the only order the two sides of
///   the workspace share.
pub fn texture_slot_registers(res: &StageResources) -> Vec<u16> {
    let mut regs: Vec<u16> = res
        .bindings
        .iter()
        .filter_map(|b| match b {
            StageBinding::Texture { register, .. } => Some(*register),
            _ => None,
        })
        .collect();
    regs.sort_unstable();
    regs.dedup();
    regs
}

/// The upload feeding each texture slot: slot `i` (ascending-s#
/// position `i`) receives upload `i` — upload 0 is the diffuse slot,
/// uploads 1.. are the extras in param order — and `None` when the
/// uploads ran out (that slot binds the shared white view).  Uploads
/// beyond the slot count are simply not bound (harmless surplus).
///
/// Returns one entry per texture binding, in ascending-register order
/// (== plan order).  Device-free so the mapping contract is unit-tested;
/// the GPU edge applies it mechanically.
pub fn assign_texture_uploads(regs: &[u16], upload_count: usize) -> Vec<Option<usize>> {
    regs.iter()
        .enumerate()
        .map(|(i, _)| if i < upload_count { Some(i) } else { None })
        .collect()
}

// ---------------------------------------------------------------------------
// Debug-triangle GPU half — one ready-to-draw batch per pipeline.
//
// VALIDITY BY CONSTRUCTION (why there are no error scopes here): every
// buffer / bind group below is created strictly from a plan that
// `create_render_pipeline` already cross-checked against the compiled
// WGSL (`validate_plan_matches_wgsl`): one 16-byte uniform buffer per
// `StageBinding::Uniform` row (the BGL's `min_binding_size`), the shared
// 1×1 white view for every `Texture` binding, the shared filtering
// sampler for every `Sampler` binding.  If a driver still rejects one
// of these, the plan arithmetic is broken — a loud panic is the correct
// outcome (same policy as the pipeline path).
// ---------------------------------------------------------------------------

/// One uniform-row buffer of a batch, with the provenance needed to
/// upload real values into it later (block 17): the c# register it backs
/// and the exact WGSL variable name (e.g. `p3dWorldViewProjectionMatrix_r0`).
#[derive(Debug, Clone)]
pub struct BatchUniform {
    /// The c# (float) or b# (bool) register this 16-byte row backs.
    pub register: u16,
    /// The exact WGSL variable name from the validated plan.
    pub wgsl_name: String,
    /// True for a BOOL-constant row (`b#`; block 34) — the write path
    /// must not confuse it with the float row of the same index.
    pub is_bool: bool,
    /// The 16-byte UNIFORM + COPY_DST buffer bound at the plan's binding.
    pub buffer: wgpu::Buffer,
}

/// One debug triangle / real mesh made drawable through one pipeline:
/// pipeline + bind groups + buffers, all created from that pipeline's
/// validated plans.  Dropping the batch releases the GPU resources (wgpu
/// types are internally refcounted).
pub struct DebugDrawBatch {
    /// Which shader pair this batch draws through (for logs).
    pub name: String,
    /// The pipeline (owned clone — wgpu pipelines are cheap handles).
    pub pipeline: wgpu::RenderPipeline,
    /// Vertex-stage bind group (group 0; empty if the vs has no bindings).
    pub vertex_bind_group: wgpu::BindGroup,
    /// Fragment-stage bind group (group 1; empty if the fs has none).
    pub fragment_bind_group: wgpu::BindGroup,
    /// Layout-shaped vertex buffer (debug triangle or REAL mesh bytes).
    pub vertex_buffer: wgpu::Buffer,
    /// u16 index buffer.
    pub index_buffer: wgpu::Buffer,
    /// Indices to draw (3 for the debug triangle, `indices.len()` for a
    /// real mesh).  Since block 16 the draw call consumes this — real
    /// meshes flow through the identical encode sequence.
    pub index_count: u32,
    /// The vertex stage's uniform-row buffers (zero-filled until
    /// [`Renderer::write_uniform_row`]-family calls upload real values).
    pub vertex_uniforms: Vec<BatchUniform>,
    /// The fragment stage's uniform-row buffers.
    pub fragment_uniforms: Vec<BatchUniform>,
}

/// Descriptor of one zero-filled 16-byte uniform-row buffer (the exact
/// shape of every `StageBinding::Uniform` binding: `min_binding_size =
/// ROW_SIZE`, UNIFORM + COPY_DST, mapped at creation so the caller can
/// zero it before unmapping).
pub fn uniform_row_buffer_descriptor(label: &str) -> wgpu::BufferDescriptor<'_> {
    wgpu::BufferDescriptor {
        label: Some(label),
        size: ROW_SIZE,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: true,
    }
}

/// Descriptor of the shared 1×1 white RGBA8Unorm texture bound to every
/// `Texture` binding of the debug path (white so textured triangles show
/// a white triangle rather than black; DESIGN).
pub fn debug_texture_descriptor() -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: Some("prototype-rs debug 1x1 white"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    }
}

/// Descriptor of the shared filtering sampler for every `Sampler`
/// binding (linear, clamp — matches the BGL's `Filtering` type).
pub fn debug_sampler_descriptor() -> wgpu::SamplerDescriptor<'static> {
    wgpu::SamplerDescriptor {
        label: Some("prototype-rs debug sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Tests — device-free, against REAL fixture shaders
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {

    // -- REAL mesh packing (block 16, device-free) ------------------------

    fn two_vertex_mesh() -> SceneMesh {
        SceneMesh {
            name: "testmesh".into(),
            desc: 0x2011,
            positions: vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]],
            normals: vec![[0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            uvs: vec![[0.25, 0.75], [0.5, 0.5]],
            tangents: Vec::new(),
            indices: vec![0, 1, 0],
            bound_box: None,
            bound_sphere: None,
            texture_dds: None,
            extra_textures: Vec::new(),
        }
    }

    fn layout_pos_uv_normal() -> MeshVertexLayout {
        // The block-34 location convention: texcoord0 @loc0 (the mesh's
        // UVs), normal (usage3) @loc19, position0 @loc32 (the dedicated
        // vertex INPUT slot) — 3 rows, 48-B stride.
        MeshVertexLayout {
            stride: 48,
            attributes: vec![
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 0, shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 16, shader_location: 19 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 32, shader_location: 32 },
            ],
        }
    }

    #[test]
    fn mesh_vertex_bytes_pack_positions_uvs_and_normals() {
        let layout = layout_pos_uv_normal();
        let mesh = two_vertex_mesh();
        let bytes = mesh_vertex_bytes(&layout, &mesh);
        assert_eq!(bytes.len(), 2 * 48);
        let f = |vertex: usize, row: usize, comp: usize| -> f32 {
            let o = vertex * 48 + row * 16 + comp * 4;
            f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]])
        };
        // vertex 0: texcoord0 row (location 0 — the mesh's UVs since
        // block 34 moved position to the dedicated location 32)
        assert_eq!([f(0, 0, 0), f(0, 0, 1), f(0, 0, 3)], [0.25, 0.75, 1.0]);
        // normal row (usage3 at loc 19)
        assert_eq!([f(0, 1, 0), f(0, 1, 1), f(0, 1, 2), f(0, 1, 3)], [0.0, 1.0, 0.0, 0.0]);
        // position row (loc 32)
        assert_eq!([f(0, 2, 0), f(0, 2, 1), f(0, 2, 2), f(0, 2, 3)], [1.0, 2.0, 3.0, 1.0]);
        // vertex 1
        assert_eq!([f(1, 0, 0), f(1, 0, 1)], [0.5, 0.5]);
        assert_eq!([f(1, 1, 0), f(1, 1, 1), f(1, 1, 2)], [0.0, 0.0, 1.0]);
        assert_eq!([f(1, 2, 0), f(1, 2, 1), f(1, 2, 2)], [4.0, 5.0, 6.0]);
    }

    #[test]
    fn mesh_vertex_bytes_degrade_missing_sets_and_colors() {
        // A layout with texcoord1 (loc 2) and color0 (loc 8) rows
        // (location 0 = texcoord0 — degrades to u=0 when UVs are gone).
        let layout = MeshVertexLayout {
            stride: 32,
            attributes: vec![
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 0, shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 16, shader_location: 2 },
            ],
        };
        let mut mesh = two_vertex_mesh();
        mesh.uvs.clear();
        let bytes = mesh_vertex_bytes(&layout, &mesh);
        assert_eq!(bytes.len(), 2 * 32);
        let o = 16; // vertex 0, row 1
        let v = f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        assert_eq!(v, 0.0, "texcoord1 degrades to u=0 (only UV set 0 exists)");
        // and a color row is opaque white
        let layout_c = MeshVertexLayout {
            stride: 32,
            attributes: vec![
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 0, shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 16, shader_location: 8 },
            ],
        };
        let bytes_c = mesh_vertex_bytes(&layout_c, &two_vertex_mesh());
        let comps: Vec<f32> = (0..4)
            .map(|c| f32::from_le_bytes([
                bytes_c[16 + c * 4],
                bytes_c[16 + c * 4 + 1],
                bytes_c[16 + c * 4 + 2],
                bytes_c[16 + c * 4 + 3],
            ]))
            .collect();
        assert_eq!(comps, vec![1.0, 1.0, 1.0, 1.0]);
    }

    /// MeshBatchSource carries the real index count that the draw call
    /// consumes (1074 for the barrier mesh).
    #[test]
    fn mesh_batch_source_carries_real_index_count() {
        let buf = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("prototype-bin")
                .join("tests")
                .join("fixtures")
                .join("art_barrier_os002.p3d"),
        )
        .unwrap();
        let top = prototype_p3d::geometry::top_level_chunks(&buf).unwrap();
        let shape = top
            .iter()
            .find(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_SHAPE)
            .map(|c| prototype_p3d::geometry::decode_shape(&buf, c.offset, c.size).unwrap())
            .unwrap();
        let mesh = SceneMesh::from_p3d_mesh(&shape.meshes[0]);
        let layout = layout_pos_uv_normal();
        let src = MeshBatchSource::from_mesh(&layout, &mesh);
        assert_eq!(src.label, "131185ef20d24489daf1165e65814182");
        assert_eq!(src.indices.len(), 1074);
        assert_eq!(src.vertex_bytes.len(), 576 * 48);
    }
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
    fn vertex_layout_matches_vs_in_locations() {
        // The first real vertex program ("CxformGauraud - lit") declares:
        //   Position v0 -> location 32 (block 34: the dedicated vertex
        //   INPUT slot — it used to collide with texcoord0's location 0),
        //   Color v1 -> location 8, Color v2 -> location 9
        // (interpolant_location: TexCoord->idx, Color->8+idx).  Locations are
        // NON-CONTIGUOUS — valid for wgpu — while buffer offsets stay compact.
        let programs = all_programs();
        let (name, is_fragment, tokens) = programs
            .iter()
            .find(|(n, f, _)| !*f && n.contains("CxformGauraud - lit"))
            .expect("vertex CxformGauraud - lit");
        let vs = Sm30Shader::parse(tokens, *is_fragment).unwrap();
        let layout = plan_vertex_layout(&vs);
        assert_eq!(layout.attributes.len(), 3, "{name}");
        assert_eq!(layout.stride, 48);
        let locs: Vec<u32> = layout.attributes.iter().map(|a| a.shader_location).collect();
        assert_eq!(locs, vec![32, 8, 9], "{name}");
        assert!(layout.attributes.iter().all(|a| a.format == wgpu::VertexFormat::Float32x4));
        // Offsets are compact: 0, 16, 32.
        assert_eq!(layout.attributes[0].offset, 0);
        assert_eq!(layout.attributes[1].offset, 16);
        assert_eq!(layout.attributes[2].offset, 32);
        // And the WGSL's VsIn struct must agree on the locations.
        let wgsl = translate_program(&vs, name).unwrap();
        assert!(wgsl.contains("@location(32) position0: vec4<f32>"), "{wgsl}");
        assert!(wgsl.contains("@location(8) color0: vec4<f32>"), "{wgsl}");
        assert!(wgsl.contains("@location(9) color1: vec4<f32>"), "{wgsl}");
        // The layout must borrow cleanly into a wgpu VertexBufferLayout.
        let borrowed = layout.as_layout();
        assert_eq!(borrowed.array_stride, 48);
        assert_eq!(borrowed.attributes.len(), 3);
    }

    #[test]
    fn bind_group_layout_entries_match_plan() {
        // A textured fragment program: N uniform rows + one texture +
        // one sampler — entry types must follow the plan exactly.
        let programs = all_programs();
        let (name, is_fragment, tokens) = programs
            .iter()
            .find(|(n, f, _)| *f && n.contains("cxformtexture"))
            .expect("fragment cxformtexture");
        let fs = Sm30Shader::parse(tokens, *is_fragment).unwrap();
        let res = plan_stage_resources(&fs);
        let entries = bind_group_layout_entries(&res, wgpu::ShaderStages::FRAGMENT);
        assert_eq!(entries.len(), res.bindings.len());
        // First entries are uniform buffers (binding 0..k), then texture,
        // then sampler.
        let mut saw_texture = false;
        for e in &entries {
            match &e.ty {
                wgpu::BindingType::Buffer { ty, min_binding_size, .. } => {
                    assert!(!saw_texture, "uniform after texture = plan disorder");
                    assert_eq!(*ty, wgpu::BufferBindingType::Uniform);
                    assert_eq!(min_binding_size.unwrap().get(), 16);
                    assert_eq!(e.visibility, wgpu::ShaderStages::FRAGMENT);
                }
                wgpu::BindingType::Texture { .. } => saw_texture = true,
                wgpu::BindingType::Sampler(_) => assert!(saw_texture),
                other => panic!("unexpected binding type {other:?}"),
            }
        }
        assert!(saw_texture, "{name} should be textured");
    }

    #[test]
    fn textured_fragment_has_sampler_pair_and_vertex_has_none() {
        let programs = all_programs();
        let mut saw_textured_fs = false;
        let mut saw_vs = false;
        for (name, is_fragment, tokens) in &programs {
            let s = Sm30Shader::parse(tokens, *is_fragment).unwrap();
            let res = plan_stage_resources(&s);
            if *is_fragment {
                let textures = res
                    .bindings
                    .iter()
                    .filter(|b| matches!(b, StageBinding::Texture { .. }))
                    .count();
                let samplers = res
                    .bindings
                    .iter()
                    .filter(|b| matches!(b, StageBinding::Sampler { .. }))
                    .count();
                assert_eq!(textures, samplers, "{name}: texture/sampler must pair");
                if textures > 0 {
                    saw_textured_fs = true;
                }
            } else {
                assert_eq!(res.sampler_count(), 0, "{name}: vs never samples");
                saw_vs = true;
            }
        }
        assert!(saw_textured_fs && saw_vs);
    }

    // -- debug triangle contract (NEXT-UP item 1, device-free half) -----

    #[test]
    fn debug_triangle_matches_every_real_vertex_layout() {
        // For EVERY real vertex program: byte length exactly
        // 3 * stride, and each corner's first row decodes to the
        // expected clip-space corner.
        for (name, is_fragment, tokens) in all_programs() {
            if is_fragment {
                continue;
            }
            let vs = Sm30Shader::parse(&tokens, is_fragment)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let layout = plan_vertex_layout(&vs);
            let bytes = debug_triangle_vertices(&layout);
            assert_eq!(
                bytes.len() as u64,
                3 * layout.stride,
                "{name}: 3 vertices of stride {}",
                layout.stride
            );
            // First corner, first row: [-0.75, -0.75, 0.5, 1.0].
            let x = f32::from_le_bytes(bytes[0..4].try_into().unwrap());
            let y = f32::from_le_bytes(bytes[4..8].try_into().unwrap());
            let z = f32::from_le_bytes(bytes[8..12].try_into().unwrap());
            let w = f32::from_le_bytes(bytes[12..16].try_into().unwrap());
            assert_eq!((x, y, z, w), (-0.75, -0.75, 0.5, 1.0), "{name}");
            // Last corner's final row ends the buffer with w = 1.0.
            let tail = &bytes[bytes.len() - 4..];
            assert_eq!(f32::from_le_bytes(tail.try_into().unwrap()), 1.0, "{name}");
        }
    }

    #[test]
    fn debug_triangle_bytes_are_valid_le_f32s_throughout() {
        // Synthetic 4-attribute layout: all 3*4 rows must decode as
        // finite f32s and each row must be the shared corner row.
        let layout = MeshVertexLayout {
            stride: 4 * 16,
            attributes: (0..4)
                .map(|i: usize| wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: (i as u64) * 16,
                    shader_location: i as u32,
                })
                .collect(),
        };
        let bytes = debug_triangle_vertices(&layout);
        assert_eq!(bytes.len(), 3 * 64);
        for (vi, chunk) in bytes.chunks_exact(64).enumerate() {
            let (x, y) = (
                DEBUG_TRIANGLE_CORNERS[vi][0],
                DEBUG_TRIANGLE_CORNERS[vi][1],
            );
            for row in chunk.chunks_exact(16) {
                let vals: [f32; 4] = std::array::from_fn(|k| {
                    f32::from_le_bytes(row[k * 4..k * 4 + 4].try_into().unwrap())
                });
                assert_eq!(vals, [x, y, 0.5, 1.0]);
                assert!(vals.iter().all(|v| v.is_finite()));
            }
        }
    }

    #[test]
    fn debug_triangle_indices_are_a_single_ccw_triangle() {
        assert_eq!(debug_triangle_indices(), vec![0, 1, 2]);
    }

    // ------------------------------------------------------------------
    // Block 21: debug cube + WVP·model rows (device-free)
    // ------------------------------------------------------------------

    #[test]
    fn debug_cube_geometry_has_the_documented_shape() {
        assert_eq!(DEBUG_CUBE_CORNERS.len(), 36, "12 triangles × 3 corners");
        // Indices are the identity run 0..36.
        assert_eq!(debug_cube_indices(), (0..36u16).collect::<Vec<_>>());
        // Every corner row sits inside [-0.5, 0.5]³ and on EXACTLY 3
        // face planes (all rows are true cube vertices) — 36 rows × 3
        // faces / 6 planes = 18 rows per plane.
        for c in DEBUG_CUBE_CORNERS {
            for v in c {
                assert!((-0.5..=0.5).contains(&v), "corner {c:?}");
            }
            let on = [
                (c[2] - 0.5).abs() < 1e-6, // +Z
                (c[2] + 0.5).abs() < 1e-6, // -Z
                (c[0] - 0.5).abs() < 1e-6, // +X
                (c[0] + 0.5).abs() < 1e-6, // -X
                (c[1] - 0.5).abs() < 1e-6, // +Y
                (c[1] + 0.5).abs() < 1e-6, // -Y
            ];
            assert_eq!(on.iter().filter(|b| **b).count(), 3, "corner {c:?} on three faces");
        }
        // All 8 distinct cube vertices appear.
        let distinct: std::collections::BTreeSet<[u8; 3]> = DEBUG_CUBE_CORNERS
            .iter()
            .map(|c| [(c[0] + 0.5) as u8, (c[1] + 0.5) as u8, (c[2] + 0.5) as u8])
            .collect();
        assert_eq!(distinct.len(), 8, "8 distinct cube vertices: {distinct:?}");
    }

    #[test]
    fn debug_cube_vertices_match_layout_stride() {
        let layout = MeshVertexLayout {
            stride: 48, // 3 attribute rows
            attributes: Vec::new(),
        };
        let bytes = debug_cube_vertices(&layout);
        assert_eq!(bytes.len() as u64, 36 * 48);
        // First corner row is (-0.5, -0.5, 0.5, 1.0) little-endian,
        // repeated for every attribute row of that vertex.
        let first: Vec<u8> = [-0.5f32, -0.5, 0.5, 1.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(&bytes[..16], &first[..]);
        assert_eq!(&bytes[16..32], &first[..]);
        // The 7th corner row (index 6) starts the −Z face.
        let seventh: Vec<u8> = [0.5f32, -0.5, -0.5, 1.0].iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(&bytes[6 * 48..6 * 48 + 16], &seventh[..]);
    }

    /// Identity model ⇒ exactly the camera's own rows; a translation
    /// model moves a model-space point to the same clip position the
    /// camera assigns to the world-space point (the composition
    /// contract the cube upload relies on).
    #[test]
    fn view_projection_model_rows_compose_correctly() {
        let cam = Camera {
            eye: prototype_math::Vec3::new(0.0, 2.0, 5.0),
            target: prototype_math::Vec3::ZERO,
            up: prototype_math::Vec3::UP,
            fov_y: 1.2,
            aspect: 1.0,
            near: 0.1,
            far: 100.0,
        };
        // Identity: rows equal the plain view-projection rows.
        let identity = view_projection_model_rows(&cam, &prototype_math::Mat4::IDENTITY);
        assert_eq!(identity, cam.view_projection_rows());

        // Model translate +2 on x: model point (0,0,0) must land where
        // the camera places world point (2,0,0).
        let model = prototype_math::Mat4::from_translation(prototype_math::Vec3::new(2.0, 0.0, 0.0));
        let rows = view_projection_model_rows(&cam, &model);
        let apply = |r: [[f32; 4]; 4], p: [f32; 4]| -> [f32; 4] {
            let mut o = [0.0f32; 4];
            for (i, row) in r.iter().enumerate() {
                o[i] = row[0] * p[0] + row[1] * p[1] + row[2] * p[2] + row[3] * p[3];
            }
            o
        };
        let via_model = apply(rows, [0.0, 0.0, 0.0, 1.0]);
        let direct = apply(cam.view_projection_rows(), [2.0, 0.0, 0.0, 1.0]);
        for i in 0..4 {
            assert!(
                (via_model[i] - direct[i]).abs() < 1e-4,
                "row {i}: {via_model:?} vs {direct:?}"
            );
        }
    }

    #[test]
    fn debug_resource_descriptors_match_the_plan_shapes() {
        // The bind-group path relies on these exact shapes: a 16-byte
        // uniform row (ROW_SIZE / the BGL's min_binding_size), a 1×1
        // RGBA8Unorm bindable texture, a filtering sampler.
        let b = uniform_row_buffer_descriptor("test row");
        assert_eq!(b.size, ROW_SIZE);
        assert_eq!(b.size, 16);
        assert!(b.usage.contains(wgpu::BufferUsages::UNIFORM));
        assert!(b.usage.contains(wgpu::BufferUsages::COPY_DST));
        assert!(b.mapped_at_creation);

        let t = debug_texture_descriptor();
        assert_eq!((t.size.width, t.size.height, t.size.depth_or_array_layers), (1, 1, 1));
        assert_eq!(t.format, wgpu::TextureFormat::Rgba8Unorm);
        assert_eq!(t.dimension, wgpu::TextureDimension::D2);
        assert!(t.usage.contains(wgpu::TextureUsages::TEXTURE_BINDING));

        let s = debug_sampler_descriptor();
        assert_eq!(s.mag_filter, wgpu::FilterMode::Linear);
        assert_eq!(s.min_filter, wgpu::FilterMode::Linear);
        assert_eq!(s.address_mode_u, wgpu::AddressMode::ClampToEdge);
    }

    // ------------------------------------------------------------------
    // Block 26: multi-texture material slots (device-free)
    // ------------------------------------------------------------------

    /// The real bink fragment program (shaders.rcf) declares THREE
    /// samplers — the multi-slot case exists on real shipped data.  Its
    /// slot registers must be ascending, unique, and equal to the plan's
    /// Texture-binding order (so slot i == plan position i).
    #[test]
    fn bink_fragment_plan_has_three_ascending_texture_slots() {
        let rcf = fixture("shaders.rcf");
        let arc = prototype_rcf::RcfArchive::parse(&rcf).unwrap();
        let mut found = false;
        for e in &arc.entries {
            let payload = arc.payload(e).unwrap();
            if !payload.starts_with(b"P3D\xff") {
                continue;
            }
            let f = P3dFile::parse(payload).unwrap();
            for p in f.shader_programs().unwrap() {
                if p.is_fragment && p.name.contains("bink") {
                    let fs = Sm30Shader::parse(&p.d3d9_tokens, true).unwrap();
                    let res = plan_stage_resources(&fs);
                    let regs = texture_slot_registers(&res);
                    assert_eq!(regs.len(), 3, "{:?}: {regs:?}", p.name);
                    let mut sorted = regs.clone();
                    sorted.sort_unstable();
                    sorted.dedup();
                    assert_eq!(regs, sorted, "{:?}: ascending + unique", p.name);
                    let plan_order: Vec<u16> = res
                        .bindings
                        .iter()
                        .filter_map(|b| match b {
                            StageBinding::Texture { register, .. } => Some(*register),
                            _ => None,
                        })
                        .collect();
                    assert_eq!(regs, plan_order, "{:?}: plan order == slot order", p.name);
                    found = true;
                }
            }
        }
        assert!(found, "bink fragment program not found in shaders.rcf");
    }

    /// The slot-fill contract: upload i feeds slot i (slot 0 = diffuse,
    /// 1.. = extras in param order); slots past the uploads fall back to
    /// the shared white view (None); surplus uploads are not bound.
    #[test]
    fn assign_texture_uploads_fills_then_falls_back() {
        // bink-shaped: 3 slots, diffuse + one extra → third slot white.
        assert_eq!(
            assign_texture_uploads(&[0, 1, 2], 2),
            vec![Some(0), Some(1), None]
        );
        // No uploads at all: every slot stays white (the pre-block-26
        // behaviour for multi-slot shaders was diffuse-on-every-slot).
        assert_eq!(assign_texture_uploads(&[0, 1, 2], 0), vec![None, None, None]);
        // Fully covered: a 4-slot world shader with all 4 uploads
        // (the observed world-shader slot maximum).
        assert_eq!(
            assign_texture_uploads(&[0, 1, 2, 3], 4),
            vec![Some(0), Some(1), Some(2), Some(3)]
        );
        // Surplus uploads are ignored; an empty plan maps to nothing.
        assert_eq!(assign_texture_uploads(&[4], 3), vec![Some(0)]);
        assert_eq!(assign_texture_uploads(&[], 2), Vec::<Option<usize>>::new());
    }

    /// MeshBatchSource decodes BOTH material slots of a real fixture:
    /// cells82_multitex.p3d carries the zCBV2_env_color_glass shader
    /// whose `color` (gen_c1_02_win2 diffuse) AND `normal` textures are
    /// embedded in-member.  The SceneMesh is assembled exactly the way
    /// the android collectors populate it (color → texture_dds, normal →
    /// extra_textures), and a garbage extra is dropped fail-soft.
    #[test]
    fn mesh_batch_source_decodes_diffuse_and_extra_slots() {
        let buf = fixture("cells82_multitex.p3d");
        let top = prototype_p3d::geometry::top_level_chunks(&buf).unwrap();
        let dds: Vec<Vec<u8>> = top
            .iter()
            .filter(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_TEXTURE)
            .map(|c| {
                prototype_p3d::geometry::decode_texture(&buf, c.offset, c.size)
                    .unwrap()
                    .dds
            })
            .collect();
        assert!(dds.len() >= 2, "fixture must embed both slots: {}", dds.len());

        let mut mesh = two_vertex_mesh();
        mesh.texture_dds = Some(dds[0].clone());
        mesh.extra_textures = vec![
            crate::scene::NamedDds { slot: "normal".into(), dds: dds[1].clone() },
            crate::scene::NamedDds { slot: "bogus".into(), dds: vec![0u8; 64] },
        ];
        let layout = layout_pos_uv_normal();
        let src = MeshBatchSource::from_mesh(&layout, &mesh);
        let diffuse = src.texture.as_ref().expect("diffuse decodes");
        assert!(diffuse.width > 0 && diffuse.height > 0);
        assert_eq!(
            diffuse.rgba.len(),
            (diffuse.width as usize) * (diffuse.height as usize) * 4
        );
        // The valid extra decodes; the garbage one is dropped fail-soft.
        assert_eq!(src.extra_textures.len(), 1, "bogus DDS filtered out");
        assert!(src.extra_textures[0].width > 0);
        assert_eq!(src.indices.len(), 3);
        assert_eq!(src.vertex_bytes.len(), 2 * 48);
    }

    /// Block 29: `from_mesh` carries the FULL mip chain when the DDS
    /// ships one — the multitex fixture's BC1 diffuse (9 levels) lands
    /// as base + 8 chained levels with exact per-level dimensions, and
    /// a base-only DDS (the art barrier) lands with no extra levels.
    /// Device-free: no Renderer is involved.
    #[test]
    fn mesh_batch_source_carries_full_mip_chains() {
        let load_dds = |name: &str| {
            let buf = fixture(name);
            let top = prototype_p3d::geometry::top_level_chunks(&buf).unwrap();
            top.iter()
                .find(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_TEXTURE)
                .map(|c| prototype_p3d::geometry::decode_texture(&buf, c.offset, c.size).unwrap())
                .unwrap()
                .dds
        };
        let layout = layout_pos_uv_normal();

        // Chained: cells win2 diffuse, 256² BC1, 9 levels.
        let mut mesh = two_vertex_mesh();
        mesh.texture_dds = Some(load_dds("cells82_multitex.p3d"));
        let src = MeshBatchSource::from_mesh(&layout, &mesh);
        let tex = src.texture.as_ref().expect("diffuse decodes");
        assert_eq!((tex.width, tex.height), (256, 256));
        assert_eq!(tex.mip_levels.len(), 8, "levels 1..8 of the 9-level chain");
        for (i, m) in tex.mip_levels.iter().enumerate() {
            let w = (256u32 >> (i + 1)).max(1);
            assert_eq!((m.width, m.height), (w, w), "level {}", i + 1);
            assert_eq!(m.rgba.len(), m.width as usize * m.height as usize * 4);
        }
        assert_eq!((tex.mip_levels[7].width, tex.mip_levels[7].height), (1, 1));

        // Base-only: the art barrier ships dwMipMapCount 0.
        let mut mesh = two_vertex_mesh();
        mesh.texture_dds = Some(load_dds("art_barrier_os002.p3d"));
        let src = MeshBatchSource::from_mesh(&layout, &mesh);
        let tex = src.texture.as_ref().expect("diffuse decodes");
        assert_eq!((tex.width, tex.height), (256, 256));
        assert!(tex.mip_levels.is_empty(), "base-only DDS carries no extra levels");
    }

    // -------------------------------------------------------------------
    // (block 54) The fullscreen movie quad — OFFSCREEN device test.
    // Skips when the host has no wgpu adapter (this sandbox has none;
    // a GPU host runs the full encode + readback below).
    // -------------------------------------------------------------------

    /// The quad drawn 1:1 (8×8 video texture on an 8×8 target) must show
    /// the uploaded pixels EXACTLY: a pixel centre's UV lands on a texel
    /// centre when the dims match, so linear filtering degenerates to the
    /// pure texel value — every one of the 64 pixels is asserted, which
    /// also pins the V-orientation (no flip) and the opaque alpha.
    #[test]
    fn video_quad_offscreen_draw_matches_the_uploaded_frame() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = match pollster::block_on(
            instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
        ) {
            Ok(a) => a,
            Err(_) => {
                eprintln!("skip: no wgpu adapter on this host (offscreen quad test)");
                return;
            }
        };
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("prototype-rs video quad offscreen test"),
                ..Default::default()
            },
        ))
        .expect("offscreen device");

        const N: u32 = 8;
        // Video texture: texel (x,y) = (x·32, y·32, 128, 255) — distinct
        // per pixel, easy to recognise in the readback.
        let mut rgba = Vec::with_capacity((N * N * 4) as usize);
        for y in 0..N {
            for x in 0..N {
                rgba.extend_from_slice(&[x as u8 * 32, y as u8 * 32, 128, 255]);
            }
        }
        let video = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("video quad test texture"),
            size: wgpu::Extent3d { width: N, height: N, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &video, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: None, rows_per_image: None },
            wgpu::Extent3d { width: N, height: N, depth_or_array_layers: 1 },
        );
        let video_view = video.create_view(&wgpu::TextureViewDescriptor::default());

        // Offscreen 8×8 render target (the same dims — the 1:1 trick).
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("video quad test target"),
            size: wgpu::Extent3d { width: N, height: N, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

        let quad = create_video_quad(&device, wgpu::TextureFormat::Rgba8Unorm);
        let bg = create_video_bind_group(&device, &quad, &video_view);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("video quad offscreen test"),
        });
        Renderer::encode_video_pass(&mut encoder, &target_view, &quad, &bg);

        // Readback: 8·4 = 32 B/row padded to the 256-B copy alignment.
        let out = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("video quad test readback"),
            size: 256 * N as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &out,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: None },
            },
            wgpu::Extent3d { width: N, height: N, depth_or_array_layers: 1 },
        );
        queue.submit(Some(encoder.finish()));
        let slice = out.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        // The poll status reports queue-empty state, not the map —
        // the mapped-range read below is the actual result channel.
        let _ = device.poll(wgpu::PollType::Wait);
        let data = slice.get_mapped_range();
        for y in 0..N {
            for x in 0..N {
                let o = (y * 256) as usize + (x * 4) as usize;
                let px = [data[o], data[o + 1], data[o + 2], data[o + 3]];
                let want = [x as u8 * 32, y as u8 * 32, 128, 255];
                assert_eq!(px, want, "pixel ({x},{y}) must be the uploaded texel");
            }
        }
        drop(data);
        out.unmap();
    }
}

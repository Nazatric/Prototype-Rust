//! Block 107 — the menu/UI text layer: a font atlas + ortho quad
//! renderer, the pieces the REAL Prototype menu screens draw through.
//!
//! The real game's menus are Scaleform GFx (`art\hud\protomainmenu.gfx`
//! and 24 siblings — the catalog is PROVEN from the engine DLL's own
//! strings; see `prototype_android::menu`).  Rebuilding a Scaleform
//! runtime is its own project; the MVP menu draws the SAME screens
//! natively — the real screen names, the real flow, game-styled —
//! through this layer, and `menu_assets.rs` already locates the real
//! `.gfx` members in the user's corpus (proven `hash_name`) for the
//! art bridge.
//!
//! * [`FontAtlas`] — rasterize a bundled TTF (DejaVu Sans Bold, free
//!   license) into one RGBA canvas + per-glyph metrics.  Pure CPU —
//!   tested without a GPU.
//! * [`layout_text`] / [`layout_rect`] — build [`UiVertex`] lists
//!   (screen-space pixels).
//! * [`UiRenderer`] — the GPU half (feature `render-wgpu`): one
//!   pipeline + the atlas texture, drawn as a second (alpha-blended)
//!   pass after the world pass.

/// The bundled menu font — DejaVu Sans Bold (free license, public
/// domain-ish per its COPYRIGHT; the closest open face to the game's
/// bold condensed UI look, and the honest stand-in until the real
/// `.gfx` font assets are extracted from the user's corpus).
pub const MENU_FONT_BYTES: &[u8] = include_bytes!("../assets/DejaVuSans-Bold.ttf");

/// A glyph inside the atlas canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphSlot {
    /// Pixel rect inside the atlas (x, y, w, h).
    pub rect: [u32; 4],
    /// Horizontal advance at the atlas's raster size.
    pub advance: f32,
    /// Left / top bearing at the atlas raster size.
    pub bearing: [f32; 2],
}

/// CPU-side font atlas: rasterize the UI charset once, lay out text
/// forever.  The atlas raster size is [`ATLAS_PX`] — menus draw at
/// this size or scale their layout (crisp enough for phone + desktop).
pub struct FontAtlas {
    pub font: fontdue::Font,
    /// Atlas canvas (RGBA8, white glyphs, alpha-shaped).
    pub canvas: Vec<u8>,
    pub canvas_size: u32,
    pub glyphs: std::collections::HashMap<char, GlyphSlot>,
    /// The raster pixel size every glyph metric is valid at.
    pub raster_px: f32,
}

/// Atlas raster size (px) and canvas edge.
pub const ATLAS_PX: f32 = 36.0;
const CANVAS: u32 = 1024;

/// The charset the menus need: ASCII + the punctuation the UI uses.
fn charset() -> Vec<char> {
    let mut chars: Vec<char> = (32u8..127u8).map(|b| b as char).collect();
    for c in ['→', '←', '↑', '↓', '✓', '…', '•', '%', '—', '·', '◀'] {
        chars.push(c);
    }
    chars
}

impl FontAtlas {
    /// Rasterize the menu charset into the atlas canvas.
    pub fn new() -> Self {
        let font =
            fontdue::Font::from_bytes(MENU_FONT_BYTES, fontdue::FontSettings::default())
                .expect("the bundled menu font must always parse");
        let mut canvas = vec![0u8; (CANVAS * CANVAS * 4) as usize];
        let mut glyphs = std::collections::HashMap::new();
        let mut cursor = (1u32, 1u32);
        let mut row_h = 0u32;
        for c in charset() {
            let (m, bitmap) = font.rasterize(c, ATLAS_PX);
            let w = m.width as u32;
            let h = m.height as u32;
            if w == 0 || h == 0 {
                // Blank glyph (the space): keep the real advance, no
                // texels — text layout stays honest across the charset.
                glyphs.insert(
                    c,
                    GlyphSlot {
                        rect: [0, 0, 0, 0],
                        advance: m.advance_width,
                        bearing: [m.xmin as f32, m.ymin as f32],
                    },
                );
                continue;
            }
            // Row wrap (simple shelf packing).
            if cursor.0 + w + 1 > CANVAS {
                cursor = (1, cursor.1 + row_h + 1);
                row_h = 0;
            }
            if cursor.1 + h >= CANVAS {
                // The charset fits a 1024² shelf at 36 px by a wide
                // margin; a runaway here is a bug, fail loud in tests.
                panic!("font atlas overflow — enlarge CANVAS");
            }
            for yy in 0..h {
                for xx in 0..w {
                    let a = bitmap[(yy * w + xx) as usize] as u32;
                    let di = (((cursor.1 + yy) * CANVAS + (cursor.0 + xx)) * 4) as usize;
                    // White glyph, alpha = coverage (shaders multiply).
                    canvas[di] = 255;
                    canvas[di + 1] = 255;
                    canvas[di + 2] = 255;
                    canvas[di + 3] = a.min(255) as u8;
                }
            }
            glyphs.insert(
                c,
                GlyphSlot {
                    rect: [cursor.0, cursor.1, w, h],
                    advance: m.advance_width,
                    bearing: [m.xmin as f32, m.ymin as f32],
                },
            );
            cursor.0 += w + 1;
            row_h = row_h.max(h);
        }
        Self {
            font,
            canvas,
            canvas_size: CANVAS,
            glyphs,
            raster_px: ATLAS_PX,
        }
    }

    /// Measure a string's width/height at `px` (scaled from the atlas
    /// metrics).
    pub fn measure(&self, text: &str, px: f32) -> [f32; 2] {
        let s = px / self.raster_px;
        let mut w = 0.0f32;
        let mut h = 0.0f32;
        for c in text.chars() {
            match self.glyphs.get(&c) {
                Some(g) => {
                    w += g.advance * s;
                    h = h.max((g.rect[3] as f32) * s);
                }
                None => w += px * 0.3,
            }
        }
        [w, h]
    }
}

impl Default for FontAtlas {
    fn default() -> Self {
        Self::new()
    }
}

/// One screen-space UI vertex (pixels; y DOWN like the OS).
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct UiVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

/// CPU-built UI frame: a triangle-list of [`UiVertex`] plus the
/// current screen size (the ortho basis).
#[derive(Debug, Default, Clone)]
pub struct UiDrawList {
    pub verts: Vec<UiVertex>,
    pub screen: [f32; 2],
}

impl UiDrawList {
    /// Push a solid rect (x, y top-left, w, h, rgba) as two triangles.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        let (x0, y0, x1, y1) = (x, y, x + w, y + h);
        for v in [
            [x0, y0],
            [x1, y0],
            [x1, y1],
            [x0, y0],
            [x1, y1],
            [x0, y1],
        ] {
            self.verts.push(UiVertex {
                pos: v,
                uv: [0.0, 0.0],
                color,
            });
        }
    }

    /// Push a string at `px` size with the atlas's glyph metrics.
    /// `color` multiplies the (white, alpha-shaped) atlas texels.
    pub fn text(&mut self, atlas: &FontAtlas, x: f32, y: f32, s: &str, px: f32, color: [f32; 4]) {
        let scale = px / atlas.raster_px;
        let inv = 1.0 / atlas.canvas_size as f32;
        let mut pen_x = x;
        for c in s.chars() {
            let Some(g) = atlas.glyphs.get(&c) else {
                pen_x += px * 0.3;
                continue;
            };
            let [gx, gy, gw, gh] = g.rect;
            let dx = pen_x + g.bearing[0] * scale;
            let dy = y + g.bearing[1] * scale;
            let (x0, y0, x1, y1) = (dx, dy, dx + gw as f32 * scale, dy + gh as f32 * scale);
            let (u0, v0, u1, v1) = (
                gx as f32 * inv,
                gy as f32 * inv,
                (gx + gw) as f32 * inv,
                (gy + gh) as f32 * inv,
            );
            for (pos, uv) in [
                ([x0, y0], [u0, v0]),
                ([x1, y0], [u1, v0]),
                ([x1, y1], [u1, v1]),
                ([x0, y0], [u0, v0]),
                ([x1, y1], [u1, v1]),
                ([x0, y1], [u0, v1]),
            ] {
                self.verts.push(UiVertex {
                    pos,
                    uv,
                    color,
                });
            }
            pen_x += g.advance * scale;
        }
    }

    /// Center a string in a rect (the menu rows' alignment).
    pub fn text_centered(
        &mut self,
        atlas: &FontAtlas,
        cx: f32,
        cy: f32,
        s: &str,
        px: f32,
        color: [f32; 4],
    ) {
        let m = atlas.measure(s, px);
        self.text(atlas, cx - m[0] / 2.0, cy - m[1] / 2.0, s, px, color);
    }
}

/// The GPU half of the UI (feature `render-wgpu`): pipeline + atlas
/// texture + per-frame vertex upload.
#[cfg(feature = "render-wgpu")]
pub mod gpu {
    use super::{UiDrawList, UiVertex, MENU_FONT_BYTES};
    use crate::FontAtlas;

    /// The UI shader — ortho pixel-space quads, one atlas texture,
    /// vertex color multiply, alpha blend.  (Inline WGSL: this is OUR
    /// shader, not a translated game shader.)
    const UI_WGSL: &str = r#"
struct UiUniforms {
    screen: vec2<f32>,
    _pad: vec2<f32>,
};
@group(0) @binding(0) var<uniform> u: UiUniforms;
@group(1) @binding(0) var s: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs(@location(0) p: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>) -> VsOut {
    var o: VsOut;
    let ndc = vec2<f32>(
        p.x / u.screen.x * 2.0 - 1.0,
        1.0 - p.y / u.screen.y * 2.0,
    );
    o.pos = vec4<f32>(ndc, 0.0, 1.0);
    o.uv = uv;
    o.color = color;
    return o;
}

@fragment
fn fs(i: VsOut) -> @location(0) vec4<f32> {
    return textureSample(s, samp, i.uv) * i.color;
}
"#;

    /// The uploaded UI state (rebuilt per frame by the app).
    pub struct UiPass {
        pub pipeline: wgpu::RenderPipeline,
        pub atlas_bind: wgpu::BindGroup,
        pub uniforms: wgpu::Buffer,
        pub uniform_bind: wgpu::BindGroup,
        pub verts: wgpu::Buffer,
        pub capacity: u64,
    }

    impl UiPass {
        /// Build the UI pass from the device + queue + surface format +
        /// the CPU atlas (upload once).
        pub fn new(
            device: &wgpu::Device,
            queue: &wgpu::Queue,
            format: wgpu::TextureFormat,
            screen: [f32; 2],
        ) -> (Self, FontAtlas) {
            let atlas = FontAtlas::new();
            let size = wgpu::Extent3d {
                width: atlas.canvas_size,
                height: atlas.canvas_size,
                depth_or_array_layers: 1,
            };
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("menu font atlas"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &atlas.canvas,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(atlas.canvas_size * 4),
                    rows_per_image: None,
                },
                size,
            );
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("menu font sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });
            let bgl0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ui uniform bgl"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
            let bgl1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ui atlas bgl"),
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
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("ui.wgsl"),
                source: wgpu::ShaderSource::Wgsl(UI_WGSL.into()),
            });
            let vbl = wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<UiVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x2,
                        offset: 0,
                        shader_location: 0,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x2,
                        offset: 8,
                        shader_location: 1,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 16,
                        shader_location: 2,
                    },
                ],
            };
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("ui pl"),
                bind_group_layouts: &[&bgl0, &bgl1],
                push_constant_ranges: &[],
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("ui pipeline"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[vbl],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
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
            let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ui uniforms"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let uniform_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ui uniform bind"),
                layout: &bgl0,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                }],
            });
            let atlas_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ui atlas bind"),
                layout: &bgl1,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            });
            let capacity = 256u64 * std::mem::size_of::<UiVertex>() as u64;
            let verts = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ui verts"),
                size: capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let pass = Self {
                pipeline,
                atlas_bind,
                uniforms,
                uniform_bind,
                verts,
                capacity,
            };
            pass.set_screen(queue, screen);
            (pass, atlas)
        }

        /// Update the ortho basis on resize.
        pub fn set_screen(&self, queue: &wgpu::Queue, screen: [f32; 2]) {
            let data = [screen[0], screen[1], 0.0f32, 0.0f32];
            queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&data));
        }

        /// Upload this frame's quads (grow the buffer if needed).
        pub fn upload(
            &mut self,
            device: &wgpu::Device,
            queue: &wgpu::Queue,
            list: &UiDrawList,
        ) {
            let need = (list.verts.len() * std::mem::size_of::<UiVertex>()) as u64;
            if need > self.capacity {
                self.capacity = (need * 2).max(1 << 20);
                self.verts = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("ui verts (grown)"),
                    size: self.capacity,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            }
            if !list.verts.is_empty() {
                let bytes: &[u8] = bytemuck::cast_slice(&list.verts);
                queue.write_buffer(&self.verts, 0, bytes);
            }
        }

        /// The second render pass (alpha-blended over the world pass).
        pub fn draw<'r>(&'r self, pass: &mut wgpu::RenderPass<'r>, list: &UiDrawList) {
            if list.verts.is_empty() {
                return;
            }
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            pass.set_vertex_buffer(0, self.verts.slice(..));
            pass.draw(0..list.verts.len() as u32, 0..1);
        }

        /// The bundled font bytes (for callers that need the metrics
        /// without the GPU half).
        pub fn font_bytes() -> &'static [u8] {
            MENU_FONT_BYTES
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Block 107: the bundled font builds an atlas covering the UI
    /// charset, and every glyph lands inside the canvas with sane
    /// metrics.
    #[test]
    fn menu_font_atlas_covers_the_charset() {
        let atlas = FontAtlas::new();
        for c in charset() {
            let g = atlas
                .glyphs
                .get(&c)
                .unwrap_or_else(|| panic!("glyph missing: {c:?}"));
            // Blank glyphs (space) carry a real advance with a
            // zero-size rect — that's correct, not a hole.
            assert!(g.advance > 0.0 || (g.rect[2] > 0 && g.rect[3] > 0));
            assert!(g.rect[0] + g.rect[2] <= atlas.canvas_size);
            assert!(g.rect[1] + g.rect[3] <= atlas.canvas_size);
        }
        // Sanity: known letters rasterize with nonzero advance.
        let m = atlas.measure("PROTOTYPE", 36.0);
        assert!(m[0] > 100.0, "PROTOTYPE measures {}", m[0]);
        assert!(m[1] > 20.0);
    }

    /// Text layout produces correct, in-canvas quads (3 verts per
    /// glyph, y-down screen space).
    #[test]
    fn text_layout_builds_quads() {
        let atlas = FontAtlas::new();
        let mut list = UiDrawList {
            screen: [1280.0, 720.0],
            ..Default::default()
        };
        list.text(&atlas, 100.0, 100.0, "PROTOTYPE", 36.0, [1.0; 4]);
        let n = "PROTOTYPE".chars().count();
        assert_eq!(list.verts.len(), n * 6);
        for v in &list.verts {
            assert!(v.pos[0] >= 0.0 && v.pos[0] <= 1280.0);
            assert!(v.pos[1] >= 0.0 && v.pos[1] <= 720.0);
        }
        // Solid rects work too (6 verts).
        list.rect(10.0, 10.0, 50.0, 20.0, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(list.verts.len(), n * 6 + 6);
    }
}

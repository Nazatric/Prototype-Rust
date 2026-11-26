//! render_gfxtext_cpu — the device-free CPU mirror of the FRONTEND
//! TEXT pipeline, driven by the REAL shipped `gfxtextshader` pair
//! (block 104).
//!
//! ## Why this exists (block 104)
//!
//! Block 101 wired the `cxformtexture` / `cxformsolidcolour` frontend
//! pairs into a CPU sprite pipeline (UI panels).  The remaining
//! frontend pair — **`gfxtextshader`**, the pair the engine uses for
//! every text string in the menus and HUD — was parsed (block 99)
//! and its HLSL source extracted (block 86) but never consumed by a
//! renderer.  This module closes that gap: the register contract is
//! implemented 1:1 from the real program bytes and the real Radical
//! HLSL source, rasterized on the CPU with alpha blending, and the
//! evidence renders REAL game text strings (the mission-name table
//! from `scripts.rcf` member[0] and the free-roam-event schema names
//! from `frevents.lua`, both committed fixtures) through it.
//!
//! ## The REAL register contract (proven from two committed sources)
//!
//! Pinned by [`tests::gfxtext_contract_matches_shipped_sm30_pair`]
//! against `frontendshaders.p3d` (the SM3.0 token streams, parsed
//! with [`crate::shader::sm30::Sm30Shader`]) and by
//! [`tests::gfxtext_contract_matches_committed_hlsl_fixtures`] against
//! the Radical HLSL sources extracted in block 86:
//!
//! * `gfxtextshader - lit - Vertex Program` (12 instructions, `vs_3_0`):
//!   - `c0..c3` = `p3dWorldViewProjectionMatrix` (4×4)
//!   - `c4` = `p3dLight_ObjectSpaceLightBounds`, `c5` = `p3dNaN`
//!     (both consumed ONLY by the `RAD_XENON` cull branch — the PC
//!     path never reads them; mirrored as unused)
//!   - inputs: `v0` = `p3dVertex_Position`, `v1` = `p3dVertex_Texcoord0`,
//!     `v2` = `p3dVertex_Colour0`
//!   - out position = `mul(WVP, position)` with `position.w = 1.0`
//!     (the literal `GenerateWVPPosition0` call + the
//!     `#ifndef RAD_OGL` w-force); the same row-major `M · v`
//!     convention the `default`/`legacy` VS chains compile to.
//!   - out `TexCoord_0` = uv PASSTHROUGH, out `Colour_0` = colour
//!     PASSTHROUGH (the text-shader feature: the tint travels on the
//!     vertex colour, no lighting math at all).
//! * `gfxtextshader - lit - Fragment Program` (8 instructions, `ps_3_0`):
//!   - `s0` = `diffuseMap` (the glyph mask texture)
//!   - `c0` = `p3dLight_ObjectSpaceLightBounds`, `c1` = `p3dNaN`
//!     (unused on the PC path), `c2` = `cxmul`, `c3` = `cxadd`,
//!     `c4` = `p3dBuiltIn_Zero`, `c5` = `p3dBuiltIn_One`
//!   - the literal chain (real HLSL helper names):
//!     1. `out931 = tex2D(diffuseMap, uv * 1 + 0)`
//!     2. `out943 = Multiply0(vertexColour.w, out931.w)` — the
//!        ALPHA is `vc.a × tex.a` (the glyph mask multiplies the
//!        text fade)
//!     3. `out933 = Float40(vertexColour.rgb, out943)` — the RGB is
//!        the VERTEX colour (the texture RGB is discarded — that is
//!        the text-shader feature that makes one white mask tintable
//!        to any colour)
//!     4. `out936 = MultiplyAdd2(cxmul, out933, cxadd)` — the same
//!        Flash-style colour transform the cxform pairs use
//!     5. `out940 = Pure3D_Colour00(out936.xyz, out936.w)`
//!
//! ## What is a documented approximation
//!
//! * The glyph atlas is PROCEDURAL (a 5×7 bitmap font baked into an
//!   RGBA alpha mask): no real font/glyph texture member has been
//!   fetched from `art.rcf` yet (its directory is hash-keyed without
//!   a member-name census; the fetch is the separate NEXT-UP item).
//!   The shader MATH, register contract and text STRINGS are 100%
//!   real; the glyph SHAPES are declared DESIGN.
//! * Alpha blending (SrcAlpha / InvSrcAlpha over the framebuffer) is
//!   the engine-side BLEND STATE, which lives outside the shader
//!   program bytes; the standard D3D default is mirrored (text
//!   output alpha is `vc.a × tex.a`, so blending is required for
//!   anti-aliased coverage — a no-blend rasterizer would render
//!   solid rectangles).
//! * Single-threaded scanline rasterizer, barycentric interpolation,
//!   CLAMP + BILINEAR sampling (same conventions as the block-101
//!   frontend mirror).
//! * The screen layout (positions/sizes of the strings) is DESIGN —
//!   no real frontend LAYOUT file has been decoded yet (the block-101
//!   deferral stands); the TEXT CONTENT is real game data.

#![forbid(unsafe_code)]

use crate::render_art_world_cpu::FrameStats;
use crate::render_frontend_cpu::{render_sprites_cpu, FrontendSprite};

/// One `gfxtextshader` draw call — the EXACT constant payload the
/// real shader pair consumes (register-for-register).
#[derive(Debug, Clone, PartialEq)]
pub struct GfxTextQuad {
    /// Vertex shader `c0..c3` — `p3dWorldViewProjectionMatrix`
    /// (row-major rows; clip[r] = pos · row[r], the convention the
    /// compiled `GenerateWVPPosition0` chain uses).
    pub wvp_rows: [[f32; 4]; 4],
    /// Fragment shader `c2` — `cxmul` (per-channel multiply).
    pub cxmul: [f32; 4],
    /// Fragment shader `c3` — `cxadd` (per-channel add).
    pub cxadd: [f32; 4],
    /// Vertex shader `v2` — `p3dVertex_Colour0` per corner (corner
    /// order = `quad`).
    pub colour: [[f32; 4]; 4],
    /// Vertex shader `v1` — `p3dVertex_Texcoord0` per corner.
    pub uv: [[f32; 2]; 4],
    /// Clip-space quad corners: `[bl, br, tr, tl]` (x-right, y-up).
    pub quad: [[f32; 2]; 4],
}

impl GfxTextQuad {
    /// A text quad from a clip-space rect + a uv rect (corners bl,
    /// br, tr, tl) with one flat vertex colour.
    pub fn from_rect(
        rect: [[f32; 2]; 4],
        uv: [[f32; 2]; 4],
        colour: [f32; 4],
    ) -> Self {
        Self {
            wvp_rows: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            cxmul: [1.0, 1.0, 1.0, 1.0],
            cxadd: [0.0, 0.0, 0.0, 0.0],
            colour: [colour; 4],
            uv,
            quad: rect,
        }
    }
}

/// The literal port of the real VS entry point (`EntryPoint_vs`):
/// returns `(position, texcoord, colour)` — position is the WVP chain
/// with the w forced to 1.0 first, texcoord and colour pass through
/// untouched.
pub fn gfxtext_vertex_stage(
    position: [f32; 2],
    wvp_rows: &[[f32; 4]; 4],
    uv: [f32; 2],
    colour: [f32; 4],
) -> ([f32; 4], [f32; 2], [f32; 4]) {
    // out0 = position; out0.w = 1.0 (the literal #ifndef RAD_OGL line).
    let p = [position[0], position[1], 0.0, 1.0];
    // out237 = GenerateWVPPosition0(out0, WVP) — the row-major
    // 4-row dot chain the compiled stream uses.
    let mut clip = [0.0f32; 4];
    for r in 0..4 {
        clip[r] = p[0] * wvp_rows[r][0]
            + p[1] * wvp_rows[r][1]
            + p[2] * wvp_rows[r][2]
            + wvp_rows[r][3];
    }
    // out2 / out30 pass straight through (mov o1.xy, v1 / mov oC0, v2).
    (clip, uv, colour)
}

/// The literal port of the real FP entry point (`EntryPoint_ps`):
/// texel in, final colour out, f32 side — the
/// `Multiply0 → Float40 → MultiplyAdd2 → Pure3D_Colour00` chain.
pub fn gfxtext_fragment_stage(
    vertex_colour: [f32; 4],
    texel: [f32; 4],
    cxmul: [f32; 4],
    cxadd: [f32; 4],
) -> [f32; 4] {
    // out943 = Multiply0(out31.w, out931.w) — alpha = vc.a × tex.a.
    let a = vertex_colour[3] * texel[3];
    // out933 = Float40(out31.xyz, out943) — rgb = vc.rgb (tex.rgb
    // discarded; the text-shader tint feature).
    let c = [vertex_colour[0], vertex_colour[1], vertex_colour[2], a];
    // out936 = MultiplyAdd2(cxmul, out933, cxadd).
    let mut out = [0.0f32; 4];
    for k in 0..4 {
        out[k] = cxmul[k] * c[k] + cxadd[k];
    }
    // out940 = Pure3D_Colour00(out936.xyz, out936.w) — pass-through.
    out
}

fn f32_to_u8(v: f32) -> u8 {
    let x = (v * 255.0).round();
    if x < 0.0 {
        0
    } else if x > 255.0 {
        255
    } else {
        x as u8
    }
}

/// One rendered text frame plus its non-blank statistics.
#[derive(Debug, Clone, PartialEq)]
pub struct GfxTextFrame {
    /// Texel width.
    pub width: u32,
    /// Texel height.
    pub height: u32,
    /// Packed RGBA8, row-major, opaque (alpha 255), `width*height*4` B.
    pub rgba: Vec<u8>,
    /// Non-blank evidence statistics (computed over the COMPOSITED
    /// buffer: base + text).
    pub stats: FrameStats,
}

/// Pack the f32 framebuffer to RGBA8 + compute the non-blank stats.
fn finish_frame(
    fb: Vec<[f32; 4]>,
    width: u32,
    height: u32,
    drawn: usize,
    clipped: usize,
) -> Result<GfxTextFrame, String> {
    let mut rgba = Vec::with_capacity(fb.len() * 4);
    let mut distinct = std::collections::BTreeSet::new();
    let mut min = [255u8; 4];
    let mut max = [0u8; 4];
    for px in &fb {
        let b = [
            f32_to_u8(px[0]),
            f32_to_u8(px[1]),
            f32_to_u8(px[2]),
            f32_to_u8(px[3]),
        ];
        for k in 0..4 {
            min[k] = min[k].min(b[k]);
            max[k] = max[k].max(b[k]);
        }
        distinct.insert(u32::from_le_bytes(b));
        rgba.extend_from_slice(&b);
    }
    Ok(GfxTextFrame {
        width,
        height,
        stats: FrameStats {
            distinct: distinct.len(),
            min,
            max,
            triangles_drawn: drawn,
            triangles_clipped: clipped,
        },
        rgba,
    })
}

/// CLAMP + BILINEAR sampling, top-left origin — the same conventions
/// as the block-101 frontend mirror (screen-space UI, textures stored
/// top-down).
pub fn sample_clamp_bilinear(
    tex: &[u8],
    tw: u32,
    th: u32,
    u: f32,
    v: f32,
) -> [f32; 4] {
    if tw == 0 || th == 0 {
        return [0.0; 4];
    }
    let (twf, thf) = (tw as f32, th as f32);
    // D3D texel-centre convention: u = (i+0.5)/tw samples texel i
    // exactly; the glyph uv rects are inset half a texel to match.
    let x = u.clamp(0.0, 1.0) * twf - 0.5;
    let y = v.clamp(0.0, 1.0) * thf - 0.5;
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(tw as usize - 1);
    let y1 = (y0 + 1).min(th as usize - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let px = |xx: usize, yy: usize| -> [f32; 4] {
        let o = (yy * tw as usize + xx) * 4;
        [
            tex[o] as f32 / 255.0,
            tex[o + 1] as f32 / 255.0,
            tex[o + 2] as f32 / 255.0,
            tex[o + 3] as f32 / 255.0,
        ]
    };
    let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    let mut out = [0.0f32; 4];
    for k in 0..4 {
        let top = a[k] * (1.0 - fx) + b[k] * fx;
        let bot = c[k] * (1.0 - fx) + d[k] * fx;
        out[k] = top * (1.0 - fy) + bot * fy;
    }
    out
}

// ---------------------------------------------------------------------------
// The procedural glyph atlas (DECLARED DESIGN — see module docs)
// ---------------------------------------------------------------------------

/// One glyph's position in the atlas (texels, top-left origin).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphSlot {
    pub x0: u32,
    pub y0: u32,
    pub w: u32,
    pub h: u32,
}

/// A procedural 5×7 bitmap font baked into an RGBA alpha-mask atlas.
/// The shader consumes ONLY `tex.a` (the real FP discards tex.rgb),
/// so the mask is what defines the glyphs; RGB is baked white.
#[derive(Debug, Clone)]
pub struct GlyphAtlas {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Cell width in texels (5 glyph + 1 pad).
    pub cell_w: u32,
    /// Cell height in texels (7 glyph + 1 pad).
    pub cell_h: u32,
    slots: Vec<(char, GlyphSlot)>,
}

impl GlyphAtlas {
    /// UV rect for a glyph, corners ordered `[bl, br, tr, tl]` to
    /// match [`GfxTextQuad::quad`] (v grows DOWNWARD — the top-left
    /// texture origin convention).  The rect is INSET by half a texel
    /// so the D3D texel-centre sampling (`x = u·tw − 0.5`) lands on
    /// the glyph body texels exactly (cell-edge texels straddle the
    /// pad and soften, like a real GPU).
    pub fn glyph_uv(&self, ch: char) -> Option<[[f32; 2]; 4]> {
        let s = self.slot(ch)?;
        let w = self.width as f32;
        let h = self.height as f32;
        let u0 = (s.x0 as f32 + 0.5) / w;
        let u1 = (s.x0 + s.w) as f32 - 0.5;
        let u1 = u1 / w;
        let v_top = (s.y0 as f32 + 0.5) / h;
        let v_bot = ((s.y0 + s.h) as f32 - 0.5) / h;
        Some([
            [u0, v_bot],
            [u1, v_bot],
            [u1, v_top],
            [u0, v_top],
        ])
    }

    pub fn slot(&self, ch: char) -> Option<&GlyphSlot> {
        self.slots.iter().find(|(c, _)| *c == ch).map(|(_, s)| s)
    }

    /// Number of glyphs baked into the atlas.
    pub fn glyph_count(&self) -> usize {
        self.slots.len()
    }
}

/// The 5×7 glyph bitmaps (7 rows × 5 bits, MSB = left column).
const GLYPHS: &[(char, [u8; 7])] = &[
    ('A', [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11]),
    ('B', [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E]),
    ('C', [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E]),
    ('D', [0x1C, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1C]),
    ('E', [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F]),
    ('F', [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10]),
    ('G', [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F]),
    ('H', [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11]),
    ('I', [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E]),
    ('J', [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C]),
    ('K', [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11]),
    ('L', [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F]),
    ('M', [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11]),
    ('N', [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11]),
    ('O', [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E]),
    ('P', [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10]),
    ('Q', [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D]),
    ('R', [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11]),
    ('S', [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E]),
    ('T', [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04]),
    ('U', [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E]),
    ('V', [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04]),
    ('W', [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11]),
    ('X', [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11]),
    ('Y', [0x11, 0x11, 0x11, 0x0A, 0x04, 0x04, 0x04]),
    ('Z', [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F]),
    ('a', [0x00, 0x0E, 0x01, 0x0F, 0x11, 0x11, 0x0F]),
    ('b', [0x10, 0x10, 0x1E, 0x11, 0x11, 0x11, 0x1E]),
    ('c', [0x00, 0x0E, 0x11, 0x10, 0x10, 0x11, 0x0E]),
    ('d', [0x01, 0x01, 0x0F, 0x11, 0x11, 0x11, 0x0F]),
    ('e', [0x00, 0x0E, 0x11, 0x1F, 0x10, 0x11, 0x0E]),
    ('f', [0x06, 0x08, 0x1E, 0x08, 0x08, 0x08, 0x08]),
    ('g', [0x0F, 0x11, 0x11, 0x0F, 0x01, 0x11, 0x0E]),
    ('h', [0x10, 0x10, 0x1E, 0x11, 0x11, 0x11, 0x11]),
    ('i', [0x04, 0x00, 0x0C, 0x04, 0x04, 0x04, 0x0E]),
    ('j', [0x02, 0x00, 0x06, 0x02, 0x02, 0x12, 0x0C]),
    ('k', [0x10, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11]),
    ('l', [0x0C, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E]),
    ('m', [0x00, 0x1A, 0x15, 0x15, 0x15, 0x15, 0x15]),
    ('n', [0x00, 0x1E, 0x11, 0x11, 0x11, 0x11, 0x11]),
    ('o', [0x00, 0x0E, 0x11, 0x11, 0x11, 0x11, 0x0E]),
    ('p', [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10]),
    ('q', [0x0F, 0x11, 0x11, 0x0F, 0x01, 0x01, 0x01]),
    ('r', [0x00, 0x1E, 0x11, 0x10, 0x10, 0x10, 0x10]),
    ('s', [0x00, 0x0F, 0x10, 0x0E, 0x01, 0x11, 0x0E]),
    ('t', [0x08, 0x08, 0x1E, 0x08, 0x08, 0x08, 0x06]),
    ('u', [0x00, 0x11, 0x11, 0x11, 0x11, 0x13, 0x0D]),
    ('v', [0x00, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04]),
    ('w', [0x00, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A]),
    ('x', [0x00, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x00]),
    ('y', [0x11, 0x11, 0x11, 0x0F, 0x01, 0x11, 0x0E]),
    ('z', [0x00, 0x1F, 0x02, 0x04, 0x08, 0x10, 0x1F]),
    ('0', [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E]),
    ('1', [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E]),
    ('2', [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F]),
    ('3', [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E]),
    ('4', [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02]),
    ('5', [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E]),
    ('6', [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E]),
    ('7', [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08]),
    ('8', [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E]),
    ('9', [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C]),
    ('_', [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F]),
    ('-', [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00]),
    ('/', [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10]),
    ('.', [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C]),
    (' ', [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
];

/// Bake the glyph atlas: `cols` × enough rows for every glyph, each
/// cell [`GlyphAtlas::cell_w`] × [`GlyphAtlas::cell_h`] texels
/// (5×7 glyph + 1-texel pad).  RGB = white, alpha = coverage.
pub fn build_glyph_atlas(cols: u32) -> GlyphAtlas {
    let cell_w = 6;
    let cell_h = 8;
    let n = GLYPHS.len() as u32;
    let rows = n.div_ceil(cols);
    let w = cols * cell_w;
    let h = rows * cell_h;
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let mut slots = Vec::with_capacity(GLYPHS.len());
    for (i, (ch, bits)) in GLYPHS.iter().enumerate() {
        let gx = (i as u32 % cols) * cell_w;
        let gy = (i as u32 / cols) * cell_h;
        for row in 0..7usize {
            for col in 0..5usize {
                let on = bits[row] & (0x10 >> col) != 0;
                let a = if on { 255u8 } else { 0u8 };
                let o = (((gy + row as u32) * w + gx + col as u32) * 4) as usize;
                rgba[o] = 255;
                rgba[o + 1] = 255;
                rgba[o + 2] = 255;
                rgba[o + 3] = a;
            }
        }
        slots.push((
            *ch,
            GlyphSlot {
                x0: gx,
                y0: gy,
                w: 5,
                h: 7,
            },
        ));
    }
    GlyphAtlas {
        rgba,
        width: w,
        height: h,
        cell_w,
        cell_h,
        slots,
    }
}

/// Lay out one string as per-glyph quads (one draw call per glyph —
/// the real frontend queues one text-shader draw per glyph cell).
///
/// * `x`, `y` — clip-space of the FIRST glyph's top-left (y-up, so
///   `y` is the TOP because the layout builds downward with negative
///   steps).
/// * `gw`, `gh` — per-glyph size in clip units.
/// * `gap` — per-glyph advance pad in clip units.
pub fn layout_text(
    atlas: &GlyphAtlas,
    text: &str,
    x: f32,
    y: f32,
    gw: f32,
    gh: f32,
    gap: f32,
    colour: [f32; 4],
) -> Vec<GfxTextQuad> {
    let mut out = Vec::new();
    let mut cx = x;
    for ch in text.chars() {
        if let Some(uv) = atlas.glyph_uv(ch) {
            // Corners [bl, br, tr, tl]; the glyph TOP is at clip y,
            // the bottom at y − gh (y-up clip space).
            let rect = [
                [cx, y - gh],
                [cx + gw, y - gh],
                [cx + gw, y],
                [cx, y],
            ];
            out.push(GfxTextQuad::from_rect(rect, uv, colour));
        }
        cx += gw + gap;
    }
    out
}

/// Render the `gfxtextshader` draw list over a base buffer with the
/// standard alpha blend (`fb = src.rgb·src.a + dst.rgb·(1−src.a)` —
/// the engine-side blend state, mirrored; see module docs).
///
/// * `quads` — the text draw list (each = one real shader-pair
///   constant set + clip-space glyph quad).
/// * `atlas` — the glyph mask texture (procedural; see below).
/// * `base` — the base RGBA8 buffer (`width*height*4` B) to blend
///   OVER (typically the block-101 frontend background pass);
///   `None` clears to transparent black first.
pub fn render_gfxtext_cpu(
    quads: &[GfxTextQuad],
    atlas: &GlyphAtlas,
    base: Option<&[u8]>,
    width: u32,
    height: u32,
) -> Result<GfxTextFrame, String> {
    if width == 0 || height == 0 {
        return Err("empty render target".into());
    }
    let n = (width * height) as usize;
    let mut fb: Vec<[f32; 4]> = match base {
        Some(b) => {
            if b.len() != n * 4 {
                return Err(format!(
                    "base buffer {} B != expected {} B",
                    b.len(),
                    n * 4
                ));
            }
            b.chunks_exact(4)
                .map(|px| {
                    [
                        px[0] as f32 / 255.0,
                        px[1] as f32 / 255.0,
                        px[2] as f32 / 255.0,
                        px[3] as f32 / 255.0,
                    ]
                })
                .collect()
        }
        None => vec![[0.0; 4]; n],
    };
    let mut drawn = 0usize;
    let mut clipped = 0usize;

    for quad in quads {
        for tri in [[0usize, 1, 2], [0, 2, 3]] {
            let corners = quad.quad;
            let (p0, p1, p2) = (corners[tri[0]], corners[tri[1]], corners[tri[2]]);
            let sx = |c: [f32; 2]| -> (f32, f32) {
                (
                    (c[0] * 0.5 + 0.5) * (width as f32 - 1.0),
                    (1.0 - (c[1] * 0.5 + 0.5)) * (height as f32 - 1.0),
                )
            };
            let (a, b, c) = (sx(p0), sx(p1), sx(p2));
            let minx = a.0.min(b.0).min(c.0).floor().max(0.0) as i64;
            let maxx = a.0.max(b.0).max(c.0).ceil().min(width as f32 - 1.0) as i64;
            let miny = a.1.min(b.1).min(c.1).floor().max(0.0) as i64;
            let maxy = a.1.max(b.1).max(c.1).ceil().min(height as f32 - 1.0) as i64;
            if minx > maxx || miny > maxy {
                clipped += 1;
                continue;
            }
            let area = (b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1);
            if area.abs() < 1e-9 {
                clipped += 1;
                continue;
            }
            let inv = 1.0 / area;
            // Corner uv/colour for this triangle (corner 0 belongs to
            // both triangles).
            let (uv_a, uv_b, uv_c) = match tri {
                [0, 1, 2] => (quad.uv[0], quad.uv[1], quad.uv[2]),
                _ => (quad.uv[0], quad.uv[2], quad.uv[3]),
            };
            let (col_a, col_b, col_c) = match tri {
                [0, 1, 2] => (quad.colour[0], quad.colour[1], quad.colour[2]),
                _ => (quad.colour[0], quad.colour[2], quad.colour[3]),
            };
            // Barycentrics paired to the RIGHT corners (the same
            // edge-function convention as the proven art-world
            // rasterizer): wa = weight of a, wb = weight of b,
            // wc = weight of c.  (The block-101 frontend mirror had
            // these cyclically SHIFTED against the uv/colour corners
            // — a diagonal shear — fixed here in block 104.)
            //
            // TOP-LEFT FILL RULE: with blending, a pixel on the shared
            // bl→tr diagonal would otherwise be drawn by BOTH
            // triangles and double-blended (the exact artifact the
            // composite test caught).  Raw edge functions are sign-
            // normalised to area > 0; an edge-zero pixel is included
            // only when its edge is the "positive-direction" one
            // (downward, or rightward when horizontal) — each shared
            // edge is then claimed by exactly one triangle.
            let s = area.signum();
            let e_pos = |p0: (f32, f32), p1: (f32, f32)| -> bool {
                p1.1 > p0.1 || (p1.1 == p0.1 && p1.0 > p0.0)
            };
            for y in miny..=maxy {
                for x in minx..=maxx {
                    let px = x as f32 + 0.5;
                    let py = y as f32 + 0.5;
                    let ea = s * ((c.0 - b.0) * (py - b.1) - (c.1 - b.1) * (px - b.0));
                    let eb = s * ((a.0 - c.0) * (py - c.1) - (a.1 - c.1) * (px - c.0));
                    let ec = s * ((b.0 - a.0) * (py - a.1) - (b.1 - a.1) * (px - a.0));
                    let in_a = ea > 0.0 || (ea == 0.0 && e_pos(b, c));
                    let in_b = eb > 0.0 || (eb == 0.0 && e_pos(c, a));
                    let in_c = ec > 0.0 || (ec == 0.0 && e_pos(a, b));
                    if !(in_a && in_b && in_c) {
                        continue;
                    }
                    let wa = ea * inv.abs();
                    let wb = eb * inv.abs();
                    let wc = ec * inv.abs();
                    let u = uv_a[0] * wa + uv_b[0] * wb + uv_c[0] * wc;
                    let v = uv_a[1] * wa + uv_b[1] * wb + uv_c[1] * wc;
                    let tex =
                        sample_clamp_bilinear(&atlas.rgba, atlas.width, atlas.height, u, v);
                    let mut vc = [0.0f32; 4];
                    for k in 0..4 {
                        vc[k] = col_a[k] * wa + col_b[k] * wb + col_c[k] * wc;
                    }
                    // The REAL fragment stage (the literal port).
                    let src = gfxtext_fragment_stage(vc, tex, quad.cxmul, quad.cxadd);
                    let o = (y as u32 * width + x as u32) as usize;
                    let dst = fb[o];
                    let sa = src[3].clamp(0.0, 1.0);
                    let mut out = [0.0f32; 4];
                    for k in 0..3 {
                        out[k] = src[k] * sa + dst[k] * (1.0 - sa);
                    }
                    out[3] = sa + dst[3] * (1.0 - sa);
                    fb[o] = out;
                }
            }
            drawn += 1;
        }
    }
    finish_frame(fb, width, height, drawn, clipped)
}

// ---------------------------------------------------------------------------
// The evidence scene (block 104) — REAL game strings through the REAL
// text shader; the glyph shapes and the screen layout are DESIGN.
// ---------------------------------------------------------------------------

/// The block-104 HUD text scene.  Every string is REAL game data:
/// `TITLE` + [`Self::LIST`] come from the `AddValidMission` mission
/// name table (scripts.rcf member[0], committed census
/// `lua_script_strings.txt`), [`Self::HINT`] names two free-roam
/// event schema keys from `frevents.lua` (scripts.rcf member[2],
/// byte-identical to the committed `frevents.lua` fixture).
pub struct TextHudScene;

impl TextHudScene {
    /// The level name row (REAL string: the first AddValidMission
    /// argument in scripts.rcf member[0]).
    pub const TITLE: &'static str = "manhattan";
    /// The mission list (REAL strings from the same table; note the
    /// REAL engine spelling `Millitary_Test` — the game's own typo).
    pub const LIST: [&'static str; 6] = [
        "Infected_Test",
        "Millitary_Test",
        "NIS_Test",
        "c18m01",
        "e01m00",
        "e02m02",
    ];
    /// The hint row (REAL schema keys from frevents.lua).
    pub const HINT: &'static str = "collectiblesDef maxHintCollectibles";

    /// The background pass — built from the block-101 frontend
    /// solid-colour primitives (the REAL `cxformsolidcolour` pair
    /// handles these): header strip, list panel, selection bar
    /// (animated), bottom strip.
    pub fn background(t: f32) -> Vec<FrontendSprite> {
        let mut v = Vec::new();
        // Backdrop fill.
        let mut s = FrontendSprite::fullscreen();
        s.textured = false;
        s.fill_colour = [0.04, 0.05, 0.07, 1.0];
        v.push(s);
        // Header strip.
        let mut s = FrontendSprite::fullscreen();
        s.textured = false;
        s.fill_colour = [0.10, 0.12, 0.16, 1.0];
        s.quad = [[-1.0, 0.55], [1.0, 0.55], [1.0, 0.80], [-1.0, 0.80]];
        v.push(s);
        // List panel.
        let mut s = FrontendSprite::fullscreen();
        s.textured = false;
        s.fill_colour = [0.07, 0.08, 0.11, 1.0];
        s.quad = [[-0.65, -0.35], [0.80, -0.35], [0.80, 0.45], [-0.65, 0.45]];
        v.push(s);
        // Selection bar sweeping the list (the frontend highlight).
        let sel = Self::selected_row(t);
        let y = Self::row_y(sel);
        let mut s = FrontendSprite::fullscreen();
        s.textured = false;
        s.fill_colour = [0.22, 0.16, 0.05, 1.0];
        s.quad = [[-0.62, y - 0.06], [0.77, y - 0.06], [0.77, y - 0.20], [-0.62, y - 0.20]];
        v.push(s);
        // Bottom strip.
        let mut s = FrontendSprite::fullscreen();
        s.textured = false;
        s.fill_colour = [0.10, 0.12, 0.16, 1.0];
        s.quad = [[-1.0, -0.95], [1.0, -0.95], [1.0, -0.80], [-1.0, -0.80]];
        v.push(s);
        v
    }

    /// The selected list row for progress `t` in [0,1) — cycles
    /// through the three combat-test rows.
    pub fn selected_row(t: f32) -> usize {
        ((t * 3.0) as usize) % 3
    }

    /// Clip-space y of list row `k`'s TOP.
    pub fn row_y(k: usize) -> f32 {
        0.30 - 0.145 * k as f32
    }

    /// The text draw list for one frame — every glyph quad goes
    /// through the REAL `gfxtextshader` contract.
    pub fn text_quads(t: f32, atlas: &GlyphAtlas) -> Vec<GfxTextQuad> {
        let mut v = Vec::new();
        // Title: fades in over the first 20% then holds.
        let fade = (t / 0.20).clamp(0.0, 1.0);
        v.extend(layout_text(
            atlas,
            Self::TITLE,
            -0.92,
            0.745,
            0.072,
            0.1008,
            0.008,
            [0.92, 0.94, 0.97, 0.35 + 0.65 * fade],
        ));
        // List rows: the selected one pulses warm, the rest cool.
        let sel = Self::selected_row(t);
        let pulse = 0.75 + 0.25 * ((t * 12.0).sin() * 0.5 + 0.5);
        for (k, s) in Self::LIST.iter().enumerate() {
            let colour = if k == sel {
                [1.0 * pulse, 0.80 * pulse, 0.28, 1.0]
            } else {
                [0.60, 0.64, 0.70, 0.92]
            };
            v.extend(layout_text(
                atlas,
                s,
                -0.55,
                Self::row_y(k),
                0.062,
                0.0868,
                0.007,
                colour,
            ));
        }
        // Hint row: steady dim.
        v.extend(layout_text(
            atlas,
            Self::HINT,
            -0.92,
            -0.865,
            0.048,
            0.0672,
            0.006,
            [0.46, 0.50, 0.56, 0.9],
        ));
        v
    }

    /// One composited frame: the block-101 background pass + the
    /// block-104 text pass blended over it.
    pub fn render_frame(
        t: f32,
        atlas: &GlyphAtlas,
        width: u32,
        height: u32,
    ) -> Result<GfxTextFrame, String> {
        let bg = render_sprites_cpu(
            &Self::background(t),
            None,
            width,
            height,
            [0.04, 0.05, 0.07, 1.0],
        )?;
        render_gfxtext_cpu(&Self::text_quads(t, atlas), atlas, Some(&bg.rgba), width, height)
    }
}

// ---------------------------------------------------------------------------
// Tests
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

    /// The real shipped `gfxtextshader` pair from frontendshaders.p3d.
    fn shipped_pair(sub: &str) -> Vec<(String, bool, Vec<u8>)> {
        let data = fixture("frontendshaders.p3d");
        let f = P3dFile::parse(&data).unwrap();
        f.shader_programs()
            .unwrap()
            .into_iter()
            .filter(|p| p.name.contains(sub))
            .map(|p| (p.name.clone(), p.is_fragment, p.d3d9_tokens.to_vec()))
            .collect()
    }

    /// PIN: the CPU module's register contract == the REAL shipped
    /// gfxtextshader pair's CTAB, byte-for-byte proven.
    #[test]
    fn gfxtext_contract_matches_shipped_sm30_pair() {
        let vp = shipped_pair("gfxtextshader - lit - Vertex Program");
        let fp = shipped_pair("gfxtextshader - lit - Fragment Program");
        assert_eq!(vp.len(), 1, "exactly one gfxtextshader VP ships");
        assert_eq!(fp.len(), 1, "exactly one gfxtextshader FP ships");

        let v = Sm30Shader::parse(&vp[0].2, false).unwrap();
        let f = Sm30Shader::parse(&fp[0].2, true).unwrap();
        assert_eq!(v.instructions.len(), 12, "VP instruction count (PROVEN)");
        assert_eq!(f.instructions.len(), 8, "FP instruction count (PROVEN)");

        fn ctab<'a>(s: &'a Sm30Shader, n: &str) -> &'a crate::shader::sm30::CtabConstant {
            s.ctab
                .as_ref()
                .unwrap()
                .constants
                .iter()
                .find(|c| c.name == n)
                .unwrap_or_else(move || panic!("constant {n} missing"))
        }
        // VP: WVP c0 (4 regs) — REQUIRED.  The Xenon-only constants
        // (LightBounds c4, NaN c5) are OPTIMISED OUT of the shipped PC
        // CTAB by fxc (the HLSL declares them; the RAD_XENON branch is
        // not compiled for the PC program) — pinned optional.
        let wvp = ctab(&v, "p3dWorldViewProjectionMatrix");
        assert_eq!(
            (wvp.register_set, wvp.register_index, wvp.register_count),
            (2, 0, 4)
        );
        for (name, idx) in [("p3dLight_ObjectSpaceLightBounds", 4), ("p3dNaN", 5)] {
            if let Some(c) = v.ctab.as_ref().unwrap().constants.iter().find(|c| c.name == name) {
                assert_eq!(c.register_index, idx, "{name} slot (declared)");
            }
        }
        // FP: diffuseMap s0 (sampler), cxmul c2, cxadd c3 — the
        // PROVEN shipped CTAB census is EXACTLY these three: fxc
        // folds p3dBuiltIn_Zero/One into immediates and strips the
        // unused-on-PC LightBounds/NaN (the HLSL declares all of
        // them; the compiled PC program reads only these three).
        let d = ctab(&f, "diffuseMap");
        assert_eq!((d.register_set, d.register_index, d.register_count), (3, 0, 1));
        assert_eq!(
            f.ctab.as_ref().unwrap().constants.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["cxadd", "cxmul", "diffuseMap"],
            "the PROVEN FP CTAB census (order as shipped)"
        );
        for (name, idx) in [("cxmul", 2), ("cxadd", 3)] {
            assert_eq!(ctab(&f, name).register_index, idx, "{name} slot");
        }
        // The optional/optimised-out constants keep their HLSL slots
        // whenever a build does declare them.
        for (name, idx) in [("p3dLight_ObjectSpaceLightBounds", 0), ("p3dNaN", 1), ("p3dBuiltIn_Zero", 4), ("p3dBuiltIn_One", 5)] {
            if let Some(c) = f.ctab.as_ref().unwrap().constants.iter().find(|c| c.name == name) {
                assert_eq!(c.register_index, idx, "{name} slot (declared)");
            }
        }
    }

    /// PIN: the CPU module's contract == the Radical HLSL sources
    /// extracted in block 86 (the register declarations + the exact
    /// FP helper chain).
    #[test]
    fn gfxtext_contract_matches_committed_hlsl_fixtures() {
        let vs = String::from_utf8(fixture("gfxtextshader_vert.hlsl")).unwrap();
        assert!(vs.contains("p3dWorldViewProjectionMatrix: register( c0 )"));
        assert!(vs.contains("p3dLight_ObjectSpaceLightBounds: register( c4 )"));
        assert!(vs.contains("p3dNaN: register( c5 )"));
        assert!(vs.contains("hlslFunc_GenerateWVPPosition0"));
        assert!(vs.contains("p3dVertex_Colour0      : COLOR0"));
        assert!(vs.contains("p3dVertex_Texcoord0    : TEXCOORD0"));

        let fs = String::from_utf8(fixture("gfxtextshader_frag.hlsl")).unwrap();
        assert!(fs.contains("diffuseMap: register( s0 )"));
        assert!(fs.contains("cxmul: register( c2 )"));
        assert!(fs.contains("cxadd: register( c3 )"));
        assert!(fs.contains("p3dBuiltIn_Zero: register( c4 )"));
        assert!(fs.contains("p3dBuiltIn_One: register( c5 )"));
        // The literal math chain: alpha = vc.w × tex.w, rgb = vc.rgb,
        // out = cxmul·c + cxadd (MultiplyAdd2), Pure3D_Colour00 pass.
        assert!(fs.contains("hlslFunc_Multiply0( out31.w, out931.w, out943 )"));
        assert!(fs.contains("hlslFunc_Float40( out31.x, out31.y, out31.z, out943, out933 )"));
        assert!(fs.contains("hlslFunc_MultiplyAdd2( cxmul, out933, cxadd, out936 )"));
        assert!(fs.contains("hlslFunc_Pure3D_Colour00( out936.xyz, out936.w, out940 )"));
    }

    /// The VS is the literal port: identity WVP leaves the position
    /// alone; a scaled WVP transforms ONLY the position (uv + colour
    /// pass straight through — the text-shader feature).
    #[test]
    fn gfxtext_vertex_is_passthrough_with_wvp() {
        let uv = [0.25, 0.75];
        let colour = [0.8, 0.4, 0.2, 1.0];
        let identity = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let (clip, u, c) = gfxtext_vertex_stage([-0.5, 0.5], &identity, uv, colour);
        assert_eq!(clip, [-0.5, 0.5, 0.0, 1.0]);
        assert_eq!(u, uv);
        assert_eq!(c, colour);

        // 2× scale on x/y, translation in the 4th COLUMN (the
        // convention the legacy/default CPU mirrors and their camera
        // matrices use).
        let scaled = [
            [2.0, 0.0, 0.0, 0.1],
            [0.0, 2.0, 0.0, 0.2],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let (clip, u, c) = gfxtext_vertex_stage([1.0, 1.0], &scaled, uv, colour);
        assert_eq!(clip, [2.1, 2.2, 0.0, 1.0]);
        assert_eq!(u, uv, "uv passes through");
        assert_eq!(c, colour, "colour passes through");
    }

    /// The FP is a bit-exact port: alpha = vc.a × tex.a, rgb = vc.rgb
    /// (the texel RGB is DISCARDED — the text-shader tint feature),
    /// then the cxform multiply-add.
    #[test]
    fn gfxtext_fragment_is_exact_port() {
        // texel RGB values are chosen distinct to prove they are ignored.
        let out = gfxtext_fragment_stage(
            [0.8, 0.4, 0.2, 1.0],
            [0.9, 0.7, 0.6, 0.5],
            [1.0, 1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0, 0.0],
        );
        assert_eq!(out, [0.8, 0.4, 0.2, 0.5]);

        // With a real cxform: rgb = cxmul·vc.rgb + cxadd, alpha =
        // cxmul.a·(vc.a·tex.a) + cxadd.a — hand-computed.
        let out = gfxtext_fragment_stage(
            [0.8, 0.4, 0.2, 1.0],
            [0.9, 0.7, 0.6, 0.5],
            [2.0, 1.0, 1.0, 1.0],
            [0.1, 0.0, 0.0, 0.25],
        );
        assert_eq!(out[0], 0.8 * 2.0 + 0.1);
        assert_eq!(out[1], 0.4);
        assert_eq!(out[2], 0.2);
        assert_eq!(out[3], 1.0 * 0.5 * 1.0 + 0.25);
    }

    /// The alpha blend composite: a text pixel over a known base =
    /// src.rgb·a + dst.rgb·(1−a) exactly.  The expected value is
    /// rebuilt from the SAME sample point the rasterizer interpolates
    /// (the glyph coverage at an 8×8 target is bilinear-mixed, so the
    /// test derives the texel instead of guessing it).
    #[test]
    fn gfxtext_alpha_blend_composite() {
        let atlas = build_glyph_atlas(8);
        let uv = atlas.glyph_uv('A').unwrap();
        let rect = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
        let q = GfxTextQuad::from_rect(rect, uv, [0.8, 0.4, 0.2, 0.5]);
        let base = vec![25u8; 8 * 8 * 4];
        let frame =
            render_gfxtext_cpu(std::slice::from_ref(&q), &atlas, Some(&base), 8, 8).unwrap();
        // The QUAD CENTRE — pixel (3,3), centre (3.5, 3.5) — lies ON
        // the shared bl→tr diagonal; with the top-left fill rule it is
        // claimed by triangle (0,1,2) = (bl, br, tr) ONLY (the other
        // triangle's diagonal edge runs the opposite direction, so its
        // edge-zero gate rejects it) → single blend.  Replicate the
        // rasterizer's corrected barycentric uv for that triangle.
        let (a, b, c) = ((0.0f32, 7.0f32), (7.0, 7.0), (7.0, 0.0)); // bl,br,tr
        let (px, py) = (3.5f32, 3.5f32);
        let area = (b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1);
        let s = area.signum();
        let ea = s * ((c.0 - b.0) * (py - b.1) - (c.1 - b.1) * (px - b.0));
        let eb = s * ((a.0 - c.0) * (py - c.1) - (a.1 - c.1) * (px - c.0));
        let ec = s * ((b.0 - a.0) * (py - a.1) - (b.1 - a.1) * (px - a.0));
        let wa = ea / area.abs();
        let wb = eb / area.abs();
        let wc = ec / area.abs();
        let u = uv[0][0] * wa + uv[1][0] * wb + uv[2][0] * wc;
        let v = uv[0][1] * wa + uv[1][1] * wb + uv[2][1] * wc;
        let tex = sample_clamp_bilinear(&atlas.rgba, atlas.width, atlas.height, u, v);
        let src = gfxtext_fragment_stage([0.8, 0.4, 0.2, 0.5], tex, q.cxmul, q.cxadd);
        let sa = src[3].clamp(0.0, 1.0);
        let dst = 25.0 / 255.0;
        let want = src[0] * sa + dst * (1.0 - sa);
        let o = (3 * 8 + 3) * 4;
        let r = frame.rgba[o] as f32;
        assert!((r - want * 255.0).abs() <= 1.5, "blend: got {r}, want {}", want * 255.0);
        assert!(sa > 0.05, "the sample point must be on the glyph: sa={sa}");
    }

    /// The atlas + layout produce READABLE text: rendering a string
    /// lays out one quad per glyph and lights the target.
    #[test]
    fn gfxtext_atlas_layout_is_readable() {
        let atlas = build_glyph_atlas(8);
        assert!(atlas.slot('A').is_some());
        assert!(atlas.slot('m').is_some(), "lowercase glyphs present");
        let quads = layout_text(&atlas, "manhattan", -0.9, 0.5, 0.05, 0.07, 0.005, [1.0; 4]);
        assert_eq!(quads.len(), 9, "one quad per glyph");
        let frame = render_gfxtext_cpu(&quads, &atlas, None, 160, 60).unwrap();
        assert!(frame.stats.distinct >= 3, "{:?}", frame.stats);
        // Texel-centre bilinear softens every glyph edge (all sample
        // points straddle texels), so full alpha is ~236, not 255.
        assert!(frame.stats.max[3] >= 200, "glyph pixels stay lit: {:?}", frame.stats);
    }

    /// End-to-end: the evidence scene renders non-blank at the media
    /// resolution, and the text visibly lands ON the background.
    #[test]
    fn gfxtext_scene_frame_is_nonblank() {
        let atlas = build_glyph_atlas(8);
        let frame = TextHudScene::render_frame(0.45, &atlas, 480, 360).unwrap();
        assert!(frame.stats.distinct > 64, "{:?}", frame.stats);
        assert!(frame.stats.max[0] >= 200, "text reaches near-full red: {:?}", frame.stats);
        assert_eq!(frame.rgba.len(), 480 * 360 * 4);
    }
}

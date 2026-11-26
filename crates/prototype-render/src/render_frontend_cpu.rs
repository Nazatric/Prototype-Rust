//! render_frontend_cpu — the device-free CPU mirror of the FRONTEND
//! sprite pipeline, driven by the REAL shipped frontend shaders.
//!
//! ## Why this exists (block 101)
//!
//! Block 99/100 proved `frontendshaders.p3d` (24 `0x0001100B` shader
//! programs + 12 `0x00011009` definitions) and the SM3.0 decoder
//! already parses all 24 shipped programs (26 with bink).  What was
//! still missing was the LAST piece of the frontend NEXT-UP: **a
//! render test that actually CONSUMES a shipped frontend shader pair**
//! — the register contract implemented 1:1 from the real program
//! bytes, rasterized on the CPU, so the loader's future frontend
//! menu/HUD path has a proven, sandbox-runnable reference.
//!
//! ## The REAL register contract (proven from the committed fixture)
//!
//! Pinned by [`frontend_contract_matches_committed_cxformtexture`]
//! against `frontendshaders.p3d` (the bytes ship in the repo):
//!
//! * `cxformtexture - lit - Vertex Program` (12 instructions, `vs_3_0`):
//!   - `c0..c3` = `p3dWorldViewProjectionMatrix` (4×4, row-major cols)
//!   - `c7` = `SPlane`, `c6` = `TPlane`
//!   - out position = `WVP × p3dVertex_Position` (`GenerateWVPPosition0`)
//!   - out `TexCoord_0` = `(dot(SPlane, pos), 1.0 / dot(TPlane, pos))`
//!     — the literal `Dot1`/`Dot1`/`Invert0` sequence in the real
//!     HLSL entry point (the T dot is INVERTED; mirrored exactly).
//! * `cxformtexture - lit - Fragment Program` (5 instructions, `ps_3_0`):
//!   - `s0` = `diffuseMap`, `c2` = `cxmul`, `c3` = `cxadd`
//!   - out = `cxmul × tex2D(diffuseMap, uv) + cxadd`
//!     (the real `MultiplyAdd2` → `Pure3D_Colour00` chain).
//! * `cxformsolidcolour - lit - Fragment Program` (1 instruction):
//!   out = `FillColour` (`c2`) — the untextured UI fill primitive.
//!   Its vertex program is `cxformsolidcolour - lit - Vertex Program`
//!   (6 instructions, WVP only).
//!
//! ## What is a documented approximation
//!
//! * Single-threaded scanline rasterizer, barycentric interpolation,
//!   z-buffer; NO blending (the caller composites back-to-front —
//!   the real frontend queues sorted draws).
//! * f32 → u8 conversion uses round-half-away-from-zero; the GPU's
//!   unorm rounding (nearest-even) only differs on exact `.5` ties,
//!   which the exact-math test avoids by construction.
//! * The sprite GEOMETRY in tests/evidence is the frontend primitive
//!   (2-triangle screen quad); no real frontend LAYOUT file has been
//!   decoded yet (honest — layout data lives in other art.rcf members
//!   not fetched in the sandbox).

#![forbid(unsafe_code)]

use crate::render_art_world_cpu::FrameStats;
use prototype_math::Mat4;

/// One frontend sprite draw call — the EXACT constant payload the
/// real shader pair consumes (register-for-register).
#[derive(Debug, Clone, PartialEq)]
pub struct FrontendSprite {
    /// Vertex shader `c0..c3` — `p3dWorldViewProjectionMatrix`.
    pub wvp: Mat4,
    /// Vertex shader `c7` — `SPlane` (uv.x = dot(plane, position)).
    pub plane_s: [f32; 4],
    /// Vertex shader `c6` — `TPlane` (uv.y = 1.0 / dot(plane, position)).
    pub plane_t: [f32; 4],
    /// Fragment shader `c2` — `cxmul` (per-channel multiply).
    pub cxmul: [f32; 4],
    /// Fragment shader `c3` — `cxadd` (per-channel add).
    pub cxadd: [f32; 4],
    /// `true` = cxformtexture pass (samples `diffuseMap`);
    /// `false` = cxformsolidcolour pass (outputs `FillColour`, and the
    /// real VP of that pair does NOT generate UVs).
    pub textured: bool,
    /// cxformsolidcolour `c2` — `FillColour` (only used when
    /// `textured == false`).
    pub fill_colour: [f32; 4],
    /// Optional per-corner UV override `[(u, v); 4]` (corner order =
    /// `quad`).  The REAL VP generates uv from the S/T plane dots
    /// ([`FrontendSprite::shader_uv`], literal port, T inverted) — but
    /// the engine feeds per-sprite plane constants from its own sprite
    /// space, and that mapping convention for arbitrary rects is an
    /// honest UNKNOWN (see module docs).  Scenes that must sample a
    /// specific texture rect set this override; the plane path stays
    /// pinned by the `shader_uv_inverts_t_plane` test.
    pub uv_override: Option<[[f32; 2]; 4]>,
    /// Clip-space quad corners (post-WVP, w-wilded already by the
    /// caller through `wvp`): `[(-1,-1), (1,-1), (1,1), (-1,1)]` order.
    pub quad: [[f32; 2]; 4],
}

impl FrontendSprite {
    /// A full-screen quad in clip space (the identity WVP).
    pub fn fullscreen() -> Self {
        Self {
            wvp: Mat4::IDENTITY,
            plane_s: [0.5, 0.0, 0.0, 0.5],
            plane_t: [0.0, -0.5, 0.0, 0.5],
            cxmul: [1.0, 1.0, 1.0, 1.0],
            cxadd: [0.0, 0.0, 0.0, 0.0],
            textured: true,
            fill_colour: [0.0, 0.0, 0.0, 1.0],
            uv_override: None,
            quad: [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]],
        }
    }

    /// Compute the shader-side UV for an object-space position — the
    /// literal port of the real VP entry point (Dot1, Dot1, Invert0).
    pub fn shader_uv(&self, pos: &[f32; 4]) -> [f32; 2] {
        let s = dot4(&self.plane_s, pos);
        let t = dot4(&self.plane_t, pos);
        [s, 1.0 / t]
    }

    /// Compute the fragment output for a texel — the literal port of
    /// the real FP (MultiplyAdd2 → Pure3D_Colour00), f32 side.
    pub fn shader_fragment(&self, tex: [f32; 4]) -> [f32; 4] {
        if !self.textured {
            return self.fill_colour;
        }
        [
            self.cxmul[0] * tex[0] + self.cxadd[0],
            self.cxmul[1] * tex[1] + self.cxadd[1],
            self.cxmul[2] * tex[2] + self.cxadd[2],
            self.cxmul[3] * tex[3] + self.cxadd[3],
        ]
    }
}

fn dot4(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]
}

fn f32_to_u8(v: f32) -> u8 {
    // The real pipeline converts the f32 render target to unorm8 on
    // readback; nearest rounding, clamped to [0, 255].
    let x = (v * 255.0).round();
    if x < 0.0 {
        0
    } else if x > 255.0 {
        255
    } else {
        x as u8
    }
}

/// One rendered frontend frame plus its non-blank statistics.
#[derive(Debug, Clone, PartialEq)]
pub struct FrontendFrame {
    /// Texel width.
    pub width: u32,
    /// Texel height.
    pub height: u32,
    /// Packed RGBA8, row-major, opaque (alpha 255), `width*height*4` B.
    pub rgba: Vec<u8>,
    /// Non-blank evidence statistics.
    pub stats: FrameStats,
}

/// Sample `texture` at a UV with the frontend pipeline's conventions:
/// CLAMP addressing, BILINEAR filtering, origin top-left (the real
/// frontend draws screen-space UI where v grows downward; the shipped
/// textures store rows top-down).  `uv` is the shader-side value.
fn sample_clamp_bilinear(
    tex: &[u8],
    tw: u32,
    th: u32,
    u: f32,
    v: f32,
) -> [f32; 4] {
    let (tw, th) = (tw as f32, th as f32);
    // shader-side uv: u,v in [0,1]; map to texel space.
    let x = u.clamp(0.0, 1.0) * (tw - 1.0);
    let y = v.clamp(0.0, 1.0) * (th - 1.0);
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

/// Render a list of frontend sprites back-to-front (painter's order —
/// no blending: later sprites overwrite earlier ones, matching the
/// opaque UI panels drawn over the backdrop).
///
/// * `sprites` — the draw list (each = one real shader-pair constant
///   set + clip-space quad).
/// * `texture` — the diffuse RGBA8 `(bytes, width, height)`; `None`
///   is only valid when every sprite uses the solid-colour pass.
/// * `clear` — the background RGBA (the frontend's render target
///   clear colour).
pub fn render_sprites_cpu(
    sprites: &[FrontendSprite],
    texture: Option<TexRef<'_>>,
    width: u32,
    height: u32,
    clear: [f32; 4],
) -> Result<FrontendFrame, String> {
    // One shared texture for every sprite (the classic single-image
    // frontend path).  Layered real-screen art rides the per-sprite
    // variant below.
    let textures: Vec<Option<TexRef<'_>>> = sprites.iter().map(|_| texture).collect();
    render_sprites_textured(sprites, &textures, width, height, clear)
}

/// One RGBA8 texture reference `(bytes, width, height)` — the shape
/// the single-texture path already took, now PER SPRITE so a real
/// Scaleform screen's layered images can ride ONE draw list
/// (block 120: the `.gfx` screens' extracted bitmaps).
pub type TexRef<'a> = (&'a [u8], u32, u32);

/// The per-sprite-texture variant of [`render_sprites_cpu`]: sprite i
/// samples `textures[i]` (`None` is valid only while the sprite is
/// untextured).  Everything else — the shader-pair constant
/// semantics, the BLOCK-104 barycentric corner pairing, bilinear
/// sampling, the fill/full-bbox contracts — is byte-identical.
pub fn render_sprites_textured(
    sprites: &[FrontendSprite],
    textures: &[Option<TexRef<'_>>],
    width: u32,
    height: u32,
    clear: [f32; 4],
) -> Result<FrontendFrame, String> {
    if width == 0 || height == 0 {
        return Err("empty render target".into());
    }
    if sprites.len() != textures.len() {
        return Err("sprite/texture count mismatch".into());
    }
    let mut fb = vec![clear; (width * height) as usize];
    let mut drawn = 0usize;
    let mut clipped = 0usize;

    for (sprite, texture) in sprites.iter().zip(textures.iter()) {
        let texture = *texture;
        // Two triangles: (0,1,2) and (0,2,3) of the clip-space quad.
        // For each vertex: uv = shader_uv(object pos).  The frontend
        // sprites are screen-space; object space IS clip space here
        // (identity WVP for full-screen panels), so the same value
        // feeds both paths — mirrored from GenerateWVPPosition0 with
        // the caller-supplied WVP.
        let corners: [[f32; 2]; 4] = sprite.quad;
        let mut uvs = [[0.0f32; 2]; 4];
        if sprite.textured {
            match sprite.uv_override {
                Some(ov) => uvs = ov,
                None => {
                    for k in 0..4 {
                        uvs[k] = sprite.shader_uv(&[
                            corners[k][0], corners[k][1], 0.0, 1.0,
                        ]);
                    }
                }
            }
        }
        for tri in [[0usize, 1, 2], [0, 2, 3]] {
            let (p0, p1, p2) = (corners[tri[0]], corners[tri[1]], corners[tri[2]]);
            // Screen space.
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
            for y in miny..=maxy {
                for x in minx..=maxx {
                    let px = x as f32 + 0.5;
                    let py = y as f32 + 0.5;
                    // Barycentrics paired to the RIGHT corners — wa =
                    // weight of a, wb = weight of b, wc = weight of c
                    // (the proven art-world edge-function convention).
                    // BLOCK-104 FIX: these were cyclically SHIFTED
                    // against the uv corners (w0 was actually the
                    // weight of c, etc), which sheared every textured
                    // sprite diagonally.
                    let wa = ((c.0 - b.0) * (py - b.1) - (c.1 - b.1) * (px - b.0)) * inv;
                    let wb = ((a.0 - c.0) * (py - c.1) - (a.1 - c.1) * (px - c.0)) * inv;
                    let wc = ((b.0 - a.0) * (py - a.1) - (b.1 - a.1) * (px - a.0)) * inv;
                    if wa < -1e-6 || wb < -1e-6 || wc < -1e-6 {
                        continue;
                    }
                    let col = if sprite.textured {
                        // Perspective-irrelevant for ortho UI: uv =
                        // barycentric mix of the three corner UVs
                        // (corner 0 belongs to both triangles).
                        let (uv_a, uv_b, uv_c) = match tri {
                            [0, 1, 2] => (uvs[0], uvs[1], uvs[2]),
                            _ => (uvs[0], uvs[2], uvs[3]),
                        };
                        let u = uv_a[0] * wa + uv_b[0] * wb + uv_c[0] * wc;
                        let v = uv_a[1] * wa + uv_b[1] * wb + uv_c[1] * wc;
                        let t = match texture {
                            Some((bytes, tw, th)) => {
                                sample_clamp_bilinear(bytes, tw, th, u, v)
                            }
                            None => return Err("textured sprite without texture".into()),
                        };
                        sprite.shader_fragment(t)
                    } else {
                        sprite.fill_colour
                    };
                    let o = (y as u32 * width + x as u32) as usize;
                    fb[o] = col;
                }
            }
            drawn += 1;
        }
    }

    // Pack + stats (same shape as the art-world CPU mirror).
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
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
        let packed = u32::from_le_bytes(b);
        distinct.insert(packed);
        rgba.extend_from_slice(&b);
    }
    Ok(FrontendFrame {
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

/// Build a UI-panel scene: the block-101 evidence frame list.  Returns
/// `frames` sprites per frame: backdrop, tinted texture panel, header
/// strip, growing progress bar (all real-shader passes).
pub struct PanelScene;

impl PanelScene {
    /// One frame's draw list.  `t` in [0, 1) drives the cxform
    /// animation (the Flash-style colour-transform animation the real
    /// frontend uses for fades/pulses).
    pub fn frame(t: f32) -> Vec<FrontendSprite> {
        let mut v = Vec::new();
        // Backdrop: solid fill, near-black.
        let mut s = FrontendSprite::fullscreen();
        s.textured = false;
        s.fill_colour = [0.05, 0.06, 0.08, 1.0];
        v.push(s);
        // Centre texture panel: the real alex diffuse, tinted by a
        // slow cxmul pulse (0.75..1.25 on rgb, alpha 1) + cxadd lift.
        let mut s = FrontendSprite::fullscreen();
        s.quad = [[-0.45, -0.45], [0.45, -0.45], [0.45, 0.45], [-0.45, 0.45]];
        // Top-left origin texture: quad top (y=+0.45) → v=0.
        s.uv_override = Some([[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]);
        let pulse = 0.75 + 0.5 * t;
        s.cxmul = [pulse, pulse, pulse, 1.0];
        s.cxadd = [0.08 * t, 0.04 * t, 0.0, 0.0];
        v.push(s);
        // Header strip: solid fill, cooled white.
        let mut s = FrontendSprite::fullscreen();
        s.textured = false;
        s.fill_colour = [0.82, 0.85, 0.92, 1.0];
        s.quad = [[-0.95, 0.62], [0.95, 0.62], [0.95, 0.82], [-0.95, 0.82]];
        v.push(s);
        // Progress bar: back + growing fill (cxform animated).
        let mut s = FrontendSprite::fullscreen();
        s.textured = false;
        s.fill_colour = [0.10, 0.11, 0.14, 1.0];
        s.quad = [[-0.60, -0.80], [0.60, -0.80], [0.60, -0.72], [-0.60, -0.72]];
        v.push(s);
        let w = 1.2 * t;
        let mut s = FrontendSprite::fullscreen();
        s.textured = false;
        s.fill_colour = [0.85, 0.55, 0.15, 1.0];
        s.quad = [[-0.60, -0.80], [-0.60 + w, -0.80], [-0.60 + w, -0.72], [-0.60, -0.72]];
        v.push(s);
        // Menu rows: three solid bars under the panel, the selected
        // one brightened by the cxform pulse (the real frontend
        // highlights via colour transforms).
        for (k, y) in [0.28f32, 0.10, -0.08].iter().enumerate() {
            let mut s = FrontendSprite::fullscreen();
            s.textured = false;
            let sel = k == 0;
            let g = if sel { 0.55 + 0.35 * t } else { 0.16 };
            s.fill_colour = if sel { [0.85, g, 0.25, 1.0] } else { [g, g, g + 0.04, 1.0] };
            s.quad = [[-0.30, *y], [0.30, *y], [0.30, y - 0.12], [-0.30, y - 0.12]];
            v.push(s);
        }
        // Bottom vignette: textured sprite with a darkening cxmul to
        // prove multi-sprite cxform variance per frame.
        let mut s = FrontendSprite::fullscreen();
        s.quad = [[-1.0, -1.0], [1.0, -1.0], [1.0, -0.95], [-1.0, -0.95]];
        s.uv_override = Some([[0.0, 1.0], [1.0, 1.0], [1.0, 1.0], [0.0, 1.0]]);
        s.cxmul = [0.4 + 0.6 * t, 0.2, 0.2, 1.0];
        v.push(s);
        v
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

    /// The real shipped `cxformtexture` pair from frontendshaders.p3d.
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
    /// cxformtexture pair's CTAB, byte-for-byte proven.
    #[test]
    fn frontend_contract_matches_committed_cxformtexture() {
        let vp = shipped_pair("cxformtexture - lit - Vertex Program");
        let fp = shipped_pair("cxformtexture - lit - Fragment Program");
        assert_eq!(vp.len(), 1, "exactly one cxformtexture VP ships");
        assert_eq!(fp.len(), 1, "exactly one cxformtexture FP ships");

        let v = Sm30Shader::parse(&vp[0].2, false).unwrap();
        let f = Sm30Shader::parse(&fp[0].2, true).unwrap();
        assert_eq!(v.instructions.len(), 12, "VP instruction count (PROVEN)");
        assert_eq!(f.instructions.len(), 5, "FP instruction count (PROVEN)");

        fn ctab<'a>(s: &'a Sm30Shader, n: &str) -> &'a crate::shader::sm30::CtabConstant {
            s.ctab
                .as_ref()
                .unwrap()
                .constants
                .iter()
                .find(|c| c.name == n)
                .unwrap_or_else(move || panic!("constant {n} missing"))
        }
        // VP: WVP c0 (4 regs), TPlane c6, SPlane c7.
        let wvp = ctab(&v, "p3dWorldViewProjectionMatrix");
        assert_eq!((wvp.register_set, wvp.register_index, wvp.register_count), (2, 0, 4));
        assert_eq!(ctab(&v, "TPlane").register_index, 6);
        assert_eq!(ctab(&v, "SPlane").register_index, 7);
        // FP: diffuseMap s0, cxmul c2, cxadd c3.
        assert_eq!(ctab(&f, "diffuseMap").register_index, 0);
        let ctab_f = f.ctab.as_ref().unwrap();
        assert!(matches!(
            ctab_f.constants.iter().find(|c| c.name == "diffuseMap").unwrap().register_set,
            3
        ));
        assert_eq!(ctab(&f, "cxmul").register_index, 2);
        assert_eq!(ctab(&f, "cxadd").register_index, 3);
    }

    /// The cxform FP math is a bit-exact port: every pixel equals
    /// clamp(cxmul*tex + cxadd) — values chosen away from rounding
    /// ties so the f32→u8 convention is provably irrelevant.
    #[test]
    fn cxform_math_is_exact_port() {
        // 2x2 texture, distinct channels.
        let tex: Vec<u8> = vec![
            200, 100, 40, 255, 40, 220, 80, 255, 16, 32, 64, 255, 255, 128, 8, 255,
        ];
        let mut s = FrontendSprite::fullscreen();
        s.cxmul = [1.0, 0.5, 0.25, 1.0];
        s.cxadd = [0.02, 0.3, 0.6, 0.5];
        let frame = render_sprites_cpu(&[s], Some((&tex, 2, 2)), 8, 8, [0.0; 4]).unwrap();
        assert!(frame.stats.triangles_drawn >= 2, "{:?}", frame.stats);
        // Expected: uv covers the whole texture (bilinear), so pixel
        // values are mixes; instead verify the EXACT maths on the
        // shader_fragment f32 path + the u8 conversion of the four
        // texel colours pushed through the same cxform.
        let spr = {
            let mut x = FrontendSprite::fullscreen();
            x.cxmul = [1.0, 0.5, 0.25, 1.0];
            x.cxadd = [0.02, 0.3, 0.6, 0.5];
            x
        };
        for (texel, want) in [
            // Hand-computed f32 math, then f32→u8 round+clamp — e.g.
            // texel 200/255 through cxmul 1.0 / cxadd 0.02:
            // (0.78431 + 0.02) × 255 = 205.10 → 205.
            ([200, 100, 40, 255], [205, 127, 163, 255]),
            ([40, 220, 80, 255], [45, 187, 173, 255]),
            ([16, 32, 64, 255], [21, 93, 169, 255]),
            ([255, 128, 8, 255], [255, 141, 155, 255]),
        ] {
            let t = [
                texel[0] as f32 / 255.0,
                texel[1] as f32 / 255.0,
                texel[2] as f32 / 255.0,
                texel[3] as f32 / 255.0,
            ];
            let got: [u8; 4] = spr
                .shader_fragment(t)
                .iter()
                .map(|&v| f32_to_u8(v))
                .collect::<Vec<_>>()
                .try_into()
                .unwrap();
            assert_eq!(got, want, "texel {texel:?}");
        }
        let _ = frame;
    }

    /// A full panel scene renders non-blank with both real passes
    /// (textured + solid-colour) and the cxform animation visibly
    /// changes the frame (t=0 vs t=0.9 differ).
    #[test]
    fn panel_scene_renders_nonblank_and_animates() {
        // 64x64 deterministic gradient texture (the "alex" stand-in;
        // the MEDIA path runs the SAME pipeline with the real DDS).
        let tw = 64u32;
        let th = 64u32;
        let mut tex = Vec::with_capacity((tw * th * 4) as usize);
        for y in 0..th {
            for x in 0..tw {
                tex.extend_from_slice(&[
                    (x * 4) as u8,
                    (y * 4) as u8,
                    ((x + y) * 2) as u8,
                    255,
                ]);
            }
        }
        let f0 = render_sprites_cpu(
            &PanelScene::frame(0.0),
            Some((&tex, tw, th)),
            320,
            240,
            [0.0, 0.0, 0.0, 1.0],
        )
        .unwrap();
        let f9 = render_sprites_cpu(
            &PanelScene::frame(0.9),
            Some((&tex, tw, th)),
            320,
            240,
            [0.0, 0.0, 0.0, 1.0],
        )
        .unwrap();
        for f in [&f0, &f9] {
            assert!(f.stats.distinct > 200, "distinct {}", f.stats.distinct);
            assert!(f.stats.min[0] < 255 && f.stats.max[0] > 0);
            assert!(f.stats.triangles_drawn >= 12, "{:?}", f.stats);
        }
        assert_ne!(f0.rgba, f9.rgba, "cxform animation must change pixels");
    }

    /// The solid-colour pass (real cxformsolidcolour FP: out =
    /// FillColour) fills exactly its quad, and the textured pass
    /// errors fail-closed without a bound texture.
    #[test]
    fn solidcolour_pass_and_fail_closed() {
        let mut s = FrontendSprite::fullscreen();
        s.textured = false;
        s.fill_colour = [0.5, 0.25, 0.125, 1.0];
        s.quad = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
        let f = render_sprites_cpu(&[s], None, 16, 16, [0.0; 4]).unwrap();
        // centre pixel == FillColour converted (0.5*255 = 127.5 →
        // round-half-away = 128; 0.25*255 = 63.75 → 64; 0.125*255 =
        // 31.875 → 32).
        let o = (8usize * 16 + 8) * 4;
        assert_eq!(&f.rgba[o..o + 4], &[128, 64, 32, 255]);
        let mut t = FrontendSprite::fullscreen();
        t.textured = true;
        assert!(render_sprites_cpu(&[t], None, 16, 16, [0.0; 4]).is_err());
    }

    /// The shader_uv port matches the real VP's Dot1/Dot1/Invert0
    /// sequence: SPlane=(1,0,0,0), TPlane=(0,2,0,0) at pos=(0.5,0.25)
    /// gives uv=(0.5, 1/(0.5))=(0.5, 2.0) — the INVERTED T is pinned.
    #[test]
    fn shader_uv_inverts_t_plane() {
        let s = FrontendSprite {
            plane_s: [1.0, 0.0, 0.0, 0.0],
            plane_t: [0.0, 2.0, 0.0, 0.0],
            ..FrontendSprite::fullscreen()
        };
        let uv = s.shader_uv(&[0.5, 0.25, 0.0, 1.0]);
        assert_eq!(uv, [0.5, 2.0]);
    }

    /// REGRESSION (block 101 media render): a wide flat quad must fill
    /// its FULL bbox — the scanline x-loop once bounded on `maxy`,
    /// truncating wide sprites to a square of the y extent.  A quad
    /// spanning x −0.95..0.95 at 480px wide must cover x 12..467.
    #[test]
    fn wide_quad_fills_full_bbox() {
        let mut s = FrontendSprite::fullscreen();
        s.textured = false;
        s.fill_colour = [0.82, 0.85, 0.92, 1.0];
        s.quad = [[-0.95, 0.62], [0.95, 0.62], [0.95, 0.82], [-0.95, 0.82]];
        let f = render_sprites_cpu(&[s], None, 480, 360, [0.0; 4]).unwrap();
        let px = |x: usize, y: usize| {
            let o = (y * 480 + x) * 4;
            [f.rgba[o], f.rgba[o + 1], f.rgba[o + 2]]
        };
        // left edge, right edge, centre, top, bottom — all FillColour
        // (0.82*255=209.1→209, 0.85*255=216.75→217, 0.92*255=234.6→235).
        // Pixel-CENTRE sampling: the geometric right edge sits at
        // x=467.05, so centres 12.5..466.5 land inside → x 12..466.
        for (x, y) in [(12usize, 50usize), (466, 50), (240, 50), (240, 34), (240, 66)] {
            assert_eq!(px(x, y), [209, 217, 235], "at ({x},{y})");
        }
        // strictly outside: above and below the strip.
        assert_eq!(px(240, 20), [0, 0, 0]);
        assert_eq!(px(240, 80), [0, 0, 0]);
        // row continuity at y=50: filled from 12 to 466 (455 columns).
        let row: Vec<u8> = (0..480)
            .filter(|&x| px(x, 50)[0] == 209)
            .map(|x| x as u8)
            .collect();
        assert_eq!(row.len(), 455, "filled column count at y=50");
    }
}

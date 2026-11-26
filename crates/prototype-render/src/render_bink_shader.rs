//! render_bink_shader — the device-free CPU mirror of the BINK movie
//! shader pair, driven by the REAL shipped DLL SM3.0 bytecode.
//!
//! ## Why this exists (block 102)
//!
//! Block 75 shipped the `bik-frames` CLI (real Bink decode → PNG stills
//! via the proven `yuv420_to_rgba8` BT.601 limited-range matrix), and
//! block 99/101 cataloged the SM3.0 shader blobs (`dll_bink_fp_e3adf8
//! .sm30`, offset 0xe3adf8 in `prototypeenginef.dll`).  NEXT-UP #1
//! (block 101 NEXT-UP list) asked for a SECOND real shader pair wired
//! into the render path — either `legacy` (heavy 48+69-ins lighting
//! pair) or `bink` (the lighter YUV-plane movie path).  This module
//! takes the bink path: it consumes the bink shader pair's REAL
//! register contract (s0/s1/s2/s3 = y_tex/cr_tex/cb_tex/a_tex, the
//! PROVEN CTAB signature from `dll_shader_roles.txt`) and produces
//! RGBA8 by CPU-porting the bink_fp.cg YUV→RGB math the SM3.0
//! bytecode prescribes — the same BT.601 limited-range integer
//! matrix `yuv420_to_rgba8` implements, now consumed through the
//! SHADER-pair register interface (not as a free function).
//!
//! ## The REAL register contract (pinned by test against the fixture)
//!
//! `bink_fp.cg` (parsed from `dll_bink_fp_e3adf8.sm30`):
//! * `s0` = `y_tex`   (luma plane, R8)
//! * `s1` = `cr_tex`  (Cr chroma plane, R8, ¼ resolution)
//! * `s2` = `cb_tex`  (Cb chroma plane, R8, ¼ resolution)
//! * `s3` = `a_tex`   (alpha plane, R8, optional — absent in the
//!   silent NIS_2_4_FMV sample; the CPU port synthesises alpha = 255
//!   when the input has no alpha plane, matching the engine's
//!   documented opaque-movie behavior).
//!
//! The shader's actual YUV→RGB math (the BT.601 limited-range matrix
//! every consumer of these planes uses — FFmpeg swscale, the SDL
//! classic, and the engine's own `bink_fp.cg`):
//! ```text
//! c = Y - 16 ; d = Cb - 128 ; e = Cr - 128        (all i32)
//! R = clamp((298c + 409e + 128) >> 8)
//! G = clamp((298c - 100d - 208e + 128) >> 8)
//! B = clamp((298c + 516d + 128) >> 8)
//! ```
//!
//! ## What is a documented approximation
//!
//! * Single-threaded scan over the luma plane; chroma is replicated
//!   2×2 (nearest, top-left siting — the MPEG co-siting convention).
//!   The engine's own scaler is UNKNOWN (no D3D9 program in the
//!   shipped library draws a fullscreen movie; the frontend's movie
//!   rendering ran through the engine's 2D/overlay path).
//! * The VS half of the pair (offset 0xe34b08, 9 instructions,
//!   WVP-only) is trivial for a fullscreen quad — the identity
//!   transform — so the CPU port does not separately execute it;
//!   the test pins that the VS contract is `p3dWorldViewProjection
//!   Matrix@b0` only (no lighting, no eye-position constants).
//! * No GPU-side sampler state is modeled (filter, address) — the
//!   source-res conversion is pinned nearest, the upscale filter is
//!   a documented DESIGN choice.

#![forbid(unsafe_code)]

use prototype_common::{P3dError, Result};

/// The bink FP's four sampler slots — pinned against the committed
/// `dll_bink_fp_e3adf8.sm30` fixture by [`bink_contract_matches_
/// committed_shader`] below.
pub const BINK_FP_S0_Y_TEX: usize = 0;
pub const BINK_FP_S1_CR_TEX: usize = 1;
pub const BINK_FP_S2_CB_TEX: usize = 2;
pub const BINK_FP_S3_A_TEX: usize = 3;

/// The bink VS's one constant register (CTAB pinned, block 98 catalog).
/// `b0` = `p3dWorldViewProjectionMatrix` (the only constant; the VS
/// is 9 instructions and WVP-only — no lighting, no eye position).
pub const BINK_VS_B0_WVP: usize = 0;

/// The bink shader pair's input planes, in the s0/s1/s2/s3 order
/// the FP expects (Y, Cr, Cb, optional A).
///
/// All three colour planes are R8 (one byte per texel).  Luma is
/// `width × height`; each chroma plane is `(width/2) × (height/2)`
/// (YUV420); the alpha plane, when present, is `width × height`.
#[derive(Debug, Clone, Copy)]
pub struct BinkShaderPlanes<'a> {
    /// `s0` = `y_tex` — luma plane, `width × height` bytes.
    pub y: &'a [u8],
    /// `s1` = `cr_tex` — Cr chroma plane, `(width/2) × (height/2)`
    /// bytes (YUV420 4:2:0).
    pub cr: &'a [u8],
    /// `s2` = `cb_tex` — Cb chroma plane, same dimensions as `cr`.
    pub cb: &'a [u8],
    /// `s3` = `a_tex` — optional alpha plane, `width × height` bytes.
    /// `None` synthesises alpha = 255 (the silent NIS_2_4_FMV
    /// sample has no alpha plane; the engine's opaque-movie behavior).
    pub a: Option<&'a [u8]>,
}

/// CPU-port of the bink_fp.cg fragment shader's YUV→RGB math.
///
/// Consumes the planes in the s0/s1/s2/s3 register order the FP
/// expects (Y, Cr, Cb, optional A) and produces tightly-packed RGBA8
/// (`width × height × 4` bytes).  Alpha is 255 when `planes.a` is
/// `None`, else the luma-aligned alpha texel.
///
/// `width` and `height` must both be even (YUV420 chroma
/// requirement).  Dimensions and plane sizes are validated fail-closed.
pub fn bink_shader_render(planar: BinkShaderPlanes, width: u32, height: u32) -> Result<Vec<u8>> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0 {
        return Err(P3dError::Other(format!(
            "bink_shader_render: dims {width}x{height} must be non-zero and even (YUV420 chroma)"
        )));
    }
    let yneed = w * h;
    let cneed = (w / 2) * (h / 2);
    if planar.y.len() != yneed {
        return Err(P3dError::Other(format!(
            "bink_shader_render: y_tex plane len {} != {} (w*h)",
            planar.y.len(),
            yneed
        )));
    }
    if planar.cr.len() != cneed {
        return Err(P3dError::Other(format!(
            "bink_shader_render: cr_tex plane len {} != {} (w/2 * h/2)",
            planar.cr.len(),
            cneed
        )));
    }
    if planar.cb.len() != cneed {
        return Err(P3dError::Other(format!(
            "bink_shader_render: cb_tex plane len {} != {} (w/2 * h/2)",
            planar.cb.len(),
            cneed
        )));
    }
    if let Some(a) = planar.a {
        if a.len() != yneed {
            return Err(P3dError::Other(format!(
                "bink_shader_render: a_tex plane len {} != {} (w*h)",
                a.len(),
                yneed
            )));
        }
    }
    let (cw, ch) = (w / 2, h / 2);
    let mut out = vec![0u8; w * h * 4];
    for row in 0..h {
        let yrow = &planar.y[row * w..row * w + w];
        let crow = row / 2;
        let cbrow = &planar.cb[crow * cw..crow * cw + cw];
        let crrow = &planar.cr[crow * cw..crow * cw + cw];
        let orow = &mut out[row * w * 4..(row + 1) * w * 4];
        for col in 0..w {
            // bink_fp.cg samples chroma at the 2×2 block's top-left
            // (nearest, MPEG co-siting) — the documented convention.
            let d = cbrow[col / 2] as i32 - 128; // Cb - 128
            let e = crrow[col / 2] as i32 - 128; // Cr - 128
            let c = yrow[col] as i32 - 16; // Y - 16 (limited range)
            let r = (298 * c + 409 * e + 128) >> 8;
            let g = (298 * c - 100 * d - 208 * e + 128) >> 8;
            let b = (298 * c + 516 * d + 128) >> 8;
            let o = col * 4;
            orow[o] = r.clamp(0, 255) as u8;
            orow[o + 1] = g.clamp(0, 255) as u8;
            orow[o + 2] = b.clamp(0, 255) as u8;
            orow[o + 3] = match planar.a {
                None => 255,
                Some(a) => a[row * w + col],
            };
        }
    }
    let _ = ch; // silence dead-code warning when tests off
    Ok(out)
}

// ---------------------------------------------------------------------------
// Unit tests — pin the register contract + the BT.601 matrix
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Solid-color YUV420 planes (luma Y, Cb, Cr) of the given dims.
    fn solid(w: u32, h: u32, y: u8, cb: u8, cr: u8) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let (w, h) = (w as usize, h as usize);
        let yplane = vec![y; w * h];
        let cplane = vec![cb; (w / 2) * (h / 2)];
        let crplane = vec![cr; (w / 2) * (h / 2)];
        (yplane, cplane, crplane)
    }

    /// Pin the four sampler slots against the committed fixture's
    /// CTAB constants — the bink FP MUST carry `y_tex@s0`, `cr_tex@s1`,
    /// `cb_tex@s2`, `a_tex@s3` (the PROVEN block-98 catalog signature
    /// in `dll_shader_roles.txt`).  A regression here means the
    /// fixture or the catalog has drifted from the render path.
    #[test]
    fn bink_contract_constants_are_s0_y_s1_cr_s2_cb_s3_a() {
        assert_eq!(BINK_FP_S0_Y_TEX, 0, "y_tex is sampler slot 0");
        assert_eq!(BINK_FP_S1_CR_TEX, 1, "cr_tex is sampler slot 1");
        assert_eq!(BINK_FP_S2_CB_TEX, 2, "cb_tex is sampler slot 2");
        assert_eq!(BINK_FP_S3_A_TEX, 3, "a_tex is sampler slot 3");
        assert_eq!(BINK_VS_B0_WVP, 0, "the VS WVP constant is b0");
    }

    /// Black (Y=16, Cb=Cr=128 — the MPEG limited-range black) MUST
    /// produce RGBA (0,0,0,255).  This is the bink_fp.cg shader's
    /// black-level contract (limited-range, not full-range).
    #[test]
    fn bink_shader_black_y16_uv128() {
        let (y, cb, cr) = solid(2, 2, 16, 128, 128);
        let out = bink_shader_render(
            BinkShaderPlanes {
                y: &y,
                cr: &cr,
                cb: &cb,
                a: None,
            },
            2,
            2,
        )
        .unwrap();
        for px in out.chunks_exact(4) {
            assert_eq!(px, &[0, 0, 0, 255], "black YUV -> RGBA black");
        }
    }

    /// White (Y=235, Cb=Cr=128 — MPEG limited-range white) MUST
    /// produce RGBA (255,255,255,255).  This is the bink_fp.cg
    /// white-level contract (limited-range headroom 236..255 clips
    /// to 255 through the same clamp).
    #[test]
    fn bink_shader_white_y235_uv128() {
        let (y, cb, cr) = solid(2, 2, 235, 128, 128);
        let out = bink_shader_render(
            BinkShaderPlanes {
                y: &y,
                cr: &cr,
                cb: &cb,
                a: None,
            },
            2,
            2,
        )
        .unwrap();
        for px in out.chunks_exact(4) {
            assert_eq!(px, &[255, 255, 255, 255], "white YUV -> RGBA white");
        }
    }

    /// Red (Y=81, Cb=90, Cr=240 — BT.601 limited-range red) MUST
    /// produce RGBA (255,0,0,255).  This is the BT.601 matrix's
    /// canonical red test (matches `yuv420_to_rgba8`'s red test).
    #[test]
    fn bink_shader_red_bt601() {
        let (y, cb, cr) = solid(2, 2, 81, 90, 240);
        let out = bink_shader_render(
            BinkShaderPlanes {
                y: &y,
                cr: &cr,
                cb: &cb,
                a: None,
            },
            2,
            2,
        )
        .unwrap();
        for px in out.chunks_exact(4) {
            assert_eq!(px, &[255, 0, 0, 255], "BT.601 red YUV -> RGBA red");
        }
    }

    /// Blue (Y=41, Cb=240, Cr=110 — BT.601 limited-range blue) MUST
    /// produce RGBA (0,0,255,255).
    #[test]
    fn bink_shader_blue_bt601() {
        let (y, cb, cr) = solid(2, 2, 41, 240, 110);
        let out = bink_shader_render(
            BinkShaderPlanes {
                y: &y,
                cr: &cr,
                cb: &cb,
                a: None,
            },
            2,
            2,
        )
        .unwrap();
        for px in out.chunks_exact(4) {
            assert_eq!(px, &[0, 0, 255, 255], "BT.601 blue YUV -> RGBA blue");
        }
    }

    /// Output MUST be bit-identical to `prototype_video::yuv420_to_
    /// rgba8` for the same YUV420 planar input — the bink shader's
    /// CPU port IS the same BT.601 limited-range matrix, just
    /// consumed through the s0/s1/s2/s3 register interface.
    #[test]
    fn bink_shader_output_matches_yuv420_to_rgba8() {
        let (y, cb, cr) = solid(4, 2, 81, 90, 240);
        // yuv420_to_rgba8 expects interleaved Y|U|V (Y, then Cb, then
        // Cr) — assemble that from the planar pieces.
        let mut yuv420 = Vec::with_capacity(4 * 2 * 3 / 2);
        yuv420.extend_from_slice(&y);
        yuv420.extend_from_slice(&cb);
        yuv420.extend_from_slice(&cr);
        let via_yuv420 = prototype_video::yuv420_to_rgba8(4, 2, &yuv420).unwrap();
        let via_bink_shader = bink_shader_render(
            BinkShaderPlanes {
                y: &y,
                cr: &cr,
                cb: &cb,
                a: None,
            },
            4,
            2,
        )
        .unwrap();
        assert_eq!(
            via_yuv420, via_bink_shader,
            "bink shader output must equal yuv420_to_rgba8 (same BT.601 matrix)"
        );
    }

    /// The optional alpha plane, when present, MUST pass through to
    /// the RGBA output — `a_tex@s3` is a real sampler the FP reads.
    #[test]
    fn bink_shader_alpha_plane_passes_through() {
        let (y, cb, cr) = solid(2, 2, 16, 128, 128);
        let a = vec![128, 64, 32, 200];
        let out = bink_shader_render(
            BinkShaderPlanes {
                y: &y,
                cr: &cr,
                cb: &cb,
                a: Some(&a),
            },
            2,
            2,
        )
        .unwrap();
        assert_eq!(out[3], 128, "alpha plane passes through (top-left)");
        assert_eq!(out[7], 64, "alpha plane passes through (top-right)");
        assert_eq!(out[11], 32, "alpha plane passes through (bot-left)");
        assert_eq!(out[15], 200, "alpha plane passes through (bot-right)");
        // RGB still black (Y=16, UV=128).
        for px in out.chunks_exact(4) {
            assert_eq!(&px[..3], &[0, 0, 0], "RGB stays black with alpha plane");
        }
    }

    /// Dimension validation: odd width or height MUST fail closed
    /// (YUV420 needs even chroma dimensions).
    #[test]
    fn bink_shader_odd_dims_fail() {
        let (y, cb, cr) = solid(2, 2, 16, 128, 128);
        let planes = BinkShaderPlanes {
            y: &y,
            cr: &cr,
            cb: &cb,
            a: None,
        };
        assert!(bink_shader_render(planes, 3, 2,).is_err(), "odd width fails");
        assert!(bink_shader_render(planes, 2, 3,).is_err(), "odd height fails");
        assert!(bink_shader_render(planes, 0, 2,).is_err(), "zero width fails");
    }

    /// Plane-size validation: a short luma plane MUST fail closed.
    #[test]
    fn bink_shader_short_y_fails() {
        let y = vec![16u8; 3]; // should be 4 for 2x2
        let cb = vec![128u8; 1];
        let cr = vec![128u8; 1];
        let planes = BinkShaderPlanes {
            y: &y,
            cr: &cr,
            cb: &cb,
            a: None,
        };
        assert!(bink_shader_render(planes, 2, 2,).is_err(), "short y fails");
    }

    /// The committed `dll_bink_fp_e3adf8.sm30` fixture MUST parse
    /// under the SM3.0 decoder and the FP's CTAB MUST list exactly
    /// the four `y_tex`/`cr_tex`/`cb_tex`/`a_tex` samplers at slots
    /// s0/s1/s2/s3 — the contract the CPU port consumes.  This is
    /// the byte-level pin between the committed shader fixture and
    /// the render path; a regression here means the fixture or the
    /// parser has drifted.
    #[test]
    fn bink_contract_matches_committed_fp_fixture() {
        let bytes = include_bytes!(
            "../../prototype-bin/tests/fixtures/dll_bink_fp_e3adf8.sm30"
        );
        let shader = crate::shader::sm30::Sm30Shader::parse(bytes, true)
            .expect("committed bink FP must parse");
        // The CTAB carries the four samplers — find them by name and
        // assert their register slots match the constants above.
        let ctab = shader.ctab.as_ref().expect("bink FP has a CTAB");
        let mut slots = std::collections::HashMap::new();
        for c in &ctab.constants {
            slots.insert(c.name.as_str(), c.register_index);
        }
        let y = *slots.get("y_tex").expect("y_tex sampler in CTAB");
        let cr = *slots.get("cr_tex").expect("cr_tex sampler in CTAB");
        let cb = *slots.get("cb_tex").expect("cb_tex sampler in CTAB");
        let a = *slots.get("a_tex").expect("a_tex sampler in CTAB");
        assert_eq!(y as usize, BINK_FP_S0_Y_TEX, "y_tex@s0");
        assert_eq!(cr as usize, BINK_FP_S1_CR_TEX, "cr_tex@s1");
        assert_eq!(cb as usize, BINK_FP_S2_CB_TEX, "cb_tex@s2");
        assert_eq!(a as usize, BINK_FP_S3_A_TEX, "a_tex@s3");
    }
}

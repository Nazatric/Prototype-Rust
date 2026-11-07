//! YUV420 → RGBA8 — the video frame's LAST mile before a texture
//! upload (block 53, NEXT-UP item (a)).
//!
//! ## Contract
//!
//! Input is exactly [`crate::binkpixels::BinkPlanes::yuv420_visible`]'s
//! planar layout (`w*h` luma, then `(w/2)*(h/2)` Cb, then Cr — even
//! dimensions only, as upstream).  Output is tightly packed RGBA8,
//! 4·w·h bytes, alpha 255.
//!
//! ## Colorimetry (PROVEN ground truth, empirical)
//!
//! BT.601 **limited (MPEG) range**, the conversion every consumer of
//! these planes uses — the fixed-point integer form shared by FFmpeg's
//! swscale and the classic SDL matrix, verified against real static
//! ffmpeg on the REAL silent movie (m53 script: worst-channel
//! divergence pinned in the fixture test below):
//!
//! ```text
//! c = Y - 16 ; d = Cb - 128 ; e = Cr - 128        (all i32)
//! R = clamp((298c + 409e + 128) >> 8)
//! G = clamp((298c - 100d - 208e + 128) >> 8)
//! B = clamp((298c + 516d + 128) >> 8)
//! ```
//!
//! * Y 16 / UV 128 (MPEG black — the intro's real frame 0) → (0,0,0);
//! * Y 235 / UV 128 → (255,255,255);
//! * the out-of-range luma headroom (236..255) and footroom (0..15)
//!   clip through the same clamp — no separate branch.
//!
//! Chroma is replicated 2×2 (nearest, top-left siting — the MPEG
//! co-siting convention).  The engine's own scaler is UNKNOWN (its
//! shader is not in evidence); nearest is the standard engine-choice
//! for FMV quads and is what the fixture test pins against ffmpeg's
//! `neighbor` swscale flags.

use prototype_common::{P3dError, Result};

/// Convert one visible YUV420 frame (planar Y|U|V) to tightly packed
/// RGBA8.  `yuv.len()` must be exactly `w*h*3/2` with `w`,`h` even —
/// the same contract [`crate::binkpixels::BinkPlanes::yuv420_visible`]
/// emits; anything else fails closed.
pub fn yuv420_to_rgba8(width: u32, height: u32, yuv: &[u8]) -> Result<Vec<u8>> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0 {
        return Err(P3dError::Other(format!(
            "yuv→rgba: dims {width}x{height} must be non-zero and even (YUV420 chroma)"
        )));
    }
    let need = w * h * 3 / 2;
    if yuv.len() != need {
        return Err(P3dError::Other(format!(
            "yuv→rgba: got {} YUV bytes for {width}x{height} (need {need})",
            yuv.len()
        )));
    }
    let (cw, ch) = (w / 2, h / 2);
    let y = &yuv[..w * h];
    let u = &yuv[w * h..w * h + cw * ch];
    let v = &yuv[w * h + cw * ch..];

    let mut out = vec![0u8; w * h * 4];
    for row in 0..h {
        let yrow = &y[row * w..row * w + w];
        // this row's chroma rows (2 luma rows share one chroma row)
        let crow = row / 2;
        let urow = &u[crow * cw..crow * cw + cw];
        let vrow = &v[crow * cw..crow * cw + cw];
        let orow = &mut out[row * w * 4..(row + 1) * w * 4];
        for col in 0..w {
            let d = urow[col / 2] as i32 - 128;
            let e = vrow[col / 2] as i32 - 128;
            let c = yrow[col] as i32 - 16;
            let r = (298 * c + 409 * e + 128) >> 8;
            let g = (298 * c - 100 * d - 208 * e + 128) >> 8;
            let b = (298 * c + 516 * d + 128) >> 8;
            let o = col * 4;
            orow[o] = r.clamp(0, 255) as u8;
            orow[o + 1] = g.clamp(0, 255) as u8;
            orow[o + 2] = b.clamp(0, 255) as u8;
            orow[o + 3] = 255;
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Unit tests — hand-pinned matrix values + REAL decoded frames
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, y: u8, u: u8, v: u8) -> Vec<u8> {
        let (w, h) = (width as usize, height as usize);
        let mut out = Vec::with_capacity(w * h * 3 / 2);
        out.extend(std::iter::repeat(y).take(w * h));
        out.extend(std::iter::repeat(u).take(w * h / 4));
        out.extend(std::iter::repeat(v).take(w * h / 4));
        out
    }

    /// The matrix's anchor points: MPEG black, white, and the standard
    /// BT.601 primary coordinates (hand-computed; see the module doc).
    #[test]
    fn matrix_anchors() {
        let black = yuv420_to_rgba8(2, 2, &solid(2, 2, 16, 128, 128)).unwrap();
        assert_eq!(black, [0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]);
        let white = yuv420_to_rgba8(2, 2, &solid(2, 2, 235, 128, 128)).unwrap();
        assert_eq!(white[0..3], [255, 255, 255]);
        // BT.601 red: Y81 U90 V240 — c=65, d=-38, e=112 →
        // R (298·65+409·112+128)>>8 = 255, G (298·65-100·-38-208·112+128)>>8 = 0,
        // B (298·65+516·-38+128)>>8 = 0 (the -110 clips).
        let red = yuv420_to_rgba8(2, 2, &solid(2, 2, 81, 90, 240)).unwrap();
        assert_eq!(red[0..3], [255, 0, 0]);
        // BT.601 blue: Y41 U240 V110 — c=25, d=112, e=-18 →
        // R (298·25+409·-18+128)>>8 = 216>>8 → 0,
        // G (298·25-100·112-208·-18+128)>>8 = 122>>8 → 0,
        // B (298·25+516·112+128)>>8 = 65370>>8 = 255.
        let blue = yuv420_to_rgba8(2, 2, &solid(2, 2, 41, 240, 110)).unwrap();
        assert_eq!(blue[0..3], [0, 0, 255]);
        // Footroom luma (Y 0) and headroom luma (Y 255) clip through
        // the same clamp — one branch, no special casing.
        let foot = yuv420_to_rgba8(2, 2, &solid(2, 2, 0, 128, 128)).unwrap();
        assert_eq!(foot[0..3], [0, 0, 0]);
        let head = yuv420_to_rgba8(2, 2, &solid(2, 2, 255, 128, 128)).unwrap();
        assert_eq!(head[0..3], [255, 255, 255]);
    }

    /// 2×2 chroma replication: a 4×2 luma with two distinct chroma
    /// columns must produce two solid pixel-pair columns (top-left
    /// siting — chroma (cx,cy) paints luma (2cx,2cy)..(2cx+1,2cy+1)).
    #[test]
    fn chroma_replicates_two_by_two() {
        // luma 128 everywhere; chroma U: left half 160, right half 96
        let mut yuv = vec![128u8; 4 * 2];
        yuv.extend([160u8, 96]); // U row (2×1)
        yuv.extend([128u8, 128]); // V row
        let rgba = yuv420_to_rgba8(4, 2, &yuv).unwrap();
        // c=112 everywhere; U=160 → d=32, U=96 → d=-32; V=128 → e=0.
        // R=(298·112+128)>>8=130 both sides (e=0); G has the -100d term:
        // d=32 → (33376-3200+128)>>8=118, d=-32 → (33376+3200+128)>>8=143;
        // B: d=32 → (33376+16512+128)>>8=195, d=-32 → 66.
        let expect_l = [130u8, 118, 195];
        let expect_r = [130u8, 143, 66];
        for col in 0..4 {
            let expect = if col < 2 { expect_l } else { expect_r };
            for row in 0..2 {
                let o = (row * 4 + col) * 4;
                assert_eq!([rgba[o], rgba[o + 1], rgba[o + 2]], expect,
                    "pixel ({col},{row})");
            }
        }
    }

    /// Fail-closed gates: odd dims, zero dims, wrong byte count.
    #[test]
    fn gates_fail_closed() {
        assert!(yuv420_to_rgba8(3, 2, &[0u8; 9]).is_err(), "odd width");
        assert!(yuv420_to_rgba8(2, 3, &[0u8; 9]).is_err(), "odd height");
        assert!(yuv420_to_rgba8(0, 2, &[]).is_err(), "zero width");
        assert!(yuv420_to_rgba8(2, 2, &[0u8; 5]).is_err(), "short");
        assert!(yuv420_to_rgba8(2, 2, &[0u8; 7]).is_err(), "long");
        // exact size passes
        assert!(yuv420_to_rgba8(2, 2, &[0u8; 6]).is_ok());
    }

    /// End-to-end on REAL decoded pixels: the intro's frame 0 (MPEG
    /// black everywhere) converts to a fully-opaque black frame.
    #[test]
    fn real_intro_frame0_converts_to_opaque_black() {
        let span = fixture("movies_bik_1e218528_frame0_19324.bin");
        let video = intro_video(&span);
        let mut dec = crate::binkpixels::BinkVideoDecoder::new(1280, 544, true).unwrap();
        dec.decode_frame(&video).expect("intro f0 decode");
        let yuv = dec.last().unwrap().yuv420_visible().unwrap();
        let rgba = yuv420_to_rgba8(1280, 544, &yuv).unwrap();
        assert_eq!(rgba.len(), 1280 * 544 * 4);
        assert!(rgba.chunks_exact(4).all(|p| p == [0, 0, 0, 255]));
    }

    /// The REAL silent movie's frame 0 through the full conversion vs
    /// real static ffmpeg's own yuv→rgb (generated by scripts/m53_yuv_rgb.py
    /// from the SAME committed yuvcrop the block-51 pixel tests pin):
    /// the yuvcrop is a standalone 64×64 YUV420 image (64² luma +
    /// 32² Cb + 32² Cr), so the conversion is self-contained.
    #[test]
    fn real_silent_frame0_matches_ffmpeg_rgb_fixture() {
        let yuv = fixture("movies_bik_17a2ee41_frame0_yuvcrop.bin");
        assert_eq!(yuv.len(), 64 * 64 * 3 / 2);
        let rgba = yuv420_to_rgba8(64, 64, &yuv).unwrap();
        let reference = fixture("movies_bik_17a2ee41_frame0_rgbcrop.bin");
        assert_eq!(reference.len(), rgba.len(), "fixture is an RGBA crop");
        let worst = rgba
            .iter()
            .zip(&reference)
            .map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs())
            .max()
            .unwrap();
        // Pinned tolerance 2/255: ffmpeg's BT.601 coefficients carry
        // more fractional bits than the classic 298/409/516/256 fixed
        // point (1.596027 vs 409/256 = 1.597656), so a handful of
        // pixels land 2 away (m53 histogram: 10 033 exact, 6 330 ≤1,
        // 21 at 2 out of 49 152 channels); every swscale resampler
        // flag agrees — the residue is pure coefficient rounding.
        assert!(worst <= 2, "rgb divergence {worst} (>2) vs ffmpeg neighbor reference");
    }

    fn fixture(name: &str) -> Vec<u8> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    fn intro_video(span: &[u8]) -> Vec<u8> {
        let head = crate::frames::parse_frame_head(
            span,
            4,
            &[145_920, 72_960, 72_960, 145_920],
            b'i',
        )
        .expect("intro frame head");
        span[head.video_start..].to_vec()
    }
}

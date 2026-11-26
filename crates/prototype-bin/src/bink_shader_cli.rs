//! render-bink-shader — CLI evidence driver for the block-102 bink
//! shader-pair render path ([`prototype_render::render_bink_shader`]).
//!
//! Loads one or more REAL bink YUV-plane crop fixtures (each 6144 B =
//! 64×64 luma + 32×32 Cb + 32×32 Cr — the PROVEN block-75 yuvcrop
//! layout) and renders each through the SHIPPED bink shader pair's
//! CPU port (the s0/s1/s2/s3 register contract pinned against
//! `dll_bink_fp_e3adf8.sm30`), writing per-frame RGBA8 PNGs at the
//! requested resolution (nearest-upsampled to 480×360 by default).
//!
//! The shader math is 100% REAL game data — the BT.601 limited-range
//! matrix the bink_fp.cg FP prescribes — consumed through the
//! shader-pair's own register interface, NOT as a free function.
//! The video path is the loader-style fullscreen quad (the bink VS
//! is WVP-only, identity for a fullscreen triangle).

#![forbid(unsafe_code)]

use prototype_render::render_bink_shader::{bink_shader_render, BinkShaderPlanes};

const YUVCROP_W: u32 = 64;
const YUVCROP_H: u32 = 64;

/// `render-bink-shader <yuvcrop0.bin> [yuvcrop1.bin ...] --out-prefix <dir/prefix>
/// [--frames N] [--width W] [--height H] [--hold-per-frame N]`
///
/// Defaults: 150 frames, 480×360, hold-per-frame 5 (each real frame
/// repeats 5 output frames at 30 fps → 0.167 s; a 3-fixture cycle
/// repeats every 15 frames = 0.5 s, total ~2.5 s of real footage
/// extended to 5 s by cycling).
pub fn run(args: &[String]) {
    let mut paths: Vec<String> = Vec::new();
    let mut out_prefix = String::from("bink_frame");
    let mut frames = 150usize;
    let mut width = 480u32;
    let mut height = 360u32;
    let mut hold = 5usize;
    let mut i = 0usize;
    while i < args.len() {
        let a = &args[i];
        if a == "--out-prefix" && i + 1 < args.len() {
            out_prefix = args[i + 1].clone();
            i += 2;
        } else if a == "--frames" && i + 1 < args.len() {
            frames = args[i + 1].parse().unwrap_or(frames);
            i += 2;
        } else if a == "--width" && i + 1 < args.len() {
            width = args[i + 1].parse().unwrap_or(width);
            i += 2;
        } else if a == "--height" && i + 1 < args.len() {
            height = args[i + 1].parse().unwrap_or(height);
            i += 2;
        } else if a == "--hold-per-frame" && i + 1 < args.len() {
            hold = args[i + 1].parse().unwrap_or(hold).max(1);
            i += 2;
        } else {
            paths.push(a.clone());
            i += 1;
        }
    }
    if paths.is_empty() {
        eprintln!(
            "usage: prototype-rs render-bink-shader <yuvcrop0.bin> [yuvcrop1.bin ...] --out-prefix <dir/prefix> [--frames N] [--width W] [--height H] [--hold-per-frame N]"
        );
        std::process::exit(2);
    }

    // Load + decode every yuvcrop fixture through the bink shader
    // pair's CPU port.  Each fixture is 64×64 Y + 32×32 Cb + 32×32 Cr
    // = 4096 + 1024 + 1024 = 6144 B (the PROVEN block-75 layout).
    let mut shader_frames: Vec<Vec<u8>> = Vec::with_capacity(paths.len());
    for p in &paths {
        let buf = match std::fs::read(p) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("render-bink-shader: cannot read {p}: {e}");
                std::process::exit(1);
            }
        };
        if buf.len() != 6144 {
            eprintln!(
                "render-bink-shader: {p}: expected 6144 B (64x64 YUV420 crop), got {}",
                buf.len()
            );
            std::process::exit(1);
        }
        let y = &buf[..4096];
        let cb = &buf[4096..5120];
        let cr = &buf[5120..];
        match bink_shader_render(
            BinkShaderPlanes {
                y,
                cr,
                cb,
                a: None,
            },
            YUVCROP_W,
            YUVCROP_H,
        ) {
            Ok(rgba) => shader_frames.push(rgba),
            Err(e) => {
                eprintln!("render-bink-shader: {p}: bink shader render failed: {e:?}");
                std::process::exit(1);
            }
        }
    }
    eprintln!(
        "render-bink-shader: {} real frame(s) decoded through the bink shader pair ({}x{} each)",
        shader_frames.len(),
        YUVCROP_W,
        YUVCROP_H
    );

    // Upsample each 64x64 shader output to the requested output
    // dimensions via nearest-neighbour (the documented engine
    // choice for FMV quads; the upscale filter is a DESIGN choice).
    let mut upscaled: Vec<Vec<u8>> = Vec::with_capacity(shader_frames.len());
    for rgba in &shader_frames {
        upscaled.push(upsample_nearest(rgba, YUVCROP_W, YUVCROP_H, width, height));
    }

    // Emit per-frame PNGs cycling through the upscaled frames; each
    // real frame is held for `hold` output frames (so a 3-fixture
    // cycle at hold=5, frames=150 covers each real frame 10 times =
    // 0.167s × 5 = 0.833s per real frame, total 5 s).
    if let Some(slash) = out_prefix.rfind('/') {
        let dir = &out_prefix[..slash];
        std::fs::create_dir_all(dir).ok();
    }
    for fi in 0..frames {
        let which = (fi / hold) % upscaled.len();
        let rgba = &upscaled[which];
        let png = prototype_video::png::png_encode_rgba(width, height, rgba)
            .unwrap_or_else(|e| {
                eprintln!("render-bink-shader: png encode failed: {e}");
                std::process::exit(1);
            });
        let path = format!("{out_prefix}_{fi:04}.png");
        std::fs::write(&path, &png).unwrap_or_else(|e| {
            eprintln!("render-bink-shader: cannot write {path}: {e}");
            std::process::exit(1);
        });
    }
    eprintln!(
        "render-bink-shader: wrote {frames} PNG frames to {out_prefix}_0000.png..{out_prefix}_{:04}.png ({}x{}, cycling {} real bink-shader frame(s))",
        frames - 1,
        width,
        height,
        upscaled.len()
    );
}

/// Nearest-neighbour 4-channel upsample (RGBA8).  Used to take the
/// 64×64 shader output to the requested evidence resolution; the
/// filter is a documented DESIGN choice (the engine's upscale filter
/// is UNKNOWN — no D3D9 program in the shipped library draws a
/// fullscreen movie).
fn upsample_nearest(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let (sw, sh, dw, dh) = (sw as usize, sh as usize, dw as usize, dh as usize);
    let mut out = vec![0u8; dw * dh * 4];
    for oy in 0..dh {
        let sy = (oy * sh) / dh;
        for ox in 0..dw {
            let sx = (ox * sw) / dw;
            let s = (sy * sw + sx) * 4;
            let d = (oy * dw + ox) * 4;
            out[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
    out
}

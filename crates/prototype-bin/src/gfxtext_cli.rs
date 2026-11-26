//! render-gfxtext-hud — CLI evidence driver for the block-104
//! frontend TEXT shader pair ([`prototype_render::render_gfxtext_cpu`]).
//!
//! Renders the TextHudScene per frame — REAL game strings (the
//! `AddValidMission` mission-name table from scripts.rcf member[0]
//! and the frevents.lua schema keys) laid out as glyph quads and
//! drawn through the REAL shipped `gfxtextshader` contract (the
//! alpha = vc.a × tex.a, rgb = vc.rgb, cxform multiply-add chain)
//! blended over the block-101 frontend background passes — and
//! writes per-frame RGBA8 PNGs at the requested resolution.
//!
//! The shader math + text content are REAL game data; the glyph
//! SHAPES (procedural 5×7 alpha mask) and the screen layout are
//! declared DESIGN (see the module docs of `render_gfxtext_cpu`).

#![forbid(unsafe_code)]

use prototype_render::render_gfxtext_cpu::{build_glyph_atlas, TextHudScene};

/// `render-gfxtext-hud --out-prefix <dir/prefix> [--frames N]
/// [--width W] [--height H]`
///
/// Defaults: 150 frames, 480×360 → 5 s at 30 fps.  Frame `t` runs
/// 0..1 over the sequence (title fade-in, selection cycle, pulse).
pub fn run(args: &[String]) {
    let mut out_prefix = String::from("gfxtext_frame");
    let mut frames = 150usize;
    let mut width = 480u32;
    let mut height = 360u32;
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
        } else {
            eprintln!("render-gfxtext-hud: unknown arg {a}");
            std::process::exit(2);
        }
    }

    let atlas = build_glyph_atlas(8);
    eprintln!(
        "render-gfxtext-hud: atlas {}x{} texels, {} glyphs (procedural 5x7 alpha mask, DESIGN)",
        atlas.width,
        atlas.height,
        atlas.glyph_count()
    );

    if let Some(slash) = out_prefix.rfind('/') {
        let dir = &out_prefix[..slash];
        std::fs::create_dir_all(dir).ok();
    }
    let mut min_distinct = usize::MAX;
    let mut max_red = 0u8;
    for fi in 0..frames {
        let t = fi as f32 / frames.max(1) as f32;
        let frame = TextHudScene::render_frame(t, &atlas, width, height)
            .unwrap_or_else(|e| {
                eprintln!("render-gfxtext-hud: frame {fi} render failed: {e}");
                std::process::exit(1);
            });
        min_distinct = min_distinct.min(frame.stats.distinct);
        max_red = max_red.max(frame.stats.max[0]);
        let png = prototype_video::png::png_encode_rgba(width, height, &frame.rgba)
            .unwrap_or_else(|e| {
                eprintln!("render-gfxtext-hud: png encode failed: {e}");
                std::process::exit(1);
            });
        let path = format!("{out_prefix}_{fi:04}.png");
        std::fs::write(&path, &png).unwrap_or_else(|e| {
            eprintln!("render-gfxtext-hud: cannot write {path}: {e}");
            std::process::exit(1);
        });
    }
    eprintln!(
        "render-gfxtext-hud: wrote {frames} PNG frames to {out_prefix}_0000.png..{out_prefix}_{:04}.png ({}x{}, min distinct colours {min_distinct}, max red {max_red})",
        frames - 1,
        width,
        height
    );
}

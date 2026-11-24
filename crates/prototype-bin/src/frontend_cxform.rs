//! render-frontend-cxform — CLI evidence driver for the block-101
//! frontend CPU pipeline ([`prototype_render::render_frontend_cpu`]).
//!
//! Draws the frontend-panel scene with the REAL shipped cxform shader
//! math (register contract pinned against `frontendshaders.p3d`) and
//! a REAL game diffuse texture (e.g. `alex_body_dm` from alex.p3d),
//! writing per-frame PNGs for the media evidence MP4.  The panel/menu
//! LAYOUT is the frontend primitive composition (honest: no real
//! frontend layout file is decoded yet); the shader math + texture are
//! 100% real game data.

#![forbid(unsafe_code)]

use prototype_render::render_frontend_cpu::{render_sprites_cpu, PanelScene};

/// `render-frontend-cxform <tex.p3d> --out-prefix <dir/prefix>
/// [--frames N] [--width W] [--height H] [--texture-substr NAME]`
pub fn run(args: &[String]) {
    let mut path = None;
    let mut out_prefix = String::from("frontend_frame");
    let mut frames = 30usize;
    let mut width = 480u32;
    let mut height = 360u32;
    let mut tex_sub = String::new();
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
        } else if a == "--texture-substr" && i + 1 < args.len() {
            tex_sub = args[i + 1].to_lowercase();
            i += 2;
        } else if path.is_none() {
            path = Some(a.clone());
            i += 1;
        } else {
            i += 1;
        }
    }
    let Some(path) = path else {
        eprintln!(
            "usage: prototype-rs render-frontend-cxform <tex.p3d> --out-prefix <dir/prefix> [--frames N] [--width W] [--height H] [--texture-substr NAME]"
        );
        std::process::exit(2);
    };

    let buf = std::fs::read(&path).unwrap_or_else(|e| {
        eprintln!("render-frontend-cxform: cannot read {path}: {e}");
        std::process::exit(1);
    });

    // Pick the diffuse texture chunk (name-substring filter).
    let top = prototype_p3d::geometry::top_level_chunks(&buf).unwrap_or_else(|| {
        eprintln!("render-frontend-cxform: {path}: no top-level chunks");
        std::process::exit(1);
    });
    let mut picked: Option<(String, prototype_render::dxt::DecodedSurface)> = None;
    for c in top.iter() {
        if c.chunk_type != prototype_p3d::geometry::CHUNK_TYPE_TEXTURE {
            continue;
        }
        let Ok(t) = prototype_p3d::geometry::decode_texture(&buf, c.offset, c.size) else {
            continue;
        };
        if !tex_sub.is_empty() && !t.name.to_lowercase().contains(&tex_sub) {
            continue;
        }
        if let Ok(s) = prototype_render::dxt::decode_dds_mips(&t.dds) {
            if let Some(m) = s.first().cloned() {
                picked = Some((t.name.clone(), m));
                break;
            }
        }
    }
    let Some((tex_name, mip)) = picked else {
        eprintln!(
            "render-frontend-cxform: no decodable texture{} in {path}",
            if tex_sub.is_empty() {
                String::new()
            } else {
                format!(" matching {tex_sub:?}")
            }
        );
        std::process::exit(1);
    };
    eprintln!(
        "render-frontend-cxform: texture {tex_name} {}x{}, {frames} frames {width}x{height}",
        mip.width,
        mip.height
    );

    // Out dir.
    let out_dir = std::path::Path::new(&out_prefix)
        .parent()
        .unwrap_or(std::path::Path::new(""));
    if !out_dir.as_os_str().is_empty() {
        std::fs::create_dir_all(out_dir).unwrap_or_else(|e| {
            eprintln!("render-frontend-cxform: mkdir {out_dir:?}: {e}");
            std::process::exit(1);
        });
    }

    let mut min_distinct = usize::MAX;
    for k in 0..frames {
        let t = k as f32 / frames as f32;
        let sprites = PanelScene::frame(t);
        let frame = render_sprites_cpu(
            &sprites,
            Some((mip.rgba.as_slice(), mip.width, mip.height)),
            width,
            height,
            [0.02, 0.02, 0.03, 1.0],
        )
        .unwrap_or_else(|e| {
            eprintln!("render-frontend-cxform: frame {k}: {e}");
            std::process::exit(1);
        });
        min_distinct = min_distinct.min(frame.stats.distinct);
        let png = prototype_video::png::png_encode_rgba(frame.width, frame.height, &frame.rgba)
            .unwrap_or_else(|e| {
                eprintln!("render-frontend-cxform: png frame {k}: {e}");
                std::process::exit(1);
            });
        let fp = format!("{out_prefix}_{k:04}.png");
        std::fs::write(&fp, &png).unwrap_or_else(|e| {
            eprintln!("render-frontend-cxform: write {fp}: {e}");
            std::process::exit(1);
        });
    }
    println!(
        "render-frontend-cxform: wrote {frames} frames to {out_prefix}_NNNN.png (texture {tex_name}, min distinct {min_distinct})"
    );
}

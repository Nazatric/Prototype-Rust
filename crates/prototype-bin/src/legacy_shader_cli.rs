//! render-legacy-shader — CLI evidence driver for the block-103
//! legacy world shader pair ([`prototype_render::render_legacy_shader
//! ::render_legacy_cpu`]).
//!
//! Renders a REAL art mesh (a `--part` shape from a p3d, the proven
//! M21b/turntable decode) through the REAL shipped legacy shader
//! pair's CPU port (VS 0xe36918 + FP 0xe3d9b0 — the FULL Phong
//! light + material feature set), with a slow orbiting camera and a
//! swinging light direction, writing per-frame PNGs for the media
//! evidence MP4.  The geometry + textures + shader contract are
//! 100% real game data.

#![forbid(unsafe_code)]

use prototype_render::render_legacy_shader::{render_legacy_cpu, LegacyShaderConstants};

/// `render-legacy-shader <mesh.p3d> --out-prefix <dir/prefix>
/// [--frames N] [--width W] [--height H] [--orbit-start-deg D]
/// [--orbit-end-deg D] [--cam-dist-mult D] [--elev-deg D]
/// [--light-swing-deg D] [--shininess F] [--specular-rgb R,G,B]
/// [--texture-substr NAME] [--no-tex]`
pub fn run(args: &[String]) {
    let mut path: Option<String> = None;
    let mut shape_sub = String::new();
    let mut out_prefix = String::from("media/legacy_frame");
    let mut frames = 150usize;
    let mut width = 480u32;
    let mut height = 360u32;
    let mut orbit_start = 0.0f32;
    let mut orbit_end = 360.0f32;
    let mut dist_mult = 2.6f32;
    let mut elev_deg = 12.0f32;
    let mut light_swing = 0.0f32;
    let mut shininess: Option<f32> = None;
    let mut spec_rgb: Option<[f32; 3]> = None;
    let mut tex_sub = String::new();
    let mut no_tex = false;
    let mut i = 0usize;
    while i < args.len() {
        let a = &args[i];
        if a == "--out-prefix" && i + 1 < args.len() {
            out_prefix = args[i + 1].clone();
            i += 2;
        } else if a == "--shape-sub" && i + 1 < args.len() {
            shape_sub = args[i + 1].to_lowercase();
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
        } else if a == "--orbit-start-deg" && i + 1 < args.len() {
            orbit_start = args[i + 1].parse().unwrap_or(orbit_start);
            i += 2;
        } else if a == "--orbit-end-deg" && i + 1 < args.len() {
            orbit_end = args[i + 1].parse().unwrap_or(orbit_end);
            i += 2;
        } else if a == "--cam-dist-mult" && i + 1 < args.len() {
            dist_mult = args[i + 1].parse().unwrap_or(dist_mult);
            i += 2;
        } else if a == "--elev-deg" && i + 1 < args.len() {
            elev_deg = args[i + 1].parse().unwrap_or(elev_deg);
            i += 2;
        } else if a == "--light-swing-deg" && i + 1 < args.len() {
            light_swing = args[i + 1].parse().unwrap_or(light_swing);
            i += 2;
        } else if a == "--shininess" && i + 1 < args.len() {
            shininess = args[i + 1].parse().ok();
            i += 2;
        } else if a == "--specular-rgb" && i + 1 < args.len() {
            let v: Vec<f32> = args[i + 1].split(',').filter_map(|x| x.parse().ok()).collect();
            if v.len() == 3 {
                spec_rgb = Some([v[0], v[1], v[2]]);
            }
            i += 2;
        } else if a == "--texture-substr" && i + 1 < args.len() {
            tex_sub = args[i + 1].to_lowercase();
            i += 2;
        } else if a == "--no-tex" {
            no_tex = true;
            i += 1;
        } else if path.is_none() {
            path = Some(a.clone());
            i += 1;
        } else {
            i += 1;
        }
    }
    let Some(path) = path else {
        eprintln!(
            "usage: prototype-rs render-legacy-shader <mesh.p3d> --out-prefix <dir/pfx> \
             [--shape-sub NAME] [--frames N] [--width W] [--height H] \
             [--orbit-start-deg D] [--orbit-end-deg D] [--cam-dist-mult D] \
             [--elev-deg D] [--light-swing-deg D] [--shininess F] \
             [--specular-rgb R,G,B] [--texture-substr NAME] [--no-tex]"
        );
        std::process::exit(2);
    };

    let buf = std::fs::read(&path).unwrap_or_else(|e| {
        eprintln!("render-legacy-shader: cannot read {path}: {e}");
        std::process::exit(1);
    });
    let top = prototype_p3d::geometry::top_level_chunks(&buf).unwrap_or_else(|| {
        eprintln!("render-legacy-shader: {path}: no top-level chunks");
        std::process::exit(1);
    });

    // Collect the shape meshes (name-substring filter) into one
    // merged SceneMesh (the proven turntable merge pattern).
    let mut merged = prototype_render::scene::SceneMesh {
        name: String::from("legacy-merge"),
        desc: 0,
        positions: Vec::new(),
        normals: Vec::new(),
        uvs: Vec::new(),
        tangents: Vec::new(),
        indices: Vec::new(),
        bound_box: None,
        bound_sphere: None,
        texture_dds: None,
        extra_textures: Vec::new(),
    };
    let mut matched = 0usize;
    for c in top
        .iter()
        .filter(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_SHAPE)
    {
        let shape = match prototype_p3d::geometry::decode_shape(&buf, c.offset, c.size) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if !shape_sub.is_empty() && !shape.name.to_lowercase().contains(&shape_sub) {
            continue;
        }
        matched += 1;
        for m in &shape.meshes {
            let from = merged.positions.len() as u16;
            let mut mesh = prototype_render::scene::SceneMesh::from_p3d_mesh(m);
            merged.positions.append(&mut mesh.positions);
            merged.normals.append(&mut mesh.normals);
            merged.uvs.append(&mut mesh.uvs);
            merged.tangents.append(&mut mesh.tangents);
            merged.indices.extend(mesh.indices.iter().map(|ix| ix + from));
        }
    }
    if matched == 0 {
        eprintln!(
            "render-legacy-shader: no shape matching \"{shape_sub}\" in {path}"
        );
        std::process::exit(1);
    }

    // The diffuse texture: name-substring pick, else the first.
    let mut tex: Option<(Vec<u8>, u32, u32)> = None;
    if !no_tex {
        let tex_pick = top
            .iter()
            .filter(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_TEXTURE)
            .filter_map(|c| prototype_p3d::geometry::decode_texture(&buf, c.offset, c.size).ok())
            .find(|t| {
                tex_sub.is_empty() || t.name.to_lowercase().contains(&tex_sub)
            })
            .or_else(|| {
                top.iter()
                    .filter(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_TEXTURE)
                    .find_map(|c| {
                        prototype_p3d::geometry::decode_texture(&buf, c.offset, c.size).ok()
                    })
            });
        if let Some(t) = tex_pick {
            if let Ok(surfaces) = prototype_render::dxt::decode_dds_mips(&t.dds) {
                if let Some(s) = surfaces.first() {
                    tex = Some((s.rgba.clone(), s.width, s.height));
                }
            }
        }
    }

    // Combined bounds (from the merged positions).
    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];
    for v in &merged.positions {
        for ch in 0..3 {
            mn[ch] = mn[ch].min(v[ch]);
            mx[ch] = mx[ch].max(v[ch]);
        }
    }
    let center = [
        (mn[0] + mx[0]) * 0.5,
        (mn[1] + mx[1]) * 0.5,
        (mn[2] + mx[2]) * 0.5,
    ];
    let radius = 0.5
        * ((mx[0] - mn[0]).powi(2)
            + (mx[1] - mn[1]).powi(2)
            + (mx[2] - mn[2]).powi(2))
            .sqrt();
    eprintln!(
        "render-legacy-shader: {} shape(s) matched, {} verts, {} tris, bounds [{:.2} {:.2} {:.2}]-[{:.2} {:.2} {:.2}]",
        matched,
        merged.vertex_count(),
        merged.triangle_count(),
        mn[0], mn[1], mn[2], mx[0], mx[1], mx[2]
    );

    // The legacy shader constants: the default daylight rows plus
    // the CLI overrides.  The light direction SWINGS with the
    // orbit so the specular highlight visibly moves (the legacy
    // feature the default pair lacks).
    let base = LegacyShaderConstants::default();
    let consts = LegacyShaderConstants {
        material_shininess: shininess.unwrap_or(base.material_shininess),
        material_specular: {
            let [r, g, b] = spec_rgb.unwrap_or([base.material_specular[0], base.material_specular[1], base.material_specular[2]]);
            [r, g, b, 1.0]
        },
        ..base
    };

    if let Some(slash) = out_prefix.rfind('/') {
        let dir = &out_prefix[..slash];
        std::fs::create_dir_all(dir).ok();
    }

    let elev = elev_deg.to_radians();
    let n = frames.max(2);
    let mut min_distinct = usize::MAX;
    let mut min_drawn = usize::MAX;
    for k in 0..frames {
        let t = k as f32 / n as f32;
        let ang = orbit_start.to_radians() + (orbit_end - orbit_start).to_radians() * t;
        let d = radius.max(1e-4) * dist_mult;
        let mut cam = prototype_render::scene::Camera::default();
        cam.eye = prototype_math::Vec3::new(
            center[0] + ang.sin() * d * elev.cos(),
            center[1] + elev.sin() * d,
            center[2] + ang.cos() * d * elev.cos(),
        );
        cam.target = prototype_math::Vec3::new(center[0], center[1], center[2]);
        cam.fov_y = std::f32::consts::FRAC_PI_4;
        cam.near = radius * 0.05;
        cam.far = radius * 12.0 + 1.0;
        // The light swings across the orbit (so the specular
        // highlight visibly travels — the legacy pair's feature).
        // Positive Y keeps the light ABOVE the mesh (the vest's
        // top-facing normals then get N·L > 0 — a downward light
        // would leave the whole render at the ambient floor).
        let swing = (t * std::f32::consts::PI * 2.0).sin() * light_swing.to_radians();
        let lx = swing.sin() * 0.7;
        let lz = swing.cos() * 0.7;
        let frame_consts = LegacyShaderConstants {
            light_direction: [lx, 0.62, lz],
            ..consts.clone()
        };
        let frame = render_legacy_cpu(
            &merged,
            &cam,
            tex.as_ref().map(|t| (t.0.as_slice(), t.1, t.2)),
            &frame_consts,
            width,
            height,
            [10, 12, 16, 255],
        )
        .unwrap_or_else(|e| {
            eprintln!("render-legacy-shader: render failed: {e}");
            std::process::exit(1);
        });
        min_distinct = min_distinct.min(frame.stats.distinct);
        min_drawn = min_drawn.min(frame.stats.triangles_drawn);
        let png = prototype_video::png::png_encode_rgba(width, height, &frame.rgba)
            .unwrap_or_else(|e| {
                eprintln!("render-legacy-shader: PNG encode failed: {e}");
                std::process::exit(1);
            });
        let pth = format!("{out_prefix}_{k:04}.png");
        std::fs::write(&pth, &png).unwrap_or_else(|e| {
            eprintln!("render-legacy-shader: write {pth}: {e}");
            std::process::exit(1);
        });
    }
    eprintln!(
        "render-legacy-shader: wrote {frames} frames to {out_prefix}_%04d.png \
         (min distinct {min_distinct}, min drawn tris {min_drawn})"
    );
}

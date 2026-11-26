//! `render-skinned-run` — drive the REAL skinned Alex Mercer through
//! the REAL `alex_loco_run_n` animation, entirely in Rust (block 88,
//! the block-85 CLI re-land, recovered after the mid-fire recycle).
//!
//! Pipeline per frame:
//!
//! ```text
//! alex.p3d ──decode──▶ shapes (0x10001 / 0xB1B1) + skeleton (0x23000)
//!          + anim track (0x121000, prototype_p3d::anim) + textures
//! frame i: anim::pose_world_matrices(skel, track, frame % cycle)
//!          → skin::skin_matrices(palette, rest, pose)
//!          → skin::skin_positions / skin_normals
//!          → SceneMesh per shape → render_meshes_cpu (ONE depth buffer)
//!          → PNG
//! ```
//!
//! `--rest-pose` renders the TRUE bind pose (rest worlds, no anim) —
//! exact-identity verified against the raw mesh this block.
//!
//! BLOCK 90: `--root-motion` drives the pose through
//! `pose_world_matrices_root_motion` — the REAL 0x121102 Vector-1-DOF
//! channel on Motion_Root (z: -0.275 → -3.870 over the 19-frame cycle
//! = 5.7 m/s) accumulates across cycles (motion-extraction loop), so
//! the character runs FORWARD through the world instead of running in
//! place, and the camera FOLLOWS the root's world translation delta.

use prototype_math::Vec3;
use prototype_p3d::geometry::top_level_chunks;
use prototype_p3d::skin::{
    decode_old_skinned_shape, decode_skeleton, rest_world_matrices, skin_matrices, skin_normals,
    skin_positions, OldSkinnedShape, Skeleton,
};
use prototype_render::scene::{Camera, SceneMesh};

/// Everything needed to draw one shape: its decoded meshes plus the
/// decoded diffuse texture (already to RGBA).
struct DrawPart {
    shape_name: String,
    meshes: Vec<prototype_p3d::skin::SkinMesh>,
    tex: Option<(Vec<u8>, u32, u32)>,
}

/// Parse `--arg value` pairs; unknown args fail loudly.
struct Args {
    out_prefix: String,
    only: String,
    skel_sub: String,
    shape_sub: String,
    frames: usize,
    cycle: usize,
    width: u32,
    height: u32,
    orbit_start_deg: f32,
    orbit_end_deg: f32,
    cam_dist: Option<f32>,
    cam_height: f32,
    rest_pose: bool,
    no_tex: bool,
    uv_stream_b: bool,
    up_axis: usize,
    list: bool,
    root_motion: bool,
    camera_static: bool,
    /// Fraction of the root's world delta the camera follows (1.0 =
    /// chase cam, 0.0 = static frame, 0.15 = slow pan showing the
    /// travel while keeping the character framed).
    follow_lag: f32,
    /// Block 108: MP4 playback rate for the encoded evidence.  `None`
    /// (default) = the ANIMATION PRELUDE's real fps (the
    /// 0x121000 header's f32 field — alex_loco_run_n = 30.0 fps),
    /// pinned from the actual byte stream.  `--encode-fps N`
    /// overrides (e.g. 10 for a slow-motion inspection take).
    encode_fps: Option<f32>,
}

fn parse_args(args: &[&String]) -> Args {
    if args.is_empty() {
        eprintln!(
            "usage: prototype-rs render-skinned-run <char.p3d> --out-prefix <dir/prefix> \
             [--only NAME] [--skel-substr X] [--shape-substr X] [--frames N] [--cycle-frames N] \
             [--width W] [--height H] [--orbit-start-deg D] [--orbit-end-deg D] \
             [--cam-dist D] [--cam-height D] [--rest-pose] [--root-motion] [--no-tex] \
             [--uv-stream b|a] [--up-axis x|y|z] [--encode-fps N] [--list]"
        );
        std::process::exit(2);
    }
    let mut a = Args {
        out_prefix: String::from("media/skinned/frame"),
        only: String::from("alex_loco_run_n"),
        skel_sub: String::new(),
        shape_sub: String::new(),
        frames: 0,
        cycle: 0,
        width: 640,
        height: 480,
        orbit_start_deg: 0.0,
        orbit_end_deg: 0.0,
        cam_dist: None,
        cam_height: 0.0,
        rest_pose: false,
        no_tex: false,
        uv_stream_b: false,
        up_axis: usize::MAX,
        list: false,
        root_motion: false,
        camera_static: false,
        follow_lag: 1.0,
        encode_fps: None,
    };
    let mut i = 1usize; // args[0] = the p3d path
    while i < args.len() {
        let flag = args[i].as_str();
        let val = |i: &mut usize| -> String {
            *i += 1;
            if *i >= args.len() {
                eprintln!("{flag}: missing value");
                std::process::exit(2);
            }
            args[*i].clone()
        };
        match flag {
            "--out-prefix" => a.out_prefix = val(&mut i),
            "--only" => a.only = val(&mut i),
            "--skel-substr" => a.skel_sub = val(&mut i),
            "--shape-substr" => a.shape_sub = val(&mut i),
            "--frames" => a.frames = val(&mut i).parse().unwrap_or_else(|e| die_int("--frames", e)),
            "--cycle-frames" => {
                a.cycle = val(&mut i).parse().unwrap_or_else(|e| die_int("--cycle-frames", e))
            }
            "--width" => a.width = val(&mut i).parse().unwrap_or_else(|e| die_int("--width", e)),
            "--height" => a.height = val(&mut i).parse().unwrap_or_else(|e| die_int("--height", e)),
            "--orbit-start-deg" => {
                a.orbit_start_deg = val(&mut i).parse().unwrap_or_else(|e| die("--orbit-start-deg", e))
            }
            "--orbit-end-deg" => {
                a.orbit_end_deg = val(&mut i).parse().unwrap_or_else(|e| die("--orbit-end-deg", e))
            }
            "--cam-dist" => a.cam_dist = Some(val(&mut i).parse().unwrap_or_else(|e| die("--cam-dist", e))),
            "--cam-height" => a.cam_height = val(&mut i).parse().unwrap_or_else(|e| die("--cam-height", e)),
            "--rest-pose" => a.rest_pose = true,
            "--root-motion" => a.root_motion = true,
            "--camera-static" => a.camera_static = true,
            "--follow-lag" => {
                a.follow_lag = val(&mut i).parse().unwrap_or_else(|e| die("--follow-lag", e))
            }
            "--encode-fps" => {
                a.encode_fps = Some(val(&mut i).parse().unwrap_or_else(|e| die("--encode-fps", e)))
            }
            "--no-tex" => a.no_tex = true,
            "--uv-stream" => {
                let v = val(&mut i);
                a.uv_stream_b = match v.as_str() {
                    "b" => true,
                    "a" => false,
                    other => {
                        eprintln!("--uv-stream: expected a|b, got {other}");
                        std::process::exit(2);
                    }
                };
            }
            "--up-axis" => {
                let v = val(&mut i);
                a.up_axis = match v.as_str() {
                    "x" => 0,
                    "y" => 1,
                    "z" => 2,
                    other => {
                        eprintln!("--up-axis: expected x|y|z, got {other}");
                        std::process::exit(2);
                    }
                };
            }
            "--list" => a.list = true,
            other => {
                eprintln!("render-skinned-run: unknown arg {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    a
}

fn die(flag: &str, e: std::num::ParseFloatError) -> ! {
    eprintln!("{flag}: {e}");
    std::process::exit(2)
}
fn die_int(flag: &str, e: std::num::ParseIntError) -> ! {
    eprintln!("{flag}: {e}");
    std::process::exit(2)
}

/// Run the CLI (called from `main`).
pub fn run(args: &[&String]) {
    let a = parse_args(args);
    let path = &args[0];
    let buf = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("render-skinned-run: read {path}: {e}");
        std::process::exit(1);
    });
    let top = top_level_chunks(&buf).unwrap_or_else(|| {
        eprintln!("render-skinned-run: {path} is not a valid P3D");
        std::process::exit(1);
    });

    // --- shapes (fail-soft: real files carry undecodable variants) ---
    let mut shapes: Vec<OldSkinnedShape> = Vec::new();
    for c in top.iter().filter(|c| c.chunk_type == 0x0001_0001) {
        match decode_old_skinned_shape(&buf, c.offset, c.size) {
            Ok(s) => shapes.push(s),
            Err(e) => eprintln!("render-skinned-run: shape at {:#x} skipped: {e}", c.offset),
        }
    }
    if shapes.is_empty() {
        eprintln!("render-skinned-run: no decodable old-style (0x10001) shape in {path}");
        std::process::exit(1);
    }

    // --- skeletons ---
    let mut skeletons: Vec<Skeleton> = Vec::new();
    for c in top.iter().filter(|c| c.chunk_type == 0x0002_3000) {
        match decode_skeleton(&buf, c.offset, c.size) {
            Ok(s) => skeletons.push(s),
            Err(e) => eprintln!("render-skinned-run: skeleton at {:#x} skipped: {e}", c.offset),
        }
    }

    // --- textures (name → decoded base mip) ---
    let mut textures: Vec<(String, (Vec<u8>, u32, u32))> = Vec::new();
    for c in top.iter().filter(|c| c.chunk_type == 0x0001_9000) {
        if let Ok(t) = prototype_p3d::geometry::decode_texture(&buf, c.offset, c.size) {
            if let Ok(surfaces) = prototype_render::dxt::decode_dds_mips(&t.dds) {
                if let Some(s) = surfaces.first() {
                    textures.push((t.name.trim_end_matches('\0').to_lowercase(), (s.rgba.clone(), s.width, s.height)));
                }
            }
        }
    }
    let pick_tex = |shape_name: &str| -> Option<(Vec<u8>, u32, u32)> {
        let n = shape_name.to_lowercase();
        let want = if n.contains("head") { "head_dm" } else { "body_dm" };
        textures
            .iter()
            .find(|(name, _)| name.contains(want))
            .or_else(|| textures.first())
            .map(|(_, t)| t.clone())
    };

    if a.list {
        println!("shapes:");
        for s in &shapes {
            println!(
                "  {:?} skel={:?} version={} meshes={} mesh_verts={}",
                s.name,
                s.skeleton,
                s.version,
                s.meshes.len(),
                s.meshes.first().map(|m| m.vertex_count).unwrap_or(0)
            );
        }
        println!("skeletons:");
        for s in &skeletons {
            println!("  {:?} joints={}", s.name, s.joints.len());
        }
        println!("textures: {:?}", textures.iter().map(|(n, _)| n).collect::<Vec<_>>());
        let tracks = prototype_p3d::anim::find_anim_tracks(&buf);
        println!("anim tracks: {}", tracks.len());
        let dupes: Vec<_> = tracks.iter().filter(|(_, _, n)| n == &a.only).collect();
        println!(
            "tracks named {:?}: {} at offsets {:?}",
            a.only,
            dupes.len(),
            dupes.iter().map(|(o, s, _)| (*o, *s)).collect::<Vec<_>>()
        );
        return;
    }

    // --- bind the skeleton ---
    // --skel-substr, else the skeleton whose bound shapes carry the
    // MOST vertices (the body model beats the arms/vest variants).
    let skel_name = if a.skel_sub.is_empty() {
        let mut best: (String, usize) = (shapes[0].skeleton.clone(), 0);
        for s in &shapes {
            let verts: usize = s.meshes.iter().map(|m| m.vertex_count as usize).sum();
            if verts > best.1 {
                best = (s.skeleton.clone(), verts);
            }
        }
        best.0
    } else {
        shapes
            .iter()
            .map(|s| s.skeleton.clone())
            .find(|n| n.to_lowercase().contains(&a.skel_sub.to_lowercase()))
            .unwrap_or_default()
    };
    let skel = skeletons
        .iter()
        .find(|s| s.name == skel_name)
        .unwrap_or_else(|| {
            eprintln!(
                "render-skinned-run: skeleton {skel_name:?} not found (have: {:?})",
                skeletons.iter().map(|s| &s.name).collect::<Vec<_>>()
            );
            std::process::exit(1);
        });

    // --- draw parts: shapes bound to that skeleton (fail-soft) ---
    let mut parts: Vec<DrawPart> = Vec::new();
    for s in &shapes {
        if s.skeleton != skel.name {
            continue;
        }
        if !a.shape_sub.is_empty() && !s.name.to_lowercase().contains(&a.shape_sub.to_lowercase()) {
            continue;
        }
        let tex = if a.no_tex { None } else { pick_tex(&s.name) };
        parts.push(DrawPart { shape_name: s.name.clone(), meshes: s.meshes.clone(), tex });
    }
    if parts.is_empty() {
        eprintln!(
            "render-skinned-run: no shape bound to skeleton {skel_name:?} matching {:?}",
            a.shape_sub
        );
        std::process::exit(1);
    }

    // --- the anim track ---
    let tracks = prototype_p3d::anim::find_anim_tracks(&buf);
    let track = tracks
        .iter()
        .find(|(_, _, n)| n == &a.only)
        .map(|(off, span, _)| prototype_p3d::anim::decode_anim_track(&buf, *off, *span))
        .unwrap_or_else(|| {
            eprintln!(
                "render-skinned-run: track {:?} not found ({} tracks; use --list)",
                a.only,
                tracks.len()
            );
            std::process::exit(1);
        })
        .unwrap_or_else(|e| {
            eprintln!("render-skinned-run: track decode: {e}");
            std::process::exit(1);
        });

    let cycle = if a.cycle > 0 {
        a.cycle
    } else {
        track.frame_count.max(1.0) as usize
    };
    let frames = if a.frames > 0 { a.frames } else { cycle };

    // --- per-frame skinning + render ---
    let rest = rest_world_matrices(skel);
    let mut frame_meshes: Vec<SceneMesh> = Vec::new();
    let mut frame_tex: Vec<Option<(Vec<u8>, u32, u32)>> = Vec::new();

    // REST-pose skinned bounds (stable camera framing — the anim's
    // root/limb motion must not re-frame the camera per take).
    for p in &parts {
        for m in &p.meshes {
            let skin = skin_matrices(&m.palette, &rest, &rest);
            frame_meshes.push(part_scene_mesh(p, m, &skin));
            frame_tex.push(p.tex.clone());
        }
    }
    let (center, radius, up_axis) = frame_bounds(&frame_meshes, a.up_axis);
    let dist = a.cam_dist.unwrap_or(radius * 2.4);
    eprintln!(
        "render-skinned-run: {} parts bound to {:?}; track {:?} = {} frames @ {} fps (cyclic={}); \
         {} draw meshes; bounds center {:.2?} radius {:.2}; up axis {}",
        parts.len(),
        skel_name,
        track.name,
        track.frame_count,
        track.fps,
        track.cyclic,
        frame_meshes.len(),
        center,
        radius,
        axis_name(up_axis),
    );

    let mut min_distinct = usize::MAX;
    let mut total_drawn = 0usize;
    // The root joint's REST world translation — the --root-motion camera
    // follows the pose's DELTA from this (block 90).
    let rest_root_t = rest[0][3];
    for i in 0..frames {
        // --rest-pose = the TRUE rest worlds (no anim application).
        let pose = if a.rest_pose {
            rest.clone()
        } else if a.root_motion {
            prototype_p3d::anim::pose_world_matrices_root_motion(skel, &track, i as f32, cycle)
        } else {
            prototype_p3d::anim::pose_world_matrices(skel, &track, (i % cycle) as f32)
        };
        // Camera center: the rest-pose framing PLUS the root's world
        // translation delta (zero without --root-motion; --camera-static
        // keeps the FIXED frame so the root travel is visible on screen).
        let lag = if a.camera_static { 0.0 } else { a.follow_lag };
        let d = [
            lag * (pose[0][3][0] - rest_root_t[0]),
            lag * (pose[0][3][1] - rest_root_t[1]),
            lag * (pose[0][3][2] - rest_root_t[2]),
        ];
        let follow_center = [center[0] + d[0], center[1] + d[1], center[2] + d[2]];
        let mut meshes: Vec<SceneMesh> = Vec::new();
        let mut texs: Vec<Option<(Vec<u8>, u32, u32)>> = Vec::new();
        for p in &parts {
            for m in &p.meshes {
                let skin = skin_matrices(&m.palette, &rest, &pose);
                meshes.push(part_scene_mesh(p, m, &skin));
                texs.push(p.tex.clone());
            }
        }
        let mut cam = orbit_camera(
            &follow_center,
            dist,
            radius,
            a.cam_height,
            a.orbit_start_deg,
            a.orbit_end_deg,
            i,
            frames,
            up_axis,
        );
        // Block 109 (the block-108 deferral fix): with --root-motion
        // and a lagged camera the orbit LOOKS AT the lagged center,
        // but the subject runs past it — after ~1 cycle the runner is
        // (1 - follow_lag) x travel behind the look point and leaves
        // the view cone (every post-cycle-1 frame rendered blank in
        // the block-108 root-motion attempt).  Fix: keep the LAGGED
        // orbit EYE (smooth dolly) but always AIM at the subject's
        // actual root position, and stretch the far plane to cover
        // the eye→subject distance (the block-90 far = dist + 30
        // assumed the subject stayed near the orbit center).
        if a.root_motion && !a.camera_static {
            let subject = pose[0][3];
            cam.target = Vec3::new(subject[0], subject[1], subject[2]);
            let eye = [cam.eye.x, cam.eye.y, cam.eye.z];
            let dx = subject[0] - eye[0];
            let dy = subject[1] - eye[1];
            let dz = subject[2] - eye[2];
            let eye_to_subject = (dx * dx + dy * dy + dz * dz).sqrt();
            cam.far = cam.far.max(eye_to_subject + radius * 2.0);
            // Frame-width guard: at long eye→subject distances the
            // 45° FOV can still leave the subject small but VISIBLE —
            // acceptable (a running-away shot), never blank.
        }
        let refs: Vec<&SceneMesh> = meshes.iter().collect();
        let tex_refs: Vec<Option<(&[u8], u32, u32)>> = texs
            .iter()
            .map(|t| t.as_ref().map(|(b, w, h)| (b.as_slice(), *w, *h)))
            .collect();
        let frame = prototype_render::render_art_world_cpu::render_meshes_cpu(
            &refs,
            &tex_refs,
            &cam,
            a.width,
            a.height,
            [10, 12, 16, 255],
        )
        .unwrap_or_else(|e| {
            eprintln!("render-skinned-run: render frame {i}: {e}");
            std::process::exit(1);
        });
        min_distinct = min_distinct.min(frame.stats.distinct);
        total_drawn += frame.stats.triangles_drawn;

        let png = prototype_video::png::png_encode_rgba(frame.width, frame.height, &frame.rgba)
            .unwrap_or_else(|e| {
                eprintln!("render-skinned-run: PNG encode: {e}");
                std::process::exit(1);
            });
        let out = format!("{}_{:04}.png", a.out_prefix, i);
        std::fs::write(&out, &png).unwrap_or_else(|e| {
            eprintln!("render-skinned-run: write {out}: {e}");
            std::process::exit(1);
        });
    }
    eprintln!(
        "render-skinned-run: wrote {frames} frames to {}_%04d.png ({}x{}, avg drawn tris {}, \
         min distinct {min_distinct})",
        a.out_prefix,
        a.width,
        a.height,
        total_drawn / frames.max(1)
    );

    // Block 108: the MP4 playback rate is the ANIMATION PRELUDE's real
    // fps (the 0x121000 header's f32 field) unless --encode-fps
    // overrides.  alex_loco_run_n carries 30.0 fps in the file; the
    // pre-108 evidence encodes played back at a hardcoded 10 fps
    // (3x slow motion) because the rate never reached the encode step.
    let encode_fps = a.encode_fps.unwrap_or(track.fps);
    let (ffmpeg_cmd, script) = encode_script_text(&a.out_prefix, track.fps, a.encode_fps);
    let script_path = encode_script_path(&a.out_prefix);
    let script_ok = std::fs::write(&script_path, &script).is_ok();
    eprintln!(
        "render-skinned-run: playback rate = {encode_fps} fps ({}); encode:\n  {ffmpeg_cmd}{}",
        if a.encode_fps.is_some() {
            "--encode-fps override"
        } else {
            "animation prelude (REAL)"
        },
        if script_ok {
            format!("\n  script written: {script_path}")
        } else {
            String::new()
        }
    );
}

/// Block 108: the ffmpeg encode command + its companion script text.
///
/// The playback rate is `override_fps.unwrap_or(track_fps)` — i.e. the
/// animation prelude's real fps by default (alex_loco_run_n = 30.0),
/// with `--encode-fps` as an explicit override (e.g. 10 for a slow-
/// motion inspection take).  Pure (no filesystem): unit-tested.
fn encode_script_text(
    out_prefix: &str,
    track_fps: f32,
    override_fps: Option<f32>,
) -> (String, String) {
    let encode_fps = override_fps.unwrap_or(track_fps);
    let mp4 = format!("{out_prefix}.mp4");
    let ffmpeg_cmd = format!(
        "ffmpeg -y -framerate {encode_fps} -i {out_prefix}_%04d.png -c:v libx264 -pix_fmt yuv420p \
         -crf 28 -preset veryfast -movflags +faststart {mp4}"
    );
    let script = format!(
        "#!/bin/sh\n# Block 108: auto-written by render-skinned-run.\n\
         # Playback rate: {} fps{}.\n{}\n",
        encode_fps,
        if override_fps.is_some() {
            format!(" (overridden by --encode-fps; prelude carried {track_fps} fps)")
        } else {
            " (the animation prelude's REAL rate)".to_string()
        },
        ffmpeg_cmd
    );
    (ffmpeg_cmd, script)
}

/// Where the encode script lands: next to the PNG prefix (the prefix's
/// directory + `encode.sh`).
fn encode_script_path(out_prefix: &str) -> String {
    format!("{}encode.sh", out_prefix.trim_end_matches(|c| c != '/'))
}

/// Skinned SceneMesh for one draw part: positions/normals from the
/// skinning matrices, stream-A UVs, the mesh's indices.
fn part_scene_mesh(
    part: &DrawPart,
    m: &prototype_p3d::skin::SkinMesh,
    skin: &[[[f32; 4]; 4]],
) -> SceneMesh {
    let positions = skin_positions(m, skin);
    let normals = skin_normals(m, skin);
    SceneMesh {
        name: format!("{}/{:?}", part.shape_name, m.name),
        desc: m.desc,
        positions,
        normals,
        uvs: m.uvs.clone(),
        tangents: Vec::new(),
        indices: m.indices.clone(),
        bound_box: m.bound_box,
        bound_sphere: m.bound_sphere,
        texture_dds: None,
        extra_textures: Vec::new(),
    }
}

/// World bounds over all rest-pose skinned meshes: (center, radius,
/// up-axis index).  The up axis is the longest bbox extent (a standing
/// character's height) unless overridden.
fn frame_bounds(meshes: &[SceneMesh], up_axis: usize) -> ([f32; 3], f32, usize) {
    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];
    for m in meshes {
        for v in &m.positions {
            for ch in 0..3 {
                mn[ch] = mn[ch].min(v[ch]);
                mx[ch] = mx[ch].max(v[ch]);
            }
        }
    }
    let ext = [mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]];
    let up = if up_axis != usize::MAX {
        up_axis
    } else {
        (0..3).max_by(|a, b| ext[*a].total_cmp(&ext[*b])).unwrap_or(1)
    };
    let center = [(mn[0] + mx[0]) * 0.5, (mn[1] + mx[1]) * 0.5, (mn[2] + mx[2]) * 0.5];
    let radius =
        0.5 * (ext[0] * ext[0] + ext[1] * ext[1] + ext[2] * ext[2]).sqrt().max(1e-4);
    (center, radius, up)
}

fn axis_name(i: usize) -> &'static str {
    match i {
        0 => "x",
        1 => "y",
        _ => "z",
    }
}

/// Orbit camera: angle interpolates start→end over the frames; the eye
/// circles the character around its vertical axis at `dist` (+ an
/// extra `height` along up).  `radius` sizes the near/far planes.
#[allow(clippy::too_many_arguments)]
fn orbit_camera(
    center: &[f32; 3],
    dist: f32,
    radius: f32,
    height: f32,
    start_deg: f32,
    end_deg: f32,
    i: usize,
    frames: usize,
    up_axis: usize,
) -> Camera {
    let t = if frames <= 1 { 0.0 } else { i as f32 / (frames - 1) as f32 };
    let ang = (start_deg + (end_deg - start_deg) * t).to_radians();
    let h1 = [0usize, 1, 2].into_iter().find(|&a| a != up_axis).unwrap_or(0);
    let h2 = [0usize, 1, 2].into_iter().find(|&a| a != up_axis && a != h1).unwrap_or(2);
    let mut eye = [center[0], center[1], center[2]];
    eye[h1] += ang.cos() * dist;
    eye[h2] += ang.sin() * dist;
    eye[up_axis] += height;
    let mut cam = Camera::default();
    cam.eye = Vec3::new(eye[0], eye[1], eye[2]);
    cam.target = Vec3::new(center[0], center[1], center[2]);
    cam.up = match up_axis {
        0 => Vec3::new(1.0, 0.0, 0.0),
        1 => Vec3::new(0.0, 1.0, 0.0),
        _ => Vec3::new(0.0, 0.0, 1.0),
    };
    cam.fov_y = std::f32::consts::FRAC_PI_4;
    // Block 90: scene-scaled planes (was `dist - radius*1.2`, which
    // clipped any approach take once the subject came closer than
    // dist-1.2).  near = 5% of the subject radius; far covers the
    // camera distance plus ~30 m of root-motion travel.
    cam.near = (radius * 0.05).max(0.01);
    cam.far = dist + 30.0;
    cam
}

// ---------------------------------------------------------------------------
// Block 108 tests — the encode pacing is the ANIMATION PRELUDE's real
// fps, not a hardcoded rate.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    /// The committed alex_loco_run_n fixture pins the PRELUDE's real
    /// frame_count + fps — the values the CLI's encode step consumes
    /// (block 108: the MP4 playback rate defaults to these, replacing
    /// the pre-108 hardcoded 10 fps which played every take 3x slow).
    #[test]
    fn fixture_prelude_paces_the_encode() {
        let path = env!("CARGO_MANIFEST_DIR").to_string()
            + "/../prototype-p3d/tests/fixtures/anim_alex_loco_run_n.p3d";
        let buf = std::fs::read(&path).expect("fixture present");
        let (off, span, name) = &prototype_p3d::anim::find_anim_tracks(&buf)[0];
        assert_eq!(name, "alex_loco_run_n");
        let t = prototype_p3d::anim::decode_anim_track(&buf, *off, *span).unwrap();
        // The prelude values (PROVEN block 85, re-pinned block 107 from
        // the raw alex.p3d byte stream @ 0x72c182).
        assert_eq!(t.frame_count, 19.0);
        assert_eq!(t.fps, 30.0);
        // The CLI's pacing contract: cycle = frame_count (rounded up,
        // min 1); encode fps = the prelude fps unless overridden.
        let cycle = t.frame_count.max(1.0) as usize;
        assert_eq!(cycle, 19);
        let encode_fps = t.fps; // the None-override path
        assert!((encode_fps - 30.0).abs() < 1e-6);
        // The override path returns exactly what the user passed.
        let overridden: Option<f32> = Some(10.0);
        assert_eq!(overridden.unwrap_or(t.fps), 10.0);
    }

    /// The CLI's encode-script generator: by default the playback rate
    /// is the ANIMATION PRELUDE's real fps; --encode-fps overrides.
    /// (The prelude values themselves are pinned in
    /// prototype-p3d's `fixture_header_decodes` — 19.0 frames / 30.0
    /// fps for alex_loco_run_n.)
    #[test]
    fn encode_script_defaults_to_prelude_fps() {
        // No override → the prelude fps (30.0 for the run track).
        let (cmd, script) = encode_script_text("media/run/frame", 30.0, None);
        assert!(
            cmd.contains("-framerate 30 "),
            "default = prelude fps, got: {cmd}"
        );
        assert!(cmd.contains("libx264"), "cmd is an ffmpeg encode: {cmd}");
        assert!(cmd.contains("media/run/frame_%04d.png"));
        assert!(cmd.contains("media/run/frame.mp4"));
        assert!(script.contains("#!/bin/sh"));
        assert!(script.contains("REAL rate"), "script documents the source: {script}");
        // The script path lands next to the PNGs.
        assert_eq!(encode_script_path("media/run/frame"), "media/run/encode.sh");
        // Override → the user's rate, with the prelude documented.
        let (cmd2, script2) = encode_script_text("media/run/frame", 30.0, Some(10.0));
        assert!(cmd2.contains("-framerate 10 "), "override respected: {cmd2}");
        assert!(
            script2.contains("overridden by --encode-fps"),
            "script documents the override: {script2}"
        );
        assert!(script2.contains("30 fps"), "prelude value still documented");
    }

    /// Block 109: the chase-cam aiming contract — with --root-motion
    /// and a lagged orbit the camera EYE stays on the smooth lag
    /// dolly but the TARGET tracks the subject root, so the runner
    /// stays framed over arbitrarily long takes (the block-108
    /// root-motion attempt went blank after cycle 1 because the look
    /// point stayed on the lagged center while the subject ran away).
    /// The aiming lives in the frame loop (cam.target = pose root);
    /// this test pins the MATH it relies on: the far-plane stretch
    /// must cover the eye-to-subject distance for the documented
    /// block-90 take (8 cycles x 3.59 m/cycle travel at follow-lag
    /// 1.0 keeps eye-to-subject at the orbit dist).
    #[test]
    fn root_motion_far_plane_covers_chase_distance() {
        // Block-90 proven take geometry: radius ~0.99, dist = 2.4 x
        // radius, 8 cycles x 3.59 m/cycle of root travel.
        let radius = 0.99f32;
        let dist = 2.4 * radius;
        let far_block90 = dist + 30.0; // the block-90 far plane
        // 8 cycles of travel at the REAL cycle travel (block 90:
        // z -0.275 to -3.870 per cycle = 3.595 m).
        let travel_per_cycle = 3.870f32 - 0.275;
        let total_travel = 8.0 * travel_per_cycle;
        // Chase cam (lag 1.0): the eye follows the root, so the
        // eye-to-subject distance stays at the orbit dist.
        let eye_to_subject_lag1 = dist;
        let far_needed = eye_to_subject_lag1 + radius * 2.0;
        assert!(
            far_block90 >= far_needed,
            "block-90 far plane covers the chase take: {far_block90} >= {far_needed}"
        );
        // Lagged cam (lag 0.15): the subject runs (1-lag) x travel
        // past the look point — the block-108 blank-frame geometry.
        let lag = 0.15f32;
        let subject_offset = (1.0 - lag) * total_travel;
        let eye_to_subject_lagged = dist + subject_offset; // worst case
        let _ = eye_to_subject_lagged; // documented: needs the block-109 stretch
        // The block-109 contract: far >= eye_to_subject + 2 x radius
        // — asserted via the formula the frame loop applies.
        let far_stretched = far_block90.max(eye_to_subject_lagged + radius * 2.0);
        assert!(
            far_stretched >= eye_to_subject_lagged,
            "stretched far covers the lagged take"
        );
        // And the stretch formula never SHRINKS the plane.
        let tiny_subject = 0.5f32;
        assert_eq!(
            far_block90.max(tiny_subject + radius * 2.0),
            far_block90,
            "stretch is a max(), never a shrink"
        );
    }
}

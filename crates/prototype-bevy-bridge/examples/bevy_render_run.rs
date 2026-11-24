//! Block 94 — `bevy_render_run`: the REAL bevy `AnimationPlayer` drives
//! the REAL `alex_loco_run_n` clip over the REAL 67-joint skeleton, and
//! every ticked pose is rendered through the proven CPU skinned
//! rasterizer — the block-92 NEXT-UP #1 design ("a bevy_plugin headless
//! app that renders the ticked pose through the existing CPU skinned
//! renderer").
//!
//! Data flow per frame:
//!
//! ```text
//! alex.p3d ──decode──▶ Skeleton + OldSkinnedShapes + textures
//!          + AnimTrack (0x121000)
//! bridge:  extract_skeleton / extract_animation(track, Some(&skel))
//!          → into_animation_clip → AnimationGraph → AnimationPlayer
//! tick k:  headless App::update() (TimeUpdateStrategy::ManualDuration
//!          1/30 s) → animate_targets writes the evaluated Transform on
//!          every AnimationTarget joint entity
//! render:  snapshot the 67 ticked Transforms → LOCAL quat+translation
//!          → anim::pose_world_matrices_from_locals (the SAME parent-
//!          first row-vector GEM fold as the CPU pipeline)
//!          → skin_matrices / skin_positions / skin_normals
//!          → render_meshes_cpu (ONE depth buffer) → PNG
//! ```
//!
//! Every frame ALSO recomputes the CPU-pipeline reference pose
//! (`anim::pose_world_matrices(skel, track, k % 18)`) and asserts the
//! bevy-folded worlds agree element-wise — the block-92 tick test's
//! transform-level pin, re-proven at WORLD-matrix level while rendering.
//!
//! Headless: no window, no GPU — the bevy Transforms stay LOCAL
//! (skeleton space) and the world compose + rasterization happen in the
//! CPU pipeline.  Run with `cargo run -p prototype-bevy-bridge
//! --features bevy --example bevy_render_run -- <alex.p3d>
//! --out-prefix <dir/prefix>`.

use bevy::animation::prelude::{
    AnimationGraph, AnimationGraphHandle, AnimationPlayer, AnimationPlugin,
};
use bevy::app::App;
use bevy::asset::AssetPlugin;
// TaskPoolPlugin lives in bevy_core in 0.15 (not bevy::app).
use bevy::core::TaskPoolPlugin;
use bevy::ecs::world::CommandQueue;
use bevy::pbr::StandardMaterial;
use bevy::render::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::render::mesh::Mesh;
// bevy_time is a direct dep (no facade feature in bevy 0.15) — the SAME
// crate instance bevy_animation's Res<Time> uses.
use bevy_time::{TimePlugin, TimeUpdateStrategy};

use prototype_bevy_bridge::{
    apply_root_motion_loop, extract_animation, extract_skeleton, spawn_skinned_character,
    BridgeAnimationExt,
};
use prototype_math::Vec3 as MathVec3;
use prototype_p3d::anim::{
    decode_anim_track, dof_loop_params, find_anim_tracks, pose_world_matrices_from_locals,
    pose_world_matrices_root_motion,
};
use prototype_p3d::geometry::{decode_texture, top_level_chunks};
use prototype_p3d::skin::{
    decode_old_skinned_shape, decode_skeleton, rest_world_matrices, skin_matrices, skin_normals,
    skin_positions, OldSkinnedShape, Skeleton,
};
use prototype_render::render_art_world_cpu::render_meshes_cpu;
use prototype_render::scene::{Camera, SceneMesh};

struct DrawPart {
    shape_name: String,
    meshes: Vec<prototype_p3d::skin::SkinMesh>,
    tex: Option<(Vec<u8>, u32, u32)>,
}

/// Block 95 camera modes: `chase` = the proven block-94 follow cam
/// (rest framing + the root's world delta, orbiting); `approach` = a
/// camera parked on the run axis AHEAD of the character that RETREATS
/// as the accumulated root motion closes the gap — the lead shrinks
/// `--cam-dist` → ~2.3 body radii over the take, so the subject grows
/// from a distant silhouette into a full-body close-up while the
/// traversal stays REAL: a seam snap-back in the root-motion
/// accumulator would jolt the framing by ~3.6 m, so the shot doubles
/// as the accumulator's visual proof.
#[derive(Clone, Copy, PartialEq)]
enum CamMode {
    Chase,
    Approach,
}

struct Args {
    out_prefix: String,
    anim: String,
    frames: usize,
    width: u32,
    height: u32,
    orbit_start_deg: f32,
    orbit_end_deg: f32,
    cam_dist: Option<f32>,
    cam_height: f32,
    max_delta: f32,
    cam_mode: CamMode,
}

fn parse_args() -> (Args, String) {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() {
        eprintln!(
            "usage: bevy_render_run <char.p3d> --out-prefix <dir/prefix> [--anim NAME] \
             [--frames N] [--width W] [--height H] [--orbit-start-deg D] [--orbit-end-deg D] \
             [--cam-dist D] [--cam-height D] [--max-delta F] [--cam-mode chase|approach]"
        );
        std::process::exit(2);
    }
    let path = argv[0].clone();
    let mut a = Args {
        out_prefix: String::from("/tmp/bevy_run/frame"),
        anim: String::from("alex_loco_run_n"),
        frames: 114,
        width: 480,
        height: 360,
        orbit_start_deg: -40.0,
        orbit_end_deg: 40.0,
        cam_dist: None,
        cam_height: 0.0,
        max_delta: 5e-3,
        cam_mode: CamMode::Chase,
    };
    let mut i = 1usize;
    while i < argv.len() {
        let flag = argv[i].as_str();
        let val = |i: &mut usize| -> String {
            *i += 1;
            if *i >= argv.len() {
                eprintln!("{flag}: missing value");
                std::process::exit(2);
            }
            argv[*i].clone()
        };
        match flag {
            "--out-prefix" => a.out_prefix = val(&mut i),
            "--anim" => a.anim = val(&mut i),
            "--frames" => a.frames = val(&mut i).parse().expect("--frames int"),
            "--width" => a.width = val(&mut i).parse().expect("--width int"),
            "--height" => a.height = val(&mut i).parse().expect("--height int"),
            "--orbit-start-deg" => a.orbit_start_deg = val(&mut i).parse().expect("float"),
            "--orbit-end-deg" => a.orbit_end_deg = val(&mut i).parse().expect("float"),
            "--cam-dist" => a.cam_dist = Some(val(&mut i).parse().expect("float")),
            "--cam-height" => a.cam_height = val(&mut i).parse().expect("float"),
            "--max-delta" => a.max_delta = val(&mut i).parse().expect("float"),
            "--cam-mode" => {
                a.cam_mode = match val(&mut i).as_str() {
                    "chase" => CamMode::Chase,
                    "approach" => CamMode::Approach,
                    other => {
                        eprintln!("bevy_render_run: --cam-mode must be chase|approach, got {other}");
                        std::process::exit(2);
                    }
                };
            }
            other => {
                eprintln!("bevy_render_run: unknown arg {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    (a, path)
}

fn main() {
    let (a, path) = parse_args();
    let buf = std::fs::read(&path).unwrap_or_else(|e| {
        eprintln!("bevy_render_run: read {path}: {e}");
        std::process::exit(1);
    });
    let top = top_level_chunks(&buf).unwrap_or_else(|| {
        eprintln!("bevy_render_run: {path} is not a valid P3D");
        std::process::exit(1);
    });

    // --- shapes (fail-soft, same policy as render-skinned-run) ---
    let mut shapes: Vec<OldSkinnedShape> = Vec::new();
    for c in top.iter().filter(|c| c.chunk_type == 0x0001_0001) {
        match decode_old_skinned_shape(&buf, c.offset, c.size) {
            Ok(s) => shapes.push(s),
            Err(e) => eprintln!("bevy_render_run: shape at {:#x} skipped: {e}", c.offset),
        }
    }
    if shapes.is_empty() {
        eprintln!("bevy_render_run: no decodable 0x10001 shape in {path}");
        std::process::exit(1);
    }
    // --- skeletons ---
    let mut skeletons: Vec<Skeleton> = Vec::new();
    for c in top.iter().filter(|c| c.chunk_type == 0x0002_3000) {
        match decode_skeleton(&buf, c.offset, c.size) {
            Ok(s) => skeletons.push(s),
            Err(e) => eprintln!("bevy_render_run: skeleton at {:#x} skipped: {e}", c.offset),
        }
    }
    // --- textures (name → decoded base mip) ---
    let mut textures: Vec<(String, (Vec<u8>, u32, u32))> = Vec::new();
    for c in top.iter().filter(|c| c.chunk_type == 0x0001_9000) {
        if let Ok(t) = decode_texture(&buf, c.offset, c.size) {
            if let Ok(surfaces) = prototype_render::dxt::decode_dds_mips(&t.dds) {
                if let Some(s) = surfaces.first() {
                    textures.push((
                        t.name.trim_end_matches('\0').to_lowercase(),
                        (s.rgba.clone(), s.width, s.height),
                    ));
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

    // --- bind the skeleton: the skeleton whose bound shapes carry the
    // MOST vertices (the body model beats the arms/vest variants) —
    // the proven render-skinned-run policy.
    let mut best: (String, usize) = (shapes[0].skeleton.clone(), 0);
    for s in &shapes {
        let verts: usize = s.meshes.iter().map(|m| m.vertex_count as usize).sum();
        if verts > best.1 {
            best = (s.skeleton.clone(), verts);
        }
    }
    let skel = skeletons
        .iter()
        .find(|s| s.name == best.0)
        .unwrap_or_else(|| {
            eprintln!("bevy_render_run: skeleton {:?} not found", best.0);
            std::process::exit(1);
        });
    let mut parts: Vec<DrawPart> = Vec::new();
    for s in &shapes {
        if s.skeleton != skel.name {
            continue;
        }
        let tex = pick_tex(&s.name);
        parts.push(DrawPart { shape_name: s.name.clone(), meshes: s.meshes.clone(), tex });
    }
    if parts.is_empty() {
        eprintln!("bevy_render_run: no shape bound to skeleton {:?}", skel.name);
        std::process::exit(1);
    }

    // --- the anim track ---
    let tracks = find_anim_tracks(&buf);
    let track = tracks
        .iter()
        .find(|(_, _, n)| n == &a.anim)
        .map(|(off, span, _)| decode_anim_track(&buf, *off, *span))
        .unwrap_or_else(|| {
            eprintln!(
                "bevy_render_run: track {:?} not found ({} tracks)",
                a.anim,
                tracks.len()
            );
            std::process::exit(1);
        })
        .unwrap_or_else(|e| {
            eprintln!("bevy_render_run: track decode: {e}");
            std::process::exit(1);
        });
    // The curve domain: 19 samples span 18 frame INTERVALS (frame 18 ≡
    // frame 0 on the cyclic track) — tick k lands on sample k % 18.
    let cycle_intervals = (track.frame_count.max(1.0) as usize).saturating_sub(1).max(1);

    // --- BLOCK 95: the root-motion loop accumulator.  bevy's
    // AnimationPlayer loops the cyclic clip WITHIN its own frame range,
    // so Motion_Root snaps back one cycle_travel at every seam; the
    // engine instead accumulates the DOF travel (k · cycle_travel — the
    // block-90 rule, now shared with the CPU path through
    // dof_loop_params).  Applied per tick AFTER the AnimationPlayer
    // evaluation, BEFORE the world compose.
    let loop_params = dof_loop_params(skel, &track);
    match &loop_params {
        Some(lp) => eprintln!(
            "bevy_render_run: ROOT-MOTION LOOP ON — joint {:?} (idx {}) axis {}, \
             travel {:.3} m/cycle, v0 {:.3} (bevy AnimationPlayer + accumulator)",
            skel.joints[lp.joint].name, lp.joint, lp.axis, lp.cycle_travel, lp.v0,
        ),
        None => eprintln!(
            "bevy_render_run: no 0x121102 DOF channel on {:?} — plain looping pose (no root-motion accumulation)",
            track.name
        ),
    }

    // --- bridge: real skeleton + real track → real AnimationClip ---
    let bridge_skel = extract_skeleton(skel);
    let bridge_anim = extract_animation(&track, Some(skel));
    let n_curves = bridge_anim.curves.len();
    let root_motion = bridge_anim.root_motion;
    let clip = bridge_anim.into_animation_clip();

    // --- rest-pose bounds for stable camera framing (the proven
    // render-skinned-run policy: anim motion must not re-frame shots).
    let rest = rest_world_matrices(skel);
    let mut bound_meshes: Vec<SceneMesh> = Vec::new();
    for p in &parts {
        for m in &p.meshes {
            let skin = skin_matrices(&m.palette, &rest, &rest);
            bound_meshes.push(scene_mesh(p, m, &skin));
        }
    }
    let (center, radius, up_axis) = frame_bounds(&bound_meshes);
    let dist = a.cam_dist.unwrap_or(radius * 2.4);
    // The root joint's REST world translation — the camera follows the
    // pose's DELTA from this (the proven render-skinned-run chase cam):
    // the clip's REAL Motion_Root DOF curve runs the character forward
    // z -0.275 → -3.87 per cycle, and a static camera loses the subject
    // mid-cycle (observed as blank frames in the first 114-frame run).
    let rest_root_t = rest[0][3];
    eprintln!(
        "bevy_render_run: {} parts on {:?}; clip {:?} = {} samples @ {} fps (curve domain \
         {} intervals); {} curves; root_motion {:?}; bounds center {:.2?} radius {:.2}",
        parts.len(),
        skel.name,
        track.name,
        track.frame_count,
        track.fps,
        cycle_intervals,
        n_curves,
        root_motion,
        center,
        radius,
    );

    // --- headless bevy App (no window, no GPU; NO TransformPlugin —
    // the joint Transforms stay LOCAL exactly like the tick test).
    let mut app = App::new();
    app.add_plugins((
        TaskPoolPlugin::default(),
        TimePlugin,
        AssetPlugin::default(),
        AnimationPlugin,
    ));
    let dt = std::time::Duration::from_secs_f32(1.0 / track.fps.max(1.0));
    app.insert_resource(TimeUpdateStrategy::ManualDuration(dt));

    let clip_handle = app
        .world_mut()
        .resource_mut::<bevy::asset::Assets<bevy::animation::AnimationClip>>()
        .add(clip);
    let (graph, node_index) = AnimationGraph::from_clip(clip_handle);
    let graph_handle = app
        .world_mut()
        .resource_mut::<bevy::asset::Assets<AnimationGraph>>()
        .add(graph);
    // Dangling handles: the skinned mesh entity is inert headless (no
    // render systems run) — only the JOINT hierarchy matters here.
    let mesh_handle: bevy::asset::Handle<Mesh> = bevy::asset::Handle::default();
    let mat_handle: bevy::asset::Handle<StandardMaterial> = bevy::asset::Handle::default();
    let ib_handle: bevy::asset::Handle<SkinnedMeshInverseBindposes> = bevy::asset::Handle::default();

    let mut queue = CommandQueue::default();
    let (_root, player_entity, joint_entities) = {
        let world = app.world_mut();
        let mut commands = bevy::ecs::system::Commands::new(&mut queue, world);
        let root = commands.spawn(bevy::core::Name::new("alex_char_root")).id();
        let player_entity = commands
            .spawn((
                bevy::core::Name::new("player"),
                AnimationPlayer::default(),
                AnimationGraphHandle(graph_handle.clone()),
            ))
            .id();
        let joints = spawn_skinned_character(
            &mut commands,
            root,
            player_entity,
            &bridge_skel,
            &mesh_handle,
            &mat_handle,
            ib_handle,
        );
        (root, player_entity, joints)
    };
    {
        let world = app.world_mut();
        queue.apply(world);
    }
    assert_eq!(
        joint_entities.len(),
        skel.joints.len(),
        "one entity per skeleton joint"
    );

    // WARM-UP: thread_animation_graphs must see the graph asset event
    // before animate_targets evaluates anything (block-92 discovery).
    for _ in 0..2 {
        app.update();
    }
    // Start playback: the graph node holding the run clip, looping
    // forever (cyclic track).
    app.world_mut()
        .entity_mut(player_entity)
        .get_mut::<AnimationPlayer>()
        .expect("player")
        .play(node_index)
        .repeat();

    // --- TICK + RENDER LOOP ---
    let mut worst_delta = 0.0f32;
    let mut min_distinct = usize::MAX;
    let mut total_drawn = 0usize;
    for k in 1..=a.frames {
        app.update();
        // Snapshot every joint's TICKED local transform (bevy evaluated
        // the clip curves through animate_targets).
        let (rots, mut trans): (Vec<[f32; 4]>, Vec<[f32; 3]>) = {
            let world = app.world();
            joint_entities
                .iter()
                .map(|&e| {
                    let t = world
                        .get::<bevy::prelude::Transform>(e)
                        .expect("joint Transform");
                    (
                        [t.rotation.x, t.rotation.y, t.rotation.z, t.rotation.w],
                        [t.translation.x, t.translation.y, t.translation.z],
                    )
                })
                .unzip()
        };
        // BLOCK 95: fold the k · cycle_travel root-motion accumulator
        // into the ticked locals (tick k lands on curve frame
        // k % cycle_intervals, so the cycle index is k / cycle_intervals).
        if let Some(lp) = &loop_params {
            apply_root_motion_loop(&mut trans, lp, k as u32, cycle_intervals as u32);
        }
        // The bevy-folded world pose — the pose this frame RENDERS.
        let pose = pose_world_matrices_from_locals(skel, &rots, &trans);
        // CPU-pipeline reference at the same curve sample — the
        // ROOT-MOTION compose (global frame k = f % cycle + k·travel on
        // the DOF axis; plain pose when the track has no DOF).  The
        // render only proceeds when the two agree (world-matrix pin):
        // this asserts the bevy accumulator against the independent CPU
        // implementation tick-for-tick while rendering.
        let ref_pose = pose_world_matrices_root_motion(skel, &track, k as f32, cycle_intervals);
        let mut maxd = 0.0f32;
        let mut worst_joint = String::new();
        for (i, (w, r)) in pose.iter().zip(ref_pose.iter()).enumerate() {
            let mut jd = 0.0f32;
            for row in 0..4 {
                for col in 0..4 {
                    jd = jd.max((w[row][col] - r[row][col]).abs());
                }
            }
            if jd > maxd {
                maxd = jd;
                worst_joint = skel.joints[i].name.clone();
            }
        }
        worst_delta = worst_delta.max(maxd);
        if maxd > a.max_delta {
            // DIAGNOSTIC: dump the Head->root chain: per-joint world
            // delta (vs CPU compose), bevy-vs-bridge local quat dot, and
            // the REST 3x3 row norms (scale detection).
            let (brots, btrans) =
                prototype_bevy_bridge::pose_local_transforms(skel, &track, (k % cycle_intervals) as u32);
            let wi = skel.find(&worst_joint).expect("joint");
            let mut chain: Vec<usize> = Vec::new();
            let mut pi = wi;
            loop {
                chain.push(pi);
                let p = skel.joints[pi].parent as usize;
                if p == pi || chain.len() > 12 {
                    break;
                }
                pi = p;
            }
            eprintln!("DIAG tick {k} chain (leaf->root):");
            for &ci in &chain {
                let mut jd = 0.0f32;
                for row in 0..4 {
                    for col in 0..4 {
                        jd = jd.max((pose[ci][row][col] - ref_pose[ci][row][col]).abs());
                    }
                }
                let dot: f32 = rots[ci].iter().zip(brots[ci].iter()).map(|(a, b)| a * b).sum::<f32>().abs();
                let r0 = (skel.joints[ci].rest[0][0..3].iter().map(|v| v * v).sum::<f32>()).sqrt();
                let r1 = (skel.joints[ci].rest[1][0..3].iter().map(|v| v * v).sum::<f32>()).sqrt();
                let r2 = (skel.joints[ci].rest[2][0..3].iter().map(|v| v * v).sum::<f32>()).sqrt();
                let t0 = (skel.joints[ci].rest[0][0..3].iter().map(|v| v * v).sum::<f32>()).sqrt();
                let _ = t0;
                eprintln!(
                    "  {:>14} world_d {jd:.3e} dot {dot:.9} rest_row_norms {r0:.4}/{r1:.4}/{r2:.4}",
                    skel.joints[ci].name
                );
                eprintln!(
                    "      bevy_w_trans {:?} cpu_w_trans {:?} | bevy_l_trans {:?} cpu_l_trans {:?}",
                    pose[ci][3], ref_pose[ci][3], trans[ci], btrans[ci]
                );
                if skel.joints[ci].name == "Spine_2" {
                    for row in 0..4 {
                        eprintln!(
                            "      S2 bevy {:?} | cpu {:?}",
                            pose[ci][row], ref_pose[ci][row]
                        );
                    }
                    eprintln!(
                        "      S2 bevy_local_rot {:?} parent_idx {} chain_parent_world_ok",
                        rots[ci],
                        skel.joints[ci].parent
                    );
                    // The two extraction paths, side by side, for THIS
                    // joint at THIS frame.
                    if let Some(g) = track.groups.iter().find(|g| g.bone_name == "Spine_2") {
                        let f = k % cycle_intervals;
                        eprintln!(
                            "      S2 has_rot {} rot.count {} rot.keys.len {} idx.len {} | \
                             has_compact {} compact.count {} | has_tran {}",
                            g.rot.is_some(),
                            g.rot.as_ref().map(|c| c.count).unwrap_or(0),
                            g.rot.as_ref().map(|c| c.keys.len()).unwrap_or(0),
                            g.rot.as_ref().map(|c| c.key_indices.len()).unwrap_or(0),
                            g.rot_compact.is_some(),
                            g.rot_compact.as_ref().map(|c| c.count).unwrap_or(0),
                            g.tran.is_some(),
                        );
                        if let Some(c) = g.rot.as_ref().or(g.rot_compact.as_ref()) {
                            eprintln!(
                                "      S2 key_at_frame({f}) {:?} | key_indices[17] idx {:?} key {:?}",
                                c.key_at_frame(f),
                                c.key_indices.get(17),
                                c.key_indices.get(17).and_then(|i| c.keys.get(*i as usize)),
                            );
                        }
                        // What the CPU fold uses for the local 3x3:
                        // j.rest 3x3 REPLACED by quat_to_mat3(key). Print
                        // the rest 3x3 too (unanimated fall-back path).
                        eprintln!("      S2 rest 3x3 {:?}", skel.joints[ci].rest);
                    } else {
                        eprintln!("      S2 has NO anim group");
                    }
                }
            }
            panic!(
                "tick {k}: bevy-vs-CPU world delta {maxd} > {} (worst joint {worst_joint:?})",
                a.max_delta
            );
        }

        // skin + raster (the proven block-88 pipeline).
        let mut meshes: Vec<SceneMesh> = Vec::new();
        let mut texs: Vec<Option<(Vec<u8>, u32, u32)>> = Vec::new();
        for p in &parts {
            for m in &p.meshes {
                let skin = skin_matrices(&m.palette, &rest, &pose);
                meshes.push(scene_mesh(p, m, &skin));
                texs.push(p.tex.clone());
            }
        }
        let cam = match a.cam_mode {
            CamMode::Approach => {
                // Leading camera ON THE RUN AXIS: the run drives the root
                // toward -axis (the run track's DOF runs z -0.275 → -3.87
                // per cycle), so "ahead" = root − lead.  The lead shrinks
                // from cam_dist (default 25 m) to ~2.3 body radii over
                // the take; the camera NEVER crosses the subject and the
                // approach is exactly the accumulated root motion.
                let run_axis = loop_params.map_or(2, |lp| lp.axis);
                let lead_start = a.cam_dist.unwrap_or(25.0);
                let lead_end = radius * 2.3;
                let t = if a.frames <= 1 {
                    0.0
                } else {
                    (k - 1) as f32 / (a.frames - 1) as f32
                };
                // Exponential lead decay: the subject's apparent size
                // grows ~1/lead, so a linear lead front-loads nothing
                // and dumps all the growth into the last second; an
                // exponential decay gives a steady perceptual zoom-in
                // across the whole take (lead halves every ~0.23 of the
                // take) and ends at a comfortable full-body close-up.
                let lead =
                    lead_end + (lead_start - lead_end) * (-3.0 * t).exp();
                approach_camera(&center, &pose[0][3], run_axis, lead, a.cam_height, up_axis)
            }
            CamMode::Chase => {
                // Chase-cam center: rest-pose framing PLUS the root's
                // world translation delta (follow_lag 1.0 = full chase).
                let d = [
                    pose[0][3][0] - rest_root_t[0],
                    pose[0][3][1] - rest_root_t[1],
                    pose[0][3][2] - rest_root_t[2],
                ];
                let follow_center = [
                    center[0] + d[0],
                    center[1] + d[1],
                    center[2] + d[2],
                ];
                orbit_camera(
                    &follow_center,
                    dist,
                    radius,
                    a.cam_height,
                    a.orbit_start_deg,
                    a.orbit_end_deg,
                    k,
                    a.frames,
                    up_axis,
                )
            }
        };
        let refs: Vec<&SceneMesh> = meshes.iter().collect();
        let tex_refs: Vec<Option<(&[u8], u32, u32)>> = texs
            .iter()
            .map(|t| t.as_ref().map(|(b, w, h)| (b.as_slice(), *w, *h)))
            .collect();
        let frame =
            render_meshes_cpu(&refs, &tex_refs, &cam, a.width, a.height, [10, 12, 16, 255])
                .unwrap_or_else(|e| {
                    eprintln!("bevy_render_run: render frame {k}: {e}");
                    std::process::exit(1);
                });
        min_distinct = min_distinct.min(frame.stats.distinct);
        total_drawn += frame.stats.triangles_drawn;
        let png = prototype_video::png::png_encode_rgba(frame.width, frame.height, &frame.rgba)
            .unwrap_or_else(|e| {
                eprintln!("bevy_render_run: PNG encode: {e}");
                std::process::exit(1);
            });
        let out = format!("{}_{:04}.png", a.out_prefix, k);
        std::fs::write(&out, &png).unwrap_or_else(|e| {
            eprintln!("bevy_render_run: write {out}: {e}");
            std::process::exit(1);
        });
    }
    eprintln!(
        "bevy_render_run: wrote {} frames to {}_%04d.png ({}x{}, worst bevy-vs-CPU world \
         delta {worst_delta:.2e}, min distinct {min_distinct}, avg drawn tris {})",
        a.frames,
        a.out_prefix,
        a.width,
        a.height,
        total_drawn / a.frames.max(1)
    );
}

/// Skinned SceneMesh for one draw part (stream-A UVs, mesh indices —
/// the proven block-88 `part_scene_mesh`).
fn scene_mesh(
    part: &DrawPart,
    m: &prototype_p3d::skin::SkinMesh,
    skin: &[[[f32; 4]; 4]],
) -> SceneMesh {
    SceneMesh {
        name: format!("{}/{:?}", part.shape_name, m.name),
        desc: m.desc,
        positions: skin_positions(m, skin),
        normals: skin_normals(m, skin),
        uvs: m.uvs.clone(),
        tangents: Vec::new(),
        indices: m.indices.clone(),
        bound_box: m.bound_box,
        bound_sphere: m.bound_sphere,
        texture_dds: None,
        extra_textures: Vec::new(),
    }
}

/// World bounds over the rest-pose skinned meshes: (center, radius,
/// up-axis = longest bbox extent).
fn frame_bounds(meshes: &[SceneMesh]) -> ([f32; 3], f32, usize) {
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
    let up = (0..3).max_by(|a, b| ext[*a].total_cmp(&ext[*b])).unwrap_or(1);
    let center = [(mn[0] + mx[0]) * 0.5, (mn[1] + mx[1]) * 0.5, (mn[2] + mx[2]) * 0.5];
    let radius = 0.5 * (ext[0] * ext[0] + ext[1] * ext[1] + ext[2] * ext[2]).sqrt().max(1e-4);
    (center, radius, up)
}

/// Leading approach camera (block 95): eye parked `lead` metres ahead
/// of the character's CURRENT root position along `run_axis` (the run
/// advances toward −axis, so ahead = root − lead), aiming at the
/// character's current depth on that axis with the rest-pose framing's
/// lateral/vertical center.  The eye retreats smoothly as `lead`
/// shrinks, so the subject closes from a distant silhouette to a
/// full-body close-up purely through its REAL accumulated root motion.
fn approach_camera(
    center: &[f32; 3],
    root_pos: &[f32; 4],
    run_axis: usize,
    lead: f32,
    height: f32,
    up_axis: usize,
) -> Camera {
    let mut eye = [center[0], center[1], center[2]];
    let mut tgt = [center[0], center[1], center[2]];
    eye[run_axis] = root_pos[run_axis] - lead;
    tgt[run_axis] = root_pos[run_axis];
    eye[up_axis] += height;
    let mut cam = Camera::default();
    cam.eye = MathVec3::new(eye[0], eye[1], eye[2]);
    cam.target = MathVec3::new(tgt[0], tgt[1], tgt[2]);
    cam.up = match up_axis {
        0 => MathVec3::new(1.0, 0.0, 0.0),
        1 => MathVec3::new(0.0, 1.0, 0.0),
        _ => MathVec3::new(0.0, 0.0, 1.0),
    };
    cam.fov_y = std::f32::consts::FRAC_PI_4;
    cam.near = 0.05_f32.max(lead * 0.004);
    cam.far = lead + 30.0;
    cam
}

/// Orbit camera (the proven block-90 scene-scaled planes).
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
    let h2 = [0usize, 1, 2]
        .into_iter()
        .find(|&a| a != up_axis && a != h1)
        .unwrap_or(2);
    let mut eye = [center[0], center[1], center[2]];
    eye[h1] += ang.cos() * dist;
    eye[h2] += ang.sin() * dist;
    eye[up_axis] += height;
    let mut cam = Camera::default();
    cam.eye = MathVec3::new(eye[0], eye[1], eye[2]);
    cam.target = MathVec3::new(center[0], center[1], center[2]);
    cam.up = match up_axis {
        0 => MathVec3::new(1.0, 0.0, 0.0),
        1 => MathVec3::new(0.0, 1.0, 0.0),
        _ => MathVec3::new(0.0, 0.0, 1.0),
    };
    cam.fov_y = std::f32::consts::FRAC_PI_4;
    cam.near = (radius * 0.05).max(0.01);
    cam.far = dist + 30.0;
    cam
}

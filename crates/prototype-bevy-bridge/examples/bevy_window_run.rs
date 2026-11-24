//! Block 97 — `bevy_window_run`: NEXT-UP #1, the WINDOWED bevy app (the
//! block-96 deferral).  The headless chain (blocks 94/95) proved the
//! REAL `AnimationPlayer` driving the REAL `alex_loco_run_n` clip with
//! the REAL root-motion accumulator, rendering through the CPU
//! rasterizer.  This example is the same proven pipeline INSIDE a real
//! winit window: `DefaultPlugins` + `WindowPlugin`, a real GPU mesh
//! (`BridgeSkinExt::into_bevy_mesh` — POSITION/NORMAL/UV_0/JOINT_*
//! attributes), real decoded textures (`StandardMaterial
//! base_color_texture`), the real joint hierarchy
//! (`spawn_skinned_character`), the real clip through
//! `AnimationGraph::from_clip` → `AnimationPlayer::play().repeat()`,
//! and the accumulator as an ECS resource on the CHARACTER ROOT entity
//! (the clip animates only the 67 joint entities — the root above them
//! is free, so `k · cycle_travel` lands there and bevy's own
//! GlobalTransform propagation folds it into every joint world pose,
//! exactly the block-90/95 rule).
//!
//! USER-SIDE RENDER: the sandbox has no display server — this example
//! is compile-checked here (`cargo build -p prototype-bevy-bridge
//! --features bevy-window --example bevy_window_run`) and RUN on the
//! user's desktop.  Loader-style: the .p3d path is a CLI argument, no
//! game files are bundled.
//!
//! ```text
//! cargo run -p prototype-bevy-bridge --features bevy-window \
//!     --example bevy_window_run -- /path/to/alex.p3d [--anim NAME]
//! ```
//!
//! Controls: none (demo).  The camera slowly orbits the character root
//! and follows its REAL accumulated forward travel; the character
//! loops the run cycle and walks forward forever with no seam snap.

use bevy::animation::prelude::{
    AnimationGraph, AnimationGraphHandle, AnimationNodeIndex, AnimationPlayer,
};
use bevy::app::App;
use bevy::asset::{Assets, Handle, RenderAssetUsages};
use bevy::core_pipeline::core_3d::Camera3d;
use bevy::pbr::{DirectionalLight, StandardMaterial};
use bevy::prelude::*;
use bevy::render::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::render::mesh::Mesh;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::window::{PresentMode, Window, WindowPlugin};

use prototype_bevy_bridge::{
    extract_animation, extract_skin, extract_skeleton, inverse_bindposes,
    spawn_skinned_character, BridgeAnimationExt, BridgeSkinExt,
};
use prototype_p3d::anim::{decode_anim_track, dof_loop_params, find_anim_tracks, DofLoop};
use prototype_p3d::geometry::{decode_texture, top_level_chunks};
use prototype_p3d::skin::{
    decode_old_skinned_shape, decode_skeleton, OldSkinnedShape, Skeleton,
};

/// Marker on the CHARACTER ROOT entity (the accumulator's target —
/// NOT an AnimationTarget, so the clip never overwrites it).
#[derive(Component)]
struct CharacterRoot;

/// The decoded game data handed from `main` to the Startup systems.
struct CharacterSource {
    skel: Skeleton,
    bridge_skel: prototype_bevy_bridge::BridgeSkeleton,
    /// (bevy Mesh built from the BridgeSkin, base-color Image name, part index)
    parts: Vec<(prototype_bevy_bridge::BridgeSkin, Option<(Vec<u8>, u32, u32)>)>,
    clip: bevy::animation::AnimationClip,
    loop_params: Option<DofLoop>,
    fps: f32,
    cycle_intervals: u32,
}

/// The root-motion accumulator parameters (block-90/95 rule).
#[derive(Resource)]
struct RootMotionDrive {
    lp: DofLoop,
    fps: f32,
    cycle_intervals: u32,
}

fn main() {
    // --- args (loader-style: the p3d stays user-supplied) ---
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() {
        eprintln!(
            "usage: bevy_window_run <char.p3d> [--anim NAME]  \
             (default anim alex_loco_run_n)"
        );
        std::process::exit(2);
    }
    let path = argv[0].clone();
    let anim = argv
        .iter()
        .position(|a| a == "--anim")
        .and_then(|i| argv.get(i + 1))
        .cloned()
        .unwrap_or_else(|| String::from("alex_loco_run_n"));

    // --- decode the REAL game data (same policy as bevy_render_run) ---
    let buf = std::fs::read(&path).unwrap_or_else(|e| {
        eprintln!("bevy_window_run: read {path}: {e}");
        std::process::exit(1);
    });
    let top = top_level_chunks(&buf).unwrap_or_else(|| {
        eprintln!("bevy_window_run: {path} is not a valid P3D");
        std::process::exit(1);
    });
    let mut shapes: Vec<OldSkinnedShape> = Vec::new();
    for c in top.iter().filter(|c| c.chunk_type == 0x0001_0001) {
        match decode_old_skinned_shape(&buf, c.offset, c.size) {
            Ok(s) => shapes.push(s),
            Err(e) => eprintln!("bevy_window_run: shape at {:#x} skipped: {e}", c.offset),
        }
    }
    if shapes.is_empty() {
        eprintln!("bevy_window_run: no decodable 0x10001 shape in {path}");
        std::process::exit(1);
    }
    let mut skeletons: Vec<Skeleton> = Vec::new();
    for c in top.iter().filter(|c| c.chunk_type == 0x0002_3000) {
        if let Ok(s) = decode_skeleton(&buf, c.offset, c.size) {
            skeletons.push(s);
        }
    }
    // textures: name → decoded base mip (RGBA8)
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
    // bind the skeleton with the MOST vertices (the proven policy)
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
            eprintln!("bevy_window_run: skeleton {:?} not found", best.0);
            std::process::exit(1);
        })
        .clone();
    // one BridgeSkin per (part, mesh); the texture rides with the part
    let mut parts: Vec<(prototype_bevy_bridge::BridgeSkin, Option<(Vec<u8>, u32, u32)>)> =
        Vec::new();
    for s in &shapes {
        if s.skeleton != skel.name {
            continue;
        }
        let tex = pick_tex(&s.name);
        for m in &s.meshes {
            parts.push((extract_skin(m), tex.clone()));
        }
    }
    if parts.is_empty() {
        eprintln!("bevy_window_run: no shape bound to skeleton {:?}", skel.name);
        std::process::exit(1);
    }
    // the anim track + accumulator params
    let tracks = find_anim_tracks(&buf);
    let track = tracks
        .iter()
        .find(|(_, _, n)| *n == anim)
        .map(|(off, span, _)| decode_anim_track(&buf, *off, *span))
        .unwrap_or_else(|| {
            eprintln!(
                "bevy_window_run: track {anim:?} not found ({} tracks)",
                tracks.len()
            );
            std::process::exit(1);
        })
        .unwrap_or_else(|e| {
            eprintln!("bevy_window_run: track decode: {e}");
            std::process::exit(1);
        });
    let fps = track.fps.max(1.0);
    let cycle_intervals = (track.frame_count.max(1.0) as usize).saturating_sub(1).max(1) as u32;
    let loop_params = dof_loop_params(&skel, &track);
    match &loop_params {
        Some(lp) => eprintln!(
            "bevy_window_run: ROOT-MOTION LOOP ON — joint {:?} axis {} travel {:.3} m/cycle",
            skel.joints[lp.joint].name, lp.axis, lp.cycle_travel
        ),
        None => eprintln!(
            "bevy_window_run: no 0x121102 DOF channel on {:?} — plain looping pose",
            track.name
        ),
    }
    let bridge_skel = extract_skeleton(&skel);
    let clip = extract_animation(&track, Some(&skel)).into_animation_clip();

    // --- the WINDOWED app: DefaultPlugins + WindowPlugin (bevy 0.15) ---
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "prototype-rs — REAL 0x121000 clip through bevy AnimationPlayer".into(),
                present_mode: PresentMode::AutoVsync,
                ..default()
            }),
            ..default()
        }),
    )
    // AnimationPlugin rides in DefaultPlugins under the bevy-window
    // feature set (bevy_animation enabled) — no manual add (duplicate
    // plugin insertion panics).
    ;
    app.insert_resource(CharacterSourceHolder(Some(CharacterSource {
        bridge_skel,
        parts,
        clip,
        loop_params,
        fps,
        cycle_intervals,
        skel,
    })));
    app.add_systems(Startup, (setup_world, spawn_character));
    app.add_systems(Update, (start_playback, root_motion_drive, follow_camera));
    app.run();
}

#[derive(Resource)]
struct CharacterSourceHolder(Option<CharacterSource>);

/// Ground + sun + camera (visual context for the USER-SIDE window).
fn setup_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // camera: the follow_camera system drives it every Update; the
    // initial pose just puts it somewhere sane near the origin.
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(3.0, 1.6, 3.0).looking_at(Vec3::new(0.0, 0.9, 0.0), Vec3::Y),
    ));
    // sun
    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_xyz(6.0, 10.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    // ground plane so the forward travel is visible
    let ground_mesh = meshes.add(Plane3d::default().mesh().size(60.0, 60.0));
    let ground_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.28, 0.30, 0.34),
        perceptual_roughness: 0.95,
        ..default()
    });
    commands.spawn((Mesh3d(ground_mesh), MeshMaterial3d(ground_mat)));
}

/// Build the REAL assets + entities: meshes, materials, textures, the
/// clip graph, the joint hierarchy, and one skinned mesh child per part.
fn spawn_character(
    mut commands: Commands,
    mut holder: ResMut<CharacterSourceHolder>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut clips: ResMut<Assets<bevy::animation::AnimationClip>>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut ibs: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    let Some(src) = holder.0.take() else {
        return;
    };
    // real textures → Images
    let mut image_cache: std::collections::HashMap<(u32, u32, u64), Handle<Image>> =
        std::collections::HashMap::new();
    let tex_handle = |images: &mut Assets<Image>,
                      cache: &mut std::collections::HashMap<(u32, u32, u64), Handle<Image>>,
                      tex: &Option<(Vec<u8>, u32, u32)>|
     -> Handle<Image> {
        if let Some((rgba, w, h)) = tex {
            let key = (*w, *h, {
                use std::hash::Hasher;
                let mut hh = std::collections::hash_map::DefaultHasher::new();
                hh.write(rgba);
                hh.finish()
            });
            if let Some(h) = cache.get(&key) {
                return h.clone();
            }
            let image = Image::new(
                Extent3d {
                    width: *w,
                    height: *h,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                rgba.clone(),
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::default(),
            );
            let h = images.add(image);
            cache.insert(key, h.clone());
            return h;
        }
        Handle::default()
    };
    // real materials (one per distinct texture)
    let ib = ibs.add(inverse_bindposes(&src.skel));

    // clip + graph
    let clip_handle = clips.add(src.clip);
    let (graph, node_index) = AnimationGraph::from_clip(clip_handle);
    let graph_handle = graphs.add(graph);

    // entities: char root (accumulator target) + player + joints
    let root = commands
        .spawn((
            Name::new("alex_char_root"),
            CharacterRoot,
            Transform::default(),
        ))
        .id();
    let player_entity = commands
        .spawn((
            Name::new("player"),
            AnimationPlayer::default(),
            AnimationGraphHandle(graph_handle.clone()),
        ))
        .id();

    // real meshes + materials; the FIRST part goes through
    // spawn_skinned_character (joint hierarchy + primary skinned child),
    // the remaining parts spawn extra skinned children with the SAME
    // joint list (the exact pattern spawn_skinned_character uses).
    let first_mesh = meshes.add(src.parts[0].0.clone().into_bevy_mesh());
    let first_mat = materials.add(StandardMaterial {
        base_color_texture: Some(tex_handle(
            &mut images,
            &mut image_cache,
            &src.parts[0].1,
        )),
        perceptual_roughness: 0.75,
        ..default()
    });
    let joints = spawn_skinned_character(
        &mut commands,
        root,
        player_entity,
        &src.bridge_skel,
        &first_mesh,
        &first_mat,
        ib.clone(),
    );
    for (skin, tex) in src.parts.iter().skip(1) {
        let mh = meshes.add(skin.clone().into_bevy_mesh());
        let mt = materials.add(StandardMaterial {
            base_color_texture: Some(tex_handle(&mut images, &mut image_cache, tex)),
            perceptual_roughness: 0.75,
            ..default()
        });
        commands
            .spawn((
                Mesh3d(mh),
                MeshMaterial3d(mt),
                Transform::default(),
                SkinnedMesh {
                    inverse_bindposes: ib.clone(),
                    joints: joints.clone(),
                },
            ))
            .set_parent(root);
    }
    assert_eq!(
        joints.len(),
        src.skel.joints.len(),
        "one entity per skeleton joint"
    );
    eprintln!(
        "bevy_window_run: {} skinned parts, {} joints, clip looping (cycle {} intervals @ {} fps)",
        src.parts.len(),
        joints.len(),
        src.cycle_intervals,
        src.fps
    );

    // start playback: the graph node holding the clip, looping forever
    // (a dedicated Update system — EntityCommands has no get_mut).
    commands.insert_resource(PlaybackStart {
        node: node_index,
        started: false,
    });

    if let Some(lp) = src.loop_params {
        commands.insert_resource(RootMotionDrive {
            lp,
            fps: src.fps,
            cycle_intervals: src.cycle_intervals,
        });
    }
}

/// One-shot playback starter (the AnimationPlayer component is written
/// by animate_targets-compatible systems, not by Commands).
#[derive(Resource)]
struct PlaybackStart {
    node: AnimationNodeIndex,
    started: bool,
}

fn start_playback(mut res: Option<ResMut<PlaybackStart>>, mut q: Query<&mut AnimationPlayer>) {
    let Some(r) = res.as_mut() else {
        return;
    };
    if r.started {
        return;
    }
    let Ok(mut player) = q.get_single_mut() else {
        return;
    };
    player.play(r.node).repeat();
    r.started = true;
    eprintln!("bevy_window_run: AnimationPlayer.play(node {:?}).repeat() STARTED", r.node);
}

/// The block-90/95 root-motion rule, ECS form: the clip animates only
/// the 67 joint entities, so the accumulated `k · cycle_travel` lands
/// on the CHARACTER ROOT (never overwritten by animate_targets) and
/// bevy's GlobalTransform propagation folds it into every joint world
/// pose — the SAME math as the headless apply_root_motion_loop fold.
fn root_motion_drive(
    time: Res<Time>,
    drive: Option<Res<RootMotionDrive>>,
    mut q: Query<&mut Transform, With<CharacterRoot>>,
) {
    let Some(drive) = drive else {
        return;
    };
    let Ok(mut t) = q.get_single_mut() else {
        return;
    };
    // tick index from REAL time (frame-rate independent)
    let k = (time.elapsed_secs() * drive.fps) as u32;
    // apply_root_motion_loop adds k·travel to trans[lp.joint] — here the
    // "skeleton" is the single root, so pass a 1-element slice.
    let mut trans = [[0.0f32; 3]];
    prototype_bevy_bridge::apply_root_motion_loop(
        &mut trans,
        &drive.lp,
        k,
        drive.cycle_intervals,
    );
    let off = trans[0][drive.lp.axis.min(2)];
    let mut p = [0.0f32; 3];
    p[drive.lp.axis.min(2)] = off;
    t.translation = Vec3::new(p[0], p[1], p[2]);
}

/// Slow orbiting follow cam anchored on the character root's REAL world
/// position (the accumulated forward travel included).
fn follow_camera(
    time: Res<Time>,
    root_q: Query<&GlobalTransform, With<CharacterRoot>>,
    mut cam_q: Query<&mut Transform, With<Camera3d>>,
) {
    let Ok(root) = root_q.get_single() else {
        return;
    };
    let Ok(mut cam) = cam_q.get_single_mut() else {
        return;
    };
    let target = root.translation() + Vec3::new(0.0, 0.9, 0.0);
    let t = time.elapsed_secs();
    let orbit = t * 0.25;
    let dist = 3.2;
    cam.translation = target + Vec3::new(orbit.cos() * dist, 1.3, orbit.sin() * dist);
    cam.look_at(target, Vec3::Y);
}

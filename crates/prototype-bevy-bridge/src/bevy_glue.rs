//! Block 85 — BEVY GLUE (enabled by the `bevy` feature).  Turns the
//! plain-f32 extraction structures (see [`crate`]) into real Bevy 0.15
//! assets/components:
//!
//! * [`BridgeSkinExt::into_bevy_mesh`] — a `bevy::render::mesh::Mesh`
//!   with POSITION/NORMAL/UV_0/JOINT_INDEX/JOINT_WEIGHT attributes and
//!   U32 indices.
//! * [`BridgeAnimationExt::into_animation_clip`] — an
//!   `bevy::animation::AnimationClip` whose curves are
//!   `AnimatableKeyframeCurve`s (per-frame samples at `frame/fps`)
//!   wrapped in `AnimatableCurve` targeting `Transform::rotation` /
//!   `Transform::translation`, keyed by
//!   `AnimationTargetId::from_name(&Name)` of each bone.
//! * [`inverse_bindposes`] — per-joint inverse REST world matrices in
//!   bevy's column convention (`SkinnedMeshInverseBindposes` payload).
//! * [`spawn_skinned_character`] — builds the joint entity hierarchy
//!   (one child entity per bone, `Transform` = rest local, each with an
//!   `AnimationTarget`), attaches the skinned mesh child and returns
//!   the joint entities (skeleton-joint order) for animation plumbing.
//!
//! Headless-safe: nothing here needs a GPU or a window — clip/mesh
//! construction is pure data and the unit tests run without a display.

use bevy::animation::{
    animated_field,
    animation_curves::{AnimatableCurve, AnimatableKeyframeCurve, AnimatedField},
    AnimationClip, AnimationTarget, AnimationTargetId,
};
use bevy::asset::{Handle, RenderAssetUsages};
use bevy::core::Name;
use bevy::ecs::entity::Entity;
use bevy::ecs::system::Commands;
use bevy::hierarchy::BuildChildren;
use bevy::math::{Mat4, Quat, Vec3};
use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::prelude::Transform;
use bevy::render::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::render::mesh::{
    Indices, Mesh, Mesh3d, PrimitiveTopology, VertexAttributeValues,
};

use crate::{BridgeAnimation, BridgeSkeleton, BridgeSkin};

/// Mesh + clip conversion.
pub trait BridgeSkinExt {
    /// Build a bevy `Mesh` (TriangleList, MAIN_WORLD | RENDER_WORLD so
    /// the CPU copy survives for re-processing).
    fn into_bevy_mesh(self) -> Mesh;
}

impl BridgeSkinExt for BridgeSkin {
    fn into_bevy_mesh(self) -> Mesh {
        let n = self.positions.len();
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, self.positions);
        if self.normals.len() == n {
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals);
        }
        if self.uvs.len() == n {
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs);
        }
        // Joint indices: bevy 0.15's VertexAttributeValues has NO
        // From<Vec<[u16; 4]>> (only f32/i32/u32 families), so the enum
        // variant is constructed explicitly.
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_JOINT_INDEX,
            VertexAttributeValues::Uint16x4(self.joint_indices),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, self.joint_weights);
        mesh.insert_indices(Indices::U32(self.indices));
        mesh
    }
}

/// Clip conversion.
pub trait BridgeAnimationExt {
    /// Build a bevy `AnimationClip`.  Bones without a channel (or with
    /// fewer than 2 samples) get no curve — their entities keep their
    /// rest `Transform`, matching the CPU pipeline.
    fn into_animation_clip(self) -> AnimationClip;
}

impl BridgeAnimationExt for BridgeAnimation {
    fn into_animation_clip(self) -> AnimationClip {
        let mut clip = AnimationClip::default();
        for curve in self.curves {
            let target = AnimationTargetId::from_name(&Name::new(curve.bone.clone()));
            if curve.rotations.len() >= 2 && curve.rotations.len() == curve.times.len() {
                let keys: Vec<(f32, Quat)> = curve
                    .times
                    .iter()
                    .zip(curve.rotations.iter())
                    .map(|(t, r)| (*t, Quat::from_xyzw(r[0], r[1], r[2], r[3])))
                    .collect();
                if let Ok(c) = AnimatableKeyframeCurve::new(keys) {
                    clip.add_curve_to_target(
                        target,
                        AnimatableCurve::new(animated_field!(Transform::rotation), c),
                    );
                }
            }
            if curve.translations.len() >= 2 && curve.translations.len() == curve.times.len() {
                let keys: Vec<(f32, Vec3)> = curve
                    .times
                    .iter()
                    .zip(curve.translations.iter())
                    .map(|(t, v)| (*t, Vec3::new(v[0], v[1], v[2])))
                    .collect();
                if let Ok(c) = AnimatableKeyframeCurve::new(keys) {
                    clip.add_curve_to_target(
                        target,
                        AnimatableCurve::new(animated_field!(Transform::translation), c),
                    );
                }
            }
        }
        clip
    }
}

/// Per-joint inverse bindposes in bevy's COLUMN convention: the stored
/// row-vector inverse (already computed by `mat4_inverse_affine`)
/// transposed into glam via `from_cols_array_2d` (glam reads the inner
/// arrays as columns, so passing the row-vector array yields exactly
/// the transposed column-convention matrix — see crate docs).
pub fn inverse_bindposes(skel: &prototype_p3d::skin::Skeleton) -> SkinnedMeshInverseBindposes {
    use prototype_p3d::skin::{mat4_inverse_affine, rest_world_matrices};
    let rest = rest_world_matrices(skel);
    SkinnedMeshInverseBindposes::from(
        rest.iter()
            .map(|w| Mat4::from_cols_array_2d(&mat4_inverse_affine(w)))
            .collect::<Vec<Mat4>>(),
    )
}

/// Build the joint entity hierarchy for a skinned character:
///
/// * one child entity per joint, named after the bone (bevy `Name`),
///   with `Transform` = rest LOCAL translation/rotation (bottom-row
///   translation + extracted quat — see crate docs);
/// * each joint gets an `AnimationTarget` with
///   `AnimationTargetId::from_name` (matching the clip's targets) and
///   `player` = `player_entity`;
/// * root-level joints (parent == own index) are parented under
///   `parent`; one skinned mesh child entity carries the
///   `MaterialMeshBundle` and a `SkinnedMesh { inverse_bindposes,
///   joints }` component.
///
/// Returns the joint entities in skeleton-joint order.
pub fn spawn_skinned_character(
    commands: &mut Commands,
    parent: Entity,
    player_entity: Entity,
    skeleton: &BridgeSkeleton,
    mesh: &Handle<Mesh>,
    material: &Handle<StandardMaterial>,
    inverse_bindposes: Handle<SkinnedMeshInverseBindposes>,
) -> Vec<Entity> {
    // Pass 1: spawn every joint with its real rest-local Transform.
    let joint_entities: Vec<Entity> = skeleton
        .joints
        .iter()
        .enumerate()
        .map(|(i, j)| {
            let t = skeleton.local_translations[i];
            let q = skeleton.local_rotations[i];
            commands
                .spawn((
                    Name::new(j.name.clone()),
                    Transform {
                        translation: Vec3::new(t[0], t[1], t[2]),
                        rotation: Quat::from_xyzw(q[0], q[1], q[2], q[3]),
                        ..Default::default()
                    },
                    AnimationTarget {
                        id: AnimationTargetId::from_name(&Name::new(j.name.clone())),
                        player: player_entity,
                    },
                ))
                .id()
        })
        .collect();
    // Pass 2: parent joints into the hierarchy; root-level joints
    // (parent == own index, per the P3D layout) go under `parent`.
    for (i, j) in skeleton.joints.iter().enumerate() {
        if j.parent as usize != i {
            commands
                .entity(joint_entities[j.parent as usize])
                .add_child(joint_entities[i]);
        } else {
            commands.entity(parent).add_child(joint_entities[i]);
        }
    }
    // The skinned mesh entity (child of the character root).  bevy 0.15
    // deprecates MaterialMeshBundle — spawn the Mesh3d + MeshMaterial3d
    // components directly (inserting them auto-inserts the required
    // visibility/inheritance machinery).
    commands
        .spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material.clone()),
            Transform::default(),
            SkinnedMesh {
                inverse_bindposes,
                joints: joint_entities.clone(),
            },
        ))
        .set_parent(parent);
    joint_entities
}

/// Block 95 — the bevy-side root-motion LOOP accumulator (the block-94
/// deferral).  bevy's `AnimationPlayer` loops a cyclic clip WITHIN the
/// clip's own frame range: at the seam the Motion_Root translation
/// snaps from the cycle's last DOF value back to its first, so the
/// character never leaves the origin.  The engine instead accumulates
/// the DOF travel across cycles (`k · cycle_travel` — the block-90
/// `pose_world_matrices_root_motion` rule).  Because the player
/// OVERWRITES every joint's local `Transform` each tick, the offset is
/// folded AFTER evaluation: snapshot the ticked locals, add
/// `k · cycle_travel` to the DOF joint's animated axis, then compose
/// the worlds (`pose_world_matrices_from_locals`).
///
/// `global_tick` is the tick index since playback started (tick k lands
/// on curve frame k % `cycle_frames`, so the cycle index is
/// `global_tick / cycle_frames`).  With `cycle_frames == 0` the offset
/// is 0 (a non-cyclic clip must not accumulate).
pub fn apply_root_motion_loop(
    trans: &mut [[f32; 3]],
    lp: &prototype_p3d::anim::DofLoop,
    global_tick: u32,
    cycle_frames: u32,
) {
    let k = if cycle_frames > 0 {
        global_tick / cycle_frames
    } else {
        0
    };
    if k == 0 || lp.cycle_travel == 0.0 {
        return;
    }
    let idx = (lp.joint as usize).min(trans.len().saturating_sub(1));
    trans[idx][lp.axis] += k as f32 * lp.cycle_travel;
}

#[cfg(test)]
mod bevy_tests {
    use super::*;
    use prototype_p3d::skin::identity4;

    /// The real run-cycle fixture drives a real AnimationClip: 19
    /// frames @ 30 fps → duration 19/30, Pelvis targeted with curves.
    #[test]
    fn fixture_anim_to_clip() {
        let buf = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../prototype-bin/tests/fixtures/alex_loco_run_n_chunk.bin"
        ))
        .expect("fixture");
        use prototype_p3d::anim::decode_anim_track;
        let track = decode_anim_track(&buf, 0, buf.len()).expect("decode");
        // The fixture is the RAW 0x121000 chunk (12-B header + body), so
        // decode it directly (find_anim_tracks expects a whole P3D file).
        assert_eq!(track.name, "alex_loco_run_n");
        // Block 92: no skeleton in this test — DOF static axes fall back
        // to the recorded constants (only affects Motion_Root x; the
        // assertions below do not read it).
        let clip = crate::extract_animation(&track, None).into_animation_clip();
        // The clip loops on its curve domain: 19 frame SAMPLES span 18
        // intervals @ 30 fps = 0.6 s (frame 18 ≡ frame 0 → seamless).
        assert!((clip.duration() - 18.0 / 30.0).abs() < 1e-5, "duration {}", clip.duration());
        let pelvis = AnimationTargetId::from_name(&Name::new("Pelvis"));
        let curves = clip.curves_for_target(pelvis).expect("Pelvis must be animated");
        assert!(curves.len() >= 1, "Pelvis curve count {}", curves.len());
        // Bones without channels must NOT be targeted.
        let missing = AnimationTargetId::from_name(&Name::new("Not_A_Bone"));
        assert!(clip.curves_for_target(missing).is_none());
    }

    /// Block 91 — the FULL real-track census survives the clip
    /// conversion: all 60 animated groups (24 primary ROT + 34
    /// compact ROT + 5 TRAN + 1 DOF) land a curve under their
    /// `AnimationTargetId::from_name` target, and the block-90-only
    /// channels are represented (compact-ROT Shoulder_Con_L / Index_R,
    /// DOF-driven Motion_Root translation).
    #[test]
    fn fixture_full_census_to_clip() {
        let buf = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../prototype-bin/tests/fixtures/alex_loco_run_n_chunk.bin"
        ))
        .expect("fixture");
        use prototype_p3d::anim::decode_anim_track;
        let track = decode_anim_track(&buf, 0, buf.len()).expect("decode");
        // Block 92: no skeleton here — the DOF static-axis fallback is
        // unused by these assertions (they read z endpoints and curve
        // presence only).
        let bridge = crate::extract_animation(&track, None);
        assert_eq!(bridge.curves.len(), 60);
        // Root-motion summary survived extraction.
        let (axis, travel) = bridge.root_motion.expect("root motion");
        assert_eq!(axis, 2);
        assert!((travel - (-3.5945205)).abs() < 1e-4, "travel {travel}");
        let bones: Vec<String> = bridge.curves.iter().map(|c| c.bone.clone()).collect();
        let clip = bridge.into_animation_clip();
        // Every one of the 60 animated bones hits a clip target.
        let mut hit = 0;
        for bone in &bones {
            let id = AnimationTargetId::from_name(&Name::new(bone.clone()));
            if clip.curves_for_target(id).is_some() {
                hit += 1;
            }
        }
        assert_eq!(hit, 60, "every animated bone must have a clip target");
        // The block-90-only channels are represented in the clip.
        for bone in ["Shoulder_Con_L", "Index_R", "Motion_Root", "Ball_L"] {
            let id = AnimationTargetId::from_name(&Name::new(bone));
            assert!(clip.curves_for_target(id).is_some(), "{bone} curve missing");
        }
        // The grapple wrist targets (skeleton joints WITHOUT anim
        // groups on this track) keep their rest pose.
        for bone in ["Root_Grapple", "R_Wrist_Grapple"] {
            let id = AnimationTargetId::from_name(&Name::new(bone));
            assert!(clip.curves_for_target(id).is_none(), "{bone} must be unanimated");
        }
    }

    /// A trivial BridgeSkin round-trips into a bevy Mesh with the
    /// skinning attributes and U32 indices.
    #[test]
    fn skin_to_mesh_attributes() {
        let skin = crate::BridgeSkin {
            name: "m".into(),
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.0, 0.0]; 3],
            joint_indices: vec![[0, 0, 0, 0]; 3],
            joint_weights: vec![[1.0, 0.0, 0.0, 0.0]; 3],
            indices: vec![0, 1, 2],
        };
        let mesh = skin.into_bevy_mesh();
        assert!(mesh.attribute(Mesh::ATTRIBUTE_POSITION).is_some());
        assert!(mesh.attribute(Mesh::ATTRIBUTE_JOINT_INDEX).is_some());
        assert!(mesh.attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT).is_some());
        assert!(mesh.attribute(Mesh::ATTRIBUTE_NORMAL).is_some());
        let _: Handle<Mesh> = Handle::default();
        let _ = identity4();
    }

    /// Block 92 — THE ANIMATIONPLAYER TICK TEST.  The REAL
    /// alex_loco_run_n clip drives a real bevy `AnimationPlayer` over
    /// `spawn_skinned_character`'s 67-joint hierarchy inside a headless
    /// `App` (TaskPool + Time + Asset + Animation plugins, no window no
    /// GPU).  Per ticked frame, every skeleton joint's `Transform` is
    /// compared against `pose_local_transforms` — the CPU-pipeline
    /// mirror — so the bevy playback path is pinned to the proven CPU
    /// pose data byte-for-byte at the transform level.
    ///
    /// What must hold:
    /// * all 24 primary + 34 compact ROT-driven bones and the 5 TRAN
    ///   bones track the CPU pose every frame (quat dot |d| > 0.99999,
    ///   translation delta < 1e-4);
    /// * the DOF-driven Motion_Root translation (constants on x/y,
    ///   per-frame z) matches the CPU fold;
    /// * skeleton joints WITHOUT anim groups (Root_Grapple /
    ///   R_Wrist_Grapple) keep their REST transform at every tick;
    /// * after one full cycle the player LOOPS back to the frame-0 pose
    ///   and reports a completion (RepeatAnimation::Forever).
    #[test]
    fn fixture_animationplayer_tick_matches_pose() {
        use bevy::animation::prelude::{
            AnimationGraph, AnimationGraphHandle, AnimationPlayer, AnimationPlugin,
        };
        use bevy::app::App;
        use bevy::asset::AssetPlugin;
        // TaskPoolPlugin lives in bevy_core in 0.15 (not bevy::app).
        use bevy::core::TaskPoolPlugin;
        use bevy::ecs::world::CommandQueue;
        // bevy_time is a direct dep (no facade feature in bevy 0.15) —
        // the SAME crate instance bevy_animation's Res<Time> uses.
        use bevy_time::{TimePlugin, TimeUpdateStrategy};

        use crate::pose_local_transforms;
        use prototype_p3d::anim::decode_anim_track;

        // ---- real game data: the run track + the 67-joint skeleton.
        let buf = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../prototype-bin/tests/fixtures/alex_loco_run_n_chunk.bin"
        ))
        .expect("anim fixture");
        let track = decode_anim_track(&buf, 0, buf.len()).expect("decode");
        assert_eq!(track.name, "alex_loco_run_n");
        let fix = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../prototype-p3d/tests/fixtures/skeleton_alex_reg_body.p3d"
        ))
        .expect("skeleton fixture");
        // the fixture carries a 12-B P3D header first (block-90 prelude
        // entry: size word at +8).
        let skel = prototype_p3d::skin::decode_skeleton_at(&fix, 12).expect("skeleton");
        assert_eq!(skel.joints.len(), 67);
        let bridge_skel = crate::extract_skeleton(&skel);
        // Block 92: the REAL skeleton rides along so the DOF static axes
        // fold the joint's REST translation — compose_pose semantics, the
        // exact fold the tick comparison below asserts.
        let bridge_anim = crate::extract_animation(&track, Some(&skel));
        assert_eq!(bridge_anim.curves.len(), 60, "60 animated groups");
        let clip = bridge_anim.into_animation_clip();
        assert!((clip.duration() - 18.0 / 30.0).abs() < 1e-5);

        // ---- headless app (no window, no renderer, no hierarchy/transform
        // propagation — the tick test compares LOCAL transforms).
        let mut app = App::new();
        app.add_plugins((
            TaskPoolPlugin::default(),
            TimePlugin,
            AssetPlugin::default(),
            AnimationPlugin,
        ));
        // Deterministic playback: every update advances EXACTLY one frame
        // (TimeUpdateStrategy::ManualDuration pins time_system's delta;
        // Automatic would use wall-clock time).  1/30 s matches the
        // track's fps, so each update lands on an exact curve key.
        let dt = std::time::Duration::from_secs_f32(1.0 / 30.0);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(dt));

        // ---- assets: clip -> graph (bevy 0.15 graph API), dangling
        // mesh/material handles (the skinned mesh entity is inert
        // headless — no render systems run).
        let clip_handle = app
            .world_mut()
            .resource_mut::<bevy::asset::Assets<AnimationClip>>()
            .add(clip);
        let (graph, node_index) = AnimationGraph::from_clip(clip_handle);
        let graph_handle = app
            .world_mut()
            .resource_mut::<bevy::asset::Assets<AnimationGraph>>()
            .add(graph);
        let mesh_handle: Handle<Mesh> = Handle::default();
        let mat_handle: Handle<StandardMaterial> = Handle::default();
        let ib_handle: Handle<SkinnedMeshInverseBindposes> = Handle::default();

        // ---- spawn the real joint hierarchy + the player entity via
        // Commands (spawn_skinned_character needs Commands).  Every world
        // borrow is SCOPED — `app.update()` below re-borrows fresh.
        let mut queue = CommandQueue::default();
        let (player_entity, joint_entities) = {
            let world = app.world_mut();
            let mut commands = Commands::new(&mut queue, world);
            let root = commands.spawn(Name::new("alex_char_root")).id();
            let player_entity = commands
                .spawn((
                    Name::new("player"),
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
            (player_entity, joints)
        };
        {
            let world = app.world_mut();
            queue.apply(world);
        }
        assert_eq!(joint_entities.len(), 67, "one entity per skeleton joint");

        // WARM-UP (player not playing yet — no ActiveAnimation exists, so
        // these updates advance nothing): the graph asset event is read by
        // thread_animation_graphs during these updates, which is required
        // before animate_targets can evaluate any curve.
        for _ in 0..2 {
            app.update();
        }

        // start playback: the graph node that holds the run clip, looping
        // forever (cyclic track).  From here every update advances exactly
        // 1/30 s of animation time.
        app.world_mut()
            .entity_mut(player_entity)
            .get_mut::<AnimationPlayer>()
            .expect("player")
            .play(node_index)
            .repeat();

        // ---- unanimated-bone bookkeeping (rest transforms must survive).
        let grapple_i = skel.find("Root_Grapple").expect("Root_Grapple joint");
        let rgrapple_i = skel.find("R_Wrist_Grapple").expect("R_Wrist_Grapple joint");
        let (grapple_rest, rgrapple_rest) = {
            let world = app.world_mut();
            (
                *world.get::<Transform>(joint_entities[grapple_i]).unwrap(),
                *world.get::<Transform>(joint_entities[rgrapple_i]).unwrap(),
            )
        };
        assert_eq!(
            grapple_rest.translation,
            Vec3::new(
                bridge_skel.local_translations[grapple_i][0],
                bridge_skel.local_translations[grapple_i][1],
                bridge_skel.local_translations[grapple_i][2],
            ),
            "grapple joint rest translation"
        );

        // ---- TICK LOOP: 18 one-frame updates.  The first update AFTER
        // play() advances one frame, so tick k lands on frame k (seek =
        // k/30); tick 18's seek 18/30 wraps to 0 — the cyclic seam, frame
        // 18 ≡ frame 0.  Frame index for the CPU reference = k % 18.
        let mut worst_dot = 1.0f32;
        let mut worst_trans = 0.0f32;
        for k in 1..=18u32 {
            app.update();

            // snapshot every joint's ticked transform, then compare
            // OUTSIDE the ECS borrow.
            let snaps: Vec<Transform> = {
                let world = app.world_mut();
                joint_entities
                    .iter()
                    .map(|&e| *world.get::<Transform>(e).expect("joint Transform"))
                    .collect()
            };
            // per-frame CPU reference pose (the proven mirror).
            let (rots, trans) = pose_local_transforms(&skel, &track, k % 18);
            for (i, t) in snaps.iter().enumerate() {
                let r = rots[i];
                let q = t.rotation;
                let tv = t.translation;
                // quat agreement, sign-insensitive (double cover).
                let dot = (q.x * r[0] + q.y * r[1] + q.z * r[2] + q.w * r[3]).abs();
                assert!(
                    dot > 0.99999,
                    "joint {} (idx {i}) frame {k}: quat dot {dot} — bevy {q:?} vs cpu {r:?}",
                    skel.joints[i].name
                );
                worst_dot = worst_dot.min(dot);
                for k in 0..3 {
                    let d = (tv[k] - trans[i][k]).abs();
                    assert!(
                        d < 1e-4,
                        "joint {} (idx {i}) frame {k}: trans axis {k} off {d} — bevy {tv:?} vs cpu {:?}",
                        skel.joints[i].name,
                        trans[i]
                    );
                    worst_trans = worst_trans.max(d);
                }
            }

            // unanimated joints NEVER move off rest.
            for (idx, rest) in [(grapple_i, grapple_rest), (rgrapple_i, rgrapple_rest)] {
                let t = &snaps[idx];
                assert!(
                    t.translation.distance_squared(rest.translation) < 1e-8
                        && (t.rotation.w * rest.rotation.w
                            + t.rotation.x * rest.rotation.x
                            + t.rotation.y * rest.rotation.y
                            + t.rotation.z * rest.rotation.z)
                            .abs()
                            > 0.99999,
                    "unanimated joint {} moved at frame {k}",
                    skel.joints[idx].name
                );
            }
        }

        // ---- after 18 ticks the player wrapped once: tick 18's seek
        // 18/30 wrapped to 0 (frame 18 ≡ frame 0, the cyclic seam) ->
        // completions >= 1.
        let completed: u32 = {
            let world = app.world_mut();
            let player = world
                .get::<AnimationPlayer>(player_entity)
                .expect("player");
            player
                .playing_animations()
                .map(|(_, active)| active.completions())
                .sum()
        };
        assert!(completed >= 1, "run clip must have looped once, got {completed}");
        println!(
            "fixture_animationplayer_tick_matches_pose: 18 ticks over 67 joints, \
             worst quat dot {worst_dot:.6}, worst translation delta {worst_trans:.2e}, \
             completions {completed}"
        );
    }

    /// BLOCK 95 — the bevy-side root-motion loop accumulator against the
    /// REAL `AnimationPlayer` + the REAL run track.  Without the
    /// accumulator the player's cyclic loop snaps Motion_Root's local z
    /// back by one cycle_travel (≈ −3.595 m, z runs −0.275 → −3.87) at
    /// every 18-tick seam; with [`apply_root_motion_loop`] the
    /// accumulated root z continues EXACTLY where the previous cycle
    /// ended (step at the seam = one frame's stride, not the snap), and
    /// the bevy-folded DOF-joint world axis position matches the CPU
    /// `pose_world_matrices_root_motion` reference tick-for-tick — the
    /// two independent accumulator implementations cross-validated.
    #[test]
    fn fixture_root_motion_loop_continuous_across_seams() {
        use bevy::animation::prelude::{
            AnimationGraph, AnimationGraphHandle, AnimationPlayer, AnimationPlugin,
        };
        use bevy::app::App;
        use bevy::asset::AssetPlugin;
        use bevy::core::TaskPoolPlugin;
        use bevy::ecs::world::CommandQueue;
        use bevy_time::{TimePlugin, TimeUpdateStrategy};

        use prototype_p3d::anim::{
            decode_anim_track, dof_loop_params, pose_world_matrices_from_locals,
            pose_world_matrices_root_motion,
        };

        // ---- real game data (same fixtures as the tick test).
        let buf = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../prototype-bin/tests/fixtures/alex_loco_run_n_chunk.bin"
        ))
        .expect("anim fixture");
        let track = decode_anim_track(&buf, 0, buf.len()).expect("decode");
        let fix = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../prototype-p3d/tests/fixtures/skeleton_alex_reg_body.p3d"
        ))
        .expect("skeleton fixture");
        let skel = prototype_p3d::skin::decode_skeleton_at(&fix, 12).expect("skeleton");
        let cycle_frames = 18u32;

        // ---- the loop parameters, pinned to the block-90/94 census.
        let lp = dof_loop_params(&skel, &track).expect("run track carries the 0x121102 DOF");
        assert_eq!(skel.joints[lp.joint].name, "Motion_Root");
        assert_eq!(lp.axis, 2, "the DOF drives z");
        assert!(
            (lp.v0 - (-0.275)).abs() < 0.01,
            "DOF frame-0 value ≈ −0.275, got {}",
            lp.v0
        );
        assert!(
            (lp.cycle_travel - (-3.595)).abs() < 0.05,
            "cycle travel ≈ −3.595 m (z −0.275 → −3.87), got {}",
            lp.cycle_travel
        );

        // ---- headless app, real AnimationPlayer (tick-test setup).
        let bridge_skel = crate::extract_skeleton(&skel);
        let bridge_anim = crate::extract_animation(&track, Some(&skel));
        let clip = bridge_anim.into_animation_clip();
        let mut app = App::new();
        app.add_plugins((
            TaskPoolPlugin::default(),
            TimePlugin,
            AssetPlugin::default(),
            AnimationPlugin,
        ));
        let dt = std::time::Duration::from_secs_f32(1.0 / 30.0);
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
        let mesh_handle: Handle<Mesh> = Handle::default();
        let mat_handle: Handle<StandardMaterial> = Handle::default();
        let ib_handle: Handle<SkinnedMeshInverseBindposes> = Handle::default();
        let mut queue = CommandQueue::default();
        let (player_entity, joint_entities) = {
            let world = app.world_mut();
            let mut commands = Commands::new(&mut queue, world);
            let root = commands.spawn(Name::new("alex_char_root")).id();
            let player_entity = commands
                .spawn((
                    Name::new("player"),
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
            (player_entity, joints)
        };
        {
            let world = app.world_mut();
            queue.apply(world);
        }
        for _ in 0..2 {
            app.update();
        }
        app.world_mut()
            .entity_mut(player_entity)
            .get_mut::<AnimationPlayer>()
            .expect("player")
            .play(node_index)
            .repeat();

        // ---- 40 one-frame ticks (2 full cycles + 4): snapshot the DOF
        // joint's RAW local z, apply the accumulator, fold the worlds and
        // compare against the CPU root-motion reference.
        let ji = lp.joint;
        let mut raw_zs: Vec<f32> = Vec::new();
        let mut acc_zs: Vec<f32> = Vec::new();
        let mut worst_world_delta = 0.0f32;
        for k in 1..=40u32 {
            app.update();
            let (rots, mut trans): (Vec<[f32; 4]>, Vec<[f32; 3]>) = {
                let world = app.world_mut();
                joint_entities
                    .iter()
                    .map(|&e| {
                        let t = world.get::<Transform>(e).expect("joint Transform");
                        (
                            [t.rotation.x, t.rotation.y, t.rotation.z, t.rotation.w],
                            [t.translation.x, t.translation.y, t.translation.z],
                        )
                    })
                    .unzip()
            };
            raw_zs.push(trans[ji][lp.axis]);
            apply_root_motion_loop(&mut trans, &lp, k, cycle_frames);
            acc_zs.push(trans[ji][lp.axis]);
            // Cross-validate against the CPU accumulator (block-90 rule).
            let worlds = pose_world_matrices_from_locals(&skel, &rots, &trans);
            let reference = pose_world_matrices_root_motion(&skel, &track, k as f32, cycle_frames as usize);
            let d = (worlds[ji][3][lp.axis] - reference[ji][3][lp.axis]).abs();
            worst_world_delta = worst_world_delta.max(d);
            assert!(
                d < 1e-4,
                "tick {k}: bevy+accumulator root world z {} vs CPU root-motion {} (delta {d})",
                worlds[ji][3][lp.axis],
                reference[ji][3][lp.axis]
            );
        }

        // ---- the SEAM: tick 17 (frame 17, cycle 0) → tick 18 (frame 0,
        // cycle 1 — the player wrapped).  RAW locals snap back by
        // ~cycle_travel; the accumulated axis continues with one frame's
        // stride (acc(tick 18) = v(0) + travel = v(18), the cycle's last
        // value).  Second seam at ticks 35→36.
        let raw_snap = (raw_zs[17] - raw_zs[16]).abs(); // ticks 17→18
        let seam_step_1 = (acc_zs[17] - acc_zs[16]).abs(); // ticks 17→18
        let seam_step_2 = (acc_zs[35] - acc_zs[34]).abs(); // ticks 35→36
        assert!(
            raw_snap > 2.0,
            "RAW bevy loop must snap back at the seam (cycle_travel ≈ 3.6), got {raw_snap}"
        );
        assert!(
            seam_step_1 < 0.35 && seam_step_2 < 0.35,
            "accumulated root z must continue at the seams with a ~0.2 m stride, \
             got {seam_step_1} / {seam_step_2}"
        );
        // ---- forward progress, EXACT: acc(tick 18) = v(0) + 1·travel
        // (cycle 1 start), acc(tick 40) = v(4) + 2·travel (frame 4 of
        // cycle 2).  The step between them must equal the analytic
        // v(0) − v(4) − travel (z decreasing = forward).
        let dof_v4 = track
            .groups
            .iter()
            .find_map(|g| g.dof.as_ref())
            .and_then(|d| d.value_at_frame(4))
            .expect("DOF value at frame 4");
        let forward = acc_zs[17] - acc_zs[39]; // tick 18 → tick 40
        let expected = lp.v0 - dof_v4 - lp.cycle_travel;
        assert!(
            (forward - expected).abs() < 0.05 && forward > 3.0,
            "the accumulator must advance the root by v(0) − v(4) − travel = {expected:.3} m \
             over ticks 18→40, got {forward:.3}"
        );
        println!(
            "fixture_root_motion_loop_continuous_across_seams: 40 ticks, joint {}, \
             raw seam snap {raw_snap:.3} m, accumulated seam steps {:.3}/{:.3} m, \
             forward ticks 18→40 {forward:.3} m (expected {expected:.3}), \
             worst bevy-vs-CPU world delta {worst_world_delta:.2e}",
            skel.joints[lp.joint].name,
            seam_step_1,
            seam_step_2,
        );
    }
}

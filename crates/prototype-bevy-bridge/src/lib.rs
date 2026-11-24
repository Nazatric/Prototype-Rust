//! Block 85 — BEVY BRIDGE (extraction layer; the `bevy` feature adds the
//! glue that turns these plain-f32 structures into real Bevy assets).
//!
//! Loader-style: the game files stay user-supplied; this crate only
//! reshapes what [`prototype_p3d`] already decoded into the shapes Bevy
//! 0.15 wants:
//!
//! * [`extract_skeleton`] — a [`Skeleton`] becomes a [`BridgeSkeleton`]:
//!   per-joint parent-relative LOCAL translation (the rest matrix's
//!   bottom row — Radical GEM row-vector layout, PROVEN block 85) and
//!   LOCAL rotation quat extracted from the rest 3×3.
//! * [`extract_skin`] — a [`SkinMesh`] becomes a [`BridgeSkin`]: palette
//!   -mapped ABSOLUTE joint indices (`u16×4`, bevy's
//!   `ATTRIBUTE_JOINT_INDEX` format) and the REAL per-vertex blend
//!   weights.  Block 88 CORRECTION: the weight stream DOES exist —
//!   stream B carries (w0, w1), stream A @48 carries w2 and the file
//!   complements w3 = 1 - w0 - w1 - w2 (proven on the real body mesh:
//!   vertex 500 reads (0.4877, 0.2866, 0.1681) with slots [9, 11, 12,
//!   7], four weights summing to 1).  The old PREFIX-EQUAL assumption
//!   (block 85) remains the fallback for meshes without stream B.
//! * [`extract_animation`] — an [`AnimTrack`] becomes a
//!   [`BridgeAnimation`]: per animated bone, per-frame samples at
//!   `t = frame / fps` with the SAME clamped key lookup
//!   `key_indices[min(frame, count-1)]` as `pose_world_matrices`, so a
//!   Bevy `AnimationPlayer` driven by this clip reproduces the CPU
//!   pipeline exactly.
//!
//! # Coordinate conventions (PROVEN block 85)
//!
//! The file stores Radical GEM row-vector matrices (`p' = p·M`,
//! translation in the bottom row).  Bevy is column-vector
//! (`v' = M·v`, translation in the last column).  The two linear maps
//! agree EXACTLY when a bevy `Quat`/`Transform` is built from the same
//! xyzw numbers: the row-vector quat matrix of `q` is the transpose of
//! the column-vector quat matrix of `q` (see the `mat4_apply_rotation_
//! row_vector` and `bevy_quat_matches_row_vector_world` tests), so the
//! bridge passes quats through UNCHANGED.  For 4×4s, bevy's
//! `Mat4::from_cols_array_2d` of the stored row-vector array is the
//! transpose — exactly the column-convention matrix.

use prototype_p3d::anim::{AnimTrack, Key};
use prototype_p3d::skin::{SkinMesh, Skeleton};

#[cfg(feature = "bevy")]
pub mod bevy_glue;
#[cfg(feature = "bevy")]
pub use bevy_glue::{
    apply_root_motion_loop, BridgeAnimationExt, BridgeSkinExt, inverse_bindposes,
    spawn_skinned_character,
};

// ---------------------------------------------------------------------------
// Extraction output types (pure f32 arrays — no bevy, no glam)
// ---------------------------------------------------------------------------

/// One skeleton joint: name + parent index (mirrors [`SkeletonJoint`]).
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeJoint {
    pub name: String,
    pub parent: u32,
}

/// A skeleton reshaped for Bevy: parent-relative LOCAL transforms per
/// joint (what a Bevy entity hierarchy + `AnimationPlayer` consume).
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeSkeleton {
    pub name: String,
    pub joints: Vec<BridgeJoint>,
    /// Rest LOCAL translation per joint (rest matrix bottom row).
    pub local_translations: Vec<[f32; 3]>,
    /// Rest LOCAL rotation per joint as `[x, y, z, w]`, extracted from
    /// the rest 3×3 (row-vector) — bevy `Quat::from_xyzw` of these
    /// numbers reproduces the same physical rotation.
    pub local_rotations: Vec<[f32; 4]>,
}

/// A skinned mesh reshaped for Bevy: absolute joint indices + explicit
/// prefix-equal weights.
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeSkin {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Stream-A UVs (block-84's proven render path uses these).
    pub uvs: Vec<[f32; 2]>,
    /// Palette-mapped ABSOLUTE skeleton joint indices, `u16×4` per
    /// vertex (bevy `ATTRIBUTE_JOINT_INDEX` = `Uint16x4`).
    pub joint_indices: Vec<[u16; 4]>,
    /// PREFIX-EQUAL weights mirroring `prototype_p3d::skin::weights_for`
    /// (slots 0..=last-nonzero blend equally; all-zero = 100% slot 0),
    /// summing to 1 (bevy `ATTRIBUTE_JOINT_WEIGHT` = `Float32x4`).
    pub joint_weights: Vec<[f32; 4]>,
    /// Triangle indices as `u32` (bevy `Indices::U32`).
    pub indices: Vec<u32>,
}

/// One animated bone's per-frame samples (Bevy curve source data).
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeBoneCurve {
    /// The bone (skeleton joint) this curve targets — matches
    /// [`BridgeSkeleton::joints`] names, and the `AnimationTargetId`
    /// the bevy glue assigns to each bone entity.
    pub bone: String,
    /// Sample times in SECONDS (`frame / fps`), ascending.
    pub times: Vec<f32>,
    /// Rotation per sample `[x, y, z, w]` (same length as `times`) —
    /// empty when the bone has no ROT channel.
    pub rotations: Vec<[f32; 4]>,
    /// Translation per sample (same length as `times`) — empty when the
    /// bone has no TRAN channel.
    pub translations: Vec<[f32; 3]>,
}

/// An animation track reshaped for Bevy (one `AnimationClip`).
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeAnimation {
    pub name: String,
    /// Loop period in seconds: `(frame_count - 1) / fps` — the CURVE
    /// domain bevy's AnimationPlayer loops on.  For a cyclic track the
    /// last frame equals the first (frame 18 ≡ frame 0 on the real run
    /// cycle), so N frame SAMPLES span N-1 frame INTERVALS; looping at
    /// this period wraps seamlessly into the clamped first sample.
    pub duration: f32,
    /// The track's cyclic flag — the bevy glue maps it to
    /// `AnimationPlayer::repeat()` (RepeatAnimation::Forever).
    pub cyclic: bool,
    /// One curve per animated bone (bones without channels keep their
    /// rest Transform — the AnimationPlayer only overrides targets).
    pub curves: Vec<BridgeBoneCurve>,
    /// Root-motion DOF summary (block 91): `(axis, cycle_travel)` when
    /// the track carries a 0x121102 Vector-1-DOF channel — `axis` is
    /// 0/1/2 (x/y/z) and `cycle_travel = v(last key) - v(0)` along that
    /// axis, matching the translation curve's one-cycle span.  A
    /// looping driver that wants the character to KEEP moving forward
    /// must add `k * cycle_travel` to the DOF axis after `k` loops
    /// (the CPU pipeline's `pose_world_matrices_root_motion`
    /// accumulates exactly this); the clip itself covers one cycle.
    /// `None` = no DOF channel (no root motion).
    pub root_motion: Option<(u16, f32)>,
}

// ---------------------------------------------------------------------------
// Extraction functions
// ---------------------------------------------------------------------------

/// Extract a Bevy-ready skeleton (local translations from the rest
/// bottom row, local rotations from the rest 3×3).
pub fn extract_skeleton(skel: &Skeleton) -> BridgeSkeleton {
    let mut out = BridgeSkeleton {
        name: skel.name.clone(),
        joints: Vec::with_capacity(skel.joints.len()),
        local_translations: Vec::with_capacity(skel.joints.len()),
        local_rotations: Vec::with_capacity(skel.joints.len()),
    };
    for j in &skel.joints {
        out.joints.push(BridgeJoint { name: j.name.clone(), parent: j.parent });
        out.local_translations.push([j.rest[3][0], j.rest[3][1], j.rest[3][2]]);
        out.local_rotations.push(mat3_row_to_quat([
            [j.rest[0][0], j.rest[0][1], j.rest[0][2]],
            [j.rest[1][0], j.rest[1][1], j.rest[1][2]],
            [j.rest[2][0], j.rest[2][1], j.rest[2][2]],
        ]));
    }
    out
}

/// Extract a Bevy-ready skinned mesh (palette-mapped joint indices +
/// prefix-equal weights, mirroring the block-85-fixed `skin_positions`).
pub fn extract_skin(mesh: &SkinMesh) -> BridgeSkin {
    let mut out = BridgeSkin {
        name: mesh.name.clone(),
        positions: Vec::with_capacity(mesh.positions.len()),
        normals: Vec::with_capacity(mesh.normals.len()),
        uvs: mesh.uvs.clone(),
        joint_indices: Vec::with_capacity(mesh.bone_indices.len()),
        joint_weights: Vec::with_capacity(mesh.bone_indices.len()),
        indices: mesh.indices.iter().map(|&i| i as u32).collect(),
    };
    out.positions.extend_from_slice(&mesh.positions);
    out.normals.extend_from_slice(&mesh.normals);
    for (vi, b) in mesh.bone_indices.iter().enumerate() {
        // Palette-relative slot index → ABSOLUTE skeleton joint index.
        let ji: [u16; 4] = [
            palette_joint(&mesh.palette, b[0]),
            palette_joint(&mesh.palette, b[1]),
            palette_joint(&mesh.palette, b[2]),
            palette_joint(&mesh.palette, b[3]),
        ];
        out.joint_indices.push(ji);
        // Block 88: the REAL per-vertex weights when the mesh carries
        // the stream-B weight stream; prefix-equal fallback otherwise
        // (identical to skin.rs's weights_for).
        if let Some(&w) = mesh.weights.get(vi) {
            out.joint_weights.push(w);
        } else {
            let mut last = 0usize;
            for (i, &x) in b.iter().enumerate() {
                if x != 0 {
                    last = i;
                }
            }
            let n = last + 1;
            let w = 1.0 / n as f32;
            let mut wt = [0f32; 4];
            for o in wt.iter_mut().take(n) {
                *o = w;
            }
            out.joint_weights.push(wt);
        }
    }
    out
}

/// Extract a Bevy-ready animation clip (per-frame samples with the same
/// clamped lookup `pose_world_matrices` uses).
///
/// Block 91: the block-90 channels are now FOLDED —
/// * compact-ROT (0x121114) drives the bone's rotation only where no
///   primary 0x121112 exists (`g.rot.or(g.rot_compact)`, the pose
///   pipeline's preference — no real run-track group carries both);
/// * the Vector-1-DOF root-motion channel (0x121102) overrides ONE
///   translation axis per frame (`dyn_index`): the other two axes come
///   from the TRAN channel when the bone has one, else from the DOF's
///   recorded constants (on the real run track they equal the
///   Motion_Root rest translation, i.e. the frame-0 state).
/// The four IK groups (leg/arm_left/right) also emit curves — they
/// target names that are NOT skeleton joints, so a Bevy AnimationPlayer
/// leaves them unmatched, exactly like the CPU pose pipeline ignores
/// them.
pub fn extract_animation(
    track: &AnimTrack,
    rest_skel: Option<&Skeleton>,
) -> BridgeAnimation {
    let fps = if track.fps > 0.0 { track.fps } else { 30.0 };
    let frames = track.frame_count.max(1.0) as usize;
    let mut out = BridgeAnimation {
        name: track.name.clone(),
        duration: (frames.max(1) - 1) as f32 / fps,
        cyclic: track.cyclic != 0,
        curves: Vec::new(),
        root_motion: None,
    };
    for g in &track.groups {
        let mut curve = BridgeBoneCurve {
            bone: g.bone_name.clone(),
            times: (0..frames).map(|f| f as f32 / fps).collect(),
            rotations: Vec::new(),
            translations: Vec::new(),
        };
        // Primary 0x121112 preferred over compact 0x121114.
        if let Some(rot) = g.rot.as_ref().or(g.rot_compact.as_ref()) {
            curve.rotations = (0..frames)
                .map(|f| match rot_key(rot, f as u32) {
                    Key::Rot { x, y, z, w } => [x, y, z, w],
                    _ => [0.0, 0.0, 0.0, 1.0],
                })
                .collect();
        }
        if g.tran.is_some() || g.dof.is_some() {
            let dof = g.dof.as_ref();
            // Block 92 CORRECTION: the static axes fold the joint's REST
            // translation — compose_pose semantics.  The old fold filled
            // them with the DOF's recorded constants, which do NOT equal
            // the rest on the real track (Motion_Root x constant 0.06216
            // vs rest 0.0) and made a bevy AnimationPlayer diverge from
            // the CPU pipeline.  Without a skeleton the constants remain
            // the only available fallback (documented approximation).
            let static_rest = rest_skel.and_then(|s| {
                s.joints
                    .iter()
                    .find(|j| j.name == g.bone_name)
                    .map(|j| [j.rest[3][0], j.rest[3][1], j.rest[3][2]])
            });
            curve.translations = (0..frames)
                .map(|f| {
                    let mut t = match g.tran.as_ref().map(|c| tran_key(c, f as u32)) {
                        Some(Key::Tran { x, y, z }) => [x, y, z],
                        // No TRAN channel: keep the joint's REST
                        // translation on the static axes; the DOF then
                        // overrides its one axis below.
                        _ => static_rest
                            .unwrap_or_else(|| dof.map(|d| d.constants).unwrap_or([0.0; 3])),
                    };
                    if let Some(d) = dof {
                        if let Some(v) = d.value_at_frame(f as usize) {
                            t[(d.dyn_index as usize).min(2)] = v;
                        }
                    }
                    t
                })
                .collect();
        }
        out.curves.push(curve);
    }
    // Root-motion summary from the DOF channel (block 91).
    if let Some(g) = track.groups.iter().find(|g| g.dof.is_some()) {
        let d = g.dof.as_ref().unwrap();
        if let (Some(v0), Some(v1)) = (d.value_at_frame(0), d.value_at_frame(usize::MAX)) {
            out.root_motion = Some((d.dyn_index, v1 - v0));
        }
    }
    out
}

/// Per-joint LOCAL pose transforms at `frame` — exactly what the Bevy
/// AnimationPlayer writes into each bone entity's `Transform` when
/// driven by `extract_animation`'s clip (animated bones get the anim
/// ROT/TRAN; unanimated bones keep their rest local).  Used by the
/// convention-equivalence test against `pose_world_matrices`.
pub fn pose_local_transforms(
    skel: &Skeleton,
    track: &AnimTrack,
    frame: u32,
) -> (Vec<[f32; 4]>, Vec<[f32; 3]>) {
    let bridge = extract_skeleton(skel);
    let mut rots = bridge.local_rotations.clone();
    let mut trans = bridge.local_translations.clone();
    for (i, j) in skel.joints.iter().enumerate() {
        if let Some(g) = track.groups.iter().find(|g| g.bone_name == j.name) {
            // Block 91: mirror the new clip folding — compact-ROT feeds
            // the rotation where no primary ROT exists, and the DOF
            // channel overrides its one translation axis (the CPU pose
            // pipeline's exact semantics).
            let rot = g.rot.as_ref().or(g.rot_compact.as_ref());
            if let Some(rot) = rot {
                if rot.count > 0 && !rot.keys.is_empty() {
                    if let Key::Rot { x, y, z, w } = rot_key(rot, frame) {
                        rots[i] = [x, y, z, w];
                    }
                }
            }
            if let Some(tran) = &g.tran {
                if tran.count > 0 && !tran.keys.is_empty() {
                    if let Key::Tran { x, y, z } = tran_key(tran, frame) {
                        trans[i] = [x, y, z];
                    }
                }
            }
            if let Some(d) = &g.dof {
                if let Some(v) = d.value_at_frame(frame as usize) {
                    trans[i][(d.dyn_index as usize).min(2)] = v;
                }
            }
        }
    }
    (rots, trans)
}

/// Column-convention world matrix from a bevy-style local transform —
/// the reference implementation the `bevy_quat_matches_row_vector_world`
/// test compares the proven row-vector pipeline against (pure f32, no
/// glam): `W = T(local_trans) · R(local_quat) · W_parent` with the
/// translation in the LAST column.
pub fn column_world(
    trans: [f32; 3],
    quat: [f32; 4],
    parent_world: Option<&[[f32; 4]; 4]>,
) -> [[f32; 4]; 4] {
    let (x, y, z, w) = (quat[0], quat[1], quat[2], quat[3]);
    // COLUMN-vector quat matrix (bevy/glam convention).
    let mut m = [[0f32; 4]; 4];
    m[0][0] = 1.0 - 2.0 * (y * y + z * z);
    m[0][1] = 2.0 * (x * y - z * w);
    m[0][2] = 2.0 * (x * z + y * w);
    m[1][0] = 2.0 * (x * y + z * w);
    m[1][1] = 1.0 - 2.0 * (x * x + z * z);
    m[1][2] = 2.0 * (y * z - x * w);
    m[2][0] = 2.0 * (x * z - y * w);
    m[2][1] = 2.0 * (y * z + x * w);
    m[2][2] = 1.0 - 2.0 * (x * x + y * y);
    m[3][3] = 1.0;
    if let Some(p) = parent_world {
        // W = T · R · parent  (column-vector: parent applied first).
        let mut out = [[0f32; 4]; 4];
        for r in 0..4 {
            for c in 0..4 {
                out[r][c] = m[r][0] * p[0][c]
                    + m[r][1] * p[1][c]
                    + m[r][2] * p[2][c]
                    + m[r][3] * p[3][c];
            }
        }
        out[0][3] += trans[0];
        out[1][3] += trans[1];
        out[2][3] += trans[2];
        out
    } else {
        m[0][3] = trans[0];
        m[1][3] = trans[1];
        m[2][3] = trans[2];
        m
    }
}

/// Apply a COLUMN-convention 4×4 to a point (the bevy-side mirror of
/// `mat4_apply`).
pub fn column_apply(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
    let mut out = [0f32; 3];
    for r in 0..3 {
        out[r] = m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3];
    }
    out
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn palette_joint(palette: &[u32], slot: u8) -> u16 {
    let j = palette.get(slot as usize).copied().unwrap_or(0);
    j.min(u16::MAX as u32) as u16
}

/// Block 94: both key lookups now go through the corrected
/// `Channel::key_at_frame` — the u16 table is each KEY'S FRAME
/// POSITION (the 0x121102 DOF semantic, PROVEN block 90), so sparse
/// tables (the run track's Spine_2: 18 keys at frames 0..16 + 18)
/// BRACKET-LERP instead of the old clamp-to-last-key read, and the
/// bridge's clips match the CPU pose pipeline EXACTLY on every frame.
fn rot_key(ch: &prototype_p3d::anim::Channel, frame: u32) -> Key {
    ch.key_at_frame(frame as usize).unwrap_or(Key::Rot { x: 0.0, y: 0.0, z: 0.0, w: 1.0 })
}

fn tran_key(ch: &prototype_p3d::anim::Channel, frame: u32) -> Key {
    ch.key_at_frame(frame as usize).unwrap_or(Key::Tran { x: 0.0, y: 0.0, z: 0.0 })
}

/// Extract a rotation quat from a ROW-VECTOR 3×3 (the stored GEM
/// layout).  Because `M_row(q) = M_col(q)ᵀ`, this is the standard
/// column-vector extraction run on the TRANSPOSE — so the returned
/// xyzw feeds `bevy::math::Quat::from_xyzw` unchanged.
pub fn mat3_row_to_quat(m: [[f32; 3]; 3]) -> [f32; 4] {
    // Transpose: t = M_rowᵀ = M_col(q).
    let t = [
        [m[0][0], m[1][0], m[2][0]],
        [m[0][1], m[1][1], m[2][1]],
        [m[0][2], m[1][2], m[2][2]],
    ];
    let (m00, m01, m02) = (t[0][0], t[0][1], t[0][2]);
    let (m10, m11, m12) = (t[1][0], t[1][1], t[1][2]);
    let (m20, m21, m22) = (t[2][0], t[2][1], t[2][2]);
    // Standard branch-by-trace extraction (Shepperd).
    let tr = m00 + m11 + m22;
    let (x, y, z, w);
    if tr > 0.0 {
        let s = (tr + 1.0).sqrt() * 2.0;
        w = 0.25 * s;
        x = (m21 - m12) / s;
        y = (m02 - m20) / s;
        z = (m10 - m01) / s;
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        w = (m21 - m12) / s;
        x = 0.25 * s;
        y = (m01 + m10) / s;
        z = (m02 + m20) / s;
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        w = (m02 - m20) / s;
        x = (m01 + m10) / s;
        y = 0.25 * s;
        z = (m12 + m21) / s;
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        w = (m10 - m01) / s;
        x = (m02 + m20) / s;
        y = (m12 + m21) / s;
        z = 0.25 * s;
    }
    // Normalize (rest matrices carry accumulated float noise).
    let len = (x * x + y * y + z * z + w * w).sqrt();
    if len > 1e-12 {
        [x / len, y / len, z / len, w / len]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

// ---------------------------------------------------------------------------
// Tests (extraction layer — no bevy needed)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use prototype_p3d::skin::{SkeletonJoint, identity4};

    fn two_bone_skeleton() -> Skeleton {
        let joint = |name: &str, parent: u32, t: [f32; 3]| SkeletonJoint {
            name: name.to_string(),
            parent,
            rest: {
                let mut m = identity4();
                m[3] = [t[0], t[1], t[2], 1.0];
                m
            },
        };
        Skeleton {
            name: "test_skeleton".to_string(),
            joints: vec![joint("Root", 0, [0.0, 1.0, 0.0]), joint("Child", 0, [0.0, 0.5, 0.0])],
        }
    }

    /// Skeleton extraction: bottom-row translations, identity rotations
    /// extracted as identity quats (translation-only rest matrices).
    #[test]
    fn extract_skeleton_bottom_row_translations() {
        let skel = two_bone_skeleton();
        let b = extract_skeleton(&skel);
        assert_eq!(b.name, "test_skeleton");
        assert_eq!(b.joints.len(), 2);
        assert_eq!(b.joints[1], BridgeJoint { name: "Child".into(), parent: 0 });
        assert_eq!(b.local_translations[0], [0.0, 1.0, 0.0]);
        assert_eq!(b.local_translations[1], [0.0, 0.5, 0.0]);
        for r in &b.local_rotations {
            assert!((r[3] - 1.0).abs() < 1e-6, "identity quat w=1, got {r:?}");
            assert!(r[0].abs() < 1e-6 && r[1].abs() < 1e-6 && r[2].abs() < 1e-6);
        }
    }

    /// The row-vector 3×3 of a +90°-about-Y quat must extract back to
    /// that quat (round-trip through mat3_row_to_quat).
    #[test]
    fn mat3_row_to_quat_roundtrip_90y() {
        let (s, c) = (0.5f32.sqrt(), 0.5f32.sqrt());
        // Row-vector quat matrix (same convention as pose_world_matrices).
        let (x, y, z, w) = (0.0f32, s, 0.0f32, c);
        let m = [
            [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + z * w), 2.0 * (x * z - y * w)],
            [2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + x * w)],
            [2.0 * (x * z + y * w), 2.0 * (y * z - x * w), 1.0 - 2.0 * (x * x + y * y)],
        ];
        let m4 = [[m[0][0], m[0][1], m[0][2]], [m[1][0], m[1][1], m[1][2]], [m[2][0], m[2][1], m[2][2]]];
        let q = mat3_row_to_quat(m4);
        assert!((q[1] - s).abs() < 1e-5, "y {q:?}");
        assert!((q[3] - c).abs() < 1e-5, "w {q:?}");
        assert!(q[0].abs() < 1e-5 && q[2].abs() < 1e-5);
    }

    /// THE BRIDGE CONVENTION TEST: for the real-style 2-bone chain
    /// driven by an animated quat, the bevy-side column-convention
    /// hierarchy (via `column_world`/`column_apply`) must reproduce the
    /// proven row-vector pipeline (`pose_world_matrices` +
    /// `mat4_apply`) — vertex positions agree to 1e-4.
    #[test]
    fn bevy_quat_matches_row_vector_world() {
        use prototype_p3d::anim::{AnimGroup, AnimKind, AnimTrack, Channel, Key};

        let skel = two_bone_skeleton();
        let (s, c) = (0.5f32.sqrt(), 0.5f32.sqrt());
        let quat90y = Key::Rot { x: 0.0, y: s, z: 0.0, w: c };
        let track = AnimTrack {
            name: "t".into(),
            kind: AnimKind::Ptrn,
            frame_count: 2.0,
            fps: 30.0,
            cyclic: 1,
            groups: vec![AnimGroup {
                bone_name: "Child".into(),
                rot: Some(Channel { count: 2, key_indices: vec![0, 0], keys: vec![quat90y, quat90y] }),
                tran: Some(Channel {
                    count: 2,
                    key_indices: vec![0, 0],
                    keys: vec![Key::Tran { x: 0.2, y: 0.5, z: 0.0 }, Key::Tran { x: 0.2, y: 0.5, z: 0.0 }],
                }),
                ..Default::default()
            }],
        };

        // Row-vector side (the proven pipeline).
        let worlds = prototype_p3d::anim::pose_world_matrices(&skel, &track, 0.0);
        let probe = [[1.0f32, 1.5, 0.0], [0.3, 1.2, -0.4], [-2.0, 0.5, 3.0]];
        for p in probe {
            let row = prototype_p3d::skin::mat4_apply(&worlds[1], p);
            // Column side: compose local transforms bottom-up.
            let (rots, trans) = pose_local_transforms(&skel, &track, 0);
            let w_root = column_world(trans[0], rots[0], None);
            let w_child = column_world(trans[1], rots[1], Some(&w_root));
            let col = column_apply(&w_child, p);
            for k in 0..3 {
                assert!(
                    (row[k] - col[k]).abs() < 1e-4,
                    "probe {p:?} axis {k}: row {row:?} vs column {col:?}"
                );
            }
        }
    }

    /// Prefix-equal weights: an all-zero record = 100% slot 0; a
    /// [j, j', 0, 0] record = 50/50; indices are palette-mapped to
    /// ABSOLUTE joints.
    #[test]
    fn extract_skin_prefix_equal_weights() {
        let mut mesh = SkinMesh {
            name: "m".into(),
            desc: 0xB1B1,
            vertex_count: 3,
            index_count: 0,
            tail: [2, 0, 0, 0, 0],
            palette: vec![7, 14],
            positions: vec![[0.0; 3]; 3],
            normals: vec![[0.0; 3]; 3],
            tangents: Vec::new(),
            uvs: Vec::new(),
            weights: Vec::new(),
            bone_indices: vec![[0, 0, 0, 0], [0, 1, 0, 0], [1, 0, 0, 0]],
            indices: vec![0, 1, 2],
            bound_box: None,
            bound_sphere: None,
            decl_words: [0; 8],
        };
        let b = extract_skin(&mesh);
        assert_eq!(b.joint_indices[0], [7, 7, 7, 7]); // slot 0 (all-zero record → slot 0 = joint 7)
        assert_eq!(b.joint_indices[1], [7, 14, 7, 7]); // slots 0,1 → joints 7,14
        assert_eq!(b.joint_indices[2], [14, 7, 7, 7]);
        assert_eq!(b.joint_weights[0], [1.0, 0.0, 0.0, 0.0]);
        assert!((b.joint_weights[1][0] - 0.5).abs() < 1e-6);
        assert!((b.joint_weights[1][1] - 0.5).abs() < 1e-6);
        assert_eq!(b.indices, vec![0, 1, 2]);
        mesh.positions.push([9.0, 9.0, 9.0]); // silence unused-mut
    }

    /// Animation extraction: per-frame times at frame/fps, the SAME
    /// frame-position key lookup as pose_world_matrices (block 94: the
    /// u16 table = each KEY'S frame position; a cyclic wrap comes from
    /// the DATA — a wrap key carrying frame 0's pose — not from a
    /// table entry pointing backwards), duration = (frames-1)/fps.
    #[test]
    fn extract_animation_per_frame_times_and_clamped_lookup() {
        use prototype_p3d::anim::{AnimGroup, AnimKind, AnimTrack, Channel, Key};

        let track = AnimTrack {
            name: "alex_loco_run_n".into(),
            kind: AnimKind::Ptrn,
            frame_count: 4.0,
            fps: 30.0,
            cyclic: 1,
            groups: vec![AnimGroup {
                bone_name: "Pelvis".into(),
                rot: Some(Channel {
                    count: 4,
                    // Keys at frames 0,1,2,3 — key 3 is the WRAP KEY
                    // (its pose equals frame 0's, the cyclic seam the
                    // real 19-sample run track carries as key 18 ≡ 0).
                    key_indices: vec![0, 1, 2, 3],
                    keys: vec![
                        Key::Rot { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },
                        Key::Rot { x: 0.1, y: 0.0, z: 0.0, w: 1.0 },
                        Key::Rot { x: 0.2, y: 0.0, z: 0.0, w: 1.0 },
                        Key::Rot { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },
                    ],
                }),
                tran: Some(Channel {
                    count: 4,
                    key_indices: vec![0, 1, 2, 3],
                    keys: vec![
                        Key::Tran { x: 0.0, y: 1.0, z: 0.0 },
                        Key::Tran { x: 0.1, y: 1.1, z: 0.0 },
                        Key::Tran { x: 0.2, y: 1.2, z: 0.0 },
                        Key::Tran { x: 0.0, y: 1.0, z: 0.0 },
                    ],
                }),
                ..Default::default()
            }],
        };
        let a = extract_animation(&track, None);
        assert!((a.duration - 3.0 / 30.0).abs() < 1e-6);
        assert!(a.cyclic);
        assert_eq!(a.curves.len(), 1);
        let c = &a.curves[0];
        assert_eq!(c.bone, "Pelvis");
        assert_eq!(c.times, vec![0.0, 1.0 / 30.0, 2.0 / 30.0, 3.0 / 30.0]);
        // Frame 3 samples the wrap key (= frame 0's pose) — same as
        // pose_world_matrices.
        assert_eq!(c.rotations[3], [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(c.rotations[2], [0.2, 0.0, 0.0, 1.0]);
        assert_eq!(c.translations[1], [0.1, 1.1, 0.0]);
    }

    /// Block 91 — synthetic folding: primary 0x121112 beats compact
    /// 0x121114; a compact-only bone drives the rotation; a DOF-only
    /// bone gets constants on the static axes + values on the dyn
    /// axis; TRAN+DOF stacks the DOF override on top of the TRAN.
    #[test]
    fn extract_animation_folds_compact_and_dof() {
        use prototype_p3d::anim::{AnimGroup, AnimKind, Channel, DofChannel};
        let track = AnimTrack {
            name: "t".into(),
            kind: AnimKind::Ptrn,
            frame_count: 3.0,
            fps: 30.0,
            cyclic: 0,
            groups: vec![
                // Primary + compact on one bone: PRIMARY wins (pose
                // pipeline's preference).
                AnimGroup {
                    bone_name: "Pelvis".into(),
                    rot: Some(Channel {
                        count: 3,
                        // Block 94 format-true table: keys at frames
                        // 0,1,2 (frame 2's pose = 0.1 explicitly).
                        key_indices: vec![0, 1, 2],
                        keys: vec![
                            Key::Rot { x: 0.1, y: 0.0, z: 0.0, w: 1.0 },
                            Key::Rot { x: 0.2, y: 0.0, z: 0.0, w: 1.0 },
                            Key::Rot { x: 0.1, y: 0.0, z: 0.0, w: 1.0 },
                        ],
                    }),
                    rot_compact: Some(Channel {
                        count: 3,
                        key_indices: vec![0, 0, 0],
                        keys: vec![Key::Rot { x: 0.9, y: 0.0, z: 0.0, w: 1.0 }],
                    }),
                    ..Default::default()
                },
                // Compact-only bone (like the real Jaw / finger bones).
                AnimGroup {
                    bone_name: "Jaw".into(),
                    rot_compact: Some(Channel {
                        count: 2,
                        key_indices: vec![0, 1, 1],
                        keys: vec![
                            Key::Rot { x: 0.0, y: 0.0, z: 0.1, w: 1.0 },
                            Key::Rot { x: 0.0, y: 0.0, z: 0.2, w: 1.0 },
                        ],
                    }),
                    ..Default::default()
                },
                // DOF-only bone (like the real Motion_Root).
                AnimGroup {
                    bone_name: "Motion_Root".into(),
                    dof: Some(DofChannel {
                        dyn_index: 2,
                        constants: [0.5, 1.0, -0.25],
                        frames: vec![0, 2],
                        values: vec![-0.25, -1.25],
                    }),
                    ..Default::default()
                },
                // TRAN + DOF: TRAN fills all three, DOF overrides x.
                AnimGroup {
                    bone_name: "Character_Root".into(),
                    tran: Some(Channel {
                        count: 3,
                        key_indices: vec![0, 0, 0],
                        keys: vec![Key::Tran { x: 1.0, y: 2.0, z: 9.0 }],
                    }),
                    dof: Some(DofChannel {
                        dyn_index: 0,
                        constants: [0.0, 0.0, 0.0],
                        frames: vec![0],
                        values: vec![4.0],
                    }),
                    ..Default::default()
                },
            ],
        };
        // Block 92: no skeleton passed — the DOF-only bone falls back to
        // its recorded constants on the static axes (documented
        // approximation; the skeleton-rest fold is pinned by the real
        // census test below and the AnimationPlayer tick test).
        let a = extract_animation(&track, None);
        assert_eq!(a.curves.len(), 4);
        assert!(!a.cyclic);
        assert_eq!(a.root_motion, Some((2, -1.0)), "first DOF group's travel");
        // Pelvis: PRIMARY samples, not the compact 0.9 key.
        let pelvis = &a.curves[0];
        assert_eq!(pelvis.rotations, vec![
            [0.1, 0.0, 0.0, 1.0],
            [0.2, 0.0, 0.0, 1.0],
            [0.1, 0.0, 0.0, 1.0],
        ]);
        // Jaw: compact drives it; frame 2 clamps to key 1.
        let jaw = &a.curves[1];
        assert_eq!(jaw.rotations.len(), 3);
        assert_eq!(jaw.rotations[0][2], 0.1);
        assert_eq!(jaw.rotations[2][2], 0.2);
        assert!(jaw.translations.is_empty());
        // Motion_Root (no skeleton): constants on x/y, DOF values (linear)
        // on z.
        let mr = &a.curves[2];
        assert!(mr.rotations.is_empty());
        assert_eq!(mr.translations.len(), 3);
        assert_eq!(mr.translations[0], [0.5, 1.0, -0.25]);
        assert!((mr.translations[1][2] - (-0.75)).abs() < 1e-6, "linear mid {}", mr.translations[1][2]);
        assert_eq!(mr.translations[2][2], -1.25);
        // Character_Root: TRAN [1,2,9] with the DOF x override → [4,2,9].
        let cr = &a.curves[3];
        for t in &cr.translations {
            assert_eq!(*t, [4.0, 2.0, 9.0]);
        }
    }

    /// Block 91/92 — THE FULL-CENSUS TEST on the REAL alex_loco_run_n
    /// fixture: all 60 groups fold (24 primary ROT + 34 compact ROT +
    /// 5 TRAN + 1 DOF; the leg/arm IK groups carry rot+tran), the
    /// root-motion summary reads -3.5945 m per cycle on z, and the
    /// Motion_Root DOF curve endpoints match the decoded channel bytes.
    /// Block 92: the fold now carries the REAL 67-joint skeleton, so the
    /// static axes take the joint's REST translation (compose_pose
    /// semantics) — Motion_Root x = rest 0.0, NOT the recorded constant
    /// 0.06216216 the old fold asserted.
    #[test]
    fn extract_animation_real_run_census_block91() {
        use prototype_p3d::anim::decode_anim_track;
        let buf = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../prototype-bin/tests/fixtures/alex_loco_run_n_chunk.bin"
        ))
        .expect("fixture");
        // The fixture is the RAW 0x121000 chunk (12-B header + body).
        let track = decode_anim_track(&buf, 0, buf.len()).expect("decode");
        assert_eq!(track.name, "alex_loco_run_n");
        assert_eq!(track.groups.len(), 60);
        // Block 92: the fold carries the REAL 67-joint skeleton so the
        // DOF static axes take the joint's REST translation (compose_pose
        // semantics).
        let fix = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../prototype-p3d/tests/fixtures/skeleton_alex_reg_body.p3d"
        ))
        .expect("skeleton fixture");
        let skel = prototype_p3d::skin::decode_skeleton_at(&fix, 12).expect("skeleton");
        assert_eq!(skel.joints.len(), 67);
        let a = extract_animation(&track, Some(&skel));
        assert_eq!(a.curves.len(), 60, "one curve per group");
        let rot_curves = a.curves.iter().filter(|c| !c.rotations.is_empty()).count();
        let tran_curves = a.curves.iter().filter(|c| !c.translations.is_empty()).count();
        assert_eq!(rot_curves, 58, "24 primary + 34 compact ROT");
        assert_eq!(tran_curves, 6, "5 TRAN + 1 DOF (Motion_Root)");
        // Root-motion summary: z axis, -3.5945 m per 19-frame cycle
        // (= 5.7 m/s at 30 fps — the REAL run speed).
        let (axis, travel) = a.root_motion.expect("root motion summary");
        assert_eq!(axis, 2);
        assert!((travel - (-3.5945205)).abs() < 1e-4, "travel {travel}");
        // The DOF curve: Motion_Root z = -0.27545 @ f0 -> -3.86997 @
        // f18; x/y take the joint's REST translation (block 92 —
        // compose_pose semantics), NOT the recorded constants (x
        // constant 0.06216216 vs rest 0.0; y rest 1.0 happens to equal
        // its constant).
        let mr = a.curves.iter().find(|c| c.bone == "Motion_Root").expect("Motion_Root");
        assert!(mr.rotations.is_empty(), "DOF-only bone has no rotation curve");
        assert_eq!(mr.translations.len(), 19);
        assert!((mr.translations[0][2] - (-0.27545443)).abs() < 1e-5);
        assert!((mr.translations[18][2] - (-3.8699749)).abs() < 1e-4);
        assert!((mr.translations[0][0]).abs() < 1e-6, "x = rest 0.0, got {}", mr.translations[0][0]);
        assert!((mr.translations[0][1] - 1.0).abs() < 1e-6, "y = rest 1.0");
        // The recorded constants are NOT folded into the clip (block 92).
        assert!((mr.translations[0][0] - 0.06216216).abs() > 1e-3);
        // Mid-cycle linear interpolation.
        let mid = mr.translations[9][2];
        let expect_mid = -0.27545443 + (-3.8699749 + 0.27545443) * 0.5;
        assert!((mid - expect_mid).abs() < 1e-4, "mid {mid} vs {expect_mid}");
        // A compact-only ANIMATED bone: Shoulder_Con_L (19 keys).
        let scl = a.curves.iter().find(|c| c.bone == "Shoulder_Con_L").expect("Shoulder_Con_L");
        assert_eq!(scl.rotations.len(), 19);
        assert!(scl.translations.is_empty());
        assert_eq!(scl.times.len(), 19);
        // A 1-key compact finger: 19 samples all equal the single key.
        let idx = a.curves.iter().find(|c| c.bone == "Index_R").expect("Index_R");
        assert_eq!(idx.rotations.len(), 19);
        assert_eq!(idx.rotations[0], idx.rotations[18]);
        // Every sampled quat of that finger is a unit quat.
        for r in &idx.rotations {
            let n = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2] + r[3] * r[3]).sqrt();
            assert!((n - 1.0).abs() < 1e-3, "quat norm {n}");
        }
        // The four IK groups (not skeleton joints) still emit curves —
        // a Bevy AnimationPlayer leaves them unmatched (harmless).
        for ik in ["leg_left", "leg_right", "arm_left", "arm_right"] {
            let g = a.curves.iter().find(|c| c.bone == ik).expect(ik);
            assert!(!g.rotations.is_empty() && !g.translations.is_empty());
        }
    }
}

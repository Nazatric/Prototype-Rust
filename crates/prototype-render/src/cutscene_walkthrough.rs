//! cutscene_walkthrough — the REAL decoded NIS camera takes drive the
//! render path (block 78).
//!
//! ## What this is (USER DIRECTIVE item 1 + the block-77 NEXT-UP)
//!
//! Block 77 decoded the intro cutscene's REAL in-engine camera takes
//! (`prototype_p3d::nis::camera_takes` on the committed
//! `art_nis_nis_0_3_a{0,1}_camera_takes.p3d` fixtures — the game's OWN
//! `CAM_NisCam_Take` TRAN/LOOK keyframe streams, NOT synthetic camera
//! paths).  This module closes the loop the directive names: those
//! real keyframed poses are fed, per frame, into the render path the
//! block-76 harness already accepts ([`Camera`] →
//! [`render_mesh_cpu`]), producing a real in-engine cutscene camera
//! walkthrough of a real streamed-world art mesh.
//!
//! ## The NIS rig mount (documented, data-derived)
//!
//! The NIS track members are SCENE-LOCAL (Take 00100's keys live in a
//! few-units box around the origin) while a streamed-world mesh lives
//! in art space (Potentially Manhattan-scale).  A NIS system stages a
//! cutscene into the level with a placement transform; this module
//! applies the same transform CLASS — ONE uniform scale + translation
//! (rotation untouched) derived from the data: the take's key
//! bounding sphere (TRAN and LOOK keys alike) is mapped onto the
//! mesh's bounding sphere at `framing`× its radius.  The REAL path
//! shape is preserved exactly under that mount — distances scale
//! uniformly (pinned by test), every key passes through verbatim.
//!
//! ## Documented approximations (honest limits)
//!
//! * INTERPOLATION: poses are sampled piecewise-linearly in KEY space
//!   (a shared normalized parameter walks the TRAN and LOOK sections'
//!   own key lists).  The real tracks interpolate in TIME via each
//!   section's u16 index/time table, which the block-77 scan-based
//!   decoder does not expose yet — key-space sampling passes through
//!   every real key and preserves the path; exact time-sync between
//!   TRAN (131 keys) and LOOK (207 keys) is a documented UNKNOWN until
//!   the u16 tables decode.
//! * FOV: the take's REAL `FOV` section holds ONE key = 0.4798 rad
//!   (block-77 evidence) — a constant-FOV shot; [`NIS_TAKE_FOV_Y`]
//!   carries that real value as the walkthrough default (LIKELY the
//!   vertical FOV in radians; pinned by test).
//! * RASTER: the CPU mirror (`render_art_world_cpu`) — the same
//!   documented approximation class as block 76 (no GPU in the
//!   sandbox; the GPU PNG remains USER-SIDE).

#![forbid(unsafe_code)]

use crate::render_art_world_cpu::{render_mesh_cpu, CpuMirrorFrame};
use crate::scene::{Camera, SceneMesh};
use prototype_math::Vec3;
use prototype_p3d::nis::CameraTake;

/// The REAL `FOV` key of `CAM_NisCam_Take 00100` (`NIS_0_3_a`'s first
/// camera take): one key at 0.4798 rad ≈ 27.5° — a constant-FOV shot
/// (block-77 scan evidence).  LIKELY the vertical FOV in radians; the
/// walkthrough default so the renders use the take's own lens.
pub const NIS_TAKE_FOV_Y: f32 = 0.4798;

/// The NIS scene rig mount: ONE uniform scale + translation (rotation
/// untouched) mapping the take's scene-local key space onto the art
/// mesh's bounding sphere.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigMount {
    /// Uniform scale (scene units → art units).
    pub scale: f32,
    /// Translation applied AFTER scaling (art-space offset).
    pub translation: [f32; 3],
}

impl RigMount {
    /// Mount a scene-local point into art space.
    pub fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        [
            p[0] * self.scale + self.translation[0],
            p[1] * self.scale + self.translation[1],
            p[2] * self.scale + self.translation[2],
        ]
    }
}

/// One walkthrough frame: a REAL keyframed pose driving the camera,
/// plus the rendered CPU-mirror frame.
#[derive(Debug, Clone, PartialEq)]
pub struct WalkthroughFrame {
    /// Sample index (`0..frames`).
    pub index: usize,
    /// The REAL take name (e.g. `"CAM_NisCam_Take 00100"`).
    pub take_name: String,
    /// The mounted eye point (the take's TRAN key at this sample).
    pub eye: [f32; 3],
    /// The mounted look target (the take's LOOK key at this sample).
    pub target: [f32; 3],
    /// The camera this pose produced (the render path's input).
    pub camera: Camera,
    /// The rendered frame (CPU mirror — the documented sandbox path).
    pub cpu: CpuMirrorFrame,
}

/// The take's key bounding-box centre over TRAN + LOOK keys — the
/// EXACT point [`rig_mount`] maps onto the mesh's bounding-sphere
/// centre (shared so the no-LOOK fallback aims the camera straight at
/// the mounted stage).
fn take_key_center(tran: &[[f32; 3]], look: &[[f32; 3]]) -> [f32; 3] {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in tran.iter().chain(look.iter()) {
        for a in 0..3 {
            min[a] = min[a].min(p[a]);
            max[a] = max[a].max(p[a]);
        }
    }
    [
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ]
}

/// Derive the NIS rig mount: map the take's key bounding sphere
/// (TRAN + LOOK keys) onto the mesh's bounding sphere at
/// `framing` × its radius.  Rotation untouched — the REAL path shape
/// survives exactly under scale + translation.
///
/// Fails only when the mesh has no bounds (nothing to frame onto) or
/// the take's keys are degenerate (zero extent → scale undefined; a
/// single-key take cannot walk).
pub fn rig_mount(take: &CameraTake, mesh: &SceneMesh, framing: f32) -> Result<RigMount, String> {
    if framing <= 0.0 || !framing.is_finite() {
        return Err(format!("framing must be finite > 0, got {framing}"));
    }
    if take.tran_keys.len() < 2 {
        return Err(format!(
            "take {:?} has {} TRAN keys — need >= 2 to walk",
            take.name,
            take.tran_keys.len()
        ));
    }
    let center = mesh.bounds_center().ok_or_else(|| {
        format!(
            "mesh {:?} has no bounds — nothing to mount the rig onto",
            mesh.name
        )
    })?;
    let radius = mesh
        .bounds_radius()
        .ok_or_else(|| format!("mesh {:?} has no bounds radius", mesh.name))?;

    // The take's key bounding sphere over BOTH sections (the mount
    // must keep eyes AND targets coherent).
    let take_center = take_key_center(&take.tran_keys, &take.look_keys);
    let take_radius = take
        .tran_keys
        .iter()
        .chain(take.look_keys.iter())
        .map(|p| {
            let d = [p[0] - take_center[0], p[1] - take_center[1], p[2] - take_center[2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        })
        .fold(0.0f32, f32::max);
    if !take_radius.is_finite() || take_radius <= 1e-6 {
        return Err(format!(
            "take {:?} keys are degenerate (radius {take_radius}) — mount undefined",
            take.name
        ));
    }
    let scale = radius * framing / take_radius;
    Ok(RigMount {
        scale,
        translation: [
            center[0] - take_center[0] * scale,
            center[1] - take_center[1] * scale,
            center[2] - take_center[2] * scale,
        ],
    })
}

/// Sample the take's REAL keys at normalized parameter `t ∈ [0, 1]`:
/// piecewise-linear between neighbouring TRAN keys and (its own key
/// list) neighbouring LOOK keys.  `t = 0` is the take's first key,
/// `t = 1` its last — every real key passes through verbatim.
///
/// Target fallback chain (all steps data-derived, documented):
/// 1. the take's REAL LOOK keys (the normal path);
/// 2. with no LOOK keys — the take's OWN key bounding-box centre
///    ("look at the stage": the exact point the rig mount maps onto
///    the mesh centre, so the mounted mesh sits dead-centre);
/// 3. a DEGENERATE pose (some real takes end with the camera arriving
///    AT its look point — Take 00110's last TRAN == last LOOK key) —
///    fall back to the stage centre, then to the direction of travel.
pub fn sample_pose(take: &CameraTake, t: f32) -> ([f32; 3], [f32; 3]) {
    let tran = &take.tran_keys;
    let n = tran.len().max(2);
    let tt = (t.clamp(0.0, 1.0)) * (n - 1) as f32;
    let i = (tt as usize).min(n - 2);
    let f = tt - i as f32;
    let lerp3 = |a: &[f32; 3], b: &[f32; 3], k: f32| {
        [
            a[0] + (b[0] - a[0]) * k,
            a[1] + (b[1] - a[1]) * k,
            a[2] + (b[2] - a[2]) * k,
        ]
    };
    let eye = lerp3(&tran[i], &tran[i + 1], f);

    let look = &take.look_keys;
    let mut target = if look.len() >= 2 {
        let m = look.len();
        let tl = t.clamp(0.0, 1.0) * (m - 1) as f32;
        let j = (tl as usize).min(m - 2);
        let g = tl - j as f32;
        lerp3(&look[j], &look[j + 1], g)
    } else if look.len() == 1 {
        look[0]
    } else {
        take_key_center(tran, look)
    };
    let near = |a: [f32; 3], b: [f32; 3]| {
        let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= 1e-12
    };
    if near(eye, target) {
        let c = take_key_center(tran, look);
        if !near(eye, c) {
            target = c;
        } else {
            target = [
                eye[0] + tran[i + 1][0] - tran[i][0],
                eye[1] + tran[i + 1][1] - tran[i][1],
                eye[2] + tran[i + 1][2] - tran[i][2],
            ];
        }
    }
    (eye, target)
}

/// Build the walkthrough [`Camera`] for one mounted pose.
fn pose_camera(
    take_name: &str,
    index: usize,
    eye: [f32; 3],
    target: [f32; 3],
    mesh: &SceneMesh,
    framing: f32,
    fov_y: f32,
    width: u32,
    height: u32,
) -> Result<Camera, String> {
    let radius = mesh.bounds_radius().unwrap_or(1.0);
    let eye_v = Vec3::new(eye[0], eye[1], eye[2]);
    let target_v = Vec3::new(target[0], target[1], target[2]);
    let dir = target_v - eye_v;
    if dir.length_sq() <= 1e-12 {
        return Err(format!(
            "take {take_name} frame {index}: degenerate pose (eye == target)"
        ));
    }
    // Y-up guard: when the view direction is near-vertical, fall back
    // to +Z so look_at keeps a valid basis (the real takes dolly near
    // horizontally, so the primary up stands).
    let up = if (dir.normalized().dot(Vec3::UP)).abs() > 0.99 {
        Vec3::new(0.0, 0.0, 1.0)
    } else {
        Vec3::UP
    };
    Ok(Camera {
        eye: eye_v,
        target: target_v,
        up,
        fov_y,
        aspect: width as f32 / height as f32,
        // Mesh-scale near/far: the mounted pose sits within
        // `framing × R` of the mesh centre, so these bounds cover the
        // whole mounted rig with headroom on both ends.
        near: radius * 0.02,
        far: (2.0 * framing + 4.0) * radius,
    })
}

/// Walk a REAL take's keyframes through the render path: `frames`
/// evenly spaced poses (real keys interpolated as documented), each
/// mounted and rendered through the CPU mirror on `mesh`.
pub fn walkthrough_frames(
    mesh: &SceneMesh,
    texture: Option<(&[u8], u32, u32)>,
    take: &CameraTake,
    frames: usize,
    framing: f32,
    fov_y: f32,
    width: u32,
    height: u32,
    clear: [u8; 4],
) -> Result<Vec<WalkthroughFrame>, String> {
    if frames == 0 {
        return Err("frames must be >= 1".into());
    }
    if width == 0 || height == 0 {
        return Err("width/height must be non-zero".into());
    }
    let mount = rig_mount(take, mesh, framing)?;
    let mut out = Vec::with_capacity(frames);
    for k in 0..frames {
        let t = if frames == 1 {
            0.0
        } else {
            k as f32 / (frames - 1) as f32
        };
        let (eye, target) = sample_pose(take, t);
        let eye = mount.apply(eye);
        let target = mount.apply(target);
        let camera = pose_camera(
            &take.name,
            k,
            eye,
            target,
            mesh,
            framing,
            fov_y,
            width,
            height,
        )?;
        let cpu = render_mesh_cpu(mesh, &camera, texture, width, height, clear)?;
        out.push(WalkthroughFrame {
            index: k,
            take_name: take.name.clone(),
            eye,
            target,
            camera,
            cpu,
        });
    }
    Ok(out)
}

/// Compose walkthrough frames into a contact sheet (row-major, `cols`
/// columns, `gap`-px `bg` separators).  All frames must share dims
/// (they do — one walkthrough call produces one size).
pub fn contact_sheet(
    frames: &[WalkthroughFrame],
    cols: usize,
    gap: u32,
    bg: [u8; 4],
) -> Result<(u32, u32, Vec<u8>), String> {
    if frames.is_empty() {
        return Err("contact_sheet: no frames".into());
    }
    if cols == 0 {
        return Err("contact_sheet: cols must be >= 1".into());
    }
    let (w, h) = (frames[0].cpu.width, frames[0].cpu.height);
    if frames.iter().any(|f| f.cpu.width != w || f.cpu.height != h) {
        return Err("contact_sheet: frames must share dimensions".into());
    }
    let gap = gap as usize;
    let (wu, hu) = (w as usize, h as usize);
    let rows = (frames.len() + cols - 1) / cols;
    let out_w = cols * wu + (cols + 1) * gap;
    let out_h = rows * hu + (rows + 1) * gap;
    let mut sheet = vec![0u8; out_w * out_h * 4];
    for px in sheet.chunks_exact_mut(4) {
        px.copy_from_slice(&bg);
    }
    for (idx, frame) in frames.iter().enumerate() {
        let col = idx % cols;
        let row = idx / cols;
        let x0 = gap + col * (wu + gap);
        let y0 = gap + row * (hu + gap);
        for y in 0..hu {
            let src = &frame.cpu.rgba[y * wu * 4..(y + 1) * wu * 4];
            let dst_off = ((y0 + y) * out_w + x0) * 4;
            sheet[dst_off..dst_off + wu * 4].copy_from_slice(src);
        }
    }
    Ok((out_w as u32, out_h as u32, sheet))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::SceneMesh;

    fn fixture(name: &str) -> Vec<u8> {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join(name)
            .pipe_read()
    }
    trait PipeRead {
        fn pipe_read(&self) -> Vec<u8>;
    }
    impl PipeRead for std::path::PathBuf {
        fn pipe_read(&self) -> Vec<u8> {
            std::fs::read(self).unwrap_or_else(|e| panic!("missing fixture {self:?}: {e}"))
        }
    }

    fn barrier_mesh() -> SceneMesh {
        let buf = fixture("art_barrier_os002.p3d");
        let top = prototype_p3d::geometry::top_level_chunks(&buf).unwrap();
        let shape = top
            .iter()
            .find(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_SHAPE)
            .map(|c| prototype_p3d::geometry::decode_shape(&buf, c.offset, c.size).unwrap())
            .unwrap();
        SceneMesh::from_p3d_mesh(shape.meshes.first().expect("barrier has a mesh"))
    }

    const CLEAR: [u8; 4] = [10, 12, 16, 255];

    /// THE BLOCK-78 HEADLINE: the REAL intro-cutscene camera take
    /// (Take 00100, 131 TRAN keys — the game's OWN dolly path) drives
    /// the render path across the REAL streamed-world barrier mesh.
    /// Every frame is non-blank, the camera REALLY moves (eyes differ
    /// start→end and consecutive frames differ pixel-wise), and the
    /// first frame's eye is the take's FIRST REAL KEY, mounted.
    #[test]
    fn take_00100_drives_the_camera_through_the_real_mesh() {
        let mesh = barrier_mesh();
        let a0 = fixture("art_nis_nis_0_3_a0_camera_takes.p3d");
        let takes = prototype_p3d::nis::camera_takes(&a0);
        assert_eq!(takes.len(), 2, "a0 holds takes 00100 + 00101");
        let take = &takes[0];
        assert_eq!(take.name, "CAM_NisCam_Take 00100");
        assert_eq!(take.tran_keys.len(), 131);

        let frames = walkthrough_frames(
            &mesh,
            None,
            take,
            6,
            1.5,
            NIS_TAKE_FOV_Y,
            96,
            54,
            CLEAR,
        )
        .expect("the real take walks the real mesh");
        assert_eq!(frames.len(), 6);
        assert_eq!(frames[0].take_name, "CAM_NisCam_Take 00100");

        // The mount maps the take's FIRST REAL KEY to frame 0's eye.
        let mount = rig_mount(take, &mesh, 1.5).unwrap();
        let expected_eye = mount.apply(take.tran_keys[0]);
        let got = frames[0].eye;
        for a in 0..3 {
            assert!((got[a] - expected_eye[a]).abs() < 1e-3, "axis {a}: {got:?}");
        }

        // Non-blank everywhere + the camera really moves.
        for f in &frames {
            assert!(f.cpu.stats.distinct > 64, "frame {} blank-ish: {:?}", f.index, f.cpu.stats);
            assert!(f.cpu.stats.triangles_drawn > 0, "frame {} drew nothing", f.index);
        }
        let d = |a: [f32; 3], b: [f32; 3]| {
            let v = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
            (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
        };
        let travel = d(frames[0].eye, frames[5].eye);
        assert!(travel > 1e-3, "the take's mounted path must move: {travel}");
        for w in frames.windows(2) {
            assert_ne!(w[0].cpu.rgba, w[1].cpu.rgba, "consecutive frames must differ");
        }
    }

    /// The rig mount is ONE uniform scale + translation: distances
    /// between mounted keys equal the original distances × scale (no
    /// rotation, no re-shaping — the REAL path geometry survives), and
    /// the take's centre lands on the mesh's centre.
    #[test]
    fn rig_mount_preserves_the_real_path_shape() {
        let mesh = barrier_mesh();
        let a0 = fixture("art_nis_nis_0_3_a0_camera_takes.p3d");
        let take = &prototype_p3d::nis::camera_takes(&a0)[0];
        let mount = rig_mount(take, &mesh, 1.5).expect("mount derives");
        assert!(mount.scale.is_finite() && mount.scale > 0.0);

        let dist = |a: [f32; 3], b: [f32; 3]| {
            let v = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
            (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
        };
        let first = take.tran_keys[0];
        let last = take.tran_keys[take.tran_keys.len() - 1];
        let before = dist(first, last);
        let after = dist(mount.apply(first), mount.apply(last));
        assert!(
            (after - before * mount.scale).abs() < 1e-3 * after.max(1.0),
            "distance must scale uniformly: {after} vs {} × {}",
            before,
            mount.scale
        );

        // A mid-path key also preserves pairwise distance to the first
        // key (uniform, not per-axis).
        let mid = take.tran_keys[65];
        let m_before = dist(first, mid);
        let m_after = dist(mount.apply(first), mount.apply(mid));
        assert!((m_after - m_before * mount.scale).abs() < 1e-3 * m_after.max(1.0));
    }

    /// Walkthrough contract edges: a single-key take is rejected, a
    /// take without LOOK keys still walks (the documented stage-look
    /// fallback), and zero frames is rejected.
    #[test]
    fn walkthrough_contract_edges() {
        let mesh = barrier_mesh();
        let a0 = fixture("art_nis_nis_0_3_a0_camera_takes.p3d");
        let take = &prototype_p3d::nis::camera_takes(&a0)[0];

        assert!(walkthrough_frames(&mesh, None, take, 0, 1.5, NIS_TAKE_FOV_Y, 96, 54, CLEAR).is_err());

        let mut single = take.clone();
        single.tran_keys.truncate(1);
        assert!(walkthrough_frames(&mesh, None, &single, 4, 1.5, NIS_TAKE_FOV_Y, 96, 54, CLEAR).is_err());

        let mut no_look = take.clone();
        no_look.look_keys.clear();
        let frames = walkthrough_frames(&mesh, None, &no_look, 4, 1.5, NIS_TAKE_FOV_Y, 96, 54, CLEAR)
            .expect("stage-look fallback walks");
        assert_eq!(frames.len(), 4);
        for f in &frames {
            // A mounted cutscene path legitimately flies THROUGH the
            // set's bounding sphere — a close-up frame can be one
            // uniformly-lit face (few distinct values).  The honest
            // non-blank bar here: not the clear colour everywhere AND
            // geometry actually reached the rasteriser.
            assert!(
                f.cpu.stats.distinct >= 2 && f.cpu.stats.triangles_drawn > 0,
                "look-at frame {} blank: {:?}",
                f.index,
                f.cpu.stats
            );
        }
    }

    /// The contact sheet composes the walkthrough into one evidence
    /// PNG payload: exact dims (cols × W + gaps), each cell holds its
    /// frame's pixels verbatim, and the sheet is non-blank.
    #[test]
    fn contact_sheet_composes_the_walkthrough() {
        let mesh = barrier_mesh();
        let a0 = fixture("art_nis_nis_0_3_a0_camera_takes.p3d");
        let take = &prototype_p3d::nis::camera_takes(&a0)[0];
        let frames = walkthrough_frames(&mesh, None, take, 8, 1.5, NIS_TAKE_FOV_Y, 80, 45, CLEAR)
            .expect("walk");
        let (w, h, rgba) = contact_sheet(&frames, 2, 4, [0, 0, 0, 255]).expect("sheet");
        // 2 cols × 80 + 3 gaps × 4 = 172; 4 rows × 45 + 5 gaps × 4 = 200.
        assert_eq!((w, h), (172, 200));
        // Cell (0,0) starts at (4, 4) and holds frame 0's first row.
        let f0 = &frames[0].cpu.rgba[0..80 * 4];
        assert_eq!(&rgba[(4 * 172 + 4) * 4..(4 * 172 + 4) * 4 + 80 * 4], f0);
        // The separator grid is the bg colour.
        let sep = ((2 * 45 + 8) * 172 + 2) * 4; // a gap row inside
        assert_eq!(&rgba[sep..sep + 4], &[0, 0, 0, 255]);
        let distinct: std::collections::HashSet<u32> = rgba
            .chunks_exact(4)
            .map(|p| u32::from_le_bytes([p[0], p[1], p[2], p[3]]))
            .collect();
        assert!(distinct.len() > 64, "sheet distinct = {}", distinct.len());
    }
}

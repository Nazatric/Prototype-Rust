//! render_art_world_cpu — the DEVICE-FREE CPU mirror of the
//! RENDER_ART_WORLD_PATCH harness (`super::render_art_world`).
//!
//! ## Why this exists (block 76)
//!
//! The real harness renders the streamed world through the REAL
//! translated DLL `default` shader pair on a GPU.  The build sandbox
//! has NO GPU (no wgpu adapter — the harness's own test skips), so
//! USER-SIDE runs produce the GPU PNGs first.  This module is the
//! honest CPU stand-in used for IN-SANDBOX visual evidence: it walks
//! the EXACT same inputs — a real [`SceneMesh`] (block-16 M21b
//! decode), the EXACT same camera math (`Camera::view_projection`,
//! PROVEN), and the EXACT same fragment-side daylight DESIGN rows
//! (`shader::world::daylight_state` semantics) — and rasterizes the
//! mesh on the CPU into RGBA8, which the proven zero-dependency PNG
//! encoder (`prototype_video::png::png_encode_rgba`, block 75) turns
//! into the shipped evidence file.
//!
//! ## What is mirrored 1:1, and what is a documented approximation
//!
//! * PROVEN / mirrored exactly: the WVP transform (same
//!   `prototype_math` matrices, `z ∈ [0, 1]` depth), the vertex data
//!   (bit-exact decoded positions / normals / first UV set), the
//!   nearest-wrapped texture sample, and the daylight fragment model —
//!   `ambient = (0.35, 0.36, 0.40)`, `sun colour = (1.0, 0.96, 0.88)`,
//!   the directional term `N·L` against
//!   [`super::shader::world::SUN_DIRECTION`], `diffuse × (ambient +
//!   colour × max(N·L, 0))` — the same DESIGN rows the GPU harness
//!   uploads (block-35 evidence line-by-line in `world.rs`).
//! * APPROXIMATION (documented): single-threaded scanline rasteriser
//!   with a z-buffer, perspective-correct interpolants, NO backface
//!   culling (the GPU pipeline also runs `cull_mode: None`), base
//!   mip level only, and a w > 0 near-guard per triangle instead of a
//!   real clipper.  The point is evidence, not parity: the GPU PNG
//!   remains the authoritative first-run output (USER-SIDE).

#![forbid(unsafe_code)]

use crate::scene::{Camera, SceneMesh};
use crate::shader::world::SUN_DIRECTION;
use prototype_math::Vec3;

/// One rendered CPU-mirror frame plus its non-blank statistics.
#[derive(Debug, Clone, PartialEq)]
pub struct CpuMirrorFrame {
    /// Texel width.
    pub width: u32,
    /// Texel height.
    pub height: u32,
    /// Packed RGBA8, row-major, opaque (alpha 255), `width*height*4` B.
    pub rgba: Vec<u8>,
    /// Statistics proving the frame is not blank.
    pub stats: FrameStats,
}

/// Non-blank evidence statistics (the same shape the `bik-frames`
/// evidence path prints: distinct-value count + per-channel extrema).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameStats {
    /// Distinct packed-RGBA u32 values in the frame.
    pub distinct: usize,
    /// Per-channel minima [r, g, b, a].
    pub min: [u8; 4],
    /// Per-channel maxima [r, g, b, a].
    pub max: [u8; 4],
    /// Triangles that passed the w-guard and reached the rasteriser.
    pub triangles_drawn: usize,
    /// Triangles rejected by the w-guard (any vertex w <= EPS_W).
    pub triangles_clipped: usize,
}

/// The daylight DESIGN rows, mirrored from
/// `shader::world::daylight_state` (block 35) — one source of truth
/// per value is not possible across crates without a const, so the
/// values are pinned by test (`cpu_mirror_matches_daylight_rows`).
mod daylight {
    /// `p3dLight_Ambient` (c2) — the ambient floor multiplier.
    pub const AMBIENT: [f32; 3] = [0.35, 0.36, 0.40];
    /// `p3dLight_Colour` (c3) — the warm sun multiplier.
    pub const COLOUR: [f32; 3] = [1.0, 0.96, 0.88];
}

/// Frame the camera on a mesh from its bounding box: eye on a diagonal
/// toward the bbox centre at 2.6× the bounding radius (a framing that
/// works under either world-up convention — documented UNKNOWN).
/// Meshes without bounds fall back to the default camera.
pub fn frame_camera_for(mesh: &SceneMesh) -> Camera {
    let mut cam = Camera::default();
    let Some(center) = mesh.bounds_center() else {
        return cam;
    };
    let Some(radius) = mesh.bounds_radius() else {
        return cam;
    };
    let r = radius.max(1e-4);
    let dir = Vec3::new(0.55, 0.40, 0.95).normalized();
    cam.eye = Vec3::new(
        center[0] + dir.x * r * 2.6,
        center[1] + dir.y * r * 2.6,
        center[2] + dir.z * r * 2.6,
    );
    cam.target = Vec3::new(center[0], center[1], center[2]);
    cam.fov_y = std::f32::consts::FRAC_PI_4;
    cam.near = r * 0.05;
    cam.far = r * 12.0 + 1.0;
    cam
}

/// Render one real mesh into a CPU-mirror frame.
///
/// * `mesh` — the REAL decoded art mesh (positions / normals / first
///   UV set / indices; meshes without normals get face normals).
/// * `camera` — the IN-GAME camera model (`Camera::view_projection`,
///   the proven `z ∈ [0, 1]` perspective).
/// * `texture` — the diffuse RGBA8 (`(bytes, width, height)`), already
///   decoded from the mesh's real DDS (`dxt::decode_dds_mips`); `None`
///   samples the shared white fallback (the GPU path binds white too).
/// * `clear` — the background RGBA (the harness clears to the same
///   colour so the two paths' frames are comparable).
pub fn render_mesh_cpu(
    mesh: &SceneMesh,
    camera: &Camera,
    texture: Option<(&[u8], u32, u32)>,
    width: u32,
    height: u32,
    clear: [u8; 4],
) -> Result<CpuMirrorFrame, String> {
    render_meshes_cpu(
        &[mesh],
        &[texture],
        camera,
        width,
        height,
        clear,
    )
}

/// Render SEVERAL real meshes into ONE CPU-mirror frame sharing a
/// single depth buffer (block 88: the skinned Alex render composites
/// body + head with correct z-buffered overlap — NOT a painter's
/// merge).
///
/// `meshes[i]` is paired with `textures[i]` (the diffuse RGBA8 already
/// decoded from the mesh's real DDS, or `None` for the shared white
/// fallback).  All other semantics match [`render_mesh_cpu`].
pub fn render_meshes_cpu(
    meshes: &[&SceneMesh],
    textures: &[Option<(&[u8], u32, u32)>],
    camera: &Camera,
    width: u32,
    height: u32,
    clear: [u8; 4],
) -> Result<CpuMirrorFrame, String> {
    if width == 0 || height == 0 {
        return Err("width/height must be non-zero".into());
    }
    if meshes.len() != textures.len() {
        return Err(format!(
            "{} meshes but {} textures — pair them one-to-one",
            meshes.len(),
            textures.len()
        ));
    }
    for mesh in meshes {
        let n_verts = mesh.positions.len();
        if mesh.indices.is_empty() || n_verts == 0 {
            return Err(format!("mesh {:?} has no drawable geometry", mesh.name));
        }
        if !mesh.uvs.is_empty() && mesh.uvs.len() != n_verts {
            return Err(format!(
                "mesh {:?}: {} uvs != {} positions",
                mesh.name,
                mesh.uvs.len(),
                n_verts
            ));
        }
    }

    let w = width as i64;
    let h = height as i64;
    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    for px in rgba.chunks_exact_mut(4) {
        px[0] = clear[0];
        px[1] = clear[1];
        px[2] = clear[2];
        px[3] = clear[3];
    }
    let mut depth = vec![f32::INFINITY; (width as usize) * (height as usize)];

    // The WVP — the SAME matrices the GPU vertex stage consumes via
    // the c0 row block (`Camera::view_projection_rows` packing).
    let wvp = camera.view_projection();

    let mut tri_drawn = 0usize;
    let mut tri_clipped = 0usize;

    for (mesh, texture) in meshes.iter().zip(textures.iter()) {
        let texture = *texture;
        let n_verts = mesh.positions.len();
        for tri in mesh.indices.chunks_exact(3) {
            let i0 = tri[0] as usize;
            let i1 = tri[1] as usize;
            let i2 = tri[2] as usize;
            if i0 >= n_verts || i1 >= n_verts || i2 >= n_verts {
                continue; // the decoder already enforces this; belt and braces
            }
            let c = [
                wvp.transform_point4(Vec3::new(
                    mesh.positions[i0][0],
                    mesh.positions[i0][1],
                    mesh.positions[i0][2],
                )),
                wvp.transform_point4(Vec3::new(
                    mesh.positions[i1][0],
                    mesh.positions[i1][1],
                    mesh.positions[i1][2],
                )),
                wvp.transform_point4(Vec3::new(
                    mesh.positions[i2][0],
                    mesh.positions[i2][1],
                    mesh.positions[i2][2],
                )),
            ];
            if c[0].w <= 1e-6 || c[1].w <= 1e-6 || c[2].w <= 1e-6 {
                tri_clipped += 1;
                continue;
            }
            tri_drawn += 1;

            // NDC → screen (row 0 at the TOP of the image; +y NDC up).
            let sx = [c[0].x / c[0].w, c[1].x / c[1].w, c[2].x / c[2].w];
            let sy = [c[0].y / c[0].w, c[1].y / c[1].w, c[2].y / c[2].w];
            let sz = [c[0].z / c[0].w, c[1].z / c[1].w, c[2].z / c[2].w];
            let px = [sx[0] * 0.5 + 0.5, sx[1] * 0.5 + 0.5, sx[2] * 0.5 + 0.5];
            // Block 88 ORIENTATION FIX: the shared `perspective_rh_zo`
            // NEGATES clip-space Y (Vulkan framebuffer convention), so
            // world-up lands at NEGATIVE NDC y — the old `py = 0.5 -
            // sy*0.5` mapping rendered every CPU frame vertically
            // flipped (upside-down Alex, caught by eyeball on the real
            // skinned model).  Row 0 of the PNG = image TOP:
            let py = [0.5 + sy[0] * 0.5, 0.5 + sy[1] * 0.5, 0.5 + sy[2] * 0.5];
            let scr = [
                (px[0] * w as f32, py[0] * h as f32),
                (px[1] * w as f32, py[1] * h as f32),
                (px[2] * w as f32, py[2] * h as f32),
            ];
            let inv_w = [1.0 / c[0].w, 1.0 / c[1].w, 1.0 / c[2].w];

            // Per-vertex attributes (perspective-correct: premultiplied by
            // 1/w here, un-divided per pixel below).
            let has_normals = mesh.normals.len() == n_verts;
            let has_uvs = mesh.uvs.len() == n_verts;
            let face_normal = || {
                let a = Vec3::new(
                    mesh.positions[i1][0] - mesh.positions[i0][0],
                    mesh.positions[i1][1] - mesh.positions[i0][1],
                    mesh.positions[i1][2] - mesh.positions[i0][2],
                );
                let b = Vec3::new(
                    mesh.positions[i2][0] - mesh.positions[i0][0],
                    mesh.positions[i2][1] - mesh.positions[i0][1],
                    mesh.positions[i2][2] - mesh.positions[i0][2],
                );
                let n = a.cross(b);
                let l = n.length();
                if l > 1e-9 {
                    [n.x / l, n.y / l, n.z / l]
                } else {
                    [0.0, 0.0, 1.0]
                }
            };
            let attr_n = |i: usize| -> [f32; 3] {
                if has_normals {
                    mesh.normals[i]
                } else {
                    face_normal()
                }
            };
            let attr_uv = |i: usize| -> [f32; 2] {
                if has_uvs { mesh.uvs[i] } else { [0.5, 0.5] }
            };
            let n0 = attr_n(i0);
            let n1 = attr_n(i1);
            let n2 = attr_n(i2);
            let uv0 = attr_uv(i0);
            let uv1 = attr_uv(i1);
            let uv2 = attr_uv(i2);

            // Bounding box (clamped) + the edge-function fill.
            let min_x = scr[0].0.min(scr[1].0).min(scr[2].0).floor().max(0.0) as i64;
            let max_x = (scr[0].0.max(scr[1].0).max(scr[2].0).ceil() as i64).min(w - 1);
            let min_y = scr[0].1.min(scr[1].1).min(scr[2].1).floor().max(0.0) as i64;
            let max_y = (scr[0].1.max(scr[1].1).max(scr[2].1).ceil() as i64).min(h - 1);
            if min_x > max_x || min_y > max_y {
                continue;
            }
            let area = edge(scr[0], scr[1], scr[2]);
            if area.abs() < 1e-9 {
                continue; // degenerate
            }

            for y in min_y..=max_y {
                for x in min_x..=max_x {
                    let p = ((x as f32) + 0.5, (y as f32) + 0.5);
                    // Barycentrics via edge functions (no topology assumed).
                    let b0 = edge(scr[1], scr[2], p) / area;
                    let b1 = edge(scr[2], scr[0], p) / area;
                    let b2 = edge(scr[0], scr[1], p) / area;
                    if b0 < 0.0 || b1 < 0.0 || b2 < 0.0 {
                        continue;
                    }
                    // Perspective-correct depth (z/w linear in screen space
                    // is z_ndc itself — interpolate the NDC z directly).
                    let z = b0 * sz[0] + b1 * sz[1] + b2 * sz[2];
                    if !(0.0..=1.0).contains(&z) {
                        continue;
                    }
                    let di = (y as usize) * (width as usize) + x as usize;
                    if z >= depth[di] {
                        continue;
                    }
                    depth[di] = z;

                    // Perspective-correct interpolants.
                    let wp = b0 * inv_w[0] + b1 * inv_w[1] + b2 * inv_w[2];
                    if wp <= 1e-12 {
                        continue;
                    }
                    let u = (b0 * uv0[0] * inv_w[0]
                        + b1 * uv1[0] * inv_w[1]
                        + b2 * uv2[0] * inv_w[2])
                        / wp;
                    let v = (b0 * uv0[1] * inv_w[0]
                        + b1 * uv1[1] * inv_w[1]
                        + b2 * uv2[1] * inv_w[2])
                        / wp;
                    let mut n = [
                        (b0 * n0[0] * inv_w[0]
                            + b1 * n1[0] * inv_w[1]
                            + b2 * n2[0] * inv_w[2])
                            / wp,
                        (b0 * n0[1] * inv_w[0]
                            + b1 * n1[1] * inv_w[1]
                            + b2 * n2[1] * inv_w[2])
                            / wp,
                        (b0 * n0[2] * inv_w[0]
                            + b1 * n1[2] * inv_w[1]
                            + b2 * n2[2] * inv_w[2])
                            / wp,
                    ];
                    let nl = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                    if nl > 1e-9 {
                        n = [n[0] / nl, n[1] / nl, n[2] / nl];
                    }

                    // The daylight fragment model (see module docs).
                    let ndl = (n[0] * SUN_DIRECTION[0]
                        + n[1] * SUN_DIRECTION[1]
                        + n[2] * SUN_DIRECTION[2])
                        .clamp(0.0, 1.0);
                    // Nearest sample, wrapped (the art UVs leave [0, 1]);
                    // `None` = the shared white fallback (the GPU path
                    // binds white too).
                    let texel: [u8; 4] = match texture {
                        Some((bytes, tw, th)) if tw > 0 && th > 0 => {
                            let tx = (u.rem_euclid(1.0) * tw as f32) as u32 % tw;
                            let ty = (v.rem_euclid(1.0) * th as f32) as u32 % th;
                            let to = ((ty * tw + tx) * 4) as usize;
                            let mut t = [255u8; 4];
                            if to + 4 <= bytes.len() {
                                t.copy_from_slice(&bytes[to..to + 4]);
                            }
                            t
                        }
                        _ => [255u8, 255, 255, 255],
                    };
                    let mut out = [0u8; 4];
                    for ch in 0..3 {
                        let amb = daylight::AMBIENT[ch];
                        let sun_c = daylight::COLOUR[ch];
                        let v = (texel[ch] as f32 / 255.0) * (amb + sun_c * ndl);
                        out[ch] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                    }
                    out[3] = 255;
                    let o = di * 4;
                    rgba[o..o + 4].copy_from_slice(&out);
                }
            }
        }

    }

    let stats = frame_stats(&rgba, tri_drawn, tri_clipped);
    Ok(CpuMirrorFrame {
        width,
        height,
        rgba,
        stats,
    })
}

/// Edge function (signed doubled area) for the fill + barycentrics.
fn edge(a: (f32, f32), b: (f32, f32), p: (f32, f32)) -> f32 {
    (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
}

/// The non-blank statistics over a packed RGBA frame.
pub fn frame_stats(rgba: &[u8], tri_drawn: usize, tri_clipped: usize) -> FrameStats {
    use std::collections::HashSet;
    let mut distinct: HashSet<u32> = HashSet::new();
    let mut min = [255u8; 4];
    let mut max = [0u8; 4];
    for px in rgba.chunks_exact(4) {
        distinct.insert(u32::from_le_bytes([px[0], px[1], px[2], px[3]]));
        for ch in 0..4 {
            min[ch] = min[ch].min(px[ch]);
            max[ch] = max[ch].max(px[ch]);
        }
    }
    FrameStats {
        distinct: distinct.len(),
        min,
        max,
        triangles_drawn: tri_drawn,
        triangles_clipped: tri_clipped,
    }
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

    /// The REAL barrier mesh + its REAL in-member diffuse DDS through
    /// the CPU mirror: non-blank, textured (texel extrema vary), and
    /// almost every triangle reaches the rasteriser.
    #[test]
    fn cpu_mirror_barrier_mesh_renders_textured_and_nonblank() {
        let buf = fixture("art_barrier_os002.p3d");
        let top = prototype_p3d::geometry::top_level_chunks(&buf).unwrap();
        let shape = top
            .iter()
            .find(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_SHAPE)
            .map(|c| prototype_p3d::geometry::decode_shape(&buf, c.offset, c.size).unwrap())
            .unwrap();
        let decoded = shape.meshes.first().expect("the barrier shape has a mesh");
        let mesh = SceneMesh::from_p3d_mesh(decoded);
        assert_eq!(mesh.triangle_count(), 358);

        let dds = top
            .iter()
            .find(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_TEXTURE)
            .map(|c| prototype_p3d::geometry::decode_texture(&buf, c.offset, c.size).unwrap().dds)
            .expect("the barrier member embeds its diffuse");
        let surfaces = crate::dxt::decode_dds_mips(&dds).expect("diffuse decodes");
        let base = surfaces.first().unwrap();
        assert_eq!((base.width, base.height), (256, 256));

        let cam = frame_camera_for(&mesh);
        let frame = render_mesh_cpu(
            &mesh,
            &cam,
            Some((&base.rgba, base.width, base.height)),
            320,
            180,
            [10, 12, 16, 255],
        )
        .expect("the barrier renders");

        assert!(
            frame.stats.triangles_drawn >= 300,
            "almost every triangle reaches the rasteriser: {:?}",
            frame.stats
        );
        assert!(frame.stats.distinct > 1_000, "distinct = {}", frame.stats.distinct);
        assert!(frame.stats.max[0] > 200, "max r = {}", frame.stats.max[0]);

        // The proven zero-dependency encoder turns the frame into the
        // shipped evidence PNG (the same call the CLI makes).
        let png = prototype_video::png::png_encode_rgba(frame.width, frame.height, &frame.rgba)
            .expect("PNG encode");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert!(png.len() > 8_000, "a real frame PNG, {} B", png.len());
    }

    /// The REAL Alex Mercer player-model member (`alex_shot_body`,
    /// art.rcf member hash 0xe404d36f, block 82): the five
    /// `alex_shot_body_AlexVest_*Shape` jacket damage-state variants
    /// decode as five 1 198-triangle meshes with the REAL
    /// `alex_body_shot_dm` diffuse, and the UpOne variant renders
    /// non-blank and textured through the CPU mirror — the geometry
    /// the block-82 turntable evidence was made from.
    #[test]
    fn cpu_mirror_alex_shot_body_vest_variants_render_textured() {
        let buf = fixture("art_alex_shot_body.p3d");
        let top = prototype_p3d::geometry::top_level_chunks(&buf).unwrap();
        let shapes: Vec<_> = top
            .iter()
            .filter(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_SHAPE)
            .map(|c| prototype_p3d::geometry::decode_shape(&buf, c.offset, c.size).unwrap())
            .collect();
        assert_eq!(shapes.len(), 5, "the five AlexVest damage states");
        for s in &shapes {
            assert!(s.name.starts_with("alex_shot_body_AlexVest_"), "{}", s.name);
            assert_eq!(s.meshes.len(), 1);
            assert_eq!(s.meshes[0].header.index_count as usize / 3, 1_198);
            assert!(!s.meshes[0].uvs.is_empty(), "textured (uv set present)");
        }

        // The member's REAL diffuse decodes.
        let tex = top
            .iter()
            .find(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_TEXTURE)
            .map(|c| prototype_p3d::geometry::decode_texture(&buf, c.offset, c.size).unwrap())
            .expect("alex_shot_body embeds its diffuse");
        let surfaces = crate::dxt::decode_dds_mips(&tex.dds).expect("diffuse decodes");
        let base = surfaces.first().unwrap();

        let up_one = shapes
            .iter()
            .find(|s| s.name.contains("UpOne"))
            .expect("UpOne variant");
        let mesh = SceneMesh::from_p3d_mesh(&up_one.meshes[0]);
        let cam = frame_camera_for(&mesh);
        let frame = render_mesh_cpu(
            &mesh,
            &cam,
            Some((&base.rgba, base.width, base.height)),
            320,
            180,
            [10, 12, 16, 255],
        )
        .expect("the jacket renders");
        assert_eq!(frame.stats.triangles_drawn, 1_198, "every tri rasterised");
        assert!(frame.stats.distinct > 100, "distinct = {}", frame.stats.distinct);
        assert!(frame.stats.max[0] > 60, "the red tribal tattoos show (max r = {})", frame.stats.max[0]);
    }

    /// The CPU mirror's daylight constants ARE the fragment rows the
    /// GPU harness uploads: pinned against
    /// `shader::world::daylight_state` on the REAL translated DLL
    /// `default` pair (the committed SM 3.0 fixtures).
    #[test]
    fn cpu_mirror_matches_daylight_rows_on_the_real_pair() {
        let vs = fixture("dll_default_vp_e34e60.sm30");
        let fs = fixture("dll_default_fp_e3b308.sm30");
        let plan = crate::shader::world::WorldPairPlan::from_blobs(&vs, &fs)
            .expect("the real pair plans");
        let rows = crate::shader::world::daylight_state(&plan).expect("daylight rows");
        let amb_reg = plan.fragment_names.float_register("p3dLight_Ambient").expect("ambient reg");
        let col_reg = plan.fragment_names.float_register("p3dLight_Colour").expect("colour reg");
        let mut amb: Option<[f32; 4]> = None;
        let mut col: Option<[f32; 4]> = None;
        for (r, v) in rows.floats() {
            if r == amb_reg {
                amb = Some(v);
            }
            if r == col_reg {
                col = Some(v);
            }
        }
        let (amb, col) = (amb.expect("ambient row"), col.expect("colour row"));
        for ch in 0..3 {
            assert!(
                (amb[ch] - daylight::AMBIENT[ch]).abs() < 1e-5,
                "ambient[{ch}]: {} vs {}",
                amb[ch],
                daylight::AMBIENT[ch]
            );
            assert!(
                (col[ch] - daylight::COLOUR[ch]).abs() < 1e-5,
                "colour[{ch}]: {} vs {}",
                col[ch],
                daylight::COLOUR[ch]
            );
        }
    }


    /// Block 88: multi-mesh compositing shares ONE depth buffer — a
    /// NEAR cube drawn FIRST keeps its pixels even though a FAR cube
    /// is rasterised second (a painter's merge would wrongly show the
    /// far cube on top).
    #[test]
    fn render_meshes_cpu_z_buffers_overlapping_meshes() {
        fn cube(center: [f32; 3], s: f32) -> SceneMesh {
            let mut positions = Vec::new();
            let mut normals = Vec::new();
            let faces: [([f32; 3], [f32; 3]); 6] = [
                ([0.0, 0.0, 1.0], [0.0, 0.0, 1.0]),
                ([0.0, 0.0, -1.0], [0.0, 0.0, -1.0]),
                ([1.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
                ([-1.0, 0.0, 0.0], [-1.0, 0.0, 0.0]),
                ([0.0, 1.0, 0.0], [0.0, 1.0, 0.0]),
                ([0.0, -1.0, 0.0], [0.0, -1.0, 0.0]),
            ];
            let mut indices: Vec<u16> = Vec::new();
            for (c, n) in faces {
                let (u, v) = if n[2].abs() == 1.0 {
                    ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0])
                } else if n[0].abs() == 1.0 {
                    ([0.0, 0.0, 1.0], [0.0, 1.0, 0.0])
                } else {
                    ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0])
                };
                let base = positions.len() as u16;
                for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                    positions.push([
                        center[0] + c[0] * s + u[0] * su * s + v[0] * sv * s,
                        center[1] + c[1] * s + u[1] * su * s + v[1] * sv * s,
                        center[2] + c[2] * s + u[2] * su * s + v[2] * sv * s,
                    ]);
                    normals.push(n);
                }
                indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            }
            SceneMesh {
                name: "cube".into(),
                desc: 0,
                positions,
                normals,
                uvs: Vec::new(),
                tangents: Vec::new(),
                indices,
                bound_box: None,
                bound_sphere: None,
                texture_dds: None,
                extra_textures: Vec::new(),
            }
        }
        // 1x1 flat textures: near = red, far = blue.
        let red = [255u8, 0, 0, 255];
        let blue = [0u8, 0, 255, 255];
        let near = cube([0.0, 0.0, 2.0], 1.0); // z=2, eye at z=5 → nearer
        let far = cube([0.0, 0.0, 0.0], 1.0);
        let cam = Camera::default();
        let frame = render_meshes_cpu(
            &[&near, &far],
            &[Some((&red, 1, 1)), Some((&blue, 1, 1))],
            &cam,
            96,
            96,
            [10, 12, 16, 255],
        )
        .expect("both cubes render");
        // the centre pixel must be the NEAR cube's red, not the far blue
        let (cx, cy) = (48usize, 48usize);
        let o = (cy * 96 + cx) * 4;
        let px = [frame.rgba[o], frame.rgba[o + 1], frame.rgba[o + 2]];
        assert!(
            px[0] > px[2],
            "near red must win the shared depth region, got {px:?}"
        );
        assert!(px[0] > 60, "red texel through daylight: {px:?}");
    }

    /// A synthetic 12-tri cube (device-free geometry sanity): every
    /// triangle reaches the rasteriser and the flat-shaded faces catch
    /// THREE distinct daylight levels (N·L = 0 / mid / near-1 against
    /// the pinned SUN_DIRECTION) — the lighting path really varies.
    #[test]
    fn cpu_mirror_synthetic_cube_renders() {
        let s = 1.0f32;
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        // 6 faces × 4 verts (flat-shaded, no sharing).
        let faces: [([f32; 3], [f32; 3]); 6] = [
            ([0.0, 0.0, 1.0], [0.0, 0.0, 1.0]),
            ([0.0, 0.0, -1.0], [0.0, 0.0, -1.0]),
            ([1.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
            ([-1.0, 0.0, 0.0], [-1.0, 0.0, 0.0]),
            ([0.0, 1.0, 0.0], [0.0, 1.0, 0.0]),
            ([0.0, -1.0, 0.0], [0.0, -1.0, 0.0]),
        ];
        let mut indices: Vec<u16> = Vec::new();
        for (c, n) in faces {
            let (u, v) = if n[2].abs() == 1.0 {
                ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0])
            } else if n[0].abs() == 1.0 {
                ([0.0, 0.0, 1.0], [0.0, 1.0, 0.0])
            } else {
                ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0])
            };
            let base = positions.len() as u16;
            for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                positions.push([
                    c[0] + u[0] * su * s + v[0] * sv * s,
                    c[1] + u[1] * su * s + v[1] * sv * s,
                    c[2] + u[2] * su * s + v[2] * sv * s,
                ]);
                normals.push(n);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        let mesh = SceneMesh {
            name: "synthetic_cube".into(),
            desc: 0,
            positions,
            normals,
            uvs: Vec::new(),
            tangents: Vec::new(),
            indices,
            bound_box: Some([-2.0, -2.0, -2.0, 2.0, 2.0, 2.0]),
            bound_sphere: Some([0.0, 0.0, 0.0, 3.46]),
            texture_dds: None,
            extra_textures: Vec::new(),
        };
        let mut cam = frame_camera_for(&mesh);
        cam.fov_y = std::f32::consts::FRAC_PI_3;
        let frame = render_mesh_cpu(&mesh, &cam, None, 96, 96, [10, 12, 16, 255])
            .expect("the cube renders");
        assert_eq!(frame.stats.triangles_drawn, 12, "every cube face reaches the rasteriser: {:?}", frame.stats);
        assert!(frame.stats.distinct >= 3, "three lit levels expected: {:?}", frame.stats);
    }
}

/// Small helper so the test module above reads fixtures without
/// repeating the path plumbing (kept private to this module).
#[cfg(test)]
trait PipeRead {
    fn pipe_read(&self) -> Vec<u8>;
}
#[cfg(test)]
impl PipeRead for std::path::PathBuf {
    fn pipe_read(&self) -> Vec<u8> {
        std::fs::read(self).unwrap_or_else(|e| panic!("missing fixture {self:?}: {e}"))
    }
}

// Block 111 evidence renderer — the REAL static world geometry rendered.
//
// One real cells.rcf GEOMETRY member (the streamed world's static
// meshes: buildings, streets, props — the 7,546-corpus classic Shape
// family, blocks 18-23 proven) is decoded through the REAL
// `prototype_p3d::geometry::decode_shape` path, bridged to
// [`prototype_render::render_art_world_cpu::render_meshes_cpu`] — the
// CPU mirror of the translated-DLL `default` shader harness with the
// REAL daylight rows (ambient (0.35,0.36,0.40), sun (1.0,0.96,0.88),
// N·L against SUN_DIRECTION) and a shared z-buffer — and rendered
// over three camera takes (high orbit, street-level fly, close
// orbit).  The block-110 REAL placement point cloud (nearby object
// members) is projected through the SAME camera and composited on
// top, so geometry and placements appear in one frame in their true
// relative positions.
//
// HONEST SCOPE (also on-screen): untextured — the world diffuse maps
// live in art.rcf, not in the cell members; flat daylight shading
// stands in until the texture chain reaches cells (documented
// approximation, same class as the block-90 --no-tex takes).
//
// usage: block111_world_geometry_evidence [member.p3d] [--tri-budget N]
//
// Everything on screen is REAL GAME DATA except the text overlays.

use prototype_demo::world_data::{load_dots, resolve_mesh_diffuses};
use prototype_math::Vec3;
use prototype_p3d::geometry::{decode_shape, top_level_chunks};
use prototype_render::render_art_world_cpu::{render_meshes_cpu, CpuMirrorFrame};
use prototype_render::render_gfxtext_cpu::{
    build_glyph_atlas, layout_text, render_gfxtext_cpu, GfxTextQuad,
};
use prototype_render::scene::{Camera, SceneMesh};
use prototype_render::world_map::MapDot;
use prototype_video::png::png_encode_rgba;

const DEFAULT_MEMBER: &str = "/home/z/my-project/source/cells_world/be6215bf.p3d";
const DOTS_DIR: &str = "/home/z/my-project/source/cells_world";
const OUT_DIR: &str = "/tmp/block111_frames";
const W: u32 = 480;
const H: u32 = 360;
const FRAMES: u32 = 150;
const FPS: u32 = 30;

/// Push one text row (pixel-space anchor) as gfxtext quads.
fn add_text(
    quads: &mut Vec<GfxTextQuad>,
    atlas: &prototype_render::render_gfxtext_cpu::GlyphAtlas,
    text: &str,
    x_px: f32,
    y_px: f32,
    colour: [f32; 4],
) {
    let x_clip = 2.0 * x_px / W as f32 - 1.0;
    let top_clip = 1.0 - 2.0 * y_px / H as f32;
    let glyph_w = 7.0 / W as f32 * 2.0;
    let glyph_h = 14.0 / H as f32 * 2.0;
    let gap = 2.0 / W as f32 * 2.0;
    let row = layout_text(atlas, text, x_clip, top_clip, glyph_w, glyph_h, gap, colour);
    quads.extend(row);
}

/// Collect (mesh, tri-count) pairs from every classic Shape chunk in
/// the member, in file order, until `tri_budget` is reached.  Returns
/// the SceneMeshes + decode stats.
fn load_geometry(buf: &[u8], tri_budget: usize) -> (Vec<SceneMesh>, usize, usize, usize) {
    let tops = top_level_chunks(buf).expect("valid P3D");
    let mut meshes = Vec::new();
    let mut shapes = 0usize;
    let mut tris = 0usize;
    let mut verts = 0usize;
    for c in &tops {
        // classic (0x00010000) and skinned (0x00010001) Shapes both
        // decode through decode_shape (cells carry the classic family)
        if c.chunk_type != 0x0001_0000 && c.chunk_type != 0x0001_0001 {
            continue;
        }
        shapes += 1;
        let shape = match decode_shape(buf, c.offset, c.size) {
            Ok(s) => s,
            Err(_) => continue,
        };
        for m in &shape.meshes {
            let t = m.indices.len() / 3;
            if tris + t > tri_budget {
                continue;
            }
            tris += t;
            verts += m.positions.len();
            meshes.push(SceneMesh::from_p3d_mesh(m));
        }
    }
    (meshes, shapes, verts, tris)
}

/// Joint bbox over the drawn meshes.
fn joint_bounds(meshes: &[SceneMesh]) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for m in meshes {
        for p in &m.positions {
            for a in 0..3 {
                min[a] = min[a].min(p[a]);
                max[a] = max[a].max(p[a]);
            }
        }
    }
    (min, max)
}

/// Project world dots through the SAME camera the geometry used and
/// splat them over the rasterized frame (near-culled, clamped).
fn paint_dots_over(
    fb: &mut [u8],
    cam: &Camera,
    dots: &[MapDot],
    w: u32,
    h: u32,
) -> usize {
    let mvp = cam.view_projection();
    let mut drawn = 0usize;
    for d in dots {
        let c = mvp.transform_point4(Vec3::new(d.pos[0], d.pos[1], d.pos[2]));
        if c.w <= 1.0 {
            continue; // behind / at the near plane
        }
        let ndc_x = c.x / c.w;
        let ndc_y = c.y / c.w;
        if !(-1.3..=1.3).contains(&ndc_x) || !(-1.3..=1.3).contains(&ndc_y) {
            continue;
        }
        // block-88 orientation: row 0 = image TOP, py = 0.5 + sy*0.5
        let sx = (ndc_x * 0.5 + 0.5) * (w as f32 - 1.0);
        let sy = (0.5 + ndc_y * 0.5) * (h as f32 - 1.0);
        if !(sx.is_finite() && sy.is_finite()) {
            continue;
        }
        let rgb = d.kind.colour();
        let x0 = sx.floor() as i64;
        let y0 = sy.floor() as i64;
        for dy in 0..2i64 {
            for dx in 0..2i64 {
                let x = x0 + dx;
                let y = y0 + dy;
                if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
                    continue;
                }
                let o = ((y * w as i64 + x) * 4) as usize;
                // brighten over the raster (additive-ish, clamped)
                fb[o] = fb[o].saturating_add((rgb[0] as u32 * 3 / 5) as u8);
                fb[o + 1] = fb[o + 1].saturating_add((rgb[1] as u32 * 3 / 5) as u8);
                fb[o + 2] = fb[o + 2].saturating_add((rgb[2] as u32 * 3 / 5) as u8);
            }
        }
        drawn += 1;
    }
    drawn
}

fn write_frame(path: &str, fb: &[u8]) {
    let bytes = png_encode_rgba(W, H, fb).expect("png encode");
    std::fs::write(path, bytes).expect("write png");
}

fn main() {
    std::fs::create_dir_all(OUT_DIR).expect("frame dir");
    let atlas = build_glyph_atlas(12);

    // ---- args ----------------------------------------------------------
    let mut args = std::env::args().skip(1);
    let member_path = args.next().unwrap_or_else(|| DEFAULT_MEMBER.to_string());
    let tri_budget: usize = args
        .next()
        .and_then(|a| a.parse().ok())
        .unwrap_or(150_000);

    // ---- decode the real member ---------------------------------------
    let buf = std::fs::read(&member_path).expect("member file");
    let (meshes, shapes, verts, tris) = load_geometry(&buf, tri_budget);
    let (min, max) = joint_bounds(&meshes);

    // ---- the block-17 diffuse chain (mesh→shader→color→texture) -------
    let owned_diffuses = resolve_mesh_diffuses(&buf, &meshes);
    let resolved = owned_diffuses.iter().filter(|t| t.is_some()).count();
    println!(
        "member {}: shapes={shapes} meshes={} verts={verts} tris={tris}; \
         bbox x[{:.1},{:.1}] y[{:.1},{:.1}] z[{:.1},{:.1}]",
        member_path,
        meshes.len(),
        min[0],
        max[0],
        min[1],
        max[1],
        min[2],
        max[2],
    );
    println!("diffuses resolved via mesh→shader→color chain: {resolved}/{}", meshes.len());
    assert!(!meshes.is_empty(), "no shapes decoded");

    // ---- nearby real placements ---------------------------------------
    let (dots, _members, _objects) = load_dots(DOTS_DIR);
    let near: Vec<MapDot> = dots
        .iter()
        .filter(|d| {
            d.pos[0] > min[0] - 400.0
                && d.pos[0] < max[0] + 400.0
                && d.pos[2] > min[2] - 400.0
                && d.pos[2] < max[2] + 400.0
        })
        .copied()
        .collect();
    println!("placement dots nearby: {} of {}", near.len(), dots.len());

    // ---- camera frame --------------------------------------------------
    let center = [
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ];
    let extent = [
        max[0] - min[0],
        max[1] - min[1],
        max[2] - min[2],
    ];
    let radius = (extent[0] * extent[0] + extent[1] * extent[1] + extent[2] * extent[2]).sqrt() * 0.5;
    println!(
        "center [{:.1},{:.1},{:.1}] radius {:.1}; extent x{:.1} y{:.1} z{:.1}",
        center[0], center[1], center[2], radius, extent[0], extent[1], extent[2]
    );

    let clear = [8u8, 10, 20, 255];
    let (a_end, b_end) = (60u32, 120u32);
    let mut min_tris_drawn = usize::MAX;

    for f in 0..FRAMES {
        let cam: Camera;
        let phase: &str;
        if f < a_end {
            // ---- Take A: high orbit -----------------------------------
            let t = f as f32 / (a_end - 1) as f32;
            let ang = t * std::f32::consts::PI * 2.0;
            let r = radius * 1.8;
            cam = Camera {
                eye: Vec3::new(
                    center[0] + ang.cos() * r,
                    center[1] + radius * 0.9,
                    center[2] + ang.sin() * r,
                ),
                target: Vec3::new(center[0], center[1], center[2]),
                up: Vec3::UP,
                fov_y: 55.0f32.to_radians(),
                aspect: W as f32 / H as f32,
                near: 1.0,
                far: radius * 6.0,
            };
            phase = "TAKE A: HIGH ORBIT - REAL CELL GEOMETRY";
        } else if f < b_end {
            // ---- Take B: low sweep just above the rooftops ------------
            // (OUTSIDE the member's x/z envelope: the CPU mirror has a
            // per-triangle w-guard instead of a real near-plane clipper
            // — documented approximation — so an envelope-interior
            // street fly would vanish most triangles whose vertices
            // straddle the near plane.  A low orbit keeps every vertex
            // in front of the eye while reading close to true scale.)
            let t = (f - a_end) as f32 / (b_end - a_end - 1) as f32;
            let ang = t * std::f32::consts::PI * 1.4 + 2.4;
            let r = radius * 1.15;
            cam = Camera {
                eye: Vec3::new(
                    center[0] + ang.cos() * r,
                    min[1] + 42.0,
                    center[2] + ang.sin() * r,
                ),
                target: Vec3::new(center[0], min[1] + 8.0, center[2]),
                up: Vec3::UP,
                fov_y: 60.0f32.to_radians(),
                aspect: W as f32 / H as f32,
                near: 0.8,
                far: radius * 5.0,
            };
            phase = "TAKE B: LOW SWEEP - JUST ABOVE THE ROOFTOPS";
        } else {
            // ---- Take C: close orbit ----------------------------------
            let t = (f - b_end) as f32 / (FRAMES - b_end - 1) as f32;
            let ang = t * std::f32::consts::PI * 1.6 + 0.6;
            let r = radius * 0.85;
            cam = Camera {
                eye: Vec3::new(
                    center[0] + ang.cos() * r,
                    center[1] + radius * 0.45,
                    center[2] + ang.sin() * r,
                ),
                target: Vec3::new(center[0], center[1] + radius * 0.1, center[2]),
                up: Vec3::UP,
                fov_y: 60.0f32.to_radians(),
                aspect: W as f32 / H as f32,
                near: 0.8,
                far: radius * 4.0,
            };
            phase = "TAKE C: CLOSE ORBIT + REAL PLACEMENTS";
        }

        // ---- raster the real geometry (translated-DLL daylight rows) --
        let textures: Vec<Option<(&[u8], u32, u32)>> = owned_diffuses
            .iter()
            .map(|t| t.as_ref().map(|(rgba, w, h)| (rgba.as_slice(), *w, *h)))
            .collect();
        let refs: Vec<&SceneMesh> = meshes.iter().collect();
        let frame: CpuMirrorFrame =
            render_meshes_cpu(&refs, &textures, &cam, W, H, clear).expect("raster");
        let mut fb = frame.rgba;
        min_tris_drawn = min_tris_drawn.min(frame.stats.triangles_drawn);

        // ---- composite the real placement dots ------------------------
        let dots_drawn = paint_dots_over(&mut fb, &cam, &near, W, H);

        // ---- text ------------------------------------------------------
        let mut quads: Vec<GfxTextQuad> = Vec::new();
        add_text(
            &mut quads,
            &atlas,
            "REAL STATIC WORLD GEOMETRY (CELL MEMBER)",
            10.0,
            10.0,
            [1.0, 0.85, 0.3, 1.0],
        );
        add_text(&mut quads, &atlas, phase, 10.0, 28.0, [0.9, 0.9, 0.95, 1.0]);
        add_text(
            &mut quads,
            &atlas,
            &format!(
                "{} SHAPES {} TRIS - TEXTURED {}/{}",
                shapes, tris, resolved, meshes.len()
            ),
            10.0,
            46.0,
            [0.75, 0.85, 1.0, 1.0],
        );
        add_text(
            &mut quads,
            &atlas,
            &format!("{} REAL PLACEMENTS OVERLAID", dots_drawn),
            10.0,
            H as f32 - 24.0,
            [0.7, 0.85, 0.7, 1.0],
        );
        add_text(
            &mut quads,
            &atlas,
            "IN-MEMBER DDS DIFFUSES VIA MESH-SHADER-COLOR CHAIN",
            10.0,
            H as f32 - 42.0,
            [0.75, 0.75, 0.8, 1.0],
        );

        let text_frame = render_gfxtext_cpu(&quads, &atlas, Some(&fb), W, H).expect("text");
        write_frame(&format!("{OUT_DIR}/frame_{f:04}.png"), &text_frame.rgba);
    }

    println!(
        "wrote {FRAMES} frames to {OUT_DIR}; min tris drawn/frame = {min_tris_drawn}; \
         encode: ffmpeg -y -framerate {FPS} -i {OUT_DIR}/frame_%04d.png -c:v libx264 \
         -pix_fmt yuv420p -crf 28 -preset veryfast -movflags +faststart <out.mp4>"
    );
}

// Block 110 evidence renderer — the REAL streamed world rendered.
//
// The placements come from the block-18/41 PROVEN decoders
// (`prototype_p3d::world::scan_world_member_opts`) over a surgical
// Range-fetched SAMPLE of cells.rcf members (scripts/
// m110_fetch_cells_world.py + m110b_fetch_object_members.py): 36 object
// members spread across the whole archive (offsets 5 MB → 639 MB),
// ~19.6k real placed world objects (spawners, cars, props, light
// decals, audio exclusion volumes, web-swing locators) spanning the
// REAL Prototype 2009 map (x[-1752,1708] z[-2713,1658]).
//
// Three takes:
//   A. CELL-SCALE top-down (zoom-out from a real cell region)
//   B. FULL-MAP top-down (the whole island from real placements)
//   C. 3D FLYTHROUGH (perspective flight over the real point cloud)
//
// Compile + run:
//   cargo run --release -p prototype-demo --bin block110_world_map_evidence
//
// Everything on screen is REAL GAME DATA except the text overlays
// (our own labels, rendered through the block-106 gfxtext CPU port).

use prototype_demo::world_data::load_dots;
use prototype_render::render_gfxtext_cpu::{
    build_glyph_atlas, layout_text, render_gfxtext_cpu, GfxTextQuad,
};
use prototype_render::world_map::{
    fly_path, map_bounds, paint_flythrough, paint_map_dots, pan_path, zoom_out_path, MapView,
};
use prototype_video::png::png_encode_rgba;

const DATA_DIR: &str = "/home/z/my-project/source/cells_world";
const OUT_DIR: &str = "/tmp/block110_frames";
const W: u32 = 480;
const H: u32 = 360;
const FRAMES: u32 = 150;
const FPS: u32 = 30;

fn write_frame(path: &str, fb: &[u8]) {
    let bytes = png_encode_rgba(W, H, fb).expect("png encode");
    std::fs::write(path, bytes).expect("write png");
}

fn dark_background() -> Vec<u8> {
    let mut fb = vec![0u8; (W * H * 4) as usize];
    for y in 0..H {
        for x in 0..W {
            let o = ((y * W + x) * 4) as usize;
            let t = y as f32 / H as f32;
            fb[o] = (6.0 + 10.0 * t) as u8;
            fb[o + 1] = (6.0 + 8.0 * t) as u8;
            fb[o + 2] = (16.0 + 26.0 * t) as u8;
            fb[o + 3] = 255;
        }
    }
    fb
}

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

fn main() {
    std::fs::create_dir_all(OUT_DIR).expect("frame dir");
    let atlas = build_glyph_atlas(12);

    let (dots, members, objects_seen) = load_dots(DATA_DIR);
    let (min, max) = map_bounds(&dots).expect("non-empty dot set");
    println!(
        "loaded {members} members, {objects_seen} GameObject chunks, {} placed dots; \
         map x[{minx:.0},{maxx:.0}] y[{miny:.0},{maxy:.0}] z[{minz:.0},{maxz:.0}]",
        dots.len(),
        minx = min[0],
        maxx = max[0],
        miny = min[1],
        maxy = max[1],
        minz = min[2],
        maxz = max[2],
    );

    // The real map bbox centres (corpus span x[-1752,1708] z[-2713,1658]).
    let map_center: [f32; 2] = [(min[0] + max[0]) * 0.5, (min[2] + max[2]) * 0.5];
    // Cell-82 region (the block-22/23 pinned transforms ~ (250, 335)).
    let cell_center: [f32; 2] = [250.0, 335.0];

    // Phase boundaries: A = 0..45 (cell zoom-out), B = 45..95 (map), C = 95..150 (flythrough).
    let (a_end, b_end) = (45u32, 95u32);
    let mut min_drawn = usize::MAX;

    for f in 0..FRAMES {
        let mut fb = dark_background();
        let mut quads: Vec<GfxTextQuad> = Vec::new();

        if f < a_end {
            // ---- Phase A: cell-scale zoom-out --------------------------------
            let t = f as f32 / (a_end - 1) as f32;
            let span_near = 500.0;
            let span_far = 2600.0;
            let view = zoom_out_path(cell_center, span_near, span_far, t);
            let landed = paint_map_dots(&mut fb, W, H, &dots, &view);
            add_text(
                &mut quads,
                &atlas,
                "REAL CELLS.RCF PLACEMENT DATA",
                10.0,
                28.0,
                [0.9, 0.9, 0.95, 1.0],
            );
            add_text(
                &mut quads,
                &atlas,
                "TAKE A: CELL-SCALE TOP-DOWN (CELL 82 REGION)",
                10.0,
                46.0,
                [0.75, 0.85, 1.0, 1.0],
            );
            add_text(
                &mut quads,
                &atlas,
                &format!("{} DOTS FROM {} REAL MEMBERS", dots.len(), members),
                10.0,
                H as f32 - 24.0,
                [0.7, 0.85, 0.7, 1.0],
            );
            let _ = landed;
        } else if f < b_end {
            // ---- Phase B: full-map top-down ----------------------------------
            let t = (f - a_end) as f32 / (b_end - a_end - 1) as f32;
            // zoom out from the cell view to the WHOLE map, drifting the
            // centre from the cell region to the map centre.
            let near = 2600.0;
            let far = 6000.0;
            let start_center = cell_center;
            let end_center = map_center;
            let zoom_view = zoom_out_path(start_center, near, far, t);
            let pan_view = pan_path(start_center, end_center, zoom_view.span_x, t);
            let view = MapView {
                center: pan_view.center,
                span_x: zoom_view.span_x,
            };
            let landed = paint_map_dots(&mut fb, W, H, &dots, &view);
            add_text(
                &mut quads,
                &atlas,
                "TAKE B: THE WHOLE ISLAND FROM REAL PLACEMENTS",
                10.0,
                28.0,
                [0.9, 0.9, 0.95, 1.0],
            );
            add_text(
                &mut quads,
                &atlas,
                &format!(
                    "X[{:.0},{:.0}] Z[{:.0},{:.0}] REAL CELLS.RCF DATA",
                    min[0], max[0], min[2], max[2]
                ),
                10.0,
                H as f32 - 24.0,
                [0.7, 0.85, 0.7, 1.0],
            );
            let _ = landed;
        } else {
            // ---- Phase C: 3D flythrough --------------------------------------
            let t = (f - b_end) as f32 / (FRAMES - b_end - 1) as f32;
            // south-west quadrant → mid-north-east, staying INSIDE the
            // point cloud so the take ends with dots still in view
            let a = [min[0] * 0.6, min[2] * 0.75];
            let b = [max[0] * 0.6, max[2] * 0.65];
            let cam = fly_path(a, b, 60.0, 420.0, t, W as f32 / H as f32);
            let drawn = paint_flythrough(&mut fb, W, H, &dots, &cam);
            min_drawn = min_drawn.min(drawn);
            add_text(
                &mut quads,
                &atlas,
                "TAKE C: 3D FLYTHROUGH - REAL POINT CLOUD",
                10.0,
                28.0,
                [0.9, 0.9, 0.95, 1.0],
            );
            add_text(
                &mut quads,
                &atlas,
                &format!("{} DOTS DRAWN THIS FRAME", drawn),
                10.0,
                H as f32 - 24.0,
                [0.7, 0.85, 0.7, 1.0],
            );
        }

        // Common header on every frame.
        add_text(
            &mut quads,
            &atlas,
            "PROTOTYPE-RS BLOCK 110",
            10.0,
            10.0,
            [1.0, 0.85, 0.3, 1.0],
        );
        add_text(
            &mut quads,
            &atlas,
            "36/520 MEMBERS SAMPLED - HONEST COVERAGE",
            10.0,
            H as f32 - 42.0,
            [0.75, 0.75, 0.8, 1.0],
        );

        let text_frame = render_gfxtext_cpu(&quads, &atlas, Some(&fb), W, H).expect("text");
        write_frame(&format!("{OUT_DIR}/frame_{f:04}.png"), &text_frame.rgba);
    }

    println!(
        "wrote {FRAMES} frames to {OUT_DIR}; flythrough min dots/frame = {min_drawn}; \
         encode with: ffmpeg -y -framerate {FPS} \
         -i {OUT_DIR}/frame_%04d.png -c:v libx264 -pix_fmt yuv420p -crf 28 -preset \
         veryfast -movflags +faststart <out.mp4>"
    );
}

// Block 106 evidence renderer — REAL game text from the REAL text bible
// (art/hud/fe_textbible.p3d, surgical-fetched from art.rcf) through the
// REAL gfxtextshader CPU port.
//
// Compile + run:
//   cargo run --release -p prototype-demo --bin block106_real_text_evidence
//
// The strings used here are EXTRACTED FROM THE REAL GAME'S TEXT BIBLE —
// block 104 used a hand-picked census of mission-name table strings;
// block 106 uses the REAL frontend text-bible strings (MAIN MENU,
// NEW GAME, CONTINUE, CLAWS, HAMMERFIST, WEB OF INTRIGUE, etc.) and
// renders them through the same gfxtextshader CPU port that pins
// the shader contract.

use prototype_render::render_gfxtext_cpu::{
    build_glyph_atlas, layout_text, render_gfxtext_cpu, GlyphAtlas, GfxTextQuad,
};
use prototype_video::png::png_encode_rgba;

/// The curated list of REAL game strings extracted from
/// art/hud/fe_textbible.p3d (block 106 surgical fetch).
const REAL_GAME_STRINGS: &[&str] = &[
    "MAIN MENU",
    "NEW GAME",
    "CONTINUE",
    "PAUSE GAME",
    "OPTIONS",
    "CONTROLS",
    "AUDIO",
    "VIDEO",
    "CREDITS",
    "QUIT",
    "ATTACK",
    "GRAB",
    "JUMP",
    "WALK",
    "RUN",
    "CLAWS",
    "HAMMERFIST",
    "MUSCLEMASS",
    "WHIPFIST",
    "ARMOR",
    "SHIELD",
    "DISGUISE",
    "BLADE",
    "INFECTED VISION",
    "THERMAL VISION",
    "WEB OF INTRIGUE",
    "MILITARY",
    "INFECTED",
    "HELICOPTER",
    "TANK",
    "Alex Mercer",
    "Manhattan",
    "Hive",
    "Base",
    "Consume",
    "WOI",
    "Saving...",
    "Difficulty",
    "Xbox 360 Controller",
];

fn write_png(path: &str, w: u32, h: u32, fb: &[u8]) {
    let bytes = png_encode_rgba(w, h, fb).expect("png encode");
    std::fs::write(path, bytes).expect("write");
}

fn main() {
    let w = 640u32;
    let h = 480u32;
    std::fs::create_dir_all("/tmp/block106_frames").unwrap();

    // Build the REAL glyph atlas (the same one the gfxtextshader CPU port uses).
    let atlas: GlyphAtlas = build_glyph_atlas(12);

    // Render 150 frames: a vertical list of the real game strings,
    // scrolling upward, with a golden highlight sweeping the list
    // (a "menu selection" animation, mirroring block 104's pattern).
    let total_frames = 150u32;
    for f in 0..total_frames {
        let mut fb = vec![0u8; (w * h * 4) as usize];
        // Frontend background: a dark blue gradient (DESIGN — the real
        // frontend background is a streaming .p3d art asset we haven't
        // fetched yet; this stands in for the gradient the real menu
        // sits over).
        for y in 0..h {
            for x in 0..w {
                let o = ((y * w + x) * 4) as usize;
                let r = (10 + (y as u32 * 8 / h) as u32) as u8;
                let g = (8 + (y as u32 * 12 / h) as u32) as u8;
                let b = (30 + (y as u32 * 50 / h) as u32) as u8;
                fb[o] = r;
                fb[o + 1] = g;
                fb[o + 2] = b;
                fb[o + 3] = 255;
            }
        }

        // Layout: 9 visible rows at a time. Start index = floor(f / 5).
        let start = (f / 5) as usize % REAL_GAME_STRINGS.len();
        let row_h = 32.0f32;
        let top_y = 60.0f32;
        let highlighted_idx = (f / 10) as usize % 9; // index within the visible window
        let cxmul = [1.0f32; 4];

        let mut quads: Vec<GfxTextQuad> = Vec::new();
        for i in 0..9 {
            let str_idx = (start + i) % REAL_GAME_STRINGS.len();
            let label = REAL_GAME_STRINGS[str_idx];
            // Clip-space y for this row.
            let row_y = top_y + (i as f32 * row_h);
            // Convert to clip space: y_clip = (1 - 2 * y_pixel / h_pixel)
            // (the gfxtext rasterizer expects clip-space quads in [-1,1]).
            let y_clip = 1.0 - 2.0 * row_y / h as f32;
            // Highlight the selected row in gold; others in white.
            let is_highlighted = i == highlighted_idx;
            let colour = if is_highlighted {
                [1.0, 0.85, 0.3, 1.0] // gold
            } else {
                [0.9, 0.9, 0.95, 1.0] // off-white
            };
            // For the highlighted row, push the cxadd to brighten it.
            let row_cxmul = if is_highlighted {
                [1.2, 1.0, 0.6, 1.0]
            } else {
                cxmul
            };
            let row_cxadd: [f32; 4] = if is_highlighted {
                [0.1, 0.05, 0.0, 0.0]
            } else {
                [0.0; 4]
            };

            // Layout the text at clip-space position. The gfxtext
            // rasterizer's layout_text uses (x, y) in CLIP space where
            // y is the TOP of the glyph (y_clip is the bottom of the
            // glyph rect).
            let glyph_h = 16.0f32 / h as f32 * 2.0; // ~16 px high
            let glyph_w = 8.0f32 / w as f32 * 2.0; // ~8 px wide
            let x_clip = -0.85; // ~10% from left
            let top_clip = y_clip;
            // layout_text wants (x, y) where the rect is [bl, br, tr, tl]
            // and y is the TOP — so pass top_clip; bottom = top - glyph_h.
            let row_quads = layout_text(
                &atlas,
                label,
                x_clip,
                top_clip,
                glyph_w,
                glyph_h,
                2.0 / w as f32 * 2.0, // gap in clip space
                colour,
            );
            // Adjust each quad's cxmul/cxadd for this row.
            for mut q in row_quads {
                q.cxmul = row_cxmul;
                q.cxadd = row_cxadd;
                quads.push(q);
            }

            // Draw a horizontal underline bar for the highlighted row.
            if is_highlighted {
                let bar_top = row_y + 18.0;
                let bar_h = 2.0f32 as u32;
                let bar_w = (w as f32 * 0.6) as u32;
                let bar_x = (w as f32 * 0.1) as i32;
                for y in (bar_top as u32)..(bar_top as u32 + bar_h) {
                    for x in bar_x..(bar_x + bar_w as i32) {
                        let o = ((y * w + x as u32) * 4) as usize;
                        if o + 3 < fb.len() {
                            fb[o] = 220;
                            fb[o + 1] = 180;
                            fb[o + 2] = 60;
                            fb[o + 3] = 255;
                        }
                    }
                }
            }
        }

        // Now render the text quads through the REAL gfxtext CPU port.
        let frame = render_gfxtext_cpu(&quads, &atlas, Some(&fb), w, h)
            .expect("gfxtext render");
        // frame.rgba is RGBA8.
        write_png(
            &format!("/tmp/block106_frames/f_{:04}.png", f),
            w,
            h,
            &frame.rgba,
        );
    }

    println!("wrote {} frames to /tmp/block106_frames/", total_frames);
}

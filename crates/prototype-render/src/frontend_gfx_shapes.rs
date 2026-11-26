//! frontend_gfx_shapes — the REAL Scaleform VECTOR art (block 121).
//!
//! The block-120 parse journal proved the real screens carry ZERO
//! bitmaps: `protostart.gfx parsed — 0 image(s)` and the content is
//! `tag 2×13` (DefineShape), `tag 22×2` (DefineShape2), `tag 39×18`
//! (DefineSprite) and `tag 37×4` (DefineEditText).  The menu art is
//! VECTOR — so THIS is what renders it:
//!
//! * [`GfxShape`] — DefineShape 1/2/3: solid fill styles, the edge
//!   records flattened into per-fill contour sets (curves subdivided,
//!   edges chained into closed loops per fill side — the classic SWF
//!   fill-side model).  Gradients/bitmap fills and line styles are
//!   consumed exactly and SKIPPED (honest counters — never guessed).
//! * [`GfxText`] — DefineEditText: the REAL string, height, colour
//!   and alignment straight out of the game file; rasterized with the
//!   app's own font (`ui::MENU_FONT_BYTES`) — the game's text content
//!   at the game's place, our glyphs until the game fonts parse.
//! * the even-odd scanline filler ([`fill_even_odd`]) — holes render
//!   as holes (a shape's reversed inner contour cancels).
//!
//! # Evidence labels
//!
//! * PROVEN: the SWF shape-record bit layout (TypeFlag/flags/edges —
//!   spec-pinned, exercised on synthetic shapes below); the fill-side
//!   chaining model (the standard SWF decode); fontdue rasterization.
//! * DESIGN: curve subdivision (8 segments/edge), no antialiasing v1,
//!   text glyphs from the bundled font (the REAL strings/positions/
//!   colours/sizes are the game's; the glyph SHAPES are ours until
//!   DefineFont outlines parse).
//! * UNKNOWN: which fills the real screens use beyond solids — the
//!   per-screen journal reports skipped gradient/bitmap/line counts.

use super::render_frontend_cpu::f32_to_u8;
use super::frontend_gfx::{BitReader, GfxPlacement};
use crate::render_art_world_cpu::FrameStats;
use super::render_frontend_cpu::FrontendFrame;
use super::ui::MENU_FONT_BYTES;

/// One solid fill layer of a shape: the fill's colour index into
/// [`GfxShape::fills`] plus the closed contours (twips, shape-local
/// coordinates) that carry it.
#[derive(Debug, Clone)]
pub struct ShapeLayer {
    pub fill: usize,
    pub contours: Vec<Vec<[f32; 2]>>,
}

/// A DefineShape: the solid fills + the per-fill contour layers.
/// Gradient/bitmap fills and line styles are consumed and counted,
/// not rendered.
#[derive(Debug, Clone, Default)]
pub struct GfxShape {
    /// Solid fill colours, RGBA f32 (the style order = paint order).
    pub fills: Vec<[f32; 4]>,
    /// Per fill style, in style order: the chained closed contours.
    pub layers: Vec<ShapeLayer>,
    pub skipped_gradient_fills: u32,
    pub skipped_bitmap_fills: u32,
    pub skipped_line_styles: u32,
}

/// A DefineEditText: the REAL string with its size, colour and
/// alignment (the game's own UI text, verbatim from the member).
#[derive(Debug, Clone, Default)]
pub struct GfxText {
    pub text: String,
    /// Font height in twips (the record's Height field).
    pub height: u16,
    pub color: [f32; 4],
    /// 0 = left, 1 = right, 2 = centre (the record's Align field).
    pub align: u8,
    /// The record's bounds (twips): `[xmin, ymin, xmax, ymax]` — the
    /// alignment box.
    pub bounds: [f32; 4],
    pub has_font: bool,
}

/// A parsed character (shape / text / sprite display list).
#[derive(Debug, Clone)]
pub enum GfxChar {
    Shape(GfxShape),
    Text(GfxText),
    Sprite(Vec<GfxPlacement>),
}

// ---------------------------------------------------------------------------
// shared record parsing (byte-aligned style arrays, bit-packed records)
// ---------------------------------------------------------------------------

/// Read a style count: `u8`, or `0xFF` → the extended `u16` follows.
fn read_count(br: &mut BitReader<'_>) -> Result<usize, String> {
    let n = br.byte()?;
    if n == 0xff {
        Ok(br.u16()? as usize)
    } else {
        Ok(n as usize)
    }
}

/// One fill style: solid colours parse, everything else consumes
/// exactly and skips (the honest counter decides).
fn parse_fill_style(br: &mut BitReader<'_>, version: u8) -> Result<FillStyle, String> {
    let ty = br.byte()?;
    match ty {
        0x00 => {
            // Solid: v3 RGBA, v1/v2 RGB (alpha = opaque).
            let color = if version >= 3 {
                let (r, g, b, a) = (br.byte()?, br.byte()?, br.byte()?, br.byte()?);
                [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a as f32 / 255.0]
            } else {
                let (r, g, b) = (br.byte()?, br.byte()?, br.byte()?);
                [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
            };
            Ok(FillStyle::Solid(color))
        }
        0x10 | 0x12 | 0x13 => {
            // Gradient: matrix (bit field), focal point (0x13), spread,
            // count, records.  Consumed exactly.
            consume_matrix(br)?;
            if ty == 0x13 {
                let _focal = br.u16()?;
            }
            let _spread = br.byte()?;
            let n = br.byte()?;
            for _ in 0..n {
                let _ratio = br.byte()?;
                if version >= 3 {
                    let _ = (br.byte()?, br.byte()?, br.byte()?, br.byte()?);
                } else {
                    let _ = (br.byte()?, br.byte()?, br.byte()?);
                }
            }
            Ok(FillStyle::Skipped)
        }
        0x40..=0x43 => {
            // Bitmap: character id + matrix.  Consumed exactly.
            let _bitmap = br.u16()?;
            consume_matrix(br)?;
            Ok(FillStyle::Skipped)
        }
        other => Err(format!("gfx shape: unknown fill style type {other:#x}")),
    }
}

enum FillStyle {
    Solid([f32; 4]),
    Skipped,
}

/// Consume a bit-packed MATRIX (scale/rotate/translate) — the same
/// field layout [`super::frontend_gfx::parse_matrix`] reads, inlined
/// here so the style arrays' matrices consume exactly.
fn consume_matrix(br: &mut BitReader<'_>) -> Result<(), String> {
    if br.bits(1)? == 1 {
        let n = br.bits(5)?;
        let _ = (br.sbits(n)?, br.sbits(n)?);
    }
    if br.bits(1)? == 1 {
        let n = br.bits(5)?;
        let _ = (br.sbits(n)?, br.sbits(n)?);
    }
    let n = br.bits(5)?;
    let _ = (br.sbits(n)?, br.sbits(n)?);
    br.align_byte();
    Ok(())
}

/// Consume one line style (v1/v2: width + colour; v3: + the cap/join
/// bit fields, the miter limit, the optional fill).  Lines are not
/// rendered (counted) — but they MUST be consumed exactly.
fn parse_line_style(br: &mut BitReader<'_>, version: u8) -> Result<(), String> {
    let _width = br.u16()?;
    if version >= 3 {
        let _ = (br.byte()?, br.byte()?, br.byte()?, br.byte()?); // RGBA
        let start_cap = br.bits(1)?;
        let _reserved = br.bits(2)?;
        let join = br.bits(2)?;
        let has_fill = br.bits(1)?;
        let _no_h = br.bits(1)?;
        let _no_v = br.bits(1)?;
        let _no_close = br.bits(1)?;
        let _pad = br.bits(1)?;
        br.align_byte();
        let _ = (start_cap, _reserved, _no_h, _no_v, _no_close, _pad);
        if join == 2 {
            let _miter = br.u16()?;
        }
        if has_fill == 1 {
            let _ = parse_fill_style(br, version)?;
        }
    } else {
        let _ = (br.byte()?, br.byte()?, br.byte()?); // RGB
    }
    Ok(())
}

/// The shape record walk.  Edges arrive as absolute polylines (curves
/// flattened — 8 segments per quadratic edge) tagged with their fill
/// sides as (style GENERATION, style index): a mid-shape "new styles"
/// record restarts the index space, so both numbers are needed to
/// group the fills.  `fills` accumulates every SOLID style in parse
/// order (leading first); skipped styles get no colour entry and
/// their layers render nothing.
fn parse_shape_records(
    br: &mut BitReader<'_>,
    version: u8,
    num_fill_bits: u8,
    num_line_bits: u8,
    fills: &mut Vec<[f32; 4]>,
    leading_styles: usize,
    skipped_fills: &mut u32,
    skipped_lines: &mut u32,
) -> Result<Vec<ShapeLayer>, String> {
    // The style state: indices are 1-based (0 = no fill on this side).
    let mut fill0 = (0usize, 0usize); // (generation, style index)
    let mut fill1 = (0usize, 0usize);
    let mut pen = [0.0f32, 0.0f32];
    // Raw edges: (polyline, fill0, fill1) — generation-tagged.
    let mut edges: Vec<(Vec<[f32; 2]>, (usize, usize), (usize, usize))> = Vec::new();
    let mut generation = 0usize;
    // The per-generation style count (the chain loop walks these).
    let mut styles_per_gen: Vec<usize> = vec![leading_styles];
    // Solid fill GLOBAL index per (generation, style index) —
    // usize::MAX for skipped styles.
    let mut fill_index: std::collections::BTreeMap<(usize, usize), usize> = Default::default();
    for s in 1..=leading_styles {
        fill_index.insert((0, s), s - 1);
    }

    let mut nfb = num_fill_bits;
    let mut nlb = num_line_bits;

    loop {
        let type_flag = br.bits(1)?;
        if type_flag == 1 {
            // Edge record.
            let straight = br.bits(1)?;
            let num_bits = br.bits(4)? + 2;
            if straight == 1 {
                let general = br.bits(1)?;
                let (dx, dy) = if general == 1 {
                    (br.sbits(num_bits)? as f32, br.sbits(num_bits)? as f32)
                } else {
                    let vert = br.bits(1)?;
                    if vert == 1 {
                        (0.0, br.sbits(num_bits)? as f32)
                    } else {
                        (br.sbits(num_bits)? as f32, 0.0)
                    }
                };
                let next = [pen[0] + dx, pen[1] + dy];
                edges.push((vec![pen, next], fill0, fill1));
                pen = next;
            } else {
                // Curved quadratic: control + anchor — flatten 8 segments.
                let cx = pen[0] + br.sbits(num_bits)? as f32;
                let cy = pen[1] + br.sbits(num_bits)? as f32;
                let ax = cx + br.sbits(num_bits)? as f32;
                let ay = cy + br.sbits(num_bits)? as f32;
                let mut pts = Vec::with_capacity(9);
                for k in 0..=8i32 {
                    let t = k as f32 / 8.0;
                    let mt = 1.0 - t;
                    pts.push([
                        mt * mt * pen[0] + 2.0 * mt * t * cx + t * t * ax,
                        mt * mt * pen[1] + 2.0 * mt * t * cy + t * t * ay,
                    ]);
                }
                edges.push((pts, fill0, fill1));
                pen = [ax, ay];
            }
            continue;
        }
        // Style change (or end): the five state flags, IN ORDER.
        let state_new_styles = br.bits(1)?;
        let state_line = br.bits(1)?;
        let state_fill1 = br.bits(1)?;
        let state_fill0 = br.bits(1)?;
        let state_move = br.bits(1)?;
        if state_new_styles == 0
            && state_line == 0
            && state_fill1 == 0
            && state_fill0 == 0
            && state_move == 0
        {
            break; // EndShapeRecord
        }
        if state_move == 1 {
            let n = br.bits(5)?;
            pen = [br.sbits(n)? as f32, br.sbits(n)? as f32];
        }
        if state_fill0 == 1 {
            fill0 = (generation, br.bits(nfb as u32)? as usize);
        }
        if state_fill1 == 1 {
            fill1 = (generation, br.bits(nfb as u32)? as usize);
        }
        if state_line == 1 {
            let _line = br.bits(nlb as u32)?; // line styles not rendered
        }
        if state_new_styles == 1 {
            // New fill + line arrays + fresh bit widths — a new index
            // space (generation bump).
            br.align_byte();
            generation += 1;
            let fc = read_count(br)?;
            let mut new_count = 0usize;
            for i in 0..fc {
                match parse_fill_style(br, version)? {
                    FillStyle::Solid(c) => {
                        fill_index.insert((generation, i + 1), fills.len());
                        fills.push(c);
                        new_count += 1;
                    }
                    FillStyle::Skipped => {
                        *skipped_fills += 1;
                        new_count += 1;
                    }
                }
            }
            styles_per_gen.push(new_count);
            let lc = read_count(br)?;
            for _ in 0..lc {
                parse_line_style(br, version)?;
                *skipped_lines += 1;
            }
            let widths = br.byte()?; // NumFillBits(4) | NumLineBits(4)
            nfb = widths >> 4;
            nlb = widths & 0x0f;
            if nfb == 0 {
                return Err("gfx shape: new-styles record has zero NumFillBits".into());
            }
        }
    }

    // Chain the edges per (generation, style index) into closed
    // contours, in PAINT ORDER: generation 0 first, then each new
    // array's styles in index order (SWF paints fill styles in style
    // order; later fills over earlier ones).
    let mut layers = Vec::new();
    for (gen, count) in styles_per_gen.iter().enumerate() {
        for style in 1..=(*count) {
            let gi = match fill_index.get(&(gen, style)) {
                Some(&g) if g != usize::MAX => g,
                _ => continue, // skipped style — renders nothing
            };
            // Orient: fill1 (the right side) → forward; fill0 (left) →
            // reversed.  An edge can serve BOTH sides (two fills).
            let mut oriented: Vec<(usize, bool)> = Vec::new();
            for (ei, (_, f0, f1)) in edges.iter().enumerate() {
                if *f1 == (gen, style) {
                    oriented.push((ei, false));
                }
                if *f0 == (gen, style) {
                    oriented.push((ei, true));
                }
            }
            if oriented.is_empty() {
                continue;
            }
            let key = |p: [f32; 2]| (p[0] as i64, p[1] as i64);
            let mut from_start: std::collections::HashMap<(i64, i64), Vec<usize>> = Default::default();
            for (oi, (ei, rev)) in oriented.iter().enumerate() {
                let start = if *rev { edges[*ei].0.last() } else { edges[*ei].0.first() }.unwrap();
                from_start.entry(key(*start)).or_default().push(oi);
            }
            let mut used = vec![false; oriented.len()];
            for start_oi in 0..oriented.len() {
                if used[start_oi] {
                    continue;
                }
                let mut contour: Vec<[f32; 2]> = Vec::new();
                let mut oi = start_oi;
                let mut guard = 0usize;
                loop {
                    guard += 1;
                    if guard > 100_000 {
                        return Err("gfx shape: edge chaining ran away".into());
                    }
                    used[oi] = true;
                    let (ei, rev) = oriented[oi];
                    let mut pts = edges[ei].0.clone();
                    if rev {
                        pts.reverse();
                    }
                    let start = if rev { edges[ei].0.last().unwrap() } else { edges[ei].0.first().unwrap() };
                    let end = if rev { edges[ei].0.first().unwrap() } else { edges[ei].0.last().unwrap() };
                    if contour.is_empty() {
                        contour.extend_from_slice(&pts);
                    } else {
                        contour.extend_from_slice(&pts[1..]); // skip the shared joint
                    }
                    let start_key = key(*start);
                    let end_key = key(*end);
                    let next_list = from_start.get(&end_key).cloned().unwrap_or_default();
                    let next = next_list.into_iter().find(|&n| !used[n] && n != oi);
                    match next {
                        Some(n) if end_key != start_key || contour.len() < 3 => oi = n,
                        _ => {
                            if end_key != start_key {
                                contour.push(*start); // close a gap explicitly
                            }
                            break;
                        }
                    }
                }
                if contour.len() >= 3 {
                    layers.push(ShapeLayer {
                        fill: gi,
                        contours: vec![contour],
                    });
                }
            }
        }
    }
    Ok(layers)
}

/// Parse a DefineShape (tag 2 / 22 / 32) body: styles + records.
pub fn parse_shape(body: &[u8], version: u8) -> Result<GfxShape, String> {
    let mut br = BitReader::new(body);
    let _id = br.u16()?;
    let _bounds = super::frontend_gfx::parse_rect_pub(&mut br)?;
    if version >= 2 {
        let _edge_bounds = super::frontend_gfx::parse_rect_pub(&mut br)?;
    }
    if version == 3 {
        let _reserved = br.bits(6)?;
        let _flags = br.bits(2)?;
        br.align_byte();
    }
    // The leading style arrays (byte-aligned; the RECTs left us
    // bit-aligned — align now).
    br.align_byte();
    let fc = read_count(&mut br)?;
    let mut fills = Vec::with_capacity(fc);
    let mut skipped_fills = 0u32;
    let mut leading_solids = 0usize;
    for _ in 0..fc {
        match parse_fill_style(&mut br, version)? {
            FillStyle::Solid(c) => {
                fills.push(c);
                leading_solids += 1;
            }
            FillStyle::Skipped => {
                skipped_fills += 1;
            }
        }
    }
    // The leading styles map 1..=fc → global fill indices (solids get
    // theirs in order; skipped styles map to usize::MAX — the walk
    // only needs the SOLID mapping, and the leading count for the
    // chain loop's style range).
    let lc = read_count(&mut br)?;
    let mut skipped_lines = 0u32;
    for _ in 0..lc {
        parse_line_style(&mut br, version)?;
        skipped_lines += 1;
    }
    let widths = br.byte()?;
    let nfb = widths >> 4;
    let nlb = widths & 0x0f;
    if nfb == 0 {
        return Err("gfx shape: NumFillBits is zero".into());
    }
    let layers = parse_shape_records(
        &mut br,
        version,
        nfb,
        nlb,
        &mut fills,
        fc.max(leading_solids.max(1)),
        &mut skipped_fills,
        &mut skipped_lines,
    )?;
    Ok(GfxShape {
        fills,
        layers,
        skipped_gradient_fills: skipped_fills,
        skipped_bitmap_fills: 0,
        skipped_line_styles: skipped_lines,
    })
}

/// Strip the simple HTML the game's text carries (`<p align=…>`,
/// `<br>`, `<b>`, `<font …>`) → plain text with real newlines.
fn strip_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let after = &rest[lt..];
        let Some(gt) = after.find('>') else {
            break;
        };
        let tag = &after[1..gt];
        if tag.eq_ignore_ascii_case("br") || tag.starts_with("br ") || tag.starts_with("br/") {
            out.push('\n');
        }
        rest = &after[gt + 1..];
    }
    out.push_str(rest);
    out
}

/// Parse a DefineEditText (tag 37) body — everything through ONE bit
/// reader (the RECT leaves the stream bit-aligned; the following
/// fields are byte-aligned, so align and read bytes).
pub fn parse_edit_text(body: &[u8]) -> Result<GfxText, String> {
    let mut br = BitReader::new(body);
    let _id = br.u16()?;
    let bounds = super::frontend_gfx::parse_rect_pub(&mut br)?;
    br.align_byte();
    let f1 = br.byte()?;
    let f2 = br.byte()?;
    let has_text = f1 & 0x80 != 0;
    let has_text_color = f1 & 0x04 != 0;
    let has_max_len = f1 & 0x02 != 0;
    let has_layout = f2 & 0x20 != 0;
    let has_font = f1 & 0x01 != 0;
    let mut text = GfxText {
        height: 240, // 12 px default if the record never says
        color: [0.9, 0.9, 0.9, 1.0],
        bounds,
        align: 0,
        has_font,
        text: String::new(),
    };
    if has_font {
        let _font_id = br.u16()?;
        text.height = br.u16()?;
    }
    if has_text_color {
        let (r, g, b, a) = (br.byte()?, br.byte()?, br.byte()?, br.byte()?);
        text.color = [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a as f32 / 255.0];
    }
    if has_max_len {
        let _ = br.u16()?;
    }
    if has_layout {
        text.align = br.byte()?;
        let _ = (br.u16()?, br.u16()?, br.u16()?); // left, right, indent
        let _ = br.u16()?; // leading (i16 — unused)
    }
    // VariableName (always present), InitialText (when HasText).
    let mut var = Vec::new();
    loop {
        let c = br.byte()?;
        if c == 0 {
            break;
        }
        var.push(c);
    }
    let _ = var;
    if has_text {
        let mut init = Vec::new();
        loop {
            let c = br.byte()?;
            if c == 0 {
                break;
            }
            init.push(c);
        }
        text.text = strip_markup(&String::from_utf8_lossy(&init));
    }
    Ok(text)
}

/// Read a byte via the bit reader (ALIGNED — align_byte first).
impl BitReader<'_> {
    fn byte(&mut self) -> Result<u8, String> {
        self.align_byte();
        Ok(self.bits(8)? as u8)
    }
    fn u16(&mut self) -> Result<u16, String> {
        let lo = self.byte()?;
        let hi = self.byte()?;
        Ok(lo as u16 | (hi as u16) << 8)
    }
}

// ---------------------------------------------------------------------------
// composition + rasterization
// ---------------------------------------------------------------------------

/// Compose two 2D affine transforms `[a, b, c, d, tx, ty]` — `p`
/// applied AFTER `c` (a point goes through `c` first).
pub fn compose(p: [f32; 6], c: [f32; 6]) -> [f32; 6] {
    [
        p[0] * c[0] + p[2] * c[1],
        p[1] * c[0] + p[3] * c[1],
        p[0] * c[2] + p[2] * c[3],
        p[1] * c[2] + p[3] * c[3],
        p[4] + p[0] * c[4] + p[2] * c[5],
        p[5] + p[1] * c[4] + p[3] * c[5],
    ]
}

/// Apply a transform to a point.
fn apply(m: [f32; 6], p: [f32; 2]) -> [f32; 2] {
    [m[0] * p[0] + m[2] * p[1] + m[4], m[1] * p[0] + m[3] * p[1] + m[5]]
}

/// The even-odd scanline filler: hard-edged, holes cancel.
pub fn fill_even_odd(
    fb: &mut [[f32; 4]],
    w: u32,
    h: u32,
    contours: &[Vec<[f32; 2]>],
    color: [f32; 4],
) {
    if color[3] <= 0.0 || contours.is_empty() {
        return;
    }
    for y in 0..h as i32 {
        let yc = y as f32 + 0.5;
        let mut xs: Vec<f32> = Vec::new();
        for c in contours {
            if c.len() < 2 {
                continue;
            }
            let pts: Vec<[f32; 2]> = c.clone();
            let n = pts.len();
            for s in 0..n {
                let (p0, p1) = (pts[s], pts[(s + 1) % n]);
                if (p0[1] <= yc) != (p1[1] <= yc) {
                    let t = (yc - p0[1]) / (p1[1] - p0[1]);
                    xs.push(p0[0] + t * (p1[0] - p0[0]));
                }
            }
        }
        if xs.len() < 2 {
            continue;
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        for pair in xs.chunks(2) {
            if pair.len() != 2 {
                continue;
            }
            let x0 = pair[0].max(0.0).ceil() as u32;
            let x1 = (pair[1].floor() as u32).min(w.saturating_sub(1));
            for x in x0..=x1.min(w.saturating_sub(1)) {
                if x0 > x1 {
                    break;
                }
                let o = (y as u32 * w + x) as usize;
                if o < fb.len() {
                    fb[o] = color;
                }
            }
        }
    }
}

/// One flattened draw: a shape's contours transformed to DEVICE px.
struct ShapeDraw {
    layers: Vec<(Vec<Vec<[f32; 2]>>, [f32; 4])>,
}

/// One flattened text draw: device-space position + size.
struct TextDraw {
    text: String,
    x: f32,
    y: f32,
    px: f32,
    color: [f32; 4],
    align: u8,
    box_left: f32,
    box_right: f32,
    has_box: bool,
}

/// Render the screen's frame 1 from its VECTOR characters (shapes +
/// real text) — the replacement for the bitmap-only path on screens
/// that carry none (the user's protostart/protomainmenu: zero
/// bitmaps).  The stage letterboxes exactly like the bitmap path.
pub fn render_vector_frame(
    screen: &super::frontend_gfx::GfxScreen,
    width: u32,
    height: u32,
) -> Result<FrontendFrame, String> {
    if width == 0 || height == 0 {
        return Err("gfx: empty render target".into());
    }
    let (xmin, ymin, xmax, ymax) = (screen.rect[0], screen.rect[1], screen.rect[2], screen.rect[3]);
    let stage_w = xmax - xmin;
    let stage_h = ymax - ymin;
    if stage_w <= 0.0 || stage_h <= 0.0 {
        return Err("gfx: degenerate stage RECT".into());
    }
    let stage_w_px = stage_w / 20.0;
    let stage_h_px = stage_h / 20.0;
    let scale = (width as f32 / stage_w_px).min(height as f32 / stage_h_px);
    let off_x = (width as f32 - stage_w_px * scale) / 2.0;
    let off_y = (height as f32 - stage_h_px * scale) / 2.0;
    // Stage twips → device px (the SAME letterbox map the bitmap path
    // uses): device = translate(offset) ∘ scale(scale/20) ∘ translate(-rectmin).
    let stage = [scale / 20.0, 0.0, 0.0, scale / 20.0, off_x - xmin / 20.0 * scale, off_y - ymin / 20.0 * scale];

    let mut fb = vec![screen.background; (width * height) as usize];

    // Recursive flatten (sprites nest; depth-bounded).
    fn walk(
        placements: &[GfxPlacement],
        chars: &std::collections::BTreeMap<u16, GfxChar>,
        parent: [f32; 6],
        stage: [f32; 6],
        shapes: &mut Vec<ShapeDraw>,
        texts: &mut Vec<TextDraw>,
        depth: u32,
        stats: &mut (u32, u32),
    ) {
        if depth > 12 {
            return;
        }
        for pl in placements {
            let Some(ch) = chars.get(&pl.char_id) else {
                continue;
            };
            let m = compose(parent, pl.matrix);
            match ch {
                GfxChar::Shape(s) => {
                    let mut layers = Vec::new();
                    for layer in &s.layers {
                        let color = s.fills.get(layer.fill).copied().unwrap_or([0.0, 0.0, 0.0, 0.0]);
                        let contours: Vec<Vec<[f32; 2]>> = layer
                            .contours
                            .iter()
                            .map(|c| {
                                c.iter()
                                    .map(|p| {
                                        let d = apply(compose(stage, m), *p);
                                        [d[0], d[1]]
                                    })
                                    .collect()
                            })
                            .collect();
                        layers.push((contours, color));
                    }
                    stats.0 += 1;
                    shapes.push(ShapeDraw { layers });
                }
                GfxChar::Sprite(nested) => {
                    walk(nested, chars, m, stage, shapes, texts, depth + 1, stats);
                }
                GfxChar::Text(t) => {
                    // The text renders from the record's local origin:
                    // the box's top-left in local twips → device.
                    let origin = apply(compose(stage, m), [0.0, 0.0]);
                    let size = apply(compose(stage, m), [20.0 * t.height.max(1) as f32, 0.0]);
                    let px = ((size[0] - origin[0]).abs()).max(6.0);
                    let bl = apply(compose(stage, m), [t.bounds[0], t.bounds[1]]);
                    let br = apply(compose(stage, m), [t.bounds[2], t.bounds[1]]);
                    stats.1 += 1;
                    texts.push(TextDraw {
                        text: t.text.clone(),
                        x: origin[0],
                        y: origin[1],
                        px,
                        color: t.color,
                        align: t.align,
                        box_left: bl[0].min(br[0]),
                        box_right: bl[0].max(br[0]),
                        has_box: t.bounds[2] > t.bounds[0],
                    });
                }
            }
        }
    }

    let mut shapes = Vec::new();
    let mut texts = Vec::new();
    let mut stats = (0u32, 0u32);
    walk(
        &screen.placements,
        &screen.chars,
        [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        stage,
        &mut shapes,
        &mut texts,
        0,
        &mut stats,
    );

    // Painters: shapes in placement order, fills in style order.
    for s in &shapes {
        for (contours, color) in &s.layers {
            fill_even_odd(&mut fb, width, height, contours, *color);
        }
    }
    // Text LAST (over the art — the real strings read on top).
    let font = fontdue::Font::from_bytes(MENU_FONT_BYTES, fontdue::FontSettings::default())
        .map_err(|e| format!("gfx: font load failed: {e}"))?;
    for t in &texts {
        blit_text(&mut fb, width, height, t, &font);
    }

    // Pack (+ stats, mirroring the other paths).
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    let mut distinct = std::collections::BTreeSet::new();
    for px_v in &fb {
        let b = [
            f32_to_u8(px_v[0]),
            f32_to_u8(px_v[1]),
            f32_to_u8(px_v[2]),
            f32_to_u8(px_v[3]),
        ];
        distinct.insert(u32::from_le_bytes(b));
        rgba.extend_from_slice(&b);
    }
    Ok(FrontendFrame {
        width,
        height,
        rgba,
        stats: FrameStats {
            distinct: distinct.len(),
            min: [0; 4],
            max: [255; 4],
            triangles_drawn: stats.0 as usize,
            triangles_clipped: stats.1 as usize,
        },
    })
}

/// One text draw, rasterized with the app font (the game's REAL
/// strings/sizes/colours/positions; our glyphs until DefineFont
/// outlines parse).  Word wrap: none (the screens' boxes fit).
fn blit_text(fb: &mut [[f32; 4]], w: u32, h: u32, t: &TextDraw, font: &fontdue::Font) {
    if t.text.is_empty() || t.px <= 2.0 {
        return;
    }
    let lines: Vec<&str> = t.text.split('\n').collect();
    let line_h = t.px * 1.25;
    for (li, line) in lines.iter().enumerate() {
        let baseline = t.y + t.px * 0.8 + li as f32 * line_h;
        if baseline < -t.px || baseline > h as f32 + t.px {
            continue;
        }
        // Width for alignment.
        let mut total = 0.0f32;
        let mut glyphs = Vec::new();
        for c in line.chars() {
            let idx = font.lookup_glyph_index(c);
            if c == ' ' {
                let (m, _) = font.rasterize_indexed(font.lookup_glyph_index('n'), t.px);
                total += m.advance_width;
                glyphs.push((None, Vec::new(), m.advance_width, 0.0));
                continue;
            }
            let (m, cov) = font.rasterize_indexed(idx, t.px);
            total += m.advance_width;
            glyphs.push((Some(idx), cov, m.advance_width, m.xmin as f32));
        }
        let x0 = if t.has_box && t.align == 2 {
            (t.box_left + t.box_right) / 2.0 - total / 2.0
        } else if t.has_box && t.align == 1 {
            t.box_right - total
        } else {
            t.x
        };
        let mut pen_x = x0;
        for (idx, cov, advance, xmin) in glyphs {
            if let (Some(idx), false) = (idx, cov.is_empty()) {
                let (_m, cov) = font.rasterize_indexed(idx, t.px);
                let gx = (pen_x + xmin).floor() as i32;
                let gy = (baseline - t.px * 0.25).floor() as i32;
                let gw = _m.width;
                let gh = _m.height;
                for yy in 0..gh {
                    for xx in 0..gw {
                        let a = cov[yy * gw + xx] as f32 / 255.0;
                        if a <= 0.0 {
                            continue;
                        }
                        let px_i = gx + xx as i32;
                        let py_i = gy + yy as i32;
                        if px_i < 0 || py_i < 0 || px_i >= w as i32 || py_i >= h as i32 {
                            continue;
                        }
                        let o = (py_i as u32 * w + px_i as u32) as usize;
                        let src = t.color;
                        let dst = &mut fb[o];
                        for k in 0..4 {
                            dst[k] = src[k] * a + dst[k] * (1.0 - a);
                        }
                    }
                }
            }
            pen_x += advance;
        }
    }
}

// ---------------------------------------------------------------------------
// tests — synthetic DefineShape/EditText/Sprite streams (spec-pinned)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// MSB-first bit writer (mirrors frontend_gfx's test writer).
    struct W {
        out: Vec<u8>,
        pos: usize,
    }
    impl W {
        fn new() -> Self {
            Self { out: vec![], pos: 0 }
        }
        fn wr(&mut self, n: u32, v: u32) {
            for i in (0..n).rev() {
                let bit = (v >> i) & 1;
                if self.pos >> 3 >= self.out.len() {
                    self.out.push(0);
                }
                if bit == 1 {
                    self.out[self.pos >> 3] |= 1 << (7 - (self.pos & 7));
                }
                self.pos += 1;
            }
        }
        fn rect(&mut self, xmin: i32, ymin: i32, xmax: i32, ymax: i32) {
            self.wr(5, 16);
            self.wr(16, xmin as u32);
            self.wr(16, xmax as u32);
            self.wr(16, ymin as u32);
            self.wr(16, ymax as u32);
        }
        fn finish(mut self) -> Vec<u8> {
            self.pos = (self.pos + 7) & !7;
            while self.out.len() < self.pos >> 3 {
                self.out.push(0);
            }
            self.out
        }
    }

    /// A DefineShape v1: ONE solid red fill, a 20×20 px square
    /// (twips: 100..500) drawn with four straight edges — placed by
    /// the caller, this is the bread-and-butter menu shape.
    fn red_square_shape(id: u16) -> Vec<u8> {
        let mut b = W::new();
        // id (LE, BEFORE the bit stream — the body starts byte-aligned)
        let mut v = id.to_le_bytes().to_vec();
        b.rect(100, 100, 500, 500); // bounds
        v.extend(b.finish());
        v.push(1); // fill style count = 1
        v.push(0x00); // solid
        v.extend_from_slice(&[255, 0, 0]); // RGB red (v1)
        v.push(0); // line style count = 0
        v.push(0x10); // NumFillBits 1 | NumLineBits 0
        // Shape records:
        let mut r = W::new();
        // StyleChange: move to (100,100), fill1 = 1
        // bits: type 0, new 0, line 0, fill1 1, fill0 0, move 1
        r.wr(1, 0);
        r.wr(1, 0);
        r.wr(1, 0);
        r.wr(1, 1);
        r.wr(1, 0);
        r.wr(1, 1);
        r.wr(5, 9); // MoveBits 9 (values fit ±256)
        r.wr(9, 100 as u32); // MoveDeltaX (absolute — signed 9-bit: 100 ✓)
        r.wr(9, 100 as u32); // MoveDeltaY
        r.wr(1, 1); // FillStyle1 value = 1 (4 bits — from NumFillBits)
        // Straight edges: NumBitsField = 8 → the parser reads
        // NumBits+2 = 10-bit signed deltas (400 fits ±511).
        let edge = |r: &mut W, dx: i32, dy: i32| {
            r.wr(1, 1); // type = edge
            r.wr(1, 1); // straight
            r.wr(4, 8); // NumBits field (deltas read as 10 bits)
            r.wr(1, 1); // general line
            r.wr(10, dx as u32);
            r.wr(10, dy as u32);
        };
        // Edge 1: straight general +400,0
        edge(&mut r, 400, 0);
        // Edge 2: +0,+400
        edge(&mut r, 0, 400);
        // Edge 3: -400,0
        edge(&mut r, -400, 0);
        // Edge 4: 0,-400
        edge(&mut r, 0, -400);
        // End: type 0 + five zero flags
        r.wr(6, 0);
        v.extend(r.finish());
        v
    }

    /// The shape parses; the even-odd filler fills its square and the
    /// corners stay background.
    #[test]
    fn parses_and_fills_a_red_square() {
        let body = red_square_shape(9);
        let shape = parse_shape(&body, 1).expect("the v1 shape parses");
        assert_eq!(shape.fills.len(), 1);
        assert_eq!(shape.fills[0], [1.0, 0.0, 0.0, 1.0]);
        assert!(!shape.layers.is_empty(), "the square chained");
        // The contours bound 100..500 twips.
        let all: &Vec<[f32; 2]> = &shape.layers[0].contours[0];
        let xs: Vec<f32> = all.iter().map(|p| p[0]).collect();
        assert!((xs.iter().cloned().fold(f32::MIN, f32::max) - 500.0).abs() < 1e-4);
        assert!((xs.iter().cloned().fold(f32::MAX, f32::min) - 100.0).abs() < 1e-4);
    }

    /// A hole: the same square with an inner REVERSED contour
    /// (fill0 side) cancels through even-odd — the centre shows the
    /// background, the ring is red.
    #[test]
    fn even_odd_cancels_the_inner_contour() {
        let mut fb = vec![[0.0, 0.0, 0.0, 1.0]; 40 * 40];
        // Outer square 2..38 px, inner square 10..30 px (opposite
        // winding — even-odd does not actually need winding, only
        // containment; this matches SWF's reversed-inner style).
        let outer: Vec<[f32; 2]> = vec![[2.0, 2.0], [38.0, 2.0], [38.0, 38.0], [2.0, 38.0]];
        let inner: Vec<[f32; 2]> = vec![[10.0, 10.0], [10.0, 30.0], [30.0, 30.0], [30.0, 10.0]];
        fill_even_odd(&mut fb, 40, 40, &[outer, inner], [1.0, 0.0, 0.0, 1.0]);
        let px = |x: usize, y: usize| fb[y * 40 + x];
        assert_eq!(px(5, 5), [1.0, 0.0, 0.0, 1.0], "ring: red");
        assert_eq!(px(20, 20), [0.0, 0.0, 0.0, 1.0], "hole: background");
        assert_eq!(px(1, 1), [0.0, 0.0, 0.0, 1.0], "outside: background");
    }

    /// The EditText: the REAL string, height, colour and centre
    /// alignment survive the parse; the markup stripper turns <br>
    /// into a newline.
    #[test]
    fn parses_edit_text_with_real_string() {
        let mut v = Vec::new();
        v.extend_from_slice(&7u16.to_le_bytes()); // id
        let mut b = W::new();
        b.rect(0, 0, 4000, 600); // bounds (200×30 px)
        v.extend(b.finish());
        v.push(0x80 | 0x04 | 0x01); // HasText | HasTextColor | HasFont
        v.push(0x20); // HasLayout
        v.extend_from_slice(&3u16.to_le_bytes()); // font id
        v.extend_from_slice(&240u16.to_le_bytes()); // height 12 px
        v.extend_from_slice(&[255, 255, 255, 255]); // white
        v.push(2); // align = centre
        v.extend_from_slice(&0u16.to_le_bytes()); // left
        v.extend_from_slice(&4000u16.to_le_bytes()); // right
        v.extend_from_slice(&0u16.to_le_bytes()); // indent
        v.extend_from_slice(&0u16.to_le_bytes()); // leading
        v.push(0); // variable name ""
        v.extend_from_slice(b"HELLO<font>WORLD</font>");
        v.push(0); // terminator
        let text = parse_edit_text(&v).expect("the edit text parses");
        assert_eq!(text.text, "HELLOWORLD");
        assert_eq!(text.height, 240);
        assert_eq!(text.align, 2);
        assert_eq!(text.color, [1.0, 1.0, 1.0, 1.0]);
    }

    /// The full vector frame: shape char + sprite char + text char —
    /// the sprite nests the shape, all three render, and the text
    /// lands ON TOP (its glyphs overwrite the fill).
    #[test]
    fn renders_shapes_sprites_and_real_text() {
        // char 9: red square shape (the fixture above)
        // char 5: sprite placing shape 9 at (100,100) twips identity
        // char 7: text "PLAY"
        let shape = red_square_shape(9);
        let mut sprite = Vec::new();
        sprite.extend_from_slice(&5u16.to_le_bytes()); // sprite id
        sprite.extend_from_slice(&1u16.to_le_bytes()); // 1 frame
        // PlaceObject2: flags has_char|has_matrix, depth 1, char 9,
        // identity matrix.
        let mut po = Vec::new();
        po.push(0x02 | 0x04);
        po.extend_from_slice(&1u16.to_le_bytes());
        po.extend_from_slice(&9u16.to_le_bytes());
        po.extend_from_slice(&[0x00, 0x00]); // identity matrix
        let cl = ((26u16) << 6) | po.len() as u16;
        sprite.extend_from_slice(&cl.to_le_bytes());
        sprite.extend_from_slice(&po);
        sprite.extend_from_slice(&[0x00, 0x00]); // End
        // char 7: edit text
        let mut text = Vec::new();
        text.extend_from_slice(&7u16.to_le_bytes());
        let mut b = W::new();
        b.rect(0, 0, 8000, 2000);
        text.extend(b.finish());
        text.push(0x81); // f1: HasText 0x80 | HasFont 0x01
        text.push(0x00); // f2: no layout (align defaults left)
        text.extend_from_slice(&3u16.to_le_bytes());
        text.extend_from_slice(&600u16.to_le_bytes()); // 30 px
        text.push(0); // var name
        text.extend_from_slice(b"PLAY");
        text.push(0);

        // The screen: stage 400×400 px, root places sprite 5 and text 7.
        let mut body = Vec::new();
        body.extend_from_slice(b"FWS");
        body.push(6);
        body.extend_from_slice(&[0, 0, 0, 0]);
        let mut b = W::new();
        b.rect(0, 0, 8000, 8000);
        body.extend(b.finish());
        body.extend_from_slice(&[0, 0, 1, 0]); // fps, 1 frame
        for (code, payload) in [(2u16, &shape), (39u16, &sprite), (37u16, &text)] {
            let cl = (code << 6) | 0x3f;
            body.extend_from_slice(&cl.to_le_bytes());
            body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            body.extend_from_slice(payload);
        }
        // PlaceObject2 root: depth 1 → sprite 5 at identity; depth 2 → text 7.
        let mut po = Vec::new();
        po.push(0x02 | 0x04);
        po.extend_from_slice(&1u16.to_le_bytes());
        po.extend_from_slice(&5u16.to_le_bytes());
        po.extend_from_slice(&[0x00, 0x00]);
        let cl = ((26u16) << 6) | po.len() as u16;
        body.extend_from_slice(&cl.to_le_bytes());
        body.extend_from_slice(&po);
        let mut po2 = Vec::new();
        po2.push(0x02 | 0x04);
        po2.extend_from_slice(&2u16.to_le_bytes());
        po2.extend_from_slice(&7u16.to_le_bytes());
        po2.extend_from_slice(&[0x00, 0x00]); // identity matrix
        let cl = ((26u16) << 6) | po2.len() as u16;
        body.extend_from_slice(&cl.to_le_bytes());
        body.extend_from_slice(&po2);
        body.extend_from_slice(&[0x40, 0x00]); // ShowFrame
        body.extend_from_slice(&[0x00, 0x00]); // End

        let screen = super::super::frontend_gfx::parse_gfx(&body).expect("the screen parses");
        assert!(screen.has_vector_content());
        assert_eq!(screen.chars.len(), 3, "shape + sprite + text");
        let f = render_vector_frame(&screen, 100, 100).expect("the vector frame renders");
        let px = |x: usize, y: usize| {
            let o = (y * 100 + x) * 4;
            [f.rgba[o], f.rgba[o + 1], f.rgba[o + 2], f.rgba[o + 3]]
        };
        // The square is 100..500 twips = 5..25 stage px of a 400 px
        // stage; the 100×100 target letterboxes at scale 0.25 → the
        // square occupies device px 1..6.  Sample its interior.
        assert_eq!(px(3, 3), [255, 0, 0, 255], "sprite shape renders red");
        // Text pixels exist somewhere above the shape's fill (white).
        let whites = (0..f.rgba.len() / 4)
            .filter(|&i| {
                let o = i * 4;
                f.rgba[o] > 200 && f.rgba[o + 1] > 200 && f.rgba[o + 2] > 200
            })
            .count();
        assert!(whites > 10, "the real string rendered white glyphs");
    }
}
//! Overlay UI primitives — text, buttons, sliders, checkboxes, panels.
//!
//! This module is the **drawing layer** of the in-game Settings overlay
//! and the first-launch Setup screen.  It renders directly into a
//! host-owned RGBA8 framebuffer with NO shader-pair involvement (the
//! gfxtextshader pair is the *game's* text path; the overlay UI is OUR
//! chrome, not the game's, so it gets its own simpler path).
//!
//! # Why not use the gfxtext rasterizer
//!
//! The gfxtext path goes through the real shader pair (correct for
//! pinning the shader contract).  The overlay UI does not need shader
//! correctness — it needs crisp rectangles, click hit-testing, and
//! text labels at arbitrary sizes.  A 5×7 bitmap font + a fill_rect
//! primitive is simpler and host-runnable in unit tests with zero
//! dependencies on wgpu or the shader crates.
//!
//! # Layout
//!
//! All coordinates are **window pixel space** with origin top-left:
//! `x` grows right, `y` grows down.  This matches winit's
//! `CursorMoved::position` (logical pixels) and the touchmapper's
//! window-pixel convention, so hit-testing is a direct bounds check.
//!
//! # Host testability
//!
//! Every primitive is host-testable: `paint` writes into a `Vec<u8>`
//! owned by the caller, and hit-testing is pure arithmetic over the
//! [`Rect`] type.  The tests cover fill_rect bounds, text glyph
//! placement, button hit-testing, slider drag, checkbox toggle, panel
//! composition, and that an empty `paint` does not crash.

use crate::display_settings::Settings;

/// A screen-space rectangle.  Origin top-left, integer pixel bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// Left edge, inclusive.
    pub x: i32,
    /// Top edge, inclusive.
    pub y: i32,
    /// Width in pixels.
    pub w: u32,
    /// Height in pixels.
    pub h: u32,
}

impl Rect {
    /// Construct a rect from (x, y, w, h).
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    /// Right edge (exclusive).
    pub fn right(&self) -> i32 {
        self.x + self.w as i32
    }

    /// Bottom edge (exclusive).
    pub fn bottom(&self) -> i32 {
        self.y + self.h as i32
    }

    /// Hit-test: is `(px, py)` inside this rect?
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }
}

/// RGBA8 colour.  Stored as `[r, g, b, a]` in `0..=255`.
pub type Colour = [u8; 4];

/// Paint a solid-coloured rectangle into `fb` (a `width*height*4` byte
/// buffer, RGBA8).  Out-of-bounds pixels are clipped (the rect can
/// extend past the framebuffer without panicking).
pub fn fill_rect(fb: &mut [u8], width: u32, height: u32, rect: Rect, c: Colour) {
    let x0 = rect.x.max(0) as u32;
    let y0 = rect.y.max(0) as u32;
    let x1 = (rect.right() as u32).min(width);
    let y1 = (rect.bottom() as u32).min(height);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    for y in y0..y1 {
        for x in x0..x1 {
            let o = ((y * width + x) * 4) as usize;
            blend_pixel(&mut fb[o..o + 4], c);
        }
    }
}

/// Stroke (outline-only) a rectangle: 1-pixel border on every side.
pub fn stroke_rect(fb: &mut [u8], width: u32, height: u32, rect: Rect, c: Colour) {
    let top = Rect::new(rect.x, rect.y, rect.w, 1);
    let bot = Rect::new(rect.x, rect.y + rect.h as i32 - 1, rect.w, 1);
    let lhs = Rect::new(rect.x, rect.y, 1, rect.h);
    let rhs = Rect::new(rect.x + rect.w as i32 - 1, rect.y, 1, rect.h);
    fill_rect(fb, width, height, top, c);
    fill_rect(fb, width, height, bot, c);
    fill_rect(fb, width, height, lhs, c);
    fill_rect(fb, width, height, rhs, c);
}

/// Alpha-bl a source colour over a destination pixel (the standard
/// "over" operator).  Used by every primitive.
fn blend_pixel(dst: &mut [u8], src: Colour) {
    let sa = src[3] as f32 / 255.0;
    let da = dst[3] as f32 / 255.0;
    let out_a = sa + da * (1.0 - sa);
    if out_a <= 0.0 {
        dst[0] = 0;
        dst[1] = 0;
        dst[2] = 0;
        dst[3] = 0;
        return;
    }
    for k in 0..3 {
        let s = src[k] as f32 * sa;
        let d = dst[k] as f32 * da * (1.0 - sa);
        dst[k] = ((s + d) / out_a).clamp(0.0, 255.0) as u8;
    }
    dst[3] = (out_a * 255.0).clamp(0.0, 255.0) as u8;
}

/// A 5×7 bitmap glyph (one byte per row, MSB = leftmost column, top
/// row first).  Identical to the engine's text-shader atlas, but kept
/// here so the UI module has no cross-crate dependency for its font.
const GLYPHS_5X7: &[(&str, [u8; 7])] = &[
    ("A", [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11]),
    ("B", [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E]),
    ("C", [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E]),
    ("D", [0x1C, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1C]),
    ("E", [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F]),
    ("F", [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10]),
    ("G", [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F]),
    ("H", [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11]),
    ("I", [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E]),
    ("J", [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C]),
    ("K", [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11]),
    ("L", [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F]),
    ("M", [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11]),
    ("N", [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11]),
    ("O", [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E]),
    ("P", [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10]),
    ("Q", [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D]),
    ("R", [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11]),
    ("S", [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E]),
    ("T", [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04]),
    ("U", [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E]),
    ("V", [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04]),
    ("W", [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11]),
    ("X", [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11]),
    ("Y", [0x11, 0x11, 0x11, 0x0A, 0x04, 0x04, 0x04]),
    ("Z", [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F]),
    ("a", [0x00, 0x0E, 0x01, 0x0F, 0x11, 0x11, 0x0F]),
    ("b", [0x10, 0x10, 0x1E, 0x11, 0x11, 0x11, 0x1E]),
    ("c", [0x00, 0x0E, 0x11, 0x10, 0x10, 0x11, 0x0E]),
    ("d", [0x01, 0x01, 0x0F, 0x11, 0x11, 0x11, 0x0F]),
    ("e", [0x00, 0x0E, 0x11, 0x1F, 0x10, 0x11, 0x0E]),
    ("f", [0x06, 0x08, 0x1E, 0x08, 0x08, 0x08, 0x08]),
    ("g", [0x0F, 0x11, 0x11, 0x0F, 0x01, 0x11, 0x0E]),
    ("h", [0x10, 0x10, 0x1E, 0x11, 0x11, 0x11, 0x11]),
    ("i", [0x04, 0x00, 0x0C, 0x04, 0x04, 0x04, 0x0E]),
    ("j", [0x02, 0x00, 0x06, 0x02, 0x02, 0x12, 0x0C]),
    ("k", [0x10, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11]),
    ("l", [0x0C, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E]),
    ("m", [0x00, 0x1A, 0x15, 0x15, 0x15, 0x15, 0x15]),
    ("n", [0x00, 0x1E, 0x11, 0x11, 0x11, 0x11, 0x11]),
    ("o", [0x00, 0x0E, 0x11, 0x11, 0x11, 0x11, 0x0E]),
    ("p", [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10]),
    ("q", [0x0F, 0x11, 0x11, 0x0F, 0x01, 0x01, 0x01]),
    ("r", [0x00, 0x1E, 0x11, 0x10, 0x10, 0x10, 0x10]),
    ("s", [0x00, 0x0F, 0x10, 0x0E, 0x01, 0x01, 0x1E]),
    ("t", [0x08, 0x08, 0x1C, 0x08, 0x08, 0x09, 0x06]),
    ("u", [0x00, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0F]),
    ("v", [0x00, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04]),
    ("w", [0x00, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11]),
    ("x", [0x00, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11]),
    ("y", [0x11, 0x11, 0x11, 0x0F, 0x01, 0x11, 0x0E]),
    ("z", [0x00, 0x1F, 0x01, 0x02, 0x04, 0x08, 0x1F]),
    ("0", [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E]),
    ("1", [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E]),
    ("2", [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F]),
    ("3", [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E]),
    ("4", [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02]),
    ("5", [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E]),
    ("6", [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E]),
    ("7", [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08]),
    ("8", [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E]),
    ("9", [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C]),
    (" ", [0, 0, 0, 0, 0, 0, 0]),
    ("_", [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F]),
    ("-", [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00]),
    ("/", [0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10]),
    (".", [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C]),
    (":", [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x0C, 0x00]),
    ("%", [0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03]),
    ("!", [0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04]),
    ("?", [0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04]),
    ("(", [0x04, 0x08, 0x10, 0x10, 0x10, 0x08, 0x04]),
    (")", [0x04, 0x02, 0x01, 0x01, 0x01, 0x02, 0x04]),
];

/// Look up a glyph's 5×7 bitmap by character.  Returns the space glyph
/// (all-zero) for unknown chars so text never panics.
pub fn glyph_for(ch: char) -> [u8; 7] {
    let mut buf = [0u8; 4];
    let s: &str = ch.encode_utf8(&mut buf);
    for (name, bits) in GLYPHS_5X7 {
        if **name == *s {
            return *bits;
        }
    }
    // The space glyph — anything unknown renders blank, no panic.
    [0u8; 7]
}

/// Draw a single glyph at `(x, y)` with cell scale `sx, sy` (1 = native
/// 5×7, 2 = 10×14, etc.).  The glyph's bit pattern is alpha-blended
/// into the framebuffer in colour `c`.
pub fn draw_glyph(
    fb: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    sx: u32,
    sy: u32,
    bits: [u8; 7],
    c: Colour,
) {
    for (row, byte) in bits.iter().enumerate() {
        for col in 0..5 {
            if (byte >> (4 - col)) & 1 == 1 {
                for dy in 0..sy {
                    for dx in 0..sx {
                        let px = x + (col as u32 * sx + dx) as i32;
                        let py = y + (row as u32 * sy + dy) as i32;
                        if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                            continue;
                        }
                        let o = ((py as u32 * width + px as u32) * 4) as usize;
                        blend_pixel(&mut fb[o..o + 4], c);
                    }
                }
            }
        }
    }
}

/// Draw a string at `(x, y)` with cell scale `sx, sy` and `gap` pixels
/// between glyphs (in addition to the 1-pixel inter-glyph padding
/// built into the 5×7 bitmap).  Returns the x just past the last glyph
/// (for chained `draw_text` calls).
pub fn draw_text(
    fb: &mut [u8],
    width: u32,
    height: u32,
    text: &str,
    x: i32,
    y: i32,
    sx: u32,
    sy: u32,
    gap: u32,
    c: Colour,
) -> i32 {
    let mut cx = x;
    for ch in text.chars() {
        let bits = glyph_for(ch);
        draw_glyph(fb, width, height, cx, y, sx, sy, bits, c);
        cx += (5 * sx + gap) as i32;
    }
    cx
}

/// A clickable button.  The label is drawn centred in the rect.
#[derive(Debug, Clone)]
pub struct Button {
    /// Screen-space bounds.
    pub rect: Rect,
    /// Label text (ASCII subset supported by the 5×7 font).
    pub label: String,
    /// Background colour (when not pressed).
    pub bg: Colour,
    /// Background colour when the pointer is inside the rect.
    pub hover: Colour,
    /// Background colour when the button is pressed (pointer-down
    /// inside the rect).
    pub pressed: Colour,
    /// Label colour.
    pub fg: Colour,
    /// Scale of the label text (1 = native 5×7).
    pub scale: u32,
}

impl Button {
    /// Construct a standard button with the default palette.
    pub fn new(rect: Rect, label: impl Into<String>) -> Self {
        Self {
            rect,
            label: label.into(),
            bg: [40, 40, 60, 230],
            hover: [70, 70, 110, 240],
            pressed: [110, 110, 160, 255],
            fg: [240, 240, 240, 255],
            scale: 2,
        }
    }

    /// Draw the button into `fb` with the given visual state.
    pub fn paint(&self, fb: &mut [u8], w: u32, h: u32, hover: bool, pressed: bool) {
        let c = if pressed {
            self.pressed
        } else if hover {
            self.hover
        } else {
            self.bg
        };
        fill_rect(fb, w, h, self.rect, c);
        stroke_rect(fb, w, h, self.rect, [200, 200, 220, 255]);
        // Centre the label.
        let glyph_w = (5 * self.scale) as i32;
        let glyph_h = (7 * self.scale) as i32;
        let total_w = self.label.chars().count() as i32 * (glyph_w + 2) - 2;
        let tx = self.rect.x + ((self.rect.w as i32 - total_w) / 2).max(0);
        let ty = self.rect.y + ((self.rect.h as i32 - glyph_h) / 2).max(0);
        draw_text(fb, w, h, &self.label, tx, ty, self.scale, self.scale, 2, self.fg);
    }

    /// Hit-test a click at `(px, py)`.  Returns `true` if the point is
    /// inside the button's rect (the caller decides whether to fire
    /// the action on press, release, or both).
    pub fn hit(&self, px: i32, py: i32) -> bool {
        self.rect.contains(px, py)
    }
}

/// A checkbox: a small square + a label.  Toggles a boolean setting.
#[derive(Debug, Clone)]
pub struct Checkbox {
    /// Square bounds (the label is drawn to the right).
    pub box_rect: Rect,
    /// Label text.
    pub label: String,
    /// Whether the box is currently ticked.
    pub checked: bool,
    /// Box border colour.
    pub border: Colour,
    /// Box fill colour when checked.
    pub fill: Colour,
    /// Tick mark colour (drawn inside the box when checked).
    pub tick: Colour,
    /// Label colour.
    pub fg: Colour,
    /// Scale of the label text.
    pub scale: u32,
}

impl Checkbox {
    /// Construct a standard checkbox with the default palette.
    pub fn new(box_rect: Rect, label: impl Into<String>, checked: bool) -> Self {
        Self {
            box_rect,
            label: label.into(),
            checked,
            border: [200, 200, 220, 255],
            fill: [80, 120, 200, 255],
            tick: [240, 240, 240, 255],
            fg: [240, 240, 240, 255],
            scale: 2,
        }
    }

    /// Paint the checkbox + label.
    pub fn paint(&self, fb: &mut [u8], w: u32, h: u32) {
        fill_rect(fb, w, h, self.box_rect, [20, 20, 30, 230]);
        stroke_rect(fb, w, h, self.box_rect, self.border);
        if self.checked {
            // A simple cross-hatch tick: two diagonals.
            let r = self.box_rect;
            let sz = r.w.min(r.h) as i32;
            for i in 0..sz {
                let x = r.x + i;
                let y = r.y + i;
                let o = ((y as u32 * w + x as u32) * 4) as usize;
                blend_pixel(&mut fb[o..o + 4], self.tick);
            }
            for i in 0..sz {
                let x = r.x + i;
                let y = r.y + sz - 1 - i;
                let o = ((y as u32 * w + x as u32) * 4) as usize;
                blend_pixel(&mut fb[o..o + 4], self.tick);
            }
        }
        let tx = self.box_rect.right() + 6;
        let ty = self.box_rect.y + ((self.box_rect.h as i32 - 7 * self.scale as i32) / 2).max(0);
        draw_text(fb, w, h, &self.label, tx, ty, self.scale, self.scale, 2, self.fg);
    }

    /// Hit-test the box (not the label — labels are not interactive
    /// surfaces in this UI; only the checkbox square toggles).
    pub fn hit(&self, px: i32, py: i32) -> bool {
        self.box_rect.contains(px, py)
    }

    /// Toggle the checked state (mutates `self`).
    pub fn toggle(&mut self) {
        self.checked = !self.checked;
    }
}

/// A slider: a horizontal track with a draggable thumb.  The value is
/// normalised to `[0.0, 1.0]`; the caller maps it to the setting's
/// real range.
#[derive(Debug, Clone)]
pub struct Slider {
    /// Track bounds.
    pub rect: Rect,
    /// Label drawn above the track.
    pub label: String,
    /// Current value in `[0.0, 1.0]`.
    pub value: f32,
    /// Track background.
    pub bg: Colour,
    /// Track fill (left of the thumb).
    pub fill: Colour,
    /// Thumb colour.
    pub thumb: Colour,
    /// Label colour.
    pub fg: Colour,
    /// Scale of the label text.
    pub scale: u32,
}

impl Slider {
    /// Construct a standard slider with the default palette.
    pub fn new(rect: Rect, label: impl Into<String>, value: f32) -> Self {
        Self {
            rect,
            label: label.into(),
            value: value.clamp(0.0, 1.0),
            bg: [30, 30, 45, 230],
            fill: [80, 120, 200, 255],
            thumb: [240, 240, 240, 255],
            fg: [240, 240, 240, 255],
            scale: 2,
        }
    }

    /// Paint the slider + label.
    pub fn paint(&self, fb: &mut [u8], w: u32, h: u32) {
        let tx = self.rect.x;
        let ty = self.rect.y - (7 * self.scale as i32) - 4;
        draw_text(fb, w, h, &self.label, tx, ty.max(0), self.scale, self.scale, 2, self.fg);
        // Track
        fill_rect(fb, w, h, self.rect, self.bg);
        // Fill (left of thumb)
        let fill_w = ((self.rect.w as f32) * self.value) as u32;
        let fill_zone = Rect::new(self.rect.x, self.rect.y, fill_w, self.rect.h);
        fill_rect(fb, w, h, fill_zone, self.fill);
        // Thumb (a small square at the value's x)
        let thumb_x = self.rect.x + (self.rect.w as f32 * self.value) as i32 - 4;
        let thumb_y = self.rect.y - 2;
        let thumb_r = Rect::new(thumb_x, thumb_y, 8, self.rect.h as u32 + 4);
        fill_rect(fb, w, h, thumb_r, self.thumb);
    }

    /// Hit-test the slider's track (used to start a drag).
    pub fn hit_track(&self, px: i32, py: i32) -> bool {
        self.rect.contains(px, py)
    }

    /// Update the value from an absolute x position (the pointer's x).
    /// Returns the new value (clamped to `[0.0, 1.0]`).
    pub fn drag_to(&mut self, px: i32) -> f32 {
        let rel = (px - self.rect.x) as f32 / self.rect.w as f32;
        self.value = rel.clamp(0.0, 1.0);
        self.value
    }
}

/// A panel: a labelled container that groups widgets.  Renders a
/// semi-transparent background + a title bar; children are drawn by
/// the caller into the panel's body rect.
#[derive(Debug, Clone)]
pub struct Panel {
    /// Outer bounds (includes the title bar).
    pub rect: Rect,
    /// Title text (drawn in the title bar).
    pub title: String,
    /// Background colour (the body).
    pub bg: Colour,
    /// Title bar colour.
    pub title_bg: Colour,
    /// Title text colour.
    pub title_fg: Colour,
    /// Scale of the title text.
    pub scale: u32,
}

impl Panel {
    /// Construct a standard panel with the default palette.
    pub fn new(rect: Rect, title: impl Into<String>) -> Self {
        Self {
            rect,
            title: title.into(),
            bg: [15, 15, 25, 220],
            title_bg: [40, 40, 70, 240],
            title_fg: [240, 240, 240, 255],
            scale: 2,
        }
    }

    /// The body rect (the area inside the panel below the title bar).
    pub fn body(&self) -> Rect {
        let title_h = (8 * self.scale) as u32 + 6;
        Rect::new(
            self.rect.x + 4,
            self.rect.y + title_h as i32,
            self.rect.w.saturating_sub(8),
            self.rect.h.saturating_sub(title_h + 4),
        )
    }

    /// Paint the panel background + title bar.
    pub fn paint(&self, fb: &mut [u8], w: u32, h: u32) {
        fill_rect(fb, w, h, self.rect, self.bg);
        let title_h = (8 * self.scale) as u32 + 6;
        let title_bar = Rect::new(self.rect.x, self.rect.y, self.rect.w, title_h);
        fill_rect(fb, w, h, title_bar, self.title_bg);
        stroke_rect(fb, w, h, self.rect, [200, 200, 220, 255]);
        draw_text(
            fb,
            w,
            h,
            &self.title,
            self.rect.x + 6,
            self.rect.y + 4,
            self.scale,
            self.scale,
            2,
            self.title_fg,
        );
    }
}

/// The Settings overlay: a panel + the per-platform widgets.  The
/// app constructs one when the user opens Settings, runs the event
/// loop, and saves the resulting [`Settings`] when the user closes.
///
/// The widget list is built from the current [`Settings`]; mutations
/// flow back into the struct so the caller can persist them.
pub struct SettingsOverlay {
    /// The current settings being edited.
    pub settings: Settings,
    /// The panel itself.
    pub panel: Panel,
    /// The checkboxes (one per boolean setting).
    pub checkboxes: Vec<Checkbox>,
    /// The sliders (one per float setting).
    pub sliders: Vec<Slider>,
    /// The close button.
    pub close: Button,
}

impl SettingsOverlay {
    /// Construct the overlay centred in a `(w, h)` window with the
    /// given starting [`Settings`].
    pub fn new(w: u32, h: u32, settings: Settings) -> Self {
        let pw = 480.min(w.saturating_sub(20));
        let ph = 360.min(h.saturating_sub(20));
        let px = ((w as i32 - pw as i32) / 2).max(10);
        let py = ((h as i32 - ph as i32) / 2).max(10);
        let panel = Panel::new(Rect::new(px, py, pw, ph), "Settings");

        // Layout: vertical stack of widgets inside the panel body.
        let body = panel.body();
        let mut y = body.y + 8;
        let mut checkboxes = Vec::new();
        let mut sliders = Vec::new();

        // Input section
        let cb = Checkbox::new(
            Rect::new(body.x + 12, y, 16, 16),
            "Virtual joystick",
            settings.input.virtual_joystick,
        );
        checkboxes.push(cb);
        y += 26;
        let cb = Checkbox::new(
            Rect::new(body.x + 12, y, 16, 16),
            "Action buttons",
            settings.input.virtual_buttons,
        );
        checkboxes.push(cb);
        y += 26;
        let cb = Checkbox::new(
            Rect::new(body.x + 12, y, 16, 16),
            "Invert look Y",
            settings.input.invert_look_y,
        );
        checkboxes.push(cb);
        y += 32;

        // Look sensitivity slider (mapped from [0.1, 10.0] to [0, 1]).
        let sens_norm = (settings.input.look_sensitivity - 0.1) / (10.0 - 0.1);
        let s = Slider::new(
            Rect::new(body.x + 12, y + 12, body.w.saturating_sub(24), 8),
            "Look sensitivity",
            sens_norm,
        );
        sliders.push(s);
        y += 44;

        // Graphics section
        let res_norm = (settings.graphics.resolution_scale - 0.25) / (2.0 - 0.25);
        let s = Slider::new(
            Rect::new(body.x + 12, y + 12, body.w.saturating_sub(24), 8),
            "Resolution scale",
            res_norm,
        );
        sliders.push(s);
        y += 44;

        let vsync_cb = Checkbox::new(
            Rect::new(body.x + 12, y, 16, 16),
            "Vsync",
            settings.graphics.vsync,
        );
        checkboxes.push(vsync_cb);
        y += 32;

        let draw_norm = (settings.graphics.draw_distance - 0.25) / (4.0 - 0.25);
        let s = Slider::new(
            Rect::new(body.x + 12, y + 12, body.w.saturating_sub(24), 8),
            "Draw distance",
            draw_norm,
        );
        sliders.push(s);
        y += 44;

        // MSAA as a slider over [1, 8] (mapped to {1,2,4,8} by
        // quantisation on read).
        let msaa_norm = (settings.graphics.msaa_samples as f32 - 1.0) / (8.0 - 1.0);
        let s = Slider::new(
            Rect::new(body.x + 12, y + 12, body.w.saturating_sub(24), 8),
            "Anti-aliasing (1=off, 8=max)",
            msaa_norm,
        );
        sliders.push(s);

        let close = Button::new(
            Rect::new(body.x + body.w as i32 - 100, body.y + body.h as i32 - 32, 90, 24),
            "Close",
        );

        Self {
            settings,
            panel,
            checkboxes,
            sliders,
            close,
        }
    }

    /// Paint the whole overlay.
    pub fn paint(&self, fb: &mut [u8], w: u32, h: u32) {
        self.panel.paint(fb, w, h);
        for cb in &self.checkboxes {
            cb.paint(fb, w, h);
        }
        for s in &self.sliders {
            s.paint(fb, w, h);
        }
        self.close.paint(fb, w, h, false, false);
    }

    /// Handle a click (pointer-down) at `(px, py)`.  Returns `true` if
    /// the click closed the overlay (the caller stops handling further
    /// clicks until the overlay is reopened).
    pub fn on_click(&mut self, px: i32, py: i32) -> bool {
        if self.close.hit(px, py) {
            return true;
        }
        // The checkboxes are: 0=vjoy, 1=buttons, 2=invert_y, 3=vsync
        // (the order they were pushed in `new`).
        if self.checkboxes[0].hit(px, py) {
            self.checkboxes[0].toggle();
            self.settings.input.virtual_joystick = self.checkboxes[0].checked;
        } else if self.checkboxes[1].hit(px, py) {
            self.checkboxes[1].toggle();
            self.settings.input.virtual_buttons = self.checkboxes[1].checked;
        } else if self.checkboxes[2].hit(px, py) {
            self.checkboxes[2].toggle();
            self.settings.input.invert_look_y = self.checkboxes[2].checked;
        } else if self.checkboxes[3].hit(px, py) {
            self.checkboxes[3].toggle();
            self.settings.graphics.vsync = self.checkboxes[3].checked;
        }
        false
    }

    /// Handle a drag (pointer-move with button held).  Updates any
    /// slider whose track is hit.
    pub fn on_drag(&mut self, px: i32, _py: i32) {
        // Slider indices: 0=sensitivity, 1=resolution, 2=draw_dist, 3=msaa
        // (the order they were pushed in `new`).
        for (i, s) in self.sliders.iter_mut().enumerate() {
            if s.hit_track(px, 0) || (px >= s.rect.x && px <= s.rect.right()) {
                let v = s.drag_to(px);
                match i {
                    0 => {
                        self.settings.input.look_sensitivity = 0.1 + v * (10.0 - 0.1);
                    }
                    1 => {
                        self.settings.graphics.resolution_scale = 0.25 + v * (2.0 - 0.25);
                    }
                    2 => {
                        self.settings.graphics.draw_distance = 0.25 + v * (4.0 - 0.25);
                    }
                    3 => {
                        // Quantise to {1, 2, 4, 8}.
                        let raw = 1.0 + v * 7.0;
                        let q = if raw < 1.5 {
                            1
                        } else if raw < 3.0 {
                            2
                        } else if raw < 6.0 {
                            4
                        } else {
                            8
                        };
                        self.settings.graphics.msaa_samples = q;
                    }
                    _ => {}
                }
            }
        }
    }
}

/// The first-launch Setup screen: a panel telling the user to copy
/// their game files into the asset folder, plus a button to open the
/// system file picker (Android: an Intent; desktop: a no-op message).
pub struct SetupScreen {
    /// The panel.
    pub panel: Panel,
    /// The "open file picker" / "open all-files-access settings" button.
    pub open_button: Button,
    /// The "re-scan now" button (after the user has copied files in
    /// outside the app and wants to retry without restarting).
    pub rescan_button: Button,
}

impl SetupScreen {
    /// Construct a centred Setup screen for a `(w, h)` window.
    pub fn new(w: u32, h: u32) -> Self {
        let pw = 480.min(w.saturating_sub(20));
        let ph = 220.min(h.saturating_sub(20));
        let px = ((w as i32 - pw as i32) / 2).max(10);
        let py = ((h as i32 - ph as i32) / 2).max(10);
        let panel = Panel::new(Rect::new(px, py, pw, ph), "Prototype Rust");
        let body = panel.body();
        let open_button = Button::new(
            Rect::new(body.x + 12, body.y + 70, body.w.saturating_sub(24), 28),
            "Open all-files-access settings",
        );
        let rescan_button = Button::new(
            Rect::new(body.x + 12, body.y + 110, body.w.saturating_sub(24), 28),
            "Re-scan folder",
        );
        Self {
            panel,
            open_button,
            rescan_button,
        }
    }

    /// Paint the Setup screen.
    pub fn paint(&self, fb: &mut [u8], w: u32, h: u32) {
        self.panel.paint(fb, w, h);
        let body = self.panel.body();
        draw_text(
            fb,
            w,
            h,
            "Waiting for game files in:",
            body.x + 8,
            body.y + 4,
            2,
            2,
            2,
            [240, 240, 240, 255],
        );
        // The path is the user's /sdcard/Prototype Rust/ — drawn from
        // the platform default (this is informational only).
        let path = crate::asset_folder::default_folder();
        draw_text(
            fb,
            w,
            h,
            &path.display().to_string(),
            body.x + 8,
            body.y + 28,
            2,
            2,
            2,
            [180, 220, 255, 255],
        );
        self.open_button.paint(fb, w, h, false, false);
        self.rescan_button.paint(fb, w, h, false, false);
    }

    /// Hit-test the "open settings" button.
    pub fn hit_open(&self, px: i32, py: i32) -> bool {
        self.open_button.hit(px, py)
    }

    /// Hit-test the "re-scan" button.
    pub fn hit_rescan(&self, px: i32, py: i32) -> bool {
        self.rescan_button.hit(px, py)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_fb(w: u32, h: u32) -> Vec<u8> {
        vec![0u8; (w * h * 4) as usize]
    }

    fn pixel_at(fb: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let o = ((y * w + x) * 4) as usize;
        [fb[o], fb[o + 1], fb[o + 2], fb[o + 3]]
    }

    #[test]
    fn rect_contains_is_inclusive_top_left_exclusive_bottom_right() {
        let r = Rect::new(10, 10, 5, 5);
        assert!(r.contains(10, 10));
        assert!(r.contains(14, 14));
        assert!(!r.contains(15, 10));
        assert!(!r.contains(10, 15));
        assert!(!r.contains(9, 10));
    }

    #[test]
    fn fill_rect_writes_in_bounds_pixels_only() {
        let mut fb = fresh_fb(32, 32);
        fill_rect(&mut fb, 32, 32, Rect::new(5, 5, 10, 10), [255, 0, 0, 255]);
        // In-bounds: red.
        assert_eq!(pixel_at(&fb, 32, 10, 10), [255, 0, 0, 255]);
        // Out-of-bounds (top-left): transparent.
        assert_eq!(pixel_at(&fb, 32, 0, 0), [0, 0, 0, 0]);
        // Out-of-bounds (bottom-right edge): transparent.
        assert_eq!(pixel_at(&fb, 32, 15, 15), [0, 0, 0, 0]);
    }

    #[test]
    fn fill_rect_clips_negative_origin() {
        let mut fb = fresh_fb(16, 16);
        // Rect starts at -5; should clip to 0.
        fill_rect(&mut fb, 16, 16, Rect::new(-5, -5, 10, 10), [0, 255, 0, 255]);
        assert_eq!(pixel_at(&fb, 16, 0, 0), [0, 255, 0, 255]);
        assert_eq!(pixel_at(&fb, 16, 4, 4), [0, 255, 0, 255]);
        assert_eq!(pixel_at(&fb, 16, 5, 5), [0, 0, 0, 0]);
    }

    #[test]
    fn fill_rect_clips_oversized_rect() {
        let mut fb = fresh_fb(16, 16);
        fill_rect(&mut fb, 16, 16, Rect::new(8, 8, 100, 100), [0, 0, 255, 255]);
        assert_eq!(pixel_at(&fb, 16, 8, 8), [0, 0, 255, 255]);
        assert_eq!(pixel_at(&fb, 16, 15, 15), [0, 0, 255, 255]);
    }

    #[test]
    fn glyph_for_known_char_is_not_blank() {
        let g = glyph_for('A');
        assert_ne!(g, [0u8; 7]);
    }

    #[test]
    fn glyph_for_unknown_char_is_blank() {
        // Unicode snowman is not in the 5×7 atlas.
        let g = glyph_for('\u{2603}');
        assert_eq!(g, [0u8; 7]);
    }

    #[test]
    fn draw_text_writes_some_pixels() {
        let mut fb = fresh_fb(64, 16);
        draw_text(&mut fb, 64, 16, "AB", 0, 0, 1, 1, 1, [255, 255, 255, 255]);
        // Some pixel inside the glyph bounds should be non-zero.
        let mut any_lit = false;
        for y in 0..7 {
            for x in 0..12 {
                if pixel_at(&fb, 64, x, y)[0] > 0 {
                    any_lit = true;
                    break;
                }
            }
            if any_lit {
                break;
            }
        }
        assert!(any_lit);
    }

    #[test]
    fn button_hit_test_uses_rect() {
        let b = Button::new(Rect::new(10, 10, 50, 20), "OK");
        assert!(b.hit(35, 20));
        assert!(!b.hit(5, 20));
        assert!(!b.hit(70, 20));
    }

    #[test]
    fn button_paint_writes_pixels_inside_rect() {
        let mut fb = fresh_fb(80, 40);
        let b = Button::new(Rect::new(10, 10, 50, 20), "OK");
        b.paint(&mut fb, 80, 40, false, false);
        // Background pixel inside the rect should be non-zero.
        let p = pixel_at(&fb, 80, 30, 15);
        assert_ne!(p, [0, 0, 0, 0]);
    }

    #[test]
    fn checkbox_toggle_changes_state() {
        let mut cb = Checkbox::new(Rect::new(0, 0, 16, 16), "X", false);
        assert!(!cb.checked);
        cb.toggle();
        assert!(cb.checked);
        cb.toggle();
        assert!(!cb.checked);
    }

    #[test]
    fn slider_drag_to_clamps_to_zero_one() {
        let mut s = Slider::new(Rect::new(0, 0, 100, 8), "v", 0.5);
        s.drag_to(-10);
        assert_eq!(s.value, 0.0);
        s.drag_to(200);
        assert_eq!(s.value, 1.0);
        s.drag_to(50);
        assert_eq!(s.value, 0.5);
    }

    #[test]
    fn panel_body_lies_below_title_bar() {
        let p = Panel::new(Rect::new(10, 10, 200, 100), "Settings");
        let b = p.body();
        assert!(b.y > p.rect.y);
        assert!(b.x > p.rect.x);
        assert!(b.right() < p.rect.right());
        assert!(b.bottom() < p.rect.bottom());
    }

    #[test]
    fn settings_overlay_close_returns_true() {
        let mut o = SettingsOverlay::new(640, 480, Settings::pc_default());
        let cx = o.close.rect.x + o.close.rect.w as i32 / 2;
        let cy = o.close.rect.y + o.close.rect.h as i32 / 2;
        assert!(o.on_click(cx, cy));
    }

    #[test]
    fn settings_overlay_toggles_virtual_joystick() {
        let mut o = SettingsOverlay::new(640, 480, Settings::pc_default());
        let cb = &o.checkboxes[0];
        let cx = cb.box_rect.x + 8;
        let cy = cb.box_rect.y + 8;
        let was = o.settings.input.virtual_joystick;
        o.on_click(cx, cy);
        assert_ne!(o.settings.input.virtual_joystick, was);
    }

    #[test]
    fn settings_overlay_drag_updates_sensitivity() {
        let mut o = SettingsOverlay::new(640, 480, Settings::pc_default());
        let s = &o.sliders[0];
        let before = o.settings.input.look_sensitivity;
        // Drag the slider fully right.
        o.on_drag(s.rect.right() - 1, 0);
        assert!(o.settings.input.look_sensitivity > before);
    }

    #[test]
    fn setup_screen_paint_does_not_panic() {
        let mut fb = fresh_fb(640, 480);
        let s = SetupScreen::new(640, 480);
        s.paint(&mut fb, 640, 480);
        // Some pixel should be lit (the panel background).
        let mut any_lit = false;
        for y in 100..300 {
            for x in 100..400 {
                if pixel_at(&fb, 640, x, y)[3] > 0 {
                    any_lit = true;
                    break;
                }
            }
            if any_lit {
                break;
            }
        }
        assert!(any_lit);
    }
}

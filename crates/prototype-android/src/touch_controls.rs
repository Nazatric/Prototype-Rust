//! On-screen touch controls — virtual joystick + action buttons.
//!
//! The user directive: "more necessary settings like touch controls"
//! in-game.  This module is the Android gameplay input layer — the
//! PC build keeps the mouse (Prototype shipped PC-only).
//!
//! # Layout (DESIGN — ours, matching common mobile-shooter convention)
//!
//! - **Left thumb zone** (bottom-left quarter of the screen): a virtual
//!   movement joystick.  Touch-down anchors the joystick base; drag
//!   produces a normalised `(dx, dy)` in `[-1, 1]²` for the engine's
//!   movement input.
//! - **Right thumb zone** (bottom-right quarter): a cluster of four
//!   virtual action buttons (Attack, Jump, Grab, Dash), arranged in a
//!   diamond.  Each maps to the engine's `KeyCode` (the same code the
//!   PC keyboard uses).
//!
//! # Visibility
//!
//! The overlay is enabled by [`crate::display_settings::InputSettings::virtual_joystick`]
//! and [`crate::display_settings::InputSettings::virtual_buttons`] — toggled
//! from the in-game Settings menu.  When both are off the overlay
//! draws nothing and touch events pass through to the existing
//! single-pointer-as-mouse scheme in [`crate::touch`].
//!
//! # Hit-testing
//!
//! The joystick claims any touch that lands in the left zone; the
//! buttons claim any touch in the right zone.  When a touch lands on
//! a button, the button's `KeyCode` is fed into the engine's
//! [`prototype_input::InputState`] as a press (and a release when the
//! finger lifts).  The joystick's `(dx, dy)` is fed as a movement
//! vector (translated to WASD key presses by the engine layer).
//!
//! # Host testability
//!
//! Every behaviour is host-testable: the touch-state machine, the
//! hit-testing, the joystick-into-delta mapping, the button-into-key
//! mapping, and the paint path.  No Android/JVM code lives here.

use prototype_input::{InputEvent, KeyCode};

use crate::ui::{fill_rect, stroke_rect, draw_text, Colour, Rect};

/// The four action buttons on the right side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionButton {
    /// Primary attack (left-click on PC).
    Attack,
    /// Jump (Space on PC).
    Jump,
    /// Grab/consume (E on PC).
    Grab,
    /// Dash (Shift on PC — but we use Q here as the engine's KeyCode
    /// subset doesn't include Shift).
    Dash,
}

impl ActionButton {
    /// The PC keyboard key this button emulates.
    pub fn key_code(self) -> KeyCode {
        match self {
            ActionButton::Attack => KeyCode::F,
            ActionButton::Jump => KeyCode::Space,
            ActionButton::Grab => KeyCode::E,
            ActionButton::Dash => KeyCode::Q,
        }
    }

    /// The single-character label drawn on the button.
    pub fn label(self) -> &'static str {
        match self {
            ActionButton::Attack => "ATK",
            ActionButton::Jump => "JMP",
            ActionButton::Grab => "GRB",
            ActionButton::Dash => "DSH",
        }
    }
}

/// The on-screen touch overlay.  Built from the window's `(w, h)` and
/// the user's [`crate::display_settings::InputSettings`].
pub struct TouchControls {
    /// Window pixel width (used for layout).
    pub width: u32,
    /// Window pixel height.
    pub height: u32,
    /// Whether the joystick is visible (left zone).
    pub joystick_enabled: bool,
    /// Whether the action buttons are visible (right zone).
    pub buttons_enabled: bool,

    /// The current joystick base (anchor) position in pixels, or
    /// `None` when no finger is on the joystick.  The base follows the
    /// initial touch position (a "floating" joystick — better for
    /// different hand sizes than a fixed one).
    pub joystick_base: Option<(i32, i32)>,
    /// The finger id currently owning the joystick (winit pointer id).
    pub joystick_finger: Option<u64>,
    /// The current joystick displacement `(dx, dy)` in `[-1, 1]²`.
    pub joystick_value: (f32, f32),

    /// The current button-press state: which finger owns which button.
    pub button_fingers: [(Option<u64>, bool); 4],
}

impl TouchControls {
    /// Construct an overlay for a `(w, h)` window with the given
    /// input settings.
    pub fn new(width: u32, height: u32, input: &crate::display_settings::InputSettings) -> Self {
        Self {
            width,
            height,
            joystick_enabled: input.virtual_joystick,
            buttons_enabled: input.virtual_buttons,
            joystick_base: None,
            joystick_finger: None,
            joystick_value: (0.0, 0.0),
            button_fingers: [(None, false); 4],
        }
    }

    /// The joystick activation zone: the bottom-left quarter of the
    /// window, inset by 20 px from each edge.
    pub fn joystick_zone(&self) -> Rect {
        let half_w = (self.width / 2).saturating_sub(20);
        let quarter_h = (self.height / 2).saturating_sub(20);
        Rect::new(20, (self.height as i32 - quarter_h as i32 - 20) as i32, half_w, quarter_h)
    }

    /// The action-button zone: the bottom-right quarter, inset.
    pub fn button_zone(&self) -> Rect {
        let half_w = (self.width / 2).saturating_sub(20);
        let quarter_h = (self.height / 2).saturating_sub(20);
        Rect::new(
            (self.width as i32 - half_w as i32 - 20) as i32,
            (self.height as i32 - quarter_h as i32 - 20) as i32,
            half_w,
            quarter_h,
        )
    }

    /// The screen rect of one action button.  The four buttons are
    /// laid out in a 2×2 grid inside the button zone.
    pub fn button_rect(&self, btn: ActionButton) -> Rect {
        let zone = self.button_zone();
        let bw = 70u32.min(zone.w / 2);
        let bh = 70u32.min(zone.h / 2);
        let gap = 6;
        let right_col_x = zone.x + zone.w as i32 - bw as i32 - gap;
        let left_col_x = zone.x + gap;
        let top_row_y = zone.y + gap;
        let bot_row_y = zone.y + zone.h as i32 - bh as i32 - gap;
        let (x, y) = match btn {
            ActionButton::Attack => (right_col_x, top_row_y + (bh as i32) / 2 - 4), // top-right-ish (Attack prominent)
            ActionButton::Jump => (right_col_x, bot_row_y),                         // bottom-right
            ActionButton::Grab => (left_col_x, bot_row_y),                          // bottom-left (interior)
            ActionButton::Dash => (left_col_x, top_row_y),                          // top-left
        };
        Rect::new(x, y, bw, bh)
    }

    /// Handle a touch-down at `(px, py)` with finger id `id`.
    /// Returns the input events to feed into the engine (zero or one
    /// per call; joystick motion is sampled on `move`, not here).
    pub fn on_touch_down(&mut self, id: u64, px: i32, py: i32) -> Vec<InputEvent> {
        let mut events = Vec::new();
        if self.joystick_enabled {
            let jz = self.joystick_zone();
            if jz.contains(px, py) && self.joystick_finger.is_none() {
                self.joystick_base = Some((px, py));
                self.joystick_finger = Some(id);
                self.joystick_value = (0.0, 0.0);
                return events; // no key event from joystick-down
            }
        }
        if self.buttons_enabled {
            for (i, btn) in [
                ActionButton::Attack,
                ActionButton::Jump,
                ActionButton::Grab,
                ActionButton::Dash,
            ]
            .iter()
            .enumerate()
            {
                if self.button_fingers[i].0.is_some() {
                    continue; // button already owned
                }
                let r = self.button_rect(*btn);
                if r.contains(px, py) {
                    self.button_fingers[i] = (Some(id), true);
                    events.push(InputEvent::KeyDown(btn.key_code()));
                    break;
                }
            }
        }
        events
    }

    /// Handle a touch-move for finger `id` at `(px, py)`.
    pub fn on_touch_move(&mut self, id: u64, px: i32, py: i32) {
        if self.joystick_finger == Some(id) {
            if let Some((bx, by)) = self.joystick_base {
                let dx = (px - bx) as f32 / 60.0; // 60 px = full deflection
                let dy = (py - by) as f32 / 60.0;
                self.joystick_value = (dx.clamp(-1.0, 1.0), (-dy).clamp(-1.0, 1.0));
                // Y inverted: dragging UP (negative py-bx) gives +Y
                // (forward movement), matching the engine convention.
            }
        }
    }

    /// Handle a touch-up for finger `id` at `(px, py)`.
    /// Returns the input events to feed (key-up for any button that
    /// was held by this finger).
    pub fn on_touch_up(&mut self, id: u64, _px: i32, _py: i32) -> Vec<InputEvent> {
        let mut events = Vec::new();
        if self.joystick_finger == Some(id) {
            self.joystick_finger = None;
            self.joystick_base = None;
            self.joystick_value = (0.0, 0.0);
        }
        for (i, btn) in [
            ActionButton::Attack,
            ActionButton::Jump,
            ActionButton::Grab,
            ActionButton::Dash,
        ]
        .iter()
        .enumerate()
        {
            if self.button_fingers[i].0 == Some(id) {
                self.button_fingers[i] = (None, false);
                events.push(InputEvent::KeyUp(btn.key_code()));
            }
        }
        events
    }

    /// Handle a touch-cancel (system cancelled the touch).
    pub fn on_touch_cancel(&mut self, id: u64) -> Vec<InputEvent> {
        self.on_touch_up(id, 0, 0)
    }

    /// The current joystick value as a normalised `(dx, dy)` pair in
    /// `[-1, 1]²`.  The engine reads this each frame to drive movement
    /// (WASD-equivalent).
    pub fn joystick(&self) -> (f32, f32) {
        self.joystick_value
    }

    /// Paint the overlay into `fb` (an RGBA8 buffer of size
    /// `width*height*4`).
    pub fn paint(&self, fb: &mut [u8]) {
        if !self.joystick_enabled && !self.buttons_enabled {
            return;
        }
        let w = self.width;
        let h = self.height;
        if self.joystick_enabled {
            // The joystick base circle: an outlined 120-px circle at
            // the current base (or the centre of the zone if no base).
            let jz = self.joystick_zone();
            let (cx, cy) = self
                .joystick_base
                .unwrap_or((jz.x + jz.w as i32 / 2, jz.y + jz.h as i32 / 2));
            paint_circle_outline(fb, w, h, cx, cy, 60, [200, 200, 220, 100]);
            // The stick (a filled small circle at the displaced
            // position).
            let stick_x = cx + (self.joystick_value.0 * 60.0) as i32;
            let stick_y = cy - (self.joystick_value.1 * 60.0) as i32;
            paint_circle_filled(fb, w, h, stick_x, stick_y, 20, [240, 240, 240, 180]);
        }
        if self.buttons_enabled {
            for (i, btn) in [
                ActionButton::Attack,
                ActionButton::Jump,
                ActionButton::Grab,
                ActionButton::Dash,
            ]
            .iter()
            .enumerate()
            {
                let r = self.button_rect(*btn);
                let pressed = self.button_fingers[i].1;
                let bg = if pressed {
                    [120, 120, 160, 220]
                } else {
                    [40, 40, 60, 180]
                };
                fill_rect(fb, w, h, r, bg);
                stroke_rect(fb, w, h, r, [200, 200, 220, 200]);
                let label = btn.label();
                let glyph_w = 5 * 2;
                let total_w = (label.len() as i32) * (glyph_w + 2) - 2;
                let tx = r.x + ((r.w as i32 - total_w) / 2).max(0);
                let ty = r.y + ((r.h as i32 - 7 * 2) / 2).max(0);
                draw_text(fb, w, h, label, tx, ty, 2, 2, 2, [240, 240, 240, 255]);
            }
        }
    }
}

/// Paint an outlined circle (Bresenham-style, integer arithmetic).
fn paint_circle_outline(fb: &mut [u8], w: u32, h: u32, cx: i32, cy: i32, r: i32, c: Colour) {
    let mut x = r;
    let mut y = 0;
    let mut err = 0;
    while x >= y {
        plot_8(fb, w, h, cx, cy, x, y, c);
        y += 1;
        if err <= 0 {
            err += 2 * y + 1;
        } else {
            x -= 1;
            err += 2 * (y - x) + 1;
        }
    }
}

/// Paint a filled circle (scan-line fill, integer arithmetic).
fn paint_circle_filled(fb: &mut [u8], w: u32, h: u32, cx: i32, cy: i32, r: i32, c: Colour) {
    let r2 = r * r;
    for y in -r..=r {
        for x in -r..=r {
            if x * x + y * y <= r2 {
                let px = cx + x;
                let py = cy + y;
                if px >= 0 && py >= 0 && px < w as i32 && py < h as i32 {
                    let o = ((py as u32 * w + px as u32) * 4) as usize;
                    let mut px_buf = [0u8; 4];
                    px_buf.copy_from_slice(&fb[o..o + 4]);
                    let mut dst = px_buf;
                    blend_over(&mut dst, c);
                    fb[o..o + 4].copy_from_slice(&dst);
                }
            }
        }
    }
}

/// Plot the 8 octant points of a circle (Bresenham).
fn plot_8(fb: &mut [u8], w: u32, h: u32, cx: i32, cy: i32, x: i32, y: i32, c: Colour) {
    for &(dx, dy) in &[
        (x, y),
        (y, x),
        (-x, y),
        (-y, x),
        (x, -y),
        (y, -x),
        (-x, -y),
        (-y, -x),
    ] {
        let px = cx + dx;
        let py = cy + dy;
        if px >= 0 && py >= 0 && px < w as i32 && py < h as i32 {
            let o = ((py as u32 * w + px as u32) * 4) as usize;
            let mut buf = [0u8; 4];
            buf.copy_from_slice(&fb[o..o + 4]);
            let mut dst = buf;
            blend_over(&mut dst, c);
            fb[o..o + 4].copy_from_slice(&dst);
        }
    }
}

/// Alpha-over composite (the same operator as the ui module).
fn blend_over(dst: &mut [u8; 4], src: Colour) {
    let sa = src[3] as f32 / 255.0;
    let da = dst[3] as f32 / 255.0;
    let out_a = sa + da * (1.0 - sa);
    if out_a <= 0.0 {
        *dst = [0, 0, 0, 0];
        return;
    }
    for k in 0..3 {
        let s = src[k] as f32 * sa;
        let d = dst[k] as f32 * da * (1.0 - sa);
        dst[k] = ((s + d) / out_a).clamp(0.0, 255.0) as u8;
    }
    dst[3] = (out_a * 255.0).clamp(0.0, 255.0) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_with_joystick(on: bool, buttons: bool) -> crate::display_settings::InputSettings {
        crate::display_settings::InputSettings {
            virtual_joystick: on,
            virtual_buttons: buttons,
            look_sensitivity: 1.0,
            invert_look_y: false,
        }
    }

    #[test]
    fn joystick_zone_is_bottom_left_quarter() {
        let t = TouchControls::new(800, 600, &input_with_joystick(true, true));
        let jz = t.joystick_zone();
        assert!(jz.x < 400, "joystick zone is on the LEFT half");
        assert!(jz.y >= 300, "joystick zone is in the BOTTOM half");
        assert!(jz.right() <= 400);
        assert!(jz.bottom() <= 600);
    }

    #[test]
    fn button_zone_is_bottom_right_quarter() {
        let t = TouchControls::new(800, 600, &input_with_joystick(true, true));
        let bz = t.button_zone();
        assert!(bz.x >= 400, "button zone is on the RIGHT half");
        assert!(bz.y >= 300, "button zone is in the BOTTOM half");
    }

    #[test]
    fn touch_down_in_joystick_zone_anchors_joystick() {
        let mut t = TouchControls::new(800, 600, &input_with_joystick(true, false));
        let jz = t.joystick_zone();
        let px = jz.x + jz.w as i32 / 2;
        let py = jz.y + jz.h as i32 / 2;
        let ev = t.on_touch_down(7, px, py);
        assert!(ev.is_empty(), "joystick-down emits no key event");
        assert_eq!(t.joystick_finger, Some(7));
        assert_eq!(t.joystick_base, Some((px, py)));
    }

    #[test]
    fn touch_move_drives_joystick_value() {
        let mut t = TouchControls::new(800, 600, &input_with_joystick(true, false));
        let jz = t.joystick_zone();
        let bx = jz.x + 50;
        let by = jz.y + 50;
        t.on_touch_down(7, bx, by);
        t.on_touch_move(7, bx + 60, by); // full right deflection
        let (dx, dy) = t.joystick();
        assert!((dx - 1.0).abs() < 0.01);
        assert!(dy.abs() < 0.01);
    }

    #[test]
    fn touch_move_up_gives_positive_dy() {
        let mut t = TouchControls::new(800, 600, &input_with_joystick(true, false));
        let jz = t.joystick_zone();
        let bx = jz.x + 50;
        let by = jz.y + 50;
        t.on_touch_down(7, bx, by);
        t.on_touch_move(7, bx, by - 60); // full up deflection
        let (_, dy) = t.joystick();
        assert!((dy - 1.0).abs() < 0.01, "dragging up gives +Y (forward)");
    }

    #[test]
    fn touch_move_clamps_to_unit_circle() {
        let mut t = TouchControls::new(800, 600, &input_with_joystick(true, false));
        let jz = t.joystick_zone();
        let bx = jz.x + 50;
        let by = jz.y + 50;
        t.on_touch_down(7, bx, by);
        t.on_touch_move(7, bx + 200, by + 200);
        let (dx, dy) = t.joystick();
        assert!(dx <= 1.0);
        assert!(dy <= 1.0);
    }

    #[test]
    fn touch_up_releases_joystick() {
        let mut t = TouchControls::new(800, 600, &input_with_joystick(true, false));
        let jz = t.joystick_zone();
        t.on_touch_down(7, jz.x + 50, jz.y + 50);
        t.on_touch_move(7, jz.x + 110, jz.y + 50);
        assert_ne!(t.joystick(), (0.0, 0.0));
        t.on_touch_up(7, 0, 0);
        assert_eq!(t.joystick(), (0.0, 0.0));
        assert_eq!(t.joystick_finger, None);
    }

    #[test]
    fn second_finger_in_joystick_zone_is_ignored() {
        let mut t = TouchControls::new(800, 600, &input_with_joystick(true, false));
        let jz = t.joystick_zone();
        t.on_touch_down(7, jz.x + 50, jz.y + 50);
        let ev = t.on_touch_down(8, jz.x + 60, jz.y + 60);
        // Already owned by finger 7 — finger 8 should be ignored.
        assert!(ev.is_empty());
        assert_eq!(t.joystick_finger, Some(7));
    }

    #[test]
    fn touch_down_on_attack_button_emits_key_down() {
        let mut t = TouchControls::new(800, 600, &input_with_joystick(false, true));
        let r = t.button_rect(ActionButton::Attack);
        let ev = t.on_touch_down(7, r.x + 10, r.y + 10);
        assert_eq!(ev.len(), 1);
        assert!(matches!(
            ev[0],
            InputEvent::KeyDown(KeyCode::F)
        ));
    }

    #[test]
    fn touch_up_on_held_button_emits_key_up() {
        let mut t = TouchControls::new(800, 600, &input_with_joystick(false, true));
        let r = t.button_rect(ActionButton::Jump);
        t.on_touch_down(7, r.x + 10, r.y + 10);
        let ev = t.on_touch_up(7, 0, 0);
        assert_eq!(ev.len(), 1);
        assert!(matches!(
            ev[0],
            InputEvent::KeyUp(KeyCode::Space)
        ));
    }

    #[test]
    fn touch_cancel_releases_held_button() {
        let mut t = TouchControls::new(800, 600, &input_with_joystick(false, true));
        let r = t.button_rect(ActionButton::Grab);
        t.on_touch_down(7, r.x + 10, r.y + 10);
        let ev = t.on_touch_cancel(7);
        assert_eq!(ev.len(), 1);
        assert!(matches!(
            ev[0],
            InputEvent::KeyUp(KeyCode::E)
        ));
    }

    #[test]
    fn second_finger_on_same_button_is_ignored() {
        let mut t = TouchControls::new(800, 600, &input_with_joystick(false, true));
        let r = t.button_rect(ActionButton::Attack);
        t.on_touch_down(7, r.x + 10, r.y + 10);
        let ev = t.on_touch_down(8, r.x + 10, r.y + 10);
        assert!(ev.is_empty(), "already owned; finger 8 ignored");
    }

    #[test]
    fn button_hit_outside_button_zone_does_nothing_when_buttons_off() {
        let mut t = TouchControls::new(800, 600, &input_with_joystick(false, false));
        // Buttons disabled — touch anywhere produces nothing.
        let ev = t.on_touch_down(7, 600, 500);
        assert!(ev.is_empty());
    }

    #[test]
    fn paint_does_not_panic_when_disabled() {
        let mut fb = vec![0u8; 800 * 600 * 4];
        let t = TouchControls::new(800, 600, &input_with_joystick(false, false));
        t.paint(&mut fb);
        // Frame should be all-zero (nothing drawn).
        assert!(fb.iter().all(|&b| b == 0));
    }

    #[test]
    fn paint_writes_some_pixels_when_enabled() {
        let mut fb = vec![0u8; 800 * 600 * 4];
        let t = TouchControls::new(800, 600, &input_with_joystick(true, true));
        t.paint(&mut fb);
        let any_lit = fb.chunks_exact(4).any(|px| px[3] > 0);
        assert!(any_lit);
    }

    #[test]
    fn action_button_key_codes_match_pc_keyboard() {
        // The four action buttons map to keys the engine's KeyCode
        // enum already knows (F, Space, E, Q) — the PC port uses the
        // same keys.
        assert_eq!(ActionButton::Attack.key_code(), KeyCode::F);
        assert_eq!(ActionButton::Jump.key_code(), KeyCode::Space);
        assert_eq!(ActionButton::Grab.key_code(), KeyCode::E);
        assert_eq!(ActionButton::Dash.key_code(), KeyCode::Q);
    }
}

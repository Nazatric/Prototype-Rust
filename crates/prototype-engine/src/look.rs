//! look — the camera LOOK control (block 38, NEXT-UP item 9d "touch
//! look control for the player camera").
//!
//! # What this closes
//!
//! Prototype (2009) shipped PC-only: there is no original touch-look
//! behaviour to match (the same DESIGN class as
//! `prototype_android::touch::TouchMapper`).  Until this module the
//! camera was a pure auto-follow ([`CameraFollow`]) and the only
//! camera control was the keyboard's ←/→ player yaw — unusable on a
//! touch screen where WASD does not exist and the player must be able
//! to look around while walking.
//!
//! # Contract
//!
//! * [`LookControl`] accumulates drag deltas (pixels) into a
//!   **camera orbit**: `yaw` (horizontal, world-space radians, applied
//!   ON TOP of the player's yaw, free-spinning) and `pitch` (vertical,
//!   clamped to [`PITCH_MIN`]..=[`PITCH_MAX`]).
//! * Sign conventions (pinned by tests): dragging RIGHT (dx > 0)
//!   turns the VIEW right (the orbit yaw decreases — a positive world
//!   yaw turns left, matching the app's ←-key sign); dragging UP
//!   (dy < 0 — screen y grows downward) looks UP (pitch increases,
//!   which LOWERS the camera eye and pitches the view up).
//! * The pitch clamp is geometric, not aesthetic: the eye orbits
//!   through the player, and at the default (0, 2, 5) offset the eye
//!   height is `2·cos φ − 5·sin φ`, which reaches the ground plane at
//!   φ = atan(2/5) ≈ 0.38 rad — [`PITCH_MAX`] 0.35 keeps a margin.
//!   [`PITCH_MIN`] −1.2 (≈ 69° overhead) is a sanity bound on the
//!   other side.
//! * [`LookControl::apply_to`] pushes the orbit into a
//!   [`CameraFollow`] (which gained `yaw_offset`/`pitch_offset`
//!   fields; zero offsets reproduce the legacy auto-follow
//!   bit-for-bit — guarded skips, pinned by a test).
//!
//! # Evidence
//!
//! * DESIGN (ours): every number here is a documented placeholder of
//!   the same class as the block-21 camera parameters — the original
//!   `proto::CameraBehaviour` config has not been recovered.
//! * PROVEN (math): the orbit formulas are exact rotations
//!   (Rodrigues around the horizontal right axis; world +Y for yaw)
//!   and the ground-plane bound follows from the default offset;
//!   tests pin eye heights at both clamp ends.

#![forbid(unsafe_code)]

use prototype_math::{Quat, Vec3};

use crate::player::CameraFollow;

/// Pitch clamp, radians: the camera may look UP until the eye reaches
/// the ground plane for the default (0, 2, 5) offset
/// (`2·cos φ − 5·sin φ ≥ 0` at φ ≈ 0.38 — clamped below that with a
/// margin).
pub const PITCH_MAX: f32 = 0.35;
/// Pitch clamp, radians: ≈ 69° overhead view (sanity bound; no
/// geometric limit looking down — the eye rises above the player).
pub const PITCH_MIN: f32 = -1.2;

/// Default drag sensitivity: pixels of finger travel per radian of
/// orbit.  900 px/rad ≈ a full screen height for ~1.4 rad on a
/// 1080-class phone panel (DESIGN tuning).
pub const PIXELS_PER_RADIAN: f32 = 900.0;

/// The touch look control: drag deltas in, camera orbit out.
///
/// Device-free and unit-tested; the Android glue feeds it one
/// [`TouchMapped::Look`] delta per move sample and the frame loop
/// applies it through [`LookControl::apply_to`] before framing the
/// camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LookControl {
    /// Horizontal orbit offset around world +Y, radians.  Applied on
    /// top of the player's yaw; free-spinning (no wrap issues — the
    /// trig is periodic).
    pub yaw: f32,
    /// Vertical orbit offset, radians, clamped to
    /// [`PITCH_MIN`]..=[`PITCH_MAX`].  Positive = looking up (the eye
    /// lowers toward the ground plane and the view pitches up).
    pub pitch: f32,
    /// Drag pixels per radian of orbit (sensitivity; see
    /// [`PIXELS_PER_RADIAN`]).
    pub pixels_per_radian: f32,
}

impl Default for LookControl {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: 0.0,
            pixels_per_radian: PIXELS_PER_RADIAN,
        }
    }
}

impl LookControl {
    /// Start centred: no orbit offset, default sensitivity.
    pub const fn new() -> Self {
        Self {
            yaw: 0.0,
            pitch: 0.0,
            pixels_per_radian: PIXELS_PER_RADIAN,
        }
    }

    /// Accumulate one drag sample (pixel deltas; dx right-positive,
    /// dy down-positive — winit touch locations).
    ///
    /// Drag right (dx > 0) turns the VIEW right: the orbit yaw
    /// DECREASES (a positive world yaw turns left — see module docs).
    /// Drag up (dy < 0) looks up: pitch increases, clamped.
    pub fn drag(&mut self, dx_px: f32, dy_px: f32) {
        let k = 1.0 / self.pixels_per_radian.max(1.0);
        self.yaw -= dx_px * k;
        self.pitch = (self.pitch - dy_px * k).clamp(PITCH_MIN, PITCH_MAX);
    }

    /// The camera's world-space look yaw for a player whose own yaw is
    /// `player_yaw` (radians): the player's yaw plus the orbit offset.
    pub fn camera_yaw(&self, player_yaw: f32) -> f32 {
        player_yaw + self.yaw
    }

    /// Push the current orbit into a [`CameraFollow`] — the app calls
    /// this once per frame before framing the camera.  Zero offsets
    /// leave the follow exactly at its legacy auto-follow behaviour.
    pub fn apply_to(&self, follow: &mut CameraFollow) {
        follow.yaw_offset = self.yaw;
        follow.pitch_offset = self.pitch;
    }

    /// Reset the orbit (e.g. on suspend — the auto-follow is the sane
    /// resume pose).
    pub fn reset(&mut self) {
        self.yaw = 0.0;
        self.pitch = 0.0;
    }
}

/// Rotate `v` around the horizontal axis perpendicular to its ground
/// projection (the camera's "right"), by `angle` radians — the pitch
/// half of the orbit.  Rodrigues' formula; the axis degenerates only
/// for a zero ground projection (never true for the follow offsets —
/// guarded by returning `v` unchanged).
fn pitch_around_right(v: Vec3, angle: f32) -> Vec3 {
    let h = Vec3::new(v.x, 0.0, v.z);
    if h.length_sq() < 1e-10 {
        return v;
    }
    // right = cross(UP, ĥ) — for h = (0, 0, 5) this is +X (the
    // camera sits behind at +Z, its right is +X).
    let right = Vec3::new(h.z, 0.0, -h.x).normalized();
    let c = angle.cos();
    let s = angle.sin();
    // Rodrigues: v·cos + (a×v)·sin + a·(a·v)·(1−cos).
    v * c + right.cross(v) * s + right * (right.dot(v) * (1.0 - c))
}

/// Apply a [`CameraFollow`]'s look offsets to a player-local offset
/// vector (shared by the follow's eye computation and its tests):
/// yaw around world +Y first, then pitch around the yawed right.
pub fn apply_look_offsets(local_offset: Vec3, yaw_offset: f32, pitch_offset: f32) -> Vec3 {
    let mut v = local_offset;
    if yaw_offset != 0.0 {
        v = Quat::from_axis_angle(Vec3::UP, yaw_offset)
            .to_mat4()
            .transform_dir(v);
    }
    if pitch_offset != 0.0 {
        v = pitch_around_right(v, pitch_offset);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use prototype_math::Vec3 as V3;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn new_look_control_is_centred() {
        let l = LookControl::new();
        assert_eq!(l.yaw, 0.0);
        assert_eq!(l.pitch, 0.0);
        assert_eq!(l.pixels_per_radian, PIXELS_PER_RADIAN);
        assert_eq!(l.camera_yaw(2.6), 2.6, "zero offset is transparent");
    }

    #[test]
    fn drag_right_turns_the_view_right() {
        // dx > 0 (finger right) must DECREASE the orbit yaw (a positive
        // world yaw turns left; the view must turn right).
        let mut l = LookControl::new();
        l.drag(100.0, 0.0);
        assert!(l.yaw < 0.0, "yaw = {}", l.yaw);
        assert!(approx(l.yaw, -100.0 / PIXELS_PER_RADIAN));
        assert_eq!(l.pitch, 0.0, "horizontal drag never pitches");
    }

    #[test]
    fn drag_up_looks_up() {
        // dy < 0 (finger up — screen y grows downward) must INCREASE
        // the pitch (looking up).
        let mut l = LookControl::new();
        l.drag(0.0, -100.0);
        assert!(l.pitch > 0.0, "pitch = {}", l.pitch);
        assert!(approx(l.pitch, 100.0 / PIXELS_PER_RADIAN));
        assert_eq!(l.yaw, 0.0, "vertical drag never yaws");
    }

    #[test]
    fn pitch_clamps_at_both_ends_yaw_spins_free() {
        let mut l = LookControl::new();
        l.drag(0.0, -1_000_000.0);
        assert_eq!(l.pitch, PITCH_MAX, "looking up clamps at the ground bound");
        l.drag(0.0, 1_000_000.0);
        assert_eq!(l.pitch, PITCH_MIN, "looking down clamps at the sanity bound");
        // Yaw is free-spinning: a huge horizontal drag is NOT clamped.
        // Drag LEFT (dx < 0) turns the view left → yaw INCREASES.
        let yaw_before = l.yaw;
        l.drag(-1_000_000.0, 0.0);
        assert!(l.yaw > yaw_before);
        assert!((l.yaw - (yaw_before + 1_000_000.0 / PIXELS_PER_RADIAN)).abs() < 1e-2);
    }

    #[test]
    fn camera_yaw_adds_the_player_yaw() {
        let mut l = LookControl::new();
        l.drag(900.0, 0.0); // one radian to the right
        assert!(approx(l.camera_yaw(0.5), 0.5 - 1.0));
    }

    #[test]
    fn apply_to_sets_the_follow_offsets_and_reset_clears() {
        let mut l = LookControl::new();
        l.drag(90.0, -180.0);
        let mut follow = CameraFollow::default();
        assert_eq!(follow.yaw_offset, 0.0);
        assert_eq!(follow.pitch_offset, 0.0);
        l.apply_to(&mut follow);
        assert!(approx(follow.yaw_offset, l.yaw));
        assert!(approx(follow.pitch_offset, l.pitch));
        l.reset();
        assert_eq!(l.yaw, 0.0);
        assert_eq!(l.pitch, 0.0);
    }

    #[test]
    fn zero_offsets_reproduce_the_legacy_offset_exactly() {
        // apply_look_offsets with both offsets zero is the identity —
        // the guard the CameraFollow bit-for-bit compatibility leans on.
        let v = V3::new(0.0, 2.0, 5.0);
        assert_eq!(apply_look_offsets(v, 0.0, 0.0), v);
    }

    #[test]
    fn yaw_offset_orbits_around_world_up() {
        // The block-21 rotation test's numbers, through the new path:
        // (0, 2, 5) yawed by π/2 → (5, 2, 0).
        let v = apply_look_offsets(V3::new(0.0, 2.0, 5.0), std::f32::consts::FRAC_PI_2, 0.0);
        assert!(approx(v.x, 5.0), "x = {}", v.x);
        assert!(approx(v.y, 2.0));
        assert!(approx(v.z, 0.0), "z = {}", v.z);
    }

    #[test]
    fn positive_pitch_lowers_the_eye_looking_up() {
        // φ = +0.35: eye height = 2·cos − 5·sin ≈ 0.166 (just above the
        // ground plane — the clamp's geometric justification) and the
        // camera is FARTHER behind (z grows).
        let v = apply_look_offsets(V3::new(0.0, 2.0, 5.0), 0.0, PITCH_MAX);
        let want_y = 2.0 * PITCH_MAX.cos() - 5.0 * PITCH_MAX.sin();
        assert!(approx(v.y, want_y), "y = {} want {}", v.y, want_y);
        assert!(v.y > 0.0, "the clamp keeps the eye above the ground plane");
        assert!(v.z > 5.0, "looking up also pushes the eye back: z = {}", v.z);
    }

    #[test]
    fn negative_pitch_raises_the_eye_looking_down() {
        // φ = −1.2: eye height ≈ 2·0.362 + 5·0.932 ≈ 5.39 — overhead.
        let v = apply_look_offsets(V3::new(0.0, 2.0, 5.0), 0.0, PITCH_MIN);
        let want_y = 2.0 * PITCH_MIN.cos() - 5.0 * PITCH_MIN.sin();
        assert!(approx(v.y, want_y), "y = {} want {}", v.y, want_y);
        assert!(v.y > 5.0, "nearly overhead: y = {}", v.y);
    }

    #[test]
    fn yaw_then_pitch_compose_in_that_order() {
        // Yaw first, then pitch around the YAWED right: the pitch must
        // act in the rotated frame (a yawed camera looking up lowers
        // along ITS right, not world +X).
        let yaw = std::f32::consts::FRAC_PI_2;
        let pitch = 0.2;
        let v = apply_look_offsets(V3::new(0.0, 2.0, 5.0), yaw, pitch);
        // Yawed offset: (5, 2, 0); its right = cross(UP, (5,0,0))/|·| =
        // (0,0,−1); Rodrigues: v' = v·c + (a×v)·s + a·(a·v)·(1−c) with
        // a×v = (0,0,−1)×(5,2,0) = (2,−5,0) and a·v = 0 →
        // v' = (5c + 2s, 2c − 5s, 0).
        let c = pitch.cos();
        let s = pitch.sin();
        assert!(approx(v.x, 5.0 * c + 2.0 * s), "x = {}", v.x);
        assert!(approx(v.y, 2.0 * c - 5.0 * s), "y = {}", v.y);
        assert!(approx(v.z, 0.0), "z = {}", v.z);
    }
}

//! player — the typed Player entity (M11f).
//!
//! ## What this closes
//!
//! The 60 free-roam events and 70 missions are first-class engine
//! entities (M9/M11a).  The PLAYER is the missing piece — the
//! entity that drives the streaming grid (M11e), the camera (the
//! render-scene's `Camera`), and the input (M11b).  This module
//! adds a typed [`Player`] entity with a [`Transform`] (from
//! `prototype-math`), a state machine (Idle/Move/Airborne — a
//! minimal subset of the original game's animation states), and a
//! camera-follow helper that updates a [`Camera`] to track the
//! player.
//!
//! Block 38 adds the look-orbit plumbing the touch camera control
//! needs: [`Player::yaw`] (the +Y heading extracted from the
//! rotation) and [`Player::move_camera_relative`] (WASD in the
//! CAMERA's frame with the player turning to face the movement
//! direction — the third-person control feel of the original game,
//! documented DESIGN).
//!
//! ## PROVEN vs LIKELY
//!
//! * PROVEN: `prototype_math::Transform` (composed of position +
//!   rotation + scale) is the right model for a player's world
//!   placement (the original engine's RTTI declares
//!   `engine::GetCurrentDrawable` which implies a drawable
//!   transform).
//! * LIKELY: the player's animation-state set (Idle / Move /
//!   Airborne) is OUR clean-room minimal subset; the original
//!   game's `proto::MotionStateBehaviour` has many more states
//!   (walk/run/jump/fall/land/etc.).
//! * UNKNOWN: the exact camera-follow parameters (offset,
//!   look-ahead, smoothing) — the original engine's camera
//!   controller is in `proto::CameraBehaviour` (RTTI class), but
//!   its parameters are in `.p3d` config files we haven't
//!   recovered for this purpose.

#![forbid(unsafe_code)]

use prototype_math::{Mat4, Transform, Vec3};

// ---------------------------------------------------------------------------
// PlayerState
// ---------------------------------------------------------------------------

/// Minimal animation-state machine for the player (OUR clean-room
/// model — see module docs).  The original engine has many more
/// states; this subset is enough for the gameplay-frame loop to
/// dispatch the right camera mode and animation set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlayerState {
    /// Standing still (no input).
    Idle,
    /// Moving on the ground (WASD or analog stick active).
    Move,
    /// In the air (jumping or falling).
    Airborne,
}

impl PlayerState {
    pub fn as_str(&self) -> &'static str {
        match self {
            PlayerState::Idle => "Idle",
            PlayerState::Move => "Move",
            PlayerState::Airborne => "Airborne",
        }
    }
}

// ---------------------------------------------------------------------------
// Player
// ---------------------------------------------------------------------------

/// The typed Player entity: world [`Transform`] + current
/// [`PlayerState`].  Drives the streaming grid's player position,
/// the camera-follow logic, and the animation-state selection.
#[derive(Debug, Clone)]
pub struct Player {
    /// World-space transform (position + rotation + scale).
    pub transform: Transform,
    /// Current animation state.
    pub state: PlayerState,
    /// Forward vector in world space (the direction the player is
    /// facing).  Derived from `transform.rotation` if needed; kept
    /// here for O(1) access by the camera-follow.
    pub forward: Vec3,
    /// Player's health (0..max_health).  LIKELY: the original
    /// engine's health model is more complex (shields, regen,
    /// damage types); this is a minimal scalar.
    pub health: f32,
    /// Max health (health is clamped to [0, max_health]).
    pub max_health: f32,
}

impl Default for Player {
    fn default() -> Self {
        Self {
            transform: Transform::default(),
            state: PlayerState::Idle,
            forward: Vec3::new(0.0, 0.0, -1.0),
            health: 100.0,
            max_health: 100.0,
        }
    }
}

impl Player {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn at(position: Vec3) -> Self {
        Self {
            transform: Transform::from_position(position),
            ..Default::default()
        }
    }

    pub fn position(&self) -> Vec3 {
        self.transform.position
    }

    /// Apply damage; health is clamped to [0, max_health].
    /// Returns true if the player is still alive after the damage.
    pub fn damage(&mut self, amount: f32) -> bool {
        self.health = (self.health - amount).max(0.0);
        self.health > 0.0
    }

    /// Heal; health is clamped to [0, max_health].
    pub fn heal(&mut self, amount: f32) {
        self.health = (self.health + amount).min(self.max_health);
    }

    pub fn is_alive(&self) -> bool {
        self.health > 0.0
    }

    /// Set the player's state.  Transitions are free (no
    /// state-machine gating); the gameplay frame loop is
    /// responsible for the policy (e.g. can't go Airborne
    /// without a jump input).
    pub fn set_state(&mut self, state: PlayerState) {
        self.state = state;
    }

    /// The player's world-space yaw in radians — the +Y heading
    /// extracted from `transform.rotation` via the horizontal forward
    /// (the −Z column of the rotation matrix: forward(yaw) =
    /// (−sin yaw, 0, −cos yaw), so `yaw = atan2(−f.x, −f.z)`).
    /// 0.0 for a degenerate rotation whose forward has no horizontal
    /// component (straight up/down) — the honest fallback, documented.
    ///
    /// Block 38: the camera-relative movement and the touch look orbit
    /// both key off this heading.
    pub fn yaw(&self) -> f32 {
        let rot = self.transform.rotation.to_mat4();
        let fx = -rot.cols[2][0];
        let fz = -rot.cols[2][2];
        if fx * fx + fz * fz < 1e-10 {
            0.0
        } else {
            (-fx).atan2(-fz)
        }
    }

    /// Apply keyboard WASD input relative to the CAMERA's frame
    /// (block 38, the touch-look control's movement half).
    ///
    /// `camera_yaw` is the camera's world-space look yaw (the player's
    /// yaw plus the look orbit's `yaw_offset` — see
    /// `crate::look::LookControl::camera_yaw`).  W moves along the
    /// camera's horizontal forward, S backs up, A/D strafe; the player
    /// then TURNS TO FACE the movement direction (instant snap — a
    /// documented DESIGN simplification of the original's smoothed
    /// turn) and translates at `speed`.  With a zero orbit offset and
    /// an already-facing player this is exactly
    /// [`Player::move_with_input`] (pinned by a test).
    ///
    /// State updates match `move_with_input`: Move while any key is
    /// held, Idle otherwise, Airborne never overridden.  Returns the
    /// applied world-space displacement.
    pub fn move_camera_relative(
        &mut self,
        input: &prototype_input::InputState,
        delta_seconds: f32,
        speed: f32,
        camera_yaw: f32,
    ) -> Vec3 {
        let (s, c) = camera_yaw.sin_cos();
        // Camera forward/right in world XZ (the same forward(yaw)
        // convention as the rotation matrix: forward = (−sin, −cos),
        // right = (cos, −sin)).
        let cam_forward = Vec3::new(-s, 0.0, -c);
        let cam_right = Vec3::new(c, 0.0, -s);

        let mut move_dir = Vec3::ZERO;
        if input.is_key_down(prototype_input::KeyCode::W) {
            move_dir = move_dir + cam_forward;
        }
        if input.is_key_down(prototype_input::KeyCode::S) {
            move_dir = move_dir - cam_forward;
        }
        if input.is_key_down(prototype_input::KeyCode::A) {
            move_dir = move_dir - cam_right;
        }
        if input.is_key_down(prototype_input::KeyCode::D) {
            move_dir = move_dir + cam_right;
        }

        let is_moving = move_dir.length_sq() > 0.0;
        if self.state != PlayerState::Airborne {
            self.state = if is_moving { PlayerState::Move } else { PlayerState::Idle };
        }
        if !is_moving {
            return Vec3::ZERO;
        }
        let normalized = move_dir.normalized();
        // Face the movement direction: forward(yaw) = (−sin, 0, −cos)
        // ⇒ yaw = atan2(−d.x, −d.z).  Instant snap (DESIGN — see the
        // doc comment).
        let desired_yaw = (-normalized.x).atan2(-normalized.z);
        self.transform.rotation = prototype_math::Quat::from_axis_angle(
            prototype_math::Vec3::UP,
            desired_yaw,
        );
        self.forward = normalized;
        let displacement = normalized * (speed * delta_seconds);
        self.transform.position = self.transform.position + displacement;
        displacement
    }

    /// Apply keyboard WASD input to the player's position.  W
    /// moves forward (along the player's forward vector, derived
    /// from `transform.rotation`), S backward, A left, D right.
    /// Movement is in the XZ plane (Y stays 0 — no gravity / jump
    /// in this minimal model).
    ///
    /// Also updates the player's `state` to `Move` if any WASD
    /// key is held, else `Idle` (only when the player isn't
    /// `Airborne` — the gameplay loop sets `Airborne` separately
    /// for jumps and we don't override it).
    ///
    /// Returns the world-space movement vector that was applied
    /// (for visualization / debugging).
    pub fn move_with_input(
        &mut self,
        input: &prototype_input::InputState,
        delta_seconds: f32,
        speed: f32,
    ) -> Vec3 {
        // Compute the player's forward and right vectors from the
        // transform's rotation.  We use the rotation matrix's
        // first two columns (X = right, Z = forward in world
        // space).  In a right-handed system with +Y up, the
        // default forward is -Z (so `forward = -rot.col_z`, but
        // for `look_at_rh` the camera looks down -Z so we use the
        // convention that the player's forward is -Z by default).
        let rot = self.transform.rotation.to_mat4();
        // Right vector: column 0 of the rotation matrix.
        let right = Vec3::new(rot.cols[0][0], 0.0, rot.cols[0][2]).normalized();
        // Forward vector: -column 2 (player looks down -Z by
        // default).
        let forward = Vec3::new(-rot.cols[2][0], 0.0, -rot.cols[2][2]).normalized();

        let mut move_dir = Vec3::ZERO;
        if input.is_key_down(prototype_input::KeyCode::W) {
            move_dir = move_dir + forward;
        }
        if input.is_key_down(prototype_input::KeyCode::S) {
            move_dir = move_dir - forward;
        }
        if input.is_key_down(prototype_input::KeyCode::A) {
            move_dir = move_dir - right;
        }
        if input.is_key_down(prototype_input::KeyCode::D) {
            move_dir = move_dir + right;
        }

        // Update the player's state based on input.
        let is_moving = move_dir.length_sq() > 0.0;
        if self.state != PlayerState::Airborne {
            self.state = if is_moving { PlayerState::Move } else { PlayerState::Idle };
        }

        // Normalize the move direction (so diagonal isn't faster)
        // and apply the delta.
        if is_moving {
            let normalized = move_dir.normalized();
            let displacement = normalized * (speed * delta_seconds);
            self.transform.position = self.transform.position + displacement;
            // Update the player's `forward` field to match the
            // movement direction (so the camera follows the
            // direction of motion, not the orientation).
            self.forward = normalized;
            displacement
        } else {
            Vec3::ZERO
        }
    }
}

// ---------------------------------------------------------------------------
// Camera-follow
// ---------------------------------------------------------------------------

/// Camera-follow parameters (LIKELY — see module docs).  The
/// camera sits at `offset` behind + above the player, looks at
/// the player's position + `look_at_offset`, and the player's
/// forward direction is used to orient the offset.
///
/// Block 38 adds the touch-look ORBIT: `yaw_offset` (world-space
/// radians on top of the player's yaw) and `pitch_offset` (vertical
/// radians; positive lowers the eye / looks up, negative raises it /
/// looks down — clamped by `crate::look::LookControl` to
/// `[PITCH_MIN, PITCH_MAX]`).  Both default to 0.0 and ZERO offsets
/// reproduce the legacy auto-follow bit-for-bit (the orbit math is
/// guarded-skipped, pinned by a test).
#[derive(Debug, Clone, Copy)]
pub struct CameraFollow {
    /// World-space offset from the player to the camera.  The
    /// offset is applied in the player's local frame (rotated by
    /// the player's yaw), so the camera stays behind the player
    /// when they turn.
    pub offset: Vec3,
    /// Look-at target offset (added to the player's position).
    pub look_at_offset: Vec3,
    /// Block 38: horizontal orbit offset around world +Y, radians,
    /// applied ON TOP of the player's yaw (the touch look control's
    /// yaw).  0.0 = pure auto-follow.
    pub yaw_offset: f32,
    /// Block 38: vertical orbit offset, radians.  Positive LOWERS the
    /// eye (the view pitches up), negative RAISES it (the view pitches
    /// down).  0.0 = pure auto-follow.
    pub pitch_offset: f32,
}

impl Default for CameraFollow {
    fn default() -> Self {
        // 5 meters behind, 2 meters above, looking at the player's
        // center + 1 meter up.  No look orbit (the legacy block-21
        // behaviour).
        Self {
            offset: Vec3::new(0.0, 2.0, 5.0),
            look_at_offset: Vec3::new(0.0, 1.0, 0.0),
            yaw_offset: 0.0,
            pitch_offset: 0.0,
        }
    }
}

impl CameraFollow {
    /// Compute the camera eye and target positions for `player`.
    /// Returns `(eye, target)` world-space positions for
    /// [`prototype_math::Mat4::look_at_rh`].
    ///
    /// The orbit (block 38): the local offset is yawed around world
    /// +Y by `yaw_offset` on top of the player's yaw, then pitched
    /// around the yawed horizontal right axis by `pitch_offset`
    /// (Rodrigues — `crate::look::apply_look_offsets`).  The target
    /// does NOT move with the orbit: the eye orbits the player while
    /// the look point stays on them, which is what changes the view
    /// direction.
    pub fn eye_target(&self, player: &Player) -> (Vec3, Vec3) {
        // Compute the camera position in the player's local frame:
        // rotate the offset by the player's yaw (around +Y), then the
        // look orbit (no-op at zero offsets — legacy path unchanged).
        let yaw = player.transform.rotation;
        let local_offset = crate::look::apply_look_offsets(
            yaw.to_mat4().transform_dir(self.offset),
            self.yaw_offset,
            self.pitch_offset,
        );
        let eye = player.position() + local_offset;
        let target = player.position() + self.look_at_offset;
        (eye, target)
    }

    /// Build the view matrix for `player` using this camera-follow.
    pub fn view_matrix(&self, player: &Player) -> Mat4 {
        let (eye, target) = self.eye_target(player);
        Mat4::look_at_rh(eye, target, prototype_math::Vec3::UP)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use prototype_math::{Quat, Vec3};

    #[test]
    fn player_default_is_alive_at_full_health() {
        let p = Player::new();
        assert!(p.is_alive());
        assert_eq!(p.health, 100.0);
        assert_eq!(p.max_health, 100.0);
        assert_eq!(p.state, PlayerState::Idle);
        assert_eq!(p.position(), Vec3::ZERO);
    }

    #[test]
    fn player_at_constructs_at_position() {
        let p = Player::at(Vec3::new(10.0, 0.0, 20.0));
        assert_eq!(p.position(), Vec3::new(10.0, 0.0, 20.0));
        assert_eq!(p.state, PlayerState::Idle);
    }

    #[test]
    fn damage_clamps_to_zero_and_returns_alive_flag() {
        let mut p = Player::new();
        assert!(p.damage(30.0));
        assert_eq!(p.health, 70.0);
        assert!(p.is_alive());
        // 70 - 100 = -30 → clamped to 0; player is dead → false.
        assert!(!p.damage(100.0));
        assert_eq!(p.health, 0.0);
        assert!(!p.is_alive());
        // Overkill: still 0, still false.
        assert!(!p.damage(50.0));
        assert_eq!(p.health, 0.0);
    }

    #[test]
    fn heal_clamps_to_max() {
        let mut p = Player::new();
        p.damage(30.0).then(|| ());
        p.heal(10.0);
        assert_eq!(p.health, 80.0);
        p.heal(50.0);
        assert_eq!(p.health, 100.0);
    }

    #[test]
    fn set_state_changes_state() {
        let mut p = Player::new();
        assert_eq!(p.state, PlayerState::Idle);
        p.set_state(PlayerState::Move);
        assert_eq!(p.state, PlayerState::Move);
        p.set_state(PlayerState::Airborne);
        assert_eq!(p.state, PlayerState::Airborne);
        p.set_state(PlayerState::Idle);
        assert_eq!(p.state, PlayerState::Idle);
    }

    #[test]
    fn state_strings_round_trip() {
        assert_eq!(PlayerState::Idle.as_str(), "Idle");
        assert_eq!(PlayerState::Move.as_str(), "Move");
        assert_eq!(PlayerState::Airborne.as_str(), "Airborne");
    }

    #[test]
    fn camera_follow_default_offset_is_behind_and_above() {
        let cf = CameraFollow::default();
        // Default offset: 5 meters back (+Z is behind, since the
        // player's forward is -Z), 2 meters up.
        assert_eq!(cf.offset, Vec3::new(0.0, 2.0, 5.0));
        // Default look-at offset: 1 meter up from the player.
        assert_eq!(cf.look_at_offset, Vec3::new(0.0, 1.0, 0.0));
    }

    #[test]
    fn camera_follow_eye_target_for_idle_player_at_origin() {
        // Player at origin, facing -Z (default forward).
        let p = Player::new();
        let cf = CameraFollow::default();
        let (eye, target) = cf.eye_target(&p);
        // Eye: player_pos + offset rotated by player's yaw.
        // The player's yaw is identity (Quat::IDENTITY), so the
        // offset is applied as-is: (0, 2, 5).
        assert!((eye.x - 0.0).abs() < 1e-5);
        assert!((eye.y - 2.0).abs() < 1e-5);
        assert!((eye.z - 5.0).abs() < 1e-5);
        // Target: player_pos + look_at_offset = (0, 1, 0).
        assert!((target.x - 0.0).abs() < 1e-5);
        assert!((target.y - 1.0).abs() < 1e-5);
        assert!((target.z - 0.0).abs() < 1e-5);
    }

    #[test]
    fn camera_follow_eye_moves_with_player() {
        // Player at (10, 0, 0).  Eye should be at (10, 2, 5).
        let p = Player::at(Vec3::new(10.0, 0.0, 0.0));
        let cf = CameraFollow::default();
        let (eye, _target) = cf.eye_target(&p);
        assert!((eye.x - 10.0).abs() < 1e-5);
        assert!((eye.y - 2.0).abs() < 1e-5);
        assert!((eye.z - 5.0).abs() < 1e-5);
    }

    #[test]
    fn camera_follow_rotates_with_player_yaw() {
        // Player turns 90° around +Y (looking toward +X).  The
        // camera should rotate too: the offset (0, 2, 5) becomes
        // (5, 2, 0) — 5 meters along +X (behind the player's new
        // forward of +X), 2 meters up.
        let mut p = Player::new();
        p.transform.rotation = Quat::from_axis_angle(Vec3::UP, std::f32::consts::FRAC_PI_2);
        let cf = CameraFollow::default();
        let (eye, _target) = cf.eye_target(&p);
        // The offset (0, 2, 5) rotated by 90° around +Y:
        //   +Z becomes +X (right-handed, +Y up).
        // So eye = (5, 2, 0).
        assert!((eye.x - 5.0).abs() < 1e-5, "eye.x = {}", eye.x);
        assert!((eye.y - 2.0).abs() < 1e-5, "eye.y = {}", eye.y);
        assert!((eye.z - 0.0).abs() < 1e-5, "eye.z = {}", eye.z);
    }

    #[test]
    fn camera_follow_view_matrix_places_eye_at_origin() {
        // The view matrix should transform the eye position to the
        // origin (the look_at_rh invariant).
        let p = Player::at(Vec3::new(10.0, 0.0, 0.0));
        let cf = CameraFollow::default();
        let view = cf.view_matrix(&p);
        let (eye, _) = cf.eye_target(&p);
        let eye_in_view = view.transform_point(eye);
        assert!((eye_in_view.x - 0.0).abs() < 1e-5);
        assert!((eye_in_view.y - 0.0).abs() < 1e-5);
        assert!((eye_in_view.z - 0.0).abs() < 1e-5);
    }

    #[test]
    fn player_at_origin_streams_zero_cell_through_default_offset() {
        // The streaming grid (M11e) uses the player's position.
        // The camera-follow's eye is offset from the player, but
        // the streaming grid sees the PLAYER's position, not the
        // camera's.  This test proves the player's position is the
        // streaming input (a sanity check on the integration,
        // which lives in EngineRuntime).
        let p = Player::new();
        assert_eq!(p.position(), Vec3::ZERO);
        // The camera's eye is at (0, 2, 5) — different from the
        // player's position.  The streaming grid would query the
        // player's position (0, 0, 0), not the camera's.
        let cf = CameraFollow::default();
        let (eye, _) = cf.eye_target(&p);
        assert_ne!(eye, p.position());
    }

    // ----- M11g: input-driven movement ----------------------------

    #[test]
    fn move_with_input_no_keys_no_movement() {
        let mut p = Player::at(Vec3::new(10.0, 0.0, 10.0));
        let input = prototype_input::InputState::new();
        let disp = p.move_with_input(&input, 0.016, 5.0);
        assert_eq!(disp, Vec3::ZERO);
        assert_eq!(p.position(), Vec3::new(10.0, 0.0, 10.0));
        // No movement → state should be Idle (player was Idle
        // already).
        assert_eq!(p.state, PlayerState::Idle);
    }

    #[test]
    fn move_with_input_w_moves_forward() {
        let mut p = Player::new(); // at origin, forward = -Z (default)
        let mut input = prototype_input::InputState::new();
        input.apply(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::W));
        // 1 second at 5 m/s → 5 meters forward.
        let disp = p.move_with_input(&input, 1.0, 5.0);
        // Default forward is -Z, so displacement is (0, 0, -5).
        assert!((disp.x - 0.0).abs() < 1e-5, "disp.x = {}", disp.x);
        assert!((disp.y - 0.0).abs() < 1e-5, "disp.y = {}", disp.y);
        assert!((disp.z + 5.0).abs() < 1e-5, "disp.z = {}", disp.z);
        // Position updated.
        assert!((p.position().z + 5.0).abs() < 1e-5);
        // State is Move (W is held).
        assert_eq!(p.state, PlayerState::Move);
    }

    #[test]
    fn move_with_input_s_moves_backward() {
        let mut p = Player::new();
        let mut input = prototype_input::InputState::new();
        input.apply(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::S));
        let disp = p.move_with_input(&input, 1.0, 5.0);
        // S = backward → +Z (opposite of forward -Z).
        assert!((disp.z - 5.0).abs() < 1e-5, "disp.z = {}", disp.z);
        assert_eq!(p.state, PlayerState::Move);
    }

    #[test]
    fn move_with_input_d_moves_right() {
        let mut p = Player::new();
        let mut input = prototype_input::InputState::new();
        input.apply(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::D));
        let disp = p.move_with_input(&input, 1.0, 5.0);
        // D = right → +X (right is +X by default).
        assert!((disp.x - 5.0).abs() < 1e-5, "disp.x = {}", disp.x);
        assert!((disp.z - 0.0).abs() < 1e-5, "disp.z = {}", disp.z);
        assert_eq!(p.state, PlayerState::Move);
    }

    #[test]
    fn move_with_input_a_moves_left() {
        let mut p = Player::new();
        let mut input = prototype_input::InputState::new();
        input.apply(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::A));
        let disp = p.move_with_input(&input, 1.0, 5.0);
        // A = left → -X.
        assert!((disp.x + 5.0).abs() < 1e-5, "disp.x = {}", disp.x);
        assert_eq!(p.state, PlayerState::Move);
    }

    #[test]
    fn move_with_input_diagonal_is_normalized() {
        // Pressing W + D should give diagonal movement at unit
        // speed (not sqrt(2) faster).
        let mut p = Player::new();
        let mut input = prototype_input::InputState::new();
        input.apply(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::W));
        input.apply(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::D));
        let disp = p.move_with_input(&input, 1.0, 5.0);
        // Magnitude should be 5 (speed × 1 second), not 5*sqrt(2).
        let mag = disp.length();
        assert!((mag - 5.0).abs() < 1e-5, "diag mag = {}", mag);
    }

    #[test]
    fn move_with_input_doesnt_override_airborne_state() {
        // When the player is Airborne (e.g. jumping), move_with_input
        // should NOT change the state to Move/Idle even if WASD is
        // held (the gameplay loop controls Airborne separately).
        let mut p = Player::new();
        p.set_state(PlayerState::Airborne);
        let mut input = prototype_input::InputState::new();
        input.apply(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::W));
        let _disp = p.move_with_input(&input, 1.0, 5.0);
        // Still Airborne.
        assert_eq!(p.state, PlayerState::Airborne);
        // But the position DID update (movement still applies
        // while airborne — air control).
        assert!((p.position().z + 5.0).abs() < 1e-5);
    }

    // ----- Block 38: the look orbit + camera-relative movement ----

    #[test]
    fn yaw_extracts_the_rotation_heading() {
        let mut p = Player::new();
        assert_eq!(p.yaw(), 0.0, "default forward −Z is yaw 0");
        p.transform.rotation =
            Quat::from_axis_angle(Vec3::UP, std::f32::consts::FRAC_PI_2);
        assert!((p.yaw() - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
        // Negative yaw too.
        p.transform.rotation =
            Quat::from_axis_angle(Vec3::UP, -std::f32::consts::FRAC_PI_2);
        assert!((p.yaw() + std::f32::consts::FRAC_PI_2).abs() < 1e-4);
    }

    #[test]
    fn camera_relative_zero_offset_matches_move_with_input() {
        // With a zero orbit and an already-facing player, the new
        // camera-relative path IS the legacy path (the compatibility
        // claim the app's keyboard behaviour leans on).
        let mut a = Player::new();
        let mut b = Player::new();
        let mut input = prototype_input::InputState::new();
        input.apply(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::W));
        input.apply(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::D));
        let da = a.move_with_input(&input, 0.5, 7.0);
        let db = b.move_camera_relative(&input, 0.5, 7.0, b.yaw());
        assert!((da.x - db.x).abs() < 1e-4 && (da.z - db.z).abs() < 1e-4);
        assert_eq!(a.position(), b.position());
        assert_eq!(a.state, b.state);
        // move_with_input leaves the ROTATION alone (it only updates
        // the `forward` field); the camera-relative path snaps the
        // facing to the walk heading: the W+D diagonal from identity
        // is yaw = atan2(−0.7071, +0.7071) = −π/4.
        assert!(a.yaw().abs() < 1e-6, "legacy path never rotates");
        assert!((b.yaw() + std::f32::consts::FRAC_PI_4).abs() < 1e-4, "snap = walk heading");
    }

    #[test]
    fn camera_relative_w_follows_the_camera_yaw() {
        // Camera yawed 90° right of the player: W must walk along the
        // CAMERA's forward (−X), not the player's old forward (−Z).
        let mut p = Player::new();
        let mut input = prototype_input::InputState::new();
        input.apply(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::W));
        let disp = p.move_camera_relative(&input, 1.0, 5.0, std::f32::consts::FRAC_PI_2);
        assert!((disp.x + 5.0).abs() < 1e-4, "x = {}", disp.x);
        assert!(disp.z.abs() < 1e-4, "z = {}", disp.z);
        // The player FACES the movement direction now: forward(yaw) =
        // (−sin, −cos) = (−1, 0) ⇒ yaw = +π/2.
        assert!((p.yaw() - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
    }

    #[test]
    fn camera_relative_d_strafes_and_faces_right() {
        // D at camera yaw 0 strafes +X; the player turns to face +X
        // (yaw = atan2(−1, 0) = −π/2).
        let mut p = Player::new();
        let mut input = prototype_input::InputState::new();
        input.apply(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::D));
        let disp = p.move_camera_relative(&input, 1.0, 5.0, 0.0);
        assert!((disp.x - 5.0).abs() < 1e-4, "x = {}", disp.x);
        assert!((p.yaw() + std::f32::consts::FRAC_PI_2).abs() < 1e-4);
        assert_eq!(p.state, PlayerState::Move);
    }

    #[test]
    fn camera_relative_s_backs_up_along_the_camera_forward() {
        // S at camera yaw π walks +Z (the camera looks down −Z·−1…)
        // — backward along the camera's forward.
        let mut p = Player::new();
        let mut input = prototype_input::InputState::new();
        input.apply(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::S));
        let disp = p.move_camera_relative(&input, 1.0, 5.0, 0.0);
        assert!((disp.z - 5.0).abs() < 1e-4, "z = {}", disp.z);
    }

    #[test]
    fn camera_relative_no_keys_is_a_noop_even_with_an_offset() {
        let mut p = Player::at(Vec3::new(3.0, 0.0, 4.0));
        let input = prototype_input::InputState::new();
        let disp = p.move_camera_relative(&input, 1.0, 5.0, 1.234);
        assert_eq!(disp, Vec3::ZERO);
        assert_eq!(p.position(), Vec3::new(3.0, 0.0, 4.0));
        assert_eq!(p.state, PlayerState::Idle);
        assert_eq!(p.yaw(), 0.0, "no movement → no facing snap");
    }

    #[test]
    fn camera_relative_doesnt_override_airborne() {
        let mut p = Player::new();
        p.set_state(PlayerState::Airborne);
        let mut input = prototype_input::InputState::new();
        input.apply(prototype_input::InputEvent::KeyDown(prototype_input::KeyCode::W));
        let _ = p.move_camera_relative(&input, 1.0, 5.0, 0.0);
        assert_eq!(p.state, PlayerState::Airborne);
        assert!((p.position().z + 5.0).abs() < 1e-5, "air control still applies");
    }

    #[test]
    fn follow_with_yaw_offset_orbits_the_eye() {
        // The look yaw orbit: (0, 2, 5) yawed π/2 on top of the
        // player's identity yaw → eye (5, 2, 0); the TARGET stays on
        // the player's look point (the orbit changes the view).
        let p = Player::new();
        let mut follow = CameraFollow::default();
        follow.yaw_offset = std::f32::consts::FRAC_PI_2;
        let (eye, target) = follow.eye_target(&p);
        assert!((eye.x - 5.0).abs() < 1e-4, "eye.x = {}", eye.x);
        assert!((eye.y - 2.0).abs() < 1e-4);
        assert!((eye.z - 0.0).abs() < 1e-4, "eye.z = {}", eye.z);
        assert!((target.x).abs() < 1e-6 && (target.y - 1.0).abs() < 1e-6 && target.z.abs() < 1e-6);
    }

    #[test]
    fn follow_with_pitch_offset_moves_the_eye_vertically() {
        // Positive pitch (looking up) LOWERS the eye toward the ground
        // plane; negative (looking down) RAISES it overhead.  Both
        // clamps keep the eye above y = 0 at the default offset.
        let p = Player::new();
        let mut up = CameraFollow::default();
        up.pitch_offset = 0.35;
        let (eye_up, _) = up.eye_target(&p);
        assert!((eye_up.y - (2.0 * 0.35f32.cos() - 5.0 * 0.35f32.sin())).abs() < 1e-4);
        assert!(eye_up.y > 0.0, "ground constraint: y = {}", eye_up.y);
        let mut down = CameraFollow::default();
        down.pitch_offset = -1.2;
        let (eye_down, _) = down.eye_target(&p);
        assert!(eye_down.y > 5.0, "overhead: y = {}", eye_down.y);
        // The target never moves with the orbit.
        let (_, target) = down.eye_target(&p);
        assert!((target.y - 1.0).abs() < 1e-6);
    }

    #[test]
    fn follow_zero_offsets_are_bit_identical_to_the_legacy_path() {
        // The compatibility guard: both orbit fields at 0.0 must give
        // EXACTLY the legacy eye (the guarded-skip design).
        let p = Player::at(Vec3::new(1687.0, 11.0, -1845.0));
        let legacy = {
            let yaw = p.transform.rotation;
            let local = yaw.to_mat4().transform_dir(CameraFollow::default().offset);
            (p.position() + local, p.position() + CameraFollow::default().look_at_offset)
        };
        let (eye, target) = CameraFollow::default().eye_target(&p);
        assert_eq!(eye, legacy.0);
        assert_eq!(target, legacy.1);
    }
}

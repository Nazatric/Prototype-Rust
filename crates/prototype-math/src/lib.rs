//! prototype-math
//! ===============
//! Self-contained game math for the prototype-rs engine: 2/3/4-component
//! vectors, 4×4 column-major matrices, quaternions, and the standard
//! view/projection constructors needed by the renderer.
//!
//! Column-major, right-handed, clip space `z ∈ [0, 1]` (Vulkan / D3D
//! convention — Prototype's original D3D9 target used the same depth
//! range, so this matches the original engine's behavior).
//!
//! No `unsafe`, no dependencies, no panics in release (`*` panics on
//! divide-by-zero only in debug builds via the standard float behavior).

#![forbid(unsafe_code)]

use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

// ---------------------------------------------------------------------------
// Vec2
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };
    pub const fn new(x: f32, y: f32) -> Self { Self { x, y } }
    pub fn dot(self, o: Self) -> f32 { self.x * o.x + self.y * o.y }
    pub fn length(self) -> f32 { self.dot(self).sqrt() }
    pub fn normalized(self) -> Self {
        let l = self.length();
        if l > f32::EPSILON { Self::new(self.x / l, self.y / l) } else { self }
    }
}

impl Add for Vec2 { type Output = Self; fn add(self, o: Self) -> Self { Self::new(self.x + o.x, self.y + o.y) } }
impl Sub for Vec2 { type Output = Self; fn sub(self, o: Self) -> Self { Self::new(self.x - o.x, self.y - o.y) } }
impl Mul<f32> for Vec2 { type Output = Self; fn mul(self, s: f32) -> Self { Self::new(self.x * s, self.y * s) } }

// ---------------------------------------------------------------------------
// Vec3
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0, z: 0.0 };
    pub const UP: Self = Self { x: 0.0, y: 1.0, z: 0.0 };
    pub const fn new(x: f32, y: f32, z: f32) -> Self { Self { x, y, z } }
    pub const fn splat(v: f32) -> Self { Self { x: v, y: v, z: v } }

    pub fn dot(self, o: Self) -> f32 { self.x * o.x + self.y * o.y + self.z * o.z }
    pub fn cross(self, o: Self) -> Self {
        Self::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    pub fn length(self) -> f32 { self.dot(self).sqrt() }
    pub fn length_sq(self) -> f32 { self.dot(self) }
    pub fn normalized(self) -> Self {
        let l = self.length();
        if l > f32::EPSILON { Self::new(self.x / l, self.y / l, self.z / l) } else { self }
    }
    pub fn lerp(self, o: Self, t: f32) -> Self {
        self + (o - self) * t
    }
}

impl Add for Vec3 { type Output = Self; fn add(self, o: Self) -> Self { Self::new(self.x + o.x, self.y + o.y, self.z + o.z) } }
impl AddAssign for Vec3 { fn add_assign(&mut self, o: Self) { *self = *self + o; } }
impl Sub for Vec3 { type Output = Self; fn sub(self, o: Self) -> Self { Self::new(self.x - o.x, self.y - o.y, self.z - o.z) } }
impl SubAssign for Vec3 { fn sub_assign(&mut self, o: Self) { *self = *self - o; } }
impl Mul<f32> for Vec3 { type Output = Self; fn mul(self, s: f32) -> Self { Self::new(self.x * s, self.y * s, self.z * s) } }
impl Div<f32> for Vec3 { type Output = Self; fn div(self, s: f32) -> Self { Self::new(self.x / s, self.y / s, self.z / s) } }
impl Neg for Vec3 { type Output = Self; fn neg(self) -> Self { Self::new(-self.x, -self.y, -self.z) } }

// ---------------------------------------------------------------------------
// Vec4
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Vec4 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0, z: 0.0, w: 0.0 };
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self { Self { x, y, z, w } }
    pub fn from_vec3(v: Vec3, w: f32) -> Self { Self { x: v.x, y: v.y, z: v.z, w } }
    pub fn dot(self, o: Self) -> f32 { self.x * o.x + self.y * o.y + self.z * o.z + self.w * o.w }
    pub fn xyz(self) -> Vec3 { Vec3::new(self.x, self.y, self.z) }
}

// ---------------------------------------------------------------------------
// Mat4 — column-major
// ---------------------------------------------------------------------------

/// 4×4 matrix, column-major: `cols[c][r]`.  `m[(c, r)]` = column c, row r.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat4 {
    pub cols: [[f32; 4]; 4],
}

impl Default for Mat4 {
    fn default() -> Self { Self::IDENTITY }
}

impl Mat4 {
    pub const IDENTITY: Self = Self {
        cols: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
    };

    pub fn from_cols(c0: [f32; 4], c1: [f32; 4], c2: [f32; 4], c3: [f32; 4]) -> Self {
        Self { cols: [c0, c1, c2, c3] }
    }

    /// Translate by `t`.
    pub fn from_translation(t: Vec3) -> Self {
        let mut m = Self::IDENTITY;
        m.cols[3] = [t.x, t.y, t.z, 1.0];
        m
    }

    /// Uniform scale.
    pub fn from_scale(s: f32) -> Self {
        let mut m = Self::IDENTITY;
        m.cols[0][0] = s;
        m.cols[1][1] = s;
        m.cols[2][2] = s;
        m
    }

    /// Rotation about the X axis (radians).
    pub fn from_rotation_x(a: f32) -> Self {
        let (s, c) = a.sin_cos();
        Self::from_cols(
            [1.0, 0.0, 0.0, 0.0],
            [0.0, c, s, 0.0],
            [0.0, -s, c, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        )
    }

    /// Rotation about the Y axis (radians).
    pub fn from_rotation_y(a: f32) -> Self {
        let (s, c) = a.sin_cos();
        Self::from_cols(
            [c, 0.0, -s, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [s, 0.0, c, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        )
    }

    /// Rotation about the Z axis (radians).
    pub fn from_rotation_z(a: f32) -> Self {
        let (s, c) = a.sin_cos();
        Self::from_cols(
            [c, s, 0.0, 0.0],
            [-s, c, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        )
    }

    /// Matrix product `self * other` (apply `other` first, then `self`).
    pub fn mul_mat4(&self, o: &Mat4) -> Mat4 {
        let mut out = [[0f32; 4]; 4];
        for c in 0..4 {
            for r in 0..4 {
                out[c][r] = self.cols[0][r] * o.cols[c][0]
                    + self.cols[1][r] * o.cols[c][1]
                    + self.cols[2][r] * o.cols[c][2]
                    + self.cols[3][r] * o.cols[c][3];
            }
        }
        Mat4 { cols: out }
    }

    /// Transform a point (w=1).
    pub fn transform_point(&self, p: Vec3) -> Vec3 {
        let x = self.cols[0][0] * p.x + self.cols[1][0] * p.y + self.cols[2][0] * p.z + self.cols[3][0];
        let y = self.cols[0][1] * p.x + self.cols[1][1] * p.y + self.cols[2][1] * p.z + self.cols[3][1];
        let z = self.cols[0][2] * p.x + self.cols[1][2] * p.y + self.cols[2][2] * p.z + self.cols[3][2];
        Vec3::new(x, y, z)
    }

    /// Transform a point to clip space (w=1, returns Vec4).
    pub fn transform_point4(&self, p: Vec3) -> Vec4 {
        Vec4::new(
            self.cols[0][0] * p.x + self.cols[1][0] * p.y + self.cols[2][0] * p.z + self.cols[3][0],
            self.cols[0][1] * p.x + self.cols[1][1] * p.y + self.cols[2][1] * p.z + self.cols[3][1],
            self.cols[0][2] * p.x + self.cols[1][2] * p.y + self.cols[2][2] * p.z + self.cols[3][2],
            self.cols[0][3] * p.x + self.cols[1][3] * p.y + self.cols[2][3] * p.z + self.cols[3][3],
        )
    }

    /// Transform a direction (w=0, no translation).
    pub fn transform_dir(&self, d: Vec3) -> Vec3 {
        Vec3::new(
            self.cols[0][0] * d.x + self.cols[1][0] * d.y + self.cols[2][0] * d.z,
            self.cols[0][1] * d.x + self.cols[1][1] * d.y + self.cols[2][1] * d.z,
            self.cols[0][2] * d.x + self.cols[1][2] * d.y + self.cols[2][2] * d.z,
        )
    }

    /// Right-handed look-at view matrix (eye → target, up).
    pub fn look_at_rh(eye: Vec3, target: Vec3, up: Vec3) -> Self {
        let z = (eye - target).normalized(); // forward is -z in view space
        let x = up.cross(z).normalized();
        let y = z.cross(x);
        Self::from_cols(
            [x.x, y.x, z.x, 0.0],
            [x.y, y.y, z.y, 0.0],
            [x.z, y.z, z.z, 0.0],
            [-x.dot(eye), -y.dot(eye), -z.dot(eye), 1.0],
        )
    }

    /// Right-handed perspective projection with Vulkan-style
    /// `z ∈ [0, 1]` depth (also D3D-style; Prototype's D3D9 used
    /// the same range).  `fov_y` in radians.  Camera looks down −Z
    /// in view space; `w_clip = −z_view` so points ahead have w > 0.
    pub fn perspective_rh_zo(fov_y: f32, aspect: f32, near: f32, far: f32) -> Self {
        let f = 1.0 / (fov_y / 2.0).tan();
        // Solve z_ndc(-near) = 0, z_ndc(-far) = 1 with w = -z:
        //   z_clip = A·z + B,  A = far/(near-far),  B = A·near
        let a = far / (near - far);
        let b = far * near / (near - far);
        Self::from_cols(
            [f / aspect, 0.0, 0.0, 0.0],
            [0.0, -f, 0.0, 0.0], // negative Y: Vulkan clip-space Y is down
            [0.0, 0.0, a, -1.0],
            [0.0, 0.0, b, 0.0],
        )
    }

    /// Orthographic projection with `z ∈ [0, 1]` depth, right-handed
    /// (camera looks down −Z).
    pub fn orthographic_rh_zo(l: f32, r: f32, b: f32, t: f32, near: f32, far: f32) -> Self {
        // z_ndc(-near) = 0, z_ndc(-far) = 1:
        //   A = 1/(near-far), B = near/(near-far)
        let a = 1.0 / (near - far);
        let bb = near / (near - far);
        Self::from_cols(
            [2.0 / (r - l), 0.0, 0.0, 0.0],
            [0.0, -2.0 / (t - b), 0.0, 0.0],
            [0.0, 0.0, a, 0.0],
            [-(r + l) / (r - l), -(t + b) / (t - b), bb, 1.0],
        )
    }
}

impl Mul for Mat4 {
    type Output = Mat4;
    fn mul(self, o: Mat4) -> Mat4 { self.mul_mat4(&o) }
}

// ---------------------------------------------------------------------------
// Quat
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Default for Quat {
    fn default() -> Self { Self::IDENTITY }
}

impl Quat {
    pub const IDENTITY: Self = Self { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };

    pub fn from_axis_angle(axis: Vec3, angle: f32) -> Self {
        let a = axis.normalized();
        let (s, c) = (angle / 2.0).sin_cos();
        Self { x: a.x * s, y: a.y * s, z: a.z * s, w: c }
    }

    pub fn mul_quat(&self, o: &Quat) -> Quat {
        Quat {
            x: self.w * o.x + self.x * o.w + self.y * o.z - self.z * o.y,
            y: self.w * o.y - self.x * o.z + self.y * o.w + self.z * o.x,
            z: self.w * o.z + self.x * o.y - self.y * o.x + self.z * o.w,
            w: self.w * o.w - self.x * o.x - self.y * o.y - self.z * o.z,
        }
    }

    pub fn normalized(self) -> Self {
        let l = (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt();
        if l > f32::EPSILON {
            Self { x: self.x / l, y: self.y / l, z: self.z / l, w: self.w / l }
        } else {
            self
        }
    }

    /// Convert to a rotation matrix.
    pub fn to_mat4(self) -> Mat4 {
        let x2 = self.x + self.x;
        let y2 = self.y + self.y;
        let z2 = self.z + self.z;
        let xx = self.x * x2;
        let xy = self.x * y2;
        let xz = self.x * z2;
        let yy = self.y * y2;
        let yz = self.y * z2;
        let zz = self.z * z2;
        let wx = self.w * x2;
        let wy = self.w * y2;
        let wz = self.w * z2;
        Mat4::from_cols(
            [1.0 - (yy + zz), xy + wz, xz - wy, 0.0],
            [xy - wz, 1.0 - (xx + zz), yz + wx, 0.0],
            [xz + wy, yz - wx, 1.0 - (xx + yy), 0.0],
            [0.0, 0.0, 0.0, 1.0],
        )
    }
}

// ---------------------------------------------------------------------------
// Transform (position + rotation + scale) — the scene-graph node type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Self { position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: 1.0 }
    }
}

impl Transform {
    pub fn from_position(p: Vec3) -> Self {
        Self { position: p, ..Default::default() }
    }

    /// Compose: `self` is the parent, `child` is in parent space.
    /// Result = parent ∘ child (child applied first).
    pub fn compose(&self, child: &Transform) -> Transform {
        Transform {
            position: self.position + self.rotation.to_mat4().transform_dir(child.position * self.scale),
            rotation: self.rotation.mul_quat(&child.rotation).normalized(),
            scale: self.scale * child.scale,
        }
    }

    pub fn to_mat4(&self) -> Mat4 {
        let rot = self.rotation.to_mat4();
        let mut m = rot.mul_mat4(&Mat4::from_scale(self.scale));
        m.cols[3] = [self.position.x, self.position.y, self.position.z, 1.0];
        m
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn vec3_basics() {
        let a = Vec3::new(1.0, 0.0, 0.0);
        let b = Vec3::new(0.0, 1.0, 0.0);
        let c = a.cross(b);
        assert!(approx(c.x, 0.0) && approx(c.y, 0.0) && approx(c.z, 1.0));
        assert!(approx(a.dot(b), 0.0));
        assert!(approx(a.length(), 1.0));
        let n = Vec3::new(3.0, 4.0, 0.0).normalized();
        assert!(approx(n.length(), 1.0));
    }

    #[test]
    fn mat4_identity_is_noop() {
        let m = Mat4IDENTITY();
        let p = Vec3::new(1.0, 2.0, 3.0);
        let q = m.transform_point(p);
        assert!(approx(q.x, 1.0) && approx(q.y, 2.0) && approx(q.z, 3.0));
    }

    #[allow(non_snake_case)] // mirrors the associated const name
    fn Mat4IDENTITY() -> Mat4 { Mat4::IDENTITY }

    #[test]
    fn mat4_translation() {
        let m = Mat4::from_translation(Vec3::new(10.0, 20.0, 30.0));
        let p = m.transform_point(Vec3::new(1.0, 1.0, 1.0));
        assert!(approx(p.x, 11.0) && approx(p.y, 21.0) && approx(p.z, 31.0));
    }

    #[test]
    fn mat4_rotation_z_quarter_turn() {
        let m = Mat4::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let p = m.transform_point(Vec3::new(1.0, 0.0, 0.0));
        assert!(approx(p.x, 0.0));
        assert!(approx(p.y, 1.0));
    }

    #[test]
    fn mat4_compose_translation_then_scale() {
        let t = Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0));
        let s = Mat4::from_scale(2.0);
        let m = t.mul_mat4(&s); // scale first, then translate
        let p = m.transform_point(Vec3::new(1.0, 0.0, 0.0));
        assert!(approx(p.x, 7.0)); // (1*2)+5
    }

    #[test]
    fn perspective_maps_unit_cube() {
        let proj = Mat4::perspective_rh_zo(
            std::f32::consts::FRAC_PI_2, 1.0, 0.1, 100.0,
        );
        // A point on the near plane, straight ahead, maps to z=0.
        let near_pt = proj.transform_point4(Vec3::new(0.0, 0.0, -0.1));
        assert!(approx(near_pt.z / near_pt.w, 0.0));
        // A point on the far plane maps to z=1.
        let far_pt = proj.transform_point4(Vec3::new(0.0, 0.0, -100.0));
        assert!(approx(far_pt.z / far_pt.w, 1.0));
        // Points in front of the camera have w > 0.
        assert!(near_pt.w > 0.0 && far_pt.w > 0.0);
    }

    #[test]
    fn look_at_places_eye_at_origin() {
        let eye = Vec3::new(0.0, 0.0, 5.0);
        let view = Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::UP);
        let e = view.transform_point(eye);
        assert!(approx(e.x, 0.0) && approx(e.y, 0.0) && approx(e.z, 0.0));
        // A point 1 unit in front of the eye lands at z = -1 in view space.
        let ahead = view.transform_point(Vec3::new(0.0, 0.0, 4.0));
        assert!(approx(ahead.z, -1.0));
    }

    #[test]
    fn quat_rotation_matches_matrix() {
        let q = Quat::from_axis_angle(Vec3::UP, std::f32::consts::FRAC_PI_2);
        let m = q.to_mat4();
        let p = m.transform_point(Vec3::new(1.0, 0.0, 0.0));
        // Quarter turn about +Y: +X becomes -Z.
        assert!(approx(p.x, 0.0));
        assert!(approx(p.z, -1.0));
    }

    #[test]
    fn transform_compose() {
        let parent = Transform {
            position: Vec3::new(10.0, 0.0, 0.0),
            rotation: Quat::from_axis_angle(Vec3::UP, std::f32::consts::FRAC_PI_2),
            scale: 2.0,
        };
        let child = Transform::from_position(Vec3::new(1.0, 0.0, 0.0));
        let world = parent.compose(&child);
        // Child's +X becomes parent's -Z (rotated), scaled by 2, offset by 10 on X.
        assert!(approx(world.position.x, 10.0));
        assert!(approx(world.position.z, -2.0));
    }

    #[test]
    fn vec2_and_vec4() {
        let a = Vec2::new(3.0, 4.0);
        assert!(approx(a.length(), 5.0));
        let v4 = Vec4::from_vec3(Vec3::new(1.0, 2.0, 3.0), 1.0);
        assert!(approx(v4.w, 1.0));
        assert!(approx(v4.dot(v4), 15.0));
    }
}

//! world_map — CPU rendering of the REAL streamed-world placement
//! database, for the block-110 evidence take.
//!
//! ## Where the data comes from (all REAL)
//!
//! The placements are the `cells.rcf` GameObjects decoded by the
//! block-18/41 proven decoders in `prototype_p3d::world`
//! ([`prototype_p3d::world::scan_world_member_opts`]):
//! 81,370 corpus-wide, positions spanning the real map
//! `x[-1752,1708] y[-0.5,350] z[-2713,1658]` — the actual Prototype
//! 2009 streamed world, NOT synthetic points.  The block-110 evidence
//! take renders a 36-member archive sample (~19.6k placements) and
//! says so honestly on-screen.
//!
//! ## What this module does
//!
//! * [`MapView`] + [`paint_map_dots`] — a top-down orthographic
//!   projection (world x/z → screen, +z south/down like a street map)
//!   with 2×2 splats, colour-coded by object family.
//! * [`FlyCam`] + [`paint_flythrough`] — a 3D perspective flythrough
//!   over the same points: the proven `prototype_math` RH/ Zo camera
//!   (the same convention the wgpu path and the skinned-Alex CPU
//!   mirror use), depth-sorted far→near painter splats with a
//!   distance fade, near-plane cull.
//! * [`map_bounds`] — honest bounds for framing a dot set.
//!
//! ## Documented approximations (honest scope)
//!
//! * Splat rendering is a POINT representation of the world objects
//!   (no meshes) — real geometry members exist but their mesh decode +
//!   placement pairing is the next block's work.
//! * The map plane ignores y for the top-down view (y drives the dot
//!   brightness instead: high objects render lighter, so the skyline
//!   reads through).
//! * Depth sorting is per-dot (painter's algorithm), which is exact
//!   for non-overlapping splats and approximate where splats overlap.

#![forbid(unsafe_code)]

use prototype_math::{Mat4, Vec3};

// ---------------------------------------------------------------------------
// Dots
// ---------------------------------------------------------------------------

/// One rendered placement.  `kind` picks the palette entry; the four
/// families are the ones `prototype_p3d::world` decodes with real
/// placements (binary/prop-table GameObjects, LightDecalInfo,
/// AmbientExclusionVolume, WebNodeLocator).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapDot {
    /// World position (game units, metres-ish; the corpus-proven span
    /// is x[-1752,1708] y[-0.5,350] z[-2713,1658]).
    pub pos: [f32; 3],
    /// Object family (drives the palette colour).
    pub kind: DotKind,
}

/// Object family of a rendered placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DotKind {
    /// Plain placed GameObject (spawners, props, cars, …) — the
    /// 71,178-record binary + prop-table families.
    Spawn,
    /// `proto::LightDecalInfo` (streetlight/road decals).
    LightDecal,
    /// `proto::AmbientExclusionVolume` (audio volumes).
    AmbientExclusion,
    /// `proto::WebNodeLocator` (web-swing ground nodes).
    WebLocator,
}

impl DotKind {
    /// Palette (RGB): spawns white-blue, decals amber, exclusion
    /// volumes red, web locators green — distinguishable at 2×2 splats.
    pub fn colour(self) -> [u8; 3] {
        match self {
            DotKind::Spawn => [190, 210, 255],
            DotKind::LightDecal => [255, 190, 90],
            DotKind::AmbientExclusion => [255, 90, 90],
            DotKind::WebLocator => [110, 240, 130],
        }
    }
}

/// Honest bounds of a dot set: `(min, max)` per axis.  `None` for an
/// empty set.
pub fn map_bounds(dots: &[MapDot]) -> Option<([f32; 3], [f32; 3])> {
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for d in dots {
        for a in 0..3 {
            min[a] = min[a].min(d.pos[a]);
            max[a] = max[a].max(d.pos[a]);
        }
    }
    if dots.is_empty() {
        None
    } else {
        Some((min, max))
    }
}

// ---------------------------------------------------------------------------
// Top-down orthographic map
// ---------------------------------------------------------------------------

/// A top-down map view: world x/z → screen, `+z` renders downward
/// (south-down street-map orientation; the engine's z+ heads north the
/// same way the corpus bbox suggests: z[-2713,1658] spans the island).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapView {
    /// World (x, z) shown at the screen centre.
    pub center: [f32; 2],
    /// World units shown across the full screen width.
    pub span_x: f32,
}

impl MapView {
    /// Vertical scale matches horizontal (square world units).
    pub fn world_to_screen(&self, pos: [f32; 3], w: f32, h: f32) -> (f32, f32) {
        let scale = w / self.span_x;
        let sx = (pos[0] - self.center[0]) * scale + w * 0.5;
        let sy = (pos[2] - self.center[1]) * scale + h * 0.5;
        (sx, sy)
    }

    /// Zoomed view around a world point: `span_x_new = span_x / zoom`
    /// (zoom > 1 = closer).
    pub fn zoomed_toward(&self, world_center: [f32; 2], zoom: f32) -> MapView {
        MapView {
            center: world_center,
            span_x: (self.span_x / zoom.max(1e-6)),
        }
    }
}

/// Paint every dot as a 2×2 splat over an RGBA8 framebuffer (alpha
/// blend `over`).  Returns the number of dots that landed fully or
/// partially on-screen.
pub fn paint_map_dots(
    fb: &mut [u8],
    w: u32,
    h: u32,
    dots: &[MapDot],
    view: &MapView,
) -> usize {
    let (w, h) = (w as f32, h as f32);
    let mut landed = 0usize;
    for d in dots {
        let (sx, sy) = view.world_to_screen(d.pos, w, h);
        if splat2(fb, w as u32, h as u32, sx, sy, d.kind.colour(), y_shade(d.pos[1])) {
            landed += 1;
        }
    }
    landed
}

/// y-height brightness: y=0 → 0.72, y=350 → 1.0 (skyline reads
/// through; clamped, negative y darkens to 0.55 floor).
fn y_shade(y: f32) -> f32 {
    let t = ((y / 350.0).max(0.0)).min(1.0);
    0.72 + 0.28 * t
}

/// 2×2 splat with clip + `over` blend.  True if any pixel touched.
fn splat2(
    fb: &mut [u8],
    w: u32,
    h: u32,
    sx: f32,
    sy: f32,
    rgb: [u8; 3],
    brightness: f32,
) -> bool {
    if !(sx.is_finite() && sy.is_finite()) {
        return false;
    }
    let x0 = sx.floor() as i64;
    let y0 = sy.floor() as i64;
    let mut touched = false;
    for dy in 0..2i64 {
        for dx in 0..2i64 {
            let x = x0 + dx;
            let y = y0 + dy;
            if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
                continue;
            }
            blend_px(
                fb,
                (y * w as i64 + x) as usize * 4,
                rgb,
                brightness,
            );
            touched = true;
        }
    }
    touched
}

fn blend_px(fb: &mut [u8], idx: usize, rgb: [u8; 3], brightness: f32) {
    let b = brightness.clamp(0.0, 1.0);
    let sr = (rgb[0] as f32 * b).min(255.0);
    let sg = (rgb[1] as f32 * b).min(255.0);
    let sb = (rgb[2] as f32 * b).min(255.0);
    let (dr, dg, db) = (fb[idx] as f32, fb[idx + 1] as f32, fb[idx + 2] as f32);
    fb[idx] = (sr * 0.85 + dr * 0.15).round() as u8;
    fb[idx + 1] = (sg * 0.85 + dg * 0.15).round() as u8;
    fb[idx + 2] = (sb * 0.85 + db * 0.15).round() as u8;
    fb[idx + 3] = 255;
}

// ---------------------------------------------------------------------------
// 3D flythrough
// ---------------------------------------------------------------------------

/// The flythrough camera: eye + look-at + the proven RH-Zo perspective
/// (same convention as [`prototype_render::scene::Camera`]; rebuilt
/// here on `prototype_math` directly so the pure module stays
/// dependency-light).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlyCam {
    pub eye: [f32; 3],
    pub target: [f32; 3],
    /// Vertical field of view, degrees.
    pub fov_y_deg: f32,
    pub near: f32,
    pub far: f32,
    /// Screen aspect (w / h) — bake it into the projection.
    pub aspect: f32,
}

impl FlyCam {
    fn view_projection(&self) -> Mat4 {
        let eye = Vec3::new(self.eye[0], self.eye[1], self.eye[2]);
        let tgt = Vec3::new(self.target[0], self.target[1], self.target[2]);
        let up = Vec3::UP;
        let view = Mat4::look_at_rh(eye, tgt, up);
        let proj = Mat4::perspective_rh_zo(
            self.fov_y_deg.to_radians(),
            self.aspect,
            self.near,
            self.far,
        );
        proj.mul_mat4(&view)
    }

    /// Project a world point → `(sx, sy, view_depth)`.  `None` when
    /// the point is behind the near plane (culled).
    pub fn project(
        &self,
        pos: [f32; 3],
        w: u32,
        h: u32,
    ) -> Option<(f32, f32, f32)> {
        let m = self.view_projection();
        let x = pos[0];
        let y = pos[1];
        let z = pos[2];
        // out = M · v (column-major cols, v as column)
        let cx = m.cols[0][0] * x + m.cols[1][0] * y + m.cols[2][0] * z + m.cols[3][0];
        let cy = m.cols[0][1] * x + m.cols[1][1] * y + m.cols[2][1] * z + m.cols[3][1];
        let cw = m.cols[0][3] * x + m.cols[1][3] * y + m.cols[2][3] * z + m.cols[3][3];
        if cw <= 0.0 {
            return None;
        }
        let ndc_x = cx / cw;
        let ndc_y = cy / cw;
        if !ndc_x.is_finite() || !ndc_y.is_finite() {
            return None;
        }
        // view depth along -z (RH): the distance from the eye is what
        // the depth sort needs; derive it from the eye-point vector.
        let dx = pos[0] - self.eye[0];
        let dy = pos[1] - self.eye[1];
        let dz = pos[2] - self.eye[2];
        let depth = (dx * dx + dy * dy + dz * dz).sqrt();
        if depth < self.near {
            return None;
        }
        let sx = (ndc_x * 0.5 + 0.5) * (w as f32 - 1.0);
        let sy = (1.0 - (ndc_y * 0.5 + 0.5)) * (h as f32 - 1.0);
        Some((sx, sy, depth))
    }
}

/// Paint the flythrough: project every dot, cull behind the near
/// plane, depth-sort far→near (painter), splat with a distance fade
/// (far dots dimmer) and a 1-3 px size by proximity.  Returns the
/// number of dots drawn.
pub fn paint_flythrough(
    fb: &mut [u8],
    w: u32,
    h: u32,
    dots: &[MapDot],
    cam: &FlyCam,
) -> usize {
    let mut projected: Vec<(f32, f32, f32, DotKind)> = Vec::with_capacity(dots.len());
    for d in dots {
        if let Some((sx, sy, depth)) = cam.project(d.pos, w, h) {
            projected.push((sx, sy, depth, d.kind));
        }
    }
    // far → near
    projected.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    let mut drawn = 0usize;
    for (sx, sy, depth, kind) in projected {
        // fade: near=1.0, far=0.25 over [near, far]
        let t = ((depth - cam.near) / (cam.far - cam.near).max(1e-6)).clamp(0.0, 1.0);
        let brightness = 1.0 - 0.75 * t;
        // size: 3 px when very close, 2 mid, 1 far
        let px = if depth < 120.0 {
            3
        } else if depth < 600.0 {
            2
        } else {
            1
        };
        if splat_n(fb, w, h, sx, sy, px, kind.colour(), brightness) {
            drawn += 1;
        }
    }
    drawn
}

fn splat_n(
    fb: &mut [u8],
    w: u32,
    h: u32,
    sx: f32,
    sy: f32,
    n: i64,
    rgb: [u8; 3],
    brightness: f32,
) -> bool {
    if !(sx.is_finite() && sy.is_finite()) {
        return false;
    }
    // guard the extreme off-screen cases before the i64 cast (a
    // near-plane edge dot projects to huge finite coords)
    if sx < -1.0e6 || sx > (w as f32 + 1.0e6) || sy < -1.0e6 || sy > (h as f32 + 1.0e6) {
        return false;
    }
    let x0 = sx.floor() as i64 - (n / 2);
    let y0 = sy.floor() as i64 - (n / 2);
    let mut touched = false;
    for dy in 0..n {
        for dx in 0..n {
            let x = x0 + dx;
            let y = y0 + dy;
            if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
                continue;
            }
            blend_px(fb, (y * w as i64 + x) as usize * 4, rgb, brightness);
            touched = true;
        }
    }
    touched
}

// ---------------------------------------------------------------------------
// Camera paths (the evidence takes)
// ---------------------------------------------------------------------------

/// Zoom-out path: `t ∈ [0,1]` → a [`MapView`] zooming from `span_near`
/// to `span_far` around a fixed centre.
pub fn zoom_out_path(center: [f32; 2], span_near: f32, span_far: f32, t: f32) -> MapView {
    let tt = t.clamp(0.0, 1.0);
    MapView {
        center,
        span_x: span_near + (span_far - span_near) * tt,
    }
}

/// Pan path: `t ∈ [0,1]` → a [`MapView`] sliding the centre from `a`
/// to `b` at fixed span.
pub fn pan_path(a: [f32; 2], b: [f32; 2], span_x: f32, t: f32) -> MapView {
    let tt = t.clamp(0.0, 1.0);
    MapView {
        center: [a[0] + (b[0] - a[0]) * tt, a[1] + (b[1] - a[1]) * tt],
        span_x,
    }
}

/// Flythrough path: an arc over the map from `a` to `b` at height
/// `alt(t)` (lerped `alt_near→alt_far→alt_near`), looking ahead along
/// the path at a ground point `look_ahead` units forward.
pub fn fly_path(
    a: [f32; 2],
    b: [f32; 2],
    alt_a: f32,
    alt_b: f32,
    t: f32,
    aspect: f32,
) -> FlyCam {
    let tt = t.clamp(0.0, 1.0);
    let px = a[0] + (b[0] - a[0]) * tt;
    let pz = a[1] + (b[1] - a[1]) * tt;
    // altitude arc: rises to alt_b mid-flight
    let alt = alt_a + (alt_b - alt_a) * (tt * std::f32::consts::PI).sin();
    let dir = [b[0] - a[0], b[1] - a[1]];
    let len = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt().max(1e-6);
    let ux = dir[0] / len;
    let uz = dir[1] / len;
    let look = 220.0;
    FlyCam {
        eye: [px, alt, pz],
        target: [px + ux * look, 12.0, pz + uz * look],
        fov_y_deg: 60.0,
        near: 2.0,
        far: 4200.0,
        aspect,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn dot(x: f32, y: f32, z: f32, kind: DotKind) -> MapDot {
        MapDot { pos: [x, y, z], kind }
    }

    #[test]
    fn bounds_are_honest() {
        let dots = vec![
            dot(-100.0, 0.0, -200.0, DotKind::Spawn),
            dot(50.0, 30.0, 70.0, DotKind::LightDecal),
        ];
        let (min, max) = map_bounds(&dots).unwrap();
        assert_eq!(min, [-100.0, 0.0, -200.0]);
        assert_eq!(max, [50.0, 30.0, 70.0]);
        assert!(map_bounds(&[]).is_none());
    }

    #[test]
    fn map_view_projects_center_to_center() {
        let view = MapView { center: [0.0, 0.0], span_x: 4000.0 };
        let (w, h) = (640.0, 360.0);
        let (sx, sy) = view.world_to_screen([0.0, 50.0, 0.0], w, h);
        assert!((sx - w / 2.0).abs() < 1e-4);
        assert!((sy - h / 2.0).abs() < 1e-4);
        // +x → right, +z → DOWN (south-down map)
        let (sx2, sy2) = view.world_to_screen([100.0, 0.0, 200.0], w, h);
        assert!(sx2 > sx);
        assert!(sy2 > sy);
    }

    #[test]
    fn map_paint_lands_and_counts() {
        let (w, h) = (64u32, 64u32);
        let mut fb = vec![0u8; (w * h * 4) as usize];
        let dots = vec![
            dot(0.0, 0.0, 0.0, DotKind::Spawn),
            dot(50.0, 10.0, 50.0, DotKind::LightDecal),
            dot(1e6, 0.0, 1e6, DotKind::WebLocator), // off-screen
        ];
        let view = MapView { center: [25.0, 25.0], span_x: 100.0 };
        let landed = paint_map_dots(&mut fb, w, h, &dots, &view);
        assert_eq!(landed, 2, "two on-screen, one culled");
        // non-blank + the spawn colour landed somewhere (brightness
        // 0.72 dims the [190,210,255] palette to ~[116,127,156])
        let any = fb.chunks_exact(4).any(|p| p[2] > 140 && p[0] > 100);
        assert!(any, "spawn blue tint visible");
    }

    #[test]
    fn fly_cam_culls_behind_and_projects_ahead() {
        let cam = FlyCam {
            eye: [0.0, 100.0, 0.0],
            target: [0.0, 0.0, -1000.0],
            fov_y_deg: 60.0,
            near: 2.0,
            far: 4200.0,
            aspect: 16.0 / 9.0,
        };
        let (w, h) = (640u32, 360u32);
        // straight ahead → on-screen; the point ON THE LOOK AXIS must
        // land exactly at screen centre
        let dir = [
            cam.target[0] - cam.eye[0],
            cam.target[1] - cam.eye[1],
            cam.target[2] - cam.eye[2],
        ];
        let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        let axis = [
            cam.eye[0] + dir[0] / len * 500.0,
            cam.eye[1] + dir[1] / len * 500.0,
            cam.eye[2] + dir[2] / len * 500.0,
        ];
        let (sx, sy, depth) = cam.project(axis, w, h).expect("axis point projects");
        assert!((sx - (w as f32 - 1.0) / 2.0).abs() < 2.0, "centered x, got {sx}");
        assert!((sy - (h as f32 - 1.0) / 2.0).abs() < 2.0, "centered y, got {sy}");
        assert!((depth - 500.0).abs() < 1.0, "depth is eye distance, got {depth}");
        // a point level with the eye but BELOW the look axis lands below centre
        let (sx2, sy2, _) = cam.project([0.0, 100.0, -500.0], w, h).expect("eye-level point");
        assert!((sx2 - sx).abs() < 2.0);
        assert!(sy2 > sy, "eye-level point sits below the downward axis");
        // behind the eye → culled
        assert!(cam.project([0.0, 0.0, 500.0], w, h).is_none());
        // nearer than near plane → culled
        assert!(cam.project([0.0, 99.9, -0.5], w, h).is_none());
    }

    #[test]
    fn flythrough_paint_draws_and_sorts() {
        let (w, h) = (96u32, 64u32);
        let mut fb = vec![0u8; (w * h * 4) as usize];
        let dots = vec![
            dot(0.0, 0.0, -400.0, DotKind::Spawn),
            dot(0.0, 0.0, -800.0, DotKind::WebLocator),
            dot(50.0, 0.0, -300.0, DotKind::AmbientExclusion),
        ];
        let cam = FlyCam {
            eye: [0.0, 80.0, 0.0],
            target: [0.0, 0.0, -600.0],
            fov_y_deg: 60.0,
            near: 2.0,
            far: 4200.0,
            aspect: 1.5,
        };
        let drawn = paint_flythrough(&mut fb, w, h, &dots, &cam);
        assert!(drawn >= 2, "at least the mid dots draw, got {drawn}");
        let nonblank = fb
            .chunks_exact(4)
            .filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 0)
            .count();
        assert!(nonblank > 0);
    }

    #[test]
    fn paths_move_monotonically() {
        let v0 = zoom_out_path([0.0, 0.0], 400.0, 4800.0, 0.0);
        let v1 = zoom_out_path([0.0, 0.0], 400.0, 4800.0, 1.0);
        assert!((v0.span_x - 400.0).abs() < 1e-5);
        assert!((v1.span_x - 4800.0).abs() < 1e-5);
        let p0 = pan_path([0.0, 0.0], [1000.0, -2000.0], 3000.0, 0.5);
        assert!((p0.center[0] - 500.0).abs() < 1e-5);
        assert!((p0.center[1] + 1000.0).abs() < 1e-5);
        let c0 = fly_path([0.0, 0.0], [1000.0, -2000.0], 90.0, 300.0, 0.5, 1.5);
        assert!(c0.eye[1] > 90.0, "mid-flight altitude arc rises, got {}", c0.eye[1]);
    }

    #[test]
    fn palette_families_are_distinct() {
        let ks = [
            DotKind::Spawn,
            DotKind::LightDecal,
            DotKind::AmbientExclusion,
            DotKind::WebLocator,
        ];
        for i in 0..ks.len() {
            for j in (i + 1)..ks.len() {
                assert_ne!(ks[i].colour(), ks[j].colour());
            }
        }
    }
}

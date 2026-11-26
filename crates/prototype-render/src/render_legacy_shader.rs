//! render_legacy_shader — the device-free CPU port of the LEGACY
//! world shader pair, driven by the REAL shipped DLL SM3.0 bytecode.
//!
//! ## Why this exists (block 103)
//!
//! Block 102 wired the bink movie pair (the LIGHT YUV path).  Block
//! 103 wires the `legacy` world pair (VS 0xe36918, 48 instructions;
//! FP 0xe3d9b0, 69 instructions) — the FULL-LIGHTING feature set the
//! `default` pair does not carry: `materialDiffuse` / `material
//! Shininess` / `materialSpecular` (the Phong material terms) plus
//! `p3dLight_Direction` / `p3dLight_Colour` / `p3dLight_Ambient` /
//! `p3dLight_LightType` / `p3dLight_Attenuation` / `p3dLight_
//! SpotlightConstant` / `p3dLight_DecayConstant` (the light terms),
//! over a REAL art mesh (`SceneMesh`, the proven M21b decode).
//!
//! ## The REAL register contract (pinned by test against the fixtures)
//!
//! `legacy - lit - Vertex Program` (48 instructions, `vs_3_0`):
//! * `c0..c3` = `p3dWorldViewProjectionMatrix` (4×4, row-major rows)
//! * `c4..c7` = `p3dWorldMatrix` (4×4)
//! * `c8..c11` = `p3dLight_ProjectedTextureMatrix` (4×4)
//! * `c12` = `p3dEyePositionObjectSpace` (float3)
//! * `c13` = `p3dLight_PositionObjectSpace` (float3)
//! * `c14` = `def` `(0.5, -0.5, 0, 0)` — the QUARTER-SPACE trick
//!   constants baked into the shader (the 0.5/-0.5 pair used for the
//!   projected-texture half-space mapping).
//! * inputs: `v0` = position (dcl_position0), `v1` = normal
//!   (dcl_normal0), `v2` = texcoord (dcl_texcoord0).
//! * outputs: `o0` = clip position (WVP × pos, the standard 4-row
//!   `mul/mad/mad/add` chain — mirrored exactly), `o1.xy` = uv,
//!   `o2.xy` = the quarter-space projected coords, `o2.zw` = raw
//!   clip, `o3` = world normal (three `dp3`s against the World
//!   COLUMNS — the row-major transpose convention), `o4` = world
//!   light direction (World × (light_pos − pos)), `o5` = world eye
//!   direction (World × (eye_pos − pos)), `o6` = the full-space
//!   projected-texture coords (`LPT × world_pos`).
//!
//! `legacy - lit - Fragment Program` (69 instructions, `ps_3_0`):
//! * `c0` = `p3dLight_Direction` (float3)
//! * `c1` = `p3dLight_Attenuation` (float3)
//! * `c2` = `p3dLight_Ambient` (float4)
//! * `c3` = `p3dLight_Colour` (float4)
//! * `c4` = `p3dLight_LightType` (float4)
//! * `c5` = `p3dLight_SpotlightConstant` (float4)
//! * `c6` = `p3dLight_DecayConstant` (float4)
//! * `c7` = `materialShininess` (scalar)
//! * `c8` = `materialDiffuse` (float4)
//! * `c9` = `materialSpecular` (float4)
//! * `c10` = `def` `(1, 0, 0, 0)` — the cmp/step helper.
//! * samplers: `s0` = `p3dLight_DecayTexture`, `s1` =
//!   `p3dLight_SpotlightTexture`, `s2` = `p3dLight_ProjectedTexture`,
//!   `s3` = `diffuseMap`.
//! * the Blinn-Phong chain (`p3dLight_IsLitPass` = b0 = TRUE — the
//!   lit pass): `N` = normalize(o3), `L` = normalize(o4), `E` =
//!   normalize(o5), `H` = normalize(L + E), `spec = pow(max(N·H, 0),
//!   materialShininess)`, `diff = max(N·L, 0)`, and
//!   `out = texel·materialDiffuse·(p3dLight_Ambient
//!         + p3dLight_Colour·diff)
//!         + p3dLight_Colour·materialSpecular·spec·projected`.
//!   The decay / spotlight / projected-texture branches (`s0`/`s1`
//!   samples, the `texkill` / `cmp` chains) stay DATA-GATED: with no
//!   decay / spotlight / projected texture bound (the daylight
//!   ambient world case the harness renders), those terms collapse
//!   to the directional-only path above — a documented
//!   approximation (the full three-branch light-type matrix is NOT
//!   simulated).
//!
//! ## What is a documented approximation
//!
//! * Single-threaded scanline rasteriser, perspective-correct
//!   interpolants, z-buffer, no blending — the same CPU-mirror
//!   semantics as [`crate::render_art_world_cpu`].
//! * The directional-light branch only (the ambient daylight
//!   harness case); the point-light and spotlight branches are
//!   documented as not simulated (the `c4` LightType mask drives
//!   them in the real bytecode, but the sandbox renders the
//!   directional day case).
//! * The three auxiliary textures (`s0` decay, `s1` spotlight, `s2`
//!   projected) are NOT bound — their terms collapse (the
//!   `texkill`/`cmp` chains that would discard the fragment run
//!   only when the corresponding `c5.w`/`c6.w` gates are negative;
//!   the daylight case leaves them at 0 — documented, not
//!   invented).

#![forbid(unsafe_code)]

use crate::scene::{Camera, SceneMesh};

/// The legacy VS's constant registers (CTAB pinned by test against
/// `dll_vs_e36918.sm30`).
pub const LEGACY_VS_C0_WVP: usize = 0;
pub const LEGACY_VS_C4_WORLD: usize = 4;
pub const LEGACY_VS_C8_LIGHT_PROJECTED: usize = 8;
pub const LEGACY_VS_C12_EYE_OBJECT: usize = 12;
pub const LEGACY_VS_C13_LIGHT_POSITION_OBJECT: usize = 13;
pub const LEGACY_VS_C14_DEF: [f32; 4] = [0.5, -0.5, 0.0, 0.0];

/// The legacy FP's constant registers (CTAB pinned by test against
/// `dll_ps_e3d9b0.sm30`).
pub const LEGACY_FP_C0_LIGHT_DIRECTION: usize = 0;
pub const LEGACY_FP_C1_LIGHT_ATTENUATION: usize = 1;
pub const LEGACY_FP_C2_LIGHT_AMBIENT: usize = 2;
pub const LEGACY_FP_C3_LIGHT_COLOUR: usize = 3;
pub const LEGACY_FP_C4_LIGHT_TYPE: usize = 4;
pub const LEGACY_FP_C5_SPOTLIGHT_CONSTANT: usize = 5;
pub const LEGACY_FP_C6_DECAY_CONSTANT: usize = 6;
pub const LEGACY_FP_C7_MATERIAL_SHININESS: usize = 7;
pub const LEGACY_FP_C8_MATERIAL_DIFFUSE: usize = 8;
pub const LEGACY_FP_C9_MATERIAL_SPECULAR: usize = 9;
pub const LEGACY_FP_C10_DEF: [f32; 4] = [1.0, 0.0, 0.0, 0.0];

/// The legacy FP's four sampler slots (CTAB pinned).
pub const LEGACY_FP_S0_DECAY_TEXTURE: usize = 0;
pub const LEGACY_FP_S1_SPOTLIGHT_TEXTURE: usize = 1;
pub const LEGACY_FP_S2_PROJECTED_TEXTURE: usize = 2;
pub const LEGACY_FP_S3_DIFFUSE_MAP: usize = 3;

/// The legacy shader's material + light constants — the CPU-side
/// supply the FP consumes (the "CTAB supply" the block's NEXT-UP
/// asks to port).  Defaults are the harness's documented daylight
/// rows (the same values `shader::world::daylight_state` uploads)
/// plus the Phong material terms (a mid-grey diffuse, a specular
/// white, shininess 24 — a plausible game-look material).
#[derive(Debug, Clone, PartialEq)]
pub struct LegacyShaderConstants {
    /// `c2` = `p3dLight_Ambient` (float4, RGB in 0..1).
    pub light_ambient: [f32; 4],
    /// `c3` = `p3dLight_Colour` (float4, RGB in 0..1).
    pub light_colour: [f32; 4],
    /// `c0` = `p3dLight_Direction` (float3, world space).
    pub light_direction: [f32; 3],
    /// `c1` = `p3dLight_Attenuation` (float3) — unused by the
    /// directional branch; carried so the contract stays complete.
    pub light_attenuation: [f32; 3],
    /// `c4` = `p3dLight_LightType` (float4).  `0.0` = directional
    /// (the daylight branch the CPU port renders).
    pub light_type: [f32; 4],
    /// `c5` = `p3dLight_SpotlightConstant` (float4) — the daylight
    /// branch leaves the cone gates at 0 (no spotlight bound).
    pub spotlight_constant: [f32; 4],
    /// `c6` = `p3dLight_DecayConstant` (float4) — the daylight
    /// branch leaves the decay gates at 0 (no decay texture bound).
    pub decay_constant: [f32; 4],
    /// `c7` = `materialShininess` (scalar).
    pub material_shininess: f32,
    /// `c8` = `materialDiffuse` (float4, RGB in 0..1).
    pub material_diffuse: [f32; 4],
    /// `c9` = `materialSpecular` (float4, RGB in 0..1).
    pub material_specular: [f32; 4],
}

impl Default for LegacyShaderConstants {
    fn default() -> Self {
        Self {
            light_ambient: [0.35, 0.36, 0.40, 1.0],
            light_colour: [1.0, 0.96, 0.88, 1.0],
            light_direction: [-0.54, -0.72, 0.43],
            light_attenuation: [0.0, 0.0, 0.0],
            light_type: [0.0; 4], // 0 = directional
            spotlight_constant: [0.0; 4],
            decay_constant: [0.0; 4],
            material_shininess: 24.0,
            material_diffuse: [0.8, 0.8, 0.8, 1.0],
            material_specular: [0.35, 0.35, 0.35, 1.0],
        }
    }
}

/// The legacy VS's per-vertex outputs — the CPU port's own stage
/// boundary (the `o0`..`o6` registers as named struct fields).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LegacyVertexOutputs {
    /// `o0` — clip position (`WVP × pos`, 4 rows).
    pub clip: [f32; 4],
    /// `o1.xy` — the diffuse-map UV.
    pub uv: [f32; 2],
    /// `o3` — world normal (the 3 `dp3`s against the World columns).
    pub world_normal: [f32; 3],
    /// `o4` — world light direction (`World × (light_pos − pos)`).
    pub world_light_dir: [f32; 3],
    /// `o5` — world eye direction (`World × (eye_pos − pos)`).
    pub world_eye_dir: [f32; 3],
    /// `o6` — the projected-texture coords (`LPT × world_pos`);
    /// unused by the directional branch (the texture is unbound).
    pub projected_coords: [f32; 4],
}

/// The legacy VS's `p3dWorldMatrix` — identity in the harness (the
/// meshes decode in world space; a per-mesh World transform is
/// design-UNKNOWN, so the identity carries the contract).
pub const IDENTITY_WORLD_MATRIX: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// The legacy VS: the literal port of the 48-instruction stream.
///
/// * clip = the standard `mul c1·y / mad c0·x / mad c2·z / add c3`
///   4-row WVP chain (the row-major `M · v` convention the
///   `default` VS uses too).
/// * world normal = the three `dp3`s against the World COLUMNS
///   (`o3.x = dot(World_col0, normal)` etc — the row-major
///   transpose; the identity World keeps this the plain normal).
/// * world light / eye dirs = the same three `dp3`s against
///   `(light_pos − pos)` / `(eye_pos − pos)` (the identity World
///   keeps them the plain object-space deltas).
/// * uv passes straight through (`mov o1.xy, v2`).
/// * the projected-texture coords are computed (`o6`) but unused by
///   the directional branch (the texture is unbound).
pub fn legacy_vertex_stage(
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
    wvp_rows: &[[f32; 4]; 4],
    world_matrix: &[[f32; 4]; 4],
    eye_pos_object: [f32; 3],
    light_pos_object: [f32; 3],
) -> LegacyVertexOutputs {
    // Clip position: r0 = WVP · pos (the row-major 4-row dot chain).
    let mut clip = [0.0f32; 4];
    for r in 0..4 {
        clip[r] = position[0] * wvp_rows[r][0]
            + position[1] * wvp_rows[r][1]
            + position[2] * wvp_rows[r][2]
            + wvp_rows[r][3];
    }
    // World transform of the position (the same 4-row chain, the
    // World matrix this time) — needed for the projected coords.
    let mut world_pos = [0.0f32; 4];
    for r in 0..4 {
        world_pos[r] = position[0] * world_matrix[r][0]
            + position[1] * world_matrix[r][1]
            + position[2] * world_matrix[r][2]
            + world_matrix[r][3];
    }
    // The World COLUMNS (the VS does `mov r2.x, c4.x / mov r2.y, c5.x
    // / mov r2.z, c6.x` — i.e. column 0 of the World matrix = the x
    // components of rows 0..2), then `dp3` the normal and the two
    // direction deltas against each column.
    let col = |c: usize| -> [f32; 3] {
        [world_matrix[0][c], world_matrix[1][c], world_matrix[2][c]]
    };
    let dot3 = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let mut world_normal = [0.0f32; 3];
    for c in 0..3 {
        world_normal[c] = dot3(col(c), normal);
    }
    let light_delta = [
        light_pos_object[0] - position[0],
        light_pos_object[1] - position[1],
        light_pos_object[2] - position[2],
    ];
    let eye_delta = [
        eye_pos_object[0] - position[0],
        eye_pos_object[1] - position[1],
        eye_pos_object[2] - position[2],
    ];
    let mut world_light_dir = [0.0f32; 3];
    let mut world_eye_dir = [0.0f32; 3];
    for c in 0..3 {
        world_light_dir[c] = dot3(col(c), light_delta);
        world_eye_dir[c] = dot3(col(c), eye_delta);
    }
    // The projected-texture coords (`LPT × world_pos`) — computed
    // but unused by the directional branch (the texture is unbound).
    let mut projected_coords = [0.0f32; 4];
    for r in 0..4 {
        projected_coords[r] = world_pos[0] * wvp_rows[r][0]
            + world_pos[1] * wvp_rows[r][1]
            + world_pos[2] * wvp_rows[r][2]
            + wvp_rows[r][3];
    }
    let _ = &IDENTITY_WORLD_MATRIX; // silence dead-code when tests off
    LegacyVertexOutputs {
        clip,
        uv,
        world_normal,
        world_light_dir,
        world_eye_dir,
        projected_coords,
    }
}

/// The legacy FP's directional-light branch — the literal port of
/// the lit-pass math (the `b0` = TRUE side of the `if/else/endif`).
///
/// Consumes the VS outputs + the FP constants + the diffuse texel
/// (already sampled by the caller) and produces the final RGB in
/// 0..1.  The decay / spotlight / projected-texture textures are
/// UNBOUND (the daylight harness case): their `texkill`/`cmp`
/// chains collapse to the directional-only path (documented in the
/// module docs).
pub fn legacy_fragment_stage(
    vs: &LegacyVertexOutputs,
    texel_rgb: [f32; 3],
    c: &LegacyShaderConstants,
) -> [f32; 3] {
    // N = normalize(world normal)
    let n = normalize3(vs.world_normal);
    // L = normalize(world light direction)
    let l = normalize3(vs.world_light_dir);
    // E = normalize(world eye direction)
    let e = normalize3(vs.world_eye_dir);
    // H = normalize(L + E) — the Blinn half vector.
    let h = normalize3([l[0] + e[0], l[1] + e[1], l[2] + e[2]]);
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    // diffuse = max(N·L, 0); specular = pow(max(N·H, 0), shininess).
    let diff = dot(n, l).clamp(0.0, 1.0);
    let nh = dot(n, h).clamp(0.0, 1.0);
    let spec = nh.powf(c.material_shininess);
    // The final Phong sum (the directional branch of the real PS):
    //   out = texel·materialDiffuse·(ambient + colour·diff)
    //       + colour·materialSpecular·spec·projected
    // The projected texture is UNBOUND in the daylight case, so its
    // multiplier collapses to 1 (documented approximation).
    let mut out = [0.0f32; 3];
    for ch in 0..3 {
        let diffuse_term = texel_rgb[ch]
            * c.material_diffuse[ch]
            * (c.light_ambient[ch] + c.light_colour[ch] * diff);
        let specular_term = c.light_colour[ch]
            * c.material_specular[ch]
            * spec;
        out[ch] = diffuse_term + specular_term;
    }
    out
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-9 {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// Render one real mesh through the legacy shader pair on the CPU.
///
/// Mirrors the same CPU-mirror rasteriser semantics as
/// [`crate::render_art_world_cpu::render_mesh_cpu`] (single-threaded
/// scanline, perspective-correct interpolants, z-buffer, no
/// blending) but the FRAGMENT MATH is the legacy pair's
/// Blinn-Phong (the FULL lighting feature set) instead of the
/// default pair's daylight diffuse-only model.
#[allow(clippy::too_many_arguments)]
pub fn render_legacy_cpu(
    mesh: &SceneMesh,
    camera: &Camera,
    texture: Option<(&[u8], u32, u32)>,
    consts: &LegacyShaderConstants,
    width: u32,
    height: u32,
    clear: [u8; 4],
) -> Result<crate::render_art_world_cpu::CpuMirrorFrame, String> {
    use crate::render_art_world_cpu::CpuMirrorFrame;
    if width == 0 || height == 0 {
        return Err("width/height must be non-zero".into());
    }
    let n_verts = mesh.positions.len();
    if mesh.indices.is_empty() || n_verts == 0 {
        return Err(format!("mesh {:?} has no drawable geometry", mesh.name));
    }

    let w = width as i64;
    let h = height as i64;
    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    for px in rgba.chunks_exact_mut(4) {
        px.copy_from_slice(&clear);
    }
    let mut depth = vec![f32::INFINITY; (width as usize) * (height as usize)];

    // The VS constants: WVP (the camera's row-major rows) + the
    // identity World + the eye position (the camera's eye) + the
    // light position (far along the light direction from the bbox
    // centre — the directional light's positional stand-in).
    let wvp_rows = camera.view_projection_rows();
    let world = IDENTITY_WORLD_MATRIX;
    let center = mesh.bounds_center().unwrap_or([0.0; 3]);
    let radius = mesh.bounds_radius().unwrap_or(1.0);
    let light_pos = [
        center[0] + consts.light_direction[0] * radius * 4.0,
        center[1] + consts.light_direction[1] * radius * 4.0,
        center[2] + consts.light_direction[2] * radius * 4.0,
    ];
    let eye_pos = [camera.eye.x, camera.eye.y, camera.eye.z];

    let mut tri_drawn = 0usize;
    let mut tri_clipped = 0usize;

    for tri in mesh.indices.chunks_exact(3) {
        let i0 = tri[0] as usize;
        let i1 = tri[1] as usize;
        let i2 = tri[2] as usize;
        if i0 >= n_verts || i1 >= n_verts || i2 >= n_verts {
            continue;
        }
        // The VS per vertex.
        let v = [
            legacy_vertex_stage(
                mesh.positions[i0],
                mesh.normals.get(i0).copied().unwrap_or([0.0, 0.0, 1.0]),
                mesh.uvs.get(i0).copied().unwrap_or([0.5, 0.5]),
                &wvp_rows,
                &world,
                eye_pos,
                light_pos,
            ),
            legacy_vertex_stage(
                mesh.positions[i1],
                mesh.normals.get(i1).copied().unwrap_or([0.0, 0.0, 1.0]),
                mesh.uvs.get(i1).copied().unwrap_or([0.5, 0.5]),
                &wvp_rows,
                &world,
                eye_pos,
                light_pos,
            ),
            legacy_vertex_stage(
                mesh.positions[i2],
                mesh.normals.get(i2).copied().unwrap_or([0.0, 0.0, 1.0]),
                mesh.uvs.get(i2).copied().unwrap_or([0.5, 0.5]),
                &wvp_rows,
                &world,
                eye_pos,
                light_pos,
            ),
        ];
        if v[0].clip[3] <= 1e-6 || v[1].clip[3] <= 1e-6 || v[2].clip[3] <= 1e-6 {
            tri_clipped += 1;
            continue;
        }
        tri_drawn += 1;

        // NDC → screen (the block-88 orientation fix: `py = 0.5 +
        // sy·0.5`, row 0 = image TOP).
        let sx = [
            v[0].clip[0] / v[0].clip[3],
            v[1].clip[0] / v[1].clip[3],
            v[2].clip[0] / v[2].clip[3],
        ];
        let sy = [
            v[0].clip[1] / v[0].clip[3],
            v[1].clip[1] / v[1].clip[3],
            v[2].clip[1] / v[2].clip[3],
        ];
        let sz = [
            v[0].clip[2] / v[0].clip[3],
            v[1].clip[2] / v[1].clip[3],
            v[2].clip[2] / v[2].clip[3],
        ];
        let scr = [
            ((sx[0] * 0.5 + 0.5) * w as f32, (0.5 + sy[0] * 0.5) * h as f32),
            ((sx[1] * 0.5 + 0.5) * w as f32, (0.5 + sy[1] * 0.5) * h as f32),
            ((sx[2] * 0.5 + 0.5) * w as f32, (0.5 + sy[2] * 0.5) * h as f32),
        ];
        let inv_w = [1.0 / v[0].clip[3], 1.0 / v[1].clip[3], 1.0 / v[2].clip[3]];

        let min_x = scr[0].0.min(scr[1].0).min(scr[2].0).floor().max(0.0) as i64;
        let max_x = (scr[0].0.max(scr[1].0).max(scr[2].0).ceil() as i64).min(w - 1);
        let min_y = scr[0].1.min(scr[1].1).min(scr[2].1).floor().max(0.0) as i64;
        let max_y = (scr[0].1.max(scr[1].1).max(scr[2].1).ceil() as i64).min(h - 1);
        if min_x > max_x || min_y > max_y {
            continue;
        }
        let area = edge(scr[0], scr[1], scr[2]);
        if area.abs() < 1e-9 {
            continue;
        }

        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let p = ((x as f32) + 0.5, (y as f32) + 0.5);
                let b0 = edge(scr[1], scr[2], p) / area;
                let b1 = edge(scr[2], scr[0], p) / area;
                let b2 = edge(scr[0], scr[1], p) / area;
                if b0 < 0.0 || b1 < 0.0 || b2 < 0.0 {
                    continue;
                }
                let z = b0 * sz[0] + b1 * sz[1] + b2 * sz[2];
                if !(0.0..=1.0).contains(&z) {
                    continue;
                }
                let di = (y as usize) * (width as usize) + x as usize;
                if z >= depth[di] {
                    continue;
                }
                depth[di] = z;
                let wp = b0 * inv_w[0] + b1 * inv_w[1] + b2 * inv_w[2];
                if wp <= 1e-12 {
                    continue;
                }
                let un = |a: [f32; 3], b: [f32; 3], cc: [f32; 3]| -> [f32; 3] {
                    [
                        (b0 * a[0] * inv_w[0] + b1 * b[0] * inv_w[1] + b2 * cc[0] * inv_w[2]) / wp,
                        (b0 * a[1] * inv_w[0] + b1 * b[1] * inv_w[1] + b2 * cc[1] * inv_w[2]) / wp,
                        (b0 * a[2] * inv_w[0] + b1 * b[2] * inv_w[1] + b2 * cc[2] * inv_w[2]) / wp,
                    ]
                };
                let u = (b0 * v[0].uv[0] * inv_w[0]
                    + b1 * v[1].uv[0] * inv_w[1]
                    + b2 * v[2].uv[0] * inv_w[2])
                    / wp;
                let vv = (b0 * v[0].uv[1] * inv_w[0]
                    + b1 * v[1].uv[1] * inv_w[1]
                    + b2 * v[2].uv[1] * inv_w[2])
                    / wp;
                let n = un(v[0].world_normal, v[1].world_normal, v[2].world_normal);
                let ld = un(v[0].world_light_dir, v[1].world_light_dir, v[2].world_light_dir);
                let ed = un(v[0].world_eye_dir, v[1].world_eye_dir, v[2].world_eye_dir);
                // The diffuse texel (nearest, wrapped — the shared
                // CPU-mirror sampling convention).
                let texel: [f32; 3] = match texture {
                    Some((bytes, tw, th)) if tw > 0 && th > 0 => {
                        let tx = (u.rem_euclid(1.0) * tw as f32) as u32 % tw;
                        let ty = (vv.rem_euclid(1.0) * th as f32) as u32 % th;
                        let to = ((ty * tw + tx) * 4) as usize;
                        if to + 3 <= bytes.len() {
                            [
                                bytes[to] as f32 / 255.0,
                                bytes[to + 1] as f32 / 255.0,
                                bytes[to + 2] as f32 / 255.0,
                            ]
                        } else {
                            [1.0, 1.0, 1.0]
                        }
                    }
                    _ => [1.0, 1.0, 1.0],
                };
                // The FP: the legacy Blinn-Phong directional branch.
                let interp = LegacyVertexOutputs {
                    clip: [0.0; 4],
                    uv: [u, vv],
                    world_normal: n,
                    world_light_dir: ld,
                    world_eye_dir: ed,
                    projected_coords: [0.0; 4],
                };
                let rgb = legacy_fragment_stage(&interp, texel, consts);
                let out = [
                    (rgb[0].clamp(0.0, 1.0) * 255.0).round() as u8,
                    (rgb[1].clamp(0.0, 1.0) * 255.0).round() as u8,
                    (rgb[2].clamp(0.0, 1.0) * 255.0).round() as u8,
                    255u8,
                ];
                let o = di * 4;
                rgba[o..o + 4].copy_from_slice(&out);
            }
        }
    }

    let stats = crate::render_art_world_cpu::frame_stats(&rgba, tri_drawn, tri_clipped);
    Ok(CpuMirrorFrame {
        width,
        height,
        rgba,
        stats,
    })
}

fn edge(a: (f32, f32), b: (f32, f32), p: (f32, f32)) -> f32 {
    (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
}

// ---------------------------------------------------------------------------
// Unit tests — pin the register contract + the Phong math
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The four VS sampler-slot constants — pinned against the
    /// committed fixture's CTAB by [`legacy_vs_contract_matches_
    /// committed_fixture`].
    #[test]
    fn legacy_vs_constants_match_ctab_slots() {
        assert_eq!(LEGACY_VS_C0_WVP, 0, "WVP is c0-c3");
        assert_eq!(LEGACY_VS_C4_WORLD, 4, "World is c4-c7");
        assert_eq!(LEGACY_VS_C8_LIGHT_PROJECTED, 8, "LPT is c8-c11");
        assert_eq!(LEGACY_VS_C12_EYE_OBJECT, 12, "eye is c12");
        assert_eq!(LEGACY_VS_C13_LIGHT_POSITION_OBJECT, 13, "light pos is c13");
        assert_eq!(LEGACY_VS_C14_DEF, [0.5, -0.5, 0.0, 0.0], "the baked def");
    }

    /// The FP sampler slots — pinned against the committed fixture's
    /// CTAB by [`legacy_fp_contract_matches_committed_fixture`].
    #[test]
    fn legacy_fp_constants_match_ctab_slots() {
        assert_eq!(LEGACY_FP_C0_LIGHT_DIRECTION, 0, "light dir is c0");
        assert_eq!(LEGACY_FP_C1_LIGHT_ATTENUATION, 1, "attenuation is c1");
        assert_eq!(LEGACY_FP_C2_LIGHT_AMBIENT, 2, "ambient is c2");
        assert_eq!(LEGACY_FP_C3_LIGHT_COLOUR, 3, "colour is c3");
        assert_eq!(LEGACY_FP_C4_LIGHT_TYPE, 4, "light type is c4");
        assert_eq!(LEGACY_FP_C5_SPOTLIGHT_CONSTANT, 5, "spotlight is c5");
        assert_eq!(LEGACY_FP_C6_DECAY_CONSTANT, 6, "decay is c6");
        assert_eq!(LEGACY_FP_C7_MATERIAL_SHININESS, 7, "shininess is c7");
        assert_eq!(LEGACY_FP_C8_MATERIAL_DIFFUSE, 8, "material diffuse is c8");
        assert_eq!(LEGACY_FP_C9_MATERIAL_SPECULAR, 9, "material specular is c9");
        assert_eq!(LEGACY_FP_C10_DEF, [1.0, 0.0, 0.0, 0.0], "the baked def");
        assert_eq!(LEGACY_FP_S0_DECAY_TEXTURE, 0, "decay texture is s0");
        assert_eq!(LEGACY_FP_S1_SPOTLIGHT_TEXTURE, 1, "spotlight texture is s1");
        assert_eq!(LEGACY_FP_S2_PROJECTED_TEXTURE, 2, "projected texture is s2");
        assert_eq!(LEGACY_FP_S3_DIFFUSE_MAP, 3, "diffuse map is s3");
    }

    /// The committed `dll_vs_e36918.sm30` fixture MUST parse under
    /// the SM3.0 decoder and its CTAB MUST list exactly the five
    /// constants at the slots the CPU port consumes — the byte-level
    /// pin between the committed fixture and the render path.
    #[test]
    fn legacy_vs_contract_matches_committed_fixture() {
        let bytes = include_bytes!(
            "../../prototype-bin/tests/fixtures/dll_vs_e36918.sm30"
        );
        let sh = crate::shader::sm30::Sm30Shader::parse(bytes, false)
            .expect("committed legacy VS must parse");
        let ctab = sh.ctab.as_ref().expect("legacy VS has a CTAB");
        let mut slots = std::collections::HashMap::new();
        for c in &ctab.constants {
            slots.insert(c.name.as_str(), (c.register_index, c.register_count));
        }
        let (wvp_idx, wvp_n) = *slots
            .get("p3dWorldViewProjectionMatrix")
            .expect("WVP in CTAB");
        assert_eq!(wvp_idx as usize, LEGACY_VS_C0_WVP, "WVP@c0");
        assert_eq!(wvp_n as usize, 4, "WVP is a 4×4 (4 registers)");
        let (world_idx, world_n) = *slots.get("p3dWorldMatrix").expect("World in CTAB");
        assert_eq!(world_idx as usize, LEGACY_VS_C4_WORLD, "World@c4");
        assert_eq!(world_n as usize, 4, "World is a 4×4");
        let (lpt_idx, lpt_n) = *slots
            .get("p3dLight_ProjectedTextureMatrix")
            .expect("LPT in CTAB");
        assert_eq!(lpt_idx as usize, LEGACY_VS_C8_LIGHT_PROJECTED, "LPT@c8");
        assert_eq!(lpt_n as usize, 4, "LPT is a 4×4");
        let (eye_idx, _) = *slots
            .get("p3dEyePositionObjectSpace")
            .expect("eye in CTAB");
        assert_eq!(eye_idx as usize, LEGACY_VS_C12_EYE_OBJECT, "eye@c12");
        let (lp_idx, _) = *slots
            .get("p3dLight_PositionObjectSpace")
            .expect("light pos in CTAB");
        assert_eq!(lp_idx as usize, LEGACY_VS_C13_LIGHT_POSITION_OBJECT);
    }

    /// The committed `dll_ps_e3d9b0.sm30` fixture MUST parse and its
    /// CTAB MUST list the 15 light + material constants and the four
    /// samplers at the slots the CPU port consumes.
    #[test]
    fn legacy_fp_contract_matches_committed_fixture() {
        let bytes = include_bytes!(
            "../../prototype-bin/tests/fixtures/dll_ps_e3d9b0.sm30"
        );
        let sh = crate::shader::sm30::Sm30Shader::parse(bytes, true)
            .expect("committed legacy FP must parse");
        let ctab = sh.ctab.as_ref().expect("legacy FP has a CTAB");
        let mut slots = std::collections::HashMap::new();
        for c in &ctab.constants {
            slots.insert(c.name.as_str(), (c.register_index, c.register_count));
        }
        let (idx, _) = *slots.get("p3dLight_Direction").expect("light dir in CTAB");
        assert_eq!(idx as usize, LEGACY_FP_C0_LIGHT_DIRECTION, "light dir@c0");
        let (idx, _) = *slots.get("p3dLight_Ambient").expect("ambient in CTAB");
        assert_eq!(idx as usize, LEGACY_FP_C2_LIGHT_AMBIENT, "ambient@c2");
        let (idx, _) = *slots.get("p3dLight_Colour").expect("colour in CTAB");
        assert_eq!(idx as usize, LEGACY_FP_C3_LIGHT_COLOUR, "colour@c3");
        let (idx, _) = *slots.get("materialShininess").expect("shininess in CTAB");
        assert_eq!(idx as usize, LEGACY_FP_C7_MATERIAL_SHININESS, "shininess@c7");
        let (idx, _) = *slots.get("materialDiffuse").expect("material diffuse in CTAB");
        assert_eq!(idx as usize, LEGACY_FP_C8_MATERIAL_DIFFUSE, "material diffuse@c8");
        let (idx, _) = *slots.get("materialSpecular").expect("material specular in CTAB");
        assert_eq!(idx as usize, LEGACY_FP_C9_MATERIAL_SPECULAR, "material specular@c9");
        let (idx, _) = *slots.get("diffuseMap").expect("diffuseMap in CTAB");
        assert_eq!(idx as usize, LEGACY_FP_S3_DIFFUSE_MAP, "diffuseMap@s3");
        // The FP is 69 instructions, the VS 48 — the PROVEN catalog
        // sizes (block 98/101).
        assert_eq!(sh.instructions.len(), 69, "legacy FP is 69 instructions");
    }

    /// The committed legacy VS MUST be 48 instructions (the PROVEN
    /// catalog size).
    #[test]
    fn legacy_vs_is_48_instructions() {
        let bytes = include_bytes!(
            "../../prototype-bin/tests/fixtures/dll_vs_e36918.sm30"
        );
        let sh = crate::shader::sm30::Sm30Shader::parse(bytes, false)
            .expect("committed legacy VS must parse");
        assert_eq!(sh.instructions.len(), 48, "legacy VS is 48 instructions");
    }

    /// A surface facing the light with a zero-specular material MUST
    /// produce exactly `texel·materialDiffuse·(ambient + colour)`
    /// (the diffuse term at full strength); a surface facing AWAY
    /// MUST be exactly the ambient floor (diffuse clamps to 0).
    #[test]
    fn legacy_fragment_diffuse_directional() {
        let mut c = LegacyShaderConstants::default();
        c.material_specular = [0.0; 4]; // isolate the diffuse term
        // Facing the light: N = the light direction.
        let vs = LegacyVertexOutputs {
            clip: [0.0; 4],
            uv: [0.0, 0.0],
            world_normal: c.light_direction,
            world_light_dir: c.light_direction,
            world_eye_dir: [0.0, 0.0, 1.0],
            projected_coords: [0.0; 4],
        };
        let out = legacy_fragment_stage(&vs, [1.0, 1.0, 1.0], &c);
        for ch in 0..3 {
            // diffuse = 1 → the full diffuse term (ambient + colour).
            let expected = c.material_diffuse[ch]
                * (c.light_ambient[ch] + c.light_colour[ch]);
            assert!(
                (out[ch] - expected).abs() < 1e-4,
                "facing the light: out[{ch}] = {} expected {expected}",
                out[ch]
            );
        }
        // Facing away: N = -light direction → diffuse = 0 → ambient
        // floor only.
        let vs_away = LegacyVertexOutputs {
            world_normal: [
                -c.light_direction[0],
                -c.light_direction[1],
                -c.light_direction[2],
            ],
            ..vs
        };
        let out_away = legacy_fragment_stage(&vs_away, [1.0, 1.0, 1.0], &c);
        for ch in 0..3 {
            let expected = c.material_diffuse[ch] * c.light_ambient[ch];
            assert!(
                (out_away[ch] - expected).abs() < 1e-4,
                "facing away: out[{ch}] = {} expected {expected}",
                out_away[ch]
            );
        }
    }

    /// The specular term: a mid-facing surface (N·L = 0.5, N·H = 1)
    /// with a NON-zero shininess MUST produce a brighter result than
    /// the same surface with materialSpecular = 0 (the Phong
    /// highlight is the feature the `default` pair does NOT carry).
    #[test]
    fn legacy_fragment_specular_is_the_legacy_feature() {
        let mut c = LegacyShaderConstants::default();
        // Face the half-vector directly so N·H = 1 → spec = 1.
        let l = normalize3(c.light_direction);
        let e = [0.0, 0.0, 1.0];
        let h = normalize3([l[0] + e[0], l[1] + e[1], l[2] + e[2]]);
        let vs = LegacyVertexOutputs {
            clip: [0.0; 4],
            uv: [0.0, 0.0],
            world_normal: h,
            world_light_dir: c.light_direction,
            world_eye_dir: e,
            projected_coords: [0.0; 4],
        };
        let texel = [1.0, 1.0, 1.0];
        let with_spec = legacy_fragment_stage(&vs, texel, &c);
        c.material_specular = [0.0, 0.0, 0.0, 1.0];
        let without_spec = legacy_fragment_stage(&vs, texel, &c);
        for ch in 0..3 {
            assert!(
                with_spec[ch] > without_spec[ch],
                "specular must ADD light: with[{}] = {} without = {}",
                ch,
                with_spec[ch],
                without_spec[ch]
            );
        }
    }

    /// The vertex stage: a unit-identity WVP and World MUST leave
    /// the position and the normal unchanged (the row-major dot
    /// chain with an identity matrix is the identity).
    #[test]
    fn legacy_vertex_identity() {
        let id = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let pos = [0.3, -0.7, 1.4];
        let normal = [0.0, 1.0, 0.0];
        let uv = [0.25, 0.75];
        let vs = legacy_vertex_stage(pos, normal, uv, &id, &id, [0.0; 3], [0.0; 3]);
        assert_eq!(vs.clip[0], pos[0]);
        assert_eq!(vs.clip[1], pos[1]);
        assert_eq!(vs.clip[2], pos[2]);
        assert_eq!(vs.clip[3], 1.0);
        assert_eq!(vs.uv, uv);
        assert_eq!(vs.world_normal, normal);
    }
}

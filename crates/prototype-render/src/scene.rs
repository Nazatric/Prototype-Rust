//! scene — a tiny Transform-based scene graph for the render path.
//!
//! Built on [`prototype_math`]'s `Transform` / `Mat4` / `Vec3` — the
//! M9 work surfaces a typed `EntityId`-bearing entity model in the
//! engine crate; this scene module is the rendering-side mirror: a
//! node hierarchy that yields world matrices for a renderer.
//!
//! ## PROVEN vs LIKELY
//!
//! * PROVEN: `prototype_math::Mat4::perspective_rh_zo` /
//!   `look_at_rh` produce correct clip-space transforms for the
//!   right-handed `z ∈ [0, 1]` depth convention (also the original
//!   D3D9 depth range — see the math crate's docs).
//! * LIKELY: the original Prototype engine used a similar
//!   scene-graph node hierarchy (the RTTI dump lists
//!   `engine::GetCurrentDrawable`, which implies a drawable tree);
//!   the exact node-type taxonomy is UNKNOWN.
//!
//! No decompiled code; this is a clean-room model.

#![forbid(unsafe_code)]

use prototype_math::{Mat4, Transform, Vec3};

// ---------------------------------------------------------------------------
// Camera
// ---------------------------------------------------------------------------

/// A first-person / look-at camera model.  Computes the view
/// matrix from eye / target / up; the projection matrix is
/// pre-built once (perspective right-handed, `z ∈ [0, 1]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub fov_y: f32,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            eye: Vec3::new(0.0, 0.0, 5.0),
            target: Vec3::ZERO,
            up: Vec3::UP,
            fov_y: std::f32::consts::FRAC_PI_2,
            aspect: 1.0,
            near: 0.1,
            far: 100.0,
        }
    }
}

impl Camera {
    /// Build the view matrix (world → view space).
    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye, self.target, self.up)
    }

    /// Build the projection matrix (view → clip space, `z ∈ [0, 1]`).
    pub fn projection(&self) -> Mat4 {
        Mat4::perspective_rh_zo(self.fov_y, self.aspect, self.near, self.far)
    }

    /// Combined view-projection matrix (world → clip).
    pub fn view_projection(&self) -> Mat4 {
        self.projection().mul_mat4(&self.view())
    }

    /// The view-projection as four D3D9 row-major rows — the exact shape
    /// the translated WGSL consumes (`p3dWorldViewProjectionMatrix_r0..r3`
    /// dotted with the input position, i.e. `out = M · v` with `_r{i}` =
    /// row `i` of M).  `Mat4` stores column-major `cols`, so row `i` of
    /// the product is the `i`-th component ACROSS the columns
    /// (`M.cols[j][i]`) — the transpose, written out explicitly.
    pub fn view_projection_rows(&self) -> [[f32; 4]; 4] {
        let m = self.view_projection();
        let mut rows = [[0.0f32; 4]; 4];
        for (i, row) in rows.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = m.cols[j][i];
            }
        }
        rows
    }
}

// ---------------------------------------------------------------------------
// SceneNode
// ---------------------------------------------------------------------------

/// One node in a render-scene graph: a local-space [`Transform`]
/// (relative to its parent) plus an optional mesh handle (just a
/// name string in this clean-room model — actual GPU mesh buffers
/// are out of scope for this crate).
#[derive(Debug, Clone)]
pub struct SceneNode {
    pub name: String,
    pub local: Transform,
    /// Optional mesh reference (a shader/material name from the
    /// shipped P3D, e.g. `"unlit"`).
    pub mesh: Option<String>,
    pub children: Vec<SceneNode>,
}

impl SceneNode {
    /// Empty leaf node with identity transform.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            local: Transform::default(),
            mesh: None,
            children: Vec::new(),
        }
    }

    /// Add a child node; returns the index of the new child.
    pub fn add_child(&mut self, child: SceneNode) -> usize {
        let i = self.children.len();
        self.children.push(child);
        i
    }

    /// Compute the WORLD transform of this node given a parent
    /// world transform.
    pub fn world(&self, parent_world: &Transform) -> Transform {
        parent_world.compose(&self.local)
    }

    /// Walk the subtree, calling `f(world, node)` for every node
    /// in pre-order.  Used by a renderer to gather draw calls.
    pub fn walk<F: FnMut(&Transform, &SceneNode)>(&self, parent_world: &Transform, f: &mut F) {
        let world = self.world(parent_world);
        f(&world, self);
        for child in &self.children {
            child.walk(&world, f);
        }
    }

    /// Count nodes in the subtree (including self).
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(SceneNode::count).sum::<usize>()
    }
}

// ---------------------------------------------------------------------------
// Scene
// ---------------------------------------------------------------------------

/// A render scene: a root transform + a root node + the active
/// camera.  Pure data; the renderer walks the scene to produce
/// draw calls.
#[derive(Debug, Clone)]
pub struct Scene {
    pub root: SceneNode,
    pub camera: Camera,
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            root: SceneNode::new("root"),
            camera: Camera::default(),
        }
    }
}

impl Scene {
    pub fn new() -> Self {
        Self::default()
    }

    /// Collect every node carrying a mesh handle, in pre-order,
    /// paired with its world transform.
    pub fn draw_list(&self) -> Vec<(Transform, String)> {
        let mut out = Vec::new();
        let identity = Transform::default();
        self.root.walk(&identity, &mut |world, node| {
            if let Some(mesh) = &node.mesh {
                out.push((world.clone(), mesh.clone()));
            }
        });
        out
    }

    /// Total node count (root + descendants).
    pub fn node_count(&self) -> usize {
        self.root.count()
    }
}


// ---------------------------------------------------------------------------
// SceneMesh — REAL geometry from the shipped art files (M21b, block 16)
// ---------------------------------------------------------------------------

/// One drawable mesh, decoded from a real P3D Mesh chunk
/// (`prototype_p3d::geometry::decode_mesh`).  This is the render path's
/// currency for real art geometry: positions at offset 0 (bbox-proven),
/// unit-or-zero normals, u16 indices (range-validated by the decoder).
#[derive(Debug, Clone, PartialEq)]
pub struct SceneMesh {
    /// Mesh name — PROVEN to equal a `pure3dShader` chunk name (the
    /// mesh→shader binding).
    pub name: String,
    /// The vertex-format descriptor word from the Mesh header.
    pub desc: u32,
    /// Positions (`mesh.header.vertex_count` entries).
    pub positions: Vec<[f32; 3]>,
    /// Normals (empty when the vertex format has none).
    pub normals: Vec<[f32; 3]>,
    /// First UV set (empty when the vertex format has none).
    pub uvs: Vec<[f32; 2]>,
    /// Unit tangents (empty when the layout has none — only the 0xb
    /// families measured them; block 35 feeds the world lit-pass's TBN
    /// path from these).
    pub tangents: Vec<[f32; 3]>,
    /// u16 indices (all < positions.len(), enforced by the decoder).
    pub indices: Vec<u16>,
    /// Mesh bounding box (min, max) from the BoundBox child.
    pub bound_box: Option<[f32; 6]>,
    /// Mesh bounding sphere (center, radius) from the BoundSphere child.
    pub bound_sphere: Option<[f32; 4]>,
    /// The mesh's diffuse texture as a raw DDS file — resolved by the
    /// collector through the proven chain mesh→shader→`color` param→
    /// in-member Texture (block 17).  `None` = not resolved (the draw
    /// falls back to the shared white texture).
    pub texture_dds: Option<Vec<u8>>,
    /// Additional textures named by the shader's other texture params
    /// (block 24: `normal`, `grime`, `glass_shatter`, `reflection`, …
    /// — 123 of the 197 shaders in the real cell-82 member name ≥2
    /// in-member-resolvable textures, so multi-texture is the NORM for
    /// world shaders).  In param order, capped by the collector.
    pub extra_textures: Vec<NamedDds>,
}

/// One named auxiliary texture (a raw DDS file plus the shader param
/// name that requested it, e.g. `"normal"`).
#[derive(Debug, Clone, PartialEq)]
pub struct NamedDds {
    /// The shader param name that requested this texture.
    pub slot: String,
    /// The complete embedded DDS file.
    pub dds: Vec<u8>,
}

impl SceneMesh {
    /// Bridge a decoded P3D MeshChunk into the scene model (pure data
    /// move; both crates share the same decoded arrays).
    pub fn from_p3d_mesh(mesh: &prototype_p3d::geometry::MeshChunk) -> SceneMesh {
        SceneMesh {
            name: mesh.name.clone(),
            desc: mesh.header.desc,
            positions: mesh.positions.clone(),
            normals: mesh.normals.clone(),
            uvs: mesh.uvs.clone(),
            tangents: mesh.tangents.clone(),
            indices: mesh.indices.clone(),
            bound_box: mesh.bound_box,
            bound_sphere: mesh.bound_sphere,
            texture_dds: None,
            extra_textures: Vec::new(),
        }
    }

    /// Center of the bounding box (None when the mesh has no bbox).
    pub fn bounds_center(&self) -> Option<[f32; 3]> {
        let bb = self.bound_box?;
        Some([
            (bb[0] + bb[3]) * 0.5,
            (bb[1] + bb[4]) * 0.5,
            (bb[2] + bb[5]) * 0.5,
        ])
    }

    /// Bounding-sphere radius estimate from the bbox diagonal (None when
    /// the mesh has no bbox) — used to frame the mesh with a camera.
    pub fn bounds_radius(&self) -> Option<f32> {
        let bb = self.bound_box?;
        let d = [bb[3] - bb[0], bb[4] - bb[1], bb[5] - bb[2]];
        Some((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() * 0.5)
    }

    /// Vertex count.
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    /// Triangle count (index_count / 3).
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use prototype_math::Quat;

    #[test]
    fn camera_view_projection_basic() {
        let cam = Camera {
            eye: Vec3::new(0.0, 0.0, 5.0),
            target: Vec3::ZERO,
            up: Vec3::UP,
            fov_y: std::f32::consts::FRAC_PI_2,
            aspect: 1.0,
            near: 0.1,
            far: 100.0,
        };
        // Eye maps to origin in view space.
        let e = cam.view().transform_point(cam.eye);
        assert!((e.x - 0.0).abs() < 1e-5);
        assert!((e.y - 0.0).abs() < 1e-5);
        assert!((e.z - 0.0).abs() < 1e-5);
        // A point at the near plane: world z = eye_z - near = 5 - 0.1
        // = 4.9 (camera looks down -z, so the near plane is in
        // front of the eye at z = 4.9).
        let near_pt = cam.view_projection().transform_point4(Vec3::new(0.0, 0.0, 4.9));
        assert!(
            (near_pt.z / near_pt.w - 0.0).abs() < 1e-5,
            "near_pt.z/w = {}, expected 0",
            near_pt.z / near_pt.w
        );
        // Far plane: world z = eye_z - far = 5 - 100 = -95.
        let far_pt = cam.view_projection().transform_point4(Vec3::new(0.0, 0.0, -95.0));
        assert!(
            (far_pt.z / far_pt.w - 1.0).abs() < 1e-5,
            "far_pt.z/w = {}, expected 1",
            far_pt.z / far_pt.w
        );
        // Points in front of the camera have w > 0.
        assert!(near_pt.w > 0.0 && far_pt.w > 0.0);
    }

    #[test]
    fn scene_graph_world_compose() {
        // Build a 2-level tree: parent translated by (10, 0, 0),
        // child translated by (1, 0, 0) in parent space.
        let mut root = SceneNode::new("root");
        let mut parent = SceneNode::new("parent");
        parent.local.position = Vec3::new(10.0, 0.0, 0.0);
        let mut child = SceneNode::new("child");
        child.local.position = Vec3::new(1.0, 0.0, 0.0);
        child.mesh = Some("unlit".into());
        parent.add_child(child);
        root.add_child(parent);

        let s = Scene {
            root,
            camera: Camera::default(),
        };

        // Total: root + parent + child = 3 nodes.
        assert_eq!(s.node_count(), 3);
        // Only the child has a mesh → draw_list has 1 entry.
        let dl = s.draw_list();
        assert_eq!(dl.len(), 1);
        let (world, mesh) = &dl[0];
        assert_eq!(mesh, "unlit");
        // World transform of the child: (10 + 1) on X.
        assert!((world.position.x - 11.0).abs() < 1e-5);
        assert!((world.position.y - 0.0).abs() < 1e-5);
        assert!((world.position.z - 0.0).abs() < 1e-5);
    }

    #[test]
    fn rotation_propagates_through_hierarchy() {
        let mut root = SceneNode::new("root");
        let mut parent = SceneNode::new("parent");
        // Quarter turn about +Y.
        parent.local.rotation = Quat::from_axis_angle(Vec3::UP, std::f32::consts::FRAC_PI_2);
        let mut child = SceneNode::new("child");
        // Child at +1 X in parent's local space; after parent's
        // quarter-turn about +Y, child's +X becomes -Z in world.
        child.local.position = Vec3::new(1.0, 0.0, 0.0);
        parent.add_child(child);
        root.add_child(parent);

        let s = Scene {
            root,
            camera: Camera::default(),
        };
        let _dl = s.draw_list();
        // No meshes set, but the world computation should still
        // produce the rotated position via the scene walk.
        // (draw_list only collects mesh-bearing nodes; check
        // directly via walk.)
        let mut visited = Vec::new();
        let id = Transform::default();
        s.root.walk(&id, &mut |w, n| {
            visited.push((w.position, n.name.clone()));
        });
        // root at origin; parent at origin (rotated); child at -Z (rotated +X).
        assert_eq!(visited.len(), 3);
        let (_, child_name) = visited.iter().find(|(_, n)| n == "child").unwrap();
        assert_eq!(child_name, "child");
        let (child_pos, _) = visited.iter().find(|(_, n)| n == "child").unwrap();
        assert!((child_pos.x - 0.0).abs() < 1e-5, "child.x: {}", child_pos.x);
        assert!((child_pos.z + 1.0).abs() < 1e-5, "child.z: {}", child_pos.z);
    }

    // -- SceneMesh / P3D bridge (real fixture, block 16) ------------------

    fn fixture(name: &str) -> Vec<u8> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    fn barrier_mesh() -> prototype_p3d::geometry::MeshChunk {
        let buf = fixture("art_barrier_os002.p3d");
        let top = prototype_p3d::geometry::top_level_chunks(&buf).unwrap();
        let shape = top
            .iter()
            .find(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_SHAPE)
            .map(|c| prototype_p3d::geometry::decode_shape(&buf, c.offset, c.size).unwrap())
            .unwrap();
        shape.meshes.into_iter().next().unwrap()
    }

    /// The real barrier mesh flows into the scene model unchanged:
    /// 576 positions, 1074 indices, bbox carried over.
    #[test]
    fn scene_mesh_bridges_real_p3d_mesh() {
        let decoded = barrier_mesh();
        let mesh = SceneMesh::from_p3d_mesh(&decoded);
        assert_eq!(mesh.desc, 0x2011);
        assert_eq!(mesh.vertex_count(), 576);
        assert_eq!(mesh.triangle_count(), 358);
        assert_eq!(mesh.indices.len(), 1074);
        assert!(mesh.uvs.len() == 576);
        assert!(!mesh.normals.is_empty());
        let bb = mesh.bound_box.unwrap();
        assert!((bb[0] - -0.951_343_24).abs() < 1e-6);
        // vertex data survives the bridge bit-exact
        assert_eq!(decoded.positions, mesh.positions);
    }

    /// Scene nodes can reference real meshes by name (the P3D name binds
    /// to the pure3dShader name; the scene stores it verbatim).
    #[test]
    fn scene_draw_list_carries_real_mesh_names() {
        let mesh = SceneMesh::from_p3d_mesh(&barrier_mesh());
        let mut root = SceneNode::new("root");
        let mut n = SceneNode::new("barrier");
        n.mesh = Some(mesh.name.clone());
        root.add_child(n);
        let s = Scene { root, camera: Camera::default() };
        let dl = s.draw_list();
        assert_eq!(dl.len(), 1);
        assert_eq!(dl[0].1, "131185ef20d24489daf1165e65814182");
    }


    /// THE convention test: for a real camera, `view_projection_rows()[i]`
    /// dotted with a point must reproduce the column-major
    /// `view_projection().transform_point4` — proving the D3D-row packing
    /// matches the WGSL's `out = M · v` arithmetic.
    #[test]
    fn view_projection_rows_match_column_major_transform() {
        let cam = Camera {
            eye: Vec3::new(3.0, -2.0, 7.0),
            target: Vec3::new(0.5, 0.25, -1.0),
            up: Vec3::UP,
            fov_y: 1.1,
            aspect: 1.7777,
            near: 0.2,
            far: 90.0,
        };
        let m = cam.view_projection();
        let rows = cam.view_projection_rows();
        for q in [
            Vec3::new(0.5, 0.25, -1.0),   // the target
            Vec3::new(3.0, -2.0, 7.0),    // the eye
            Vec3::new(-12.0, 4.4, -30.0), // arbitrary
        ] {
            let p4 = m.transform_point4(q);
            let comps = [p4.x, p4.y, p4.z, p4.w];
            let mut via_rows = [0.0f32; 4];
            for (i, r) in rows.iter().enumerate() {
                via_rows[i] = r[0] * q.x + r[1] * q.y + r[2] * q.z + r[3];
            }
            for i in 0..4 {
                let scale = if p4.w == 0.0 { 1.0 } else { p4.w };
                let a = comps[i] / scale;
                let b = via_rows[i] / scale;
                assert!(
                    (a - b).abs() < 1e-4,
                    "row {i} mismatch: {a} vs {b}"
                );
            }
        }
    }

    /// bounds helpers on the real barrier mesh.
    #[test]
    fn scene_mesh_bounds_helpers() {
        let mesh = SceneMesh::from_p3d_mesh(&barrier_mesh());
        let c = mesh.bounds_center().unwrap();
        let bb = mesh.bound_box.unwrap();
        assert!((c[0] - (bb[0] + bb[3]) * 0.5).abs() < 1e-6);
        let r = mesh.bounds_radius().unwrap();
        let d = [bb[3] - bb[0], bb[4] - bb[1], bb[5] - bb[2]];
        let want = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() * 0.5;
        assert!((r - want).abs() < 1e-6);
    }

}

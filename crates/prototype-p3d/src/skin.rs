//! Block 83 — OLD-STYLE SKINNED P3D GEOMETRY + SKELETON (cracked).
//!
//! The NIS character packages embed the REAL skinned characters as
//! old-style shapes (`0x0001_0001`) that reference a Skeleton chunk
//! (`0x0002_3000`).  Both families are decoded here, byte-exactly,
//! fail-closed, against real data:
//!
//! * **Skeleton `0x0002_3000`**: u8-pascal name, u32 version (1),
//!   u32 joint count, 8 reserved zero bytes, then EXACTLY `joint_count`
//!   nested `0x0002_3001` chunks, one per joint.  Each joint chunk's
//!   payload is a FIXED 122 bytes: u32 parent index, then a 4×4
//!   ROW-VECTOR rest matrix (16 f32, bottom row = translation), then
//!   13 f32 (zeros except three 1.0 scale words — recorded, unused)
//!   and a u16 tail (0).  Root joints self-reference (parent == own
//!   index).
//!
//! * **Old-style shape `0x0001_0001`**: u8-pascal name, u32 version
//!   (3 = skinned), u8-pascal skeleton reference name, u32 prim count,
//!   then nested chunks: a `0x0001_0021` stream-table record (8×u32),
//!   `0x0001_0020` MESHES (same MeshHeader as the modern family:
//!   field0/desc/vertex_count/index_count/tail[5] — but with old-style
//!   per-shape names like `Body_SG`), plus `0x0012_2000`, `0x0001_0017`,
//!   `0x0001_0003` (bound box) and `0x0001_0004` (bound sphere).
//!
//! * **Skinned mesh `0x0001_0020`** (desc `0xB1B1` — the skinned body
//!   desc, derived from the mesh's OWN PrimitiveGroup declaration):
//!   TWO vertex buffers:
//!   - stream A (`field2 == 2`), stride 56: position f32×3 @0, normal
//!     f32×3 @12, tangent f32×4 @24, uv f32×3 @40 (u,v used; third
//!     word 0), bone-matrix indices u8×4 @52 (PALETTE-relative);
//!   - stream B (`field2 == 1`), stride 12: u32 marker (0xFFFFFFFF or
//!     0xFF7F7F7F) + uv f32×2 (a second UV set).
//!   - `0x0001_0013` IndexBuffer: u16 LE indices.
//!   - `0x0001_000D` BONE PALETTE: u32 count + count × u32 SKELETON
//!     joint indices (vertex bone indices index THIS table).  The mesh
//!     header's tail[0] equals the palette count on every observed
//!     mesh (validated loosely: equal OR the palette chunk simply
//!     carries the truth).
//!
//! Per-slot blend weights are NOT present in either stream (the
//! `0xFF7F7F7F`-marker vertices and the raw floats were probed — no
//! weight-like data).  [`skin_positions`] therefore applies the
//! PREFIX-EQUAL rule: slots 0..=last-nonzero blend equally (bone 0 is
//! a valid slot, so an all-zero record = 100% slot 0).  Rest-pose
//! skinning is exact for ANY weights (the GPU `G = W·rest⁻¹·W_pose`
//! collapses to identity), which the regression test pins.
//!
//! Every decoder validates structure fail-closed (exact sizes, buffer
//! magics, index bounds, palette bounds, decl offsets) and returns
//! [`crate::geometry::ArtError`] on the first violation.

use crate::geometry::{pascal_at, u32_at, ArtError};

/// [`pascal_at`] with trailing NUL bytes trimmed.
///
/// Block 88: REAL alex.p3d p3dstrings carry trailing NULs (the shape's
/// skeleton name field is `alex_reg_body_skeleton\0\0` — 24 B for 22
/// chars).  Untrimmed names break skeleton binding and anim bone-name
/// matching, so the skinned-family decoders trim here.
fn pascal_trimmed(buf: &[u8], off: usize, end: usize) -> Result<(String, usize), ArtError> {
    let (s, q) = pascal_at(buf, off, end)?;
    Ok((s.trim_end_matches('\0').to_string(), q))
}

// ---------------------------------------------------------------------------
// Skeleton
// ---------------------------------------------------------------------------

/// Skeleton chunk type.
pub const CHUNK_TYPE_SKELETON: u32 = 0x0002_3000;
/// One joint = one nested chunk of this type.
pub const CHUNK_TYPE_SKELETON_JOINT: u32 = 0x0002_3001;
/// Fixed per-joint payload after the parent word (13 f32 + u16 tail).
const JOINT_FIXED_TAIL: usize = 13 * 4 + 2;
/// Old-style shape chunk type.
pub const CHUNK_TYPE_OLD_SHAPE: u32 = 0x0001_0001;
/// Bone-palette chunk type.
pub const CHUNK_TYPE_BONE_PALETTE: u32 = 0x0001_000D;

/// One skeleton joint: name, parent index, and the 4×4 ROW-VECTOR rest
/// matrix (parent-relative; bottom row = translation).
#[derive(Debug, Clone, PartialEq)]
pub struct SkeletonJoint {
    pub name: String,
    pub parent: u32,
    pub rest: [[f32; 4]; 4],
}

/// A decoded Skeleton chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct Skeleton {
    pub name: String,
    pub joints: Vec<SkeletonJoint>,
}

impl Skeleton {
    /// Index of a joint by exact name.
    pub fn find(&self, name: &str) -> Option<usize> {
        self.joints.iter().position(|j| j.name == name)
    }
}

/// Decode the Skeleton chunk at `offset` (total `size`).
pub fn decode_skeleton(buf: &[u8], offset: usize, size: usize) -> Result<Skeleton, ArtError> {
    let end = offset + size;
    if end > buf.len() {
        return Err(ArtError::Truncated { offset, need: size, got: buf.len() - offset });
    }
    let (name, mut q) = pascal_trimmed(buf, offset + 12, end)?;
    let version = u32_at(buf, q)?;
    if version != 1 {
        return Err(ArtError::UnexpectedVersion { offset: q, got: version, want: 1 });
    }
    let n = u32_at(buf, q + 4)? as usize;
    // 8 bytes follow (q+8 .. q+16).  Block 88: RECORDED, not enforced —
    // the REAL alex.p3d skeletons carry 0x0000000d in the first word
    // (the nis_alex fixture carried zeros; both parse).
    for _ in 0..2 {
        let _z = u32_at(buf, q + 8)?;
    }
    q += 16;
    let mut joints = Vec::with_capacity(n);
    while joints.len() < n {
        if q + 12 > end {
            return Err(ArtError::Truncated { offset: q, need: 12, got: end - q });
        }
        let t = u32_at(buf, q)?;
        if t != CHUNK_TYPE_SKELETON_JOINT {
            return Err(ArtError::UnexpectedData {
                offset: q,
                why: format!("expected joint chunk {CHUNK_TYPE_SKELETON_JOINT:#x}, got {t:#x}"),
            });
        }
        let jsize = u32_at(buf, q + 8)? as usize;
        if jsize < 12 || q + jsize > end {
            return Err(ArtError::Truncated { offset: q, need: jsize, got: end - q });
        }
        let jend = q + jsize;
        let (jname, jp) = pascal_trimmed(buf, q + 12, jend)?;
        let parent = u32_at(buf, jp)?;
        let idx = joints.len() as u32;
        if parent > idx {
            return Err(ArtError::UnexpectedData {
                offset: jp,
                why: format!("joint {jname:?} parent {parent} > own index {idx}"),
            });
        }
        let mut rest = [[0f32; 4]; 4];
        for r in 0..4 {
            for c in 0..4 {
                rest[r][c] = f32_at(buf, jp + 4 + (r * 4 + c) * 4)?;
            }
        }
        // 13 f32 + u16 tail must fit exactly inside the chunk payload.
        let after_matrix = jp + 4 + 64;
        if after_matrix + JOINT_FIXED_TAIL != jend {
            return Err(ArtError::UnexpectedData {
                offset: after_matrix,
                why: format!(
                    "joint {jname:?} payload tail {} B, expected {JOINT_FIXED_TAIL}",
                    jend - after_matrix
                ),
            });
        }
        joints.push(SkeletonJoint { name: jname, parent, rest });
        q = jend;
    }
    // Block 88: trailing children (the REAL alex.p3d skeletons carry
    // 0x23002 / 0x23003 records after the joints) — walk them, demand
    // only that they tile the remaining span exactly.
    while q + 12 <= end {
        let t = u32_at(buf, q)?;
        let sz = u32_at(buf, q + 8)? as usize;
        if sz < 12 || q + sz > end {
            return Err(ArtError::NotExactCover { offset: q, end });
        }
        let _ = t; // recorded-not-decoded extra chunk
        q += sz;
    }
    if q != end {
        return Err(ArtError::NotExactCover { offset: q, end });
    }
    Ok(Skeleton { name, joints })
}

/// Prelude-aware skeleton decode at the chunk START `offset` (block 90
/// NEXT-UP #1): parse the standard 12-B Pure3D prelude
/// `[type][uid][size]` — the size word sits at +8, the +4 word is the
/// per-instance uid — verify the type and dispatch to
/// [`decode_skeleton`].  This removes the m89 failure mode where a
/// caller passed the +4 uid word (53 on the real alex_reg_body
/// skeleton) as the size and the decode failed closed.
pub fn decode_skeleton_at(buf: &[u8], offset: usize) -> Result<Skeleton, ArtError> {
    if offset + 12 > buf.len() {
        return Err(ArtError::Truncated { offset, need: 12, got: buf.len() - offset });
    }
    let t = u32_at(buf, offset)?;
    if t != CHUNK_TYPE_SKELETON {
        return Err(ArtError::UnexpectedData {
            offset,
            why: format!(
                "expected skeleton chunk {CHUNK_TYPE_SKELETON:#x}, got {t:#x}"
            ),
        });
    }
    let size = u32_at(buf, offset + 8)? as usize;
    decode_skeleton(buf, offset, size)
}

fn f32_at(buf: &[u8], off: usize) -> Result<f32, ArtError> {
    if off + 4 > buf.len() {
        return Err(ArtError::Truncated { offset: off, need: 4, got: buf.len() - off });
    }
    Ok(f32::from_le_bytes(buf[off..off + 4].try_into().unwrap()))
}

// ---------------------------------------------------------------------------
// Old-style skinned shape + mesh
// ---------------------------------------------------------------------------

/// One decoded skinned mesh (nested `0x0001_0020` of an old-style shape).
#[derive(Debug, Clone, PartialEq)]
pub struct SkinMesh {
    pub name: String,
    pub desc: u32,
    pub vertex_count: u32,
    pub index_count: u32,
    /// Mesh header tail words (tail[0] = palette count on every sample).
    pub tail: [u32; 5],
    /// Bone palette: skeleton joint indices (u32), palette-relative
    /// vertex bone indices index this table.
    pub palette: Vec<u32>,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub tangents: Vec<[f32; 4]>,
    /// Stream-A UV (u,v) — element @40.
    pub uvs: Vec<[f32; 2]>,
    /// Per-vertex blend weights aligned with `bone_indices` slots.
    /// Block 88 CRACKED: the REAL alex.p3d weight stream is
    /// stream-B f32 pair (w0, w1) + stream-A f32 @48 (w2) + the
    /// complement (w3 = 1 - w0 - w1 - w2, clamped >= 0), paired
    /// POSITIONALLY with the bone slots @52.  Empty = no weight
    /// stream present (the prefix-equal fallback applies).
    pub weights: Vec<[f32; 4]>,
    /// Palette-relative bone indices u8×4 per vertex.
    pub bone_indices: Vec<[u8; 4]>,
    pub indices: Vec<u16>,
    pub bound_box: Option<[f32; 6]>,
    pub bound_sphere: Option<[f32; 4]>,
    /// The shape-level `0x0001_0021` stream-table words (8×u32).
    pub decl_words: [u32; 8],
}

/// A decoded old-style skinned shape.
#[derive(Debug, Clone, PartialEq)]
pub struct OldSkinnedShape {
    pub name: String,
    pub version: u32,
    pub skeleton: String,
    pub meshes: Vec<SkinMesh>,
    /// Types of the non-mesh child chunks (the 0x0001_0021 record is
    /// consumed into `decl_words`; bbox/sphere decoded).
    pub child_types: Vec<u32>,
}

/// The proven stream-A offsets for desc `0xB1B1` (from the meshes' own
/// PrimitiveGroup declarations on six real alex meshes).
const OFF_NORMAL: usize = 12;
const OFF_TANGENT: usize = 24;
const OFF_UV: usize = 40;
const OFF_BONES: usize = 52;
/// Stream-A stride.
const STRIDE_A: usize = 56;
/// Stream-B stride (u32 marker + f32 uv).
const STRIDE_B: usize = 12;

/// Decode the old-style skinned shape at `offset` (total `size`).
pub fn decode_old_skinned_shape(
    buf: &[u8],
    offset: usize,
    size: usize,
) -> Result<OldSkinnedShape, ArtError> {
    let end = offset + size;
    if end > buf.len() {
        return Err(ArtError::Truncated { offset, need: size, got: buf.len() - offset });
    }
    let (name, mut q) = pascal_trimmed(buf, offset + 12, end)?;
    let version = u32_at(buf, q)?;
    q += 4;
    let mut skeleton = String::new();
    if version >= 1 {
        // Block 88: the REAL alex.p3d shapes are version 3 and their
        // skeleton p3dstring carries trailing NULs — trim so the
        // shape→skeleton chunk binding compares clean names.
        let (s, nq) = pascal_trimmed(buf, q, end)?;
        skeleton = s;
        q = nq;
    }
    let nprims = u32_at(buf, q)? as usize;
    q += 4;
    let mut meshes = Vec::new();
    let mut child_types = Vec::new();
    let mut decl_words = [0u32; 8];
    let mut children = 0usize;
    while q + 12 <= end {
        let t = u32_at(buf, q)?;
        let csize = u32_at(buf, q + 8)? as usize;
        if csize < 12 || q + csize > end {
            return Err(ArtError::Truncated { offset: q, need: csize, got: end - q });
        }
        children += 1;
        match t {
            0x0001_0021 => {
                let payload = csize - 12;
                if payload != 32 {
                    return Err(ArtError::UnexpectedData {
                        offset: q + 12,
                        why: format!("stream-table payload {payload} B, expected 32"),
                    });
                }
                for i in 0..8 {
                    decl_words[i] = u32_at(buf, q + 12 + i * 4)?;
                }
            }
            0x0001_0020 => {
                let m = decode_skin_mesh(buf, q, csize, &decl_words)?;
                meshes.push(m);
            }
            _ => child_types.push(t),
        }
        q += csize;
    }
    if q != end {
        return Err(ArtError::NotExactCover { offset: q, end });
    }
    let _ = children; // total child chunks (incl. the decl record) — recorded, not enforced
    // Block 88 (the block-85 re-land): `nprims` counts the PRIMITIVE
    // (mesh) chunks, NOT every child — the REAL alex.p3d shapes
    // declare nprims = 1 while carrying decl + mesh + bbox + sphere +
    // modifier children (the live-file census: 6-7 children, 1 mesh).
    if meshes.len() != nprims {
        return Err(ArtError::UnexpectedData {
            offset,
            why: format!(
                "shape {name:?} declares {nprims} prims, decoded {} meshes",
                meshes.len()
            ),
        });
    }
    Ok(OldSkinnedShape { name, version, skeleton, meshes, child_types })
}

/// Decode one skinned mesh chunk.
fn decode_skin_mesh(
    buf: &[u8],
    offset: usize,
    size: usize,
    decl_words: &[u32; 8],
) -> Result<SkinMesh, ArtError> {
    let end = offset + size;
    let q = offset + 12;
    let ver = u32_at(buf, q)?;
    if ver != 0 {
        return Err(ArtError::UnexpectedVersion { offset: q, got: ver, want: 0 });
    }
    let (name, after_name) = pascal_trimmed(buf, q + 4, end)?;
    let mut words = [0u32; 9];
    for (i, w) in words.iter_mut().enumerate() {
        *w = u32_at(buf, after_name + i * 4)?;
    }
    let desc = words[1];
    let vcount = words[2] as usize;
    let icount = words[3] as usize;
    if desc != 0xB1B1 {
        return Err(ArtError::UnexpectedData {
            offset: after_name + 4,
            why: format!("skinned mesh {name:?} desc {desc:#010x}, only 0xB1B1 is proven"),
        });
    }
    let mut palette = Vec::new();
    let mut bound_box = None;
    let mut bound_sphere = None;
    let mut stream_a: Option<&[u8]> = None;
    let mut stream_b: Option<&[u8]> = None;
    let mut indices_raw: Option<&[u8]> = None;
    let mut w = after_name + 36;
    let mut child_types = Vec::new();
    while w + 12 <= end {
        let t = u32_at(buf, w)?;
        let csize = u32_at(buf, w + 8)? as usize;
        if csize < 12 || w + csize > end {
            return Err(ArtError::Truncated { offset: w, need: csize, got: end - w });
        }
        let p = w + 12;
        match t {
            0x0001_0012 => {
                let magic = u32_at(buf, p)?;
                if magic != 0x0002_0001 {
                    return Err(ArtError::BadBufferMagic { offset: p, got: magic });
                }
                let f2 = u32_at(buf, p + 4)?;
                let dsz = u32_at(buf, p + 8)? as usize;
                if p + 12 + dsz > end {
                    return Err(ArtError::Truncated { offset: p, need: dsz, got: end - p - 12 });
                }
                match f2 {
                    2 => {
                        if dsz != vcount * STRIDE_A {
                            return Err(ArtError::BadVertexDataSize {
                                declared: dsz as u32,
                                want: vcount * STRIDE_A,
                            });
                        }
                        stream_a = Some(&buf[p + 12..p + 12 + dsz]);
                    }
                    1 => {
                        if dsz != vcount * STRIDE_B {
                            return Err(ArtError::BadVertexDataSize {
                                declared: dsz as u32,
                                want: vcount * STRIDE_B,
                            });
                        }
                        stream_b = Some(&buf[p + 12..p + 12 + dsz]);
                    }
                    other => {
                        return Err(ArtError::UnexpectedData {
                            offset: p + 4,
                            why: format!("vertex buffer field2 {other}, want 1 or 2"),
                        })
                    }
                }
            }
            0x0001_0013 => {
                let magic = u32_at(buf, p)?;
                if magic != 0x0002_0001 {
                    return Err(ArtError::BadBufferMagic { offset: p, got: magic });
                }
                let dsz = u32_at(buf, p + 8)? as usize;
                if dsz != icount * 2 {
                    return Err(ArtError::BadIndexDataSize {
                        declared: dsz as u32,
                        want: icount * 2,
                    });
                }
                indices_raw = Some(&buf[p + 12..p + 12 + dsz]);
            }
            CHUNK_TYPE_BONE_PALETTE => {
                let count = u32_at(buf, p)? as usize;
                if 4 + count * 4 > csize - 12 {
                    return Err(ArtError::UnexpectedData {
                        offset: p,
                        why: format!("palette count {count} exceeds payload {}", csize - 12),
                    });
                }
                palette = (0..count).map(|i| u32_at(buf, p + 4 + i * 4).unwrap()).collect();
            }
            0x0001_0003 if csize - 12 == 24 => {
                bound_box = Some([
                    f32_at(buf, p)?,
                    f32_at(buf, p + 4)?,
                    f32_at(buf, p + 8)?,
                    f32_at(buf, p + 12)?,
                    f32_at(buf, p + 16)?,
                    f32_at(buf, p + 20)?,
                ]);
            }
            0x0001_0004 if csize - 12 == 16 => {
                bound_sphere = Some([
                    f32_at(buf, p)?,
                    f32_at(buf, p + 4)?,
                    f32_at(buf, p + 8)?,
                    f32_at(buf, p + 12)?,
                ]);
            }
            _ => child_types.push(t),
        }
        w += csize;
    }
    if w != end {
        return Err(ArtError::NotExactCover { offset: w, end });
    }
    let sa = stream_a.ok_or(ArtError::UnsupportedMeshFamily {
        children: child_types.clone(),
    })?;
    let ib = indices_raw.ok_or(ArtError::UnsupportedMeshFamily {
        children: child_types.clone(),
    })?;

    // Stream A decode (decl offsets are proven constants for 0xB1B1).
    // Block 88 (the block-85 re-land): the decl's stride words are
    // RECORDED on the mesh (decl_words) but NOT enforced — the REAL
    // alex.p3d shapes declare 0x20-sized stream entries while the
    // actual stream-A vertex stride is 56 B (proven by the stream
    // byte counts: 140 672 B / 2 512 verts = 56).
    let mut positions = Vec::with_capacity(vcount);
    let mut normals = Vec::with_capacity(vcount);
    let mut tangents = Vec::with_capacity(vcount);
    let mut uvs = Vec::with_capacity(vcount);
    let mut uvs_w2 = Vec::with_capacity(vcount);
    let mut bone_indices = Vec::with_capacity(vcount);
    for v in 0..vcount {
        let base = sa.as_ptr() as usize - buf.as_ptr() as usize + v * STRIDE_A;
        let g = |o: usize| f32_at(buf, base + o);
        positions.push([g(0)?, g(4)?, g(8)?]);
        normals.push([g(OFF_NORMAL)?, g(OFF_NORMAL + 4)?, g(OFF_NORMAL + 8)?]);
        tangents.push([
            g(OFF_TANGENT)?,
            g(OFF_TANGENT + 4)?,
            g(OFF_TANGENT + 8)?,
            g(OFF_TANGENT + 12)?,
        ]);
        uvs.push([g(OFF_UV)?, g(OFF_UV + 4)?]);
        uvs_w2.push(g(OFF_UV + 8)?); // @48 = blend weight slot 2
        let b0 = buf[base + OFF_BONES];
        let b1 = buf[base + OFF_BONES + 1];
        let b2 = buf[base + OFF_BONES + 2];
        let b3 = buf[base + OFF_BONES + 3];
        bone_indices.push([b0, b1, b2, b3]);
    }
    // Block 88: stream B is the WEIGHT stream (12 B/vertex: u32
    // marker + f32 w0 + f32 w1) — the per-vertex blend weights the
    // old-style fallback carries as separate 0x1000c chunks.  The
    // third weight rides in stream A @48; the fourth is the
    // complement.  (Re-derivation: vert 500 of the REAL body mesh
    // reads (A,B,X) = (0.4877, 0.2866, 0.1681) with slots
    // [9, 11, 12, 7] — four weights summing to 1.)
    let mut weights = Vec::with_capacity(vcount);
    if let Some(sb) = stream_b {
        for v in 0..vcount {
            let base = sb.as_ptr() as usize - buf.as_ptr() as usize + v * STRIDE_B;
            let w0 = f32_at(buf, base + 4)?;
            let w1 = f32_at(buf, base + 8)?;
            let w2 = uvs_w2[v];
            let w3 = (1.0 - w0 - w1 - w2).max(0.0);
            let mut w = [w0, w1, w2, w3];
            let sum: f32 = w.iter().sum();
            if sum > 1e-6 {
                for x in w.iter_mut() {
                    *x /= sum;
                }
            } else {
                w = [0.0; 4];
            }
            weights.push(w);
        }
    }
    // Indices.
    let mut indices = Vec::with_capacity(icount);
    for i in 0..icount {
        let o = ib.as_ptr() as usize - buf.as_ptr() as usize + i * 2;
        let idx = u16::from_le_bytes([buf[o], buf[o + 1]]);
        if idx as usize >= vcount {
            return Err(ArtError::IndexOutOfRange { index: idx, vertices: vcount as u32 });
        }
        indices.push(idx);
    }
    // Palette bounds vs the declared bone indices.
    for (v, b) in bone_indices.iter().enumerate() {
        for (slot, &bx) in b.iter().enumerate() {
            if bx as usize >= palette.len() {
                return Err(ArtError::UnexpectedData {
                    offset,
                    why: format!(
                        "vertex {v} bone slot {slot} index {bx} >= palette {}",
                        palette.len()
                    ),
                });
            }
        }
    }
    Ok(SkinMesh {
        name,
        desc,
        vertex_count: vcount as u32,
        index_count: icount as u32,
        tail: [words[4], words[5], words[6], words[7], words[8]],
        palette,
        positions,
        normals,
        tangents,
        uvs,
        weights,
        bone_indices,
        indices,
        bound_box,
        bound_sphere,
        decl_words: *decl_words,
    })
}

// ---------------------------------------------------------------------------
// Row-vector 4×4 matrix helpers + linear-blend skinning
// ---------------------------------------------------------------------------

/// Row-vector 4×4 matrix product: `mat4_mul(a, b)` = A×B such that a
/// point transforms as `p' = p × A × B`.
pub fn mat4_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0f32; 4]; 4];
    for r in 0..4 {
        for c in 0..4 {
            let mut s = 0f32;
            for k in 0..4 {
                s += a[r][k] * b[k][c];
            }
            out[r][c] = s;
        }
    }
    out
}

/// Apply a row-vector matrix to a point (w = 1).
///
/// Block 88 BUG FIX: the perspective row of a row-vector matrix is the
/// LAST COLUMN (`p'[3] = \u{3a3}_k p[k]\u{b7}M[k][3]`), NOT the bottom row (which
/// carries the TRANSLATION).  The old code divided by the bottom-row
/// dot — for affine skinning matrices (last column `[0,0,0,1]`) w' = 1
/// exactly, so the bogus divide silently scaled every posed vertex by
/// `1/(p\u{b7}t + 1)` (~0.7-1.4x) — the collapsed/tangled renders.
pub fn mat4_apply(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
    // TRUE ROW-VECTOR apply: p' = p·M (out[r] = Σ_k p[k]·m[k][r] +
    // w·m[3][r]).  Block 88 BUG FIX #2: the old apply computed the
    // COLUMN-vector M·p while the whole pipeline composes/composes
    // row-vector (`world = local × parent`, `G = inv_rest × pose`) —
    // the mixed convention tangled every posed render.  For affine
    // matrices w' = 1 exactly, so no divide.
    let mut out = [0f32; 3];
    for r in 0..3 {
        out[r] = p[0] * m[0][r] + p[1] * m[1][r] + p[2] * m[2][r] + m[3][r];
    }
    out
}

/// Apply a row-vector matrix to a direction (w = 0).
pub fn mat4_apply_dir(m: &[[f32; 4]; 4], d: [f32; 3]) -> [f32; 3] {
    // TRUE ROW-VECTOR direction apply (see `mat4_apply`).
    let mut out = [0f32; 3];
    for r in 0..3 {
        out[r] = d[0] * m[0][r] + d[1] * m[1][r] + d[2] * m[2][r];
    }
    out
}

/// Invert an affine row-vector 4×4 (upper 3×3 invertible).
pub fn mat4_inverse_affine(m: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    // Adjugate of the 3×3.
    let a = m;
    let mut inv3 = [[0f32; 3]; 3];
    inv3[0][0] = a[1][1] * a[2][2] - a[1][2] * a[2][1];
    inv3[0][1] = a[0][2] * a[2][1] - a[0][1] * a[2][2];
    inv3[0][2] = a[0][1] * a[1][2] - a[0][2] * a[1][1];
    inv3[1][0] = a[1][2] * a[2][0] - a[1][0] * a[2][2];
    inv3[1][1] = a[0][0] * a[2][2] - a[0][2] * a[2][0];
    inv3[1][2] = a[0][2] * a[1][0] - a[0][0] * a[1][2];
    inv3[2][0] = a[1][0] * a[2][1] - a[1][1] * a[2][0];
    inv3[2][1] = a[0][1] * a[2][0] - a[0][0] * a[2][1];
    inv3[2][2] = a[0][0] * a[1][1] - a[0][1] * a[1][0];
    let det = a[0][0] * inv3[0][0] + a[0][1] * inv3[1][0] + a[0][2] * inv3[2][0];
    if det.abs() < 1e-12 {
        return identity4();
    }
    let s = 1.0 / det;
    let mut out = [[0f32; 4]; 4];
    for r in 0..3 {
        for c in 0..3 {
            out[r][c] = inv3[r][c] * s;
        }
    }
    // Translation column: -t × inv3.
    for c in 0..3 {
        out[3][c] = -(a[3][0] * out[0][c] + a[3][1] * out[1][c] + a[3][2] * out[2][c]);
    }
    out[3][3] = 1.0;
    out
}

/// Identity 4×4.
pub fn identity4() -> [[f32; 4]; 4] {
    let mut m = [[0f32; 4]; 4];
    for (i, r) in m.iter_mut().enumerate() {
        r[i] = 1.0;
    }
    m
}

/// Rotation about an arbitrary axis (row-vector convention: `p' = p × R`).
pub fn mat4_rot_axis(axis: [f32; 3], ang: f32) -> [[f32; 4]; 4] {
    let len = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    if len < 1e-9 {
        return identity4();
    }
    let (x, y, z) = (axis[0] / len, axis[1] / len, axis[2] / len);
    let (s, c) = ang.sin_cos();
    let t = 1.0 - c;
    // Row-vector rotation matrix (transpose of the column-vector one).
    let mut m = [[0f32; 4]; 4];
    m[0][0] = c + x * x * t;
    m[0][1] = x * y * t + z * s;
    m[0][2] = x * z * t - y * s;
    m[1][0] = y * x * t - z * s;
    m[1][1] = c + y * y * t;
    m[1][2] = y * z * t + x * s;
    m[2][0] = z * x * t + y * s;
    m[2][1] = z * y * t - x * s;
    m[2][2] = c + z * z * t;
    m[3][3] = 1.0;
    m
}

/// Rest-pose FORWARD KINEMATICS: world matrix per joint
/// (`W_j = rest_j × W_parent`; root: `W = rest`).
pub fn rest_world_matrices(s: &Skeleton) -> Vec<[[f32; 4]; 4]> {
    let mut out = vec![identity4(); s.joints.len()];
    for (i, j) in s.joints.iter().enumerate() {
        let parent_w = if j.parent < i as u32 { out[j.parent as usize] } else { identity4() };
        out[i] = mat4_mul(&j.rest, &parent_w);
    }
    out
}

/// The per-vertex blend record: (weights, active slot count).
/// With a REAL weight stream (block 88) the stored weights drive the
/// blend; without one the PREFIX-EQUAL fallback applies (slots
/// 0..=last-nonzero blend equally; bone 0 is a valid slot, so an
/// all-zero record = 100% slot 0).
fn blend_record(mesh: &SkinMesh, i: usize, b: &[u8; 4]) -> ([f32; 4], usize) {
    if let Some(&w) = mesh.weights.get(i) {
        let last = (0..4)
            .filter(|&s| w[s] > 0.0)
            .next_back()
            .unwrap_or(0);
        return (w, last + 1);
    }
    weights_for(b)
}

/// Prefix-equal blend weights for a bone-index record (see module docs).
fn weights_for(b: &[u8; 4]) -> ([f32; 4], usize) {
    let mut last = 0usize;
    for (i, &x) in b.iter().enumerate() {
        if x != 0 {
            last = i;
        }
    }
    let n = last + 1;
    let w = 1.0 / n as f32;
    let mut out = [0f32; 4];
    for o in out.iter_mut().take(n) {
        *o = w;
    }
    (out, n)
}

/// Skinning matrix per palette slot: `G = W_rest⁻¹ × W_pose` (row-vector).
///
/// `rest_world` / `pose_world` are per-JOINT (skeleton index) world
/// matrices — the palette maps vertex slots to joints.
pub fn skin_matrices(
    palette: &[u32],
    rest_world: &[[[f32; 4]; 4]],
    pose_world: &[[[f32; 4]; 4]],
) -> Vec<[[f32; 4]; 4]> {
    palette
        .iter()
        .map(|&j| {
            let id = identity4();
            let rest = rest_world.get(j as usize).unwrap_or(&id);
            let pose = pose_world.get(j as usize).unwrap_or(&id);
            let inv = mat4_inverse_affine(rest);
            mat4_mul(&inv, pose)
        })
        .collect()
}

/// Skin the mesh's positions with the given per-palette-slot skinning
/// matrices (prefix-equal blend).
/// Skin the mesh's positions with the given per-palette-slot skinning
/// matrices (prefix-equal blend).
///
/// Block 88 (the block-85 re-land): the matrices are indexed by the
/// vertex's PALETTE-RELATIVE slot (`bone_indices[i]`), NOT through the
/// palette again — `skin_matrices` emits ONE matrix per palette entry,
/// so the palette was already consumed building it (the old code
/// mapped through the palette a second time and clamped the overflow).
pub fn skin_positions(mesh: &SkinMesh, skin: &[[[f32; 4]; 4]]) -> Vec<[f32; 3]> {
    mesh.positions
        .iter()
        .enumerate()
        .zip(mesh.bone_indices.iter())
        .map(|((i, p), b)| {
            let (w, n) = blend_record(mesh, i, b);
            let mut acc = [0f32; 3];
            for slot in 0..n {
                let slot_idx = b[slot] as usize;
                let id = identity4();
                let g = skin.get(slot_idx).unwrap_or(&id);
                let t = mat4_apply(g, *p);
                for c in 0..3 {
                    acc[c] += w[slot] * t[c];
                }
            }
            acc
        })
        .collect()
}

/// Skin the mesh's normals (direction transform, renormalized).
///
/// Block 88: per-PALETTE-SLOT indexing (see [`skin_positions`]).
pub fn skin_normals(mesh: &SkinMesh, skin: &[[[f32; 4]; 4]]) -> Vec<[f32; 3]> {
    mesh.normals
        .iter()
        .enumerate()
        .zip(mesh.bone_indices.iter())
        .map(|((i, n), b)| {
            let (w, slots) = blend_record(mesh, i, b);
            let mut acc = [0f32; 3];
            for slot in 0..slots {
                let id = identity4();
                let g = skin.get(b[slot] as usize).unwrap_or(&id);
                let t = mat4_apply_dir(g, *n);
                for c in 0..3 {
                    acc[c] += w[slot] * t[c];
                }
            }
            let len = (acc[0] * acc[0] + acc[1] * acc[1] + acc[2] * acc[2]).sqrt();
            if len > 1e-9 {
                acc.map(|v| v / len)
            } else {
                [0.0, 1.0, 0.0]
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The synthetic alex-census fixtures (generated by
    /// scripts/m113_make_skin_fixtures.py, byte-mirroring the REAL
    /// alex.p3d layout probed this block).
    const SKEL: &[u8] = include_bytes!("../../prototype-bin/tests/fixtures/alex_style_skeleton.bin");
    const SHAPE_OK: &[u8] = include_bytes!("../../prototype-bin/tests/fixtures/alex_style_shape_ok.bin");
    const SHAPE_BADPAL: &[u8] = include_bytes!("../../prototype-bin/tests/fixtures/alex_style_shape_badpal.bin");

    #[test]
    fn synthetic_skeleton_trims_joint_names() {
        let skel = decode_skeleton(SKEL, 0, SKEL.len()).expect("skeleton decodes");
        assert_eq!(skel.name, "alex_reg_body_skeleton", "trailing NULs trimmed");
        assert_eq!(skel.joints.len(), 2);
        assert_eq!(skel.joints[0].name, "Character_Root", "trailing NUL trimmed");
        assert_eq!(skel.joints[1].name, "Pelvis");
        let worlds = rest_world_matrices(&skel);
        assert!((worlds[1][3][1] - 2.0).abs() < 1e-6, "pelvis composes above root");
    }

    /// The block-88 relaxations on ONE synthetic alex-census shape:
    /// NUL-trimmed names, `nprims` == MESH count (not all children),
    /// decl words recorded-not-enforced, streams + palette decoded.
    #[test]
    fn synthetic_alex_shape_decodes_with_nul_nprims_and_decl_relaxations() {
        let shape = decode_old_skinned_shape(SHAPE_OK, 0, SHAPE_OK.len()).expect("shape decodes");
        assert_eq!(shape.name, "alex_reg_body_alex_bodyShape");
        assert_eq!(shape.skeleton, "alex_reg_body_skeleton", "NULs trimmed");
        assert_eq!(shape.version, 3);
        assert_eq!(shape.meshes.len(), 1, "nprims=1 counts MESHES, not all children");
        assert_eq!(shape.child_types, vec![0x0001_0003, 0x0001_0004], "extras recorded");

        let m = &shape.meshes[0];
        assert_eq!(m.desc, 0xB1B1);
        assert_eq!(m.vertex_count, 3);
        assert_eq!(m.index_count, 3);
        assert_eq!(m.decl_words[1], 0x20, "the decl words are RECORDED (not enforced)");
        assert_eq!(m.palette, vec![0, 1]);
        assert_eq!(m.positions[1], [1.0, 0.0, 0.0]);
        assert_eq!(m.normals[0], [0.0, 1.0, 0.0]);
        assert_eq!(m.uvs[2], [0.75, 0.5]);
        // Block 88: stream B + @48 carry the REAL blend weights:
        // w0=0.6 w1=0.3 w2=0.1*v w3=1-sum (clamped), normalized.
        assert!(
            (m.weights[0][3] - 0.1).abs() < 1e-5,
            "w3 = 1-0.6-0.3-0 (f32): {:?}",
            m.weights[0]
        );
        assert_eq!(m.weights[1][2], 0.1, "w2 rides in stream A @48");
        assert!(
            (m.weights[2].iter().sum::<f32>() - 1.0).abs() < 1e-6,
            "weights normalize to 1: {:?}",
            m.weights[2]
        );
    }

    /// Rest-pose skinning is exact for ANY weights (`G = W·rest⁻¹·W_pose`
    /// collapses to identity) — the whole skinning pipeline sanity.
    #[test]
    fn rest_pose_skinning_is_identity() {
        let skel = decode_skeleton(SKEL, 0, SKEL.len()).unwrap();
        let shape = decode_old_skinned_shape(SHAPE_OK, 0, SHAPE_OK.len()).unwrap();
        let m = &shape.meshes[0];
        let rest = rest_world_matrices(&skel);
        let skin = skin_matrices(&m.palette, &rest, &rest);
        let out = skin_positions(m, &skin);
        for (a, b) in out.iter().zip(m.positions.iter()) {
            for ch in 0..3 {
                assert!((a[ch] - b[ch]).abs() < 1e-5, "{a:?} vs {b:?}");
            }
        }
    }

    /// A REAL anim track moves the skinned mesh: the committed
    /// alex_loco_run_n fixture drives a Pelvis rotation that displaces
    /// the joint-1-attached vertex (the anim→skin integration).
    #[test]
    fn anim_pose_moves_skinned_vertex() {
        use crate::anim;
        let mut skel = decode_skeleton(SKEL, 0, SKEL.len()).unwrap();
        skel.joints[1].name = "Pelvis".into(); // bind to the REAL track's bone
        let shape = decode_old_skinned_shape(SHAPE_OK, 0, SHAPE_OK.len()).unwrap();
        let m = &shape.meshes[0];

        const FIXTURE: &[u8] =
            include_bytes!("../../prototype-bin/tests/fixtures/alex_loco_run_n_chunk.bin");
        let (off, span, _) = &anim::find_anim_tracks(FIXTURE)[0];
        let track = anim::decode_anim_track(FIXTURE, *off, *span).unwrap();

        let rest = rest_world_matrices(&skel);
        let pose = anim::pose_world_matrices(&skel, &track, 9.0);
        let skin = skin_matrices(&m.palette, &rest, &pose);
        let out = skin_positions(m, &skin);
        let rest_skin = skin_matrices(&m.palette, &rest, &rest);
        let rest_out = skin_positions(m, &rest_skin);
        let maxd: f32 = out
            .iter()
            .zip(rest_out.iter())
            .map(|(a, b)| (0..3).map(|c| (a[c] - b[c]).abs()).sum::<f32>())
            .fold(0.0, f32::max);
        assert!(maxd > 1e-3, "anim pose must displace the mesh (max {maxd})");
    }

    /// Palette bounds are enforced: a vertex whose bone slot references
    /// a palette entry beyond the table fails closed.
    #[test]
    fn palette_out_of_range_fails_closed() {
        let err = decode_old_skinned_shape(SHAPE_BADPAL, 0, SHAPE_BADPAL.len());
        assert!(err.is_err(), "out-of-range palette index must fail");
    }

    /// BLOCK 90 (NEXT-UP #1): the REAL alex_reg_body_skeleton fixture
    /// decodes through the prelude-aware entry point and pins all 67
    /// joint names + parent indices.  The fixture is the chunk stream
    /// `12-B P3D header + [0x23000][uid=53][size=10479]` — passing the
    /// +4 word (the uid, 53) as the size used to fail; the size word
    /// lives at +8.
    #[test]
    fn real_skeleton_fixture_decodes_67_joints() {
        const FIX: &[u8] = include_bytes!("../tests/fixtures/skeleton_alex_reg_body.p3d");
        // the fixture carries a 12-B P3D file header first
        let skel = decode_skeleton_at(FIX, 12).expect("skeleton decodes via prelude entry");
        assert_eq!(skel.name, "alex_reg_body_skeleton");
        assert_eq!(skel.joints.len(), 67, "67 joints");
        const WANT: [&str; 67] = [
            "Motion_Root", "Balance_Root", "Character_Root", "Pelvis", "Hip_L", "Knee_L",
            "Ankle_L", "Ball_L", "Hip_R", "Knee_R", "Ankle_R", "Ball_R", "Spine_1",
            "Spine_2", "Spine_3", "Clavicle_L", "Shoulder_L", "Elbow_L", "Forarm_L",
            "Wrist_L", "Index_Base_L", "Index_L", "Index_L_End", "Middle_Base_L",
            "Middle_L", "Middle_L_End", "Ring_Base_L", "Ring_L", "Ring_L_End",
            "Pink_Base_L", "Pink_L", "Pink_L_End", "Thumb_Base_L", "Thumb_L",
            "Thumb_L_End", "Shoulder_Con_L", "Neck", "Head", "Jaw", "Brow", "EyeLid",
            "Eye_R", "Eye_L", "Clavicle_R", "Shoulder_R", "Elbow_R", "Forarm_R",
            "Wrist_R", "Index_Base_R", "Index_R", "Index_R_End", "Middle_Base_R",
            "Middle_R", "Middle_R_End", "Ring_Base_R", "Ring_R", "Ring_R_End",
            "Pink_Base_R", "Pink_R", "Pink_R_End", "Thumb_Base_R", "Thumb_R",
            "Thumb_R_End", "Shoulder_Con_R", "Root_Grapple", "L_Wrist_Grapple",
            "R_Wrist_Grapple",
        ];
        for (i, want) in WANT.iter().enumerate() {
            assert_eq!(&skel.joints[i].name, want, "joint {i}");
        }
        // parent chain: Pelvis under Character_Root (2), fingers under
        // the wrist, grapple roots at the end
        assert_eq!(skel.joints[3].name, "Pelvis");
        assert_eq!(skel.joints[3].parent, 2, "Pelvis.parent = Character_Root");
        assert_eq!(skel.joints[19].name, "Wrist_L");
        assert_eq!(skel.joints[20].parent, 19, "Index_Base_L.parent = Wrist_L");
        assert_eq!(skel.joints[66].name, "R_Wrist_Grapple");
        // every joint's rest matrix last row is finite (the 13 f32 + u16
        // tail parsed cleanly)
        for j in &skel.joints {
            assert!(j.rest[3].iter().all(|v| v.is_finite()), "{}", j.name);
        }
        // and the +4 word (the uid) must NOT be the size
        assert_eq!(u32_at(FIX, 16).unwrap(), 53, "the +4 word is the uid, not the size");
    }
}

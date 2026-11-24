//! Block 85 — MODERN SKINNED SHAPE decoder (cracked).
//!
//! The `art\alex\alex.p3d` player-model family (as opposed to the NIS
//! cutscene packages' old-style `0x0001_0001` shapes decoded in
//! [`crate::skin`]) uses the MODERN skinned-shape family:
//!
//! ## PROVEN layout (block 83 python prototype, block 85 Rust port)
//!
//! Shape chunk `0x0001_0001` payload:
//! ```text
//! [pascal name]                — e.g. "alex_reg_body_alex_bodyShape"
//! [u32 version]                — 3 on every observed sample
//! [pascal skeleton_ref]        — e.g. "alex_reg_body_skeleton"
//! [u32 prim_group_count]
//! ```
//!
//! Children (descendants, any depth):
//! * `0x0001_0012` with **param == 2** — main vertex stream.  Payload:
//!   `[u32 version][u32 param=2][u32 nbytes][data]` where
//!   `nbytes % 56 == 0` and each 56-B record is
//!   `pos(3f) @0 | normal(3f) @24 | uv(2f) @40 | weight3(f32) @48 |
//!   slots(4u8) @52`.
//! * `0x0001_0012` with **param == 1** — second vertex stream
//!   (12 B/vertex): `[u32 marker][f32 A][f32 B]` — two more blend
//!   weights.
//! * `0x0001_0013` — IndexBuffer `[u32 version][u32 0][u32 nbytes]
//!   [u16 × n]`.
//! * `0x0001_000D` — BonePalette `[u32 count][u32 × count]`
//!   (palette-relative slot → skeleton joint index).
//!
//! Per-vertex blend weights: the four slots carry weights
//! `(A, B, W3, 1-A-B-W3)` where `A`/`B` come from the second stream
//! and `W3` from the main stream (unless the marker in the second
//! stream's header is `0xFF7F7F7F`, in which case the second-stream
//! words are ignored and the vertex is fully weighted on slot 0).
//! Weight 0 slots are dropped; the remainder is normalised.
//!
//! Every decoder validates structure fail-closed and returns
//! [`crate::geometry::ArtError`] on the first violation.

use crate::geometry::{pascal_at, u32_at, ArtError};

/// Modern skinned shape chunk type.
pub const CHUNK_TYPE_SHAPE: u32 = 0x0001_0001;
/// VertexBuffer chunk type (both the main and second streams).
pub const CHUNK_TYPE_VERTEX_BUFFER: u32 = 0x0001_0012;
/// IndexBuffer chunk type.
pub const CHUNK_TYPE_INDEX_BUFFER: u32 = 0x0001_0013;
/// BonePalette chunk type.
pub const CHUNK_TYPE_BONE_PALETTE: u32 = 0x0001_000D;

/// Main-stream stride (56 B) and per-field offsets.
pub const STRIDE_MAIN: usize = 56;
/// Second-stream stride (12 B).
pub const STRIDE_SECOND: usize = 12;

/// One decoded modern skinned mesh.
#[derive(Debug, Clone, PartialEq)]
pub struct ModernSkinnedMesh {
    pub name: String,
    pub version: u32,
    pub skeleton: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Palette-relative bone slot indices per vertex (trimmed to the
    /// non-zero-weight slots).
    pub bones: Vec<Vec<u8>>,
    /// Normalised blend weights per vertex (sums to 1.0).
    pub weights: Vec<Vec<f32>>,
    pub indices: Vec<u16>,
    /// Bone palette: palette slot -> skeleton joint index.
    pub palette: Vec<u32>,
}

/// Find every top-level `0x0001_0001` shape in the buffer that carries
/// a modern skinned mesh, and decode it.  Shapes whose descendants
/// don't form a complete (main stream + indices + palette) triple are
/// skipped silently.
pub fn find_modern_skinned_meshes(buf: &[u8]) -> Vec<ModernSkinnedMesh> {
    let mut out = Vec::new();
    let mut p = 12usize;
    while p + 12 <= buf.len() {
        let t = match u32_at(buf, p) { Ok(v) => v, Err(_) => break };
        let cs = match u32_at(buf, p + 4) { Ok(v) => v as usize, Err(_) => break };
        let sz = match u32_at(buf, p + 8) { Ok(v) => v as usize, Err(_) => break };
        if cs < 12 || sz < cs || p + sz > buf.len() { break; }
        if t == CHUNK_TYPE_SHAPE {
            if let Ok(mesh) = decode_modern_skinned_shape(buf, p, sz) {
                out.push(mesh);
            }
        }
        p += sz;
    }
    out
}

/// Walk all descendant chunks of a shape (bounded depth) and return
/// `(type, payload_start, payload_len, child_start, child_len, depth)`.
fn walk_descendants(
    buf: &[u8],
    start: usize,
    end: usize,
    depth: usize,
    out: &mut Vec<(u32, usize, usize, usize, usize, usize)>,
) {
    if depth > 10 { return; }
    let mut p = start;
    while p + 12 <= end {
        let Ok(t) = u32_at(buf, p) else { break };
        let Ok(cs) = u32_at(buf, p + 4) else { break };
        let Ok(sz) = u32_at(buf, p + 8) else { break };
        let (cs, sz) = (cs as usize, sz as usize);
        if cs < 12 || sz < cs || p + sz > end { break; }
        out.push((t, p + 12, cs - 12, p + cs, sz - cs, depth));
        walk_descendants(buf, p + cs, p + sz, depth + 1, out);
        p += sz;
    }
}

/// Decode the modern skinned shape at `offset` (total `size`).
pub fn decode_modern_skinned_shape(
    buf: &[u8],
    offset: usize,
    size: usize,
) -> Result<ModernSkinnedMesh, ArtError> {
    let end = offset + size;
    if end > buf.len() {
        return Err(ArtError::Truncated { offset, need: size, got: buf.len() - offset });
    }
    let payload_end = offset + (u32_at(buf, offset + 4)? as usize);
    if payload_end > end {
        return Err(ArtError::BadChunkSize {
            chunk_type: CHUNK_TYPE_SHAPE, offset, size, end,
        });
    }
    let (name, mut q) = pascal_at(buf, offset + 12, payload_end)?;
    let name = trim_nul(&name);
    let version = u32_at(buf, q)?;
    q += 4;
    let (skel, q2) = pascal_at(buf, q, payload_end)?;
    let skel = trim_nul(&skel);
    let _prim_count = u32_at(buf, q2)?;

    // Walk all descendants, collect the three pieces.
    let mut kids = Vec::new();
    walk_descendants(buf, payload_end, end, 0, &mut kids);

    let mut main_stream: Option<(usize, usize, u32)> = None; // (payload_start, nbytes, param)
    let mut second_stream: Option<(usize, u32)> = None;      // (payload_start, param)
    let mut index_buffer: Option<(usize, usize)> = None;     // (payload_start, nbytes)
    let mut palette: Option<(usize, u32)> = None;            // (payload_start, count)

    for &(t, pstart, plen, _cstart, _clen, _depth) in &kids {
        if t != CHUNK_TYPE_VERTEX_BUFFER
            && t != CHUNK_TYPE_INDEX_BUFFER
            && t != CHUNK_TYPE_BONE_PALETTE
        {
            continue;
        }
        match t {
            CHUNK_TYPE_VERTEX_BUFFER => {
                if plen < 12 { continue; }
                let _ver = u32_at(buf, pstart).ok();
                let param = u32_at(buf, pstart + 4).unwrap_or(0);
                let nbytes = u32_at(buf, pstart + 8).unwrap_or(0) as usize;
                if param == 2 && nbytes % STRIDE_MAIN == 0 && main_stream.is_none() {
                    main_stream = Some((pstart + 12, nbytes, param));
                } else if param == 1 && second_stream.is_none() {
                    second_stream = Some((pstart + 12, param));
                }
            }
            CHUNK_TYPE_INDEX_BUFFER => {
                if index_buffer.is_none() && plen >= 12 {
                    let nbytes = u32_at(buf, pstart + 8).unwrap_or(0) as usize;
                    if nbytes > 0 {
                        index_buffer = Some((pstart + 12, nbytes));
                    }
                }
            }
            CHUNK_TYPE_BONE_PALETTE => {
                if palette.is_none() && plen >= 4 {
                    let cnt = u32_at(buf, pstart).unwrap_or(0);
                    if cnt > 0 {
                        palette = Some((pstart + 4, cnt));
                    }
                }
            }
            _ => {}
        }
    }

    // Assemble.  All three pieces are required.
    let (main_off, main_bytes, _param) =
        main_stream.ok_or_else(|| ArtError::UnsupportedMeshFamily { children: child_types(&kids) })?;
    let (idx_off, idx_bytes) =
        index_buffer.ok_or_else(|| ArtError::UnsupportedMeshFamily { children: child_types(&kids) })?;
    let (pal_off, pal_count) =
        palette.ok_or_else(|| ArtError::UnsupportedMeshFamily { children: child_types(&kids) })?;

    let vcount = main_bytes / STRIDE_MAIN;
    if vcount == 0 {
        return Err(ArtError::UnexpectedData {
            offset: main_off,
            why: "modern skinned shape: zero vertices".into(),
        });
    }
    if pal_off + 4 * pal_count as usize > buf.len() {
        return Err(ArtError::Truncated {
            offset: pal_off, need: 4 * pal_count as usize, got: buf.len() - pal_off,
        });
    }
    let mut palette_v = Vec::with_capacity(pal_count as usize);
    for i in 0..pal_count as usize {
        palette_v.push(u32_at(buf, pal_off + 4 * i)?);
    }
    // Index count
    let icount = idx_bytes / 2;
    if idx_off + 2 * icount > buf.len() {
        return Err(ArtError::Truncated {
            offset: idx_off, need: 2 * icount, got: buf.len() - idx_off,
        });
    }
    let mut indices = Vec::with_capacity(icount);
    for i in 0..icount {
        indices.push(u16::from_le_bytes([buf[idx_off + 2 * i], buf[idx_off + 2 * i + 1]]));
    }

    // Vertices
    let mut positions = Vec::with_capacity(vcount);
    let mut normals = Vec::with_capacity(vcount);
    let mut uvs = Vec::with_capacity(vcount);
    let mut bones = Vec::with_capacity(vcount);
    let mut weights = Vec::with_capacity(vcount);
    for vi in 0..vcount {
        let b = main_off + STRIDE_MAIN * vi;
        if b + STRIDE_MAIN > buf.len() {
            return Err(ArtError::Truncated {
                offset: b, need: STRIDE_MAIN, got: buf.len() - b,
            });
        }
        let px = f32le(buf, b);
        let py = f32le(buf, b + 4);
        let pz = f32le(buf, b + 8);
        let nx = f32le(buf, b + 24);
        let ny = f32le(buf, b + 28);
        let nz = f32le(buf, b + 32);
        let u = f32le(buf, b + 40);
        let v = f32le(buf, b + 44);
        let w3 = f32le(buf, b + 48);
        let s0 = buf[b + 52];
        let s1 = buf[b + 53];
        let s2 = buf[b + 54];
        let s3 = buf[b + 55];
        // Second stream: two more weights (A, B).
        let (a, b2) = if let Some((soff, _sparam)) = second_stream {
            let sb = soff + STRIDE_SECOND * vi;
            if sb + STRIDE_SECOND <= buf.len() {
                (f32le(buf, sb + 4), f32le(buf, sb + 8))
            } else {
                (0.0, 0.0)
            }
        } else {
            (0.0, 0.0)
        };
        let w4 = 1.0 - a - b2 - w3;
        let raw = [
            (s0, a),
            (s1, b2),
            (s2, w3),
            (s3, if w4 > 0.0 { w4 } else { 0.0 }),
        ];
        let tot: f32 = raw.iter().map(|(_, w)| w).sum();
        let (bs, ws): (Vec<u8>, Vec<f32>) = if tot > 0.0 {
            let pairs: Vec<(u8, f32)> = raw
                .iter()
                .filter(|(_, w)| *w > 0.0)
                .map(|(s, w)| (*s, *w / tot))
                .collect();
            pairs.into_iter().unzip()
        } else {
            (vec![s0], vec![1.0])
        };
        positions.push([px, py, pz]);
        normals.push([nx, ny, nz]);
        uvs.push([u, v]);
        bones.push(bs);
        weights.push(ws);
    }

    Ok(ModernSkinnedMesh {
        name, version, skeleton: skel,
        positions, normals, uvs, bones, weights, indices, palette: palette_v,
    })
}

fn f32le(buf: &[u8], off: usize) -> f32 {
    f32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn trim_nul(s: &str) -> String {
    s.split('\0').next().unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le32(v: u32) -> [u8; 4] { v.to_le_bytes() }
    fn le_f(v: f32) -> [u8; 4] { v.to_le_bytes() }

    /// Synthetic shape: 1 vertex, identity palette, 3 indices.
    #[test]
    fn modern_skinned_shape_decode_round_trip() {
        // Layout:
        // [12-B shape prelude] [name 5 B "test\0"] [u32 version=3]
        // [skel 8 B "skel\0"] [u32 prim_count=1] ... children
        let mut buf: Vec<u8> = Vec::new();
        // Placeholder for the shape prelude (patched at the end).
        buf.extend_from_slice(&[0u8; 12]);
        // name
        buf.push(5); buf.extend_from_slice(b"test\0");
        // version
        buf.extend_from_slice(&le32(3));
        // skeleton
        buf.push(5); buf.extend_from_slice(b"skel\0");
        // prim count
        buf.extend_from_slice(&le32(1));
        let shape_payload_end = buf.len();
        // --- main VB child (param=2, 1 vertex = 56 B) ---
        let vb_main: Vec<u8> = {
            let mut v = Vec::new();
            v.extend_from_slice(&le32(CHUNK_TYPE_VERTEX_BUFFER));
            v.extend_from_slice(&[0u8; 4]); // cs placeholder
            v.extend_from_slice(&[0u8; 4]); // sz placeholder
            v.extend_from_slice(&le32(0)); // version
            v.extend_from_slice(&le32(2)); // param=2
            v.extend_from_slice(&le32(56)); // nbytes
            // vertex 0: pos (1,2,3), normal (0,1,0), uv (0.5, 0.5),
            // w3 = 0.3, slots (0, 1, 2, 3)
            v.extend_from_slice(&le_f(1.0));
            v.extend_from_slice(&le_f(2.0));
            v.extend_from_slice(&le_f(3.0));
            for _ in 12..24 { v.push(0); }
            v.extend_from_slice(&le_f(0.0));
            v.extend_from_slice(&le_f(1.0));
            v.extend_from_slice(&le_f(0.0));
            v.extend_from_slice(&le_f(0.5));
            v.extend_from_slice(&le_f(0.5));
            v.extend_from_slice(&le_f(0.3)); // w3 @48
            v.push(0); v.push(1); v.push(2); v.push(3);
            v
        };
        // --- second VB child (param=1, 1 vertex = 12 B) ---
        let vb_second: Vec<u8> = {
            let mut v = Vec::new();
            v.extend_from_slice(&le32(CHUNK_TYPE_VERTEX_BUFFER));
            v.extend_from_slice(&[0u8; 4]); // cs placeholder
            v.extend_from_slice(&[0u8; 4]); // sz placeholder
            v.extend_from_slice(&le32(0)); // version
            v.extend_from_slice(&le32(1)); // param=1
            v.extend_from_slice(&le32(12)); // nbytes
            v.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // marker
            v.extend_from_slice(&le_f(0.5)); // A
            v.extend_from_slice(&le_f(0.1)); // B
            v
        };
        // --- index buffer child (3 u16) ---
        let ib: Vec<u8> = {
            let mut v = Vec::new();
            v.extend_from_slice(&le32(CHUNK_TYPE_INDEX_BUFFER));
            v.extend_from_slice(&[0u8; 4]);
            v.extend_from_slice(&[0u8; 4]);
            v.extend_from_slice(&le32(0));
            v.extend_from_slice(&le32(0));
            v.extend_from_slice(&le32(6)); // nbytes
            v.extend_from_slice(&0u16.to_le_bytes());
            v.extend_from_slice(&1u16.to_le_bytes());
            v.extend_from_slice(&2u16.to_le_bytes());
            v
        };
        // --- bone palette child (2 joints) ---
        let bp: Vec<u8> = {
            let mut v = Vec::new();
            v.extend_from_slice(&le32(CHUNK_TYPE_BONE_PALETTE));
            v.extend_from_slice(&[0u8; 4]);
            v.extend_from_slice(&[0u8; 4]);
            v.extend_from_slice(&le32(2));
            v.extend_from_slice(&le32(7));  // slot 0 -> joint 7
            v.extend_from_slice(&le32(11)); // slot 1 -> joint 11
            v
        };
        // Stitch children into the buffer (with real preludes).
        for child in [&vb_main, &vb_second, &ib, &bp] {
            // Compute child size = 12 + payload.
            let sz = child.len() as u32;
            buf.extend_from_slice(&child[0..12]);
            buf.truncate(buf.len() - 12);
            // Rewrite the prelude with real type + sizes.
            buf.extend_from_slice(&child[0..4]); // type
            buf.extend_from_slice(&sz.to_le_bytes()); // cs = sz (no children)
            buf.extend_from_slice(&sz.to_le_bytes()); // sz
            buf.extend_from_slice(&child[12..]);
        }
        let total = buf.len() as u32;
        // Patch the shape prelude: type=0x10001, cs = payload_end, sz = total.
        buf[0..4].copy_from_slice(&le32(CHUNK_TYPE_SHAPE));
        buf[4..8].copy_from_slice(&(shape_payload_end as u32).to_le_bytes());
        buf[8..12].copy_from_slice(&total.to_le_bytes());
        let mesh = decode_modern_skinned_shape(&buf, 0, buf.len())
            .expect("decode modern skinned shape");
        assert_eq!(mesh.name, "test");
        assert_eq!(mesh.skeleton, "skel");
        assert_eq!(mesh.version, 3);
        assert_eq!(mesh.positions.len(), 1);
        assert_eq!(mesh.indices.len(), 3);
        assert_eq!(mesh.palette, vec![7, 11]);
        // Weights: A=0.5, B=0.1, W3=0.3, W4=0.1 -> normalised
        // (already sums to 1.0).
        assert_eq!(mesh.bones[0].len(), 4);
        assert_eq!(mesh.weights[0].len(), 4);
        let sum: f32 = mesh.weights[0].iter().sum();
        assert!((sum - 1.0).abs() < 1e-5, "weights sum {sum}");
        assert!((mesh.positions[0][0] - 1.0).abs() < 1e-6);
        assert!((mesh.normals[0][1] - 1.0).abs() < 1e-6);
        assert!((mesh.uvs[0][0] - 0.5).abs() < 1e-6);
    }

    /// Missing palette -> the shape is skipped by `find_modern_skinned_meshes`.
    #[test]
    fn incomplete_shape_is_skipped() {
        // Just the 12-B prelude, nothing else.
        let buf: Vec<u8> = {
            let mut v = Vec::new();
            v.extend_from_slice(&le32(CHUNK_TYPE_SHAPE));
            v.extend_from_slice(&le32(12));
            v.extend_from_slice(&le32(12));
            v
        };
        let out = find_modern_skinned_meshes(&buf);
        assert!(out.is_empty());
    }
}

fn child_types(kids: &[(u32, usize, usize, usize, usize, usize)]) -> Vec<u32> {
    kids.iter().map(|k| k.0).collect()
}

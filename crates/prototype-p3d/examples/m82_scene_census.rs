//! Block 82: census the NIS_0_3_a.p3d SCENE FILE (the morgue set).
//!
//! Walks every top-level chunk, decodes every Shape (0x00010000),
//! and prints per-mesh stats: name, vertex count, index count, tri
//! count, world-space bounds. The goal: find the CHARACTER meshes
//! (Alex Mercer) embedded in the cutscene's own scene data.

use std::io::Cursor;

fn main() {
    let path = std::env::args().nth(1).expect("usage: m82_scene_census <file.p3d>");
    let buf = std::fs::read(&path).expect("read p3d");
    println!("file: {path} ({} B)", buf.len());

    let top = prototype_p3d::geometry::top_level_chunks(&buf).expect("top-level walk");
    println!("top-level chunks: {}", top.len());

    // Type census first.
    let mut types: Vec<(u32, usize)> = Vec::new();
    for c in &top {
        match types.iter_mut().find(|(t, _)| *t == c.chunk_type) {
            Some((_, n)) => *n += 1,
            None => types.push((c.chunk_type, 1)),
        }
    }
    types.sort();
    for (t, n) in &types {
        println!("  type {t:#010x} x{n}");
    }

    // Now decode every shape + mesh.
    let mut total_tris = 0usize;
    let mut mesh_no = 0usize;
    for c in top.iter().filter(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_SHAPE) {
        let shape = match prototype_p3d::geometry::decode_shape(&buf, c.offset, c.size) {
            Ok(s) => s,
            Err(e) => {
                println!("  SHAPE decode FAIL at {}: {e}", c.offset);
                continue;
            }
        };
        println!(
            "SHAPE \"{}\" version {} flag {} meshes {} bbox {:?}",
            shape.name,
            shape.version,
            shape.flag,
            shape.meshes.len(),
            shape.bound_box.map(|b| [b[0], b[1], b[2], b[3], b[4], b[5]]),
        );
        for m in &shape.meshes {
            mesh_no += 1;
            let tris = m.header.index_count as usize / 3;
            total_tris += tris;
            let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &m.positions {
                for ch in 0..3 {
                    mn[ch] = mn[ch].min(p[ch]);
                    mx[ch] = mx[ch].max(p[ch]);
                }
            }
            let extent = [mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]];
            let has_uv = !m.uvs.is_empty();
            let has_n = !m.normals.is_empty();
            println!(
                "  MESH[{mesh_no}] \"{}\" verts {} idx {} tris {tris} uv {has_uv} n {has_n} extent [{:.2}, {:.2}, {:.2}] min [{:.2} {:.2} {:.2}]",
                m.name,
                m.header.vertex_count,
                m.header.index_count,
                extent[0],
                extent[1],
                extent[2],
                mn[0],
                mn[1],
                mn[2],
            );
        }
    }
    let _ = Cursor::new(&buf);
    println!("TOTAL: {mesh_no} meshes, {total_tris} triangles");
}

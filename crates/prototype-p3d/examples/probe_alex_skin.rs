use prototype_p3d::{geometry::top_level_chunks, skin::decode_old_skinned_shape, skin::CHUNK_TYPE_OLD_SHAPE};

fn main() {
    let path = std::env::args().nth(1).expect("usage");
    let buf = std::fs::read(&path).expect("read");
    let top = top_level_chunks(&buf).expect("top");
    for c in &top {
        if c.chunk_type != CHUNK_TYPE_OLD_SHAPE { continue; }
        if let Ok(s) = decode_old_skinned_shape(&buf, c.offset, c.size) {
            for m in &s.meshes {
                let max_slot = m.bone_indices.iter().flatten().max().copied().unwrap_or(0);
                let over: usize = m.bone_indices.iter().flatten().filter(|&&b| b as usize >= m.palette.len()).count();
                println!("{:?} skel={:?}: verts={} pal={} max_slot={} over_palette={} uvs={} normals={}",
                    m.name, s.skeleton, m.vertex_count, m.palette.len(), max_slot, over,
                    m.uvs.len(), m.normals.len());
            }
        }
    }
}

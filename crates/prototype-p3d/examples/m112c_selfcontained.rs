//! m112c probe: which fetched geometry member is the most
//! self-contained (its own mesh color values resolve against its own
//! texture chunks)?  Ranks all 20 by in-member resolution rate and
//! tri count — the pick for the textured world take.

use prototype_p3d::geometry::{
    top_level_chunks, decode_shape, decode_shader_chunk, decode_texture, CHUNK_TYPE_SHAPE,
    CHUNK_TYPE_SHADER, CHUNK_TYPE_TEXTURE,
};

fn main() {
    let dir = "/home/z/my-project/source/cells_world";
    let mut rows: Vec<(String, usize, usize, usize, usize)> = Vec::new(); // name, meshes, hit, tris, texcount
    for entry in std::fs::read_dir(dir).expect("dir") {
        let path = entry.expect("e").path();
        if path.extension().and_then(|e| e.to_str()) != Some("p3d") {
            continue;
        }
        let buf = std::fs::read(&path).expect("member");
        if buf.len() <= 12 {
            continue;
        }
        let tops = match top_level_chunks(&buf) {
            Some(t) => t,
            None => continue,
        };
        let mut tex_names: std::collections::HashSet<String> = Default::default();
        let mut texcount = 0usize;
        for c in &tops {
            if c.chunk_type == CHUNK_TYPE_TEXTURE {
                texcount += 1;
                if let Ok(t) = decode_texture(&buf, c.offset, c.size) {
                    tex_names.insert(t.name.to_lowercase());
                }
            }
        }
        let mut shader_color: std::collections::HashMap<String, String> = Default::default();
        for c in &tops {
            if c.chunk_type == CHUNK_TYPE_SHADER {
                if let Ok(s) = decode_shader_chunk(&buf, c.offset, c.size) {
                    for p in &s.params {
                        if let (Some(pn), Some(pv)) = (&p.name, &p.value) {
                            if pn.eq_ignore_ascii_case("color") {
                                shader_color.insert(s.name.clone(), pv.clone());
                            }
                        }
                    }
                }
            }
        }
        let mut meshes = 0usize;
        let mut hit = 0usize;
        let mut tris = 0usize;
        for c in &tops {
            if c.chunk_type != CHUNK_TYPE_SHAPE {
                continue;
            }
            if let Ok(shape) = decode_shape(&buf, c.offset, c.size) {
                for m in &shape.meshes {
                    tris += m.indices.len() / 3;
                    if let Some(color) = shader_color.get(&m.name) {
                        meshes += 1;
                        if tex_names.contains(&color.to_lowercase()) {
                            hit += 1;
                        }
                    }
                }
            }
        }
        if meshes > 0 {
            rows.push((
                path.file_name().unwrap().to_string_lossy().to_string(),
                meshes,
                hit,
                tris,
                texcount,
            ));
        }
    }
    rows.sort_by(|a, b| (b.2 * 1000 / b.1.max(1)).cmp(&(a.2 * 1000 / a.1.max(1))));
    println!("member | meshes | hit | tris | textures");
    for r in &rows {
        println!("{} | {} | {} | {} | {}", r.0, r.1, r.2, r.3, r.4);
    }
}

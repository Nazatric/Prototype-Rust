//! m112b probe: multi-member texture coverage for the be6215bf take.
//! Builds texture-name index across ALL fetched cells_world members
//! and reports how many of be6215bf's 91 mesh color values would
//! resolve against the union.

use prototype_p3d::geometry::{
    top_level_chunks, decode_shape, decode_shader_chunk, decode_texture, CHUNK_TYPE_SHAPE,
    CHUNK_TYPE_SHADER, CHUNK_TYPE_TEXTURE,
};

fn main() {
    let dir = "/home/z/my-project/source/cells_world";
    let target = format!("{dir}/be6215bf.p3d");

    let mut tex_names: std::collections::HashSet<String> = Default::default();
    let mut per_member = Vec::new();
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
        let mut n = 0usize;
        for c in &tops {
            if c.chunk_type == CHUNK_TYPE_TEXTURE {
                if let Ok(t) = decode_texture(&buf, c.offset, c.size) {
                    tex_names.insert(t.name.to_lowercase());
                    n += 1;
                }
            }
        }
        if n > 0 {
            per_member.push((path.file_name().unwrap().to_string_lossy().to_string(), n));
        }
    }
    per_member.sort();
    println!("texture inventory: {} distinct names across members", tex_names.len());

    // be6215bf mesh color values
    let buf = std::fs::read(&target).expect("target member");
    let tops = top_level_chunks(&buf).expect("p3d");
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
    let mut total = 0usize;
    let mut hit = 0usize;
    let mut misses: Vec<String> = Vec::new();
    for c in &tops {
        if c.chunk_type != CHUNK_TYPE_SHAPE {
            continue;
        }
        if let Ok(shape) = decode_shape(&buf, c.offset, c.size) {
            for m in &shape.meshes {
                if let Some(color) = shader_color.get(&m.name) {
                    total += 1;
                    if tex_names.contains(&color.to_lowercase()) {
                        hit += 1;
                    } else if misses.len() < 12 {
                        misses.push(color.clone());
                    }
                }
            }
        }
    }
    println!("mesh color values: {total}, resolved against union: {hit}");
    println!("miss samples: {misses:?}");
}

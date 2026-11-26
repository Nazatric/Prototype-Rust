//! m112 probe: why does the mesh→shader→color chain resolve only 3/91
//! diffuses in the real geometry member be6215bf?  Prints, via the
//! PROVEN Rust decoders: how many 0x11015 chunks decode, their param
//! name/value inventory, how many mesh names exactly match a shader
//! name, and which texture names the color params point at.

use prototype_p3d::geometry::{
    top_level_chunks, decode_shape, decode_shader_chunk, CHUNK_TYPE_MESH, CHUNK_TYPE_SHAPE,
    CHUNK_TYPE_SHADER,
};

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/home/z/my-project/source/cells_world/be6215bf.p3d".into());
    let buf = std::fs::read(&path).expect("member");
    let tops = top_level_chunks(&buf).expect("p3d");

    let mut shader_ok = 0usize;
    let mut shader_fail = 0usize;
    let mut param_names: std::collections::BTreeMap<String, usize> = Default::default();
    let mut shader_names: std::collections::HashSet<String> = Default::default();
    let mut color_values: Vec<String> = Vec::new();
    for c in &tops {
        if c.chunk_type != CHUNK_TYPE_SHADER {
            continue;
        }
        match decode_shader_chunk(&buf, c.offset, c.size) {
            Ok(s) => {
                shader_ok += 1;
                shader_names.insert(s.name.clone());
                for p in &s.params {
                    if let Some(pn) = &p.name {
                        *param_names.entry(pn.clone()).or_default() += 1;
                        if let Some(pv) = &p.value {
                            if pn.eq_ignore_ascii_case("color") {
                                color_values.push(pv.clone());
                            }
                        }
                    }
                }
            }
            Err(_) => shader_fail += 1,
        }
    }
    println!(
        "shader chunks: {} decode ok, {} fail; distinct param names: {:?}",
        shader_ok,
        shader_fail,
        param_names.iter().take(20).collect::<Vec<_>>()
    );
    println!("color param values ({}): {:?}", color_values.len(), {
        let v = color_values.iter().take(10).cloned().collect::<Vec<_>>();
        v
    });

    // mesh names + their shader-name match rate
    let mut mesh_names = 0usize;
    let mut matched = 0usize;
    let mut sample = Vec::new();
    for c in &tops {
        if c.chunk_type != CHUNK_TYPE_SHAPE {
            continue;
        }
        if let Ok(shape) = decode_shape(&buf, c.offset, c.size) {
            for m in &shape.meshes {
                mesh_names += 1;
                if shader_names.contains(&m.name) {
                    matched += 1;
                } else if sample.len() < 8 {
                    sample.push(m.name.clone());
                }
            }
        }
    }
    println!("meshes: {mesh_names}, name-matched to a shader: {matched}");
    println!("unmatched mesh name samples: {sample:?}");

    let _ = CHUNK_TYPE_MESH;
}

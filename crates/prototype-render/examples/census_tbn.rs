//! census_tbn — measure |normal|, |tangent|, |extra4.xyz|, |extra3| over
//! every committed mesh fixture; identifies the lit-arm TBN basis rows.
use prototype_p3d::geometry as geo;
use std::path::Path;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("prototype-bin")
        .join("tests")
        .join("fixtures");
    let names = [
        "art_barrier_os002.p3d",
        "art_mesh_a031.p3d",
        "art_mesh_b031.p3d",
        "art_mesh_b032.p3d",
        "art_mesh_nouvmesh2010.p3d",
        "art_mesh_skin6000.p3d",
    ];
    for name in names {
        let p = dir.join(name);
        let Ok(buf) = std::fs::read(&p) else { continue };
        let Some(top) = geo::top_level_chunks(&buf) else { continue };
        for c in top.iter().filter(|c| c.chunk_type == geo::CHUNK_TYPE_SHAPE) {
            let Ok(shape) = geo::decode_shape(&buf, c.offset, c.size) else { continue };
            for m in &shape.meshes {
                let stat = |v: &[[f32; 3]]| -> (f32, f32, f32) {
                    let lens: Vec<f32> = v.iter().map(|x| (x[0] * x[0] + x[1] * x[1] + x[2] * x[2]).sqrt()).collect();
                    if lens.is_empty() { return (0.0, 0.0, 0.0); }
                    let min = lens.iter().cloned().fold(f32::MAX, f32::min);
                    let max = lens.iter().cloned().fold(f32::MIN, f32::max);
                    let avg = lens.iter().sum::<f32>() / lens.len() as f32;
                    (min, avg, max)
                };
                let e4: Vec<[f32; 3]> = m.extra4.iter().map(|x| [x[0], x[1], x[2]]).collect();
                println!(
                    "{} [desc {:#x}] verts {} n{:?} t{:?} e4{:?} e3{:?}",
                    name,
                    m.header.desc,
                    m.positions.len(),
                    stat(&m.normals),
                    stat(&m.tangents),
                    stat(&e4),
                    stat(&m.extra3),
                );
            }
        }
    }
}

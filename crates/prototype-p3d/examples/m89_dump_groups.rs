use prototype_p3d::anim::{find_anim_tracks, decode_anim_track};
fn main() {
    let buf = include_bytes!("../../../crates/prototype-bin/tests/fixtures/alex_loco_run_n_chunk.bin");
    let tracks = find_anim_tracks(buf);
    println!("tracks: {}", tracks.len());
    let (off, span, name) = &tracks[0];
    println!("track: {} off={} span={}", name, off, span);
    let t = decode_anim_track(buf, *off, *span).unwrap();
    println!("groups: {}", t.groups.len());
    for (i, g) in t.groups.iter().enumerate() {
        let r = if g.rot.is_some() {"R"} else {"-"};
        let tr = if g.tran.is_some() {"T"} else {"-"};
        println!("  {:2}: {:<32} {}{}", i, g.bone_name, r, tr);
    }
}

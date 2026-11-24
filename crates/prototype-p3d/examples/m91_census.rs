// block 91 probe: census of the real alex_loco_run_n fixture groups —
// how many bones carry rot / rot_compact / tran / dof, and the DOF
// curve endpoints — the constants the bevy-bridge tests pin.
use prototype_p3d::anim::{decode_anim_track, find_anim_tracks};

fn main() {
    let buf =
        include_bytes!("../../prototype-bin/tests/fixtures/alex_loco_run_n_chunk.bin");
    let tracks = find_anim_tracks(buf);
    let (off, span, name) = &tracks[0];
    let t = decode_anim_track(buf, *off, *span).unwrap();
    println!("track {} frames={} fps={} cyclic={}", name, t.frame_count, t.fps, t.cyclic);
    let (mut rot, mut compact, mut tran, mut dof, mut both, mut neither) = (0, 0, 0, 0, 0, 0);
    for g in &t.groups {
        let r = g.rot.is_some();
        let c = g.rot_compact.is_some();
        let tr = g.tran.is_some();
        let d = g.dof.is_some();
        if r { rot += 1; }
        if c { compact += 1; }
        if tr { tran += 1; }
        if d { dof += 1; }
        if r && c { both += 1; }
        if !r && !c && !tr && !d { neither += 1; }
        if d || tr || c {
            println!(
                "  {}: rot={} compact={} keys={} tran={} dof={}",
                g.bone_name, r, c,
                g.rot_compact.as_ref().or(g.rot.as_ref()).map(|x| x.keys.len()).unwrap_or(0),
                tr, d
            );
        }
    }
    println!("census: rot={} compact={} tran={} dof={} both={} neither={} groups={}",
        rot, compact, tran, dof, both, neither, t.groups.len());
    if let Some(g) = t.groups.iter().find(|g| g.dof.is_some()) {
        let d = g.dof.as_ref().unwrap();
        println!("dof bone={} dyn={} consts={:?} frames={:?} values={:?}",
            g.bone_name, d.dyn_index, d.constants, d.frames, d.values);
    }
}

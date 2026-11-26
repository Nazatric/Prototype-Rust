//! m107 (block 86): print every bone group of `alex_loco_run_n` with
//! its ROT/TRAN key counts, and cross-reference the arms skeleton's
//! joints to see which arm bones the run cycle does NOT drive.
use prototype_p3d::anim::{decode_anim_track, find_anim_tracks};

fn main() {
    let buf = match std::fs::read("/home/z/my-project/source/alex.p3d") {
        Ok(b) => b,
        Err(e) => {
            eprintln!("no source/alex.p3d ({e}) — fetch pipeline first");
            return;
        }
    };
    let tracks = find_anim_tracks(&buf);
    let Some(&(off, sz, _)) = tracks.iter().find(|t| t.2 == "alex_loco_run_n") else {
        eprintln!("alex_loco_run_n not found");
        return;
    };
    let run = match decode_anim_track(&buf, off, sz) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("decode failed: {e:?}");
            return;
        }
    };
    println!("=== alex_loco_run_n: {} groups ===", run.groups.len());
    for g in &run.groups {
        let rot = match &g.rot {
            Some(ch) => {
                format!("ROT {}idx/{}keys", ch.key_indices.len(), ch.keys.len())
            }
            None => "---".to_string(),
        };
        let tran = match &g.tran {
            Some(ch) => {
                format!("TRAN {}idx/{}keys", ch.key_indices.len(), ch.keys.len())
            }
            None => "---".to_string(),
        };
        println!("  {:<18} {rot:<18} {tran}", g.bone_name);
    }
}

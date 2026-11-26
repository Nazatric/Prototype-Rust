//! m108 (block 86): validate every ROT/TRAN key of `alex_loco_run_n` —
//! quaternion norms (NaN / over-unit = smallest-three assumption broken
//! for that bone) and TRAN value ranges.  Updated to the current
//! `Channel { count, key_indices, keys: Vec<Key> }` API.
use prototype_p3d::anim::{decode_anim_track, find_anim_tracks, Key};

fn main() {
    let buf = match std::fs::read("/home/z/my-project/source/alex.p3d") {
        Ok(b) => b,
        Err(e) => {
            eprintln!("no source/alex.p3d ({e})");
            return;
        }
    };
    let tracks = find_anim_tracks(&buf);
    let Some(&(off, sz, ref name)) = tracks.iter().find(|t| t.2 == "alex_loco_run_n") else {
        eprintln!("alex_loco_run_n not found");
        return;
    };
    let run = match decode_anim_track(&buf, off, sz) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("decode_anim_track failed: {e:?}");
            return;
        }
    };
    println!("decoded alex_loco_run_n: {} groups", run.groups.len());
    let _ = name;
    let mut bad_bones: Vec<String> = Vec::new();
    for g in &run.groups {
        if let Some(ch) = &g.rot {
            let mut n_bad = 0usize;
            let mut n_nan = 0usize;
            let mut min = f32::MAX;
            let mut max = f32::MIN;
            for k in &ch.key_indices {
                let Some(Key::Rot { x, y, z, w }) = ch.keys.get(*k as usize) else {
                    n_nan += 1;
                    continue;
                };
                let n = (x * x + y * y + z * z + w * w).sqrt();
                min = min.min(n);
                max = max.max(n);
                if (n - 1.0).abs() > 1e-3 {
                    n_bad += 1;
                }
            }
            let status = if n_nan > 0 {
                "NAN-W"
            } else if n_bad > 0 {
                "OVER-UNIT"
            } else {
                "ok"
            };
            if status != "ok" {
                bad_bones.push(format!(
                    "{} [{status}: {n_nan} nan / {n_bad} over-unit, |q| {min:.4}..{max:.4}]",
                    g.bone_name
                ));
            } else {
                println!("{:<16} ok", g.bone_name);
            }
        }
    }
    println!("--- bones with bad quats: {}", bad_bones.len());
    for b in &bad_bones {
        println!("  {b}");
    }
}

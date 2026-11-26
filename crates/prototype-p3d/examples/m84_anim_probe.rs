//! Block 84: smoke-test the new anim track decoder against the REAL
//! `alex.p3d` (12 191 912 B).  Reads every top-level `0x0012_1000`
//! chunk, decodes it via `prototype_p3d::anim::decode_anim_track`,
//! and prints a one-line summary; for `alex_loco_run_n` prints the
//! first 3 bone groups with their ROT/TRAN key counts and the first
//! key of each channel (so a human can eyeball the run cycle's
//! hip/spine translations and leg rotations).
//!
//! Run: `cargo run --release --example m84_anim_probe -- <alex.p3d>`

use std::env;
use std::fs;

use prototype_p3d::anim::{decode_anim_track, find_anim_tracks, Key};

fn main() {
    let path = env::args().nth(1).unwrap_or_else(|| {
        "/home/z/my-project/source/alex.p3d".to_string()
    });
    let buf = fs::read(&path).expect("read alex.p3d");
    println!("loaded {} ({} B)", path, buf.len());

    let tracks = find_anim_tracks(&buf);
    println!("top-level 0x0012_1000 anim tracks: {}", tracks.len());

    let mut ptrn = 0;
    let mut cam = 0;
    let mut exp = 0;
    for (_, _, name) in &tracks {
        // The proven 533 PTRN / 67 CAM / 34 EXP census (block 87).
        // Heuristic: PTRN names usually contain "_loco_" or end in
        // "_run"/"_walk"/etc; CAM names contain "CAM"; EXP are short.
        if name.contains("CAM") || name.starts_with("CAM_") {
            cam += 1;
        } else if name.starts_with("EXP") || name.contains("_exp_") {
            exp += 1;
        } else {
            ptrn += 1;
        }
    }
    println!("  PTRN pose: {ptrn}, CAM camera: {cam}, EXP expression: {exp}");

    // Find alex_loco_run_n specifically (the run cycle).
    if let Some(&(off, sz, _)) = tracks.iter().find(|t| t.2 == "alex_loco_run_n") {
        let run = match decode_anim_track(&buf, off, sz) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("decode_anim_track failed: {e:?}");
                std::process::exit(3);
            }
        };
        println!();
        println!("=== alex_loco_run_n ===");
        println!(
            "  kind={:?} frames={} fps={} cyclic={} groups={}",
            run.kind,
            run.frame_count,
            run.fps,
            run.cyclic,
            run.groups.len()
        );
        let mut with_rot = 0;
        let mut with_tran = 0;
        for g in &run.groups {
            if g.rot.is_some() { with_rot += 1; }
            if g.tran.is_some() { with_tran += 1; }
        }
        println!("  groups with ROT channel: {with_rot}");
        println!("  groups with TRAN channel: {with_tran}");

        // Print ALL group names for bone-matching debug.
        eprintln!("ALL GROUP NAMES ({}):", run.groups.len());
        for (i, g) in run.groups.iter().enumerate() {
            eprintln!("  [{}] {:?}", i, g.bone_name);
        }
        // Print first 3 groups in detail.
        for (i, g) in run.groups.iter().take(3).enumerate() {
            println!();
            println!("  group[{i}] bone = {:?}", g.bone_name);
            if let Some(ch) = &g.rot {
                println!("    ROT: count={} idx[0..3]={:?}", ch.count, &ch.key_indices[..ch.key_indices.len().min(3)]);
                if !ch.keys.is_empty() {
                    if let Some(Key::Rot { x, y, z, w }) = ch.keys.first().copied() {
                        let n = (x * x + y * y + z * z + w * w).sqrt();
                        println!("    ROT key[0] = (x={x:.4}, y={y:.4}, z={z:.4}, w={w:.4})  |q|={n:.4}");
                    }
                }
            }
            if let Some(ch) = &g.tran {
                println!("    TRAN: count={} idx[0..3]={:?}", ch.count, &ch.key_indices[..ch.key_indices.len().min(3)]);
                if !ch.keys.is_empty() {
                    if let Some(Key::Tran { x, y, z }) = ch.keys.first().copied() {
                        println!("    TRAN key[0] = ({x:.4}, {y:.4}, {z:.4})");
                    }
                }
            }
        }
    } else {
        eprintln!("alex_loco_run_n NOT FOUND in {path}");
        std::process::exit(2);
    }

    // DEBUG: dump the raw chunk structure around the first 0x121000
    // top-level chunk so we can see whether 0x121001 groups are direct
    // children or nested one level deeper.
    let mut p = 12usize;
    let mut dumped = 0;
    while p + 12 <= buf.len() && dumped < 3 {
        let t = u32_at_raw(&buf, p);
        let cs = u32_at_raw(&buf, p + 4) as usize;
        let sz = u32_at_raw(&buf, p + 8) as usize;
        if cs < 12 || sz < cs || p + sz > buf.len() { break; }
        if t == 0x121000 {
            // Try to decode this specific chunk and report any error.
            match decode_anim_track(&buf, p, sz) {
                Ok(trk) => {
                    let name = &trk.name;
                    if name.starts_with("alex_loco_run") || dumped == 0 {
                        println!();
                        println!("=== decode_anim_track @ {p:#x}: name={name:?} groups={}",
                            trk.groups.len());
                        if trk.groups.is_empty() {
                            // Dump the chunk structure
                            println!("  cs={cs:#x} sz={sz:#x}");
                            let payload_end = p + cs;
                            let child_off = payload_end;
                            let child_end = p + sz;
                            let mut q = child_off;
                            let mut k = 0;
                            while q + 12 <= child_end && k < 10 {
                                let ct = u32_at_raw(&buf, q);
                                let ccs = u32_at_raw(&buf, q + 4) as usize;
                                let csz = u32_at_raw(&buf, q + 8) as usize;
                                if ccs < 12 || csz < ccs || q + csz > child_end { break; }
                                println!("    child[{k}] type={ct:#x} cs={ccs:#x} sz={csz:#x} @ {q:#x}");
                                // For each child, dump its inner children if any.
                                if csz > ccs {
                                    let inner_off = q + ccs;
                                    let inner_end = q + csz;
                                    let mut qi = inner_off;
                                    let mut ki = 0;
                                    while qi + 12 <= inner_end && ki < 3 {
                                        let it = u32_at_raw(&buf, qi);
                                        let icc = u32_at_raw(&buf, qi + 4) as usize;
                                        let isz = u32_at_raw(&buf, qi + 8) as usize;
                                        if icc < 12 || isz < icc || qi + isz > inner_end { break; }
                                        println!("      inner[{ki}] type={it:#x} cs={icc:#x} sz={isz:#x} @ {qi:#x}");
                                        qi += isz;
                                        ki += 1;
                                    }
                                }
                                q += csz;
                                k += 1;
                            }
                        }
                        dumped += 1;
                    }
                }
                Err(e) => {
                    if dumped == 0 {
                        println!();
                        println!("=== decode_anim_track @ {p:#x}: ERROR {e}");
                        dumped += 1;
                    }
                }
            }
        }
        p += sz;
    }
}

fn u32_at_raw(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

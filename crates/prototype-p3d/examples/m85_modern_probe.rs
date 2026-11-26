//! Block 85: probe the MODERN skinned shapes in `alex.p3d` (12 191 912 B).
//! Decodes via `prototype_p3d::modern::find_modern_skinned_meshes`,
//! prints per-mesh vertex/index/palette counts and per-mesh skeleton
//! reference, and verifies that the bone palettes index valid joints
//! in the corresponding `0x0002_3000` Skeleton.
//!
//! Run: `cargo run --release --example m85_modern_probe -- <alex.p3d>`

use std::env;
use std::fs;

use prototype_p3d::modern::{find_modern_skinned_meshes, CHUNK_TYPE_BONE_PALETTE};
use prototype_p3d::skin::{decode_skeleton, CHUNK_TYPE_SKELETON};

fn main() {
    let path = env::args()
        .nth(1)
        .unwrap_or_else(|| "/home/z/my-project/source/alex.p3d".to_string());
    let buf = fs::read(&path).expect("read alex.p3d");
    println!("loaded {} ({} B)", path, buf.len());

    // Decode skeletons first (for palette cross-validation) — walk ALL
    // descendants (the alex_reg_* skeletons are nested, not top-level).
    // Depth-limited BFS to avoid quadratic blowup on the 12 MB buffer.
    let mut skeletons: std::collections::HashMap<String, prototype_p3d::skin::Skeleton> = std::collections::HashMap::new();
    let mut stack = vec![(12usize, 0usize)];
    let mut visited = 0usize;
    while let Some((p, depth)) = stack.pop() {
        if depth > 6 { continue; }
        let mut q = p;
        while q + 12 <= buf.len() {
            visited += 1;
            if visited > 4_000_000 { break; }
            let t = u32le(&buf, q);
            let cs = u32le(&buf, q + 4) as usize;
            let sz = u32le(&buf, q + 8) as usize;
            if cs < 12 || sz < cs || q + sz > buf.len() { break; }
            if t == CHUNK_TYPE_SKELETON {
                match decode_skeleton(&buf, q, sz) {
                    Ok(sk) => { skeletons.insert(sk.name.clone(), sk); }
                    Err(e) => { eprintln!("  skeleton decode err @ {q:#x}: {e}"); }
                }
            }
            if sz > cs { stack.push((q + cs, depth + 1)); }
            q += sz;
        }
    }
    println!("skeletons decoded: {} (visited {visited} chunk headers)", skeletons.len());
    for (name, sk) in skeletons.iter() {
        println!("  {:?} ({} joints)", name.trim_end(), sk.joints.len());
    }
    // Dump the joint names of the alex_reg_body_skeleton.
    for (name, sk) in skeletons.iter() {
        if name.starts_with("alex_reg_body") {
            println!("  {} joint names:", name);
            for (i, j) in sk.joints.iter().take(15).enumerate() {
                println!("    [{}] {:?}", i, j.name);
            }
        }
    }
    let _ = CHUNK_TYPE_BONE_PALETTE;

    // Decode modern skinned meshes.
    let meshes = find_modern_skinned_meshes(&buf);
    println!();
    println!("modern skinned shapes decoded: {}", meshes.len());
    for m in &meshes {
        let skel_joints = skeletons.get(&m.skeleton).map(|s| s.joints.len()).unwrap_or(0);
        let mut max_slot = 0usize;
        for bs in &m.bones {
            for b in bs {
                max_slot = max_slot.max(*b as usize);
            }
        }
        let max_joint = m.palette.iter().copied().max().unwrap_or(0);
        println!(
            "  {}: skel={:?} verts={} idx={} palette={} max_slot={} max_joint={} skel_joints={}",
            m.name, m.skeleton, m.positions.len(), m.indices.len(),
            m.palette.len(), max_slot, max_joint, skel_joints
        );
    }

    // Cross-validate palettes: every palette joint must be < skel joints.
    let mut bad = 0;
    for m in &meshes {
        if let Some(sk) = skeletons.get(&m.skeleton) {
            for j in &m.palette {
                if *j >= sk.joints.len() as u32 {
                    bad += 1;
                    eprintln!(
                        "  PALETTE OOB: {} joint {} >= {} joints",
                        m.name, j, sk.joints.len()
                    );
                }
            }
        }
    }
    if bad == 0 {
        println!("palette cross-validation: OK (all joints in range)");
    } else {
        eprintln!("palette cross-validation: {bad} out-of-range entries");
        std::process::exit(2);
    }
}

fn u32le(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

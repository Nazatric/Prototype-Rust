//! Dump the alex_reg_body_skeleton joint names — using the REAL
//! chunk size at chunk_offset+8 (which includes all 67 sibling joints)
//! rather than the small "header-only" size at chunk_offset+4.
use prototype_p3d::skin::decode_skeleton;

fn main() {
    let buf = include_bytes!("../tests/fixtures/skeleton_alex_reg_body.p3d");
    let mut off = 0usize;
    while off + 12 <= buf.len() {
        let t = u32::from_le_bytes(buf[off..off+4].try_into().unwrap());
        if t == 0x23000 {
            let size_small = u32::from_le_bytes(buf[off+4..off+8].try_into().unwrap()) as usize;
            let size_total = u32::from_le_bytes(buf[off+8..off+12].try_into().unwrap()) as usize;
            println!("=== Skeleton @{}  size_small={}  size_total={} ===", off, size_small, size_total);
            // Use size_total (the whole-tree size)
            let sz = if off + size_total <= buf.len() { size_total } else { size_small };
            match decode_skeleton(buf, off, sz) {
                Ok(s) => {
                    println!("name: {}  joints: {}", s.name, s.joints.len());
                    for (i, j) in s.joints.iter().enumerate() {
                        println!("  {:2}: {:<32}  parent={}", i, j.name, j.parent);
                    }
                }
                Err(e) => println!("  decode error: {:?}", e),
            }
            break;
        }
        let sz = u32::from_le_bytes(buf[off+4..off+8].try_into().unwrap()) as usize;
        if sz < 8 { break; }
        off += sz;
    }
}

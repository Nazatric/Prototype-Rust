//! Block 83: probe the OLD-STYLE skinned geometry (0x00010001 shapes).
//!
//! For each old-style shape: name, version, skeleton ref, num_prims,
//! then every nested chunk; for nested 0x00010020 meshes print the
//! MeshHeader words and decode every 0x00010014 PrimitiveGroup with the
//! PROVEN decode_primitive_group (elements + semantics + offsets), and
//! the VB/IB headers (magic, field2, data size).  Goal: derive the
//! vertex layout of the unobserved desc ids (e.g. 0xb1b1) from their
//! own declarations, and locate the SKIN stream.

use prototype_p3d::geometry::{attr_semantic, decode_primitive_group, ArtError, ChildChunk};

fn u32_at(buf: &[u8], off: usize) -> Result<u32, ArtError> {
    if off + 4 > buf.len() {
        return Err(ArtError::Truncated { offset: off, need: 4, got: buf.len() - off });
    }
    Ok(u32::from_le_bytes(buf[off..off + 4].try_into().unwrap()))
}

fn pascal_at(buf: &[u8], off: usize, end: usize) -> Result<(String, usize), ArtError> {
    if off >= end || off >= buf.len() {
        return Err(ArtError::BadPascal { offset: off, len: 0 });
    }
    let n = buf[off] as usize;
    if off + 1 + n > end || off + 1 + n > buf.len() {
        return Err(ArtError::BadPascal { offset: off, len: n });
    }
    Ok((String::from_utf8_lossy(&buf[off + 1..off + 1 + n]).into_owned(), off + 1 + n))
}

fn child_chunks(buf: &[u8], off: usize, end: usize) -> Option<Vec<ChildChunk>> {
    let mut out = Vec::new();
    let mut w = off;
    while w + 12 <= end {
        let t = u32_at(buf, w).ok()?;
        let uid = u32_at(buf, w + 4).ok()?;
        let sz = u32_at(buf, w + 8).ok()? as usize;
        if sz < 12 || w + sz > end {
            return None;
        }
        out.push(ChildChunk { chunk_type: t, uid, offset: w, size: sz });
        w += sz;
    }
    if w != end {
        return None;
    }
    Some(out)
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: m83_skinned_probe <file.p3d> [shape-substr]");
    let filter = std::env::args().nth(2);
    let buf = std::fs::read(&path).expect("read p3d");
    let top = prototype_p3d::geometry::top_level_chunks(&buf).expect("top-level walk");
    for c in &top {
        if c.chunk_type != 0x0001_0001 {
            continue;
        }
        let (name, after_name) = match pascal_at(&buf, c.offset + 12, c.offset + c.size) {
            Ok(v) => v,
            Err(e) => {
                println!("shape at {}: name err {e}", c.offset);
                continue;
            }
        };
        if let Some(f) = &filter {
            if !name.to_lowercase().contains(&f.to_lowercase()) {
                continue;
            }
        }
        let version = u32_at(&buf, after_name).unwrap_or(u32::MAX);
        let mut q = after_name + 4;
        let mut skel = String::new();
        if version >= 1 {
            if let Ok((s, a)) = pascal_at(&buf, q, c.offset + c.size) {
                skel = s;
                q = a;
            }
        }
        let nprims = u32_at(&buf, q).unwrap_or(u32::MAX);
        q += 4;
        println!(
            "SHAPE {name:?} at {} size {} version {version} skeleton {skel:?} nprims {nprims}",
            c.offset, c.size
        );
        let children = child_chunks(&buf, q, c.offset + c.size);
        let children = match children {
            Some(ch) => ch,
            None => {
                println!("  child walk FAILED");
                continue;
            }
        };
        for ch in &children {
            println!(
                "  child {:#010x} uid {} size {} payload {}",
                ch.chunk_type,
                ch.offset,
                ch.size,
                ch.size - 12
            );
            if ch.chunk_type == 0x0001_0020 {
                print_mesh(&buf, ch.offset, ch.size);
            }
        }
    }
}

fn print_mesh(buf: &[u8], offset: usize, size: usize) {
    let end = offset + size;
    let ver = u32_at(buf, offset + 12).unwrap_or(u32::MAX);
    let (name, after_name) = match pascal_at(buf, offset + 16, end) {
        Ok(v) => v,
        Err(e) => {
            println!("    mesh name err {e}");
            return;
        }
    };
    let words: Vec<u32> = (0..9).filter_map(|i| u32_at(buf, after_name + i * 4).ok()).collect();
    println!(
        "    MESH {name:?} ver {ver} field0 {} desc {:#010x} vcount {} icount {} tail {:?}",
        words[0], words[1], words[2], words[3], &words[4..]
    );
    if let Some(ch) = child_chunks(buf, after_name + 36, end) {
        for c in &ch {
            println!("      buf-child {:#010x} size {}", c.chunk_type, c.size);
            if c.chunk_type == 0x0001_0014 {
                match decode_primitive_group(buf, c.offset, c.size) {
                    Ok(pg) => {
                        println!(
                            "        PG kind2 vertexBytes {} code {:#x} dhash {:#x} elements {}",
                            pg.vertex_bytes,
                            pg.code,
                            pg.descriptor_hash,
                            pg.elements.len()
                        );
                        for el in &pg.elements {
                            println!(
                                "          elem stride {} n {} class {} attr {:#08x} ({:?}) y {} offset {}",
                                el.stride,
                                el.n,
                                el.class,
                                el.attr,
                                attr_semantic(el.attr),
                                el.field_y,
                                el.offset
                            );
                        }
                        println!(
                            "          final stride {} n {} flag {}",
                            pg.final_element.stride, pg.final_element.n, pg.final_element.flag
                        );
                    }
                    Err(e) => println!("        PG decode FAILED: {e}"),
                }
            }
            if c.chunk_type == 0x0001_0012 || c.chunk_type == 0x0001_0013 {
                let p = c.offset + 12;
                let magic = u32_at(buf, p).unwrap_or(0);
                let f2 = u32_at(buf, p + 4).unwrap_or(0);
                let dsz = u32_at(buf, p + 8).unwrap_or(0);
                println!("        buffer magic {magic:#010x} field2 {f2} data_size {dsz}");
            }
        }
    }
}

//! Block 82: inspect the shapes decode_shape FAILS on (old-style P3D
//! geometry: separate PositionList/NormalList/UVList/IndexList chunks).
//! Print the shape name, child chunk types with sizes, and for
//! old-style position lists decode the raw f32 triples.

use std::io::Cursor;

fn pascal_at(buf: &[u8], mut pos: usize, end: usize) -> Option<(String, usize)> {
    if pos + 4 > end {
        return None;
    }
    let len = u32::from_le_bytes(buf[pos..pos + 4].try_into().ok()?) as usize;
    pos += 4;
    if pos + len > end || len == 0 {
        return None;
    }
    let field = &buf[pos..pos + len];
    let nul = field.iter().position(|&b| b == 0).unwrap_or(len);
    Some((String::from_utf8_lossy(&field[..nul]).into_owned(), pos + len))
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: m82_old_shape <file.p3d> [offset]");
    let buf = std::fs::read(&path).expect("read p3d");
    let off_arg = std::env::args().nth(2);
    let top = prototype_p3d::geometry::top_level_chunks(&buf).expect("top-level walk");
    let _ = Cursor::new(&buf);

    // The header of a top-level chunk: type(4) uid(4) size(4) -> payload
    // starts at offset+12.
    for c in top.iter().filter(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_SHAPE) {
        let name = pascal_at(&buf, c.offset + 12, c.offset + c.size)
            .map(|(n, _)| n)
            .unwrap_or_default();
        // Only the requested offset, or every shape decode_shape rejects.
        if let Some(want) = &off_arg {
            let want: usize = want.parse().expect("offset number");
            if c.offset != want {
                continue;
            }
        } else {
            // probe: does decode_shape accept it?
            if prototype_p3d::geometry::decode_shape(&buf, c.offset, c.size).is_ok() {
                continue;
            }
        }
        println!("SHAPE \"{}\" at {} size {}", name, c.offset, c.size);
        // walk children from after the pascal name + version + flag
        if let Some((_, after_name)) = pascal_at(&buf, c.offset + 12, c.offset + c.size) {
            let mut pos = after_name + 8;
            let end = c.offset + c.size;
            while pos + 12 <= end {
                let t = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap());
                let _uid = u32::from_le_bytes(buf[pos + 4..pos + 8].try_into().unwrap());
                let sz = u32::from_le_bytes(buf[pos + 8..pos + 12].try_into().unwrap()) as usize;
                if sz < 12 || pos + sz > end {
                    println!("  child at {pos}: type {t:#010x} BAD SIZE {sz}");
                    break;
                }
                println!("  child at {pos}: type {t:#010x} size {sz} payload {}", sz - 12);
                // old-style list payloads: first u32 = element count
                if sz >= 16 && matches!(t, 0x00010005 | 0x00010006 | 0x00010007 | 0x00010008 | 0x0001000a) {
                    let n = u32::from_le_bytes(buf[pos + 12..pos + 16].try_into().unwrap());
                    println!("    element count u32[0] = {n} (payload/4 - 1 = {})", (sz - 12) / 4 - 1);
                    if t == 0x00010005 && n > 0 && (sz - 16) / 4 / 3 >= n as usize {
                        let mut mn = [f32::MAX; 3];
                        let mut mx = [f32::MIN; 3];
                        for i in 0..n as usize {
                            let p = pos + 16 + i * 12;
                            let v: [f32; 3] = bytemuck_triple(&buf[p..p + 12]);
                            for ch in 0..3 {
                                mn[ch] = mn[ch].min(v[ch]);
                                mx[ch] = mx[ch].max(v[ch]);
                            }
                        }
                        println!(
                            "    POSITIONS bounds min [{:.3} {:.3} {:.3}] max [{:.3} {:.3} {:.3}]",
                            mn[0], mn[1], mn[2], mx[0], mx[1], mx[2]
                        );
                    }
                }
                pos += sz;
            }
        }
    }
}

fn bytemuck_triple(b: &[u8]) -> [f32; 3] {
    [
        f32::from_le_bytes(b[0..4].try_into().unwrap()),
        f32::from_le_bytes(b[4..8].try_into().unwrap()),
        f32::from_le_bytes(b[8..12].try_into().unwrap()),
    ]
}

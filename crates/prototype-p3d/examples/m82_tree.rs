//! Block 82: recursive chunk-tree dump for old-style P3D geometry
//! (0x00010001 families and their list children).  Prints every
//! chunk's type, size, pascal name (best effort), and for list-type
//! payloads the element counts, to reverse the skinned-mesh layout.

fn pascal_at(buf: &[u8], mut pos: usize, end: usize) -> Option<(String, usize)> {
    if pos + 4 > end {
        return None;
    }
    let len = u32::from_le_bytes(buf[pos..pos + 4].try_into().ok()?) as usize;
    pos += 4;
    if len == 0 || pos + len > end {
        return None;
    }
    let field = &buf[pos..pos + len];
    let nul = field.iter().position(|&b| b == 0).unwrap_or(len);
    std::str::from_utf8(&field[..nul])
        .ok()
        .map(|s| (s.to_string(), pos + len))
}

fn dump(buf: &[u8], off: usize, end: usize, depth: usize, out: &mut Vec<String>) {
    if off + 12 > end || depth > 4 {
        return;
    }
    let t = u32::from_le_bytes(buf[off..off + 4].try_into().unwrap());
    let uid = u32::from_le_bytes(buf[off + 4..off + 8].try_into().unwrap());
    let sz = u32::from_le_bytes(buf[off + 8..off + 12].try_into().unwrap()) as usize;
    if sz < 12 || off + sz > end {
        out.push(format!("{}type {t:#010x} uid {uid:#010x} BAD SIZE {sz} at {off}", "  ".repeat(depth)));
        return;
    }
    let pend = off + sz;
    let mut name = String::new();
    let mut extra = String::new();
    let pstart = off + 12;
    // Try pascal name (old-style shapes/meshes carry one).
    if let Some((n, after)) = pascal_at(buf, pstart, pend) {
        name = n;
        // list payloads: first u32 after name = element count
        if after + 4 <= pend {
            let n0 = u32::from_le_bytes(buf[after..after + 4].try_into().unwrap());
            let n1 = u32::from_le_bytes(buf[after + 4..after + 8].try_into().unwrap());
            extra = format!(" u32[0]={n0} u32[1]={n1} payload_left={}", pend - after);
        }
    } else if pstart + 8 <= pend {
        let n0 = u32::from_le_bytes(buf[pstart..pstart + 4].try_into().unwrap());
        let n1 = u32::from_le_bytes(buf[pstart + 4..pstart + 8].try_into().unwrap());
        extra = format!(" (no name) u32[0]={n0} u32[1]={n1} payload={}", sz - 12);
    }
    out.push(format!(
        "{}type {t:#010x} uid {uid:#010x} size {sz} \"{}\"{}",
        "  ".repeat(depth),
        name,
        extra
    ));
    // recurse children: try from after the name, else from payload start
    let child_start = if name.is_empty() { pstart } else {
        pascal_at(buf, pstart, pend).map(|(_, a)| a).unwrap_or(pstart)
    };
    // children only make sense for container-ish types; walk greedily
    let mut pos = child_start;
    while pos + 12 <= pend {
        let ct = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap());
        let csz = u32::from_le_bytes(buf[pos + 8..pos + 12].try_into().unwrap()) as usize;
        let looks_like_chunk = csz >= 12 && pos + csz <= pend && (0x1000..=0xFFFF_FFFF).contains(&ct);
        if !looks_like_chunk {
            break;
        }
        dump(buf, pos, pend, depth + 1, out);
        pos += csz;
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: m82_tree <file.p3d> [type_hex]");
    let buf = std::fs::read(&path).expect("read");
    let want = std::env::args().nth(2).and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok());
    let top = prototype_p3d::geometry::top_level_chunks(&buf).expect("walk");
    let mut out = Vec::new();
    for c in &top {
        if let Some(w) = want {
            if c.chunk_type != w {
                continue;
            }
        }
        dump(&buf, c.offset, c.offset + c.size, 0, &mut out);
        out.push("---".into());
    }
    for l in &out {
        println!("{l}");
    }
    eprintln!("{} lines", out.len());
}

//! Block 82: build the FULL art.rcf name index (member -> internal
//! Shape/Texture names) by decompressing every member and walking its
//! top-level P3D chunks.  The archive's directory is hash-keyed (no
//! name table), so this ONE pass is the deterministic way to find a
//! member by what it CONTAINS (e.g. the Alex Mercer character model).
//!
//! Usage: m82_art_name_scan <art.rcf> <out_index.txt> [filter]
//!   filter (optional): only print members whose extracted names or
//!   kind contain this substring (case-insensitive).  The index file
//!   always holds every member.

use std::io::{Read, Seek, SeekFrom};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let out_path = &args[2];
    let filter = args.get(3).map(|s| s.to_lowercase());

    let mut f = std::fs::File::open(path).expect("open art.rcf");
    let file_len = f.metadata().unwrap().len() as usize;

    // Directory: parse from the head (proven path — 256 KB prefix
    // covers dir_offset 60 + dir_size 31212).
    let mut head = vec![0u8; 262_144];
    f.read_exact(&mut head).expect("head");
    let (header, entries) =
        prototype_rcf::parse_directory(&mut std::io::Cursor::new(&head)).expect("dir parse");
    println!(
        "dir: {} entries, dir_offset {}, dir_size {} (file {file_len} B)",
        entries.len(),
        header.dir_offset,
        header.dir_size_in_bytes
    );

    let mut out = String::new();
    let mut stats = (0usize, 0usize, 0usize, 0usize, 0usize); // rz_p3d, gfx, dds, other, fail
    let mut alexish: Vec<String> = Vec::new();

    for (i, e) in entries.iter().enumerate() {
        f.seek(SeekFrom::Start(e.file_offset as u64)).expect("seek");
        let mut raw = vec![0u8; e.file_size as usize];
        if let Err(er) = f.read_exact(&mut raw) {
            out.push_str(&format!("{i}\t{:08x}\t{}\t{}\tREAD_FAIL {er}\n", e.name_hash, e.file_offset, e.file_size));
            continue;
        }
        let (kind, data): (&str, Vec<u8>) = if prototype_rcf::rz::is_rz(&raw) {
            match prototype_rcf::rz::decompress(&raw) {
                Ok(d) => ("rz", d),
                Err(_) => ("rz_fail", raw),
            }
        } else if raw.len() >= 4 && &raw[0..4] == b"P3D\x00" {
            ("p3d_raw", raw)
        } else if raw.len() >= 8 && &raw[0..4] == b"GFx0" || raw.starts_with(b"CFX") {
            ("gfx", raw)
        } else if raw.len() >= 4 && &raw[0..4] == b"DDS " {
            ("dds", raw)
        } else {
            ("other", raw)
        };

        let mut names: Vec<String> = Vec::new();
        if kind == "rz" || kind == "p3d_raw" {
            match prototype_p3d::geometry::top_level_chunks(&data) {
                Some(top) => {
                    for c in &top {
                        if c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_SHAPE {
                            if let Ok(s) = prototype_p3d::geometry::decode_shape(&data, c.offset, c.size) {
                                names.push(s.name.trim_end().to_string());
                                for m in &s.meshes {
                                    names.push(format!("  mesh:{}", m.name.trim_end()));
                                }
                            } else {
                                names.push("  shape:DECODE_FAIL".into());
                            }
                        } else if c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_TEXTURE {
                            if let Ok(t) = prototype_p3d::geometry::decode_texture(&data, c.offset, c.size) {
                                names.push(format!("  tex:{}", t.name.trim_end()));
                            }
                        } else if c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_MATERIAL {
                            // material chunk names come from a pascal read; cheap probe
                            if let Ok(Some(m)) = material_name(&data, c) {
                                names.push(format!("  mat:{}", m));
                            }
                        }
                    }
                }
                None => names.push("WALK_FAIL".into()),
            }
        }
        match kind {
            "rz" => stats.0 += 1,
            "gfx" => stats.1 += 1,
            "dds" => stats.2 += 1,
            "other" | "rz_fail" => stats.3 += 1,
            _ => {}
        }
        if kind == "rz_fail" {
            stats.4 += 1;
        }

        let joined = names.join(" | ");
        let line = format!(
            "{i}\t{:08x}\t{}\t{}\t{kind}\t{joined}\n",
            e.name_hash, e.file_offset, e.file_size
        );
        let lower = line.to_lowercase();
        if let Some(flt) = &filter {
            if lower.contains(flt) {
                alexish.push(line.clone());
            }
        }
        out.push_str(&line);
    }

    std::fs::write(out_path, &out).expect("write index");
    println!(
        "kinds: rz_p3d {} gfx {} dds {} other {} (rz_fail {}) — index written to {out_path}",
        stats.0, stats.1, stats.2, stats.3, stats.4
    );
    if let Some(flt) = &filter {
        println!("--- filter \"{}\" hits: {} ---", flt, alexish.len());
        for l in alexish.iter().take(80) {
            print!("{l}");
        }
    }
}

/// Best-effort pascal-name probe for a Material chunk
/// (pascal name directly in the payload).
fn material_name(data: &[u8], c: &prototype_p3d::geometry::ChildChunk) -> std::result::Result<Option<String>, ()> {
    let p = c.payload();
    if p + 4 > data.len() {
        return Err(());
    }
    let len = u32::from_le_bytes(data[p..p + 4].try_into().map_err(|_| ())?) as usize;
    if len == 0 || p + 4 + len > data.len() {
        return Err(());
    }
    let field = &data[p + 4..p + 4 + len];
    let nul = field.iter().position(|&b| b == 0).unwrap_or(len);
    std::str::from_utf8(&field[..nul]).map(|s| Some(s.to_string())).map_err(|_| ())
}

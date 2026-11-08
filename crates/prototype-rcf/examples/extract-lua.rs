//! extract-lua — pull every Lua 5.1 payload out of an RCF archive
//! and write each to <out-dir>/lua_<offset>_<size>.lua
//!   cargo run -p prototype-rcf --example extract-lua -- <archive.rcf> <out-dir>

fn main() {
    let path = std::env::args().nth(1).expect("usage: extract-lua <rcf> <outdir>");
    let outdir = std::env::args().nth(2).expect("usage: extract-lua <rcf> <outdir>");
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let arc = prototype_rcf::RcfArchive::parse(&data).unwrap();
    std::fs::create_dir_all(&outdir).unwrap();
    for e in &arc.entries {
        let p = arc.payload(e).unwrap();
        if prototype_rcf::PayloadKind::sniff(p) == prototype_rcf::PayloadKind::Lua51 {
            let name = format!("lua_{}_{}.lua", e.file_offset, e.file_size);
            let out = std::path::Path::new(&outdir).join(&name);
            std::fs::write(&out, p).unwrap();
            println!("wrote {} ({} bytes)", out.display(), p.len());
        }
    }
}

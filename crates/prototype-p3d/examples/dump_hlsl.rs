//! Dump the first few ShaderProgram HLSL sources from the
//! frontendshaders.p3d fixture, so we can see what the M10
//! HLSL→WGSL translator needs to handle.

use std::path::Path;

fn main() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("prototype-bin")
        .join("tests")
        .join("fixtures")
        .join("frontendshaders.p3d");
    let data = std::fs::read(&p).expect("fixture");
    let f = prototype_p3d::P3dFile::parse(&data).expect("parse");
    let progs = f.shader_programs().expect("shader_programs");
    println!("{} programs total", progs.len());
    for (i, p) in progs.iter().take(4).enumerate() {
        let src = std::str::from_utf8(p.source).unwrap_or("<utf8-error>");
        let banner_end = src.lines().next().unwrap_or("").len();
        let body = &src[banner_end.min(src.len())..];
        println!("\n========== Program {} ==========", i);
        println!("name        = {:?}", p.name);
        println!("is_fragment = {}", p.is_fragment);
        println!("version     = {:#010x}", p.version_token);
        println!("source len  = {}", p.source.len());
        println!("--- HLSL source (trimmed banner) ---");
        println!("{}", body.trim_end_matches('\0'));
        println!("--- end ---");
    }
}

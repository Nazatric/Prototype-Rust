//! dump_wgsl_text — write the translated WGSL of a committed sm30 fixture
//! to a file for external inspection (naga debugging, block 36).
//! Usage: cargo run -p prototype-render --example dump_wgsl_text -- <fixture-name> <out-path> [fs]
use prototype_render::shader::sm30::Sm30Shader;
use prototype_render::shader::wgsl::translate_program_with_plan;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let name = &args[1];
    let out = &args[2];
    let frag = args.get(3).map(|s| s == "fs").unwrap_or(false);
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..").join("prototype-bin").join("tests").join("fixtures").join(name);
    let bytes = std::fs::read(&p).unwrap();
    let prog = Sm30Shader::parse_program(&bytes, frag).unwrap();
    let (text, _plan) = translate_program_with_plan(&prog, name).unwrap();
    std::fs::write(out, text).unwrap();
    println!("wrote {out}");
}

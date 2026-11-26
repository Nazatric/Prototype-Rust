//! m103 — dump the legacy shader pair's REAL instruction stream with
//! the repo's own SM3.0 parser (blocks 98/101), so the CPU port can
//! mirror the math instruction-for-instruction.
use prototype_render::shader::sm30::Sm30Shader;

fn dump(path: &str, is_fragment: bool) {
    let bytes = std::fs::read(path).unwrap();
    let sh = Sm30Shader::parse(&bytes, is_fragment).unwrap();
    println!(
        "=== {path} ({} instructions, {} ctab constants) ===",
        sh.instructions.len(),
        sh.ctab.as_ref().map(|c| c.constants.len()).unwrap_or(0)
    );
    println!("{}", sh.disasm());
    println!();
}

fn main() {
    dump(
        "crates/prototype-bin/tests/fixtures/dll_vs_e36918.sm30",
        false,
    );
    dump(
        "crates/prototype-bin/tests/fixtures/dll_ps_e3d9b0.sm30",
        true,
    );
}

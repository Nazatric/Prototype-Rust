//! dump_world_pair — audit tool for NEXT-UP item 7(e): print EVERYTHING
//! the DLL `default` pair (the world pipeline) declares, so the wiring
//! code is grounded in the real bytes, not invention.
//!
//! Usage: `cargo run -p prototype-render --example dump_world_pair`
//! (reads the two committed fixtures; exits non-zero on any parse error).

use prototype_render::shader::sm30::{Instruction, RegType, Sm30Op, Sm30Shader};
use prototype_render::shader::wgsl::translate_program;
use std::path::Path;

fn fixture(name: &str) -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("prototype-bin")
        .join("tests")
        .join("fixtures")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
}

fn dump(name: &str, is_fragment: bool) {
    let bytes = fixture(name);
    let prog = Sm30Shader::parse_program(&bytes, is_fragment)
        .unwrap_or_else(|e| panic!("{name}: parse: {e}"));
    println!("===== {name} (is_fragment={is_fragment}) =====");
    println!(
        "instructions: {}  param records: {}  defs: {}",
        prog.instructions.len(),
        prog.params.len(),
        prog.defs.len()
    );
    if let Some(ctab) = &prog.ctab {
        println!("ctab target: {:?}  creator: {:?}", ctab.target, ctab.creator);
        for c in &ctab.constants {
            println!(
                "  ctab: {:<44} set={} idx={:<3} count={} class={} type={} rows={} cols={}",
                c.name, c.register_set, c.register_index, c.register_count, c.class, c.ptype,
                c.rows, c.columns
            );
        }
    } else {
        println!("ctab: NONE");
    }
    println!("-- param records --");
    for p in &prog.params {
        println!(
            "  param: {:<44} uid={} type_id={} reg={} sub={} count={}",
            p.name, p.uid, p.type_id, p.register_index, p.register_sub, p.register_count
        );
    }
    for (reg, vals) in &prog.defs {
        println!("  def: c{reg} = {vals:?}");
    }
    println!("-- dcl inputs --");
    for (usage, idx, reg) in prog.inputs() {
        println!("  in: {usage}{idx} -> v{} ({reg:?})", reg.reg);
    }
    println!("-- dcl outputs --");
    for (usage, idx, reg) in prog.outputs() {
        println!("  out: {usage}{idx} -> o{} ({reg:?})", reg.reg);
    }
    println!("-- samplers --");
    for s in prog.samplers() {
        println!("  sampler: s{} ({s:?})", s.reg);
    }
    println!("-- flow / bool usage --");
    for inst in &prog.instructions {
        if let Instruction::Flow { op, reg } = inst {
            let cond = reg
                .as_ref()
                .map(|r| format!("{:?}{}", r.ty, r.reg))
                .unwrap_or_else(|| "-".into());
            println!("  {op:?} {cond}");
        }
    }
    // c-registers actually read (Const operands) + bool reads + samplers.
    let mut const_reads = std::collections::BTreeSet::new();
    let mut bool_reads = std::collections::BTreeSet::new();
    let mut sampler_reads = std::collections::BTreeSet::new();
    let mut visit = |t: &prototype_render::shader::sm30::ParamToken| match t.ty {
        RegType::Const => const_reads.insert(t.reg),
        RegType::ConstBool => bool_reads.insert(t.reg),
        _ => false,
    };
    for inst in &prog.instructions {
        match inst {
            Instruction::Op3 { op: Sm30Op::Tex, b, .. } => {
                sampler_reads.insert(b.reg);
            }
            Instruction::Op2 { a, .. } => {
                visit(a);
            }
            Instruction::Op3 { a, b, .. } => {
                visit(a);
                visit(b);
            }
            Instruction::Op4 { a, b, c, .. } => {
                visit(a);
                visit(b);
                visit(c);
            }
            Instruction::Flow { reg, .. } => {
                if let Some(r) = reg {
                    visit(r);
                }
            }
            Instruction::Texkill { .. } | Instruction::Def { .. } | Instruction::Dcl { .. } => {}
        }
    }
    let const_reads: Vec<_> = const_reads.into_iter().collect();
    let bool_reads: Vec<_> = bool_reads.into_iter().collect();
    let mut sampler_reads: Vec<_> = sampler_reads.into_iter().collect();
    sampler_reads.sort_unstable();
    println!("-- census --");
    println!("  const reads (c#): {const_reads:?}");
    println!("  bool reads (b#): {bool_reads:?}");
    println!("  sampler reads (s#): {sampler_reads:?}");
    match translate_program(&prog, name) {
        Ok(wgsl) => {
            let globals: String = wgsl
                .lines()
                .filter(|l| l.contains("@binding("))
                .map(|l| format!("  {l}\n"))
                .collect();
            print!("-- wgsl resource globals --\n{globals}");
            let locs: String = wgsl
                .lines()
                .filter(|l| l.contains("@location(") || l.contains("@builtin("))
                .map(|l| format!("  {l}\n"))
                .collect();
            print!("-- wgsl in/out locations --\n{locs}");
        }
        Err(e) => println!("-- wgsl translation FAILED: {e:?} --"),
    }
    println!();
}

fn main() {
    // Optional full-source dump (audit aid): set DUMP_WORLD_WGSL_DIR to
    // a directory and the complete translated WGSL of both stages is
    // written there as default_vp.wgsl / default_fp.wgsl.
    let dump_dir = std::env::var("DUMP_WORLD_WGSL_DIR").ok();
    if let Some(dir) = &dump_dir {
        let bytes_vs = fixture("dll_default_vp_e34e60.sm30");
        let bytes_fp = fixture("dll_default_fp_e3b308.sm30");
        let vs = Sm30Shader::parse_program(&bytes_vs, false).unwrap();
        let fp = Sm30Shader::parse_program(&bytes_fp, true).unwrap();
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            Path::new(dir).join("default_vp.wgsl"),
            translate_program(&vs, "default_vp").unwrap(),
        )
        .unwrap();
        std::fs::write(
            Path::new(dir).join("default_fp.wgsl"),
            translate_program(&fp, "default_fp").unwrap(),
        )
        .unwrap();
        println!("full WGSL written to {dir}");
    }
    dump("dll_default_vp_e34e60.sm30", false);
    dump("dll_default_fp_e3b308.sm30", true);
}

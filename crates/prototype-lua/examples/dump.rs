//! dump — disassemble a compiled Lua 5.1 chunk and print an opcode
//! census + constant pool.  Usage:
//!   cargo run -p prototype-lua --example dump -- <file.lua>

use prototype_lua::{Instruction, Lua51File, OPCODE_NAMES};

fn census(proto: &prototype_lua::Proto, counters: &mut [usize; 40]) {
    for &raw in &proto.code {
        let op = Instruction::decode(raw).opcode as usize;
        if op < 40 {
            counters[op] += 1;
        }
    }
    for p in &proto.protos {
        census(p, counters);
    }
}

fn dump(proto: &prototype_lua::Proto, label: &str) {
    println!("=== {label} lines {}..{} params={} vararg={} nups={} stack={} ===",
        proto.linedefined, proto.lastlinedefined, proto.nparams, proto.is_vararg, proto.nups, proto.maxstacksize);
    println!("-- constants ({}):", proto.constants.len());
    for (i, c) in proto.constants.iter().enumerate() {
        println!("  K{i} = {}", c.display());
    }
    println!("-- code ({}):", proto.code.len());
    for (i, &raw) in proto.code.iter().enumerate() {
        let ins = Instruction::decode(raw);
        let operand = match ins.opname() {
            "LOADK" | "GETGLOBAL" | "SETGLOBAL" | "CLOSURE" => format!("K{} = {}", ins.bx,
                proto.constants.get(ins.bx as usize).map(|c| c.display()).unwrap_or("?".into())),
            "JMP" | "FORLOOP" | "FORPREP" => format!("-> {}", i as i64 + 1 + ins.sbx as i64),
            _ => String::new(),
        };
        println!("  {i:4} {} A={} B={} C={} {operand}", ins.opname(), ins.a, ins.b, ins.c);
    }
    for (i, p) in proto.protos.iter().enumerate() {
        println!();
        dump(p, &format!("{label} :: nested #{i}"));
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: dump <file.lua>");
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let f = Lua51File::parse(&data).unwrap_or_else(|e| panic!("parse {path}: {e}"));
    println!("exact={} trailing={}", f.is_exact(), f.trailing.len());
    let mut counters = [0usize; 40];
    census(&f.main, &mut counters);
    println!("-- opcode census (whole tree):");
    for (op, &n) in counters.iter().enumerate() {
        if n > 0 {
            println!("  {:>2} {:<10} x{n}", op, OPCODE_NAMES[op]);
        }
    }
    println!();
    dump(&f.main, "main");
}

//! prototype-lua (v2)
//! ==================
//! Full compiled Lua 5.1 bytecode parser for Prototype (2009).
//!
//! ## PROVEN header (12 bytes — from `scripts/frevents.lua` and the
//! Lua entries inside `scripts.rcf`)
//!
//! | Offset | Field                | Value |
//! |--------|----------------------|-------|
//! | 0      | magic `\x1bLua`      |       |
//! | 4      | version              | 0x51 (Lua 5.1) |
//! | 5      | format               | 0 (official) |
//! | 6      | endian               | 1 (little) |
//! | 7      | sizeof(int)          | 4 |
//! | 8      | sizeof(size_t)       | 4 |
//! | 9      | sizeof(Instruction)  | 4 |
//! | 10     | sizeof(lua_Number)   | **4** (float — NOT the standard 8-byte double) |
//! | 11     | integral flag        | 0 (floating) |
//!
//! `lua_Number = f32` is a compile-time choice of Radical's build —
//! any interoperating VM must be built with `LUA_NUMBER=float`.
//!
//! ## PROVEN chunk body layout (Lua 5.1 undump format)
//!
//! After the header follows exactly one **main Proto**.  The layout
//! follows the public Lua 5.1 `ldump.c` order and was validated
//! byte-exactly against the real Prototype fixtures:
//!
//! ```text
//! source            : u32 size + bytes  (0 = NULL; sizes INCLUDE the
//!                                     trailing NUL byte — Lua 5.1 dumps
//!                                     `strlen+1`)
//! linedefined       : i32 (0 for main)
//! lastlinedefined   : i32 (0 for main)
//! nups              : u8   (upvalue count — comes BEFORE nparams,
//!                          per ldump.c DumpFunction order)
//! nparams           : u8   (0 for main)
//! is_vararg         : u8   (2 observed for main = VARARG_ISVARARG)
//! maxstacksize      : u8
//! code              : u32 count + count × u32 instructions
//! constants         : u32 count + count × {
//!                        u8 tag: 0=nil, 1=bool(u8), 3=number(f32),
//!                        4=string(u32 size incl. NUL + bytes)
//!                     }
//! prototypes        : u32 count + nested Protos  (inside DumpConstants)
//! lineinfo          : u32 count + count × i32
//! locvars           : u32 count + { string, i32 startpc, i32 endpc }
//! upvalues          : u32 count + { string, u8 instack, u8 idx }
//! ```
//!
//! **Validation rule**: parsing consumes the file EXACTLY (zero
//! leftover bytes).  PROVEN on all three real samples:
//! - `frevents.lua` (4383 bytes) — exact, 3 protos (main + 2 nested)
//! - `scripts.rcf` Lua entry @ offset 184320 (2318 bytes) — exact, 1 proto
//! - `scripts.rcf` Lua entry @ offset 167936 (4383 bytes) — exact, 3 protos
//!
//! **Stripped debug info (PROVEN)**: Prototype ships Lua bytecode with
//! `sizelineinfo = 0` and `sizeupvalues_dbg = 0` even when `nups > 0`
//! and `sizecode > 0`.  The parser must NOT assume
//! `lineinfo.len() == code.len()`.

#![forbid(unsafe_code)]

use prototype_common::{P3dError, Reader, Result, LUA_MAGIC};

pub mod vm;

pub use vm::{
    ClosureVal, HostCall, HostCallable, Table, Value, Vm, LFIELDS_PER_FLUSH,
};

// ---------------------------------------------------------------------------
// Header (unchanged from v1)
// ---------------------------------------------------------------------------

/// 12-byte Lua 5.1 bytecode file header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lua51Header {
    pub magic: [u8; 4],
    pub version: u8,
    pub format: u8,
    pub endian: u8,
    pub sizeof_int: u8,
    pub sizeof_size_t: u8,
    pub sizeof_instruction: u8,
    pub sizeof_lua_number: u8,
    pub integral_flag: u8,
}

impl Lua51Header {
    pub fn parse(data: &[u8]) -> Result<(Self, Reader<'_>)> {
        let mut r = Reader::new(data);
        let magic_raw = r.read_bytes(4)?;
        if magic_raw != LUA_MAGIC {
            return Err(P3dError::Other(format!(
                "invalid Lua magic at offset 0: expected {LUA_MAGIC:?}, got {magic_raw:?}"
            )));
        }
        let version = r.read_u8()?;
        let format = r.read_u8()?;
        let endian = r.read_u8()?;
        let sizeof_int = r.read_u8()?;
        let sizeof_size_t = r.read_u8()?;
        let sizeof_instruction = r.read_u8()?;
        let sizeof_lua_number = r.read_u8()?;
        let integral_flag = r.read_u8()?;
        if version != 0x51 {
            return Err(P3dError::Other(format!(
                "unsupported Lua bytecode version 0x{version:02x} (only 5.1/0x51)"
            )));
        }
        if sizeof_int != 4 || sizeof_size_t != 4 || sizeof_instruction != 4 {
            return Err(P3dError::Other(format!(
                "unsupported Lua sizes: int={sizeof_int} size_t={sizeof_size_t} instr={sizeof_instruction} (expected 4/4/4)"
            )));
        }
        if sizeof_lua_number != 4 || integral_flag != 0 {
            return Err(P3dError::Other(format!(
                "unexpected lua_Number config: size={sizeof_lua_number} integral={integral_flag} \
                 (Prototype uses 4-byte float)"
            )));
        }
        let mut magic = [0u8; 4];
        magic.copy_from_slice(magic_raw);
        Ok((
            Self {
                magic,
                version,
                format,
                endian,
                sizeof_int,
                sizeof_size_t,
                sizeof_instruction,
                sizeof_lua_number,
                integral_flag,
            },
            r,
        ))
    }

    pub fn is_lua_51(&self) -> bool { self.version == 0x51 }
    pub fn uses_float_lua_number(&self) -> bool {
        self.sizeof_lua_number == 4 && self.integral_flag == 0
    }
}

// ---------------------------------------------------------------------------
// Values / constants
// ---------------------------------------------------------------------------

/// A Lua constant value (tagged union per the 5.1 dump format).
#[derive(Debug, Clone, PartialEq)]
pub enum LuaValue {
    Nil,
    Bool(bool),
    /// Prototype's `lua_Number` is a 4-byte float (PROVEN).
    Num(f32),
    Str(Vec<u8>),
}

impl LuaValue {
    /// Best-effort display string.
    pub fn display(&self) -> String {
        match self {
            LuaValue::Nil => "nil".into(),
            LuaValue::Bool(b) => b.to_string(),
            LuaValue::Num(n) => format!("{n}"),
            LuaValue::Str(s) => format!("{:?}", String::from_utf8_lossy(s)),
        }
    }
}

// ---------------------------------------------------------------------------
// Instructions
// ---------------------------------------------------------------------------

/// A decoded Lua 5.1 instruction (raw u32 plus decoded fields).
///
/// 5.1 encoding (verified against lua.org 5.1.5 `lopcodes.h`):
/// `SIZE_A=8, SIZE_C=9, SIZE_B=9, SIZE_Bx=18` with
/// `POS_A=6, POS_C=14, POS_B=23, POS_Bx=14` — i.e. the word is
/// `Op(6) | A(8) | C(9) | B(9)` for iABC and
/// `Op(6) | A(8) | Bx(18)` for iABx/iAsBx (sBx biased by 131071).
/// (`A` is 8 bits — a 9-bit read into A silently ORs in C's top
/// bit, which is exactly what exposed this during M8 VM work.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instruction {
    pub raw: u32,
    pub opcode: u8,
    pub a: u32,
    pub b: u32,
    pub c: u32,
    pub bx: u32,
    pub sbx: i32,
}

impl Instruction {
    pub fn decode(raw: u32) -> Self {
        let opcode = (raw & 0x3f) as u8;
        let a = (raw >> 6) & 0xff;
        let c = (raw >> 14) & 0x1ff;
        let b = (raw >> 23) & 0x1ff;
        let bx = (raw >> 14) & 0x3ffff;
        let sbx = bx as i32 - 131_071;
        Self { raw, opcode, a, b, c, bx, sbx }
    }

    /// Opcode name (Lua 5.1 instruction set).
    pub fn opname(&self) -> &'static str {
        OPCODE_NAMES
            .get(self.opcode as usize)
            .copied()
            .unwrap_or("OP_?")
    }
}

/// Lua 5.1 opcode names, in order (0..37) + OP_VARARG etc.
pub const OPCODE_NAMES: [&str; 40] = [
    "MOVE", "LOADK", "LOADBOOL", "LOADNIL", "GETUPVAL", "GETGLOBAL", "GETTABLE",
    "SETGLOBAL", "SETUPVAL", "SETTABLE", "NEWTABLE", "SELF", "ADD", "SUB",
    "MUL", "DIV", "MOD", "POW", "UNM", "NOT", "LEN", "CONCAT", "JMP", "EQ",
    "LT", "LE", "TEST", "TESTSET", "CALL", "TAILCALL", "RETURN", "FORLOOP",
    "FORPREP", "TFORLOOP", "SETLIST", "CLOSE", "CLOSURE", "VARARG", "OP38",
    "OP39",
];

// ---------------------------------------------------------------------------
// Proto (function prototype)
// ---------------------------------------------------------------------------

/// A parsed Lua 5.1 function prototype.
#[derive(Debug, Clone, Default)]
pub struct Proto {
    /// Source name (usually NULL/empty for compiled game scripts).
    pub source: Option<Vec<u8>>,
    pub linedefined: i32,
    pub lastlinedefined: i32,
    /// Upvalue count.  PROVEN to precede `nparams` (ldump.c order):
    /// nested protos in `frevents.lua` have `nups = 1` with a stripped
    /// (zero-entry) upvalue debug section.
    pub nups: u8,
    pub nparams: u8,
    pub is_vararg: u8,
    pub maxstacksize: u8,
    /// Raw instruction words.
    pub code: Vec<u32>,
    /// Constant pool.
    pub constants: Vec<LuaValue>,
    /// Nested function prototypes.
    pub protos: Vec<Proto>,
    /// Per-instruction source line numbers.
    pub lineinfo: Vec<i32>,
    /// Local variable debug info.
    pub locvars: Vec<LocalVar>,
    /// Upvalue debug info.
    pub upvalues: Vec<UpvalDebug>,
}

/// Local-variable debug entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalVar {
    pub name: Vec<u8>,
    pub startpc: i32,
    pub endpc: i32,
}

/// Upvalue debug entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpvalDebug {
    pub name: Vec<u8>,
    pub instack: u8,
    pub idx: u8,
}

impl Proto {
    /// Parse one Proto at the cursor.
    pub fn parse(r: &mut Reader<'_>) -> Result<Self> {
        // source: u32 len + bytes (0 = NULL)
        let src_len = r.read_u32()? as usize;
        let source = if src_len == 0 {
            None
        } else {
            if src_len > r.remaining() {
                return Err(P3dError::BadLength {
                    offset: r.pos,
                    value: src_len as u32,
                    remaining: r.remaining(),
                });
            }
            Some(r.read_bytes(src_len)?.to_vec())
        };
        let linedefined = r.read_u32()? as i32;
        let lastlinedefined = r.read_u32()? as i32;
        // PROVEN order (Lua 5.1 ldump.c DumpFunction): nups BEFORE nparams.
        let nups = r.read_u8()?;
        let nparams = r.read_u8()?;
        let is_vararg = r.read_u8()?;
        let maxstacksize = r.read_u8()?;

        // code
        let ncode = r.read_u32()? as usize;
        if ncode > 1 << 22 {
            return Err(P3dError::Other(format!("implausible code count {ncode}")));
        }
        let mut code = Vec::with_capacity(ncode);
        for _ in 0..ncode {
            code.push(r.read_u32()?);
        }

        // constants
        let nk = r.read_u32()? as usize;
        if nk > 1 << 22 {
            return Err(P3dError::Other(format!("implausible constant count {nk}")));
        }
        let mut constants = Vec::with_capacity(nk);
        for _ in 0..nk {
            let tag = r.read_u8()?;
            constants.push(match tag {
                0 => LuaValue::Nil,
                1 => LuaValue::Bool(r.read_u8()? != 0),
                3 => {
                    let bits = r.read_u32()?;
                    LuaValue::Num(f32::from_bits(bits))
                }
                4 => {
                    // PROVEN: the dumped size INCLUDES the trailing NUL
                    // byte (ldump.c DumpString writes strlen+1).
                    let len = r.read_u32()? as usize;
                    if len == 0 || len > r.remaining() {
                        return Err(P3dError::BadLength {
                            offset: r.pos,
                            value: len as u32,
                            remaining: r.remaining(),
                        });
                    }
                    let mut s = r.read_bytes(len)?.to_vec();
                    if s.last() == Some(&0) {
                        s.pop();
                    }
                    LuaValue::Str(s)
                }
                other => {
                    return Err(P3dError::Other(format!(
                        "unknown Lua constant tag {other} at offset {}",
                        r.pos
                    )))
                }
            });
        }

        // nested protos
        let np = r.read_u32()? as usize;
        if np > 1 << 16 {
            return Err(P3dError::Other(format!("implausible proto count {np}")));
        }
        let mut protos = Vec::with_capacity(np);
        for _ in 0..np {
            protos.push(Self::parse(r)?);
        }

        // lineinfo
        let nl = r.read_u32()? as usize;
        if nl > 1 << 22 {
            return Err(P3dError::Other(format!("implausible lineinfo count {nl}")));
        }
        let mut lineinfo = Vec::with_capacity(nl);
        for _ in 0..nl {
            lineinfo.push(r.read_u32()? as i32);
        }

        // locvars
        let nlv = r.read_u32()? as usize;
        if nlv > 1 << 16 {
            return Err(P3dError::Other(format!("implausible locvar count {nlv}")));
        }
        let mut locvars = Vec::with_capacity(nlv);
        for _ in 0..nlv {
            let len = r.read_u32()? as usize;
            if len > r.remaining() {
                return Err(P3dError::BadLength {
                    offset: r.pos,
                    value: len as u32,
                    remaining: r.remaining(),
                });
            }
            let mut name = r.read_bytes(len)?.to_vec();
            if name.last() == Some(&0) {
                name.pop();
            }
            let startpc = r.read_u32()? as i32;
            let endpc = r.read_u32()? as i32;
            locvars.push(LocalVar { name, startpc, endpc });
        }

        // upvalues
        let nu = r.read_u32()? as usize;
        if nu > 1 << 16 {
            return Err(P3dError::Other(format!("implausible upvalue count {nu}")));
        }
        let mut upvalues = Vec::with_capacity(nu);
        for _ in 0..nu {
            let len = r.read_u32()? as usize;
            if len > r.remaining() {
                return Err(P3dError::BadLength {
                    offset: r.pos,
                    value: len as u32,
                    remaining: r.remaining(),
                });
            }
            let mut name = r.read_bytes(len)?.to_vec();
            if name.last() == Some(&0) {
                name.pop();
            }
            let instack = r.read_u8()?;
            let idx = r.read_u8()?;
            upvalues.push(UpvalDebug { name, instack, idx });
        }

        Ok(Self {
            source,
            linedefined,
            lastlinedefined,
            nups,
            nparams,
            is_vararg,
            maxstacksize,
            code,
            constants,
            protos,
            lineinfo,
            locvars,
            upvalues,
        })
    }

    /// Total number of instructions in this proto and all children.
    pub fn total_instructions(&self) -> usize {
        self.code.len()
            + self.protos.iter().map(Proto::total_instructions).sum::<usize>()
    }

    /// The constant at a `Bx` index, if in range.
    pub fn constant(&self, bx: u32) -> Option<&LuaValue> {
        self.constants.get(bx as usize)
    }

    /// The string constant at a `Bx` index, if it is one.
    pub fn constant_str(&self, bx: u32) -> Option<&[u8]> {
        match self.constants.get(bx as usize) {
            Some(LuaValue::Str(s)) => Some(s),
            _ => None,
        }
    }

    /// Global-variable NAMES this proto reads (`GETGLOBAL`, opcode 5).
    ///
    /// GETGLOBAL encodes the global's name as a string constant at
    /// `Bx` (Lua 5.1 `OP_GETGLOBAL A Bx  R[A] := Gbl[K[Bx]]`).  This
    /// recovers the script's *imports* — which engine-provided
    /// functions and tables it consumes.
    pub fn globals_read(&self) -> Vec<&[u8]> {
        self.code
            .iter()
            .filter_map(|&raw| {
                let ins = Instruction::decode(raw);
                if ins.opcode == 5 {
                    self.constant_str(ins.bx)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Global-variable NAMES this proto writes (`SETGLOBAL`, opcode 7).
    ///
    /// This recovers the script's *exports* — the entry points the
    /// engine (or other scripts) can call.
    pub fn globals_written(&self) -> Vec<&[u8]> {
        self.code
            .iter()
            .filter_map(|&raw| {
                let ins = Instruction::decode(raw);
                if ins.opcode == 7 {
                    self.constant_str(ins.bx)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Union of [`Proto::globals_read`] over this proto and all
    /// nested protos (order-preserving, first occurrence first).
    pub fn all_globals_read(&self) -> Vec<Vec<u8>> {
        let mut out: Vec<Vec<u8>> = Vec::new();
        for g in self.globals_read() {
            if !out.iter().any(|o| o == g) {
                out.push(g.to_vec());
            }
        }
        for p in &self.protos {
            for g in p.all_globals_read() {
                if !out.iter().any(|o| o == &g) {
                    out.push(g);
                }
            }
        }
        out
    }

    /// Union of [`Proto::globals_written`] over this proto and all
    /// nested protos (order-preserving, first occurrence first).
    pub fn all_globals_written(&self) -> Vec<Vec<u8>> {
        let mut out: Vec<Vec<u8>> = Vec::new();
        for g in self.globals_written() {
            if !out.iter().any(|o| o == g) {
                out.push(g.to_vec());
            }
        }
        for p in &self.protos {
            for g in p.all_globals_written() {
                if !out.iter().any(|o| o == &g) {
                    out.push(g);
                }
            }
        }
        out
    }

    /// Collect all string constants in this proto and children.
    pub fn all_strings(&self, out: &mut Vec<Vec<u8>>) {
        for c in &self.constants {
            if let LuaValue::Str(s) = c {
                out.push(s.clone());
            }
        }
        for p in &self.protos {
            p.all_strings(out);
        }
    }

    /// Simple text disassembly (one instruction per line).
    pub fn disassemble(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "; function lines {}..{}  params={} vararg={} stack={}\n",
            self.linedefined, self.lastlinedefined, self.nparams, self.is_vararg, self.maxstacksize
        ));
        for (i, &raw) in self.code.iter().enumerate() {
            let ins = Instruction::decode(raw);
            let line = self.lineinfo.get(i).copied().unwrap_or(-1);
            out.push_str(&format!(
                "{i:5}  [{line:>4}]  {}  A={} B={} C={}\n",
                ins.opname(),
                ins.a,
                ins.b,
                ins.c
            ));
        }
        for (i, p) in self.protos.iter().enumerate() {
            out.push_str(&format!("\n; proto #{i}\n"));
            out.push_str(&p.disassemble());
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Whole-file parser
// ---------------------------------------------------------------------------

/// A parsed Lua 5.1 bytecode file: header + main proto.
#[derive(Debug, Clone)]
pub struct Lua51File {
    pub header: Lua51Header,
    /// The main function prototype.
    pub main: Proto,
    /// Bytes left over after parsing the main proto — MUST be 0 for
    /// valid Prototype scripts (PROVEN on all samples).
    pub trailing: Vec<u8>,
}

impl Lua51File {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let (header, mut r) = Lua51Header::parse(data)?;
        let main = Proto::parse(&mut r)?;
        let trailing = r.data[r.pos..].to_vec();
        Ok(Self { header, main, trailing })
    }

    /// True when the whole file was consumed by the proto tree.
    pub fn is_exact(&self) -> bool {
        self.trailing.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
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

#[test]
fn throwaway_dump_frevents() {
    let data = fixture("frevents.lua");
    let f = Lua51File::parse(&data).unwrap();
    println!("=== MAIN raw tail ===");
    for (i, &raw) in f.main.code.iter().enumerate().skip(648) {
        let ins = Instruction::decode(raw);
        println!("{i:4} raw={raw:#010x} {} A={} B={} C={} Bx={:#x}", ins.opname(), ins.a, ins.b, ins.c, ins.bx);
    }
    println!("constants 96..103:");
    for (i, k) in f.main.constants.iter().enumerate().skip(96).take(8) {
        println!("  K{i} = {}", k.display());
    }
    println!("=== MAIN ===");
    println!("{}", f.main.disassemble());
    for (i, p) in f.main.protos.iter().enumerate() {
        println!("=== NESTED {i} (params={}, vararg={}, nups={}) ===", p.nparams, p.is_vararg, p.nups);
        println!("{}", p.disassemble());
    }
}

    #[test]
    fn frevents_lua_parses_exactly() {
        let data = fixture("frevents.lua");
        let f = Lua51File::parse(&data).expect("frevents.lua must parse");
        assert_eq!(f.header.version, 0x51);
        assert_eq!(f.header.sizeof_lua_number, 4);
        assert!(f.header.uses_float_lua_number());
        // EXACT consumption — the decisive validation of the layout.
        assert!(f.is_exact(), "leftover bytes: {}", f.trailing.len());
        // Main proto shape (PROVEN from the byte dump).
        assert_eq!(f.main.source, None);
        assert_eq!(f.main.linedefined, 0);
        assert_eq!(f.main.nups, 0);
        assert_eq!(f.main.nparams, 0);
        assert_eq!(f.main.is_vararg, 2);
        assert_eq!(f.main.maxstacksize, 53);
        // 672 instructions in the main proto.
        assert_eq!(f.main.code.len(), 672);
        // First instruction decodes to NEWTABLE (opcode 10).
        let first = Instruction::decode(f.main.code[0]);
        assert_eq!(first.opname(), "NEWTABLE");
        // Constants: 94 strings + 8 numbers + 1 bool = 103 (PROVEN).
        assert_eq!(f.main.constants.len(), 103);
        let strs = f
            .main
            .constants
            .iter()
            .filter(|c| matches!(c, LuaValue::Str(_)))
            .count();
        assert_eq!(strs, 94);
        // Two nested closures (PROVEN): lines 558..563 and 566..571,
        // each 10 instructions with 1 upvalue and stripped debug info.
        assert_eq!(f.main.protos.len(), 2);
        assert_eq!(f.main.protos[0].linedefined, 558);
        assert_eq!(f.main.protos[0].lastlinedefined, 563);
        assert_eq!(f.main.protos[0].code.len(), 10);
        assert_eq!(f.main.protos[0].nups, 1);
        assert!(f.main.protos[0].lineinfo.is_empty(), "lineinfo is stripped");
        assert!(f.main.protos[0].upvalues.is_empty(), "upvalue debug is stripped");
        assert_eq!(f.main.protos[1].linedefined, 566);
        assert_eq!(f.main.protos[1].lastlinedefined, 571);
        // Line info stripped for the main proto too (672 code, 0 lines).
        assert!(f.main.lineinfo.is_empty(), "main lineinfo is stripped");
    }

    #[test]
    fn scripts_rcf_lua_entries_parse_exactly() {
        // The two Lua payloads inside scripts.rcf (via prototype-rcf).
        let data = fixture("scripts.rcf");
        let arc = prototype_rcf::RcfArchive::parse(&data).unwrap();
        let mut lua_count = 0;
        for e in &arc.entries {
            let p = arc.payload(e).unwrap();
            if prototype_rcf::PayloadKind::sniff(p) == prototype_rcf::PayloadKind::Lua51 {
                let f = Lua51File::parse(p)
                    .unwrap_or_else(|e| panic!("RCF Lua entry failed: {e}"));
                assert!(f.is_exact(), "leftover: {}", f.trailing.len());
                assert!(f.main.total_instructions() > 0);
                lua_count += 1;
            }
        }
        assert!(lua_count >= 1, "expected at least one Lua entry in scripts.rcf");
    }

    #[test]
    fn frevents_lua_is_byte_identical_to_scripts_rcf_entry() {
        // PROVEN: the frevents.lua on disk is shipped byte-identical
        // inside scripts.rcf (entry with offset 167936, size 4383).
        let free = fixture("frevents.lua");
        let rcf = fixture("scripts.rcf");
        let arc = prototype_rcf::RcfArchive::parse(&rcf).unwrap();
        let hit = arc
            .entries
            .iter()
            .filter_map(|e| arc.payload(e).ok())
            .any(|p| p == free.as_slice());
        assert!(hit, "frevents.lua must appear verbatim inside scripts.rcf");
    }

    #[test]
    fn frevents_globals_recover_event_system_exports() {
        // PROVEN (bytecode-level): frevents.lua defines the free-roam
        // event system's data table and lifecycle entry points.
        let data = fixture("frevents.lua");
        let f = Lua51File::parse(&data).unwrap();
        let written = f.main.all_globals_written();
        let written: Vec<&str> = written
            .iter()
            .map(|g| std::str::from_utf8(g).unwrap())
            .collect();
        assert_eq!(
            written,
            vec!["collectiblesDef", "ResetFreeRoamEvents", "DestroyFreeRoamEvents"]
        );
        // Imports: the table itself (4 call sites), the `pairs`
        // builtin, and the two engine-provided registration hooks.
        // `globals_read()` counts every GETGLOBAL occurrence (no dedup).
        let read: Vec<String> = f
            .main
            .globals_read()
            .iter()
            .map(|g| String::from_utf8_lossy(g).to_string())
            .collect();
        for expected in ["collectiblesDef", "pairs", "fre_RegisterEvent", "fre_RegisterCollectibles"] {
            assert!(read.iter().any(|g| g == expected), "frevents must read {expected:?}");
        }
        assert_eq!(read.iter().filter(|g| *g == "collectiblesDef").count(), 4);
    }

    #[test]
    fn mission_lua_calls_add_valid_mission_70_times() {
        // PROVEN: the 2 318-byte Lua entry inside scripts.rcf is a
        // mission-registration script — it reads `math` and
        // `util_GetRandomFloat` and calls `AddValidMission` exactly
        // 70 times (once per valid mission entry).
        let rcf = fixture("scripts.rcf");
        let arc = prototype_rcf::RcfArchive::parse(&rcf).unwrap();
        let mut found = false;
        for e in &arc.entries {
            let p = arc.payload(e).unwrap();
            if prototype_rcf::PayloadKind::sniff(p) == prototype_rcf::PayloadKind::Lua51
                && p.len() == 2318
            {
                let f = Lua51File::parse(p).unwrap();
                // occurrence-counting (no dedup): 71 AddValidMission sites
                let read: Vec<String> = f
                    .main
                    .globals_read()
                    .iter()
                    .map(|g| String::from_utf8_lossy(g).to_string())
                    .collect();
                assert!(read.iter().any(|g| g == "math"));
                assert!(read.iter().any(|g| g == "util_GetRandomFloat"));
                let calls = read.iter().filter(|g| *g == "AddValidMission").count();
                assert_eq!(calls, 70, "AddValidMission call sites");
                found = true;
            }
        }
        assert!(found, "the 2318-byte mission Lua must exist in scripts.rcf");
    }

    #[test]
    fn frevents_strings_are_frontend_event_definitions() {
        // The string constants name Prototype's free-roam event system
        // (PROVEN — recovered from the constant pool):
        // `collectiblesDef`, the freP/freW/freV event series, and
        // `fre3WW` (three-way war).
        let data = fixture("frevents.lua");
        let f = Lua51File::parse(&data).unwrap();
        let mut strings = Vec::new();
        f.main.all_strings(&mut strings);
        let have: Vec<String> = strings
            .iter()
            .map(|s| String::from_utf8_lossy(s).to_string())
            .collect();
        for expected in [
            "collectiblesDef",
            "maxHintCollectibles",
            "numScientistEvents",
            "freP00",
            "freP11",
            "freW00",
            "freW08",
            "freV00",
            "freV15",
            "fre3WW",
            "freKill",
            "scores",
            "isFloat",
        ] {
            assert!(
                have.iter().any(|s| s == expected),
                "constant pool must contain {expected:?}"
            );
        }
        // String constants are NUL-stripped (dump size includes the NUL).
        assert!(
            strings.iter().all(|s| !s.ends_with(b"\0")),
            "strings must be stored without the trailing NUL"
        );
    }

    #[test]
    fn instruction_decode_fields() {
        // MOVE R3 R2 → opcode 0, A=3, B=2
        let raw = (0u32) | (3 << 6) | (2 << 23);
        let i = Instruction::decode(raw);
        assert_eq!(i.opname(), "MOVE");
        assert_eq!(i.a, 3);
        assert_eq!(i.b, 2);
        // LOADK with Bx=0x1234
        let raw = 1u32 | (7 << 6) | (0x1234 << 14);
        let i = Instruction::decode(raw);
        assert_eq!(i.opname(), "LOADK");
        assert_eq!(i.a, 7);
        assert_eq!(i.bx, 0x1234);
    }

    #[test]
    fn disassembly_produces_text() {
        let data = fixture("frevents.lua");
        let f = Lua51File::parse(&data).unwrap();
        let dis = f.main.disassemble();
        assert!(dis.contains("NEWTABLE"));
        assert!(dis.lines().count() >= 672);
    }

    #[test]
    fn rejects_non_lua_data() {
        assert!(Lua51File::parse(b"not lua bytecode").is_err());
    }
}

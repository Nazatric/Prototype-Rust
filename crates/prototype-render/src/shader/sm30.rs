//! sm30 — decoder for the D3D9 Shader Model 3.0 token streams embedded
//! in Prototype (2009) P3D `0x0001100B` ShaderProgram chunks.
//!
//! Every decoding rule below was derived EMPIRICALLY from the 26 real
//! programs shipped with the game (24 in `frontendshaders.p3d` + 2 in
//! `atg\shaders\bink.p3d` inside `shaders.rcf`) and cross-checked
//! against the inline HLSL source and the CTAB constant tables that
//! accompany each program.  Nothing here is guessed: any token outside
//! the observed set produces an error, never a guess.
//!
//! ## Layout of a token stream (PROVEN 26/26)
//!
//! ```text
//! u32 version — 0xFFFE0300 (vs_3_0) for vertex, 0xFFFF0300 (ps_3_0)
//!               for fragment.  No other value observed.
//! tokens...:
//!   comment  — bit31 clear, low16 == 0xFFFE, HIGH16 = payload length
//!              in DWORDs.  The only comment observed is the constant
//!              table ("CTAB", see [`Ctab`]).
//!   instruction — bit31 CLEAR (bit31 marks operand tokens); opcode =
//!              bits 0..15; bits 24..27 = the number of operand DWORDs
//!              that follow (so an instruction spans n+1 DWORDs).
//!              PROVEN convention on 26/26 shipped P3D programs AND
//!              45/45 DLL blobs (the alternative "length includes the
//!              instruction token" reading accepts 0/45): the field
//!              counts ONLY the operand DWORDs; zero-operand
//!              flow-control instructions carry length 0.
//!              Observed opcodes and their operand counts (the shipped
//!              26 + the 45 DLL blobs — see [`scan_blobs`]):
//!                  1  MOV  (2)   5  MUL  (3)   12  SLT  (3)   43 ENDIF (0)
//!                  2  ADD  (3)   6  RCP  (2)   18  LRP  (4)   65 TKILL(1)
//!                  4  MAD  (4)   7  RSQ  (2)   31  DCL  (2)   66  TEX  (3)
//!                 10  MIN  (3)  19  FRC  (2)   81  DEF  (5)   90 DP2ADD(4)
//!                 11  MAX  (3)  32  POW  (3)   88  CMP  (4)
//!                  8  DP3  (3)  35  ABS  (2)   36  NRM  (2)   37 SINCOS(2)
//!                  9  DP4  (3)  40   IF  (1)   42 ELSE  (0)
//!              (counts are the operand DWORDs; DEF carries 4 raw f32;
//!              DP2ADD's 4th operand is a replicated scalar — the
//!              `dst, src, src, c.replicate` signature pinned on the
//!              real instructions.)
//!   end      — 0x0000FFFF.
//!   comments — the shipped programs carry exactly one comment (the
//!              CTAB); every DLL blob additionally carries a `DBUG`
//!              comment BEFORE the CTAB — a cgc debug blob whose exact
//!              binary layout was NOT derived, but whose printable
//!              ASCII runs are stable 45/45 and start with the Cg
//!              source path (e.g.
//!              `c:\\workspace\\outrun\\...\\cg_shaders\\bink_vp.cg`),
//!              then the entry/struct/semantic names
//!              (`bink_vp`, `__vsIn`, `p3dVertex_Position`,
//!              `p3dVertex_Texcoord0`, `__vsOut`, `p3dOutputPosition`,
//!              `TexCoord_0`, the compiler signature).
//!   params   — a chain of records, each: u32 type == 0x0001100D,
//!              u32 uid, u32 size (from the type field, like every
//!              P3D prelude), then size-12 payload bytes:
//!              u8 namelen, name (namelen bytes, NUL padded),
//!              6×u32: {type_id, register_packed, magic, 16, 16, count}
//!              where register_packed = (flag<<16)|register_index and
//!              magic == 0x00011115 in every observed record.
//!              NOTE: these records are BYTE-packed — their sizes are
//!              not multiples of 4 and no padding follows them, so the
//!              tail of the stream is generally not u32-aligned.
//! ```
//!
//! ## Parameter-record ↔ CTAB cross-check (PROVEN invariant)
//!
//! For every program the parameter records agree with the CTAB: the
//! record's register index equals the CTAB constant's register index
//! and the record's count equals the CTAB register count (4 for the
//! float4x4 matrix, 1 for everything else).  [`Sm30Shader::cross_check`]
//! asserts this invariant.
//!
//! ## Parameter-record type ids (empirical mapping, semantics UNKNOWN)
//!
//! | uid | type_id | observed meaning (from CTAB class/type)  |
//! |-----|---------|------------------------------------------|
//! | 29  | 1       | float scalar                             |
//! | 29  | 4       | float4 vector                            |
//! | 33  | 6       | sampler2D                                |
//! | 41  | 10      | bool-ish flag (p3dLight_IsLitPass)       |
//! | 49  | 5       | float4x4 matrix                          |

#![forbid(unsafe_code)]

use std::fmt;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Everything that can go wrong while decoding an SM 3.0 token stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sm30Error {
    /// The first token is not a recognized shader version token.
    BadVersion(u32),
    /// Ran off the end of the token slice before finding END.
    UnexpectedEof,
    /// A parameter token appeared where an instruction was expected
    /// (instruction tokens have bit31 CLEAR; operands carry bit31).
    BadInstructionToken { offset: usize, token: u32 },
    /// An opcode outside the observed shipped set.
    UnknownOpcode { offset: usize, opcode: u16 },
    /// An instruction whose operand count differs from the observed one.
    BadOperandCount {
        offset: usize,
        opcode: u16,
        expected: usize,
        got: usize,
    },
    /// A parameter token without bit 31 set.
    BadParamToken { offset: usize, token: u32 },
    /// A register type outside the observed set.
    UnknownRegType { offset: usize, bits: u8 },
    /// A source-modifier code outside the public D3D9 `D3DSHADER_SRCMOD_*`
    /// set (0..13).
    UnknownSrcMod { offset: usize, bits: u8 },
    /// A DCL usage outside the observed set.
    UnknownUsage { offset: usize, usage: u8 },
    /// A DCL register type that does not fit the shader stage.
    BadDclTarget {
        offset: usize,
        is_fragment: bool,
        reg_type: u8,
    },
    /// Malformed CTAB comment.
    BadCtab(&'static str),
    /// Malformed or missing post-END parameter record.
    BadParamRecord(String),
    /// Trailing bytes after the last parameter record.
    TrailingBytes(usize),
}

impl fmt::Display for Sm30Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Sm30Error::BadVersion(t) => write!(f, "unknown shader version token 0x{t:08X}"),
            Sm30Error::UnexpectedEof => write!(f, "token stream ended before END token"),
            Sm30Error::BadInstructionToken { offset, token } => {
                write!(f, "parameter token 0x{token:08X} in instruction position at byte offset {offset}")
            }
            Sm30Error::UnknownOpcode { offset, opcode } => {
                write!(f, "unknown opcode {opcode} at token offset {offset}")
            }
            Sm30Error::BadOperandCount { offset, opcode, expected, got } => write!(
                f,
                "opcode {opcode} at token offset {offset}: expected {expected} operand tokens, got {got}"
            ),
            Sm30Error::BadParamToken { offset, token } => {
                write!(f, "operand at token offset {offset} is not a parameter token (0x{token:08X})")
            }
            Sm30Error::UnknownRegType { offset, bits } => {
                write!(f, "unknown register type bits {bits} at token offset {offset}")
            }
            Sm30Error::UnknownSrcMod { offset, bits } => {
                write!(f, "unknown source-modifier code {bits} at token offset {offset}")
            }
            Sm30Error::UnknownUsage { offset, usage } => {
                write!(f, "unknown DCL usage {usage} at token offset {offset}")
            }
            Sm30Error::BadDclTarget { offset, is_fragment, reg_type } => write!(
                f,
                "DCL register type {reg_type} invalid for {} shader at token offset {offset}",
                if *is_fragment { "fragment" } else { "vertex" }
            ),
            Sm30Error::BadCtab(why) => write!(f, "malformed CTAB comment: {why}"),
            Sm30Error::BadParamRecord(why) => write!(f, "malformed parameter record: {why}"),
            Sm30Error::TrailingBytes(n) => write!(f, "{n} unexpected bytes after parameter records"),
        }
    }
}

impl std::error::Error for Sm30Error {}

// ---------------------------------------------------------------------------
// Operand model
// ---------------------------------------------------------------------------

/// D3DSPR register type (5 bits: bits 28..30 + bit 11 of the token),
/// numbered per the PUBLIC D3D9 `D3DSPR_*` enum.
///
/// Empirical evidence: values 0..10 appear in the 26 shipped programs
/// (their old ad-hoc numbering happened to coincide with the public one
/// there); value 14 (`CONSTBOOL`) is PROVEN by the DLL blobs — the
/// three flow-controlled world fragment shaders branch with
/// `if b#` (register type 14, registers 0..2, [`Sm30Op::If`]).
/// Values 11..13 and 15..19 follow the public enum and have never been
/// observed in any Prototype token stream; they decode (so a future
/// stream does not hard-fail on a known public value) but no test
/// pins their semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegType {
    Temp,
    Input,
    Const,
    AddrTexture,
    RastOut,
    AttrOut,
    Output,
    ConstInt,
    ColorOut,
    DepthOut,
    Sampler,
    /// Public `D3DSPR_CONST2` (never observed).
    Const2,
    /// Public `D3DSPR_CONST3` (never observed).
    Const3,
    /// Public `D3DSPR_CONST4` (never observed).
    Const4,
    /// Public `D3DSPR_CONSTBOOL` — the `if b#` branch condition
    /// register (PROVEN: DLL world-shader blobs).
    ConstBool,
    /// Public `D3DSPR_LOOP` (never observed).
    Loop,
    /// Public `D3DSPR_TEMPFLOAT16` (never observed).
    TempFloat16,
    /// Public `D3DSPR_MISCTYPE` (vPos/vFace; never observed).
    MiscType,
    /// Public `D3DSPR_LABEL` (never observed).
    Label,
    /// Public `D3DSPR_PREDICATE` (never observed).
    Predicate,
    /// A type id outside the public D3DSPR set.
    Other(u8),
}

impl RegType {
    fn from_bits(bits: u8) -> Self {
        match bits {
            0 => RegType::Temp,
            1 => RegType::Input,
            2 => RegType::Const,
            3 => RegType::AddrTexture,
            4 => RegType::RastOut,
            5 => RegType::AttrOut,
            6 => RegType::Output,
            7 => RegType::ConstInt,
            8 => RegType::ColorOut,
            9 => RegType::DepthOut,
            10 => RegType::Sampler,
            11 => RegType::Const2,
            12 => RegType::Const3,
            13 => RegType::Const4,
            14 => RegType::ConstBool,
            15 => RegType::Loop,
            16 => RegType::TempFloat16,
            17 => RegType::MiscType,
            18 => RegType::Label,
            19 => RegType::Predicate,
            other => RegType::Other(other),
        }
    }
}

impl fmt::Display for RegType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            RegType::Temp => "r",
            RegType::Input => "v",
            RegType::Const => "c",
            RegType::AddrTexture => "a",
            RegType::RastOut => "rastout",
            RegType::AttrOut => "attrout",
            RegType::Output => "o",
            RegType::ConstInt => "i",
            RegType::ColorOut => "oC",
            RegType::DepthOut => "oDepth",
            RegType::Sampler => "s",
            RegType::Const2 | RegType::Const3 | RegType::Const4 => "c",
            RegType::ConstBool => "b",
            RegType::Loop => "aL",
            RegType::TempFloat16 => "rH",
            RegType::MiscType => "m",
            RegType::Label => "l",
            RegType::Predicate => "p",
            RegType::Other(b) => return write!(f, "T{b}"),
        };
        f.write_str(s)
    }
}

/// Source-register modifier (bits 24..27 of a source token), numbered
/// per the PUBLIC D3D9 `D3DSHADER_SRCMOD_*` enum plus the two ps_3_0
/// `_abs`/`-_abs` extensions.
///
/// Empirical evidence (block 31 census over all 45 DLL blobs, 1649
/// modified/unmodified source tokens): 0 (`None`) ×1557, 1 (`Neg`) ×90,
/// 11 ×6 and 12 ×2.  11 = `Dw` is PROVEN-by-semantics: every one of the
/// 6 occurrences sits on a projective divide (`_dw(temp)` in the
/// shadow-map branches of default_fp/shadowdebug_fp and the point-sprite
/// size divide y/w of pointsprite_vp — the public
/// `D3DSHADER_SRCMOD_DW = 11`).  12 = `Abs` (the ps_3_0 `_abs`
/// extension): both occurrences are `add r, _abs(r'), bias` over a
/// depth-difference in shadowblur_fp.  Values 2..10 and 13 follow the
/// public enum order and have never been observed; the translator
/// rejects them (fail-closed) and only 0/1/11/12 get WGSL rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SrcMod {
    None,
    Neg,
    Bias,
    BiasNeg,
    Sign,
    SignNeg,
    Comp,
    CompNeg,
    X2,
    X2Neg,
    Dz,
    Dw,
    Abs,
    AbsNeg,
}

impl SrcMod {
    /// `None` (the Option) for codes outside the public 0..13 set — the
    /// caller turns that into a fail-closed decode error.
    fn from_bits(bits: u8) -> Option<Self> {
        Some(match bits {
            0 => SrcMod::None,
            1 => SrcMod::Neg,
            2 => SrcMod::Bias,
            3 => SrcMod::BiasNeg,
            4 => SrcMod::Sign,
            5 => SrcMod::SignNeg,
            6 => SrcMod::Comp,
            7 => SrcMod::CompNeg,
            8 => SrcMod::X2,
            9 => SrcMod::X2Neg,
            10 => SrcMod::Dz,
            11 => SrcMod::Dw,
            12 => SrcMod::Abs,
            13 => SrcMod::AbsNeg,
            _ => return None, // 14/15 unobserved
        })
    }
}

impl fmt::Display for SrcMod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            SrcMod::None => "",
            SrcMod::Neg => "-",
            SrcMod::Bias => "_bias",
            SrcMod::BiasNeg => "-_bias",
            SrcMod::Sign => "_sign",
            SrcMod::SignNeg => "-_sign",
            SrcMod::Comp => "_comp",
            SrcMod::CompNeg => "-_comp",
            SrcMod::X2 => "_x2",
            SrcMod::X2Neg => "-_x2",
            SrcMod::Dz => "_dz",
            SrcMod::Dw => "_dw",
            SrcMod::Abs => "_abs",
            SrcMod::AbsNeg => "-_abs",
        };
        f.write_str(s)
    }
}

/// One decoded parameter token (destination or source operand).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamToken {
    /// Register number (bits 0..10).
    pub reg: u16,
    /// Register type.
    pub ty: RegType,
    /// Destination write mask (bits 16..19); meaningful for dst only.
    pub writemask: [bool; 4],
    /// Source swizzle (bits 16..23), one `xyzw` selector per component.
    pub swizzle: [u8; 4],
    /// Source modifier.
    pub src_mod: SrcMod,
    /// Destination saturate modifier (bit 20).
    pub saturate: bool,
    /// Destination partial-precision modifier (bit 21).
    pub partial_precision: bool,
    /// Whether this token was decoded as a DESTINATION operand (its
    /// bits 16..19 are a write mask rather than a swizzle).  Only
    /// affects [`Display`].
    pub is_dst: bool,
}

impl ParamToken {
    fn decode(t: u32, offset: usize) -> Result<Self, Sm30Error> {
        Self::decode_op(t, offset, false)
    }

    fn decode_dst(t: u32, offset: usize) -> Result<Self, Sm30Error> {
        Self::decode_op(t, offset, true)
    }

    fn decode_op(t: u32, offset: usize, is_dst: bool) -> Result<Self, Sm30Error> {
        if t & 0x8000_0000 == 0 {
            return Err(Sm30Error::BadParamToken { offset, token: t });
        }
        // Register type: bits 28..30 plus bit 11 as the 4th bit.
        let ty_bits = (((t >> 28) & 0x7) | ((t >> 11) & 0x1) << 3) as u8;
        let ty = RegType::from_bits(ty_bits);
        if let RegType::Other(_) = ty {
            return Err(Sm30Error::UnknownRegType { offset, bits: ty_bits });
        }
        let writemask = [(t >> 16) & 1 == 1, (t >> 17) & 1 == 1, (t >> 18) & 1 == 1, (t >> 19) & 1 == 1];
        let sw = ((t >> 16) & 0xFF) as u8;
        let swizzle = [sw & 3, (sw >> 2) & 3, (sw >> 4) & 3, (sw >> 6) & 3];
        let src_mod = SrcMod::from_bits(((t >> 24) & 0xF) as u8).ok_or(Sm30Error::UnknownSrcMod {
            offset,
            bits: ((t >> 24) & 0xF) as u8,
        })?;
        Ok(Self {
            reg: (t & 0x7FF) as u16,
            ty,
            writemask,
            swizzle,
            src_mod,
            saturate: t & 0x0010_0000 != 0,
            partial_precision: t & 0x0020_0000 != 0,
            is_dst,
        })
    }

    /// `true` if the swizzle is the identity (`xyzw`).
    pub fn swizzle_is_identity(&self) -> bool {
        self.swizzle == [0, 1, 2, 3]
    }

    /// Swizzle as a `xyzw` string.
    pub fn swizzle_str(&self) -> String {
        const C: [char; 4] = ['x', 'y', 'z', 'w'];
        self.swizzle.iter().map(|&c| C[c as usize]).collect()
    }

    /// Write mask as a `xyzw` string (empty for the full mask).
    pub fn writemask_str(&self) -> String {
        const C: [char; 4] = ['x', 'y', 'z', 'w'];
        self.writemask
            .iter()
            .enumerate()
            .filter(|(_, b)| **b)
            .map(|(i, _)| C[i])
            .collect()
    }
}

impl fmt::Display for ParamToken {
    /// Disassembly-style rendering: destinations print their write
    /// mask (`r0.xyzw`, `oC0.xyz`), sources print their swizzle
    /// (`c1.xyzw`, `v0.yyyy`), with the source modifier prefix.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}{}", self.src_mod, self.ty, self.reg)?;
        match self.ty {
            RegType::Sampler => Ok(()),
            _ if self.is_dst => {
                let m = self.writemask_str();
                if m.is_empty() || m == "xyzw" {
                    write!(f, ".xyzw")
                } else {
                    write!(f, ".{m}")
                }
            }
            _ => write!(f, ".{}", self.swizzle_str()),
        }
    }
}

// ---------------------------------------------------------------------------
// Instructions
// ---------------------------------------------------------------------------

/// The opcodes observed in Prototype token streams — the 26 shipped
/// programs (frontend + Bink) plus the 45 blobs embedded in
/// `prototypeenginef.dll` (see [`scan_blobs`]).  Any other opcode is a
/// decode error (never guessed).  Operand counts are PROVEN per
/// occurrence by the length field (consistent across every occurrence
/// in every stream, see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Sm30Op {
    /// 1 — dst, src.
    Mov,
    /// 2 — dst, a, b.
    Add,
    /// 4 — dst, a, b, c  (dst = a*b + c).
    Mad,
    /// 5 — dst, a, b.
    Mul,
    /// 6 — dst, src (reciprocal; DLL blobs only).
    Rcp,
    /// 7 — dst, src (reciprocal square root; DLL blobs only).
    Rsq,
    /// 8 — dst, a, b  (3-component dot product; bink shaders).
    Dp3,
    /// 9 — dst, a, b  (4-component dot product).
    Dp4,
    /// 10 — dst, a, b  (component minimum; DLL blobs only).
    Min,
    /// 11 — dst, a, b  (component maximum; DLL blobs only).
    Max,
    /// 12 — dst, a, b  (dst = a < b ? 1 : 0; DLL blobs only).
    Slt,
    /// 18 — dst, a, b, c  (linear interpolate).
    Lrp,
    /// 19 — dst, src (fractional part; DLL blobs only).
    Frc,
    /// 31 — usage token + register.
    Dcl,
    /// 32 — dst, a, b  (pow(a, b); DLL blobs only).
    Pow,
    /// 35 — dst, src (absolute value; DLL blobs only).
    Abs,
    /// 36 — dst, src (3-component normalize; DLL blobs only).
    Nrm,
    /// 37 — dst, src (sine (x) into x, cosine (x) into y; DLL blobs).
    Sincos,
    /// 40 — if b#  (dynamic branch on a ConstBool register; DLL blobs).
    If,
    /// 42 — else  (no operands; DLL blobs only).
    Else,
    /// 43 — endif  (no operands; DLL blobs only).
    Endif,
    /// 65 — texkill t#  (kill on any component < 0; DLL blobs only).
    Texkill,
    /// 66 — dst, texcoord, sampler.
    Tex,
    /// 81 — dst, 4 raw f32 constants.
    Def,
    /// 88 — dst, a, b, c  (dst = a >= 0 ? b : c, per component).
    Cmp,
    /// 90 — dst, a, b, c  (dot2(a,b) + c.w; DLL blobs only; the c
    /// operand carries a replicate swizzle on every occurrence).
    Dp2add,
}

impl Sm30Op {
    /// Opcode number in the token stream.
    pub fn code(self) -> u16 {
        match self {
            Sm30Op::Mov => 1,
            Sm30Op::Add => 2,
            Sm30Op::Mad => 4,
            Sm30Op::Mul => 5,
            Sm30Op::Rcp => 6,
            Sm30Op::Rsq => 7,
            Sm30Op::Dp3 => 8,
            Sm30Op::Dp4 => 9,
            Sm30Op::Min => 10,
            Sm30Op::Max => 11,
            Sm30Op::Slt => 12,
            Sm30Op::Lrp => 18,
            Sm30Op::Frc => 19,
            Sm30Op::Dcl => 31,
            Sm30Op::Pow => 32,
            Sm30Op::Abs => 35,
            Sm30Op::Nrm => 36,
            Sm30Op::Sincos => 37,
            Sm30Op::If => 40,
            Sm30Op::Else => 42,
            Sm30Op::Endif => 43,
            Sm30Op::Texkill => 65,
            Sm30Op::Tex => 66,
            Sm30Op::Def => 81,
            Sm30Op::Cmp => 88,
            Sm30Op::Dp2add => 90,
        }
    }

    /// Number of operand DWORDs after the instruction token (PROVEN:
    /// identical for every occurrence in every stream — the length
    /// field census over the 26 shipped programs and the 45 DLL blobs).
    pub fn operand_count(self) -> usize {
        match self {
            Sm30Op::Mov | Sm30Op::Dcl => 2,
            Sm30Op::Rcp | Sm30Op::Rsq | Sm30Op::Frc | Sm30Op::Abs | Sm30Op::Nrm
            | Sm30Op::Sincos => 2,
            Sm30Op::Add | Sm30Op::Mul | Sm30Op::Dp3 | Sm30Op::Dp4 | Sm30Op::Tex
            | Sm30Op::Min | Sm30Op::Max | Sm30Op::Slt | Sm30Op::Pow => 3,
            Sm30Op::Mad | Sm30Op::Lrp | Sm30Op::Cmp | Sm30Op::Dp2add => 4,
            Sm30Op::Def => 5,
            Sm30Op::If | Sm30Op::Texkill => 1,
            Sm30Op::Else | Sm30Op::Endif => 0,
        }
    }

    fn from_code(code: u16) -> Option<Self> {
        Some(match code {
            1 => Sm30Op::Mov,
            2 => Sm30Op::Add,
            4 => Sm30Op::Mad,
            5 => Sm30Op::Mul,
            6 => Sm30Op::Rcp,
            7 => Sm30Op::Rsq,
            8 => Sm30Op::Dp3,
            9 => Sm30Op::Dp4,
            10 => Sm30Op::Min,
            11 => Sm30Op::Max,
            12 => Sm30Op::Slt,
            18 => Sm30Op::Lrp,
            19 => Sm30Op::Frc,
            31 => Sm30Op::Dcl,
            32 => Sm30Op::Pow,
            35 => Sm30Op::Abs,
            36 => Sm30Op::Nrm,
            37 => Sm30Op::Sincos,
            40 => Sm30Op::If,
            42 => Sm30Op::Else,
            43 => Sm30Op::Endif,
            65 => Sm30Op::Texkill,
            66 => Sm30Op::Tex,
            81 => Sm30Op::Def,
            88 => Sm30Op::Cmp,
            90 => Sm30Op::Dp2add,
            _ => return None,
        })
    }
}

impl fmt::Display for Sm30Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Sm30Op::Mov => "mov",
            Sm30Op::Add => "add",
            Sm30Op::Mad => "mad",
            Sm30Op::Mul => "mul",
            Sm30Op::Rcp => "rcp",
            Sm30Op::Rsq => "rsq",
            Sm30Op::Dp3 => "dp3",
            Sm30Op::Dp4 => "dp4",
            Sm30Op::Min => "min",
            Sm30Op::Max => "max",
            Sm30Op::Slt => "slt",
            Sm30Op::Lrp => "lrp",
            Sm30Op::Frc => "frc",
            Sm30Op::Dcl => "dcl",
            Sm30Op::Pow => "pow",
            Sm30Op::Abs => "abs",
            Sm30Op::Nrm => "nrm",
            Sm30Op::Sincos => "sincos",
            Sm30Op::If => "if",
            Sm30Op::Else => "else",
            Sm30Op::Endif => "endif",
            Sm30Op::Texkill => "texkill",
            Sm30Op::Tex => "tex",
            Sm30Op::Def => "def",
            Sm30Op::Cmp => "cmp",
            Sm30Op::Dp2add => "dp2add",
        };
        f.write_str(s)
    }
}

/// D3DDECLUSAGE values observed in DCL instructions.  The numeric
/// values match the public D3D9 enum: the shipped shaders use 0/5/10
/// (position, texcoord, color); the DLL blobs add 3 (normal, vertex
/// inputs) and 4 (psize, the pointsprite vertex size output).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Usage {
    Position,
    /// `D3DDECLUSAGE_NORMAL` (DLL blob vertex inputs).
    Normal,
    /// `D3DDECLUSAGE_PSIZE` (DLL blob pointsprite output).
    PSize,
    TexCoord,
    Color,
    /// Any other documented D3DDECLUSAGE value.
    Other(u8),
}

impl Usage {
    fn from_bits(b: u8) -> Self {
        match b {
            0 => Usage::Position,
            3 => Usage::Normal,
            4 => Usage::PSize,
            5 => Usage::TexCoord,
            10 => Usage::Color,
            other => Usage::Other(other),
        }
    }
}

impl fmt::Display for Usage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Usage::Position => f.write_str("position"),
            Usage::Normal => f.write_str("normal"),
            Usage::PSize => f.write_str("psize"),
            Usage::TexCoord => f.write_str("texcoord"),
            Usage::Color => f.write_str("color"),
            Usage::Other(b) => write!(f, "usage{b}"),
        }
    }
}

/// One decoded instruction.  Which variant is produced depends on the
/// opcode; the decoder guarantees the operand-count invariant.
#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    /// `dcl <usage><index> <reg>` — input/output/sampler declaration.
    Dcl {
        /// Raw usage token (kept verbatim; its bits are not fully mapped).
        usage_token: u32,
        /// Decoded usage.
        usage: Usage,
        /// Semantic index (bits 16..23 of the usage token).
        usage_index: u8,
        /// The declared register.
        reg: ParamToken,
    },
    /// `def <const>, f, f, f, f`.
    Def { dst: ParamToken, values: [f32; 4] },
    /// Two-operand instruction (mov, rcp, rsq, frc, abs, nrm, sincos).
    Op2 { op: Sm30Op, dst: ParamToken, a: ParamToken },
    /// Three-operand instruction (add, mul, dp4, tex, min, max, slt, pow).
    Op3 { op: Sm30Op, dst: ParamToken, a: ParamToken, b: ParamToken },
    /// Four-operand instruction (mad, lrp, cmp, dp2add).
    Op4 { op: Sm30Op, dst: ParamToken, a: ParamToken, b: ParamToken, c: ParamToken },
    /// Flow control: `if b#` (one ConstBool operand), bare `else`,
    /// bare `endif`.  DLL blobs only.
    Flow { op: Sm30Op, reg: Option<ParamToken> },
    /// `texkill t#` — the operand is a destination-form register token
    /// (write mask), sampled coords die when any masked component < 0.
    /// DLL blobs only.
    Texkill { reg: ParamToken },
}

impl Instruction {
    /// The instruction's opcode.
    pub fn op(&self) -> Sm30Op {
        match self {
            Instruction::Dcl { .. } => Sm30Op::Dcl,
            Instruction::Def { .. } => Sm30Op::Def,
            Instruction::Op2 { op, .. } => *op,
            Instruction::Op3 { op, .. } => *op,
            Instruction::Op4 { op, .. } => *op,
            Instruction::Flow { op, .. } => *op,
            Instruction::Texkill { .. } => Sm30Op::Texkill,
        }
    }

    /// The destination register, when the instruction writes one.
    pub fn dst(&self) -> Option<&ParamToken> {
        match self {
            Instruction::Dcl { reg, .. } => Some(reg),
            Instruction::Def { dst, .. } => Some(dst),
            Instruction::Op2 { dst, .. } => Some(dst),
            Instruction::Op3 { dst, .. } => Some(dst),
            Instruction::Op4 { dst, .. } => Some(dst),
            Instruction::Flow { .. } => None,
            Instruction::Texkill { reg } => Some(reg),
        }
    }
}

impl fmt::Display for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Instruction::Dcl { usage, usage_index, reg, .. } => {
                write!(f, "dcl_{}{} {}", usage, usage_index, reg)
            }
            Instruction::Def { dst, values } => write!(f, "def {}, {:?}", dst, values),
            Instruction::Op2 { op, dst, a } => write!(f, "{op} {dst}, {a}"),
            Instruction::Op3 { op, dst, a, b } => write!(f, "{op} {dst}, {a}, {b}"),
            Instruction::Op4 { op, dst, a, b, c } => write!(f, "{op} {dst}, {a}, {b}, {c}"),
            Instruction::Flow { op, reg } => match reg {
                Some(r) => write!(f, "{op} {r}"),
                None => write!(f, "{op}"),
            },
            Instruction::Texkill { reg } => write!(f, "texkill {reg}"),
        }
    }
}

// ---------------------------------------------------------------------------
// CTAB constant table
// ---------------------------------------------------------------------------

/// One constant from the CTAB comment.
#[derive(Debug, Clone, PartialEq)]
pub struct CtabConstant {
    pub name: String,
    /// Register set: 2 = FLOAT4, 3 = SAMPLER (observed values).
    pub register_set: u16,
    /// First register index (c# / s#).
    pub register_index: u16,
    /// Register span (4 for a float4x4, 1 otherwise).
    pub register_count: u16,
    /// D3DXPARAMETER_CLASS (0 scalar, 1 vector, 3 matrix — observed).
    pub class: u16,
    /// D3DXPARAMETER_TYPE (3 = FLOAT observed).
    pub ptype: u16,
    pub rows: u16,
    pub columns: u16,
}

/// The parsed "CTAB" comment: compiler-generated constant table.
#[derive(Debug, Clone, PartialEq)]
pub struct Ctab {
    /// Compiler target string, e.g. `"vs_3_0"` / `"ps_3_0"`.
    pub target: String,
    /// Compiler signature, e.g. `"Microsoft (R) HLSL Shader Compiler 9.24.949.2307"`.
    pub creator: String,
    pub constants: Vec<CtabConstant>,
}

// ---------------------------------------------------------------------------
// Post-END parameter records (0x0001100D)
// ---------------------------------------------------------------------------

/// One `0x0001100D` record from the chain that follows the END token.
#[derive(Debug, Clone, PartialEq)]
pub struct ShaderParamRecord {
    /// Parameter name (e.g. `"p3dWorldViewProjectionMatrix"`).
    pub name: String,
    /// Per-record uid (observed: 29 float-ish, 33 sampler, 41 bool-ish, 49 matrix).
    pub uid: u32,
    /// First u32 of the record body (observed: 1 float, 4 float4,
    /// 5 matrix, 6 sampler, 10 bool-ish).
    pub type_id: u32,
    /// Byte 0 of the second u32: the register index (c#/s#).
    pub register_index: u16,
    /// Byte 1 of the second u32: 0 normally, 1 on `p3dLight_IsLitPass`
    /// (semantics UNKNOWN — possibly a bool sub-slot).  Bytes 2..3 are
    /// always 0 in the shipped set ([`Self::register_flag`]).
    pub register_sub: u8,
    /// Bytes 2..3 of the second u32 (always 0 in the shipped set).
    pub register_flag: u16,
    /// Constant magic word 0x00011115 (identical in all observed records).
    pub magic: u32,
    /// The two constant u32 fields (observed: 16, 16 — meaning UNKNOWN).
    pub pad: [u32; 2],
    /// Register count (4 for the matrix, 1 otherwise) — agrees with CTAB.
    pub register_count: u32,
}

/// One comment token's payload from a token stream.
///
/// Shipped programs carry exactly one comment (the CTAB); every DLL
/// blob additionally carries a `DBUG` comment before the CTAB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShaderComment {
    /// The full comment payload: the four fourcc bytes followed by the
    /// comment body (the same slice `parse_ctab` consumes for CTAB).
    pub data: Vec<u8>,
}

impl ShaderComment {
    /// The four fourcc bytes (e.g. `b"CTAB"`, `b"DBUG"`).
    pub fn fourcc(&self) -> &[u8; 4] {
        self.data.first_chunk::<4>().unwrap_or(b"????")
    }

    /// The comment body after the fourcc.
    pub fn body(&self) -> &[u8] {
        self.data.get(4..).unwrap_or(&[])
    }

    /// The printable-ASCII runs (≥ 3 chars) inside the comment body,
    /// in stream order.
    ///
    /// For `DBUG` comments (cgc debug blobs) this yields the Cg source
    /// path (always first, ends in `.cg`), then entry/struct/semantic
    /// names and the compiler signature — stable 45/45 on the DLL
    /// blobs.  The DBUG blob's exact binary layout (offset-pair tables
    /// between the strings) was NOT derived; only the strings are
    /// consumed, never interpreted structurally.
    pub fn strings(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut start = None;
        for (i, &b) in self.body().iter().chain(std::iter::once(&0)).enumerate() {
            let printable = (0x20..0x7F).contains(&b);
            if printable && start.is_none() {
                start = Some(i);
            } else if !printable {
                if let Some(s) = start.take() {
                    if i - s >= 3 {
                        out.push(String::from_utf8_lossy(&self.body()[s..i]).into_owned());
                    }
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Shader
// ---------------------------------------------------------------------------

/// A fully decoded SM 3.0 program: version, CTAB, instructions and the
/// post-END parameter records.
#[derive(Debug, Clone, PartialEq)]
pub struct Sm30Shader {
    /// `false` for vs_3_0, `true` for ps_3_0.
    pub is_fragment: bool,
    /// Raw version token (`0xFFFE0300` / `0xFFFF0300`).
    pub version_token: u32,
    /// The CTAB comment, when present (present in all observed programs).
    pub ctab: Option<Ctab>,
    /// Instructions in stream order (DCLs included).
    pub instructions: Vec<Instruction>,
    /// `def` constants, in stream order (duplicated from
    /// [`Self::instructions`] for convenience).
    pub defs: Vec<(u16, [f32; 4])>,
    /// Post-END `0x0001100D` parameter records.
    pub params: Vec<ShaderParamRecord>,
    /// Every comment token in stream order (the CTAB one included;
    /// DLL blobs also carry a `DBUG` comment — see [`ShaderComment`]).
    pub comments: Vec<ShaderComment>,
}

const VERSION_VS: u32 = 0xFFFE_0300;
const VERSION_PS: u32 = 0xFFFF_0300;
const END_TOKEN: u32 = 0x0000_FFFF;
const PARAM_RECORD_TYPE: u32 = 0x0001_100D;
const PARAM_MAGIC: u32 = 0x0001_1115;

/// Byte cursor with on-demand little-endian u32 reads (the token
/// stream's instruction region is u32-aligned but the post-END P3D
/// parameter records are BYTE-packed, so a flat `&[u32]` will not do).
struct Cur<'a> {
    b: &'a [u8],
    i: usize,
}
impl<'a> Cur<'a> {
    fn u32(&mut self) -> Result<u32, Sm30Error> {
        if self.i + 4 > self.b.len() {
            return Err(Sm30Error::UnexpectedEof);
        }
        let v = u32::from_le_bytes([
            self.b[self.i],
            self.b[self.i + 1],
            self.b[self.i + 2],
            self.b[self.i + 3],
        ]);
        self.i += 4;
        Ok(v)
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], Sm30Error> {
        if self.i + n > self.b.len() {
            return Err(Sm30Error::UnexpectedEof);
        }
        let s = &self.b[self.i..self.i + n];
        self.i += n;
        Ok(s)
    }
    fn remaining(&self) -> usize {
        self.b.len() - self.i
    }
}

impl Sm30Shader {
    /// Decode a token stream (the `d3d9_tokens` slice of a P3D
    /// ShaderProgram chunk — everything from the version token on,
    /// INCLUDING the post-END parameter records).
    ///
    /// For the raw DLL blobs (version token .. END, no P3D records)
    /// use [`Self::parse_program`] or [`scan_blobs`].
    ///
    /// ## Alignment quirk (PROVEN 26/26)
    ///
    /// The instruction region is u32-aligned, but the post-END
    /// `0x0001100D` parameter records are BYTE-packed — their sizes
    /// (e.g. 45, 65, 49) are not multiples of 4 and the next record
    /// starts immediately after the previous one, unaligned.  The
    /// stream is therefore decoded with a byte cursor that reads
    /// little-endian u32s on demand, not as a flat `&[u32]` array.
    pub fn parse(tokens: &[u8], is_fragment: bool) -> Result<Self, Sm30Error> {
        let (version_token, ctab, instructions, defs, comments, consumed) =
            Self::walk_program(tokens, is_fragment)?;

        // Post-END parameter records — BYTE-packed, no alignment.
        let mut cur = Cur { b: tokens, i: consumed };
        let mut params = Vec::new();
        while cur.remaining() > 0 {
            if cur.remaining() < 12 {
                return Err(Sm30Error::BadParamRecord(format!(
                    "record prelude truncated: {} bytes left",
                    cur.remaining()
                )));
            }
            let rec_off = cur.i;
            let rtype = cur.u32()?;
            if rtype != PARAM_RECORD_TYPE {
                return Err(Sm30Error::BadParamRecord(format!(
                    "expected record type 0x{PARAM_RECORD_TYPE:08X}, got 0x{rtype:08X} at byte {rec_off}"
                )));
            }
            let uid = cur.u32()?;
            let size = cur.u32()? as usize;
            if size < 13 || size - 12 > cur.remaining() {
                return Err(Sm30Error::BadParamRecord(format!(
                    "implausible record size {size} at byte {rec_off}"
                )));
            }
            let payload = cur.bytes(size - 12)?;
            params.push(parse_param_record(payload, uid)?);
        }

        Ok(Self {
            is_fragment,
            version_token,
            ctab,
            instructions,
            defs,
            params,
            comments,
        })
    }

    /// Decode a RAW program blob: version token .. END token, exactly —
    /// the form embedded in `prototypeenginef.dll` (see [`scan_blobs`]).
    ///
    /// Identical to [`Self::parse`] minus the post-END P3D parameter
    /// records: any byte after the END token is a `TrailingBytes`
    /// error, so a slice must terminate exactly at END.
    pub fn parse_program(tokens: &[u8], is_fragment: bool) -> Result<Self, Sm30Error> {
        let (version_token, ctab, instructions, defs, comments, consumed) =
            Self::walk_program(tokens, is_fragment)?;
        let trailing = tokens.len() - consumed;
        if trailing != 0 {
            return Err(Sm30Error::TrailingBytes(trailing));
        }
        Ok(Self {
            is_fragment,
            version_token,
            ctab,
            instructions,
            defs,
            params: Vec::new(),
            comments,
        })
    }

    /// The shared instruction walker: version token .. END token.
    /// Returns the decoded parts plus the byte offset just past END.
    fn walk_program(
        tokens: &[u8],
        is_fragment: bool,
    ) -> Result<
        (
            u32,
            Option<Ctab>,
            Vec<Instruction>,
            Vec<(u16, [f32; 4])>,
            Vec<ShaderComment>,
            usize,
        ),
        Sm30Error,
    > {
        let mut cur = Cur { b: tokens, i: 0 };
        if tokens.len() < 4 {
            return Err(Sm30Error::UnexpectedEof);
        }
        let version_token = cur.u32()?;
        let expected_ver = if is_fragment { VERSION_PS } else { VERSION_VS };
        if version_token != expected_ver {
            return Err(Sm30Error::BadVersion(version_token));
        }

        let mut ctab = None;
        let mut instructions = Vec::new();
        let mut defs = Vec::new();
        let mut comments = Vec::new();
        loop {
            let inst_off = cur.i;
            let t = cur.u32()?;
            if t == END_TOKEN {
                return Ok((version_token, ctab, instructions, defs, comments, cur.i));
            }
            if t & 0x8000_0000 == 0 && (t & 0xFFFF) == 0xFFFE {
                // Comment token: HIGH16 = payload length in DWORDs.
                let n = ((t >> 16) & 0xFFFF) as usize;
                let body = cur.bytes(n * 4)?;
                comments.push(ShaderComment { data: body.to_vec() });
                if body.starts_with(b"CTAB") {
                    if ctab.is_some() {
                        return Err(Sm30Error::BadCtab("duplicate CTAB"));
                    }
                    ctab = Some(parse_ctab(body)?);
                }
                continue;
            }
            // NOTE (PROVEN): instruction tokens have bit31 CLEAR — the
            // operand/parameter tokens are the ones carrying bit31.  A
            // bit31-set word in instruction position is a decode error.
            if t & 0x8000_0000 != 0 {
                return Err(Sm30Error::BadInstructionToken { offset: inst_off, token: t });
            }
            let opcode = (t & 0xFFFF) as u16;
            let op = Sm30Op::from_code(opcode)
                .ok_or(Sm30Error::UnknownOpcode { offset: inst_off, opcode })?;
            let got = ((t >> 24) & 0xF) as usize;
            let expected = op.operand_count();
            if got != expected {
                return Err(Sm30Error::BadOperandCount {
                    offset: inst_off,
                    opcode,
                    expected,
                    got,
                });
            }
            let mut operands = [0u32; 5];
            for o in operands.iter_mut().take(expected) {
                *o = cur.u32()?;
            }
            let opnd = |k: usize| operands[k];
            let inst = match op {
                Sm30Op::Dcl => {
                    let ut = opnd(0);
                    let usage_bits = (ut & 0xFF) as u8;
                    // Only the observed D3DDECLUSAGE values are accepted:
                    // 0/5/10 in the shipped shaders, 3 (normal) and 4
                    // (psize) in the DLL blobs (census 45/45).
                    if ![0u8, 3, 4, 5, 10].contains(&usage_bits) {
                        return Err(Sm30Error::UnknownUsage {
                            offset: inst_off + 4,
                            usage: usage_bits,
                        });
                    }
                    let reg = ParamToken::decode_dst(opnd(1), inst_off + 8)?;
                    let ok_ty = match (is_fragment, reg.ty) {
                        (false, RegType::Input) | (false, RegType::Output) => true,
                        (true, RegType::Input) | (true, RegType::Sampler) => true,
                        _ => false,
                    };
                    if !ok_ty {
                        return Err(Sm30Error::BadDclTarget {
                            offset: inst_off + 8,
                            is_fragment,
                            reg_type: (((opnd(1) >> 28) & 0x7) | (((opnd(1) >> 11) & 1) << 3)) as u8,
                        });
                    }
                    Instruction::Dcl {
                        usage_token: ut,
                        usage: Usage::from_bits(usage_bits),
                        usage_index: ((ut >> 16) & 0xFF) as u8,
                        reg,
                    }
                }
                Sm30Op::Def => {
                    let dst = ParamToken::decode_dst(opnd(0), inst_off + 4)?;
                    let mut values = [0f32; 4];
                    for (k, v) in values.iter_mut().enumerate() {
                        *v = f32::from_bits(opnd(k + 1));
                    }
                    defs.push((dst.reg, values));
                    Instruction::Def { dst, values }
                }
                Sm30Op::Mov
                | Sm30Op::Rcp
                | Sm30Op::Rsq
                | Sm30Op::Frc
                | Sm30Op::Abs
                | Sm30Op::Nrm
                | Sm30Op::Sincos => {
                    let dst = ParamToken::decode_dst(opnd(0), inst_off + 4)?;
                    let a = ParamToken::decode(opnd(1), inst_off + 8)?;
                    Instruction::Op2 { op, dst, a }
                }
                Sm30Op::Add | Sm30Op::Mul | Sm30Op::Dp3 | Sm30Op::Dp4 | Sm30Op::Tex
                | Sm30Op::Min | Sm30Op::Max | Sm30Op::Slt | Sm30Op::Pow => {
                    let dst = ParamToken::decode_dst(opnd(0), inst_off + 4)?;
                    let a = ParamToken::decode(opnd(1), inst_off + 8)?;
                    let b = ParamToken::decode(opnd(2), inst_off + 12)?;
                    Instruction::Op3 { op, dst, a, b }
                }
                Sm30Op::Mad | Sm30Op::Lrp | Sm30Op::Cmp | Sm30Op::Dp2add => {
                    let dst = ParamToken::decode_dst(opnd(0), inst_off + 4)?;
                    let a = ParamToken::decode(opnd(1), inst_off + 8)?;
                    let b = ParamToken::decode(opnd(2), inst_off + 12)?;
                    let c = ParamToken::decode(opnd(3), inst_off + 16)?;
                    Instruction::Op4 { op, dst, a, b, c }
                }
                Sm30Op::If => {
                    let reg = ParamToken::decode(opnd(0), inst_off + 4)?;
                    Instruction::Flow { op, reg: Some(reg) }
                }
                Sm30Op::Else | Sm30Op::Endif => Instruction::Flow { op, reg: None },
                Sm30Op::Texkill => {
                    let reg = ParamToken::decode_dst(opnd(0), inst_off + 4)?;
                    Instruction::Texkill { reg }
                }
            };
            instructions.push(inst);
        }
    }

    /// The shader's DCL instructions.
    pub fn dcls(&self) -> impl Iterator<Item = &Instruction> {
        self.instructions.iter().filter(|i| matches!(i, Instruction::Dcl { .. }))
    }

    /// Declared input registers (vertex and fragment `v#`).
    pub fn inputs(&self) -> Vec<(&Usage, u8, &ParamToken)> {
        self.dcls()
            .filter_map(|d| match d {
                Instruction::Dcl { usage, usage_index, reg, .. }
                    if reg.ty == RegType::Input =>
                {
                    Some((usage, *usage_index, reg))
                }
                _ => None,
            })
            .collect()
    }

    /// Declared outputs (vertex `o#`).
    pub fn outputs(&self) -> Vec<(&Usage, u8, &ParamToken)> {
        self.dcls()
            .filter_map(|d| match d {
                Instruction::Dcl { usage, usage_index, reg, .. }
                    if reg.ty == RegType::Output =>
                {
                    Some((usage, *usage_index, reg))
                }
                _ => None,
            })
            .collect()
    }

    /// Declared samplers (fragment `s#`).
    pub fn samplers(&self) -> Vec<&ParamToken> {
        self.dcls()
            .filter_map(|d| match d {
                Instruction::Dcl { reg, .. } if reg.ty == RegType::Sampler => Some(reg),
                _ => None,
            })
            .collect()
    }

    /// PROVEN invariant (observed on all shipped programs): every
    /// parameter record agrees with the CTAB — same name, same register
    /// index, same count — and every CTAB float/sampler constant has a
    /// matching record.
    pub fn cross_check(&self) -> Result<(), String> {
        let ctab = self
            .ctab
            .as_ref()
            .ok_or_else(|| "no CTAB present".to_string())?;
        for cc in &ctab.constants {
            let rec = self
                .params
                .iter()
                .find(|p| p.register_index == cc.register_index)
                .ok_or_else(|| {
                    format!(
                        "CTAB constant {:?} (register {}) has no parameter record",
                        cc.name, cc.register_index
                    )
                })?;
            if rec.register_count != cc.register_count as u32 {
                return Err(format!(
                    "constant {:?}: CTAB count {} != record count {}",
                    cc.name, cc.register_count, rec.register_count
                ));
            }
            if rec.name != cc.name {
                return Err(format!(
                    "register {}: CTAB name {:?} != record name {:?}",
                    cc.register_index, cc.name, rec.name
                ));
            }
        }
        Ok(())
    }

    /// Disassembly listing (one instruction per line), for tests and
    /// diagnostics.
    pub fn disasm(&self) -> String {
        let mut s = String::new();
        for i in &self.instructions {
            s.push_str(&format!("  {i}\n"));
        }
        s
    }
}

// ---------------------------------------------------------------------------
// CTAB parsing
// ---------------------------------------------------------------------------

fn cstr(body: &[u8], off: usize) -> Result<String, Sm30Error> {
    let end = body
        .iter()
        .skip(off)
        .position(|&b| b == 0)
        .ok_or(Sm30Error::BadCtab("unterminated string"))?
        + off;
    Ok(String::from_utf8_lossy(&body[off..end]).into_owned())
}

fn parse_ctab(body: &[u8]) -> Result<Ctab, Sm30Error> {
    // body = "CTAB" fourcc + struct; ALL offsets are relative to the
    // struct start (the byte right after "CTAB") — derived empirically
    // and cross-checked against every program's HLSL register() notes.
    const HDR: usize = 28; // 7 x u32: Size, Creator, Version, Constants,
                           //          ConstantInfo, Flags, Target
    if body.len() < 4 + HDR {
        return Err(Sm30Error::BadCtab("header truncated"));
    }
    let rd = |o: usize| -> u32 {
        u32::from_le_bytes([body[4 + o], body[5 + o], body[6 + o], body[7 + o]])
    };
    let _size = rd(0);
    let creator_off = rd(4) as usize;
    let _version = rd(8);
    let nconst = rd(12) as usize;
    let constinfo = rd(16) as usize;
    let _flags = rd(20);
    let target_off = rd(24) as usize;

    let struct_bytes = &body[4..];
    let creator = cstr(struct_bytes, creator_off)?;
    let target = cstr(struct_bytes, target_off)?;

    let mut constants = Vec::with_capacity(nconst.min(64));
    for k in 0..nconst {
        let e = constinfo + k * 20;
        if e + 20 > struct_bytes.len() {
            return Err(Sm30Error::BadCtab("constant entry out of range"));
        }
        let rd32 = |o: usize| {
            u32::from_le_bytes([
                struct_bytes[o],
                struct_bytes[o + 1],
                struct_bytes[o + 2],
                struct_bytes[o + 3],
            ])
        };
        let rd16 = |o: usize| u16::from_le_bytes([struct_bytes[o], struct_bytes[o + 1]]);
        let name_off = rd32(e) as usize;
        let register_set = rd16(e + 4);
        let register_index = rd16(e + 6);
        let register_count = rd16(e + 8);
        let tinfo = rd32(e + 12) as usize;
        if tinfo + 12 > struct_bytes.len() {
            return Err(Sm30Error::BadCtab("type info out of range"));
        }
        let class = rd16(tinfo);
        let ptype = rd16(tinfo + 2);
        let rows = rd16(tinfo + 4);
        let columns = rd16(tinfo + 6);
        constants.push(CtabConstant {
            name: cstr(struct_bytes, name_off)?,
            register_set,
            register_index,
            register_count,
            class,
            ptype,
            rows,
            columns,
        });
    }
    Ok(Ctab { target, creator, constants })
}

// ---------------------------------------------------------------------------
// Post-END parameter records
// ---------------------------------------------------------------------------

fn parse_param_record(payload: &[u8], uid: u32) -> Result<ShaderParamRecord, Sm30Error> {
    if payload.is_empty() {
        return Err(Sm30Error::BadParamRecord("empty record payload".into()));
    }
    let nlen = payload[0] as usize;
    if nlen == 0 || 1 + nlen + 24 > payload.len() {
        return Err(Sm30Error::BadParamRecord(format!(
            "bad name length {nlen} in record payload of {} bytes",
            payload.len()
        )));
    }
    let name = String::from_utf8_lossy(&payload[1..1 + nlen])
        .trim_end_matches('\0')
        .to_owned();
    let rest = &payload[1 + nlen..];
    let word = |k: usize| {
        u32::from_le_bytes([rest[k * 4], rest[k * 4 + 1], rest[k * 4 + 2], rest[k * 4 + 3]])
    };
    let type_id = word(0);
    // The second u32 is byte-packed: byte0 = register index, byte1 =
    // sub-slot (observed 0/1), bytes 2..3 = flag (observed 0).
    let reg_packed = word(1);
    let magic = word(2);
    let pad = [word(3), word(4)];
    let count = word(5);
    if magic != PARAM_MAGIC {
        return Err(Sm30Error::BadParamRecord(format!(
            "record {:?}: magic 0x{magic:08X} != 0x{PARAM_MAGIC:08X}",
            name
        )));
    }
    Ok(ShaderParamRecord {
        name,
        uid,
        type_id,
        register_index: (reg_packed & 0xFF) as u16,
        register_sub: ((reg_packed >> 8) & 0xFF) as u8,
        register_flag: (reg_packed >> 16) as u16,
        magic,
        pad,
        register_count: count,
    })
}

// ---------------------------------------------------------------------------
// Embedded-blob scanning (prototypeenginef.dll)
// ---------------------------------------------------------------------------

/// One SM 3.0 program blob found by [`scan_blobs`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobRef {
    /// Byte offset of the version token.
    pub offset: usize,
    /// Byte length: version token .. END token inclusive.
    pub len: usize,
    /// `true` for ps_3_0, `false` for vs_3_0.
    pub is_fragment: bool,
}

impl BlobRef {
    /// The blob's bytes — exactly what [`Sm30Shader::parse_program`]
    /// consumes.
    pub fn slice<'a>(&self, bytes: &'a [u8]) -> &'a [u8] {
        &bytes[self.offset..self.offset + self.len]
    }
}

/// Structural-walk limits for [`scan_blobs`] (the largest real blob is
/// 6 848 B / 133 instructions; these caps are ~10× headroom and keep a
/// corrupted candidate from scanning the whole input).
const SCAN_MAX_BYTES: usize = 0x1_0000;
const SCAN_MAX_TOKENS: usize = 8192;

/// Scan raw binary data (e.g. the whole `prototypeenginef.dll`) for
/// embedded SM 3.0 shader blobs.
///
/// A candidate version token (`0xFFFE0300`/`0xFFFF0300`, 4-aligned) is
/// ACCEPTED only when, walking from it:
///
/// (a) every token up to the END token is structurally well-formed —
///     comment tokens carry in-bounds payload lengths, instruction
///     tokens have bit31 clear and a length field that stays in
///     bounds (the length field counts operand DWORDs — PROVEN
///     45/45 on the DLL blobs and 26/26 on the shipped programs;
///     zero-length instructions are legitimate for zero-operand
///     flow-control opcodes),
/// (b) exactly one CTAB comment is present and parses,
/// (c) the CTAB target string matches the version token's stage.
///
/// Candidates starting INSIDE an already-accepted blob are skipped:
/// in the real DLL every one of the 46 rejected version tokens was
/// either data inside an accepted blob's byte range or the single
/// stray hit at 0x9e098c whose second word carried bit31 set (a data
/// coincidence — not a blob).  The accepted 45 tile their cluster
/// region exactly (every inter-blob gap is one zero DWORD), which is
/// the structural evidence that the accept rule finds ALL blobs.
///
/// Returns the accepted blobs in ascending offset order.
pub fn scan_blobs(bytes: &[u8]) -> Vec<BlobRef> {
    let mut accepted: Vec<BlobRef> = Vec::new();
    if bytes.len() < 8 {
        return accepted;
    }
    for off in (0..=bytes.len() - 4).step_by(4) {
        let t0 = le_u32(bytes, off);
        let is_fragment = match t0 {
            VERSION_PS => true,
            VERSION_VS => false,
            _ => continue,
        };
        // Skip candidates inside an already-accepted blob.
        if accepted
            .last()
            .map(|b: &BlobRef| off < b.offset + b.len)
            .unwrap_or(false)
        {
            continue;
        }
        if let Some(end) = walk_blob_structure(bytes, off, is_fragment) {
            accepted.push(BlobRef { offset: off, len: end - off, is_fragment });
        }
    }
    accepted
}

/// Structural walk from a candidate version token; returns the offset
/// just past the END token when the candidate validates as a blob
/// (well-formed walk + single CTAB whose target matches the stage).
fn walk_blob_structure(bytes: &[u8], off: usize, is_fragment: bool) -> Option<usize> {
    let mut cur = off + 4;
    let mut steps = 0usize;
    let mut ctab: Option<&[u8]> = None;
    loop {
        if cur + 4 > bytes.len() || cur - off > SCAN_MAX_BYTES || steps > SCAN_MAX_TOKENS {
            return None;
        }
        steps += 1;
        let t = le_u32(bytes, cur);
        if t == END_TOKEN {
            let end = cur + 4;
            // Validate the CTAB: present, parseable, target matches.
            let ctab = ctab?;
            let parsed = parse_ctab(ctab).ok()?;
            let want = if is_fragment { "ps_3_0" } else { "vs_3_0" };
            if parsed.target != want {
                return None;
            }
            return Some(end);
        }
        if t & 0x8000_0000 == 0 && (t & 0xFFFF) == 0xFFFE {
            let n = ((t >> 16) & 0xFFFF) as usize;
            let body_off = cur + 4;
            if n == 0 || body_off + n * 4 > bytes.len() {
                return None;
            }
            let body = &bytes[body_off..body_off + n * 4];
            if body.starts_with(b"CTAB") {
                if ctab.is_some() {
                    return None;
                }
                ctab = Some(body);
            }
            cur = body_off + n * 4;
            continue;
        }
        if t & 0x8000_0000 == 0 {
            // Instruction token: advance over its operand DWORDs.
            let ln = ((t >> 24) & 0xF) as usize;
            cur += 4 + ln * 4;
            continue;
        }
        // Bit31-set word in instruction position: not a token stream.
        return None;
    }
}

fn le_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

// ---------------------------------------------------------------------------
// Fixture-driven tests
// ---------------------------------------------------------------------------
//
// Every test decodes the REAL token streams shipped with Prototype
// (2009): the 24 ShaderProgram chunks in `frontendshaders.p3d` plus
// the 2 Bink-video programs in `atg\shaders\bink.p3d` (payload of
// `shaders.rcf`).  All assertions were cross-checked by hand against
// the inline HLSL sources and the CTAB constant tables.
//
// NOTE: these tests live INLINE (not in tests/) because the shared
// workspace saw concurrent `tests/` deletions from a parallel work
// block; in-module unit tests are immune to that.

#[cfg(test)]
mod tests {
    use crate::shader::sm30::{scan_blobs, Instruction, RegType, Sm30Error, Sm30Op, Sm30Shader, Usage};
    use prototype_p3d::P3dFile;

    fn fixture(name: &str) -> Vec<u8> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../prototype-bin/tests/fixtures")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    /// (name, is_fragment, tokens) for every program in frontendshaders.p3d.
    fn frontend_programs() -> Vec<(String, bool, Vec<u8>)> {
        let data = fixture("frontendshaders.p3d");
        let f = P3dFile::parse(&data).unwrap();
        f.shader_programs()
            .unwrap()
            .into_iter()
            .map(|p| (p.name.clone(), p.is_fragment, p.d3d9_tokens.to_vec()))
            .collect()
    }

    /// The 2 Bink programs from shaders.rcf (`atg\shaders\bink.p3d`).
    fn bink_programs() -> Vec<(String, bool, Vec<u8>)> {
        let data = fixture("shaders.rcf");
        let arc = prototype_rcf::RcfArchive::parse(&data).unwrap();
        for e in &arc.entries {
            let payload = arc.payload(e).unwrap();
            if payload.starts_with(b"P3D\xff") {
                let f = P3dFile::parse(payload).unwrap();
                let progs = f.shader_programs().unwrap();
                if !progs.is_empty() {
                    return progs
                        .into_iter()
                        .map(|p| (p.name.clone(), p.is_fragment, p.d3d9_tokens.to_vec()))
                        .collect();
                }
            }
        }
        panic!("shaders.rcf contains no P3D payload with ShaderProgram chunks")
    }

    fn all_programs() -> Vec<(String, bool, Vec<u8>)> {
        let mut v = frontend_programs();
        v.extend(bink_programs());
        v
    }

    #[test]
    fn decodes_all_26_shipped_programs() {
        let progs = all_programs();
        assert_eq!(progs.len(), 26, "24 frontend + 2 bink programs expected");
        let mut vs = 0;
        let mut ps = 0;
        for (name, is_fragment, tokens) in &progs {
            let s = Sm30Shader::parse(tokens, *is_fragment)
                .unwrap_or_else(|e| panic!("{name} failed to decode: {e}"));
            assert_eq!(s.is_fragment, *is_fragment);
            assert_eq!(s.version_token, if *is_fragment { 0xFFFF_0300 } else { 0xFFFE_0300 });
            assert!(s.ctab.is_some(), "{name}: CTAB missing");
            assert!(!s.instructions.is_empty());
            let target = s.ctab.as_ref().unwrap().target.clone();
            if *is_fragment {
                assert_eq!(target, "ps_3_0", "{name}");
                ps += 1;
            } else {
                assert_eq!(target, "vs_3_0", "{name}");
                vs += 1;
            }
            assert!(
                s.ctab.as_ref().unwrap().creator.contains("HLSL Shader Compiler"),
                "{name}: unexpected creator"
            );
        }
        assert_eq!(vs, 13, "13 vertex programs (12 frontend + bink)");
        assert_eq!(ps, 13, "13 fragment programs (12 frontend + bink)");
    }

    #[test]
    fn ctab_and_param_records_agree_on_every_program() {
        // PROVEN invariant: post-END 0x0001100D records and the CTAB
        // agree on name, register index and register count.
        for (name, is_fragment, tokens) in all_programs() {
            let s = Sm30Shader::parse(&tokens, is_fragment).unwrap();
            s.cross_check()
                .unwrap_or_else(|e| panic!("{name}: cross-check failed: {e}"));
            assert!(!s.params.is_empty(), "{name}: no parameter records");
        }
    }

    #[test]
    fn instruction_set_is_exactly_the_eleven_observed_opcodes() {
        let mut ops = std::collections::BTreeSet::new();
        let mut total = 0;
        for (_name, is_fragment, tokens) in all_programs() {
            let s = Sm30Shader::parse(&tokens, is_fragment).unwrap();
            for i in &s.instructions {
                ops.insert(i.op());
                total += 1;
            }
        }
        let expected: std::collections::BTreeSet<Sm30Op> = [
            Sm30Op::Mov,
            Sm30Op::Add,
            Sm30Op::Mad,
            Sm30Op::Mul,
            Sm30Op::Dp3,
            Sm30Op::Dp4,
            Sm30Op::Lrp,
            Sm30Op::Dcl,
            Sm30Op::Tex,
            Sm30Op::Def,
            Sm30Op::Cmp,
        ]
        .into_iter()
        .collect();
        assert_eq!(ops, expected, "the shipped set uses exactly these 11 opcodes");
        // Empirically counted across all 26 programs (frontend 235 +
        // bink ps 18 + bink vs 9).
        assert_eq!(total, 262, "total instruction count across all programs");
    }

    #[test]
    fn cxformsolidcolour_vertex_matches_hand_decoded_listing() {
        // Hand-decoded from the real bytes (cross-checked against the
        // HLSL).  The smallest vertex program — the decoder gold test.
        let (_, is_fragment, tokens) = all_programs()
            .into_iter()
            .find(|(n, _, _)| n == "cxformsolidcolour - lit - Vertex Program")
            .unwrap();
        assert!(!is_fragment);
        let s = Sm30Shader::parse(&tokens, false).unwrap();

        // Ground truth (verified against the raw bytes with an
        // independent Python decode): 2 dcls + the fxc m4x4 unroll —
        // mul (c1·v.y), mad (c0·v.x), mad (c2·v.z), then add (+c3, the
        // w-row folded under the w=1 vertex convention).  No def here.
        assert_eq!(s.instructions.len(), 6, "{}", s.disasm());
        assert_eq!(s.defs.len(), 0);

        let listing = s.disasm();
        assert!(listing.contains("mul r0.xyzw, c1.xyzw, v0.yyyy"), "{listing}");
        assert!(listing.contains("mad r0.xyzw, c0.xyzw, v0.xxxx, r0.xyzw"), "{listing}");
        assert!(listing.contains("mad r0.xyzw, c2.xyzw, v0.zzzz, r0.xyzw"), "{listing}");
        assert!(listing.contains("add o0.xyzw, r0.xyzw, c3.xyzw"), "{listing}");

        // DCLs: position in (v0) and position out (o0) only.
        let dcls: Vec<_> = s.dcls().collect();
        assert_eq!(dcls.len(), 2);
        for d in dcls {
            match d {
                Instruction::Dcl { usage, usage_index, reg, .. } => {
                    assert_eq!(*usage, Usage::Position);
                    assert_eq!(*usage_index, 0);
                    assert_eq!(reg.reg, 0);
                }
                _ => panic!("non-dcl in dcls()"),
            }
        }
    }

    #[test]
    fn ctab_constants_match_hlsl_register_annotations() {
        // CxformGauraudMultiplyTexture - lit - Fragment: HLSL declares
        // diffuseMap(s0), cxmul(c2), cxadd(c3), constMix(c6).
        let (_, is_fragment, tokens) = all_programs()
            .into_iter()
            .find(|(n, _, _)| n == "CxformGauraudMultiplyTexture - lit - Fragment Program")
            .unwrap();
        assert!(is_fragment);
        let s = Sm30Shader::parse(&tokens, true).unwrap();
        let ctab = s.ctab.as_ref().unwrap();
        let named = |n: &str| ctab.constants.iter().find(|c| c.name == n).unwrap();
        let diffuse = named("diffuseMap");
        assert_eq!((diffuse.register_set, diffuse.register_index, diffuse.register_count), (3, 0, 1));
        let cxmul = named("cxmul");
        assert_eq!((cxmul.register_set, cxmul.register_index), (2, 2));
        let cxadd = named("cxadd");
        assert_eq!((cxadd.register_set, cxadd.register_index), (2, 3));
        let mix = named("constMix");
        assert_eq!((mix.register_set, mix.register_index), (2, 6));
        assert_eq!((mix.class, mix.ptype, mix.rows, mix.columns), (0, 3, 1, 1));

        assert_eq!(s.samplers().len(), 1);
        assert_eq!(s.samplers()[0].reg, 0);
        assert_eq!(s.samplers()[0].ty, RegType::Sampler);
        let inputs = s.inputs();
        assert!(inputs.iter().any(|(u, i, _)| **u == Usage::TexCoord && *i == 0));
        assert!(inputs.iter().any(|(u, i, _)| **u == Usage::Color && *i == 0));

        for idx in [0u16, 2, 3, 6] {
            assert!(
                s.params.iter().any(|p| p.register_index == idx),
                "param record for register {idx} missing"
            );
        }
    }

    #[test]
    fn bink_programs_decode_and_use_dp3() {
        let progs = bink_programs();
        assert_eq!(progs.len(), 2, "shaders.rcf carries exactly 2 bink programs");
        for (name, is_fragment, tokens) in &progs {
            assert!(name.contains("bink"), "{name}");
            let s = Sm30Shader::parse(tokens, *is_fragment)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            s.cross_check().unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        // PROVEN: the bink fragment program is the only DP3 user.
        let (_, is_fragment, tokens) = progs.into_iter().find(|(_, f, _)| *f).unwrap();
        assert!(is_fragment);
        let s = Sm30Shader::parse(&tokens, true).unwrap();
        assert_eq!(
            s.instructions.iter().filter(|i| i.op() == Sm30Op::Dp3).count(),
            2,
            "bink fragment program has exactly 2 dp3"
        );
        assert!(s.disasm().contains("dp3"));
    }

    #[test]
    fn rejected_inputs_produce_precise_errors() {
        // Bad version token.
        let mut bad = vec![0u8; 32];
        bad[..4].copy_from_slice(&0xFFFF_0201u32.to_le_bytes());
        assert_eq!(
            Sm30Shader::parse(&bad, false),
            Err(Sm30Error::BadVersion(0xFFFF_0201))
        );

        // Stage mismatch: a vertex stream parsed as fragment.
        let (_, _, tokens) = frontend_programs()
            .into_iter()
            .find(|(n, _, _)| n == "cxformsolidcolour - lit - Vertex Program")
            .unwrap();
        assert!(matches!(
            Sm30Shader::parse(&tokens, true),
            Err(Sm30Error::BadVersion(_))
        ));

        // Too-short stream.
        assert_eq!(Sm30Shader::parse(&[0; 3], false), Err(Sm30Error::UnexpectedEof));
        assert!(matches!(
            Sm30Shader::parse(&[0; 7], false),
            Err(Sm30Error::BadVersion(0))
        ));

        // Truncated stream: version + garbage instruction token (bit31
        // CLEAR, low16 = opcode 200, which is not in the shipped set).
        let mut bad = Vec::new();
        bad.extend_from_slice(&0xFFFE_0300u32.to_le_bytes());
        bad.extend_from_slice(&0x0000_00C8u32.to_le_bytes());
        match Sm30Shader::parse(&bad, false) {
            Err(Sm30Error::UnknownOpcode { opcode, .. }) => assert_eq!(opcode, 200),
            other => panic!("expected UnknownOpcode, got {other:?}"),
        }

        // A parameter token in instruction position (bit31 set).
        let mut bad = Vec::new();
        bad.extend_from_slice(&0xFFFE_0300u32.to_le_bytes());
        bad.extend_from_slice(&0x8000_0000u32.to_le_bytes());
        assert_eq!(
            Sm30Shader::parse(&bad, false),
            Err(Sm30Error::BadInstructionToken { offset: 4, token: 0x8000_0000 })
        );
    }

    #[test]
    fn fragment_programs_use_tex_and_samplers() {
        for (name, is_fragment, tokens) in all_programs() {
            let s = Sm30Shader::parse(&tokens, is_fragment).unwrap();
            let has_tex = s.instructions.iter().any(|i| i.op() == Sm30Op::Tex);
            if is_fragment {
                if has_tex {
                    // Every sampler used by a TEX must be declared; the
                    // frontend programs all use exactly s0, bink uses 3.
                    let declared: Vec<u16> = s.samplers().iter().map(|t| t.reg).collect();
                    assert!(!declared.is_empty(), "{name}: tex without sampler dcl");
                    for i in &s.instructions {
                        if let Instruction::Op3 { op: Sm30Op::Tex, b, .. } = i {
                            assert!(
                                declared.contains(&b.reg),
                                "{name}: tex uses undeclared sampler s{}",
                                b.reg
                            );
                        }
                    }
                }
            } else {
                assert!(!has_tex, "{name}: vertex program contains TEX");
                let outs = s.outputs();
                assert!(
                    outs.iter().any(|(u, _, _)| **u == Usage::Position),
                    "{name}: no position output"
                );
            }
        }
    }

    #[test]
    fn every_vertex_program_transforms_position_through_the_wvp_matrix() {
        // PROVEN pattern: all 13 vertex programs unroll mul(mvp, pos)
        // as `mul r?, c1, v0.yyyy` + mads for rows 0 and 2, then either
        // `mad ..., c3.xyzw, v0.wwww, ...` or `add ..., c3.xyzw` (the
        // w-row folded under the w=1 vertex convention).
        for (name, is_fragment, tokens) in all_programs() {
            if is_fragment {
                continue;
            }
            let s = Sm30Shader::parse(&tokens, false).unwrap();
            let listing = s.disasm();
            assert!(listing.contains("c1.xyzw, v0.yyyy"), "{name}:\n{listing}");
            assert!(listing.contains("c0.xyzw, v0.xxxx"), "{name}:\n{listing}");
            assert!(listing.contains("c2.xyzw, v0.zzzz"), "{name}:\n{listing}");
            assert!(
                listing.contains("c3.xyzw, v0.wwww") || listing.contains("add o0.xyzw, r0.xyzw, c3.xyzw"),
                "{name}: no c3 row term:\n{listing}"
            );
            let ctab = s.ctab.as_ref().unwrap();
            let wvp = ctab
                .constants
                .iter()
                .find(|c| c.register_index == 0 && c.register_count == 4)
                .unwrap_or_else(|| panic!("{name}: no float4x4 at c0"));
            assert_eq!(wvp.name, "p3dWorldViewProjectionMatrix", "{name}");
            assert_eq!((wvp.rows, wvp.columns), (4, 4), "{name}");
        }
    }

    // -------------------------------------------------------------------
    // DLL blob extraction (block 30): the 45 SM 3.0 programs embedded in
    // prototypeenginef.dll.  Six real blobs are committed as fixtures
    // (extracted byte-exactly from the DLL at the offsets in their
    // names); every assertion below was cross-checked against the
    // block-30 python census over all 45 blobs.
    // -------------------------------------------------------------------

    /// (fixture name, is_fragment) for the six committed DLL blobs.
    fn dll_blob_fixtures() -> Vec<(&'static str, bool)> {
        vec![
            ("dll_error_vp_e36010.sm30", false),
            ("dll_lightcount_vp_e37238.sm30", false),
            ("dll_default_vp_e34e60.sm30", false),
            ("dll_unlit_fp_e41670.sm30", true),
            ("dll_bink_fp_e3adf8.sm30", true),
            ("dll_pointsprite_fp_e41cf0.sm30", true),
            ("dll_default_fp_e3b308.sm30", true),
            ("dll_shadowdebug_fp_e405f0.sm30", true),
            ("dll_shadowblur_fp_e3f858.sm30", true),
        ]
    }

    #[test]
    fn dll_blob_fixtures_parse_via_parse_program() {
        for (name, is_fragment) in dll_blob_fixtures() {
            let bytes = fixture(name);
            let s = Sm30Shader::parse_program(&bytes, is_fragment)
                .unwrap_or_else(|e| panic!("{name} failed to decode: {e}"));
            assert_eq!(s.version_token, if is_fragment { 0xFFFF_0300 } else { 0xFFFE_0300 }, "{name}");
            let target = s.ctab.as_ref().unwrap_or_else(|| panic!("{name}: no CTAB")).target.clone();
            assert_eq!(target, if is_fragment { "ps_3_0" } else { "vs_3_0" }, "{name}");
            assert!(s.params.is_empty(), "{name}: raw blobs carry no P3D records");
            // Every blob carries exactly one DBUG comment before the
            // CTAB (45/45 in the census); its first string is the Cg
            // source path.
            assert_eq!(s.comments.len(), 2, "{name}: expected [DBUG, CTAB]");
            assert_eq!(s.comments[0].fourcc(), b"DBUG", "{name}");
            assert_eq!(s.comments[1].fourcc(), b"CTAB", "{name}");
            let strings = s.comments[0].strings();
            assert!(
                strings.first().map(|p| p.ends_with(".cg")).unwrap_or(false),
                "{name}: DBUG first string is not a .cg path: {strings:?}"
            );
            assert!(
                strings.iter().any(|x| x.contains("HLSL Shader Compiler")),
                "{name}: compiler signature missing"
            );
        }
    }

    #[test]
    fn dll_error_vp_is_the_minimal_wvp_transform() {
        let bytes = fixture("dll_error_vp_e36010.sm30");
        let s = Sm30Shader::parse_program(&bytes, false).unwrap();
        // Census: 6 instructions, one constant.
        assert_eq!(s.instructions.len(), 6);
        let ctab = s.ctab.as_ref().unwrap();
        assert_eq!(ctab.constants.len(), 1);
        assert_eq!(ctab.constants[0].name, "p3dWorldViewProjectionMatrix");
        assert_eq!(ctab.constants[0].register_count, 4);
        // The same hand-proven WVP unroll pattern as the shipped
        // vertex programs (mul row1, mad row0, mad row2, add row3).
        let listing = s.disasm();
        assert!(listing.contains("mul r0.xyzw, c1.xyzw, v0.yyyy"), "\n{listing}");
        assert!(listing.contains("mad r0.xyzw, c0.xyzw, v0.xxxx, r0.xyzw"), "\n{listing}");
        assert!(listing.contains("mad r0.xyzw, c2.xyzw, v0.zzzz, r0.xyzw"), "\n{listing}");
        assert!(listing.contains("add o0.xyzw, r0.xyzw, c3.xyzw"), "\n{listing}");
    }

    #[test]
    fn dll_bink_fp_carries_the_four_bink_samplers() {
        let bytes = fixture("dll_bink_fp_e3adf8.sm30");
        let s = Sm30Shader::parse_program(&bytes, true).unwrap();
        let ctab = s.ctab.as_ref().unwrap();
        // Census: y_tex/cb_tex/cr_tex/a_tex at s0..s3 (register_set 3).
        let mut samplers: Vec<(String, u16)> = ctab
            .constants
            .iter()
            .filter(|c| c.register_set == 3)
            .map(|c| (c.name.clone(), c.register_index))
            .collect();
        samplers.sort_by_key(|(_, r)| *r);
        assert_eq!(
            samplers,
            vec![
                ("y_tex".to_string(), 0),
                ("cr_tex".to_string(), 1),
                ("cb_tex".to_string(), 2),
                ("a_tex".to_string(), 3),
            ]
        );
        // 20 instructions, texture samples present.
        assert_eq!(s.instructions.len(), 20);
        assert!(s.instructions.iter().any(|i| i.op() == Sm30Op::Tex));
    }

    #[test]
    fn dll_unlit_fp_declares_diffusemap_at_s0() {
        let bytes = fixture("dll_unlit_fp_e41670.sm30");
        let s = Sm30Shader::parse_program(&bytes, true).unwrap();
        let ctab = s.ctab.as_ref().unwrap();
        assert_eq!(ctab.constants.len(), 1);
        assert_eq!(ctab.constants[0].name, "diffuseMap");
        assert_eq!(ctab.constants[0].register_index, 0);
        assert_eq!(ctab.constants[0].register_set, 3, "sampler register set");
        assert_eq!(s.instructions.len(), 5);
    }

    #[test]
    fn dll_lightcount_vp_uses_max_slt_nrm_and_the_predicate_constant() {
        let bytes = fixture("dll_lightcount_vp_e37238.sm30");
        let s = Sm30Shader::parse_program(&bytes, false).unwrap();
        let names: Vec<&str> = s.ctab.as_ref().unwrap().constants.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "lightCountColor",
                "p3dWorldViewMatrix",
                "p3dWorldViewMatrixInverseTranspose",
                "p3dWorldViewProjectionMatrix",
                "predicate",
            ]
        );
        // 26 instructions per the census; uses max/slt (light-count
        // comparisons) and nrm.
        assert_eq!(s.instructions.len(), 26);
        let ops: std::collections::BTreeSet<Sm30Op> = s.instructions.iter().map(|i| i.op()).collect();
        assert!(ops.contains(&Sm30Op::Max), "{ops:?}");
        assert!(ops.contains(&Sm30Op::Slt), "{ops:?}");
        assert!(ops.contains(&Sm30Op::Nrm), "{ops:?}");
    }

    #[test]
    fn dll_pointsprite_fp_decodes_dp2add_sincos_and_min() {
        let bytes = fixture("dll_pointsprite_fp_e41cf0.sm30");
        let s = Sm30Shader::parse_program(&bytes, true).unwrap();
        let names: Vec<&str> = s.ctab.as_ref().unwrap().constants.iter().map(|c| c.name.as_str()).collect();
        // Census: pointsprite_fp's only constant is the sprite texture.
        // (The p3dParticle_debugParam* constants live in the separate
        // particledebug_fp blob at 0xe3f2d0, not committed.)
        assert_eq!(names, vec!["diffuseMap"]);
        assert_eq!(s.instructions.len(), 24);
        // DP2ADD: dst, src, src, replicated-scalar signature (pinned on
        // the real instruction at 0xe42164: r0.yxxx, r0.xyzw, r0.zwzw,
        // c1.zzzz).
        let dp2: Vec<&Instruction> = s.instructions.iter().filter(|i| i.op() == Sm30Op::Dp2add).collect();
        assert!(!dp2.is_empty(), "pointsprite_fp must contain dp2add");
        for i in dp2 {
            if let Instruction::Op4 { op: Sm30Op::Dp2add, dst: _, a: _, b: _, c } = i {
                // 4th operand is a replicated single-component swizzle.
                let s0 = c.swizzle[0];
                assert!(
                    c.swizzle.iter().all(|&s| s == s0),
                    "dp2add 4th operand not replicated: {i}"
                );
            }
        }
        assert!(s.instructions.iter().any(|i| i.op() == Sm30Op::Sincos));
        assert!(s.instructions.iter().any(|i| i.op() == Sm30Op::Min));
    }

    #[test]
    fn dll_default_fp_is_the_full_world_fragment_shader_with_flow_control() {
        let bytes = fixture("dll_default_fp_e3b308.sm30");
        let s = Sm30Shader::parse_program(&bytes, true).unwrap();
        // Census: 133 instructions, 30 constants.
        assert_eq!(s.instructions.len(), 133);
        let ctab = s.ctab.as_ref().unwrap();
        assert_eq!(ctab.constants.len(), 30);
        let names: std::collections::BTreeSet<&str> = ctab.constants.iter().map(|c| c.name.as_str()).collect();
        for want in [
            "diffuseMap",
            "normalMap",
            "specularMap",
            "p3dLight_Ambient",
            "p3dLight_Colour",
            "p3dLight_LightType",
            "p3dLight_SpotlightConstant",
            "p3dShadowMapInfo",
            "p3dShadowMapChildInfo00",
            "p3dLight_IsLitPass",
        ] {
            assert!(names.contains(want), "missing constant {want}");
        }
        // Flow control: 3 if / 3 else / 3 endif, the `if` operands are
        // ConstBool registers 0..2 (PROVEN — this is how the world
        // shaders branch on light type / shadow enable).
        let mut ifs = 0;
        let mut elses = 0;
        let mut endifs = 0;
        for i in &s.instructions {
            match i {
                Instruction::Flow { op: Sm30Op::If, reg: Some(r) } => {
                    ifs += 1;
                    assert_eq!(r.ty, RegType::ConstBool, "if operand must be b#: {i}");
                    assert!(r.reg <= 2, "if operand register out of observed set: {i}");
                }
                Instruction::Flow { op: Sm30Op::Else, reg: None } => elses += 1,
                Instruction::Flow { op: Sm30Op::Endif, reg: None } => endifs += 1,
                _ => {}
            }
        }
        assert_eq!((ifs, elses, endifs), (3, 3, 3));
        // texkill present (alpha-test style clip).
        assert!(s.instructions.iter().any(|i| matches!(i, Instruction::Texkill { .. })));
    }

    #[test]
    fn dll_dw_and_abs_source_modifiers_decode_from_real_blobs() {
        use crate::shader::sm30::SrcMod;
        // default_fp's shadow branch: `mul r7.xyzw, _dw(r7).xyzw, r1.zzzz`
        // and `max r8.xyzw, _dw(r6).xyzw, r7.xyzw` — the projective
        // divide (public D3DSHADER_SRCMOD_DW = 11; the pre-block-31
        // enum misread these as Abs/AbsNeg).
        let s = Sm30Shader::parse_program(&fixture("dll_default_fp_e3b308.sm30"), true).unwrap();
        let mut dw = 0;
        for i in &s.instructions {
            if let Instruction::Op3 { a, .. } = i {
                if a.src_mod == SrcMod::Dw {
                    dw += 1;
                    assert_eq!(a.ty, RegType::Temp);
                }
            }
        }
        assert_eq!(dw, 2, "default_fp carries exactly two _dw sources");
        // shadowdebug_fp: two more _dw sources in its shadow branch.
        let s2 = Sm30Shader::parse_program(&fixture("dll_shadowdebug_fp_e405f0.sm30"), true).unwrap();
        let mut dw2 = 0;
        for i in &s2.instructions {
            if let Instruction::Op3 { a, .. } = i {
                if a.src_mod == SrcMod::Dw {
                    dw2 += 1;
                }
            }
        }
        assert_eq!(dw2, 2);
        // shadowblur_fp: the two `_abs` depth-difference adds
        // (public ps_3_0 `_abs` extension = 12).
        let s3 = Sm30Shader::parse_program(&fixture("dll_shadowblur_fp_e3f858.sm30"), true).unwrap();
        let mut abs = 0;
        for i in &s3.instructions {
            if let Instruction::Op3 { a, .. } = i {
                if a.src_mod == SrcMod::Abs {
                    abs += 1;
                    assert_eq!(a.ty, RegType::Temp);
                }
            }
        }
        assert_eq!(abs, 2);
        // The shipped 26 programs stay within None/Neg (the realignment
        // must not have disturbed them).
        for (name, is_frag, tokens) in all_programs() {
            let s = Sm30Shader::parse(&tokens, is_frag).unwrap();
            for i in &s.instructions {
                let check = |t: &crate::shader::sm30::ParamToken| {
                    assert!(
                        matches!(t.src_mod, SrcMod::None | SrcMod::Neg),
                        "{name}: shipped program uses {t} ({:?})", t.src_mod
                    );
                };
                match i {
                    Instruction::Op2 { a, .. } => check(a),
                    Instruction::Op3 { a, b, .. } => {
                        check(a);
                        check(b);
                    }
                    Instruction::Op4 { a, b, c, .. } => {
                        check(a);
                        check(b);
                        check(c);
                    }
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn src_modifier_codes_outside_the_public_enum_fail_closed() {
        // version + `mov r1.xyzw, <modifier-14 source>` + END — modifier
        // codes 14/15 are outside the public D3DSHADER_SRCMOD set and
        // must be a decode error, never a guessed variant.
        let tokens: Vec<u32> = vec![
            0xFFFF_0300,
            0x0200_0001, // mov, 2 operands (dst + src)
            0x800F_0001,
            0x8E0F_0001,
            0x0000_FFFF,
        ];
        let mut bytes = Vec::new();
        for t in tokens {
            bytes.extend_from_slice(&t.to_le_bytes());
        }
        match Sm30Shader::parse(&bytes, true) {
            Err(Sm30Error::UnknownSrcMod { bits: 14, .. }) => {}
            other => panic!("expected UnknownSrcMod(14), got {other:?}"),
        }
    }

    #[test]
    fn dll_scan_finds_exactly_the_blobs_in_a_synthetic_container() {
        // Concatenate the committed real blobs with the DLL's zero-DWORD
        // separators plus pseudo-random garbage before/between/after;
        // the scan must find exactly those blobs, at the right offsets,
        // with the right stages.
        let mut rng: u32 = 0x1234_5678;
        let mut garbage = |n: usize| -> Vec<u8> {
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
                v.extend_from_slice(&rng.to_le_bytes());
            }
            v
        };
        let mut container = garbage(64);
        let mut expected: Vec<(usize, bool)> = Vec::new();
        let picks = dll_blob_fixtures();
        for (k, (name, is_fragment)) in picks.iter().enumerate() {
            let blob = fixture(name);
            expected.push((container.len(), *is_fragment));
            container.extend_from_slice(&blob);
            if k + 1 < picks.len() {
                container.extend_from_slice(&[0, 0, 0, 0]); // the DLL separator
            }
            if k == 2 {
                container.extend_from_slice(&garbage(48)); // junk between blobs
            }
        }
        container.extend_from_slice(&garbage(96));

        let found = scan_blobs(&container);
        assert_eq!(found.len(), picks.len(), "scan must find exactly the committed blobs");
        for (b, (off, is_fragment)) in found.iter().zip(&expected) {
            assert_eq!(b.offset, *off);
            assert_eq!(b.is_fragment, *is_fragment);
            // Every scanned blob must also FULLY decode.
            let s = Sm30Shader::parse_program(b.slice(&container), b.is_fragment)
                .unwrap_or_else(|e| panic!("blob at {:#x} does not decode: {e}", b.offset));
            assert!(s.ctab.is_some());
            assert!(!s.instructions.is_empty());
        }
    }

    #[test]
    fn dll_scan_rejects_garbage_and_forged_version_tokens() {
        // Pure pseudo-random bytes: no blob may be reported.
        let mut rng: u32 = 0xDEAD_BEEF;
        let mut buf = Vec::new();
        for _ in 0..4096 {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            buf.extend_from_slice(&rng.to_le_bytes());
        }
        assert!(scan_blobs(&buf).is_empty());

        // A forged version token followed by non-token data must not
        // validate (the second word carries bit31 set).
        let mut forged = buf.clone();
        forged[100..104].copy_from_slice(&0xFFFE_0300u32.to_le_bytes());
        assert!(scan_blobs(&forged).is_empty());

        // A forged ps token on a vs blob's bytes: the walk succeeds
        // structurally but the CTAB target (vs_3_0) contradicts the
        // stage — must be rejected.
        let vs = fixture("dll_error_vp_e36010.sm30");
        let mut wrong_stage = vs.clone();
        wrong_stage[0..4].copy_from_slice(&0xFFFF_0300u32.to_le_bytes());
        assert!(scan_blobs(&wrong_stage).is_empty());
    }

    #[test]
    fn parse_program_reproduces_parse_on_the_shipped_26() {
        // Refactor safety: walk_program is shared, so parse and
        // parse_program must agree on everything before END.  A P3D
        // token slice continues past END with the parameter records, so
        // parse_program reports exactly those bytes as TrailingBytes —
        // slicing them off must yield a program identical to parse's
        // prefix fields (and parse keeps the records).
        for (name, is_fragment, tokens) in all_programs() {
            let full = Sm30Shader::parse(&tokens, is_fragment)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let trailing = match Sm30Shader::parse_program(&tokens, is_fragment) {
                Err(Sm30Error::TrailingBytes(n)) => n,
                other => panic!("{name}: expected TrailingBytes, got {:?}",
                    other.map(|_| ()).map_err(|e| e.to_string())),
            };
            assert!(trailing > 0, "{name}: P3D programs carry records");
            let bare = Sm30Shader::parse_program(&tokens[..tokens.len() - trailing], is_fragment)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(full.version_token, bare.version_token, "{name}");
            assert_eq!(full.ctab, bare.ctab, "{name}");
            assert_eq!(full.instructions, bare.instructions, "{name}");
            assert_eq!(full.comments, bare.comments, "{name}");
            assert!(!full.params.is_empty(), "{name}: parse must keep records");
            assert!(bare.params.is_empty(), "{name}: parse_program has no records");
        }
    }

    #[test]
    fn parse_program_fails_closed_on_trailing_bytes() {
        let mut bytes = fixture("dll_error_vp_e36010.sm30");
        bytes.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
        match Sm30Shader::parse_program(&bytes, false) {
            Err(Sm30Error::TrailingBytes(4)) => {}
            other => panic!("expected TrailingBytes(4), got {other:?}"),
        }
        // One stray byte is equally rejected.
        let mut bytes1 = fixture("dll_error_vp_e36010.sm30");
        bytes1.push(0x00);
        assert!(matches!(
            Sm30Shader::parse_program(&bytes1, false),
            Err(Sm30Error::TrailingBytes(1))
        ));
    }

    #[test]
    fn every_new_dll_opcode_decodes_through_its_variant() {
        // Minimal synthetic streams (one instruction + END) pinning the
        // opcode->variant wiring for the DLL-only opcodes.  Operand
        // tokens follow the PROVEN token layout: dst r0.xyzw =
        // 0x800F0000 (bit31 | temp | writemask), src v0.xyzw =
        // 0x90E40000 (bit31 | input | identity swizzle), b0 =
        // 0xE0000800 (bit31 | CONSTBOOL type 14).
        const DST: u32 = 0x800F_0000;
        const SRC: u32 = 0x90E4_0000;
        const B0: u32 = 0xE000_0800;
        const END: u32 = 0x0000_FFFF;
        let ps: u32 = 0xFFFF_0300;
        let build = |ins: u32, ops: &[u32]| -> Vec<u8> {
            let mut v = vec![ps.to_le_bytes().to_vec(), ins.to_le_bytes().to_vec()].concat();
            for o in ops {
                v.extend_from_slice(&o.to_le_bytes());
            }
            v.extend_from_slice(&END.to_le_bytes());
            v
        };
        let op2 = |len: u32, code: u16| (len << 24) | code as u32;
        let op3 = |len: u32, code: u16| (len << 24) | code as u32;
        let op4 = |len: u32, code: u16| (len << 24) | code as u32;

        // (label, tokens, matcher)
        macro_rules! assert_decodes {
            ($tokens:expr, $is_frag:expr, $pat:pat) => {
                let s = Sm30Shader::parse_program(&$tokens, $is_frag)
                    .unwrap_or_else(|e| panic!("stream failed: {e}"));
                assert_eq!(s.instructions.len(), 1);
                assert!(
                    matches!(&s.instructions[0], $pat),
                    "got {:?}",
                    s.instructions[0]
                );
            };
        }
        let two = [DST, SRC];
        let three = [DST, SRC, SRC];
        let four = [DST, SRC, SRC, SRC];
        assert_decodes!(build(op2(2, 1), &two), true, Instruction::Op2 { op: Sm30Op::Mov, .. });
        assert_decodes!(build(op2(2, 6), &two), true, Instruction::Op2 { op: Sm30Op::Rcp, .. });
        assert_decodes!(build(op2(2, 7), &two), true, Instruction::Op2 { op: Sm30Op::Rsq, .. });
        assert_decodes!(build(op2(2, 19), &two), true, Instruction::Op2 { op: Sm30Op::Frc, .. });
        assert_decodes!(build(op2(2, 35), &two), true, Instruction::Op2 { op: Sm30Op::Abs, .. });
        assert_decodes!(build(op2(2, 36), &two), true, Instruction::Op2 { op: Sm30Op::Nrm, .. });
        assert_decodes!(build(op2(2, 37), &two), true, Instruction::Op2 { op: Sm30Op::Sincos, .. });
        assert_decodes!(build(op3(3, 10), &three), true, Instruction::Op3 { op: Sm30Op::Min, .. });
        assert_decodes!(build(op3(3, 11), &three), true, Instruction::Op3 { op: Sm30Op::Max, .. });
        assert_decodes!(build(op3(3, 12), &three), true, Instruction::Op3 { op: Sm30Op::Slt, .. });
        assert_decodes!(build(op3(3, 32), &three), true, Instruction::Op3 { op: Sm30Op::Pow, .. });
        assert_decodes!(build(op4(4, 90), &four), true, Instruction::Op4 { op: Sm30Op::Dp2add, .. });
        assert_decodes!(build(op2(1, 40), &[B0]), true, Instruction::Flow { op: Sm30Op::If, reg: Some(_) });
        assert_decodes!(build(0x0000_002A, &[]), true, Instruction::Flow { op: Sm30Op::Else, reg: None });
        assert_decodes!(build(0x0000_002B, &[]), true, Instruction::Flow { op: Sm30Op::Endif, reg: None });
        assert_decodes!(build(op2(1, 65), &[DST]), true, Instruction::Texkill { .. });

        // The `if` operand decodes as the ConstBool register b0.
        let s = Sm30Shader::parse_program(&build(op2(1, 40), &[B0]), true).unwrap();
        if let Instruction::Flow { reg: Some(r), .. } = &s.instructions[0] {
            assert_eq!(r.ty, RegType::ConstBool);
            assert_eq!(r.reg, 0);
        } else {
            panic!("expected Flow/If");
        }

        // Fail-closed: an unobserved opcode (e.g. 13 SGE, never seen in
        // any Prototype stream) stays a decode error.
        let sge = build(op3(3, 13), &three);
        assert!(matches!(
            Sm30Shader::parse_program(&sge, true),
            Err(Sm30Error::UnknownOpcode { opcode: 13, .. })
        ));
    }

    #[test]
    fn dll_blob_opcode_census_matches_the_block_30_corpus() {
        // Across the six committed fixtures the new opcodes must appear
        // (this pins the fixture set against silent re-extraction
        // drift); counts match the python census per blob.
        let counts: Vec<(&str, usize, usize)> = dll_blob_fixtures()
            .into_iter()
            .map(|(name, is_frag)| {
                let bytes = fixture(name);
                let s = Sm30Shader::parse_program(&bytes, is_frag).unwrap();
                (name, s.instructions.len(), s.comments.len())
            })
            .collect();
        for (name, n_ins, n_comments) in counts {
            assert_eq!(n_comments, 2, "{name}");
            let expected = match name {
                "dll_error_vp_e36010.sm30" => 6,
                "dll_lightcount_vp_e37238.sm30" => 26,
                "dll_default_vp_e34e60.sm30" => 57,
                "dll_unlit_fp_e41670.sm30" => 5,
                "dll_bink_fp_e3adf8.sm30" => 20,
                "dll_pointsprite_fp_e41cf0.sm30" => 24,
                "dll_default_fp_e3b308.sm30" => 133,
                "dll_shadowdebug_fp_e405f0.sm30" => 63,
                "dll_shadowblur_fp_e3f858.sm30" => 83,
                other => panic!("unknown fixture {other}"),
            };
            assert_eq!(n_ins, expected, "{name}");
        }
    }
}

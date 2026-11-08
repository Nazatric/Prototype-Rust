//! Bink VIDEO symbol layer — the codec's first real decoder layer.
//!
//! Walks one frame's video bitstream exactly the way FFmpeg's
//! `libavcodec/bink.c` `bink_decode_plane` does (bit consumption AND
//! values), capturing per-block records a later pixel layer can
//! render from.  Every semantic below was verified against the real
//! decoder source and — decisively — against REAL frames:
//!
//! ## Evidence (block 50)
//!
//! The walk was proven BIT-EXACT on eight real frames (four committed
//! fixtures + four more extracted locally): the 1280x544 intro's
//! all-black frame 0 (312 B video) and frame 1 (6 172 B), and the
//! 1280x582 silent movie's frames 0..5 + the 260 B all-skip frame 47.
//! On every one of them the three plane walks consume the video
//! region EXACTLY — after plane 2's tail align the reader sits at
//! `video_len * 8` bits, zero bits left — and the column arithmetic
//! closes (`pops + scaled_pops == bw*bh` per plane).  The two bugs
//! that hid this for a block are documented at the bottom.
//!
//! ## Semantics (all FFmpeg-verbatim, `bink.c` + `vlc.c` + `get_bits.h`)
//!
//! * Bit reader: LSB-first; the first bit read is the LSB of the
//!   value (`BITSTREAM_READER_LE`).  This port fails closed on
//!   overread instead of FFmpeg's zero-padding tolerance — a valid
//!   stream never overreads (proven: exact consumption on 8 frames).
//! * VLC: `vlc_init(..., VLC_INIT_USE_STATIC | VLC_INIT_LE)` builds a
//!   single-level table indexed by the code value itself; the first
//!   stream bit is the code's LSB.  `GET_HUFF` returns
//!   `tree.syms[vlc_index]`.  The 16 static tables are prefix-free.
//! * Trees (`read_tree`): `vlc_num` (4 bits); 0 = identity, else
//!   either an explicit symbol list (`bits(1)=1`, len 3 bits + len+1
//!   nibbles, remaining slots filled in ascending order) or a merge
//!   permutation (`bits(1)=0`, len 2 bits + len+1 interleave rounds).
//! * Bundles: FFmpeg's `cur_dec`/`cur_ptr` byte pointers over a
//!   zero-initialized buffer of `blocks*64` bytes (blocks = the LUMA
//!   block count; the same buffers are reused for the chroma planes).
//!   `get_value` reads `*cur_ptr++` — 1 byte everywhere except
//!   INTRA_DC/INTER_DC (little-endian i16, `cur_ptr += 2`) and
//!   returns whatever the buffer holds past `cur_dec` (the
//!   zero-buffer: an exhausted bt bundle decodes SKIP blocks — the
//!   encoder relies on this for all-skip tails).  The row-start
//!   refill (`CHECK_READ_VAL`) fires only when the bundle is alive
//!   and drained (`cur_dec == cur_ptr`, overshoot included); it reads
//!   the bundle's `len`-bit count `t`, and `t == 0` kills the bundle
//!   for the rest of the plane.
//! * Bundle t-widths (`init_lengths`): bt/dc/xoff/yoff
//!   `av_log2((width>>3)+511)+1`, sbt `av_log2((width>>4)+511)+1`,
//!   colors `av_log2(bw*64+511)+1`, pattern `av_log2((bw<<3)+511)+1`,
//!   run `av_log2(bw*48+511)+1` — width = the plane's pixel width
//!   (luma width for luma, `width>>1` for chroma), `bw` = the plane's
//!   block-column count.
//! * Plane dims: luma `bw=(width+7)>>3, bh=(height+7)>>3`; chroma
//!   `bw=(width+15)>>4, bh=(height+15)>>4` (8x8 chroma blocks — the
//!   width/height are the FULL frame's; FFmpeg derives the chroma
//!   dims from them, NOT from width/2 x height/2).
//! * Row loop: every row starts with the 9 refill calls in source
//!   order (bt, sbt, colors, pattern, xoff, yoff, intradc, interdc,
//!   run); the block loop pops bt per block column, with SCALED
//!   blocks covering 2 columns (on an odd row or odd column the
//!   pop is discarded without touching any other bundle).
//! * Plane tail: align to 32 bits; `decode_frame` then proceeds to
//!   the next plane.  Planes come in bitstream order Y, U, V — with
//!   `swap_planes` (every revision >= 'h', so all of this game's
//!   `BIKi` movies) FFmpeg writes bitstream plane 1 to the frame's V
//!   plane and plane 2 to U; the pixel layer must route accordingly.

use prototype_common::{P3dError, Result};

// ---------------------------------------------------------------------------
// FFmpeg binkdata.h static tables (verbatim constants)
// ---------------------------------------------------------------------------

/// `bink_tree_bits[16][16]` — the VLC code patterns (MSB-first as
/// written; the LE reader matches their LSB against the first bit).
const BINK_TREE_BITS: [[u16; 16]; 16] = [
    [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F],
    [0x00, 0x01, 0x03, 0x05, 0x07, 0x09, 0x0B, 0x0D, 0x0F, 0x13, 0x15, 0x17, 0x19, 0x1B, 0x1D, 0x1F],
    [0x00, 0x02, 0x01, 0x09, 0x05, 0x15, 0x0D, 0x1D, 0x03, 0x13, 0x0B, 0x1B, 0x07, 0x17, 0x0F, 0x1F],
    [0x00, 0x02, 0x06, 0x01, 0x09, 0x05, 0x0D, 0x1D, 0x03, 0x13, 0x0B, 0x1B, 0x07, 0x17, 0x0F, 0x1F],
    [0x00, 0x04, 0x02, 0x06, 0x01, 0x09, 0x05, 0x0D, 0x03, 0x13, 0x0B, 0x1B, 0x07, 0x17, 0x0F, 0x1F],
    [0x00, 0x04, 0x02, 0x0A, 0x06, 0x0E, 0x01, 0x09, 0x05, 0x0D, 0x03, 0x0B, 0x07, 0x17, 0x0F, 0x1F],
    [0x00, 0x02, 0x0A, 0x06, 0x0E, 0x01, 0x09, 0x05, 0x0D, 0x03, 0x0B, 0x1B, 0x07, 0x17, 0x0F, 0x1F],
    [0x00, 0x01, 0x05, 0x03, 0x13, 0x0B, 0x1B, 0x3B, 0x07, 0x27, 0x17, 0x37, 0x0F, 0x2F, 0x1F, 0x3F],
    [0x00, 0x01, 0x03, 0x13, 0x0B, 0x2B, 0x1B, 0x3B, 0x07, 0x27, 0x17, 0x37, 0x0F, 0x2F, 0x1F, 0x3F],
    [0x00, 0x01, 0x05, 0x0D, 0x03, 0x13, 0x0B, 0x1B, 0x07, 0x27, 0x17, 0x37, 0x0F, 0x2F, 0x1F, 0x3F],
    [0x00, 0x02, 0x01, 0x05, 0x0D, 0x03, 0x13, 0x0B, 0x1B, 0x07, 0x17, 0x37, 0x0F, 0x2F, 0x1F, 0x3F],
    [0x00, 0x01, 0x09, 0x05, 0x0D, 0x03, 0x13, 0x0B, 0x1B, 0x07, 0x17, 0x37, 0x0F, 0x2F, 0x1F, 0x3F],
    [0x00, 0x02, 0x01, 0x03, 0x13, 0x0B, 0x1B, 0x3B, 0x07, 0x27, 0x17, 0x37, 0x0F, 0x2F, 0x1F, 0x3F],
    [0x00, 0x01, 0x05, 0x03, 0x07, 0x27, 0x17, 0x37, 0x0F, 0x4F, 0x2F, 0x6F, 0x1F, 0x5F, 0x3F, 0x7F],
    [0x00, 0x01, 0x05, 0x03, 0x07, 0x17, 0x37, 0x77, 0x0F, 0x4F, 0x2F, 0x6F, 0x1F, 0x5F, 0x3F, 0x7F],
    [0x00, 0x02, 0x01, 0x05, 0x03, 0x07, 0x27, 0x17, 0x37, 0x0F, 0x2F, 0x6F, 0x1F, 0x5F, 0x3F, 0x7F],
];

/// `bink_tree_lens[16][16]` — the matching code lengths.
const BINK_TREE_LENS: [[u8; 16]; 16] = [
    [4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4],
    [1, 4, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5],
    [2, 2, 4, 4, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5],
    [2, 3, 3, 4, 4, 4, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5],
    [3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 5, 5, 5, 5],
    [3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5, 5, 5],
    [2, 4, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5, 5, 5, 5, 5],
    [1, 3, 3, 5, 5, 5, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6],
    [1, 2, 5, 5, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6],
    [1, 3, 4, 4, 5, 5, 5, 5, 6, 6, 6, 6, 6, 6, 6, 6],
    [2, 2, 3, 4, 4, 5, 5, 5, 5, 5, 6, 6, 6, 6, 6, 6],
    [1, 4, 4, 4, 4, 5, 5, 5, 5, 5, 6, 6, 6, 6, 6, 6],
    [2, 2, 2, 5, 5, 5, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6],
    [1, 3, 3, 3, 6, 6, 6, 6, 7, 7, 7, 7, 7, 7, 7, 7],
    [1, 3, 3, 3, 5, 6, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7],
    [2, 2, 3, 3, 3, 6, 6, 6, 6, 6, 7, 7, 7, 7, 7, 7],
];

/// `bink_rlelens` — the run lengths behind block-type symbols 12..15.
const RLE_LENS: [usize; 4] = [4, 8, 12, 32];

/// `DC_START_BITS` — the first-DC bit width in the dcs bundles.
const DC_START_BITS: usize = 11;

/// `bink_scan` — the DCT scan order (residue coefficient positions).
/// Block 51: the canonical copy now lives in [`crate::binkdata`] (the
/// mechanical transcription of binkdata.h); this module re-uses it.
use crate::binkdata::BINK_SCAN;

/// Block types (`enum BlockTypes`).
mod bt {
    pub const SKIP: u8 = 0;
    pub const SCALED: u8 = 1;
    pub const MOTION: u8 = 2;
    pub const RUN: u8 = 3;
    pub const RESIDUE: u8 = 4;
    pub const INTRA: u8 = 5;
    pub const FILL: u8 = 6;
    pub const INTER: u8 = 7;
    pub const PATTERN: u8 = 8;
    pub const RAW: u8 = 9;
}

/// 16x16 sub-block types accepted inside SCALED (FFmpeg's switch).
mod sbt {
    pub const RUN: u8 = 3;
    pub const INTRA: u8 = 5;
    pub const FILL: u8 = 6;
    pub const PATTERN: u8 = 8;
    pub const RAW: u8 = 9;
}

// ---------------------------------------------------------------------------
// LSB-first bit reader (fail-closed; FFmpeg's GetBitContext with
// BITSTREAM_READER_LE semantics)
// ---------------------------------------------------------------------------

/// LSB-first bit reader shared by the video (block 50) and audio
/// (block 52) codec layers.
pub(crate) struct LeBitReader<'a> {
    data: &'a [u8],
    /// Bit position from the start of `data`.
    pos: usize,
}

impl<'a> LeBitReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub(crate) fn bits_left(&self) -> usize {
        self.data.len() * 8 - self.pos
    }

    pub(crate) fn bit(&mut self) -> Result<u32> {
        if self.pos >= self.data.len() * 8 {
            return Err(P3dError::Eof {
                offset: self.data.len(),
                needed: 1,
            });
        }
        let b = ((self.data[self.pos >> 3] >> (self.pos & 7)) & 1) as u32;
        self.pos += 1;
        Ok(b)
    }

    /// `get_bits`: n bits, first stream bit = LSB of the result.
    pub(crate) fn bits(&mut self, n: usize) -> Result<u32> {
        if n > 25 {
            return Err(P3dError::Other(format!(
                "binkvideo: get_bits({n}) exceeds the 25-bit reader width"
            )));
        }
        let mut v = 0u32;
        for i in 0..n {
            v |= self.bit()? << i;
        }
        Ok(v)
    }

    /// The 32-bit align shared by the video plane tails and the audio
    /// block loop (FFmpeg's get_bits_align32).
    pub(crate) fn align32(&mut self) -> Result<()> {
        let r = self.pos & 0x1F;
        if r != 0 {
            let skip = 32 - r;
            if skip > self.bits_left() {
                return Err(P3dError::Eof {
                    offset: self.data.len(),
                    needed: (skip - self.bits_left() + 7) / 8,
                });
            }
            self.pos += skip;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// VLC decode (FFmpeg's single-level LE table == incremental LSB match)
// ---------------------------------------------------------------------------

/// Decode one VLC symbol index (0..16) with table `vlc_num`.
///
/// FFmpeg builds a single-level table indexed by the code value with
/// `VLC_INIT_LE` (first stream bit = code LSB) and `get_vlc2(...,
/// table.bits, 1)`; that is exactly an incremental LSB-first match
/// against `(code, len)`, shortest code first.
fn huff_decode(gb: &mut LeBitReader, vlc_num: usize) -> Result<usize> {
    let mut acc = 0u32;
    for n in 1..=8usize {
        acc |= gb.bit()? << (n - 1);
        let mask = (1u32 << n) - 1;
        for (sym, len) in BINK_TREE_LENS[vlc_num].iter().enumerate() {
            if *len as usize == n && (acc & mask) == (BINK_TREE_BITS[vlc_num][sym] as u32 & mask) {
                return Ok(sym);
            }
        }
    }
    Err(P3dError::Other(format!(
        "binkvideo: no VLC symbol in table {vlc_num} (acc={acc:#x})"
    )))
}

/// A decoded Huffman tree (`Tree` in bink.c).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tree {
    vlc_num: usize,
    syms: [u8; 16],
}

impl Tree {
    fn identity() -> Self {
        Self {
            vlc_num: 0,
            syms: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
        }
    }
}

/// `read_tree` — verbatim semantics.
fn read_tree(gb: &mut LeBitReader) -> Result<Tree> {
    let vlc_num = gb.bits(4)? as usize;
    if vlc_num == 0 {
        return Ok(Tree::identity());
    }
    let mut syms = [0u8; 16];
    if gb.bit()? != 0 {
        // explicit symbol list; the unused tail is filled in ascending order
        let mut len = gb.bits(3)? as usize;
        let mut used = [false; 16];
        if len >= 16 {
            return Err(P3dError::Other(format!(
                "binkvideo: read_tree list len {len} out of range"
            )));
        }
        for s in syms.iter_mut().take(len + 1) {
            let v = gb.bits(4)? as u8;
            *s = v;
            used[v as usize] = true;
        }
        let mut i = 0usize;
        while i < 16 && len < 15 {
            if !used[i] {
                len += 1;
                syms[len] = i as u8;
            }
            i += 1;
        }
    } else {
        // merge permutation: len+1 interleave rounds of doubling size
        let len = gb.bits(2)? as usize;
        let mut cur: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        for round in 0..=len {
            let size = 1usize << round;
            let mut out = [0u8; 16];
            let mut t = 0usize;
            while t < 16 {
                merge(gb, &mut out, t, &cur, t, size)?;
                t += size << 1;
            }
            cur = out;
        }
        syms = cur;
    }
    Ok(Tree { vlc_num, syms })
}

/// FFmpeg's `merge`: interleave src[soff..soff+size) and
/// src[soff+size..soff+2*size) by decision bits into dst[doff..].
fn merge(
    gb: &mut LeBitReader,
    dst: &mut [u8; 16],
    doff: usize,
    src: &[u8; 16],
    soff: usize,
    size: usize,
) -> Result<()> {
    let mut p1 = soff;
    let mut p2 = soff + size;
    let mut s1 = size;
    let mut s2 = size;
    let mut k = doff;
    while s1 > 0 && s2 > 0 {
        if gb.bit()? == 0 {
            dst[k] = src[p1];
            p1 += 1;
            s1 -= 1;
        } else {
            dst[k] = src[p2];
            p2 += 1;
            s2 -= 1;
        }
        k += 1;
    }
    while s1 > 0 {
        dst[k] = src[p1];
        p1 += 1;
        s1 -= 1;
        k += 1;
    }
    while s2 > 0 {
        dst[k] = src[p2];
        p2 += 1;
        s2 -= 1;
        k += 1;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Bundles (FFmpeg's cur_dec/cur_ptr byte-pointer model)
// ---------------------------------------------------------------------------

/// Bundle sources (`enum Sources`).
const B_BT: usize = 0;
const B_SBT: usize = 1;
const B_COLORS: usize = 2;
const B_PATTERN: usize = 3;
const B_XOFF: usize = 4;
const B_YOFF: usize = 5;
const B_INTRADC: usize = 6;
const B_INTERDC: usize = 7;
const B_RUN: usize = 8;
const BINK_NB_SRC: usize = 9;

struct Bundle {
    /// The refill count's bit width (`Bundle.len`).
    len: usize,
    tree: Tree,
    /// Zero-initialized entry buffer; i16 entries occupy 2 bytes.
    data: Vec<u8>,
    /// Byte pointers (FFmpeg semantics).
    cur_dec: usize,
    cur_ptr: usize,
    /// The fill's start position (where the in-flight refill began).
    cur_dec_start: usize,
    /// `cur_dec == NULL` — killed by a t=0 refill for the whole plane.
    killed: bool,
    /// INTRA_DC/INTER_DC carry i16 entries (2-byte steps).
    wide: bool,
    /// X_OFF/Y_OFF read as signed i8.
    signed8: bool,
}

impl Bundle {
    fn new(len: usize, capacity: usize, wide: bool, signed8: bool) -> Self {
        Self {
            len,
            tree: Tree::identity(),
            data: vec![0u8; capacity],
            cur_dec: 0,
            cur_ptr: 0,
            cur_dec_start: 0,
            killed: false,
            wide,
            signed8,
        }
    }

    /// `get_value`: `*cur_ptr++` (i16 LE for the dcs bundles, i8 for
    /// the motion bundles); reads the zero-buffer past `cur_dec`.
    fn get_value(&mut self) -> Result<i32> {
        if self.wide {
            if self.cur_ptr + 2 > self.data.len() {
                return Err(P3dError::Other("binkvideo: dcs bundle pointer overrun".into()));
            }
            let v = i16::from_le_bytes([self.data[self.cur_ptr], self.data[self.cur_ptr + 1]]);
            self.cur_ptr += 2;
            Ok(v as i32)
        } else {
            if self.cur_ptr >= self.data.len() {
                return Err(P3dError::Other("binkvideo: bundle pointer overrun".into()));
            }
            let b = self.data[self.cur_ptr];
            self.cur_ptr += 1;
            Ok(if self.signed8 { b as i8 as i32 } else { b as i32 })
        }
    }

    /// `CHECK_READ_VAL`'s gate: a refill only fires when the bundle
    /// is alive and fully consumed (overshoot included).
    fn needs_refill(&self) -> bool {
        !self.killed && self.cur_dec <= self.cur_ptr
    }

    fn read_t(&mut self, gb: &mut LeBitReader) -> Result<Option<usize>> {
        let t = gb.bits(self.len)? as usize;
        if t == 0 {
            self.killed = true;
            return Ok(None);
        }
        Ok(Some(t))
    }
}

/// `init_lengths` — the per-bundle t widths for one plane, in the
/// `B_*` source order: bt, sbt, colors, pattern, xoff, yoff, intradc,
/// interdc, run (`av_log2(x + 511) + 1` == the bit length of `x+511`;
/// x/yoff and both dcs share the bt width, FFmpeg's `init_lengths`).
fn init_lengths(plane_width: usize, bw: usize) -> [usize; BINK_NB_SRC] {
    let width = (plane_width + 7) & !7;
    let l = |v: usize| -> usize { (usize::BITS - (v + 511).leading_zeros()) as usize };
    [
        l(width >> 3), // bt
        l(width >> 4), // sbt
        l(bw * 64),    // colors
        l(bw << 3),    // pattern
        l(width >> 3), // xoff
        l(width >> 3), // yoff
        l(width >> 3), // intradc
        l(width >> 3), // interdc
        l(bw * 48),    // run
    ]
}

// ---------------------------------------------------------------------------
// DCT coefficient / residue walks (consumption + payload capture)
// ---------------------------------------------------------------------------

/// One written DCT/residue coefficient.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DctCoef {
    /// For DCT blocks: the raw `ccoef` (the index INTO `bink_scan`; the
    /// pixel layer applies the scan).  For residue blocks: the ALREADY
    /// scan-mapped raster position (FFmpeg's `nz_coeff` semantics).
    pub ccoef: u8,
    /// DCT blocks: the absolute coefficient value.  Residue blocks:
    /// the `+mask` DELTA to accumulate onto the (zero-initialized)
    /// block position, in stream order.
    pub value: i32,
}

/// `read_dct_coeffs` (the `q == -1` variant: a 4-bit quant index at
/// the end).  Verbatim walk including the case-0 fallthrough and the
/// per-sub-iteration `ccoef++`.
fn read_dct_coeffs(gb: &mut LeBitReader, out: &mut Vec<DctCoef>) -> Result<u8> {
    let mut coef_list = [0i32; 128];
    let mut mode_list = [0i32; 128];
    let mut list_start = 64usize;
    let mut list_end = 64usize;

    coef_list[list_end] = 4;
    mode_list[list_end] = 0;
    list_end += 1;
    coef_list[list_end] = 24;
    mode_list[list_end] = 0;
    list_end += 1;
    coef_list[list_end] = 44;
    mode_list[list_end] = 0;
    list_end += 1;
    coef_list[list_end] = 1;
    mode_list[list_end] = 3;
    list_end += 1;
    coef_list[list_end] = 2;
    mode_list[list_end] = 3;
    list_end += 1;
    coef_list[list_end] = 3;
    mode_list[list_end] = 3;
    list_end += 1;

    // a 0 nibble is LEGAL: bits becomes -1 and the walk goes straight
    // to the trailing quant index (FFmpeg's `for (bits = ..; bits >= 0)`)
    let mut bits = gb.bits(4)? as i32 - 1;
    while bits >= 0 {
        let mut list_pos = list_start;
        while list_pos < list_end {
            if (mode_list[list_pos] | coef_list[list_pos]) == 0 || gb.bit()? == 0 {
                list_pos += 1;
                continue;
            }
            let mut ccoef = coef_list[list_pos];
            let mode = mode_list[list_pos];
            match mode {
                0 => {
                    coef_list[list_pos] = ccoef + 4;
                    mode_list[list_pos] = 1;
                    // FFmpeg's case 0 FALLS THROUGH into the case-2 body
                    // (mode != 2 there: no zeroing, no list_pos advance)
                    dct_case2(gb, &mut coef_list, &mut mode_list, &mut list_start, &mut list_end, &mut list_pos, &mut ccoef, bits, out)?;
                }
                2 => {
                    coef_list[list_pos] = 0;
                    mode_list[list_pos] = 0;
                    list_pos += 1;
                    dct_case2(gb, &mut coef_list, &mut mode_list, &mut list_start, &mut list_end, &mut list_pos, &mut ccoef, bits, out)?;
                }
                1 => {
                    mode_list[list_pos] = 2;
                    for _ in 0..3 {
                        ccoef += 4;
                        coef_list[list_end] = ccoef;
                        mode_list[list_end] = 2;
                        list_end += 1;
                    }
                    // no list_pos advance: the entry is re-examined
                }
                3 => {
                    let t = dct_value(gb, bits)?;
                    out.push(DctCoef {
                        ccoef: ccoef as u8,
                        value: t,
                    });
                    coef_list[list_pos] = 0;
                    mode_list[list_pos] = 0;
                    list_pos += 1;
                }
                _ => unreachable!(),
            }
        }
        bits -= 1;
    }
    Ok(gb.bits(4)? as u8)
}

/// The shared case-2 body (4 sub-iterations; `mode == 2` callers have
/// already zeroed the entry and advanced `list_pos`).
#[allow(clippy::too_many_arguments)]
fn dct_case2(
    gb: &mut LeBitReader,
    coef_list: &mut [i32; 128],
    mode_list: &mut [i32; 128],
    list_start: &mut usize,
    _list_end: &mut usize,
    _list_pos: &mut usize,
    ccoef: &mut i32,
    bits: i32,
    out: &mut Vec<DctCoef>,
) -> Result<()> {
    for _ in 0..4 {
        if gb.bit()? != 0 {
            *list_start -= 1;
            coef_list[*list_start] = *ccoef;
            mode_list[*list_start] = 3;
        } else {
            let t = dct_value(gb, bits)?;
            out.push(DctCoef {
                ccoef: *ccoef as u8,
                value: t,
            });
        }
        *ccoef += 1;
    }
    Ok(())
}

/// The `t` value of one DCT coefficient (`bits == 0` special case).
fn dct_value(gb: &mut LeBitReader, bits: i32) -> Result<i32> {
    if bits == 0 {
        Ok(1 - ((gb.bit()? << 1) as i32))
    } else {
        let mut t = (gb.bits(bits as usize)? | (1 << bits)) as i32;
        let sign = -(gb.bit()? as i32);
        t = (t ^ sign) - sign;
        Ok(t)
    }
}

/// `read_residue` — verbatim walk over the mask pyramid.
fn read_residue(gb: &mut LeBitReader, masks_count: u32, out: &mut Vec<DctCoef>) -> Result<()> {
    let mut coef_list = [0i32; 128];
    let mut mode_list = [0i32; 128];
    let mut list_start = 64usize;
    let mut list_end = 64usize;
    let mut nz_coeff: [u8; 64] = [0; 64];
    let mut nz_count = 0usize;
    let mut masks_count = masks_count as i32;

    coef_list[list_end] = 4;
    mode_list[list_end] = 0;
    list_end += 1;
    coef_list[list_end] = 24;
    mode_list[list_end] = 0;
    list_end += 1;
    coef_list[list_end] = 44;
    mode_list[list_end] = 0;
    list_end += 1;
    coef_list[list_end] = 0;
    mode_list[list_end] = 2;
    list_end += 1;

    let mut mask = 1i32 << gb.bits(3)?;
    while mask != 0 {
        for i in 0..nz_count {
            if gb.bit()? == 0 {
                continue;
            }
            // block[nz_coeff[i]] += mask (sign by the current value)
            out.push(DctCoef {
                ccoef: nz_coeff[i],
                value: mask as i32,
            });
            masks_count -= 1;
            if masks_count < 0 {
                return Ok(());
            }
        }
        let mut list_pos = list_start;
        while list_pos < list_end {
            if (coef_list[list_pos] | mode_list[list_pos]) == 0 || gb.bit()? == 0 {
                list_pos += 1;
                continue;
            }
            let mut ccoef = coef_list[list_pos];
            let mode = mode_list[list_pos];
            match mode {
                0 => {
                    coef_list[list_pos] = ccoef + 4;
                    mode_list[list_pos] = 1;
                    residue_case2(
                        gb,
                        &mut coef_list,
                        &mut mode_list,
                        &mut list_start,
                        &mut list_end,
                        &mut list_pos,
                        &mut ccoef,
                        mask,
                        &mut nz_coeff,
                        &mut nz_count,
                        &mut masks_count,
                        out,
                    )?;
                }
                2 => {
                    coef_list[list_pos] = 0;
                    mode_list[list_pos] = 0;
                    list_pos += 1;
                    residue_case2(
                        gb,
                        &mut coef_list,
                        &mut mode_list,
                        &mut list_start,
                        &mut list_end,
                        &mut list_pos,
                        &mut ccoef,
                        mask,
                        &mut nz_coeff,
                        &mut nz_count,
                        &mut masks_count,
                        out,
                    )?;
                }
                1 => {
                    mode_list[list_pos] = 2;
                    for _ in 0..3 {
                        ccoef += 4;
                        coef_list[list_end] = ccoef;
                        mode_list[list_end] = 2;
                        list_end += 1;
                    }
                }
                3 => {
                    nz_coeff[nz_count] = BINK_SCAN[ccoef as usize];
                    nz_count += 1;
                    let sign = -(gb.bit()? as i32);
                    out.push(DctCoef {
                        ccoef: BINK_SCAN[ccoef as usize],
                        value: (mask ^ sign) - sign,
                    });
                    coef_list[list_pos] = 0;
                    mode_list[list_pos] = 0;
                    list_pos += 1;
                    masks_count -= 1;
                    if masks_count < 0 {
                        return Ok(());
                    }
                }
                _ => unreachable!(),
            }
            if masks_count < 0 {
                return Ok(());
            }
        }
        mask >>= 1;
    }
    Ok(())
}

/// The shared residue case-2 body (4 sub-iterations).
#[allow(clippy::too_many_arguments)]
fn residue_case2(
    gb: &mut LeBitReader,
    coef_list: &mut [i32; 128],
    mode_list: &mut [i32; 128],
    list_start: &mut usize,
    _list_end: &mut usize,
    _list_pos: &mut usize,
    ccoef: &mut i32,
    mask: i32,
    nz_coeff: &mut [u8; 64],
    nz_count: &mut usize,
    masks_count: &mut i32,
    out: &mut Vec<DctCoef>,
) -> Result<()> {
    for _ in 0..4 {
        if gb.bit()? != 0 {
            *list_start -= 1;
            coef_list[*list_start] = *ccoef;
            mode_list[*list_start] = 3;
        } else {
            nz_coeff[*nz_count] = BINK_SCAN[*ccoef as usize];
            *nz_count += 1;
            let sign = -(gb.bit()? as i32);
            out.push(DctCoef {
                ccoef: BINK_SCAN[*ccoef as usize],
                value: (mask ^ sign) - sign,
            });
            *masks_count -= 1;
            if *masks_count < 0 {
                return Ok(());
            }
        }
        *ccoef += 1;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-block payload records (what a later pixel layer renders from)
// ---------------------------------------------------------------------------

/// One decoded 8x8 sub-block payload (inside a 16x16 SCALED block).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubBlock {
    Run {
        /// The 4-bit `bink_patterns` selector.
        pattern: u8,
        /// Stream-order runs: `run` coefficient positions, either one
        /// color for all of them (`color: Some`) or one color each.
        runs: Vec<RunItem>,
    },
    Intra {
        dc: i16,
        quant: u8,
        coeffs: Vec<DctCoef>,
    },
    Fill {
        color: u8,
    },
    Pattern {
        colors: [u8; 2],
        /// The 8 pattern nibbles (2 bits each after expansion — the
        /// raw bundle values).
        patterns: [u8; 8],
    },
    Raw {
        colors: [u8; 64],
    },
}

/// One RUN-block run entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunItem {
    /// `run_value + 1` — how many scan positions this entry covers.
    pub run: u16,
    /// `Some(c)` when the stream set the single-color bit (the run's
    /// positions all take `c`); `None` when each position pops its
    /// own color from the colors bundle (the pops are recorded in
    /// `PlaneSymbols::colors_consumed`).
    pub color: Option<u8>,
}

/// One decoded block (one `bt` pop with its full payload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockRecord {
    /// SKIP — copy the previous frame's block.
    Skip,
    /// SCALED on an odd row/column — discarded (part of the previous
    /// 16x16 block); the pop still happened.
    ScaledSkip,
    /// SCALED with its 8x8 sub-block payload (the pixel layer scales
    /// it to 16x16; a FILL sub-block fills 16x16 directly).
    Scaled(Box<SubBlock>),
    Motion {
        x: i8,
        y: i8,
    },
    Run {
        pattern: u8,
        runs: Vec<RunItem>,
    },
    Residue {
        x: i8,
        y: i8,
        /// The 7-bit mask count and the accumulated deltas.
        coeffs: Vec<DctCoef>,
    },
    Intra {
        dc: i16,
        quant: u8,
        coeffs: Vec<DctCoef>,
    },
    Fill {
        color: u8,
    },
    Inter {
        x: i8,
        y: i8,
        dc: i16,
        quant: u8,
        coeffs: Vec<DctCoef>,
    },
    Pattern {
        colors: [u8; 2],
        patterns: [u8; 8],
    },
    Raw {
        colors: [u8; 64],
    },
}

/// The symbol walk of one plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneSymbols {
    /// The plane's block columns (`(width+7)>>3` luma,
    /// `(width+15)>>4` chroma — width/height are the FULL frame's).
    pub bw: usize,
    pub bh: usize,
    /// One record per bt pop, in pop order (row-major).
    pub blocks: Vec<BlockRecord>,
    /// Every colors-bundle value popped while decoding the blocks, in
    /// pop order (RUN blocks reference their per-position colors here).
    pub colors_consumed: Vec<u8>,
    /// Every pattern-bundle value popped (PATTERN blocks).
    pub patterns_consumed: Vec<u8>,
    /// Every xoff/yoff value popped (MOTION/RESIDUE/INTER blocks), in
    /// pop order (x then y per block).
    pub xoff_consumed: Vec<i8>,
    pub yoff_consumed: Vec<i8>,
    /// Every INTRA/INTER DC value popped, in pop order.
    pub intradc_consumed: Vec<i16>,
    pub interdc_consumed: Vec<i16>,
    /// Bit position where the plane's walk ended (after the tail
    /// align), from the start of the video bitstream.
    pub end_bit: usize,
}

/// Decode one plane's symbol layer from `video` starting at bit
/// `start_bit`.  `width`/`height` are the FULL frame's pixel dims;
/// `is_chroma` selects the chroma dims and lengths (FFmpeg passes the
/// full dims and derives the plane geometry internally).
pub fn decode_plane_symbols(
    video: &[u8],
    start_bit: usize,
    width: usize,
    height: usize,
    is_chroma: bool,
) -> Result<PlaneSymbols> {
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err(P3dError::Other(format!(
            "binkvideo: bad plane dims {width}x{height}"
        )));
    }
    let bw = if is_chroma { (width + 15) >> 4 } else { (width + 7) >> 3 };
    let bh = if is_chroma { (height + 15) >> 4 } else { (height + 7) >> 3 };
    let plane_width = width >> (is_chroma as usize);
    if start_bit > video.len() * 8 {
        return Err(P3dError::Other(format!(
            "binkvideo: start bit {start_bit} beyond the {} B video region",
            video.len()
        )));
    }
    let mut gb = LeBitReader::new(video);
    gb.pos = start_bit;

    // FFmpeg's init_bundles: blocks*64 bytes per bundle, blocks = the
    // LUMA block count (the same buffers serve the chroma planes).
    let lbw = (width + 7) >> 3;
    let lbh = (height + 7) >> 3;
    let blocks = lbw * lbh;
    let capacity = blocks * 64;
    let lens = init_lengths(plane_width.max(8), bw);
    let mut bundles: Vec<Bundle> = vec![
        Bundle::new(lens[B_BT], capacity, false, false),
        Bundle::new(lens[B_SBT], capacity, false, false),
        Bundle::new(lens[B_COLORS], capacity, false, false),
        Bundle::new(lens[B_PATTERN], capacity, false, false),
        Bundle::new(lens[B_XOFF], capacity, false, true),
        Bundle::new(lens[B_YOFF], capacity, false, true),
        Bundle::new(lens[B_INTRADC], capacity, true, false),
        Bundle::new(lens[B_INTERDC], capacity, true, false),
        Bundle::new(lens[B_RUN], capacity, false, false),
    ];
    let mut col_high: [Tree; 16] = [Tree::identity(); 16];
    let mut col_lastval = 0usize;

    // --- prologue: read_bundle for sources 0..8 in order ---
    // COLORS first reads its 16 col_high trees, then every non-dcs
    // bundle reads its own tree; cur_dec/cur_ptr reset to the buffer.
    for i in 0..BINK_NB_SRC {
        if i == B_COLORS {
            for t in col_high.iter_mut() {
                *t = read_tree(&mut gb)?;
            }
            col_lastval = 0;
        }
        if i != B_INTRADC && i != B_INTERDC {
            bundles[i].tree = read_tree(&mut gb)?;
        }
        bundles[i].cur_dec = 0;
        bundles[i].cur_ptr = 0;
        bundles[i].killed = false;
    }

    // --- refill: FFmpeg's read_* functions over one bundle ---

    fn read_runs_fill(b: &mut Bundle, gb: &mut LeBitReader, t: usize) -> Result<()> {
        if b.cur_dec + t > b.data.len() {
            return Err(P3dError::Other("binkvideo: runs fill oob".into()));
        }
        if gb.bit()? != 0 {
            let v = gb.bits(4)? as u8;
            b.data[b.cur_dec..b.cur_dec + t].fill(v);
            b.cur_dec += t;
        } else {
            while b.cur_dec < b.cur_dec_start + t {
                let sym = huff_decode(gb, b.tree.vlc_num)?;
                b.data[b.cur_dec] = b.tree.syms[sym];
                b.cur_dec += 1;
            }
        }
        Ok(())
    }

    fn read_motion_fill(b: &mut Bundle, gb: &mut LeBitReader, t: usize) -> Result<()> {
        if b.cur_dec + t > b.data.len() {
            return Err(P3dError::Other("binkvideo: motion fill oob".into()));
        }
        let end = b.cur_dec_start + t;
        if gb.bit()? != 0 {
            let mut v = gb.bits(4)? as i32;
            if v != 0 {
                let sign = -(gb.bit()? as i32);
                v = (v ^ sign) - sign;
            }
            b.data[b.cur_dec..end].fill(v as i8 as u8);
            b.cur_dec = end;
        } else {
            while b.cur_dec < end {
                let sym = huff_decode(gb, b.tree.vlc_num)?;
                let mut v = b.tree.syms[sym] as i32;
                if v != 0 {
                    let sign = -(gb.bit()? as i32);
                    v = (v ^ sign) - sign;
                }
                b.data[b.cur_dec] = v as i8 as u8;
                b.cur_dec += 1;
            }
        }
        Ok(())
    }

    fn read_bt_fill(b: &mut Bundle, gb: &mut LeBitReader, t: usize) -> Result<()> {
        if b.cur_dec + t > b.data.len() {
            return Err(P3dError::Other("binkvideo: block-types fill oob".into()));
        }
        let end = b.cur_dec_start + t;
        let mut last = 0u8;
        if gb.bit()? != 0 {
            let v = gb.bits(4)? as u8;
            b.data[b.cur_dec..end].fill(v);
            b.cur_dec = end;
        } else {
            while b.cur_dec < end {
                let sym = huff_decode(gb, b.tree.vlc_num)?;
                let v = b.tree.syms[sym];
                if v < 12 {
                    last = v;
                    b.data[b.cur_dec] = v;
                    b.cur_dec += 1;
                } else {
                    let run = RLE_LENS[(v - 12) as usize];
                    if end - b.cur_dec < run {
                        return Err(P3dError::Other(format!(
                            "binkvideo: block-type RLE run {run} overshoots the {t} entries"
                        )));
                    }
                    b.data[b.cur_dec..b.cur_dec + run].fill(last);
                    b.cur_dec += run;
                }
            }
        }
        Ok(())
    }

    fn read_patterns_fill(b: &mut Bundle, gb: &mut LeBitReader, t: usize) -> Result<()> {
        if b.cur_dec + t > b.data.len() {
            return Err(P3dError::Other("binkvideo: patterns fill oob".into()));
        }
        let end = b.cur_dec_start + t;
        while b.cur_dec < end {
            let lo = huff_decode(gb, b.tree.vlc_num)?;
            let hi = huff_decode(gb, b.tree.vlc_num)?;
            b.data[b.cur_dec] = b.tree.syms[lo] | (b.tree.syms[hi] << 4);
            b.cur_dec += 1;
        }
        Ok(())
    }

    fn read_colors_fill(
        b: &mut Bundle,
        gb: &mut LeBitReader,
        t: usize,
        col_high: &mut [Tree; 16],
        col_lastval: &mut usize,
    ) -> Result<()> {
        if b.cur_dec + t > b.data.len() {
            return Err(P3dError::Other("binkvideo: colors fill oob".into()));
        }
        let end = b.cur_dec_start + t;
        if gb.bit()? != 0 {
            let high = huff_decode(gb, col_high[*col_lastval].vlc_num)?;
            *col_lastval = col_high[*col_lastval].syms[high] as usize;
            let low = huff_decode(gb, b.tree.vlc_num)?;
            let v = ((*col_lastval as u8) << 4) | b.tree.syms[low];
            b.data[b.cur_dec..end].fill(v);
            b.cur_dec = end;
        } else {
            while b.cur_dec < end {
                let high = huff_decode(gb, col_high[*col_lastval].vlc_num)?;
                *col_lastval = col_high[*col_lastval].syms[high] as usize;
                let low = huff_decode(gb, b.tree.vlc_num)?;
                b.data[b.cur_dec] = ((*col_lastval as u8) << 4) | b.tree.syms[low];
                b.cur_dec += 1;
            }
        }
        Ok(())
    }

    fn read_dcs_fill(
        b: &mut Bundle,
        gb: &mut LeBitReader,
        t: usize,
        start_bits: usize,
        has_sign: bool,
    ) -> Result<()> {
        // i16 entries over the byte buffer
        let end = b.cur_dec_start + t * 2;
        if end > b.data.len() {
            return Err(P3dError::Other("binkvideo: dcs fill oob".into()));
        }
        let mut v = gb.bits(start_bits - has_sign as usize)? as i32;
        if v != 0 && has_sign {
            let sign = -(gb.bit()? as i32);
            v = (v ^ sign) - sign;
        }
        if v < -32768 || v > 32767 {
            return Err(P3dError::Other("binkvideo: dc out of range".into()));
        }
        let mut cur = b.cur_dec;
        b.data[cur..cur + 2].copy_from_slice(&(v as i16).to_le_bytes());
        cur += 2;
        let mut i = 1usize;
        while i < t {
            let len2 = (t - i).min(8);
            let bsize = gb.bits(4)? as usize;
            if bsize != 0 {
                for _ in 0..len2 {
                    let mut v2 = gb.bits(bsize)? as i32;
                    if v2 != 0 {
                        let sign = -(gb.bit()? as i32);
                        v2 = (v2 ^ sign) - sign;
                    }
                    v += v2;
                    if !(-32768..=32767).contains(&v) {
                        return Err(P3dError::Other("binkvideo: dc out of range".into()));
                    }
                    b.data[cur..cur + 2].copy_from_slice(&(v as i16).to_le_bytes());
                    cur += 2;
                }
            } else {
                for _ in 0..len2 {
                    b.data[cur..cur + 2].copy_from_slice(&(v as i16).to_le_bytes());
                    cur += 2;
                }
            }
            i += len2;
        }
        b.cur_dec = cur;
        Ok(())
    }

    fn do_refill(
        bundles: &mut [Bundle],
        gb: &mut LeBitReader,
        col_high: &mut [Tree; 16],
        col_lastval: &mut usize,
        idx: usize,
    ) -> Result<()> {
        if !bundles[idx].needs_refill() {
            return Ok(());
        }
        let t = match bundles[idx].read_t(gb)? {
            Some(t) => t,
            None => return Ok(()),
        };
        bundles[idx].cur_dec_start = bundles[idx].cur_dec;
        match idx {
            B_BT | B_SBT => read_bt_fill(&mut bundles[idx], gb, t)?,
            B_COLORS => read_colors_fill(&mut bundles[idx], gb, t, col_high, col_lastval)?,
            B_PATTERN => read_patterns_fill(&mut bundles[idx], gb, t)?,
            B_XOFF | B_YOFF => read_motion_fill(&mut bundles[idx], gb, t)?,
            B_RUN => read_runs_fill(&mut bundles[idx], gb, t)?,
            B_INTRADC => read_dcs_fill(&mut bundles[idx], gb, t, DC_START_BITS, false)?,
            B_INTERDC => read_dcs_fill(&mut bundles[idx], gb, t, DC_START_BITS, true)?,
            _ => unreachable!(),
        }
        Ok(())
    }

    fn gv(bundles: &mut [Bundle], idx: usize) -> Result<i32> {
        bundles[idx].get_value()
    }

    let mut symbols = PlaneSymbols {
        bw,
        bh,
        blocks: Vec::new(),
        colors_consumed: Vec::new(),
        patterns_consumed: Vec::new(),
        xoff_consumed: Vec::new(),
        yoff_consumed: Vec::new(),
        intradc_consumed: Vec::new(),
        interdc_consumed: Vec::new(),
        end_bit: 0,
    };

    for by in 0..bh {
        for idx in [
            B_BT, B_SBT, B_COLORS, B_PATTERN, B_XOFF, B_YOFF, B_INTRADC, B_INTERDC, B_RUN,
        ] {
            do_refill(&mut bundles, &mut gb, &mut col_high, &mut col_lastval, idx)?;
        }
        let mut bx = 0usize;
        while bx < bw {
            let blk = gv(&mut bundles, B_BT)? as u8;
            let odd_col = bx & 1;
            bx += 1;
            if ((by & 1) != 0 || odd_col != 0) && blk == bt::SCALED {
                symbols.blocks.push(BlockRecord::ScaledSkip);
                bx += 1;
                continue;
            }
            let record = match blk {
                bt::SKIP => BlockRecord::Skip,
                bt::SCALED => {
                    let sblk = gv(&mut bundles, B_SBT)? as u8;
                    let sub = match sblk {
                        sbt::RUN => {
                            let pattern = gb.bits(4)? as u8;
                            let mut runs = Vec::new();
                            let mut i = 0u32;
                            loop {
                                let run = (gv(&mut bundles, B_RUN)? as u16) + 1;
                                i += run as u32;
                                if i > 64 {
                                    return Err(P3dError::Other(
                                        "binkvideo: scaled run out of bounds".into(),
                                    ));
                                }
                                if gb.bit()? != 0 {
                                    let c = gv(&mut bundles, B_COLORS)? as u8;
                                    symbols.colors_consumed.push(c);
                                    runs.push(RunItem {
                                        run,
                                        color: Some(c),
                                    });
                                } else {
                                    for _ in 0..run {
                                        let c = gv(&mut bundles, B_COLORS)? as u8;
                                        symbols.colors_consumed.push(c);
                                    }
                                    runs.push(RunItem { run, color: None });
                                }
                                if i >= 63 {
                                    break;
                                }
                            }
                            if i == 63 {
                                let c = gv(&mut bundles, B_COLORS)? as u8;
                                symbols.colors_consumed.push(c);
                                // Block 51: the i==63 tail pop is a real
                                // 1-position run — record it so the pixel
                                // layer's colors-stream cursor stays in
                                // lockstep (FFmpeg fills scan position 63
                                // with this color).
                                runs.push(RunItem { run: 1, color: Some(c) });
                            }
                            SubBlock::Run { pattern, runs }
                        }
                        sbt::INTRA => {
                            let dc = gv(&mut bundles, B_INTRADC)? as i16;
                            symbols.intradc_consumed.push(dc);
                            let mut coeffs = Vec::new();
                            let quant = read_dct_coeffs(&mut gb, &mut coeffs)?;
                            SubBlock::Intra {
                                dc,
                                quant,
                                coeffs,
                            }
                        }
                        sbt::FILL => {
                            let c = gv(&mut bundles, B_COLORS)? as u8;
                            symbols.colors_consumed.push(c);
                            SubBlock::Fill { color: c }
                        }
                        sbt::PATTERN => {
                            let c0 = gv(&mut bundles, B_COLORS)? as u8;
                            let c1 = gv(&mut bundles, B_COLORS)? as u8;
                            symbols.colors_consumed.push(c0);
                            symbols.colors_consumed.push(c1);
                            let mut patterns = [0u8; 8];
                            for p in patterns.iter_mut() {
                                let v = gv(&mut bundles, B_PATTERN)? as u8;
                                symbols.patterns_consumed.push(v);
                                *p = v;
                            }
                            SubBlock::Pattern {
                                colors: [c0, c1],
                                patterns,
                            }
                        }
                        sbt::RAW => {
                            let mut colors = [0u8; 64];
                            for c in colors.iter_mut() {
                                let v = gv(&mut bundles, B_COLORS)? as u8;
                                symbols.colors_consumed.push(v);
                                *c = v;
                            }
                            SubBlock::Raw { colors }
                        }
                        other => {
                            return Err(P3dError::Other(format!(
                                "binkvideo: bad 16x16 sub-block type {other}"
                            )))
                        }
                    };
                    BlockRecord::Scaled(Box::new(sub))
                }
                bt::MOTION => {
                    let x = gv(&mut bundles, B_XOFF)? as i8;
                    let y = gv(&mut bundles, B_YOFF)? as i8;
                    symbols.xoff_consumed.push(x);
                    symbols.yoff_consumed.push(y);
                    BlockRecord::Motion { x, y }
                }
                bt::RUN => {
                    let pattern = gb.bits(4)? as u8;
                    let mut runs = Vec::new();
                    let mut i = 0u32;
                    loop {
                        let run = (gv(&mut bundles, B_RUN)? as u16) + 1;
                        i += run as u32;
                        if i > 64 {
                            return Err(P3dError::Other(
                                "binkvideo: run out of bounds".into(),
                            ));
                        }
                        if gb.bit()? != 0 {
                            let c = gv(&mut bundles, B_COLORS)? as u8;
                            symbols.colors_consumed.push(c);
                            runs.push(RunItem {
                                run,
                                color: Some(c),
                            });
                        } else {
                            for _ in 0..run {
                                let c = gv(&mut bundles, B_COLORS)? as u8;
                                symbols.colors_consumed.push(c);
                            }
                            runs.push(RunItem { run, color: None });
                        }
                        if i >= 63 {
                            break;
                        }
                    }
                    if i == 63 {
                        let c = gv(&mut bundles, B_COLORS)? as u8;
                        symbols.colors_consumed.push(c);
                        // Block 51: the i==63 tail pop is a real
                        // 1-position run — record it so the pixel
                        // layer's colors-stream cursor stays in
                        // lockstep (FFmpeg fills scan position 63
                        // with this color).
                        runs.push(RunItem { run: 1, color: Some(c) });
                    }
                    BlockRecord::Run { pattern, runs }
                }
                bt::RESIDUE => {
                    let x = gv(&mut bundles, B_XOFF)? as i8;
                    let y = gv(&mut bundles, B_YOFF)? as i8;
                    symbols.xoff_consumed.push(x);
                    symbols.yoff_consumed.push(y);
                    let masks = gb.bits(7)?;
                    let mut coeffs = Vec::new();
                    read_residue(&mut gb, masks, &mut coeffs)?;
                    BlockRecord::Residue { x, y, coeffs }
                }
                bt::INTRA => {
                    let dc = gv(&mut bundles, B_INTRADC)? as i16;
                    symbols.intradc_consumed.push(dc);
                    let mut coeffs = Vec::new();
                    let quant = read_dct_coeffs(&mut gb, &mut coeffs)?;
                    BlockRecord::Intra {
                        dc,
                        quant,
                        coeffs,
                    }
                }
                bt::FILL => {
                    let c = gv(&mut bundles, B_COLORS)? as u8;
                    symbols.colors_consumed.push(c);
                    BlockRecord::Fill { color: c }
                }
                bt::INTER => {
                    let x = gv(&mut bundles, B_XOFF)? as i8;
                    let y = gv(&mut bundles, B_YOFF)? as i8;
                    symbols.xoff_consumed.push(x);
                    symbols.yoff_consumed.push(y);
                    let dc = gv(&mut bundles, B_INTERDC)? as i16;
                    symbols.interdc_consumed.push(dc);
                    let mut coeffs = Vec::new();
                    let quant = read_dct_coeffs(&mut gb, &mut coeffs)?;
                    BlockRecord::Inter {
                        x,
                        y,
                        dc,
                        quant,
                        coeffs,
                    }
                }
                bt::PATTERN => {
                    let c0 = gv(&mut bundles, B_COLORS)? as u8;
                    let c1 = gv(&mut bundles, B_COLORS)? as u8;
                    symbols.colors_consumed.push(c0);
                    symbols.colors_consumed.push(c1);
                    let mut patterns = [0u8; 8];
                    for p in patterns.iter_mut() {
                        let v = gv(&mut bundles, B_PATTERN)? as u8;
                        symbols.patterns_consumed.push(v);
                        *p = v;
                    }
                    BlockRecord::Pattern {
                        colors: [c0, c1],
                        patterns,
                    }
                }
                bt::RAW => {
                    let mut colors = [0u8; 64];
                    for c in colors.iter_mut() {
                        let v = gv(&mut bundles, B_COLORS)? as u8;
                        symbols.colors_consumed.push(v);
                        *c = v;
                    }
                    BlockRecord::Raw { colors }
                }
                other => {
                    return Err(P3dError::Other(format!(
                        "binkvideo: unknown block type {other}"
                    )))
                }
            };
            symbols.blocks.push(record);
            if blk == bt::SCALED {
                bx += 1;
            }
        }
    }
    gb.align32()?;
    symbols.end_bit = gb.pos;
    Ok(symbols)
}

/// The symbol walk of one complete frame: planes in BITSTREAM order
/// (0 = luma, then the two chroma planes).  With `swap_planes` (every
/// revision >= 'h' — all of this game's movies) FFmpeg's pixel layer
/// writes bitstream plane 1 into the frame's V plane and plane 2 into
/// U; the pixel layer must route accordingly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameSymbols {
    pub planes: [PlaneSymbols; 3],
}

/// Decode all three planes of one frame's video bitstream (the whole
/// `FrameHead::video` region — the bytes after the audio packets and
/// the rev-`i` skipped word).  The walk must consume the region
/// EXACTLY (proven on 8 real frames): any trailing garbage is
/// rejected, and an overread anywhere fails closed.
pub fn decode_frame_symbols(video: &[u8], width: usize, height: usize) -> Result<FrameSymbols> {
    if video.len() < 4 {
        return Err(P3dError::Other(format!(
            "binkvideo: video region {} B too small for a frame",
            video.len()
        )));
    }
    let mut pos = 0usize;
    let mut planes = Vec::with_capacity(3);
    for is_chroma in [false, true, true] {
        let p = decode_plane_symbols(video, pos, width, height, is_chroma)?;
        pos = p.end_bit;
        planes.push(p);
    }
    let end = planes[2].end_bit;
    if end != video.len() * 8 {
        return Err(P3dError::Other(format!(
            "binkvideo: frame walk ended at bit {end} but the video region has {}",
            video.len() * 8
        )));
    }
    Ok(FrameSymbols {
        planes: planes.try_into().unwrap(),
    })
}

// ---------------------------------------------------------------------------
// Unit tests against REAL frames (block 50)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    /// The intro movie is 1280x544 — the height field at head offset
    /// 24 (block 49 walked every intro frame at the silent movie's
    /// 582 and never converged; 544 closes the residual exactly).
    #[test]
    fn intro_height_is_544() {
        let head = fixture("movies_bik_1e218528_head22980.bin");
        let w = u32::from_le_bytes([head[20], head[21], head[22], head[23]]);
        let h = u32::from_le_bytes([head[24], head[25], head[26], head[27]]);
        assert_eq!((w, h), (1280, 544));
        let head2 = fixture("movies_bik_17a2ee41_head256.bin");
        let w2 = u32::from_le_bytes([head2[20], head2[21], head2[22], head2[23]]);
        let h2 = u32::from_le_bytes([head2[24], head2[25], head2[26], head2[27]]);
        assert_eq!((w2, h2), (1280, 582));
    }

    /// Intro frame 0 — the all-black fade-in (312 B video): three
    /// clean plane walks that consume the region EXACTLY; every block
    /// is SCALED+FILL (constant color), luma 5440 pops = 68 rows x 80.
    #[test]
    fn intro_frame0_symbol_walk_exact() {
        let span = fixture("movies_bik_1e218528_frame0_19324.bin");
        let head = crate::frames::parse_frame_head(&span, 4, &[145_920, 72_960, 72_960, 145_920], b'i')
            .expect("frame head");
        let video = &span[head.video_start..];
        assert_eq!(video.len(), 312);
        let sym = decode_frame_symbols(video, 1280, 544).expect("symbol walk");
        // plane 0: 68 rows x 80 pops, all SCALED+FILL, ending at the
        // 32-bit boundary 1344
        let p0 = &sym.planes[0];
        assert_eq!((p0.bw, p0.bh), (160, 68));
        assert_eq!(p0.blocks.len(), 5440);
        assert!(p0.blocks.iter().all(|b| match b {
            BlockRecord::ScaledSkip => true,
            BlockRecord::Scaled(s) => matches!(s.as_ref(), SubBlock::Fill { .. }),
            _ => false,
        }));
        assert_eq!(p0.end_bit, 1344);
        // chroma: 80x34, 40 pops/row, all SCALED (odd pops are discarded)
        for (i, p) in sym.planes[1..].iter().enumerate() {
            assert_eq!((p.bw, p.bh), (80, 34));
            assert_eq!(p.blocks.len(), 1360);
            let processed = p.blocks.iter().filter(|b| matches!(b, BlockRecord::Scaled(_))).count();
            assert_eq!(processed, 680, "chroma plane {i}: 20 even rows x 34 blocks");
        }
        assert_eq!(sym.planes[1].end_bit, 1920);
        assert_eq!(sym.planes[2].end_bit, 2496);
        // the fade-in color: every PROCESSED fill carries the SAME color
        let colors: Vec<u8> = p0
            .blocks
            .iter()
            .filter_map(|b| match b {
                BlockRecord::Scaled(s) => match s.as_ref() {
                    SubBlock::Fill { color } => Some(*color),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        // 34 even rows x 80 processed blocks each
        assert_eq!(colors.len(), 2720);
        assert!(colors.iter().all(|c| *c == colors[0]));
    }

    /// Intro frame 1 — real content (6 172 B video): the walk consumes
    /// the region exactly; the per-plane pop counts and block-type
    /// histograms match the FFmpeg-verbatim C harness bit-for-bit.
    #[test]
    fn intro_frame1_symbol_walk_exact() {
        let span = fixture("movies_bik_1e218528_frame1_7644.bin");
        let head = crate::frames::parse_frame_head(&span, 4, &[145_920, 72_960, 72_960, 145_920], b'i')
            .expect("frame head");
        let video = &span[head.video_start..];
        assert_eq!(video.len(), 6_172);
        let sym = decode_frame_symbols(video, 1280, 544).expect("symbol walk");
        let p0 = &sym.planes[0];
        assert_eq!(p0.blocks.len(), 8604);
        let mut hist = [0usize; 16];
        for b in &p0.blocks {
            let t = match b {
                BlockRecord::Skip => 0,
                BlockRecord::ScaledSkip | BlockRecord::Scaled(_) => 1,
                BlockRecord::Motion { .. } => 2,
                BlockRecord::Run { .. } => 3,
                BlockRecord::Residue { .. } => 4,
                BlockRecord::Intra { .. } => 5,
                BlockRecord::Fill { .. } => 6,
                BlockRecord::Inter { .. } => 7,
                BlockRecord::Pattern { .. } => 8,
                BlockRecord::Raw { .. } => 9,
            };
            hist[t] += 1;
        }
        assert_eq!(
            hist,
            [5699, 2276, 0, 0, 0, 173, 301, 1, 154, 0, 0, 0, 0, 0, 0, 0],
            "plane 0 block-type histogram must equal the harness's"
        );
        assert_eq!(p0.end_bit, 48_800);
        // chroma planes: all-skip (2720 pops each), plane 1 ends at
        // 49088 and plane 2 at 49376 (= the region end, 6172*8)
        assert_eq!(sym.planes[1].end_bit, 49_088);
        assert_eq!(sym.planes[2].end_bit, 49_376);
        for p in &sym.planes[1..] {
            assert_eq!((p.bw, p.bh), (80, 34));
            assert_eq!(p.blocks.len(), 2720);
            assert!(p.blocks.iter().all(|b| matches!(b, BlockRecord::Skip)));
        }
        // the single INTER block carries its motion + DC + coefficients
        let inter: Vec<&BlockRecord> =
            p0.blocks.iter().filter(|b| matches!(b, BlockRecord::Inter { .. })).collect();
        assert_eq!(inter.len(), 1);
    }

    /// Silent movie frame 0 — a real keyframe with INTRA blocks
    /// (48 928 B video, 1280x582): exact consumption, 7912 luma pops
    /// (+3768 scaled = 73x160 cols), DCT blocks carry real
    /// coefficients and quant indices.
    #[test]
    fn silent_frame0_symbol_walk_exact() {
        let span = fixture("movies_bik_17a2ee41_frame0_48932.bin");
        let head = crate::frames::parse_frame_head(&span, 0, &[], b'i').expect("frame head");
        let video = &span[head.video_start..];
        assert_eq!(video.len(), 48_928);
        let sym = decode_frame_symbols(video, 1280, 582).expect("symbol walk");
        let p0 = &sym.planes[0];
        assert_eq!((p0.bw, p0.bh), (160, 73));
        let scaled = p0
            .blocks
            .iter()
            .filter(|b| matches!(b, BlockRecord::ScaledSkip | BlockRecord::Scaled(_)))
            .count();
        assert_eq!((p0.blocks.len(), scaled), (7912, 3768));
        assert_eq!(p0.blocks.len() + scaled, 11680); // the cols arithmetic closes
        let intra: Vec<&BlockRecord> =
            p0.blocks.iter().filter(|b| matches!(b, BlockRecord::Intra { .. })).collect();
        assert_eq!(intra.len(), 3509);
        // every INTRA block has a quant index and a DC; most have coefficients
        let with_coeffs = intra
            .iter()
            .filter(|b| match b {
                BlockRecord::Intra { coeffs, .. } => !coeffs.is_empty(),
                _ => false,
            })
            .count();
        assert!(with_coeffs > 3000, "real INTRA blocks carry coefficients");
        assert_eq!(p0.end_bit, 357_664);
        assert_eq!(sym.planes[1].end_bit, 373_216);
        assert_eq!(sym.planes[2].end_bit, 391_424);
    }

    /// Silent frame 47 — the 260 B all-skip tail: bundles die at the
    /// first row refill (t=0), all pops come from the zero-buffer as
    /// SKIP, and the walk still consumes the region exactly.
    #[test]
    fn silent_frame47_all_skip_tail() {
        let span = fixture("movies_bik_17a2ee41_frame47_264.bin");
        let head = crate::frames::parse_frame_head(&span, 0, &[], b'i').expect("frame head");
        let video = &span[head.video_start..];
        assert_eq!(video.len(), 260);
        let sym = decode_frame_symbols(video, 1280, 582).expect("symbol walk");
        let p0 = &sym.planes[0];
        assert_eq!(p0.blocks.len(), 11_632);
        let skips = p0.blocks.iter().filter(|b| matches!(b, BlockRecord::Skip)).count();
        assert_eq!(skips, 11_570);
        assert_eq!(p0.end_bit, 1024);
        assert_eq!(sym.planes[2].end_bit, 2080);
    }

    /// The cross-layer path: BikIndex::frame_head -> video region ->
    /// symbol walk, on the silent movie's committed head fixture.
    #[test]
    fn silent_head_to_symbols() {
        let head = fixture("movies_bik_17a2ee41_head256.bin");
        let idx = crate::bik::BikIndex::parse_head(&head, 1_603_968).expect("head");
        let span = fixture("movies_bik_17a2ee41_frame0_48932.bin");
        let fh = idx.frame_head(0, &span).expect("frame head");
        let video = &span[fh.video_start..];
        let sym = decode_frame_symbols(video, 1280, 582).expect("walk");
        assert_eq!(sym.planes[2].end_bit, video.len() * 8);
    }

    /// Fail-closed: a corrupted block type is rejected.
    #[test]
    fn corrupted_block_type_rejected() {
        let span = fixture("movies_bik_1e218528_frame0_19324.bin");
        let head = crate::frames::parse_frame_head(&span, 4, &[145_920, 72_960, 72_960, 145_920], b'i')
            .unwrap();
        let start = head.video_start;
        let mut video = span[start..].to_vec();
        // the bt constant-fill value lives inside the first refill;
        // flip a byte inside the plane-0 body (bit 200 region)
        video[25] ^= 0xFF;
        assert!(decode_frame_symbols(&video, 1280, 544).is_err());
    }

    /// Fail-closed: truncation EOF.
    #[test]
    fn truncated_video_rejected() {
        let span = fixture("movies_bik_1e218528_frame1_7644.bin");
        let head = crate::frames::parse_frame_head(&span, 4, &[145_920, 72_960, 72_960, 145_920], b'i')
            .unwrap();
        let start = head.video_start;
        let video = &span[start..start + 3000];
        assert!(decode_frame_symbols(video, 1280, 544).is_err());
    }

    /// Fail-closed: the exact-consumption contract — one extra byte
    /// appended to the video region makes the final end-bit check fail.
    #[test]
    fn trailing_garbage_rejected() {
        let span = fixture("movies_bik_1e218528_frame0_19324.bin");
        let head = crate::frames::parse_frame_head(&span, 4, &[145_920, 72_960, 72_960, 145_920], b'i')
            .unwrap();
        let start = head.video_start;
        let mut video = span[start..].to_vec();
        video.push(0);
        let err = decode_frame_symbols(&video, 1280, 544).unwrap_err();
        assert!(format!("{err}").contains("ended at bit"), "{err}");
    }
}

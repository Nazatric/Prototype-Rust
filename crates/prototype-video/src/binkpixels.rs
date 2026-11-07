//! Bink VIDEO pixel layer — the codec's second real decoder layer.
//!
//! Renders the per-block symbol records captured by
//! [`crate::binkvideo`] into actual YUV planes, with arithmetic that
//! mirrors FFmpeg's C exactly (wrapping i32/u8 semantics, arithmetic
//! shifts, the implicit truncating u8 stores) so decoded frames are
//! byte-identical to `ffmpeg -pix_fmt yuv420p -f rawvideo` output.
//!
//! ## Reference (block 51, fetched to `scripts/ffmpeg_src/`)
//!
//! * `libavcodec/bink.c` — `bink_decode_plane`'s block dispatch
//!   (SKIP/SCALED/MOTION/RUN/RESIDUE/INTRA/FILL/INTER/PATTERN/RAW),
//!   `bink_put_pixels`' bounds gate, the prev-frame rules
//!   (`prev = last ? last : frame` — frame 0 references its own
//!   zeroed plane), the plane routing (`swap_planes` routes bitstream
//!   plane 1 → the frame's V plane and plane 2 → U) and the 32-bit
//!   per-plane tail align (already in the symbol walk).
//! * `libavcodec/binkdsp.c` — `bink_idct_put_c` / `bink_idct_add_c`
//!   (column pass without rounding + row pass with `((x+0x7F)>>8)`,
//!   u8 stores TRUNCATE), `scale_block` (2x2 replication),
//!   `add_pixels8` (wrapping u8 += i16).
//! * `libavcodec/blockdsp.c` — `fill_block_tab` (memset rows).
//! * `libavcodec/hpeldsp.c` — `put_pixels_tab[1][0]` = per-row 8-byte
//!   copy (load-then-store per row; row i is stored before row i+1 is
//!   loaded, which matters for the frame-0 self-reference case).
//! * `libavcodec/binkdata.h` — `bink_scan`, `bink_patterns`,
//!   `bink_intra_quant`, `bink_inter_quant` (mechanically transcribed
//!   into [`crate::binkdata`] by `scripts/m51_binkdata.py`).
//!
//! ## Semantics pinned here (all FFmpeg-verbatim)
//!
//! * DCT blocks: `block[0] = dc`; each captured coefficient is placed
//!   at `block[scan[ccoef]]`; then unquantize: `block[0] =
//!   block[0]*quant[0] >> 11` and `block[scan[idx]] =
//!   block[scan[idx]]*quant[idx] >> 11` (idx = the scan position; the
//!   tables are stored in scan order).  INTRA renders with
//!   `idct_put`, INTER first copies the motion block then `idct_add`.
//! * Residue blocks: the captured coefficient stream replays
//!   FFmpeg's `read_residue` write order — the FIRST write to a
//!   raster position is the direct `block[pos] = ±mask` set, every
//!   later write is a pyramid re-check (`block[pos] -= mask` when
//!   negative else `+= mask`; the captured value is the positive
//!   mask).  A position can only ever be direct-set once, so the
//!   first-occurrence rule reconstructs the block exactly.
//! * Motion copies: `ref = prev + xoff + yoff*stride` must lie in
//!   `[plane_start, ref_end]` where
//!   `ref_end = ((bw-1) + stride*(bh-1)) * 8` — otherwise the frame
//!   fails closed (FFmpeg returns `AVERROR_INVALIDDATA`).
//! * SCALED 16x16 blocks: the 8x8 sub-block payload (any type except
//!   FILL, which fills 16x16 directly) is expanded 2x2 by
//!   `scale_block`; a SCALED pop on an odd row/column is discarded
//!   (the capture already emits `ScaledSkip` for it).
//! * RUN blocks consume their per-position colors from the captured
//!   `colors_consumed` stream in exact pop order (an inlined
//!   single-color run skips the stream's copy of that color; a
//!   per-position run takes its colors from the stream); every other
//!   record advances the cursor past its own inlined pops.  The
//!   renderer fails closed if the cursor does not land exactly on the
//!   stream end — a capture/render disagreement.
//! * Frame buffers are zero-initialized with 16 spare rows (FFmpeg's
//!   aligned-height padding; only ever WRITTEN there — motion
//!   references are bounded by the block grid and never read it).

use crate::binkdata::{BINK_INTER_QUANT, BINK_INTRA_QUANT, BINK_PATTERNS, BINK_SCAN};
use crate::binkvideo::{decode_frame_symbols, BlockRecord, DctCoef, PlaneSymbols, RunItem, SubBlock};
use prototype_common::{P3dError, Result};

// ---------------------------------------------------------------------------
// FFmpeg-exact primitives (binkdsp.c / blockdsp.c / hpeldsp.c)
// ---------------------------------------------------------------------------

/// `MUL(X, Y)` — `(int)((unsigned)(X) * (Y)) >> 11` (binkdsp.c).
#[inline]
fn mul11(x: i32, y: i32) -> i32 {
    ((x as u32).wrapping_mul(y as u32) as i32) >> 11
}

const A1: i32 = 2896; // (1/sqrt(2))<<12
const A2: i32 = 2217;
const A3: i32 = 3784;
const A4: i32 = -5352;

/// One column pass of the Bink IDCT (`IDCT_COL`): reads the block's
/// column `i` (stride 8) into `temp`'s column `i`, no rounding.
/// FFmpeg's all-AC-zero fast path is replicated.
fn idct_col_pass(temp: &mut [i32; 64], block: &[i32; 64]) {
    for i in 0..8 {
        let s = |k: usize| block[i + k * 8];
        if s(1) | s(2) | s(3) | s(4) | s(5) | s(6) | s(7) == 0 {
            for k in 0..8 {
                temp[i + k * 8] = s(0);
            }
        } else {
            let a0 = s(0).wrapping_add(s(4));
            let a1 = s(0).wrapping_sub(s(4));
            let a2 = s(2).wrapping_add(s(6));
            let a3 = mul11(A1, s(2).wrapping_sub(s(6)));
            let a4 = s(5).wrapping_add(s(3));
            let a5 = s(5).wrapping_sub(s(3));
            let a6 = s(1).wrapping_add(s(7));
            let a7 = s(1).wrapping_sub(s(7));
            let b0 = a4.wrapping_add(a6);
            let b1 = mul11(A3, a5.wrapping_add(a7));
            let b2 = mul11(A4, a5).wrapping_sub(b0).wrapping_add(b1);
            let b3 = mul11(A1, a6.wrapping_sub(a4)).wrapping_sub(b2);
            let b4 = mul11(A2, a7).wrapping_add(b3).wrapping_sub(b1);
            temp[i] = a0.wrapping_add(a2).wrapping_add(b0);
            temp[i + 8] = a1.wrapping_add(a3).wrapping_sub(a2).wrapping_add(b2);
            temp[i + 16] = a1.wrapping_sub(a3).wrapping_add(a2).wrapping_add(b3);
            temp[i + 24] = a0.wrapping_sub(a2).wrapping_sub(b4);
            temp[i + 32] = a0.wrapping_sub(a2).wrapping_add(b4);
            temp[i + 40] = a1.wrapping_sub(a3).wrapping_add(a2).wrapping_sub(b3);
            temp[i + 48] = a1.wrapping_add(a3).wrapping_sub(a2).wrapping_sub(b2);
            temp[i + 56] = a0.wrapping_add(a2).wrapping_sub(b0);
        }
    }
}

/// One row pass with `MUNGE_ROW(x) = ((x) + 0x7F) >> 8` applied to the
/// eight outputs (the caller stores them — into u8 with truncation for
/// `idct_put`, into i32 for `bink_idct_add`'s block).
fn idct_row_pass(temp: &[i32; 64], r: usize) -> [i32; 8] {
    let s = |k: usize| temp[r * 8 + k];
    let a0 = s(0).wrapping_add(s(4));
    let a1 = s(0).wrapping_sub(s(4));
    let a2 = s(2).wrapping_add(s(6));
    let a3 = mul11(A1, s(2).wrapping_sub(s(6)));
    let a4 = s(5).wrapping_add(s(3));
    let a5 = s(5).wrapping_sub(s(3));
    let a6 = s(1).wrapping_add(s(7));
    let a7 = s(1).wrapping_sub(s(7));
    let b0 = a4.wrapping_add(a6);
    let b1 = mul11(A3, a5.wrapping_add(a7));
    let b2 = mul11(A4, a5).wrapping_sub(b0).wrapping_add(b1);
    let b3 = mul11(A1, a6.wrapping_sub(a4)).wrapping_sub(b2);
    let b4 = mul11(A2, a7).wrapping_add(b3).wrapping_sub(b1);
    let m = |x: i32| x.wrapping_add(0x7F) >> 8;
    [
        m(a0.wrapping_add(a2).wrapping_add(b0)),
        m(a1.wrapping_add(a3).wrapping_sub(a2).wrapping_add(b2)),
        m(a1.wrapping_sub(a3).wrapping_add(a2).wrapping_add(b3)),
        m(a0.wrapping_sub(a2).wrapping_sub(b4)),
        m(a0.wrapping_sub(a2).wrapping_add(b4)),
        m(a1.wrapping_sub(a3).wrapping_add(a2).wrapping_sub(b3)),
        m(a1.wrapping_add(a3).wrapping_sub(a2).wrapping_sub(b2)),
        m(a0.wrapping_add(a2).wrapping_sub(b0)),
    ]
}

/// `bink_idct_put_c` — IDCT straight into the u8 plane (stores
/// truncate mod 256, exactly like C's int→uint8 assignment).
fn idct_put(dest: &mut [u8], off: usize, stride: usize, block: &[i32; 64]) {
    let mut temp = [0i32; 64];
    idct_col_pass(&mut temp, block);
    for r in 0..8 {
        let row = idct_row_pass(&temp, r);
        for (j, v) in row.iter().enumerate() {
            dest[off + r * stride + j] = *v as u8;
        }
    }
}

/// `bink_idct_add_c` — full IDCT into the i32 block (row pass keeps
/// the munged ints, no truncation), then wrapping u8 add.
fn idct_add(dest: &mut [u8], off: usize, stride: usize, block: &mut [i32; 64]) {
    let mut temp = [0i32; 64];
    idct_col_pass(&mut temp, block);
    for r in 0..8 {
        let row = idct_row_pass(&temp, r);
        block[r * 8..r * 8 + 8].copy_from_slice(&row);
    }
    for i in 0..8 {
        for j in 0..8 {
            let p = off + i * stride + j;
            dest[p] = dest[p].wrapping_add(block[i * 8 + j] as u8);
        }
    }
}

/// `scale_block_c` — replicate every source byte 2x2 into a 16x16
/// plane block (FFmpeg's uint16 `0x0101` writes, little-endian).
fn scale_block(src: &[u8; 64], dest: &mut [u8], off: usize, stride: usize) {
    for j in 0..8 {
        for i in 0..8 {
            let v = src[j * 8 + i];
            let r0 = off + (2 * j) * stride + 2 * i;
            let r1 = r0 + stride;
            dest[r0] = v;
            dest[r0 + 1] = v;
            dest[r1] = v;
            dest[r1 + 1] = v;
        }
    }
}

/// `add_pixels8_c` — wrapping `u8 += i16` over an 8x8 plane block.
fn add_pixels8(dest: &mut [u8], off: usize, stride: usize, block: &[i16; 64]) {
    for i in 0..8 {
        for j in 0..8 {
            let p = off + i * stride + j;
            dest[p] = dest[p].wrapping_add(block[i * 8 + j] as u8);
        }
    }
}

/// `put_pixels_tab[1][0]` (hpeldsp `put_pixels8_c`) — per-row
/// load-then-store of 8 bytes from a DISJOINT buffer (the previous
/// frame's plane).
fn copy_block8_from(dest: &mut [u8], dst_off: usize, src: &[u8], src_off: usize, stride: usize) {
    for i in 0..8 {
        let mut row = [0u8; 8];
        row.copy_from_slice(&src[src_off + i * stride..src_off + i * stride + 8]);
        dest[dst_off + i * stride..dst_off + i * stride + 8].copy_from_slice(&row);
    }
}

/// Same-row-buffer variant for frame 0 (`prev == frame`): reads each
/// row before writing it (FFmpeg's row-ordered copy semantics).
fn copy_block8_within(dest: &mut [u8], dst_off: usize, src_off: usize, stride: usize) {
    for i in 0..8 {
        let mut row = [0u8; 8];
        row.copy_from_slice(&dest[src_off + i * stride..src_off + i * stride + 8]);
        dest[dst_off + i * stride..dst_off + i * stride + 8].copy_from_slice(&row);
    }
}

/// `fill_block_tab[0]/[1]` (blockdsp) — memset `w`-byte rows.
fn fill_block(dest: &mut [u8], off: usize, stride: usize, v: u8, w: usize, h: usize) {
    for i in 0..h {
        dest[off + i * stride..off + i * stride + w].fill(v);
    }
}

// ---------------------------------------------------------------------------
// DCT unquantize + residue replay
// ---------------------------------------------------------------------------

/// Unquantize one DCT block the way `unquantize_dct_coeffs` does:
/// `block[0] = block[0]*quant[0] >> 11`, then every captured
/// coefficient position `block[scan[idx]] = block[scan[idx]]*quant[idx] >> 11`
/// in stream order (the multiplication is C's int32 wrap-around).
fn render_dct_block(
    dc: i16,
    quant_idx: u8,
    coeffs: &[DctCoef],
    table: &[[i32; 64]; 16],
) -> [i32; 64] {
    let mut block = [0i32; 64];
    block[0] = dc as i32;
    for c in coeffs {
        block[BINK_SCAN[c.ccoef as usize] as usize] = c.value;
    }
    let q = &table[quant_idx as usize];
    block[0] = block[0].wrapping_mul(q[0]) >> 11;
    for c in coeffs {
        let pos = BINK_SCAN[c.ccoef as usize] as usize;
        block[pos] = block[pos].wrapping_mul(q[c.ccoef as usize]) >> 11;
    }
    block
}

/// Replay a captured residue stream into the zeroed int16 block.
/// First write to a position = the direct set (FFmpeg's
/// `block[bink_scan[ccoef]] = (mask ^ sign) - sign`); every later
/// write to the same position = a pyramid re-check
/// (`block[pos] -= mask` if negative else `+= mask`, captured value
/// is the positive mask).  A position is direct-set exactly once in
/// FFmpeg's `nz_coeff` discipline, so first-occurrence is exact.
fn replay_residue(coeffs: &[DctCoef]) -> [i16; 64] {
    let mut block = [0i16; 64];
    let mut seen = [false; 64];
    for c in coeffs {
        let pos = c.ccoef as usize;
        if !seen[pos] {
            seen[pos] = true;
            block[pos] = c.value as i16;
        } else if block[pos] < 0 {
            block[pos] = block[pos].wrapping_sub(c.value as i16);
        } else {
            block[pos] = block[pos].wrapping_add(c.value as i16);
        }
    }
    block
}

// ---------------------------------------------------------------------------
// Plane renderer
// ---------------------------------------------------------------------------

/// How many `colors_consumed` pops one record accounts for (the
/// capture pushes EVERY pop into the stream, inlined ones included).
pub fn colors_contribution(r: &BlockRecord) -> usize {
    match r {
        BlockRecord::Fill { .. } => 1,
        BlockRecord::Pattern { .. } => 2,
        BlockRecord::Raw { .. } => 64,
        BlockRecord::Run { runs, .. } => run_pops(runs),
        BlockRecord::Scaled(sub) => match sub.as_ref() {
            SubBlock::Fill { .. } => 1,
            SubBlock::Pattern { .. } => 2,
            SubBlock::Raw { .. } => 64,
            SubBlock::Run { runs, .. } => run_pops(runs),
            SubBlock::Intra { .. } => 0,
        },
        _ => 0,
    }
}

fn run_pops(runs: &[RunItem]) -> usize {
    runs.iter()
        .map(|ri| if ri.color.is_some() { 1 } else { ri.run as usize })
        .sum()
}

/// Render one plane's symbol records into `dst` (row stride `stride`,
/// `bw`x`bh` block grid).  `prev` is the previous frame's plane, or
/// `None` for the first frame (FFmpeg: `prev = last ? last : frame` —
/// motion references then read the CURRENT plane, zero-initialized).
pub fn render_plane(
    sym: &PlaneSymbols,
    dst: &mut [u8],
    prev: Option<&[u8]>,
    stride: usize,
    bw: usize,
    bh: usize,
) -> Result<()> {
    // FFmpeg's gate (bink.c): `ref_end = ref_start +
    // (bw - 1 + linesize * (bh - 1)) * 8` — the *8 scales the WHOLE
    // sum, making it the tightest byte bound under which an 8x8 read
    // from `ref` never leaves the bh*8-row block-grid plane
    // (ref_end + 7*stride + 7 == bh*8*stride - 1 when stride == bw*8).
    let ref_end: i64 = ((bw - 1 + stride * (bh - 1)) * 8) as i64;
    let mut cc = 0usize; // cursor into colors_consumed
    let mut bi = 0usize; // cursor into blocks
    let colors = &sym.colors_consumed;

    // Resolve a RUN record's colors: (positions, color) pairs in scan
    // order — an inlined single-color run covers `run` positions with
    // one color (and skips the stream's copy of it); a per-position
    // run takes `run` colors from the stream cursor.
    fn resolve_runs(runs: &[RunItem], colors: &[u8], cc: &mut usize) -> Result<Vec<(usize, u8)>> {
        let mut out = Vec::new();
        for ri in runs {
            match ri.color {
                Some(c) => {
                    out.push((ri.run as usize, c));
                    *cc += 1; // skip the stream's copy of the inlined color
                }
                None => {
                    let end = *cc + ri.run as usize;
                    if end > colors.len() {
                        return Err(P3dError::Other(
                            "binkpixels: run colors exceed the captured colors stream".into(),
                        ));
                    }
                    for &c in &colors[*cc..end] {
                        out.push((1, c));
                    }
                    *cc = end;
                }
            }
        }
        Ok(out)
    }

    let mut by = 0usize;
    while by < bh {
        let mut bx = 0usize;
        while bx < bw {
            let record = sym.blocks.get(bi).ok_or_else(|| {
                P3dError::Other(format!(
                    "binkpixels: plane records exhausted at block ({bx},{by})"
                ))
            })?;
            bi += 1;
            // Clone so `sym` stays immutably borrowed while `dst` is
            // mutably written below.
            let record = record.clone();
            let off = by * stride * 8 + bx * 8;
            let mut scaled = false;

            match &record {
                // A 16x16 block type on an odd row/column is part of
                // the already-decoded even-even block: the pop is
                // discarded (FFmpeg just advances two columns — the
                // extra bx step below applies here too).
                BlockRecord::ScaledSkip => {
                    scaled = true;
                }
                BlockRecord::Skip => match prev {
                    Some(p) => copy_block8_from(dst, off, p, off, stride),
                    None => copy_block8_within(dst, off, off, stride),
                },
                BlockRecord::Motion { x, y }
                | BlockRecord::Residue { x, y, .. }
                | BlockRecord::Inter { x, y, .. } => {
                    // bink_put_pixels: bounds gate, then the copy.
                    let ref_off = off as i64 + *x as i64 + *y as i64 * stride as i64;
                    if ref_off < 0 || ref_off > ref_end {
                        return Err(P3dError::Other(format!(
                            "binkpixels: copy out of bounds @({bx},{by}) xoff {x} yoff {y} ref {ref_off} end {ref_end}"
                        )));
                    }
                    let ref_off = ref_off as usize;
                    match prev {
                        Some(p) => copy_block8_from(dst, off, p, ref_off, stride),
                        None => copy_block8_within(dst, off, ref_off, stride),
                    }
                    if let BlockRecord::Residue { coeffs, .. } = &record {
                        let block = replay_residue(coeffs);
                        add_pixels8(dst, off, stride, &block);
                    } else if let BlockRecord::Inter { dc, quant, coeffs, .. } = &record {
                        let mut block = render_dct_block(*dc, *quant, coeffs, &BINK_INTER_QUANT);
                        idct_add(dst, off, stride, &mut block);
                    }
                }
                BlockRecord::Intra { dc, quant, coeffs } => {
                    let block = render_dct_block(*dc, *quant, coeffs, &BINK_INTRA_QUANT);
                    idct_put(dst, off, stride, &block);
                }
                BlockRecord::Fill { color } => {
                    fill_block(dst, off, stride, *color, 8, 8);
                }
                BlockRecord::Pattern {
                    colors: cols,
                    patterns,
                } => {
                    for (i, &pv) in patterns.iter().enumerate() {
                        let row = off + i * stride;
                        for j in 0..8 {
                            dst[row + j] = cols[((pv >> j) & 1) as usize];
                        }
                    }
                }
                BlockRecord::Raw { colors: cols } => {
                    for i in 0..8 {
                        dst[off + i * stride..off + i * stride + 8]
                            .copy_from_slice(&cols[i * 8..i * 8 + 8]);
                    }
                }
                BlockRecord::Run { pattern, runs } => {
                    let pat = &BINK_PATTERNS[*pattern as usize];
                    let resolved = resolve_runs(runs, colors, &mut cc)?;
                    let mut pos_idx = 0usize;
                    for (count, c) in &resolved {
                        for _ in 0..*count {
                            let pos = pat[pos_idx] as usize;
                            pos_idx += 1;
                            dst[off + (pos >> 3) * stride + (pos & 7)] = *c;
                        }
                    }
                }
                BlockRecord::Scaled(sub) => {
                    scaled = true;
                    match sub.as_ref() {
                        SubBlock::Fill { color } => {
                            // The one SCALED subtype that fills 16x16
                            // directly (no ublock, no scale_block).
                            fill_block(dst, off, stride, *color, 16, 16);
                        }
                        SubBlock::Run { pattern, runs } => {
                            let mut ublock = [0u8; 64];
                            let pat = &BINK_PATTERNS[*pattern as usize];
                            let resolved = resolve_runs(runs, colors, &mut cc)?;
                            let mut pos_idx = 0usize;
                            for (count, c) in &resolved {
                                for _ in 0..*count {
                                    ublock[pat[pos_idx] as usize] = *c;
                                    pos_idx += 1;
                                }
                            }
                            scale_block(&ublock, dst, off, stride);
                        }
                        SubBlock::Intra { dc, quant, coeffs } => {
                            let mut ublock = [0u8; 64];
                            let block = render_dct_block(*dc, *quant, coeffs, &BINK_INTRA_QUANT);
                            idct_put(&mut ublock, 0, 8, &block);
                            scale_block(&ublock, dst, off, stride);
                        }
                        SubBlock::Pattern {
                            colors: cols,
                            patterns,
                        } => {
                            let mut ublock = [0u8; 64];
                            for (j, &pv) in patterns.iter().enumerate() {
                                for i in 0..8 {
                                    ublock[j * 8 + i] = cols[((pv >> i) & 1) as usize];
                                }
                            }
                            scale_block(&ublock, dst, off, stride);
                        }
                        SubBlock::Raw { colors: cols } => {
                            scale_block(cols, dst, off, stride);
                        }
                    }
                }
            }

            // Records whose colors went through resolve_runs already
            // advanced the cursor; every other record must skip past
            // its own inlined pops (the capture pushes them all).
            let run_handled = match &record {
                BlockRecord::Run { .. } => true,
                BlockRecord::Scaled(sub) => matches!(sub.as_ref(), SubBlock::Run { .. }),
                _ => false,
            };
            if !run_handled {
                cc += colors_contribution(&record);
            }

            if scaled {
                bx += 1; // SCALED occupies two block columns
            }
            bx += 1;
        }
        by += 1;
    }

    if bi != sym.blocks.len() {
        return Err(P3dError::Other(format!(
            "binkpixels: {} of {} plane records rendered — grid mismatch",
            bi,
            sym.blocks.len()
        )));
    }
    if cc != colors.len() {
        return Err(P3dError::Other(format!(
            "binkpixels: colors stream cursor {cc} != {} captured pops",
            colors.len()
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Frame decoder
// ---------------------------------------------------------------------------

/// The decoded planes of one frame (block-grid-sized buffers; only
/// the visible `width`x`height` region is meaningful).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinkPlanes {
    pub width: u32,
    pub height: u32,
    /// Luma plane: `stride_y * (bh_luma*8 + 16)` zero-padded rows.
    pub y: Vec<u8>,
    /// U (Cb) plane — bitstream plane 2 when `swap_planes` is set.
    pub u: Vec<u8>,
    /// V (Cr) plane — bitstream plane 1 when `swap_planes` is set.
    pub v: Vec<u8>,
    pub stride_y: usize,
    pub stride_c: usize,
}

impl BinkPlanes {
    /// The visible-region YUV420p bytes (w*h luma + 2 * (w/2)*(h/2)
    /// chroma) — the layout `ffmpeg -pix_fmt yuv420p -f rawvideo`
    /// writes; both observed movies have even dims.
    pub fn yuv420_visible(&self) -> Result<Vec<u8>> {
        let w = self.width as usize;
        let h = self.height as usize;
        if w % 2 != 0 || h % 2 != 0 {
            return Err(P3dError::Other(format!(
                "binkpixels: odd dims {w}x{h} — visible YUV420 extraction not defined"
            )));
        }
        let mut out = Vec::with_capacity(w * h * 3 / 2);
        for r in 0..h {
            out.extend_from_slice(&self.y[r * self.stride_y..r * self.stride_y + w]);
        }
        let (cw, ch) = (w / 2, h / 2);
        for plane in [&self.u, &self.v] {
            for r in 0..ch {
                out.extend_from_slice(&plane[r * self.stride_c..r * self.stride_c + cw]);
            }
        }
        Ok(out)
    }
}

/// A stateful Bink VIDEO frame decoder: the pixel counterpart of
/// [`crate::binkvideo::decode_frame_symbols`].  Holds the previous
/// frame (FFmpeg's `c->last`) so SKIP/MOTION/RESIDUE/INTER blocks
/// reference it; frame 0 references its own zeroed plane.
pub struct BinkVideoDecoder {
    width: u32,
    height: u32,
    /// `version >= 'h'` — every movie in this game ('i') sets it:
    /// bitstream plane 1 lands in the frame's V plane, plane 2 in U.
    swap_planes: bool,
    last: Option<BinkPlanes>,
}

impl BinkVideoDecoder {
    pub fn new(width: u32, height: u32, swap_planes: bool) -> Result<Self> {
        if width == 0 || height == 0 || width > 4096 || height > 4096 {
            return Err(P3dError::Other(format!(
                "binkpixels: bad frame dims {width}x{height}"
            )));
        }
        Ok(Self {
            width,
            height,
            swap_planes,
            last: None,
        })
    }

    pub fn last(&self) -> Option<&BinkPlanes> {
        self.last.as_ref()
    }

    /// Decode one frame's video bitstream (the region after the audio
    /// packets and the rev-`i` skipped word — exactly what
    /// [`crate::binkvideo::decode_frame_symbols`] consumes).  The
    /// decoded planes become the next frame's `last`.
    pub fn decode_frame(&mut self, video: &[u8]) -> Result<()> {
        let (w, h) = (self.width as usize, self.height as usize);
        let symbols = decode_frame_symbols(video, w, h)?;

        let stride_y = ((w + 7) >> 3) << 3;
        let rows_y = (((h + 7) >> 3) << 3) + 16; // aligned-height padding
        let stride_c = ((w + 15) >> 4) << 3;
        let rows_c = (((h + 15) >> 4) << 3) + 16;
        let (bw_l, bh_l) = ((w + 7) >> 3, (h + 7) >> 3);
        let (bw_c, bh_c) = ((w + 15) >> 4, (h + 15) >> 4);

        let mut y = vec![0u8; stride_y * rows_y];
        let mut u = vec![0u8; stride_c * rows_c];
        let mut v = vec![0u8; stride_c * rows_c];

        let prev_y = self.last.as_ref().map(|p| p.y.as_slice());
        let prev_u = self.last.as_ref().map(|p| p.u.as_slice());
        let prev_v = self.last.as_ref().map(|p| p.v.as_slice());

        // Bitstream plane 0 is always luma.  With swap_planes
        // (revision >= 'h') bitstream plane 1 → V and plane 2 → U.
        render_plane(&symbols.planes[0], &mut y, prev_y, stride_y, bw_l, bh_l)?;
        if self.swap_planes {
            render_plane(&symbols.planes[1], &mut v, prev_v, stride_c, bw_c, bh_c)?;
            render_plane(&symbols.planes[2], &mut u, prev_u, stride_c, bw_c, bh_c)?;
        } else {
            render_plane(&symbols.planes[1], &mut u, prev_u, stride_c, bw_c, bh_c)?;
            render_plane(&symbols.planes[2], &mut v, prev_v, stride_c, bw_c, bh_c)?;
        }

        self.last = Some(BinkPlanes {
            width: self.width,
            height: self.height,
            y,
            u,
            v,
            stride_y,
            stride_c,
        });
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Unit tests — primitives + REAL fixtures vs real static-ffmpeg renders
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

    // -- primitives (FFmpeg-exact arithmetic) -------------------------------

    /// DC-only block: the column fast path replicates the DC, the row
    /// pass spreads it — every pixel is `((dc + 0x7F) >> 8) & 0xFF`.
    /// 32768 renders as 128; 65536 renders as 0 (the C u8 store
    /// TRUNCATES — no clamping — replicated exactly).
    #[test]
    fn idct_put_dc_only_is_constant_and_truncates() {
        let mut dst = [0u8; 64];
        idct_put(&mut dst, 0, 8, &[32768, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(dst.iter().all(|&v| v == 128), "got {:?}", &dst[..8]);
        let mut dst2 = [0xAAu8; 64];
        idct_put(&mut dst2, 0, 8, &[65536, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(dst2.iter().all(|&v| v == 0), "truncation: got {:?}", &dst2[..8]);
    }

    /// `bink_idct_add_c`: the munged row values are added onto the
    /// plane with wrapping u8 arithmetic.
    #[test]
    fn idct_add_accumulates_onto_the_plane() {
        let mut dst = [5u8; 64];
        let mut block = [0i32; 64];
        block[0] = 32768;
        idct_add(&mut dst, 0, 8, &mut block);
        assert!(dst.iter().all(|&v| v == 133));
        // wrapping: 250 + 128 = 378 mod 256 = 122
        let mut dst2 = [250u8; 64];
        let mut block2 = [0i32; 64];
        block2[0] = 32768;
        idct_add(&mut dst2, 0, 8, &mut block2);
        assert!(dst2.iter().all(|&v| v == 122));
    }

    /// `scale_block_c`: every source byte replicated 2x2.
    #[test]
    fn scale_block_replicates_2x2() {
        let src: [u8; 64] = std::array::from_fn(|i| i as u8);
        let mut dst = [0u8; 16 * 16];
        scale_block(&src, &mut dst, 0, 16);
        for j in 0..8 {
            for i in 0..8 {
                let v = src[j * 8 + i];
                assert_eq!(dst[(2 * j) * 16 + 2 * i], v);
                assert_eq!(dst[(2 * j) * 16 + 2 * i + 1], v);
                assert_eq!(dst[(2 * j + 1) * 16 + 2 * i], v);
                assert_eq!(dst[(2 * j + 1) * 16 + 2 * i + 1], v);
            }
        }
    }

    /// The residue replay: first write to a position SETS it, later
    /// writes are pyramid re-checks whose direction follows the
    /// current sign (FFmpeg's nz_coeff discipline).
    #[test]
    fn residue_replay_first_write_sets_then_rechecks() {
        let block = replay_residue(&[
            DctCoef { ccoef: 5, value: -3 },
            DctCoef { ccoef: 5, value: 2 },  // re-check: was < 0 → -2
            DctCoef { ccoef: 7, value: 4 },
            DctCoef { ccoef: 5, value: 1 },  // re-check: still < 0 → -1
        ]);
        assert_eq!(block[5], -6);
        assert_eq!(block[7], 4);
        let block2 = replay_residue(&[
            DctCoef { ccoef: 9, value: 3 },
            DctCoef { ccoef: 9, value: 2 },  // was >= 0 → +2
        ]);
        assert_eq!(block2[9], 5);
    }

    /// Every colors-bundle pop is accounted for: inlined payloads too.
    #[test]
    fn colors_contribution_counts_every_pop() {
        let run = BlockRecord::Run {
            pattern: 0,
            runs: vec![
                RunItem { run: 3, color: Some(7) },
                RunItem { run: 2, color: None },
            ],
        };
        assert_eq!(colors_contribution(&run), 3); // 1 inline + 2 stream
        let scaled_run = BlockRecord::Scaled(Box::new(SubBlock::Run {
            pattern: 0,
            runs: vec![RunItem { run: 5, color: None }],
        }));
        assert_eq!(colors_contribution(&scaled_run), 5);
        assert_eq!(
            colors_contribution(&BlockRecord::Fill { color: 1 }),
            1
        );
        assert_eq!(
            colors_contribution(&BlockRecord::Pattern {
                colors: [0, 0],
                patterns: [0; 8]
            }),
            2
        );
        assert_eq!(
            colors_contribution(&BlockRecord::Raw { colors: [0; 64] }),
            64
        );
        assert_eq!(
            colors_contribution(&BlockRecord::Scaled(Box::new(SubBlock::Intra {
                dc: 0,
                quant: 0,
                coeffs: Vec::new(),
            }))),
            0
        );
    }

    // -- render_plane fail-closed gates --------------------------------------

    fn symbols_with(blocks: Vec<BlockRecord>) -> PlaneSymbols {
        PlaneSymbols {
            bw: 2,
            bh: 1,
            blocks,
            colors_consumed: Vec::new(),
            patterns_consumed: Vec::new(),
            xoff_consumed: Vec::new(),
            yoff_consumed: Vec::new(),
            intradc_consumed: Vec::new(),
            interdc_consumed: Vec::new(),
            end_bit: 0,
        }
    }

    /// bink_put_pixels' bounds gate: ref outside
    /// [0, ((bw-1)+stride*(bh-1))*8] fails the frame.
    #[test]
    fn render_plane_rejects_out_of_bounds_motion() {
        let sym = symbols_with(vec![BlockRecord::Motion { x: 0, y: 100 }]);
        let mut dst = vec![0u8; 16 * 24];
        let err = render_plane(&sym, &mut dst, None, 16, 2, 1).unwrap_err();
        assert!(format!("{err}").contains("out of bounds"));
    }

    /// A RUN record whose per-position colors are not in the captured
    /// stream fails closed.
    #[test]
    fn render_plane_rejects_short_colors_stream() {
        let sym = symbols_with(vec![BlockRecord::Run {
            pattern: 0,
            runs: vec![RunItem { run: 4, color: None }],
        }]);
        let mut dst = vec![0u8; 16 * 24];
        assert!(render_plane(&sym, &mut dst, None, 16, 2, 1).is_err());
    }

    // -- REAL fixtures, rendered pixels == real static-ffmpeg renders -------

    /// The m51 crop layout: luma 64x64 + U 32x32 + V 32x32 at (0,0).
    fn crop(planes: &BinkPlanes) -> Vec<u8> {
        let mut out = Vec::new();
        for r in 0..64 {
            out.extend_from_slice(&planes.y[r * planes.stride_y..r * planes.stride_y + 64]);
        }
        for r in 0..32 {
            out.extend_from_slice(&planes.u[r * planes.stride_c..r * planes.stride_c + 32]);
        }
        for r in 0..32 {
            out.extend_from_slice(&planes.v[r * planes.stride_c..r * planes.stride_c + 32]);
        }
        out
    }

    fn intro_span(name: &str) -> Vec<u8> {
        let span = fixture(name);
        let head = crate::frames::parse_frame_head(
            &span,
            4,
            &[145_920, 72_960, 72_960, 145_920],
            b'i',
        )
        .expect("intro frame head");
        span[head.video_start..].to_vec()
    }

    /// Intro frame 0 — the all-black fade-in: luma 16 / chroma 128
    /// (MPEG range black) EVERYWHERE, and byte-identical to the real
    /// static-ffmpeg render (m51_validate.py ground truth).
    #[test]
    fn intro_frame0_pixels_are_mpeg_black_and_match_ffmpeg() {
        let video = intro_span("movies_bik_1e218528_frame0_19324.bin");
        let mut dec = BinkVideoDecoder::new(1280, 544, true).unwrap();
        dec.decode_frame(&video).expect("intro f0 decode");
        let planes = dec.last().unwrap();
        let w = 1280usize;
        let h = 544usize;
        assert!(planes.y[..w * h].iter().all(|&v| v == 16));
        assert!(planes.u[..(w / 2) * (h / 2)].iter().all(|&v| v == 128));
        assert!(planes.v[..(w / 2) * (h / 2)].iter().all(|&v| v == 128));
        assert_eq!(crop(planes), fixture("movies_bik_1e218528_frame0_yuvcrop.bin"));
    }

    /// Intro frame 1 — real INTER content predicted from frame 0:
    /// decoding f0 then f1 with the same decoder reproduces the real
    /// static-ffmpeg render byte-exactly (motion compensation +
    /// inter DCT against a real previous frame).
    #[test]
    fn intro_frame1_inter_pixels_match_ffmpeg() {
        let v0 = intro_span("movies_bik_1e218528_frame0_19324.bin");
        let v1 = intro_span("movies_bik_1e218528_frame1_7644.bin");
        let mut dec = BinkVideoDecoder::new(1280, 544, true).unwrap();
        dec.decode_frame(&v0).expect("intro f0");
        dec.decode_frame(&v1).expect("intro f1");
        let planes = dec.last().unwrap();
        // the fade-in covers part of the crop with non-black content
        let crop = crop(planes);
        assert!(crop[..4096].iter().any(|&v| v != 16));
        assert_eq!(crop, fixture("movies_bik_1e218528_frame1_yuvcrop.bin"));
    }

    /// Silent movie frame 0 — the densest committed frame (48 932 B
    /// span, keyframe with INTRA/SCALED/RUN/FILL/PATTERN blocks):
    /// rendered pixels byte-identical to real static ffmpeg.
    #[test]
    fn silent_frame0_pixels_match_ffmpeg() {
        let span = fixture("movies_bik_17a2ee41_frame0_48932.bin");
        let head = crate::frames::parse_frame_head(&span, 0, &[], b'i').expect("frame head");
        let mut dec = BinkVideoDecoder::new(1280, 582, true).unwrap();
        dec.decode_frame(&span[head.video_start..]).expect("silent f0");
        let planes = dec.last().unwrap();
        assert_eq!(crop(planes), fixture("movies_bik_17a2ee41_frame0_yuvcrop.bin"));
    }
}

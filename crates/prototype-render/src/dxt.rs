//! Pure-Rust DXT block-compression decoder (BC1 / BC2 / BC3 = DXT1/2/3/5).
//!
//! Prototype (2009) textures inside P3D files use the S3TC family; the
//! wgpu render loop will feed them either directly to the GPU (native DXT
//! support) or through this CPU decoder for software paths and tooling.
//!
//! # Evidence status: PROVEN (spec + real art data)
//!
//! BC1/BC2/BC3 are *documented public formats* (Microsoft S3TC), so the
//! block decoders are specified completely by the spec; synthetic-block
//! tests check them against hand-computed spec values.  Block 16 flipped
//! the remaining gap: the real art textures (art.rcf, 4 775 Texture
//! chunks) are DXT1 (2 017), DXT5 (2 729) and DXT3 (11), and the
//! `0x00019002` ImageData chunk embeds a COMPLETE DDS file — 128-byte
//! header + block data, byte length exact on 4 765/4 765 chunks
//! (`prototype_p3d::geometry::decode_texture` hands it over verbatim).
//! [`parse_dds`] below consumes those files; 8 textures carry a
//! `0xFFFFFFFF` fourcc (uncompressed? — UNKNOWN) and fail closed.
//!
//! # Mip chains (block 29: PROVEN on 6 real committed fixtures)
//!
//! The DDS header's own `dwMipMapCount` word (offset 28) is the mip
//! ground truth, and it is the ONLY reliable one: the P3D Texture
//! header's mip field is unreliable in BOTH directions on real data —
//! art's `TUT078__I1` declares 8 mips while its DDS ships base-only
//! (`dwMipMapCount` 0), and cells' `communicationsarray001_diffuse`
//! declares 1 while its DDS carries the FULL 8-level chain.  Measured
//! facts (all 6 committed texture fixtures, byte-exact):
//!
//! * `dwMipMapCount` 0 (art fixtures) ⇒ base level only; caps 0x1000
//!   (`DDSCAPS_TEXTURE`); the payload is exactly the base level.
//! * `dwMipMapCount` N ≥ 2 (cells fixtures: 8 and 9) ⇒ a FULL chain of
//!   exactly N levels, `caps & 0x400000` (`DDSCAPS_MIPMAP`) set; the
//!   payload is EXACTLY the sum of the per-level sizes — BC1 8 B/block,
//!   BC2/3 16 B/block, one minimum block per level down to 1×1 — with
//!   no trailing bytes and no padding (43 704 / 87 408 / 10 936 all
//!   byte-exact).
//! * The declared word equals the chain length implied by the
//!   dimensions (`log2(max(w,h)) + 1`) on every chain file.
//!
//! `parse_dds` therefore fails closed on any chain file whose payload
//! is not byte-exact, and [`decode_dds_mips`] decodes every level.
//!
//! All decode paths are allocation-only: `#![forbid(unsafe_code)]` holds.

/// One BC1 (DXT1) colour block: 2 RGB565 endpoints + 2-bit indices.
const BC1_BLOCK_BYTES: usize = 8;
/// One BC3 (DXT5) block: 8-byte alpha block + 8-byte colour block.
const BC3_BLOCK_BYTES: usize = 16;
/// Blocks are always 4x4 texels.
const BLOCK: usize = 4;

/// Decode an RGB565 little-endian word into 8-bit R, G, B.
fn rgb565(w: u16) -> [u8; 3] {
    let r = ((w >> 11) & 0x1F) as u8;
    let g = ((w >> 5) & 0x3F) as u8;
    let b = (w & 0x1F) as u8;
    // Bit-replication expansion (spec-exact: high bits repeated into low).
    [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)]
}

/// Build the 4-entry BC1 colour palette for a block.
///
/// `c0 > c1` (as raw u16) => 4 opaque colours; otherwise the 4th slot is
/// transparent black (mode used by 1-bit-alpha DXT1 textures).
fn bc1_palette(c0: u16, c1: u16) -> [[u8; 4]; 4] {
    let (r0, g0, b0) = { let p = rgb565(c0); (p[0], p[1], p[2]) };
    let (r1, g1, b1) = { let p = rgb565(c1); (p[0], p[1], p[2]) };
    // Interpolate in wide arithmetic — u8 would overflow (2*255+255).
    let c2 = if c0 > c1 {
        [
            ((2 * r0 as u16 + r1 as u16) / 3) as u8,
            ((2 * g0 as u16 + g1 as u16) / 3) as u8,
            ((2 * b0 as u16 + b1 as u16) / 3) as u8,
            255,
        ]
    } else {
        [
            ((r0 as u16 + r1 as u16) / 2) as u8,
            ((g0 as u16 + g1 as u16) / 2) as u8,
            ((b0 as u16 + b1 as u16) / 2) as u8,
            255,
        ]
    };
    // Slot 3: opaque mode interpolates (c0 + 2*c1)/3; transparent mode is
    // transparent black (the DXT1 1-bit-alpha escape).
    let c3 = if c0 > c1 {
        [
            ((r0 as u16 + 2 * r1 as u16) / 3) as u8,
            ((g0 as u16 + 2 * g1 as u16) / 3) as u8,
            ((b0 as u16 + 2 * b1 as u16) / 3) as u8,
            255,
        ]
    } else {
        [0, 0, 0, 0]
    };
    [[r0, g0, b0, 255], [r1, g1, b1, 255], c2, c3]
}

/// Build the 8-entry BC3 alpha palette for a block.
fn bc3_alpha_palette(a0: u8, a1: u8) -> [u8; 8] {
    let (a0, a1) = (a0 as u16, a1 as u16);
    let mut p = [0u8; 8];
    p[0] = a0 as u8;
    p[1] = a1 as u8;
    if a0 > a1 {
        p[2] = ((6 * a0 + a1) / 7) as u8;
        p[3] = ((5 * a0 + 2 * a1) / 7) as u8;
        p[4] = ((4 * a0 + 3 * a1) / 7) as u8;
        p[5] = ((3 * a0 + 4 * a1) / 7) as u8;
        p[6] = ((2 * a0 + 5 * a1) / 7) as u8;
        p[7] = ((a0 + 6 * a1) / 7) as u8;
    } else {
        p[2] = ((4 * a0 + a1) / 5) as u8;
        p[3] = ((3 * a0 + 2 * a1) / 5) as u8;
        p[4] = ((2 * a0 + 3 * a1) / 5) as u8;
        p[5] = ((a0 + 4 * a1) / 5) as u8;
        p[6] = 0;
        p[7] = 255;
    }
    p
}

/// Decode result errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DxtError {
    /// Data length does not match the declared dimensions.
    BadLength {
        /// Declared surface width.
        width: u32,
        /// Declared surface height.
        height: u32,
        /// Bytes provided.
        got: usize,
        /// Bytes required by the format.
        want: usize,
    },
    /// Width or height is zero.
    EmptySurface,
}

impl std::fmt::Display for DxtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DxtError::BadLength { width, height, got, want } => write!(
                f,
                "DXT surface {width}x{height}: got {got} bytes, need {want}"
            ),
            DxtError::EmptySurface => write!(f, "DXT surface has zero extent"),
        }
    }
}
impl std::error::Error for DxtError {}

/// Required byte length of a BC1 (DXT1) surface.
pub fn bc1_len(width: u32, height: u32) -> usize {
    let bw = width.div_ceil(BLOCK as u32) as usize;
    let bh = height.div_ceil(BLOCK as u32) as usize;
    bw * bh * BC1_BLOCK_BYTES
}

/// Required byte length of a BC2/BC3 (DXT3/DXT5) surface.
pub fn bc3_len(width: u32, height: u32) -> usize {
    let bw = width.div_ceil(BLOCK as u32) as usize;
    let bh = height.div_ceil(BLOCK as u32) as usize;
    bw * bh * BC3_BLOCK_BYTES
}

fn check_len(width: u32, height: u32, need: usize, got: usize) -> Result<(), DxtError> {
    if width == 0 || height == 0 {
        return Err(DxtError::EmptySurface);
    }
    if got != need {
        return Err(DxtError::BadLength { width, height, got, want: need });
    }
    Ok(())
}

/// Push one decoded 4x4 block into `out` at texel position (bx, by).
fn blit_block(
    out: &mut Vec<u8>,
    width: u32,
    height: u32,
    bx: usize,
    by: usize,
    colors: &[[u8; 4]; 16],
) {
    for ty in 0..BLOCK {
        let y = by + ty;
        if y as u32 >= height {
            break;
        }
        for tx in 0..BLOCK {
            let x = bx + tx;
            if x as u32 >= width {
                break;
            }
            let c = colors[ty * BLOCK + tx];
            let i = (y as usize * width as usize + x as usize) * 4;
            out[i] = c[0];
            out[i + 1] = c[1];
            out[i + 2] = c[2];
            out[i + 3] = c[3];
        }
    }
}

/// Decode a BC1 (DXT1) surface to RGBA8, row-major, `width*height*4` bytes.
pub fn decode_bc1(data: &[u8], width: u32, height: u32) -> Result<Vec<u8>, DxtError> {
    check_len(width, height, bc1_len(width, height), data.len())?;
    let bw = width.div_ceil(BLOCK as u32) as usize;
    let mut out = vec![0u8; width as usize * height as usize * 4];
    for block in 0..bw * ((height as usize + BLOCK - 1) / BLOCK) {
        let b = &data[block * BC1_BLOCK_BYTES..(block + 1) * BC1_BLOCK_BYTES];
        let c0 = u16::from_le_bytes([b[0], b[1]]);
        let c1 = u16::from_le_bytes([b[2], b[3]]);
        let pal = bc1_palette(c0, c1);
        let mut colors = [[0u8; 4]; 16];
        for ty in 0..BLOCK {
            for tx in 0..BLOCK {
                let idx = ((b[4 + ty] >> (2 * tx)) & 0b11) as usize;
                colors[ty * BLOCK + tx] = pal[idx];
            }
        }
        blit_block(&mut out, width, height, (block % bw) * BLOCK, (block / bw) * BLOCK, &colors);
    }
    Ok(out)
}

/// Decode a BC3 (DXT5) surface to RGBA8.
///
/// BC2 (DXT3) shares the explicit-alpha path: see [`decode_bc2`].
pub fn decode_bc3(data: &[u8], width: u32, height: u32) -> Result<Vec<u8>, DxtError> {
    check_len(width, height, bc3_len(width, height), data.len())?;
    let bw = width.div_ceil(BLOCK as u32) as usize;
    let mut out = vec![0u8; width as usize * height as usize * 4];
    for block in 0..bw * ((height as usize + BLOCK - 1) / BLOCK) {
        let b = &data[block * BC3_BLOCK_BYTES..(block + 1) * BC3_BLOCK_BYTES];
        // --- alpha half (bytes 0..8) ---
        let a0 = b[0];
        let a1 = b[1];
        let apal = bc3_alpha_palette(a0, a1);
        let mut colors = [[0u8; 4]; 16];
        for ty in 0..BLOCK {
            for tx in 0..BLOCK {
                let bit = 3 * (ty * BLOCK + tx);
                let byte = 2 + bit / 8;
                let shift = bit % 8;
                // 3-bit code can straddle a byte boundary — reassemble.
                let code = ((b[byte] as u16 | ((b[byte + 1] as u16) << 8)) >> shift) & 0b111;
                colors[ty * BLOCK + tx] =
                    [0, 0, 0, apal[code as usize]];
            }
        }
        // --- colour half (bytes 8..16) — same layout as BC1 ---
        let c0 = u16::from_le_bytes([b[8], b[9]]);
        let c1 = u16::from_le_bytes([b[10], b[11]]);
        let cpal = bc1_palette(c0, c1);
        for ty in 0..BLOCK {
            for tx in 0..BLOCK {
                let idx = ((b[12 + ty] >> (2 * tx)) & 0b11) as usize;
                let c = colors[ty * BLOCK + tx];
                colors[ty * BLOCK + tx] = [cpal[idx][0], cpal[idx][1], cpal[idx][2], c[3]];
            }
        }
        blit_block(&mut out, width, height, (block % bw) * BLOCK, (block / bw) * BLOCK, &colors);
    }
    Ok(out)
}

/// Decode a BC2 (DXT3) surface to RGBA8 (explicit 4-bit alpha).
pub fn decode_bc2(data: &[u8], width: u32, height: u32) -> Result<Vec<u8>, DxtError> {
    check_len(width, height, bc3_len(width, height), data.len())?;
    let bw = width.div_ceil(BLOCK as u32) as usize;
    let mut out = vec![0u8; width as usize * height as usize * 4];
    for block in 0..bw * ((height as usize + BLOCK - 1) / BLOCK) {
        let b = &data[block * BC3_BLOCK_BYTES..(block + 1) * BC3_BLOCK_BYTES];
        let mut colors = [[0u8; 4]; 16];
        // --- explicit 4-bit alpha (bytes 0..8), little-endian nibbles ---
        for ty in 0..BLOCK {
            for tx in 0..BLOCK {
                let bit = 4 * (ty * BLOCK + tx);
                let byte = bit / 8;
                let nib = (b[byte] >> (4 * (bit % 8 / 4))) & 0x0F;
                colors[ty * BLOCK + tx] = [0, 0, 0, (nib << 4) | nib];
            }
        }
        // --- colour half — same layout as BC1 ---
        let c0 = u16::from_le_bytes([b[8], b[9]]);
        let c1 = u16::from_le_bytes([b[10], b[11]]);
        let cpal = bc1_palette(c0, c1);
        for ty in 0..BLOCK {
            for tx in 0..BLOCK {
                let idx = ((b[12 + ty] >> (2 * tx)) & 0b11) as usize;
                let c = colors[ty * BLOCK + tx];
                colors[ty * BLOCK + tx] = [cpal[idx][0], cpal[idx][1], cpal[idx][2], c[3]];
            }
        }
        blit_block(&mut out, width, height, (block % bw) * BLOCK, (block / bw) * BLOCK, &colors);
    }
    Ok(out)
}

/// Compress one 4x4 RGBA8 tile to a BC1 block (simple min/max-endpoint
/// encoder, no refinement) — for round-trip tests and tooling.
pub fn encode_bc1_block(tile: &[[u8; 4]; 16]) -> [u8; BC1_BLOCK_BYTES] {
    // Pick the two most distant colours as endpoints.
    let mut best = (0u16, 0u16, 0u32); // (c0, c1, distance)
    for i in 0..16 {
        for j in (i + 1)..16 {
            let d = dist2(tile[i], tile[j]);
            if d >= best.2 {
                best = (to565(tile[i]), to565(tile[j]), d);
            }
        }
    }
    let (mut c0, mut c1, _) = best;
    if c0 < c1 {
        std::mem::swap(&mut c0, &mut c1); // opaque mode needs c0 > c1
    }
    let pal = bc1_palette(c0, c1);
    let mut out = [0u8; BC1_BLOCK_BYTES];
    out[0..2].copy_from_slice(&c0.to_le_bytes());
    out[2..4].copy_from_slice(&c1.to_le_bytes());
    for ty in 0..BLOCK {
        for tx in 0..BLOCK {
            let t = tile[ty * BLOCK + tx];
            let mut bi = 0usize;
            let mut bd = u32::MAX;
            for (k, p) in pal.iter().enumerate() {
                let d = dist2(t, *p);
                if d < bd {
                    bd = d;
                    bi = k;
                }
            }
            out[4 + ty] |= (bi as u8) << (2 * tx);
        }
    }
    out
}

fn dist2(a: [u8; 4], b: [u8; 4]) -> u32 {
    let dr = a[0] as i32 - b[0] as i32;
    let dg = a[1] as i32 - b[1] as i32;
    let db = a[2] as i32 - b[2] as i32;
    (dr * dr + dg * dg + db * db) as u32
}

fn to565(c: [u8; 4]) -> u16 {
    ((c[0] as u16 >> 3) << 11) | ((c[1] as u16 >> 2) << 5) | (c[2] as u16 >> 3)
}


// ---------------------------------------------------------------------------
// DDS container parsing — PROVEN against the DDS files embedded in the
// `0x00019002` ImageData chunks of art.rcf (block 16, corpus-wide)
// ---------------------------------------------------------------------------

/// Which BC format a DDS fourcc names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DdsFormat {
    /// `DXT1` = BC1.
    Bc1,
    /// `DXT2` / `DXT3` = BC2 (explicit alpha).
    Bc2,
    /// `DXT4` / `DXT5` = BC3 (interpolated alpha).
    Bc3,
}

/// One mip level of a chained DDS: dimensions plus where its block data
/// lives in the original DDS buffer.  Level 0 (the largest) comes first
/// in [`DdsImage::mip_levels`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DdsLevel {
    /// Texel width of this level (`max(1, base >> i)`).
    pub width: u32,
    /// Texel height of this level.
    pub height: u32,
    /// Byte offset of the level's block data inside the buffer passed
    /// to [`parse_dds`] (always ≥ 128: the header precedes the payload).
    pub offset: usize,
    /// Byte length of the level's block payload (validated exact).
    pub len: usize,
}

/// One parsed DDS file: dimensions, format and the block payload that
/// follows the 128-byte header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdsImage {
    /// Surface width in texels.
    pub width: u32,
    /// Surface height in texels.
    pub height: u32,
    /// Compression format named by the pixel-format fourcc.
    pub format: DdsFormat,
    /// Raw level-0 block data (`data[128..128 + base]`), verbatim — the
    /// same meaning [`parse_dds`] has always produced (base level, even
    /// when a full mip chain follows; use [`DdsImage::mip_levels`] for
    /// the chained levels).
    pub blocks: Vec<u8>,
    /// The DDS header's own `dwMipMapCount` word (offset 28), verbatim.
    /// 0 on the base-only art fixtures; the full level count (8/9) on
    /// the chain-bearing cells fixtures.  PROVEN ground truth — the P3D
    /// Texture header's mip field is unreliable in both directions.
    pub mip_count: u32,
    /// Mip-level descriptors, level 0 first — EMPTY for base-only files
    /// (`mip_count ≤ 1`), length `mip_count` for chain files (payload
    /// validated byte-exact against the chain sum).
    pub mip_levels: Vec<DdsLevel>,
}

/// DDS parsing errors.  Every variant is fail-closed: nothing is guessed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DdsError {
    /// Fewer than the fixed 128-byte header.
    TooShort {
        /// Bytes provided.
        got: usize,
    },
    /// The `DDS ` magic is missing.
    BadMagic,
    /// `dwSize` (the header's own length field) is not the spec's 124.
    BadHeaderSize {
        /// Value found.
        got: u32,
    },
    /// The pixel-format fourcc is not a DXT block-compression format
    /// (this includes the `0xFFFFFFFF` fourcc carried by 8 real art
    /// textures whose encoding is UNKNOWN).
    UnsupportedFourcc {
        /// The four bytes found.
        fourcc: [u8; 4],
    },
    /// The block payload does not fill the declared surface.
    PayloadTruncated {
        /// Bytes required by the format.
        need: usize,
        /// Bytes available.
        got: usize,
    },
    /// A `dwMipMapCount ≥ 2` file whose payload is not byte-exact
    /// against the full per-level chain (or whose declared level count
    /// disagrees with the dimensions).  Fail-closed: the corpus's chain
    /// files are exact, so any mismatch means misparse or corruption.
    BadMipChain {
        /// Declared `dwMipMapCount`.
        declared: u32,
        /// Levels implied by the dimensions.
        implied: u32,
        /// Bytes required by the full chain.
        need: usize,
        /// Bytes available.
        got: usize,
    },
    /// Width or height is zero.
    EmptySurface,
}

impl std::fmt::Display for DdsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DdsError::TooShort { got } => write!(f, "DDS needs a 128-B header, got {got} B"),
            DdsError::BadMagic => write!(f, "DDS magic missing"),
            DdsError::BadHeaderSize { got } => write!(f, "DDS dwSize = {got}, want 124"),
            DdsError::UnsupportedFourcc { fourcc } => {
                write!(f, "DDS fourcc {:?} is not a DXT block format", fourcc)
            }
            DdsError::PayloadTruncated { need, got } => {
                write!(f, "DDS payload {got} B, need {need}")
            }
            DdsError::BadMipChain { declared, implied, need, got } => write!(
                f,
                "DDS mip chain: declared {declared} levels (implied {implied}), payload {got} B, chain needs {need} B"
            ),
            DdsError::EmptySurface => write!(f, "DDS surface has zero extent"),
        }
    }
}
impl std::error::Error for DdsError {}

/// DDS header is a fixed 128 bytes before the (optional) payload.
const DDS_HEADER_LEN: usize = 128;

/// Parse a complete DDS file (as embedded verbatim in a P3D
/// [`prototype_p3d::geometry::decode_texture`] result).
///
/// Validates the magic, the fixed header size, and that the block
/// payload exactly fills the declared surface for the named format.
/// When the header's `dwMipMapCount` (offset 28) is ≥ 2 the payload
/// must additionally be byte-exact against the FULL per-level mip
/// chain (block-29 corpus facts: chain files are exact, base-only
/// files declare 0); [`DdsImage::mip_levels`] then carries one
/// descriptor per level.
pub fn parse_dds(data: &[u8]) -> Result<DdsImage, DdsError> {
    if data.len() < DDS_HEADER_LEN {
        return Err(DdsError::TooShort { got: data.len() });
    }
    if &data[0..4] != b"DDS " {
        return Err(DdsError::BadMagic);
    }
    let u32le = |o: usize| u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
    let size = u32le(4);
    if size != 124 {
        return Err(DdsError::BadHeaderSize { got: size });
    }
    let height = u32le(12);
    let width = u32le(16);
    if width == 0 || height == 0 {
        return Err(DdsError::EmptySurface);
    }
    let declared_mips = u32le(28);
    let mut fourcc = [0u8; 4];
    fourcc.copy_from_slice(&data[84..88]);
    let format = match &fourcc {
        b"DXT1" => DdsFormat::Bc1,
        b"DXT2" | b"DXT3" => DdsFormat::Bc2,
        b"DXT4" | b"DXT5" => DdsFormat::Bc3,
        other => return Err(DdsError::UnsupportedFourcc { fourcc: *other }),
    };
    let base = match format {
        DdsFormat::Bc1 => bc1_len(width, height),
        DdsFormat::Bc2 | DdsFormat::Bc3 => bc3_len(width, height),
    };
    let blocks = &data[DDS_HEADER_LEN..];
    if blocks.len() < base {
        return Err(DdsError::PayloadTruncated { need: base, got: blocks.len() });
    }
    // -- Mip chain (block 29, PROVEN on 6 real fixtures) ------------------
    // `dwMipMapCount` 0/1 ⇒ base-only (every art fixture).  ≥ 2 ⇒ a FULL
    // chain whose payload is byte-exact; the declared count must also
    // equal the count implied by the dimensions (8 on 128², 9 on 256²).
    let mip_levels = if declared_mips >= 2 {
        let dims = mip_level_dims(width, height);
        let implied = dims.len() as u32;
        let mut levels = Vec::with_capacity(dims.len());
        let mut total = 0usize;
        let mut off = DDS_HEADER_LEN;
        for (lw, lh) in dims {
            let len = match format {
                DdsFormat::Bc1 => bc1_len(lw, lh),
                DdsFormat::Bc2 | DdsFormat::Bc3 => bc3_len(lw, lh),
            };
            levels.push(DdsLevel { width: lw, height: lh, offset: off, len });
            total += len;
            off += len;
        }
        if declared_mips != implied || blocks.len() != total {
            return Err(DdsError::BadMipChain {
                declared: declared_mips,
                implied,
                need: total,
                got: blocks.len(),
            });
        }
        levels
    } else {
        Vec::new()
    };
    Ok(DdsImage {
        width,
        height,
        format,
        blocks: blocks[..base].to_vec(),
        mip_count: declared_mips,
        mip_levels,
    })
}

/// Mip-level dimensions for a `width × height` base surface: halving
/// both to a floor of 1 until 1×1 inclusive (256² ⇒ 9 levels, 128² ⇒ 8).
fn mip_level_dims(width: u32, height: u32) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let (mut w, mut h) = (width, height);
    loop {
        out.push((w, h));
        if w == 1 && h == 1 {
            return out;
        }
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
}

/// Decode a complete DDS file to RGBA8 (row-major, `w*h*4` bytes).
///
/// [`parse_dds`] has already validated the payload length exactly, so the
/// block decoders' length re-check cannot fire; the conversion is total.
pub fn decode_dds(data: &[u8]) -> Result<Vec<u8>, DdsError> {
    let img = parse_dds(data)?;
    let decoded = match img.format {
        DdsFormat::Bc1 => decode_bc1(&img.blocks, img.width, img.height),
        DdsFormat::Bc2 => decode_bc2(&img.blocks, img.width, img.height),
        DdsFormat::Bc3 => decode_bc3(&img.blocks, img.width, img.height),
    };
    match decoded {
        Ok(px) => Ok(px),
        // Total by construction: surface non-empty + exact length proven
        // above.  Map the residual to the length error with real values.
        Err(DxtError::BadLength { got, want, .. }) => {
            Err(DdsError::PayloadTruncated { need: want, got })
        }
        Err(DxtError::EmptySurface) => Err(DdsError::EmptySurface),
    }
}

/// One decoded mip surface: RGBA8 texels + dimensions (block 29).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedSurface {
    /// RGBA8, row-major, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
    /// Texel width of this level.
    pub width: u32,
    /// Texel height of this level.
    pub height: u32,
}

/// Decode a complete DDS file's mip chain to RGBA8 — level 0 (base)
/// first, then every chained level when the file carries a full chain
/// (block 29).  Base-only files decode to a single surface, so the
/// result is never empty.
///
/// Per-level decoding runs the same proven BC1/BC2/BC3 block decoders
/// on the level's exact block slice (validated by [`parse_dds`]).
pub fn decode_dds_mips(data: &[u8]) -> Result<Vec<DecodedSurface>, DdsError> {
    let img = parse_dds(data)?;
    let decode_level = |level: &DdsLevel| {
        let blocks = &data[level.offset..level.offset + level.len];
        let decoded = match img.format {
            DdsFormat::Bc1 => decode_bc1(blocks, level.width, level.height),
            DdsFormat::Bc2 => decode_bc2(blocks, level.width, level.height),
            DdsFormat::Bc3 => decode_bc3(blocks, level.width, level.height),
        };
        match decoded {
            Ok(px) => Ok(DecodedSurface { rgba: px, width: level.width, height: level.height }),
            Err(DxtError::BadLength { got, want, .. }) => {
                Err(DdsError::PayloadTruncated { need: want, got })
            }
            Err(DxtError::EmptySurface) => Err(DdsError::EmptySurface),
        }
    };
    if img.mip_levels.is_empty() {
        // Base-only file: decode the base level once.
        return decode_level(&DdsLevel {
            width: img.width,
            height: img.height,
            offset: DDS_HEADER_LEN,
            len: img.blocks.len(),
        })
        .map(|s| vec![s]);
    }
    img.mip_levels.iter().map(decode_level).collect()
}

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

    fn barrier_texture_dds() -> Vec<u8> {
        let buf = fixture("art_barrier_os002.p3d");
        let top = prototype_p3d::geometry::top_level_chunks(&buf).unwrap();
        let tex = top
            .iter()
            .find(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_TEXTURE)
            .map(|c| prototype_p3d::geometry::decode_texture(&buf, c.offset, c.size).unwrap())
            .unwrap();
        tex.dds
    }

    /// The real barrier DDS: 256x256 DXT1, decoded to RGBA8 with non-trivial
    /// content (the decoder runs against genuine compressed art data).
    #[test]
    fn real_barrier_dds_decodes_to_rgba8() {
        let dds = barrier_texture_dds();
        let img = parse_dds(&dds).unwrap();
        assert_eq!((img.width, img.height), (256, 256));
        assert_eq!(img.format, DdsFormat::Bc1);
        assert_eq!(img.blocks.len(), 32_768);
        let px = decode_dds(&dds).unwrap();
        assert_eq!(px.len(), 256 * 256 * 4);
        // Non-trivial: neither all-black nor all-white, alpha opaque (BC1
        // opaque mode on this texture).
        let mut black = 0;
        let mut white = 0;
        let mut opaque = 0;
        for p in px.chunks_exact(4) {
            if p[0] == 0 && p[1] == 0 && p[2] == 0 {
                black += 1;
            }
            if p[0] == 255 && p[1] == 255 && p[2] == 255 {
                white += 1;
            }
            if p[3] == 255 {
                opaque += 1;
            }
        }
        assert!(black + white < px.len() / 4);
        assert!(opaque > 0);
    }

    /// The real TUT078 DDS: 128x128 DXT5 with alpha.
    #[test]
    fn real_tut078_dds_decodes() {
        let buf = fixture("art_texture_tut078.p3d");
        let top = prototype_p3d::geometry::top_level_chunks(&buf).unwrap();
        let tex = top
            .iter()
            .find(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_TEXTURE)
            .map(|c| prototype_p3d::geometry::decode_texture(&buf, c.offset, c.size).unwrap())
            .unwrap();
        assert_eq!(&tex.fourcc, b"DXT5");
        let img = parse_dds(&tex.dds).unwrap();
        assert_eq!((img.width, img.height), (128, 128));
        assert_eq!(img.format, DdsFormat::Bc3);
        assert_eq!(img.blocks.len(), 16_384);
        let px = decode_dds(&tex.dds).unwrap();
        assert_eq!(px.len(), 128 * 128 * 4);
    }

    /// Block 24: a texture EMBEDDED IN A CELLS.RCF GEOMETRY MEMBER
    /// decodes through the exact same DXT path as art.rcf textures.
    /// This one ships a FULL mip chain in the DDS (1 367 DXT1 blocks;
    /// art fixtures ship the base level only) — block 29 consumes the
    /// chain (see `real_dds_mip_chain_ground_truth`); the base-level
    /// parse here is unchanged.
    #[test]
    fn real_cells82_dds_decodes() {
        let buf = fixture("cells82_texture_comm.p3d");
        let top = prototype_p3d::geometry::top_level_chunks(&buf).unwrap();
        let tex = top
            .iter()
            .find(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_TEXTURE)
            .map(|c| prototype_p3d::geometry::decode_texture(&buf, c.offset, c.size).unwrap())
            .unwrap();
        assert_eq!(&tex.fourcc, b"DXT1");
        assert_eq!(tex.dds.len(), 11_064, "128 B header + full mip chain");
        let img = parse_dds(&tex.dds).unwrap();
        assert_eq!((img.width, img.height), (128, 128));
        assert_eq!(img.format, DdsFormat::Bc1);
        assert_eq!(img.blocks.len(), 8_192, "base level only (1 024 blocks × 8 B)");
        let px = decode_dds(&tex.dds).unwrap();
        assert_eq!(px.len(), 128 * 128 * 4);
        // Non-trivial content: not everything maps to one flat colour.
        let mut black = 0;
        let mut white = 0;
        for p in px.chunks_exact(4) {
            if p[0] == 0 && p[1] == 0 && p[2] == 0 {
                black += 1;
            }
            if p[0] == 255 && p[1] == 255 && p[2] == 255 {
                white += 1;
            }
        }
        assert!(black + white < px.len() / 4);
    }

    /// Block 29: the DDS header's own `dwMipMapCount` word is the mip
    /// ground truth, PROVEN byte-exact across all committed fixtures —
    /// and the P3D Texture header's mip field is unreliable in BOTH
    /// directions (art TUT078 declares 8 with a base-only DDS; cells
    /// communicationsarray001 declares 1 with a FULL 8-level chain).
    #[test]
    fn real_dds_mip_chain_ground_truth() {
        let tex_dds = |name: &str| {
            let buf = fixture(name);
            let top = prototype_p3d::geometry::top_level_chunks(&buf).unwrap();
            let tex = top
                .iter()
                .find(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_TEXTURE)
                .map(|c| prototype_p3d::geometry::decode_texture(&buf, c.offset, c.size).unwrap())
                .unwrap();
            (tex.fields[4], tex.dds)
        };

        // Art TUT078: P3D declares 8 mips, the DDS ships BASE ONLY
        // (dwMipMapCount 0 — parse must NOT treat the declaration as
        // chain evidence).
        let (p3d_mips, dds) = tex_dds("art_texture_tut078.p3d");
        assert_eq!(p3d_mips, 8, "P3D header declares 8");
        let img = parse_dds(&dds).unwrap();
        assert_eq!(img.mip_count, 0, "DDS word says base-only — ground truth");
        assert!(img.mip_levels.is_empty());
        assert_eq!(img.blocks.len(), 16_384);

        // Cells communicationsarray001_diffuse: P3D declares 1, the DDS
        // carries the FULL 8-level BC1 chain (byte-exact 10 936 B).
        let (p3d_mips, dds) = tex_dds("cells82_texture_comm.p3d");
        assert_eq!(p3d_mips, 1, "P3D header declares 1");
        let img = parse_dds(&dds).unwrap();
        assert_eq!(img.mip_count, 8);
        assert_eq!(img.mip_levels.len(), 8);
        let sizes: Vec<usize> = [8192, 2048, 512, 128, 32, 8, 8, 8].to_vec();
        let mut off = 128usize;
        for (i, lv) in img.mip_levels.iter().enumerate() {
            let (w, h) = (128u32 >> i, 128u32 >> i);
            assert_eq!((lv.width, lv.height), (w.max(1), h.max(1)), "level {i} dims");
            assert_eq!(lv.len, sizes[i], "level {i} bytes");
            assert_eq!(lv.offset, off, "level {i} offset");
            off += lv.len;
        }
        assert_eq!(off, dds.len(), "chain ends exactly at the payload end");
        assert_eq!(img.blocks.len(), 8_192, "blocks stays the base level");
    }

    /// Block 29: the win2 multitex member carries TWO full chains (BC1
    /// diffuse 9 levels + BC3 normal 9 levels on 256²) — both parse
    /// byte-exact and every level decodes to the exact RGBA8 size.
    #[test]
    fn real_cells82_multitex_mips_decode() {
        let buf = fixture("cells82_multitex.p3d");
        let top = prototype_p3d::geometry::top_level_chunks(&buf).unwrap();
        let texs: Vec<_> = top
            .iter()
            .filter(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_TEXTURE)
            .map(|c| prototype_p3d::geometry::decode_texture(&buf, c.offset, c.size).unwrap())
            .collect();
        assert_eq!(texs.len(), 2, "diffuse + normal");
        let mut saw_bc1_chain = false;
        let mut saw_bc3_chain = false;
        for tex in &texs {
            let img = parse_dds(&tex.dds).unwrap();
            let (fourcc, levels) = if &tex.fourcc == b"DXT1" {
                saw_bc1_chain = true;
                (b"DXT1", 9usize)
            } else {
                assert_eq!(&tex.fourcc, b"DXT5");
                saw_bc3_chain = true;
                (b"DXT5", 9usize)
            };
            assert_eq!(img.mip_count as usize, levels, "{fourcc:?} chain");
            assert_eq!(img.mip_levels.len(), levels);
            assert_eq!(img.mip_levels[0].width, 256);
            assert_eq!(
                img.mip_levels.iter().map(|l| l.len).sum::<usize>(),
                tex.dds.len() - 128,
                "chain sum == payload, byte-exact"
            );
            let surfaces = decode_dds_mips(&tex.dds).unwrap();
            assert_eq!(surfaces.len(), levels);
            for (i, s) in surfaces.iter().enumerate() {
                let (w, h) = (256u32 >> i, 256u32 >> i);
                assert_eq!((s.width, s.height), (w.max(1), h.max(1)));
                assert_eq!(s.rgba.len(), s.width as usize * s.height as usize * 4);
            }
            // The last level is 1×1 — still real decoded texels.
            assert_eq!((surfaces[levels - 1].width, surfaces[levels - 1].height), (1, 1));
        }
        assert!(saw_bc1_chain && saw_bc3_chain);
    }

    /// Block 29: base-only files decode through `decode_dds_mips` as a
    /// single surface (the API never returns an empty chain).
    #[test]
    fn real_base_only_dds_mips_is_single_surface() {
        let buf = fixture("art_barrier_os002.p3d");
        let top = prototype_p3d::geometry::top_level_chunks(&buf).unwrap();
        let tex = top
            .iter()
            .find(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_TEXTURE)
            .map(|c| prototype_p3d::geometry::decode_texture(&buf, c.offset, c.size).unwrap())
            .unwrap();
        let img = parse_dds(&tex.dds).unwrap();
        assert_eq!(img.mip_count, 0);
        assert!(img.mip_levels.is_empty());
        let surfaces = decode_dds_mips(&tex.dds).unwrap();
        assert_eq!(surfaces.len(), 1);
        assert_eq!((surfaces[0].width, surfaces[0].height), (256, 256));
        assert_eq!(surfaces[0].rgba.len(), 256 * 256 * 4);
    }

    /// Block 29 fail-closed: a chain declaration the payload cannot
    /// back (declared 8 on a base-only file), a truncated chain and an
    /// overlong payload all error with `BadMipChain` — nothing is
    /// guessed from the unreliable P3D-side declarations.
    #[test]
    fn dds_mip_chain_mismatches_fail_closed() {
        // cells communicationsarray001_diffuse: real 8-level chain.
        let buf = fixture("cells82_texture_comm.p3d");
        let top = prototype_p3d::geometry::top_level_chunks(&buf).unwrap();
        let tex = top
            .iter()
            .find(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_TEXTURE)
            .map(|c| prototype_p3d::geometry::decode_texture(&buf, c.offset, c.size).unwrap())
            .unwrap();
        // TRUNCATED: one byte short of the exact chain.
        let short = &tex.dds[..tex.dds.len() - 1];
        let img = parse_dds(short).unwrap_err();
        assert!(matches!(img, DdsError::BadMipChain { declared: 8, implied: 8, .. }), "{img:?}");
        // OVERLONG: one extra byte appended.
        let mut long = tex.dds.clone();
        long.push(0);
        let img = parse_dds(&long).unwrap_err();
        assert!(matches!(img, DdsError::BadMipChain { declared: 8, implied: 8, .. }), "{img:?}");
        // DECLARED ≠ IMPLIED: an 8-level declaration on the 64² base
        // dims (implies 7 levels) — declared/implied disagree.
        let mut lied = tex.dds.clone();
        lied[16..20].copy_from_slice(&64u32.to_le_bytes());
        lied[12..16].copy_from_slice(&64u32.to_le_bytes());
        let img = parse_dds(&lied).unwrap_err();
        assert!(
            matches!(img, DdsError::BadMipChain { declared: 8, implied: 7, .. }),
            "{img:?}"
        );
        // BASE-ONLY FILE WITH A FORGED CHAIN WORD: barrier DDS declares
        // 0; forcing 9 makes the payload mismatch the chain.
        let bbuf = fixture("art_barrier_os002.p3d");
        let btop = prototype_p3d::geometry::top_level_chunks(&bbuf).unwrap();
        let btex = btop
            .iter()
            .find(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_TEXTURE)
            .map(|c| prototype_p3d::geometry::decode_texture(&bbuf, c.offset, c.size).unwrap())
            .unwrap();
        let mut forged = btex.dds.clone();
        forged[28..32].copy_from_slice(&9u32.to_le_bytes());
        let img = parse_dds(&forged).unwrap_err();
        assert!(
            matches!(img, DdsError::BadMipChain { declared: 9, implied: 9, .. }),
            "{img:?}"
        );
    }

    /// Fail-closed: bad magic, short buffer, non-DXT fourcc.
    #[test]
    fn dds_errors_fail_closed() {
        assert_eq!(parse_dds(b"short"), Err(DdsError::TooShort { got: 5 }));
        let mut hdr = vec![0u8; 128];
        assert_eq!(parse_dds(&hdr), Err(DdsError::BadMagic));
        hdr[0..4].copy_from_slice(b"DDS ");
        assert_eq!(parse_dds(&hdr), Err(DdsError::BadHeaderSize { got: 0 }));
        hdr[4..8].copy_from_slice(&124u32.to_le_bytes());
        hdr[12..16].copy_from_slice(&64u32.to_le_bytes());
        hdr[16..20].copy_from_slice(&64u32.to_le_bytes());
        assert_eq!(
            parse_dds(&hdr),
            Err(DdsError::UnsupportedFourcc { fourcc: [0, 0, 0, 0] })
        );
        // The corpus's 8 unknown-fourcc textures hit the same error.
        hdr[84..88].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(
            parse_dds(&hdr),
            Err(DdsError::UnsupportedFourcc { fourcc: [0xFF, 0xFF, 0xFF, 0xFF] })
        );
        // A DXT1 header with an empty payload is truncated.
        hdr[84..88].copy_from_slice(b"DXT1");
        assert_eq!(parse_dds(&hdr), Err(DdsError::PayloadTruncated { need: 8 * 16 * 16, got: 0 }));
    }

    #[test]
    fn rgb565_expands_with_bit_replication() {
        assert_eq!(rgb565(0xFFFF), [255, 255, 255]);
        assert_eq!(rgb565(0x0000), [0, 0, 0]);
        // R=31 G=0 B=0 -> pure red channel
        assert_eq!(rgb565(0xF800), [255, 0, 0]);
        // G=63 -> pure green
        assert_eq!(rgb565(0x07E0), [0, 255, 0]);
        // B=31 -> pure blue
        assert_eq!(rgb565(0x001F), [0, 0, 255]);
    }

    #[test]
    fn bc1_opaque_block_interpolates_thirds() {
        // c0 = pure red (0xF800), c1 = pure blue (0x001F); c0 > c1 -> opaque.
        let mut b = [0u8; 8];
        b[0..2].copy_from_slice(&0xF800u16.to_le_bytes());
        b[2..4].copy_from_slice(&0x001Fu16.to_le_bytes());
        // index row pattern: every texel its own palette entry
        let idx: [u8; 16] = [0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3];
        for ty in 0..4 {
            for tx in 0..4 {
                b[4 + ty] |= idx[ty * 4 + tx] << (2 * tx);
            }
        }
        let px = decode_bc1(&b, 4, 4).unwrap();
        let at = |x: usize, y: usize| {
            let i = (y * 4 + x) * 4;
            [px[i], px[i + 1], px[i + 2], px[i + 3]]
        };
        assert_eq!(at(0, 0), [255, 0, 0, 255]); // c0
        assert_eq!(at(1, 0), [0, 0, 255, 255]); // c1
        assert_eq!(at(2, 0), [170, 0, 85, 255]); // (2c0+c1)/3
        assert_eq!(at(3, 0), [85, 0, 170, 255]); // (c0+2c1)/3
    }

    #[test]
    fn bc1_transparent_block_zeroes_fourth_slot() {
        // c0 < c1 -> 4th palette entry is transparent black.
        let mut b = [0u8; 8];
        b[0..2].copy_from_slice(&0x001Fu16.to_le_bytes()); // blue, smaller
        b[2..4].copy_from_slice(&0xF800u16.to_le_bytes()); // red, larger
        for ty in 0..4 {
            b[4 + ty] = 0b11_11_11_11; // every texel -> palette[3]
        }
        let px = decode_bc1(&b, 4, 4).unwrap();
        for i in 0..16 {
            assert_eq!(&px[i * 4..i * 4 + 4], &[0, 0, 0, 0]);
        }
    }

    #[test]
    fn bc1_mixed_mode_half_transparent() {
        let mut b = [0u8; 8];
        b[0..2].copy_from_slice(&0x001Fu16.to_le_bytes()); // c0 < c1
        b[2..4].copy_from_slice(&0xF800u16.to_le_bytes());
        // row 0: all index 3 (transparent), row 1: all index 0 (blue),
        // row 2: all index 1 (red), row 3: all index 2 ((c0+c1)/2)
        b[4] = 0b11_11_11_11;
        b[5] = 0b00_00_00_00;
        b[6] = 0b01_01_01_01;
        b[7] = 0b10_10_10_10;
        let px = decode_bc1(&b, 4, 4).unwrap();
        let at = |x: usize, y: usize| {
            let i = (y * 4 + x) * 4;
            [px[i], px[i + 1], px[i + 2], px[i + 3]]
        };
        assert_eq!(at(0, 0), [0, 0, 0, 0]);
        assert_eq!(at(0, 1), [0, 0, 255, 255]);
        assert_eq!(at(0, 2), [255, 0, 0, 255]);
        assert_eq!(at(0, 3), [127, 0, 127, 255]); // (r0+r1)/2 integer division
    }

    #[test]
    fn bc3_alpha_ramp_decodes() {
        let mut b = [0u8; 16];
        b[0] = 0; // a0
        b[1] = 255; // a1, a0 < a1 -> the "a0 <= a1" palette
        // texel 0 -> code 0 (alpha 0), texel 1 -> code 1 (alpha 255),
        // texel 2 -> code 6 (0), texel 3 -> code 7 (255)
        // 3-bit codes packed LSB-first from byte 2:
        // bits: t0=000 t1=001 t2=110 t3=111 -> 24 bits = 0b111_110_001_000
        let bits: u32 = 0b111_110_001_000;
        b[2] = (bits & 0xFF) as u8;
        b[3] = ((bits >> 8) & 0xFF) as u8;
        b[4] = ((bits >> 16) & 0xFF) as u8;
        // colour half: c0 = c1 = black opaque, all indices 0
        for ty in 0..4 {
            b[12 + ty] = 0;
        }
        let px = decode_bc3(&b, 4, 4).unwrap();
        let a = |x: usize| px[x * 4 + 3];
        assert_eq!(a(0), 0);
        assert_eq!(a(1), 255);
        assert_eq!(a(2), 0); // palette[6] when a0<a1
        assert_eq!(a(3), 255); // palette[7] when a0<a1
    }

    #[test]
    fn bc3_alpha_palette_interpolates_sevenths() {
        // a0 > a1 -> the sevenths table
        let p = bc3_alpha_palette(254, 0);
        assert_eq!(p[2] as u32, (6 * 254) / 7);
        assert_eq!(p[5] as u32, (3 * 254) / 7);
        assert_eq!(p[7] as u32, 254 / 7);
        // a0 <= a1 -> the fifths table
        let q = bc3_alpha_palette(0, 254);
        assert_eq!(q[2] as u32, 254 / 5);
        assert_eq!(q[5] as u32, (4 * 254) / 5);
        let p2 = bc3_alpha_palette(255, 0);
        assert_eq!(p2[7] as u32, (255 + 6 * 0) / 7);
        // degenerate: a0 == a1 -> the a0<=a1 table
        let p3 = bc3_alpha_palette(128, 128);
        assert_eq!(p3[0], 128);
        assert_eq!(p3[6], 0);
        assert_eq!(p3[7], 255);
    }

    #[test]
    fn bc2_explicit_alpha_expands_nibbles() {
        let mut b = [0u8; 16];
        // texel 0 alpha nibble = 0x0, texel 1 = 0xF
        b[0] = 0b1111_0000;
        for ty in 0..4 {
            b[12 + ty] = 0;
        }
        let px = decode_bc2(&b, 4, 4).unwrap();
        assert_eq!(px[3], 0);
        assert_eq!(px[1 * 4 + 3], 255);
    }

    #[test]
    fn length_errors_are_precise() {
        assert_eq!(decode_bc1(&[], 4, 4), Err(DxtError::BadLength { width: 4, height: 4, got: 0, want: 8 }));
        assert_eq!(decode_bc3(&[], 4, 4), Err(DxtError::BadLength { width: 4, height: 4, got: 0, want: 16 }));
        assert_eq!(decode_bc1(&[0u8; 8], 0, 4), Err(DxtError::EmptySurface));
        assert_eq!(bc1_len(8, 8), 32);
        assert_eq!(bc3_len(8, 8), 64);
        // non-multiple-of-4 surfaces round up to full blocks
        assert_eq!(bc1_len(5, 5), 2 * 2 * 8);
    }

    #[test]
    fn non_multiple_of_four_surface_decodes_partial_edge() {
        // 5x5 = 2x2 blocks (32 bytes); texel (4,4) must be written.
        let b = [0u8; 32];
        let px = decode_bc1(&b, 5, 5).unwrap();
        assert_eq!(px.len(), 5 * 5 * 4);
        // A zeroed block: all indices 0 -> palette[0] = c0 = opaque black.
        assert_eq!(&px[(4 * 5 + 4) * 4..(4 * 5 + 4) * 4 + 4], &[0, 0, 0, 255]);
    }

    #[test]
    fn encode_round_trip_is_lossy_but_stable() {
        // Solid tile -> exact colour out.
        let tile = [[128u8, 64, 32, 255]; 16];
        let block = encode_bc1_block(&tile);
        let px = decode_bc1(&block, 4, 4).unwrap();
        for i in 0..16 {
            let c = &px[i * 4..i * 4 + 4];
            assert_eq!(c[3], 255);
            // endpoints snap to 565 grid: within 8/4/8 per channel
            assert!((c[0] as i32 - 128).abs() <= 8);
            assert!((c[1] as i32 - 64).abs() <= 4);
            assert!((c[2] as i32 - 32).abs() <= 8);
        }
    }

    #[test]
    fn full_surface_round_trip() {
        // Two-tile surface: left red tile, right blue tile.
        let mut tiles: Vec<[u8; 4]> = vec![];
        for _ in 0..16 {
            tiles.push([255, 0, 0, 255]);
        }
        let mut tiles_b: Vec<[u8; 4]> = vec![];
        for _ in 0..16 {
            tiles_b.push([0, 0, 255, 255]);
        }
        let mut data = vec![];
        data.extend_from_slice(&encode_bc1_block(&tiles.try_into().unwrap()));
        data.extend_from_slice(&encode_bc1_block(&tiles_b.try_into().unwrap()));
        let px = decode_bc1(&data, 8, 4).unwrap();
        let at = |x: usize, y: usize| {
            let i = (y * 8 + x) * 4;
            (px[i], px[i + 1], px[i + 2], px[i + 3])
        };
        let (r, g, b, a) = at(0, 0);
        assert_eq!((r, g, b, a), (255, 0, 0, 255));
        let (r, g, b, a) = at(5, 3);
        assert_eq!((r, g, b, a), (0, 0, 255, 255));
    }
}

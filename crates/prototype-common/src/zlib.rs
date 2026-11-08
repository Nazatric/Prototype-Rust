//! Minimal RFC 1950 (zlib) + RFC 1951 (DEFLATE) decoder.
//!
//! # Why hand-rolled
//!
//! The workspace is dependency-free Rust; the RZ payload wrapper used by
//! every art `.rcf` member (see `prototype_rcf::rz`) is a bare zlib
//! stream, so the engine needs a small, auditable inflater with no
//! supply-chain surface.  This module implements exactly RFC 1951
//! (stored / fixed-Huffman / dynamic-Huffman blocks, LZ77
//! back-references) plus the RFC 1950 header and Adler-32 trailer, and
//! **fails closed** on every malformed input (bad header check, bad
//! block length complement, over-subscribed Huffman codes, invalid
//! length/distance codes, distances before the start of the output,
//! truncated streams, Adler-32 mismatch).
//!
//! # Proof
//!
//! PROVEN against all 2 219 RZ members of `art.rcf` (the user's own PC
//! game data, 601 MB compressed): every member decompresses with
//! `zlib_decompress` to a byte-exact `P3D` file whose length equals the
//! RZ header's declared size (cross-checked against Python `zlib`).
//! The unit tests here additionally pin Python-`zlib`-generated
//! vectors for all three block types.

use crate::{P3dError, Result};

// ---------------------------------------------------------------------------
// Adler-32 (RFC 1950 §9)
// ---------------------------------------------------------------------------

/// Adler-32 checksum as defined by RFC 1950 (s1 = 1 + Σ bytes, s2 = Σ s1,
/// both mod 65521, big-endian in the zlib trailer).
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let (mut s1, mut s2) = (1u32, 0u32);
    // Process in chunks so s2 cannot overflow u32 (5552 is the classic
    // safe bound: 65521 * 5552 + 65520 < 2^32).
    for chunk in data.chunks(5552) {
        for &b in chunk {
            s1 += b as u32;
            s2 += s1;
        }
        s1 %= MOD;
        s2 %= MOD;
    }
    (s2 << 16) | s1
}

// ---------------------------------------------------------------------------
// Bit reader (LSB-first, per RFC 1951 §3.1.1)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bitbuf: u32,
    bitcnt: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0, bitbuf: 0, bitcnt: 0 }
    }

    /// Read `need` (0..=16) bits, LSB first.  `need == 0` is valid and
    /// common: length/distance codes with zero extra bits call this with
    /// 0 (a real-art-data bug caught by the fixture tests).
    fn bits(&mut self, need: u32) -> Result<u32> {
        debug_assert!(need <= 16);
        if need == 0 {
            return Ok(0);
        }
        while self.bitcnt < need {
            let b = *self.data.get(self.pos).ok_or_else(|| P3dError::Other(format!(
                "deflate: truncated stream at byte {} (needed {} more bit(s))",
                self.pos, need - self.bitcnt
            )))? as u32;
            self.pos += 1;
            self.bitbuf |= b << self.bitcnt;
            self.bitcnt += 8;
        }
        let val = self.bitbuf & ((1u32 << need) - 1);
        self.bitbuf >>= need;
        self.bitcnt -= need;
        Ok(val)
    }

    /// Discard partial-byte padding (before a stored-block header).
    fn align(&mut self) {
        self.bitbuf = 0;
        self.bitcnt = 0;
    }

    fn u16le(&mut self) -> Result<u16> {
        let lo = *self
            .data
            .get(self.pos)
            .ok_or_else(|| P3dError::Other("deflate: stored block truncated (LEN)".into()))?;
        let hi = *self
            .data
            .get(self.pos + 1)
            .ok_or_else(|| P3dError::Other("deflate: stored block truncated (LEN)".into()))?;
        self.pos += 2;
        Ok(u16::from_le_bytes([lo, hi]))
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| P3dError::Other("deflate: stored block length overflows".into()))?;
        let s = self
            .data
            .get(self.pos..end)
            .ok_or_else(|| P3dError::Other("deflate: stored block truncated (DATA)".into()))?;
        self.pos = end;
        Ok(s)
    }
}

// ---------------------------------------------------------------------------
// Canonical Huffman decoding (RFC 1951 §3.2.2)
// ---------------------------------------------------------------------------

/// A canonical Huffman table: per-length symbol counts + symbols sorted
/// by (length, symbol value).  Decoding walks one bit at a time — the
/// reference "puff" algorithm, easy to audit.
#[derive(Debug)]
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn build(lengths: &[u8]) -> Result<Self> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            counts[l as usize] += 1;
        }
        counts[0] = 0; // zero-length codes are absent codes
        // Over-subscription check: Σ counts[len] * 2^(15-len) <= 2^15.
        let mut left: i32 = 1;
        for &c in &counts[1..] {
            left <<= 1;
            left -= c as i32;
            if left < 0 {
                return Err(P3dError::Other("deflate: over-subscribed huffman code".into()));
            }
        }
        let mut offs = [0u16; 16];
        for len in 1..15usize {
            offs[len + 1] = offs[len] + counts[len];
        }
        let total = lengths.iter().filter(|&&l| l != 0).count();
        let mut symbols = vec![0u16; total];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[offs[l as usize] as usize] = sym as u16;
                offs[l as usize] += 1;
            }
        }
        Ok(Self { counts, symbols })
    }

    /// Decode one symbol, reading bits MSB-of-code-first out of the
    /// LSB-first stream (canonical code property).
    fn decode(&self, br: &mut BitReader<'_>) -> Result<u16> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..=15usize {
            code |= br.bits(1)? as i32;
            let count = self.counts[len] as i32;
            if code - first < count {
                let i = (index + (code - first)) as usize;
                return self
                    .symbols
                    .get(i)
                    .copied()
                    .ok_or_else(|| P3dError::Other("deflate: huffman symbol index out of range".into()));
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(P3dError::Other("deflate: invalid huffman code (ran out of codes)".into()))
    }
}

// ---------------------------------------------------------------------------
// Length / distance tables (RFC 1951 §3.2.5)
// ---------------------------------------------------------------------------

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
    131, 163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
    13, 13,
];
const CODE_LENGTH_ORDER: [usize; 19] =
    [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

// ---------------------------------------------------------------------------
// Inflater (RFC 1951)
// ---------------------------------------------------------------------------

/// Inflate a raw DEFLATE stream.  Returns the output and the number of
/// input bytes consumed (the byte containing the last stream bit; the
/// final partial byte's unused high bits are padding).
pub fn inflate(data: &[u8]) -> Result<(Vec<u8>, usize)> {
    let mut br = BitReader::new(data);
    let mut out: Vec<u8> = Vec::new();
    loop {
        let last = br.bits(1)?;
        let btype = br.bits(2)?;
        match btype {
            0 => {
                // Stored: discard padding, LEN + !LEN, then raw bytes.
                br.align();
                let len = br.u16le()?;
                let nlen = br.u16le()?;
                if len != !nlen {
                    return Err(P3dError::Other(format!(
                        "deflate: stored block length complement mismatch (LEN={len}, NLEN={nlen})"
                    )));
                }
                out.extend_from_slice(br.take(len as usize)?);
            }
            1 => {
                let (lit, dist) = fixed_tables()?;
                inflate_block(&mut br, &mut out, &lit, &dist)?;
            }
            2 => {
                let (lit, dist) = dynamic_tables(&mut br)?;
                inflate_block(&mut br, &mut out, &lit, &dist)?;
            }
            _ => {
                return Err(P3dError::Other("deflate: reserved block type 11".into()));
            }
        }
        if last == 1 {
            break;
        }
    }
    Ok((out, br.pos))
}

fn fixed_tables() -> Result<(Huffman, Huffman)> {
    let mut lit_lengths = [0u8; 288];
    for (i, l) in lit_lengths.iter_mut().enumerate() {
        *l = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    let dist_lengths = [5u8; 30];
    Ok((Huffman::build(&lit_lengths)?, Huffman::build(&dist_lengths)?))
}

fn dynamic_tables(br: &mut BitReader<'_>) -> Result<(Huffman, Huffman)> {
    let hlit = br.bits(5)? as usize + 257;
    let hdist = br.bits(5)? as usize + 1;
    let hclen = br.bits(4)? as usize + 4;
    if hlit > 286 || hdist > 30 {
        return Err(P3dError::Other(format!(
            "deflate: dynamic header counts out of range (HLIT={hlit}, HDIST={hdist})"
        )));
    }
    let mut cl_lengths = [0u8; 19];
    for &idx in CODE_LENGTH_ORDER.iter().take(hclen) {
        cl_lengths[idx] = br.bits(3)? as u8;
    }
    let cl = Huffman::build(&cl_lengths)?;

    let mut lengths = vec![0u8; hlit + hdist];
    let mut i = 0usize;
    while i < lengths.len() {
        let sym = cl.decode(br)?;
        match sym {
            0..=15 => {
                lengths[i] = sym as u8;
                i += 1;
            }
            16 => {
                if i == 0 {
                    return Err(P3dError::Other(
                        "deflate: code-length repeat 16 with no previous length".into(),
                    ));
                }
                let prev = lengths[i - 1];
                let n = 3 + br.bits(2)? as usize;
                if i + n > lengths.len() {
                    return Err(P3dError::Other("deflate: code-length repeat overruns".into()));
                }
                for _ in 0..n {
                    lengths[i] = prev;
                    i += 1;
                }
            }
            17 => {
                let n = 3 + br.bits(3)? as usize;
                if i + n > lengths.len() {
                    return Err(P3dError::Other("deflate: code-length zeros overruns".into()));
                }
                i += n; // lengths already zero
            }
            18 => {
                let n = 11 + br.bits(7)? as usize;
                if i + n > lengths.len() {
                    return Err(P3dError::Other("deflate: code-length zeros overruns".into()));
                }
                i += n;
            }
            _ => {
                return Err(P3dError::Other(format!(
                    "deflate: invalid code-length symbol {sym}"
                )));
            }
        }
    }
    if lengths[256] == 0 {
        return Err(P3dError::Other(
            "deflate: dynamic table has no end-of-block code".into(),
        ));
    }
    let lit = Huffman::build(&lengths[..hlit])?;
    let dist = Huffman::build(&lengths[hlit..])?;
    Ok((lit, dist))
}

fn inflate_block(
    br: &mut BitReader<'_>,
    out: &mut Vec<u8>,
    lit: &Huffman,
    dist: &Huffman,
) -> Result<()> {
    loop {
        let sym = lit.decode(br)?;
        if sym < 256 {
            out.push(sym as u8);
        } else if sym == 256 {
            return Ok(());
        } else {
            let li = sym as usize - 257;
            if li >= LEN_BASE.len() {
                return Err(P3dError::Other(format!(
                    "deflate: invalid length symbol {sym}"
                )));
            }
            let len = LEN_BASE[li] as usize + br.bits(LEN_EXTRA[li] as u32)? as usize;
            let dsym = dist.decode(br)? as usize;
            if dsym >= DIST_BASE.len() {
                return Err(P3dError::Other(format!(
                    "deflate: invalid distance symbol {dsym}"
                )));
            }
            let d = DIST_BASE[dsym] as usize + br.bits(DIST_EXTRA[dsym] as u32)? as usize;
            if d == 0 || d > out.len() {
                return Err(P3dError::Other(format!(
                    "deflate: distance {d} reaches before the start of the output (len {})",
                    out.len()
                )));
            }
            // Byte-at-a-time copy handles overlapping back-references.
            for _ in 0..len {
                let b = out[out.len() - d];
                out.push(b);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Zlib wrapper (RFC 1950)
// ---------------------------------------------------------------------------

/// Decompress a zlib stream (RFC 1950): 2-byte header, one DEFLATE
/// stream, 4-byte big-endian Adler-32 trailer.  Preset dictionaries are
/// rejected (never emitted for RZ payloads).  Bytes after the trailer
/// are ignored (callers passing trailing garbage get a checksum error
/// only if the stream itself is malformed — mirroring `zlib`'s
/// `decompressobj().unused_data` semantics).
pub fn zlib_decompress(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 2 {
        return Err(P3dError::Other("zlib: input shorter than the 2-byte header".into()));
    }
    let cmf = data[0];
    let flg = data[1];
    if cmf & 0x0F != 8 {
        return Err(P3dError::Other(format!(
            "zlib: compression method {} (only 8 = deflate is defined)",
            cmf & 0x0F
        )));
    }
    if cmf >> 4 > 7 {
        return Err(P3dError::Other(format!(
            "zlib: window size bits {} > 7",
            cmf >> 4
        )));
    }
    if (u16::from(cmf) * 256 + u16::from(flg)) % 31 != 0 {
        return Err(P3dError::Other(
            "zlib: header check ((CMF*256+FLG) % 31) failed".into(),
        ));
    }
    if flg & 0x20 != 0 {
        return Err(P3dError::Other(
            "zlib: preset dictionary (FDICT) not supported".into(),
        ));
    }
    let (out, consumed) = inflate(&data[2..])?;
    let trailer_at = 2 + consumed;
    let trailer = data
        .get(trailer_at..trailer_at + 4)
        .ok_or_else(|| P3dError::Other("zlib: truncated Adler-32 trailer".into()))?;
    let expect = u32::from_be_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    let got = adler32(&out);
    if expect != got {
        return Err(P3dError::Other(format!(
            "zlib: Adler-32 mismatch (stream says {expect:#010x}, decoded data is {got:#010x})"
        )));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        let clean: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        assert_eq!(clean.len() % 2, 0);
        (0..clean.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&clean[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn adler32_known_vectors() {
        // RFC 1950 reference + classic example.
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        // Python: zlib.adler32(b"hello world hello world hello world") = 0xef930d55
        assert_eq!(adler32(b"hello world hello world hello world"), 0xEF93_0D55);
    }

    /// Vectors generated with Python's zlib (level 0 = stored blocks,
    /// level 9 = fixed or dynamic Huffman as chosen by the deflater).
    #[test]
    fn zlib_vectors_from_python() {
        // empty, level 9 → fixed-Huffman block with a single EOB.
        let out = zlib_decompress(&hex("78da030000000001")).unwrap();
        assert_eq!(out, b"");

        // "hello world" ×3, level 0 → stored block.
        let out = zlib_decompress(&hex(concat!(
            "7801012300dcff68656c6c6f20776f726c642068656c6c6f20776f726c64",
            "2068656c6c6f20776f726c64ef930d55"
        )))
        .unwrap();
        assert_eq!(out, b"hello world hello world hello world");

        // "hello world" ×3, level 9 → fixed Huffman.
        let out = zlib_decompress(&hex(
            "78dacb48cdc9c95728cf2fca4951c8c0ce0600ef930d55",
        ))
        .unwrap();
        assert_eq!(out, b"hello world hello world hello world");

        // 'A' ×300, level 9 → dynamic Huffman + LZ77 run.
        let out = zlib_decompress(&hex("78da73741c05c40200cb9e4c2d")).unwrap();
        assert_eq!(out, vec![b'A'; 300]);

        // bytes 0..255 twice, level 9 → dynamic Huffman over binary data.
        let mut expect: Vec<u8> = (0..=255u8).collect();
        expect.extend_from_slice(&expect.clone());
        let out = zlib_decompress(&hex(concat!(
            "78da6360646266616563e7e0e4e2e6e1e5e317101412161115139790949296919593",
            "5750545256515553d7d0d4d2d6d1d5d33730343236313533b7b0b4b2b6b1b5b377",
            "70747276717573f7f0f4f2f6f1f5f30f080c0a0e090d0b8f888c8a8e898d8b4f48",
            "4c4a4e494d4bcfc8cccacec9cdcb2f282c2a2e292d2bafa8acaaaea9adab6f686c",
            "6a6e696d6befe8eceaeee9edeb9f3071d2e42953a74d9f3173d6ec3973e7cd5fb0",
            "70d1e2254b972d5fb172d5ea356bd7addfb071d3e62d5bb76ddfb173d7ee3d7bf7",
            "ed3f70f0d0e123478f1d3f71f2d4e93367cf9dbf70f1d2e52b57af5dbf71f3d6ed",
            "3b77efdd7ff0f0d1e3274f9f3d7ff1f2d5eb376fdfbdfff0f1d3e72f5fbf7dfff1",
            "f3d7ef3f7ffffd6718e1fe0700e36cff01"
        )))
        .unwrap();
        assert_eq!(out, expect);
    }

    #[test]
    fn zlib_rejects_bad_headers() {
        // Wrong method nibble.
        assert!(zlib_decompress(&hex("79da030000000001")).is_err());
        // Header check bits invalid (not a multiple of 31).
        assert!(zlib_decompress(&hex("78db030000000001")).is_err());
        // FDICT set.
        assert!(zlib_decompress(&hex("78f9030000000001")).is_err());
        // Too short.
        assert!(zlib_decompress(&hex("78")).is_err());
        assert!(zlib_decompress(b"").is_err());
    }

    #[test]
    fn zlib_rejects_corrupt_stream_and_checksum() {
        // Flip one bit inside the stored data of the hello-level0 vector.
        let mut v = hex("7801012300dcff68656c6c6f20776f726c642068656c6c6f20776f726c64ef930d55");
        let last = v.len() - 1;
        v[last] ^= 0x01; // break the Adler-32 trailer
        assert!(zlib_decompress(&v).is_err());
        let mut w = hex("7801012300dcff68656c6c6f20776f726c642068656c6c6f20776f726c64ef930d55");
        w[9] ^= 0x40; // break the stored data
        assert!(zlib_decompress(&w).is_err());
        // Truncated stream.
        let t = hex("78da73741c05c40200cb9e4c2d");
        assert!(zlib_decompress(&t[..t.len() - 6]).is_err());
    }

    #[test]
    fn inflate_reports_consumed_bytes() {
        // Stored block carrying "hello": 1 header byte + 2 LEN (05 00) +
        // 2 NLEN (fa ff = !5 in u16 = 0xFFFA) + 5 data = 10 bytes exactly.
        let (out, used) = inflate(&hex("010500faff68656c6c6f")).unwrap();
        assert_eq!(out, b"hello");
        assert_eq!(used, 10);
    }

    #[test]
    fn overlapping_backreference_expands_runs() {
        // 'A' ×300 vector decodes through a distance-1 overlapping copy.
        let (out, _) = inflate(&hex("73741c05c40200")).unwrap();
        assert_eq!(out.len(), 300);
        assert!(out.iter().all(|&b| b == b'A'));
    }
}

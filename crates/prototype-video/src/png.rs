//! Minimal deterministic PNG encoder — RGBA8, filter 0, stored-zlib IDAT.
//!
//! Zero dependencies: the zlib wrapper uses STORED (uncompressed) deflate
//! blocks, so writing a valid PNG needs no deflate implementation.  The
//! output is byte-deterministic (same input → same bytes), which is what
//! the on-device visual-regression CI wants: two renders of the same
//! frame byte-diff clean.
//!
//! Layout (PNG 1.2, single IDAT):
//! * signature `89 50 4E 47 0D 0A 1A 0A`
//! * IHDR: width, height (BE u32), bit depth 8, color type 6 (RGBA),
//!   compression 0 (deflate), filter 0 (adaptive→None), interlace 0
//! * IDAT: zlib stream `78 01`, stored deflate blocks (≤ 65 535 raw
//!   bytes each: BFINAL/BTYPE byte, LEN u16 LE, NLEN u16 LE, payload),
//!   big-endian Adler-32 of the raw scanline stream
//! * IEND: empty
//!
//! The raw stream is per-scanline `[filter byte 0x00][RGBA row]` —
//! filter type None, the only choice that needs no pixel arithmetic.
//!
//! Block 75.  This is the media-evidence encoder: the REAL decoded
//! frames (player → [`crate::yuv::yuv420_to_rgba8`]) go to PNG through
//! here; the test suite round-trips the encoder's own output with a
//! stored-zlib reader (validating every chunk CRC, the zlib header,
//! the block structure AND the Adler-32), so the bytes we ship are
//! proven parseable, not just well-formed-looking.

/// Encode `rgba` (width×height×4 bytes, row-major, RGBA order) as PNG.
pub fn png_encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 {
        return Err(format!("empty image {width}x{height}"));
    }
    let expected = w * h * 4;
    if rgba.len() != expected {
        return Err(format!(
            "rgba buffer {} bytes != {width}x{height}x4 = {expected}",
            rgba.len()
        ));
    }

    // Raw scanline stream: filter None (0) + row bytes.
    let row_len = w * 4;
    let mut raw = Vec::with_capacity(h * (1 + row_len));
    for y in 0..h {
        raw.push(0u8);
        raw.extend_from_slice(&rgba[y * row_len..(y + 1) * row_len]);
    }

    let mut out = Vec::with_capacity(raw.len() + 128);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    push_chunk(&mut out, b"IHDR", &ihdr);

    let mut idat = Vec::with_capacity(raw.len() + 16);
    idat.extend_from_slice(&[0x78, 0x01]); // zlib: deflate, 32K window unused, check bits valid
    let mut off = 0usize;
    loop {
        let take = (raw.len() - off).min(65_535);
        let last = off + take == raw.len();
        idat.push(u8::from(last)); // BFINAL<<0 | BTYPE=00 (stored)
        let len = take as u16;
        idat.extend_from_slice(&len.to_le_bytes());
        idat.extend_from_slice(&(!len).to_le_bytes());
        idat.extend_from_slice(&raw[off..off + take]);
        off += take;
        if last {
            break;
        }
    }
    idat.extend_from_slice(&adler32(&raw).to_be_bytes());
    push_chunk(&mut out, b"IDAT", &idat);
    push_chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

/// CRC-32 (IEEE 0xEDB88320, reflected) — the PNG chunk checksum.
fn crc32(data: &[u8]) -> u32 {
    let mut c: u32 = 0xFFFF_FFFF;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            let mask = (c & 1).wrapping_neg();
            c = (c >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !c
}

/// Adler-32 of the raw (uncompressed) stream — the zlib trailer.
fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

// ---------------------------------------------------------------------------
// Tests — including a stored-zlib reader that round-trips OUR output
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-only reader for the exact subset of PNG we emit: validates
    /// the signature, every chunk's CRC and length, the IHDR fields,
    /// then inflates the stored-zlib IDAT (header, block structure,
    /// Adler-32) and returns (width, height, raw scanline stream).
    fn png_read_back(png: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
        const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        if png.len() < 8 + 12 + 13 || png[..8] != SIG {
            return Err("bad signature".into());
        }
        let mut pos = 8usize;
        let (mut width, mut height) = (0u32, 0u32);
        let mut zlib: Option<&[u8]> = None;
        while pos + 8 <= png.len() {
            let len = u32::from_be_bytes(png[pos..pos + 4].try_into().unwrap()) as usize;
            let kind = &png[pos + 4..pos + 8];
            let data = png
                .get(pos + 8..pos + 8 + len)
                .ok_or("chunk data past end")?;
            let crc_off = pos + 8 + len;
            let crc = u32::from_be_bytes(png.get(crc_off..crc_off + 4).ok_or("no crc")?.try_into().unwrap());
            if crc != crc32(&png[pos + 4..crc_off]) {
                return Err(format!("{}: crc mismatch", String::from_utf8_lossy(kind)));
            }
            match kind {
                b"IHDR" => {
                    if len != 13 {
                        return Err("IHDR not 13 bytes".into());
                    }
                    width = u32::from_be_bytes(data[0..4].try_into().unwrap());
                    height = u32::from_be_bytes(data[4..8].try_into().unwrap());
                    if data[8..] != [8, 6, 0, 0, 0] {
                        return Err(format!("unexpected IHDR tail {:?}", &data[8..]));
                    }
                }
                b"IDAT" => zlib = Some(data),
                b"IEND" => break,
                _ => return Err(format!("unexpected chunk {}", String::from_utf8_lossy(kind))),
            }
            pos = crc_off + 4;
        }
        let z = zlib.ok_or("no IDAT")?;
        if z.len() < 6 || z[0] != 0x78 || (u16::from(z[0]) << 8 | u16::from(z[1])) % 31 != 0 {
            return Err("bad zlib header".into());
        }
        let mut raw = Vec::new();
        let mut off = 2usize;
        loop {
            if off >= z.len() - 4 {
                return Err("stored blocks run past zlib trailer".into());
            }
            let bfinal = z[off] & 1 == 1;
            if z[off] & 0b110 != 0 {
                return Err("block is not stored".into());
            }
            let len = u16::from_le_bytes(z[off + 1..off + 3].try_into().unwrap());
            let nlen = u16::from_le_bytes(z[off + 3..off + 5].try_into().unwrap());
            if len != !nlen {
                return Err("NLEN mismatch".into());
            }
            let len = len as usize;
            let payload = z.get(off + 5..off + 5 + len).ok_or("block past end")?;
            raw.extend_from_slice(payload);
            off += 5 + len;
            if bfinal {
                break;
            }
        }
        if off != z.len() - 4 {
            return Err("adler trailer not immediately after last block".into());
        }
        let adler = u32::from_be_bytes(z[z.len() - 4..].try_into().unwrap());
        if adler != adler32(&raw) {
            return Err("adler mismatch".into());
        }
        Ok((width, height, raw))
    }

    fn round_trip(w: u32, h: u32) {
        let n = (w as usize) * (h as usize) * 4;
        let img: Vec<u8> = (0..n).map(|i| (i % 251) as u8).collect();
        let png = png_encode_rgba(w, h, &img).expect("encode");
        let (rw, rh, raw) = png_read_back(&png).expect("read back");
        assert_eq!((rw, rh), (w, h));
        // strip the per-scanline filter bytes and compare
        let row_len = w as usize * 4;
        for y in 0..h as usize {
            assert_eq!(raw[y * (1 + row_len)], 0, "row {y}: filter byte");
            assert_eq!(
                &raw[y * (1 + row_len) + 1..(y + 1) * (1 + row_len)],
                &img[y * row_len..(y + 1) * row_len]
            );
        }
    }

    #[test]
    fn round_trip_small_image() {
        round_trip(3, 2);
    }

    #[test]
    fn round_trip_exact_one_stored_block() {
        // 64x64x4 = 16384 raw bytes + 64 filter bytes = 16448 < 65535
        round_trip(64, 64);
    }

    #[test]
    fn round_trip_multiple_stored_blocks() {
        // 256x256x4 = 262144 + 256 filter bytes → 4 stored blocks
        round_trip(256, 256);
    }

    #[test]
    fn deterministic_bytes() {
        let img: Vec<u8> = (0..16 * 16 * 4).map(|i| (i * 7) as u8).collect();
        let a = png_encode_rgba(16, 16, &img).expect("a");
        let b = png_encode_rgba(16, 16, &img).expect("b");
        assert_eq!(a, b, "same input must give byte-identical PNG");
    }

    #[test]
    fn rejects_bad_buffers() {
        assert!(png_encode_rgba(0, 8, &[]).is_err());
        assert!(png_encode_rgba(4, 4, &[0u8; 63]).is_err());
        let ok = vec![0u8; 4 * 4 * 4];
        assert!(png_encode_rgba(4, 4, &ok).is_ok());
    }

    #[test]
    fn known_answer_ihdr_bytes() {
        // Pin the first bytes of a 2x1 black image: signature + IHDR
        // length/type + width/height/color spec.
        let png = png_encode_rgba(2, 1, &[0, 0, 0, 255, 1, 2, 3, 255]).expect("enc");
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[16..20], &2u32.to_be_bytes());
        assert_eq!(&png[20..24], &1u32.to_be_bytes());
        assert_eq!(&png[24..29], &[8, 6, 0, 0, 0]);
        // last chunk is IEND (len 0, crc 0xAE426082)
        let tail = &png[png.len() - 12..];
        assert_eq!(&tail[..4], &0u32.to_be_bytes());
        assert_eq!(&tail[4..8], b"IEND");
        assert_eq!(&tail[8..12], &0xAE42_6082u32.to_be_bytes());
    }
}

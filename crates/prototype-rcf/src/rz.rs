//! RZ compressed-payload wrapper — the compression used for the art
//! data inside Prototype's `.rcf` archives.
//!
//! # Format (PROVEN)
//!
//! Empirically verified against **all 2 219 RZ members** of the user's
//! own PC `art.rcf` (601 MB compressed; member sizes 33 B .. 16 MB;
//! decompressed up to 24.5 MB), cross-checked with Python `zlib`:
//!
//! ```text
//! [ 0.. 3]  magic "RZ\0\0"            (52 5A 00 00)
//! [ 4.. 7]  u32  zero                 (0 in every observed member)
//! [ 8..11]  u32  decompressed_size    (exact output byte length)
//! [12..15]  u32  zero                 (0 in every observed member)
//! [16..  ]  ONE RFC 1950 zlib stream  (78 da header, Adler-32 trailer)
//! ```
//!
//! Every observed member's zlib stream ends exactly at the end of the
//! payload, and every decompressed length equals the declared size
//! field.  The decompressed payloads are `P3D\xff` files (geometry,
//! textures, materials, shaders), `GFX` UI movies, and other game
//! data.  Members that are NOT RZ-wrapped (e.g. the 19 raw `DDS `
//! members of `art.rcf`) carry their content directly.
//!
//! The two zero fields are rejected when non-zero: their semantics are
//! unknown and failing closed is safer than silently misreading a
//! future variant.

use prototype_common::{zlib, P3dError, Result, Reader};

/// Magic bytes `RZ\0\0`.
pub const RZ_MAGIC: [u8; 4] = [0x52, 0x5A, 0x00, 0x00];

/// Fixed header size: magic + three u32 fields.
pub const RZ_HEADER_SIZE: usize = 16;

/// True when `data` begins with the RZ magic.
pub fn is_rz(data: &[u8]) -> bool {
    data.len() >= 4 && data[..4] == RZ_MAGIC
}

/// Decompress an RZ payload (magic + 16-byte header + zlib stream).
///
/// Verifies: magic, the two zero fields, the zlib header + Adler-32
/// trailer (via [`zlib::zlib_decompress`]), and that the decompressed
/// length equals the declared `decompressed_size` field.
pub fn decompress(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < RZ_HEADER_SIZE {
        return Err(P3dError::Eof {
            offset: data.len(),
            needed: RZ_HEADER_SIZE - data.len(),
        });
    }
    let mut r = Reader::new(data);
    let magic = r.read_bytes(4)?;
    if magic != RZ_MAGIC {
        return Err(P3dError::BadMagic {
            offset: 0,
            expected: &RZ_MAGIC,
            got: [magic[0], magic[1], magic[2], magic[3]],
        });
    }
    let zero0 = r.read_u32()?;
    let declared_size = r.read_u32()?;
    let zero1 = r.read_u32()?;
    if zero0 != 0 || zero1 != 0 {
        return Err(P3dError::Other(format!(
            "rz: unknown header fields are non-zero (a={zero0}, c={zero1}) — \
             refusing to guess the layout"
        )));
    }
    let out = zlib::zlib_decompress(&data[RZ_HEADER_SIZE..])?;
    if out.len() != declared_size as usize {
        return Err(P3dError::Other(format!(
            "rz: decompressed {} byte(s) but the header declared {declared_size}",
            out.len()
        )));
    }
    Ok(out)
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

    /// Byte-exact round-trips: `.rz` member → decompress → the committed
    /// `.p3d` produced from the same member by Python's zlib.
    #[test]
    fn decompresses_every_real_rz_fixture_byte_exactly() {
        for name in [
            "rz_tiny",
            "rz_small",
            "art_texture_tut078",
            "art_barrier_os002",
        ] {
            let rz = fixture(&format!("{name}.rz"));
            let expect = fixture(&format!("{name}.p3d"));
            assert!(is_rz(&rz), "{name}.rz must carry the RZ magic");
            let out = decompress(&rz)
                .unwrap_or_else(|e| panic!("{name}.rz failed to decompress: {e}"));
            assert_eq!(out, expect, "{name}.rz must decode byte-exactly");
        }
    }

    /// The tiny member's declared size is 12 and the output is an
    /// EMPTY P3D file (header only) — pin the exact bytes.
    #[test]
    fn tiny_member_is_an_empty_p3d() {
        let rz = fixture("rz_tiny.rz");
        let out = decompress(&rz).unwrap();
        assert_eq!(out.len(), 12);
        assert_eq!(out[..4], [0x50, 0x33, 0x44, 0xFF]); // P3D\xff
        assert_eq!(&out[4..8], &12u32.to_le_bytes()); // version
        assert_eq!(&out[8..12], &12u32.to_le_bytes()); // file_size
    }

    /// The real art model member (84 KB decompressed) proves the
    /// inflater handles dynamic-Huffman streams of real game data.
    #[test]
    fn barrier_model_declares_its_true_size() {
        let rz = fixture("art_barrier_os002.rz");
        let out = decompress(&rz).unwrap();
        assert_eq!(out.len(), 83978);
        assert_eq!(&out[..4], &[0x50, 0x33, 0x44, 0xFF]);
    }

    #[test]
    fn rejects_bad_magic_truncation_and_lies() {
        // Wrong magic.
        let mut v = fixture("rz_small.rz");
        v[0] = b'X';
        assert!(decompress(&v).is_err());
        // Truncated header.
        let v = fixture("rz_small.rz");
        assert!(decompress(&v[..10]).is_err());
        // Corrupted zlib body (flip a bit deep inside the stream).
        let mut v = fixture("rz_small.rz");
        let mid = RZ_HEADER_SIZE + v.len() / 3;
        v[mid] ^= 0x40;
        assert!(decompress(&v).is_err());
        // Declared-size lie: patch the size field to a wrong value.
        let mut v = fixture("rz_small.rz");
        v[8..12].copy_from_slice(&12345u32.to_le_bytes());
        assert!(decompress(&v).is_err());
        // Non-zero unknown field.
        let mut v = fixture("rz_small.rz");
        v[4..8].copy_from_slice(&1u32.to_le_bytes());
        assert!(decompress(&v).is_err());
    }

    #[test]
    fn is_rz_detects_the_magic() {
        assert!(is_rz(&fixture("rz_tiny.rz")));
        assert!(!is_rz(&fixture("rz_tiny.p3d")));
        assert!(!is_rz(b"RZ")); // too short
        assert!(!is_rz(b"RZ\x01\x00"));
    }
}

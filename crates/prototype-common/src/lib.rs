//! prototype-common
//! ================
//! Shared types and byte-reading primitives used by every crate in
//! the prototype-rs workspace.  All file formats reverse-engineered
//! here are **little-endian** (the original binaries were compiled
//! for x86 / Windows).
//!
//! Two primitives anchor the rest of the workspace:
//!   * [`Reader`] — a bounds-checked cursor over `&[u8]` data with
//!     explicit little-endian readers for u8/u16/u32/u64, Pascal
//!     strings (1-byte length prefix), and u32-length-prefixed
//!     strings.
//!   * [`P3dError`] — a single `thiserror` enum that every crate
//!     converts into via `From`.  No panics on malformed input.

#![forbid(unsafe_code)]

//! # Modules
//!
//! * [`zlib`] — minimal RFC 1950/1951 decoder (Adler-32, stored /
//!   fixed / dynamic Huffman blocks), used by the RZ payload wrapper.

pub mod zlib;

use thiserror::Error;

// ---------------------------------------------------------------------------
// Error model
// ---------------------------------------------------------------------------

/// Unified error type for every prototype-rs parser.  Parsers must
/// never panic on malformed input — they return `Err(P3dError::...)`
/// with a precise reason and the offset at which parsing failed.
#[derive(Debug, Error)]
pub enum P3dError {
    #[error("unexpected end of input at offset {offset}: needed {needed} more byte(s)")]
    Eof { offset: usize, needed: usize },

    #[error("invalid magic at offset {offset}: expected {expected:?}, got {got:?}")]
    BadMagic {
        offset: usize,
        expected: &'static [u8],
        got: [u8; 4],
    },

    #[error("invalid magic (ascii) at offset {offset}: expected {expected:?}, got {got:?}")]
    BadMagicAscii {
        offset: usize,
        expected: &'static str,
        got: String,
    },

    #[error("invalid u32 length prefix at offset {offset}: value {value} exceeds remaining {remaining}")]
    BadLength { offset: usize, value: u32, remaining: usize },

    #[error("invalid u8 length prefix at offset {offset}: value {value} exceeds remaining {remaining}")]
    BadLengthU8 { offset: usize, value: u8, remaining: usize },

    #[error("invalid utf-8 at offset {offset}: {source}")]
    BadUtf8 {
        offset: usize,
        #[source]
        source: std::str::Utf8Error,
    },

    #[error("invalid pe signature at offset {offset}: expected 'PE\\0\\0'")]
    BadPeSignature { offset: usize },

    #[error("unsupported machine 0x{machine:04x} (only i386 0x014c supported)")]
    UnsupportedMachine { machine: u16 },

    #[error("section {name:?} overlaps previously-mapped section")]
    SectionOverlap { name: String },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("parse failed: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, P3dError>;

// ---------------------------------------------------------------------------
// Reader — bounds-checked cursor over a byte slice
// ---------------------------------------------------------------------------

/// A bounds-checked cursor over `&[u8]` data.  Every primitive reads
/// little-endian and advances `self.pos`.  All methods return
/// [`P3dError::Eof`] rather than panicking when the cursor runs
/// past the end.
#[derive(Debug, Clone, Copy)]
pub struct Reader<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    #[inline]
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    #[inline]
    pub fn from_at(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }

    #[inline]
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    #[inline]
    pub fn at_end(&self) -> bool {
        self.pos >= self.data.len()
    }

    #[inline]
    pub fn peek_u8(&self) -> Result<u8> {
        self.data
            .get(self.pos)
            .copied()
            .ok_or_else(|| P3dError::Eof { offset: self.pos, needed: 1 })
    }

    #[inline]
    pub fn read_u8(&mut self) -> Result<u8> {
        let b = self.peek_u8()?;
        self.pos += 1;
        Ok(b)
    }

    #[inline]
    pub fn read_u16(&mut self) -> Result<u16> {
        if self.remaining() < 2 {
            return Err(P3dError::Eof { offset: self.pos, needed: 2 });
        }
        let v = u16::from_le_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }

    #[inline]
    pub fn read_u32(&mut self) -> Result<u32> {
        if self.remaining() < 4 {
            return Err(P3dError::Eof { offset: self.pos, needed: 4 });
        }
        let v = u32::from_le_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ]);
        self.pos += 4;
        Ok(v)
    }

    #[inline]
    pub fn read_u64(&mut self) -> Result<u64> {
        if self.remaining() < 8 {
            return Err(P3dError::Eof { offset: self.pos, needed: 8 });
        }
        let v = u64::from_le_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
            self.data[self.pos + 4],
            self.data[self.pos + 5],
            self.data[self.pos + 6],
            self.data[self.pos + 7],
        ]);
        self.pos += 8;
        Ok(v)
    }

    /// Read `n` bytes verbatim and return them as a sub-slice.
    #[inline]
    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(P3dError::Eof { offset: self.pos, needed: n });
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    /// Skip `n` bytes.
    #[inline]
    pub fn skip(&mut self, n: usize) -> Result<()> {
        if self.remaining() < n {
            return Err(P3dError::Eof { offset: self.pos, needed: n });
        }
        self.pos += n;
        Ok(())
    }

    /// Read a Pascal string with a 1-byte length prefix (Pure3D inline names).
    /// The original format sometimes null-terminates within the length-prefixed
    /// region; we trim at the first NUL when interpreting.
    pub fn read_pascal_string_u8(&mut self) -> Result<String> {
        let len = self.read_u8()? as usize;
        if self.remaining() < len {
            return Err(P3dError::BadLengthU8 {
                offset: self.pos - 1,
                value: len as u8,
                remaining: self.remaining(),
            });
        }
        let raw = &self.data[self.pos..self.pos + len];
        self.pos += len;
        // Trim at first NUL — P3D frequently embeds a trailing \0 inside
        // the length-prefixed region.
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        Ok(std::str::from_utf8(&raw[..end])
            .map(|s| s.to_owned())
            .map_err(|e| P3dError::BadUtf8 { offset: self.pos - len, source: e })?)
    }

    /// Read a Pascal string with a u32 length prefix (used in P3D META
    /// sections, RCF file-lists, and Lua string constants).
    pub fn read_pascal_string_u32(&mut self) -> Result<String> {
        let len = self.read_u32()? as usize;
        if self.remaining() < len {
            return Err(P3dError::BadLength {
                offset: self.pos - 4,
                value: len as u32,
                remaining: self.remaining(),
            });
        }
        let raw = &self.data[self.pos..self.pos + len];
        self.pos += len;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        Ok(std::str::from_utf8(&raw[..end])
            .map(|s| s.to_owned())
            .map_err(|e| P3dError::BadUtf8 { offset: self.pos - len, source: e })?)
    }

    /// Read a NUL-terminated C string starting at the current cursor
    /// position; advance past the NUL.
    pub fn read_cstring(&mut self) -> Result<String> {
        let start = self.pos;
        let end = self.data[start..]
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| P3dError::Eof { offset: start, needed: 1 })?;
        let s = std::str::from_utf8(&self.data[start..start + end])
            .map(|s| s.to_owned())
            .map_err(|e| P3dError::BadUtf8 { offset: start, source: e })?;
        self.pos = start + end + 1;
        Ok(s)
    }

    /// Read `n` bytes interpreted as ASCII (latin1-safe): non-printable
    /// bytes become '.'.  Used for magic-string checks.
    pub fn read_ascii(&mut self, n: usize) -> Result<String> {
        let raw = self.read_bytes(n)?;
        Ok(raw
            .iter()
            .map(|&b| if (32..127).contains(&b) { b as char } else { '.' })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Magic constants
// ---------------------------------------------------------------------------

/// 4-byte magic `50 33 44 ff` ("P3D\xff") prefixing every Pure3D file.
pub const P3D_MAGIC: [u8; 4] = *b"P3D\xff";

/// Pure3D file format version observed in all Prototype PC samples: 12.
pub const P3D_VERSION: u32 = 12;

/// 32-byte ASCII header (NUL-padded) prefixing every Radical "ATG CORE
/// CEMENT LIBRARY" file (`*.rcf`).
pub const RCF_MAGIC: &[u8] = b"ATG CORE CEMENT LIBRARY";

/// Lua 5.1 bytecode magic (`\x1bLua` — 4 bytes; the version byte
/// 0x51 is read separately as `version:u8`).
pub const LUA_MAGIC: [u8; 4] = [0x1b, b'L', b'u', b'a'];

/// DOS/PE MZ magic.
pub const MZ_MAGIC: [u8; 2] = *b"MZ";

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_little_endian_u32() {
        let mut r = Reader::new(&[0x78, 0x56, 0x34, 0x12, 0x00]);
        assert_eq!(r.read_u32().unwrap(), 0x12345678);
        assert!(r.read_u8().is_ok());
        assert!(r.read_u8().is_err());
    }

    #[test]
    fn pascal_string_u8_trims_at_null() {
        let mut r = Reader::new(&[0x05, b'A', b'B', b'C', 0x00, b'X']);
        let s = r.read_pascal_string_u8().unwrap();
        assert_eq!(s, "ABC");
        assert_eq!(r.pos, 6);
    }

    #[test]
    fn pascal_string_u32_round_trip() {
        let mut bytes = vec![];
        bytes.extend_from_slice(&5u32.to_le_bytes());
        bytes.extend_from_slice(b"hello");
        let mut r = Reader::new(&bytes);
        assert_eq!(r.read_pascal_string_u32().unwrap(), "hello");
    }

    #[test]
    fn eof_reports_offset() {
        // A failed read does NOT advance the cursor (it's a bounds
        // check that fails before any state mutation).  This mirrors
        // the original engine's zero-cost EOF path.
        let mut r = Reader::new(&[1, 2, 3]);
        let err = r.read_u32().unwrap_err();
        match err {
            P3dError::Eof { offset, needed } => {
                assert_eq!(offset, 0, "offset should be the cursor pos at the failed read");
                assert_eq!(needed, 4);
            }
            _ => panic!("expected Eof, got {err:?}"),
        }
        // The second read fails at the SAME offset (cursor did not advance).
        let err2 = r.read_u32().unwrap_err();
        match err2 {
            P3dError::Eof { offset, needed } => {
                assert_eq!(offset, 0);
                assert_eq!(needed, 4);
            }
            _ => panic!(),
        }
        // After a successful read of available bytes, EOF reports the
        // advanced offset.
        let _ = r.read_u8().unwrap(); // pos = 1
        let _ = r.read_u8().unwrap(); // pos = 2
        let _ = r.read_u8().unwrap(); // pos = 3
        let err3 = r.read_u8().unwrap_err();
        match err3 {
            P3dError::Eof { offset, needed } => {
                assert_eq!(offset, 3);
                assert_eq!(needed, 1);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn cstring_advances_past_null() {
        let mut r = Reader::new(b"hello\0world\0extra");
        assert_eq!(r.read_cstring().unwrap(), "hello");
        assert_eq!(r.read_cstring().unwrap(), "world");
    }
}

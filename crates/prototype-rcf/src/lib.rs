//! prototype-rcf
//! ==============
//! Radical "ATG CORE CEMENT LIBRARY" container parser — the `.rcf`
//! archive format used by Prototype (2009, Activision / Radical
//! Entertainment) for scripts, shaders, audio, art, movies and WOI
//! (World of Interest) bundles.
//!
//! ## Evidence (from `shaders.rcf` 10 793 bytes & `scripts.rcf` 186 638 bytes)
//!
//! `shaders.rcf` first 80 bytes, decoded as little-endian u32 fields:
//!
//! ```text
//! +  0..31 : magic     "ATG CORE CEMENT LIBRARY\0\0\0\0\0\0\0\0\0"
//! + 32..35 : 0x01000102  (version field — u16 major=2, u16 minor=1, plus
//!                         a flag byte 0x01)
//! + 36..39 : 0x0000003c  = 60   dir_offset
//! + 40..43 : 0x0000000c  = 12   dir_size_in_bytes   (= 1 entry × 12 B)
//! + 44..47 : 0x00000800  = 2048 sector_size
//! + 48..51 : 0x00000030  = 48   name_table_offset  (unused — see below)
//! + 52..55 : 0x00000000  = 0    reserved
//! + 56..59 : 0x00000001  = 1    entry_count
//! + 60..71 : directory (12 bytes per entry):
//!              hash        = 0xd1d2c18d
//!              file_offset = 4096 (= 2 × sector_size)
//!              file_size   = 6697
//! ```
//!
//! `scripts.rcf` decodes identically except `dir_size_in_bytes = 84`
//! (= 7 entries × 12 B), `name_table_offset = 316`, `entry_count = 7`.
//!
//! ### Decoded layout (PROVEN for both sample files)
//!
//! * **Header (60 bytes)**:
//!   * `[0..31]`   32-byte ASCII magic, NUL-padded: `"ATG CORE CEMENT LIBRARY"`
//!   * `[32..35]`  version field (constant `02 01 00 01` in both samples)
//!   * `[36..39]`  u32 `dir_offset`        — always 60 in observed files
//!   * `[40..43]`  u32 `dir_size_in_bytes` — `entry_count × 12`
//!   * `[44..47]`  u32 `sector_size`       — always 2048 (0x800) in observed files
//!   * `[48..51]`  u32 `name_table_offset` — points to a region that is
//!     **all-zero** in both samples (likely reserved for an optional
//!     filename table that the build pipeline does not emit for these
//!     archives; the engine looks up entries by hash).
//!   * `[52..55]`  u32 reserved (always 0)
//!   * `[56..59]`  u32 `entry_count`
//! * **Directory**: at `dir_offset`, `entry_count` × 12 bytes per entry.
//!   Each entry is:
//!   * `name_hash:u32`  — a 32-bit hash of the entry's logical name.
//!     Not a plain CRC32 of common filenames (verified — see
//!     `tests::hash_is_not_plain_crc32`); likely a custom Radical
//!     hash function or a CRC of the canonical path.
//!   * `file_offset:u32` — byte offset of the payload, aligned to
//!     `sector_size`.
//!   * `file_size:u32`   — exact byte length of the payload.
//! * **Payload region**: file payloads at their `file_offset`.  Each
//!   payload may itself be a `.p3d` chunk, a compiled Lua 5.1 bytecode
//!   file, or a plain-text file (scripts.rcf stores the credits text
//!   in this way — `EXECUTIVE PRODUCER\r\n      Tim Bennison...`).
//!
//! ### Confidence
//!
//! * Header & directory layout: **PROVEN** (matches in 2 of 2 samples,
//!   entry counts and file sizes verified by computing end offsets).
//! * `name_hash` algorithm: **PROVEN** (recovered 2026-11-03 by
//!   disassembling `prototypeenginef.dll`; see [`hash_name`]).
//!   Directory entries are sorted ascending by hash and the engine
//!   binary-searches them (see `tests::entries_are_sorted_by_hash`).

#![forbid(unsafe_code)]

use prototype_common::{P3dError, Reader, Result, RCF_MAGIC};

pub mod name_table;
pub mod named;
pub mod rz;
pub mod shard_index;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// ASCII magic bytes (without NUL padding).
pub const RCF_MAGIC_ASCII: &str = "ATG CORE CEMENT LIBRARY";

/// 32-byte magic length including NUL padding.
pub const RCF_MAGIC_LEN: usize = 32;

/// Fixed header size before the directory starts.
pub const RCF_HEADER_SIZE: usize = 60;

/// Each directory entry is exactly 12 bytes: hash + offset + size.
pub const RCF_ENTRY_SIZE: usize = 12;

/// Default sector alignment used by Radical's build pipeline.
pub const RCF_DEFAULT_SECTOR_SIZE: u32 = 2048;

/// Version field constant observed in Prototype RCF files.
pub const RCF_VERSION_FIELD: u32 = 0x0100_0102;

// ---------------------------------------------------------------------------
// Name hash (PROVEN — recovered from prototypeenginef.dll)
// ---------------------------------------------------------------------------

/// Radical's cement-library **name hash** — the exact algorithm used
/// to build the `name_hash` field of RCF directory entries.
///
/// ## How it was recovered (PROVEN, no decompiler needed)
///
/// 1. The string `"ATG CORE CEMENT LIBRARY"` lives at VA
///    `0x10E47F34` in `prototypeenginef.dll`'s `.rdata`; its single
///    `.text` xref sits inside a **binary search** over 12-byte
///    records (function at `0x106502C0`: `shr ecx,1` midpoint,
///    `cmp edx, esi` against the requested hash, `imul eax, edi`
///    stride indexing) — i.e. `CementLibrary::FindEntry`.
/// 2. Its caller chain leads to `Open(name)` wrappers at
///    `0x10650CF0` / `0x10643ED0`, which strip one leading `'\'`
///    (byte `0x5C`) and call **`0x1063A5D0(str, seed)`**.
/// 3. `0x1063A5D0` is the hash loop (capstone decode):
///
/// ```text
///   esi = seed (0 at the RCF layer)
///   loop over chars until NUL:
///       eax = movsx (i8)c          ; SIGN-EXTEND the byte
///       if (signed)c < 0x61:  eax += 0x20
///       edi = (esi << 5) - esi     ; hash * 31
///       esi = edi + eax            ; hash = hash*31 + c
///   returns esi (eax)
///   ; then a debug hook [0x110BC59C] is called — it does not
///   ; modify the result (default target 0x1063A450, a registry).
/// ```
///
/// In C: `h = h*31 + ((i8)c < 0x61 ? (i8)c + 0x20 : (i8)c)` — a
/// djb2-family ×31 hash with an asymmetric case-fold that maps every
/// byte below `'a'` (including `_ . \ / digits`) 0x20 higher.
///
/// ## Verification (two independent real pairs)
///
/// | Name | Hash | Source of truth |
/// |---|---|---|
/// | `art\hud\fonts_latin.gfx` | `0x78630CB6` | X360 research (Nazatric/PrototypeRecomp) — same cement library on both platforms |
/// | `scripts\statsachievements_tod.p3d` | `0xD1815F8C` | cracked locally: scripts.rcf entry 6, payload byte-identical to the standalone `statsachievements_tod.p3d` |
/// | `scripts\frevents.lua` | `0x34E40DC3` | cracked: entry 2, payload byte-identical to standalone `frevents.lua` |
/// | `scripts\credits_english.txt` | `0xCE310C99` | cracked: entry 5 |
/// | `scripts\credits_french.txt` | `0xA99C4C55` | cracked: entry 4 |
/// | `scripts\credits_italian.txt` | `0x437F3629` | cracked: entry 3 |
/// | `scripts\credits_spanish.txt` | `0x316D751D` | cracked: entry 1 |
///
/// Every hash in the two available real archives except two
/// (`scripts.rcf` entry 0, a 2 318-byte Lua; `shaders.rcf` entry 0,
/// a 6 697-byte shader P3D) is now resolved to a real name.
///
/// Entry names are full archive paths with backslash separators,
/// lowercase (case is irrelevant to the hash by construction).
pub fn hash_name(name: &str) -> u32 {
    hash_bytes(name.as_bytes())
}

/// Raw-byte variant of [`hash_name`] (the engine hashes NUL-terminated
/// C strings; this version hashes exactly the bytes given).
pub fn hash_bytes(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0; // RCF layer always seeds 0 (push 0 at 0x10643EDC)
    for &b in bytes {
        // movsx eax, al  — the engine sign-extends each byte
        let mut c = b as i8 as i32;
        // cmp al, 0x61 / jge / add eax, 0x20
        if c < 0x61 {
            c += 0x20;
        }
        // edi = (esi << 5) - esi + eax
        h = h.wrapping_mul(31).wrapping_add(c as u32);
    }
    h
}

/// Entry names recovered for the archives shipped with this repo
/// (cracked with [`hash_name`] against the real directory hashes).
/// The full game's other archives can be indexed the same way.
pub const KNOWN_ENTRY_NAMES: &[&str] = &[
    "scripts\\statsachievements_tod.p3d",
    "scripts\\frevents.lua",
    "scripts\\credits_english.txt",
    "scripts\\credits_french.txt",
    "scripts\\credits_italian.txt",
    "scripts\\credits_spanish.txt",
];

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

/// 60-byte RCF file header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RcfHeader {
    /// ASCII magic, NUL-padded to 32 bytes.  Always
    /// `"ATG CORE CEMENT LIBRARY"` in Prototype.
    pub magic: String,
    /// Version field, observed to be `0x0100_0102`.  Decoded as
    /// `u16 version_major = 2`, `u16 version_minor = 1`, plus a
    /// flag byte = 0x01.  Stored raw here; consumers may pattern-match.
    pub version_field: u32,
    /// Byte offset of the directory table (observed: 60).
    pub dir_offset: u32,
    /// Size of the directory in bytes = `entry_count × 12`.
    pub dir_size_in_bytes: u32,
    /// Sector size for file payload alignment (observed: 2048).
    pub sector_size: u32,
    /// Offset of an (optional) filename table.  In the two observed
    /// Prototype samples this points to a zero-filled region — the
    /// engine looks files up by `name_hash`, not by name.  Retained
    /// for forward-compatibility with archives that DO include names.
    pub name_table_offset: u32,
    /// Reserved field (always 0 in observed samples).
    pub reserved: u32,
    /// Number of directory entries (= `dir_size_in_bytes / 12`).
    pub entry_count: u32,
}

impl RcfHeader {
    pub fn parse(data: &[u8]) -> Result<(Self, Reader<'_>)> {
        let mut r = Reader::new(data);
        let magic_raw = r.read_bytes(RCF_MAGIC_LEN)?;
        let magic_str = std::str::from_utf8(magic_raw)
            .map(|s| s.trim_end_matches('\0').to_string())
            .map_err(|e| P3dError::BadUtf8 { offset: 0, source: e })?;
        if !magic_str.as_bytes().starts_with(RCF_MAGIC) {
            return Err(P3dError::BadMagicAscii {
                offset: 0,
                expected: "ATG CORE CEMENT LIBRARY",
                got: magic_str,
            });
        }
        let version_field = r.read_u32()?;
        let dir_offset = r.read_u32()?;
        let dir_size_in_bytes = r.read_u32()?;
        let sector_size = r.read_u32()?;
        let name_table_offset = r.read_u32()?;
        let reserved = r.read_u32()?;
        let entry_count = r.read_u32()?;
        // Cross-check dir_size_in_bytes == entry_count * 12
        if entry_count != dir_size_in_bytes / RCF_ENTRY_SIZE as u32 {
            return Err(P3dError::Other(format!(
                "rcf dir_size_in_bytes {} is not a multiple of 12 (= entry_count {} × 12)",
                dir_size_in_bytes, entry_count
            )));
        }
        Ok((
            Self {
                magic: magic_str,
                version_field,
                dir_offset,
                dir_size_in_bytes,
                sector_size,
                name_table_offset,
                reserved,
                entry_count,
            },
            r,
        ))
    }
}

// ---------------------------------------------------------------------------
// Directory entry (12 bytes)
// ---------------------------------------------------------------------------

/// One 12-byte directory entry: hash + offset + size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RcfDirEntry {
    /// [`hash_name`] of the entry's archive path (e.g.
    /// `scripts\\frevents.lua`).  **PROVEN** — see [`hash_name`].
    pub name_hash: u32,
    /// Absolute byte offset of the payload (aligned to `sector_size`).
    pub file_offset: u32,
    /// Exact byte length of the payload.
    pub file_size: u32,
}

impl RcfDirEntry {
    pub fn parse(r: &mut Reader<'_>) -> Result<Self> {
        let name_hash = r.read_u32()?;
        let file_offset = r.read_u32()?;
        let file_size = r.read_u32()?;
        Ok(Self {
            name_hash,
            file_offset,
            file_size,
        })
    }
}

// ---------------------------------------------------------------------------
// Whole-file parser
// ---------------------------------------------------------------------------

/// Parsed RCF archive: header + directory + borrowed payload slice.
#[derive(Debug, Clone)]
pub struct RcfArchive<'a> {
    pub header: RcfHeader,
    pub entries: Vec<RcfDirEntry>,
    pub data: &'a [u8],
}

impl<'a> RcfArchive<'a> {
    /// Parse a full RCF archive from a byte slice.
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let (header, mut r) = RcfHeader::parse(data)?;
        if header.dir_offset as usize > data.len() {
            return Err(P3dError::Other(format!(
                "rcf dir_offset {} beyond file size {}",
                header.dir_offset,
                data.len()
            )));
        }
        r.pos = header.dir_offset as usize;
        let mut entries = Vec::with_capacity(header.entry_count.min(65536) as usize);
        for _ in 0..header.entry_count {
            entries.push(RcfDirEntry::parse(&mut r)?);
        }
        Ok(Self {
            header,
            entries,
            data,
        })
    }

    /// Borrow the raw payload bytes for one entry.  Returns `Err` if
    /// the entry's offset+size exceeds the file.
    pub fn payload(&self, entry: &RcfDirEntry) -> Result<&'a [u8]> {
        let start = entry.file_offset as usize;
        let end = start.checked_add(entry.file_size as usize)
            .ok_or_else(|| P3dError::Other("rcf entry overflows usize".into()))?;
        if end > self.data.len() {
            return Err(P3dError::BadLength {
                offset: start,
                value: entry.file_size,
                remaining: self.data.len().saturating_sub(start),
            });
        }
        Ok(&self.data[start..end])
    }

    /// Look up an entry by its opaque `name_hash`.  Returns the first
    /// matching entry (the format does not document duplicate-hash
    /// behaviour; the build pipeline is observed to emit unique hashes).
    pub fn lookup_by_hash(&self, hash: u32) -> Option<&RcfDirEntry> {
        self.entries.iter().find(|e| e.name_hash == hash)
    }

    /// Look up an entry by name — the exact operation the original
    /// engine performs when opening a game file from a cement library.
    ///
    /// Mirrors the engine faithfully:
    /// * up to two leading `\\` are stripped (one in `Open` at
    ///   `0x10650CF0`, one in `0x10643ED0`),
    /// * the remainder is hashed with [`hash_name`] (seed 0),
    /// * the directory — sorted ascending by hash, like the real
    ///   archives — is binary-searched (`partition_point`).
    pub fn lookup(&self, name: &str) -> Option<&RcfDirEntry> {
        let s = name
            .strip_prefix('\\')
            .and_then(|s| s.strip_prefix('\\').or(Some(s)))
            .unwrap_or(name);
        let hash = hash_name(s);
        let idx = self.entries.partition_point(|e| e.name_hash < hash);
        self.entries.get(idx).filter(|e| e.name_hash == hash)
    }

    /// Borrow the payload of the entry with the given archive name.
    pub fn payload_by_name(&self, name: &str) -> Option<&'a [u8]> {
        self.lookup(name).and_then(|e| self.payload(e).ok())
    }

    /// Detect what kind of payload an entry contains by inspecting the
    /// first few magic bytes.
    pub fn payload_kind(&self, entry: &RcfDirEntry) -> Result<PayloadKind> {
        let p = self.payload(entry)?;
        Ok(PayloadKind::sniff(p))
    }

    /// Iterate over `(entry, payload)` pairs for every entry.
    pub fn iter(&self) -> impl Iterator<Item = Result<(&RcfDirEntry, &'a [u8])>> {
        self.entries.iter().map(move |e| {
            let p = self.payload(e)?;
            Ok((e, p))
        })
    }

    /// Payload for one entry, transparently **RZ-decompressed** when
    /// the member is an RZ-wrapped payload.  Every art-archive P3D
    /// member observed in the real PC `art.rcf` (2 219 of 2 601
    /// entries) is RZ-wrapped; raw members (DDS textures, GFX movies)
    /// are returned as-is.
    pub fn payload_decompressed(&self, entry: &RcfDirEntry) -> Result<Vec<u8>> {
        let p = self.payload(entry)?;
        if rz::is_rz(p) {
            rz::decompress(p)
        } else {
            Ok(p.to_vec())
        }
    }
}

// ---------------------------------------------------------------------------
// Disk-backed span reads (block 44): header + directory without the
// payload region, and one member's payload by span — the streaming
// loader's I/O shape (a phone cannot hold a 633 MB archive in RAM).
// ---------------------------------------------------------------------------

/// Parse the header + directory of an RCF archive from a seekable
/// stream WITHOUT loading the payload region: 60 header bytes, then
/// `dir_size_in_bytes` directory bytes.  The produced header and
/// entries are BIT-IDENTICAL to [`RcfArchive::parse`] over the whole
/// file (tested against the committed real-archive fixtures).
pub fn parse_directory<F: std::io::Read + std::io::Seek>(
    file: &mut F,
) -> Result<(RcfHeader, Vec<RcfDirEntry>)> {
    use std::io::SeekFrom;
    // 1. the 60-byte head (everything RcfHeader::parse consumes).
    let mut head = [0u8; 60];
    file.read_exact(&mut head)
        .map_err(|e| P3dError::Other(format!("rcf head read failed: {e}")))?;
    let (header, _) = RcfHeader::parse(&head)?;
    // 2. the directory table at dir_offset.
    file.seek(SeekFrom::Start(header.dir_offset as u64))
        .map_err(|e| P3dError::Other(format!("rcf dir seek failed: {e}")))?;
    let mut dir = vec![0u8; header.dir_size_in_bytes as usize];
    file.read_exact(&mut dir)
        .map_err(|e| P3dError::Other(format!("rcf dir read failed: {e}")))?;
    let mut r = Reader::new(&dir);
    let mut entries = Vec::with_capacity(header.entry_count.min(65536) as usize);
    for _ in 0..header.entry_count {
        entries.push(RcfDirEntry::parse(&mut r)?);
    }
    Ok((header, entries))
}

/// Read ONE member's payload by span from a seekable stream: seek to
/// the entry's `file_offset`, read exactly `file_size` bytes
/// (bounds-checked against the stream's real length — fail-closed),
/// then RZ-decompress when wrapped.  The returned bytes are
/// BIT-IDENTICAL to [`RcfArchive::payload_decompressed`] over the
/// whole-file parse (tested against the committed real-archive
/// fixtures).
pub fn read_member_payload<F: std::io::Read + std::io::Seek>(
    file: &mut F,
    entry: &RcfDirEntry,
) -> Result<Vec<u8>> {
    use std::io::SeekFrom;
    // Bounds-check the span against the stream's actual length —
    // the directory is trusted for ORDER, not for spans.
    let len = file
        .seek(SeekFrom::End(0))
        .map_err(|e| P3dError::Other(format!("rcf len seek failed: {e}")))?;
    let start = entry.file_offset as u64;
    let end = start + entry.file_size as u64;
    if end > len {
        return Err(P3dError::BadLength {
            offset: entry.file_offset as usize,
            value: entry.file_size,
            remaining: len.saturating_sub(start) as usize,
        });
    }
    file.seek(SeekFrom::Start(start))
        .map_err(|e| P3dError::Other(format!("rcf payload seek failed: {e}")))?;
    let mut raw = vec![0u8; entry.file_size as usize];
    file.read_exact(&mut raw)
        .map_err(|e| P3dError::Other(format!("rcf payload read failed: {e}")))?;
    if rz::is_rz(&raw) {
        rz::decompress(&raw)
    } else {
        Ok(raw)
    }
}

// ---------------------------------------------------------------------------
// Streaming member window (block 54): a Read + Seek over ONE member's
// span — the Bink player's I/O shape.  A 311 MB movie member is never
// in RAM: the player seeks and reads frame spans straight out of the
// archive file, exactly like the streaming mesh loader reads its spans.
// ---------------------------------------------------------------------------

/// A `Read + Seek` window over one member's byte span inside a larger
/// seekable stream (block 54, NEXT-UP (a) — the boot-path movie hookup).
///
/// Offsets are MOVIE-RELATIVE: position 0 is the span's first byte
/// (`start` in the underlying stream), so a [`BinkPlayer`] opened over
/// this window behaves exactly as if it held the extracted member file
/// — `BinkPlayer::open` reads its head at window offset 0 and every
/// frame is one `seek(Start(frame.start))` + one exact span read, the
/// same one-seek-per-frame shape the player's own docs promise.
///
/// * Fail-closed construction: `start + len` must fit the underlying
///   stream's real length (the directory is trusted for ORDER, not for
///   spans — the same policy `read_member_payload` applies).
/// * Reads past the window end return 0 bytes (`read_exact` therefore
///   errors past the end — a truncated member cannot decode silently).
/// * Seeks clamp into `[0, len]` (std permits seeking past EOF; the
///   window keeps the position in-span so `stream_position` stays
///   meaningful).
pub struct MemberSpanReader<F> {
    inner: F,
    /// Absolute offset of the member's first byte in the stream.
    start: u64,
    /// Exact member length in bytes.
    len: u64,
    /// Movie-relative cursor (`0..=len`).
    pos: u64,
}

impl<F: std::io::Read + std::io::Seek> MemberSpanReader<F> {
    /// Window the member `start..start+len` of `inner`.  Fail-closed:
    /// errors when the span extends past the stream's real end.
    pub fn new(mut inner: F, start: u64, len: u64) -> Result<Self> {
        use std::io::SeekFrom;
        let stream_len = inner
            .seek(SeekFrom::End(0))
            .map_err(|e| P3dError::Other(format!("rcf span: length seek failed: {e}")))?;
        if start.checked_add(len).map_or(true, |end| end > stream_len) {
            return Err(P3dError::BadLength {
                offset: start as usize,
                value: len as u32,
                remaining: stream_len.saturating_sub(start) as usize,
            });
        }
        Ok(Self { inner, start, len, pos: 0 })
    }

    /// Exact member length in bytes.
    pub fn member_len(&self) -> u64 {
        self.len
    }

    /// Absolute offset of the member's first byte in the stream.
    pub fn span_start(&self) -> u64 {
        self.start
    }

    /// Movie-relative cursor (`0..=len`).
    pub fn window_position(&self) -> u64 {
        self.pos
    }
}

impl<F: std::io::Read + std::io::Seek> std::io::Read for MemberSpanReader<F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::io::SeekFrom;
        let avail = (self.len - self.pos) as usize;
        let n = buf.len().min(avail);
        if n == 0 {
            return Ok(0); // at the window end — std read semantics
        }
        self.inner
            .seek(SeekFrom::Start(self.start + self.pos))?;
        let read = self.inner.read(&mut buf[..n])?;
        self.pos += read as u64;
        Ok(read)
    }
}

impl<F: std::io::Read + std::io::Seek> std::io::Seek for MemberSpanReader<F> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        let target: i64 = match pos {
            std::io::SeekFrom::Start(off) => off as i64,
            std::io::SeekFrom::End(off) => self.len as i64 + off,
            std::io::SeekFrom::Current(delta) => self.pos as i64 + delta,
        };
        // Clamp into [0, len] — a seek past either edge parks at the
        // edge (reads there yield 0 / error, never touch other members).
        self.pos = target.clamp(0, self.len as i64) as u64;
        Ok(self.pos)
    }
}

// ---------------------------------------------------------------------------
// Payload sniffing
// ---------------------------------------------------------------------------

/// The kind of payload an RCF entry contains, inferred from its magic
/// bytes.  Used by tooling that wants to dispatch to the right
/// downstream parser (P3D, Lua, etc.) without a name table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadKind {
    /// Pure3D (`.p3d`) chunk file — magic `P3D\xff`.
    P3D,
    /// Compiled Lua 5.1 bytecode — magic `\x1bLua\x51`.
    Lua51,
    /// Plain ASCII text (e.g. credits file).  Heuristic: first 4 bytes
    /// are all printable ASCII.
    Text,
    /// RZ-compressed payload — magic `RZ\0\0`: a 16-byte Radical header
    /// wrapping one zlib stream (see [`rz`]).  All art-archive P3D
    /// members use this wrapper.
    Rz,
    /// Unknown / binary.
    Unknown,
}

impl PayloadKind {
    pub fn sniff(bytes: &[u8]) -> Self {
        if bytes.starts_with(&[0x50, 0x33, 0x44, 0xff]) {
            return Self::P3D;
        }
        if bytes.starts_with(&[0x52, 0x5A, 0x00, 0x00]) {
            return Self::Rz;
        }
        if bytes.starts_with(&[0x1b, b'L', b'u', b'a', 0x51]) {
            return Self::Lua51;
        }
        if bytes.len() >= 4 && bytes[..4].iter().all(|&b| (32..127).contains(&b)) {
            return Self::Text;
        }
        Self::Unknown
    }
}

// ---------------------------------------------------------------------------
// Unit tests against real fixture files
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
    fn parses_shaders_rcf() {
        let data = fixture("shaders.rcf");
        let arc = RcfArchive::parse(&data).expect("shaders.rcf must parse");
        assert_eq!(arc.header.magic, "ATG CORE CEMENT LIBRARY");
        assert_eq!(arc.header.version_field, 0x0100_0102);
        assert_eq!(arc.header.dir_offset, 60);
        assert_eq!(arc.header.dir_size_in_bytes, 12);
        assert_eq!(arc.header.sector_size, 2048);
        assert_eq!(arc.header.entry_count, 1);
        assert_eq!(arc.entries.len(), 1);
        let e = &arc.entries[0];
        assert_eq!(e.name_hash, 0xd1d2c18d);
        assert_eq!(e.file_offset, 4096);
        assert_eq!(e.file_size, 6697);
        // Payload must be a P3D file (verified directly above).
        let payload = arc.payload(e).unwrap();
        assert_eq!(&payload[..4], &[0x50, 0x33, 0x44, 0xff]);
        assert_eq!(arc.payload_kind(e).unwrap(), PayloadKind::P3D);
        // End of payload must equal end of file.
        assert_eq!(e.file_offset as usize + e.file_size as usize, data.len());
    }

    #[test]
    fn parses_scripts_rcf() {
        let data = fixture("scripts.rcf");
        let arc = RcfArchive::parse(&data).expect("scripts.rcf must parse");
        assert_eq!(arc.header.magic, "ATG CORE CEMENT LIBRARY");
        assert_eq!(arc.header.dir_offset, 60);
        assert_eq!(arc.header.dir_size_in_bytes, 84);
        assert_eq!(arc.header.sector_size, 2048);
        assert_eq!(arc.header.entry_count, 7);
        assert_eq!(arc.entries.len(), 7);
        // Each entry's end offset must fit within the file.
        for e in &arc.entries {
            let end = e.file_offset as usize + e.file_size as usize;
            assert!(end <= data.len(), "entry overflows: {e:?}");
        }
        // The credits text file starts with "EXECUTIVE PRODUCER\r\n".
        let mut found_credits = false;
        let mut found_lua = false;
        let mut found_p3d = false;
        for e in &arc.entries {
            let p = arc.payload(e).unwrap();
            match PayloadKind::sniff(p) {
                PayloadKind::Text => {
                    if p.starts_with(b"EXECUTIVE PRODUCER\r\n") {
                        found_credits = true;
                        // Confirm Eric Holmes (Lead Game Designer) is in the credits.
                        assert!(p.windows(11).any(|w| w == b"Eric Holmes"));
                    }
                }
                PayloadKind::Lua51 => {
                    found_lua = true;
                    // Lua 5.1 bytecode header fields
                    assert_eq!(p[5], 0x00); // format
                    assert_eq!(p[6], 0x01); // little-endian
                }
                PayloadKind::P3D => {
                    found_p3d = true;
                }
                _ => {}
            }
        }
        assert!(found_credits, "scripts.rcf must contain the credits text");
        assert!(found_lua, "scripts.rcf must contain a Lua bytecode file");
        assert!(found_p3d, "scripts.rcf must contain an embedded P3D file");
    }

    #[test]
    fn lookup_by_hash_round_trip() {
        let data = fixture("shaders.rcf");
        let arc = RcfArchive::parse(&data).unwrap();
        let hash = arc.entries[0].name_hash;
        assert!(arc.lookup_by_hash(hash).is_some());
        assert!(arc.lookup_by_hash(0xffff_ffff).is_none());
    }

    #[test]
    fn rejects_non_rcf_data() {
        let data = b"NOT AN RCF FILE..............................";
        assert!(RcfArchive::parse(data).is_err());
    }

    // -----------------------------------------------------------------------
    // PROVEN name-hash tests (recovered algorithm vs real archives)
    // -----------------------------------------------------------------------

    #[test]
    fn hash_name_matches_x360_research_pair() {
        // Known pair from the Nazatric/PrototypeRecomp X360 analysis —
        // the same cement library ships on both platforms.
        assert_eq!(hash_name(r"art\hud\fonts_latin.gfx"), 0x7863_0CB6);
    }

    #[test]
    fn hash_name_edges() {
        assert_eq!(hash_name(""), 0);
        // Sign-extension: byte 0x80 -> i8 -128, < 0x61 so +0x20 -> -96,
        // added to the (zero) accumulator as a wrapping u32.
        assert_eq!(hash_bytes(&[0x80]), (-96i32) as u32);
        // Case-insensitivity by construction ('A'+0x20 == 'a').
        assert_eq!(hash_name("SCRIPTS\\STATSACHIEVEMENTS_TOD.P3D"),
                   hash_name("scripts\\statsachievements_tod.p3d"));
    }

    #[test]
    fn entries_are_sorted_by_hash() {
        // The engine binary-searches the directory (0x106502C0), so the
        // build pipeline must emit sorted entries — verified on both
        // real archives.
        for name in ["scripts.rcf", "shaders.rcf"] {
            let data = fixture(name);
            let arc = RcfArchive::parse(&data).unwrap();
            assert!(arc.entries.windows(2).all(|w| w[0].name_hash <= w[1].name_hash),
                    "{name} entries must be sorted by hash");
        }
    }

    #[test]
    fn lookup_resolves_every_cracked_name() {
        let data = fixture("scripts.rcf");
        let arc = RcfArchive::parse(&data).unwrap();
        let expected: [(u32, &str); 6] = [
            (0x0FFD_75FA, ""), // entry 0: name still unknown
            (0x316D_751D, "scripts\\credits_spanish.txt"),
            (0x34E4_0DC3, "scripts\\frevents.lua"),
            (0x437F_3629, "scripts\\credits_italian.txt"),
            (0xA99C_4C55, "scripts\\credits_french.txt"),
            (0xCE31_0C99, "scripts\\credits_english.txt"),
        ];
        for (hash, name) in expected {
            assert!(arc.lookup_by_hash(hash).is_some(), "hash {hash:#x} must exist");
            if !name.is_empty() {
                let e = arc.lookup(name)
                    .unwrap_or_else(|| panic!("{name:?} must resolve"));
                assert_eq!(e.name_hash, hash);
            }
        }
        // Leading backslashes are stripped like the engine does.
        assert!(arc.lookup(r"\scripts\frevents.lua").is_some());
        assert!(arc.lookup(r"\\scripts\frevents.lua").is_some());
        assert!(arc.lookup("scripts\\nope.lua").is_none());
    }

    #[test]
    fn payload_by_name_returns_byte_identical_standalone_files() {
        // The strongest possible verification: the payloads fetched by
        // NAME are byte-identical to the standalone game files.
        let rcf_data = fixture("scripts.rcf");
        let arc = RcfArchive::parse(&rcf_data).unwrap();
        let fre = fixture("frevents.lua");
        assert_eq!(arc.payload_by_name("scripts\\frevents.lua").unwrap(), fre.as_slice());
        let stats = fixture("statsachievements_tod.p3d");
        assert_eq!(
            arc.payload_by_name("scripts\\statsachievements_tod.p3d").unwrap(),
            stats.as_slice()
        );
        // The English credits payload opens with the recovered credits text.
        let credits = arc.payload_by_name("scripts\\credits_english.txt").unwrap();
        assert!(credits.starts_with(b"EXECUTIVE PRODUCER\r\n"));
    }

    /// Negative result that still holds: the entry hashes are NOT
    /// plain CRC32s of common filenames.  The algorithm is now PROVEN
    /// (`hash_name`); this test pins the historical negative finding.
    #[test]
    fn hash_is_not_plain_crc32() {
        let data = fixture("scripts.rcf");
        let arc = RcfArchive::parse(&data).unwrap();
        let candidates = [
            "scripts.rcf",
            "shaders.rcf",
            "audio.rcf",
            "00audio.rcf",
            "frevents.lua",
            "main.lua",
            "credits.txt",
            "credits.lua",
            "init.lua",
            "statsachievements_tod.p3d",
            "boot_strap.p3d",
            "frontendshaders.p3d",
            "skuinfo.p3d",
            "cementfiles.p3d",
        ];
        for c in candidates {
            let h = crc32fast::hash(c.as_bytes());
            assert!(
                !arc.entries.iter().any(|e| e.name_hash == h),
                "CRC32({c:?}) unexpectedly matched an RCF entry hash"
            );
        }
    }

    /// (block 44) Disk-backed span reads are BIT-IDENTICAL to the
    /// whole-file parse, on the two committed REAL cells.rcf extracts:
    /// header + directory fields, every directory entry, and every
    /// member's decompressed payload.
    #[test]
    fn disk_span_reads_match_the_whole_file_parse() {
        for name in ["cells_mini.rcf", "cells_pairing.rcf"] {
            let data = fixture(name);
            let arc = RcfArchive::parse(&data)
                .unwrap_or_else(|e| panic!("{name} must parse: {e}"));
            let mut cur = std::io::Cursor::new(&data[..]);
            let (header, entries) = parse_directory(&mut cur)
                .unwrap_or_else(|e| panic!("{name}: parse_directory failed: {e}"));
            assert_eq!(header, arc.header, "{name}: header bit-identity");
            assert_eq!(entries, arc.entries, "{name}: directory bit-identity");
            for (i, e) in arc.entries.iter().enumerate() {
                let want = arc
                    .payload_decompressed(e)
                    .unwrap_or_else(|e| panic!("{name} entry {i}: whole-file decode: {e}"));
                let got = read_member_payload(&mut cur, e)
                    .unwrap_or_else(|e| panic!("{name} entry {i}: span read: {e}"));
                assert_eq!(got, want, "{name} entry {i}: payload bit-identity");
            }
        }
    }

    /// (block 44) Span reads fail CLOSED on a truncated file: the
    /// directory may still parse (it lives at the head), but every
    /// payload span that extends past the truncated length errors —
    /// the directory is trusted for order, not for spans.
    #[test]
    fn disk_span_reads_fail_closed_on_truncation() {
        let data = fixture("cells_mini.rcf");
        assert!(data.len() > 64);
        let truncated = &data[..data.len() - 64];
        let mut cur = std::io::Cursor::new(truncated.to_vec());
        let arc = RcfArchive::parse(&data).unwrap();
        // every entry's span ends at/after the truncation point in
        // this fixture (single-member 8 KB archive) — all must error
        let mut errored = 0;
        for e in &arc.entries {
            if e.file_offset as u64 + e.file_size as u64 > truncated.len() as u64 {
                assert!(read_member_payload(&mut cur, e).is_err());
                errored += 1;
            }
        }
        assert!(errored > 0, "fixture spans must cross the truncation");
        // and a bogus offset is rejected before any read
        let bogus = RcfDirEntry { name_hash: 0, file_offset: u32::MAX - 8, file_size: 64 };
        assert!(read_member_payload(&mut cur, &bogus).is_err());
    }

    // -----------------------------------------------------------------------
    // (block 54) MemberSpanReader — the streaming member window
    // -----------------------------------------------------------------------

    use std::io::{Read as _, Seek as _};

    /// Synthetic stream: 10-B prefix | 16-B member (marker bytes) | 6-B
    /// suffix.  Reads/seeks through the window must never see the
    /// prefix or the suffix.
    fn windowed_stream() -> (Vec<u8>, u64, u64) {
        let mut blob = Vec::new();
        blob.extend(b"0123456789"); // prefix
        let start = blob.len() as u64;
        blob.extend((0u8..16).collect::<Vec<u8>>()); // the "member"
        let len = 16u64;
        blob.extend(b"ABCDEF"); // suffix
        (blob, start, len)
    }

    #[test]
    fn member_span_reader_maps_reads_onto_its_window() {
        let (blob, start, len) = windowed_stream();
        let mut r = MemberSpanReader::new(std::io::Cursor::new(blob), start, len).unwrap();
        assert_eq!(r.member_len(), 16);
        assert_eq!(r.span_start(), 10);
        assert_eq!(r.window_position(), 0);

        // Sequential reads walk the member bytes only.
        let mut buf = [0u8; 5];
        r.read_exact(&mut buf).unwrap();
        assert_eq!(buf, [0, 1, 2, 3, 4]);
        assert_eq!(r.window_position(), 5);
        r.read_exact(&mut buf).unwrap();
        assert_eq!(buf, [5, 6, 7, 8, 9]);

        // Read the rest; then the window end yields 0 bytes (std EOF).
        let mut rest = Vec::new();
        std::io::Read::read_to_end(&mut r, &mut rest).unwrap();
        assert_eq!(rest, (10u8..16).collect::<Vec<u8>>());
        assert_eq!(r.window_position(), 16);
        assert_eq!(std::io::Read::by_ref(&mut r).read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn member_span_reader_seeks_start_current_and_end() {
        let (blob, start, len) = windowed_stream();
        let mut r = MemberSpanReader::new(std::io::Cursor::new(blob), start, len).unwrap();
        use std::io::SeekFrom;
        // Start(offset) is movie-relative.
        assert_eq!(r.seek(SeekFrom::Start(6)).unwrap(), 6);
        let mut b = [0u8; 2];
        r.read_exact(&mut b).unwrap();
        assert_eq!(b, [6, 7]);
        // Current(delta) rides from there.
        assert_eq!(r.seek(SeekFrom::Current(-4)).unwrap(), 4);
        r.read_exact(&mut b).unwrap();
        assert_eq!(b, [4, 5]);
        // End(0) parks at the member end, NOT the stream end.
        assert_eq!(r.seek(SeekFrom::End(0)).unwrap(), 16);
        assert_eq!(r.seek(SeekFrom::End(-16)).unwrap(), 0);
        // Clamping: past either edge parks at the edge.
        assert_eq!(r.seek(SeekFrom::Start(9999)).unwrap(), 16);
        assert_eq!(r.seek(SeekFrom::End(-9999)).unwrap(), 0);
        assert_eq!(r.seek(SeekFrom::Current(-1)).unwrap(), 0);
    }

    #[test]
    fn member_span_reader_read_exact_fails_closed_past_the_window() {
        let (blob, start, len) = windowed_stream();
        let mut r = MemberSpanReader::new(std::io::Cursor::new(blob), start, len).unwrap();
        // An over-long exact read errors — and never leaks the suffix.
        let mut too_many = [0u8; 17];
        let err = r.read_exact(&mut too_many).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
        // The bytes consumed before the EOF are the member's first 16.
        assert_eq!(too_many[..16], (0u8..16).collect::<Vec<u8>>());
        // The window position clamped at the end, not into the suffix.
        assert_eq!(r.window_position(), 16);
    }

    #[test]
    fn member_span_reader_fails_closed_on_out_of_bounds_span() {
        let (blob, _start, _len) = windowed_stream();
        // Span beyond the stream end.
        assert!(MemberSpanReader::new(std::io::Cursor::new(&blob), 20, 16).is_err());
        // Overflowing span (start + len wraps).
        assert!(MemberSpanReader::new(
            std::io::Cursor::new(&blob),
            u64::MAX - 4,
            16
        )
        .is_err());
    }

    /// The REAL-archive proof: windowing `scripts.rcf` to the
    /// `scripts\frevents.lua` entry and reading the whole window yields
    /// the byte-identical standalone file — the extraction-free member
    /// read the boot-path movie player runs on.
    #[test]
    fn member_span_reader_reads_the_real_scripts_rcf_member() {
        let data = fixture("scripts.rcf");
        let arc = RcfArchive::parse(&data).unwrap();
        let e = arc.lookup("scripts\\frevents.lua").unwrap();
        let mut r = MemberSpanReader::new(
            std::io::Cursor::new(&data[..]),
            e.file_offset as u64,
            e.file_size as u64,
        )
        .unwrap();
        let mut got = Vec::new();
        std::io::Read::read_to_end(&mut r, &mut got).unwrap();
        let want = fixture("frevents.lua");
        assert_eq!(got, want, "windowed read must be byte-identical");
        assert_eq!(got.len() as u64, r.member_len());
    }
}

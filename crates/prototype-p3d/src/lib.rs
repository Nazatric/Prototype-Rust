//! prototype-p3d (v2)
//! ==================
//! Pure3D (`.p3d`) container parser — **v2, empirically verified**.
//!
//! ## PROVEN format (exact-cover walk on 5 real Prototype files)
//!
//! A `.p3d` file is:
//!
//! ```text
//! [ 0..11]  file header (12 bytes):
//!             magic    : [u8;4]  = "P3D\xff"  (50 33 44 ff)
//!             version  : u32 LE  = 12
//!             file_size: u32 LE  = exact file length in bytes
//! [12..  ]  SEQUENCE of top-level chunks until EOF
//! ```
//!
//! Every chunk (top-level or nested) shares the same 12-byte prelude:
//!
//! ```text
//! type : u32   — chunk family/type id
//! uid  : u32   — per-instance id (unique within the file in practice)
//! size : u32   — TOTAL chunk size in bytes, counted FROM the `type`
//!                field.  chunk spans [start, start+size).
//! ```
//!
//! Validation rule (PROVEN): the top-level chunks exactly cover the
//! file — `12 + Σ size == file_size` in all 5 samples.
//!
//! ## Payload styles by `type` (PROVEN from samples)
//!
//! | type | family | payload after the 12-byte prelude |
//! |------|--------|-----------------------------------|
//! | `0xFE000000` | **classic** | `id:u32`, `namelen:u32`, `name:[u8;namelen]`, then type-specific data.  For `AudioFile`/`Cue` chunks the data begins `u8(0)`, `u32 count`, then `count` × (`u32`-length-prefixed string + NUL). |
//! | `0x07F00000` | **content** | a run of `u8`-length-prefixed Pascal strings (`"Class(1):Display"`, `"Display"`, `"ns::Class"`, `""`), then `u32` fields, then optionally a `"META"` marker + `u32 count` + `count` × `u32`-length-prefixed entries. |
//! | `0x0001100B` | **shader** | `u8 namelen`, `name`, then `u8(0)`, several `u32`s, then NESTED classic-style chunks (`0x0001100C` …). |
//!
//! Observed chunk samples: `boot_strap.p3d` = 280 `AudioFile` + 1
//! `Cue` classic chunks; `frontendshaders.p3d` = 36 shader chunks;
//! `skuinfo.p3d` = 1 content chunk (META: 4 languages);
//! `statsachievements_tod.p3d` = 2 content chunks (META: 40
//! achievements); `cementfiles.p3d` = 1 content chunk (RCF list).
//!
//! ## Nested-chunk types (from the public Pure3D format family — LIKELY, no mesh sample in this dataset)
//!
//! Real art `.p3d` files (inside the game's `*.rcf` archives) contain
//! classic chunks of these types:
//!   * `0x00010012` VertexBuffer
//!   * `0x00010013` IndexBuffer
//!   * `0x00010020` Mesh
//! The generic classic-chunk walker here supports them; typed decoders
//! live in [`mesh`] and are marked LIKELY until verified against a real
//! art file.

#![forbid(unsafe_code)]

use prototype_common::{P3dError, Reader, Result, P3D_MAGIC, P3D_VERSION};

// ---------------------------------------------------------------------------
// Constants — PROVEN type ids
// ---------------------------------------------------------------------------

/// Classic Pure3D chunk family (`boot_strap.p3d` audio chunks).
pub const CHUNK_TYPE_CLASSIC: u32 = 0xFE00_0000;
/// Prototype "content definition" chunk family (skuinfo, cementfiles, stats).
pub const CHUNK_TYPE_CONTENT: u32 = 0x07F0_0000;
/// Prototype shader chunk family — a RANGE of type ids observed in
/// `frontendshaders.p3d` (24× `0x0001100B` + 12× `0x00011009`).
/// All shader chunks match `(type & 0xFFFFFF00) == 0x00011000`.
pub const CHUNK_TYPE_SHADER_BASE: u32 = 0x0001_1000;
/// `0x0001100B` — a **ShaderProgram** chunk: compiled D3D9 bytecode +
/// inline HLSL source (PROVEN — see [`Chunk::shader_program`]).
pub const CHUNK_TYPE_SHADER_PROGRAM: u32 = 0x0001_100B;
/// `0x0001100C` — the nested code-child type inside a ShaderProgram
/// (PROVEN — every program chunk's section 1 opens with this type).
pub const CHUNK_TYPE_SHADER_PROGRAM_CODE: u32 = 0x0001_100C;
/// `0x00011009` — a named **Shader definition** chunk (12 in
/// `frontendshaders.p3d`: `CxformGauraud`, `gfxtextshader`,
/// `hud_2dmarker`, ...).  Payload beyond the name: opaque fields.
pub const CHUNK_TYPE_SHADER_DEFINITION: u32 = 0x0001_1009;

/// File header is exactly 12 bytes: magic + version + file_size.
pub const P3D_FILE_HEADER_SIZE: usize = 12;
/// Chunk prelude is exactly 12 bytes: type + uid + size.
pub const P3D_CHUNK_PRELUDE_SIZE: usize = 12;

// ---------------------------------------------------------------------------
// File header
// ---------------------------------------------------------------------------

/// 12-byte P3D file header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileHeader {
    /// Always `P3D\xff`.
    pub magic: [u8; 4],
    /// Always 12 in every observed Prototype file.
    pub version: u32,
    /// Exact file length in bytes.
    pub file_size: u32,
}

impl FileHeader {
    pub fn parse(data: &[u8]) -> Result<(Self, Reader<'_>)> {
        let mut r = Reader::new(data);
        let m = r.read_bytes(4)?;
        if m != P3D_MAGIC {
            return Err(P3dError::Other(format!(
                "invalid P3D magic: expected {P3D_MAGIC:?}, got {m:?}"
            )));
        }
        let mut magic = [0u8; 4];
        magic.copy_from_slice(m);
        let version = r.read_u32()?;
        if version != P3D_VERSION {
            return Err(P3dError::Other(format!(
                "unsupported P3D version {version} (only 12 supported)"
            )));
        }
        let file_size = r.read_u32()?;
        if file_size as usize != data.len() {
            return Err(P3dError::Other(format!(
                "P3D file_size {file_size} != actual length {}",
                data.len()
            )));
        }
        Ok((Self { magic, version, file_size }, r))
    }
}

// ---------------------------------------------------------------------------
// Chunk family classification
// ---------------------------------------------------------------------------

/// The family a chunk belongs to, derived from its `type` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChunkFamily {
    /// `0xFE000000` — id + u32-length name + type-specific data.
    Classic,
    /// `0x07F00000` — Pascal class names + fields + optional META.
    Content,
    /// `0x000110xx` — u8-length name + nested shader chunks.  PROVEN:
    /// `frontendshaders.p3d` contains 24× `0x0001100B` and 12×
    /// `0x00011009`, all matching the family mask.
    Shader,
    /// Any other type id (kept for forward compatibility).
    Other(u32),
}

impl ChunkFamily {
    pub fn from_type(t: u32) -> Self {
        match t {
            CHUNK_TYPE_CLASSIC => Self::Classic,
            CHUNK_TYPE_CONTENT => Self::Content,
            t if (t & 0xFFFF_FF00) == CHUNK_TYPE_SHADER_BASE => Self::Shader,
            other => Self::Other(other),
        }
    }
}

// ---------------------------------------------------------------------------
// Chunk
// ---------------------------------------------------------------------------

/// A parsed chunk: prelude + a borrowed payload slice.
///
/// The payload is the chunk's bytes AFTER the 12-byte prelude, i.e.
/// `data[start+12 .. start+size]`.
#[derive(Debug, Clone)]
pub struct Chunk<'a> {
    /// Raw type id.
    pub chunk_type: u32,
    /// Per-instance uid.
    pub uid: u32,
    /// Total chunk size in bytes (from the `type` field).
    pub size: u32,
    /// Byte offset of the chunk in the file.
    pub offset: usize,
    /// Family classification.
    pub family: ChunkFamily,
    /// Payload bytes (after the 12-byte prelude).
    pub payload: &'a [u8],
}

impl<'a> Chunk<'a> {
    /// Parse one chunk at `offset`.  Returns the chunk and the offset
    /// just past its end.
    pub fn parse(data: &'a [u8], offset: usize) -> Result<(Self, usize)> {
        let mut r = Reader::from_at(data, offset);
        let chunk_type = r.read_u32()?;
        let uid = r.read_u32()?;
        let size = r.read_u32()?;
        if size < P3D_CHUNK_PRELUDE_SIZE as u32 {
            return Err(P3dError::Other(format!(
                "chunk at {offset:#x} has size {size} < 12"
            )));
        }
        let end = offset
            .checked_add(size as usize)
            .ok_or_else(|| P3dError::Other("chunk size overflows usize".into()))?;
        if end > data.len() {
            return Err(P3dError::Other(format!(
                "chunk at {offset:#x} (size {size}) runs past end of file"
            )));
        }
        let payload = &data[offset + P3D_CHUNK_PRELUDE_SIZE..end];
        Ok((
            Self {
                chunk_type,
                uid,
                size,
                offset,
                family: ChunkFamily::from_type(chunk_type),
                payload,
            },
            end,
        ))
    }

    // ----- payload decoders -------------------------------------------

    /// Decode a **classic** payload: `{id:u32, namelen:u32, name}` plus
    /// the remaining type-specific bytes.
    pub fn classic(&self) -> Result<Option<ClassicChunk<'a>>> {
        if self.family != ChunkFamily::Classic {
            return Ok(None);
        }
        let mut r = Reader::new(self.payload);
        let id = r.read_u32()?;
        let namelen = r.read_u32()? as usize;
        if namelen > r.remaining() {
            return Err(P3dError::BadLength {
                offset: self.offset + 20,
                value: namelen as u32,
                remaining: r.remaining(),
            });
        }
        let name_bytes = r.read_bytes(namelen)?;
        let name = String::from_utf8_lossy(name_bytes).trim_end_matches('\0').to_string();
        let data_start = 8 + namelen;
        Ok(Some(ClassicChunk {
            id,
            name,
            data: &self.payload[data_start..],
        }))
    }

    /// Decode a **content** payload: Pascal class-name strings followed
    /// by `u32` fields and an optional META section.
    pub fn content(&self) -> Result<Option<ContentChunk<'_>>> {
        if self.family != ChunkFamily::Content {
            return Ok(None);
        }
        let mut r = Reader::new(self.payload);
        let mut names = Vec::new();
        // Read the run of u8-prefixed Pascal strings.  The run ends
        // at the first zero length byte or a non-printable string.
        while !r.at_end() {
            let saved = r.pos;
            let Ok(len) = r.read_u8() else { break };
            if len == 0 || len as usize > r.remaining() {
                r.pos = saved;
                break;
            }
            let Ok(bytes) = r.read_bytes(len as usize) else {
                r.pos = saved;
                break;
            };
            let printable = bytes
                .iter()
                .filter(|&&b| (32..127).contains(&b) || b == 0)
                .count();
            if printable * 10 < bytes.len() * 8 {
                r.pos = saved;
                break;
            }
            names.push(String::from_utf8_lossy(bytes).trim_end_matches('\0').to_string());
            // A fully empty decoded name usually ends the run.
            if names.last().map(String::is_empty).unwrap_or(false) {
                break;
            }
        }
        // Everything after the string run that is not a META section is
        // treated as opaque fields.
        let rest_start = r.pos;
        let meta_off = find_subslice(self.payload, b"META");
        let (fields, meta) = match meta_off {
            Some(m) if m >= rest_start => {
                let _fields = &self.payload[rest_start..m];
                let mut mr = Reader::from_at(self.payload, m + 4);
                let count = mr.read_u32()?;
                let raw = &self.payload[m + 8..];
                (
                    &self.payload[rest_start..m],
                    Some(MetaSection { count, raw }),
                )
            }
            _ => (&self.payload[rest_start..], None),
        };
        Ok(Some(ContentChunk {
            names,
            fields,
            meta,
        }))
    }

    /// Decode a **shader** payload's name (the `u8`-length prefix string).
    pub fn shader_name(&self) -> Result<Option<String>> {
        if self.family != ChunkFamily::Shader {
            return Ok(None);
        }
        let mut r = Reader::new(self.payload);
        let len = r.read_u8()? as usize;
        if len > r.remaining() {
            return Err(P3dError::BadLengthU8 {
                offset: self.offset + 12,
                value: len as u8,
                remaining: r.remaining(),
            });
        }
        let bytes = r.read_bytes(len)?;
        Ok(Some(
            String::from_utf8_lossy(bytes).trim_end_matches('\0').to_string(),
        ))
    }

    /// Decode a `0x0001100B` **ShaderProgram** chunk — a compiled D3D9
    /// shader plus its HLSL source text.
    ///
    /// ## PROVEN layout (24 of 24 program chunks in `frontendshaders.p3d`,
    /// plus the two programs inside `shaders.rcf`'s payload — exact
    /// consumption in every sample)
    ///
    /// ```text
    /// u8  namelen
    /// [namelen] name (NUL-padded, trimmed)
    /// u32 version?    — 0x100 in every sample
    /// u32 is_fragment — 1 for "...Fragment Program", 0 for "...Vertex Program"
    /// u32 section_count — always 2
    /// child 1 (compiled bytecode) — same 12-byte prelude convention
    /// as top-level chunks (type + uid + size, size counts from type):
    ///   u32 child_type = 0x0001100C
    ///   u32 child_uid
    ///   u32 child_size          (includes the 12-byte prelude)
    ///   [child_size-12] blob:
    ///     u32 = 5               (observed constant)
    ///     u32 = 0x100           (observed constant)
    ///     u32 = 2               (observed constant)
    ///     D3D9 token stream — first token 0xFFFF0300 (ps_3_0) for
    ///     fragment, 0xFFFE0300 (vs_3_0) for vertex; contains a
    ///     'CTAB' constant-table comment.
    /// child 2 (source text) — also a 0x0001100C record:
    ///   u32 child_type = 0x0001100C
    ///   u32 child_uid
    ///   u32 child_size
    ///   [child_size-12] blob:
    ///     u32 = 1               (observed constant)
    ///     u32 text_len          — HLSL source length in bytes
    ///     u32 = 0               (observed constant)
    ///     [text_len] source text — begins "//---" banner, ends "}\n\0"
    /// ```
    pub fn shader_program(&self) -> Result<Option<ShaderProgram<'a>>> {
        if self.chunk_type != CHUNK_TYPE_SHADER_PROGRAM {
            return Ok(None);
        }
        let mut r = Reader::new(self.payload);
        let len = r.read_u8()? as usize;
        if len > r.remaining() {
            return Err(P3dError::BadLengthU8 {
                offset: self.offset + 12,
                value: len as u8,
                remaining: r.remaining(),
            });
        }
        let name = String::from_utf8_lossy(r.read_bytes(len)?)
            .trim_end_matches('\0')
            .to_string();
        let _version = r.read_u32()?;
        let is_fragment = r.read_u32()? == 1;
        let _section_count = r.read_u32()?;

        // Child 1: compiled D3D9 bytecode.
        let (blob1, _) = Self::read_shader_child(&mut r, &name)?;
        if blob1.len() < 16 {
            return Err(P3dError::Other("shader code blob too small".into()));
        }
        let d3d9_tokens = &blob1[12..];
        let version_token = u32::from_le_bytes([
            d3d9_tokens[0], d3d9_tokens[1], d3d9_tokens[2], d3d9_tokens[3],
        ]);

        // Child 2: inline HLSL source text.
        let (blob2, _) = Self::read_shader_child(&mut r, &name)?;
        if blob2.len() < 12 {
            return Err(P3dError::Other("shader source blob too small".into()));
        }
        let text_len = u32::from_le_bytes([blob2[4], blob2[5], blob2[6], blob2[7]]) as usize;
        if text_len + 12 != blob2.len() {
            return Err(P3dError::Other(format!(
                "shader program {name:?}: source text_len {text_len} does not match blob {}",
                blob2.len()
            )));
        }
        let source = &blob2[12..];

        Ok(Some(ShaderProgram {
            name,
            is_fragment,
            d3d9_tokens,
            version_token,
            source,
        }))
    }

    /// Read one nested `0x0001100C` child record: 12-byte prelude
    /// (type + uid + size, size counting from the type field — the
    /// same convention as top-level chunks) followed by its payload.
    /// Returns `(payload, bytes_consumed_including_prelude)`.
    fn read_shader_child<'b>(
        r: &mut Reader<'b>,
        name: &str,
    ) -> Result<(&'b [u8], usize)> {
        let child_type = r.read_u32()?;
        let _child_uid = r.read_u32()?;
        let child_size = r.read_u32()? as usize;
        if child_type != CHUNK_TYPE_SHADER_PROGRAM_CODE {
            return Err(P3dError::Other(format!(
                "shader program {name:?}: expected code child 0x{:08x}, got 0x{child_type:08x}",
                CHUNK_TYPE_SHADER_PROGRAM_CODE
            )));
        }
        if child_size < P3D_CHUNK_PRELUDE_SIZE + 12 {
            return Err(P3dError::Other(format!(
                "shader program {name:?}: implausible code child size {child_size}"
            )));
        }
        let blob = r.read_bytes(child_size - P3D_CHUNK_PRELUDE_SIZE)?;
        Ok((blob, child_size))
    }
}

/// A decoded `0x0001100B` ShaderProgram chunk (see
/// [`Chunk::shader_program`] for the byte layout).
#[derive(Debug, Clone)]
pub struct ShaderProgram<'a> {
    /// e.g. `"CxformGauraud - lit - Fragment Program"`.
    pub name: String,
    /// `true` for fragment (pixel) programs, `false` for vertex.
    pub is_fragment: bool,
    /// The D3D9 shader token stream (SM 3.0) after the code child's
    /// 12-byte header.
    pub d3d9_tokens: &'a [u8],
    /// First token of [`ShaderProgram::d3d9_tokens`] — `0xFFFF0300`
    /// (ps_3_0) or `0xFFFE0300` (vs_3_0) in every observed sample.
    pub version_token: u32,
    /// The inline HLSL source text.
    pub source: &'a [u8],
}

/// Find a byte substring in a haystack (used for the META marker).
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

// ---------------------------------------------------------------------------
// Classic chunk
// ---------------------------------------------------------------------------

/// Decoded classic-family payload.
#[derive(Debug, Clone)]
pub struct ClassicChunk<'a> {
    /// The chunk's instance id (10 for AudioFile/Cue samples).
    pub id: u32,
    /// The chunk's type name (e.g. `"AudioFile"`, `"Cue"`).
    pub name: String,
    /// Remaining type-specific bytes after the name.
    pub data: &'a [u8],
}

impl<'a> ClassicChunk<'a> {
    /// Decode the `{u8(0), u32 count, [u32-pascal + NUL] × count}`
    /// name-list found in `AudioFile` and `Cue` chunk data.
    /// PROVEN on all 281 boot_strap chunks.
    pub fn name_list(&self) -> Result<Vec<String>> {
        let mut r = Reader::new(self.data);
        let _zero = r.read_u8()?;
        let count = r.read_u32()? as usize;
        let mut out = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            let mut s = r.read_pascal_string_u32()?;
            // Optional NUL terminator after the length-prefixed string.
            if r.peek_u8().map(|b| b == 0).unwrap_or(false) {
                let _ = r.read_u8();
            }
            if s.is_empty() {
                s = String::new();
            }
            let _ = &mut s;
            out.push(s);
        }
        Ok(out)
    }

    /// Decode an `AudioFile` chunk's **RADP** payload — Radical's
    /// audio data block (PROVEN 70 of 70 AudioFile chunks in
    /// `boot_strap.p3d`; data_size matches the remaining bytes in
    /// every one).
    ///
    /// Layout after the name list:
    ///
    /// ```text
    /// u32 = 0
    /// u32 = 4
    /// b"radp"                     (u32-length-prefixed string)
    /// u8 = 0                      (terminator)
    /// b"RADP"                     (raw 4-byte magic)
    /// u32 version                 — 1 or 2 (59× v1, 11× v2 observed)
    /// u32 sample_rate             — 44100 or 48000 observed
    /// u32 format_tag              — opaque codec id, 0..=31 observed
    /// u32 data_size               — exact remaining byte count
    /// [data_size] audio payload   — compressed (NOT raw PCM)
    /// ```
    pub fn audio_file(&self) -> Result<Option<AudioFile<'a>>> {
        if self.name != "AudioFile" {
            return Ok(None);
        }
        let mut r = Reader::new(self.data);
        // Skip the name list.
        let _zero = r.read_u8()?;
        let count = r.read_u32()? as usize;
        let mut names = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            let s = r.read_pascal_string_u32()?;
            if r.peek_u8().map(|b| b == 0).unwrap_or(false) {
                let _ = r.read_u8();
            }
            names.push(s);
        }
        // RADP block.
        let _zero2 = r.read_u32()?;
        let radp_len = r.read_u32()? as usize;
        let magic1 = r.read_bytes(radp_len.min(r.remaining()))?;
        if magic1 != b"radp" {
            return Err(P3dError::Other(format!(
                "AudioFile {:?}: expected \"radp\" magic, got {magic1:?}",
                self.name
            )));
        }
        let _nul = r.read_u8()?;
        let magic2 = r.read_bytes(4)?;
        if magic2 != b"RADP" {
            return Err(P3dError::Other(format!(
                "AudioFile {:?}: expected \"RADP\" magic, got {magic2:?}",
                self.name
            )));
        }
        let version = r.read_u32()?;
        let sample_rate = r.read_u32()?;
        let format_tag = r.read_u32()?;
        let data_size = r.read_u32()? as usize;
        if data_size > r.remaining() {
            return Err(P3dError::BadLength {
                offset: 0,
                value: data_size as u32,
                remaining: r.remaining(),
            });
        }
        let data = r.read_bytes(data_size)?;
        Ok(Some(AudioFile {
            names,
            radp_version: version,
            sample_rate,
            format_tag,
            data,
        }))
    }
}

/// A decoded AudioFile chunk: clip names plus the RADP audio payload.
#[derive(Debug, Clone)]
pub struct AudioFile<'a> {
    /// Clip names (the observed chunks repeat the same name twice).
    pub names: Vec<String>,
    /// RADP block version (observed: 1).
    ///
    /// Block 38 engine-PROVEN: this is the INTERLEAVED STREAM COUNT —
    /// the engine's parse computes
    /// `blocks_per_stream = data_size / (20 · version)` and
    /// `samples_per_stream = blocks_per_stream · 32`.
    pub radp_version: u32,
    /// Sample rate (observed: 44100 / 48000).
    pub sample_rate: u32,
    /// Opaque codec/format id (observed range 0..=31).  The payloads
    /// are compressed audio — NOT raw PCM — so decoding requires the
    /// matching Radical/radSound codec.
    ///
    /// Block 38 engine walk: the tag is a PASS-THROUGH to a decoder
    /// factory (`RadicalAdpcmFileDecoder::open` 0x10aaebc0 pushes it
    /// unchanged as the 6th arg of a `vtable+0x14` virtual; no
    /// dispatch in the parse) and is NOT the codec — every payload
    /// decodes as IMA ADPCM regardless of tag.  See
    /// prototype-audio's crate docs for the full chain.
    pub format_tag: u32,
    /// Exact-length audio payload.
    pub data: &'a [u8],
}

// ---------------------------------------------------------------------------
// Content chunk
// ---------------------------------------------------------------------------

/// Decoded content-family payload.
#[derive(Debug, Clone)]
pub struct ContentChunk<'a> {
    /// The Pascal class-name run, e.g.
    /// `["proto::StatsManager(1):Stats Manager", "Stats Manager",
    ///   "proto::StatsManager"]`.
    pub names: Vec<String>,
    /// Opaque `u32` fields between the name run and the META section.
    pub fields: &'a [u8],
    /// The `"META"` attribute section, when present.
    pub meta: Option<MetaSection<'a>>,
}

/// A META section: the `"META"` marker, a `u32 count`, and a raw
/// payload whose internal shape differs per content class.
///
/// Two decoders are provided, each PROVEN on a different sample:
///
/// * [`MetaSection::as_string_list`] — a plain run of `u32`-length-
///   prefixed strings.  PROVEN on `skuinfo.p3d`: count=4 but exactly 6
///   strings follow (`english`, `french`, `italian`, `spanish`,
///   `vug_prototype_pc_na`, `0.0`), consuming the chunk exactly.
///
/// * [`MetaSection::as_string_value_pairs`] — `count` × {string, u32}.
///   PROVEN on `statsachievements_tod.p3d`: count=40, each achievement
///   name followed by a `u32` (0 for "Complete X boss", 1 for
///   "Consumed N Health").
///
/// The `count` field's exact semantics are UNKNOWN (it equals 4 for
/// skuinfo's 6 strings and 40 for achievements' 40 pairs).
#[derive(Debug, Clone)]
pub struct MetaSection<'a> {
    /// The `u32` right after the `"META"` marker.
    pub count: u32,
    /// Raw META payload after the count field.
    pub raw: &'a [u8],
}

impl<'a> MetaSection<'a> {
    /// Decode the payload as a plain list of `u32`-length-prefixed
    /// strings.  PROVEN on skuinfo.p3d (6 strings, exact consumption).
    pub fn as_string_list(&self) -> Result<Vec<String>> {
        let mut r = Reader::new(self.raw);
        let mut out = Vec::new();
        while !r.at_end() {
            match r.read_pascal_string_u32() {
                Ok(s) => {
                    // Sanity: a "length" that produces mostly-binary
                    // output means we've run past the string list.
                    out.push(s);
                }
                Err(_) => break,
            }
            if out.len() > 65536 {
                break;
            }
        }
        Ok(out)
    }

    /// Decode the payload as `count` × {string, u32 value} pairs.
    /// PROVEN on statsachievements_tod.p3d (40 achievement pairs).
    pub fn as_string_value_pairs(&self) -> Result<Vec<(String, u32)>> {
        let mut r = Reader::new(self.raw);
        let mut out = Vec::with_capacity(self.count.min(65536) as usize);
        for _ in 0..self.count {
            let s = match r.read_pascal_string_u32() {
                Ok(s) => s,
                Err(_) => break,
            };
            let v = match r.read_u32() {
                Ok(v) => v,
                Err(_) => break,
            };
            out.push((s, v));
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Whole-file parser
// ---------------------------------------------------------------------------

/// A parsed `.p3d` file: 12-byte header + all top-level chunks.
#[derive(Debug, Clone)]
pub struct P3dFile<'a> {
    pub header: FileHeader,
    /// All top-level chunks, in file order.
    pub chunks: Vec<Chunk<'a>>,
}

impl<'a> P3dFile<'a> {
    /// Parse a complete `.p3d` file.  Enforces the PROVEN invariant
    /// that top-level chunks exactly cover the file.
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let (header, _) = FileHeader::parse(data)?;
        let mut chunks = Vec::new();
        let mut off = P3D_FILE_HEADER_SIZE;
        while off < data.len() {
            let (chunk, end) = Chunk::parse(data, off)?;
            chunks.push(chunk);
            off = end;
        }
        if off != data.len() {
            return Err(P3dError::Other(format!(
                "chunk walk ended at {off:#x} but file is {:#x}",
                data.len()
            )));
        }
        Ok(Self { header, chunks })
    }

    /// Iterator over classic chunks with a given type name
    /// (e.g. `"AudioFile"`).
    pub fn classic_named<'b>(
        &'b self,
        name: &'b str,
    ) -> impl Iterator<Item = &'b Chunk<'a>> + 'b {
        self.chunks.iter().filter(move |c| {
            c.family == ChunkFamily::Classic
                && c.classic()
                    .ok()
                    .flatten()
                    .map(|cc| cc.name == name)
                    .unwrap_or(false)
        })
    }

    /// Extract every audio clip name from `AudioFile` chunks.
    /// Returns `(chunk_uid, names)` pairs.  PROVEN on boot_strap.p3d
    /// (280 entries, names like `air_brake`, `ui_fe_main_select`).
    pub fn audio_names(&self) -> Result<Vec<(u32, Vec<String>)>> {
        let mut out = Vec::new();
        for c in &self.chunks {
            if c.family != ChunkFamily::Classic {
                continue;
            }
            if let Some(cc) = c.classic()? {
                if cc.name == "AudioFile" {
                    out.push((c.uid, cc.name_list()?));
                }
            }
        }
        Ok(out)
    }

    /// Decode every `AudioFile` chunk fully (names + RADP payload).
    /// PROVEN on boot_strap.p3d: 70 audio files, all with exact
    /// data_size accounting.
    pub fn audio_files(&self) -> Result<Vec<(u32, AudioFile<'a>)>> {
        let mut out = Vec::new();
        for c in &self.chunks {
            if c.family != ChunkFamily::Classic {
                continue;
            }
            if let Some(cc) = c.classic()? {
                if let Some(af) = cc.audio_file()? {
                    out.push((c.uid, af));
                }
            }
        }
        Ok(out)
    }

    /// Decode every `0x0001100B` ShaderProgram chunk in the file.
    /// PROVEN on `frontendshaders.p3d` (24 programs) and on the
    /// single program inside `shaders.rcf`'s payload.
    pub fn shader_programs(&self) -> Result<Vec<ShaderProgram<'a>>> {
        let mut out = Vec::new();
        for c in &self.chunks {
            if let Some(sp) = c.shader_program()? {
                out.push(sp);
            }
        }
        Ok(out)
    }

    /// The names of the `0x00011009` **Shader definition** chunks —
    /// the engine-facing shader identities (`CxformGauraud`,
    /// `gfxtextshader`, `hud_2dmarker`, ...).  Their parameter-block
    /// payload layout beyond the name is UNKNOWN (fields are opaque
    /// u32 runs in the 12 observed samples); the program chunks carry
    /// the actual code.
    pub fn shader_definition_names(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for c in &self.chunks {
            if c.chunk_type == CHUNK_TYPE_SHADER_DEFINITION {
                out.push(
                    c.shader_name()?
                        .unwrap_or_else(|| String::from("<unnamed>")),
                );
            }
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Mesh decoders (LIKELY — public Pure3D family layout, no sample in dataset)
// ---------------------------------------------------------------------------

pub mod bank;
pub mod geometry;

/// Block 77 — REAL in-engine NIS cutscene track decoders: the game's
/// OWN camera takes (`CAM_NisCam_Take` TRAN/LOOK keyframes) from the
/// `art\nis\` track members, decoded from the committed real fixtures.
pub mod nis;

/// cells.rcf world decoders — GameObject placements, node transforms,
/// world index (M25, block 18; PROVEN corpus-wide on the real archive).
pub mod world;

/// Block 105 — the world-index CACHE: the built index (plain data) is
/// written once to the game folder and read back in milliseconds on
/// every later boot; the fingerprint binds it to the exact corpus.
pub mod index_cache;

pub use index_cache::{index_fingerprint, read_index_cache, write_index_cache};

/// Block 87 — REAL `0x0012_1000` animation-track decoder (rebuild of
/// the recycle-lost blocks 83/85, re-derived and validated live:
/// 634/634 tracks, exact 533 PTRN / 67 CAM / 34 EXP census).  Decodes
/// the flat chunk sequence (header + per-bone groups + `0x0012_1112`
/// ROT smallest-three-quat + `0x0012_1119` TRAN 3×f16 channels), and
/// closes the anim→skinning pipeline with `pose_world_matrices`.
pub mod anim;

/// Block 83 — OLD-STYLE SKINNED GEOMETRY + SKELETON: the NIS character
/// packages' real skinned Alex Mercer (`0x0001_0001` shapes, desc
/// `0xB1B1`, bone palettes `0x0001_000D`) and the `0x0002_3000`
/// Skeleton with its 94 `0x0002_3001` joints, plus row-vector
/// linear-blend skinning helpers.
pub mod skin;

/// Typed mesh-related constants from the public Pure3D format family.
pub mod mesh {
    /// VertexBuffer chunk type id.
    pub const VERTEX_BUFFER: u32 = 0x0001_0012;
    /// IndexBuffer chunk type id.
    pub const INDEX_BUFFER: u32 = 0x0001_0013;
    /// Mesh chunk type id.
    pub const MESH: u32 = 0x0001_0020;

    /// A decoded IndexBuffer payload: `{u32 count, u16[count] indices}`.
    ///
    /// LIKELY layout (public Pure3D family); verify against a real art
    /// file before relying on it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct IndexBuffer {
        pub indices: Vec<u16>,
    }

    impl IndexBuffer {
        pub fn parse(payload: &[u8]) -> super::Result<Self> {
            let mut r = prototype_common::Reader::new(payload);
            let count = r.read_u32()? as usize;
            let mut indices = Vec::with_capacity(count.min(1 << 22));
            for _ in 0..count {
                indices.push(r.read_u16()?);
            }
            Ok(Self { indices })
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — against REAL fixture files
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fixture_in(name: &str) -> Vec<u8> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    #[test]
    fn skuinfo_single_content_chunk() {
        let data = fixture_in("skuinfo.p3d");
        let f = P3dFile::parse(&data).unwrap();
        assert_eq!(f.header.version, 12);
        assert_eq!(f.header.file_size as usize, data.len());
        assert_eq!(f.chunks.len(), 1);
        let c = &f.chunks[0];
        assert_eq!(c.chunk_type, CHUNK_TYPE_CONTENT);
        assert_eq!(c.uid, 0x4b);
        let content = c.content().unwrap().expect("content payload");
        assert_eq!(content.names[0], "engine::SkuInfo(1):SkuInfo");
        assert!(content.names.iter().any(|n| n == "engine::SkuInfo"));
        let meta = content.meta.expect("skuinfo has META");
        // PROVEN: 6 strings, exactly consuming the chunk (4 languages +
        // SKU name + version).
        let strings = meta.as_string_list().unwrap();
        assert_eq!(
            strings,
            vec![
                "english",
                "french",
                "italian",
                "spanish",
                "vug_prototype_pc_na",
                "0.0"
            ]
        );
    }

    #[test]
    fn cementfiles_lists_rcf_archives() {
        let data = fixture_in("cementfiles.p3d");
        let f = P3dFile::parse(&data).unwrap();
        assert_eq!(f.chunks.len(), 1);
        let c = &f.chunks[0];
        assert_eq!(c.uid, 0x7b);
        let content = c.content().unwrap().unwrap();
        assert_eq!(
            content.names[0],
            "ContentCementFileDefinition(1):CementFileDefinition"
        );
        for needle in [
            "scripts.rcf".as_bytes(),
            "shaders.rcf".as_bytes(),
            "00woi.rcf".as_bytes(),
            "cached".as_bytes(),
        ] {
            assert!(
                c.payload.windows(needle.len()).any(|w| w == needle),
                "cementfiles must mention {:?}",
                String::from_utf8_lossy(needle)
            );
        }
    }

    #[test]
    fn statsachievements_two_content_chunks() {
        let data = fixture_in("statsachievements_tod.p3d");
        let f = P3dFile::parse(&data).unwrap();
        assert_eq!(f.chunks.len(), 2);
        assert_eq!(f.chunks[0].uid, 0x5f);
        assert_eq!(f.chunks[1].uid, 0x7b);
        let a = f.chunks[1].content().unwrap().unwrap();
        assert_eq!(
            a.names[0],
            "proto::AchievementsManager(1):Achievements Manager"
        );
        let meta = a.meta.expect("achievements META");
        assert_eq!(meta.count, 40);
        // PROVEN: {string, u32} pairs — 0 for "Complete X boss" entries,
        // 1 for "Consumed N Health".
        let pairs = meta.as_string_value_pairs().unwrap();
        assert_eq!(pairs.len(), 40);
        assert_eq!(pairs[0], ("Complete Brawler boss".to_string(), 0));
        assert_eq!(pairs[1], ("Complete Specialist boss".to_string(), 0));
        assert_eq!(pairs[2], ("Complete Supreme Hunter boss".to_string(), 0));
        assert_eq!(pairs[3], ("Complete Mother boss".to_string(), 0));
        assert!(pairs
            .iter()
            .any(|(s, v)| s == "Consumed N Health" && *v == 1));
    }

    #[test]
    fn frontendshaders_36_shader_chunks() {
        let data = fixture_in("frontendshaders.p3d");
        let f = P3dFile::parse(&data).unwrap();
        assert_eq!(f.chunks.len(), 36);
        // PROVEN composition: 24 chunks of type 0x0001100B and 12 of
        // type 0x00011009 — all members of the 0x000110xx shader family.
        let t_b = f.chunks.iter().filter(|c| c.chunk_type == 0x0001_100B).count();
        let t_9 = f.chunks.iter().filter(|c| c.chunk_type == 0x0001_1009).count();
        assert_eq!(t_b, 24);
        assert_eq!(t_9, 12);
        for c in &f.chunks {
            assert_eq!(c.family, ChunkFamily::Shader);
        }
        let n0 = f.chunks[0].shader_name().unwrap().unwrap();
        assert_eq!(n0, "CxformGauraud - lit - Fragment Program");
        let n1 = f.chunks[1].shader_name().unwrap().unwrap();
        assert_eq!(n1, "CxformGauraud - lit - Vertex Program");
        // Every shader chunk must carry a non-empty name.
        for c in &f.chunks {
            let n = c.shader_name().unwrap().unwrap();
            assert!(!n.is_empty());
        }
    }

    #[test]
    fn boot_strap_chunk_composition() {
        let data = fixture_in("boot_strap.p3d");
        let f = P3dFile::parse(&data).unwrap();
        // PROVEN composition (empirical count):
        //   70 AudioFile, 2 Cue, 1 GlobalSettings, 2 MaterialMap,
        //   54 Mix, 152 Patch  =  281 classic chunks.
        assert_eq!(f.chunks.len(), 281);
        let mut name_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for c in &f.chunks {
            assert_eq!(c.family, ChunkFamily::Classic);
            let cc = c.classic().unwrap().expect("classic decode");
            *name_counts.entry(cc.name.clone()).or_insert(0) += 1;
        }
        assert_eq!(name_counts.get("AudioFile"), Some(&70));
        assert_eq!(name_counts.get("Cue"), Some(&2));
        assert_eq!(name_counts.get("GlobalSettings"), Some(&1));
        assert_eq!(name_counts.get("MaterialMap"), Some(&2));
        assert_eq!(name_counts.get("Mix"), Some(&54));
        assert_eq!(name_counts.get("Patch"), Some(&152));

        // Audio name extraction (PROVEN values from the byte dump).
        let audio = f.audio_names().unwrap();
        assert_eq!(audio.len(), 70);
        assert_eq!(audio[0].0, 0x9cb3);
        assert_eq!(audio[0].1, vec!["air_brake", "air_brake"]);
        assert!(audio
            .iter()
            .any(|(_, names)| names.first().map(String::as_str) == Some("pink_noise")));
        assert!(audio
            .iter()
            .any(|(_, names)| names.first().map(String::as_str) == Some("ui_fe_main_select")));
    }

    #[test]
    fn rejects_truncated_chunk() {
        let mut data = vec![0x50u8, 0x33, 0x44, 0xff];
        data.extend_from_slice(&12u32.to_le_bytes());
        data.extend_from_slice(&100u32.to_le_bytes()); // lies about size
        data.extend_from_slice(&[0u8; 10]); // only 10 bytes follow
        assert!(P3dFile::parse(&data).is_err());
    }

    #[test]
    fn index_buffer_synthetic_round_trip() {
        // LIKELY layout: u32 count + u16 indices.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&3u32.to_le_bytes());
        for i in [0u16, 1, 2] {
            bytes.extend_from_slice(&i.to_le_bytes());
        }
        let ib = mesh::IndexBuffer::parse(&bytes).unwrap();
        assert_eq!(ib.indices, vec![0, 1, 2]);
    }

    // -----------------------------------------------------------------------
    // ShaderProgram decoding (PROVEN — 24 real programs)
    // -----------------------------------------------------------------------

    #[test]
    fn frontendshaders_has_24_programs_and_12_definitions() {
        let data = fixture_in("frontendshaders.p3d");
        let f = P3dFile::parse(&data).unwrap();
        assert_eq!(f.chunks.len(), 36);
        let defs = f.shader_definition_names().unwrap();
        assert_eq!(defs.len(), 12);
        for expected in ["CxformGauraud", "gfxtextshader", "hud_2dmarker", "hud_minimapmarker"] {
            assert!(defs.iter().any(|d| d == expected), "definition {expected:?} missing");
        }
        let progs = f.shader_programs().unwrap();
        assert_eq!(progs.len(), 24);
    }

    #[test]
    fn shader_programs_are_sm30_with_inline_hlsl() {
        let data = fixture_in("frontendshaders.p3d");
        let f = P3dFile::parse(&data).unwrap();
        let progs = f.shader_programs().unwrap();
        let mut frag = 0;
        let mut vert = 0;
        for p in &progs {
            // Name and is_fragment agree in every sample.
            assert_eq!(
                p.name.ends_with("Fragment Program"),
                p.is_fragment,
                "{:?}",
                p.name
            );
            // D3D9 SM 3.0 version tokens: ps_3_0 / vs_3_0.
            if p.is_fragment {
                assert_eq!(p.version_token, 0xFFFF_0300, "{:?} must be ps_3_0", p.name);
                frag += 1;
            } else {
                assert_eq!(p.version_token, 0xFFFE_0300, "{:?} must be vs_3_0", p.name);
                vert += 1;
            }
            // Compiled code carries a D3D9 CTAB constant table.
            assert!(
                p.d3d9_tokens.windows(4).any(|w| w == b"CTAB"),
                "{:?} bytecode must contain CTAB",
                p.name
            );
            // Inline HLSL source: "//---" banner, "}\n\0" tail.
            assert!(p.source.starts_with(b"//"), "{:?} source banner", p.name);
            assert!(p.source.ends_with(b"}\n\0"), "{:?} source tail", p.name);
        }
        assert_eq!(frag, 12);
        assert_eq!(vert, 12);
    }

    #[test]
    fn first_shader_program_decodes_exactly() {
        let data = fixture_in("frontendshaders.p3d");
        let f = P3dFile::parse(&data).unwrap();
        let progs = f.shader_programs().unwrap();
        let p0 = &progs[0];
        assert_eq!(p0.name, "CxformGauraud - lit - Fragment Program");
        assert!(p0.is_fragment);
        assert_eq!(p0.version_token, 0xFFFF_0300);
        // Section 2 consumption is exact: source length 2326, and the
        // program chunk's sections tile its payload with no leftover.
        assert_eq!(p0.source.len(), 2326);
    }

    #[test]
    fn shaders_rcf_payload_is_a_shader_bundle() {
        // shaders.rcf entry 0 (hash 0xd1d2c18d) is a P3D holding two
        // 0x1100B programs (bink fragment + vertex) and one 0x11009
        // definition named "bink".
        let rcf = fixture_in("shaders.rcf");
        let arc = prototype_rcf::RcfArchive::parse(&rcf).unwrap();
        let p = arc.payload(&arc.entries[0]).unwrap();
        let f = P3dFile::parse(p).unwrap();
        assert_eq!(f.chunks.len(), 3);
        let progs = f.shader_programs().unwrap();
        assert_eq!(progs.len(), 2);
        assert_eq!(progs[0].name, "bink - lit - Fragment Program");
        assert!(progs[0].is_fragment);
        assert_eq!(progs[0].version_token, 0xFFFF_0300);
        assert_eq!(progs[1].name, "bink - lit - Vertex Program");
        assert!(!progs[1].is_fragment);
        assert_eq!(progs[1].version_token, 0xFFFE_0300);
        let defs = f.shader_definition_names().unwrap();
        assert_eq!(defs, vec!["bink".to_string()]);
    }

    // -----------------------------------------------------------------------
    // AudioFile / RADP decoding (PROVEN — 70 of 70 chunks)
    // -----------------------------------------------------------------------

    #[test]
    fn boot_strap_audio_files_decode_with_radp() {
        let data = fixture_in("boot_strap.p3d");
        let f = P3dFile::parse(&data).unwrap();
        let audio = f.audio_files().unwrap();
        assert_eq!(audio.len(), 70);
        // First clip: air_brake @ 44100 Hz, format tag 13, 40 020 B.
        let (uid0, a0) = &audio[0];
        assert_eq!(*uid0, 0x9cb3);
        assert_eq!(a0.names.first().map(String::as_str), Some("air_brake"));
        assert_eq!(a0.radp_version, 1);
        assert_eq!(a0.sample_rate, 44100);
        assert_eq!(a0.format_tag, 13);
        assert_eq!(a0.data.len(), 40020);
        // All 70: version 1 or 2, exact data_size accounting, plausible rate.
        let mut rates = std::collections::BTreeSet::new();
        let mut versions = std::collections::BTreeSet::new();
        for (_, af) in &audio {
            versions.insert(af.radp_version);
            rates.insert(af.sample_rate);
            assert!(!af.data.is_empty());
        }
        assert_eq!(versions, [1u32, 2].into_iter().collect());
        assert_eq!(rates, [44100, 48000].into_iter().collect());
    }
}

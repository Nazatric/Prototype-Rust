//! Pure3D art-pipeline chunk decoders — **PROVEN against real art data**.
//!
//! # Evidence status (flipped from LIKELY to PROVEN in block 16)
//!
//! The user's own PC art files (`art.rcf`, 604 812 872 B, downloaded from
//! the GitHub releases of Nazatric/PrototypeRecomp) contain 2 219 RZ-wrapped
//! P3D members.  Every claim below was validated **corpus-wide** by
//! `scripts/m21b_census.py` / `m21b_layout.py` / `m21b_uvprobe.py` (all
//! 2 219 members parse with exact top-level cover; all 3 877 Shape chunks
//! and 7 171 Mesh chunks conform):
//!
//! * `0x00010000` **Shape**: pascal name + `u32 version(0)` + `u32(1)` +
//!   child run `[0x00010021 ShapeInfo, 0x00010020 Mesh…, 0x00122000,
//!   0x00010017, 0x00010003 BoundBox, 0x00010004 BoundSphere]`
//!   (3 877/3 877 exact cover; the two extra ids `0x00121305`/`0x00121201`
//!   appear in 2 shapes and 2 meshes carry a legacy chunk family
//!   `0x00010005…` — both decoded as raw passthrough).
//! * `0x00010020` **Mesh**: `u32 version(0)` + pascal name (the
//!   `pure3dShader` name — the mesh→shader binding) + 9×u32 header
//!   `[0, desc, vertexCount, indexCount, 0, 1, 1, 0, 0]` + child run
//!   `[0x00010014 PrimitiveGroup, 0x00010012 VertexBuffer,
//!   0x00010013 IndexBuffer, 0x00010003, 0x00010004]`
//!   (7 107/7 171 exact; 64 add an extra `0x00121204`).
//! * `0x00010012` **VertexBuffer**: `u32 magic(0x00020001)` + `u32 2` +
//!   `u32 dataSize` + `vertexCount × stride` bytes
//!   (magic constant on all 7 171 meshes).
//! * `0x00010013` **IndexBuffer**: `u32 magic(0x00020001)` + `u32 0` +
//!   `u32 dataSize` + `indexCount × u16` LE indices
//!   (max index == vertexCount-1 checked corpus-wide on every mesh).
//! * `0x00010003` **BoundBox**: 6×f32 (min, max) — measured vertex data
//!   min/max == bbox to full f32 precision on the barrier fixture.
//! * `0x00010004` **BoundSphere**: 4×f32 (center, radius).
//! * **cells.rcf world geometry (block 25, M28)**: the cells members'
//!   `0x00010020` sections are the SAME layout as art meshes —
//!   `u32 version(0)` + pascal(32) **shader-hash** name + the 9×u32
//!   header + an inline record run
//!   `[0x00010014 PrimitiveGroup, 0x00010012 VertexBuffer,
//!   0x00010013 IndexBuffer, 0x00010003 BoundBox, 0x00010004 BoundSphere]`
//!   whose headers are `[u32 type][u32 size][u32 size]` — they parse as
//!   ordinary child chunks with uid == size, so [`decode_mesh`] decodes
//!   them unchanged
//!   ([`decode_shape`] needs no cells special case).  Verified 235/235
//!   sections of the real Cell-82 geometry member
//!   (`scripts/m30f_verify.py`): exact cover, VB count == V·stride(desc)
//!   on 14 distinct descs, IB count == 2·I, all indices in range; the
//!   committed fixtures `cells_section_quad.p3d` /
//!   `cells_section_textured.p3d` pin the proven vertex values.
//! * The mesh `desc` word is a **vertex-format descriptor**: each of the
//!   30 descs observed in art.rcf maps to exactly ONE vertex stride, and
//!   the component offsets in [`vertex_layout`] were measured by strict
//!   unit-normal / UV-range scans over every vertex of every mesh
//!   (positions are bbox-conformant at offset 0 for 100% of sampled
//!   vertices of every desc).  Unknown descs fail closed.
//! * `0x00019000` **Texture**: pascal name + 9×u32
//!   `[version(14000), width, height, 8, mipCount, 1, 0, 0, 0]` + child
//!   run `[0x00019006 Image (, 0x00019003)]` — version 14000 on
//!   4 775/4 775 textures.
//! * `0x00019006` **Image**: pascal name + 6×u32 `[1, w, h, 8, mips, 1]` +
//!   4-byte fourcc (`DXT1`/`DXT3`/`DXT5`) + children
//!   `[0x00019002 ImageData, 0x00019003 SourcePath]`.
//! * `0x00019002` **ImageData**: `u32 byteLen` + a complete embedded DDS
//!   file (`DDS ` magic, 128-B header) — `childSize == 4 + byteLen` and
//!   the DDS magic hold on 4 765/4 765 image-data chunks corpus-wide.
//! * `0x00019003` **SourcePath**: NUL-padded ASCII exporter path
//!   (artifact; parsed as raw bytes).
//! * `0x00023000` **Material**: pascal name + `u32 1` + `u32 3` + child
//!   run of `0x00023001/2/3` param blocks: pascal name + f32 run
//!   (+ ≤3 trailing bytes on some blocks — kept raw).
//!
//! * **PrimitiveGroup `0x00010014` (block 27, cracked)**: NOT a draw-range
//!   list — it is the mesh's OWN vertex declaration.  Payload = 28-B
//!   header `[0x00020001][kind 2][vertexBytes == vertexCount·stride]
//!   [code, low nibble == element count][hash 0x2c929929][0][0]`
//!   + (N−1) × 17-B element blocks `[u16 stride][u16 n][u8 class]
//!   [u24 attribute id BE][u32 y][0][u8 byteOffset][0][0][0]` + one 5-B
//!   final block `[u16 stride][u16 n][u8 flag]` (the position element,
//!   offset 0 implicit).  Attribute ids → semantics PROVEN cross-fixture:
//!   `0x094536`/`0x0a4536`/`0x0b4536` = UV sets 0/1/2, `0xe7bc06` =
//!   normal, `0x456217` = tangent; `0x24c46f`/`0x781c3f` are the two
//!   trailing blocks of the `0x6000…` descs (semantics UNKNOWN — see
//!   [`AttrSemantic::Extra4`]).  The decl CORRECTED the measured table
//!   twice: desc `0xB033` carries THREE UV sets at 20/28/36 and desc
//!   `0xB032` carries TWO at 20/28 (both old single `uv@24` rows were
//!   artifacts — 24 is uv0's V half; block 28 proved 0xB032 on real
//!   decls from beam002/skyLight001/bloodtoxFacility meshes).  Element
//!   `0x04fc98` sits at offset 16 on EVERY 0xA/0xB-family mesh (decl-
//!   proven on 0xA021/A031/A032/B031/B032/B033 + 0x6000B032 samples);
//!   its 4 bytes hold per-mesh-constant RGB-ish values with a VARYING
//!   4th byte (0x7f7f7fff white, 0x7f7f7f4c gray-alpha-76, ffffff00 /
//!   ffffff255 mixed) — LIKELY an RGBA8 vertex color, kept `Unknown`.
//!   The final block's flag byte is NOT a static/skinned marker: the
//!   real 0xA021 decl (fixture mesh 56ea1b5d) carries flag 0 on a
//!   static mesh.  See [`decode_primitive_group`].
//!
//! What remains UNKNOWN (fail-closed, never guessed): the meaning of the
//! 4-byte f32 scalar between position and UV in several descs (small
//! positive values like 0.2 / 2.0; the decl does not describe it), the
//! semantics of the two `0x6000…` trailing blocks (`0x24c46f` = 16 B
//! vec4 with w==0 on every vertex ever sampled, xyz small floats ±0.4;
//! `0x781c3f` = 12 B vec3; NOT skin weights/indices — negatives, zeros
//! and per-mesh-constant values observed; values are identical for the
//! same topology shared across two car variants while positions differ;
//! not an affine function of position; the family is car panels +
//! breakable props), the `n` and `y` words of a declared element, the
//! tangent offset for desc `0xB021`, the final block's flag byte, the
//! two zero fields of the Texture header, and the `ÿÿÿÿ` (0xFFFFFFFF)
//! fourcc carried by 8 textures (uncompressed? — decode refuses them).

#![forbid(unsafe_code)]



// ---------------------------------------------------------------------------
// PROVEN chunk type ids (corpus-wide, 2 219 art.rcf members)
// ---------------------------------------------------------------------------

/// `0x00010000` — Shape (geometry container; holds Mesh children).
pub const CHUNK_TYPE_SHAPE: u32 = 0x0001_0000;
/// `0x00010020` — Mesh (vertex/index buffers + bounds + primitive group).
pub const CHUNK_TYPE_MESH: u32 = 0x0001_0020;
/// `0x00010012` — VertexBuffer.
pub const CHUNK_TYPE_VERTEX_BUFFER: u32 = 0x0001_0012;
/// `0x00010013` — IndexBuffer (u16 LE indices).
pub const CHUNK_TYPE_INDEX_BUFFER: u32 = 0x0001_0013;
/// `0x00010014` — PrimitiveGroup: the mesh's own vertex declaration
/// (decoded by [`decode_primitive_group`] since block 27).
pub const CHUNK_TYPE_PRIMITIVE_GROUP: u32 = 0x0001_0014;
/// `0x00010003` — BoundBox (min, max as 6×f32).
pub const CHUNK_TYPE_BOUND_BOX: u32 = 0x0001_0003;
/// `0x00010004` — BoundSphere (center, radius as 4×f32).
pub const CHUNK_TYPE_BOUND_SPHERE: u32 = 0x0001_0004;
/// `0x00010021` — ShapeInfo (u32 0 + 7×u32 32 on all 3 877 shapes;
/// semantics UNKNOWN).
pub const CHUNK_TYPE_SHAPE_INFO: u32 = 0x0001_0021;
/// `0x00019000` — Texture.
pub const CHUNK_TYPE_TEXTURE: u32 = 0x0001_9000;
/// `0x00019006` — Image (named, dims + fourcc, holds ImageData).
pub const CHUNK_TYPE_IMAGE: u32 = 0x0001_9006;
/// `0x00019002` — ImageData (u32 byteLen + embedded DDS file).
pub const CHUNK_TYPE_IMAGE_DATA: u32 = 0x0001_9002;
/// `0x00019003` — Image source path (exporter artifact).
pub const CHUNK_TYPE_IMAGE_SOURCE_PATH: u32 = 0x0001_9003;
/// `0x00023000` — Material.
pub const CHUNK_TYPE_MATERIAL: u32 = 0x0002_3000;
/// `0x00011015` — pure3dShader: the art-side shader template a Mesh binds
/// to BY NAME (mesh name == shader name).  Carries a readable engine
/// shader FAMILY name (e.g. `env_car_tire`, `1c1s1n`) plus named param
/// children (0x11016…0x11022).
pub const CHUNK_TYPE_SHADER: u32 = 0x0001_1015;
/// pure3dShader version constant (all 4 004 corpus occurrences).
pub const SHADER_VERSION: u32 = 256;

/// `0x00011009` — the shader-INSTANCE chunk (block-37 PROVEN: the only
/// content-side creator of shader instances).  Its first pascal string
/// is the NAME the engine passes to the instance factory
/// (`0x1061bcd0`, called at `0x10b95948` inside the chunk's parse
/// `0x10b95780`; loader vtable `0x10eafc50`, ctor stores the chunk id
/// at object+8).  12 occurrences in `frontendshaders.p3d` with
/// readable names (`CxformGauraud`, `hud_2dmarker`, …).  Child arms
/// walk sub-chunks 0x1100A/0x1100E (the 0x1100E arm reads a second
/// pascal string).
pub const CHUNK_TYPE_SHADER_INSTANCE: u32 = 0x0001_1009;

/// VertexBuffer/IndexBuffer header magic (all 7 171 + 7 171 occurrences).
pub const BUFFER_MAGIC: u32 = 0x0002_0001;
/// Texture chunk version (all 4 775 occurrences).
pub const TEXTURE_VERSION: u32 = 14_000;

// ---------------------------------------------------------------------------
// Errors — every variant carries the offending offset / values, fail closed
// ---------------------------------------------------------------------------

/// Decoder errors.  Nothing is guessed: a field that does not match the
/// proven layout is an error, not a fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtError {
    /// The chunk (or a child) does not end where its size says it does.
    NotExactCover {
        /// Offset where the walk failed.
        offset: usize,
        /// Declared enclosing end.
        end: usize,
    },
    /// A chunk prelude claims a size smaller than the 12-byte prelude or
    /// runs past the enclosing end.
    BadChunkSize {
        /// Chunk type id.
        chunk_type: u32,
        /// Chunk start offset.
        offset: usize,
        /// Declared size.
        size: usize,
        /// Enclosing end.
        end: usize,
    },
    /// Data ended before a fixed-size read completed.
    Truncated {
        /// Offset of the read.
        offset: usize,
        /// Bytes needed.
        need: usize,
        /// Bytes available.
        got: usize,
    },
    /// A u8-Pascal string length ran past the enclosing end.
    BadPascal {
        /// Offset of the length byte.
        offset: usize,
        /// Declared length.
        len: usize,
    },
    /// Buffer magic differed from [`BUFFER_MAGIC`].
    BadBufferMagic {
        /// Offset of the magic.
        offset: usize,
        /// Value found.
        got: u32,
    },
    /// Vertex data size is not `vertexCount * stride`.
    BadVertexDataSize {
        /// Declared size field.
        declared: u32,
        /// Expected `vertexCount * stride`.
        want: usize,
    },
    /// Index data size is not `indexCount * 2`.
    BadIndexDataSize {
        /// Declared size field.
        declared: u32,
        /// Expected `indexCount * 2`.
        want: usize,
    },
    /// An index refers past the last vertex.
    IndexOutOfRange {
        /// Index value.
        index: u16,
        /// Vertex count.
        vertices: u32,
    },
    /// The mesh `desc` word is not in the proven layout table.
    UnknownVertexDesc {
        /// The unrecognised descriptor value.
        desc: u32,
    },
    /// A Mesh chunk has no VertexBuffer/IndexBuffer children (the two
    /// legacy-family meshes in art.rcf: children 0x00010005/06/07/08/0A/28).
    UnsupportedMeshFamily {
        /// Child types actually found.
        children: Vec<u32>,
    },
    /// A fixed-size bound chunk had the wrong payload size.
    BadBoundsSize {
        /// Chunk type id.
        chunk_type: u32,
        /// Payload bytes found.
        got: usize,
    },
    /// A version field disagreed with its corpus-wide constant.
    UnexpectedVersion {
        /// Offset of the field.
        offset: usize,
        /// Value found.
        got: u32,
        /// The corpus-wide constant.
        want: u32,
    },
    /// Texture version differed from [`TEXTURE_VERSION`].
    BadTextureVersion {
        /// Offset of the version field.
        offset: usize,
        /// Value found.
        got: u32,
    },
    /// ImageData length field disagreed with the child chunk size.
    BadImageData {
        /// Offset of the ImageData chunk.
        offset: usize,
        /// Declared DDS byte length.
        declared: u32,
        /// Bytes actually available.
        got: usize,
    },
    /// A world chunk was missing its `"META"` marker (world.rs).
    ExpectedMeta {
        /// Offset where the marker was expected.
        offset: usize,
    },
    /// A world placement field violated its proven layout (world.rs).
    BadPlacement {
        /// Offset of the offending field.
        offset: usize,
        /// What was wrong.
        why: String,
    },
    /// A PrimitiveGroup payload violated its proven structure (block 27).
    BadPrimitiveGroup {
        /// Offset of the chunk (or of the offending field).
        offset: usize,
        /// What was wrong.
        why: String,
    },
}

impl std::fmt::Display for ArtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArtError::NotExactCover { offset, end } => {
                write!(f, "chunk run at {offset:#x} does not exactly cover ..{end:#x}")
            }
            ArtError::BadChunkSize { chunk_type, offset, size, end } => {
                write!(f, "chunk {chunk_type:08x} at {offset:#x}: size {size} past end {end:#x}")
            }
            ArtError::Truncated { offset, need, got } => {
                write!(f, "read of {need} B at {offset:#x} past end ({got} B)")
            }
            ArtError::BadPascal { offset, len } => {
                write!(f, "pascal string at {offset:#x}: length {len} past end")
            }
            ArtError::BadBufferMagic { offset, got } => {
                write!(f, "buffer magic at {offset:#x}: {got:#010x}, want {BUFFER_MAGIC:#010x}")
            }
            ArtError::BadVertexDataSize { declared, want } => {
                write!(f, "vertex data {declared} B, want {want} (count*stride)")
            }
            ArtError::BadIndexDataSize { declared, want } => {
                write!(f, "index data {declared} B, want {want} (count*2)")
            }
            ArtError::IndexOutOfRange { index, vertices } => {
                write!(f, "index {index} >= vertex count {vertices}")
            }
            ArtError::UnknownVertexDesc { desc } => {
                write!(f, "vertex desc {desc:#x} not in the proven layout table")
            }
            ArtError::UnsupportedMeshFamily { children } => {
                write!(f, "mesh without VertexBuffer/IndexBuffer (children {children:08x?})")
            }
            ArtError::BadBoundsSize { chunk_type, got } => {
                write!(f, "bounds chunk {chunk_type:08x}: payload {got} B")
            }
            ArtError::BadTextureVersion { offset, got } => {
                write!(f, "texture version {got} at {offset:#x}, want {TEXTURE_VERSION}")
            }
            ArtError::UnexpectedVersion { offset, got, want } => {
                write!(f, "version {got} at {offset:#x}, want {want}")
            }
            ArtError::BadImageData { offset, declared, got } => {
                write!(f, "image data at {offset:#x}: declared {declared} B, available {got}")
            }
            ArtError::ExpectedMeta { offset } => {
                write!(f, "expected \"META\" marker at {offset:#x}")
            }
            ArtError::BadPlacement { offset, why } => {
                write!(f, "bad placement at {offset:#x}: {why}")
            }
            ArtError::BadPrimitiveGroup { offset, why } => {
                write!(f, "PrimitiveGroup at {offset:#x}: {why}")
            }
        }
    }
}
impl std::error::Error for ArtError {}

// ---------------------------------------------------------------------------
// Low-level readers (all bounds-checked, all fail closed)
// ---------------------------------------------------------------------------

fn u32_at(buf: &[u8], off: usize) -> Result<u32, ArtError> {
    if off + 4 > buf.len() {
        return Err(ArtError::Truncated { offset: off, need: 4, got: buf.len() - off.min(buf.len()) });
    }
    Ok(u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]))
}

fn f32_at(buf: &[u8], off: usize) -> Result<f32, ArtError> {
    Ok(f32::from_bits(u32_at(buf, off)?))
}

/// Read a u8-length-prefixed Pascal string (length byte includes any
/// trailing NUL the exporter wrote).  Returns (string, offset-after).
fn pascal_at(buf: &[u8], off: usize, end: usize) -> Result<(String, usize), ArtError> {
    if off >= end || off >= buf.len() {
        return Err(ArtError::BadPascal { offset: off, len: 0 });
    }
    let n = buf[off] as usize;
    if off + 1 + n > end || off + 1 + n > buf.len() {
        return Err(ArtError::BadPascal { offset: off, len: n });
    }
    let s = String::from_utf8_lossy(&buf[off + 1..off + 1 + n]).into_owned();
    Ok((s, off + 1 + n))
}

/// One child chunk of an exact-cover run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChildChunk {
    /// Chunk type id.
    pub chunk_type: u32,
    /// Per-instance id (== size for mesh/texture data children — see docs).
    pub uid: u32,
    /// Absolute offset of the chunk (the `type` field).
    pub offset: usize,
    /// Total chunk size (prelude included).
    pub size: usize,
}

impl ChildChunk {
    /// Payload start (chunk offset + prelude).
    pub fn payload(&self) -> usize {
        self.offset + 12
    }
    /// Payload byte length.
    pub fn payload_len(&self) -> usize {
        self.size - 12
    }
}

/// Walk `[off, end)` as an exact-cover run of chunks.  Returns `None` when
/// the run is malformed (caller decides whether that is fatal — for proven
/// containers it is).
fn child_chunks(buf: &[u8], off: usize, end: usize) -> Option<Vec<ChildChunk>> {
    let mut out = Vec::new();
    let mut o = off;
    while o + 12 <= end {
        if o + 12 > buf.len() {
            return None;
        }
        let t = u32_at(buf, o).ok()?;
        let uid = u32_at(buf, o + 4).ok()?;
        let size = u32_at(buf, o + 8).ok()? as usize;
        if size < 12 || o + size > end {
            return None;
        }
        out.push(ChildChunk { chunk_type: t, uid, offset: o, size });
        o += size;
    }
    if o != end {
        return None;
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Vertex layout table — measured, corpus-wide (see module docs)
// ---------------------------------------------------------------------------

/// The proven vertex layout for one mesh `desc` word.
///
/// Offsets are byte offsets into one vertex of `stride` bytes.  Every desc
/// ever observed in Prototype art data appears exactly once here; anything
/// else fails closed with [`ArtError::UnknownVertexDesc`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexLayout {
    /// The `desc` word this row was measured from.
    pub desc: u32,
    /// Bytes per vertex.
    pub stride: usize,
    /// Offset of the unit normal ([f32;3]) — `None` when the format has none.
    pub normal_offset: Option<usize>,
    /// Offset of a second unit vector ([f32;3], likely the tangent) —
    /// measured at a 1.000 unit-rate on the 0x3/0xB/0x17 families.
    /// `None` where not measured (only desc `0xB021`).
    pub tangent_offset: Option<usize>,
    /// Offsets of the UV ([f32;2]) sets, first set first.
    pub uv_offsets: &'static [usize],
    /// Offset of the 4-byte `0x04fc98` slot (decl-proven at 16 on every
    /// 0xA/0xB-family mesh; content LIKELY an RGBA8 vertex color — kept
    /// bytes, not channels).  `None` on the other families.
    pub color_offset: Option<usize>,
    /// Byte size of the trailing block pair (only the `0x6000…` descs;
    /// always 28 = 16-B `0x24c46f` vec4 + 12-B `0x781c3f` vec3, decl-
    /// proven on all five sampled descs).  Semantics UNKNOWN.
    pub skin_bytes: usize,
}

/// All 30 descs observed across the 7 171 meshes of art.rcf, each mapping
/// to exactly one stride corpus-wide.
pub const VERTEX_LAYOUTS: &[VertexLayout] = &[
    // — 0x2 family: pos [+ X4] [uv…] [normal12] —
    VertexLayout { desc: 0x2001, stride: 24, normal_offset: None, tangent_offset: None, uv_offsets: &[16], color_offset: None, skin_bytes: 0 },
    VertexLayout { desc: 0x2010, stride: 28, normal_offset: Some(16), tangent_offset: None, uv_offsets: &[], color_offset: None, skin_bytes: 0 },
    VertexLayout { desc: 0x2011, stride: 36, normal_offset: Some(24), tangent_offset: None, uv_offsets: &[16], color_offset: None, skin_bytes: 0 },
    VertexLayout { desc: 0x2012, stride: 44, normal_offset: Some(32), tangent_offset: None, uv_offsets: &[16, 24], color_offset: None, skin_bytes: 0 },
    VertexLayout { desc: 0x2013, stride: 52, normal_offset: Some(40), tangent_offset: None, uv_offsets: &[16, 24, 32], color_offset: None, skin_bytes: 0 },
    VertexLayout { desc: 0x2021, stride: 28, normal_offset: None, tangent_offset: None, uv_offsets: &[20], color_offset: None, skin_bytes: 0 },
    // — 0x3 family: adds a second unit vector (tangent) —
    VertexLayout { desc: 0x3011, stride: 52, normal_offset: Some(24), tangent_offset: Some(36), uv_offsets: &[16], color_offset: None, skin_bytes: 0 },
    VertexLayout { desc: 0x3012, stride: 60, normal_offset: Some(32), tangent_offset: Some(44), uv_offsets: &[16, 24], color_offset: None, skin_bytes: 0 },
    VertexLayout { desc: 0x3013, stride: 68, normal_offset: Some(40), tangent_offset: Some(52), uv_offsets: &[16, 24, 32], color_offset: None, skin_bytes: 0 },
    // — 0xA family: pos, X4, 0x4fc98 slot @16, [uv…], [normal12] —
    VertexLayout { desc: 0xA020, stride: 20, normal_offset: None, tangent_offset: None, uv_offsets: &[], color_offset: Some(16), skin_bytes: 0 },
    VertexLayout { desc: 0xA021, stride: 28, normal_offset: None, tangent_offset: None, uv_offsets: &[20], color_offset: Some(16), skin_bytes: 0 },
    VertexLayout { desc: 0xA022, stride: 36, normal_offset: None, tangent_offset: None, uv_offsets: &[20, 28], color_offset: Some(16), skin_bytes: 0 },
    VertexLayout { desc: 0xA030, stride: 32, normal_offset: Some(20), tangent_offset: None, uv_offsets: &[], color_offset: Some(16), skin_bytes: 0 },
    VertexLayout { desc: 0xA031, stride: 40, normal_offset: Some(28), tangent_offset: None, uv_offsets: &[20], color_offset: Some(16), skin_bytes: 0 },
    VertexLayout { desc: 0xA032, stride: 48, normal_offset: Some(36), tangent_offset: None, uv_offsets: &[20, 28], color_offset: Some(16), skin_bytes: 0 },
    // — 0xB family: 0xA + tangent —
    //   NOTE (blocks 27–28): the PrimitiveGroup decl PROVES 0xB033 carries
    //   THREE uv sets at 20/28/36 and 0xB032 carries TWO at 20/28 (the old
    //   single `uv@24` rows were measurement artifacts — 24 is uv0's V
    //   half; real decls: beam002InitialShape/skyLight001InitialShape
    //   members ff56868c/bcaf0e88 + bloodtoxFacilityFiltrationUnit001-
    //   InitialShape 6e229aee for the skinned row).  The decl also proves
    //   the tangent spans 16 B (xyz + trailing f32 word, values ±1.0-ish;
    //   only xyz are consumed).  0xB021 keeps its measured row (no decl
    //   sample; stride arithmetic leaves 8 B unaccounted — fail-closed).
    VertexLayout { desc: 0xB021, stride: 44, normal_offset: Some(28), tangent_offset: None, uv_offsets: &[20], color_offset: Some(16), skin_bytes: 0 },
    VertexLayout { desc: 0xB031, stride: 56, normal_offset: Some(28), tangent_offset: Some(40), uv_offsets: &[20], color_offset: Some(16), skin_bytes: 0 },
    VertexLayout { desc: 0xB032, stride: 64, normal_offset: Some(36), tangent_offset: Some(48), uv_offsets: &[20, 28], color_offset: Some(16), skin_bytes: 0 },
    VertexLayout { desc: 0xB033, stride: 72, normal_offset: Some(44), tangent_offset: Some(56), uv_offsets: &[20, 28, 36], color_offset: Some(16), skin_bytes: 0 },
    // — 0x16 family: pos, X4, 0x4fc98 slot @16, [uv…], [normal12] —
    VertexLayout { desc: 0x16001, stride: 32, normal_offset: None, tangent_offset: None, uv_offsets: &[24], color_offset: None, skin_bytes: 0 },
    VertexLayout { desc: 0x16011, stride: 44, normal_offset: Some(32), tangent_offset: None, uv_offsets: &[16], color_offset: None, skin_bytes: 0 },
    VertexLayout { desc: 0x16012, stride: 52, normal_offset: Some(40), tangent_offset: None, uv_offsets: &[16, 24], color_offset: None, skin_bytes: 0 },
    // — 0x17 family: 0x16 + tangent —
    VertexLayout { desc: 0x17011, stride: 60, normal_offset: Some(32), tangent_offset: Some(44), uv_offsets: &[16], color_offset: None, skin_bytes: 0 },
    VertexLayout { desc: 0x17012, stride: 68, normal_offset: Some(40), tangent_offset: Some(52), uv_offsets: &[16, 24], color_offset: None, skin_bytes: 0 },
    VertexLayout { desc: 0x17013, stride: 76, normal_offset: Some(48), tangent_offset: Some(60), uv_offsets: &[16, 24, 32], color_offset: None, skin_bytes: 0 },
    // — 0x6000-prefixed: base layout + 28-B trailing block pair at the END
    //   (proven: the base normal/uv/color offsets are unchanged vs the
    //   unskinned descs, so the trailing blocks cannot interleave before
    //   them; decl offsets = stride−28 / stride−12 on all five descs) —
    VertexLayout { desc: 0x6000_2011, stride: 64, normal_offset: Some(24), tangent_offset: None, uv_offsets: &[16], color_offset: None, skin_bytes: 28 },
    VertexLayout { desc: 0x6000_2013, stride: 80, normal_offset: Some(40), tangent_offset: None, uv_offsets: &[16, 24, 32], color_offset: None, skin_bytes: 28 },
    VertexLayout { desc: 0x6000_3011, stride: 80, normal_offset: Some(24), tangent_offset: Some(36), uv_offsets: &[16], color_offset: None, skin_bytes: 28 },
    VertexLayout { desc: 0x6000_B031, stride: 84, normal_offset: Some(28), tangent_offset: Some(40), uv_offsets: &[20], color_offset: Some(16), skin_bytes: 28 },
    VertexLayout { desc: 0x6000_B032, stride: 92, normal_offset: Some(36), tangent_offset: Some(48), uv_offsets: &[20, 28], color_offset: Some(16), skin_bytes: 28 },
];

/// Look up the proven layout for a `desc` word; `None` = not observed in
/// any real art file (callers must fail closed, not guess).
pub fn vertex_layout(desc: u32) -> Option<&'static VertexLayout> {
    VERTEX_LAYOUTS.iter().find(|l| l.desc == desc)
}

// ---------------------------------------------------------------------------
// PrimitiveGroup 0x00010014 — the mesh's own vertex declaration (block 27)
// ---------------------------------------------------------------------------

/// A PrimitiveGroup payload shares the inline-buffer magic with
/// VertexBuffer/IndexBuffer ([`BUFFER_MAGIC`]) and carries `kind == 2`.
pub const PRIMITIVE_GROUP_KIND: u32 = 2;
/// Word[4] of every PrimitiveGroup payload observed (11 real meshes across
/// 7 descs + every sampled cells section) — validated.
pub const PRIMITIVE_GROUP_DESCRIPTOR_HASH: u32 = 0x2C92_9929;
/// Payload header: magic + kind + vertexBytes + code + hash + 0 + 0.
pub const PRIMITIVE_GROUP_HEADER: usize = 28;
/// One full element block (stride, n, class, attr24, y, pad, offset, 3 pads).
pub const PRIMITIVE_GROUP_BLOCK: usize = 17;
/// The final 5-byte block: stride + n + flag (the position element).
pub const PRIMITIVE_GROUP_FINAL_BLOCK: usize = 5;

/// Semantic of one declared vertex element, from its 24-bit attribute id.
///
/// Mapping PROVEN by cross-fixture agreement: the same id appears with
/// byte offsets that match the independently measured layout table on
/// every desc where the two overlap (uv/uv2/normal/tangent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttrSemantic {
    /// Position — the final 5-B block; offset 0 is implicit.
    Position,
    /// One UV set: `0x094536` → 0, `0x0a4536` → 1, `0x0b4536` → 2.
    Uv(u32),
    /// Unit normal (`0xe7bc06`).
    Normal,
    /// Unit tangent (`0x456217`) — spans 16 B (xyz + trailing f32 word,
    /// values ±1.0-ish; decl-proven spacing, only xyz consumed).
    Tangent,
    /// The FIRST trailing block of the `0x6000…` descs (`0x24c46f`, 16 B):
    /// a vec4 whose w == 0 on every vertex ever sampled (1 500+ meshes),
    /// xyz small floats (±0.4).  Block 28 DISPROVED the old `Skin` guess:
    /// values are negative / zero / per-mesh-constant (impossible for
    /// weights), identical across car variants sharing topology, and not
    /// an affine function of position.  Semantics UNKNOWN — recorded and
    /// exposed as [`MeshChunk::extra4`].
    Extra4,
    /// The SECOND trailing block of the `0x6000…` descs (`0x781c3f`, 12 B):
    /// a vec3 with the same disproof set as [`AttrSemantic::Extra4`].
    /// Semantics UNKNOWN — exposed as [`MeshChunk::extra3`].
    Extra3,
    /// Not yet mapped.  NOTE: `0x04fc98` (offset 16 on every 0xA/0xB-family
    /// mesh, decl-proven) holds 4 bytes that look like an RGBA8 vertex
    /// color (per-mesh-constant RGB, varying 4th byte: 0xff / 0x7f / 0x4c
    /// / 0x00 observed) — still recorded as `Unknown` until the varying
    /// byte is explained (see [`MeshChunk::color_bytes`]).
    Unknown(u32),
}

/// One declared vertex element (a full 17-B block).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrimitiveElement {
    /// Stride word — equals the mesh stride on every sample (validated).
    pub stride: u16,
    /// Second word — recorded verbatim (2..4 per desc; semantics UNKNOWN).
    pub n: u16,
    /// Class byte — 0 or 1 observed (recorded).
    pub class: u8,
    /// 24-bit big-endian attribute id (see [`attr_semantic`]).
    pub attr: u32,
    /// Word Y — recorded verbatim (small values, per desc family; UNKNOWN).
    pub field_y: u32,
    /// Byte offset of the element inside one vertex (multiple of 4, < stride).
    pub offset: u8,
    /// The semantic this attribute id maps to.
    pub semantic: AttrSemantic,
}

/// The final 5-B block of a PrimitiveGroup payload — the position element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinalElement {
    /// Stride word (validated against the mesh stride by [`decode_mesh`]).
    pub stride: u16,
    /// Second word — 2..4 per desc (recorded).
    pub n: u16,
    /// Flag byte — recorded verbatim.  NOT a static/skinned marker (the
    /// real 0xA021 decl carries 0 on a static mesh; 1 appears on the
    /// tangent-bearing A031/B031/B032/B033 statics and 0 on every
    /// `0x6000…` mesh sampled).  Semantics UNKNOWN.
    pub flag: u8,
}

/// The decoded `0x00010014` PrimitiveGroup — a mesh's own vertex
/// declaration (block 27 crack; PROVEN structure on 11 real meshes
/// across 7 descs, DFS-unique tilings).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrimitiveGroup {
    /// `vertexCount * stride` — validated against the mesh header.
    pub vertex_bytes: u32,
    /// Word[3] verbatim; the LOW NIBBLE equals the element count (PROVEN).
    pub code: u32,
    /// Word[4] — [`PRIMITIVE_GROUP_DESCRIPTOR_HASH`] on every sample (validated).
    pub descriptor_hash: u32,
    /// The declared elements in payload order (offsets strictly increasing).
    pub elements: Vec<PrimitiveElement>,
    /// The final position element (the payload ends with exactly 5 B).
    pub final_element: FinalElement,
}

/// Map a 24-bit attribute id to its semantic (PROVEN cross-fixture).
pub fn attr_semantic(attr: u32) -> AttrSemantic {
    match attr {
        0x09_4536 => AttrSemantic::Uv(0),
        0x0a_4536 => AttrSemantic::Uv(1),
        0x0b_4536 => AttrSemantic::Uv(2),
        0xe7_bc06 => AttrSemantic::Normal,
        0x45_6217 => AttrSemantic::Tangent,
        0x24_c46f => AttrSemantic::Extra4,
        0x78_1c3f => AttrSemantic::Extra3,
        other => AttrSemantic::Unknown(other),
    }
}

/// Decode the PrimitiveGroup chunk starting at `offset` (total `size`).
///
/// Structure (block 27, PROVEN on 11 real meshes — DFS-unique tiling):
/// payload = 28-B header + (N−1) × 17-B element blocks + one 5-B final
/// block, where N = low nibble of word[3] (`code`).  Fail-closed: every
/// reserved byte, stride echo and offset invariant is enforced.
pub fn decode_primitive_group(
    buf: &[u8],
    offset: usize,
    size: usize,
) -> Result<PrimitiveGroup, ArtError> {
    let end = offset + size;
    if end > buf.len() {
        return Err(ArtError::Truncated { offset, need: size, got: buf.len() - offset });
    }
    let p = offset + 12;
    let plen = size - 12;
    let min = PRIMITIVE_GROUP_HEADER + PRIMITIVE_GROUP_FINAL_BLOCK;
    if plen < min {
        return Err(ArtError::BadPrimitiveGroup {
            offset,
            why: format!("payload {plen} B < minimum {min} B"),
        });
    }
    let magic = u32_at(buf, p)?;
    if magic != BUFFER_MAGIC {
        return Err(ArtError::BadBufferMagic { offset: p, got: magic });
    }
    let kind = u32_at(buf, p + 4)?;
    if kind != PRIMITIVE_GROUP_KIND {
        return Err(ArtError::BadPrimitiveGroup {
            offset: p + 4,
            why: format!("kind {kind} != {PRIMITIVE_GROUP_KIND}"),
        });
    }
    let vertex_bytes = u32_at(buf, p + 8)?;
    let code = u32_at(buf, p + 12)?;
    let dhash = u32_at(buf, p + 16)?;
    if dhash != PRIMITIVE_GROUP_DESCRIPTOR_HASH {
        return Err(ArtError::BadPrimitiveGroup {
            offset: p + 16,
            why: format!("descriptor hash {dhash:#x} != {PRIMITIVE_GROUP_DESCRIPTOR_HASH:#x}"),
        });
    }
    for i in 0..2 {
        let z = u32_at(buf, p + 20 + i * 4)?;
        if z != 0 {
            return Err(ArtError::BadPrimitiveGroup {
                offset: p + 20 + i * 4,
                why: format!("reserved word {i} = {z:#x}, expected 0"),
            });
        }
    }
    let want_elements = (code & 0xF) as usize;
    if want_elements == 0 {
        return Err(ArtError::BadPrimitiveGroup {
            offset: p + 12,
            why: "code low nibble declares 0 elements".into(),
        });
    }
    let mut o = p + PRIMITIVE_GROUP_HEADER;
    let mut elements = Vec::with_capacity(want_elements.saturating_sub(1));
    let mut prev_offset: usize = 0;
    let mut final_element = None;
    for i in 0..want_elements {
        let last = i + 1 == want_elements;
        if last {
            if o + PRIMITIVE_GROUP_FINAL_BLOCK != end {
                return Err(ArtError::BadPrimitiveGroup {
                    offset,
                    why: format!(
                        "final block must end the payload: ends at {}, payload end {end}",
                        o + PRIMITIVE_GROUP_FINAL_BLOCK
                    ),
                });
            }
            final_element = Some(FinalElement {
                stride: u16::from_le_bytes([buf[o], buf[o + 1]]),
                n: u16::from_le_bytes([buf[o + 2], buf[o + 3]]),
                flag: buf[o + 4],
            });
            o += PRIMITIVE_GROUP_FINAL_BLOCK;
        } else {
            if o + PRIMITIVE_GROUP_BLOCK > end {
                return Err(ArtError::Truncated {
                    offset: o,
                    need: PRIMITIVE_GROUP_BLOCK,
                    got: end - o,
                });
            }
            let stride = u16::from_le_bytes([buf[o], buf[o + 1]]);
            let n = u16::from_le_bytes([buf[o + 2], buf[o + 3]]);
            let class = buf[o + 4];
            let attr =
                ((buf[o + 5] as u32) << 16) | ((buf[o + 6] as u32) << 8) | buf[o + 7] as u32;
            let field_y = u32_at(buf, o + 8)?;
            if buf[o + 12] != 0 || buf[o + 14] != 0 || buf[o + 15] != 0 {
                return Err(ArtError::BadPrimitiveGroup {
                    offset: o + 12,
                    why: "element block reserved bytes are not zero".into(),
                });
            }
            let offset8 = buf[o + 13] as usize;
            if offset8 % 4 != 0 {
                return Err(ArtError::BadPrimitiveGroup {
                    offset: o + 13,
                    why: format!("element offset {offset8} is not 4-byte aligned"),
                });
            }
            if stride == 0 || offset8 >= stride as usize {
                return Err(ArtError::BadPrimitiveGroup {
                    offset: o + 13,
                    why: format!("element offset {offset8} >= stride {stride}"),
                });
            }
            if offset8 <= prev_offset {
                return Err(ArtError::BadPrimitiveGroup {
                    offset: o + 13,
                    why: format!("element offset {offset8} not strictly after {prev_offset}"),
                });
            }
            prev_offset = offset8;
            elements.push(PrimitiveElement {
                stride,
                n,
                class,
                attr,
                field_y,
                offset: buf[o + 13],
                semantic: attr_semantic(attr),
            });
            o += PRIMITIVE_GROUP_BLOCK;
        }
    }
    debug_assert_eq!(o, end);
    Ok(PrimitiveGroup {
        vertex_bytes,
        code,
        descriptor_hash: dhash,
        elements,
        final_element: final_element.expect("want_elements >= 1 guarantees the final block"),
    })
}

// ---------------------------------------------------------------------------
// Decoded mesh / shape
// ---------------------------------------------------------------------------

/// The 9×u32 Mesh header after the name (offsets 0..36 of the post-name
/// area).  Field meanings are PROVEN for [1] (desc), [2] (vertex count)
/// and [3] (index count); the rest are recorded verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshHeader {
    /// Field 0 — always 0 corpus-wide (recorded).
    pub field0: u32,
    /// Field 1 — the vertex-format descriptor (see [`vertex_layout`]).
    pub desc: u32,
    /// Field 2 — vertex count (validated against the VertexBuffer size).
    pub vertex_count: u32,
    /// Field 3 — index count (validated against the IndexBuffer size).
    pub index_count: u32,
    /// Fields 4..9 verbatim (0, 1, 1, 0, 0 on the barrier fixture).
    pub tail: [u32; 5],
}

/// One decoded Mesh chunk (`0x00010020`).
#[derive(Debug, Clone)]
pub struct MeshChunk {
    /// Mesh name — PROVEN to equal a `pure3dShader` chunk name (the
    /// mesh→shader binding).
    pub name: String,
    /// The 9×u32 header.
    pub header: MeshHeader,
    /// The proven layout for `header.desc`.
    pub layout: &'static VertexLayout,
    /// Raw vertex bytes (`vertex_count × stride`), verbatim.
    pub vertex_data: Vec<u8>,
    /// Decoded positions (always present, offset 0).
    pub positions: Vec<[f32; 3]>,
    /// Decoded normals (empty when the layout has none).
    pub normals: Vec<[f32; 3]>,
    /// Decoded tangents (empty when the layout has none measured).
    pub tangents: Vec<[f32; 3]>,
    /// Decoded first UV set (empty when the layout has none).
    pub uvs: Vec<[f32; 2]>,
    /// Decoded SECOND UV set (empty when the layout has none) — decl- or
    /// table-driven; matches `uv_offsets[1]` everywhere both exist.
    pub uvs1: Vec<[f32; 2]>,
    /// The 4-B `0x04fc98` slot bytes per vertex (offset from
    /// [`VertexLayout::color_offset`] / the decl) — LIKELY an RGBA8 vertex
    /// color; kept as raw bytes (the 4th byte varies 0x00/0x4c/0x7f/0xff
    /// across samples).  `None` when the layout has no such slot.
    pub color_bytes: Option<Vec<[u8; 4]>>,
    /// The FIRST trailing block of the `0x6000…` descs (`0x24c46f`, 16 B,
    /// read as f32×4 — w == 0 on every sample; semantics UNKNOWN).
    pub extra4: Vec<[f32; 4]>,
    /// The SECOND trailing block of the `0x6000…` descs (`0x781c3f`, 12 B,
    /// read as f32×3; semantics UNKNOWN).
    pub extra3: Vec<[f32; 3]>,
    /// u16 indices, validated in range.
    pub indices: Vec<u16>,
    /// Mesh BoundBox child payload (min, max).
    pub bound_box: Option<[f32; 6]>,
    /// Mesh BoundSphere child payload (center, radius).
    pub bound_sphere: Option<[f32; 4]>,
    /// The mesh's own vertex declaration (`0x00010014` child, block 27).
    /// `None` when the mesh carries no PrimitiveGroup child.
    pub primitive_group: Option<PrimitiveGroup>,
    /// Child chunk types in file order (for passthrough re-encoding).
    pub child_types: Vec<u32>,
}

/// Read one [f32; N] starting at `off`.
fn f32n<const N: usize>(buf: &[u8], off: usize) -> Result<[f32; N], ArtError> {
    let mut out = [0.0f32; N];
    for (i, v) in out.iter_mut().enumerate() {
        *v = f32_at(buf, off + i * 4)?;
    }
    Ok(out)
}

/// Decode the Mesh chunk that starts at `chunk.offset` (the `type` field)
/// with total size `chunk.size`.  `buf` is the whole P3D (or any buffer
/// containing the chunk at those absolute offsets).
pub fn decode_mesh(buf: &[u8], offset: usize, size: usize) -> Result<MeshChunk, ArtError> {
    let end = offset + size;
    if end > buf.len() {
        return Err(ArtError::Truncated { offset, need: size, got: buf.len() - offset });
    }
    let off = offset + 12; // prelude
    let version = u32_at(buf, off)?;
    if version != 0 {
        // corpus-wide constant; fail closed rather than guess
        return Err(ArtError::UnexpectedVersion { offset: off, got: version, want: 0 });
    }
    let (name, after_name) = pascal_at(buf, off + 4, end)?;
    if after_name + 36 > end {
        return Err(ArtError::Truncated { offset: after_name, need: 36, got: end - after_name });
    }
    let field0 = u32_at(buf, after_name)?;
    let desc = u32_at(buf, after_name + 4)?;
    let vertex_count = u32_at(buf, after_name + 8)?;
    let index_count = u32_at(buf, after_name + 12)?;
    let tail = [
        u32_at(buf, after_name + 16)?,
        u32_at(buf, after_name + 20)?,
        u32_at(buf, after_name + 24)?,
        u32_at(buf, after_name + 28)?,
        u32_at(buf, after_name + 32)?,
    ];
    let header = MeshHeader { field0, desc, vertex_count, index_count, tail };
    let layout = vertex_layout(desc).ok_or(ArtError::UnknownVertexDesc { desc })?;

    let children = child_chunks(buf, after_name + 36, end)
        .ok_or(ArtError::NotExactCover { offset: after_name + 36, end })?;
    let child_types: Vec<u32> = children.iter().map(|c| c.chunk_type).collect();

    let vb = children.iter().find(|c| c.chunk_type == CHUNK_TYPE_VERTEX_BUFFER);
    let ib = children.iter().find(|c| c.chunk_type == CHUNK_TYPE_INDEX_BUFFER);
    let (vb, ib) = match (vb, ib) {
        (Some(vb), Some(ib)) => (vb, ib),
        _ => return Err(ArtError::UnsupportedMeshFamily { children: child_types }),
    };

    // -- VertexBuffer: magic + field2 + dataSize + data -------------------
    let voff = vb.payload();
    let magic = u32_at(buf, voff)?;
    if magic != BUFFER_MAGIC {
        return Err(ArtError::BadBufferMagic { offset: voff, got: magic });
    }
    let _field2 = u32_at(buf, voff + 4)?; // 2 on all corpus VBs; recorded, unchecked
    let data_size = u32_at(buf, voff + 8)? as usize;
    let want = vertex_count as usize * layout.stride;
    if data_size != want {
        return Err(ArtError::BadVertexDataSize { declared: data_size as u32, want });
    }
    let vdata_start = voff + 12;
    if vdata_start + want > buf.len() {
        return Err(ArtError::Truncated { offset: vdata_start, need: want, got: buf.len() - vdata_start });
    }
    let vertex_data = buf[vdata_start..vdata_start + want].to_vec();

    // -- IndexBuffer: magic + field2(0) + dataSize + u16 LE indices -------
    let ioff = ib.payload();
    let imagic = u32_at(buf, ioff)?;
    if imagic != BUFFER_MAGIC {
        return Err(ArtError::BadBufferMagic { offset: ioff, got: imagic });
    }
    let _ifield2 = u32_at(buf, ioff + 4)?; // 0 on all corpus IBs
    let isize_ = u32_at(buf, ioff + 8)? as usize;
    let iwant = index_count as usize * 2;
    if isize_ != iwant {
        return Err(ArtError::BadIndexDataSize { declared: isize_ as u32, want: iwant });
    }
    let idata = ioff + 12;
    if idata + iwant > buf.len() {
        return Err(ArtError::Truncated { offset: idata, need: iwant, got: buf.len() - idata });
    }
    let mut indices = Vec::with_capacity(index_count as usize);
    for i in 0..index_count as usize {
        let o = idata + i * 2;
        let idx = u16::from_le_bytes([buf[o], buf[o + 1]]);
        if u32::from(idx) >= vertex_count {
            return Err(ArtError::IndexOutOfRange { index: idx, vertices: vertex_count });
        }
        indices.push(idx);
    }

    // -- decode components per the layout ---------------------------------
    let s = layout.stride;
    let mut positions = Vec::with_capacity(vertex_count as usize);
    for i in 0..vertex_count as usize {
        positions.push(f32n::<3>(buf, vdata_start + i * s)?);
    }
    let mut normals = Vec::new();
    if let Some(no) = layout.normal_offset {
        for i in 0..vertex_count as usize {
            normals.push(f32n::<3>(buf, vdata_start + i * s + no)?);
        }
    }
    let mut tangents = Vec::new();
    if let Some(to) = layout.tangent_offset {
        for i in 0..vertex_count as usize {
            tangents.push(f32n::<3>(buf, vdata_start + i * s + to)?);
        }
    }
    let mut uvs = Vec::new();
    if let Some(&uo) = layout.uv_offsets.first() {
        for i in 0..vertex_count as usize {
            uvs.push(f32n::<2>(buf, vdata_start + i * s + uo)?);
        }
    }
    let mut uvs1 = Vec::new();
    if let Some(&uo) = layout.uv_offsets.get(1) {
        for i in 0..vertex_count as usize {
            uvs1.push(f32n::<2>(buf, vdata_start + i * s + uo)?);
        }
    }
    let mut color_bytes = Vec::new();
    if let Some(co) = layout.color_offset {
        for i in 0..vertex_count as usize {
            let b = vdata_start + i * s + co;
            color_bytes.push([buf[b], buf[b + 1], buf[b + 2], buf[b + 3]]);
        }
    }
    let mut extra4 = Vec::new();
    let mut extra3 = Vec::new();
    if layout.skin_bytes == 28 {
        // The trailing block pair sits at the END of each vertex: the
        // 16-B `0x24c46f` vec4 at stride−28 and the 12-B `0x781c3f` vec3
        // at stride−12 (decl-proven on all five sampled descs).
        for i in 0..vertex_count as usize {
            extra4.push(f32n::<4>(buf, vdata_start + i * s + s - 28)?);
            extra3.push(f32n::<3>(buf, vdata_start + i * s + s - 12)?);
        }
    }

    // -- bounds ------------------------------------------------------------
    let bb = children.iter().find(|c| c.chunk_type == CHUNK_TYPE_BOUND_BOX);
    let bound_box = match bb {
        Some(c) if c.payload_len() == 24 => Some(f32n::<6>(buf, c.payload())?),
        Some(c) => return Err(ArtError::BadBoundsSize { chunk_type: c.chunk_type, got: c.payload_len() }),
        None => None,
    };
    let bs = children.iter().find(|c| c.chunk_type == CHUNK_TYPE_BOUND_SPHERE);
    let bound_sphere = match bs {
        Some(c) if c.payload_len() == 16 => Some(f32n::<4>(buf, c.payload())?),
        Some(c) => return Err(ArtError::BadBoundsSize { chunk_type: c.chunk_type, got: c.payload_len() }),
        None => None,
    };

    // -- PrimitiveGroup: the mesh's own vertex declaration (block 27) -----
    let primitive_group = match children.iter().find(|c| c.chunk_type == CHUNK_TYPE_PRIMITIVE_GROUP) {
        Some(c) => {
            let pg = decode_primitive_group(buf, c.offset, c.size)?;
            // PROVEN cross-checks (11/11 real meshes): the declared vertex
            // byte count and the per-block stride echo the mesh header.
            if pg.vertex_bytes != vertex_count * layout.stride as u32 {
                return Err(ArtError::BadPrimitiveGroup {
                    offset: c.offset,
                    why: format!(
                        "vertex_bytes {} != vertex_count {vertex_count} * stride {}",
                        pg.vertex_bytes, layout.stride
                    ),
                });
            }
            if let Some(e) = pg
                .elements
                .iter()
                .find(|e| e.stride as usize != layout.stride)
            {
                return Err(ArtError::BadPrimitiveGroup {
                    offset: c.offset,
                    why: format!(
                        "element stride {} != mesh stride {}",
                        e.stride,
                        layout.stride
                    ),
                });
            }
            if pg.final_element.stride as usize != layout.stride {
                return Err(ArtError::BadPrimitiveGroup {
                    offset: c.offset,
                    why: format!(
                        "final stride {} != mesh stride {}",
                        pg.final_element.stride,
                        layout.stride
                    ),
                });
            }
            Some(pg)
        }
        None => None,
    };

    // -- decl-corrected UV extraction (block 27) ---------------------------
    // When the mesh declares its own vertex elements, the declared uv0
    // offset is the format's own word and overrides the measured-table
    // heuristic.  PROVEN fix: desc 0xB033 declares uv0@20 — the old table
    // row (uv@24) read uv0's V half mixed with uv1's U half.
    let uvs = match primitive_group
        .as_ref()
        .and_then(|pg| {
            pg.elements
                .iter()
                .find(|e| e.semantic == AttrSemantic::Uv(0))
                .map(|e| e.offset as usize)
        }) {
        Some(uo) if layout.uv_offsets.first() != Some(&uo) => {
            let mut v = Vec::with_capacity(vertex_count as usize);
            for i in 0..vertex_count as usize {
                v.push(f32n::<2>(buf, vdata_start + i * s + uo)?);
            }
            v
        }
        _ => uvs,
    };
    // Same decl-first rule for the SECOND uv set and the color slot, and
    // a cross-check for the trailing blocks (decl offsets must equal the
    // stride−28 / stride−12 positions; else fail closed).
    if let Some(pg) = primitive_group.as_ref() {
        if let Some(e1) = pg.elements.iter().find(|e| e.semantic == AttrSemantic::Uv(1)) {
            let uo = e1.offset as usize;
            if layout.uv_offsets.get(1) != Some(&uo) {
                uvs1.clear();
                for i in 0..vertex_count as usize {
                    uvs1.push(f32n::<2>(buf, vdata_start + i * s + uo)?);
                }
            }
        }
        if let Some(ec) = pg
            .elements
            .iter()
            .find(|e| e.semantic == AttrSemantic::Unknown(0x04_fc98))
        {
            let co = ec.offset as usize;
            if layout.color_offset != Some(co) {
                return Err(ArtError::BadPrimitiveGroup {
                    offset,
                    why: format!("decl color slot @{co} != table {:?}", layout.color_offset),
                });
            }
        }
        if layout.skin_bytes == 28 {
            let e4 = pg
                .elements
                .iter()
                .find(|e| e.semantic == AttrSemantic::Extra4)
                .map(|e| e.offset as usize);
            let e3 = pg
                .elements
                .iter()
                .find(|e| e.semantic == AttrSemantic::Extra3)
                .map(|e| e.offset as usize);
            if e4 != Some(s - 28) || e3 != Some(s - 12) {
                return Err(ArtError::BadPrimitiveGroup {
                    offset,
                    why: format!(
                        "decl trailing blocks at {e4:?}/{e3:?} != stride−28/−12 ({}/{})",
                        s - 28,
                        s - 12
                    ),
                });
            }
        }
    }

    Ok(MeshChunk {
        name,
        header,
        layout,
        vertex_data,
        positions,
        normals,
        tangents,
        uvs,
        uvs1,
        color_bytes: if color_bytes.is_empty() { None } else { Some(color_bytes) },
        extra4,
        extra3,
        indices,
        bound_box,
        bound_sphere,
        primitive_group,
        child_types,
    })
}

/// One decoded Shape chunk (`0x00010000`).
#[derive(Debug, Clone)]
pub struct ShapeChunk {
    /// Shape name (e.g. `"barrierPlasticOS002InitialShape"`).
    pub name: String,
    /// Header u32[0] — 0 on all 3 877 corpus shapes (recorded).
    pub version: u32,
    /// Header u32[1] — varies (1, 2, 3, 4, …; recorded, meaning UNKNOWN).
    pub flag: u32,
    /// Mesh children in file order.
    pub meshes: Vec<MeshChunk>,
    /// Shape-level BoundBox (present on 3 877/3 877 corpus shapes).
    pub bound_box: Option<[f32; 6]>,
    /// Shape-level BoundSphere.
    pub bound_sphere: Option<[f32; 4]>,
    /// Child chunk types in file order.
    pub child_types: Vec<u32>,
}

/// Decode the Shape chunk starting at `offset` with total size `size`.
pub fn decode_shape(buf: &[u8], offset: usize, size: usize) -> Result<ShapeChunk, ArtError> {
    let end = offset + size;
    if end > buf.len() {
        return Err(ArtError::Truncated { offset, need: size, got: buf.len() - offset });
    }
    let (name, after_name) = pascal_at(buf, offset + 12, end)?;
    if after_name + 8 > end {
        return Err(ArtError::Truncated { offset: after_name, need: 8, got: end - after_name });
    }
    let version = u32_at(buf, after_name)?;
    let flag = u32_at(buf, after_name + 4)?;
    let children = child_chunks(buf, after_name + 8, end)
        .ok_or(ArtError::NotExactCover { offset: after_name + 8, end })?;
    let child_types: Vec<u32> = children.iter().map(|c| c.chunk_type).collect();
    let mut meshes = Vec::new();
    let mut bound_box = None;
    let mut bound_sphere = None;
    for c in &children {
        match c.chunk_type {
            CHUNK_TYPE_MESH => meshes.push(decode_mesh(buf, c.offset, c.size)?),
            CHUNK_TYPE_BOUND_BOX if c.payload_len() == 24 => bound_box = Some(f32n::<6>(buf, c.payload())?),
            CHUNK_TYPE_BOUND_SPHERE if c.payload_len() == 16 => bound_sphere = Some(f32n::<4>(buf, c.payload())?),
            // 0x10021 / 0x122000 / 0x10017 / 0x121201 / 0x121305 / legacy:
            // recorded in child_types, semantics UNKNOWN, passthrough.
            _ => {}
        }
    }
    Ok(ShapeChunk { name, version, flag, meshes, bound_box, bound_sphere, child_types })
}

// ---------------------------------------------------------------------------
// Decoded texture / material
// ---------------------------------------------------------------------------

/// One decoded Texture chunk (`0x00019000`) with its embedded DDS image.
#[derive(Debug, Clone)]
pub struct TextureChunk {
    /// Texture name (the exporter's `.dds` file name).
    pub name: String,
    /// The 9×u32 header: [0] = 14000 (validated), [1] width, [2] height,
    /// [3] = 8, [4] mip count, [5] = 1, [5..9] zeros (recorded).
    ///
    /// Block 29: [4] is PROVEN UNRELIABLE on real data — art's
    /// `TUT078__I1` declares 8 while its embedded DDS ships base-only
    /// (`dwMipMapCount` 0), and cells' `communicationsarray001_diffuse`
    /// declares 1 while its DDS carries the FULL 8-level chain.  The
    /// DDS header word (offset 28) is the ground truth; consumers must
    /// consult `dds` (see `prototype_render::dxt::parse_dds`), never
    /// this field, when they need mip facts.
    pub fields: [u32; 9],
    /// FourCC from the Image chunk (`DXT1` / `DXT3` / `DXT5`).
    pub fourcc: [u8; 4],
    /// The complete embedded DDS file (128-B header + block data).
    pub dds: Vec<u8>,
    /// Exporter source path (0x19003 child, when present).
    pub source_path: Option<String>,
}

/// Decode the Texture chunk starting at `offset` with total size `size`.
pub fn decode_texture(buf: &[u8], offset: usize, size: usize) -> Result<TextureChunk, ArtError> {
    let end = offset + size;
    if end > buf.len() {
        return Err(ArtError::Truncated { offset, need: size, got: buf.len() - offset });
    }
    let (name, after_name) = pascal_at(buf, offset + 12, end)?;
    if after_name + 36 > end {
        return Err(ArtError::Truncated { offset: after_name, need: 36, got: end - after_name });
    }
    let mut fields = [0u32; 9];
    for (i, f) in fields.iter_mut().enumerate() {
        *f = u32_at(buf, after_name + i * 4)?;
    }
    if fields[0] != TEXTURE_VERSION {
        return Err(ArtError::BadTextureVersion { offset: after_name, got: fields[0] });
    }
    let children = child_chunks(buf, after_name + 36, end)
        .ok_or(ArtError::NotExactCover { offset: after_name + 36, end })?;
    let image = children
        .iter()
        .find(|c| c.chunk_type == CHUNK_TYPE_IMAGE)
        .ok_or(ArtError::NotExactCover { offset: after_name + 36, end })?;
    // -- Image chunk: name + 6×u32 + fourcc + children --------------------
    let iend = image.offset + image.size;
    let (_iname, iafter) = pascal_at(buf, image.payload(), iend)?;
    if iafter + 28 > iend {
        return Err(ArtError::Truncated { offset: iafter, need: 28, got: iend - iafter });
    }
    let _img_fields: [u32; 6] = {
        let mut f = [0u32; 6];
        for (i, v) in f.iter_mut().enumerate() {
            *v = u32_at(buf, iafter + i * 4)?;
        }
        f
    };
    let mut fourcc = [0u8; 4];
    fourcc.copy_from_slice(&buf[iafter + 24..iafter + 28]);
    let ichildren = child_chunks(buf, iafter + 28, iend)
        .ok_or(ArtError::NotExactCover { offset: iafter + 28, end: iend })?;
    let mut dds = None;
    let mut source_path = None;
    for c in &ichildren {
        match c.chunk_type {
            CHUNK_TYPE_IMAGE_DATA => {
                let declared = u32_at(buf, c.payload())?;
                let avail = c.payload_len() - 4;
                if declared as usize != avail {
                    return Err(ArtError::BadImageData { offset: c.offset, declared, got: avail });
                }
                let start = c.payload() + 4;
                if buf[start..start + 4] != *b"DDS " {
                    return Err(ArtError::BadImageData { offset: start, declared, got: avail });
                }
                dds = Some(buf[start..start + avail].to_vec());
            }
            CHUNK_TYPE_IMAGE_SOURCE_PATH => {
                // NUL-padded ASCII exporter path; trim trailing NULs.
                let raw = &buf[c.payload()..c.payload() + c.payload_len()];
                let s = String::from_utf8_lossy(raw);
                source_path = Some(s.trim_end_matches('\0').to_string());
            }
            // 0x19001 observed 10× corpus-wide: passthrough.
            _ => {}
        }
    }
    Ok(TextureChunk {
        name,
        fields,
        fourcc,
        dds: dds.ok_or(ArtError::NotExactCover { offset: image.offset, end: iend })?,
        source_path,
    })
}

/// One Material param child (0x00023001/2/3).
///
/// PROVEN (block 26, corpus census over the real cell-82 geometry member
/// + the barrier art member): type `0x23001` is a **per-drawable STATE
/// record, NOT a shader uniform parameter block**.  Every observed
/// record (27/27) is bit-identical: 30 little-endian f32s followed by a
/// 2-byte `[0, 0]` tail, the skeleton
/// `[0,1,0,0, 0,0,1,0, 0,0,0,1, 0,0,0,0, 1,0,0,0, 1,1,1,0, 0,0,0,0,0,0]`
/// (−0.0 quirked) — an identity-like pose.  Names are DRAWABLE names,
/// never uniform names: `{drawable}`, `{drawable}Initial`,
/// `{drawable}Damaged` (the same state strings the GameObject binary
/// records carry), plus per-instance names in the cells member
/// (`billBoard001Ad001`, `bakery`, …).  The Material chunk that wraps
/// them is named after a SHAPE/drawable (e.g. `barrierPlasticOS002`,
/// `signHorizontal001`), not a shader.  Per-value semantics beyond the
/// state naming are UNKNOWN; since no observed value ever varies, there
/// are no material float params to feed shader uniforms in this corpus.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterialParam {
    /// Child chunk type (0x23001 observed on every record so far;
    /// 0x23002/3 were recorded from earlier probes).
    pub chunk_type: u32,
    /// Param block name — a drawable/state name (see the type doc).
    pub name: String,
    /// Payload after the name, read as little-endian f32s (floor(n/4)).
    /// PROVEN: always 30 on 0x23001 records, always the same skeleton.
    pub floats: Vec<f32>,
    /// Trailing bytes not consumed by whole f32s.  PROVEN: `[0, 0]`
    /// (2 bytes) on every observed 0x23001 record.
    pub tail: Vec<u8>,
}

/// One decoded Material chunk (`0x00023000`).
///
/// PROVEN naming (block 26): the material name is a SHAPE/drawable name
/// (art: `barrierPlasticOS002`; cells member: `billBoard001`,
/// `signHorizontal001`), and the children are that drawable's state
/// records (see [`MaterialParam`]) — NOT a shader-parameter material.
/// The mesh→material binding therefore cannot be by shader name; treat
/// materials as per-drawable state tables.
#[derive(Debug, Clone)]
pub struct MaterialChunk {
    /// Material name (a drawable/shape name — see the type doc).
    pub name: String,
    /// Header u32[0] — 1 on every observed member (recorded).
    pub field0: u32,
    /// Header u32[1] — the child count (3 on the barrier fixture, 7/2/15
    /// on the cells member's three materials).
    pub field1: u32,
    /// Param children in file order.
    pub params: Vec<MaterialParam>,
}

/// Decode the Material chunk starting at `offset` with total size `size`.
pub fn decode_material(buf: &[u8], offset: usize, size: usize) -> Result<MaterialChunk, ArtError> {
    let end = offset + size;
    if end > buf.len() {
        return Err(ArtError::Truncated { offset, need: size, got: buf.len() - offset });
    }
    let (name, after_name) = pascal_at(buf, offset + 12, end)?;
    if after_name + 8 > end {
        return Err(ArtError::Truncated { offset: after_name, need: 8, got: end - after_name });
    }
    let field0 = u32_at(buf, after_name)?;
    let field1 = u32_at(buf, after_name + 4)?;
    // Scan forward (4-aligned) for the first 0x000230xx child chunk, then
    // require an exact-cover run to the end.  The scan is bounded by the
    // chunk end so it always terminates.
    let mut scan = after_name + 8;
    let kids_start = loop {
        if scan + 12 > end {
            return Err(ArtError::NotExactCover { offset: after_name + 8, end });
        }
        let t = u32_at(buf, scan)?;
        if (t & 0xFFFF_FF00) == 0x0002_3000 && t != CHUNK_TYPE_MATERIAL {
            break scan;
        }
        scan += 4;
    };
    let children = child_chunks(buf, kids_start, end)
        .ok_or(ArtError::NotExactCover { offset: kids_start, end })?;
    let mut params = Vec::new();
    for c in &children {
        let (pname, pafter) = pascal_at(buf, c.payload(), c.offset + c.size)?;
        let rest = c.offset + c.size - pafter;
        let nfloats = rest / 4;
        let mut floats = Vec::with_capacity(nfloats);
        for i in 0..nfloats {
            floats.push(f32_at(buf, pafter + i * 4)?);
        }
        params.push(MaterialParam {
            chunk_type: c.chunk_type,
            name: pname,
            floats,
            tail: buf[pafter + nfloats * 4..c.offset + c.size].to_vec(),
        });
    }
    Ok(MaterialChunk { name, field0, field1, params })
}

/// One named param child of a pure3dShader chunk.
///
/// Corpus evidence: `0x11016` = pascal param name + pascal value (e.g.
/// `color` → `rifle001_diffuse.dds`); `0x11017` carries a pascal name +
/// what looks like f32 value(s) (`spec_intensity`) — kept raw until
/// proven; the other types are raw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShaderParam {
    /// Child chunk type (0x11016/17/18/19/1A/1C/20/21/22 observed).
    pub chunk_type: u32,
    /// Param name — decoded for 0x11016 only (its proven layout:
    /// pascal param name, NUL-padded, then a pascal value).  `None`
    /// for the types whose layout is UNKNOWN (raw `data` only).
    pub name: Option<String>,
    /// Decoded value string for 0x11016 (e.g. a texture name).
    pub value: Option<String>,
    /// Payload bytes after the name (raw, verbatim).
    pub data: Vec<u8>,
}

/// One decoded pure3dShader chunk (`0x00011015`).
///
/// The mesh→shader binding is BY NAME (block-16 proven); this decoder
/// extracts what the shader chunk says about itself: the engine shader
/// FAMILY (139 distinct corpus-wide: `env_car_tire`, `char_NIS`,
/// `1c1s1n`, …) and the named param children.  Block-33 RE resolved
/// what the family does engine-side (see
/// `prototype-render::shader::library`): families are FEATURE FLAGS —
/// the 30-entry registry reduces them to ten booleans (keys 5..14)
/// written into the shader instance at creation; shader PROGRAMS are
/// selected by CLASS (nine built-in name-keyed instances), never by
/// family.  The chunk loader class is `proto::ShaderConfigureFixup`
/// (vtable `0x10eafa30`).
///
/// Block-37 RE closed the remaining loader question: this chunk's
/// loader (`proto::ShaderConfigureFixup`, parse `0x10b96280` behind
/// vtable `0x10eafcb8` — the ctor pins chunk id 0x11015 at object+8)
/// **never factories an instance**.  It reads exactly what this decoder
/// reads — name pascal, u32 version, family pascal, then ~30 more
/// stream fields — interns the strings, and applies the family
/// configuration to an instance found BY NAME (state+0x2e8, gated by
/// the 30-family registry check `0x1054d960`).  Shader instances are
/// created only by the `0x00011009` loader (see
/// [`decode_shader_instance`]); this chunk is the per-name
/// configuration (a "fixup") that rides alongside.
#[derive(Debug, Clone)]
pub struct ShaderChunk {
    /// Chunk name — MOSTLY MD5-style (content-addressed variant id) but
    /// NOT always: readable names like `rifle001Shader1` also occur
    /// (observed corpus-wide).
    pub name: String,
    /// Engine shader family name (NUL padding trimmed).
    pub family: String,
    /// The nested self-reference pascal (32-hex when present; the scan
    /// is what the params walk keys off).
    pub self_name: Option<String>,
    /// Param children in file order (best-effort; raw for unknown types).
    pub params: Vec<ShaderParam>,
    /// True when the param area parsed as an exact-cover chunk run.
    pub params_exact: bool,
}

/// Decode the pure3dShader chunk starting at `offset` with size `size`.
pub fn decode_shader_chunk(buf: &[u8], offset: usize, size: usize) -> Result<ShaderChunk, ArtError> {
    let end = offset + size;
    if end > buf.len() {
        return Err(ArtError::Truncated { offset, need: size, got: buf.len() - offset });
    }
    let (name, after_name) = pascal_at(buf, offset + 12, end)?;
    if after_name + 4 > end {
        return Err(ArtError::Truncated { offset: after_name, need: 4, got: end - after_name });
    }
    let version = u32_at(buf, after_name)?;
    if version != SHADER_VERSION {
        return Err(ArtError::UnexpectedVersion { offset: after_name, got: version, want: SHADER_VERSION });
    }
    let (family_raw, after_family) = pascal_at(buf, after_name + 4, end)?;
    let family = family_raw.trim_end_matches('\u{0}').to_string();

    // Scan forward for the 32-hex self-reference pascal (the corpus
    // layout between family and self-name is a small fixed-ish run whose
    // exact fields are UNKNOWN — the scan is bounded and byte-aligned).
    let mut self_name = None;
    let mut scan = after_family;
    let limit = end.min(after_family + 128);
    while scan < limit {
        let n = buf[scan] as usize;
        if n == 32 && scan + 1 + 32 <= end {
            let cand = &buf[scan + 1..scan + 1 + 32];
            if cand.iter().all(|c| c.is_ascii_hexdigit()) {
                self_name = Some(String::from_utf8_lossy(cand).into_owned());
                scan += 1 + 32;
                break;
            }
        }
        scan += 1;
    }

    // Params: exact-cover chunk run from the self-name end (when found)
    // to the chunk end.  Fail-soft: a non-exact area stays raw.
    let mut params = Vec::new();
    let mut params_exact = false;
    if let Some(_) = &self_name {
        if let Some(children) = child_chunks(buf, scan, end) {
            params_exact = true;
            for c in &children {
                let (pname, value, data_off) = if c.chunk_type == 0x0001_1016 {
                    // proven 0x11016 layout: pascal name (NUL-padded),
                    // pascal value (e.g. a texture name)
                    let (n, a) = pascal_at(buf, c.payload(), c.offset + c.size)?;
                    match pascal_at(buf, a, c.offset + c.size) {
                        Ok((v, v_end)) => (
                            Some(n.trim_end_matches('\u{0}').to_string()),
                            Some(v.trim_end_matches('\u{0}').to_string()),
                            v_end,
                        ),
                        Err(_) => (Some(n.trim_end_matches('\u{0}').to_string()), None, a),
                    }
                } else {
                    (None, None, c.payload())
                };
                params.push(ShaderParam {
                    chunk_type: c.chunk_type,
                    name: pname,
                    value,
                    data: buf[data_off..c.offset + c.size].to_vec(),
                });
            }
        }
    }
    Ok(ShaderChunk { name, family, self_name, params, params_exact })
}

/// One decoded shader-INSTANCE chunk (`0x00011009`, block-37).
///
/// This is the chunk whose loader creates shader instances: its parse
/// (`0x10b95780`, loader vtable `0x10eafc50`) reads the FIRST pascal
/// string into a frame buffer and passes that buffer as the NAME
/// argument of the instance factory `0x1061bcd0` (call site
/// `0x10b95948`; `this` = the manager global `0x1127f358`).  Buffer
/// identity proven by symbolic-ESP tracking (the same frame slot is
/// lea'd at `0x10b957b2` for the stream read and at `0x10b95940` for
/// the factory argument).  After the factory call the parse walks
/// child chunks (ids 0x1100A / 0x1100E; the 0x1100E arm reads a
/// second pascal string).
///
/// Field semantics after the name are NOT decoded (the engine reads
/// them as raw u32s into a scratch buffer whose meaning is UNKNOWN);
/// the decoder captures them as raw bytes, fail-soft.
#[derive(Debug, Clone)]
pub struct ShaderInstanceChunk {
    /// The instance NAME — the exact string the engine hands to the
    /// factory (interned into the instance's name field at +0x2e8).
    /// Readable names in the frontend dialect (`CxformGauraud`,
    /// `hud_2dmarker`); hash-style names occur in world data.
    /// Trailing NUL padding is trimmed (the engine NUL-terminates the
    /// frame buffer and interns it as a C string, so the effective
    /// name stops at the first NUL).
    pub name: String,
    /// Chunk version (u32 read right after the name, as the engine
    /// does; semantics otherwise UNKNOWN).
    pub version: u32,
    /// Raw bytes after the version u32 (fail-soft; not decoded).
    pub tail: Vec<u8>,
}

/// Decode the shader-instance chunk starting at `offset` with size
/// `size` (the proven engine read order: pascal name, u32, raw tail).
pub fn decode_shader_instance(buf: &[u8], offset: usize, size: usize) -> Result<ShaderInstanceChunk, ArtError> {
    let end = offset + size;
    if end > buf.len() {
        return Err(ArtError::Truncated { offset, need: size, got: buf.len() - offset });
    }
    let (name_raw, after_name) = pascal_at(buf, offset + 12, end)?;
    let name = name_raw.trim_end_matches('\u{0}').to_string();
    if after_name + 4 > end {
        return Err(ArtError::Truncated { offset: after_name, need: 4, got: end - after_name });
    }
    let version = u32_at(buf, after_name)?;
    let tail = buf[after_name + 4..end].to_vec();
    Ok(ShaderInstanceChunk { name, version, tail })
}

/// Top-level chunk run of a P3D file (skips the 12-byte file header).
/// Returns `None` when the top level does not exactly cover the file —
/// proven to never happen on real Prototype files.
pub fn top_level_chunks(buf: &[u8]) -> Option<Vec<ChildChunk>> {
    if buf.len() < 12 || &buf[..4] != b"P3D\xff" {
        return None;
    }
    child_chunks(buf, 12, buf.len())
}

// ---------------------------------------------------------------------------
// Census helpers (kept from the pre-art groundwork, now evidence-backed)
// ---------------------------------------------------------------------------

/// A `(chunk_type, count)` census entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TypeCount {
    /// Raw chunk type id.
    pub chunk_type: u32,
    /// How many times it appeared.
    pub count: u32,
    /// Total payload bytes across all instances.
    pub payload_bytes: u64,
}

/// Census of one P3D buffer (see [`type_histogram`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypeHistogram {
    /// All observed chunk types, sorted by count then id.
    pub types: Vec<TypeCount>,
    /// Chunks whose prelude was malformed.
    pub malformed: u32,
    /// True when the input carried the `P3D\xff` file header.
    pub had_file_header: bool,
}

const PRELUDE: usize = 12;

/// Census of every chunk in the tree.  Container probing: a chunk's
/// payload is also walked as a chunk list when it parses as a
/// self-consistent list ending exactly at the chunk end with ≥1 child.
pub fn type_histogram(data: &[u8]) -> TypeHistogram {
    let mut h = TypeHistogram::default();
    const P3D_MAGIC_LE: [u8; 4] = [0x50, 0x33, 0x44, 0xFF];
    let start = if data.len() >= 12 && data[..4] == P3D_MAGIC_LE {
        h.had_file_header = true;
        12
    } else {
        0
    };
    let mut counts: std::collections::BTreeMap<u32, (u32, u64)> = Default::default();
    walk(data, start, data.len(), 0, &mut counts, &mut h.malformed);
    h.types = counts
        .into_iter()
        .map(|(chunk_type, (count, payload_bytes))| TypeCount { chunk_type, count, payload_bytes })
        .collect();
    h.types.sort_by_key(|t| (std::cmp::Reverse(t.count), t.chunk_type));
    h
}

fn walk(
    data: &[u8],
    offset: usize,
    end: usize,
    depth: usize,
    counts: &mut std::collections::BTreeMap<u32, (u32, u64)>,
    malformed: &mut u32,
) {
    if depth > 8 {
        return; // containment probe depth guard
    }
    let mut off = offset;
    while off + PRELUDE <= end {
        let t = u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
        let size = u32::from_le_bytes([data[off + 8], data[off + 9], data[off + 10], data[off + 11]])
            as usize;
        if size < PRELUDE || off + size > end {
            *malformed += 1;
            return;
        }
        let e = counts.entry(t).or_insert((0, 0));
        e.0 += 1;
        e.1 += (size - PRELUDE) as u64;
        let payload_start = off + PRELUDE;
        let payload_end = off + size;
        let mut probe_ok = payload_end - payload_start >= PRELUDE;
        if probe_ok {
            let mut p = payload_start;
            let mut children = 0u32;
            while p + PRELUDE <= payload_end {
                let cs = u32::from_le_bytes([
                    data[p + 8],
                    data[p + 9],
                    data[p + 10],
                    data[p + 11],
                ]) as usize;
                if cs < PRELUDE || p + cs > payload_end {
                    probe_ok = false;
                    break;
                }
                p += cs;
                children += 1;
            }
            if p != payload_end || children == 0 {
                probe_ok = false;
            }
        }
        if probe_ok {
            walk(data, payload_start, payload_end, depth + 1, counts, malformed);
        }
        off += size;
    }
}

/// Best-effort human name for a chunk type id — only corpus-observed ids
/// get names; unknown ids stay unnamed (never guessed).
pub fn classify(chunk_type: u32) -> Option<&'static str> {
    match chunk_type {
        CHUNK_TYPE_SHAPE => Some("Shape"),
        CHUNK_TYPE_MESH => Some("Mesh"),
        CHUNK_TYPE_VERTEX_BUFFER => Some("VertexBuffer"),
        CHUNK_TYPE_INDEX_BUFFER => Some("IndexBuffer"),
        CHUNK_TYPE_PRIMITIVE_GROUP => Some("PrimitiveGroup"),
        CHUNK_TYPE_BOUND_BOX => Some("BoundBox"),
        CHUNK_TYPE_BOUND_SPHERE => Some("BoundSphere"),
        CHUNK_TYPE_SHAPE_INFO => Some("ShapeInfo"),
        CHUNK_TYPE_TEXTURE => Some("Texture"),
        CHUNK_TYPE_IMAGE => Some("Image"),
        CHUNK_TYPE_IMAGE_DATA => Some("ImageData"),
        CHUNK_TYPE_IMAGE_SOURCE_PATH => Some("ImageSourcePath"),
        CHUNK_TYPE_MATERIAL => Some("Material"),
        CHUNK_TYPE_SHADER => Some("pure3dShader"),
        CHUNK_TYPE_SHADER_INSTANCE => Some("ShaderInstance"),
        _ => None,
    }
}

/// Format the census as the evidence table used in PROGRESS.md updates.
pub fn histogram_table(name: &str, h: &TypeHistogram) -> String {
    let mut s = format!("{name}: {} type(s), {} malformed\n", h.types.len(), h.malformed);
    for t in &h.types {
        let label = classify(t.chunk_type).unwrap_or("?");
        s.push_str(&format!(
            "  0x{:08X}  x{:<4} {:>9} payload bytes  {}\n",
            t.chunk_type, t.count, t.payload_bytes, label
        ));
    }
    s
}

// ---------------------------------------------------------------------------
// Tests — REAL fixtures committed from the user's own art.rcf
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

    fn first_mesh(buf: &[u8]) -> MeshChunk {
        let top = top_level_chunks(buf).expect("exact cover");
        let shape = top
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_SHAPE)
            .map(|c| decode_shape(buf, c.offset, c.size).unwrap())
            .expect("shape present");
        shape.meshes[0].clone()
    }

    // ------------------------------------------------------------------
    // Block 27 — PrimitiveGroup 0x00010014: the mesh's own vertex decl
    // ------------------------------------------------------------------

    /// The quad section fixture (desc 0xA032, stride 48): 4 declared
    /// elements + the 5-B final position block; the decl's UV/normal
    /// offsets agree with the measured layout table.
    #[test]
    fn primitive_group_quad_declaration() {
        let buf = fixture("cells_section_quad.p3d");
        let mesh = first_mesh(&buf);
        assert_eq!(mesh.layout.stride, 48);
        let pg = mesh.primitive_group.as_ref().expect("quad carries a decl");
        assert_eq!(pg.vertex_bytes, 4 * 48);
        assert_eq!(pg.code & 0xF, 5);
        assert_eq!(pg.descriptor_hash, PRIMITIVE_GROUP_DESCRIPTOR_HASH);
        let expect: [(u32, u8, AttrSemantic); 4] = [
            (0x04_fc98, 16, AttrSemantic::Unknown(0x04_fc98)),
            (0x09_4536, 20, AttrSemantic::Uv(0)),
            (0x0a_4536, 28, AttrSemantic::Uv(1)),
            (0xe7_bc06, 36, AttrSemantic::Normal),
        ];
        for (e, (attr, off, sem)) in pg.elements.iter().zip(expect) {
            assert_eq!(e.attr, attr);
            assert_eq!(e.offset, off);
            assert_eq!(e.semantic, sem);
            assert_eq!(e.stride as usize, 48);
        }
        // field_y recorded verbatim (0x738 / 0 / 0 / 0xc2 observed).
        assert_eq!(pg.elements[0].field_y, 0x738);
        assert_eq!(pg.elements[1].field_y, 0);
        assert_eq!(pg.elements[2].field_y, 0);
        assert_eq!(pg.elements[3].field_y, 0xc2);
        assert_eq!(pg.final_element, FinalElement { stride: 48, n: 3, flag: 1 });
    }

    /// The 0xB033 world mesh: the decl declares THREE UV sets (20/28/36)
    /// where the measured table only found one — the decl corrects the
    /// table, and the normal/tangent offsets agree (44/56).
    #[test]
    fn primitive_group_b033_three_uv_sets() {
        let buf = fixture("cells_member_geometry_small.p3d");
        let top = top_level_chunks(&buf).expect("exact cover");
        let mut b033 = None;
        for c in top.iter().filter(|c| c.chunk_type == CHUNK_TYPE_SHAPE) {
            let s = decode_shape(&buf, c.offset, c.size).unwrap();
            for m in &s.meshes {
                if m.header.desc == 0xB033 {
                    b033 = Some(m.clone());
                }
            }
        }
        let mesh = b033.expect("a 0xB033 mesh in the fixture");
        let pg = mesh.primitive_group.as_ref().expect("decl present");
        assert_eq!(pg.vertex_bytes, 62 * 72);
        assert_eq!(pg.code & 0xF, 7);
        let sems: Vec<AttrSemantic> = pg.elements.iter().map(|e| e.semantic).collect();
        assert_eq!(
            sems,
            vec![
                AttrSemantic::Unknown(0x04_fc98),
                AttrSemantic::Uv(0),
                AttrSemantic::Uv(1),
                AttrSemantic::Uv(2),
                AttrSemantic::Normal,
                AttrSemantic::Tangent,
            ]
        );
        let offs: Vec<u8> = pg.elements.iter().map(|e| e.offset).collect();
        assert_eq!(offs, vec![16, 20, 28, 36, 44, 56]);
        // Normal + tangent offsets agree with the measured table.
        assert_eq!(mesh.layout.normal_offset, Some(44));
        assert_eq!(mesh.layout.tangent_offset, Some(56));
        assert_eq!(pg.final_element, FinalElement { stride: 72, n: 4, flag: 1 });
        // The decl-corrected uv extraction reads uv0@20: the first
        // vertices carry the exact f32 2.5 (bit pattern 0x40200000) —
        // world geometry uses tiled UVs outside [0, 1].
        assert_eq!(mesh.uvs.len(), 62);
        assert_eq!(mesh.uvs[0][0].to_bits(), 0x4020_0000);
        assert_eq!(mesh.uvs[1][0].to_bits(), 0x4020_0000);
    }

    /// The 0x6000_2011 fixture: the trailing-block pair is declared as
    /// TWO elements (Extra4 @36 = stride−28, Extra3 @52 = stride−12),
    /// final flag byte 0.  (Block 28 renamed the old `Skin` guess — the
    /// data disproved weights/indices.)
    #[test]
    fn primitive_group_skinned_skin_elements() {
        let buf = fixture("art_mesh_skin6000.p3d");
        let mesh = first_mesh(&buf);
        let pg = mesh.primitive_group.as_ref().expect("decl present");
        assert_eq!(pg.vertex_bytes, 8 * 64);
        let sems: Vec<AttrSemantic> = pg.elements.iter().map(|e| e.semantic).collect();
        assert_eq!(
            sems,
            vec![
                AttrSemantic::Uv(0),
                AttrSemantic::Normal,
                AttrSemantic::Extra4,
                AttrSemantic::Extra3,
            ]
        );
        let offs: Vec<u8> = pg.elements.iter().map(|e| e.offset).collect();
        assert_eq!(offs, vec![16, 24, 36, 52]);
        assert_eq!(pg.final_element, FinalElement { stride: 64, n: 3, flag: 0 });
    }

    /// Block 28: the trailing-block pair is DECODED on the real skinned
    /// fixture.  Pinned facts: extra4[i][3] == 0 on all 8 verts; the
    /// extra3 vec3 is CONSTANT across all 8 verts (a per-mesh value, not
    /// per-vertex); the decl offsets (36/52) equal the stride−28/−12
    /// positions; the old Skin interpretation is impossible (negative
    /// floats, no [0,1] weights, no small indices).
    #[test]
    fn trailing_block_pair_decoded_on_skinned_fixture() {
        let buf = fixture("art_mesh_skin6000.p3d");
        let mesh = first_mesh(&buf);
        assert_eq!(mesh.layout.skin_bytes, 28);
        assert_eq!(mesh.extra4.len(), 8);
        assert_eq!(mesh.extra3.len(), 8);
        // extra4 xyz per vertex — pinned by BIT PATTERN from the committed
        // fixture (v4..v7 repeat v0/v2/v1/v3 with flipped normals):
        const A0: u32 = 0xBCFB_7E91; // v0/v4 x
        const A1: u32 = 0xBE62_B6AE; // v0/v2/v4/v5 y
        const A2: u32 = 0xBDF6_FD22; // v1/v6 x
        const A3: u32 = 0xBE12_1FF3; // v2/v5 x
        const A4: u32 = 0xBCBD_3C36; // v3/v7 x
        let want4: [[u32; 4]; 8] = [
            [A0, A1, 0, 0],
            [A2, 0, 0, 0],
            [A3, A1, 0x8000_0000, 0], // −0.0 third component on v2/v5
            [A4, 0, 0, 0],
            [A0, A1, 0, 0],
            [A3, A1, 0x8000_0000, 0],
            [A2, 0, 0, 0],
            [A4, 0, 0, 0],
        ];
        for (i, (got, want)) in mesh.extra4.iter().zip(want4).enumerate() {
            for c in 0..4 {
                assert_eq!(got[c].to_bits(), want[c], "extra4[{i}][{c}]");
            }
        }
        // extra3: one constant vec3 across all 8 verts.
        let b0 = mesh.extra3[0];
        assert_eq!(b0[0].to_bits(), 0xBDBE_76C9); // −0.092_969_4
        assert_eq!(b0[1].to_bits(), 0x3E41_54CA); // 0.188_754_2
        assert_eq!(b0[2].to_bits(), 0x3DFE_C56D); // 0.124_379_4
        assert!(mesh.extra3.iter().all(|v| v == &b0));
        // NOT weights (negative / zero-sum facts), NOT indices (no small
        // integers in any component).
        assert!(mesh.extra4.iter().any(|v| v[0] < 0.0));
    }

    /// Block 28: real 0xB032 meshes with decls (skyLight001InitialShape
    /// member bcaf0e88).  The decl PROVES uv0@20 + uv1@28 — the old
    /// measured `uv@24` row is corrected to &[20, 28]; the 0x4fc98 slot
    /// sits at 16; the tangent spans 16 B (48..64).  Also pins the
    /// second 0xB032 mesh and the decl-first uv agreement.
    #[test]
    fn b032_decl_corrected_layout() {
        let buf = fixture("art_mesh_b032.p3d");
        let top = top_level_chunks(&buf).expect("exact cover");
        let shape = top
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_SHAPE)
            .map(|c| decode_shape(&buf, c.offset, c.size).unwrap())
            .expect("shape present");
        let b032: Vec<_> = shape
            .meshes
            .iter()
            .filter(|m| m.header.desc == 0xB032)
            .collect();
        assert_eq!(b032.len(), 2, "two 0xB032 meshes");
        for m in &b032 {
            assert_eq!(m.layout.uv_offsets, &[20, 28]);
            let pg = m.primitive_group.as_ref().expect("decl present");
            let sems: Vec<AttrSemantic> = pg.elements.iter().map(|e| e.semantic).collect();
            assert_eq!(
                sems,
                vec![
                    AttrSemantic::Unknown(0x04_fc98),
                    AttrSemantic::Uv(0),
                    AttrSemantic::Uv(1),
                    AttrSemantic::Normal,
                    AttrSemantic::Tangent,
                ]
            );
            let offs: Vec<u8> = pg.elements.iter().map(|e| e.offset).collect();
            assert_eq!(offs, vec![16, 20, 28, 36, 48]);
            assert_eq!(m.layout.normal_offset, Some(36));
            assert_eq!(m.layout.tangent_offset, Some(48));
            assert_eq!(pg.final_element, FinalElement { stride: 64, n: 4, flag: 1 });
            // uv1 extracted at the decl/table-agreed offset 28 and EQUAL
            // to uv0 on these meshes (the lens material duplicates the
            // map onto both channels).
            assert_eq!(m.uvs1.len(), m.uvs.len());
            assert!(m.uvs1.iter().zip(m.uvs.iter()).all(|(a, b)| a == b));
            // color slot: constant 0x7f7f7f4c on every vertex.
            let cb = m.color_bytes.as_ref().expect("color slot present");
            assert_eq!(cb.len(), m.header.vertex_count as usize);
            assert!(cb.iter().all(|c| c == &[0x7f, 0x7f, 0x7f, 0x4c]));
            // no trailing blocks on a non-0x6000 desc.
            assert!(m.extra4.is_empty() && m.extra3.is_empty());
        }
        // The 130-vert and 38-vert meshes: bit-exact uv0 values pinned
        // (tiled lens atlas coordinates, x up to 1.5).
        let m38 = b032.iter().find(|m| m.header.vertex_count == 38).unwrap();
        assert_eq!(m38.uvs[0][0].to_bits(), 0x3FC0_0000); // exactly 1.5
        assert_eq!(m38.uvs[0], [1.5, 0.0]);
    }

    /// Block 28: a real 0xA021 decl (the third mesh of the same fixture)
    /// — previously unsampled desc — with its final-block flag 0 on a
    /// STATIC mesh (the old "flag 1 = static" rule is dead) and its
    /// color slot carrying two distinct values (ffffff00 /ffffffff).
    #[test]
    fn a021_decl_sample_and_flag_counterexample() {
        let buf = fixture("art_mesh_b032.p3d");
        let top = top_level_chunks(&buf).expect("exact cover");
        let shape = top
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_SHAPE)
            .map(|c| decode_shape(&buf, c.offset, c.size).unwrap())
            .expect("shape present");
        let m = shape
            .meshes
            .iter()
            .find(|m| m.header.desc == 0xA021)
            .expect("the 0xA021 mesh");
        assert_eq!(m.header.vertex_count, 32);
        assert_eq!(m.layout.uv_offsets, &[20]);
        assert_eq!(m.layout.color_offset, Some(16));
        let pg = m.primitive_group.as_ref().expect("decl present");
        let sems: Vec<AttrSemantic> = pg.elements.iter().map(|e| e.semantic).collect();
        assert_eq!(
            sems,
            vec![AttrSemantic::Unknown(0x04_fc98), AttrSemantic::Uv(0)]
        );
        let offs: Vec<u8> = pg.elements.iter().map(|e| e.offset).collect();
        assert_eq!(offs, vec![16, 20]);
        assert_eq!(pg.final_element, FinalElement { stride: 28, n: 2, flag: 0 });
        let cb = m.color_bytes.as_ref().expect("color slot present");
        let whites_0 = cb.iter().filter(|c| c == &&[0xff, 0xff, 0xff, 0x00]).count();
        let whites_f = cb.iter().filter(|c| c == &&[0xff, 0xff, 0xff, 0xff]).count();
        assert_eq!(whites_0, 24);
        assert_eq!(whites_f, 8);
        assert_eq!(m.uvs.len(), 32);
        assert!(m
            .uvs
            .iter()
            .all(|uv| (0.25..=0.50).contains(&uv[0]) && (0.62..=0.72).contains(&uv[1])));
    }

    /// The real barrier model (desc 0x2011, 576+640 verts): decl offsets
    /// (uv@16, normal@24) match the measured table, vertex_bytes matches
    /// on BOTH shapes, and the final block is n=3/flag=1.
    #[test]
    fn primitive_group_barrier_matches_measured_table() {
        let buf = fixture("art_barrier_os002.p3d");
        let top = top_level_chunks(&buf).expect("exact cover");
        let shapes: Vec<_> = top
            .iter()
            .filter(|c| c.chunk_type == CHUNK_TYPE_SHAPE)
            .map(|c| decode_shape(&buf, c.offset, c.size).unwrap())
            .collect();
        assert_eq!(shapes.len(), 2);
        for s in &shapes {
            let m = &s.meshes[0];
            let pg = m.primitive_group.as_ref().expect("decl present");
            assert_eq!(
                pg.vertex_bytes as usize,
                m.header.vertex_count as usize * 36
            );
            let sems: Vec<AttrSemantic> = pg.elements.iter().map(|e| e.semantic).collect();
            assert_eq!(sems, vec![AttrSemantic::Uv(0), AttrSemantic::Normal]);
            let offs: Vec<u8> = pg.elements.iter().map(|e| e.offset).collect();
            assert_eq!(offs, vec![16, 24]);
            assert_eq!(m.layout.uv_offsets, &[16]);
            assert_eq!(m.layout.normal_offset, Some(24));
            assert_eq!(pg.final_element, FinalElement { stride: 36, n: 3, flag: 1 });
        }
    }

    /// A corrupted decl (descriptor hash flipped) fails closed with
    /// `BadPrimitiveGroup`, and a mesh without a decl child still decodes
    /// (primitive_group = None is legal).
    #[test]
    fn primitive_group_fail_closed() {
        let mut buf = fixture("cells_section_quad.p3d");
        // Locate the decl payload by walking the proven path.
        let top = top_level_chunks(&buf).unwrap();
        let shape = top
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_SHAPE)
            .unwrap();
        let (_, after_name) = pascal_at(&buf, shape.offset + 12, shape.offset + shape.size)
            .unwrap();
        let kids = child_chunks(&buf, after_name + 8, shape.offset + shape.size).unwrap();
        let mkid = kids.iter().find(|c| c.chunk_type == CHUNK_TYPE_MESH).unwrap();
        let (_, mafter) = pascal_at(&buf, mkid.offset + 16, mkid.offset + mkid.size).unwrap();
        let mkids = child_chunks(&buf, mafter + 36, mkid.offset + mkid.size).unwrap();
        let pg = mkids
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_PRIMITIVE_GROUP)
            .unwrap();
        // Flip one byte of the descriptor hash word (payload +16).
        let h = pg.offset + 12 + 16;
        buf[h] ^= 0xFF;
        let err = decode_primitive_group(&buf, pg.offset, pg.size).unwrap_err();
        assert!(matches!(err, ArtError::BadPrimitiveGroup { .. }), "got {err}");
    }

    /// The barrier fixture: two 0x2011 meshes (36-B stride) with PROVEN
    /// vertex values measured by scripts/m21b_probe.py + m21b_verify.py.
    #[test]
    fn barrier_shapes_decode_with_proven_values() {
        let buf = fixture("art_barrier_os002.p3d");
        let top = top_level_chunks(&buf).expect("exact top-level cover");
        let shapes: Vec<_> = top
            .iter()
            .filter(|c| c.chunk_type == CHUNK_TYPE_SHAPE)
            .map(|c| decode_shape(&buf, c.offset, c.size).unwrap())
            .collect();
        assert_eq!(shapes.len(), 2);
        assert_eq!(shapes[0].name, "barrierPlasticOS002DamagedShape\u{0}");
        assert_eq!(shapes[0].version, 0);

        let damaged = &shapes[0].meshes[0];
        assert_eq!(damaged.header.desc, 0x2011);
        assert_eq!(damaged.header.vertex_count, 576);
        assert_eq!(damaged.header.index_count, 1074);
        assert_eq!(damaged.header.tail, [0, 1, 1, 0, 0]);
        assert_eq!(damaged.layout.stride, 36);
        assert_eq!(damaged.positions.len(), 576);
        assert_eq!(damaged.indices.len(), 1074);
        // Position of vertex 0 (measured byte-exact by m21b_probe.py):
        assert!((damaged.positions[0][0] - -0.951_343_24).abs() < 1e-6);
        assert!((damaged.positions[0][1] - 0.013_813).abs() < 1e-6);
        assert!((damaged.positions[0][2] - -0.358_19).abs() < 1e-6);
        // UV of vertex 0 (exact f32 bit patterns from the fixture bytes).
        assert_eq!(damaged.uvs[0][0].to_bits(), 0x3F7D_EC7E);
        assert_eq!(damaged.uvs[0][1].to_bits(), 0x3F80_03A2);
        // Normal of vertex 0 (unit, measured).
        assert!((damaged.normals[0][0] - -0.749_71).abs() < 1e-5);
        assert!((damaged.normals[0][1] - 0.660_27).abs() < 1e-5);
        assert!((damaged.normals[0][2] - -0.044_443).abs() < 1e-5);
        // Normals unit-or-zero (corpus-proven; 16 degenerate verts of this
        // shape carry a 0,0,0 normal — exporter artifact, kept as data).
        let mut zero = 0;
        for n in &damaged.normals {
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if l == 0.0 {
                zero += 1;
            } else {
                assert!((l - 1.0).abs() < 1e-3, "non-unit normal {n:?}");
            }
        }
        assert_eq!(zero, 16, "exactly the 16 degenerate verts, pinned");
        // The Initial shape's normals are 100% unit (640/640 measured).
        let initial = &shapes[1].meshes[0];
        for n in &initial.normals {
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!((l - 1.0).abs() < 1e-3, "non-unit normal {n:?}");
        }
        // BBox equals the chunk's BoundBox (data min/max == bbox measured).
        let bb = damaged.bound_box.unwrap();
        assert!((bb[0] - -0.951_343_24).abs() < 1e-6);
        assert!((bb[3] - 1.042_568).abs() < 1e-6);
        // Indices in range.
        assert!(damaged.indices.iter().max().unwrap() < &576);

        let initial = &shapes[1].meshes[0];
        assert_eq!(initial.header.vertex_count, 640);
        assert_eq!(initial.header.index_count, 900);
        assert_eq!(initial.name, "131185ef20d24489daf1165e65814182");
        // The mesh name binds to the pure3dShader chunk name (block-15
        // census: shader 0x00011015 named '131185ef…' in the same file).
        assert!(initial.child_types.contains(&CHUNK_TYPE_PRIMITIVE_GROUP));
    }

    /// The 0xB031 fixture: 56-B stride, normal + tangent, UV at 20.
    #[test]
    fn b031_fixture_decodes() {
        let buf = fixture("art_mesh_b031.p3d");
        let top = top_level_chunks(&buf).expect("exact cover");
        let shape = top
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_SHAPE)
            .map(|c| decode_shape(&buf, c.offset, c.size).unwrap())
            .expect("shape present");
        let mesh = &shape.meshes[0];
        assert_eq!(mesh.header.desc, 0xB031);
        assert_eq!(mesh.layout.stride, 56);
        assert_eq!(mesh.positions.len(), mesh.header.vertex_count as usize);
        assert_eq!(mesh.tangents.len(), mesh.header.vertex_count as usize);
        for n in mesh.normals.iter().chain(mesh.tangents.iter()) {
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!((l - 1.0).abs() < 2e-3, "non-unit vector {n:?}");
        }
        assert!((mesh.indices.iter().max().copied().unwrap() as u32) < mesh.header.vertex_count);
    }

    /// The skinned 0x60002011 fixture: 64-B stride = 36-B base + 28-B skin
    /// at the END; base normal/uv offsets unchanged (proven corpus-wide).
    #[test]
    fn skinned_fixture_decodes() {
        let buf = fixture("art_mesh_skin6000.p3d");
        let top = top_level_chunks(&buf).expect("exact cover");
        let shape = top
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_SHAPE)
            .map(|c| decode_shape(&buf, c.offset, c.size).unwrap())
            .expect("shape present");
        let mesh = &shape.meshes[0];
        assert_eq!(mesh.header.desc, 0x6000_2011);
        assert_eq!(mesh.layout.stride, 64);
        assert_eq!(mesh.layout.skin_bytes, 28);
        assert_eq!(mesh.layout.normal_offset, Some(24), "skin block must not shift the base layout");
        for n in &mesh.normals {
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!((l - 1.0).abs() < 2e-3, "non-unit normal {n:?}");
        }
    }

    /// The 0xA031 fixture: 40-B stride, normal at 28, UV at 20.
    #[test]
    fn a031_fixture_decodes() {
        let buf = fixture("art_mesh_a031.p3d");
        let top = top_level_chunks(&buf).expect("exact cover");
        let shape = top
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_SHAPE)
            .map(|c| decode_shape(&buf, c.offset, c.size).unwrap())
            .expect("shape present");
        let mesh = &shape.meshes[0];
        assert_eq!(mesh.header.desc, 0xA031);
        assert_eq!(mesh.layout.stride, 40);
        assert_eq!(mesh.layout.normal_offset, Some(28));
        assert_eq!(mesh.layout.uv_offsets, &[20]);
        assert!(mesh.uvs.iter().all(|uv| (-0.3..=1.8).contains(&uv[0]) && (-0.3..=1.8).contains(&uv[1])));
    }

    /// The 0x2010 fixture: 28-B stride, normal at 16, NO UVs.
    #[test]
    fn no_uv_fixture_decodes() {
        let buf = fixture("art_mesh_nouvmesh2010.p3d");
        let top = top_level_chunks(&buf).expect("exact cover");
        let shape = top
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_SHAPE)
            .map(|c| decode_shape(&buf, c.offset, c.size).unwrap())
            .expect("shape present");
        let mesh = &shape.meshes[0];
        assert_eq!(mesh.header.desc, 0x2010);
        assert_eq!(mesh.layout.stride, 28);
        assert!(mesh.layout.uv_offsets.is_empty());
        assert!(mesh.uvs.is_empty(), "0x2010 has no UV set");
        for n in &mesh.normals {
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!((l - 1.0).abs() < 2e-3, "non-unit normal {n:?}");
        }
    }

    /// Textures: version 14000, dims, fourcc, embedded-DDS byte lengths.
    #[test]
    fn barrier_and_tut078_textures_decode() {
        let barrier = fixture("art_barrier_os002.p3d");
        let top = top_level_chunks(&barrier).unwrap();
        let tex = top
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_TEXTURE)
            .map(|c| decode_texture(&barrier, c.offset, c.size).unwrap())
            .unwrap();
        assert_eq!(tex.name, "barrierplasticos002_diffuse.dds\u{0}");
        assert_eq!(tex.fields[0], 14_000);
        assert_eq!((tex.fields[1], tex.fields[2]), (256, 256));
        assert_eq!(tex.fields[4], 1, "mip count");
        assert_eq!(&tex.fourcc, b"DXT1");
        assert_eq!(tex.dds.len(), 32_896, "128-B DDS header + 32768 B DXT1");
        assert_eq!(&tex.dds[..4], b"DDS ");
        assert!(tex.source_path.is_some());

        let tut = fixture("art_texture_tut078.p3d");
        let top = top_level_chunks(&tut).unwrap();
        let tex = top
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_TEXTURE)
            .map(|c| decode_texture(&tut, c.offset, c.size).unwrap())
            .unwrap();
        assert_eq!((tex.fields[1], tex.fields[2]), (128, 128));
        assert_eq!(tex.fields[4], 8, "TUT078 declares 8 mips");
        assert_eq!(&tex.fourcc, b"DXT5");
        assert_eq!(tex.dds.len(), 16_512);
    }

    /// Block 24: the textures EMBEDDED in cells.rcf geometry members use
    /// the SAME 0x19000 layout as art.rcf — the existing decoder
    /// exact-covers them.  This closes the block-23 OPEN item: the
    /// "chain of sub-records with a ~260 B residual" was a flat-walk
    /// misparse (the "magic 1" is the Image chunk's `[1, w, h, 8, mips,
    /// 1]` first word; the "0x36b0" is the Texture version 14000).
    /// Verified on all 8 real cell-82 textures by the m29 texmodel
    /// probe (ImageData DDS valid 8/8); this fixture commits the
    /// smallest one verbatim.
    #[test]
    fn cells82_embedded_texture_decodes() {
        let buf = fixture("cells82_texture_comm.p3d");
        let top = top_level_chunks(&buf).unwrap();
        let tex = top
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_TEXTURE)
            .map(|c| decode_texture(&buf, c.offset, c.size).unwrap())
            .unwrap();
        assert_eq!(tex.name, "communicationsarray001_diffuse.dds\u{0}\u{0}");
        assert_eq!(tex.fields[0], 14_000);
        assert_eq!((tex.fields[1], tex.fields[2]), (128, 128));
        assert_eq!(tex.fields[4], 1, "declared mip count");
        assert_eq!(&tex.fourcc, b"DXT1");
        // DDS carries the FULL mip chain: 128 header + (1024+256+64+16+
        // 4+1+1+1) DXT1 blocks × 8 B = 128 + 1367×8 = 11 064.
        assert_eq!(tex.dds.len(), 11_064);
        assert_eq!(&tex.dds[..4], b"DDS ");
        assert!(tex.source_path.is_some());
    }

    /// Material: name + param children with names + f32 runs.
    #[test]
    fn barrier_material_decodes() {
        let buf = fixture("art_barrier_os002.p3d");
        let top = top_level_chunks(&buf).unwrap();
        let mat = top
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_MATERIAL)
            .map(|c| decode_material(&buf, c.offset, c.size).unwrap())
            .unwrap();
        assert_eq!(mat.name, "barrierPlasticOS002\u{0}");
        assert_eq!(mat.field0, 1);
        assert_eq!(mat.field1, 3);
        assert_eq!(mat.params.len(), 3);
        assert_eq!(mat.params[0].name, "barrierPlasticOS002\u{0}");
        assert_eq!(mat.params[1].name, "barrierPlasticOS002Initial\u{0}\u{0}");
        assert_eq!(mat.params[2].name, "barrierPlasticOS002Damaged\u{0}\u{0}");
        assert!(!mat.params[0].floats.is_empty());
    }

    /// Block 26 — the 0x23001 records are per-drawable STATE records,
    /// not shader uniform params.  Pinned against the REAL barrier
    /// fixture (the same skeleton the block-26 census found on all 24
    /// records of the real cell-82 geometry member): the three state
    /// variants (base/Initial/Damaged) carry the IDENTICAL 30-f32
    /// identity-like skeleton with the `[0, 0]` tail.  This is why the
    /// "material floats → uniform rows" NEXT-UP item dissolved: there
    /// are no varying material float params in the observed corpus.
    #[test]
    fn material_params_are_per_drawable_state_records() {
        const SKELETON: [f32; 30] = [
            0.0, 1.0, 0.0, -0.0, 0.0, 0.0, 1.0, -0.0, 0.0, -0.0, -0.0, 1.0, -0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ];
        let buf = fixture("art_barrier_os002.p3d");
        let top = top_level_chunks(&buf).unwrap();
        let mat = top
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_MATERIAL)
            .map(|c| decode_material(&buf, c.offset, c.size).unwrap())
            .unwrap();
        assert_eq!(mat.params.len(), 3, "base + Initial + Damaged states");
        for p in &mat.params {
            assert_eq!(p.chunk_type, 0x0002_3001);
            assert_eq!(p.floats.len(), 30);
            // The sign bits (−0.0 quirks) are part of the record; compare
            // bit patterns, not float equality.
            for (got, want) in p.floats.iter().zip(SKELETON) {
                assert_eq!(got.to_bits(), want.to_bits(), "param {:?}", p.name);
            }
            assert_eq!(p.tail, [0u8, 0u8]);
        }
        // The material name is the drawable's, NOT a shader name: the
        // fixture's shader chunk has the MD5-style name, the material
        // has the readable shape name.
        let shader = top
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_SHADER)
            .map(|c| decode_shader_chunk(&buf, c.offset, c.size).unwrap())
            .unwrap();
        assert_ne!(shader.name, mat.name);
        assert_eq!(mat.name, "barrierPlasticOS002\u{0}");
    }

    /// Census on the barrier fixture: the TOP-LEVEL families counted, 0
    /// malformed.  The nested mesh-family ids (0x10012/13/20) are NOT
    /// visible to this histogram: its container probe cannot descend past
    /// name-led payloads (Shape/Mesh/Image payloads start with a pascal
    /// name, not a chunk prelude — that is exactly why the block-15 census
    /// reported them absent).  The real decoders above are the nested
    /// evidence; this pins the documented probe limitation.
    #[test]
    fn barrier_census_counts_top_level_families() {
        let buf = fixture("art_barrier_os002.p3d");
        let h = type_histogram(&buf);
        assert_eq!(h.malformed, 0);
        let find = |t: u32| h.types.iter().find(|x| x.chunk_type == t).map(|x| x.count);
        assert_eq!(find(CHUNK_TYPE_SHAPE), Some(2));
        assert_eq!(find(CHUNK_TYPE_TEXTURE), Some(1));
        assert_eq!(find(CHUNK_TYPE_MATERIAL), Some(1));
        assert_eq!(find(CHUNK_TYPE_MESH), None, "probe limitation, see doc comment");
    }

    /// The layout table covers every desc observed corpus-wide, and each
    /// row's arithmetic is self-consistent.
    #[test]
    fn layout_table_covers_all_corpus_descs() {
        for desc in [
            0x2001u32, 0x2010, 0x2011, 0x2012, 0x2013, 0x2021, 0x3011, 0x3012, 0x3013,
            0xA020, 0xA021, 0xA022, 0xA030, 0xA031, 0xA032, 0xB021, 0xB031, 0xB032, 0xB033,
            0x16001, 0x16011, 0x16012, 0x17011, 0x17012, 0x17013,
            0x6000_2011, 0x6000_2013, 0x6000_3011, 0x6000_B031, 0x6000_B032,
        ] {
            let l = vertex_layout(desc).unwrap_or_else(|| panic!("desc {desc:#x} missing"));
            // position 12 B at 0 + normal 12 + tangent 12 + uvs 8 each
            // + skin — total must be <= stride (residual = X4 / 8-B blocks).
            let mut need = 12usize;
            need += l.normal_offset.map_or(0, |_| 12);
            need += l.tangent_offset.map_or(0, |_| 12);
            need += l.uv_offsets.len() * 8;
            need += l.skin_bytes;
            assert!(need <= l.stride, "desc {desc:#x}: components {need} > stride {}", l.stride);
            // every offset + component size fits the stride
            if let Some(o) = l.normal_offset {
                assert!(o + 12 <= l.stride);
            }
            if let Some(o) = l.tangent_offset {
                assert!(o + 12 <= l.stride);
            }
            for &o in l.uv_offsets {
                assert!(o + 8 <= l.stride);
            }
        }
        // and an unobserved desc fails closed
        assert!(vertex_layout(0x1FFFF).is_none());
    }

    /// Truncated / malformed input fails closed with a specific error.
    #[test]
    fn errors_fail_closed() {
        let buf = fixture("art_barrier_os002.p3d");
        // Truncate mid-shape.
        let err = decode_shape(&buf[..0x9000], 0x8453, 23_357).unwrap_err();
        assert!(matches!(err, ArtError::Truncated { .. } | ArtError::NotExactCover { .. }));
        // Unknown desc: craft a mesh chunk (12-B prelude + version + empty
        // name + 36-B header with desc 0x1FFFF + empty child run).
        let mut m = Vec::new();
        m.extend_from_slice(&CHUNK_TYPE_MESH.to_le_bytes());
        m.extend_from_slice(&0u32.to_le_bytes()); // uid
        m.extend_from_slice(&[0u8; 4]); // size placeholder (unused by decode_mesh)
        m.extend_from_slice(&0u32.to_le_bytes()); // version
        m.push(0); // empty name
        m.extend_from_slice(&[0u8; 36]);
        m[21..25].copy_from_slice(&0x1FFFFu32.to_le_bytes()); // desc (12 prelude + 4 version + 1 name len + 4)
        m.extend_from_slice(&12u32.to_le_bytes()); // empty child run
        let err = decode_mesh(&m, 0, m.len()).unwrap_err();
        assert_eq!(err, ArtError::UnknownVertexDesc { desc: 0x1FFFF });
        // Not a P3D at all → no top-level walk.
        assert!(top_level_chunks(b"garbage!").is_none());
    }

    /// The old fixtures still contain no art-family chunks (the
    /// documented-absence proof, kept).  `frontendshaders.p3d` is
    /// excluded since block 37: it carries 12 `0x11009` shader-instance
    /// chunks (now a classified type) — see
    /// `shader_instances_decode_names_from_frontendshaders`.
    #[test]
    fn pre_art_fixtures_still_have_no_geometry_chunks() {
        for name in [
            "boot_strap.p3d",
            "skuinfo.p3d",
            "statsachievements_tod.p3d",
            "cementfiles.p3d",
        ] {
            let h = type_histogram(&fixture(name));
            for t in &h.types {
                assert!(
                    classify(t.chunk_type).is_none(),
                    "{name} unexpectedly contains {label} — update the evidence docs!",
                    label = classify(t.chunk_type).unwrap()
                );
            }
        }
    }

    #[test]
    fn classify_names_corpus_ids_only() {
        assert_eq!(classify(CHUNK_TYPE_MESH), Some("Mesh"));
        assert_eq!(classify(CHUNK_TYPE_IMAGE_DATA), Some("ImageData"));
        assert_eq!(classify(0x1234_5678), None, "unknown ids stay unnamed");
    }

    #[test]
    fn histogram_table_mentions_labels_and_unknowns() {
        let h = type_histogram(&fixture("boot_strap.p3d"));
        let table = histogram_table("boot_strap.p3d", &h);
        assert!(table.contains("0xFE000000"));
        assert!(table.contains("x281 "));
    }

    #[test]
    fn histogram_walks_synthetic_container() {
        let child_a = {
            let mut c = vec![0xAu8, 0, 0, 0, 1, 0, 0, 0, 20, 0, 0, 0];
            c.extend_from_slice(&[0u8; 8]);
            c
        };
        let child_b = {
            let mut c = vec![0xBu8, 0, 0, 0, 2, 0, 0, 0, 16, 0, 0, 0];
            c.extend_from_slice(&[0u8; 4]);
            c
        };
        let container_payload_len = child_a.len() + child_b.len();
        let mut container = vec![0xCu8, 0, 0, 0, 3, 0, 0, 0];
        let total = (12 + container_payload_len) as u32;
        container.extend_from_slice(&total.to_le_bytes());
        container.extend_from_slice(&child_a);
        container.extend_from_slice(&child_b);

        let mut data = vec![b'P', b'3', b'D', 0xFF];
        data.extend_from_slice(&12u32.to_le_bytes());
        data.extend_from_slice(&((12 + container.len()) as u32).to_le_bytes());
        data.extend_from_slice(&container);

        let h = type_histogram(&data);
        assert_eq!(h.malformed, 0);
        let find = |t: u32| h.types.iter().find(|x| x.chunk_type == t).map(|x| x.count);
        assert_eq!(find(0xA), Some(1));
        assert_eq!(find(0xB), Some(1));
        assert_eq!(find(0xC), Some(1));
    }

    #[test]
    fn file_header_is_not_censused_as_a_chunk() {
        let h = type_histogram(&fixture("boot_strap.p3d"));
        assert!(h.types.iter().all(|t| t.chunk_type != 0xFF44_3350));
        let bare = &fixture("boot_strap.p3d")[12..];
        let h2 = type_histogram(bare);
        assert!(!h2.had_file_header);
        assert_eq!(h2.types[0].chunk_type, 0xFE00_0000);
    }

    /// pure3dShader: family + params from real fixtures (barrier inline +
    /// two extracted corpus chunks).  Corpus evidence: version 256 on all
    /// 4 004 shader chunks; 139 distinct families; params 0x11016…0x11022.
    #[test]
    fn shader_chunks_decode_families_and_params() {
        // The barrier member carries its own shader (family '1c').
        let buf = fixture("art_barrier_os002.p3d");
        let top = top_level_chunks(&buf).unwrap();
        let sh = top
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_SHADER)
            .map(|c| decode_shader_chunk(&buf, c.offset, c.size).unwrap())
            .unwrap();
        assert_eq!(sh.name, "131185ef20d24489daf1165e65814182");
        assert_eq!(sh.family, "1c");
        assert_eq!(sh.self_name.as_deref(), Some("131185ef20d24489daf1165e65814182"));

        // Extracted corpus shaders: named engine families.
        let tire = {
            let b = fixture("art_shader_env_car_tire.p3d");
            let t = top_level_chunks(&b).unwrap();
            t.iter()
                .find(|c| c.chunk_type == CHUNK_TYPE_SHADER)
                .map(|c| decode_shader_chunk(&b, c.offset, c.size).unwrap())
                .unwrap()
        };
        assert_eq!(tire.family, "env_car_tire");
        assert!(tire.params_exact, "param run must be exact-cover on real data");
        assert!(!tire.params.is_empty());
        // 0x11016 params decode name + value (texture names).
        let named: Vec<_> = tire.params.iter().filter(|p| p.chunk_type == 0x0001_1016).collect();
        assert!(named.iter().all(|p| p.name.is_some() && p.value.is_some()));
        assert!(tire.params.iter().any(|p| p.name.as_deref() == Some("color")));

        let combo = {
            let b = fixture("art_shader_1c1s1n.p3d");
            let t = top_level_chunks(&b).unwrap();
            t.iter()
                .find(|c| c.chunk_type == CHUNK_TYPE_SHADER)
                .map(|c| decode_shader_chunk(&b, c.offset, c.size).unwrap())
                .unwrap()
        };
        assert_eq!(combo.family, "1c1s1n", "feature-combo family code");
        assert!(combo.params_exact);
    }

    /// A corrupted shader version fails closed.
    #[test]
    fn shader_bad_version_fails_closed() {
        let mut buf = fixture("art_shader_1c1s1n.p3d");
        // The version u32 sits after the chunk name: 12 (file header)
        // + 12 (chunk prelude) + 1 (len byte) + name length.  (Shader
        // chunk names are NOT always MD5-32 — this fixture's is the
        // readable 'rifle001Shader1'; the length byte is authoritative.)
        let voff = 12 + 12 + 1 + buf[24] as usize;
        buf[voff..voff + 4].copy_from_slice(&999u32.to_le_bytes());
        let top = top_level_chunks(&buf).unwrap();
        let err = top
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_SHADER)
            .map(|c| decode_shader_chunk(&buf, c.offset, c.size).unwrap_err())
            .unwrap();
        assert_eq!(err, ArtError::UnexpectedVersion { offset: voff, got: 999, want: SHADER_VERSION });
    }

    /// M28 (block 25): cells.rcf geometry members carry their meshes as
    /// `0x00010020` SECTIONS whose inline records
    /// `[u32 type][u32 size][u32 size][data]` parse as ordinary child
    /// chunks with uid == size — so the art mesh path decodes them
    /// unchanged.  Verified corpus-wide by scripts/m30f_verify.py:
    /// 235/235 sections of the real Cell-82 geometry member close the
    /// exact-cover walk, VB count == V*stride(desc) on 14 distinct descs,
    /// IB count == 2*I, all indices in range.
    ///
    /// This fixture is a VERBATIM sliver of that member: the
    /// billBoard001Ad001Shape quad (V=4, I=6, desc 0xA032, stride 48) +
    /// its 0x11015 shader chunk (family ao_billboard).
    #[test]
    fn cells_section_quad_decodes_through_art_mesh_path() {
        let buf = fixture("cells_section_quad.p3d");
        let top = top_level_chunks(&buf).expect("exact top-level cover");
        let shape = top
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_SHAPE)
            .expect("shape chunk");
        let sh = decode_shape(&buf, shape.offset, shape.size).unwrap();
        assert_eq!(sh.name.trim_end_matches('\u{0}'), "billBoard001Ad001Shape");
        assert_eq!(sh.version, 0);
        assert_eq!(sh.meshes.len(), 1, "the single 0x00010020 section");

        let m = &sh.meshes[0];
        // The section's pascal(32) name IS the pure3dShader hash —
        // the same name the member's 0x11015 chunk carries.
        assert_eq!(m.name, "575efc478e0dad75f8c5bfbd7ca9047b");
        assert_eq!(m.header.field0, 0);
        assert_eq!(m.header.desc, 0xA032);
        assert_eq!(m.header.vertex_count, 4);
        assert_eq!(m.header.index_count, 6);
        assert_eq!(m.header.tail, [0, 1, 1, 0, 0]);
        assert_eq!(m.layout.stride, 48);
        assert_eq!(m.positions.len(), 4);
        assert_eq!(m.indices, vec![0, 1, 2, 1, 3, 2]);

        // Positions land exactly on the shape bbox corners (measured).
        let bb = sh.bound_box.expect("shape-level BoundBox");
        assert!((m.positions[0][0] - -3.330_800).abs() < 1e-6);
        assert!((m.positions[0][1] - 9.056_926).abs() < 1e-6);
        assert!((m.positions[0][2] - -0.982_086).abs() < 1e-6);
        assert!((m.positions[3][0] - 3.044_186).abs() < 1e-6);
        assert!((m.positions[3][1] - 2.681_929).abs() < 1e-6);
        assert!((bb[0] - -3.330_800).abs() < 1e-6);
        assert!((bb[3] - 3.044_186).abs() < 1e-6);
        assert!((m.bound_box.unwrap()[0] - bb[0]).abs() < 1e-6);
        assert!((m.bound_sphere.unwrap()[3] - 4.507_803).abs() < 1e-6);

        // 0xA032 layout: normal at +36 (unit), UV at +20 (atlas sub-rect).
        for n in &m.normals {
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!((l - 1.0).abs() < 1e-5, "non-unit normal {n:?}");
        }
        assert_eq!(m.uvs.len(), 4);
        assert!((m.uvs[0][0] - 0.750_920).abs() < 1e-6);
        assert!((m.uvs[0][1] - 0.922_726).abs() < 1e-6);
        assert!((m.uvs[3][0] - 0.999_143).abs() < 1e-6);

        // The section's first inline record is the UNKNOWN 0x00010014
        // vertex-decl block (113 B on every section of the member) — it
        // must be tolerated (skipped by size), never fatal.
        assert!(m.child_types.contains(&0x0001_0014));
        assert!(m.child_types.contains(&CHUNK_TYPE_VERTEX_BUFFER));
        assert!(m.child_types.contains(&CHUNK_TYPE_INDEX_BUFFER));
    }

    /// M28 continued: a REAL multi-section cells shape (two 0xB031
    /// sections, V=8/I=12 and V=92/I=162) plus the full
    /// section→shader→texture chain inside one member: the first
    /// section's shader hash names the member's 0x11015 chunk, whose
    /// `color` param names the member's 0x19000 texture (a complete DDS).
    #[test]
    fn cells_section_textured_multi_section_chain() {
        let buf = fixture("cells_section_textured.p3d");
        let top = top_level_chunks(&buf).expect("exact top-level cover");

        let shape = top
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_SHAPE)
            .expect("shape chunk");
        let sh = decode_shape(&buf, shape.offset, shape.size).unwrap();
        assert_eq!(sh.name.trim_end_matches('\u{0}'), "signHorizontal001DamagedShape");
        assert_eq!(sh.meshes.len(), 2, "both sections decode");

        let (a, b) = (&sh.meshes[0], &sh.meshes[1]);
        assert_eq!(a.name, "67742f11c1bfde60105739308ef55d26");
        assert_eq!(a.header.desc, 0xB031);
        assert_eq!(a.layout.stride, 56);
        assert_eq!(a.header.vertex_count, 8);
        assert_eq!(a.header.index_count, 12);
        assert_eq!(b.name, "b2ec7bc9261797200c7d60d699dc7cbc");
        assert_eq!(b.header.vertex_count, 92);
        assert_eq!(b.header.index_count, 162);
        assert_eq!(b.positions.len(), 92);
        assert!(b.indices.iter().max().unwrap() < &92);
        for m in [&sh.meshes[0], &sh.meshes[1]] {
            for n in &m.normals {
                let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                assert!(
                    l == 0.0 || (l - 1.0).abs() < 1e-3,
                    "non-unit normal {n:?}"
                );
            }
        }

        // Shader chain: hash name -> 0x11015 (family + color param).
        let shader = top
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_SHADER)
            .map(|c| decode_shader_chunk(&buf, c.offset, c.size).unwrap())
            .unwrap();
        assert_eq!(shader.name, a.name, "mesh name binds the shader");
        assert_eq!(shader.family, "prop_damage_gn");

        let color = shader
            .params
            .iter()
            .find(|p| p.name.as_deref() == Some("color"))
            .expect("color param");
        let tex_name = color.value.as_deref().unwrap();
        assert_eq!(tex_name, "communicationsarray001_diffuse.dds");

        // Texture chain: the named 0x19000 texture decodes to a DDS.
        let tex = top
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_TEXTURE)
            .map(|c| decode_texture(&buf, c.offset, c.size).unwrap())
            .unwrap();
        assert_eq!(tex.name.trim_end_matches('\u{0}'), tex_name);
        assert!(tex.dds.starts_with(b"DDS "), "embedded DDS payload");
    }

    /// Block-37: the `0x00011009` shader-INSTANCE chunks of the real
    /// frontend corpus decode with their exact engine names.  These are
    /// the strings the engine's parse (`0x10b95780`) reads into its
    /// frame buffer and hands to the instance factory `0x1061bcd0` as
    /// the NAME argument (call site `0x10b95948`; buffer identity
    /// proven by symbolic-ESP tracking — the same frame slot is lea'd
    /// at `0x10b957b2` for the stream read and `0x10b95940` for the
    /// factory argument).  The names are READABLE, not hashes and not
    /// family codes — closing the block-34 "hash name vs family
    /// string" question from the data side: the instance registry is
    /// NAME-keyed.
    #[test]
    fn shader_instances_decode_names_from_frontendshaders() {
        fn collect(buf: &[u8], start: usize, end: usize, out: &mut Vec<ShaderInstanceChunk>) {
            if let Some(kids) = child_chunks(buf, start, end) {
                for c in kids {
                    if c.chunk_type == CHUNK_TYPE_SHADER_INSTANCE {
                        out.push(decode_shader_instance(buf, c.offset, c.size).unwrap());
                    }
                    collect(buf, c.payload(), c.offset + c.size, out);
                }
            }
        }
        let buf = fixture("frontendshaders.p3d");
        let mut found = Vec::new();
        collect(&buf, 12, buf.len(), &mut found);
        let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
        let want = [
            "CxformGauraud",
            "CxformGauraudMultiply",
            "CxformGauraudMultiplyNoAddAlpha",
            "CxformGauraudMultiplyTexture",
            "CxformGauraudNoAddAlpha",
            "CxformGauraudTexture",
            "cxformsolidcolour",
            "cxformtexture",
            "CxformTextureMultiply",
            "gfxtextshader",
            "hud_2dmarker",
            "hud_minimapmarker",
        ];
        assert_eq!(found.len(), 12, "census: {names:?}");
        for w in want {
            assert!(names.contains(&w), "missing instance name {w:?} in {names:?}");
        }
        // Names are the whole string the engine interns (no NULs).
        assert!(found.iter().all(|s| !s.name.contains('\u{0}')));
    }

    /// Block-37 cross-check: the `0x11015` pure3dShader layout matches
    /// the engine's fixup-parse read order EXACTLY (name pascal, u32
    /// version, family pascal) on the readable-name fixture — the same
    /// order the disassembly proves for `0x10b96280`, which never
    /// calls the factory.
    #[test]
    fn shader_chunk_read_order_matches_engine_parse() {
        let buf = fixture("art_shader_1c1s1n.p3d");
        let top = top_level_chunks(&buf).unwrap();
        let sh = top
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_SHADER)
            .map(|c| decode_shader_chunk(&buf, c.offset, c.size).unwrap())
            .unwrap();
        // First pascal = instance NAME (readable here), second = family.
        assert_eq!(sh.name.trim_end_matches('\u{0}'), "rifle001Shader1");
        assert_eq!(sh.family, "1c1s1n");

        // The env fixture's name is the 32-hex content hash — the two
        // name styles coexist; both are NAME-keyed, family stays
        // separate data.
        let b = fixture("art_shader_env_car_tire.p3d");
        let t = top_level_chunks(&b).unwrap();
        let tire = t
            .iter()
            .find(|c| c.chunk_type == CHUNK_TYPE_SHADER)
            .map(|c| decode_shader_chunk(&b, c.offset, c.size).unwrap())
            .unwrap();
        assert_eq!(tire.name.trim_end_matches('\u{0}'), "34c7c8d5511a5cb8db3057c73a99db57");
        assert_eq!(tire.family, "env_car_tire");
    }

}

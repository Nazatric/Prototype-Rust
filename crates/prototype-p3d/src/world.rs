//! world — cells.rcf world decoders (M25, block 18).
//!
//! `cells.rcf` is the streamed-world archive: 520 members, all RZ-wrapped
//! P3D, every top-level chunk walk exact-cover (corpus PROVEN).  It holds
//! three member kinds:
//!
//! * **object members** (~147): placed world objects —
//!   `0x07F00000` GameObject chunks (81,370 corpus-wide) with world
//!   placement, plus shaders/materials;
//! * **geometry members** (~149): the static world — Shape/Mesh chunks
//!   (7,546 Shapes) + `0x07020000` collision models, one cell each for
//!   the 106 members whose shape names carry `Cell_NN` (0 multi-cell);
//! * **empty members** (~224): a bare 12-byte `P3D\xff` header.
//!
//! ## PROVEN layouts (corpus-wide, real `cells.rcf`, 520/520 members)
//!
//! ### GameObject chunk `0x07F00000` (81,370 instances)
//!
//! ```text
//! [pascal full-name]            "proto::CloudSpawner(1):Cell_65Collectible004_38"
//! [pascal short-name]           "Cell_65Collectible004_38"
//! [pascal class]                "proto::CloudSpawner"
//! [u32 1 | 0x00010001]          (class-dependent flag)
//! [u32 0x36a4173b]              CONSTANT hash (81,370/81,370)
//! [u32 0x07f00001]              child marker (the 0x07F00001 type id)
//! [u32 A][u32 B][u32 C]         A == B == C+16 observed; semantics UNKNOWN
//! "META"                        4-byte marker
//! ```
//!
//! then one of the variants below (block 41 added the three typed
//! `proto::*` families, routed by the class pascal and structurally
//! fail-closed):
//!
//! * **binary** (CloudSpawner, ParkedCarSpawner, EffectsPropSpawner, …):
//!   the u32 after META is NOT a count (it is the first placement float),
//!   and the layout is
//!   `[pos 3×f32][quat x,y,z,w 4×f32][pascal drawable][scale 3×f32][u32 len][state]`.
//!   |q| ≈ 1 for 71,142/71,178; positions span the real map
//!   x[-1752,1708] y[-0.5,350] z[-2713,1658].  An empty drawable is a
//!   0-length pascal (10,299 — the ParkedCarSpawners).
//!   (The 71,178 bucket is the pre-block-41 census, which still counted
//!   the two raw-layout families below as binary.)
//! * **prop table** (`GameObject`, `proto::ShaderPalette`):
//!   `[u32 count][count × prop]` where
//!   `prop = [u32 keylen][key][u32 typelen][type][u16 n][n × element]`:
//!   - `MatrixAttribute` element = 28 B = pos 3×f32 + quat x,y,z,w
//!     4×f32 (1,720/2,091); 371 carry a further constant 10-byte tail
//!     `01 00 00 00 00 00 01 00 00 00` (semantics UNKNOWN);
//!   - `VectorAttribute` element = 12 B (3×f32);
//!   - `BoolAttribute` element = 4 B (u32);
//!   - `NameAttribute` element = [u32 strlen][chars].
//!   The table is followed by a behaviour list this decoder does not
//!   need; parsing advances by the proven element stride per type and
//!   stops cleanly when the next key stops looking like
//!   `[u32 len][printable][u32 len][printable][u16]`.
//!   (The pre-block-41 10,192-record bucket also held the 4,697
//!   `proto::LightDecalInfo` records, now typed separately — their
//!   count field made the strict walk stall at prop 0.)
//! * **`proto::LightDecalInfo`** (4,697/4,697 cells.rcf-wide, block 41):
//!   the u32 after META is the HOST-NAME byte length, and the payload is
//!   `[u32 cnt][A: cnt bytes][u32 n][n × ([u32 len][len bytes])]` —
//!   `A` = host prop name (`cnt` == its length; 0 = the per-cell
//!   TEMPLATE record), the `n` strings = decal names
//!   (`Cell_NN_light_decal_K`; streetlight hosts carry 2, road-curve
//!   hosts 1, template records up to 27).
//! * **`proto::AmbientExclusionVolume`** (227/227, block 41): raw 52-B
//!   oriented box `[pos xyz 3f][quat xyzw — unit][hx hy hz][u32 a]
//!   [u32 b][u32 c]` — the SAME `[pos][quat]` head as the binary
//!   variant; positions span the real map (x[-905,1126] y[6.1,42.3]
//!   z[-1839,1447]); tail domain a,b ∈ {0,1}, c ∈ {0,2,6} — semantics
//!   UNKNOWN.
//! * **`proto::WebNodeLocator`** (114/114, block 41): raw 32-B ground
//!   node `[pos xyz 3f][quat xyzw — unit][u32 id]`; `id` equals the
//!   short name's numeric suffix (`woi_loc_47` → 47).
//!
//! ### Node transform chunk `0x00124000` (773 instances, geometry members)
//!
//! Payload = 5 zero bytes then exactly one child `0x00010029` covering
//! the rest of the chunk (773/773).  Child payload:
//! `[u32 0][pos 3×f32][quat w,x,y,z 4×f32][f32 1.0][f32 ~0.0][extents 3×f32]`.
//! NOTE the quaternion order: **w first** here, x,y,z,w in the GameObject
//! variants.  The (1.0, ~0.0) pair is corpus-constant, semantics UNKNOWN;
//! extents are the node's positive bounding-box half-extents.
//!
//! ## World index
//!
//! [`scan_world_member`] walks one decompressed member cheaply (chunk
//! preludes + names + placements only — no mesh decode) and produces a
//! [`WorldIndexEntry`]: member kind, cell ids (from `Cell_NN` in object
//! full-names and shape names), placement bbox, honest counters.  A
//! [`WorldIndex`] answers "which members are near the camera" —
//! replacing the blind in-order member scan at boot.

#![forbid(unsafe_code)]

use crate::geometry::{
    top_level_chunks, ArtError, ChildChunk, CHUNK_TYPE_MATERIAL, CHUNK_TYPE_SHADER,
    CHUNK_TYPE_SHAPE, CHUNK_TYPE_TEXTURE,
};

/// GameObject chunk type id (cells.rcf placed world objects).
pub const CHUNK_TYPE_GAME_OBJECT: u32 = 0x07F0_0000;
/// Node transform chunk type id (cells.rcf geometry members).
pub const CHUNK_TYPE_NODE_TRANSFORM: u32 = 0x0012_4000;
/// The transform child inside a node transform chunk.
pub const NODE_TRANSFORM_CHILD: u32 = 0x0001_0029;
/// The constant hash in every GameObject header (81,370/81,370).
pub const GAME_OBJECT_HASH: u32 = 0x36A4_173B;
/// The child marker u32 in every GameObject header.
pub const GAME_OBJECT_MARKER: u32 = 0x07F0_0001;
/// Collision model chunk family (`0x07020000`, cells.rcf geometry members).
pub const CHUNK_TYPE_COLLISION: u32 = 0x0702_0000;

/// A world placement: position + unit-ish quaternion (x, y, z, w).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// World position.
    pub pos: [f32; 3],
    /// Quaternion, x/y/z/w order (as stored in both GameObject variants).
    pub quat_xyzw: [f32; 4],
}

impl Placement {
    /// Quaternion norm (1.0 for 71,142 of 71,178 corpus placements).
    pub fn quat_norm(&self) -> f32 {
        let [x, y, z, w] = self.quat_xyzw;
        (x * x + y * y + z * z + w * w).sqrt()
    }

    /// `true` when the quaternion norm is within [0.9, 1.1].
    pub fn is_unit_quat(&self) -> bool {
        (0.9..=1.1).contains(&self.quat_norm())
    }
}

/// Which META layout a GameObject used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ObjectVariant {
    /// "META" + u32 count + named attribute properties.
    PropTable,
    /// "META" + raw `[pos][quat][drawable][scale][state]`.
    Binary,
    /// `proto::LightDecalInfo` — "META" + the host-name length + the
    /// host name + `n` u32-length decal names (block 41, PROVEN
    /// 4,697/4,697 across cells.rcf).
    LightDecal,
    /// `proto::AmbientExclusionVolume` — "META" + raw 48-B oriented box
    /// (block 41, PROVEN 227/227).
    AmbientExclusionVolume,
    /// `proto::WebNodeLocator` — "META" + raw 28-B ground node (block
    /// 41, PROVEN 114/114).
    WebNodeLocator,
}

/// `proto::LightDecalInfo` payload (block 41 — PROVEN on 4,697/4,697
/// records across every object member of the real cells.rcf):
///
/// ```text
/// [u32 cnt][A: cnt bytes][u32 n][n × ([u32 len][len bytes])]
/// ```
///
/// `A` is the HOST prop's name — `cnt` is exactly its byte length, and
/// `0` marks the per-cell TEMPLATE record (short name
/// `__light_decal_info`, listing the whole cell's decal set; corpus max
/// n = 27).  The `n` strings are the decal resource names
/// (`Cell_NN_light_decal_K`) the engine attaches to that host:
/// streetlight hosts carry 2 (one per head), road-curve hosts 1.
#[derive(Debug, Clone, PartialEq)]
pub struct LightDecalInfo {
    /// Host prop name (empty for the per-cell template records).
    pub host_prop: String,
    /// Decal names attached to the host (1..=27 corpus; 2 for
    /// streetlights, 1 for road curves).
    pub decals: Vec<String>,
}

/// `proto::AmbientExclusionVolume` payload (block 41 — PROVEN on
/// 227/227 records, all unit quaternions):
///
/// ```text
/// [pos x,y,z 3×f32][quat x,y,z,w 4×f32][hx][hy][hz][u32 a][u32 b][u32 c]
///                                                            = 52 B
/// ```
///
/// The SAME `[pos][quat]` head as the binary placement variant, then a
/// half-extent triple and a 3-u32 tail instead of drawable/scale/state.
/// Positions span the real map x[-905,1126] y[6.1,42.3] z[-1839,1447]
/// (world-scale ✓).  Half-extents are x / height (y-up) / z
/// (hx [1.38,185.1], hy [0.50,26.0], hz [1.03,175.1]).  The tail's
/// observed value domain is a,b ∈ {0,1}, c ∈ {0,2,6} — semantics
/// UNKNOWN (fail-closed structure, honest fields).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AmbientExclusionVolume {
    /// World position (x, y, z) — the volume's center.
    pub pos: [f32; 3],
    /// Unit orientation quaternion, x/y/z/w order.
    pub quat_xyzw: [f32; 4],
    /// Positive half-extents (ground-x, height, ground-z).
    pub half_extents: [f32; 3],
    /// Corpus tail (a, b, c) — semantics UNKNOWN, domain pinned.
    pub tail: [u32; 3],
}

/// `proto::WebNodeLocator` payload (block 41 — PROVEN on 114/114
/// records, all unit quaternions, every id equal to its short name's
/// numeric suffix):
///
/// ```text
/// [pos x,y,z 3×f32][quat x,y,z,w 4×f32][u32 id]         = 32 B
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WebNodeLocator {
    /// World position (x, y, z).
    pub pos: [f32; 3],
    /// Unit orientation quaternion, x/y/z/w order.
    pub quat_xyzw: [f32; 4],
    /// Locator id — equals the short name's numeric suffix
    /// (`woi_loc_47` → 47; 114/114 corpus).
    pub id: u32,
}

/// Typed payload carried by the GameObject families whose layouts are
/// proven beyond the plain spawner/prop-table grammars (block 41).
#[derive(Debug, Clone, PartialEq)]
pub enum ObjectPayload {
    /// `proto::LightDecalInfo`.
    LightDecal(LightDecalInfo),
    /// `proto::AmbientExclusionVolume`.
    AmbientExclusion(AmbientExclusionVolume),
    /// `proto::WebNodeLocator`.
    WebLocator(WebNodeLocator),
}

/// One decoded placed world object (`0x07F00000`).
#[derive(Debug, Clone, PartialEq)]
pub struct CellObject {
    /// First pascal, e.g. `proto::CloudSpawner(1):Cell_65Collectible004_38`.
    pub full_name: String,
    /// Second pascal (short name), e.g. `Cell_65Collectible004_38`.
    pub short_name: String,
    /// Third pascal (class), e.g. `proto::CloudSpawner`.
    pub class: String,
    /// Which META layout carried the placement.
    pub variant: ObjectVariant,
    /// World placement.  `None` for the prop-table objects that carry
    /// no `transform` property and for `LightDecalInfo` (no placement
    /// in its grammar).  The block-41 AEV/WNL records DO carry the
    /// standard `[pos][quat]` head and are placed.
    pub placement: Option<Placement>,
    /// Drawable/model name (binary variant pascal, prop-table
    /// `drawableName`).  `None` when absent/empty.
    pub drawable: Option<String>,
    /// Scale vector (binary variant floats, prop-table `scale`).
    pub scale: Option<[f32; 3]>,
    /// State string (binary variant only, usually `initial`).
    pub state: Option<String>,
    /// Typed payload for the block-41 families (`LightDecalInfo`,
    /// `AmbientExclusionVolume`, `WebNodeLocator`); `None` for the
    /// plain prop-table/binary variants.
    pub payload: Option<ObjectPayload>,
}

/// A geometry-member node transform (`0x00124000` → `0x00010029`).
///
/// Block-23 context (real cell-82 member, 4 transforms): the nodes sit
/// street-spaced along the member's geometry (z 235.9 → 447.2, ~70 m
/// apart) with building-block-sized extents (~48 × 25 × 23) — they read
/// as the member's per-block streaming/culling volumes (LIKELY role).
/// The members' LOCAL store shapes carry NO record-side world placement
/// and no node reference; their world instances are most plausibly
/// baked into the member's merged roots (or placed by the engine's
/// building-template system) — the exact mapping stays UNKNOWN, and the
/// draw path does not depend on it (see `PairedDrawable::placement`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeTransform {
    /// World position.
    pub pos: [f32; 3],
    /// Quaternion, **w/x/y/z order** (as stored — unlike the GameObject
    /// variants, which are x/y/z/w).
    pub quat_wxyz: [f32; 4],
    /// Corpus-constant pair (1.0, ~0.0) — semantics UNKNOWN.
    pub unknown: [f32; 2],
    /// Positive bounding-box half-extents.
    pub extents: [f32; 3],
}

/// Scan options for [`scan_world_member_opts`] (block 20).
#[derive(Debug, Clone, Copy, Default)]
pub struct ScanOptions {
    /// Retain every successfully-decoded GameObject as a full
    /// [`CellObject`] in [`WorldIndexEntry::retained_objects`].  The
    /// cheap index pass keeps this OFF (the index needs counts and
    /// bboxes only); the entity-spawn pass turns it ON to feed the
    /// runtime's real placed-object database.  Corpus scale: 81,370
    /// objects × ~120 B ≈ 10 MB — acceptable at boot, pointless in the
    /// index.
    pub retain_objects: bool,
    /// Block 22: keep EVERY shape name in
    /// [`WorldIndexEntry::shape_names_all`] (the 4-name diagnostic cap
    /// in `shape_names` stays).  The GameObject→drawable pairing needs
    /// the full per-member shape list; corpus scale ≈ 7,546 names
    /// × ~30 B ≈ 0.25 MB — negligible next to object retention.
    pub retain_shape_names: bool,
    /// Block 22: retain decoded [`NodeTransform`]s in
    /// [`WorldIndexEntry::node_transform_list`].  Corpus scale: 773
    /// transforms × 48 B — negligible.  These are the world-space
    /// volumes the local-transform prop-table records compose into
    /// (the pairing rule between them is still UNKNOWN).
    pub retain_node_transforms: bool,
}

/// One placed world object flattened out of a scanned member — the
/// unit the entity-spawn path consumes (M25b, block 20).
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedObject {
    /// RCF hash of the member the object came from.
    pub member_hash: u32,
    /// The member's `Cell_NN` ids (the object's full name is where
    /// the cell id came from; the member may carry more).
    pub cells: Vec<u32>,
    /// The decoded object.
    pub object: CellObject,
}

/// Member classification derived from the chunk census.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MemberKind {
    /// Bare 12-byte `P3D\xff` header (no chunks).
    #[default]
    Empty,
    /// Carries placed GameObjects.
    Objects,
    /// Carries Shapes / collision (the static world).
    Geometry,
    /// Neither (shader-only, GFX, malformed, …).
    Other,
}

/// What one member scan found.  Honest counters: a GameObject that
/// fails to decode is counted, not hidden.
#[derive(Debug, Clone, Default)]
pub struct WorldIndexEntry {
    /// RCF directory hash of the member.
    pub hash: u32,
    /// Decompressed P3D byte length (12 = empty member).
    pub decompressed_len: u32,
    /// Derived member kind.
    pub kind: MemberKind,
    /// `Cell_NN` ids found in object full-names and shape names.
    pub cells: Vec<u32>,
    /// GameObject chunks found.
    pub objects: u32,
    /// GameObjects that failed to decode.
    pub object_errors: u32,
    /// Shape chunks found.
    pub shapes: u32,
    /// Collision model chunks (`0x07020000`).
    pub collisions: u32,
    /// Node transform chunks (`0x00124000`).
    pub node_transforms: u32,
    /// Texture / shader / material chunks (art data living in cells).
    pub textures: u32,
    pub shaders: u32,
    pub materials: u32,
    /// Min/max of every placement position (None when no placements).
    pub bbox: Option<[f32; 6]>,
    /// First few unique drawable names (diagnostics).
    pub drawables: Vec<String>,
    /// First few shape names (diagnostics).
    pub shape_names: Vec<String>,
    /// First few decode-error messages (bounded).
    pub error_notes: Vec<String>,
    /// Full decoded GameObjects — EMPTY unless the scan was asked to
    /// retain them ([`ScanOptions::retain_objects`]).
    pub retained_objects: Vec<CellObject>,
    /// EVERY shape name in member order — EMPTY unless the scan was
    /// asked to retain them ([`ScanOptions::retain_shape_names`]).
    /// This is the lookup side of the GameObject→drawable pairing.
    pub shape_names_all: Vec<String>,
    /// Decoded node transforms — EMPTY unless the scan was asked to
    /// retain them ([`ScanOptions::retain_node_transforms`]).
    pub node_transform_list: Vec<NodeTransform>,
}

impl WorldIndexEntry {
    /// The name list the pairing should match against: the full list
    /// when retained, the 4-name diagnostic cap otherwise.
    pub fn pairing_shape_names(&self) -> &[String] {
        if self.shape_names_all.is_empty() {
            &self.shape_names
        } else {
            &self.shape_names_all
        }
    }
}

impl WorldIndexEntry {
    fn note_pos(&mut self, pos: [f32; 3]) {
        let b = self
            .bbox
            .get_or_insert([f32::MAX, f32::MAX, f32::MAX, -f32::MAX, -f32::MAX, -f32::MAX]);
        b[0] = b[0].min(pos[0]);
        b[1] = b[1].min(pos[1]);
        b[2] = b[2].min(pos[2]);
        b[3] = b[3].max(pos[0]);
        b[4] = b[4].max(pos[1]);
        b[5] = b[5].max(pos[2]);
    }

    fn note_cell(&mut self, id: u32) {
        if !self.cells.contains(&id) {
            self.cells.push(id);
        }
    }

    /// Distance from `cam` to the bbox center (INFINITY without a bbox).
    pub fn distance_from(&self, cam: [f32; 3]) -> f32 {
        match self.bbox {
            Some(b) => bbox_center_distance(b, cam),
            None => f32::INFINITY,
        }
    }
}

/// The placement-bbox center distance (M25c, block 43): exposed as a
/// free function so the member-level streaming table
/// (`EngineRuntime::attach_member_stream` → `MemberRef`) computes
/// distances with the EXACT formula [`WorldIndexEntry::distance_from`]
/// uses — the two consumers cannot drift.
pub fn bbox_center_distance(bbox: [f32; 6], cam: [f32; 3]) -> f32 {
    let [xmin, ymin, zmin, xmax, ymax, zmax] = bbox;
    let cx = (xmin + xmax) * 0.5 - cam[0];
    let cy = (ymin + ymax) * 0.5 - cam[1];
    let cz = (zmin + zmax) * 0.5 - cam[2];
    (cx * cx + cy * cy + cz * cz).sqrt()
}

fn note_unique(list: &mut Vec<String>, cap: usize, name: &str) {
    if list.len() < cap && !list.iter().any(|s| s == name) {
        list.push(name.to_string());
    }
}

/// `Cell_NN` extraction: the digits after the FIRST `Cell_` in `name`.
/// `Cell_67_light_decal_0` → 67, `…Cell_65Collectible004_38` → 65,
/// `GameObject(1):NIS_BoxStack1` → None.  A `Cell_` not followed by at
/// least one digit is None.
pub fn extract_cell_id(name: &str) -> Option<u32> {
    let b = name.as_bytes();
    for i in 0..b.len().saturating_sub(4) {
        if &b[i..i + 5] == b"Cell_" {
            let mut j = i + 5;
            let mut v: u32 = 0;
            let mut any = false;
            while j < b.len() && b[j].is_ascii_digit() {
                v = v.saturating_mul(10).saturating_add((b[j] - b'0') as u32);
                any = true;
                j += 1;
            }
            if any {
                return Some(v);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// GameObject (0x07F00000)
// ---------------------------------------------------------------------------

/// Decode one GameObject chunk (`offset..offset+size` inside `buf`).
///
/// Fail-closed on the proven header (pascals, constant hash, marker,
/// "META") and on every typed payload below; the plain prop-table walk
/// stays fail-soft at the behaviour tail (the behaviour list is not
/// needed, so a stalled parse just stops the table).
///
/// Block-23: the binary variant serves the WHOLE `proto::*` engine-class
/// family, not just plain spawners — `proto::CloudSpawner`,
/// `proto::ParkedCarSpawner`, `proto::EffectsPropSpawner` all share this
/// exact head and payload (637 records in the real cell-82 member
/// decode with unit quaternions and world-scale positions).  The class
/// string (third pascal) carries the RTTI name; the payload layout does
/// not change.
///
/// Block-41: three more `proto::*` families are TYPED (routed by the
/// class pascal, structurally fail-closed — a class-name match with a
/// non-conforming payload is an ERROR, never a guess):
/// `proto::LightDecalInfo` (4,697/4,697), `proto::AmbientExclusionVolume`
/// (227/227) and `proto::WebNodeLocator` (114/114) across the real
/// cells.rcf.  The cell-82 member now decodes 712/712 with ZERO errors
/// (previously 9 fail-closed errors).
pub fn decode_game_object(buf: &[u8], offset: usize, size: usize) -> Result<CellObject, ArtError> {
    let end = offset + size;
    let mut p = offset + 12; // skip prelude
    if p + 24 > end {
        return Err(ArtError::Truncated { offset: p, need: 24, got: end - p });
    }
    let full = read_pascal_trimmed(buf, &mut p, end)?;
    let short = read_pascal_trimmed(buf, &mut p, end)?;
    let class = read_pascal_trimmed(buf, &mut p, end)?;
    // fixed 24-byte header: [flag][hash][marker][A][B][C]
    let flag = u32_at(buf, p, end)?;
    if flag != 1 && flag != 0x0001_0001 {
        return Err(ArtError::UnexpectedVersion { offset: p, got: flag, want: 1 });
    }
    let hash = u32_at(buf, p + 4, end)?;
    if hash != GAME_OBJECT_HASH {
        return Err(ArtError::UnexpectedVersion {
            offset: p + 4,
            got: hash,
            want: GAME_OBJECT_HASH,
        });
    }
    let marker = u32_at(buf, p + 8, end)?;
    if marker != GAME_OBJECT_MARKER {
        return Err(ArtError::UnexpectedVersion {
            offset: p + 8,
            got: marker,
            want: GAME_OBJECT_MARKER,
        });
    }
    p += 24; // A/B/C are read but their semantics are UNKNOWN
    if p + 4 > end {
        return Err(ArtError::Truncated { offset: p, need: 4, got: end - p });
    }
    if &buf[p..p + 4] != b"META" {
        return Err(ArtError::ExpectedMeta { offset: p });
    }
    p += 4;
    // Block-41 typed families: route by class pascal, parse
    // structurally fail-closed.  Each family's grammar was PROVEN on
    // the full real cells.rcf corpus (see the variant docs); a class
    // match whose payload violates the grammar returns Err (counted in
    // `object_errors`, never guessed through).
    if class.contains("LightDecalInfo") {
        return decode_light_decal_info(buf, p, end, full, short, class);
    }
    if class.contains("AmbientExclusionVolume") {
        return decode_ambient_exclusion_volume(buf, p, end, full, short, class);
    }
    if class.contains("WebNodeLocator") {
        return decode_web_node_locator(buf, p, end, full, short, class);
    }
    let cnt = u32_at(buf, p, end)?;
    if cnt > 0 && cnt <= 4096 {
        decode_prop_table(buf, p + 4, end, full, short, class)
    } else {
        decode_binary(buf, p, end, full, short, class)
    }
}

/// `proto::LightDecalInfo` (block 41, PROVEN 4,697/4,697 across
/// cells.rcf): after META, `[u32 cnt][A: cnt bytes][u32 n]
/// [n × ([u32 len][len bytes])]`.  `cnt` is the HOST name's byte length
/// (0 = per-cell template record), `n` the decal-name count.  Bounded,
/// printable, and the payload must end EXACTLY at the chunk end.
fn decode_light_decal_info(
    buf: &[u8],
    p: usize,
    end: usize,
    full: String,
    short: String,
    class: String,
) -> Result<CellObject, ArtError> {
    let cnt = u32_at(buf, p, end)? as usize;
    if cnt > 96 {
        return Err(ArtError::BadPlacement {
            offset: p,
            why: format!("LightDecalInfo host-name length {cnt} > 96"),
        });
    }
    let name_end = p + 4 + cnt;
    if name_end > end {
        return Err(ArtError::Truncated {
            offset: p + 4,
            need: cnt,
            got: end.saturating_sub(p + 4),
        });
    }
    let host = &buf[p + 4..name_end];
    if !host.iter().all(|c| (32..127).contains(c)) {
        return Err(ArtError::BadPlacement {
            offset: p + 4,
            why: "LightDecalInfo host name has non-printable bytes".into(),
        });
    }
    let n = u32_at(buf, name_end, end)?;
    if n == 0 || n > 64 {
        return Err(ArtError::BadPlacement {
            offset: name_end,
            why: format!("LightDecalInfo decal count {n} outside 1..=64"),
        });
    }
    let mut q = name_end + 4;
    let mut decals = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let sl = u32_at(buf, q, end)? as usize;
        if sl == 0 || sl > 96 {
            return Err(ArtError::BadPlacement {
                offset: q,
                why: format!("LightDecalInfo decal-name length {sl} outside 1..=96"),
            });
        }
        q += 4;
        if q + sl > end {
            return Err(ArtError::Truncated {
                offset: q,
                need: sl,
                got: end - q,
            });
        }
        let s = &buf[q..q + sl];
        if !s.iter().all(|c| (32..127).contains(c)) {
            return Err(ArtError::BadPlacement {
                offset: q,
                why: "LightDecalInfo decal name has non-printable bytes".into(),
            });
        }
        decals.push(String::from_utf8_lossy(s).to_string());
        q += sl;
    }
    if q != end {
        return Err(ArtError::NotExactCover { offset: q, end });
    }
    Ok(CellObject {
        full_name: full,
        short_name: short,
        class,
        variant: ObjectVariant::LightDecal,
        placement: None,
        drawable: None,
        scale: None,
        state: None,
        payload: Some(ObjectPayload::LightDecal(LightDecalInfo {
            host_prop: String::from_utf8_lossy(host).to_string(),
            decals,
        })),
    })
}

/// `proto::AmbientExclusionVolume` (block 41, PROVEN 227/227):
/// after META, exactly 52 B = `[pos xyz][quat xyzw][hx hy hz][a b c]`
/// — the SAME `[pos][quat]` head as the binary placement variant.
/// The quaternion must be unit (corpus norm 1.000; gate 0.01).
fn decode_ambient_exclusion_volume(
    buf: &[u8],
    p: usize,
    end: usize,
    full: String,
    short: String,
    class: String,
) -> Result<CellObject, ArtError> {
    let len = end - p;
    if len != 52 {
        return Err(ArtError::BadPlacement {
            offset: p,
            why: format!("AmbientExclusionVolume payload {len} B, want 52"),
        });
    }
    let pos = f32x3(buf, p)?;
    let quat_xyzw = f32x4(buf, p + 12)?;
    let half_extents = f32x3(buf, p + 28)?;
    let tail = [
        u32_at(buf, p + 40, end)?,
        u32_at(buf, p + 44, end)?,
        u32_at(buf, p + 48, end)?,
    ];
    let n = quat_norm(quat_xyzw);
    if (n - 1.0).abs() > 0.01 {
        return Err(ArtError::BadPlacement {
            offset: p + 12,
            why: format!("AmbientExclusionVolume quaternion norm {n:.4} is not unit"),
        });
    }
    Ok(CellObject {
        full_name: full,
        short_name: short,
        class,
        variant: ObjectVariant::AmbientExclusionVolume,
        placement: Some(Placement { pos, quat_xyzw }),
        drawable: None,
        scale: None,
        state: None,
        payload: Some(ObjectPayload::AmbientExclusion(AmbientExclusionVolume {
            pos,
            quat_xyzw,
            half_extents,
            tail,
        })),
    })
}

/// `proto::WebNodeLocator` (block 41, PROVEN 114/114): after META,
/// exactly 32 B = `[pos xyz][quat xyzw][u32 id]`; unit quaternion, and
/// `id` equals the short name's numeric suffix (`woi_loc_47` → 47 —
/// 114/114 corpus).  A short name without trailing digits cannot pin
/// the id and is accepted (the pin is documented as name-conditional).
fn decode_web_node_locator(
    buf: &[u8],
    p: usize,
    end: usize,
    full: String,
    short: String,
    class: String,
) -> Result<CellObject, ArtError> {
    let len = end - p;
    if len != 32 {
        return Err(ArtError::BadPlacement {
            offset: p,
            why: format!("WebNodeLocator payload {len} B, want 32"),
        });
    }
    let pos = f32x3(buf, p)?;
    let quat_xyzw = f32x4(buf, p + 12)?;
    let id = u32_at(buf, p + 28, end)?;
    let n = quat_norm(quat_xyzw);
    if (n - 1.0).abs() > 0.01 {
        return Err(ArtError::BadPlacement {
            offset: p + 12,
            why: format!("WebNodeLocator quaternion norm {n:.4} is not unit"),
        });
    }
    if let Some(name_id) = trailing_number(&short) {
        if name_id != id {
            return Err(ArtError::BadPlacement {
                offset: p + 28,
                why: format!("WebNodeLocator id {id} != short-name suffix {name_id}"),
            });
        }
    }
    Ok(CellObject {
        full_name: full,
        short_name: short,
        class,
        variant: ObjectVariant::WebNodeLocator,
        placement: Some(Placement { pos, quat_xyzw }),
        drawable: None,
        scale: None,
        state: None,
        payload: Some(ObjectPayload::WebLocator(WebNodeLocator {
            pos,
            quat_xyzw,
            id,
        })),
    })
}

/// Quaternion norm |q| = sqrt(x²+y²+z²+w²).
fn quat_norm(q: [f32; 4]) -> f32 {
    (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt()
}

/// The number formed by the TRAILING decimal digits of `s`
/// (`woi_loc_47` → Some(47), `woi_loc` → None).
fn trailing_number(s: &str) -> Option<u32> {
    let digits = s.trim_end().bytes().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>();
    if digits.is_empty() {
        return None;
    }
    let mut v: u32 = 0;
    for &c in digits.iter().rev() {
        v = v.checked_mul(10)?.checked_add((c - b'0') as u32)?;
    }
    Some(v)
}

/// Variant 2: raw placement + name + scale + state after "META".
/// `mp` points just PAST the META marker: `[pos 3f][quat 4f]` start there.
fn decode_binary(
    buf: &[u8],
    mp: usize,
    end: usize,
    full: String,
    short: String,
    class: String,
) -> Result<CellObject, ArtError> {
    if mp + 28 > end {
        return Err(ArtError::Truncated { offset: mp, need: 28, got: end - mp });
    }
    let pos = f32x3(buf, mp)?;
    let quat_xyzw = f32x4(buf, mp + 12)?;
    let mut p = mp + 28;
    // drawable pascal: corpus max 48 bytes; empty (len 0) is valid
    if p >= end || buf[p] as usize > 48 {
        return Err(ArtError::BadPascal { offset: p, len: if p < end { buf[p] as usize } else { 0 } });
    }
    let drawable_raw = read_pascal_trimmed(buf, &mut p, end)?;
    if p + 12 > end {
        return Err(ArtError::Truncated { offset: p, need: 12, got: end - p });
    }
    let scale = f32x3(buf, p)?;
    p += 12;
    if p + 4 > end {
        return Err(ArtError::Truncated { offset: p, need: 4, got: end - p });
    }
    let sl = u32_at(buf, p, end)? as usize;
    if sl > 64 || p + 4 + sl > end {
        return Err(ArtError::BadPlacement {
            offset: p,
            why: format!("state length {sl} past end"),
        });
    }
    let state = String::from_utf8_lossy(&buf[p + 4..p + 4 + sl]).to_string();
    let state = Some(state.trim_end_matches('\u{0}').to_string()).filter(|s| !s.is_empty());
    Ok(CellObject {
        full_name: full,
        short_name: short,
        class,
        variant: ObjectVariant::Binary,
        placement: Some(Placement { pos, quat_xyzw }),
        drawable: (!drawable_raw.is_empty()).then_some(drawable_raw),
        scale: Some(scale),
        state,
        payload: None,
    })
}

/// Variant 1: "META" + [u32 count][count × prop].  Captures
/// transform/scale/drawableName; advances by the proven element stride
/// per type and stops cleanly at the behaviour tail.
fn decode_prop_table(
    buf: &[u8],
    start: usize,
    end: usize,
    full: String,
    short: String,
    class: String,
) -> Result<CellObject, ArtError> {
    let mut placement: Option<Placement> = None;
    let mut scale: Option<[f32; 3]> = None;
    let mut drawable: Option<String> = None;
    let mut p = start;
    loop {
        // next prop: [u32 keylen][key][u32 typelen][type][u16 n][elements]
        let Some((key, ty, n, elem_start)) = read_prop_head(buf, p, end) else {
            break;
        };
        match (key.as_str(), ty.as_str()) {
            ("transform", "MatrixAttribute") if n >= 1 => {
                if elem_start + 28 > end {
                    return Err(ArtError::Truncated {
                        offset: elem_start,
                        need: 28,
                        got: end - elem_start,
                    });
                }
                let pos = f32x3(buf, elem_start)?;
                let quat_xyzw = f32x4(buf, elem_start + 12)?;
                placement = Some(Placement { pos, quat_xyzw });
            }
            ("scale", "VectorAttribute") if n >= 1 => {
                if elem_start + 12 > end {
                    return Err(ArtError::Truncated {
                        offset: elem_start,
                        need: 12,
                        got: end - elem_start,
                    });
                }
                scale = Some(f32x3(buf, elem_start)?);
            }
            ("drawableName", "NameAttribute") if n >= 1 => {
                if let Some(nm) = read_name_element(buf, elem_start, end) {
                    drawable = (!nm.is_empty()).then_some(nm);
                }
            }
            _ => {}
        }
        // advance past [u16 n][n × element] using the PROVEN strides;
        // MatrixAttribute also accepts the 10-byte constant tail form.
        let stride = match ty.as_str() {
            "MatrixAttribute" => 28,
            "VectorAttribute" => 12,
            "BoolAttribute" => 4,
            "IntAttribute" | "FloatAttribute" => 4,
            _ => 0,
        };
        let mut next = None;
        if stride > 0 {
            for delta in [n as usize * stride, n as usize * stride + 10] {
                let np = elem_start + delta;
                if np <= end && read_prop_head(buf, np, end).is_some() {
                    next = Some(np);
                    break;
                }
            }
        } else if ty == "NameAttribute" {
            // element = [u32 strlen][chars]
            if let Some(sl) = u32_at(buf, elem_start, end).ok() {
                let np = elem_start + 4 + sl as usize;
                if np <= end && read_prop_head(buf, np, end).is_some() {
                    next = Some(np);
                }
            }
        }
        match next {
            Some(np) => p = np,
            None => break,
        }
    }
    Ok(CellObject {
        full_name: full,
        short_name: short,
        class,
        variant: ObjectVariant::PropTable,
        placement,
        drawable,
        scale,
        state: None,
        payload: None,
    })
}

/// NameAttribute element: `[u32 strlen][chars]`, NUL-trimmed.
fn read_name_element(buf: &[u8], elem: usize, end: usize) -> Option<String> {
    let sl = u32_at(buf, elem, end).ok()? as usize;
    if sl > 256 || elem + 4 + sl > end {
        return None;
    }
    Some(
        String::from_utf8_lossy(&buf[elem + 4..elem + 4 + sl])
            .trim_end_matches('\u{0}')
            .to_string(),
    )
}

/// Try to read `[u32 keylen][key][u32 typelen][type][u16 n]` at `p`;
/// returns (key, type, n, element start) or None.  Keys/types must be
/// printable, lengths bounded (96) — the behaviour list never matches.
fn read_prop_head(buf: &[u8], p: usize, end: usize) -> Option<(String, String, u16, usize)> {
    if p + 4 > end {
        return None;
    }
    let klen = u32_at(buf, p, end).ok()? as usize;
    if klen == 0 || klen > 96 || p + 4 + klen + 4 > end {
        return None;
    }
    let key = buf.get(p + 4..p + 4 + klen)?;
    if !key.iter().all(|c| (32..127).contains(c)) {
        return None;
    }
    let tp = p + 4 + klen;
    let tlen = u32_at(buf, tp, end).ok()? as usize;
    if tlen == 0 || tlen > 96 || tp + 4 + tlen + 2 > end {
        return None;
    }
    let ty = buf.get(tp + 4..tp + 4 + tlen)?;
    if !ty.iter().all(|c| (32..127).contains(c)) {
        return None;
    }
    let ep = tp + 4 + tlen;
    let n = u16::from_le_bytes([*buf.get(ep)?, *buf.get(ep + 1)?]);
    Some((
        String::from_utf8_lossy(key).to_string(),
        String::from_utf8_lossy(ty).to_string(),
        n,
        ep + 2,
    ))
}

// ---------------------------------------------------------------------------
// Node transform (0x00124000 → 0x00010029)
// ---------------------------------------------------------------------------

/// Decode one node transform chunk: payload = 5 zero bytes, then exactly
/// one `0x00010029` child covering the rest (773/773 corpus).
pub fn decode_node_transform(
    buf: &[u8],
    offset: usize,
    size: usize,
) -> Result<NodeTransform, ArtError> {
    let end = offset + size;
    let p = offset + 12;
    if p + 5 + 12 > end {
        return Err(ArtError::Truncated { offset: p, need: 17, got: end - p });
    }
    if buf[p..p + 5] != [0u8; 5] {
        return Err(ArtError::BadPlacement {
            offset: p,
            why: "node transform payload does not start with 5 zero bytes".to_string(),
        });
    }
    let c = p + 5;
    let (ty, _uid, csize) = chunk_prelude(buf, c, end)?;
    if ty != NODE_TRANSFORM_CHILD {
        return Err(ArtError::BadPlacement {
            offset: c,
            why: format!("child is {ty:08x}, want 00010029"),
        });
    }
    if c + csize != end {
        return Err(ArtError::NotExactCover { offset: c, end });
    }
    let d = c + 12;
    if d + 4 + 48 > end {
        return Err(ArtError::Truncated { offset: d, need: 52, got: end - d });
    }
    let ver = u32_at(buf, d, end)?;
    if ver != 0 {
        return Err(ArtError::UnexpectedVersion { offset: d, got: ver, want: 0 });
    }
    let pos = f32x3(buf, d + 4)?;
    let quat_wxyz = f32x4(buf, d + 16)?;
    let unknown = [f32_at(buf, d + 32)?, f32_at(buf, d + 36)?];
    let extents = f32x3(buf, d + 40)?;
    Ok(NodeTransform { pos, quat_wxyz, unknown, extents })
}

// ---------------------------------------------------------------------------
// World index scan
// ---------------------------------------------------------------------------

/// Cheaply scan one decompressed member into a [`WorldIndexEntry`].
///
/// Decodes every GameObject and node transform (names + placements
/// only — no mesh data); Shape chunks contribute just their name.  A
/// GameObject decode failure increments `object_errors` (first 4
/// messages land in `error_notes`), never aborts the scan.
pub fn scan_world_member(hash: u32, p3d: &[u8]) -> WorldIndexEntry {
    scan_world_member_opts(hash, p3d, ScanOptions::default())
}

/// [`scan_world_member`] with control over full-object retention —
/// see [`ScanOptions::retain_objects`] (block 20).
pub fn scan_world_member_opts(hash: u32, p3d: &[u8], opts: ScanOptions) -> WorldIndexEntry {
    let mut e = WorldIndexEntry {
        hash,
        decompressed_len: p3d.len() as u32,
        ..Default::default()
    };
    if p3d.len() <= 13 {
        e.kind = MemberKind::Empty;
        return e;
    }
    let Some(top) = top_level_chunks(p3d) else {
        e.kind = MemberKind::Other;
        return e;
    };
    for chunk in &top {
        match chunk.chunk_type {
            CHUNK_TYPE_GAME_OBJECT => {
                e.objects += 1;
                match decode_game_object(p3d, chunk.offset, chunk.size) {
                    Ok(obj) => {
                        // Placements feed the world bbox: the binary
                        // spawner variant and the block-41 AEV/WNL
                        // records (both carry the real [pos][quat]
                        // head).  Prop-table (NIS-stage) objects are
                        // frequently stage-local, not world-space
                        // (observed (0.18, -0.15, -4.38) for a real NIS
                        // prop), and LightDecalInfo has no placement.
                        if matches!(
                            obj.variant,
                            ObjectVariant::Binary
                                | ObjectVariant::AmbientExclusionVolume
                                | ObjectVariant::WebNodeLocator
                        ) {
                            if let Some(pl) = &obj.placement {
                                e.note_pos(pl.pos);
                            }
                        }
                        if let Some(id) = extract_cell_id(&obj.full_name) {
                            e.note_cell(id);
                        }
                        if let Some(d) = &obj.drawable {
                            note_unique(&mut e.drawables, 8, d);
                        }
                        if opts.retain_objects {
                            e.retained_objects.push(obj);
                        }
                    }
                    Err(err) => {
                        e.object_errors += 1;
                        if e.error_notes.len() < 4 {
                            e.error_notes.push(format!("object: {err}"));
                        }
                    }
                }
            }
            CHUNK_TYPE_SHAPE => {
                e.shapes += 1;
                if let Some(name) = shape_name(p3d, chunk) {
                    if let Some(id) = extract_cell_id(&name) {
                        e.note_cell(id);
                    }
                    note_unique(&mut e.shape_names, 4, &name);
                    if opts.retain_shape_names {
                        e.shape_names_all.push(name);
                    }
                }
            }
            CHUNK_TYPE_NODE_TRANSFORM => {
                e.node_transforms += 1;
                match decode_node_transform(p3d, chunk.offset, chunk.size) {
                    Ok(nt) => {
                        e.note_pos(nt.pos);
                        if opts.retain_node_transforms {
                            e.node_transform_list.push(nt);
                        }
                    }
                    Err(err) => {
                        if e.error_notes.len() < 4 {
                            e.error_notes.push(format!("node: {err}"));
                        }
                    }
                }
            }
            CHUNK_TYPE_COLLISION => e.collisions += 1,
            CHUNK_TYPE_TEXTURE => e.textures += 1,
            CHUNK_TYPE_SHADER => e.shaders += 1,
            CHUNK_TYPE_MATERIAL => e.materials += 1,
            _ => {}
        }
    }
    e.kind = if e.objects > 0 {
        MemberKind::Objects
    } else if e.shapes > 0 || e.collisions > 0 {
        MemberKind::Geometry
    } else {
        MemberKind::Other
    };
    e
}

/// The u8-pascal name at the start of a Shape chunk (NUL-trimmed).
fn shape_name(p3d: &[u8], chunk: &ChildChunk) -> Option<String> {
    let start = chunk.payload();
    let n = *p3d.get(start)? as usize;
    if n == 0 || start + 1 + n > p3d.len() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&p3d[start + 1..start + 1 + n])
            .trim_end_matches('\u{0}')
            .to_string(),
    )
}

/// One cell's real spatial layout (from [`WorldIndex::cell_layout`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellLayout {
    /// The `Cell_NN` id.
    pub cell: u32,
    /// Union bbox [xmin, ymin, zmin, xmax, ymax, zmax].
    pub bbox: [f32; 6],
    /// How many members contributed placements.
    pub members: u32,
}

impl CellLayout {
    /// bbox center.
    pub fn center(&self) -> [f32; 3] {
        [
            (self.bbox[0] + self.bbox[3]) * 0.5,
            (self.bbox[1] + self.bbox[4]) * 0.5,
            (self.bbox[2] + self.bbox[5]) * 0.5,
        ]
    }

    /// Half-diagonal of the bbox (a lower-bound load radius).
    pub fn half_diagonal(&self) -> f32 {
        let dx = self.bbox[3] - self.bbox[0];
        let dy = self.bbox[4] - self.bbox[1];
        let dz = self.bbox[5] - self.bbox[2];
        (dx * dx + dy * dy + dz * dz).sqrt() * 0.5
    }
}

/// The streamed-world index over one archive's members.
#[derive(Debug, Clone, Default)]
pub struct WorldIndex {
    /// One entry per scanned member.
    pub entries: Vec<WorldIndexEntry>,
}

impl WorldIndex {
    /// Distinct cell ids across all members.
    pub fn cells(&self) -> std::collections::BTreeSet<u32> {
        self.entries
            .iter()
            .flat_map(|e| e.cells.iter().copied())
            .collect()
    }

    /// Total placed objects / shapes across members.
    pub fn totals(&self) -> (u32, u32) {
        (
            self.entries.iter().map(|e| e.objects).sum(),
            self.entries.iter().map(|e| e.shapes).sum(),
        )
    }

    /// The real per-cell spatial layout: union bbox of every member
    /// bbox that carries the cell id.  This is the data the synthetic
    /// M11 streaming grid was waiting for — cell geometry derived from
    /// REAL spawner/node placements (a lower bound: spawners under-cover
    /// a block, so treat the radius as a hint, not a hard boundary).
    pub fn cell_layout(&self) -> std::collections::BTreeMap<u32, CellLayout> {
        let mut raw: std::collections::BTreeMap<u32, ([f32; 6], u32)> = Default::default();
        for e in &self.entries {
            let Some(b) = e.bbox else { continue };
            for &c in &e.cells {
                let slot = raw.entry(c).or_insert((b, 0));
                let u = &mut slot.0;
                u[0] = u[0].min(b[0]);
                u[1] = u[1].min(b[1]);
                u[2] = u[2].min(b[2]);
                u[3] = u[3].max(b[3]);
                u[4] = u[4].max(b[4]);
                u[5] = u[5].max(b[5]);
                slot.1 += 1;
            }
        }
        raw.into_iter()
            .map(|(cell, (bbox, members))| (cell, CellLayout { cell, bbox, members }))
            .collect()
    }

    /// The spawn set (M25b, block 20): every RETAINED object that is
    /// (a) binary-variant and (b) carries a placement, flattened out of
    /// the members.  This is the real placed-object database —
    /// CloudSpawner / ParkedCarSpawner / EffectsPropSpawner / …
    /// (71,178 binary corpus-wide).
    ///
    /// Prop-table objects are EXCLUDED on purpose: their placements are
    /// frequently stage-local NIS coordinates, not world-space (PROVEN
    /// on the real Cell_141 member: `GameObject(1):NIS_BoxStack1` sits
    /// at (0.1848, -0.15, -4.3822) — a stage, not Manhattan).
    ///
    /// Requires the members to have been scanned with
    /// [`ScanOptions::retain_objects`] — otherwise the result is empty.
    pub fn spawn_placements(&self) -> Vec<PlacedObject> {
        let mut out = Vec::new();
        for e in &self.entries {
            for obj in &e.retained_objects {
                if obj.variant == ObjectVariant::Binary {
                    if obj.placement.is_some() {
                        out.push(PlacedObject {
                            member_hash: e.hash,
                            cells: e.cells.clone(),
                            object: obj.clone(),
                        });
                    }
                }
            }
        }
        out
    }

    /// (retained, spawnable) object counts — the honest boot-log pair
    /// ([`WorldIndex::spawn_placements`] length is the second half).
    pub fn retained_totals(&self) -> (usize, usize) {
        let retained: usize = self.entries.iter().map(|e| e.retained_objects.len()).sum();
        (retained, self.spawn_placements().len())
    }

    /// Members whose bbox is nearest `cam` first (no-bbox members last,
    /// stable among equals).
    pub fn members_near(&self, cam: [f32; 3]) -> Vec<&WorldIndexEntry> {
        self.members_near_indices(cam)
            .into_iter()
            .map(|i| &self.entries[i])
            .collect()
    }

    /// [`WorldIndex::members_near`] as global entry indices — the form
    /// the batch→member provenance (block 20 `DrawSelector`) consumes.
    pub fn members_near_indices(&self, cam: [f32; 3]) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.entries.len()).collect();
        v.sort_by_key(|&i| {
            let d = self.entries[i].distance_from(cam);
            (d.is_infinite(), d.to_bits())
        });
        v
    }

    /// (block 22) GameObject → drawable → Shape pairing over the
    /// retained records.
    ///
    /// Evidence (real cells.rcf members, block 22 probe — 40 members,
    /// ~2,600 binary records, 9 cell pairs checked both directions):
    ///
    /// * BINARY spawner records carry a `drawable` pascal
    ///   (`tree003`, `streetlight001`, `barrierPlasticOS002`, …) and a
    ///   `state` (usually `initial`).  The same cell's geometry member
    ///   holds Shapes named `<drawable><State>Shape`
    ///   (`barrierPlasticOS002InitialShape`, `…DamagedShape`) or,
    ///   stateless, `<drawable>Shape` (`garbageGroundShape`).  This is
    ///   PROVEN as the naming convention (e.g. cell 47: 42 records
    ///   matched `D+ucfirst(state)+Shape`; cell 74: 44; cell 120: 42) —
    ///   NOT a completeness proof: most record drawables reference
    ///   shapes that live elsewhere (vegetation/instanced art), which
    ///   is exactly why the pairing reports both sides honestly.
    /// * PROP-TABLE records with a `drawableName` prop
    ///   (`GameObject(1):billBoard001`) carry LOCAL transforms and pair
    ///   by the same rule against their cell's shapes
    ///   (`billBoard001InitialShape` — 3/3 in cell 82).  Their world
    ///   placement composes with the member's node transforms — the
    ///   record→node-transform mapping is UNKNOWN, so these pairs are
    ///   reported with `variant == PropTable` and the draw path must
    ///   NOT place them at their local transform.
    ///
    /// Matching is cell-scoped: a record only pairs with a geometry
    /// member that shares one of the record member's `Cell_NN` ids.
    pub fn pair_drawables(&self, max: usize) -> PairReport {
        // cell -> geometry member indices (with shape names)
        let mut geo_by_cell: std::collections::BTreeMap<u32, Vec<usize>> = Default::default();
        for (gi, e) in self.entries.iter().enumerate() {
            if e.kind != MemberKind::Geometry {
                continue;
            }
            for &c in &e.cells {
                geo_by_cell.entry(c).or_default().push(gi);
            }
        }
        let mut rep = PairReport::default();
        'outer: for (oi, e) in self.entries.iter().enumerate() {
            'records: for obj in &e.retained_objects {
                let Some(drawable) = obj.drawable.as_deref() else {
                    continue;
                };
                if drawable.is_empty() {
                    continue;
                }
                rep.records_with_drawable += 1;
                let Some(pl) = &obj.placement else {
                    rep.unmatched += 1;
                    continue;
                };
                // Naming candidates, most specific first.  Real states
                // include comma lists (`initial,010` — cell 82 record
                // Cell_82signHorizontal001_2); each component tries its
                // own shape (`…InitialShape` from the first component).
                // Prop-table records carry no state prop, yet their real
                // shapes are the `…InitialShape` ones (cell 82:
                // billBoard001/billBoard005/signHorizontal001 → 3/3), so
                // the implicit default state `initial` is tried for them
                // (LIKELY — the corpus never spells it out).
                let mut state = obj
                    .state
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .trim_end_matches('\u{0}')
                    .to_string();
                if state.is_empty() && obj.variant == ObjectVariant::PropTable {
                    state.push_str("initial");
                }
                let mut candidates: Vec<(PairRule, String)> =
                    vec![(PairRule::DrawableBare, format!("{drawable}Shape"))];
                for part in state.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                    let mut st = part.to_string();
                    if let Some(first) = st.chars().next() {
                        let up = first.to_uppercase().next().unwrap_or(first);
                        st.replace_range(..first.len_utf8(), &up.to_string());
                    }
                    candidates.insert(
                        0,
                        (PairRule::DrawableState, format!("{drawable}{st}Shape")),
                    );
                }
                for &cell in &e.cells {
                    let Some(geos) = geo_by_cell.get(&cell) else {
                        continue;
                    };
                    for &gi in geos {
                        let names = self.entries[gi].pairing_shape_names();
                        if names.is_empty() {
                            continue;
                        }
                        for (rule, shape) in &candidates {
                            if shape.is_empty() || !names.iter().any(|n| n == shape) {
                                continue;
                            }
                            if rep.pairs.len() >= max {
                                rep.capped = true;
                                break 'outer;
                            }
                            rep.pairs.push(PairedDrawable {
                                cell,
                                shape: shape.clone(),
                                rule: *rule,
                                object_member: oi,
                                geometry_member: gi,
                                placement: pl.clone(),
                                scale: obj.scale,
                                variant: obj.variant.clone(),
                                class: obj.class.clone(),
                                drawable: drawable.to_string(),
                            });
                            // ONE pair per record — the next RECORD, not
                            // the next member (the block-22 loop-label
                            // bug: 'continue' on the outer loop skipped
                            // every remaining record after a match).
                            continue 'records;
                        }
                    }
                }
                rep.unmatched += 1;
            }
        }
        rep
    }
}

/// (block 22) Which naming rule produced a pair — honest provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairRule {
    /// `<drawable><ucfirst(state)>Shape` (e.g. `…OS002InitialShape`).
    DrawableState,
    /// `<drawable>Shape` (stateless shapes, e.g. `garbageGroundShape`).
    DrawableBare,
}

/// (block 22) One paired record↔shape instance.
#[derive(Debug, Clone, PartialEq)]
pub struct PairedDrawable {
    /// The shared `Cell_NN` id the pairing matched on.
    pub cell: u32,
    /// The geometry member's shape name.
    pub shape: String,
    /// Which naming rule matched.
    pub rule: PairRule,
    /// Global index of the member carrying the record.
    pub object_member: usize,
    /// Global index of the geometry member carrying the shape.
    pub geometry_member: usize,
    /// The record's placement.  BINARY records are WORLD-space (draw
    /// directly).  PROP-TABLE records are mostly LOCAL stage/template
    /// definitions (block 23: the same local transforms repeat verbatim
    /// across different cells' members) whose drawables ALSO have binary
    /// CloudSpawner records carrying the real world placement — so the
    /// draw path uses the binary pair and skips the prop-table one
    /// (fail-closed; composing them with the member's node transforms is
    /// UNKNOWN and unnecessary).  Prop-table EXCEPTION: point-light
    /// records carry WORLD transforms, but they are lights, not
    /// drawable placements.
    pub placement: Placement,
    /// The record's scale vector (None when absent).
    pub scale: Option<[f32; 3]>,
    /// Binary vs prop-table record.
    pub variant: ObjectVariant,
    /// The record's class string (census/diagnostics).
    pub class: String,
    /// The raw drawable name.
    pub drawable: String,
}

/// (block 22) What [`WorldIndex::pair_drawables`] found.
#[derive(Debug, Clone, Default)]
pub struct PairReport {
    /// The matched pairs (capped at the caller's `max`).
    pub pairs: Vec<PairedDrawable>,
    /// Retained records that carry a non-empty drawable.
    pub records_with_drawable: u32,
    /// Records with a drawable that matched no shape in any same-cell
    /// geometry member (their shapes live elsewhere — vegetation,
    /// instanced art, other members).
    pub unmatched: u32,
    /// True when `max` stopped the pairing early.
    pub capped: bool,
}

impl PairReport {
    /// Pairs whose placement the draw path may use directly (binary
    /// records — world-space).
    pub fn placeable(&self) -> impl Iterator<Item = &PairedDrawable> {
        self.pairs.iter().filter(|p| p.variant == ObjectVariant::Binary)
    }
}

// ---------------------------------------------------------------------------
// small readers (offsets in every error, like geometry.rs)
// ---------------------------------------------------------------------------

fn u32_at(buf: &[u8], off: usize, end: usize) -> Result<u32, ArtError> {
    if off + 4 > end {
        return Err(ArtError::Truncated { offset: off, need: 4, got: end - off });
    }
    Ok(u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]))
}

fn f32_at(buf: &[u8], off: usize) -> Result<f32, ArtError> {
    if off + 4 > buf.len() {
        return Err(ArtError::Truncated { offset: off, need: 4, got: buf.len() - off });
    }
    Ok(f32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]))
}

fn f32x3(buf: &[u8], off: usize) -> Result<[f32; 3], ArtError> {
    Ok([f32_at(buf, off)?, f32_at(buf, off + 4)?, f32_at(buf, off + 8)?])
}

fn f32x4(buf: &[u8], off: usize) -> Result<[f32; 4], ArtError> {
    Ok([
        f32_at(buf, off)?,
        f32_at(buf, off + 4)?,
        f32_at(buf, off + 8)?,
        f32_at(buf, off + 12)?,
    ])
}

/// Read a u8-pascal string at `*p`, NUL-trimmed; advances `*p`.
fn read_pascal_trimmed(buf: &[u8], p: &mut usize, end: usize) -> Result<String, ArtError> {
    let off = *p;
    if off + 1 > end {
        return Err(ArtError::Truncated { offset: off, need: 1, got: end - off });
    }
    let n = buf[off] as usize;
    if off + 1 + n > end {
        return Err(ArtError::BadPascal { offset: off, len: n });
    }
    *p = off + 1 + n;
    Ok(
        String::from_utf8_lossy(&buf[off + 1..off + 1 + n])
            .trim_end_matches('\u{0}')
            .to_string(),
    )
}

fn chunk_prelude(buf: &[u8], off: usize, end: usize) -> Result<(u32, u32, usize), ArtError> {
    if off + 12 > end {
        return Err(ArtError::Truncated { offset: off, need: 12, got: end - off });
    }
    let ty = u32_at(buf, off, end)?;
    let uid = u32_at(buf, off + 4, end)?;
    let size = u32_at(buf, off + 8, end)? as usize;
    if size < 12 || off + size > end {
        return Err(ArtError::BadChunkSize { chunk_type: ty, offset: off, size, end });
    }
    Ok((ty, uid, size))
}

// ---------------------------------------------------------------------------
// Tests — against REAL cells.rcf fixtures
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

    fn chunk_of(p3d: &[u8], want: u32) -> (usize, usize) {
        top_level_chunks(p3d)
            .expect("fixture is a p3d")
            .into_iter()
            .find(|c| c.chunk_type == want)
            .map(|c| (c.offset, c.size))
            .unwrap_or_else(|| panic!("no chunk {want:08x} in fixture"))
    }

    /// Variant-1 (prop table) GameObject from the real NIS member:
    /// names, placement, scale, drawable — all corpus-proven values.
    #[test]
    fn decodes_v1_prop_table_game_object() {
        let buf = fixture("cells_go_v1_nisboxstack.p3d");
        let (off, size) = chunk_of(&buf, CHUNK_TYPE_GAME_OBJECT);
        let obj = decode_game_object(&buf, off, size).expect("v1 decode");
        assert_eq!(obj.full_name, "GameObject(1):NIS_BoxStack1");
        assert_eq!(obj.short_name, "NIS_BoxStack1");
        assert_eq!(obj.class, "GameObject");
        assert_eq!(obj.variant, ObjectVariant::PropTable);
        let pl = obj.placement.expect("v1 transform present");
        assert!((pl.pos[0] - 0.1848).abs() < 1e-3);
        assert!((pl.pos[1] - (-0.15)).abs() < 1e-3);
        assert!((pl.pos[2] - (-4.3822)).abs() < 1e-3);
        assert_eq!(pl.quat_xyzw, [0.0, 0.0, 0.0, -1.0]);
        assert!(pl.is_unit_quat());
        assert_eq!(obj.scale, Some([1.0, 1.0, 1.0]));
        assert_eq!(obj.drawable.as_deref(), Some("NIS_BoxStack1"));
        assert_eq!(obj.state, None);
    }

    /// Variant-2 (binary) GameObject: the streamed-world spawner layout.
    #[test]
    fn decodes_v2_binary_game_object() {
        let buf = fixture("cells_go_v2_cloudspawner.p3d");
        let (off, size) = chunk_of(&buf, CHUNK_TYPE_GAME_OBJECT);
        let obj = decode_game_object(&buf, off, size).expect("v2 decode");
        assert_eq!(obj.full_name, "proto::CloudSpawner(1):Cell_65Collectible004_38");
        assert_eq!(obj.short_name, "Cell_65Collectible004_38");
        assert_eq!(obj.class, "proto::CloudSpawner");
        assert_eq!(obj.variant, ObjectVariant::Binary);
        let pl = obj.placement.expect("v2 placement");
        assert!((pl.pos[0] - (-244.106)).abs() < 1e-2);
        assert!((pl.pos[1] - 15.697).abs() < 1e-2);
        assert!((pl.pos[2] - (-62.826)).abs() < 1e-2);
        assert_eq!(pl.quat_xyzw, [0.0, 0.0, 0.0, -1.0]);
        assert!(pl.is_unit_quat());
        assert_eq!(obj.drawable.as_deref(), Some("Collectible004"));
        assert_eq!(obj.scale, Some([1.0, 1.0, 1.0]));
        assert_eq!(obj.state.as_deref(), Some("initial"));
    }

    /// Geometry-member node transform: pos, w-first quaternion, extents.
    #[test]
    fn decodes_node_transform() {
        let buf = fixture("cells_node_xform.p3d");
        let (off, size) = chunk_of(&buf, CHUNK_TYPE_NODE_TRANSFORM);
        let nt = decode_node_transform(&buf, off, size).expect("node decode");
        assert!((nt.pos[0] - 403.8565).abs() < 1e-3);
        assert!((nt.pos[1] - 14.9949).abs() < 1e-3);
        assert!((nt.pos[2] - (-114.0292)).abs() < 1e-3);
        assert_eq!(nt.quat_wxyz, [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(nt.unknown[0], 1.0);
        assert!(nt.unknown[1].abs() < 1e-6);
        assert!((nt.extents[0] - 47.8233).abs() < 1e-3);
        assert!((nt.extents[1] - 15.5288).abs() < 1e-3);
        assert!((nt.extents[2] - 23.9479).abs() < 1e-3);
    }

    /// The real object member: 62 GameObjects, kind Objects, the Cell_141
    /// id from the object names, bbox from the placements.
    #[test]
    fn scans_object_member() {
        let p3d = fixture("cells_member_objects_nis.p3d");
        let e = scan_world_member(0x0049_b319, &p3d);
        assert_eq!(e.hash, 0x0049_b319);
        assert_eq!(e.kind, MemberKind::Objects);
        assert_eq!(e.objects, 62);
        assert_eq!(e.shapes, 0);
        assert!(e.cells.contains(&141), "cells = {:?}", e.cells);
        let bbox = e.bbox.expect("v2 placements give a bbox");
        assert!(bbox[0] > 1600.0 && bbox[3] < 1750.0, "bbox {bbox:?}");
        assert!(bbox[1] > 0.0 && bbox[4] < 30.0);
        assert!(bbox[2] < -1800.0 && bbox[5] > -1850.0);
        assert_eq!(e.object_errors, 0, "{:?}", e.error_notes);
    }

    /// The real geometry member: 1 shape, kind Geometry, no placements.
    #[test]
    fn scans_geometry_member() {
        let p3d = fixture("cells_member_geometry_small.p3d");
        let e = scan_world_member(0x213a_bf76, &p3d);
        assert_eq!(e.kind, MemberKind::Geometry);
        assert_eq!(e.shapes, 1);
        assert_eq!(e.objects, 0);
        assert_eq!(e.shape_names, vec!["mergedDrawableRootCastShadow".to_string()]);
        assert_eq!(e.bbox, None);
    }

    /// The empty member (bare 12-byte header) classifies as Empty.
    #[test]
    fn scans_empty_member() {
        let p3d = fixture("cells_member_empty.p3d");
        let e = scan_world_member(0x010d_9ce1, &p3d);
        assert_eq!(e.kind, MemberKind::Empty);
        assert_eq!(e.decompressed_len, 12);
    }

    /// Cell id extraction rules.
    #[test]
    fn cell_id_extraction() {
        assert_eq!(extract_cell_id("Cell_67_light_decal_0"), Some(67));
        assert_eq!(extract_cell_id("proto::CloudSpawner(1):Cell_65Collectible004_38"), Some(65));
        assert_eq!(extract_cell_id("GameObject(1):NIS_BoxStack1"), None);
        assert_eq!(extract_cell_id("mergedDrawableRootCastShadow"), None);
        assert_eq!(extract_cell_id("Cell_2x"), Some(2));
        assert_eq!(extract_cell_id("Cell_"), None);
    }

    /// Fail-closed: a truncated binary GameObject is an error, and a
    /// non-META blob reports ExpectedMeta.
    #[test]
    fn world_decoders_fail_closed() {
        let buf = fixture("cells_go_v2_cloudspawner.p3d");
        let (off, size) = chunk_of(&buf, CHUNK_TYPE_GAME_OBJECT);
        // cut into the pos/quat region — the decoder must refuse
        let cut = &buf[off..off + 60];
        assert!(decode_game_object(cut, 0, cut.len()).is_err());
        // corrupt the META marker
        let mut bad = buf.clone();
        let meta = bad[off..off + size]
            .windows(4)
            .position(|w| w == b"META")
            .expect("fixture carries META")
            + off;
        bad[meta] = b'X';
        assert!(matches!(
            decode_game_object(&bad, off, size),
            Err(ArtError::ExpectedMeta { .. })
        ));
        // a truncated node transform errors too
        let nbuf = fixture("cells_node_xform.p3d");
        assert!(decode_node_transform(&nbuf, 0, 40).is_err());
    }

    /// The real per-cell layout: cell 141's bbox is the union of the
    /// members that carry it (here: the one object member's v2 bbox).
    #[test]
    fn cell_layout_from_real_member() {
        let p3d = fixture("cells_member_objects_nis.p3d");
        let idx = WorldIndex { entries: vec![scan_world_member(0x0049_b319, &p3d)] };
        let layout = idx.cell_layout();
        let cl = layout.get(&141).expect("cell 141 present");
        assert_eq!(cl.members, 1);
        let member = &idx.entries[0];
        assert_eq!(cl.bbox, member.bbox.unwrap());
        let c = cl.center();
        assert!(c[0] > 1600.0 && c[2] < -1800.0, "center {c:?}");
        assert!(cl.half_diagonal() > 0.0);
        assert_eq!(layout.len(), 1);
    }

    /// WorldIndex ordering: nearest bbox center first, no-bbox last.
    #[test]
    fn members_near_orders_by_distance() {
        let mut a = WorldIndexEntry::default();
        a.bbox = Some([10.0, 0.0, 0.0, 20.0, 10.0, 10.0]); // center (15,5,5)
        let mut b = WorldIndexEntry::default();
        b.bbox = Some([100.0, 0.0, 0.0, 110.0, 10.0, 10.0]); // center (105,5,5)
        let c = WorldIndexEntry::default(); // no bbox
        let idx = WorldIndex { entries: vec![b.clone(), c.clone(), a.clone()] };
        let ordered = idx.members_near([12.0, 5.0, 5.0]);
        assert_eq!(ordered[0].bbox, a.bbox);
        assert_eq!(ordered[1].bbox, b.bbox);
        assert_eq!(ordered[2].bbox, None);
        // stable among equals
        let idx2 = WorldIndex { entries: vec![a.clone(), b.clone()] };
        let o2 = idx2.members_near([12.0, 5.0, 5.0]);
        assert_eq!(o2.len(), 2);
    }

    // -------------------------------------------------------------------
    // Block 20: optional full-object retention + spawn placements
    // -------------------------------------------------------------------

    /// Default scan retains NOTHING (the index pass stays lean); with
    /// `retain_objects` the full CloudSpawner comes through with every
    /// corpus-proven field intact.
    #[test]
    fn scan_opts_retain_real_cloudspawner() {
        let buf = fixture("cells_go_v2_cloudspawner.p3d");
        // default: not retained
        let e = scan_world_member(0x1, &buf);
        assert_eq!(e.objects, 1);
        assert!(e.retained_objects.is_empty());
        // retain: the whole object
        let e = scan_world_member_opts(
            0x1,
            &buf,
            ScanOptions { retain_objects: true, ..Default::default() },
        );
        assert_eq!(e.retained_objects.len(), 1);
        let o = &e.retained_objects[0];
        assert_eq!(o.class, "proto::CloudSpawner");
        assert_eq!(o.variant, ObjectVariant::Binary);
        assert_eq!(o.drawable.as_deref(), Some("Collectible004"));
        assert_eq!(o.state.as_deref(), Some("initial"));
        assert_eq!(o.scale, Some([1.0, 1.0, 1.0]));
        let pl = o.placement.expect("binary spawner is placed");
        assert_eq!(pl.pos, [-244.10567, 15.696654, -62.826157]);
        assert_eq!(pl.quat_xyzw, [0.0, 0.0, 0.0, -1.0]);
        assert!(pl.is_unit_quat());
        // and it flattens into the spawn set
        let idx = WorldIndex { entries: vec![e.clone()] };
        let spawns = idx.spawn_placements();
        assert_eq!(spawns.len(), 1);
        assert_eq!(spawns[0].member_hash, 0x1);
        assert_eq!(spawns[0].object.full_name, o.full_name);
        assert_eq!(idx.retained_totals(), (1, 1));
    }

    /// The REAL Cell_141 objects member (the mini cells.rcf's object
    /// member, scanned standalone): 62 objects, 43 binary
    /// CloudSpawners + 19 prop-table `GameObject`s.  The spawn set is
    /// EXACTLY the 43 binary ones; the prop-table placements are NIS
    /// stage-local (the corpus PROVES it: NIS_BoxStack1 at
    /// (0.1848, -0.15, -4.3822)) and MUST NOT reach world spawn.
    #[test]
    fn nis_member_spawn_set_is_binary_only() {
        let buf = fixture("cells_member_objects_nis.p3d");
        let e = scan_world_member_opts(
            0x49B319,
            &buf,
            ScanOptions { retain_objects: true, ..Default::default() },
        );
        assert_eq!(e.retained_objects.len(), 62);
        let binary = e
            .retained_objects
            .iter()
            .filter(|o| o.variant == ObjectVariant::Binary)
            .count();
        assert_eq!(binary, 43);
        // every binary object is a placed CloudSpawner with a real
        // morgue drawable
        for o in e.retained_objects.iter().filter(|o| o.variant == ObjectVariant::Binary) {
            assert_eq!(o.class, "proto::CloudSpawner");
            assert!(o.placement.is_some());
            assert!(o.drawable.is_some(), "binary spawners carry drawables");
        }
        // prop-table placements are stage-local: |pos| < 100 m (observed
        // max 11.08 — the stage is bigger than the origin suggests, but
        // never world-scale), while every binary spawner in this member
        // sits at world-scale distance (the morgue, ~(1687, 10, -1845)).
        for o in e.retained_objects.iter().filter(|o| o.variant == ObjectVariant::PropTable) {
            if let Some(pl) = &o.placement {
                let r = (pl.pos[0] * pl.pos[0] + pl.pos[1] * pl.pos[1] + pl.pos[2] * pl.pos[2]).sqrt();
                assert!(r < 100.0, "prop-table placement {r} not stage-local: {o:?}");
            }
        }
        for o in e.retained_objects.iter().filter(|o| o.variant == ObjectVariant::Binary) {
            let pl = o.placement.as_ref().unwrap();
            let r = (pl.pos[0] * pl.pos[0] + pl.pos[1] * pl.pos[1] + pl.pos[2] * pl.pos[2]).sqrt();
            assert!(r > 1000.0, "binary placement {r} unexpectedly stage-local: {o:?}");
        }
        // the spawn set excludes every prop-table object
        let idx = WorldIndex { entries: vec![e] };
        let spawns = idx.spawn_placements();
        assert_eq!(spawns.len(), 43);
        assert!(spawns.iter().all(|p| p.object.variant == ObjectVariant::Binary));
        assert_eq!(idx.retained_totals(), (62, 43));
    }

    // -- block 22: retention flags + GameObject -> drawable pairing ------

    /// New ScanOptions flags default OFF (the cheap index pass stays
    /// lean) and behave on real data: `retain_shape_names` keeps every
    /// Shape name of the real Cell_82 rebuilt member (capped diagnostic
    /// list stays at 4), `retain_node_transforms` keeps the member's
    /// real node transform with its PROVEN values
    /// (pos (250.94193, 27.01727, 235.89066), quat wxyz identity,
    /// extents (48.536133, 25.642342, 23.279104)).
    #[test]
    fn scan_opts_retain_shape_names_and_node_transforms() {
        let buf = fixture("cells_pairing_shapes.p3d");
        // default: nothing retained
        let e = scan_world_member(0x2268DE7E, &buf);
        assert_eq!(e.kind, MemberKind::Geometry);
        assert_eq!(e.shapes, 3);
        assert_eq!(e.node_transforms, 1);
        assert!(e.shape_names_all.is_empty());
        assert!(e.node_transform_list.is_empty());
        assert_eq!(e.shape_names.len(), 4.min(e.shapes as usize));
        // full retention
        let e = scan_world_member_opts(
            0x2268DE7E,
            &buf,
            ScanOptions {
                retain_shape_names: true,
                retain_node_transforms: true,
                ..Default::default()
            },
        );
        assert_eq!(e.shape_names_all.len(), 3);
        assert!(e
            .shape_names_all
            .iter()
            .any(|n| n == "billBoard001InitialShape"));
        assert!(e
            .shape_names_all
            .iter()
            .any(|n| n == "signHorizontal001InitialShape"));
        assert!(e.shape_names_all.iter().any(|n| n == "Cell_82_light_decal_0"));
        assert_eq!(e.node_transform_list.len(), 1);
        let nt = &e.node_transform_list[0];
        assert!((nt.pos[0] - 250.94193).abs() < 1e-5);
        assert!((nt.pos[1] - 27.01727).abs() < 1e-5);
        assert!((nt.pos[2] - 235.89066).abs() < 1e-5);
        assert_eq!(nt.quat_wxyz, [1.0, 0.0, 0.0, 0.0]);
        assert!((nt.extents[0] - 48.536133).abs() < 1e-5);
        assert!((nt.extents[1] - 25.642342).abs() < 1e-5);
        assert!((nt.extents[2] - 23.279104).abs() < 1e-5);
        // the Cell_82 id comes from the light-decal shape name
        assert!(e.cells.contains(&82));
    }

    /// THE pairing test, on the real Cell_82 members (object member
    /// 0x51A2CA3B + the rebuilt shapes member of 0x2268DE7E, packaged
    /// as cells_pairing.rcf):
    /// * the binary `signHorizontal001` record (state `initial,010`,
    ///   world pos (199.99554, 27.7564, 331.50858)) pairs with
    ///   `signHorizontal001InitialShape` through the FIRST comma state
    ///   component;
    /// * the prop-table `billBoard001` record (drawableName prop,
    ///   LOCAL template transform (-0.45272, 0.0, 5.01676)) pairs with
    ///   `billBoard001InitialShape` and is NOT placeable (block 23:
    ///   it is a stage/template definition — the same drawable's WORLD
    ///   placement comes from its binary CloudSpawner record, asserted
    ///   below at (298.7417, 37.4367, 353.3841));
    /// * everything else honestly lands in `unmatched` (the other 500+
    ///   records reference shapes that live in other members).
    #[test]
    fn pairs_cell82_drawables_to_shapes() {
        let data = fixture("cells_pairing.rcf");
        let archive = prototype_rcf::RcfArchive::parse(&data).expect("pairing rcf parses");
        let mut entries = Vec::new();
        for entry in &archive.entries {
            let p3d = archive.payload_decompressed(entry).expect("member rz");
            entries.push(scan_world_member_opts(
                entry.name_hash,
                &p3d,
                ScanOptions {
                    retain_objects: true,
                    retain_shape_names: true,
                    retain_node_transforms: true,
                },
            ));
        }
        assert_eq!(entries.len(), 2);
        let obj_idx = entries
            .iter()
            .position(|e| e.kind == MemberKind::Objects)
            .expect("object member present");
        let geo_idx = entries
            .iter()
            .position(|e| e.kind == MemberKind::Geometry)
            .expect("geometry member present");
        assert_eq!(entries[obj_idx].hash, 0x51A2CA3B);
        assert_eq!(entries[geo_idx].hash, 0x2268DE7E);
        assert!(entries[obj_idx].retained_objects.len() >= 600);

        let idx = WorldIndex { entries };
        let rep = idx.pair_drawables(usize::MAX);
        // The corpus numbers (real member 0x51A2CA3B): 703 retained
        // records, 528 carry a drawable (525 binary + 3 prop-table
        // drawableName), 4 pair with this member's shapes (both
        // billBoard001 and signHorizontal001 exist as BOTH variants),
        // 524 reference shapes that live in other members/other systems.
        assert_eq!(rep.records_with_drawable, 528);
        assert_eq!(rep.pairs.len(), 4);
        assert_eq!(rep.unmatched, 524);
        let bill = rep
            .pairs
            .iter()
            .find(|p| p.shape == "billBoard001InitialShape" && p.variant == ObjectVariant::PropTable)
            .expect("billBoard001 prop-table pairs");
        assert_eq!(bill.variant, ObjectVariant::PropTable);
        assert_eq!(bill.rule, PairRule::DrawableState);
        assert_eq!(bill.cell, 82);
        assert_eq!(bill.object_member, obj_idx);
        assert_eq!(bill.geometry_member, geo_idx);
        assert_eq!(bill.drawable, "billBoard001");
        let pl = &bill.placement;
        assert!((pl.pos[0] - (-0.45272)).abs() < 1e-4);
        assert!(pl.pos[1].abs() < 1e-5);
        assert!((pl.pos[2] - 5.01676).abs() < 1e-4);
        assert_eq!(pl.quat_xyzw, [0.0, 0.0, 0.0, -1.0]);
        let sign = rep
            .pairs
            .iter()
            .find(|p| p.shape == "signHorizontal001InitialShape" && p.variant == ObjectVariant::Binary)
            .expect("signHorizontal001 binary pairs");
        assert_eq!(sign.variant, ObjectVariant::Binary);
        assert_eq!(sign.rule, PairRule::DrawableState);
        assert_eq!(sign.cell, 82);
        assert_eq!(sign.drawable, "signHorizontal001");
        let pl = &sign.placement;
        assert!((pl.pos[0] - 199.99554).abs() < 1e-3);
        assert!((pl.pos[1] - 27.7564).abs() < 1e-3);
        assert!((pl.pos[2] - 331.50858).abs() < 1e-3);
        assert!((pl.quat_xyzw[1] - 0.70711).abs() < 1e-4);
        assert!((pl.quat_xyzw[3] - (-0.70711)).abs() < 1e-4);
        assert!(sign.placement.is_unit_quat());
        // the BINARY billBoard001 record is the CloudSpawner world
        // placement of the same drawable the prop-table stage record
        // defines (block 23: (298.7417, 37.4367, 353.3841) from the real
        // member 0x51A2CA3B — the rooftop billboard south-east of the
        // member's third node transform)
        let bill_bin = rep
            .pairs
            .iter()
            .find(|p| p.shape == "billBoard001InitialShape" && p.variant == ObjectVariant::Binary)
            .expect("billBoard001 binary (CloudSpawner) pair");
        assert!((bill_bin.placement.pos[0] - 298.7417).abs() < 1e-3);
        assert!((bill_bin.placement.pos[1] - 37.4367).abs() < 1e-3);
        assert!((bill_bin.placement.pos[2] - 353.3841).abs() < 1e-3);
        assert!((bill_bin.placement.quat_xyzw[1] - 0.38268).abs() < 1e-4);
        assert!((bill_bin.placement.quat_xyzw[3] - (-0.92388)).abs() < 1e-4);
        assert_eq!(bill_bin.drawable, "billBoard001");
        assert!(bill_bin.placement.is_unit_quat());
        // prop-table pairs are NOT placeable; binary ones are
        assert!(rep.placeable().all(|p| p.variant == ObjectVariant::Binary));
        assert_eq!(rep.placeable().count(), 2);
        // cap stops early and says so
        let rep2 = idx.pair_drawables(1);
        assert_eq!(rep2.pairs.len(), 1);
        assert!(rep2.capped);
    }

    /// Block-23 corpus census of the REAL cell-82 object member
    /// (0x51A2CA3B): the binary variant serves the whole `proto::*`
    /// spawner family — same head (flag/hash/META), same
    /// [pos][quat][drawable][scale][state] payload; only the class
    /// string differs.  Every number below is measured on the committed
    /// member bytes.
    ///
    /// PROVEN findings:
    /// * 712 GameObject records, ALL decode (block 41: ZERO errors —
    ///   the previous 9 fail-closed errors were the 6
    ///   AmbientExclusionVolume + 3 WebNodeLocator records whose
    ///   layouts are now PROVEN and typed);
    /// * 511 `proto::CloudSpawner` + 112 `proto::ParkedCarSpawner` + 14
    ///   `proto::EffectsPropSpawner` = 637 binary records, ALL with a
    ///   unit quaternion and world-scale positions;
    /// * ParkedCarSpawner drawables are EMPTY (112/112 — the engine
    ///   picks the car model at runtime), so they carry no drawable;
    /// * 45 `proto::LightDecalInfo` are LightDecal records (block 41:
    ///   `[A][n][n decal names]`, all n=1 in this cell, host = road-curve
    ///   props); 21 `GameObject` (point lights + billboards) are the
    ///   prop-table records; the point-light transforms are WORLD-scale
    ///   (e.g. pointLight147 at (182.0, 28.37, 409.44));
    /// * 6 AmbientExclusionVolume + 3 WebNodeLocator carry the block-41
    ///   typed payloads (unit quats 9/9; WNL ids == name suffixes);
    /// * the billboard/sign prop-table records are stage-local TEMPLATE
    ///   definitions — the same local transforms repeat verbatim in
    ///   other cells' members — and the world placement of the same
    ///   drawables comes from their CloudSpawner records (proven in
    ///   `pairs_cell82_drawables_to_shapes`).
    #[test]
    fn proto_variant_census_cell82() {
        let buf = fixture("cells_obj_cell82.p3d");
        let e = scan_world_member_opts(
            0x51A2CA3B,
            &buf,
            ScanOptions {
                retain_objects: true,
                ..Default::default()
            },
        );
        assert_eq!(e.hash, 0x51A2CA3B);
        assert_eq!(e.kind, MemberKind::Objects);
        assert_eq!(e.objects, 712);
        assert_eq!(e.object_errors, 0, "{:?}", e.error_notes);
        assert_eq!(e.retained_objects.len(), 712);

        let mut census: std::collections::BTreeMap<&str, usize> = Default::default();
        let mut variants: std::collections::BTreeMap<ObjectVariant, usize> = Default::default();
        for obj in &e.retained_objects {
            *census.entry(obj.class.as_str()).or_default() += 1;
            *variants.entry(obj.variant).or_default() += 1;
        }
        assert_eq!(variants.get(&ObjectVariant::Binary), Some(&637));
        assert_eq!(variants.get(&ObjectVariant::PropTable), Some(&21));
        assert_eq!(variants.get(&ObjectVariant::LightDecal), Some(&45));
        assert_eq!(
            variants.get(&ObjectVariant::AmbientExclusionVolume),
            Some(&6)
        );
        assert_eq!(variants.get(&ObjectVariant::WebNodeLocator), Some(&3));
        assert_eq!(census.get("proto::CloudSpawner"), Some(&511));
        assert_eq!(census.get("proto::ParkedCarSpawner"), Some(&112));
        assert_eq!(census.get("proto::EffectsPropSpawner"), Some(&14));
        assert_eq!(census.get("proto::LightDecalInfo"), Some(&45));
        assert_eq!(census.get("GameObject"), Some(&21));
        assert_eq!(
            census.get("proto::AmbientExclusionVolume"),
            Some(&6)
        );
        assert_eq!(census.get("proto::WebNodeLocator"), Some(&3));

        // ParkedCarSpawner: world-placed but drawable-less (runtime pick)
        let parked: Vec<_> = e
            .retained_objects
            .iter()
            .filter(|o| o.class == "proto::ParkedCarSpawner")
            .collect();
        assert_eq!(parked.len(), 112);
        assert!(parked.iter().all(|o| o.drawable.is_none()));
        assert!(parked.iter().all(|o| o.placement.is_some()));
        assert!(parked.iter().all(|o| o.placement.as_ref().unwrap().is_unit_quat()));

        // billBoard001's WORLD placement lives on the CloudSpawner record
        let bb1 = e
            .retained_objects
            .iter()
            .find(|o| o.class == "proto::CloudSpawner"
                && o.drawable.as_deref() == Some("billBoard001"))
            .expect("billBoard001 CloudSpawner");
        assert_eq!(bb1.variant, ObjectVariant::Binary);
        assert_eq!(bb1.state.as_deref(), Some("initial,001"));
        let pl = bb1.placement.as_ref().unwrap();
        assert!((pl.pos[0] - 298.7417).abs() < 1e-3);
        assert!((pl.pos[1] - 37.4367).abs() < 1e-3);
        assert!((pl.pos[2] - 353.3841).abs() < 1e-3);
        // billBoard005 likewise (301.0, 44.0, 374.0) — round numbers,
        // state "initial"
        let bb5 = e
            .retained_objects
            .iter()
            .find(|o| o.class == "proto::CloudSpawner"
                && o.drawable.as_deref() == Some("billBoard005"))
            .expect("billBoard005 CloudSpawner");
        let pl = bb5.placement.as_ref().unwrap();
        assert!((pl.pos[0] - 301.0).abs() < 1e-4);
        assert!((pl.pos[1] - 44.0).abs() < 1e-4);
        assert!((pl.pos[2] - 374.0).abs() < 1e-4);
        assert_eq!(bb5.state.as_deref(), Some("initial"));

        // prop-table point lights are WORLD-placed (stage-local rule
        // does NOT apply to them)
        let light = e
            .retained_objects
            .iter()
            .find(|o| o.class == "GameObject" && o.full_name.contains("pointLight147"))
            .expect("pointLight147");
        assert_eq!(light.variant, ObjectVariant::PropTable);
        let pl = light.placement.expect("pointLight147 transform");
        assert!((pl.pos[0] - 182.0).abs() < 1e-2);
        assert!((pl.pos[1] - 28.3667).abs() < 1e-2);
        assert!((pl.pos[2] - 409.4378).abs() < 1e-2);
        assert_eq!(pl.quat_xyzw, [0.0, 0.0, 0.0, -1.0]);

        // ---- block-41 typed payloads on the same real member ----

        // LightDecalInfo: 45 records, all n=1 here, host = road-curve
        // props, decal = the cell's light decal resource.
        let ld: Vec<_> = e
            .retained_objects
            .iter()
            .filter(|o| o.variant == ObjectVariant::LightDecal)
            .collect();
        assert_eq!(ld.len(), 45);
        assert!(ld.iter().all(|o| o.placement.is_none()
            && o.drawable.is_none()
            && o.state.is_none()));

        // AmbientExclusionVolume: 6 records, unit quats, real 3-D
        // placements; AEVol_12 is the measured street-length box
        // (101 m × 6.4 m × 14.5 m half-extents) at (443.30, 26.80, 341.37).
        let aev: Vec<_> = e
            .retained_objects
            .iter()
            .filter(|o| o.variant == ObjectVariant::AmbientExclusionVolume)
            .collect();
        assert_eq!(aev.len(), 6);
        let v12 = aev
            .iter()
            .find(|o| o.short_name.trim_end() == "AEVol_12")
            .expect("AEVol_12");
        match v12.payload.as_ref().unwrap() {
            ObjectPayload::AmbientExclusion(v) => {
                assert!((v.pos[0] - 443.3046).abs() < 1e-3);
                assert!((v.pos[1] - 26.80).abs() < 1e-2);
                assert!((v.pos[2] - 341.37).abs() < 1e-2);
                assert!((quat_norm(v.quat_xyzw) - 1.0).abs() < 1e-3);
                assert!((v.half_extents[0] - 101.36).abs() < 1e-2);
                assert!((v.half_extents[1] - 6.43).abs() < 1e-2);
                assert!((v.half_extents[2] - 14.49).abs() < 1e-2);
                assert_eq!(v.tail, [0, 1, 6]);
            }
            other => panic!("unexpected payload {other:?}"),
        }
        assert!(aev.iter().all(|o| o.placement.is_some()
            && match o.payload.as_ref().unwrap() {
                ObjectPayload::AmbientExclusion(v) => {
                    (quat_norm(v.quat_xyzw) - 1.0).abs() < 0.01
                        && v.tail[0] <= 1
                        && v.tail[1] <= 1
                        && matches!(v.tail[2], 0 | 2 | 6)
                }
                other => panic!("unexpected payload {other:?}"),
            }));

        // WebNodeLocator: 3 records, ids pinned to the name suffixes;
        // woi_loc_100 sits at (196.6, 20.0, 209.4) with the identity quat.
        let wnl: Vec<_> = e
            .retained_objects
            .iter()
            .filter(|o| o.variant == ObjectVariant::WebNodeLocator)
            .collect();
        assert_eq!(wnl.len(), 3);
        let l100 = wnl
            .iter()
            .find(|o| o.short_name.trim_end() == "woi_loc_100")
            .expect("woi_loc_100");
        match l100.payload.as_ref().unwrap() {
            ObjectPayload::WebLocator(v) => {
                assert_eq!(v.id, 100);
                assert!((v.pos[0] - 196.576).abs() < 1e-2);
                assert!((v.pos[1] - 20.0).abs() < 1e-4);
                assert!((v.pos[2] - 209.403).abs() < 1e-2);
                assert_eq!(v.quat_xyzw, [0.0, 0.0, 0.0, -1.0]);
            }
            other => panic!("unexpected payload {other:?}"),
        }
        assert!(wnl.iter().all(|o| match (
            o.short_name.trim_end(),
            o.payload.as_ref().unwrap()
        ) {
            (name, ObjectPayload::WebLocator(v)) => {
                trailing_number(name) == Some(v.id)
            }
            (_, other) => panic!("unexpected payload {other:?}"),
        }));

        // The decal hosts: every LightDecalInfo host is a road-curve
        // prop of this cell and every decal name is Cell_82's set.
        let first = match ld[0].payload.as_ref().unwrap() {
            ObjectPayload::LightDecal(l) => l,
            other => panic!("unexpected payload {other:?}"),
        };
        assert!(
            first.host_prop.starts_with("Cell_82path")
                && first.host_prop.ends_with("_curve_road_prop"),
            "host = {first:?}"
        );
        assert_eq!(first.decals, vec!["Cell_82_light_decal_0".to_string()]);
        assert!(ld.iter().all(|o| match o.payload.as_ref().unwrap() {
            ObjectPayload::LightDecal(l) => l.decals.len() == 1,
            other => panic!("unexpected payload {other:?}"),
        }));
    }

    /// Block 41 — the template + dual-decal families on two MORE real
    /// members (extracted from cells.rcf, committed as fixtures):
    /// * the per-cell TEMPLATE record (short name `__light_decal_info`,
    ///   empty host, n = the cell's whole decal set);
    /// * the DUAL records — streetlight hosts carry TWO decals (one per
    ///   head), road-curve hosts one.
    #[test]
    fn light_decal_template_and_dual_records() {
        // Cell_201 mix member: 1 template LDI (n=3), 6 prop-table
        // GameObjects, 8 CloudSpawners — 15 records, 0 errors.
        let buf = fixture("cells_obj_decal_mix.p3d");
        let e = scan_world_member_opts(
            0xD006241E,
            &buf,
            ScanOptions {
                retain_objects: true,
                ..Default::default()
            },
        );
        assert_eq!(e.objects, 15);
        assert_eq!(e.object_errors, 0, "{:?}", e.error_notes);
        let lds: Vec<&CellObject> = e
            .retained_objects
            .iter()
            .filter(|o| o.variant == ObjectVariant::LightDecal)
            .collect();
        assert_eq!(lds.len(), 1);
        match lds[0].payload.as_ref().unwrap() {
            ObjectPayload::LightDecal(l) => {
                assert_eq!(l.host_prop, "");
                assert_eq!(
                    l.decals,
                    vec![
                        "Cell_201_light_decal_0".to_string(),
                        "Cell_201_light_decal_1".to_string(),
                        "Cell_201_light_decal_2".to_string(),
                    ]
                );
            }
            other => panic!("unexpected payload {other:?}"),
        }
        assert_eq!(lds[0].short_name.trim_end(), "__light_decal_info");
        assert_eq!(
            e.retained_objects
                .iter()
                .filter(|o| o.variant == ObjectVariant::PropTable)
                .count(),
            6
        );
        assert_eq!(
            e.retained_objects
                .iter()
                .filter(|o| o.variant == ObjectVariant::Binary)
                .count(),
            8
        );

        // Cell_125 dual member: 14 LDIs — 5 streetlight hosts with TWO
        // decals each, 9 road-curve hosts with one.
        let buf = fixture("cells_obj_decal_dual.p3d");
        let e = scan_world_member_opts(
            0x4479_335F,
            &buf,
            ScanOptions {
                retain_objects: true,
                ..Default::default()
            },
        );
        assert_eq!(e.object_errors, 0, "{:?}", e.error_notes);
        let lds: Vec<&CellObject> = e
            .retained_objects
            .iter()
            .filter(|o| o.variant == ObjectVariant::LightDecal)
            .collect();
        assert_eq!(lds.len(), 14);
        let duals: Vec<&LightDecalInfo> = lds
            .iter()
            .filter_map(|o| match o.payload.as_ref().unwrap() {
                ObjectPayload::LightDecal(l) if l.decals.len() == 2 => Some(l),
                _ => None,
            })
            .collect();
        assert_eq!(duals.len(), 5);
        assert!(duals.iter().all(|l| l.host_prop.starts_with("Cell_125streetlight006")));
        assert_eq!(
            duals[0].decals,
            vec![
                "Cell_125_light_decal_9".to_string(),
                "Cell_125_light_decal_10".to_string(),
            ]
        );
        let singles: Vec<&LightDecalInfo> = lds
            .iter()
            .filter_map(|o| match o.payload.as_ref().unwrap() {
                ObjectPayload::LightDecal(l) if l.decals.len() == 1 => Some(l),
                _ => None,
            })
            .collect();
        assert_eq!(singles.len(), 9);
        assert!(
            singles
                .iter()
                .all(|l| l.host_prop.contains("curve_road_prop")),
            "road-curve hosts"
        );
    }

    /// Block 41 — fail-closed on the typed payloads: a truncated decal
    /// payload, a decal count of 0, a non-unit AEV quaternion and a WNL
    /// id that disagrees with the short-name suffix are ALL errors.
    #[test]
    fn typed_families_fail_closed() {
        // Build a minimal GameObject chunk: [12-B prelude][3 pascals]
        // [24-B header][META][payload].
        let build = |class: &str, short: &str, payload: &[u8]| -> Vec<u8> {
            let mut b = Vec::new();
            b.extend_from_slice(&[0u8; 12]); // prelude
            for s in ["GameObject(1):x", short, class] {
                b.push(s.len() as u8);
                b.extend_from_slice(s.as_bytes());
            }
            b.extend_from_slice(&1u32.to_le_bytes()); // flag
            b.extend_from_slice(&GAME_OBJECT_HASH.to_le_bytes());
            b.extend_from_slice(&GAME_OBJECT_MARKER.to_le_bytes());
            b.extend_from_slice(&[0u8; 12]); // A/B/C
            b.extend_from_slice(b"META");
            b.extend_from_slice(payload);
            b
        };
        let decal_payload = |host: &str, decals: &[&str]| {
            let mut p = Vec::new();
            p.extend_from_slice(&(host.len() as u32).to_le_bytes());
            p.extend_from_slice(host.as_bytes());
            p.extend_from_slice(&(decals.len() as u32).to_le_bytes());
            for d in decals {
                p.extend_from_slice(&(d.len() as u32).to_le_bytes());
                p.extend_from_slice(d.as_bytes());
            }
            p
        };
        let aev_payload = |quat: [f32; 4]| {
            let mut p = Vec::new();
            for f in [1.0f32, 2.0, 3.0] {
                p.extend_from_slice(&f.to_le_bytes());
            }
            for f in quat {
                p.extend_from_slice(&f.to_le_bytes());
            }
            for f in [1.0f32, 1.0, 1.0] {
                p.extend_from_slice(&f.to_le_bytes());
            }
            for u in [1u32, 0, 2] {
                p.extend_from_slice(&u.to_le_bytes());
            }
            p
        };

        // Well-formed decal record decodes.
        let good = build(
            "proto::LightDecalInfo",
            "Cell_9path1_2_curve_road_prop__light_decal_info",
            &decal_payload("Cell_9path1_2_curve_road_prop", &["Cell_9_light_decal_0"]),
        );
        let obj = decode_game_object(&good, 0, good.len()).expect("good decal record");
        match obj.payload.as_ref().unwrap() {
            ObjectPayload::LightDecal(l) => {
                assert_eq!(l.host_prop, "Cell_9path1_2_curve_road_prop");
                assert_eq!(l.decals, vec!["Cell_9_light_decal_0".to_string()]);
            }
            other => panic!("unexpected payload {other:?}"),
        }
        assert_eq!(obj.variant, ObjectVariant::LightDecal);

        // Truncated payload (one byte short) is an error.
        let cut = &good[..good.len() - 1];
        assert!(decode_game_object(cut, 0, cut.len()).is_err());

        // Decal count 0 is an error.
        let zero = build(
            "proto::LightDecalInfo",
            "s__light_decal_info",
            &decal_payload("h", &[]),
        );
        assert!(decode_game_object(&zero, 0, zero.len()).is_err());

        // Trailing garbage past the last decal name is an error.
        let mut garbage = decal_payload("h", &["Cell_9_light_decal_0"]);
        garbage.extend_from_slice(&[0u8; 4]);
        let g = build("proto::LightDecalInfo", "s__light_decal_info", &garbage);
        assert!(decode_game_object(&g, 0, g.len()).is_err());

        // Non-unit AEV quaternion is an error.
        let bad_quat = build(
            "proto::AmbientExclusionVolume",
            "AEVol_1",
            &aev_payload([0.0, 0.0, 0.0, -2.0]),
        );
        assert!(decode_game_object(&bad_quat, 0, bad_quat.len()).is_err());

        // A WNL id that disagrees with the short-name suffix is an error.
        let mut wnl = Vec::new();
        for f in [10.0f32, 20.0, 30.0] {
            wnl.extend_from_slice(&f.to_le_bytes());
        }
        for f in [0.0f32, 0.0, 0.0, -1.0] {
            wnl.extend_from_slice(&f.to_le_bytes());
        }
        wnl.extend_from_slice(&99u32.to_le_bytes());
        let w = build("proto::WebNodeLocator", "woi_loc_47", &wnl);
        assert!(decode_game_object(&w, 0, w.len()).is_err());
        // ...and the matching id decodes.
        let mut wnl_ok = wnl.clone();
        let fix = 28; // the id's offset inside the 32-B payload
        wnl_ok[fix..fix + 4].copy_from_slice(&47u32.to_le_bytes());
        let w2 = build("proto::WebNodeLocator", "woi_loc_47", &wnl_ok);
        let obj = decode_game_object(&w2, 0, w2.len()).expect("matching id");
        match obj.payload.as_ref().unwrap() {
            ObjectPayload::WebLocator(v) => assert_eq!(v.id, 47),
            other => panic!("unexpected payload {other:?}"),
        }

        // An AEV payload of the wrong size is an error.
        let short_aev = build(
            "proto::AmbientExclusionVolume",
            "AEVol_1",
            &aev_payload([0.0, 0.0, 0.0, -1.0])[..40],
        );
        assert!(decode_game_object(&short_aev, 0, short_aev.len()).is_err());
    }

    /// Block 41 — the trailing-number helper's rules.
    #[test]
    fn trailing_number_rules() {
        assert_eq!(trailing_number("woi_loc_47"), Some(47));
        assert_eq!(trailing_number("woi_loc_100"), Some(100));
        assert_eq!(trailing_number("woi_loc"), None);
        assert_eq!(trailing_number(""), None);
        assert_eq!(trailing_number("x9"), Some(9));
    }
}


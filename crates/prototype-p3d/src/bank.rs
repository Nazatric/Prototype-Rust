//! Bank descriptor decoding — the `Cue` / `Patch` classic-chunk dialect.
//!
//! ## PROVEN format (block 65; byte-exact on ALL 325 Cue/Patch bodies of the
//! 12 music/ambience banks mined from the live `00audio.rcf` release asset)
//!
//! The game's 41 top-level audio banks are flat sequences of classic chunks
//! named `Cue` or `Patch` (classic id `0x0000000A`).  Their payload after the
//! classic name field ("body") is a typed property stream:
//!
//! ```text
//! body := 0x00 | kind:u32 (1=Patch, 2=Cue) | ...
//! ```
//!
//! **Patch body** (274/274 byte-exact on the 12 music banks; block 66
//! extends the grammar to ALL 154 boot_strap.p3d bodies — 152/152
//! Patches + 2/2 Cues byte-exact — with three corpus-gap cases the
//! music-only corpus never exercised, marked Ⓑ66 below):
//!
//! ```text
//! 0x00 | 1:u32
//! object_name  : u32len ascii NUL          (untagged)
//! object_id    : [u8;8]
//! class_name   : 01 | u32len ascii NUL     (tagged)
//! flag | prop_count:u32 | class_id:[u8;8]  (13-byte class head; (B66)
//!        the first byte is a FLAG 0x00/0x01 — 0x00 on every music-bank
//!        chunk, 0x01 observed on Master / radverb_generic (semantic
//!        UNKNOWN); prop_count == the exact number of key properties,
//!        PROVEN: music_player=6, Ambience_all=59 — every head count
//!        verified byte-exact by the exact end-of-body accounting)
//! properties, prop_count × [type-decl, key]:
//!   type-decl := 01 | u32len ascii NUL | type_id:[u8;8]
//!                (Bool | Float32 | String | "String List" | EnvelopeData
//!                 | Vector (B66))
//!   key       := 01 | u32len ascii NUL | value | trailing:[u8;8]
//!   value by declared type:
//!     Bool          : 1 byte (0/1 in the corpus)
//!     Float32       : 4 bytes LE
//!     String        : u32len ascii NUL ((B66) len MAY be 0 — an empty
//!                     value, e.g. the Radio_Dialogue templates'
//!                     file = "")
//!     "String List" : u32count | count × (u32len ascii NUL)
//!                     ((B66) count MAY be 0 — an empty list)
//!     EnvelopeData  : u32count | count × (f32, f32)
//!     Vector (B66)  : 12 bytes = 3 × f32 LE (x, y, z)
//!   the trailing 8-byte word is present on every key EXCEPT the final
//!   property of the chunk (the chunk ends right after its value).
//! ```
//!
//! **Cue body** (51/51 byte-exact; the mission music graphs):
//!
//! ```text
//! 0x00 | 2:u32
//! object_name : u32len ascii NUL           (the mission id, e.g. "e10m04")
//! radius:f32 | transition_count:u32 | 1:u32 | header_id:[u8;8]
//! transitions, transition_count × :
//!   01 | u32len ascii NUL | 0:u32 | id:[u8;8]
//! "main" : 01 | u32len ascii NUL | class_id:[u8;8]
//! cells, transition_count × — each cell is ONE of:
//!   "MusicMGraphCell": 01 | u32len ascii NUL | struct:[u8;80]
//!                     [ stem: 01 | u32len ascii NUL  (a patch/stem name)
//!                       [ trailing:[u8;8] unless final item ] ]
//!   "EmptyCell"     : 01 | u32len ascii NUL | id:[u8;8]   (no struct, no stem)
//!   (the cell count equals transition_count in all 51 cues)
//! ```
//!
//! Every byte of every body is accounted for by this grammar — the parser
//! is fail-closed and requires `Reader::at_end()` on success.
//!
//! **Cross-check (PROVEN)**: every `music_player.filename` value of the 12
//! banks (245/245) is an exact clip name in the committed block-63 shard
//! index (`audio_shard_index.tsv`); cue cell stems hit 144 directly and 35
//! more as prefixes of numbered variants (e.g. `e01m01_complete` →
//! `e01m01_complete01`).  77 stems (the e04m03/e05m02 `_fr` families) match
//! nothing in the two AUDIO archives — LIKELY they live in the four
//! never-mined `0xwoi.rcf` release assets.
//!
//! BLOCK 66 CORRECTION of the block-65 "command-script Cue" note: there
//! is NO command-dialect Cue in boot_strap.p3d.  Both of its Cues
//! (`clear_cue`, `front_end`) decode with the standard grammar, and its
//! 24 previously-refused chunks were Patches hitting three gaps the
//! music-only corpus never exercised: the `Vector` type, empty String
//! values / empty String Lists (template patches), and the head flag
//! byte 0x01.  With those three extensions the parser decodes ALL 154
//! boot_strap bodies byte-exact (scripts/m66_command_dialect.py +
//! m66b_grammar_extension.py committed; the knob sweep proved the combo
//! is the unique full decode among the tried variants).

#![forbid(unsafe_code)]

use prototype_common::{P3dError, Reader, Result};

/// Classic chunk name for patch descriptor chunks.
pub const PATCH_CHUNK_NAME: &str = "Patch";
/// Classic chunk name for cue descriptor chunks.
pub const CUE_CHUNK_NAME: &str = "Cue";
/// kind u32 of a Patch body.
pub const KIND_PATCH: u32 = 1;
/// kind u32 of a Cue body.
pub const KIND_CUE: u32 = 2;

/// The six schema type names observed in the corpus (block 66: `Vector`
/// joined the five music-bank types once boot_strap.p3d's descriptors
/// decoded).
pub const TYPE_BOOL: &str = "Bool";
pub const TYPE_FLOAT32: &str = "Float32";
pub const TYPE_STRING: &str = "String";
pub const TYPE_STRING_LIST: &str = "String List";
pub const TYPE_ENVELOPE: &str = "EnvelopeData";
/// `Vector` — 12 bytes = 3 × f32 LE (block 66, PROVEN: the extension
/// combo that decodes ALL 152 boot_strap Patch bodies byte-exact; the
/// music banks never declare it).
pub const TYPE_VECTOR: &str = "Vector";

/// Cell-class names inside a Cue.
pub const CELL_MUSIC_MGRAPH: &str = "MusicMGraphCell";
pub const CELL_EMPTY: &str = "EmptyCell";

/// Size of the `MusicMGraphCell` cell struct (PROVEN: 80 in all 306
/// occurrences across the 51 cues).
pub const MUSIC_MGRAPH_STRUCT_SIZE: usize = 80;

/// Sanity cap for any counted repeat (names, list items, envelope points,
/// transitions, cells).  The corpus maxima are 59 props / 18 stems / 9
/// transitions; 256 leaves 4× head-room while refusing runaway loops.
const COUNT_CAP: usize = 256;

/// A typed property value.
#[derive(Debug, Clone, PartialEq)]
pub enum BankValue {
    /// `Bool` — 1 byte.
    Bool(bool),
    /// `Float32` — 4 bytes LE.
    Float32(f32),
    /// `String` — u32len ascii NUL.
    Str(String),
    /// `String List` — u32count + count strings.
    StrList(Vec<String>),
    /// `EnvelopeData` — u32count + count × (f32 time, f32 value).
    Envelope(Vec<(f32, f32)>),
    /// `Vector` — 12 bytes = 3 × f32 LE (block 66: boot_strap's
    /// positional/dialogue descriptors; the music banks never use it).
    Vector([f32; 3]),
}

/// One key property of a Patch.
#[derive(Debug, Clone, PartialEq)]
pub struct BankProperty {
    /// Key name (shipped spelling preserved — the corpus contains the
    /// game's own typo `rofftop_env`).
    pub name: String,
    /// Typed value.
    pub value: BankValue,
}

/// A decoded Patch chunk (a sound/player descriptor).
#[derive(Debug, Clone, PartialEq)]
pub struct BankPatch {
    /// Object name — usually the stem/player id (e.g. `e10m04_ambient`).
    pub object: String,
    /// 8-byte instance id that follows the object name.
    pub object_id: [u8; 8],
    /// Class name (e.g. `music_player`, `music_player_random`,
    /// `AtCamera_Player_Quad`, `Ambience_all`).
    pub class: String,
    /// The class head's property count (PROVEN == `props.len()`).
    pub class_prop_count: u32,
    /// The class head's 8-byte id.
    pub class_id: [u8; 8],
    /// Key properties in stream order.
    pub props: Vec<BankProperty>,
}

impl BankPatch {
    /// The value of the first property with this name, if any.
    pub fn prop(&self, name: &str) -> Option<&BankValue> {
        self.props.iter().find(|p| p.name == name).map(|p| &p.value)
    }

    /// The `filename` / `filenames` clip references of this patch.
    pub fn clip_names(&self) -> Vec<&str> {
        let mut out = Vec::new();
        for p in &self.props {
            match &p.value {
                BankValue::Str(s) if p.name == "filename" => out.push(s.as_str()),
                BankValue::StrList(v) if p.name == "filenames" || p.name == "files" => {
                    out.extend(v.iter().map(|s| s.as_str()))
                }
                _ => {}
            }
        }
        out
    }
}

/// One cell of a Cue's music graph.
#[derive(Debug, Clone, PartialEq)]
pub struct BankCell {
    /// `MusicMGraphCell` (has an 80-byte struct and usually a stem) or
    /// `EmptyCell` (bare 8-byte id).
    pub is_empty: bool,
    /// The 80-byte `MusicMGraphCell` struct, when not an `EmptyCell`.
    pub mgraph_struct: Option<[u8; MUSIC_MGRAPH_STRUCT_SIZE]>,
    /// The stem/patch name this cell plays (absent for `EmptyCell`).
    pub stem: Option<String>,
}

/// A decoded Cue chunk (a mission music graph).
#[derive(Debug, Clone, PartialEq)]
pub struct BankCue {
    /// Object name — the mission id (e.g. `e10m04`).
    pub object: String,
    /// Header f32 (150.0 / 75.0 / 132.0 in the corpus — likely a hearing
    /// radius; exact semantic UNKNOWN).
    pub radius: f32,
    /// Transition names in stream order (e.g. ambient/suspense/combat/...).
    pub transitions: Vec<String>,
    /// Cells in stream order (`cells.len() == transitions.len()` PROVEN).
    pub cells: Vec<BankCell>,
}

/// A decoded bank chunk.
#[derive(Debug, Clone, PartialEq)]
pub enum BankChunk {
    Patch(BankPatch),
    Cue(BankCue),
}

/// Read a bank classic-chunk body (the bytes after the classic name field).
///
/// `body` is `ClassicChunk::data` for a chunk named `Patch` or `Cue`.
pub fn parse_body(body: &[u8]) -> Result<BankChunk> {
    let mut r = Reader::new(body);
    let pad = r.read_u8()?;
    if pad != 0 {
        return Err(P3dError::Other(format!(
            "bank body must start 0x00, got {pad:#04x}"
        )));
    }
    let kind = r.read_u32()?;
    match kind {
        KIND_PATCH => Ok(BankChunk::Patch(parse_patch(&mut r)?)),
        KIND_CUE => Ok(BankChunk::Cue(parse_cue(&mut r)?)),
        other => Err(P3dError::Other(format!(
            "bank kind must be 1 (Patch) or 2 (Cue), got {other}"
        ))),
    }
}

/// Read `01 | u32len ascii NUL` (tagged) or `u32len ascii NUL` (untagged).
fn read_name(r: &mut Reader<'_>, tagged: bool) -> Result<String> {
    read_string(r, tagged, false)
}

/// Read a String VALUE — untagged, and MAY be EMPTY (block 66: the
/// template patches carry e.g. `file = ""`; names/keys keep the ≥1
/// rule, only values can be zero-length).
fn read_value_string(r: &mut Reader<'_>) -> Result<String> {
    read_string(r, false, true)
}

fn read_string(r: &mut Reader<'_>, tagged: bool, allow_empty: bool) -> Result<String> {
    if tagged {
        let tag = r.read_u8()?;
        if tag != 1 {
            return Err(P3dError::Other(format!(
                "expected string tag 0x01, got {tag:#04x}"
            )));
        }
    }
    let len = r.read_u32()? as usize;
    let min = if allow_empty { 0 } else { 1 };
    if !(min..=COUNT_CAP).contains(&len) {
        return Err(P3dError::Other(format!("bank name length {len} out of range")));
    }
    let bytes = r.read_bytes(len)?;
    if !bytes.iter().all(|&b| (0x20..0x7f).contains(&b)) {
        return Err(P3dError::Other("bank name has non-ASCII bytes".into()));
    }
    let nul = r.read_u8()?;
    if nul != 0 {
        return Err(P3dError::Other("bank name is not NUL-terminated".into()));
    }
    // from_utf8 is safe: every byte was validated ASCII above.
    Ok(String::from_utf8(bytes.to_vec()).expect("ASCII validated"))
}

fn read_id8(r: &mut Reader<'_>) -> Result<[u8; 8]> {
    let b = r.read_bytes(8)?;
    let mut id = [0u8; 8];
    id.copy_from_slice(b);
    Ok(id)
}

/// Consume a key's trailing 8-byte word: present unless it is the chunk's
/// final bytes (PROVEN rule).
fn read_trailing_word(r: &mut Reader<'_>) -> Result<()> {
    if r.remaining() >= 8 {
        r.skip(8)?;
    } else if !r.at_end() {
        return Err(P3dError::Other(format!(
            "{} trailing bytes after final key value",
            r.remaining()
        )));
    }
    Ok(())
}

fn parse_patch(r: &mut Reader<'_>) -> Result<BankPatch> {
    let object = read_name(r, false)?;
    let object_id = read_id8(r)?;
    let class = read_name(r, true)?;
    // Block 66: the head's first byte is a FLAG, not a pad — 0x00 on
    // every music-bank chunk, 0x01 observed on Master / radverb_generic
    // in boot_strap.p3d.  The semantic is honest UNKNOWN; anything
    // outside {0, 1} still fails closed.
    let head_flag = r.read_u8()?;
    if head_flag > 1 {
        return Err(P3dError::Other(format!(
            "class head flag must be 0x00 or 0x01, got {head_flag:#04x}"
        )));
    }
    let class_prop_count = r.read_u32()?;
    if class_prop_count as usize > COUNT_CAP {
        return Err(P3dError::Other(format!(
            "class prop count {} out of range",
            class_prop_count
        )));
    }
    let class_id = read_id8(r)?;

    let mut props = Vec::with_capacity(class_prop_count as usize);
    let mut decl: Option<String> = None;
    while !r.at_end() {
        let name = read_name(r, true)?;
        match name.as_str() {
            TYPE_BOOL | TYPE_FLOAT32 | TYPE_STRING | TYPE_STRING_LIST | TYPE_ENVELOPE
            | TYPE_VECTOR => {
                // type declaration
                read_id8(r)?;
                decl = Some(name);
                continue;
            }
            _ => {}
        }
        let decl_name = decl.as_deref().ok_or_else(|| {
            P3dError::Other(format!("key {name:?} appears before any type declaration"))
        })?;
        let value = match decl_name {
            TYPE_BOOL => {
                let b = r.read_u8()?;
                BankValue::Bool(b != 0)
            }
            TYPE_FLOAT32 => BankValue::Float32(f32::from_bits(r.read_u32()?)),
            TYPE_STRING => BankValue::Str(read_value_string(r)?),
            TYPE_STRING_LIST => {
                let count = r.read_u32()? as usize;
                // Block 66: count 0 is a legitimate EMPTY list (the
                // template patches carry one); only > COUNT_CAP fails.
                if count > COUNT_CAP {
                    return Err(P3dError::Other(format!(
                        "string list count {count} out of range"
                    )));
                }
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    items.push(read_value_string(r)?);
                }
                BankValue::StrList(items)
            }
            TYPE_ENVELOPE => {
                let count = r.read_u32()? as usize;
                if count == 0 || count > COUNT_CAP {
                    return Err(P3dError::Other(format!("envelope count {count} out of range")));
                }
                let mut pts = Vec::with_capacity(count);
                for _ in 0..count {
                    let t = f32::from_bits(r.read_u32()?);
                    let v = f32::from_bits(r.read_u32()?);
                    pts.push((t, v));
                }
                BankValue::Envelope(pts)
            }
            TYPE_VECTOR => {
                let x = f32::from_bits(r.read_u32()?);
                let y = f32::from_bits(r.read_u32()?);
                let z = f32::from_bits(r.read_u32()?);
                BankValue::Vector([x, y, z])
            }
            other => {
                return Err(P3dError::Other(format!(
                    "key {name:?} declared by unhandled type {other:?}"
                )))
            }
        };
        read_trailing_word(r)?;
        props.push(BankProperty { name, value });
    }
    if props.len() as u32 != class_prop_count {
        return Err(P3dError::Other(format!(
            "class head promises {class_prop_count} properties, body carries {}",
            props.len()
        )));
    }
    Ok(BankPatch {
        object,
        object_id,
        class,
        class_prop_count,
        class_id,
        props,
    })
}

fn parse_cue(r: &mut Reader<'_>) -> Result<BankCue> {
    let object = read_name(r, false)?;
    let radius = f32::from_bits(r.read_u32()?);
    let transition_count = r.read_u32()? as usize;
    if transition_count == 0 || transition_count > COUNT_CAP {
        return Err(P3dError::Other(format!(
            "cue transition count {transition_count} out of range"
        )));
    }
    let header_one = r.read_u32()?;
    if header_one != 1 {
        return Err(P3dError::Other(format!(
            "cue header u32 must be 1 in every observed cue, got {header_one}"
        )));
    }
    read_id8(r)?;
    let mut transitions = Vec::with_capacity(transition_count);
    for _ in 0..transition_count {
        let name = read_name(r, true)?;
        let zero = r.read_u32()?;
        if zero != 0 {
            return Err(P3dError::Other(format!(
                "transition {name:?} carries u32 {zero}, 0 in every observed cue"
            )));
        }
        read_id8(r)?;
        transitions.push(name);
    }
    let main = read_name(r, true)?;
    if main != "main" {
        return Err(P3dError::Other(format!(
            "expected the 'main' cell-class key, got {main:?}"
        )));
    }
    read_id8(r)?;
    let mut cells = Vec::with_capacity(transition_count);
    while !r.at_end() {
        let class = read_name(r, true)?;
        if class == CELL_MUSIC_MGRAPH {
            let bytes = r.read_bytes(MUSIC_MGRAPH_STRUCT_SIZE)?;
            let mut st = [0u8; MUSIC_MGRAPH_STRUCT_SIZE];
            st.copy_from_slice(bytes);
            // the stem ref follows unless the next string is another cell
            let mut stem = None;
            if r.peek_u8()? == 1 {
                let save = r.clone();
                let candidate = read_name(r, true);
                match candidate {
                    Ok(s) if s != CELL_MUSIC_MGRAPH && s != CELL_EMPTY => {
                        read_trailing_word(r)?;
                        stem = Some(s);
                    }
                    Ok(_) => {
                        // next cell's class — restore the reader
                        *r = save;
                    }
                    Err(e) => return Err(e),
                }
            }
            cells.push(BankCell {
                is_empty: false,
                mgraph_struct: Some(st),
                stem,
            });
        } else if class == CELL_EMPTY {
            read_trailing_word(r)?;
            cells.push(BankCell {
                is_empty: true,
                mgraph_struct: None,
                stem: None,
            });
        } else {
            return Err(P3dError::Other(format!(
                "unexpected cue cell class {class:?} (only {CELL_MUSIC_MGRAPH:?} / \
                 {CELL_EMPTY:?} are proven)"
            )));
        }
    }
    if cells.len() != transition_count {
        return Err(P3dError::Other(format!(
            "cue carries {} cells for {} transitions",
            cells.len(),
            transition_count
        )));
    }
    Ok(BankCue {
        object,
        radius,
        transitions,
        cells,
    })
}

// ---------------------------------------------------------------------------
// Tests — pinned on the REAL release-mined bank fixtures (block 65)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Chunk, FileHeader};

    const E10: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../prototype-bin/tests/fixtures/bank_e10_music.p3d"
    ));
    const E02: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../prototype-bin/tests/fixtures/bank_e02_music.p3d"
    ));
    const AMBIENCE: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../prototype-bin/tests/fixtures/bank_ambience.p3d"
    ));
    const SHARD_INDEX: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../prototype-bin/tests/fixtures/audio_shard_index.tsv"
    ));
    const BOOTSTRAP: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../prototype-bin/tests/fixtures/boot_strap.p3d"
    ));

    /// Block 66: with the three grammar extensions (the `Vector` type,
    /// empty String values / empty String Lists, the head flag 0x00|0x01)
    /// EVERY boot_strap.p3d descriptor decodes byte-exact — 152 Patches +
    /// 2 Cues, zero refusals (the block-65 "command-script Cue" note was
    /// an error; the previously-refusing 24 were Patches hitting the
    /// corpus gaps).  Pins the classes, the Vector keys (pos/vel), the
    /// empty values, and the NIS_Player mix references.
    #[test]
    fn boot_strap_descriptors_fully_decode_with_the_block66_extensions() {
        let chunks = decode_banks(BOOTSTRAP);
        let patches: Vec<&BankPatch> = chunks
            .iter()
            .filter_map(|(n, c)| {
                if n == PATCH_CHUNK_NAME {
                    match c {
                        BankChunk::Patch(p) => Some(p),
                        _ => panic!("kind/name mismatch"),
                    }
                } else {
                    None
                }
            })
            .collect();
        let cues = chunks.iter().filter(|(n, _)| n == CUE_CHUNK_NAME).count();
        assert_eq!(patches.len(), 152, "every Patch decodes");
        assert_eq!(cues, 2, "clear_cue + front_end decode with the standard grammar");

        // The class census (block 66 CLI census, byte-exact decode).
        let mut classes: Vec<(String, usize)> = Vec::new();
        for p in &patches {
            match classes.iter_mut().find(|(c, _)| *c == p.class) {
                Some((_, n)) => *n += 1,
                None => classes.push((p.class.clone(), 1)),
            }
        }
        classes.sort();
        let want: Vec<(&str, usize)> = vec![
            ("3D_Clip_Player", 4),
            ("Alex_Monologue_Center", 1),
            ("AtCamera_Player", 1),
            ("AtCamera_Player_Mono", 2),
            ("AtCamera_Player_Quad", 1),
            ("Master", 1),
            ("NIS_Player", 46),
            ("Positional_Dialogue", 7),
            ("Radio_Dialogue", 4),
            ("Semi-Positional_Dialogue", 1),
            ("Simple_Mono_Player", 4),
            ("TestTones", 1),
            ("generic_mono_lr", 62),
            ("generic_positional", 1),
            ("generic_stereo", 11),
            ("music_player", 1),
            ("radverb_generic", 4),
        ];
        let got: Vec<(String, usize)> = classes.clone();
        assert_eq!(got.len(), want.len(), "class set: {classes:?}");
        for ((gc, gn), (wc, wn)) in got.iter().zip(want.iter()) {
            assert_eq!(gc, *wc);
            assert_eq!(*gn, *wn, "class {gc}");
        }

        // Vector: the positional classes carry pos/vel keys, 3 floats each.
        let posd = patches
            .iter()
            .find(|p| p.class == "Positional_Dialogue")
            .expect("a Positional_Dialogue patch");
        match (posd.prop("pos"), posd.prop("vel")) {
            (Some(BankValue::Vector(v)), Some(BankValue::Vector(w))) => {
                assert_eq!(v.len(), 3);
                assert_eq!(w.len(), 3);
                assert!(v.iter().all(|f| f.is_finite()));
            }
            _ => panic!("positional dialogue must carry pos/vel Vectors"),
        }

        // Empty values: the Radio_Dialogue templates carry file = "".
        let radio = patches
            .iter()
            .find(|p| p.class == "Radio_Dialogue")
            .expect("a Radio_Dialogue patch");
        assert_eq!(
            radio.prop("file"),
            Some(&BankValue::Str(String::new())),
            "the empty-string case the music banks never exercised"
        );
        // An empty String List exists too (bus_name = []).
        assert!(
            patches
                .iter()
                .any(|p| matches!(p.prop("bus_name"), Some(BankValue::StrList(v)) if v.is_empty())),
            "the empty-list case the music banks never exercised"
        );

        // The head flag: 0x01 on Master / radverb_generic, 0x00 elsewhere.
        let master = patches.iter().find(|p| p.class == "Master").unwrap();
        assert_eq!(master.props.len(), 13);
        let flagged = patches
            .iter()
            .filter(|p| p.class == "Master" || p.class == "radverb_generic")
            .count();
        assert_eq!(flagged, 5, "the known 0x01-flag classes");
        assert!(
            patches
                .iter()
                .filter(|p| p.class != "Master" && p.class != "radverb_generic")
                .all(|p| p.class_prop_count == p.props.len() as u32),
            "end-of-body accounting holds everywhere"
        );

        // NIS_Player: every one references its NIS mix clip by filename —
        // the direct data feed for the NIS-mix playback wiring.
        let nis: Vec<&BankPatch> = patches
            .iter()
            .filter(|p| p.class == "NIS_Player")
            .copied()
            .collect();
        assert_eq!(nis.len(), 46);
        let mut with_wav = 0usize;
        for p in &nis {
            match p.prop("filename") {
                // Two REAL spellings: the three audio-carrying intro
                // movies (nis0-3a/nis0-4/nis0-5) reference
                // `<movie>_mix.wav` (vestigial — they have Bink audio);
                // the 43 silent NISes reference `<movie>_mix`, the
                // block-64 RADP v6 mix clips.
                Some(BankValue::Str(s)) if s.ends_with("_mix.wav") => with_wav += 1,
                Some(BankValue::Str(s)) => assert!(
                    s.ends_with("_mix"),
                    "NIS_Player {} references the mix clip {s:?}",
                    p.object
                ),
                other => panic!("NIS_Player without a filename: {other:?}"),
            }
        }
        assert_eq!(with_wav, 3, "the audio-carrying intro movies");
    }

    /// Walk a real bank P3D file and decode every Cue/Patch chunk.
    fn decode_banks(data: &[u8]) -> Vec<(String, BankChunk)> {
        let (hdr, mut r) = FileHeader::parse(data).expect("bank file header");
        assert_eq!(hdr.file_size as usize, data.len(), "exact file size");
        let mut out = Vec::new();
        while !r.at_end() {
            let (chunk, next) = Chunk::parse(data, r.pos).expect("chunk");
            r = Reader::from_at(data, next);
            assert_eq!(chunk.chunk_type, crate::CHUNK_TYPE_CLASSIC, "bank chunks are classic");
            let classic = chunk.classic().expect("classic payload").expect("classic");
            if classic.name == PATCH_CHUNK_NAME || classic.name == CUE_CHUNK_NAME {
                let parsed = parse_body(classic.data)
                    .unwrap_or_else(|e| panic!("{} body: {e}", classic.name));
                out.push((classic.name, parsed));
            }
        }
        out
    }

    fn shard_clip_names() -> std::collections::HashSet<String> {
        SHARD_INDEX
            .lines()
            .skip(1)
            .filter_map(|l| l.split('\t').next().map(|s| s.to_string()))
            .collect()
    }

    #[test]
    fn e10_bank_decodes_byte_exactly_with_its_pinned_music_graph() {
        let banks = decode_banks(E10);
        let (cues, patches): (Vec<_>, Vec<_>) = banks
            .iter()
            .partition(|(n, _)| n == CUE_CHUNK_NAME);
        assert_eq!(cues.len(), 1, "e10 carries exactly one cue");
        assert_eq!(patches.len(), 7, "e10 carries seven patches");
        let BankChunk::Cue(cue) = &cues[0].1 else {
            panic!("cue");
        };
        assert_eq!(cue.object, "e10m04");
        assert_eq!(cue.radius, 150.0);
        assert_eq!(
            cue.transitions,
            ["ambient", "suspense", "combat", "death", "complete", "complete_long"]
        );
        assert_eq!(cue.cells.len(), 6, "one cell per transition");
        let stems: Vec<_> = cue.cells.iter().map(|c| c.stem.as_deref()).collect();
        assert_eq!(
            stems,
            [
                Some("e10m04_ambient"),
                Some("e10m04_suspense"),
                Some("e10m04_combat"),
                Some("e10m04_death"),
                Some("e10m04_complete"),
                Some("e10m04_complete_long"),
            ]
        );
        assert!(cue.cells.iter().all(|c| !c.is_empty), "e10 has no EmptyCell");

        // the ambient patch is pinned in full
        let amb = patches
            .iter()
            .find(|(_, c)| matches!(c, BankChunk::Patch(p) if p.object == "e10m04_ambient"))
            .expect("e10m04_ambient patch");
        let BankChunk::Patch(p) = &amb.1 else { panic!() };
        assert_eq!(p.class, "music_player");
        assert_eq!(p.class_prop_count, 6);
        assert_eq!(p.props.len(), 6);
        assert_eq!(p.prop("filename"), Some(&BankValue::Str("e10m04_ambient".into())));
        assert_eq!(
            p.prop("bus_name"),
            Some(&BankValue::StrList(vec![
                "master".into(),
                "music".into(),
                "igmusic".into()
            ]))
        );
        assert_eq!(p.prop("loop"), Some(&BankValue::Bool(true)));
        match p.prop("volume") {
            Some(BankValue::Float32(v)) => assert!((v - 1.0).abs() < 1e-6, "volume {v}"),
            other => panic!("volume: {other:?}"),
        }
        match p.prop("fade_time") {
            Some(BankValue::Float32(v)) => assert!(*v == 0.0, "fade_time {v}"),
            other => panic!("fade_time: {other:?}"),
        }
    }

    #[test]
    fn e02_bank_decodes_with_empty_cells_and_random_player_patches() {
        let banks = decode_banks(E02);
        let cues: Vec<_> = banks
            .iter()
            .filter(|(n, _)| n == CUE_CHUNK_NAME)
            .map(|(_, c)| c)
            .collect();
        let patches: Vec<_> = banks
            .iter()
            .filter(|(n, _)| n == PATCH_CHUNK_NAME)
            .map(|(_, c)| c)
            .collect();
        assert_eq!(cues.len(), 4, "e02 cues");
        assert_eq!(patches.len(), 20, "e02 patches");

        // the corpus-proven EmptyCell cells live in the e02 cues
        let empty_cells = cues
            .iter()
            .filter_map(|c| match c {
                BankChunk::Cue(c) => Some(c.cells.iter().filter(|cell| cell.is_empty).count()),
                _ => None,
            })
            .sum::<usize>();
        assert!(empty_cells >= 1, "e02 must expose EmptyCell cells, got 0");
        // every cell count still matches its transitions
        for c in &cues {
            let BankChunk::Cue(c) = c else { panic!() };
            assert_eq!(c.cells.len(), c.transitions.len());
        }
        // music_player_random patches carry plural props
        let randoms = patches
            .iter()
            .filter_map(|c| match c {
                BankChunk::Patch(p) if p.class == "music_player_random" => Some(p),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(!randoms.is_empty(), "music_player_random present in e02");
        for p in &randoms {
            assert_eq!(p.class_prop_count, 6);
            assert!(
                matches!(p.prop("filenames"), Some(BankValue::StrList(v)) if v.len() >= 2),
                "random player {} has a filename list",
                p.object
            );
            assert!(matches!(p.prop("busnames"), Some(BankValue::StrList(_))));
        }
    }

    #[test]
    fn ambience_bank_decodes_with_envelope_data_and_the_shipped_typo() {
        let banks = decode_banks(AMBIENCE);
        assert_eq!(banks.len(), 3, "ambience carries three patches");
        let classes: Vec<_> = banks
            .iter()
            .filter_map(|(_, c)| match c {
                BankChunk::Patch(p) => Some(p.class.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            classes,
            ["Ambience_all", "AtCamera_Player_Quad", "AtCamera_Player_Quad"]
        );

        let BankChunk::Patch(all) = &banks[0].1 else { panic!() };
        assert_eq!(all.object, "ambience");
        assert_eq!(all.class_prop_count, 59, "the Ambience_all schema is 59 properties");
        assert_eq!(all.props.len(), 59);
        // the game's own typo, preserved verbatim by the decoder
        match all.prop("rofftop_env") {
            Some(BankValue::Envelope(pts)) => {
                assert_eq!(pts.len(), 4);
                assert_eq!(pts[0], (0.0, 0.0));
            }
            other => panic!("rofftop_env: {other:?}"),
        }
        match all.prop("world_point_distance") {
            Some(BankValue::Float32(v)) => assert_eq!(*v, 90.0),
            other => panic!("world_point_distance: {other:?}"),
        }
        match all.prop("city_vol") {
            Some(BankValue::Float32(v)) => {
                assert_eq!(v.to_bits(), 0x3f66_1859, "city_vol pinned byte-exact (0.898809…)")
            }
            other => panic!("city_vol: {other:?}"),
        }

        let BankChunk::Patch(quad) = &banks[1].1 else { panic!() };
        assert_eq!(quad.object, "base_interior_ambience");
        assert_eq!(quad.class_prop_count, 8);
        assert_eq!(
            quad.prop("bus"),
            Some(&BankValue::StrList(vec![
                "amb".into(),
                "sfx".into(),
                "master".into()
            ]))
        );
        // the second quad player of this bank
        let BankChunk::Patch(quad2) = &banks[2].1 else { panic!() };
        assert_eq!(quad2.object, "e10m04_ambience");
        assert_eq!(quad2.class, "AtCamera_Player_Quad");
        assert_eq!(quad2.class_prop_count, 8);
    }

    /// THE CROSS-CRATE CONSISTENCY TEST: every music patch filename in the
    /// real fixtures is an exact clip name of the committed block-63 index.
    #[test]
    fn every_real_patch_filename_is_a_committed_shard_clip() {
        let clips = shard_clip_names();
        for (file, data) in [("e10", E10), ("e02", E02), ("ambience", AMBIENCE)] {
            for (name, chunk) in decode_banks(data) {
                let BankChunk::Patch(p) = chunk else { continue };
                for clip in p.clip_names() {
                    assert!(
                        clips.contains(clip),
                        "{file} patch {name:?} references clip {clip:?} missing from the shard index"
                    );
                }
            }
        }
    }

    #[test]
    fn cue_stems_reference_committed_clips_or_numbered_variants() {
        // corpus census: 144 stems are exact clip names of the committed
        // block-63 index, 35 more are base names of numbered variants
        // (e.g. e10m04_complete -> e10m04_complete01/02/03); the engine
        // picks the variant at runtime.
        let clips = shard_clip_names();
        let banks = decode_banks(E10);
        for (name, chunk) in &banks {
            let BankChunk::Cue(c) = chunk else { continue };
            for cell in &c.cells {
                let Some(stem) = &cell.stem else { continue };
                let exact = clips.contains(stem);
                let variant = clips
                    .iter()
                    .any(|k| k.len() > stem.len() && k.starts_with(stem) && k[stem.len()..].chars().all(|d| d.is_ascii_digit()));
                assert!(
                    exact || variant,
                    "{name} stem {stem:?} is neither an exact clip nor a numbered-variant base"
                );
                if stem == "e10m04_complete" {
                    assert!(variant, "e10m04_complete resolves through numbered variants");
                }
            }
        }
    }

    #[test]
    fn refusals_are_typed_and_fail_closed() {
        // bad pad — mutate a copy of the cue body's first byte
        let mut bad = E10.to_vec();
        let (_, r) = FileHeader::parse(&bad).unwrap();
        let (chunk, _) = Chunk::parse(&bad, r.pos).unwrap();
        let classic = chunk.classic().unwrap().unwrap();
        assert_eq!(classic.name, "Cue", "e10's first chunk is the cue");
        let body_off = classic.data.as_ptr() as usize - bad.as_ptr() as usize;
        let body = classic.data.to_vec();
        drop(classic);
        bad[body_off] = 1;
        let err = parse_body(&bad[body_off..]).unwrap_err();
        assert!(err.to_string().contains("0x01"), "pad error: {err}");

        // truncation
        for cut in [0, 5, 17, 40] {
            let err = parse_body(&body[..body.len().min(cut)]).unwrap_err();
            assert!(!err.to_string().is_empty());
        }

        // residue: append one junk byte to a valid body -> must be refused
        let ok = parse_body(&body).expect("valid body");
        let mut residue = body.clone();
        residue.push(0xAA);
        let err = parse_body(&residue).unwrap_err();
        assert!(err.to_string().contains("trailing"), "residue error: {err}");
        let _ = ok;

        // kind 3 refused
        let mut kind = body.clone();
        kind[1] = 3;
        let err = parse_body(&kind).unwrap_err();
        assert!(err.to_string().contains("kind"), "kind error: {err}");
    }

    #[test]
    fn empty_cell_body_parses_as_a_bare_cell() {
        // synthesized from the PROVEN EmptyCell shape (class string + 8B id):
        // pad 0 | kind 2 | "t" | f32 150 | T=1 | 1 | id8 | transition "a" + u32 + id8
        // | "main" + id8 | "EmptyCell" + id8 (final item: trailing word omitted
        // is NOT observed for EmptyCell in the corpus — its 8B word is the value).
        let mut b = Vec::new();
        b.push(0u8);
        b.extend_from_slice(&2u32.to_le_bytes());
        for &s in &[1u8, 0, 0, 0, b't', 0] {
            b.push(s);
        }
        b.extend_from_slice(&150.0f32.to_bits().to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        // transition "a"
        b.push(1);
        b.extend_from_slice(&1u32.to_le_bytes());
        b.push(b'a');
        b.push(0);
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&[9; 8]);
        // main
        b.push(1);
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(b"main\0");
        b.extend_from_slice(&[10; 8]);
        // one EmptyCell cell
        b.push(1);
        b.extend_from_slice(&9u32.to_le_bytes());
        b.extend_from_slice(b"EmptyCell\0");
        b.extend_from_slice(&[11; 8]);
        let parsed = parse_body(&b).expect("synthesized EmptyCell cue");
        let BankChunk::Cue(c) = parsed else { panic!() };
        assert_eq!(c.object, "t");
        assert_eq!(c.cells.len(), 1);
        assert!(c.cells[0].is_empty);
        assert!(c.cells[0].stem.is_none());
        assert!(c.cells[0].mgraph_struct.is_none());
    }
}

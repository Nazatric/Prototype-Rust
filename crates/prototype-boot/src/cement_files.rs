//! cement_files — the PC content map: `ContentCementFileDefinition`
//! (M17).
//!
//! `cementfiles.p3d` (338 bytes, install root) is the engine's list
//! of cement-library archives for THIS platform.  Its single content
//! chunk (`ContentCementFileDefinition(1):CementFileDefinition`)
//! carries a nested group/file structure after the `META` marker:
//!
//! ```text
//! META
//! u32 group_count                       (2 in the shipped file)
//! per group:
//!   u32 name_len, [name_len] bytes      ("default", "cached")
//!   u32 file_count
//!   per file: u32 name_len, [name_len] bytes
//! ```
//!
//! ## The PROVEN PC map (from the real 338-byte file)
//!
//! * group `default` — 11 archives, in file order:
//!   `scripts.rcf`, `00audio.rcf`, `01audio.rcf`, `shaders.rcf`,
//!   `art.rcf`, `cells.rcf`, `movies.rcf`, `00woi.rcf`,
//!   `01woi.rcf`, `02woi.rcf`, `03woi.rcf`
//! * group `cached` — 0 entries (a cache-directory marker, not an
//!   archive).
//!
//! ## Platform reconciliation (important)
//!
//! The Xbox 360 build's mount order recovered in Phase 2F
//! (`00cells.rcf → 01cells.rcf → 00art.rcf → 01art.rcf → 02art.rcf`
//! — see `prototype_stream::ArchiveKind`) does NOT match this PC
//! file: the PC map names the world archives **unprefixed**
//! (`cells.rcf`, `art.rcf`) and adds `00audio`/`01audio`,
//! `movies`, four `woi` archives and `cached`.  Both observations
//! are PROVEN for their own platforms; [`crate::streaming`] and
//! `prototype_stream::ArchiveKind` stay 360-flavoured (that is
//! where the mount ORDER evidence lives), while this module is the
//! authoritative PC list.  A PC host should mount what this file
//! says, not what the 360 log says.
//!
//! ## Still UNKNOWN
//!
//! * The pre-META field block (~22 bytes) — it contains what looks
//!   like a content hash (`0x36a4173b`) plus three u32s
//!   (203/203/187 in the shipped file); no name hashes to it and
//!   its semantics are unresolved.  The parse does not depend on
//!   it (the generic `prototype_p3d::ContentChunk::fields` slice
//!   carries it opaquely).
//! * Whether the original mounts the `default` group strictly in
//!   file order (LIKELY — the list is ordered and Radical's
//!   earlier engines mount cement files in list order) or
//!   re-orders by dependency.

#![forbid(unsafe_code)]

use prototype_common::Reader;
use prototype_p3d::ContentChunk;

/// One named group of cement files (`default`, `cached`, …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CementGroup {
    pub name: String,
    /// Archive file names in file order (e.g. `"scripts.rcf"`).
    pub files: Vec<String>,
}

/// The parsed `ContentCementFileDefinition` — the platform's cement
/// library map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CementFileList {
    pub groups: Vec<CementGroup>,
}

/// Read one `u32`-length-prefixed Latin-1 string.
fn read_str(r: &mut Reader) -> Result<String, String> {
    let len = r
        .read_u32()
        .map_err(|e| format!("string length: {e}"))? as usize;
    let bytes = r
        .read_bytes(len)
        .map_err(|e| format!("string bytes (len {len}): {e}"))?;
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

impl CementFileList {
    /// Parse from a decoded content chunk.  Fails loudly when the
    /// chunk is not a `ContentCementFileDefinition`, has no META
    /// section, or the group/file structure does not exactly
    /// consume the META payload (an unconsumed or overrunning parse
    /// means the layout model is wrong — never guess past it).
    pub fn from_content(chunk: &ContentChunk) -> Result<Self, String> {
        let class = chunk
            .names
            .first()
            .map(String::as_str)
            .unwrap_or("");
        if !class.starts_with("ContentCementFileDefinition") {
            return Err(format!(
                "not a ContentCementFileDefinition chunk (names[0] = {class:?})"
            ));
        }
        let meta = chunk
            .meta
            .as_ref()
            .ok_or("CementFileDefinition chunk has no META section")?;

        let mut r = Reader::new(meta.raw);
        let mut groups = Vec::new();
        for gi in 0..meta.count {
            let name = read_str(&mut r)
                .map_err(|e| format!("group {gi} name: {e}"))?;
            let file_count = r
                .read_u32()
                .map_err(|e| format!("group {gi} ({name}) file count: {e}"))?;
            let mut files = Vec::new();
            for fi in 0..file_count {
                let f = read_str(&mut r)
                    .map_err(|e| format!("group {gi} ({name}) file {fi}: {e}"))?;
                files.push(f);
            }
            groups.push(CementGroup { name, files });
        }
        if !r.at_end() {
            return Err(format!(
                "cement list parse left {} unconsumed bytes — layout model wrong",
                r.remaining()
            ));
        }
        Ok(Self { groups })
    }

    /// Group by name (e.g. `"default"`).
    pub fn group(&self, name: &str) -> Option<&CementGroup> {
        self.groups.iter().find(|g| g.name == name)
    }

    /// The `default` group's files, in file order (the archives the
    /// engine mounts).
    pub fn default_files(&self) -> &[String] {
        self.group("default")
            .map(|g| g.files.as_slice())
            .unwrap_or(&[])
    }

    /// Every file of every group (flattened, group order then file
    /// order).
    pub fn all_files(&self) -> Vec<&str> {
        self.groups
            .iter()
            .flat_map(|g| g.files.iter().map(String::as_str))
            .collect()
    }

    /// Total file count across groups.
    pub fn total_files(&self) -> usize {
        self.groups.iter().map(|g| g.files.len()).sum()
    }
}

// ---------------------------------------------------------------------------
// Tests — against the real 338-byte cementfiles.p3d
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

    fn real_list() -> CementFileList {
        let data = fixture("cementfiles.p3d");
        let f = prototype_p3d::P3dFile::parse(&data).expect("parse cementfiles");
        assert_eq!(f.chunks.len(), 1);
        let content = f.chunks[0]
            .content()
            .expect("content decode")
            .expect("is a content chunk");
        CementFileList::from_content(&content).expect("parse cement list")
    }

    #[test]
    fn parses_the_real_pc_cement_map() {
        let list = real_list();
        assert_eq!(list.groups.len(), 2);
        // The `default` group: 11 archives in exact file order.
        assert_eq!(
            list.default_files(),
            [
                "scripts.rcf",
                "00audio.rcf",
                "01audio.rcf",
                "shaders.rcf",
                "art.rcf",
                "cells.rcf",
                "movies.rcf",
                "00woi.rcf",
                "01woi.rcf",
                "02woi.rcf",
                "03woi.rcf",
            ]
        );
        // The `cached` group is an empty marker.
        let cached = list.group("cached").expect("cached group");
        assert!(cached.files.is_empty());
        assert_eq!(list.total_files(), 11);
        // The parse consumed the META payload EXACTLY (checked inside
        // from_content — an under/overrun is an Err).
    }

    #[test]
    fn pc_map_names_the_world_archives_unprefixed() {
        // The platform reconciliation: the PC map says `cells.rcf` /
        // `art.rcf`; the Xbox 360 build (Phase 2F) says
        // `00cells/01cells/00art/01art/02art`.  Both are PROVEN for
        // their own platform — this test pins the PC side so a
        // future edit cannot silently merge the two.
        let list = real_list();
        let files = list.all_files();
        assert!(files.contains(&"cells.rcf"), "PC name");
        assert!(files.contains(&"art.rcf"), "PC name");
        assert!(
            !files.iter().any(|f| f.starts_with("00cells")
                || f.starts_with("01cells")
                || f.starts_with("00art")
                || f.starts_with("01art")
                || f.starts_with("02art")),
            "the PC map must not contain 360-prefixed archive names"
        );
        // And prototype_stream's ArchiveKind is the 360-flavoured
        // model (5 archives) — deliberately different from this PC
        // list (11).
        assert_eq!(prototype_stream::ArchiveKind::ALL_IN_MOUNT_ORDER.len(), 5);
    }

    #[test]
    fn rejects_non_cement_content_chunks() {
        let data = fixture("skuinfo.p3d");
        let f = prototype_p3d::P3dFile::parse(&data).unwrap();
        let content = f.chunks[0].content().unwrap().unwrap();
        let err = CementFileList::from_content(&content).unwrap_err();
        assert!(err.contains("not a ContentCementFileDefinition"), "{err}");
    }

    #[test]
    fn rejects_truncated_group_payloads() {
        // Take the real chunk and truncate the META raw payload —
        // the parse must fail instead of guessing.
        let data = fixture("cementfiles.p3d");
        let f = prototype_p3d::P3dFile::parse(&data).unwrap();
        let mut content = f.chunks[0].content().unwrap().unwrap();
        let mut meta = content.meta.take().expect("meta");
        // Rebuild with a short raw payload (drop the last group's
        // bytes).
        let cut = meta.raw.len() - 12;
        meta.raw = &meta.raw[..cut];
        content.meta = Some(meta);
        let err = CementFileList::from_content(&content).unwrap_err();
        assert!(
            err.contains("file count")
                || err.contains("string")
                || err.contains("unconsumed"),
            "{err}"
        );
    }

    #[test]
    fn the_list_feeds_the_content_pipeline() {
        // Mount the two archives that are physically present in the
        // partial install, using the names THE PC MAP gives them,
        // and load real content through the pipeline.
        let list = real_list();
        let scripts = fixture("scripts.rcf");
        let shaders = fixture("shaders.rcf");
        assert!(list.default_files().iter().any(|f| f == "scripts.rcf"));
        assert!(list.default_files().iter().any(|f| f == "shaders.rcf"));

        let mut pipeline = crate::ContentPipeline::new();
        pipeline
            .mount_rcf("scripts.rcf", &scripts)
            .expect("mount scripts.rcf");
        pipeline
            .mount_rcf("shaders.rcf", &shaders)
            .expect("mount shaders.rcf");
        let fre = pipeline
            .load(r"scripts\frevents.lua")
            .expect("frevents through the PC-named mount");
        assert_eq!(fre.kind, crate::PayloadKind::Lua51);
    }
}

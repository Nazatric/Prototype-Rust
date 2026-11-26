//! world_data — the shared loader for the REAL streamed-world
//! evidence bins (blocks 110/111): walks every decompressed cells.rcf
//! member in a directory through the blocks-18/41 PROVEN decoders
//! (`prototype_p3d::world::scan_world_member_opts`) and either
//! flattens the placed GameObjects to `MapDot`s (the point-cloud
//! takes) or hands the whole scan to the caller (geometry takes).

#![forbid(unsafe_code)]

use prototype_p3d::world::{scan_world_member_opts, CellObject, ObjectPayload, ScanOptions};
use prototype_render::world_map::{DotKind, MapDot};

/// One scanned member, for callers that need more than the dots.
pub struct MemberScan {
    /// RCF hash (from the file stem).
    pub hash: u32,
    /// The full `WorldIndexEntry` (counts, bbox, retained objects).
    pub scanned: prototype_p3d::world::WorldIndexEntry,
}

/// Scan every non-empty `*.p3d` in `dir`.  Returns `(scans, dots)`
/// where `dots` are every retained placement as a [`MapDot`].
pub fn load_world(dir: &str) -> (Vec<MemberScan>, Vec<MapDot>) {
    let mut scans = Vec::new();
    let mut dots = Vec::new();
    for entry in std::fs::read_dir(dir).expect("cells_world dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("p3d") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("00000000");
        let hash = u32::from_str_radix(&stem[..8.min(stem.len())], 16).unwrap_or(0);
        let p3d = std::fs::read(&path).expect("member file");
        if p3d.len() <= 12 {
            continue; // empty member
        }
        let opts = ScanOptions {
            retain_objects: true,
            ..Default::default()
        };
        let scanned = scan_world_member_opts(hash, &p3d, opts);
        for obj in &scanned.retained_objects {
            if let Some(pl) = obj.placement {
                dots.push(MapDot {
                    pos: pl.pos,
                    kind: classify(obj),
                });
            }
        }
        scans.push(MemberScan { hash, scanned });
    }
    (scans, dots)
}

/// Load only the placed-object dots (the block-110 point-cloud takes).
pub fn load_dots(dir: &str) -> (Vec<MapDot>, usize, usize) {
    let (scans, dots) = load_world(dir);
    let members = scans.len();
    let objects: usize = scans.iter().map(|s| s.scanned.objects as usize).sum();
    (dots, members, objects)
}

/// Dot family for a decoded GameObject.
pub fn classify(obj: &CellObject) -> DotKind {
    match &obj.payload {
        Some(ObjectPayload::LightDecal(_)) => DotKind::LightDecal,
        Some(ObjectPayload::AmbientExclusion(_)) => DotKind::AmbientExclusion,
        Some(ObjectPayload::WebLocator(_)) => DotKind::WebLocator,
        _ => DotKind::Spawn,
    }
}

/// The block-17 PROVEN diffuse chain, resolved against ONE member's
/// own chunks: `mesh.name` → ShaderChunk (`0x00011015`) of the same
/// name → its `color` param (0x11016 name/value) → TextureChunk
/// (`0x00019000`) of that name → DDS decoded by `dxt::decode_dds_mips`.
/// Meshes whose chain does not resolve get `None` (the rasteriser's
/// documented white-fallback draw).  Returns one entry per mesh, in
/// order.
pub fn resolve_mesh_diffuses(
    buf: &[u8],
    meshes: &[prototype_render::scene::SceneMesh],
) -> Vec<Option<(Vec<u8>, u32, u32)>> {
    use prototype_p3d::geometry::{
        CHUNK_TYPE_SHADER, CHUNK_TYPE_TEXTURE, decode_shader_chunk, decode_texture,
    };

    let tops = match prototype_p3d::geometry::top_level_chunks(buf) {
        Some(t) => t,
        None => return vec![None; meshes.len()],
    };

    // shader-name → `color` texture name
    let mut color_of: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for c in &tops {
        if c.chunk_type == CHUNK_TYPE_SHADER {
            if let Ok(s) = decode_shader_chunk(buf, c.offset, c.size) {
                for p in &s.params {
                    if let (Some(pn), Some(pv)) = (&p.name, &p.value) {
                        if pn.eq_ignore_ascii_case("color") {
                            color_of.insert(s.name.clone(), pv.clone());
                        }
                    }
                }
            }
        }
    }

    // texture-name → decoded RGBA surface (lazy: decode each at most once)
    let tex_chunks: Vec<(String, usize, usize)> = tops
        .iter()
        .filter(|c| c.chunk_type == CHUNK_TYPE_TEXTURE)
        .filter_map(|c| {
            decode_texture(buf, c.offset, c.size)
                .ok()
                .map(|t| (t.name, c.offset, c.size))
        })
        .collect();

    let mut decoded: std::collections::HashMap<String, Option<(Vec<u8>, u32, u32)>> =
        std::collections::HashMap::new();

    let mut out = Vec::with_capacity(meshes.len());
    for m in meshes {
        let resolved = color_of
            .get(&m.name)
            .cloned()
            .or_else(|| {
                // honest fallback chain: some world meshes bind the
                // shader under a hash the mesh name carries directly
                m.name.clone().into()
            })
            .and_then(|tex_name| {
                let key = tex_name.to_lowercase();
                if let Some(hit) = decoded.get(&key) {
                    return hit.clone();
                }
                let hit = tex_chunks
                    .iter()
                    .filter(|(n, _, _)| n.eq_ignore_ascii_case(&tex_name))
                    .find_map(|(_, off, sz)| {
                        let t = decode_texture(buf, *off, *sz).ok()?;
                        let s = prototype_render::dxt::decode_dds_mips(&t.dds).ok()?;
                        let first = s.first()?;
                        Some((first.rgba.clone(), first.width, first.height))
                    });
                decoded.insert(key, hit.clone());
                hit
            });
        out.push(resolved);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed cell-82 texture-comm fixture carries exactly one
    /// texture (`communicationsarray001_diffuse.dds`) and no shapes;
    /// the resolver must build the index and return one (unresolved)
    /// slot per requested mesh without panicking.
    #[test]
    fn resolve_handles_texture_only_member() {
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../prototype-bin/tests/fixtures/cells_section_textured.p3d"
        );
        let buf = std::fs::read(fixture)
            .unwrap_or_else(|e| panic!("read {fixture}: {e}"));
        // no meshes requested → empty resolution list
        let out = resolve_mesh_diffuses(&buf, &[]);
        assert!(out.is_empty());

        // one synthetic unbound mesh → one None slot (white fallback)
        let mesh = prototype_render::scene::SceneMesh {
            name: "no_such_shader".into(),
            desc: 0,
            positions: vec![[0.0; 3]; 3],
            normals: Vec::new(),
            uvs: Vec::new(),
            tangents: Vec::new(),
            indices: vec![0, 1, 2],
            bound_box: None,
            bound_sphere: None,
            texture_dds: None,
            extra_textures: Vec::new(),
        };
        let out = resolve_mesh_diffuses(&buf, &[mesh]);
        assert_eq!(out.len(), 1);
        assert!(out[0].is_none(), "unbound mesh falls back to white");
    }
}

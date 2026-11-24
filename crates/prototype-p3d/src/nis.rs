//! nis — REAL in-engine NIS cutscene track decoders (block 77).
//!
//! ## Where the tracks live (PROVEN against the live release, this block)
//!
//! The game's NIS cutscene data streams off `art.rcf` (the release's
//! real-art archive): directory + name table parsed with the proven
//! cement layout gives **2 601 / 2 601 hash-verified names**, among
//! them **452 NIS-bearing members** in three families — `art\nis\NIS_*`
//! (301, the in-engine cutscenes incl. `NIS_0_3_a.p3d.rz` = the
//! `nis0-3a` boot_strap scene id), `art\nis\MNIS_*` (54) and
//! `art\nis\MB_*` (76, the mission-briefing families the block-69
//! audio patches name), plus `art\missions\NIS_Test\` (the dev test
//! scene).  `.rz` members decompress (PROVEN `prototype_common::zlib`
//! path) to plain P3Ds byte-verified against the RZ header's declared
//! size.
//!
//! ## The scene file's own structure (NIS_0_3_a.p3d, 14 201 283 B)
//!
//! 158 top-level chunks: `0x00019000` ×59 (11.3 MB textures, e.g. the
//! morgue-set `genetech_morgue_*` DDS names), `0x00011015` ×30
//! (shaders), `0x00010000/0x00010001` ×43 (meshes), **`0x00023000`
//! ×10 = SKELETON chunks** (the cutscene's character rigs:
//! `nis_alex_shotSkeleton`, `NIS_blackwatch_marine_heliSkeleton`,
//! `nis_bw_scientist_2008Skeleton`, … each with a `Motion_Root` bone),
//! `0x00018202` ×4 (TextBibleData — the scene's own subtitles, keys
//! `nis0-3a_01`… in english/spanish/french/italian), and the modifier
//! family `0x00121000` / `0x00023000` / `0x00123000`.
//!
//! The per-shot TRACK data streams in the companion members
//! (`NIS_0_3_a0/a1/...p3d.rz`): P3Ds that are essentially PURE
//! `0x00121000` track chunks — camera takes named
//! `CAM_NisCam_Take 0NNNN` plus `PTRN_*` prop tracks.
//!
//! ## The camera-track layout (typed from the real bytes; framing
//! scan-based, exact fields pinned by the tests below)
//!
//! Each `0x00121000` chunk: `[u32 0][u8 slot][slot bytes: name]`
//! (slot = strlen+1 rounded up to 4; `CAM_NisCam_Take 00100` → 24),
//! a 4-byte kind (`CAM\0` / `PTRN`), two f32s (observed 237.25, 30.0
//! — LIKELY time-base/duration), then nested sub-chunks carrying the
//! named sections `TRAN` (position keys), `LOOK` (target keys),
//! `FOV`, `ROT`, `LVY`, `BWL`, `SJL`.  Each section: `[u32 count]`
//! then a `count × u16` index/time table, then packed f32 key data —
//! the key payload reads as clean f32 vec3 triples (smooth dolly
//! paths), so this decoder extracts the vec3 stream scan-based (up to
//! the next section marker) exactly like the committed evidence
//! script `scripts/m77_nis_camera_plot.py`, which produced
//! `media/12_nis_0_3_a_real_camera_paths.png` from these same bytes.

#![forbid(unsafe_code)]

/// One decoded camera take from a REAL NIS track member.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraTake {
    /// The real track name, e.g. `"CAM_NisCam_Take 00100"`.
    pub name: String,
    /// The chunk's total byte size.
    pub size: u32,
    /// The `TRAN` (camera position) f32 vec3 keys, in track order.
    pub tran_keys: Vec<[f32; 3]>,
    /// The `LOOK` (camera target) f32 vec3 keys (may be empty when the
    /// section does not decode).
    pub look_keys: Vec<[f32; 3]>,
}

/// The modifier/track chunk family type word.
pub const CHUNK_TYPE_TRACK: u32 = 0x00121000;

/// Walk a P3D's top-level chunks and decode every camera take.
///
/// `p3d` is a decompressed NIS track member (the `.rz` members of
/// `art.rcf`'s `art\nis\` family decompress to these with the proven
/// zlib path).  Returns the takes in file order.
pub fn camera_takes(p3d: &[u8]) -> Vec<CameraTake> {
    let mut out = Vec::new();
    let mut pos = 12usize; // skip the P3D file header (proven 12 B)
    while pos + 12 <= p3d.len() {
        let (ctype, _uid, size) = read_prelude(p3d, pos);
        let size = size as usize;
        if size < 12 || pos + size > p3d.len() {
            break; // corrupt tail — stop (fail-soft at the walk level)
        }
        if ctype == CHUNK_TYPE_TRACK {
            let chunk = &p3d[pos..pos + size];
            let payload = &chunk[12..];
            if payload.len() > 5 && payload[0..4] == [0, 0, 0, 0] {
                let slot = payload[4] as usize;
                if slot > 0 && 5 + slot <= payload.len() {
                    let name_bytes = payload[5..5 + slot].split(|&b| b == 0).next().unwrap_or(&[]);
                    if let Ok(name) = std::str::from_utf8(name_bytes) {
                        if name.starts_with("CAM_") {
                            let tran_keys = section_vec3s(chunk, b"TRAN");
                            let look_keys = section_vec3s(chunk, b"LOOK");
                            out.push(CameraTake {
                                name: name.to_string(),
                                size: size as u32,
                                tran_keys,
                                look_keys,
                            });
                        }
                    }
                }
            }
        }
        pos += size;
    }
    out
}

fn read_prelude(buf: &[u8], pos: usize) -> (u32, u32, u32) {
    let t = u32_le(buf, pos);
    let u = u32_le(buf, pos + 4);
    let s = u32_le(buf, pos + 8);
    (t, u, s)
}

fn u32_le(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn f32_le(buf: &[u8], off: usize) -> f32 {
    f32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

/// The named sections this decoder understands.
const SECTION_MARKERS: [&[u8]; 7] = [b"LOOK", b"TRAN", b"FOV", b"ROT", b"LVY", b"BWL", b"SJL"];

/// Extract the f32 vec3 key stream of one named section (scan-based
/// framing — see the module docs).
fn section_vec3s(chunk: &[u8], marker: &[u8]) -> Vec<[f32; 3]> {
    let Some(start) = find_marker(chunk, marker, 40) else {
        return Vec::new();
    };
    let end = SECTION_MARKERS
        .iter()
        .filter_map(|m| find_marker(chunk, m, 40))
        .filter(|&i| i > start)
        .min()
        .unwrap_or(chunk.len());
    if end <= start + 8 {
        return Vec::new();
    }
    let count = u32_le(chunk, start + marker.len());
    if count == 0 || count > 1_000_000 {
        return Vec::new();
    }
    let base = start + marker.len() + 4 + (count as usize) * 2; // skip the u16 index table
    if base >= end {
        return Vec::new();
    }
    let n = (end - base) / 12;
    let mut keys = Vec::with_capacity(n);
    for k in 0..n {
        let off = base + k * 12;
        let key = [
            f32_le(chunk, off),
            f32_le(chunk, off + 4),
            f32_le(chunk, off + 8),
        ];
        // Truncate at the first exact-origin OR non-finite key after
        // the first (the scan can overrun into the next section's
        // framing bytes — the same cleanup the evidence plot applies,
        // plus the finiteness guard the framing bytes require).
        if k > 0 && (key == [0.0, 0.0, 0.0] || key.iter().any(|v| !v.is_finite())) {
            break;
        }
        keys.push(key);
    }
    keys
}

fn find_marker(buf: &[u8], marker: &[u8], from: usize) -> Option<usize> {
    if from >= buf.len() {
        return None;
    }
    buf[from..]
        .windows(marker.len())
        .position(|w| w == marker)
        .map(|i| i + from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join(name)
            .pipe_read()
    }
    trait PipeRead {
        fn pipe_read(&self) -> Vec<u8>;
    }
    #[cfg(test)]
    impl PipeRead for std::path::PathBuf {
        fn pipe_read(&self) -> Vec<u8> {
            std::fs::read(self).unwrap_or_else(|e| panic!("missing fixture {self:?}: {e}"))
        }
    }

    /// THE REAL INTRO CUTSCENE'S CAMERA TAKES (art.rcf → art\nis\
    /// → NIS_0_3_a0.p3d.rz, decompressed): 2 takes, real names, real
    /// keyframe paths — pinned to the exact counts the live decode
    /// produced (`scripts/m77_nis_camera_plot.py` on the same bytes).
    #[test]
    fn nis_0_3_a0_camera_takes_decode_from_the_real_member() {
        let p3d = fixture("art_nis_nis_0_3_a0_camera_takes.p3d");
        assert_eq!(&p3d[..4], b"P3D\xff");
        let takes = camera_takes(&p3d);
        assert_eq!(takes.len(), 2, "a0 holds takes 00100 + 00101");
        let t0 = &takes[0];
        assert_eq!(t0.name, "CAM_NisCam_Take 00100");
        assert_eq!(t0.size, 9644);
        assert_eq!(t0.tran_keys.len(), 131, "the take's TRAN key count");
        assert_eq!(t0.look_keys.len(), 207, "the take's LOOK key count");
        // The first REAL keyframe (pinned from the live decode).
        let first = t0.tran_keys[0];
        assert!((first[0] - -0.962019).abs() < 1e-5, "first x = {}", first[0]);
        assert!((first[1] - 3.83344).abs() < 1e-4, "first y = {}", first[1]);
        assert!((first[2] - 6.571).abs() < 1e-4, "first z = {}", first[2]);
        // The dolly END (a real move, not a static camera).
        let last = t0.tran_keys[t0.tran_keys.len() - 1];
        assert!((last[0] - -3.083).abs() < 1e-2, "last x = {}", last[0]);
        assert_ne!(first, last, "the camera moves");
        // Every key is finite (no garbage from a framing overrun).
        assert!(t0.tran_keys.iter().all(|k| k.iter().all(|v| v.is_finite())));
        assert!(t0.look_keys.iter().all(|k| k.iter().all(|v| v.is_finite())));
        // The second take.
        let t1 = &takes[1];
        assert_eq!(t1.name, "CAM_NisCam_Take 00101");
        assert_eq!(t1.tran_keys.len(), 53);
        assert_eq!(t1.look_keys.len(), 55);
    }

    /// The a1 companion: 3 more takes of the same scene (00110-00112).
    #[test]
    fn nis_0_3_a1_camera_takes_decode_from_the_real_member() {
        let p3d = fixture("art_nis_nis_0_3_a1_camera_takes.p3d");
        let takes = camera_takes(&p3d);
        assert_eq!(takes.len(), 3);
        let names: Vec<&str> = takes.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            ["CAM_NisCam_Take 00110", "CAM_NisCam_Take 00111", "CAM_NisCam_Take 00112"]
        );
        assert_eq!(takes[0].tran_keys.len(), 60);
        assert_eq!(takes[1].tran_keys.len(), 97);
        assert_eq!(takes[2].tran_keys.len(), 5);
        // All paths finite + non-degenerate (the camera really moves).
        for t in &takes {
            assert!(t.tran_keys.iter().all(|k| k.iter().all(|v| v.is_finite())));
            let len2: f32 = t
                .tran_keys
                .windows(2)
                .map(|w| {
                    let d = [w[1][0] - w[0][0], w[1][1] - w[0][1], w[1][2] - w[0][2]];
                    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
                })
                .sum();
            assert!(len2 > 0.0, "take {} has a non-zero path", t.name);
        }
    }

    /// Fail-soft battery: truncated / garbage inputs yield empty, never
    /// panic (the walk-level contract).
    #[test]
    fn camera_takes_fail_soft_on_garbage() {
        assert!(camera_takes(&b""[..]).is_empty());
        assert!(camera_takes(&vec![0u8; 11]).is_empty());
        let mut junk = b"P3D\xff".to_vec();
        junk.extend_from_slice(&12u32.to_le_bytes());
        junk.extend_from_slice(&0u32.to_le_bytes());
        junk.extend_from_slice(&[0u8; 64]);
        assert!(camera_takes(&junk).is_empty());
    }
}

//! Block 87 (rebuild of the recycle-lost blocks 83/85) — REAL
//! `0x0012_1000` animation-track decoder for alex.p3d, plus the
//! anim→skeleton pose pipeline that drives skinning.
//!
//! THE FORMAT (re-derived and validated live this block, 634/634
//! tracks decode with the exact kind census 533 PTRN / 67 CAM /
//! 34 EXP):
//!
//! An anim track is a FLAT SEQUENCE of sibling chunks starting at a
//! `0x0012_1000` header chunk:
//!
//! ```text
//! header chunk (0x0012_1000):
//!   [u32 type][u32 size][u32 span][u32 0][u8 namelen][pascal name]
//!   [4-B kind 'PTRN'/'CAM\0'/'EXP\0'][f32 frame_count][f32 fps]
//!   [u32 cyclic]
//! ```
//!
//! The first body u32 (`span`) is the TOTAL byte length of the whole
//! track sequence — verified on 633/634 tracks (the last track has no
//! successor).  The remaining chunks until `offset + span` are:
//!
//! ```text
//! group (0x0012_1001):
//!   [u32 outer][u32 0][u8 namelen][pascal name][u32 channel_id]
//!   [u32 sub_count][u32 chan_type][u32 inner][u32 outer2]
//!   (the channel reference is REDUNDANT — the channel chunk itself
//!   follows the group in the sequence; we associate by order)
//! ROT channel (0x0012_1112):
//!   [u32 outer][u32 flag=1][4-B marker 'ROT\0'][u32 count]
//!   [u16 × count key indices][keys: 6 B each = 3×i16/32767
//!   smallest-three quaternion, w = sqrt(1-x²-y²-z²)]
//! TRAN channel (0x0012_1119):
//!   [u32 outer][u32 flag=0][4-B marker 'TRAN'][u32 count]
//!   [u16 × count key indices][keys: 6 B each = 3×IEEE f16 meters,
//!   ABSOLUTE local translation]
//! trailer (0x0012_1110): 20 B, skipped
//! ```
//!
//! BLOCK 90 — BOTH remaining variants CRACKED against real bytes (the
//! committed alex_loco_run_n fixture):
//!
//! ```text
//! 0x0012_1114 compact ROT (34 chunks on the run track):
//!   [u32 A][u32 interpolate][4-B param 'ROT\0'][u32 count]
//!   [u16 × count key frames][keys: 3 B each = 3×i8/127,
//!   w = sqrt(1 - x² - y² - z²)]
//!   Verified: 29 B chunks (1 key) + 119 B chunks (19 keys); EVERY key
//!   decodes to a unit quaternion (probe: 19/19 within 2%); targets
//!   Ball_L/R, Shoulder_Con_L/R, all 30 finger bones, Jaw, EyeLid — all
//!   present in alex_reg_body_skeleton (67 joints).
//!
//! 0x0012_1102 Vector-1-DOF root-motion TRAN (1 chunk on Motion_Root):
//!   [u32 A][u32 interpolate][4-B param 'TRAN'][u16 dyn_index 0=x/1=y/2=z]
//!   [f32 × 3 constants][u32 count][u16 × count key frames]
//!   [f32 × count values]
//!   Verified: A=70, dyn=2 (z), constants (0.0616, 1.0, -0.2755),
//!   keys @ frames [0, 18] → values [-0.2755, -3.870] — the character
//!   travels 3.59 m per 19-frame cycle = 5.7 m/s (a real run speed).
//! ```
//!
//! Channel→group association is EXPLICIT in each group's tail words
//! [idx, sub_count, chan_uid, inner_size, outer_size, flag] (block 90
//! probe) — the walker's by-order association matches it on real data.
//!
//! pose_world_matrices applies rot_compact only where no primary ROT
//! exists (no run-track group carries both), and a DOF channel sets the
//! animated axis of the bone's local translation.  The new
//! pose_world_matrices_root_motion accumulates the DOF travel across
//! cycles (standard motion-extraction loop) so the character runs
//! FORWARD instead of snapping back at each loop point.

use crate::geometry::ArtError;
use crate::skin::{mat4_mul, Skeleton};

/// Top-level anim-track header chunk type.
pub const CHUNK_TYPE_ANIM_TRACK: u32 = 0x0012_1000;
/// Per-bone group chunk type.
pub const CHUNK_TYPE_ANIM_GROUP: u32 = 0x0012_1001;
/// ROT channel chunk (smallest-three quat, 6 B/key).
pub const CHUNK_TYPE_ROT: u32 = 0x0012_1112;
/// TRAN channel chunk (3×f16, 6 B/key).
pub const CHUNK_TYPE_TRAN: u32 = 0x0012_1119;
/// Trailer chunk (20 B, skipped).
pub const CHUNK_TYPE_TRAILER: u32 = 0x0012_1110;
/// Compact ROT variant (3×i8/127 keys, 3 B/key) — decoded block 90.
pub const CHUNK_TYPE_ROT_COMPACT: u32 = 0x0012_1114;
/// Vector-1-DOF root-motion TRAN variant — decoded block 90.
pub const CHUNK_TYPE_TRAN_F32: u32 = 0x0012_1102;

/// Animation kind word (the 4 bytes after the track name).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimKind {
    /// Pose animation ('PTRN').
    Ptrn,
    /// Camera animation ('CAM\0').
    Cam,
    /// Expression animation ('EXP\0').
    Exp,
    Unknown([u8; 4]),
}

impl AnimKind {
    fn from_word(w: [u8; 4]) -> Self {
        match &w {
            b"PTRN" => AnimKind::Ptrn,
            b"CAM\0" => AnimKind::Cam,
            b"EXP\0" => AnimKind::Exp,
            _ => AnimKind::Unknown(w),
        }
    }

    /// Short tag for reports ("PTRN"/"CAM"/"EXP"/"????").
    pub fn tag(&self) -> &'static str {
        match self {
            AnimKind::Ptrn => "PTRN",
            AnimKind::Cam => "CAM",
            AnimKind::Exp => "EXP",
            AnimKind::Unknown(_) => "UNK",
        }
    }
}

/// One animation key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Key {
    /// Rotation key: smallest-three quaternion (unit length).
    Rot { x: f32, y: f32, z: f32, w: f32 },
    /// Translation key: meters, ABSOLUTE local translation.
    Tran { x: f32, y: f32, z: f32 },
}

impl Key {
    /// Block 94: interpolate between two bracketing keys at fraction
    /// `t` (the sparse-table bracket lookup).  Rotations SLERP with
    /// double-cover handling (dot < 0 negates the second quat so the
    /// interpolation takes the SHORT arc); translations lerp.
    /// Mismatched variants fall back to `self`.  `t == 0.0`/`1.0`
    /// return the exact endpoint keys bit-identically, so sequential
    /// identity tables produce EXACTLY the old direct-key values.
    pub fn lerped_to(&self, other: &Key, t: f32) -> Key {
        if t <= 0.0 {
            return *self;
        }
        if t >= 1.0 {
            return *other;
        }
        match (self, other) {
            (
                Key::Rot { x: ax, y: ay, z: az, w: aw },
                Key::Rot { x: bx, y: by, z: bz, w: bw },
            ) => {
                // SLERP with double-cover handling (the standard
                // Shoemake path via the sin/sinc form; for near-parallel
                // quats fall back to normalized lerp, which is what the
                // engine's fixed-point path approximates anyway).
                let mut bx = *bx;
                let mut by = *by;
                let mut bz = *bz;
                let mut bw = *bw;
                let mut dot = ax * bx + ay * by + az * bz + aw * bw;
                if dot < 0.0 {
                    bx = -bx;
                    by = -by;
                    bz = -bz;
                    bw = -bw;
                    dot = -dot;
                }
                if dot > 0.9995 {
                    // Normalized lerp (numerically safe near-parallel).
                    let (x, y, z, w) = (
                        ax + (bx - ax) * t,
                        ay + (by - ay) * t,
                        az + (bz - az) * t,
                        aw + (bw - aw) * t,
                    );
                    let inv = 1.0 / (x * x + y * y + z * z + w * w).max(1e-12).sqrt();
                    return Key::Rot { x: x * inv, y: y * inv, z: z * inv, w: w * inv };
                }
                let theta = dot.acos();
                let sin_t = theta.sin();
                let wa = ((1.0 - t) * theta).sin() / sin_t;
                let wb = (t * theta).sin() / sin_t;
                Key::Rot {
                    x: ax * wa + bx * wb,
                    y: ay * wa + by * wb,
                    z: az * wa + bz * wb,
                    w: aw * wa + bw * wb,
                }
            }
            (
                Key::Tran { x: ax, y: ay, z: az },
                Key::Tran { x: bx, y: by, z: bz },
            ) => Key::Tran {
                x: ax + (bx - ax) * t,
                y: ay + (by - ay) * t,
                z: az + (bz - az) * t,
            },
            _ => *self,
        }
    }
}

/// One decoded channel (a key-index table + key list).
#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    pub count: u32,
    pub key_indices: Vec<u16>,
    pub keys: Vec<Key>,
}

/// Vector-1-DOF root-motion channel (0x0012_1102, block 90).
#[derive(Debug, Clone, PartialEq)]
pub struct DofChannel {
    /// Which component the channel drives: 0 = x, 1 = y, 2 = z.
    pub dyn_index: u16,
    /// Constant values of the three components (RECORDED; the pose
    /// pipeline drives only the animated axis over the REST translation —
    /// block 92 CORRECTION: on the real run track the constants do NOT
    /// equal the Motion_Root rest translation, x differs (constant
    /// 0.06216 vs rest 0.0), so clips must fold rest, not constants).
    pub constants: [f32; 3],
    /// Key frame positions (track frame space, u16).
    pub frames: Vec<u16>,
    /// Key values of the animated component (f32, meters).
    pub values: Vec<f32>,
}

impl DofChannel {
    /// Linear interpolation at (integer) frame `f`, clamped outside
    /// the key span.  `None` only when the channel holds no keys.
    pub fn value_at_frame(&self, f: usize) -> Option<f32> {
        if self.frames.is_empty() || self.values.is_empty() {
            return None;
        }
        let last = self.frames.len().min(self.values.len()) - 1;
        let fu = self.frames[last] as usize;
        if f <= self.frames[0] as usize {
            return Some(self.values[0]);
        }
        if f >= fu {
            return Some(self.values[last]);
        }
        for k in 0..last {
            let a = self.frames[k] as usize;
            let b = self.frames[k + 1] as usize;
            if f >= a && f <= b {
                let t = if b > a { (f - a) as f32 / (b - a) as f32 } else { 0.0 };
                return Some(self.values[k] + (self.values[k + 1] - self.values[k]) * t);
            }
        }
        Some(self.values[last])
    }
}

impl Channel {
    /// Clamped key lookup at (integer) frame `f`: the file stores a
    /// u16 per key mapping into the key list; frames beyond the
    /// table clamp to the last entry.
    ///
    /// BLOCK 94 CORRECTION (the Spine_2 discovery): the u16 table is
    /// each KEY'S FRAME POSITION — the same `Frames u16[N]` semantic
    /// PROVEN block 90 on the 0x121102 DOF channel (whose
    /// `value_at_frame` brackets and lerps over exactly such a table),
    /// NOT a frame→key-index map.  On sequential identity tables
    /// (table[i] == i, e.g. the run track's primary channels) the old
    /// frame→index read and this read agree EXACTLY, so every
    /// previously rendered track is bit-identical.  On sparse tables
    /// (the run track's Spine_2 carries 18 keys at frames 0..16 + 18:
    /// frame 17 holds NO key) the old read indexed `keys[table[f]]`
    /// past the key list, returned None, and the pose pipeline
    /// silently dropped the joint to its REST rotation — a one-frame
    /// pop every cycle.  This read BRACKETS frame 17 between the keys
    /// at frames 16 and 18 and lerps t = 0.5 (rotations SLERP with
    /// double-cover handling; translations lerp), matching the
    /// engine's continuous sampling.
    pub fn key_at_frame(&self, f: usize) -> Option<Key> {
        if self.keys.is_empty() || self.key_indices.is_empty() {
            return None;
        }
        let n = self.keys.len().min(self.key_indices.len());
        let frames = &self.key_indices[..n];
        let keys = &self.keys[..n];
        // Outside the key span: clamp to the end keys (the old clamped
        // behaviour, now expressed on frame positions).
        if f <= frames[0] as usize {
            return Some(keys[0]);
        }
        let last = frames[n - 1] as usize;
        if f >= last {
            return Some(keys[n - 1]);
        }
        for k in 0..n - 1 {
            let a = frames[k] as usize;
            let b = frames[k + 1] as usize;
            if f >= a && f <= b {
                let t = if b > a { (f - a) as f32 / (b - a) as f32 } else { 0.0 };
                return Some(keys[k].lerped_to(&keys[k + 1], t));
            }
        }
        Some(keys[n - 1])
    }
}

/// One per-bone group: the bone name plus whichever channels the
/// sequence associates with it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnimGroup {
    pub bone_name: String,
    pub rot: Option<Channel>,
    pub tran: Option<Channel>,
    /// Compact-ROT channel (0x121114) — block 90.  Applied by the pose
    /// pipeline only where the group carries NO primary 0x121112 ROT
    /// (no real run-track group carries both).
    pub rot_compact: Option<Channel>,
    /// Vector-1-DOF root-motion channel (0x121102) — block 90.
    pub dof: Option<DofChannel>,
}

/// One decoded anim track.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimTrack {
    pub name: String,
    pub kind: AnimKind,
    pub frame_count: f32,
    pub fps: f32,
    pub cyclic: u32,
    pub groups: Vec<AnimGroup>,
}

/// Every top-level 0x0012_1000 track in the file: (offset, span, name).
///
/// The span comes from the header's first body u32 and covers the
/// WHOLE track sequence (header + groups + channels + trailers).
pub fn find_anim_tracks(buf: &[u8]) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 8 <= buf.len() {
        let t = u32_at(buf, off);
        let sz = u32_at(buf, off + 4) as usize;
        if sz < 12 || off + sz > buf.len() {
            break;
        }
        if t == CHUNK_TYPE_ANIM_TRACK {
            // header body: [u32 span][u32 0][u8 namelen][pascal name]…
            let span = u32_at(buf, off + 8) as usize;
            let name = buf
                .get(off + 17..off + 17 + 64)
                .map(|b| b.split(|&c| c == 0).next().unwrap_or(b""))
                .unwrap_or(b"");
            let name = String::from_utf8_lossy(name).into_owned();
            out.push((off, span.min(buf.len() - off), name));
        }
        off += sz;
    }
    out
}

/// Decode the track whose sequence starts at `offset` and spans `size`
/// bytes (as [`find_anim_tracks`] reports).
pub fn decode_anim_track(buf: &[u8], offset: usize, size: usize) -> Result<AnimTrack, ArtError> {
    let end = (offset + size).min(buf.len());
    if offset + 8 > end {
        return Err(ArtError::Truncated { offset, need: 8, got: buf.len() - offset });
    }
    if u32_at(buf, offset) != CHUNK_TYPE_ANIM_TRACK {
        return Err(ArtError::UnexpectedData {
            offset,
            why: format!("expected anim-track chunk {CHUNK_TYPE_ANIM_TRACK:#x}"),
        });
    }
    // header body: [u32 span][u32 0][u8 namelen][pascal name][kind 4B][f32 frames][f32 fps][u32 cyclic]
    let span = u32_at(buf, offset + 8) as usize;
    let end = (offset + span).min(end);
    let name_len = buf[offset + 16] as usize;
    let name_start = offset + 17;
    let name_end = name_start + name_len;
    if name_end > end {
        return Err(ArtError::Truncated {
            offset: name_start,
            need: name_len,
            got: end.saturating_sub(name_start),
        });
    }
    let name = String::from_utf8_lossy(&buf[name_start..name_end])
        .split('\0')
        .next()
        .unwrap_or("")
        .to_string();
    let mut q = name_end;
    if q + 16 > end {
        return Err(ArtError::Truncated { offset: q, need: 16, got: end.saturating_sub(q) });
    }
    let mut kind_word = [0u8; 4];
    kind_word.copy_from_slice(&buf[q..q + 4]);
    let kind = AnimKind::from_word(kind_word);
    let frame_count = f32_at(buf, q + 4);
    let fps = f32_at(buf, q + 8);
    let cyclic = u32_at(buf, q + 12);
    q += 16;

    let mut track = AnimTrack {
        name,
        kind,
        frame_count,
        fps,
        cyclic,
        groups: Vec::new(),
    };

    // Walk the rest of the sequence; each group owns the channel
    // chunks that follow it until the next group.
    while q + 8 <= end {
        let t = u32_at(buf, q);
        let sz = u32_at(buf, q + 4) as usize;
        if sz < 8 || q + sz > end {
            break;
        }
        match t {
            CHUNK_TYPE_ANIM_GROUP => {
                // [u32 outer][u32 0][u8 namelen][pascal name]…
                let nlen = buf[q + 16] as usize;
                let ns = q + 17;
                let ne = (ns + nlen).min(q + sz);
                let gname = String::from_utf8_lossy(&buf[ns..ne])
                    .split('\0')
                    .next()
                    .unwrap_or("")
                    .to_string();
                track.groups.push(AnimGroup {
                    bone_name: gname,
                    rot: None,
                    tran: None,
                    rot_compact: None,
                    dof: None,
                });
            }
            CHUNK_TYPE_ROT | CHUNK_TYPE_TRAN => {
                // [u32 outer][u32 flag][4-B marker][u32 count][u16×count][keys]
                if q + 24 > q + sz {
                    q += sz;
                    continue;
                }
                let _marker = &buf[q + 16..q + 20];
                let count = u32_at(buf, q + 20) as usize;
                let idx_off = q + 24;
                if idx_off + 2 * count > q + sz {
                    q += sz;
                    continue;
                }
                let mut key_indices = Vec::with_capacity(count);
                for i in 0..count {
                    key_indices.push(u16_at(buf, idx_off + i * 2));
                }
                let keys_off = idx_off + 2 * count;
                let ksz = sz - 8 - 16 - 2 * count;
                let is_rot = t == CHUNK_TYPE_ROT;
                let per = if count == 0 { 0 } else { ksz / count };
                let want_per = 6; // 3×i16 or 3×f16
                let mut keys = Vec::with_capacity(count);
                if per == want_per {
                    for i in 0..count {
                        let ko = keys_off + i * per;
                        if ko + 6 > buf.len() {
                            break;
                        }
                        if is_rot {
                            let x = i16_at(buf, ko) as f32 / 32767.0;
                            let y = i16_at(buf, ko + 2) as f32 / 32767.0;
                            let z = i16_at(buf, ko + 4) as f32 / 32767.0;
                            let w2 = 1.0 - x * x - y * y - z * z;
                            let w = if w2 > 0.0 { w2.sqrt() } else { 0.0 };
                            keys.push(Key::Rot { x, y, z, w });
                        } else {
                            let x = f16_to_f32(u16_at(buf, ko));
                            let y = f16_to_f32(u16_at(buf, ko + 2));
                            let z = f16_to_f32(u16_at(buf, ko + 4));
                            keys.push(Key::Tran { x, y, z });
                        }
                    }
                }
                let chan = Channel { count: count as u32, key_indices, keys };
                if is_rot {
                    if let Some(g) = track.groups.last_mut() {
                        g.rot = Some(chan);
                    }
                } else {
                    if let Some(g) = track.groups.last_mut() {
                        g.tran = Some(chan);
                    }
                }
            }
            CHUNK_TYPE_ROT_COMPACT => {
                // [A][interp]['ROT\0'][count][u16×count frames]
                // [keys: 3 B each = 3×i8/127, w = sqrt(1-x²-y²-z²)]
                // (chunk size = 24 + 5·count; verified 29 B @ 1 key,
                // 119 B @ 19 keys on the real run track)
                if sz >= 24 && q + 24 <= end {
                    let count = u32_at(buf, q + 20) as usize;
                    let idx_off = q + 24;
                    let mut key_indices = Vec::with_capacity(count);
                    let mut keys = Vec::with_capacity(count);
                    if idx_off + 2 * count <= q + sz {
                        for i in 0..count {
                            key_indices.push(u16_at(buf, idx_off + i * 2));
                        }
                        let keys_off = idx_off + 2 * count;
                        let per = if count == 0 { 0 } else { (q + sz - keys_off) / count };
                        if per == 3 {
                            for i in 0..count {
                                let ko = keys_off + i * 3;
                                if ko + 3 > buf.len() {
                                    break;
                                }
                                let x = buf[ko] as i8 as f32 / 127.0;
                                let y = buf[ko + 1] as i8 as f32 / 127.0;
                                let z = buf[ko + 2] as i8 as f32 / 127.0;
                                let w2 = 1.0 - x * x - y * y - z * z;
                                let w = if w2 > 0.0 { w2.sqrt() } else { 0.0 };
                                keys.push(Key::Rot { x, y, z, w });
                            }
                        }
                    }
                    let chan = Channel { count: count as u32, key_indices, keys };
                    if let Some(g) = track.groups.last_mut() {
                        // Compact channels ALWAYS land in rot_compact; the
                        // pose lookup prefers a primary 0x121112 when the
                        // same bone carries one (no real run-track group
                        // carries both).
                        g.rot_compact = Some(chan);
                    }
                }
            }
            CHUNK_TYPE_TRAN_F32 => {
                // [A][interp]['TRAN'][u16 dyn 0=x/1=y/2=z][3×f32 const]
                // [count][u16×count frames][f32×count values]
                // (chunk size = 38 + 6·count; verified 50 B @ 2 keys on
                // Motion_Root)
                if sz >= 38 && q + 38 <= end {
                    let dyn_index = u16_at(buf, q + 20);
                    let constants =
                        [f32_at(buf, q + 22), f32_at(buf, q + 26), f32_at(buf, q + 30)];
                    let count = u32_at(buf, q + 34) as usize;
                    let idx_off = q + 38;
                    if idx_off + 6 * count <= q + sz {
                        let mut frames = Vec::with_capacity(count);
                        for i in 0..count {
                            frames.push(u16_at(buf, idx_off + i * 2));
                        }
                        let val_off = idx_off + 2 * count;
                        let mut values = Vec::with_capacity(count);
                        for i in 0..count {
                            values.push(f32_at(buf, val_off + i * 4));
                        }
                        let dof = DofChannel { dyn_index, constants, frames, values };
                        if let Some(g) = track.groups.last_mut() {
                            g.dof = Some(dof);
                        }
                    }
                }
            }
            // trailers (0x121110) / containers / metadata — recorded,
            // keys not decoded.
            _ => {}
        }
        q += sz;
    }
    Ok(track)
}

fn f32_at(buf: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(buf[o..o + 4].try_into().unwrap())
}

fn u32_at(buf: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(buf[o..o + 4].try_into().unwrap())
}

fn u16_at(buf: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(buf[o..o + 2].try_into().unwrap())
}

fn i16_at(buf: &[u8], o: usize) -> i16 {
    i16::from_le_bytes(buf[o..o + 2].try_into().unwrap())
}

/// IEEE 754 binary16 → f32, with the CORRECT subnormal scale
/// (2^-24 = 2^-14 × 2^-10 — the block-83 bug fix; a naive
/// `2^-14 * mantissa/1024` reads subnormals 1024× too big).
pub fn f16_to_f32(h: u16) -> f32 {
    let s = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = ((h >> 10) & 0x1f) as i32;
    let m = (h & 0x3ff) as f32;
    if e == 0 {
        return s * m * 2.0f32.powi(-24);
    }
    if e == 31 {
        return if m == 0.0 { s * f32::INFINITY } else { f32::NAN };
    }
    s * (1.0 + m / 1024.0) * 2.0f32.powi(e - 15)
}

/// Row-vector quaternion → 3×3 rotation (the Radical GEM convention:
/// `p' = p·M`, the TRANSPOSE of the column-vector quat matrix — the
/// block-85 bevy-glue tests pin this exact convention).
pub fn quat_to_mat3_row_vector(x: f32, y: f32, z: f32, w: f32) -> [[f32; 3]; 3] {
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + z * w), 2.0 * (x * z - y * w)],
        [2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + x * w)],
        [2.0 * (x * z + y * w), 2.0 * (y * z - x * w), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

/// Per-joint POSE world matrices at (integer) `frame`.
///
/// For every joint: the LOCAL matrix is the rest matrix's rotation
/// REPLACED by the anim ROT quat (row-vector quat→mat3; the compact
/// 0x121114 channel is used only where no primary 0x121112 exists) and
/// its translation REPLACED by the anim TRAN (absolute local meters); a
/// DOF channel (0x121102) sets the animated axis of the local
/// translation.  A joint without a channel keeps its full rest matrix.
/// Worlds compose parent-first: `world_i = local_i × world_parent`
/// (row-vector GEM order, see [`crate::skin::rest_world_matrices`]).
pub fn pose_world_matrices(
    skel: &Skeleton,
    track: &AnimTrack,
    frame: f32,
) -> Vec<[[f32; 4]; 4]> {
    compose_pose(skel, track, frame, None)
}

/// Block 95: the root-motion LOOP parameters extracted from a track's
/// 0x121102 Vector-1-DOF channel — the ONE source of truth for the
/// `k · cycle_travel` accumulator, shared by the CPU path
/// ([`pose_world_matrices_root_motion`]) and the bevy path (bridge
/// `apply_root_motion_loop`, the block-94 deferral).  `None` when the
/// track carries no DOF channel (then both paths are plain poses).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DofLoop {
    /// Skeleton-joint index of the DOF bone (Motion_Root on the run track).
    pub joint: usize,
    /// Animated axis: 0 = x, 1 = y, 2 = z.
    pub axis: usize,
    /// Net root displacement per cycle, signed: value(last) − value(0).
    /// The run track runs z −0.275 → −3.87 per 18-frame cycle, so the
    /// travel is ≈ −3.595 m (negative z = forward).
    pub cycle_travel: f32,
    /// The DOF value at frame 0 — the loop's starting root offset.
    pub v0: f32,
}

/// Extract [`DofLoop`] from `track` (the DOF group must also name a
/// joint that exists in `skel`).
pub fn dof_loop_params(skel: &Skeleton, track: &AnimTrack) -> Option<DofLoop> {
    let g = track.groups.iter().find(|g| g.dof.is_some())?;
    let joint = skel.find(&g.bone_name)?;
    let dof = g.dof.as_ref()?;
    let v0 = dof.value_at_frame(0).unwrap_or(0.0);
    let vlast = dof.value_at_frame(usize::MAX).unwrap_or(v0);
    Some(DofLoop {
        joint,
        axis: (dof.dyn_index as usize).min(2),
        cycle_travel: vlast - v0,
        v0,
    })
}

/// Block 90: pose at GLOBAL frame `global_frame` with the track's
/// Vector-1-DOF root-motion channel (0x121102) accumulated across
/// cycles (the standard motion-extraction loop).  With cycle length
/// `cycle_frames`, f = G % cycle_frames and k = G / cycle_frames; the
/// DOF bone's local translation along the DOF axis gets
/// `k · cycle_travel + (value(f) − value(0))` added, so frame 0 of each
/// cycle continues exactly where the previous cycle ended — a looping
/// track drives the character FORWARD with no snap.  Tracks without a
/// DOF channel behave exactly like [`pose_world_matrices`].
pub fn pose_world_matrices_root_motion(
    skel: &Skeleton,
    track: &AnimTrack,
    global_frame: f32,
    cycle_frames: usize,
) -> Vec<[[f32; 4]; 4]> {
    let Some(lp) = dof_loop_params(skel, track) else {
        return pose_world_matrices(skel, track, global_frame);
    };
    let g_frame = global_frame.floor().max(0.0) as usize;
    let (f, k) = if cycle_frames > 0 {
        (g_frame % cycle_frames, g_frame / cycle_frames)
    } else {
        (g_frame, 0)
    };
    // Absolute root axis position at global frame G = v(f) + k · travel:
    // frame 0 of each cycle lands exactly where the previous cycle ended
    // (v(0) + k·travel = v(last key) + (k-1)·travel).
    let extra = k as f32 * lp.cycle_travel;
    compose_pose(skel, track, f as f32, Some((lp.axis, extra)))
}

/// Block 94: compose world matrices from EXTERNAL per-joint LOCAL
/// poses — the bevy `AnimationPlayer`'s ticked transforms.  The bevy
/// render path (bridge example `bevy_render_run`) snapshots every
/// joint's evaluated `Transform` after an `App::update()` and folds
/// THAT pose through the proven GEM hierarchy, so the rendered frames
/// show exactly what bevy's animation engine evaluated, not what the
/// CPU sampler would have produced.
///
/// Row-vector GEM convention (identical to [`compose_pose`]'s fold):
/// the local matrix is the quat's row-vector 3×3
/// ([`quat_to_mat3_row_vector`]) with the translation in the bottom
/// row; worlds compose parent-first `world_i = local_i × world_parent`.
pub fn pose_world_matrices_from_locals(
    skel: &Skeleton,
    rots: &[[f32; 4]],
    trans: &[[f32; 3]],
) -> Vec<[[f32; 4]; 4]> {
    assert_eq!(rots.len(), skel.joints.len(), "one quat per joint");
    assert_eq!(trans.len(), skel.joints.len(), "one translation per joint");
    let mut out = vec![crate::skin::identity4(); skel.joints.len()];
    for (i, j) in skel.joints.iter().enumerate() {
        let r = quat_to_mat3_row_vector(rots[i][0], rots[i][1], rots[i][2], rots[i][3]);
        let mut local = crate::skin::identity4();
        for c in 0..3 {
            for k in 0..3 {
                local[c][k] = r[c][k];
            }
        }
        local[3][0] = trans[i][0];
        local[3][1] = trans[i][1];
        local[3][2] = trans[i][2];
        let parent_w = if j.parent < i as u32 {
            out[j.parent as usize]
        } else {
            crate::skin::identity4()
        };
        out[i] = mat4_mul(&local, &parent_w);
    }
    out
}

/// Shared compose: `dof_extra` shifts the DOF bone's local translation
/// along the DOF axis (root-motion accumulation); `None` = plain pose.
fn compose_pose(
    skel: &Skeleton,
    track: &AnimTrack,
    frame: f32,
    dof_extra: Option<(usize, f32)>,
) -> Vec<[[f32; 4]; 4]> {
    let f = frame.floor().max(0.0) as usize;
    let mut out = vec![crate::skin::identity4(); skel.joints.len()];
    for (i, j) in skel.joints.iter().enumerate() {
        // find the group for this bone
        let g = track.groups.iter().find(|g| g.bone_name == j.name);
        let (rot, tran, dof) = match g {
            Some(g) => (
                g.rot.as_ref().or(g.rot_compact.as_ref()).and_then(|c| c.key_at_frame(f)),
                g.tran.as_ref().and_then(|c| c.key_at_frame(f)),
                g.dof.as_ref(),
            ),
            None => (None, None, None),
        };
        // local = rest with rotation / translation replaced where keyed
        let mut local = j.rest;
        if let Some(Key::Rot { x, y, z, w }) = rot {
            let r = quat_to_mat3_row_vector(x, y, z, w);
            for c in 0..3 {
                for k in 0..3 {
                    local[c][k] = r[c][k];
                }
            }
        }
        if let Some(Key::Tran { x, y, z }) = tran {
            local[3][0] = x;
            local[3][1] = y;
            local[3][2] = z;
        }
        if let Some(d) = dof {
            if let Some(v) = d.value_at_frame(f) {
                let axis = (d.dyn_index as usize).min(2);
                local[3][axis] = v
                    + dof_extra.map_or(0.0, |(ax, e)| if ax == axis { e } else { 0.0 });
            }
        }
        let parent_w = if j.parent < i as u32 {
            out[j.parent as usize]
        } else {
            crate::skin::identity4()
        };
        out[i] = mat4_mul(&local, &parent_w);
    }
    out
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skin::{Skeleton, SkeletonJoint};

    /// The real alex_loco_run_n chunk sequence (header + groups +
    /// channels + trailers), extracted from the live alex.p3d.
    const FIXTURE: &[u8] = include_bytes!("../../prototype-bin/tests/fixtures/alex_loco_run_n_chunk.bin");

    #[test]
    fn f16_boundaries() {
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0x4000), 2.0);
        assert_eq!(f16_to_f32(0xc000), -2.0);
        // smallest subnormal: 2^-24 (the block-83 fix)
        assert_eq!(f16_to_f32(0x0001), 2.0f32.powi(-24));
        // largest subnormal: (1023/1024)·2^-24
        assert!((f16_to_f32(0x03ff) - 1023.0 * 2.0f32.powi(-24)).abs() < 1e-30);
        assert_eq!(f16_to_f32(0x7c00), f32::INFINITY);
    }

    #[test]
    fn fixture_header_decodes() {
        let tracks = find_anim_tracks(FIXTURE);
        assert_eq!(tracks.len(), 1, "fixture holds exactly one track");
        let (off, span, name) = &tracks[0];
        assert_eq!(name, "alex_loco_run_n");
        assert_eq!(off + span, FIXTURE.len(), "span covers the whole fixture");
        let t = decode_anim_track(FIXTURE, *off, *span).unwrap();
        assert!(matches!(t.kind, AnimKind::Ptrn));
        assert_eq!(t.name, "alex_loco_run_n");
        assert_eq!(t.frame_count, 19.0);
        assert_eq!(t.fps, 30.0);
        assert_eq!(t.cyclic, 0);
    }

    #[test]
    fn fixture_group_and_channel_counts() {
        let (off, span, _) = &find_anim_tracks(FIXTURE)[0];
        let t = decode_anim_track(FIXTURE, *off, *span).unwrap();
        assert_eq!(t.groups.len(), 60, "60 bone groups");
        let rots = t.groups.iter().filter(|g| g.rot.is_some()).count();
        let trans = t.groups.iter().filter(|g| g.tran.is_some()).count();
        let compact = t.groups.iter().filter(|g| g.rot_compact.is_some()).count();
        let dofs = t.groups.iter().filter(|g| g.dof.is_some()).count();
        let both = t
            .groups
            .iter()
            .filter(|g| g.rot.is_some() && g.rot_compact.is_some())
            .count();
        assert_eq!(rots, 24, "24 groups carry a primary 0x121112 ROT");
        assert_eq!(trans, 5, "5 groups carry a TRAN channel");
        assert_eq!(compact, 34, "34 groups carry a compact 0x121114 ROT");
        assert_eq!(dofs, 1, "exactly 1 group (Motion_Root) carries the 0x121102 DOF");
        assert_eq!(both, 0, "no group carries BOTH primary and compact ROT");
        let with_any = t
            .groups
            .iter()
            .filter(|g| {
                g.rot.is_some() || g.rot_compact.is_some() || g.tran.is_some() || g.dof.is_some()
            })
            .count();
        assert_eq!(with_any, 60, "EVERY one of the 60 groups carries a channel");
    }

    /// BLOCK 90: the compact-ROT (0x121114) layout pinned against the
    /// real bytes — 3×i8/127 smallest-three quats, unit length, and the
    /// exact decode of the animated 19-key Ball_L channel + a static
    /// 1-key finger channel.
    #[test]
    fn compact_rot_channels_decode_unit_quats() {
        let (off, span, _) = &find_anim_tracks(FIXTURE)[0];
        let t = decode_anim_track(FIXTURE, *off, *span).unwrap();
        let ball = t.groups.iter().find(|g| g.bone_name == "Ball_L").expect("Ball_L group");
        // Ball_L carries NO primary ROT — its channel is the compact one.
        assert!(ball.rot_compact.is_some(), "Ball_L uses the compact channel");
        let chan = ball.rot_compact.as_ref().unwrap();
        assert_eq!(chan.count, 19);
        assert_eq!(chan.keys.len(), 19);
        assert_eq!(chan.key_indices[0], 0);
        assert_eq!(chan.key_indices[18], 18);
        for k in &chan.keys {
            match *k {
                Key::Rot { x, y, z, w } => {
                    let n = (x * x + y * y + z * z + w * w).sqrt();
                    assert!((n - 1.0).abs() < 0.02, "|q| = {n}");
                }
                _ => panic!("expected a rotation key"),
            }
        }
        // Real bytes: Ball_L k0 = (+0.008, -0.079, -0.307) (probe).
        match chan.keys[0] {
            Key::Rot { x, y, z, .. } => {
                assert!((x - 0.008).abs() < 0.02 && (y + 0.079).abs() < 0.02
                    && (z + 0.307).abs() < 0.02,
                    "Ball_L k0 = ({x}, {y}, {z})")
            }
            _ => panic!(),
        }
        // A static finger channel: 1 key, clamped lookup at every frame.
        let thumb = t.groups.iter().find(|g| g.bone_name == "Thumb_L").expect("Thumb_L");
        let tc = thumb.rot_compact.as_ref().expect("Thumb_L compact");
        assert_eq!(tc.count, 1);
        assert_eq!(tc.keys.len(), 1);
        for f in [0usize, 5, 18, 500] {
            assert!(matches!(tc.key_at_frame(f), Some(Key::Rot { .. })), "frame {f}");
        }
        // Shoulder_Con_L animates too (19 keys).
        let scon = t.groups.iter().find(|g| g.bone_name == "Shoulder_Con_L").unwrap();
        assert_eq!(scon.rot_compact.as_ref().unwrap().count, 19);
    }

    /// BLOCK 90: the Vector-1-DOF root-motion channel pinned against
    /// real bytes — Motion_Root, axis z, keys @ [0, 18], linear interp.
    #[test]
    fn dof_root_motion_channel_decodes() {
        let (off, span, _) = &find_anim_tracks(FIXTURE)[0];
        let t = decode_anim_track(FIXTURE, *off, *span).unwrap();
        let mr = t.groups.iter().find(|g| g.bone_name == "Motion_Root").expect("Motion_Root");
        let d = mr.dof.as_ref().expect("Motion_Root DOF");
        assert_eq!(d.dyn_index, 2, "z axis");
        assert!((d.constants[1] - 1.0).abs() < 1e-4, "const y = 1.0 (root at 1 m)");
        assert_eq!(d.frames, vec![0, 18]);
        assert!((d.values[0] + 0.2755).abs() < 1e-3);
        assert!((d.values[1] + 3.870).abs() < 1e-2);
        // linear interpolation mid-cycle
        let mid = d.value_at_frame(9).unwrap();
        let expect = d.values[0] + (d.values[1] - d.values[0]) * (9.0 / 18.0);
        assert!((mid - expect).abs() < 1e-5);
        // clamped outside
        assert!((d.value_at_frame(0).unwrap() - d.values[0]).abs() < 1e-6);
        assert!((d.value_at_frame(18).unwrap() - d.values[1]).abs() < 1e-6);
        assert!((d.value_at_frame(500).unwrap() - d.values[1]).abs() < 1e-6);
        // travel per cycle ≈ 3.59 m (≈5.7 m/s at 30 fps)
        let travel = d.values[1] - d.values[0];
        assert!(travel < -3.0 && travel > -4.5, "cycle travel {travel}");
    }

    /// A one-joint Motion_Root skeleton for the DOF / root-motion pose
    /// tests (the DOF group binds by bone NAME).
    fn mini_skel_root() -> Skeleton {
        Skeleton {
            name: "mini_root".into(),
            joints: vec![SkeletonJoint {
                name: "Motion_Root".into(),
                parent: 0,
                rest: crate::skin::identity4(),
            }],
        }
    }

    /// BLOCK 90: pose_world_matrices applies the DOF channel to the
    /// Motion_Root local translation (root world z = the DOF value).
    #[test]
    fn pose_applies_dof_and_compact_rot() {
        let (off, span, _) = &find_anim_tracks(FIXTURE)[0];
        let t = decode_anim_track(FIXTURE, *off, *span).unwrap();
        let skel = mini_skel_root();
        let w0 = pose_world_matrices(&skel, &t, 0.0);
        let w18 = pose_world_matrices(&skel, &t, 18.0);
        // Motion_Root is joint 0: its world translation z must be the
        // DOF value (the rest z is replaced).
        assert!((w0[0][3][2] + 0.2755).abs() < 1e-3, "z@f0 = {}", w0[0][3][2]);
        assert!((w18[0][3][2] + 3.870).abs() < 1e-2, "z@f18 = {}", w18[0][3][2]);
    }

    /// BLOCK 90: pose_world_matrices_root_motion accumulates across
    /// cycles (z = v(f) + k·travel) — no snap at the loop point.
    #[test]
    fn root_motion_accumulates_across_cycles() {
        let (off, span, _) = &find_anim_tracks(FIXTURE)[0];
        let t = decode_anim_track(FIXTURE, *off, *span).unwrap();
        let skel = mini_skel_root();
        let cy = 19usize;
        let z = |g: usize| pose_world_matrices_root_motion(&skel, &t, g as f32, cy)[0][3][2];
        let z18 = z(18);
        let z19 = z(19); // cycle 1, frame 0 — must CONTINUE from z18
        let z20 = z(20);
        assert!((z19 - z18).abs() < 1e-4, "no snap: z18 {z18} vs z19 {z19}");
        assert!(z20 < z19, "forward motion continues (z20 {z20} < z19 {z19})");
        // cycle 0 (k=0) matches the plain pose exactly
        let plain = pose_world_matrices(&skel, &t, 9.0)[0][3][2];
        assert!((z(9) - plain).abs() < 1e-6, "cycle 0 = plain pose");
        // cycle 2, frame 16: z = v(16) + 2·travel
        let dof = t
            .groups
            .iter()
            .find(|g| g.bone_name == "Motion_Root")
            .unwrap()
            .dof
            .clone()
            .unwrap();
        let travel = dof.values[1] - dof.values[0];
        let expect = dof.value_at_frame(16).unwrap() + 2.0 * travel;
        assert!((z(54) - expect).abs() < 1e-4, "z54 {} vs {expect}", z(54));
    }

    #[test]
    fn pelvis_rot_keys_are_unit_quats() {
        let (off, span, _) = &find_anim_tracks(FIXTURE)[0];
        let t = decode_anim_track(FIXTURE, *off, *span).unwrap();
        let pelvis = t.groups.iter().find(|g| g.bone_name == "Pelvis").expect("Pelvis group");
        let chan = pelvis.rot.as_ref().expect("Pelvis ROT channel");
        assert_eq!(chan.count, 19);
        assert_eq!(chan.keys.len(), 19);
        let k0 = chan.keys[0];
        match k0 {
            Key::Rot { x, y, z, w } => {
                let n = (x * x + y * y + z * z + w * w).sqrt();
                assert!((n - 1.0).abs() < 1e-3, "|q| = {n}");
            }
            _ => panic!("expected a rotation key"),
        }
        // all keys unit
        for k in &chan.keys {
            if let Key::Rot { x, y, z, w } = *k {
                let n = (x * x + y * y + z * z + w * w).sqrt();
                assert!((n - 1.0).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn pelvis_rot_varies_per_frame() {
        let (off, span, _) = &find_anim_tracks(FIXTURE)[0];
        let t = decode_anim_track(FIXTURE, *off, *span).unwrap();
        let pelvis = t.groups.iter().find(|g| g.bone_name == "Pelvis").unwrap();
        let chan = pelvis.rot.as_ref().unwrap();
        assert_ne!(chan.keys[0], chan.keys[9], "frame 0 != frame 9 (real motion)");
    }

    #[test]
    fn tran_keys_finite_and_bounded() {
        let (off, span, _) = &find_anim_tracks(FIXTURE)[0];
        let t = decode_anim_track(FIXTURE, *off, *span).unwrap();
        for g in &t.groups {
            if let Some(c) = &g.tran {
                assert_eq!(c.keys.len() as u32, c.count);
                for k in &c.keys {
                    if let Key::Tran { x, y, z } = *k {
                        assert!(x.is_finite() && y.is_finite() && z.is_finite());
                        assert!(x.abs() < 100.0 && y.abs() < 100.0 && z.abs() < 100.0);
                    }
                }
            }
        }
    }

    /// A minimal 2-bone skeleton (Root → Pelvis) wired to the real
    /// fixture track: the pose worlds must differ per frame and loop.
    fn mini_skel() -> Skeleton {
        let mut rest = crate::skin::identity4();
        rest[3][1] = 1.0; // root at y=1
        let pelvis = crate::skin::identity4();
        Skeleton {
            name: "mini".into(),
            joints: vec![
                SkeletonJoint { name: "Character_Root".into(), parent: 0, rest },
                SkeletonJoint { name: "Pelvis".into(), parent: 0, rest: pelvis },
            ],
        }
    }

    #[test]
    fn pose_world_matrices_differ_per_frame() {
        let (off, span, _) = &find_anim_tracks(FIXTURE)[0];
        let t = decode_anim_track(FIXTURE, *off, *span).unwrap();
        let skel = mini_skel();
        let w0 = pose_world_matrices(&skel, &t, 0.0);
        let w9 = pose_world_matrices(&skel, &t, 9.0);
        let w18 = pose_world_matrices(&skel, &t, 18.0);
        assert_eq!(w0.len(), 2);
        // Pelvis (index 1) must move between frames 0 and 9
        let delta: f32 = (0..3)
            .map(|r| (0..3).map(|c| (w0[1][r][c] - w9[1][r][c]).abs()).sum::<f32>())
            .sum();
        assert!(delta > 1e-4, "Pelvis world must differ per frame (delta={delta})");
        // cyclic loop: frame 18 ≈ frame 0 (the run cycle wraps)
        let mut maxd = 0f32;
        for r in 0..4 {
            for c in 0..4 {
                maxd = maxd.max((w0[1][r][c] - w18[1][r][c]).abs());
            }
        }
        assert!(maxd < 0.2, "cyclic wrap: max delta {maxd}");
    }

    /// Block 94 — `pose_world_matrices_from_locals` folds an external
    /// local pose (the bevy AnimationPlayer's ticked transforms) through
    /// the same parent-first row-vector GEM hierarchy as
    /// [`pose_world_matrices`].  Hand-checked 2-joint chain: both
    /// locals rotated +90° about Z with translation [1,0,0]:
    /// * world0 translation = [1,0,0] (parent identity);
    /// * world0 maps the direction [1,0,0] to [0,1,0] (the +90° spin);
    /// * world1 translation = local1's translation rotated by world0's
    ///   3×3 ([1,0,0] → [0,1,0]) plus world0's translation = [1,1,0];
    /// * world1's 3×3 = M·M = the +180° rotation diag(-1,-1,1).
    #[test]
    fn locals_compose_row_vector_chain() {
        use crate::skin::{mat4_apply_dir, mat4_apply, SkeletonJoint};
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let joints = vec![
            SkeletonJoint { name: "A".into(), parent: 0, rest: crate::skin::identity4() },
            SkeletonJoint { name: "B".into(), parent: 0, rest: crate::skin::identity4() },
        ];
        let skel = Skeleton { name: "chain".into(), joints };
        let rots = vec![[0.0, 0.0, s, s]; 2];
        let trans = vec![[1.0, 0.0, 0.0]; 2];
        let w = pose_world_matrices_from_locals(&skel, &rots, &trans);
        // world0: translation row.
        for c in 0..3 {
            assert!((w[0][3][c] - [1.0, 0.0, 0.0][c]).abs() < 1e-6, "w0 trans {c}: {:?}", w[0][3]);
        }
        // world0 rotates +90° about Z (row-vector: p' = p·M).
        let d = mat4_apply_dir(&w[0], [1.0, 0.0, 0.0]);
        assert!(d[0].abs() < 1e-6 && (d[1] - 1.0).abs() < 1e-6, "w0 dir {d:?}");
        // world1: translation = [1,1,0]; 3×3 = +180°.
        for c in 0..3 {
            assert!((w[1][3][c] - [1.0, 1.0, 0.0][c]).abs() < 1e-6, "w1 trans {c}: {:?}", w[1][3]);
        }
        let p = mat4_apply(&w[1], [1.0, 0.0, 0.0]);
        // p·world1 = [1,0,0]·M² + [1,1,0] = [-1,0,0] + [1,1,0] = [0,1,0].
        assert!(p[0].abs() < 1e-6 && (p[1] - 1.0).abs() < 1e-6, "w1 point {p:?}");
        assert!((w[1][0][0] + 1.0).abs() < 1e-6 && (w[1][1][1] + 1.0).abs() < 1e-6);
    }
}

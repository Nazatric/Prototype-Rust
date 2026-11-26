//! frontend_gfx — the REAL Scaleform `.gfx` menu screens, parsed and
//! rendered (block 120).
//!
//! The game's menus are Scaleform GFx UI movies stored as `art\hud\*.gfx`
//! members inside the user's own archives — the boot census (block 107)
//! PROVES all 25 real screens are present on-device (protostart.gfx,
//! protomainmenu.gfx, …).  A `.gfx` member is a SWF container: GFx
//! writes the standard SWF tag stream (GFx adds tags of its own above
//! 90 — this parser records and skips anything it cannot render, it
//! never guesses).
//!
//! # What this block renders
//!
//! The FIRST FRAME of a screen: `DefineBitsLossless2` bitmaps
//! (formats 3/4/5 — the raster art Scaleform menus are built from)
//! placed through `PlaceObject2/3` matrices over the stage `RECT`,
//! rasterized on the CPU through the REAL frontend shader contract
//! ([`super::render_frontend_cpu`] — the cxformtexture/cxformsolid
//! pairs proven from `frontendshaders.p3d`).  `DefineShape` vector
//! art, fonts/text (the block-106 textbible line renders text
//! separately) and ActionScript are recorded as unrendered and
//! skipped — honest scopes, never guesses.
//!
//! # Evidence labels
//!
//! * PROVEN: the SWF container shape (SIG/RECT/tag-stream — the
//!   decompressed members are literally `GFX`-magic SWF files, the
//!   RZ module's own docs note it), the zlib body (the PROVEN
//!   `prototype_common::zlib` inflate), the frontend rasterizer.
//! * DESIGN (ours): first-frame-only rendering, depth-order painter
//!   overwrite (no alpha blending yet — the CPU frontend path is
//!   opaque), letterboxed stage fit.
//! * UNKNOWN: which exact tag mix each of the 25 real screens
//!   carries (the on-device parse journal line reports it per
//!   screen — the next device run closes it).

use super::render_frontend_cpu::{render_sprites_textured, FrontendFrame, FrontendSprite, TexRef};

/// One extracted bitmap: an RGBA8 image with its SWF character id.
#[derive(Debug, Clone)]
pub struct GfxImage {
    /// The SWF character id (`DefineBitsLossless2` CharacterId) —
    /// placements reference this.
    pub id: u16,
    pub width: u32,
    pub height: u32,
    /// RGBA8, row-major, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

/// One frame-1 display-list entry: character `char_id` at `depth`,
/// transformed by the SWF MATRIX `[a, b, c, d, tx, ty]` (a..d 16.16
/// fixed, tx/ty twips; `x' = a·x + c·y + tx`, `y' = b·x + d·y + ty`).
#[derive(Debug, Clone, Copy)]
pub struct GfxPlacement {
    pub depth: u16,
    pub char_id: u16,
    pub matrix: [f32; 6],
}

/// A parsed screen: the stage `RECT` (twips), background colour, the
/// extracted bitmaps, the character dictionary (shapes / texts /
/// sprites — block 121), the frame-1 display list (depth order =
/// paint order), and an honest count of every tag kind this parser
/// does not render yet.
#[derive(Debug, Clone, Default)]
pub struct GfxScreen {
    /// Stage RECT `(xmin, ymin, xmax, ymax)` in twips.
    pub rect: [f32; 4],
    /// `SetBackgroundColor` as RGBA8 floats (alpha 1).
    pub background: [f32; 4],
    pub images: Vec<GfxImage>,
    /// Block 121: the character dictionary — DefineShape (2/22/32),
    /// DefineEditText (37) and DefineSprite (39, its own frame-1
    /// display list) — keyed by character id.
    pub chars: std::collections::BTreeMap<u16, crate::frontend_gfx_shapes::GfxChar>,
    /// Frame-1 display list in DEPTH order (ascending = painter order).
    pub placements: Vec<GfxPlacement>,
    /// `(tag code, occurrences)` for every tag kind skipped — the
    /// on-device journal prints this so the next block knows exactly
    /// what the real screens carry.
    pub unrendered: Vec<(u8, u32)>,
}

impl GfxScreen {
    /// True when the screen carries VECTOR content (shapes or texts)
    /// — the vector renderer is the right path for it (block 121:
    /// the real screens are 100% vector, zero bitmaps).
    pub fn has_vector_content(&self) -> bool {
        self.chars.values().any(|c| {
            matches!(
                c,
                crate::frontend_gfx_shapes::GfxChar::Shape(_)
                    | crate::frontend_gfx_shapes::GfxChar::Text(_)
            )
        })
    }
}

// ---------------------------------------------------------------------------
// byte + bit readers (SWF is little-endian bytes, MSB-first bit fields)
// ---------------------------------------------------------------------------

pub(crate) struct ByteReader<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> ByteReader<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { b, p: 0 }
    }
    fn u8(&mut self) -> Result<u8, String> {
        if self.p >= self.b.len() {
            return Err("gfx: eof in byte field".into());
        }
        let v = self.b[self.p];
        self.p += 1;
        Ok(v)
    }
    fn u16(&mut self) -> Result<u16, String> {
        Ok(self.u8()? as u16 | (self.u8()? as u16) << 8)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(self.u8()? as u32
            | (self.u8()? as u32) << 8
            | (self.u8()? as u32) << 16
            | (self.u8()? as u32) << 24)
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.p + n > self.b.len() {
            return Err("gfx: eof in byte span".into());
        }
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn take_tag_body(&mut self, n: usize) -> Result<Self, String> {
        Ok(Self::new(self.bytes(n)?))
    }
    fn remaining(&self) -> usize {
        self.b.len() - self.p.min(self.b.len())
    }
}

/// MSB-first bit reader over a fixed span (RECT / MATRIX fields).
pub(crate) struct BitReader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(b: &'a [u8]) -> Self {
        Self { b, pos: 0 }
    }
    pub(crate) fn bits(&mut self, n: u32) -> Result<u32, String> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = self.b.get(self.pos >> 3).ok_or("gfx: eof in bit field")?;
            let bit = (byte >> (7 - (self.pos & 7))) & 1;
            v = (v << 1) | bit as u32;
            self.pos += 1;
        }
        Ok(v)
    }
    /// Sign-extend an `n`-bit two's-complement field.
    pub(crate) fn sbits(&mut self, n: u32) -> Result<i32, String> {
        let v = self.bits(n)?;
        if n == 0 {
            return Ok(0);
        }
        if v & (1 << (n - 1)) != 0 {
            Ok((v as i32) - (1 << n))
        } else {
            Ok(v as i32)
        }
    }
    pub(crate) fn align_byte(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }
    /// Byte offset the reader has consumed through (byte-aligned).
    pub(crate) fn consumed_bytes(&self) -> usize {
        (self.pos + 7) >> 3
    }
}

/// The SWF RECT: `UB[5] NBits` + four `SB[NBits]` (twips).  FILE
/// order is (xmin, xmax, ymin, ymax); stored here normalized to
/// (xmin, ymin, xmax, ymax).  (The shapes module reuses this — see
/// [`parse_rect_pub`].)
pub(crate) fn parse_rect_pub(r: &mut BitReader<'_>) -> Result<[f32; 4], String> {
    parse_rect(r)
}

fn parse_rect(r: &mut BitReader<'_>) -> Result<[f32; 4], String> {
    let n = r.bits(5)?;
    if n == 0 || n > 31 {
        return Err(format!("gfx: RECT bit width {n} out of range"));
    }
    let (xmin, xmax, ymin, ymax) = (
        r.sbits(n)? as f32,
        r.sbits(n)? as f32,
        r.sbits(n)? as f32,
        r.sbits(n)? as f32,
    );
    Ok([xmin, ymin, xmax, ymax])
}

/// The SWF MATRIX: scale / rotate / translate bit fields.
fn parse_matrix(r: &mut BitReader<'_>) -> Result<[f32; 6], String> {
    let mut m = [0.0f32, 0.0, 0.0, 0.0, 0.0, 0.0];
    if r.bits(1)? == 1 {
        let n = r.bits(5)?;
        let sx = r.sbits(n)? as f32 / 65536.0;
        let sy = r.sbits(n)? as f32 / 65536.0;
        m[0] = sx;
        m[3] = sy;
    } else {
        m[0] = 1.0;
        m[3] = 1.0;
    }
    if r.bits(1)? == 1 {
        let n = r.bits(5)?;
        m[1] = r.sbits(n)? as f32 / 65536.0; // rotate skew 0 → y'
        m[2] = r.sbits(n)? as f32 / 65536.0; // rotate skew 1 → x'
    }
    let n = r.bits(5)?;
    m[4] = r.sbits(n)? as f32; // tx (twips)
    m[5] = r.sbits(n)? as f32; // ty (twips)
    r.align_byte();
    Ok(m)
}

// ---------------------------------------------------------------------------
// DefineBitsLossless2 decode
// ---------------------------------------------------------------------------

/// The zlib-inflated row data → RGBA8.  `format`: 3 = colormap
/// (palette entries are RGBA for Lossless2), 4 = 15-bit RGB, 5 =
/// 32-bit BGRA-in-file (little-endian ARGB).  Formats 3/4 pad each
/// row to a 32-bit boundary; format 5 does not (SWF spec).
fn decode_lossless2(
    format: u8,
    width: u32,
    height: u32,
    n_colors: u8,
    data: &[u8],
) -> Result<GfxImage, String> {
    let inflated = prototype_common::zlib::zlib_decompress(data)
        .map_err(|e| format!("gfx: lossless2 zlib failed: {e}"))?;
    let w = width as usize;
    let h = height as usize;
    let mut rgba = vec![0u8; w * h * 4];
    match format {
        5 => {
            if inflated.len() < w * h * 4 {
                return Err(format!(
                    "gfx: lossless5 data {} < {} bytes",
                    inflated.len(),
                    w * h * 4
                ));
            }
            for i in 0..w * h {
                // File order: B, G, R, A (ARGB u32 little-endian).
                rgba[i * 4] = inflated[i * 4 + 2];
                rgba[i * 4 + 1] = inflated[i * 4 + 1];
                rgba[i * 4 + 2] = inflated[i * 4];
                rgba[i * 4 + 3] = inflated[i * 4 + 3];
            }
        }
        3 => {
            let palette_len = (n_colors as usize + 1) * 4;
            let pitch = (w + 3) / 4 * 4;
            if inflated.len() < pitch * h + palette_len {
                return Err(format!(
                    "gfx: lossless3 data {} < {} bytes",
                    inflated.len(),
                    pitch * h + palette_len
                ));
            }
            let pal = &inflated[pitch * h..pitch * h + palette_len];
            for y in 0..h {
                for x in 0..w {
                    let idx = inflated[y * pitch + x] as usize;
                    if idx * 4 + 4 > pal.len() {
                        return Err("gfx: lossless3 index out of palette".into());
                    }
                    let o = (y * w + x) * 4;
                    rgba[o] = pal[idx * 4 + 2];
                    rgba[o + 1] = pal[idx * 4 + 1];
                    rgba[o + 2] = pal[idx * 4];
                    rgba[o + 3] = pal[idx * 4 + 3];
                }
            }
        }
        4 => {
            let pitch = (w * 2 + 3) / 4 * 4;
            if inflated.len() < pitch * h {
                return Err(format!(
                    "gfx: lossless4 data {} < {} bytes",
                    inflated.len(),
                    pitch * h
                ));
            }
            for y in 0..h {
                for x in 0..w {
                    let v = u16::from_le_bytes([
                        inflated[y * pitch + x * 2],
                        inflated[y * pitch + x * 2 + 1],
                    ]);
                    let r = (((v >> 10) & 0x1f) as u32 * 255 / 31) as u8;
                    let g = (((v >> 5) & 0x1f) as u32 * 255 / 31) as u8;
                    let b = ((v & 0x1f) as u32 * 255 / 31) as u8;
                    let o = (y * w + x) * 4;
                    rgba[o] = r;
                    rgba[o + 1] = g;
                    rgba[o + 2] = b;
                    rgba[o + 3] = 0xff;
                }
            }
        }
        f => return Err(format!("gfx: lossless format {f} unsupported")),
    }
    Ok(GfxImage { id: 0, width, height, rgba })
}

// ---------------------------------------------------------------------------
// the tag stream
// ---------------------------------------------------------------------------

/// Parse a `.gfx` member: signature (FWS / CWS / GFX / CFG), stage
/// RECT, then the tag stream up to the FIRST ShowFrame (frame 1 —
/// the screen's at-rest look).  `DefineSprite` sub-streams are
/// skipped to their own End tag so their innards cannot derail the
/// outer stream.
pub fn parse_gfx(bytes: &[u8]) -> Result<GfxScreen, String> {
    if bytes.len() < 8 {
        return Err("gfx: shorter than the 8-byte SWF header".into());
    }
    let sig = &bytes[..3];
    let body: Vec<u8>;
    match sig {
        b"FWS" | b"GFX" => body = bytes.to_vec(),
        b"CWS" | b"CFG" => {
            let inflated = prototype_common::zlib::zlib_decompress(&bytes[8..])
                .map_err(|e| format!("gfx: {} body zlib failed: {e}", String::from_utf8_lossy(sig)))?;
            // The inflated body starts where the plain stream would
            // continue after the 8-byte header: prepend the header so
            // the RECT/frame fields read uniformly.
            let mut full = bytes[..8].to_vec();
            full.extend_from_slice(&inflated);
            body = full;
        }
        other => {
            return Err(format!(
                "gfx: signature {:?} not a SWF/GFx container",
                String::from_utf8_lossy(other)
            ))
        }
    }

    let mut r = BitReader::new(&body[8..]);
    let rect = parse_rect(&mut r)?;
    r.align_byte();
    let mut tags = ByteReader::new(&body[8 + r.consumed_bytes()..]);
    // fps (FIXED8 u16) + frame count — not needed for frame 1.
    let _fps = tags.u16()?;
    let _frames = tags.u16()?;

    let mut screen = GfxScreen {
        rect,
        background: [0.0, 0.0, 0.0, 1.0],
        ..Default::default()
    };
    // depth → (char id, matrix): the live display list.
    let mut list: std::collections::BTreeMap<u16, (u16, [f32; 6])> = Default::default();
    let mut skipped: std::collections::BTreeMap<u8, u32> = Default::default();

    loop {
        let code_len = tags.u16()?;
        let code = (code_len >> 6) as u8;
        let mut len = (code_len & 0x3f) as usize;
        if len == 0x3f {
            len = tags.u32()? as usize;
        }
        match code {
            0 => break,            // End
            1 => break,            // ShowFrame — frame 1 collected
            9 => {
                // SetBackgroundColor: RGB → RGBA
                let mut c = tags.take_tag_body(len)?;
                screen.background = [
                    c.u8()? as f32 / 255.0,
                    c.u8()? as f32 / 255.0,
                    c.u8()? as f32 / 255.0,
                    1.0,
                ];
            }
            36 => {
                // DefineBitsLossless2
                let mut t = tags.take_tag_body(len)?;
                let id = t.u16()?;
                let format = t.u8()?;
                let width = t.u16()? as u32;
                let height = t.u16()? as u32;
                let n_colors = if format == 3 { t.u8()? } else { 0 };
                let data = t.bytes(t.remaining())?;
                let mut img = decode_lossless2(format, width, height, n_colors, data)?;
                img.id = id;
                screen.images.push(img);
            }
            26 | 27 => {
                // PlaceObject2 / PlaceObject3
                let mut t = tags.take_tag_body(len)?;
                let mut f2 = 0u8;
                if code == 27 {
                    let f1 = t.u8()?;
                    f2 = t.u8()?;
                    let _has_class_name = f2 & 0x20 != 0;
                    let _has_image = f2 & 0x40 != 0;
                    let _reserved = f1 & 0x80 != 0; // clip actions flag bit (PO3 splits flags)
                }
                let flags = t.u8()?;
                let depth = t.u16()?;
                let has_char = flags & 0x02 != 0;
                let has_matrix = flags & 0x04 != 0;
                if code == 27 && f2 & 0x20 != 0 {
                    // ClassName (null-terminated string) — skip.
                    loop {
                        match t.u8()? {
                            0 => break,
                            _ => {}
                        }
                    }
                }
                let char_id = if has_char { Some(t.u16()?) } else { None };
                let matrix = if has_matrix {
                    let mut br = BitReader::new(t.bytes(t.remaining())?);
                    parse_matrix(&mut br)?
                } else {
                    [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
                };
                // (color transforms / ratio / names / filters are
                // irrelevant to frame-1 bitmap placement — ignored.)
                match (char_id, list.get_mut(&depth)) {
                    (Some(id), _) => {
                        list.insert(depth, (id, matrix));
                    }
                    (None, Some(slot)) => slot.1 = matrix, // Move: re-transform
                    (None, None) => {}                     // move on an empty depth — ignore
                }
            }
            28 => {
                // RemoveObject2
                let mut t = tags.take_tag_body(len)?;
                let depth = t.u16()?;
                list.remove(&depth);
            }
            39 => {
                // DefineSprite (block 121): the nested tag stream IS
                // the sprite's frame-1 display list (nested End
                // terminates it).  Nested character definitions are
                // legal inside sprites — they join the SAME dictionary.
                let body = tags.bytes(len)?.to_vec();
                let mut sr = ByteReader::new(&body);
                let id = sr.u16()?;
                let _frames = sr.u16()?;
                let list = parse_sprite_stream(sr.bytes(sr.remaining())?, &mut screen.chars)?;
                screen.chars.insert(id, crate::frontend_gfx_shapes::GfxChar::Sprite(list));
                *skipped.entry(39).or_insert(0) += 1;
            }
            2 | 22 | 32 => {
                // DefineShape / DefineShape2 / DefineShape3 (block 121):
                // the vector art the real screens are made of.
                let body = tags.bytes(len)?.to_vec();
                let version = match code {
                    2 => 1u8,
                    22 => 2,
                    _ => 3,
                };
                let shape = crate::frontend_gfx_shapes::parse_shape(&body, version)?;
                let id = (body[0] as u16) | ((body[1] as u16) << 8);
                screen.chars.insert(id, crate::frontend_gfx_shapes::GfxChar::Shape(shape));
                *skipped.entry(code).or_insert(0) += 1;
            }
            37 => {
                // DefineEditText (block 121): the REAL UI text.
                let body = tags.bytes(len)?.to_vec();
                let text = crate::frontend_gfx_shapes::parse_edit_text(&body)?;
                let id = (body[0] as u16) | ((body[1] as u16) << 8);
                screen.chars.insert(id, crate::frontend_gfx_shapes::GfxChar::Text(text));
                *skipped.entry(37).or_insert(0) += 1;
            }
            other => {
                // Everything else (shapes, fonts, text, JPEGs,
                // ActionScript, GFx extensions ≥ 90): recorded, skipped.
                let _ = tags.bytes(len)?;
                *skipped.entry(other).or_insert(0) += 1;
            }
        }
    }

    screen.placements = list
        .into_iter()
        .map(|(depth, (char_id, matrix))| GfxPlacement { depth, char_id, matrix })
        .collect();
    screen.unrendered = skipped.into_iter().collect();
    Ok(screen)
}


/// Parse a DefineSprite's nested tag stream to ITS frame-1 display
/// list.  Nested character definitions (shapes/texts/sprites) join
/// the shared dictionary; placements/remove-records build the list;
/// ShowFrame or End terminates.
fn parse_sprite_stream(
    body: &[u8],
    chars: &mut std::collections::BTreeMap<u16, crate::frontend_gfx_shapes::GfxChar>,
) -> Result<Vec<GfxPlacement>, String> {
    let mut list: std::collections::BTreeMap<u16, (u16, [f32; 6])> = Default::default();
    let mut r = ByteReader::new(body);
    let mut guard = 0u32;
    loop {
        guard += 1;
        if guard > 1_000_000 {
            return Err("gfx: sprite sub-stream ran away".into());
        }
        let code_len = r.u16()?;
        let code = (code_len >> 6) as u8;
        let mut len = (code_len & 0x3f) as usize;
        if len == 0x3f {
            len = r.u32()? as usize;
        }
        match code {
            0 => break,        // End
            1 => break,        // ShowFrame — frame 1 done
            26 | 27 => {
                let body = r.bytes(len)?.to_vec();
                parse_place_record(&body, code, &mut list)?;
            }
            28 => {
                let mut t = r.take_tag_body(len)?;
                let depth = t.u16()?;
                list.remove(&depth);
            }
            2 | 22 | 32 | 37 | 39 => {
                let nested = r.bytes(len)?.to_vec();
                match code {
                    39 => {
                        let mut sr = ByteReader::new(&nested);
                        let id = sr.u16()?;
                        let _frames = sr.u16()?;
                        let sub = parse_sprite_stream(sr.bytes(sr.remaining())?, chars)?;
                        chars.insert(id, crate::frontend_gfx_shapes::GfxChar::Sprite(sub));
                    }
                    37 => {
                        let text = crate::frontend_gfx_shapes::parse_edit_text(&nested)?;
                        let id = (nested[0] as u16) | ((nested[1] as u16) << 8);
                        chars.insert(id, crate::frontend_gfx_shapes::GfxChar::Text(text));
                    }
                    _ => {
                        let version = match code { 2 => 1u8, 22 => 2, _ => 3 };
                        let shape = crate::frontend_gfx_shapes::parse_shape(&nested, version)?;
                        let id = (nested[0] as u16) | ((nested[1] as u16) << 8);
                        chars.insert(id, crate::frontend_gfx_shapes::GfxChar::Shape(shape));
                    }
                }
            }
            _ => {
                let _ = r.bytes(len)?;
            }
        }
    }
    Ok(list
        .into_iter()
        .map(|(depth, (char_id, matrix))| GfxPlacement { depth, char_id, matrix })
        .collect())
}

/// One PlaceObject2/3 record: (depth, char id, matrix) into the
/// display list (the Move case re-transforms the existing depth).
fn parse_place_record(
    body: &[u8],
    code: u8,
    list: &mut std::collections::BTreeMap<u16, (u16, [f32; 6])>,
) -> Result<(), String> {
    let mut t = ByteReader::new(body);
    let mut f2 = 0u8;
    if code == 27 {
        let _f1 = t.u8()?;
        f2 = t.u8()?;
    }
    let flags = t.u8()?;
    let depth = t.u16()?;
    let has_char = flags & 0x02 != 0;
    let has_matrix = flags & 0x04 != 0;
    if code == 27 && f2 & 0x20 != 0 {
        // ClassName (null-terminated) — skip.
        loop {
            if t.u8()? == 0 {
                break;
            }
        }
    }
    let char_id = if has_char { Some(t.u16()?) } else { None };
    let matrix = if has_matrix {
        let mut br = BitReader::new(t.bytes(t.remaining())?);
        parse_matrix(&mut br)?
    } else {
        [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
    };
    match (char_id, list.get_mut(&depth)) {
        (Some(id), _) => {
            list.insert(depth, (id, matrix));
        }
        (None, Some(slot)) => slot.1 = matrix,
        (None, None) => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// frame-1 renderer — the REAL frontend shader contract
// ---------------------------------------------------------------------------

/// Render the parsed screen's frame 1 into `width × height` RGBA with
/// the stage letterboxed inside (aspect fit, centred; the background
/// colour fills the letterbox bars).  Bitmaps ride the REAL frontend
/// shader contract through [`render_sprites_textured`] — depth order
/// painter overwrite (no blending yet — DESIGN, see module docs).
pub fn render_gfx_first_frame(
    screen: &GfxScreen,
    width: u32,
    height: u32,
) -> Result<FrontendFrame, String> {
    if width == 0 || height == 0 {
        return Err("gfx: empty render target".into());
    }
    let (xmin, ymin, xmax, ymax) = (screen.rect[0], screen.rect[1], screen.rect[2], screen.rect[3]);
    let stage_w = xmax - xmin;
    let stage_h = ymax - ymin;
    if stage_w <= 0.0 || stage_h <= 0.0 {
        return Err("gfx: degenerate stage RECT".into());
    }
    let stage_w_px = stage_w / 20.0;
    let stage_h_px = stage_h / 20.0;
    let scale = (width as f32 / stage_w_px).min(height as f32 / stage_h_px);
    let off_x = (width as f32 - stage_w_px * scale) / 2.0;
    let off_y = (height as f32 - stage_h_px * scale) / 2.0;

    let mut sprites = Vec::new();
    let mut textures: Vec<Option<TexRef<'_>>> = Vec::new();
    for pl in &screen.placements {
        let Some(img) = screen.images.iter().find(|i| i.id == pl.char_id) else {
            continue; // a placed character we did not extract (shape/font) — skip
        };
        let [a, b, c, d, tx, ty] = pl.matrix;
        // The bitmap's rect corners in twips (1 px = 20 twips),
        // TL → TR → BR → BL in stage space (y down).
        let (w_tw, h_tw) = (img.width as f32 * 20.0, img.height as f32 * 20.0);
        let corners_tw = [
            [0.0, 0.0],
            [w_tw, 0.0],
            [w_tw, h_tw],
            [0.0, h_tw],
        ];
        let mut clip = [[0.0f32; 2]; 4];
        for (k, p) in corners_tw.iter().enumerate() {
            let x = a * p[0] + c * p[1] + tx;
            let y = b * p[0] + d * p[1] + ty;
            let px = off_x + (x - xmin) / 20.0 * scale;
            let py = off_y + (y - ymin) / 20.0 * scale;
            clip[k] = [px / width as f32 * 2.0 - 1.0, 1.0 - py / height as f32 * 2.0];
        }
        // FrontendSprite quad corner order is [BL, BR, TR, TL]
        // ([(-1,-1), (1,-1), (1,1), (-1,1)]) — remap, and give each
        // corner its own texture UV.  Sampling origin is TOP-left
        // (v grows downward — the frontend path's convention): the
        // screen top corners take v=0.
        let quad = [clip[3], clip[2], clip[1], clip[0]];
        let uv = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        sprites.push(FrontendSprite {
            quad,
            uv_override: Some(uv),
            textured: true,
            ..FrontendSprite::fullscreen()
        });
        textures.push(Some((img.rgba.as_slice(), img.width, img.height)));
    }
    render_sprites_textured(&sprites, &textures, width, height, screen.background)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Little-endian inline RECORDHEADER + body.
    fn tag(code: u8, body: &[u8]) -> Vec<u8> {
        assert!(body.len() < 0x3f, "test tags stay inline-lengthed");
        // u16 arithmetic — codes above 3 (e.g. 36) overflow a u8 shift.
        let cl = ((code as u16) << 6) | body.len() as u16;
        let mut v = cl.to_le_bytes().to_vec();
        v.extend_from_slice(body);
        v
    }

    /// Long-form RECORDHEADER (u32 length) — the 8×8 image body
    /// exceeds the 0x3f inline limit.
    fn tag_long(code: u8, body: &[u8]) -> Vec<u8> {
        let mut v = ((code as u16) << 6 | 0x3f).to_le_bytes().to_vec();
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(body);
        v
    }

    /// A RECT with fixed 16-bit fields (fits any test stage).
    fn rect(xmin: i32, ymin: i32, xmax: i32, ymax: i32) -> Vec<u8> {
        let mut b = BitWriter::new();
        b.write(5, 16);
        for v in [xmin, xmax, ymin, ymax] {
            b.write(16, v as u32);
        }
        b.finish()
    }

    /// MSB-first bit writer (tests only).
    struct BitWriter {
        out: Vec<u8>,
        pos: usize,
    }
    impl BitWriter {
        fn new() -> Self {
            Self { out: vec![], pos: 0 }
        }
        fn write(&mut self, n: u32, v: u32) {
            for i in (0..n).rev() {
                let bit = (v >> i) & 1;
                if self.pos >> 3 >= self.out.len() {
                    self.out.push(0);
                }
                if bit == 1 {
                    self.out[self.pos >> 3] |= 1 << (7 - (self.pos & 7));
                }
                self.pos += 1;
            }
        }
        fn finish(mut self) -> Vec<u8> {
            self.pos = (self.pos + 7) & !7;
            while self.out.len() < self.pos >> 3 {
                self.out.push(0);
            }
            self.out
        }
    }

    /// A real inflater accepts this: RFC1950 header (0x78 0x01), ONE
    /// stored deflate block, Adler-32 trailer (BIG-endian — RFC 1950
    /// stores it MSB-first; the first test run proved it the hard
    /// way).  The tree has no compressor, so tests wrap with this.
    fn zlib_stored(raw: &[u8]) -> Vec<u8> {
        assert!(raw.len() <= 0xffff);
        let mut z = vec![0x78, 0x01, 0x01]; // deflate flags + final stored block
        z.extend_from_slice(&(raw.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
        z.extend_from_slice(raw);
        let (mut a, mut b) = (1u32, 0u32);
        for &byte in raw {
            a = (a + byte as u32) % 65521;
            b = (b + a) % 65521;
        }
        let adler = (b << 16) | a;
        z.extend_from_slice(&[
            (adler >> 24) as u8,
            (adler >> 16) as u8,
            (adler >> 8) as u8,
            adler as u8,
        ]);
        z
    }

    /// An FWS frame-1 stream: an 8×8 fmt-5 image (4×4-texel quadrant
    /// blocks: red / green / blue / white), placed full-stage through
    /// an identity matrix (the stage RECT is the image's 160-twip
    /// rect), ZLIB-WRAPPED (the tag's payload is always a zlib
    /// stream).
    fn fws_fixture() -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"FWS");
        v.push(6); // version
        v.extend_from_slice(&[0, 0, 0, 0]); // length (unverified)
        v.extend_from_slice(&rect(0, 0, 160, 160)); // 8×8 px stage
        v.extend_from_slice(&[0, 0]); // fps
        v.extend_from_slice(&[1, 0]); // 1 frame
        // DefineBitsLossless2: id 7, fmt 5, 8×8. File byte order is
        // BGRA (little-endian ARGB — the documented fmt-5 layout).
        // Quadrant blocks written ROW-MAJOR (16 consecutive texels
        // would be only TWO rows of 8 — the first fixture draft made
        // stripes, and the pixel grid caught it): rows 0..4 red|green,
        // rows 4..8 blue|white.
        let mut raw = Vec::new();
        for row in 0..8u32 {
            for col in 0..8u32 {
                let t: [u8; 4] = if row < 4 {
                    if col < 4 { [0, 0, 255, 255] } else { [0, 255, 0, 255] } // red / green
                } else if col < 4 {
                    [255, 0, 0, 255] // blue
                } else {
                    [255, 255, 255, 255] // white
                };
                raw.extend_from_slice(&t);
            }
        }
        let mut img = Vec::new();
        img.extend_from_slice(&7u16.to_le_bytes());
        img.push(5);
        img.extend_from_slice(&8u16.to_le_bytes());
        img.extend_from_slice(&8u16.to_le_bytes());
        img.extend_from_slice(&zlib_stored(&raw));
        v.extend_from_slice(&tag_long(36, &img));
        // PlaceObject2: has_character | has_matrix, depth 1, id 7,
        // identity matrix (via the bit-packed MATRIX: no scale, no
        // rotate, 0-bit translate).
        let mut po = Vec::new();
        po.push(0x02 | 0x04);
        po.extend_from_slice(&1u16.to_le_bytes());
        po.extend_from_slice(&7u16.to_le_bytes());
        po.extend_from_slice(&[0x00, 0x00]); // MATRIX: no flags, 0-bit translate
        v.extend_from_slice(&tag(26, &po));
        v.extend_from_slice(&tag(1, &[])); // ShowFrame — stop here
        v.extend_from_slice(&tag(0, &[])); // End
        v
    }

    #[test]
    fn parses_and_renders_fws_first_frame() {
        let screen = parse_gfx(&fws_fixture()).unwrap();
        assert_eq!(screen.rect, [0.0, 0.0, 160.0, 160.0]);
        assert_eq!(screen.images.len(), 1);
        assert_eq!(screen.images[0].id, 7);
        assert_eq!((screen.images[0].width, screen.images[0].height), (8, 8));
        assert_eq!(screen.placements.len(), 1);
        assert_eq!(screen.placements[0].char_id, 7);
        // Render 40×30: the square stage letterboxes to a centred
        // 30×30 field (off_x = 5).  TL quadrant red, TR green, BL
        // blue, BR white — sample INSIDE quadrant texel blocks (the
        // image is 8×8, bilinear blends only near block boundaries).
        let f = render_gfx_first_frame(&screen, 40, 30).unwrap();
        let px = |x: usize, y: usize| {
            let o = (y * 40 + x) * 4;
            [f.rgba[o], f.rgba[o + 1], f.rgba[o + 2], f.rgba[o + 3]]
        };
        assert_eq!(px(12, 7), [255, 0, 0, 255], "TL quadrant red");
        assert_eq!(px(30, 7), [0, 255, 0, 255], "TR quadrant green");
        assert_eq!(px(12, 24), [0, 0, 255, 255], "BL quadrant blue");
        assert_eq!(px(30, 24), [255, 255, 255, 255], "BR quadrant white");
        // Letterbox bars (left of the centred field) = background.
        assert_eq!(px(1, 15), [0, 0, 0, 255], "letterbox bar");
    }

    #[test]
    fn gfx_signature_is_swf_compatible() {
        // The real members are `GFX`-magic — the same stream must
        // parse identically through that signature.
        let mut v = fws_fixture();
        v[..3].copy_from_slice(b"GFX");
        let screen = parse_gfx(&v).unwrap();
        assert_eq!(screen.images.len(), 1);
        assert_eq!(screen.placements.len(), 1);
    }

    #[test]
    fn cws_body_inflates_through_the_proven_zlib() {
        // The compressed-signature path: same stream, body zlib-wrapped.
        let body = {
            let raw = {
                let mut v = Vec::new();
                v.extend_from_slice(&rect(0, 0, 800, 600));
                v.extend_from_slice(&[0, 0]); // fps
                v.extend_from_slice(&[1, 0]); // frames
                v.extend_from_slice(&tag(1, &[]));
                v.extend_from_slice(&tag(0, &[]));
                v
            };
            zlib_stored(&raw)
        };
        let mut v = Vec::new();
        v.extend_from_slice(b"CWS");
        v.push(6);
        v.extend_from_slice(&[0, 0, 0, 0]);
        v.extend_from_slice(&body);
        let screen = parse_gfx(&v).unwrap();
        assert_eq!(screen.rect, [0.0, 0.0, 800.0, 600.0]);
        assert_eq!(screen.placements.len(), 0);
    }

    #[test]
    fn colormap_and_15bit_formats_decode() {
        // fmt 3: 2×1 colormap (2 colors: red, translucent blue).
        let mut v = Vec::new();
        v.extend_from_slice(b"FWS");
        v.push(6);
        v.extend_from_slice(&[0, 0, 0, 0]);
        v.extend_from_slice(&rect(0, 0, 40, 20));
        v.extend_from_slice(&[0, 0, 1, 0]);
        let mut img = Vec::new();
        img.extend_from_slice(&3u16.to_le_bytes());
        img.push(3);
        img.extend_from_slice(&2u16.to_le_bytes());
        img.extend_from_slice(&1u16.to_le_bytes());
        img.push(1); // color table size: 1 → 2 entries
        // zlib stream (stored) over: 2 index bytes (row padded to 4)
        // + 2 palette entries in the same BGRA-in-file order fmt 5
        // uses (the honest UNKNOWN — see the assert below).
        let raw = {
            let mut r = vec![0u8, 1u8, 0, 0]; // row: idx 0, idx 1, pad
            r.extend_from_slice(&[0, 0, 255, 255]); // entry 0: red
            r.extend_from_slice(&[255, 0, 0, 128]); // entry 1: blue a=128
            r
        };
        img.extend_from_slice(&zlib_stored(&raw));
        v.extend_from_slice(&tag(36, &img));
        v.extend_from_slice(&tag(0, &[]));
        let screen = parse_gfx(&v).unwrap();
        assert_eq!(screen.images.len(), 1);
        // idx 0 → decoded [255,0,0,255] red; idx 1 → file entry
        // [255,0,0,128] read in the same BGRA-in-file order fmt 5
        // uses → RGBA (255, 0, 0) with alpha 128.  (The colormap byte
        // order for Lossless2 is an honest UNKNOWN — the on-device
        // render journal reports per-screen what was decoded; real
        // art will confirm.)
        assert_eq!(&screen.images[0].rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(&screen.images[0].rgba[4..8], &[0, 0, 255, 128]);
    }

}

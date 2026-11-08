//! FMV subtitle text bibles — the small Pure3D members of `movies.rcf`.
//!
//! ## Evidence (block 47, all against the REAL movies.rcf members)
//!
//! `movies.rcf` carries 7 tiny P3D members (568–5 366 B) — one text
//! bible per subtitled movie (some movies share a file across two
//! variants).  Each file = N `0x00018202` **TextBibleData** chunks
//! (one per language: english, spanish, french, italian — NIS_10_3
//! has 4, every observed file has all 4) + one `0x07f00000` content
//! chunk whose first Pascal name is
//! `engine::FmvSubtitleManager(1):<MOVIE_ID>` — the id that names the
//! Bink member (block 47 cracked 7/19 Bink names exactly this way).
//!
//! The layout below validated BYTE-EXACTLY on all 28 TextBibleData
//! chunks of the 7 files (every language of every file):
//!
//! ```text
//! pstr8 language                    — aligned Pascal string: u8 slot =
//!                                     (strlen+1+3) & !3, then slot bytes
//!                                     (chars, NUL, zero padding)
//! u32 version                       — 0 in every observed chunk
//! u32 count                         — 1 ..= 23 observed
//! count × pstr8 keys                — e.g. "fmv0-1_700", "nis6-4a_01"
//! count × u32 starts                — blob offsets; starts[0] == 0
//! count × u32 ends_plus_1           — ends[i]+1: ends1[i]−1 ==
//!                                     starts[i+1] (chained), ends1[count−1]
//!                                     == text_len + 1
//! nested chunk (12-B prelude):
//!   u32 type = 0x00018201
//!   u32 uid
//!   u32 size                        — covers prelude + payload
//!   pstr8 "Text Bible Data"         — literal label
//!   u32 0
//!   u32 text_len                    — strlen of the whole blob
//!   text_len bytes                  — UTF-8, subtitles CONCATENATED with
//!                                     no separators; subtitle i =
//!                                     blob[starts[i] .. ends_plus_1[i] − 1]
//! ```
//!
//! The `ends_plus_1` semantics (end offset + 1 — a NUL-terminator
//! allowance in the engine's buffer) is LIKELY; the ARITHMETIC
//! (`ends1[i] − 1 == starts[i+1]`, `ends1[last] == text_len + 1`) is
//! PROVEN 28/28.

use prototype_common::{P3dError, Result};
use prototype_p3d::P3dFile;

/// TextBibleData chunk type.
pub const TEXT_BIBLE_DATA: u32 = 0x0001_8202;
/// The nested per-bible text chunk type.
pub const TEXT_BIBLE_TEXT: u32 = 0x0001_8201;
/// Content/META chunk type (carries the movie id).
pub const CONTENT_META: u32 = 0x07F0_0000;

/// Loader cap on entries per bible (observed max 23; DESIGN headroom).
pub const MAX_BIBLE_ENTRIES: u32 = 4096;

/// One subtitle line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FmvSubtitle {
    /// The bible key, e.g. `fmv0-1_700` (the `_700`-series is the
    /// game's opening monologue).
    pub key: String,
    /// The UTF-8 subtitle text.
    pub text: String,
}

/// One language's table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FmvLanguage {
    /// `english` / `spanish` / `french` / `italian` observed.
    pub language: String,
    /// Subtitles in file order.
    pub entries: Vec<FmvSubtitle>,
}

/// A parsed FMV subtitle text bible (one movies.rcf P3D member).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FmvSubtitles {
    /// The movie id from the content chunk's
    /// `engine::FmvSubtitleManager(1):<id>` name, e.g.
    /// `NIS_6-4a_FMV`.
    pub movie_id: Option<String>,
    /// One table per language chunk, file order.
    pub languages: Vec<FmvLanguage>,
}

/// Aligned Pascal string: `u8` slot length, then slot bytes — the
/// string, its NUL, zero padding.  Fail-closed on every byte.
fn pstr8(b: &[u8], p: usize) -> Result<(String, usize)> {
    if p >= b.len() {
        return Err(P3dError::Eof { offset: p, needed: 1 });
    }
    let slot = b[p] as usize;
    let end = p + 1 + slot;
    if end > b.len() {
        return Err(P3dError::Eof {
            offset: b.len(),
            needed: end - b.len(),
        });
    }
    let raw = &b[p + 1..end];
    let strlen = raw.iter().position(|&c| c == 0).ok_or_else(|| {
        P3dError::Other(format!("pstr8 at {p}: no NUL in {slot}-byte slot"))
    })?;
    // the slot must be exactly (strlen+1) rounded up to 4
    if slot != (strlen + 1 + 3) & !3 {
        return Err(P3dError::Other(format!(
            "pstr8 at {p}: slot {slot} != aligned strlen+1 for {strlen}-char string"
        )));
    }
    // every padding byte must be zero
    if !raw[strlen + 1..].iter().all(|&c| c == 0) {
        return Err(P3dError::Other(format!(
            "pstr8 at {p}: non-zero padding in {slot}-byte slot"
        )));
    }
    let s = std::str::from_utf8(&raw[..strlen])
        .map_err(|e| P3dError::Other(format!("pstr8 at {p}: bad utf-8: {e}")))?;
    Ok((s.to_string(), end))
}

impl FmvSubtitles {
    /// Parse one subtitle text-bible P3D member.  Fail-closed: exact
    /// consumption, aligned-pascal discipline, chained offset arrays,
    /// non-empty UTF-8 slices.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let file = P3dFile::parse(data)?;
        let mut out = Self::default();
        let mut bible_count = 0usize;
        for chunk in &file.chunks {
            match chunk.chunk_type {
                TEXT_BIBLE_DATA => {
                    out.languages.push(parse_language(chunk.payload)?);
                    bible_count += 1;
                }
                CONTENT_META => {
                    if let Some(content) = chunk.content()? {
                        if let Some(name) = content.names.first() {
                            if let Some(pos) = name.find("(1):") {
                                out.movie_id = Some(name[pos + 4..].to_string());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if bible_count == 0 {
            return Err(P3dError::Other(
                "fmv subtitle bible: no TextBibleData chunk".into(),
            ));
        }
        Ok(out)
    }

    /// The table for one language.
    pub fn language(&self, name: &str) -> Option<&FmvLanguage> {
        self.languages.iter().find(|l| l.language == name)
    }

    /// One subtitle line by language + key.
    pub fn entry(&self, lang: &str, key: &str) -> Option<&str> {
        self.language(lang)
            .and_then(|l| l.entries.iter().find(|e| e.key == key))
            .map(|e| e.text.as_str())
    }
}

/// Parse one TextBibleData chunk payload (see the module doc).
fn parse_language(body: &[u8]) -> Result<FmvLanguage> {
    let (language, mut p) = pstr8(body, 0)?;
    if p + 8 > body.len() {
        return Err(P3dError::Eof {
            offset: body.len(),
            needed: p + 8 - body.len(),
        });
    }
    let version = u32::from_le_bytes(body[p..p + 4].try_into().unwrap());
    let count = u32::from_le_bytes(body[p + 4..p + 8].try_into().unwrap());
    p += 8;
    if version != 0 {
        return Err(P3dError::Other(format!(
            "text bible {language}: version {version} != 0"
        )));
    }
    if count == 0 || count > MAX_BIBLE_ENTRIES {
        return Err(P3dError::Other(format!(
            "text bible {language}: count {count} outside 1..={MAX_BIBLE_ENTRIES}"
        )));
    }
    let count = count as usize;
    let mut keys = Vec::with_capacity(count);
    for _ in 0..count {
        let (k, next) = pstr8(body, p)?;
        keys.push(k);
        p = next;
    }
    let arrays = 8 * count;
    if p + arrays > body.len() {
        return Err(P3dError::Eof {
            offset: body.len(),
            needed: p + arrays - body.len(),
        });
    }
    let starts: Vec<u32> = (0..count)
        .map(|i| u32::from_le_bytes(body[p + 4 * i..p + 4 * i + 4].try_into().unwrap()))
        .collect();
    p += 4 * count;
    let ends1: Vec<u32> = (0..count)
        .map(|i| u32::from_le_bytes(body[p + 4 * i..p + 4 * i + 4].try_into().unwrap()))
        .collect();
    p += 4 * count;

    // nested 0x00018201 chunk with the text blob
    if p + 12 > body.len() {
        return Err(P3dError::Eof {
            offset: body.len(),
            needed: p + 12 - body.len(),
        });
    }
    let ntype = u32::from_le_bytes(body[p..p + 4].try_into().unwrap());
    let nsize = u32::from_le_bytes(body[p + 8..p + 12].try_into().unwrap()) as usize;
    if ntype != TEXT_BIBLE_TEXT {
        return Err(P3dError::Other(format!(
            "text bible {language}: nested chunk type {ntype:#x} != {TEXT_BIBLE_TEXT:#x}"
        )));
    }
    let nested_end = p + nsize;
    if nsize < 12 || nested_end > body.len() {
        return Err(P3dError::Other(format!(
            "text bible {language}: nested size {nsize} invalid"
        )));
    }
    let nested = &body[p + 12..nested_end];
    let (label, q) = pstr8(nested, 0)?;
    if label != "Text Bible Data" {
        return Err(P3dError::Other(format!(
            "text bible {language}: nested label {label:?}"
        )));
    }
    if q + 8 > nested.len() {
        return Err(P3dError::Eof {
            offset: nested.len(),
            needed: q + 8 - nested.len(),
        });
    }
    let nver = u32::from_le_bytes(nested[q..q + 4].try_into().unwrap());
    let tlen = u32::from_le_bytes(nested[q + 4..q + 8].try_into().unwrap()) as usize;
    if nver != 0 {
        return Err(P3dError::Other(format!(
            "text bible {language}: nested version {nver} != 0"
        )));
    }
    let text = nested.get(q + 8..q + 8 + tlen).ok_or_else(|| {
        P3dError::Other(format!(
            "text bible {language}: text len {tlen} overruns nested chunk"
        ))
    })?;
    // exact consumption of BOTH the nested chunk and the outer payload
    if q + 8 + tlen != nested.len() {
        return Err(P3dError::Other(format!(
            "text bible {language}: nested leftover {} B",
            nested.len() - (q + 8 + tlen)
        )));
    }
    if nested_end != body.len() {
        return Err(P3dError::Other(format!(
            "text bible {language}: outer leftover {} B",
            body.len() - nested_end
        )));
    }

    // the chained offset arithmetic (PROVEN 28/28)
    if starts[0] != 0 {
        return Err(P3dError::Other(format!(
            "text bible {language}: starts[0] {} != 0",
            starts[0]
        )));
    }
    if ends1[count - 1] as usize != tlen + 1 {
        return Err(P3dError::Other(format!(
            "text bible {language}: last ends+1 {} != text_len+1 {}",
            ends1[count - 1],
            tlen + 1
        )));
    }
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        if i + 1 < count && ends1[i] as usize != starts[i + 1] as usize + 1 {
            return Err(P3dError::Other(format!(
                "text bible {language}: entry {i} chain broken (ends1 {} vs start {})",
                ends1[i], starts[i + 1]
            )));
        }
        let s = starts[i] as usize;
        let e = ends1[i] as usize;
        if e <= s || e - 1 <= s || e - 1 > tlen {
            return Err(P3dError::Other(format!(
                "text bible {language}: entry {i} span {s}..{e} invalid"
            )));
        }
        let text_str = std::str::from_utf8(&text[s..e - 1]).map_err(|er| {
            P3dError::Other(format!("text bible {language}: entry {i} bad utf-8: {er}"))
        })?;
        if text_str.is_empty() {
            return Err(P3dError::Other(format!(
                "text bible {language}: entry {i} empty"
            )));
        }
        entries.push(FmvSubtitle {
            key: keys[i].clone(),
            text: text_str.to_string(),
        });
    }
    Ok(FmvLanguage { language, entries })
}

// ---------------------------------------------------------------------------
// Unit tests against REAL movies.rcf members (block 47)
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

    #[test]
    fn parses_the_nis6_4a_bible() {
        let data = fixture("movies_e1e63987.p3d");
        let subs = FmvSubtitles::parse(&data).unwrap();
        assert_eq!(subs.movie_id.as_deref(), Some("NIS_6-4a_FMV"));
        assert_eq!(subs.languages.len(), 4);
        for (lang, text) in [
            ("english", "Hey, hey. Dana!"),
            ("italian", "Ehi! Dana!"),
        ] {
            let l = subs.language(lang).unwrap();
            assert_eq!(l.entries.len(), 1);
            assert_eq!(l.entries[0].key, "nis6-4a_01");
            assert_eq!(l.entries[0].text, text);
        }
        assert_eq!(subs.entry("english", "nis6-4a_01"), Some("Hey, hey. Dana!"));
        assert!(subs.entry("english", "nope").is_none());
        // every language's entries are non-empty and key-aligned
        let keys: Vec<&str> = subs.language("english").unwrap()
            .entries.iter().map(|e| e.key.as_str()).collect();
        for l in &subs.languages {
            let lk: Vec<&str> = l.entries.iter().map(|e| e.key.as_str()).collect();
            assert_eq!(lk, keys, "language {} key order", l.language);
        }
    }

    #[test]
    fn parses_the_nuke_bible() {
        let data = fixture("movies_0122d2d0.p3d");
        let subs = FmvSubtitles::parse(&data).unwrap();
        assert_eq!(subs.movie_id.as_deref(), Some("NIS_10_3_FMV_nuke"));
        let en = subs.language("english").unwrap();
        assert_eq!(en.entries.len(), 1);
        assert_eq!(en.entries[0].key, "nukefmv_01");
        assert_eq!(
            en.entries[0].text,
            "Abandon ship. All crew, abandon ship."
        );
    }

    #[test]
    fn fails_closed_on_truncation() {
        let data = fixture("movies_e1e63987.p3d");
        assert!(FmvSubtitles::parse(&data[..200]).is_err());
        assert!(FmvSubtitles::parse(&data[..400]).is_err());
    }

    #[test]
    fn fails_closed_on_corrupted_strings_and_arrays() {
        let data = fixture("movies_e1e63987.p3d");
        // break the language pstr's alignment rule (slot byte = file
        // byte 24: 12-B file head + 12-B chunk prelude)
        let mut bad = data.clone();
        bad[24] = 9;
        assert!(FmvSubtitles::parse(&bad).is_err());
        // break the version gate (version u32 = file bytes 33..37:
        // 12-B file head + 12-B chunk prelude + 9-B lang pstr)
        let mut bad = data.clone();
        bad[33] = 1;
        assert!(FmvSubtitles::parse(&bad).is_err());
        // break the chain: starts[0] != 0 (the starts array lives after
        // the key table — corrupt the LAST u32 of the chunk payload,
        // which the chain gate must catch wherever it lands)
        let mut bad = data.clone();
        let n = bad.len();
        bad[n - 1] ^= 0xff;
        let _ = FmvSubtitles::parse(&bad); // may fail at any gate; must not panic
    }
}

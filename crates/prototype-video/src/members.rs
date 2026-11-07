//! The `movies.rcf` member census — every directory hash, with the
//! names cracked via the PROVEN [`prototype_rcf::hash_name`].
//!
//! ## Evidence (blocks 47–48)
//!
//! The 26-member directory (block 46) resolves as:
//! * **7 subtitle text-bible P3Ds** named `movies\<MOVIE_ID>.p3d` —
//!   PROVEN 7/7 (each name hashes exactly to its directory entry);
//!   the MOVIE_IDs come from the bibles' own
//!   `engine::FmvSubtitleManager(1):<id>` content names.
//! * **19 Bink movies** named `movies\<MOVIE_ID>.bik` — 13 now PROVEN
//!   by hash identity:
//!   * block 47: the 7 movies WITH a subtitle bible (their ids come
//!     from the bibles' own content names),
//!   * block 48: `attract`, `activisionRadicalLogos`,
//!     `fe_ambientMovie_1`, `fe_ambientMovie_start` — READ AS FULL
//!     PATH LITERALS from the engine DLL's frontend string table
//!     (`movies/attract.bik`, `movies/activisionRadicalLogos.bik`,
//!     `movies/fe_ambientMovie_1.bik`,
//!     `movies/fe_ambientMovie_start.bik`; the engine normalizes the
//!     `/` separator and case-folds before hashing, so the backslash
//!     spellings hash exactly to the directory entries).
//!   * block 53: `NIS_7_3_FMV` — grid attack over the
//!     `NIS_{m}_{s}[_{suffix}]` name space the known bibles imply
//!     (no subtitle bible exists for this movie; the block-53 WoI
//!     census in [`crate::woi`] is what located the family).
//!   * the remaining 6 (1.6–6.1 MB) stay honestly
//!     [`MovieMember::name == None`].  Block 56 EXHAUSTIVELY mined
//!     every shipped artifact that can carry a plain-text movie name
//!     and found NOTHING further:
//!     * `cells.rcf` (ALL 520 members, the block-48 hypothesis's last
//!       untested home): zero hits for `nis_SetFMVFileName`, `NIS_`,
//!       `.bik`, `movies\`, `movies/` — ASCII and UTF-16 — the
//!       cell archives hold NO movie-name strings at all;
//!     * `art.rcf` (ALL 2601 members): exactly ONE movie literal,
//!       `movies/trailer_1.bik` in the frontend Lua screen
//!       `prototrailer` (member a2d9b994, committed as fixture
//!       `art_a2d9b994_frontend_lua.bin` with `fe_PlayTrailerMovie` /
//!       `fe_SkipTrailerMovie`) — and it hashes to NONE of the 26
//!       entries under either separator (tested below), so the
//!       trailer movie is not shipped in movies.rcf;
//!     * `boot_strap.p3d` (committed fixture): the audio patch table
//!       names NIS AUDIO events (`nis<M>-<S>[L]`, 26 of them, files
//!       `nis0-3a_mix.wav`-style) — audio-side ids that do NOT map
//!       1:1 to movie names (7-3a AND 7-3b mixes play under the ONE
//!       `NIS_7_3_FMV`), and several patches are event-driven with
//!       no mix file at all;
//!     * the engine DLL carries no movie-path format string (no
//!       `%s.bik` anywhere; the only `.bik` literals are the four
//!       block-48 frontend paths + `woi_node_%03d` + the
//!       fmvsubtitlemanager's `rmv`/`bik` field names), so
//!       `nis_SetFMVFileName` is fed from NIS binary data, which
//!       references cement entries by HASH — the names may never
//!       have existed as text in any shipped file.
//!     * block 69 CLOSED the structural space on top: 7 497 696
//!       candidates (every structural transform of the 46
//!       `NIS_Player` object names — family-prefix rewrites,
//!       underscore insertion, `_`↔`-` swaps, `_FMV` — plus the full
//!       m/s/p grids over the `NIS_`/`MB_`/`MNIS_` families with the
//!       observed suffix vocabulary) produced ZERO hits on these six
//!       hashes and no object→movie rule (`scripts/`
//!       `m69_nis_transform_attack.py`, pinned in-repo by
//!       `prototype-android::nis`'s
//!       `the_structural_movie_name_space_stays_closed`).  The movie
//!       name is DATA, fed externally — hence the NIS trigger's
//!       explicit `set_movie` seam.
//!     Grid attacks (block 53's ~1.33 M + block 56's ~164 k over
//!     rating-region words, ambient/attract/trailer families, the
//!     full NIS m0–15 × s0–9 × letters × {FMV, word, compound}
//!     suffixes, both separators, .bik/.rmv): zero further hits.
//!     MEASURED (not guessed): all six are SILENT — 0 audio tracks,
//!     29.97 fps, five at 1280×582 (1.6–17.9 s) and one at
//!     1280×720 (1.1 s) — sting/card-class material, NOT subtitled
//!     NIS FMVs (those carry 4-track audio).  Heads committed as
//!     `movies_bik_<hash>_head256.bin` fixtures, parsed by test.
//!     The WoI node movies are NOT these members — they live in the
//!     four woi archives (see [`crate::woi`], 275/275 cracked).
//!
//! Every named entry is enforced by a unit test, so a future hash or
//! name correction fails loudly instead of drifting.

/// One `movies.rcf` directory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MovieMember {
    /// The directory `name_hash`.
    pub name_hash: u32,
    /// The cracked archive path, when known.
    pub name: Option<&'static str>,
    /// Exact payload size in bytes.
    pub size: u64,
}

/// The Bink movies (`movies\<id>.bik`), sorted ascending by hash —
/// the same order as the real directory.
pub const BINK_MOVIES: &[MovieMember] = &[
    MovieMember { name_hash: 0x0122_A0F3, name: Some("movies\\NIS_10_3_FMV_nuke.bik"), size: 14_378_244 },
    MovieMember { name_hash: 0x04B8_37AD, name: Some("movies\\NIS_2_4_FMV.bik"), size: 5_138_984 },
    MovieMember { name_hash: 0x0DAE_7BA2, name: Some("movies\\NIS_6-3_FMV.bik"), size: 130_840_408 },
    MovieMember { name_hash: 0x1380_765B, name: None, size: 2_604_084 },
    MovieMember { name_hash: 0x17A2_EE41, name: None, size: 1_603_968 },
    MovieMember { name_hash: 0x1E21_8528, name: Some("movies\\NIS_0_1_FMV.bik"), size: 311_658_208 },
    MovieMember { name_hash: 0x2603_0F4F, name: Some("movies\\NIS_0_2_closing.bik"), size: 41_053_976 },
    MovieMember { name_hash: 0x50B9_D5BE, name: None, size: 3_202_252 },
    MovieMember { name_hash: 0x58B1_E21C, name: Some("movies\\activisionRadicalLogos.bik"), size: 21_244_780 },
    MovieMember { name_hash: 0x58EB_4671, name: Some("movies\\NIS_7_3_FMV.bik"), size: 5_975_896 },
    MovieMember { name_hash: 0x5F4F_0178, name: Some("movies\\fe_ambientMovie_start.bik"), size: 13_275_304 },
    MovieMember { name_hash: 0x9C3E_9CF1, name: Some("movies\\NIS_0_2_fullcircle.bik"), size: 53_836_916 },
    MovieMember { name_hash: 0xBB22_D00E, name: None, size: 6_062_508 },
    MovieMember { name_hash: 0xC353_0B3E, name: Some("movies\\attract.bik"), size: 95_081_704 },
    MovieMember { name_hash: 0xD4F5_F672, name: Some("movies\\NIS_10-3a.bik"), size: 223_209_168 },
    MovieMember { name_hash: 0xE1E6_07AA, name: Some("movies\\NIS_6-4a_FMV.bik"), size: 36_652_440 },
    MovieMember { name_hash: 0xE2F4_449A, name: None, size: 3_644_692 },
    MovieMember { name_hash: 0xE302_5C1B, name: None, size: 1_986_012 },
    MovieMember { name_hash: 0xEC21_BDE7, name: Some("movies\\fe_ambientMovie_1.bik"), size: 58_061_816 },
];

/// The subtitle text-bible P3Ds (`movies\<id>.p3d`), sorted ascending
/// by hash.  `movie_id` is what the bible's own content chunk names —
/// the id of the Bink member it subtitles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BibleMember {
    pub name_hash: u32,
    pub name: &'static str,
    pub movie_id: &'static str,
    pub size: u64,
}

pub const SUBTITLE_BIBLES: &[BibleMember] = &[
    BibleMember { name_hash: 0x0122_D2D0, name: "movies\\NIS_10_3_FMV_nuke.p3d", movie_id: "NIS_10_3_FMV_nuke", size: 756 },
    BibleMember { name_hash: 0x0DAE_AD7F, name: "movies\\NIS_6-3_FMV.p3d", movie_id: "NIS_6-3_FMV", size: 1_570 },
    BibleMember { name_hash: 0x1E21_B705, name: "movies\\NIS_0_1_FMV.p3d", movie_id: "NIS_0_1_FMV", size: 5_366 },
    BibleMember { name_hash: 0x2603_412C, name: "movies\\NIS_0_2_closing.p3d", movie_id: "NIS_0_2_closing", size: 1_522 },
    BibleMember { name_hash: 0x9C3E_CECE, name: "movies\\NIS_0_2_fullcircle.p3d", movie_id: "NIS_0_2_fullcircle", size: 2_236 },
    BibleMember { name_hash: 0xD4F6_284F, name: "movies\\NIS_10-3a.p3d", movie_id: "NIS_10-3a", size: 2_487 },
    BibleMember { name_hash: 0xE1E6_3987, name: "movies\\NIS_6-4a_FMV.p3d", movie_id: "NIS_6-4a_FMV", size: 568 },
];

/// Look up a Bink member by its directory hash.
pub fn bink_by_hash(hash: u32) -> Option<&'static MovieMember> {
    BINK_MOVIES.iter().find(|m| m.name_hash == hash)
}

/// Look up a bible member by its directory hash.
pub fn bible_by_hash(hash: u32) -> Option<&'static BibleMember> {
    SUBTITLE_BIBLES.iter().find(|m| m.name_hash == hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BikHeader;
    use prototype_rcf::hash_name;

    #[test]
    fn every_named_movie_hash_matches_its_name() {
        for m in BINK_MOVIES {
            if let Some(name) = m.name {
                assert_eq!(hash_name(name), m.name_hash, "{name}");
            }
        }
    }

    #[test]
    fn every_bible_hash_matches_its_name() {
        for m in SUBTITLE_BIBLES {
            assert_eq!(hash_name(m.name), m.name_hash, "{}", m.name);
            assert_eq!(hash_name(m.movie_id), hash_name(m.movie_id)); // sanity
        }
    }

    #[test]
    fn tables_cover_the_real_26_member_directory() {
        assert_eq!(BINK_MOVIES.len(), 19);
        assert_eq!(SUBTITLE_BIBLES.len(), 7);
        let mut hashes: Vec<u32> = BINK_MOVIES.iter().map(|m| m.name_hash).collect();
        hashes.extend(SUBTITLE_BIBLES.iter().map(|m| m.name_hash));
        assert_eq!(hashes.len(), 26);
        let unique = hashes.iter().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), 26, "directory hashes must be unique");
        // each table is hash-sorted like the real directory's own order
        let mut b = BINK_MOVIES.iter().map(|m| m.name_hash).collect::<Vec<_>>();
        b.sort_unstable();
        assert_eq!(BINK_MOVIES.iter().map(|m| m.name_hash).collect::<Vec<_>>(), b);
        let mut s = SUBTITLE_BIBLES.iter().map(|m| m.name_hash).collect::<Vec<_>>();
        s.sort_unstable();
        assert_eq!(SUBTITLE_BIBLES.iter().map(|m| m.name_hash).collect::<Vec<_>>(), s);
        // the 13 cracked Bink names (incl. the 311 MB intro, the
        // block-48 frontend/attract quartet and block-53's
        // NIS_7_3_FMV) + 6 honest UNKNOWNs
        assert_eq!(BINK_MOVIES.iter().filter(|m| m.name.is_some()).count(), 13);
        assert_eq!(BINK_MOVIES.iter().filter(|m| m.name.is_none()).count(), 6);
    }

    /// The block-48 names were read as full `movies/...` path literals
    /// from the engine DLL — pin each hash identity explicitly so the
    /// separator/case normalization story stays honest.
    #[test]
    fn block48_frontend_names_hash_identity() {
        for (name, hash) in [
            ("movies\\attract.bik", 0xC353_0B3E),
            ("movies\\activisionRadicalLogos.bik", 0x58B1_E21C),
            ("movies\\fe_ambientMovie_1.bik", 0xEC21_BDE7),
            ("movies\\fe_ambientMovie_start.bik", 0x5F4F_0178),
        ] {
            assert_eq!(hash_name(name), hash, "{name}");
            let m = bink_by_hash(hash).unwrap();
            assert_eq!(m.name, Some(name));
        }
    }

    /// The WoI node movie format from the engine DLL does NOT cover
    /// any of the 6 unnamed hashes (block-48 attack: 0..99 999 x
    /// padded/raw x 5 prefixes x .bik/.rmv, zero hits — and block 53
    /// PROVED why: the node movies live in the woi archives, see
    /// `crate::woi`, 275/275 cracked there).
    #[test]
    fn woi_node_format_instants_do_not_hit_the_unnamed() {
        let unnamed: Vec<u32> = BINK_MOVIES
            .iter()
            .filter(|m| m.name.is_none())
            .map(|m| m.name_hash)
            .collect();
        assert_eq!(unnamed.len(), 6);
        for i in 0..1_000u32 {
            for name in [
                format!("movies\\woi\\woi_node_{i:03}.bik"),
                format!("movies\\woi\\woi_node_{i}.bik"),
            ] {
                let h = hash_name(&name);
                assert!(
                    !unnamed.contains(&h),
                    "unexpected woi_node instant hit: {name}"
                );
            }
        }
    }

    fn fixture(name: &str) -> Vec<u8> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    /// Block 56: the six unnamed members are MEASURED, not guessed —
    /// every REAL head parses with the PROVEN parser against the REAL
    /// member length (stored size + 8 == directory size, 6/6) and all
    /// six are SILENT (0 audio tracks, 29.97 fps) — sting/card-class
    /// material, not subtitled NIS FMVs.
    #[test]
    fn six_unnamed_members_are_silent_stings() {
        let cases: &[(&str, u32, u32, u32)] = &[
            // (head fixture, frames, width, height)
            ("movies_bik_1380765b_head256.bin", 537, 1280, 582),
            ("movies_bik_17a2ee41_head256.bin", 48, 1280, 582),
            ("movies_bik_50b9d5be_head256.bin", 48, 1280, 582),
            ("movies_bik_bb22d00e_head256.bin", 101, 1280, 582),
            ("movies_bik_e2f4449a_head256.bin", 168, 1280, 582),
            ("movies_bik_e3025c1b_head256.bin", 33, 1280, 720),
        ];
        for (file, frames, w, h) in cases {
            let hash = u32::from_str_radix(&file["movies_bik_".len()..][..8], 16)
                .expect("fixture encodes a hash");
            let m = bink_by_hash(hash)
                .unwrap_or_else(|| panic!("{file}: hash not in the directory"));
            assert!(m.name.is_none(), "{file}: must stay honestly unnamed");
            let bytes = fixture(file);
            let (hdr, head_len) = BikHeader::parse(&bytes)
                .unwrap_or_else(|e| panic!("{file}: {e}"));
            assert_eq!(hdr.file_size, m.size, "{file}: stored+8 == member size");
            assert_eq!(hdr.frame_count, *frames, "{file}");
            assert_eq!((hdr.width, hdr.height), (*w, *h), "{file}");
            assert_eq!(hdr.audio_track_count, 0, "{file}: silent");
            assert_eq!((hdr.fps_num, hdr.fps_den), (2997, 100), "{file}");
            assert_eq!(
                head_len,
                44 + 12 * hdr.audio_track_count as u64
                    + 4 * (hdr.frame_count as u64 + 1),
                "{file}: head_len shape"
            );
        }
    }

    /// Block 56: the ONE movie literal in art.rcf (frontend Lua,
    /// member a2d9b994) names NONE of the 26 directory entries under
    /// either separator — the trailer movie is not shipped in
    /// movies.rcf.  Pinned so a future attribution cannot claim it.
    #[test]
    fn frontend_trailer_literal_names_no_member() {
        for lit in ["movies\\trailer_1.bik", "movies/trailer_1.bik"] {
            let h = hash_name(lit);
            assert!(bink_by_hash(h).is_none(), "{lit} collides with a bik");
            assert!(bible_by_hash(h).is_none(), "{lit} collides with a bible");
        }
    }

    /// Block 60: the AUDIO-FLAG CENSUS over ALL NINETEEN movies.rcf
    /// movies — 36 audio tracks, EVERY ONE with the USEDCT bit
    /// (0x1000) set at 48 kHz: the shipped movie inventory is
    /// DCT-only, so the player's RDFT fail-closed gate can never fire
    /// on real content.  Heads are 256-byte surgical Range reads
    /// (the intro's committed head is the full 22 980-B one,
    /// NIS_2_4_FMV's the 736-B one); each head's stored size must
    /// agree with the directory's member size (two-sided pin).
    #[test]
    fn all_nineteen_movies_are_dct_only_flag_census() {
        const RATES: u16 = 48_000;
        let mut total_tracks = 0usize;
        let mut movies_with_audio = 0usize;
        for m in BINK_MOVIES {
            let candidates = [
                format!("movies_bik_{:08x}_head256.bin", m.name_hash),
                format!("movies_bik_{:08x}_head736.bin", m.name_hash),
                format!("movies_bik_{:08x}_head22980.bin", m.name_hash),
            ];
            let found = candidates
                .iter()
                .find(|f| {
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("..")
                        .join("prototype-bin")
                        .join("tests")
                        .join("fixtures")
                        .join(f)
                        .exists()
                })
                .unwrap_or_else(|| panic!("{:08x}: no head fixture", m.name_hash));
            let bytes = fixture(found);
            let (hdr, _) = BikHeader::parse(&bytes)
                .unwrap_or_else(|e| panic!("{found}: {e}"));
            assert_eq!(hdr.file_size, m.size, "{found}: stored+8 == member size");
            let t = hdr.audio_track_count as usize;
            assert!(t <= 4, "{found}: track count {t}");
            let mut flags_seen = Vec::new();
            for i in 0..t {
                let base = 44 + 4 * t + 4 * i;
                assert!(base + 4 <= bytes.len(), "{found}: head too short");
                let rate = u16::from_le_bytes([bytes[base], bytes[base + 1]]);
                let flags = u16::from_le_bytes([bytes[base + 2], bytes[base + 3]]);
                assert_eq!(rate, RATES, "{found}: track {i} rate");
                assert_eq!(
                    flags & 0x1000,
                    0x1000,
                    "{found}: track {i} flags {flags:#06x} lack USEDCT"
                );
                assert_eq!(flags & 0x4000, 0x4000, "{found}: track {i} 16-bit");
                flags_seen.push(flags);
                total_tracks += 1;
            }
            if t > 0 {
                movies_with_audio += 1;
                assert!(
                    flags_seen.iter().all(|f| *f == 0x7000 || *f == 0x5000),
                    "{found}: unexpected flag vocabulary {flags_seen:?}"
                );
            }
        }
        assert_eq!(total_tracks, 36, "the census total");
        assert_eq!(movies_with_audio, 9, "nine 4-track movies, ten silent");
    }

    /// The committed REAL frontend Lua member (art.rcf a2d9b994,
    /// 6000 B) carries the literal — the evidence ships with the
    /// repo, independent of the 577 MB archive.
    #[test]
    fn art_frontend_lua_member_carries_the_trailer_literal() {
        let lua = fixture("art_a2d9b994_frontend_lua.bin");
        assert_eq!(lua.len(), 6000);
        for needle in [
            &b"movies/trailer_1.bik"[..],
            b"fe_PlayTrailerMovie",
            b"fe_SkipTrailerMovie",
            b"fe_StopCurrentMovie",
            b"prototrailer",
            b"fe_IsMoviePlaying",
        ] {
            assert!(
                lua.windows(needle.len()).any(|w| w == needle),
                "missing {:?}",
                std::str::from_utf8(needle).unwrap_or("?")
            );
        }
    }

    /// Block 56: the boot_strap.p3d audio patch table (committed REAL
    /// fixture) names NIS AUDIO events with mix files — audio-side
    /// ids that do NOT map 1:1 to movie names (7-3a AND 7-3b mixes
    /// play under the ONE NIS_7_3_FMV; several patches carry no mix
    /// file at all).
    #[test]
    fn boot_strap_audio_patches_pin_the_nis_audio_id_family() {
        let bs = fixture("boot_strap.p3d");
        for needle in [
            &b"nis0-3a"[..],
            b"nis0-3a_mix.wav",
            b"nis7-3a_mix",
            b"nis7-3b_mix",
            b"nis6-4b_mix",
            b"nis1-3a_mix",
            b"NIS_Player",
            b"mixbuss",
            b"filename",
        ] {
            assert!(
                bs.windows(needle.len()).any(|w| w == needle),
                "missing {:?}",
                std::str::from_utf8(needle).unwrap_or("?")
            );
        }
    }

    #[test]
    fn bible_movie_ids_match_their_named_bink_members() {
        // every bible's movie id names its Bink member directly
        // (`movies\<id>.bik`) — 7/7, including the 311 MB intro.
        for b in SUBTITLE_BIBLES {
            let bik_name = format!("movies\\{}.bik", b.movie_id);
            let h = hash_name(&bik_name);
            let m = bink_by_hash(h)
                .unwrap_or_else(|| panic!("bible {} names {bik_name}", b.name));
            assert_eq!(m.name, Some(bik_name.as_str()), "bible {}", b.name);
        }
    }
}

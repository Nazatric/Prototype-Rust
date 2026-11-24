//! The four WoI archives' COMPLETE member census — every directory
//! entry resolved to its real name (block 53, NEXT-UP (b)).
//!
//! ## Evidence (all PROVEN, surgical HTTP Range reads on the v1.0
//! release; the ~3.7 GB of archives were never downloaded — the 60-B
//! headers, the 12-B/entry directories and 14 small bible members are
//! the only bytes ever fetched)
//!
//! Each archive is an ATG `CEMENT LIBRARY` RCF (60-B header,
//! `dir_offset = 60`, 12-B entries: `hash, file_offset, file_size`),
//! holding **Bink movies + their subtitle text-bible P3Ds** for the
//! Web-of-Intrigue screen:
//!
//! * `00woi.rcf` (864 MB, 25 members) — the 7 ENDING cinematics
//!   `movies\woi\woi_ec1..7.bik` (73–192 MB) + node movies 001,
//!   003–007 with 5 bibles (node_002 does not exist; node_007's
//!   bible ships in 01woi);
//! * `01woi.rcf` (951 MB, 84) / `02woi.rcf` (948 MB, 70) /
//!   `03woi.rcf` (1 000 MB, 96) — node movies continuing 008…221 in
//!   a SPARSE id set (131 node movies total) with 131 bibles;
//! * every name resolves through [`prototype_rcf::hash_name`] —
//!   **275/275 entries cracked**; the block-48 format-string attack
//!   failed only because it was aimed at the movies.rcf hashes.
//!
//! The 12 fetched bibles name their movies in the engine's own
//! content chunks: `engine::FmvSubtitleManager(1):woi_node_001` …
//! `woi_ec7` — the same shape [`crate::subtitle`] parses for
//! movies.rcf; two of them ship as REAL fixtures
//! (`woi_bible_node001.p3d`, `woi_bible_ec1.p3d`).
//!
//! Honest notes: the directory `file_size` of the big BIK members
//! disagrees with the BINK head's stored size by small paddings
//! (woi_ec1: directory 84 089 184 vs stored+8 84 089 952 — the
//! engine pads member spans; UNKNOWN which the loader trusts).  The
//! 7 still-unnamed movies.rcf members (1.6–6 MB) are NOT woi-named:
//! ~1.33 M candidate names across every observable family
//! (NIS_{m}_{s}[_{desc}][{cut}], woi_node/woi_ec, frontend, subtitle-id
//! stems) produced zero further hits; their names live in NIS data
//! (`nis_SetFMVFileName`) inside the un-fetched cell archives.

#![forbid(unsafe_code)]

/// One member of a WoI archive (PROVEN name/hash/size/offset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WoiMember {
    /// Which archive file the member ships in (`"00woi"`…`"03woi"`).
    pub archive: &'static str,
    /// The directory `name_hash` (== `prototype_rcf::hash_name(name)`).
    pub name_hash: u32,
    /// The cracked archive path (full path literal, backslash form).
    pub name: &'static str,
    /// Exact payload size in bytes (from the directory entry).
    pub size: u64,
    /// Absolute byte offset of the member inside its archive file
    /// (the surgical-fetch address a Range request needs).
    pub offset: u64,
}

/// Look up a WoI member by its directory hash.
pub fn woi_by_hash(hash: u32) -> Option<&'static WoiMember> {
    WOI_MEMBERS.iter().find(|m| m.name_hash == hash)
}

/// Every Bink movie of the WoI archives (`movies\woi\*.bik`), sorted
/// ascending by hash — the same order as the real directories.
pub const WOI_BIKS: &[WoiMember] = &[
    WoiMember { archive: "00woi", name_hash: 0x0777529C, name: "movies\\woi\\woi_node_001.bik", size: 19053992, offset: 745019392 },
    WoiMember { archive: "00woi", name_hash: 0x0793819E, name: "movies\\woi\\woi_node_003.bik", size: 21445180, offset: 764076032 },
    WoiMember { archive: "00woi", name_hash: 0x07A1991F, name: "movies\\woi\\woi_node_004.bik", size: 24430452, offset: 785524736 },
    WoiMember { archive: "00woi", name_hash: 0x07AFB0A0, name: "movies\\woi\\woi_node_005.bik", size: 37131312, offset: 809957376 },
    WoiMember { archive: "00woi", name_hash: 0x07BDC821, name: "movies\\woi\\woi_node_006.bik", size: 32927196, offset: 847093760 },
    WoiMember { archive: "00woi", name_hash: 0x07CBDFA2, name: "movies\\woi\\woi_node_007.bik", size: 25957848, offset: 880025600 },
    WoiMember { archive: "00woi", name_hash: 0x32824477, name: "movies\\woi\\woi_ec1.bik", size: 84089184, offset: 4096 },
    WoiMember { archive: "00woi", name_hash: 0x32905BF8, name: "movies\\woi\\woi_ec2.bik", size: 94747484, offset: 84099072 },
    WoiMember { archive: "00woi", name_hash: 0x329E7379, name: "movies\\woi\\woi_ec3.bik", size: 78190792, offset: 178851840 },
    WoiMember { archive: "00woi", name_hash: 0x32AC8AFA, name: "movies\\woi\\woi_ec4.bik", size: 73096520, offset: 257048576 },
    WoiMember { archive: "00woi", name_hash: 0x32BAA27B, name: "movies\\woi\\woi_ec5.bik", size: 121363772, offset: 330149888 },
    WoiMember { archive: "00woi", name_hash: 0x32C8B9FC, name: "movies\\woi\\woi_ec6.bik", size: 101759324, offset: 451520512 },
    WoiMember { archive: "00woi", name_hash: 0x32D6D17D, name: "movies\\woi\\woi_ec7.bik", size: 191723244, offset: 553287680 },
    WoiMember { archive: "01woi", name_hash: 0x07D9F723, name: "movies\\woi\\woi_node_008.bik", size: 46498992, offset: 10240 },
    WoiMember { archive: "01woi", name_hash: 0x07E80EA4, name: "movies\\woi\\woi_node_009.bik", size: 26386728, offset: 46514176 },
    WoiMember { archive: "01woi", name_hash: 0x091E13BA, name: "movies\\woi\\woi_node_010.bik", size: 28628952, offset: 72906752 },
    WoiMember { archive: "01woi", name_hash: 0x092C2B3B, name: "movies\\woi\\woi_node_011.bik", size: 14906172, offset: 101539840 },
    WoiMember { archive: "01woi", name_hash: 0x09485A3D, name: "movies\\woi\\woi_node_013.bik", size: 20466932, offset: 116449280 },
    WoiMember { archive: "01woi", name_hash: 0x095671BE, name: "movies\\woi\\woi_node_014.bik", size: 17431800, offset: 136919040 },
    WoiMember { archive: "01woi", name_hash: 0x0964893F, name: "movies\\woi\\woi_node_015.bik", size: 19205392, offset: 154353664 },
    WoiMember { archive: "01woi", name_hash: 0x0972A0C0, name: "movies\\woi\\woi_node_016.bik", size: 29027128, offset: 173561856 },
    WoiMember { archive: "01woi", name_hash: 0x0980B841, name: "movies\\woi\\woi_node_017.bik", size: 19653980, offset: 202594304 },
    WoiMember { archive: "01woi", name_hash: 0x098ECFC2, name: "movies\\woi\\woi_node_018.bik", size: 27552492, offset: 222251008 },
    WoiMember { archive: "01woi", name_hash: 0x099CE743, name: "movies\\woi\\woi_node_019.bik", size: 24480848, offset: 249806848 },
    WoiMember { archive: "01woi", name_hash: 0x0AE103DA, name: "movies\\woi\\woi_node_021.bik", size: 34839352, offset: 274292736 },
    WoiMember { archive: "01woi", name_hash: 0x0AEF1B5B, name: "movies\\woi\\woi_node_022.bik", size: 19127780, offset: 309137408 },
    WoiMember { archive: "01woi", name_hash: 0x0AFD32DC, name: "movies\\woi\\woi_node_023.bik", size: 26546724, offset: 328267776 },
    WoiMember { archive: "01woi", name_hash: 0x0B0B4A5D, name: "movies\\woi\\woi_node_024.bik", size: 15152924, offset: 354820096 },
    WoiMember { archive: "01woi", name_hash: 0x0B1961DE, name: "movies\\woi\\woi_node_025.bik", size: 15045020, offset: 369975296 },
    WoiMember { archive: "01woi", name_hash: 0x0B27795F, name: "movies\\woi\\woi_node_026.bik", size: 29430276, offset: 385024000 },
    WoiMember { archive: "01woi", name_hash: 0x0B3590E0, name: "movies\\woi\\woi_node_027.bik", size: 25140664, offset: 414459904 },
    WoiMember { archive: "01woi", name_hash: 0x0B43A861, name: "movies\\woi\\woi_node_028.bik", size: 22678664, offset: 439603200 },
    WoiMember { archive: "01woi", name_hash: 0x0B51BFE2, name: "movies\\woi\\woi_node_029.bik", size: 30079856, offset: 462284800 },
    WoiMember { archive: "01woi", name_hash: 0x0C87C4F8, name: "movies\\woi\\woi_node_030.bik", size: 17316544, offset: 492369920 },
    WoiMember { archive: "01woi", name_hash: 0x0C95DC79, name: "movies\\woi\\woi_node_031.bik", size: 25682628, offset: 509689856 },
    WoiMember { archive: "01woi", name_hash: 0x0CA3F3FA, name: "movies\\woi\\woi_node_032.bik", size: 33252132, offset: 535377920 },
    WoiMember { archive: "01woi", name_hash: 0x0CB20B7B, name: "movies\\woi\\woi_node_033.bik", size: 26121512, offset: 568635392 },
    WoiMember { archive: "01woi", name_hash: 0x0CC022FC, name: "movies\\woi\\woi_node_034.bik", size: 25098892, offset: 594759680 },
    WoiMember { archive: "01woi", name_hash: 0x0CCE3A7D, name: "movies\\woi\\woi_node_035.bik", size: 40339648, offset: 619864064 },
    WoiMember { archive: "01woi", name_hash: 0x0CDC51FE, name: "movies\\woi\\woi_node_036.bik", size: 24967576, offset: 660209664 },
    WoiMember { archive: "01woi", name_hash: 0x0CEA697F, name: "movies\\woi\\woi_node_037.bik", size: 27085948, offset: 685180928 },
    WoiMember { archive: "01woi", name_hash: 0x0CF88100, name: "movies\\woi\\woi_node_038.bik", size: 23521880, offset: 712271872 },
    WoiMember { archive: "01woi", name_hash: 0x0D069881, name: "movies\\woi\\woi_node_039.bik", size: 17078268, offset: 735797248 },
    WoiMember { archive: "01woi", name_hash: 0x0E3C9D97, name: "movies\\woi\\woi_node_040.bik", size: 13897100, offset: 752877568 },
    WoiMember { archive: "01woi", name_hash: 0x0E4AB518, name: "movies\\woi\\woi_node_041.bik", size: 17701376, offset: 766777344 },
    WoiMember { archive: "01woi", name_hash: 0x0E58CC99, name: "movies\\woi\\woi_node_042.bik", size: 26123204, offset: 784482304 },
    WoiMember { archive: "01woi", name_hash: 0x0E66E41A, name: "movies\\woi\\woi_node_043.bik", size: 27849220, offset: 810610688 },
    WoiMember { archive: "01woi", name_hash: 0x0E74FB9B, name: "movies\\woi\\woi_node_044.bik", size: 16503656, offset: 838465536 },
    WoiMember { archive: "01woi", name_hash: 0x0E83131C, name: "movies\\woi\\woi_node_045.bik", size: 19232088, offset: 854972416 },
    WoiMember { archive: "01woi", name_hash: 0x0E912A9D, name: "movies\\woi\\woi_node_046.bik", size: 16955032, offset: 874207232 },
    WoiMember { archive: "01woi", name_hash: 0x0E9F421E, name: "movies\\woi\\woi_node_047.bik", size: 19871888, offset: 891164672 },
    WoiMember { archive: "01woi", name_hash: 0x0EAD599F, name: "movies\\woi\\woi_node_048.bik", size: 16937804, offset: 911040512 },
    WoiMember { archive: "01woi", name_hash: 0x0EBB7120, name: "movies\\woi\\woi_node_049.bik", size: 16342356, offset: 927981568 },
    WoiMember { archive: "01woi", name_hash: 0x0FF17636, name: "movies\\woi\\woi_node_050.bik", size: 23054384, offset: 944326656 },
    WoiMember { archive: "01woi", name_hash: 0x0FFF8DB7, name: "movies\\woi\\woi_node_051.bik", size: 29437432, offset: 967385088 },
    WoiMember { archive: "02woi", name_hash: 0x101BBCB9, name: "movies\\woi\\woi_node_053.bik", size: 21588252, offset: 10240 },
    WoiMember { archive: "02woi", name_hash: 0x1037EBBB, name: "movies\\woi\\woi_node_055.bik", size: 18121652, offset: 21604352 },
    WoiMember { archive: "02woi", name_hash: 0x10541ABD, name: "movies\\woi\\woi_node_057.bik", size: 17652940, offset: 39729152 },
    WoiMember { archive: "02woi", name_hash: 0x1062323E, name: "movies\\woi\\woi_node_058.bik", size: 25207484, offset: 57384960 },
    WoiMember { archive: "02woi", name_hash: 0x107049BF, name: "movies\\woi\\woi_node_059.bik", size: 20557328, offset: 82597888 },
    WoiMember { archive: "02woi", name_hash: 0x11A64ED5, name: "movies\\woi\\woi_node_060.bik", size: 25851532, offset: 103157760 },
    WoiMember { archive: "02woi", name_hash: 0x11B46656, name: "movies\\woi\\woi_node_061.bik", size: 60003300, offset: 129011712 },
    WoiMember { archive: "02woi", name_hash: 0x11C27DD7, name: "movies\\woi\\woi_node_062.bik", size: 24476916, offset: 189020160 },
    WoiMember { archive: "02woi", name_hash: 0x11D09558, name: "movies\\woi\\woi_node_063.bik", size: 39695576, offset: 213499904 },
    WoiMember { archive: "02woi", name_hash: 0x11DEACD9, name: "movies\\woi\\woi_node_064.bik", size: 30967316, offset: 253200384 },
    WoiMember { archive: "02woi", name_hash: 0x11ECC45A, name: "movies\\woi\\woi_node_065.bik", size: 21989520, offset: 284172288 },
    WoiMember { archive: "02woi", name_hash: 0x11FADBDB, name: "movies\\woi\\woi_node_066.bik", size: 28709208, offset: 306165760 },
    WoiMember { archive: "02woi", name_hash: 0x1208F35C, name: "movies\\woi\\woi_node_067.bik", size: 28812296, offset: 334878720 },
    WoiMember { archive: "02woi", name_hash: 0x12170ADD, name: "movies\\woi\\woi_node_068.bik", size: 24499240, offset: 363694080 },
    WoiMember { archive: "02woi", name_hash: 0x1225225E, name: "movies\\woi\\woi_node_069.bik", size: 28686904, offset: 388196352 },
    WoiMember { archive: "02woi", name_hash: 0x135B2774, name: "movies\\woi\\woi_node_070.bik", size: 30987432, offset: 416886784 },
    WoiMember { archive: "02woi", name_hash: 0x13693EF5, name: "movies\\woi\\woi_node_071.bik", size: 23014048, offset: 447879168 },
    WoiMember { archive: "02woi", name_hash: 0x13775676, name: "movies\\woi\\woi_node_072.bik", size: 33957116, offset: 470896640 },
    WoiMember { archive: "02woi", name_hash: 0x13856DF7, name: "movies\\woi\\woi_node_073.bik", size: 25202240, offset: 504858624 },
    WoiMember { archive: "02woi", name_hash: 0x13938578, name: "movies\\woi\\woi_node_074.bik", size: 27789528, offset: 530063360 },
    WoiMember { archive: "02woi", name_hash: 0x13A19CF9, name: "movies\\woi\\woi_node_075.bik", size: 32849832, offset: 557858816 },
    WoiMember { archive: "02woi", name_hash: 0x13CBE37C, name: "movies\\woi\\woi_node_078.bik", size: 36307880, offset: 590712832 },
    WoiMember { archive: "02woi", name_hash: 0x13D9FAFD, name: "movies\\woi\\woi_node_079.bik", size: 34059240, offset: 627025920 },
    WoiMember { archive: "02woi", name_hash: 0x15100013, name: "movies\\woi\\woi_node_080.bik", size: 38470924, offset: 661090304 },
    WoiMember { archive: "02woi", name_hash: 0x151E1794, name: "movies\\woi\\woi_node_081.bik", size: 26887128, offset: 699566080 },
    WoiMember { archive: "02woi", name_hash: 0x15485E17, name: "movies\\woi\\woi_node_084.bik", size: 16656416, offset: 726458368 },
    WoiMember { archive: "02woi", name_hash: 0x15648D19, name: "movies\\woi\\woi_node_086.bik", size: 32777816, offset: 743118848 },
    WoiMember { archive: "02woi", name_hash: 0x1572A49A, name: "movies\\woi\\woi_node_087.bik", size: 45872524, offset: 775901184 },
    WoiMember { archive: "02woi", name_hash: 0x16D2F033, name: "movies\\woi\\woi_node_091.bik", size: 22181108, offset: 821778432 },
    WoiMember { archive: "02woi", name_hash: 0x16E107B4, name: "movies\\woi\\woi_node_092.bik", size: 20941832, offset: 843962368 },
    WoiMember { archive: "02woi", name_hash: 0x16EF1F35, name: "movies\\woi\\woi_node_093.bik", size: 30746056, offset: 864907264 },
    WoiMember { archive: "02woi", name_hash: 0x16FD36B6, name: "movies\\woi\\woi_node_094.bik", size: 19956052, offset: 895657984 },
    WoiMember { archive: "02woi", name_hash: 0x170B4E37, name: "movies\\woi\\woi_node_095.bik", size: 28042756, offset: 915617792 },
    WoiMember { archive: "02woi", name_hash: 0x173594BA, name: "movies\\woi\\woi_node_098.bik", size: 32928108, offset: 943665152 },
    WoiMember { archive: "02woi", name_hash: 0x1743AC3B, name: "movies\\woi\\woi_node_099.bik", size: 17166992, offset: 976599040 },
    WoiMember { archive: "03woi", name_hash: 0x3C4F765C, name: "movies\\woi\\woi_node_100.bik", size: 18698544, offset: 10240 },
    WoiMember { archive: "03woi", name_hash: 0x3C5D8DDD, name: "movies\\woi\\woi_node_101.bik", size: 11490456, offset: 18712576 },
    WoiMember { archive: "03woi", name_hash: 0x3C6BA55E, name: "movies\\woi\\woi_node_102.bik", size: 21677256, offset: 30205952 },
    WoiMember { archive: "03woi", name_hash: 0x3C79BCDF, name: "movies\\woi\\woi_node_103.bik", size: 21184820, offset: 51886080 },
    WoiMember { archive: "03woi", name_hash: 0x3C87D460, name: "movies\\woi\\woi_node_104.bik", size: 20573708, offset: 73074688 },
    WoiMember { archive: "03woi", name_hash: 0x3C95EBE1, name: "movies\\woi\\woi_node_105.bik", size: 16817408, offset: 93650944 },
    WoiMember { archive: "03woi", name_hash: 0x3CA40362, name: "movies\\woi\\woi_node_106.bik", size: 12748516, offset: 110473216 },
    WoiMember { archive: "03woi", name_hash: 0x3CB21AE3, name: "movies\\woi\\woi_node_107.bik", size: 19733992, offset: 123224064 },
    WoiMember { archive: "03woi", name_hash: 0x3CCE49E5, name: "movies\\woi\\woi_node_109.bik", size: 21718432, offset: 142960640 },
    WoiMember { archive: "03woi", name_hash: 0x3E044EFB, name: "movies\\woi\\woi_node_110.bik", size: 18668752, offset: 164681728 },
    WoiMember { archive: "03woi", name_hash: 0x3E207DFD, name: "movies\\woi\\woi_node_112.bik", size: 32039152, offset: 183353344 },
    WoiMember { archive: "03woi", name_hash: 0x3E3CACFF, name: "movies\\woi\\woi_node_114.bik", size: 24047512, offset: 215396352 },
    WoiMember { archive: "03woi", name_hash: 0x3E66F382, name: "movies\\woi\\woi_node_117.bik", size: 15459748, offset: 239446016 },
    WoiMember { archive: "03woi", name_hash: 0x3E832284, name: "movies\\woi\\woi_node_119.bik", size: 15940640, offset: 254908416 },
    WoiMember { archive: "03woi", name_hash: 0x3FB9279A, name: "movies\\woi\\woi_node_120.bik", size: 16534936, offset: 270852096 },
    WoiMember { archive: "03woi", name_hash: 0x3FD5569C, name: "movies\\woi\\woi_node_122.bik", size: 23602852, offset: 287389696 },
    WoiMember { archive: "03woi", name_hash: 0x3FF1859E, name: "movies\\woi\\woi_node_124.bik", size: 24720520, offset: 310996992 },
    WoiMember { archive: "03woi", name_hash: 0x3FFF9D1F, name: "movies\\woi\\woi_node_125.bik", size: 28634804, offset: 335722496 },
    WoiMember { archive: "03woi", name_hash: 0x400DB4A0, name: "movies\\woi\\woi_node_126.bik", size: 29039956, offset: 364361728 },
    WoiMember { archive: "03woi", name_hash: 0x401BCC21, name: "movies\\woi\\woi_node_127.bik", size: 32133096, offset: 393406464 },
    WoiMember { archive: "03woi", name_hash: 0x4029E3A2, name: "movies\\woi\\woi_node_128.bik", size: 26528440, offset: 425543680 },
    WoiMember { archive: "03woi", name_hash: 0x4037FB23, name: "movies\\woi\\woi_node_129.bik", size: 21514680, offset: 452075520 },
    WoiMember { archive: "03woi", name_hash: 0x44D7B177, name: "movies\\woi\\woi_node_150.bik", size: 13977472, offset: 473593856 },
    WoiMember { archive: "03woi", name_hash: 0x4501F7FA, name: "movies\\woi\\woi_node_153.bik", size: 12662088, offset: 487573504 },
    WoiMember { archive: "03woi", name_hash: 0x451E26FC, name: "movies\\woi\\woi_node_155.bik", size: 17076380, offset: 500238336 },
    WoiMember { archive: "03woi", name_hash: 0x46FD461E, name: "movies\\woi\\woi_node_168.bik", size: 16342556, offset: 517318656 },
    WoiMember { archive: "03woi", name_hash: 0x470B5D9F, name: "movies\\woi\\woi_node_169.bik", size: 23376076, offset: 533663744 },
    WoiMember { archive: "03woi", name_hash: 0x485D91B7, name: "movies\\woi\\woi_node_172.bik", size: 22060468, offset: 557045760 },
    WoiMember { archive: "03woi", name_hash: 0x486BA938, name: "movies\\woi\\woi_node_173.bik", size: 12787284, offset: 579110912 },
    WoiMember { archive: "03woi", name_hash: 0x48A4073C, name: "movies\\woi\\woi_node_177.bik", size: 13066116, offset: 591900672 },
    WoiMember { archive: "03woi", name_hash: 0x4A3CB0D9, name: "movies\\woi\\woi_node_185.bik", size: 18099084, offset: 604968960 },
    WoiMember { archive: "03woi", name_hash: 0x4A58DFDB, name: "movies\\woi\\woi_node_187.bik", size: 13134336, offset: 623071232 },
    WoiMember { archive: "03woi", name_hash: 0x4A750EDD, name: "movies\\woi\\woi_node_189.bik", size: 21834600, offset: 636209152 },
    WoiMember { archive: "03woi", name_hash: 0x4BAB13F3, name: "movies\\woi\\woi_node_190.bik", size: 38816404, offset: 658049024 },
    WoiMember { archive: "03woi", name_hash: 0x4BB92B74, name: "movies\\woi\\woi_node_191.bik", size: 29421500, offset: 696870912 },
    WoiMember { archive: "03woi", name_hash: 0x7143C91E, name: "movies\\woi\\woi_node_201.bik", size: 14521116, offset: 726292480 },
    WoiMember { archive: "03woi", name_hash: 0x71985624, name: "movies\\woi\\woi_node_207.bik", size: 29943492, offset: 740816896 },
    WoiMember { archive: "03woi", name_hash: 0x71A66DA5, name: "movies\\woi\\woi_node_208.bik", size: 34317424, offset: 770764800 },
    WoiMember { archive: "03woi", name_hash: 0x71B48526, name: "movies\\woi\\woi_node_209.bik", size: 19038624, offset: 805087232 },
    WoiMember { archive: "03woi", name_hash: 0x72F8A1BD, name: "movies\\woi\\woi_node_211.bik", size: 25581328, offset: 824129536 },
    WoiMember { archive: "03woi", name_hash: 0x7306B93E, name: "movies\\woi\\woi_node_212.bik", size: 32248004, offset: 849715200 },
    WoiMember { archive: "03woi", name_hash: 0x7314D0BF, name: "movies\\woi\\woi_node_213.bik", size: 34786132, offset: 881969152 },
    WoiMember { archive: "03woi", name_hash: 0x7322E840, name: "movies\\woi\\woi_node_214.bik", size: 22190404, offset: 916756480 },
    WoiMember { archive: "03woi", name_hash: 0x733F1742, name: "movies\\woi\\woi_node_216.bik", size: 35832164, offset: 938952704 },
    WoiMember { archive: "03woi", name_hash: 0x734D2EC3, name: "movies\\woi\\woi_node_217.bik", size: 23017828, offset: 974790656 },
    WoiMember { archive: "03woi", name_hash: 0x73695DC5, name: "movies\\woi\\woi_node_219.bik", size: 18251240, offset: 997812224 },
    WoiMember { archive: "03woi", name_hash: 0x749F62DB, name: "movies\\woi\\woi_node_220.bik", size: 19510436, offset: 1016068096 },
    WoiMember { archive: "03woi", name_hash: 0x74AD7A5C, name: "movies\\woi\\woi_node_221.bik", size: 13574276, offset: 1035581440 },
];

/// Every subtitle text-bible P3D of the WoI archives
/// (`movies\woi\*.p3d`), sorted ascending by hash.
pub const WOI_BIBLES: &[WoiMember] = &[
    WoiMember { archive: "00woi", name_hash: 0x07778479, name: "movies\\woi\\woi_node_001.p3d", size: 1469, offset: 764073984 },
    WoiMember { archive: "00woi", name_hash: 0x0793B37B, name: "movies\\woi\\woi_node_003.p3d", size: 1587, offset: 785522688 },
    WoiMember { archive: "00woi", name_hash: 0x07A1CAFC, name: "movies\\woi\\woi_node_004.p3d", size: 1982, offset: 809955328 },
    WoiMember { archive: "00woi", name_hash: 0x07AFE27D, name: "movies\\woi\\woi_node_005.p3d", size: 3120, offset: 847089664 },
    WoiMember { archive: "00woi", name_hash: 0x07BDF9FE, name: "movies\\woi\\woi_node_006.p3d", size: 2264, offset: 880021504 },
    WoiMember { archive: "00woi", name_hash: 0x32827654, name: "movies\\woi\\woi_ec1.p3d", size: 3805, offset: 84094976 },
    WoiMember { archive: "00woi", name_hash: 0x32908DD5, name: "movies\\woi\\woi_ec2.p3d", size: 3948, offset: 178847744 },
    WoiMember { archive: "00woi", name_hash: 0x329EA556, name: "movies\\woi\\woi_ec3.p3d", size: 3826, offset: 257044480 },
    WoiMember { archive: "00woi", name_hash: 0x32ACBCD7, name: "movies\\woi\\woi_ec4.p3d", size: 3806, offset: 330145792 },
    WoiMember { archive: "00woi", name_hash: 0x32BAD458, name: "movies\\woi\\woi_ec5.p3d", size: 5302, offset: 451514368 },
    WoiMember { archive: "00woi", name_hash: 0x32C8EBD9, name: "movies\\woi\\woi_ec6.p3d", size: 4521, offset: 553281536 },
    WoiMember { archive: "00woi", name_hash: 0x32D7035A, name: "movies\\woi\\woi_ec7.p3d", size: 8056, offset: 745011200 },
    WoiMember { archive: "01woi", name_hash: 0x07CC117F, name: "movies\\woi\\woi_node_007.p3d", size: 2396, offset: 6144 },
    WoiMember { archive: "01woi", name_hash: 0x07DA2900, name: "movies\\woi\\woi_node_008.p3d", size: 3714, offset: 46510080 },
    WoiMember { archive: "01woi", name_hash: 0x07E84081, name: "movies\\woi\\woi_node_009.p3d", size: 2214, offset: 72902656 },
    WoiMember { archive: "01woi", name_hash: 0x091E4597, name: "movies\\woi\\woi_node_010.p3d", size: 2522, offset: 101535744 },
    WoiMember { archive: "01woi", name_hash: 0x092C5D18, name: "movies\\woi\\woi_node_011.p3d", size: 1677, offset: 116447232 },
    WoiMember { archive: "01woi", name_hash: 0x09488C1A, name: "movies\\woi\\woi_node_013.p3d", size: 1755, offset: 136916992 },
    WoiMember { archive: "01woi", name_hash: 0x0956A39B, name: "movies\\woi\\woi_node_014.p3d", size: 1533, offset: 154351616 },
    WoiMember { archive: "01woi", name_hash: 0x0964BB1C, name: "movies\\woi\\woi_node_015.p3d", size: 1872, offset: 173559808 },
    WoiMember { archive: "01woi", name_hash: 0x0972D29D, name: "movies\\woi\\woi_node_016.p3d", size: 2309, offset: 202590208 },
    WoiMember { archive: "01woi", name_hash: 0x0980EA1E, name: "movies\\woi\\woi_node_017.p3d", size: 2020, offset: 222248960 },
    WoiMember { archive: "01woi", name_hash: 0x098F019F, name: "movies\\woi\\woi_node_018.p3d", size: 1900, offset: 249804800 },
    WoiMember { archive: "01woi", name_hash: 0x099D1920, name: "movies\\woi\\woi_node_019.p3d", size: 2050, offset: 274288640 },
    WoiMember { archive: "01woi", name_hash: 0x0AE135B7, name: "movies\\woi\\woi_node_021.p3d", size: 2908, offset: 309133312 },
    WoiMember { archive: "01woi", name_hash: 0x0AEF4D38, name: "movies\\woi\\woi_node_022.p3d", size: 1762, offset: 328265728 },
    WoiMember { archive: "01woi", name_hash: 0x0AFD64B9, name: "movies\\woi\\woi_node_023.p3d", size: 2374, offset: 354816000 },
    WoiMember { archive: "01woi", name_hash: 0x0B0B7C3A, name: "movies\\woi\\woi_node_024.p3d", size: 1511, offset: 369973248 },
    WoiMember { archive: "01woi", name_hash: 0x0B1993BB, name: "movies\\woi\\woi_node_025.p3d", size: 1686, offset: 385021952 },
    WoiMember { archive: "01woi", name_hash: 0x0B27AB3C, name: "movies\\woi\\woi_node_026.p3d", size: 2306, offset: 414455808 },
    WoiMember { archive: "01woi", name_hash: 0x0B35C2BD, name: "movies\\woi\\woi_node_027.p3d", size: 2004, offset: 439601152 },
    WoiMember { archive: "01woi", name_hash: 0x0B43DA3E, name: "movies\\woi\\woi_node_028.p3d", size: 1745, offset: 462282752 },
    WoiMember { archive: "01woi", name_hash: 0x0B51F1BF, name: "movies\\woi\\woi_node_029.p3d", size: 2345, offset: 492365824 },
    WoiMember { archive: "01woi", name_hash: 0x0C87F6D5, name: "movies\\woi\\woi_node_030.p3d", size: 1688, offset: 509687808 },
    WoiMember { archive: "01woi", name_hash: 0x0C960E56, name: "movies\\woi\\woi_node_031.p3d", size: 2188, offset: 535373824 },
    WoiMember { archive: "01woi", name_hash: 0x0CA425D7, name: "movies\\woi\\woi_node_032.p3d", size: 2682, offset: 568631296 },
    WoiMember { archive: "01woi", name_hash: 0x0CB23D58, name: "movies\\woi\\woi_node_033.p3d", size: 1977, offset: 594757632 },
    WoiMember { archive: "01woi", name_hash: 0x0CC054D9, name: "movies\\woi\\woi_node_034.p3d", size: 2124, offset: 619859968 },
    WoiMember { archive: "01woi", name_hash: 0x0CCE6C5A, name: "movies\\woi\\woi_node_035.p3d", size: 2578, offset: 660205568 },
    WoiMember { archive: "01woi", name_hash: 0x0CDC83DB, name: "movies\\woi\\woi_node_036.p3d", size: 1945, offset: 685178880 },
    WoiMember { archive: "01woi", name_hash: 0x0CEA9B5C, name: "movies\\woi\\woi_node_037.p3d", size: 2386, offset: 712267776 },
    WoiMember { archive: "01woi", name_hash: 0x0CF8B2DD, name: "movies\\woi\\woi_node_038.p3d", size: 1546, offset: 735795200 },
    WoiMember { archive: "01woi", name_hash: 0x0D06CA5E, name: "movies\\woi\\woi_node_039.p3d", size: 1502, offset: 752875520 },
    WoiMember { archive: "01woi", name_hash: 0x0E3CCF74, name: "movies\\woi\\woi_node_040.p3d", size: 1571, offset: 766775296 },
    WoiMember { archive: "01woi", name_hash: 0x0E4AE6F5, name: "movies\\woi\\woi_node_041.p3d", size: 1528, offset: 784480256 },
    WoiMember { archive: "01woi", name_hash: 0x0E58FE76, name: "movies\\woi\\woi_node_042.p3d", size: 2267, offset: 810606592 },
    WoiMember { archive: "01woi", name_hash: 0x0E6715F7, name: "movies\\woi\\woi_node_043.p3d", size: 2429, offset: 838461440 },
    WoiMember { archive: "01woi", name_hash: 0x0E752D78, name: "movies\\woi\\woi_node_044.p3d", size: 1613, offset: 854970368 },
    WoiMember { archive: "01woi", name_hash: 0x0E8344F9, name: "movies\\woi\\woi_node_045.p3d", size: 1612, offset: 874205184 },
    WoiMember { archive: "01woi", name_hash: 0x0E915C7A, name: "movies\\woi\\woi_node_046.p3d", size: 1828, offset: 891162624 },
    WoiMember { archive: "01woi", name_hash: 0x0E9F73FB, name: "movies\\woi\\woi_node_047.p3d", size: 1852, offset: 911038464 },
    WoiMember { archive: "01woi", name_hash: 0x0EAD8B7C, name: "movies\\woi\\woi_node_048.p3d", size: 1514, offset: 927979520 },
    WoiMember { archive: "01woi", name_hash: 0x0EBBA2FD, name: "movies\\woi\\woi_node_049.p3d", size: 1886, offset: 944324608 },
    WoiMember { archive: "01woi", name_hash: 0x0FF1A813, name: "movies\\woi\\woi_node_050.p3d", size: 2005, offset: 967383040 },
    WoiMember { archive: "02woi", name_hash: 0x0FFFBF94, name: "movies\\woi\\woi_node_051.p3d", size: 2597, offset: 6144 },
    WoiMember { archive: "02woi", name_hash: 0x101BEE96, name: "movies\\woi\\woi_node_053.p3d", size: 2081, offset: 21600256 },
    WoiMember { archive: "02woi", name_hash: 0x10381D98, name: "movies\\woi\\woi_node_055.p3d", size: 1488, offset: 39727104 },
    WoiMember { archive: "02woi", name_hash: 0x10544C9A, name: "movies\\woi\\woi_node_057.p3d", size: 1375, offset: 57382912 },
    WoiMember { archive: "02woi", name_hash: 0x1062641B, name: "movies\\woi\\woi_node_058.p3d", size: 2277, offset: 82593792 },
    WoiMember { archive: "02woi", name_hash: 0x10707B9C, name: "movies\\woi\\woi_node_059.p3d", size: 1679, offset: 103155712 },
    WoiMember { archive: "02woi", name_hash: 0x11A680B2, name: "movies\\woi\\woi_node_060.p3d", size: 1528, offset: 129009664 },
    WoiMember { archive: "02woi", name_hash: 0x11B49833, name: "movies\\woi\\woi_node_061.p3d", size: 3649, offset: 189016064 },
    WoiMember { archive: "02woi", name_hash: 0x11C2AFB4, name: "movies\\woi\\woi_node_062.p3d", size: 2017, offset: 213497856 },
    WoiMember { archive: "02woi", name_hash: 0x11D0C735, name: "movies\\woi\\woi_node_063.p3d", size: 3446, offset: 253196288 },
    WoiMember { archive: "02woi", name_hash: 0x11DEDEB6, name: "movies\\woi\\woi_node_064.p3d", size: 2247, offset: 284168192 },
    WoiMember { archive: "02woi", name_hash: 0x11ECF637, name: "movies\\woi\\woi_node_065.p3d", size: 1983, offset: 306163712 },
    WoiMember { archive: "02woi", name_hash: 0x11FB0DB8, name: "movies\\woi\\woi_node_066.p3d", size: 1581, offset: 334876672 },
    WoiMember { archive: "02woi", name_hash: 0x12092539, name: "movies\\woi\\woi_node_067.p3d", size: 1771, offset: 363692032 },
    WoiMember { archive: "02woi", name_hash: 0x12173CBA, name: "movies\\woi\\woi_node_068.p3d", size: 1947, offset: 388194304 },
    WoiMember { archive: "02woi", name_hash: 0x1225543B, name: "movies\\woi\\woi_node_069.p3d", size: 1825, offset: 416884736 },
    WoiMember { archive: "02woi", name_hash: 0x135B5951, name: "movies\\woi\\woi_node_070.p3d", size: 2245, offset: 447875072 },
    WoiMember { archive: "02woi", name_hash: 0x136970D2, name: "movies\\woi\\woi_node_071.p3d", size: 1965, offset: 470894592 },
    WoiMember { archive: "02woi", name_hash: 0x13778853, name: "movies\\woi\\woi_node_072.p3d", size: 2102, offset: 504854528 },
    WoiMember { archive: "02woi", name_hash: 0x13859FD4, name: "movies\\woi\\woi_node_073.p3d", size: 1873, offset: 530061312 },
    WoiMember { archive: "02woi", name_hash: 0x1393B755, name: "movies\\woi\\woi_node_074.p3d", size: 2491, offset: 557854720 },
    WoiMember { archive: "02woi", name_hash: 0x13A1CED6, name: "movies\\woi\\woi_node_075.p3d", size: 2170, offset: 590708736 },
    WoiMember { archive: "02woi", name_hash: 0x13CC1559, name: "movies\\woi\\woi_node_078.p3d", size: 2395, offset: 627021824 },
    WoiMember { archive: "02woi", name_hash: 0x13DA2CDA, name: "movies\\woi\\woi_node_079.p3d", size: 2445, offset: 661086208 },
    WoiMember { archive: "02woi", name_hash: 0x151031F0, name: "movies\\woi\\woi_node_080.p3d", size: 3093, offset: 699561984 },
    WoiMember { archive: "02woi", name_hash: 0x151E4971, name: "movies\\woi\\woi_node_081.p3d", size: 2083, offset: 726454272 },
    WoiMember { archive: "02woi", name_hash: 0x15488FF4, name: "movies\\woi\\woi_node_084.p3d", size: 1702, offset: 743116800 },
    WoiMember { archive: "02woi", name_hash: 0x1564BEF6, name: "movies\\woi\\woi_node_086.p3d", size: 2240, offset: 775897088 },
    WoiMember { archive: "02woi", name_hash: 0x1572D677, name: "movies\\woi\\woi_node_087.p3d", size: 2936, offset: 821774336 },
    WoiMember { archive: "02woi", name_hash: 0x16D32210, name: "movies\\woi\\woi_node_091.p3d", size: 1827, offset: 843960320 },
    WoiMember { archive: "02woi", name_hash: 0x16E13991, name: "movies\\woi\\woi_node_092.p3d", size: 1768, offset: 864905216 },
    WoiMember { archive: "02woi", name_hash: 0x16EF5112, name: "movies\\woi\\woi_node_093.p3d", size: 2237, offset: 895653888 },
    WoiMember { archive: "02woi", name_hash: 0x16FD6893, name: "movies\\woi\\woi_node_094.p3d", size: 1912, offset: 915615744 },
    WoiMember { archive: "02woi", name_hash: 0x170B8014, name: "movies\\woi\\woi_node_095.p3d", size: 2136, offset: 943661056 },
    WoiMember { archive: "02woi", name_hash: 0x1735C697, name: "movies\\woi\\woi_node_098.p3d", size: 2427, offset: 976594944 },
    WoiMember { archive: "03woi", name_hash: 0x1743DE18, name: "movies\\woi\\woi_node_099.p3d", size: 1650, offset: 8192 },
    WoiMember { archive: "03woi", name_hash: 0x3C4FA839, name: "movies\\woi\\woi_node_100.p3d", size: 1613, offset: 18710528 },
    WoiMember { archive: "03woi", name_hash: 0x3C5DBFBA, name: "movies\\woi\\woi_node_101.p3d", size: 1192, offset: 30203904 },
    WoiMember { archive: "03woi", name_hash: 0x3C6BD73B, name: "movies\\woi\\woi_node_102.p3d", size: 1803, offset: 51884032 },
    WoiMember { archive: "03woi", name_hash: 0x3C79EEBC, name: "movies\\woi\\woi_node_103.p3d", size: 1896, offset: 73072640 },
    WoiMember { archive: "03woi", name_hash: 0x3C88063D, name: "movies\\woi\\woi_node_104.p3d", size: 1391, offset: 93648896 },
    WoiMember { archive: "03woi", name_hash: 0x3C961DBE, name: "movies\\woi\\woi_node_105.p3d", size: 2061, offset: 110469120 },
    WoiMember { archive: "03woi", name_hash: 0x3CA4353F, name: "movies\\woi\\woi_node_106.p3d", size: 1303, offset: 123222016 },
    WoiMember { archive: "03woi", name_hash: 0x3CB24CC0, name: "movies\\woi\\woi_node_107.p3d", size: 2002, offset: 142958592 },
    WoiMember { archive: "03woi", name_hash: 0x3CCE7BC2, name: "movies\\woi\\woi_node_109.p3d", size: 1985, offset: 164679680 },
    WoiMember { archive: "03woi", name_hash: 0x3E0480D8, name: "movies\\woi\\woi_node_110.p3d", size: 1720, offset: 183351296 },
    WoiMember { archive: "03woi", name_hash: 0x3E20AFDA, name: "movies\\woi\\woi_node_112.p3d", size: 2011, offset: 215394304 },
    WoiMember { archive: "03woi", name_hash: 0x3E3CDEDC, name: "movies\\woi\\woi_node_114.p3d", size: 1963, offset: 239443968 },
    WoiMember { archive: "03woi", name_hash: 0x3E67255F, name: "movies\\woi\\woi_node_117.p3d", size: 1677, offset: 254906368 },
    WoiMember { archive: "03woi", name_hash: 0x3E835461, name: "movies\\woi\\woi_node_119.p3d", size: 1330, offset: 270850048 },
    WoiMember { archive: "03woi", name_hash: 0x3FB95977, name: "movies\\woi\\woi_node_120.p3d", size: 1457, offset: 287387648 },
    WoiMember { archive: "03woi", name_hash: 0x3FD58879, name: "movies\\woi\\woi_node_122.p3d", size: 2080, offset: 310992896 },
    WoiMember { archive: "03woi", name_hash: 0x3FF1B77B, name: "movies\\woi\\woi_node_124.p3d", size: 2309, offset: 335718400 },
    WoiMember { archive: "03woi", name_hash: 0x3FFFCEFC, name: "movies\\woi\\woi_node_125.p3d", size: 2054, offset: 364357632 },
    WoiMember { archive: "03woi", name_hash: 0x400DE67D, name: "movies\\woi\\woi_node_126.p3d", size: 2501, offset: 393402368 },
    WoiMember { archive: "03woi", name_hash: 0x401BFDFE, name: "movies\\woi\\woi_node_127.p3d", size: 2710, offset: 425539584 },
    WoiMember { archive: "03woi", name_hash: 0x402A157F, name: "movies\\woi\\woi_node_128.p3d", size: 1947, offset: 452073472 },
    WoiMember { archive: "03woi", name_hash: 0x40382D00, name: "movies\\woi\\woi_node_129.p3d", size: 1850, offset: 473591808 },
    WoiMember { archive: "03woi", name_hash: 0x44D7E354, name: "movies\\woi\\woi_node_150.p3d", size: 1415, offset: 487571456 },
    WoiMember { archive: "03woi", name_hash: 0x450229D7, name: "movies\\woi\\woi_node_153.p3d", size: 1342, offset: 500236288 },
    WoiMember { archive: "03woi", name_hash: 0x451E58D9, name: "movies\\woi\\woi_node_155.p3d", size: 1580, offset: 517316608 },
    WoiMember { archive: "03woi", name_hash: 0x46FD77FB, name: "movies\\woi\\woi_node_168.p3d", size: 1511, offset: 533661696 },
    WoiMember { archive: "03woi", name_hash: 0x470B8F7C, name: "movies\\woi\\woi_node_169.p3d", size: 2175, offset: 557041664 },
    WoiMember { archive: "03woi", name_hash: 0x485DC394, name: "movies\\woi\\woi_node_172.p3d", size: 2326, offset: 579106816 },
    WoiMember { archive: "03woi", name_hash: 0x486BDB15, name: "movies\\woi\\woi_node_173.p3d", size: 1409, offset: 591898624 },
    WoiMember { archive: "03woi", name_hash: 0x48A43919, name: "movies\\woi\\woi_node_177.p3d", size: 1377, offset: 604966912 },
    WoiMember { archive: "03woi", name_hash: 0x4A3CE2B6, name: "movies\\woi\\woi_node_185.p3d", size: 1857, offset: 623069184 },
    WoiMember { archive: "03woi", name_hash: 0x4A5911B8, name: "movies\\woi\\woi_node_187.p3d", size: 1518, offset: 636207104 },
    WoiMember { archive: "03woi", name_hash: 0x4A7540BA, name: "movies\\woi\\woi_node_189.p3d", size: 2181, offset: 658044928 },
    WoiMember { archive: "03woi", name_hash: 0x4BAB45D0, name: "movies\\woi\\woi_node_190.p3d", size: 2632, offset: 696866816 },
    WoiMember { archive: "03woi", name_hash: 0x7143FAFB, name: "movies\\woi\\woi_node_201.p3d", size: 1341, offset: 740814848 },
    WoiMember { archive: "03woi", name_hash: 0x71988801, name: "movies\\woi\\woi_node_207.p3d", size: 2435, offset: 770760704 },
    WoiMember { archive: "03woi", name_hash: 0x71A69F82, name: "movies\\woi\\woi_node_208.p3d", size: 2851, offset: 805083136 },
    WoiMember { archive: "03woi", name_hash: 0x71B4B703, name: "movies\\woi\\woi_node_209.p3d", size: 1663, offset: 824127488 },
    WoiMember { archive: "03woi", name_hash: 0x72F8D39A, name: "movies\\woi\\woi_node_211.p3d", size: 2442, offset: 849711104 },
    WoiMember { archive: "03woi", name_hash: 0x7306EB1B, name: "movies\\woi\\woi_node_212.p3d", size: 2879, offset: 881965056 },
    WoiMember { archive: "03woi", name_hash: 0x73231A1D, name: "movies\\woi\\woi_node_214.p3d", size: 143, offset: 938948608 },
    WoiMember { archive: "03woi", name_hash: 0x7331319E, name: "movies\\woi\\woi_node_215.p3d", size: 1245, offset: 938950656 },
    WoiMember { archive: "03woi", name_hash: 0x733F491F, name: "movies\\woi\\woi_node_216.p3d", size: 2749, offset: 974786560 },
    WoiMember { archive: "03woi", name_hash: 0x734D60A0, name: "movies\\woi\\woi_node_217.p3d", size: 1860, offset: 997810176 },
    WoiMember { archive: "03woi", name_hash: 0x73698FA2, name: "movies\\woi\\woi_node_219.p3d", size: 2171, offset: 1016064000 },
    WoiMember { archive: "03woi", name_hash: 0x749F94B8, name: "movies\\woi\\woi_node_220.p3d", size: 1783, offset: 1035579392 },
    WoiMember { archive: "03woi", name_hash: 0x74ADAC39, name: "movies\\woi\\woi_node_221.p3d", size: 1320, offset: 1049157632 },
];

/// Both tables concatenated (the complete 275-member census).
pub const WOI_MEMBERS: &[WoiMember] = &[
    WoiMember { archive: "00woi", name_hash: 0x0777529C, name: "movies\\woi\\woi_node_001.bik", size: 19053992, offset: 745019392 },
    WoiMember { archive: "00woi", name_hash: 0x07778479, name: "movies\\woi\\woi_node_001.p3d", size: 1469, offset: 764073984 },
    WoiMember { archive: "00woi", name_hash: 0x0793819E, name: "movies\\woi\\woi_node_003.bik", size: 21445180, offset: 764076032 },
    WoiMember { archive: "00woi", name_hash: 0x0793B37B, name: "movies\\woi\\woi_node_003.p3d", size: 1587, offset: 785522688 },
    WoiMember { archive: "00woi", name_hash: 0x07A1991F, name: "movies\\woi\\woi_node_004.bik", size: 24430452, offset: 785524736 },
    WoiMember { archive: "00woi", name_hash: 0x07A1CAFC, name: "movies\\woi\\woi_node_004.p3d", size: 1982, offset: 809955328 },
    WoiMember { archive: "00woi", name_hash: 0x07AFB0A0, name: "movies\\woi\\woi_node_005.bik", size: 37131312, offset: 809957376 },
    WoiMember { archive: "00woi", name_hash: 0x07AFE27D, name: "movies\\woi\\woi_node_005.p3d", size: 3120, offset: 847089664 },
    WoiMember { archive: "00woi", name_hash: 0x07BDC821, name: "movies\\woi\\woi_node_006.bik", size: 32927196, offset: 847093760 },
    WoiMember { archive: "00woi", name_hash: 0x07BDF9FE, name: "movies\\woi\\woi_node_006.p3d", size: 2264, offset: 880021504 },
    WoiMember { archive: "00woi", name_hash: 0x07CBDFA2, name: "movies\\woi\\woi_node_007.bik", size: 25957848, offset: 880025600 },
    WoiMember { archive: "00woi", name_hash: 0x32824477, name: "movies\\woi\\woi_ec1.bik", size: 84089184, offset: 4096 },
    WoiMember { archive: "00woi", name_hash: 0x32827654, name: "movies\\woi\\woi_ec1.p3d", size: 3805, offset: 84094976 },
    WoiMember { archive: "00woi", name_hash: 0x32905BF8, name: "movies\\woi\\woi_ec2.bik", size: 94747484, offset: 84099072 },
    WoiMember { archive: "00woi", name_hash: 0x32908DD5, name: "movies\\woi\\woi_ec2.p3d", size: 3948, offset: 178847744 },
    WoiMember { archive: "00woi", name_hash: 0x329E7379, name: "movies\\woi\\woi_ec3.bik", size: 78190792, offset: 178851840 },
    WoiMember { archive: "00woi", name_hash: 0x329EA556, name: "movies\\woi\\woi_ec3.p3d", size: 3826, offset: 257044480 },
    WoiMember { archive: "00woi", name_hash: 0x32AC8AFA, name: "movies\\woi\\woi_ec4.bik", size: 73096520, offset: 257048576 },
    WoiMember { archive: "00woi", name_hash: 0x32ACBCD7, name: "movies\\woi\\woi_ec4.p3d", size: 3806, offset: 330145792 },
    WoiMember { archive: "00woi", name_hash: 0x32BAA27B, name: "movies\\woi\\woi_ec5.bik", size: 121363772, offset: 330149888 },
    WoiMember { archive: "00woi", name_hash: 0x32BAD458, name: "movies\\woi\\woi_ec5.p3d", size: 5302, offset: 451514368 },
    WoiMember { archive: "00woi", name_hash: 0x32C8B9FC, name: "movies\\woi\\woi_ec6.bik", size: 101759324, offset: 451520512 },
    WoiMember { archive: "00woi", name_hash: 0x32C8EBD9, name: "movies\\woi\\woi_ec6.p3d", size: 4521, offset: 553281536 },
    WoiMember { archive: "00woi", name_hash: 0x32D6D17D, name: "movies\\woi\\woi_ec7.bik", size: 191723244, offset: 553287680 },
    WoiMember { archive: "00woi", name_hash: 0x32D7035A, name: "movies\\woi\\woi_ec7.p3d", size: 8056, offset: 745011200 },
    WoiMember { archive: "01woi", name_hash: 0x07CC117F, name: "movies\\woi\\woi_node_007.p3d", size: 2396, offset: 6144 },
    WoiMember { archive: "01woi", name_hash: 0x07D9F723, name: "movies\\woi\\woi_node_008.bik", size: 46498992, offset: 10240 },
    WoiMember { archive: "01woi", name_hash: 0x07DA2900, name: "movies\\woi\\woi_node_008.p3d", size: 3714, offset: 46510080 },
    WoiMember { archive: "01woi", name_hash: 0x07E80EA4, name: "movies\\woi\\woi_node_009.bik", size: 26386728, offset: 46514176 },
    WoiMember { archive: "01woi", name_hash: 0x07E84081, name: "movies\\woi\\woi_node_009.p3d", size: 2214, offset: 72902656 },
    WoiMember { archive: "01woi", name_hash: 0x091E13BA, name: "movies\\woi\\woi_node_010.bik", size: 28628952, offset: 72906752 },
    WoiMember { archive: "01woi", name_hash: 0x091E4597, name: "movies\\woi\\woi_node_010.p3d", size: 2522, offset: 101535744 },
    WoiMember { archive: "01woi", name_hash: 0x092C2B3B, name: "movies\\woi\\woi_node_011.bik", size: 14906172, offset: 101539840 },
    WoiMember { archive: "01woi", name_hash: 0x092C5D18, name: "movies\\woi\\woi_node_011.p3d", size: 1677, offset: 116447232 },
    WoiMember { archive: "01woi", name_hash: 0x09485A3D, name: "movies\\woi\\woi_node_013.bik", size: 20466932, offset: 116449280 },
    WoiMember { archive: "01woi", name_hash: 0x09488C1A, name: "movies\\woi\\woi_node_013.p3d", size: 1755, offset: 136916992 },
    WoiMember { archive: "01woi", name_hash: 0x095671BE, name: "movies\\woi\\woi_node_014.bik", size: 17431800, offset: 136919040 },
    WoiMember { archive: "01woi", name_hash: 0x0956A39B, name: "movies\\woi\\woi_node_014.p3d", size: 1533, offset: 154351616 },
    WoiMember { archive: "01woi", name_hash: 0x0964893F, name: "movies\\woi\\woi_node_015.bik", size: 19205392, offset: 154353664 },
    WoiMember { archive: "01woi", name_hash: 0x0964BB1C, name: "movies\\woi\\woi_node_015.p3d", size: 1872, offset: 173559808 },
    WoiMember { archive: "01woi", name_hash: 0x0972A0C0, name: "movies\\woi\\woi_node_016.bik", size: 29027128, offset: 173561856 },
    WoiMember { archive: "01woi", name_hash: 0x0972D29D, name: "movies\\woi\\woi_node_016.p3d", size: 2309, offset: 202590208 },
    WoiMember { archive: "01woi", name_hash: 0x0980B841, name: "movies\\woi\\woi_node_017.bik", size: 19653980, offset: 202594304 },
    WoiMember { archive: "01woi", name_hash: 0x0980EA1E, name: "movies\\woi\\woi_node_017.p3d", size: 2020, offset: 222248960 },
    WoiMember { archive: "01woi", name_hash: 0x098ECFC2, name: "movies\\woi\\woi_node_018.bik", size: 27552492, offset: 222251008 },
    WoiMember { archive: "01woi", name_hash: 0x098F019F, name: "movies\\woi\\woi_node_018.p3d", size: 1900, offset: 249804800 },
    WoiMember { archive: "01woi", name_hash: 0x099CE743, name: "movies\\woi\\woi_node_019.bik", size: 24480848, offset: 249806848 },
    WoiMember { archive: "01woi", name_hash: 0x099D1920, name: "movies\\woi\\woi_node_019.p3d", size: 2050, offset: 274288640 },
    WoiMember { archive: "01woi", name_hash: 0x0AE103DA, name: "movies\\woi\\woi_node_021.bik", size: 34839352, offset: 274292736 },
    WoiMember { archive: "01woi", name_hash: 0x0AE135B7, name: "movies\\woi\\woi_node_021.p3d", size: 2908, offset: 309133312 },
    WoiMember { archive: "01woi", name_hash: 0x0AEF1B5B, name: "movies\\woi\\woi_node_022.bik", size: 19127780, offset: 309137408 },
    WoiMember { archive: "01woi", name_hash: 0x0AEF4D38, name: "movies\\woi\\woi_node_022.p3d", size: 1762, offset: 328265728 },
    WoiMember { archive: "01woi", name_hash: 0x0AFD32DC, name: "movies\\woi\\woi_node_023.bik", size: 26546724, offset: 328267776 },
    WoiMember { archive: "01woi", name_hash: 0x0AFD64B9, name: "movies\\woi\\woi_node_023.p3d", size: 2374, offset: 354816000 },
    WoiMember { archive: "01woi", name_hash: 0x0B0B4A5D, name: "movies\\woi\\woi_node_024.bik", size: 15152924, offset: 354820096 },
    WoiMember { archive: "01woi", name_hash: 0x0B0B7C3A, name: "movies\\woi\\woi_node_024.p3d", size: 1511, offset: 369973248 },
    WoiMember { archive: "01woi", name_hash: 0x0B1961DE, name: "movies\\woi\\woi_node_025.bik", size: 15045020, offset: 369975296 },
    WoiMember { archive: "01woi", name_hash: 0x0B1993BB, name: "movies\\woi\\woi_node_025.p3d", size: 1686, offset: 385021952 },
    WoiMember { archive: "01woi", name_hash: 0x0B27795F, name: "movies\\woi\\woi_node_026.bik", size: 29430276, offset: 385024000 },
    WoiMember { archive: "01woi", name_hash: 0x0B27AB3C, name: "movies\\woi\\woi_node_026.p3d", size: 2306, offset: 414455808 },
    WoiMember { archive: "01woi", name_hash: 0x0B3590E0, name: "movies\\woi\\woi_node_027.bik", size: 25140664, offset: 414459904 },
    WoiMember { archive: "01woi", name_hash: 0x0B35C2BD, name: "movies\\woi\\woi_node_027.p3d", size: 2004, offset: 439601152 },
    WoiMember { archive: "01woi", name_hash: 0x0B43A861, name: "movies\\woi\\woi_node_028.bik", size: 22678664, offset: 439603200 },
    WoiMember { archive: "01woi", name_hash: 0x0B43DA3E, name: "movies\\woi\\woi_node_028.p3d", size: 1745, offset: 462282752 },
    WoiMember { archive: "01woi", name_hash: 0x0B51BFE2, name: "movies\\woi\\woi_node_029.bik", size: 30079856, offset: 462284800 },
    WoiMember { archive: "01woi", name_hash: 0x0B51F1BF, name: "movies\\woi\\woi_node_029.p3d", size: 2345, offset: 492365824 },
    WoiMember { archive: "01woi", name_hash: 0x0C87C4F8, name: "movies\\woi\\woi_node_030.bik", size: 17316544, offset: 492369920 },
    WoiMember { archive: "01woi", name_hash: 0x0C87F6D5, name: "movies\\woi\\woi_node_030.p3d", size: 1688, offset: 509687808 },
    WoiMember { archive: "01woi", name_hash: 0x0C95DC79, name: "movies\\woi\\woi_node_031.bik", size: 25682628, offset: 509689856 },
    WoiMember { archive: "01woi", name_hash: 0x0C960E56, name: "movies\\woi\\woi_node_031.p3d", size: 2188, offset: 535373824 },
    WoiMember { archive: "01woi", name_hash: 0x0CA3F3FA, name: "movies\\woi\\woi_node_032.bik", size: 33252132, offset: 535377920 },
    WoiMember { archive: "01woi", name_hash: 0x0CA425D7, name: "movies\\woi\\woi_node_032.p3d", size: 2682, offset: 568631296 },
    WoiMember { archive: "01woi", name_hash: 0x0CB20B7B, name: "movies\\woi\\woi_node_033.bik", size: 26121512, offset: 568635392 },
    WoiMember { archive: "01woi", name_hash: 0x0CB23D58, name: "movies\\woi\\woi_node_033.p3d", size: 1977, offset: 594757632 },
    WoiMember { archive: "01woi", name_hash: 0x0CC022FC, name: "movies\\woi\\woi_node_034.bik", size: 25098892, offset: 594759680 },
    WoiMember { archive: "01woi", name_hash: 0x0CC054D9, name: "movies\\woi\\woi_node_034.p3d", size: 2124, offset: 619859968 },
    WoiMember { archive: "01woi", name_hash: 0x0CCE3A7D, name: "movies\\woi\\woi_node_035.bik", size: 40339648, offset: 619864064 },
    WoiMember { archive: "01woi", name_hash: 0x0CCE6C5A, name: "movies\\woi\\woi_node_035.p3d", size: 2578, offset: 660205568 },
    WoiMember { archive: "01woi", name_hash: 0x0CDC51FE, name: "movies\\woi\\woi_node_036.bik", size: 24967576, offset: 660209664 },
    WoiMember { archive: "01woi", name_hash: 0x0CDC83DB, name: "movies\\woi\\woi_node_036.p3d", size: 1945, offset: 685178880 },
    WoiMember { archive: "01woi", name_hash: 0x0CEA697F, name: "movies\\woi\\woi_node_037.bik", size: 27085948, offset: 685180928 },
    WoiMember { archive: "01woi", name_hash: 0x0CEA9B5C, name: "movies\\woi\\woi_node_037.p3d", size: 2386, offset: 712267776 },
    WoiMember { archive: "01woi", name_hash: 0x0CF88100, name: "movies\\woi\\woi_node_038.bik", size: 23521880, offset: 712271872 },
    WoiMember { archive: "01woi", name_hash: 0x0CF8B2DD, name: "movies\\woi\\woi_node_038.p3d", size: 1546, offset: 735795200 },
    WoiMember { archive: "01woi", name_hash: 0x0D069881, name: "movies\\woi\\woi_node_039.bik", size: 17078268, offset: 735797248 },
    WoiMember { archive: "01woi", name_hash: 0x0D06CA5E, name: "movies\\woi\\woi_node_039.p3d", size: 1502, offset: 752875520 },
    WoiMember { archive: "01woi", name_hash: 0x0E3C9D97, name: "movies\\woi\\woi_node_040.bik", size: 13897100, offset: 752877568 },
    WoiMember { archive: "01woi", name_hash: 0x0E3CCF74, name: "movies\\woi\\woi_node_040.p3d", size: 1571, offset: 766775296 },
    WoiMember { archive: "01woi", name_hash: 0x0E4AB518, name: "movies\\woi\\woi_node_041.bik", size: 17701376, offset: 766777344 },
    WoiMember { archive: "01woi", name_hash: 0x0E4AE6F5, name: "movies\\woi\\woi_node_041.p3d", size: 1528, offset: 784480256 },
    WoiMember { archive: "01woi", name_hash: 0x0E58CC99, name: "movies\\woi\\woi_node_042.bik", size: 26123204, offset: 784482304 },
    WoiMember { archive: "01woi", name_hash: 0x0E58FE76, name: "movies\\woi\\woi_node_042.p3d", size: 2267, offset: 810606592 },
    WoiMember { archive: "01woi", name_hash: 0x0E66E41A, name: "movies\\woi\\woi_node_043.bik", size: 27849220, offset: 810610688 },
    WoiMember { archive: "01woi", name_hash: 0x0E6715F7, name: "movies\\woi\\woi_node_043.p3d", size: 2429, offset: 838461440 },
    WoiMember { archive: "01woi", name_hash: 0x0E74FB9B, name: "movies\\woi\\woi_node_044.bik", size: 16503656, offset: 838465536 },
    WoiMember { archive: "01woi", name_hash: 0x0E752D78, name: "movies\\woi\\woi_node_044.p3d", size: 1613, offset: 854970368 },
    WoiMember { archive: "01woi", name_hash: 0x0E83131C, name: "movies\\woi\\woi_node_045.bik", size: 19232088, offset: 854972416 },
    WoiMember { archive: "01woi", name_hash: 0x0E8344F9, name: "movies\\woi\\woi_node_045.p3d", size: 1612, offset: 874205184 },
    WoiMember { archive: "01woi", name_hash: 0x0E912A9D, name: "movies\\woi\\woi_node_046.bik", size: 16955032, offset: 874207232 },
    WoiMember { archive: "01woi", name_hash: 0x0E915C7A, name: "movies\\woi\\woi_node_046.p3d", size: 1828, offset: 891162624 },
    WoiMember { archive: "01woi", name_hash: 0x0E9F421E, name: "movies\\woi\\woi_node_047.bik", size: 19871888, offset: 891164672 },
    WoiMember { archive: "01woi", name_hash: 0x0E9F73FB, name: "movies\\woi\\woi_node_047.p3d", size: 1852, offset: 911038464 },
    WoiMember { archive: "01woi", name_hash: 0x0EAD599F, name: "movies\\woi\\woi_node_048.bik", size: 16937804, offset: 911040512 },
    WoiMember { archive: "01woi", name_hash: 0x0EAD8B7C, name: "movies\\woi\\woi_node_048.p3d", size: 1514, offset: 927979520 },
    WoiMember { archive: "01woi", name_hash: 0x0EBB7120, name: "movies\\woi\\woi_node_049.bik", size: 16342356, offset: 927981568 },
    WoiMember { archive: "01woi", name_hash: 0x0EBBA2FD, name: "movies\\woi\\woi_node_049.p3d", size: 1886, offset: 944324608 },
    WoiMember { archive: "01woi", name_hash: 0x0FF17636, name: "movies\\woi\\woi_node_050.bik", size: 23054384, offset: 944326656 },
    WoiMember { archive: "01woi", name_hash: 0x0FF1A813, name: "movies\\woi\\woi_node_050.p3d", size: 2005, offset: 967383040 },
    WoiMember { archive: "01woi", name_hash: 0x0FFF8DB7, name: "movies\\woi\\woi_node_051.bik", size: 29437432, offset: 967385088 },
    WoiMember { archive: "02woi", name_hash: 0x0FFFBF94, name: "movies\\woi\\woi_node_051.p3d", size: 2597, offset: 6144 },
    WoiMember { archive: "02woi", name_hash: 0x101BBCB9, name: "movies\\woi\\woi_node_053.bik", size: 21588252, offset: 10240 },
    WoiMember { archive: "02woi", name_hash: 0x101BEE96, name: "movies\\woi\\woi_node_053.p3d", size: 2081, offset: 21600256 },
    WoiMember { archive: "02woi", name_hash: 0x1037EBBB, name: "movies\\woi\\woi_node_055.bik", size: 18121652, offset: 21604352 },
    WoiMember { archive: "02woi", name_hash: 0x10381D98, name: "movies\\woi\\woi_node_055.p3d", size: 1488, offset: 39727104 },
    WoiMember { archive: "02woi", name_hash: 0x10541ABD, name: "movies\\woi\\woi_node_057.bik", size: 17652940, offset: 39729152 },
    WoiMember { archive: "02woi", name_hash: 0x10544C9A, name: "movies\\woi\\woi_node_057.p3d", size: 1375, offset: 57382912 },
    WoiMember { archive: "02woi", name_hash: 0x1062323E, name: "movies\\woi\\woi_node_058.bik", size: 25207484, offset: 57384960 },
    WoiMember { archive: "02woi", name_hash: 0x1062641B, name: "movies\\woi\\woi_node_058.p3d", size: 2277, offset: 82593792 },
    WoiMember { archive: "02woi", name_hash: 0x107049BF, name: "movies\\woi\\woi_node_059.bik", size: 20557328, offset: 82597888 },
    WoiMember { archive: "02woi", name_hash: 0x10707B9C, name: "movies\\woi\\woi_node_059.p3d", size: 1679, offset: 103155712 },
    WoiMember { archive: "02woi", name_hash: 0x11A64ED5, name: "movies\\woi\\woi_node_060.bik", size: 25851532, offset: 103157760 },
    WoiMember { archive: "02woi", name_hash: 0x11A680B2, name: "movies\\woi\\woi_node_060.p3d", size: 1528, offset: 129009664 },
    WoiMember { archive: "02woi", name_hash: 0x11B46656, name: "movies\\woi\\woi_node_061.bik", size: 60003300, offset: 129011712 },
    WoiMember { archive: "02woi", name_hash: 0x11B49833, name: "movies\\woi\\woi_node_061.p3d", size: 3649, offset: 189016064 },
    WoiMember { archive: "02woi", name_hash: 0x11C27DD7, name: "movies\\woi\\woi_node_062.bik", size: 24476916, offset: 189020160 },
    WoiMember { archive: "02woi", name_hash: 0x11C2AFB4, name: "movies\\woi\\woi_node_062.p3d", size: 2017, offset: 213497856 },
    WoiMember { archive: "02woi", name_hash: 0x11D09558, name: "movies\\woi\\woi_node_063.bik", size: 39695576, offset: 213499904 },
    WoiMember { archive: "02woi", name_hash: 0x11D0C735, name: "movies\\woi\\woi_node_063.p3d", size: 3446, offset: 253196288 },
    WoiMember { archive: "02woi", name_hash: 0x11DEACD9, name: "movies\\woi\\woi_node_064.bik", size: 30967316, offset: 253200384 },
    WoiMember { archive: "02woi", name_hash: 0x11DEDEB6, name: "movies\\woi\\woi_node_064.p3d", size: 2247, offset: 284168192 },
    WoiMember { archive: "02woi", name_hash: 0x11ECC45A, name: "movies\\woi\\woi_node_065.bik", size: 21989520, offset: 284172288 },
    WoiMember { archive: "02woi", name_hash: 0x11ECF637, name: "movies\\woi\\woi_node_065.p3d", size: 1983, offset: 306163712 },
    WoiMember { archive: "02woi", name_hash: 0x11FADBDB, name: "movies\\woi\\woi_node_066.bik", size: 28709208, offset: 306165760 },
    WoiMember { archive: "02woi", name_hash: 0x11FB0DB8, name: "movies\\woi\\woi_node_066.p3d", size: 1581, offset: 334876672 },
    WoiMember { archive: "02woi", name_hash: 0x1208F35C, name: "movies\\woi\\woi_node_067.bik", size: 28812296, offset: 334878720 },
    WoiMember { archive: "02woi", name_hash: 0x12092539, name: "movies\\woi\\woi_node_067.p3d", size: 1771, offset: 363692032 },
    WoiMember { archive: "02woi", name_hash: 0x12170ADD, name: "movies\\woi\\woi_node_068.bik", size: 24499240, offset: 363694080 },
    WoiMember { archive: "02woi", name_hash: 0x12173CBA, name: "movies\\woi\\woi_node_068.p3d", size: 1947, offset: 388194304 },
    WoiMember { archive: "02woi", name_hash: 0x1225225E, name: "movies\\woi\\woi_node_069.bik", size: 28686904, offset: 388196352 },
    WoiMember { archive: "02woi", name_hash: 0x1225543B, name: "movies\\woi\\woi_node_069.p3d", size: 1825, offset: 416884736 },
    WoiMember { archive: "02woi", name_hash: 0x135B2774, name: "movies\\woi\\woi_node_070.bik", size: 30987432, offset: 416886784 },
    WoiMember { archive: "02woi", name_hash: 0x135B5951, name: "movies\\woi\\woi_node_070.p3d", size: 2245, offset: 447875072 },
    WoiMember { archive: "02woi", name_hash: 0x13693EF5, name: "movies\\woi\\woi_node_071.bik", size: 23014048, offset: 447879168 },
    WoiMember { archive: "02woi", name_hash: 0x136970D2, name: "movies\\woi\\woi_node_071.p3d", size: 1965, offset: 470894592 },
    WoiMember { archive: "02woi", name_hash: 0x13775676, name: "movies\\woi\\woi_node_072.bik", size: 33957116, offset: 470896640 },
    WoiMember { archive: "02woi", name_hash: 0x13778853, name: "movies\\woi\\woi_node_072.p3d", size: 2102, offset: 504854528 },
    WoiMember { archive: "02woi", name_hash: 0x13856DF7, name: "movies\\woi\\woi_node_073.bik", size: 25202240, offset: 504858624 },
    WoiMember { archive: "02woi", name_hash: 0x13859FD4, name: "movies\\woi\\woi_node_073.p3d", size: 1873, offset: 530061312 },
    WoiMember { archive: "02woi", name_hash: 0x13938578, name: "movies\\woi\\woi_node_074.bik", size: 27789528, offset: 530063360 },
    WoiMember { archive: "02woi", name_hash: 0x1393B755, name: "movies\\woi\\woi_node_074.p3d", size: 2491, offset: 557854720 },
    WoiMember { archive: "02woi", name_hash: 0x13A19CF9, name: "movies\\woi\\woi_node_075.bik", size: 32849832, offset: 557858816 },
    WoiMember { archive: "02woi", name_hash: 0x13A1CED6, name: "movies\\woi\\woi_node_075.p3d", size: 2170, offset: 590708736 },
    WoiMember { archive: "02woi", name_hash: 0x13CBE37C, name: "movies\\woi\\woi_node_078.bik", size: 36307880, offset: 590712832 },
    WoiMember { archive: "02woi", name_hash: 0x13CC1559, name: "movies\\woi\\woi_node_078.p3d", size: 2395, offset: 627021824 },
    WoiMember { archive: "02woi", name_hash: 0x13D9FAFD, name: "movies\\woi\\woi_node_079.bik", size: 34059240, offset: 627025920 },
    WoiMember { archive: "02woi", name_hash: 0x13DA2CDA, name: "movies\\woi\\woi_node_079.p3d", size: 2445, offset: 661086208 },
    WoiMember { archive: "02woi", name_hash: 0x15100013, name: "movies\\woi\\woi_node_080.bik", size: 38470924, offset: 661090304 },
    WoiMember { archive: "02woi", name_hash: 0x151031F0, name: "movies\\woi\\woi_node_080.p3d", size: 3093, offset: 699561984 },
    WoiMember { archive: "02woi", name_hash: 0x151E1794, name: "movies\\woi\\woi_node_081.bik", size: 26887128, offset: 699566080 },
    WoiMember { archive: "02woi", name_hash: 0x151E4971, name: "movies\\woi\\woi_node_081.p3d", size: 2083, offset: 726454272 },
    WoiMember { archive: "02woi", name_hash: 0x15485E17, name: "movies\\woi\\woi_node_084.bik", size: 16656416, offset: 726458368 },
    WoiMember { archive: "02woi", name_hash: 0x15488FF4, name: "movies\\woi\\woi_node_084.p3d", size: 1702, offset: 743116800 },
    WoiMember { archive: "02woi", name_hash: 0x15648D19, name: "movies\\woi\\woi_node_086.bik", size: 32777816, offset: 743118848 },
    WoiMember { archive: "02woi", name_hash: 0x1564BEF6, name: "movies\\woi\\woi_node_086.p3d", size: 2240, offset: 775897088 },
    WoiMember { archive: "02woi", name_hash: 0x1572A49A, name: "movies\\woi\\woi_node_087.bik", size: 45872524, offset: 775901184 },
    WoiMember { archive: "02woi", name_hash: 0x1572D677, name: "movies\\woi\\woi_node_087.p3d", size: 2936, offset: 821774336 },
    WoiMember { archive: "02woi", name_hash: 0x16D2F033, name: "movies\\woi\\woi_node_091.bik", size: 22181108, offset: 821778432 },
    WoiMember { archive: "02woi", name_hash: 0x16D32210, name: "movies\\woi\\woi_node_091.p3d", size: 1827, offset: 843960320 },
    WoiMember { archive: "02woi", name_hash: 0x16E107B4, name: "movies\\woi\\woi_node_092.bik", size: 20941832, offset: 843962368 },
    WoiMember { archive: "02woi", name_hash: 0x16E13991, name: "movies\\woi\\woi_node_092.p3d", size: 1768, offset: 864905216 },
    WoiMember { archive: "02woi", name_hash: 0x16EF1F35, name: "movies\\woi\\woi_node_093.bik", size: 30746056, offset: 864907264 },
    WoiMember { archive: "02woi", name_hash: 0x16EF5112, name: "movies\\woi\\woi_node_093.p3d", size: 2237, offset: 895653888 },
    WoiMember { archive: "02woi", name_hash: 0x16FD36B6, name: "movies\\woi\\woi_node_094.bik", size: 19956052, offset: 895657984 },
    WoiMember { archive: "02woi", name_hash: 0x16FD6893, name: "movies\\woi\\woi_node_094.p3d", size: 1912, offset: 915615744 },
    WoiMember { archive: "02woi", name_hash: 0x170B4E37, name: "movies\\woi\\woi_node_095.bik", size: 28042756, offset: 915617792 },
    WoiMember { archive: "02woi", name_hash: 0x170B8014, name: "movies\\woi\\woi_node_095.p3d", size: 2136, offset: 943661056 },
    WoiMember { archive: "02woi", name_hash: 0x173594BA, name: "movies\\woi\\woi_node_098.bik", size: 32928108, offset: 943665152 },
    WoiMember { archive: "02woi", name_hash: 0x1735C697, name: "movies\\woi\\woi_node_098.p3d", size: 2427, offset: 976594944 },
    WoiMember { archive: "02woi", name_hash: 0x1743AC3B, name: "movies\\woi\\woi_node_099.bik", size: 17166992, offset: 976599040 },
    WoiMember { archive: "03woi", name_hash: 0x1743DE18, name: "movies\\woi\\woi_node_099.p3d", size: 1650, offset: 8192 },
    WoiMember { archive: "03woi", name_hash: 0x3C4F765C, name: "movies\\woi\\woi_node_100.bik", size: 18698544, offset: 10240 },
    WoiMember { archive: "03woi", name_hash: 0x3C4FA839, name: "movies\\woi\\woi_node_100.p3d", size: 1613, offset: 18710528 },
    WoiMember { archive: "03woi", name_hash: 0x3C5D8DDD, name: "movies\\woi\\woi_node_101.bik", size: 11490456, offset: 18712576 },
    WoiMember { archive: "03woi", name_hash: 0x3C5DBFBA, name: "movies\\woi\\woi_node_101.p3d", size: 1192, offset: 30203904 },
    WoiMember { archive: "03woi", name_hash: 0x3C6BA55E, name: "movies\\woi\\woi_node_102.bik", size: 21677256, offset: 30205952 },
    WoiMember { archive: "03woi", name_hash: 0x3C6BD73B, name: "movies\\woi\\woi_node_102.p3d", size: 1803, offset: 51884032 },
    WoiMember { archive: "03woi", name_hash: 0x3C79BCDF, name: "movies\\woi\\woi_node_103.bik", size: 21184820, offset: 51886080 },
    WoiMember { archive: "03woi", name_hash: 0x3C79EEBC, name: "movies\\woi\\woi_node_103.p3d", size: 1896, offset: 73072640 },
    WoiMember { archive: "03woi", name_hash: 0x3C87D460, name: "movies\\woi\\woi_node_104.bik", size: 20573708, offset: 73074688 },
    WoiMember { archive: "03woi", name_hash: 0x3C88063D, name: "movies\\woi\\woi_node_104.p3d", size: 1391, offset: 93648896 },
    WoiMember { archive: "03woi", name_hash: 0x3C95EBE1, name: "movies\\woi\\woi_node_105.bik", size: 16817408, offset: 93650944 },
    WoiMember { archive: "03woi", name_hash: 0x3C961DBE, name: "movies\\woi\\woi_node_105.p3d", size: 2061, offset: 110469120 },
    WoiMember { archive: "03woi", name_hash: 0x3CA40362, name: "movies\\woi\\woi_node_106.bik", size: 12748516, offset: 110473216 },
    WoiMember { archive: "03woi", name_hash: 0x3CA4353F, name: "movies\\woi\\woi_node_106.p3d", size: 1303, offset: 123222016 },
    WoiMember { archive: "03woi", name_hash: 0x3CB21AE3, name: "movies\\woi\\woi_node_107.bik", size: 19733992, offset: 123224064 },
    WoiMember { archive: "03woi", name_hash: 0x3CB24CC0, name: "movies\\woi\\woi_node_107.p3d", size: 2002, offset: 142958592 },
    WoiMember { archive: "03woi", name_hash: 0x3CCE49E5, name: "movies\\woi\\woi_node_109.bik", size: 21718432, offset: 142960640 },
    WoiMember { archive: "03woi", name_hash: 0x3CCE7BC2, name: "movies\\woi\\woi_node_109.p3d", size: 1985, offset: 164679680 },
    WoiMember { archive: "03woi", name_hash: 0x3E044EFB, name: "movies\\woi\\woi_node_110.bik", size: 18668752, offset: 164681728 },
    WoiMember { archive: "03woi", name_hash: 0x3E0480D8, name: "movies\\woi\\woi_node_110.p3d", size: 1720, offset: 183351296 },
    WoiMember { archive: "03woi", name_hash: 0x3E207DFD, name: "movies\\woi\\woi_node_112.bik", size: 32039152, offset: 183353344 },
    WoiMember { archive: "03woi", name_hash: 0x3E20AFDA, name: "movies\\woi\\woi_node_112.p3d", size: 2011, offset: 215394304 },
    WoiMember { archive: "03woi", name_hash: 0x3E3CACFF, name: "movies\\woi\\woi_node_114.bik", size: 24047512, offset: 215396352 },
    WoiMember { archive: "03woi", name_hash: 0x3E3CDEDC, name: "movies\\woi\\woi_node_114.p3d", size: 1963, offset: 239443968 },
    WoiMember { archive: "03woi", name_hash: 0x3E66F382, name: "movies\\woi\\woi_node_117.bik", size: 15459748, offset: 239446016 },
    WoiMember { archive: "03woi", name_hash: 0x3E67255F, name: "movies\\woi\\woi_node_117.p3d", size: 1677, offset: 254906368 },
    WoiMember { archive: "03woi", name_hash: 0x3E832284, name: "movies\\woi\\woi_node_119.bik", size: 15940640, offset: 254908416 },
    WoiMember { archive: "03woi", name_hash: 0x3E835461, name: "movies\\woi\\woi_node_119.p3d", size: 1330, offset: 270850048 },
    WoiMember { archive: "03woi", name_hash: 0x3FB9279A, name: "movies\\woi\\woi_node_120.bik", size: 16534936, offset: 270852096 },
    WoiMember { archive: "03woi", name_hash: 0x3FB95977, name: "movies\\woi\\woi_node_120.p3d", size: 1457, offset: 287387648 },
    WoiMember { archive: "03woi", name_hash: 0x3FD5569C, name: "movies\\woi\\woi_node_122.bik", size: 23602852, offset: 287389696 },
    WoiMember { archive: "03woi", name_hash: 0x3FD58879, name: "movies\\woi\\woi_node_122.p3d", size: 2080, offset: 310992896 },
    WoiMember { archive: "03woi", name_hash: 0x3FF1859E, name: "movies\\woi\\woi_node_124.bik", size: 24720520, offset: 310996992 },
    WoiMember { archive: "03woi", name_hash: 0x3FF1B77B, name: "movies\\woi\\woi_node_124.p3d", size: 2309, offset: 335718400 },
    WoiMember { archive: "03woi", name_hash: 0x3FFF9D1F, name: "movies\\woi\\woi_node_125.bik", size: 28634804, offset: 335722496 },
    WoiMember { archive: "03woi", name_hash: 0x3FFFCEFC, name: "movies\\woi\\woi_node_125.p3d", size: 2054, offset: 364357632 },
    WoiMember { archive: "03woi", name_hash: 0x400DB4A0, name: "movies\\woi\\woi_node_126.bik", size: 29039956, offset: 364361728 },
    WoiMember { archive: "03woi", name_hash: 0x400DE67D, name: "movies\\woi\\woi_node_126.p3d", size: 2501, offset: 393402368 },
    WoiMember { archive: "03woi", name_hash: 0x401BCC21, name: "movies\\woi\\woi_node_127.bik", size: 32133096, offset: 393406464 },
    WoiMember { archive: "03woi", name_hash: 0x401BFDFE, name: "movies\\woi\\woi_node_127.p3d", size: 2710, offset: 425539584 },
    WoiMember { archive: "03woi", name_hash: 0x4029E3A2, name: "movies\\woi\\woi_node_128.bik", size: 26528440, offset: 425543680 },
    WoiMember { archive: "03woi", name_hash: 0x402A157F, name: "movies\\woi\\woi_node_128.p3d", size: 1947, offset: 452073472 },
    WoiMember { archive: "03woi", name_hash: 0x4037FB23, name: "movies\\woi\\woi_node_129.bik", size: 21514680, offset: 452075520 },
    WoiMember { archive: "03woi", name_hash: 0x40382D00, name: "movies\\woi\\woi_node_129.p3d", size: 1850, offset: 473591808 },
    WoiMember { archive: "03woi", name_hash: 0x44D7B177, name: "movies\\woi\\woi_node_150.bik", size: 13977472, offset: 473593856 },
    WoiMember { archive: "03woi", name_hash: 0x44D7E354, name: "movies\\woi\\woi_node_150.p3d", size: 1415, offset: 487571456 },
    WoiMember { archive: "03woi", name_hash: 0x4501F7FA, name: "movies\\woi\\woi_node_153.bik", size: 12662088, offset: 487573504 },
    WoiMember { archive: "03woi", name_hash: 0x450229D7, name: "movies\\woi\\woi_node_153.p3d", size: 1342, offset: 500236288 },
    WoiMember { archive: "03woi", name_hash: 0x451E26FC, name: "movies\\woi\\woi_node_155.bik", size: 17076380, offset: 500238336 },
    WoiMember { archive: "03woi", name_hash: 0x451E58D9, name: "movies\\woi\\woi_node_155.p3d", size: 1580, offset: 517316608 },
    WoiMember { archive: "03woi", name_hash: 0x46FD461E, name: "movies\\woi\\woi_node_168.bik", size: 16342556, offset: 517318656 },
    WoiMember { archive: "03woi", name_hash: 0x46FD77FB, name: "movies\\woi\\woi_node_168.p3d", size: 1511, offset: 533661696 },
    WoiMember { archive: "03woi", name_hash: 0x470B5D9F, name: "movies\\woi\\woi_node_169.bik", size: 23376076, offset: 533663744 },
    WoiMember { archive: "03woi", name_hash: 0x470B8F7C, name: "movies\\woi\\woi_node_169.p3d", size: 2175, offset: 557041664 },
    WoiMember { archive: "03woi", name_hash: 0x485D91B7, name: "movies\\woi\\woi_node_172.bik", size: 22060468, offset: 557045760 },
    WoiMember { archive: "03woi", name_hash: 0x485DC394, name: "movies\\woi\\woi_node_172.p3d", size: 2326, offset: 579106816 },
    WoiMember { archive: "03woi", name_hash: 0x486BA938, name: "movies\\woi\\woi_node_173.bik", size: 12787284, offset: 579110912 },
    WoiMember { archive: "03woi", name_hash: 0x486BDB15, name: "movies\\woi\\woi_node_173.p3d", size: 1409, offset: 591898624 },
    WoiMember { archive: "03woi", name_hash: 0x48A4073C, name: "movies\\woi\\woi_node_177.bik", size: 13066116, offset: 591900672 },
    WoiMember { archive: "03woi", name_hash: 0x48A43919, name: "movies\\woi\\woi_node_177.p3d", size: 1377, offset: 604966912 },
    WoiMember { archive: "03woi", name_hash: 0x4A3CB0D9, name: "movies\\woi\\woi_node_185.bik", size: 18099084, offset: 604968960 },
    WoiMember { archive: "03woi", name_hash: 0x4A3CE2B6, name: "movies\\woi\\woi_node_185.p3d", size: 1857, offset: 623069184 },
    WoiMember { archive: "03woi", name_hash: 0x4A58DFDB, name: "movies\\woi\\woi_node_187.bik", size: 13134336, offset: 623071232 },
    WoiMember { archive: "03woi", name_hash: 0x4A5911B8, name: "movies\\woi\\woi_node_187.p3d", size: 1518, offset: 636207104 },
    WoiMember { archive: "03woi", name_hash: 0x4A750EDD, name: "movies\\woi\\woi_node_189.bik", size: 21834600, offset: 636209152 },
    WoiMember { archive: "03woi", name_hash: 0x4A7540BA, name: "movies\\woi\\woi_node_189.p3d", size: 2181, offset: 658044928 },
    WoiMember { archive: "03woi", name_hash: 0x4BAB13F3, name: "movies\\woi\\woi_node_190.bik", size: 38816404, offset: 658049024 },
    WoiMember { archive: "03woi", name_hash: 0x4BAB45D0, name: "movies\\woi\\woi_node_190.p3d", size: 2632, offset: 696866816 },
    WoiMember { archive: "03woi", name_hash: 0x4BB92B74, name: "movies\\woi\\woi_node_191.bik", size: 29421500, offset: 696870912 },
    WoiMember { archive: "03woi", name_hash: 0x7143C91E, name: "movies\\woi\\woi_node_201.bik", size: 14521116, offset: 726292480 },
    WoiMember { archive: "03woi", name_hash: 0x7143FAFB, name: "movies\\woi\\woi_node_201.p3d", size: 1341, offset: 740814848 },
    WoiMember { archive: "03woi", name_hash: 0x71985624, name: "movies\\woi\\woi_node_207.bik", size: 29943492, offset: 740816896 },
    WoiMember { archive: "03woi", name_hash: 0x71988801, name: "movies\\woi\\woi_node_207.p3d", size: 2435, offset: 770760704 },
    WoiMember { archive: "03woi", name_hash: 0x71A66DA5, name: "movies\\woi\\woi_node_208.bik", size: 34317424, offset: 770764800 },
    WoiMember { archive: "03woi", name_hash: 0x71A69F82, name: "movies\\woi\\woi_node_208.p3d", size: 2851, offset: 805083136 },
    WoiMember { archive: "03woi", name_hash: 0x71B48526, name: "movies\\woi\\woi_node_209.bik", size: 19038624, offset: 805087232 },
    WoiMember { archive: "03woi", name_hash: 0x71B4B703, name: "movies\\woi\\woi_node_209.p3d", size: 1663, offset: 824127488 },
    WoiMember { archive: "03woi", name_hash: 0x72F8A1BD, name: "movies\\woi\\woi_node_211.bik", size: 25581328, offset: 824129536 },
    WoiMember { archive: "03woi", name_hash: 0x72F8D39A, name: "movies\\woi\\woi_node_211.p3d", size: 2442, offset: 849711104 },
    WoiMember { archive: "03woi", name_hash: 0x7306B93E, name: "movies\\woi\\woi_node_212.bik", size: 32248004, offset: 849715200 },
    WoiMember { archive: "03woi", name_hash: 0x7306EB1B, name: "movies\\woi\\woi_node_212.p3d", size: 2879, offset: 881965056 },
    WoiMember { archive: "03woi", name_hash: 0x7314D0BF, name: "movies\\woi\\woi_node_213.bik", size: 34786132, offset: 881969152 },
    WoiMember { archive: "03woi", name_hash: 0x7322E840, name: "movies\\woi\\woi_node_214.bik", size: 22190404, offset: 916756480 },
    WoiMember { archive: "03woi", name_hash: 0x73231A1D, name: "movies\\woi\\woi_node_214.p3d", size: 143, offset: 938948608 },
    WoiMember { archive: "03woi", name_hash: 0x7331319E, name: "movies\\woi\\woi_node_215.p3d", size: 1245, offset: 938950656 },
    WoiMember { archive: "03woi", name_hash: 0x733F1742, name: "movies\\woi\\woi_node_216.bik", size: 35832164, offset: 938952704 },
    WoiMember { archive: "03woi", name_hash: 0x733F491F, name: "movies\\woi\\woi_node_216.p3d", size: 2749, offset: 974786560 },
    WoiMember { archive: "03woi", name_hash: 0x734D2EC3, name: "movies\\woi\\woi_node_217.bik", size: 23017828, offset: 974790656 },
    WoiMember { archive: "03woi", name_hash: 0x734D60A0, name: "movies\\woi\\woi_node_217.p3d", size: 1860, offset: 997810176 },
    WoiMember { archive: "03woi", name_hash: 0x73695DC5, name: "movies\\woi\\woi_node_219.bik", size: 18251240, offset: 997812224 },
    WoiMember { archive: "03woi", name_hash: 0x73698FA2, name: "movies\\woi\\woi_node_219.p3d", size: 2171, offset: 1016064000 },
    WoiMember { archive: "03woi", name_hash: 0x749F62DB, name: "movies\\woi\\woi_node_220.bik", size: 19510436, offset: 1016068096 },
    WoiMember { archive: "03woi", name_hash: 0x749F94B8, name: "movies\\woi\\woi_node_220.p3d", size: 1783, offset: 1035579392 },
    WoiMember { archive: "03woi", name_hash: 0x74AD7A5C, name: "movies\\woi\\woi_node_221.bik", size: 13574276, offset: 1035581440 },
    WoiMember { archive: "03woi", name_hash: 0x74ADAC39, name: "movies\\woi\\woi_node_221.p3d", size: 1320, offset: 1049157632 },
];
#[cfg(test)]
mod tests {
    use super::*;
    use crate::subtitle::FmvSubtitles;

    fn fixture(name: &str) -> Vec<u8> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    /// Block 60: the AUDIO-FLAG CENSUS over ALL 138 WoI node/EC
    /// movies — 290 audio tracks, EVERY ONE with the USEDCT bit
    /// (0x1000) at 48 kHz.  The heads ship as ONE concatenated
    /// fixture (138 × 256-byte surgical Range reads, in
    /// [`WOI_BIKS`] order); each head's stored size must agree with
    /// the table's member size (two-sided pin).  Together with
    /// `members.rs`'s movies.rcf census this proves the game's
    /// ENTIRE shipped movie inventory (157 movies, 326 tracks) is
    /// DCT-only — the player's RDFT fail-closed gate never fires on
    /// real content.
    #[test]
    fn all_woi_movies_are_dct_only_flag_census() {
        use crate::BikHeader;
        let blob = fixture("woi_bik_heads_138x256.bin");
        assert_eq!(blob.len(), WOI_BIKS.len() * 256);
        let mut total_tracks = 0usize;
        for (i, m) in WOI_BIKS.iter().enumerate() {
            let head = &blob[i * 256..(i + 1) * 256];
            let (hdr, _) =
                BikHeader::parse(head).unwrap_or_else(|e| panic!("{}: {e}", m.name));
            assert_eq!(hdr.file_size, m.size, "{}: stored+8 == member size", m.name);
            let t = hdr.audio_track_count as usize;
            assert!(t <= 4, "{}: track count {t}", m.name);
            for k in 0..t {
                let base = 44 + 4 * t + 4 * k;
                let rate = u16::from_le_bytes([head[base], head[base + 1]]);
                let flags = u16::from_le_bytes([head[base + 2], head[base + 3]]);
                assert_eq!(rate, 48_000, "{}: track {k} rate", m.name);
                assert_eq!(
                    flags & 0x1000,
                    0x1000,
                    "{}: track {k} flags {flags:#06x} lack USEDCT",
                    m.name
                );
                assert!(flags == 0x7000 || flags == 0x5000, "{}", m.name);
                total_tracks += 1;
            }
        }
        assert_eq!(total_tracks, 290, "the WoI census total");
    }

    /// EVERY member name hashes exactly to its directory hash — the
    /// 275/275 crack, enforced permanently.
    #[test]
    fn every_name_hashes_to_its_directory_hash() {
        assert_eq!(WOI_MEMBERS.len(), 275);
        for m in WOI_MEMBERS {
            assert_eq!(prototype_rcf::hash_name(m.name), m.name_hash, "{}", m.name);
        }
    }

    /// Census shape: 138 Bink movies + 137 bibles across the four
    /// archives (25 / 84 / 70 / 96 members); each archive's slice is
    /// hash-sorted like the real directory.
    #[test]
    fn census_shape_matches_the_real_directories() {
        assert_eq!(WOI_BIKS.len(), 138);
        assert_eq!(WOI_BIBLES.len(), 137);
        assert_eq!(WOI_MEMBERS.len(), WOI_BIKS.len() + WOI_BIBLES.len());
        for (a, len) in [("00woi", 25usize), ("01woi", 84), ("02woi", 70), ("03woi", 96)] {
            let mut h: Vec<u32> = WOI_MEMBERS
                .iter()
                .filter(|m| m.archive == a)
                .map(|m| m.name_hash)
                .collect();
            assert_eq!(h.len(), len, "{a}");
            let sorted = h.clone();
            h.sort_unstable();
            assert_eq!(h, sorted, "{a} directory is hash-sorted");
        }
        let mut b = WOI_BIKS.iter().map(|m| m.name_hash).collect::<Vec<_>>();
        b.sort_unstable();
        // WOI_BIKS concatenates the four per-archive sorted spans
        assert_eq!(b.len(), 138);
        assert_eq!(b.iter().collect::<std::collections::BTreeSet<_>>().len(), 138);
    }

    /// The 00woi census against the REAL committed directory bytes
    /// (woi_00woi_dir.bin = 00woi.rcf bytes 60..2160): the first 25
    /// 12-B entries must match hash/offset/size exactly.
    #[test]
    fn census_matches_real_directory_bytes() {
        let dir = fixture("woi_00woi_dir.bin");
        let mut mine: Vec<&WoiMember> = WOI_MEMBERS
            .iter()
            .filter(|m| m.archive == "00woi")
            .collect();
        assert_eq!(mine.len(), 25);
        // the real directory is HASH-sorted (the movies.rcf precedent)
        mine.sort_by_key(|m| m.name_hash);
        for (i, m) in mine.iter().enumerate() {
            let hh = u32::from_le_bytes([
                dir[i * 12],
                dir[i * 12 + 1],
                dir[i * 12 + 2],
                dir[i * 12 + 3],
            ]);
            let off = u32::from_le_bytes([
                dir[i * 12 + 4],
                dir[i * 12 + 5],
                dir[i * 12 + 6],
                dir[i * 12 + 7],
            ]);
            let size = u32::from_le_bytes([
                dir[i * 12 + 8],
                dir[i * 12 + 9],
                dir[i * 12 + 10],
                dir[i * 12 + 11],
            ]);
            assert_eq!(m.name_hash, hh, "{}", m.name);
            assert_eq!(m.offset, off as u64, "{}", m.name);
            assert_eq!(m.size, size as u64, "{}", m.name);
        }
    }

    /// The full committed directories of 01/02/03woi match too (each
    /// fixture is the archive's exact directory span).
    #[test]
    fn census_matches_full_directories_01_03() {
        for (fix, archive, count) in [
            ("woi_01woi_dir.bin", "01woi", 84usize),
            ("woi_02woi_dir.bin", "02woi", 70),
            ("woi_03woi_dir.bin", "03woi", 96),
        ] {
            let dir = fixture(fix);
            assert_eq!(dir.len(), count * 12, "{fix}");
            let mut mine: Vec<&WoiMember> =
                WOI_MEMBERS.iter().filter(|m| m.archive == archive).collect();
            mine.sort_by_key(|m| m.offset);
            assert_eq!(mine.len(), count, "{archive}");
            for (i, m) in mine.iter().enumerate() {
                let hh = u32::from_le_bytes([
                    dir[i * 12],
                    dir[i * 12 + 1],
                    dir[i * 12 + 2],
                    dir[i * 12 + 3],
                ]);
                assert_eq!(m.name_hash, hh, "{} vs entry {i}", m.name);
            }
        }
    }

    /// The node-id census: 131 node movies over a SPARSE id set
    /// (node_002 does not exist; ids 130–149 and 192–200 are almost
    /// entirely absent), plus the 7 ending cinematics in 00woi.
    #[test]
    fn node_ids_are_sparse_and_the_ecs_are_seven() {
        let node_ids: Vec<u32> = WOI_BIKS
            .iter()
            .filter(|m| m.name.contains("woi_node_"))
            .map(|m| {
                m.name
                    .split("woi_node_")
                    .nth(1)
                    .unwrap()
                    .trim_end_matches(".bik")
                    .parse()
                    .unwrap()
            })
            .collect();
        assert_eq!(node_ids.len(), 131);
        assert!(!node_ids.contains(&2), "node_002 does not exist");
        assert!(node_ids.contains(&1));
        assert!(node_ids.contains(&221));
        assert!(node_ids.contains(&201));
        for id in [130usize, 140, 194, 200] {
            assert!(
                !node_ids.contains(&(id as u32)),
                "unexpected node id {id}"
            );
        }
        let ecs: Vec<&WoiMember> = WOI_BIKS
            .iter()
            .filter(|m| m.name.contains("woi_ec"))
            .collect();
        assert_eq!(ecs.len(), 7);
        assert!(ecs.iter().all(|m| m.archive == "00woi"));
    }

    /// A real committed bible parses through the SAME subtitle path
    /// movies.rcf uses, and its content chunk names its movie —
    /// `woi_node_001` / `woi_ec1` — the naming ground truth.
    #[test]
    fn real_woi_bibles_parse_and_name_their_movies() {
        let b = FmvSubtitles::parse(&fixture("woi_bible_node001.p3d"))
            .expect("woi_node_001 bible parses");
        assert_eq!(b.movie_id.as_deref(), Some("woi_node_001"));
        assert!(b.languages.iter().any(|l| l.language == "english"));
        assert!(b.languages.iter().any(|l| l.language == "spanish"));
        assert!(b.languages[0]
            .entries
            .iter()
            .any(|e| e.text.contains("number one terrorist threat")));

        let b = FmvSubtitles::parse(&fixture("woi_bible_ec1.p3d"))
            .expect("woi_ec1 bible parses");
        assert_eq!(b.movie_id.as_deref(), Some("woi_ec1"));
    }

    /// `woi_by_hash` round-trips, and the bik/bible archive split is
    /// honest: node_007's MOVIE ships in 00woi while its BIBLE ships
    /// in 01woi (pinned explicitly).
    #[test]
    fn bik_and_bible_pairs_share_ids() {
        let node001 = woi_by_hash(prototype_rcf::hash_name("movies\\woi\\woi_node_001.bik"))
            .expect("node_001 known");
        assert_eq!(node001.archive, "00woi");
        assert_eq!(node001.size, 19_053_992);
        let b7 = WOI_BIBLES
            .iter()
            .find(|m| m.name.contains("woi_node_007"))
            .expect("node_007 bible exists");
        assert_eq!(b7.archive, "01woi", "node_007's bible ships in 01woi");
        let m7 = WOI_BIKS
            .iter()
            .find(|m| m.name.contains("woi_node_007"))
            .expect("node_007 movie exists");
        assert_eq!(m7.archive, "00woi", "node_007's movie ships in 00woi");
    }
}

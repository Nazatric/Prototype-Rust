//! naga_wgsl_validate — REAL WGSL compile checks, device-free (block 36).
//!
//! Every shader the GPU edge will ever compile goes through
//! `shader::wgsl::translate_program{,_with_plan}` → a WGSL string →
//! `wgpu::Device::create_shader_module`.  wgpu feeds that string to
//! **naga** (its embedded WGSL front-end) BEFORE anything touches the
//! GPU — so a naga parse/validation error is a guaranteed on-device
//! pipeline-creation failure.  Until this file existed, no WGSL compiler
//! was available in the sandbox and the honest-limits list carried
//! "the output WGSL is NOT verified to compile".  That limit is closed:
//! naga's `parse_str` + `Validator::validate` are pure CPU — no device,
//! no adapter, no surface.
//!
//! What is compile-checked here (in order):
//! 1. **The world pair** (`default` VP/FP) through the EXACT app path —
//!    `translate_program_with_plan` (the WGSL the world batch draws
//!    with), VS as-is, FS after `renumber_group(_, 1)` (the fragment
//!    stage moves to group 1 before module creation — see
//!    `wgpu_backend::create_render_pipeline_with_plans` step 2).
//! 2. **Every committed SM 3.0 fixture** (the 9 `dll_*.sm30` extracts in
//!    prototype-bin/tests/fixtures) — same as-is / renumbered treatment.
//! 3. **The ENTIRE 45-program DLL shader library** when the bulk game
//!    binary is present (`/home/z/my-project/source/ADM/prototypeenginef.dll`,
//!    git-ignored; the test skips silently without it, like every
//!    bulk-data test in this workspace).  All 19 VS + 26 PS programs the
//!    DLL constructor registers are parsed from `PROGRAM_LIBRARY`'s
//!    proven blob offsets and compile-checked.
//!
//! Validation configuration mirrors wgpu's module-level checks as
//! closely as a device-free check can: `ValidationFlags::all()` and
//! `Capabilities::all()` (permissive on capability bits a specific
//! device might lack, strict on every structural/type/entry-point rule —
//! the class of bug we care about).  If a module passes here, wgpu's
//! `create_shader_module` accepts it on any device whose capabilities
//! cover the module's features (ours use only plain textures/samplers/
//! uniform buffers — covered by every backend).
//!
//! This test is UNCONDITIONAL (not behind `render-wgpu`): the WGSL
//! compile check belongs in the standard `cargo test --workspace` gate
//! so CI catches translator regressions even without the wgpu feature.

use naga::front::wgsl::parse_str;
use naga::valid::{Capabilities, ValidationFlags, Validator};
use prototype_render::shader::library::{ProgramRow, PROGRAM_LIBRARY};
use prototype_render::shader::sm30::{scan_blobs, Sm30Shader};
use prototype_render::shader::wgsl::translate_program_with_plan;
use prototype_render::shader::world::WorldPairPlan;
use prototype_render::shader::plan::renumber_group;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// A committed fixture: `crates/prototype-bin/tests/fixtures/<name>`.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("prototype-bin")
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// The bulk game binary, when the sandbox has it (git-ignored, never
/// committed).  Block 30 PROVEN: the 45-program blob cluster sits at
/// 0x00e34b08..0x00e45250 (file offsets == VA - image base there).
const BULK_DLL: &str = "/home/z/my-project/source/ADM/prototypeenginef.dll";

/// Numbered source listing — naga errors reference spans; make them
/// findable in a panic message without a file system round-trip.
fn numbered(text: &str) -> String {
    let mut out = String::new();
    for (i, line) in text.lines().enumerate() {
        out.push_str(&format!("{:>4} | {line}\n", i + 1));
    }
    out
}

/// The real compile check: naga parse + full validation of one WGSL
/// module.  Panics with the numbered source + naga's error chain on any
/// failure (this is a test helper — panicking IS the report).
fn assert_naga_compiles(label: &str, text: &str) {
    match parse_str(text) {
        Ok(module) => {
            let mut validator = Validator::new(ValidationFlags::all(), Capabilities::all());
            if let Err(e) = validator.validate(&module) {
                panic!(
                    "{label}: naga VALIDATION failed (parse ok):\n{e}\n--- source ---\n{}",
                    numbered(text)
                );
            }
        }
        Err(e) => {
            panic!(
                "{label}: naga PARSE failed:\n{e}\n--- source ---\n{}",
                numbered(text)
            );
        }
    }
}

/// Stage from the fixture filename convention (`*_vp_*` → vertex,
/// `*_fp_*` → fragment) — same convention the extraction scripts use.
fn is_fragment_from_name(name: &str) -> bool {
    if name.contains("_fp") || name.contains("_ps_") {
        true
    } else if name.contains("_vp") || name.contains("_vs_") {
        false
    } else {
        panic!("fixture {name} does not follow the dll_*_fp/dll_*_vp or dll_ps_/dll_vs_ naming convention");
    }
}

/// Translate one decoded program and naga-compile it with the exact
/// GPU-edge treatment (FS text renumbered to bind group 1 first).
fn translate_and_check(prog: &Sm30Shader, name: &str) {
    let (text, _plan) = translate_program_with_plan(prog, name)
        .unwrap_or_else(|e| panic!("{name}: translate: {e}"));
    if prog.is_fragment {
        // Mirror create_render_pipeline_with_plans: the fragment text is
        // renumbered to group 1 BEFORE create_shader_module sees it.
        let g1 = renumber_group(&text, 1);
        assert_naga_compiles(&format!("{name} [fs, group-1-renumbered]"), &g1);
    } else {
        assert_naga_compiles(&format!("{name} [vs, as-is]"), &text);
    }
}

// ---------------------------------------------------------------------------
// 1. the world pair — the exact WGSL the world batch draws with
// ---------------------------------------------------------------------------

#[test]
fn world_pair_default_wgsl_compiles_under_naga() {
    let vp = std::fs::read(fixture("dll_default_vp_e34e60.sm30"))
        .expect("world VP fixture (committed)");
    let fp = std::fs::read(fixture("dll_default_fp_e3b308.sm30"))
        .expect("world FP fixture (committed)");

    // The app path (block 34): plan from the blobs, WGSL from
    // translate_program_with_plan (the with_plan variant IS what
    // create_render_pipeline_with_plans is fed).
    let _plan = WorldPairPlan::from_blobs(&vp, &fp)
        .expect("WorldPairPlan::from_blobs on the committed default pair");
    let vs = Sm30Shader::parse_program(&vp, false).expect("world VP decode");
    let fs = Sm30Shader::parse_program(&fp, true).expect("world FP decode");

    let (vs_wgsl, vs_res) = translate_program_with_plan(&vs, "default_vp")
        .expect("world VP translate");
    let (fs_wgsl, fs_res) = translate_program_with_plan(&fs, "default_fp")
        .expect("world FP translate");

    // The plan capture must exist (the GPU edge builds bind group
    // layouts from exactly these rows) — a cheap structural assertion
    // alongside the compile check.
    assert!(
        !vs_res.bindings.is_empty(),
        "world VP plan capture produced no bindings"
    );
    assert!(
        !fs_res.bindings.is_empty(),
        "world FP plan capture produced no bindings"
    );

    assert_naga_compiles("world default_vp [as-is]", &vs_wgsl);
    let fs_g1 = renumber_group(&fs_wgsl, 1);
    assert_naga_compiles("world default_fp [group-1-renumbered]", &fs_g1);

    // The renumber must actually change the group indices — otherwise
    // the "mirror what the GPU sees" claim above is hollow.
    assert!(
        fs_g1 != fs_wgsl || !fs_wgsl.contains("@group(0)"),
        "renumber_group(_, 1) left the fragment text unchanged while it still declares group 0"
    );
}

// ---------------------------------------------------------------------------
// 2. every committed SM 3.0 fixture
// ---------------------------------------------------------------------------

#[test]
fn all_committed_sm30_fixtures_compile_under_naga() {
    // Block 98: the FULL engine-DLL SM3.0 library is committed — 45 unique
    // validated blobs (walk + CTAB + target proven by
    // scripts/m98_dll_sm30_catalog.py over prototypeenginef.dll, 91 version
    // tokens -> 45 accepted, 0 duplicates, 0 unknown opcodes).  The corpus
    // is enumerated DYNAMICALLY: every dll_*.sm30 fixture in the fixture
    // dir, content-digest-deduped (FNV-1a; the 9 block-33 role-named files
    // duplicate the offset-named set by content), every blob parsed
    // fail-closed and translated to WGSL + naga-validated.
    let dir = fixture(""); // .../tests/fixtures/
    let mut blobs: std::collections::BTreeMap<String, (String, Vec<u8>)> = Default::default();
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("fixture dir readable") {
        let entry = entry.expect("fixture dir entry");
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("dll_") && name.ends_with(".sm30") {
            let bytes = std::fs::read(entry.path())
                .unwrap_or_else(|e| panic!("committed fixture {name} unreadable: {e}"));
            let digest = {
                // stable content digest for dedupe (FNV-1a 64-bit, hex)
                let mut h: u64 = 0xcbf29ce484222325;
                for b in &bytes {
                    h ^= *b as u64;
                    h = h.wrapping_mul(0x100000001b3);
                }
                format!("{h:016x}")
            };
            let frag = is_fragment_from_name(&name);
            blobs.entry(digest).or_insert((name.clone(), bytes));
            names.push((name, frag));
        }
    }
    assert!(
        names.len() >= 45,
        "expected at least 45 dll_*.sm30 fixtures (54 files = 45 unique + 9 role-named dups), got {}",
        names.len()
    );
    assert_eq!(
        blobs.len(),
        45,
        "expected exactly the 45 unique DLL SM3.0 blobs after MD5 dedupe, got {}",
        blobs.len()
    );
    // every ROLE-NAMED fixture the render wiring references by name must exist
    for role in [
        "dll_bink_fp_e3adf8.sm30",
        "dll_default_fp_e3b308.sm30",
        "dll_default_vp_e34e60.sm30",
        "dll_error_vp_e36010.sm30",
        "dll_lightcount_vp_e37238.sm30",
        "dll_pointsprite_fp_e41cf0.sm30",
        "dll_shadowblur_fp_e3f858.sm30",
        "dll_shadowdebug_fp_e405f0.sm30",
        "dll_unlit_fp_e41670.sm30",
    ] {
        assert!(
            names.iter().any(|(n, _)| n == role),
            "role-named fixture {role} missing"
        );
    }
    let mut checked = 0;
    for (name, bytes) in blobs.values() {
        let frag = is_fragment_from_name(name);
        let prog = Sm30Shader::parse_program(bytes, frag)
            .unwrap_or_else(|e| panic!("{name}: decode: {e}"));
        translate_and_check(&prog, name);
        checked += 1;
    }
    assert_eq!(checked, 45, "expected exactly the 45 unique sm30 DLL blobs after dedupe");
}

// ---------------------------------------------------------------------------
// 2b. every shipped P3D/RCF shader program (the M10-era corpus)
// ---------------------------------------------------------------------------

#[test]
fn all_shipped_p3d_programs_compile_under_naga() {
    use prototype_p3d::P3dFile;

    let mut programs: Vec<(String, bool, Vec<u8>)> = Vec::new();
    let fs_data = std::fs::read(fixture("frontendshaders.p3d")).expect("fixture");
    let f = P3dFile::parse(&fs_data).expect("frontendshaders.p3d parses");
    for p in f.shader_programs().expect("shader_programs") {
        programs.push((p.name.clone(), p.is_fragment, p.d3d9_tokens.to_vec()));
    }
    let rcf_data = std::fs::read(fixture("shaders.rcf")).expect("fixture");
    let rcf = prototype_rcf::RcfArchive::parse(&rcf_data).expect("shaders.rcf parses");
    for e in &rcf.entries {
        let payload = rcf.payload(e).expect("rcf payload");
        if payload.starts_with(b"P3D\xff") {
            let f = P3dFile::parse(payload).expect("rcf-embedded p3d parses");
            for p in f.shader_programs().expect("shader_programs") {
                programs.push((p.name.clone(), p.is_fragment, p.d3d9_tokens.to_vec()));
            }
        }
    }
    assert_eq!(
        programs.len(),
        26,
        "the shipped corpus is 26 programs (PROVEN, M10 census)"
    );
    for (name, is_fragment, tokens) in &programs {
        let prog = Sm30Shader::parse(tokens, *is_fragment)
            .unwrap_or_else(|e| panic!("shipped program '{name}': decode: {e}"));
        let (text, _) = translate_program_with_plan(&prog, name)
            .unwrap_or_else(|e| panic!("shipped program '{name}': translate: {e}"));
        if *is_fragment {
            let g1 = renumber_group(&text, 1);
            assert_naga_compiles(&format!("shipped:{name} [fs]"), &g1);
        } else {
            assert_naga_compiles(&format!("shipped:{name} [vs]"), &text);
        }
    }
}

// ---------------------------------------------------------------------------
// 3. the entire 45-program DLL library (bulk-gated)
// ---------------------------------------------------------------------------

#[test]
fn full_dll_shader_library_compiles_under_naga() {
    let dll = match std::fs::read(BULK_DLL) {
        Ok(b) => b,
        Err(_) => {
            // Bulk data absent (fresh sandbox / CI): the committed-fixture
            // test above still covers the world pair + 7 named programs.
            eprintln!(
                "skipped: bulk DLL not present at {BULK_DLL} (bulk game data is git-ignored)"
            );
            return;
        }
    };

    // Blob extent table: every version-token..END blob in the binary.
    let blobs = scan_blobs(&dll);

    let mut checked = 0usize;
    let mut rows: Vec<&ProgramRow> = PROGRAM_LIBRARY.iter().collect();
    rows.sort_by_key(|r| r.blob_offset);
    for row in rows {
        let blob = blobs
            .iter()
            .find(|b| b.offset == row.blob_offset as usize)
            .unwrap_or_else(|| {
                panic!(
                    "PROGRAM_LIBRARY row '{}' offset 0x{:x} has no scanned blob",
                    row.name, row.blob_offset
                )
            });
        let bytes = blob.slice(&dll);
        let frag = matches!(row.stage, prototype_render::shader::library::Stage::Ps);
        let prog = Sm30Shader::parse_program(bytes, frag)
            .unwrap_or_else(|e| panic!("DLL program '{}' (0x{:x}): decode: {e}", row.name, row.blob_offset));
        translate_and_check(&prog, &format!("dll:{}", row.name));
        checked += 1;
    }
    assert_eq!(
        checked, 45,
        "the DLL constructor registers exactly 45 programs (PROVEN, block 30)"
    );
}

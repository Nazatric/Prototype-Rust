//! boot — the DEVICE-FREE engine boot sequence: from a scanned
//! [`GameFileSet`] to decoded, translated, cross-checked shader pairs
//! that the GPU edge (the feature-gated `activity` glue) turns into real
//! wgpu pipelines.
//!
//! # What "boot" means here
//!
//! The original engine mounts the RCF archives in the canonical PC order
//! (PROVEN — see `game_files::CANONICAL_PC_ARCHIVES`) and then reads P3D
//! chunks through that filesystem view.  This module mirrors the first
//! leg of that path for the shader subset the render loop actually
//! needs at boot:
//!
//! 1. [`plan`] — the mount plan (archive order + loose files), pure data;
//! 2. [`collect_program_sources`] — walk the archives' P3D members and
//!    the loose `*shader*.p3d` files in mount order, dedupe keep-first
//!    (canonical order wins);
//! 3. [`pair_programs`] — pair `"X - Vertex Program"` with
//!    `"X - Fragment Program"` by base name;
//! 4. [`prepare_programs`] — decode each stage to
//!    [`Sm30Shader`], translate to WGSL, and cross-check the binding
//!    plan against the emitted text (EXACTLY what
//!    `prototype_render::wgpu_backend::create_render_pipeline` will
//!    re-verify on the device — a pair that fails here never reaches
//!    the GPU edge).
//!
//! # Failure policy
//!
//! Boot must tolerate a partial/junk asset folder: every per-file and
//! per-program failure lands in [`BootReport::problems`] (skipped, with
//! the reason) instead of aborting.  An empty report + non-empty
//! problems is a bootable-but-degraded state the activity logs loudly.
//!
//! # Evidence
//!
//! * PROVEN: canonical mount order (block-10 audit, `game_files` docs).
//! * PROVEN: shader programs live in P3D ShaderProgram chunks reachable
//!   both loose (`frontendshaders.p3d`) and inside RCF members whose
//!   payload starts with `P3D\xff` (`shaders.rcf`) — fixtures prove both.
//! * LIKELY: walk order archives-then-loose and keep-first dedup (the
//!   original's exact precedence between a mounted archive member and a
//!   loose file of the same name is UNKNOWN; canonical-first is the
//!   conservative choice).
//! * DESIGN: pairing by the `" - Vertex/Fragment Program"` suffix — the
//!   suffix convention is PROVEN (every shipped program carries one);
//!   pairing *by base name* is the mechanical consequence.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use prototype_p3d::P3dFile;
use prototype_rcf::RcfArchive;
use prototype_render::shader::{
    renumber_group, translate_program, Sm30Shader, StageResources, validate_plan_matches_wgsl,
};

use crate::game_files::GameFileSet;

/// P3D file magic (little-endian `0xFF443350`).
const P3D_MAGIC: &[u8] = b"P3D\xff";

/// The P3D ShaderProgram chunk-name suffix marking a vertex program.
pub const VERTEX_SUFFIX: &str = " - Vertex Program";
/// The P3D ShaderProgram chunk-name suffix marking a fragment program.
pub const FRAGMENT_SUFFIX: &str = " - Fragment Program";

/// Split a ShaderProgram chunk name into `(base name, is_fragment)`.
///
/// `None` when the name carries no stage suffix (no shipped program does;
/// if one ever appears it is reported unpaired rather than guessed).
pub fn stage_of(name: &str) -> Option<(&str, bool)> {
    if let Some(base) = name.strip_suffix(FRAGMENT_SUFFIX) {
        Some((base, true))
    } else if let Some(base) = name.strip_suffix(VERTEX_SUFFIX) {
        Some((base, false))
    } else {
        None
    }
}

/// One shader program found in the user's files (owned copy of the P3D
/// chunk payload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShaderProgramSource {
    /// The full chunk name, e.g. `"cxformtexture - lit - Fragment Program"`.
    pub name: String,
    /// `true` for fragment (pixel) programs.
    pub is_fragment: bool,
    /// The D3D9 SM 3.0 token stream.
    pub d3d9_tokens: Vec<u8>,
}

/// The mount plan for one scanned asset folder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BootPlan {
    /// RCF archives in mount order (canonical five first, then unknown).
    pub mount_order: Vec<PathBuf>,
    /// Loose P3D files (top level).
    pub loose_p3d: Vec<PathBuf>,
    /// Loose Lua scripts (top level).
    pub lua_scripts: Vec<PathBuf>,
}

/// Derive the mount plan from a scan.
pub fn plan(set: &GameFileSet) -> BootPlan {
    BootPlan {
        mount_order: set.rcf_archives.clone(),
        loose_p3d: set.p3d_files.clone(),
        lua_scripts: set.lua_files.clone(),
    }
}

/// One decode + translate + cross-checked shader pair, ready for
/// `create_render_pipeline` on the GPU edge.
#[derive(Debug, Clone)]
pub struct PreparedPair {
    /// The base pair name (suffix stripped), e.g. `"cxformtexture - lit"`.
    pub name: String,
    /// Decoded vertex stage.
    pub vs: Sm30Shader,
    /// Decoded fragment stage.
    pub fs: Sm30Shader,
    /// Translated vertex WGSL (group 0 — `create_render_pipeline` keeps it there).
    pub vs_wgsl: String,
    /// Translated fragment WGSL (group 0 — `create_render_pipeline`
    /// renumbers it to group 1 itself; we validate against the SAME
    /// renumbered text it will compile).
    pub fs_wgsl: String,
}

/// The full boot report: everything found, everything paired, everything
/// that went wrong (with reasons) — the honest boot summary.
#[derive(Debug, Clone, Default)]
pub struct BootReport {
    /// Deduped program sources in discovery order (canonical first).
    pub sources: Vec<ShaderProgramSource>,
    /// Complete, decoded, validated pairs, sorted by name.
    pub pairs: Vec<PreparedPair>,
    /// Base names that had only one stage (or no stage suffix) — logged,
    /// not fatal.
    pub unpaired: Vec<String>,
    /// Per-file / per-program failures with reasons (skipped, not fatal).
    pub problems: Vec<String>,
    /// The DLL `default` WORLD-pair plan (block 34, NEXT-UP 7(e)) —
    /// present when the user's `prototypeenginef.dll` was found and the
    /// pair planned cleanly.  This is the pipeline world geometry is
    /// MEANT to draw through (the frontend pairs stay for their own
    /// passes); `None` keeps the boot on the frontend fallback with the
    /// reason in `problems`.
    pub world: Option<prototype_render::shader::world::WorldPairPlan>,
}

impl BootReport {
    /// True when the boot produced nothing usable.
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }
}

/// One line for logcat: the whole boot state in ~100 chars.
pub fn boot_summary(r: &BootReport) -> String {
    format!(
        "boot: {} program source(s), {} pipeline pair(s), {} unpaired, {} problem(s), world {}",
        r.sources.len(),
        r.pairs.len(),
        r.unpaired.len(),
        r.problems.len(),
        if r.world.is_some() { "READY" } else { "unavailable" }
    )
}

/// Walk one P3D byte buffer and push its shader programs into `out`.
fn collect_from_p3d(data: &[u8], label: &str, out: &mut Vec<ShaderProgramSource>, problems: &mut Vec<String>) {
    match P3dFile::parse(data) {
        Ok(f) => match f.shader_programs() {
            Ok(programs) => {
                for p in programs {
                    out.push(ShaderProgramSource {
                        name: p.name.clone(),
                        is_fragment: p.is_fragment,
                        d3d9_tokens: p.d3d9_tokens.to_vec(),
                    });
                }
            }
            Err(e) => problems.push(format!("{label}: shader_programs: {e}")),
        },
        Err(e) => problems.push(format!("{label}: p3d parse: {e}")),
    }
}

/// Walk the scanned set in mount order and collect every shader program
/// source, deduped keep-first.
///
/// Order: canonical RCF archives first (in `mount_order`), then loose
/// `*shader*.p3d` files (alphabetical — `scan_folder` sorts them).  A
/// member/file that fails to parse becomes a problem entry; boot goes on.
pub fn collect_program_sources(set: &GameFileSet) -> BootReport {
    let mut report = BootReport::default();

    // 1. RCF archives in mount order; P3D-magic members are parsed as
    //    P3D containers (PROVEN: shaders.rcf's member is exactly that).
    //    The directory is hash-only (PROVEN — no member names in the
    //    format), so problem labels use the name hash.
    for path in &set.rcf_archives {
        let label = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                report.problems.push(format!("{label}: read: {e}"));
                continue;
            }
        };
        let arc = match RcfArchive::parse(&bytes) {
            Ok(a) => a,
            Err(e) => {
                report.problems.push(format!("{label}: rcf parse: {e}"));
                continue;
            }
        };
        for entry in &arc.entries {
            let member = format!("{:08X}", entry.name_hash);
            match arc.payload(entry) {
                Ok(payload) if payload.starts_with(P3D_MAGIC) => {
                    collect_from_p3d(
                        payload,
                        &format!("{label}/{member}"),
                        &mut report.sources,
                        &mut report.problems,
                    );
                }
                Ok(_) => {} // non-P3D member (textures, lua, ...) — not our business here
                Err(e) => report.problems.push(format!("{label}: member payload: {e}")),
            }
        }
    }

    // 2. Loose shader P3Ds (the filename filter mirrors the engine's
    //    known frontend layout; other loose p3ds are art/cells data that
    //    M21b will decode — parsing them here would be wasted boot time).
    for path in &set.p3d_files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if !name.contains("shader") {
            continue;
        }
        let label = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        match std::fs::read(path) {
            Ok(bytes) => {
                collect_from_p3d(&bytes, &label, &mut report.sources, &mut report.problems)
            }
            Err(e) => report.problems.push(format!("{label}: read: {e}")),
        }
    }

    // Dedup keep-first: canonical archive discoveries precede loose-file
    // ones, so a duplicate loose copy never shadows the archive original.
    let mut seen: BTreeSet<(String, bool)> = BTreeSet::new();
    report.sources.retain(|src| seen.insert((src.name.clone(), src.is_fragment)));
    report
}

/// Pair collected sources by base name.
///
/// Returns `(pairs, unpaired)`: a pair is a base name with exactly one
/// vertex and one fragment source (post-dedup guarantee); anything else
/// is reported unpaired with the reason baked into the string.
pub fn pair_programs(
    sources: &[ShaderProgramSource],
) -> (Vec<(String, ShaderProgramSource, ShaderProgramSource)>, Vec<String>) {
    let mut by_base: BTreeMap<&str, (Option<&ShaderProgramSource>, Option<&ShaderProgramSource>)> =
        BTreeMap::new();
    let mut suffixless: Vec<String> = Vec::new();
    for src in sources {
        match stage_of(&src.name) {
            Some((base, true)) => by_base.entry(base).or_default().1 = Some(src),
            Some((base, false)) => by_base.entry(base).or_default().0 = Some(src),
            None => suffixless.push(src.name.clone()),
        }
    }
    let mut pairs = Vec::new();
    let mut unpaired = suffixless;
    for (base, (vs, fs)) in by_base {
        match (vs, fs) {
            (Some(vs), Some(fs)) => pairs.push((base.to_owned(), vs.clone(), fs.clone())),
            (Some(_), None) => unpaired.push(format!("{base} (no fragment program)")),
            (None, Some(_)) => unpaired.push(format!("{base} (no vertex program)")),
            (None, None) => unreachable!("entry created only on insert"),
        }
    }
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    (pairs, unpaired)
}

/// The full device-free boot: collect, pair, decode, translate,
/// cross-check — then plan the DLL world pair when the engine DLL is
/// present.  Every failure is recorded, none is fatal.
pub fn prepare_programs(set: &GameFileSet) -> BootReport {
    let mut report = collect_program_sources(set);
    let (pairs, mut unpaired) = pair_programs(&report.sources);
    report.unpaired = std::mem::take(&mut unpaired);

    for (base, vs_src, fs_src) in pairs {
        let pair = match build_pair(&base, &vs_src, &fs_src) {
            Ok(p) => p,
            Err(problems) => {
                report.problems.extend(problems);
                continue;
            }
        };
        report.pairs.push(pair);
    }

    // Block 34 (NEXT-UP 7(e)): the real WORLD pipeline.  The loader
    // ships no game data; the DLL comes from the user's own game folder
    // (`GameFileSet.dll`).  A missing DLL is NORMAL for a bare boot
    // (no problem entry); a present-but-unplannable DLL is one.
    match &set.dll {
        None => {}
        Some(path) => match std::fs::read(path) {
            Err(e) => report.problems.push(format!(
                "world pair: read {}: {e}",
                path.display()
            )),
            Ok(bytes) => {
                match prototype_render::shader::world::WorldPairPlan::from_dll(&bytes) {
                    Ok(plan) => report.world = Some(plan),
                    Err(e) => report
                        .problems
                        .push(format!("world pair: {} : {e}", path.display())),
                }
            }
        },
    }
    report
}

/// Decode + translate + cross-check one pair.  Returns the problems on
/// failure (the pair is skipped).
fn build_pair(
    base: &str,
    vs_src: &ShaderProgramSource,
    fs_src: &ShaderProgramSource,
) -> Result<PreparedPair, Vec<String>> {
    let mut problems = Vec::new();

    let vs = match Sm30Shader::parse(&vs_src.d3d9_tokens, false) {
        Ok(s) => s,
        Err(e) => {
            problems.push(format!("{base}: vertex decode: {e}"));
            return Err(problems);
        }
    };
    let fs = match Sm30Shader::parse(&fs_src.d3d9_tokens, true) {
        Ok(s) => s,
        Err(e) => {
            problems.push(format!("{base}: fragment decode: {e}"));
            return Err(problems);
        }
    };
    let vs_wgsl = match translate_program(&vs, &vs_src.name) {
        Ok(w) => w,
        Err(e) => {
            problems.push(format!("{base}: vertex translate: {e}"));
            return Err(problems);
        }
    };
    let fs_wgsl = match translate_program(&fs, &fs_src.name) {
        Ok(w) => w,
        Err(e) => {
            problems.push(format!("{base}: fragment translate: {e}"));
            return Err(problems);
        }
    };

    // Cross-check EXACTLY what the GPU edge will compile: vs as-is, fs
    // renumbered to group 1 (create_render_pipeline renumbers before
    // validation — mirror that here so a boot-validated pair cannot
    // fail validation again on-device).
    if let Err(e) = check(&vs, &vs_wgsl) {
        problems.push(format!("{base}: vertex plan: {e}"));
    }
    let fs_g1 = renumber_group(&fs_wgsl, 1);
    if let Err(e) = check(&fs, &fs_g1) {
        problems.push(format!("{base}: fragment plan: {e}"));
    }
    if !problems.is_empty() {
        return Err(problems);
    }

    Ok(PreparedPair {
        name: base.to_owned(),
        vs,
        fs,
        vs_wgsl,
        fs_wgsl,
    })
}

fn check(stage: &Sm30Shader, wgsl: &str) -> Result<(), String> {
    let res: StageResources = prototype_render::shader::plan_stage_resources(stage);
    validate_plan_matches_wgsl(&res, wgsl).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_files::scan_folder;
    use std::path::Path;

    fn fixtures_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .to_path_buf()
    }

    /// A temp asset folder containing the real shader fixtures.
    fn shader_folder(tag: &str) -> (tempdir::TempDir, GameFileSet) {
        let dir = tempdir::TempDir::new(tag);
        std::fs::copy(fixtures_dir().join("shaders.rcf"), dir.path.join("shaders.rcf")).unwrap();
        std::fs::copy(
            fixtures_dir().join("frontendshaders.p3d"),
            dir.path.join("frontendshaders.p3d"),
        )
        .unwrap();
        let set = scan_folder(&dir.path).unwrap();
        (dir, set)
    }

    /// Minimal temp-dir helper (the workspace stays dependency-free).
    mod tempdir {
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicU32, Ordering};

        static NEXT: AtomicU32 = AtomicU32::new(0);

        pub struct TempDir {
            pub path: PathBuf,
        }

        impl TempDir {
            pub fn new(tag: &str) -> Self {
                let n = NEXT.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "prototype-android-{tag}-{}-{n}",
                    std::process::id()
                ));
                let _ = std::fs::remove_dir_all(&path);
                std::fs::create_dir_all(&path).unwrap();
                Self { path }
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }
    }

    #[test]
    fn stage_of_strips_suffixes() {
        assert_eq!(
            stage_of("CxformGauraud - lit - Vertex Program"),
            Some(("CxformGauraud - lit", false))
        );
        assert_eq!(
            stage_of("cxformtexture - Fragment Program"),
            Some(("cxformtexture", true))
        );
        assert_eq!(stage_of("no suffix here"), None);
        assert_eq!(stage_of(""), None);
        // Only the EXACT suffix counts.
        assert_eq!(stage_of("X - Vertex ProgramX"), None);
    }

    #[test]
    fn plan_mirrors_scan_order() {
        let dir = tempdir::TempDir::new("plan");
        for name in ["02art.rcf", "00cells.rcf", "zzz.rcf", "aaa.rcf"] {
            std::fs::write(dir.path.join(name), b"").unwrap();
        }
        std::fs::write(dir.path.join("some.p3d"), b"").unwrap();
        std::fs::write(dir.path.join("main.lua"), b"").unwrap();
        let set = scan_folder(&dir.path).unwrap();
        let p = plan(&set);
        let names: Vec<String> = p
            .mount_order
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        // Canonical first (by rank, not alphabet), then unknown alphabetical.
        assert_eq!(names, vec!["00cells.rcf", "02art.rcf", "aaa.rcf", "zzz.rcf"]);
        assert_eq!(p.loose_p3d.len(), 1);
        assert_eq!(p.lua_scripts.len(), 1);
    }

    #[test]
    fn prepare_from_real_fixtures_is_complete() {
        // THE boot-path integration test: real archives, real programs,
        // full decode + translate + cross-check, zero problems.
        let (_dir, set) = shader_folder("boot-real");
        let report = prepare_programs(&set);
        assert!(
            report.problems.is_empty(),
            "unexpected problems: {:?}",
            report.problems
        );
        assert!(report.sources.len() >= 26, "expected the full shipped set, got {}", report.sources.len());
        assert!(report.pairs.len() >= 13, "expected the shipped pairs, got {}", report.pairs.len());
        assert!(report.unpaired.is_empty(), "unexpected unpaired: {:?}", report.unpaired);
        // Pairs are sorted and complete.
        let names: Vec<&str> = report.pairs.iter().map(|p| p.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
        // Spot-check two known pairs, verifying stage correctness.
        let cx = report
            .pairs
            .iter()
            .find(|p| p.name == "cxformtexture - lit")
            .expect("cxformtexture - lit pair");
        assert!(!cx.vs.is_fragment);
        assert!(cx.fs.is_fragment);
        assert!(cx.vs_wgsl.contains("@vertex"));
        assert!(cx.fs_wgsl.contains("@fragment"));
        // The fs WGSL stored is GROUP 0 (the GPU edge renumbers) — but it
        // must validate against group 1 exactly like create_render_pipeline.
        let fs_res = prototype_render::shader::plan_stage_resources(&cx.fs);
        validate_plan_matches_wgsl(&fs_res, &renumber_group(&cx.fs_wgsl, 1)).unwrap();
        let gauraud = report
            .pairs
            .iter()
            .find(|p| p.name == "CxformGauraud - lit")
            .expect("CxformGauraud - lit pair");
        // Block 34: the vertex INPUT position carries the dedicated
        // location 32 (it used to collide with texcoord0's @location(0)).
        assert!(gauraud.vs_wgsl.contains("@location(32) position0"), "vertex stage really is the vs");
    }

    #[test]
    fn sources_are_deduped_keep_first() {
        // The same programs reachable twice (archive member + loose file
        // copy under a different name) must dedupe to one copy.  The two
        // fixture sets are disjoint (probe-verified: bink - lit lives in
        // shaders.rcf, the 12 cxform/hud pairs in frontendshaders.p3d);
        // the expected total is computed from the fixtures themselves so
        // the test cannot lie.  Every folder is scanned while it is alive.
        let count_in = |tag: &str, files: &[(&str, &str)]| -> usize {
            let d = tempdir::TempDir::new(tag);
            for (src, dst) in files {
                std::fs::copy(fixtures_dir().join(src), d.path.join(dst)).unwrap();
            }
            let set = scan_folder(&d.path).unwrap();
            collect_program_sources(&set).sources.len()
        };
        let rcf_only = count_in("boot-dedup-rcf", &[("shaders.rcf", "shaders.rcf")]);
        let loose_only = count_in(
            "boot-dedup-loose",
            &[("frontendshaders.p3d", "zz-loose-shaders.p3d")],
        );
        let both = count_in(
            "boot-dedup-both",
            &[
                ("shaders.rcf", "shaders.rcf"),
                ("frontendshaders.p3d", "zz-loose-shaders.p3d"),
            ],
        );
        // Disjoint fixtures: no dedup needed, plain addition holds — and
        // the combined count equals the real-fixture integration count.
        let overlap = overlap_count();
        assert_eq!(both, rcf_only + loose_only - overlap);
        assert_eq!(rcf_only, 2, "bink - lit vs+fs (probe-verified)");
        assert_eq!(loose_only, 24, "12 cxform/hud pairs (probe-verified)");
    }

    /// How many programs appear in BOTH fixtures (computed for real from
    /// the fixture bytes, so this test stays honest if the fixtures grow).
    fn overlap_count() -> usize {
        let rcf_names = program_names(&std::fs::read(fixtures_dir().join("shaders.rcf")).unwrap());
        let loose_names =
            program_names(&std::fs::read(fixtures_dir().join("frontendshaders.p3d")).unwrap());
        rcf_names.intersection(&loose_names).count()
    }

    fn program_names(data: &[u8]) -> BTreeSet<(String, bool)> {
        if data.starts_with(b"P3D\xff") {
            if let Ok(f) = P3dFile::parse(data) {
                if let Ok(ps) = f.shader_programs() {
                    return ps.into_iter().map(|p| (p.name, p.is_fragment)).collect();
                }
            }
        }
        if let Ok(arc) = RcfArchive::parse(data) {
            let mut out = BTreeSet::new();
            for e in &arc.entries {
                if let Ok(p) = arc.payload(e) {
                    if p.starts_with(b"P3D\xff") {
                        if let Ok(f) = P3dFile::parse(p) {
                            if let Ok(ps) = f.shader_programs() {
                                out.extend(ps.into_iter().map(|p| (p.name, p.is_fragment)));
                            }
                        }
                    }
                }
            }
            return out;
        }
        BTreeSet::new()
    }

    #[test]
    fn pair_programs_reports_incomplete_stages() {
        let mk = |name: &str| ShaderProgramSource {
            name: name.to_owned(),
            is_fragment: name.ends_with("Fragment Program"),
            d3d9_tokens: Vec::new(),
        };
        let sources = vec![
            mk("a - Vertex Program"),
            mk("a - Fragment Program"),
            mk("b - Vertex Program"),             // no fragment
            mk("c - Fragment Program"),           // no vertex
            mk("nameless"),                        // no suffix
        ];
        let (pairs, unpaired) = pair_programs(&sources);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "a");
        assert_eq!(unpaired, vec![
            "nameless".to_string(),
            "b (no fragment program)".to_string(),
            "c (no vertex program)".to_string(),
        ]);
    }

    #[test]
    fn corrupt_archive_degrades_but_does_not_kill_boot() {
        let dir = tempdir::TempDir::new("boot-corrupt");
        std::fs::write(dir.path.join("00cells.rcf"), b"NOT AN RCF AT ALL").unwrap();
        std::fs::copy(fixtures_dir().join("shaders.rcf"), dir.path.join("shaders.rcf")).unwrap();
        let set = scan_folder(&dir.path).unwrap();
        let report = prepare_programs(&set);
        assert!(
            report.problems.iter().any(|p| p.contains("00cells.rcf")),
            "junk archive must be reported: {:?}",
            report.problems
        );
        assert!(report.pairs.len() >= 1, "the good archive still boots (bink - lit)");
    }

    #[test]
    fn empty_folder_boots_to_empty_report() {
        let dir = tempdir::TempDir::new("boot-empty");
        std::fs::write(dir.path.join("00cells.rcf"), b"").unwrap();
        let set = scan_folder(&dir.path).unwrap();
        let report = prepare_programs(&set);
        assert!(report.is_empty());
        assert!(!report.problems.is_empty(), "an empty .rcf is a problem entry");
        let s = boot_summary(&report);
        assert!(s.contains("0 pipeline pair(s)"), "{s}");
    }

    #[test]
    fn boot_summary_counts_everything() {
        let (_dir, set) = shader_folder("boot-summary");
        let report = prepare_programs(&set);
        let s = boot_summary(&report);
        assert!(s.contains(&format!("{} program source(s)", report.sources.len())), "{s}");
        assert!(s.contains(&format!("{} pipeline pair(s)", report.pairs.len())), "{s}");
    }
}

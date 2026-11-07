//! prototype-rs — CLI front-end for the prototype-rs workspace.
//!
//! Inspects every supported file format reverse-engineered from the
//! Prototype (2009) PC binaries, plus prints the known boot-handshake
//! milestones from the Phase 2D / 2F / 2C analysis:
//!
//!   * `.p3d` — Pure3D chunk container (v2 parser: full chunk walk)
//!   * `.rcf` — Radical ATG CORE CEMENT LIBRARY archive
//!   * `.lua` — compiled Lua 5.1 bytecode
//!   * `.exe` / `.dll` / `.sls` — PE32 binaries
//!   * `boot-status` — print the known boot handshake stages
//!   * `lua-api [<group>|<name>]` — the recovered Lua host-function registry
//!   * `lua-api --live <dll>` — re-sweep a real engine DLL and diff
//!     against the static table
//!   * `run-lua <file.lua>` — execute a recovered script in the safe
//!     Rust Lua 5.1 VM (engine natives become record-only stubs
//!     validated against the M7 registry)
//!   * `list-audio <file.p3d>` — catalog the AudioFile clips in a
//!     P3D (name, RADP version, sample rate, block count)
//!   * `decode-audio <file.p3d> <clip-name|all> <out.wav|dir>` —
//!     decode RADP (PROVEN IMA ADPCM) to 16-bit WAV
//!   * `play-audio <file.p3d> <clip-name>` — play a clip through
//!     the REAL device edge (CPAL; needs `--features audio-cpal`)
//!
//! Run with `cargo run -p prototype-bin -- <path>` to auto-detect the
//! format from magic bytes and print a structured summary.

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "usage: {} <file> [<file> ...]\n\
             \n\
             Inspects Prototype (2009) file formats:\n  \
             .p3d  Pure3D chunk container\n  \
             .rcf  Radical ATG CORE CEMENT LIBRARY archive\n  \
             .lua  compiled Lua 5.1 bytecode\n  \
             .exe/.dll/.sls  PE32 binaries\n\
             \n\
             Or:\n  \
             {} boot-status\n      print the known boot handshake stages and milestones\n  \
             {} lua-api [<group>|<name>]\n      print the recovered Lua host-function registry (use --live <dll> to re-sweep)\n  \
             {} list-audio <file.p3d>\n      catalog the AudioFile clips in a P3D\n  \
             {} list-missions <scripts.rcf>\n      resolve the mission chunk STRUCTURALLY (the Lua51 member carrying the engine's AddValidMission marker) and print the mission table\n  \
             {} decode-audio <file.p3d> <clip-name|all> <out.wav|dir>\n      decode RADP audio (PROVEN IMA ADPCM) to 16-bit WAV\n  \
             {} play-audio <file.p3d> <clip-name>\n      play a clip through the real device edge (feature audio-cpal)\n  \
             {} bik-info <file.bik>\n      parse a Bink movie container (block-46 layout; reads the head only)\n  \
             {} list-movies <movies.rcf>\n      census the movie archive: cracked member names, Bink headers, subtitle tables\n  \
             {} audio-rcf <archive.rcf> [--list|--nis|--banks|--extract <name> <out>]\n      serve a name-table audio archive BY NAME (the release 00audio/01audio.rcf)\n  \
             {} bank-info <bank.p3d>\n      decode a Cue/Patch audio bank (music graphs, stem maps, patch descriptors)",
            args[0], args[0], args[0], args[0], args[0], args[0], args[0], args[0], args[0], args[0], args[0]
        );
        std::process::exit(2);
    }
    if args.iter().any(|a| a == "boot-status") {
        print_boot_status();
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "run-lua") {
        match args.get(pos + 1) {
            Some(path) => run_lua(path),
            None => {
                eprintln!("usage: prototype-rs run-lua <file.lua>");
                std::process::exit(2);
            }
        }
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "fre-events") {
        match args.get(pos + 1) {
            Some(path) => fre_events(path),
            None => {
                eprintln!("usage: prototype-rs fre-events <frevents.lua>");
                std::process::exit(2);
            }
        }
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "list-missions") {
        match args.get(pos + 1) {
            Some(path) => list_missions(path),
            None => {
                eprintln!("usage: prototype-rs list-missions <scripts.rcf>");
                std::process::exit(2);
            }
        }
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "list-missions") {
        match args.get(pos + 1) {
            Some(path) => list_missions(path),
            None => {
                eprintln!("usage: prototype-rs list-missions <scripts.rcf>");
                std::process::exit(2);
            }
        }
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "list-missions") {
        match args.get(pos + 1) {
            Some(path) => list_missions(path),
            None => {
                eprintln!("usage: prototype-rs list-missions <scripts.rcf>");
                std::process::exit(2);
            }
        }
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "lua-api") {
        print_lua_api(args.get(pos + 1).map(String::as_str), args.get(pos + 2).map(String::as_str));
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "list-audio") {
        match args.get(pos + 1) {
            Some(path) => list_audio(path),
            None => {
                eprintln!("usage: prototype-rs list-audio <file.p3d>");
                std::process::exit(2);
            }
        }
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "decode-audio") {
        match (args.get(pos + 1), args.get(pos + 2), args.get(pos + 3)) {
            (Some(p3d), Some(clip), Some(out)) => decode_audio(p3d, clip, out),
            _ => {
                eprintln!("usage: prototype-rs decode-audio <file.p3d> <clip-name|all> <out.wav|dir>");
                std::process::exit(2);
            }
        }
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "play-audio") {
        play_audio_dispatch(args.get(pos + 1), args.get(pos + 2));
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "bik-info") {
        match args.get(pos + 1) {
            Some(path) => bik_info(path),
            None => {
                eprintln!("usage: prototype-rs bik-info <file.bik>");
                std::process::exit(2);
            }
        }
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "bik-decode") {
        match (args.get(pos + 1), args.get(pos + 2)) {
            (Some(path), Some(out)) => bik_decode(path, out),
            _ => {
                eprintln!("usage: prototype-rs bik-decode <file.bik> <out.yuv>");
                std::process::exit(2);
            }
        }
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "bik-audio") {
        match (args.get(pos + 1), args.get(pos + 2), args.get(pos + 3)) {
            (Some(path), Some(track), Some(out)) => bik_audio(path, track, out),
            _ => {
                eprintln!("usage: prototype-rs bik-audio <file.bik> <track> <out.f32>");
                std::process::exit(2);
            }
        }
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "bik-play") {
        let rest: Vec<&String> = args[pos + 1..].iter().collect();
        bik_play(&rest);
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "list-movies") {
        match args.get(pos + 1) {
            Some(path) => list_movies(path),
            None => {
                eprintln!("usage: prototype-rs list-movies <movies.rcf>");
                std::process::exit(2);
            }
        }
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "audio-rcf") {
        let rest: Vec<&String> = args[pos + 1..].iter().collect();
        audio_rcf(&rest);
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "bank-info") {
        match args.get(pos + 1) {
            Some(path) => bank_info(path),
            None => {
                eprintln!("usage: prototype-rs bank-info <bank.p3d>");
                std::process::exit(2);
            }
        }
        return;
    }
    let mut had_error = false;
    for path in args.iter().skip(1) {
        let pb = PathBuf::from(path);
        match std::fs::read(&pb) {
            Ok(bytes) => match inspect(&bytes) {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("{}: {e}", pb.display());
                    had_error = true;
                }
            },
            Err(e) => {
                eprintln!("{}: {e}", pb.display());
                had_error = true;
            }
        }
    }
    if had_error {
        std::process::exit(1);
    }
}

/// `run-lua <file>` — execute a recovered script in the safe VM.
fn run_lua(path: &str) {
    use prototype_engine::script_host;
    use prototype_lua::Lua51File;

    let data = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let f = Lua51File::parse(&data).unwrap_or_else(|e| {
        eprintln!("{path}: Lua parse error: {e}");
        std::process::exit(1);
    });
    println!("== classify (imports vs base library / script globals / M7 registry) ==");
    let cls = script_host::classify_script(&f);
    println!("imports  : {:?}", cls.imports);
    println!("exports  : {:?}", cls.exports);
    println!("registry natives: {:?}", cls.engine_hosts);
    if !cls.unresolved_imports.is_empty() {
        println!("UNRESOLVED imports (not base, not script-defined, not in registry):");
        for u in &cls.unresolved_imports {
            println!("  ! {u}");
        }
    }
    println!("== run ==");
    match script_host::run_script(&f) {
        Ok(report) => {
            let mut engine_calls = 0usize;
            let mut by_name: std::collections::BTreeMap<&str, usize> =
                Default::default();
            for c in &report.host_calls {
                *by_name.entry(c.name.as_str()).or_default() += 1;
                if cls.engine_hosts.iter().any(|h| h == &c.name) {
                    engine_calls += 1;
                }
            }
            println!("host calls:");
            for (name, n) in &by_name {
                println!("  {name:32} x{n}");
            }
            println!("engine-native calls: {engine_calls}");
            println!("closure exports: {:?}", report.closure_exports);
            // Show the first few engine-native calls with args.
            println!("== first engine-native calls ==");
            let display_arg = |a: &prototype_lua::vm::Value| -> String {
                match a {
                    prototype_lua::vm::Value::Table(t) => {
                        let name = t
                            .borrow()
                            .get(&prototype_lua::vm::Value::Str(
                                std::rc::Rc::from(b"name".as_slice()),
                            ))
                            .map(|v| v.display())
                            .unwrap_or_default();
                        if name.is_empty() {
                            a.display()
                        } else {
                            format!("table{{name={name}}}")
                        }
                    }
                    _ => a.display(),
                }
            };
            let mut shown = 0;
            for c in &report.host_calls {
                if cls.engine_hosts.iter().any(|h| h == &c.name) && shown < 10 {
                    let args: Vec<String> =
                        c.args.iter().map(display_arg).collect();
                    println!("  {}({})", c.name, args.join(", "));
                    shown += 1;
                }
            }
        }
        Err(e) => {
            eprintln!("runtime error: {e}");
            std::process::exit(1);
        }
    }
}

/// `list-missions <scripts.rcf>` — the block-39 structural mission
/// load path, end to end: resolve the mission chunk by its
/// `AddValidMission` marker (prototype_engine::missions_load), run the
/// compiled Lua through the real VM registration, print the typed
/// mission table with zones.
fn list_missions(path: &str) {
    use prototype_engine::missions_load::find_mission_members;
    use prototype_engine::runtime::run_mission_script;
    use prototype_lua::Lua51File;
    use prototype_rcf::RcfArchive;

    let data = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let arc = RcfArchive::parse(&data).unwrap_or_else(|e| {
        eprintln!("{path}: not a readable RCF archive: {e}");
        std::process::exit(1);
    });
    let members = find_mission_members(&arc);
    if members.len() != 1 {
        eprintln!(
            "{path}: {} mission-chunk candidates (fail-closed — expected exactly 1)",
            members.len()
        );
        std::process::exit(1);
    }
    let entry = &arc.entries[members[0]];
    let payload = arc.payload(entry).unwrap_or_else(|e| {
        eprintln!("{path}: entry payload error: {e}");
        std::process::exit(1);
    });
    let lua = Lua51File::parse(payload).unwrap_or_else(|e| {
        eprintln!("{path}: mission chunk Lua parse error: {e}");
        std::process::exit(1);
    });
    let sys = run_mission_script(&lua).unwrap_or_else(|e| {
        eprintln!("{path}: mission registration failed: {e}");
        std::process::exit(1);
    });
    println!(
        "{path}: {} mission(s) via the structural load path (member hash 0x{:08x}, {} bytes)",
        sys.missions.len(),
        entry.name_hash,
        entry.file_size
    );
    let zones = sys.zones();
    println!("  zones: {}", zones.join(", "));
    for z in &zones {
        println!("  {} ({}):", z, sys.in_district(z).len());
        for m in sys.in_district(z) {
            println!("    {:<30} {:?}", m.name, m.state);
        }
    }
}

/// `fre-events <file>` — run frevents.lua through the typed engine
/// runtime (M9) and dump the free-roam event system.
fn fre_events(path: &str) {
    use prototype_engine::runtime::{
        CollectiblesDef, EventState, FreClass, ScriptedFreeRoam,
    };
    use prototype_lua::Lua51File;

    let data = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let f = Lua51File::parse(&data).unwrap_or_else(|e| {
        eprintln!("{path}: Lua parse error: {e}");
        std::process::exit(1);
    });
    let mut fre = ScriptedFreeRoam::new(&f).unwrap_or_else(|e| {
        eprintln!("runtime error: {e}");
        std::process::exit(1);
    });

    println!("== free-roam event system (typed, from the real script) ==");
    {
        let sys = fre.system();
        println!("events: {}", sys.len());
        let classes = [
            FreClass::Kill,
            FreClass::ThreeWayWar,
            FreClass::Loco,
            FreClass::Glide,
            FreClass::Opp,
            FreClass::Coll,
        ];
        for c in &classes {
            println!("  {:<12} {}", c.as_str(), sys.class_count(c));
        }
        if let Some(c @ CollectiblesDef { .. }) = sys.collectibles() {
            println!(
                "collectibles: maxHint={} maxLocation={} numScientists={}",
                c.max_hint_collectibles,
                c.max_location_collectibles,
                c.num_scientist_events
            );
        }
        println!("first event:");
        let e0 = &sys.events()[0];
        println!("  {} ({})", e0.name(), e0.class().as_str());
        for v in &e0.def.vars {
            println!(
                "    var {} isFloat={:?} default={:?} numElements={:?}",
                v.name, v.is_float, v.default, v.num_elements
            );
        }
        let last = &sys.events()[sys.len() - 1];
        println!("last event:");
        println!(
            "  {} ({}) backgroundType={:?} vars={}",
            last.name(),
            last.class().as_str(),
            last.def.background_type,
            last.def.vars.len()
        );
        for v in &last.def.vars {
            println!("    var {} default={:?}", v.name, v.default);
        }
    }

    println!("== lifecycle via the script's own closures ==");
    let n = fre.reset_all_via_script().unwrap();
    println!("ResetFreeRoamEvents(): {n} events reset");
    let states: std::collections::BTreeMap<&str, usize> = {
        let sys = fre.system();
        let mut m = std::collections::BTreeMap::new();
        for e in sys.events() {
            let s = match e.state {
                EventState::Registered => "Registered",
                EventState::Reset => "Reset",
                EventState::Destroyed => "Destroyed",
            };
            *m.entry(s).or_default() += 1;
        }
        m
    };
    for (s, n) in &states {
        println!("  {s}: {n}");
    }
    let n = fre.destroy_all_via_script().unwrap();
    println!("DestroyFreeRoamEvents(): {n} events destroyed");
    println!(
        "live events after destroy: {}",
        fre.system().live_events().count()
    );
}

fn print_lua_api(arg: Option<&str>, arg2: Option<&str>) {
    use prototype_engine::lua_api::{self, Mechanism};

    // `--live <dll>`: run the live byte-pattern sweep on a real binary
    // and diff it against the static recovered table.
    if arg == Some("--live") {
        let Some(path) = arg2 else {
            eprintln!("usage: prototype-rs lua-api --live <prototypeenginef.dll>");
            std::process::exit(2);
        };
        let data = std::fs::read(path).unwrap_or_else(|e| {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        });
        let pe = prototype_pe::PeFile::parse(&data)
            .unwrap_or_else(|e| {
                eprintln!("{path}: pe parse error: {e}");
                std::process::exit(1);
            });
        let live = lua_api::extract_host_fns(&pe).unwrap_or_else(|e| {
            eprintln!("{path}: sweep error: {e}");
            std::process::exit(1);
        });
        println!(
            "live sweep of {path}: {} Lua-facing registrations",
            live.len()
        );
        let reg = prototype_engine::LuaApiRegistry::new();
        let mut mismatches = 0usize;
        for (l, s) in live.iter().zip(reg.all()) {
            if l.name != s.name
                || l.fn_va != s.fn_va
                || l.thunk_target != s.thunk_target
                || l.registrar_va != s.registrar_va
                || l.mechanism != s.mechanism
            {
                mismatches += 1;
                if mismatches <= 5 {
                    eprintln!(
                        "  MISMATCH live={l:?} static={s:?}"
                    );
                }
            }
        }
        if live.len() != reg.registration_count() || mismatches != 0 {
            println!(
                "  DIFFERS from static table ({} records): {mismatches} mismatches",
                reg.registration_count()
            );
            std::process::exit(1);
        }
        println!(
            "  matches static table exactly ({} records) — table is reproducible from the binary",
            reg.registration_count()
        );
        return;
    }

    let reg = prototype_engine::LuaApiRegistry::new();
    let bn = reg.by_mechanism(Mechanism::BindingNode).count();
    let nm = reg.by_mechanism(Mechanism::NamedMap).count();
    println!("=== Prototype (2009) Lua host-function registry ===");
    println!(
        "recovered from prototypeenginef.dll: {} registrations, {} unique names",
        reg.registration_count(),
        reg.unique_name_count()
    );
    println!("  BindingNode (radLua-style binding objects): {bn}");
    println!("  NamedMap    (per-subsystem binding classes): {nm}");
    println!();
    println!("Subsystems (prefix -> registrations):");
    for (p, c) in reg.prefix_counts() {
        println!("  {:<16} {c}", p);
    }
    println!();
    println!("Script-facing anchors (cross-checked with real script data):");
    for hook in [
        "fre_RegisterEvent",
        "fre_RegisterCollectibles",
        "AddValidMission",
        "util_GetRandomFloat",
    ] {
        match reg.lookup_unique(hook) {
            Some(r) => println!(
                "  {:<26} fn={:#010x} registrar={:#010x} {:?}",
                hook, r.fn_va, r.registrar_va, r.mechanism
            ),
            None => println!("  {hook:<26} MISSING"),
        }
    }

    if let Some(a) = arg {
        println!();
        if let Some(recs) = reg.lookup(a) {
            println!("{a}:");
            for r in recs {
                println!(
                    "  fn={:#010x} thunk={:?} registrar={:#010x} {:?}",
                    r.fn_va, r.thunk_target, r.registrar_va, r.mechanism
                );
            }
        } else {
            let group: Vec<_> = reg.group(a).collect();
            if group.is_empty() {
                println!("no registry entry or group named {a:?}");
            } else {
                println!("group {a:?} ({} registrations):", group.len());
                for r in group {
                    println!("  {:<48} fn={:#010x}", r.name, r.fn_va);
                }
            }
        }
    }
}

fn print_boot_status() {
    use prototype_boot::*;
    println!("=== Prototype (2009) Boot Handshake Reference ===");
    println!();
    println!("Boot stages (in order):");
    for (i, s) in BootStage::ALL.iter().enumerate() {
        println!("  {:>2}. {:?}", i, s);
    }
    println!();
    println!("Engine service IDs (from prototypef.exe .rdata):");
    for v in [
        EngineServiceId::Initialize,
        EngineServiceId::WindowFocus,
        EngineServiceId::MouseService,
        EngineServiceId::KeyboardService,
        EngineServiceId::Service,
        EngineServiceId::Terminate,
    ] {
        println!("  - {}", v.as_str());
    }
    println!();
    println!("Cement library initialization gates (5 — all must pass):");
    for g in CementLibraryGate::ALL {
        let desc = match g {
            CementLibraryGate::XexCheckExecutablePrivilege =>
                "XEX header system flags = 0x600 (privileges 11 & 23 -> 0)",
            CementLibraryGate::RawHddNamespace =>
                "\\\\Device\\\\Harddisk0\\\\partition0 (NullDevice territory, 2 MiB persistent)",
            CementLibraryGate::XMountUtilityDrive =>
                "ioctls 0x70000 (geometry) + 0x74004 (partition info)",
            CementLibraryGate::FileFsSizeInformation =>
                "{31 units, 31 avail, 64 sectors/unit, 512 bps} = 0x8000 cluster",
            CementLibraryGate::XamTaskSchedule =>
                "spawns real GuestThread (0x40000 stack) for each callback",
        };
        println!("  - {:<32} {}", format!("{:?}", g), desc);
    }
    println!();
    println!("RCF mount order (XEX strings at 0x8202FB9C..0x8202FBE4):");
    for r in RcfMountOrder::ALL {
        println!("  - {}", r.game_path());
    }
    println!();
    println!("Startup package (XEX string at 0x82033C90):");
    println!("  - package name: {}", StartupPackageFile::PACKAGE_NAME);
    println!("  - packed path: {}", StartupPackageFile::PACKED_PATH);
    println!();
    println!("Boot-critical material (XEX string at 0x820024B0):");
    println!("  - {}", UNLIT_MATERIAL_NAME);
    println!();
    println!("Historical milestones:");
    println!("  Phase 2C stable:  {:?}, {} PM4 pkts, {} draws, {} vblanks",
        PHASE2C_STABLE_CHECKPOINT.current_stage,
        PHASE2C_STABLE_CHECKPOINT.pm4_packets,
        PHASE2C_STABLE_CHECKPOINT.draws,
        PHASE2C_STABLE_CHECKPOINT.vblanks);
    println!("  Phase 2D gate:    {:?}, {} PM4 pkts, {} draws, {} shader loads",
        PHASE2D_CONTENT_GATE.current_stage,
        PHASE2D_CONTENT_GATE.pm4_packets,
        PHASE2D_CONTENT_GATE.draws,
        PHASE2D_CONTENT_GATE.shader_loads);
    println!("  Phase 2F init:    {:?}, all 5 cement gates passed",
        PHASE2F_ARCHIVE_INIT.current_stage);
}

/// Inspect one file, auto-detecting the format from its magic bytes.
fn inspect(data: &[u8]) -> Result<(), String> {
    if data.len() >= 4 && &data[..4] == prototype_common::P3D_MAGIC {
        inspect_p3d(data)
    } else if data.len() >= 32 && data[..24].starts_with(prototype_common::RCF_MAGIC) {
        inspect_rcf(data)
    } else if data.len() >= 4 && &data[..4] == prototype_common::LUA_MAGIC {
        inspect_lua(data)
    } else if data.len() >= 2 && &data[..2] == prototype_common::MZ_MAGIC {
        inspect_pe(data)
    } else {
        Err(format!(
            "unknown file format (first 8 bytes = {})",
            data.iter()
                .take(8)
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(" ")
        ))
    }
}

fn inspect_p3d(data: &[u8]) -> Result<(), String> {
    use prototype_p3d::{ChunkFamily, P3dFile};
    let f = P3dFile::parse(data).map_err(|e| format!("p3d parse error: {e}"))?;
    println!(
        "P3D  v{}  file_size={}  chunks={}",
        f.header.version,
        f.header.file_size,
        f.chunks.len()
    );
    for c in f.chunks.iter().take(20) {
        match c.family {
            ChunkFamily::Classic => {
                if let Ok(Some(cc)) = c.classic() {
                    let names = cc.name_list().unwrap_or_default();
                    let extra = if names.is_empty() {
                        String::new()
                    } else {
                        format!("  names={names:?}")
                    };
                    println!(
                        "  [{:#010x}] classic uid={:#06x} id={} {:?}{}",
                        c.chunk_type, c.uid, cc.id, cc.name, extra
                    );
                    continue;
                }
                println!("  [{:#010x}] classic uid={:#06x} (decode failed)", c.chunk_type, c.uid);
            }
            ChunkFamily::Content => {
                if let Ok(Some(content)) = c.content() {
                    let meta = content
                        .meta
                        .as_ref()
                        .map(|m| format!("  META(count={})", m.count))
                        .unwrap_or_default();
                    println!(
                        "  [{:#010x}] content uid={:#06x} {:?}{}",
                        c.chunk_type, c.uid, content.names.first(), meta
                    );
                    continue;
                }
                println!("  [{:#010x}] content uid={:#06x} (decode failed)", c.chunk_type, c.uid);
            }
            ChunkFamily::Shader => {
                let name = c.shader_name().ok().flatten().unwrap_or_default();
                println!("  [{:#010x}] shader uid={:#06x} {:?}", c.chunk_type, c.uid, name);
            }
            ChunkFamily::Other(t) => {
                println!("  [{:#010x}] other   uid={:#06x} size={}", t, c.uid, c.size);
            }
        }
    }
    if f.chunks.len() > 20 {
        println!("  ... and {} more chunks", f.chunks.len() - 20);
    }
    // Audio summary for audio bundles
    if let Ok(audio) = f.audio_names() {
        if !audio.is_empty() {
            println!("  audio entries: {}", audio.len());
        }
    }
    Ok(())
}

fn inspect_rcf(data: &[u8]) -> Result<(), String> {
    let arc = prototype_rcf::RcfArchive::parse(data)
        .map_err(|e| format!("rcf parse error: {e}"))?;
    println!(
        "RCF  {}  v0x{:08x}  sector_size={}  entries={}",
        arc.header.magic, arc.header.version_field, arc.header.sector_size, arc.header.entry_count
    );
    for e in &arc.entries {
        let kind = arc.payload_kind(e).unwrap_or(prototype_rcf::PayloadKind::Unknown);
        // Resolve the hash against names cracked from the real archives.
        let resolved = prototype_rcf::KNOWN_ENTRY_NAMES
            .iter()
            .copied()
            .find(|n| prototype_rcf::hash_name(n) == e.name_hash);
        let name = resolved
            .map(|n| format!(" name={n:?}"))
            .unwrap_or_else(|| " name=<unresolved>".to_string());
        println!(
            "  hash=0x{:08x}  offset={:>8}  size={:>8}  kind={:?}{}",
            e.name_hash, e.file_offset, e.file_size, kind, name
        );
    }
    Ok(())
}

fn inspect_lua(data: &[u8]) -> Result<(), String> {
    let f = prototype_lua::Lua51File::parse(data)
        .map_err(|e| format!("lua parse error: {e}"))?;
    println!(
        "Lua  v0x{:02x}  format={}  endian={}  sizeof(int)={} sizeof(size_t)={} sizeof(Instr)={} sizeof(Number)={}  integral={}",
        f.header.version,
        f.header.format,
        f.header.endian,
        f.header.sizeof_int,
        f.header.sizeof_size_t,
        f.header.sizeof_instruction,
        f.header.sizeof_lua_number,
        f.header.integral_flag
    );
    println!(
        "  main proto: instructions={} constants={} nested-protos={} params={} vararg={} maxstack={}",
        f.main.code.len(),
        f.main.constants.len(),
        f.main.protos.len(),
        f.main.nparams,
        f.main.is_vararg,
        f.main.maxstacksize
    );
    println!(
        "  total instructions (incl. nested): {}  exact-consumption: {}",
        f.main.total_instructions(),
        if f.is_exact() { "yes" } else { "no" }
    );
    let mut strings = Vec::new();
    f.main.all_strings(&mut strings);
    if !strings.is_empty() {
        println!("  string constants: {}", strings.len());
        for s in strings.iter().take(12) {
            println!("    - {:?}", String::from_utf8_lossy(s));
        }
        if strings.len() > 12 {
            println!("    ... and {} more", strings.len() - 12);
        }
    }
    // Global-variable analysis (GETGLOBAL/SETGLOBAL over the code).
    let read = f.main.all_globals_read();
    let written = f.main.all_globals_written();
    if !read.is_empty() {
        let names: Vec<String> = read
            .iter()
            .map(|g| String::from_utf8_lossy(g).to_string())
            .collect();
        println!("  globals read ({} unique): {}", read.len(), names.join(", "));
    }
    if !written.is_empty() {
        let names: Vec<String> = written
            .iter()
            .map(|g| String::from_utf8_lossy(g).to_string())
            .collect();
        println!("  globals written ({} unique): {}", written.len(), names.join(", "));
    }
    Ok(())
}

fn inspect_pe(data: &[u8]) -> Result<(), String> {
    let pe = prototype_pe::PeFile::parse(data)
        .map_err(|e| format!("pe parse error: {e}"))?;
    println!(
        "PE  machine=0x{:04x}  subsystem={}  image_base=0x{:08x}  entry=0x{:08x}",
        pe.coff.machine,
        pe.optional.subsystem,
        pe.optional.image_base,
        pe.optional.address_of_entry_point
    );
    println!("  sections ({}):", pe.sections.len());
    for s in &pe.sections {
        println!(
            "    {:<10} vaddr=0x{:08x} vsize={:>8} raw_size={:>8}",
            s.name, s.virtual_address, s.virtual_size, s.size_of_raw_data
        );
    }
    if !pe.import_dlls.is_empty() {
        println!("  imports ({} DLLs):", pe.import_dlls.len());
        for d in &pe.import_dlls {
            println!("    - {}", d);
        }
    }
    Ok(())
}

/// `list-movies <movies.rcf>` — census the movie archive: the block-47
/// member-name table (7 cracked Bink + 7 cracked bible names, 12 honest
/// UNKNOWNs), the Bink container headers, and the FMV subtitle text
/// bibles.  Streaming-shaped: the RCF head+directory plus ONLY the 7
/// small bible payloads (≤ 5.4 KB each) are ever read — the 19 movie
/// payloads stay on disk.
fn list_movies(path: &str) {
    let mut f = std::fs::File::open(path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let (header, entries) =
        prototype_rcf::parse_directory(&mut f).unwrap_or_else(|e| {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        });
    println!(
        "{}: {} members (sector {})",
        path, header.entry_count, header.sector_size
    );
    for e in &entries {
        let hash = e.name_hash;
        if let Some(m) = prototype_video::bink_by_hash(hash) {
            println!(
                "  {:08x} {:>12} B  BIK  {}",
                hash,
                e.file_size,
                m.name.unwrap_or("(id unknown — not in any local corpus)")
            );
        } else if let Some(b) = prototype_video::bible_by_hash(hash) {
            let payload = prototype_rcf::read_member_payload(&mut f, e)
                .unwrap_or_else(|er| {
                    eprintln!("{path}: member {hash:08x}: {er}");
                    std::process::exit(1);
                });
            match prototype_video::FmvSubtitles::parse(&payload) {
                Ok(subs) => {
                    let en = subs.language("english");
                    let first = en
                        .and_then(|l| l.entries.first())
                        .map(|s| s.text.clone())
                        .unwrap_or_default();
                    println!(
                        "  {:08x} {:>12} B  P3D  {}  movie {}  {} langs, {} lines — {}",
                        hash,
                        e.file_size,
                        b.name,
                        subs.movie_id.as_deref().unwrap_or("?"),
                        subs.languages.len(),
                        en.map(|l| l.entries.len()).unwrap_or(0),
                        first
                    );
                }
                Err(er) => {
                    println!("  {:08x} {:>12} B  P3D  {}  BIBLE PARSE FAILED: {er}", hash, e.file_size, b.name);
                }
            }
        } else {
            println!("  {:08x} {:>12} B  ???  unknown member", hash, e.file_size);
        }
    }
}

/// `bik-info <file.bik>` — parse a Bink movie CONTAINER (block-46
/// layout, PROVEN on real movies.rcf members) and print the header,
/// audio tracks and frame census.  Streaming-shaped: only the head
/// (44 B + audio block + frame table) is ever read, never the payload
/// region — the same I/O shape the phone loader will use.
fn bik_info(path: &str) {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let real_len = f.metadata().map(|m| m.len()).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let mut head44 = [0u8; 44];
    f.read_exact(&mut head44).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let (_, head_len) = prototype_video::BikHeader::parse(&head44)
        .unwrap_or_else(|e| {
            eprintln!("{path}: not a parseable Bink file: {e}");
            std::process::exit(1);
        });
    let mut head = vec![0u8; head_len as usize];
    f.seek(SeekFrom::Start(0)).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    f.read_exact(&mut head).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let idx = prototype_video::BikIndex::parse_head(&head, real_len)
        .unwrap_or_else(|e| {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        });
    let h = &idx.header;
    println!(
        "{}: Bink {}  {}x{} @ {}/{} fps  {} frames  ~{:.2}s  {} bytes",
        path,
        String::from_utf8_lossy(&h.tag),
        h.width,
        h.height,
        h.fps_num,
        h.fps_den,
        h.frame_count,
        h.duration_secs(),
        h.file_size
    );
    println!(
        "  head: {} B (44 header + {} audio + {} table)  demux bound {} B  \
         max frame {} B",
        idx.head_len,
        12 * h.audio_track_count,
        4 * (h.frame_count + 1),
        h.largest_frame_size,
        idx.max_frame_size()
    );
    for (i, t) in idx.audio.iter().enumerate() {
        println!(
            "  audio[{i}]: id {}  {} Hz  {}  {}  max decoded {} B",
            t.stream_id,
            t.sample_rate,
            if t.is_stereo() { "stereo" } else { "mono" },
            if t.uses_dct() { "DCT" } else { "RDFT" },
            t.max_decoded_size
        );
    }
    if idx.audio.is_empty() {
        println!("  audio: none");
    }
    let kf = idx.keyframe_count();
    println!(
        "  frames: {} spans ({} keyframes), first {} B, last {} B",
        idx.frames.len(),
        kf,
        idx.frames[0].size(),
        idx.frames[idx.frames.len() - 1].size()
    );
    // block-48 demux layer: frame 0's audio/video split (reads exactly
    // one frame span — the same streaming shape a player uses).
    let f0_size = idx.frames[0].size() as usize;
    let mut f0 = vec![0u8; f0_size];
    f.seek(SeekFrom::Start(idx.frames[0].start)).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    f.read_exact(&mut f0).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let fh = idx
        .frame_head(0, &f0)
        .unwrap_or_else(|e| {
            eprintln!("{path}: frame 0 demux failed: {e}");
            std::process::exit(1);
        });
    let audio_sizes: Vec<String> = fh
        .audio
        .iter()
        .map(|a| match a.decoded_bytes {
            Some(d) => format!("{}B→{}B", a.compressed_len, d),
            None => format!("{}B", a.compressed_len),
        })
        .collect();
    if audio_sizes.is_empty() {
        println!("  frame 0: no audio packets");
    } else {
        println!("  frame 0: audio [{}] (compressed→decoded)", audio_sizes.join(", "));
    }
    println!(
        "  frame 0: skip word {}  video bitstream {} B at span+{}",
        match fh.skip_word {
            Some(w) => format!("{w:#x} (UNKNOWN, skipped like FFmpeg)"),
            None => "none (rev < i)".into(),
        },
        fh.video_len,
        fh.video_start
    );
}

/// `bik-decode <file.bik> <out.yuv>` — decode every frame's VIDEO
/// layer (block-51 pixel layer) to raw YUV420p: the visible w×h luma
/// + 2×(w/2)×(h/2) chroma per frame, concatenated — byte-identical to
/// `ffmpeg -pix_fmt yuv420p -f rawvideo` when the decoder matches.
/// Reads one frame span at a time (the streaming shape a player uses).
fn bik_decode(path: &str, out_path: &str) {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut f = std::fs::File::open(path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let real_len = f.metadata().map(|m| m.len()).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let mut head44 = [0u8; 44];
    f.read_exact(&mut head44).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let (_, head_len) = prototype_video::BikHeader::parse(&head44).unwrap_or_else(|e| {
        eprintln!("{path}: not a parseable Bink file: {e}");
        std::process::exit(1);
    });
    let mut head = vec![0u8; head_len as usize];
    f.seek(SeekFrom::Start(0)).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    f.read_exact(&mut head).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let idx = prototype_video::BikIndex::parse_head(&head, real_len).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let h = &idx.header;
    // swap_planes for revision >= 'h' (FFmpeg's decode_init); every
    // movie in this game is revision 'i'.
    let swap = h.tag[3] >= b'h';
    let mut dec = prototype_video::BinkVideoDecoder::new(h.width, h.height, swap)
        .unwrap_or_else(|e| {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        });
    let mut out = std::io::BufWriter::new(
        std::fs::File::create(out_path).unwrap_or_else(|e| {
            eprintln!("{out_path}: {e}");
            std::process::exit(1);
        }),
    );
    let mut span = Vec::new();
    for i in 0..idx.frames.len() {
        let size = idx.frames[i].size() as usize;
        span.clear();
        span.resize(size, 0);
        f.seek(SeekFrom::Start(idx.frames[i].start)).unwrap_or_else(|e| {
            eprintln!("{path}: frame {i}: {e}");
            std::process::exit(1);
        });
        f.read_exact(&mut span).unwrap_or_else(|e| {
            eprintln!("{path}: frame {i}: {e}");
            std::process::exit(1);
        });
        let fh = idx.frame_head(i, &span).unwrap_or_else(|e| {
            eprintln!("{path}: frame {i} demux failed: {e}");
            std::process::exit(1);
        });
        let video = &span[fh.video_start..];
        dec.decode_frame(video).unwrap_or_else(|e| {
            eprintln!("{path}: frame {i} decode failed: {e}");
            std::process::exit(1);
        });
        let planes = dec.last().unwrap();
        let yuv = planes.yuv420_visible().unwrap_or_else(|e| {
            eprintln!("{path}: frame {i}: {e}");
            std::process::exit(1);
        });
        out.write_all(&yuv).unwrap_or_else(|e| {
            eprintln!("{out_path}: {e}");
            std::process::exit(1);
        });
        if (i + 1) % 12 == 0 || i + 1 == idx.frames.len() {
            eprintln!("  decoded {}/{} frames", i + 1, idx.frames.len());
        }
    }
    eprintln!(
        "{path}: {} frames ({}x{}, rev {}) -> {out_path}",
        idx.frames.len(),
        h.width,
        h.height,
        h.tag[3] as char
    );
}

/// `bik-audio <file.bik> <track> <out.f32>` — decode one Bink AUDIO
/// track's packets (block-52 DCT decoder) to interleaved f32 PCM,
/// frames in order — the streaming shape a player's audio thread uses.
fn bik_audio(path: &str, track_arg: &str, out_path: &str) {
    use std::io::{Read, Seek, SeekFrom, Write};
    let track: usize = track_arg.parse().unwrap_or_else(|_| {
        eprintln!("{track_arg}: not a track index");
        std::process::exit(1);
    });
    let mut f = std::fs::File::open(path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let real_len = f.metadata().map(|m| m.len()).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let mut head44 = [0u8; 44];
    f.read_exact(&mut head44).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let (_, head_len) = prototype_video::BikHeader::parse(&head44).unwrap_or_else(|e| {
        eprintln!("{path}: not a parseable Bink file: {e}");
        std::process::exit(1);
    });
    let mut head = vec![0u8; head_len as usize];
    f.seek(SeekFrom::Start(0)).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    f.read_exact(&mut head).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let idx = prototype_video::BikIndex::parse_head(&head, real_len).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    if track >= idx.audio.len() {
        eprintln!("{path}: track {track} of {}", idx.audio.len());
        std::process::exit(1);
    }
    let t = &idx.audio[track];
    println!(
        "track {track}: {} Hz {} {} max decoded {} B",
        t.sample_rate,
        if t.is_stereo() { "stereo" } else { "mono" },
        if t.uses_dct() { "DCT" } else { "RDFT" },
        t.max_decoded_size
    );
    let mut dec = prototype_video::BinkAudioDecoder::new(
        t.sample_rate as u32,
        t.is_stereo(),
        t.uses_dct(),
    )
    .unwrap_or_else(|e| {
        eprintln!("{path}: track {track}: {e}");
        std::process::exit(1);
    });
    let mut out = std::io::BufWriter::new(
        std::fs::File::create(out_path).unwrap_or_else(|e| {
            eprintln!("{out_path}: {e}");
            std::process::exit(1);
        }),
    );
    let mut span = Vec::new();
    let mut blocks_total = 0usize;
    for i in 0..idx.frames.len() {
        let size = idx.frames[i].size() as usize;
        span.clear();
        span.resize(size, 0);
        f.seek(SeekFrom::Start(idx.frames[i].start)).unwrap_or_else(|e| {
            eprintln!("{path}: frame {i}: {e}");
            std::process::exit(1);
        });
        f.read_exact(&mut span).unwrap_or_else(|e| {
            eprintln!("{path}: frame {i}: {e}");
            std::process::exit(1);
        });
        let fh = idx.frame_head(i, &span).unwrap_or_else(|e| {
            eprintln!("{path}: frame {i} demux failed: {e}");
            std::process::exit(1);
        });
        let Some(a) = fh.audio.get(track) else { continue };
        let packet = &span[a.offset..a.offset + a.compressed_len as usize];
        let pcm = dec.decode_packet(packet).unwrap_or_else(|e| {
            eprintln!("{path}: frame {i} track {track} audio decode failed: {e}");
            std::process::exit(1);
        });
        blocks_total += pcm.len() / (dec.frame_len() - dec.frame_len() / 16) / dec.channels();
        let mut le = Vec::with_capacity(pcm.len() * 4);
        for s in &pcm {
            le.extend_from_slice(&s.to_le_bytes());
        }
        out.write_all(&le).unwrap_or_else(|e| {
            eprintln!("{out_path}: {e}");
            std::process::exit(1);
        });
    }
    eprintln!(
        "{path}: track {track} -> {out_path} ({blocks_total} blocks, {} samples/channel)",
        blocks_total * (dec.frame_len() - dec.frame_len() / 16)
    );
}

/// `bik-play <file.bik> <out-prefix> [--frames N] [--no-video]
/// [--no-audio] [--s16]` — the full PLAYER walk (block 53): one
/// `prototype_video::BinkPlayer` streams the movie, writing the
/// visible YUV420 of every decoded frame to `<prefix>.yuv`
/// (byte-identical to `bik-decode`'s output) and every audio track's
/// interleaved f32 PCM to `<prefix>.t<k>.f32` (byte-identical to
/// `bik-audio`'s output for the same movie; add `--s16` for the
/// FFmpeg-rounded i16 the device ring consumes).  This IS the shape
/// the APK player runs on-device.
fn bik_play(args: &[&String]) {
    use std::io::Write;
    if args.is_empty() {
        eprintln!(
            "usage: prototype-rs bik-play <file.bik> <out-prefix> [--frames N] [--no-video] [--no-audio] [--s16]"
        );
        std::process::exit(2);
    }
    let path = args[0].as_str();
    let prefix = args
        .get(1)
        .map(|s| s.as_str())
        .unwrap_or_else(|| {
            eprintln!("bik-play: missing <out-prefix>");
            std::process::exit(2);
        });
    let max_frames: Option<usize> = args
        .iter()
        .position(|a| a.as_str() == "--frames")
        .and_then(|p| args.get(p + 1))
        .and_then(|s| s.parse().ok());
    let no_video = args.iter().any(|a| a.as_str() == "--no-video");
    let no_audio = args.iter().any(|a| a.as_str() == "--no-audio");
    let want_s16 = args.iter().any(|a| a.as_str() == "--s16");

    let f = std::fs::File::open(path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let len = f.metadata().unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    }).len();
    let mut player = prototype_video::BinkPlayer::open(f, len).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    println!(
        "{path}: {}x{} @ {}/{} fps, {} frames, {} audio track(s), {:.2} s",
        player.width(),
        player.height(),
        player.fps().0,
        player.fps().1,
        player.frame_count(),
        player.audio_tracks().len(),
        player.duration_secs()
    );
    for t in player.audio_tracks() {
        println!(
            "  track {}: {} Hz, {}",
            t.index,
            t.sample_rate,
            if t.stereo { "stereo" } else { "mono" }
        );
    }

    let mut video_out = if no_video {
        None
    } else {
        Some(std::io::BufWriter::new(
            std::fs::File::create(format!("{prefix}.yuv")).unwrap_or_else(|e| {
                eprintln!("{prefix}.yuv: {e}");
                std::process::exit(1);
            }),
        ))
    };
    let mut audio_out: Vec<Option<std::io::BufWriter<std::fs::File>>> =
        if no_audio { Vec::new() } else {
            player
                .audio_tracks()
                .iter()
                .map(|t| {
                    Some(std::io::BufWriter::new(
                        std::fs::File::create(format!("{prefix}.t{}.f32", t.index))
                            .unwrap_or_else(|e| {
                                eprintln!("{prefix}.t{}.f32: {e}", t.index);
                                std::process::exit(1);
                            }),
                    ))
                })
                .collect()
        };
    let mut s16_out: Vec<Option<std::io::BufWriter<std::fs::File>>> = if want_s16 && !no_audio {
        player
            .audio_tracks()
            .iter()
            .map(|t| {
                Some(std::io::BufWriter::new(
                    std::fs::File::create(format!("{prefix}.t{}.s16", t.index))
                        .unwrap_or_else(|e| {
                            eprintln!("{prefix}.t{}.s16: {e}", t.index);
                            std::process::exit(1);
                        }),
                ))
            })
            .collect()
    } else {
        Vec::new()
    };

    let mut decoded = 0usize;
    let mut track_samples = vec![0u64; player.audio_tracks().len()];
    while decoded < max_frames.unwrap_or(usize::MAX) {
        let frame = match player.next_frame() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("{path}: frame {decoded} decode failed: {e}");
                std::process::exit(1);
            }
        };
        if let (Some(out), true) = (video_out.as_mut(), !no_video) {
            out.write_all(&frame.yuv420).unwrap_or_else(|e| {
                eprintln!("{prefix}.yuv: {e}");
                std::process::exit(1);
            });
        }
        for t in &frame.audio {
            track_samples[t.track] += t.samples.len() as u64;
            if let Some(out) = audio_out.get_mut(t.track).and_then(|o| o.as_mut()) {
                let bytes: Vec<u8> = t
                    .samples
                    .iter()
                    .flat_map(|s| s.to_le_bytes())
                    .collect();
                out.write_all(&bytes).unwrap_or_else(|e| {
                    eprintln!("track {} write: {e}", t.track);
                    std::process::exit(1);
                });
            }
        }
        if want_s16 {
            for t in &frame.audio {
                if let Some(out) = s16_out.get_mut(t.track).and_then(|o| o.as_mut()) {
                    let s16 = prototype_video::pcm_f32_to_s16(&t.samples);
                    let bytes: Vec<u8> =
                        s16.iter().flat_map(|s| s.to_le_bytes()).collect();
                    out.write_all(&bytes).unwrap_or_else(|e| {
                        eprintln!("track {} s16 write: {e}", t.track);
                        std::process::exit(1);
                    });
                }
            }
        }
        decoded += 1;
        if decoded % 12 == 0 {
            eprintln!("  played {decoded}/{} frames", player.frame_count());
        }
    }
    println!("  played {decoded} frames");
    for (i, n) in track_samples.iter().enumerate() {
        println!("  track {i}: {n} f32 samples decoded");
    }
}

/// `list-audio <file.p3d>` — catalog every AudioFile clip in a P3D.
fn list_audio(path: &str) {
    let data = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let file = prototype_p3d::P3dFile::parse(&data).unwrap_or_else(|e| {
        eprintln!("{path}: not a parseable P3D: {e}");
        std::process::exit(1);
    });
    let cat = prototype_audio::AudioClipCatalog::from_p3d(&file)
        .unwrap_or_else(|e| {
            eprintln!("{path}: catalog error: {e}");
            std::process::exit(1);
        });
    println!("{}: {} audio clips", path, cat.len());
    for c in cat.clips() {
        let blocks = c.payload_len() / prototype_audio::radp::BLOCK_SIZE;
        // Frame duration: v2 (stereo) SPLITS the blocks across the two
        // streams (block-38 engine arithmetic: per stream = data_size /
        // (20 · version)), so the playback length divides by `version`
        // — the old print counted both streams (2× too long for v2).
        let secs = blocks as f64 * prototype_audio::radp::SAMPLES_PER_BLOCK as f64
            / (c.sample_rate as f64 * c.radp_version as f64);
        println!(
            "  {:<34} v{} {:>5} Hz tag {:>2} {} blocks {:7} bytes ~{:.2}s",
            c.name, c.radp_version, c.sample_rate, c.format_tag.raw(),
            blocks, c.payload_len(), secs
        );
    }
}

/// `decode-audio <file.p3d> <clip-name|all> <out.wav|dir>` — decode
/// RADP (PROVEN standard IMA ADPCM, M13) to 16-bit WAV.
fn decode_audio(p3d_path: &str, clip: &str, out: &str) {
    let data = std::fs::read(p3d_path).unwrap_or_else(|e| {
        eprintln!("{p3d_path}: {e}");
        std::process::exit(1);
    });
    let file = prototype_p3d::P3dFile::parse(&data).unwrap_or_else(|e| {
        eprintln!("{p3d_path}: not a parseable P3D: {e}");
        std::process::exit(1);
    });
    let cat = prototype_audio::AudioClipCatalog::from_p3d(&file)
        .unwrap_or_else(|e| {
            eprintln!("{p3d_path}: catalog error: {e}");
            std::process::exit(1);
        });

    if clip == "all" {
        let dir = PathBuf::from(out);
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| {
            eprintln!("{out}: {e}");
            std::process::exit(1);
        });
        let mut ok = 0usize;
        let mut failed = 0usize;
        for c in cat.clips() {
            match c.decode_to_wav() {
                Ok(wav) => {
                    let p = dir.join(format!(
                        "{}_{}hz_{}.wav",
                        c.name,
                        c.sample_rate,
                        match c.radp_version {
                            2 => "stereo",
                            6 => "6ch",
                            _ => "mono",
                        }
                    ));
                    let size = wav.len();
                    std::fs::write(&p, wav).unwrap_or_else(|e| {
                        eprintln!("{}: {e}", p.display());
                        std::process::exit(1);
                    });
                    println!("  wrote {} ({} bytes)", p.display(), size);
                    ok += 1;
                }
                Err(e) => {
                    eprintln!("  {}: {e}", c.name);
                    failed += 1;
                }
            }
        }
        println!("decoded {ok} clips into {} ({failed} failed)", dir.display());
        if failed > 0 {
            std::process::exit(1);
        }
        return;
    }

    let c = cat.find(clip).unwrap_or_else(|| {
        eprintln!("{p3d_path}: no clip named {clip:?} (see list-audio)");
        std::process::exit(1);
    });
    let wav = c.decode_to_wav().unwrap_or_else(|e| {
        eprintln!("{clip}: decode failed: {e}");
        std::process::exit(1);
    });
    let audio = c.decode().expect("just decoded");
    std::fs::write(out, &wav).unwrap_or_else(|e| {
        eprintln!("{out}: {e}");
        std::process::exit(1);
    });
    println!(
        "{clip}: {} Hz, {} channel(s), {} frames, {:.2}s -> {out} ({} bytes)",
        c.sample_rate,
        audio.channels,
        audio.samples.len() / audio.channels as usize,
        audio.duration_seconds(c.sample_rate),
        wav.len()
    );
}

// ---------------------------------------------------------------------------
// play-audio (M18) — real device playback through the CPAL edge
// ---------------------------------------------------------------------------

/// Feature dispatch: without `audio-cpal`, explain how to enable it.
fn play_audio_dispatch(p3d: Option<&String>, clip: Option<&String>) {
    #[cfg(feature = "audio-cpal")]
    {
        play_audio(p3d, clip);
    }
    #[cfg(not(feature = "audio-cpal"))]
    {
        let _ = (p3d, clip);
        eprintln!(
            "play-audio needs the `audio-cpal` feature (links libasound on Linux):\
             \n  cargo build -p prototype-bin --features audio-cpal"
        );
        std::process::exit(2);
    }
}

/// `play-audio <file.p3d> <clip-name>` — decode the clip, open the
/// DEFAULT output device via the M18 CPAL edge, and play it once.
/// Prints the device config and the device-side counters (callback
/// count, underruns, stream errors, dropped mix samples).
#[cfg(feature = "audio-cpal")]
fn play_audio(p3d: Option<&String>, clip: Option<&String>) {
    use prototype_audio::cpal_out::{CpalOut, SharedRing};
    use prototype_audio::sink::RingSink;

    let (p3d_path, clip) = match (p3d, clip) {
        (Some(p), Some(c)) => (p.clone(), c.clone()),
        _ => {
            eprintln!("usage: prototype-rs play-audio <file.p3d> <clip-name>");
            std::process::exit(2);
        }
    };
    let data = std::fs::read(&p3d_path).unwrap_or_else(|e| {
        eprintln!("{p3d_path}: {e}");
        std::process::exit(1);
    });
    let file = prototype_p3d::P3dFile::parse(&data).unwrap_or_else(|e| {
        eprintln!("{p3d_path}: not a parseable P3D: {e}");
        std::process::exit(1);
    });
    let cat = prototype_audio::AudioClipCatalog::from_p3d(&file).unwrap_or_else(|e| {
        eprintln!("{p3d_path}: catalog error: {e}");
        std::process::exit(1);
    });
    let c = cat.find(&clip).unwrap_or_else(|| {
        eprintln!("{p3d_path}: no clip named {clip:?} (see list-audio)");
        std::process::exit(1);
    });
    let audio = c.decode().unwrap_or_else(|e| {
        eprintln!("{clip}: decode failed: {e}");
        std::process::exit(1);
    });

    // Open the device FIRST (its rate governs the resample target).
    let ring = SharedRing::new(RingSink::new(1 << 16));
    let out = CpalOut::open_default(ring.clone()).unwrap_or_else(|e| {
        eprintln!("audio device: {e}");
        std::process::exit(1);
    });
    let dev_rate = out.config.sample_rate;
    let dev_ch = out.config.channels as u32;
    println!(
        "device: {} Hz × {} ch ({} callbacks budget: stream live)",
        dev_rate,
        dev_ch,
        out.stats.callbacks()
    );

    // Match the clip to the device: channel count first, then rate.
    let pcm = if audio.channels == dev_ch {
        audio.samples.clone()
    } else if audio.channels == 1 && dev_ch == 2 {
        // Mono → stereo duplicate (the mixer's equal downmix inverse).
        let mut v = Vec::with_capacity(audio.samples.len() * 2);
        for &s in &audio.samples {
            v.push(s);
            v.push(s);
        }
        v
    } else if audio.channels > 2 && dev_ch == 2 {
        // Multi-stream beds/mixes (v4/v6/v18): fold onto the first two
        // outputs — the mixer's documented ch>2 behaviour (block 62).
        let frames = audio.samples.len() / audio.channels as usize;
        let mut v = Vec::with_capacity(frames * 2);
        for f in 0..frames {
            v.push(audio.samples[f * audio.channels as usize]);
            v.push(audio.samples[f * audio.channels as usize + 1]);
        }
        v
    } else {
        eprintln!(
            "clip/channel mismatch: clip {} ch vs device {} ch (not handled)",
            audio.channels, dev_ch
        );
        std::process::exit(1);
    };
    // Rate-clean at the DEVICE rate: the block-59 library converter
    // (the private CLI copy is gone — one implementation, engine and
    // CLI both; duration-preserving ceil arithmetic now).
    let pcm = prototype_audio::sample_rate::resample(&pcm, dev_ch, c.sample_rate, dev_rate);
    let frames = pcm.len() / dev_ch as usize;
    println!(
        "{clip}: {} Hz v{} tag {} → {} frames @ {} Hz × {} ch ({:.2}s)",
        c.sample_rate,
        c.radp_version,
        c.format_tag.raw(),
        frames,
        dev_rate,
        dev_ch,
        frames as f64 / dev_rate as f64
    );

    // Feed the ring from this thread in ~10 ms chunks (the game
    // thread's role; a real engine mixes incrementally).
    let ch = dev_ch as usize;
    let chunk_frames = (dev_rate as usize) / 100;
    let mut cursor = 0usize;
    while cursor < frames {
        let end = (cursor + chunk_frames).min(frames);
        // Back-pressure: if the ring is >3/4 full, yield.
        loop {
            let r = ring.lock();
            if r.buffered() <= r.capacity() * 3 / 4 {
                break;
            }
            drop(r);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        ring.lock()
            .write(&pcm[cursor * ch..end * ch]);
        cursor = end;
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
    // Drain: wait for the device to consume what is left.
    for _ in 0..2000 {
        if ring.lock().buffered() == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    println!(
        "played {clip}: device callbacks {}, underruns {}, stream errors {}, mix-side dropped {}",
        out.stats.callbacks(),
        out.stats.underruns(),
        out.stats.errors(),
        ring.lock().dropped
    );
    if out.stats.errors() > 0 {
        std::process::exit(1);
    }
}

/// `audio-rcf <archive.rcf> [--info] [--list] [--nis] [--banks]
///              [--extract <name> <out>]`
///
/// Serve the game's audio archives BY NAME (block 62).  The two
/// release audio archives (`00audio.rcf`, `01audio.rcf`) carry a NAME
/// TABLE (block 61) — this command opens the user's own archive
/// through [`prototype_rcf::named::NamedArchive`] and serves members
/// by archive path, never loading more than the queried member's
/// span.
///
/// * `--info` — the roster summary (default).
/// * `--list` — every member: name, hash, span.  `--nis` restricts
///   to the NIS mix files, `--banks` to the top-level bank P3Ds.
/// * `--extract <name> <out>` — write one member's payload
///   (RZ-decompressed when wrapped) to `out`.
fn audio_rcf(rest: &[&String]) {
    if rest.is_empty() {
        eprintln!(
            "usage: prototype-rs audio-rcf <archive.rcf> [--info] [--list] [--nis] [--banks]\n\
             \x20                                      [--extract <name> <out>]"
        );
        std::process::exit(2);
    }
    let path = rest[0];
    let flags: Vec<&str> = rest[1..].iter().map(|s| s.as_str()).collect();
    let file = std::fs::File::open(path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let mut named = prototype_rcf::named::NamedArchive::open(file).unwrap_or_else(|e| {
        eprintln!("{path}: not a named cement archive: {e}");
        eprintln!("(only archives WITH a name table qualify — the two release audio archives do; art/cells/movies are hash-keyed)");
        std::process::exit(1);
    });

    if let Some(x) = flags.iter().position(|f| *f == "--extract") {
        let (name, out) = match (flags.get(x + 1), flags.get(x + 2)) {
            (Some(n), Some(o)) => (n.to_string(), PathBuf::from(o)),
            _ => {
                eprintln!("usage: prototype-rs audio-rcf <archive.rcf> --extract <name> <out>");
                std::process::exit(2);
            }
        };
        let bytes = named
            .read_member(&name)
            .or_else(|_| named.read_member_normalized(&name))
            .unwrap_or_else(|e| {
                eprintln!("{path}: {name}: {e}");
                std::process::exit(1);
            });
        std::fs::write(&out, &bytes).unwrap_or_else(|e| {
            eprintln!("{}: {e}", out.display());
            std::process::exit(1);
        });
        println!(
            "extracted {name} ({} B) -> {}",
            bytes.len(),
            out.display()
        );
        return;
    }

    // summary (default) — mirrors the block-61 census shapes
    let names: Vec<&str> = named.names().collect();
    let banks: Vec<&&str> = names.iter().filter(|n| !n.contains("english\\")).collect();
    let nis: Vec<&&str> = names
        .iter()
        .filter(|n| {
            let leaf = n.rsplit('\\').next().unwrap();
            leaf.contains("nis") && leaf.contains("_mix")
        })
        .collect();
    let payload: u64 = named
        .entries
        .iter()
        .map(|e| e.file_size as u64)
        .sum();
    println!(
        "{path}: {} named members, payload {} B, {} top-level banks, {} NIS mix file(s)",
        named.len(),
        payload,
        banks.len(),
        nis.len()
    );

    if flags.iter().any(|f| *f == "--list" || *f == "--nis" || *f == "--banks") {
        let want_nis = flags.iter().any(|f| *f == "--nis");
        let want_banks = flags.iter().any(|f| *f == "--banks");
        let both = !want_nis && !want_banks;
        for name in &names {
            let leaf = name.rsplit('\\').next().unwrap();
            let is_nis = leaf.contains("nis") && leaf.contains("_mix");
            let is_bank = !name.contains("english\\");
            if (want_nis && !is_nis) || (want_banks && !is_bank) {
                continue;
            }
            if both || (want_nis && is_nis) || (want_banks && is_bank) {
                let m = named.lookup(name).unwrap();
                println!(
                    "  {name:<58} hash 0x{:08X} off {:>10} size {:>9}",
                    m.dir_hash, m.file_offset, m.file_size
                );
            }
        }
    }
}

/// `bank-info <bank.p3d>` — decode every `Cue`/`Patch` chunk of an audio
/// bank with the block-65 PROVEN grammar and print the music graph /
/// descriptor content (mission stem maps, clip references, volumes).
fn bank_info(path: &str) {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        }
    };
    let file = match prototype_p3d::P3dFile::parse(&bytes) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        }
    };
    let mut cues = 0usize;
    let mut patches = 0usize;
    for name in [prototype_p3d::bank::CUE_CHUNK_NAME, prototype_p3d::bank::PATCH_CHUNK_NAME] {
        for chunk in file.classic_named(name) {
            let classic = match chunk.classic() {
                Ok(Some(c)) => c,
                _ => continue,
            };
            let parsed = match prototype_p3d::bank::parse_body(classic.data) {
                Ok(p) => p,
                Err(e) => {
                    println!("{}[uid {:#010x}]: UNDECODABLE: {e}", classic.name, chunk.uid);
                    continue;
                }
            };
            match parsed {
                prototype_p3d::bank::BankChunk::Cue(c) => {
                    cues += 1;
                    println!("Cue {} radius={} transitions={} cells={}", c.object, c.radius, c.transitions.len(), c.cells.len());
                    println!("  transitions: {}", c.transitions.join(", "));
                    for (t, cell) in c.transitions.iter().zip(c.cells.iter()) {
                        match (&cell.stem, cell.is_empty) {
                            (Some(stem), _) => println!("  {t:<16} -> {} ({})", stem, if cell.is_empty { "empty" } else { "MusicMGraphCell" }),
                            (None, true) => println!("  {t:<16} -> <EmptyCell>"),
                            (None, false) => println!("  {t:<16} -> <MusicMGraphCell, no stem>"),
                        }
                    }
                }
                prototype_p3d::bank::BankChunk::Patch(p) => {
                    patches += 1;
                    println!("Patch {} class={} props={}", p.object, p.class, p.props.len());
                    for prop in &p.props {
                        match &prop.value {
                            prototype_p3d::bank::BankValue::Bool(b) => {
                                println!("  {} = {}", prop.name, b);
                            }
                            prototype_p3d::bank::BankValue::Float32(v) => {
                                println!("  {} = {v}", prop.name);
                            }
                            prototype_p3d::bank::BankValue::Str(s) => {
                                println!("  {} = {s:?}", prop.name);
                            }
                            prototype_p3d::bank::BankValue::StrList(v) => {
                                println!("  {} = [{}]", prop.name, v.join(", "));
                            }
                            prototype_p3d::bank::BankValue::Envelope(pts) => {
                                let pts: Vec<String> =
                                    pts.iter().map(|(t, v)| format!("({t},{v})")).collect();
                                println!("  {} = envelope[{}]", prop.name, pts.join(" "));
                            }
                            prototype_p3d::bank::BankValue::Vector(v) => {
                                println!("  {} = vector[{}]", prop.name, v.iter().map(|f| f.to_string()).collect::<Vec<_>>().join(", "));
                            }
                        }
                    }
                }
            }
        }
    }
    println!("--- {path}: {cues} cues, {patches} patches decoded");
}

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
//!   * `bik-frames <file.bik> <out-dir> [--every N] [--audio out.wav]`
//!     — decode a REAL Bink movie's frames to PNG stills (block-75
//!     media-evidence path; deterministic zero-dependency encoder)
//!
//! Run with `cargo run -p prototype-bin -- <path>` to auto-detect the
//! format from magic bytes and print a structured summary.

use std::path::PathBuf;

mod skinned_run;
mod frontend_cxform;

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
             {} list-anims <file.p3d> [name_filter]\n      list every 0x121000 anim track with name/frames/fps (block 93)\n  \
             {} list-missions <scripts.rcf>\n      resolve the mission chunk STRUCTURALLY (the Lua51 member carrying the engine's AddValidMission marker) and print the mission table\n  \
             {} decode-audio <file.p3d> <clip-name|all> <out.wav|dir>\n      decode RADP audio (PROVEN IMA ADPCM) to 16-bit WAV\n  \
             {} play-audio <file.p3d> <clip-name>\n      play a clip through the real device edge (feature audio-cpal)\n  \
             {} bik-info <file.bik>\n      parse a Bink movie container (block-46 layout; reads the head only)\n  \
             {} list-movies <movies.rcf>\n      census the movie archive: cracked member names, Bink headers, subtitle tables\n  \
             {} audio-rcf <archive.rcf> [--list|--nis|--banks|--extract <name> <out>]\n      serve a name-table audio archive BY NAME (the release 00audio/01audio.rcf)\n  \
             {} bank-info <bank.p3d>\n      decode a Cue/Patch audio bank (music graphs, stem maps, patch descriptors)",
            args[0], args[0], args[0], args[0], args[0], args[0], args[0], args[0], args[0], args[0], args[0], args[0]
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
    if let Some(pos) = args.iter().position(|a| a == "list-anims") {
        // block 93: list every 0x121000 anim track in a P3D with its
        // group/frame/fps census so the user can pick real takes to
        // render.  Usage: list-anims <file.p3d> [name_filter]
        match args.get(pos + 1) {
            Some(path) => list_anims(path, args.get(pos + 2).map(|s| s.as_str())),
            None => {
                eprintln!("usage: prototype-rs list-anims <file.p3d> [name_filter]");
                std::process::exit(2);
            }
        }
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
    if let Some(pos) = args.iter().position(|a| a == "bik-frames") {
        let rest: Vec<&String> = args[pos + 1..].iter().collect();
        bik_frames(&rest);
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "render-art-world") {
        let rest: Vec<&String> = args[pos + 1..].iter().collect();
        render_art_world(&rest);
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "render-skinned-run") {
        let rest: Vec<&String> = args[pos + 1..].iter().collect();
        skinned_run::run(&rest);
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "render-frontend-cxform") {
        let rest: Vec<String> = args[pos + 1..].iter().map(|s| s.to_string()).collect();
        frontend_cxform::run(&rest);
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "render-cutscene-walkthrough") {
        let rest: Vec<&String> = args[pos + 1..].iter().collect();
        render_cutscene_walkthrough(&rest);
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "render-mesh-turntable") {
        let rest: Vec<&String> = args[pos + 1..].iter().collect();
        render_mesh_turntable(&rest);
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
///
/// `bik-frames <file.bik> <out-dir> [--every N] [--max N] [--prefix P]
/// [--audio <out.wav>]` — the block-75 REAL-GAME-VISUALS evidence
/// path: decodes a REAL Bink movie through the proven streaming
/// player (`BinkPlayer`) and writes PNG stills of every Nth frame
/// (zero-dependency deterministic PNG encoder
/// [`prototype_video::png_encode_rgba`] on the ffmpeg-verified
/// [`prototype_video::yuv420_to_rgba8`]), printing per-frame
/// non-blank stats; `--audio` also dumps track 0's full decode as a
/// 16-bit WAV for the MP4 mux.
fn bik_frames(args: &[&String]) {
    use std::collections::BTreeSet;
    if args.is_empty() {
        eprintln!(
            "usage: prototype-rs bik-frames <file.bik> <out-dir> \
             [--every N] [--max N] [--prefix P] [--audio <out.wav>]"
        );
        std::process::exit(2);
    }
    let path = args[0].as_str();
    let out_dir = args
        .get(1)
        .map(|s| s.as_str())
        .unwrap_or_else(|| {
            eprintln!("bik-frames: missing <out-dir>");
            std::process::exit(2);
        });
    let flag = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a.as_str() == name)
            .and_then(|p| args.get(p + 1))
            .map(|s| s.to_string())
    };
    let every: usize = flag("--every").and_then(|s| s.parse().ok()).unwrap_or(1).max(1);
    let max_frames: Option<usize> = flag("--max").and_then(|s| s.parse().ok());
    let stem = std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "frame".into());
    let prefix = flag("--prefix").unwrap_or(stem);
    std::fs::create_dir_all(out_dir).unwrap_or_else(|e| {
        eprintln!("{out_dir}: {e}");
        std::process::exit(1);
    });

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
    let (w, h) = (player.width(), player.height());

    let mut track0: Vec<f32> = Vec::new();
    let mut decoded = 0usize;
    let mut pngs = 0usize;
    let mut written: Vec<String> = Vec::new();
    while decoded < max_frames.unwrap_or(usize::MAX) {
        let frame = match player.next_frame() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("{path}: frame {decoded} decode failed: {e}");
                std::process::exit(1);
            }
        };
        for t in &frame.audio {
            if t.track == 0 {
                track0.extend_from_slice(&t.samples);
            }
        }
        if decoded % every == 0 {
            let rgba = prototype_video::yuv420_to_rgba8(w, h, &frame.yuv420)
                .unwrap_or_else(|e| {
                    eprintln!("{path}: frame {decoded} rgba failed: {e}");
                    std::process::exit(1);
                });
            let png = prototype_video::png_encode_rgba(w, h, &rgba).unwrap_or_else(|e| {
                eprintln!("{path}: frame {decoded} png failed: {e}");
                std::process::exit(1);
            });
            let name = format!("{prefix}_{:04}.png", frame.index);
            let full = format!("{out_dir}/{name}");
            std::fs::write(&full, &png).unwrap_or_else(|e| {
                eprintln!("{full}: {e}");
                std::process::exit(1);
            });
            // non-blank stats: distinct byte values + min/max luma-ish
            let mut distinct = BTreeSet::new();
            let (mut min, mut max) = (u8::MAX, u8::MIN);
            for &b in &rgba {
                distinct.insert(b);
                min = min.min(b);
                max = max.max(b);
            }
            let verdict = if max > min { "content" } else { "FLAT" };
            println!(
                "  {name}: {}x{}, {} B png, distinct {} , min {min} max {max} — {verdict}",
                w,
                h,
                png.len(),
                distinct.len()
            );
            written.push(name);
            pngs += 1;
        }
        decoded += 1;
    }
    println!("decoded {decoded} frames, wrote {pngs} PNG(s) to {out_dir}");
    for n in &written {
        println!("  written: {n}");
    }
    if let Some(wav_path) = flag("--audio") {
        if player.audio_tracks().is_empty() {
            eprintln!("bik-frames: --audio requested but the movie has no audio tracks");
            std::process::exit(1);
        }
        let t = &player.audio_tracks()[0];
        let s16 = prototype_video::pcm_f32_to_s16(&track0);
        let wav = prototype_audio::radp::encode_wav16(&s16, t.channels as u32, t.sample_rate);
        std::fs::write(&wav_path, &wav).unwrap_or_else(|e| {
            eprintln!("{wav_path}: {e}");
            std::process::exit(1);
        });
        println!(
            "audio track 0: {} sample frames ({} Hz, {} ch) → {wav_path}",
            s16.len() / t.channels.max(1),
            t.sample_rate,
            t.channels
        );
    }
}

/// `render-art-world <fixture.p3d> <out.png> [--width N] [--height N]`
/// — the block-76 CPU-mirror evidence path: decodes the first Shape +
/// its first Mesh from a real P3D art fixture, frames the camera on
/// the mesh's bounding box, renders through the daylight fragment
/// model (mirrored from `shader::world::daylight_state` on the REAL
/// DLL `default` pair), and encodes a PNG via the proven
/// zero-dependency encoder (block 75).  The GPU harness
/// (`prototype_render::render_art_world`) skips in the sandbox (no
/// wgpu adapter); the first real GPU PNG is USER-SIDE.
fn render_art_world(args: &[&String]) {
    if args.len() < 2 {
        eprintln!(
            "usage: prototype-rs render-art-world <fixture.p3d> <out.png> \
             [--width N] [--height N]"
        );
        std::process::exit(2);
    }
    let fixture_path = args[0];
    let out_path = args[1];
    let mut width = 320u32;
    let mut height = 180u32;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--width" if i + 1 < args.len() => {
                width = args[i + 1].parse().unwrap_or_else(|e| {
                    eprintln!("--width: {e}");
                    std::process::exit(2);
                });
                i += 2;
            }
            "--height" if i + 1 < args.len() => {
                height = args[i + 1].parse().unwrap_or_else(|e| {
                    eprintln!("--height: {e}");
                    std::process::exit(2);
                });
                i += 2;
            }
            other => {
                eprintln!("unknown arg: {other}");
                std::process::exit(2);
            }
        }
    }

    let buf = std::fs::read(fixture_path).unwrap_or_else(|e| {
        eprintln!("render-art-world: read {fixture_path}: {e}");
        std::process::exit(1);
    });
    let top = prototype_p3d::geometry::top_level_chunks(&buf).unwrap_or_else(|| {
        eprintln!("render-art-world: {fixture_path} is not a valid P3D");
        std::process::exit(1);
    });
    let shape = top
        .iter()
        .find(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_SHAPE)
        .map(|c| prototype_p3d::geometry::decode_shape(&buf, c.offset, c.size).unwrap())
        .unwrap_or_else(|| {
            eprintln!("render-art-world: no Shape chunk in {fixture_path}");
            std::process::exit(1);
        });
    let decoded = shape.meshes.first().unwrap_or_else(|| {
        eprintln!("render-art-world: shape has no mesh");
        std::process::exit(1);
    });
    let mesh = prototype_render::scene::SceneMesh::from_p3d_mesh(decoded);
    eprintln!(
        "render-art-world: mesh {:?} — {} verts, {} tris, bbox {:?}",
        mesh.name,
        mesh.vertex_count(),
        mesh.triangle_count(),
        mesh.bound_box
    );

    // Try the in-member diffuse DDS (fail-soft → white fallback).
    let texture: Option<(Vec<u8>, u32, u32)> = top
        .iter()
        .find(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_TEXTURE)
        .and_then(|c| prototype_p3d::geometry::decode_texture(&buf, c.offset, c.size).ok())
        .and_then(|t| prototype_render::dxt::decode_dds_mips(&t.dds).ok())
        .and_then(|s| s.first().cloned())
        .map(|s| (s.rgba, s.width, s.height));

    let cam = prototype_render::render_art_world_cpu::frame_camera_for(&mesh);
    let frame = prototype_render::render_art_world_cpu::render_mesh_cpu(
        &mesh,
        &cam,
        texture.as_ref().map(|t| (t.0.as_slice(), t.1, t.2)),
        width,
        height,
        [10, 12, 16, 255],
    )
    .unwrap_or_else(|e| {
        eprintln!("render-art-world: render failed: {e}");
        std::process::exit(1);
    });

    let png = prototype_video::png::png_encode_rgba(frame.width, frame.height, &frame.rgba)
        .unwrap_or_else(|e| {
            eprintln!("render-art-world: PNG encode failed: {e}");
            std::process::exit(1);
        });
    std::fs::write(out_path, &png).unwrap_or_else(|e| {
        eprintln!("render-art-world: write {out_path}: {e}");
        std::process::exit(1);
    });
    eprintln!(
        "render-art-world: wrote {out_path} ({}x{}, {} B PNG, {} distinct colours, \
         {} tris drawn / {} clipped)",
        frame.width,
        frame.height,
        png.len(),
        frame.stats.distinct,
        frame.stats.triangles_drawn,
        frame.stats.triangles_clipped
    );
}

/// `render-mesh-turntable --part <p3d>:<shape-substr>[@dx,dy,dz][#tex-substr]...
/// --out-prefix <dir/prefix> [--frames N] [--width W] [--height H]
/// [--dist-mult F] [--elev-deg D] [--start-deg D] [--turn-deg D]` —
/// the block-82 Alex Mercer evidence path: collects the REAL decoded
/// art meshes whose SHAPE names match the `--part` filters (optionally
/// re-positioned by the given offset — the head/jacket pieces ship in
/// per-package layout coordinates), merges them into one scene, orbits
/// the proven CPU-mirror camera around it and writes one PNG per frame
/// (`<prefix>_%04d.png`) ready for ffmpeg MP4 assembly.
///
/// The motion is PROCEDURAL (an orbiting camera — a turntable), not a
/// real animation track: the skeleton (`nis_alexSkeleton`) and its
/// skinning are not yet decoded.  Stated honestly in the report.
fn render_mesh_turntable(args: &[&String]) {
    #[derive(Clone)]
    struct PartSpec {
        path: String,
        shape_sub: String,
        offset: [f32; 3],
        tex_sub: Option<String>,
    }
    let mut parts: Vec<PartSpec> = Vec::new();
    let mut out_prefix = String::from("media/turntable/frame");
    let mut frames = 48usize;
    let mut width = 640u32;
    let mut height = 360u32;
    let mut dist_mult = 2.6f32;
    let mut elev_deg = 12.0f32;
    let mut start_deg = 0.0f32;
    let mut turn_deg = 360.0f32;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--part" if i + 1 < args.len() => {
                // <p3d>:<shape-sub>[@dx,dy,dz][#tex-sub]
                let spec = args[i + 1].clone();
                let (path, rest) = spec.split_once(':').unwrap_or_else(|| {
                    eprintln!("--part: expected <p3d>:<shape-sub>[...]: {spec}");
                    std::process::exit(2);
                });
                let (rest, tex_sub) = match rest.split_once('#') {
                    Some((r, t)) => (r, Some(t.to_string())),
                    None => (rest, None),
                };
                let (shape_sub, off) = match rest.split_once('@') {
                    Some((s, o)) => {
                        let v: Vec<f32> =
                            o.split(',').filter_map(|x| x.parse().ok()).collect();
                        if v.len() != 3 {
                            eprintln!("--part: offset needs dx,dy,dz: {spec}");
                            std::process::exit(2);
                        }
                        (s.to_string(), [v[0], v[1], v[2]])
                    }
                    None => (rest.to_string(), [0.0; 3]),
                };
                parts.push(PartSpec { path: path.to_string(), shape_sub, offset: off, tex_sub });
                i += 2;
            }
            "--out-prefix" if i + 1 < args.len() => {
                out_prefix = args[i + 1].clone();
                i += 2;
            }
            "--frames" if i + 1 < args.len() => {
                frames = args[i + 1].parse().unwrap_or_else(|e| {
                    eprintln!("--frames: {e}");
                    std::process::exit(2);
                });
                i += 2;
            }
            "--width" if i + 1 < args.len() => {
                width = args[i + 1].parse().unwrap_or_else(|e| {
                    eprintln!("--width: {e}");
                    std::process::exit(2);
                });
                i += 2;
            }
            "--height" if i + 1 < args.len() => {
                height = args[i + 1].parse().unwrap_or_else(|e| {
                    eprintln!("--height: {e}");
                    std::process::exit(2);
                });
                i += 2;
            }
            "--dist-mult" if i + 1 < args.len() => {
                dist_mult = args[i + 1].parse().unwrap_or_else(|e| {
                    eprintln!("--dist-mult: {e}");
                    std::process::exit(2);
                });
                i += 2;
            }
            "--elev-deg" if i + 1 < args.len() => {
                elev_deg = args[i + 1].parse().unwrap_or_else(|e| {
                    eprintln!("--elev-deg: {e}");
                    std::process::exit(2);
                });
                i += 2;
            }
            "--start-deg" if i + 1 < args.len() => {
                start_deg = args[i + 1].parse().unwrap_or_else(|e| {
                    eprintln!("--start-deg: {e}");
                    std::process::exit(2);
                });
                i += 2;
            }
            "--turn-deg" if i + 1 < args.len() => {
                turn_deg = args[i + 1].parse().unwrap_or_else(|e| {
                    eprintln!("--turn-deg: {e}");
                    std::process::exit(2);
                });
                i += 2;
            }
            other => {
                eprintln!("unknown arg: {other}");
                std::process::exit(2);
            }
        }
    }
    if parts.is_empty() {
        eprintln!("render-mesh-turntable: no --part given");
        std::process::exit(2);
    }

    // Collect the real meshes + per-part textures.
    let mut merged = prototype_render::scene::SceneMesh {
        name: String::from("turntable-merge"),
        desc: 0,
        positions: Vec::new(),
        normals: Vec::new(),
        uvs: Vec::new(),
        tangents: Vec::new(),
        indices: Vec::new(),
        bound_box: None,
        bound_sphere: None,
        texture_dds: None,
        extra_textures: Vec::new(),
    };
    struct PartTex {
        tex: Option<(Vec<u8>, u32, u32)>,
        from: usize,
        to: usize,
    }
    let mut part_tex: Vec<PartTex> = Vec::new();
    for p in &parts {
        let buf = std::fs::read(&p.path).unwrap_or_else(|e| {
            eprintln!("render-mesh-turntable: read {}: {e}", p.path);
            std::process::exit(1);
        });
        let top = prototype_p3d::geometry::top_level_chunks(&buf).unwrap_or_else(|| {
            eprintln!("render-mesh-turntable: {} is not a valid P3D", p.path);
            std::process::exit(1);
        });
        let sub = p.shape_sub.to_lowercase();
        let mut matched = 0usize;
        for c in top
            .iter()
            .filter(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_SHAPE)
        {
            let shape = prototype_p3d::geometry::decode_shape(&buf, c.offset, c.size)
                .unwrap_or_else(|e| {
                    eprintln!("render-mesh-turntable: shape decode: {e}");
                    std::process::exit(1);
                });
            if !shape.name.to_lowercase().contains(&sub) {
                continue;
            }
            matched += 1;
            for m in &shape.meshes {
                let from = merged.positions.len() as u16;
                let mut mesh = prototype_render::scene::SceneMesh::from_p3d_mesh(m);
                for v in mesh.positions.iter_mut() {
                    v[0] += p.offset[0];
                    v[1] += p.offset[1];
                    v[2] += p.offset[2];
                }
                merged.positions.extend(mesh.positions.drain(..));
                merged.normals.extend(mesh.normals.drain(..));
                merged.uvs.extend(mesh.uvs.drain(..));
                merged.tangents.extend(mesh.tangents.drain(..));
                merged.indices.extend(mesh.indices.iter().map(|ix| ix + from));
                part_tex.push(PartTex { tex: None, from: from as usize, to: merged.positions.len() });
            }
        }
        if matched == 0 {
            eprintln!(
                "render-mesh-turntable: no shape matching \"{}\" in {}",
                p.shape_sub, p.path
            );
            std::process::exit(1);
        }
        // The part's diffuse: the texture whose NAME matches tex-sub,
        // else the member's first texture.
        let tex_pick = top
            .iter()
            .filter(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_TEXTURE)
            .filter_map(|c| prototype_p3d::geometry::decode_texture(&buf, c.offset, c.size).ok())
            .find(|t| {
                p.tex_sub
                    .as_ref()
                    .map(|s| t.name.to_lowercase().contains(&s.to_lowercase()))
                    .unwrap_or(true)
            })
            .or_else(|| {
                // a --part filter without a match falls back to ANY texture
                top.iter()
                    .filter(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_TEXTURE)
                    .find_map(|c| {
                        prototype_p3d::geometry::decode_texture(&buf, c.offset, c.size).ok()
                    })
            });
        if let Some(t) = tex_pick {
            if let Ok(surfaces) = prototype_render::dxt::decode_dds_mips(&t.dds) {
                if let Some(s) = surfaces.first() {
                    let rgba = s.rgba.clone();
                    let (tw, th) = (s.width, s.height);
                    // fill the just-added parts' texture slots
                    let last_to = merged.positions.len();
                    for pt in part_tex.iter_mut().rev() {
                        if pt.to == last_to && pt.tex.is_none() {
                            pt.tex = Some((rgba.clone(), tw, th));
                        } else {
                            break;
                        }
                    }
                }
            }
        }
    }

    // Combined bounds (from the merged positions — some parts' bbox
    // children may be absent).
    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];
    for v in &merged.positions {
        for ch in 0..3 {
            mn[ch] = mn[ch].min(v[ch]);
            mx[ch] = mx[ch].max(v[ch]);
        }
    }
    let center = [
        (mn[0] + mx[0]) * 0.5,
        (mn[1] + mx[1]) * 0.5,
        (mn[2] + mx[2]) * 0.5,
    ];
    let radius = 0.5
        * ((mx[0] - mn[0]).powi(2) + (mx[1] - mn[1]).powi(2) + (mx[2] - mn[2]).powi(2))
            .sqrt();
    eprintln!(
        "render-mesh-turntable: merged {} parts, {} verts, {} tris, bounds [{:.2} {:.2} {:.2}]-[{:.2} {:.2} {:.2}]",
        parts.len(),
        merged.vertex_count(),
        merged.triangle_count(),
        mn[0], mn[1], mn[2], mx[0], mx[1], mx[2]
    );

    // One PNG per turntable frame.
    let elev = elev_deg.to_radians();
    let n = frames.max(2);
    let mut total_drawn = 0usize;
    let mut min_distinct = usize::MAX;
    for k in 0..frames {
        let ang = start_deg.to_radians() + turn_deg.to_radians() * (k as f32) / (n as f32);
        let d = (radius.max(1e-4)) * dist_mult;
        let mut cam = prototype_render::scene::Camera::default();
        cam.eye = prototype_math::Vec3::new(
            center[0] + ang.sin() * d * elev.cos(),
            center[1] + elev.sin() * d,
            center[2] + ang.cos() * d * elev.cos(),
        );
        cam.target = prototype_math::Vec3::new(center[0], center[1], center[2]);
        cam.fov_y = std::f32::consts::FRAC_PI_4;
        cam.near = radius * 0.05;
        cam.far = radius * 12.0 + 1.0;

        // Multi-part compositing: the CPU mirror renders one mesh per
        // call, so each part is rendered with the SAME camera and the
        // non-background pixels are overdrawn onto the canvas in part
        // order.  This is a painter's merge — NOT depth-correct where
        // parts overlap.  The Alex head sits above the collar with
        // negligible overlap, so the artefact is not visible; noted
        // honestly here and in the report.
        let mut canvas = vec![10u8, 12, 16, 255];
        canvas.resize((width * height) as usize * 4, 0);
        for px in canvas.chunks_exact_mut(4) {
            px.copy_from_slice(&[10, 12, 16, 255]);
        }
        for p in &part_tex {
            let sub = slice_scene_mesh(&merged, p.from, p.to);
            let frame = prototype_render::render_art_world_cpu::render_mesh_cpu(
                &sub,
                &cam,
                p.tex.as_ref().map(|t| (t.0.as_slice(), t.1, t.2)),
                width,
                height,
                [10, 12, 16, 255],
            )
            .unwrap_or_else(|e| {
                eprintln!("render-mesh-turntable: render failed: {e}");
                std::process::exit(1);
            });
            // overwrite where the part drew (its clear colour marks
            // background — the cpu mirror fills with `clear`, so copy
            // only non-clear pixels; the parts' textures never hit this
            // exact background colour except accidentally, accepted).
            for (dst, src) in canvas.chunks_exact_mut(4).zip(frame.rgba.chunks_exact(4)) {
                if src != [10, 12, 16, 255] {
                    dst.copy_from_slice(src);
                }
            }
            total_drawn += frame.stats.triangles_drawn;
            min_distinct = min_distinct.min(frame.stats.distinct);
        }
        let png = prototype_video::png::png_encode_rgba(width, height, &canvas)
            .unwrap_or_else(|e| {
                eprintln!("render-mesh-turntable: PNG encode failed: {e}");
                std::process::exit(1);
            });
        let path = format!("{out_prefix}_{:04}.png", k);
        std::fs::write(&path, &png).unwrap_or_else(|e| {
            eprintln!("render-mesh-turntable: write {path}: {e}");
            std::process::exit(1);
        });
    }
    eprintln!(
        "render-mesh-turntable: wrote {frames} frames to {out_prefix}_%04d.png \
         (avg drawn tris {}, min distinct {min_distinct})",
        total_drawn / frames
    );
}

/// Slice a contiguous vertex range of a merged mesh into its own
/// SceneMesh (indices re-based to zero).
fn slice_scene_mesh(m: &prototype_render::scene::SceneMesh, from: usize, to: usize) -> prototype_render::scene::SceneMesh {
    let positions = m.positions[from..to].to_vec();
    let normals = if m.normals.len() == m.positions.len() {
        m.normals[from..to].to_vec()
    } else {
        Vec::new()
    };
    let uvs = if m.uvs.len() == m.positions.len() {
        m.uvs[from..to].to_vec()
    } else {
        Vec::new()
    };
    let indices: Vec<u16> = m
        .indices
        .iter()
        .filter(|ix| (**ix as usize) >= from && (**ix as usize) < to)
        .map(|ix| ix - from as u16)
        .collect();
    prototype_render::scene::SceneMesh {
        name: format!("{}[{from}..{to}]", m.name),
        desc: m.desc,
        positions,
        normals,
        uvs,
        tangents: Vec::new(),
        indices,
        bound_box: None,
        bound_sphere: None,
        texture_dds: None,
        extra_textures: Vec::new(),
    }
}

/// `render-cutscene-walkthrough <takes.p3d> <mesh.p3d> <out.png>
/// [--take N|NAME] [--frames N] [--width W] [--height H] [--cols C]
/// [--framing F] [--list]` — the block-78 evidence path: decodes the
/// REAL NIS camera takes from a track member (the committed
/// `art_nis_nis_0_3_a{0,1}_camera_takes.p3d` fixtures), mounts the
/// chosen take's scene-local rig onto the REAL art mesh (one uniform
/// scale + translation — the REAL path shape preserved), samples
/// evenly spaced keyframed poses, renders each through the CPU-mirror
/// daylight path, and writes a contact-sheet PNG plus the individual
/// frames (`<out-no-ext>.fNN.png`) so the walkthrough can be flipped
/// through or assembled into video USER-SIDE.
fn render_cutscene_walkthrough(args: &[&String]) {
    if args.len() < 3 {
        eprintln!(
            "usage: prototype-rs render-cutscene-walkthrough <takes.p3d> <mesh.p3d> \
             <out.png> [--take N|NAME] [--frames N] [--width W] [--height H] \
             [--cols C] [--framing F] [--list]"
        );
        std::process::exit(2);
    }
    let takes_path = args[0].as_str();
    let mesh_path = args[1].as_str();
    let out_path = args[2].as_str();
    let mut take_sel: Option<String> = None;
    let mut frames = 8usize;
    let mut width = 320u32;
    let mut height = 180u32;
    let mut cols = 2usize;
    let mut framing = 1.5f32;
    let mut list_only = false;
    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "--take" if i + 1 < args.len() => {
                take_sel = Some(args[i + 1].clone());
                i += 2;
            }
            "--frames" if i + 1 < args.len() => {
                frames = args[i + 1].parse().unwrap_or_else(|e| {
                    eprintln!("--frames: {e}");
                    std::process::exit(2);
                });
                i += 2;
            }
            "--width" if i + 1 < args.len() => {
                width = args[i + 1].parse().unwrap_or_else(|e| {
                    eprintln!("--width: {e}");
                    std::process::exit(2);
                });
                i += 2;
            }
            "--height" if i + 1 < args.len() => {
                height = args[i + 1].parse().unwrap_or_else(|e| {
                    eprintln!("--height: {e}");
                    std::process::exit(2);
                });
                i += 2;
            }
            "--cols" if i + 1 < args.len() => {
                cols = args[i + 1].parse().unwrap_or_else(|e| {
                    eprintln!("--cols: {e}");
                    std::process::exit(2);
                });
                i += 2;
            }
            "--framing" if i + 1 < args.len() => {
                framing = args[i + 1].parse().unwrap_or_else(|e| {
                    eprintln!("--framing: {e}");
                    std::process::exit(2);
                });
                i += 2;
            }
            "--list" => {
                list_only = true;
                i += 1;
            }
            other => {
                eprintln!("unknown arg: {other}");
                std::process::exit(2);
            }
        }
    }

    // The REAL takes.
    let takes_buf = std::fs::read(takes_path).unwrap_or_else(|e| {
        eprintln!("render-cutscene-walkthrough: read {takes_path}: {e}");
        std::process::exit(1);
    });
    let takes = prototype_p3d::nis::camera_takes(&takes_buf);
    if takes.is_empty() {
        eprintln!("render-cutscene-walkthrough: no CAM takes in {takes_path}");
        std::process::exit(1);
    }
    if list_only {
        for (n, t) in takes.iter().enumerate() {
            eprintln!(
                "[{n}] {} — {} TRAN keys, {} LOOK keys, chunk {} B",
                t.name,
                t.tran_keys.len(),
                t.look_keys.len(),
                t.size
            );
        }
        return;
    }

    // Pick the take: index or exact name; default = first walkable one.
    let take = match &take_sel {
        Some(sel) => {
            if let Ok(n) = sel.parse::<usize>() {
                takes.get(n).unwrap_or_else(|| {
                    eprintln!("render-cutscene-walkthrough: take index {n} out of range");
                    std::process::exit(1);
                })
            } else {
                takes
                    .iter()
                    .find(|t| t.name == *sel)
                    .unwrap_or_else(|| {
                        eprintln!("render-cutscene-walkthrough: no take named {sel:?}");
                        std::process::exit(1);
                    })
            }
        }
        None => takes
            .iter()
            .find(|t| t.tran_keys.len() >= 2)
            .unwrap_or_else(|| {
                eprintln!("render-cutscene-walkthrough: no take with >= 2 TRAN keys");
                std::process::exit(1);
            }),
    };

    // The REAL art mesh + its in-member diffuse (fail-soft → white).
    let mesh_buf = std::fs::read(mesh_path).unwrap_or_else(|e| {
        eprintln!("render-cutscene-walkthrough: read {mesh_path}: {e}");
        std::process::exit(1);
    });
    let top = prototype_p3d::geometry::top_level_chunks(&mesh_buf).unwrap_or_else(|| {
        eprintln!("render-cutscene-walkthrough: {mesh_path} is not a valid P3D");
        std::process::exit(1);
    });
    let shape = top
        .iter()
        .find(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_SHAPE)
        .map(|c| prototype_p3d::geometry::decode_shape(&mesh_buf, c.offset, c.size).unwrap())
        .unwrap_or_else(|| {
            eprintln!("render-cutscene-walkthrough: no Shape chunk in {mesh_path}");
            std::process::exit(1);
        });
    let decoded = shape.meshes.first().unwrap_or_else(|| {
        eprintln!("render-cutscene-walkthrough: shape has no mesh");
        std::process::exit(1);
    });
    let mesh = prototype_render::scene::SceneMesh::from_p3d_mesh(decoded);
    let texture: Option<(Vec<u8>, u32, u32)> = top
        .iter()
        .find(|c| c.chunk_type == prototype_p3d::geometry::CHUNK_TYPE_TEXTURE)
        .and_then(|c| prototype_p3d::geometry::decode_texture(&mesh_buf, c.offset, c.size).ok())
        .and_then(|t| prototype_render::dxt::decode_dds_mips(&t.dds).ok())
        .and_then(|s| s.first().cloned())
        .map(|s| (s.rgba, s.width, s.height));

    eprintln!(
        "render-cutscene-walkthrough: take {:?} ({} TRAN / {} LOOK keys) → mesh {:?} \
         ({} tris, diffuse {}), fov {} rad (the take's REAL FOV key)",
        take.name,
        take.tran_keys.len(),
        take.look_keys.len(),
        mesh.name,
        mesh.triangle_count(),
        if texture.is_some() { "real DDS" } else { "white fallback" },
        prototype_render::cutscene_walkthrough::NIS_TAKE_FOV_Y
    );

    let walked = prototype_render::cutscene_walkthrough::walkthrough_frames(
        &mesh,
        texture.as_ref().map(|t| (t.0.as_slice(), t.1, t.2)),
        take,
        frames,
        framing,
        prototype_render::cutscene_walkthrough::NIS_TAKE_FOV_Y,
        width,
        height,
        [10, 12, 16, 255],
    )
    .unwrap_or_else(|e| {
        eprintln!("render-cutscene-walkthrough: {e}");
        std::process::exit(1);
    });

    // Individual frames next to the sheet (USER-SIDE video assembly).
    let stem = out_path
        .trim_end_matches(".png")
        .trim_end_matches(".PNG")
        .to_string();
    for f in &walked {
        let png = prototype_video::png::png_encode_rgba(f.cpu.width, f.cpu.height, &f.cpu.rgba)
            .unwrap_or_else(|e| {
                eprintln!("render-cutscene-walkthrough: frame {} PNG: {e}", f.index);
                std::process::exit(1);
            });
        let path = format!("{}.f{:02}.png", stem, f.index);
        std::fs::write(&path, &png).unwrap_or_else(|e| {
            eprintln!("render-cutscene-walkthrough: write {path}: {e}");
            std::process::exit(1);
        });
        eprintln!(
            "  f{:02} eye ({:.3}, {:.3}, {:.3}) → target ({:.3}, {:.3}, {:.3}) — \
             {} distinct, {} tris, {} B",
            f.index,
            f.eye[0],
            f.eye[1],
            f.eye[2],
            f.target[0],
            f.target[1],
            f.target[2],
            f.cpu.stats.distinct,
            f.cpu.stats.triangles_drawn,
            png.len()
        );
    }

    // The contact sheet — the committed evidence artefact.
    let (sw, sh, sheet) =
        prototype_render::cutscene_walkthrough::contact_sheet(&walked, cols, 4, [0, 0, 0, 255])
            .unwrap_or_else(|e| {
                eprintln!("render-cutscene-walkthrough: sheet: {e}");
                std::process::exit(1);
            });
    let png = prototype_video::png::png_encode_rgba(sw, sh, &sheet).unwrap_or_else(|e| {
        eprintln!("render-cutscene-walkthrough: sheet PNG: {e}");
        std::process::exit(1);
    });
    std::fs::write(out_path, &png).unwrap_or_else(|e| {
        eprintln!("render-cutscene-walkthrough: write {out_path}: {e}");
        std::process::exit(1);
    });
    let total_distinct = walked.iter().map(|f| f.cpu.stats.distinct).sum::<usize>();
    eprintln!(
        "render-cutscene-walkthrough: wrote {out_path} ({}x{}, {} B; {} frames, \
         {} distinct colours total) + {} frame PNGs ({}.fNN.png)",
        sw,
        sh,
        png.len(),
        walked.len(),
        total_distinct,
        walked.len(),
        stem
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

/// block 93 — `list-anims <file.p3d> [name_filter]`: list every
/// 0x121000 anim track in a P3D with its name, frame count, fps, and
/// chunk span so the user can pick real takes to render.  Mirrors the
/// m91_list_tracks example but is reachable as a CLI subcommand (the
/// payload's claimed `list-alex-anims` did not actually exist as a
/// CLI subcommand; this is the canonical entry point).
fn list_anims(path: &str, filter: Option<&str>) {
    let data = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let tracks = prototype_p3d::anim::find_anim_tracks(&data);
    let flt = filter.unwrap_or("").to_lowercase();
    println!("tracks: {}", tracks.len());
    let mut shown = 0;
    for (off, span, name) in &tracks {
        let lname = name.to_lowercase();
        if !flt.is_empty() && !lname.contains(&flt) {
            continue;
        }
        // frame count / fps live in the track header at off:
        // [u32 0][u8 slot][name][kind][f32 frame_count][f32 fps]
        let hdr = &data[*off..(*off + span).min(data.len())];
        let mut fc = 0.0f32;
        let mut fps = 0.0f32;
        for i in 0..hdr.len().saturating_sub(12) {
            let tag = &hdr[i..i + 4];
            if tag == b"PTRN" || tag == b"CAM\0" || tag == b"EXP\0" {
                fc = f32::from_le_bytes(hdr[i + 4..i + 8].try_into().unwrap());
                fps = f32::from_le_bytes(hdr[i + 8..i + 12].try_into().unwrap());
                break;
            }
        }
        println!("{} off={} span={} frames={} fps={}", name, off, span, fc, fps);
        shown += 1;
    }
    if !flt.is_empty() {
        println!("filter \"{flt}\" -> {shown} tracks");
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

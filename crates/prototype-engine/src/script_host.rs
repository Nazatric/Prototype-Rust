//! script_host — run recovered Prototype scripts against stub hosts
//! validated by the M7 Lua host-API registry.
//!
//! ## What this closes (M8)
//!
//! The M7 registry maps the 1 119 engine natives the DLL registers
//! to Lua.  The M5/M8 analysis recovers what the shipped scripts
//! *import*.  This module connects the two:
//!
//! ```text
//! script imports  ──┬── base library (pairs, math, …)
//!                    ├── globals the script itself defines
//!                    └── engine natives ── ALL present in the M7
//!                                         LuaApiRegistry  ✔ (proven
//!                                         for every shipped script)
//! ```
//!
//! `run_script` executes the chunk in the safe-Rust VM
//! ([`prototype_lua::vm`]) with record-only stubs for every engine
//! native the script imports, and returns the call log.  Running the
//! real `frevents.lua` reproduces the static analysis exactly:
//! 60 × `fre_RegisterEvent`, 1 × `fre_RegisterCollectibles`, and the
//! two lifecycle closures walking the same 60 defs.
//!
//! ## PROVEN / LIKELY
//!
//! * PROVEN: every non-library import of both shipped scripts is in
//!   the registry (see the `script_host` tests — this is asserted,
//!   not assumed).
//! * LIKELY: the registry sweep (M7) found *all* registrations —
//!   a script importing a native that is missing from the registry
//!   would surface here as an `unresolved_imports` entry.

use std::collections::BTreeSet;

use prototype_lua::vm::{HostCall, Value, Vm};
use prototype_lua::Lua51File;

use crate::lua_api::LuaApiRegistry;

/// Names the VM provides itself (see `Vm::BASE_LIBRARY`).
fn is_base_library(name: &str) -> bool {
    Vm::BASE_LIBRARY.contains(&name)
}

/// Result of a script run.
#[derive(Debug)]
pub struct ScriptRunReport {
    /// Globals the script reads (GETGLOBAL, whole tree).
    pub imports: Vec<String>,
    /// Globals the script writes (SETGLOBAL, whole tree).
    pub exports: Vec<String>,
    /// Imports that are neither base library, script-defined, nor in
    /// the M7 registry.  Empty for every shipped script (PROVEN).
    pub unresolved_imports: Vec<String>,
    /// Engine natives (registry-confirmed) the script imports.
    pub engine_hosts: Vec<String>,
    /// The recorded host-call log (engine natives + library calls).
    pub host_calls: Vec<HostCall>,
    /// Globals set by the script that hold closures (lifecycle hooks).
    pub closure_exports: Vec<String>,
}

/// Classify a script's imports against the base library, its own
/// exports, and the M7 engine registry — without running it.
pub fn classify_script(file: &Lua51File) -> ScriptRunReport {
    let imports: Vec<String> = file
        .main
        .all_globals_read()
        .iter()
        .map(|g| String::from_utf8_lossy(g).into_owned())
        .collect();
    let exports: Vec<String> = file
        .main
        .all_globals_written()
        .iter()
        .map(|g| String::from_utf8_lossy(g).into_owned())
        .collect();
    let export_set: BTreeSet<&str> =
        exports.iter().map(String::as_str).collect();
    let registry = LuaApiRegistry::new();

    let mut unresolved = Vec::new();
    let mut engine_hosts = Vec::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for name in &imports {
        if !seen.insert(name.as_str()) {
            continue;
        }
        if is_base_library(name) || export_set.contains(name.as_str()) {
            continue;
        }
        if registry.lookup(name).is_some() {
            engine_hosts.push(name.clone());
        } else {
            unresolved.push(name.clone());
        }
    }
    ScriptRunReport {
        imports,
        exports,
        unresolved_imports: unresolved,
        engine_hosts,
        host_calls: Vec::new(),
        closure_exports: Vec::new(),
    }
}

/// Run a script with record-only stubs for every engine native it
/// imports (all of them must exist in the M7 registry; anything
/// unresolved is registered as a failing stub so the mismatch is
/// loud and attributed).
pub fn run_script(file: &Lua51File) -> Result<ScriptRunReport, String> {
    let mut report = classify_script(file);
    let mut vm = Vm::new(file).map_err(|e| e.to_string())?;
    for name in &report.engine_hosts {
        vm.register_host(name, |_vm, _args| Ok(Vec::new()))
            .map_err(|e| e.to_string())?;
    }
    for name in &report.unresolved_imports {
        // Not in the registry: register a stub that fails if actually
        // called, so the run surfaces the gap with a clear message.
        let missing = name.clone();
        let fail = move |_vm: &mut Vm<'_>, _args: &[Value]| {
            Err(prototype_common::P3dError::Other(format!(
                "unresolved host function '{missing}' (not in the M7 registry)"
            )))
        };
        vm.register_host(name, fail).map_err(|e| e.to_string())?;
    }
    // util_GetRandomFloat needs a value; the engine RNG is not
    // reconstructed, so supply a deterministic constant (documented).
    if report
        .engine_hosts
        .iter()
        .any(|h| h == "util_GetRandomFloat")
    {
        // Already registered above as a no-op stub; re-register with a
        // deterministic value so math.randomseed receives a number.
        vm.register_host("util_GetRandomFloat", |_vm, _args| {
            Ok(vec![Value::Num(0.5)])
        })
        .map_err(|e| e.to_string())?;
    }
    vm.run_main().map_err(|e| e.to_string())?;

    // Closure exports (lifecycle entry points).
    let mut closure_exports = Vec::new();
    for name in &report.exports {
        if matches!(vm.get_global(name), Value::Closure(_)) {
            closure_exports.push(name.clone());
        }
    }
    report.closure_exports = closure_exports;
    report.host_calls = vm.host_calls;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fixture(name: &str) -> Vec<u8> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("prototype-bin")
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
    }

    fn lua_entries_from_scripts_rcf() -> Vec<Vec<u8>> {
        let rcf = fixture("scripts.rcf");
        let arc = prototype_rcf::RcfArchive::parse(&rcf).unwrap();
        let mut out = Vec::new();
        for e in &arc.entries {
            let p = arc.payload(e).unwrap();
            if prototype_rcf::PayloadKind::sniff(p)
                == prototype_rcf::PayloadKind::Lua51
            {
                out.push(p.to_vec());
            }
        }
        out
    }

    #[test]
    fn every_script_import_is_base_library_or_registry_native() {
        // THE M7 <-> M8 CLOSURE: for every shipped Lua chunk, every
        // import resolves to the base library, a script-defined
        // global, or a registered engine native.
        let entries = lua_entries_from_scripts_rcf();
        assert!(entries.len() >= 2, "expected both shipped scripts");
        for data in &entries {
            let f = Lua51File::parse(data).unwrap();
            let report = classify_script(&f);
            assert!(
                report.unresolved_imports.is_empty(),
                "unresolved imports: {:?} (imports: {:?})",
                report.unresolved_imports,
                report.imports
            );
            assert!(
                !report.engine_hosts.is_empty(),
                "every script must import at least one engine native"
            );
        }
    }

    #[test]
    fn frevents_host_report_matches_static_analysis() {
        let data = fixture("frevents.lua");
        let f = Lua51File::parse(&data).unwrap();
        let report = run_script(&f).unwrap();
        // Engine natives used — the classifier walks the WHOLE proto
        // tree, so the nested lifecycle closures' imports
        // (fre_ResetEvent, fre_DestroyEvent) are included.
        let mut hosts = report.engine_hosts.clone();
        hosts.sort();
        assert_eq!(
            hosts,
            vec![
                "fre_DestroyEvent",
                "fre_RegisterCollectibles",
                "fre_RegisterEvent",
                "fre_ResetEvent",
            ]
        );
        // Run results: 60 registrations + 1 collectibles.
        let reg = report
            .host_calls
            .iter()
            .filter(|c| c.name == "fre_RegisterEvent")
            .count();
        assert_eq!(reg, 60);
        let coll = report
            .host_calls
            .iter()
            .filter(|c| c.name == "fre_RegisterCollectibles")
            .count();
        assert_eq!(coll, 1);
        // Lifecycle closures exported.
        assert!(
            report
                .closure_exports
                .contains(&"ResetFreeRoamEvents".to_string())
        );
        assert!(
            report
                .closure_exports
                .contains(&"DestroyFreeRoamEvents".to_string())
        );
    }

    #[test]
    fn frevents_engine_natives_are_in_registry_with_fn_addresses() {
        // The names the script imports must point at REAL registry
        // records (fn VA + mechanism) — not just name matches.
        let data = fixture("frevents.lua");
        let f = Lua51File::parse(&data).unwrap();
        let report = classify_script(&f);
        let registry = LuaApiRegistry::new();
        for host in &report.engine_hosts {
            let rec = registry
                .lookup_unique(host)
                .unwrap_or_else(|| panic!("{host} must be in the registry"));
            assert!(rec.fn_va != 0, "{host} has a fn VA");
        }
        // Spot-check the known bindings (PROVEN in M7):
        // fre_RegisterEvent -> 0x10028d62 via registrar 0x10802db0.
        let rec = registry.lookup_unique("fre_RegisterEvent").unwrap();
        assert_eq!(rec.fn_va, 0x10028d62);
        assert_eq!(rec.registrar_va, 0x10802db0);
        let rec = registry.lookup_unique("fre_RegisterCollectibles").unwrap();
        assert_eq!(rec.fn_va, 0x1003ab4d);
    }

    #[test]
    fn mission_script_runs_with_seventy_registrations() {
        let mut chunk = None;
        for data in lua_entries_from_scripts_rcf() {
            if data.len() == 2318 {
                chunk = Some(data);
            }
        }
        let data = chunk.expect("mission chunk");
        let f = Lua51File::parse(&data).unwrap();
        let report = run_script(&f).unwrap();
        assert!(report.unresolved_imports.is_empty());
        let adds = report
            .host_calls
            .iter()
            .filter(|c| c.name == "AddValidMission")
            .count();
        assert_eq!(adds, 70);
        // math.randomseed received util_GetRandomFloat() * 10000 = 5000
        // with the deterministic 0.5 stub.
        let seeds: Vec<&HostCall> = report
            .host_calls
            .iter()
            .filter(|c| c.name == "math.randomseed")
            .collect();
        assert_eq!(seeds.len(), 1);
        assert_eq!(seeds[0].args[0], Value::Num(5000.0));
    }
}

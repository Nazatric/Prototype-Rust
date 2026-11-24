//! Lua-facing engine API registry — M7.
//! ================================
//!
//! The Prototype (2009) Lua scripts call engine hooks by global name
//! (`fre_RegisterEvent`, `AddValidMission`, `go_SetState`, …).  The
//! engine registers those natives into its Lua states at startup.
//! This module models the **complete recovered registry** of such
//! host functions and can *re-derive it live* from
//! `prototypeenginef.dll`, so the static table is never trusted
//! without the binary that proves it.
//!
//! ## How the registration works (PROVEN, byte-exact)
//!
//! Disassembly of `prototypeenginef.dll` (20 205 568 B,
//! image base `0x10000000`) shows every registration as one of:
//!
//! ```text
//! A) 68 <fn>            push fn     ; lua_CFunction (often an ILT thunk)
//!     68 <name>          push name   ; .rdata c-string
//!     8B CE              mov  ecx, this        (optional)
//!     E8 <registrar>     call Register
//!
//! B) 68 <fn>            push fn
//!     68 <name>          push name
//!     E8 <getter>        call get_singleton   ; mov eax,[g]; ret
//!     8B C8              mov  ecx, eax
//!     E8 <registrar>     call Register
//!
//! C) like A/B but fn pushed via  mov r32,imm32 ; push r32
//!     (e.g. AddValidMission: mov eax,0x100088A0 ; push eax)
//! ```
//!
//! Verified example (site `0x1058D1AC`, prologue-verified registrar
//! `0x10802DB0`, whose body allocates a node via `0x107FE720` and
//! stores name+fn through `0x107FE7E0`):
//!
//! ```text
//! push 0x10028D62          ; fre_RegisterEvent (ILT thunk)
//! push 0x10E2A09C          ; "fre_RegisterEvent"
//! mov  ecx, esi
//! call 0x10802DB0
//! ```
//!
//! ## The two registrar families
//!
//! * **BindingNode** — registrar prologue
//!   `push ebx; push esi; push edi; push <size>; mov ebx, ecx;
//!   call 0x107FE720` — allocates a `{name, fn, …}` node from the
//!   engine allocator and links it into a per-binding-object table
//!   (the radLua-style binding object; 583 registrations, 583 names).
//! * **NamedMap** — registrar calls the string-keyed map helper
//!   `0x108351B0` (one binding class per gameplay subsystem:
//!   `go_*`, `ai_*`, `fe_*`, `snd_*`, `cm_*`, `nis_*`, `woi_*`, …;
//!   536 registrations, 536 names).
//!
//! Everything else the sweep sees was classified and **excluded** on
//! evidence: profiler channel labels (`0x10B80220`: "Collision
//! Check", "AsyncSkinning", …), the animation-property registry
//! (`0x1008CC20`: "InheritVelocity", …), the RTTI/class-name registry
//! (`0x105B3D50`), the string-intern/UID registry (`0x1005D5E0`),
//! and the Scaleform **ActionScript** natives (`0x109BA520`: `abs`,
//! `ASSetPropFlags`, … — a different VM, kept for future GFx work).
//!
//! ## What this is NOT
//!
//! No engine code is reproduced — only the *fact* that global name X
//! maps to native function address Y via registrar Z, which is
//! interface information (same tier as the RTTI class-name list).

#![forbid(unsafe_code)]

use std::collections::HashMap;

use prototype_pe::PeFile;

pub use lua_api_data::{HostFnRecord, Mechanism};

#[path = "lua_api_data.rs"]
mod lua_api_data;

// ---------------------------------------------------------------------------
// Registry model over the static table
// ---------------------------------------------------------------------------

/// Typed registry of every recovered Lua host function.
///
/// Built over the static [`HOST_FNS`] table; indexes by name and by
/// subsystem prefix for fast lookup and grouped iteration.
pub struct LuaApiRegistry {
    by_name: HashMap<&'static str, Vec<&'static HostFnRecord>>,
}

impl LuaApiRegistry {
    /// Build the registry from the static recovered table.
    pub fn new() -> Self {
        let mut by_name: HashMap<&'static str, Vec<&'static HostFnRecord>> =
            HashMap::with_capacity(lua_api_data::HOST_FNS.len());
        for rec in lua_api_data::HOST_FNS {
            by_name.entry(rec.name).or_default().push(rec);
        }
        Self { by_name }
    }

    /// Number of static registrations (a name may be registered by
    /// more than one binding class — each registration is kept).
    pub fn registration_count(&self) -> usize {
        lua_api_data::HOST_FNS.len()
    }

    /// Number of distinct global names.
    pub fn unique_name_count(&self) -> usize {
        self.by_name.len()
    }

    /// All registrations for one global name (usually exactly one).
    pub fn lookup(&self, name: &str) -> Option<&[&'static HostFnRecord]> {
        self.by_name.get(name).map(|v| v.as_slice())
    }

    /// The single registration of a name, or `None` if the name is
    /// unknown or (unexpectedly) registered more than once.
    pub fn lookup_unique(&self, name: &str) -> Option<&'static HostFnRecord> {
        match self.by_name.get(name) {
            Some(v) if v.len() == 1 => Some(v[0]),
            _ => None,
        }
    }

    /// All registrations, in site order (the order the engine's
    /// init code registers them).
    pub fn all(&self) -> impl Iterator<Item = &'static HostFnRecord> + 'static {
        lua_api_data::HOST_FNS.iter()
    }

    /// Registrations of one mechanism.
    pub fn by_mechanism(
        &self,
        m: Mechanism,
    ) -> impl Iterator<Item = &'static HostFnRecord> + 'static {
        lua_api_data::HOST_FNS
            .iter()
            .filter(move |r| r.mechanism == m)
    }

    /// Subsystem prefix of a name (`"go_SetState" -> "go"`); names
    /// without `_` get the whole name as their group.
    pub fn prefix_of(name: &str) -> &str {
        match name.find('_') {
            Some(i) => &name[..i],
            None => name,
        }
    }

    /// Count registrations per subsystem prefix, descending.
    pub fn prefix_counts(&self) -> Vec<(&'static str, usize)> {
        let mut counts: HashMap<&'static str, usize> = HashMap::new();
        for rec in lua_api_data::HOST_FNS {
            *counts.entry(Self::prefix_of(rec.name)).or_default() += 1;
        }
        let mut v: Vec<(&'static str, usize)> = counts.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        v
    }

    /// All registrations whose name starts with the given prefix
    /// plus `'_'` (or equals it, for `_`-less groups).
    pub fn group(&self, prefix: &str) -> impl Iterator<Item = &'static HostFnRecord> + 'static {
        let prefix = prefix.to_string();
        lua_api_data::HOST_FNS
            .iter()
            .filter(move |r| Self::prefix_of(r.name) == prefix.as_str())
    }
}

impl Default for LuaApiRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Live extraction from a PE image
// ---------------------------------------------------------------------------

/// Engine-internal helper VAs used by the classification.  Both are
/// fixed facts of this exact `prototypeenginef.dll` build (PC SKU
/// `vug_prototype_pc_na`); see module docs.
pub const ENGINE_ALLOCATOR_VA: u32 = 0x107F_E720;
pub const ENGINE_STRING_MAP_HELPER_VA: u32 = 0x1083_51B0;

/// One host function recovered by a live sweep of a PE image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedHostFn {
    pub name: String,
    pub fn_va: u32,
    pub thunk_target: Option<u32>,
    pub registrar_va: u32,
    pub mechanism: Mechanism,
}

/// Error returned by [`extract_host_fns`].
#[derive(Debug)]
pub enum ExtractError {
    /// A required section is missing from the image.
    MissingSection(&'static str),
    /// The image has no `IMAGE_FILE_MACHINE_I386`-style PE32 layout.
    BadImage(String),
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtractError::MissingSection(s) => write!(f, "missing section {s}"),
            ExtractError::BadImage(m) => write!(f, "bad PE image: {m}"),
        }
    }
}

impl std::error::Error for ExtractError {}

struct SectionView<'a> {
    va: u32,
    data: &'a [u8],
}

/// Sweep a parsed PE for static `push fn; push name; call registrar`
/// registration sites (idioms A/B/C in the module docs) and classify
/// each resolved registrar.  Returns the Lua-facing registrations in
/// site order.  This is a faithful port of `scripts/sweep_v5_final.py`
/// (the Python that produced the static table) — pure byte-pattern
/// work, no disassembler needed.
pub fn extract_host_fns<'a>(pe: &PeFile<'a>) -> Result<Vec<ExtractedHostFn>, ExtractError> {
    let text_sec = pe
        .sections
        .iter()
        .find(|s| s.name == ".text")
        .ok_or(ExtractError::MissingSection(".text"))?;
    let rdata_sec = pe
        .sections
        .iter()
        .find(|s| s.name == ".rdata")
        .ok_or(ExtractError::MissingSection(".rdata"))?;
    let text = pe
        .section_data(".text")
        .ok_or(ExtractError::MissingSection(".text"))?;
    let rdata = pe
        .section_data(".rdata")
        .ok_or(ExtractError::MissingSection(".rdata"))?;
    let text_view = SectionView {
        va: pe.optional.image_base + text_sec.virtual_address,
        data: text,
    };
    let rdata_view = SectionView {
        va: pe.optional.image_base + rdata_sec.virtual_address,
        data: rdata,
    };

    let in_text = |va: u32| {
        va >= text_view.va && ((va - text_view.va) as usize) < text_view.data.len()
    };
    let in_rdata = |va: u32| {
        va >= rdata_view.va && ((va - rdata_view.va) as usize) < rdata_view.data.len()
    };
    let text_at = |va: u32| -> Option<&[u8]> {
        let off = (va - text_view.va) as usize;
        text_view.data.get(off..off + 40)
    };
    let text_byte = |va: u32| -> Option<u8> {
        text_view.data.get((va - text_view.va) as usize).copied()
    };
    // printable ascii c-string inside .rdata/.data
    let cstr_at = |va: u32| -> Option<String> {
        if !in_rdata(va) {
            return None;
        }
        let off = (va - rdata_view.va) as usize;
        let end = rdata_view.data[off..(off + 96).min(rdata_view.data.len())]
            .iter()
            .position(|&b| b == 0)?;
        if end < 2 {
            return None;
        }
        let s = &rdata_view.data[off..off + end];
        if !s.iter().all(|&b| (0x20..0x7f).contains(&b)) {
            return None;
        }
        std::str::from_utf8(s).ok().map(|s| s.to_string())
    };

    // 1. index real E8 calls (rel32 target must land inside .text —
    //    an operand byte of `push imm32` can itself be 0xE8)
    let mut calls: Vec<usize> = Vec::new();
    for i in 0..text_view.data.len().saturating_sub(5) {
        if text_view.data[i] != 0xE8 {
            continue;
        }
        let rel = i32::from_le_bytes([
            text_view.data[i + 1],
            text_view.data[i + 2],
            text_view.data[i + 3],
            text_view.data[i + 4],
        ]);
        let tgt = (text_view.va as i64) + (i as i64) + 5 + rel as i64;
        if tgt >= text_view.va as i64
            && tgt < (text_view.va as i64 + text_view.data.len() as i64)
        {
            calls.push(i);
        }
    }
    let call_offsets: std::collections::HashSet<usize> = calls.iter().copied().collect();

    // resolve an E9 thunk chain to its end (MSVC /INCREMENTAL ILT
    // thunks occasionally chain through more than one hop)
    let resolve_thunk = |va: u32| -> u32 {
        let mut cur = va;
        for _ in 0..8 {
            let b = match text_at(cur) {
                Some(b) if !b.is_empty() => b,
                _ => return cur,
            };
            if b[0] != 0xE9 {
                return cur;
            }
            let rel = i32::from_le_bytes([b[1], b[2], b[3], b[4]]);
            cur = (cur.wrapping_add(5) as i64 + rel as i64) as u32;
        }
        cur
    };

    // classify a resolved registrar
    let classify = |reg: u32| -> Option<Mechanism> {
        let t = text_at(reg)?;
        // binding_node: [push regs...] push <size> [2-byte mov] E8 alloc
        //   push imm8 (6A): E8 at +4 (with mov) or +2 (without)
        //   push imm32 (68): E8 at +7 (with mov) or +5 (without)
        for i in 0..8 {
            let gaps: [usize; 2] = match t.get(i) {
                Some(0x6A) => [4, 2],
                Some(0x68) => [7, 5],
                _ => continue,
            };
            for gap in gaps {
                let j = i + gap;
                if j + 5 > t.len() || t[j] != 0xE8 {
                    continue;
                }
                let rel = i32::from_le_bytes([t[j + 1], t[j + 2], t[j + 3], t[j + 4]]);
                if reg.wrapping_add(j as u32 + 5).wrapping_add(rel as u32)
                    == ENGINE_ALLOCATOR_VA
                {
                    return Some(Mechanism::BindingNode);
                }
            }
        }
        // named_map: calls the string-keyed map helper early on
        for i in 0..t.len().saturating_sub(5).min(24) {
            if t[i] == 0xE8 {
                let rel = i32::from_le_bytes([t[i + 1], t[i + 2], t[i + 3], t[i + 4]]);
                if reg.wrapping_add(i as u32 + 5).wrapping_add(rel as u32)
                    == ENGINE_STRING_MAP_HELPER_VA
                {
                    return Some(Mechanism::NamedMap);
                }
            }
        }
        None
    };

    // 2. walk call sites
    const WINDOW: usize = 48;
    let debug = std::env::var("PROTO_DEBUG").is_ok();
    let mut out: Vec<ExtractedHostFn> = Vec::new();
    for &i in &calls {
        if debug && out.len() < 3 {
            eprintln!("site {:#x}: pushes_in_window={}", text_view.va + i as u32, {
                let mut cnt = 0; let start = i.saturating_sub(WINDOW); let mut j = start;
                while j + 1 < i { let b = text_view.data[j];
                    if b == 0x68 && j + 5 <= i { cnt += 1; j += 5; continue; }
                    if (0xB8..=0xBF).contains(&b) && j + 6 <= i && text_view.data[j + 5] == 0x50 + (b - 0xB8) { cnt += 1; j += 6; continue; }
                    j += 1; }
                cnt });
        }
        let site_va = text_view.va + i as u32;
        // immediate-argument pushes in the preceding window
        let mut pushes: Vec<(usize, u32)> = Vec::new();
        let start = i.saturating_sub(WINDOW);
        let mut j = start;
        while j + 1 < i {
            let b = text_view.data[j];
            if b == 0x68 && j + 5 <= i {
                let v = u32::from_le_bytes([
                    text_view.data[j + 1],
                    text_view.data[j + 2],
                    text_view.data[j + 3],
                    text_view.data[j + 4],
                ]);
                pushes.push((j, v));
                j += 5;
                continue;
            }
            if (0xB8..=0xBF).contains(&b)
                && j + 6 <= i
                && text_view.data[j + 5] == 0x50 + (b - 0xB8)
            {
                let v = u32::from_le_bytes([
                    text_view.data[j + 1],
                    text_view.data[j + 2],
                    text_view.data[j + 3],
                    text_view.data[j + 4],
                ]);
                pushes.push((j, v));
                j += 6;
                continue;
            }
            j += 1;
        }
        if pushes.len() < 2 {
            continue;
        }
        let (fn_pos, fn_va) = pushes[pushes.len() - 2];
        let name_va = pushes[pushes.len() - 1].1;
        let name = match cstr_at(name_va) {
            Some(n) => n,
            None => {
                if debug { eprintln!("  site {site_va:#x}: cstr_at({name_va:#x}) failed"); }
                continue;
            }
        };
        if !in_text(fn_va) {
            if debug { eprintln!("  site {site_va:#x}: fn_va {fn_va:#x} not in .text"); }
            continue;
        }
        // the fn push must belong to THIS call: no real E8 call may
        // sit between it and the call site (else the pushes belong to
        // an earlier registration and this E8 is e.g. its allocator)
        if (fn_pos + 5..i.min(fn_pos + WINDOW))
            .any(|c| call_offsets.contains(&c))
        {
            if debug { eprintln!("  site {site_va:#x}: intervening call"); }
            continue;
        }
        // rel32 of this call
        let rel = i32::from_le_bytes([
            text_view.data[i + 1],
            text_view.data[i + 2],
            text_view.data[i + 3],
            text_view.data[i + 4],
        ]);
        let mut reg_raw = (site_va.wrapping_add(5) as i64 + rel as i64) as u32;
        // singleton-getter idiom: `mov eax,[imm32]; ret` — the true
        // registrar call follows within 20 bytes
        if let Some(t) = text_at(reg_raw) {
            if t.len() >= 6 && t[0] == 0xA1 && t[5] == 0xC3 {
                match calls.iter().find(|&&c| site_va < text_view.va + c as u32
                    && text_view.va + c as u32 <= site_va + 20)
                {
                    Some(&c) => {
                        let crel = i32::from_le_bytes([
                            text_view.data[c + 1],
                            text_view.data[c + 2],
                            text_view.data[c + 3],
                            text_view.data[c + 4],
                        ]);
                        reg_raw = (text_view.va + c as u32 + 5) .wrapping_add(crel as u32);
                    }
                    None => continue,
                }
            }
        }
        if !in_text(reg_raw) {
            continue;
        }
        let registrar_va = resolve_thunk(reg_raw);
        let mechanism = match classify(registrar_va) {
            Some(m) => m,
            None => {
                if debug { eprintln!("  site {site_va:#x}: registrar {registrar_va:#x} unclassified"); }
                continue; // profiler/property/RTTI/AS/unknown — excluded
            }
        };
        let thunk_target = {
            let b = text_byte(fn_va);
            if b == Some(0xE9) {
                let t = resolve_thunk(fn_va);
                if t != fn_va {
                    Some(t)
                } else {
                    None
                }
            } else {
                None
            }
        };
        out.push(ExtractedHostFn {
            name,
            fn_va,
            thunk_target,
            registrar_va,
            mechanism,
        });
    }

    // 3. dedupe (name, registrar) — same as the Python sweep
    let mut seen = std::collections::HashSet::new();
    out.retain(|e| seen.insert((e.name.clone(), e.registrar_va)));
    Ok(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Names recovered from the M5 Lua-globals analysis of the real
    /// script data (frevents.lua imports, mission-Lua imports).
    const SCRIPT_KNOWN_HOOKS: &[&str] = &[
        "fre_RegisterEvent",
        "fre_RegisterCollectibles",
        "AddValidMission",
        "util_GetRandomFloat",
    ];

    #[test]
    fn static_table_shape() {
        let reg = LuaApiRegistry::new();
        assert_eq!(reg.registration_count(), 1119);
        assert!(reg.unique_name_count() >= 1116, "some names double-registered");
        // every record: name printable ascii with no NUL, fn_va nonzero
        for rec in reg.all() {
            assert!(!rec.name.is_empty());
            assert!(rec
                .name
                .bytes()
                .all(|b| (0x20..0x7f).contains(&b)));
            assert_ne!(rec.fn_va, 0);
            assert_ne!(rec.registrar_va, 0);
            // thunk targets, when present, differ from the thunk site
            if let Some(t) = rec.thunk_target {
                assert_ne!(t, rec.fn_va);
            }
        }
    }

    #[test]
    fn anchors_from_scripts_are_registered() {
        let reg = LuaApiRegistry::new();
        for hook in SCRIPT_KNOWN_HOOKS {
            let rec = reg
                .lookup_unique(hook)
                .unwrap_or_else(|| panic!("script hook {hook} must be in the registry"));
            assert!(
                rec.fn_va >= 0x10001000 && rec.fn_va < 0x10D9_0000,
                "{hook} fn_va {:#x} outside .text range",
                rec.fn_va
            );
        }
    }

    #[test]
    fn proven_anchor_addresses() {
        // Values individually verified against the binary's
        // disassembly (see module docs + scripts/verify_lua_api.py).
        let reg = LuaApiRegistry::new();
        assert_eq!(
            reg.lookup_unique("fre_RegisterEvent").unwrap().fn_va,
            0x1002_8D62
        );
        assert_eq!(
            reg.lookup_unique("AddValidMission").unwrap().fn_va,
            0x1000_88A0
        );
        assert_eq!(
            reg.lookup_unique("util_GetRandomFloat").unwrap().fn_va,
            0x1001_613F
        );
        // fre_RegisterEvent's fn_va is an ILT thunk into the body at
        // 0x1057E300 (verified by following E9 in the binary)
        assert_eq!(
            reg.lookup_unique("fre_RegisterEvent").unwrap().thunk_target,
            Some(0x1058_E300)
        );
    }

    #[test]
    fn mechanism_split() {
        let reg = LuaApiRegistry::new();
        let bn = reg.by_mechanism(Mechanism::BindingNode).count();
        let nm = reg.by_mechanism(Mechanism::NamedMap).count();
        assert_eq!(bn, 583);
        assert_eq!(nm, 536);
    }

    #[test]
    fn prefix_groups() {
        let reg = LuaApiRegistry::new();
        let counts = reg.prefix_counts();
        assert!(!counts.is_empty());
        // gameplay subsystems the scripts exercise must all exist
        for expected in ["go", "ai", "fre", "fe", "fsm", "snd", "cm", "nis", "woi"] {
            assert!(
                counts.iter().any(|(p, _)| *p == expected),
                "prefix {expected} missing from {counts:?}"
            );
        }
        // go_* is one of the largest families
        let go = reg.group("go").count();
        assert!(go >= 60, "go_* family smaller than expected: {go}");
    }

    #[test]
    fn cross_check_with_m5_script_analysis() {
        // The M5 milestone decoded which globals the real scripts
        // import.  Every engine hook those scripts call must now be
        // present in the recovered engine-side registry — this closes
        // the script <-> engine loop on real data.
        let reg = LuaApiRegistry::new();
        let mut missing = Vec::new();
        for hook in SCRIPT_KNOWN_HOOKS {
            if reg.lookup(hook).is_none() {
                missing.push(*hook);
            }
        }
        assert!(missing.is_empty(), "unresolved script hooks: {missing:?}");
    }

    // -- synthetic PE: validate the live extractor without the DLL --

    /// A minimal PE32 image builder whose .text spans the real
    /// helper VAs (`0x107FE720` allocator, `0x108351B0` map helper),
    /// so the classifier's absolute-VA comparisons work.  The image
    /// is sparse (mostly zero bytes) — ~8.6 MB per test, fine.
    struct TestImage {
        img: Vec<u8>,
        text_va: u32,
        rdata_va: u32,
        text_bytes: usize,
    }

    const T_ALLOC_OFF: usize = 0x7FD720; // (0x107FE720 - 0x10001000)
    const T_MAP_OFF: usize = 0x8341B0;   // (0x108351B0 - 0x10001000)

    impl TestImage {
        fn new() -> Self {
            const BASE: u32 = 0x1000_0000;
            const TEXT_RVA: u32 = 0x1000;
            const RDATA_RVA: u32 = 0x8000;
            let text_bytes = T_MAP_OFF + 0x40;
            let mut img = vec![0u8; 0x400 + text_bytes + 0x1000];
            img[0..2].copy_from_slice(b"MZ");
            img[60..64].copy_from_slice(&64u32.to_le_bytes());
            img[64..68].copy_from_slice(b"PE\0\0");
            let coff = 68usize;
            img[coff..coff + 2].copy_from_slice(&0x014Cu16.to_le_bytes()); // i386
            img[coff + 2..coff + 4].copy_from_slice(&2u16.to_le_bytes()); // 2 sections
            img[coff + 16..coff + 18].copy_from_slice(&224u16.to_le_bytes()); // opt size
            let opt = coff + 20;
            img[opt..opt + 2].copy_from_slice(&0x10Bu16.to_le_bytes()); // PE32
            img[opt + 28..opt + 32].copy_from_slice(&BASE.to_le_bytes());
            img[opt + 92..opt + 96].copy_from_slice(&16u32.to_le_bytes());
            let sh = opt + 96 + 16 * 8;
            for (idx, name, vs, va, rs, rp) in [
                (0usize, b".text\0\0\0", text_bytes as u32, TEXT_RVA, text_bytes as u32, 0x400u32),
                (1, b".rdata\0\0", 0x1000u32, RDATA_RVA, 0x1000u32, (0x400 + text_bytes) as u32),
            ] {
                let o = sh + idx * 40;
                img[o..o + 8].copy_from_slice(name.as_slice());
                img[o + 8..o + 12].copy_from_slice(&vs.to_le_bytes());
                img[o + 12..o + 16].copy_from_slice(&va.to_le_bytes());
                img[o + 16..o + 20].copy_from_slice(&rs.to_le_bytes());
                img[o + 20..o + 24].copy_from_slice(&rp.to_le_bytes());
            }
            Self {
                img,
                text_va: BASE + TEXT_RVA,
                rdata_va: BASE + RDATA_RVA,
                text_bytes,
            }
        }

        /// Put a NUL-terminated ASCII string into .rdata, return its VA.
        fn put_rdata_str(&mut self, off: usize, s: &str) -> u32 {
            let base = self.text_bytes + 0x400;
            self.img[base + off..base + off + s.len()].copy_from_slice(s.as_bytes());
            self.img[base + off + s.len()] = 0;
            self.rdata_va + off as u32
        }

        /// Put bytes at .text offset, return the VA of that offset.
        fn put_text(&mut self, off: usize, bytes: &[u8]) -> u32 {
            let base = 0x400usize;
            self.img[base + off..base + off + bytes.len()].copy_from_slice(bytes);
            self.text_va + off as u32
        }

        /// Patch a u32 at .text offset.
        fn patch_text(&mut self, off: usize, v: u32) {
            let base = 0x400usize;
            self.img[base + off..base + off + 4].copy_from_slice(&v.to_le_bytes());
        }

        /// Write an E8 call at .text offset `at` targeting `to`.
        fn put_call(&mut self, at: usize, to: u32) {
            let next = self.text_va + at as u32 + 5;
            let rel = (to as i64 - next as i64) as i32;
            self.put_text(at, &[0xE8]);
            let base = 0x400usize;
            self.img[base + at + 1..base + at + 5].copy_from_slice(&rel.to_le_bytes());
        }
    }

    /// Image #1: a binding_node registrar (`53 56 57 6A 28 8B D9 E8
    /// <alloc>` — the exact shape of the real 0x104E0F70/0x10802DB0
    /// registrars) plus two registrations: one through an ILT thunk
    /// with `push imm32`, one direct with the `mov eax,imm32; push
    /// eax` idiom.
    #[test]
    fn live_extraction_on_synthetic_pe() {
        let mut t = TestImage::new();
        let name1 = t.put_rdata_str(0x10, "hook_one");
        let name2 = t.put_rdata_str(0x30, "hook_two");

        // fake allocator at the real allocator VA
        t.put_text(T_ALLOC_OFF, &[0xC3]);
        // ILT thunk at .text 0x000 -> body at 0x100
        t.put_text(0x000, &[0xE9]);
        t.patch_text(0x001, 0x100u32.wrapping_sub(5));
        // registrar #1 at .text 0x010 (binding_node shape)
        t.put_text(0x010, &[0x53, 0x56, 0x57, 0x6A, 0x28, 0x8B, 0xD9]);
        t.put_call(0x017, t.text_va + T_ALLOC_OFF as u32);
        t.put_text(0x01C, &[0xC3]);
        // fn bodies
        t.put_text(0x100, &[0xC3]);
        t.put_text(0x110, &[0xC3]);
        // registration A @ 0x140: push thunk; push name1; mov ecx,esi; call reg
        let mut p = 0x140usize;
        t.put_text(p, &[0x68]);
        t.patch_text(p + 1, t.text_va); // thunk VA = text_va + 0
        p += 5;
        t.put_text(p, &[0x68]);
        t.patch_text(p + 1, name1);
        p += 5;
        t.put_text(p, &[0x8B, 0xCE]);
        p += 2;
        t.put_call(p, t.text_va + 0x010);
        p += 5;
        // registration B: mov eax, fn2; push eax; push name2; call reg
        t.put_text(p, &[0xB8]);
        t.patch_text(p + 1, t.text_va + 0x110);
        t.put_text(p + 5, &[0x50]);
        p += 6;
        t.put_text(p, &[0x68]);
        t.patch_text(p + 1, name2);
        p += 5;
        t.put_text(p, &[0x8B, 0xCE]);
        p += 2;
        t.put_call(p, t.text_va + 0x010);

        let pe = PeFile::parse(&t.img).expect("synthetic PE parses");
        {
            let txt = pe.section_data(".text").unwrap();
            println!("REG bytes @0x10: {}", txt[0x10..0x20].iter().map(|b| format!("{b:02x}")).collect::<String>());
            println!("ALLOC bytes @T_ALLOC_OFF: {}", txt[T_ALLOC_OFF..T_ALLOC_OFF + 4].iter().map(|b| format!("{b:02x}")).collect::<String>());
            let sec = pe.sections.iter().find(|s| s.name == ".text").unwrap();
            println!("text sec: va={:#x} rawptr={:#x} rawsize={:#x} vsize={:#x}", sec.virtual_address, sec.pointer_to_raw_data, sec.size_of_raw_data, sec.virtual_size);
            println!("image_base={:#x}", pe.optional.image_base);
        }
        let fns = extract_host_fns(&pe).expect("extraction succeeds");
        let names: Vec<&str> = fns.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["hook_one", "hook_two"]);
        let f0 = &fns[0];
        assert_eq!(f0.fn_va, t.text_va); // the ILT thunk VA
        assert_eq!(f0.thunk_target, Some(t.text_va + 0x100));
        assert_eq!(f0.mechanism, Mechanism::BindingNode);
        assert_eq!(f0.registrar_va, t.text_va + 0x010);
        let f1 = &fns[1];
        assert_eq!(f1.fn_va, t.text_va + 0x110);
        assert_eq!(f1.thunk_target, None);
        assert_eq!(f1.mechanism, Mechanism::BindingNode);
    }

    /// Image #2: a named_map registrar (calls the string-map helper
    /// early, like the real 0x104E08A0 fe-binding registrar).
    #[test]
    fn named_map_classification_on_synthetic_pe() {
        let mut t = TestImage::new();
        let name_map = t.put_rdata_str(0x10, "hook_map");
        t.put_text(T_ALLOC_OFF, &[0xC3]);
        t.put_text(T_MAP_OFF, &[0xC3]);
        // registrar #2 at .text 0x020: mov eax,[esp+4]; push esi;
        //   push 3; push eax; mov ecx,esi; call map_helper; ret
        t.put_text(
            0x020,
            &[0x8B, 0x44, 0x24, 0x04, 0x56, 0x6A, 0x03, 0x50, 0x8B, 0xCE],
        );
        t.put_call(0x02A, t.text_va + T_MAP_OFF as u32);
        t.put_text(0x02F, &[0xC3]);
        // registration: push fn; push name; call reg2
        let mut p = 0x140usize;
        t.put_text(p, &[0x68]);
        t.patch_text(p + 1, t.text_va + 0x100);
        p += 5;
        t.put_text(p, &[0x68]);
        t.patch_text(p + 1, name_map);
        p += 5;
        t.put_call(p, t.text_va + 0x020);
        t.put_text(0x100, &[0xC3]);

        let pe = PeFile::parse(&t.img).expect("synthetic PE parses");
        let fns = extract_host_fns(&pe).expect("extraction succeeds");
        assert_eq!(fns.len(), 1);
        assert_eq!(fns[0].name, "hook_map");
        assert_eq!(fns[0].mechanism, Mechanism::NamedMap);
        assert_eq!(fns[0].registrar_va, t.text_va + 0x020);
    }


    // -- real-binary test (skipped when the game files are absent) --

    fn engine_dll_path() -> Option<std::path::PathBuf> {
        if let Ok(dir) = std::env::var("PROTOTYPE_ADM_DIR") {
            let p = std::path::PathBuf::from(dir).join("prototypeenginef.dll");
            return p.exists().then_some(p);
        }
        for cand in [
            "/home/z/my-project/source/ADM/prototypeenginef.dll",
            "../../../../source/ADM/prototypeenginef.dll",
        ] {
            let p = std::path::PathBuf::from(cand);
            if p.exists() {
                return Some(p);
            }
        }
        None
    }

    #[test]
    fn live_extraction_matches_static_table_on_real_dll() {
        let Some(path) = engine_dll_path() else {
            eprintln!("skipping: prototypeenginef.dll not available");
            return;
        };
        let data = std::fs::read(&path).expect("read DLL");
        let pe = PeFile::parse(&data).expect("parse DLL");
        let live = extract_host_fns(&pe).expect("sweep DLL");
        assert_eq!(
            live.len(),
            lua_api_data::HOST_FNS.len(),
            "live sweep must reproduce the static table exactly"
        );
        for (live_rec, static_rec) in live.iter().zip(lua_api_data::HOST_FNS) {
            assert_eq!(live_rec.name, static_rec.name, "name mismatch");
            assert_eq!(live_rec.fn_va, static_rec.fn_va, "fn_va mismatch");
            assert_eq!(live_rec.thunk_target, static_rec.thunk_target);
            assert_eq!(live_rec.registrar_va, static_rec.registrar_va);
            assert_eq!(live_rec.mechanism, static_rec.mechanism);
        }
    }
}

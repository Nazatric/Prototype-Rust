# REA Evidence Report — Prototype (2009) Static Analysis

**Target:** Prototype (2009, Activision / Radical Entertainment) PC binaries
**Analysis date:** 2026-11-03
**Engine:** REA v6.8.0 (no decompiler configured — used raw PE + format parsing)
**Analyst:** prototype-rs workspace

---

## 1. Executive Summary

Static analysis of 15 files (31 MB total) extracted from a partial
Prototype PC install produced a fully working multi-crate Rust
workspace (`prototype-rs`) that parses every observed file format
clean-room. The Rust workspace has **31 passing unit + integration
tests**, compiles cleanly with `cargo check --workspace`, and the
`prototype-rs` CLI correctly identifies and inspects every fixture
file. The findings include: full Pure3D `.p3d` chunk-header layout,
the Radical "ATG CORE CEMENT LIBRARY" `.rcf` archive format,
confirmed Lua 5.1 scripting with float (not double) numbers, the
full RTTI class hierarchy of the engine DLL (11 837 classes
recovered), the engine's source-path leak
(`..\..\..\code\main\mainwin32.cpp`), and the original team credits
(Tim Bennison, Eric Holmes) recovered verbatim from
`scripts.rcf`. The name-hash algorithm used by RCF archives is
documented as **UNKNOWN** — it is not a plain CRC32 of any obvious
filename.

## 2. Target Profile

| Field | Value |
|---|---|
| Game | Prototype (2009) |
| Developer | Radical Entertainment (Vancouver) |
| Publisher | Activision |
| SKU | `vug_prototype_pc_na` (Vivendi Universal Games, PC, North America) |
| Languages | english, french, italian, spanish |
| Engine | Pure3D (Radical in-house, version 12) |
| Scripting | Lua 5.1 (compiled with `lua_Number = float`) |
| UI | Scaleform GFx (`scaleformp3d::FlashFileHandler`) |
| Audio | Bink (`binkw32.dll`) |
| Graphics | Direct3D 9 (`d3d9.dll`) |
| Input | XInput (`XINPUT1_3.dll`) |
| Networking | Winsock 2 + iphlpapi |

### Files analyzed (15 total, 31 MB)

| Path | Size | Format | Magic |
|---|---|---|---|
| `prototypef.exe` | 2.4 MB | PE32 | `MZ` |
| `prototypef.sls` | 2.4 MB | PE32 | `MZ` (renamed `.exe`) |
| `prototypeenginef.dll` | 20 MB | PE32 | `MZ` |
| `binkw32.dll` | 167 KB | PE32 | `MZ` |
| `vcredist_x86.exe` | 2.6 MB | PE32 | `MZ` (MSVC redistributable — not game code) |
| `cementfiles.p3d` | 338 B | Pure3D | `P3D\xff` |
| `skuinfo.p3d` | 184 B | Pure3D | `P3D\xff` |
| `art/frontendshaders.p3d` | 81 KB | Pure3D | `P3D\xff` |
| `audio/boot_strap.p3d` | 3.97 MB | Pure3D | `P3D\xff` |
| `scripts/statsachievements_tod.p3d` | 8.5 KB | Pure3D | `P3D\xff` |
| `scripts.rcf` | 182 KB | ATG Cement | `ATG CORE CEMENT LIBRARY` |
| `shaders.rcf` | 10.5 KB | ATG Cement | `ATG CORE CEMENT LIBRARY` |
| `scripts/frevents.lua` | 4.3 KB | Lua 5.1 bytecode | `\x1bLua\x51` |
| `10150_install.vdf` | 990 B | Steam VDF (text) | `"installscript"` |
| `proto_uv.dat` | 1 KB | Unknown (high entropy — likely encrypted/compressed UV map) | — |

## 3. Feature Trace

### 3.1 Engine boot sequence (proven)

The thin launcher `prototypef.exe` (2.4 MB) is essentially a Windows
GUI shell. It contains the literal strings
`EngineInitialize`, `EngineWindowFocus`, `EngineMouseService`,
`EngineKeyboardService`, `EngineService`, `EngineTerminate` —
**these are the service IDs** the launcher dispatches into
`prototypeenginef.dll` (20 MB). The original Radical source path is
leaked as a debug string in the executable: `..\..\..\code\main\mainwin32.cpp`.

The engine DLL exports 77 C++ mangled symbols (e.g.
`??0FlashFileHandler@scaleformp3d@@QAE@PBDHH@Z`,
`??0GBufferedFile@@IAE@XZ`, `??0GImage@@QAE@ABV0@@Z`) and imports
from 17 system DLLs — confirming the engine's subsystem stack:
Direct3D 9 for rendering, Bink for video, Scaleform GFx for UI,
Winsock + iphlpapi for networking, XInput for controllers.

### 3.2 Pure3D `.p3d` container format (proven)

Every `.p3d` file observed starts with the 16-byte file header:

```
magic[4]      = "P3D\xff"          (PROVEN — 5 of 5 samples)
version:u32   = 12                 (PROVEN — constant)
total_size:u32 = file size on disk  (PROVEN — matches every sample)
flags:u32      = 0x07f00000        (PROVEN for skuinfo, cementfiles)
                 0x0001100b        (PROVEN for frontendshaders)
                 0x000000fe        (PROVEN for boot_strap)
```

After the file header, the root chunk header is:

```
chunk_id:u32    — small per-class ID assigned by the engine factory
chunk_size:u32  — always file_size - 12 (= total_size - 12)
name_len:u8     — 1-byte length of the inline Pascal name
name[name_len]  — name string, may contain embedded NULs (trimmed at first)
```

Observed chunk IDs and names (PROVEN from fixtures):

| `chunk_id` | name | file |
|---|---|---|
| 0x4b | `engine::SkuInfo(1):SkuInfo` | `skuinfo.p3d` |
| 0x7b | `ContentCementFileDefinition(1):CementFileDefinition` | `cementfiles.p3d` |
| 0x41 | `CxformGauraud - lit - Fragment Program` | `art/frontendshaders.p3d` (Radical's typo preserved) |
| 0x9cb3 | `AudioFile` (sub-chunk) | `audio/boot_strap.p3d` (280 audio sub-chunks) |
| 0x5f | `proto::StatsManager(1):Stats Manager` | `scripts/statsachievements_tod.p3d` |

Inside the chunk body (after the inline name):
- 1-byte length-prefixed Pascal strings for the runtime class hierarchy
  (e.g. `SkuInfo\0`, `engine::SkuInfo\0` in `skuinfo.p3d`)
- A `u32` type-id field
- A `u32` CRC field (observed: `0x36a4173b` in `skuinfo.p3d`)
- A `u32` flags field (`0x07f00001` in `skuinfo.p3d`)
- A `u32` size field for the META sub-section
- The 4-byte ASCII marker `"META"` followed by a `u32` count and that
  many `u32`-length-prefixed strings (languages, file names, etc.)

### 3.3 Radical "ATG CORE CEMENT LIBRARY" `.rcf` archive format (proven)

Every `.rcf` file has the same 60-byte header followed by a 12-byte
directory entry array. Decoded field layout:

```
[ 0..31]  magic     = "ATG CORE CEMENT LIBRARY\0\0\0\0\0\0\0\0\0" (32 bytes)
[32..35]  version_field = 0x01000102 (constant)
[36..39]  dir_offset = 60 (constant)
[40..43]  dir_size_in_bytes = entry_count × 12
[44..47]  sector_size = 2048 (0x800) (constant)
[48..51]  name_table_offset = varies per file (region is zero-filled in samples)
[52..55]  reserved = 0
[56..59]  entry_count
[60..N]   directory (12 bytes per entry):
              name_hash:u32   (opaque — see UNKNOWN below)
              file_offset:u32  (aligned to sector_size)
              file_size:u32
```

`shaders.rcf` has 1 directory entry; `scripts.rcf` has 7 entries.
Each payload is one of:
- Pure3D chunk file (magic `P3D\xff`)
- Compiled Lua 5.1 bytecode (magic `\x1bLua`)
- Plain ASCII text (e.g. the original game credits)

### 3.4 Lua 5.1 scripting (proven)

`scripts/frevents.lua` and the Lua entries inside `scripts.rcf` are
all compiled Lua 5.1 bytecode. The 12-byte header is decoded:

| Offset | Size | Field | Observed | Note |
|---|---|---|---|---|
| 0 | 4 | magic | `\x1bLua` | |
| 4 | 1 | version | 0x51 | Lua 5.1 |
| 5 | 1 | format | 0x00 | official |
| 6 | 1 | endian | 0x01 | little-endian |
| 7 | 1 | sizeof(int) | 4 | |
| 8 | 1 | sizeof(size_t) | 4 | |
| 9 | 1 | sizeof(Instruction) | 4 | |
| 10 | 1 | sizeof(lua_Number) | **4** | **unusual** — Prototype uses `float`, not the standard `double` |
| 11 | 1 | integral_flag | 0 | `lua_Number` is floating-point |

This means any tool that wants to interoperate with Prototype's Lua
bytecode must be built with `LUA_NUMBER=float`. Stock Lua 5.1 will
mis-parse the constant pool.

### 3.5 Engine class hierarchy (proven — 11 837 RTTI classes)

The `.rdata` section of `prototypeenginef.dll` contains the
MSVC RTTI type-descriptor list. We extracted every class name
preceded by `.?AV` (class), `.?AU` (struct), or `.?AW` (interface/
forward-declared). Sample of the most gameplay-relevant:

**`engine::*` messages (the message-passing system):**
- `engine::Message` — base struct
- `engine::IntrusiveMessage`, `engine::NonintrusiveMessage` — refcount variants
- `engine::AnimationQueryLocomotionStateMessage`
- `engine::CharacterQueryMessage`, `CharacterQueryIntentionMessage`, `CharacterQueryCollisionStateMessage`
- `engine::SupportingLimbGetActiveMessageConst`
- `engine::StickToSupportingSurfaceMessage`
- `engine::SetIntersectionPropertiesMessage`
- `engine::PhysicsSetEnabledMessage`, `engine::PhysicsWakeUpMessage`
- `engine::GetCurrentDrawable`

**`proto::*` Prototype-specific gameplay:**
- `proto::TargetMessage`, `proto::TargetableMessage`, `proto::GetTargetMessage`, `proto::GetTargetPointMessage`
- `proto::Behaviour` (base)
- `proto::AnchorToGroundBehaviour`
- `proto::ButtonHintBehaviour`
- `proto::CharacterMotionBehaviour`
- `proto::ChargeBehaviour`
- `proto::CollisionActionBehaviour`
- `proto::FightVariableInt`, `proto::FightVariableName`, `proto::FightTimer`, `proto::FightVariablesBehaviour`
- `proto::GroundSpikeBehaviour`, `proto::GroundSpikeMasterBehaviour`, `proto::GroundSpikeShardBehaviour`
- `proto::HelicopterIntentionBehaviour`
- `proto::IKLimbInfo`, `proto::IKSupportingLimbInfo`, `proto::IKBehaviour`
- `proto::MotionState::State`, `proto::MotionStateBehaviour`
- `proto::SensesType`

**`content::*` reflection:**
- `content::MetaType` — reflection root
- `content::PrimitiveMetaType<bool>`, `content::PrimitiveMetaType<float>`
- `content::IBaseClassCaster`, `content::BaseClassCasterImpl<engine::Behaviour, proto::AnchorToGroundBehaviour>`
  (PROVES `AnchorToGroundBehaviour` derives from `engine::Behaviour`)

**Other:**
- `container::String` — engine's custom string type
- `scaleformp3d::FlashFileHandler` — Scaleform GFx integration for UI

The full RTTI dump is saved to `_inventory/deep/rtti_engine_dll.txt`.

### 3.6 Original team credits (proven — recovered from `scripts.rcf`)

`scripts.rcf` contains a plain-text entry whose first 80 bytes are:

```
EXECUTIVE PRODUCER\r\n      Tim Bennison\r\n\r\nLEAD GAME DESIGNER\r\n      Eric Holmes\r\n
```

Cross-referenced against Wikipedia and Radical Entertainment's
public history — both names match (Tim Bennison was Radical's
Executive Producer; Eric Holmes was Lead Game Designer on
Prototype). This is direct, primary-source evidence that the
archive is the unmodified original game data.

## 4. Evidence Log

| ID | Claim | Evidence | Confidence |
|---|---|---|---|
| E1 | `prototypef.exe` is a thin launcher that delegates to the engine DLL | Imports only KERNEL32/USER32/GDI32/ole32/OLEAUT32/MSVCR80/WS2_32; debug string `..\..\..\code\main\mainwin32.cpp`; service IDs `EngineInitialize`, `EngineWindowFocus`, `EngineMouseService`, `EngineKeyboardService`, `EngineService`, `EngineTerminate` | proven |
| E2 | Pure3D file header is 16 bytes (magic + version + total_size + flags) | Byte-exact match in 5 of 5 `.p3d` files; total_size always matches on-disk file size | proven |
| E3 | Pure3D root chunk header is 9 bytes (chunk_id:u32 + chunk_size:u32 + name_len:u8) + name | Byte-exact match in all 5 `.p3d` files; chunk_size always equals `total_size - 12` | proven |
| E4 | Pure3D chunk names contain a `Class(1):Class` pattern | `engine::SkuInfo(1):SkuInfo`, `ContentCementFileDefinition(1):CementFileDefinition`, `proto::StatsManager(1):Stats Manager` | proven |
| E5 | RCF header is 60 bytes (32-byte magic + 7×u32 fields) | Byte-exact match in both `.rcf` files; entry_count matches `dir_size_in_bytes / 12` | proven |
| E6 | RCF directory entries are 12 bytes (hash + offset + size) | Byte-exact match for all 8 total entries (1 in shaders.rcf, 7 in scripts.rcf); file_offset + file_size = end of payload fits within file | proven |
| E7 | RCF `name_hash` is NOT a plain CRC32 of any common filename | Tested CRC32 against 14 plausible names (scripts.rcf, frevents.lua, credits.txt, etc.) — none match the observed hashes | proven (negatively) |
| E8 | Engine uses compiled Lua 5.1 bytecode with `lua_Number = float` | `frevents.lua` byte 10 = 0x04 (sizeof_lua_number), byte 11 = 0x00 (integral_flag) | proven |
| E9 | Engine RTTI exposes a message-passing architecture | 11 837 class names recovered; `engine::Message` is a named base; `engine::IntrusiveMessage`/`NonintrusiveMessage` are refcount variants; concrete messages like `engine::PhysicsWakeUpMessage` follow the pattern | proven |
| E10 | `AnchorToGroundBehaviour` is a behaviour derived from `engine::Behaviour` | RTTI string `content::BaseClassCasterImpl<engine::Behaviour, proto::AnchorToGroundBehaviour>` in `prototypeenginef.dll` `.rdata` | proven |
| E11 | Engine integrates Scaleform GFx for UI | RTTI string `scaleformp3d::FlashFileHandler` and mangled exports `??0FlashFileHandler@scaleformp3d@@QAE@PBDHH@Z` | proven |
| E12 | Original game credits include Tim Bennison (Exec Producer) and Eric Holmes (Lead Game Designer) | Plain-text entry inside `scripts.rcf` starts with `EXECUTIVE PRODUCER\r\n      Tim Bennison\r\n\r\nLEAD GAME DESIGNER\r\n      Eric Holmes` | proven |

## 5. Reconstruction — `prototype-rs` Rust workspace

A 7-crate Cargo workspace at `/home/z/my-project/download/prototype-rs/`
implements clean-room parsers for every format above plus a typed
reconstruction of the engine's message-passing architecture.

```
prototype-rs/
├── Cargo.toml                    # workspace manifest
├── crates/
│   ├── prototype-common/          # Reader cursor, P3dError, magic constants
│   ├── prototype-p3d/             # Pure3D v12 chunk container parser
│   ├── prototype-rcf/             # ATG CORE CEMENT LIBRARY archive parser
│   ├── prototype-pe/              # PE32 header + section + import parser
│   ├── prototype-lua/             # Lua 5.1 bytecode header parser
│   ├── prototype-engine/          # Message/Behaviour typed reconstruction
│   └── prototype-bin/              # CLI: `prototype-rs <file>` auto-detects format
└── crates/prototype-bin/tests/fixtures/
    ├── skuinfo.p3d
    ├── cementfiles.p3d
    ├── scripts.rcf
    ├── shaders.rcf
    └── frevents.lua
```

Build: `cargo build --release`
Test: `cargo test --workspace` — **31 tests, all passing**
Inspect: `cargo run -p prototype-bin -- <path>`

The workspace:
- forbids `unsafe` in every crate (`#![forbid(unsafe_code)]`)
- never panics on malformed input — returns `Result<T, P3dError>` with
  precise offset + reason
- is documented at module level with byte-exact evidence tables
- separates **PROVEN** findings from **LIKELY** and **UNKNOWN** in
  every doc comment

## 6. Verification Results

| Test | Result |
|---|---|
| `cargo check --workspace` | passes |
| `cargo build --release` | passes |
| `cargo test --workspace` | 31 tests, 31 passing |
| CLI `prototype-rs skuinfo.p3d` | parses; reports META entries "english/french/italian/spanish" |
| CLI `prototype-rs cementfiles.p3d` | parses; asserts presence of `scripts.rcf`, `shaders.rcf`, `art.rcf`, `cells.rcf`, `movies.rcf`, `00woi.rcf`, `03woi.rcf`, `cached` |
| CLI `prototype-rs scripts.rcf` | parses 7 entries; auto-detects payload kinds (P3D, Lua51, Text) |
| CLI `prototype-rs shaders.rcf` | parses 1 entry; payload kind = P3D |
| CLI `prototype-rs frevents.lua` | parses; confirms `lua_Number = float` |
| CLI `prototype-rs prototypef.exe` | parses; reports 5 sections + 7 imported DLLs |
| CLI `prototype-rs prototypeenginef.dll` | parses; reports 6 sections + 17 imported DLLs (d3d9, binkw32, XINPUT1_3, etc.) |

## 7. Build Plan

To extend `prototype-rs` toward a playable engine reconstruction
(the original user's stated goal), the realistic next steps in
priority order:

1. **Lua 5.1 function-body parser** — extend `prototype-lua` to parse
   the chunk body (header is done, function bodies use the standard
   Lua 5.1 chunk format).  This unlocks reading the actual gameplay
   scripts (fight timers, target acquisition, button-hint triggers).
   Effort: ~1 week.

2. **Pure3D chunk tree parser** — extend `prototype-p3d` to walk the
   full chunk tree (currently we stop at the root chunk + META
   section).  Children include VertexBuffer, IndexBuffer, Mesh,
   Skeleton, Animation, Texture, Shader.  Effort: ~1 month.

3. **Engine message-router runtime** — extend `prototype-engine` with
   a small actor-like dispatcher that mirrors the original
   `MessageRouter` semantics.  Combined with a Lua VM (e.g. `mlua`
   built with `LUA_NUMBER=float`), this can execute the original
   gameplay scripts against reconstructed behaviour classes.  Effort:
   ~2 months.

4. **RCF name-hash function recovery** — disassemble the engine
   DLL's `CementLibrary::Lookup` function (likely lives in
   `prototypeenginef.dll`'s `.text` section near the `IBaseClassCaster`
   RTTI cluster).  This is the one UNKNOWN from the current analysis.
   Effort: ~1 week of disassembly with Ghidra or IDA (currently not
   available in this REA setup).

5. **Rendering and physics** — far beyond the scope of static
   analysis.  Would require either a full Pure3D renderer in Rust
   (targeting `wgpu` for Android + PC) or an interop layer that
   delegates to a D3D9-on-Vulkan translation layer.  Multi-year
   effort; not realistic without a team.

## 8. Unknowns & Limits

| Item | Status |
|---|---|
| RCF `name_hash` algorithm | UNKNOWN — proven NOT to be plain CRC32 of common filenames; would require disassembly of the engine's lookup function |
| `proto_uv.dat` format | UNKNOWN — first bytes are high-entropy; likely an encrypted/compressed UV-map blob (filename suggests "Prototype UV map") |
| P3D child chunk layout (beyond root) | KNOWN-OF but NOT YET IMPLEMENTED — root chunk header is proven, but the recursive chunk tree (VertexBuffer, Mesh, etc.) is not yet parsed by the Rust crate |
| Engine's actual subscription edges (which behaviour handles which message at runtime) | GUESSED from class-name semantics in `MessageRouter::prototype_default()`; the actual edges live in `.p3d` content files we do not have (the `audio/boot_strap.p3d` and others in the upload are too small to be the gameplay content) |
| Lua 5.1 function-body parsing | NOT IMPLEMENTED in `prototype-lua` (only the 12-byte header is parsed) |
| The 2009 Radical C++ source code | NOT AVAILABLE — the only source-path leak is `..\..\..\code\main\mainwin32.cpp` (a debug string in `prototypef.exe`); no source files were included in the upload |

## 9. Legal & License Boundaries

- The original game code and assets remain © Activision / Radical
  Entertainment.  This workspace does NOT reproduce executable code
  or original assets.
- The Rust code in this workspace is a clean-room re-implementation
  of the **file formats** and the **message/behaviour dispatch
  architecture** only.  It interoperates with legally-acquired game
  data; it does not replace the game.
- All class names recovered from RTTI are factual identifiers from
  the binary's type-descriptor section; they are not creative
  content.
- The team credits (Tim Bennison, Eric Holmes) are recovered from
  the unmodified game archive and are public historical fact.
- The Rust workspace is licensed under `MIT OR Apache-2.0` (the
  workspace's `Cargo.toml` declares this).

---

## 10. Addendum — block 6 (2026-11-03, ~16:40 UTC)

This addendum supersedes the stale counts in sections 1/6 (the
workspace is now 14 crates, ~23k lines, **271 tests / 0 failed /
0 ignored**) and records today's decisive evidence.

### 10.1 The RADP audio codec is SOLVED — standard IMA ADPCM

Evidence chain, each step independently checkable:

| # | Evidence | Method | Verdict |
|---|---|---|---|
| 1 | `audio::RadicalAdpcmDecoder`, `RadicalAdpcmFile`, `RadicalAdpcmFileDecoder` in the DLL RTTI (11 837-record dump) | static strings/RTTI | context: the engine ships an ADPCM decoder |
| 2 | All 70 RADP payload sizes are exact multiples of **20 bytes**, with odd block counts (20300 = 20×1015) | arithmetic on real payloads | block size = 20 bytes (not 40) |
| 3 | 191 295-block histogram: byte0 ∈ **exactly 0..=88**; byte1 ≡ 0; bytes 2..4 = little-endian i16 | byte histogram across every block | header = `u8 step_index, u8 reserved, i16 sample` |
| 4 | Decoding the 16 data bytes as 32 **low-nibble-first** IMA nibbles (published Intel/DVI step + index tables) reproduces every following block's header sample: **84 722/84 722 (100.0000%)** | full decode continuity sweep (Rust test `radp_v1_continuity_is_100_percent_on_all_59_clips`) | codec = standard IMA ADPCM, low-nibble-first |
| 5 | High-nibble-first scores 4.9% (chance); sequential (non-parity) decode of version-2 scores 1.2% | control experiments | the pass in #4 is not accidental |
| 6 | Version-2 payloads: even blocks chain 53 246/53 246 and odd blocks chain 53 246/53 246, each with its own step/predictor | parity continuity sweep | v2 = two interleaved mono streams (stereo LIKELY; L/R Pearson r +0.44…+0.96, non-identical except silent loops) |
| 7 | Rust decoder WAV output ≡ independent Python decoder WAV output, byte-for-byte, on real clips | `cmp` of outputs | no implementation drift |
| 8 | Silence (1214 fully-zero blocks) decodes to a perfectly flat line (index 0 → step 7 → delta 7>>3 = 0) | decode check | consistent with IMA semantics |

Still UNKNOWN: `format_tag` (0..=31) semantics — no correlation
with sample_rate or version; plausibly the DLL's
`MetaEnum W4WAVEFORMAT@audio@@`.  Whether the original decoder
trusts the header sample or chains the predictor is unobservable on
well-formed data (identical results).

Artifacts: `prototype-audio/src/radp.rs` (decoder + WAV writer +
all evidence tests), CLI `list-audio` / `decode-audio`, three
audible WAV proofs derived from the user's own files
(`radp_proof_*.wav` in the download dir, kept out of the repo).

### 10.2 Streaming is REAL now (M12)

`prototype-boot::streaming::StreamingMounts::apply` executes a
`LoadSet` against a `ContentPipeline`: pre-validation (missing
sources, double-mount guard) before mutation, reverse-canonical
unmount, canonical mount under the PROVEN `game:\*.rcf` file
names, rollback on mid-mount parse failure, append-only
`MountEvent` audit.  End-to-end test drives a real `frevents.lua`
runtime walking a player across a two-cell grid; pipeline
resolution appears/disappears with the mounts.

### 10.3 Missions are runtime-owned (M14)

`EngineRuntime::attach_mission_script` registers the real
2318-byte mission chunk (70 missions, 4 districts — same census as
the standalone runner); `start/complete/fail_mission` transition
`Available → Active → Completed|Failed` with frame-stamped
`MissionTransition` audit entries; invalid transitions are
rejected AND unlogged.  GameplayEnter deliberately does NOT reset
missions (frevents' reset is PROVEN only for the 60 fre events;
mission reset semantics UNKNOWN — not invented).

### 10.4 Audio path completed to the device edge (M15 + sink)

`prototype-audio::mixer` (voices, DS3D-style inverse-distance
listener, deterministic sum+clip mix, mono↔stereo fold-down) and
`prototype-audio::sink::RingSink` (fixed-capacity ring,
overwrite-oldest write policy, silence-fill read policy with
underrun accounting) — the exact piece a CPAL callback will pull
from on real hardware.

### 10.5 The full gameplay slice (M16)

`the_full_gameplay_slice_runs_end_to_end` (prototype-bin
integration tests): 4000 frames of mounts → scripts → player →
streaming transitions → missions → real RADP decode → 3D-attenuated
real-time mix, every subsystem asserted in one deterministic loop.

### 10.6 proto_uv.dat (first examination)

Exactly 1024 bytes; near-uniform high entropy (249 distinct byte
values; no magic, no structure, printable ratio 0.374).  In
`prototypeenginef.dll`'s strings it sits between `art/massive.p3d`,
`prototypepclive.quazal.net` (Quazal = the online service) and
`SOFTWARE\Activision\Prototype`.  Verdict: LIKELY an encrypted
validation/licensing blob owned by the online/DRM layer; contents
UNKNOWN; not needed for offline play.  No decode attempted — the
evidence is documented instead of guessed.

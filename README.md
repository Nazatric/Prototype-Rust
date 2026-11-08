# prototype-rs

Clean-room Rust reconstruction of the **file formats, script host,
engine runtime, world streaming, audio path, and boot handshake /
cement library / content pipeline** of **Prototype (2009)**,
reverse-engineered from the original PC binaries (`prototypef.exe`,
`prototypeenginef.dll`) and informed by the Phase 2C / 2D / 2F
analysis docs in the [Nazatric/PrototypeRecomp](https://github.com/Nazatric/PrototypeRecomp)
repository.

## What this is

A 14-crate Cargo workspace (~23k lines, all `#![forbid(unsafe_code)]`)
that parses every file format observed in the original game's PC
install, runs the recovered scripts in a safe Lua 5.1 VM, drives a
typed engine runtime (events, missions, input, player, streaming,
frames), decodes the game's RADP audio codec, and mixes PCM.  Built
without decompilers (REA was configured but had no Ghidra/IDA — all
parsing was done from raw byte inspection with `objdump`, `readelf`,
`pefile`, `capstone`, custom Python scripts, and the public Phase
2C/2D/2F research docs).  Every subsystem carries PROVEN / LIKELY /
UNKNOWN confidence labels in its doc comments; nothing is invented
silently.

## Crates

| Crate | Purpose |
|---|---|
| `prototype-common`  | Bounds-checked `Reader` cursor, `P3dError`, magic constants |
| `prototype-p3d`     | Pure3D v12 chunk container parser (exact-cover walk, AudioFile/ShaderProgram/content decoders) |
| `prototype-rcf`     | Radical "ATG CORE CEMENT LIBRARY" archive parser (name-hash recovery SOLVED) |
| `prototype-pe`      | PE32 header + section + import-table parser, RTTI harvest |
| `prototype-lua`     | Lua 5.1 function-body undumper + safe VM (closures, tables, host calls) |
| `prototype-math`    | Vec3 / Mat4 / Quat / Transform |
| `prototype-engine`  | MessageRouter, FreeRoam event system, MissionSystem, InputState, Player + camera, EngineRuntime (signals, frames, streaming, missions) |
| `prototype-render`  | Scene graph + SM 3.0 bytecode decoder + symbolic SM 3.0 → WGSL translator + pipeline descriptors |
| `prototype-audio`   | AudioClip catalog, **RADP codec decode (PROVEN IMA ADPCM)**, WAV writer, clean-room voice/mixer, **real device output (`cpal_out`, feature `audio-cpal` — M18)** |
| `prototype-input`   | InputState / InputEvent / Win32 VK mapping (PROVEN service IDs) |
| `prototype-stream`  | ArchiveKind (5 PROVEN RCFs + mount order), StreamingGrid, LoadSet diff |
| `prototype-boot`    | Boot handshake, cement gates, ContentPipeline (ordered RCF mounts) + streaming bridge (real mount/unmount) |
| `prototype-bin`     | CLI front-end (see below) |

## Build & test

```bash
cargo test --workspace                            # 283 tests, 0 failed, 0 ignored
cargo run -p prototype-bin -- <file>              # inspect any supported file
cargo run -p prototype-bin -- boot-status         # boot handshake reference
cargo run -p prototype-bin -- run-lua <file.lua>  # run a recovered script in the safe VM
cargo run -p prototype-bin -- fre-events <file>   # the 60 free-roam events
cargo run -p prototype-bin -- lua-api [<group>]   # recovered Lua host-function registry
cargo run -p prototype-bin -- list-audio <p3d>    # catalog RADP clips
cargo run -p prototype-bin -- decode-audio <p3d> <clip|all> <out.wav|dir>
                                                  # RADP → 16-bit WAV (PROVEN codec)

# REAL audio output (M18) — needs the `audio-cpal` feature:
cargo test -p prototype-audio --features audio-cpal
cargo run -p prototype-bin --features audio-cpal -- play-audio <p3d> <clip>
                                                  # play a clip through the device edge
```

On Linux the `audio-cpal` backend links libasound.  If the dev
headers are missing and you cannot install them (no root), build
alsa-lib into `~/.local` instead — one command, no privileges:

```bash
sh dev/alsa-local.sh && eval "$(sh dev/alsa-local.sh)"
```

Android/Windows need no extra provisioning (AAudio/WASAPI via the
same CPAL API; the callback code is host-agnostic).

## What's PROVEN (highlights)

### Formats (from the uploaded PC install)

- P3D v12: 12-byte file header + 12-byte chunk preludes; top-level
  chunks exactly cover every observed file; classic / content /
  shader payload families decoded.
- AudioFile chunks: 70 in `boot_strap.p3d`, each = clip-name list +
  RADP block (`radp`, `RADP`, version 1|2, sample_rate, format_tag,
  exact-length payload).
- **RADP codec = standard IMA ADPCM** (M13): 20-byte blocks
  (`u8 step_index 0..=88, u8 0, i16 predictor sample`, 16 data
  bytes = 32 low-nibble-first nibbles); version 1 = mono, 2 = two
  interleaved mono streams (stereo).  Verified at 100.0000%
  block-continuity across all 191 295 real blocks; Rust and Python
  decoders agree byte-for-byte; all 70 clips decode.
- RCF: 60-byte header + hash+offset+size directory; the
  `name_hash` algorithm was RECOVERED (M3) against the known
  plaintext; `scripts.rcf`'s directory is fully name-resolved
  (credits text, `frevents.lua`, the 2318-byte mission chunk, …).
- Lua 5.1 with `lua_Number = float`; full function-body undump
  (M2) and a safe VM (M8).
- ShaderProgram chunks: 24 programs (12 vs + 12 ps_3_0) with
  compiled SM 3.0 bytecode + inline HLSL; a bytecode decoder
  (CTAB, 11 opcodes, byte-packed parameter records) and a symbolic
  SM 3.0 → WGSL translator pass all 26 program decodes + a 26/26
  translation sweep.

### Engine runtime (RTTI + fixtures)

- 11 837 RTTI class names from the engine DLL; the audio
  subsystem's shape (`RadicalAdpcmDecoder`, `Mix`, `MixStrip`,
  `MixVoice`, renderer drivers XAudio2/DirectSound/WDM-KS/MME/ASIO,
  …) is RTTI-proven.
- `frevents.lua` exports `ResetFreeRoamEvents` /
  `DestroyFreeRoamEvents`; dispatching `GameplayEnter` /
  `SessionEnd` through the runtime invokes them (60 events reset /
  destroyed each).
- The real mission chunk registers **70 missions in 4 districts**
  (manhattan=63, minimal=4, loco_test_bed=2, manhattan_mini=1);
  the runtime owns the typed registry with frame-stamped
  transition audits.
- Engine service IDs, RCF mount order (`00cells → 01cells →
  00art → 01art → 02art`), startup package, `unlit` material.

### The full gameplay slice (M16)

One 4000-frame integration loop drives real RCF mounts →
`frevents.lua` lifecycle → 70-mission registration → input-driven
player movement → streaming diff with REAL archive mount/unmount
(player leaves `00cells`, enters `00art`, parks inside) → real
RADP decode → 3D-attenuated real-time mix — with runtime
bookkeeping and real mounts asserted in lock-step every frame.

## What's UNKNOWN

| Item | Status |
|---|---|
| Cell-grid spatial layout | Which cells live in `00cells` vs `01cells` at which world coordinates — the real cells archives are not in this sandbox. `StreamingGrid` currently uses one mega-cell per archive; populating real AABBs is a drop-in when the archives are available. |
| `format_tag` (0..=31) semantics | Block 38 engine walk: the RADP header parse pushes it UNCHANGED to a decoder-factory virtual (no dispatch, no table); NOT the codec (all 70 shipped clips decode as IMA ADPCM regardless of tag); the old `MetaEnum W4WAVEFORMAT@audio@@` lead DISSOLVED (that enum is the MmeRendererDriver output-format property). `version` is engine-PROVEN to be the interleaved STREAM count (`blocks = data_size/(20·version)`, `samples = blocks·32`). The tag's exact consumer is one virtual deeper — UNKNOWN, LIKELY a wave-format/mixing config. Decode does not depend on it. |
| `proto_uv.dat` | Exactly 1024 bytes, near-uniform high entropy, no internal structure; string-adjacent to the Quazal online hostname + Activision registry key — LIKELY an encrypted validation/licensing blob. Not needed offline. |
| Engine mix arithmetic | The mixer is a documented clean-room model (inverse-distance attenuation, linear sum, clip); the original's exact curve/effects (CompressorLimiter, Reverb settings) are UNKNOWN. |
| Mission ↔ fre-event cross-links | Which event completes which mission — needs script evidence not yet decoded. |
| Engine runtime subscription edges | `MessageRouter::prototype_default()` is guessed from class-name semantics; script-side bindings are PROVEN. |

See `PROGRESS.md` for the milestone ladder (currently 52%) and the
per-block work log, and `REA_EVIDENCE_REPORT.md` for the full
evidence log with confidence labels per finding.

## Legal

- Original game code and assets remain © Activision / Radical
  Entertainment.  This workspace is a clean-room re-implementation
  of the **file formats**, the **dispatch architecture**, the
  **codec identification**, and the **boot handshake reference
  model** only; it does not reproduce executable code or original
  assets.  The IMA ADPCM step/index tables are the published
  Intel/DVI constants.
- The Rust code is licensed under `MIT OR Apache-2.0`.
- The workspace interoperates with legally-acquired game data; it
  does not reproduce executable code or original assets.
- The Phase 2C / 2D / 2F findings quoted in `prototype-boot` are
  attributed to the `Nazatric/PrototypeRecomp` repository; users of
  this workspace should consult that project for the full context of
  those findings.

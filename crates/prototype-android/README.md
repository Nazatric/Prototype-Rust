# prototype-android

The Android platform layer of **prototype-rs** — a *loader-style* app in the
mould of cinnabar (Bedrock) and horizon (Minecraft Java): the APK ships
**no game assets**.  The user supplies their own legally-owned Prototype
(2009) files on first launch.

## First-launch UX

1. Install & launch the APK (built by `.github/workflows/build-apk.yml`).
2. The app asks for **All files access** (`MANAGE_EXTERNAL_STORAGE`) — needed
   to read the user's own files on shared storage.
3. It creates **`/sdcard/Prototype Rust/`**.
4. The user copies their own `Prototype.exe`, `prototypeenginef.dll`,
   `*.rcf`, `*.p3d`, `scripts/…` into that folder (from a PC over USB, or a
   file manager).
5. Re-open / resume the app: it re-scans on every `onResume`; once ≥ 1 `.rcf`
   is present the engine boot sequence starts.

## Modules

| Module | Gating | Purpose |
| --- | --- | --- |
| `asset_folder` | always | create / validate `Prototype Rust/` (unit-tested on every host) |
| `game_files` | always | classify + canonically order game files (PROVEN PC mount order) |
| `activity` | `android-activity` feature **and** Android target | NativeActivity entry, lifecycle, logcat |

## Building

```sh
# Desktop (tests only — no Android deps pulled):
cargo test -p prototype-android

# Device APK (needs NDK; or just push to GitHub and use the workflow):
cargo apk build -p prototype-android --features android-activity
```

Targets (API 24 / Android 7.0 min — chosen because `NativeActivity` +
`MANAGE_EXTERNAL_STORAGE` both need modern-but-not-latest Android, and
Android 7 still covers the vast majority of devices people actually game on):

| ABI | Rust triple | Status |
| --- | --- | --- |
| arm64-v8a | `aarch64-linux-android` | primary |
| armeabi-v7a | `armv7-linux-androideabi` | secondary |
| x86_64 | `x86_64-linux-android` | emulators |

## Legal

The game's code and assets remain © Activision / Radical Entertainment.
This crate contains none of either — it is a clean-room re-implementation
of file-format *behaviour* and expects the user to bring their own copy.

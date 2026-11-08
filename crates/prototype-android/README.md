# prototype-android

The Android platform layer of **prototype-rs** — a *loader-style* app in the
mould of cinnabar (Bedrock) and horizon (Minecraft Java): the APK ships
**no game assets**.  The user supplies their own legally-owned Prototype
(2009) files on first launch.

## First-launch UX (implemented)

1. Install & launch the APK (built by `.github/workflows/build-apk.yml`).
2. A fullscreen screen says **ALLOW ALL FILE ACCESS**.  There is no
   Play button and no hidden logcat-only state.
3. **Tap anywhere** on that screen → Android opens
   `ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION` for
   `com.nazatric.prototype_rs`.  Enable the toggle there.
4. Return to the app (`onResume`).  Only **after** the grant does the app
   create **`/sdcard/Prototype Rust/`**.  A second screen shows the exact
   path.
5. Copy your own `Prototype.exe`, `prototypeenginef.dll`, `*.rcf`,
   `*.p3d`, `scripts/…` **flat** into that folder (from a PC over USB or a
   file manager).
6. Re-open / resume the app: it re-scans on every `onResume`; once ≥ 1
   `.rcf` is present the engine boot sequence starts **automatically**.

`MANAGE_EXTERNAL_STORAGE` is a *special-access* permission: declaring it in
the manifest grants nothing on Android 11+, hence the Settings hop.  The
JNI bridge that does both the check (`Environment.isExternalStorageManager`)
and the Settings intent lives in `src/permission.rs`.  The five status
screens (ALLOW, WAITING, BOOTING, RUNNING, ERROR) are pre-rendered
**landscape 640×360** RGBA8 frames (`assets/screen_*.rgba`, a 5×7 bitmap
font) — no font, UI or widget crate is needed.  They normally draw through
the existing fullscreen WGSL movie quad; `software_screen` blits the same
pixels straight into the `ANativeWindow` if GPU bring-up fails, so a
broken Vulkan driver can never leave a black screen.

The Activity is locked to `sensorLandscape` in both the generated manifest
metadata and `AndroidManifest.xml`, and `permission::force_landscape` calls
`Activity.setRequestedOrientation(6)` at startup as a runtime backstop.
GPU bring-up and the engine boot are deferred out of `resumed()` onto a
redraw and are panic-contained; failures show an on-screen ERROR screen
with tap-to-retry instead of closing the app.

## Modules

| Module | Gating | Purpose |
| --- | --- | --- |
| `asset_folder` | always | create / validate `Prototype Rust/` (unit-tested on every host) |
| `game_files` | always | classify + canonically order game files (PROVEN PC mount order) |
| `permission` | always (JNI impl Android-only) | all-files-access check + Settings deep link + landscape lock |
| `permission_ui` | `winit-app` feature | the five landscape status screens |
| `software_screen` | `winit-app` feature | GPU-free `ANativeWindow` status-screen fallback |
| `app` | `winit-app` feature | shared winit handler: gate, folder, scan, auto-boot, render |
| `activity` | `android-activity` feature **and** Android target | NativeActivity entry, logcat, JNI handles |

## Building

```sh
# Desktop (tests only — no Android deps pulled):
cargo test -p prototype-android

# Device APK (needs NDK; or just push to GitHub and use the workflow):
cargo apk build -p prototype-android --features android-activity
```

Targets **API 30 / Android 11** minimum: `MANAGE_EXTERNAL_STORAGE`,
`Environment.isExternalStorageManager()` and the per-app Settings deep link
all start at Android 11.  The CI APK is built WITHOUT `audio-cpal` (no
Oboe/AAudio device stream) to keep startup on a single, stable path.

| ABI | Rust triple | Status |
| --- | --- | --- |
| arm64-v8a | `aarch64-linux-android` | shipped by CI (debug APK) |
| armeabi-v7a | `armv7-linux-androideabi` | not built by current workflow |
| x86_64 | `x86_64-linux-android` | not built by current workflow |

## Legal

The game's code and assets remain © Activision / Radical Entertainment.
This crate contains none of either — it is a clean-room re-implementation
of file-format *behaviour* and expects the user to bring their own copy.

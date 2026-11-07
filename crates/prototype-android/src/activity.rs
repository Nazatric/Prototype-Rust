//! NativeActivity entry point — the Android-specific shell around the
//! SHARED winit [`crate::app::App`] handler.
//!
//! Compiled ONLY when `feature = "android-activity"` is enabled AND the
//! target is Android: the native glue cannot link anywhere else.  On every
//! other host this module does not exist, which is why `cargo test
//! --workspace` stays green on Linux CI.
//!
//! Everything the app actually DOES — scan-or-setup, device-free shader
//! boot, wgpu surface + pipeline bring-up, the redraw loop, touch/mouse/
//! keyboard input — lives in [`crate::app`] and runs unchanged on the
//! desktop demo (`prototype-demo`'s `demo-render` bin).  This module only
//! supplies the three things that genuinely cannot leave Android:
//!
//! 1. the logcat sink ([`init_logging`]);
//! 2. the exported `android_main` symbol (unmangled, Rust-ABI —
//!    android-activity 0.6's contract; no proc-macro helper exists in
//!    0.6, the old `#[android_activity::main]` came from 0.5-era
//!    crates);
//! 3. winit's Android event-loop builder (`with_android_app`) — winit
//!    owns the event loop on Android (its `android-native-activity`
//!    backend wraps android-activity's poll loop — the two must never
//!    be driven separately).
//!
//! # Safety
//!
//! FFI with the Android native glue cannot be expressed in safe Rust;
//! this module is the ONE documented exception to the workspace-wide
//! deny(unsafe_code) rule (see PROGRESS.md "unsafe policy").  It is
//! compile-proven only in this sandbox (no NDK/GPU here); the first real
//! execution is the user's CI-built APK on a device.

#![cfg(feature = "android-activity")]
#![cfg(target_os = "android")]
// FFI with the Android native glue cannot be expressed in safe Rust; this
// module is the ONE documented exception to the workspace-wide
// deny(unsafe_code) rule (see PROGRESS.md "unsafe policy").
#![allow(unsafe_code)]

use android_activity::AndroidApp;
use android_logger::Config;
use winit::event_loop::EventLoop;
use winit::platform::android::EventLoopBuilderExtAndroid;

use crate::app::{scan_or_setup, App, AppState};
use crate::game_files;

/// Install the logcat sink tagged `prototype-rs`.
pub fn init_logging(app: &AndroidApp) {
    android_logger::init_once(
        Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("prototype-rs"),
    );
    let _ = app; // kept for future vm-visibility config
}

/// The NativeActivity entry point.
#[no_mangle]
pub fn android_main(app: AndroidApp) {
    init_logging(&app);
    log::info!("prototype-rs native activity starting (loader-style, no bundled assets)");

    let folder = crate::asset_folder::default_folder();
    // Pre-loop scan only for immediate logcat visibility; the App gets
    // the result handed over (no double scan) and re-scans on every
    // resume from here on.
    let state = scan_or_setup(&folder);
    log::info!("asset folder: {}", folder.display());
    match &state {
        AppState::Ready(set) => log::info!("ready: {}", game_files::missing_summary(set)),
        AppState::Setup { last_error, .. } => log::warn!(
            "setup: waiting for game files{}",
            last_error
                .as_deref()
                .map(|e| format!(" ({e})"))
                .unwrap_or_default()
        ),
    }

    let mut app_h = App::with_state(folder.clone(), state);
    let event_loop: EventLoop<()> = EventLoop::with_user_event()
        .with_android_app(app)
        .build()
        .expect("winit event loop");
    if let Err(e) = event_loop.run_app(&mut app_h) {
        log::error!("event loop terminated with error: {e}");
    }
    log::info!("prototype-rs native activity finished");
}

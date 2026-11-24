//! `demo-render` — the desktop demo binary (M26): the SAME winit
//! `App` handler the Android NativeActivity runs, driven by winit's
//! DESKTOP backends.
//!
//! ```text
//! cargo run -p prototype-demo --features render-wgpu
//! ```
//!
//! First end-to-end GPU proof point for the workspace: a real GPU
//! driver compiles the translated WGSL shader programs (one pipeline
//! per shader pair, built by the shared boot sequence over the user's
//! own files in `./Prototype Rust/`).  No APK, no NDK.
//!
//! This file is deliberately tiny: everything app-shaped lives in
//! [`prototype_android::app`] so the desktop and Android paths cannot
//! drift apart.

// Compiled only behind `render-wgpu` (see `[[bin]] required-features`),
// so the winit import below is always available when this file builds.
#![forbid(unsafe_code)]

fn main() {
    // stderr logging with `RUST_LOG` override (default: info).
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    let cfg = prototype_demo::default_config();
    log::info!("prototype-rs desktop demo (loader-style: the APK/bin ships no assets)");
    log::info!("asset folder: {}", cfg.folder.display());
    log::info!("copy your own Prototype PC files (*.rcf, *.p3d, scripts) there;");

    // The loader contract, identical to the Android activity: scan now
    // (creates the folder on first run), then let the shared App
    // handler re-scan and boot the renderer once files appear.
    let state = prototype_android::scan_or_setup(&cfg.folder);
    match &state {
        prototype_android::AppState::Ready(set) => {
            log::info!("ready: {}", prototype_android::game_files::missing_summary(set));
        }
        prototype_android::AppState::Setup { last_error, .. } => log::warn!(
            "setup: waiting for game files{}",
            last_error
                .as_deref()
                .map(|e| format!(" ({e})"))
                .unwrap_or_default()
        ),
    }

    let attrs = winit::window::Window::default_attributes()
        .with_title(cfg.title.clone())
        .with_inner_size(winit::dpi::LogicalSize::new(cfg.width, cfg.height));

    let mut app = prototype_android::App::with_state(cfg.folder.clone(), state)
        .with_window_attributes(attrs);

    let event_loop = winit::event_loop::EventLoop::with_user_event()
        .build()
        .expect("winit desktop event loop");
    if let Err(e) = event_loop.run_app(&mut app) {
        log::error!("event loop terminated with error: {e}");
        std::process::exit(1);
    }
}

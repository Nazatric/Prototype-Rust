//! prototype-demo — the DESKTOP proof binary for prototype-rs (M26).
//!
//! # What this crate is
//!
//! A thin shell around [`prototype_android`]'s shared winit
//! `ApplicationHandler` ([`prototype_android::app::App`]).  The SAME
//! handler that the NativeActivity entry point runs on Android is driven
//! here by winit's desktop backends (X11/Wayland): asset-folder
//! scan-or-setup → device-free shader boot → wgpu surface + one real
//! pipeline per shader pair → clear-frame loop with mouse/keyboard into
//! the engine's input snapshot.
//!
//! # Why it matters
//!
//! The sandbox has no GPU and no Android device, so every render-loop
//! claim is compile-proven only.  THIS bin is the first end-to-end proof
//! point the user can run on their own machine (`cargo run -p
//! prototype-demo --features render-wgpu`): a real GPU driver compiling
//! the translated WGSL shader programs, no APK or NDK required.  If
//! the desktop demo boots, the Android path differs only by the entry
//! point (see `prototype_android::activity`).
//!
//! # Loader contract (same as Android)
//!
//! The demo does NOT ship game assets.  It creates `./Prototype Rust/`
//! relative to the working directory (see
//! [`prototype_android::asset_folder::default_folder`]) and waits for
//! the user's own legally-owned `*.rcf` files to appear there; stderr
//! logs the exact missing-items summary until then.
//!
//! # Compile gating
//!
//! The lib target is feature-free (a [`DemoConfig`] plus its tests) so
//! the workspace's default build and every cross-compile audit stay
//! lean.  The `demo-render` bin and its winit/env_logger dependencies
//! exist only behind the `render-wgpu` feature.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::path::PathBuf;

/// Configuration for the desktop demo window + asset folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemoConfig {
    /// Window title.
    pub title: String,
    /// The user's asset folder (created on first run if missing).
    pub folder: PathBuf,
    /// Initial window width in logical pixels.
    pub width: u32,
    /// Initial window height in logical pixels.
    pub height: u32,
}

/// The default desktop demo configuration: a 1280x720 window titled
/// `prototype-rs` over the loader-contract folder `./Prototype Rust`
/// (desktop spelling of the Android `/sdcard/Prototype Rust/`).
///
/// 1280x720 was the PC game's most common fullscreen resolution era
/// default (DESIGN — a starting size, not recovered evidence).
pub fn default_config() -> DemoConfig {
    DemoConfig {
        title: "prototype-rs — desktop demo".to_string(),
        folder: prototype_android::asset_folder::default_folder(),
        width: 1280,
        height: 720,
    }
}

/// Block 110/111 (ported block 117): the evidence bins' shared
/// world-data loaders (placement dots, mesh diffuse resolution).
pub mod world_data;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_uses_the_loader_contract_folder() {
        let cfg = default_config();
        // On a desktop host the loader folder is the relative
        // `./Prototype Rust` (asset_folder's documented desktop path).
        assert_eq!(
            cfg.folder,
            std::path::PathBuf::from(".").join("Prototype Rust")
        );
    }

    #[test]
    fn default_config_is_a_sane_window() {
        let cfg = default_config();
        assert!(cfg.width >= 640 && cfg.height >= 480, "usable window");
        assert!(!cfg.title.is_empty());
    }

    #[test]
    fn config_is_cloneable_and_comparable() {
        let a = default_config();
        let b = a.clone();
        assert_eq!(a, b);
    }
}

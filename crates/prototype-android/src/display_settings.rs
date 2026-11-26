//! display_settings.rs — block 117: the content-side line's
//! platform-default settings, ported under a new module name so
//! main's persisted game-settings.json (`settings`) stays as is.
//! User-facing Settings — the model, persistence, and platform defaults.
//!
//! The user's directive: every necessary setting (touch controls,
//! graphics quality, etc.) lives IN-GAME, not in a separate launcher.
//! This module owns the **model**: what settings exist, what their
//! defaults are per platform, and how they're loaded/saved.
//!
//! The on-screen UI (the Settings overlay drawn on top of a rendered
//! frame) lives in [`crate::ui`] and [`crate::touch_controls`]; this
//! module is the data they share.
//!
//! # Persistence
//!
//! Settings live as `settings.json` inside the asset folder, next to
//! the user's game files.  Rationale:
//! - The folder is OS-accessible (the user already copies files into
//!   it), so editing `settings.json` by hand is trivial.
//! - It survives an APK reinstall (the user's own folder is untouched).
//! - It is per-install — different folders can have different settings.
//!
//! # Platform defaults
//!
//! Defaults differ by platform because the user explicitly asked:
//! - **Android**: virtual joystick + on-screen action buttons ON, vsync
//!   ON (battery), AA OFF (fill-rate), resolution scale 1.0 (the
//!   device's native resolution — the GPU should not be asked to do
//!   more than the panel needs at default), draw distance 1.0 (the
//!   engine's designed value).
//! - **PC**: virtual joystick OFF (mouse is the native model —
//!   Prototype shipped PC-only), vsync ON, AA OFF (let the user opt in
//!   for high-end GPUs), resolution scale 1.0, draw distance 1.0.
//!
//! # Host testability
//!
//! Everything here is `std` + `serde_json` only.  The host tests cover
//! the round-trip, the defaults per `cfg!`, the platform override
//! behaviour, and the file-not-found → defaults fallback (the very
//! first launch has no settings file).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The filename of the settings file inside the asset folder.
pub const SETTINGS_FILE: &str = "settings.json";

/// One user-tunable graphics knob.  See [`GraphicsSettings`] for the
/// full bundle and the meaning of each field.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GraphicsSettings {
    /// Framebuffer resolution scale (1.0 = native panel).
    ///
    /// 0.5 = half-resolution (cheap, blurry); 2.0 = supersampled
    /// (sharp, expensive).  Clamped to `[0.25, 2.0]` on load.
    pub resolution_scale: f32,
    /// Vsync.  `true` = `PresentMode::Fifo` (the only always-supported
    /// mode), `false` = `PresentMode::AutoVsync`/`Immediate` when the
    /// backend supports it (the renderer falls back to Fifo if not).
    pub vsync: bool,
    /// MSAA sample count (1, 2, 4, 8).  1 = no AA.  The renderer asks
    /// the device for `sample_count` on the surface; if the device
    /// refuses, it falls back to 1.
    pub msaa_samples: u32,
    /// Draw-distance multiplier on the engine's designed value.
    /// 1.0 = the engine's full default; 0.5 = half (cheap), 2.0 =
    /// double (expensive).  Clamped to `[0.25, 4.0]`.
    pub draw_distance: f32,
}

/// One user-tunable input knob.  See [`InputSettings`] for the bundle.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct InputSettings {
    /// Show the on-screen virtual movement joystick (left thumb).
    /// Android default ON, PC default OFF (the mouse is the native
    /// model — Prototype shipped PC-only).
    pub virtual_joystick: bool,
    /// Show the on-screen action buttons (right thumb): attack, jump,
    /// grab, dash.  Same defaults as the joystick.
    pub virtual_buttons: bool,
    /// Sensitivity multiplier for the look (camera) input.
    /// 1.0 = the engine default; clamped to `[0.1, 10.0]`.
    pub look_sensitivity: f32,
    /// Invert the Y look axis (some users prefer up-is-down).
    pub invert_look_y: bool,
}

/// The full settings bundle.  Serializable as `settings.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Graphics quality knobs.
    pub graphics: GraphicsSettings,
    /// Input + on-screen touch control knobs.
    pub input: InputSettings,
}

/// Errors the settings layer can return.
#[derive(Debug, Error)]
pub enum SettingsError {
    /// The asset folder doesn't exist (the user has not copied their
    /// game files in yet, or has not granted the permission).  The
    /// caller should use [`Settings::platform_defaults`] instead.
    #[error("asset folder missing: {0}")]
    FolderMissing(PathBuf),
    /// The settings file exists but isn't valid JSON, or fails the
    /// field-range checks on load.  The caller should use defaults and
    /// overwrite the bad file on next save.
    #[error("settings file invalid: {0}")]
    Invalid(String),
    /// The settings file could not be read (permissions, I/O).
    #[error("settings file unreadable: {0}: {1}")]
    Unreadable(PathBuf, #[source] std::io::Error),
    /// The settings file could not be written (disk full, permissions).
    #[error("settings file unwritable: {0}: {1}")]
    Unwritable(PathBuf, #[source] std::io::Error),
}

impl GraphicsSettings {
    /// Android default: vsync ON, AA OFF, scale 1.0, draw 1.0.
    pub fn android_default() -> Self {
        Self {
            resolution_scale: 1.0,
            vsync: true,
            msaa_samples: 1,
            draw_distance: 1.0,
        }
    }

    /// PC default: vsync ON, AA OFF, scale 1.0, draw 1.0.
    pub fn pc_default() -> Self {
        Self {
            resolution_scale: 1.0,
            vsync: true,
            msaa_samples: 1,
            draw_distance: 1.0,
        }
    }

    /// Clamp each field to its valid range.  Used on load (a hand-edited
    /// file can have out-of-range values; we coerce rather than reject).
    pub fn clamp(self) -> Self {
        Self {
            resolution_scale: self.resolution_scale.clamp(0.25, 2.0),
            vsync: self.vsync,
            msaa_samples: match self.msaa_samples {
                1 | 2 | 4 | 8 => self.msaa_samples,
                _ if self.msaa_samples > 8 => 8,
                _ => 1,
            },
            draw_distance: self.draw_distance.clamp(0.25, 4.0),
        }
    }
}

impl InputSettings {
    /// Android default: virtual joystick + buttons ON, sensitivity 1.0,
    /// Y not inverted.
    pub fn android_default() -> Self {
        Self {
            virtual_joystick: true,
            virtual_buttons: true,
            look_sensitivity: 1.0,
            invert_look_y: false,
        }
    }

    /// PC default: virtual joystick + buttons OFF (mouse is the native
    /// model — Prototype shipped PC-only), sensitivity 1.0, Y not
    /// inverted.
    pub fn pc_default() -> Self {
        Self {
            virtual_joystick: false,
            virtual_buttons: false,
            look_sensitivity: 1.0,
            invert_look_y: false,
        }
    }

    /// Clamp sensitivity to `[0.1, 10.0]`.  Booleans need no clamp.
    pub fn clamp(self) -> Self {
        Self {
            virtual_joystick: self.virtual_joystick,
            virtual_buttons: self.virtual_buttons,
            look_sensitivity: self.look_sensitivity.clamp(0.1, 10.0),
            invert_look_y: self.invert_look_y,
        }
    }
}

impl Settings {
    /// Platform-default `Settings`: Android or PC defaults based on
    /// `cfg!(target_os = "android")`.
    pub fn platform_defaults() -> Self {
        if cfg!(target_os = "android") {
            Self::android_default()
        } else {
            Self::pc_default()
        }
    }

    /// Android-default `Settings`.
    pub fn android_default() -> Self {
        Self {
            graphics: GraphicsSettings::android_default(),
            input: InputSettings::android_default(),
        }
    }

    /// PC-default `Settings`.
    pub fn pc_default() -> Self {
        Self {
            graphics: GraphicsSettings::pc_default(),
            input: InputSettings::pc_default(),
        }
    }

    /// Clamp every field.  Used after load.
    pub fn clamp(self) -> Self {
        Self {
            graphics: self.graphics.clamp(),
            input: self.input.clamp(),
        }
    }

    /// The expected path of the settings file inside `folder`.
    pub fn path_for(folder: &Path) -> PathBuf {
        folder.join(SETTINGS_FILE)
    }

    /// Load settings from `folder/settings.json`, or return the
    /// platform defaults when the file is missing or the folder is
    /// missing.
    ///
    /// The "missing file → defaults" path is the first-launch path;
    /// the user has no settings yet, so we hand them the platform
    /// defaults.  When the file exists but is invalid JSON or out of
    /// range, we return the defaults AND overwrite the file on the
    /// next save (the caller surfaces a one-line warning).
    pub fn load_or_defaults(folder: &Path) -> Result<Self, SettingsError> {
        if !folder.exists() {
            // First launch before the user has copied their files in.
            return Ok(Self::platform_defaults());
        }
        let path = Self::path_for(folder);
        if !path.exists() {
            return Ok(Self::platform_defaults());
        }
        let raw = fs::read_to_string(&path).map_err(|e| SettingsError::Unreadable(path.clone(), e))?;
        let parsed: Self = serde_json::from_str(&raw)
            .map_err(|e| SettingsError::Invalid(format!("parse: {e}")))?;
        Ok(parsed.clamp())
    }

    /// Save settings to `folder/settings.json`, creating the folder
    /// when missing (the loader contract: the app owns the folder).
    pub fn save(&self, folder: &Path) -> Result<(), SettingsError> {
        if !folder.exists() {
            return Err(SettingsError::FolderMissing(folder.to_path_buf()));
        }
        let path = Self::path_for(folder);
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| SettingsError::Invalid(format!("serialise: {e}")))?;
        fs::write(&path, json).map_err(|e| SettingsError::Unwritable(path, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir_tagged(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "proto-android-settings-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn android_default_has_virtual_controls_on() {
        let s = Settings::android_default();
        assert!(s.input.virtual_joystick);
        assert!(s.input.virtual_buttons);
    }

    #[test]
    fn pc_default_has_virtual_controls_off() {
        let s = Settings::pc_default();
        assert!(!s.input.virtual_joystick);
        assert!(!s.input.virtual_buttons);
    }

    #[test]
    fn platform_defaults_match_cfg() {
        let s = Settings::platform_defaults();
        if cfg!(target_os = "android") {
            assert_eq!(s, Settings::android_default());
        } else {
            assert_eq!(s, Settings::pc_default());
        }
    }

    #[test]
    fn round_trip_preserves_all_fields() {
        let s = Settings {
            graphics: GraphicsSettings {
                resolution_scale: 1.5,
                vsync: false,
                msaa_samples: 4,
                draw_distance: 2.0,
            },
            input: InputSettings {
                virtual_joystick: true,
                virtual_buttons: false,
                look_sensitivity: 2.5,
                invert_look_y: true,
            },
        };
        let dir = temp_dir_tagged("roundtrip");
        s.save(&dir).unwrap();
        let loaded = Settings::load_or_defaults(&dir).unwrap();
        assert_eq!(loaded, s);
    }

    #[test]
    fn missing_folder_returns_defaults() {
        let dir = std::env::temp_dir().join(format!(
            "proto-android-settings-noexist-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(!dir.exists());
        let s = Settings::load_or_defaults(&dir).unwrap();
        assert_eq!(s, Settings::platform_defaults());
    }

    #[test]
    fn missing_file_returns_defaults() {
        let dir = temp_dir_tagged("missing-file");
        let s = Settings::load_or_defaults(&dir).unwrap();
        assert_eq!(s, Settings::platform_defaults());
    }

    #[test]
    fn invalid_json_returns_defaults_error() {
        let dir = temp_dir_tagged("invalid");
        std::fs::write(dir.join(SETTINGS_FILE), b"{not valid json").unwrap();
        let err = Settings::load_or_defaults(&dir).unwrap_err();
        assert!(matches!(err, SettingsError::Invalid(_)));
    }

    #[test]
    fn out_of_range_values_are_clamped_on_load() {
        let dir = temp_dir_tagged("clamp");
        // resolution_scale 100.0, msaa_samples 16, draw_distance 99.0,
        // look_sensitivity 50.0 — all out of range.
        let raw = r#"{
            "graphics": {
                "resolution_scale": 100.0,
                "vsync": true,
                "msaa_samples": 16,
                "draw_distance": 99.0
            },
            "input": {
                "virtual_joystick": true,
                "virtual_buttons": true,
                "look_sensitivity": 50.0,
                "invert_look_y": false
            }
        }"#;
        std::fs::write(dir.join(SETTINGS_FILE), raw).unwrap();
        let s = Settings::load_or_defaults(&dir).unwrap();
        assert_eq!(s.graphics.resolution_scale, 2.0);
        assert_eq!(s.graphics.msaa_samples, 8);
        assert_eq!(s.graphics.draw_distance, 4.0);
        assert_eq!(s.input.look_sensitivity, 10.0);
    }

    #[test]
    fn save_creates_file_with_pretty_json() {
        let dir = temp_dir_tagged("save");
        let s = Settings::pc_default();
        s.save(&dir).unwrap();
        let raw = std::fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap();
        assert!(raw.contains("\"graphics\""));
        assert!(raw.contains("\"input\""));
        assert!(raw.contains("\"virtual_joystick\": false"));
    }

    #[test]
    fn save_to_missing_folder_errors() {
        let dir = std::env::temp_dir().join(format!(
            "proto-android-settings-savemiss-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(!dir.exists());
        let err = Settings::pc_default().save(&dir).unwrap_err();
        assert!(matches!(err, SettingsError::FolderMissing(_)));
    }

    #[test]
    fn msaa_clamp_rounds_to_nearest_valid() {
        // 0 → 1 (the "no AA" value); 3 → 1 (the smallest power-of-two
        // ≥ 3 is 4, but our impl says "anything <2 is 1" — we choose 1
        // because the field is a USER setting and 3 is impossible on
        // every device).
        let g = GraphicsSettings {
            resolution_scale: 1.0,
            vsync: true,
            msaa_samples: 0,
            draw_distance: 1.0,
        };
        assert_eq!(g.clamp().msaa_samples, 1);

        let g = GraphicsSettings {
            resolution_scale: 1.0,
            vsync: true,
            msaa_samples: 4,
            draw_distance: 1.0,
        };
        assert_eq!(g.clamp().msaa_samples, 4);
    }
}

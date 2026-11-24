//! Block 107 — the game settings, persisted in the asset folder as
//! JSON (`game-settings.json`) and WIRED to real engine behaviour:
//! the streaming budget feeds `MemberStreamConfig`, the mesh cap feeds
//! the per-frame load budget, the volume feeds the audio mixer, the
//! look sensitivity feeds the camera.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// The user-tunable settings (the real Options menu's page set).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Streaming budget per frame, MB of decompressed member data.
    pub stream_budget_mb: u32,
    /// Max mesh batches decoded per frame from the stream.
    pub mesh_cap_per_frame: u32,
    /// Master volume 0.0..=1.0.
    pub master_volume: f32,
    /// Touch/mouse look sensitivity multiplier.
    pub look_sensitivity: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            stream_budget_mb: 24,
            mesh_cap_per_frame: 6,
            master_volume: 0.8,
            look_sensitivity: 1.0,
        }
    }
}

/// Load from the game folder; ANY problem (missing, corrupt, partial)
/// falls back to defaults — settings must never block a boot.
pub fn load(folder: &Path) -> Settings {
    let path = folder.join("game-settings.json");
    match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("settings parse failed ({e}) — defaults");
                Settings::default()
            }
        },
        Err(_) => Settings::default(),
    }
}

/// Save atomically (tmp + rename); failures are logged non-fatally.
pub fn save(folder: &Path, s: &Settings) {
    let path = folder.join("game-settings.json");
    let tmp = folder.join(".game-settings.json.tmp");
    match serde_json::to_string_pretty(s) {
        Ok(text) => {
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
        Err(e) => log::warn!("settings serialize failed (non-fatal): {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join("proto-settings-107");
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The roundtrip preserves every knob, and a corrupt file falls
    /// back to defaults (fail-open — settings never block a boot).
    #[test]
    fn settings_roundtrip_and_fail_open() {
        let dir = tempdir();
        let mut s = Settings::default();
        s.stream_budget_mb = 48;
        s.master_volume = 0.5;
        save(&dir, &s);
        let back = load(&dir);
        assert_eq!(back, s);
        // Corrupt → defaults.
        std::fs::write(dir.join("game-settings.json"), "{ not json").unwrap();
        assert_eq!(load(&dir), Settings::default());
        // Missing → defaults.
        std::fs::remove_file(dir.join("game-settings.json")).unwrap();
        assert_eq!(load(&dir), Settings::default());
    }
}

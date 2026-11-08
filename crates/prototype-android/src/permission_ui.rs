//! The five **first-launch / boot status screens**.
//!
//! The APK ships no font/glyph/UI crate and no Java layout.  Each screen
//! is a pre-rendered 640×360 RGBA8 image (a 5×7 bitmap font rendered
//! offline into `assets/`).  They are normally drawn through the wgpu
//! fullscreen quad that the game itself uses; when GPU bring-up fails,
//! the GPU-INDEPENDENT blitter in [`crate::software_screen`] writes the
//! same pixels straight into the `ANativeWindow` buffer, so a
//! missing/broken Vulkan driver can no longer turn the app into a black
//! screen or a crash.
//!
//! 640×360 is landscape (16:9) to match the locked sensor-landscape
//! orientation; the blitter nearest-neighbour scales it to the real
//! window size, so it stays readable on any phone.

/// Which status screen is currently on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionScreenKind {
    /// All-files access has NOT been granted.  Tapping opens Settings.
    Grant,
    /// Access was granted and the folder exists, but no `.rcf` files
    /// were found yet: tell the user where to copy them.
    Waiting,
    /// Game files found — the (heavy) engine boot is about to run.
    Booting,
    /// Boot succeeded; short confirmation before the render loop.
    Running,
    /// Boot failed or panicked; tap to retry.  The app stays alive.
    Error,
}

/// Width of every pre-rendered screen (RGBA8, tightly packed).
pub const SCREEN_WIDTH: u32 = 640;
/// Height of every pre-rendered screen (RGBA8, tightly packed).
pub const SCREEN_HEIGHT: u32 = 360;

const GRANT: &[u8] = include_bytes!("../assets/screen_grant.rgba");
const WAITING: &[u8] = include_bytes!("../assets/screen_waiting.rgba");
const BOOTING: &[u8] = include_bytes!("../assets/screen_booting.rgba");
const RUNNING: &[u8] = include_bytes!("../assets/screen_running.rgba");
const ERROR: &[u8] = include_bytes!("../assets/screen_error.rgba");

/// Return the tightly-packed RGBA8 pixels and dimensions for `kind`.
///
/// Always `4 * SCREEN_WIDTH * SCREEN_HEIGHT` bytes.
pub fn screen(kind: PermissionScreenKind) -> (&'static [u8], u32, u32) {
    let rgba = match kind {
        PermissionScreenKind::Grant => GRANT,
        PermissionScreenKind::Waiting => WAITING,
        PermissionScreenKind::Booting => BOOTING,
        PermissionScreenKind::Running => RUNNING,
        PermissionScreenKind::Error => ERROR,
    };
    (rgba, SCREEN_WIDTH, SCREEN_HEIGHT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_screen_is_exactly_one_landscape_rgba_frame() {
        for kind in [
            PermissionScreenKind::Grant,
            PermissionScreenKind::Waiting,
            PermissionScreenKind::Booting,
            PermissionScreenKind::Running,
            PermissionScreenKind::Error,
        ] {
            let (rgba, w, h) = screen(kind);
            assert_eq!((w, h), (640, 360));
            assert!(w > h, "screens must be landscape");
            assert_eq!(rgba.len(), (w * h * 4) as usize);
            assert!(rgba.iter().any(|b| *b != 0), "screen must not be blank");
        }
    }

    #[test]
    fn package_id_matches_the_deep_link() {
        assert_eq!(crate::permission::PACKAGE, "com.nazatric.prototype_rs");
    }
}

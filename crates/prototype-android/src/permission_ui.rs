//! The two **first-launch screens** shown before the game can boot.
//!
//! The APK deliberately ships no font/glyph/UI crate and no Android Java
//! layout: the only thing it *can* draw is the existing fullscreen
//! WGSL movie quad (one RGBA8 texture, one triangle).  So the setup UI
//! is two pre-rendered 360×640 RGBA8 images (a 5×7 bitmap font, rendered
//! offline into `assets/`) that the [`crate::app::App`] uploads into a
//! video texture and presents through exactly the same pipeline the
//! boot movie uses.  No new dependency, no text renderer, no JNI view.
//!
//! The images are stretched to the window by the quad shader (same as a
//! movie frame); portrait phones therefore see them essentially 1:1.

/// Which setup screen is currently on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionScreenKind {
    /// All-files access has NOT been granted.  Tapping opens Settings.
    Grant,
    /// Access was granted and the folder exists, but no `.rcf` files
    /// were found yet: tell the user where to copy them.
    Waiting,
}

/// Width of both pre-rendered screens (RGBA8, tightly packed).
pub const SCREEN_WIDTH: u32 = 360;
/// Height of both pre-rendered screens (RGBA8, tightly packed).
pub const SCREEN_HEIGHT: u32 = 640;

const GRANT_RGBA: &[u8] = include_bytes!("../assets/permission_grant.rgba");
const WAITING_RGBA: &[u8] = include_bytes!("../assets/permission_waiting.rgba");

/// Return the tightly-packed RGBA8 pixels and dimensions for `kind`.
///
/// The returned slice is `4 * SCREEN_WIDTH * SCREEN_HEIGHT` bytes, which
/// is exactly what [`prototype_render::wgpu_backend::Renderer::upload_video_frame`]
/// expects for a texture of these dimensions.
pub fn screen(kind: PermissionScreenKind) -> (&'static [u8], u32, u32) {
    let rgba = match kind {
        PermissionScreenKind::Grant => GRANT_RGBA,
        PermissionScreenKind::Waiting => WAITING_RGBA,
    };
    (rgba, SCREEN_WIDTH, SCREEN_HEIGHT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_screens_are_exactly_one_rgba_frame() {
        for kind in [PermissionScreenKind::Grant, PermissionScreenKind::Waiting] {
            let (rgba, w, h) = screen(kind);
            assert_eq!((w, h), (360, 640));
            assert_eq!(rgba.len(), (w * h * 4) as usize);
            assert!(rgba.iter().any(|b| *b != 0), "screen must not be blank");
        }
    }

    #[test]
    fn package_id_matches_the_deep_link() {
        assert_eq!(crate::permission::PACKAGE, "com.nazatric.prototype_rs");
    }
}

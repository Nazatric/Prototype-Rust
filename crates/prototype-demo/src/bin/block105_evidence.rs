// Block 105 evidence renderer — drives the REAL SettingsOverlay,
// SetupScreen, and TouchControls code to produce one PNG per frame.
// Compile + run from the workspace root:
//   cargo run --release -p prototype-demo --bin block105_evidence
use prototype_android::display_settings::Settings;
use prototype_android::ui::{SetupScreen, SettingsOverlay};
use prototype_android::touch_controls::TouchControls;
use prototype_video::png::png_encode_rgba;

fn write_png(path: &str, w: u32, h: u32, fb: &[u8]) {
    let bytes = png_encode_rgba(w, h, fb).expect("png encode");
    std::fs::write(path, bytes).expect("write");
}

fn game_frame(w: u32, h: u32) -> Vec<u8> {
    let mut fb = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let o = ((y * w + x) * 4) as usize;
            let day = (y as f32 / h as f32) < 0.5;
            let r = if day { (x as u32 * 255 / w) as u8 } else { 40 };
            let g = if day { (y as u32 * 200 / h) as u8 } else { 80 };
            let b = if day { 100 } else { 60 };
            fb[o] = r;
            fb[o + 1] = g;
            fb[o + 2] = b;
            fb[o + 3] = 255;
        }
    }
    fb
}

fn dim_screen(w: u32, h: u32, shade: u8) -> Vec<u8> {
    let mut fb = vec![0u8; (w * h * 4) as usize];
    for px in fb.chunks_exact_mut(4) {
        px[0] = shade;
        px[1] = shade;
        px[2] = shade.saturating_add(10);
        px[3] = 255;
    }
    fb
}

fn main() {
    let w = 640u32;
    let h = 480u32;
    std::fs::create_dir_all("/tmp/block105_frames").unwrap();

    // Phase 1 (0-30): Setup screen — first launch, no game files.
    let setup = SetupScreen::new(w, h);
    for f in 0..30 {
        let mut fb = dim_screen(w, h, 10);
        setup.paint(&mut fb, w, h);
        write_png(&format!("/tmp/block105_frames/f_{:04}.png", f), w, h, &fb);
    }

    // Phase 2 (30-60): User granted permission; folder empty.
    for f in 30..60 {
        let mut fb = dim_screen(w, h, 30);
        setup.paint(&mut fb, w, h);
        write_png(&format!("/tmp/block105_frames/f_{:04}.png", f), w, h, &fb);
    }

    // Phase 3 (60-90): Game loaded — touch controls overlay ACTIVE.
    for f in 60..90 {
        let mut fb = game_frame(w, h);
        let tc = TouchControls::new(w, h, &Settings::android_default().input);
        tc.paint(&mut fb);
        write_png(&format!("/tmp/block105_frames/f_{:04}.png", f), w, h, &fb);
    }

    // Phase 4 (90-120): Settings overlay OPEN (P key).
    let overlay = SettingsOverlay::new(w, h, Settings::android_default());
    for f in 90..120 {
        let mut fb = game_frame(w, h);
        let tc = TouchControls::new(w, h, &Settings::android_default().input);
        tc.paint(&mut fb);
        overlay.paint(&mut fb, w, h);
        write_png(&format!("/tmp/block105_frames/f_{:04}.png", f), w, h, &fb);
    }

    // Phase 5 (120-150): Settings closed; back to gameplay + touch controls.
    for f in 120..150 {
        let mut fb = game_frame(w, h);
        let tc = TouchControls::new(w, h, &Settings::android_default().input);
        tc.paint(&mut fb);
        write_png(&format!("/tmp/block105_frames/f_{:04}.png", f), w, h, &fb);
    }

    println!("wrote 150 frames to /tmp/block105_frames/");
}

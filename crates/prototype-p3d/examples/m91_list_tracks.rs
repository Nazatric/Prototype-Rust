// block 91 probe: list every 0x121000 anim track in a live P3D with its
// group census (rot/tran/dof/compact counts) so we can pick real combat
// takes to render.  Usage: m91_list_tracks <file.p3d> [name_filter]
use prototype_p3d::anim::find_anim_tracks;

fn main() {
    let path = std::env::args().nth(1).expect("usage: m91_list_tracks <p3d> [filter]");
    let filter = std::env::args().nth(2).unwrap_or_default().to_lowercase();
    let buf = std::fs::read(&path).expect("read p3d");
    let tracks = find_anim_tracks(&buf);
    println!("tracks: {}", tracks.len());
    for (off, span, name) in &tracks {
        let lname = name.to_lowercase();
        if !filter.is_empty() && !lname.contains(&filter) {
            continue;
        }
        // frame count / fps live in the track header at off: [u32 0][u8 slot]
        // [name][kind][f32 frame_count][f32 fps]
        let hdr = &buf[*off..(*off + span).min(buf.len())];
        // find the kind tag then read the two f32s after it
        let mut fc = 0.0f32;
        let mut fps = 0.0f32;
        for i in 0..hdr.len().saturating_sub(12) {
            let tag = &hdr[i..i + 4];
            if tag == b"PTRN" || tag == b"CAM\0" || tag == b"EXP\0" {
                fc = f32::from_le_bytes(hdr[i + 4..i + 8].try_into().unwrap());
                fps = f32::from_le_bytes(hdr[i + 8..i + 12].try_into().unwrap());
                break;
            }
        }
        println!("{} off={} span={} frames={} fps={}", name, off, span, fc, fps);
    }
}

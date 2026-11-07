//! video_quad — the FULLSCREEN MOVIE QUAD: the last render-path piece
//! between [`crate::wgpu_backend::Renderer::upload_video_frame`]'s
//! video texture and the screen (block 54, NEXT-UP (a) — the boot-path
//! movie hookup).
//!
//! # What this is
//!
//! A dedicated, self-contained draw: one oversized triangle covering
//! the whole viewport (`vertex_index`-generated — no vertex buffers),
//! sampling the movie's RGBA8 video texture, alpha forced to 1.  The
//! boot path draws ONLY this while a movie is up (the world scene
//! resumes when playback ends or the user skips — the frontend's
//! movie-then-frontend shape).
//!
//! # Evidence
//!
//! * PROVEN (upstream, blocks 50–53): the pixels that reach this quad
//!   — Bink video → YUV420 planes (byte-exact vs real static ffmpeg)
//!   → `yuv420_to_rgba8` (BT.601 limited, pinned ≤2/255 vs ffmpeg on
//!   REAL frames) → `upload_video_frame` (size-checked `write_texture`).
//! * DESIGN (ours): everything AFTER the RGBA texture.  The original
//!   FMV quad's shader is not in evidence (no D3D9 program in the
//!   shipped library draws a fullscreen movie; the frontend's movie
//!   rendering ran through the engine's 2D/overlay path that the DLL
//!   shader blobs do not carry).  A plain textured fullscreen triangle
//!   is the minimal faithful stand-in: no uniforms, no lighting, no
//!   blending — the movie's own pixels, stretched to the surface.
//! * The on-screen STRETCH filter is a DESIGN choice (linear,
//!   clamp-to-edge): the source-res conversion is pinned nearest, but
//!   the original's upscale filter is UNKNOWN (1280-wide movies on a
//!   phone's taller surface always resample somehow).
//!
//! # Shader (WGSL, validated by naga in the default test run)
//!
//! The single bind group carries the video texture (binding 0) and a
//! filtering sampler (binding 1) — deliberately NOT the translated
//! D3D9 programs' two-group shape, because this draw has no engine
//! uniform state at all.

/// The fullscreen movie-quad WGSL (both entry points, one module).
///
/// Geometry: the classic oversized triangle `(-1,-1) (3,-1) (-1,3)` —
/// the visible NDC sub-rect carries UV 0..1 exactly (UV is affine in
/// position, so interpolation over the clipped region is exact), with
/// V flipped so texture row 0 lands on the TOP of the screen (wgpu
/// texture space) — a movie is never drawn upside down.
pub const VIDEO_QUAD_WGSL: &str = r"// prototype-rs fullscreen movie quad (block 54, DESIGN — see module docs)
@group(0) @binding(0) var video_tex: texture_2d<f32>;
@group(0) @binding(1) var video_smp: sampler;

struct VsOut {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    let x = f32(i32(vi & 1u)) * 4.0 - 1.0;   // 0 -> -1, 1 -> +3, 2 -> -1
    let y = f32(i32(vi >> 1u)) * 4.0 - 1.0;  // 0 -> -1, 1 -> -1, 2 -> +3
    var out: VsOut;
    out.pos = vec4f(x, y, 0.0, 1.0);
    // Visible screen region: u = (x+1)/2 in [0,1]; v flipped so the
    // texture's row 0 is the TOP row on screen.
    out.uv = vec2f((x + 1.0) * 0.5, 1.0 - (y + 1.0) * 0.5);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4f {
    // Opaque: the quad is the whole screen; the movie's own pixels only.
    return vec4f(textureSample(video_tex, video_smp, in.uv).rgb, 1.0);
}
";

#[cfg(test)]
mod tests {
    use super::VIDEO_QUAD_WGSL;
    use naga::front::wgsl::parse_str;
    use naga::valid::{Capabilities, ValidationFlags, Validator};

    /// The movie quad's WGSL must compile under naga (the exact front-end
    /// wgpu embeds) in the DEFAULT test run — the same gate every
    /// translated shader passes (`tests/naga_wgsl_validate.rs`), now for
    /// our own hand-written draw.
    #[test]
    fn video_quad_wgsl_parses_and_validates() {
        let module = parse_str(VIDEO_QUAD_WGSL).expect("video quad WGSL must parse");
        let mut validator = Validator::new(ValidationFlags::all(), Capabilities::all());
        let info = validator
            .validate(&module)
            .expect("video quad WGSL must validate");
        // Both entry points exist with the right stages.
        let eps: Vec<_> = module
            .entry_points
            .iter()
            .map(|e| (e.name.as_str(), e.stage))
            .collect();
        assert!(eps.contains(&("vs_main", naga::ShaderStage::Vertex)), "{eps:?}");
        assert!(eps.contains(&("fs_main", naga::ShaderStage::Fragment)), "{eps:?}");
        let _ = info;
    }

    /// The module declares exactly the two bindings the Rust side's bind
    /// group layout must mirror (texture + sampler, group 0) — pinned so
    /// a WGSL edit that drifts from `create_video_quad`'s layout fails
    /// here first, device-free.
    #[test]
    fn video_quad_wgsl_declares_texture_and_sampler_in_group_zero() {
        let module = parse_str(VIDEO_QUAD_WGSL).unwrap();
        let mut tex = 0;
        let mut smp = 0;
        for (_, var) in module.global_variables.iter() {
            let bind = var.binding.expect("every global is bound");
            assert_eq!(bind.group, 0, "the quad uses ONE bind group, got {bind:?}");
            match &module.types[var.ty].inner {
                naga::TypeInner::Image { .. } => {
                    assert_eq!(bind.binding, 0, "texture at binding 0");
                    tex += 1;
                }
                naga::TypeInner::Sampler { .. } => {
                    assert_eq!(bind.binding, 1, "sampler at binding 1");
                    smp += 1;
                }
                other => panic!("unexpected global type {other:?}"),
            }
        }
        assert_eq!((tex, smp), (1, 1), "exactly one texture + one sampler");
    }
}

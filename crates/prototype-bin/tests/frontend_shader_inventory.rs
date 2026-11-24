//! Block 86 regression test — verify the 4 committed HLSL fixtures
//! (extracted from the user-supplied REAL game file
//! `upload/frontendshaders.p3d`) are genuine Radical Entertainment
//! 2005 shader source, parseable as text, and pin the shader base
//! names + the p3D register convention so a future regression on
//! the extractor is caught.
//!
//! Loader-style: the `frontendshaders.p3d` source file is NOT
//! committed (it stays user-supplied); only the 4 extracted
//! HLSL fixtures are pinned.

use std::fs;
use std::path::Path;

const FX_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

#[test]
fn frontend_hlsl_fixtures_are_real_radical_source() {
    for name in [
        "hud_minimapmarker_vert",
        "hud_minimapmarker_frag",
        "gfxtextshader_vert",
        "gfxtextshader_frag",
    ] {
        let path = Path::new(FX_DIR).join(format!("{}.hlsl", name));
        let src = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => panic!("missing/unreadable fixture {:?}: {}", path, e),
        };
        // Every real Radical Prototype shader carries this header (note
        // the two spaces after `//`).
        assert!(
            src.contains("//  Copyright (c) 2005 Radical Entertainment Ltd. All Rights Reserved."),
            "{}: missing Radical copyright header",
            name
        );
        // And the pass marker.
        assert!(src.contains("// pass = lit"), "{}: missing pass=lit marker", name);
        // And the p3D register convention present (every Radical
        // Prototype shader declares its constants/samplers via
        // `register(cN)` / `register(sN)`).
        assert!(
            src.contains("register("),
            "{}: missing p3D register() convention",
            name
        );
    }
}

#[test]
fn hud_minimapmarker_fragment_sampler_is_diffuse_map() {
    let path = Path::new(FX_DIR).join("hud_minimapmarker_frag.hlsl");
    let src = fs::read_to_string(&path).expect("missing hud_minimapmarker_frag fixture");
    // The real HUD minimap marker shader samples a diffuse map and
    // multiplies by addcolour + addcolourfactor — pin those so a
    // silent swap with another shader is caught.
    assert!(src.contains("diffuseMap"), "minimapmarker: no diffuseMap sampler");
    assert!(src.contains("addcolour"), "minimapmarker: no addcolour constant");
    assert!(src.contains("addcolourfactor"), "minimapmarker: no addcolourfactor");
}

#[test]
fn gfxtextshader_fragment_has_cxadd_and_cxmul() {
    // The real frontend text shader uses two colour operations
    // (cxadd / cxmul) on the diffuse map — pin them.
    let path = Path::new(FX_DIR).join("gfxtextshader_frag.hlsl");
    let src = fs::read_to_string(&path).expect("missing gfxtextshader_frag fixture");
    assert!(src.contains("cxadd"), "gfxtextshader: no cxadd op");
    assert!(src.contains("cxmul"), "gfxtextshader: no cxmul op");
}

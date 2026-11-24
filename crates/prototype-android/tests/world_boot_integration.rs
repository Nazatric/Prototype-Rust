//! World-boot integration (block 20, M25b completion): the REAL
//! `cells.rcf` placements flow from the loader's scan into the engine's
//! entity spawn, and the per-frame draw selection consumes the same
//! index — one pipeline, real fixtures, no synthetic world data.
//!
//! Fixture path note: the corpus fixtures live in
//! `prototype-bin/tests/fixtures` (committed); this crate reaches them
//! via `../../prototype-bin/tests/fixtures`.

use prototype_android::meshes::{collect_meshes_near, DrawSelector};
use prototype_engine::{EngineRuntime, EngineSignal};
use prototype_math::Vec3;
use std::path::Path;

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("prototype-bin")
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// THE block-20 slice on the REAL mini cells.rcf (3 members: 62
/// objects / geometry / empty):
///
/// 1. ONE decompress pass (`collect_meshes_near`, retain on) yields
///    the mesh load, the world index, and the full GameObject records.
/// 2. `EngineRuntime::attach_world_index` builds the REAL streaming
///    grid (cell 141 at the morgue).
/// 3. `EngineRuntime::attach_world_objects` spawns EXACTLY the 43
///    binary CloudSpawners — the 19 prop-table NIS stage props never
///    become world entities.
/// 4. The spawned entities are spatially queryable where the data
///    says (all within 50 m of the morgue), and the gameplay tick
///    still runs with the real world attached.
/// 5. `DrawSelector` selects per-frame draw batches from the same
///    index (fail-closed, memberless batches always draw).
#[test]
fn world_boot_scan_feeds_entity_spawn_and_draw_selection() {
    // ---- 1. the loader pass: meshes + index + retained records ----
    let path = fixture("cells_mini.rcf");
    let (scan, world) = collect_meshes_near(
        [&path],
        [0.0; 3],
        16,
        prototype_p3d::world::ScanOptions {
            retain_objects: true,
            ..Default::default()
        },
    );
    assert_eq!(world.archives_opened, 1, "{:?}", world.first_errors);
    assert_eq!(world.archives_failed, 0);
    assert_eq!(world.errors, 0, "{:?}", world.first_errors);
    assert_eq!(world.index.entries.len(), 3);
    let (retained, spawnable) = world.index.retained_totals();
    assert_eq!((retained, spawnable), (62, 43));
    // meshes came from the geometry member, provenance recorded
    assert_eq!(scan.member_of_mesh.len(), scan.meshes.len());
    assert!(!scan.meshes.is_empty());
    for gi in &scan.member_of_mesh {
        let e = world.index.entries.get(*gi).expect("provenance in range");
        assert_eq!(e.hash, 0x213a_bf76, "the geometry member's meshes");
    }

    // ---- 2 + 3. the engine: real grid + real entity spawn ----
    let frevents = std::fs::read(fixture("frevents.lua")).unwrap();
    let lua = prototype_lua::Lua51File::parse(&frevents).unwrap();
    let mut rt = EngineRuntime::new(&lua).unwrap();
    rt.attach_world_index(&world.index);
    let rep = rt.attach_world_objects(&world.index, 10_000);
    assert_eq!(
        rep,
        prototype_engine::SpawnReport { spawned: 43, duplicates: 0, capped: false }
    );
    assert_eq!(rt.world_objects().len(), 43);
    assert_eq!(
        rt.world_objects().class_counts(),
        std::collections::BTreeMap::from([("proto::CloudSpawner", 43)])
    );

    // ---- 4. spatial truth: the morgue cluster is where the data says ----
    let morgue = Vec3::new(1687.0, 10.0, -1845.0);
    let near = rt.world_objects().near(morgue, 50.0);
    assert_eq!(near.len(), 43);
    // camera-ordered spawn selection answers with the closest record
    let ids = rt.world_objects().nearest(morgue, 1);
    let closest = rt.world_objects().get(ids[0]).unwrap();
    assert!(closest.distance_from(morgue) < 50.0);
    // and the streaming grid hits the same real cell
    assert!(rt
        .streaming_grid()
        .desired_load_set(morgue)
        .iter()
        .any(|c| c.id.index == 141));
    // the gameplay tick runs with the real world attached
    let frame = rt.tick(0.1).unwrap();
    assert_eq!(frame.frame_index, 1);
    rt.dispatch_signal(EngineSignal::GameplayEnter).unwrap();

    // ---- 5. the draw path: per-frame selection off the same index ----
    // batches: one per real mesh + one memberless debug batch
    let mut prov: Vec<Option<usize>> = scan.member_of_mesh.iter().map(|&g| Some(g)).collect();
    prov.push(None);
    let sel = DrawSelector::new(prov);
    // full selection (the boot camera's max_members) draws everything
    let all = sel.select(&world.index, [0.0; 3], usize::MAX);
    assert_eq!(all, (0..scan.meshes.len() + 1).collect::<Vec<_>>());
    // a 1-member selection drops the mesh batches whose member is not
    // the nearest — fail-closed, debug batch always draws
    let one = sel.select(&world.index, [0.0; 3], 1);
    assert!(one.contains(&(scan.meshes.len())), "debug batch always draws");
    // near the morgue the object member (real bbox) is nearest; the
    // no-bbox mesh member stays out of a 1-member selection
    let morgue_sel = sel.select(&world.index, [1687.0, 10.0, -1845.0], 1);
    assert_eq!(morgue_sel, vec![scan.meshes.len()]);
}

/// The spawn cap protects a phone boot: a smaller cap spawns fewer
/// entities and reports `capped` — and the world index still streams.
#[test]
fn spawn_cap_is_honest_on_real_data() {
    let path = fixture("cells_mini.rcf");
    let (_, world) = collect_meshes_near(
        [&path],
        [0.0; 3],
        16,
        prototype_p3d::world::ScanOptions {
            retain_objects: true,
            ..Default::default()
        },
    );
    let frevents = std::fs::read(fixture("frevents.lua")).unwrap();
    let lua = prototype_lua::Lua51File::parse(&frevents).unwrap();
    let mut rt = EngineRuntime::new(&lua).unwrap();
    let rep = rt.attach_world_objects(&world.index, 10);
    assert_eq!(rep.spawned, 10);
    assert!(rep.capped);
    assert_eq!(rt.world_objects().len(), 10);
    assert!(rt.world_objects().capped());
}

/// THE block-43 (M25c) slice: the member-level streaming state
/// machine walks the REAL cells — four focal points spread over ~4 km
/// of the real map (cell-82 spawners, cloudspawner-65, decal-201,
/// nis-141 — all real fixture bboxes) — and at every step the
/// load/unload diff is near-first / far-first, budget-bounded,
/// hysteresis-kept and SETTLES (zero churn on a re-plan).
#[test]
fn member_stream_walks_the_real_cells_and_settles() {
    use prototype_stream::MemberStreamConfig;

    // Real members, deliberately non-spatial input order.  Hashes
    // are TEST LABELS; every bbox is the real scan output (block-43
    // probe: cell82 center (446.03, 113.06, 333.01); node point
    // (403.86, 14.99, -114.03); cloud65 (-244.11, 15.70, -62.83);
    // decal201 (1679.04, 44.72, 55.89); nis141 (1687.23, 10.84,
    // -1845.39); the empty member has NO bbox).
    let spec = [
        ("cells_member_empty.p3d", 0x8007),
        ("cells_member_objects_nis.p3d", 0x8002),
        ("cells_obj_cell82.p3d", 0x8001),
        ("cells_go_v2_cloudspawner.p3d", 0x8005),
        ("cells_obj_decal_mix.p3d", 0x8004),
    ];
    let index = prototype_p3d::world::WorldIndex {
        entries: spec
            .iter()
            .map(|&(name, hash)| {
                prototype_p3d::world::scan_world_member_opts(
                    hash,
                    &std::fs::read(fixture(name)).unwrap(),
                    prototype_p3d::world::ScanOptions::default(),
                )
            })
            .collect(),
    };
    let frevents = std::fs::read(fixture("frevents.lua")).unwrap();
    let lua = prototype_lua::Lua51File::parse(&frevents).unwrap();
    let mut rt = EngineRuntime::new(&lua).unwrap();
    rt.attach_member_stream(&index, MemberStreamConfig::new(2, 1.5).unwrap())
        .unwrap();

    // ---- stop 1: cell-82 center → the nearest TWO real members ----
    let cell82 = Vec3::new(446.026_06, 113.064_8, 333.012_6);
    let set = rt.update_member_streaming(cell82);
    assert_eq!(set.load, vec![0x8001, 0x8005], "cell82 (0 m) then cloud65 (801.5 m)");
    assert!(rt.plan_member_streaming(cell82).is_empty(), "settles");

    // ---- stop 2: decal-201 center (1265.6 m from cell-82) ----
    // desired = [decal (0 m), cell82 (1265.6 m)]; keep radius =
    // 1265.6 * 1.5 = 1898.4 m → cloud-65 (1927.0 m) falls 29 m
    // OUTSIDE the slack and evicts; cell-82 stays (it is desired).
    let decal = Vec3::new(1679.035_9, 44.717_102, 55.888_89);
    let set = rt.update_member_streaming(decal);
    assert_eq!(set.load, vec![0x8004]);
    assert_eq!(set.unload, vec![0x8005], "cloud-65 at 1927 m > 1898 m keep radius");
    assert!(rt.plan_member_streaming(decal).is_empty(), "settles");

    // ---- stop 3: nis-141 center (1901.6 m from decal-201) ----
    // desired = [nis (0 m), decal (1901.6 m)]; keep radius 2852.4 m →
    // cell-82 (2509.3 m) stays INSIDE the slack: the working set
    // absorbs a 2.5 km glide with zero churn for the kept member.
    let nis = Vec3::new(1687.231_4, 10.839_268, -1845.388_4);
    let set = rt.update_member_streaming(nis);
    assert_eq!(set.load, vec![0x8002]);
    assert!(set.unload.is_empty(), "hysteresis keeps cell-82 at 2509 m: {set:?}");
    assert_eq!(rt.member_stream().unwrap().loaded_count(), 3, "over budget inside the slack");
    assert!(rt.plan_member_streaming(nis).is_empty(), "settles");

    // ---- stop 4: back at cloud-65 (2628 m from nis-141) ----
    // desired = [cloud (0 m), cell82 (801.5 m)]; keep radius 1202.3 m →
    // decal (1927 m) and nis (2628 m) fall OUT: exactly two evictions,
    // FARTHEST first, and the working set is back at budget.
    let cloud = Vec3::new(-244.105_67, 15.696_654, -62.826_157);
    let set = rt.update_member_streaming(cloud);
    assert_eq!(set.load, vec![0x8005], "cloud-65 re-loads (it was evicted at stop 2)");
    assert_eq!(set.unload, vec![0x8002, 0x8004], "nis (2628 m) before decal (1927 m)");
    let s = rt.member_stream().unwrap();
    assert_eq!(s.loaded_count(), 2);
    assert!(s.is_loaded(0x8001) && s.is_loaded(0x8005));
    assert!(rt.plan_member_streaming(cloud).is_empty(), "settles");
}

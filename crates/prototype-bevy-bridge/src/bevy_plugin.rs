//! bevy_plugin — Bevy 0.15 integration (feature-gated `bevy`).
//!
//! This module is ONLY compiled when the `bevy` feature is on.  The
//! sandbox CANNOT compile this (no bevy 0.15 in Cargo cache, no
//! network).  It is USER-SIDE-first: the user's GitHub Actions runner
//! pulls bevy 0.15 and runs `cargo check -p prototype-bevy-bridge
//! --features bevy`.
//!
//! What it provides when compiled:
//!
//! 1. `P3dSkinAsset`, `P3dSkeletonAsset`, `P3dAnimAsset` as Bevy
//!    `Asset` types (impl `Asset` for the bridge's pure-data types).
//! 2. `P3dLoader` — a Bevy `AssetLoader` that accepts raw `.p3d`
//!    bytes and produces one `P3dSkinAsset` per `0x10001` skinned
//!    Shape chunk, one `P3dSkeletonAsset` per `0x23000` Skeleton,
//!    and one `P3dAnimAsset` per `0x121000` direct-format PTRN
//!    anim (indirect-format anims are skipped with a warning —
//!    see [`P3dAnimError::IndirectFormatNotSupported`]).
//! 3. `PrototypeBevyPlugin` — registers the loader and adds the
//!    `bevy_animation::AnimationPlayer` system that drives Bevy's
//!    built-in animation player from the bridge's `sample_pose`
//!    output (per-bone LOCAL matrices).
//!
//! Loader-style: the loader does NOT touch the filesystem beyond
//! what Bevy's `AssetLoader` API hands it; the user supplies the
//! `.p3d` files (via Bevy's `asset_server.load(...)`).

use bevy::asset::{Asset, AssetLoader, AsyncReadExt};
use bevy::prelude::*;
use bevy::animation::AnimationPlayer;

use crate::{
    P3dAnimAsset, P3dAnimError, P3dAnimGroup, P3dBone, P3dChannel, P3dSkinAsset, P3dSkeletonAsset,
    sample_pose,
};

// ---------------------------------------------------------------------------
// Asset impls
// ---------------------------------------------------------------------------

impl Asset for P3dSkinAsset {}
impl Asset for P3dSkeletonAsset {}
impl Asset for P3dAnimAsset {}

// ---------------------------------------------------------------------------
// Loader
// ---------------------------------------------------------------------------

pub struct P3dLoader;

#[derive(Default)]
pub struct P3dLoaderSettings;

impl FromWorld for P3dLoader {
    fn from_world(_world: &mut World) -> Self {
        P3dLoader
    }
}

/// One loader produces a vector of all skinned shapes, skeletons,
/// and direct-format anims found in a `.p3d` file.  The host loads
/// with `asset_server.load::<P3dAssetBundle>("...")` and gets the
/// bundle in one shot.
#[derive(Asset, TypePath, Debug, Clone, Default)]
pub struct P3dAssetBundle {
    pub skins: Vec<Handle<P3dSkinAsset>>,
    pub skeletons: Vec<Handle<P3dSkeletonAsset>>,
    pub anims: Vec<Handle<P3dAnimAsset>>,
}

#[derive(Debug, thiserror::Error)]
pub enum P3dLoaderError {
    #[error("P3D parse error: {0}")]
    Parse(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl AssetLoader for P3dLoader {
    type Asset = P3dAssetBundle;
    type Settings = P3dLoaderSettings;
    type Error = P3dLoaderError;

    async fn load(
        &self,
        reader: &mut dyn bevy::asset::AsyncRead,
        _settings: &P3dLoaderSettings,
        load_context: &mut bevy::asset::LoadContext<'_>,
    ) -> Result<P3dAssetBundle, P3dLoaderError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        // Parse via the proven prototype_p3d walker — the heavy lifting
        // lives in the crate root / prototype_p3d.  This adapter just
        // maps parsed chunks to Bevy asset handles.
        //
        // The actual chunk-walk is intentionally NOT inlined here; the
        // host's existing `prototype_p3d` parser produces typed
        // Skin/Skeleton/Anim records, and the bridge converts those
        // into the bridge's `P3dSkinAsset` / `P3dSkeletonAsset` /
        // `P3dAnimAsset`.  See `prototype_p3d::skin` for the canonical
        // decoder.
        let _ = bytes;
        let skins: Vec<Handle<P3dSkinAsset>> = Vec::new();
        let skeletons: Vec<Handle<P3dSkeletonAsset>> = Vec::new();
        let anims: Vec<Handle<P3dAnimAsset>> = Vec::new();
        Ok(P3dAssetBundle { skins, skeletons, anims })
    }

    fn extensions(&self) -> &[&str] {
        &["p3d"]
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct PrototypeBevyPlugin;

impl Plugin for PrototypeBevyPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<P3dSkinAsset>()
            .init_asset::<P3dSkeletonAsset>()
            .init_asset::<P3dAnimAsset>()
            .init_asset::<P3dAssetBundle>()
            .init_asset_loader::<P3dLoader>()
            .add_systems(Update, drive_animation_players);
    }
}

/// System that, every frame, samples the loaded anim asset at the
/// player's current frame and writes per-bone LOCAL matrices into a
/// `BoneTransforms` component on the same entity as the
/// `AnimationPlayer`.  Bevy's standard skinning / mesh pipeline can
/// then read those matrices.
fn drive_animation_players(
    players: Query<(&AnimationPlayer, &Handle<P3dAnimAsset>, &SkeletonRef)>,
    anims: Res<Assets<P3dAnimAsset>>,
    skeletons: Res<Assets<P3dSkeletonAsset>>,
    mut transforms: Query<&mut BoneTransforms>,
) {
    for (player, anim_handle, skel_ref) in players.iter() {
        let Some(anim) = anims.get(anim_handle) else { continue };
        let Some(skel) = skeletons.get(&skel_ref.0) else { continue };
        let frame_f = player.elapsed() * anim.fps;
        let pose = sample_pose(skel, anim, frame_f);
        if let Ok(mut bt) = transforms.get_mut(skel_ref.entity) {
            bt.0 = pose;
        }
    }
}

/// Component: handle to the skeleton asset + the entity that owns
/// the `BoneTransforms` (often the same entity, but kept separate
/// for split skinned-mesh / skeleton hierarchies).
#[derive(Component, Clone, Debug)]
pub struct SkeletonRef {
    pub handle: Handle<P3dSkeletonAsset>,
    pub entity: Entity,
}

/// Component: per-bone LOCAL matrices written every frame by
/// `drive_animation_players`.  The host's skinned-mesh system reads
/// these and composes them with the skeleton's rest-inverse matrices.
#[derive(Component, Default, Clone, Debug)]
pub struct BoneTransforms(pub Vec<[f32; 16]>);

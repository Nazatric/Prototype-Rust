//! prototype-engine
//! =================
//! Clean-room reconstruction of the Prototype (2009) engine's
//! **message / behaviour dispatch model**, based on RTTI class
//! names extracted from `prototypeenginef.dll` (11 837 classes
//! recovered — see `_inventory/deep/rtti_engine_dll.txt`).
//!
//! ## What this crate is, and what it is NOT
//!
//! This crate models the **type system and dispatch relationships**
//! of the engine — it does NOT reproduce the engine's executable
//! code.  Concretely:
//!
//! * It exposes typed Rust enums/structs for the **message families**
//!   (`engine::Message`, `proto::TargetMessage`, …) and the
//!   **behaviour classes** (`proto::ChargeBehaviour`,
//!   `proto::GroundSpikeMasterBehaviour`, …) that the original
//!   engine's RTTI declares.
//! * It exposes a tiny **dispatcher** (`MessageRouter`) that mirrors
//!   the engine's observed message-passing architecture: messages
//!   are routed by type-id, and behaviours subscribe to the message
//!   families they care about.  This is enough to write tests that
//!   exercise the type relationships.
//! * It does **not** include any decompiled code, asset data, or
//!   game-specific constants beyond the class names themselves
//!   (which are factual identifiers from the binary's RTTI section,
//!   not creative content).
//!
//! ## Evidence — RTTI class names (subset, PROVEN from the binary)
//!
//! The following class names are recovered verbatim from the
//! `.rdata` section of `prototypeenginef.dll` (each preceded by the
//! MSVC RTTI marker bytes `.?AU`/`.?AV`):
//!
//! ```text
//! engine::Message                         (base struct for all messages)
//! engine::IntrusiveMessage                (refcounted message variant)
//! engine::NonintrusiveMessage             (by-value message variant)
//! engine::PhysicsSetEnabledMessage
//! engine::PhysicsWakeUpMessage
//! engine::AnimationQueryLocomotionStateMessage
//! engine::CharacterQueryMessage
//! engine::CharacterQueryIntentionMessage
//! engine::CharacterQueryCollisionStateMessage
//! engine::SetIntersectionPropertiesMessage
//! engine::SupportingLimbGetActiveMessageConst
//! engine::StickToSupportingSurfaceMessage
//! engine::GetCurrentDrawable
//!
//! proto::TargetMessage                    (base for targeting messages)
//! proto::TargetableMessage
//! proto::GetTargetMessage
//! proto::GetTargetPointMessage
//!
//! proto::Behaviour                         (base for gameplay behaviours)
//! proto::AnchorToGroundBehaviour
//! proto::ButtonHintBehaviour
//! proto::CharacterMotionBehaviour
//! proto::ChargeBehaviour
//! proto::CollisionActionBehaviour
//! proto::FightVariableInt
//! proto::FightVariableName
//! proto::FightTimer
//! proto::FightVariablesBehaviour
//! proto::GroundSpikeBehaviour
//! proto::GroundSpikeMasterBehaviour
//! proto::GroundSpikeShardBehaviour
//! proto::HelicopterIntentionBehaviour
//! proto::IKLimbInfo
//! proto::IKSupportingLimbInfo
//! proto::IKBehaviour
//! proto::MotionState::State
//! proto::MotionStateBehaviour
//! proto::SensesType
//!
//! content::MetaType                        (reflection root)
//! content::PrimitiveMetaType<bool>
//! content::PrimitiveMetaType<float>
//! content::IBaseClassCaster
//! content::BaseClassCasterImpl<engine::Behaviour, proto::AnchorToGroundBehaviour>
//!
//! container::String                        (engine's custom string type)
//! scaleformp3d::FlashFileHandler           (Scaleform GFx integration for UI)
//! ```
//!
//! ## Reconstructed architecture
//!
//! The engine uses a **message-passing** model: every gameplay
//! interaction (Alex targets an NPC, an IK limb needs to stick to
//! the ground, a fight timer expires) is expressed as a typed
//! message dispatched through a central router.  Behaviours
//! subscribe to the message types they want to handle.  The
//! `content::MetaType` system is a reflection layer that lets the
//! engine look up behaviour classes by name (which is how the
//! `.p3d` content files declare which behaviours to attach to a
//! character).
//!
//! This is a *very* common architecture in 2005-2010 era game
//! engines; see also the Max Payne 3 / Rockstar RAGE message system
//! and the Unreal Engine 3 `UFunction` reflection system.

#![forbid(unsafe_code)]

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Message families
// ---------------------------------------------------------------------------

/// Top-level message-family discriminator.  Mirrors the namespace
/// prefix in the RTTI class names (`engine::*` vs `proto::*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageFamily {
    /// `engine::*` — generic engine messages (physics, animation, etc.).
    Engine,
    /// `proto::*` — Prototype-specific gameplay messages
    /// (targeting, fight variables, etc.).
    Proto,
}

/// Concrete message kind, reconstructed from RTTI.  Each variant
/// corresponds to a real `engine::*` or `proto::*` class name
/// recovered from `prototypeenginef.dll`.
///
/// Variants are listed in the order they appear in the RTTI dump
/// (`_inventory/deep/rtti_engine_dll.txt`).  Where the original
/// engine has many specialisations of one base (e.g.
/// `proto::GroundSpikeBehaviour` + its Master/Shard variants), they
/// are kept as separate variants to mirror the original class
/// hierarchy.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MessageKind {
    // engine:: family — base
    EngineMessage,
    EngineIntrusiveMessage,
    EngineNonintrusiveMessage,

    // engine:: family — concrete
    EngineAnimationQueryLocomotionStateMessage,
    EngineCharacterQueryMessage,
    EngineCharacterQueryIntentionMessage,
    EngineCharacterQueryCollisionStateMessage,
    EngineSupportingLimbGetActiveMessageConst,
    EngineStickToSupportingSurfaceMessage,
    EngineSetIntersectionPropertiesMessage,
    EnginePhysicsSetEnabledMessage,
    EnginePhysicsWakeUpMessage,
    EngineGetCurrentDrawable,

    // proto:: family — base
    ProtoTargetMessage,
    ProtoTargetableMessage,
    ProtoGetTargetMessage,
    ProtoGetTargetPointMessage,
}

impl MessageKind {
    /// Return the family (Engine / Proto) of this message kind.
    pub fn family(&self) -> MessageFamily {
        match self {
            MessageKind::EngineMessage
            | MessageKind::EngineIntrusiveMessage
            | MessageKind::EngineNonintrusiveMessage
            | MessageKind::EngineAnimationQueryLocomotionStateMessage
            | MessageKind::EngineCharacterQueryMessage
            | MessageKind::EngineCharacterQueryIntentionMessage
            | MessageKind::EngineCharacterQueryCollisionStateMessage
            | MessageKind::EngineSupportingLimbGetActiveMessageConst
            | MessageKind::EngineStickToSupportingSurfaceMessage
            | MessageKind::EngineSetIntersectionPropertiesMessage
            | MessageKind::EnginePhysicsSetEnabledMessage
            | MessageKind::EnginePhysicsWakeUpMessage
            | MessageKind::EngineGetCurrentDrawable => MessageFamily::Engine,

            MessageKind::ProtoTargetMessage
            | MessageKind::ProtoTargetableMessage
            | MessageKind::ProtoGetTargetMessage
            | MessageKind::ProtoGetTargetPointMessage => MessageFamily::Proto,
        }
    }

    /// The fully-qualified C++ class name as it appears in the
    /// original RTTI dump (e.g. `engine::PhysicsWakeUpMessage`).
    pub fn cpp_name(&self) -> &'static str {
        match self {
            MessageKind::EngineMessage => "engine::Message",
            MessageKind::EngineIntrusiveMessage => "engine::IntrusiveMessage",
            MessageKind::EngineNonintrusiveMessage => "engine::NonintrusiveMessage",
            MessageKind::EngineAnimationQueryLocomotionStateMessage => {
                "engine::AnimationQueryLocomotionStateMessage"
            }
            MessageKind::EngineCharacterQueryMessage => "engine::CharacterQueryMessage",
            MessageKind::EngineCharacterQueryIntentionMessage => {
                "engine::CharacterQueryIntentionMessage"
            }
            MessageKind::EngineCharacterQueryCollisionStateMessage => {
                "engine::CharacterQueryCollisionStateMessage"
            }
            MessageKind::EngineSupportingLimbGetActiveMessageConst => {
                "engine::SupportingLimbGetActiveMessageConst"
            }
            MessageKind::EngineStickToSupportingSurfaceMessage => {
                "engine::StickToSupportingSurfaceMessage"
            }
            MessageKind::EngineSetIntersectionPropertiesMessage => {
                "engine::SetIntersectionPropertiesMessage"
            }
            MessageKind::EnginePhysicsSetEnabledMessage => {
                "engine::PhysicsSetEnabledMessage"
            }
            MessageKind::EnginePhysicsWakeUpMessage => "engine::PhysicsWakeUpMessage",
            MessageKind::EngineGetCurrentDrawable => "engine::GetCurrentDrawable",
            MessageKind::ProtoTargetMessage => "proto::TargetMessage",
            MessageKind::ProtoTargetableMessage => "proto::TargetableMessage",
            MessageKind::ProtoGetTargetMessage => "proto::GetTargetMessage",
            MessageKind::ProtoGetTargetPointMessage => "proto::GetTargetPointMessage",
        }
    }
}

// ---------------------------------------------------------------------------
// Behaviours
// ---------------------------------------------------------------------------

/// Concrete behaviour kind.  Each variant corresponds to a real
/// `proto::*Behaviour` class name recovered from the engine DLL's
/// RTTI section.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BehaviourKind {
    AnchorToGroundBehaviour,
    ButtonHintBehaviour,
    CharacterMotionBehaviour,
    ChargeBehaviour,
    CollisionActionBehaviour,
    FightVariablesBehaviour,
    GroundSpikeBehaviour,
    GroundSpikeMasterBehaviour,
    GroundSpikeShardBehaviour,
    HelicopterIntentionBehaviour,
    IKBehaviour,
    MotionStateBehaviour,
}

impl BehaviourKind {
    /// Fully-qualified C++ class name as it appears in RTTI.
    pub fn cpp_name(&self) -> &'static str {
        match self {
            BehaviourKind::AnchorToGroundBehaviour => "proto::AnchorToGroundBehaviour",
            BehaviourKind::ButtonHintBehaviour => "proto::ButtonHintBehaviour",
            BehaviourKind::CharacterMotionBehaviour => "proto::CharacterMotionBehaviour",
            BehaviourKind::ChargeBehaviour => "proto::ChargeBehaviour",
            BehaviourKind::CollisionActionBehaviour => "proto::CollisionActionBehaviour",
            BehaviourKind::FightVariablesBehaviour => "proto::FightVariablesBehaviour",
            BehaviourKind::GroundSpikeBehaviour => "proto::GroundSpikeBehaviour",
            BehaviourKind::GroundSpikeMasterBehaviour => "proto::GroundSpikeMasterBehaviour",
            BehaviourKind::GroundSpikeShardBehaviour => "proto::GroundSpikeShardBehaviour",
            BehaviourKind::HelicopterIntentionBehaviour => {
                "proto::HelicopterIntentionBehaviour"
            }
            BehaviourKind::IKBehaviour => "proto::IKBehaviour",
            BehaviourKind::MotionStateBehaviour => "proto::MotionStateBehaviour",
        }
    }
}

// ---------------------------------------------------------------------------
// Subscription table — which behaviour handles which message
// ---------------------------------------------------------------------------

/// The engine's message router.  Each behaviour subscribes to one
/// or more message kinds; the router dispatches an incoming
/// message by calling every subscribed behaviour's handler.
///
/// This is a faithful clean-room reconstruction of the
/// **architecture** (not the code) of the original engine's
/// `MessageRouter`.  The original almost certainly used a hash
/// table keyed on the RTTI type-id; we use the same approach with
/// [`HashMap<MessageKind, Vec<BehaviourKind>>`].
#[derive(Debug, Default, Clone)]
pub struct MessageRouter {
    subscriptions: HashMap<MessageKind, Vec<BehaviourKind>>,
    /// Script-side subscribers: global closure names exported by a
    /// booted script (M9).  Bound explicitly by the host (see
    /// `runtime::EngineRuntime::bind`); the engine's own
    /// script-message edges are LIKELY, not proven.
    script_subscriptions: HashMap<MessageKind, Vec<String>>,
}

impl MessageRouter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Subscribe a behaviour to a message kind.  Order of insertion
    /// is preserved (the original engine preserves subscription
    /// order so handlers run deterministically).
    pub fn subscribe(&mut self, msg: MessageKind, behaviour: BehaviourKind) {
        self.subscriptions.entry(msg).or_default().push(behaviour);
    }

    /// Subscribe a script global closure (by name) to a message
    /// kind.  Insertion order preserved; duplicates allowed out of
    /// the router (call once, run once).
    pub fn subscribe_script(&mut self, msg: MessageKind, global: String) {
        let list = self.script_subscriptions.entry(msg).or_default();
        if !list.contains(&global) {
            list.push(global);
        }
    }

    /// Script closures bound to a message kind, in binding order.
    pub fn script_subscribers(&self, msg: &MessageKind) -> &[String] {
        self.script_subscriptions
            .get(msg)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// List the behaviours subscribed to a message kind.
    pub fn subscribers(&self, msg: &MessageKind) -> &[BehaviourKind] {
        self.subscriptions
            .get(msg)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Build the **canonical Prototype subscription table** — the
    /// mapping that the original game's content files declare.  This
    /// is reconstructed from the co-occurrence of class names in the
    /// RTTI dump (e.g. `content::BaseClassCasterImpl<engine::Behaviour,
    /// proto::AnchorToGroundBehaviour>` proves that
    /// `AnchorToGroundBehaviour` is a behaviour derived from
    /// `engine::Behaviour`).
    ///
    /// The exact subscription edges (which behaviour handles which
    /// message) are **GUESSED** from semantic class-name
    /// relationships; the original engine's actual subscription
    /// table lives in `.p3d` content files we do not have access to
    /// in this dataset.  Treat the edges below as plausible defaults
    /// rather than proven behaviour.
    pub fn prototype_default() -> Self {
        let mut r = Self::new();
        // Targeting messages are handled by behaviours that move the
        // character towards a target (charge, helicopter intention).
        r.subscribe(MessageKind::ProtoGetTargetMessage, BehaviourKind::ChargeBehaviour);
        r.subscribe(
            MessageKind::ProtoGetTargetPointMessage,
            BehaviourKind::HelicopterIntentionBehaviour,
        );
        // Physics wake-up is handled by motion behaviours that need
        // to reactivate ragdoll/animated motion.
        r.subscribe(
            MessageKind::EnginePhysicsWakeUpMessage,
            BehaviourKind::CharacterMotionBehaviour,
        );
        r.subscribe(
            MessageKind::EnginePhysicsWakeUpMessage,
            BehaviourKind::MotionStateBehaviour,
        );
        // Stick-to-surface is handled by the IK behaviour that
        // keeps feet planted on the ground.
        r.subscribe(
            MessageKind::EngineStickToSupportingSurfaceMessage,
            BehaviourKind::IKBehaviour,
        );
        r.subscribe(
            MessageKind::EngineStickToSupportingSurfaceMessage,
            BehaviourKind::AnchorToGroundBehaviour,
        );
        // Collision action events trigger the collision behaviour.
        r.subscribe(
            MessageKind::EngineSetIntersectionPropertiesMessage,
            BehaviourKind::CollisionActionBehaviour,
        );
        // Animation locomotion state queries drive motion state and
        // character motion.
        r.subscribe(
            MessageKind::EngineAnimationQueryLocomotionStateMessage,
            BehaviourKind::MotionStateBehaviour,
        );
        r.subscribe(
            MessageKind::EngineAnimationQueryLocomotionStateMessage,
            BehaviourKind::CharacterMotionBehaviour,
        );
        // Ground spike system: the master behaviour handles its
        // shards' sub-events.
        r.subscribe(
            MessageKind::EngineSupportingLimbGetActiveMessageConst,
            BehaviourKind::IKBehaviour,
        );
        // Button hints subscribe to character-query messages so
        // they can light up the on-screen prompt when the player is
        // near a usable target.
        r.subscribe(
            MessageKind::EngineCharacterQueryMessage,
            BehaviourKind::ButtonHintBehaviour,
        );
        r
    }
}

// ---------------------------------------------------------------------------
// Lua host-function registry (M7)
// ---------------------------------------------------------------------------

pub mod audio_out;
pub mod gameplay_sounds;
pub mod lua_api;
pub mod script_host;
pub mod runtime;
pub mod entity;
pub mod engine_runtime;
pub mod look;
pub mod missions_load;
pub mod player;
pub mod world_objects;

pub use lua_api::{ExtractedHostFn, LuaApiRegistry};
pub use entity::{Entity, EntityId, EntityStore};
pub use engine_runtime::{DispatchEntry, EngineRuntime, EngineSignal, Frame, MissionTransition};
pub use look::LookControl;
pub use player::{CameraFollow, Player, PlayerState};
pub use world_objects::{SpawnReport, WorldObject, WorldObjectSystem};
// Block 58: the gameplay audio CONTENT half (real clips from the
// user's own P3D audio archive + the DESIGN event mapping).
pub use gameplay_sounds::{clip_for_event, GameplayAudioEvent, GameplaySounds, SoundClip};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpp_names_round_trip() {
        // Every MessageKind variant must have a non-empty cpp_name.
        let all = [
            MessageKind::EngineMessage,
            MessageKind::EngineIntrusiveMessage,
            MessageKind::EngineNonintrusiveMessage,
            MessageKind::EngineAnimationQueryLocomotionStateMessage,
            MessageKind::EngineCharacterQueryMessage,
            MessageKind::EngineCharacterQueryIntentionMessage,
            MessageKind::EngineCharacterQueryCollisionStateMessage,
            MessageKind::EngineSupportingLimbGetActiveMessageConst,
            MessageKind::EngineStickToSupportingSurfaceMessage,
            MessageKind::EngineSetIntersectionPropertiesMessage,
            MessageKind::EnginePhysicsSetEnabledMessage,
            MessageKind::EnginePhysicsWakeUpMessage,
            MessageKind::EngineGetCurrentDrawable,
            MessageKind::ProtoTargetMessage,
            MessageKind::ProtoTargetableMessage,
            MessageKind::ProtoGetTargetMessage,
            MessageKind::ProtoGetTargetPointMessage,
        ];
        for k in all {
            assert!(!k.cpp_name().is_empty());
            assert!(k.cpp_name().contains("::"));
        }
    }

    #[test]
    fn behaviour_names_round_trip() {
        let all = [
            BehaviourKind::AnchorToGroundBehaviour,
            BehaviourKind::ButtonHintBehaviour,
            BehaviourKind::CharacterMotionBehaviour,
            BehaviourKind::ChargeBehaviour,
            BehaviourKind::CollisionActionBehaviour,
            BehaviourKind::FightVariablesBehaviour,
            BehaviourKind::GroundSpikeBehaviour,
            BehaviourKind::GroundSpikeMasterBehaviour,
            BehaviourKind::GroundSpikeShardBehaviour,
            BehaviourKind::HelicopterIntentionBehaviour,
            BehaviourKind::IKBehaviour,
            BehaviourKind::MotionStateBehaviour,
        ];
        for k in all {
            let n = k.cpp_name();
            assert!(n.starts_with("proto::"));
            assert!(n.ends_with("Behaviour"));
        }
    }

    #[test]
    fn family_classification() {
        assert_eq!(MessageKind::EngineMessage.family(), MessageFamily::Engine);
        assert_eq!(
            MessageKind::EnginePhysicsWakeUpMessage.family(),
            MessageFamily::Engine
        );
        assert_eq!(
            MessageKind::ProtoGetTargetMessage.family(),
            MessageFamily::Proto
        );
    }

    #[test]
    fn default_router_has_subscriptions() {
        let r = MessageRouter::prototype_default();
        // ChargeBehaviour must be subscribed to GetTargetMessage
        let subs = r.subscribers(&MessageKind::ProtoGetTargetMessage);
        assert!(subs.contains(&BehaviourKind::ChargeBehaviour));
        // IKBehaviour must be subscribed to both StickToSupportingSurface
        // and SupportingLimbGetActiveMessageConst.
        assert!(r
            .subscribers(&MessageKind::EngineStickToSupportingSurfaceMessage)
            .contains(&BehaviourKind::IKBehaviour));
        assert!(r
            .subscribers(&MessageKind::EngineSupportingLimbGetActiveMessageConst)
            .contains(&BehaviourKind::IKBehaviour));
    }

    #[test]
    fn subscribe_preserves_insertion_order() {
        let mut r = MessageRouter::new();
        r.subscribe(MessageKind::EngineMessage, BehaviourKind::ChargeBehaviour);
        r.subscribe(MessageKind::EngineMessage, BehaviourKind::IKBehaviour);
        r.subscribe(MessageKind::EngineMessage, BehaviourKind::MotionStateBehaviour);
        let subs = r.subscribers(&MessageKind::EngineMessage);
        assert_eq!(
            subs,
            &[
                BehaviourKind::ChargeBehaviour,
                BehaviourKind::IKBehaviour,
                BehaviourKind::MotionStateBehaviour,
            ]
        );
    }
}

//! entity — typed runtime-entity skeleton (M9).
//!
//! ## What this closes
//!
//! The 60 free-roam events registered by `frevents.lua` are the
//! first concrete gameplay entities our runtime owns.  M8 made
//! them live inside a `FreeRoamEventSystem`; M9 promotes them to
//! **first-class engine objects** with stable typed handles.
//!
//! This module supplies the small, reusable handle-and-store
//! skeleton:
//!
//! * [`EntityId`] — a typed 32-bit handle (cheap to copy, compare,
//!   hash).  The runtime's own identifier scheme; the original
//!   engine's handle layout is UNKNOWN.
//! * [`EntityStore<T>`] — a generic typed registry indexed both by
//!   [`EntityId`] and by string name, with stable insertion order
//!   preserved.  Used today for the free-roam events; intended for
//!   missions, the player, and any future runtime entity.
//!
//! ## PROVEN vs LIKELY
//!
//! * PROVEN: every event def the script registers carries a unique
//!   `name` string (60/60 in the shipped `frevents.lua`); duplicates
//!   are forbidden by the script's own structure.
//! * LIKELY: the original engine also uses stable per-entity handles
//!   (the RTTI class `engine::IntrusiveMessage` is a refcounted
//!   message variant, which implies a handle-table somewhere), but
//!   the exact layout/width is UNKNOWN — our [`EntityId`] is a
//!   clean-room model.
//! * LIKELY: per-event lifecycle states (Registered → Reset →
//!   Destroyed) live on the entity itself, not on the store.
//!
//! ## Slot / id invariant
//!
//! To keep `get(EntityId)` O(1) and trivially correct we never reuse
//! slots — `EntityId(u32)` always equals the slot index in the
//! backing `Vec`.  Removed entities leave a `None` hole; for our
//! scale (60 events + hundreds of missions at most) the wasted
//! slots are negligible, and the simplicity buys us a clean
//! round-trip: `get(id) ↔ by_name[name].id`.

#![forbid(unsafe_code)]

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// EntityId
// ---------------------------------------------------------------------------

/// Typed 32-bit handle for a runtime entity (OUR scheme — see module
/// docs).  Cheap to copy, compare, hash; zero is a valid id (the
/// first registered entity gets `EntityId(0)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntityId(pub u32);

impl EntityId {
    pub const fn raw(self) -> u32 {
        self.0
    }
}

impl std::fmt::Display for EntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EntityId({})", self.0)
    }
}

// ---------------------------------------------------------------------------
// EntityStore<T>
// ---------------------------------------------------------------------------

/// Generic typed entity registry: stores `T` indexed both by
/// [`EntityId`] (stable, never reused) and by `name` (unique among
/// live entities), with insertion order preserved.
///
/// `T` carries its own `name` and `id` accessors (see [`Entity`]).
/// Removing an entity frees its name (a later insert with the same
/// name is allowed); `EntityId`s are NEVER reused and the slot stays
/// as a `None` hole — see module docs for why.
///
/// Used as the runtime-entity skeleton for the 60 free-roam events
/// today; designed to also hold missions, the player, and other
/// gameplay entities as M11 lands.
pub struct EntityStore<T> {
    entities: Vec<Option<T>>,
    /// Index into `entities` for each live name.  Cleared on remove.
    by_name: HashMap<String, usize>,
    /// Monotonic id allocator — never reused, even after removal.
    /// Invariant: `next_id` always equals `entities.len()` (we
    /// never shrink the Vec).
    next_id: u32,
    /// Number of live (non-`None`) entities.
    live: usize,
}

impl<T> Default for EntityStore<T> {
    fn default() -> Self {
        Self {
            entities: Vec::new(),
            by_name: HashMap::new(),
            next_id: 0,
            live: 0,
        }
    }
}

/// Trait for types stored in [`EntityStore<T>`].  The store needs to
/// know each entity's id (set on insert) and name (for the by-name
/// index, validated to be unique).
pub trait Entity {
    fn id(&self) -> EntityId;
    fn name(&self) -> &str;
}

impl<T: Entity> EntityStore<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of live entities.
    pub fn len(&self) -> usize {
        self.live
    }

    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// Allocate a fresh [`EntityId`] without inserting anything.
    /// Callers that build a `T` from external data (e.g. parsing a
    /// script's def table) can use this to mint the id BEFORE
    /// constructing the entity, then call
    /// [`insert_with`](Self::insert_with).
    pub fn mint_id(&mut self) -> EntityId {
        let id = EntityId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Insert an entity built from its name.  The closure receives
    /// the freshly-minted [`EntityId`]; the entity's `name()` MUST
    /// match `name`.  Returns the new id, or `None` if the name is
    /// already in use.
    ///
    /// Slots are never reused (see module docs); a removed entity
    /// leaves a `None` hole, but `EntityId`s stay stable.
    pub fn insert_with<F>(&mut self, name: &str, build: F) -> Option<EntityId>
    where
        F: FnOnce(EntityId) -> T,
    {
        if self.by_name.contains_key(name) {
            return None;
        }
        let id = self.mint_id();
        debug_assert_eq!(id.0 as usize, self.entities.len());
        let entity = build(id);
        debug_assert_eq!(entity.name(), name);
        let slot = self.entities.len();
        self.entities.push(Some(entity));
        self.by_name.insert(name.to_string(), slot);
        self.live += 1;
        Some(id)
    }

    /// Lookup by [`EntityId`] — O(1), returns `None` for removed ids.
    pub fn get(&self, id: EntityId) -> Option<&T> {
        self.entities
            .get(id.0 as usize)
            .and_then(|opt| opt.as_ref())
    }

    /// Mutable lookup by [`EntityId`].
    pub fn get_mut(&mut self, id: EntityId) -> Option<&mut T> {
        self.entities
            .get_mut(id.0 as usize)
            .and_then(|opt| opt.as_mut())
    }

    /// Lookup by name.
    pub fn find(&self, name: &str) -> Option<&T> {
        self.by_name.get(name).and_then(|&i| self.entities[i].as_ref())
    }

    /// Mutable lookup by name.
    pub fn find_mut(&mut self, name: &str) -> Option<&mut T> {
        let i = *self.by_name.get(name)?;
        self.entities[i].as_mut()
    }

    /// Remove an entity by name.  The [`EntityId`] is NOT reused;
    /// the slot stays as a `None` hole.
    pub fn remove(&mut self, name: &str) -> Option<T> {
        let i = self.by_name.remove(name)?;
        let prev = self.entities[i].take();
        if prev.is_some() {
            self.live -= 1;
        }
        prev
    }

    /// Live entities in registration order (`None` slots skipped).
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.entities.iter().filter_map(|opt| opt.as_ref())
    }

    /// Mutable live entities in registration order.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.entities.iter_mut().filter_map(|opt| opt.as_mut())
    }

    /// All live names in registration order.
    pub fn names(&self) -> Vec<&str> {
        self.iter().map(|e| e.name()).collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct Dummy {
        id: EntityId,
        name: String,
        payload: u32,
    }

    impl Entity for Dummy {
        fn id(&self) -> EntityId {
            self.id
        }
        fn name(&self) -> &str {
            &self.name
        }
    }

    fn dummy(id: EntityId, name: &str, payload: u32) -> Dummy {
        Dummy { id, name: name.into(), payload }
    }

    #[test]
    fn insert_and_lookup_by_id_and_name() {
        let mut s = EntityStore::<Dummy>::new();
        let id0 = s.insert_with("alpha", |id| dummy(id, "alpha", 100)).unwrap();
        let id1 = s.insert_with("beta", |id| dummy(id, "beta", 200)).unwrap();
        assert_eq!(id0, EntityId(0));
        assert_eq!(id1, EntityId(1));
        assert_eq!(s.len(), 2);
        assert_eq!(s.get(id0).unwrap().payload, 100);
        assert_eq!(s.find("beta").unwrap().payload, 200);
        assert!(s.find("missing").is_none());
        // Round-trip: id ↔ name.
        assert_eq!(s.find("alpha").unwrap().id(), id0);
    }

    #[test]
    fn duplicate_name_rejected() {
        let mut s = EntityStore::<Dummy>::new();
        assert!(s.insert_with("a", |id| dummy(id, "a", 1)).is_some());
        assert!(s.insert_with("a", |id| dummy(id, "a", 2)).is_none());
        assert_eq!(s.len(), 1);
        assert_eq!(s.find("a").unwrap().payload, 1);
    }

    #[test]
    fn ids_are_never_reused_after_removal() {
        let mut s = EntityStore::<Dummy>::new();
        let id0 = s.insert_with("a", |id| dummy(id, "a", 1)).unwrap();
        let _id1 = s.insert_with("b", |id| dummy(id, "b", 2)).unwrap();
        assert!(s.remove("a").is_some());
        assert_eq!(s.len(), 1);
        // Inserting "c" mints a FRESH id; the slot at index 0 stays
        // a None hole (never reused — see module docs).
        let id2 = s.insert_with("c", |id| dummy(id, "c", 3)).unwrap();
        assert_eq!(id2, EntityId(2), "ids are never reused");
        // "a" is free for reuse as a NAME.
        let id3 = s.insert_with("a", |id| dummy(id, "a", 4)).unwrap();
        assert_eq!(id3, EntityId(3));
        assert_eq!(s.len(), 3);
        // The original id0 is no longer reachable (its slot was
        // taken by `None`).
        assert!(s.get(id0).is_none());
        // But the new "a" lives at id3.
        assert_eq!(s.find("a").unwrap().id(), id3);
    }

    #[test]
    fn iter_preserves_registration_order_skipping_removed() {
        let mut s = EntityStore::<Dummy>::new();
        s.insert_with("x", |id| dummy(id, "x", 0)).unwrap();
        s.insert_with("y", |id| dummy(id, "y", 1)).unwrap();
        s.insert_with("z", |id| dummy(id, "z", 2)).unwrap();
        s.remove("y").unwrap();
        let names: Vec<&str> = s.iter().map(|e| e.name()).collect();
        assert_eq!(names, vec!["x", "z"]);
        assert_eq!(s.names(), vec!["x", "z"]);
    }

    #[test]
    fn mutable_access_works() {
        let mut s = EntityStore::<Dummy>::new();
        let id = s.insert_with("a", |id| dummy(id, "a", 1)).unwrap();
        s.get_mut(id).unwrap().payload = 999;
        assert_eq!(s.find("a").unwrap().payload, 999);
        s.find_mut("a").unwrap().payload = 42;
        assert_eq!(s.get(id).unwrap().payload, 42);
    }

    #[test]
    fn empty_store_iterators_are_empty() {
        let s = EntityStore::<Dummy>::new();
        assert!(s.is_empty());
        assert_eq!(s.iter().count(), 0);
        assert_eq!(s.names().len(), 0);
    }

    #[test]
    fn display_format() {
        let id = EntityId(42);
        assert_eq!(format!("{id}"), "EntityId(42)");
        assert_eq!(id.raw(), 42);
    }
}

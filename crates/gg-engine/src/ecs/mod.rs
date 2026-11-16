//! The engine's ECS:
//!
//! - entities, which are handles, and their components, data of any reflected type, kept in
//!   archetype tables (a table for each set of component types) and found by queries or by name;
//! - entities' persistent IDs;
//! - the hierarchy, in which parents carry their children along;
//! - scenes, the world's entities as text;
//! - operations, the authoring changes made to it, each with its inverse, and their log;
//! - snapshots, the world copied whole, which restore it;
//! - the world's hash, which two runs compare at each tick;
//! - what tells two worlds apart, field by field;
//! - resources, what a world holds beside its entities.

mod by_name;
mod declared;
mod diff;
mod file;
mod hash;
mod hierarchy;
mod id;
mod log;
mod nodes;
mod ops;
mod prefab;
mod query;
mod resources;
mod scene;
mod snapshot;
mod table;

use std::any::TypeId;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt;
use std::hash::{BuildHasher, RandomState};
use std::mem;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gg_core::{Error, Result};

pub use by_name::Entities;
pub use declared::Kind;
pub use diff::Difference;
#[cfg(feature = "dev")]
pub(crate) use diff::SceneEdit;
pub use hash::{Digest, WorldHash};
pub(crate) use hash::{bytes_digest, text_digest};
use hierarchy::Links;
pub use hierarchy::{Parent, Pose};
pub use id::EntityId;
use id::IdStream;
pub(crate) use id::mix64;
pub use log::Log;
pub(crate) use nodes::node_id;
pub use ops::Operation;
pub use query::{Query, QueryIter, QueryIterMut, ReadOnlyQuery};
use resources::Resources;
pub use resources::{Resource, Saved};
pub(crate) use scene::read_scene_from;
pub(crate) use scene::{MAX_DEPTH, Parsed, float, quote};
pub use scene::{read_scene, write_scene};
use table::{AnyColumn, Components, Row};
pub(crate) use table::{ComponentId, Table};

use crate::assets::Assets;
use crate::reflect::Reflect;

/// Data an entity can have: a struct or enum with `#[derive(Clone, Component)]`. The derive also
/// implements [`Reflect`], so tools reach it by name, and [`Writable`]. `Clone` lets snapshots
/// copy the world; `Send + Sync` lets [`par_for_each`](QueryIterMut::par_for_each) hand rows to
/// job threads. A type registers when an entity first has one, or through [`World::register`].
#[diagnostic::on_unimplemented(
    message = "`{Self}` isn't a component",
    note = "a type becomes one with `#[derive(Clone, gg_engine::Component)]`"
)]
pub trait Component: Reflect + Clone {
    /// What a scene builds its component on, to supply what's opaque (a field marked
    /// `#[reflect(opaque)]`, which scenes leave out): it uses
    /// [`with_value`](Reflect::with_value). `None`, the derive's, uses
    /// [`from_value`](Reflect::from_value), which fails on an opaque field, so such a type
    /// implements `Component` by hand to load.
    fn base() -> Option<Self> {
        None
    }

    /// Its exclusive field, if any: a `bool` field of a struct with named fields that
    /// [`Operation`]s keep true on at most one entity. An operation that makes it true on an
    /// entity (a create, an add or a set) makes it false on every other; undoing restores them,
    /// as one edit. Code and loaded scenes may make several true. [`Camera`](crate::Camera)'s
    /// `primary` is one. `None`, the derive's.
    fn exclusive() -> Option<&'static str> {
        None
    }

    /// Its fields that a delete clears: `Option<Entity>` fields of a struct with named fields. A
    /// [`Log`]'s delete of an entity, or of what's under it, first sets each to `None` on every
    /// surviving entity whose field names a deleted one. That is part of the same edit, and the
    /// undo sets it to the entity remade under the named one's ID, as a scene file's relink of an
    /// instance does. So a component that holds two entities together, such as a
    /// [`Joint3D`](crate::Joint3D)'s bodies, never names one gone, which a scene couldn't save
    /// (`ecs.no_entity`). `None`, the derive's.
    fn cleared() -> &'static [&'static str] {
        &[]
    }

    /// What a snapshot file keeps of a component whose type is opaque as a whole, such as
    /// [`MeshDraw`](crate::MeshDraw): a line of text that [`load_opaque`](Self::load_opaque) reads
    /// back, in a run whose layers added the same assets in the same order as the run it was taken
    /// from. `None`, the derive's, keeps nothing, which a restart leaves out.
    fn save_opaque(&self) -> Option<String> {
        None
    }

    /// Makes a component from `text`, which [`save_opaque`](Self::save_opaque) gave.
    ///
    /// Fails with `snapshot.opaque` by default, for a type that keeps nothing, or as the type
    /// says for text it doesn't take.
    fn load_opaque(_text: &str) -> Result<Self> {
        Err(Error::new(
            "snapshot.opaque",
            "its type keeps nothing opaque in snapshots",
        ))
    }
}

/// A component code may spawn, insert, remove and edit: through [`World::spawn`],
/// [`World::insert`], [`World::remove`], [`World::get_mut`] and `&mut T` in
/// [`World::query_mut`]. The derive implements it; a hand-written component implements it beside
/// `Component`. Every engine component is one but [`Parent`], which the world keeps so the
/// hierarchy stays whole: only [`World::set_parent`] changes it.
#[diagnostic::on_unimplemented(
    message = "`{Self}` isn't a component that code changes",
    note = "a type becomes one with `#[derive(Clone, gg_engine::Component)]`, or with \
            `impl gg_engine::Writable` beside its own `impl Component`; the world keeps `Parent`, \
            which `World::set_parent` changes"
)]
pub trait Writable: Component {}

/// Makes a struct or enum a [`Component`] and [`Writable`].
pub use gg_macros::Component;

/// What an entity is spawned with: a [`Writable`] component, or a tuple of up to 8 of different
/// types.
#[diagnostic::on_unimplemented(
    message = "`{Self}` isn't a bundle of components",
    note = "`World::spawn` takes a component, a type that derives `gg_engine::Component`, or \
            a tuple of up to 8 of different types; `World::insert` takes one; the world keeps \
            `Parent`, which `World::set_parent` gives"
)]
pub trait Bundle: seal::Bundle {}

/// A handle to an entity. It never names another entity once despawned, unless its world is
/// restored from a snapshot taken before the spawn. It prints as slot and generation, such as
/// `7v2`. It's the world's own: files, tools and diagnostics use the [`EntityId`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Entity {
    /// Its slot.
    index: u32,
    /// Which use of the slot it is.
    generation: NonZeroU32,
}

impl Entity {
    /// Names no entity in any world, for one dead where it's read: its slot is the hierarchy's
    /// "none", which no entity takes.
    pub(crate) const DEAD: Self = Self {
        index: u32::MAX,
        generation: NonZeroU32::MIN,
    };

    /// Its slot as an index; no other living entity has it.
    pub(crate) fn slot(self) -> usize {
        self.index as usize
    }

    /// As a script holds it: slot in the low 32 bits, generation (never 0) in the high 32.
    pub(crate) fn to_bits(self) -> u64 {
        u64::from(self.generation.get()) << 32 | u64::from(self.index)
    }

    /// The entity whose [`to_bits`](Self::to_bits) are `bits`; `None` if no entity's are.
    pub(crate) fn from_bits(bits: u64) -> Option<Self> {
        Some(Self {
            index: bits as u32,
            generation: NonZeroU32::new((bits >> 32) as u32)?,
        })
    }
}

impl fmt::Display for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}v{}", self.index, self.generation)
    }
}

impl fmt::Debug for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Entities and their components. Each set of component types has a table: rows are the entities
/// with exactly those types, in the order they joined; columns are each type's values.
/// Despawning an entity, or giving or taking a component, moves the table's last row into its
/// place. So query order follows from the changes made and is the same every time, but a game
/// shouldn't depend on it: order anything (such as what's drawn over what) by component values or
/// entity IDs.
///
/// A copy of the world is a snapshot: [`clone`](Clone::clone) takes one, and
/// [`clone_from`](Clone::clone_from) restores the world from one, or takes one into a copy kept
/// from before. A copy holds all of the world:
/// - each slot under its generation, the free ones in the order they're taken, and those kept for
///   entities a [`Log`] can put back, with their IDs;
/// - each entity under its handle and ID, in its table and row, with its parent, children and
///   last interpolation reset;
/// - the tables in the order made (a restore cuts those made since);
/// - the ID stream, the component types, and the [`Resource`]s.
///
/// So a restored world goes on as the original did: given the same changes, it spawns the same
/// handles with the same IDs, and its queries find the same order. But it's another world: drawing
/// sorts its rows again and doesn't interpolate into it from the world before, and it refuses what
/// undoes an operation applied before. So the slots kept when the snapshot was taken stay vacant,
/// with their IDs, until [`clear`](Self::clear) or [`replace_scene`](Self::replace_scene) frees
/// them.
///
/// A snapshot taken or restored again copies into the memory the world holds. That allocates
/// nothing while the tables, component types and world size stay as they were, for components and
/// resources whose `Clone` allocates nothing.
///
/// ```
/// use gg_engine::{Transform, World};
///
/// let mut world = World::default();
/// let entity = world.spawn(Transform::IDENTITY);
/// let id = world.id(entity);
/// let snapshot = world.clone();
/// world.despawn(entity)?;
/// world.clone_from(&snapshot);
/// assert_eq!(world.id(entity), id);
/// # Ok::<(), gg_engine::Error>(())
/// ```
pub struct World {
    /// Each slot's generation, and where its entity is if it's alive.
    meta: Vec<Meta>,
    /// The slots free for new entities, the last freed taken first.
    free: Vec<u32>,
    /// Each slot's persistent ID. Kept beside `Meta`, which every handle lookup reads, so `Meta`
    /// stays small. Read through [`World::id`], which checks the handle. `None` while the slot is
    /// free, vacant or retired.
    entity_ids: Vec<Option<EntityId>>,
    /// Each slot's entity's last [`reset_interpolation`](World::reset_interpolation), as the
    /// count of resets it made; 0 if none. Beside `Meta` like the IDs: only drawing reads it.
    /// Neither it nor the count is hashed, saved or recorded.
    resets: Vec<u64>,
    /// How many interpolation resets there have been; numbers each reset.
    reset_count: u64,
    /// Each living entity by ID; looked up, never iterated. SipHash with a random key, so IDs from
    /// files can't be chosen to collide.
    by_id: HashMap<EntityId, Entity>,
    /// The room `by_id` got as it last grew; removals' leftover entries hide it from `capacity`.
    /// [`find_by_id`](Self::find_by_id) keeps it at most half full.
    id_room: usize,
    /// How many vacant slots keep each ID: IDs of entities a delete took, which no draw gives to
    /// another entity while an undo can give them back. Looked up, never iterated, hashed like
    /// `by_id`.
    kept: HashMap<EntityId, u32>,
    stream: IdStream,
    /// Which world this is among the process's, so what's kept per world (drawing's order, a
    /// pass of scripts' entity list, undo entries) never passes for another's, as when a game
    /// replaces its world. A copy takes another, as does a world restored, cleared, renewed or
    /// given a scene's entities.
    serial: u64,
    /// How many times entities' rows have changed (spawned, despawned, moved to other tables).
    /// Drawing keeps its sorted order while this and `serial` stay.
    version: u64,
    /// What each table marks changed rows with, as [`Changed`](table::Changed) says. The engine
    /// moves it on as [`advance`](Self::advance) says, so drawing finds what changed since it
    /// last drew, and a server what changed since a client's state. Neither it nor the marks are
    /// hashed, saved or recorded.
    change_tick: u64,
    /// In the order made, which is the order queries search them.
    tables: Vec<Table>,
    /// Each table by sorted component types; looked up, never iterated, so its random order never
    /// shows.
    by_signature: HashMap<Box<[ComponentId]>, u32>,
    components: Components,
    /// The component types of the table an entity moves to.
    scratch: Vec<ComponentId>,
    /// Lists of entities kept for their memory, for what lists entities while changing the world
    /// (a model spawning, a pass of scripts): as many as were ever taken at once, since a pass of
    /// scripts run within another takes its own.
    lists: Vec<Vec<Entity>>,
    resources: Resources,
    /// Names the textures its components show, where they cross into text. It's the context's,
    /// shared with copies, and the context gives it again after each layer's tick.
    assets: Arc<Assets>,
}

impl Default for World {
    /// An empty world with [`Parent`] registered, so tools find it, `set_field` refuses it by name
    /// in any world, and no game's type takes its name. Its IDs come from a randomly seeded
    /// stream, so each world's are its own.
    fn default() -> Self {
        // std seeds its hashers' keys from the OS's randomness once a thread and steps them for
        // each `RandomState`, so each world gets another seed.
        Self::seeded(RandomState::new().hash_one(()))
    }
}

/// How many worlds the process has made, which numbers each.
static WORLDS: AtomicU64 = AtomicU64::new(0);

/// A serial no other world has had: for a world made, copied, restored, cleared, renewed or
/// replaced, and for [`Components::revision`].
///
/// [`Components::revision`]: table::Components::revision
fn next_serial() -> u64 {
    WORLDS.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Meta {
    generation: NonZeroU32,
    /// `None` while the slot is free, vacant (kept for an entity a delete took, which its inverse
    /// puts back) or retired.
    location: Option<Location>,
    /// Its entity's parent and children; a free slot has none.
    links: Links,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Location {
    table: u32,
    row: u32,
}

/// The most components a bundle has.
const MOST: usize = 8;

impl World {
    /// An empty world like [`World::default`], with IDs drawn from `seed`.
    pub(crate) fn seeded(seed: u64) -> Self {
        let mut components = Components::default();
        components
            .register::<Parent>()
            .expect("the world's own component describes itself");
        Self {
            meta: Vec::new(),
            free: Vec::new(),
            entity_ids: Vec::new(),
            resets: Vec::new(),
            reset_count: 0,
            by_id: HashMap::new(),
            id_room: 0,
            kept: HashMap::new(),
            stream: IdStream::new(seed),
            serial: next_serial(),
            version: 0,
            change_tick: 1,
            tables: Vec::new(),
            by_signature: HashMap::new(),
            components,
            scratch: Vec::new(),
            lists: Vec::new(),
            resources: Resources::default(),
            assets: Arc::default(),
        }
    }

    pub(crate) fn assets(&self) -> &Assets {
        &self.assets
    }

    pub(crate) fn shared_assets(&self) -> &Arc<Assets> {
        &self.assets
    }

    /// Copies from before don't see the change.
    #[cfg(test)]
    pub(crate) fn assets_mut(&mut self) -> &mut Assets {
        Arc::make_mut(&mut self.assets)
    }

    /// Shares the context's `assets` unless it does already. The world may be a replacement that
    /// names others, a new one that names none, or a copy from before an asset was added; the
    /// context's hold all the layers added, which only grow.
    pub(crate) fn share_assets(&mut self, assets: &Arc<Assets>) {
        if !Arc::ptr_eq(&self.assets, assets) {
            self.assets.clone_from(assets);
        }
    }

    /// Makes an entity with `components`.
    ///
    /// # Panics
    ///
    /// - If `components` has a type twice: an entity has one of each.
    /// - If a type fails to [`register`](Self::register).
    pub fn spawn<B: Bundle>(&mut self, components: B) -> Entity {
        self.spawn_as(None, components)
    }

    /// Like [`spawn`](Self::spawn), with ID `id` (no living entity has it) if given, as a model's
    /// node's is, else drawn.
    fn spawn_as<B: Bundle>(&mut self, id: Option<EntityId>, components: B) -> Entity {
        let mut ids = [ComponentId::default(); MOST];
        let ids = &mut ids[..B::COUNT];
        B::ids(&mut self.components, ids);
        let mut signature = [ComponentId::default(); MOST];
        let signature = &mut signature[..B::COUNT];
        signature.copy_from_slice(ids);
        signature.sort_unstable();
        if let Some(twice) = signature.windows(2).find(|pair| pair[0] == pair[1]) {
            panic!(
                "a bundle has `{}` twice, but an entity has one of each component",
                self.components.name(twice[0])
            );
        }
        let table = self.table(signature);
        let entity = self.take_slot();
        let id = match id {
            Some(id) => {
                let given = self.find_by_id(id, entity);
                assert!(
                    given.is_none(),
                    "a spawn gives ID {id}, which a living entity has"
                );
                id
            }
            None => self.draw_id(entity, &[]),
        };
        self.entity_ids[entity.index as usize] = Some(id);
        let into = &mut self.tables[table as usize];
        let row = into.entities().len() as u32;
        components.push(into, ids);
        into.push_entity(entity, self.change_tick);
        self.meta[entity.index as usize].location = Some(Location { table, row });
        self.version += 1;
        entity
    }

    /// Draws an ID for `entity` from the stream, and finds it by it from now on. A draw that meets
    /// a living entity's ID (as when the stream has walked into the run of the session that saved
    /// a loaded scene), one a delete keeps for its undo, or one of `given` (a duplicate's
    /// not-yet-placed copies) reseeds the stream and draws again.
    fn draw_id(&mut self, entity: Entity, given: &[EntityId]) -> EntityId {
        loop {
            let id = self.stream.draw();
            if self.reserved(id) || given.contains(&id) {
                tracing::debug!(code = "ecs.reseed", id = %id, "Drew an ID that's taken");
                self.stream.reseed();
                continue;
            }
            self.find_by_id(id, entity);
            return id;
        }
    }

    /// Whether no draw may give `id`: a living entity has it, or a delete keeps it for its undo.
    fn reserved(&self, id: EntityId) -> bool {
        self.by_id.contains_key(&id) || self.kept.contains_key(&id)
    }

    /// Spawns an entity with ID `id` (no living entity has it) and the components a scene built in
    /// `row`. The entity is `expected`, as [`next_slots`](Self::next_slots) predicted.
    fn spawn_built(&mut self, id: EntityId, row: Row, expected: Entity) {
        let entity = self.take_slot();
        assert_eq!(
            entity, expected,
            "a scene's entity takes the handle predicted for it"
        );
        let given = self.find_by_id(id, entity);
        assert!(
            given.is_none(),
            "a scene gives ID {id}, which a living entity has"
        );
        self.place(entity, id, row);
    }

    /// An empty list in memory kept by [`keep_list`](Self::keep_list), for what lists entities
    /// while changing the world, such as [`Model::spawn`](crate::Model::spawn) and
    /// [`run_scripts`](crate::run_scripts). It allocates nothing once the world has listed as many
    /// entities, in as many lists at once.
    pub(crate) fn take_list(&mut self) -> Vec<Entity> {
        let mut list = self.lists.pop().unwrap_or_default();
        list.clear();
        list
    }

    /// Keeps `list`'s memory for a [`take_list`](Self::take_list) after.
    pub(crate) fn keep_list(&mut self, list: Vec<Entity>) {
        self.lists.push(list);
    }

    /// Puts `entity`, found by `id` already, into the table of the components in `row`.
    fn place(&mut self, entity: Entity, id: EntityId, row: Row) {
        let mut signature = mem::take(&mut self.scratch);
        signature.clear();
        signature.extend(row.iter().map(|&(component, _)| component));
        let table = self.table(&signature);
        self.scratch = signature;
        self.entity_ids[entity.index as usize] = Some(id);
        let into = &mut self.tables[table as usize];
        let row_at = into.entities().len() as u32;
        for (position, (_, mut from)) in row.into_iter().enumerate() {
            into.push_from(position, &mut *from);
        }
        into.push_entity(entity, self.change_tick);
        let at = Location { table, row: row_at };
        self.meta[entity.index as usize].location = Some(at);
        self.version += 1;
    }

    /// Gives `entity` `component`, replacing one of its type.
    ///
    /// Fails with `ecs.no_entity` if it isn't alive, and like [`register`](Self::register) if `T`
    /// can't register.
    pub fn insert<T: Writable>(&mut self, entity: Entity, component: T) -> Result<()> {
        self.put(entity, component)
    }

    /// [`insert`](Self::insert) for any component, such as [`Parent`].
    fn put<T: Component>(&mut self, entity: Entity, component: T) -> Result<()> {
        let at = self.locate(entity)?;
        let id = self.components.register::<T>()?;
        let (row, tick) = (at.row as usize, self.change_tick);
        if let Some(value) = self.tables[at.table as usize].get_mut::<T>(id, row, tick) {
            *value = component;
            return Ok(());
        }
        let to = self.table_with(at, id);
        self.move_entity(entity, at, to);
        self.tables[to as usize].push(id, component);
        Ok(())
    }

    /// Takes `entity`'s component of type `T` away, and returns it; `None` if it has none.
    ///
    /// Fails with `ecs.no_entity` if it isn't alive.
    pub fn remove<T: Writable>(&mut self, entity: Entity) -> Result<Option<T>> {
        self.take(entity)
    }

    /// [`remove`](Self::remove) for any component, such as [`Parent`].
    fn take<T: Component>(&mut self, entity: Entity) -> Result<Option<T>> {
        let at = self.locate(entity)?;
        let Some(id) = self.components.get(TypeId::of::<T>()) else {
            return Ok(None);
        };
        let table = &mut self.tables[at.table as usize];
        let Some(component) = table.take::<T>(id, at.row as usize) else {
            return Ok(None);
        };
        let to = self.table_without(at, id);
        self.move_entity(entity, at, to);
        Ok(Some(component))
    }

    /// [`put`](Self::put) for an unknown type: gives `entity`, at `at` and without component
    /// `id`, the value in one-row `column`.
    fn give(
        &mut self,
        entity: Entity,
        at: Location,
        id: ComponentId,
        mut column: Box<dyn AnyColumn>,
    ) {
        let to = self.table_with(at, id);
        self.move_entity(entity, at, to);
        let into = &mut self.tables[to as usize];
        let position = into.position(id).expect("the table has the component");
        into.push_from(position, &mut *column);
    }

    /// [`take`](Self::take) for an unknown type: takes `entity`'s component `id`, at `at`, into a
    /// one-row column.
    fn take_out(&mut self, entity: Entity, at: Location, id: ComponentId) -> Box<dyn AnyColumn> {
        let mut column = self.components.new_column(id);
        let from = &mut self.tables[at.table as usize];
        let position = from.position(id).expect("the entity has the component");
        from.take_out(position, at.row as usize, &mut *column);
        let to = self.table_without(at, id);
        self.move_entity(entity, at, to);
        column
    }

    /// The table of the types of the entity at `at` plus `id`, which it lacks.
    fn table_with(&mut self, at: Location, id: ComponentId) -> u32 {
        let mut signature = mem::take(&mut self.scratch);
        signature.clear();
        signature.extend_from_slice(self.tables[at.table as usize].components());
        let place = signature.binary_search(&id).unwrap_err();
        signature.insert(place, id);
        let to = self.table(&signature);
        self.scratch = signature;
        to
    }

    /// The table of the types of the entity at `at` minus `id`.
    fn table_without(&mut self, at: Location, id: ComponentId) -> u32 {
        let mut signature = mem::take(&mut self.scratch);
        signature.clear();
        let from = self.tables[at.table as usize].components();
        signature.extend(from.iter().filter(|&&other| other != id));
        let to = self.table(&signature);
        self.scratch = signature;
        to
    }

    /// `entity`'s component of type `T`; `None` if it has none, or isn't alive.
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        let at = self.location(entity)?;
        let id = self.components.get(TypeId::of::<T>())?;
        self.tables[at.table as usize]
            .column::<T>(id)?
            .get(at.row as usize)
    }

    /// `entity`'s component of type `T`, to change; `None` if it has none, or isn't alive.
    pub fn get_mut<T: Writable>(&mut self, entity: Entity) -> Option<&mut T> {
        let at = self.location(entity)?;
        let id = self.components.get(TypeId::of::<T>())?;
        let tick = self.change_tick;
        self.tables[at.table as usize].get_mut(id, at.row as usize, tick)
    }

    /// Whether `entity` is alive.
    pub fn contains(&self, entity: Entity) -> bool {
        self.location(entity).is_some()
    }

    /// Draws `entity`'s own transform where this tick leaves it, not interpolated from where the
    /// tick found it, until its next tick. For a move that jumps, such as a teleport or a wrap;
    /// call it before or after the move, within the tick. Its descendants jump with it; its
    /// ancestors still carry it on their way. Only drawing reads it: it changes no hash, scene,
    /// recording or drawing order.
    ///
    /// Fails with `ecs.no_entity` if it isn't alive.
    pub fn reset_interpolation(&mut self, entity: Entity) -> Result<()> {
        self.locate(entity)?;
        self.reset_count += 1;
        self.resets[entity.slot()] = self.reset_count;
        Ok(())
    }

    /// Despawns every entity and lets go of what logs keep for undos (the slots and IDs of
    /// deleted entities), so no handle from before names an entity. The component types and
    /// [`Resource`]s stay, and no ID drawn before is drawn again. The world becomes another (see
    /// [`serial`](Self::serial)) and refuses what undoes an earlier operation: clear any [`Log`]
    /// of it too.
    pub fn clear(&mut self) {
        let _zone = gg_core::zone!();
        let mut emptied = self.emptied();
        emptied.resources = mem::take(&mut self.resources);
        *self = emptied;
    }

    /// Like [`clear`](Self::clear), but keeps the entities: each slot a log kept for a deleted
    /// entity is [freed](Self::free_slot) under its next generation and its ID let go. The world
    /// becomes another and refuses every earlier log entry, so drop any log of it for a fresh
    /// history, as after the tool API's `scene open` and `scene new`, and on a lockstep host
    /// sending its joiner its world.
    ///
    /// Slots are freed in slot order, after the free ones, as a load of the world's text frees
    /// those it names vacant. So a host and its joiner give the same slots to later spawns, as
    /// code ordering by [`Entity`] needs. A retired slot stays retired.
    pub(crate) fn renew(&mut self) {
        let mut free = vec![false; self.meta.len()];
        for &index in &self.free {
            free[index as usize] = true;
        }
        // Vacant or retired; a retired slot stays so.
        for (index, free) in (0..).zip(free) {
            if !free && self.meta[index as usize].location.is_none() {
                self.free_slot(index);
            }
        }
        self.kept.clear();
        self.serial = next_serial();
        // Its rows changed, as the serial says too.
        self.version += 1;
    }

    /// The entities with every component `Q` reads, table by table, such as
    /// `world.query::<(Entity, &Transform)>()`. Read-only; [`query_mut`](Self::query_mut) may
    /// change components.
    ///
    /// # Panics
    ///
    /// If `Q` names a component twice.
    pub fn query<Q: ReadOnlyQuery>(&self) -> QueryIter<'_, Q> {
        QueryIter::new(&self.tables, &self.components)
    }

    /// Like [`query`](Self::query), with `&mut T` terms too, such as
    /// `world.query_mut::<(&Velocity, &mut Transform)>()`. `for_each` is the tightest loop,
    /// over each table's columns.
    ///
    /// # Panics
    ///
    /// If `Q` names a component twice: two terms would reach the same values.
    pub fn query_mut<Q: Query>(&mut self) -> QueryIterMut<'_, Q> {
        QueryIterMut::new(&mut self.tables, &self.components, self.change_tick)
    }

    /// The tables, in the order made.
    pub(crate) fn tables(&self) -> &[Table] {
        &self.tables
    }

    /// `T`'s id, if registered ([`Parent`] is in every world).
    pub(crate) fn component<T: Component>(&self) -> Option<ComponentId> {
        self.components.get(TypeId::of::<T>())
    }

    /// Which world this is among the process's. A copy is another, and so is a world restored,
    /// [cleared](Self::clear) or [replaced](Self::replace_scene). Another world refuses a
    /// [`Log`]'s earlier entries, so whoever keeps a log, or entities picked, lets go of them when
    /// the serial changes.
    pub fn serial(&self) -> u64 {
        self.serial
    }

    /// The serial, and how many times entities' rows have changed: drawing keeps its sorted order
    /// while both stay, and a pass of scripts ends once the world isn't the one it listed.
    pub(crate) fn version(&self) -> (u64, u64) {
        (self.serial, self.version)
    }

    /// The change tick, from 1 on, which tables mark changed rows with (see
    /// [`Changed`](table::Changed)). Only [`advance`](Self::advance) moves it, so later marks are
    /// past earlier ones.
    pub(crate) fn change_tick(&self) -> u64 {
        self.change_tick
    }

    /// Moves the change tick on as each tick starts and each frame has drawn the world, so
    /// drawing sees what changed since it last looked. A server's world, which draws nothing,
    /// advances as each tick ends, so each client's state sees what changed since its tick.
    pub(crate) fn advance(&mut self) {
        self.change_tick += 1;
    }

    /// How many interpolation resets there have been; drawing keeps it before a tick.
    pub(crate) fn resets(&self) -> u64 {
        self.reset_count
    }

    /// Whether living `entity`'s interpolation was reset after `count` resets.
    pub(crate) fn reset_since(&self, entity: Entity, count: u64) -> bool {
        self.resets[entity.slot()] > count
    }

    /// How many slots, free or not: every entity's [`slot`](Entity::slot) is below it.
    pub(crate) fn slots(&self) -> usize {
        self.meta.len()
    }

    /// The table and row indices of living `entity`, for drawing.
    pub(crate) fn table_row(&self, entity: Entity) -> Option<(u32, u32)> {
        self.location(entity).map(|at| (at.table, at.row))
    }

    /// Where `entity` is, if it's alive.
    fn location(&self, entity: Entity) -> Option<Location> {
        let meta = self.meta.get(entity.index as usize)?;
        meta.location
            .filter(|_| meta.generation == entity.generation)
    }

    /// Where `entity` is, or the error that it isn't alive.
    fn locate(&self, entity: Entity) -> Result<Location> {
        self.location(entity).ok_or_else(|| {
            Error::new(
                "ecs.no_entity",
                format!(
                    "entity {entity} isn't in the world: it was despawned, or is another world's"
                ),
            )
        })
    }

    /// The table of sorted `signature`, made if missing.
    fn table(&mut self, signature: &[ComponentId]) -> u32 {
        if let Some(&table) = self.by_signature.get(signature) {
            return table;
        }
        let table = u32::try_from(self.tables.len()).expect("fewer than 2^32 tables");
        self.tables.push(Table::new(signature, &self.components));
        self.by_signature.insert(signature.into(), table);
        table
    }

    /// A slot for a new entity, the last freed if there's one, and the entity in it.
    fn take_slot(&mut self) -> Entity {
        if let Some(index) = self.free.pop() {
            let generation = self.meta[index as usize].generation;
            // Its last entity's resets aren't the new one's.
            self.resets[index as usize] = 0;
            return Entity { index, generation };
        }
        // The last index is the hierarchy's for none.
        let index = u32::try_from(self.meta.len())
            .ok()
            .filter(|&index| index != u32::MAX)
            .expect("fewer than 2^32 - 1 entities");
        let generation = NonZeroU32::MIN;
        self.meta.push(Meta {
            generation,
            location: None,
            links: Links::NONE,
        });
        self.entity_ids.push(None);
        self.resets.push(0);
        Entity { index, generation }
    }

    /// The handles the next spawns take, as [`take_slot`](Self::take_slot) gives them: free slots,
    /// last freed first, then new ones.
    fn next_slots(&self) -> impl Iterator<Item = Entity> + '_ {
        let free = self.free.iter().rev().map(|&index| Entity {
            index,
            generation: self.meta[index as usize].generation,
        });
        let new = (self.meta.len()..).map(|index| Entity {
            index: u32::try_from(index)
                .ok()
                .filter(|&index| index != u32::MAX)
                .expect("fewer than 2^32 - 1 entities"),
            generation: NonZeroU32::MIN,
        });
        free.chain(new)
    }

    /// Removes the living entity in slot `index`, without parent or children, and its components.
    fn remove_slot(&mut self, index: u32) {
        let at = self.meta[index as usize].location;
        let at = at.expect("the slot's entity is alive");
        let moved = self.tables[at.table as usize].swap_remove(at.row as usize);
        self.leave(index, at, moved);
        self.free_slot(index);
    }

    /// Like [`remove_slot`](Self::remove_slot), but returns the entity's components and leaves the
    /// slot vacant, not free: same generation, taken by no entity until freed, so the entity can
    /// come back under its handle, as an undone delete does. Its ID stays out of draws until then.
    fn vacate(&mut self, index: u32) -> Row {
        let at = self.meta[index as usize].location;
        let at = at.expect("the slot's entity is alive");
        let table = &mut self.tables[at.table as usize];
        let (row, moved) = table.take_row(at.row as usize, &self.components);
        let id = self.leave(index, at, moved);
        *self.kept.entry(id).or_insert(0) += 1;
        row
    }

    /// Finishes taking slot `index`'s entity, at `at`, out of its table, whose row `moved` (if any)
    /// took. Returns its ID, which goes from the world with it.
    fn leave(&mut self, index: u32, at: Location, moved: Option<Entity>) -> EntityId {
        if let Some(moved) = moved {
            self.meta[moved.index as usize].location = Some(at);
        }
        let id = self.entity_ids[index as usize].take();
        let id = id.expect("a living entity has an ID");
        self.by_id.remove(&id);
        self.version += 1;
        self.meta[index as usize].location = None;
        id
    }

    /// Releases one vacant slot's keeping of `id`, as its entity comes back or the slot is freed.
    /// Another slot may still keep it: a create or load gave it to another entity meanwhile, which
    /// a delete took too.
    fn free_id(&mut self, id: EntityId) {
        let Entry::Occupied(mut kept) = self.kept.entry(id) else {
            unreachable!("a vacant slot keeps its entity's ID");
        };
        if *kept.get() > 1 {
            *kept.get_mut() -= 1;
        } else {
            kept.remove();
        }
    }

    /// Frees slot `index`, which no entity is in, under its next generation, so the old handle
    /// never names another entity.
    fn free_slot(&mut self, index: u32) {
        let meta = &mut self.meta[index as usize];
        // A slot whose generation can't go higher retires, so no handle names another entity: its
        // `Meta` stays, for every 4 billion entities spawned in the slot.
        if let Some(next) = meta.generation.checked_add(1) {
            meta.generation = next;
            self.free.push(index);
        }
    }

    /// This world without its entities. Each slot that held one, or that a log kept for a deleted
    /// one, is [freed](Self::free_slot) under its next generation, after the free ones and in slot
    /// order, as despawning each in turn would, so no old handle names an entity. It keeps the
    /// component types, ID stream (so none is drawn twice) and reset count. It has no tables, as a
    /// new world has none, so it costs the slots, not the components; and no resources, which the
    /// caller moves over, since copying would allocate.
    fn emptied(&self) -> World {
        let slots = self.meta.len();
        let mut was_free = vec![false; slots];
        for &index in &self.free {
            was_free[index as usize] = true;
        }
        let meta = self.meta.iter().map(|meta| Meta {
            location: None,
            links: Links::NONE,
            ..*meta
        });
        let mut world = World {
            meta: meta.collect(),
            free: self.free.clone(),
            entity_ids: vec![None; slots],
            resets: vec![0; slots],
            reset_count: self.reset_count,
            by_id: HashMap::new(),
            id_room: 0,
            kept: HashMap::new(),
            stream: self.stream,
            serial: next_serial(),
            // Its rows changed, as the serial says too.
            version: self.version + 1,
            change_tick: self.change_tick,
            tables: Vec::new(),
            by_signature: HashMap::new(),
            components: self.components.clone(),
            scratch: Vec::new(),
            lists: Vec::new(),
            resources: Resources::default(),
            assets: self.assets.clone(),
        };
        // Alive, vacant or retired; a retired slot stays so.
        for (index, free) in (0..).zip(was_free) {
            if !free {
                world.free_slot(index);
            }
        }
        world
    }

    /// A world of no entities with this one's component types and assets, for a prefab's text to
    /// load its template into and a scene to parse in (see [parse_scene](Self::parse_scene)). It
    /// costs neither this world's slots nor tables.
    pub(crate) fn scratch(&self) -> World {
        let mut world = World::seeded(0);
        world.components = self.components.clone();
        world.assets = Arc::clone(&self.assets);
        world
    }

    /// Moves `entity`, at `at`, to the end of table `to` with the components `to` has. The others
    /// must have left already.
    fn move_entity(&mut self, entity: Entity, at: Location, to: u32) {
        let [from, into] = self
            .tables
            .get_disjoint_mut([at.table as usize, to as usize])
            .expect("an entity moves between two tables");
        let row = into.entities().len() as u32;
        if let Some(moved) = from.move_row(at.row as usize, into, entity, self.change_tick) {
            self.meta[moved.index as usize].location = Some(at);
        }
        self.meta[entity.index as usize].location = Some(Location { table: to, row });
        self.version += 1;
    }
}

// The world can move to another thread, and be read from several.
const _: () = {
    const fn send_and_sync<T: Send + Sync>() {}
    send_and_sync::<World>();
};

/// What only this module implements.
mod seal {
    use super::table::{ComponentId, Components, Table};

    pub trait Bundle: Send + Sync + 'static {
        /// How many components it has.
        const COUNT: usize;

        /// Writes its components' ids to `ids`, `COUNT` of them, in its order.
        fn ids(components: &mut Components, ids: &mut [ComponentId]);

        /// Pushes its components onto their columns of `table`, whose ids are `ids`, in its order.
        fn push(self, table: &mut Table, ids: &[ComponentId]);
    }
}

impl<T: Writable> seal::Bundle for T {
    const COUNT: usize = 1;

    fn ids(components: &mut Components, ids: &mut [ComponentId]) {
        ids[0] = components.id::<T>();
    }

    fn push(self, table: &mut Table, ids: &[ComponentId]) {
        table.push(ids[0], self);
    }
}

impl<T: Writable> Bundle for T {}

impl seal::Bundle for () {
    const COUNT: usize = 0;

    fn ids(_: &mut Components, _: &mut [ComponentId]) {}

    fn push(self, _: &mut Table, _: &[ComponentId]) {}
}

impl Bundle for () {}

macro_rules! tuple_bundle {
    ($($T:ident $i:tt),+) => {
        impl<$($T: Writable),+> seal::Bundle for ($($T,)+) {
            const COUNT: usize = <[&str]>::len(&[$(stringify!($T)),+]);

            fn ids(components: &mut Components, ids: &mut [ComponentId]) {
                $(ids[$i] = components.id::<$T>();)+
            }

            fn push(self, table: &mut Table, ids: &[ComponentId]) {
                $(table.push(ids[$i], self.$i);)+
            }
        }

        impl<$($T: Writable),+> Bundle for ($($T,)+) {}
    };
}

tuple_bundle!(A 0);
tuple_bundle!(A 0, B 1);
tuple_bundle!(A 0, B 1, C 2);
tuple_bundle!(A 0, B 1, C 2, D 3);
tuple_bundle!(A 0, B 1, C 2, D 3, E 4);
tuple_bundle!(A 0, B 1, C 2, D 3, E 4, F 5);
tuple_bundle!(A 0, B 1, C 2, D 3, E 4, F 5, G 6);
tuple_bundle!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7);

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::iter;

    use super::hierarchy::tests::check_links;
    use super::id::splitmix64;
    use super::id::tests::{check_ids, vacated};
    use super::ops::tests::fixture;
    use super::*;
    use crate::reflect::Value;

    #[derive(Clone, Copy, Debug, PartialEq, Component)]
    struct Index(u32);

    #[derive(Clone, Copy, Debug, PartialEq, Component)]
    struct Marked;

    /// The change tick at which `entity`'s `T` last changed.
    fn changed_at<T: Component>(world: &World, entity: Entity) -> u64 {
        let at = world.location(entity).expect("alive");
        let table = &world.tables[at.table as usize];
        let id = world.component::<T>().expect("registered");
        table.changed(id).expect("it has one").row(at.row as usize)
    }

    /// Rows keep the tick they last changed at: on spawn or a move into a table, in every column;
    /// through `get_mut`, `insert` or a set, in that component alone; through a query's `&mut`
    /// term, in its column's rows only. A row keeps its ticks when it takes a leaver's place; a
    /// rewind changes every row.
    #[test]
    fn rows_keep_the_tick_they_last_changed_at() -> Result<()> {
        let mut world = World::default();
        let start = world.change_tick();
        let [a, b, c] = [0, 1, 2].map(|i| world.spawn((Index(i), crate::Transform::IDENTITY)));
        let snapshot = world.clone();
        let at = |world: &World, entity| {
            let index = changed_at::<Index>(world, entity) - start;
            (index, changed_at::<crate::Transform>(world, entity) - start)
        };
        assert_eq!([a, b, c].map(|entity| at(&world, entity)), [(0, 0); 3]);
        let index = world.component::<Index>().expect("registered");
        let table = &world.tables[world.location(a).expect("alive").table as usize];
        assert!(!table.changed(index).expect("it has one").since(start));

        world.advance();
        world.get_mut::<Index>(b).expect("it has one").0 = 9;
        world.advance();
        world.insert(c, Index(7))?;
        world.advance();
        world.set_field(a, "Transform", "translation.x", &Value::F64(2.5))?;
        assert_eq!(
            [a, b, c].map(|entity| at(&world, entity)),
            [(0, 3), (1, 0), (2, 0)],
        );
        let table = &world.tables[world.location(a).expect("alive").table as usize];
        assert!(table.changed(index).expect("it has one").since(start + 1));
        assert!(!table.changed(index).expect("it has one").since(start + 2));

        world.advance();
        for (_, transform) in world.query_mut::<(&Index, &mut crate::Transform)>() {
            transform.translation.y = 1.0;
        }
        assert_eq!(
            [a, b, c].map(|entity| at(&world, entity)),
            [(0, 4), (1, 4), (2, 4)],
        );

        world.advance();
        world.insert(b, Marked)?;
        assert_eq!(at(&world, b), (5, 5));
        assert_eq!(changed_at::<Marked>(&world, b) - start, 5);
        // `c` takes `a`'s row as `a` goes.
        world.despawn(a)?;
        assert_eq!(at(&world, c), (2, 4));

        world.rewind(&snapshot);
        let tick = world.change_tick() - start;
        assert!(tick > 5);
        assert_eq!(
            [a, b, c].map(|entity| at(&world, entity)),
            [(tick, tick); 3]
        );
        Ok(())
    }

    #[test]
    fn entities_keep_their_handles_and_ids_as_others_are_removed() {
        let mut world = World::default();
        let first: Vec<Entity> = (0..1000).map(|i| world.spawn(Index(i))).collect();
        let id = |world: &World, entity: Entity| world.id(entity).expect("alive");
        let ids: Vec<EntityId> = first.iter().map(|&entity| id(&world, entity)).collect();
        let gone = |i: usize| i.is_multiple_of(3);
        let marked = |i: usize| !gone(i) && i.is_multiple_of(5);
        for (i, &entity) in first.iter().enumerate() {
            if gone(i) {
                // The table's last row fills the gap.
                world.despawn(entity).expect("alive");
            } else if marked(i) {
                // Moves its row to another table.
                world.insert(entity, Marked).expect("alive");
            }
        }
        let second: Vec<Entity> = (1000..1400).map(|i| world.spawn(Index(i))).collect();
        let slots: HashSet<u32> = first.iter().map(|entity| entity.index).collect();
        let reused = second.iter().filter(|entity| slots.contains(&entity.index));
        assert_eq!(
            reused.count(),
            334,
            "the new entities take the freed slots first"
        );
        for (i, &entity) in first.iter().enumerate() {
            if gone(i) {
                assert!(!world.contains(entity), "{entity}");
                assert_eq!(world.get::<Index>(entity), None, "{entity}");
                assert_eq!(world.id(entity), None, "{entity}");
                assert_eq!(world.entity(ids[i]), None, "{entity}'s ID");
                let error = world.despawn(entity).expect_err("a dead entity");
                assert_eq!(error.code(), "ecs.no_entity");
                assert!(error.to_string().contains(&entity.to_string()), "{error}");
            } else {
                assert_eq!(world.get::<Index>(entity), Some(&Index(i as u32)));
                assert_eq!(world.get::<Marked>(entity).is_some(), marked(i), "{entity}");
                assert_eq!(world.id(entity), Some(ids[i]), "{entity}");
                assert_eq!(world.entity(ids[i]), Some(entity), "{entity}'s ID");
            }
        }
        for (i, &entity) in (1000..).zip(&second) {
            assert_eq!(world.get::<Index>(entity), Some(&Index(i)));
        }
        assert_eq!(world.query::<Entity>().count(), 1000 - 334 + 400);
        // Each drawn once, the new entities in reused slots among them.
        let seconds = second.iter().map(|&entity| id(&world, entity));
        let drawn: HashSet<EntityId> = ids.iter().copied().chain(seconds).collect();
        assert_eq!(drawn.len(), 1400);
        check_ids(&world);
    }

    #[test]
    fn a_slot_retires_rather_than_wrap() {
        let mut world = World::default();
        let first = world.spawn(Index(0));
        // As though its slot had held 4 billion entities.
        world.meta[first.index as usize].generation = NonZeroU32::MAX;
        let last = Entity {
            index: first.index,
            generation: NonZeroU32::MAX,
        };
        let id = world.id(last).expect("alive");
        world.despawn(last).expect("alive");
        assert_eq!(world.entity(id), None, "a retired slot's ID");
        check_ids(&world);
        let next = world.spawn(Index(1));
        assert_ne!(next.index, last.index, "a retired slot takes no entity");
        for entity in [first, last] {
            assert!(!world.contains(entity), "{entity}");
            assert_eq!(world.get::<Index>(entity), None, "{entity}");
            let error = world.despawn(entity).expect_err("a dead entity");
            assert_eq!(error.code(), "ecs.no_entity");
        }
        assert_eq!(world.get::<Index>(next), Some(&Index(1)));
    }

    /// Clearing frees every slot that held an entity, or that a log kept for a deleted one, under
    /// its next generation, after the free ones (a slot at its highest generation retires). No old
    /// handle names an entity after, and no ID is drawn again. The component types stay. The
    /// world is another and refuses the log's entries.
    #[test]
    fn clearing_the_world_frees_every_slot_and_keeps_the_types() {
        let mut x = fixture();
        let worn = x.world.spawn(Index(0));
        let worn_id = x.world.id(worn).expect("alive");
        // As though its slot had held 4 billion entities.
        x.world.meta[worn.slot()].generation = NonZeroU32::MAX;
        x.world.despawn(x.g).expect("alive");
        let mut log = Log::default();
        let delete = Operation::Delete { entity: x.id(x.a) };
        log.apply(&mut x.world, delete).expect("deleted");
        assert_eq!(vacated(&x.world), 5, "a and its descendants, kept");
        let handles = [x.g, x.r, x.a, x.s, x.b, x.c, x.d, x.e, x.f];
        let mut ids: HashSet<EntityId> = handles.iter().map(|&entity| x.id(entity)).collect();
        ids.insert(worn_id);
        let types: Vec<String> = x.world.component_types().map(str::to_owned).collect();
        let serial = x.world.serial();
        let world = &mut x.world;
        world.clear();
        assert_ne!(world.serial(), serial);
        check_ids(world);
        check_links(world);
        assert_eq!(world.entities(None).expect("every entity").len(), 0);
        for entity in handles.into_iter().chain([worn]) {
            assert!(!world.contains(entity), "{entity}");
        }
        assert!(ids.iter().all(|&id| world.entity(id).is_none()));
        let listed = world.component_types().map(str::to_owned);
        assert!(listed.eq(types), "the component types");
        // g's slot, freed before, then the others in slot order, those the log kept among them;
        // the worn one retired.
        let slots = handles.map(|entity| entity.index);
        assert_eq!(world.free, slots);
        assert_eq!(vacated(world), 0, "the slots the log kept");
        // Its entries are another world's: refused, not put back, and cleared, freeing nothing.
        let error = log.undo(world).expect_err("another world's");
        assert_eq!(error.code(), "ops.other_world", "{error}");
        log.clear(world);
        assert_eq!(world.free, slots, "freed once");
        // The freed slots take new entities, under handles and IDs of their own, then a new slot
        // does, never the retired one.
        let spawned: Vec<Entity> = (1..11).map(|i| world.spawn(Index(i))).collect();
        assert_eq!(
            spawned.last().map(|entity| entity.index),
            Some(worn.index + 1)
        );
        for &entity in &spawned {
            assert!(!handles.contains(&entity), "{entity}");
            let id = world.id(entity).expect("alive");
            assert!(!ids.contains(&id), "{id} drawn again");
        }
        check_ids(world);
        check_links(world);
    }

    /// Renewing frees each slot a log kept for a deleted entity, under its next generation, and
    /// lets its ID go; entities stay as they were. The world is another, its version moved on, and
    /// refuses the log's entries.
    #[cfg(feature = "dev")]
    #[test]
    fn renewing_the_world_frees_what_logs_kept_and_keeps_the_entities() {
        let mut x = fixture();
        let mut log = Log::default();
        let a = x.id(x.a);
        log.apply(&mut x.world, Operation::Delete { entity: a })
            .expect("deleted");
        assert_eq!(vacated(&x.world), 5, "a and its descendants, kept");
        let (hash, (serial, version)) = (x.world.hash(), x.world.version());
        let living = x.world.entities(None).expect("every entity").len();
        x.world.renew();
        let world = &mut x.world;
        assert_eq!((world.hash(), vacated(world)), (hash, 0));
        assert_eq!(world.entities(None).expect("every entity").len(), living);
        let (renewed, moved_on) = world.version();
        assert!(renewed != serial && moved_on > version);
        check_ids(world);
        check_links(world);
        let error = log.undo(world).expect_err("another world's");
        assert_eq!(error.code(), "ops.other_world", "{error}");
        assert_eq!(world.entity(a), None);
        // The freed slots take new entities, under handles of their own.
        let taken = [x.a, x.b, x.c, x.d, x.e];
        let spawned: Vec<Entity> = (0..5).map(|i| world.spawn(Index(i))).collect();
        let mut slots: Vec<u32> = spawned.iter().map(|entity| entity.index).collect();
        slots.sort_unstable();
        assert_eq!(slots, taken.map(|entity| entity.index));
        assert!(spawned.iter().all(|entity| !taken.contains(entity)));
        log.clear(world);
        check_ids(world);
    }

    /// A reset belongs to its slot alone and changes neither the hash nor the version, which
    /// drawing sorts by. Snapshots keep it and the count, and a slot's next entity starts without
    /// its last's.
    #[test]
    fn a_reset_touches_its_slot_alone() {
        let mut world = World::seeded(1);
        let [reset, other] = [0, 1].map(|i| world.spawn(Index(i)));
        let (hash, version) = (world.hash(), world.version());
        let before = world.resets();
        world.reset_interpolation(reset).expect("alive");
        assert!(world.reset_since(reset, before));
        assert!(!world.reset_since(other, before));
        assert!(!world.reset_since(reset, world.resets()), "not since");
        assert_eq!((world.hash(), world.version()), (hash, version));

        let snapshot = world.clone();
        assert!(snapshot.reset_since(reset, before), "in the snapshot");
        assert_eq!(snapshot.resets(), world.resets(), "the snapshot's count");
        let more = world.spawn(Index(2));
        world.reset_interpolation(more).expect("alive");
        // Restored with fewer slots than it had, then given more again.
        world.clone_from(&snapshot);
        assert!(world.reset_since(reset, before), "restored");
        let again = world.spawn(Index(3));
        assert!(!world.reset_since(again, before), "a slot made again");
        world.reset_interpolation(again).expect("alive");
        assert!(world.reset_since(again, snapshot.resets()));

        world.despawn(reset).expect("alive");
        let error = world.reset_interpolation(reset).expect_err("a dead entity");
        assert_eq!(error.code(), "ecs.no_entity");
        let next = world.spawn(Index(4));
        assert_eq!(next.slot(), reset.slot(), "the slot freed");
        assert!(!world.reset_since(next, 0), "its last entity's reset");
    }

    #[derive(Clone, Copy, Debug, PartialEq, Component)]
    struct A(u32);

    #[derive(Clone, Copy, Debug, PartialEq, Component)]
    struct B(u64);

    #[derive(Clone, Copy, Debug, PartialEq, Component)]
    struct C;

    /// Not `Copy`, and on the heap, so a value dropped or cloned where it should move shows.
    #[derive(Clone, Debug, PartialEq, Component)]
    struct D(String);

    /// An entity's components, as the model keeps them.
    #[derive(Clone, Debug, Default, PartialEq)]
    struct Has {
        a: Option<A>,
        b: Option<B>,
        c: bool,
        d: Option<D>,
        parent: Option<Entity>,
        id: Option<EntityId>,
    }

    impl Has {
        fn of(world: &World, entity: Entity) -> Self {
            Self {
                a: world.get(entity).copied(),
                b: world.get(entity).copied(),
                c: world.get::<C>(entity).is_some(),
                d: world.get(entity).cloned(),
                parent: world.get::<Parent>(entity).map(|parent| parent.0),
                id: world.id(entity),
            }
        }
    }

    /// Saves every root and loads the scene into another world (types registered in another
    /// order, slots freed). Checks against the model: each entity by ID with components and parent,
    /// children in ID order, `world`'s content hash, and a save that matches the first.
    fn scene_matches_the_model(world: &World, live: &[(Entity, Has)], step: usize) {
        let roots = live.iter().filter(|(_, has)| has.parent.is_none());
        let roots: Vec<Entity> = roots.map(|&(entity, _)| entity).collect();
        let text = world
            .save_scene(&roots)
            .unwrap_or_else(|error| panic!("step {step}: {error}"));
        let mut other = World::seeded(step as u64);
        let registered = [
            other.register::<D>(),
            other.register::<C>(),
            other.register::<B>(),
            other.register::<A>(),
        ];
        registered
            .into_iter()
            .for_each(|done| done.expect("registered"));
        let spare: Vec<Entity> = (0..step % 7).map(|_| other.spawn(())).collect();
        for entity in spare {
            other.despawn(entity).expect("alive");
        }
        let loaded = other
            .load_scene(&text)
            .unwrap_or_else(|error| panic!("step {step}: {error}"));
        check_ids(&other);
        check_links(&other);
        let content = |world: &World| world.hash().content;
        assert_eq!(content(&other), content(world), "step {step}: the content");
        assert_eq!(
            other.save_scene(&loaded).expect("saved"),
            text,
            "step {step}"
        );
        assert_eq!(other.query::<Entity>().count(), live.len(), "step {step}");
        let copy = |entity: Entity| {
            let id = world.id(entity).expect("alive");
            other.entity(id).expect("loaded")
        };
        let ids = |world: &World, entity| -> Vec<EntityId> {
            let children = world.children(entity).expect("alive");
            children
                .map(|child| world.id(child).expect("alive"))
                .collect()
        };
        for (entity, has) in live {
            let expected = Has {
                parent: has.parent.map(copy),
                ..has.clone()
            };
            assert_eq!(
                Has::of(&other, copy(*entity)),
                expected,
                "{entity} at step {step}"
            );
            let mut kids = ids(world, *entity);
            kids.sort();
            assert_eq!(
                ids(&other, copy(*entity)),
                kids,
                "{entity}'s children at step {step}"
            );
        }
    }

    /// SplitMix64, seeded, so a test takes the same steps every run.
    pub(in crate::ecs) struct Rng(pub(in crate::ecs) u64);

    impl Rng {
        /// A number below `n`.
        pub(in crate::ecs) fn below(&mut self, n: usize) -> usize {
            (splitmix64(&mut self.0) % n as u64) as usize
        }
    }

    /// Takes random steps (spawn, despawn, give, take, change through queries and by name, set
    /// parents, now and then clear) and checks the world against a model after each: each living
    /// entity's components, parent and ID, and when it became its parent's child.
    #[test]
    fn the_world_matches_a_model() {
        // Seeded, so its IDs are the same every run too.
        let mut world = World::seeded(0x5eed);
        let mut rng = Rng(7);
        let mut live: Vec<(Entity, Has)> = Vec::new();
        let mut dead: Vec<(Entity, EntityId)> = Vec::new();
        // Each ID drawn, which none is again.
        let mut seen = HashSet::new();
        let mut spawned = |world: &World, entity: Entity, mut has: Has| {
            let id = world.id(entity).expect("alive");
            assert!(seen.insert(id), "{id} drawn twice");
            has.id = Some(id);
            (entity, has)
        };
        // The step at which each child became its parent's, which orders its siblings.
        let mut joined: HashMap<Entity, usize> = HashMap::new();
        // How many steps cleared the world.
        let mut cleared = 0;
        // Order A, C, B, D puts C's column between A's and B's: a query of B then A takes them
        // out of the tables' order.
        let first = world.spawn((A(0), C, B(0), D("0".to_owned())));
        let all = |n: u32| Has {
            a: Some(A(n)),
            b: Some(B(n.into())),
            c: true,
            d: Some(D(n.to_string())),
            parent: None,
            id: None,
        };
        live.push(spawned(&world, first, all(0)));
        for step in 1..2000 {
            let n = step as u32;
            match rng.below(11) {
                0 | 1 if live.len() < 200 => {
                    let (entity, has) = match rng.below(6) {
                        0 => (world.spawn(()), Has::default()),
                        1 => (
                            world.spawn(A(n)),
                            Has {
                                a: Some(A(n)),
                                ..Has::default()
                            },
                        ),
                        2 => {
                            let has = Has {
                                a: Some(A(n)),
                                b: Some(B(n.into())),
                                ..Has::default()
                            };
                            (world.spawn((B(n.into()), A(n))), has)
                        }
                        3 => {
                            let has = Has {
                                b: Some(B(n.into())),
                                c: true,
                                ..Has::default()
                            };
                            (world.spawn((C, B(n.into()))), has)
                        }
                        4 => {
                            let has = Has {
                                c: true,
                                d: Some(D(n.to_string())),
                                ..Has::default()
                            };
                            (world.spawn((D(n.to_string()), C)), has)
                        }
                        _ => (
                            world.spawn((A(n), C, B(n.into()), D(n.to_string()))),
                            all(n),
                        ),
                    };
                    live.push(spawned(&world, entity, has));
                }
                2 if rng.below(40) == 0 => {
                    // Every entity at once.
                    let serial = world.serial();
                    world.clear();
                    assert_ne!(world.serial(), serial, "step {step}");
                    let gone = live.drain(..).map(|(entity, has)| (entity, has.id));
                    dead.extend(
                        gone.map(|(entity, id)| (entity, id.expect("drawn as it spawned"))),
                    );
                    cleared += 1;
                }
                2 if !live.is_empty() => {
                    // With its descendants.
                    let entity = live[rng.below(live.len())].0;
                    world.despawn(entity).expect("alive");
                    let mut gone = vec![entity];
                    let mut next = 0;
                    while let Some(&parent) = gone.get(next) {
                        let children = live.iter().filter(|(_, has)| has.parent == Some(parent));
                        gone.extend(children.map(|&(child, _)| child));
                        next += 1;
                    }
                    let id = |entity: &Entity| {
                        let found = live.iter().find(|(other, _)| other == entity);
                        found
                            .and_then(|(_, has)| has.id)
                            .expect("drawn as it spawned")
                    };
                    let gone: Vec<(Entity, EntityId)> = gone.iter().map(|e| (*e, id(e))).collect();
                    live.retain(|(entity, _)| gone.iter().all(|(gone, _)| gone != entity));
                    dead.extend(gone);
                }
                3 if !dead.is_empty() => {
                    let entity = dead[rng.below(dead.len())].0;
                    let code = |error: Error| error.code().to_owned();
                    assert_eq!(
                        world.despawn(entity).map_err(code),
                        Err("ecs.no_entity".into())
                    );
                    let inserted = world.insert(entity, A(n)).map_err(code);
                    assert_eq!(inserted, Err("ecs.no_entity".into()));
                    let removed = world.remove::<A>(entity).map_err(code);
                    assert_eq!(removed, Err("ecs.no_entity".into()));
                    assert_eq!(world.get_mut::<A>(entity), None);
                    let orphaned = world.set_parent(entity, None).map_err(code);
                    assert_eq!(orphaned, Err("ecs.no_entity".into()));
                    if let Some(&(child, _)) = live.first() {
                        let adopted = world.set_parent(child, Some(entity)).map_err(code);
                        assert_eq!(adopted, Err("ecs.no_entity".into()));
                    }
                }
                4 if !live.is_empty() => {
                    let picked = rng.below(live.len());
                    let (entity, has) = &mut live[picked];
                    let inserted = match rng.below(4) {
                        0 => world.insert(*entity, *has.a.insert(A(n))),
                        1 => world.insert(*entity, *has.b.insert(B(n.into()))),
                        2 => {
                            has.c = true;
                            world.insert(*entity, C)
                        }
                        _ => world.insert(*entity, has.d.insert(D(n.to_string())).clone()),
                    };
                    inserted.expect("alive");
                }
                5 if !live.is_empty() => {
                    let picked = rng.below(live.len());
                    let (entity, has) = &mut live[picked];
                    let entity = *entity;
                    match rng.below(4) {
                        0 => assert_eq!(world.remove(entity).expect("alive"), has.a.take()),
                        1 => assert_eq!(world.remove(entity).expect("alive"), has.b.take()),
                        2 => {
                            let removed = world.remove::<C>(entity).expect("alive");
                            assert_eq!(removed.is_some(), mem::take(&mut has.c));
                        }
                        _ => assert_eq!(world.remove(entity).expect("alive"), has.d.take()),
                    }
                }
                6 => {
                    // Terms out of the tables' order, with an entity among them, written in a `for`
                    // loop, `for_each`, or one then `for_each`: each takes rows differently.
                    let mut found = Vec::new();
                    let mut write = |(b, entity, a): (&mut B, Entity, &mut A)| {
                        b.0 += 1;
                        a.0 += 1;
                        found.push(entity);
                    };
                    let mut query = world.query_mut::<(&mut B, Entity, &mut A)>();
                    match rng.below(3) {
                        0 => {
                            for item in query {
                                write(item);
                            }
                        }
                        1 => query.for_each(write),
                        _ => {
                            if let Some(item) = query.next() {
                                write(item);
                            }
                            query.for_each(write);
                        }
                    }
                    let mut expected = Vec::new();
                    for (entity, has) in &mut live {
                        if let (Some(a), Some(b)) = (&mut has.a, &mut has.b) {
                            a.0 += 1;
                            b.0 += 1;
                            expected.push(*entity);
                        }
                    }
                    found.sort();
                    expected.sort();
                    assert_eq!(found, expected, "step {step}");
                }
                7 if !live.is_empty() => {
                    let picked = rng.below(live.len());
                    let (entity, has) = &mut live[picked];
                    if let Some(d) = world.get_mut::<D>(*entity) {
                        d.0.push('!');
                    }
                    if let Some(d) = &mut has.d {
                        d.0.push('!');
                    }
                }
                9 if !dead.is_empty() && rng.below(4) == 0 => {
                    let entity = dead[rng.below(dead.len())].0;
                    let set = world.set_field(entity, "A", "0", &Value::UInt(n.into()));
                    let error = set.expect_err("a dead entity");
                    assert_eq!(error.code(), "ecs.no_entity", "{entity} at step {step}");
                    // By its handle, never by the ID of the entity now in its slot.
                    let message = error.to_string();
                    assert!(message.contains(&entity.to_string()), "{message}");
                    let occupant = live.iter().find(|(other, _)| other.index == entity.index);
                    if let Some((_, has)) = occupant {
                        let id = has.id.expect("drawn as it spawned").to_string();
                        assert!(!message.contains(&id), "{message}");
                    }
                }
                9 if !live.is_empty() => {
                    // As tools set fields, on an entity with the component or without it.
                    let picked = rng.below(live.len());
                    let (entity, has) = &mut live[picked];
                    let (component, value, old) = match rng.below(3) {
                        0 => {
                            let old = has.a.as_mut().map(|a| mem::replace(&mut a.0, n).into());
                            ("A", Value::UInt(n.into()), old.map(Value::UInt))
                        }
                        1 => {
                            let old = has.b.as_mut().map(|b| mem::replace(&mut b.0, n.into()));
                            ("B", Value::UInt(n.into()), old.map(Value::UInt))
                        }
                        _ => {
                            let text = n.to_string();
                            let old = has.d.as_mut().map(|d| mem::replace(&mut d.0, text.clone()));
                            ("D", Value::String(text), old.map(Value::String))
                        }
                    };
                    let set = world.set_field(*entity, component, "0", &value);
                    let expected = old.ok_or("reflect.missing_component");
                    assert_eq!(set.map_err(|error| error.code()), expected, "step {step}");
                    let got = world.get_field(*entity, component, "0").ok();
                    assert_eq!(got, expected.is_ok().then_some(value), "step {step}");
                }
                10 if !live.is_empty() => {
                    // A parent, or none; a cycle, as the model predicts it, is refused.
                    let child = live[rng.below(live.len())].0;
                    let parent = match rng.below(4) {
                        0 => None,
                        _ => Some(live[rng.below(live.len())].0),
                    };
                    let parent_of = |entity: Entity| {
                        let found = live.iter().find(|&&(other, _)| other == entity);
                        found.and_then(|(_, has)| has.parent)
                    };
                    let mut up = iter::successors(parent, |&at| parent_of(at));
                    let cycle = up.any(|ancestor| ancestor == child);
                    let old = parent_of(child);
                    let set = world.set_parent(child, parent);
                    if cycle {
                        let code = set.map_err(|error| error.code());
                        assert_eq!(code, Err("ecs.parent_cycle"), "step {step}");
                    } else {
                        assert_eq!(set.expect("no cycle"), old, "step {step}");
                        if parent != old {
                            let at = live.iter_mut().find(|(entity, _)| *entity == child);
                            at.expect("alive").1.parent = parent;
                            joined.insert(child, step);
                        }
                    }
                    let by_name = Value::Entity(child);
                    let set = world.set_field(child, "Parent", "0", &by_name);
                    let code = set.map_err(|error| error.code());
                    assert_eq!(code, Err("reflect.read_only"), "step {step}");
                }
                8 => {
                    // A query of one term, in a `for` loop, through `for_each` or through
                    // `par_for_each`.
                    let query = world.query_mut::<&mut B>();
                    match rng.below(3) {
                        0 => {
                            for b in query {
                                b.0 += 1;
                            }
                        }
                        1 => query.for_each(|b| b.0 += 1),
                        _ => query.par_for_each(|b| b.0 += 1),
                    }
                    for (_, has) in &mut live {
                        if let Some(b) = &mut has.b {
                            b.0 += 1;
                        }
                    }
                }
                _ => {}
            }

            for (entity, has) in &live {
                assert_eq!(Has::of(&world, *entity), *has, "{entity} at step {step}");
                let id = has.id.expect("drawn as it spawned");
                assert_eq!(world.entity(id), Some(*entity), "{id} at step {step}");
            }
            check_links(&world);
            check_ids(&world);
            if step % 100 == 0 {
                scene_matches_the_model(&world, &live, step);
            }
            // Each parent's children in the order they became its.
            let mut children: Vec<(Entity, usize, Entity)> = live
                .iter()
                .filter_map(|&(child, ref has)| Some((has.parent?, joined[&child], child)))
                .collect();
            children.sort();
            for siblings in children.chunk_by(|a, b| a.0 == b.0) {
                let parent = siblings[0].0;
                let listed: Vec<Entity> = world.children(parent).expect("alive").collect();
                let expected: Vec<Entity> = siblings.iter().map(|&(_, _, child)| child).collect();
                assert_eq!(listed, expected, "{parent}'s children at step {step}");
            }
            // Every dead entity now and then, and some of them every step.
            let checked: Vec<(Entity, EntityId)> = if step % 100 == 0 {
                dead.clone()
            } else {
                let recent = dead.iter().rev().take(64).copied();
                let some = (0..16.min(dead.len())).map(|_| dead[rng.below(dead.len())]);
                recent.chain(some).collect()
            };
            for (entity, id) in checked {
                assert!(!world.contains(entity), "{entity} at step {step}");
                assert_eq!(world.entity(id), None, "{id} at step {step}");
                assert_eq!(
                    Has::of(&world, entity),
                    Has::default(),
                    "{entity} at step {step}"
                );
            }
            // All one by one, or the first and then the rest through `for_each`.
            let mut query = world
                .query::<(Entity, &A)>()
                .map(|(entity, a)| (entity, *a));
            let mut found: Vec<(Entity, A)> = if rng.below(2) == 0 {
                query.by_ref().collect()
            } else {
                query.next().into_iter().collect()
            };
            query.for_each(|item| found.push(item));
            let mut expected: Vec<(Entity, A)> = live
                .iter()
                .filter_map(|(entity, has)| Some((*entity, has.a?)))
                .collect();
            found.sort_by_key(|&(entity, _)| entity);
            expected.sort_by_key(|&(entity, _)| entity);
            assert_eq!(found, expected, "step {step}");
            assert_eq!(world.query::<Entity>().count(), live.len(), "step {step}");
            let listed = world.entities(None).expect("every entity");
            assert_eq!(listed.len(), live.len(), "step {step}");
            assert_eq!(
                world.query_mut::<Entity>().count(),
                live.len(),
                "step {step}"
            );
        }
        assert!(cleared > 0, "no step cleared the world");
    }

    #[test]
    #[should_panic(expected = "a query names `gg_engine::ecs::tests::A` twice")]
    fn a_query_of_a_component_twice_is_a_bug() {
        let mut world = World::default();
        world.spawn(A(0));
        let _ = world.query_mut::<(&mut A, &mut A)>();
    }

    #[test]
    #[should_panic(expected = "a query names `gg_engine::ecs::tests::A` twice")]
    fn a_query_of_a_component_twice_is_a_bug_before_any_entity_has_it() {
        let mut world = World::default();
        let _ = world.query_mut::<(Entity, &A, &mut A)>();
    }

    #[test]
    #[should_panic(expected = "a bundle has `A` twice")]
    fn a_bundle_of_a_component_twice_is_a_bug() {
        World::default().spawn((A(0), B(0), A(1)));
    }
}

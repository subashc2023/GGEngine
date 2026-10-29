//! The engine's ECS: entities, which are handles, and their components, data of any reflected type,
//! kept in archetype tables, a table for each set of component types, and found by queries, or by
//! name; entities' persistent IDs; the hierarchy, in which entities' parents carry their children
//! along; scenes, the world's entities as text; operations, the authoring changes made to it, each
//! with its inverse, and their log; snapshots, the world copied whole, which restore it; the
//! world's hash, of what it holds and how, which two runs compare at each tick; what tells two
//! worlds apart, field by field; and resources, what a world holds beside its entities.

mod by_name;
mod diff;
mod hash;
mod hierarchy;
mod id;
mod log;
mod ops;
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
use std::sync::atomic::{AtomicU64, Ordering};

use gg_core::{Error, Result};

pub use by_name::Entities;
pub use diff::Difference;
pub use hash::{Digest, WorldHash};
use hierarchy::Links;
pub use hierarchy::{Parent, Pose};
pub use id::EntityId;
use id::IdStream;
pub use log::Log;
pub use ops::Operation;
pub use query::{Query, QueryIter, QueryIterMut, ReadOnlyQuery};
pub use resources::Resource;
use resources::Resources;
pub(crate) use scene::{MAX_DEPTH, float, quote};
pub use scene::{read_scene, write_scene};
pub(crate) use table::ComponentId;
use table::{AnyColumn, Components, Row, Table};

use crate::reflect::Reflect;

/// Data an entity can have: a struct or an enum that derives it, with `#[derive(Clone,
/// Component)]`, which describes it to reflection too, as [`Reflect`] says, so that tools reach it
/// by name. Clone, so that the world can be copied whole, as a snapshot is; and, through
/// `Reflect`, `Send` and `Sync`, so that it can go to the job system's threads, in the rows
/// [`par_for_each`](QueryIterMut::par_for_each) hands them. A type registers the first time an
/// entity has one, or with [`World::register`]. The derive makes it [`Writable`] too, a component
/// that code changes.
#[diagnostic::on_unimplemented(
    message = "`{Self}` isn't a component",
    note = "a type becomes one with `#[derive(Clone, gg_engine::Component)]`"
)]
pub trait Component: Reflect + Clone {
    /// What a scene's component of the type is built on, which gives it what's opaque, since
    /// scenes leave that out, as [`Sprite`](crate::Sprite)'s white texture: it's made with
    /// [`with_value`](Reflect::with_value). `None`, the derive's, makes it with
    /// [`from_value`](Reflect::from_value) alone, which fails where it has something opaque, so
    /// that a type with an opaque field implements `Component` by hand to load.
    fn base() -> Option<Self> {
        None
    }

    /// The name of its exclusive field, if it has one: a `bool` field of its own, of a struct with
    /// named fields, which [`Operation`]s keep true on one entity at most. One that makes it true
    /// on an entity, as a create, an add or a set of it does, makes it false on every other entity
    /// with the component, and what undoes the operation makes theirs true again, as one edit;
    /// code, and a scene loaded, make it true on as many as they do. [`Camera`](crate::Camera)'s
    /// `primary` is one. `None`, the derive's.
    fn exclusive() -> Option<&'static str> {
        None
    }
}

/// A component that code changes: gives an entity, takes away and changes in place, through
/// [`World::spawn`], [`World::insert`], [`World::remove`], [`World::get_mut`] and `&mut T` in
/// [`World::query_mut`]. `#[derive(Component)]` implements it, and a component implemented by hand
/// implements it beside `Component`. Each of the engine's components is one but [`Parent`], which
/// the world keeps, so that the hierarchy stays whole: [`World::set_parent`] alone changes it.
#[diagnostic::on_unimplemented(
    message = "`{Self}` isn't a component that code changes",
    note = "a type becomes one with `#[derive(Clone, gg_engine::Component)]`, or with \
            `impl gg_engine::Writable` beside its own `impl Component`; the world keeps `Parent`, \
            which `World::set_parent` changes"
)]
pub trait Writable: Component {}

/// Makes a struct or an enum a component, one that code changes, describing it to reflection:
/// [`Component`] says how.
pub use gg_macros::Component;

/// Components that an entity is spawned with: a [`Writable`] component, or a tuple of up to 8 of
/// different types.
#[diagnostic::on_unimplemented(
    message = "`{Self}` isn't a bundle of components",
    note = "`World::spawn` takes a component, a type that derives `gg_engine::Component`, or \
            a tuple of up to 8 of different types; `World::insert` takes one; the world keeps \
            `Parent`, which `World::set_parent` gives"
)]
pub trait Bundle: seal::Bundle {}

/// An entity: a handle to it, which stays its own as other entities come and go, and never names
/// another once it's despawned, unless its world is restored from a snapshot taken before it was
/// spawned. It's printed as its slot and the slot's generation, such as `7v2`.
/// It's the world's own: files, tools and diagnostics name an entity by its [`EntityId`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Entity {
    /// Its slot.
    index: u32,
    /// Which use of the slot it is.
    generation: NonZeroU32,
}

impl Entity {
    /// A handle that names no entity in any world, for one that's dead where it's read: its slot is
    /// the one the hierarchy keeps for none, which no entity takes.
    pub(crate) const DEAD: Self = Self {
        index: u32::MAX,
        generation: NonZeroU32::MIN,
    };

    /// Its slot, which no other living entity has, as an index.
    pub(crate) fn slot(self) -> usize {
        self.index as usize
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

/// Entities and their components. Each set of component types has a table, whose rows are the
/// entities with exactly those types, in the order they joined it, and whose columns are each
/// type's values; despawning an entity, or giving it a component or taking one away, moves the
/// table's last row into its place. So the order queries find entities in follows from the changes
/// made, the same every time, but means nothing a game should depend on: anything ordered, such as
/// what's drawn over what, orders by its components' values, or by its entities' IDs.
///
/// A copy of the world is a snapshot of it, which restores it: [`clone`](Clone::clone) takes one,
/// and [`clone_from`](Clone::clone_from) restores the world from one, or takes one into a copy kept
/// from before. A copy is all of the world: each slot under its generation, the free ones in the
/// order they're taken, and those kept for the entities a [`Log`] can put back still kept, with
/// their IDs; each entity under its handle and its ID, in its table and row, with its parent and
/// children, and when its interpolation was last reset; the tables in the order they were made,
/// those made since a snapshot cut away as it's restored; the stream the IDs come from; the
/// component types; and its [`Resource`]s. So a world restored goes on as the world the snapshot
/// was taken from did: given the same changes, it spawns the same handles with the same IDs, and
/// its queries find the same order, as it gives out again the handles and IDs given out since the
/// snapshot. But it's another world: drawing sorts its rows again, and doesn't interpolate into it
/// from the world before, and it refuses what undoes an operation applied before, as another
/// world's, so that the slots kept when the snapshot was taken stay vacant, with their IDs, until
/// [`clear`](Self::clear) or [`replace_scene`](Self::replace_scene) frees them. Taken again, or
/// restored, a snapshot is copied into the memory the world holds, which allocates nothing while
/// the tables, the component types and the world's size stay as they were, for components and
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
    /// Each slot's entity's persistent ID, beside its `Meta` rather than in it, since every lookup
    /// of a handle reads `Meta`, which stays the smaller for it, and IDs are read far less often:
    /// written as an entity is spawned into the slot and removed from it, and read through
    /// [`World::id`], which checks the handle first. `None` while the slot is free, vacant or
    /// retired.
    entity_ids: Vec<Option<EntityId>>,
    /// Each slot's entity's last [`reset_interpolation`](World::reset_interpolation), as the count
    /// of resets it made; 0 if none. Beside `Meta`, as the IDs are, since only drawing reads it,
    /// once a frame that ticked: neither it nor the count is hashed, saved or recorded.
    resets: Vec<u64>,
    /// How many times entities' interpolation has been reset, which numbers each reset.
    reset_count: u64,
    /// Each living entity by its ID; looked up, never iterated. Hashed with SipHash, keyed at
    /// random, so that IDs from files can't be chosen to collide.
    by_id: HashMap<EntityId, Entity>,
    /// The room `by_id` was given as it last grew, which the entries its removals leave hide from
    /// its `capacity`: [`find_by_id`](Self::find_by_id) keeps it at most half full.
    id_room: usize,
    /// How many vacant slots keep each ID: the IDs of the entities a delete took, which no draw
    /// gives to another entity while an undo can give them back, as it gives back their slots.
    /// Looked up, never iterated, and hashed as `by_id` is.
    kept: HashMap<EntityId, u32>,
    /// Where the next entity's ID comes from.
    stream: IdStream,
    /// Which world this is among the process's, so that the order drawing keeps for one world,
    /// where it draws the world's entities on from, the entities a pass of scripts listed in it,
    /// and what undoes an operation applied to it never pass for another's, as when a game
    /// replaces its world; a world copied takes another, as does a world restored from a copy,
    /// cleared, or whose entities a scene replaced.
    serial: u64,
    /// How many times entities' rows have changed, as entities are spawned, despawned or moved to
    /// other tables: drawing keeps the order it sorted while it and `serial` stay.
    version: u64,
    /// In the order they were made, which is the order queries search them in.
    tables: Vec<Table>,
    /// Each table, by its sorted component types; looked up, never iterated, so that its random
    /// order never shows.
    by_signature: HashMap<Box<[ComponentId]>, u32>,
    components: Components,
    /// The component types of the table an entity moves to.
    scratch: Vec<ComponentId>,
    /// Lists of entities, kept for their memory, for what lists entities as it changes the world,
    /// such as a model spawning or a pass of scripts: as many as were ever taken at once, as a
    /// pass of scripts run within another takes one of its own.
    lists: Vec<Vec<Entity>>,
    /// What it holds beside its entities.
    resources: Resources,
}

impl Default for World {
    /// An empty world, which has registered [`Parent`], so that tools find it and `set_field`
    /// refuses it by name in any world, and no game's type takes its name; and which draws its
    /// entities' IDs from a stream seeded at random, so that each world's are its own.
    fn default() -> Self {
        // std seeds its hashers' keys from the OS's randomness, once a thread, and steps them for
        // each `RandomState`, so that each world gets another seed.
        Self::seeded(RandomState::new().hash_one(()))
    }
}

/// How many worlds the process has made, which numbers each.
static WORLDS: AtomicU64 = AtomicU64::new(0);

/// A serial no other world has had, for a world made, copied, restored, cleared or replaced.
fn next_serial() -> u64 {
    WORLDS.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Meta {
    generation: NonZeroU32,
    /// `None` while the slot is free; vacant, kept for the entity a delete took, which its inverse
    /// puts back; or retired.
    location: Option<Location>,
    /// Its entity's parent and children, which a free slot has none of.
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
    /// An empty world, as [`World::default`] makes one, whose entities' IDs are drawn from `seed`.
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
            tables: Vec::new(),
            by_signature: HashMap::new(),
            components,
            scratch: Vec::new(),
            lists: Vec::new(),
            resources: Resources::default(),
        }
    }

    /// Makes an entity with `components`, and returns it.
    ///
    /// # Panics
    ///
    /// If `components` has a type twice, since an entity has one of each; and if a type of them
    /// fails to [`register`](Self::register).
    pub fn spawn<B: Bundle>(&mut self, components: B) -> Entity {
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
        let id = self.draw_id(entity);
        self.entity_ids[entity.index as usize] = Some(id);
        let into = &mut self.tables[table as usize];
        let row = into.entities.len() as u32;
        components.push(into, ids);
        into.entities.push(entity);
        self.meta[entity.index as usize].location = Some(Location { table, row });
        self.version += 1;
        entity
    }

    /// An ID for `entity`, drawn from the stream, by which the world now finds it. A draw that meets
    /// a living entity's ID, as a stream that has walked into the run of the session that saved a
    /// loaded scene does, or one a delete keeps for its undo, reseeds the stream and draws again.
    fn draw_id(&mut self, entity: Entity) -> EntityId {
        loop {
            let id = self.stream.draw();
            if self.reserved(id) {
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

    /// Spawns an entity with ID `id`, which no living entity has, and the components in `row`, a
    /// one-row column each, sorted by their ids, which a scene built: the entity `expected`, the
    /// handle [`next_slots`](Self::next_slots) predicted for it.
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

    /// A list of entities, empty, in memory kept from one given back through
    /// [`keep_list`](Self::keep_list), for what lists entities as it changes the world, as
    /// [`Model::spawn`](crate::Model::spawn) and [`run_scripts`](crate::run_scripts) do: so that
    /// it allocates nothing once the world has listed as many, in as many lists at once.
    pub(crate) fn take_list(&mut self) -> Vec<Entity> {
        let mut list = self.lists.pop().unwrap_or_default();
        list.clear();
        list
    }

    /// Keeps `list`'s memory for a [`take_list`](Self::take_list) after.
    pub(crate) fn keep_list(&mut self, list: Vec<Entity>) {
        self.lists.push(list);
    }

    /// Puts `entity`, whose slot holds none, with ID `id`, which the world finds it by already,
    /// into the table of the components in `row`, a one-row column each, sorted by their ids.
    fn place(&mut self, entity: Entity, id: EntityId, row: Row) {
        let mut signature = mem::take(&mut self.scratch);
        signature.clear();
        signature.extend(row.iter().map(|&(component, _)| component));
        let table = self.table(&signature);
        self.scratch = signature;
        self.entity_ids[entity.index as usize] = Some(id);
        let into = &mut self.tables[table as usize];
        let row_at = into.entities.len() as u32;
        for ((_, mut from), to) in row.into_iter().zip(into.columns.iter_mut()) {
            from.move_row(0, &mut **to);
        }
        into.entities.push(entity);
        let at = Location { table, row: row_at };
        self.meta[entity.index as usize].location = Some(at);
        self.version += 1;
    }

    /// Gives `entity` `component`, in place of the one of its type it has.
    ///
    /// Fails with `ecs.no_entity` if it isn't alive.
    ///
    /// # Panics
    ///
    /// If `T` fails to [`register`](Self::register).
    pub fn insert<T: Writable>(&mut self, entity: Entity, component: T) -> Result<()> {
        self.put(entity, component)
    }

    /// As [`insert`](Self::insert), for any component, such as the [`Parent`] the world keeps.
    fn put<T: Component>(&mut self, entity: Entity, component: T) -> Result<()> {
        let at = self.locate(entity)?;
        let id = self.components.id::<T>();
        if let Some(column) = self.tables[at.table as usize].column_mut::<T>(id) {
            column[at.row as usize] = component;
            return Ok(());
        }
        let to = self.table_with(at, id);
        self.move_entity(entity, at, to);
        let column = self.tables[to as usize].column_mut::<T>(id);
        column.expect("the table has the component").push(component);
        Ok(())
    }

    /// Takes `entity`'s component of type `T` away, and returns it; `None` if it has none.
    ///
    /// Fails with `ecs.no_entity` if it isn't alive.
    pub fn remove<T: Writable>(&mut self, entity: Entity) -> Result<Option<T>> {
        self.take(entity)
    }

    /// As [`remove`](Self::remove), for any component, such as the [`Parent`] the world keeps.
    fn take<T: Component>(&mut self, entity: Entity) -> Result<Option<T>> {
        let at = self.locate(entity)?;
        let Some(id) = self.components.get(TypeId::of::<T>()) else {
            return Ok(None);
        };
        let Some(column) = self.tables[at.table as usize].column_mut::<T>(id) else {
            return Ok(None);
        };
        let component = column.swap_remove(at.row as usize);
        let to = self.table_without(at, id);
        self.move_entity(entity, at, to);
        Ok(Some(component))
    }

    /// Gives `entity`, at `at`, which has no component `id`, the value in `column`, a one-row
    /// column of it: as [`put`](Self::put) does, for a component whose type isn't known.
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
        column.move_row(0, &mut *into.columns[position]);
    }

    /// Takes `entity`'s component `id` away, at `at`, and returns it in a one-row column of its
    /// own: as [`take`](Self::take) does, for a component whose type isn't known.
    fn take_out(&mut self, entity: Entity, at: Location, id: ComponentId) -> Box<dyn AnyColumn> {
        let mut column = self.components.new_column(id);
        let from = &mut self.tables[at.table as usize];
        let position = from.position(id).expect("the entity has the component");
        from.columns[position].move_row(at.row as usize, &mut *column);
        let to = self.table_without(at, id);
        self.move_entity(entity, at, to);
        column
    }

    /// The table of the component types of the entity at `at`, and `id`, which it hasn't.
    fn table_with(&mut self, at: Location, id: ComponentId) -> u32 {
        let mut signature = mem::take(&mut self.scratch);
        signature.clear();
        signature.extend_from_slice(&self.tables[at.table as usize].components);
        let place = signature.binary_search(&id).unwrap_err();
        signature.insert(place, id);
        let to = self.table(&signature);
        self.scratch = signature;
        to
    }

    /// The table of the component types of the entity at `at`, but `id`.
    fn table_without(&mut self, at: Location, id: ComponentId) -> u32 {
        let mut signature = mem::take(&mut self.scratch);
        signature.clear();
        let from = &self.tables[at.table as usize].components;
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
        self.tables[at.table as usize]
            .column_mut::<T>(id)?
            .get_mut(at.row as usize)
    }

    /// Whether `entity` is alive.
    pub fn contains(&self, entity: Entity) -> bool {
        self.location(entity).is_some()
    }

    /// Has frames draw `entity`'s own transform where this tick leaves it, rather than on its way
    /// there from where the tick found it, until its next tick: for a move that jumps, such as a
    /// teleport or a wrap. Called before or after the move, within the tick; its descendants, which
    /// it carries, jump with it, and its ancestors still carry it on their way. Only drawing reads
    /// it, never the simulation: it changes no hash, scene or recording, nor the order drawing
    /// sorts.
    ///
    /// Fails with `ecs.no_entity` if it isn't alive.
    pub fn reset_interpolation(&mut self, entity: Entity) -> Result<()> {
        self.locate(entity)?;
        self.reset_count += 1;
        self.resets[entity.slot()] = self.reset_count;
        Ok(())
    }

    /// Despawns every entity, and lets go of what logs keep for their undos, the slots and IDs of
    /// the entities deletes took, so that no handle from before names an entity. The component
    /// types and the [`Resource`]s stay, and no ID drawn before is drawn again. The world is then
    /// another, as a restored one is, which refuses what undoes an operation applied before, as
    /// [`serial`](Self::serial) says; so whoever keeps a [`Log`] of it clears the log too.
    pub fn clear(&mut self) {
        let _zone = gg_core::zone!();
        let mut emptied = self.emptied();
        emptied.resources = mem::take(&mut self.resources);
        *self = emptied;
    }

    /// The entities with every component `Q` reads, table by table, as `Q` takes them: such as
    /// `world.query::<(Entity, &Transform)>()`. It only reads; [`query_mut`](Self::query_mut)
    /// changes components too.
    ///
    /// # Panics
    ///
    /// If `Q` names a component twice.
    pub fn query<Q: ReadOnlyQuery>(&self) -> QueryIter<'_, Q> {
        QueryIter::new(&self.tables, &self.components)
    }

    /// As [`query`](Self::query), with `&mut T` terms too, which change the entities' `T`s: such
    /// as `world.query_mut::<(&Velocity, &mut Transform)>()`. `for_each` loops over each table's
    /// columns, the tightest loop.
    ///
    /// # Panics
    ///
    /// If `Q` names a component twice, since two of its terms would reach the same values.
    pub fn query_mut<Q: Query>(&mut self) -> QueryIterMut<'_, Q> {
        QueryIterMut::new(&mut self.tables, &self.components)
    }

    /// The tables, in the order they were made, for the engine's systems.
    pub(crate) fn tables(&self) -> &[Table] {
        &self.tables
    }

    /// `T`'s id, which its tables' columns are found by, if it has registered, as [`Parent`] has in
    /// every world.
    pub(crate) fn component<T: Component>(&self) -> Option<ComponentId> {
        self.components.get(TypeId::of::<T>())
    }

    /// Which world this is among the process's: a copy is another, and so is the world after it's
    /// restored from one, [cleared](Self::clear), or [replaced](Self::replace_scene). Another
    /// refuses the entries a [`Log`] made before, what undoes each operation, as another world's;
    /// so whoever keeps a log of a world, or entities picked in it, lets go of them as the serial
    /// changes.
    pub fn serial(&self) -> u64 {
        self.serial
    }

    /// Which world this is, and how many times its entities' rows have changed, for drawing, which
    /// keeps the order it sorted while both stay, and for a pass of scripts, which ends once the
    /// world isn't the one it listed.
    pub(crate) fn version(&self) -> (u64, u64) {
        (self.serial, self.version)
    }

    /// How many times entities' interpolation has been reset, which drawing keeps before a tick.
    pub(crate) fn resets(&self) -> u64 {
        self.reset_count
    }

    /// Whether `entity`, which is alive, has had its interpolation reset since the world had made
    /// `count` resets.
    pub(crate) fn reset_since(&self, entity: Entity, count: u64) -> bool {
        self.resets[entity.slot()] > count
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

    /// The table of the component types `signature`, sorted, which is made if there's none.
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

    /// The handles the next entities spawned take, in order, as [`take_slot`](Self::take_slot) gives
    /// them: the free slots, the last freed first, and then new ones.
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

    /// Removes the entity in slot `index`, which is alive, without parent or children, and its
    /// components, its table's last row taking its place.
    fn remove_slot(&mut self, index: u32) {
        let at = self.meta[index as usize].location;
        let at = at.expect("the slot's entity is alive");
        let moved = self.tables[at.table as usize].swap_remove(at.row as usize);
        self.leave(index, at, moved);
        self.free_slot(index);
    }

    /// As [`remove_slot`](Self::remove_slot), but returns the entity's components, a one-row column
    /// each, sorted by their ids, and leaves its slot vacant rather than free: its generation as it
    /// was, and taken by no entity until it's freed, so that the entity can come back into it under
    /// its handle, as an undone delete does; and keeps its ID from draws until then.
    fn vacate(&mut self, index: u32) -> Row {
        let at = self.meta[index as usize].location;
        let at = at.expect("the slot's entity is alive");
        let table = &mut self.tables[at.table as usize];
        let (row, moved) = table.take_row(at.row as usize, &self.components);
        let id = self.leave(index, at, moved);
        *self.kept.entry(id).or_insert(0) += 1;
        row
    }

    /// The rest of taking slot `index`'s entity, at `at`, out of its table, whose entity `moved`,
    /// if any, took its row: it goes from the world, and its ID with it, which is returned.
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

    /// Takes `id` out of one vacant slot's keeping: as its entity comes back under it, or as the
    /// slot is freed. Another slot may keep it still, if a create or a load gave it to another
    /// entity meanwhile, which a delete took too.
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

    /// Frees slot `index`, which no entity is in, for the next spawned: under its next generation,
    /// so that the handle of the entity it held never names another.
    fn free_slot(&mut self, index: u32) {
        let meta = &mut self.meta[index as usize];
        // A slot whose generation can't go higher retires, so that no handle names another
        // entity: its `Meta` stays, for every 4 billion entities spawned in the slot.
        if let Some(next) = meta.generation.checked_add(1) {
            meta.generation = next;
            self.free.push(index);
        }
    }

    /// This world without its entities: each slot that held one, or that a log kept for one a
    /// delete took, freed under its next generation, as [`free_slot`](Self::free_slot) frees it,
    /// after the free ones and in slot order, as despawning each in turn would, so that no handle
    /// from before names an entity in it. It keeps the component types, the stream of IDs, so that
    /// none is drawn twice, and the count of resets; and has no tables, as a world made hasn't, so
    /// that it costs the slots, never the components; nor resources, which whoever empties it
    /// moves over, since copying them would allocate.
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
            tables: Vec::new(),
            by_signature: HashMap::new(),
            components: self.components.clone(),
            scratch: Vec::new(),
            lists: Vec::new(),
            resources: Resources::default(),
        };
        // Alive, vacant or retired; a retired slot stays so.
        for (index, free) in (0..).zip(was_free) {
            if !free {
                world.free_slot(index);
            }
        }
        world
    }

    /// Moves `entity`, at `at`, to the end of table `to`, with its components that `to` has. Its
    /// others must have left its row already.
    fn move_entity(&mut self, entity: Entity, at: Location, to: u32) {
        let [from, into] = self
            .tables
            .get_disjoint_mut([at.table as usize, to as usize])
            .expect("an entity moves between two tables");
        let row = at.row as usize;
        for (id, column) in from.components.iter().zip(from.columns.iter_mut()) {
            if let Some(position) = into.position(*id) {
                column.move_row(row, &mut *into.columns[position]);
            }
        }
        from.entities.swap_remove(row);
        if let Some(&moved) = from.entities.get(row) {
            self.meta[moved.index as usize].location = Some(at);
        }
        let row = into.entities.len() as u32;
        into.entities.push(entity);
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
        let column = table.column_mut::<T>(ids[0]);
        column.expect("the table has the component").push(self);
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
                $(
                    let column = table.column_mut::<$T>(ids[$i]);
                    column.expect("the table has the component").push(self.$i);
                )+
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

    /// Clearing the world despawns every entity, and frees each slot that held one, or that a log
    /// kept for one a delete took, under its next generation, after the free ones, a slot that
    /// can't go higher retiring: so no handle from before names an entity after, and no ID drawn
    /// before is drawn again. The component types stay; and the world is another, which refuses
    /// the log's entries rather than put their entities back in slots it has let go.
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
        // The slots freed take new entities, under handles and IDs of their own, and then a new
        // slot does, never the retired one.
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

    /// A reset of an entity's interpolation is its slot's alone, and changes neither the hash nor
    /// the version, which drawing sorts by; a snapshot keeps it and the count, a world restored
    /// keeps a reset for each of its slots, and a slot's next entity starts without its last's.
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

    /// Not `Copy`, and on the heap, so that a value dropped or cloned where it should move shows.
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

    /// Saves `world`'s every root, as the model has them, and loads the scene into a world of its
    /// own, whose types registered in another order and whose slots were freed; and checks it
    /// against the model, each entity by ID, with its components and parent, and its children in
    /// the order of their IDs; that it has `world`'s content hash; and that it saves again as it
    /// was.
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

    /// SplitMix64, seeded, so that a test takes the same steps every run.
    pub(in crate::ecs) struct Rng(pub(in crate::ecs) u64);

    impl Rng {
        /// A number below `n`.
        pub(in crate::ecs) fn below(&mut self, n: usize) -> usize {
            (splitmix64(&mut self.0) % n as u64) as usize
        }
    }

    /// Takes random steps, spawning, despawning, giving and taking components and changing them
    /// through queries and by name, setting parents, and now and then clearing the world, and
    /// after each checks the world against a model of it: a list of each living entity's
    /// components, parent and ID, and when it became its parent's child.
    #[test]
    fn the_world_matches_a_model() {
        // Seeded, so that its IDs are the same every run too.
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
        // Its types in the order A, C, B, D, so that C's column comes between A's and B's, and a
        // query of B then A takes them out of their order in the tables.
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
                    // Writes through terms out of the tables' order, with an entity among them,
                    // in a `for` loop, through `for_each`, or one by one and then the rest through
                    // `for_each`, which take the rows differently.
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

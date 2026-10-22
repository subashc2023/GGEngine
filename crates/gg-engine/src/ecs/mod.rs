//! The engine's ECS: entities, which are handles, and their components, data of any type, kept in
//! archetype tables, a table for each set of component types, and found by queries.

mod query;
mod table;

use std::any::TypeId;
use std::collections::HashMap;
use std::fmt;
use std::mem;
use std::num::NonZeroU32;

use gg_core::{Error, Result};

pub use query::{Query, QueryIter, QueryIterMut, ReadOnlyQuery};
use table::{ComponentId, Components, Table};

/// Data an entity can have: any type that implements it, with `impl Component for Health {}`.
/// Clone, so that the world can be copied whole, as snapshots will; and `Send` and `Sync`, so that
/// it can move to the threads that will run the simulation.
#[diagnostic::on_unimplemented(
    message = "`{Self}` isn't a component",
    note = "a type becomes one with `impl gg_engine::Component for {Self} {{}}`, if it's \
            `Clone`, `Send` and `Sync`"
)]
pub trait Component: Clone + Send + Sync + 'static {}

/// Components that an entity is spawned with: a component, or a tuple of up to 8 of different
/// types.
#[diagnostic::on_unimplemented(
    message = "`{Self}` isn't a bundle of components",
    note = "`World::spawn` takes a component, a type with `impl gg_engine::Component`, or a \
            tuple of up to 8 of different types; `World::insert` takes one"
)]
pub trait Bundle: seal::Bundle {}

/// An entity: a handle to it, which stays its own as other entities come and go, and never names
/// another once it's despawned. It's printed as its slot and the slot's generation, such as `7v2`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Entity {
    /// Its slot.
    index: u32,
    /// Which use of the slot it is.
    generation: NonZeroU32,
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
/// what's drawn over what, orders by its components' values.
#[derive(Default)]
pub struct World {
    /// Each slot's generation, and where its entity is if it's alive.
    meta: Vec<Meta>,
    /// The slots free for new entities, the last freed taken first.
    free: Vec<u32>,
    /// In the order they were made, which is the order queries search them in.
    tables: Vec<Table>,
    /// Each table, by its sorted component types; looked up, never iterated, so that its random
    /// order never shows.
    by_signature: HashMap<Box<[ComponentId]>, u32>,
    components: Components,
    /// The component types of the table an entity moves to.
    scratch: Vec<ComponentId>,
}

struct Meta {
    generation: NonZeroU32,
    /// `None` while the slot is free, or retired.
    location: Option<Location>,
}

#[derive(Clone, Copy)]
struct Location {
    table: u32,
    row: u32,
}

/// The most components a bundle has.
const MOST: usize = 8;

impl World {
    /// Makes an entity with `components`, and returns it.
    ///
    /// # Panics
    ///
    /// If `components` has a type twice, since an entity has one of each.
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
        let into = &mut self.tables[table as usize];
        let row = into.entities.len() as u32;
        components.push(into, ids);
        into.entities.push(entity);
        self.meta[entity.index as usize].location = Some(Location { table, row });
        entity
    }

    /// Removes `entity` and its components.
    ///
    /// Fails with `ecs.no_entity` if it isn't alive.
    pub fn despawn(&mut self, entity: Entity) -> Result<()> {
        let at = self.locate(entity)?;
        let moved = self.tables[at.table as usize].swap_remove(at.row as usize);
        if let Some(moved) = moved {
            self.meta[moved.index as usize].location = Some(at);
        }
        let meta = &mut self.meta[entity.index as usize];
        meta.location = None;
        // A slot whose generation can't go higher retires, so that no handle names another
        // entity: its `Meta` stays, for every 4 billion entities spawned in the slot.
        if let Some(next) = meta.generation.checked_add(1) {
            meta.generation = next;
            self.free.push(entity.index);
        }
        Ok(())
    }

    /// Gives `entity` `component`, in place of the one of its type it has.
    ///
    /// Fails with `ecs.no_entity` if it isn't alive.
    pub fn insert<T: Component>(&mut self, entity: Entity, component: T) -> Result<()> {
        let at = self.locate(entity)?;
        let id = self.components.id::<T>();
        if let Some(column) = self.tables[at.table as usize].column_mut::<T>(id) {
            column[at.row as usize] = component;
            return Ok(());
        }
        let mut signature = mem::take(&mut self.scratch);
        signature.clear();
        signature.extend_from_slice(&self.tables[at.table as usize].components);
        let place = signature.binary_search(&id).unwrap_err();
        signature.insert(place, id);
        let to = self.table(&signature);
        self.scratch = signature;
        self.move_entity(entity, at, to);
        let column = self.tables[to as usize].column_mut::<T>(id);
        column.expect("the table has the component").push(component);
        Ok(())
    }

    /// Takes `entity`'s component of type `T` away, and returns it; `None` if it has none.
    ///
    /// Fails with `ecs.no_entity` if it isn't alive.
    pub fn remove<T: Component>(&mut self, entity: Entity) -> Result<Option<T>> {
        let at = self.locate(entity)?;
        let Some(id) = self.components.get(TypeId::of::<T>()) else {
            return Ok(None);
        };
        let Some(column) = self.tables[at.table as usize].column_mut::<T>(id) else {
            return Ok(None);
        };
        let component = column.swap_remove(at.row as usize);
        let mut signature = mem::take(&mut self.scratch);
        signature.clear();
        let from = &self.tables[at.table as usize].components;
        signature.extend(from.iter().filter(|&&other| other != id));
        let to = self.table(&signature);
        self.scratch = signature;
        self.move_entity(entity, at, to);
        Ok(Some(component))
    }

    /// `entity`'s component of type `T`; `None` if it has none, or isn't alive.
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        let at = self.location(entity)?;
        let id = self.components.get(TypeId::of::<T>())?;
        self.tables[at.table as usize]
            .column::<T>(id)?
            .get(at.row as usize)
    }

    pub fn get_mut<T: Component>(&mut self, entity: Entity) -> Option<&mut T> {
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

    /// `T`'s id, which its tables' columns are found by, if any entity has had one.
    pub(crate) fn component<T: Component>(&self) -> Option<ComponentId> {
        self.components.get(TypeId::of::<T>())
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
            return Entity { index, generation };
        }
        let index = u32::try_from(self.meta.len()).expect("fewer than 2^32 entities");
        let generation = NonZeroU32::MIN;
        self.meta.push(Meta {
            generation,
            location: None,
        });
        Entity { index, generation }
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

impl<T: Component> seal::Bundle for T {
    const COUNT: usize = 1;

    fn ids(components: &mut Components, ids: &mut [ComponentId]) {
        ids[0] = components.id::<T>();
    }

    fn push(self, table: &mut Table, ids: &[ComponentId]) {
        let column = table.column_mut::<T>(ids[0]);
        column.expect("the table has the component").push(self);
    }
}

impl<T: Component> Bundle for T {}

impl seal::Bundle for () {
    const COUNT: usize = 0;

    fn ids(_: &mut Components, _: &mut [ComponentId]) {}

    fn push(self, _: &mut Table, _: &[ComponentId]) {}
}

impl Bundle for () {}

macro_rules! tuple_bundle {
    ($($T:ident $i:tt),+) => {
        impl<$($T: Component),+> seal::Bundle for ($($T,)+) {
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

        impl<$($T: Component),+> Bundle for ($($T,)+) {}
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

    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Index(u32);
    impl Component for Index {}

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Marked;
    impl Component for Marked {}

    #[test]
    fn entities_keep_their_ids_as_others_are_removed() {
        let mut world = World::default();
        let first: Vec<Entity> = (0..1000).map(|i| world.spawn(Index(i))).collect();
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
                let error = world.despawn(entity).expect_err("a dead entity");
                assert_eq!(error.code(), "ecs.no_entity");
                assert!(error.to_string().contains(&entity.to_string()), "{error}");
            } else {
                assert_eq!(world.get::<Index>(entity), Some(&Index(i as u32)));
                assert_eq!(world.get::<Marked>(entity).is_some(), marked(i), "{entity}");
            }
        }
        for (i, &entity) in (1000..).zip(&second) {
            assert_eq!(world.get::<Index>(entity), Some(&Index(i)));
        }
        assert_eq!(world.query::<Entity>().count(), 1000 - 334 + 400);
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
        world.despawn(last).expect("alive");
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

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct A(u32);
    impl Component for A {}

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct B(u64);
    impl Component for B {}

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct C;
    impl Component for C {}

    /// Not `Copy`, and on the heap, so that a value dropped or cloned where it should move shows.
    #[derive(Clone, Debug, PartialEq)]
    struct D(String);
    impl Component for D {}

    /// An entity's components, as the model keeps them.
    #[derive(Clone, Debug, Default, PartialEq)]
    struct Has {
        a: Option<A>,
        b: Option<B>,
        c: bool,
        d: Option<D>,
    }

    impl Has {
        fn of(world: &World, entity: Entity) -> Self {
            Self {
                a: world.get(entity).copied(),
                b: world.get(entity).copied(),
                c: world.get::<C>(entity).is_some(),
                d: world.get(entity).cloned(),
            }
        }
    }

    /// Sebastiano Vigna's SplitMix64, seeded, so that the test takes the same steps every run.
    struct Rng(u64);

    impl Rng {
        /// A number below `n`.
        fn below(&mut self, n: usize) -> usize {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            ((z ^ (z >> 31)) % n as u64) as usize
        }
    }

    /// Takes random steps, spawning, despawning, giving and taking components and changing them
    /// through queries, and after each checks the world against a model of it: a list of each
    /// living entity's components.
    #[test]
    fn the_world_matches_a_model() {
        let mut world = World::default();
        let mut rng = Rng(7);
        let mut live: Vec<(Entity, Has)> = Vec::new();
        let mut dead: Vec<Entity> = Vec::new();
        // Its types in the order A, C, B, D, so that C's column comes between A's and B's, and a
        // query of B then A takes them out of their order in the tables.
        let first = world.spawn((A(0), C, B(0), D("0".to_owned())));
        let all = |n: u32| Has {
            a: Some(A(n)),
            b: Some(B(n.into())),
            c: true,
            d: Some(D(n.to_string())),
        };
        live.push((first, all(0)));
        for step in 1..2000 {
            let n = step as u32;
            match rng.below(9) {
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
                    live.push((entity, has));
                }
                2 if !live.is_empty() => {
                    let (entity, _) = live.swap_remove(rng.below(live.len()));
                    world.despawn(entity).expect("alive");
                    dead.push(entity);
                }
                3 if !dead.is_empty() => {
                    let entity = dead[rng.below(dead.len())];
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
                8 => {
                    // A query of one term.
                    let query = world.query_mut::<&mut B>();
                    if rng.below(2) == 0 {
                        for b in query {
                            b.0 += 1;
                        }
                    } else {
                        query.for_each(|b| b.0 += 1);
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
            }
            // Every dead entity now and then, and some of them every step.
            let checked: Vec<Entity> = if step % 100 == 0 {
                dead.clone()
            } else {
                let recent = dead.iter().rev().take(64).copied();
                let some = (0..16.min(dead.len())).map(|_| dead[rng.below(dead.len())]);
                recent.chain(some).collect()
            };
            for entity in checked {
                assert!(!world.contains(entity), "{entity} at step {step}");
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
            assert_eq!(
                world.query_mut::<Entity>().count(),
                live.len(),
                "step {step}"
            );
        }
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
    #[should_panic(expected = "a bundle has `gg_engine::ecs::tests::A` twice")]
    fn a_bundle_of_a_component_twice_is_a_bug() {
        World::default().spawn((A(0), B(0), A(1)));
    }
}

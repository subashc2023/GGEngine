//! Queries: each entity's components of some types, from every table that has them all.

use std::any::{TypeId, type_name};
use std::iter::{Copied, Zip};
use std::slice;

use super::table::{AnyColumn, ComponentId, Components, Table};
use super::{Component, Entity, Writable};
use crate::jobs;

/// Rows per job: a fixed number, never a share of the threads, so rows split the same way on
/// every machine. 1,024 of the sandbox's drifters take about 17 µs in a release build.
const CHUNK: usize = 1024;

/// What a query takes of each entity: `&T`, `&mut T`, `Entity`, or a tuple of up to 8 of them.
///
/// `&T` reads its `T`, `&mut T` changes it ([`Writable`] only), `Entity` is the entity; a component
/// is named once. [`World::query_mut`](super::World::query_mut) takes any;
/// [`World::query`](super::World::query) only readers.
#[diagnostic::on_unimplemented(
    message = "`{Self}` isn't a query",
    note = "a query's terms are `&T` and `&mut T` for components `T`, and `Entity`, alone or in a \
            tuple of up to 8, such as `(Entity, &Transform, &mut Sprite)`; `&mut` doesn't take \
            `Parent`, which the world keeps"
)]
pub trait Query: seal::Query {}

/// A [`Query`] of `&T` and `Entity` terms only, for [`World::query`](super::World::query).
#[diagnostic::on_unimplemented(
    message = "`{Self}` isn't a query that only reads",
    note = "`World::query` takes `&T` and `Entity` terms, alone or in a tuple of up to 8; \
            `&mut T` needs `World::query_mut`"
)]
pub trait ReadOnlyQuery: Query + seal::ReadOnlyQuery {}

/// The entities a [`World::query`](super::World::query) finds, table by table.
pub struct QueryIter<'w, Q: ReadOnlyQuery> {
    tables: slice::Iter<'w, Table>,
    /// `None` if a component it needs has never been in the world: no table matches.
    needs: Option<Q::Needs>,
    /// The rows left of the table it's in.
    rows: Option<Q::Fetch<'w>>,
}

/// The entities a [`World::query_mut`](super::World::query_mut) finds, table by table.
pub struct QueryIterMut<'w, Q: Query> {
    tables: slice::IterMut<'w, Table>,
    needs: Option<Q::Needs>,
    rows: Option<Q::Fetch<'w>>,
    /// The tick each reached table marks its `&mut` terms' columns changed with, all rows.
    tick: u64,
}

impl<'w, Q: ReadOnlyQuery> QueryIter<'w, Q> {
    pub(super) fn new(tables: &'w [Table], components: &Components) -> Self {
        Q::check();
        Self {
            tables: tables.iter(),
            needs: Q::needs(components),
            rows: None,
        }
    }
}

impl<'w, Q: Query> QueryIterMut<'w, Q> {
    pub(super) fn new(tables: &'w mut [Table], components: &Components, tick: u64) -> Self {
        Q::check();
        Self {
            tables: tables.iter_mut(),
            needs: Q::needs(components),
            rows: None,
            tick,
        }
    }
}

impl<'w, Q: ReadOnlyQuery> Iterator for QueryIter<'w, Q> {
    type Item = Q::Item<'w>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(item) = self.rows.as_mut().and_then(Iterator::next) {
                return Some(item);
            }
            let needs = self.needs?;
            let table = self.tables.find(|table| Q::matches(needs, table))?;
            self.rows = Some(Q::fetch(needs, table));
        }
    }

    /// A table at a time, so `for_each` and the like loop over each table's columns.
    fn fold<B, F: FnMut(B, Self::Item) -> B>(self, init: B, mut f: F) -> B {
        let mut acc = match self.rows {
            Some(rows) => rows.fold(init, &mut f),
            None => init,
        };
        let Some(needs) = self.needs else {
            return acc;
        };
        for table in self.tables {
            if Q::matches(needs, table) {
                acc = Q::fetch(needs, table).fold(acc, &mut f);
            }
        }
        acc
    }
}

impl<'w, Q: Query> Iterator for QueryIterMut<'w, Q> {
    type Item = Q::Item<'w>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(item) = self.rows.as_mut().and_then(Iterator::next) {
                return Some(item);
            }
            let needs = self.needs?;
            let table = self.tables.find(|table| Q::matches(needs, table))?;
            self.rows = Some(Q::fetch_mut(needs, table, self.tick));
        }
    }

    /// A table at a time, so `for_each` and the like loop over each table's columns.
    fn fold<B, F: FnMut(B, Self::Item) -> B>(self, init: B, mut f: F) -> B {
        let mut acc = match self.rows {
            Some(rows) => rows.fold(init, &mut f),
            None => init,
        };
        let Some(needs) = self.needs else {
            return acc;
        };
        for table in self.tables {
            if Q::matches(needs, table) {
                acc = Q::fetch_mut(needs, table, self.tick).fold(acc, &mut f);
            }
        }
        acc
    }
}

impl<'w, Q: Query> QueryIterMut<'w, Q> {
    /// Calls `f` on each entity found, like `for_each`, with each table's rows in runs of 1,024
    /// that the job system's threads take, the calling thread included. Outside the pool it runs
    /// on the calling thread, in `for_each` order.
    ///
    /// Rows [`next`](Iterator::next) left of a table come first, on the calling thread, like a
    /// table of one run or less. Each table finishes before the next starts.
    ///
    /// `f` gets its row's terms and no world, so a row's result can't depend on thread count or
    /// assignment, as determinism needs. Writes through captures, such as a `Mutex` or an atomic,
    /// follow the threads' order.
    ///
    /// In the pool, a thread waiting on another's rows runs other rows meanwhile. So `f` mustn't
    /// hold a lock `f` takes, or a `RefCell` it borrows, while it joins (`par_for_each` or
    /// anything else): the thread could run another row's `f`, which would wait on the held lock
    /// or find the `RefCell` borrowed.
    ///
    /// If `f` panics, this panics with its payload, and in the pool other runs may have changed
    /// by then.
    pub fn par_for_each(self, f: impl Fn(Q::Item<'w>) + Sync) {
        if let Some(rows) = self.rows {
            rows.for_each(&f);
        }
        let Some(needs) = self.needs else {
            return;
        };
        for table in self.tables {
            if Q::matches(needs, table) {
                let len = table.entities().len();
                split::<Q, _>(Q::slices_mut(needs, table, self.tick), len, &f);
            }
        }
    }
}

/// Calls `f` on each of the `len` rows of `slices`, splitting them in two at a multiple of `CHUNK`
/// for two jobs, until each run is a chunk or less.
fn split<'w, Q: Query, F: Fn(Q::Item<'w>) + Sync>(slices: Q::Slices<'w>, len: usize, f: &F) {
    if len <= CHUNK {
        // A shorter run can take less than the 10 µs or so a zone is for.
        let _zone = (len == CHUNK).then(|| gg_core::zone!("job"));
        Q::rows(slices).for_each(f);
        return;
    }
    let mid = len.div_ceil(CHUNK) / 2 * CHUNK;
    let (a, b) = Q::split_at(slices, mid);
    jobs::join(
        || split::<Q, F>(a, mid, f),
        || split::<Q, F>(b, len - mid, f),
    );
}

/// How each query fetches; only this module implements these.
mod seal {
    use super::*;

    pub trait Query {
        /// What it gives for each entity.
        type Item<'w>;
        /// A matching table's rows: its terms' slice iterators, zipped.
        type Fetch<'w>: Iterator<Item = Self::Item<'w>>;
        /// A matching table's rows as its terms' slices, which split for jobs.
        type Slices<'w>: Send;
        /// What it needs of a table: the ids of its terms' components.
        type Needs: Copy;

        /// Panics if it names a component twice.
        fn check();

        /// `None` if a component hasn't been seen in `components`: no table matches.
        fn needs(components: &Components) -> Option<Self::Needs>;

        fn matches(needs: Self::Needs, table: &Table) -> bool;

        /// The matching `table`'s rows as slices; `&mut` terms' columns are marked changed at
        /// `tick`, all rows.
        fn slices_mut<'w>(needs: Self::Needs, table: &'w mut Table, tick: u64) -> Self::Slices<'w>;

        /// The first `mid` rows of `slices`, and the rest.
        fn split_at<'w>(
            slices: Self::Slices<'w>,
            mid: usize,
        ) -> (Self::Slices<'w>, Self::Slices<'w>);

        fn rows<'w>(slices: Self::Slices<'w>) -> Self::Fetch<'w>;

        /// The matching `table`'s rows, marked as [`slices_mut`](Self::slices_mut) does.
        fn fetch_mut<'w>(needs: Self::Needs, table: &'w mut Table, tick: u64) -> Self::Fetch<'w> {
            Self::rows(Self::slices_mut(needs, table, tick))
        }
    }

    pub trait ReadOnlyQuery: Query {
        fn fetch<'w>(needs: Self::Needs, table: &'w Table) -> Self::Fetch<'w>;
    }

    /// A query's term: `&T`, `&mut T` or `Entity`.
    #[diagnostic::on_unimplemented(
        message = "`{Self}` isn't a query's term",
        note = "a query's terms are `&T` and `&mut T` for components `T`, and `Entity`; `&mut` \
                doesn't take `Parent`, which the world keeps"
    )]
    pub trait Term {
        type Item<'w>;
        type Fetch<'w>: Iterator<Item = Self::Item<'w>>;
        /// Its rows of a table: `&[T]`, `&mut [T]` or the table's entities.
        type Slice<'w>: Send;
        /// What it needs of a table: its component's id, or nothing for `Entity`.
        type Need: Copy;
        /// Whether it changes the values it takes, as `&mut T` does.
        const CHANGES: bool;

        /// Its component's type and name; `None` for `Entity`.
        fn component() -> Option<(TypeId, &'static str)>;

        fn need(components: &Components) -> Option<Self::Need>;

        fn has(need: Self::Need, table: &Table) -> bool;

        /// Whether the column of component `id` is the one it takes.
        fn takes(need: Self::Need, id: ComponentId) -> bool;

        /// Its rows of a table, from the column it takes, which the table has, or the entities.
        fn slice_mut<'w>(
            column: Option<&'w mut Box<dyn AnyColumn>>,
            entities: &'w [Entity],
        ) -> Self::Slice<'w>;

        fn split_at<'w>(slice: Self::Slice<'w>, mid: usize) -> (Self::Slice<'w>, Self::Slice<'w>);

        fn rows<'w>(slice: Self::Slice<'w>) -> Self::Fetch<'w>;
    }

    #[diagnostic::on_unimplemented(
        message = "`{Self}` changes components, which `World::query` doesn't",
        note = "`&mut T` needs `World::query_mut`"
    )]
    pub trait ReadTerm: Term {
        fn fetch<'w>(need: Self::Need, table: &'w Table) -> Self::Fetch<'w>;
    }
}

use seal::{ReadTerm, Term};

impl<T: Component> Term for &T {
    type Item<'w> = &'w T;
    type Fetch<'w> = slice::Iter<'w, T>;
    type Slice<'w> = &'w [T];
    type Need = ComponentId;
    const CHANGES: bool = false;

    fn component() -> Option<(TypeId, &'static str)> {
        Some((TypeId::of::<T>(), type_name::<T>()))
    }

    fn need(components: &Components) -> Option<ComponentId> {
        components.get(TypeId::of::<T>())
    }

    fn has(need: ComponentId, table: &Table) -> bool {
        table.has(need)
    }

    fn takes(need: ComponentId, id: ComponentId) -> bool {
        need == id
    }

    fn slice_mut<'w>(column: Option<&'w mut Box<dyn AnyColumn>>, _: &'w [Entity]) -> &'w [T] {
        let column: &'w dyn AnyColumn = &**column.expect("a matching table has the column");
        column.vec()
    }

    fn split_at<'w>(slice: Self::Slice<'w>, mid: usize) -> (Self::Slice<'w>, Self::Slice<'w>) {
        slice.split_at(mid)
    }

    fn rows<'w>(slice: Self::Slice<'w>) -> Self::Fetch<'w> {
        slice.iter()
    }
}

impl<T: Component> ReadTerm for &T {
    fn fetch<'w>(need: ComponentId, table: &'w Table) -> slice::Iter<'w, T> {
        let column = table.column(need);
        column.expect("a matching table has the column").iter()
    }
}

impl<T: Writable> Term for &mut T {
    type Item<'w> = &'w mut T;
    type Fetch<'w> = slice::IterMut<'w, T>;
    type Slice<'w> = &'w mut [T];
    type Need = ComponentId;
    const CHANGES: bool = true;

    fn component() -> Option<(TypeId, &'static str)> {
        Some((TypeId::of::<T>(), type_name::<T>()))
    }

    fn need(components: &Components) -> Option<ComponentId> {
        components.get(TypeId::of::<T>())
    }

    fn has(need: ComponentId, table: &Table) -> bool {
        table.has(need)
    }

    fn takes(need: ComponentId, id: ComponentId) -> bool {
        need == id
    }

    fn slice_mut<'w>(column: Option<&'w mut Box<dyn AnyColumn>>, _: &'w [Entity]) -> &'w mut [T] {
        let column = column.expect("a matching table has the column");
        column.vec_mut()
    }

    fn split_at<'w>(slice: Self::Slice<'w>, mid: usize) -> (Self::Slice<'w>, Self::Slice<'w>) {
        slice.split_at_mut(mid)
    }

    fn rows<'w>(slice: Self::Slice<'w>) -> Self::Fetch<'w> {
        slice.iter_mut()
    }
}

impl Term for Entity {
    type Item<'w> = Entity;
    type Fetch<'w> = Copied<slice::Iter<'w, Entity>>;
    type Slice<'w> = &'w [Entity];
    type Need = ();
    const CHANGES: bool = false;

    fn component() -> Option<(TypeId, &'static str)> {
        None
    }

    fn need(_: &Components) -> Option<()> {
        Some(())
    }

    fn has((): (), _: &Table) -> bool {
        true
    }

    fn takes((): (), _: ComponentId) -> bool {
        false
    }

    fn slice_mut<'w>(
        _: Option<&'w mut Box<dyn AnyColumn>>,
        entities: &'w [Entity],
    ) -> &'w [Entity] {
        entities
    }

    fn split_at<'w>(slice: Self::Slice<'w>, mid: usize) -> (Self::Slice<'w>, Self::Slice<'w>) {
        slice.split_at(mid)
    }

    fn rows<'w>(slice: Self::Slice<'w>) -> Self::Fetch<'w> {
        slice.iter().copied()
    }
}

impl ReadTerm for Entity {
    fn fetch<'w>((): (), table: &'w Table) -> Copied<slice::Iter<'w, Entity>> {
        table.entities().iter().copied()
    }
}

/// A term alone is a query.
macro_rules! term_query {
    ($($term:ty => [$($generics:tt)*]),*) => {$(
        impl<$($generics)*> seal::Query for $term {
            type Item<'w> = <$term as Term>::Item<'w>;
            type Fetch<'w> = <$term as Term>::Fetch<'w>;
            type Slices<'w> = <$term as Term>::Slice<'w>;
            type Needs = <$term as Term>::Need;

            fn check() {}

            fn needs(components: &Components) -> Option<Self::Needs> {
                <$term as Term>::need(components)
            }

            fn matches(needs: Self::Needs, table: &Table) -> bool {
                <$term as Term>::has(needs, table)
            }

            fn slices_mut<'w>(
                needs: Self::Needs,
                table: &'w mut Table,
                tick: u64,
            ) -> Self::Slices<'w> {
                let changes = |id| <$term as Term>::CHANGES && <$term as Term>::takes(needs, id);
                let (components, columns, entities) = table.reach(tick, changes);
                let column = components
                    .iter()
                    .zip(columns.iter_mut())
                    .find(|&(&id, _)| <$term as Term>::takes(needs, id))
                    .map(|(_, column)| column);
                <$term as Term>::slice_mut(column, entities)
            }

            fn split_at<'w>(
                slices: Self::Slices<'w>,
                mid: usize,
            ) -> (Self::Slices<'w>, Self::Slices<'w>) {
                <$term as Term>::split_at(slices, mid)
            }

            fn rows<'w>(slices: Self::Slices<'w>) -> Self::Fetch<'w> {
                <$term as Term>::rows(slices)
            }
        }

        impl<$($generics)*> Query for $term {}
    )*};
}

term_query!(&'a T => ['a, T: Component], &'a mut T => ['a, T: Writable], Entity => []);

impl<'a, T: Component> seal::ReadOnlyQuery for &'a T {
    fn fetch<'w>(needs: ComponentId, table: &'w Table) -> slice::Iter<'w, T> {
        <&'a T as ReadTerm>::fetch(needs, table)
    }
}

impl<T: Component> ReadOnlyQuery for &T {}

impl seal::ReadOnlyQuery for Entity {
    fn fetch<'w>((): (), table: &'w Table) -> Copied<slice::Iter<'w, Entity>> {
        <Entity as ReadTerm>::fetch((), table)
    }
}

impl ReadOnlyQuery for Entity {}

/// Panics if two of `components`, a query's, are the same type.
fn check_twice(components: &[Option<(TypeId, &'static str)>]) {
    for (i, component) in components.iter().enumerate() {
        let Some((id, name)) = component else {
            continue;
        };
        if components[i + 1..]
            .iter()
            .flatten()
            .any(|(other, _)| other == id)
        {
            panic!(
                "a query names `{name}` twice; it takes each component once, so that no two \
                 terms reach the same values"
            );
        }
    }
}

/// `((A, B), C)` from `A, B, C`: the items of iterators zipped one after another.
macro_rules! nested {
    ($a:ident) => { $a };
    ($a:ident, $b:ident $(, $rest:ident)*) => { nested!(@ ($a, $b) $(, $rest)*) };
    (@ $nest:ty) => { $nest };
    (@ $nest:ty, $c:ident $(, $rest:ident)*) => { nested!(@ ($nest, $c) $(, $rest)*) };
}

/// The pattern of [`nested!`]'s tuples, binding `a, b, c`.
macro_rules! nested_pattern {
    ($a:ident) => { $a };
    ($a:ident, $b:ident $(, $rest:ident)*) => { nested_pattern!(@ ($a, $b) $(, $rest)*) };
    (@ $nest:pat) => { $nest };
    (@ $nest:pat, $c:ident $(, $rest:ident)*) => { nested_pattern!(@ ($nest, $c) $(, $rest)*) };
}

/// `Zip<Zip<A::Fetch<'w>, B::Fetch<'w>>, C::Fetch<'w>>` from `'w; A, B, C`.
macro_rules! zipped {
    ($w:lifetime; $a:ident) => { $a::Fetch<$w> };
    ($w:lifetime; $a:ident, $b:ident $(, $rest:ident)*) => {
        zipped!(@ $w; Zip<$a::Fetch<$w>, $b::Fetch<$w>> $(, $rest)*)
    };
    (@ $w:lifetime; $zip:ty) => { $zip };
    (@ $w:lifetime; $zip:ty, $c:ident $(, $rest:ident)*) => {
        zipped!(@ $w; Zip<$zip, $c::Fetch<$w>> $(, $rest)*)
    };
}

/// A tuple of terms is a query. `$Rows` is a table's rows: its terms' slice iterators zipped,
/// items flattened from the nested tuples. Zipped slice iterators take std's indexed path, so
/// each row costs one count check, in a `for` loop or a `for_each` fold.
macro_rules! tuple_query {
    ($Rows:ident; $($T:ident $t:ident $i:tt),+) => {
        /// A table's rows, for a query of a tuple of this many terms.
        pub struct $Rows<I>(I);

        impl<I, $($T),+> Iterator for $Rows<I>
        where
            I: Iterator<Item = nested!($($T),+)>,
        {
            type Item = ($($T,)+);

            #[inline]
            fn next(&mut self) -> Option<Self::Item> {
                let nested_pattern!($($t),+) = self.0.next()?;
                Some(($($t,)+))
            }

            #[inline]
            fn size_hint(&self) -> (usize, Option<usize>) {
                self.0.size_hint()
            }

            #[inline]
            fn fold<Acc, Fold>(self, init: Acc, mut f: Fold) -> Acc
            where
                Fold: FnMut(Acc, Self::Item) -> Acc,
            {
                self.0.fold(init, |acc, nested_pattern!($($t),+)| f(acc, ($($t,)+)))
            }
        }

        impl<$($T: Term),+> seal::Query for ($($T,)+) {
            type Item<'w> = ($($T::Item<'w>,)+);
            type Fetch<'w> = $Rows<zipped!('w; $($T),+)>;
            type Slices<'w> = ($($T::Slice<'w>,)+);
            type Needs = ($($T::Need,)+);

            fn check() {
                check_twice(&[$($T::component()),+]);
            }

            fn needs(components: &Components) -> Option<Self::Needs> {
                Some(($($T::need(components)?,)+))
            }

            fn matches(needs: Self::Needs, table: &Table) -> bool {
                $($T::has(needs.$i, table))&&+
            }

            fn slices_mut<'w>(
                needs: Self::Needs,
                table: &'w mut Table,
                tick: u64,
            ) -> Self::Slices<'w> {
                let changes = |id| $(($T::CHANGES && $T::takes(needs.$i, id)))||+;
                let (components, columns, entities) = table.reach(tick, changes);
                // Each column goes to the one term that takes it, so no two terms reach the same
                // values: the borrow checker sees each reference moved once.
                $(let mut $t = None;)+
                for (&id, column) in components.iter().zip(columns.iter_mut()) {
                    $(
                        if $T::takes(needs.$i, id) {
                            $t = Some(column);
                            continue;
                        }
                    )+
                }
                ($($T::slice_mut($t, entities),)+)
            }

            fn split_at<'w>(
                slices: Self::Slices<'w>,
                mid: usize,
            ) -> (Self::Slices<'w>, Self::Slices<'w>) {
                let ($($t,)+) = slices;
                $(let $t = $T::split_at($t, mid);)+
                (($($t.0,)+), ($($t.1,)+))
            }

            fn rows<'w>(slices: Self::Slices<'w>) -> Self::Fetch<'w> {
                let ($($t,)+) = slices;
                $Rows(zip_fetches!($($T::rows($t)),+))
            }
        }

        impl<$($T: Term),+> Query for ($($T,)+) {}

        impl<$($T: ReadTerm),+> seal::ReadOnlyQuery for ($($T,)+) {
            fn fetch<'w>(needs: Self::Needs, table: &'w Table) -> Self::Fetch<'w> {
                $Rows(zip_fetches!($($T::fetch(needs.$i, table)),+))
            }
        }

        impl<$($T: ReadTerm),+> ReadOnlyQuery for ($($T,)+) {}
    };
}

/// `a.zip(b).zip(c)` from `a, b, c`.
macro_rules! zip_fetches {
    ($first:expr $(, $rest:expr)*) => { $first $(.zip($rest))* };
}

tuple_query!(Rows1; A a 0);
tuple_query!(Rows2; A a 0, B b 1);
tuple_query!(Rows3; A a 0, B b 1, C c 2);
tuple_query!(Rows4; A a 0, B b 1, C c 2, D d 3);
tuple_query!(Rows5; A a 0, B b 1, C c 2, D d 3, E e 4);
tuple_query!(Rows6; A a 0, B b 1, C c 2, D d 3, E e 4, F f 5);
tuple_query!(Rows7; A a 0, B b 1, C c 2, D d 3, E e 4, F f 5, G g 6);
tuple_query!(Rows8; A a 0, B b 1, C c 2, D d 3, E e 4, F f 5, G g 6, H h 7);

#[cfg(test)]
mod tests {
    use std::panic::{self, AssertUnwindSafe};
    use std::sync::Mutex;

    use super::*;
    use crate::World;

    #[derive(Clone, Copy, Debug, PartialEq, Component)]
    struct Visits(u32);

    /// Where a row's entity is among them all.
    #[derive(Clone, Copy, Debug, PartialEq, Component)]
    struct Order(u32);

    #[derive(Clone, Copy, Debug, PartialEq, Component)]
    struct M1;

    #[derive(Clone, Copy, Debug, PartialEq, Component)]
    struct M2;

    #[derive(Clone, Copy, Debug, PartialEq, Component)]
    struct M3;

    /// Six tables of 0, 1, a chunk less 1, a chunk, a chunk and 1, and 5 chunks and 3 rows;
    /// entities are in `Order` order.
    fn world() -> (World, Vec<Entity>) {
        let mut world = World::default();
        let mut entities = Vec::new();
        let sizes = [0, 1, CHUNK - 1, CHUNK, CHUNK + 1, 5 * CHUNK + 3];
        // Leaves its table, empty.
        let gone = world.spawn((Visits(0), Order(u32::MAX)));
        world.despawn(gone).expect("alive");
        for (table, size) in sizes.into_iter().enumerate() {
            for _ in 0..size {
                let row = (Visits(0), Order(entities.len() as u32));
                let entity = match table {
                    0 => world.spawn(row),
                    1 => world.spawn((row.0, row.1, M1)),
                    2 => world.spawn((row.0, row.1, M2)),
                    3 => world.spawn((row.0, row.1, M3)),
                    4 => world.spawn((row.0, row.1, M1, M2)),
                    _ => world.spawn((row.0, row.1, M1, M3)),
                };
                entities.push(entity);
            }
        }
        (world, entities)
    }

    /// `par_for_each` gives each row to `f` once, whatever its table's size, after the rows `next`
    /// took, on one thread and on four; on one, in the order `for_each` takes them.
    #[test]
    fn par_for_each_gives_each_row_once() {
        for threads in [1, 4] {
            for taken in [0, 1, 7] {
                let (mut world, entities) = world();
                let in_order: Vec<_> = world
                    .query::<&Order>()
                    .skip(taken)
                    .map(|order| order.0)
                    .collect();
                let ran = Mutex::new(Vec::new());
                let mut run = || {
                    let mut query = world.query_mut::<(Entity, &Order, &mut Visits)>();
                    for (_, _, visits) in query.by_ref().take(taken) {
                        visits.0 += 1;
                    }
                    query.par_for_each(|(entity, order, visits)| {
                        assert_eq!(entity, entities[order.0 as usize], "{order:?}'s entity");
                        visits.0 += 1;
                        ran.lock().unwrap().push(order.0);
                    });
                    // A query of one term.
                    world
                        .query_mut::<&mut Visits>()
                        .par_for_each(|visits| visits.0 += 1);
                };
                if threads == 1 {
                    run();
                    let ran = ran.into_inner().unwrap();
                    let parted = (0..ran.len().max(in_order.len()))
                        .find(|&row| ran.get(row) != in_order.get(row));
                    assert_eq!(
                        parted, None,
                        "the first row out of order, {taken} taken first"
                    );
                } else {
                    jobs::on_threads(threads, run);
                }
                for (order, visits) in world.query::<(&Order, &Visits)>() {
                    let at = format!(
                        "row {} with {threads} threads, {taken} taken first",
                        order.0
                    );
                    assert_eq!(*visits, Visits(2), "{at}");
                }
                assert_eq!(world.query::<&Visits>().count(), entities.len());
            }
        }
    }

    /// A panic in `f` reaches `par_for_each`'s caller with its payload, on one thread and four,
    /// whether the calling thread runs the row or another can.
    #[test]
    fn a_panic_in_par_for_each_reaches_its_caller() {
        for threads in [1, 4] {
            // The first run of the table of a chunk and a row (the calling thread runs it), and
            // the last run of the largest table (another thread can take it).
            for row in [3000, 8195] {
                let (mut world, _) = world();
                let mut run = || {
                    panic::catch_unwind(AssertUnwindSafe(|| {
                        world.query_mut::<&Order>().par_for_each(|order| {
                            if order.0 == row {
                                panic!("row {row}");
                            }
                        });
                    }))
                };
                let caught = if threads == 1 {
                    run()
                } else {
                    jobs::on_threads(threads, run)
                };
                let payload = caught.expect_err("a panic");
                assert_eq!(
                    payload.downcast_ref::<String>(),
                    Some(&format!("row {row}")),
                    "on {threads} threads"
                );
            }
        }
    }
}

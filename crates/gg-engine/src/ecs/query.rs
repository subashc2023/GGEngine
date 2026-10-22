//! Queries: each entity's components of some types, from every table that has them all.

use std::any::{TypeId, type_name};
use std::iter::{Copied, Zip};
use std::slice;

use super::table::{AnyColumn, ComponentId, Components, Table};
use super::{Component, Entity};

/// What a query takes of each entity: `&T` reads its `T`, `&mut T` changes it, and `Entity` is the
/// entity itself; or a tuple of up to 8 of them, each component named once.
/// [`World::query_mut`](super::World::query_mut) takes any, and
/// [`World::query`](super::World::query) those that only read.
#[diagnostic::on_unimplemented(
    message = "`{Self}` isn't a query",
    note = "a query's terms are `&T` and `&mut T` for components `T`, and `Entity`, alone or in a \
            tuple of up to 8, such as `(Entity, &Transform, &mut Sprite)`"
)]
pub trait Query: seal::Query {}

/// A [`Query`] that only reads: `&T` and `Entity` terms, which
/// [`World::query`](super::World::query) takes.
#[diagnostic::on_unimplemented(
    message = "`{Self}` isn't a query that only reads",
    note = "`World::query` takes `&T` and `Entity` terms, alone or in a tuple of up to 8; \
            `&mut T` needs `World::query_mut`"
)]
pub trait ReadOnlyQuery: Query + seal::ReadOnlyQuery {}

/// The entities a [`World::query`](super::World::query) finds, table by table.
pub struct QueryIter<'w, Q: ReadOnlyQuery> {
    tables: slice::Iter<'w, Table>,
    /// `None` if a component it needs has never been in the world, so that no table matches.
    needs: Option<Q::Needs>,
    /// The rows left of the table it's in.
    rows: Option<Q::Fetch<'w>>,
}

/// The entities a [`World::query_mut`](super::World::query_mut) finds, table by table.
pub struct QueryIterMut<'w, Q: Query> {
    tables: slice::IterMut<'w, Table>,
    needs: Option<Q::Needs>,
    rows: Option<Q::Fetch<'w>>,
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
    pub(super) fn new(tables: &'w mut [Table], components: &Components) -> Self {
        Q::check();
        Self {
            tables: tables.iter_mut(),
            needs: Q::needs(components),
            rows: None,
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

    /// A table at a time, so that `for_each` and the like loop over each table's columns.
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
            self.rows = Some(Q::fetch_mut(needs, table));
        }
    }

    /// A table at a time, so that `for_each` and the like loop over each table's columns.
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
                acc = Q::fetch_mut(needs, table).fold(acc, &mut f);
            }
        }
        acc
    }
}

/// The traits that only this module implements, which say how each query fetches.
mod seal {
    use super::*;

    pub trait Query {
        /// What it gives for each entity.
        type Item<'w>;
        /// A matching table's rows: its terms' slice iterators, zipped.
        type Fetch<'w>: Iterator<Item = Self::Item<'w>>;
        /// What it needs of a table: the ids of its terms' components.
        type Needs: Copy;

        /// Panics if it names a component twice.
        fn check();

        /// What it needs in a world that has seen `components`; `None` if one of its components
        /// isn't among them, so that no table matches.
        fn needs(components: &Components) -> Option<Self::Needs>;

        fn matches(needs: Self::Needs, table: &Table) -> bool;

        /// The rows of `table`, which matches.
        fn fetch_mut<'w>(needs: Self::Needs, table: &'w mut Table) -> Self::Fetch<'w>;
    }

    pub trait ReadOnlyQuery: Query {
        fn fetch<'w>(needs: Self::Needs, table: &'w Table) -> Self::Fetch<'w>;
    }

    /// A query's term: `&T`, `&mut T` or `Entity`.
    #[diagnostic::on_unimplemented(
        message = "`{Self}` isn't a query's term",
        note = "a query's terms are `&T` and `&mut T` for components `T`, and `Entity`"
    )]
    pub trait Term {
        type Item<'w>;
        type Fetch<'w>: Iterator<Item = Self::Item<'w>>;
        /// What it needs of a table: its component's id, or nothing for `Entity`.
        type Need: Copy;

        /// Its component's type and name; `None` for `Entity`.
        fn component() -> Option<(TypeId, &'static str)>;

        fn need(components: &Components) -> Option<Self::Need>;

        fn has(need: Self::Need, table: &Table) -> bool;

        /// Whether the column of component `id` is the one it takes.
        fn takes(need: Self::Need, id: ComponentId) -> bool;

        /// Its rows of a table, from the column it takes, which the table has, and the table's
        /// entities.
        fn fetch_mut<'w>(
            column: Option<&'w mut Box<dyn AnyColumn>>,
            entities: &'w [Entity],
        ) -> Self::Fetch<'w>;
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
    type Need = ComponentId;

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

    fn fetch_mut<'w>(
        column: Option<&'w mut Box<dyn AnyColumn>>,
        _: &'w [Entity],
    ) -> slice::Iter<'w, T> {
        let column: &'w dyn AnyColumn = &**column.expect("a matching table has the column");
        column.vec().iter()
    }
}

impl<T: Component> ReadTerm for &T {
    fn fetch<'w>(need: ComponentId, table: &'w Table) -> slice::Iter<'w, T> {
        let column = table.column(need);
        column.expect("a matching table has the column").iter()
    }
}

impl<T: Component> Term for &mut T {
    type Item<'w> = &'w mut T;
    type Fetch<'w> = slice::IterMut<'w, T>;
    type Need = ComponentId;

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

    fn fetch_mut<'w>(
        column: Option<&'w mut Box<dyn AnyColumn>>,
        _: &'w [Entity],
    ) -> slice::IterMut<'w, T> {
        let column = column.expect("a matching table has the column");
        column.vec_mut().iter_mut()
    }
}

impl Term for Entity {
    type Item<'w> = Entity;
    type Fetch<'w> = Copied<slice::Iter<'w, Entity>>;
    type Need = ();

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

    fn fetch_mut<'w>(
        _: Option<&'w mut Box<dyn AnyColumn>>,
        entities: &'w [Entity],
    ) -> Copied<slice::Iter<'w, Entity>> {
        entities.iter().copied()
    }
}

impl ReadTerm for Entity {
    fn fetch<'w>((): (), table: &'w Table) -> Copied<slice::Iter<'w, Entity>> {
        table.entities.iter().copied()
    }
}

/// A term alone is a query.
macro_rules! term_query {
    ($($term:ty => [$($generics:tt)*]),*) => {$(
        impl<$($generics)*> seal::Query for $term {
            type Item<'w> = <$term as Term>::Item<'w>;
            type Fetch<'w> = <$term as Term>::Fetch<'w>;
            type Needs = <$term as Term>::Need;

            fn check() {}

            fn needs(components: &Components) -> Option<Self::Needs> {
                <$term as Term>::need(components)
            }

            fn matches(needs: Self::Needs, table: &Table) -> bool {
                <$term as Term>::has(needs, table)
            }

            fn fetch_mut<'w>(needs: Self::Needs, table: &'w mut Table) -> Self::Fetch<'w> {
                let Table {
                    components,
                    columns,
                    entities,
                } = table;
                let column = components
                    .iter()
                    .zip(columns.iter_mut())
                    .find(|&(&id, _)| <$term as Term>::takes(needs, id))
                    .map(|(_, column)| column);
                <$term as Term>::fetch_mut(column, entities)
            }
        }

        impl<$($generics)*> Query for $term {}
    )*};
}

term_query!(&'a T => ['a, T: Component], &'a mut T => ['a, T: Component], Entity => []);

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

/// A tuple of terms is a query: `$Rows` is its tables' rows, its terms' slice iterators zipped,
/// each item flattened from the nested tuples zipping makes. Zipped slice iterators take std's
/// indexed path, so that each row costs one check of the count, whether a `for` loop takes the
/// rows one by one or `for_each` folds them.
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

            fn fetch_mut<'w>(needs: Self::Needs, table: &'w mut Table) -> Self::Fetch<'w> {
                let Table {
                    components,
                    columns,
                    entities,
                } = table;
                let entities: &'w [Entity] = entities;
                // Each column goes to the one term that takes it, so that no two terms reach
                // the same values: the borrow checker sees each reference moved once.
                $(let mut $t = None;)+
                for (&id, column) in components.iter().zip(columns.iter_mut()) {
                    $(
                        if $T::takes(needs.$i, id) {
                            $t = Some(column);
                            continue;
                        }
                    )+
                }
                $Rows(zip_fetches!($($T::fetch_mut($t, entities)),+))
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

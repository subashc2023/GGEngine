//! The world saved whole as text, and loaded back, as a snapshot file holds it, so that a restart in
//! another process goes on where the world was: each slot under its generation, the free ones in
//! the order they're taken, and the rest that hold no entity, those a log keeps vacant and those
//! retired, so that every slot is named, and a load makes no more than its lines name; each table
//! in the order it was made, empty ones too, and its rows in their order, each its entity's slot,
//! ID and components' values, as a recording's text writes values, by their fields' names; each
//! parent's children in their order; the stream the IDs come from; and the resources that save
//! themselves, as [`Saved`](super::Saved) says. Loaded, it's the world saved, hash for hash, but
//! for the slots a log kept vacant, which no log in another process gives back, so that they're
//! freed; and for what's opaque, which comes back only into a process whose layers added the same
//! assets in the same order, and is otherwise left out. A component whose type changed since is
//! matched to the type as it is by its fields' names, as [`World::load_whole`] says.

use std::any::{TypeId, type_name};
use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroU32;

use gg_core::math::Quat;
use gg_core::{Error, Result};

use super::MAX_DEPTH;
use super::hierarchy::Links;
use super::id::IdStream;
use super::scene::NO_HINTS;
use super::table::ComponentId;
use super::{Component, Entity, EntityId, Meta, Parent, World, next_serial};
use crate::cells::Seen;
use crate::recording::text::Text;
use crate::reflect::{Field, Ident, Meta as Hints, Shape, Type, Value};

/// What a load changed of what it read, to match the types as they are: each change once, with how
/// many times it came, for warnings that say what a restart gave up.
#[derive(Debug, Default)]
pub(crate) struct Changes(BTreeMap<String, u64>);

impl Changes {
    pub(crate) fn note(&mut self, change: String) {
        *self.0.entry(change).or_default() += 1;
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Each change, with how many times it came, in the order of their text.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, u64)> {
        self.0
            .iter()
            .map(|(change, &count)| (change.as_str(), count))
    }
}

/// A table as a load reads it: its line's number, its types' names, and its rows.
struct ReadTable<'t> {
    line: usize,
    types: Vec<&'t str>,
    rows: Vec<ReadRow<'t>>,
}

/// A row as a load reads it: its line's number, its entity's slot and ID, its components' values,
/// in its table's order, and the opaque state of those that keep one, by their place in it.
struct ReadRow<'t> {
    line: usize,
    slot: u32,
    id: EntityId,
    values: Vec<Text>,
    opaque: Vec<(usize, &'t str)>,
}

impl World {
    /// Writes the world whole after `out`, a line at a time, as the module says, its values as a
    /// recording's text writes them.
    ///
    /// Fails as a resource's [`Saved::save`](super::Saved::save) does; with `snapshot.bad_name`
    /// for a component's or a saved resource's type whose name isn't a word, which its line
    /// couldn't be read by; with `snapshot.too_deep` for a value nested deeper than a scene's may,
    /// as no load reads; and with `snapshot.line_break` for opaque state or a resource's text that
    /// breaks its line.
    pub(crate) fn save_whole(&self, out: &mut String) -> Result<()> {
        self.save_seen(out, None)
    }

    /// Writes what `seen` sees of the world after `out`, as [`save_whole`](Self::save_whole)
    /// writes the world, as a server's client's state: the entities it hides left out, each
    /// slot of theirs written as retired, at the highest generation, so that a load spawns no
    /// entity there and the entities spawned after take the slots this world's would; and its
    /// cells' state in place of the world's, in a world that has one. `None` is the world whole.
    ///
    /// Fails as `save_whole` does.
    pub(crate) fn save_seen(&self, out: &mut String, seen: Option<&Seen>) -> Result<()> {
        use std::fmt::Write as _;

        let _zone = gg_core::zone!();
        let hidden = seen.map_or(&[][..], |seen| &seen.hidden[..]);
        self.save_head(out, hidden);
        for table in 0..self.tables.len() {
            self.save_table(out, table, hidden)?;
        }
        self.save_children(out, hidden);
        for (name, text) in self.save_resources(seen)? {
            writeln!(out, "resource {name} {text}").expect("a string takes any text");
        }
        Ok(())
    }

    /// Writes the head of what [`save_seen`](Self::save_seen) writes after `out`, its lines before
    /// the tables': the stream, the slots, their generations, and the free and the vacant ones, of
    /// the world but for the entities whose slots `hidden` says, as [`Seen`]'s does.
    pub(crate) fn save_head(&self, out: &mut String, hidden: &[bool]) {
        use std::fmt::Write as _;

        let text = "a string takes any text";
        writeln!(out, "stream {:016x}", self.stream.state()).expect(text);
        writeln!(out, "slots {}", self.meta.len()).expect(text);
        for (index, meta) in self.meta.iter().enumerate() {
            if hidden.get(index) == Some(&true) {
                writeln!(out, "generation {index} {}", NonZeroU32::MAX).expect(text);
            } else if meta.generation != NonZeroU32::MIN {
                writeln!(out, "generation {index} {}", meta.generation).expect(text);
            }
        }
        out.push_str("free");
        for index in &self.free {
            write!(out, " {index}").expect(text);
        }
        out.push('\n');
        let mut vacant: Vec<bool> = (self.meta.iter().enumerate())
            .map(|(index, meta)| meta.location.is_none() || hidden.get(index) == Some(&true))
            .collect();
        for &index in &self.free {
            vacant[index as usize] = false;
        }
        out.push_str("vacant");
        for (index, _) in vacant.iter().enumerate().filter(|&(_, &vacant)| vacant) {
            write!(out, " {index}").expect(text);
        }
        out.push('\n');
    }

    /// Writes table `table` after `out`, as [`save_seen`](Self::save_seen) writes it: its `table`
    /// line and its rows, each row's `opaque` lines after it, but for the entities whose slots
    /// `hidden` says.
    ///
    /// Fails as `save_seen` does for a table's names and values.
    pub(crate) fn save_table(&self, out: &mut String, table: usize, hidden: &[bool]) -> Result<()> {
        use std::fmt::Write as _;

        use crate::recording::text;

        let text = "a string takes any text";
        let id = |entity: Entity| self.id(entity).expect("a row's entity is alive");
        let table = &self.tables[table];
        out.push_str("table");
        for &component in table.components() {
            let name = self.components.name(component);
            one_word(name)?;
            write!(out, " {name}").expect(text);
        }
        out.push('\n');
        for (row, &entity) in table.entities().iter().enumerate() {
            if super::hash::hidden(hidden, entity) {
                continue;
            }
            write!(out, "row {} {} ", entity.index, id(entity)).expect(text);
            let values: Vec<Value> = (table.columns().iter())
                .map(|column| column.value(row))
                .collect();
            let name = |at: usize| self.components.name(table.components()[at]);
            if let Some(at) = values.iter().position(|value| depth(value) > MAX_DEPTH) {
                return Err(Error::new(
                    "snapshot.too_deep",
                    format!(
                        "entity {}'s `{}` is more than {MAX_DEPTH} levels deep, the most a \
                         snapshot nests, as a scene does",
                        id(entity),
                        name(at)
                    ),
                ));
            }
            text::write(out, &Value::List(values), self);
            out.push('\n');
            for (at, column) in table.columns().iter().enumerate() {
                if let Some(opaque) = column.opaque(row) {
                    let what = format!("entity {}'s `{}`", id(entity), name(at));
                    one_line(&opaque, &what)?;
                    writeln!(out, "opaque {at} {opaque}").expect(text);
                }
            }
        }
        Ok(())
    }

    /// Writes the `children` lines of what [`save_seen`](Self::save_seen) writes after `out`: each
    /// parent's, in the order of the tables and their rows, but for the entities whose slots
    /// `hidden` says.
    pub(crate) fn save_children(&self, out: &mut String, hidden: &[bool]) {
        use std::fmt::Write as _;

        let text = "a string takes any text";
        let id = |entity: Entity| self.id(entity).expect("a row's entity is alive");
        // A view hides an entity with what's under it, so that a parent shown has its children.
        for table in &self.tables {
            let shown = |&&entity: &&Entity| !super::hash::hidden(hidden, entity);
            for &entity in table.entities().iter().filter(shown) {
                let mut children = self.children(entity).expect("a row's entity is alive");
                let Some(first) = children.next() else {
                    continue;
                };
                write!(out, "children {} {}", id(entity), id(first)).expect(text);
                for child in children {
                    write!(out, " {}", id(child)).expect(text);
                }
                out.push('\n');
            }
        }
    }

    /// The text of each resource that saves itself, by its name, as [`save_seen`](Self::save_seen)
    /// writes its `resource` lines, in the order their types first came: of what `seen` sees, as
    /// [`Saved::save_seen`](super::Saved::save_seen) writes a resource for it, and its cells'
    /// state in place of the world's.
    ///
    /// Fails as a resource's `save_seen` does, with `snapshot.bad_name` for a name that isn't a
    /// word, and with `snapshot.line_break` for text with a line break.
    pub(crate) fn save_resources(
        &self,
        seen: Option<&Seen>,
    ) -> Result<Vec<(&'static str, String)>> {
        self.resources.save(self, seen)
    }

    /// Puts the world that `lines` give, as [`save_whole`](Self::save_whole) wrote them, each with
    /// its number in its file, in place of this one's entities, resources and stream of IDs,
    /// keeping its component types and its assets; and returns what it changed of what it read to
    /// match the types as they are, which leaves the world otherwise than it was saved. Where a
    /// component's type changed since, its value is matched to the type by its fields' and its
    /// variants' names: a field the type hasn't is dropped, and one it has that the value hasn't
    /// takes its default; a number of another kind is taken where it's exactly a number of the
    /// field's; a texture is found by its name or a path to its file; and an entity by its ID. A
    /// component whose value that can't give, or whose type no longer is, is left out, its entity
    /// going on without it,
    /// as is one opaque as a whole unless `opaque` says the process's layers added the same assets
    /// in the same order as the saved one's did, so that its state names them. A resource is made
    /// by the [`Saved`](super::Saved) type of its name, and left out if there's none; one the world
    /// holds of a type the file holds none of stays, as the layers made it as they loaded.
    ///
    /// Fails, changing nothing, with `snapshot.syntax` for a line that isn't one, and with
    /// `snapshot.bad` for lines that don't make a world: a count of slots other than those they
    /// name, a slot past the slots, one given twice, an ID given twice, a child given two parents,
    /// a parent its child doesn't name, or an entity among its own ancestors.
    pub(crate) fn load_whole<'t>(
        &mut self,
        lines: impl IntoIterator<Item = (usize, &'t str)>,
        opaque: bool,
    ) -> Result<Changes> {
        let _zone = gg_core::zone!();
        let bad =
            |line: usize, what: String| Error::new("snapshot.bad", format!("line {line}: {what}"));
        let syntax =
            |line: usize, what: &str| Error::new("snapshot.syntax", format!("line {line}: {what}"));
        let (mut stream, mut slots) = (None, None);
        let mut generations = Vec::new();
        let mut free = Vec::new();
        let mut vacant = Vec::new();
        let mut tables: Vec<ReadTable<'t>> = Vec::new();
        let mut children = Vec::new();
        let mut resources = Vec::new();
        for (line, text) in lines {
            let (word, rest) = text.split_once(' ').unwrap_or((text, ""));
            let number = |text: &str| {
                text.parse::<u32>()
                    .map_err(|_| syntax(line, "not a number"))
            };
            let id = |text: &str| {
                text.parse::<EntityId>()
                    .map_err(|error| bad(line, format!("{error:#}")))
            };
            match word {
                "stream" => {
                    let state = u64::from_str_radix(rest, 16);
                    stream = Some(state.map_err(|_| syntax(line, "not 16 hex digits"))?);
                }
                "slots" => slots = Some(number(rest)? as usize),
                "generation" => {
                    let (index, generation) = rest
                        .split_once(' ')
                        .ok_or_else(|| syntax(line, "not a slot and its generation"))?;
                    let generation = NonZeroU32::new(number(generation)?);
                    let generation = generation.ok_or_else(|| syntax(line, "generation 0"))?;
                    generations.push((line, number(index)?, generation));
                }
                "free" => {
                    for index in rest.split_whitespace() {
                        free.push((line, number(index)?));
                    }
                }
                "vacant" => {
                    for index in rest.split_whitespace() {
                        vacant.push((line, number(index)?));
                    }
                }
                "table" => tables.push(ReadTable {
                    line,
                    types: rest.split_whitespace().collect(),
                    rows: Vec::new(),
                }),
                "row" => {
                    let mut parts = rest.splitn(3, ' ');
                    let (Some(slot), Some(entity), Some(values)) =
                        (parts.next(), parts.next(), parts.next())
                    else {
                        return Err(syntax(line, "not a slot, an ID and values"));
                    };
                    let table = tables
                        .last_mut()
                        .ok_or_else(|| syntax(line, "a row before a table"))?;
                    let from = text.len() - values.len() + 1;
                    let values = Text::parse(values, from)
                        .map_err(|error| syntax(line, &format!("{error:#}")))?;
                    let Text::List(values) = values else {
                        return Err(syntax(line, "a row's values are a list"));
                    };
                    if values.len() != table.types.len() {
                        return Err(bad(
                            line,
                            format!("{} values for {} types", values.len(), table.types.len()),
                        ));
                    }
                    table.rows.push(ReadRow {
                        line,
                        slot: number(slot)?,
                        id: id(entity)?,
                        values,
                        opaque: Vec::new(),
                    });
                }
                "opaque" => {
                    let (at, state) = rest
                        .split_once(' ')
                        .ok_or_else(|| syntax(line, "not a column and its state"))?;
                    let row = tables.last_mut().and_then(|table| table.rows.last_mut());
                    let row = row.ok_or_else(|| syntax(line, "opaque state before a row"))?;
                    row.opaque.push((number(at)? as usize, state));
                }
                "children" => {
                    let mut ids = rest.split_whitespace();
                    let parent = id(ids.next().ok_or_else(|| syntax(line, "no parent"))?)?;
                    let kids: Result<Vec<EntityId>> = ids.map(id).collect();
                    children.push((line, parent, kids?));
                }
                "resource" => {
                    let (name, state) = rest
                        .split_once(' ')
                        .ok_or_else(|| syntax(line, "not a name and its state"))?;
                    resources.push((line, name, state));
                }
                _ => return Err(syntax(line, &format!("`{word}` isn't a line of a world's"))),
            }
        }
        let (Some(stream), Some(slots)) = (stream, slots) else {
            return Err(Error::new(
                "snapshot.bad",
                "it has no `stream` or no `slots`",
            ));
        };
        // The slots are as many as the lines name, checked before any is made.
        let rows: usize = tables.iter().map(|table| table.rows.len()).sum();
        let named = free.len() + vacant.len() + rows;
        if named != slots {
            return Err(Error::new(
                "snapshot.bad",
                format!("it has {slots} slots, but names {named}: each is free, vacant or a row's"),
            ));
        }
        let mut world = World {
            meta: vec![
                Meta {
                    generation: NonZeroU32::MIN,
                    location: None,
                    links: Links::NONE,
                };
                slots
            ],
            free: Vec::new(),
            entity_ids: vec![None; slots],
            resets: vec![0; slots],
            reset_count: 0,
            by_id: HashMap::new(),
            id_room: 0,
            kept: HashMap::new(),
            stream: IdStream::new(stream),
            serial: next_serial(),
            version: 0,
            change_tick: self.change_tick,
            tables: Vec::new(),
            by_signature: HashMap::new(),
            components: self.components.clone(),
            scratch: Vec::new(),
            lists: Vec::new(),
            resources: self.resources.emptied(),
            assets: self.assets.clone(),
        };
        let slot = |line: usize, index: u32| {
            ((index as usize) < slots)
                .then_some(index)
                .ok_or_else(|| bad(line, format!("slot {index} is past the {slots} slots")))
        };
        for (line, index, generation) in generations {
            world.meta[slot(line, index)? as usize].generation = generation;
        }
        // Free, vacant or a row's: each slot is one at most, and so, as they're as many, one.
        let mut taken = vec![false; slots];
        for (line, index) in free {
            let index = slot(line, index)?;
            if std::mem::replace(&mut taken[index as usize], true) {
                return Err(bad(line, format!("slot {index} is given twice")));
            }
            world.free.push(index);
        }
        let mut vacant: Vec<u32> = (vacant.into_iter())
            .map(|(line, index)| {
                let index = slot(line, index)?;
                if std::mem::replace(&mut taken[index as usize], true) {
                    return Err(bad(line, format!("slot {index} is given twice")));
                }
                Ok(index)
            })
            .collect::<Result<_>>()?;
        for row in tables.iter().flat_map(|table| &table.rows) {
            let index = slot(row.line, row.slot)?;
            if std::mem::replace(&mut taken[index as usize], true) {
                return Err(bad(row.line, format!("slot {index} is given twice")));
            }
            let entity = Entity {
                index,
                generation: world.meta[index as usize].generation,
            };
            world.entity_ids[index as usize] = Some(row.id);
            if world.find_by_id(row.id, entity).is_some() {
                return Err(bad(row.line, format!("entity {} is given twice", row.id)));
            }
        }
        // Each table in its place, empty ones too, so that the tables made afterwards, of rows that
        // lost a component, come after them.
        let mut changes = Changes::default();
        let mut mapped = Vec::with_capacity(tables.len());
        for table in &tables {
            let ids: Vec<Option<ComponentId>> = (table.types.iter())
                .map(|&name| world.components.named(name))
                .collect();
            let mut signature: Vec<ComponentId> = ids.iter().flatten().copied().collect();
            signature.sort_unstable();
            if signature.windows(2).any(|pair| pair[0] == pair[1]) {
                return Err(bad(table.line, "a table has a type twice".to_owned()));
            }
            world.table(&signature);
            mapped.push(ids);
        }
        for (table, ids) in tables.iter().zip(&mapped) {
            for row in &table.rows {
                let entity = Entity {
                    index: row.slot,
                    generation: world.meta[row.slot as usize].generation,
                };
                let mut built = Vec::with_capacity(ids.len());
                for (at, (&name, id)) in table.types.iter().zip(ids).enumerate() {
                    let Some(id) = *id else {
                        changes.note(format!("`{name}`: left out, since no type has its name"));
                        continue;
                    };
                    let mut column = world.components.new_column(id);
                    let ty = world.components.ty(id);
                    let made = if ty.is_opaque(world.components.types()) {
                        let state = row.opaque.iter().find(|&&(column, _)| column == at);
                        match state {
                            Some((_, state)) if opaque => column
                                .push_opaque(state)
                                .map_err(|error| format!("{error:#}")),
                            Some(_) => {
                                Err("what's opaque comes back only into a run whose layers \
                                            added the same assets in the same order"
                                    .to_owned())
                            }
                            None => Err("it's opaque, and keeps nothing".to_owned()),
                        }
                    } else {
                        let value = &row.values[at];
                        migrate(&world, &ty, &NO_HINTS, value, name, &mut changes).and_then(
                            |value| {
                                column
                                    .push_value(&value)
                                    .map_err(|error| format!("{error:#}"))
                            },
                        )
                    };
                    match made {
                        Ok(()) => built.push((id, column)),
                        Err(why) => changes.note(format!("`{name}`: {why}, so it's left out")),
                    }
                }
                built.sort_unstable_by_key(|&(id, _)| id);
                world.place(entity, row.id, built);
            }
        }
        for (line, parent, kids) in children {
            let found = |world: &World, id: EntityId| {
                (world.entity(id)).ok_or_else(|| bad(line, format!("there's no entity {id}")))
            };
            let parent = found(&world, parent)?;
            for kid in kids {
                let child = found(&world, kid)?;
                if world.get::<Parent>(child).map(|up| up.0) != Some(parent) {
                    let says = format!("entity {kid}'s parent isn't the one its line gives");
                    return Err(bad(line, says));
                }
                if world.parent(child)?.is_some() {
                    return Err(bad(line, format!("entity {kid} is given two parents")));
                }
                world.append(parent.index, child.index);
            }
        }
        for table in &world.tables {
            for &entity in table.entities() {
                let parent = world.get::<Parent>(entity).map(|up| up.0);
                if parent.is_some() && world.parent(entity)? != parent {
                    let id = world.id(entity).expect("alive");
                    let says = format!("entity {id} has a parent, but no line gives it as a child");
                    return Err(Error::new("snapshot.bad", says));
                }
            }
        }
        // Each entity's parents end at one without a parent, or every walk up the hierarchy would
        // go round for good: each slot is unseen, on the walk going on, or reaches the top.
        let (mut walked, mut walk) = (vec![0u8; slots], Vec::new());
        for &entity in world.tables.iter().flat_map(|table| table.entities()) {
            let mut at = Some(entity);
            while let Some(up) = at
                && walked[up.slot()] != 2
            {
                if walked[up.slot()] == 1 {
                    let id = world.id(up).expect("alive");
                    let says = format!("entity {id} is among its own ancestors");
                    return Err(Error::new("snapshot.bad", says));
                }
                walked[up.slot()] = 1;
                walk.push(up);
                at = world.parent(up)?;
            }
            for up in walk.drain(..) {
                walked[up.slot()] = 2;
            }
        }
        // Those a log kept vacant, which no log here gives back, in their order; a retired one
        // stays retired.
        vacant.sort_unstable();
        for index in vacant {
            world.free_slot(index);
        }
        for (line, name, state) in resources {
            let Some(saved) = world.resources.saved(name) else {
                let why = "left out, since no resource saves under that name";
                changes.note(format!("resource `{name}`: {why}"));
                continue;
            };
            match (saved.load)(state, &world) {
                Ok(resource) => world.resources.restore(saved.type_id, resource),
                Err(error) => {
                    let why = format!("line {line}: {error:#}, so it's left out");
                    changes.note(format!("resource `{name}`: {why}"));
                }
            }
        }
        world.resources.keep(std::mem::take(&mut self.resources));
        // Models' nodes as the snapshot kept them, code's changes among them, but for their draws,
        // which a run of other assets leaves out, as it leaves out what's opaque.
        if !opaque {
            world.redraw_nodes();
        }
        *self = world;
        Ok(changes)
    }

    /// The component of type `T` that `text` gives, a value as a recording's text writes values,
    /// written for `T` as it was, matched to `T` as it is as [`load_whole`](Self::load_whole)
    /// matches components: for a resource that keeps components, as physics keeps what its last
    /// step left in them.
    ///
    /// Fails with `snapshot.syntax` for text that isn't a value, and with `snapshot.changed` for one
    /// that no longer makes a `T`, or if `T` isn't registered.
    pub(crate) fn component_from_text<T: Component>(&self, text: &str) -> Result<T> {
        let changed = |why: String| Error::new("snapshot.changed", why);
        let parsed = Text::parse(text, 1)
            .map_err(|error| Error::new("snapshot.syntax", format!("{error:#}")))?;
        let id = self.components.get(TypeId::of::<T>());
        let id = id.ok_or_else(|| changed(format!("`{}` isn't registered", type_name::<T>())))?;
        let name = self.components.name(id);
        let ty = self.components.ty(id);
        let value = migrate(self, &ty, &NO_HINTS, &parsed, name, &mut Changes::default());
        let value = value.map_err(changed)?;
        let made = T::base().map_or_else(|| T::from_value(&value), |base| base.with_value(&value));
        made.map_err(|error| changed(format!("{error:#}")))
    }
}

/// The value of type `ty`, under `hints`, its field's, that `text`, written for the type as it was,
/// gives as the type is, as [`World::load_whole`] matches them, in `world`, whose entities are all
/// there, noting in `changes` what it changed of it on the way, at `path`. A note says what kind of
/// value it was, never the value, and a list's or an array's elements are all at `[..]`, so that
/// one change to a type is noted once, however many values it changed.
///
/// Fails, saying why, where it gives none, as for a variant the type no longer has.
fn migrate(
    world: &World,
    ty: &Type,
    hints: &Hints,
    text: &Text,
    path: &str,
    changes: &mut Changes,
) -> Result<Value, String> {
    let types = world.components.types();
    let unheld = || "it's no longer a number its type holds".to_owned();
    // A number taken as one of another kind, which the hash digests otherwise, is noted, so that a
    // restore whose values are all as they were but for it isn't taken to have lost something.
    let mut converted = |to: &str| {
        changes.note(format!("`{path}`: its `{}` taken as {to}", text.kind()));
    };
    let value = match (ty, text) {
        (Type::Opaque, _) => Value::Opaque,
        (&Type::Named(index), _) if matches!(types.info(index).shape, Shape::Opaque) => {
            Value::Opaque
        }
        (Type::Bool, &Text::Bool(b)) => Value::Bool(b),
        (
            &Type::Int { bits, signed },
            Text::Int(_) | Text::UInt(_) | Text::F32(_) | Text::F64(_),
        ) => {
            let n = match *text {
                Text::Int(n) => Some(i128::from(n)),
                Text::UInt(n) => Some(i128::from(n)),
                Text::F32(x) => whole(f64::from(x)),
                Text::F64(x) => whole(x),
                _ => None,
            };
            let (least, most) = match signed {
                true => (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1),
                false => (0, (1i128 << bits) - 1),
            };
            let n = n
                .filter(|n| (least..=most).contains(n))
                .ok_or_else(unheld)?;
            if let Text::F32(_) | Text::F64(_) = text {
                converted("an integer");
            }
            if signed {
                Value::Int(n as i64)
            } else {
                Value::UInt(n as u64)
            }
        }
        (Type::F32, &Text::F32(x)) => Value::F32(x),
        (Type::F64, &Text::F64(x)) => Value::F64(x),
        // Compared in `i128`, which holds every integer of 64 bits, and the cast back of a float
        // past them, so that a cast that saturates doesn't pass for exact.
        (Type::F32, Text::F64(_) | Text::Int(_) | Text::UInt(_)) => {
            let x = match *text {
                Text::F64(x) if x.is_nan() || f64::from(x as f32) == x => Some(x as f32),
                Text::Int(n) if n as f32 as i128 == i128::from(n) => Some(n as f32),
                Text::UInt(n) if n as f32 as i128 == i128::from(n) => Some(n as f32),
                _ => None,
            };
            let x = x.ok_or_else(unheld)?;
            converted("an `F32`");
            Value::F32(x)
        }
        (Type::F64, Text::F32(_) | Text::Int(_) | Text::UInt(_)) => {
            let x = match *text {
                Text::F32(x) => Some(f64::from(x)),
                Text::Int(n) if n as f64 as i128 == i128::from(n) => Some(n as f64),
                Text::UInt(n) if n as f64 as i128 == i128::from(n) => Some(n as f64),
                _ => None,
            };
            let x = x.ok_or_else(unheld)?;
            converted("an `F64`");
            Value::F64(x)
        }
        (Type::String, Text::String(text)) => Value::String(text.clone()),
        (Type::Vec2, &Text::Vec2(x, y)) => Value::Vec2([x, y].into()),
        (Type::Vec3, &Text::Vec3(x, y, z)) => Value::Vec3([x, y, z].into()),
        (Type::Vec4, &Text::Vec4(x, y, z, w)) => Value::Vec4([x, y, z, w].into()),
        (Type::DVec3, &Text::DVec3(x, y, z)) => Value::DVec3([x, y, z].into()),
        // As a translation was before it was `f64`, widened exactly.
        (Type::DVec3, &Text::Vec3(x, y, z)) => {
            converted("a `DVec3`");
            Value::DVec3([x, y, z].map(f64::from).into())
        }
        (Type::Quat, &Text::Quat(x, y, z, w)) => {
            let rotation = Quat::from_xyzw(x, y, z, w);
            let length = rotation.length();
            if rotation.is_normalized() {
                Value::Quat(rotation)
            } else if length.is_finite() && length > 0.0 {
                // As rounding takes one off unit length that's multiplied by others tick after
                // tick, which a rotation can't be set to.
                changes.note(format!(
                    "`{path}`: it wasn't a unit quaternion, so it's normalized"
                ));
                Value::Quat(rotation / length)
            } else {
                return Err("it isn't a rotation, which a unit quaternion is".to_owned());
            }
        }
        (Type::Entity, Text::Entity(id)) => {
            Value::Entity(world.entity(id.0).unwrap_or(Entity::DEAD))
        }
        (Type::Entity, Text::Dead) => Value::Entity(Entity::DEAD),
        (Type::Texture, Text::Texture(at)) => match world.assets().texture(at) {
            Some(texture) => Value::Texture(texture),
            None => return Err(world.assets().not_found("texture", at)),
        },
        (Type::Model, Text::Model(at)) => match world.assets().model(at) {
            Some(model) => Value::Model(model),
            None => return Err(world.assets().not_found("model", at)),
        },
        (Type::Font, Text::Font(at)) => match world.assets().font(at) {
            Some(font) => Value::Font(font),
            None => return Err(world.assets().not_found("font", at)),
        },
        (Type::Prefab, Text::Prefab(at)) => match world.assets().prefab(at) {
            Some(prefab) => Value::Prefab(prefab),
            None => return Err(world.assets().not_found("prefab", at)),
        },
        (Type::Option(_), Text::None) => Value::Option(None),
        (Type::Option(inner), Text::Some(text)) => {
            match migrate(world, inner, hints, text, path, changes) {
                Ok(value) => Value::Option(Some(Box::new(value))),
                Err(why) => {
                    changes.note(format!("`{path}`: {why}, so it's none"));
                    Value::Option(None)
                }
            }
        }
        (Type::List(inner), Text::List(items)) => {
            let at = format!("{path}[..]");
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                match migrate(world, inner, hints, item, &at, changes) {
                    Ok(value) => values.push(value),
                    Err(why) => changes.note(format!("`{at}`: {why}, so it's dropped")),
                }
            }
            Value::List(values)
        }
        (&Type::Array(ref inner, len), Text::List(items)) => {
            if items.len() != len as usize {
                changes.note(format!(
                    "`{path}`: it had {} elements where its type holds {len}, matched by their \
                     places",
                    items.len()
                ));
            }
            let at = format!("{path}[..]");
            let mut values = Vec::with_capacity(len as usize);
            for i in 0..len as usize {
                let value = items
                    .get(i)
                    .map(|item| migrate(world, inner, hints, item, &at, changes));
                // Under the field's bounds, as a new component's elements are.
                let value = match value {
                    Some(Ok(value)) => value,
                    Some(Err(why)) => {
                        changes.note(format!("`{at}`: {why}, so it takes its default"));
                        default(types.default_value(inner, hints))?
                    }
                    None => default(types.default_value(inner, hints))?,
                };
                values.push(value);
            }
            Value::List(values)
        }
        (&Type::Named(index), Text::Struct(given)) => match &types.info(index).shape {
            Shape::Struct { fields, .. } => {
                Value::Struct(fields_of(world, fields, given, path, changes)?)
            }
            _ => return Err("it's no longer a struct".to_owned()),
        },
        (&Type::Named(index), Text::Variant(name, given)) => match &types.info(index).shape {
            Shape::Enum(variants) => {
                let Some(variant) = variants.iter().find(|variant| variant.name == *name) else {
                    return Err(format!("its type no longer has a variant `{name}`"));
                };
                let path = format!("{path}.{name}");
                let fields = fields_of(world, &variant.fields, given, &path, changes)?;
                Value::Variant(variant.name.clone(), fields)
            }
            _ => return Err("it's no longer an enum".to_owned()),
        },
        _ => {
            return Err(format!(
                "its `{}` is no longer of its type's kind",
                text.kind()
            ));
        }
    };
    Ok(value)
}

/// The values of `fields` that `given` gives, matched by name as [`migrate`] matches them: each
/// that `given` hasn't, or whose value gives none, at its default.
///
/// Fails, saying why, if such a field has no default.
fn fields_of(
    world: &World,
    fields: &[Field],
    given: &[(String, Text)],
    path: &str,
    changes: &mut Changes,
) -> Result<Vec<(Ident, Value)>, String> {
    let types = world.components.types();
    let mut values = Vec::with_capacity(fields.len());
    for field in fields {
        let at = format!("{path}.{}", field.name);
        let found = given.iter().find(|(name, _)| *name == field.name);
        let value = match found {
            _ if field.ty.is_opaque(types) => Value::Opaque,
            Some((_, text)) => match migrate(world, &field.ty, &field.meta, text, &at, changes) {
                Ok(value) => value,
                Err(why) => {
                    changes.note(format!("`{at}`: {why}, so it takes its default"));
                    default(types.field_default(field))?
                }
            },
            None => {
                changes.note(format!("`{at}`: added, at its default"));
                default(types.field_default(field))?
            }
        };
        values.push((field.name.clone(), value));
    }
    let given_names = given.iter().map(|(name, _)| name.as_str());
    let names = fields.iter().map(|field| &*field.name);
    for name in given_names.clone() {
        if !names.clone().any(|field| field == name) {
            changes.note(format!(
                "`{path}.{name}`: dropped, since its type no longer has a field of that name"
            ));
        }
    }
    // The fields both have, in another order, which the hash digests otherwise.
    if !given_names.clone().eq(names.clone()) {
        let theirs = given_names.filter(|&name| names.clone().any(|field| field == name));
        let ours = (names.clone()).filter(|&field| given.iter().any(|(name, _)| name == field));
        if !theirs.eq(ours) {
            changes.note(format!(
                "`{path}`: its fields are in another order, matched by name"
            ));
        }
    }
    Ok(values)
}

/// A default, or why there's none.
fn default(made: Result<Value>) -> Result<Value, String> {
    made.map_err(|error| format!("{error:#}"))
}

/// `x` as an integer, if it's exactly one: a cast that rounds it, or saturates, gives another.
fn whole(x: f64) -> Option<i128> {
    let n = x as i128;
    (n as f64 == x).then_some(n)
}

/// Refuses `name`, a type's, which a snapshot's lines split at whitespace, unless it's a word.
pub(super) fn one_word(name: &str) -> Result<()> {
    if !name.is_empty() && !name.contains(char::is_whitespace) {
        return Ok(());
    }
    Err(Error::new(
        "snapshot.bad_name",
        format!("`{name}` isn't a name a snapshot writes, which is a word, without whitespace"),
    ))
}

/// Refuses `text`, what `what` keeps on a line of a snapshot's, if it breaks the line.
pub(super) fn one_line(text: &str, what: &str) -> Result<()> {
    if !text.contains(['\n', '\r']) {
        return Ok(());
    }
    Err(Error::new(
        "snapshot.line_break",
        format!("{what} writes a line break, which would end its line of the snapshot"),
    ))
}

/// How many levels `value` nests, as [`MAX_DEPTH`] counts them: it's the first, and each value it
/// holds a level deeper than it.
fn depth(value: &Value) -> u32 {
    let inner = match value {
        Value::Option(Some(value)) => depth(value),
        Value::List(items) => items.iter().map(depth).max().unwrap_or(0),
        Value::Struct(fields) | Value::Variant(_, fields) => {
            let depths = fields.iter().map(|(_, value)| depth(value));
            depths.max().unwrap_or(0)
        }
        _ => 0,
    };
    1 + inner
}

// They save worlds, which only development builds do.
#[cfg(all(test, feature = "dev"))]
mod tests {
    use gg_core::math::{DVec3, Vec2, Vec3};

    use super::*;
    use crate::ecs::hierarchy::tests::check_links;
    use crate::ecs::id::tests::{check_ids, vacated};
    use crate::ecs::ops::tests::{Sealed, fixture};
    use crate::reflect::tests::{Everything, same_snapshots, snapshot};
    use crate::{Component, Log, Operation, Reflect, Saved, Sprite, Transform, Writable};

    /// `text`'s lines, each with its number.
    fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
        (1..).zip(text.lines())
    }

    /// A world of the fixture's types, registered in its order, sharing `assets`, which saves
    /// [`Count`].
    fn fixture_types(seed: u64, from: &World) -> World {
        let mut world = World::seeded(seed);
        let registered = [
            world.register::<Transform>(),
            world.register::<Sprite>(),
            world.register::<Everything>(),
            world.register::<Sealed>(),
        ];
        registered
            .into_iter()
            .for_each(|done| done.expect("registered"));
        world.assets = from.assets.clone();
        world.register_saved::<Count>().expect("saved");
        world
    }

    /// A resource that saves itself.
    #[derive(Clone, Debug, PartialEq)]
    struct Count(u32);

    impl Saved for Count {
        const NAME: &'static str = "Count";

        fn save(&self, _: &World) -> Result<String> {
            Ok(self.0.to_string())
        }

        fn load(text: &str, _: &World) -> Result<Self> {
            let count = text
                .parse()
                .map_err(|_| Error::new("test.count", "not a count"));
            count.map(Self)
        }
    }

    /// A world loaded from its file is the world saved: the same slots under the same
    /// generations, the free ones in the same order, the same entities under their handles and
    /// IDs, in the same tables and rows, with the same values, parents and children, the same
    /// stream of IDs to come, the resources that save themselves, and so the same hash; and it
    /// goes on as the world saved does, spawning the same handles with the same IDs.
    #[test]
    fn a_world_loaded_from_its_file_is_the_world_saved() {
        let mut x = fixture();
        // Opaque whole, which keeps nothing.
        x.world.despawn(x.g).expect("alive");
        // Slots freed, and one taken again, at its next generation.
        let [one, two] = [0, 1].map(|_| x.world.spawn(Transform::IDENTITY));
        x.world.despawn(one).expect("alive");
        x.world.despawn(two).expect("alive");
        x.world.spawn(Transform::IDENTITY);
        x.world.register_saved::<Count>().expect("saved");
        x.world.insert_resource(Count(3));
        let mut text = String::new();
        x.world.save_whole(&mut text).expect("saved");
        let mut loaded = fixture_types(99, &x.world);
        // As layers make them as they load: the file's of a type it saves takes the place of the
        // world's, and one of a type it doesn't stays.
        loaded.insert_resource(Count(0));
        loaded.insert_resource("loaded");
        let changes = loaded.load_whole(lines(&text), true).expect("loaded");
        assert!(changes.is_empty(), "{changes:?}");
        assert_eq!(loaded.resource::<&str>(), Some(&"loaded"));
        for world in [&x.world, &loaded] {
            check_ids(world);
            check_links(world);
        }
        assert_eq!(loaded.hash(), x.world.hash());
        assert_eq!(loaded.meta, x.world.meta, "the slots");
        assert_eq!(loaded.free, x.world.free, "the free slots");
        assert_eq!(loaded.entity_ids, x.world.entity_ids, "the IDs");
        assert_eq!(loaded.stream, x.world.stream, "the stream");
        let tables = |world: &World| -> Vec<(Box<[ComponentId]>, Vec<Entity>)> {
            let tables = world.tables.iter();
            tables
                .map(|table| (table.components().into(), table.entities().to_vec()))
                .collect()
        };
        assert_eq!(
            tables(&loaded),
            tables(&x.world),
            "the tables and their rows"
        );
        assert!(same_snapshots(&snapshot(&loaded), &snapshot(&x.world)));
        assert_eq!(loaded.resource::<Count>(), Some(&Count(3)));
        assert_ne!(loaded.serial, x.world.serial, "another world");
        for world in [&mut x.world, &mut loaded] {
            world.spawn((Transform::IDENTITY, Sprite::default()));
        }
        assert_eq!(loaded.hash(), x.world.hash(), "a spawn after");
        assert_eq!(loaded.entity_ids, x.world.entity_ids, "a spawn after");
    }

    /// The slots a log keeps vacant for its undo are freed, since no log in the world loaded gives
    /// them back.
    #[test]
    fn the_slots_a_log_kept_are_freed() {
        let mut x = fixture();
        x.world.despawn(x.g).expect("alive");
        let mut log = Log::default();
        let delete = Operation::Delete { entity: x.id(x.b) };
        log.apply(&mut x.world, delete).expect("deleted");
        let kept = vacated(&x.world);
        assert!(kept > 0, "the delete keeps slots");
        let mut text = String::new();
        x.world.save_whole(&mut text).expect("saved");
        let mut loaded = fixture_types(1, &x.world);
        loaded.load_whole(lines(&text), true).expect("loaded");
        assert_eq!(vacated(&loaded), 0);
        assert_eq!(loaded.free.len(), x.world.free.len() + kept);
        assert_eq!(loaded.hash(), x.world.hash());
        check_ids(&loaded);
    }

    /// A component as it was saved, which [`After`] takes the place of.
    #[derive(Clone, crate::Component)]
    #[reflect(name = "Shape")]
    struct Before {
        a: f32,
        b: u32,
        c: String,
        mode: OldMode,
        at: [Vec2; 2],
        path: Vec<Vec2>,
        turn: Quat,
        big: u64,
        far: Vec3,
    }

    #[derive(Clone, crate::Reflect)]
    #[reflect(name = "Mode")]
    enum OldMode {
        Idle,
        Gone(u8),
    }

    /// [`Before`] as its type changed: its fields reordered, one dropped, one added, a number
    /// widened, a variant gone, an array longer, under bounds, a list's elements of another kind,
    /// a number taken by a type that can't hold it exactly, and a vector widened, as a
    /// translation was; its rotation as it was, but written off unit length.
    #[derive(Clone, Debug, PartialEq, crate::Component)]
    #[reflect(name = "Shape")]
    struct After {
        b: u64,
        a: f64,
        #[reflect(default = Vec2::new(1.0, 2.0))]
        d: Vec2,
        mode: NewMode,
        #[reflect(min = 1.5)]
        at: [Vec2; 3],
        path: Vec<Vec3>,
        turn: Quat,
        big: f64,
        far: DVec3,
    }

    #[derive(Clone, Debug, PartialEq, crate::Reflect)]
    #[reflect(name = "Mode")]
    enum NewMode {
        Idle,
        Kept,
    }

    /// A component whose type changed since it was saved is matched to its type by its fields'
    /// names and its variants', each change said once, whatever the values, with how many times it
    /// came; one whose type is gone is left out, its entity going on without it.
    #[test]
    fn a_component_whose_type_changed_is_matched_by_its_fields_names() {
        let mut saved = World::seeded(3);
        let before = |mode, path| Before {
            a: 1.5,
            b: 7,
            c: "dropped".to_owned(),
            mode,
            at: [Vec2::splat(2.0), Vec2::splat(3.0)],
            path,
            turn: Quat::from_xyzw(0.0, 0.0, 0.0, 2.0),
            big: u64::MAX,
            far: Vec3::new(1.0e6, 0.1, -2.5),
        };
        let kept = saved.spawn((
            Transform::IDENTITY,
            before(OldMode::Idle, vec![Vec2::X, Vec2::Y]),
        ));
        let gone = saved.spawn((
            Transform::IDENTITY,
            before(OldMode::Gone(3), vec![Vec2::ONE]),
        ));
        let mut text = String::new();
        saved.save_whole(&mut text).expect("saved");
        let mut loaded = World::seeded(4);
        loaded.register::<After>().expect("registered");
        loaded.register::<Transform>().expect("registered");
        let changes = loaded.load_whole(lines(&text), true).expect("loaded");
        let after = |mode| After {
            b: 7,
            a: 1.5,
            d: Vec2::new(1.0, 2.0),
            mode,
            at: [Vec2::splat(2.0), Vec2::splat(3.0), Vec2::splat(1.5)],
            path: Vec::new(),
            turn: Quat::IDENTITY,
            big: 0.0,
            far: DVec3::new(1.0e6, f64::from(0.1f32), -2.5),
        };
        assert_eq!(loaded.get::<After>(kept), Some(&after(NewMode::Idle)));
        assert_eq!(loaded.get::<After>(gone), Some(&after(NewMode::Idle)));
        let changes: Vec<(String, u64)> = (changes.iter())
            .map(|(change, count)| (change.to_owned(), count))
            .collect();
        let want = [
            ("`Shape.a`: its `F32` taken as an `F64`", 2),
            (
                "`Shape.at`: it had 2 elements where its type holds 3, matched by their places",
                2,
            ),
            (
                "`Shape.big`: it's no longer a number its type holds, so it takes its default",
                2,
            ),
            (
                "`Shape.c`: dropped, since its type no longer has a field of that name",
                2,
            ),
            ("`Shape.d`: added, at its default", 2),
            ("`Shape.far`: its `Vec3` taken as a `DVec3`", 2),
            (
                "`Shape.mode`: its type no longer has a variant `Gone`, so it takes its default",
                1,
            ),
            (
                "`Shape.path[..]`: its `Vec2` is no longer of its type's kind, so it's dropped",
                3,
            ),
            (
                "`Shape.turn`: it wasn't a unit quaternion, so it's normalized",
                2,
            ),
            (
                "`Shape`: its fields are in another order, matched by name",
                2,
            ),
        ];
        let want: Vec<(String, u64)> = (want.iter())
            .map(|&(change, count)| (change.to_owned(), count))
            .collect();
        assert_eq!(changes, want);
        // A world without the type.
        let mut without = World::seeded(5);
        without.register::<Transform>().expect("registered");
        let changes = without.load_whole(lines(&text), true).expect("loaded");
        let left_out: Vec<_> = changes.iter().collect();
        assert_eq!(
            left_out,
            [("`Shape`: left out, since no type has its name", 2)]
        );
        assert_eq!(without.get::<Transform>(kept), Some(&Transform::IDENTITY));
        check_ids(&without);
    }

    /// Opaque as a whole, keeping its state in snapshots.
    #[derive(Clone, Debug, PartialEq, Reflect)]
    #[reflect(opaque)]
    struct Kept(u32);

    impl Component for Kept {
        fn save_opaque(&self) -> Option<String> {
            Some(self.0.to_string())
        }

        fn load_opaque(text: &str) -> Result<Self> {
            let kept = text
                .parse()
                .map_err(|_| Error::new("test.kept", "not a number"));
            kept.map(Self)
        }
    }

    impl Writable for Kept {}

    /// A component opaque as a whole comes back from what it keeps, into a world whose assets
    /// are those it was saved with, and is left out otherwise, as one that keeps nothing is.
    #[test]
    fn what_is_opaque_comes_back_only_where_the_assets_are_the_same() {
        let mut saved = World::seeded(6);
        let entity = saved.spawn((Transform::IDENTITY, Kept(42), Sealed(7)));
        let mut text = String::new();
        saved.save_whole(&mut text).expect("saved");
        let registered = || {
            let mut world = World::seeded(7);
            world.register::<Transform>().expect("registered");
            world.register::<Kept>().expect("registered");
            world.register::<Sealed>().expect("registered");
            world
        };
        let mut same = registered();
        let changes = same.load_whole(lines(&text), true).expect("loaded");
        assert_eq!(same.get::<Kept>(entity), Some(&Kept(42)));
        assert_eq!(same.get::<Sealed>(entity), None);
        let left_out: Vec<_> = changes.iter().collect();
        assert_eq!(
            left_out,
            [(
                "`Sealed`: it's opaque, and keeps nothing, so it's left out",
                1
            )]
        );
        let mut other = registered();
        let changes = other.load_whole(lines(&text), false).expect("loaded");
        assert_eq!(other.get::<Kept>(entity), None);
        assert_eq!(other.get::<Transform>(entity), Some(&Transform::IDENTITY));
        assert!(
            changes
                .iter()
                .any(|(change, _)| change.starts_with("`Kept`: what's opaque"))
        );
    }

    /// Lines that don't make a world are refused, changing nothing.
    #[test]
    fn lines_that_dont_make_a_world_are_refused() {
        let mut saved = World::seeded(8);
        let parent = saved.spawn(Transform::IDENTITY);
        let child = saved.spawn(Transform::IDENTITY);
        saved.set_parent(child, Some(parent)).expect("no cycle");
        let mut text = String::new();
        saved.save_whole(&mut text).expect("saved");
        let row = text
            .lines()
            .find(|line| line.starts_with("row 1 "))
            .expect("a row");
        let children = text
            .lines()
            .find(|line| line.starts_with("children"))
            .expect("children");
        let id = |entity| saved.id(entity).expect("alive").to_string();
        let entity = |entity| format!("Entity(\"{}\")", id(entity));
        // The child its own parent, its line and its row agreeing.
        let looped = (text.replace(&entity(parent), &entity(child)))
            .replace(children, &format!("children {} {}", id(child), id(child)));
        for (text, code) in [
            (text.replace("slots 2", "slots 1"), "snapshot.bad"),
            // More slots than it names, as many as a slot's index takes, refused before they're
            // made.
            (text.replace("slots 2", "slots 4294967295"), "snapshot.bad"),
            (text.replace("slots 2", "slots 3"), "snapshot.bad"),
            (text.replacen("free", "free 0", 1), "snapshot.bad"),
            (
                text.replace("slots 2", "slots 3")
                    .replace("vacant", "vacant 1"),
                "snapshot.bad",
            ),
            (text.replace(&id(child), &id(parent)), "snapshot.bad"),
            (text.replace(&format!("{children}\n"), ""), "snapshot.bad"),
            (format!("{text}{row}\n"), "snapshot.bad"),
            (looped.clone(), "snapshot.bad"),
            (text.replace("stream", "streem"), "snapshot.syntax"),
        ] {
            let mut world = World::seeded(9);
            world.register::<Transform>().expect("registered");
            let serial = world.serial;
            let error = world.load_whole(lines(&text), true).expect_err(&text);
            assert_eq!(error.code(), code, "{error}");
            assert_eq!(world.serial, serial, "unchanged");
        }
        let mut world = World::seeded(9);
        world.register::<Transform>().expect("registered");
        let error = world.load_whole(lines(&looped), true).expect_err("a loop");
        assert!(error.to_string().contains("its own ancestors"), "{error}");
    }

    /// A type whose name isn't a word.
    #[derive(Clone, crate::Component)]
    #[reflect(name = "Two Words")]
    struct Spaced;

    /// Opaque as a whole, keeping text that breaks its line.
    #[derive(Clone, Reflect)]
    #[reflect(opaque)]
    struct Broken;

    impl Component for Broken {
        fn save_opaque(&self) -> Option<String> {
            Some("two\nlines".to_owned())
        }
    }

    impl Writable for Broken {}

    /// As deep as it's made.
    #[derive(Clone, crate::Component)]
    struct Nest {
        kids: Vec<Nest>,
    }

    /// A world that holds what no load could read isn't saved: a type whose name isn't a word,
    /// opaque state that breaks its line, and a value nested deeper than a load reads.
    #[test]
    fn a_world_no_load_could_read_is_refused() {
        let mut nest = Nest { kids: Vec::new() };
        for _ in 0..40 {
            nest = Nest { kids: vec![nest] };
        }
        let spawn: [&dyn Fn(&mut World); 3] = [
            &|world| {
                let _ = world.spawn(Spaced);
            },
            &|world| {
                let _ = world.spawn(Broken);
            },
            &|world| {
                let _ = world.spawn(nest.clone());
            },
        ];
        let codes = spawn.map(|spawn| {
            let mut world = World::seeded(10);
            spawn(&mut world);
            let saved = world.save_whole(&mut String::new());
            saved.err().map(|error| error.code().to_owned())
        });
        let want = [
            "snapshot.bad_name",
            "snapshot.line_break",
            "snapshot.too_deep",
        ];
        assert_eq!(codes, want.map(|code| Some(code.to_owned())));
    }
}

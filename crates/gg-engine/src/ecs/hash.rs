//! The world's hash, which two runs compare at each tick to find where they part: its content
//! (components keyed by entity ID) and its layout (how the world holds them).

use std::fmt;

use super::id::mix64;
use super::{Entity, EntityId, World};
use crate::cells::{Cell, CellState, Seen};
use crate::reflect::Reflect;
use crate::{Font, Model, Prefab, Texture};

// Starting states of a type's key, a content term and the layout: 64-bit words of π's fraction
// after the ID stream's (id.rs). None is the zero that `mix64` keeps.
const TYPE: u64 = 0xa409_3822_299f_31d0;
const LAYOUT: u64 = 0x082e_fa98_ec4e_6c89;
const TERM: u64 = 0x4528_21e6_38d0_1377;
const CELLS: u64 = 0xbe54_66cf_34e9_0c6c;
const TEXT: u64 = 0xc0ac_29b7_c97c_50dd;

// What every NaN hashes as: arithmetic leaves a NaN's sign and payload to the platform, and x86's
// and ARM's differ. Literals, as std doesn't promise `f32::NAN`'s bits.
const NAN32: u32 = 0x7fc0_0000;
const NAN64: u64 = 0x7ff8_0000_0000_0000;

/// A world's hash: equal at every tick for two runs of a simulation that go the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldHash {
    /// The components, each under its entity's ID and type name, summed, so where the world
    /// holds them doesn't count. Reads what reflection reads, minus opaque fields; an entity a
    /// field holds counts as its ID, or 0 if dead. Undoing a log restores it. Leaves out
    /// componentless entities, slots, handles and type registration order.
    pub content: u64,
    /// How the world holds its entities, which decides query order:
    /// - tables in creation order, each with its types and its rows' IDs in order;
    /// - each parent's children's IDs, in order;
    /// - the ID stream's state;
    /// - in a world of [cells](crate::Cells): those in, those refused, and each cell out's kept
    ///   entity text (omitted while the cells never came in).
    ///
    /// Leaves out slots, handles and type registration order. An undo can change it and needn't
    /// restore it: undone rows go last in their table and drawn IDs stay drawn.
    pub layout: u64,
}

/// As recordings and the sandbox's `hashes.txt` write it: content, then layout, 16 hex digits
/// each, such as `8c35cd01914c7df5 fb695ce29daa8479`.
impl fmt::Display for WorldHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x} {:016x}", self.content, self.layout)
    }
}

/// What [`Reflect::digest`] writes a value into, word by word, each mixed into the state.
pub struct Digest<'w> {
    state: u64,
    /// Supplies the IDs of entities a value holds.
    world: &'w World,
    /// Slots whose entities read as dead, as a client's view leaves them out.
    hidden: &'w [bool],
}

impl Digest<'_> {
    pub(crate) fn word(&mut self, word: u64) {
        self.state = absorb(self.state, word);
    }

    pub(crate) fn int(&mut self, n: i64) {
        self.word(n as u64);
    }

    pub(crate) fn bool(&mut self, b: bool) {
        self.word(u64::from(b));
    }

    /// By its bits: every NaN as one, -0.0 apart from 0.0.
    pub(crate) fn f32(&mut self, x: f32) {
        let bits = if x.is_nan() { NAN32 } else { x.to_bits() };
        self.word(u64::from(bits));
    }

    /// As [`f32`](Self::f32).
    pub(crate) fn f64(&mut self, x: f64) {
        self.word(if x.is_nan() { NAN64 } else { x.to_bits() });
    }

    /// Their number, then each 8 of them, little-endian.
    pub(crate) fn bytes(&mut self, bytes: &[u8]) {
        self.state = absorb_bytes(self.state, bytes);
    }

    /// By its ID; 0 if dead or hidden (a client's load of a view without it finds it dead).
    pub(crate) fn entity(&mut self, entity: Entity) {
        let id = self
            .world
            .id(entity)
            .filter(|_| !hidden(self.hidden, entity));
        self.word(id.map_or(0, EntityId::get));
    }

    /// By its name (ID, else path); 0 if none, so a texture file renamed with its ID file hashes
    /// as before.
    pub(crate) fn texture(&mut self, texture: Texture) {
        match self.world.assets().name(texture) {
            Some(name) => self.bytes(name.as_bytes()),
            None => self.word(0),
        }
    }

    /// By its name, as a texture is.
    pub(crate) fn model(&mut self, model: Model) {
        let world = self.world;
        self.bytes(world.assets().model_name(model).as_bytes());
    }

    /// By its name, as a texture is.
    pub(crate) fn font(&mut self, font: Font) {
        let world = self.world;
        self.bytes(world.assets().font_name(font).as_bytes());
    }

    /// By its name, as a texture is.
    pub(crate) fn prefab(&mut self, prefab: Prefab) {
        let world = self.world;
        self.bytes(world.assets().prefab_name(prefab).as_bytes());
    }
}

fn absorb(state: u64, word: u64) -> u64 {
    mix64(state ^ word)
}

fn absorb_bytes(mut state: u64, bytes: &[u8]) -> u64 {
    state = absorb(state, bytes.len() as u64);
    for chunk in bytes.chunks(8) {
        let mut word = [0; 8];
        word[..chunk.len()].copy_from_slice(chunk);
        state = absorb(state, u64::from_le_bytes(word));
    }
    state
}

/// The key a component type is hashed under: from its name, so it's the same in every build and
/// world, whatever order types registered in.
pub(super) fn type_key(name: &str) -> u64 {
    absorb_bytes(TYPE, name.as_bytes())
}

/// The digest of a cell's kept scene text, which the hash reads in its place (see
/// [`WorldHash::layout`]); platform-independent.
pub(crate) fn text_digest(text: &str) -> u64 {
    bytes_digest(text.as_bytes())
}

/// The digest of `bytes`, like [`text_digest`]. A session's hello carries the executable's or
/// package's, so peers of other builds don't join.
pub(crate) fn bytes_digest(bytes: &[u8]) -> u64 {
    absorb_bytes(TEXT, bytes)
}

/// A [`CellState`], absorbed; each list in order after its count: cells in, coming, with files
/// (plus file digest), came as their files have them (plus digest of what they came as), out and
/// keeping something (plus texts' digests), refused.
fn cells(cells: &CellState) -> u64 {
    let cell = |state: u64, cell: &Cell| {
        let [x, y, z] = cell.0.to_array().map(|axis| u64::from(axis as u32));
        absorb(absorb(absorb(state, x), y), z)
    };
    let mut state = absorb(CELLS, cells.cells_in.len() as u64);
    state = cells.cells_in.iter().fold(state, cell);
    state = absorb(state, cells.coming.len() as u64);
    state = cells.coming.iter().fold(state, cell);
    for digests in [&cells.files, &cells.came_as] {
        state = absorb(state, digests.len() as u64);
        for (at, &digest) in digests {
            state = absorb(cell(state, at), digest);
        }
    }
    state = absorb(state, cells.kept.len() as u64);
    for (at, kept) in &cells.kept {
        state = cell(state, at);
        state = absorb(state, kept.texts.len() as u64);
        state = kept
            .texts
            .iter()
            .fold(state, |state, text| absorb(state, text.digest));
    }
    state = absorb(state, cells.refused.len() as u64);
    cells.refused.iter().fold(state, cell)
}

/// Whether `hidden`, by slot (a client's view's [`Seen::hidden`]), has `entity`; none past its end.
pub(super) fn hidden(hidden: &[bool], entity: Entity) -> bool {
    hidden.get(entity.slot()).copied().unwrap_or(false)
}

/// The sum of `values`' terms (components of the type keyed `key` on `rows`), skipping `hidden`
/// ones ([`hidden()`]); each from the entity's ID, the key and what the value writes.
pub(super) fn terms<T: Reflect>(
    values: &[T],
    rows: &[Entity],
    key: u64,
    world: &World,
    hidden: &[bool],
) -> u64 {
    values.iter().zip(rows).fold(0, |sum, (value, &entity)| {
        if self::hidden(hidden, entity) {
            return sum;
        }
        sum.wrapping_add(term(entity, key, world, hidden, |to| value.digest(to)))
    })
}

/// The term of `entity`'s component of the type keyed `key`, whose value `digest` writes;
/// `hidden` entities read as dead.
pub(super) fn term(
    entity: Entity,
    key: u64,
    world: &World,
    hidden: &[bool],
    digest: impl FnOnce(&mut Digest<'_>),
) -> u64 {
    let id = world.id(entity).expect("a table's rows are alive");
    let state = absorb(absorb(TERM, id.get()), key);
    let mut to = Digest {
        state,
        world,
        hidden,
    };
    digest(&mut to);
    to.state
}

impl World {
    /// The world's hash, compared at each tick by a run that must match another. Doesn't
    /// allocate.
    pub fn hash(&self) -> WorldHash {
        self.hash_seen(None)
    }

    /// The hash of what [`save_seen`](Self::save_seen) writes for `seen`, as loaded. Doesn't
    /// allocate. Like [`hash`](Self::hash), but hidden entities' rows are skipped, values naming
    /// them read as dead, and `seen`'s cell state replaces the world's. `None` sees everything.
    pub(crate) fn hash_seen(&self, seen: Option<&Seen>) -> WorldHash {
        let _zone = gg_core::zone!();
        let hidden = seen.map_or(&[][..], |seen| &seen.hidden[..]);
        let shown = |entity: &&Entity| !self::hidden(hidden, **entity);
        let id = |entity| self.id(entity).map_or(0, EntityId::get);
        let mut content = 0u64;
        let mut layout = absorb(LAYOUT, self.tables.len() as u64);
        for table in &self.tables {
            // Its types as a set, by key: the registration order that sorts its columns doesn't
            // change it.
            let mut types = 0u64;
            for (&component, column) in table.components().iter().zip(table.columns()) {
                let key = self.components.key(component);
                types = types.wrapping_add(key);
                let sum = column.digest(table.entities(), key, self, hidden);
                content = content.wrapping_add(sum);
            }
            layout = absorb(layout, types);
            let rows = table.entities().iter().filter(shown);
            layout = absorb(layout, rows.clone().count() as u64);
            for &entity in rows {
                layout = absorb(layout, id(entity));
            }
        }
        // A view hides an entity with what's under it, so a parent shown has its children.
        let entities = self.tables.iter().flat_map(|table| table.entities());
        for &entity in entities.filter(shown) {
            let children = self.children(entity).expect("a table's rows are alive");
            let mut children = children.peekable();
            if children.peek().is_none() {
                continue;
            }
            layout = absorb(layout, id(entity));
            let mut count = 0u64;
            for child in children {
                layout = absorb(layout, id(child));
                count += 1;
            }
            layout = absorb(layout, count);
        }
        layout = absorb(layout, self.stream.state());
        let state = self.resource::<CellState>();
        let state = match seen {
            Some(seen) => state.map(|_| &seen.cells),
            None => state,
        };
        // A world whose cells never came in hashes as it did before worlds had cells.
        if let Some(state) = state.filter(|state| !state.is_empty()) {
            layout = absorb(layout, cells(state));
        }
        WorldHash { content, layout }
    }
}

#[cfg(test)]
mod tests {
    use gg_core::math::{Quat, Vec3};
    use gg_renderer::{Mipmaps, Textures};

    use super::*;
    use crate::ecs::Writable;
    use crate::ecs::ops::tests::{Fixture, Sealed, fixture};
    use crate::reflect::tests::{Everything, Form, Handle, everything};
    use crate::{Sampler, Sprite, Transform};

    /// Values a hash tells apart only by where one field ends and the next begins.
    #[derive(Clone, Default, crate::Component)]
    struct Neighbors {
        options: [Option<u8>; 2],
        lists: [Vec<u8>; 2],
        texts: [String; 2],
    }

    /// What's changed, and how.
    type Change<T> = (&'static str, fn(&mut T));

    /// Two types whose values write the same words.
    #[derive(Clone, Copy, crate::Component)]
    struct Left(u32);

    #[derive(Clone, Copy, crate::Component)]
    struct Right(u32);

    /// A component as it was before fields were added to it.
    #[derive(Clone, Copy, crate::Component)]
    #[reflect(name = "Grown")]
    struct Before {
        size: f32,
    }

    /// The same component, with fields added that the hash reads only while they're set.
    #[derive(Clone, Copy, crate::Component)]
    #[reflect(name = "Grown")]
    struct After {
        size: f32,
        #[reflect(default = false, hashed_when_set)]
        on: bool,
        #[reflect(default = u32::MAX, hashed_when_set)]
        mask: u32,
        #[reflect(default = u32::MAX, hashed_when_set)]
        other: u32,
    }

    /// Fields marked `hashed_when_set` leave a component's hash as it was before they were added
    /// while they hold their defaults, and change it once set, each apart from the others.
    #[test]
    fn fields_hashed_when_set_leave_the_hash_as_it_was_until_set() {
        let hashed = |spawn: &dyn Fn(&mut World)| {
            let mut world = World::seeded(5);
            spawn(&mut world);
            world.hash().content
        };
        let before = hashed(&|world| {
            world.spawn(Before { size: 2.0 });
        });
        let after = |on, mask, other| {
            hashed(&move |world| {
                world.spawn(After {
                    size: 2.0,
                    on,
                    mask,
                    other,
                });
            })
        };
        assert_eq!(after(false, u32::MAX, u32::MAX), before);
        let set = [
            after(true, u32::MAX, u32::MAX),
            after(false, 3, u32::MAX),
            after(false, u32::MAX, 3),
        ];
        for (i, one) in set.iter().enumerate() {
            assert_ne!(*one, before, "{i}");
            for other in &set[i + 1..] {
                assert_ne!(one, other, "{i}");
            }
        }
    }

    /// Where entities are held changes the layout, not the content. A scene loaded into other
    /// worlds keeps its content and has one layout whatever the type registration order. In the
    /// source world, each of a row moved to its table's end, a table made in passing, a child
    /// made its parent's last, an ID drawn and empty tables of other types changes only the
    /// layout.
    #[test]
    fn content_ignores_where_entities_are_and_layout_follows_it() {
        let Fixture {
            mut world,
            r,
            a,
            s,
            b,
            c,
            f,
            g,
            ..
        } = fixture();
        // Opaque, so a scene doesn't save it.
        world.despawn(g).expect("alive");
        let content = world.hash().content;
        let text = world.save_scene(&[r, f]).expect("saved");
        let loaded = |transform_first: bool| {
            let mut loaded = World::seeded(1);
            // Names the sprite's texture.
            loaded.assets = world.assets.clone();
            let registered = if transform_first {
                [
                    loaded.register::<Transform>(),
                    loaded.register::<Sprite>(),
                    loaded.register::<Everything>(),
                ]
            } else {
                [
                    loaded.register::<Everything>(),
                    loaded.register::<Sprite>(),
                    loaded.register::<Transform>(),
                ]
            };
            registered
                .into_iter()
                .for_each(|done| done.expect("registered"));
            loaded.load_scene(&text).expect("loaded");
            loaded.hash()
        };
        let hash = loaded(true);
        assert_eq!(hash, loaded(false), "the order types registered in");
        assert_eq!(hash.content, content, "loaded");

        let mut layout = world.hash().layout;
        let mut held_otherwise = |world: &World, what: &str| {
            let hash = world.hash();
            assert_eq!(hash.content, content, "{what}");
            assert_ne!(hash.layout, layout, "{what}");
            layout = hash.layout;
        };
        let through_sealed = |world: &mut World, entity| {
            world.insert(entity, Sealed(1)).expect("alive");
            world.remove::<Sealed>(entity).expect("alive");
        };
        // Through a new table, back to the end of its own.
        through_sealed(&mut world, s);
        held_otherwise(&world, "s through a table of its own");
        through_sealed(&mut world, c);
        held_otherwise(&world, "c moved to the end of its table");
        // Alone in its table, so only the table it passed through is new.
        through_sealed(&mut world, f);
        held_otherwise(&world, "a table made in passing");
        world.set_parent(b, Some(s)).expect("no cycle");
        world.set_parent(b, Some(a)).expect("no cycle");
        held_otherwise(&world, "b made a's last child");
        let spare = world.spawn(Transform::IDENTITY);
        world.despawn(spare).expect("alive");
        held_otherwise(&world, "an ID drawn");

        // Tables told apart by type alone, left empty by a spawn and despawn: queries search
        // them in another order.
        let emptied = |spawn: fn(&mut World) -> Entity| {
            let mut world = World::seeded(2);
            let entity = spawn(&mut world);
            world.despawn(entity).expect("alive");
            world.hash()
        };
        let one = emptied(|world| world.spawn(Sealed(1)));
        let other = emptied(|world| world.spawn(Transform::IDENTITY));
        assert_eq!(one.content, other.content, "empty tables");
        assert_ne!(one.layout, other.layout, "empty tables of other types");
    }

    /// The content hash with `entity`'s `T` changed by `change`, which is then set back as it was.
    fn changed<T: Writable + Clone>(
        world: &mut World,
        entity: Entity,
        change: impl FnOnce(&mut T),
    ) -> u64 {
        let kept = world.get::<T>(entity).expect("one").clone();
        change(world.get_mut(entity).expect("one"));
        let content = world.hash().content;
        *world.get_mut(entity).expect("one") = kept;
        content
    }

    /// Every field reflection reads changes the content hash (and setting it back restores it);
    /// opaque ones don't. Covers each value kind, variants with opaque fields, values moved
    /// between fields, same words of another type, NaNs of any sign and payload (one hash), -0.0
    /// (not 0.0), and an entity held after its slot was reused (hashes as dead).
    #[test]
    fn every_field_reflection_reads_changes_the_content_hash() {
        let mut world = World::seeded(7);
        let other = world.spawn(Transform::IDENTITY);
        let stale = world.spawn(());
        world.despawn(stale).expect("alive");
        // In `stale`'s slot, the last freed.
        world.spawn(Transform::IDENTITY);
        let all = world.spawn(Everything {
            form: Form::Circle(1.0),
            ..everything()
        });
        let posed = world.spawn((Transform::IDENTITY, Sprite::default()));
        let neighbors = world.spawn(Neighbors::default());
        // Last, so that its slot stays free.
        let dead = world.spawn(());
        world.despawn(dead).expect("alive");
        let start = world.hash().content;

        let fields: [Change<Everything>; 17] = [
            ("on", |e| e.on = false),
            ("count", |e| e.count = 43),
            ("byte", |e| e.byte = 8),
            ("weight", |e| e.weight = 2.75),
            ("angle", |e| e.angle = 0.25),
            ("tint", |e| e.tint.w = 0.5),
            ("offset, its -0.0", |e| e.offset.y = 0.0),
            ("turn", |e| e.turn = Quat::from_rotation_x(0.5)),
            ("inner.level", |e| e.inner.level = 0),
            ("inner.name", |e| e.inner.name.push('!')),
            ("form's field", |e| e.form = Form::Circle(2.0)),
            ("form's variant", |e| e.form = Form::Held(Handle(1))),
            ("maybe", |e| e.maybe = Some(e.inner.clone())),
            ("list", |e| e.list.push(3)),
            ("rect", |e| e.rect[1].y = 2.0),
            ("pair.0", |e| e.pair.0 = 4),
            ("pair.1", |e| e.pair.1 = -5),
        ];
        for (field, change) in fields {
            assert_ne!(changed(&mut world, all, change), start, "{field}");
            assert_eq!(world.hash().content, start, "{field} set back");
        }
        let sprites: [Change<Sprite>; 3] = [
            ("color", |sprite| sprite.color.w = 0.5),
            ("rect", |sprite| sprite.rect[0].x = 0.5),
            ("sampler", |sprite| sprite.sampler = Sampler::NearestRepeat),
        ];
        for (field, change) in sprites {
            assert_ne!(
                changed(&mut world, posed, change),
                start,
                "the sprite's {field}"
            );
        }
        let transforms: [Change<Transform>; 3] = [
            ("translation", |at| at.translation.z = 1.0),
            ("rotation", |at| at.rotation = Quat::from_rotation_z(1.0)),
            ("scale", |at| at.scale = Vec3::splat(2.0)),
        ];
        for (field, change) in transforms {
            assert_ne!(changed(&mut world, posed, change), start, "the {field}");
        }

        let handle = changed(&mut world, all, |e: &mut Everything| e.handle = Handle(8));
        assert_eq!(handle, start, "an opaque field");
        let held = |world: &mut World, handle| {
            changed(world, all, move |e: &mut Everything| {
                e.form = Form::Held(handle)
            })
        };
        let first = held(&mut world, Handle(1));
        assert_eq!(
            first,
            held(&mut world, Handle(2)),
            "an opaque field of a variant"
        );
        // Neither writes a field: only their indices tell them apart.
        let point = changed(&mut world, all, |e: &mut Everything| e.form = Form::Point);
        assert_ne!(point, first, "a variant, by its index");
        let texture = Textures::default().add("own", [1, 1], &[255, 0, 0, 255], Mipmaps::None);
        let texture = texture.expect("a texture");
        world
            .assets_mut()
            .add_texture(texture, "own.png".into(), "own.png".into());
        let textured = changed(&mut world, posed, |sprite: &mut Sprite| {
            sprite.texture = Some(texture);
        });
        assert_ne!(textured, start, "a sprite's texture, by its name");

        let target = |world: &mut World, target: Option<Entity>| {
            changed(world, all, move |e: &mut Everything| e.target = target)
        };
        assert_ne!(target(&mut world, Some(other)), start, "a target");
        let dead_target = target(&mut world, Some(dead));
        assert_ne!(dead_target, start, "a dead target, which isn't none");
        assert_eq!(
            target(&mut world, Some(stale)),
            dead_target,
            "a target whose slot was taken again"
        );

        let angle = |world: &mut World, bits: u32| {
            changed(world, all, move |e: &mut Everything| {
                e.angle = f32::from_bits(bits)
            })
        };
        let weight = |world: &mut World, bits: u64| {
            changed(world, all, move |e: &mut Everything| {
                e.weight = f64::from_bits(bits)
            })
        };
        let nans = [0x7fc0_0000, 0xffc0_0000, 0x7f80_0001, 0xffc1_2345]
            .map(|bits| angle(&mut world, bits));
        assert!(nans.iter().all(|&nan| nan == nans[0]), "f32 NaNs");
        let nans = [
            0x7ff8_0000_0000_0000,
            0xfff8_0000_0000_0000,
            0x7ff0_0000_0000_0001,
            0xfff8_1234_5678_9abc,
        ]
        .map(|bits| weight(&mut world, bits));
        assert!(nans.iter().all(|&nan| nan == nans[0]), "f64 NaNs");

        let moved = |world: &mut World, to: Neighbors| {
            changed(world, neighbors, move |n: &mut Neighbors| *n = to)
        };
        let text = |text: &str| text.to_owned();
        let pairs = [
            (
                Neighbors {
                    options: [Some(0), None],
                    ..Neighbors::default()
                },
                Neighbors {
                    options: [None, Some(0)],
                    ..Neighbors::default()
                },
            ),
            (
                Neighbors {
                    lists: [vec![1], vec![2, 3]],
                    ..Neighbors::default()
                },
                Neighbors {
                    lists: [vec![1, 2], vec![3]],
                    ..Neighbors::default()
                },
            ),
            (
                Neighbors {
                    texts: [text(""), text("a")],
                    ..Neighbors::default()
                },
                Neighbors {
                    texts: [text("a"), text("")],
                    ..Neighbors::default()
                },
            ),
        ];
        for (one, other) in pairs {
            assert_ne!(moved(&mut world, one), moved(&mut world, other));
        }

        // On an entity of the same ID.
        let spawned = |spawn: fn(&mut World) -> Entity| {
            let mut world = World::seeded(3);
            spawn(&mut world);
            world.hash().content
        };
        let left = spawned(|world| world.spawn(Left(1)));
        assert_ne!(left, spawned(|world| world.spawn(Right(1))), "another type");
    }
}

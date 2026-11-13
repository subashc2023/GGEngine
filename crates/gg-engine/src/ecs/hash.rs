//! The world's hash, which two runs of a simulation compare at each tick to find where they part:
//! its content, the entities' components keyed by their IDs, which undoing a log gives back; and
//! its layout, how the world holds them, which the order queries find entities in follows from.

use std::fmt;

use super::id::mix64;
use super::{Entity, EntityId, World};
use crate::cells::{Cell, CellState, Seen};
use crate::reflect::Reflect;
use crate::{Font, Model, Prefab, Texture};

// What a type's key, a term of the content and the layout start from: the 64-bit words of π's
// fraction after the ID stream's (id.rs), as constants that hide nothing, none of them the zero
// that `mix64` keeps.
const TYPE: u64 = 0xa409_3822_299f_31d0;
const LAYOUT: u64 = 0x082e_fa98_ec4e_6c89;
const TERM: u64 = 0x4528_21e6_38d0_1377;
const CELLS: u64 = 0xbe54_66cf_34e9_0c6c;
const TEXT: u64 = 0xc0ac_29b7_c97c_50dd;

// What every NaN hashes as, since arithmetic leaves a NaN's sign and payload to the platform, and
// x86's and ARM's differ. Literals, since std doesn't promise `f32::NAN`'s bits.
const NAN32: u32 = 0x7fc0_0000;
const NAN64: u64 = 0x7ff8_0000_0000_0000;

/// A world's hash, which two runs of a simulation that go the same way have the same of at every
/// tick: what the world holds, and how.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldHash {
    /// The entities' components, each under its entity's ID and its type's name, and summed, so
    /// that where the world holds them doesn't count: what reflection reads of them, but for what's
    /// opaque, with an entity they hold as its ID, or 0 if it's dead. Undoing a log gives it back.
    /// It leaves out entities without components, which the layout has, slots and handles, and the
    /// order types registered in.
    pub content: u64,
    /// How the world holds its entities, which the order queries find them in follows from: its
    /// tables, in the order they were made, each with its types and its rows' IDs in order; each
    /// parent's children's IDs, in their order; the state of the stream it draws IDs from; and in
    /// a world of [cells](crate::Cells), those in, those refused, and what each cell out keeps of
    /// its entities as text, which a world whose cells never came in leaves out. It leaves out
    /// slots and handles, and the order types registered in. An undo can change it, and needn't
    /// give it back: what an undo puts back goes last in its table, and the IDs a create drew stay
    /// drawn.
    pub layout: u64,
}

/// The hash as recordings and the sandbox's `hashes.txt` write it: its content, then its layout,
/// 16 hex digits each, such as `8c35cd01914c7df5 fb695ce29daa8479`.
impl fmt::Display for WorldHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x} {:016x}", self.content, self.layout)
    }
}

/// What a value is written into for the world's hash, by [`Reflect::digest`]: word by word, each
/// mixed into what came before, so that values whose words differ anywhere give other hashes.
pub struct Digest<'w> {
    state: u64,
    /// Whose IDs an entity a value holds is written by.
    world: &'w World,
    /// The slots whose entities read as dead, as a client's view leaves them out, as `hidden`
    /// reads them.
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

    /// By its bits, every NaN as one, and -0.0 apart from 0.0, as arithmetic keeps it.
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

    /// By its ID, the world's handle being its own, or 0 if it isn't alive or is hidden, as a
    /// client's load of a view without it finds it dead.
    pub(crate) fn entity(&mut self, entity: Entity) {
        let id = self
            .world
            .id(entity)
            .filter(|_| !hidden(self.hidden, entity));
        self.word(id.map_or(0, EntityId::get));
    }

    /// By its name, its ID or else its path, the renderer's handle being its own, or 0 if it has
    /// none, so that a texture's file renamed with its ID file hashes as it did.
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

/// The key a component type is hashed under: from its name, so that it's the same in every build
/// and every world, whatever order types registered in.
pub(super) fn type_key(name: &str) -> u64 {
    absorb_bytes(TYPE, name.as_bytes())
}

/// The digest of `text`, as of a scene text a cell keeps, which the world's hash reads in its place,
/// as [`WorldHash::layout`] says: the same of the same bytes on every platform.
pub(crate) fn text_digest(text: &str) -> u64 {
    absorb_bytes(TEXT, text.as_bytes())
}

/// What a world of cells keeps of them, as [`CellState`] holds it, absorbed: the cells in, those
/// coming, those whose files came, each with its file's digest, those out that keep something,
/// each with its texts' digests, and those refused, each in order, after its count.
fn cells(cells: &CellState) -> u64 {
    let cell = |state: u64, cell: &Cell| {
        let [x, y, z] = cell.0.to_array().map(|axis| u64::from(axis as u32));
        absorb(absorb(absorb(state, x), y), z)
    };
    let mut state = absorb(CELLS, cells.cells_in.len() as u64);
    state = cells.cells_in.iter().fold(state, cell);
    state = absorb(state, cells.coming.len() as u64);
    state = cells.coming.iter().fold(state, cell);
    state = absorb(state, cells.files.len() as u64);
    for (at, &digest) in &cells.files {
        state = absorb(cell(state, at), digest);
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

/// Whether `entity` is hidden, as `hidden`, by slot, has it, a client's view's [`Seen::hidden`];
/// none past its end.
pub(super) fn hidden(hidden: &[bool], entity: Entity) -> bool {
    hidden.get(entity.slot()).copied().unwrap_or(false)
}

/// The sum of the terms of `values`, components of the type keyed `key`, which are `rows`'s, but
/// for those `hidden` has, as [`hidden()`] reads it: each from its entity's ID, the key and what the
/// value writes.
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

/// The term of the world's hash of `entity`'s component of the type whose key is `key`, whose
/// value `digest` writes, as [`Reflect::digest`] does, the entities `hidden` has reading as dead.
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
    /// The world's hash, which a run that must go as another did compares at each tick: reading
    /// every component, without allocating.
    pub fn hash(&self) -> WorldHash {
        self.hash_seen(None)
    }

    /// The hash of the world that [`save_seen`](Self::save_seen) writes of what `seen` sees, as
    /// its load gives it, without allocating: as [`hash`](Self::hash), but for the entities it
    /// hides, whose rows it skips and which a value naming one reads as dead, and with its cells'
    /// state in place of the world's, in a world that has one. `None` is the world whole.
    pub(crate) fn hash_seen(&self, seen: Option<&Seen>) -> WorldHash {
        let _zone = gg_core::zone!();
        let hidden = seen.map_or(&[][..], |seen| &seen.hidden[..]);
        let shown = |entity: &&Entity| !self::hidden(hidden, **entity);
        let id = |entity| self.id(entity).map_or(0, EntityId::get);
        let mut content = 0u64;
        let mut layout = absorb(LAYOUT, self.tables.len() as u64);
        for table in &self.tables {
            // Its types as a set, by their keys, which the order they registered in, which sorts
            // its columns, doesn't change.
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
        // A view hides an entity with what's under it, so that a parent shown has its children.
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
        // So that a world whose cells never came in hashes as it did before worlds had cells.
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
    use crate::ecs::ops::tests::{Fixture, Sealed, fixture};
    use crate::reflect::tests::{Everything, Form, Handle, everything};
    use crate::{Sampler, Sprite, Transform, Writable};

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
    /// while they hold their defaults, and change it once set, each apart from the others, though
    /// two set alike write the same words.
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

    /// Where the world holds its entities changes its layout, not its content: a scene's entities
    /// loaded into other worlds have the content they had, and the same layout as each other
    /// whatever order their types registered in; in the world they came from, a row moved to the
    /// end of its table, a table made in passing, a child made its parent's last, and an ID drawn
    /// each change the layout alone; and so do empty tables of other types.
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
        // Opaque whole, which a scene doesn't save.
        world.despawn(g).expect("alive");
        let content = world.hash().content;
        let text = world.save_scene(&[r, f]).expect("saved");
        let loaded = |transform_first: bool| {
            let mut loaded = World::seeded(1);
            // Its context's, which names the sprite's texture.
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
        // Onto a table of its own, made in passing, and back to the end of its own.
        through_sealed(&mut world, s);
        held_otherwise(&world, "s through a table of its own");
        through_sealed(&mut world, c);
        held_otherwise(&world, "c moved to the end of its table");
        // Alone in its table, so that only the table it passed through is new.
        through_sealed(&mut world, f);
        held_otherwise(&world, "a table made in passing");
        world.set_parent(b, Some(s)).expect("no cycle");
        world.set_parent(b, Some(a)).expect("no cycle");
        held_otherwise(&world, "b made a's last child");
        let spare = world.spawn(Transform::IDENTITY);
        world.despawn(spare).expect("alive");
        held_otherwise(&world, "an ID drawn");

        // Tables told apart by their types alone, each left empty by an entity spawned and
        // despawned: queries will search them in another order.
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

    /// Every field reflection reads reaches the content hash, which setting it back gives back,
    /// and what's opaque doesn't: each kind of value, a variant whose fields are opaque, and the
    /// fields of a variant; values moved from one field to the next; the same words of another
    /// type; NaNs of every sign and payload, which are one, and -0.0, which isn't 0.0; and an
    /// entity held after its slot was taken again, which hashes as one dead in a free slot does.
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
        // Neither writes a field, so that only their indices tell them apart.
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

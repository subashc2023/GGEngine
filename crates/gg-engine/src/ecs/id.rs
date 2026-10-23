//! Persistent IDs: what files, operations, diagnostics and the tool API name an entity by, where
//! its handle is the world's own. Each world draws them from a stream of its own, and finds its
//! entities by them.

use std::fmt;
use std::num::NonZeroU64;

use super::{Entity, World};

/// An entity's persistent ID: what files, operations, diagnostics and the tool API name it by,
/// where its [`Entity`] handle is the world's own. A world draws one at random for each entity it
/// spawns, and never draws the same one twice. It's written as 16 lowercase hex digits, such as
/// `5f3a9c0e12b47d68`, so that IDs sort as their text does and never read as a handle.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntityId(NonZeroU64);

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

impl fmt::Debug for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// What SplitMix64's state steps by.
const GAMMA: u64 = 0x9e37_79b9_7f4a_7c15;

/// Steps `state` and returns the next number of its stream: Sebastiano Vigna's SplitMix64
/// (<https://prng.di.unimi.it/splitmix64.c>). The state steps by an odd constant, so that it
/// passes through all 2^64 values before it repeats one, and the mix that makes each number of it,
/// of xorshifts and multiplications by odd constants, is a bijection, so a stream gives 2^64
/// different numbers before it repeats. The mix can be undone, so one number gives away the rest of
/// its stream: they're not secrets.
pub(super) fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(GAMMA);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The stream a world draws its entities' IDs from: SplitMix64, from a seed. A stream never gives
/// an ID twice. Two worlds' streams, seeded apart, are windows on the one cycle of 2^64 states,
/// which give the same IDs only where they overlap: for n and m IDs, about (n + m) / 2^64.
pub(super) struct IdStream(u64);

impl IdStream {
    pub(super) fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// The next ID: the stream's next number, past the one zero among them.
    pub(super) fn draw(&mut self) -> EntityId {
        loop {
            if let Some(id) = NonZeroU64::new(splitmix64(&mut self.0)) {
                return EntityId(id);
            }
        }
    }
}

impl World {
    /// `entity`'s persistent ID, the same from its spawn to its despawn, through every table it
    /// moves to; `None` if it isn't alive. It's read through the handle, checked as any handle is,
    /// so that a stale handle never gives the ID of the entity now in its slot.
    pub fn id(&self, entity: Entity) -> Option<EntityId> {
        self.location(entity)?;
        self.entity_ids[entity.index as usize]
    }

    /// The living entity whose ID is `id`; `None` if none has it.
    ///
    /// ```
    /// use gg_engine::{Transform, World};
    ///
    /// let mut world = World::default();
    /// let entity = world.spawn(Transform::IDENTITY);
    /// let id = world.id(entity).expect("alive");
    /// assert_eq!(world.entity(id), Some(entity));
    /// world.despawn(entity)?;
    /// assert_eq!(world.id(entity), None);
    /// assert_eq!(world.entity(id), None);
    /// # Ok::<(), gg_engine::reflect::Error>(())
    /// ```
    pub fn entity(&self, id: EntityId) -> Option<Entity> {
        self.by_id.get(&id).copied()
    }

    /// How diagnostics name `entity`: by its ID while it's alive, and by its handle once it isn't,
    /// since then it has none.
    pub(super) fn name_of(&self, entity: Entity) -> Named {
        match self.id(entity) {
            Some(id) => Named::Id(id),
            None => Named::Handle(entity),
        }
    }
}

/// An entity as diagnostics name it, by its ID, or by its handle if it has none.
#[derive(Clone, Copy)]
pub(super) enum Named {
    Id(EntityId),
    Handle(Entity),
}

impl fmt::Display for Named {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Id(id) => id.fmt(f),
            Self::Handle(entity) => entity.fmt(f),
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// Panics unless the world's IDs agree with its slots: each slot has an ID exactly while its
    /// entity is alive, by which the world finds that entity, and it finds no more entities by ID
    /// than are alive.
    pub(in crate::ecs) fn check_ids(world: &World) {
        assert_eq!(
            world.entity_ids.len(),
            world.meta.len(),
            "an ID for each slot"
        );
        let mut alive = 0;
        for (index, (meta, id)) in world.meta.iter().zip(&world.entity_ids).enumerate() {
            assert_eq!(id.is_some(), meta.location.is_some(), "slot {index}'s ID");
            if let Some(id) = *id {
                let entity = Entity {
                    index: index as u32,
                    generation: meta.generation,
                };
                assert_eq!(world.entity(id), Some(entity), "slot {index}'s ID {id}");
                alive += 1;
            }
        }
        assert_eq!(world.by_id.len(), alive, "the entities found by ID");
    }

    /// SplitMix64's numbers from a seed of 0, as Vigna's C gives them, so that every platform draws
    /// the same IDs from a seed, as replays will need.
    #[test]
    fn a_stream_draws_splitmix64s_numbers() {
        let mut stream = IdStream::new(0);
        let drawn = [0; 3].map(|_| stream.draw().to_string());
        assert_eq!(
            drawn,
            ["e220a8397b1dcdaf", "6e789e6aa1b965f4", "06c45d188009454f"]
        );
    }

    #[test]
    fn a_stream_draws_past_zero() {
        // Whose first state is 0, which mixes to 0.
        let mut stream = IdStream::new(0u64.wrapping_sub(GAMMA));
        assert_eq!(stream.draw().to_string(), "e220a8397b1dcdaf");
    }

    #[test]
    fn ids_are_written_as_16_hex_digits_which_sort_as_they_do() {
        let id = |n| EntityId(NonZeroU64::new(n).expect("not zero"));
        assert_eq!(id(1).to_string(), "0000000000000001");
        assert_eq!(format!("{:?}", id(0xab)), "00000000000000ab");
        assert_eq!(id(u64::MAX).to_string(), "ffffffffffffffff");
        let mut stream = IdStream::new(7);
        let mut by_value: Vec<EntityId> = (0..1000).map(|_| stream.draw()).collect();
        let mut by_text: Vec<String> = by_value.iter().map(ToString::to_string).collect();
        by_value.sort();
        by_text.sort();
        let sorted: Vec<String> = by_value.iter().map(ToString::to_string).collect();
        assert_eq!(sorted, by_text);
    }

    #[test]
    fn each_world_draws_its_own_ids() {
        let [a, b] = [0, 1].map(|_| {
            let mut world = World::default();
            let entity = world.spawn(());
            world.id(entity).expect("alive")
        });
        assert_ne!(a, b);
    }
}

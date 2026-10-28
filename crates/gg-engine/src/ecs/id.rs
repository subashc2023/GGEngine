//! Persistent IDs: what files, operations, diagnostics and the tool API name an entity by, where
//! its handle is the world's own. Each world draws them from a stream of its own, and finds its
//! entities by them.

use std::fmt;
use std::num::NonZeroU64;
use std::str::FromStr;

use gg_core::{Error, Result};

use super::{Entity, World};

/// An entity's persistent ID: what files, operations, diagnostics and the tool API name it by,
/// where its [`Entity`] handle is the world's own. A world draws one at random for each entity
/// code spawns, a loaded scene's entities keep theirs, and a placed prefab's are mixed from
/// theirs; no two living entities have the same one. It's written as 16 lowercase hex digits,
/// such as `5f3a9c0e12b47d68`, so that IDs sort as their text does and never read as a handle,
/// and parsed from them alone.
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

impl EntityId {
    /// The ID that's `number`, as [`get`](Self::get) gives it, such as one drawing gave a draw;
    /// `None` for 0, which no entity has.
    pub(crate) fn new(number: u64) -> Option<Self> {
        NonZeroU64::new(number).map(Self)
    }

    /// The number it is.
    pub(crate) fn get(self) -> u64 {
        self.0.get()
    }
}

/// Exactly as `Display` writes one, so that each ID has one text: 16 lowercase hex digits, not all
/// zero.
///
/// Fails with `ecs.bad_id` for any other text.
impl FromStr for EntityId {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        let hex = |b: u8| matches!(b, b'0'..=b'9' | b'a'..=b'f');
        let n = (text.len() == 16 && text.bytes().all(hex))
            .then(|| u64::from_str_radix(text, 16).ok())
            .flatten()
            .and_then(NonZeroU64::new);
        n.map(Self).ok_or_else(|| {
            Error::new(
                "ecs.bad_id",
                format!(
                    "`{text}` isn't an entity's ID: an ID is 16 lowercase hex digits, not all \
                     zero, such as `5f3a9c0e12b47d68`"
                ),
            )
        })
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
    mix64(*state)
}

/// SplitMix64's mix, of xorshifts and multiplications by odd constants: a bijection of the 64-bit
/// numbers, which spreads each bit of `z` over all of them.
pub(super) fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// What a reseed mixes into the stream's state, and what a placement mixes into its instance: each
/// 64 bits of pi's fraction, as constants that hide nothing. Both are fixed, since seeded worlds
/// reseed the same way every run, and linked prefab instances will mix their IDs again.
const RESEED: u64 = 0x243f_6a88_85a3_08d3;
const PLACE: u64 = 0x1319_8a2e_0370_7344;

/// The ID of a prefab's entity `local` in the copy of it placed as `instance`: a permutation of
/// the local IDs for each instance, so that a copy's IDs are as distinct as its prefab's, and a
/// mix that spreads each instance's over the 64-bit numbers, so that copies' IDs meet only by
/// chance. `None` for the one local ID it mixes to zero.
pub(super) fn placed(instance: EntityId, local: EntityId) -> Option<EntityId> {
    let key = mix64(instance.0.get() ^ PLACE);
    NonZeroU64::new(mix64(local.0.get() ^ key)).map(EntityId)
}

/// The stream a world draws its entities' IDs from: SplitMix64, from a seed. A stream never gives
/// an ID twice until it's reseeded, which moves it to another window on its cycle. Two worlds'
/// streams, seeded apart, are windows on the one cycle of 2^64 states, which give the same IDs
/// only where they overlap, as a stream's before and after a reseed do: for n and m IDs, about
/// (n + m) / 2^64.
#[derive(Clone, Copy, Debug, PartialEq)]
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

    /// Moves the stream to another place in its cycle, as one that has walked into another
    /// session's run, drawing IDs that a loaded scene's entities have, does: the same place for
    /// the same state, so that a seeded world stays the same every run.
    pub(super) fn reseed(&mut self) {
        self.0 = mix64(self.0.rotate_left(32) ^ RESEED);
    }

    /// Where it is on its cycle, which the IDs to come follow from.
    pub(super) fn state(self) -> u64 {
        self.0
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

    /// Finds `entity` by `id` from now on, and returns the entity found by `id` before, if any, as
    /// [`HashMap::insert`](std::collections::HashMap::insert) does.
    ///
    /// The map keeps room for twice as many entities as it finds. Once the entries its removals
    /// leave have used up its room, std's map rehashes in place if it's at most half full, and
    /// grows otherwise (hashbrown's `reserve_rehash_inner`), at a time its random keys decide. So
    /// it grows here alone, as the world comes to hold more than half its room, to room for twice
    /// as many, and spawning and despawning at a population the world has held never allocates.
    pub(super) fn find_by_id(&mut self, id: EntityId, entity: Entity) -> Option<Entity> {
        let finding = self.by_id.len() + 1;
        if finding > self.id_room / 2 {
            // Room for `finding + 1` more than the `finding - 1` it holds.
            self.by_id.reserve(finding + 1);
            self.id_room = self.by_id.capacity();
        }
        self.by_id.insert(id, entity)
    }

    /// Where the stream the world draws its IDs from is, which a world seeded with it goes on
    /// from: its seed, until it draws one.
    pub(crate) fn stream_state(&self) -> u64 {
        self.stream.state()
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
    use std::collections::HashSet;
    use std::num::NonZeroU32;

    use super::*;

    /// How many of `world`'s slots are vacant, kept for entities a delete took: neither alive,
    /// free nor retired.
    pub(in crate::ecs) fn vacated(world: &World) -> usize {
        let mut free = vec![false; world.meta.len()];
        for &index in &world.free {
            free[index as usize] = true;
        }
        let slots = world.meta.iter().zip(free);
        slots
            .filter(|(meta, free)| {
                meta.location.is_none() && !free && meta.generation != NonZeroU32::MAX
            })
            .count()
    }

    /// Panics unless the world's IDs agree with its slots: each slot has an ID exactly while its
    /// entity is alive, by which the world finds that entity, it finds no more entities by ID than
    /// are alive, and it keeps as many IDs as it has vacant slots.
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
        let kept: u32 = world.kept.values().sum();
        assert_eq!(
            kept as usize,
            vacated(world),
            "the IDs kept for vacant slots"
        );
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
    fn an_id_is_parsed_from_its_text_alone() {
        let mut stream = IdStream::new(3);
        for _ in 0..100 {
            let id = stream.draw();
            assert_eq!(id.to_string().parse::<EntityId>().ok(), Some(id));
        }
        let parsed: EntityId = "00000000000000ff".parse().expect("an ID");
        assert_eq!(parsed.0.get(), 0xff);
        for text in [
            "",
            "5f3a9c0e12b47d6",
            "5f3a9c0e12b47d680",
            "5F3A9C0E12B47D68",
            "0000000000000000",
            " 5f3a9c0e12b47d6",
            "5f3a9c0e12b47d6g",
            "+f3a9c0e12b47d68",
            "0x3a9c0e12b47d68",
            "5f3a9c0e12b47d6é",
            "\"5f3a9c0e12b47d68\"",
        ] {
            let error = text.parse::<EntityId>().expect_err(text);
            assert_eq!(error.code(), "ecs.bad_id", "{text}");
            let says = format!("`{text}` isn't an entity's ID: an ID is 16 lowercase hex digits");
            assert!(error.to_string().starts_with(&says), "{error}");
        }
    }

    /// Pinned, since seeded worlds reseed the same way every run, and linked prefab instances
    /// will mix their IDs again.
    #[test]
    fn reseeding_and_placing_mix_as_they_always_have() {
        let mut stream = IdStream::new(0);
        stream.reseed();
        let drawn = [0; 2].map(|_| stream.draw().to_string());
        assert_eq!(drawn, ["74d28e025ceaac29", "890710ced7fbc4af"]);
        let id = |n| EntityId(NonZeroU64::new(n).expect("not zero"));
        let mixed = [(1, 1), (1, 2), (2, 1), (u64::MAX, 0x5f3a_9c0e_12b4_7d68)];
        let mixed = mixed.map(|(instance, local)| {
            let placed = placed(id(instance), id(local)).expect("not zero");
            placed.to_string()
        });
        let pinned = [
            "7ef7947dab2f6a6f",
            "c0f6b07d9e7819fb",
            "37275696e7b56e52",
            "776f67f8ae5c03a9",
        ];
        assert_eq!(mixed, pinned);
        // A permutation of the local IDs for each instance.
        let locals: Vec<EntityId> = (1..=1000).map(id).collect();
        let copies: HashSet<EntityId> = locals.iter().filter_map(|&l| placed(id(9), l)).collect();
        assert_eq!(copies.len(), 1000);
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

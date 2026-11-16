//! Contacts: which entities' colliders touch, or overlap where either is a sensor, as each step of
//! physics leaves them, and which started and stopped at that step, ordered by their entities' IDs
//! so that every platform, peer and replay has them alike.
//!
//! They come from rapier's contact and intersection pairs, not its events: an event names
//! colliders, some of them gone by the time it's read.

use std::mem;

use serde::{Deserialize, Serialize};

use super::{Physics2D, Physics3D};
use crate::{Entity, EntityId, World};

/// Two entities whose colliders touch, as the last step of physics left them: one pushing the other,
/// or, where either collider is a sensor, overlapping it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contact {
    /// The two, the one of the lower ID first. Either may have gone since, as an entity that went
    /// is in the event of its contacts stopping.
    pub entities: [Entity; 2],
    /// Whether either collider is a sensor, so that they overlap rather than push. Two entities
    /// whose colliders both touch and overlap, as a body's box and its sensor's circle may, are in
    /// a contact of each kind.
    pub sensor: bool,
}

impl Contact {
    /// The entity `entity` is in contact with, if it's one of the two.
    pub fn other(&self, entity: Entity) -> Option<Entity> {
        match self.entities {
            [a, b] if a == entity => Some(b),
            [a, b] if b == entity => Some(a),
            _ => None,
        }
    }
}

/// A contact that started or stopped at the last step of physics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContactEvent {
    pub contact: Contact,
    /// Whether it started: otherwise it stopped, as its colliders came apart, or one of them, or
    /// its entity, went.
    pub started: bool,
}

/// A contact as physics keeps it: by its entities' IDs, which order it and name it in a snapshot
/// file, with the entities they were as the step found it.
#[derive(Clone, Copy, Debug)]
struct Pair {
    ids: [u64; 2],
    entities: [Entity; 2],
    sensor: bool,
}

impl Pair {
    /// What orders it, and tells it from every other.
    fn key(&self) -> ([u64; 2], bool) {
        (self.ids, self.sensor)
    }

    fn contact(&self) -> Contact {
        Contact {
            entities: self.entities,
            sensor: self.sensor,
        }
    }
}

/// The contacts of a physics resource, 2D's or 3D's: those the last step left, and the events of
/// that step, which snapshots copy, so that a world restored, as a rollback restores it, steps on to
/// the events the world it was taken from had.
#[derive(Default)]
pub(super) struct Contacts {
    /// Sorted by their keys, each once.
    touching: Vec<Pair>,
    /// Each pair, sorted by its key, and whether it started.
    events: Vec<(Pair, bool)>,
    /// The pairs a step finds, in memory kept from one step to the next.
    found: Vec<Pair>,
}

impl Clone for Contacts {
    /// A copy, without `found`'s memory, as no step carries anything in it over.
    fn clone(&self) -> Self {
        Self {
            touching: self.touching.clone(),
            events: self.events.clone(),
            found: Vec::new(),
        }
    }

    /// Copies `source` into the memory this one holds, keeping its own `found`.
    fn clone_from(&mut self, source: &Self) {
        self.touching.clone_from(&source.touching);
        self.events.clone_from(&source.events);
    }
}

impl Contacts {
    /// Takes what a step left touching, each pair of colliders as `found` gives it, by their
    /// entities and whether either is a sensor, in whatever order rapier holds them: sorted by the
    /// entities' IDs, each pair of entities once for each kind, but those of an entity gone or with
    /// itself; and makes the step's events of what started and stopped since the last, in that
    /// order too. It allocates only as there are more contacts than it has held.
    pub(super) fn take(
        &mut self,
        world: &World,
        found: impl Iterator<Item = (Entity, Entity, bool)>,
    ) {
        let _zone = gg_core::zone!();
        let mut now = mem::take(&mut self.found);
        now.clear();
        for (a, b, sensor) in found {
            let (Some(id_a), Some(id_b)) = (world.id(a), world.id(b)) else {
                continue;
            };
            let (id_a, id_b) = (id_a.get(), id_b.get());
            let pair = match id_a.cmp(&id_b) {
                std::cmp::Ordering::Less => Pair {
                    ids: [id_a, id_b],
                    entities: [a, b],
                    sensor,
                },
                std::cmp::Ordering::Greater => Pair {
                    ids: [id_b, id_a],
                    entities: [b, a],
                    sensor,
                },
                std::cmp::Ordering::Equal => continue,
            };
            now.push(pair);
        }
        // Pairs of equal keys are of the same entities, so that which goes first doesn't show.
        now.sort_unstable_by_key(Pair::key);
        now.dedup_by_key(|pair| pair.key());
        self.events.clear();
        let (mut was, mut is) = (self.touching.iter().peekable(), now.iter().peekable());
        loop {
            let event = match (was.peek(), is.peek()) {
                (None, None) => break,
                (Some(a), Some(b)) if a.key() == b.key() => {
                    was.next();
                    is.next();
                    continue;
                }
                (Some(a), Some(b)) if a.key() > b.key() => (**b, true),
                (Some(a), _) => (**a, false),
                (None, Some(b)) => (**b, true),
            };
            if event.1 {
                is.next();
            } else {
                was.next();
            }
            self.events.push(event);
        }
        self.found = mem::replace(&mut self.touching, now);
    }

    /// The contacts the last step left, in their order.
    pub(super) fn touching(&self) -> impl Iterator<Item = Contact> + '_ {
        self.touching.iter().map(Pair::contact)
    }

    /// The contacts that started and stopped at the last step, in their order.
    pub(super) fn events(&self) -> impl Iterator<Item = ContactEvent> + '_ {
        (self.events.iter()).map(|(pair, started)| ContactEvent {
            contact: pair.contact(),
            started: *started,
        })
    }

    /// What a snapshot file holds of them: each by its entities' IDs.
    pub(super) fn save(&self) -> SavedContacts {
        let saved = |pair: &Pair| (pair.ids, pair.sensor);
        SavedContacts {
            touching: self.touching.iter().map(saved).collect(),
            events: (self.events.iter())
                .map(|(pair, started)| (pair.ids, pair.sensor, *started))
                .collect(),
        }
    }

    /// Them as a snapshot file held them, in `world`, an entity gone [`Entity::DEAD`], their order
    /// made again as it may be text from elsewhere.
    pub(super) fn load(saved: SavedContacts, world: &World) -> Self {
        let pair = |ids: [u64; 2], sensor| {
            let entity = |id| {
                let id = EntityId::new(id);
                id.and_then(|id| world.entity(id)).unwrap_or(Entity::DEAD)
            };
            let [a, b] = ids;
            let ids = if a < b { [a, b] } else { [b, a] };
            Pair {
                ids,
                entities: ids.map(entity),
                sensor,
            }
        };
        let mut touching: Vec<_> = (saved.touching.into_iter())
            .map(|(ids, sensor)| pair(ids, sensor))
            .collect();
        touching.sort_unstable_by_key(Pair::key);
        touching.dedup_by_key(|pair| pair.key());
        let events = (saved.events.into_iter())
            .map(|(ids, sensor, started)| (pair(ids, sensor), started))
            .collect();
        Self {
            touching,
            events,
            found: Vec::new(),
        }
    }
}

/// What a snapshot file holds of [`Contacts`]: each contact by its entities' IDs and whether a
/// sensor's, and each event so and whether it started. Text without them reads as none.
#[derive(Default, Serialize, Deserialize)]
pub(super) struct SavedContacts {
    touching: Vec<([u64; 2], bool)>,
    events: Vec<([u64; 2], bool, bool)>,
}

impl World {
    /// The contacts the last step of physics left: each two entities whose colliders touch, or
    /// overlap where either is a sensor, 2D's then 3D's, each sorted by its entities' IDs. Contacts
    /// are rapier's, between a dynamic body and another; a sensor finds kinematic bodies and
    /// characters too, and static ones but from a static body's sensor. A world that has never
    /// stepped physics has none.
    pub fn contacts(&self) -> impl Iterator<Item = Contact> + '_ {
        self.physics_contacts().flat_map(Contacts::touching)
    }

    /// The contacts that started and stopped at the last step of physics, as
    /// [`contacts`](Self::contacts) orders them: so that a system after
    /// [`step_physics`](crate::step_physics), or a script at the next tick, reads them, and a world
    /// restored, as a rollback restores it, has those of the step its snapshot was taken after. A
    /// collider changed, as its component's field set, is made again, but its contacts go on.
    pub fn contact_events(&self) -> impl Iterator<Item = ContactEvent> + '_ {
        self.physics_contacts().flat_map(Contacts::events)
    }

    /// The contacts of 2D physics, then 3D's, of those the world has.
    fn physics_contacts(&self) -> impl Iterator<Item = &Contacts> {
        let two = self
            .resource::<Physics2D>()
            .map(|physics| &physics.contacts);
        let three = self
            .resource::<Physics3D>()
            .map(|physics| &physics.contacts);
        two.into_iter().chain(three)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A step's pairs, in any order, with an entity twice, with itself and gone, are kept once each,
    /// by their IDs, and each started or stopped in that order.
    #[test]
    fn contacts_are_sorted_and_events_follow_them() {
        let mut world = World::seeded(4);
        let [a, b, c] = [(); 3].map(|()| world.spawn(()));
        let gone = world.spawn(());
        world.despawn(gone).expect("alive");
        let id = |entity| world.id(entity).expect("alive").get();
        let mut contacts = Contacts::default();
        contacts.take(
            &world,
            [
                (b, a, false),
                (a, b, false),
                (c, a, true),
                (a, a, false),
                (gone, a, false),
            ]
            .into_iter(),
        );
        let ordered = |x: Entity, y: Entity| if id(x) < id(y) { [x, y] } else { [y, x] };
        let mut want = vec![
            Contact {
                entities: ordered(a, b),
                sensor: false,
            },
            Contact {
                entities: ordered(a, c),
                sensor: true,
            },
        ];
        want.sort_by_key(|contact| (contact.entities.map(id), contact.sensor));
        assert_eq!(contacts.touching().collect::<Vec<_>>(), want);
        let started: Vec<_> = contacts.events().map(|event| event.started).collect();
        assert_eq!(started, [true, true]);
        // a and c apart, b and c touching.
        contacts.take(&world, [(c, b, false), (a, b, false)].into_iter());
        let events: Vec<_> = contacts.events().collect();
        let mut want = vec![
            ContactEvent {
                contact: Contact {
                    entities: ordered(a, c),
                    sensor: true,
                },
                started: false,
            },
            ContactEvent {
                contact: Contact {
                    entities: ordered(b, c),
                    sensor: false,
                },
                started: true,
            },
        ];
        want.sort_by_key(|event| (event.contact.entities.map(id), event.contact.sensor));
        assert_eq!(events, want);
        assert_eq!(want[0].contact.other(a), Some(c));
        assert_eq!(want[0].contact.other(b), None);
        // Saved and loaded, as a snapshot file holds them.
        let loaded = Contacts::load(contacts.save(), &world);
        assert!(loaded.touching().eq(contacts.touching()));
        assert!(loaded.events().eq(contacts.events()));
    }
}

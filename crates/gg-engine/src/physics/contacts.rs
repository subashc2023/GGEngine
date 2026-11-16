//! Contacts: which entities' colliders touch (or overlap, where either is a sensor) after each
//! step, and which started and stopped. Ordered by entity IDs, so every platform, peer and replay
//! has them alike.
//!
//! They come from rapier's contact and intersection pairs, not its events: an event names
//! colliders, some gone by the time it's read.

use std::mem;

use serde::{Deserialize, Serialize};

use super::{Physics2D, Physics3D};
use crate::{Entity, EntityId, World};

/// Two entities whose colliders touch as the last step left them: pushing, or overlapping where
/// either collider is a sensor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contact {
    /// The two, the lower ID first. Either may have gone since: an entity that went is in the
    /// event of its contacts stopping.
    pub entities: [Entity; 2],
    /// Whether either collider is a sensor, so they overlap rather than push. Two entities whose
    /// colliders both touch and overlap, as a body's box and its sensor's circle may, are in a
    /// contact of each kind.
    pub sensor: bool,
}

impl Contact {
    /// The other of the two, if `entity` is one.
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
    /// Whether it started; otherwise it stopped, as its colliders came apart or one of them, or
    /// its entity, went.
    pub started: bool,
}

/// A contact as physics keeps it: its entities' IDs order it and name it in a snapshot file.
#[derive(Clone, Copy, Debug)]
struct Pair {
    ids: [u64; 2],
    entities: [Entity; 2],
    sensor: bool,
}

impl Pair {
    /// What orders it and tells it from every other.
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

/// A physics resource's contacts and the last step's events. Snapshots copy both, so a restored
/// world steps on to the events its original had.
#[derive(Default)]
pub(super) struct Contacts {
    /// Sorted by key, each once.
    touching: Vec<Pair>,
    /// Each pair, sorted by key, and whether it started.
    events: Vec<(Pair, bool)>,
    /// The pairs a step finds; the memory is kept between steps.
    found: Vec<Pair>,
}

impl Clone for Contacts {
    /// Leaves out `found`: no step carries anything in it over.
    fn clone(&self) -> Self {
        Self {
            touching: self.touching.clone(),
            events: self.events.clone(),
            found: Vec::new(),
        }
    }

    /// Keeps this one's own `found`.
    fn clone_from(&mut self, source: &Self) {
        self.touching.clone_from(&source.touching);
        self.events.clone_from(&source.events);
    }
}

impl Contacts {
    /// Takes the pairs `found` gives (entities, sensor flag), in rapier's order, sorted by entity
    /// IDs, once per kind, minus those of an entity gone or with itself. Makes the events of what
    /// started and stopped since the last step, in that order too. Allocates only for more
    /// contacts than it held.
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
        // Equal keys mean the same entities, so which goes first doesn't show.
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

    pub(super) fn touching(&self) -> impl Iterator<Item = Contact> + '_ {
        self.touching.iter().map(Pair::contact)
    }

    pub(super) fn events(&self) -> impl Iterator<Item = ContactEvent> + '_ {
        (self.events.iter()).map(|(pair, started)| ContactEvent {
            contact: pair.contact(),
            started: *started,
        })
    }

    pub(super) fn save(&self) -> SavedContacts {
        let saved = |pair: &Pair| (pair.ids, pair.sensor);
        SavedContacts {
            touching: self.touching.iter().map(saved).collect(),
            events: (self.events.iter())
                .map(|(pair, started)| (pair.ids, pair.sensor, *started))
                .collect(),
        }
    }

    /// From a snapshot file, in `world`: an entity gone is [`Entity::DEAD`]. The order is made
    /// again, as the text may come from elsewhere.
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

/// [`Contacts`] in a snapshot file: each by its entities' IDs and sensor flag, each event with
/// whether it started. Text without them reads as none.
#[derive(Default, Serialize, Deserialize)]
pub(super) struct SavedContacts {
    touching: Vec<([u64; 2], bool)>,
    events: Vec<([u64; 2], bool, bool)>,
}

impl World {
    /// The contacts the last step of physics left: each two entities whose colliders touch (or
    /// overlap, where either is a sensor), 2D's then 3D's, each sorted by entity IDs. Contacts are
    /// rapier's, between a dynamic body and another; a sensor finds kinematic bodies and
    /// characters too, and static ones only from a static body's sensor. A world that never
    /// stepped physics has none.
    pub fn contacts(&self) -> impl Iterator<Item = Contact> + '_ {
        self.physics_contacts().flat_map(Contacts::touching)
    }

    /// The contacts that started and stopped at the last step of physics, ordered as
    /// [`contacts`](Self::contacts) are, for a system after [`step_physics`](crate::step_physics)
    /// or a script at the next tick. A world restored, as by rollback, has those of the step its
    /// snapshot was taken after. A collider changed is made again, but its contacts go on.
    pub fn contact_events(&self) -> impl Iterator<Item = ContactEvent> + '_ {
        self.physics_contacts().flat_map(Contacts::events)
    }

    /// 2D's contacts, then 3D's, of those the world has.
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

    /// A step's pairs, in any order, repeated, with itself or gone, are kept once each by ID, and
    /// each started or stopped in that order.
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
        // Saved and loaded.
        let loaded = Contacts::load(contacts.save(), &world);
        assert!(loaded.touching().eq(contacts.touching()));
        assert!(loaded.events().eq(contacts.events()));
    }
}

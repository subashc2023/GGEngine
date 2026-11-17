//! Scripts: components that act, each for the entity it's on, at its layer's ticks.

use crate::ecs::Writable;
use crate::{Entity, Tick, World};

/// A component that acts: a layer's tick runs it for each entity that has one, with
/// [`run_scripts`].
///
/// It runs among the layer's systems and after its commands, so its input arrives as the rest of
/// the simulation's does, in the components those commands set. Its fields are its state, held by
/// the world: scenes save it, the Inspector and operations edit it, the hash reads it, snapshots
/// copy it. A script keeps nothing elsewhere. It starts from the value code, a scene or an
/// operation gave it; setup needing the world runs in [`tick`](Self::tick), behind a field of its
/// own.
///
/// Scripts run one entity at a time on the tick's thread, since each gets the whole world. A kind
/// of entity a game has thousands of is faster as a system over a query, which
/// [`par_for_each`](crate::ecs::QueryIterMut::par_for_each) splits across the job system's
/// threads.
pub trait Script: Writable {
    /// Runs the script for `entity` at `tick`. It reads and sets its own state through `world`'s
    /// typed calls on `entity`, and can change anything a system can: spawn and despawn entities,
    /// its own and its script included.
    fn tick(entity: Entity, world: &mut World, tick: Tick);
}

/// Runs `S` at `tick`, once for each entity that has one as the pass starts, in query order.
///
/// An entity despawned, or stripped of `S`, by a script before its turn is skipped. One given an
/// `S` during the pass starts at the next tick. A script that replaces the world, or restores it
/// from a copy, ends the pass, since the listed entities are another world's: that world's scripts
/// start at the next tick. The list lives in memory the world keeps, so a pass, and one nested in a
/// script's tick, allocates nothing once the world has listed as many.
pub fn run_scripts<S: Script>(world: &mut World, tick: Tick) {
    let zone = gg_core::zone!();
    // A zone made in a generic function is one for every `S`, which its text tells apart.
    zone.text(std::any::type_name::<S>());
    let mut scripted = world.take_list();
    scripted.extend(world.query::<(Entity, &S)>().map(|(entity, _)| entity));
    let (listed, _) = world.version();
    for &entity in &scripted {
        if world.version().0 != listed {
            break;
        }
        if world.get::<S>(entity).is_some() {
            S::tick(entity, world, tick);
        }
    }
    world.keep_list(scripted);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Component;

    /// The numbers of the scripts that ran, in order.
    #[derive(Clone, Component)]
    struct Ran(Vec<u32>);

    /// Notes its number in the world's `Ran`, then, the first time, may despawn another entity,
    /// strip another's script, give one to an entity without, spawn a scripted entity and despawn
    /// its own; or replace the world with one of `Ran` and `REPLACED`'s numbers.
    #[derive(Clone, Default, Component)]
    struct Noted {
        number: u32,
        despawn: Option<Entity>,
        strip: Option<Entity>,
        give: Option<Entity>,
        spawn: bool,
        leave: bool,
        replace: bool,
    }

    /// The script numbers in the world a script replaces its own with.
    const REPLACED: [u32; 2] = [7, 8];

    impl Script for Noted {
        fn tick(entity: Entity, world: &mut World, _: Tick) {
            let noted = world
                .get::<Self>(entity)
                .expect("run with its script")
                .clone();
            let ran = world.query_mut::<&mut Ran>().next().expect("one");
            ran.0.push(noted.number);
            if noted.replace {
                *world = World::seeded(3);
                world.spawn(Ran(Vec::new()));
                for number in REPLACED {
                    world.spawn(self::noted(number));
                }
                return;
            }
            if let Some(other) = noted.despawn {
                world.despawn(other).expect("alive");
            }
            if let Some(other) = noted.strip {
                world.remove::<Self>(other).expect("alive");
            }
            if let Some(other) = noted.give {
                let given = Self {
                    number: 100 + noted.number,
                    ..Self::default()
                };
                world.insert(other, given).expect("alive");
            }
            if noted.spawn {
                world.spawn(Self {
                    number: 200 + noted.number,
                    ..Self::default()
                });
            }
            if noted.leave {
                world.despawn(entity).expect("alive");
            } else if let Some(noted) = world.get_mut::<Self>(entity) {
                *noted = Self {
                    number: noted.number,
                    ..Self::default()
                };
            }
        }
    }

    /// Runs a tick of `Noted`s and takes the world's `Ran`.
    fn tick(world: &mut World, number: u64) -> Vec<u32> {
        run_scripts::<Noted>(world, Tick { number });
        let ran = world.query_mut::<&mut Ran>().next().expect("one");
        std::mem::take(&mut ran.0)
    }

    fn noted(number: u32) -> Noted {
        Noted {
            number,
            ..Noted::default()
        }
    }

    /// A pass runs each scripted entity once, in query order across tables.
    #[test]
    fn a_pass_runs_each_scripted_entity_once_in_the_order_queries_find_them() {
        let mut world = World::seeded(1);
        world.spawn(Ran(Vec::new()));
        world.spawn(noted(1));
        world.spawn((noted(2), crate::Transform::IDENTITY));
        world.spawn(noted(3));
        // An entity without the script, between them.
        world.spawn(crate::Transform::IDENTITY);
        world.spawn((noted(4), crate::Transform::IDENTITY));
        let order: Vec<u32> = world.query::<&Noted>().map(|noted| noted.number).collect();
        assert_eq!(order, [1, 3, 2, 4], "tables in the order they were made");
        assert_eq!(tick(&mut world, 0), order);
        assert_eq!(tick(&mut world, 1), order);
    }

    /// Entities despawned or stripped before their turn are skipped; ones given the script or
    /// spawned with it start next tick; a script despawning its own entity doesn't stop others.
    #[test]
    fn what_scripts_change_during_a_pass_runs_from_the_next_tick() {
        let mut world = World::seeded(2);
        world.spawn(Ran(Vec::new()));
        let first = world.spawn(noted(1));
        let [second, third, fourth, fifth] = [2, 3, 4, 5].map(|number| world.spawn(noted(number)));
        let bare = world.spawn(crate::Transform::IDENTITY);
        *world.get_mut::<Noted>(first).expect("scripted") = Noted {
            number: 1,
            // Despawning `second` moves the table's last row, `fifth`, into its place, and taking
            // `third`'s script moves `fourth` into its.
            despawn: Some(second),
            strip: Some(third),
            give: Some(bare),
            spawn: true,
            ..Noted::default()
        };
        world.get_mut::<Noted>(fourth).expect("scripted").leave = true;
        assert_eq!(tick(&mut world, 0), [1, 4, 5]);
        assert!(!world.contains(second) && !world.contains(fourth));
        assert!(world.get::<Noted>(third).is_none());
        // The script given to `bare` and the entity spawned with one start now.
        let mut next = tick(&mut world, 1);
        next.sort_unstable();
        assert_eq!(next, [1, 5, 101, 201]);
        assert!(world.contains(fifth));
    }

    /// A script that replaces the world ends the pass, though the new world has slots for the
    /// entities left to run: its scripts start at the next tick.
    #[test]
    fn a_script_that_replaces_the_world_ends_the_pass() {
        let mut world = World::seeded(4);
        world.spawn(Ran(Vec::new()));
        let first = world.spawn(noted(1));
        world.spawn(noted(2));
        world.get_mut::<Noted>(first).expect("scripted").replace = true;
        assert!(tick(&mut world, 0).is_empty(), "the new world's `Ran`");
        assert_eq!(tick(&mut world, 1), REPLACED);
    }
}

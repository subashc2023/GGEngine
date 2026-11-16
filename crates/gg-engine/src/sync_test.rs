//! The SyncTest (`--sync-test`): each tick run again, once it has run, on a copy of the world taken
//! as it started, with the commands each layer's tick applied, and the copy then checked against
//! the world, as rollback needs, which runs ticks again from a snapshot: a tick that goes by
//! anything but the world, its commands and what its layer set up, as a layer's state kept from one
//! tick to the next, or that a copy of the world doesn't hold all of, is named where the two part.
//! The stack runs it, at each tick and each tick a replay runs. What a tick logs, it logs again
//! as the SyncTest runs it again.

use gg_core::{Error, Result};

use crate::layer::Commands;
use crate::recording::text;
use crate::reflect::Value;
use crate::{Tick, World};

/// What a SyncTest keeps of a tick, from its run to its run again.
#[derive(Default)]
pub(crate) struct SyncTest {
    /// The world as the tick started, taken in the memory of the copy of the tick before, which
    /// the tick runs again on.
    world: World,
    /// The commands each layer's tick applied, as values, after the layer's index from the bottom
    /// of the stack.
    commands: Vec<(usize, Value)>,
    /// The first command a layer's tick applied that its queue wouldn't queue again, as one holding
    /// something opaque, which keeps the tick from running again.
    refused: Option<Error>,
    /// How many ticks ran again and left the copy as they left the world.
    matched: u64,
}

impl SyncTest {
    /// Takes a copy of `world`, as the tick about to run finds it, in the memory of the copy it
    /// holds, and forgets the commands of the tick before.
    pub(crate) fn take(&mut self, world: &World) {
        self.world.clone_from(world);
        self.commands.clear();
    }

    /// Keeps the commands that `queue`, the queue of the layer at `layer` from the bottom of the
    /// stack, of type `name`, holds as its tick is about to apply them in `world`; but the first
    /// that its [`check`](Commands::check) says it wouldn't queue again keeps the tick from running
    /// again, as [`refused`](Self::refused) says.
    pub(crate) fn queued(&mut self, layer: usize, name: &str, queue: &dyn Commands, world: &World) {
        for i in 0..queue.len() {
            let value = queue.value(i);
            if self.refused.is_none()
                && let Err(error) = queue.check(&value)
            {
                let mut command = String::new();
                text::write(&mut command, &value, world);
                let message = format!(
                    "layer {layer}, {name}, queued `{command}`, which it wouldn't queue again, so \
                     that its tick can't run again, as rollback would run it: {error}"
                );
                self.refused = Some(Error::new("sync_test.bad_command", message));
            }
            self.commands.push((layer, value));
        }
    }

    /// Fails with `sync_test.bad_command`, naming the layer, the command and why, if a layer's tick
    /// applied a command its queue wouldn't queue again, as [`queued`](Self::queued) says.
    pub(crate) fn refused(&mut self) -> Result<()> {
        self.refused.take().map_or(Ok(()), Err)
    }

    /// Empties `queue`, the queue of the layer at `layer` from the bottom of the stack, and queues
    /// the commands its tick applied again.
    ///
    /// # Panics
    ///
    /// If the queue refuses one that its check took, as [`Commands::check`] says it doesn't.
    pub(crate) fn give_back(&self, layer: usize, queue: &mut dyn Commands) {
        queue.clear();
        let commands = self.commands.iter().filter(|(at, _)| *at == layer);
        for (_, value) in commands {
            let pushed = queue.push(value);
            pushed.expect("a queue queues a command its check takes");
        }
    }

    /// The copy of the world taken as the tick started, which it runs again on.
    pub(crate) fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// Checks that `tick`, run again on the copy, left it as it left `world`, as their hashes say.
    ///
    /// Fails with `sync_test.diverged` if it didn't, naming the first entity and field where they
    /// differ, with each world's value there, as [`World::difference`] finds it; or saying that
    /// their layouts part, where they hold the same.
    pub(crate) fn compare(&mut self, tick: Tick, world: &World) -> Result<()> {
        if world.hash() == self.world.hash() {
            self.matched += 1;
            return Ok(());
        }
        let tick = tick.number;
        let parted = match world.difference(&self.world) {
            Some(difference) => {
                let written = difference.written(world, &self.world);
                let (at, ours, theirs) = (&written.at, &written.ours, &written.theirs);
                format!("first differ at {at}: the world's {ours}, the copy's {theirs}")
            }
            None => "hold the same, so their layouts part, where they hold their entities or \
                     their streams of IDs"
                .to_owned(),
        };
        let message = format!(
            "after tick {tick}, the world and a copy of it taken as the tick started, the tick run \
             again on it, {parted}: so the tick goes by something a copy of the world doesn't \
             hold, as what a layer keeps from one tick to the next"
        );
        Err(Error::new("sync_test.diverged", message))
    }

    /// How many ticks ran again and left the copy as they left the world.
    pub(crate) fn matched(&self) -> u64 {
        self.matched
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::dirs::tests::Scratch;
    use crate::layer::{Context, Layer, Stack};
    use crate::reflect::Reflect;
    use crate::reflect::tests::Handle;
    use crate::testing::logged;
    use crate::{Component, Entity, Place, Sound, Transform, audio};

    /// A command of [`Pusher`]'s: moves the entity along x by this far.
    #[derive(Reflect)]
    struct Push(Entity, f32);

    /// Applies its pushes at each tick, playing its sound on each entity pushed: its ticks go by the
    /// world and its commands alone.
    struct Pusher {
        pushes: Vec<Push>,
        sound: Sound,
    }

    impl Layer for Pusher {
        fn tick(&mut self, world: &mut World, _: Tick) {
            for Push(entity, by) in self.pushes.drain(..) {
                if let Some(transform) = world.get_mut::<Transform>(entity) {
                    transform.translation.x += f64::from(by);
                }
                world.play_sound(self.sound, Place::On(entity));
            }
        }

        fn commands(&mut self) -> Option<&mut dyn Commands> {
            Some(&mut self.pushes)
        }
    }

    /// How many ticks [`Counter`] has counted.
    #[derive(Clone, Copy, Component)]
    struct Counted {
        ticks: u32,
    }

    /// Counts its ticks in itself, and writes the count into the world: a tick run again counts on
    /// from where its first run left the count.
    #[derive(Default)]
    struct Counter {
        ticks: u32,
    }

    impl Layer for Counter {
        fn tick(&mut self, world: &mut World, _: Tick) {
            self.ticks += 1;
            for counted in world.query_mut::<&mut Counted>() {
                counted.ticks = self.ticks;
            }
        }
    }

    /// A command of [`Sealer`]'s, holding a handle of its own, which is opaque, so that no value
    /// makes it.
    #[derive(Reflect)]
    struct Seal(#[reflect(opaque)] Handle);

    /// Spawns an entity for each seal queued.
    struct Sealer(Vec<Seal>);

    impl Layer for Sealer {
        fn tick(&mut self, world: &mut World, _: Tick) {
            for _ in self.0.drain(..) {
                world.spawn(Transform::IDENTITY);
            }
        }

        fn commands(&mut self) -> Option<&mut dyn Commands> {
            Some(&mut self.0)
        }
    }

    /// Each tick runs again, once it has run, on a copy of the world taken as it started, with the
    /// commands it applied: the world takes them once, keeps its serial, and plays the sounds the
    /// tick queued, while the copy keeps those it queued again, which nothing plays; and the run
    /// ends saying that the ticks left the copy as they left the world.
    #[test]
    fn each_tick_runs_again_on_a_copy_which_it_leaves_as_it_left_the_world() {
        let scratch = Scratch::new("sync-test-sounds");
        fs::create_dir_all(&scratch.0).expect("a directory");
        let path = scratch.0.join("chime.wav");
        fs::write(&path, audio::tests::wav(&[0; 480])).expect("a WAV");
        let mut cx = Context::seeded(0);
        let sound = cx.add_sound("chime", &path).expect("a sound");
        let entity = cx.world_mut().spawn(Transform::IDENTITY);
        let mut stack = Stack::default();
        let pushes = Vec::new();
        stack.push_layer(Pusher { pushes, sound });
        stack.sync_test();
        let logged = logged();
        stack.attach(&mut cx);
        let serial = cx.world().serial();
        for number in 0..3 {
            let push = Push(entity, 1.0).to_value();
            let queue = stack.commands_of(0).expect("a queue");
            queue.push(&push).expect("queued");
            assert!(stack.tick(&mut cx, Tick { number }));
            assert!(!cx.exiting(), "tick {number}");
            let x = cx
                .world()
                .get::<Transform>(entity)
                .expect("alive")
                .translation
                .x;
            assert_eq!(x, f64::from(number as f32 + 1.0), "pushed once a tick");
            assert_eq!(cx.world().serial(), serial);
            assert_eq!(cx.world().sounds_queued().count(), 0, "played");
            let copy = stack.sync_test_mut().expect("sync-tested").world_mut();
            assert_eq!(
                copy.sounds_queued().count(),
                1,
                "queued again, and not played"
            );
        }
        assert_eq!(stack.sync_test_mut().map(|sync| sync.matched()), Some(3));
        stack.detach(&mut cx);
        assert!(!stack.failed());
        let codes: Vec<String> = logged.codes.try_iter().collect();
        assert!(codes.contains(&"sync_test.matched".to_owned()), "{codes:?}");
    }

    /// A layer that keeps what its tick changes from one tick to the next, which the tick run
    /// again finds as its first run left it, parts the copy from the world: the run again fails
    /// naming the entity and the field where they first differ, with both values; and a run's tick
    /// ends the run with the error, and the SyncTest with it.
    #[test]
    fn a_layer_keeping_state_between_ticks_parts_the_copy_from_the_world_at_its_field() {
        let mut cx = Context::seeded(0);
        let counted = cx.world_mut().spawn(Counted { ticks: 0 });
        let id = cx.world().id(counted).expect("alive");
        let mut stack = Stack::default();
        stack.push_layer(Counter::default());
        stack.sync_test();
        stack.attach(&mut cx);
        let replayed = stack.replay_tick(&mut cx, Tick { number: 0 }, |_, _, _| Ok(()));
        let error = replayed.expect_err("parted");
        assert_eq!(error.code(), "sync_test.diverged");
        let message = error.to_string();
        let at = format!("{id}'s `Counted.ticks`");
        for named in [
            "after tick 0,",
            &at,
            "the world's UInt(1), the copy's UInt(2)",
        ] {
            assert!(message.contains(named), "{named}: {message}");
        }
        assert!(stack.sync_test_mut().is_none(), "over");
        stack.sync_test();
        let logged = logged();
        assert!(stack.tick(&mut cx, Tick { number: 1 }));
        assert!(cx.exiting());
        assert!(stack.sync_test_mut().is_none(), "over");
        stack.detach(&mut cx);
        assert!(stack.failed());
        let codes: Vec<String> = logged.codes.try_iter().collect();
        assert!(
            codes.contains(&"sync_test.diverged".to_owned()),
            "{codes:?}"
        );
        assert!(
            !codes.contains(&"sync_test.matched".to_owned()),
            "{codes:?}"
        );
    }

    /// A command a layer's tick applied that its queue wouldn't queue again, as one holding
    /// something opaque, keeps the tick from running again: it applies to the world, and the run
    /// again fails naming the layer, the command and why.
    #[test]
    fn a_command_its_queue_wouldnt_queue_again_keeps_the_tick_from_running_again() {
        let mut cx = Context::seeded(0);
        let mut stack = Stack::default();
        stack.push_layer(Sealer(vec![Seal(Handle(7))]));
        stack.sync_test();
        stack.attach(&mut cx);
        let replayed = stack.replay_tick(&mut cx, Tick { number: 0 }, |_, _, _| Ok(()));
        let error = replayed.expect_err("refused");
        assert_eq!(error.code(), "sync_test.bad_command");
        let message = error.to_string();
        for named in ["layer 0, Sealer,", "Opaque", "it's opaque"] {
            assert!(message.contains(named), "{named}: {message}");
        }
        let spawned = cx.world().query::<&Transform>().count();
        assert_eq!(spawned, 1, "applied to the world");
    }
}

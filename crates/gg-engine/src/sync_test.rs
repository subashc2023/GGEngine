//! The SyncTest (`--sync-test`) runs each tick again on a copy of the world taken as the tick
//! started, with the commands the layers' ticks applied, then checks the copy against the world.
//! Rollback needs that: it reruns ticks from a snapshot. A tick that depends on more than the
//! world, its commands and what its layer set up (layer state kept between ticks, something a
//! world copy doesn't hold) is named where the two part. The stack runs it at each tick, replayed
//! ticks included. A tick's logs repeat on the second run.

use gg_core::{Error, Result};

use crate::layer::Commands;
use crate::recording::text;
use crate::reflect::Value;
use crate::{Tick, World};

/// What a SyncTest keeps of a tick between its run and its rerun.
#[derive(Default)]
pub(crate) struct SyncTest {
    /// The world as the tick started, in the previous copy's memory; the tick reruns on it.
    world: World,
    /// The commands each layer's tick applied, as values, with the layer's index from the bottom.
    commands: Vec<(usize, Value)>,
    /// The first applied command its queue wouldn't queue again (one holding something opaque).
    /// It stops the rerun.
    refused: Option<Error>,
    /// How many reruns left the copy as the world.
    matched: u64,
}

impl SyncTest {
    pub(crate) fn take(&mut self, world: &World) {
        self.world.clone_from(world);
        self.commands.clear();
    }

    /// Keeps the commands `queue` (layer `layer` from the bottom, type `name`) holds as its tick
    /// is about to apply them in `world`. The first its [`check`](Commands::check) rejects stops
    /// the rerun ([`refused`](Self::refused)).
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

    /// Fails with `sync_test.bad_command`, naming layer, command and why, if
    /// [`queued`](Self::queued) met a command its queue wouldn't requeue.
    pub(crate) fn refused(&mut self) -> Result<()> {
        self.refused.take().map_or(Ok(()), Err)
    }

    /// Empties `queue` (layer `layer` from the bottom) and requeues its tick's applied commands.
    ///
    /// # Panics
    ///
    /// If the queue refuses a command its [`check`](Commands::check) took.
    pub(crate) fn give_back(&self, layer: usize, queue: &mut dyn Commands) {
        queue.clear();
        let commands = self.commands.iter().filter(|(at, _)| *at == layer);
        for (_, value) in commands {
            let pushed = queue.push(value);
            pushed.expect("a queue queues a command its check takes");
        }
    }

    pub(crate) fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// Checks by hash that `tick`, rerun on the copy, left it as it left `world`.
    ///
    /// Fails with `sync_test.diverged` if not, naming the first differing entity and field
    /// ([`World::difference`]) with each value, or saying their layouts part if they hold the same.
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

    /// Moves the entity along x by this far.
    #[derive(Reflect)]
    struct Push(Entity, f32);

    /// Applies its pushes each tick, playing its sound on each entity pushed; depends on the world
    /// and its commands alone.
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

    /// Ticks [`Counter`] counted.
    #[derive(Clone, Copy, Component)]
    struct Counted {
        ticks: u32,
    }

    /// Counts ticks in itself and writes the count into the world, so a rerun counts on from the
    /// first run's count.
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

    /// Holds an opaque handle, so no value makes it.
    #[derive(Reflect)]
    struct Seal(#[reflect(opaque)] Handle);

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

    /// Each tick reruns on a copy taken as it started, with its applied commands. The world takes
    /// them once, keeps its serial and plays the tick's sounds; the copy keeps its requeued
    /// sounds, which nothing plays. The run ends saying the copy matched the world.
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
        stack.attach(&mut cx).expect("loaded");
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

    /// A layer keeping state between ticks, which the rerun finds as the first run left it, parts
    /// the copy from the world: the rerun fails naming the first differing entity, field and both
    /// values. In a run, that ends the run with the error, and the SyncTest too.
    #[test]
    fn a_layer_keeping_state_between_ticks_parts_the_copy_from_the_world_at_its_field() {
        let mut cx = Context::seeded(0);
        let counted = cx.world_mut().spawn(Counted { ticks: 0 });
        let id = cx.world().id(counted).expect("alive");
        let mut stack = Stack::default();
        stack.push_layer(Counter::default());
        stack.sync_test();
        stack.attach(&mut cx).expect("loaded");
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

    /// A command its queue wouldn't requeue (one holding something opaque) stops the rerun: it
    /// applies to the world, and the rerun fails naming layer, command and why.
    #[test]
    fn a_command_its_queue_wouldnt_queue_again_keeps_the_tick_from_running_again() {
        let mut cx = Context::seeded(0);
        let mut stack = Stack::default();
        stack.push_layer(Sealer(vec![Seal(Handle(7))]));
        stack.sync_test();
        stack.attach(&mut cx).expect("loaded");
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

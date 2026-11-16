//! How a lockstep peer finds a desync: the hashes it compares, and where the worlds parted.

use tracing::{error, warn};

use super::REFUSED;
use super::lockstep::Lockstep;
use crate::World;

impl Lockstep {
    /// Ends tick `tick`, run on both peers' inputs, leaving `world`. With another peer: sends the
    /// hash, keeps the world until it's compared (unless the peers have parted), and compares.
    pub(super) fn finish(&mut self, tick: u64, world: &World) {
        if self.from.is_none() {
            return;
        }
        if !self.parted {
            let mut kept = self.spares.pop().unwrap_or_default();
            kept.clone_from(world);
            self.worlds.insert(tick, kept);
        }
        let hash = world.hash();
        self.send(format!("h {tick} {hash}\n"));
        self.hashes[0].insert(tick, hash);
        self.compare();
    }

    /// Compares both peers' hashes after the same tick, dropping them and our kept worlds (unless
    /// the peers have parted). A pair that differs logs `net.desync`; the host sends its joiner its
    /// world after that tick, and the joiner keeps its own until it comes.
    pub(super) fn compare(&mut self) {
        if self.parted {
            self.hashes = Default::default();
            return;
        }
        let hosts = self.hosts();
        let mut parted = None;
        let [ours, theirs] = &mut self.hashes;
        while let (Some((&a, _)), Some((&b, _))) =
            (ours.first_key_value(), theirs.first_key_value())
        {
            if a != b {
                // One peer's hashes run ahead: drop the older, unmatched.
                if a < b {
                    ours.pop_first();
                } else {
                    theirs.pop_first();
                }
                continue;
            }
            let (_, mine) = ours.pop_first().expect("it has a first");
            let (_, other) = theirs.pop_first().expect("it has a first");
            if mine != other {
                error!(
                    code = "net.desync",
                    tick = a,
                    "The peers' worlds went apart at tick {a}: this one's hashes {mine}, the \
                     other's {other}; the host sends its world again"
                );
                self.owed = hosts;
                self.parted = true;
                ours.clear();
                theirs.clear();
                parted = Some(a);
                break;
            }
        }
        // Drop worlds whose hashes went, but keep the one after the parting tick.
        let first = self.hashes[0].first_key_value().map(|(&tick, _)| tick);
        let mut kept = None;
        while let Some(world) = self.worlds.first_entry()
            && first.is_none_or(|first| *world.key() < first)
        {
            let (tick, world) = world.remove_entry();
            if parted == Some(tick) {
                kept = Some(world);
            } else {
                self.spares.push(world);
            }
        }
        if let (Some(tick), Some(world)) = (parted, kept) {
            if hosts {
                // Dropped after: sending may lose the joiner, and its spares.
                self.send_parted(tick, &world);
            } else {
                self.worlds.insert(tick, world);
            }
        }
    }

    /// Host: sends the joiner `world`, its own after `tick`, where the hashes parted.
    fn send_parted(&mut self, tick: u64, world: &World) {
        let mut text = format!("parted {tick}\n");
        match world.save_whole(&mut text) {
            Ok(()) => {
                text.push_str("end\n");
                self.send(text);
            }
            Err(error) => warn!(
                code = error.code(),
                "Can't send the joiner the world where theirs parted from it: {error:#}"
            ),
        }
    }

    /// Joiner: the host's world after `tick`, where the hashes parted, has come as `text`. Logs
    /// the first entity, component and field where ours differs ([`World::difference`],
    /// `net.desync_field`). If a cell's file differs from the host's
    /// ([`World::cell_file_parted`]), names it (`net.cell_file`) and leaves the session to try
    /// again later, as after a refusal.
    pub(super) fn name_parting(&mut self, tick: u64, text: &str) {
        // None after a tick it kept no world after, as one it didn't part at.
        let Some(ours) = self.worlds.remove(&tick) else {
            return;
        };
        // Loaded into a copy of ours, with the same types and assets. What's opaque comes back as
        // the host's world names it, as in a world never drawn and compared as the same.
        let mut theirs = ours.clone();
        // Numbered as the message's lines, after its `parted`.
        let lines = (2..).zip(text.lines().take_while(|line| *line != "end"));
        match theirs.load_whole(lines, true) {
            Ok(_) => {
                log_parting(tick, &ours, &theirs);
                if let Some((file, joiner, host)) = ours.cell_file_parted(&theirs) {
                    error!(
                        code = "net.cell_file",
                        tick,
                        file = &*file,
                        joiner = %format_args!("{joiner:016x}"),
                        host = %format_args!("{host:016x}"),
                        "The peers' worlds parted after tick {tick} as the cell file {file} came \
                         in, which isn't the host's here: the joiner leaves, and tries again in {} \
                         s; give both peers the same files",
                        REFUSED.as_secs()
                    );
                    self.lose(&format!("its cell file {file} isn't the host's"));
                    self.find.again();
                }
            }
            Err(error) => warn!(
                code = error.code(),
                tick,
                "Can't read the host's world after tick {tick}, where the peers parted: {error:#}"
            ),
        }
        self.spares.push(ours);
    }
}

/// Logs where `ours` (the joiner's world after `tick`) first differs from `theirs` (the host's)
/// (`net.desync_field`): the entity, the component's field and each world's value as recordings
/// write them, `none` where a world lacks the component. An entity only one world has is `alive`
/// or `none`, its field empty.
fn log_parting(tick: u64, ours: &World, theirs: &World) {
    let Some(difference) = ours.difference(theirs) else {
        error!(
            code = "net.desync_field",
            tick,
            "After tick {tick} the peers' worlds hold the same content, so their layouts part, \
             where they hold their entities or their streams of IDs"
        );
        return;
    };
    let written = difference.written(ours, theirs);
    let (at, joiner, host) = (&written.at, &written.ours, &written.theirs);
    error!(
        code = "net.desync_field",
        tick,
        entity = %difference.entity,
        field = written.field.as_str(),
        joiner = joiner.as_str(),
        host = host.as_str(),
        "After tick {tick} the peers' worlds first differ at {at}: the joiner's {joiner}, the \
         host's {host}"
    );
}

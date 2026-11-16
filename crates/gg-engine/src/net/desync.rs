//! How a lockstep peer finds a desync: the hashes it compares, and where the worlds parted.

use tracing::{error, warn};

use super::REFUSED;
use super::lockstep::Lockstep;
use crate::World;

impl Lockstep {
    /// Ends tick `tick`, which has run on both peers' inputs, leaving `world`: sends its hash, if
    /// there's another peer, keeping a copy of the world until the hash is compared with theirs,
    /// unless the peers have parted, and compares it.
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

    /// Compares the hashes both peers have after the same tick, dropping them and the worlds kept
    /// with this peer's, unless the peers have parted; a pair that differs logs `net.desync`, and
    /// has the host send its joiner its world after that tick, which the joiner keeps its own after
    /// until it comes, then its world again.
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
                // One peer's hashes run ahead of the other's: drop the older, unmatched.
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
        // The worlds kept with the hashes dropped, but the one after the tick the peers parted at.
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
                // Dropped after, as sending it may lose the joiner, its spares with it.
                self.send_parted(tick, &world);
            } else {
                self.worlds.insert(tick, world);
            }
        }
    }

    /// The host, as its hashes part from its joiner's after tick `tick`: sends the joiner `world`,
    /// its own after that tick, which the joiner names where they part by.
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

    /// The joiner, as its host's world after tick `tick`, where their hashes parted, has come whole
    /// as `text`: logs the first entity, component and field where its own world after that tick
    /// differs, as [`World::difference`] finds it (`net.desync_field`); and, where a cell's file
    /// came in from another file than the host's, as [`World::cell_file_parted`] finds it, names
    /// it (`net.cell_file`) and leaves the session, to try again later, as a refused hello does.
    pub(super) fn name_parting(&mut self, tick: u64, text: &str) {
        // None after a tick the joiner hasn't kept a world after, as one it didn't part at.
        let Some(ours) = self.worlds.remove(&tick) else {
            return;
        };
        // Into a copy of its own, of the same types and assets. What's opaque comes back as the
        // host's world names it, which a world never drawn and compared as the same takes.
        let mut theirs = ours.clone();
        // Numbered as lines of the message, after its `parted`.
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

/// Logs where `ours`, the joiner's world after tick `tick`, first differs from `theirs`, its host's
/// (`net.desync_field`): the entity, and the component's field, with each world's value there,
/// as recordings write values, or `none` where a world hasn't the component; `alive` or `none`
/// for an entity one world alone has, its field empty.
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

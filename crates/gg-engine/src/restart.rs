//! Restarts in place, for a patch whose types are laid out unlike the code the run runs
//! ([`gg_api::Patch::restart`]). Between frames, the main loop ends the layers and drops all the
//! run held with that code, applies the patch, builds the app again with the patch's code
//! ([`App::hot`]), and restores the snapshot the tool API wrote. The process, its window and the
//! API's connections stay. ARCHITECTURE.md's Hot reload keeps the world says why.
//!
//! The rule it keeps is subsecond's: no value laid out by the code the run ran reaches the
//! patch's. A type keeps its `TypeId` as its fields change, so nothing else would catch a breach.
//! Hence the order: what the layers and the context hold drops before the patch applies, and the
//! world crosses as the snapshot's text.

use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::process;
use std::time::Instant;

use gg_core::{Error, Result};
use subsecond::{HotFn, JumpTable};
use tracing::{error, info};

use crate::api::{self, Api};
use crate::layer::{Context, Stack};
use crate::snapshot::Snapshot;
use crate::{App, patch};

/// Why a restart in place that failed once the layers ended ends the game.
const ENDED: &str = "the restart in place failed after the layers ended, so the game ends; the \
                     snapshot it wrote holds its world";

/// A restart in place the tool API holds for the main loop, its snapshot written.
pub(crate) struct Restart {
    pub(crate) table: JumpTable,
    /// The snapshot, written with the code the run ran.
    pub(crate) snapshot: PathBuf,
    /// When the snapshot started, which the restored run's load (`engine.loaded`) is timed from.
    pub(crate) started: Instant,
    pub(crate) snapshot_ms: u64,
}

/// What the restart's stages took so far, in milliseconds, as [`gg_api::Restarted`] gives them.
pub(crate) struct Stages {
    snapshot_ms: u64,
    teardown_ms: u64,
    apply_ms: u64,
    build_ms: u64,
    /// Where the patch's library loaded, as [`gg_api::Restarted::loaded`] says.
    loaded: Option<u64>,
}

impl Stages {
    /// The answer, the restore having taken `restore_ms`.
    pub(crate) fn restored(&self, restore_ms: u64) -> gg_api::Restarted {
        gg_api::Restarted {
            snapshot_ms: self.snapshot_ms,
            teardown_ms: self.teardown_ms,
            apply_ms: self.apply_ms,
            build_ms: self.build_ms,
            restore_ms,
            loaded: self.loaded,
        }
    }
}

impl Restart {
    /// Ends the layers, top down, then drops them and all `cx` holds, with the code the run ran:
    /// the world, cells, meshes, sound, UIs, loads under way, authors' logs and tools. `cx` is left
    /// empty, for the restored run's context to replace. The main loop drops egui's state and the
    /// renderer too.
    pub(crate) fn tear_down(stack: &mut Stack, cx: &mut Context) {
        let _zone = gg_core::zone!();
        stack.detach(cx);
        stack.drop_layers();
        *cx = Context::new(None);
    }

    /// Applies the patch after `ticks` ticks, then builds the app again with its code and takes
    /// its layers into `app` ([`App::take_layers`]). Returns the snapshot to restore, read, with
    /// what each stage took, `teardown_ms` the teardown's.
    ///
    /// Fails with `api.restart_failed`, the layers gone, if:
    /// - the patch can't apply ([`patch::apply`]);
    /// - the app's function panics, or the app it builds differs ([`App::take_layers`]);
    /// - the snapshot can't be read.
    pub(crate) fn build(self, app: &mut App, ticks: u64, teardown_ms: u64) -> Result<Restored> {
        let _zone = gg_core::zone!();
        let failed = |error: Error| Error::new("api.restart_failed", ENDED).with_source(error);
        let started = Instant::now();
        let loaded = patch::apply(self.table, ticks, true).map_err(failed)?;
        let apply_ms = millis(started);
        let started = Instant::now();
        let hot = app
            .hot
            .expect("a run restarts in place only if its app names its function");
        // A function pointer: subsecond looks it up by the executable's own address of it, which
        // every patch maps. gg-core's panic hook logs a panic.
        let built = panic::catch_unwind(AssertUnwindSafe(|| HotFn::current(hot).call(())))
            .map_err(|_| {
                let message = "the function that builds the app panicked, with the patch's code";
                failed(Error::new("api.restart", message))
            })?;
        app.take_layers(built).map_err(failed)?;
        app.started = Some(self.started);
        let snapshot = Snapshot::read(&self.snapshot, app.name).map_err(failed)?;
        let build_ms = millis(started);
        info!(
            layers = ?app.stack.names(),
            snapshot = %self.snapshot.display(),
            "Built the app again with the patch's code"
        );
        Ok(Restored {
            snapshot,
            stages: Stages {
                snapshot_ms: self.snapshot_ms,
                teardown_ms,
                apply_ms,
                build_ms,
                loaded,
            },
        })
    }
}

/// The snapshot a restart in place restores, and what its stages took.
pub(crate) struct Restored {
    pub(crate) snapshot: Snapshot,
    pub(crate) stages: Stages,
}

/// Ends the process after a restart in place failed with its layers gone, as nothing is left to
/// run: logs `error`, as `api.restart_failed`, answers the `patch` with it, waiting for the answer
/// to be written, and drops the API, which removes the session file. `gg run` then restarts the
/// game from the snapshot.
pub(crate) fn fail(api: &mut Option<Api>, error: Error, ticks: u64) -> ! {
    let error = if error.code() == "api.restart_failed" {
        error
    } else {
        Error::new("api.restart_failed", ENDED).with_source(error)
    };
    error!(code = error.code(), "{error:#}");
    api::restarted(api, Err(error), ticks);
    drop(api.take());
    process::exit(1)
}

fn millis(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Layer;

    struct Hull;

    impl Layer for Hull {}

    struct Sails;

    impl Layer for Sails {}

    /// The app built again gives its layers, in place of those dropped, unless it differs in what
    /// the run started with.
    #[test]
    fn an_app_built_again_gives_its_layers_unless_it_differs() {
        let game = || App::new("Game").seeded(3);
        let mut app = game().layer(Hull);
        app.stack.drop_layers();
        app.take_layers(game().layer(Sails).layer(Hull))
            .expect("taken");
        assert_eq!(app.stack.names(), ["Sails", "Hull"]);
        assert_eq!(app.own_sessions.layers, ["Sails", "Hull"]);
        for differs in [
            App::new("Other").seeded(3),
            App::new("Game"),
            game().window_size(640, 480),
            game().view_panel("View"),
            game().root("elsewhere"),
        ] {
            app.stack.drop_layers();
            let refused = app.take_layers(differs.layer(Hull)).expect_err("refused");
            assert_eq!(refused.code(), "api.restart", "{refused}");
            assert!(app.stack.names().is_empty());
        }
    }
}

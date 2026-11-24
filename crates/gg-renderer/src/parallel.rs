//! Making what's slow to make on threads of its own: pipelines, as a driver compiles their shaders
//! as they're made. On macOS, Metal compiles each anew for an app the first time it starts, seconds
//! in all made one after another.

use std::panic;
use std::thread::{self, Scope, ScopedJoinHandle};

use gg_core::{Error, Result};

/// What a thread of a scope is making, or why the thread didn't start.
pub(crate) struct Making<'scope, T>(Result<ScopedJoinHandle<'scope, Result<T>>>);

/// Starts `make` on a thread of `scope`'s own. A failure to start comes out of [`Making::made`].
pub(crate) fn make<'scope, T: Send + 'scope>(
    scope: &'scope Scope<'scope, '_>,
    make: impl FnOnce() -> Result<T> + Send + 'scope,
) -> Making<'scope, T> {
    let started = thread::Builder::new()
        // Within the 15 bytes Linux keeps of a thread's name.
        .name("gg-pipelines".to_owned())
        .spawn_scoped(scope, make)
        .map_err(|error| {
            Error::new("renderer.threads", "can't start a thread to make pipelines")
                .with_source(error)
        });
    Making(started)
}

impl<T> Making<'_, T> {
    /// What it made, once made. A panic making it resumes here.
    ///
    /// # Errors
    ///
    /// - As making it failed.
    /// - `renderer.threads`: its thread didn't start.
    pub(crate) fn made(self) -> Result<T> {
        match self.0?.join() {
            Ok(made) => made,
            Err(panic) => panic::resume_unwind(panic),
        }
    }
}

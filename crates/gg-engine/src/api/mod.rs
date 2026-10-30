//! The tool API, which development builds serve, as ARCHITECTURE.md's Agents and tools describes,
//! through gg-api's protocol: what the main loop calls of it, each a call that does nothing without
//! `dev`, or without the API, as with `--no-api`; and what frames have paused for, which `ping`
//! reports.

#[cfg(feature = "dev")]
mod json;
#[cfg(feature = "dev")]
mod server;

use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::Context;
use crate::layer::Stack;
use crate::main_loop::Stepper;
use crate::render_thread::{Render, RenderThread};

#[cfg(feature = "dev")]
pub(crate) use server::Api;

/// The API, which builds without `dev` have none of.
#[cfg(not(feature = "dev"))]
pub(crate) enum Api {}

/// Starts serving the tool API for app `app`, whose layers are `layers`, from the bottom up, and
/// whose JSON log is at `log`, if it has one, unless `serve` says not to: `None` then, and without
/// `dev`. If it can't start, the run goes on without it, warning `api.start`.
pub(crate) fn start(app: &str, layers: &[&str], log: Option<&Path>, serve: bool) -> Option<Api> {
    #[cfg(feature = "dev")]
    if serve {
        return Api::start(app, layers, log)
            .inspect_err(|error| {
                let consequence = "tools can't reach this run";
                tracing::warn!(code = error.code(), "{error:#}; {consequence}");
            })
            .ok();
    }
    let _ = (app, layers, log, serve);
    None
}

/// As a frame starts, before its ticks, in a development build, applies the scene files saved since,
/// as [`scene_files::look`] says, and answers the requests that have come, from `cx`, whose layers
/// are `stack`'s and whose simulation `stepper` steps, as [`Api::serve`] says; returns whether one
/// asked the run to end.
///
/// [`scene_files::look`]: crate::scene_files
pub(crate) fn serve(
    api: &mut Option<Api>,
    cx: &mut Context,
    stack: &mut Stack,
    stepper: &mut Stepper,
) -> bool {
    #[cfg(feature = "dev")]
    crate::scene_files::look(cx, stack);
    #[cfg(feature = "dev")]
    if let Some(api) = api {
        return api.serve(cx, stack, stepper);
    }
    let _ = (api, cx, stack, stepper);
    false
}

/// Between frames, while they pause, applies the scene files saved since and answers the requests
/// that need no frames, as [`serve`] does as a frame starts; returns whether one asked the run to
/// end.
pub(crate) fn serve_paused(
    api: &mut Option<Api>,
    cx: &mut Context,
    stack: &mut Stack,
    stepper: &mut Stepper,
) -> bool {
    #[cfg(feature = "dev")]
    crate::scene_files::look(cx, stack);
    #[cfg(feature = "dev")]
    if let Some(api) = api {
        return api.serve_paused(cx, stack, stepper);
    }
    let _ = (api, cx, stack, stepper);
    false
}

/// With the renderer back from a frame, takes the view it read back, if it rode with captures and
/// rendered, and has the next frame handed off, which shows the world after `ticks` ticks, read
/// its view back if captures wait, as [`Api::frame_back`] says.
pub(crate) fn frame_back<R: Render>(
    api: &mut Option<Api>,
    render: &mut RenderThread<R>,
    ticks: u64,
) {
    #[cfg(feature = "dev")]
    if let Some(api) = api {
        api.frame_back(render.frame_mut(), ticks);
    }
    let _ = (api, render, ticks);
}

/// While the simulation is paused with nothing asked of the frames, waits for a request for at
/// most a tick's time, so that a headless run, whose frames would otherwise go as fast as the GPU
/// renders them, doesn't spin.
pub(crate) fn idle(api: &mut Option<Api>, stepper: &Stepper) {
    #[cfg(feature = "dev")]
    if let Some(api) = api
        && api.is_idle(stepper)
    {
        api.wait(crate::Tick::DT);
    }
    let _ = (api, stepper);
}

/// Has `wake` wake the main loop as each request comes, so that it answers them while frames
/// pause.
pub(crate) fn set_waker(api: &mut Option<Api>, wake: impl Fn() + Send + 'static) {
    #[cfg(feature = "dev")]
    if let Some(api) = api {
        api.set_waker(Box::new(wake));
        return;
    }
    let _ = (api, wake);
}

/// What frames have paused for, which `ping` reports from the connections' threads, while the main
/// thread waits: 0 for nothing, else a [`Waiting`]. A process runs one app at a time, whose main
/// thread is the one that waits.
static WAITING: AtomicU8 = AtomicU8::new(0);

/// What frames pause for that's the user's doing, rather than a hang.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Waiting {
    /// The main window has no area.
    Minimized = 1,
    /// A file dialog is open, which development builds alone show.
    #[cfg(feature = "dev")]
    Dialog = 2,
    /// The window menu Windows runs, in a loop of its own, is open.
    Menu = 3,
}

impl Waiting {
    /// What frames have paused for now.
    #[cfg(feature = "dev")]
    fn now() -> Option<Self> {
        match WAITING.load(Ordering::Relaxed) {
            1 => Some(Self::Minimized),
            2 => Some(Self::Dialog),
            3 => Some(Self::Menu),
            _ => None,
        }
    }

    /// Its name, as `ping` gives it.
    #[cfg(feature = "dev")]
    fn name(self) -> &'static str {
        match self {
            Self::Minimized => "minimized",
            Self::Dialog => "dialog",
            Self::Menu => "menu",
        }
    }
}

/// Says that frames have paused for `waiting` until what it returns drops, which says again what
/// they waited for before.
pub(crate) fn waiting(waiting: Waiting) -> Waited {
    Waited(WAITING.swap(waiting as u8, Ordering::Relaxed))
}

/// What [`waiting`] returns.
pub(crate) struct Waited(u8);

impl Drop for Waited {
    fn drop(&mut self) {
        WAITING.store(self.0, Ordering::Relaxed);
    }
}

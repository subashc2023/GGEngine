//! The tool API, which development builds serve, as ARCHITECTURE.md's Agents and tools describes,
//! through gg-api's protocol: what the main loop calls of it, each a call that does nothing without
//! `dev`, or without the API, as with `--no-api`; and what frames have paused for, which `ping`
//! reports.

#[cfg(feature = "dev")]
pub(crate) mod json;
#[cfg(feature = "dev")]
mod server;

use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
#[cfg(feature = "dev")]
use std::time::Duration;

use crate::Context;
use crate::layer::Stack;
use crate::main_loop::Stepper;
use crate::render_thread::{Render, RenderThread};

#[cfg(feature = "dev")]
pub(crate) use server::{Api, api_viewpoint};

/// The API, which builds without `dev` have none of.
#[cfg(not(feature = "dev"))]
pub(crate) enum Api {}

/// The look the API's clients set, as a frame draws with it, which [`Api`] sets as it answers:
/// each part in place of the game's own, where one's set; the revision it's at, which a frame
/// published gives; whether the frame is bare, as a capture of the game's own view asks, which
/// leaves the look out; and the viewpoint the last frame handed off drew from, the look's or the
/// game's own.
#[cfg(feature = "dev")]
#[derive(Default)]
pub(crate) struct Look {
    pub(crate) viewpoint: Option<gg_renderer::Viewpoint>,
    pub(crate) outline: Option<gg_renderer::Outline>,
    pub(crate) grid: Option<crate::grid::Plane>,
    pub(crate) colliders: Option<bool>,
    pub(crate) revision: u64,
    pub(crate) bare: bool,
    pub(crate) seen: Option<gg_renderer::Viewpoint>,
}

/// Starts serving the tool API for app `app`, whose layers are `layers`, from the bottom up, whose
/// JSON log is at `log`, if it has one, its last events kept in gg-core's `log::EVENTS`, which
/// plays `player` in a lockstep session, if it's a peer of one, and whose project's manifest is at
/// `project`, if it's a project's game, unless `serve` says not to: `None` then, and without `dev`.
///
/// Fails, with `api.start`, if it can't start, as when its port can't be bound or its session file
/// written: the run goes on without it, warning so, but for an embedded one, which ends.
pub(crate) fn start(
    app: &str,
    layers: &[&str],
    log: Option<&Path>,
    player: Option<u8>,
    project: Option<&Path>,
    serve: bool,
) -> gg_core::Result<Option<Api>> {
    #[cfg(feature = "dev")]
    if serve {
        return Api::start(app, layers, log, &gg_core::log::EVENTS, player, project).map(Some);
    }
    let _ = (app, layers, log, player, project, serve);
    Ok(None)
}

/// As a frame starts, before its ticks, in a development build, takes the asset database as the
/// thread that watches the root last sent it, as `Context::take_assets` says, and the shaders,
/// textures, models and fonts read again as their files changed, as `Context::take_reloaded` says,
/// applies the scene files saved since, as [`scene_files::look`] says, and answers the requests
/// that have come, from `cx`, whose layers are `stack`'s and whose simulation `stepper` steps, as
/// [`Api::serve`] says; returns whether one asked the run to end.
///
/// [`scene_files::look`]: crate::scene_files
pub(crate) fn serve(
    api: &mut Option<Api>,
    cx: &mut Context,
    stack: &mut Stack,
    stepper: &mut Stepper,
) -> bool {
    #[cfg(feature = "dev")]
    {
        cx.take_assets();
        cx.take_reloaded(stack.records());
    }
    #[cfg(feature = "dev")]
    crate::scene_files::look(cx, stack);
    #[cfg(feature = "dev")]
    if let Some(api) = api {
        return api.serve(cx, stack, stepper);
    }
    let _ = (api, cx, stack, stepper);
    false
}

/// Between frames, while they pause, takes the asset database and the assets read again, applies
/// the scene files saved since and answers the requests that need no frames, as [`serve`] does as
/// a frame starts; returns whether one asked the run to end.
pub(crate) fn serve_paused(
    api: &mut Option<Api>,
    cx: &mut Context,
    stack: &mut Stack,
    stepper: &mut Stepper,
) -> bool {
    #[cfg(feature = "dev")]
    {
        cx.take_assets();
        cx.take_reloaded(stack.records());
    }
    #[cfg(feature = "dev")]
    crate::scene_files::look(cx, stack);
    #[cfg(feature = "dev")]
    if let Some(api) = api {
        return api.serve_paused(cx, stack, stepper);
    }
    let _ = (api, cx, stack, stepper);
    false
}

/// As frames pause, as the window is minimized, answers each `assets` waiting for a look at the
/// asset root from the database as the thread that watches it last sent it, as one that comes while
/// they pause is answered, since a look the thread takes then would wait for the next request.
pub(crate) fn frames_paused(api: &mut Option<Api>, cx: &mut Context) {
    #[cfg(feature = "dev")]
    if let Some(api) = api {
        cx.take_assets();
        api.frames_paused(cx);
    }
    let _ = (api, cx);
}

/// With the renderer back from a frame, takes the view it read back, if it rode with captures and
/// rendered, answers the picks whose answers came back into `cx`, and has the next frame handed
/// off, which `cx` made and shows the world after `ticks` ticks, read its view back if captures
/// wait or a client has it published, as [`Api::frame_back`] says.
pub(crate) fn frame_back<R: Render>(
    api: &mut Option<Api>,
    render: &mut RenderThread<R>,
    cx: &Context,
    ticks: u64,
) {
    #[cfg(feature = "dev")]
    if let Some(api) = api {
        api.frame_back(render.frame_mut(), cx, ticks);
    }
    let _ = (api, render, cx, ticks);
}

/// The size the client the view is published to asks frames to be, while it's published, as an
/// embedded run renders them, if it fits in the client's memory; with the renderer back from a
/// frame.
pub(crate) fn asked<R: Render>(render: &RenderThread<R>) -> Option<[u32; 2]> {
    #[cfg(feature = "dev")]
    if let Some(publish) = &render.frame().publish {
        return publish.writer.asked();
    }
    let _ = render;
    None
}

/// How long an embedded run waits for its client to take a frame, before it runs the next anyway:
/// its ticks go on in time while the client takes none, as when its window is minimized.
#[cfg(feature = "dev")]
const PACED: Duration = Duration::from_millis(100);

/// Before the frame's handed off, while the view is published, the number of the frame, as the
/// client will take it, which [`pace`] then waits for; `None` for a frame that isn't published, as
/// a capture of the game's own view's isn't.
pub(crate) fn handing(api: &Option<Api>) -> Option<u64> {
    #[cfg(feature = "dev")]
    if let Some(api) = api {
        return api.handing();
    }
    let _ = api;
    None
}

/// Once the frame's handed off, waits until the client has taken it, as [`handing`] numbered it,
/// for at most [`PACED`], so that an embedded run renders no frame its client doesn't take.
pub(crate) fn pace(api: &Option<Api>, handed: u64) {
    #[cfg(feature = "dev")]
    if let Some(api) = api {
        api.pace(handed, PACED);
    }
    let _ = (api, handed);
}

/// While the simulation is paused or stopped with nothing asked of the frames, waits for a request
/// for at most a tick's time, so that a headless run, whose frames would otherwise go as fast as
/// the GPU renders them, doesn't spin.
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

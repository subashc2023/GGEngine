//! The tool API development builds serve through gg-api's protocol (ARCHITECTURE.md's Agents and
//! tools). It holds the calls the main loop makes, each a no-op without `dev` or with `--no-api`,
//! and what frames have paused for, which `ping` reports.

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
pub(crate) use server::{Api, instanced};
#[cfg(feature = "dev")]
pub use server::{api_viewpoint, engine_viewpoint};

/// Builds without `dev` have no API.
#[cfg(not(feature = "dev"))]
pub(crate) enum Api {}

/// The look the API's clients set, which [`Api`] updates as it answers and a frame draws with.
///
/// Each part replaces the game's own where set. `revision` is what a published frame reports.
/// A `bare` frame, as a capture of the game's own view asks, ignores the look, `skip` included.
/// `captured` is what a capture asks the frame to show instead of the view's light, over what the
/// look shows. `seen` is the viewpoint the last handed-off frame drew from.
#[cfg(feature = "dev")]
#[derive(Default)]
pub(crate) struct Look {
    pub(crate) viewpoint: Option<gg_renderer::Viewpoint>,
    pub(crate) outline: Option<crate::layer::Outlining>,
    pub(crate) grid: Option<crate::grid::Plane>,
    pub(crate) colliders: Option<bool>,
    pub(crate) show: Option<Shown>,
    /// The passes the frames leave out, empty for none.
    pub(crate) skip: Vec<gg_api::Pass>,
    pub(crate) revision: u64,
    pub(crate) bare: bool,
    pub(crate) captured: Option<Shown>,
    pub(crate) seen: Option<gg_renderer::Viewpoint>,
}

#[cfg(feature = "dev")]
impl Look {
    /// What the frame shows instead of the view's light: its capture's ask, else the look's, unless
    /// the frame is bare.
    pub(crate) fn shown(&self) -> Option<Shown> {
        let look = self.show.as_ref().filter(|_| !self.bare);
        self.captured.as_ref().or(look).cloned()
    }

    /// Turns off in `frame`, as extracted from the game's environment and lights, each pass the
    /// look leaves out, unless the frame is bare. See [`gg_api::Pass`].
    pub(crate) fn leave_out(&self, frame: &mut crate::render_thread::Extracted) {
        use gg_api::Pass;
        if self.bare {
            return;
        }
        for pass in &self.skip {
            match pass {
                Pass::AmbientOcclusion => frame.ambient_occlusion = None,
                Pass::Reflections => frame.reflections = None,
                Pass::Shadows => {
                    for light in &mut frame.lights {
                        light.shadow_distance = None;
                    }
                }
                Pass::Fog => frame.fog = None,
                Pass::Bloom => frame.bloom = 0.0,
                Pass::DepthOfField => frame.depth_of_field = None,
                Pass::MotionBlur => frame.motion_blur = None,
                Pass::Antialiasing => frame.antialiasing = gg_renderer::Antialiasing::None,
                Pass::Grade => frame.grade = gg_renderer::Grade::IDENTITY,
                Pass::Lens => frame.lens = gg_renderer::Lens::NONE,
                Pass::Adaptation => frame.adaptation = None,
            }
        }
    }
}

/// Like [`gg_renderer::Show`], with an owned image name so frames take it without allocating.
#[cfg(feature = "dev")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Shown {
    Shading(gg_renderer::Channel),
    Image(std::sync::Arc<str>),
}

#[cfg(feature = "dev")]
impl Shown {
    /// From the tool API's `shown`.
    pub(crate) fn of(shown: &gg_api::Shown) -> Self {
        use gg_api::Shown as Api;
        use gg_renderer::Channel;
        let channel = match shown {
            Api::Image(name) => return Self::Image(name.as_str().into()),
            Api::BaseColor => Channel::BaseColor,
            Api::Normal => Channel::Normal,
            Api::SurfaceNormal => Channel::SurfaceNormal,
            Api::Metallic => Channel::Metallic,
            Api::Roughness => Channel::Roughness,
            Api::Occlusion => Channel::Occlusion,
            Api::Emission => Channel::Emission,
            Api::SpecularColor => Channel::SpecularColor,
            Api::DirectLight => Channel::DirectLight,
            Api::SkyLight => Channel::SkyLight,
            Api::Reflection => Channel::Reflection,
            Api::DiffuseLight => Channel::DiffuseLight,
            Api::SpecularLight => Channel::SpecularLight,
        };
        Self::Shading(channel)
    }

    /// As the renderer takes it.
    pub(crate) fn show(&self) -> gg_renderer::Show<'_> {
        match self {
            Self::Shading(channel) => gg_renderer::Show::Shading(*channel),
            Self::Image(name) => gg_renderer::Show::Image(name),
        }
    }
}

/// Starts serving the tool API for `app`, or returns `None` if `serve` is false or `dev` is off.
///
/// `layers` are bottom up. `log` is the JSON log's path, if any; its last events are in gg-core's
/// `log::EVENTS`. `player` is the peer's number in a lockstep session. `project` is the project
/// manifest's path.
///
/// Fails with `api.start` if it can't start, as when its port can't be bound or its session file
/// written. The run goes on without it, warning so, but an embedded run ends.
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

/// Has the API's debugger debug `cx`'s scripts (`luau::Hosts::debug`), before the layers load them.
pub(crate) fn debug(api: &Option<Api>, cx: &mut Context) {
    #[cfg(feature = "dev")]
    if let Some(api) = api {
        cx.hosts.debug(api.debugger());
    }
    let _ = (api, cx);
}

/// As a frame starts, before its ticks, in a development build: takes the asset database
/// (`Context::take_assets`) and the assets read again (`Context::take_reloaded`), applies the
/// scene files ([`scene_files::look`]) and scripts (`luau::look`) saved since, places the scripts'
/// breakpoints again if they changed, and answers the waiting requests ([`Api::serve`]). Returns
/// whether one asked the run to end.
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
    {
        crate::scene_files::look(cx, stack);
        crate::luau::look(cx, stack);
        cx.hosts.place();
    }
    #[cfg(feature = "dev")]
    if let Some(api) = api {
        return api.serve(cx, stack, stepper);
    }
    let _ = (api, cx, stack, stepper);
    false
}

/// Like [`serve`], between frames while they pause. Requests that need frames fail with
/// `api.paused`.
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
    {
        crate::scene_files::look(cx, stack);
        crate::luau::look(cx, stack);
        cx.hosts.place();
    }
    #[cfg(feature = "dev")]
    if let Some(api) = api {
        return api.serve_paused(cx, stack, stepper);
    }
    let _ = (api, cx, stack, stepper);
    false
}

/// As frames pause, as when the window is minimized, answers each waiting `assets` from the
/// database the watching thread last sent. A fresh look would wait for the next request.
pub(crate) fn frames_paused(api: &mut Option<Api>, cx: &mut Context) {
    #[cfg(feature = "dev")]
    if let Some(api) = api {
        cx.take_assets();
        api.frames_paused(cx);
    }
    let _ = (api, cx);
}

/// With the renderer back from a frame: takes the view it read back, answers the picks whose
/// answers came back, and has the next frame, showing the world after `ticks` ticks, read its view
/// back if captures wait or a client has it published ([`Api::frame_back`]).
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

/// The size the view's client asks frames to be while it's published, if it fits in the client's
/// memory. An embedded run renders at it. Call with the renderer back from a frame.
pub(crate) fn asked<R: Render>(render: &RenderThread<R>) -> Option<[u32; 2]> {
    #[cfg(feature = "dev")]
    if let Some(publish) = &render.frame().publish {
        return publish.writer.asked();
    }
    let _ = render;
    None
}

/// How long an embedded run waits for its client to take a frame before running on: ticks keep
/// time while the client takes none, as when its window is minimized.
#[cfg(feature = "dev")]
const PACED: Duration = Duration::from_millis(100);

/// Before the frame's handed off while the view is published: its number as the client will take
/// it, which [`pace`] waits for. `None` for an unpublished frame, as a capture of the game's own
/// view is.
pub(crate) fn handing(api: &Option<Api>) -> Option<u64> {
    #[cfg(feature = "dev")]
    if let Some(api) = api {
        return api.handing();
    }
    let _ = api;
    None
}

/// Once the frame's handed off, waits up to [`PACED`] for the client to take frame `handed`
/// ([`handing`]), so an embedded run renders no frame its client doesn't take.
pub(crate) fn pace(api: &Option<Api>, handed: u64) {
    #[cfg(feature = "dev")]
    if let Some(api) = api {
        api.pace(handed, PACED);
    }
    let _ = (api, handed);
}

/// While the simulation is paused or stopped with nothing asked of the frames, waits for a request
/// for at most a tick's time, so a headless run doesn't spin at the GPU's speed.
pub(crate) fn idle(api: &mut Option<Api>, stepper: &Stepper) {
    #[cfg(feature = "dev")]
    if let Some(api) = api
        && api.is_idle(stepper)
    {
        api.wait(crate::Tick::DT);
    }
    let _ = (api, stepper);
}

/// Has `wake` wake the main loop as each request comes, so it answers them while frames pause.
pub(crate) fn set_waker(api: &mut Option<Api>, wake: impl Fn() + Send + 'static) {
    #[cfg(feature = "dev")]
    if let Some(api) = api {
        api.set_waker(Box::new(wake));
        return;
    }
    let _ = (api, wake);
}

/// What frames have paused for, which `ping` reads from the connections' threads while the main
/// thread waits: 0 for nothing, else a [`Waiting`]. A process runs one app at a time.
static WAITING: AtomicU8 = AtomicU8::new(0);

/// What frames pause for that's the user's doing, rather than a hang.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Waiting {
    /// The main window has no area.
    Minimized = 1,
    /// A file dialog is open (development builds only).
    #[cfg(feature = "dev")]
    Dialog = 2,
    /// The window menu Windows runs in its own loop is open, as the UI's title bar opens it
    /// (development builds only).
    #[cfg(feature = "dev")]
    Menu = 3,
}

impl Waiting {
    #[cfg(feature = "dev")]
    fn now() -> Option<Self> {
        match WAITING.load(Ordering::Relaxed) {
            1 => Some(Self::Minimized),
            2 => Some(Self::Dialog),
            3 => Some(Self::Menu),
            _ => None,
        }
    }

    /// The name `ping` gives.
    #[cfg(feature = "dev")]
    fn name(self) -> &'static str {
        match self {
            Self::Minimized => "minimized",
            Self::Dialog => "dialog",
            Self::Menu => "menu",
        }
    }
}

/// Says frames have paused for `waiting` until the result drops, which restores the previous value.
pub(crate) fn waiting(waiting: Waiting) -> Waited {
    Waited(WAITING.swap(waiting as u8, Ordering::Relaxed))
}

/// [`waiting`]'s guard.
pub(crate) struct Waited(u8);

impl Drop for Waited {
    fn drop(&mut self) {
        WAITING.store(self.0, Ordering::Relaxed);
    }
}

//! The main loop's turn, a frame's, which windowed and headless runs share: running the ticks of
//! the simulation that the frame's time owes, at a fixed rate whatever the frame rate, updating
//! the layers and having them draw their UI, and handing the frame to the render thread; headless
//! runs, which render offscreen; and what a run reports as it has loaded and as it ends.

use std::mem;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use gg_core::Result;
use gg_renderer::{Arenas, MemoryBudget, Renderer, Settings, Validation, Viewport};
use tracing::{debug, error, info, warn};

use crate::api::{self, Api};
use crate::args::{Args, Net};
#[cfg(feature = "dev")]
use crate::chrome::Chrome;
#[cfg(feature = "dev")]
use crate::dock::{Dock, Ui};
#[cfg(feature = "dev")]
use crate::frame_rate;
use crate::interrupt;
use crate::layer::{Context, Stack, Tick};
use crate::memory;
use crate::perf::{self, Perf, Summary};
use crate::pipeline_cache;
use crate::render_thread::{Drawn, Render, RenderThread, TextureChanges};
use crate::snapshot::Snapshot;
use crate::{App, BUILD};

/// Renders offscreen, without a window, at 1280 by 720 pixels or the size `--size` gives, as fast
/// as the GPU allows, with a tick of the simulation each frame, so that a run of a set number of
/// frames runs as many ticks on any machine. Frames render on the render thread, as a windowed
/// run's do, the last as the run ends. `api` serves tools as each frame starts; while it has
/// paused or stopped the simulation with nothing asked of the frames, each waits up to a tick's
/// time for a request, rather than spinning. The run starts as [`start`] says, and ends at
/// `--frames`, at Ctrl+C, or, under `--supervised`, as stdin ends. A server's client
/// (`--connect`) steps its simulation in real time, its frames a tick apart, as its server's go,
/// and so does a run given `--real-time`.
///
/// Embedded (`--embedded`), it steps the simulation in real time instead, as a windowed run does,
/// with no UI, playing sound and keeping the pipeline cache as one does, and its view fills its
/// image, at the size the client it's published to asks for:
/// each frame then waits for the client to take it, as [`api::pace`] says, and until a client has
/// it published, frames come a tick apart or more.
pub(crate) fn run_headless(
    app: &mut App,
    args: &Args,
    mut api: Option<Api>,
    snapshot: Option<Snapshot>,
) -> Result<Ran> {
    let embedded = args.embedded;
    // A client's ticks keep pace with its server's, in real time, and `--real-time` paces a
    // headless run's so, as for an agent that watches it play, and a peer's, which then waits for
    // the other's inputs as a windowed peer does, its ticks owed meanwhile.
    let real_time = embedded || args.real_time || matches!(args.net, Some(Net::Connect(_)));
    let [mut width, mut height] = args.size.unwrap_or([1280, 720]);
    // An embedded run keeps the pipeline cache as a windowed run does, as a tool restarts it at
    // each change it can't patch in; the rest keep none, as `pipeline_cache` says.
    let pipelines = embedded
        .then(|| {
            pipeline_cache::path(app.name, std::env::consts::OS, |name| {
                std::env::var_os(name)
            })
        })
        .flatten();
    let cached = pipelines.as_deref().map(pipeline_cache::load);
    let settings = Settings {
        pipeline_cache: cached.as_deref(),
        ..settings(app, args)
    };
    let renderer = Renderer::headless(&settings, width, height)?;
    // The device holds what it loaded of it.
    drop(cached);
    let mut render = RenderThread::start(renderer)?;
    // An embedded run is the game a tool shows, as the editor plays it, which plays its sounds.
    let mut cx = Context::of_run(None, app, embedded);
    api::debug(&api, &mut cx);
    let mut clock = Clock::default();
    let mut stepper = match app.headless_step {
        Some(_) => Stepper::default(),
        None if real_time => Stepper::default(),
        None => Stepper::lockstep(),
    };
    #[cfg(feature = "dev")]
    let egui = crate::egui_context();
    // Headless runs neither read nor save the layout, so that they're reproducible, and have no
    // window to frame.
    #[cfg(feature = "dev")]
    let mut dock = Dock::new(
        None,
        Chrome::Os,
        app.view_panel,
        app.custom_title_bar,
        &app.docked,
    );
    #[cfg(feature = "dev")]
    let mut egui_runs = !embedded;
    // The frames handed over to render, and those rendered.
    let (mut handed, mut frames) = (0, 0);
    let started = start(
        &mut app.stack,
        &mut cx,
        &mut stepper,
        snapshot,
        args.stopped,
        api.is_some(),
    );
    let rendered = started.and_then(|()| {
        while args.frames.is_none_or(|limit| handed < limit) {
            // A frame its client takes waits for that instead, paused or stopped as much as
            // playing, as `api::pace` says.
            if !embedded || api::handing(&api).is_none() {
                api::idle(&mut api, &stepper);
            }
            if api::serve(&mut api, &mut cx, &mut app.stack, &mut stepper) {
                debug!("A tool asked the run to end");
                break;
            }
            if cx.exiting() {
                debug!("A layer asked the run to end");
                break;
            }
            if interrupt::interrupted() {
                info!("Ending the run, as Ctrl+C asks");
                break;
            }
            if interrupt::orphaned() {
                info!("Ending the run, as stdin has ended with its supervisor");
                break;
            }
            // One point is one pixel, as on a display without scaling.
            #[cfg(feature = "dev")]
            let input = egui_runs.then(|| egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width as f32, height as f32),
                )),
                max_texture_side: Some(MAX_TEXTURE_SIDE),
                time: Some(clock.elapsed().as_secs_f64()),
                ..Default::default()
            });
            // As a stopped script did, so that no frame's time counts the stop.
            if mem::take(&mut cx.paused) {
                clock.resume();
            }
            // The clock starts every frame, even with a step set: egui and the run's end read it.
            let since_last = clock.start_frame();
            let frame_started = Instant::now();
            let dt = app.headless_step.unwrap_or(since_last);
            #[cfg(feature = "dev")]
            let ui = input.map(|input| (&egui, &mut dock, input));
            #[cfg(not(feature = "dev"))]
            let ui = None;
            let pass = run_frame(&mut app.stack, &mut cx, dt, &mut stepper, ui);
            if cx.copied.take().is_some() {
                warn!(
                    code = "ui.clipboard",
                    "A layer copied text, but a headless or embedded run has no clipboard"
                );
            }
            let (ui, tessellated, view) = match pass {
                #[cfg(feature = "dev")]
                Some((mut output, layers_draw)) => {
                    egui_runs = keeps_egui(&app.stack, layers_draw, Chrome::Os, &dock);
                    let (ui, tessellated) = tessellate(&egui, &mut output);
                    let view = Viewport::from_points(dock.view(), ui.pixels_per_point);
                    (ui, tessellated, view)
                }
                None => {
                    let view = Viewport {
                        origin: [0, 0],
                        size: [width, height],
                    };
                    (MainUi::none(), Duration::ZERO, view)
                }
            };
            if take_back(&mut render, &mut cx, tessellated)?.is_some() {
                frames += 1;
            }
            api::frame_back(&mut api, &mut render, &cx, stepper.ticks);
            let mut view = view;
            if embedded {
                if let Some(asked) = api::asked(&render)
                    && asked != [width, height]
                {
                    [width, height] = asked;
                    render.renderer().resize(width, height);
                }
                view = Viewport {
                    origin: [0, 0],
                    size: [width, height],
                };
            }
            let handing = api::handing(&api).filter(|_| embedded);
            hand_off(&mut render, &mut cx, view, ui, Vec::new());
            handed += 1;
            match handing {
                Some(frame) => api::pace(&api, frame),
                None if real_time => {
                    let tick = frame_started + crate::Tick::DT;
                    std::thread::sleep(tick.saturating_duration_since(Instant::now()));
                }
                None => {}
            }
            // Rather than spinning while a lockstep peer's inputs are on their way, its frames
            // unpaced.
            if !real_time && app.stack.net_waits(stepper.ticks) {
                std::thread::sleep(Duration::from_millis(1));
            }
            app.stack.recorded()?;
        }
        Ok(())
    });
    // The last frame ends here, and the frame handed over last renders before the run's end is
    // timed.
    cx.perf.finish(clock.since_last(Instant::now()));
    let (renderer, last) = render.stop();
    let rendered = rendered.and_then(|()| {
        if let Some(last) = last {
            last.presented?;
            frames += 1;
        }
        Ok(())
    });
    let stopping = Instant::now();
    let _zone = gg_core::zone!("shutdown");
    app.stack.detach(&mut cx);
    if let Some(path) = &pipelines {
        pipeline_cache::save(path, &renderer);
    }
    rendered?;
    app.stack.recorded()?;
    let perf = mem::take(&mut cx.perf);
    Ok(Ran::finish(
        Some(renderer),
        frames,
        &clock,
        &stepper,
        perf,
        stopping,
    ))
}

/// Runs a server (`--serve`): without a window, a GPU or the tool API, which looks at a run's
/// frames, its frames a tick apart, each taking what its clients sent and running the ticks owed in
/// real time, as [`Stack::net_turn`] and [`Stepper::step`] say, until `--frames` frames, Ctrl+C,
/// stdin's end under `--supervised` or a layer's [`Context::exit`]. The layers load their assets,
/// which nothing draws, and their ticks run, but not their updates, whose drawing nothing would
/// take. The run starts as [`start`] says.
pub(crate) fn run_server(app: &mut App, args: &Args, snapshot: Option<Snapshot>) -> Result<Ran> {
    let mut cx = Context::of_run(None, app, false);
    let mut clock = Clock::default();
    let mut stepper = Stepper::default();
    start(
        &mut app.stack,
        &mut cx,
        &mut stepper,
        snapshot,
        false,
        false,
    )?;
    let mut frames = 0;
    while args.frames.is_none_or(|limit| frames < limit) {
        if cx.exiting() {
            debug!("A layer asked the run to end");
            break;
        }
        if interrupt::interrupted() {
            info!("Ending the run, as Ctrl+C asks");
            break;
        }
        if interrupt::orphaned() {
            info!("Ending the run, as stdin has ended with its supervisor");
            break;
        }
        let dt = clock.start_frame();
        let start = Instant::now();
        gg_core::profile::frame_mark();
        cx.perf.start_frame(dt);
        if let Some(ticks) = app.stack.net_turn(&mut cx, stepper.ticks) {
            stepper.jump_to(ticks);
        }
        let stack = &mut app.stack;
        let tick = |tick: Tick| stack.tick(&mut cx, tick);
        stepper.step(dt, tick, || start.elapsed());
        let ticked = start.elapsed();
        cx.perf.ran(ticked, Duration::ZERO, Duration::ZERO);
        frames += 1;
        app.stack.net_wait(start + crate::Tick::DT);
    }
    cx.perf.finish(clock.since_last(Instant::now()));
    let stopping = Instant::now();
    let _zone = gg_core::zone!("shutdown");
    app.stack.detach(&mut cx);
    let perf = mem::take(&mut cx.perf);
    Ok(Ran::finish(None, frames, &clock, &stepper, perf, stopping))
}

/// Starts the run in `cx`, windowed or not: from `snapshot`'s world, at its ticks, if it's given
/// one, rather than the one `stack`'s layers attach, stopped or playing as the snapshot's run was,
/// as [`Stack::restore`] says, given whether a tool API `serves` the run, and paused if it was in a
/// development build, which alone pauses, dropping it once it has;
/// or else from the world the layers attach, stopped if `stopped` says so, as `--stopped` asks.
/// In a development build it first scans the assets under the app's root, before the layers load,
/// and has a thread watch them if the API serves the run, as `Context::start_assets` says, writing
/// the ID files missing, and moving those renames left behind, if the API serves the run and it
/// isn't a lockstep session's joiner, whose host alone writes them, and whose textures go by the
/// names its host gives them, by their paths until the host's world comes. The snapshot's textures
/// are then found where the run's database has their IDs, as after a rename, as
/// `Snapshot::follow` says.
///
/// Fails as `Stack::restore` does.
pub(crate) fn start(
    stack: &mut Stack,
    cx: &mut Context,
    stepper: &mut Stepper,
    snapshot: Option<Snapshot>,
    stopped: bool,
    serves: bool,
) -> Result<()> {
    #[cfg(feature = "dev")]
    let mut snapshot = snapshot;
    #[cfg(feature = "dev")]
    {
        // Whether it hosts or plays alone, rather than joins.
        let own = stack.watches_files();
        cx.start_assets(serves, serves && own);
        if own && let Some(snapshot) = &mut snapshot {
            snapshot.follow(cx.catalog());
        }
        if !own {
            cx.name_as([]);
        }
        // Before the layers load, so that music they have play as they load waits for the run to
        // play; the first frame follows a snapshot's run that plays all the same.
        let snapshot = snapshot.as_ref();
        let paused = snapshot.is_some_and(|snapshot| snapshot.paused);
        cx.follow_playback(
            stopped || snapshot.is_some_and(|snapshot| snapshot.stopped),
            paused,
        );
    }
    let (stopped, paused) = match snapshot {
        Some(snapshot) => {
            let (ticks, stopped) = stack.restore(cx, &snapshot, serves)?;
            stepper.start_at(ticks);
            (stopped, snapshot.paused)
        }
        None => {
            stack.attach(cx);
            (stopped, false)
        }
    };
    #[cfg(feature = "dev")]
    if stopped {
        stepper.stop();
    }
    #[cfg(feature = "dev")]
    if paused {
        stepper.pause();
    }
    let _ = (stopped, paused);
    Ok(())
}

/// The most pixels a side of egui's textures may have, the most every device the engine runs on
/// takes, which each pass's input tells egui: every window's alike, since egui makes its font atlas
/// again for a pass whose limit isn't the last one's.
#[cfg(feature = "dev")]
pub(crate) const MAX_TEXTURE_SIDE: usize = gg_renderer::MAX_TEXTURE_SIZE as usize;

/// What [`run_frame`] runs a pass of egui's with: egui, the dock and egui's input.
#[cfg(feature = "dev")]
pub(crate) type EguiPass<'a> = (&'a egui::Context, &'a mut Dock, egui::RawInput);

/// What a pass of egui's that [`run_frame`] ran made: its output, and whether any layer draws UI,
/// as [`Stack::ui`] says.
#[cfg(feature = "dev")]
pub(crate) type Passed = (egui::FullOutput, bool);

/// What [`run_frame`] would run a pass of egui's with, of which a build without `dev`, which has
/// no UI, has none: its frames run no pass.
#[cfg(not(feature = "dev"))]
pub(crate) type EguiPass<'a> = (std::convert::Infallible, std::marker::PhantomData<&'a ()>);

/// What a pass of egui's would make, of which a build without `dev` has none.
#[cfg(not(feature = "dev"))]
pub(crate) type Passed = std::convert::Infallible;

/// The first half of a turn of the main loop: starts the frame, `dt` after the last, for Tracy and
/// the frame's measure, takes what the session sent, as [`Stack::net_turn`] says, runs the ticks of
/// the simulation that the time owes, as `stepper` steps it, a peer's a little less and a client's a
/// little more or less, as [`Stack::paced`] says, those a lockstep peer's inputs have come for,
/// copying where the world's entities are as each starts, updates the layers, draws the world after
/// what they drew, as far on from where the last tick found it to where it left it as the frame is
/// past the tick, a client's other players as [`Stack::interpolate`] says, and, given egui, the
/// dock and egui's input in `ui`, adds the engine's settings, as [`frame_rate::settings`] does, and
/// has the layers draw their UI, in the dock and with egui. Returns that pass's output, and whether
/// any layer draws UI, as [`Stack::ui`] says. [`tessellate`] starts the second half, which hands
/// what egui made of it to the render thread.
///
/// egui allocates as it lays out here, and as [`tessellate`] tessellates, so frames that run its
/// pass allocate; the UI is a development overlay, and an app that draws none stops running it,
/// as [`keeps_egui`] says. A build without `dev`, which has no UI, runs none.
pub(crate) fn run_frame(
    stack: &mut Stack,
    cx: &mut Context,
    dt: Duration,
    stepper: &mut Stepper,
    ui: Option<EguiPass<'_>>,
) -> Option<Passed> {
    gg_core::profile::frame_mark();
    cx.perf.start_frame(dt);
    let _zone = gg_core::zone!();
    let start = Instant::now();
    #[cfg(feature = "dev")]
    {
        cx.started = std::time::SystemTime::now();
    }
    stack.take_asked(cx);
    if let Some(ticks) = stack.net_turn(cx, stepper.ticks) {
        stepper.jump_to(ticks);
    }
    cx.session = stack.session_status();
    let owed = stack.paced(dt);
    // As the requests served before the frame left the run, before its ticks queue sounds.
    #[cfg(feature = "dev")]
    cx.follow_playback(stepper.stopped(), stepper.paused());
    let tick = |tick: Tick| {
        if stack.net_waits(tick.number) {
            return false;
        }
        cx.capture();
        stack.tick(cx, tick)
    };
    stepper.step(owed, tick, || start.elapsed());
    // A world edited while no tick runs has its cells come and go around the look's viewpoint.
    #[cfg(feature = "dev")]
    if stepper.stopped() {
        cx.stream_stopped();
    }
    // A script that stopped in a tick paused the frames while it waited.
    #[cfg(feature = "dev")]
    if cx.hosts.took_stop() {
        cx.paused = true;
    }
    let ticked = Instant::now();
    #[cfg(feature = "dev")]
    cx.follow_look();
    stack.update(cx, dt);
    stack.interpolate(cx, dt);
    let alpha = stepper.alpha();
    gg_core::plot!("alpha", f64::from(alpha));
    cx.draw_world(alpha, stepper.time(alpha), stepper.ticks);
    let updated = Instant::now();
    #[cfg(feature = "dev")]
    let pass = ui.map(|(egui, dock, input)| {
        let _zone = gg_core::zone!("egui");
        let mut layers_draw = false;
        // egui may run the pass again in a frame, as it asks for one.
        let output = egui.run_ui(input, |root| {
            dock.begin(root);
            let mut ui = Ui::new(root, dock);
            // The engine's settings come before the layers'.
            frame_rate::settings(&mut ui, cx);
            layers_draw |= stack.ui(cx, &mut ui);
            dock.end(root);
        });
        (output, layers_draw)
    });
    #[cfg(not(feature = "dev"))]
    let pass = ui.map(|(never, _)| match never {});
    cx.perf
        .ran(ticked - start, updated - ticked, updated.elapsed());
    pass
}

/// Whether egui runs from the next frame on, after a pass in which `layers_draw` says whether any
/// of `stack`'s layers draw UI, in windows framed as `chrome` says. Once none does, the OS frames
/// the windows, and the game's view isn't in `dock`'s panel, egui would show nothing, so it stops
/// for the rest of the run: whether a layer draws UI is its type's, which no later frame changes.
#[cfg(feature = "dev")]
pub(crate) fn keeps_egui(stack: &Stack, layers_draw: bool, chrome: Chrome, dock: &Dock) -> bool {
    let keeps = layers_draw || chrome != Chrome::Os || dock.is_dockspace();
    if !keeps {
        debug!(
            layers = ?stack.names(),
            "No layer draws UI, so frames run without egui from the next"
        );
    }
    keeps
}

/// What the main window's pass drew in a frame, tessellated, for the render thread: nothing in a
/// build without `dev`, which runs no pass.
pub(crate) struct MainUi {
    primitives: Vec<egui::ClippedPrimitive>,
    textures: TextureChanges,
    pub(crate) pixels_per_point: f32,
}

impl MainUi {
    /// Nothing, from a frame that ran no pass.
    pub(crate) fn none() -> Self {
        Self {
            primitives: Vec::new(),
            textures: TextureChanges::default(),
            pixels_per_point: 1.0,
        }
    }
}

/// The second half of a turn of the main loop starts here, while the frame before renders:
/// tessellates the UI the main window's pass drew, taking it and its texture changes from `output`,
/// and returns it, and how long that took.
#[cfg(feature = "dev")]
pub(crate) fn tessellate(
    egui: &egui::Context,
    output: &mut egui::FullOutput,
) -> (MainUi, Duration) {
    let _zone = gg_core::zone!("tessellate UI");
    let start = Instant::now();
    let textures = TextureChanges(mem::take(&mut output.textures_delta));
    let primitives = egui.tessellate(mem::take(&mut output.shapes), output.pixels_per_point);
    let ui = MainUi {
        primitives,
        textures,
        pixels_per_point: output.pixels_per_point,
    };
    (ui, start.elapsed())
}

/// Takes the renderer back from rendering the frame before, which this frame, having tessellated
/// its own UI in `tessellated`, is measured with, and the answers to picks that came in as it
/// rendered, which `cx` gives in place of the last, and what it left streaming to the GPU, which
/// [`Context::streaming`] tells, logging `engine.loaded` once nothing is, as [`loaded`] says; and
/// returns whether that frame presented: `None` if no frame was rendering.
pub(crate) fn take_back<R: Render>(
    render: &mut RenderThread<R>,
    cx: &mut Context,
    tessellated: Duration,
) -> Result<Option<bool>> {
    let start = Instant::now();
    let rendered = render.take();
    cx.perf
        .rendered(tessellated, start.elapsed(), rendered.as_ref());
    let answers = if rendered.is_some() {
        cx.streamed = render.frame().streaming;
        if let Some(started) = cx.take_loaded() {
            loaded(started, render.renderer().memory_budget());
        }
        &render.frame().picked[..]
    } else {
        &[]
    };
    cx.take_answers(answers);
    rendered.map(|rendered| rendered.presented).transpose()
}

/// Logs `engine.loaded`, once a run, at the first frame back after which nothing is decoding or
/// streaming once the layers have attached: the milliseconds since the engine `started`, the
/// process's resident memory and its peak, and the GPU's own memory in use and its budget, as `gpu`
/// gives them. Each count the OS or the GPU doesn't give is left out.
fn loaded(started: Instant, gpu: Option<MemoryBudget>) {
    let memory = memory::resident();
    info!(
        code = "engine.loaded",
        load_ms = perf::millis(started.elapsed()),
        memory_bytes = memory.now,
        memory_peak_bytes = memory.peak,
        gpu_memory_bytes = gpu.map(|gpu| gpu.usage),
        gpu_budget_bytes = gpu.map(|gpu| gpu.budget),
        "Loaded"
    );
}

/// Hands the frame over to the render thread: what the layers drew in the game's view, `view` in
/// the main window's pixels, the UI in `ui` over it, and what the windows of their own drew, in
/// `windows`.
pub(crate) fn hand_off<R: Render>(
    render: &mut RenderThread<R>,
    cx: &mut Context,
    view: Viewport,
    ui: MainUi,
    windows: Vec<Drawn>,
) {
    render.hand(|frame| {
        cx.extract(frame, view);
        frame.primitives = ui.primitives;
        frame.textures = ui.textures;
        frame.pixels_per_point = ui.pixels_per_point;
        frame.windows = windows;
    });
}

/// Measures frames: the time between them, and since the first.
#[derive(Default)]
pub(crate) struct Clock {
    first: Option<Instant>,
    last: Option<Instant>,
}

impl Clock {
    /// Starts a frame, and returns the time since the last one started; zero for the first, and
    /// for the first after [`Clock::resume`].
    pub(crate) fn start_frame(&mut self) -> Duration {
        let now = Instant::now();
        let dt = self.last.map_or(Duration::ZERO, |last| now - last);
        self.first.get_or_insert(now);
        self.last = Some(now);
        dt
    }

    /// Has the next frame count from its own start, as the first does, across a pause in which no
    /// frames run, such as while the window is minimized, as the pause starts and as it ends:
    /// otherwise the layers would take the pause for one long frame, and so would the frame times.
    pub(crate) fn resume(&mut self) {
        self.last = None;
    }

    /// The time since the first frame started, which egui's input gives as its time; zero before
    /// it.
    #[cfg(any(feature = "dev", test))]
    fn elapsed(&self) -> Duration {
        self.first.map_or(Duration::ZERO, |first| first.elapsed())
    }

    /// The time from the last frame's start to `at`; zero before the first frame.
    pub(crate) fn since_last(&self, at: Instant) -> Duration {
        self.last
            .map_or(Duration::ZERO, |last| at.saturating_duration_since(last))
    }
}

/// The most ticks a frame runs: 125 ms of the simulation. Past them, the simulation falls behind
/// real time, rather than taking the frame rate down with it.
const MOST_TICKS: u64 = 8;

/// Turns frames' time into the simulation's ticks, as many as the time owed holds, so that the
/// simulation steps at its fixed rate whatever the frame rate; or, paused or stopped, as the tool
/// API asks.
#[derive(Default)]
pub(crate) struct Stepper {
    /// The time not yet simulated: less than a tick, between frames.
    owed: Duration,
    /// The ticks run, which number the next: the world's, which go back as the tool API's `stop`
    /// brings back the world as edited.
    pub(crate) ticks: u64,
    /// The ticks the run has run, in whichever world, since it started.
    ticked: u64,
    /// Whether the run started from a snapshot, rather than from the world its layers attached.
    #[cfg_attr(
        not(feature = "dev"),
        allow(dead_code, reason = "only the tool API asks")
    )]
    restored: bool,
    /// The whole ticks the time owed held that no frame ran.
    dropped: u64,
    /// One tick every frame, whatever its time.
    lockstep: bool,
    /// Set while the tool API has paused the simulation, whose frames then run only the ticks
    /// `steps` asks for, and draw the world as the last tick left it.
    paused: bool,
    /// Set while the tool API has stopped the simulation, whose frames then run no ticks, and draw
    /// the world as edits leave it. Only development builds stop.
    stopped: bool,
    /// The ticks still to run while paused, at most [`MOST_TICKS`] a frame.
    steps: u64,
}

impl Stepper {
    /// A tick every frame, as headless runs step, so that a run of a set number of frames runs as
    /// many ticks, however fast its frames are.
    fn lockstep() -> Self {
        Self {
            lockstep: true,
            ..Self::default()
        }
    }

    /// Numbers the ticks on from `ticks`, those a snapshot that the run starts from had run.
    pub(crate) fn start_at(&mut self, ticks: u64) {
        self.ticks = ticks;
        self.restored = true;
    }

    /// Numbers the ticks on from `ticks`, those of a world that took the world's place as the run
    /// went on, as a lockstep joiner's world gives way to its host's, or the world as edited comes
    /// back at the tool API's `stop`, owing no time.
    pub(crate) fn jump_to(&mut self, ticks: u64) {
        self.ticks = ticks;
        self.owed = Duration::ZERO;
    }

    /// The ticks the run has run, in whichever world, since it started.
    pub(crate) fn ran(&self) -> u64 {
        self.ticked
    }

    /// Whether the run started afresh, from the world its layers attached, and hasn't ticked
    /// since: what they queued as they attached is still to apply, at its first tick. A run
    /// restored from a snapshot attached nothing.
    #[cfg(feature = "dev")]
    pub(crate) fn fresh(&self) -> bool {
        !self.restored && self.ticked == 0
    }

    /// Runs, through `tick`, the ticks owed by a frame `dt` after the last, numbered on from the
    /// last: one for each whole tick the time owed holds, at most [`MOST_TICKS`], and no more once
    /// `took`, the time the frame's ticks have taken so far, is longer than the time they've
    /// simulated, so that a simulation slower than real time slows down rather than spending every
    /// frame catching up. The whole ticks not run are dropped, and the fraction of one left over
    /// carries to the next frame. In lockstep, exactly one. Paused, those the steps asked for,
    /// whatever the time, as many as the time they take allows, the rest left for the next frame.
    /// A tick that `tick` doesn't run, returning `false`, as one waiting for a lockstep peer's
    /// inputs, stops the frame's ticks, and those owed are owed still, up to [`MOST_TICKS`] of them,
    /// rather than dropped. Stopped, none, owing no time and dropping none. Returns how many ran.
    pub(crate) fn step(
        &mut self,
        dt: Duration,
        mut tick: impl FnMut(Tick) -> bool,
        took: impl Fn() -> Duration,
    ) -> u64 {
        if self.stopped {
            return 0;
        }
        let due = if self.paused {
            self.steps
        } else if self.lockstep {
            1
        } else {
            self.owed += dt;
            let step = Tick::DT.as_nanos();
            let owed = self.owed.as_nanos();
            self.owed = Duration::from_nanos((owed % step) as u64);
            (owed / step) as u64
        };
        let mut ran = 0;
        let mut waiting = false;
        while ran < due.min(MOST_TICKS) {
            if !tick(Tick { number: self.ticks }) {
                waiting = true;
                break;
            }
            self.ticks += 1;
            self.ticked += 1;
            ran += 1;
            // Slower than real time: the ticks left would only fall further behind.
            if took() > Tick::DT * ran as u32 {
                break;
            }
        }
        if self.paused {
            self.steps -= ran;
        } else if waiting && !self.lockstep {
            let owed = self.owed + Tick::DT * (due - ran) as u32;
            self.owed = owed.min(Tick::DT * MOST_TICKS as u32);
        } else if !waiting {
            self.dropped += due - ran;
        }
        ran
    }

    /// How far the frame is past its last tick, as a fraction of a tick, from 0 to less than 1,
    /// which the world draws that far on from where the tick found it to where it left it; 1 in
    /// lockstep, so that a headless frame draws its tick as it leaves the world, and while paused
    /// or stopped, so that a frame draws the world as the tool API reads it.
    fn alpha(&self) -> f32 {
        if self.lockstep || self.paused || self.stopped {
            return 1.0;
        }
        // Past a tick while waiting for a lockstep peer, which draws the last tick as it left it.
        (self.owed.as_nanos() as f64 / Tick::DT.as_nanos() as f64).min(1.0) as f32
    }

    /// The simulation's time, in seconds, as a frame draws the world `alpha` of the way on from
    /// where the latest tick found it to where it left it: 0 before any tick. It goes by the
    /// world's ticks, so it stops while the simulation does, and a replay's frames draw at the
    /// times its run's did.
    fn time(&self, alpha: f32) -> f32 {
        let ticks = (self.ticks as f64 - 1.0 + f64::from(alpha)).max(0.0);
        (ticks * Tick::DT.as_secs_f64()) as f32
    }

    /// Pauses the simulation, as the tool API's `pause` asks: frames run no ticks but those
    /// [`step_by`](Self::step_by) asks for, and the time they take isn't owed.
    #[cfg(feature = "dev")]
    pub(crate) fn pause(&mut self) {
        self.paused = true;
        self.owed = Duration::ZERO;
    }

    /// Runs the simulation in real time again, dropping the steps not yet run, as the tool API's
    /// `resume` asks. The first frame after owes only its own time, so that the simulation doesn't
    /// catch up with the pause.
    #[cfg(feature = "dev")]
    pub(crate) fn resume(&mut self) {
        self.paused = false;
        self.steps = 0;
        self.owed = Duration::ZERO;
    }

    /// Pauses the simulation, and has the frames from the next run `ticks` more ticks, as the tool
    /// API's `step` asks.
    #[cfg(feature = "dev")]
    pub(crate) fn step_by(&mut self, ticks: u64) {
        self.pause();
        self.steps += ticks;
    }

    /// Whether the simulation is paused.
    #[cfg(feature = "dev")]
    pub(crate) fn paused(&self) -> bool {
        self.paused
    }

    /// The ticks still to run while paused.
    #[cfg(feature = "dev")]
    pub(crate) fn steps(&self) -> u64 {
        self.steps
    }

    /// Stops the simulation, as the tool API's `stop` asks: frames run no ticks, whatever their
    /// time, and the steps not yet run are dropped, until [`play`](Self::play). A pause lasts
    /// across it.
    #[cfg(feature = "dev")]
    pub(crate) fn stop(&mut self) {
        self.stopped = true;
        self.owed = Duration::ZERO;
        self.steps = 0;
    }

    /// Runs the simulation again after [`stop`](Self::stop), as the tool API's `play` asks: in
    /// real time, or paused if it was, the first frame after owing only its own time.
    #[cfg(feature = "dev")]
    pub(crate) fn play(&mut self) {
        self.stopped = false;
        self.owed = Duration::ZERO;
    }

    /// Whether the simulation is stopped.
    #[cfg(feature = "dev")]
    pub(crate) fn stopped(&self) -> bool {
        self.stopped
    }
}

pub(crate) fn settings<'a>(app: &'a App, args: &'a Args) -> Settings<'a> {
    Settings {
        app_name: app.name,
        validation: args.validation,
        gpu: args.gpu.as_deref(),
        vsync: args.vsync,
        uploads: None,
        mesh_shaders: None,
        visibility_buffer: true,
        upload_budget: gg_renderer::DEFAULT_UPLOAD_BUDGET,
        pipeline_cache: None,
        capture: args.capture,
    }
}

/// What a run did, reported when it ends.
pub(crate) struct Ran {
    frames: u64,
    /// The ticks the simulation ran in the run, in whichever world: past a snapshot's, in a run
    /// restored from one.
    ticks: u64,
    /// The whole ticks the frames' time held that they didn't run, which the simulation fell
    /// behind by.
    ticks_dropped: u64,
    /// How the run was validated, the GPU it rendered on, and the GPU's own memory in use as the
    /// run ended, before the renderer let go of its own: none for a server, which renders nothing.
    validation: Option<Validation>,
    gpu: Option<String>,
    gpu_memory: Option<u64>,
    /// What the frames' arenas held as the run ended, and the most a frame took of its own.
    arenas: Arenas,
    /// The GPU memory the frames' own images, the view's among them, were in as the run ended.
    transient_bytes: u64,
    /// The most of the process's memory that was resident, if the OS says.
    memory_peak: Option<u64>,
    /// When its first frame started, if one did.
    first_frame_at: Option<Instant>,
    shutdown: Duration,
    perf: Summary,
}

impl Ran {
    /// Shuts the renderer down first, so that what validation reports on the way, such as
    /// objects left undestroyed, counts too, once it has told what of the GPU's memory is in use
    /// and what the frames' arenas hold, given one: a server's run has none. `stopping` is when the
    /// run began to end, after its last frame rendered, `stepper` what stepped its simulation, and
    /// `perf` what it measured of its frames, the last of them finished.
    pub(crate) fn finish(
        renderer: Option<Renderer>,
        frames: u64,
        clock: &Clock,
        stepper: &Stepper,
        perf: Perf,
        stopping: Instant,
    ) -> Self {
        let validation = renderer.as_ref().map(Renderer::validation);
        let gpu = renderer
            .as_ref()
            .map(|renderer| renderer.gpu_name().to_owned());
        let gpu_memory = renderer
            .as_ref()
            .map(|renderer| renderer.memory_budget().usage);
        let arenas = renderer.as_ref().map(Renderer::arenas).unwrap_or_default();
        let transient_bytes = (renderer.as_ref())
            .map(Renderer::transient_bytes)
            .unwrap_or_default();
        drop(renderer);
        Self {
            frames,
            ticks: stepper.ran(),
            ticks_dropped: stepper.dropped,
            validation,
            gpu,
            gpu_memory,
            arenas,
            transient_bytes,
            memory_peak: memory::resident().peak,
            first_frame_at: clock.first,
            shutdown: stopping.elapsed(),
            perf: perf.summary(),
        }
    }

    /// Logs how long the run's frames took, and what memory it held, then how it ended:
    /// `engine.perf`, whose times are in milliseconds, comes first, so that a run that fails
    /// validation has it too, and one that `failed`, as a layer ends one through
    /// [`Context::fail`], which logged why. `started` is when the engine started, and `args` how it
    /// ran.
    pub(crate) fn report(self, started: Instant, args: &Args, failed: bool) -> ExitCode {
        let perf = &self.perf;
        info!(
            code = "engine.perf",
            frames = self.frames,
            ticks = self.ticks,
            ticks_dropped = self.ticks_dropped,
            measured = perf.measured,
            percentile_frames = perf.percentile_frames,
            startup_ms = self
                .first_frame_at
                .map(|first| perf::millis(first.saturating_duration_since(started))),
            first_frame_ms = perf.first_frame,
            shutdown_ms = perf::millis(self.shutdown),
            frame_ms_p50 = perf.frame.p50,
            frame_ms_p95 = perf.frame.p95,
            frame_ms_p99 = perf.frame.p99,
            frame_ms_mean = perf.frame.mean,
            frame_ms_max = perf.frame.max,
            ticks_ms_mean = perf.ticks.mean,
            ticks_ms_p95 = perf.ticks.p95,
            update_ms_mean = perf.update.mean,
            update_ms_p95 = perf.update.p95,
            ui_ms_mean = perf.ui.mean,
            ui_ms_p95 = perf.ui.p95,
            handoff_ms_mean = perf.handoff.mean,
            handoff_ms_p95 = perf.handoff.p95,
            limit_ms_mean = perf.limit.mean,
            limit_ms_p95 = perf.limit.p95,
            render_ms_mean = perf.render.mean,
            render_ms_p95 = perf.render.p95,
            wait_ms_mean = perf.wait.mean,
            wait_ms_p95 = perf.wait.p95,
            publish_ms_mean = perf.publish.mean,
            publish_ms_p95 = perf.publish.p95,
            gpu_ms_mean = perf.gpu.mean,
            gpu_ms_p95 = perf.gpu.p95,
            draws_max = perf.draws_max,
            instances_max = perf.instances_max,
            triangles_max = perf.triangles_max,
            memory_peak_bytes = self.memory_peak,
            gpu_memory_bytes = self.gpu_memory,
            arena_bytes = self.arenas.held,
            arena_peak_bytes = self.arenas.peak,
            transient_bytes = self.transient_bytes,
            gpu = self.gpu.as_deref(),
            build = BUILD,
            headless = args.headless,
            embedded = args.embedded,
            vsync = args.vsync,
            validated = self.validated(),
            "Frame times"
        );
        // A frame past `--frames` is refused, but a window closed early ends a run before it.
        if let Some(frame) = args.capture
            && self.frames < frame.get()
        {
            warn!(
                code = "engine.not_captured",
                frame = frame.get(),
                frames = self.frames,
                "Frame {frame} wasn't captured: the run ended after {} frames",
                self.frames
            );
        }
        let messages = self.validation.as_ref().map_or(0, Validation::messages);
        if messages > 0 {
            error!(
                code = "engine.validation",
                messages,
                "Vulkan validation reported {messages} problem(s); the warnings and errors above say what"
            );
            return ExitCode::FAILURE;
        }
        if failed {
            return ExitCode::FAILURE;
        }
        info!(
            frames = self.frames,
            validated = self.validated(),
            "Exited cleanly"
        );
        ExitCode::SUCCESS
    }

    /// Whether the run ran under Vulkan's validation layer: a server's, which renders nothing,
    /// didn't.
    fn validated(&self) -> bool {
        self.validation.as_ref().is_some_and(Validation::enabled)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::{Cell, RefCell};
    use std::f32::consts::FRAC_PI_2;
    use std::iter;
    use std::rc::Rc;

    use gg_core::math::{DVec3, Mat4, Quat, Vec3, Vec4};
    use gg_renderer::Draws;

    use super::*;
    #[cfg(feature = "dev")]
    use crate::Inspector;
    use crate::{
        Align, Camera, Entity, Layer, Projection, Sprite, Transform, Viewpoint, World,
        render_thread,
    };

    /// Spawns a red sprite as it attaches, and draws a sprite of its own each update, and another
    /// on the canvas, 100 units across, 50 left of and below the view's top right.
    struct Drawer;

    /// Where [`Drawer`]'s sprite on the canvas is from the view's top right.
    fn top_right() -> Mat4 {
        Mat4::from_translation(Vec3::new(-50.0, -50.0, 0.0)) * Mat4::from_scale(Vec3::splat(100.0))
    }

    pub(crate) fn red() -> Sprite {
        Sprite {
            color: Vec4::new(1.0, 0.0, 0.0, 1.0),
            ..Sprite::default()
        }
    }

    impl Layer for Drawer {
        fn attach(&mut self, cx: &mut Context) {
            cx.world_mut().spawn((Transform::IDENTITY, red()));
        }

        fn update(&mut self, cx: &mut Context, _: Duration) {
            cx.draw_sprite(Mat4::IDENTITY, Sprite::default());
            let mut canvas = cx.canvas([Align::End, Align::Start]);
            canvas.draw_sprite(top_right(), Sprite::default());
        }
    }

    /// Spawns two sprites as it attaches: the slider, at x 0, which moves 1 in x at each tick, and
    /// the jumper, at x 10, which jumps 90 at tick 3, resetting its interpolation; and spawns one
    /// at x 5 at tick 2. Each has a z of its own, so that they draw in that order.
    #[derive(Default)]
    struct Slider {
        slider: Option<Entity>,
        jumper: Option<Entity>,
    }

    /// Where [`Slider`]'s sprites are: at `x`, and at their own `z`.
    fn slid(x: f32, z: f32) -> Transform {
        Transform {
            translation: Vec3::new(x, 0.0, z).as_dvec3(),
            ..Transform::IDENTITY
        }
    }

    impl Layer for Slider {
        fn attach(&mut self, cx: &mut Context) {
            let world = cx.world_mut();
            self.slider = Some(world.spawn((slid(0.0, 0.0), red())));
            self.jumper = Some(world.spawn((slid(10.0, 0.1), red())));
        }

        fn tick(&mut self, world: &mut World, tick: Tick) {
            let [slider, jumper] = [self.slider, self.jumper].map(|it| it.expect("attached"));
            world
                .get_mut::<Transform>(slider)
                .expect("alive")
                .translation
                .x += 1.0;
            match tick.number {
                2 => {
                    world.spawn((slid(5.0, 0.2), red()));
                }
                3 => {
                    world
                        .get_mut::<Transform>(jumper)
                        .expect("alive")
                        .translation
                        .x += 90.0;
                    world.reset_interpolation(jumper).expect("alive");
                }
                _ => {}
            }
        }
    }

    /// Runs a frame `dt` after the last, of a 640 by 360 view, and returns what it drew.
    fn drawn(stack: &mut Stack, cx: &mut Context, dt: Duration, stepper: &mut Stepper) -> Draws {
        assert!(run_frame(stack, cx, dt, stepper, None).is_none());
        let mut frame = render_thread::Extracted::default();
        let viewport = Viewport {
            origin: [0, 0],
            size: [640, 360],
        };
        cx.extract(&mut frame, viewport);
        frame.draws
    }

    /// What [`Slider`]'s sprites in `world` draw as: the slider at `slider`, the jumper at
    /// `jumper`, and the one spawned at tick 2, once it's spawned, each with its entity's ID,
    /// found by its z.
    fn slider_draws(world: &World, slider: f32, jumper: f32, spawned: bool) -> Draws {
        let id = |z: f32| {
            let mut placed = world.query::<(Entity, &Transform)>();
            let found = placed.find(|(_, at)| at.translation.z == f64::from(z));
            let (entity, _) = found.expect("a sprite at each z");
            world.id(entity).expect("alive").get()
        };
        let mut want = Draws::default();
        want.draw_scene();
        let mut sprite = |x: f32, z: f32| {
            want.extend_sprites(iter::once((
                slid(x, z).matrix_from(DVec3::ZERO),
                red(),
                id(z),
            )));
        };
        sprite(slider, 0.0);
        sprite(jumper, 0.1);
        if spawned {
            sprite(5.0, 0.2);
        }
        want
    }

    /// Spawns an entity as it attaches, which it turns a quarter turn a second at each tick, and
    /// notes each tick it gets.
    pub(crate) struct Turner(pub(crate) Rc<RefCell<Vec<Tick>>>);

    impl Layer for Turner {
        fn attach(&mut self, cx: &mut Context) {
            cx.world_mut().spawn(Transform::IDENTITY);
        }

        fn tick(&mut self, world: &mut World, tick: Tick) {
            let turn = Quat::from_rotation_z(FRAC_PI_2 * Tick::DT.as_secs_f32());
            for transform in world.query_mut::<&mut Transform>() {
                transform.rotation = (turn * transform.rotation).normalize();
            }
            self.0.borrow_mut().push(tick);
        }
    }

    /// Notes its ticks and its updates, in the order they come.
    struct Order(Rc<RefCell<Vec<String>>>);

    impl Layer for Order {
        fn tick(&mut self, _: &mut World, tick: Tick) {
            self.0.borrow_mut().push(format!("tick {}", tick.number));
        }

        fn update(&mut self, _: &mut Context, _: Duration) {
            self.0.borrow_mut().push("update".to_owned());
        }
    }

    #[test]
    fn the_simulation_steps_at_a_fixed_rate_whatever_the_frame_rate() {
        let ms = Duration::from_millis;
        // Each a second of frames.
        let runs = [
            vec![ms(1); 1000],
            vec![ms(10); 100],
            vec![Tick::DT; 64],
            vec![ms(40); 25],
            [ms(2), ms(48)].repeat(20),
        ];
        let mut first = None;
        for frames in runs {
            let ticks = Rc::new(RefCell::new(Vec::new()));
            let mut stack = Stack::default();
            stack.push_layer(Turner(ticks.clone()));
            let mut cx = Context::new(None);
            stack.attach(&mut cx);
            let mut stepper = Stepper::default();
            for dt in frames {
                // A frame without time, as the first is and the first after a pause, runs no tick.
                for dt in [Duration::ZERO, dt] {
                    stepper.step(dt, |tick| stack.tick(&mut cx, tick), || Duration::ZERO);
                }
            }
            let ticks = ticks.borrow();
            let numbers: Vec<_> = ticks.iter().map(|tick| tick.number).collect();
            assert_eq!(numbers, (0..64).collect::<Vec<_>>());
            let last = ticks.last().map(|tick| tick.time());
            assert_eq!(last, Some(Duration::from_secs(1)));
            let turned = cx.world().query::<&Transform>().next();
            let turned = turned.map(|transform| transform.rotation.to_array().map(f32::to_bits));
            assert_eq!(turned, *first.get_or_insert(turned));
        }
    }

    #[test]
    fn a_frame_runs_at_most_8_ticks_keeping_the_fraction() {
        let mut stepper = Stepper::default();
        let fast = || Duration::ZERO;
        // 64.32 ticks' time: 8 run, the other 56 whole ticks are dropped, and 5 ms is left over.
        assert_eq!(stepper.step(Duration::from_millis(1005), |_| true, fast), 8);
        assert_eq!(stepper.dropped, 56);
        // A whole tick, with the 5 ms.
        assert_eq!(
            stepper.step(Duration::from_micros(10_625), |_| true, fast),
            1
        );
        assert_eq!(stepper.step(Duration::ZERO, |_| true, fast), 0);
        assert_eq!((stepper.ticks, stepper.dropped), (9, 56));
    }

    #[test]
    fn ticks_that_run_slow_stop_the_frame() {
        let mut stepper = Stepper::default();
        let mut numbers = Vec::new();
        let ran = Cell::new(0);
        for _ in 0..2 {
            ran.set(0);
            // Each tick takes two ticks' time.
            let took = || Tick::DT * 2 * ran.get();
            let owed = Tick::DT * MOST_TICKS as u32;
            let tick = |tick: Tick| {
                ran.set(ran.get() + 1);
                numbers.push(tick.number);
                true
            };
            assert_eq!(stepper.step(owed, tick, took), 1);
        }
        assert_eq!(numbers, [0, 1]);
        assert_eq!(stepper.dropped, 2 * (MOST_TICKS - 1));
    }

    /// A tick waiting for a lockstep peer stops the frame's ticks, and the time owed stays owed,
    /// up to 8 ticks of it, drawn as the last tick left the world, for the frames once it comes.
    #[test]
    fn ticks_waiting_for_a_peer_stay_owed() {
        let mut stepper = Stepper::default();
        let fast = || Duration::ZERO;
        let mut until = 2;
        let mut tick = |tick: Tick| tick.number < until;
        assert_eq!(stepper.step(Tick::DT * 5, &mut tick, fast), 2);
        assert_eq!((stepper.ticks, stepper.dropped), (2, 0));
        assert_eq!(stepper.alpha(), 1.0);
        assert_eq!(stepper.step(Tick::DT * 20, &mut tick, fast), 0);
        until = 100;
        let mut tick = |tick: Tick| tick.number < until;
        assert_eq!(stepper.step(Duration::ZERO, &mut tick, fast), MOST_TICKS);
        assert_eq!((stepper.ticks, stepper.dropped), (2 + MOST_TICKS, 0));
    }

    /// A frame draws the world at the simulation's time, part of the way through the latest tick
    /// as the world draws, from 0 before any; in lockstep, at the latest tick's end.
    #[test]
    fn frames_draw_the_world_at_the_simulations_time() {
        let fast = || Duration::ZERO;
        let mut stepper = Stepper::default();
        assert_eq!(stepper.time(stepper.alpha()), 0.0);
        assert_eq!(stepper.step(Tick::DT * 5 / 2, |_| true, fast), 2);
        assert_eq!(stepper.time(stepper.alpha()), 1.5 / 64.0);
        let mut lockstep = Stepper::lockstep();
        for _ in 0..3 {
            assert_eq!(lockstep.step(Duration::ZERO, |_| true, fast), 1);
        }
        assert_eq!(lockstep.time(lockstep.alpha()), 3.0 / 64.0);
    }

    /// Paused, frames run only the ticks asked for, at most 8 a frame, whatever their time, and
    /// draw the world as the last tick left it; resumed, the pause is owed nothing, and the steps
    /// not yet run are dropped.
    #[cfg(feature = "dev")]
    #[test]
    fn a_paused_simulation_runs_only_the_steps_asked_for() {
        let mut stepper = Stepper::default();
        let fast = || Duration::ZERO;
        assert_eq!(stepper.step(Tick::DT / 2, |_| true, fast), 0);
        stepper.pause();
        assert_eq!(stepper.step(Tick::DT * 3, |_| true, fast), 0);
        assert_eq!(stepper.alpha(), 1.0);
        stepper.step_by(10);
        let mut numbers = Vec::new();
        for ran in [MOST_TICKS, 2, 0] {
            let tick = |tick: Tick| {
                numbers.push(tick.number);
                true
            };
            assert_eq!(stepper.step(Duration::ZERO, tick, fast), ran);
        }
        assert_eq!(numbers, (0..10).collect::<Vec<_>>());
        assert_eq!(
            (stepper.ticks, stepper.dropped, stepper.steps()),
            (10, 0, 0)
        );
        stepper.step_by(5);
        stepper.resume();
        assert_eq!((stepper.paused(), stepper.steps()), (false, 0));
        assert_eq!(stepper.step(Tick::DT, |_| true, fast), 1);
        assert_eq!(stepper.alpha(), 0.0);
    }

    /// Stopped, frames run no ticks, whatever their time or the steps asked for, owing none and
    /// dropping none, and draw the world as the tool API reads it; played, the stop is owed
    /// nothing; and the ticks the run has run count those of a world `stop` takes back.
    #[cfg(feature = "dev")]
    #[test]
    fn a_stopped_simulation_runs_no_ticks() {
        let mut stepper = Stepper::default();
        let fast = || Duration::ZERO;
        assert_eq!(stepper.step(Tick::DT * 5 / 2, |_| true, fast), 2);
        stepper.step_by(3);
        stepper.stop();
        for dt in [Tick::DT * 3, Duration::from_secs(1)] {
            assert_eq!(stepper.step(dt, |_| true, fast), 0);
        }
        assert_eq!(stepper.alpha(), 1.0);
        assert_eq!((stepper.ticks, stepper.dropped, stepper.steps()), (2, 0, 0));
        stepper.resume();
        stepper.play();
        assert_eq!(stepper.step(Tick::DT / 2, |_| true, fast), 0);
        assert_eq!(stepper.alpha(), 0.5);
        assert_eq!(stepper.step(Tick::DT / 2, |_| true, fast), 1);
        // As `stop` brings back a world as edited at tick 0.
        stepper.jump_to(0);
        assert_eq!((stepper.ticks, stepper.ran()), (0, 3));
    }

    /// A development build's run scans the assets under its root as it starts, before its layers
    /// load: one that serves the tool API writes the ID files missing; one that doesn't, or that
    /// joins a lockstep session, whose host writes them, writes none, and warns of each asset
    /// without one.
    #[cfg(feature = "dev")]
    #[test]
    fn a_run_writes_missing_id_files_as_it_starts_only_if_it_serves_the_api() {
        /// Notes, as it loads, whether the ID file at its path is there.
        struct Looks(std::path::PathBuf, Rc<Cell<Option<bool>>>);

        impl Layer for Looks {
            fn load(&mut self, _: &mut Context) {
                self.1.set(Some(self.0.exists()));
            }
        }

        let scratch = crate::dirs::tests::Scratch::new("start-scans");
        std::fs::create_dir_all(&scratch.0).expect("a directory");
        std::fs::write(scratch.0.join("a.png"), "").expect("an asset");
        let meta = scratch.0.join("a.png.meta");
        let logged = crate::testing::logged();
        // Whether the run serves the API and whether it joins, the one that writes last.
        for (serves, joins) in [(false, false), (true, true), (true, false)] {
            let found = Rc::new(Cell::new(None));
            let mut stack = Stack::default();
            stack.push_layer(Looks(meta.clone(), Rc::clone(&found)));
            if joins {
                // It connects as it first turns, which this run never does.
                let join =
                    crate::args::Net::Join(crate::args::Toward::Address("127.0.0.1:9".to_owned()));
                let code = crate::net::TESTED;
                let net = crate::net::Lockstep::start(&join, code, "Tested", &[]).expect("joining");
                stack.lockstep(net);
            }
            let mut cx = Context::new(None);
            cx.root = Some(scratch.0.clone());
            let stepper = &mut Stepper::default();
            start(&mut stack, &mut cx, stepper, None, false, serves).expect("started");
            let writes = serves && !joins;
            let case = format!("serves: {serves}, joins: {joins}");
            assert_eq!(found.get(), Some(writes), "{case}");
            let codes = logged
                .codes
                .try_iter()
                .filter(|code| code.starts_with("asset."));
            let expected = if writes {
                "asset.id_written"
            } else {
                "asset.no_id"
            };
            assert_eq!(codes.collect::<Vec<_>>(), [expected], "{case}");
        }
    }

    #[test]
    fn headless_frames_tick_once_before_the_layers_update() {
        let mut stepper = Stepper::lockstep();
        for dt in [Duration::ZERO, Duration::from_secs(1)] {
            assert_eq!(stepper.step(dt, |_| true, || Duration::from_secs(1)), 1);
        }
        assert_eq!((stepper.ticks, stepper.dropped), (2, 0));

        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut stack = Stack::default();
        stack.push_layer(Order(calls.clone()));
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        let mut clock = Clock::default();
        let mut stepper = Stepper::lockstep();
        for _ in 0..2 {
            let (cx, clock, stepper) = (&mut cx, &mut clock, &mut stepper);
            let dt = clock.start_frame();
            assert!(run_frame(&mut stack, cx, dt, stepper, None).is_none());
        }
        assert_eq!(*calls.borrow(), ["tick 0", "update", "tick 1", "update"]);
    }

    /// A stopped run's frames, which run no tick, bring a world's cells in around the look's
    /// viewpoint, as the editor's camera is.
    #[cfg(feature = "dev")]
    #[test]
    fn a_stopped_runs_frames_bring_cells_in_around_the_look() {
        use crate::cells::tests::{look_at, project, xs};

        let dir = crate::dirs::tests::Scratch::new("main-loop-stopped-cells");
        let mut stack = Stack::default();
        let mut cx = Context::seeded(1).root(project(&dir.0));
        cx.world_mut().spawn(crate::Cells {
            folder: "cells".to_owned(),
            size: 10.0,
        });
        stack.attach(&mut cx);
        look_at(&mut cx, 15.0);
        let mut clock = Clock::default();
        let mut stepper = Stepper::default();
        stepper.stop();
        // One frame notes the cells coming and prepares them, the next brings them in.
        for _ in 0..2 {
            let (cx, clock, stepper) = (&mut cx, &mut clock, &mut stepper);
            let dt = clock.start_frame();
            assert!(run_frame(&mut stack, cx, dt, stepper, None).is_none());
        }
        assert_eq!(xs(cx.world()), [0, 1, 2, 3]);
        assert_eq!(stepper.ticks, 0);
    }

    #[test]
    fn the_world_draws_over_the_layers_draws() {
        let mut stack = Stack::default();
        stack.push_layer(Drawer);
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        let mut clock = Clock::default();
        let mut stepper = Stepper::default();
        let (cx, dt, stepper) = (&mut cx, clock.start_frame(), &mut stepper);
        assert!(run_frame(&mut stack, cx, dt, stepper, None).is_none());
        // The layer's sprite, of no entity, then the scene's place and the world's, with its
        // entity's ID, which the scene's place keeps from joining the layer's run.
        let world = cx.world();
        let red_one = world.query::<(Entity, &Sprite)>().next();
        let (red_one, _) = red_one.expect("the sprite the layer spawned");
        let id = world.id(red_one).expect("alive").get();
        let mut expected = Draws::default();
        expected.push_sprite(Mat4::IDENTITY, Sprite::default());
        expected.draw_scene();
        let red_one = (Transform::IDENTITY.matrix_from(DVec3::ZERO), red(), id);
        expected.extend_sprites(iter::once(red_one));
        let viewport = Viewport {
            origin: [0, 0],
            size: [640, 360],
        };
        let mut frame = render_thread::Extracted::default();
        cx.extract(&mut frame, viewport);
        assert_eq!(frame.draws, expected);
        // Apart, in the view's pixels from its top right, a third of a pixel a unit.
        let mut canvas = Draws::default();
        let anchor = Mat4::from_translation(Vec3::new(640.0, 360.0, 0.0));
        let scale = Mat4::from_scale(Vec3::new(1.0 / 3.0, 1.0 / 3.0, 1.0));
        canvas.push_sprite(anchor * scale * top_right(), Sprite::default());
        assert_eq!(frame.canvas, canvas);
        // The context's buffers and those the frame's draws come back in all start empty.
        for _ in 0..2 {
            cx.extract(&mut frame, viewport);
            assert!(
                frame.draws.is_empty() && frame.canvas.is_empty(),
                "the context keeps none of a frame's draws for the next"
            );
        }
    }

    /// Spawns a camera as it attaches, and sets a viewpoint of its own as it updates while `own`
    /// says to.
    struct Overlooker {
        own: Rc<Cell<bool>>,
    }

    /// What [`Overlooker`] sets.
    const OVERLOOKING: Viewpoint = Viewpoint {
        position: DVec3::new(0.0, 0.0, 5.0),
        rotation: Quat::IDENTITY,
        projection: Projection::Orthographic {
            height: 3.0,
            near: -1.0,
            far: 1.0,
        },
    };

    /// What [`Overlooker`]'s camera sees from.
    const ITS_CAMERA: Viewpoint = Viewpoint {
        position: DVec3::new(1.0, 2.0, 0.0),
        rotation: Quat::IDENTITY,
        projection: Projection::Orthographic {
            height: 6.0,
            near: -1.0,
            far: 1.0,
        },
    };

    impl Layer for Overlooker {
        fn attach(&mut self, cx: &mut Context) {
            let at = Transform {
                translation: ITS_CAMERA.position,
                ..Transform::IDENTITY
            };
            let camera = Camera {
                projection: ITS_CAMERA.projection,
                primary: true,
            };
            cx.world_mut().spawn((at, camera));
        }

        fn update(&mut self, cx: &mut Context, _: Duration) {
            if self.own.get() {
                cx.set_viewpoint(OVERLOOKING);
            }
        }
    }

    /// The view sees from the default without a camera; through the world's camera, as it draws
    /// the world, in frames no layer sets a viewpoint; and from a layer's viewpoint in the frame
    /// it sets it, that frame alone.
    #[test]
    fn the_view_sees_from_a_layers_viewpoint_for_its_frame_and_else_through_the_worlds_camera() {
        let viewport = Viewport {
            origin: [0, 0],
            size: [640, 360],
        };
        let mut frame = render_thread::Extracted::default();
        let own = Rc::new(Cell::new(false));
        let mut stack = Stack::default();
        stack.push_layer(Overlooker { own: own.clone() });
        let mut cx = Context::new(None);
        cx.extract(&mut frame, viewport);
        assert_eq!(frame.viewpoint, Viewpoint::default());
        stack.attach(&mut cx);
        let mut stepper = Stepper::lockstep();
        for (set, seen) in [
            (false, ITS_CAMERA),
            (true, OVERLOOKING),
            (false, ITS_CAMERA),
        ] {
            own.set(set);
            let dt = Duration::ZERO;
            assert!(run_frame(&mut stack, &mut cx, dt, &mut stepper, None).is_none());
            cx.extract(&mut frame, viewport);
            assert_eq!(frame.viewpoint, seen, "a viewpoint set: {set}");
        }
    }

    /// Frames between ticks draw the world on its way between them, from where the latest tick
    /// found it to where it left it, as far as they're past the tick: frames a quarter of a tick
    /// apart, and two and a half, which run two ticks or three, each copying the world as it
    /// starts. A frame before the first tick draws the world as it attached; an entity reset at a
    /// tick, and one spawned at one, draw where the tick left them.
    #[test]
    fn frames_draw_between_the_ticks_around_them() {
        let mut stack = Stack::default();
        stack.push_layer(Slider::default());
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        let mut stepper = Stepper::default();
        let quarters = iter::repeat_n(Tick::DT / 4, 16);
        let dts = quarters.chain(iter::repeat_n(Tick::DT * 5 / 2, 3));
        // How far each frame is past its last tick: a quarter more each frame, from the fourth,
        // whose tick is the first; then half a tick, none and half.
        let alphas = [0.0; 3]
            .into_iter()
            .chain((0..13).map(|frame| (frame % 4) as f32 / 4.0));
        let alphas = alphas.chain([0.5, 0.0, 0.5]);
        for (frame, (dt, alpha)) in (1..).zip(dts.zip(alphas)) {
            let draws = drawn(&mut stack, &mut cx, dt, &mut stepper);
            // Each tick moves the slider 1. The frames two and a half ticks apart run two ticks or
            // three, or fewer on a machine that stalls them past their time, as the stepper counts.
            let slider = match stepper.ticks {
                0 => 0.0,
                ticks => (ticks - 1) as f32 + alpha,
            };
            // Tick 2 runs in frame 12, and tick 3 in frame 16.
            let jumper = if frame < 16 { 10.0 } else { 100.0 };
            let want = slider_draws(cx.world(), slider, jumper, frame >= 12);
            assert_eq!(draws, want, "frame {frame}, after {} ticks", stepper.ticks);
        }
        stack.detach(&mut cx);
    }

    /// A headless frame, in lockstep, draws the world as its tick left it, bit for bit, as though
    /// nothing drew between ticks.
    #[test]
    fn a_headless_frame_draws_the_tick_it_ran() {
        let mut stack = Stack::default();
        stack.push_layer(Slider::default());
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        let mut stepper = Stepper::lockstep();
        for frame in 1..=5 {
            let draws = drawn(&mut stack, &mut cx, Duration::ZERO, &mut stepper);
            let jumper = if frame < 4 { 10.0 } else { 100.0 };
            let want = slider_draws(cx.world(), frame as f32, jumper, frame >= 3);
            assert_eq!(draws, want, "frame {frame}");
        }
        stack.detach(&mut cx);
    }

    #[cfg(feature = "dev")]
    #[test]
    fn the_inspector_lists_the_world_in_the_dock() {
        let mut stack = Stack::default();
        stack.push_layer(Drawer);
        stack.push_overlay(Inspector::default());
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 1200.0),
            )),
            ..Default::default()
        };
        let mut dock = Dock::new(None, Chrome::Os, None, false, &[]);
        let mut clock = Clock::default();
        let mut stepper = Stepper::default();
        let egui = crate::egui_context();
        let mut texts = Vec::new();
        // The dock places a panel in its first frame, and draws it from the next.
        for _ in 0..3 {
            let (cx, dt, stepper) = (&mut cx, clock.start_frame(), &mut stepper);
            let ui = Some((&egui, &mut dock, input()));
            let (mut output, _) = run_frame(&mut stack, cx, dt, stepper, ui).expect("a pass");
            output.textures_delta.clear();
            let painted = crate::testing::painted(&output.shapes).into_iter();
            let shown = painted.filter_map(|shape| match shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            });
            texts = shown.collect();
        }
        for text in [
            "1 entity",
            "Nothing picked: pick an entity in the Entities panel",
        ] {
            assert!(texts.iter().any(|shown| shown == text), "{text}: {texts:?}");
        }
    }

    #[test]
    fn the_frame_after_a_pause_counts_from_its_own_start() {
        let mut clock = Clock::default();
        assert_eq!(clock.start_frame(), Duration::ZERO);
        std::thread::sleep(Duration::from_millis(1));
        assert!(clock.start_frame() > Duration::ZERO);
        std::thread::sleep(Duration::from_millis(1));
        clock.resume();
        assert_eq!(clock.start_frame(), Duration::ZERO);
        // The run's time still counts the pause.
        assert!(clock.elapsed() >= Duration::from_millis(2));
    }
}

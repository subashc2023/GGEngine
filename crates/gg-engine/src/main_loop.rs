//! The main loop's turn, a frame's, which windowed and headless runs share: running the ticks of
//! the simulation that the frame's time owes, at a fixed rate whatever the frame rate, updating
//! the layers and having them draw their UI, and handing the frame to the render thread; headless
//! runs, which render offscreen; and what a run reports as it ends.

use std::mem;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use gg_core::Result;
use gg_renderer::{MAX_TEXTURE_SIZE, Renderer, Settings, Validation, Viewport};
use tracing::{debug, error, info, warn};

use crate::api::{self, Api};
use crate::args::Args;
use crate::chrome::Chrome;
use crate::dock::{Dock, Ui};
use crate::layer::{Context, Stack, Tick};
use crate::perf::{self, Perf, Summary};
use crate::render_thread::{Render, RenderThread, TextureChanges};
use crate::snapshot::Snapshot;
use crate::viewports::Drawn;
use crate::{App, BUILD, egui_context};

/// Renders offscreen, without a window, as fast as the GPU allows, with a tick of the simulation
/// each frame, so that a run of a set number of frames runs as many ticks on any machine. Frames
/// render on the render thread, as a windowed run's do, the last as the run ends. `api` serves
/// tools as each frame starts; while it has paused the simulation with nothing asked of the
/// frames, each waits up to a tick's time for a request, rather than spinning. The run starts from
/// `snapshot`'s world, at its ticks, if it's given one, rather than the one its layers attach, and
/// drops it once it has.
pub(crate) fn run_headless(
    app: &mut App,
    args: &Args,
    mut api: Option<Api>,
    snapshot: Option<Snapshot>,
) -> Result<Ran> {
    let (width, height) = (1280, 720);
    let renderer = Renderer::headless(&settings(app, args), width, height)?;
    let mut render = RenderThread::start(renderer)?;
    let mut cx = Context::of_run(None, app);
    let mut clock = Clock::default();
    let mut stepper = match app.headless_step {
        Some(_) => Stepper::default(),
        None => Stepper::lockstep(),
    };
    let egui = egui_context();
    // Headless runs neither read nor save the layout, so that they're reproducible, and have no
    // window to frame.
    let mut dock = Dock::new(
        None,
        Chrome::Os,
        app.view_panel,
        app.custom_title_bar,
        &app.docked,
    );
    let mut egui_runs = true;
    // The frames handed over to render, and those rendered.
    let (mut handed, mut frames) = (0, 0);
    let started = match snapshot {
        Some(snapshot) => {
            (app.stack.restore(&mut cx, &snapshot)).map(|ticks| stepper.start_at(ticks))
        }
        None => {
            app.stack.attach(&mut cx);
            Ok(())
        }
    };
    let rendered = started.and_then(|()| {
        while args.frames.is_none_or(|limit| handed < limit) {
            api::idle(&mut api, &stepper);
            if api::serve(&mut api, &mut cx, &mut app.stack, &mut stepper) {
                debug!("A tool asked the run to end");
                break;
            }
            // One point is one pixel, as on a display without scaling.
            let input = egui_runs.then(|| egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width as f32, height as f32),
                )),
                max_texture_side: Some(MAX_TEXTURE_SIDE),
                time: Some(clock.elapsed().as_secs_f64()),
                ..Default::default()
            });
            // The clock starts every frame, even with a step set: egui and the run's end read it.
            let since_last = clock.start_frame();
            let dt = app.headless_step.unwrap_or(since_last);
            let pass = run_frame(
                &mut app.stack,
                &mut cx,
                dt,
                &mut stepper,
                input.map(|input| (&egui, &mut dock, input)),
            );
            let (ui, tessellated, view) = match pass {
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
            api::frame_back(&mut api, &mut render, stepper.ticks);
            hand_off(&mut render, &mut cx, view, ui, Vec::new());
            handed += 1;
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
    rendered?;
    app.stack.recorded()?;
    let perf = mem::take(&mut cx.perf);
    Ok(Ran::finish(
        renderer, frames, &clock, &stepper, perf, stopping,
    ))
}

/// The most pixels a side of egui's textures may have, the most every device the engine runs on
/// takes, which each pass's input tells egui: every window's alike, since egui makes its font atlas
/// again for a pass whose limit isn't the last one's.
pub(crate) const MAX_TEXTURE_SIDE: usize = MAX_TEXTURE_SIZE as usize;

/// The first half of a turn of the main loop: starts the frame, `dt` after the last, for Tracy and
/// the frame's measure, runs the ticks of the simulation that the time owes, as `stepper` steps
/// it, copying where the world's entities are as each starts, updates the layers, draws the world
/// after what they drew, as far on from where the last tick found it to where it left it as the
/// frame is past the tick, and, given egui, the dock and egui's input in `ui`, has the layers draw
/// their UI, in the dock and with egui. Returns that pass's output, and whether any layer draws
/// UI, as [`Stack::ui`] says. [`tessellate`] starts the second half, which hands what egui made
/// of it to the render thread.
///
/// egui allocates as it lays out here, and as [`tessellate`] tessellates, so frames that run its
/// pass allocate; the UI is a development overlay, and an app that draws none stops running it,
/// as [`keeps_egui`] says.
pub(crate) fn run_frame(
    stack: &mut Stack,
    cx: &mut Context,
    dt: Duration,
    stepper: &mut Stepper,
    ui: Option<(&egui::Context, &mut Dock, egui::RawInput)>,
) -> Option<(egui::FullOutput, bool)> {
    gg_core::profile::frame_mark();
    cx.perf.start_frame(dt);
    let _zone = gg_core::zone!();
    let start = Instant::now();
    let tick = |tick| {
        cx.capture();
        stack.tick(cx, tick);
    };
    stepper.step(dt, tick, || start.elapsed());
    let ticked = Instant::now();
    stack.update(cx, dt);
    let alpha = stepper.alpha();
    gg_core::plot!("alpha", f64::from(alpha));
    cx.draw_world(alpha);
    let updated = Instant::now();
    let pass = ui.map(|(egui, dock, input)| {
        let _zone = gg_core::zone!("egui");
        let mut layers_draw = false;
        // egui may run the pass again in a frame, as it asks for one.
        let output = egui.run_ui(input, |root| {
            dock.begin(root);
            layers_draw |= stack.ui(cx, &mut Ui::new(root, dock));
            dock.end(root);
        });
        (output, layers_draw)
    });
    cx.perf
        .ran(ticked - start, updated - ticked, updated.elapsed());
    pass
}

/// Whether egui runs from the next frame on, after a pass in which `layers_draw` says whether any
/// of `stack`'s layers draw UI, in windows framed as `chrome` says. Once none does, the OS frames
/// the windows, and the game's view isn't in `dock`'s panel, egui would show nothing, so it stops
/// for the rest of the run: whether a layer draws UI is its type's, which no later frame changes.
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

/// What the main window's pass drew in a frame, tessellated, for the render thread.
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
/// rendered, which `cx` gives in place of the last; and returns whether that frame presented:
/// `None` if no frame was rendering.
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
        &render.frame().picked[..]
    } else {
        &[]
    };
    cx.take_answers(answers);
    rendered.map(|rendered| rendered.presented).transpose()
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
/// simulation steps at its fixed rate whatever the frame rate; or, paused, as the tool API asks.
#[derive(Default)]
pub(crate) struct Stepper {
    /// The time not yet simulated: less than a tick, between frames.
    owed: Duration,
    /// The ticks run, which number the next.
    pub(crate) ticks: u64,
    /// The ticks the run started from: a snapshot's, in a run restored from one, else 0.
    from: u64,
    /// The whole ticks the time owed held that no frame ran.
    dropped: u64,
    /// One tick every frame, whatever its time.
    lockstep: bool,
    /// Set while the tool API has paused the simulation, whose frames then run only the ticks
    /// `steps` asks for, and draw the world as the last tick left it.
    paused: bool,
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
        (self.ticks, self.from) = (ticks, ticks);
    }

    /// The ticks the run has run, since it started.
    pub(crate) fn ran(&self) -> u64 {
        self.ticks - self.from
    }

    /// Runs, through `tick`, the ticks owed by a frame `dt` after the last, numbered on from the
    /// last: one for each whole tick the time owed holds, at most [`MOST_TICKS`], and no more once
    /// `took`, the time the frame's ticks have taken so far, is longer than the time they've
    /// simulated, so that a simulation slower than real time slows down rather than spending every
    /// frame catching up. The whole ticks not run are dropped, and the fraction of one left over
    /// carries to the next frame. In lockstep, exactly one. Paused, those the steps asked for,
    /// whatever the time, as many as the time they take allows, the rest left for the next frame.
    /// Returns how many ran.
    pub(crate) fn step(
        &mut self,
        dt: Duration,
        mut tick: impl FnMut(Tick),
        took: impl Fn() -> Duration,
    ) -> u64 {
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
        while ran < due.min(MOST_TICKS) {
            tick(Tick { number: self.ticks });
            self.ticks += 1;
            ran += 1;
            // Slower than real time: the ticks left would only fall further behind.
            if took() > Tick::DT * ran as u32 {
                break;
            }
        }
        if self.paused {
            self.steps -= ran;
        } else {
            self.dropped += due - ran;
        }
        ran
    }

    /// How far the frame is past its last tick, as a fraction of a tick, from 0 to less than 1,
    /// which the world draws that far on from where the tick found it to where it left it; 1 in
    /// lockstep, so that a headless frame draws its tick as it leaves the world, and while paused,
    /// so that a frame draws the world as the tool API reads it.
    fn alpha(&self) -> f32 {
        if self.lockstep || self.paused {
            return 1.0;
        }
        (self.owed.as_nanos() as f64 / Tick::DT.as_nanos() as f64) as f32
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
}

pub(crate) fn settings<'a>(app: &'a App, args: &'a Args) -> Settings<'a> {
    Settings {
        app_name: app.name,
        validation: args.validation,
        gpu: args.gpu.as_deref(),
        vsync: args.vsync,
        uploads: None,
        pipeline_cache: None,
        capture: args.capture,
    }
}

/// What a run did, reported when it ends.
pub(crate) struct Ran {
    frames: u64,
    /// The ticks the simulation ran in the run: past a snapshot's, in a run restored from one.
    ticks: u64,
    /// The whole ticks the frames' time held that they didn't run, which the simulation fell
    /// behind by.
    ticks_dropped: u64,
    validation: Validation,
    /// The GPU it rendered on.
    gpu: String,
    /// When its first frame started, if one did.
    first_frame_at: Option<Instant>,
    shutdown: Duration,
    perf: Summary,
}

impl Ran {
    /// Shuts the renderer down first, so that what validation reports on the way, such as
    /// objects left undestroyed, counts too. `stopping` is when the run began to end, after its
    /// last frame rendered, `stepper` what stepped its simulation, and `perf` what it measured of
    /// its frames, the last of them finished.
    pub(crate) fn finish(
        renderer: Renderer,
        frames: u64,
        clock: &Clock,
        stepper: &Stepper,
        perf: Perf,
        stopping: Instant,
    ) -> Self {
        let validation = renderer.validation();
        let gpu = renderer.gpu_name().to_owned();
        drop(renderer);
        Self {
            frames,
            ticks: stepper.ran(),
            ticks_dropped: stepper.dropped,
            validation,
            gpu,
            first_frame_at: clock.first,
            shutdown: stopping.elapsed(),
            perf: perf.summary(),
        }
    }

    /// Logs how long the run's frames took, then how it ended: `engine.perf`, whose times are in
    /// milliseconds, comes first, so that a run that fails validation has it too. `started` is
    /// when the engine started, and `args` how it ran.
    pub(crate) fn report(self, started: Instant, args: &Args) -> ExitCode {
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
            render_ms_mean = perf.render.mean,
            render_ms_p95 = perf.render.p95,
            wait_ms_mean = perf.wait.mean,
            wait_ms_p95 = perf.wait.p95,
            draws_max = perf.draws_max,
            instances_max = perf.instances_max,
            triangles_max = perf.triangles_max,
            gpu = self.gpu.as_str(),
            build = BUILD,
            headless = args.headless,
            vsync = args.vsync,
            validated = self.validation.enabled(),
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
        let messages = self.validation.messages();
        if messages > 0 {
            error!(
                code = "engine.validation",
                messages,
                "Vulkan validation reported {messages} problem(s); the warnings and errors above say what"
            );
            return ExitCode::FAILURE;
        }
        info!(
            frames = self.frames,
            validated = self.validation.enabled(),
            "Exited cleanly"
        );
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::{Cell, RefCell};
    use std::f32::consts::FRAC_PI_2;
    use std::iter;
    use std::rc::Rc;

    use gg_core::math::{Mat4, Quat, Vec3, Vec4};
    use gg_renderer::Draws;

    use super::*;
    use crate::{
        Camera, Entity, Inspector, Layer, Projection, Sprite, Transform, Viewpoint, World,
        render_thread,
    };

    /// Spawns a red sprite as it attaches, and draws a sprite of its own each update.
    struct Drawer;

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
            translation: Vec3::new(x, 0.0, z),
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
    fn drawn(
        stack: &mut Stack,
        cx: &mut Context,
        dt: Duration,
        stepper: &mut Stepper,
        (egui, dock): (&egui::Context, &mut Dock),
    ) -> Draws {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 360.0),
            )),
            ..Default::default()
        };
        let pass = run_frame(stack, cx, dt, stepper, Some((egui, dock, input)));
        let (mut output, _) = pass.expect("a pass, given egui");
        // As the renderer would have.
        output.textures_delta.clear();
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
            let found = placed.find(|(_, at)| at.translation.z == z);
            let (entity, _) = found.expect("a sprite at each z");
            world.id(entity).expect("alive").get()
        };
        let mut want = Draws::default();
        let mut sprite = |x: f32, z: f32| {
            want.extend_sprites(iter::once((slid(x, z).matrix(), red(), id(z))));
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
        assert_eq!(stepper.step(Duration::from_millis(1005), |_| {}, fast), 8);
        assert_eq!(stepper.dropped, 56);
        // A whole tick, with the 5 ms.
        assert_eq!(stepper.step(Duration::from_micros(10_625), |_| {}, fast), 1);
        assert_eq!(stepper.step(Duration::ZERO, |_| {}, fast), 0);
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
            };
            assert_eq!(stepper.step(owed, tick, took), 1);
        }
        assert_eq!(numbers, [0, 1]);
        assert_eq!(stepper.dropped, 2 * (MOST_TICKS - 1));
    }

    /// Paused, frames run only the ticks asked for, at most 8 a frame, whatever their time, and
    /// draw the world as the last tick left it; resumed, the pause is owed nothing, and the steps
    /// not yet run are dropped.
    #[cfg(feature = "dev")]
    #[test]
    fn a_paused_simulation_runs_only_the_steps_asked_for() {
        let mut stepper = Stepper::default();
        let fast = || Duration::ZERO;
        assert_eq!(stepper.step(Tick::DT / 2, |_| {}, fast), 0);
        stepper.pause();
        assert_eq!(stepper.step(Tick::DT * 3, |_| {}, fast), 0);
        assert_eq!(stepper.alpha(), 1.0);
        stepper.step_by(10);
        let mut numbers = Vec::new();
        for ran in [MOST_TICKS, 2, 0] {
            let tick = |tick: Tick| numbers.push(tick.number);
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
        assert_eq!(stepper.step(Tick::DT, |_| {}, fast), 1);
        assert_eq!(stepper.alpha(), 0.0);
    }

    #[test]
    fn headless_frames_tick_once_before_the_layers_update() {
        let mut stepper = Stepper::lockstep();
        for dt in [Duration::ZERO, Duration::from_secs(1)] {
            assert_eq!(stepper.step(dt, |_| {}, || Duration::from_secs(1)), 1);
        }
        assert_eq!((stepper.ticks, stepper.dropped), (2, 0));

        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut stack = Stack::default();
        stack.push_layer(Order(calls.clone()));
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 360.0),
            )),
            ..Default::default()
        };
        let mut dock = Dock::new(None, Chrome::Os, None, false, &[]);
        let mut clock = Clock::default();
        let mut stepper = Stepper::lockstep();
        let egui = egui_context();
        for _ in 0..2 {
            let (cx, clock, stepper) = (&mut cx, &mut clock, &mut stepper);
            let dt = clock.start_frame();
            let ui = Some((&egui, &mut dock, input()));
            let (mut output, _) = run_frame(&mut stack, cx, dt, stepper, ui).expect("a pass");
            // As the renderer would have.
            output.textures_delta.clear();
        }
        assert_eq!(*calls.borrow(), ["tick 0", "update", "tick 1", "update"]);
    }

    #[test]
    fn the_world_draws_over_the_layers_draws() {
        let mut stack = Stack::default();
        stack.push_layer(Drawer);
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 360.0),
            )),
            ..Default::default()
        };
        let mut dock = Dock::new(None, Chrome::Os, None, false, &[]);
        let mut clock = Clock::default();
        let mut stepper = Stepper::default();
        let egui = egui_context();
        let (cx, dt, stepper) = (&mut cx, clock.start_frame(), &mut stepper);
        let ui = Some((&egui, &mut dock, input));
        let (mut output, _) = run_frame(&mut stack, cx, dt, stepper, ui).expect("a pass");
        // As the renderer would have.
        output.textures_delta.clear();
        // The layer's sprite, of no entity, then the world's, with its entity's ID.
        let world = cx.world();
        let red_one = world.query::<(Entity, &Sprite)>().next();
        let (red_one, _) = red_one.expect("the sprite the layer spawned");
        let id = world.id(red_one).expect("alive").get();
        let mut expected = Draws::default();
        expected.push_sprite(Mat4::IDENTITY, Sprite::default());
        let red_one = (Transform::IDENTITY.matrix(), red(), id);
        expected.extend_sprites(iter::once(red_one));
        let viewport = Viewport {
            origin: [0, 0],
            size: [640, 360],
        };
        let mut frame = render_thread::Extracted::default();
        cx.extract(&mut frame, viewport);
        assert_eq!(frame.draws, expected);
        // The context's buffer and the one the frame's draws come back in both start empty.
        for _ in 0..2 {
            cx.extract(&mut frame, viewport);
            assert!(
                frame.draws.is_empty(),
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
        position: Vec3::new(0.0, 0.0, 5.0),
        rotation: Quat::IDENTITY,
        projection: Projection::Orthographic {
            height: 3.0,
            near: -1.0,
            far: 1.0,
        },
    };

    /// What [`Overlooker`]'s camera sees from.
    const ITS_CAMERA: Viewpoint = Viewpoint {
        position: Vec3::new(1.0, 2.0, 0.0),
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
        let (mut stepper, egui) = (Stepper::default(), egui_context());
        let mut dock = Dock::new(None, Chrome::Os, None, false, &[]);
        let quarters = iter::repeat_n(Tick::DT / 4, 16);
        let dts = quarters.chain(iter::repeat_n(Tick::DT * 5 / 2, 3));
        // How far each frame is past its last tick: a quarter more each frame, from the fourth,
        // whose tick is the first; then half a tick, none and half.
        let alphas = [0.0; 3]
            .into_iter()
            .chain((0..13).map(|frame| (frame % 4) as f32 / 4.0));
        let alphas = alphas.chain([0.5, 0.0, 0.5]);
        for (frame, (dt, alpha)) in (1..).zip(dts.zip(alphas)) {
            let ui = (&egui, &mut dock);
            let draws = drawn(&mut stack, &mut cx, dt, &mut stepper, ui);
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
        let (mut stepper, egui) = (Stepper::lockstep(), egui_context());
        let mut dock = Dock::new(None, Chrome::Os, None, false, &[]);
        for frame in 1..=5 {
            let ui = (&egui, &mut dock);
            let draws = drawn(&mut stack, &mut cx, Duration::ZERO, &mut stepper, ui);
            let jumper = if frame < 4 { 10.0 } else { 100.0 };
            let want = slider_draws(cx.world(), frame as f32, jumper, frame >= 3);
            assert_eq!(draws, want, "frame {frame}");
        }
        stack.detach(&mut cx);
    }

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
        let egui = egui_context();
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

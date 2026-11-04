//! Windowed runs: the winit event loop, which hands the main window's events to the layers and
//! runs a frame whenever the window can show one, which renders on the render thread while the
//! next runs.

use std::any::Any;
use std::mem;
use std::ops::ControlFlow;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(feature = "dev")]
use egui::ViewportId;
use gg_core::{Error, Result};
use gg_renderer::{Renderer, Settings, Viewport};
use tracing::{debug, info, trace};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
#[cfg(feature = "dev")]
use winit::dpi::{PhysicalPosition, Position};
#[cfg(feature = "dev")]
use winit::event::ElementState;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
#[cfg(feature = "dev")]
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::api::{self, Api, Waited, Waiting};
use crate::args::Args;
#[cfg(feature = "dev")]
use crate::chrome::{self, Chrome};
#[cfg(feature = "dev")]
use crate::dock::{self, Dock, Under};
#[cfg(feature = "dev")]
use crate::egui_context;
use crate::layer::Stack;
use crate::main_loop::{
    Clock, MainUi, Ran, Stepper, hand_off, run_frame, settings, start, take_back,
};
#[cfg(feature = "dev")]
use crate::main_loop::{keeps_egui, tessellate};
use crate::pads::Gamepads;
use crate::pipeline_cache;
use crate::render_thread::RenderThread;
use crate::snapshot::Snapshot;
#[cfg(feature = "dev")]
use crate::view_input::{Reach, ViewInput};
#[cfg(feature = "dev")]
use crate::viewports::{self, Viewports, WindowInput};
#[cfg(all(target_os = "windows", feature = "dev"))]
use crate::windows_frame::Frame;
use crate::{App, Context, Event, event};

/// Runs `app` in a window until it closes, `api` serving tools as each frame starts, and between
/// frames while the window is minimized, woken by each request.
pub(crate) fn run(
    app: &mut App,
    args: &Args,
    mut api: Option<Api>,
    snapshot: Option<Snapshot>,
) -> Result<Ran> {
    let event_loop = EventLoop::new().map_err(|error| {
        Error::new(
            "engine.window",
            "can't connect to the window system; run with --headless to render without a window",
        )
        .with_source(error)
    })?;
    let proxy = event_loop.create_proxy();
    api::set_waker(&mut api, move || {
        // Only once the loop has ended, as the run does.
        let _ = proxy.send_event(());
    });
    #[cfg(feature = "dev")]
    let layout = dock::path(app.name, std::env::consts::OS, |name| {
        std::env::var_os(name)
    });
    let pipelines = pipeline_cache::path(app.name, std::env::consts::OS, |name| {
        std::env::var_os(name)
    });
    #[cfg(feature = "dev")]
    let chrome = Chrome::new(app.custom_title_bar);
    #[cfg(feature = "dev")]
    let view_input = app.view_panel.map(|_| ViewInput::new());
    let mut handler = Handler {
        #[cfg(feature = "dev")]
        dock: Dock::new(
            layout,
            chrome,
            app.view_panel,
            app.custom_title_bar,
            &app.docked,
        ),
        pipelines,
        app,
        args,
        snapshot,
        #[cfg(feature = "dev")]
        chrome,
        window: None,
        #[cfg(all(target_os = "windows", feature = "dev"))]
        frame: None,
        render: None,
        resized: None,
        #[cfg(feature = "dev")]
        egui: egui_context(),
        #[cfg(feature = "dev")]
        egui_runs: true,
        #[cfg(feature = "dev")]
        egui_winit: None,
        #[cfg(feature = "dev")]
        view_input,
        #[cfg(feature = "dev")]
        viewports: None,
        gamepads: Gamepads::default(),
        cx: Context::new(None),
        clock: Clock::default(),
        stepper: Stepper::default(),
        api,
        frames: 0,
        minimized: false,
        waiting: None,
        error: None,
        ran: None,
        panic: None,
    };
    let ran = event_loop.run_app(&mut handler);
    if let Some(panic) = handler.panic.take() {
        panic::resume_unwind(panic);
    }
    ran.map_err(|error| {
        Error::new("engine.window", "the window system's event loop failed").with_source(error)
    })?;
    match (handler.error, handler.ran) {
        (Some(error), _) => Err(error),
        (None, Some(ran)) => Ok(ran),
        (None, None) => Err(Error::new(
            "engine.window",
            "the event loop ended before the window opened",
        )),
    }
}

/// The event loop's state: the window and the render thread once the loop starts, and how the run
/// went. The layers are attached while the renderer exists.
struct Handler<'a> {
    app: &'a mut App,
    args: &'a Args,
    /// The snapshot the run starts from, if it's given one, in place of the world the layers
    /// attach, until the window opens and its world is restored.
    snapshot: Option<Snapshot>,
    /// Who draws the windows' frames.
    #[cfg(feature = "dev")]
    chrome: Chrome,
    window: Option<Arc<Window>>,
    /// Windows' own frame around the main window, when the UI draws its title bar.
    #[cfg(all(target_os = "windows", feature = "dev"))]
    frame: Option<Frame>,
    /// Renders the frames, each while the next runs. Before `viewports` and `cx`, which hold the
    /// windows too, so that on a way out that leaves it running, it takes the renderer back as it
    /// drops, and the renderer drops here, before the windows it presents to.
    render: Option<RenderThread>,
    /// The pipeline cache's file, which the renderer loads as it starts and saves as it ends.
    pipelines: Option<PathBuf>,
    /// The main window's size since the renderer was last told it, which it's told once it's back
    /// from rendering.
    resized: Option<PhysicalSize<u32>>,
    #[cfg(feature = "dev")]
    egui: egui::Context,
    /// Whether frames run egui's pass, as they do until one shows that it has nothing to draw.
    #[cfg(feature = "dev")]
    egui_runs: bool,
    /// egui's view of the window, which turns its events into egui's input.
    #[cfg(feature = "dev")]
    egui_winit: Option<WindowInput>,
    /// In a dockspace, what of the main window's input reaches the layers through the game's view.
    #[cfg(feature = "dev")]
    view_input: Option<ViewInput>,
    /// The windows of their own that panels tear off into.
    #[cfg(feature = "dev")]
    viewports: Option<Viewports>,
    /// Opened with the window, since Windows gives a pad's input only to a focused window.
    gamepads: Gamepads,
    #[cfg(feature = "dev")]
    dock: Dock,
    cx: Context,
    clock: Clock,
    /// Steps the simulation in real time, or as the tool API asks.
    stepper: Stepper,
    /// Serves tools, in development builds.
    api: Option<Api>,
    frames: u64,
    /// Set while the window has no area to render into.
    minimized: bool,
    /// Tells `ping` that frames have paused while the window is minimized.
    waiting: Option<Waited>,
    error: Option<Error>,
    ran: Option<Ran>,
    /// A panic rendering the frame that renders as the loop ends, which resumes once it has:
    /// winit on Windows drops a panic that leaves `exiting`.
    panic: Option<Box<dyn Any + Send>>,
}

impl Handler<'_> {
    fn open(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let _zone = gg_core::zone!();
        let attributes = Window::default_attributes()
            .with_title(self.app.name)
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        #[cfg(feature = "dev")]
        let attributes = chrome::attributes(self.chrome, attributes);
        let window = event_loop.create_window(attributes).map_err(|error| {
            Error::new("engine.window", "can't create the window").with_source(error)
        })?;
        #[cfg(all(target_os = "windows", feature = "dev"))]
        if self.chrome == Chrome::InWindowsFrame {
            self.frame = Some(Frame::install(&window)?);
        }
        let window = Arc::new(window);
        let size = window.inner_size();
        info!(
            width = size.width,
            height = size.height,
            scale = window.scale_factor(),
            "Window opened"
        );
        let pipelines = (self.pipelines.as_deref())
            .map(pipeline_cache::load)
            .unwrap_or_default();
        let settings = Settings {
            pipeline_cache: Some(&pipelines),
            ..settings(self.app, self.args)
        };
        let renderer = Renderer::windowed(&settings, window.clone(), size.width, size.height)?;
        self.render = Some(RenderThread::start(renderer)?);
        #[cfg(feature = "dev")]
        {
            self.egui_winit = Some(WindowInput::new(&self.egui, ViewportId::ROOT, &window));
            self.viewports = Some(Viewports::new(
                &self.egui,
                event_loop,
                window.clone(),
                self.chrome,
            ));
        }
        self.cx = Context::of_run(Some(crate::Window::new(window.clone())), self.app, true);
        api::debug(&self.api, &mut self.cx);
        self.gamepads = Gamepads::open(window.has_focus());
        self.window = Some(window);
        let snapshot = self.snapshot.take();
        start(
            &mut self.app.stack,
            &mut self.cx,
            &mut self.stepper,
            snapshot,
            self.args.stopped,
            self.api.is_some(),
        )?;
        Ok(())
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let _zone = gg_core::zone!();
        // Before the frame's ticks, which apply what the pads did.
        let Self {
            gamepads, app, cx, ..
        } = self;
        gamepads.poll(|event| {
            let _ = dispatch(&mut app.stack, cx, &event);
        });
        self.take_pause();
        if api::serve(
            &mut self.api,
            &mut self.cx,
            &mut self.app.stack,
            &mut self.stepper,
        ) {
            debug!("A tool asked the run to end");
            event_loop.exit();
            return;
        }
        if self.cx.exiting() {
            debug!("A layer asked the run to end");
            event_loop.exit();
            return;
        }
        #[cfg(feature = "dev")]
        let (Some(window), Some(render), Some(egui_winit), Some(viewports)) = (
            &self.window,
            &mut self.render,
            &mut self.egui_winit,
            &self.viewports,
        ) else {
            return;
        };
        #[cfg(not(feature = "dev"))]
        let (Some(window), Some(render)) = (&self.window, &mut self.render) else {
            return;
        };
        #[cfg(feature = "dev")]
        let input = self.egui_runs.then(|| viewports.input(egui_winit));
        #[cfg(all(target_os = "windows", feature = "dev"))]
        if let Some(frame) = &self.frame {
            self.dock.set_maximize(frame.maximize());
        }
        // egui opens the windows of their own during the main window's pass.
        #[cfg(feature = "dev")]
        let pass = viewports::with_event_loop(event_loop, || {
            run_frame(
                &mut self.app.stack,
                &mut self.cx,
                self.clock.start_frame(),
                &mut self.stepper,
                input.map(|input| (&self.egui, &mut self.dock, input)),
            )
        });
        #[cfg(feature = "dev")]
        let mut output = pass.map(|(output, layers_draw)| {
            self.egui_runs = keeps_egui(&self.app.stack, layers_draw, self.chrome, &self.dock);
            output
        });
        #[cfg(feature = "dev")]
        let (ui, tessellated) = match &mut output {
            Some(output) => tessellate(&self.egui, output),
            None => (MainUi::none(), Duration::ZERO),
        };
        // Without `dev`, which has no UI, frames run no pass of egui's and hand the renderer none.
        #[cfg(not(feature = "dev"))]
        let (ui, tessellated) = {
            let dt = self.clock.start_frame();
            run_frame(
                &mut self.app.stack,
                &mut self.cx,
                dt,
                &mut self.stepper,
                None,
            );
            (MainUi::none(), Duration::ZERO)
        };
        // The frame before, which rendered meanwhile.
        let (mut minimized, mut failed) = (false, None);
        match take_back(render, &mut self.cx, tessellated) {
            Ok(Some(true)) => {
                self.frames += 1;
                if self.args.frames.is_some_and(|limit| self.frames >= limit) {
                    event_loop.exit();
                }
            }
            // Nothing was presented. Without a window area, rendering waits for a resize;
            // otherwise the swapchain is rebuilt in time for the next frame.
            Ok(Some(false)) => {
                let size = window.inner_size();
                minimized = size.width == 0 || size.height == 0;
            }
            Ok(None) => {}
            Err(error) => {
                failed = Some(error);
                event_loop.exit();
            }
        }
        api::frame_back(&mut self.api, render, &self.cx, self.stepper.ticks);
        // A run that's ending hands over no more frames: one past `--frames` would present.
        #[cfg(feature = "dev")]
        let mut close = false;
        if !event_loop.exiting() {
            let renderer = render.renderer();
            if let Some(size) = self.resized.take() {
                renderer.resize(size.width, size.height);
            }
            #[cfg(feature = "dev")]
            let (view, windows) = match &mut output {
                Some(output) => {
                    let outputs = mem::take(&mut output.viewport_output);
                    let windows;
                    (windows, close) =
                        viewports.end_frame(&self.egui, egui_winit, outputs, renderer);
                    let view = Viewport::from_points(self.dock.view(), ui.pixels_per_point);
                    (view, windows)
                }
                // Without egui, the game's view is the whole window.
                None => {
                    let size = window.inner_size();
                    let view = Viewport {
                        origin: [0, 0],
                        size: [size.width, size.height],
                    };
                    (view, Vec::new())
                }
            };
            // Without egui, the game's view is the whole window.
            #[cfg(not(feature = "dev"))]
            let (view, windows) = {
                let size = window.inner_size();
                let view = Viewport {
                    origin: [0, 0],
                    size: [size.width, size.height],
                };
                (view, Vec::new())
            };
            hand_off(render, &mut self.cx, view, ui, windows);
        }
        #[cfg(all(target_os = "windows", feature = "dev"))]
        if let Some(frame) = &self.frame {
            let ppp = egui_winit::pixels_per_point(&self.egui, window);
            frame.set_caption(self.dock.caption(), ppp);
        }
        // egui's requests of the main window, such as the cursor to show, which don't wait for
        // the frame to render.
        #[cfg(feature = "dev")]
        if let Some(output) = output {
            (egui_winit.state).handle_platform_output(window, output.platform_output);
        }
        if minimized {
            self.set_minimized(true);
        }
        if let Some(error) = failed {
            self.fail(event_loop, error);
        }
        if let Err(error) = self.app.stack.recorded() {
            self.fail(event_loop, error);
        }
        #[cfg(feature = "dev")]
        if close {
            self.close(event_loop);
        }
    }

    /// Opens the OS's window menu where the title bar was right-clicked, if it was since this last
    /// ran, between frames rather than in one. On Windows the menu runs a loop of its own until it
    /// closes, which passes on no redraws, so frames pause meanwhile: the clock restarts as the
    /// menu closes, so that neither the frame before nor the update after counts the pause.
    /// Wayland's compositor shows the menu while frames go on.
    #[cfg(feature = "dev")]
    fn show_window_menu(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let ppp = egui_winit::pixels_per_point(&self.egui, window);
        let at = (self.dock.take_window_menu())
            .map(|pos| Position::from(PhysicalPosition::new(pos.x * ppp, pos.y * ppp)));
        // In Windows' frame, the title bar's free space is the OS's, whose right-click the frame
        // keeps.
        #[cfg(target_os = "windows")]
        let at = (self.frame.as_ref())
            .and_then(Frame::take_window_menu)
            .map(Position::from)
            .or(at);
        if let Some(at) = at {
            let waiting = api::waiting(Waiting::Menu);
            window.show_window_menu(at);
            drop(waiting);
            #[cfg(target_os = "windows")]
            self.clock.resume();
        }
    }

    /// Restarts the clock if a layer paused the frames since it was last read, as a file dialog
    /// does while it's open, so that neither the frame the pause came in nor the update after it
    /// counts the pause. Where the clock is read, as a frame starts and as the run ends, rather
    /// than as a frame ends, so that it covers a dialog that a layer shows for an event between
    /// frames, such as the OS's close button, as well as one from its update or its UI.
    fn take_pause(&mut self) {
        if mem::take(&mut self.cx.paused) {
            self.clock.resume();
        }
    }

    /// Asks the layers whether the app should close, for the OS's close button or egui's Close
    /// command to the main window, and exits unless one handles it.
    fn close(&mut self, event_loop: &ActiveEventLoop) {
        if self.dispatch(&Event::CloseRequested).is_continue() {
            debug!("Window closed");
            event_loop.exit();
        }
    }

    /// Tells the layers when the window loses or regains its area, which is when rendering
    /// pauses and resumes. The clock restarts at both, so that no frame counts the pause: neither
    /// the first after it nor, in the frame times, the last before it, should the app quit while
    /// it's minimized.
    fn set_minimized(&mut self, minimized: bool) {
        if self.minimized == minimized {
            return;
        }
        self.minimized = minimized;
        self.waiting = minimized.then(|| api::waiting(Waiting::Minimized));
        self.clock.resume();
        if minimized {
            debug!("Window minimized; rendering pauses until it's restored");
            api::frames_paused(&mut self.api, &mut self.cx);
            let _ = self.dispatch(&Event::Minimized);
        } else {
            debug!("Window restored; rendering resumes");
            let Self {
                gamepads, app, cx, ..
            } = self;
            gamepads.restored(|event| {
                let _ = dispatch(&mut app.stack, cx, &event);
            });
            let _ = self.dispatch(&Event::Restored);
        }
    }

    fn dispatch(&mut self, event: &Event<'_>) -> ControlFlow<()> {
        dispatch(&mut self.app.stack, &mut self.cx, event)
    }

    /// What of `event`, from the main window, reaches the layers, `consumed` saying whether egui
    /// keeps it: in a dockspace, what its view says, and otherwise the event, which egui doesn't
    /// keep.
    #[cfg(feature = "dev")]
    fn view_reach(&mut self, event: &Event<'_>, consumed: bool) -> Reach {
        let Some(input) = &mut self.view_input else {
            return Reach::Event;
        };
        let under = under(&self.dock, &self.egui, self.egui_winit.as_ref());
        let shown = self.dock.view().is_positive();
        input.route(&self.egui, event, consumed, under, shown)
    }

    /// Whether keys reach the layers: in a dockspace, as its view says; otherwise while none of
    /// egui's widgets has focus.
    #[cfg(feature = "dev")]
    fn keys(&self) -> bool {
        match &self.view_input {
            Some(input) => {
                let under = under(&self.dock, &self.egui, self.egui_winit.as_ref());
                input.keys(&self.egui, under, self.dock.view().is_positive())
            }
            None => !self.egui.egui_wants_keyboard_input(),
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: Error) {
        self.error.get_or_insert(error);
        event_loop.exit();
    }

    /// Asks for a frame, unless the window has no area to render into or the run is ending.
    fn request_frame(&self, event_loop: &ActiveEventLoop) {
        if !self.minimized
            && !event_loop.exiting()
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
    }
}

fn dispatch(stack: &mut Stack, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
    // Every event but the cursor's moves, which would drown the rest.
    if !matches!(event, Event::CursorMoved { .. }) {
        trace!(?event, "Event");
    }
    stack.event(cx, event)
}

/// What's at the cursor in the main window, whose input egui reads through `input`, as `dock` drew
/// it in the last frame.
#[cfg(feature = "dev")]
fn under(dock: &Dock, egui: &egui::Context, input: Option<&WindowInput>) -> Under {
    let cursor = input.map(WindowInput::cursor);
    cursor.map_or(Under::Other, |cursor| dock.under(egui, cursor))
}

impl ApplicationHandler for Handler<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // Desktop platforms resume once, at startup.
        if self.window.is_some() {
            return;
        }
        if let Err(error) = self.open(event_loop) {
            self.fail(event_loop, error);
        } else if self.args.frames == Some(0) {
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        #[cfg(feature = "dev")]
        if let (Some(window), Some(viewports)) = (&self.window, &self.viewports)
            && window.id() != id
        {
            viewports.window_event(id, &event);
            return;
        }
        // Without `dev`, which has no UI, the main window is the only one, and its events go
        // straight to the layers.
        #[cfg(not(feature = "dev"))]
        let _ = id;
        // egui, the top overlay, sees each event first, and keeps the input it wants: a click
        // on one of its panels, or keys while one of its widgets has focus. Tab is the
        // exception: egui-winit keeps every Tab press, for egui to move focus between its
        // widgets, so while keys reach the layers, Tab goes past egui to them alone: while none
        // of egui's widgets has focus, and in a dockspace, as its view says, while no modal of
        // egui's is open, the view shows, and the view has the keys or the cursor. Once egui has
        // stopped running, as it does in an app that draws no UI, it sees none.
        #[cfg(feature = "dev")]
        let tab_for_layers = matches!(
            &event,
            WindowEvent::KeyboardInput { event, .. }
                if event.logical_key == Key::Named(NamedKey::Tab)
        ) && self.keys();
        #[cfg(feature = "dev")]
        let mut consumed = false;
        #[cfg(feature = "dev")]
        if self.egui_runs
            && !matches!(event, WindowEvent::RedrawRequested)
            && !tab_for_layers
            && let (Some(window), Some(egui_winit)) = (&self.window, &mut self.egui_winit)
        {
            consumed = egui_winit.on_window_event(window, &event).consumed;
        }
        // Releases reach the layers regardless: egui keeps input by what it wants when the
        // input comes, so it would keep the release of a key held down before one of its text
        // fields took focus, or of a button pressed over the scene and let go over a panel.
        #[cfg(feature = "dev")]
        {
            consumed &= !matches!(
                &event,
                WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Released
            ) && !matches!(
                event,
                WindowEvent::MouseInput {
                    state: ElementState::Released,
                    ..
                }
            );
        }
        // In a dockspace, the view decides what reaches the layers, from what egui keeps among the
        // rest.
        #[cfg(feature = "dev")]
        let kept = consumed && self.view_input.is_none();
        match event {
            WindowEvent::Resized(size) => {
                self.resized = Some(size);
                if size.width == 0 || size.height == 0 {
                    self.set_minimized(true);
                    return;
                }
                self.set_minimized(false);
                let _ = self.dispatch(&Event::Resized {
                    width: size.width,
                    height: size.height,
                });
                // Frames stop while the window has no area, and while the user drags its edge,
                // Windows doesn't call `about_to_wait` to start them again.
                self.request_frame(event_loop);
            }
            WindowEvent::RedrawRequested => {
                // No frame runs while the window has no area, or it would count the pause: a
                // redraw asked for before the window was minimized still comes, and as Windows
                // restores it, a redraw comes before the resize that ends the pause.
                if !self.minimized {
                    self.frame(event_loop);
                    #[cfg(feature = "dev")]
                    self.show_window_menu();
                }
                // Windows doesn't call `about_to_wait` while it moves or resizes one of the app's
                // windows, in a loop of its own, so each frame asks for the next. winit calls
                // `about_to_wait` between two frames otherwise, and the two requests make one.
                self.request_frame(event_loop);
            }
            // Untraced, as `dispatch` leaves the cursor's moves.
            #[cfg(feature = "dev")]
            WindowEvent::CursorMoved { .. } if kept => {}
            #[cfg(feature = "dev")]
            _ if kept => trace!(?event, "Event kept by the UI"),
            WindowEvent::CloseRequested => self.close(event_loop),
            other => event::convert(&other, |event| {
                #[cfg(feature = "dev")]
                match self.view_reach(&event, consumed) {
                    Reach::Nothing => {
                        if !matches!(event, Event::CursorMoved { .. }) {
                            trace!(?event, "Event kept from the layers");
                        }
                        return;
                    }
                    Reach::MoveThen { x, y } => {
                        let _ = self.dispatch(&Event::CursorMoved { x, y });
                    }
                    Reach::Event => {}
                }
                // The pads let go first, as the window loses focus.
                if let Event::Focused(focused) = event {
                    let Self {
                        gamepads, app, cx, ..
                    } = &mut *self;
                    gamepads.focus(focused, |event| {
                        let _ = dispatch(&mut app.stack, cx, &event);
                    });
                }
                let _ = self.dispatch(&event);
            }),
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Frames run back to back, paced by the render thread, which presentation paces: mailbox's
        // to the GPU, FIFO's (`--vsync`) to the display.
        self.request_frame(event_loop);
    }

    /// A request has come to the tool API: frames answer requests as they start, but while the
    /// window is minimized, and no frames run, it's answered here, unless it needs a frame.
    fn user_event(&mut self, event_loop: &ActiveEventLoop, (): ()) {
        if self.minimized
            && api::serve_paused(
                &mut self.api,
                &mut self.cx,
                &mut self.app.stack,
                &mut self.stepper,
            )
        {
            debug!("A tool asked the run to end");
            event_loop.exit();
        }
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        // The last frame ends here, and the frame handed over last renders, and counts, before
        // the run's end is timed.
        self.take_pause();
        (self.cx.perf).finish(self.clock.since_last(Instant::now()));
        let stopped = self.render.take().and_then(|render| {
            match panic::catch_unwind(AssertUnwindSafe(|| render.stop())) {
                Ok(stopped) => Some(stopped),
                Err(panic) => {
                    self.panic = Some(panic);
                    None
                }
            }
        });
        let stopping = Instant::now();
        let _zone = gg_core::zone!("shutdown");
        #[cfg(feature = "dev")]
        self.dock.save();
        if let Some((renderer, last)) = stopped {
            match last.map(|last| last.presented) {
                Some(Ok(true)) => self.frames += 1,
                Some(Err(error)) => {
                    self.error.get_or_insert(error);
                }
                _ => {}
            }
            self.app.stack.detach(&mut self.cx);
            // The recording's end, which detaching writes.
            if let Err(error) = self.app.stack.recorded() {
                self.error.get_or_insert(error);
            }
            if let Some(path) = &self.pipelines {
                pipeline_cache::save(path, &renderer);
            }
            let perf = mem::take(&mut self.cx.perf);
            self.ran = Some(Ran::finish(
                Some(renderer),
                self.frames,
                &self.clock,
                &self.stepper,
                perf,
                stopping,
            ));
        }
        #[cfg(feature = "dev")]
        {
            self.viewports = None;
        }
        self.gamepads = Gamepads::default();
        self.cx = Context::new(None);
        #[cfg(feature = "dev")]
        {
            self.egui_winit = None;
        }
        self.window = None;
    }
}

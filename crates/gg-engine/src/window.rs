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

use egui::ViewportId;
use gg_core::{Error, Result};
use gg_renderer::{Renderer, Settings, Viewport};
use tracing::{debug, info, trace};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize, Position};
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::args::Args;
use crate::chrome::{self, Chrome};
use crate::dock::{self, Dock, Under};
#[cfg(feature = "dev")]
use crate::hot_reload::HotReload;
use crate::layer::Stack;
use crate::pads::Gamepads;
use crate::pipeline_cache;
use crate::render_thread::RenderThread;
use crate::viewports::{self, Viewports, WindowInput};
#[cfg(target_os = "windows")]
use crate::windows_frame::Frame;
use crate::{
    App, Clock, Context, Event, MainUi, MouseButton, Ran, Stepper, egui_context, event, hand_off,
    keeps_egui, run_frame, settings, take_back, tessellate, viewport,
};

pub(crate) fn run(app: &mut App, args: &Args) -> Result<Ran> {
    let event_loop = EventLoop::new().map_err(|error| {
        Error::new(
            "engine.window",
            "can't connect to the window system; run with --headless to render without a window",
        )
        .with_source(error)
    })?;
    let layout = dock::path(app.name, std::env::consts::OS, |name| {
        std::env::var_os(name)
    });
    let pipelines = pipeline_cache::path(app.name, std::env::consts::OS, |name| {
        std::env::var_os(name)
    });
    let chrome = Chrome::new(app.custom_title_bar);
    let view_input = app.view_panel.map(|_| ViewInput::new());
    let mut handler = Handler {
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
        chrome,
        window: None,
        #[cfg(target_os = "windows")]
        frame: None,
        render: None,
        #[cfg(feature = "dev")]
        hot_reload: None,
        resized: None,
        egui: egui_context(),
        egui_runs: true,
        egui_winit: None,
        view_input,
        viewports: None,
        gamepads: Gamepads::default(),
        cx: Context::new(None),
        clock: Clock::default(),
        stepper: Stepper::default(),
        frames: 0,
        minimized: false,
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
    /// Who draws the windows' frames.
    chrome: Chrome,
    window: Option<Arc<Window>>,
    /// Windows' own frame around the main window, when the UI draws its title bar.
    #[cfg(target_os = "windows")]
    frame: Option<Frame>,
    /// Renders the frames, each while the next runs. Before `viewports` and `cx`, which hold the
    /// windows too, so that on a way out that leaves it running, it takes the renderer back as it
    /// drops, and the renderer drops here, before the windows it presents to.
    render: Option<RenderThread>,
    /// The pipeline cache's file, which the renderer loads as it starts and saves as it ends.
    pipelines: Option<PathBuf>,
    /// Watches the files of the shaders the layers add, from when they attach.
    #[cfg(feature = "dev")]
    hot_reload: Option<HotReload>,
    /// The main window's size since the renderer was last told it, which it's told once it's back
    /// from rendering.
    resized: Option<PhysicalSize<u32>>,
    egui: egui::Context,
    /// Whether frames run egui's pass, as they do until one shows that it has nothing to draw.
    egui_runs: bool,
    /// egui's view of the window, which turns its events into egui's input.
    egui_winit: Option<WindowInput>,
    /// In a dockspace, what of the main window's input reaches the layers through the game's view.
    view_input: Option<ViewInput>,
    /// The windows of their own that panels tear off into.
    viewports: Option<Viewports>,
    /// Opened with the window, since Windows gives a pad's input only to a focused window.
    gamepads: Gamepads,
    dock: Dock,
    cx: Context,
    clock: Clock,
    /// Steps the simulation in real time.
    stepper: Stepper,
    frames: u64,
    /// Set while the window has no area to render into.
    minimized: bool,
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
        let attributes = chrome::attributes(self.chrome, attributes);
        let window = event_loop.create_window(attributes).map_err(|error| {
            Error::new("engine.window", "can't create the window").with_source(error)
        })?;
        #[cfg(target_os = "windows")]
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
        self.egui_winit = Some(WindowInput::new(&self.egui, ViewportId::ROOT, &window));
        self.viewports = Some(Viewports::new(
            &self.egui,
            event_loop,
            window.clone(),
            self.chrome,
        ));
        self.cx = Context::of_run(Some(crate::Window::new(window.clone())), self.app.seed);
        self.gamepads = Gamepads::open(window.has_focus());
        self.window = Some(window);
        self.app.stack.attach(&mut self.cx);
        #[cfg(feature = "dev")]
        {
            let started = HotReload::start().inspect_err(|error| {
                let consequence = "shaders don't reload as their files change";
                tracing::warn!(code = error.code(), "{error:#}; {consequence}");
            });
            self.hot_reload = started.ok();
        }
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
        let (Some(window), Some(render), Some(egui_winit), Some(viewports)) = (
            &self.window,
            &mut self.render,
            &mut self.egui_winit,
            &self.viewports,
        ) else {
            return;
        };
        let input = self.egui_runs.then(|| viewports.input(egui_winit));
        #[cfg(target_os = "windows")]
        if let Some(frame) = &self.frame {
            self.dock.set_maximize(frame.maximize());
        }
        // egui opens the windows of their own during the main window's pass.
        let pass = viewports::with_event_loop(event_loop, || {
            run_frame(
                &mut self.app.stack,
                &mut self.cx,
                self.clock.start_frame(),
                &mut self.stepper,
                input.map(|input| (&self.egui, &mut self.dock, input)),
            )
        });
        let mut output = pass.map(|(output, layers_draw)| {
            self.egui_runs = keeps_egui(&self.app.stack, layers_draw, self.chrome, &self.dock);
            output
        });
        let (ui, tessellated) = match &mut output {
            Some(output) => tessellate(&self.egui, output),
            None => (MainUi::none(), Duration::ZERO),
        };
        // Shaders compiled again since, which the frame handed off below draws with.
        #[cfg(feature = "dev")]
        if let Some(hot_reload) = &mut self.hot_reload {
            hot_reload.update(self.cx.shaders());
        }
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
        // A run that's ending hands over no more frames: one past `--frames` would present.
        let mut close = false;
        if !event_loop.exiting() {
            let renderer = render.renderer();
            if let Some(size) = self.resized.take() {
                renderer.resize(size.width, size.height);
            }
            let (view, windows) = match &mut output {
                Some(output) => {
                    let outputs = mem::take(&mut output.viewport_output);
                    let windows;
                    (windows, close) =
                        viewports.end_frame(&self.egui, egui_winit, outputs, renderer);
                    (viewport(self.dock.view(), ui.pixels_per_point), windows)
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
            hand_off(render, &mut self.cx, view, ui, windows);
        }
        #[cfg(target_os = "windows")]
        if let Some(frame) = &self.frame {
            let ppp = egui_winit::pixels_per_point(&self.egui, window);
            frame.set_caption(self.dock.caption(), ppp);
        }
        // egui's requests of the main window, such as the cursor to show, which don't wait for
        // the frame to render.
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
        if close {
            self.close(event_loop);
        }
    }

    /// Opens the OS's window menu where the title bar was right-clicked, if it was since this last
    /// ran, between frames rather than in one. On Windows the menu runs a loop of its own until it
    /// closes, which passes on no redraws, so frames pause meanwhile: the clock restarts as the
    /// menu closes, so that neither the frame before nor the update after counts the pause.
    /// Wayland's compositor shows the menu while frames go on.
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
            window.show_window_menu(at);
            #[cfg(target_os = "windows")]
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
        self.clock.resume();
        if minimized {
            debug!("Window minimized; rendering pauses until it's restored");
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
    fn view_reach(&mut self, event: &Event<'_>, consumed: bool) -> Reach {
        let Some(input) = &mut self.view_input else {
            return Reach::Event;
        };
        let cursor = self.egui_winit.as_ref().map(WindowInput::cursor);
        let under = cursor.map_or(Under::Other, |cursor| self.dock.under(&self.egui, cursor));
        let shown = self.dock.view().is_positive();
        input.route(&self.egui, event, consumed, under, shown)
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
        if let (Some(window), Some(viewports)) = (&self.window, &self.viewports)
            && window.id() != id
        {
            viewports.window_event(id, &event);
            return;
        }
        // egui, the top overlay, sees each event first, and keeps the input it wants: a click
        // on one of its panels, or keys while one of its widgets has focus. Tab is the
        // exception: egui-winit keeps every Tab press, for egui to move focus between its
        // widgets, so while none has focus, Tab goes past egui to the layers, as it does in a
        // dockspace while the view has the keys. Once egui has stopped running, as it does in an
        // app that draws no UI, it sees none.
        let shown = self.dock.view().is_positive();
        let tab_for_layers = matches!(
            &event,
            WindowEvent::KeyboardInput { event, .. }
                if event.logical_key == Key::Named(NamedKey::Tab)
        ) && !self.egui.egui_wants_keyboard_input()
            && (self.view_input.as_ref()).is_none_or(|input| input.keys(shown));
        let mut consumed = false;
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
        // In a dockspace, the view decides what reaches the layers, from what egui keeps among the
        // rest.
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
                    self.show_window_menu();
                }
                // Windows doesn't call `about_to_wait` while it moves or resizes one of the app's
                // windows, in a loop of its own, so each frame asks for the next. winit calls
                // `about_to_wait` between two frames otherwise, and the two requests make one.
                self.request_frame(event_loop);
            }
            // Untraced, as `dispatch` leaves the cursor's moves.
            WindowEvent::CursorMoved { .. } if kept => {}
            _ if kept => trace!(?event, "Event kept by the UI"),
            WindowEvent::CloseRequested => self.close(event_loop),
            other => event::convert(&other, |event| {
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

    fn exiting(&mut self, _: &ActiveEventLoop) {
        // The last frame ends here, and the frame handed over last renders, and counts, before
        // the run's end is timed.
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
        if let Some(hot_reload) = self.hot_reload.take() {
            hot_reload.stop();
        }
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
                renderer,
                self.frames,
                &self.clock,
                &self.stepper,
                perf,
                stopping,
            ));
        }
        self.viewports = None;
        self.gamepads = Gamepads::default();
        self.cx = Context::new(None);
        self.egui_winit = None;
        self.window = None;
    }
}

/// What of the main window's input reaches the layers in a dockspace, whose game's view is a panel
/// among egui's: the mouse's presses and its wheel over the view, and the drags those start, and
/// keys while the view has focus, which a press on it or on the UI over it gives and a press
/// elsewhere takes away. Releases reach them wherever they come, so that none is left holding what
/// it saw pressed.
struct ViewInput {
    /// Whether the view has the keys, as it does from the start.
    focused: bool,
    /// The mouse buttons pressed over the view and not yet let go, a bit each, as [`bit`] gives.
    held: u8,
    /// The cursor's last move, if the layers didn't get it, in physical pixels.
    withheld: Option<(f64, f64)>,
}

/// What of an event reaches the layers.
#[derive(Debug, PartialEq)]
enum Reach {
    Nothing,
    Event,
    /// The cursor's move to `x` and `y`, which they didn't get, then the event: a press, which a
    /// drag then starts from.
    MoveThen {
        x: f64,
        y: f64,
    },
}

impl ViewInput {
    fn new() -> Self {
        Self {
            focused: true,
            held: 0,
            withheld: None,
        }
    }

    /// Whether keys reach the layers: while the view has focus and shows (`shown`).
    fn keys(&self, shown: bool) -> bool {
        self.focused && shown
    }

    /// What of `event` reaches the layers, as egui keeps it if `consumed`, with the cursor over
    /// what `under` says, as the view, which `shown` says shows. Releases always reach them. A
    /// press over the view, or over the UI over it, gives it focus and takes egui's.
    fn route(
        &mut self,
        egui: &egui::Context,
        event: &Event<'_>,
        consumed: bool,
        under: Under,
        shown: bool,
    ) -> Reach {
        let reach = |reaches: bool| {
            if reaches {
                Reach::Event
            } else {
                Reach::Nothing
            }
        };
        match *event {
            Event::MouseButton {
                button,
                pressed: true,
            } => match under {
                // egui uses the pointer for the view's own widget while a button is held there, and
                // otherwise for one of its own, such as a splitter dragged over the view.
                Under::View if self.held != 0 || !egui.egui_is_using_pointer() => {
                    self.held |= bit(button);
                    // egui keeps a text field's focus until a click elsewhere, which a drag isn't.
                    egui.memory_mut(|memory| memory.stop_text_input());
                    self.focus(true);
                    match self.withheld.take() {
                        Some((x, y)) => Reach::MoveThen { x, y },
                        None => Reach::Event,
                    }
                }
                // The UI over the view is egui's, but leaves the view the keys.
                Under::Overlay => {
                    egui.memory_mut(|memory| memory.stop_text_input());
                    self.focus(true);
                    Reach::Nothing
                }
                _ => {
                    self.focus(false);
                    Reach::Nothing
                }
            },
            Event::MouseButton {
                button,
                pressed: false,
            } => {
                self.held &= !bit(button);
                Reach::Event
            }
            Event::Scroll(_) => {
                reach(self.held != 0 || (under == Under::View && !egui.egui_is_using_pointer()))
            }
            // egui uses the pointer while a button pressed over the view is held.
            Event::CursorMoved { x, y } => {
                let reaches = !consumed || self.held != 0;
                self.withheld = (!reaches).then_some((x, y));
                reach(reaches)
            }
            Event::Key { pressed: true, .. } | Event::Text(_) => {
                reach(!consumed && self.keys(shown))
            }
            Event::Key { pressed: false, .. } => Reach::Event,
            Event::Focused(false) => {
                self.held = 0;
                Reach::Event
            }
            _ => reach(!consumed),
        }
    }

    fn focus(&mut self, focused: bool) {
        if self.focused != focused {
            self.focused = focused;
            debug!(focused, "View focus");
        }
    }
}

/// `button`'s bit among those [`ViewInput`] holds; none for a button without a name.
fn bit(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Right => 2,
        MouseButton::Middle => 4,
        MouseButton::Back => 8,
        MouseButton::Forward => 16,
        MouseButton::Other(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use egui::{Id, PointerButton, RawInput, Rect, Sense, pos2, vec2};

    use super::*;
    use crate::{Key, Scroll};

    /// egui, after a pass that had a widget pressed if `pressing`, so that egui uses the pointer.
    fn context(pressing: bool) -> egui::Context {
        let egui = egui::Context::default();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0));
        let at = pos2(50.0, 50.0);
        let press = egui::Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        };
        // The widget is in egui's hit test from the pass after it's drawn.
        for events in [vec![egui::Event::PointerMoved(at)], vec![press]] {
            let events = if pressing { events } else { Vec::new() };
            let input = RawInput {
                screen_rect: Some(screen),
                events,
                ..RawInput::default()
            };
            let mut output = egui.run_ui(input, |ui| {
                ui.interact(screen, Id::unique("widget"), Sense::drag());
            });
            output.textures_delta.clear();
        }
        assert_eq!(egui.egui_is_using_pointer(), pressing);
        egui
    }

    fn button(button: MouseButton, pressed: bool) -> Event<'static> {
        Event::MouseButton { button, pressed }
    }

    fn key(pressed: bool) -> Event<'static> {
        Event::Key {
            key: Key::KeyW,
            pressed,
            repeat: false,
        }
    }

    const WHEEL: Event<'static> = Event::Scroll(Scroll::Lines { x: 0.0, y: 1.0 });
    const MOVE: Event<'static> = Event::CursorMoved { x: 10.0, y: 20.0 };

    #[test]
    fn the_view_has_the_keys_from_the_start_and_from_a_press_on_it() {
        let egui = context(false);
        let mut input = ViewInput::new();
        // Before it first shows, keys reach nothing.
        assert_eq!(
            input.route(&egui, &key(true), false, Under::Other, false),
            Reach::Nothing
        );
        for event in [key(true), Event::Text("w"), key(false)] {
            assert_eq!(
                input.route(&egui, &event, false, Under::Other, true),
                Reach::Event
            );
        }
        // A press elsewhere takes the keys away, but not their releases.
        let elsewhere = button(MouseButton::Left, true);
        assert_eq!(
            input.route(&egui, &elsewhere, true, Under::Other, true),
            Reach::Nothing
        );
        assert!(!input.keys(true));
        for (event, reach) in [
            (key(true), Reach::Nothing),
            (Event::Text("w"), Reach::Nothing),
            (key(false), Reach::Event),
        ] {
            assert_eq!(input.route(&egui, &event, false, Under::Other, true), reach);
        }
        // A press on the view, which egui counts as its own, gives them back.
        let press = button(MouseButton::Left, true);
        assert_eq!(
            input.route(&egui, &press, true, Under::View, true),
            Reach::Event
        );
        assert!(input.keys(true));
        // Not those egui keeps, as while one of its widgets has focus.
        assert_eq!(
            input.route(&egui, &key(true), true, Under::View, true),
            Reach::Nothing
        );
    }

    #[test]
    fn a_press_on_the_view_takes_egui_s_focus() {
        let egui = context(false);
        egui.memory_mut(|memory| memory.request_focus(Id::unique("text")));
        assert!(egui.egui_wants_keyboard_input());
        let mut input = ViewInput::new();
        let press = button(MouseButton::Right, true);
        assert_eq!(
            input.route(&egui, &press, true, Under::View, true),
            Reach::Event
        );
        assert!(!egui.egui_wants_keyboard_input());
    }

    #[test]
    fn a_button_held_from_the_view_brings_the_layers_moves_and_the_wheel() {
        let egui = context(false);
        let mut input = ViewInput::new();
        let right = |pressed| button(MouseButton::Right, pressed);
        assert_eq!(
            input.route(&egui, &right(true), true, Under::View, true),
            Reach::Event
        );
        // egui uses the pointer for the view's widget now, and the drag goes past the view.
        let using = context(true);
        for event in [MOVE, WHEEL] {
            assert_eq!(
                input.route(&using, &event, true, Under::Other, true),
                Reach::Event
            );
        }
        // Another button pressed over the view joins in.
        let left = |pressed| button(MouseButton::Left, pressed);
        assert_eq!(
            input.route(&using, &left(true), true, Under::View, true),
            Reach::Event
        );
        assert_eq!(
            input.route(&using, &right(false), true, Under::Other, true),
            Reach::Event
        );
        assert_eq!(
            input.route(&using, &MOVE, true, Under::Other, true),
            Reach::Event
        );
        assert_eq!(
            input.route(&using, &left(false), true, Under::Other, true),
            Reach::Event
        );
        // Let go, egui's moves and wheel are egui's.
        for event in [MOVE, WHEEL] {
            assert_eq!(
                input.route(&using, &event, true, Under::View, true),
                Reach::Nothing
            );
        }
        // The layers get the move they missed before the next press, which a drag starts from.
        let next = input.route(&egui, &right(true), true, Under::View, true);
        assert_eq!(next, Reach::MoveThen { x: 10.0, y: 20.0 });
        // A move they get leaves none to miss.
        input.route(&egui, &right(false), false, Under::View, true);
        input.route(&egui, &MOVE, false, Under::View, true);
        assert_eq!(
            input.route(&egui, &right(true), true, Under::View, true),
            Reach::Event
        );
    }

    #[test]
    fn a_press_over_the_view_while_egui_drags_one_of_its_own_is_egui_s() {
        let using = context(true);
        let mut input = ViewInput::new();
        let press = button(MouseButton::Right, true);
        assert_eq!(
            input.route(&using, &press, true, Under::View, true),
            Reach::Nothing
        );
        assert!(!input.keys(true));
        assert_eq!(
            input.route(&using, &WHEEL, true, Under::View, true),
            Reach::Nothing
        );
    }

    #[test]
    fn the_wheel_over_the_view_reaches_the_layers_and_elsewhere_doesn_t() {
        let egui = context(false);
        let mut input = ViewInput::new();
        assert_eq!(
            input.route(&egui, &WHEEL, true, Under::View, true),
            Reach::Event
        );
        assert_eq!(
            input.route(&egui, &WHEEL, true, Under::Other, true),
            Reach::Nothing
        );
    }

    #[test]
    fn a_press_on_ui_over_the_view_gives_it_the_keys_and_reaches_egui_alone() {
        let egui = context(false);
        egui.memory_mut(|memory| memory.request_focus(Id::unique("text")));
        let mut input = ViewInput::new();
        let left = |pressed| button(MouseButton::Left, pressed);
        assert_eq!(
            input.route(&egui, &left(true), true, Under::Other, true),
            Reach::Nothing
        );
        assert!(!input.keys(true));
        assert_eq!(
            input.route(&egui, &left(true), true, Under::Overlay, true),
            Reach::Nothing
        );
        assert!(input.keys(true));
        assert!(!egui.egui_wants_keyboard_input());
        // Its release reaches the layers, as every release does, but it holds nothing.
        assert_eq!(
            input.route(&egui, &left(false), true, Under::Overlay, true),
            Reach::Event
        );
        for event in [MOVE, WHEEL] {
            assert_eq!(
                input.route(&egui, &event, true, Under::Overlay, true),
                Reach::Nothing
            );
        }
    }

    #[test]
    fn losing_the_window_s_focus_lets_go_of_what_s_held() {
        let egui = context(false);
        let mut input = ViewInput::new();
        let press = button(MouseButton::Right, true);
        assert_eq!(
            input.route(&egui, &press, true, Under::View, true),
            Reach::Event
        );
        let lost = Event::Focused(false);
        assert_eq!(
            input.route(&egui, &lost, false, Under::View, true),
            Reach::Event
        );
        assert_eq!(
            input.route(&egui, &MOVE, true, Under::View, true),
            Reach::Nothing
        );
        // The view keeps the keys.
        assert!(input.keys(true));
    }

    #[test]
    fn buttons_without_a_name_reach_the_layers_but_hold_nothing() {
        let egui = context(false);
        let mut input = ViewInput::new();
        let other = button(MouseButton::Other(7), true);
        assert_eq!(
            input.route(&egui, &other, true, Under::View, true),
            Reach::Event
        );
        assert_eq!(
            input.route(&egui, &MOVE, true, Under::View, true),
            Reach::Nothing
        );
    }
}

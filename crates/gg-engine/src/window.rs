//! Windowed runs: the winit event loop hands the main window's events to the layers and runs a
//! frame whenever the window can show one. Each frame renders on the render thread while the next
//! runs.

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
use tracing::{debug, info, trace, warn};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
#[cfg(feature = "dev")]
use winit::dpi::{PhysicalPosition, Position};
#[cfg(feature = "dev")]
use winit::event::ElementState;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
#[cfg(feature = "dev")]
use winit::keyboard::{Key, NamedKey};
use winit::window::{Icon, Window, WindowId};

use crate::api::{self, Api, Waited, Waiting};
use crate::args::Args;
#[cfg(feature = "dev")]
use crate::chrome::{self, Chrome};
#[cfg(feature = "dev")]
use crate::dock::{self, Dock, Under};
#[cfg(feature = "dev")]
use crate::egui_context;
#[cfg(feature = "dev")]
use crate::frame_rate;
use crate::frame_rate::Limiter;
use crate::interrupt;
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

/// Runs `app` in a window until it closes or, under `--supervised`, stdin ends. `api` serves
/// tools as each frame starts, and while the window is minimized, woken by each request.
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
        // Fails only once the loop has ended, as the run does.
        let _ = proxy.send_event(());
    });
    if args.supervised {
        // Frames look for stdin's end, but none runs while the window is minimized.
        let proxy = event_loop.create_proxy();
        interrupt::watch_stdin(move || {
            let _ = proxy.send_event(());
        });
    }
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
        clipboard: Clipboard::default(),
        cx: Context::new(None),
        clock: Clock::default(),
        limiter: Limiter::default(),
        stepper: Stepper::default(),
        api,
        frames: 0,
        minimized: false,
        grabbed: false,
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

/// The event loop's state. The layers attach while the renderer exists.
struct Handler<'a> {
    app: &'a mut App,
    args: &'a Args,
    /// The snapshot the run starts from, restored once the window opens.
    snapshot: Option<Snapshot>,
    #[cfg(feature = "dev")]
    chrome: Chrome,
    window: Option<Arc<Window>>,
    /// Windows' frame around the main window, when the UI draws its title bar.
    #[cfg(all(target_os = "windows", feature = "dev"))]
    frame: Option<Frame>,
    /// Renders each frame while the next runs. Declared before `viewports` and `cx`, which hold
    /// windows too: it drops first and returns the renderer, which drops before its windows.
    render: Option<RenderThread>,
    /// The pipeline cache's file: loaded as the renderer starts, saved as it ends.
    pipelines: Option<PathBuf>,
    /// The main window's size since the renderer was last told; it's told once back from rendering.
    resized: Option<PhysicalSize<u32>>,
    #[cfg(feature = "dev")]
    egui: egui::Context,
    /// Whether frames run egui's pass, until one shows it has nothing to draw.
    #[cfg(feature = "dev")]
    egui_runs: bool,
    #[cfg(feature = "dev")]
    egui_winit: Option<WindowInput>,
    /// In a dockspace, which main-window input reaches the layers through the game's view.
    #[cfg(feature = "dev")]
    view_input: Option<ViewInput>,
    /// The windows panels tear off into.
    #[cfg(feature = "dev")]
    viewports: Option<Viewports>,
    /// Opened with the window: Windows gives a pad's input only to a focused window.
    gamepads: Gamepads,
    clipboard: Clipboard,
    #[cfg(feature = "dev")]
    dock: Dock,
    cx: Context,
    clock: Clock,
    limiter: Limiter,
    stepper: Stepper,
    api: Option<Api>,
    frames: u64,
    /// Set while the window has no area to render into.
    minimized: bool,
    /// Whether the cursor is grabbed, as [`hold_cursor`](Self::hold_cursor) last left it.
    grabbed: bool,
    /// Tells `ping` that frames have paused while minimized.
    waiting: Option<Waited>,
    error: Option<Error>,
    ran: Option<Ran>,
    /// A panic rendering the last frame as the loop ends. It resumes after: winit on Windows
    /// drops a panic that leaves `exiting`.
    panic: Option<Box<dyn Any + Send>>,
}

impl Handler<'_> {
    fn open(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let _zone = gg_core::zone!();
        // The app's size, or the primary monitor's if smaller.
        let [mut width, mut height] = self.app.window_size.map(f64::from);
        if let Some(monitor) = event_loop.primary_monitor() {
            let most: LogicalSize<f64> = monitor.size().to_logical(monitor.scale_factor());
            (width, height) = (width.min(most.width), height.min(most.height));
        }
        let icon = self.app.icon.and_then(|png| {
            window_icon(png)
                .inspect_err(|error| warn!(code = error.code(), "{error:#}"))
                .ok()
        });
        let attributes = Window::default_attributes()
            .with_title(self.app.name)
            .with_inner_size(LogicalSize::new(width, height));
        // The taskbar and Alt+Tab show the big icon, which winit sets apart from the title bar's.
        #[cfg(target_os = "windows")]
        let attributes = {
            use winit::platform::windows::WindowAttributesExtWindows as _;
            attributes.with_taskbar_icon(icon.clone())
        };
        let attributes = attributes.with_window_icon(icon);
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
        // What the settings kept; a flag overrides it for the run. In an app that embeds a game, it
        // caps the game.
        #[cfg(feature = "dev")]
        let kept = match frame_rate::kept(&self.cx) {
            kept if self.app.embeds_game => {
                self.cx.set_game_max_fps(kept);
                None
            }
            kept => kept,
        };
        #[cfg(not(feature = "dev"))]
        let kept = None;
        self.cx.set_max_fps(self.args.max_fps.or(kept));
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
        // Before the frame starts, so the last frame counts the wait and this one takes the pad
        // reads and tool requests that came meanwhile. Window events reach the layers after it.
        let waited = self.limiter.wait(self.cx.max_fps());
        self.cx.perf.limited(waited);
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
        if interrupt::orphaned() {
            info!("Ending the run, as stdin has ended with its supervisor");
            event_loop.exit();
            return;
        }
        // As the frames before asked.
        self.hold_cursor();
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
        #[cfg(feature = "dev")]
        if let Some(view_input) = &self.view_input {
            self.dock.set_view_focused(view_input.focused());
        }
        // egui opens the panel windows during the main window's pass.
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
        // After the pass, so after the press that asked and took the keys.
        #[cfg(feature = "dev")]
        if self.dock.take_focus_asked()
            && let Some(view_input) = &mut self.view_input
        {
            view_input.give_focus(&self.egui);
        }
        #[cfg(feature = "dev")]
        let (ui, tessellated) = match &mut output {
            Some(output) => tessellate(&self.egui, output),
            None => (MainUi::none(), Duration::ZERO),
        };
        // Without `dev` there's no UI: frames run no egui pass and hand the renderer none.
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
        if let Some(copied) = self.cx.copied.take() {
            self.clipboard.copy(copied);
        }
        // egui shows its widgets' hover cursor unless told it's hidden.
        #[cfg(feature = "dev")]
        if self.grabbed
            && let Some(output) = &mut output
        {
            output.platform_output.cursor_icon = egui::CursorIcon::None;
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
            // otherwise the swapchain is rebuilt for the next frame.
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
        // An ending run hands over no more frames: one past `--frames` would present.
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
        // egui's requests of the main window (the cursor) don't wait for the render.
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

    /// Opens the OS's window menu where the title bar was right-clicked, between frames. On Windows
    /// its loop passes on no redraws, so frames pause and the clock restarts as it closes.
    /// Wayland's compositor shows it while frames go on.
    #[cfg(feature = "dev")]
    fn show_window_menu(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let ppp = egui_winit::pixels_per_point(&self.egui, window);
        let at = (self.dock.take_window_menu())
            .map(|pos| Position::from(PhysicalPosition::new(pos.x * ppp, pos.y * ppp)));
        // In Windows' frame the free space is the OS's, and the frame keeps its right-click.
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

    /// Grabs or releases the cursor as the layers want ([`Context::set_cursor_grabbed`]), while
    /// the window has focus and, in a dockspace, the view shows and has the keys. Locks where the
    /// platform does (Windows, macOS, Wayland), else confines it (X11, which still reports raw
    /// motion past the edges); warns `engine.cursor_grab` if neither works. [`Dock::grab_at`]
    /// covers a grab starting off the view.
    fn hold_cursor(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        #[cfg(feature = "dev")]
        let view = (self.view_input.as_ref())
            .is_none_or(|input| input.focused() && self.dock.view().is_positive());
        #[cfg(not(feature = "dev"))]
        let view = true;
        let grab = self.cx.cursor_grabbed() && view && window.has_focus();
        if grab == self.grabbed {
            return;
        }
        self.grabbed = grab;
        #[cfg(feature = "dev")]
        if grab
            && let Some(input) = &self.view_input
            && let Some(at) = self.dock.grab_at(
                &self.egui,
                (self.egui_winit.as_ref())
                    .filter(|_| input.inside())
                    .map(WindowInput::cursor),
            )
        {
            let ppp = egui_winit::pixels_per_point(&self.egui, window);
            let at = PhysicalPosition::new(at.x * ppp, at.y * ppp);
            match window.set_cursor_position(at) {
                Ok(()) => debug!(x = at.x, y = at.y, "Cursor moved onto the view"),
                // Wayland moves no cursor it hasn't locked; the grab goes on where it is.
                Err(error) => debug!("The cursor can't move onto the view: {error}"),
            }
        }
        if grab {
            use winit::window::CursorGrabMode;
            let held = (window.set_cursor_grab(CursorGrabMode::Locked))
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
            if let Err(error) = held {
                warn!(
                    code = "engine.cursor_grab",
                    "The cursor can't be grabbed: {error}; it's hidden, and the mouse's raw motion \
                     still comes"
                );
            }
        } else {
            let _ = window.set_cursor_grab(winit::window::CursorGrabMode::None);
        }
        window.set_cursor_visible(!grab);
        debug!(grabbed = grab, "Cursor grab");
    }

    /// Restarts the clock if a layer paused the frames (an open file dialog does), so no frame
    /// counts the pause. Called at frame start and run end, not frame end, so it covers a dialog
    /// shown for an event between frames, such as the OS's close button.
    fn take_pause(&mut self) {
        if mem::take(&mut self.cx.paused) {
            self.clock.resume();
        }
    }

    /// Asks the layers whether to close the main window; exits unless one handles it.
    fn close(&mut self, event_loop: &ActiveEventLoop) {
        if self.dispatch(&Event::CloseRequested).is_continue() {
            debug!("Window closed");
            event_loop.exit();
        }
    }

    /// Tells the layers the window lost or regained its area, pausing or resuming rendering. The
    /// clock restarts at both, so no frame counts the pause, even the last if the app quits
    /// minimized.
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

    /// What of `event` reaches the layers, given whether egui `consumed` it. In a dockspace the
    /// view decides.
    #[cfg(feature = "dev")]
    fn view_reach(&mut self, event: &Event<'_>, consumed: bool) -> Reach {
        let Some(input) = &mut self.view_input else {
            return Reach::Event;
        };
        let under = under(&self.dock, &self.egui, self.egui_winit.as_ref());
        let shown = self.dock.view().is_positive();
        input.route(&self.egui, event, consumed, under, shown)
    }

    /// Whether keys reach the layers: in a dockspace, as its view says (no egui modal open, view
    /// shown, view has the keys or cursor); otherwise while no egui widget has focus.
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
    // Not cursor and mouse moves, which would drown the rest, nor pastes, which may be a
    // session's code.
    if !matches!(
        event,
        Event::CursorMoved { .. } | Event::MouseMotion { .. } | Event::Paste(_)
    ) {
        trace!(?event, "Event");
    }
    stack.event(cx, event)
}

/// The OS's clipboard, opened at the first paste or copy (on Linux that connects to the display
/// server). A failed open, read or write warns (`ui.clipboard`); that paste or copy does nothing.
#[derive(Default)]
struct Clipboard(Option<arboard::Clipboard>);

impl Clipboard {
    fn opened(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.0.is_none() {
            match arboard::Clipboard::new() {
                Ok(clipboard) => self.0 = Some(clipboard),
                Err(error) => warn!(
                    code = "ui.clipboard",
                    "Can't open the OS's clipboard: {error}"
                ),
            }
        }
        self.0.as_mut()
    }

    fn paste(&mut self) -> Option<String> {
        match self.opened()?.get_text() {
            Ok(text) => Some(text),
            Err(arboard::Error::ContentNotAvailable) => None,
            Err(error) => {
                warn!(
                    code = "ui.clipboard",
                    "Can't read the OS's clipboard: {error}"
                );
                None
            }
        }
    }

    fn copy(&mut self, text: String) {
        if let Some(clipboard) = self.opened()
            && let Err(error) = clipboard.set_text(text)
        {
            warn!(
                code = "ui.clipboard",
                "Can't write the OS's clipboard: {error}"
            );
        }
    }
}

/// What's at the cursor in the main window, as `dock` drew it last frame.
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
        // Without `dev` there's no UI and one window; events go straight to the layers.
        #[cfg(not(feature = "dev"))]
        let _ = id;
        // egui, the top overlay, sees each event first and keeps what it wants: clicks on its
        // panels, keys while a widget has focus. egui-winit keeps every Tab press to move focus,
        // so while keys reach the layers (see `keys`), Tab skips egui. Once egui has stopped
        // running, as in an app with no UI, it sees nothing.
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
        // Releases reach the layers regardless: egui would keep the release of a key held before
        // a text field took focus, or of a button pressed over the scene and released over a panel.
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
        // In a dockspace, the view decides what reaches the layers, among what egui keeps.
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
                // Frames stop without a window area; Windows doesn't call `about_to_wait` to
                // restart them while the user drags an edge.
                self.request_frame(event_loop);
            }
            WindowEvent::RedrawRequested => {
                // No frame runs without a window area, or it would count the pause. A redraw
                // asked for before minimizing still comes, and Windows sends one on restoring,
                // before the resize that ends the pause.
                if !self.minimized {
                    self.frame(event_loop);
                    #[cfg(feature = "dev")]
                    self.show_window_menu();
                }
                // Windows doesn't call `about_to_wait` while it moves or resizes a window in its
                // own loop, so each frame asks for the next. Otherwise winit calls it between
                // frames, and the two requests make one.
                self.request_frame(event_loop);
            }
            // Untraced, as `dispatch` leaves out cursor moves.
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
                // The pads release first, as the window loses focus.
                if let Event::Focused(focused) = event {
                    let Self {
                        gamepads, app, cx, ..
                    } = &mut *self;
                    gamepads.focus(focused, |event| {
                        let _ = dispatch(&mut app.stack, cx, &event);
                    });
                }
                let _ = self.dispatch(&event);
                // What's on the clipboard, after the key that pasted it.
                if let Event::Key {
                    key: crate::Key::KeyV,
                    pressed: true,
                    ..
                } = event
                    && self.cx.input().command_down()
                    && let Some(pasted) = self.clipboard.paste()
                {
                    let _ = self.dispatch(&Event::Paste(&pasted));
                }
            }),
        }
    }

    /// The mouse's raw motion: winit reports it while the window has focus (`WhenFocused`, its
    /// default); in a dockspace, also while the view has the keys.
    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        let Some(event) = event::device(&event) else {
            return;
        };
        #[cfg(feature = "dev")]
        if self.view_reach(&event, false) == Reach::Nothing {
            return;
        }
        let _ = self.dispatch(&event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Frames run back to back, paced by the render thread, itself paced by presentation
        // (mailbox: the GPU; FIFO under `--vsync`: the display).
        self.request_frame(event_loop);
    }

    /// A tool API request came, or stdin ended under `--supervised`. Frames handle both as they
    /// start; while minimized, it's done here, and a request is answered unless it needs a frame.
    fn user_event(&mut self, event_loop: &ActiveEventLoop, (): ()) {
        if !self.minimized {
            return;
        }
        if interrupt::orphaned() {
            info!("Ending the run, as stdin has ended with its supervisor");
            event_loop.exit();
        } else if api::serve_paused(
            &mut self.api,
            &mut self.cx,
            &mut self.app.stack,
            &mut self.stepper,
        ) {
            debug!("A tool asked the run to end");
            event_loop.exit();
        }
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        // The last frame ends here; the frame handed over last renders, and counts, before the
        // run's end is timed.
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

/// The window's icon from `png`, decoded to RGBA. Fails with `engine.icon` if it doesn't decode.
fn window_icon(png: &[u8]) -> Result<Icon> {
    let failed = |why: String| {
        let message = format!("can't give the window its icon, which {why}; it opens without one");
        Error::new("engine.icon", message)
    };
    let image = crate::image::decode_png(png).map_err(failed)?;
    let [width, height] = image.size;
    Icon::from_rgba(image.pixels, width, height).map_err(|error| failed(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG of RGBA, `width` by `height` pixels.
    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut file = Vec::new();
        let mut encoder = png::Encoder::new(&mut file, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("a header");
        let pixels = [200, 120, 40, 255].repeat(width as usize * height as usize);
        writer.write_image_data(&pixels).expect("written");
        writer.finish().expect("finished");
        file
    }

    /// winit takes the icon's RGBA only if it's as long as its size says.
    #[test]
    fn an_icon_decodes_to_rgba_of_its_size_and_anything_else_is_refused() {
        assert!(window_icon(&png(256, 256)).is_ok());
        assert!(window_icon(&png(3, 5)).is_ok());
        for refused in [&b"not a png"[..], &png(4, 4)[..20]] {
            let code = window_icon(refused).err().map(|error| error.code());
            assert_eq!(code, Some("engine.icon"));
        }
    }
}

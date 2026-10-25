//! Windowed runs: the winit event loop, which hands the main window's events to the layers and
//! runs a frame whenever the window can show one, which renders on the render thread while the
//! next runs.

use std::any::Any;
use std::mem;
use std::ops::ControlFlow;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Instant;

use egui::ViewportId;
use gg_core::{Error, Result};
use gg_renderer::Renderer;
use tracing::{debug, info, trace};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::args::Args;
use crate::chrome::{self, Chrome};
use crate::dock::{self, Dock};
use crate::layer::Stack;
use crate::pads::Gamepads;
use crate::render_thread::RenderThread;
use crate::viewports::{self, Viewports, WindowInput};
#[cfg(target_os = "windows")]
use crate::windows_frame::Frame;
use crate::{
    App, Clock, Context, Event, Ran, Stepper, egui_context, event, hand_off, run_frame, settings,
    take_back, tessellate,
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
    let chrome = Chrome::new(app.custom_title_bar);
    let mut handler = Handler {
        dock: Dock::new(layout, chrome),
        app,
        args,
        chrome,
        window: None,
        #[cfg(target_os = "windows")]
        frame: None,
        render: None,
        resized: None,
        egui: egui_context(),
        egui_winit: None,
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
    /// The main window's size since the renderer was last told it, which it's told once it's back
    /// from rendering.
    resized: Option<PhysicalSize<u32>>,
    egui: egui::Context,
    /// egui's view of the window, which turns its events into egui's input.
    egui_winit: Option<WindowInput>,
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
        let settings = settings(self.app, self.args);
        let renderer = Renderer::windowed(&settings, window.clone(), size.width, size.height)?;
        self.render = Some(RenderThread::start(renderer)?);
        self.egui_winit = Some(WindowInput::new(&self.egui, ViewportId::ROOT, &window));
        self.viewports = Some(Viewports::new(
            &self.egui,
            event_loop,
            window.clone(),
            self.chrome,
        ));
        self.cx = Context::new(Some(crate::Window::new(window.clone())));
        self.gamepads = Gamepads::open(window.has_focus());
        self.window = Some(window);
        self.app.stack.attach(&mut self.cx);
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
        let input = viewports.input(egui_winit);
        #[cfg(target_os = "windows")]
        if let Some(frame) = &self.frame {
            self.dock.set_maximize(frame.maximize());
        }
        // egui opens the windows of their own during the main window's pass.
        let mut output = viewports::with_event_loop(event_loop, || {
            run_frame(
                &mut self.app.stack,
                &mut self.cx,
                &mut self.clock,
                &mut self.stepper,
                &self.egui,
                &mut self.dock,
                input,
            )
        });
        let (ui, tessellated) = tessellate(&self.egui, &mut output);
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
        let outputs = mem::take(&mut output.viewport_output);
        let mut close = false;
        if !event_loop.exiting() {
            let renderer = render.renderer();
            if let Some(size) = self.resized.take() {
                renderer.resize(size.width, size.height);
            }
            let windows;
            (windows, close) = viewports.end_frame(&self.egui, egui_winit, outputs, renderer);
            hand_off(render, &mut self.cx, self.dock.central(), ui, windows);
        }
        let ppp = egui_winit::pixels_per_point(&self.egui, window);
        if let Some(pos) = self.dock.take_window_menu() {
            window.show_window_menu(PhysicalPosition::new(pos.x * ppp, pos.y * ppp));
        }
        #[cfg(target_os = "windows")]
        if let Some(frame) = &self.frame {
            frame.set_caption(self.dock.caption(), ppp);
            if let Some(pos) = frame.take_window_menu() {
                window.show_window_menu(pos);
            }
        }
        // egui's requests of the main window, such as the cursor to show, which don't wait for
        // the frame to render.
        (egui_winit.state).handle_platform_output(window, output.platform_output);
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
        // widgets, so while none has focus, Tab goes past egui to the layers.
        let tab_for_layers = matches!(
            &event,
            WindowEvent::KeyboardInput { event, .. }
                if event.logical_key == Key::Named(NamedKey::Tab)
        ) && !self.egui.egui_wants_keyboard_input();
        let mut consumed = false;
        if !matches!(event, WindowEvent::RedrawRequested)
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
                }
                // Windows doesn't call `about_to_wait` while it moves or resizes one of the app's
                // windows, in a loop of its own, so each frame asks for the next. winit calls
                // `about_to_wait` between two frames otherwise, and the two requests make one.
                self.request_frame(event_loop);
            }
            // Untraced, as `dispatch` leaves the cursor's moves.
            WindowEvent::CursorMoved { .. } if consumed => {}
            _ if consumed => trace!(?event, "Event kept by the UI"),
            WindowEvent::CloseRequested => self.close(event_loop),
            other => event::convert(&other, |event| {
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

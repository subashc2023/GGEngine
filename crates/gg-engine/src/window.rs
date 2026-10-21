//! Windowed runs: the winit event loop, which hands the main window's events to the layers and
//! renders a frame whenever the window can show one.

use std::mem;
use std::ops::ControlFlow;
use std::sync::Arc;

use egui::ViewportId;
use gg_core::{Error, Result};
use gg_renderer::Renderer;
use tracing::{debug, info, trace};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::args::Args;
use crate::chrome::{self, Chrome};
use crate::dock::{self, Dock};
use crate::viewports::{self, Viewports, WindowInput};
#[cfg(target_os = "windows")]
use crate::windows_frame::Frame;
use crate::{
    App, Clock, Context, Event, Ran, egui_context, event, render_frame, run_frame, settings,
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
        renderer: None,
        egui: egui_context(),
        egui_winit: None,
        viewports: None,
        cx: Context::new(None),
        clock: Clock::default(),
        frames: 0,
        minimized: false,
        error: None,
        ran: None,
    };
    event_loop.run_app(&mut handler).map_err(|error| {
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

/// The event loop's state: the window and renderer once the loop starts, and how the run went.
/// The layers are attached while the renderer exists.
struct Handler<'a> {
    app: &'a mut App,
    args: &'a Args,
    /// Who draws the windows' frames.
    chrome: Chrome,
    window: Option<Arc<Window>>,
    /// Windows' own frame around the main window, when the UI draws its title bar.
    #[cfg(target_os = "windows")]
    frame: Option<Frame>,
    renderer: Option<Renderer>,
    egui: egui::Context,
    /// egui's view of the window, which turns its events into egui's input.
    egui_winit: Option<WindowInput>,
    /// The windows of their own that panels tear off into.
    viewports: Option<Viewports>,
    dock: Dock,
    cx: Context,
    clock: Clock,
    frames: u64,
    /// Set while the window has no area to render into.
    minimized: bool,
    error: Option<Error>,
    ran: Option<Ran>,
}

impl Handler<'_> {
    fn open(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
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
        self.renderer = Some(renderer);
        self.egui_winit = Some(WindowInput::new(&self.egui, ViewportId::ROOT, &window));
        self.viewports = Some(Viewports::new(
            &self.egui,
            event_loop,
            window.clone(),
            self.chrome,
        ));
        self.cx = Context::new(Some(crate::Window::new(window.clone())));
        self.window = Some(window);
        self.app.stack.attach(&mut self.cx);
        Ok(())
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(window), Some(renderer), Some(egui_winit), Some(viewports)) = (
            &self.window,
            &mut self.renderer,
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
                &self.egui,
                &mut self.dock,
                input,
            )
        });
        let outputs = mem::take(&mut output.viewport_output);
        let (mut windows, close) = viewports.end_frame(&self.egui, egui_winit, outputs, renderer);
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
        let view = self.dock.central();
        let rendered = render_frame(
            &self.egui,
            &mut self.cx,
            view,
            output,
            &mut windows,
            renderer,
        );
        let rendered = rendered.map(|(presented, platform_output)| {
            egui_winit
                .state
                .handle_platform_output(window, platform_output);
            presented
        });
        match rendered {
            Ok(true) => {
                self.frames += 1;
                if self.args.frames.is_some_and(|limit| self.frames >= limit) {
                    event_loop.exit();
                }
            }
            // Nothing was presented. Without a window area, rendering waits for a resize;
            // otherwise the swapchain is rebuilt in time for the next frame.
            Ok(false) => {
                let size = window.inner_size();
                if size.width == 0 || size.height == 0 {
                    self.set_minimized(true);
                }
            }
            Err(error) => self.fail(event_loop, error),
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
    /// pauses and resumes; the first frame after the pause counts from its own start.
    fn set_minimized(&mut self, minimized: bool) {
        if self.minimized == minimized {
            return;
        }
        self.minimized = minimized;
        if minimized {
            debug!("Window minimized; rendering pauses until it's restored");
            let _ = self.dispatch(&Event::Minimized);
        } else {
            debug!("Window restored; rendering resumes");
            self.clock.resume();
            let _ = self.dispatch(&Event::Restored);
        }
    }

    fn dispatch(&mut self, event: &Event<'_>) -> ControlFlow<()> {
        trace!(?event, "Event");
        self.app.stack.event(&mut self.cx, event)
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: Error) {
        self.error.get_or_insert(error);
        event_loop.exit();
    }
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
        if let (Some(window), Some(viewports), Some(renderer)) =
            (&self.window, &self.viewports, &mut self.renderer)
            && window.id() != id
        {
            viewports.window_event(id, &event, renderer);
            // Windows doesn't call `about_to_wait` while the user moves or resizes a window, so
            // frames are requested here as well.
            window.request_redraw();
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
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size.width, size.height);
                }
                if size.width == 0 || size.height == 0 {
                    self.set_minimized(true);
                    return;
                }
                self.set_minimized(false);
                let _ = self.dispatch(&Event::Resized {
                    width: size.width,
                    height: size.height,
                });
                // Windows doesn't call `about_to_wait` while the user drags the window's edge,
                // so frames are requested here as well.
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.frame(event_loop),
            _ if consumed => trace!(?event, "Event kept by the UI"),
            WindowEvent::CloseRequested => self.close(event_loop),
            other => event::convert(&other, |event| {
                let _ = self.dispatch(&event);
            }),
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        // Frames render back to back, paced by presentation: mailbox's to the GPU, FIFO's
        // (`--vsync`) to the display.
        if !self.minimized
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        self.dock.save();
        if let Some(renderer) = self.renderer.take() {
            self.app.stack.detach(&mut self.cx);
            self.ran = Some(Ran::finish(renderer, self.frames, self.clock.elapsed()));
        }
        self.viewports = None;
        self.cx = Context::new(None);
        self.egui_winit = None;
        self.window = None;
    }
}

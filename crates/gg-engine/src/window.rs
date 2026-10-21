//! Windowed runs: the winit event loop, which hands events to the layers and renders a frame
//! whenever the window can show one.

use std::ops::ControlFlow;
use std::sync::Arc;

use gg_core::{Error, Result};
use gg_renderer::Renderer;
use tracing::{debug, info, trace};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use crate::args::Args;
use crate::{App, Clock, Context, Event, Ran, event, render_frame, settings};

pub(crate) fn run(app: &mut App, args: &Args) -> Result<Ran> {
    let event_loop = EventLoop::new().map_err(|error| {
        Error::new(
            "engine.window",
            "can't connect to the window system; run with --headless to render without a window",
        )
        .with_source(error)
    })?;
    let mut handler = Handler {
        app,
        args,
        window: None,
        renderer: None,
        egui: egui::Context::default(),
        egui_winit: None,
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
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    egui: egui::Context,
    /// egui's view of the window, which turns its events into egui's input.
    egui_winit: Option<egui_winit::State>,
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
        let window = event_loop.create_window(attributes).map_err(|error| {
            Error::new("engine.window", "can't create the window").with_source(error)
        })?;
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
        self.egui_winit = Some(egui_winit::State::new(
            self.egui.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        ));
        self.cx = Context::new(Some(crate::Window::new(window.clone())));
        self.window = Some(window);
        self.app.stack.attach(&mut self.cx);
        Ok(())
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(window), Some(renderer), Some(egui_winit)) =
            (&self.window, &mut self.renderer, &mut self.egui_winit)
        else {
            return;
        };
        let input = egui_winit.take_egui_input(window);
        let rendered = render_frame(
            &mut self.app.stack,
            &mut self.cx,
            &mut self.clock,
            &self.egui,
            input,
            renderer,
        );
        let rendered = rendered.map(|(presented, platform_output)| {
            egui_winit.handle_platform_output(window, platform_output);
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
    }

    /// Tells the layers when the window loses or regains its area, which is when rendering
    /// pauses and resumes.
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

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        // egui, the top overlay, sees each event first, and keeps the input it wants: a click
        // on one of its panels, or keys while one of its text fields has focus.
        let mut consumed = false;
        if !matches!(event, WindowEvent::RedrawRequested)
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
            other => {
                let mut close = false;
                event::convert(&other, |event| {
                    if self.dispatch(&event).is_continue() && event == Event::CloseRequested {
                        close = true;
                    }
                });
                if close {
                    debug!("Window closed");
                    event_loop.exit();
                }
            }
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
        if let Some(renderer) = self.renderer.take() {
            self.app.stack.detach(&mut self.cx);
            self.ran = Some(Ran::finish(renderer, self.frames, self.clock.elapsed()));
        }
        self.cx = Context::new(None);
        self.egui_winit = None;
        self.window = None;
    }
}

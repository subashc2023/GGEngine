//! Windowed runs: the winit event loop, which renders a frame whenever the window can show one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gg_core::{Error, Result};
use gg_renderer::Renderer;
use tracing::{debug, info};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use crate::args::Args;
use crate::{App, Ran, clear_color, settings};

pub(crate) fn run(app: &App, args: &Args) -> Result<Ran> {
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
        start: None,
        frames: 0,
        paused: false,
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
struct Handler<'a> {
    app: &'a App,
    args: &'a Args,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    /// When the first frame started.
    start: Option<Instant>,
    frames: u64,
    /// Set while the window has no area to render into, as when it's minimized.
    paused: bool,
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
        self.window = Some(window);
        Ok(())
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(window), Some(renderer)) = (&self.window, &mut self.renderer) else {
            return;
        };
        let start = *self.start.get_or_insert_with(Instant::now);
        match renderer.render(clear_color(start.elapsed())) {
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
                if (size.width == 0 || size.height == 0) && !self.paused {
                    debug!("Window minimized; rendering pauses until it's restored");
                    self.paused = true;
                }
            }
            Err(error) => self.fail(event_loop, error),
        }
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
        match event {
            WindowEvent::CloseRequested => {
                debug!("Window closed");
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size.width, size.height);
                }
                if self.paused && size.width > 0 && size.height > 0 {
                    debug!(
                        width = size.width,
                        height = size.height,
                        "Window restored; rendering resumes"
                    );
                    self.paused = false;
                }
                // Windows doesn't call `about_to_wait` while the user drags the window's edge,
                // so frames are requested here as well.
                if !self.paused
                    && let Some(window) = &self.window
                {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.frame(event_loop),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        // Frames render back to back; FIFO presentation paces them to the display.
        if !self.paused
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        let time = self.start.map_or(Duration::ZERO, |start| start.elapsed());
        if let Some(renderer) = self.renderer.take() {
            self.ran = Some(Ran::finish(renderer, self.frames, time));
        }
        self.window = None;
    }
}

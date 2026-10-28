//! The render thread, `gg-render`, which renders each frame while the main thread runs the next.
//! The renderer goes there and back: the main thread hands it over with a frame's render data once
//! the frame's UI is drawn and tessellated, and takes it back once the next frame's is, waiting for
//! it if need be, so that the main thread is at most a frame ahead. While it's back, the main
//! thread tells it the windows' sizes, opens and closes windows' swapchains, and at the end drops
//! it: Windows and macOS give out a window's handle only on its own thread, and macOS closes a
//! window there, so the render thread never makes a surface or holds the last reference to a
//! window. It rebuilds a swapchain whose window changed size as it acquires the swapchain's next
//! image. One thread at a time calls the renderer.

use std::any::Any;
use std::mem;
use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use gg_core::{Error, Result};
use gg_renderer::{DrawCounts, Draws, RenderData, Renderer, Uploads, Viewpoint, Viewport};

use crate::viewports::Drawn;
#[cfg(target_os = "windows")]
use crate::windows_wait::recv as wait;

/// A frame's render data, which the main thread fills in and the render thread renders: plain
/// data. Its draws and uploads keep their memory from frame to frame; its UI is egui's, made anew
/// each frame.
pub(crate) struct Extracted {
    pub(crate) clear_color: [f32; 4],
    pub(crate) viewport: Viewport,
    pub(crate) viewpoint: Viewpoint,
    pub(crate) draws: Draws,
    pub(crate) uploads: Uploads,
    /// The main window's UI, tessellated, and its texture changes.
    pub(crate) primitives: Vec<egui::ClippedPrimitive>,
    pub(crate) textures: TextureChanges,
    pub(crate) pixels_per_point: f32,
    /// What the windows of their own drew, in the order their passes ran.
    pub(crate) windows: Vec<Drawn>,
}

impl Default for Extracted {
    fn default() -> Self {
        Self {
            clear_color: [0.0; 4],
            viewport: Viewport {
                origin: [0, 0],
                size: [0, 0],
            },
            viewpoint: Viewpoint::default(),
            draws: Draws::default(),
            uploads: Uploads::default(),
            primitives: Vec::new(),
            textures: TextureChanges::default(),
            pixels_per_point: 1.0,
            windows: Vec::new(),
        }
    }
}

/// Changes to egui's textures, for the renderer to apply. egui asserts, in debug builds, that none
/// it hands out are dropped unapplied; the renderer applies those of every frame handed to it, so
/// only a frame that isn't, as the run ends on an error, a panic or its last frame, drops any,
/// which this clears as it drops.
#[derive(Default)]
pub(crate) struct TextureChanges(pub(crate) egui::TexturesDelta);

impl Drop for TextureChanges {
    fn drop(&mut self) {
        self.0.clear();
    }
}

/// How rendering a frame went.
#[derive(Debug)]
pub(crate) struct Rendered {
    /// Whether it presented, as [`Renderer::render`] says.
    pub(crate) presented: Result<bool>,
    /// The time it took, its waits among it.
    pub(crate) render: Duration,
    /// Its waits, as [`Renderer::waited`] says.
    pub(crate) wait: Duration,
    pub(crate) drawn: DrawCounts,
}

/// What renders frames on the render thread: the renderer, or a test's stand-in for it.
pub(crate) trait Render: Send + 'static {
    /// Renders `frame`, taking from it what it applies, such as its uploads and texture changes.
    fn render(&mut self, frame: &mut Extracted) -> Rendered;
}

impl Render for Renderer {
    fn render(&mut self, frame: &mut Extracted) -> Rendered {
        let start = Instant::now();
        let data = RenderData {
            clear_color: frame.clear_color,
            viewport: frame.viewport,
            viewpoint: frame.viewpoint,
            uploads: &mut frame.uploads,
            draws: &frame.draws,
        };
        let ui = gg_renderer::Ui {
            primitives: &frame.primitives,
            textures: &mut frame.textures.0,
            pixels_per_point: frame.pixels_per_point,
        };
        let mut windows: Vec<_> = frame.windows.iter_mut().map(Drawn::ui).collect();
        let presented = Renderer::render(self, data, ui, &mut windows);
        let drawn = self.drawn();
        gg_core::plot!("draws", drawn.draws as f64);
        gg_core::plot!("instances", drawn.instances as f64);
        gg_core::plot!("triangles", drawn.triangles as f64);
        Rendered {
            presented,
            render: start.elapsed(),
            wait: self.waited(),
            drawn,
        }
    }
}

/// The render thread, and the renderer while it's back from it.
pub(crate) struct RenderThread<R: Render = Renderer> {
    /// The renderer and the frame it last rendered, while they're back.
    home: Option<Box<Trip<R>>>,
    /// To the render thread, until it's to end.
    to: Option<SyncSender<Box<Trip<R>>>>,
    from: Receiver<Box<Trip<R>>>,
    thread: Option<JoinHandle<()>>,
}

/// What goes to the render thread and back each frame: the only one, made as the thread starts.
struct Trip<R> {
    renderer: R,
    frame: Extracted,
    /// How rendering the frame went, or the panic it ended in, as it comes back.
    rendered: Option<thread::Result<Rendered>>,
}

impl<R: Render> RenderThread<R> {
    /// Starts the render thread, which `renderer` renders on. Fails with `engine.render_thread` if
    /// the thread can't start.
    pub(crate) fn start(renderer: R) -> Result<Self> {
        // One trip, so that neither thread waits to send it.
        let (to, handed) = mpsc::sync_channel(1);
        let (back, from) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            // Within the 15 bytes Linux keeps of a thread's name.
            .name("gg-render".to_owned())
            .spawn(move || serve(&handed, &back))
            .map_err(|error| {
                Error::new("engine.render_thread", "can't start the render thread")
                    .with_source(error)
            })?;
        Ok(Self {
            home: Some(Box::new(Trip {
                renderer,
                frame: Extracted::default(),
                rendered: None,
            })),
            to: Some(to),
            from,
            thread: Some(thread),
        })
    }

    /// The renderer, while it's back.
    ///
    /// # Panics
    ///
    /// If it's away, rendering a frame.
    pub(crate) fn renderer(&mut self) -> &mut R {
        let trip = (self.home.as_mut())
            .expect("the renderer is back between taking it and handing it over");
        &mut trip.renderer
    }

    /// Hands the renderer over, to render the frame `fill` fills in from what the frame handed
    /// before left, while the main thread runs the next.
    ///
    /// # Panics
    ///
    /// If it's away already; or if the render thread ended in a panic, which this resumes.
    pub(crate) fn hand(&mut self, fill: impl FnOnce(&mut Extracted)) {
        let mut trip = (self.home.take()).expect("the renderer is handed over once a frame");
        fill(&mut trip.frame);
        let to = (self.to.as_ref()).expect("the render thread runs until it stops");
        if let Err(mpsc::SendError(trip)) = to.send(trip) {
            self.home = Some(trip);
            let panic = self.died();
            panic::resume_unwind(panic);
        }
    }

    /// Takes the renderer back, once it's rendered the frame handed to it, waiting for it if need
    /// be, and returns how that went; `None` if it was back already. A panic while rendering
    /// resumes here, with the renderer back.
    pub(crate) fn take(&mut self) -> Option<Rendered> {
        match self.back()? {
            Ok(rendered) => Some(rendered),
            Err(panic) => panic::resume_unwind(panic),
        }
    }

    /// Takes the renderer back for good, once it's rendered the frame handed to it, which it says
    /// how went of, and ends the thread. A panic while rendering resumes here.
    pub(crate) fn stop(mut self) -> (R, Option<Rendered>) {
        let rendered = self.take();
        self.end();
        let trip = (self.home.take()).expect("the renderer is back");
        (trip.renderer, rendered)
    }

    /// Waits for the renderer to come back, if it's away, and returns how its frame went.
    fn back(&mut self) -> Option<thread::Result<Rendered>> {
        if self.home.is_some() {
            return None;
        }
        let _zone = gg_core::zone!("wait for the render thread");
        let Ok(mut trip) = wait(&self.from) else {
            return Some(Err(self.died()));
        };
        let rendered = trip.rendered.take();
        self.home = Some(trip);
        rendered
    }

    /// The panic the render thread ended in outside a frame, without handing the renderer back.
    fn died(&mut self) -> Box<dyn Any + Send> {
        match self.thread.take().map(JoinHandle::join) {
            Some(Err(panic)) => panic,
            _ => Box::new("the render thread ended without handing the renderer back"),
        }
    }

    /// Ends the render thread, with the renderer back: its loop ends once nothing more can be
    /// handed to it.
    fn end(&mut self) {
        self.to = None;
        if let Some(thread) = self.thread.take() {
            // It panics only in frames, whose panics come back with the renderer.
            let _ = thread.join();
        }
    }
}

impl<R: Render> Drop for RenderThread<R> {
    /// Takes the renderer back and ends the thread, as [`RenderThread::stop`] does, for a run that
    /// ends on an error or a panic: so that the renderer drops here, on the main thread, before the
    /// windows it presents to. How its frame went no longer counts.
    fn drop(&mut self) {
        let _ = self.back();
        self.end();
    }
}

/// The render thread: renders each frame handed to it and hands it back, until nothing more can be
/// handed to it.
fn serve<R: Render>(handed: &Receiver<Box<Trip<R>>>, back: &SyncSender<Box<Trip<R>>>) {
    while let Ok(mut trip) = handed.recv() {
        let Trip {
            renderer,
            frame,
            rendered,
        } = &mut *trip;
        // Unwinding drops nothing of the trip, which goes back as ever: the panic resumes on the
        // main thread, which drops the renderer.
        *rendered = Some(panic::catch_unwind(AssertUnwindSafe(|| {
            renderer.render(frame)
        })));
        if let Err(mpsc::SendError(trip)) = back.send(trip) {
            // Only if the main thread let go without taking the renderer back, which it never
            // does; even so, it isn't dropped here, where a window's last reference mustn't be.
            mem::forget(trip);
            return;
        }
    }
}

/// Waits for what `from` receives.
#[cfg(not(target_os = "windows"))]
fn wait<T>(from: &Receiver<T>) -> Result<T, mpsc::RecvError> {
    from.recv()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::thread::ThreadId;

    use super::*;

    /// The longest the stand-in waits to be told to go on, and a test waits for it to start, past
    /// which the test fails rather than hangs.
    const DEADLINE: Duration = Duration::from_secs(10);

    /// A stand-in for the renderer, which renders nothing. A frame tags itself with its clear
    /// color's red, which `Rendered::drawn.draws` gives back, beside the stand-in's `setting` in
    /// `instances`; frame 2 fails and frame 3 panics, if `failing`.
    #[derive(Default)]
    struct Fake {
        setting: usize,
        failing: bool,
        /// Told each frame's tag as it starts rendering, which then waits to be told to go on.
        started: Option<(SyncSender<usize>, Receiver<()>)>,
        /// The frames it rendered, and the thread it dropped on.
        rendered: Arc<Mutex<Vec<usize>>>,
        dropped: Probe,
        /// Leaves the frames' texture changes as they were, which the renderer always applies.
        leaves_textures: bool,
    }

    impl Render for Fake {
        fn render(&mut self, frame: &mut Extracted) -> Rendered {
            let tag = frame.clear_color[0] as usize;
            if let Some((started, go)) = &self.started {
                started.send(tag).unwrap();
                go.recv_timeout(DEADLINE).expect("told to go on");
            }
            self.rendered.lock().unwrap().push(tag);
            if !self.leaves_textures {
                frame.textures.0.clear();
            }
            let presented = match tag {
                2 if self.failing => Err(Error::new("test.failed", "frame 2 fails")),
                3 if self.failing => panic!("frame 3 panics"),
                _ => Ok(true),
            };
            Rendered {
                presented,
                render: Duration::ZERO,
                wait: Duration::ZERO,
                drawn: DrawCounts {
                    draws: tag,
                    instances: self.setting,
                    triangles: 0,
                },
            }
        }
    }

    /// Notes the thread it drops on.
    #[derive(Default)]
    struct Probe(Arc<Mutex<Option<ThreadId>>>);

    impl Drop for Probe {
        fn drop(&mut self) {
            *self.0.lock().unwrap() = Some(thread::current().id());
        }
    }

    /// Hands frame `tag` over.
    fn hand(render: &mut RenderThread<Fake>, tag: usize) {
        render.hand(|frame| frame.clear_color[0] = tag as f32);
    }

    #[test]
    fn frames_render_in_order_a_frame_behind() {
        let mut render = RenderThread::start(Fake::default()).unwrap();
        assert!(render.take().is_none(), "nothing was handed over yet");
        for tag in 0..6 {
            if tag > 0 {
                let rendered = render.take().expect("the frame before");
                assert_eq!(rendered.drawn.draws, tag - 1);
            }
            hand(&mut render, tag);
        }
        let (fake, last) = render.stop();
        assert_eq!(last.expect("the last frame").drawn.draws, 5);
        assert_eq!(*fake.rendered.lock().unwrap(), [0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn a_frame_renders_while_the_next_runs() {
        let (started, starts) = mpsc::sync_channel(1);
        let (go, goes) = mpsc::sync_channel(1);
        let fake = Fake {
            started: Some((started, goes)),
            ..Fake::default()
        };
        let mut render = RenderThread::start(fake).unwrap();
        hand(&mut render, 0);
        // Only a thread of its own renders the frame while this one is between handing it over
        // and taking it back; one that rendered it in either would never see `go`.
        assert_eq!(
            starts.recv_timeout(DEADLINE),
            Ok(0),
            "rendering never started"
        );
        go.send(()).unwrap();
        assert_eq!(render.take().expect("frame 0").drawn.draws, 0);
    }

    #[test]
    fn the_renderer_comes_home_between_frames() {
        let mut render = RenderThread::start(Fake::default()).unwrap();
        hand(&mut render, 0);
        assert_eq!(render.take().unwrap().drawn.instances, 0);
        // As a window opens while it's back.
        render.renderer().setting = 1;
        hand(&mut render, 1);
        assert_eq!(render.take().unwrap().drawn.instances, 1);
        let dropped = render.renderer().dropped.0.clone();
        drop(render);
        assert_eq!(*dropped.lock().unwrap(), Some(thread::current().id()));
    }

    #[test]
    fn errors_come_back_and_panics_resume_here() {
        let fake = Fake {
            failing: true,
            ..Fake::default()
        };
        let mut render = RenderThread::start(fake).unwrap();
        hand(&mut render, 2);
        let error = render.take().unwrap().presented.unwrap_err();
        assert_eq!(error.code(), "test.failed");
        hand(&mut render, 3);
        let panic = panic::catch_unwind(AssertUnwindSafe(|| render.take()))
            .expect_err("frame 3's panic resumes as the renderer comes back");
        assert_eq!(panic.downcast_ref::<&str>(), Some(&"frame 3 panics"));
        // Back, and dropped on this thread.
        let dropped = render.renderer().dropped.0.clone();
        drop(render);
        assert_eq!(*dropped.lock().unwrap(), Some(thread::current().id()));
    }

    #[test]
    fn stopping_and_dropping_render_the_frame_handed_over_and_end_the_thread() {
        let mut render = RenderThread::start(Fake::default()).unwrap();
        hand(&mut render, 0);
        let (_, last) = render.stop();
        assert_eq!(last.expect("frame 0").drawn.draws, 0);

        let fake = Fake::default();
        let (rendered, dropped) = (fake.rendered.clone(), fake.dropped.0.clone());
        let mut render = RenderThread::start(fake).unwrap();
        hand(&mut render, 0);
        drop(render);
        assert_eq!(*rendered.lock().unwrap(), [0]);
        assert_eq!(*dropped.lock().unwrap(), Some(thread::current().id()));
    }

    /// A texture change, as egui hands one out.
    fn change() -> TextureChanges {
        let mut changes = TextureChanges::default();
        changes.0.free.insert(egui::TextureId::Managed(1));
        changes
    }

    #[test]
    fn no_texture_change_is_dropped_unapplied() {
        let fake = || Fake {
            leaves_textures: true,
            failing: true,
            ..Fake::default()
        };
        let changed = |render: &mut RenderThread<Fake>, tag| {
            render.hand(|frame| {
                frame.clear_color[0] = tag as f32;
                frame.textures = change();
            });
        };
        // egui's assert fails a debug build's test as the changes drop, on each way a run ends.
        let mut render = RenderThread::start(fake()).unwrap();
        changed(&mut render, 0);
        drop(render.stop());
        let mut render = RenderThread::start(fake()).unwrap();
        changed(&mut render, 0);
        drop(render);
        let mut render = RenderThread::start(fake()).unwrap();
        changed(&mut render, 2);
        assert!(render.take().unwrap().presented.is_err());
        drop(render);
        let mut render = RenderThread::start(fake()).unwrap();
        changed(&mut render, 3);
        let panicked = panic::catch_unwind(AssertUnwindSafe(|| render.take()));
        assert!(panicked.is_err());
        drop(render);
        // A frame that isn't handed over.
        drop(change());
    }
}

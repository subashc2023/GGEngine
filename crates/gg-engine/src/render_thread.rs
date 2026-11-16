//! The render thread, `gg-render`, renders each frame while the main thread runs the next, as
//! ARCHITECTURE.md's Threads describes. The main thread hands the renderer over with a frame's
//! render data once the frame's UI is drawn and tessellated, and takes it back once the next
//! frame's is, waiting if need be. While it's back, the main thread tells it the windows' sizes,
//! opens and closes their swapchains, and at the end drops it. The render thread rebuilds a
//! swapchain whose window changed size as it acquires the next image. One thread at a time calls
//! the renderer.

use std::any::Any;
use std::mem;
use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use gg_core::math::{DVec3, Vec4};
use gg_core::{Error, Result};
use gg_renderer::{
    Antialiasing, DrawCounts, Draws, MemoryBudget, Outline, Pick, Pixels, RenderData, Renderer,
    Streaming, Tonemapper, Uploads, Viewpoint, Viewport,
};

#[cfg(feature = "dev")]
pub(crate) use crate::viewports::Drawn;
#[cfg(target_os = "windows")]
use crate::windows_wait::recv as wait;

/// What a window of its own drew. A build without `dev` has no UI, so no torn-off panels' windows.
#[cfg(not(feature = "dev"))]
pub(crate) enum Drawn {}

#[cfg(not(feature = "dev"))]
impl Drawn {
    pub(crate) fn ui(&mut self) -> (u64, gg_renderer::Ui<'_>) {
        match *self {}
    }
}

/// A frame's render data: plain data the main thread fills in and the render thread renders. Its
/// draws, uploads and picks keep their memory from frame to frame, as do the answers to picks it
/// brings back. Its UI is egui's, made anew each frame, and empty without `dev`.
pub(crate) struct Extracted {
    pub(crate) clear_color: [f32; 4],
    pub(crate) tonemapper: Tonemapper,
    pub(crate) antialiasing: Antialiasing,
    /// See [`RenderData::exposure`].
    pub(crate) exposure: f32,
    pub(crate) adaptation: Option<gg_renderer::Adaptation>,
    /// See [`RenderData::looked`]; never without `dev`.
    pub(crate) looked: bool,
    /// Keeps its memory.
    pub(crate) lights: Vec<gg_renderer::Light>,
    /// This field and those after it, through `lens`, come from the world's environment.
    pub(crate) sky: Option<gg_renderer::Sky>,
    pub(crate) bloom: f32,
    pub(crate) grade: gg_renderer::Grade,
    pub(crate) ambient_occlusion: Option<gg_renderer::AmbientOcclusion>,
    pub(crate) fog: Option<gg_renderer::Fog>,
    pub(crate) reflections: Option<gg_renderer::Reflections>,
    pub(crate) atmosphere: Option<gg_renderer::Atmosphere>,
    pub(crate) depth_of_field: Option<gg_renderer::DepthOfField>,
    pub(crate) motion_blur: Option<gg_renderer::MotionBlur>,
    pub(crate) lens: gg_renderer::Lens,
    /// The simulation's time in seconds as drawn.
    pub(crate) time: f32,
    pub(crate) viewport: Viewport,
    pub(crate) viewpoint: Viewpoint,
    /// See [`RenderData::origin`].
    pub(crate) origin: DVec3,
    /// See [`RenderData::cut`].
    pub(crate) cut: bool,
    pub(crate) draws: Draws,
    /// What the layers drew on the canvas, in the view's pixels.
    pub(crate) canvas: Draws,
    pub(crate) uploads: Uploads,
    /// Each pick's tag is the number of the context's ticket.
    pub(crate) picks: Vec<Pick>,
    /// The outline's color and width, for the draws of the IDs in `outline_ids` (sorted; the
    /// outlined entity and its descendants). Kept from frame to frame, so frames don't allocate.
    pub(crate) outline: Option<(Vec4, f32)>,
    pub(crate) outline_ids: Vec<u64>,
    /// Answers to picks that came in as the frame rendered ([`Renderer::picked`]).
    pub(crate) picked: Vec<(u64, u64)>,
    /// The main window's tessellated UI, and its texture changes.
    pub(crate) primitives: Vec<egui::ClippedPrimitive>,
    pub(crate) textures: TextureChanges,
    pub(crate) pixels_per_point: f32,
    /// What the windows of their own drew, in the order their passes ran.
    pub(crate) windows: Vec<Drawn>,
    /// The pixels the view shows in place of what's drawn
    /// ([`Context::show_pixels`](crate::Context::show_pixels)): size, bytes (which keep their
    /// memory from frame to frame), and whether they're new since a frame last rendered them.
    pub(crate) pixels: Option<[u32; 2]>,
    pub(crate) pixel_bytes: Vec<u8>,
    pub(crate) pixels_new: bool,
    /// Textures whose images were being decoded at handoff; the frame shows their stand-ins.
    pub(crate) loading: u32,
    /// What the renderer was still streaming to the GPU after the frame ([`Renderer::streaming`]);
    /// empty once everything added before is drawable.
    pub(crate) streaming: Streaming,
    /// Whether the frame reads the game's view back, for the tool API's captures.
    #[cfg(feature = "dev")]
    pub(crate) capture: bool,
    /// The view it read back, if asked and rendered.
    #[cfg(feature = "dev")]
    pub(crate) captured: Option<Result<Shot>>,
    /// The client the view is published to, as the tool API's `view` asked.
    #[cfg(feature = "dev")]
    pub(crate) publish: Option<Publish>,
    /// The revision of the tool API's look the frame drew with.
    #[cfg(feature = "dev")]
    pub(crate) look: u64,
    /// Whether the frame is drawn without the look, for a capture of the game's own view. It reads
    /// no picks, leaving them for the next frame that isn't bare.
    #[cfg(feature = "dev")]
    pub(crate) bare: bool,
    /// What the view shows in place of its light, if the look or a capture asks.
    #[cfg(feature = "dev")]
    pub(crate) show: Option<crate::api::Shown>,
    /// Whether the frame lists the images a show finds, for the tool API's `images`.
    #[cfg(feature = "dev")]
    pub(crate) list_images: bool,
    /// Those listed ([`Renderer::images`]).
    #[cfg(feature = "dev")]
    pub(crate) images: Option<Vec<gg_renderer::FrameImage>>,
}

/// The client a game's view is published to, as the tool API's `view` asked.
///
/// - `writer`: the memory it made. Each frame that renders the view writes it once the GPU has
///   finished, except one with `skip` (a capture of the game's own view), which leaves it for the
///   next.
/// - `ticks` run in the frame handed over, and when it `started`: set by the main thread.
/// - `failed`: why a frame wasn't published, until the main thread takes it.
#[cfg(feature = "dev")]
pub(crate) struct Publish {
    pub(crate) writer: gg_api::view::Writer,
    pub(crate) skip: bool,
    pub(crate) ticks: u64,
    pub(crate) started: std::time::SystemTime,
    pub(crate) failed: Option<Error>,
}

/// The game's view as a frame rendered it, read back for the tool API's captures: rows of RGBA8
/// pixels, sRGB-encoded, top row first.
#[cfg(feature = "dev")]
pub(crate) struct Shot {
    pub(crate) size: [u32; 2],
    pub(crate) pixels: Vec<u8>,
}

impl Default for Extracted {
    fn default() -> Self {
        Self {
            clear_color: [0.0; 4],
            tonemapper: Tonemapper::None,
            antialiasing: Antialiasing::None,
            exposure: 1.0,
            adaptation: None,
            looked: false,
            lights: Vec::new(),
            sky: None,
            bloom: 0.0,
            grade: gg_renderer::Grade::IDENTITY,
            ambient_occlusion: None,
            fog: None,
            reflections: None,
            atmosphere: None,
            depth_of_field: None,
            motion_blur: None,
            lens: gg_renderer::Lens::NONE,
            time: 0.0,
            viewport: Viewport {
                origin: [0, 0],
                size: [0, 0],
            },
            viewpoint: Viewpoint::default(),
            origin: DVec3::ZERO,
            cut: true,
            draws: Draws::default(),
            canvas: Draws::default(),
            uploads: Uploads::default(),
            picks: Vec::new(),
            outline: None,
            outline_ids: Vec::new(),
            picked: Vec::new(),
            primitives: Vec::new(),
            textures: TextureChanges::default(),
            pixels_per_point: 1.0,
            windows: Vec::new(),
            pixels: None,
            pixel_bytes: Vec::new(),
            pixels_new: false,
            loading: 0,
            streaming: Streaming::default(),
            #[cfg(feature = "dev")]
            capture: false,
            #[cfg(feature = "dev")]
            captured: None,
            #[cfg(feature = "dev")]
            publish: None,
            #[cfg(feature = "dev")]
            look: 0,
            #[cfg(feature = "dev")]
            bare: false,
            #[cfg(feature = "dev")]
            show: None,
            #[cfg(feature = "dev")]
            list_images: false,
            #[cfg(feature = "dev")]
            images: None,
        }
    }
}

/// Changes to egui's textures, for the renderer to apply. egui asserts in debug builds that none
/// are dropped unapplied. The renderer applies every frame's it's handed, so only an unrendered
/// frame (a run ending on an error, a panic or its last frame) drops any; this clears them first.
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
    /// Whether it presented ([`Renderer::render`]).
    pub(crate) presented: Result<bool>,
    /// Time taken, waits included.
    pub(crate) render: Duration,
    /// Its waits ([`Renderer::waited`]) plus the GPU wait for a view read back.
    pub(crate) wait: Duration,
    /// Of the rest, copying the view into the published client's memory.
    pub(crate) publish: Duration,
    pub(crate) drawn: DrawCounts,
    /// GPU time of the frame two before ([`Renderer::gpu_time`]).
    pub(crate) gpu: Option<Duration>,
}

/// What renders frames on the render thread: the renderer, or a test's stand-in.
pub(crate) trait Render: Send + 'static {
    /// Renders `frame`, taking what it applies (uploads, texture changes) and replacing its
    /// answers to picks.
    fn render(&mut self, frame: &mut Extracted) -> Rendered;

    /// The GPU's memory in use and its budget ([`Renderer::memory_budget`]); `None` for a test's
    /// stand-in.
    fn memory_budget(&self) -> Option<MemoryBudget> {
        None
    }
}

impl Render for Renderer {
    fn render(&mut self, frame: &mut Extracted) -> Rendered {
        let start = Instant::now();
        #[cfg(feature = "dev")]
        let read_view = frame.capture || frame.publish.as_ref().is_some_and(|p| !p.skip);
        #[cfg(not(feature = "dev"))]
        let read_view = false;
        #[cfg(feature = "dev")]
        let read_picks = !frame.bare;
        #[cfg(not(feature = "dev"))]
        let read_picks = true;
        #[cfg(feature = "dev")]
        let show = frame.show.as_ref().map(crate::api::Shown::show);
        #[cfg(not(feature = "dev"))]
        let show = None;
        #[cfg(feature = "dev")]
        let list_images = frame.list_images;
        #[cfg(not(feature = "dev"))]
        let list_images = false;
        let pixels = frame.pixels.map(|size| Pixels {
            size,
            bytes: frame.pixels_new.then_some(&frame.pixel_bytes[..]),
        });
        let data = RenderData {
            clear_color: frame.clear_color,
            tonemapper: frame.tonemapper,
            antialiasing: frame.antialiasing,
            exposure: frame.exposure,
            adaptation: frame.adaptation,
            looked: frame.looked,
            time: frame.time,
            lights: &frame.lights,
            sky: frame.sky,
            bloom: frame.bloom,
            grade: frame.grade,
            ambient_occlusion: frame.ambient_occlusion,
            fog: frame.fog,
            reflections: frame.reflections,
            atmosphere: frame.atmosphere,
            depth_of_field: frame.depth_of_field,
            motion_blur: frame.motion_blur,
            lens: frame.lens,
            viewport: frame.viewport,
            viewpoint: frame.viewpoint,
            origin: frame.origin,
            cut: frame.cut,
            uploads: &mut frame.uploads,
            draws: &frame.draws,
            canvas: &frame.canvas,
            picks: &frame.picks,
            read_picks,
            outline: frame.outline.map(|(color, width)| Outline {
                ids: &frame.outline_ids,
                color,
                width,
            }),
            read_view,
            list_images,
            pixels,
            show,
        };
        let ui = gg_renderer::Ui {
            primitives: &frame.primitives,
            textures: &mut frame.textures.0,
            pixels_per_point: frame.pixels_per_point,
        };
        let mut windows: Vec<_> = frame.windows.iter_mut().map(Drawn::ui).collect();
        let presented = Renderer::render(self, data, ui, &mut windows);
        let rendered = matches!(presented, Ok(true));
        // Until a frame renders them, into a view with an area, the pixels stay new.
        let [width, height] = frame.viewport.size;
        frame.pixels_new &= !(rendered && width > 0 && height > 0);
        frame.picked.clear();
        frame.picked.extend_from_slice(self.picked());
        frame.streaming = self.streaming();
        #[cfg(feature = "dev")]
        if list_images && rendered {
            frame.images = Some(self.images().to_vec());
        }
        #[cfg(feature = "dev")]
        let (waited, publish) = if read_view && rendered {
            frame.take_view(self)
        } else {
            (Duration::ZERO, Duration::ZERO)
        };
        #[cfg(not(feature = "dev"))]
        let (waited, publish) = (Duration::ZERO, Duration::ZERO);
        let drawn = self.drawn();
        gg_core::plot!("draws", drawn.draws as f64);
        gg_core::plot!("instances", drawn.instances as f64);
        gg_core::plot!("triangles", drawn.triangles as f64);
        Rendered {
            presented,
            render: start.elapsed(),
            wait: self.waited() + waited,
            publish,
            drawn,
            gpu: self.gpu_time(),
        }
    }

    fn memory_budget(&self) -> Option<MemoryBudget> {
        Some(Renderer::memory_budget(self))
    }
}

#[cfg(feature = "dev")]
impl Extracted {
    /// Once the frame has rendered and read the view back, waits for the GPU and takes the view:
    /// a copy (or the error) for captures; unless skipped, the client's memory, with the viewpoint
    /// and look drawn with (or why it couldn't be published). Returns the GPU wait and the copy
    /// into the client's memory.
    fn take_view(&mut self, renderer: &Renderer) -> (Duration, Duration) {
        let start = Instant::now();
        let read = renderer.view_read();
        let waited = start.elapsed();
        let read = match read {
            Ok(Some(read)) => read,
            Ok(None) => {
                if self.capture {
                    let message = "the game's view has no area, as while another panel's tab shows \
                                   over it, or shows pixels from elsewhere, as the editor shows a \
                                   game's";
                    self.captured = Some(Err(Error::new("api.no_view", message)));
                }
                return (waited, Duration::ZERO);
            }
            Err(error) => {
                if let Some(publish) = self.publish.as_mut().filter(|publish| !publish.skip) {
                    publish.failed = Some(Error::new("api.view", format!("{error:#}")));
                }
                if self.capture {
                    self.captured = Some(Err(error));
                }
                return (waited, Duration::ZERO);
            }
        };
        let size = read.size();
        if self.capture {
            let mut pixels = vec![0; size[0] as usize * size[1] as usize * 4];
            self.captured = Some(read.read_into(&mut pixels).map(|()| Shot { size, pixels }));
        }
        let viewpoint = crate::api::api_viewpoint(&self.viewpoint);
        let Some(publish) = self.publish.as_mut().filter(|publish| !publish.skip) else {
            return (waited, Duration::ZERO);
        };
        let copying = Instant::now();
        let mut copied = Ok(());
        let (ticks, started) = (publish.ticks, publish.started);
        let fill = |pixels: &mut [u8]| copied = read.read_into(pixels);
        let published = (publish.writer).publish(size, ticks, started, viewpoint, self.look, fill);
        let failed = match (published, copied) {
            (Err(error), _) => Some(Error::new("api.view", error.to_string())),
            (Ok(_), Err(error)) => Some(error),
            (Ok(_), Ok(())) => None,
        };
        if failed.is_some() {
            publish.failed = failed;
        }
        (waited, copying.elapsed())
    }
}

/// The render thread, and the renderer while it's back from it.
pub(crate) struct RenderThread<R: Render = Renderer> {
    /// The renderer and its last frame, while back.
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
    /// How rendering went, or the panic it ended in, as it comes back.
    rendered: Option<thread::Result<Rendered>>,
}

impl<R: Render> RenderThread<R> {
    /// Starts the render thread, which `renderer` renders on. Fails with `engine.render_thread` if
    /// the thread can't start.
    pub(crate) fn start(renderer: R) -> Result<Self> {
        // One trip, so neither thread waits to send.
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

    /// The frame last rendered, with what it brought back, while it's back.
    ///
    /// # Panics
    ///
    /// If it's away, rendering a frame.
    pub(crate) fn frame(&self) -> &Extracted {
        let trip = (self.home.as_ref()).expect("the frame is back between frames");
        &trip.frame
    }

    /// The frame last rendered, to take what it brought back and set what the next handover is to
    /// do (it fills in the same one), while it's back.
    ///
    /// # Panics
    ///
    /// If it's away, rendering a frame.
    #[cfg(feature = "dev")]
    pub(crate) fn frame_mut(&mut self) -> &mut Extracted {
        let trip = (self.home.as_mut()).expect("the frame is back between frames");
        &mut trip.frame
    }

    /// Hands the renderer over to render the frame `fill` fills in (from what the previous frame
    /// left), while the main thread runs the next.
    ///
    /// # Panics
    ///
    /// - If it's away already.
    /// - If the render thread ended in a panic: the panic is resumed.
    pub(crate) fn hand(&mut self, fill: impl FnOnce(&mut Extracted)) {
        let home = (self.home.as_mut()).expect("the renderer is handed over once a frame");
        // Filled in while it's back, so a panic filling it leaves it back, to drop as ever, not
        // dropped with the frame while dropping this waits for it to come back.
        fill(&mut home.frame);
        let trip = (self.home.take()).expect("filling the frame leaves the renderer back");
        let to = (self.to.as_ref()).expect("the render thread runs until it stops");
        if let Err(mpsc::SendError(trip)) = to.send(trip) {
            self.home = Some(trip);
            let panic = self.died();
            panic::resume_unwind(panic);
        }
    }

    /// Takes the renderer back once it's rendered the frame, waiting if need be, and returns how
    /// that went; `None` if it was back already. A panic while rendering resumes here, with the
    /// renderer back.
    pub(crate) fn take(&mut self) -> Option<Rendered> {
        match self.back()? {
            Ok(rendered) => Some(rendered),
            Err(panic) => panic::resume_unwind(panic),
        }
    }

    /// Like [`take`](Self::take), then ends the thread for good.
    pub(crate) fn stop(mut self) -> (R, Option<Rendered>) {
        let rendered = self.take();
        self.end();
        let trip = (self.home.take()).expect("the renderer is back");
        (trip.renderer, rendered)
    }

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

    /// The panic the thread ended in outside a frame, without handing the renderer back.
    fn died(&mut self) -> Box<dyn Any + Send> {
        match self.thread.take().map(JoinHandle::join) {
            Some(Err(panic)) => panic,
            _ => Box::new("the render thread ended without handing the renderer back"),
        }
    }

    /// Ends the thread, with the renderer back: its loop ends once nothing more can be sent.
    fn end(&mut self) {
        self.to = None;
        if let Some(thread) = self.thread.take() {
            // It panics only in frames, whose panics come back with the renderer.
            let _ = thread.join();
        }
    }
}

impl<R: Render> Drop for RenderThread<R> {
    /// Like [`RenderThread::stop`], for a run that ends on an error or a panic: the renderer drops
    /// here, on the main thread, before the windows it presents to. How its frame went is ignored.
    fn drop(&mut self) {
        let _ = self.back();
        self.end();
    }
}

/// The render thread: renders each frame handed to it and hands it back, until nothing more can be
/// sent.
fn serve<R: Render>(handed: &Receiver<Box<Trip<R>>>, back: &SyncSender<Box<Trip<R>>>) {
    while let Ok(mut trip) = handed.recv() {
        let Trip {
            renderer,
            frame,
            rendered,
        } = &mut *trip;
        // Unwinding drops nothing of the trip, which goes back as ever; the panic resumes on the
        // main thread, which drops the renderer.
        *rendered = Some(panic::catch_unwind(AssertUnwindSafe(|| {
            renderer.render(frame)
        })));
        if let Err(mpsc::SendError(trip)) = back.send(trip) {
            // Only if the main thread let go without taking the renderer back, which it never
            // does. Even so, not dropped here: a window's last reference mustn't be.
            mem::forget(trip);
            return;
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn wait<T>(from: &Receiver<T>) -> Result<T, mpsc::RecvError> {
    from.recv()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::thread::ThreadId;

    use gg_renderer::MAX_PICKS;

    use super::*;
    use crate::main_loop::{MainUi, hand_off, take_back};
    use crate::{Context, EntityId};

    /// How long the stand-in waits to go on, and a test for it to start; then the test fails
    /// rather than hangs.
    const DEADLINE: Duration = Duration::from_secs(10);

    /// A renderer stand-in that renders nothing. A frame tags itself with its clear color's red,
    /// which comes back as `Rendered::drawn.draws`, beside `setting` in `instances`. If `failing`,
    /// frame 2 fails and frame 3 panics. It answers each pick in its frame: the pixel's column is
    /// the ID's low half, its row the high.
    #[derive(Default)]
    struct Fake {
        setting: usize,
        failing: bool,
        /// Told each frame's tag as it starts, then waits to be told to go on.
        started: Option<(SyncSender<usize>, Receiver<()>)>,
        /// The frames it rendered, and the thread it dropped on.
        rendered: Arc<Mutex<Vec<usize>>>,
        dropped: Probe,
        /// Leaves texture changes alone; the renderer always applies them.
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
            frame.picked.clear();
            let answers = frame.picks.iter().map(|pick| {
                let [x, y] = pick.pixel.map(u64::from);
                (pick.tag, x | y << 32)
            });
            frame.picked.extend(answers);
            let presented = match tag {
                2 if self.failing => Err(Error::new("test.failed", "frame 2 fails")),
                3 if self.failing => panic!("frame 3 panics"),
                _ => Ok(true),
            };
            Rendered {
                presented,
                render: Duration::ZERO,
                wait: Duration::ZERO,
                publish: Duration::ZERO,
                drawn: DrawCounts {
                    draws: tag,
                    instances: self.setting,
                    triangles: 0,
                },
                gpu: None,
            }
        }
    }

    #[derive(Default)]
    struct Probe(Arc<Mutex<Option<ThreadId>>>);

    impl Drop for Probe {
        fn drop(&mut self) {
            *self.0.lock().unwrap() = Some(thread::current().id());
        }
    }

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
        // Only a thread of its own renders between handing over and taking back; rendering in
        // either would never see `go`.
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

    /// A panic filling a frame in resumes before the handover and leaves the renderer back: it can
    /// hand over again and drops on this thread, not with the frame while dropping waits for it.
    #[test]
    fn a_panic_filling_a_frame_in_leaves_the_renderer_back() {
        let (done, finished) = mpsc::sync_channel(1);
        // On a thread of its own, so a drop waiting for a renderer that never returns fails the
        // test rather than hangs it.
        thread::spawn(move || {
            let fake = Fake::default();
            let dropped = fake.dropped.0.clone();
            let mut render = RenderThread::start(fake).unwrap();
            let filling = AssertUnwindSafe(|| render.hand(|_| panic!("filling panics")));
            let panic = panic::catch_unwind(filling).expect_err("the panic resumes");
            assert_eq!(panic.downcast_ref::<&str>(), Some(&"filling panics"));
            hand(&mut render, 0);
            assert_eq!(render.take().expect("frame 0").drawn.draws, 0);
            drop(render);
            let here = Some(thread::current().id());
            done.send(*dropped.lock().unwrap() == here).unwrap();
        });
        assert_eq!(
            finished.recv_timeout(DEADLINE),
            Ok(true),
            "the renderer came back, to hand over again and to drop on the thread that dropped it"
        );
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

    /// Picks ride out with their frame, each at the view's pixel under it, and the answers come
    /// home with it into the context, for one frame: at the top left pixel the stand-in answers 0
    /// (no entity); inside, an entity's ID; outside the view or at no number, `None`. A frame asks
    /// at most as many as a frame reads back; the next asks again.
    #[test]
    fn picks_ride_out_with_their_frame_and_their_answers_come_home_with_it() {
        let mut render = RenderThread::start(Fake::default()).unwrap();
        let mut cx = Context::seeded(0);
        let view = Viewport {
            origin: [10, 20],
            size: [100, 50],
        };
        let pick = |cx: &mut Context, at| cx.pick(at).expect("fewer than a frame reads back");
        let corner = pick(&mut cx, [10.0, 20.9]);
        // At the view's pixel [2, 3].
        let inside = pick(&mut cx, [12.7, 23.2]);
        let outside = [
            [9.9, 30.0],
            [110.0, 30.0],
            [50.0, 19.5],
            [50.0, 70.0],
            [f64::NAN, 30.0],
        ]
        .map(|at| pick(&mut cx, at));
        hand_off(&mut render, &mut cx, view, MainUi::none(), Vec::new());
        assert!(cx.picked().is_empty(), "nothing has come back yet");
        let presented = take_back(&mut render, &mut cx, Duration::ZERO);
        assert_eq!(presented.expect("a frame"), Some(true));
        let mut answers = vec![(corner, None), (inside, EntityId::new(2 | 3 << 32))];
        answers.extend(outside.map(|pick| (pick, None)));
        assert_eq!(cx.picked(), answers);
        hand_off(&mut render, &mut cx, view, MainUi::none(), Vec::new());
        take_back(&mut render, &mut cx, Duration::ZERO).expect("a frame");
        assert!(cx.picked().is_empty(), "{:?}", cx.picked());
        for _ in 0..MAX_PICKS {
            pick(&mut cx, [50.0, 30.0]);
        }
        assert_eq!(cx.pick([50.0, 30.0]), None, "past what a frame reads back");
        hand_off(&mut render, &mut cx, view, MainUi::none(), Vec::new());
        assert!(cx.pick([50.0, 30.0]).is_some(), "the next frame's");
        take_back(&mut render, &mut cx, Duration::ZERO).expect("a frame");
    }

    /// A run logs `engine.loaded` once: when a frame comes back after the layers attached and
    /// nothing is on its way to the GPU, not before.
    #[test]
    fn a_run_logs_that_it_has_loaded_once_its_layers_have_attached() {
        let logged = crate::testing::logged();
        let mut render = RenderThread::start(Fake::default()).unwrap();
        let mut cx = Context::seeded(0);
        cx.load_started = Some(Instant::now());
        let view = Viewport {
            origin: [0, 0],
            size: [16, 16],
        };
        let mut frames = |cx: &mut Context, count: usize| {
            for _ in 0..count {
                hand_off(&mut render, cx, view, MainUi::none(), Vec::new());
                take_back(&mut render, cx, Duration::ZERO).expect("a frame");
            }
            let codes = logged.codes.try_iter();
            codes.filter(|code| code == "engine.loaded").count()
        };
        assert_eq!(frames(&mut cx, 2), 0, "before the layers attach");
        crate::layer::Stack::default().attach(&mut cx);
        assert_eq!(frames(&mut cx, 1), 1);
        assert_eq!(frames(&mut cx, 2), 0, "once");
    }

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
        // egui's assert fails a debug test as the changes drop, on each way a run ends.
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

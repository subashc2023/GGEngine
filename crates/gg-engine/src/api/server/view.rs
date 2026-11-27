//! The view: clients' look and input, what rides with frames (captures, films, picks, images), and
//! the published view.

use std::mem;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use gg_api::{Button, Failure, Hit, ImageList, InputEvent, Looked, Picked, Projected};
use gg_core::Result;
use gg_core::math::{DVec3, Quat, Vec2, Vec3, Vec4};
use gg_renderer::{MIN_RENDER_SCALE, Projection, Viewpoint};
use tracing::warn;

use super::actions;
use super::connection::Reply;
use super::files::{Png, Writing};
use super::parse::{Projecting, bad_params};
use super::world::Subject;
use super::{Api, failure};
use crate::api::Shown;
use crate::grid::Plane;
use crate::layer::Outlining;
use crate::render_thread::{Extracted, Publish, Shot};
use crate::{Context, Entity, EntityId, Event, Key, MouseButton, Pad, Scroll, bounds};

/// The longest a `capture` waits for a frame showing every texture and mesh added before it, none
/// still decoding or streaming. Then it takes the next frame anyway, saying so.
pub(super) const CAPTURE_WAIT: Duration = Duration::from_secs(10);

/// The longest a `capture` of paths traced waits for every pixel's [`gg_api::PATH_SAMPLES`]: under
/// gg's 30 s for an answer. Then it fails, saying how many each took.
pub(super) const TRACE_WAIT: Duration = Duration::from_secs(25);

/// How long a `capture` waits before the game warns what for (`api.capture_waiting`), past
/// [`CAPTURE_WAIT`] and inside gg's 30 s for an answer: a client that gives up sees why in the log.
const CAPTURE_STUCK: Duration = Duration::from_secs(15);

/// A `pick` awaiting its answer. `within` is [`gg_api::Pick::within`]; `ticket` comes once the
/// asking frame gives it; `named` answers the entity's name, for a connection granted `read` of
/// `Name`.
pub(super) struct Picking {
    pub(super) at: [f64; 2],
    pub(super) within: Option<EntityId>,
    pub(super) ticket: Option<crate::Pick>,
    pub(super) named: bool,
    pub(super) reply: Reply,
}

/// A `capture` awaiting its frame. `own` is the game's own view, which a bare frame shows; `show`
/// replaces the view's light; [`CAPTURE_WAIT`] counts from `since`. With `at` and no path it
/// answers the pixel and writes no PNG.
pub(super) struct Capturing {
    pub(super) reply: Reply,
    pub(super) path: Option<PathBuf>,
    pub(super) at: Option<[f64; 2]>,
    pub(super) own: bool,
    pub(super) show: Option<Shown>,
    pub(super) since: Instant,
}

/// A `frames` capturing consecutive frames, or waiting to. `rect` is the part kept; `show`
/// replaces the view's light; [`CAPTURE_WAIT`] counts from `since`. For `started`, see
/// [`Api::film_hands`]. `handed` holds each handed-off frame's ticks; `back` the frames
/// returned, in order.
pub(super) struct Filming {
    pub(super) reply: Reply,
    pub(super) dir: PathBuf,
    pub(super) count: u32,
    pub(super) viewpoints: Vec<Viewpoint>,
    pub(super) rect: Option<[u32; 4]>,
    pub(super) show: Option<Shown>,
    pub(super) since: Instant,
    pub(super) started: bool,
    pub(super) handed: Vec<u64>,
    pub(super) back: Vec<FilmShot>,
}

impl Filming {
    /// The next frame's viewpoint, if it's the film's: the first until the film starts, then each
    /// in turn, the last held. `None` without viewpoints, or once every frame is handed off.
    pub(super) fn viewpoint(&self) -> Option<Viewpoint> {
        let next = self.handed.len();
        if next >= self.count as usize {
            return None;
        }
        let last = self.viewpoints.len().checked_sub(1)?;
        self.viewpoints.get(next.min(last)).copied()
    }
}

/// A `frames` frame back from the renderer, its view cropped to the film's rect. `streaming` if
/// anything was still on its way to the GPU.
pub(super) struct FilmShot {
    pub(super) shot: Arc<Shot>,
    pub(super) ticks: u64,
    pub(super) streaming: bool,
    pub(super) viewpoint: Viewpoint,
}

impl Api {
    /// Sets `given`'s parts of the look as `client`. Any part's change of value or setter is a
    /// revision. Hands the look to `cx` for frames to draw with.
    pub(super) fn set_look(&mut self, cx: &mut Context, given: gg_api::Look, client: &str) {
        let Looked {
            viewpoint,
            outline,
            grid,
            colliders,
            show,
            skip,
            max_fps,
            render_scale,
            by,
            revision,
            ..
        } = &mut self.look;
        let changed = [
            set_part(viewpoint, &mut by.viewpoint, given.viewpoint, client),
            set_part(outline, &mut by.outline, given.outline, client),
            set_part(grid, &mut by.grid, given.grid, client),
            set_part(colliders, &mut by.colliders, given.colliders, client),
            set_part(show, &mut by.show, given.show, client),
            set_part(skip, &mut by.skip, given.skip, client),
            set_part(max_fps, &mut by.max_fps, given.max_fps, client),
            set_part(
                render_scale,
                &mut by.render_scale,
                given.render_scale,
                client,
            ),
        ];
        if changed.contains(&true) {
            *revision += 1;
        }
        let look = &mut cx.dev.look;
        look.viewpoint = viewpoint.as_ref().map(engine_viewpoint);
        look.outline = outline.as_ref().map(|outline| Outlining {
            ids: (outline.ids.iter())
                .map(|id| id.parse::<EntityId>().expect("checked as it was read"))
                .collect(),
            color: Vec4::from_array(outline.color),
            width: outline.width,
        });
        look.grid = grid.map(|plane| match plane {
            gg_api::Plane::Xy => Plane::XY,
            gg_api::Plane::Xz => Plane::XZ,
            gg_api::Plane::Yz => Plane::YZ,
        });
        look.colliders = *colliders;
        look.show = show.as_ref().map(Shown::of);
        look.skip = skip.clone().unwrap_or_default();
        look.max_fps = *max_fps;
        look.render_scale = *render_scale;
        look.revision = *revision;
    }

    /// With the renderer back from `frame`:
    ///
    /// - Takes the view read back, if it rode with captures and rendered, for a thread to write
    ///   each capture's PNG and answer. Only if every texture and mesh added before is there, none
    ///   decoding or streaming; else only for captures that waited [`CAPTURE_WAIT`], saying so.
    ///   Showing paths traced, only once every pixel has taken [`gg_api::PATH_SAMPLES`]; those
    ///   that waited [`TRACE_WAIT`] fail with `api.path_traced`. The rest ride on with the next
    ///   frame.
    /// - Answers picks whose answers came back into `cx`, naming each entity as `cx`'s world does
    ///   now.
    /// - Has the next frame handed off (by `cx`, showing the world after `ticks` ticks) read its
    ///   view back for captures of its kind (bare or not) asked since, and those whose frame had
    ///   nothing to render into or showed what was still on its way. Unless bare, it's published
    ///   to the memory a `view` asked for; an unpublished frame since then warns once.
    /// - Likewise answers `images` riding with handed-off frames with the images `frame` listed, if
    ///   it rendered, and has the next frame list them for those asked since or still riding.
    pub(crate) fn frame_back(&mut self, frame: &mut Extracted, cx: &Context, ticks: u64) {
        if let Some(viewing) = self.viewing.take() {
            frame.publish = viewing.map(|writer| Publish {
                writer,
                skip: false,
                ticks,
                started: cx.dev.started,
                failed: None,
            });
        }
        if let Some(publish) = &mut frame.publish {
            // The client grabs its own cursor for the game, and shows its frame times.
            publish.writer.set_cursor_grabbed(cx.cursor_grabbed());
            if let Some(times) = cx.perf.times() {
                publish.writer.time(times);
            }
            if let Some(error) = publish.failed.take()
                && !mem::replace(&mut self.view_warned, true)
            {
                let consequence = "frames go unpublished while that lasts";
                warn!(code = error.code(), "{error:#}; {consequence}");
            }
            publish.skip = self.apart();
            publish.ticks = ticks;
            publish.started = cx.dev.started;
        }
        let back = (frame.captured.take()).map(|shot| shot.map(Arc::new));
        let came_back = back.is_some();
        let streaming = frame.loading > 0 || !frame.streaming.is_empty();
        self.film_back(back.as_ref(), streaming, frame.viewpoint);
        // Paths traced short of their samples.
        let tracing = (frame.path_traced).filter(|&taken| taken < gg_api::PATH_SAMPLES);
        match back {
            Some(Ok(shot)) => {
                let now = Instant::now();
                if let Some(taken) = tracing {
                    let late = (self.riding).extract_if(.., |capturing| {
                        now.duration_since(capturing.since) >= TRACE_WAIT
                    });
                    for capturing in late {
                        let message = format!(
                            "the view's paths traced took {taken} of {} a pixel in {} s; a \
                             smaller view or a faster GPU takes them sooner",
                            gg_api::PATH_SAMPLES,
                            TRACE_WAIT.as_secs()
                        );
                        let error = gg_core::Error::new("api.path_traced", message);
                        capturing.reply.send(Err(failure(&error)));
                    }
                }
                let taken = (self.riding).extract_if(.., |capturing| {
                    tracing.is_none()
                        && (!streaming || now.duration_since(capturing.since) >= CAPTURE_WAIT)
                });
                for Capturing {
                    reply, path, at, ..
                } in taken
                {
                    let named = path.is_none() && at.is_none();
                    let path = match path {
                        Some(path) => Some(path),
                        None if at.is_some() => None,
                        None => {
                            self.numbered += 1;
                            let name = format!("{}-{}.png", self.stem, self.numbered);
                            Some(self.captures.join(name))
                        }
                    };
                    let png = Png {
                        shot: shot.clone(),
                        path,
                        at,
                        named,
                        ticks: self.riding_ticks,
                        streaming,
                        reply,
                    };
                    // The thread runs until the API drops.
                    let _ = self.pngs.send(Writing::Capture(png));
                }
            }
            Some(Err(error)) => {
                let failure = failure(&error);
                for capturing in self.riding.drain(..) {
                    capturing.reply.send(Err(failure.clone()));
                }
            }
            None => {}
        }
        let picked = cx.picked();
        let answered = |ticket: Option<crate::Pick>| {
            let ticket = ticket?;
            picked
                .iter()
                .find(|(pick, _)| *pick == ticket)
                .map(|&(_, id)| id)
        };
        let world = cx.world();
        let back = (self.picks).extract_if(.., |picking| answered(picking.ticket).is_some());
        for Picking {
            within,
            ticket,
            named,
            reply,
            ..
        } in back
        {
            let shown = answered(ticket).flatten();
            // A model's node is picked as its root, which tools change in its place. Within the
            // model it's picked as itself, as an editor goes into a picked root or node.
            let drawn = shown.and_then(|id| world.entity(id));
            let root = drawn.map(|entity| world.node_root(entity));
            let within = within.and_then(|id| world.entity(id));
            let within = within.map(|entity| world.node_root(entity));
            let entity = if within.is_some() && within == root {
                drawn
            } else {
                root
            };
            let id = entity.map_or(shown, |entity| world.id(entity));
            let name = |entity: Option<Entity>| {
                let name = entity
                    .filter(|_| named)
                    .and_then(|entity| world.name(entity));
                name.map(str::to_owned)
            };
            let parent = entity.and_then(|entity| world.parent(entity).ok().flatten());
            reply.result(&Picked {
                id: id.map(|id| id.to_string()),
                name: name(entity),
                parent: (parent.and_then(|parent| world.id(parent))).map(|id| id.to_string()),
                parent_name: name(parent),
            });
        }
        let (bare, shown) = (self.bare, &self.shown);
        let kind = (self.asked).extract_if(.., |capturing| {
            capturing.own == bare && capturing.show == *shown
        });
        self.riding.extend(kind);
        if !self.riding.is_empty() {
            self.riding_ticks = ticks;
        }
        self.warn_stuck(came_back, streaming, tracing.is_some());
        let filming = self.film_hands(streaming, ticks);
        frame.capture = !self.riding.is_empty() || filming;
        if let Some(images) = frame.images.take() {
            let listed = ImageList {
                images: images.iter().map(frame_image).collect(),
            };
            for reply in self.images_riding.drain(..) {
                reply.result(&listed);
            }
        }
        self.images_riding.append(&mut self.images_asked);
        frame.list_images = !self.images_riding.is_empty();
    }

    /// Warns once (`api.capture_waiting`) as the oldest `capture` waiting passes [`CAPTURE_STUCK`],
    /// saying what the frame just back showed of why, so a client whose call timed out finds it
    /// in the log. Warns again only after none waits.
    fn warn_stuck(&mut self, came_back: bool, streaming: bool, tracing: bool) {
        let waiting = self.riding.iter().chain(&self.asked);
        let Some(since) = waiting.map(|capturing| capturing.since).min() else {
            self.capture_warned = false;
            return;
        };
        let waited = since.elapsed();
        if self.capture_warned || waited < CAPTURE_STUCK {
            return;
        }
        self.capture_warned = true;
        let why = if self.riding.is_empty() {
            "no frame of its kind has been handed off to read its view"
        } else if !came_back {
            "the frame just back didn't render or read its view"
        } else if tracing {
            "the view's paths traced are short of their samples"
        } else if streaming {
            "textures or meshes are still on their way"
        } else {
            "it rides the next frame handed off"
        };
        warn!(
            code = "api.capture_waiting",
            riding = self.riding.len(),
            asked = self.asked.len(),
            "A `capture` has waited {} s for the view: {why}",
            waited.as_secs()
        );
    }

    /// Keeps a frame back (`back`, its view if read) as the first film's next frame, cropped to its
    /// rect, if that film has a frame in flight. Once the film has all its frames, the PNG thread
    /// writes them and answers. A frame without its view, or with none of the rect, fails the film.
    fn film_back(
        &mut self,
        back: Option<&Result<Arc<Shot>>>,
        streaming: bool,
        viewpoint: Viewpoint,
    ) {
        let Some(film) = self.films.first_mut() else {
            return;
        };
        if film.back.len() == film.handed.len() {
            return;
        }
        let shot = match back {
            Some(Ok(shot)) => crop(shot, film.rect).ok_or_else(|| {
                let message = format!(
                    "a frame's view, of {}x{} pixels, has none of the rect {:?}",
                    shot.size[0], shot.size[1], film.rect
                );
                Failure::new("api.no_view", message)
            }),
            Some(Err(error)) => Err(failure(error)),
            None => {
                let message = "a frame of them didn't render, as while a window resizes, so they \
                               wouldn't be in a row: ask again";
                Err(Failure::new("api.no_view", message))
            }
        };
        match shot {
            Ok(shot) => film.back.push(FilmShot {
                shot,
                ticks: film.handed[film.back.len()],
                streaming,
                viewpoint,
            }),
            Err(failure) => {
                self.film_frame = false;
                self.films.remove(0).reply.send(Err(failure));
                return;
            }
        }
        if film.back.len() == film.count as usize {
            let Filming {
                reply, dir, back, ..
            } = self.films.remove(0);
            // The thread runs until the API drops.
            let _ = self.pngs.send(Writing::Frames {
                dir,
                shots: back,
                reply,
            });
        }
    }

    /// Whether the next handed-off frame (after `ticks` ticks) reads its view back for the first
    /// film: made as the film's ([`serve`](Self::serve)), once the film starts at the first frame
    /// back not `streaming`, or after [`CAPTURE_WAIT`].
    fn film_hands(&mut self, streaming: bool, ticks: u64) -> bool {
        let film = (self.films.first_mut()).filter(|_| self.film_frame);
        let Some(film) = film else {
            return false;
        };
        film.started |= !streaming || film.since.elapsed() >= CAPTURE_WAIT;
        if film.started {
            film.handed.push(ticks);
        }
        film.started
    }

    /// While the view is published, the next handed-off frame's number, as the client takes it.
    /// `None` while the frame is apart, so unpublished.
    pub(crate) fn handing(&self) -> Option<u64> {
        let pacer = self.pacer.as_ref().filter(|_| !self.apart())?;
        Some(pacer.published().saturating_add(1))
    }

    /// Whether the frame being made is unpublished: bare, or showing what a capture asks in place
    /// of the view's light.
    fn apart(&self) -> bool {
        self.bare || self.shown.is_some()
    }

    /// While the view is published, waits at most `timeout` for the client to take frame `handed`
    /// ([`handing`](Self::handing)). Returns how long it waited.
    pub(crate) fn pace(&self, handed: u64, timeout: Duration) -> Duration {
        let Some(pacer) = &self.pacer else {
            return Duration::ZERO;
        };
        let start = Instant::now();
        while pacer.taken() < handed && start.elapsed() < timeout {
            thread::sleep(Duration::from_micros(250));
        }
        start.elapsed()
    }
}

/// Sets look part `part` and its setter `by` as `change` asks, for `client`. `null` gives the part
/// back to the game, with no setter. Returns whether its value or setter changed.
fn set_part<T: PartialEq>(
    part: &mut Option<T>,
    by: &mut Option<String>,
    change: Option<Option<T>>,
    client: &str,
) -> bool {
    let Some(value) = change else {
        return false;
    };
    let setter = value.is_some().then(|| client.to_owned());
    let changed = *part != value || *by != setter;
    (*part, *by) = (value, setter);
    changed
}

/// `image` as `images` answers it.
fn frame_image(image: &gg_renderer::FrameImage) -> gg_api::FrameImage {
    use gg_api::ImageKind as Api;
    use gg_renderer::ImageKind;

    gg_api::FrameImage {
        name: image.name.to_owned(),
        kind: match image.kind {
            ImageKind::Color => Api::Color,
            ImageKind::Hdr => Api::Hdr,
            ImageKind::Ids => Api::Ids,
            ImageKind::Motion => Api::Motion,
            ImageKind::Visibility => Api::Visibility,
            ImageKind::Depth => Api::Depth,
            ImageKind::Scalar => Api::Scalar,
            ImageKind::Normals => Api::Normals,
        },
        size: image.size,
    }
}

/// `viewpoint` as the tool API gives one.
pub fn api_viewpoint(viewpoint: &Viewpoint) -> gg_api::Viewpoint {
    let projection = match viewpoint.projection {
        Projection::Orthographic { height, near, far } => {
            gg_api::Projection::Orthographic { height, near, far }
        }
        Projection::Perspective { fov, near } => gg_api::Projection::Perspective { fov, near },
    };
    gg_api::Viewpoint {
        position: viewpoint.position.to_array(),
        rotation: viewpoint.rotation.to_array(),
        projection,
    }
}

/// `given` as the engine's viewpoint.
pub fn engine_viewpoint(given: &gg_api::Viewpoint) -> Viewpoint {
    Viewpoint {
        position: DVec3::from_array(given.position),
        rotation: Quat::from_array(given.rotation),
        projection: engine_projection(given.projection),
    }
}

fn engine_projection(given: gg_api::Projection) -> Projection {
    match given {
        gg_api::Projection::Orthographic { height, near, far } => {
            Projection::Orthographic { height, near, far }
        }
        gg_api::Projection::Perspective { fov, near } => Projection::Perspective { fov, near },
    }
}

/// What a ray from the view's camera through the view's pixel `at`, else its middle, hits first
/// of the colliders, past `exclude`'s and those the camera is inside, as `cast` answers it; with
/// its entity's name if `named`. A 2D collider is hit where the ray meets the plane z = 0
/// ([`World::collider_at_2d`](crate::World::collider_at_2d)).
///
/// Fails with `api.no_view` before a frame has drawn the view, and in replays; and as `exclude`
/// resolves. An `exclude` of no living entity excludes none.
pub(super) fn cast(
    cx: &Context,
    at: Option<[f64; 2]>,
    exclude: Option<Subject>,
    named: bool,
) -> Result<Hit, Failure> {
    let world = cx.world();
    let exclude = exclude.map(|exclude| exclude.resolve(world));
    let exclude = exclude.transpose().map_err(|error| failure(&error))?;
    let view = cx.view().ok_or_else(undrawn)?;
    let middle = view.size.map(|size| f64::from(size) * 0.5);
    let in_window =
        |[x, y]: [f64; 2]| [x + f64::from(view.origin[0]), y + f64::from(view.origin[1])];
    cx.ray(in_window(middle)).ok_or_else(undrawn)?;
    let mut hit = Hit {
        id: None,
        name: None,
        point: None,
        normal: None,
        distance: None,
    };
    // Outside the view, it hits nothing, as a pick there picks nothing.
    let Some(mut ray) = cx.ray(in_window(at.unwrap_or(middle))) else {
        return Ok(hit);
    };
    ray.exclude = exclude.and_then(|id| world.entity(id));
    let solid = world.cast_ray_from_outside(ray);
    let solid = solid.map(|hit| (hit.entity, hit.point, hit.normal, hit.distance));
    // A 2D collider, where the ray meets the plane 2D physics lies in, facing the camera.
    let flat = ray.meets_plane(DVec3::ZERO, Vec3::Z).and_then(|point| {
        let entity = world.collider_at_2d(point.truncate(), ray.exclude)?;
        let normal = Vec3::new(0.0, 0.0, -ray.toward.z.signum());
        Some((entity, point, normal, ray.from.distance(point) as f32))
    });
    let first = match (solid, flat) {
        (Some(solid), Some(flat)) if flat.3 < solid.3 => Some(flat),
        (solid, flat) => solid.or(flat),
    };
    if let Some((entity, point, normal, distance)) = first {
        hit.id = world.id(entity).map(|id| id.to_string());
        hit.name = (world.name(entity).filter(|_| named)).map(str::to_owned);
        hit.point = Some(point.to_array());
        hit.normal = Some(normal.to_array());
        hit.distance = Some(distance);
    }
    Ok(hit)
}

/// Where the view shows what `projecting` names, as `project` answers it: an entity's middle of
/// its bounds, else its origin.
///
/// Fails with `api.no_view` before a frame has drawn the view, and in replays; and as the entity
/// resolves, or with `ecs.no_entity` for an ID no living entity has.
pub(super) fn project(cx: &Context, projecting: Projecting) -> Result<Projected, Failure> {
    let world = cx.world();
    let point = match projecting {
        Projecting::Point(point) => DVec3::from_array(point),
        Projecting::Entity(subject) => {
            let id = subject.resolve(world).map_err(|error| failure(&error))?;
            let entity = world.entity(id).ok_or_else(|| {
                Failure::new("ecs.no_entity", format!("no living entity has ID {id}"))
            })?;
            match bounds::of(world, cx.meshes(), entity) {
                Some((center, _)) => center,
                None => {
                    world
                        .pose(entity)
                        .map_err(|error| failure(&error))?
                        .matrix
                        .translation
                }
            }
        }
    };
    let sighting = cx.sighting(point).ok_or_else(undrawn)?;
    Ok(Projected {
        point: point.to_array(),
        at: sighting.at,
        turn: sighting.turn,
        distance: sighting.distance,
    })
}

/// `api.no_view`, for a `cast` or `project` before a frame has drawn the view.
fn undrawn() -> Failure {
    let message = "no frame has drawn the view yet, or the run replays one: run a frame, as \
                   `step` does, first";
    Failure::new("api.no_view", message)
}

/// `shot` cropped to `rect` (`[x, y, width, height]` from its top left), whole without one;
/// `None` if they don't overlap.
fn crop(shot: &Arc<Shot>, rect: Option<[u32; 4]>) -> Option<Arc<Shot>> {
    let Some([x, y, width, height]) = rect else {
        return Some(shot.clone());
    };
    let [shot_width, shot_height] = shot.size;
    let right = x.saturating_add(width).min(shot_width);
    let bottom = y.saturating_add(height).min(shot_height);
    if x >= right || y >= bottom {
        return None;
    }
    let row = (right - x) as usize * 4;
    let mut pixels = Vec::with_capacity(row * (bottom - y) as usize);
    for line in y..bottom {
        let start = (line as usize * shot_width as usize + x as usize) * 4;
        pixels.extend_from_slice(&shot.pixels[start..start + row]);
    }
    Some(Arc::new(Shot {
        size: [right - x, bottom - y],
        pixels,
    }))
}

/// Checks look `given`, normalizing the viewpoint's rotation. Skipped passes are put in
/// [`gg_api::Pass::ALL`]'s order, each once; an empty list becomes `None`.
///
/// Fails with `api.bad_params`, saying why, for:
///
/// - a viewpoint like [`checked_viewpoint`] refuses;
/// - an outline with an invalid entity ID, a non-finite color, or a width not finite and positive;
/// - a `show` that [`shown`] refuses;
/// - a `render_scale` outside [`MIN_RENDER_SCALE`] to 1, or not a number.
pub(super) fn look(method: &str, mut given: gg_api::Look) -> Result<gg_api::Look, Failure> {
    if let Some(Some(viewpoint)) = &mut given.viewpoint {
        checked_viewpoint(method, viewpoint)?;
    }
    if let Some(Some(outline)) = &given.outline {
        for id in &outline.ids {
            if let Err(error) = id.parse::<EntityId>() {
                let what = format!("takes an outline of entities' IDs: {error}");
                return Err(bad_params(method, what));
            }
        }
        if !outline.color.iter().all(|channel| channel.is_finite()) {
            return Err(bad_params(method, "takes an outline whose color is finite"));
        }
        if !(outline.width.is_finite() && outline.width > 0.0) {
            let what = format!(
                "takes an outline whose width is finite and positive, not {}",
                outline.width
            );
            return Err(bad_params(method, what));
        }
    }
    if let Some(Some(show)) = &given.show {
        shown(method, show)?;
    }
    if let Some(Some(scale)) = given.render_scale
        && !(MIN_RENDER_SCALE..=1.0).contains(&scale)
    {
        let what = format!(
            "takes a `render_scale` from {MIN_RENDER_SCALE} to 1, not {scale}; \
             `null` gives it back to the game"
        );
        return Err(bad_params(method, what));
    }
    // A set, so the same passes again aren't a change; none left out is the game's own.
    if let Some(skip) = &mut given.skip {
        if let Some(passes) = skip.as_mut() {
            passes.sort_unstable();
            passes.dedup();
        }
        if skip.as_ref().is_some_and(Vec::is_empty) {
            *skip = None;
        }
    }
    Ok(given)
}

/// Checks a viewpoint `method` takes, normalizing its rotation.
///
/// Fails with `api.bad_params`, saying why, if it isn't finite, its rotation has no length, or its
/// projection can't project ([`Projection`]).
pub(super) fn checked_viewpoint(
    method: &str,
    viewpoint: &mut gg_api::Viewpoint,
) -> Result<(), Failure> {
    if !viewpoint.is_finite() {
        let what = "takes a viewpoint whose numbers are finite";
        return Err(bad_params(method, what));
    }
    let Some(rotation) = Vec4::from_array(viewpoint.rotation).try_normalize() else {
        let what = "takes a viewpoint whose rotation is a quaternion with a length, which is \
                    normalized";
        return Err(bad_params(method, what));
    };
    viewpoint.rotation = rotation.to_array();
    let projection = engine_projection(viewpoint.projection);
    if !projection.projects() {
        let what = format!(
            "takes a projection that projects: orthographic, of a positive height, between a near \
             and a far that differ; or perspective, of a field of view more than 0 and less than \
             π, from a positive near; not {projection:?}"
        );
        return Err(bad_params(method, what));
    }
    Ok(())
}

/// Checks a `look`'s or `capture`'s `show`: an image name of 1 to [`gg_api::MOST_IMAGE_NAME`]
/// bytes.
pub(super) fn shown(method: &str, show: &gg_api::Shown) -> Result<(), Failure> {
    match show {
        gg_api::Shown::Image(name) if name.is_empty() || name.len() > gg_api::MOST_IMAGE_NAME => {
            let what = format!(
                "takes an image's name of 1 to {} bytes, not {} bytes",
                gg_api::MOST_IMAGE_NAME,
                name.len()
            );
            Err(bad_params(method, what))
        }
        _ => Ok(()),
    }
}

/// `given` as the layers get it: a cursor position over the view, at `origin` in the main window,
/// becomes the window's. `None` for an unknown key name, and a wait, an aim or an action, which the
/// server makes into ticks, a cursor and a binding's events first.
pub(super) fn input_event(given: &InputEvent, origin: [u32; 2]) -> Option<Event<'_>> {
    let event = match *given {
        InputEvent::Key {
            ref key,
            pressed,
            repeat,
        } => Event::Key {
            key: Key::from_name(key)?,
            pressed,
            repeat,
        },
        InputEvent::Text(ref text) => Event::Text(text),
        InputEvent::Cursor([x, y]) => Event::CursorMoved {
            x: x + f64::from(origin[0]),
            y: y + f64::from(origin[1]),
        },
        InputEvent::CursorEntered => Event::CursorEntered,
        InputEvent::CursorLeft => Event::CursorLeft,
        InputEvent::Motion([x, y]) => Event::MouseMotion { x, y },
        InputEvent::Button { button, pressed } => Event::MouseButton {
            button: match button {
                Button::Left => MouseButton::Left,
                Button::Right => MouseButton::Right,
                Button::Middle => MouseButton::Middle,
                Button::Back => MouseButton::Back,
                Button::Forward => MouseButton::Forward,
                Button::Other(number) => MouseButton::Other(number),
            },
            pressed,
        },
        InputEvent::Scroll(gg_api::Scroll::Lines([x, y])) => Event::Scroll(Scroll::Lines { x, y }),
        InputEvent::Scroll(gg_api::Scroll::Pixels([x, y])) => {
            Event::Scroll(Scroll::Pixels { x, y })
        }
        InputEvent::Focused(focused) => Event::Focused(focused),
        InputEvent::PadButton {
            pad,
            button,
            pressed,
        } => Event::PadButton {
            pad: Pad(pad as usize),
            button: actions::pad_button(button),
            pressed,
        },
        InputEvent::PadStick { pad, stick, value } => Event::PadStick {
            pad: Pad(pad as usize),
            stick: actions::stick(stick),
            value: Vec2::from(value).clamp_length_max(1.0),
        },
        InputEvent::PadTrigger {
            pad,
            trigger,
            value,
        } => Event::PadTrigger {
            pad: Pad(pad as usize),
            trigger: actions::trigger(trigger),
            value,
        },
        InputEvent::Wait(_) | InputEvent::Aim(_) | InputEvent::Action { .. } => return None,
    };
    Some(event)
}

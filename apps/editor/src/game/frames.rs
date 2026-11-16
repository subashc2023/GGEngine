//! The view's frames, as the game renders them, and the look the editor keeps with the game.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gg_api::view::Reader;
use gg_engine::grid::Plane;
use gg_engine::{Context, EntityId, Viewpoint, Viewport, api_viewpoint, engine_viewpoint};
use serde_json::json;
use tracing::info;

use crate::look::{Part, Parts};
use crate::overlay::Mode;
use crate::view::{OUTLINE, OUTLINE_WIDTH, ViewTools};

use super::{Game, Level, picked};

/// How many frames the measures of the view's frames keep, from the last; 68 s at 240 fps.
const MEASURED: usize = 16_384;

/// What the parts of the look the editor wants go by, which between updates only the user changes:
/// whether the game's camera drives the view, what the editor's camera sees from, what the
/// Inspector has picked, and the toolbar's grid, its plane, the colliders' switch, what the view
/// shows and the passes it leaves out.
#[derive(Clone, PartialEq)]
pub(super) struct Wants {
    pub(super) through: bool,
    pub(super) viewpoint: Viewpoint,
    pub(super) picks: Vec<EntityId>,
    pub(super) grid: bool,
    pub(super) plane: Plane,
    pub(super) colliders: bool,
    pub(super) show: Option<gg_api::Shown>,
    pub(super) skip: Vec<gg_api::Pass>,
}

impl Game {
    /// Has the game render its view at `size`, the Viewport's in pixels: in the memory it's shared
    /// through, if it fits there, or else in memory made for it, a bit larger, so that a drag that
    /// grows the view makes few, which a `view` asks the game to share it through, once the last
    /// asked is answered.
    pub(super) fn fit(&mut self, size: [u32; 2]) {
        let Some(talker) = &self.talker else {
            return;
        };
        // Past the largest side frames have, the view shows as much as a frame does.
        let size = size.map(|side| side.min(gg_api::view::MAX_SIDE));
        let fits = self.reader.as_ref().is_some_and(|reader| reader.ask(size));
        if self.asked.is_some() || self.refused || fits {
            return;
        }
        let roomy = size.map(|side| side.next_multiple_of(256).min(gg_api::view::MAX_SIDE));
        match Reader::create(roomy[0], roomy[1]) {
            Ok(reader) => {
                let fits = reader.ask(size);
                debug_assert!(fits, "the memory is as large as the frames asked");
                talker.ask("view", json!({"memory": reader.name()}));
                self.asked = Some(reader);
            }
            Err(error) => {
                let message = format!("can't make memory for a {size:?} view: {error}");
                self.log(Level::Error, message);
            }
        }
    }

    /// Takes the parts of the look that `taken` sets, as another client set them: a viewpoint the
    /// editor's camera adopts, driving the view, and `null` turns the view back to the game's
    /// camera, either turn taking the grid with it, as the user's does, unless another client set
    /// the grid; an outline's entities the Inspector picks, and `null` unpicks; and the grid and the
    /// colliders set the toolbar's switches, and the grid its plane; and what the view shows and
    /// the passes it leaves out set the toolbar's choices.
    pub(super) fn take_look(&mut self, taken: gg_api::Look) {
        let toolbar = &mut self.view.toolbar;
        let through = toolbar.world_camera;
        match taken.viewpoint {
            Some(Some(viewpoint)) => {
                self.view.camera.adopt(&engine_viewpoint(&viewpoint));
                toolbar.world_camera = false;
            }
            Some(None) => toolbar.world_camera = true,
            None => {}
        }
        if toolbar.world_camera != through {
            self.looking.follow(Part::Grid);
        }
        if let Some(outline) = taken.outline {
            let ids = outline.iter().flat_map(|outline| &outline.ids);
            let ids: Vec<EntityId> = ids.filter_map(|id| id.parse().ok()).collect();
            self.view.link.pick_all(&ids);
            self.took_pick = Some(ids);
        }
        if let Some(grid) = taken.grid {
            toolbar.grid = grid.is_some();
            if let Some(plane) = grid {
                toolbar.plane = engine_plane(plane);
            }
        }
        if let Some(colliders) = taken.colliders {
            toolbar.colliders = colliders == Some(true);
        }
        if let Some(show) = taken.show {
            toolbar.show = show;
        }
        if let Some(skip) = taken.skip {
            toolbar.skip = skip.unwrap_or_default();
        }
    }

    /// Shows the game's latest frame, if one came since: what it was drawn from is what the handles
    /// are hit against, while it projects, and the editor's camera adopts it while the game's
    /// drives the view, in Edit turning the view to the editor's camera, if the game's turned only
    /// for that, once a frame drew through the game's; and the look it drew with may have changed
    /// since the editor last read it.
    pub(super) fn take_frame(&mut self, cx: &mut Context) {
        // Another turn of the view's ends the adoption.
        self.adopting &= self.view.toolbar.world_camera;
        let Some(reader) = &mut self.reader else {
            return;
        };
        let Some(frame) = reader.take() else {
            return;
        };
        let copying = Instant::now();
        cx.show_pixels(frame.size, |pixels| reader.read(pixels));
        self.measures
            .add(frame.started, copying.elapsed(), frame.size);
        let seen = engine_viewpoint(&frame.viewpoint);
        self.seen = seen.projection.projects().then_some(seen);
        self.size = Some(frame.size);
        if self.view.toolbar.world_camera
            && let Some(seen) = self.seen
        {
            self.view.camera.adopt(&seen);
            let edit = self.mode() == Mode::Edit && self.playing == 0;
            if self.adopting && edit && self.looking.through_games_camera(frame.look) {
                self.view.toolbar.world_camera = false;
                self.adopting = false;
            }
        }
        self.looking.drew(frame.look);
    }

    /// Where the view shows the game's last frame, in the main window's pixels, with the view at
    /// `view`: from its top left, as large as the frame. What the editor's camera and the handles
    /// read as the view.
    pub(super) fn screen(&self, view: Option<Viewport>) -> Option<Viewport> {
        let (view, size) = view.zip(self.size)?;
        Some(Viewport {
            origin: view.origin,
            size,
        })
    }

    /// Tells the look which of its parts the user changed since the last update, as what the parts
    /// the editor wants go by changed since, to `now`: the viewpoint, as the editor's camera moved
    /// while it drives the view, by its input, an ease or flying, or the view turned, to the
    /// editor's camera or back to the game's; the outline, as the picks changed, but to what the
    /// Inspector made of the picks the look's outline asked for, its entities, or none for IDs a
    /// read of the game has found gone; the grid, as the view turned, or the toolbar's grid or its
    /// plane changed; the colliders, as their switch did; and what the view shows and the passes it
    /// leaves out, as the toolbar's choices did.
    pub(super) fn changes(&mut self, now: Wants) {
        let was = self.wants.clone();
        let turned = now.through != was.through;
        if turned || (!now.through && now.viewpoint != was.viewpoint) {
            self.looking.change(Part::Viewpoint);
        }
        // A pick of the user's in the Inspector's pass that made the outline's is theirs; an
        // unpick of theirs there passes for the outline's.
        let took = self.took_pick.take();
        let taken = took.is_some_and(|asked| now.picks == asked || now.picks.is_empty());
        if now.picks != was.picks && !taken {
            self.looking.change(Part::Outline);
        }
        if turned || now.grid != was.grid || now.plane != was.plane {
            self.looking.change(Part::Grid);
        }
        if now.colliders != was.colliders {
            self.looking.change(Part::Colliders);
        }
        if now.show != was.show {
            self.looking.change(Part::Show);
        }
        if now.skip != was.skip {
            self.looking.change(Part::Skip);
        }
    }

    /// Frames what the Entities panel asked to frame, as the view's F does, once the link places
    /// every pick: the active one, as the panel picks what it frames, and the others; or drops the
    /// ask as the active pick changes.
    pub(super) fn frame_asked(&mut self) {
        let link = &self.view.link;
        if let Some(asked) = link.framed() {
            self.framing = Some(asked);
        }
        let Some(asked) = self.framing else {
            return;
        };
        if link.picked() != Some(asked) {
            self.framing = None;
        } else if let Some(placed) = picked(link).filter(|placed| placed.entity == asked) {
            self.view.frame(&placed);
            self.framing = None;
        }
    }

    /// What the editor wants of the game's look, with the window at `scale` pixels a point: the
    /// editor's camera's viewpoint while it drives the view; every entity the Inspector has picked
    /// outlined; the
    /// toolbar's grid, while it's on and the editor's camera drives; the colliders' outlines while
    /// their switch is on, or else whether the game shows them; what the toolbar has the view show,
    /// or else its light; and the passes the toolbar has it leave out, if any.
    pub(super) fn wanted(&self, scale: f32) -> Parts {
        let ViewTools {
            camera,
            toolbar,
            link,
            ..
        } = &self.view;
        let driving = !toolbar.world_camera;
        let picks = link.picks();
        let outline = (!picks.is_empty()).then(|| gg_api::Outline {
            ids: picks.iter().map(EntityId::to_string).collect(),
            color: OUTLINE.to_array(),
            width: OUTLINE_WIDTH * scale,
        });
        Parts {
            viewpoint: driving.then(|| api_viewpoint(&camera.viewpoint())),
            outline,
            grid: (driving && toolbar.grid).then(|| api_plane(toolbar.plane)),
            colliders: toolbar.colliders.then_some(true),
            show: toolbar.show.clone(),
            skip: (!toolbar.skip.is_empty()).then(|| toolbar.skip.clone()),
        }
    }
}

/// What the parts of the look the editor wants go by, as `view`, the view's tools, has them.
pub(super) fn wants<T, F>(view: &ViewTools<T, F>) -> Wants {
    Wants {
        through: view.toolbar.world_camera,
        viewpoint: view.camera.viewpoint(),
        picks: view.link.picks(),
        grid: view.toolbar.grid,
        plane: view.toolbar.plane,
        colliders: view.toolbar.colliders,
        show: view.toolbar.show.clone(),
        skip: view.toolbar.skip.clone(),
    }
}

/// `plane` as the tool API names it.
fn api_plane(plane: Plane) -> gg_api::Plane {
    match plane {
        Plane::XY => gg_api::Plane::Xy,
        Plane::XZ => gg_api::Plane::Xz,
        Plane::YZ => gg_api::Plane::Yz,
    }
}

/// The plane the tool API's `given` names.
fn engine_plane(given: gg_api::Plane) -> Plane {
    match given {
        gg_api::Plane::Xy => Plane::XY,
        gg_api::Plane::Xz => Plane::XZ,
        gg_api::Plane::Yz => Plane::YZ,
    }
}

/// What the editor measures of the view's frames, as it shows them: each frame's age, from when
/// the game's frame that rendered it started to when the editor took it, and how long copying it
/// out of the memory took, of the last [`MEASURED`] frames; and the size of the last.
#[derive(Default)]
pub(super) struct Measures {
    frames: u64,
    ages: Vec<Duration>,
    copies: Vec<Duration>,
    size: [u32; 2],
}

impl Measures {
    fn add(&mut self, started: u64, copy: Duration, size: [u32; 2]) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let age = now.saturating_sub(Duration::from_nanos(started));
        let at = (self.frames % MEASURED as u64) as usize;
        if self.ages.len() < MEASURED {
            self.ages.reserve_exact(MEASURED - self.ages.len());
            self.copies.reserve_exact(MEASURED - self.copies.len());
            self.ages.push(age);
            self.copies.push(copy);
        } else {
            self.ages[at] = age;
            self.copies[at] = copy;
        }
        self.frames += 1;
        self.size = size;
    }

    /// Logs them, `editor.view`, as the editor ends, in milliseconds: their median, 95th and 99th
    /// percentiles, by nearest rank, and maximum.
    pub(super) fn report(&self) {
        if self.frames == 0 {
            return;
        }
        let stats = |times: &[Duration]| {
            let mut sorted = times.to_vec();
            sorted.sort_unstable();
            let at = |percent: usize| {
                let rank = (percent * sorted.len()).div_ceil(100);
                sorted[rank - 1].as_secs_f64() * 1000.0
            };
            let max = sorted.last().copied().unwrap_or_default().as_secs_f64() * 1000.0;
            [at(50), at(95), at(99), max]
        };
        let [age_p50, age_p95, age_p99, age_max] = stats(&self.ages);
        let [copy_p50, copy_p95, copy_p99, copy_max] = stats(&self.copies);
        info!(
            code = "editor.view",
            frames = self.frames,
            measured = self.ages.len(),
            width = self.size[0],
            height = self.size[1],
            age_ms_p50 = age_p50,
            age_ms_p95 = age_p95,
            age_ms_p99 = age_p99,
            age_ms_max = age_max,
            copy_ms_p50 = copy_p50,
            copy_ms_p95 = copy_p95,
            copy_ms_p99 = copy_p99,
            copy_ms_max = copy_max,
            "The game's view, as the editor showed it"
        );
    }
}

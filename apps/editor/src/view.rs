//! The tools of the editor's view, alike in its own world and in a project's game: its camera; the
//! toolbar; the handles, which pick and edit through the Inspector's link; the pick a press off
//! them asks for, and what the view shows under the cursor; and what they paint over the view and
//! show over it. What depends on the world the view shows, the layer gives them: what the Inspector
//! has picked, as the handles see it; the picks, which it asks for and whose answers it hands
//! them; whether there's a camera to see through; and what's under the cursor called.

use std::ops::ControlFlow;

use gg_engine::egui::{Align2, CursorIcon};
use gg_engine::grid;
use gg_engine::math::{Vec2, Vec3, Vec4};
use gg_engine::{Context, Event, Input, InspectorLink, Key, Ui, Viewpoint, Viewport};

use crate::camera::{Move, OrbitCamera, Screen, Side, Tool};
use crate::handles::{Aim, Handles, Picked, Reading, Reply, Sight};
use crate::overlay::{self, Toolbar};

/// The color of the outline round what's picked, in linear RGBA: an orange, as other editors
/// outline what's selected in.
pub const OUTLINE: Vec4 = Vec4::new(1.0, 0.35, 0.02, 1.0);

/// How wide the outline round what's picked is, in points, as the colliders' outlines are.
pub const OUTLINE_WIDTH: f32 = 2.0;

/// The view's tools: the editor's camera, the toolbar and the handles, which pick and edit through
/// the Inspector's link; what the view sees from, which the handles are hit against; the pick a
/// press asked for; and what the view shows under the cursor. Each pick's ticket is a `T`, and
/// what one finds an `F`, as the layer asks for picks and reads their answers.
pub struct ViewTools<T, F> {
    pub camera: OrbitCamera,
    pub toolbar: Toolbar,
    pub handles: Handles,
    /// The Inspector's, which says what's picked, and takes the handles' edits.
    pub link: InspectorLink,
    /// What the view on screen sees from, while that's the editor's camera, which the handles are
    /// hit against.
    pub shown: Option<Viewpoint>,
    /// Where the last press over the view off the handles asked to pick, in the main window's
    /// pixels, until the layer asks for it, keeping the ticket in `asked`.
    pub picking: Option<[f64; 2]>,
    /// The ticket of the latest pick asked for, until its answer comes back, which the layer has
    /// the Inspector pick, as [`answer`] says: a later press's takes its place, and a grab of a
    /// handle drops it, so that it picks nothing else under the drag.
    pub asked: Option<T>,
    /// What the view shows under the cursor, from the picks asked at the cursor while it's over the
    /// view, which the view's bottom left names.
    pub hover: Hover<T, F>,
}

impl<T, F> ViewTools<T, F> {
    /// The tools, the toolbar set as `toolbar` is, whose handles pick and edit through `link`, the
    /// Inspector's.
    pub fn new(link: InspectorLink, toolbar: Toolbar) -> Self {
        Self {
            camera: OrbitCamera::new(),
            toolbar,
            handles: Handles::default(),
            link,
            shown: None,
            picking: None,
            asked: None,
            hover: Hover::default(),
        }
    }

    /// Has the editor's camera keep its pivot on the plane the grid shows, as
    /// [`OrbitCamera::settle`] says.
    pub fn settle(&mut self) {
        let plane = grid::shown(&self.camera.viewpoint(), self.toolbar.plane);
        self.camera.settle(plane.normal());
    }

    /// Does what `event` does, as the editor reads the window, `reading`, with what the Inspector
    /// has picked, `picked`: first the handles, which take a press that grabs one from the camera,
    /// and while a drag lasts, every other press, the wheel and Home, setting what the drag gives
    /// through the link and sealing its edit as it ends, or as the pick, the world or the
    /// Inspector's steps change; and which have a press off them ask for a pick. Then Q, W, E and
    /// R, which pick a tool; then the camera, which the view goes back to as it takes a press, the
    /// wheel or Home. `Break` for what the handles or the camera took.
    pub fn take(
        &mut self,
        event: &Event<'_>,
        reading: &Reading,
        picked: Option<Picked>,
    ) -> ControlFlow<()> {
        if self.handles.hold(picked.as_ref()) {
            self.link.seal();
        }
        let aim = Aim {
            tool: self.toolbar.tool,
            viewpoint: self.shown,
            picked,
            camera: self.camera.moving().is_some(),
            easing: self.camera.eases(),
        };
        match self.handles.event(event, reading, &aim) {
            // The drag drops a pick asked for before it, whose answer would pick something else
            // under it.
            Reply::Take => {
                self.asked = None;
                return ControlFlow::Break(());
            }
            Reply::Set(entity, field) => {
                self.link
                    .set(entity, "Transform", field.path(), field.value());
            }
            Reply::Seal => self.link.seal(),
            // For the layer, which asks for picks, to ask.
            Reply::Pick(at) => self.picking = Some(at),
            Reply::Pass => {}
        }
        self.choose_tool(event, reading.command);
        let handled = self.camera.event(event, reading.screen, self.toolbar.tool);
        // What the editor's camera takes moves it, from where it was: the view goes back to it.
        if handled.is_break() {
            self.toolbar.world_camera = false;
        }
        handled
    }

    /// Has Q, W, E or R pick its tool, Select, Move, Rotate or Scale, for `event`, a press of it,
    /// not a repeat; but not one with `command` held, Ctrl, or Cmd on macOS, which is a
    /// shortcut's; nor while the view sees through the world's camera, which shows no handles; nor
    /// while the editor's camera flies, as its keys steer it; nor during a drag of the handles.
    pub fn choose_tool(&mut self, event: &Event<'_>, command: bool) {
        let Event::Key {
            key,
            pressed: true,
            repeat: false,
        } = *event
        else {
            return;
        };
        let busy = self.toolbar.world_camera || self.camera.flies() || self.handles.dragging();
        if command || busy {
            return;
        }
        self.toolbar.tool = match key {
            Key::KeyQ => Tool::Select,
            Key::KeyW => Tool::Move,
            Key::KeyE => Tool::Rotate,
            Key::KeyR => Tool::Scale,
            _ => return,
        };
    }

    /// Ends a drag of the handles as the pick, `picked` as the handles see it, changes or dies, the
    /// world is another, the Inspector undoes or redoes, or the view goes to the world's camera,
    /// sealing its edit.
    pub fn hold(&mut self, picked: Option<&Picked>) {
        let ended = self.handles.hold(picked);
        let left = self.toolbar.world_camera && self.handles.end();
        if ended || left {
            self.link.seal();
        }
    }

    /// Paints the handles over `picked`, the pick as they see it, on the view's own layer: over the
    /// world it shows, and under what's over it, as `sight` sees it, while it's through the
    /// editor's camera; lighting the one a press would grab, but while the camera drags or eases,
    /// or with Alt held, as `alt` says, which orbits. And over them, at the view's bottom left,
    /// `name`, what the view shows under the cursor, while the cursor's over the view.
    pub fn paint(
        &self,
        ui: &Ui<'_>,
        sight: Option<Sight>,
        alt: bool,
        picked: Option<&Picked>,
        name: Option<String>,
    ) {
        let Some(view) = ui.view() else {
            return;
        };
        if let Some(sight) = sight {
            let egui = ui.egui();
            let ppp = egui.pixels_per_point();
            let painter = egui.layer_painter(view.layer_id).with_clip_rect(view.rect);
            let camera = &self.camera;
            let hover = view.hover_pos().filter(|_| view.hovered());
            let hover = hover.filter(|_| camera.moving().is_none() && !camera.eases() && !alt);
            let hover = hover.map(|at| Vec2::new(at.x, at.y) * ppp);
            let tool = self.toolbar.tool;
            self.handles
                .paint(&painter, ppp, &sight, tool, picked, hover);
        }
        if let Some(name) = name.filter(|_| view.hovered()) {
            overlay::name_hovered(ui.egui(), &view, name);
        }
    }

    /// Shows the toolbar over the view's top left, whose world-camera switch is off unless
    /// `has_camera`, and while the view is through the editor's camera, the gizmo over its top
    /// right, naming the side the camera looks squarely from, as [`grid::square`] says, in `view`,
    /// where the view is in the main window. Then has the cursor over the view say what the left
    /// button does, and while a drag goes on, what it does, wherever the cursor is.
    pub fn overlay(&mut self, ui: &mut Ui<'_>, has_camera: bool, view: Option<Viewport>) {
        let Self {
            camera, toolbar, ..
        } = self;
        ui.over_view("toolbar", Align2::LEFT_TOP, |ui| {
            toolbar.show(ui, camera, has_camera);
        });
        if !toolbar.world_camera {
            let seen = camera.viewpoint();
            let square = view.is_some_and(|view| grid::square(&seen, view));
            let aligned = square
                .then(|| Side::facing(seen.rotation * Vec3::NEG_Z))
                .flatten();
            ui.over_view("gizmo", Align2::RIGHT_TOP, |ui| {
                overlay::gizmo(ui, camera, aligned);
            });
        }
        let hovered = ui.view().is_some_and(|view| view.hovered());
        let icon = match camera.moving() {
            Some(Move::Pan) => Some(CursorIcon::Move),
            Some(Move::Orbit) => Some(CursorIcon::Grabbing),
            Some(Move::Look) => Some(CursorIcon::Crosshair),
            Some(Move::Dolly) => Some(CursorIcon::ZoomIn),
            None if hovered => match toolbar.tool {
                Tool::Select | Tool::Move | Tool::Rotate | Tool::Scale => None,
                Tool::Pan => Some(CursorIcon::Move),
                Tool::Orbit => Some(CursorIcon::Grab),
            },
            None => None,
        };
        if let Some(icon) = icon {
            ui.egui().set_cursor_icon(icon);
        }
    }
}

/// What the editor reads of the window, as `cx` has it, as an event comes, with the game's view
/// shown at `view`, in the main window's pixels: the scale; whether Alt is held, and the command
/// key, which egui's shortcuts take as it, Ctrl, or Cmd on macOS; and where the cursor is.
pub fn reading(cx: &Context, view: Option<Viewport>) -> Reading {
    let input = cx.input();
    let command = if cfg!(target_os = "macos") {
        [Key::SuperLeft, Key::SuperRight]
    } else {
        [Key::ControlLeft, Key::ControlRight]
    };
    Reading {
        screen: Screen {
            view,
            scale: cx.scale() as f32,
            alt: alt_down(input),
        },
        command: command.into_iter().any(|key| input.key_down(key)),
        cursor: input.cursor(),
    }
}

/// Whether `input` has Alt held, by either of its keys.
pub fn alt_down(input: &Input) -> bool {
    input.key_down(Key::AltLeft) || input.key_down(Key::AltRight)
}

/// What the answers to picks that came back, `answers`, each a pick's ticket and what the view
/// showed there, if anything, have the Inspector pick: once the latest pick asked for, `asked`,
/// has its answer, which forgets it, what it found, or nothing, which unpicks, as an entity no
/// longer alive does; `None` while it waits. The answers to picks that a later press superseded
/// pick nothing.
pub fn answer<T: Copy + PartialEq, F: Clone>(
    asked: &mut Option<T>,
    answers: &[(T, Option<F>)],
) -> Option<Option<F>> {
    let ticket = (*asked)?;
    let (_, found) = answers.iter().find(|(answered, _)| *answered == ticket)?;
    *asked = None;
    Some(found.clone())
}

/// What the view shows under the cursor, as the answers to the picks asked at the cursor while it's
/// over the view come back, each pick's ticket a `T`, and what it finds an `F`: the view's bottom
/// left names what the latest answer found, a few frames behind the cursor.
pub struct Hover<T, F> {
    /// The ticket of the first pick asked since the cursor last came over the view, while it's over
    /// it: the answers to picks asked before it found what was under the cursor before it left.
    since: Option<T>,
    /// The latest answer to a pick asked since then: its ticket, and what it found under the
    /// cursor, if anything. Answers can come back out of the order their picks were asked in, as
    /// one outside the view, answered sooner, does before one inside it asked earlier.
    latest: Option<(T, Option<F>)>,
}

impl<T, F> Default for Hover<T, F> {
    fn default() -> Self {
        Self {
            since: None,
            latest: None,
        }
    }
}

impl<T: Copy + Ord, F: Clone> Hover<T, F> {
    /// Takes `answers`, those that came back with the frame, with the cursor `over` the view or
    /// not. Over it, an answer to a pick asked since it came over the view, later than the latest,
    /// becomes the latest: a press's too, since a press off the handles asks at the cursor as well.
    /// Off it, the picks asked and their answers are forgotten.
    pub fn see(&mut self, over: bool, answers: &[(T, Option<F>)]) {
        let Some(since) = self.since.filter(|_| over) else {
            *self = Self::default();
            return;
        };
        for (pick, found) in answers {
            let later = (self.latest.as_ref()).is_none_or(|(latest, _)| pick > latest);
            if *pick >= since && later {
                self.latest = Some((*pick, found.clone()));
            }
        }
    }

    /// Keeps `asked`, the ticket of a pick at the cursor over the view, if one was asked, for
    /// [`see`](Self::see) to take its answer and those after it.
    pub fn ask(&mut self, asked: Option<T>) {
        self.since = self.since.or(asked);
    }

    /// What the view shows under the cursor, as the latest answer found it.
    pub fn under(&self) -> Option<F> {
        self.latest.as_ref().and_then(|(_, found)| found.clone())
    }
}

#[cfg(test)]
mod tests {
    use gg_engine::EntityId;

    use super::*;

    /// The answer to the latest pick asked for picks what it found, or unpicks for none; answers
    /// to older picks, which a later press superseded, pick nothing; and once answered, a pick is
    /// forgotten.
    #[test]
    fn the_latest_picks_answer_picks() {
        let [a, b]: [EntityId; 2] =
            ["0123456789abcdef", "fedcba9876543210"].map(|id| id.parse().expect("an ID"));
        let mut asked = Some(2_u32);
        assert_eq!(
            answer::<_, EntityId>(&mut asked, &[]),
            None,
            "until it comes"
        );
        assert_eq!(answer(&mut asked, &[(1, Some(a))]), None, "an older pick's");
        assert_eq!(asked, Some(2));
        let answers = [(1, Some(a)), (2, Some(b))];
        assert_eq!(answer(&mut asked, &answers), Some(Some(b)));
        assert_eq!(asked, None);
        assert_eq!(answer(&mut asked, &answers), None, "answered");
        let mut asked = Some(3);
        let unpicked = answer::<_, EntityId>(&mut asked, &[(3, None)]);
        assert_eq!(unpicked, Some(None), "unpicks");
    }

    /// What the view shows under the cursor is what the latest answer found, of those to picks
    /// asked since the cursor came over the view: nothing until one comes back, nor where it found
    /// nothing, and an answer that comes back after a later one's says nothing. Off the view,
    /// nothing is, and back over it, the answers to picks asked before say nothing either.
    #[test]
    fn the_latest_pick_since_the_cursor_came_over_the_view_finds_whats_under_it() {
        let [a, b]: [EntityId; 2] =
            ["0123456789abcdef", "fedcba9876543210"].map(|id| id.parse().expect("an ID"));
        let mut hover = Hover::default();
        hover.see(true, &[(1, Some(a))]);
        assert_eq!(hover.under(), None, "a pick from before it came over");
        hover.ask(Some(2_u32));
        hover.see(true, &[]);
        assert_eq!(hover.under(), None, "until one comes back");
        hover.ask(Some(3));
        hover.see(true, &[(1, Some(a)), (2, Some(b))]);
        assert_eq!(hover.under(), Some(b));
        hover.see(true, &[]);
        assert_eq!(hover.under(), Some(b), "until the next");
        // A pick outside the view, answered sooner, before one inside it asked earlier.
        hover.see(true, &[(5, None)]);
        assert_eq!(hover.under(), None, "nothing there");
        hover.see(true, &[(3, Some(a)), (4, Some(a))]);
        assert_eq!(hover.under(), None, "after a later one's");
        hover.see(true, &[(6, Some(a))]);
        assert_eq!(hover.under(), Some(a));
        hover.see(false, &[(7, Some(b))]);
        assert_eq!(hover.under(), None, "off the view");
        hover.see(true, &[(7, Some(b))]);
        hover.ask(Some(9));
        hover.see(true, &[(8, Some(b))]);
        assert_eq!(hover.under(), None, "asked before it came back over");
        hover.see(true, &[(9, Some(b))]);
        assert_eq!(hover.under(), Some(b));
    }
}

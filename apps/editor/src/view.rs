//! The tools of the editor's view of a project's game: its camera; the toolbar; the handles, which
//! pick and edit through the Inspector's link; the pick a press off them asks for, and what the
//! view shows under the cursor; and what they paint over the view and show over it. What depends
//! on the world the view shows, the layer gives them: what the Inspector has picked, as the handles
//! see it; the picks, which it asks for and whose answers it hands them; and what's under the
//! cursor called.

use std::ops::ControlFlow;

use gg_engine::egui::{self, Align2, CursorIcon};
use gg_engine::grid;
use gg_engine::math::{DVec3, Vec2, Vec3, Vec4};
use gg_engine::{Context, Event, Input, InspectorLink, Key, Ui, Viewpoint, Viewport};

use crate::camera::{Move, OrbitCamera, Screen, Side, Tool};
use crate::handles::{Aim, Along, Handles, Picked, Reading, Reply, Sight};
use crate::overlay::{self, Toolbar};
use crate::shortcuts::{Does, SHORTCUTS, Shortcut};

/// The color of the outline round what's picked, in linear RGBA: an orange, as other editors
/// outline what's selected in.
pub const OUTLINE: Vec4 = Vec4::new(1.0, 0.35, 0.02, 1.0);

/// How wide the outline's solid band round what's picked is, in points, its glow reaching as far
/// again: wide enough to read as a selection over a busy scene.
pub const OUTLINE_WIDTH: f32 = 3.0;

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
    /// pixels, until the layer asks for it, keeping the ticket in `asked`; and whether Ctrl (Cmd
    /// on macOS) or Shift was held at it, for its answer to add what it finds to what's picked, or
    /// take it out, rather than pick it alone.
    pub picking: Option<([f64; 2], bool)>,
    /// Where the last press over the view with the Probe tool asked to probe, in the main window's
    /// pixels, until the layer asks for it.
    pub probing: Option<[f64; 2]>,
    /// The ticket of the latest pick asked for, until its answer comes back, which the layer has
    /// the Inspector pick, as [`answer`] says: a later press's takes its place, and a grab of a
    /// handle drops it, so that it picks nothing else under the drag. And whether it toggles, as
    /// its press's `picking` said.
    pub asked: Option<T>,
    pub toggles: bool,
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
            probing: None,
            asked: None,
            toggles: false,
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
    /// has picked, `picked`, the active one, and `along`, the others a drag moves with it: first
    /// the handles, which take a press that grabs one from the camera, and while a drag lasts,
    /// every other press, the wheel and Home, setting what the drag gives through the link and
    /// sealing its edit as it ends, or as the pick, the world or the Inspector's steps change; and
    /// which have a press off them ask for a pick, which adds what it finds to what's picked, or
    /// takes it out, with Ctrl (Cmd on macOS) or Shift held. Then Q, W, E and R, which pick a tool,
    /// and X, which flips the handles' space; then F, which frames what's picked, as
    /// [`frame`](Self::frame) does; then the camera, which the view goes back to as it takes a
    /// press, the wheel or Home. `Break` for what the handles, F or the camera took.
    pub fn take(
        &mut self,
        event: &Event<'_>,
        reading: &Reading,
        picked: Option<Picked>,
        along: &[Along],
    ) -> ControlFlow<()> {
        if self.handles.hold(picked.as_ref()) {
            self.link.seal();
        }
        let aim = Aim {
            tool: self.toolbar.tool,
            space: self.toolbar.space,
            viewpoint: self.shown,
            picked,
            along,
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
            Reply::Set(..) => {
                for (entity, field) in self.handles.sets() {
                    self.link
                        .set(entity, "Transform", field.path(), field.value());
                }
            }
            Reply::Seal => self.link.seal(),
            // For the layer, which asks for picks, to ask.
            Reply::Pick(at) => self.picking = Some((at, reading.command || reading.shift)),
            // As a pick's, and the camera doesn't take it.
            Reply::Probe(at) => {
                self.probing = Some(at);
                return ControlFlow::Break(());
            }
            Reply::Pass => {}
        }
        self.choose_tool(event, reading.command);
        if let Some(picked) = picked.filter(|_| frames(event, reading.command))
            && !self.handles.dragging()
        {
            self.frame(&picked);
            return ControlFlow::Break(());
        }
        let handled = self.camera.event(event, reading.screen, self.toolbar.tool);
        // What the editor's camera takes moves it, from where it was: the view goes back to it.
        if handled.is_break() {
            self.toolbar.world_camera = false;
        }
        handled
    }

    /// Has Q, W, E or R pick its tool, Select, Move, Rotate or Scale, and X flip the space of
    /// Move's and Rotate's axes between the entity's own and the world's, as [`SHORTCUTS`] has
    /// them, for `event`, a press of it, not a repeat; but not one with `command` held, Ctrl, or Cmd
    /// on macOS, which is a shortcut's; nor while the view sees through the game's camera, which
    /// shows no handles; nor while the editor's camera flies, as its keys steer it; nor during a
    /// drag of the handles.
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
        let view = SHORTCUTS
            .iter()
            .find(|shortcut| shortcut.key == key && !shortcut.command);
        match view.map(|shortcut| shortcut.does) {
            Some(Does::Tool(tool)) => self.toolbar.tool = tool,
            Some(Does::FlipSpace) => self.toolbar.space = self.toolbar.space.flipped(),
            _ => {}
        }
    }

    /// Eases the editor's camera to look at what's picked, `picked` the active one, the view going
    /// back to it, as F over the view and the Entities panel ask: at a sphere round the spheres
    /// round what each entity picked and those under it draw, and the origins of those that draw
    /// nothing, as the game's last reads of them gave them, filling the view, as
    /// [`OrbitCamera::frame_sphere`] says; or, where that's a point, as for one entity that draws
    /// nothing, at it, as [`OrbitCamera::frame`] says. The active one alone until the reads of the
    /// others have come, as [`InspectorLink::along`] says.
    pub fn frame(&mut self, picked: &Picked) {
        let placed =
            (self.link.along()).unwrap_or_else(|| self.link.placed().into_iter().collect());
        let origin = |local: &gg_engine::Transform, parent: &gg_engine::Pose| {
            (parent.matrix).transform_point3(local.translation)
        };
        let spheres = placed
            .iter()
            .map(|placed| (placed.bounds).unwrap_or((origin(&placed.local, &placed.space), 0.0)));
        self.frame_bounds(picked, spheres.reduce(enclosing));
    }

    /// Frames `picked` by `bounds`, a sphere's center and radius, as [`frame`](Self::frame) finds
    /// it: by the sphere where it has a radius; else at its center, or for none at the pick's
    /// origin, from as far as the camera is.
    fn frame_bounds(&mut self, picked: &Picked, bounds: Option<(DVec3, f32)>) {
        let origin = (picked.parent.matrix).transform_point3(picked.local.translation);
        match bounds {
            Some((center, radius)) if radius > 0.0 => self.camera.frame_sphere(center, radius),
            Some((center, _)) => self.camera.frame(center),
            None => self.camera.frame(origin),
        }
        self.toolbar.world_camera = false;
    }

    /// Ends a drag of the handles as the pick, `picked` as the handles see it, changes or dies, the
    /// world is another, the Inspector undoes or redoes, or the view goes to the game's camera,
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
            let shown = (self.toolbar.tool, self.toolbar.space);
            self.handles
                .paint(&painter, ppp, &sight, shown, picked, hover);
        }
        if let Some(name) = name.filter(|_| view.hovered()) {
            overlay::name_hovered(ui.egui(), &view, name);
        }
    }

    /// Shows the toolbar over the view's top left, or where its grip has moved it, and while the
    /// view is through the editor's camera, the gizmo over its top right, naming the side the
    /// camera looks squarely from, as [`grid::square`] says, in `view`, where the view is in the
    /// main window. Then has the cursor over the view say what the left button does, and while a
    /// drag goes on, what it does, wherever the cursor is.
    pub fn overlay(&mut self, ui: &mut Ui<'_>, view: Option<Viewport>) {
        let Self {
            camera, toolbar, ..
        } = self;
        let offset =
            (ui.view()).map_or(egui::Vec2::ZERO, |view| toolbar.place(ui.egui(), view.rect));
        ui.over_view("toolbar", Align2::LEFT_TOP, offset, |ui| {
            toolbar.show(ui, camera);
        });
        if !toolbar.world_camera {
            let seen = camera.viewpoint();
            let square = view.is_some_and(|view| grid::square(&seen, view));
            let aligned = square
                .then(|| Side::facing(seen.rotation * Vec3::NEG_Z))
                .flatten();
            ui.over_view("gizmo", Align2::RIGHT_TOP, egui::Vec2::ZERO, |ui| {
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
                Tool::Probe => Some(CursorIcon::Crosshair),
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
/// shown at `view`, in the main window's pixels: the scale; whether Alt is held, the command key,
/// which egui's shortcuts take as it, Ctrl, or Cmd on macOS, and Shift; and where the cursor is.
pub fn reading(cx: &Context, view: Option<Viewport>) -> Reading {
    let input = cx.input();
    let command = if cfg!(target_os = "macos") {
        [Key::SuperLeft, Key::SuperRight]
    } else {
        [Key::ControlLeft, Key::ControlRight]
    };
    let down = |keys: [Key; 2]| keys.into_iter().any(|key| input.key_down(key));
    Reading {
        screen: Screen {
            view,
            scale: cx.scale() as f32,
            alt: alt_down(input),
        },
        command: down(command),
        shift: down([Key::ShiftLeft, Key::ShiftRight]),
        cursor: input.cursor(),
    }
}

/// A sphere round spheres `a` and `b`, each a center and a radius: the smallest, the larger of them
/// where it holds the other.
fn enclosing(a: (DVec3, f32), b: (DVec3, f32)) -> (DVec3, f32) {
    let ((a, r), (b, s)) = (a, b);
    let (r, s) = (f64::from(r), f64::from(s));
    let apart = a.distance(b);
    if apart + s <= r {
        return (a, r as f32);
    }
    if apart + r <= s {
        return (b, s as f32);
    }
    let radius = (apart + r + s) / 2.0;
    (a + (b - a) * ((radius - r) / apart), radius as f32)
}

/// Whether `event` is a press of F, as [`SHORTCUTS`] has it, not a repeat, without `command` held,
/// which a shortcut's would have: as other editors frame what's selected.
fn frames(event: &Event<'_>, command: bool) -> bool {
    let Event::Key {
        key,
        pressed: true,
        repeat: false,
    } = *event
    else {
        return false;
    };
    let frame = |shortcut: &Shortcut| shortcut.does == Does::Frame && shortcut.key == key;
    SHORTCUTS.iter().any(frame) && !command
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
    use std::time::Duration;

    use std::f32::consts::FRAC_PI_4;

    use gg_engine::math::{DAffine3, Quat};
    use gg_engine::reflect::Value;
    use gg_engine::{EntityId, Inspector, MouseButton, Pick, Pose, Projection, Scroll, Transform};

    use super::*;
    use crate::camera::Space;
    use crate::tests::reading;

    /// The view's tools as a test has them, with the engine's picks.
    type Tools = ViewTools<Pick, EntityId>;

    /// The tools, linked to an Inspector of their own, the toolbar as it starts.
    fn fresh() -> Tools {
        ViewTools::new(Inspector::default().link(), Toolbar::default())
    }

    /// On Move's X arrow, as the editor's camera starts, for an entity at the origin: 105 of its
    /// 150 pixels, 70 points, right of the middle.
    const ON_X: [f64; 2] = [605.0, 350.0];

    /// An entity at the origin, unturned, at its own size, as the Inspector has picked it.
    fn at_the_origin() -> Option<Picked> {
        Some(Picked {
            entity: "0123456789abcdef".parse().expect("an ID"),
            serial: 1,
            local: Transform::IDENTITY,
            parent: Pose::IDENTITY,
            stepped: 0,
            waiting: false,
            moves: true,
        })
    }

    /// What `tools` do with the cursor moved to `at`, and then with `button` pressed or let go
    /// there, as the editor reads the window there, with `picked` picked.
    fn press_at(
        tools: &mut Tools,
        picked: Option<Picked>,
        at: [f64; 2],
        button: MouseButton,
        pressed: bool,
    ) -> ControlFlow<()> {
        let moved = Event::CursorMoved { x: at[0], y: at[1] };
        let _ = tools.take(&moved, &reading(at), picked, &[]);
        let event = Event::MouseButton { button, pressed };
        tools.take(&event, &reading(at), picked, &[])
    }

    /// Tools dragging Move's X arrow of `at_the_origin`, through the editor's camera as it starts.
    fn dragging() -> Tools {
        let mut tools = fresh();
        tools.toolbar.tool = Tool::Move;
        tools.shown = Some(tools.camera.viewpoint());
        let pressed = press_at(&mut tools, at_the_origin(), ON_X, MouseButton::Left, true);
        assert_eq!(pressed, ControlFlow::Break(()));
        assert!(tools.handles.dragging());
        tools
    }

    /// A drag of a handle sets one field of the picked entity through the Inspector's link, the
    /// latest value of it alone between the Inspector's passes, and seals the edit as it ends; the
    /// camera sees the moves: here Move's X arrow, dragged right along it, and off it a little.
    #[test]
    fn a_drag_of_a_handle_sets_one_field_of_the_pick_and_seals() {
        let mut tools = dragging();
        let picked = at_the_origin();
        for x in [620.0, 650.0, 680.0] {
            let moved = Event::CursorMoved { x, y: 360.0 };
            let handled = tools.take(&moved, &reading([x, 360.0]), picked, &[]);
            assert_eq!(handled, ControlFlow::Continue(()));
        }
        let at = [680.0, 360.0];
        let _ = press_at(&mut tools, picked, at, MouseButton::Left, false);
        assert!(!tools.handles.dragging());
        let pending = tools.link.pending();
        let [Some((id, component, path, Value::DVec3(moved))), None] = &pending[..] else {
            panic!("a set and a seal: {pending:?}");
        };
        let entity = picked.expect("picked").entity;
        let field = (*id, component.as_str(), path.as_str());
        assert_eq!(field, (entity, "Transform", "translation"));
        // 75 pixels right, at 60 a unit.
        assert!(
            moved.abs_diff_eq(DVec3::new(1.25, 0.0, 0.0), 1e-4),
            "{moved}"
        );
        assert_eq!(tools.picking, None, "it picked nothing");
    }

    /// While a drag of the handles lasts, the tools take the other buttons' presses, the wheel and
    /// Home from the editor's camera, which neither drags, dollies nor goes home; and while the
    /// camera drags, a press on a handle grabs nothing, and asks for no pick.
    #[test]
    fn the_handles_and_the_camera_keep_out_of_each_others_drags() {
        let mut tools = dragging();
        let picked = at_the_origin();
        // Away from where it starts, which Home would ease it back to.
        tools.camera.orbit(Vec2::new(40.0, 20.0));
        let seen = tools.camera.viewpoint();
        let press = |button| Event::MouseButton {
            button,
            pressed: true,
        };
        let home = Event::Key {
            key: Key::Home,
            pressed: true,
            repeat: false,
        };
        let wheel = Event::Scroll(Scroll::Lines { x: 0.0, y: 2.0 });
        for event in [
            press(MouseButton::Middle),
            press(MouseButton::Right),
            wheel,
            home,
        ] {
            let handled = tools.take(&event, &reading(ON_X), picked, &[]);
            assert_eq!(handled, ControlFlow::Break(()), "{event:?}");
        }
        tools.camera.update(Duration::from_secs(1));
        assert_eq!(tools.camera.moving(), None);
        assert_eq!(tools.camera.viewpoint(), seen);
        let _ = press_at(&mut tools, picked, ON_X, MouseButton::Left, false);
        let panning = press_at(&mut tools, picked, ON_X, MouseButton::Middle, true);
        assert_eq!(panning, ControlFlow::Break(()));
        assert_eq!(tools.camera.moving(), Some(Move::Pan));
        let left = press_at(&mut tools, picked, ON_X, MouseButton::Left, true);
        assert_eq!(left, ControlFlow::Continue(()));
        assert!(!tools.handles.dragging());
        assert_eq!(tools.picking, None);
    }

    /// F frames the pick, eased to look at its origin as it has no sphere, turning the view to the
    /// editor's camera from the game's: not without a pick, as it repeats, with Ctrl held, nor
    /// during a drag of the handles.
    #[test]
    fn f_frames_the_pick_turning_the_view_to_the_editors_camera() {
        let f = |repeat| Event::Key {
            key: Key::KeyF,
            pressed: true,
            repeat,
        };
        let mut placed = at_the_origin().expect("picked");
        placed.local.translation = DVec3::new(2.0, 0.0, 0.0);
        placed.parent.matrix = DAffine3::from_translation(DVec3::new(0.0, 3.0, 0.0));
        let mut tools = fresh();
        let seen = tools.camera.viewpoint();
        let mut ctrl = reading([0.0, 0.0]);
        ctrl.command = true;
        for (event, reading, picked) in [
            (f(false), reading([0.0, 0.0]), None),
            (f(true), reading([0.0, 0.0]), Some(placed)),
            (f(false), ctrl, Some(placed)),
        ] {
            let _ = tools.take(&event, &reading, picked, &[]);
            assert!(!tools.camera.eases(), "{event:?}");
        }
        let mut dragging = dragging();
        let _ = dragging.take(&f(false), &reading(ON_X), Some(placed), &[]);
        assert!(!dragging.camera.eases(), "dragging");
        tools.toolbar.world_camera = true;
        let handled = tools.take(&f(false), &reading([0.0, 0.0]), Some(placed), &[]);
        assert_eq!(handled, ControlFlow::Break(()));
        assert!(!tools.toolbar.world_camera, "the editor's camera");
        tools.camera.update(Duration::from_secs(1));
        let framed = tools.camera.viewpoint();
        assert_eq!(framed.rotation, seen.rotation);
        let ahead = (framed.rotation * Vec3::NEG_Z).as_dvec3();
        let looked_at =
            framed.position + ahead * (DVec3::new(2.0, 3.0, 0.0) - framed.position).dot(ahead);
        assert!(
            looked_at.abs_diff_eq(DVec3::new(2.0, 3.0, 0.0), 1e-4),
            "{looked_at}"
        );
    }

    /// A press off the handles with Ctrl or Shift held asks for a pick that adds what it finds to
    /// what's picked, or takes it out; one without them, for one that picks it alone.
    #[test]
    fn a_press_with_ctrl_or_shift_asks_for_a_pick_that_toggles() {
        let off = [200.0, 500.0];
        for (command, shift) in [(false, false), (true, false), (false, true)] {
            let mut tools = fresh();
            tools.shown = Some(tools.camera.viewpoint());
            let held = Reading {
                command,
                shift,
                ..reading(off)
            };
            let moved = Event::CursorMoved {
                x: off[0],
                y: off[1],
            };
            let _ = tools.take(&moved, &held, at_the_origin(), &[]);
            let press = Event::MouseButton {
                button: MouseButton::Left,
                pressed: true,
            };
            let _ = tools.take(&press, &held, at_the_origin(), &[]);
            assert_eq!(tools.picking, Some((off, command || shift)));
        }
    }

    /// The sphere framing several picks frames holds the spheres round each: the smallest that
    /// holds two apart, the larger of two where it holds the other, and of two points the one
    /// round both.
    #[test]
    fn the_sphere_round_two_holds_both() {
        let (center, radius) = enclosing((DVec3::ZERO, 1.0), (DVec3::new(4.0, 0.0, 0.0), 1.0));
        assert!(
            center.abs_diff_eq(DVec3::new(2.0, 0.0, 0.0), 1e-6),
            "{center}"
        );
        assert!((radius - 3.0).abs() < 1e-6, "{radius}");
        for (a, b) in [
            ((DVec3::ZERO, 5.0), (DVec3::X, 1.0)),
            ((DVec3::X, 1.0), (DVec3::ZERO, 5.0)),
        ] {
            assert_eq!(enclosing(a, b), (DVec3::ZERO, 5.0));
        }
        let (center, radius) = enclosing((DVec3::ZERO, 0.0), (DVec3::new(0.0, 2.0, 0.0), 0.0));
        assert!(center.abs_diff_eq(DVec3::Y, 1e-6) && (radius - 1.0).abs() < 1e-6);
    }

    /// Framing a pick the game gave a sphere for centers the view on the sphere, not on the pick's
    /// origin, and goes back far enough to fill the view with it, and turns the view to the
    /// editor's camera; one without a sphere is framed at its origin, as it was.
    #[test]
    fn framing_a_pick_with_a_sphere_looks_at_the_sphere() {
        let placed = at_the_origin().expect("picked");
        let center = DVec3::new(7.0, 8.0, 9.0);
        let mut tools = fresh();
        tools.toolbar.world_camera = true;
        tools.frame_bounds(&placed, Some((center, 2.0)));
        assert!(!tools.toolbar.world_camera, "the editor's camera");
        tools.camera.update(Duration::from_secs(1));
        let framed = tools.camera.viewpoint();
        let ahead = (framed.rotation * Vec3::NEG_Z).as_dvec3();
        let looked_at = framed.position + ahead * (center - framed.position).dot(ahead);
        assert!(looked_at.abs_diff_eq(center, 1e-4), "{looked_at}");
        let Projection::Orthographic { height, .. } = framed.projection else {
            panic!("the camera starts orthographic");
        };
        assert!((height - 5.0).abs() < 1e-4, "2 * 2 / 0.8, not {height}");
        tools.frame_bounds(&placed, None);
        tools.camera.update(Duration::from_secs(1));
        let at_origin = tools.camera.viewpoint();
        let ahead = (at_origin.rotation * Vec3::NEG_Z).as_dvec3();
        let looked_at = at_origin.position + ahead * (-at_origin.position).dot(ahead);
        assert!(looked_at.abs_diff_eq(DVec3::ZERO, 1e-4), "{looked_at}");
        assert_eq!(at_origin.projection, framed.projection, "as far back");
    }

    /// Q, W, E and R pick Select, Move, Rotate and Scale as they're pressed: not as they repeat,
    /// with Ctrl held, through the game's camera, while the editor's camera flies, nor during a
    /// drag of the handles.
    #[test]
    fn q_w_e_and_r_pick_the_tools() {
        let mut tools = fresh();
        let key = |key, repeat| Event::Key {
            key,
            pressed: true,
            repeat,
        };
        for (pressed, tool) in [
            (Key::KeyW, Tool::Move),
            (Key::KeyE, Tool::Rotate),
            (Key::KeyR, Tool::Scale),
            (Key::KeyQ, Tool::Select),
        ] {
            tools.choose_tool(&key(pressed, false), false);
            assert_eq!(tools.toolbar.tool, tool);
        }
        tools.choose_tool(&key(Key::KeyW, true), false);
        tools.choose_tool(&key(Key::KeyW, false), true);
        assert_eq!(tools.toolbar.tool, Tool::Select, "repeating, or with Ctrl");
        tools.toolbar.world_camera = true;
        tools.choose_tool(&key(Key::KeyW, false), false);
        assert_eq!(
            tools.toolbar.tool,
            Tool::Select,
            "through the game's camera"
        );
        tools.toolbar.world_camera = false;
        let right = |pressed| Event::MouseButton {
            button: MouseButton::Right,
            pressed,
        };
        let screen = reading(ON_X).screen;
        let _ = tools.camera.event(&right(true), screen, Tool::Select);
        assert!(tools.camera.flies());
        tools.choose_tool(&key(Key::KeyE, false), false);
        assert_eq!(tools.toolbar.tool, Tool::Select, "flying");
        let mut tools = dragging();
        let _ = tools.take(&key(Key::KeyR, false), &reading(ON_X), at_the_origin(), &[]);
        assert_eq!(tools.toolbar.tool, Tool::Move, "dragging");
        let _ = press_at(&mut tools, at_the_origin(), ON_X, MouseButton::Left, false);
        let _ = tools.take(&key(Key::KeyR, false), &reading(ON_X), at_the_origin(), &[]);
        assert_eq!(tools.toolbar.tool, Tool::Scale, "let go");
    }

    /// X flips the space of Move's and Rotate's axes, Local and World, as it's pressed: not as it
    /// repeats, with Ctrl held, through the game's camera, while the editor's camera flies, nor
    /// during a drag of the handles; and it starts Local.
    #[test]
    fn x_flips_the_handles_space() {
        let mut tools = fresh();
        let key = |repeat| Event::Key {
            key: Key::KeyX,
            pressed: true,
            repeat,
        };
        assert_eq!(tools.toolbar.space, Space::Local, "as it starts");
        tools.choose_tool(&key(false), false);
        assert_eq!(tools.toolbar.space, Space::World);
        tools.choose_tool(&key(true), false);
        tools.choose_tool(&key(false), true);
        assert_eq!(tools.toolbar.space, Space::World, "repeating, or with Ctrl");
        tools.choose_tool(&key(false), false);
        assert_eq!(tools.toolbar.space, Space::Local, "and back");
        tools.toolbar.world_camera = true;
        tools.choose_tool(&key(false), false);
        assert_eq!(
            tools.toolbar.space,
            Space::Local,
            "through the game's camera"
        );
        tools.toolbar.world_camera = false;
        let right = |pressed| Event::MouseButton {
            button: MouseButton::Right,
            pressed,
        };
        let screen = reading(ON_X).screen;
        let _ = tools.camera.event(&right(true), screen, Tool::Select);
        assert!(tools.camera.flies());
        tools.choose_tool(&key(false), false);
        assert_eq!(tools.toolbar.space, Space::Local, "flying");
        let mut tools = dragging();
        let _ = tools.take(&key(false), &reading(ON_X), at_the_origin(), &[]);
        assert_eq!(tools.toolbar.space, Space::Local, "dragging");
        let _ = press_at(&mut tools, at_the_origin(), ON_X, MouseButton::Left, false);
        let _ = tools.take(&key(false), &reading(ON_X), at_the_origin(), &[]);
        assert_eq!(tools.toolbar.space, Space::World, "let go");
    }

    /// For an entity turned 45° about Z, a press on the world's X arrow, where the unturned
    /// entity's is, grabs Move's X arrow in the World space, which a drag moves along the world's
    /// X; in the Local space it grabs nothing, since the arrow is turned off it: through the tools
    /// as the editor feeds them, the toolbar's space the one they go by.
    #[test]
    fn the_toolbars_space_sets_the_axes_the_handles_go_along() {
        let mut turned = at_the_origin().expect("picked");
        turned.local.rotation = Quat::from_rotation_z(FRAC_PI_4);
        let picked = Some(turned);
        let mut tools = fresh();
        tools.toolbar.tool = Tool::Move;
        tools.shown = Some(tools.camera.viewpoint());
        let pressed = press_at(&mut tools, picked, ON_X, MouseButton::Left, true);
        assert_eq!(pressed, ControlFlow::Continue(()), "Local");
        assert!(!tools.handles.dragging());
        let _ = press_at(&mut tools, picked, ON_X, MouseButton::Left, false);
        tools.toolbar.space = Space::World;
        let pressed = press_at(&mut tools, picked, ON_X, MouseButton::Left, true);
        assert_eq!(pressed, ControlFlow::Break(()), "World");
        let to = [ON_X[0] + 40.0, ON_X[1] + 25.0];
        let moved = Event::CursorMoved { x: to[0], y: to[1] };
        let _ = tools.take(&moved, &reading(to), picked, &[]);
        let pending = tools.link.pending();
        let [Some((_, _, _, Value::DVec3(moved)))] = &pending[..] else {
            panic!("a set: {pending:?}");
        };
        assert!(moved.x > 0.5 && moved.y == 0.0 && moved.z == 0.0, "{moved}");
    }

    /// A press of the left button over the view with Probe asks for a probe at the cursor, and
    /// takes it from the camera, picking nothing.
    #[test]
    fn a_press_with_probe_asks_for_a_probe() {
        let off = [200.0, 500.0];
        let mut tools = fresh();
        tools.toolbar.tool = Tool::Probe;
        tools.shown = Some(tools.camera.viewpoint());
        let pressed = press_at(&mut tools, at_the_origin(), off, MouseButton::Left, true);
        assert_eq!(pressed, ControlFlow::Break(()));
        assert_eq!((tools.probing, tools.picking), (Some(off), None));
    }

    /// A press of the left button over the view off the handles asks for a pick at the cursor,
    /// with Select, Move, Rotate and Scale, through the game's camera too, which it leaves the
    /// view seeing through; but not with Alt, which orbits, with Pan or Orbit, or with nothing
    /// under it to pick, as a press on a handle has. A grab of a handle drops the pick asked
    /// before it.
    #[test]
    fn a_press_off_the_handles_asks_for_a_pick() {
        let off = [200.0, 500.0];
        for tool in [Tool::Select, Tool::Move, Tool::Rotate, Tool::Scale] {
            let mut tools = fresh();
            tools.toolbar.tool = tool;
            tools.shown = Some(tools.camera.viewpoint());
            let pressed = press_at(&mut tools, at_the_origin(), off, MouseButton::Left, true);
            assert_eq!(pressed, ControlFlow::Continue(()), "{tool:?}");
            assert_eq!(tools.picking, Some((off, false)), "{tool:?}");
        }
        let mut tools = fresh();
        tools.toolbar.world_camera = true;
        let pressed = press_at(&mut tools, at_the_origin(), ON_X, MouseButton::Left, true);
        assert_eq!(pressed, ControlFlow::Continue(()));
        assert_eq!(tools.picking, Some((ON_X, false)));
        assert!(tools.toolbar.world_camera);
        for tool in [Tool::Pan, Tool::Orbit] {
            let mut tools = fresh();
            tools.toolbar.tool = tool;
            let _ = press_at(&mut tools, at_the_origin(), off, MouseButton::Left, true);
            assert_eq!(tools.picking, None, "{tool:?}");
        }
        let mut tools = fresh();
        let alt = Event::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        };
        let mut held = reading(off);
        held.screen.alt = true;
        let _ = tools.take(&alt, &held, at_the_origin(), &[]);
        assert_eq!(tools.picking, None, "with Alt");
        let tools = dragging();
        assert_eq!(tools.picking, None, "on a handle");
        let mut tools = fresh();
        tools.toolbar.tool = Tool::Move;
        tools.shown = Some(tools.camera.viewpoint());
        tools.asked = Context::seeded(0).pick([0.0, 0.0]);
        assert!(tools.asked.is_some());
        let grabbed = press_at(&mut tools, at_the_origin(), ON_X, MouseButton::Left, true);
        assert_eq!(grabbed, ControlFlow::Break(()));
        assert_eq!(tools.asked, None, "a grab drops the pick asked before it");
    }

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

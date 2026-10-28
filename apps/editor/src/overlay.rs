//! What the editor draws over its view: a toolbar at the view's top left, which picks what the
//! left button does, the projection and the grid, and plays the world and stops it; a gizmo at its
//! top right, which shows which way
//! the camera faces, and turns it; at its bottom left, the name of what the view shows under the
//! cursor; and round its edges, while a scene from the Assets panel is held over it, an outline.

use std::f32::consts::{PI, TAU};

use gg_engine::egui::{
    self, Align2, Atom, Button, Color32, CursorIcon, FontId, Frame, Id, Margin, Painter, Pos2,
    Rect, Response, Rgba, Sense, Shape, Stroke, StrokeKind, TextStyle, Visuals, vec2,
};
use gg_engine::math::{self, Quat, libm};

use crate::camera::{OrbitCamera, Side, Tool};
use crate::grid::{self, Plane};

/// The side of a toolbar icon, in points.
const ICON: f32 = 16.0;
/// How far the gizmo's balls are from its middle, in points.
const REACH: f32 = 34.0;
/// The radius of the gizmo's balls, in points.
const BALL: f32 = 8.0;
/// Half the side of the gizmo's cube, in points.
const CUBE: f32 = 10.0;
/// The side of the gizmo, in points.
const GIZMO: f32 = 2.0 * (REACH + BALL) + 4.0;
/// The height of the label under the gizmo, in points.
const LABEL: f32 = 18.0;
/// How far in from the view's edges the name of what's under the cursor shows, in points: as far
/// as [`Ui::over_view`](gg_engine::Ui::over_view) places the toolbar and the gizmo.
const INSET: f32 = 8.0;

/// What the toolbar sets: what the left button does over the view, whether the grid shows, and in
/// which plane, whether the view sees through the world's camera, and whether the world plays.
pub struct Toolbar {
    pub tool: Tool,
    pub grid: bool,
    pub plane: Plane,
    /// Whether the view sees through the world's camera, as the game would, rather than the
    /// editor's, and shows neither the grid nor the gizmo.
    pub world_camera: bool,
    /// Whether the world plays, as the game would: a copy of it, in its place until it stops.
    pub play: bool,
}

impl Default for Toolbar {
    fn default() -> Self {
        Self {
            tool: Tool::default(),
            grid: true,
            plane: Plane::XY,
            world_camera: false,
            play: false,
        }
    }
}

impl Toolbar {
    /// Draws the toolbar into `ui`: the tools, whose tooltips name the keys that pick them, the
    /// camera's projection, which a click switches, the grid's switch and its plane, the switch
    /// that sees through the world's camera, which is off while the world has no primary camera,
    /// as `has_camera` says, and Play, which plays the world and, lit, stops it, each with a
    /// tooltip. While the view sees through the world's camera, the projection's and the grid's
    /// buttons are off, since what they change doesn't show. It opens nothing, since a popup would
    /// take the view's keys.
    pub fn show(&mut self, ui: &mut egui::Ui, camera: &mut OrbitCamera, has_camera: bool) {
        // The key that snaps a drag of the handles, which egui's shortcuts take as the command key.
        let snaps = if cfg!(target_os = "macos") {
            "Cmd"
        } else {
            "Ctrl"
        };
        panel(ui.visuals()).show(ui, |ui| {
            ui.horizontal(|ui| {
                for (tool, icon, tip) in [
                    (
                        Tool::Select,
                        Icon::Select,
                        "Select (Q): the left button picks what the view shows there".to_owned(),
                    ),
                    (
                        Tool::Move,
                        Icon::Move,
                        format!(
                            "Move (W): the handles move what's picked along its axes, across its \
                             planes, or in the view's plane; {snaps} snaps to half units"
                        ),
                    ),
                    (
                        Tool::Rotate,
                        Icon::Rotate,
                        format!(
                            "Rotate (E): the rings turn what's picked about its axes, and the \
                             outer one about the view's; {snaps} snaps to 45°"
                        ),
                    ),
                    (
                        Tool::Scale,
                        Icon::Scale,
                        format!(
                            "Scale (R): the handles scale what's picked along its axes, and the \
                             middle one all three ways; {snaps} snaps to halves"
                        ),
                    ),
                    (
                        Tool::Pan,
                        Icon::Pan,
                        "Pan: the left button trucks and pedestals, as the middle button does"
                            .to_owned(),
                    ),
                    (
                        Tool::Orbit,
                        Icon::Orbit,
                        "Orbit: the left button turns the view about its pivot, as Alt and the \
                         left button do"
                            .to_owned(),
                    ),
                ] {
                    if icon_button(ui, icon, self.tool == tool)
                        .on_hover_text(tip)
                        .clicked()
                    {
                        self.tool = tool;
                    }
                }
                ui.separator();
                ui.add_enabled_ui(!self.world_camera, |ui| {
                    let (label, tip) = if camera.perspective {
                        ("Persp", "Perspective: click for orthographic")
                    } else {
                        ("Ortho", "Orthographic: click for perspective")
                    };
                    if ui.button(label).on_hover_text(tip).clicked() {
                        camera.perspective = !camera.perspective;
                    }
                    ui.separator();
                    if icon_button(ui, Icon::Grid, self.grid)
                        .on_hover_text("Grid")
                        .clicked()
                    {
                        self.grid = !self.grid;
                    }
                    let tip = "The grid's plane; a view along an axis without perspective shows \
                               the plane facing it";
                    for (plane, name) in [(Plane::XY, "XY"), (Plane::XZ, "XZ"), (Plane::YZ, "YZ")] {
                        let button = Button::selectable(self.plane == plane, name);
                        if ui.add(button).on_hover_text(tip).clicked() {
                            self.plane = plane;
                        }
                    }
                });
                ui.separator();
                let through = ui.add_enabled_ui(has_camera, |ui| {
                    icon_button(ui, Icon::Camera, self.world_camera)
                });
                let through = through
                    .inner
                    .on_hover_text(
                        "World camera: the view sees through the world's primary camera, as the \
                         game does, and while the world plays, W, A, S and D drive the world's \
                         cameras; moving the editor's camera comes back to it",
                    )
                    .on_disabled_hover_text("World camera: the world has no primary camera");
                if through.clicked() {
                    self.world_camera = !self.world_camera;
                }
                ui.separator();
                let tip = if self.play {
                    "Stop: brings back the world as it was before Play, with its edits' history"
                } else {
                    "Play: a copy of the world plays in its place, as the game would, its scripts \
                     and physics running and the view seeing through its camera, until Stop; \
                     what's edited meanwhile goes with the copy"
                };
                let icon = if self.play { Icon::Stop } else { Icon::Play };
                if icon_button(ui, icon, self.play)
                    .on_hover_text(tip)
                    .clicked()
                {
                    self.play = !self.play;
                }
            });
        });
    }
}

/// A frame for what floats over the view, as a window's.
fn panel(visuals: &Visuals) -> Frame {
    Frame::NONE
        .fill(visuals.window_fill)
        .stroke(visuals.window_stroke)
        .corner_radius(6)
        .inner_margin(Margin::same(3))
}

/// A button showing `icon`, lit while `selected`.
fn icon_button(ui: &mut egui::Ui, icon: Icon, selected: bool) -> Response {
    let atom = Atom::paint(egui::Vec2::splat(ICON), move |ui, args| {
        icon.paint(ui.painter(), args.rect, args.fallback_text_color);
    });
    ui.add(Button::selectable(selected, atom))
}

/// The toolbar's icons, painted as shapes rather than drawn from a font.
#[derive(Clone, Copy)]
enum Icon {
    /// A pointer.
    Select,
    /// Two arrows out of a corner, right and up, with a square between them, as Move's handles.
    Move,
    /// A ring, and another seen at a slant across it, as Rotate's.
    Rotate,
    /// Two lines out of a corner, right and up, each ending in a square, with one in the corner,
    /// as Scale's handles.
    Scale,
    /// Arrows each way.
    Pan,
    /// A circular arrow.
    Orbit,
    /// Lines across and down.
    Grid,
    /// A film camera: its body, and its lens.
    Camera,
    /// A triangle pointing right, as a player's.
    Play,
    /// A square, as a player's.
    Stop,
}

impl Icon {
    /// Paints it in `rect`, a square, in `color`.
    fn paint(self, painter: &Painter, rect: Rect, color: Color32) {
        let scale = rect.width() / 16.0;
        // From the top left of a square 16 across.
        let at = |x: f32, y: f32| rect.min + vec2(x, y) * scale;
        let line = Stroke::new(1.5, color);
        let triangle = |points: [Pos2; 3]| {
            painter.add(Shape::convex_polygon(points.to_vec(), color, Stroke::NONE));
        };
        match self {
            Self::Select => {
                triangle([at(4.0, 1.5), at(4.0, 13.0), at(12.0, 9.5)]);
                painter.line_segment([at(7.5, 10.0), at(10.0, 15.0)], Stroke::new(2.5, color));
            }
            Self::Move => {
                painter.line_segment([at(3.0, 13.0), at(11.0, 13.0)], line);
                painter.line_segment([at(3.0, 13.0), at(3.0, 5.0)], line);
                triangle([at(15.0, 13.0), at(10.5, 10.5), at(10.5, 15.5)]);
                triangle([at(3.0, 1.0), at(0.5, 5.5), at(5.5, 5.5)]);
                let square = Rect::from_min_max(at(5.5, 7.5), at(8.5, 10.5));
                painter.rect_filled(square, 0.0, color);
            }
            Self::Rotate => {
                let center = at(8.0, 8.0);
                painter.circle_stroke(center, 6.5 * scale, line);
                // Seen at a slant: squashed across, as a ring about the view's up looks.
                let way = |angle: f32| vec2(libm::cosf(angle) * 2.5, libm::sinf(angle) * 6.5);
                let slant = (0..=24).map(|i| center + way(TAU * i as f32 / 24.0) * scale);
                painter.add(Shape::line(slant.collect(), line));
            }
            Self::Scale => {
                painter.line_segment([at(3.5, 12.5), at(11.0, 12.5)], line);
                painter.line_segment([at(3.5, 12.5), at(3.5, 5.0)], line);
                for (x, y) in [(13.0, 12.5), (3.5, 3.0), (3.5, 12.5)] {
                    let square = Rect::from_center_size(at(x, y), vec2(4.0, 4.0) * scale);
                    painter.rect_filled(square, 0.0, color);
                }
            }
            Self::Pan => {
                painter.line_segment([at(8.0, 4.0), at(8.0, 12.0)], line);
                painter.line_segment([at(4.0, 8.0), at(12.0, 8.0)], line);
                triangle([at(8.0, 1.0), at(5.5, 4.5), at(10.5, 4.5)]);
                triangle([at(8.0, 15.0), at(5.5, 11.5), at(10.5, 11.5)]);
                triangle([at(1.0, 8.0), at(4.5, 5.5), at(4.5, 10.5)]);
                triangle([at(15.0, 8.0), at(11.5, 5.5), at(11.5, 10.5)]);
            }
            Self::Orbit => {
                let center = at(8.0, 8.0);
                let radius = 5.5 * scale;
                let way = |angle: f32| vec2(libm::cosf(angle), libm::sinf(angle));
                // Clockwise on the screen, whose y goes down, most of the way round.
                let (from, to) = (0.35, 0.35 + 1.7 * PI);
                let arc =
                    (0..=24).map(|i| center + way(from + (to - from) * i as f32 / 24.0) * radius);
                painter.add(Shape::line(arc.collect(), line));
                // Its head, on along the circle from where it ends.
                let end = center + way(to) * radius;
                let (along, out) = (way(to + PI / 2.0) * 3.0 * scale, way(to) * 2.5 * scale);
                triangle([
                    end + along,
                    end + out - along * 0.3,
                    end - out - along * 0.3,
                ]);
            }
            Self::Grid => {
                let thin = Stroke::new(1.0, color);
                for i in 0..4 {
                    let t = 2.0 + 4.0 * i as f32;
                    painter.line_segment([at(t, 2.0), at(t, 14.0)], thin);
                    painter.line_segment([at(2.0, t), at(14.0, t)], thin);
                }
            }
            Self::Camera => {
                let body = [at(1.5, 4.5), at(10.0, 4.5), at(10.0, 11.5), at(1.5, 11.5)];
                painter.add(Shape::closed_line(body.to_vec(), line));
                triangle([at(10.5, 8.0), at(15.0, 4.5), at(15.0, 11.5)]);
            }
            Self::Play => triangle([at(3.5, 2.0), at(3.5, 14.0), at(14.0, 8.0)]),
            Self::Stop => {
                let square = Rect::from_min_max(at(3.0, 3.0), at(13.0, 13.0));
                painter.rect_filled(square, 1.0, color);
            }
        }
    }
}

/// Draws the gizmo into `ui`: the world's axes and a cube, turned as `camera` sees them, without
/// perspective whatever its projection, and under them a label of the projection and of `aligned`,
/// the side the camera looks squarely from, if it does. Clicking an axis eases the camera to look
/// back along it, or, looking along it already, from the other side; clicking the cube or the
/// label switches the projection; dragging it orbits the camera's pivot.
pub fn gizmo(ui: &mut egui::Ui, camera: &mut OrbitCamera, aligned: Option<Side>) {
    let (rect, _) = ui.allocate_exact_size(vec2(GIZMO, GIZMO + LABEL), Sense::hover());
    let (axes, label) = rect.split_top_bottom_at_y(rect.min.y + GIZMO);
    let response = ui.interact(axes, Id::unique("gg editor gizmo"), Sense::click_and_drag());
    let center = axes.center();
    let rotation = camera.viewpoint().rotation;
    let hovered = (response.hover_pos()).and_then(|pos| hit(rotation, pos - center));
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    if response.hovered() || response.dragged() {
        let fill = visuals.window_fill.gamma_multiply(0.6);
        painter.circle_filled(center, GIZMO / 2.0, fill);
    }
    let balls = balls(rotation);
    let paint_ball = |ball: &Ball| {
        let at = center + ball.at;
        let color = color(ball.side);
        match letter(ball.side) {
            Some(letter) => {
                let from = center + ball.at * (CUBE / REACH);
                painter.line_segment([from, at], Stroke::new(2.0, color));
                painter.circle_filled(at, BALL, color);
                let font = FontId::proportional(11.0);
                painter.text(at, Align2::CENTER_CENTER, letter, font, Color32::BLACK);
            }
            None => {
                painter.circle_filled(at, BALL, color.gamma_multiply(0.25));
                painter.circle_stroke(at, BALL - 0.75, Stroke::new(1.5, color));
            }
        }
        if hovered == Some(Hit::Ball(ball.side)) {
            painter.circle_stroke(at, BALL + 1.5, Stroke::new(1.5, Color32::WHITE));
        }
    };
    for ball in balls.iter().filter(|ball| ball.near < 0.0) {
        paint_ball(ball);
    }
    let lit = if hovered == Some(Hit::Cube) {
        60.0
    } else {
        0.0
    };
    for (corners, facing) in faces(rotation) {
        let corners = corners.map(|corner| center + corner).to_vec();
        let fill = Color32::from_gray((50.0 + 70.0 * facing + lit) as u8);
        let edge = Stroke::new(1.0, Color32::from_gray(150));
        painter.add(Shape::convex_polygon(corners, fill, edge));
    }
    for ball in balls.iter().filter(|ball| ball.near >= 0.0) {
        paint_ball(ball);
    }
    if response.clicked()
        && let Some(pos) = response.interact_pointer_pos()
    {
        match hit(rotation, pos - center) {
            Some(Hit::Ball(side)) => camera.snap(side),
            Some(Hit::Cube) => camera.perspective = !camera.perspective,
            None => {}
        }
    }
    if response.dragged() {
        let by = response.drag_delta();
        camera.orbit(math::Vec2::new(by.x, by.y));
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    } else if hovered.is_some() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    } else if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Grab);
    }

    let projection = if camera.perspective { "Persp" } else { "Ortho" };
    let text = match aligned {
        Some(side) => format!("{side:?} · {projection}"),
        None => projection.to_owned(),
    };
    let tip = if camera.perspective {
        "Perspective: click for orthographic"
    } else {
        "Orthographic: click for perspective"
    };
    let switch = ui.interact(label, Id::unique("gg editor gizmo label"), Sense::click());
    let color = if switch.hovered() {
        visuals.strong_text_color()
    } else {
        visuals.text_color()
    };
    painter.text(
        label.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(12.0),
        color,
    );
    if switch
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(tip)
        .clicked()
    {
        camera.perspective = !camera.perspective;
    }
}

/// What's at a point of the gizmo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    Ball(Side),
    Cube,
}

/// One of the gizmo's axes, as the camera sees it.
#[derive(Clone, Copy, Debug)]
struct Ball {
    side: Side,
    /// Where it shows, from the gizmo's middle, in points down the screen.
    at: egui::Vec2,
    /// How far it points toward the viewer, from -1 to 1.
    near: f32,
}

/// The gizmo's six axes as a camera turned by `rotation` sees them, the furthest first.
fn balls(rotation: Quat) -> [Ball; 6] {
    let inverse = rotation.inverse();
    let mut balls = Side::ALL.map(|side| {
        let axis = inverse * side.axis();
        Ball {
            side,
            at: vec2(axis.x, -axis.y) * REACH,
            near: axis.z,
        }
    });
    balls.sort_by(|a, b| a.near.total_cmp(&b.near));
    balls
}

/// The faces of the gizmo's cube that face a camera turned by `rotation`: their corners, from the
/// gizmo's middle in points down the screen, and how squarely each faces it, from 0 to 1.
fn faces(rotation: Quat) -> impl Iterator<Item = ([egui::Vec2; 4], f32)> {
    let inverse = rotation.inverse();
    Side::ALL.into_iter().filter_map(move |side| {
        let facing = (inverse * side.axis()).z;
        if facing <= 0.0 {
            return None;
        }
        let (across, down) = side.axis().any_orthonormal_pair();
        let corners = [(1.0, 1.0), (1.0, -1.0), (-1.0, -1.0), (-1.0, 1.0)].map(|(a, b)| {
            let corner = inverse * ((side.axis() + across * a + down * b) * CUBE);
            vec2(corner.x, -corner.y)
        });
        Some((corners, facing))
    })
}

/// What's at `at`, from the gizmo's middle in points down the screen, for a camera turned by
/// `rotation`: the balls in front of the cube, the nearest first, then the cube, then the balls
/// behind it.
fn hit(rotation: Quat, at: egui::Vec2) -> Option<Hit> {
    let balls = balls(rotation);
    let on = |ball: &&Ball| (ball.at - at).length() <= BALL;
    let (front, back): (Vec<&Ball>, Vec<&Ball>) = balls.iter().rev().partition(|b| b.near >= 0.0);
    if let Some(ball) = front.into_iter().find(on) {
        return Some(Hit::Ball(ball.side));
    }
    if faces(rotation).any(|(corners, _)| inside(&corners, at)) {
        return Some(Hit::Cube);
    }
    back.into_iter().find(on).map(|ball| Hit::Ball(ball.side))
}

/// Whether `at` is in the convex polygon `corners`, wound either way.
fn inside(corners: &[egui::Vec2; 4], at: egui::Vec2) -> bool {
    let crosses = (0..4).map(|i| {
        let (a, b) = (corners[i], corners[(i + 1) % 4]);
        (b - a).x * (at - a).y - (b - a).y * (at - a).x
    });
    let (mut left, mut right) = (false, false);
    for cross in crosses {
        left |= cross > 0.0;
        right |= cross < 0.0;
    }
    !(left && right)
}

/// The color of `side`'s axis, as the grid draws it: X red, Y green, Z blue.
fn color(side: Side) -> Color32 {
    let linear = match side {
        Side::Right | Side::Left => grid::RED,
        Side::Top | Side::Bottom => grid::GREEN,
        Side::Front | Side::Back => grid::BLUE,
    };
    Rgba::from_rgb(linear.x, linear.y, linear.z).into()
}

/// The letter on a positive axis's ball; a negative one's has none.
fn letter(side: Side) -> Option<&'static str> {
    match side {
        Side::Right => Some("X"),
        Side::Top => Some("Y"),
        Side::Front => Some("Z"),
        _ => None,
    }
}

/// Names `name`, what the view shows under the cursor, at the bottom left of `view`, in a frame as
/// the toolbar's: painted on the view's own layer, over the world it shows and the handles, rather
/// than shown over the view as the toolbar is, so that the cursor over it is still over the view.
pub fn name_hovered(egui: &egui::Context, view: &Response, name: String) {
    let painter = egui.layer_painter(view.layer_id).with_clip_rect(view.rect);
    let style = egui.global_style();
    let color = style.visuals.text_color();
    let galley = painter.layout_no_wrap(name, TextStyle::Body.resolve(&style), color);
    let frame = panel(&style.visuals);
    let margin = frame.total_margin();
    let corner = view.rect.left_bottom() + vec2(INSET, -INSET);
    let outer = Align2::LEFT_BOTTOM.anchor_size(corner, galley.size() + margin.sum());
    let inner = outer - margin;
    painter.add(frame.paint(inner));
    painter.galley(inner.min, galley, color);
}

/// Outlines `view`, where a scene dragged from the Assets panel and let go opens, inside its edges
/// in the selection's color: painted on the view's own layer, as the name under the cursor is.
pub fn outline_drop(egui: &egui::Context, view: &Response) {
    let painter = egui.layer_painter(view.layer_id).with_clip_rect(view.rect);
    let color = egui.global_style().visuals.selection.stroke.color;
    painter.rect_stroke(view.rect, 0.0, Stroke::new(2.0, color), StrokeKind::Inside);
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use egui::{Modifiers, PointerButton, RawInput, UiBuilder, pos2};

    use super::*;
    use gg_engine::math::{EulerRot, Vec3};

    /// From the front, as the camera starts.
    const FRONT: Quat = Quat::IDENTITY;

    #[test]
    fn from_the_front_x_is_right_y_up_and_z_nearest() {
        let balls = balls(FRONT);
        let nearest = balls[5];
        assert_eq!(nearest.side, Side::Front);
        assert!(nearest.at.length() < 1e-5);
        let at = |side| balls.iter().find(|ball| ball.side == side).unwrap().at;
        assert!((at(Side::Right) - vec2(REACH, 0.0)).length() < 1e-4);
        assert!((at(Side::Top) - vec2(0.0, -REACH)).length() < 1e-4);
        // Its front face alone faces the viewer, squarely.
        let faces: Vec<_> = faces(FRONT).collect();
        assert_eq!(faces.len(), 1);
        assert!((faces[0].1 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_click_hits_the_balls_in_front_then_the_cube_then_the_balls_behind() {
        for (rotation, at, want) in [
            // The nearer of the two balls in the middle.
            (FRONT, vec2(0.0, 0.0), Some(Hit::Ball(Side::Front))),
            (
                Quat::from_rotation_y(PI),
                vec2(0.0, 0.0),
                Some(Hit::Ball(Side::Back)),
            ),
            // The cube's corner, clear of the ball in front of it.
            (FRONT, vec2(9.0, 9.0), Some(Hit::Cube)),
            (FRONT, vec2(REACH, 0.0), Some(Hit::Ball(Side::Right))),
            (FRONT, vec2(0.0, REACH), Some(Hit::Ball(Side::Bottom))),
            (FRONT, vec2(30.0, 30.0), None),
        ] {
            assert_eq!(hit(rotation, at), want, "{rotation} at {at:?}");
        }
        // Turned a little to the right, -Z's ball shows on the cube's right face, which hides it.
        let turned = Quat::from_rotation_y(0.2);
        let back = balls(turned)
            .into_iter()
            .find(|ball| ball.side == Side::Back)
            .unwrap();
        assert!((back.at - vec2(9.0, 0.0)).length() < BALL);
        assert_eq!(hit(turned, vec2(9.0, 0.0)), Some(Hit::Cube));
    }

    /// Runs passes of the gizmo at (100, 100) on `camera`: the pointer moved to `from`, pressed
    /// there, moved to `to` and let go there, which is a click where they're the same and a drag
    /// where they're further apart than a click goes.
    fn press(camera: &mut OrbitCamera, from: Pos2, to: Pos2) {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 400.0));
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        let moved = egui::Event::PointerMoved;
        for event in [
            moved(from),
            button(from, true),
            moved(to),
            button(to, false),
        ] {
            let input = RawInput {
                screen_rect: Some(screen),
                events: vec![event],
                ..RawInput::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let place = Rect::from_min_size(pos2(100.0, 100.0), vec2(200.0, 200.0));
                ui.scope_builder(UiBuilder::new().max_rect(place), |ui| {
                    gizmo(ui, camera, None);
                });
            });
            // As the renderer would have.
            output.textures_delta.clear();
        }
    }

    fn click(camera: &mut OrbitCamera, at: Pos2) {
        press(camera, at, at);
    }

    #[test]
    fn clicking_an_axis_snaps_and_the_cube_or_the_label_switch_the_projection() {
        let middle = pos2(100.0, 100.0) + vec2(GIZMO, GIZMO) / 2.0;
        let mut camera = OrbitCamera::new();
        click(&mut camera, middle + vec2(REACH, 0.0));
        camera.update(Duration::from_secs(1));
        let forward = camera.viewpoint().rotation * Vec3::NEG_Z;
        assert_eq!(Side::facing(forward), Some(Side::Right), "{forward}");

        let mut camera = OrbitCamera::new();
        click(&mut camera, middle + vec2(9.0, 9.0));
        assert!(camera.perspective);
        click(&mut camera, middle + vec2(0.0, GIZMO / 2.0 + LABEL / 2.0));
        assert!(!camera.perspective);
    }

    #[test]
    fn dragging_it_orbits_the_pivot_as_far_as_the_drag_goes() {
        let middle = pos2(100.0, 100.0) + vec2(GIZMO, GIZMO) / 2.0;
        let mut camera = OrbitCamera::new();
        // Far enough in one move that egui counts all of it as the drag's.
        press(&mut camera, middle, middle + vec2(20.0, 10.0));
        let mut orbited = OrbitCamera::new();
        orbited.orbit(math::Vec2::new(20.0, 10.0));
        let (seen, want) = (camera.viewpoint(), orbited.viewpoint());
        assert!(
            seen.rotation.abs_diff_eq(want.rotation, 1e-6),
            "{} against {}",
            seen.rotation,
            want.rotation
        );
        assert!(seen.position.abs_diff_eq(want.position, 1e-5));
    }

    #[test]
    fn its_axes_follow_the_camera_s_turn() {
        // Looking down from above and turned, as the camera's own Euler angles turn it.
        let rotation = Quat::from_euler(EulerRot::YXZ, 0.0, -PI / 2.0, 0.0);
        let balls = balls(rotation);
        assert_eq!(balls[5].side, Side::Top);
        let front = balls.iter().find(|ball| ball.side == Side::Front).unwrap();
        // The world's front, toward the viewer from above, is down the screen.
        assert!(
            (front.at - vec2(0.0, REACH)).length() < 1e-4,
            "{:?}",
            front.at
        );
    }
}

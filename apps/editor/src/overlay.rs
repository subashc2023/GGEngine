//! What the editor draws over its view, and the title bar's run controls ([`show_run`]):
//!
//! - the toolbar, at the top left or where its grip moves it ([`Toolbar::show`]);
//! - the gizmo, at the top right, which shows and turns the camera's facing ([`gizmo`]);
//! - the name of what's under the cursor, at the bottom left;
//! - an outline round its edges while an asset from the Assets panel is held over it.

use std::f32::consts::{PI, TAU};

use gg_api::probe::Probe;
use gg_api::{Failure, FrameImage, ImageKind, Pass, Shown};
use gg_engine::Context;
use gg_engine::egui::{
    self, Align2, Atom, Button, Color32, CursorIcon, FontId, Frame, Id, Margin, Painter, Popup,
    PopupCloseBehavior, Pos2, Rect, Response, Rgba, Sense, Shape, Stroke, StrokeKind, TextStyle,
    Visuals, vec2,
};
use gg_engine::grid::{self, Plane};
use gg_engine::math::{self, Quat, libm};
use serde_json::{Value, json};
use tracing::warn;

use crate::camera::{OrbitCamera, Side, Space, Tool};

/// Where [`Context::read_state`] keeps the toolbar's [`Placement`].
const FILE: &str = "toolbar.json";
/// The side of a toolbar icon, in points.
const ICON: f32 = 16.0;
/// How far the gizmo's balls are from its middle, in points.
const REACH: f32 = 34.0;
/// The radius of the gizmo's balls, in points.
const BALL: f32 = 8.0;
/// Half the side of the gizmo's cube, in points.
const CUBE: f32 = 10.0;
/// The gizmo's side, in points.
const GIZMO: f32 = 2.0 * (REACH + BALL) + 4.0;
/// The height of the label under the gizmo, in points.
const LABEL: f32 = 18.0;
/// How far in from the view's edges a [`label`] shows, in points: as far as
/// [`Ui::over_view`](gg_engine::Ui::over_view) places the toolbar and the gizmo.
const INSET: f32 = 8.0;

/// The list of what the view shows, before its images, as the toolbar names them: the game's own
/// light, `None`, then the parts of its shading.
const CHANNELS: &[(&str, Option<Shown>)] = &[
    ("Lit", None),
    ("Base color", Some(Shown::BaseColor)),
    ("Normal", Some(Shown::Normal)),
    ("Surface normal", Some(Shown::SurfaceNormal)),
    ("Metallic", Some(Shown::Metallic)),
    ("Roughness", Some(Shown::Roughness)),
    // "Occlusion" alone reads as occlusion culling.
    ("AO", Some(Shown::Occlusion)),
    ("Emission", Some(Shown::Emission)),
    ("Specular color", Some(Shown::SpecularColor)),
    ("Direct light", Some(Shown::DirectLight)),
    ("Sky light", Some(Shown::SkyLight)),
    ("Reflection", Some(Shown::Reflection)),
    ("Traced reflection", Some(Shown::TracedReflection)),
    ("Reflection rays", Some(Shown::ReflectionRays)),
    ("Diffuse light", Some(Shown::DiffuseLight)),
    ("Specular light", Some(Shown::SpecularLight)),
];

/// The list's own names for the frame's images, by debug name; the rest go by theirs.
const IMAGE_LABELS: &[(&str, &str)] = &[
    ("view depth", "Depth"),
    ("view motion", "Motion"),
    ("view ids", "IDs"),
    ("view visibility", "Visibility buffer"),
    ("ambient occlusion", "Ambient occlusion"),
    ("shadow cascade 0", "Shadow cascade 0"),
    ("shadow cascade 1", "Shadow cascade 1"),
    ("shadow cascade 2", "Shadow cascade 2"),
    ("shadow cascade 3", "Shadow cascade 3"),
    ("view colors", "Before tonemapping"),
];

fn image_label(name: &str) -> &str {
    IMAGE_LABELS
        .iter()
        .find(|(named, _)| *named == name)
        .map_or(name, |(_, label)| label)
}

/// What a frame image holds, as its entry's tooltip says.
fn kind_label(kind: ImageKind) -> &'static str {
    match kind {
        ImageKind::Color => "colors",
        ImageKind::Hdr => "colors of high dynamic range",
        ImageKind::Ids => "IDs, a color for each",
        ImageKind::Motion => "motion, shown as normals are",
        ImageKind::Visibility => "a visibility buffer, a color for each value",
        ImageKind::Depth => "depth",
        ImageKind::Scalar => "a value a pixel",
        ImageKind::Normals => "normals, each axis's -1 to 1 as 0 to 1",
    }
}

fn pass_label(pass: Pass) -> &'static str {
    match pass {
        Pass::AmbientOcclusion => "Ambient occlusion",
        Pass::Reflections => "Reflections",
        Pass::Shadows => "Shadows",
        Pass::Fog => "Fog",
        Pass::Bloom => "Bloom",
        Pass::DepthOfField => "Depth of field",
        Pass::MotionBlur => "Motion blur",
        Pass::Antialiasing => "Antialiasing",
        Pass::Grade => "Grade",
        Pass::Lens => "Lens",
        Pass::Adaptation => "Exposure adaptation",
    }
}

/// What the toolbar sets, and where it sits over the view.
pub struct Toolbar {
    pub tool: Tool,
    /// The entity's own axes or the world's, for Move's and Rotate's handles.
    pub space: Space,
    pub grid: bool,
    pub plane: Plane,
    /// Whether the view sees through the game's own camera, showing neither the grid nor the gizmo.
    /// While the game plays: Play rather than Simulate.
    pub world_camera: bool,
    /// Whether the game's colliders' outlines show, over all but a nearer opaque mesh, through
    /// either camera.
    pub colliders: bool,
    /// What the view shows in place of its light, through either camera; `None` for the game's own.
    pub show: Option<Shown>,
    /// The images of the game's frame the view can show, as last answered, listed after the parts;
    /// empty until the game answers.
    pub images: Vec<FrameImage>,
    /// Why the game refused to list them, as an older game without `images` does; the list says it
    /// in their place.
    pub images_refused: Option<String>,
    /// Whether the pointer came over the list's button, or clicked it, since this was last taken:
    /// the images are then asked for again.
    pub ask_images: bool,
    /// The passes the view leaves out, to find which one makes what it shows, through either
    /// camera; in [`Pass::ALL`]'s order, as the game answers; empty for none. Not kept between
    /// runs, so none is left out unawares.
    pub skip: Vec<Pass>,
    pub placement: Placement,
}

impl Default for Toolbar {
    fn default() -> Self {
        Self {
            tool: Tool::default(),
            space: Space::default(),
            grid: true,
            plane: Plane::XY,
            world_camera: false,
            colliders: false,
            show: None,
            images: Vec::new(),
            images_refused: None,
            ask_images: false,
            skip: Vec::new(),
            placement: Placement::default(),
        }
    }
}

/// Where the toolbar is over the view, which a drag of its grip moves ([`Toolbar::place`]), and
/// whether it's folded to its grip. Kept per user across runs ([`Toolbar::save`]).
#[derive(Debug, Default, PartialEq)]
pub struct Placement {
    /// Where its grip's drag last left it, from the view's top left, where
    /// [`Ui::over_view`](gg_engine::Ui::over_view) places it, in points.
    offset: egui::Vec2,
    folded: bool,
    /// Its size as it last showed, in points.
    size: egui::Vec2,
    /// The offset as the grip's drag started, while it lasts.
    from: Option<egui::Vec2>,
    /// Whether it's moved or folded since it was saved.
    changed: bool,
}

impl Placement {
    /// What file `text` holds: where the toolbar was and whether it was folded, each left as it
    /// starts if it doesn't read.
    fn parse(text: &str) -> serde_json::Result<Self> {
        let value: Value = serde_json::from_str(text)?;
        let offset = value["offset"]
            .as_array()
            .and_then(|xy| match xy.as_slice() {
                [x, y] => Some(vec2(x.as_f64()? as f32, y.as_f64()? as f32)),
                _ => None,
            });
        Ok(Self {
            offset: offset.unwrap_or_default(),
            folded: value["folded"].as_bool().unwrap_or(false),
            ..Self::default()
        })
    }

    /// Its file's text, for [`parse`](Self::parse).
    fn text(&self) -> String {
        let value = json!({"offset": [self.offset.x, self.offset.y], "folded": self.folded});
        format!("{value:#}\n")
    }
}

/// How the game runs, as the title bar's Play, Simulate and Stop set it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Stopped: its world is the world as edited, which only edits, scene files' saves and File >
    /// New and Open change.
    Edit,
    /// A copy of the world as edited plays in its place. The view sees through the game's camera,
    /// which gets the view's input.
    Play,
    /// A copy plays as in Play, but the view sees through the editor's camera, and the game gets no
    /// input.
    Simulate,
}

impl Toolbar {
    /// Reads the toolbar's [`Placement`], kept per user. Warns `editor.toolbar` if it can't, the
    /// toolbar then at the view's top left, unfolded.
    pub fn read(&mut self, cx: &Context) {
        let read = match cx.read_state(FILE) {
            Ok(None) => return,
            Ok(Some(bytes)) => String::from_utf8(bytes).map_err(|error| error.to_string()),
            Err(error) => Err(format!("{error:#}")),
        };
        let parsed = |text: String| Placement::parse(&text).map_err(|error| error.to_string());
        match read.and_then(parsed) {
            Ok(placement) => self.placement = placement,
            Err(why) => warn!(
                code = "editor.toolbar",
                file = FILE,
                "can't read where the toolbar was: {why}; it starts at the view's top left"
            ),
        }
    }

    /// Saves the toolbar's [`Placement`] per user, if changed. Warns `editor.toolbar` if it can't.
    pub fn save(&mut self, cx: &Context) {
        let placement = &mut self.placement;
        if !placement.changed {
            return;
        }
        placement.changed = false;
        if let Err(error) = cx.save_state(FILE, placement.text().as_bytes()) {
            warn!(code = "editor.toolbar", file = FILE, "{error:#}");
        }
    }

    /// Moves the toolbar by the pointer's travel since its grip's drag started, as egui saw it at
    /// the frame's start, within `view`, as far in from its edges as from its top left. Returns the
    /// offset for [`Ui::over_view`](gg_engine::Ui::over_view), kept within the view as it is now:
    /// unfolded, or in a smaller view, it shows further in than its grip left it, going back as it
    /// folds or the view grows. Call it before the toolbar shows, so it keeps up with the pointer.
    pub fn place(&mut self, egui: &egui::Context, view: Rect) -> egui::Vec2 {
        let placement = &mut self.placement;
        let room = view.size() - egui::Vec2::splat(2.0 * INSET) - placement.size;
        let room = room.max(egui::Vec2::ZERO);
        let within = |offset: egui::Vec2| offset.clamp(egui::Vec2::ZERO, room);
        let dragged = egui.read_response(grip_id());
        match dragged.and_then(|grip| grip.total_drag_delta()) {
            // From where it shows.
            Some(by) => {
                let from = *placement.from.get_or_insert(within(placement.offset));
                placement.offset = within(from + by);
            }
            None => placement.changed |= placement.from.take().is_some(),
        }
        within(placement.offset)
    }

    /// Draws the toolbar, each control with a tooltip:
    ///
    /// - its grip ([`place`](Self::place)) and the button that folds and unfolds it;
    /// - the tools, Probe last, their tooltips naming their keys;
    /// - the space of Move's and Rotate's axes, Local or World, which a click or X switches;
    /// - the camera's projection; the grid's switch and plane; the game's camera's switch; the
    ///   colliders' switch;
    /// - the list of what the view shows in place of its light;
    /// - Passes, lit while any is left out, whose list, ticked while drawn, stays open as they're
    ///   ticked, until a click outside it.
    ///
    /// While the view sees through the game's camera, the projection's and the grid's buttons are
    /// off: what they change doesn't show. The two lists are popups that take the view's keys while
    /// open.
    pub fn show(&mut self, ui: &mut egui::Ui, camera: &mut OrbitCamera) {
        // The key that snaps a drag of the handles, which egui's shortcuts take as the command key.
        let snaps = if cfg!(target_os = "macos") {
            "Cmd"
        } else {
            "Ctrl"
        };
        let shown = panel(ui.visuals()).show(ui, |ui| {
            ui.horizontal(|ui| {
                grip(ui);
                // Beside the grip, so that it stays under the pointer as it folds and unfolds.
                let placement = &mut self.placement;
                let (icon, tip) = if placement.folded {
                    (Icon::Unfold, "Unfold the toolbar")
                } else {
                    (Icon::Fold, "Fold the toolbar to its grip")
                };
                if icon_button(ui, icon, false).on_hover_text(tip).clicked() {
                    placement.folded = !placement.folded;
                    placement.changed = true;
                }
                if placement.folded {
                    return;
                }
                ui.separator();
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
                    (
                        Tool::Probe,
                        Icon::Probe,
                        "Probe: the left button reads what shading took and made at the pixel \
                         clicked, as `gg probe` does: its material's values, its normal, its light \
                         and its depth, beside it; Escape closes them"
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
                let (label, tip) = match self.space {
                    Space::Local => (
                        "Local",
                        "Local (X): Move's and Rotate's handles go along the picked entity's own \
                         axes; click, or press X, for the world's",
                    ),
                    Space::World => (
                        "World",
                        "World (X): Move's and Rotate's handles go along the world's axes; click, \
                         or press X, for the entity's own. Scale's always go along its own",
                    ),
                };
                if ui.button(label).on_hover_text(tip).clicked() {
                    self.space = self.space.flipped();
                }
                ui.separator();
                ui.add_enabled_ui(!self.world_camera, |ui| {
                    let (label, tip) = projection(camera);
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
                if icon_button(ui, Icon::Camera, self.world_camera)
                    .on_hover_text(
                        "Game camera: while the game plays, the view sees through its camera, \
                         which gets the view's input, as Play; the editor's camera is Simulate. \
                         Stopped, the view shows what the game's camera sees, the input staying \
                         the editor's; moving the editor's camera comes back to it",
                    )
                    .clicked()
                {
                    self.world_camera = !self.world_camera;
                }
                if icon_button(ui, Icon::Colliders, self.colliders)
                    .on_hover_text(
                        "Colliders: outlines each collider where its entity draws, as physics \
                         shapes it, whether the game plays or not",
                    )
                    .clicked()
                {
                    self.colliders = !self.colliders;
                }
                // An image shows by its label, or its name, whether the list has it or not, as one
                // another client shows may not be.
                let selected = match (CHANNELS.iter()).find(|(_, shown)| *shown == self.show) {
                    Some((label, _)) => (*label).to_owned(),
                    None => match &self.show {
                        Some(Shown::Image(name)) => image_label(name).to_owned(),
                        _ => "Lit".to_owned(),
                    },
                };
                // As tall as the window, past egui's default, so that the parts and the first
                // images show without scrolling. egui opens an area no taller than its style's
                // default area, 400 points, whatever the list asks: as tall as the window while
                // this one opens.
                let ctx = ui.ctx().clone();
                let tall = ctx.content_rect().height();
                let default_area = ctx.global_style().spacing.default_area_size;
                ctx.global_style_mut(|style| style.spacing.default_area_size.y = tall);
                let list = egui::ComboBox::from_id_salt("toolbar show")
                    .selected_text(selected)
                    .height(tall)
                    .show_ui(ui, |ui| {
                        for (label, shown) in CHANNELS {
                            ui.selectable_value(&mut self.show, shown.clone(), *label);
                        }
                        ui.separator();
                        if self.images.is_empty() {
                            match &self.images_refused {
                                Some(why) => ui.weak(format!("No images listed: {why}")),
                                None => ui.weak("No images listed"),
                            };
                        }
                        for FrameImage { name, kind, size } in &self.images {
                            let image = Some(Shown::Image(name.clone()));
                            let [width, height] = size;
                            let tip = format!("{name}: {}, {width}×{height}", kind_label(*kind));
                            (ui.selectable_value(&mut self.show, image, image_label(name)))
                                .on_hover_text(tip);
                        }
                    })
                    .response;
                ctx.global_style_mut(|style| style.spacing.default_area_size = default_area);
                let list = list.on_hover_text(
                    "What the view shows in place of its light, to see how it's made: a part of \
                     its shading, or an image its frame makes, as the game lists them as the list \
                     opens; agents can set it too",
                );
                // Asked as the pointer comes over the list, so that they're there as it opens,
                // which sizes it to them, and again as it opens.
                let hovered = list.hovered();
                let was_hovered = (ui.ctx()).data(|data| data.get_temp(list.id).unwrap_or(false));
                ui.ctx().data_mut(|data| data.insert_temp(list.id, hovered));
                self.ask_images |= (hovered && !was_hovered) || list.clicked();
                let label = match self.skip.len() {
                    0 => "Passes".to_owned(),
                    off => format!("Passes: {off} off"),
                };
                let passes = ui
                    .add(Button::selectable(!self.skip.is_empty(), label))
                    .on_hover_text(
                        "The passes the view draws, to narrow down which one makes what it shows: \
                         untick one to leave it out, as the game would draw without it, the game \
                         itself unchanged; agents can set them too",
                    );
                Popup::menu(&passes)
                    .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| self.show_passes(ui));
            });
        });
        self.placement.size = shown.response.rect.size();
    }

    /// Draws the passes, each ticked while drawn, a click toggling it, then Draw all.
    fn show_passes(&mut self, ui: &mut egui::Ui) {
        for pass in Pass::ALL {
            let mut drawn = !self.skip.contains(&pass);
            if ui.checkbox(&mut drawn, pass_label(pass)).changed() {
                if drawn {
                    self.skip.retain(|left_out| *left_out != pass);
                } else {
                    self.skip.push(pass);
                    self.skip.sort_unstable();
                }
            }
        }
        ui.separator();
        let all = ui.add_enabled(!self.skip.is_empty(), Button::new("Draw all"));
        if all.clicked() {
            self.skip.clear();
        }
    }
}

/// The id of the toolbar's grip, which [`Toolbar::place`] reads the drag of before it shows.
fn grip_id() -> Id {
    Id::unique("gg editor toolbar grip")
}

/// Draws the grip: two columns of dots, as tall as the buttons, lit under the pointer as a button
/// is.
fn grip(ui: &mut egui::Ui) {
    let size = vec2(ICON / 2.0, ICON + 2.0 * ui.spacing().button_padding.y);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let response = ui.interact(rect, grip_id(), Sense::drag());
    let color = ui.style().interact(&response).fg_stroke.color;
    for x in [-2.0, 2.0] {
        for y in [-4.0, 0.0, 4.0] {
            ui.painter()
                .circle_filled(rect.center() + vec2(x, y), 1.25, color);
        }
    }
    if response.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    } else if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Grab);
    }
    response.on_hover_text("Move the toolbar: drag it within the view");
}

/// What a click of the run controls asks: Play or Simulate, which play the game, or switch between
/// them while it plays; Stop, as [`Mode::Edit`]; a pause, a resume or a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clicked {
    To(Mode),
    Pause,
    Resume,
    Step,
}

/// Draws the run controls in the title bar's middle, as other editors' are, each with a tooltip,
/// for `current` and `paused`. Returns what a click asks.
///
/// - Play: plays the game while it's stopped; while it plays, pauses it, or resumes it while
///   paused.
/// - Simulate: lit while the game plays through the editor's camera. Plays it so while stopped, and
///   switches between the two while it plays.
/// - Stop: while it plays.
/// - Step: while it's paused or stopped.
pub fn show_run(ui: &mut egui::Ui, current: Mode, paused: bool) -> Option<Clicked> {
    let stopped = current == Mode::Edit;
    let (icon, tip, play) = match (stopped, paused) {
        (true, _) => (
            Icon::Play,
            "Play: a copy of the world as edited plays in its place, through the game's camera, \
             which gets the view's input, until Stop; what's edited meanwhile goes with the copy, \
             but a scene file's save",
            Clicked::To(Mode::Play),
        ),
        (false, false) => (Icon::Pause, "Pause the game's simulation", Clicked::Pause),
        (false, true) => (
            Icon::Play,
            "Resume: runs the game's simulation in real time again",
            Clicked::Resume,
        ),
    };
    let mut clicked = icon_button(ui, icon, false)
        .on_hover_text(tip)
        .clicked()
        .then_some(play);
    let simulating = current == Mode::Simulate;
    let (tip, to) = match current {
        Mode::Edit => (
            "Simulate: a copy plays as in Play, through the editor's camera, the game getting no \
             input",
            Mode::Simulate,
        ),
        Mode::Play => (
            "Simulate: the game plays on through the editor's camera, getting no input",
            Mode::Simulate,
        ),
        Mode::Simulate => (
            "Simulating: a click plays on through the game's camera, which gets the view's input",
            Mode::Play,
        ),
    };
    if icon_button(ui, Icon::Simulate, simulating)
        .on_hover_text(tip)
        .clicked()
    {
        clicked = Some(Clicked::To(to));
    }
    let stop = ui.add_enabled_ui(!stopped, |ui| icon_button(ui, Icon::Stop, false));
    let stop = (stop.inner)
        .on_hover_text("Stop: brings back the world as edited, with its edits' history")
        .on_disabled_hover_text("Stopped: the world is the world as edited");
    if stop.clicked() {
        clicked = Some(Clicked::To(Mode::Edit));
    }
    let tip = if stopped {
        "Step: plays the game paused, as the view's camera shows it, and runs a tick"
    } else {
        "Step: runs a tick of the game's simulation"
    };
    let step = ui.add_enabled_ui(stopped || paused, |ui| icon_button(ui, Icon::Step, false));
    let step = (step.inner)
        .on_hover_text(tip)
        .on_disabled_hover_text("Pause first: a step runs a tick of a paused game");
    if step.clicked() {
        clicked = Some(Clicked::Step);
    }
    clicked
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
    /// A crosshair: a ring with a line in from each side, and a dot at its middle.
    Probe,
    /// Lines across and down.
    Grid,
    /// A film camera: its body, and its lens.
    Camera,
    /// A square's outline and a circle's across its corner, as colliders' outlines.
    Colliders,
    /// A triangle pointing right, as a player's.
    Play,
    /// Two bars, as a player's.
    Pause,
    /// A ball falling onto the ground, smaller where it was.
    Simulate,
    /// A square, as a player's.
    Stop,
    /// A triangle pointing right against a bar, as a player's skip.
    Step,
    /// A chevron pointing left, toward the grip.
    Fold,
    /// A chevron pointing right, away from the grip.
    Unfold,
}

impl Icon {
    /// Paints it in square `rect`, in `color`.
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
            Self::Probe => {
                painter.circle_stroke(at(8.0, 8.0), 4.5 * scale, line);
                for [from, to] in [
                    [at(8.0, 0.5), at(8.0, 5.0)],
                    [at(8.0, 11.0), at(8.0, 15.5)],
                    [at(0.5, 8.0), at(5.0, 8.0)],
                    [at(11.0, 8.0), at(15.5, 8.0)],
                ] {
                    painter.line_segment([from, to], line);
                }
                painter.circle_filled(at(8.0, 8.0), 1.0 * scale, color);
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
            Self::Colliders => {
                let square = Rect::from_min_max(at(1.5, 1.5), at(10.5, 10.5));
                painter.rect_stroke(square, 0.0, line, StrokeKind::Middle);
                painter.circle_stroke(at(10.5, 10.5), 4.5 * scale, line);
            }
            Self::Play => triangle([at(3.5, 2.0), at(3.5, 14.0), at(14.0, 8.0)]),
            Self::Pause => {
                for x in [3.5, 9.5] {
                    let bar = Rect::from_min_max(at(x, 2.5), at(x + 3.0, 13.5));
                    painter.rect_filled(bar, 1.0, color);
                }
            }
            Self::Step => {
                triangle([at(2.5, 2.5), at(2.5, 13.5), at(10.5, 8.0)]);
                let bar = Rect::from_min_max(at(11.0, 2.5), at(13.5, 13.5));
                painter.rect_filled(bar, 1.0, color);
            }
            Self::Simulate => {
                painter.line_segment([at(1.0, 14.5), at(15.0, 14.5)], line);
                painter.circle_filled(at(10.5, 9.5), 3.5 * scale, color);
                painter.circle_filled(at(6.0, 4.5), 1.75 * scale, color);
                painter.circle_filled(at(3.0, 2.0), 1.0 * scale, color);
            }
            Self::Stop => {
                let square = Rect::from_min_max(at(3.0, 3.0), at(13.0, 13.0));
                painter.rect_filled(square, 1.0, color);
            }
            Self::Fold => {
                let chevron = vec![at(10.5, 3.0), at(5.5, 8.0), at(10.5, 13.0)];
                painter.add(Shape::line(chevron, Stroke::new(2.0, color)));
            }
            Self::Unfold => {
                let chevron = vec![at(5.5, 3.0), at(10.5, 8.0), at(5.5, 13.0)];
                painter.add(Shape::line(chevron, Stroke::new(2.0, color)));
            }
        }
    }
}

/// Draws the gizmo: the world's axes and a cube as `camera` sees them, without perspective, over a
/// label of the projection and of `aligned`, the side the camera looks squarely from, if any.
///
/// - A click on an axis eases the camera to look back along it, or from the other side if it
///   already does.
/// - A click on the cube or the label switches the projection.
/// - A drag orbits the camera's pivot.
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

    let (name, tip) = projection(camera);
    let text = match aligned {
        Some(side) => format!("{side:?} · {name}"),
        None => name.to_owned(),
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

/// The short name of `camera`'s projection, and the tip of a switch to the other, for the toolbar's
/// button and the gizmo's label.
fn projection(camera: &OrbitCamera) -> (&'static str, &'static str) {
    if camera.perspective {
        ("Persp", "Perspective: click for orthographic")
    } else {
        ("Ortho", "Orthographic: click for perspective")
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

/// Names `name`, what's under the cursor, at `view`'s bottom left, as a [`label`].
pub fn name_hovered(egui: &egui::Context, view: &Response, name: String) {
    label(egui, view, Align2::LEFT_BOTTOM, name, TextStyle::Body);
}

/// Shows `text` in `style` at `corner` of `view`, framed as the toolbar is. It paints on the view's
/// own layer, over the world and the handles, so the cursor over it is still over the view.
pub fn label(
    egui: &egui::Context,
    view: &Response,
    corner: Align2,
    text: String,
    style: TextStyle,
) {
    let painter = egui.layer_painter(view.layer_id).with_clip_rect(view.rect);
    let global = egui.global_style();
    let color = global.visuals.text_color();
    let galley = painter.layout_no_wrap(text, style.resolve(&global), color);
    let frame = panel(&global.visuals);
    let margin = frame.total_margin();
    let at = corner.pos_in_rect(&view.rect.shrink(INSET));
    let outer = corner.anchor_size(at, galley.size() + margin.sum());
    let inner = outer - margin;
    painter.add(frame.paint(inner));
    painter.galley(inner.min, galley, color);
}

/// The Probe panel's text for `probed`, or that it waits while `None`: a line per part, named as
/// the view's list names them, numbers as [`Probe`] says, values and colors linear, light as the
/// view's sRGB bytes, `-` outside the view.
pub fn probe_text(probed: Option<&Result<Probe, Failure>>) -> String {
    let probe = match probed {
        None => return "Probing…".to_owned(),
        Some(Err(Failure { code, message })) => return format!("Probe failed: [{code}] {message}"),
        Some(Ok(probe)) => probe,
    };
    let none = || "-".to_owned();
    let color = |color: Option<[f64; 3]>| {
        color.map_or_else(none, |[r, g, b]| format!("{r:.3} {g:.3} {b:.3}"))
    };
    let value = |value: Option<f64>| value.map_or_else(none, |value| format!("{value:.3}"));
    let light =
        |light: Option<[u8; 3]>| light.map_or_else(none, |[r, g, b]| format!("{r} {g} {b}"));
    let entity = match (&probe.entity.name, &probe.entity.id) {
        (Some(name), Some(id)) => format!("{name} ({id})"),
        (None, Some(id)) => id.clone(),
        _ => none(),
    };
    let [x, y] = probe.at;
    let lines = [
        ("Pixel", format!("{x:.0} {y:.0}")),
        ("Base color", color(probe.base_color)),
        ("Normal", color(probe.normal)),
        ("Surface normal", color(probe.surface_normal)),
        ("Metallic", value(probe.metallic)),
        ("Roughness", value(probe.roughness)),
        ("AO", value(probe.occlusion)),
        ("Emission", color(probe.emission)),
        ("Specular color", color(probe.specular_color)),
        ("Direct light", light(probe.direct_light)),
        ("Sky light", light(probe.sky_light)),
        ("Reflection", light(probe.reflection)),
        ("Diffuse light", light(probe.diffuse_light)),
        ("Specular light", light(probe.specular_light)),
        (
            "Depth",
            (probe.depth).map_or_else(none, |depth| format!("{depth:.3e}")),
        ),
        ("Distance", value(probe.distance)),
        ("Entity", entity),
    ];
    let mut text: String = (lines.iter())
        .map(|(label, value)| format!("{label:<16}{value}\n"))
        .collect();
    text.push_str("Light as the view's sRGB bytes; all read from 8-bit pixels");
    text
}

/// Shows [`probe_text`]'s `text` beside pixel `at`, in points, kept inside `view`, with a ring
/// round the pixel, on the view's own layer as a [`label`].
pub fn probe_readout(egui: &egui::Context, view: &Response, at: Pos2, text: String) {
    let painter = egui.layer_painter(view.layer_id).with_clip_rect(view.rect);
    let global = egui.global_style();
    let color = global.visuals.text_color();
    painter.circle_stroke(
        at,
        5.0,
        Stroke::new(1.5, global.visuals.selection.stroke.color),
    );
    let galley = painter.layout_no_wrap(text, TextStyle::Monospace.resolve(&global), color);
    let frame = panel(&global.visuals);
    let margin = frame.total_margin();
    let size = galley.size() + margin.sum();
    // Below and to the right of the pixel, moved back inside the view where it would leave it.
    let room = view.rect.shrink(INSET);
    let min = (at + vec2(12.0, 12.0)).min(room.max - size).max(room.min);
    let inner = Rect::from_min_size(min, size) - margin;
    painter.add(frame.paint(inner));
    painter.galley(inner.min, galley, color);
}

/// Outlines `view` inside its edges in the selection's color, as egui lights drop zones: while an
/// asset from the Assets panel is held over it, and in Play while it has the keys, which go to the
/// game. Painted on the view's own layer.
pub fn outline_view(egui: &egui::Context, view: &Response) {
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

    /// Passes of the gizmo at (100, 100): the pointer moved to `from`, pressed, moved to `to` and
    /// let go. That's a click where they're the same, and a drag where they're further apart than a
    /// click goes.
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

    /// A pass of `toolbar` over `view`, placed as [`Ui::over_view`](gg_engine::Ui::over_view)
    /// places it, with `events`. Returns its grip's middle.
    fn show_toolbar(
        ctx: &egui::Context,
        toolbar: &mut Toolbar,
        view: Rect,
        events: Vec<egui::Event>,
    ) -> Pos2 {
        let input = RawInput {
            screen_rect: Some(view),
            events,
            ..RawInput::default()
        };
        let mut grip = None;
        let mut output = ctx.run_ui(input, |ui| {
            let offset = toolbar.place(ui.ctx(), view);
            egui::Area::new(Id::unique("gg editor test toolbar"))
                .fixed_pos(view.min + egui::Vec2::splat(INSET) + offset)
                .constrain_to(view)
                .show(ui.ctx(), |ui| toolbar.show(ui, &mut OrbitCamera::new()));
            // Before the pass ends, after which egui reads the pass before.
            grip = ui.ctx().read_response(grip_id());
        });
        output.textures_delta.clear();
        grip.expect("it shows").rect.center()
    }

    #[test]
    fn dragging_the_grip_moves_the_toolbar_with_the_pointer_within_the_view() {
        let ctx = egui::Context::default();
        // Wider than the toolbar, which has no room to move in a view as wide as it is.
        let view = Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 400.0));
        let mut toolbar = Toolbar::default();
        // The first pass sizes it.
        show_toolbar(&ctx, &mut toolbar, view, Vec::new());
        let grip = show_toolbar(&ctx, &mut toolbar, view, Vec::new());
        let moved = egui::Event::PointerMoved;
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        show_toolbar(
            &ctx,
            &mut toolbar,
            view,
            vec![moved(grip), button(grip, true)],
        );
        // In the pass the pointer moves in.
        let by = vec2(100.0, 50.0);
        let shown = show_toolbar(&ctx, &mut toolbar, view, vec![moved(grip + by)]);
        assert_eq!(shown, grip + by);
        // Past the view's far corner, it stops as far in from it as from the top left.
        show_toolbar(&ctx, &mut toolbar, view, vec![moved(pos2(5000.0, 5000.0))]);
        let far = toolbar.placement.offset + toolbar.placement.size;
        let room = view.size() - egui::Vec2::splat(2.0 * INSET);
        assert!((far - room).length() < 1e-3, "{far:?} in {room:?}");
        // Back, it's where the pointer holds it again.
        let shown = show_toolbar(&ctx, &mut toolbar, view, vec![moved(grip + by)]);
        assert_eq!(shown, grip + by);
        // Let go, it stays, and is saved where it is.
        assert!(!toolbar.placement.changed, "while dragged");
        show_toolbar(&ctx, &mut toolbar, view, vec![button(grip + by, false)]);
        assert!(toolbar.placement.changed, "let go");
        let shown = show_toolbar(&ctx, &mut toolbar, view, vec![moved(grip)]);
        assert_eq!(shown, grip + by);
        // In a smaller view it shows further in, and where it was once the view's as big again.
        let smaller = Rect::from_min_size(Pos2::ZERO, vec2(700.0, 60.0));
        let shown = show_toolbar(&ctx, &mut toolbar, smaller, Vec::new());
        assert!(
            shown.x < grip.x + by.x && shown.y < grip.y + by.y,
            "{shown:?}"
        );
        let shown = show_toolbar(&ctx, &mut toolbar, view, Vec::new());
        assert_eq!(shown, grip + by);
    }

    /// What a click of title bar control `i` (Play, Simulate, Stop, Step) asks, for `current` and
    /// `paused`: a pass each to move, press and let go, the controls from (10, 10).
    fn click_control(current: Mode, paused: bool, i: usize) -> Option<Clicked> {
        let ctx = egui::Context::default();
        let spacing = ctx.global_style().spacing.clone();
        let width = ICON + 2.0 * spacing.button_padding.x;
        let height = (ICON + 2.0 * spacing.button_padding.y).max(spacing.interact_size.y);
        let across = i as f32 * (width + spacing.item_spacing.x) + width / 2.0;
        let at = pos2(10.0 + across, 10.0 + height / 2.0);
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        let mut clicked = None;
        for event in [egui::Event::PointerMoved(at), button(true), button(false)] {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 100.0))),
                events: vec![event],
                ..RawInput::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let place = Rect::from_min_size(pos2(10.0, 10.0), vec2(300.0, height));
                let row = UiBuilder::new()
                    .max_rect(place)
                    .layout(egui::Layout::left_to_right(egui::Align::Center));
                let shown = ui.scope_builder(row, |ui| show_run(ui, current, paused));
                clicked = clicked.or(shown.inner);
            });
            output.textures_delta.clear();
        }
        clicked
    }

    /// The run controls, as other editors' are. Stopped: Play and Simulate play, Stop is off, Step
    /// steps. Playing: Play pauses, Simulate switches cameras, Stop stops, Step is off. Paused:
    /// Play resumes, Step steps.
    #[test]
    fn the_title_bars_controls_play_pause_stop_and_step_as_the_game_runs() {
        use Clicked::{Pause, Resume, Step, To};
        let stopped = [
            Some(To(Mode::Play)),
            Some(To(Mode::Simulate)),
            None,
            Some(Step),
        ];
        for (current, paused, expected) in [
            (Mode::Edit, false, stopped),
            (Mode::Edit, true, stopped),
            (
                Mode::Play,
                false,
                [
                    Some(Pause),
                    Some(To(Mode::Simulate)),
                    Some(To(Mode::Edit)),
                    None,
                ],
            ),
            (
                Mode::Play,
                true,
                [
                    Some(Resume),
                    Some(To(Mode::Simulate)),
                    Some(To(Mode::Edit)),
                    Some(Step),
                ],
            ),
            (
                Mode::Simulate,
                false,
                [
                    Some(Pause),
                    Some(To(Mode::Play)),
                    Some(To(Mode::Edit)),
                    None,
                ],
            ),
        ] {
            let clicked = [0, 1, 2, 3].map(|i| click_control(current, paused, i));
            assert_eq!(clicked, expected, "{current:?}, paused {paused}");
        }
    }

    /// A pass of `toolbar` at the top left of a 1400 by 800 point view, tall enough for the view's
    /// list, with `events`: what it drew.
    fn toolbar_pass(
        ctx: &egui::Context,
        toolbar: &mut Toolbar,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let view = Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 800.0));
        let input = RawInput {
            screen_rect: Some(view),
            events,
            ..RawInput::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            egui::Area::new(Id::unique("gg editor test toolbar"))
                .fixed_pos(view.min + egui::Vec2::splat(INSET))
                .show(ui.ctx(), |ui| toolbar.show(ui, &mut OrbitCamera::new()));
        });
        output.textures_delta.clear();
        output
    }

    /// Clicks `toolbar` at `at` over three passes: move, press, let go. Returns what the last drew.
    fn click_toolbar(ctx: &egui::Context, toolbar: &mut Toolbar, at: Pos2) -> egui::FullOutput {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        toolbar_pass(ctx, toolbar, vec![egui::Event::PointerMoved(at)]);
        toolbar_pass(ctx, toolbar, vec![button(true)]);
        toolbar_pass(ctx, toolbar, vec![button(false)])
    }

    /// The middle of where `output` drew `text`, a text of its own, if it did.
    fn text_at(output: &egui::FullOutput, text: &str) -> Option<Pos2> {
        output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                Shape::Text(shape) if shape.galley.text() == text => {
                    Some(shape.pos + shape.galley.rect.center().to_vec2())
                }
                _ => None,
            })
    }

    /// The Passes button opens the passes, each ticked while drawn. A click leaves one out, the
    /// list staying open, kept in the game's order; another draws it again. Draw all draws them
    /// all. The button counts those left out.
    #[test]
    fn the_passes_list_leaves_passes_out_and_draws_them_all_again() {
        let ctx = egui::Context::default();
        let mut toolbar = Toolbar::default();
        // Each pass after the first that shows the toolbar, then the list, which only sizes them.
        let drawn = [(); 2].map(|()| toolbar_pass(&ctx, &mut toolbar, Vec::new()));
        let button = text_at(&drawn[1], "Passes").expect("the button");
        click_toolbar(&ctx, &mut toolbar, button);
        let opened = toolbar_pass(&ctx, &mut toolbar, Vec::new());
        let [shadows, reflections, all] =
            ["Shadows", "Reflections", "Draw all"].map(|text| text_at(&opened, text).expect(text));
        click_toolbar(&ctx, &mut toolbar, shadows);
        click_toolbar(&ctx, &mut toolbar, reflections);
        assert_eq!(toolbar.skip, [Pass::Reflections, Pass::Shadows]);
        let listed = toolbar_pass(&ctx, &mut toolbar, Vec::new());
        assert!(text_at(&listed, "Passes: 2 off").is_some(), "counted");
        click_toolbar(&ctx, &mut toolbar, reflections);
        assert_eq!(toolbar.skip, [Pass::Shadows], "drawn again");
        click_toolbar(&ctx, &mut toolbar, all);
        assert_eq!(toolbar.skip, [], "all drawn again");
    }

    /// The view's list asks for the game's images as the pointer comes over it and again as it
    /// opens. It lists them after the parts, by label or name, showing the one clicked, and says
    /// why there are none where the game refused.
    #[test]
    fn the_list_shows_the_images_the_game_lists() {
        let ctx = egui::Context::default();
        let mut toolbar = Toolbar::default();
        let drawn = [(); 2].map(|()| toolbar_pass(&ctx, &mut toolbar, Vec::new()));
        let list = text_at(&drawn[1], "Lit").expect("the list");
        toolbar_pass(&ctx, &mut toolbar, vec![egui::Event::PointerMoved(list)]);
        assert!(
            std::mem::take(&mut toolbar.ask_images),
            "asked as the pointer comes"
        );
        toolbar_pass(&ctx, &mut toolbar, Vec::new());
        assert!(!toolbar.ask_images, "once while it's over the list");
        toolbar.images = vec![
            FrameImage {
                name: "view depth".to_owned(),
                kind: ImageKind::Depth,
                size: [64, 32],
            },
            FrameImage {
                name: "bloom 1".to_owned(),
                kind: ImageKind::Hdr,
                size: [32, 16],
            },
        ];
        // A pass for the list to size its longer scroll, then one that draws it all.
        click_toolbar(&ctx, &mut toolbar, list);
        assert!(std::mem::take(&mut toolbar.ask_images), "asked as it opens");
        let opened = toolbar_pass(&ctx, &mut toolbar, Vec::new());
        assert!(text_at(&opened, "Depth").is_some(), "by its label");
        let bloom = text_at(&opened, "bloom 1").expect("by its name");
        click_toolbar(&ctx, &mut toolbar, bloom);
        assert_eq!(toolbar.show, Some(Shown::Image("bloom 1".to_owned())));
        toolbar.images.clear();
        toolbar.images_refused = Some("[api.unknown_method] no `images`".to_owned());
        let drawn = toolbar_pass(&ctx, &mut toolbar, Vec::new());
        let list = text_at(&drawn, "bloom 1").expect("the list, showing its image");
        click_toolbar(&ctx, &mut toolbar, list);
        let opened = toolbar_pass(&ctx, &mut toolbar, Vec::new());
        let why = "No images listed: [api.unknown_method] no `images`";
        assert!(text_at(&opened, why).is_some(), "says why");
    }

    /// The Probe numbers: waiting, why it failed, or a line per part: values and colors to three
    /// places, light as bytes, depth as an exponent, `-` for none, the entity by name and ID.
    #[test]
    fn a_probes_numbers_read_as_the_list_names_them() {
        assert_eq!(probe_text(None), "Probing…");
        let failed = Err(Failure::new("api.no_view", "no view"));
        assert_eq!(
            probe_text(Some(&failed)),
            "Probe failed: [api.no_view] no view"
        );
        let text = probe_text(Some(&Ok(crate::tests::probe([300.0, 200.0]))));
        for line in [
            "Pixel           300 200",
            "Base color      0.500 0.250 1.000",
            "Metallic        0.250",
            "Specular color  0.155 0.092 0.280",
            "Direct light    12 30 40",
            "Specular light  4 6 8",
            "Depth           2.441e-4",
            "Distance        -",
            "Entity          Crate (2b7e151628aed2a6)",
        ] {
            assert!(text.lines().any(|got| got == line), "{line:?} in:\n{text}");
        }
    }

    /// The view's list names each part by its label, on the closed list too while shown.
    #[test]
    fn the_list_names_what_the_view_shows() {
        let ctx = egui::Context::default();
        for (show, label) in [
            (Shown::SpecularColor, "Specular color"),
            (Shown::DiffuseLight, "Diffuse light"),
            (Shown::SpecularLight, "Specular light"),
            (Shown::TracedReflection, "Traced reflection"),
            (Shown::ReflectionRays, "Reflection rays"),
        ] {
            let mut toolbar = Toolbar {
                show: Some(show),
                ..Toolbar::default()
            };
            let drawn = [(); 2].map(|()| toolbar_pass(&ctx, &mut toolbar, Vec::new()));
            assert!(text_at(&drawn[1], label).is_some(), "{label}");
        }
    }

    /// The toolbar's place and fold read back, and what of a file doesn't read is left as it
    /// starts.
    #[test]
    fn where_the_toolbar_is_and_whether_it_s_folded_read_back() {
        let placement = Placement {
            offset: vec2(120.5, 40.0),
            folded: true,
            ..Placement::default()
        };
        assert_eq!(
            Placement::parse(&placement.text()).expect("read"),
            placement
        );
        let partly = Placement::parse(r#"{"offset": [1, "a"], "folded": true}"#).expect("read");
        let folded = Placement {
            folded: true,
            ..Placement::default()
        };
        assert_eq!(partly, folded);
        assert!(Placement::parse("not json").is_err());
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

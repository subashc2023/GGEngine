//! What the view picks, probes and has dropped on it, as the game's view finds it.

use std::ffi::OsStr;
use std::path::Path;

use gg_api::Failure;
use gg_api::asset::{AssetKind, Meta};
use gg_api::probe::Probe;
use gg_engine::egui::{self, Response};
use gg_engine::grid::{self};
use gg_engine::math::{DVec3, Vec2};
use gg_engine::reflect::{self, Reflect};
use gg_engine::{EntityId, InspectorLink, Name, Transform, Viewport};

use crate::assets::Dragged;
use crate::camera::Tool;
use crate::handles::{Along, Picked, Sight};
use crate::overlay::{self};
use crate::view::{self};

use super::{Game, Level, in_view};
#[cfg(doc)]
use gg_engine::Layer;

/// The Probe tool's probe: its ticket, its pixel in the main window's pixels, and its answer once
/// it's come.
pub(super) struct Probing {
    pub(super) ticket: u64,
    pub(super) at: [f64; 2],
    pub(super) answer: Option<Result<Probe, Failure>>,
}

/// A texture or font let go over the view: the ticket of the pick where it was let go, its kind,
/// its name as the game takes it, and its file's name as the Logs say it.
#[derive(Debug, PartialEq)]
pub(super) struct Dropping {
    pub(super) ticket: u64,
    kind: AssetKind,
    name: String,
    file: String,
}

/// What a pick of the game's view found: an entity by ID, and what the view's bottom left calls
/// it: its name, or without one, its ID.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Found {
    id: EntityId,
    pub(super) name: String,
}

impl Found {
    /// What the game's answer to a pick found: none outside the view or where no entity draws.
    fn of(picked: gg_api::Picked) -> Option<Self> {
        let id: EntityId = picked.id?.parse().ok()?;
        let name = picked.name.unwrap_or_else(|| id.to_string());
        Some(Self { id, name })
    }
}

impl Game {
    /// Takes the answers to picks that came back since. The latest click's the Inspector picks; a
    /// dropped texture's or font's [`take_dropped`](Self::take_dropped) takes; all of them serve
    /// what's under the cursor, which [`ui`](Layer::ui) reads. Failures are logged.
    pub(super) fn take_picks(&mut self) {
        self.answered.clear();
        let Some(talker) = &self.talker else {
            return;
        };
        let answers: Vec<_> = talker.picked.try_iter().collect();
        for (ticket, answer) in answers {
            if self.hovering == Some(ticket) {
                self.hovering = None;
            }
            let dropped = self.dropping.take_if(|dropping| dropping.ticket == ticket);
            match answer {
                Ok(picked) => {
                    let found = Found::of(picked);
                    if let Some(dropped) = dropped {
                        self.take_dropped(dropped, found.clone());
                    }
                    self.answered.push((ticket, found));
                }
                Err(Failure { code, message }) => {
                    if self.view.asked == Some(ticket) {
                        self.view.asked = None;
                    }
                    self.log(Level::Error, format!("`pick`: [{code}] {message}"));
                    // The connection broke or gave up on an answer.
                    if matches!(code.as_str(), "api.closed" | "api.timeout") {
                        self.disconnect();
                        return;
                    }
                }
            }
        }
        if let Some(found) = view::answer(&mut self.view.asked, &self.answered) {
            // Nothing found, with Ctrl or Shift held, leaves what's picked.
            match found {
                Some(found) if self.view.toggles => self.view.link.toggle(found.id),
                None if self.view.toggles => {}
                found => self.view.link.pick(found.map(|found| found.id)),
            }
            self.took_pick = None;
        }
    }

    /// Takes what the Assets panel let go over `response`, the view's, this frame while a game
    /// serves, as [`let_go`](Self::let_go) says, with the view at `view` and the window at `scale`
    /// pixels a point. Outlines the view while an asset is held over it.
    pub(super) fn take_drop(
        &mut self,
        egui: &egui::Context,
        response: &Response,
        view: Option<Viewport>,
        scale: f32,
    ) {
        if self.talker.is_none() {
            return;
        }
        if response.dnd_hover_payload::<Dragged>().is_some() {
            overlay::outline_view(egui, response);
        }
        let Some(dropped) = response.dnd_release_payload::<Dragged>() else {
            return;
        };
        // Where it was let go, as egui places what it hits; kept for its frame by a tap, unlike
        // the cursor once the window loses it.
        let Some(at) = egui.pointer_interact_pos() else {
            return;
        };
        let ppp = egui.pixels_per_point();
        let at = [at.x * ppp, at.y * ppp].map(f64::from);
        self.let_go(&dropped, at, view, scale);
    }

    /// Takes `dropped`, let go at `at` over the view at `view`, in the main window's pixels,
    /// `scale` to a point, named as [`asset_name`] does. A texture or font asks for a pick there,
    /// which [`take_picks`](Self::take_picks) takes. A model is a root the link creates where it
    /// [`lands`](Self::landing), named after its file; a prefab, an instance's root there, named as
    /// its prefab's root. Before a frame has come, neither lands, and it's logged as taken by
    /// nothing.
    pub(super) fn let_go(
        &mut self,
        dropped: &Dragged,
        at: [f64; 2],
        view: Option<Viewport>,
        scale: f32,
    ) {
        let root = self.folders.as_ref().map(|folders| folders.root.as_path());
        let name = asset_name(root, &dropped.path);
        let file = dropped.path.file_name().unwrap_or_default();
        let file = file.to_string_lossy().into_owned();
        match dropped.kind {
            kind @ (AssetKind::Texture | AssetKind::Font) => {
                let origin = view.map_or([0, 0], |view| view.origin);
                // A model's root, never a node, which refuses the set.
                if let Some(ticket) = self.ask_pick(at, origin, None) {
                    self.dropping = Some(Dropping {
                        ticket,
                        kind,
                        name,
                        file,
                    });
                }
            }
            kind @ (AssetKind::Model | AssetKind::Prefab) => {
                let Some(landing) = self.landing(at, view, scale) else {
                    let why = "the view has shown no frame to place it in yet";
                    self.log(Level::Info, format!("{file} was let go, but {why}"));
                    return;
                };
                let transform = Transform {
                    translation: landing,
                    ..Transform::IDENTITY
                };
                // A tuple struct's field, `0`, as the game reads one.
                let root = vec![("0".into(), reflect::Value::String(name))];
                let (root, transform) = (reflect::Value::Struct(root), transform.to_value());
                let components = if kind == AssetKind::Model {
                    let stem = dropped.path.file_stem().unwrap_or_default();
                    let called = Name(stem.to_string_lossy().into_owned());
                    vec![
                        ("Name".into(), called.to_value()),
                        ("Transform".into(), transform),
                        ("ModelRoot".into(), root),
                    ]
                } else {
                    // Named as its prefab's root is, which it takes.
                    vec![("Transform".into(), transform), ("PrefabRoot".into(), root)]
                };
                self.view.link.create(components);
            }
            // The Assets panel drags none: nothing in the view takes a UI.
            AssetKind::Ui => {}
        }
    }

    /// Takes `found`, what the game's view shows where `dropped`, a texture or font, was let go.
    /// The Inspector picks the entity there and its sprite's texture or text's font is set through
    /// the Inspector's link as an edit of its own, which the game refuses for an entity without
    /// one, the Inspector showing why. Where no entity draws, the Logs say so.
    fn take_dropped(&mut self, dropped: Dropping, found: Option<Found>) {
        let Some(found) = found else {
            let said = format!("{} was let go where no entity draws", dropped.file);
            self.log(Level::Info, said);
            return;
        };
        let (component, field) = match dropped.kind {
            AssetKind::Font => ("Text", "font"),
            AssetKind::Texture | AssetKind::Model | AssetKind::Prefab | AssetKind::Ui => {
                ("Sprite", "texture")
            }
        };
        let link = &self.view.link;
        link.pick(Some(found.id));
        self.took_pick = None;
        let asset = reflect::Value::String(dropped.name);
        let asset = reflect::Value::Option(Some(Box::new(asset)));
        link.set(found.id, component, field, asset);
        link.seal();
    }

    /// Where a model let go at `at`, in the main window's pixels, lands, with the view at `view`
    /// and the window at `scale` pixels a point.
    ///
    /// - Where the view's last frame shows the grid's plane through the origin, as the grid would
    ///   lie, whether or not it shows.
    /// - Where the ray runs along the plane or meets it behind the camera: under `at` on the plane
    ///   through the editor's camera's pivot facing the view, which sees as the game's camera does
    ///   while that drives the view.
    ///
    /// `None` until a frame that projects has come.
    fn landing(&self, at: [f64; 2], view: Option<Viewport>, scale: f32) -> Option<DVec3> {
        let (seen, screen) = self.seen.zip(self.screen(view))?;
        let plane = grid::shown(&seen, self.view.toolbar.plane);
        let sight = Sight::new(seen, screen, scale)?;
        let pixel = Vec2::new(at[0] as f32, at[1] as f32);
        let met = sight.on_plane(pixel, plane.normal());
        Some(met.unwrap_or_else(|| self.view.camera.under(at, screen)))
    }

    /// Takes the answer to the Probe tool's probe if it's come, dropping those of replaced probes.
    /// Closes its numbers once another tool is picked.
    pub(super) fn take_probes(&mut self) {
        if self.view.toolbar.tool != Tool::Probe {
            self.probing = None;
        }
        let Some(talker) = &self.talker else {
            return;
        };
        let answers: Vec<_> = talker.probed.try_iter().collect();
        for (ticket, answer) in answers {
            if let Err(Failure { code, message }) = &answer {
                self.log(Level::Error, format!("`probe`: [{code}] {message}"));
            }
            if let Some(probing) = self
                .probing
                .as_mut()
                .filter(|probing| probing.ticket == ticket)
            {
                probing.answer = Some(answer);
            }
        }
    }

    /// Asks the game for the entity its view shows at `at`, in the main window's pixels, with the
    /// view's top left at `origin`, within the model of `within`, as [`gg_api::Pick::within`]
    /// says. Returns the ticket while a game serves.
    pub(super) fn ask_pick(
        &mut self,
        at: [f64; 2],
        origin: [u32; 2],
        within: Option<EntityId>,
    ) -> Option<u64> {
        let talker = self.talker.as_ref()?;
        let ticket = self.tickets;
        self.tickets += 1;
        talker.pick(ticket, in_view(at, origin), within);
        Some(ticket)
    }
}

/// What the game names the asset at `path` by: its ID, from the ID file beside it; or, without one
/// that reads (before the game's scan has written it), its path from the asset root, `root`, with
/// `/` between its parts, which the game loads it by, naming it by the ID its scan finds.
fn asset_name(root: Option<&Path>, path: &Path) -> String {
    if let Ok(meta) = Meta::read(&Meta::path(path)) {
        return meta.id.to_string();
    }
    let under = root.and_then(|root| path.strip_prefix(root).ok());
    let parts = under.unwrap_or(path).iter().map(OsStr::to_string_lossy);
    parts.collect::<Vec<_>>().join("/")
}

/// The active pick, as the handles see it, as `link` places it in the game's world: once the
/// others picked are placed too, as [`along`] gives them, so no drag leaves one behind. None while
/// it or another a drag would move is a model's node, which the game refuses to change.
pub(super) fn picked(link: &InspectorLink) -> Option<Picked> {
    let placed = link.placed()?;
    let along = link.along()?;
    if placed.node || along.iter().any(|other| other.node) {
        return None;
    }
    Some(Picked {
        entity: placed.id,
        serial: placed.generation,
        local: placed.local,
        parent: placed.space,
        stepped: link.stepped(),
        waiting: link.waiting(),
        moves: along.iter().any(|other| other.id == placed.id),
    })
}

/// The others picked that a drag of the handles moves with the active one, as `link` places them:
/// those with no picked ancestor, as [`InspectorLink::along`] says.
pub(super) fn along(link: &InspectorLink) -> Vec<Along> {
    let active = link.picked();
    let along = link.along().unwrap_or_default().into_iter();
    let others = along.filter(|placed| Some(placed.id) != active);
    let others = others.map(|placed| Along {
        entity: placed.id,
        local: placed.local,
        parent: placed.space,
    });
    others.collect()
}

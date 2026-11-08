//! The Entities panel: a row for each of the world's entities, of those with one component as a
//! list or of them all as the tree of its hierarchy, of which a frame finds only those in view.

use std::collections::HashSet;
use std::ops::Range;

use super::commands::Command;
use super::inspected::Inspected;
use super::{Inspector, Pass, empty};
use crate::EntityId;

impl Inspector {
    /// The Entities panel: a field that finds an entity by its ID, which component the entities
    /// listed have, how many there are, and a row for each of them, or for each of the tree's, of
    /// which only those in view are drawn; and below them, where they leave room, space whose
    /// click unpicks, and whose context menu makes a root. The operations its menus choose, and
    /// what the input of the window it draws in says of the edits, go in `pass`.
    pub(super) fn entities(&mut self, world: &dyn Inspected, ui: &mut egui::Ui, pass: &mut Pass) {
        pass.cues.read(ui.ctx());
        if let Some(waiting) = world.waiting() {
            ui.label(waiting);
        }
        ui.horizontal(|ui| {
            ui.label("Find");
            let field = egui::TextEdit::singleline(&mut self.find).hint_text("an entity's ID");
            let field = ui.add(field);
            if field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                self.not_found = self.find(world).err();
            }
        });
        if let Some(not_found) = &self.not_found {
            ui.label(not_found.as_str());
        }
        egui::ComboBox::from_label("With")
            .selected_text(self.with.as_deref().unwrap_or("Any component"))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.with, None, "Any component");
                for name in world.component_types() {
                    let picked = self.with.as_deref() == Some(name);
                    if ui.selectable_label(picked, name).clicked() {
                        self.with = Some(name.to_owned());
                    }
                }
            });
        let count = match world.count(self.with.as_deref()) {
            Ok(count) => count,
            Err(error) => {
                ui.label(error);
                return;
            }
        };
        ui.label(match count {
            1 => "1 entity".to_owned(),
            n => format!("{n} entities"),
        });
        let height = ui.spacing().interact_size.y;
        let (operations, saves) = (&mut pass.operations, &mut pass.saves);
        let scroll = egui::ScrollArea::vertical();
        if self.with.is_some() {
            scroll.show_rows(ui, height, count, |ui, shown| {
                for id in world.page(self.with.as_deref(), shown) {
                    self.row(world, ui, id, None, operations, saves);
                }
            });
        } else {
            let tree = Tree::new(world, &self.expanded);
            scroll.show_rows(ui, height, tree.len, |ui, shown| {
                for (id, depth) in tree.page(world, shown) {
                    self.row(world, ui, id, Some(depth), operations, saves);
                }
            });
        }
        // Nothing is behind the rows, so that a click between two of them doesn't unpick.
        let below = ui.allocate_rect(ui.available_rect_before_wrap(), egui::Sense::click());
        if below.clicked() {
            self.unpick();
        }
        let menu = egui::Popup::context_menu(&below).style(crate::menu_style);
        menu.show(|ui| {
            if ui.button("Create Empty Entity").clicked() {
                operations.push(empty(None));
            }
        });
    }

    /// Draws the row of the entity of ID `id`: its label, across the rest of the row, which picks
    /// it, and whose context menu makes an empty child of it, duplicates it or deletes it, the
    /// operation chosen going in `operations`, or, where `world` saves prefabs, saves it and its
    /// descendants as a prefab, linked in their place, the entity going in `saves`; in the tree, at
    /// `depth`, after an arrow that shows and hides its children's rows, if it has any.
    fn row(
        &mut self,
        world: &dyn Inspected,
        ui: &mut egui::Ui,
        id: EntityId,
        depth: Option<usize>,
        operations: &mut Vec<Command>,
        saves: &mut Vec<EntityId>,
    ) {
        // The entity's own, not its row's, so that its widgets' IDs go with it as rows change, as
        // tables swap theirs: a context menu open on it acts on it, not on the entity that comes
        // to its row.
        let scope = egui::UiBuilder::new().scope_id(ui.make_persistent_id(id));
        ui.scope_builder(scope, |ui| {
            ui.horizontal(|ui| {
                if let Some(depth) = depth {
                    self.arrow(world, ui, id, depth);
                }
                let picked = self.picked == Some(id);
                // On one line, as a horizontal row has it, since the rows' paging takes each to be
                // one tall: a vertical layout would wrap it.
                let label = egui::Button::selectable(picked, row_label(world, id))
                    .wrap_mode(egui::TextWrapMode::Extend);
                let across = egui::Layout::top_down_justified(egui::Align::LEFT);
                let label = ui.with_layout(across, |ui| ui.add(label)).inner;
                if label.clicked() {
                    self.pick(id);
                }
                let menu = egui::Popup::context_menu(&label).style(crate::menu_style);
                menu.show(|ui| {
                    if ui.button("Create Empty Child").clicked() {
                        operations.push(empty(Some(id)));
                    }
                    if ui.button("Duplicate Entity").clicked() {
                        operations.push(Command::Duplicate(id));
                    }
                    if ui.button("Delete Entity").clicked() {
                        operations.push(Command::Delete(id));
                    }
                    if world.saves_prefabs() {
                        let save = ui.button("Save as Prefab").on_hover_text(
                            "Saves it and its descendants as a prefab in the project's prefabs \
                             folder, named after it, and makes it an instance of that prefab, its \
                             entities keeping their IDs",
                        );
                        if save.clicked() {
                            saves.push(id);
                        }
                    }
                });
            });
        });
    }

    /// Indents a row of the tree to `depth`, and draws the arrow of the entity of ID `id`, if it
    /// has children, which shows and hides their rows; or, if it hasn't, space as wide, so that the
    /// labels line up.
    fn arrow(&mut self, world: &dyn Inspected, ui: &mut egui::Ui, id: EntityId, depth: usize) {
        let size = egui::vec2(ui.spacing().indent, ui.spacing().interact_size.y);
        ui.add_space(depth as f32 * size.x);
        if world.children(id).next().is_none() {
            ui.allocate_exact_size(size, egui::Sense::hover());
            return;
        }
        let (rect, arrow) = ui.allocate_exact_size(size, egui::Sense::click());
        if arrow.clicked() && !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
        // Where a collapsing header places its own.
        let (mut icon, _) = ui.spacing().icon_rectangles(rect);
        icon.set_center(egui::pos2(rect.left() + size.x / 2.0, rect.center().y));
        let openness = if self.expanded.contains(&id) {
            1.0
        } else {
            0.0
        };
        egui::collapsing_header::paint_default_icon(ui, openness, &arrow.with_new_rect(icon));
    }

    /// Picks the entity whose ID is in the Find field, which may be quoted, as a scene writes it,
    /// and opens the rows of its ancestors; or says why it finds none.
    fn find(&mut self, world: &dyn Inspected) -> Result<(), String> {
        let text = self.find.trim().trim_matches('"');
        let id: EntityId = text
            .parse()
            .map_err(|error: crate::Error| error.to_string())?;
        if world.alive(id) != Some(true) {
            return Err(format!("no entity has ID {id}"));
        }
        self.pick(id);
        self.open_ancestors(world, id);
        Ok(())
    }
}

/// The rows of the Entities panel's tree, for a frame: the roots, in the order [`World::roots`]
/// lists them, each open one followed by the rows of its descendants, depth first in the order of
/// their parents' children, as far down as their parents' rows are open. Only the open roots'
/// rows are listed, and only the page in view found, so that a frame costs a few steps a table, a
/// step a table for each open root, one for each row under an open root, and one for each row of
/// the page: never one for each root.
///
/// [`World::roots`]: crate::World::roots
pub(super) struct Tree {
    /// Each open root's place among the roots, in order, and how many rows are under it.
    pub(super) spans: Vec<(usize, usize)>,
    /// The rows under each open root, in the order of `spans`: each entity, and its depth.
    under: Vec<Vec<(EntityId, usize)>>,
    /// How many rows there are.
    pub(super) len: usize,
}

impl Tree {
    /// The tree of `world`, whose open rows `expanded` holds, by their entities' IDs.
    pub(super) fn new(world: &dyn Inspected, expanded: &HashSet<EntityId>) -> Self {
        let open = expanded
            .iter()
            .filter_map(|&id| Some((world.root_index(id)?, id)));
        let mut open: Vec<(usize, EntityId)> = open.collect();
        open.sort_unstable_by_key(|&(at, _)| at);
        let under: Vec<Vec<(EntityId, usize)>> = open
            .iter()
            .map(|&(_, root)| descendants(world, expanded, root))
            .collect();
        let spans = open.iter().zip(&under);
        let spans = spans.map(|(&(at, _), rows)| (at, rows.len())).collect();
        let len = world.roots() + under.iter().map(Vec::len).sum::<usize>();
        Self { spans, under, len }
    }

    /// The rows `shown`: each entity, and its depth.
    pub(super) fn page(
        &self,
        world: &dyn Inspected,
        shown: Range<usize>,
    ) -> Vec<(EntityId, usize)> {
        let places: Vec<Place> = shown.map(|row| locate(row, &self.spans)).collect();
        // The page's roots, which are next to each other among the roots, in one go.
        let mut at = places.iter().filter_map(|place| match *place {
            Place::Root(at) => Some(at),
            Place::Under(..) => None,
        });
        let first = at.next();
        let roots = match first {
            Some(first) => {
                let last = at.next_back().unwrap_or(first);
                world.roots_page(first..last + 1)
            }
            None => Vec::new(),
        };
        let place = |place: &Place| match *place {
            Place::Root(at) => {
                let root = roots.get(at - first.expect("a root's place"));
                Some((*root?, 0))
            }
            Place::Under(span, offset) => Some(self.under[span][offset]),
        };
        // A root gone since the tree was found leaves the page short.
        places.iter().filter_map(place).collect()
    }
}

/// The rows under `root`: its descendants, depth first in the order of their parents' children,
/// each with its depth, as far down as `expanded` holds their parents' IDs. An entity met again,
/// as a hierarchy put together from reads of a game's world at two moments may have it, its own
/// ancestor, isn't opened again.
fn descendants(
    world: &dyn Inspected,
    expanded: &HashSet<EntityId>,
    root: EntityId,
) -> Vec<(EntityId, usize)> {
    let mut rows = Vec::new();
    let mut opened = HashSet::from([root]);
    // The children left at each depth, from the root's down.
    let mut stack = vec![world.children(root)];
    while let Some(children) = stack.last_mut() {
        let Some(child) = children.next() else {
            stack.pop();
            continue;
        };
        rows.push((child, stack.len()));
        if expanded.contains(&child) && opened.insert(child) {
            stack.push(world.children(child));
        }
    }
    rows
}

/// Where a row of the tree is.
enum Place {
    /// The root at this place among the roots.
    Root(usize),
    /// The row at this offset under the open root of this span.
    Under(usize, usize),
}

/// Where row `row` of a tree is, whose `spans` are each open root's place among the roots, in
/// order, and how many rows are under it: each open root's rows follow it, and push the roots
/// after it down.
fn locate(row: usize, spans: &[(usize, usize)]) -> Place {
    // How many rows under open roots come before.
    let mut before = 0;
    for (span, &(at, n)) in spans.iter().enumerate() {
        let start = at + before;
        if row <= start {
            return Place::Root(row - before);
        }
        if row <= start + n {
            return Place::Under(span, row - start - 1);
        }
        before += n;
    }
    Place::Root(row - before)
}

/// What the row of the entity of ID `id` in the Entities panel says: its name alone, leaving its
/// ID and components to the Inspector; or, without a name, or with an empty one, "Entity", faintly.
pub(super) fn row_label(world: &dyn Inspected, id: EntityId) -> egui::WidgetText {
    match world.name(id).filter(|name| !name.trim().is_empty()) {
        Some(name) => name.into(),
        None => egui::RichText::new("Entity").weak().into(),
    }
}

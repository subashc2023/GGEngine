//! The Entities panel: a row for each of the world's entities, of those with one component as a
//! list or of them all as the tree of its hierarchy, of which a frame finds only those in view.

use std::collections::HashSet;
use std::ops::Range;

use super::commands::Command;
use super::{Inspector, Pass, empty, entity_name};
use crate::{Entity, EntityId, World};

impl Inspector {
    /// The Entities panel: a field that finds an entity by its ID, which component the entities
    /// listed have, how many there are, and a row for each of them, or for each of the tree's, of
    /// which only those in view are drawn; and below them, where they leave room, space whose
    /// click unpicks, and whose context menu makes a root. The operations its menus choose, and
    /// what the input of the window it draws in says of the edits, go in `pass`.
    pub(super) fn entities(&mut self, world: &World, ui: &mut egui::Ui, pass: &mut Pass) {
        pass.cues.read(ui.ctx());
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
        let entities = match world.entities(self.with.as_deref()) {
            Ok(entities) => entities,
            Err(error) => {
                ui.label(error.to_string());
                return;
            }
        };
        ui.label(match entities.len() {
            1 => "1 entity".to_owned(),
            n => format!("{n} entities"),
        });
        let height = ui.spacing().interact_size.y;
        let operations = &mut pass.operations;
        let scroll = egui::ScrollArea::vertical();
        if self.with.is_some() {
            let rows = entities.len();
            scroll.show_rows(ui, height, rows, |ui, shown| {
                for entity in entities.skip(shown.start).take(shown.len()) {
                    self.row(world, ui, entity, None, operations);
                }
            });
        } else {
            let tree = Tree::new(world, &self.expanded);
            scroll.show_rows(ui, height, tree.len, |ui, shown| {
                for (entity, depth) in tree.page(world, shown) {
                    self.row(world, ui, entity, Some(depth), operations);
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

    /// Draws `entity`'s row: its label, across the rest of the row, which picks it, and whose
    /// context menu makes an empty child of it, duplicates it or deletes it, the operation chosen
    /// going in `operations`; in the tree, at `depth`, after an arrow that shows and hides its
    /// children's rows, if it has any.
    fn row(
        &mut self,
        world: &World,
        ui: &mut egui::Ui,
        entity: Entity,
        depth: Option<usize>,
        operations: &mut Vec<Command>,
    ) {
        let id = world.id(entity).expect("the world lists living entities");
        // The entity's own, not its row's, so that its widgets' IDs go with it as rows change, as
        // tables swap theirs: a context menu open on it acts on it, not on the entity that comes
        // to its row.
        let scope = egui::UiBuilder::new().scope_id(ui.make_persistent_id(id));
        ui.scope_builder(scope, |ui| {
            ui.horizontal(|ui| {
                if let Some(depth) = depth {
                    self.arrow(world, ui, entity, id, depth);
                }
                let picked = self.picked == Some(id);
                // On one line, as a horizontal row has it, since the rows' paging takes each to be
                // one tall: a vertical layout would wrap it.
                let label = egui::Button::selectable(picked, row_label(world, entity))
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
                });
            });
        });
    }

    /// Indents a row of the tree to `depth`, and draws the arrow of `entity`, whose ID is `id`, if
    /// it has children, which shows and hides their rows; or, if it hasn't, space as wide, so that
    /// the labels line up.
    fn arrow(
        &mut self,
        world: &World,
        ui: &mut egui::Ui,
        entity: Entity,
        id: EntityId,
        depth: usize,
    ) {
        let size = egui::vec2(ui.spacing().indent, ui.spacing().interact_size.y);
        ui.add_space(depth as f32 * size.x);
        let parent = world
            .children(entity)
            .is_ok_and(|mut children| children.next().is_some());
        if !parent {
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
    fn find(&mut self, world: &World) -> Result<(), String> {
        let text = self.find.trim().trim_matches('"');
        let id: EntityId = text
            .parse()
            .map_err(|error: crate::Error| error.to_string())?;
        let Some(entity) = world.entity(id) else {
            return Err(format!("no entity has ID {id}"));
        };
        self.pick(id);
        self.open_ancestors(world, entity);
        Ok(())
    }
}

/// The rows of the Entities panel's tree, for a frame: the roots, in the order [`World::roots`]
/// lists them, each open one followed by the rows of its descendants, depth first in the order of
/// their parents' children, as far down as their parents' rows are open. Only the open roots'
/// rows are listed, and only the page in view found, so that a frame costs a few steps a table, a
/// step a table for each open root, one for each row under an open root, and one for each row of
/// the page: never one for each root.
pub(super) struct Tree {
    /// Each open root's place among the roots, in order, and how many rows are under it.
    pub(super) spans: Vec<(usize, usize)>,
    /// The rows under each open root, in the order of `spans`: each entity, and its depth.
    under: Vec<Vec<(Entity, usize)>>,
    /// How many rows there are.
    pub(super) len: usize,
}

impl Tree {
    /// The tree of `world`, whose open rows `expanded` holds, by their entities' IDs.
    pub(super) fn new(world: &World, expanded: &HashSet<EntityId>) -> Self {
        let open = expanded.iter().filter_map(|&id| {
            let entity = world.entity(id)?;
            Some((world.root_index(entity)?, entity))
        });
        let mut open: Vec<(usize, Entity)> = open.collect();
        open.sort_unstable_by_key(|&(at, _)| at);
        let under: Vec<Vec<(Entity, usize)>> = open
            .iter()
            .map(|&(_, root)| descendants(world, expanded, root))
            .collect();
        let spans = open.iter().zip(&under);
        let spans = spans.map(|(&(at, _), rows)| (at, rows.len())).collect();
        let len = world.roots().len() + under.iter().map(Vec::len).sum::<usize>();
        Self { spans, under, len }
    }

    /// The rows `shown`: each entity, and its depth.
    pub(super) fn page(&self, world: &World, shown: Range<usize>) -> Vec<(Entity, usize)> {
        // One iterator of the roots for the page, since their places only rise down it, which
        // skips a table at a time; and the place of the root it gives next.
        let mut roots = world.roots();
        let mut next = 0;
        shown
            .map(|row| match locate(row, &self.spans) {
                Place::Root(at) => {
                    let root = roots
                        .nth(at - next)
                        .expect("the tree's roots are the world's");
                    next = at + 1;
                    (root, 0)
                }
                Place::Under(span, offset) => self.under[span][offset],
            })
            .collect()
    }
}

/// The rows under `root`, which is alive: its descendants, depth first in the order of their
/// parents' children, each with its depth, as far down as `expanded` holds their parents' IDs.
fn descendants(world: &World, expanded: &HashSet<EntityId>, root: Entity) -> Vec<(Entity, usize)> {
    let mut rows = Vec::new();
    // The children left at each depth, from the root's down.
    let mut stack = vec![world.children(root).expect("an open root is alive")];
    while let Some(children) = stack.last_mut() {
        let Some(child) = children.next() else {
            stack.pop();
            continue;
        };
        rows.push((child, stack.len()));
        if expanded.contains(&world.id(child).expect("a child is alive")) {
            stack.push(world.children(child).expect("a child is alive"));
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

/// What `entity`'s row in the Entities panel says: its name alone, leaving its ID and components
/// to the Inspector; or, without a name, or with an empty one, "Entity", faintly.
pub(super) fn row_label(world: &World, entity: Entity) -> egui::WidgetText {
    match entity_name(world, entity).filter(|name| !name.trim().is_empty()) {
        Some(name) => name.into(),
        None => egui::RichText::new("Entity").weak().into(),
    }
}

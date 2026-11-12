//! The Entities panel: a row for each of the world's entities, of those with one component as a
//! list or of them all as the tree of its hierarchy, of which a frame finds only those in view.

use std::collections::HashSet;
use std::mem;
use std::ops::Range;

use super::commands::Command;
use super::inspected::Inspected;
use super::{DELETE, DUPLICATE, FRAME, Inspector, Pass, RENAME, Renaming, empty};
use crate::EntityId;

/// An entity's row dragged, as egui's drag and drop holds it: a row it's let go on makes it a
/// child of that row's entity, and the space below the rows a root.
struct Moved(EntityId);

/// The side of the header's glyphs, in points.
const GLYPH: f32 = 12.0;

/// A glyph of the header's buttons, painted, as neither Inter nor egui's fonts have it.
#[derive(Clone, Copy)]
enum Glyph {
    /// A magnifier: a ring, and its handle, down to the right.
    Find,
    /// Two chevrons pointing up, one over the other, as rows folding away.
    Collapse,
}

impl Glyph {
    /// Paints it in `rect`, a square, in `color`.
    fn paint(self, painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
        let scale = rect.width() / GLYPH;
        // From the top left of a square 12 across.
        let at = |x: f32, y: f32| rect.min + egui::vec2(x, y) * scale;
        let line = egui::Stroke::new(1.5 * scale, color);
        match self {
            Self::Find => {
                painter.circle_stroke(at(5.0, 5.0), 3.75 * scale, line);
                let handle = egui::Stroke::new(2.0 * scale, color);
                painter.line_segment([at(7.75, 7.75), at(11.0, 11.0)], handle);
            }
            Self::Collapse => {
                for y in [1.5, 6.5] {
                    let chevron = vec![at(2.0, y + 4.0), at(6.0, y), at(10.0, y + 4.0)];
                    painter.add(egui::Shape::line(chevron, line));
                }
            }
        }
    }
}

/// A button showing `glyph`, square, as tall as one with text, so that the header leaves its
/// component filter the room a text's padding across would take.
fn glyph_button(ui: &mut egui::Ui, glyph: Glyph) -> egui::Response {
    let atom = egui::Atom::paint(egui::Vec2::splat(GLYPH), move |ui, args| {
        glyph.paint(ui.painter(), args.rect, args.fallback_text_color);
    });
    let size = egui::Vec2::splat(ui.spacing().interact_size.y);
    let button = |ui: &mut egui::Ui| {
        ui.spacing_mut().button_padding.x = ((size.x - GLYPH) / 2.0).max(0.0);
        ui.add(egui::Button::new(atom).min_size(size))
    };
    ui.scope(button).inner
}

impl Inspector {
    /// The Entities panel: a header of one row, and a row for each of the entities listed, or for
    /// each of the tree's, of which only those in view are drawn; and below them, where they leave
    /// room, space whose click unpicks, whose context menu makes a root, and which a row let go on
    /// makes a root. The header has the magnifier, which opens the Find field in place of the
    /// rest of it, as [`find_field`](Self::find_field) says; which component the entities listed
    /// have, taking the room the rest leaves, its text cut short to fit; how many there are; and,
    /// for the tree, Collapse All, and the + menu. Its keys, while it has them, frame, rename and
    /// delete the entity picked. The operations its menus, keys and drags choose, and what the
    /// input of the window it draws in says of the edits, go in `pass`.
    pub(super) fn entities(&mut self, world: &dyn Inspected, ui: &mut egui::Ui, pass: &mut Pass) {
        pass.cues.read(ui.ctx());
        let (operations, saves) = (&mut pass.operations, &mut pass.saves);
        self.take_keys(world, ui, operations);
        // The rename of an entity no longer picked is left, as Escape leaves it.
        if (self.renaming.as_ref()).is_some_and(|renaming| self.picked != Some(renaming.id)) {
            self.renaming = None;
        }
        if let Some(waiting) = world.waiting() {
            ui.label(waiting);
        }
        // Counted before the header picks a component, whose count the rows take below.
        let counted = world.count(self.with.as_deref());
        ui.horizontal(|ui| {
            if self.finding {
                self.find_field(world, ui);
                return;
            }
            let find = glyph_button(ui, Glyph::Find)
                .on_hover_text("Find an entity by its ID or a part of its name");
            if find.clicked() {
                (self.finding, self.focus_find) = (true, true);
            }
            let right = egui::Layout::right_to_left(egui::Align::Center);
            ui.with_layout(right, |ui| {
                self.header(world, ui, operations);
                if let Ok(count) = counted {
                    ui.label(match count {
                        1 => "1 entity".to_owned(),
                        n => format!("{n} entities"),
                    });
                }
                self.with_menu(world, ui);
            });
        });
        if let Some(not_found) = &self.not_found {
            ui.label(not_found.as_str());
        }
        let count = match world.count(self.with.as_deref()) {
            Ok(count) => count,
            Err(error) => {
                ui.label(error);
                return;
            }
        };
        let height = ui.spacing().interact_size.y;
        let scroll = egui::ScrollArea::vertical();
        // The row a click with Shift held picks the rows up to, from the active pick's.
        let mut ranged = None;
        if self.with.is_some() {
            scroll.show_rows(ui, height, count, |ui, shown| {
                for id in world.page(self.with.as_deref(), shown) {
                    ranged = ranged.or(self.row(world, ui, id, None, operations, saves));
                }
            });
            if let Some(to) = ranged {
                let rows = world.page(self.with.as_deref(), 0..count);
                self.pick_rows(&rows, to);
            }
        } else {
            let tree = Tree::new(world, &self.expanded);
            scroll.show_rows(ui, height, tree.len, |ui, shown| {
                for (id, depth) in tree.page(world, shown) {
                    let shifted = self.row(world, ui, id, Some(depth), operations, saves);
                    ranged = ranged.or(shifted);
                }
            });
            if let Some(to) = ranged {
                let rows = tree.page(world, 0..tree.len);
                let rows: Vec<EntityId> = rows.into_iter().map(|(id, _)| id).collect();
                self.pick_rows(&rows, to);
            }
        }
        // Nothing is behind the rows, so that a click between two of them doesn't unpick.
        let below = ui.allocate_rect(ui.available_rect_before_wrap(), egui::Sense::click());
        if below.clicked() {
            self.unpick();
        }
        self.drop_on(world, ui, &below, None, operations);
        let menu = egui::Popup::context_menu(&below).style(crate::menu_style);
        menu.show(|ui| {
            if ui.button("Create Empty Entity").clicked() {
                operations.push(empty(None));
            }
        });
    }

    /// The Find field, in place of the rest of the header, taking the keys as it opens, and its ×:
    /// each Enter there finds the next entity whose ID or a part of whose name is typed, as
    /// [`find`](Self::find) says; the × and Escape close it, emptying it.
    fn find_field(&mut self, world: &dyn Inspected, ui: &mut egui::Ui) {
        let right = egui::Layout::right_to_left(egui::Align::Center);
        ui.with_layout(right, |ui| {
            let close = ui.button("×").on_hover_text("Close the Find field");
            let field = egui::TextEdit::singleline(&mut self.find)
                .hint_text("ID or name")
                .desired_width(ui.available_width());
            let field = ui.add(field);
            if mem::take(&mut self.focus_find) {
                field.request_focus();
            }
            let pressed = |key| field.lost_focus() && ui.input(|input| input.key_pressed(key));
            if pressed(egui::Key::Enter) {
                self.not_found = self.find(world).err();
            }
            if close.clicked() || pressed(egui::Key::Escape) {
                self.finding = false;
                self.find.clear();
                self.not_found = None;
            }
        });
    }

    /// Which component the entities listed have, or All, for all of them, as a tree, taking the
    /// room left of what `ui` holds already, laid out from its right, its text cut short to fit.
    fn with_menu(&mut self, world: &dyn Inspected, ui: &mut egui::Ui) {
        egui::ComboBox::from_id_salt("entities with")
            .width(ui.available_width())
            .truncate()
            .selected_text(self.with.as_deref().unwrap_or("All"))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.with, None, "All");
                for name in world.component_types() {
                    let picked = self.with.as_deref() == Some(name);
                    if ui.selectable_label(picked, name).clicked() {
                        self.with = Some(name.to_owned());
                    }
                }
            })
            .response
            .on_hover_text("Lists all the entities, as a tree, or those with the component picked");
    }

    /// The end of the header, from its right: the + menu, which creates an empty root, or, while
    /// the entity picked is alive, an empty child of it, the create going in `operations`; and,
    /// for the tree, Collapse All, which hides the rows under every root.
    fn header(&mut self, world: &dyn Inspected, ui: &mut egui::Ui, operations: &mut Vec<Command>) {
        let live = self.picked.filter(|&id| world.alive(id) == Some(true));
        let config = egui::menu::MenuConfig::new().style(crate::menu_style);
        let create = egui::menu::MenuButton::new("+").config(config);
        let (create, _) = create.ui(ui, |ui| {
            if ui.button("Create Empty Entity").clicked() {
                operations.push(empty(None));
            }
            let child = ui.add_enabled(live.is_some(), egui::Button::new("Create Empty Child"));
            if let Some(id) = live.filter(|_| child.clicked()) {
                operations.push(empty(Some(id)));
            }
        });
        create.on_hover_text("Creates an entity");
        if self.with.is_none() {
            let collapse = ui.add_enabled_ui(!self.expanded.is_empty(), |ui| {
                glyph_button(ui, Glyph::Collapse)
            });
            if collapse
                .inner
                .on_hover_text("Collapse All: hides the rows under every root")
                .clicked()
            {
                self.expanded.clear();
            }
        }
    }

    /// Draws the row of the entity of ID `id`: its label, across the rest of the row, which picks
    /// it alone, with Ctrl (Cmd on macOS) held adds it to what's picked or takes it out, and with
    /// Shift held returns its ID, for the rows from the active pick's to it to be picked; a double
    /// click frames it in the view too; and which drags, a row let go on it making
    /// the entity dragged its child; and whose context menu frames it in the view, while a layer
    /// takes frames, renames it, copies its ID, makes an empty child of it, duplicates it, makes
    /// it a root, if it isn't one, or deletes it, the operation chosen going in `operations`, or,
    /// where `world` saves prefabs, saves it and its descendants as a prefab, linked in their
    /// place, the entity going in `saves`: Frame in View, Duplicate Entity and Delete Entity act
    /// on every entity picked if it's one of them. While it's renamed, the field its name is typed
    /// into in place of its label; in the tree, at `depth`, after an arrow that shows and hides its
    /// children's rows, if it has any.
    fn row(
        &mut self,
        world: &dyn Inspected,
        ui: &mut egui::Ui,
        id: EntityId,
        depth: Option<usize>,
        operations: &mut Vec<Command>,
        saves: &mut Vec<EntityId>,
    ) -> Option<EntityId> {
        // The entity's own, not its row's, so that its widgets' IDs go with it as rows change, as
        // tables swap theirs: a context menu open on it acts on it, not on the entity that comes
        // to its row.
        let scope = egui::UiBuilder::new().scope_id(ui.make_persistent_id(id));
        let mut ranged = None;
        ui.scope_builder(scope, |ui| {
            ui.horizontal(|ui| {
                if let Some(depth) = depth {
                    self.arrow(world, ui, id, depth);
                }
                let across = egui::Layout::top_down_justified(egui::Align::LEFT);
                if (self.renaming.as_ref()).is_some_and(|renaming| renaming.id == id) {
                    ui.with_layout(across, |ui| self.rename_field(world, ui, operations));
                    return;
                }
                let picked = self.is_picked(id);
                // On one line, as a horizontal row has it, since the rows' paging takes each to be
                // one tall: a vertical layout would wrap it.
                let label = egui::Button::selectable(picked, row_label(world, id))
                    .wrap_mode(egui::TextWrapMode::Extend);
                // Dragged as well as clicked, as one widget: one that senses drags alone over it
                // would take its clicks.
                let label = ui.with_layout(across, |ui| ui.add(label)).inner;
                let label = label.interact(egui::Sense::drag());
                label.dnd_set_drag_payload(Moved(id));
                if label.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                }
                if label.clicked() {
                    let modifiers = ui.input(|input| input.modifiers);
                    if modifiers.command {
                        self.toggle(id);
                    } else if modifiers.shift {
                        ranged = Some(id);
                    } else {
                        self.pick(id);
                    }
                }
                if label.double_clicked() {
                    self.frame(id);
                }
                self.drop_on(world, ui, &label, Some(id), operations);
                let menu = egui::Popup::context_menu(&label).style(crate::menu_style);
                menu.show(|ui| {
                    let item = |ui: &mut egui::Ui, name: &str, shortcut| {
                        let shortcut = ui.ctx().format_shortcut(shortcut);
                        ui.add(egui::Button::new(name).shortcut_text(shortcut))
                    };
                    let frames = self.link.borrow().frames;
                    if frames && item(ui, "Frame in View", &FRAME).clicked() {
                        self.frame(id);
                    }
                    if item(ui, "Rename", &RENAME).clicked() {
                        self.rename(world, id);
                    }
                    if ui.button("Copy ID").clicked() {
                        ui.ctx().copy_text(id.to_string());
                    }
                    ui.separator();
                    if ui.button("Create Empty Child").clicked() {
                        operations.push(empty(Some(id)));
                    }
                    if item(ui, "Duplicate Entity", &DUPLICATE).clicked() {
                        operations.push(Command::Duplicate(self.acted(world, id)));
                    }
                    if world.parent(id).is_some() && ui.button("Move to Root").clicked() {
                        operations.push(Command::Reparent(id, None));
                    }
                    if item(ui, "Delete Entity", &DELETE).clicked() {
                        operations.push(Command::Delete(self.acted(world, id)));
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
        ranged
    }

    /// What a row's Duplicate Entity and Delete Entity act on, the row of the entity of ID `id`:
    /// every entity picked, if it's one of them, but those under another picked, as the Entity
    /// menu's do; or else it alone.
    fn acted(&self, world: &dyn Inspected, id: EntityId) -> Vec<EntityId> {
        match self.is_picked(id) {
            true => self.uppermost(world),
            false => vec![id],
        }
    }

    /// Picks the rows `rows` list, the panel's in their order, from the active pick's to `to`'s,
    /// as a click with Shift held on `to`'s does, `to`'s the active one; or `to` alone if the
    /// active pick has no row there, as under a row that's closed. A click's, not a frame's, so
    /// that a list of every row is no frame's cost.
    fn pick_rows(&mut self, rows: &[EntityId], to: EntityId) {
        let at = |id: EntityId| rows.iter().position(|&row| row == id);
        let (Some(from), Some(end)) = (self.picked.and_then(at), at(to)) else {
            self.pick(to);
            return;
        };
        let range: Vec<EntityId> = match from <= end {
            true => rows[from..=end].to_vec(),
            false => rows[end..=from].iter().rev().copied().collect(),
        };
        self.pick_all(range);
    }

    /// The field the name of the entity being renamed is typed into, across its row: it takes the
    /// keys as it's first drawn, its text all selected, so that what's typed replaces it; then
    /// Enter, or a click elsewhere, names the entity what's typed, if that's another name, the
    /// rename going in `operations`, and Escape leaves it as it was.
    fn rename_field(
        &mut self,
        world: &dyn Inspected,
        ui: &mut egui::Ui,
        operations: &mut Vec<Command>,
    ) {
        let Some(renaming) = &mut self.renaming else {
            return;
        };
        let mut output = egui::TextEdit::singleline(&mut renaming.name).show(ui);
        if mem::take(&mut renaming.focus) {
            output.response.request_focus();
            let end = egui::text::CCursor::new(renaming.name.chars().count());
            let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), end);
            output.state.cursor.set_char_range(Some(all));
            output.state.store(ui.ctx(), output.response.id);
        }
        if !output.response.lost_focus() {
            return;
        }
        let escaped = ui.input(|input| input.key_pressed(egui::Key::Escape));
        let Renaming { id, name, .. } = self.renaming.take().expect("a rename");
        if !escaped && world.name(id).unwrap_or_default() != name {
            operations.push(Command::Rename(id, name));
        }
    }

    /// Takes the panel's keys at a press in it, `ui`'s whole, and lets them go at a press
    /// elsewhere; and while it has them, and no text field has the keys, has F frame the entities
    /// picked in the view, F2 rename the active one in its row, and Delete delete them, while the
    /// active one is alive, the delete going in `operations`.
    fn take_keys(&mut self, world: &dyn Inspected, ui: &egui::Ui, operations: &mut Vec<Command>) {
        // Where the pointer is, as a tap's release in the same frame forgets where it pressed.
        let pressed = ui.input(|input| {
            let at = input.pointer.interact_pos();
            at.filter(|_| input.pointer.any_pressed())
        });
        if let Some(at) = pressed {
            self.keys = ui.max_rect().contains(at);
        }
        let live = self.picked.filter(|&id| world.alive(id) == Some(true));
        let free = self.keys && !ui.ctx().text_edit_focused();
        let Some(id) = live.filter(|_| free) else {
            return;
        };
        let [frame, rename, delete] =
            ui.input_mut(|input| [FRAME, RENAME, DELETE].map(|key| input.consume_shortcut(&key)));
        if frame {
            self.frame(id);
        }
        if rename {
            self.rename(world, id);
        }
        if delete {
            operations.push(Command::Delete(self.uppermost(world)));
        }
    }

    /// Asks the link to frame what's picked in the view, as
    /// [`InspectorLink::framed`](super::InspectorLink::framed) says: every entity picked, if the
    /// entity of ID `id` is one of them, or else it, picked alone.
    fn frame(&mut self, id: EntityId) {
        if !self.is_picked(id) {
            self.pick(id);
        }
        self.link.borrow_mut().frame = self.picked;
    }

    /// Makes the entity of ID `id` the active pick, keeping the rest picked if it's one of them, or
    /// else picks it alone, and has its row show the field its name is typed into, holding its
    /// name, as `world` has it.
    fn rename(&mut self, world: &dyn Inspected, id: EntityId) {
        match self.is_picked(id) {
            true => self.add(id),
            false => self.pick(id),
        }
        let name = world.name(id).unwrap_or_default();
        self.renaming = Some(Renaming {
            id,
            name,
            focus: true,
        });
    }

    /// Makes the entity of a row let go on `target`, a row's label or the space below the rows,
    /// the last child of `parent`, the row's entity, opening its row, or a root for `None`, the
    /// reparent going in `operations`; and outlines `target` while a row is held over it, if its
    /// entity would go there: not onto itself, one of its descendants, or the parent it has.
    fn drop_on(
        &mut self,
        world: &dyn Inspected,
        ui: &egui::Ui,
        target: &egui::Response,
        parent: Option<EntityId>,
        operations: &mut Vec<Command>,
    ) {
        if let Some(moved) = target.dnd_release_payload::<Moved>() {
            if moves(world, moved.0, parent) {
                operations.push(Command::Reparent(moved.0, parent));
                self.expanded.extend(parent);
            }
            return;
        }
        let held = target.dnd_hover_payload::<Moved>();
        if held.is_some_and(|moved| moves(world, moved.0, parent)) {
            let stroke = ui.visuals().selection.stroke;
            (ui.painter()).rect_stroke(target.rect, 2.0, stroke, egui::StrokeKind::Inside);
        }
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
    /// or else whose name has what's there, ignoring case, the next such after the one picked at
    /// each Enter, as [`named`] finds it; and opens the rows of its ancestors; or says why it finds
    /// none.
    fn find(&mut self, world: &dyn Inspected) -> Result<(), String> {
        let text = self.find.trim().trim_matches('"');
        let id = match text.parse::<EntityId>() {
            Ok(id) if world.alive(id) == Some(true) => id,
            Ok(id) => return Err(format!("no entity has ID {id}")),
            Err(error) if text.is_empty() => return Err(error.to_string()),
            Err(error) => named(world, text, self.picked)
                .ok_or_else(|| format!("no entity's name has `{text}`, and {error}"))?,
        };
        self.pick(id);
        self.open_ancestors(world, id);
        Ok(())
    }
}

/// The entity whose name has `text`, ignoring case, next after `after` in the order queries find
/// them, and from the first again past the last, so that each Enter in the Find field finds the
/// next; `None` if no name has it.
fn named(world: &dyn Inspected, text: &str, after: Option<EntityId>) -> Option<EntityId> {
    let text = text.to_lowercase();
    let ids = world.page(None, 0..world.count(None).ok()?);
    let start = after.and_then(|after| ids.iter().position(|&id| id == after));
    let (before, from) = ids.split_at(start.map_or(0, |at| at + 1));
    let has =
        |id: &EntityId| (world.name(*id)).is_some_and(|name| name.to_lowercase().contains(&text));
    from.iter().chain(before).copied().find(has)
}

/// Whether making the entity of ID `moved` the last child of `parent`, or a root for `None`,
/// changes its parent and makes no cycle: `parent` neither it nor one of its descendants, up to
/// an ancestor met again, as a hierarchy put together from reads of a game's world at two moments
/// may have it.
fn moves(world: &dyn Inspected, moved: EntityId, parent: Option<EntityId>) -> bool {
    if world.parent(moved) == parent {
        return false;
    }
    let mut met = HashSet::new();
    let mut up = parent;
    while let Some(at) = up.filter(|&at| met.insert(at)) {
        if at == moved {
            return false;
        }
        up = world.parent(at);
    }
    true
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

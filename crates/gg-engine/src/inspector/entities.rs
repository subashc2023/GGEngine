//! The Entities panel: a row per entity, as a list of those with one component or as the tree of
//! all. A frame finds only the rows in view.

use std::collections::HashSet;
use std::mem;
use std::ops::Range;

use super::commands::Command;
use super::inspected::Inspected;
use super::{DELETE, DUPLICATE, FRAME, Inspector, Pass, RENAME, Renaming, empty};
use crate::EntityId;

/// An entity's row dragged, as egui's drag and drop holds it. Dropped on a row, it becomes that
/// entity's child; below the rows, a root.
struct Moved(EntityId);

/// The side of the header's glyphs, in points.
const GLYPH: f32 = 12.0;

/// A glyph of the header's buttons, painted, as neither Inter nor egui's fonts have it.
#[derive(Clone, Copy)]
enum Glyph {
    /// A magnifier.
    Find,
    /// Two chevrons pointing up, one over the other.
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

/// A square button showing `glyph`, as tall as one with text, so that the header leaves the
/// component filter the room a text button's padding would take.
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
    /// The Entities panel: a one-row header, a row per entity listed or in the tree (only those in
    /// view drawn), and the leftover space below.
    ///
    /// - The space unpicks on a click, makes a root from its context menu, and makes a root of a
    ///   row dropped on it.
    /// - The header has the magnifier (opens [`find_field`](Self::find_field) in place of the
    ///   rest), the component filter, the count, and, for the tree, Collapse All and the + menu.
    /// - While the panel has the keys, they frame, rename and delete the entity picked.
    ///
    /// Operations from its menus, keys and drags, and what its window's input says of edits, go in
    /// `pass`.
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

    /// The Find field and its ×, in place of the rest of the header, taking the keys as it opens.
    /// Each Enter finds the next match ([`find`](Self::find)); × and Escape close and empty it.
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

    /// The component filter, or All as a tree. Fills the room left in `ui`, text cut short.
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

    /// The header's right end: the + menu (an empty root, or child of the live picked entity) and,
    /// for the tree, Collapse All. Creates go in `operations`.
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

    /// Draws `id`'s row; in the tree, at `depth` after an arrow that shows and hides its
    /// children's rows. While renamed, the name's field replaces the label.
    ///
    /// The label, across the row:
    /// - A click picks it alone; Ctrl (Cmd on macOS) adds or removes it; Shift returns its ID, for
    ///   the rows from the active pick's to it to be picked.
    /// - A double click also frames it.
    /// - Dropping another row on it makes that entity its child.
    /// - Its context menu has Frame in View (while a layer takes frames), Rename, Copy ID, Create
    ///   Empty Child, Duplicate Entity, Move to Root (if not a root), Delete Entity (operations go
    ///   in `operations`) and, where `world` saves prefabs, Save as Prefab (the entity goes in
    ///   `saves`; it and its descendants are saved, linked in place). Frame in View, Duplicate and
    ///   Delete act on every entity picked if it's one.
    fn row(
        &mut self,
        world: &dyn Inspected,
        ui: &mut egui::Ui,
        id: EntityId,
        depth: Option<usize>,
        operations: &mut Vec<Command>,
        saves: &mut Vec<EntityId>,
    ) -> Option<EntityId> {
        // The entity's own ID, not its row's, so that a context menu open on it acts on it, not
        // on the entity that comes to its row as rows change.
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
                // One line, as the rows' paging takes each to be one tall.
                let label = egui::Button::selectable(picked, row_label(world, id))
                    .wrap_mode(egui::TextWrapMode::Extend);
                // One widget senses both: a drag-only one over it would take its clicks.
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

    /// What Duplicate and Delete on `id`'s row act on: the picked entities not under another
    /// picked, if `id` is picked; else `id` alone.
    fn acted(&self, world: &dyn Inspected, id: EntityId) -> Vec<EntityId> {
        match self.is_picked(id) {
            true => self.uppermost(world),
            false => vec![id],
        }
    }

    /// Shift click: picks `rows` (the panel's, in order) from the active pick's to `to`'s, `to`
    /// active; or `to` alone if the active pick has no row, as under a closed row. `rows` is built
    /// per click, so no frame pays for a list of all.
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

    /// The name field across the renamed entity's row, focused and all selected when first drawn.
    /// Enter or a click elsewhere renames it if the name differs (into `operations`); Escape
    /// leaves it.
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

    /// The panel takes the keys at a press in `ui` and loses them at a press elsewhere. While it
    /// has them, no text field does and the active entity is alive: F frames the picked entities,
    /// F2 renames the active one, Delete deletes them (into `operations`).
    fn take_keys(&mut self, world: &dyn Inspected, ui: &egui::Ui, operations: &mut Vec<Command>) {
        // A tap's release in the same frame forgets where it pressed.
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

    /// Asks the link to frame ([`InspectorLink::framed`](super::InspectorLink::framed)) the picked
    /// entities if `id` is one, else `id`, picked alone.
    fn frame(&mut self, id: EntityId) {
        if !self.is_picked(id) {
            self.pick(id);
        }
        self.link.borrow_mut().frame = self.picked;
    }

    /// Makes `id` the active pick (alone, unless it's already picked) and its row show the name
    /// field.
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

    /// A row dropped on `target` (a label or the space below the rows) becomes the last child of
    /// `parent`, opening its row, or a root for `None` (into `operations`). Outlines `target`
    /// while a row held over it would go there: not onto itself, a descendant or its parent.
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

    /// Indents a tree row to `depth` and draws `id`'s arrow, which shows and hides its children;
    /// with none, equal space, so labels line up.
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

    /// Picks the entity whose ID is in the Find field (quoted, as a scene writes it, or not), else
    /// the next [`named`] with the text. Opens its ancestors' rows; or says why none.
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

/// The next entity after `after`, in query order and wrapping, whose name has `text`, ignoring
/// case; `None` if none does.
fn named(world: &dyn Inspected, text: &str, after: Option<EntityId>) -> Option<EntityId> {
    let text = text.to_lowercase();
    let ids = world.page(None, 0..world.count(None).ok()?);
    let start = after.and_then(|after| ids.iter().position(|&id| id == after));
    let (before, from) = ids.split_at(start.map_or(0, |at| at + 1));
    let has =
        |id: &EntityId| (world.name(*id)).is_some_and(|name| name.to_lowercase().contains(&text));
    from.iter().chain(before).copied().find(has)
}

/// Whether making `moved` the last child of `parent` (a root for `None`) changes its parent and
/// makes no cycle. The walk up stops at an ancestor met again, which a hierarchy built from reads
/// at two moments may hold.
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

/// The tree's rows: the roots in [`World::roots`]' order, each open one followed by its open
/// descendants, depth first in children's order. Only open roots' rows are listed and only the
/// page in view found, so a frame costs a few steps a table, one per open root, per row under
/// one, and per page row: never one per root.
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
    /// The tree of `world`; `expanded` holds the open rows' entity IDs.
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
        // The page's roots are adjacent among the roots: one query.
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

/// The rows under `root`: its descendants with depths, depth first, as far down as `expanded`
/// holds their parents. An entity that is its own ancestor (a hierarchy built from reads at two
/// moments may hold one) isn't opened again.
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

/// Where row `row` is. `spans` are each open root's place among the roots and its row count; its
/// rows follow it and push later roots down.
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

/// What `id`'s row says: its name alone, leaving the ID and components to the Inspector; with no
/// name or an empty one, "Entity", faintly. Line breaks show as spaces, as `show_rows` takes each
/// row for a line.
pub(super) fn row_label(world: &dyn Inspected, id: EntityId) -> egui::WidgetText {
    match world.name(id).filter(|name| !name.trim().is_empty()) {
        Some(name) if name.contains(['\n', '\r']) => one_line(&name).into(),
        Some(name) => name.into(),
        None => egui::RichText::new("Entity").weak().into(),
    }
}

/// `text` on one line: each line break, `\r\n` too, a space.
fn one_line(text: &str) -> String {
    text.replace("\r\n", " ").replace(['\n', '\r'], " ")
}

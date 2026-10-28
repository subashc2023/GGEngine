//! The inspector: the Entities panel, which lists the world's entities as the tree of its
//! hierarchy, and creates and deletes them, and the Inspector panel, which shows the picked
//! entity's components, edits their fields, and adds and removes them. It reaches the world only
//! through its by-name calls, its IDs and its hierarchy, as the tool API will, so that an editor
//! can show it for a game in another process, and names entities by their names, where they have
//! one, and their IDs. Its edits and operations are its commands: the panels queue them, its next
//! tick makes them, and a recording keeps them.

use std::borrow::Cow;
use std::collections::HashSet;
use std::f64::consts::PI;
use std::mem;
use std::ops::Range;

use egui::color_picker::Alpha;
use egui::ecolor::{Hsva, HsvaGamma};
use egui::{KeyboardShortcut, Modifiers};
use gg_core::Result;
use gg_core::math::{self, EulerRot, Quat, Vec2, Vec3, Vec4};
use tracing::field::display;
use tracing::warn;

use crate::reflect::{self, Bound, Field, Meta, Name, Shape, Show, Type, Types, Value, Variant};
use crate::{
    Commands, Context, Entity, EntityId, Layer, Log, Operation, Reflect, Tick, Transform, Ui, World,
};

/// The Entities and Inspector panels, which an app adds as an overlay,
/// `.overlay(gg_engine::Inspector::default())`. The Entities panel lists the world's entities as
/// the tree of its hierarchy, whose rows' arrows show and hide their children, or those with one
/// component as a list, to pick one from, and finds one by an ID pasted into its Find field, from
/// a scene or a log, opening the rows above its own; a click below the rows, or the Entity menu,
/// unpicks. A row's context menu, the one below the rows and the Entity menu create empty
/// entities, which they pick, and delete entities, with their descendants. The Inspector panel
/// shows the picked entity's components, each field with a widget for its type and hints, and
/// sets a field as its widget changes it, through a [`Log`] of the edits, as [`World::set_field`]
/// does. That checks the value: a value refused shows beside its field, and is logged, as an
/// operation refused is, which shows above the components. An enum switches its variant from a
/// choice of them, to the fields that variant last had while the entity was drawn, or else to
/// their defaults; each of a vector's components, set at its own path, and each of a rotation's
/// angles has a button of its axis, which sets it to its default; the Add Component menu, below
/// the components, adds one with its type's default; and the menu beside each one's header
/// removes it. The Edit menu, Ctrl+Z (Cmd+Z on macOS) and Ctrl+Shift+Z or Ctrl+Y undo and redo
/// them, a drag, or a number typed, as one edit. The panels queue each frame's edits,
/// operations, undos and redos, which the Inspector's next tick makes, so that the world changes
/// only at ticks; until then, a field set is drawn as the set will make it, and a refusal, and the
/// Edit menu's names, show from the tick.
#[derive(Default)]
pub struct Inspector {
    /// By its ID, which outlives it, so that a pick despawned is still named by its ID: its name
    /// is gone.
    picked: Option<EntityId>,
    /// The rows of the Entities panel's tree that are open, showing their children's, by their
    /// entities' IDs, which outlive them: an entity an undo gives back comes back open.
    expanded: HashSet<EntityId>,
    /// The component type the Entities panel lists the entities of; all of them if `None`.
    with: Option<String>,
    /// What's typed into the Find field.
    find: String,
    /// Why the last ID entered there found no entity.
    not_found: Option<String>,
    /// The last value refused of each field of the picked entity.
    refusals: Vec<Refusal>,
    /// The edits made, which undo and redo step through.
    log: Log,
    /// Why the last operation, undo or redo was refused, until the next edit, operation, undo or
    /// redo.
    not_done: Option<String>,
    /// What the panels did since the last tick, which it makes.
    queued: Queue,
    /// Whether a set has been queued since the last seal, operation, undo or redo, which leave no
    /// edit going on: a seal queued otherwise would end none, so none is.
    open: bool,
}

/// Undoes the last edit.
const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Z);

/// Redo the last edit undone; the Edit menu shows the first.
const REDO: [KeyboardShortcut; 2] = [
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), egui::Key::Z),
    KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Y),
];

/// A step through the log of edits, as the Edit menu or a key asks for one.
#[derive(Clone, Copy)]
enum Step {
    Undo,
    Redo,
}

/// What a frame's input, in the windows the panels draw in, says of the edits: a press, which
/// starts another, and a release, which ends one; whether a widget has the pointer or the keys,
/// which keeps one going; and the step through them a key asks for.
#[derive(Default)]
struct Cues {
    pressed: bool,
    released: bool,
    held: bool,
    step: Option<Step>,
}

impl Cues {
    /// Adds what `egui`'s input says, as the window that's drawing has it: a panel's own, if it's
    /// torn off, or the main one. Its keys ask for a step unless a text field there has them, which
    /// undo its own text.
    fn read(&mut self, egui: &egui::Context) {
        let pointer =
            egui.input(|input| (input.pointer.any_pressed(), input.pointer.any_released()));
        self.pressed |= pointer.0;
        self.released |= pointer.1;
        self.held |= egui.egui_is_using_pointer() || egui.egui_wants_keyboard_input();
        if self.step.is_some() || egui.text_edit_focused() {
            return;
        }
        self.step = egui.input_mut(|input| {
            // Redo's first, since a shortcut matches with Shift held as well.
            if REDO.iter().any(|redo| input.consume_shortcut(redo)) {
                Some(Step::Redo)
            } else {
                input.consume_shortcut(&UNDO).then_some(Step::Undo)
            }
        });
    }
}

/// A value [`World::set_field`] refused, at `path` of `component`.
struct Refusal {
    component: String,
    path: String,
    message: String,
}

/// A field to set, found as the Inspector panel draws, and set at the next tick.
struct Edit {
    component: String,
    path: String,
    value: Value,
}

/// What a pass of the panels found to do, which [`Inspector::queue`] queues for the next tick: the
/// sets of the picked entity's fields its widgets made, the operations its menus chose, what its
/// input says of the edits, and the step the Edit menu chose.
#[derive(Default)]
struct Pass {
    sets: Vec<Edit>,
    /// Creates, deletes, adds and removes.
    operations: Vec<Command>,
    cues: Cues,
    chosen: Option<Step>,
}

/// What the panels did, which the next tick makes in the order they did it: the Inspector's
/// commands.
enum Command {
    /// Sets a field of the entity with this ID, the one picked as the field was drawn, as part of
    /// the edit going on.
    Set(EntityId, Edit),
    /// Ends the edit going on, so that the next set begins another.
    Seal,
    Step(Step),
    /// Creates an entity with `components`, each a type's name and its whole value: the last of
    /// `parent`'s children, or a root.
    Create {
        parent: Option<EntityId>,
        components: Vec<(Name, Value)>,
    },
    /// Deletes the entity with this ID, and its descendants.
    Delete(EntityId),
    /// Gives the entity with this ID a component of the type named, made from the value.
    Add(EntityId, String, Value),
    /// Takes the component of the type named away from the entity with this ID.
    Remove(EntityId, String),
}

/// The Inspector's commands, which a recording reads as values: variants `Set`, with the
/// entity's ID as text, the component and the path, and the value; `Seal`; `Undo`; `Redo`;
/// `Create`, with its parent's ID as text, if it has one, and its components as a struct, a field
/// each named as its type; `Delete`, with the entity's ID as text; `Add`, with the entity's ID as
/// text, the component and its value; and `Remove`, with the entity's ID as text and the
/// component.
#[derive(Default)]
struct Queue(Vec<Command>);

impl Commands for Queue {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn value(&self, i: usize) -> Value {
        let text = |text: &str| Value::String(text.to_owned());
        let entity = |id: &EntityId| -> (Name, Value) { ("entity".into(), text(&id.to_string())) };
        let (name, fields) = match &self.0[i] {
            Command::Set(id, edit) => (
                "Set",
                vec![
                    entity(id),
                    ("component".into(), text(&edit.component)),
                    ("path".into(), text(&edit.path)),
                    ("value".into(), edit.value.clone()),
                ],
            ),
            Command::Seal => ("Seal", Vec::new()),
            Command::Step(Step::Undo) => ("Undo", Vec::new()),
            Command::Step(Step::Redo) => ("Redo", Vec::new()),
            Command::Create { parent, components } => {
                let parent = parent.map(|id| Box::new(text(&id.to_string())));
                (
                    "Create",
                    vec![
                        ("parent".into(), Value::Option(parent)),
                        ("components".into(), Value::Struct(components.clone())),
                    ],
                )
            }
            Command::Delete(id) => ("Delete", vec![entity(id)]),
            Command::Add(id, component, value) => (
                "Add",
                vec![
                    entity(id),
                    ("component".into(), text(component)),
                    ("value".into(), value.clone()),
                ],
            ),
            Command::Remove(id, component) => (
                "Remove",
                vec![entity(id), ("component".into(), text(component))],
            ),
        };
        Value::Variant(name.into(), fields)
    }

    fn push(&mut self, value: &Value) -> Result<()> {
        let (name, fields) = reflect::variant(value)?;
        let text = |value: &Value, field: &str| match value {
            Value::String(text) => Ok(text.clone()),
            other => Err(reflect::within(reflect::wrong_kind("String", other), field)),
        };
        let id = |value: &Value, field: &str| -> Result<EntityId> {
            let id = text(value, field)?.parse();
            id.map_err(|error| reflect::within(error, field))
        };
        let command = match name {
            "Set" => {
                let names = ["entity", "component", "path", "value"];
                let [entity, component, path, value] = reflect::pick(fields, names)?;
                let entity = id(entity, "entity")?;
                let edit = Edit {
                    component: text(component, "component")?,
                    path: text(path, "path")?,
                    value: value.clone(),
                };
                Command::Set(entity, edit)
            }
            "Seal" | "Undo" | "Redo" => {
                reflect::pick(fields, [])?;
                match name {
                    "Seal" => Command::Seal,
                    "Undo" => Command::Step(Step::Undo),
                    _ => Command::Step(Step::Redo),
                }
            }
            "Create" => {
                let [parent, components] = reflect::pick(fields, ["parent", "components"])?;
                let parent = match parent {
                    Value::Option(parent) => {
                        let parent = parent.as_deref().map(|parent| id(parent, "parent"));
                        parent.transpose()?
                    }
                    other => {
                        let error = reflect::wrong_kind("Option", other);
                        return Err(reflect::within(error, "parent"));
                    }
                };
                let Value::Struct(components) = components else {
                    let error = reflect::wrong_kind("Struct", components);
                    return Err(reflect::within(error, "components"));
                };
                Command::Create {
                    parent,
                    components: components.clone(),
                }
            }
            "Delete" => {
                let [entity] = reflect::pick(fields, ["entity"])?;
                Command::Delete(id(entity, "entity")?)
            }
            "Add" => {
                let names = ["entity", "component", "value"];
                let [entity, component, value] = reflect::pick(fields, names)?;
                let entity = id(entity, "entity")?;
                Command::Add(entity, text(component, "component")?, value.clone())
            }
            "Remove" => {
                let [entity, component] = reflect::pick(fields, ["entity", "component"])?;
                let entity = id(entity, "entity")?;
                Command::Remove(entity, text(component, "component")?)
            }
            other => {
                let names = [
                    "Set", "Seal", "Undo", "Redo", "Create", "Delete", "Add", "Remove",
                ];
                return Err(reflect::no_variant(other, &names));
            }
        };
        self.0.push(command);
        Ok(())
    }

    fn clear(&mut self) {
        self.0.clear();
    }
}

impl Layer for Inspector {
    fn detach(&mut self, cx: &mut Context) {
        self.log.clear(cx.world_mut());
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        let mut queued = mem::take(&mut self.queued.0);
        for command in queued.drain(..) {
            self.run(world, command);
        }
        // Kept for its memory.
        self.queued.0 = queued;
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.queued)
    }

    fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
        let _zone = gg_core::zone!("inspector");
        let mut pass = Pass::default();
        ui.menu("Edit", |ui| pass.chosen = self.edit_menu(ui));
        let world = cx.world();
        ui.menu("Entity", |ui| {
            self.entity_menu(world, ui, &mut pass.operations)
        });
        ui.panel("Entities", |ui| self.entities(world, ui, &mut pass));
        ui.panel("Inspector", |ui| {
            // A new panel floats smaller than its contents.
            let scroll = egui::ScrollArea::vertical();
            scroll.show(ui, |ui| self.inspect(world, ui, &mut pass));
        });
        // The Edit menu's keys, wherever the panels are.
        pass.cues.read(ui.egui());
        // Every pass, even one egui discards, which alone had the frame's input; and empty ones
        // too, whose cues end an edit.
        self.queue(pass);
    }
}

impl Inspector {
    /// The Edit menu: Undo and Redo, each naming the edit it steps through; returns the one
    /// clicked.
    fn edit_menu(&self, ui: &mut egui::Ui) -> Option<Step> {
        let items = [
            (Step::Undo, "Undo", self.log.undoable(), UNDO),
            (Step::Redo, "Redo", self.log.redoable(), REDO[0]),
        ];
        let mut chosen = None;
        for (step, name, edit, shortcut) in items {
            let label = match edit {
                Some(edit) => format!("{name} {edit}"),
                None => name.to_owned(),
            };
            let shortcut = ui.ctx().format_shortcut(&shortcut);
            let item = egui::Button::new(label).shortcut_text(shortcut);
            if ui.add_enabled(edit.is_some(), item).clicked() {
                chosen = Some(step);
            }
        }
        chosen
    }

    /// The Entity menu: Create Empty, which makes a root; while an entity is picked, Unpick; and,
    /// while it's alive, Create Empty Child, which makes a child of it, and Delete, which deletes
    /// it and its descendants. The operation chosen goes in `operations`.
    fn entity_menu(&mut self, world: &World, ui: &mut egui::Ui, operations: &mut Vec<Command>) {
        let live = self.picked.filter(|&id| world.entity(id).is_some());
        if ui.button("Create Empty").clicked() {
            operations.push(empty(None));
        }
        let child = ui.add_enabled(live.is_some(), egui::Button::new("Create Empty Child"));
        let delete = ui.add_enabled(live.is_some(), egui::Button::new("Delete"));
        // A pick despawned unpicks too, since it stays picked for an undo to bring it back.
        let unpick = ui.add_enabled(self.picked.is_some(), egui::Button::new("Unpick"));
        if unpick.clicked() {
            self.unpick();
        }
        let Some(id) = live else {
            return;
        };
        if child.clicked() {
            operations.push(empty(Some(id)));
        }
        if delete.clicked() {
            operations.push(Command::Delete(id));
        }
    }

    /// Queues a pass of the panels, for the next tick to make: its sets of the picked entity's
    /// fields, as one edit from a press of the pointer to its release, or while a widget has the
    /// keys, as its input says, so that a drag, or a number typed, is one edit; then its
    /// operations, each of which ends the edit going on; then the step the Edit menu chose, or a
    /// key asks for.
    fn queue(&mut self, pass: Pass) {
        let Pass {
            sets,
            operations,
            cues,
            chosen,
        } = pass;
        // Before this pass's sets: a press begins another edit, even while a text field keeps
        // the keys.
        if cues.pressed {
            self.seal();
        }
        if let Some(id) = self.picked {
            for edit in sets {
                self.queued.0.push(Command::Set(id, edit));
                self.open = true;
            }
        }
        for operation in operations {
            self.queued.0.push(operation);
            self.open = false;
        }
        if cues.released || !cues.held {
            self.seal();
        }
        if let Some(step) = chosen.or(cues.step) {
            self.queued.0.push(Command::Step(step));
            self.open = false;
        }
    }

    /// Queues a seal, unless it would end no edit, so that a pass that does nothing queues
    /// nothing: only a set the log takes begins one, and seals, operations, undos and redos
    /// end it.
    fn seal(&mut self) {
        if mem::take(&mut self.open) {
            self.queued.0.push(Command::Seal);
        }
    }

    /// Makes, at a tick, `command`, which the panels queued.
    fn run(&mut self, world: &mut World, command: Command) {
        match command {
            Command::Set(id, edit) => self.apply(world, Some(id), edit),
            Command::Seal => self.log.seal(),
            Command::Step(step) => {
                let stepped = match step {
                    Step::Undo => self.log.undo(world),
                    Step::Redo => self.log.redo(world),
                };
                self.not_done = stepped.err().map(|error| error.to_string());
            }
            Command::Create { parent, components } => {
                let create = Operation::Create {
                    entity: None,
                    parent,
                    components,
                };
                self.operate(world, create);
            }
            Command::Delete(entity) => self.operate(world, Operation::Delete { entity }),
            Command::Add(entity, component, value) => {
                self.forget(entity, &component);
                let component = component.into();
                let add = Operation::Add {
                    entity,
                    component,
                    value,
                };
                self.operate(world, add);
            }
            Command::Remove(entity, component) => {
                self.forget(entity, &component);
                let component = component.into();
                self.operate(world, Operation::Remove { entity, component });
            }
        }
    }

    /// Applies `operation`, which the panels chose, through the log, which ends the edit going on;
    /// then picks the entity a create made, opening the rows of its ancestors. A refusal shows
    /// above the components, and is logged.
    fn operate(&mut self, world: &mut World, operation: Operation) {
        let (entity, said) = (operation.entity().map(display), operation.to_string());
        if let Err(error) = self.log.apply(world, operation) {
            warn!(code = error.code(), entity, operation = %said, "{error}");
            self.not_done = Some(error.to_string());
            return;
        }
        self.not_done = None;
        let Some(&Operation::Create {
            entity: Some(made), ..
        }) = self.log.undoable()
        else {
            return;
        };
        self.pick(made);
        let made = world
            .entity(made)
            .expect("the entity a create made is alive");
        self.open_ancestors(world, made);
    }

    /// Forgets the values `entity`'s `component` refused, if it's the entity picked, whose
    /// refusals are kept: an add or a remove makes it anew.
    fn forget(&mut self, entity: EntityId, component: &str) {
        if self.picked == Some(entity) {
            self.refusals
                .retain(|refusal| refusal.component != component);
        }
    }

    /// The Entities panel: a field that finds an entity by its ID, which component the entities
    /// listed have, how many there are, and a row for each of them, or for each of the tree's, of
    /// which only those in view are drawn; and below them, where they leave room, space whose
    /// click unpicks, and whose context menu makes a root. The operations its menus choose, and
    /// what the input of the window it draws in says of the edits, go in `pass`.
    fn entities(&mut self, world: &World, ui: &mut egui::Ui, pass: &mut Pass) {
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
        below.context_menu(|ui| {
            if ui.button("Create Empty Entity").clicked() {
                operations.push(empty(None));
            }
        });
    }

    /// Draws `entity`'s row: its label, which picks it, and whose context menu makes an empty
    /// child of it or deletes it, the operation chosen going in `operations`; in the tree, at
    /// `depth`, after an arrow that shows and hides its children's rows, if it has any.
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
                let label = ui.selectable_label(picked, row_label(world, entity, id));
                if label.clicked() {
                    self.pick(id);
                }
                label.context_menu(|ui| {
                    if ui.button("Create Empty Child").clicked() {
                        operations.push(empty(Some(id)));
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
            .map_err(|error: reflect::Error| error.to_string())?;
        let Some(entity) = world.entity(id) else {
            return Err(format!("no entity has ID {id}"));
        };
        self.pick(id);
        self.open_ancestors(world, entity);
        Ok(())
    }

    fn pick(&mut self, id: EntityId) {
        if self.picked != Some(id) {
            self.picked = Some(id);
            self.refusals.clear();
        }
    }

    fn unpick(&mut self) {
        self.picked = None;
        self.refusals.clear();
    }

    /// Opens the rows of `entity`'s ancestors, so that the tree shows its own.
    fn open_ancestors(&mut self, world: &World, entity: Entity) {
        let mut up = world.parent(entity).expect("alive");
        while let Some(parent) = up {
            self.expanded
                .insert(world.id(parent).expect("a parent is alive"));
            up = world.parent(parent).expect("a parent is alive");
        }
    }

    /// The Inspector panel: the picked entity's components, each under a header that collapses,
    /// with its fields' widgets, and beside it a menu that removes it; and below them, a menu that
    /// adds one. The fields they change, the operations the menus choose, and what the input of
    /// the window it draws in says of them, go in `pass`.
    fn inspect(&self, world: &World, ui: &mut egui::Ui, pass: &mut Pass) {
        pass.cues.read(ui.ctx());
        if let Some(not_done) = &self.not_done {
            ui.label(not_done.as_str());
        }
        let Some(id) = self.picked else {
            ui.label("Nothing picked: pick an entity in the Entities panel");
            return;
        };
        let Some(entity) = world.entity(id) else {
            ui.label(format!("Entity {id} was despawned"));
            return;
        };
        let components = world.components_of(entity);
        let components = components.expect("the entity of an ID is alive");
        let queued = self.queued.0.iter().filter_map(|command| match command {
            Command::Set(set, edit) if *set == id => Some(edit),
            _ => None,
        });
        let waiting: Vec<&Edit> = queued.collect();
        let heading = match entity_name(world, entity) {
            Some(name) => format!("{name}, entity {id}"),
            None => format!("Entity {id}"),
        };
        ui.label(heading);
        if let Some(children) = children(world, entity) {
            ui.label(children);
        }
        // So that what's typed into a field of one entity never lands in the field of the next
        // one picked; and, being its own, so that the fields keep their widgets' IDs, and a drag
        // or a number being typed, as the labels above them come and go.
        let scope = egui::UiBuilder::new().scope_id(ui.make_persistent_id(id));
        ui.scope_builder(scope, |ui| {
            let types = world.types();
            for component in components {
                // Drawn as the next tick will leave it.
                if let Some(Command::Remove(..)) = self.change(id, component) {
                    continue;
                }
                let (Ok(value), Some(index)) = (
                    world.get_field(entity, component, ""),
                    types.find(component),
                ) else {
                    continue;
                };
                let header = egui::CollapsingHeader::new(component)
                    .id_salt(component)
                    .default_open(true)
                    .show(ui, |ui| {
                        let mut fields = Fields {
                            world,
                            types,
                            entity: id,
                            component,
                            refusals: &self.refusals,
                            edits: &mut pass.sets,
                            waiting: &waiting,
                        };
                        let (ty, meta) = (Type::Named(index), Meta::default());
                        let slot = Slot {
                            ty: &ty,
                            meta: &meta,
                            default: None,
                        };
                        if let Some(value) = fields.value(ui, slot, &value, "", false) {
                            fields.edit("", value);
                        }
                        fields.refusal(ui, &ty, "");
                    })
                    .header_response;
                if component != PARENT {
                    component_menu(ui, header.rect, id, component, &mut pass.operations);
                }
                let doc = &types.info(index).doc;
                if !doc.is_empty() {
                    header.on_hover_text(doc.as_ref());
                }
            }
            self.add_menu(world, ui, entity, id, &mut pass.operations);
        });
    }

    /// The add or the remove of component `name` of the entity of ID `id` that waits for a tick,
    /// the last if several do.
    fn change(&self, id: EntityId, name: &str) -> Option<&Command> {
        self.queued.0.iter().rev().find(|command| match command {
            Command::Add(entity, component, _) | Command::Remove(entity, component) => {
                *entity == id && component == name
            }
            _ => false,
        })
    }

    /// The Add Component menu, below the components of `entity`, whose ID is `id`: the types it
    /// can be given, in the order they registered, each with its doc on hover; but not those it
    /// has, nor a parent, which the world gives it, nor those opaque as a whole, such as handles,
    /// which are code's to make, nor those an add of waits for a tick. A type whose default an add would refuse shows
    /// disabled, saying why on hover. The defaults are made only while it's open. The add chosen,
    /// of the type's default, goes in `operations`.
    fn add_menu(
        &self,
        world: &World,
        ui: &mut egui::Ui,
        entity: Entity,
        id: EntityId,
        operations: &mut Vec<Command>,
    ) {
        ui.menu_button("Add Component", |ui| {
            let types = world.types();
            let own: Vec<&str> = world.components_of(entity).into_iter().flatten().collect();
            for name in world.component_types() {
                let info = types.find(name).map(|index| types.info(index));
                let opaque = info.is_some_and(|info| matches!(info.shape, Shape::Opaque));
                let adding = matches!(self.change(id, name), Some(Command::Add(..)));
                if name == PARENT || own.contains(&name) || opaque || adding {
                    continue;
                }
                let item = egui::Button::new(name);
                match world.default_of(name) {
                    Ok(default) => {
                        let mut item = ui.add(item);
                        let doc = info.map_or("", |info| info.doc.as_ref());
                        if !doc.is_empty() {
                            item = item.on_hover_text(doc);
                        }
                        if item.clicked() {
                            operations.push(Command::Add(id, name.to_owned(), default));
                        }
                    }
                    Err(error) => {
                        let item = ui.add_enabled(false, item);
                        item.on_disabled_hover_text(error.to_string());
                    }
                }
            }
        });
    }

    /// Sets the field `edit` changed of `entity`, the entity picked as it was made, as part of the
    /// edit going on, and keeps the refusal if it's refused, while that's still the entity picked.
    fn apply(&mut self, world: &mut World, entity: Option<EntityId>, edit: Edit) {
        let Some(id) = entity.filter(|&id| world.entity(id).is_some()) else {
            return;
        };
        let Edit {
            component,
            path,
            value,
        } = edit;
        let shown = self.picked == Some(id);
        if shown {
            self.refusals
                .retain(|refusal| refusal.component != component || refusal.path != path);
        }
        self.not_done = None;
        let set = Operation::Set {
            entity: id,
            component: component.clone().into(),
            path: path.clone(),
            value,
        };
        if let Err(error) = self.log.amend(world, set) {
            warn!(
                code = error.code(),
                entity = %id,
                component = component.as_str(),
                path = path.as_str(),
                "{error}"
            );
            if shown {
                self.refusals.push(Refusal {
                    component,
                    path,
                    message: error.to_string(),
                });
            }
        }
    }
}

/// The rows of the Entities panel's tree, for a frame: the roots, in the order [`World::roots`]
/// lists them, each open one followed by the rows of its descendants, depth first in the order of
/// their parents' children, as far down as their parents' rows are open. Only the open roots'
/// rows are listed, and only the page in view found, so that a frame costs a few steps a table, a
/// step a table for each open root, one for each row under an open root, and one for each row of
/// the page: never one for each root.
struct Tree {
    /// Each open root's place among the roots, in order, and how many rows are under it.
    spans: Vec<(usize, usize)>,
    /// The rows under each open root, in the order of `spans`: each entity, and its depth.
    under: Vec<Vec<(Entity, usize)>>,
    /// How many rows there are.
    len: usize,
}

impl Tree {
    /// The tree of `world`, whose open rows `expanded` holds, by their entities' IDs.
    fn new(world: &World, expanded: &HashSet<EntityId>) -> Self {
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
    fn page(&self, world: &World, shown: Range<usize>) -> Vec<(Entity, usize)> {
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

/// [`Parent`](crate::Parent)'s name: the world gives an entity one as it's made a child, so the
/// Inspector neither adds nor removes one.
const PARENT: &str = "Parent";

/// The label of a component's menu: an ellipsis, which egui's default fonts have, where they
/// haven't the midline one, `⋯`.
const MORE: &str = "…";

/// The menu of `component`, beside its header, `header`, whose Remove Component takes it away from
/// the entity of ID `id`, the operation going in `operations`. It's in a child of `ui`, which
/// leaves the cursor where the header's body left it, for the next header.
fn component_menu(
    ui: &mut egui::Ui,
    header: egui::Rect,
    id: EntityId,
    component: &str,
    operations: &mut Vec<Command>,
) {
    let size = egui::Vec2::splat(ui.spacing().interact_size.y);
    let at = egui::pos2(header.right() + ui.spacing().item_spacing.x, header.top());
    let child = egui::UiBuilder::new()
        .id_salt(("component menu", component))
        .max_rect(egui::Rect::from_min_size(at, size));
    let mut menu = ui.new_child(child);
    let button = egui::Button::new(MORE).min_size(size);
    egui::menu::MenuButton::from_button(button).ui(&mut menu, |ui| {
        if ui.button("Remove Component").clicked() {
            operations.push(Command::Remove(id, component.to_owned()));
        }
    });
}

/// Draws the fields of one component, and keeps the edits they make.
struct Fields<'a> {
    world: &'a World,
    types: &'a Types,
    /// The ID of the entity whose component it is.
    entity: EntityId,
    component: &'a str,
    refusals: &'a [Refusal],
    edits: &'a mut Vec<Edit>,
    /// The sets of the entity's fields that wait for a tick, in the order they were made.
    waiting: &'a [&'a Edit],
}

/// What's drawn at a path: its type; the hints of the field it's in, which its parts and elements
/// inherit; and its default there, if one is left, by reference, so that a frame clones none: the
/// default of what holds it, narrowed to it, or else its field's own.
#[derive(Clone, Copy)]
struct Slot<'s> {
    ty: &'s Type,
    meta: &'s Meta,
    default: Option<&'s Value>,
}

impl<'s> Slot<'s> {
    /// Where `field` is drawn, its default the one of its name in `defaults`, the fields of the
    /// default of what holds it, or else its own.
    fn of(field: &'s Field, defaults: &'s [(Name, Value)]) -> Self {
        let given = defaults.iter().find(|(name, _)| *name == field.name);
        Self {
            ty: &field.ty,
            meta: &field.meta,
            default: given.map(|(_, default)| default).or(field.default.as_ref()),
        }
    }
}

/// Each axis of a vector, and of a rotation's angles: its component's name, as a path names it;
/// its button's label; and the button's color, then its color hovered, in RGB: Hazel's, and grays
/// for `w`.
const AXES: [(&str, &str, [u8; 3], [u8; 3]); 4] = [
    ("x", "X", [204, 25, 38], [230, 51, 51]),
    ("y", "Y", [51, 179, 51], [77, 204, 77]),
    ("z", "Z", [25, 64, 204], [51, 89, 230]),
    ("w", "W", [102, 102, 102], [128, 128, 128]),
];

impl Fields<'_> {
    /// Draws `value`, the field at `path`, as `slot` says, and returns its new value if a widget
    /// changed it and it's set `whole`, as the inside of an option or an enum is; otherwise its
    /// parts set themselves, each at its own path. A field is set only when its widget says it
    /// changed, never by comparing values: drawing it changes nothing. A field that a set waits for
    /// a tick to set is drawn as the last such set will make it, so that its widget, which keeps
    /// what it shows from frame to frame, goes on from there; and a vector as the sets of it and of
    /// its components will make it, in the order they were made, since recordings hold sets of it
    /// whole. Other than a vector, a field is set at its own path or whole with what's around it,
    /// never both, so the sets of one field and of what's around it never wait together.
    fn value(
        &mut self,
        ui: &mut egui::Ui,
        slot: Slot<'_>,
        value: &Value,
        path: &str,
        whole: bool,
    ) -> Option<Value> {
        let (waiting, component) = (self.waiting, self.component);
        let sets = waiting.iter().filter(|set| set.component == component);
        // By the type, so that a struct's field named `x` isn't taken for a component.
        if !matches!(slot.ty, Type::Vec2 | Type::Vec3 | Type::Vec4) {
            let set = sets.rev().find(|set| set.path == path);
            let value = set.map_or(value, |set| &set.value);
            return self.draw(ui, slot, value, path, whole);
        }
        let mut drawn = Cow::Borrowed(value);
        for set in sets {
            if set.path == path {
                drawn = Cow::Borrowed(&set.value);
            } else if let (Some(axis), &Value::F32(x)) = (axis_of(path, &set.path), &set.value) {
                set_component(drawn.to_mut(), axis, x);
            }
        }
        self.draw(ui, slot, &drawn, path, whole)
    }

    /// Draws `value` as [`value`](Self::value) does, as it's given.
    fn draw(
        &mut self,
        ui: &mut egui::Ui,
        slot: Slot<'_>,
        value: &Value,
        path: &str,
        whole: bool,
    ) -> Option<Value> {
        let Slot { ty, meta, default } = slot;
        match (ty, value) {
            (Type::Named(index), _) => {
                let info = self.types.info(*index);
                match (&info.shape, value) {
                    (Shape::Struct { fields, tuple }, Value::Struct(given)) => {
                        let defaults = match default {
                            Some(Value::Struct(defaults)) => defaults.as_slice(),
                            _ => &[],
                        };
                        let changed = match fields.as_slice() {
                            // A tuple struct's only field needs no label: it would say `0`.
                            [field] if *tuple => {
                                self.only_field(ui, field, given, defaults, path, whole)
                            }
                            _ => self.fields(ui, fields, given, defaults, path, whole),
                        };
                        changed.map(Value::Struct)
                    }
                    (Shape::Enum(variants), Value::Variant(name, given)) => {
                        if variants.iter().all(|variant| variant.fields.is_empty()) {
                            let names = variants.iter().map(|variant| &variant.name);
                            self.choice(ui, names, name, path)
                        } else {
                            let variant = variants.iter().find(|variant| variant.name == *name)?;
                            // The default's fields, if it's of this variant.
                            let defaults = match default {
                                Some(Value::Variant(of, defaults)) if of == name => {
                                    defaults.as_slice()
                                }
                                _ => &[],
                            };
                            let new = ui.vertical(|ui| {
                                let picked = self.variant(ui, variants, default, name, given, path);
                                let fields = &variant.fields;
                                let changed = self.fields(ui, fields, given, defaults, path, true);
                                picked.or_else(|| Some(Value::Variant(name.clone(), changed?)))
                            });
                            new.inner
                        }
                    }
                    _ => opaque(ui),
                }
            }
            (Type::Option(inner), Value::Option(Some(value))) => {
                let default = match default {
                    Some(Value::Option(Some(default))) => Some(&**default),
                    _ => None,
                };
                let inside = Slot {
                    ty: inner,
                    meta,
                    default,
                };
                let inner = ui.horizontal(|ui| {
                    ui.label("Some");
                    // As it's given: a set waiting at its path is the option's, which drew it.
                    self.draw(ui, inside, value, path, true)
                });
                Some(Value::Option(Some(Box::new(inner.inner?))))
            }
            (Type::Option(_), Value::Option(None)) => {
                ui.label("None");
                None
            }
            (Type::List(inner) | Type::Array(inner, _), Value::List(items)) => {
                let defaults = match default {
                    Some(Value::List(defaults)) => defaults.as_slice(),
                    _ => &[],
                };
                let item = Slot {
                    ty: inner,
                    meta,
                    default: None,
                };
                self.elements(ui, item, defaults, items, path, whole)
            }
            (Type::Bool, &Value::Bool(mut on)) => {
                let changed = ui.checkbox(&mut on, "").changed();
                changed.then_some(Value::Bool(on))
            }
            (&Type::Int { bits, signed: true }, &Value::Int(n)) => {
                let (min, max) = int_range(bits, true, meta);
                let mut n = n;
                drag(ui, &mut n, min as i64..=max as i64, meta).then_some(Value::Int(n))
            }
            (
                &Type::Int {
                    bits,
                    signed: false,
                },
                &Value::UInt(n),
            ) => {
                let (min, max) = int_range(bits, false, meta);
                let mut n = n;
                drag(ui, &mut n, min as u64..=max as u64, meta).then_some(Value::UInt(n))
            }
            (Type::F32, &Value::F32(x)) => {
                let x = float(ui, x.into(), meta, true)?;
                Some(Value::F32(x as f32))
            }
            (Type::F64, &Value::F64(x)) => float(ui, x, meta, false).map(Value::F64),
            (Type::String, Value::String(text)) => {
                let mut text = text.clone();
                let changed = ui.text_edit_singleline(&mut text).changed();
                changed.then_some(Value::String(text))
            }
            (Type::Vec2, &Value::Vec2(v)) => {
                let v = self.vector(ui, slot, v.to_array(), path, whole)?;
                Some(Value::Vec2(Vec2::from(v)))
            }
            (Type::Vec3, &Value::Vec3(v)) if meta.show == Show::Color => {
                let rgba = self.color(ui, v.extend(1.0).to_array(), Alpha::Opaque, path)?;
                Some(Value::Vec3(Vec4::from(rgba).truncate()))
            }
            (Type::Vec3, &Value::Vec3(v)) => {
                let v = self.vector(ui, slot, v.to_array(), path, whole)?;
                Some(Value::Vec3(Vec3::from(v)))
            }
            (Type::Vec4, &Value::Vec4(v)) if meta.show == Show::Color => {
                let rgba = self.color(ui, v.to_array(), Alpha::OnlyBlend, path)?;
                Some(Value::Vec4(Vec4::from(rgba)))
            }
            (Type::Vec4, &Value::Vec4(v)) => {
                let v = self.vector(ui, slot, v.to_array(), path, whole)?;
                Some(Value::Vec4(Vec4::from(v)))
            }
            (Type::Quat, &Value::Quat(q)) => self.rotation(ui, slot, q, path),
            (Type::Entity, Value::Entity(entity)) => {
                // A despawned entity has no ID, only its handle.
                match self.world.id(*entity) {
                    Some(id) => ui.label(id.to_string()),
                    None => ui.label(entity.to_string()),
                };
                None
            }
            _ => opaque(ui),
        }
    }

    /// Draws `given`, the values of `fields` at `path`, a row each, with the doc of each field on
    /// its name, and its default the one of its name in `defaults`, the fields of the default of
    /// what holds them, or else its own. Returns them, with what changed, if they're set `whole`;
    /// otherwise each field that changed is set at its own path.
    fn fields(
        &mut self,
        ui: &mut egui::Ui,
        fields: &[Field],
        given: &[(Name, Value)],
        defaults: &[(Name, Value)],
        path: &str,
        whole: bool,
    ) -> Option<Vec<(Name, Value)>> {
        let mut changed: Option<Vec<(Name, Value)>> = None;
        egui::Grid::new((self.component, path))
            .num_columns(2)
            .show(ui, |ui| {
                for (i, (field, (_, value))) in fields.iter().zip(given).enumerate() {
                    let path = reflect::join(path, &field.name);
                    let name = ui.label(field.name.as_ref());
                    if !field.meta.doc.is_empty() {
                        name.on_hover_text(field.meta.doc.as_ref());
                    }
                    let new = self.value(ui, Slot::of(field, defaults), value, &path, whole);
                    ui.end_row();
                    self.refusal(ui, &field.ty, &path);
                    if let Some(new) = new {
                        if whole {
                            changed.get_or_insert_with(|| given.to_vec())[i].1 = new;
                        } else {
                            self.edit(&path, new);
                        }
                    }
                }
            });
        changed
    }

    /// Draws the only field of a tuple struct, `field`, with its value from `given`, and its
    /// default as [`fields`](Self::fields) finds it in `defaults`, as that draws a field, but
    /// without a label, which would only say `0`, and in a group of its own, so that a refusal goes
    /// under it, not into the row of a grid it's in.
    fn only_field(
        &mut self,
        ui: &mut egui::Ui,
        field: &Field,
        given: &[(Name, Value)],
        defaults: &[(Name, Value)],
        path: &str,
        whole: bool,
    ) -> Option<Vec<(Name, Value)>> {
        let [(name, value)] = given else {
            return None;
        };
        let path = reflect::join(path, &field.name);
        let new = ui.vertical(|ui| {
            let new = self.value(ui, Slot::of(field, defaults), value, &path, whole);
            self.refusal(ui, &field.ty, &path);
            new
        });
        let new = new.inner?;
        if whole {
            Some(vec![(name.clone(), new)])
        } else {
            self.edit(&path, new);
            None
        }
    }

    /// Draws `items`, the elements at `path` of a list or an array, each as `item` says but for its
    /// default, which is the one of its index in `defaults`, the default's elements, if there's
    /// one; a row each, as [`fields`](Self::fields) does.
    fn elements(
        &mut self,
        ui: &mut egui::Ui,
        item: Slot<'_>,
        defaults: &[Value],
        items: &[Value],
        path: &str,
        whole: bool,
    ) -> Option<Value> {
        let mut changed: Option<Vec<Value>> = None;
        egui::Grid::new((self.component, path))
            .num_columns(2)
            .show(ui, |ui| {
                for (i, value) in items.iter().enumerate() {
                    let path = format!("{path}[{i}]");
                    ui.label(format!("[{i}]"));
                    let slot = Slot {
                        default: defaults.get(i),
                        ..item
                    };
                    let new = self.value(ui, slot, value, &path, whole);
                    ui.end_row();
                    self.refusal(ui, item.ty, &path);
                    if let Some(new) = new {
                        if whole {
                            changed.get_or_insert_with(|| items.to_vec())[i] = new;
                        } else {
                            self.edit(&path, new);
                        }
                    }
                }
            });
        changed.map(Value::List)
    }

    /// A choice of an enum's unit variants, `names`, of which `picked` is the one it is.
    fn choice<'n>(
        &mut self,
        ui: &mut egui::Ui,
        names: impl Iterator<Item = &'n Name>,
        picked: &Name,
        path: &str,
    ) -> Option<Value> {
        let mut chosen = picked.clone();
        let changed = egui::ComboBox::from_id_salt((self.component, path))
            .selected_text(picked.as_ref())
            .show_ui(ui, |ui| {
                let mut changed = false;
                for name in names {
                    let response = ui.selectable_value(&mut chosen, name.clone(), name.as_ref());
                    changed |= response.changed();
                }
                changed
            });
        changed
            .inner
            .unwrap_or(false)
            .then(|| Value::Variant(chosen, Vec::new()))
    }

    /// A choice of the variants of an enum with fields, `variants`, at `path`, where its default is
    /// `default`, of which `name` is the one it is, with fields `given`; returns the one picked, as
    /// [`switched`](Self::switched) makes it. One that can't be switched to shows disabled, saying
    /// why on hover. The fields each variant last had are kept as they're drawn, which a set
    /// waiting for a tick makes them, under an id of the component's field, not the entity's, so
    /// that what's kept doesn't grow as entities are picked: they're the entity's, and go as
    /// another is drawn.
    fn variant(
        &self,
        ui: &mut egui::Ui,
        variants: &[Variant],
        default: Option<&Value>,
        name: &Name,
        given: &[(Name, Value)],
        path: &str,
    ) -> Option<Value> {
        type Kept = (EntityId, Vec<(Name, Vec<(Name, Value)>)>);
        let id = egui::Id::unique(("inspector variants", self.component, path));
        let entity = self.entity;
        // Written only as they differ, so that a frame clones nothing.
        ui.data_mut(|data| {
            let kept = data.get_temp_mut_or_insert_with(id, || -> Kept { (entity, Vec::new()) });
            if kept.0 != entity {
                *kept = (entity, Vec::new());
            }
            match kept.1.iter_mut().find(|(of, _)| of == name) {
                Some((_, fields)) if fields.as_slice() == given => {}
                Some((_, fields)) => *fields = given.to_vec(),
                None => kept.1.push((name.clone(), given.to_vec())),
            }
        });
        let mut picked = None;
        egui::ComboBox::from_id_salt(("variant", self.component, path))
            .selected_text(name.as_ref())
            .show_ui(ui, |ui| {
                // Read only while it's open.
                let kept = ui.data(|data| data.get_temp::<Kept>(id));
                let kept = kept.map_or_else(Vec::new, |(_, kept)| kept);
                for variant in variants {
                    let current = variant.name == *name;
                    let item = egui::Button::selectable(current, variant.name.as_ref());
                    if current {
                        ui.add(item);
                        continue;
                    }
                    match self.switched(variant, &kept, default) {
                        Ok(value) => {
                            if ui.add(item).clicked() {
                                picked = Some(value);
                            }
                        }
                        Err(why) => {
                            ui.add_enabled(false, item).on_disabled_hover_text(why);
                        }
                    }
                }
            });
        picked
    }

    /// `variant`, as a switch to it makes it: with the fields it last had on the entity, which
    /// `kept` holds, if it had any; or else `default`, if it's of that variant; or else with its
    /// fields' defaults. Fails, saying why, if its default fails, or if it holds what's opaque,
    /// which reflection doesn't make, so that a set of it would be refused.
    fn switched(
        &self,
        variant: &Variant,
        kept: &[(Name, Vec<(Name, Value)>)],
        default: Option<&Value>,
    ) -> Result<Value, String> {
        let last = kept.iter().find(|(name, _)| *name == variant.name);
        let made = match (last, default) {
            (Some((name, fields)), _) => Value::Variant(name.clone(), fields.clone()),
            (None, Some(default @ Value::Variant(name, _))) if *name == variant.name => {
                default.clone()
            }
            _ => self
                .types
                .variant_default(variant)
                .map_err(|error| error.to_string())?,
        };
        match opaque_at(&made) {
            // Refused as a make of it would be.
            Some(at) => {
                reflect::make_opaque(&made).map_err(|error| reflect::within(error, &at).to_string())
            }
            None => Ok(made),
        }
    }

    /// A rotation as angles about x, y and z, in degrees, applied z first, each joined to a button
    /// of its axis that sets it to the angle of the default rotation at `slot`; it's set whole. The
    /// angles are kept with the rotation they made, so that those typed stay as they are, rather
    /// than jump to the same rotation's other angles, while the rotation stays within float noise
    /// of it: a system that renormalizes it at each tick can change its last bits, which would
    /// reset the text being typed. They're kept under an id of the component's field, not the
    /// entity's, so that what's kept doesn't grow as entities are picked.
    fn rotation(
        &mut self,
        ui: &mut egui::Ui,
        slot: Slot<'_>,
        q: Quat,
        path: &str,
    ) -> Option<Value> {
        type Kept = (Quat, [f32; 3]);
        let id = egui::Id::unique(("inspector rotation", self.component, path));
        let kept = ui.data(|data| data.get_temp::<Kept>(id));
        let mut angles = match kept {
            Some((kept, angles)) if same_rotation(kept, q) => angles,
            _ => {
                let angles = angles_of(q);
                // Shown as text, as NaN is: a drag over NaN would change it every frame. A
                // rotation that isn't finite has NaN angles.
                if !angles.iter().all(|angle| angle.is_finite()) {
                    ui.label(q.to_string());
                    return None;
                }
                ui.data_mut(|data| data.insert_temp(id, (q, angles)));
                angles
            }
        };
        let mut changed = false;
        ui.horizontal(|ui| {
            for (axis, angle) in angles.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if axis_button(ui, axis)
                        && let Some(Value::Quat(default)) = self.default_at(slot, path).as_deref()
                    {
                        *angle = angles_of(*default)[axis];
                        changed = true;
                    }
                    let drag = egui::DragValue::new(angle).speed(0.5).suffix("°");
                    changed |= ui.add(drag).changed();
                });
            }
        });
        if !changed {
            return None;
        }
        let [x, y, z] = angles.map(math::to_radians);
        let q = Quat::from_euler(EulerRot::ZYX, z, y, x).normalize();
        ui.data_mut(|data| data.insert_temp(id, (q, angles)));
        Some(Value::Quat(q))
    }

    /// Drags each of a vector's `components`, at `path`, under the hints at `slot`, each joined to
    /// a button of its axis that sets it to the default's there. Each that changes is set at its
    /// own path, as `x` is, unless they're set `whole`: then they're returned, if one changed.
    fn vector<const N: usize>(
        &mut self,
        ui: &mut egui::Ui,
        slot: Slot<'_>,
        mut components: [f32; N],
        path: &str,
        whole: bool,
    ) -> Option<[f32; N]> {
        let mut changed = false;
        ui.horizontal(|ui| {
            for (axis, component) in components.iter_mut().enumerate() {
                let new = ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let reset = axis_button(ui, axis);
                    let dragged = float(ui, (*component).into(), slot.meta, true);
                    if reset {
                        let default = self.default_at(slot, path)?;
                        component_of(&default, axis)
                    } else {
                        dragged.map(|x| x as f32)
                    }
                });
                let Some(new) = new.inner else {
                    continue;
                };
                *component = new;
                if whole {
                    changed = true;
                } else {
                    self.edit(&reflect::join(path, AXES[axis].0), Value::F32(new));
                }
            }
        });
        changed.then_some(components)
    }

    /// The default at `slot`, of the field at `path`: the one there, or else its type's under its
    /// field's bounds, made as it's asked for, which a reset does, so that a frame makes none;
    /// `None` if its type has none there, which is logged.
    fn default_at<'s>(&self, slot: Slot<'s>, path: &str) -> Option<Cow<'s, Value>> {
        if let Some(default) = slot.default {
            return Some(Cow::Borrowed(default));
        }
        match self.types.default_value(slot.ty, slot.meta) {
            Ok(default) => Some(Cow::Owned(default)),
            Err(error) => {
                warn!(
                    code = error.code(),
                    entity = %self.entity,
                    component = self.component,
                    path,
                    "{error}"
                );
                None
            }
        }
    }

    /// A linear color, alpha not multiplied in, `rgba`, the field at `path`, with alpha as `alpha`
    /// says. It's edited as `Hsva`, which keeps the color of what's transparent, where egui's
    /// button of unmultiplied RGBA multiplies alpha in and loses it; and the `Hsva` is kept with
    /// the color it made, as a rotation's angles are, so that a gray's hue, which its color doesn't
    /// hold, stays as it's picked. What egui's popup gives back for it untouched isn't an edit: it
    /// passes the color through gamma and back each frame it's open, which moves its last bits.
    fn color(
        &mut self,
        ui: &mut egui::Ui,
        rgba: [f32; 4],
        alpha: Alpha,
        path: &str,
    ) -> Option<[f32; 4]> {
        type Kept = ([u32; 4], Hsva);
        let id = egui::Id::unique(("inspector color", self.component, path));
        let bits = rgba.map(f32::to_bits);
        let mut hsva = match ui.data(|data| data.get_temp::<Kept>(id)) {
            Some((kept, hsva)) if kept == bits => hsva,
            _ => {
                let [r, g, b, a] = rgba;
                Hsva::from_rgba_unmultiplied(r, g, b, a)
            }
        };
        let untouched = Hsva::from(HsvaGamma::from(hsva));
        let button = egui::color_picker::color_edit_button_hsva(ui, &mut hsva, alpha);
        if !button.changed() || hsva == untouched {
            return None;
        }
        let new = hsva.to_rgba_unmultiplied();
        let made = new.map(f32::to_bits);
        ui.data_mut(|data| data.insert_temp(id, (made, hsva)));
        // A hue picked for a gray changes none of its color's bits: it's only kept.
        (made != bits).then_some(new)
    }

    /// Sets the field at `path` to `value`, once the panel has drawn.
    fn edit(&mut self, path: &str, value: Value) {
        self.edits.push(Edit {
            component: self.component.to_owned(),
            path: path.to_owned(),
            value,
        });
    }

    /// Shows, each on a row of its own, why the field at `path`, of type `ty`, last refused a
    /// value, if it did, and, if it's a vector, why each of its components did.
    fn refusal(&self, ui: &mut egui::Ui, ty: &Type, path: &str) {
        let vector = matches!(ty, Type::Vec2 | Type::Vec3 | Type::Vec4);
        let refused = self.refusals.iter().filter(|refusal| {
            refusal.component == self.component
                && (refusal.path == path || (vector && axis_of(path, &refusal.path).is_some()))
        });
        for refusal in refused {
            ui.label("");
            ui.colored_label(ui.visuals().error_fg_color, &refusal.message);
            ui.end_row();
        }
    }
}

/// Whether `a` and `b` are one rotation, within float noise: a millionth in each component, about
/// a ten-thousandth of a degree, below the three decimals shown. `q` and `-q` are one rotation.
fn same_rotation(a: Quat, b: Quat) -> bool {
    let near = |b: Quat| (Vec4::from(a) - Vec4::from(b)).abs().max_element() < 1e-6;
    near(b) || near(-b)
}

/// `q`'s angles about x, y and z, in degrees, applied z first.
fn angles_of(q: Quat) -> [f32; 3] {
    let (z, y, x) = q.to_euler(EulerRot::ZYX);
    [x, y, z].map(math::to_degrees)
}

/// `entity`'s name, if it has a [`Name`](crate::Name) holding one. It reads it by name, as the
/// panels read the world, and lists the entity's components first, so that one without a name
/// builds no error, as getting the field it hasn't would, each frame it's drawn.
fn entity_name(world: &World, entity: Entity) -> Option<String> {
    let mut components = world.components_of(entity).ok()?;
    if !components.any(|component| component == "Name") {
        return None;
    }
    match world.get_field(entity, "Name", "0") {
        Ok(Value::String(name)) => Some(name),
        _ => None,
    }
}

/// What `entity`'s row in the Entities panel says: its name, if it has one, its ID, `id`, and its
/// components' types.
fn row_label(world: &World, entity: Entity, id: EntityId) -> String {
    let components = world.components_of(entity).into_iter().flatten();
    let components = components.collect::<Vec<_>>().join(", ");
    match entity_name(world, entity) {
        Some(name) => format!("{name}  {id}  {components}"),
        None => format!("{id}  {components}"),
    }
}

/// The command that creates an empty entity, the last of `parent`'s children, or a root: a name
/// that says so, and a transform that leaves it where its parent is.
fn empty(parent: Option<EntityId>) -> Command {
    let name = crate::Name("Empty Entity".to_owned());
    Command::Create {
        parent,
        components: vec![
            ("Name".into(), name.to_value()),
            ("Transform".into(), Transform::IDENTITY.to_value()),
        ],
    }
}

/// `entity`'s children, by their IDs, as "Children: 5f3a9c0e12b47d68, 0c41b2e79d6a3f15": the
/// first 16 of them, then how many more there are; `None` if it has none.
fn children(world: &World, entity: Entity) -> Option<String> {
    let mut children = world.children(entity).ok()?.peekable();
    children.peek()?;
    let shown: Vec<String> = children
        .by_ref()
        .take(16)
        .map(|child| world.id(child).expect("a child is alive").to_string())
        .collect();
    let mut line = format!("Children: {}", shown.join(", "));
    let more = children.count();
    if more > 0 {
        line += &format!(" and {more} more");
    }
    Some(line)
}

/// What's opaque: shown, but not read or set.
fn opaque(ui: &mut egui::Ui) -> Option<Value> {
    ui.add_enabled(false, egui::Label::new("opaque"));
    None
}

/// The least and the most an integer of `bits` may be, within `meta`'s bounds.
fn int_range(bits: u8, signed: bool, meta: &Meta) -> (i128, i128) {
    let (mut min, mut max) = if signed {
        (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
    } else {
        (0, (1i128 << bits) - 1)
    };
    match meta.min {
        Some(Bound::Int(bound)) => min = min.max(bound),
        Some(Bound::Float(bound)) => min = min.max(bound.ceil() as i128),
        None => {}
    }
    match meta.max {
        Some(Bound::Int(bound)) => max = max.min(bound),
        Some(Bound::Float(bound)) => max = max.min(bound.floor() as i128),
        None => {}
    }
    (min, max.max(min))
}

/// Drags `n` within `range`, which the field's bounds give, a speed for which they pick. Showing a
/// value outside them leaves it as it is.
fn drag<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    n: &mut N,
    range: std::ops::RangeInclusive<N>,
    meta: &Meta,
) -> bool {
    let speed = speed(meta, 0.25);
    let drag = egui::DragValue::new(n)
        .range(range)
        .clamp_existing_to_range(false)
        .speed(speed);
    ui.add(drag).changed()
}

/// How much a drag of a pixel changes a number of a field with `meta`: a 200th of its range if it
/// has bounds both ways, and `otherwise` if it hasn't.
fn speed(meta: &Meta, otherwise: f64) -> f64 {
    match (meta.min, meta.max) {
        (Some(min), Some(max)) if max.as_f64() > min.as_f64() => {
            (max.as_f64() - min.as_f64()) / 200.0
        }
        _ => otherwise,
    }
}

/// Drags `x`, an `f32`'s if `is_f32`, in degrees if `meta` says, and returns it if it changed: no
/// further than the field's bounds, as [`World::set_field`] compares them, and finite, so that
/// what's dragged or typed is a value the field takes. NaN is shown as text: a drag over it would
/// change it every frame.
fn float(ui: &mut egui::Ui, x: f64, meta: &Meta, is_f32: bool) -> Option<f64> {
    if x.is_nan() {
        ui.label(x.to_string());
        return None;
    }
    let degrees = meta.show == Show::Degrees;
    // From the field's unit to what's shown, which the bounds are in too.
    let scale = if degrees { 180.0 / PI } else { 1.0 };
    let (most, bound): (f64, fn(Bound) -> f64) = if is_f32 {
        (f32::MAX.into(), |bound| bound.as_f32().into())
    } else {
        (f64::MAX, Bound::as_f64)
    };
    let end = |given: Option<Bound>, otherwise: f64| {
        given.map_or(otherwise, bound).clamp(-most, most) * scale
    };
    let range = end(meta.min, -most)..=end(meta.max, most);
    let mut shown = x * scale;
    let mut drag = egui::DragValue::new(&mut shown)
        .range(range)
        .clamp_existing_to_range(false)
        .speed(speed(meta, if degrees { 0.5 / scale } else { 0.01 }) * scale);
    if degrees {
        drag = drag.suffix("°");
    }
    ui.add(drag).changed().then(|| shown / scale)
}

/// The button of axis `axis`, in its color, as high and as wide as a field's widget is high:
/// whether it was clicked.
fn axis_button(ui: &mut egui::Ui, axis: usize) -> bool {
    let (_, label, fill, hovered) = AXES[axis];
    let rgb = |[r, g, b]: [u8; 3]| egui::Color32::from_rgb(r, g, b);
    let size = egui::Vec2::splat(ui.spacing().interact_size.y);
    // Its own colors, in a scope of its own: `Button::fill` would keep it from lighting up as
    // it's hovered.
    ui.scope(|ui| {
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive.weak_bg_fill = rgb(fill);
        widgets.hovered.weak_bg_fill = rgb(hovered);
        widgets.active.weak_bg_fill = rgb(fill);
        let button = egui::Button::new(egui::RichText::new(label).strong()).min_size(size);
        ui.add(button).clicked()
    })
    .inner
}

/// Which component of the vector at `path` the path `at` leads to: 0 for `x` to 3 for `w`; `None`
/// if it leads elsewhere.
fn axis_of(path: &str, at: &str) -> Option<usize> {
    let rest = at.strip_prefix(path)?;
    let name = if path.is_empty() {
        rest
    } else {
        rest.strip_prefix('.')?
    };
    AXES.iter().position(|&(axis, ..)| axis == name)
}

/// Component `axis` of `value`, a vector; `None` if it's of another kind, or hasn't that one.
fn component_of(value: &Value, axis: usize) -> Option<f32> {
    match value {
        Value::Vec2(v) if axis < 2 => Some(v[axis]),
        Value::Vec3(v) if axis < 3 => Some(v[axis]),
        Value::Vec4(v) if axis < 4 => Some(v[axis]),
        _ => None,
    }
}

/// Sets component `axis` of `value`, a vector, to `x`; leaves it as it is if it's of another kind,
/// or hasn't that one.
fn set_component(value: &mut Value, axis: usize, x: f32) {
    match value {
        Value::Vec2(v) if axis < 2 => v[axis] = x,
        Value::Vec3(v) if axis < 3 => v[axis] = x,
        Value::Vec4(v) if axis < 4 => v[axis] = x,
        _ => {}
    }
}

/// Where in `value` the first thing it holds that's opaque is, which reflection keeps but doesn't
/// make, as a path, such as `rect[1]`; `None` if it holds nothing opaque.
fn opaque_at(value: &Value) -> Option<String> {
    match value {
        Value::Opaque => Some(String::new()),
        Value::Option(Some(inner)) => opaque_at(inner),
        Value::List(items) => items.iter().enumerate().find_map(|(i, item)| {
            let at = opaque_at(item)?;
            Some(reflect::join(&format!("[{i}]"), &at))
        }),
        Value::Struct(fields) | Value::Variant(_, fields) => {
            fields.iter().find_map(|(name, field)| {
                let at = opaque_at(field)?;
                Some(reflect::join(name, &at))
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::mem;
    use std::rc::Rc;

    use egui::PointerButton::{Primary, Secondary};

    use super::*;
    use crate::reflect::tests::{Everything, Form, Inner, everything, same_snapshots, snapshot};
    use crate::{Camera, Component, Projection, Sprite};

    /// `entity`'s ID, which the Inspector picks it by and names it by.
    fn id(world: &World, entity: Entity) -> EntityId {
        world.id(entity).expect("alive")
    }

    /// A text drawn, and where.
    type Text = (String, egui::Rect);

    /// Queues `pass`, as the panels do, and makes what it found at a tick, as the next one does.
    fn make(inspector: &mut Inspector, world: &mut World, pass: Pass) {
        inspector.queue(pass);
        Layer::tick(inspector, world, Tick { number: 0 });
    }

    /// Draws the two panels side by side, each in a half of `ui` of its own, as the dock gives each
    /// a rect of its own: below its rows, the Entities panel takes the rest of its half.
    fn panels(inspector: &mut Inspector, world: &World, ui: &mut egui::Ui, pass: &mut Pass) {
        let (left, right) = ui.max_rect().split_left_right_at_fraction(0.5);
        let entities = egui::UiBuilder::new().id_salt("Entities").max_rect(left);
        ui.scope_builder(entities, |ui| inspector.entities(world, ui, pass));
        let inspect = egui::UiBuilder::new().id_salt("Inspector").max_rect(right);
        ui.scope_builder(inspect, |ui| inspector.inspect(world, ui, pass));
    }

    /// A frame of the two panels' contents, on a screen big enough for all of them, given
    /// `events`, whose edits are then set at a tick; returns its texts, and how many edits it made.
    fn frame(
        inspector: &mut Inspector,
        world: &mut World,
        ctx: &egui::Context,
        time: f64,
        events: Vec<egui::Event>,
    ) -> (Vec<Text>, usize) {
        frame_ticking(inspector, world, ctx, time, events, true)
    }

    /// A frame as [`frame`] draws it, whose edits are set at a tick if `tick`, or else wait for
    /// one, as at a frame rate above the simulation's.
    fn frame_ticking(
        inspector: &mut Inspector,
        world: &mut World,
        ctx: &egui::Context,
        time: f64,
        events: Vec<egui::Event>,
        tick: bool,
    ) -> (Vec<Text>, usize) {
        let (shapes, made) = frame_shapes(inspector, world, ctx, time, events, tick);
        let mut texts = Vec::new();
        for clipped in &shapes {
            find_texts(&clipped.shape, &mut texts);
        }
        (texts, made)
    }

    /// A frame as [`frame_ticking`] draws it; returns its shapes, and how many edits it made.
    fn frame_shapes(
        inspector: &mut Inspector,
        world: &mut World,
        ctx: &egui::Context,
        time: f64,
        events: Vec<egui::Event>,
        tick: bool,
    ) -> (Vec<egui::epaint::ClippedShape>, usize) {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 4000.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(time),
            events,
            ..egui::RawInput::default()
        };
        let mut made = 0;
        let mut output = ctx.run_ui(input, |ui| {
            let mut pass = Pass::default();
            panels(inspector, world, ui, &mut pass);
            made = pass.sets.len();
            if tick {
                make(inspector, world, pass);
            } else {
                inspector.queue(pass);
            }
        });
        output.textures_delta.clear();
        (output.shapes, made)
    }

    fn find_texts(shape: &egui::Shape, texts: &mut Vec<Text>) {
        match shape {
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    find_texts(shape, texts);
                }
            }
            egui::Shape::Text(text) => {
                texts.push((text.galley.text().to_owned(), text.visual_bounding_rect()));
            }
            _ => {}
        }
    }

    /// Three frames, the last's text: a grid lays itself out unseen in its first.
    fn frames(inspector: &mut Inspector, world: &mut World, ctx: &egui::Context) -> Vec<String> {
        let mut texts = Vec::new();
        for time in 0..3 {
            let (shown, edits) = frame(inspector, world, ctx, f64::from(time), Vec::new());
            assert_eq!(edits, 0, "drawing edits nothing");
            texts = shown;
        }
        texts.into_iter().map(|(text, _)| text).collect()
    }

    #[test]
    fn drawing_the_panels_changes_nothing() {
        let mut world = World::default();
        let turned = Transform {
            rotation: Quat::from_rotation_z(1.0),
            ..Transform::IDENTITY
        };
        let turned = world.spawn(turned);
        let lost = Transform {
            rotation: Quat::from_xyzw(f32::NAN, 0.0, 0.0, 1.0),
            ..Transform::IDENTITY
        };
        let lost = world.spawn(lost);
        // Transparent, so that its color is lost if alpha is multiplied in.
        let sprite = Sprite {
            color: Vec4::new(0.3, 0.6, 0.9, 0.0),
            ..Sprite::default()
        };
        let tinted = world.spawn((Transform::IDENTITY, sprite));
        let odd = Everything {
            count: 200,
            angle: f32::NAN,
            offset: Vec3::new(f32::INFINITY, 0.0, 0.0),
            maybe: Some(Inner {
                level: 0,
                name: "kept".to_owned(),
            }),
            form: Form::Box {
                size: Vec2::ONE,
                turn: 0.5,
            },
            list: vec![1, 2, 3],
            ..everything()
        };
        let odd = world.spawn(odd);
        let [first, second] = [0, 1].map(|_| world.spawn(Transform::IDENTITY));
        for child in [first, second] {
            world.set_parent(child, Some(turned)).expect("no cycle");
        }
        let before = snapshot(&world);
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        let texts = frames(&mut inspector, &mut world, &ctx);
        let row = format!("{}  Transform", id(&world, turned));
        for text in [
            "6 entities",
            row.as_str(),
            "Nothing picked: pick an entity in the Entities panel",
        ] {
            assert!(texts.iter().any(|shown| shown == text), "{text}: {texts:?}");
        }
        let shown: [(Entity, &[&str]); 4] = [
            (turned, &["Transform", "translation", "rotation", "scale"]),
            (lost, &["rotation", "[NaN, 0, 0, 1]"]),
            (
                tinted,
                &["Sprite", "color", "texture", "rect", "[1]", "LinearClamp"],
            ),
            (
                odd,
                &[
                    "Everything",
                    "count",
                    "200",
                    "angle",
                    "NaN",
                    "offset",
                    "inf",
                    "Box",
                    "size",
                    "maybe",
                    "Some",
                    "kept",
                    "[2]",
                    "level",
                    "pair",
                    "0",
                    "handle",
                    "opaque",
                ],
            ),
        ];
        for (entity, names) in shown {
            inspector.pick(id(&world, entity));
            let texts = frames(&mut inspector, &mut world, &ctx);
            let header = format!("Entity {}", id(&world, entity));
            for name in names.iter().copied().chain([header.as_str()]) {
                assert!(texts.iter().any(|text| text == name), "{name}: {texts:?}");
            }
            let clash = texts.iter().find(|text| text.contains(" use of "));
            assert_eq!(clash, None, "egui saw an ID twice");
        }
        // A parent lists its children, and a child shows its parent, by their IDs.
        let [one, two] = [first, second].map(|child| id(&world, child));
        let children = format!("Children: {one}, {two}");
        let parent = id(&world, turned).to_string();
        for (entity, names) in [
            (turned, [children.as_str(), "Transform"]),
            (first, ["Parent", parent.as_str()]),
        ] {
            inspector.pick(id(&world, entity));
            let texts = frames(&mut inspector, &mut world, &ctx);
            for name in names {
                assert!(texts.iter().any(|text| text == name), "{name}: {texts:?}");
            }
        }
        assert!(same_snapshots(&snapshot(&world), &before));
    }

    #[test]
    fn a_parent_lists_its_first_16_children() {
        let mut world = World::default();
        let parent = world.spawn(Transform::IDENTITY);
        assert_eq!(children(&world, parent), None);
        let kids: Vec<Entity> = (0..18).map(|_| world.spawn(Transform::IDENTITY)).collect();
        for &kid in &kids {
            world.set_parent(kid, Some(parent)).expect("no cycle");
        }
        let shown = kids[..16].iter().map(|&kid| id(&world, kid).to_string());
        let shown: Vec<String> = shown.collect();
        let shown = format!("Children: {}", shown.join(", "));
        let listed = children(&world, parent);
        assert_eq!(listed, Some(format!("{shown} and 2 more")));
        for &kid in &kids[16..] {
            world.set_parent(kid, None).expect("a root");
        }
        assert_eq!(children(&world, parent), Some(shown));
    }

    /// A click of `button` at `at`: the pointer there, pressed and released in one frame, as egui
    /// takes a tap.
    fn click(button: egui::PointerButton, at: egui::Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![egui::Event::PointerMoved(at), button(true), button(false)]
    }

    fn enter() -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]
    }

    /// The pointer pressed at `from`, moved to `to` and released, each in a frame of its own.
    fn drag(from: egui::Pos2, to: egui::Pos2) -> [Vec<egui::Event>; 3] {
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        [
            vec![egui::Event::PointerMoved(from), button(from, true)],
            vec![egui::Event::PointerMoved(to)],
            vec![button(to, false)],
        ]
    }

    /// Where the text `name` is, the first drawn at or after the text `after`.
    fn find(texts: &[Text], after: &str, name: &str) -> egui::Rect {
        let start = texts.iter().position(|(text, _)| text == after);
        let start = start.unwrap_or_else(|| panic!("no {after}: {texts:?}"));
        let found = texts[start..].iter().find(|(text, _)| text == name);
        found.unwrap_or_else(|| panic!("no {name}: {texts:?}")).1
    }

    /// Where the last of `names` is, each the first drawn at or after the one before it, as
    /// `["rect", "[1]", "X"]` finds the button of the x of a field's second element.
    fn find_after(texts: &[Text], names: &[&str]) -> egui::Rect {
        let mut at = 0;
        for name in names {
            let found = texts[at..].iter().position(|(text, _)| text == name);
            at += found.unwrap_or_else(|| panic!("no {name} of {names:?}: {texts:?}"));
        }
        texts[at].1
    }

    /// What's drawn just right of `label`, on its row: the widget of its field.
    fn next_to(texts: &[Text], label: egui::Rect) -> &Text {
        let row = |rect: &egui::Rect| (rect.center().y - label.center().y).abs() < 4.0;
        let right = texts.iter();
        let right = right.filter(|(_, rect)| row(rect) && rect.min.x > label.max.x);
        let nearest = right.min_by(|a, b| a.1.min.x.total_cmp(&b.1.min.x));
        nearest.expect("a widget beside the label")
    }

    /// The middle of what's drawn just right of `label`, on its row: the widget of its field.
    fn beside(texts: &[Text], label: egui::Rect) -> egui::Pos2 {
        next_to(texts, label).1.center()
    }

    #[test]
    fn the_find_field_picks_an_entity_by_a_pasted_id() {
        let mut world = World::default();
        let [first, second, gone] = [0, 1, 2].map(|_| world.spawn(Transform::IDENTITY));
        let [first, second, gone] = [first, second, gone].map(|entity| id(&world, entity));
        world
            .despawn(world.entity(gone).expect("alive"))
            .expect("alive");
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        let mut time = 0.0;
        let mut run = |inspector: &mut Inspector, world: &mut World, events| {
            let mut texts = Vec::new();
            for events in [events, Vec::new(), Vec::new()] {
                time += 1.0;
                texts = frame(inspector, world, &ctx, time, events).0;
            }
            texts
        };
        let texts = run(&mut inspector, &mut world, Vec::new());
        let field = beside(&texts, find(&texts, "Find", "Find"));
        let bad = "`5F3A9C0E12B47D68` isn't an entity's ID: an ID is 16 lowercase hex digits, not \
                   all zero, such as `5f3a9c0e12b47d68`";
        let missing = format!("no entity has ID {gone}");
        // Quoted, as a scene writes it; refused, keeping the pick; and another.
        for (typed, picked, says) in [
            (format!(" \"{first}\" "), first, None),
            ("5F3A9C0E12B47D68".to_owned(), first, Some(bad)),
            (gone.to_string(), first, Some(missing.as_str())),
            (second.to_string(), second, None),
        ] {
            inspector.find.clear();
            run(&mut inspector, &mut world, click(Primary, field));
            run(
                &mut inspector,
                &mut world,
                vec![egui::Event::Text(typed.clone())],
            );
            let texts = run(&mut inspector, &mut world, enter());
            assert_eq!(inspector.picked, Some(picked), "{typed}");
            let header = format!("Entity {picked}");
            assert!(texts.iter().any(|(text, _)| *text == header), "{texts:?}");
            let shown = |says: &str| texts.iter().any(|(text, _)| text == says);
            match says {
                Some(says) => assert!(shown(says), "{says}: {texts:?}"),
                None => assert!(!shown(bad) && !shown(&missing), "{texts:?}"),
            }
        }
    }

    /// `name` as the component holds it: the `Name` here is reflection's.
    fn called(name: &str) -> crate::Name {
        crate::Name(name.to_owned())
    }

    /// Names in a field and in an option, which a tuple struct of one field shows alone, and sets
    /// at its own path or whole; and an enum whose variant has a field, which keeps its label.
    #[derive(Clone, Component)]
    struct Shaped {
        name: crate::Name,
        maybe: Option<crate::Name>,
        form: Form,
    }

    fn shaped() -> Shaped {
        Shaped {
            name: called("Bob"),
            maybe: Some(called("Cy")),
            form: Form::Circle(2.5),
        }
    }

    #[test]
    fn an_entity_with_a_name_shows_by_it_and_its_id() {
        let mut world = World::default();
        let named = world.spawn((called("Ada"), Transform::IDENTITY));
        let plain = world.spawn(Transform::IDENTITY);
        let [named_id, plain_id] = [named, plain].map(|entity| id(&world, entity));
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        // The name leads a row, and an entity without one keeps the row it had.
        let texts = frames(&mut inspector, &mut world, &ctx);
        for row in [
            format!("Ada  {named_id}  Name, Transform"),
            format!("{plain_id}  Transform"),
        ] {
            assert!(texts.contains(&row), "{row}: {texts:?}");
        }
        for (picked, heading) in [
            (named_id, format!("Ada, entity {named_id}")),
            (plain_id, format!("Entity {plain_id}")),
        ] {
            inspector.pick(picked);
            let texts = frames(&mut inspector, &mut world, &ctx);
            assert!(texts.contains(&heading), "{heading}: {texts:?}");
        }
        // Despawned, a pick has no name left to show.
        inspector.pick(named_id);
        world.despawn(named).expect("alive");
        let texts = frames(&mut inspector, &mut world, &ctx);
        let despawned = format!("Entity {named_id} was despawned");
        assert!(texts.contains(&despawned), "{texts:?}");
    }

    #[test]
    fn a_tuple_of_one_field_shows_its_field_without_a_label() {
        let mut world = World::default();
        let named = world.spawn(called("Ada"));
        let shape = world.spawn(shaped());
        let [parent, child] = [(); 2].map(|()| world.spawn(()));
        world.set_parent(child, Some(parent)).expect("no cycle");
        let parent = id(&world, parent).to_string();
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        // A name or a parent shows as its field alone, and so does a name in a field or an option;
        // the field of an enum's variant keeps its label, `0`.
        let picks: [(Entity, &[&str], usize); 3] = [
            (named, &["Name", "Ada"], 0),
            (child, &["Parent", parent.as_str()], 0),
            (shape, &["name", "Bob", "maybe", "Some", "Cy", "Circle"], 1),
        ];
        for (entity, names, zeros) in picks {
            inspector.pick(id(&world, entity));
            let texts = frames(&mut inspector, &mut world, &ctx);
            for name in names {
                assert!(texts.iter().any(|text| text == name), "{name}: {texts:?}");
            }
            let labels = texts.iter().filter(|text| *text == "0").count();
            assert_eq!(labels, zeros, "{texts:?}");
        }
    }

    /// A name typed into is set at its field's path, `0` as a component and `name.0` in a field,
    /// and whole in an option.
    #[test]
    fn a_tuple_of_one_field_is_set_at_its_own_path_or_whole() {
        let mut world = World::default();
        let named = world.spawn(called("Ada"));
        let shape = world.spawn(shaped());
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        let mut time = 0.0;
        // A frame given `events`, then two that settle what they did; the last one's texts.
        let mut run = |inspector: &mut Inspector, world: &mut World, events| {
            let mut texts = Vec::new();
            for events in [events, Vec::new(), Vec::new()] {
                time += 1.0;
                texts = frame(inspector, world, &ctx, time, events).0;
            }
            texts
        };
        // An x typed into each name as a person would: clicked left of its text, which puts the
        // cursor first, and typed.
        let mut texts = Vec::new();
        for (entity, after, name) in [
            (named, "Name", "Ada"),
            (shape, "Shaped", "Bob"),
            (shape, "maybe", "Cy"),
        ] {
            inspector.pick(id(&world, entity));
            let shown = run(&mut inspector, &mut world, Vec::new());
            let rect = find(&shown, after, name);
            run(
                &mut inspector,
                &mut world,
                click(Primary, egui::pos2(rect.min.x - 3.0, rect.center().y)),
            );
            let typed = vec![egui::Event::Text("x".to_owned())];
            texts = run(&mut inspector, &mut world, typed);
        }
        assert!(inspector.refusals.is_empty(), "{texts:?}");
        assert_eq!(world.get::<crate::Name>(named), Some(&called("xAda")));
        let held = world.get::<Shaped>(shape).expect("shaped");
        assert_eq!(held.name, called("xBob"));
        assert_eq!(held.maybe, Some(called("xCy")));
        // The Entities panel shows the name as it is now.
        let row = format!("xAda  {}  Name", id(&world, named));
        assert!(texts.iter().any(|(text, _)| *text == row), "{texts:?}");
    }

    /// A value refused for the field of a tuple struct shows under it, not in the row of the grid
    /// it's in.
    #[test]
    fn a_value_refused_for_a_tuple_of_one_field_shows_under_it() {
        let mut world = World::default();
        let entity = world.spawn(shaped());
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, entity));
        let refused = Edit {
            component: "Shaped".to_owned(),
            path: "name.0".to_owned(),
            value: Value::UInt(1),
        };
        inspector.apply(&mut world, inspector.picked, refused);
        let says = format!(
            "can't set `Shaped.name.0` of entity {}: expected String, got UInt",
            id(&world, entity)
        );
        let mut time = 0.0;
        let mut texts = Vec::new();
        for _ in 0..3 {
            time += 1.0;
            texts = frame(&mut inspector, &mut world, &ctx, time, Vec::new()).0;
        }
        let (name, message) = (find(&texts, "name", "Bob"), find(&texts, "Bob", &says));
        assert!(message.min.y > name.max.y, "{texts:?}");
    }

    #[test]
    fn widgets_set_the_fields_they_show() {
        let mut world = World::default();
        let odd = Everything {
            maybe: Some(Inner {
                level: 0,
                name: "kept".to_owned(),
            }),
            form: Form::Box {
                size: Vec2::ONE,
                turn: 0.5,
            },
            // Infinite, as code may leave it, which a number typed sets back.
            offset: Vec3::new(f32::INFINITY, -0.0, 3.0),
            ..everything()
        };
        let entity = world.spawn(odd.clone());
        // Transparent, so that its color is lost if alpha is multiplied in; one whose bits egui's
        // popup moves as it passes it through gamma.
        let tint = Vec4::new(0.3, 0.6, 0.9, 0.0);
        let sprite = Sprite {
            color: tint,
            ..Sprite::default()
        };
        let sprite = world.spawn(sprite);
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        let mut time = 0.0;
        // A frame given `events`, then two that settle what they did; the last one's texts.
        let mut run = |inspector: &mut Inspector, world: &mut World, events| {
            let mut texts = Vec::new();
            for events in [events, Vec::new(), Vec::new()] {
                time += 1.0;
                texts = frame(inspector, world, &ctx, time, events).0;
            }
            texts
        };
        inspector.pick(id(&world, entity));
        let mut texts = run(&mut inspector, &mut world, Vec::new());
        // Each number typed in as a person would: clicked, which selects its text, typed over and
        // entered; a vector's beside the buttons of their axes.
        let typed: [(&[&str], &str); 5] = [
            (&["maybe", "level"], "1"),
            (&["rect", "[1]", "X"], "0.75"),
            (&["form", "turn"], "90"),
            (&["angle"], "999"),
            (&["offset", "X"], "1e39"),
        ];
        for (anchor, typed) in typed {
            let at = beside(&texts, find_after(&texts, anchor));
            run(&mut inspector, &mut world, click(Primary, at));
            let text = vec![egui::Event::Text(typed.to_owned())];
            run(&mut inspector, &mut world, text);
            texts = run(&mut inspector, &mut world, enter());
        }
        assert!(inspector.refusals.is_empty(), "{texts:?}");
        let all = world.get::<Everything>(entity).expect("everything");
        let Form::Box { turn, .. } = all.form else {
            panic!("still a box: {:?}", all.form);
        };
        assert!((turn - std::f32::consts::FRAC_PI_2).abs() < 1e-6, "{turn}");
        let expected = Everything {
            maybe: Some(Inner {
                level: 1,
                name: "kept".to_owned(),
            }),
            rect: [Vec2::ZERO, Vec2::new(0.75, 1.0)],
            form: Form::Box {
                size: Vec2::ONE,
                turn,
            },
            // Typed past its max, it stops there, at the f32 the field takes.
            angle: 3.2,
            // Past what an f32 holds, it stops at the most one does, rather than become infinite.
            offset: Vec3::new(f32::MAX, -0.0, 3.0),
            ..odd
        };
        assert_eq!(all, &expected);

        inspector.pick(id(&world, sprite));
        let texts = run(&mut inspector, &mut world, Vec::new());
        // Its color's button, in the column the texture's "opaque" starts: its popup opens.
        let at = egui::pos2(
            find(&texts, "texture", "opaque").min.x + 10.0,
            find(&texts, "Sprite", "color").center().y,
        );
        let opened = run(&mut inspector, &mut world, click(Primary, at));
        // Open a while, as egui passes it through gamma each frame.
        for _ in 0..3 {
            run(&mut inspector, &mut world, Vec::new());
        }
        let bits = |color: Vec4| color.to_array().map(f32::to_bits);
        let set = world.get::<Sprite>(sprite).expect("a sprite").color;
        assert_eq!(bits(set), bits(tint));
        // Closed by a click away from it, the sampler's choice is opened and picked from.
        let away = click(Primary, egui::pos2(1500.0, 3900.0));
        run(&mut inspector, &mut world, away);
        let at = beside(&texts, find(&texts, "Sprite", "sampler"));
        let texts = run(&mut inspector, &mut world, click(Primary, at));
        let at = find(&texts, "sampler", "NearestRepeat").center();
        run(&mut inspector, &mut world, click(Primary, at));
        let set = world.get::<Sprite>(sprite).expect("a sprite");
        assert_eq!(set.sampler, crate::Sampler::NearestRepeat);
        assert_eq!(bits(set.color), bits(tint));
        let copy = opened.iter().any(|(text, _)| text == "📋");
        assert!(copy, "the color's popup opened: {opened:?}");
    }

    #[test]
    fn a_gray_keeps_the_hue_picked_for_it() {
        // White, as a sprite is untinted, and a gray whose value gamma's round trip moves; with a
        // tick each frame, and with frames that draw the sets waiting for one, as at frame rates
        // above the simulation's.
        for gray in [Vec4::ONE, Vec4::new(0.9, 0.9, 0.9, 1.0)] {
            for every_frame in [true, false] {
                a_gray_keeps_its_hue(gray, every_frame);
            }
        }
    }

    fn a_gray_keeps_its_hue(gray: Vec4, every_frame: bool) {
        let mut world = World::default();
        let sprite = Sprite {
            color: gray,
            ..Sprite::default()
        };
        let sprite = world.spawn(sprite);
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        let mut time = 0.0;
        // A frame given `events`, then two that settle what they did, the last with a tick, if not
        // each; the last one's texts.
        let mut run = |inspector: &mut Inspector, world: &mut World, events| {
            let mut texts = Vec::new();
            for (i, events) in [events, Vec::new(), Vec::new()].into_iter().enumerate() {
                time += 1.0;
                let tick = every_frame || i == 2;
                texts = frame_ticking(inspector, world, &ctx, time, events, tick).0;
            }
            texts
        };
        inspector.pick(id(&world, sprite));
        let texts = run(&mut inspector, &mut world, Vec::new());
        let at = egui::pos2(
            find(&texts, "texture", "opaque").min.x + 10.0,
            find(&texts, "Sprite", "color").center().y,
        );
        let opened = run(&mut inspector, &mut world, click(Primary, at));
        // The popup's hue slider, found by its tooltip below its first row, about halfway along.
        let copy = find(&opened, "📋", "📋");
        let x = copy.center().x + 70.0;
        let below = (0..100).map(|step| copy.max.y + 4.0 * step as f32);
        let hue = below.into_iter().find(|&y| {
            let moved = vec![egui::Event::PointerMoved(egui::pos2(x, y))];
            let texts = run(&mut inspector, &mut world, moved);
            texts.iter().any(|(text, _)| text == "Hue")
        });
        let hue = egui::pos2(x, hue.expect("the hue's slider") + 4.0);
        // Clicked, so that it's picked once: a drag would pick it again each frame it's held.
        run(&mut inspector, &mut world, click(Primary, hue));
        // A hue alone leaves it gray, though it may be one whose last bits gamma moves.
        let color = world.get::<Sprite>(sprite).expect("a sprite").color;
        let still = color.x == color.y && color.y == color.z;
        assert!(
            still && (color - gray).abs().max_element() < 1e-6,
            "{color}"
        );
        // Saturated from the square above the slider, it takes the hue picked, not red.
        let square = egui::pos2(x, hue.y - 250.0);
        for events in drag(square, square + egui::vec2(5.0, 0.0)) {
            run(&mut inspector, &mut world, events);
        }
        let [r, g, b, a] = world.get::<Sprite>(sprite).expect("a sprite").color.into();
        let picked = Hsva::from_rgba_unmultiplied(r, g, b, a);
        assert!(
            picked.s > 0.3 && (0.2..0.8).contains(&picked.h),
            "{gray}: {picked:?}"
        );
    }

    #[test]
    fn angles_typed_stay_while_a_system_renormalizes_the_rotation() {
        let typed = Quat::from_rotation_z(math::to_radians(200.0));
        let mut world = World::default();
        let turned = Transform {
            rotation: typed,
            ..Transform::IDENTITY
        };
        let entity = world.spawn(turned);
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, entity));
        // As the widget keeps them once 200° is typed: from the rotation alone, it shows -160°.
        let id = egui::Id::unique(("inspector rotation", "Transform", "rotation"));
        ctx.data_mut(|data| data.insert_temp(id, (typed, [0.0f32, 0.0, 200.0])));
        // A bit off in each component, as renormalizing it at each tick leaves it.
        let nudged = typed.to_array().map(|x| f32::from_bits(x.to_bits() + 1));
        world
            .get_mut::<Transform>(entity)
            .expect("a transform")
            .rotation = Quat::from_array(nudged).normalize();
        // egui draws the number apart from its suffix.
        let texts = frames(&mut inspector, &mut world, &ctx);
        assert!(texts.iter().any(|text| text == "200.0"), "{texts:?}");
        // Turned by something else, it shows the new rotation's own angles.
        world
            .get_mut::<Transform>(entity)
            .expect("a transform")
            .rotation = Quat::from_rotation_z(math::to_radians(10.0));
        let texts = frames(&mut inspector, &mut world, &ctx);
        assert!(texts.iter().any(|text| text == "10.0"), "{texts:?}");
        // 200° typed again, and set at a tick that hasn't come yet: the frames before it draw the
        // rotation as the set will make it, and so keep the angles typed.
        ctx.data_mut(|data| data.insert_temp(id, (typed, [0.0f32, 0.0, 200.0])));
        let set = Edit {
            component: "Transform".to_owned(),
            path: "rotation".to_owned(),
            value: Value::Quat(typed),
        };
        inspector.queue(Pass {
            sets: vec![set],
            ..Pass::default()
        });
        frame_ticking(&mut inspector, &mut world, &ctx, 3.0, Vec::new(), false);
        Layer::tick(&mut inspector, &mut world, Tick { number: 0 });
        let texts = frames(&mut inspector, &mut world, &ctx);
        assert!(texts.iter().any(|text| text == "200.0"), "{texts:?}");
    }

    #[test]
    fn an_edit_applies_at_the_next_tick_to_the_entity_picked_as_it_was_made() {
        let mut world = World::default();
        let [a, b] = [(); 2].map(|()| world.spawn(everything()));
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, a));
        let count = |value| Edit {
            component: "Everything".to_owned(),
            path: "count".to_owned(),
            value,
        };
        // A set, and then one refused, before another entity is picked.
        for value in [7, 200] {
            inspector.queue(Pass {
                sets: vec![count(Value::UInt(value))],
                ..Pass::default()
            });
        }
        inspector.pick(id(&world, b));
        let counts = |world: &World| {
            [a, b].map(|entity| world.get::<Everything>(entity).expect("everything").count)
        };
        assert_eq!(counts(&world), [42, 42]);
        Layer::tick(&mut inspector, &mut world, Tick { number: 0 });
        assert_eq!(counts(&world), [7, 42]);
        assert_eq!(inspector.log.operations().len(), 1);
        // The refusal, of a field of the entity picked before, isn't shown beside the same field
        // of this one.
        assert!(inspector.refusals.is_empty());
    }

    #[test]
    fn a_value_refused_shows_beside_its_field_until_one_is_set() {
        let mut world = World::default();
        let entity = world.spawn(everything());
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, entity));
        let count = |value| Edit {
            component: "Everything".to_owned(),
            path: "count".to_owned(),
            value,
        };
        inspector.apply(&mut world, inspector.picked, count(Value::UInt(200)));
        let refused = format!(
            "can't set `Everything.count` of entity {}: 200 is above the field's max, 100",
            id(&world, entity)
        );
        let texts = frames(&mut inspector, &mut world, &ctx);
        assert!(texts.contains(&refused), "{texts:?}");
        assert_eq!(
            world.get::<Everything>(entity).expect("everything").count,
            42
        );
        inspector.apply(&mut world, inspector.picked, count(Value::UInt(99)));
        assert_eq!(
            world.get::<Everything>(entity).expect("everything").count,
            99
        );
        let texts = frames(&mut inspector, &mut world, &ctx);
        assert!(!texts.contains(&refused), "{texts:?}");
        // Another entity picked starts without the last one's.
        inspector.apply(&mut world, inspector.picked, count(Value::UInt(200)));
        let other = world.spawn(everything());
        inspector.pick(id(&world, other));
        assert!(inspector.refusals.is_empty());
    }

    /// A pick despawned is named by the ID it was shown by, and an entity field holding a handle
    /// whose entity is gone shows the handle, not the ID of the entity now in its slot.
    #[test]
    fn the_despawned_are_named_by_what_they_were_shown_by() {
        let mut world = World::default();
        let gone = world.spawn(Transform::IDENTITY);
        world.despawn(gone).expect("alive");
        // In the slot `gone` left, the last freed being taken first.
        let occupant = world.spawn(Transform::IDENTITY);
        let holder = world.spawn(Everything {
            target: Some(gone),
            ..everything()
        });
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, holder));
        let texts = frames(&mut inspector, &mut world, &ctx);
        assert!(texts.contains(&gone.to_string()), "{texts:?}");
        let taken = id(&world, occupant).to_string();
        assert!(!texts.contains(&taken), "{texts:?}");
        let picked = id(&world, holder);
        world.despawn(holder).expect("alive");
        let texts = frames(&mut inspector, &mut world, &ctx);
        let despawned = format!("Entity {picked} was despawned");
        assert!(texts.contains(&despawned), "{texts:?}");
    }

    /// `key` pressed with Ctrl, or Cmd on macOS, and with Shift too if `shift`.
    fn shortcut(key: egui::Key, shift: bool) -> Vec<egui::Event> {
        let modifiers = match shift {
            true => Modifiers::COMMAND.plus(Modifiers::SHIFT),
            false => Modifiers::COMMAND,
        };
        vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }]
    }

    /// The pointer pressed at `at`, moved right three times and released, each in a frame of its
    /// own, so that a drag sets its field in several frames.
    fn slide(at: egui::Pos2) -> Vec<Vec<egui::Event>> {
        let button = |pressed, pos| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        let to = |step: f32| at + egui::vec2(10.0 * step, 0.0);
        let mut gesture = vec![vec![egui::Event::PointerMoved(at), button(true, at)]];
        gesture.extend([1.0, 2.0, 3.0].map(|step| vec![egui::Event::PointerMoved(to(step))]));
        gesture.push(vec![button(false, to(3.0))]);
        gesture
    }

    #[test]
    fn a_drag_is_one_edit_which_the_keys_undo_and_redo() {
        let mut world = World::default();
        let entity = world.spawn(everything());
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, entity));
        let mut time = 0.0;
        let mut run = |inspector: &mut Inspector, world: &mut World, events| {
            time += 1.0;
            frame(inspector, world, &ctx, time, events)
        };
        // Shown above the fields until the drag's first set, so that they move as it goes on.
        inspector.not_done = Some("can't undo it".to_owned());
        let mut texts = Vec::new();
        for _ in 0..3 {
            texts = run(&mut inspector, &mut world, Vec::new()).0;
        }
        let at = beside(&texts, find(&texts, "Everything", "weight"));
        let weight = |world: &World| world.get::<Everything>(entity).expect("everything").weight;
        let mut sets = 0;
        for events in slide(at) {
            sets += run(&mut inspector, &mut world, events).1;
        }
        let dragged = weight(&world);
        assert!(sets > 1 && dragged != 2.5, "{sets} sets, to {dragged}");
        assert_eq!(inspector.log.operations().len(), 1, "one edit");
        for (keys, expected) in [
            (shortcut(egui::Key::Z, false), 2.5),
            (shortcut(egui::Key::Z, true), dragged),
            (shortcut(egui::Key::Z, false), 2.5),
            (shortcut(egui::Key::Y, false), dragged),
        ] {
            run(&mut inspector, &mut world, keys);
            assert_eq!(weight(&world).to_bits(), expected.to_bits());
        }
    }

    #[test]
    fn frames_that_change_nothing_queue_nothing() {
        let mut world = World::default();
        let entity = world.spawn(everything());
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, entity));
        let mut time = 0.0;
        let mut run = |inspector: &mut Inspector, world: &mut World, events, tick| {
            time += 1.0;
            frame_ticking(inspector, world, &ctx, time, events, tick)
        };
        let mut texts = Vec::new();
        for _ in 0..3 {
            texts = run(&mut inspector, &mut world, Vec::new(), true).0;
        }
        assert!(inspector.queued.is_empty(), "drawing queues nothing");
        let at = beside(&texts, find(&texts, "Everything", "weight"));
        let weight = |world: &World| world.get::<Everything>(entity).expect("everything").weight;
        for _ in 0..2 {
            for events in slide(at) {
                run(&mut inspector, &mut world, events, true);
            }
            // Frames between the drags, which wait for a tick, and end no edit.
            for _ in 0..10 {
                run(&mut inspector, &mut world, Vec::new(), false);
            }
            assert!(inspector.queued.is_empty(), "idle frames queue nothing");
        }
        assert_eq!(inspector.log.operations().len(), 2, "two edits");
        for _ in 0..2 {
            run(
                &mut inspector,
                &mut world,
                shortcut(egui::Key::Z, false),
                true,
            );
        }
        assert_eq!(weight(&world).to_bits(), 2.5f64.to_bits());
    }

    #[test]
    fn its_commands_are_values_that_queue_them_again() {
        let mut world = World::default();
        let entity = world.spawn(everything());
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, entity));
        let set = |path: &str, value| Edit {
            component: "Everything".to_owned(),
            path: path.to_owned(),
            value,
        };
        let target = Value::Option(Some(Box::new(Value::Entity(entity))));
        let sets = vec![set("count", Value::UInt(7)), set("target", target)];
        let pressed = || Cues {
            pressed: true,
            held: true,
            ..Cues::default()
        };
        inspector.queue(Pass {
            sets,
            cues: pressed(),
            ..Pass::default()
        });
        for step in [Step::Undo, Step::Redo] {
            inspector.queue(Pass {
                chosen: Some(step),
                ..Pass::default()
            });
        }
        // A set, and then operations, the first of which ends its edit, so that no seal does.
        inspector.queue(Pass {
            sets: vec![set("count", Value::UInt(8))],
            cues: pressed(),
            ..Pass::default()
        });
        let picked = id(&world, entity);
        let operations = vec![
            empty(None),
            empty(Some(picked)),
            Command::Delete(picked),
            Command::Add(picked, "Sprite".to_owned(), Sprite::default().to_value()),
            Command::Remove(picked, "Everything".to_owned()),
        ];
        inspector.queue(Pass {
            operations,
            ..Pass::default()
        });
        let values = |queue: &Queue| (0..queue.len()).map(|i| queue.value(i)).collect::<Vec<_>>();
        let queued = values(&inspector.queued);
        let names: Vec<&str> = queued
            .iter()
            .map(|value| reflect::variant(value).expect("a variant").0)
            .collect();
        // The press began no edit, since none went on, and the release ended the one it began.
        assert_eq!(
            names,
            [
                "Set", "Set", "Seal", "Undo", "Redo", "Set", "Create", "Create", "Delete", "Add",
                "Remove"
            ]
        );
        let mut again = Queue::default();
        for value in &queued {
            again
                .push(value)
                .unwrap_or_else(|error| panic!("{value:?}: {error}"));
        }
        assert_eq!(values(&again), queued);
        let named = |name: &str, value| (Name::Owned(name.to_owned()), value);
        let fields = |entity: &str| {
            vec![
                named("entity", Value::String(entity.to_owned())),
                named("component", Value::String("Everything".to_owned())),
                named("path", Value::String("count".to_owned())),
                named("value", Value::UInt(1)),
            ]
        };
        for (value, code) in [
            (
                Value::Variant("Set".into(), fields("5F3A9C0E12B47D68")),
                "ecs.bad_id",
            ),
            (
                Value::Variant("Set".into(), fields("")[1..].to_vec()),
                "reflect.missing_field",
            ),
            (
                Value::Variant("Seal".into(), fields("")),
                "reflect.no_field",
            ),
            (
                Value::Variant("Cut".into(), Vec::new()),
                "reflect.no_variant",
            ),
            (Value::UInt(1), "reflect.wrong_kind"),
        ] {
            let error = again.push(&value).expect_err("refused");
            assert_eq!(error.code(), code, "{value:?}: {error}");
        }
        let error = again.push(&Value::Variant("Cut".into(), Vec::new()));
        let error = error.expect_err("no such command").to_string();
        assert!(
            error.contains("`Create`, `Delete`, `Add` and `Remove`"),
            "{error}"
        );
        // The operations' values, refused where they're wrong.
        let text = |text: &str| Value::String(text.to_owned());
        let create = |parent, components| {
            let fields = vec![named("parent", parent), named("components", components)];
            Value::Variant("Create".into(), fields)
        };
        let bad_id = Value::Option(Some(Box::new(text("5F3A9C0E12B47D68"))));
        let entity = picked.to_string();
        let add = vec![
            named("entity", text(&entity)),
            named("component", Value::UInt(1)),
            named("value", Value::Struct(Vec::new())),
        ];
        for (value, code, at) in [
            (
                create(text(&entity), Value::Struct(Vec::new())),
                "reflect.wrong_kind",
                "parent",
            ),
            (
                create(bad_id, Value::Struct(Vec::new())),
                "ecs.bad_id",
                "parent",
            ),
            (
                create(Value::Option(None), Value::List(Vec::new())),
                "reflect.wrong_kind",
                "components",
            ),
            (
                Value::Variant("Delete".into(), vec![named("entity", Value::UInt(1))]),
                "reflect.wrong_kind",
                "entity",
            ),
            (
                Value::Variant("Add".into(), add),
                "reflect.wrong_kind",
                "component",
            ),
            (
                Value::Variant("Remove".into(), fields(&entity)[..1].to_vec()),
                "reflect.missing_field",
                "",
            ),
        ] {
            let error = again.push(&value).expect_err("refused");
            assert_eq!(error.code(), code, "{value:?}: {error}");
            let says = format!("at `{at}`: ");
            assert_eq!(
                error.to_string().starts_with(&says),
                !at.is_empty(),
                "{error}"
            );
        }
        assert_eq!(again.len(), queued.len());
    }

    /// Adds and removes, queued as a recording replays them, are made at the next tick, through
    /// the log, whose undo takes them back; and an add or a remove forgets what its component
    /// refused. One refused shows above the components.
    #[test]
    fn components_are_added_and_removed_at_the_next_tick() {
        let mut world = World::default();
        let entity = world.spawn((Transform::IDENTITY, everything()));
        world.register::<Sprite>().expect("registered");
        let picked = id(&world, entity);
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(picked);
        let refused = |component: &str, path: &str, value| Edit {
            component: component.to_owned(),
            path: path.to_owned(),
            value,
        };
        // Above its bounds, and of a component it hasn't yet.
        let count = refused("Everything", "count", Value::UInt(200));
        inspector.apply(&mut world, Some(picked), count);
        let tint = refused("Sprite", "color", Value::F32(1.0));
        inspector.apply(&mut world, Some(picked), tint);
        assert_eq!(inspector.refusals.len(), 2);
        let named = |name: &str, value| (Name::Owned(name.to_owned()), value);
        let text = |text: &str| Value::String(text.to_owned());
        let operation = |name: &str, component: &str, value: Option<Value>| {
            let mut fields = vec![
                named("entity", text(&picked.to_string())),
                named("component", text(component)),
            ];
            fields.extend(value.map(|value| named("value", value)));
            Value::Variant(name.to_owned().into(), fields)
        };
        let sprite = Sprite {
            color: Vec4::new(0.5, 0.25, 1.0, 1.0),
            ..Sprite::default()
        };
        let tick = |inspector: &mut Inspector, world: &mut World, values: &[Value]| {
            for value in values {
                let pushed = Commands::push(&mut inspector.queued, value);
                pushed.unwrap_or_else(|error| panic!("{value:?}: {error}"));
            }
            Layer::tick(inspector, world, Tick { number: 0 });
        };
        let added = operation("Add", "Sprite", Some(sprite.to_value()));
        let removed = operation("Remove", "Everything", None);
        tick(&mut inspector, &mut world, &[added, removed]);
        let names: Vec<&str> = world.components_of(entity).expect("alive").collect();
        assert_eq!(names, ["Transform", "Sprite"]);
        let color = world.get::<Sprite>(entity).expect("a sprite").color;
        assert_eq!(color, sprite.color);
        assert!(inspector.refusals.is_empty());
        assert_eq!(inspector.log.operations().len(), 2);
        // Refused, since it has one, and said so above its components.
        let again = operation("Add", "Transform", Some(Transform::IDENTITY.to_value()));
        tick(&mut inspector, &mut world, &[again]);
        let says = inspector.not_done.clone().expect("refused");
        let starts = format!("can't add `Transform` to {picked}: ");
        assert!(says.starts_with(&starts), "{says}");
        let mut texts = Vec::new();
        for time in 0..3 {
            texts = frame(
                &mut inspector,
                &mut world,
                &ctx,
                f64::from(time),
                Vec::new(),
            )
            .0;
        }
        let heading = format!("Entity {picked}");
        assert!(find(&texts, &says, &says).max.y < find(&texts, &says, &heading).min.y);
        // Undone, each comes back as it was, and the refusal goes.
        for _ in 0..2 {
            inspector.queue(Pass {
                chosen: Some(Step::Undo),
                ..Pass::default()
            });
            Layer::tick(&mut inspector, &mut world, Tick { number: 0 });
        }
        assert_eq!(inspector.not_done, None);
        assert_eq!(world.get::<Everything>(entity), Some(&everything()));
        assert!(world.get::<Sprite>(entity).is_none());
    }

    #[test]
    fn a_number_typed_is_one_edit() {
        // With a tick each frame, and every other one, as at frame rates above the simulation's,
        // whose frames between draw the sets waiting for one.
        for every in [1, 2] {
            a_number_is_typed(every);
        }
    }

    fn a_number_is_typed(every: u32) {
        let mut world = World::default();
        let entity = world.spawn(everything());
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, entity));
        let mut frames = 0;
        let mut run = |inspector: &mut Inspector, world: &mut World, events| {
            frames += 1;
            let tick = frames % every == 0;
            frame_ticking(inspector, world, &ctx, f64::from(frames), events, tick)
        };
        let mut texts = Vec::new();
        for _ in 0..3 {
            texts = run(&mut inspector, &mut world, Vec::new()).0;
        }
        let at = beside(&texts, find(&texts, "Everything", "weight"));
        let weight = |world: &World| world.get::<Everything>(entity).expect("everything").weight;
        // Clicked, which selects its text, typed over a key a frame, and entered.
        run(&mut inspector, &mut world, click(Primary, at));
        let mut sets = 0;
        for typed in ["0", ".", "7", "5"] {
            let text = vec![egui::Event::Text(typed.to_owned())];
            sets += run(&mut inspector, &mut world, text).1;
        }
        sets += run(&mut inspector, &mut world, enter()).1;
        // Then a frame with a tick, if that one had none.
        run(&mut inspector, &mut world, Vec::new());
        assert!(sets > 1 && weight(&world) == 0.75, "{sets} sets");
        assert_eq!(inspector.log.operations().len(), 1, "one edit");
        run(&mut inspector, &mut world, shortcut(egui::Key::Z, false));
        run(&mut inspector, &mut world, Vec::new());
        assert_eq!(weight(&world).to_bits(), 2.5f64.to_bits());
    }

    /// What the window of its own that [`in_window`] gives the panels gets and gives: its pass's
    /// time and events, and its texts.
    #[derive(Default)]
    struct Window {
        time: f64,
        events: Vec<egui::Event>,
        texts: Vec<Text>,
    }

    /// Runs `ctx`'s windows of their own as the engine runs a torn-off panel's: each in a pass of
    /// its own inside the main window's, with its own input, which the returned `Window` gives.
    fn in_window(ctx: &egui::Context) -> Rc<RefCell<Window>> {
        ctx.set_embed_viewports(false);
        let window = Rc::new(RefCell::new(Window::default()));
        let shared = window.clone();
        // The callback is the test thread's.
        egui::Context::set_immediate_viewport_renderer(move |ctx, viewport| {
            let egui::ImmediateViewport {
                ids,
                mut viewport_ui_cb,
                ..
            } = viewport;
            let input = {
                let mut window = shared.borrow_mut();
                let screen =
                    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 4000.0));
                let mut viewports = egui::ViewportIdMap::default();
                viewports.insert(ids.this, egui::ViewportInfo::default());
                egui::RawInput {
                    viewport_id: ids.this,
                    viewports,
                    screen_rect: Some(screen),
                    time: Some(window.time),
                    events: mem::take(&mut window.events),
                    ..egui::RawInput::default()
                }
            };
            let mut output = ctx.run_ui(input, |ui| viewport_ui_cb(ui));
            output.textures_delta.clear();
            let mut texts = Vec::new();
            for clipped in &output.shapes {
                find_texts(&clipped.shape, &mut texts);
            }
            shared.borrow_mut().texts = texts;
        });
        window
    }

    /// A frame of the main window, in whose pass the two panels draw in `window`, a window of
    /// their own, given `events`; returns that window's texts, and how many edits it made.
    fn window_frame(
        inspector: &mut Inspector,
        world: &mut World,
        ctx: &egui::Context,
        window: &RefCell<Window>,
        time: f64,
        events: Vec<egui::Event>,
    ) -> (Vec<Text>, usize) {
        *window.borrow_mut() = Window {
            time,
            events,
            texts: Vec::new(),
        };
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(time),
            ..egui::RawInput::default()
        };
        let mut made = 0;
        let id = egui::ViewportId::from_hash_of("the panels' window");
        let mut output = ctx.run_ui(input, |root| {
            let mut pass = Pass::default();
            let builder = egui::ViewportBuilder::default();
            root.ctx().show_viewport_immediate(id, builder, |ui, _| {
                panels(inspector, world, ui, &mut pass);
            });
            pass.cues.read(root.ctx());
            made = pass.sets.len();
            make(inspector, world, pass);
        });
        output.textures_delta.clear();
        let texts = mem::take(&mut window.borrow_mut().texts);
        (texts, made)
    }

    #[test]
    fn a_torn_off_inspector_keeps_its_drags_and_keys() {
        let mut world = World::default();
        let entity = world.spawn(everything());
        let ctx = crate::egui_context();
        let window = in_window(&ctx);
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, entity));
        let mut time = 0.0;
        let mut run = |inspector: &mut Inspector, world: &mut World, events| {
            time += 1.0;
            window_frame(inspector, world, &ctx, &window, time, events)
        };
        let mut texts = Vec::new();
        for _ in 0..3 {
            texts = run(&mut inspector, &mut world, Vec::new()).0;
        }
        let at = beside(&texts, find(&texts, "Everything", "weight"));
        let weight = |world: &World| world.get::<Everything>(entity).expect("everything").weight;
        let mut sets = 0;
        for events in slide(at) {
            sets += run(&mut inspector, &mut world, events).1;
        }
        let dragged = weight(&world);
        assert!(sets > 1 && dragged != 2.5, "{sets} sets, to {dragged}");
        assert_eq!(inspector.log.operations().len(), 1, "one edit");
        // Pressed in that window, the keys step through the edits.
        run(&mut inspector, &mut world, shortcut(egui::Key::Z, false));
        assert_eq!(weight(&world).to_bits(), 2.5f64.to_bits());
        run(&mut inspector, &mut world, shortcut(egui::Key::Z, true));
        assert_eq!(weight(&world).to_bits(), dragged.to_bits());
    }

    #[test]
    fn each_click_is_an_edit_and_a_text_field_keeps_its_keys() {
        let mut world = World::default();
        let entity = world.spawn(everything());
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, entity));
        let mut time = 0.0;
        let mut run = |inspector: &mut Inspector, world: &mut World, events| {
            time += 1.0;
            frame(inspector, world, &ctx, time, events).0
        };
        let mut texts = Vec::new();
        for _ in 0..3 {
            texts = run(&mut inspector, &mut world, Vec::new());
        }
        let on = |world: &World| world.get::<Everything>(entity).expect("everything").on;
        // Its checkbox, which has no text, in the column its neighbor's number starts.
        let checkbox = egui::pos2(
            find(&texts, "count", "42").min.x + 4.0,
            find(&texts, "Everything", "on").center().y,
        );
        // Clicked in frames one after the other, each click is an edit of its own.
        run(&mut inspector, &mut world, click(Primary, checkbox));
        run(&mut inspector, &mut world, click(Primary, checkbox));
        assert!(on(&world));
        assert_eq!(inspector.log.operations().len(), 2);
        // While the Find field has the keys, they undo its own text, not an edit.
        let field = beside(&texts, find(&texts, "Find", "Find"));
        run(&mut inspector, &mut world, click(Primary, field));
        run(&mut inspector, &mut world, shortcut(egui::Key::Z, false));
        assert_eq!(inspector.log.operations().len(), 2);
        let away = click(Primary, egui::pos2(1500.0, 3900.0));
        run(&mut inspector, &mut world, away);
        run(&mut inspector, &mut world, shortcut(egui::Key::Z, false));
        assert_eq!(inspector.log.operations().len(), 1);
        assert!(!on(&world));
        // Two drags while the Find field keeps the keys are two edits, a press beginning each.
        run(&mut inspector, &mut world, click(Primary, field));
        let weight = beside(&texts, find(&texts, "Everything", "weight"));
        for events in [slide(weight), slide(weight)].concat() {
            run(&mut inspector, &mut world, events);
        }
        assert_eq!(inspector.log.operations().len(), 3);
    }

    /// A frame of the Edit and Entity menus' items alone, as the open menus show them, given
    /// `events`, and then the tick that makes what the one clicked chose; returns its texts.
    fn menu(
        inspector: &mut Inspector,
        world: &mut World,
        ctx: &egui::Context,
        time: f64,
        events: Vec<egui::Event>,
    ) -> Vec<Text> {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 400.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(time),
            events,
            ..egui::RawInput::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            let mut pass = Pass {
                chosen: inspector.edit_menu(ui),
                ..Pass::default()
            };
            inspector.entity_menu(world, ui, &mut pass.operations);
            pass.cues.read(ui.ctx());
            make(inspector, world, pass);
        });
        output.textures_delta.clear();
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            find_texts(&clipped.shape, &mut texts);
        }
        texts
    }

    #[test]
    fn the_edit_menu_names_its_steps_and_a_refused_one_says_why() {
        let mut world = World::default();
        let entity = world.spawn(everything());
        let picked = id(&world, entity);
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(picked);
        let count = |world: &World| world.get::<Everything>(entity).expect("everything").count;
        let edit = Edit {
            component: "Everything".to_owned(),
            path: "count".to_owned(),
            value: Value::UInt(99),
        };
        inspector.apply(&mut world, inspector.picked, edit);
        let mut time = 0.0;
        let mut run = |inspector: &mut Inspector, world: &mut World, events| {
            time += 1.0;
            menu(inspector, world, &ctx, time, events)
        };
        let undo = format!("Undo set `Everything.count` of {picked}");
        let redo = format!("Redo set `Everything.count` of {picked}");
        let texts = run(&mut inspector, &mut world, Vec::new());
        for item in [undo.as_str(), "Redo"] {
            assert!(
                texts.iter().any(|(text, _)| text == item),
                "{item}: {texts:?}"
            );
        }
        run(
            &mut inspector,
            &mut world,
            click(Primary, find(&texts, &undo, &undo).center()),
        );
        assert_eq!(count(&world), 42);
        let texts = run(&mut inspector, &mut world, Vec::new());
        for item in ["Undo", redo.as_str()] {
            assert!(
                texts.iter().any(|(text, _)| text == item),
                "{item}: {texts:?}"
            );
        }
        run(
            &mut inspector,
            &mut world,
            click(Primary, find(&texts, &redo, &redo).center()),
        );
        assert_eq!(count(&world), 99);
        // With its entity gone, the edit can't be undone, which the Inspector panel says.
        world.despawn(entity).expect("alive");
        run(&mut inspector, &mut world, shortcut(egui::Key::Z, false));
        assert_eq!(inspector.log.undoable(), None);
        let mut texts = Vec::new();
        for _ in 0..3 {
            time += 1.0;
            texts = frame(&mut inspector, &mut world, &ctx, time, Vec::new()).0;
        }
        let says = format!("can't undo set `Everything.count` of {picked}: ");
        let said = texts.iter().any(|(text, _)| text.starts_with(&says));
        assert!(said, "{texts:?}");
    }

    /// Every row of the tree, the naive way: each root in turn, each followed, if its row is open,
    /// by the rows of each of its children in turn.
    fn flatten(world: &World, expanded: &HashSet<EntityId>) -> Vec<(Entity, usize)> {
        fn rows(
            world: &World,
            expanded: &HashSet<EntityId>,
            (entity, depth): (Entity, usize),
            out: &mut Vec<(Entity, usize)>,
        ) {
            out.push((entity, depth));
            if expanded.contains(&id(world, entity)) {
                for child in world.children(entity).expect("alive") {
                    rows(world, expanded, (child, depth + 1), out);
                }
            }
        }
        let mut out = Vec::new();
        for root in world.roots() {
            rows(world, expanded, (root, 0), &mut out);
        }
        out
    }

    /// `n` children made for `parent`, each spawned a root and then given its parent.
    fn children_of(world: &mut World, parent: Entity, n: usize) -> Vec<Entity> {
        let made = (0..n).map(|_| {
            let child = world.spawn(Transform::IDENTITY);
            world.set_parent(child, Some(parent)).expect("no cycle");
            child
        });
        made.collect()
    }

    /// The tree finds a page of its rows as a listing of every row does, wherever the page starts:
    /// over 50,000 roots in several tables, open ones followed by their rows, with open children
    /// among them; an open root without children; an open child under a closed parent, which adds
    /// no rows; and an open entity despawned.
    #[test]
    fn the_tree_finds_a_page_as_a_listing_of_every_row_does() {
        let mut world = World::default();
        let plain: Vec<Entity> = (0..20_000)
            .map(|_| world.spawn(Transform::IDENTITY))
            .collect();
        // In a table of children, between the roots' tables.
        let top = children_of(&mut world, plain[5], 4);
        let middle = children_of(&mut world, top[0], 3);
        let bottom = children_of(&mut world, middle[1], 2);
        let named: Vec<Entity> = (0..20_000)
            .map(|i| world.spawn((called(&i.to_string()), Transform::IDENTITY)))
            .collect();
        let sprites: Vec<Entity> = (0..10_000)
            .map(|_| world.spawn(Sprite::default()))
            .collect();
        let last = *sprites.last().expect("sprites");
        children_of(&mut world, named[100], 3);
        let closed = children_of(&mut world, named[200], 2);
        children_of(&mut world, closed[0], 2);
        children_of(&mut world, last, 2);
        let gone = world.spawn(Transform::IDENTITY);
        let gone_id = id(&world, gone);
        world.despawn(gone).expect("alive");
        let open = [
            plain[5], top[0], middle[1], named[7], named[100], closed[0], last,
        ];
        let mut expanded: HashSet<EntityId> =
            open.iter().map(|&entity| id(&world, entity)).collect();
        expanded.insert(gone_id);
        let tree = Tree::new(&world, &expanded);
        let flat = flatten(&world, &expanded);
        assert!(flat.len() > 50_000, "{}", flat.len());
        assert!(flat.contains(&(bottom[1], 3)));
        assert_eq!(tree.spans.len(), 4, "the roots open");
        assert_eq!(tree.len, flat.len());
        assert_eq!(tree.page(&world, 0..flat.len()), flat);
        // Pages from every 97th row, and from each row about each open root's.
        let mut starts: Vec<usize> = (0..flat.len()).step_by(97).collect();
        let mut before = 0;
        for &(at, n) in &tree.spans {
            let start = at + before;
            starts.extend(start.saturating_sub(2)..(start + n + 3).min(flat.len()));
            before += n;
        }
        for start in starts {
            let end = (start + 40).min(flat.len());
            assert_eq!(tree.page(&world, start..end), flat[start..end], "{start}");
        }
    }

    /// A frame scrolled thousands of rows down draws the rows in view alone, as the tree lists
    /// them, an open root's rows among them.
    #[test]
    fn a_frame_scrolled_over_thousands_of_roots_draws_the_rows_in_view() {
        let mut world = World::default();
        let roots: Vec<Entity> = (0..5000)
            .map(|i| world.spawn(called(&format!("root {i}"))))
            .collect();
        for i in 0..3 {
            let child = world.spawn(called(&format!("child {i}")));
            world.set_parent(child, Some(roots[130])).expect("no cycle");
        }
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.expanded.insert(id(&world, roots[130]));
        let flat = flatten(&world, &inspector.expanded);
        let rows: HashMap<String, usize> = flat
            .iter()
            .enumerate()
            .map(|(row, &(entity, _))| (row_label(&world, entity, id(&world, entity)), row))
            .collect();
        let mut time = 0.0;
        let mut run = |inspector: &mut Inspector, world: &mut World, events| {
            let mut texts = Vec::new();
            for events in [events, Vec::new(), Vec::new()] {
                time += 1.0;
                texts = frame(inspector, world, &ctx, time, events).0;
            }
            texts
        };
        run(&mut inspector, &mut world, Vec::new());
        // As a trackpad scrolls, which egui doesn't smooth.
        let wheel = |phase, y| egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, y),
            phase,
            source: egui::MouseWheelSource::Unknown,
            modifiers: Modifiers::NONE,
        };
        let scrolled = vec![
            egui::Event::PointerMoved(egui::pos2(200.0, 600.0)),
            wheel(egui::TouchPhase::Start, 0.0),
            wheel(egui::TouchPhase::Move, -2500.0),
        ];
        let texts = run(&mut inspector, &mut world, scrolled);
        let shown: Vec<usize> = texts
            .iter()
            .filter_map(|(text, _)| rows.get(text).copied())
            .collect();
        let first = *shown.first().expect("rows shown");
        assert!(first > 100 && shown.len() < 300, "{shown:?}");
        assert_eq!(shown, (first..first + shown.len()).collect::<Vec<_>>());
        assert!((131..134).all(|child| shown.contains(&child)), "{shown:?}");
    }

    /// Where the arrows `shape` draws are: its triangles.
    fn find_arrows(shape: &egui::Shape, arrows: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    find_arrows(shape, arrows);
                }
            }
            egui::Shape::Path(path) if path.closed && path.points.len() == 3 => {
                arrows.push(path.visual_bounding_rect());
            }
            _ => {}
        }
    }

    /// Only the rows of entities with children have arrows, which show and hide their children's
    /// rows, under theirs and further in, rather than pick them; a list of the entities with a
    /// component has neither arrows nor indents.
    #[test]
    fn rows_with_children_alone_have_arrows() {
        let mut world = World::default();
        let [parent, child, lone] =
            ["parent", "child", "lone"].map(|name| world.spawn(called(name)));
        world.set_parent(child, Some(parent)).expect("no cycle");
        let label = |world: &World, entity| row_label(world, entity, id(world, entity));
        let [parent_row, child_row, lone_row] = [parent, child, lone].map(|e| label(&world, e));
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        let mut time = 0.0;
        // A frame given `events`, then two that settle what it did; the last one's texts and
        // arrows.
        let mut run = |inspector: &mut Inspector, world: &mut World, events| {
            let (mut texts, mut arrows) = (Vec::new(), Vec::new());
            for events in [events, Vec::new(), Vec::new()] {
                time += 1.0;
                let (shapes, _) = frame_shapes(inspector, world, &ctx, time, events, true);
                (texts, arrows) = (Vec::new(), Vec::new());
                for clipped in &shapes {
                    find_texts(&clipped.shape, &mut texts);
                    find_arrows(&clipped.shape, &mut arrows);
                }
            }
            (texts, arrows)
        };
        // The arrow on the row labeled `row`, left of its label, if it has one.
        let arrow = |(texts, arrows): &(Vec<Text>, Vec<egui::Rect>), row: &str| {
            let label = find(texts, row, row);
            let on_row = |arrow: &&egui::Rect| {
                (arrow.center().y - label.center().y).abs() < 4.0 && arrow.max.x <= label.min.x
            };
            arrows.iter().find(on_row).copied()
        };
        let shows = |(texts, _): &(Vec<Text>, Vec<egui::Rect>), row: &str| {
            texts.iter().any(|(text, _)| text == row)
        };
        let drawn = run(&mut inspector, &mut world, Vec::new());
        assert!(arrow(&drawn, &lone_row).is_none());
        assert!(!shows(&drawn, &child_row));
        let open = arrow(&drawn, &parent_row)
            .expect("the parent's arrow")
            .center();
        let drawn = run(&mut inspector, &mut world, click(Primary, open));
        assert_eq!(inspector.picked, None);
        let (above, under) = (
            find(&drawn.0, &parent_row, &parent_row),
            find(&drawn.0, &child_row, &child_row),
        );
        assert!(under.min.x > above.min.x && under.min.y > above.max.y);
        assert!(arrow(&drawn, &child_row).is_none());
        let drawn = run(&mut inspector, &mut world, click(Primary, open));
        assert!(!shows(&drawn, &child_row));
        inspector.with = Some("Name".to_owned());
        let drawn = run(&mut inspector, &mut world, Vec::new());
        for row in [&parent_row, &child_row, &lone_row] {
            assert!(arrow(&drawn, row).is_none(), "{row}");
        }
        // Within a glyph's bearing.
        let lefts = [&parent_row, &child_row].map(|row| find(&drawn.0, row, row).min.x);
        assert!((lefts[0] - lefts[1]).abs() < 4.0, "{lefts:?}");
    }

    /// Where `name` shows in a menu, over everything else.
    fn in_menu(ctx: &egui::Context, texts: &[Text], name: &str) -> egui::Pos2 {
        let over = |(text, rect): &&Text| {
            let layer = ctx.layer_id_at(rect.center());
            text == name && layer.is_some_and(|layer| layer.order == egui::Order::Foreground)
        };
        let found = texts.iter().find(over);
        found
            .unwrap_or_else(|| panic!("a menu shows {name}: {texts:?}"))
            .1
            .center()
    }

    /// A world that has registered the components of an empty entity, as a bare one hasn't.
    fn creating() -> World {
        let mut world = World::default();
        world.register::<crate::Name>().expect("registered");
        world.register::<Transform>().expect("registered");
        world
    }

    /// The Entities panel's context menus, which a secondary click opens and egui sizes in a
    /// frame of their own: below the rows, one makes a root; and on a row, one makes a child of
    /// its entity, or deletes it with its descendants. Each is made at the next tick, which picks
    /// what a create made and shows its row; and each is undone and redone.
    #[test]
    fn context_menus_create_and_delete_entities() {
        let mut world = creating();
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        let clock = Cell::new(0.0);
        let now = || {
            clock.set(clock.get() + 1.0);
            clock.get()
        };
        // A frame given `events`, then two that settle what it did; the last one's texts.
        let run = |inspector: &mut Inspector, world: &mut World, events| {
            let mut texts = Vec::new();
            for events in [events, Vec::new(), Vec::new()] {
                texts = frame(inspector, world, &ctx, now(), events).0;
            }
            texts
        };
        let alive = |world: &World| world.entities(None).expect("every entity").len();
        let row = |world: &World, entity| row_label(world, entity, id(world, entity));
        // Opens the menu of what's at `at`, and clicks its item `item`.
        let choose = |inspector: &mut Inspector, world: &mut World, at: egui::Pos2, item: &str| {
            let texts = run(inspector, world, click(Secondary, at));
            let item = in_menu(&ctx, &texts, item);
            run(inspector, world, click(Primary, item))
        };
        run(&mut inspector, &mut world, Vec::new());
        let below = egui::pos2(200.0, 3000.0);
        let texts = run(&mut inspector, &mut world, click(Secondary, below));
        let item = in_menu(&ctx, &texts, "Create Empty Entity");
        let clicked = click(Primary, item);
        frame_ticking(&mut inspector, &mut world, &ctx, now(), clicked, false);
        assert_eq!(alive(&world), 0, "made at the next tick");
        Layer::tick(&mut inspector, &mut world, Tick { number: 0 });
        let root = world.roots().next().expect("a root");
        assert_eq!(inspector.picked, Some(id(&world, root)));
        assert_eq!(
            world.get::<crate::Name>(root),
            Some(&called("Empty Entity"))
        );
        assert_eq!(world.get::<Transform>(root), Some(&Transform::IDENTITY));
        // A child, and a child of that, each shown under its parent's row as it's picked.
        let mut parent = root;
        let mut made = Vec::new();
        for _ in 0..2 {
            let texts = run(&mut inspector, &mut world, Vec::new());
            let at = find(&texts, &row(&world, parent), &row(&world, parent)).center();
            let texts = choose(&mut inspector, &mut world, at, "Create Empty Child");
            let child = world
                .children(parent)
                .expect("alive")
                .next()
                .expect("a child");
            assert_eq!(inspector.picked, Some(id(&world, child)));
            let (above, under) = (row(&world, parent), row(&world, child));
            assert!(find(&texts, &above, &under).min.y > find(&texts, &above, &above).max.y);
            made.push(id(&world, child));
            parent = child;
        }
        // The whole subtree deleted from the root's row, the pick staying on what's gone.
        let texts = run(&mut inspector, &mut world, Vec::new());
        let at = find(&texts, &row(&world, root), &row(&world, root)).center();
        let texts = choose(&mut inspector, &mut world, at, "Delete Entity");
        assert_eq!(alive(&world), 0);
        assert_eq!(inspector.picked, Some(made[1]));
        let despawned = format!("Entity {} was despawned", made[1]);
        assert!(
            texts.iter().any(|(text, _)| *text == despawned),
            "{texts:?}"
        );
        // Undone, it comes back, under its IDs; then each create is undone, and each redone.
        let (undo, redo) = (shortcut(egui::Key::Z, false), shortcut(egui::Key::Z, true));
        run(&mut inspector, &mut world, undo.clone());
        assert_eq!(alive(&world), 3);
        assert!(made.iter().all(|&made| world.entity(made).is_some()));
        for (keys, expected) in [
            (&undo, 2),
            (&undo, 1),
            (&undo, 0),
            (&redo, 1),
            (&redo, 2),
            (&redo, 3),
            (&redo, 0),
        ] {
            run(&mut inspector, &mut world, keys.clone());
            assert_eq!(alive(&world), expected);
        }
    }

    /// A context menu left open acts on the entity it opened on, though its row moves as tables
    /// swap rows, and another entity's row comes to its place.
    #[test]
    fn a_context_menu_acts_on_its_entity_as_its_row_moves() {
        let mut world = World::default();
        let [a, b, c] =
            ["a", "b", "c"].map(|name| world.spawn((called(name), Transform::IDENTITY)));
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        let mut time = 0.0;
        let mut run = |inspector: &mut Inspector, world: &mut World, events| {
            let mut texts = Vec::new();
            for events in [events, Vec::new(), Vec::new()] {
                time += 1.0;
                texts = frame(inspector, world, &ctx, time, events).0;
            }
            texts
        };
        let row = |world: &World, entity| row_label(world, entity, id(world, entity));
        let texts = run(&mut inspector, &mut world, Vec::new());
        let first = find(&texts, &row(&world, a), &row(&world, a));
        run(&mut inspector, &mut world, click(Secondary, first.center()));
        // To the end of another table, the last of its own taking its row.
        world.insert(a, Sprite::default()).expect("alive");
        let texts = run(&mut inspector, &mut world, Vec::new());
        let moved_in = find(&texts, &row(&world, c), &row(&world, c));
        assert!(
            (moved_in.center().y - first.center().y).abs() < 4.0,
            "{texts:?}"
        );
        let item = in_menu(&ctx, &texts, "Delete Entity");
        run(&mut inspector, &mut world, click(Primary, item));
        assert!(!world.contains(a));
        assert!(world.contains(b) && world.contains(c));
    }

    /// The Entity menu makes a root; and, while the entity picked is alive, makes a child of it
    /// and deletes it; and unpicks it, despawned too.
    #[test]
    fn the_entity_menu_creates_deletes_and_unpicks() {
        let mut world = creating();
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        let mut time = 0.0;
        // A frame given `events`, then one that settles what it did, since egui finds what's
        // clicked among the last frame's widgets; the last one's texts.
        let mut run = |inspector: &mut Inspector, world: &mut World, events| {
            let mut texts = Vec::new();
            for events in [events, Vec::new()] {
                time += 1.0;
                texts = menu(inspector, world, &ctx, time, events);
            }
            texts
        };
        let texts = run(&mut inspector, &mut world, Vec::new());
        let item = |name: &str| click(Primary, find(&texts, name, name).center());
        // Without a pick, only Create Empty acts.
        for name in ["Create Empty Child", "Delete", "Unpick"] {
            run(&mut inspector, &mut world, item(name));
        }
        assert_eq!(world.entities(None).expect("every entity").len(), 0);
        run(&mut inspector, &mut world, item("Create Empty"));
        let root = world.roots().next().expect("a root");
        assert_eq!(inspector.picked, Some(id(&world, root)));
        run(&mut inspector, &mut world, item("Create Empty Child"));
        let child = world
            .children(root)
            .expect("alive")
            .next()
            .expect("a child");
        let child_id = id(&world, child);
        assert_eq!(inspector.picked, Some(child_id));
        assert!(inspector.expanded.contains(&id(&world, root)));
        run(&mut inspector, &mut world, item("Delete"));
        assert!(!world.contains(child));
        // Still picked, for an undo to bring it back; but its pick despawned unpicks too.
        assert_eq!(inspector.picked, Some(child_id));
        run(&mut inspector, &mut world, item("Unpick"));
        assert_eq!(inspector.picked, None);
    }

    /// A click below the rows unpicks, forgetting the refusals of the entity picked.
    #[test]
    fn a_click_below_the_rows_unpicks() {
        let mut world = World::default();
        let entity = world.spawn(everything());
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, entity));
        let refused = Edit {
            component: "Everything".to_owned(),
            path: "count".to_owned(),
            value: Value::UInt(200),
        };
        inspector.apply(&mut world, inspector.picked, refused);
        let mut texts = Vec::new();
        let below = click(Primary, egui::pos2(200.0, 3000.0));
        for (time, events) in [Vec::new(), below, Vec::new(), Vec::new()]
            .into_iter()
            .enumerate()
        {
            texts = frame(&mut inspector, &mut world, &ctx, time as f64, events).0;
        }
        assert_eq!(inspector.picked, None);
        assert!(inspector.refusals.is_empty());
        let nothing = "Nothing picked: pick an entity in the Entities panel";
        assert!(texts.iter().any(|(text, _)| text == nothing), "{texts:?}");
    }

    /// An entity found by its ID shows its row: the rows above it open.
    #[test]
    fn finding_an_entity_opens_the_rows_above_it() {
        let mut world = World::default();
        let [top, middle, bottom] =
            ["top", "middle", "bottom"].map(|name| world.spawn(called(name)));
        world.set_parent(middle, Some(top)).expect("no cycle");
        world.set_parent(bottom, Some(middle)).expect("no cycle");
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        let mut time = 0.0;
        let mut run = |inspector: &mut Inspector, world: &mut World, events| {
            let mut texts = Vec::new();
            for events in [events, Vec::new(), Vec::new()] {
                time += 1.0;
                texts = frame(inspector, world, &ctx, time, events).0;
            }
            texts
        };
        let found = row_label(&world, bottom, id(&world, bottom));
        let texts = run(&mut inspector, &mut world, Vec::new());
        assert!(!texts.iter().any(|(text, _)| *text == found));
        let field = beside(&texts, find(&texts, "Find", "Find"));
        run(&mut inspector, &mut world, click(Primary, field));
        let typed = vec![egui::Event::Text(id(&world, bottom).to_string())];
        run(&mut inspector, &mut world, typed);
        let texts = run(&mut inspector, &mut world, enter());
        assert_eq!(inspector.picked, Some(id(&world, bottom)));
        let open: HashSet<EntityId> = [top, middle].map(|entity| id(&world, entity)).into();
        assert_eq!(inspector.expanded, open);
        assert!(texts.iter().any(|(text, _)| *text == found), "{texts:?}");
    }

    /// What a component that follows an entity holds: its handle, of which there's no default.
    #[derive(Clone, Component)]
    struct Follower(Entity);

    /// Opaque as a whole, as a mesh's draw is: no value makes one.
    #[derive(Clone, Component)]
    #[reflect(opaque)]
    struct Sealed;

    /// The Add Component menu lists the types the picked entity can be given, in the order they
    /// registered: not those it has, nor a parent, nor those opaque as a whole. One picked is added
    /// at the next tick, of its type's default, which an undo takes away; one without a default
    /// shows disabled, saying why on hover, and adds nothing.
    #[test]
    fn add_component_adds_a_types_default() {
        let mut world = creating();
        world.register::<Camera>().expect("registered");
        let entity = world.spawn(Transform::IDENTITY);
        // Registered as they're spawned.
        world.spawn(Follower(entity));
        world.spawn(Sealed);
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, entity));
        let mut time = 0.0;
        // A frame given `events`, then two that settle what they did, as a popup draws unseen in
        // its first; the last one's texts.
        let mut run = |inspector: &mut Inspector, world: &mut World, events| {
            let mut texts = Vec::new();
            for events in [events, Vec::new(), Vec::new()] {
                time += 1.0;
                texts = frame(inspector, world, &ctx, time, events).0;
            }
            texts
        };
        let texts = run(&mut inspector, &mut world, Vec::new());
        let open = click(
            Primary,
            find(&texts, "Add Component", "Add Component").center(),
        );
        let texts = run(&mut inspector, &mut world, open.clone());
        let over = |(_, rect): &&Text| {
            let layer = ctx.layer_id_at(rect.center());
            layer.is_some_and(|layer| layer.order == egui::Order::Foreground)
        };
        let items: Vec<&str> = texts
            .iter()
            .filter(over)
            .map(|(text, _)| text.as_str())
            .collect();
        assert_eq!(items, ["Name", "Camera", "Follower"]);
        let follower = in_menu(&ctx, &texts, "Follower");
        let hovered = run(
            &mut inspector,
            &mut world,
            vec![egui::Event::PointerMoved(follower)],
        );
        let why = "can't make a default `Follower.0`: an entity has no default";
        let said = hovered.iter().any(|(text, _)| text.starts_with(why));
        assert!(said, "{hovered:?}");
        run(&mut inspector, &mut world, click(Primary, follower));
        assert!(world.get::<Follower>(entity).is_none());
        let texts = run(&mut inspector, &mut world, open);
        let camera = click(Primary, in_menu(&ctx, &texts, "Camera"));
        run(&mut inspector, &mut world, camera);
        let made = Camera {
            projection: Projection::Orthographic {
                height: 10.0,
                near: -1.0,
                far: 1.0,
            },
            primary: false,
        };
        assert_eq!(world.get::<Camera>(entity), Some(&made));
        assert_eq!(inspector.log.operations().len(), 1);
        run(&mut inspector, &mut world, shortcut(egui::Key::Z, false));
        assert_eq!(world.get::<Camera>(entity), None);
    }

    /// The menu beside each component's header removes it, at the next tick, and until then it
    /// isn't drawn; an undo gives it back. A parent, which the world gives, has no menu.
    #[test]
    fn a_components_menu_removes_it() {
        let mut world = World::default();
        let parent = world.spawn(Transform::IDENTITY);
        let color = Vec4::new(0.5, 0.25, 1.0, 1.0);
        let sprite = Sprite {
            color,
            ..Sprite::default()
        };
        let entity = world.spawn((Transform::IDENTITY, sprite));
        world.set_parent(entity, Some(parent)).expect("no cycle");
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, entity));
        let mut time = 0.0;
        // A frame given `events`, then two that settle what they did, each with a tick if `tick`;
        // the last one's texts.
        let mut run = |inspector: &mut Inspector, world: &mut World, events, tick| {
            let mut texts = Vec::new();
            for events in [events, Vec::new(), Vec::new()] {
                time += 1.0;
                texts = frame_ticking(inspector, world, &ctx, time, events, tick).0;
            }
            texts
        };
        let texts = run(&mut inspector, &mut world, Vec::new(), true);
        // Its transform's and its sprite's, in a glyph the default fonts have.
        let menus = texts.iter().filter(|(text, _)| text == MORE).count();
        assert_eq!(menus, 2, "{texts:?}");
        let font = egui::FontId::default();
        assert!(ctx.fonts_mut(|fonts| fonts.has_glyphs(&font, MORE)));
        let menu = click(Primary, find(&texts, "Sprite", MORE).center());
        let texts = run(&mut inspector, &mut world, menu, true);
        let remove = click(Primary, in_menu(&ctx, &texts, "Remove Component"));
        let texts = run(&mut inspector, &mut world, remove, false);
        assert!(!texts.iter().any(|(text, _)| text == "Sprite"), "{texts:?}");
        assert!(
            world.get::<Sprite>(entity).is_some(),
            "removed at the next tick"
        );
        Layer::tick(&mut inspector, &mut world, Tick { number: 0 });
        assert!(world.get::<Sprite>(entity).is_none());
        run(
            &mut inspector,
            &mut world,
            shortcut(egui::Key::Z, false),
            true,
        );
        let back = world.get::<Sprite>(entity).map(|sprite| sprite.color);
        assert_eq!(back, Some(color));
    }

    /// An enum with fields switches its variant from a choice of them: to one it had while the
    /// entity was drawn, with the fields it had then; to one it hadn't, with its fields' defaults;
    /// and on another entity, to the defaults again.
    #[test]
    fn variants_switch_to_the_fields_they_had_or_their_defaults() {
        let mut world = World::default();
        let camera = |height| Camera {
            projection: Projection::Orthographic {
                height,
                near: -2.0,
                far: 2.0,
            },
            primary: false,
        };
        let [first, second] = [4.0, 6.0].map(|height| world.spawn(camera(height)));
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, first));
        let clock = Cell::new(0.0);
        // A frame given `events`, then two that settle what they did, as a popup draws unseen in
        // its first, each with a tick if `tick`; the last one's texts.
        let run = |inspector: &mut Inspector, world: &mut World, events, tick| {
            let mut texts = Vec::new();
            for events in [events, Vec::new(), Vec::new()] {
                clock.set(clock.get() + 1.0);
                texts = frame_ticking(inspector, world, &ctx, clock.get(), events, tick).0;
            }
            texts
        };
        // Picks `to` from the projection's choice, which shows `from`, the variant it is.
        let switch = |inspector: &mut Inspector, world: &mut World, from: &str, to: &str| {
            let texts = run(inspector, world, Vec::new(), true);
            let choice = click(Primary, find(&texts, "projection", from).center());
            let texts = run(inspector, world, choice, true);
            run(
                inspector,
                world,
                click(Primary, in_menu(&ctx, &texts, to)),
                true,
            );
        };
        let projection =
            |world: &World, entity| world.get::<Camera>(entity).expect("a camera").projection;
        let orthographic = camera(4.0).projection;
        let perspective = |fov| Projection::Perspective { fov, near: 0.01 };
        let defaults = perspective(std::f32::consts::FRAC_PI_4);
        switch(&mut inspector, &mut world, "Orthographic", "Perspective");
        assert_eq!(projection(&world, first), defaults);
        switch(&mut inspector, &mut world, "Perspective", "Orthographic");
        assert_eq!(projection(&world, first), orthographic);
        switch(&mut inspector, &mut world, "Orthographic", "Perspective");
        assert_eq!(projection(&world, first), defaults);
        // Its field of view typed in; then switched away from, and back to.
        let texts = run(&mut inspector, &mut world, Vec::new(), true);
        let fov = beside(&texts, find(&texts, "projection", "fov"));
        run(&mut inspector, &mut world, click(Primary, fov), true);
        let typed = vec![egui::Event::Text("60".to_owned())];
        run(&mut inspector, &mut world, typed, true);
        run(&mut inspector, &mut world, enter(), true);
        // As a field in degrees takes what's typed.
        let sixty = perspective((60.0 / (180.0 / PI)) as f32);
        assert_eq!(projection(&world, first), sixty);
        switch(&mut inspector, &mut world, "Perspective", "Orthographic");
        assert_eq!(projection(&world, first), orthographic);
        switch(&mut inspector, &mut world, "Orthographic", "Perspective");
        assert_eq!(projection(&world, first), sixty);
        inspector.pick(id(&world, second));
        switch(&mut inspector, &mut world, "Orthographic", "Perspective");
        assert_eq!(projection(&world, second), defaults);
    }

    /// A variant that holds what's opaque, which no value makes, shows disabled in its enum's
    /// choice, saying why on hover, and isn't switched to.
    #[test]
    fn a_variant_that_holds_whats_opaque_shows_disabled() {
        let mut world = World::default();
        let entity = world.spawn(everything());
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, entity));
        let mut time = 0.0;
        let mut run = |inspector: &mut Inspector, world: &mut World, events| {
            let mut texts = Vec::new();
            for events in [events, Vec::new(), Vec::new()] {
                time += 1.0;
                texts = frame(inspector, world, &ctx, time, events).0;
            }
            texts
        };
        let texts = run(&mut inspector, &mut world, Vec::new());
        // Its form's choice, which shows the variant it is.
        let choice = click(Primary, find(&texts, "form", "Point").center());
        let texts = run(&mut inspector, &mut world, choice);
        let held = in_menu(&ctx, &texts, "Held");
        let hovered = vec![egui::Event::PointerMoved(held)];
        let texts = run(&mut inspector, &mut world, hovered);
        let why = "at `0`: it's opaque: reflection keeps one, but doesn't make one";
        assert!(texts.iter().any(|(text, _)| text == why), "{texts:?}");
        run(&mut inspector, &mut world, click(Primary, held));
        let form = &world.get::<Everything>(entity).expect("everything").form;
        assert_eq!(form, &Form::Point);
        assert_eq!(inspector.log.operations().len(), 0);
    }

    /// Each axis's button sets its component alone to the default's, a click an edit: a
    /// translation's x to 0, its type's; a scale's y to 1, the transform's; the x of a sprite's
    /// rect's bottom right to 1, the sprite's; and a rotation's angle about z to the default
    /// rotation's, keeping the others.
    #[test]
    fn axis_buttons_set_their_component_to_its_default() {
        let mut world = World::default();
        let transform = Transform {
            translation: Vec3::new(1.0, 2.0, 3.0),
            rotation: Quat::from_euler(EulerRot::ZYX, 0.3, 0.2, 0.1),
            scale: Vec3::new(2.0, 3.0, 4.0),
        };
        let sprite = Sprite {
            rect: [Vec2::new(0.1, 0.2), Vec2::new(0.5, 0.6)],
            ..Sprite::default()
        };
        let entity = world.spawn((transform, sprite));
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, entity));
        let mut time = 0.0;
        let mut run = |inspector: &mut Inspector, world: &mut World, events| {
            let mut texts = Vec::new();
            for events in [events, Vec::new(), Vec::new()] {
                time += 1.0;
                texts = frame(inspector, world, &ctx, time, events).0;
            }
            texts
        };
        let buttons: [&[&str]; 4] = [
            &["translation", "X"],
            &["scale", "Y"],
            &["rect", "[1]", "X"],
            &["rotation", "Z"],
        ];
        for button in buttons {
            let texts = run(&mut inspector, &mut world, Vec::new());
            let at = find_after(&texts, button).center();
            run(&mut inspector, &mut world, click(Primary, at));
        }
        let set = world.get::<Transform>(entity).expect("a transform");
        assert_eq!(set.translation, Vec3::new(0.0, 2.0, 3.0));
        assert_eq!(set.scale, Vec3::new(2.0, 1.0, 4.0));
        let (z, y, x) = set.rotation.to_euler(EulerRot::ZYX);
        let near = |a: f32, b: f32| (a - b).abs() < 1e-5;
        assert!(
            near(z, 0.0) && near(y, 0.2) && near(x, 0.1),
            "{x}, {y}, {z}"
        );
        let rect = world.get::<Sprite>(entity).expect("a sprite").rect;
        assert_eq!(rect, [Vec2::new(0.1, 0.2), Vec2::new(1.0, 0.6)]);
        assert_eq!(inspector.log.operations().len(), 4);
    }

    /// A drag of an axis sets that component alone, at its own path, so that a system moving
    /// another at the ticks between the frames keeps what it moved; the drag is one edit, whose
    /// undo takes back that component alone.
    #[test]
    fn an_axis_drag_sets_its_component_alone() {
        let mut world = World::default();
        let entity = world.spawn(Transform::IDENTITY);
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, entity));
        let mut time = 0.0;
        let mut run = |inspector: &mut Inspector, world: &mut World, events, tick| {
            time += 1.0;
            frame_ticking(inspector, world, &ctx, time, events, tick)
        };
        let mut texts = Vec::new();
        for _ in 0..3 {
            texts = run(&mut inspector, &mut world, Vec::new(), true).0;
        }
        let at = beside(&texts, find_after(&texts, &["translation", "X"]));
        let translation = |world: &World| {
            world
                .get::<Transform>(entity)
                .expect("a transform")
                .translation
        };
        let mut sets = 0;
        for events in slide(at) {
            sets += run(&mut inspector, &mut world, events, false).1;
            // The system, at the tick, after the frame drew it.
            let moved = world.get_mut::<Transform>(entity).expect("a transform");
            moved.translation.y += 1.0;
            Layer::tick(&mut inspector, &mut world, Tick { number: 0 });
        }
        let dragged = translation(&world);
        assert!(sets > 1 && dragged.x != 0.0, "{sets} sets, to {dragged}");
        assert_eq!(dragged.y, 5.0);
        assert_eq!(inspector.log.operations().len(), 1, "one edit");
        run(
            &mut inspector,
            &mut world,
            shortcut(egui::Key::Z, false),
            true,
        );
        assert_eq!(translation(&world), Vec3::new(0.0, 5.0, 0.0));
    }

    /// Sets of a vector and of its components that wait for a tick draw it as they'll make it, in
    /// the order they were made, as at frame rates above the simulation's; and the tick makes it
    /// so.
    #[test]
    fn sets_of_a_vector_and_of_its_components_draw_in_order() {
        let mut world = World::default();
        let entity = world.spawn(Transform::IDENTITY);
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, entity));
        let set = |path: &str, value| Edit {
            component: "Transform".to_owned(),
            path: path.to_owned(),
            value,
        };
        let whole = |x, y, z| Value::Vec3(Vec3::new(x, y, z));
        let mut time = 0.0;
        for (sets, made) in [
            (
                vec![
                    set("translation", whole(1.5, 2.5, 3.5)),
                    set("translation.x", Value::F32(7.5)),
                ],
                Vec3::new(7.5, 2.5, 3.5),
            ),
            (
                vec![
                    set("translation.y", Value::F32(8.5)),
                    set("translation", whole(4.5, 5.5, 6.5)),
                ],
                Vec3::new(4.5, 5.5, 6.5),
            ),
        ] {
            inspector.queue(Pass {
                sets,
                ..Pass::default()
            });
            let mut texts = Vec::new();
            for _ in 0..3 {
                time += 1.0;
                let drawn =
                    frame_ticking(&mut inspector, &mut world, &ctx, time, Vec::new(), false);
                texts = drawn.0;
            }
            // What each axis's drag shows, beside its button.
            let shown = ["X", "Y", "Z"].map(|axis| {
                let (text, _) = next_to(&texts, find_after(&texts, &["translation", axis]));
                text.parse::<f32>()
                    .unwrap_or_else(|_| panic!("a number: {text}"))
            });
            assert_eq!(Vec3::from(shown), made, "{texts:?}");
            Layer::tick(&mut inspector, &mut world, Tick { number: 0 });
            let held = world.get::<Transform>(entity).expect("a transform");
            assert_eq!(held.translation, made);
        }
    }

    /// A value refused for a vector's component shows under the vector's row.
    #[test]
    fn a_value_refused_for_a_component_shows_under_its_vector() {
        let mut world = World::default();
        let entity = world.spawn(everything());
        let ctx = crate::egui_context();
        let mut inspector = Inspector::default();
        inspector.pick(id(&world, entity));
        let refused = Edit {
            component: "Everything".to_owned(),
            path: "offset.x".to_owned(),
            value: Value::UInt(1),
        };
        inspector.apply(&mut world, inspector.picked, refused);
        let says = format!(
            "can't set `Everything.offset.x` of entity {}: expected F32, got UInt",
            id(&world, entity)
        );
        let mut texts = Vec::new();
        for time in 0..3 {
            let drawn = frame(
                &mut inspector,
                &mut world,
                &ctx,
                f64::from(time),
                Vec::new(),
            );
            texts = drawn.0;
        }
        let offset = find(&texts, "Everything", "offset");
        let message = find(&texts, "offset", &says);
        assert!(message.min.y > offset.max.y, "{texts:?}");
    }
}

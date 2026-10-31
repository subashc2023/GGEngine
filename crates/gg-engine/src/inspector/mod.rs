//! The inspector: the Entities panel, which lists the world's entities as the tree of its
//! hierarchy, and creates, duplicates and deletes them, and the Inspector panel, which shows the
//! picked entity's components, edits their fields, and adds and removes them. It reaches the world
//! only through its by-name calls, its IDs and its hierarchy, as the tool API will, so that an
//! editor can show it for a game in another process, and names entities by their names, where they
//! have one, and their IDs. Its edits and operations are its commands: the panels queue them, its
//! next tick makes them, and a recording keeps them. Its link lets another layer pick and edit as
//! the panels do. A remote Inspector shows a game's world in another process, as the tool API gives
//! it, and edits it through the API.

mod commands;
mod entities;
mod fields;
mod inspected;
mod link;
mod remote;
#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::collections::HashSet;
use std::mem;
use std::rc::Rc;

use egui::{KeyboardShortcut, Modifiers};
use tracing::field::display;
use tracing::warn;

use commands::{Command, Queue};
use fields::{Fields, Slot};
use inspected::{Inspected, Pending};
pub use link::{InspectorLink, Placed};
use link::{Linked, Linking, links_field};
use remote::Remote;

use crate::reflect::{Meta, Shape, Type, Value};
use crate::{
    Commands, Context, EntityId, Layer, Log, Name, Operation, Reflect, Tick, Transform, Ui, World,
};

/// The Entities and Inspector panels, which an app adds as an overlay,
/// `.overlay(gg_engine::Inspector::default())`. The Entities panel lists the world's entities as
/// the tree of its hierarchy, whose rows' arrows show and hide their children, or those with one
/// component as a list, to pick one from, and finds one by an ID pasted into its Find field, from
/// a scene or a log, opening the rows above its own; a click below the rows, or the Entity menu,
/// unpicks. A row's context menu, the one below the rows and the Entity menu create empty
/// entities, which they pick, and a row's and the Entity menu duplicate entities, picking the
/// copy, with Ctrl+D (Cmd+D on macOS) too, and delete them, with their descendants. The Inspector
/// panel shows the picked entity's components, each field with a widget for its type and hints, and
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
/// Edit menu's names, show from the tick. A world cleared, replaced or restored, which is another,
/// as [`World::serial`] says, unpicks and closes every row, which drops the edits made to the world
/// before, and those queued for it: at the next tick, or at once as the run puts a world in place
/// and calls [`restored`](Layer::restored), as a snapshot's restore, a lockstep joiner's and the
/// tool API's `stop` do. But the very world it left coming back, as an app that swaps one world for
/// another and back has it, or as `stop` brings back the world as edited, finds its edits, pick and
/// open rows as they were. Another layer picks and edits as the panels do through the
/// [`InspectorLink`] that [`link`](Self::link) gives it.
///
/// A remote Inspector, which [`remote`](Self::remote) makes, shows the world of the game its link
/// reaches, as [`InspectorLink::reach`] says, in place of the app's own, as the tool API gives it,
/// a few times a second, and edits it as the panels do through the API, as the game's client: its
/// sets of a gesture join as the log's do, its Edit menu undoes and redoes the client's edits, and
/// the panels draw what waits for the game's answers as it will be.
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
    /// The serial of the world the log, the pick and the open rows are of, as the Inspector found
    /// it as it attached, or as the last tick or restore did: another, as a world cleared, replaced
    /// or restored is, puts them aside at the next.
    world: Option<u64>,
    /// The log, the pick and the open rows of the world the Inspector left last, which they come
    /// back from if that world comes back.
    left: Option<Left>,
    /// Why the last operation, undo or redo was refused, until the next edit, operation, undo or
    /// redo.
    not_done: Option<String>,
    /// What the panels and the link did since the last tick, which it makes.
    queued: Queue,
    /// Whose edit goes on, if a set has been queued since the last seal, operation, undo or redo,
    /// which leave none going on: a seal queued otherwise would end none, so none is.
    open: Option<Gesture>,
    /// What it shares with its links.
    link: Rc<RefCell<Linked>>,
    /// The game's world it shows, if it's remote.
    remote: Option<Remote>,
}

/// What the Inspector had of a world it left, by its serial.
struct Left {
    world: u64,
    log: Log,
    picked: Option<EntityId>,
    expanded: HashSet<EntityId>,
}

/// Whose gesture an edit going on is: the panels', which their input ends, or the link's, which
/// its seal ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gesture {
    Panels,
    Link,
}

/// Undoes the last edit.
const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Z);

/// Redo the last edit undone; the Edit menu shows the first.
const REDO: [KeyboardShortcut; 2] = [
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), egui::Key::Z),
    KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Y),
];

/// Duplicates the entity picked, as Hazel's Ctrl+D does.
const DUPLICATE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::D);

/// A step through the log of edits, as the Edit menu or a key asks for one.
#[derive(Clone, Copy)]
enum Step {
    Undo,
    Redo,
}

/// What a frame's input, in the windows the panels draw in, says of the edits: a press, which
/// starts another, and a release, which ends one; whether a widget has the pointer or the keys,
/// which keeps one going; the step through them a key asks for; and whether Ctrl+D asks for the
/// entity picked to be duplicated.
#[derive(Default)]
struct Cues {
    pressed: bool,
    released: bool,
    held: bool,
    step: Option<Step>,
    duplicate: bool,
}

impl Cues {
    /// Adds what `egui`'s input says, as the window that's drawing has it: a panel's own, if it's
    /// torn off, or the main one. Its keys ask for a step, or a duplicate, unless a text field
    /// there has them, which undo its own text.
    fn read(&mut self, egui: &egui::Context) {
        let pointer =
            egui.input(|input| (input.pointer.any_pressed(), input.pointer.any_released()));
        self.pressed |= pointer.0;
        self.released |= pointer.1;
        self.held |= egui.egui_is_using_pointer() || egui.egui_wants_keyboard_input();
        if egui.text_edit_focused() {
            return;
        }
        egui.input_mut(|input| {
            // Redo's first, since a shortcut matches with Shift held as well.
            if self.step.is_none() {
                if REDO.iter().any(|redo| input.consume_shortcut(redo)) {
                    self.step = Some(Step::Redo);
                } else if input.consume_shortcut(&UNDO) {
                    self.step = Some(Step::Undo);
                }
            }
            self.duplicate |= input.consume_shortcut(&DUPLICATE);
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
#[derive(Clone)]
struct Edit {
    component: String,
    path: String,
    value: Value,
}

/// What a pass of the panels found to do, which [`Inspector::queue`] queues for the next tick: the
/// sets of the picked entity's fields its widgets made, the operations its menus and Ctrl+D chose,
/// what its input says of the edits, and the step the Edit menu chose.
#[derive(Default)]
struct Pass {
    sets: Vec<Edit>,
    /// Creates, duplicates, deletes, adds and removes.
    operations: Vec<Command>,
    cues: Cues,
    chosen: Option<Step>,
}

impl Layer for Inspector {
    /// Knows the world it shows from the start, which a stopped run's may be for its first edits,
    /// with no tick yet.
    fn attach(&mut self, cx: &mut Context) {
        if self.remote.is_none() {
            self.world = Some(cx.world().serial());
        }
    }

    /// Goes to the world now in place, if it's another, at once rather than at the next tick: as
    /// the tool API's `stop` brings back the world as edited, whose log, pick and open rows come
    /// back, since the next tick may be the next copy's, which would drop them as those of a world
    /// left before; as a run restored from a snapshot starts; and as a lockstep joiner's world
    /// gives way to its host's.
    fn restored(&mut self, cx: &mut Context) {
        let serial = cx.world().serial();
        if self.remote.is_none() && self.world != Some(serial) {
            self.swap_to(serial);
        }
    }

    fn detach(&mut self, cx: &mut Context) {
        self.log.clear(cx.world_mut());
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        // The app's own world isn't the one it shows.
        if self.remote.is_some() {
            return;
        }
        // What was queued for the world before goes, as a replay drops it at the same tick.
        if self.world.is_some_and(|serial| serial != world.serial()) {
            self.swap_to(world.serial());
        }
        self.world = Some(world.serial());
        let mut queued = mem::take(&mut self.queued.0);
        for command in queued.drain(..) {
            self.run(world, command);
        }
        // Kept for its memory.
        self.queued.0 = queued;
        let linked = &mut *self.link.borrow_mut();
        linked.queued = false;
        linked.picked = self.picked;
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.queued)
    }

    fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
        let _zone = gg_core::zone!("inspector");
        // Apart, so that it's what the panels show while they change the rest.
        let mut remote = self.remote.take();
        if let Some(remote) = &mut remote {
            remote.update(self);
        }
        let world = cx.world();
        let source: &dyn Inspected = match &remote {
            Some(remote) => remote,
            None => world,
        };
        self.follow(source);
        let mut pass = Pass::default();
        let labels = match &remote {
            Some(remote) => remote.pending(self.picked),
            None => self.pending(),
        };
        ui.menu("Edit", |ui| pass.chosen = self.edit_menu(ui, &labels));
        ui.menu("Entity", |ui| {
            self.entity_menu(source, ui, &mut pass.operations)
        });
        ui.panel("Entities", |ui| self.entities(source, ui, &mut pass));
        ui.panel("Inspector", |ui| {
            // As the pick the Entities panel may just have made has them.
            let pending = match &remote {
                Some(remote) => remote.pending(self.picked),
                None => self.pending(),
            };
            // A new panel floats smaller than its contents.
            let scroll = egui::ScrollArea::vertical();
            scroll.show(ui, |ui| self.inspect(source, ui, &mut pass, &pending));
        });
        // The Edit and Entity menus' keys, wherever the panels are.
        pass.cues.read(ui.egui());
        self.duplicate_picked(source, &mut pass);
        // Every pass, even one egui discards, which alone had the frame's input; and empty ones
        // too, whose cues end an edit.
        match &mut remote {
            Some(remote) => remote.send(pass, self),
            None => self.queue(pass),
        }
        let linked = &mut *self.link.borrow_mut();
        linked.picked = self.picked;
        linked.placed = remote
            .as_ref()
            .and_then(|remote| remote.placed(self.picked));
        self.remote = remote;
    }
}

impl Inspector {
    /// A link through which another layer picks and edits as the panels do, as
    /// [`InspectorLink`] says: each is a clone of one, which the Inspector shares.
    pub fn link(&self) -> InspectorLink {
        InspectorLink(self.link.clone())
    }

    #[cfg(feature = "dev")]
    /// A remote Inspector, which shows the world of the game its link reaches, as
    /// [`InspectorLink::reach`] says, rather than the app's own, and edits it as the game's client
    /// `client`, the author of its edits, which its Edit menu undoes and redoes.
    pub fn remote(client: &str) -> Self {
        Self {
            remote: Some(Remote::new(client)),
            ..Self::default()
        }
    }

    /// What waits for the next tick: the queued sets of the picked entity's fields, and then the
    /// link's since the last pass, which the pass queues after them; the queued adds and removes;
    /// and the edits the log would undo and redo next.
    fn pending(&self) -> Pending {
        let id = self.picked;
        let queued = self.queued.0.iter().filter_map(|command| match command {
            Command::Set(set, edit) if Some(*set) == id => Some(edit.clone()),
            _ => None,
        });
        let linked = self.link.borrow();
        let linking = linked.pending.iter().filter_map(|linking| match linking {
            Linking::Set(set, edit) if Some(*set) == id => Some(edit.clone()),
            _ => None,
        });
        let changes = self.queued.0.iter().filter_map(|command| match command {
            Command::Add(entity, component, _) => Some((*entity, component.clone(), true)),
            Command::Remove(entity, component) => Some((*entity, component.clone(), false)),
            _ => None,
        });
        let first = |edit: Option<&[Operation]>| edit.and_then(<[_]>::first).map(|o| o.to_string());
        Pending {
            waiting: queued.chain(linking).collect(),
            changes: changes.collect(),
            undo: first(self.log.undoable()),
            redo: first(self.log.redoable()),
        }
    }

    /// Makes the pick the link asked for since the last pass, before the panels draw: a living
    /// entity's ID picks it and opens the rows above its own, as Find does; an ID whose entity
    /// isn't known yet to be alive or not, as one a game has just made, picks it too, a remote
    /// Inspector opening the rows above it as its read comes; any other, or none, unpicks. A debug
    /// build checks the link's sets against the world's types first, which the link hasn't, as
    /// [`InspectorLink::set`] says, once it shows the world: a game's, once its types are read.
    fn follow(&mut self, world: &dyn Inspected) {
        if cfg!(debug_assertions) && world.waiting().is_none() {
            for linking in &self.link.borrow().pending {
                if let Linking::Set(_, edit) = linking {
                    assert!(
                        links_field(world.types(), edit),
                        "a link sets a field of a component's own that holds nothing the panels \
                         set at a path of its own, or a component of a vector that's one; got \
                         `{}` of `{}`",
                        edit.path,
                        edit.component
                    );
                }
            }
        }
        let Some(pick) = self.link.borrow_mut().pick.take() else {
            return;
        };
        match pick.filter(|&id| world.alive(id) != Some(false)) {
            Some(id) => {
                self.pick(id);
                self.open_ancestors(world, id);
            }
            None => self.unpick(),
        }
    }

    /// The Edit menu: Undo and Redo, each naming the edit it steps through, as `pending` has
    /// them; returns the one clicked.
    fn edit_menu(&self, ui: &mut egui::Ui, pending: &Pending) -> Option<Step> {
        let items = [
            (Step::Undo, "Undo", &pending.undo, UNDO),
            (Step::Redo, "Redo", &pending.redo, REDO[0]),
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
    /// while it's alive, Create Empty Child, which makes a child of it, Duplicate, which copies it
    /// and its descendants, as Ctrl+D (Cmd+D on macOS) does too, and Delete, which deletes it and
    /// its descendants. The operation chosen goes in `operations`.
    fn entity_menu(
        &mut self,
        world: &dyn Inspected,
        ui: &mut egui::Ui,
        operations: &mut Vec<Command>,
    ) {
        let live = self.picked.filter(|&id| world.alive(id) == Some(true));
        if ui.button("Create Empty").clicked() {
            operations.push(empty(None));
        }
        let child = ui.add_enabled(live.is_some(), egui::Button::new("Create Empty Child"));
        let shortcut = ui.ctx().format_shortcut(&DUPLICATE);
        let duplicate = egui::Button::new("Duplicate").shortcut_text(shortcut);
        let duplicate = ui.add_enabled(live.is_some(), duplicate);
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
        if duplicate.clicked() {
            operations.push(Command::Duplicate(id));
        }
        if delete.clicked() {
            operations.push(Command::Delete(id));
        }
    }

    /// Has `pass` duplicate the entity picked, while it's alive in `world`, if its input's Ctrl+D
    /// asks for that.
    fn duplicate_picked(&self, world: &dyn Inspected, pass: &mut Pass) {
        let live = self.picked.filter(|&id| world.alive(id) == Some(true));
        if let Some(id) = live.filter(|_| pass.cues.duplicate) {
            pass.operations.push(Command::Duplicate(id));
        }
    }

    /// Queues a pass of the panels, for the next tick to make: first what the link did since the
    /// last pass, whose events came before the pass's input, its sets of one field as one edit
    /// from its gesture's first set to its seal; then the pass's sets of the picked entity's
    /// fields, as one edit from a press of the pointer to its release, or while a widget has the
    /// keys, as its input says, so that a drag, or a number typed, is one edit; then its
    /// operations, each of which ends the edit going on; then the step the Edit menu chose, or a
    /// key asks for. Each gesture's first set ends the other's edit going on, and the pass's input
    /// ends only its own.
    fn queue(&mut self, pass: Pass) {
        let Pass {
            sets,
            operations,
            cues,
            chosen,
        } = pass;
        let mut linked = mem::take(&mut self.link.borrow_mut().pending);
        for linking in linked.drain(..) {
            match linking {
                Linking::Set(id, edit) => {
                    self.seal(Gesture::Panels);
                    self.queued.0.push(Command::Set(id, edit));
                    self.link.borrow_mut().queued = true;
                    self.open = Some(Gesture::Link);
                }
                Linking::Seal => self.seal(Gesture::Link),
            }
        }
        // Kept for its memory.
        self.link.borrow_mut().pending = linked;
        // Before this pass's sets: a press begins another edit, even while a text field keeps
        // the keys.
        if cues.pressed {
            self.seal(Gesture::Panels);
        }
        if let Some(id) = self.picked
            && !sets.is_empty()
        {
            self.seal(Gesture::Link);
            for edit in sets {
                self.queued.0.push(Command::Set(id, edit));
            }
            self.open = Some(Gesture::Panels);
        }
        for operation in operations {
            self.queued.0.push(operation);
            self.open = None;
        }
        if cues.released || !cues.held {
            self.seal(Gesture::Panels);
        }
        if let Some(step) = chosen.or(cues.step) {
            self.queued.0.push(Command::Step(step));
            self.open = None;
            self.link.borrow_mut().stepped += 1;
        }
    }

    /// Queues a seal if `gesture`'s edit goes on, which it ends, so that a pass that does nothing
    /// queues nothing: only a set the log takes begins one, and seals, operations, undos and redos
    /// end it.
    fn seal(&mut self, gesture: Gesture) {
        if self.open == Some(gesture) {
            self.queued.0.push(Command::Seal);
            self.open = None;
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
            Command::Duplicate(entity) => {
                let copies = Vec::new();
                self.operate(world, Operation::Duplicate { entity, copies });
            }
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
    /// then picks the entity a create made, or a duplicate's copy of its entity, opening the rows
    /// of its ancestors. A refusal shows above the components, and is logged.
    fn operate(&mut self, world: &mut World, operation: Operation) {
        let (entity, said) = (operation.entity().map(display), operation.to_string());
        if let Err(error) = self.log.apply(world, operation) {
            warn!(code = error.code(), entity, operation = %said, "{error}");
            self.not_done = Some(error.to_string());
            return;
        }
        self.not_done = None;
        let made = self.log.undoable().and_then(<[_]>::first);
        let made = made.and_then(|done| match done {
            Operation::Create { .. } | Operation::Duplicate { .. } => done.entity(),
            _ => None,
        });
        let Some(made) = made else {
            return;
        };
        self.pick(made);
        self.open_ancestors(&*world, made);
    }

    /// Forgets the values `entity`'s `component` refused, if it's the entity picked, whose
    /// refusals are kept: an add or a remove makes it anew.
    fn forget(&mut self, entity: EntityId, component: &str) {
        if self.picked == Some(entity) {
            self.refusals
                .retain(|refusal| refusal.component != component);
        }
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

    /// Goes from the world it showed to the one whose serial is `serial`, another: another world's
    /// entities aren't those the pick, the rows, the log and the commands queued name, though they
    /// may have their IDs, as a scene reopened gives them back; nor are those the link asked for
    /// since the last pass. The pick, the rows and the log go aside, for the world they're of to
    /// find again if it comes back; those of the world left before come back if it's that world,
    /// and go otherwise, since it never will.
    fn swap_to(&mut self, serial: u64) {
        let left = self.world.map(|world| Left {
            world,
            log: mem::take(&mut self.log),
            picked: self.picked,
            expanded: mem::take(&mut self.expanded),
        });
        self.unpick();
        if let Some(back) = self.left.take().filter(|left| left.world == serial) {
            self.log = back.log;
            self.picked = back.picked;
            self.expanded = back.expanded;
        }
        self.left = left;
        self.world = Some(serial);
        self.not_done = None;
        self.not_found = None;
        self.open = None;
        self.queued.0.clear();
        let linked = &mut *self.link.borrow_mut();
        linked.pick = None;
        linked.pending.clear();
    }

    /// Opens the rows of the ancestors of the entity of ID `id`, so that the tree shows its own; up
    /// to one met again, as a hierarchy put together from reads of a game's world at two moments
    /// may have it.
    fn open_ancestors(&mut self, world: &dyn Inspected, id: EntityId) {
        let mut met = HashSet::from([id]);
        let mut up = world.parent(id);
        while let Some(parent) = up.filter(|&parent| met.insert(parent)) {
            self.expanded.insert(parent);
            up = world.parent(parent);
        }
    }

    /// The Inspector panel: the picked entity's components, each under a header that collapses,
    /// with its fields' widgets, and beside it a menu that removes it; and below them, a menu that
    /// adds one; each drawn as what's `pending` will leave it. The fields they change, the
    /// operations the menus choose, and what the input of the window it draws in says of them, go
    /// in `pass`.
    fn inspect(
        &self,
        world: &dyn Inspected,
        ui: &mut egui::Ui,
        pass: &mut Pass,
        pending: &Pending,
    ) {
        pass.cues.read(ui.ctx());
        if let Some(not_done) = &self.not_done {
            ui.label(not_done.as_str());
        }
        let Some(id) = self.picked else {
            ui.label("Nothing picked: pick an entity in the Entities panel");
            return;
        };
        match world.alive(id) {
            Some(true) => {}
            Some(false) => {
                ui.label(format!("Entity {id} was despawned"));
                return;
            }
            None => {
                ui.label(format!("Reading entity {id}"));
                return;
            }
        }
        let heading = match world.name(id) {
            Some(name) => format!("{name}, entity {id}"),
            None => format!("Entity {id}"),
        };
        ui.label(heading);
        if let Some(children) = children(world, id) {
            ui.label(children);
        }
        // So that what's typed into a field of one entity never lands in the field of the next
        // one picked; and, being its own, so that the fields keep their widgets' IDs, and a drag
        // or a number being typed, as the labels above them come and go.
        let scope = egui::UiBuilder::new().scope_id(ui.make_persistent_id(id));
        ui.scope_builder(scope, |ui| {
            let types = world.types();
            for component in world.components(id) {
                // Drawn as the next tick will leave it.
                if pending.change(id, component) == Some(false) {
                    continue;
                }
                let (Some(value), Some(index)) =
                    (world.component(id, component), types.find(component))
                else {
                    continue;
                };
                let header = egui::CollapsingHeader::new(component)
                    .id_salt(component)
                    .default_open(true)
                    .show(ui, |ui| {
                        let value = match value {
                            Ok(value) => value,
                            Err(error) => {
                                ui.label(error);
                                return;
                            }
                        };
                        let mut fields = Fields {
                            world,
                            types,
                            entity: id,
                            component,
                            refusals: &self.refusals,
                            edits: &mut pass.sets,
                            waiting: &pending.waiting,
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
            self.add_menu(world, ui, id, pending, &mut pass.operations);
        });
    }

    /// The Add Component menu, below the components of the entity of ID `id`: the types it can be
    /// given, in the order they registered, each with its doc on hover; but not those it has, nor
    /// a parent, which the world gives it, nor those opaque as a whole, such as handles, which are
    /// code's to make, nor those an add of is `pending`. A type whose default an add would refuse
    /// shows disabled, saying why on hover. The defaults are made only while it's open. The add
    /// chosen, of the type's default, goes in `operations`.
    fn add_menu(
        &self,
        world: &dyn Inspected,
        ui: &mut egui::Ui,
        id: EntityId,
        pending: &Pending,
        operations: &mut Vec<Command>,
    ) {
        let config = egui::menu::MenuConfig::new().style(crate::menu_style);
        let button = egui::menu::MenuButton::new("Add Component").config(config);
        button.ui(ui, |ui| {
            let types = world.types();
            let own = world.components(id);
            for name in world.component_types() {
                let info = types.find(name).map(|index| types.info(index));
                let opaque = info.is_some_and(|info| matches!(info.shape, Shape::Opaque));
                let adding = pending.change(id, name) == Some(true);
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
                        item.on_disabled_hover_text(error);
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

/// [`Parent`](crate::Parent)'s name: the world gives an entity one as it's made a child, so the
/// Inspector neither adds nor removes one.
const PARENT: &str = "Parent";

/// The radius of each of the three dots across the button of a component's menu, and how far apart
/// their middles are: about as large and as far apart as the dots of Inter's ellipsis at egui's 13
/// points of button text.
const DOT: f32 = 1.0;
const DOTS_APART: f32 = 4.0;

/// The menu of `component`, at the right end of the row of its header, `header`, as it shows,
/// whose Remove Component takes it away from the entity of ID `id`, the operation going in
/// `operations`. A framed header is as wide as `ui`, and the menu is over its end, taking the
/// clicks there from it, since it's made after it. It's in a child of `ui`, which leaves the
/// cursor where the header's body left it, for the next header.
fn component_menu(
    ui: &mut egui::Ui,
    header: egui::Rect,
    id: EntityId,
    component: &str,
    operations: &mut Vec<Command>,
) {
    let size = egui::Vec2::splat(ui.spacing().interact_size.y);
    // `ui`'s end rather than the header's, which is the end of its name where it isn't framed.
    let at = egui::pos2(shown_right(ui) - size.x, header.top());
    // Laid out from its right, so that the button ends there.
    let child = egui::UiBuilder::new()
        .id_salt(("component menu", component))
        .max_rect(egui::Rect::from_min_size(at, size))
        .layout(egui::Layout::right_to_left(egui::Align::Center));
    let mut menu = ui.new_child(child);
    // Three dots across its middle, painted in the color of its text as it's lit, and centered both
    // ways: a font's ellipsis sits on its baseline, some 4 points below the middle of the row egui
    // centers a button's text in, and neither Inter nor egui's proportional fonts have the midline
    // one, `⋯`.
    let dots = egui::Atom::paint(
        egui::vec2(2.0 * (DOT + DOTS_APART), 2.0 * DOT),
        |ui, args| {
            for step in [-1.0, 0.0, 1.0] {
                let middle = args.rect.center() + egui::vec2(step * DOTS_APART, 0.0);
                ui.painter()
                    .circle_filled(middle, DOT, args.fallback_text_color);
            }
        },
    );
    let button = egui::Button::new(dots).min_size(size);
    let config = egui::menu::MenuConfig::new().style(crate::menu_style);
    let button = egui::menu::MenuButton::from_button(button).config(config);
    button.ui(&mut menu, |ui| {
        if ui.button("Remove Component").clicked() {
            operations.push(Command::Remove(id, component.to_owned()));
        }
    });
}

/// Where what `ui` shows ends on the right: its end, but no further than where it's cut off, since
/// rows wider than the panel, as a narrow Inspector's axes are, widen it past its edge; and short
/// of the strip along that edge where a scroll area's bar floats over its contents, which takes
/// the clicks there.
fn shown_right(ui: &egui::Ui) -> f32 {
    let scroll = &ui.spacing().scroll;
    let bar = scroll.bar_width + scroll.bar_outer_margin;
    ui.max_rect().right().min(ui.clip_rect().right()) - bar
}

/// The command that creates an empty entity, the last of `parent`'s children, or a root: a name
/// that says so, and a transform that leaves it where its parent is.
fn empty(parent: Option<EntityId>) -> Command {
    let name = Name("Empty Entity".to_owned());
    Command::Create {
        parent,
        components: vec![
            ("Name".into(), name.to_value()),
            ("Transform".into(), Transform::IDENTITY.to_value()),
        ],
    }
}

/// The children of the entity of ID `id`, by their IDs, as "Children: 5f3a9c0e12b47d68,
/// 0c41b2e79d6a3f15": the first 16 of them, then how many more there are; `None` if it has none.
fn children(world: &dyn Inspected, id: EntityId) -> Option<String> {
    let mut children = world.children(id).peekable();
    children.peek()?;
    let shown: Vec<String> = children
        .by_ref()
        .take(16)
        .map(|child| child.to_string())
        .collect();
    let mut line = format!("Children: {}", shown.join(", "));
    let more = children.count();
    if more > 0 {
        line += &format!(" and {more} more");
    }
    Some(line)
}

//! The inspector: the Entities panel, which lists the world's entities as the tree of its
//! hierarchy and creates, duplicates and deletes them, and the Inspector panel, which shows the
//! picked entity's components, edits their fields, and adds and removes them. It reaches the world
//! only through its by-name calls, IDs and hierarchy, as the tool API will, so an editor can show
//! it for a game in another process. It names entities by their names, where they have one, and
//! their IDs. Its edits and operations are its commands: the panels queue them, its next tick
//! makes them, and a recording keeps them. Its link lets another layer pick and edit as the panels
//! do. A remote Inspector shows a game's world in another process through the tool API, and edits
//! it through the API. Development builds alone have the UI (`dev`): a build without it has
//! `none.rs` in its place.

mod commands;
mod entities;
mod fields;
mod inspected;
mod link;
mod placed;
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
pub use link::InspectorLink;
use link::{Linked, Linking, Picking, links_field};
pub use placed::Placed;
use remote::Remote;

use crate::reflect::{Meta, Shape, Type, Value};
use crate::{
    Commands, Context, EntityId, Layer, Log, Name, Operation, Reflect, Tick, Transform, Ui, World,
};

/// The Entities and Inspector panels, which an app adds as an overlay,
/// `.overlay(gg_engine::Inspector::default())`.
///
/// # Entities
///
/// The tree of the world's hierarchy.
///
/// - A row's arrow shows or hides its children; Collapse All hides them all. A list of the
///   entities with one component can replace the tree.
/// - Find (the magnifier opens it over the header) takes an ID pasted from a scene or log, or part
///   of a name, the next match at each Enter, opening the rows above it.
/// - A click picks a row's entity alone; Ctrl (Cmd on macOS) adds or removes it; Shift picks the
///   rows from the active pick's to it. A click below the rows, or the Entity menu, unpicks. The
///   entity picked last is the active one, which the Inspector panel shows.
/// - A row's context menu, the one below the rows, the + menu and the Entity menu create empty
///   entities and pick them.
/// - A row's menu and the Entity menu duplicate entities (Ctrl+D, Cmd+D, too), picking the copies,
///   and delete them with their descendants (the panel's Delete key, too). Each acts on all
///   picked at once as one edit, those under another picked going with it.
/// - A row's menu also renames its entity in its row, copies its ID and moves it to the roots.
/// - Dragging a row onto another makes its entity that one's last child; onto the space below
///   the rows, a root. Each keeps its place in the world, as [`World::reparent_in_place`] says.
/// - A double click, or the menu's Frame in View, frames the entity through the link, as
///   [`InspectorLink::framed`] says.
/// - While the panel has the keys (from a press in it until a press elsewhere), F frames the
///   picked entities, F2 renames the active one and Delete deletes them.
///
/// # Inspector
///
/// The active entity's components, each field with a widget for its type and hints.
///
/// - A widget's change sets the field through a [`Log`] of edits, as [`World::set_field`] does,
///   checking the value. A refused value shows beside its field and is logged, as an operation
///   refused is, above the components.
/// - An enum switches variant from a choice of them, to the fields that variant last had while
///   the entity was drawn, else their defaults. An option's checkbox turns it on, to what it last
///   held while the entity was drawn, else its default, or off.
/// - Each vector component (set at its own path) and rotation angle has a button of its axis that
///   resets it to its default.
/// - The Add Component menu, below the components, adds one with its type's default. The menu
///   beside each header removes it.
/// - An entity of a prefab's instance says which instance. It marks, strong and with a bar of
///   the selection's color, each own component and field that differs from its prefab's, and each
///   component the prefab doesn't give, as the tool API's `entity` finds them. A marked field's
///   or component's menu reverts it. An outermost instance's root has Unlink.
/// - The Edit menu, Ctrl+Z (Cmd+Z) and Ctrl+Shift+Z or Ctrl+Y undo and redo, a drag or typed
///   number being one edit.
///
/// # Ticks and worlds
///
/// The panels queue each frame's edits, operations, undos and redos for the next tick, so the
/// world changes only at ticks. Until then a set field is drawn as the set will make it, and a
/// refusal and the Edit menu's names show from the tick.
///
/// A world cleared, replaced, restored or renewed is another ([`World::serial`]). The Inspector
/// then unpicks, closes every row, and drops the earlier world's edits and queued commands, at
/// the next tick or, at once, when the run calls [`restored`](Layer::restored) after putting a
/// world in place (a snapshot's restore, a lockstep joiner's, the tool API's `stop`) or renewing
/// it (`scene new`, `scene open`). The very world it left coming back, as when an app swaps
/// worlds and back, or `stop` restores the world as edited, finds its edits, pick and open rows
/// as they were.
///
/// A SyncTest's copy of the world (`--sync-test`), which each tick runs again on, takes none of
/// that: the tick's edits run again there through a copy of the log as the tick started, and the
/// pick and open rows stay.
///
/// Another layer picks and edits like the panels through the [`InspectorLink`] from
/// [`link`](Self::link).
///
/// # Remote
///
/// A remote Inspector ([`remote`](Self::remote)) shows the world of the game its link reaches
/// ([`InspectorLink::reach`]), not the app's own, a few times a second. It edits through the tool
/// API as the game's client:
///
/// - Its sets of a gesture join like the log's.
/// - Its Edit menu undoes and redoes the client's edits.
/// - The panels draw what waits for the game's answers as it will be.
/// - A row's context menu saves the entity and its descendants as a prefab, linked in their
///   place, through the API's `save_prefab`. A game's own Inspector doesn't offer it: a save
///   writes files, which neither a replay nor a lockstep peer of its commands would have.
#[derive(Default)]
pub struct Inspector {
    /// The entity picked last, the active one, by its ID, which outlives it: a despawned pick is
    /// still named by it. `others` were picked with it, in pick order: none unless one is active.
    picked: Option<EntityId>,
    others: Vec<EntityId>,
    /// The open rows of the Entities tree, by IDs, which outlive entities: an entity an undo
    /// gives back comes back open.
    expanded: HashSet<EntityId>,
    /// The component type the Entities panel lists; all entities if `None`.
    with: Option<String>,
    find: String,
    /// Why the last ID or name entered in Find found no entity.
    not_found: Option<String>,
    /// Whether Find shows in place of the rest of the header, and whether it takes the keys as it
    /// next shows (as it opens).
    finding: bool,
    focus_find: bool,
    renaming: Option<Renaming>,
    /// Whether the Entities panel has the keys: from a press in it until a press elsewhere.
    keys: bool,
    /// The last value refused of each field of the picked entity.
    refusals: Vec<Refusal>,
    log: Log,
    /// The serial of the world the log, pick and open rows are of, as found when it attached or
    /// at the last tick or restore. At the next, another serial puts them aside.
    world: Option<u64>,
    /// The log, pick and open rows of the world last left, which come back if it does.
    left: Option<Left>,
    /// Whether a SyncTest runs each tick again on a copy of the world (`--sync-test`), as the run
    /// said when the Inspector attached or picked up from a restore.
    sync_tested: bool,
    /// When ticks run again: a copy of the log as the last tick with commands started, which the
    /// tick runs again with on the world copy ([`swapped`](Layer::swapped)).
    before: Option<Log>,
    /// The log of the world left when a SyncTest swapped another in, by serial: the world's while
    /// its tick runs again on a copy, then the copy's, until the next.
    aside: Option<(u64, Log)>,
    /// Why the last operation, undo or redo was refused, until the next edit, operation, undo or
    /// redo.
    not_done: Option<String>,
    /// What the panels and the link did since the last tick, which makes it.
    queued: Queue,
    /// Whose edit goes on, if a set was queued since the last seal, operation, undo or redo,
    /// which end it: a seal queued otherwise would end none.
    open: Option<Gesture>,
    /// What it shares with its links.
    link: Rc<RefCell<Linked>>,
    remote: Option<Remote>,
}

/// What the Inspector had of a world it left, by serial.
struct Left {
    world: u64,
    log: Log,
    picked: Option<EntityId>,
    others: Vec<EntityId>,
    expanded: HashSet<EntityId>,
}

/// The name being typed into an entity's row, from F2 or its row's Rename until Enter or a click
/// elsewhere names it, or Escape or another pick leaves it.
struct Renaming {
    id: EntityId,
    name: String,
    /// Whether its field is yet to take the keys, as it does when first drawn, its text all
    /// selected.
    focus: bool,
}

/// Whose gesture an edit going on is: the panels', which their input ends, or the link's, which
/// its seal ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gesture {
    Panels,
    Link,
}

const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Z);

/// The Edit menu shows the first.
const REDO: [KeyboardShortcut; 2] = [
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), egui::Key::Z),
    KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Y),
];

/// As Hazel's Ctrl+D.
const DUPLICATE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::D);

/// The Entities panel's own keys while it has them, as other editors' hierarchies take them.
const FRAME: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, egui::Key::F);
const RENAME: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, egui::Key::F2);
const DELETE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, egui::Key::Delete);

#[derive(Clone, Copy)]
enum Step {
    Undo,
    Redo,
}

/// What a frame's input, in the panels' windows, says of the edits: a press starts one, a release
/// ends one, a widget having the pointer or keys keeps one going, and keys may ask for a step or
/// a duplicate.
#[derive(Default)]
struct Cues {
    pressed: bool,
    released: bool,
    held: bool,
    step: Option<Step>,
    duplicate: bool,
}

impl Cues {
    /// Adds `egui`'s input as the drawing window has it, a torn-off panel's or the main one. Its
    /// keys ask for a step or duplicate unless a text field there has them, which undo its text.
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

/// A field to set, found as the Inspector panel draws, set at the next tick.
#[derive(Clone)]
struct Edit {
    component: String,
    path: String,
    value: Value,
}

/// What a pass of the panels found to do, for [`Inspector::queue`]: widgets' sets of the picked
/// entity's fields, menu and Ctrl+D operations, input cues, and the Edit menu's step.
#[derive(Default)]
struct Pass {
    sets: Vec<Edit>,
    /// Creates, duplicates, deletes, adds, removes, reverts and unlinks.
    operations: Vec<Command>,
    /// The entities the Entities menus chose to save as prefabs: a remote Inspector's alone, per
    /// `Inspected::saves_prefabs`.
    saves: Vec<EntityId>,
    cues: Cues,
    chosen: Option<Step>,
}

impl Layer for Inspector {
    /// Knows the world it shows from the start: a stopped run may get edits before its first
    /// tick.
    fn attach(&mut self, cx: &mut Context) {
        self.sync_tested = cx.sync_tested();
        if self.remote.is_none() {
            self.world = Some(cx.world().serial());
        }
    }

    /// Goes to the world now in place, if another, at once, not at the next tick. Called when:
    ///
    /// - The tool API's `stop` brings back the world as edited. Its log, pick and open rows come
    ///   back; the next tick may be the next copy's and would drop them.
    /// - The API's `scene new` or `scene open` renews the world.
    /// - A run restored from a snapshot starts.
    /// - A lockstep joiner's world gives way to its host's.
    fn restored(&mut self, cx: &mut Context) {
        self.sync_tested = cx.sync_tested();
        let serial = cx.world().serial();
        if self.remote.is_none() && self.world != Some(serial) {
            self.swap_to(serial);
        }
    }

    /// Moves its log to the world in place when a SyncTest swaps a copy in for the tick's rerun.
    /// The copy takes a copy of the log as the tick started, so its undos and redos go as they
    /// went in the world. The world takes its log back after; the copy's goes aside until the
    /// next. The pick, open rows and edit going on stay: the entities they name are the same in
    /// both.
    fn swapped(&mut self, cx: &mut Context) {
        let serial = cx.world().serial();
        let left = self.world.filter(|&world| world != serial);
        let Some(left) = left.filter(|_| self.remote.is_none()) else {
            return;
        };
        let log = match self.aside.take() {
            Some((world, log)) if world == serial => log,
            _ => {
                let mut before = self.before.take().unwrap_or_default();
                before.rebind(left, serial);
                before
            }
        };
        self.aside = Some((left, mem::replace(&mut self.log, log)));
        self.world = Some(serial);
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
        // The log as the tick starts, for running again on a copy of the world.
        let again = self.sync_tested && !self.queued.0.is_empty();
        self.before = again.then(|| self.log.copied());
        let mut queued = mem::take(&mut self.queued.0);
        for command in queued.drain(..) {
            self.run(world, command);
        }
        // Kept for its memory.
        self.queued.0 = queued;
        self.link.borrow_mut().queued = false;
        self.share_picks();
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.queued)
    }

    fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
        let _zone = gg_core::zone!("inspector");
        // Apart, so it's what the panels show while they change the rest.
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
            // Again: the Entities panel may just have changed the pick.
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
        // Every pass, even one egui discards, which alone had the frame's input, and empty ones,
        // whose cues end an edit.
        match &mut remote {
            Some(remote) => remote.send(pass, self),
            None => self.queue(pass),
        }
        self.share_picks();
        let linked = &mut *self.link.borrow_mut();
        linked.placed = remote
            .as_ref()
            .and_then(|remote| remote.placed(self.picked));
        linked.along = remote
            .as_ref()
            .and_then(|remote| remote.along(&linked.picks));
        self.remote = remote;
    }
}

impl Inspector {
    /// A link for another layer to pick and edit like the panels. Each is a clone of one the
    /// Inspector shares.
    pub fn link(&self) -> InspectorLink {
        InspectorLink(self.link.clone())
    }

    /// A remote Inspector: shows the world of the game its link reaches
    /// ([`InspectorLink::reach`]), not the app's own. It edits it as the game's client `client`,
    /// the author of the edits its Edit menu undoes and redoes.
    pub fn remote(client: &str) -> Self {
        Self {
            remote: Some(Remote::new(client)),
            ..Self::default()
        }
    }

    /// What waits for the next tick: queued sets of the picked entity's fields, then the link's
    /// since the last pass (queued after them); queued adds and removes; and the edits the log
    /// would undo and redo next.
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

    /// Makes the pick the link asked for since the last pass, before the panels draw.
    ///
    /// - A living entity's ID picks it and opens the rows above it, as Find does.
    /// - An ID not yet known to be alive, as of an entity a game just made, picks it too. A remote
    ///   Inspector opens the rows above it as its read comes.
    /// - Any other ID, or none, unpicks.
    ///
    /// A debug build first checks the link's sets against the world's types (the link has none,
    /// per [`InspectorLink::set`]) once it shows the world: a game's, once its types are read.
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
        let alive = |id: &EntityId| world.alive(*id) != Some(false);
        match pick {
            Picking::Only(ids) => {
                let ids: Vec<EntityId> = ids.into_iter().filter(alive).collect();
                for &id in &ids {
                    self.open_ancestors(world, id);
                }
                self.pick_all(ids);
            }
            Picking::Toggle(id) if alive(&id) => {
                self.toggle(id);
                self.open_ancestors(world, id);
            }
            Picking::Toggle(_) => {}
        }
    }

    /// The Edit menu: Undo and Redo, each naming its edit from `pending`. Returns the one clicked.
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

    /// The Entity menu; the operation chosen goes in `operations`.
    ///
    /// - Create Empty makes a root.
    /// - While an entity is picked, Unpick unpicks all.
    /// - While the active one is alive, Create Empty Child makes its child.
    /// - While one picked is alive, Duplicate (like Ctrl+D) and Delete act on those picked and
    ///   their descendants, each as one edit.
    fn entity_menu(
        &mut self,
        world: &dyn Inspected,
        ui: &mut egui::Ui,
        operations: &mut Vec<Command>,
    ) {
        let live = self.picked.filter(|&id| world.alive(id) == Some(true));
        let uppermost = self.uppermost(world);
        let any = !uppermost.is_empty();
        if ui.button("Create Empty").clicked() {
            operations.push(empty(None));
        }
        let child = ui.add_enabled(live.is_some(), egui::Button::new("Create Empty Child"));
        let shortcut = ui.ctx().format_shortcut(&DUPLICATE);
        let duplicate = egui::Button::new("Duplicate").shortcut_text(shortcut);
        let duplicate = ui.add_enabled(any, duplicate);
        let delete = ui.add_enabled(any, egui::Button::new("Delete"));
        // A despawned pick unpicks too: it stays picked for an undo to bring it back.
        let unpick = ui.add_enabled(self.picked.is_some(), egui::Button::new("Unpick"));
        if unpick.clicked() {
            self.unpick();
        }
        if let Some(id) = live.filter(|_| child.clicked()) {
            operations.push(empty(Some(id)));
        }
        if any && duplicate.clicked() {
            operations.push(Command::Duplicate(uppermost.clone()));
        }
        if any && delete.clicked() {
            operations.push(Command::Delete(uppermost));
        }
    }

    /// Has `pass` duplicate the picked entities alive in `world`, if its input's Ctrl+D asks.
    fn duplicate_picked(&self, world: &dyn Inspected, pass: &mut Pass) {
        let uppermost = self.uppermost(world);
        if pass.cues.duplicate && !uppermost.is_empty() {
            pass.operations.push(Command::Duplicate(uppermost));
        }
    }

    /// Queues a pass of the panels for the next tick, in order:
    ///
    /// 1. What the link did since the last pass (its events came before the pass's input): its
    ///    sets as one edit from its gesture's first set to its seal, and its creates, each
    ///    ending the edit going on.
    /// 2. The pass's sets of the picked entity's fields, one edit from a pointer press to its
    ///    release, or while a widget has the keys, so a drag or typed number is one edit.
    /// 3. Its operations, each ending the edit going on.
    /// 4. The Edit menu's or a key's step.
    ///
    /// Each gesture's first set ends the other's edit going on; the pass's input ends only its
    /// own.
    fn queue(&mut self, pass: Pass) {
        // Its saves are a remote Inspector's alone.
        let Pass {
            sets,
            operations,
            cues,
            chosen,
            saves: _,
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
                Linking::Create(components) => {
                    let parent = None;
                    self.queued.0.push(Command::Create { parent, components });
                    self.open = None;
                }
                Linking::Seal => self.seal(Gesture::Link),
            }
        }
        // Kept for its memory.
        self.link.borrow_mut().pending = linked;
        // Before this pass's sets: a press begins another edit, even while a text field has the
        // keys.
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

    /// Queues a seal if `gesture`'s edit goes on, so an idle pass queues nothing. Only a set the
    /// log takes begins an edit; seals, operations, undos and redos end it.
    fn seal(&mut self, gesture: Gesture) {
        if self.open == Some(gesture) {
            self.queued.0.push(Command::Seal);
            self.open = None;
        }
    }

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
            Command::Delete(entities) => {
                let deletes = entities
                    .into_iter()
                    .map(|entity| Operation::Delete { entity });
                self.operate_all(world, deletes.collect());
            }
            Command::Duplicate(entities) => {
                let duplicates = entities.into_iter().map(|entity| Operation::Duplicate {
                    entity,
                    copies: Vec::new(),
                });
                self.operate_all(world, duplicates.collect());
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
            Command::Revert(entity, component, path) => {
                self.forget(entity, &component);
                let component = component.into();
                let revert = Operation::Revert {
                    entity,
                    component,
                    path,
                };
                self.operate(world, revert);
            }
            Command::Unlink(entity) => self.operate(world, Operation::Unlink { entity }),
            Command::Reparent(entity, parent) => {
                let operations = world.reparent_in_place(entity, parent);
                self.operate_all(world, operations);
            }
            // A set of the name it has, or an add of one, which the log undoes alike.
            Command::Rename(entity, name) => {
                let operation = match Inspected::name(&*world, entity) {
                    Some(_) => Operation::Set {
                        entity,
                        component: "Name".into(),
                        path: "0".to_owned(),
                        value: Value::String(name),
                    },
                    None => Operation::Add {
                        entity,
                        component: "Name".into(),
                        value: Name(name).to_value(),
                    },
                };
                self.operate(world, operation);
            }
        }
    }

    /// [`operate_all`](Self::operate_all) of one.
    fn operate(&mut self, world: &mut World, operation: Operation) {
        self.operate_all(world, vec![operation]);
    }

    /// Applies `operations` through the log as one edit, ending the edit going on. Then picks the
    /// entities its creates made and its duplicates' copies, the last active, opening their
    /// ancestors' rows. The first operation's refusal shows above the components and is logged.
    /// None do nothing.
    fn operate_all(&mut self, world: &mut World, operations: Vec<Operation>) {
        let Some(first) = operations.first() else {
            return;
        };
        let (entity, said) = (first.entity().map(display), first.to_string());
        if let Err(error) = self.log.apply_all(world, operations) {
            warn!(code = error.code(), entity, operation = %said, "{error}");
            self.not_done = Some(error.to_string());
            return;
        }
        self.not_done = None;
        let done = self.log.undoable().unwrap_or_default();
        let made: Vec<EntityId> = (done.iter())
            .filter_map(|done| match done {
                Operation::Create { .. } | Operation::Duplicate { .. } => done.entity(),
                _ => None,
            })
            .collect();
        if made.is_empty() {
            return;
        }
        for &id in &made {
            self.open_ancestors(&*world, id);
        }
        self.pick_all(made);
    }

    /// Forgets the values `entity`'s `component` refused, if picked: an add or remove makes it
    /// anew.
    fn forget(&mut self, entity: EntityId, component: &str) {
        if self.picked == Some(entity) {
            self.refusals
                .retain(|refusal| refusal.component != component);
        }
    }

    fn pick(&mut self, id: EntityId) {
        self.pick_all([id]);
    }

    /// Picks `ids` alone, in order, the last the active one. None unpicks.
    fn pick_all(&mut self, ids: impl IntoIterator<Item = EntityId>) {
        let mut ids: Vec<EntityId> = ids.into_iter().collect();
        let active = ids.pop();
        self.others.clear();
        for id in ids {
            if Some(id) != active && !self.others.contains(&id) {
                self.others.push(id);
            }
        }
        self.activate(active);
    }

    fn unpick(&mut self) {
        self.others.clear();
        self.activate(None);
    }

    /// Makes `id` the active pick, forgetting the previous one's refusals if it differs.
    fn activate(&mut self, id: Option<EntityId>) {
        if self.picked != id {
            self.picked = id;
            self.refusals.clear();
        }
    }

    /// Adds `id` as the active pick, moving it from `others` if it's there.
    fn add(&mut self, id: EntityId) {
        self.others.retain(|&other| other != id);
        if let Some(active) = self.picked.filter(|&active| active != id) {
            self.others.push(active);
        }
        self.activate(Some(id));
    }

    /// Adds `id` as the active pick, or takes it out if picked; the one picked before replaces it
    /// as active.
    fn toggle(&mut self, id: EntityId) {
        if self.picked == Some(id) {
            let before = self.others.pop();
            self.activate(before);
        } else if self.others.contains(&id) {
            self.others.retain(|&other| other != id);
        } else {
            self.add(id);
        }
    }

    /// Every entity picked, in pick order, the active one last.
    fn picks(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.others.iter().copied().chain(self.picked)
    }

    fn is_picked(&self, id: EntityId) -> bool {
        self.picked == Some(id) || self.others.contains(&id)
    }

    /// The picked entities alive in `world` with no picked ancestor, in pick order: what a delete
    /// or duplicate of all acts on, one under another going or copied with it.
    fn uppermost(&self, world: &dyn Inspected) -> Vec<EntityId> {
        let picks = self.picks().filter(|&id| world.alive(id) == Some(true));
        picks.filter(|&id| !self.under_a_pick(world, id)).collect()
    }

    /// Whether an ancestor of `id` is picked, stopping at a repeat: a hierarchy built from reads
    /// of a game's world at two moments may loop.
    fn under_a_pick(&self, world: &dyn Inspected, id: EntityId) -> bool {
        let mut met = HashSet::from([id]);
        let mut up = world.parent(id);
        while let Some(parent) = up.filter(|&parent| met.insert(parent)) {
            if self.is_picked(parent) {
                return true;
            }
            up = world.parent(parent);
        }
        false
    }

    fn share_picks(&self) {
        let linked = &mut *self.link.borrow_mut();
        linked.picked = self.picked;
        linked.picks.clear();
        linked.picks.extend(self.picks());
    }

    /// Goes to the other world with `serial`. Its entities aren't those the pick, rows, log,
    /// queued commands and the link's requests since the last pass name, though IDs may match, as
    /// when a scene reopens. The pick, rows and log go aside for their world to find if it comes
    /// back. Those set aside earlier come back if they're this world's, else go: it never will.
    fn swap_to(&mut self, serial: u64) {
        let left = self.world.map(|world| Left {
            world,
            log: mem::take(&mut self.log),
            picked: self.picked,
            others: mem::take(&mut self.others),
            expanded: mem::take(&mut self.expanded),
        });
        self.unpick();
        if let Some(back) = self.left.take().filter(|left| left.world == serial) {
            self.log = back.log;
            self.picked = back.picked;
            self.others = back.others;
            self.expanded = back.expanded;
        }
        self.left = left;
        self.world = Some(serial);
        self.not_done = None;
        self.not_found = None;
        self.renaming = None;
        self.open = None;
        self.queued.0.clear();
        let linked = &mut *self.link.borrow_mut();
        linked.pick = None;
        linked.pending.clear();
    }

    /// Opens `id`'s ancestors' rows so the tree shows its own, stopping at a repeat: a hierarchy
    /// built from reads of a game's world at two moments may loop.
    fn open_ancestors(&mut self, world: &dyn Inspected, id: EntityId) {
        let mut met = HashSet::from([id]);
        let mut up = world.parent(id);
        while let Some(parent) = up.filter(|&parent| met.insert(parent)) {
            self.expanded.insert(parent);
            up = world.parent(parent);
        }
    }

    /// The Inspector panel: the active pick's components (above them, how many are picked, if
    /// several), each under a collapsing header with its fields' widgets and a removal menu
    /// beside it; below, an add menu. Each is drawn as `pending` will leave it. Changed fields,
    /// menu operations and the drawing window's input cues go in `pass`.
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
        if !self.others.is_empty() {
            let n = self.others.len() + 1;
            ui.label(format!("{n} entities picked: the last one picked is below"));
        }
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
        let instance = world.instance(id);
        if let Some(instance) = &instance {
            instance_line(world, ui, id, instance, &mut pass.operations);
        }
        // The entity's own ID scope: typing into one entity's field never lands in the next's, and
        // widgets keep their IDs, and drags and typing, as the labels above come and go.
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
                let overrides = instance.as_ref().and_then(|i| i.overrides.get(component));
                let overridden = overrides.map_or(&[][..], Vec::as_slice);
                let added =
                    (instance.as_ref()).is_some_and(|i| i.added.iter().any(|a| a == component));
                let prefab = instance.as_ref().map_or("", |instance| &instance.prefab);
                let title = match overrides.is_some() || added {
                    true => egui::RichText::new(component).strong(),
                    false => egui::RichText::new(component),
                };
                let header = egui::CollapsingHeader::new(title)
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
                            overridden,
                            prefab,
                            operations: &mut pass.operations,
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
                    let (rect, reverts) = (header.rect, overrides.is_some());
                    component_menu(ui, rect, id, component, reverts, &mut pass.operations);
                }
                let said = match (added, overrides) {
                    (true, _) => Some(format!("Added to this instance: {prefab} gives it none")),
                    (false, Some(_)) => Some(format!(
                        "Differs from {prefab}: its menu reverts it whole, a field's context menu \
                         that field"
                    )),
                    (false, None) => None,
                };
                if said.is_some() {
                    mark(ui, header.rect);
                }
                let doc = &types.info(index).doc;
                match (said, doc.is_empty()) {
                    (Some(said), true) => {
                        header.on_hover_text(said);
                    }
                    (Some(said), false) => {
                        header.on_hover_text(format!("{said}\n\n{doc}"));
                    }
                    (None, false) => {
                        header.on_hover_text(doc.as_ref());
                    }
                    (None, true) => {}
                }
            }
            self.add_menu(world, ui, id, pending, &mut pass.operations);
        });
    }

    /// The Add Component menu below `id`'s components: the types it can be given, in registration
    /// order, each with its doc on hover. Not listed: those it has, a parent (the world gives
    /// it), those opaque as a whole (handles, which code makes) and those with an add `pending`.
    /// A type whose default an add would refuse shows disabled, saying why on hover. Defaults are
    /// made only while it's open. The add chosen goes in `operations`.
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
    /// Sets the field `edit` changed of `entity`, the entity picked when it was made, as part of
    /// the edit going on. If refused while it's still picked, keeps the refusal (see
    /// [`refuse`](Self::refuse)).
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
                self.refuse(&*world, id, component, path, error.to_string());
            }
        }
    }

    /// Keeps `message`, why a set of `path` of the picked entity's `component` was refused,
    /// beside the field, or above the components if `world` says the entity lacks the component
    /// (as a link's set may be), whose field isn't drawn.
    fn refuse(
        &mut self,
        world: &dyn Inspected,
        id: EntityId,
        component: String,
        path: String,
        message: String,
    ) {
        if world.components(id).contains(&component.as_str()) {
            self.refusals.push(Refusal {
                component,
                path,
                message,
            });
        } else {
            self.not_done = Some(message);
        }
    }
}

/// [`Parent`](crate::Parent)'s name: the world gives an entity one as it's made a child, so the
/// Inspector neither adds nor removes one.
const PARENT: &str = "Parent";

/// The radius of the three dots across a component's menu button, and their spacing: about
/// Inter's ellipsis dots at egui's 13 points of button text.
const DOT: f32 = 1.0;
const DOTS_APART: f32 = 4.0;

/// The menu of `component` at the right end of its `header`'s row. Remove Component removes it
/// from `id`. If it `reverts` (differs from the entity's prefab's), Revert to Prefab resets it
/// whole. The operation goes in `operations`.
///
/// A framed header is as wide as `ui`; the menu, made after it, covers its end and takes the
/// clicks there. It's in a child of `ui`, leaving the cursor where the header's body left it.
fn component_menu(
    ui: &mut egui::Ui,
    header: egui::Rect,
    id: EntityId,
    component: &str,
    reverts: bool,
    operations: &mut Vec<Command>,
) {
    let size = egui::Vec2::splat(ui.spacing().interact_size.y);
    // `ui`'s end, not the header's, which ends at its name where it isn't framed.
    let at = egui::pos2(shown_right(ui) - size.x, header.top());
    // Laid out from its right, so the button ends there.
    let child = egui::UiBuilder::new()
        .id_salt(("component menu", component))
        .max_rect(egui::Rect::from_min_size(at, size))
        .layout(egui::Layout::right_to_left(egui::Align::Center));
    let mut menu = ui.new_child(child);
    // Three dots across the middle in the lit text color, centered both ways: a font's ellipsis
    // sits on its baseline, ~4 points below the row's middle, and neither Inter nor egui's
    // proportional fonts have the midline `⋯`.
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
        if reverts && ui.button("Revert to Prefab").clicked() {
            let (component, path) = (component.to_owned(), String::new());
            operations.push(Command::Revert(id, component, path));
        }
        if ui.button("Remove Component").clicked() {
            operations.push(Command::Remove(id, component.to_owned()));
        }
    });
}

/// Says which instance of a prefab `id` is of, as `instance` has it:
///
/// - An outermost instance's root: by its prefab, with Unlink, whose operation goes in
///   `operations`.
/// - A root of an instance inside another's prefab: by its prefab and the outer instance's. Only
///   an unlink of the outer takes it.
/// - An entity an instance's prefab gives: by that instance's prefab and root.
fn instance_line(
    world: &dyn Inspected,
    ui: &mut egui::Ui,
    id: EntityId,
    instance: &gg_api::Instanced,
    operations: &mut Vec<Command>,
) {
    let root = instance
        .root
        .parse::<EntityId>()
        .ok()
        .filter(|&root| root != id);
    let outer = root.map(|root| match world.name(root) {
        Some(name) => format!("{} whose root is {name}, entity {root}", instance.prefab),
        None => format!("{} whose root is entity {root}", instance.prefab),
    });
    ui.horizontal_wrapped(|ui| match (&instance.link, outer) {
        (Some(link), None) => {
            ui.label(format!("Instance of {link}"));
            let unlink = ui.button("Unlink").on_hover_text(
                "Makes the instance's entities its own, under their IDs: they follow the prefab's \
                 file no more",
            );
            if unlink.clicked() {
                operations.push(Command::Unlink(id));
            }
        }
        (Some(link), Some(outer)) => {
            ui.label(format!("Instance of {link}, in the instance of {outer}"));
        }
        (None, outer) => {
            let outer = outer.unwrap_or_else(|| instance.prefab.clone());
            ui.label(format!("Of the instance of {outer}"));
        }
    });
}

/// Marks a field's label or component's header at `rect` as differing from the prefab's, or as a
/// component it doesn't give: a bar of the selection's color along its left edge, as Unity's
/// Inspector marks overrides. It's just left of it, or over its edge where that's cut off, as a
/// header's is (panel-wide).
pub(super) fn mark(ui: &egui::Ui, rect: egui::Rect) {
    let left = (rect.left() - 4.0).max(ui.clip_rect().left());
    let bar = egui::Rect::from_min_max(
        egui::pos2(left, rect.top()),
        egui::pos2(left + 2.0, rect.bottom()),
    );
    ui.painter()
        .rect_filled(bar, 0.0, ui.visuals().selection.bg_fill);
}

/// Where what `ui` shows ends on the right: its end, but no further than where it's cut off
/// (rows wider than the panel, as a narrow Inspector's axes are, widen it), and short of the
/// strip where a scroll bar floats over the contents and takes the clicks.
fn shown_right(ui: &egui::Ui) -> f32 {
    let scroll = &ui.spacing().scroll;
    let bar = scroll.bar_width + scroll.bar_outer_margin;
    ui.max_rect().right().min(ui.clip_rect().right()) - bar
}

/// The command that creates an empty entity, the last of `parent`'s children or a root, with a
/// name that says so and a transform that leaves it where its parent is.
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

/// `id`'s children by ID, as "Children: 5f3a9c0e12b47d68, 0c41b2e79d6a3f15": the first 16, then
/// how many more there are. `None` if it has none.
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

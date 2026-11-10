//! A remote Inspector's world: a game's in another process, as the tool API gives it, which the
//! panels show as they show their own, and edit through the API, as the game's client.
//!
//! One connection, on a thread of its own, carries a request at a time, each answered in turn, so
//! that what's read after an edit's answer shows the edit. The types go first, once, and again if
//! a type not described comes, since values are read and written by them; then edits, but for a
//! read of the picked entity between two of them, as during a drag; then the picked entity, every
//! 50 ms; the list of entities, 10,000 a page, every quarter of a second; and the client's edits in
//! words, for the Edit menu, after each edit and each list. A set waits, drawn as it will be, until
//! an `entity` answer after its own; an add or a remove likewise. The game answers a request as its
//! frame starts, so a request takes a frame of the game's, and of the editor's, which a game run
//! `--embedded` paces its frames by.
//!
//! The panels read the game's whole list of entities every quarter of a second, which costs its
//! main thread a few steps an entity, and keeps the list in memory; reading only the rows a panel
//! shows would bound both by its height, which a game of tens of thousands of entities would want.

pub(super) use imp::Remote;

mod imp {
    use std::borrow::Cow;
    use std::collections::{HashMap, HashSet, VecDeque};
    use std::hash::{BuildHasher, RandomState};
    use std::mem;
    use std::ops::Range;
    use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
    use std::thread;
    use std::time::{Duration, Instant};

    use gg_api::{Client, Failure, Session};
    use gg_core::math::Mat4;
    use serde::Deserialize;
    use serde_json::{Map, Value as JsonValue, json};
    use tracing::field::display;
    use tracing::{info, warn};

    use super::super::commands::Command;
    use super::super::inspected::{Inspected, Pending};
    use super::super::link::Linking;
    use super::super::{Edit, Gesture, Inspector, Pass, Placed, Step};
    use crate::api::json::{self, Remote as Names};
    use crate::reflect::{Reflect, Type, Types, Value};
    use crate::{EntityId, Name, Pose, Transform};

    /// How long the connection waits to connect. It waits for each answer as long as it takes: a
    /// script stopped in the game holds the answers for as long as it's debugged, and `gg run` ends
    /// a game that hangs, which closes the connection.
    const CONNECT: Duration = Duration::from_secs(10);

    /// How often the picked entity, and the list of entities, are read again.
    pub(super) const ENTITY_EVERY: Duration = Duration::from_millis(50);
    pub(super) const LIST_EVERY: Duration = Duration::from_millis(250);

    /// The most entities a page of the list asks for: as many as `entities` lists.
    pub(super) const PAGE: usize = 10_000;

    /// A game's world, as the tool API gives it, with what's been asked of it.
    pub(in super::super) struct Remote {
        /// The client it is to the game, the author of its edits.
        client: String,
        connection: Option<Connection>,
        /// Why the game's world isn't shown, if it isn't, as while it's reached: `None` once it's
        /// connected; and why the last read of each kind failed, if it did, until one of that kind
        /// doesn't, by [`Of`].
        state: Option<String>,
        troubles: [Option<String>; 3],
        /// The types the game described, and its component types, in the order they registered,
        /// each with what an add gives, or why there's nothing; `typed` once they're read.
        types: Types,
        components: Vec<(String, Result<Value, String>)>,
        typed: bool,
        /// The entities as the last list read gave them, and the pages of the one being read.
        list: List,
        reading: Option<Vec<gg_api::Listed>>,
        /// The picked entity, as the last `entity` answer of it gave it while it stayed picked; the
        /// entity the panels have picked, as of the last pass, whose answer may not have come; and
        /// the one the link asks them to pick, as of this pass's update, which the list may not
        /// have yet either.
        picked: Option<(EntityId, Option<Known>)>,
        wanted: Option<EntityId>,
        linking: Option<EntityId>,
        /// The edits to send, in order, and the request waiting for its answer.
        queued: VecDeque<Ask>,
        sent: Option<Ask>,
        /// Whether the request sent last was an edit, after which a read of the picked entity
        /// that's due goes first.
        edited: bool,
        /// When the picked entity, and the list, were last asked for: `None` to ask at once.
        entity_at: Option<Instant>,
        list_at: Option<Instant>,
        /// When the types were last asked for, which a read that failed asks again for no sooner
        /// than a list.
        types_at: Option<Instant>,
        /// Whether the types, or the client's edits in words, are to be asked for.
        types_due: bool,
        edits_due: bool,
        /// The revision of the types last read, as `types` answered it, which a list of another
        /// has them asked for again, as a script's reload changes them.
        revision: u64,
        /// Whose gesture goes on, and its number, which the client gives each of its own, from
        /// one drawn at random, so that another connection of its name, as after a game's
        /// restart, never continues one.
        open: Option<(Gesture, u64)>,
        gestures: u64,
        /// The sets made, until an `entity` answer after their own shows them, each numbered.
        waiting: Vec<Waiting>,
        /// The adds and removes asked for, likewise.
        changes: Vec<Changing>,
        made: u64,
        /// The edits the game's `undo` and `redo` of the client's would step through next, in
        /// words.
        undo: Option<String>,
        redo: Option<String>,
        /// How many times it has been reached, to a game or to none, as [`Placed::generation`]
        /// counts them.
        generation: u64,
    }

    /// The connection to the game: the requests going to its thread, and what the thread hears.
    struct Connection {
        asks: Sender<(String, JsonValue)>,
        heard: Receiver<Heard>,
    }

    /// What the connection's thread hears: that it connected, or couldn't, or an answer.
    enum Heard {
        Connected,
        Failed(Failure),
        Answer(Result<String, Failure>),
    }

    /// The kinds of reads of the game's world, each of which says why it failed until one doesn't.
    #[derive(Clone, Copy)]
    enum Of {
        Types,
        List,
        Entity,
    }

    /// A request, as what's to be done with its answer.
    enum Ask {
        Types,
        /// A page of the list of entities, from the one at `skip`.
        List {
            skip: usize,
        },
        Entity(EntityId),
        Edits,
        /// A save of the entity with this ID as a prefab, which the Entities panel chose.
        SavePrefab(EntityId),
        /// A set, its waiting one numbered `number`, of a gesture.
        Set {
            number: u64,
            id: EntityId,
            edit: Edit,
            gesture: u64,
        },
        /// An operation the menus chose, the entity picked as they chose it, and the parent of
        /// the entity it may make, whose rows open as it's picked; the change it makes, numbered.
        Operation {
            command: Command,
            picked: Option<EntityId>,
            parent: Option<EntityId>,
            number: u64,
        },
        Step(Step),
    }

    impl Ask {
        fn edits(&self) -> bool {
            matches!(
                self,
                Self::Set { .. } | Self::Operation { .. } | Self::Step(_) | Self::SavePrefab(_)
            )
        }
    }

    /// A set made, by its number, and whether the game has applied it.
    struct Waiting {
        number: u64,
        id: EntityId,
        edit: Edit,
        applied: bool,
    }

    /// An add, or a remove, of a component, as [`Waiting`] is a set.
    struct Changing {
        number: u64,
        id: EntityId,
        component: String,
        adds: bool,
        applied: bool,
    }

    /// The entities of a list read whole, in the order queries find them, and how to find each.
    #[derive(Default)]
    struct List {
        entities: Vec<Known>,
        by_id: HashMap<EntityId, usize>,
        roots: Vec<EntityId>,
        root_at: HashMap<EntityId, usize>,
        /// Whether one has been read since the game was reached.
        read: bool,
    }

    /// An entity as the game gave it: its name, parent and children, and its components' types, in
    /// the order they registered; with, for the picked entity, their values, or why one can't be
    /// read, where the space its transform is in sits, if that was given, and finite, and what of
    /// it differs from its prefab, if it's an instance's.
    struct Known {
        id: EntityId,
        name: Option<String>,
        parent: Option<EntityId>,
        children: Vec<EntityId>,
        components: Vec<String>,
        values: Vec<Result<Value, String>>,
        space: Option<Pose>,
        instance: Option<gg_api::Instanced>,
    }

    /// The entity `entity` answers, its `space` and `instance` as they came, which a game of an
    /// older engine doesn't give.
    #[derive(Deserialize)]
    struct Answered {
        name: Option<String>,
        parent: Option<String>,
        children: Vec<String>,
        components: Map<String, JsonValue>,
        #[serde(default)]
        space: Option<JsonValue>,
        #[serde(default)]
        instance: Option<gg_api::Instanced>,
    }

    impl Remote {
        pub(in super::super) fn new(client: &str) -> Self {
            Self {
                client: client.to_owned(),
                connection: None,
                state: Some(NOT_SERVING.to_owned()),
                troubles: Default::default(),
                types: Types::default(),
                components: Vec::new(),
                typed: false,
                list: List::default(),
                reading: None,
                picked: None,
                wanted: None,
                linking: None,
                queued: VecDeque::new(),
                sent: None,
                edited: false,
                entity_at: None,
                list_at: None,
                types_at: None,
                types_due: true,
                edits_due: true,
                revision: 0,
                open: None,
                // std seeds its hashers' keys from the OS's randomness, as the API's token is
                // drawn.
                gestures: RandomState::new().hash_one(0),
                waiting: Vec::new(),
                changes: Vec::new(),
                made: 0,
                undo: None,
                redo: None,
                generation: 0,
            }
        }

        /// Shows the game `session` names, connecting to it, or none: what was known of the game
        /// before goes, but the gesture's numbers, which go on, and the count of reaches, which
        /// goes up.
        fn reach(&mut self, session: Option<Session>) {
            let fresh = Self::new(&self.client);
            let (gestures, generation) = (self.gestures, self.generation);
            *self = fresh;
            self.gestures = gestures;
            self.generation = generation + 1;
            if let Some(session) = session {
                self.connection = Some(connect(session, self.client.clone()));
                self.state = Some("Reaching the game".to_owned());
            }
        }

        /// Takes in what the game answered since the last pass, changing `inspector`'s pick, open
        /// rows and refusals as it says; and first reaches the game its link names, if it names
        /// one since, and lets go of the read of the entity picked if the pick changed in the last
        /// pass, so that a read taken before the pick went elsewhere never shows once it's back.
        pub(in super::super) fn update(&mut self, inspector: &mut Inspector) {
            let reach = inspector.link.borrow_mut().reach.take();
            if let Some(session) = reach {
                self.reach(session);
            }
            if inspector.picked != self.wanted {
                self.picked = None;
            }
            while let Some(connection) = &self.connection {
                match connection.heard.try_recv() {
                    Ok(Heard::Connected) => self.state = None,
                    Ok(Heard::Failed(failure)) => {
                        self.state = Some(format!("Can't reach the game: {failure}"));
                        self.connection = None;
                    }
                    Ok(Heard::Answer(answer)) => {
                        let ask = self.sent.take().expect("an answer to what was sent");
                        self.answer(ask, answer, inspector);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.state = Some(CLOSED.to_owned());
                        self.connection = None;
                    }
                }
            }
            self.wanted = inspector.picked;
            self.linking = inspector.link.borrow().pick.flatten();
        }

        /// Takes in `answer`, to `ask`.
        fn answer(&mut self, ask: Ask, answer: Result<String, Failure>, inspector: &mut Inspector) {
            if let Err(failure) = &answer
                && matches!(failure.code.as_str(), "api.closed" | "api.timeout")
            {
                self.state = Some(format!("{CLOSED}: {failure}"));
                self.connection = None;
                return;
            }
            if ask.edits() {
                // What the edit changed, read at once.
                self.entity_at = None;
                self.list_at = None;
                self.edits_due = true;
            }
            match ask {
                Ask::Types => match answer.and_then(|json| read::<gg_api::Types>(&json)) {
                    Ok(given) => match json::described(&given) {
                        Ok(described) => {
                            self.types = described.types;
                            self.components = described.components;
                            self.typed = true;
                            self.types_due = false;
                            self.revision = given.revision;
                            self.troubles[Of::Types as usize] = None;
                            // Read again, by these.
                            self.entity_at = None;
                        }
                        Err(error) => {
                            let message = format!("Can't read the game's types: {error}");
                            self.trouble(Of::Types, error.code(), message);
                        }
                    },
                    Err(failure) => {
                        let message = format!("Can't read the game's types: {failure}");
                        self.trouble(Of::Types, &failure.code, message);
                    }
                },
                Ask::List { skip } => self.listed(skip, answer),
                Ask::Entity(id) => {
                    let read = |picked: &Option<(EntityId, Option<Known>)>| {
                        (picked.as_ref()).is_some_and(|(of, known)| *of == id && known.is_some())
                    };
                    let first = !read(&self.picked);
                    self.entity(id, answer, inspector.picked);
                    // The rows above the pick, as its first read gives them, which a pick of an
                    // entity the list hadn't yet, as the link's of one just made, couldn't open;
                    // and not at the reads after, so that they close.
                    if first && read(&self.picked) {
                        inspector.open_ancestors(&*self, id);
                    }
                }
                Ask::Edits => {
                    let history = answer.and_then(|json| read::<gg_api::History>(&json));
                    let authored = history.ok().and_then(|history| {
                        let mut authors = history.authors.into_iter();
                        authors.find(|authored| authored.author == self.client)
                    });
                    let first = |edits: Option<&Vec<String>>| edits?.first().cloned();
                    self.undo = authored.as_ref().and_then(|a| first(a.done.last()));
                    self.redo = authored.as_ref().and_then(|a| first(a.undone.last()));
                }
                Ask::Set {
                    number, id, edit, ..
                } => {
                    inspector.not_done = None;
                    let shown = inspector.picked == Some(id);
                    if shown {
                        inspector.refusals.retain(|refusal| {
                            refusal.component != edit.component || refusal.path != edit.path
                        });
                    }
                    match answer {
                        Ok(_) => {
                            let set = self.waiting.iter_mut().find(|set| set.number == number);
                            if let Some(set) = set {
                                set.applied = true;
                            }
                        }
                        Err(failure) => {
                            self.waiting.retain(|set| set.number != number);
                            warn!(
                                code = failure.code.as_str(),
                                entity = %id,
                                component = edit.component.as_str(),
                                path = edit.path.as_str(),
                                "{failure}"
                            );
                            if shown {
                                let Edit {
                                    component, path, ..
                                } = edit;
                                inspector.refuse(&*self, id, component, path, failure.message);
                            }
                        }
                    }
                }
                Ask::Operation {
                    command,
                    picked,
                    parent,
                    number,
                } => match answer.and_then(|json| read::<gg_api::Applied>(&json)) {
                    Ok(applied) => {
                        inspector.not_done = None;
                        if let Command::Add(id, component, _) | Command::Remove(id, component) =
                            &command
                        {
                            inspector.forget(*id, component);
                        }
                        let change = self.changes.iter_mut().find(|c| c.number == number);
                        if let Some(change) = change {
                            change.applied = true;
                        }
                        // The entity made, picked unless the pick changed since.
                        let made = applied.entities.first().and_then(|id| id.parse().ok());
                        if let Some(made) = made.filter(|_| inspector.picked == picked) {
                            inspector.pick(made);
                            // From the parent it was made under, which the list may not have yet.
                            if let Some(parent) = parent {
                                inspector.expanded.insert(parent);
                                inspector.open_ancestors(&*self, parent);
                            }
                        }
                    }
                    Err(failure) => {
                        self.changes.retain(|change| change.number != number);
                        // What it was of, as the local Inspector logs an operation refused.
                        let (operation, entity, component) = match &command {
                            Command::Create { .. } => ("create", None, None),
                            &Command::Duplicate(id) => ("duplicate", Some(id), None),
                            &Command::Delete(id) => ("delete", Some(id), None),
                            Command::Add(id, component, _) => ("add", Some(*id), Some(component)),
                            Command::Remove(id, component) => {
                                ("remove", Some(*id), Some(component))
                            }
                            Command::Revert(id, component, _) => {
                                ("revert", Some(*id), Some(component))
                            }
                            &Command::Unlink(id) => ("unlink", Some(id), None),
                            &Command::Reparent(id, _) => ("reparent", Some(id), None),
                            &Command::Rename(id, _) => ("rename", Some(id), None),
                            Command::Set(..) | Command::Seal | Command::Step(_) => {
                                unreachable!("the menus choose no sets, seals or steps")
                            }
                        };
                        let (entity, parent) = (entity.map(display), parent.map(display));
                        warn!(
                            code = failure.code.as_str(),
                            operation,
                            entity,
                            parent,
                            component = component.map(String::as_str),
                            "{failure}"
                        );
                        inspector.not_done = Some(failure.message);
                    }
                },
                Ask::Step(_) => {
                    inspector.not_done = answer.err().map(|failure| failure.message);
                }
                Ask::SavePrefab(id) => {
                    match answer.and_then(|json| read::<gg_api::SavedPrefab>(&json)) {
                        Ok(saved) => {
                            inspector.not_done = None;
                            info!(entity = %id, path = saved.path, "Saved as a prefab");
                        }
                        Err(failure) => {
                            let code = failure.code.as_str();
                            warn!(code, operation = "save as a prefab", entity = %id, "{failure}");
                            inspector.not_done = Some(failure.message);
                        }
                    }
                }
            }
        }

        /// Takes in a page of the list, from the entity at `skip`, which completes it once it has
        /// them all, as the page counts them.
        fn listed(&mut self, skip: usize, answer: Result<String, Failure>) {
            let page = match answer.and_then(|json| read::<gg_api::EntityList>(&json)) {
                Ok(page) => page,
                Err(failure) => {
                    self.reading = None;
                    let message = format!("Can't list the game's entities: {failure}");
                    self.trouble(Of::List, &failure.code, message);
                    return;
                }
            };
            self.types_due |= page.types != self.revision;
            let read = self.reading.get_or_insert_with(Vec::new);
            let more = !page.entities.is_empty() && skip + page.entities.len() < page.total;
            read.extend(page.entities);
            if more {
                return;
            }
            let read = self.reading.take().expect("being read");
            let mut list = List {
                read: true,
                ..List::default()
            };
            // An entity read twice, as entities coming and going between pages may move one from
            // one page to the next, is listed once.
            let mut seen = HashSet::new();
            for listed in read {
                let Ok(id) = listed.id.parse::<EntityId>() else {
                    continue;
                };
                if !seen.insert(id) {
                    continue;
                }
                let ids = |ids: Vec<String>| ids.iter().filter_map(|id| id.parse().ok()).collect();
                let parent = listed.parent.and_then(|parent| parent.parse().ok());
                if parent.is_none() {
                    list.root_at.insert(id, list.roots.len());
                    list.roots.push(id);
                }
                list.by_id.insert(id, list.entities.len());
                list.entities.push(Known {
                    id,
                    name: listed.name,
                    parent,
                    children: ids(listed.children),
                    components: listed.components,
                    values: Vec::new(),
                    space: None,
                    instance: None,
                });
            }
            let described = |name: &String| self.types.find(name).is_some();
            let mut named = list.entities.iter().flat_map(|entity| &entity.components);
            self.types_due |= !named.all(described);
            self.list = list;
            self.troubles[Of::List as usize] = None;
            self.edits_due = true;
        }

        /// Takes in the entity of ID `id`, if it's still the one `picked`, its components' values
        /// read by the game's types; and lets go of the sets and changes the game has applied, of
        /// it and of any other no longer picked, which now show.
        fn entity(
            &mut self,
            id: EntityId,
            answer: Result<String, Failure>,
            picked: Option<EntityId>,
        ) {
            let applied =
                |of: EntityId, applied: bool| !applied || (of != id && Some(of) == picked);
            self.waiting.retain(|set| applied(set.id, set.applied));
            self.changes
                .retain(|change| applied(change.id, change.applied));
            if picked != Some(id) {
                return;
            }
            let answered = match answer.and_then(|json| read::<Answered>(&json)) {
                Ok(answered) => answered,
                // Despawned.
                Err(failure) if failure.code == "ecs.no_entity" => {
                    self.troubles[Of::Entity as usize] = None;
                    self.picked = Some((id, None));
                    return;
                }
                Err(failure) => {
                    let message = format!("Can't read entity {id}: {failure}");
                    self.trouble(Of::Entity, &failure.code, message);
                    return;
                }
            };
            self.troubles[Of::Entity as usize] = None;
            let order = |name: &String| self.components.iter().position(|(n, _)| n == name);
            let mut components: Vec<(String, JsonValue)> =
                answered.components.into_iter().collect();
            components.sort_by_key(|(name, _)| order(name).unwrap_or(usize::MAX));
            let names = Names(&self.types);
            let values = components.iter().map(|(name, value)| {
                let index = self
                    .types
                    .find(name)
                    .ok_or_else(|| format!("`{name}` is a type the game hasn't described"))?;
                json::read(value, &Type::Named(index), &names).map_err(|error| error.to_string())
            });
            let values = values.collect();
            let ids = |ids: Vec<String>| ids.iter().filter_map(|id| id.parse().ok()).collect();
            let entity = Known {
                id,
                name: answered.name,
                parent: answered.parent.and_then(|parent| parent.parse().ok()),
                children: ids(answered.children),
                components: components.into_iter().map(|(name, _)| name).collect(),
                values,
                space: answered
                    .space
                    .as_ref()
                    .and_then(|space| space_of(space, &names)),
                instance: answered.instance,
            };
            self.types_due |= entity
                .components
                .iter()
                .any(|name| self.types.find(name).is_none());
            self.picked = Some((id, Some(entity)));
        }

        /// Where the entity of ID `picked` is, as the last read of it gave it: if that read was of
        /// it, and gave its transform and where the space it's in sits.
        pub(in super::super) fn placed(&self, picked: Option<EntityId>) -> Option<Placed> {
            let (id, Some(entity)) = self.picked.as_ref()? else {
                return None;
            };
            if picked != Some(*id) {
                return None;
            }
            let at = entity
                .components
                .iter()
                .position(|name| name == "Transform")?;
            let local = entity.values.get(at)?.as_ref().ok()?;
            Some(Placed {
                id: *id,
                local: Transform::from_value(local).ok()?,
                space: entity.space?,
                generation: self.generation,
            })
        }

        /// What waits for the game: the sets of the fields of the entity of ID `picked`, the adds
        /// and removes, and the client's edits Undo and Redo would step through.
        pub(in super::super) fn pending(&self, picked: Option<EntityId>) -> Pending {
            let waiting = self.waiting.iter().filter(|set| Some(set.id) == picked);
            let changes = self.changes.iter();
            Pending {
                waiting: waiting.map(|set| set.edit.clone()).collect(),
                changes: changes
                    .map(|change| (change.id, change.component.clone(), change.adds))
                    .collect(),
                undo: self.undo.clone(),
                redo: self.redo.clone(),
            }
        }

        /// Sends the game what a pass of the panels did, as [`Inspector::queue`] queues it for a
        /// tick: the link's sets, creates and seals, then the pass's sets of the picked entity's
        /// fields, each gesture's sets of one field joining as one edit, then its operations, each
        /// of which ends the gesture going on, as the link's creates do, then the step its Edit
        /// menu or keys chose; and asks for what's next.
        pub(in super::super) fn send(&mut self, pass: Pass, inspector: &mut Inspector) {
            let Pass {
                sets,
                operations,
                saves,
                cues,
                chosen,
            } = pass;
            let mut linked = mem::take(&mut inspector.link.borrow_mut().pending);
            for linking in linked.drain(..) {
                match linking {
                    Linking::Set(id, edit) => self.set(id, edit, Gesture::Link),
                    Linking::Create(components) => {
                        let parent = None;
                        let create = Command::Create { parent, components };
                        self.operation(create, inspector.picked);
                    }
                    Linking::Seal => self.end(Gesture::Link),
                }
            }
            // Kept for its memory.
            inspector.link.borrow_mut().pending = linked;
            if cues.pressed {
                self.end(Gesture::Panels);
            }
            if let Some(id) = inspector.picked {
                for edit in sets {
                    self.set(id, edit, Gesture::Panels);
                }
            }
            for command in operations {
                self.operation(command, inspector.picked);
            }
            for id in saves {
                self.open = None;
                self.queued.push_back(Ask::SavePrefab(id));
            }
            if cues.released || !cues.held {
                self.end(Gesture::Panels);
            }
            if let Some(step) = chosen.or(cues.step) {
                self.queued.push_back(Ask::Step(step));
                self.open = None;
                inspector.link.borrow_mut().stepped += 1;
            }
            // Until a read shows each, so that a tool drawing a field as it set it doesn't draw it
            // as the read before had it.
            let link = &mut *inspector.link.borrow_mut();
            link.queued = !self.waiting.is_empty();
            self.ask_next(inspector.picked);
            // Edits it can't send, with no connection, never reach the game.
            let sent = self.sent.as_ref().is_some_and(Ask::edits);
            link.unanswered = self.connection.is_some() && (sent || !self.queued.is_empty());
        }

        /// Sets the field `edit` changes of the entity of ID `id`, as part of `gesture`'s, which it
        /// begins if another goes on, or none: the latest set of the field in the gesture alone
        /// waits to be sent, after those made since its last.
        fn set(&mut self, id: EntityId, edit: Edit, gesture: Gesture) {
            let number = match self.open {
                Some((open, number)) if open == gesture => number,
                _ => {
                    self.gestures = self.gestures.wrapping_add(1);
                    self.open = Some((gesture, self.gestures));
                    self.gestures
                }
            };
            let same = self.queued.iter().position(|ask| {
                matches!(ask, Ask::Set { id: of, edit: set, gesture, .. }
                    if *of == id && *gesture == number && set.component == edit.component
                        && set.path == edit.path)
            });
            if let Some(Ask::Set { number, .. }) = same.and_then(|at| self.queued.remove(at)) {
                self.waiting.retain(|set| set.number != number);
            }
            self.made += 1;
            self.waiting.push(Waiting {
                number: self.made,
                id,
                edit: edit.clone(),
                applied: false,
            });
            self.queued.push_back(Ask::Set {
                number: self.made,
                id,
                edit,
                gesture: number,
            });
        }

        /// Ends `gesture`'s edit, if it goes on.
        fn end(&mut self, gesture: Gesture) {
            if self.open.is_some_and(|(open, _)| open == gesture) {
                self.open = None;
            }
        }

        /// Sends `command`, an operation the menus chose while the entity of ID `picked` was
        /// picked, which ends the gesture going on.
        fn operation(&mut self, command: Command, picked: Option<EntityId>) {
            self.open = None;
            self.made += 1;
            let parent = match &command {
                Command::Create { parent, .. } => *parent,
                &Command::Duplicate(id) => self.parent(id),
                _ => None,
            };
            if let Command::Add(id, component, _) | Command::Remove(id, component) = &command {
                self.changes.push(Changing {
                    number: self.made,
                    id: *id,
                    component: component.clone(),
                    adds: matches!(command, Command::Add(..)),
                    applied: false,
                });
            }
            self.queued.push_back(Ask::Operation {
                command,
                picked,
                parent,
                number: self.made,
            });
        }

        /// Sends the next request, if none waits for its answer: as the module says.
        fn ask_next(&mut self, picked: Option<EntityId>) {
            let Some(connection) = &self.connection else {
                return;
            };
            if self.sent.is_some() {
                return;
            }
            let due = |at: Option<Instant>, every| at.is_none_or(|at| at.elapsed() >= every);
            let entity_due = picked.is_some_and(|id| {
                let stale = self.picked.as_ref().is_none_or(|(of, _)| *of != id);
                stale || due(self.entity_at, ENTITY_EVERY)
            });
            let types = (!self.typed || self.types_due) && due(self.types_at, LIST_EVERY);
            let ask = if types {
                self.types_at = Some(Instant::now());
                Ask::Types
            } else if !self.typed {
                // Nothing is read or written but by them.
                return;
            } else if !self.queued.is_empty() && !(self.edited && entity_due) {
                self.queued.pop_front().expect("one queued")
            } else if let (Some(id), true) = (picked, entity_due) {
                self.entity_at = Some(Instant::now());
                Ask::Entity(id)
            } else if let Some(read) = &self.reading {
                Ask::List { skip: read.len() }
            } else if due(self.list_at, LIST_EVERY) {
                self.list_at = Some(Instant::now());
                Ask::List { skip: 0 }
            } else if self.edits_due {
                self.edits_due = false;
                Ask::Edits
            } else {
                return;
            };
            let request = match self.request(&ask) {
                Some(request) => request,
                // A value that couldn't be written, as by types not yet read, isn't sent.
                None => return,
            };
            self.edited = ask.edits();
            if connection.asks.send(request).is_ok() {
                self.sent = Some(ask);
            }
        }

        /// The method and parameters of `ask`; `None` for a set whose value can't be written.
        fn request(&self, ask: &Ask) -> Option<(String, JsonValue)> {
            let names = Names(&self.types);
            let value = |value: &Value| {
                let mut out = String::new();
                json::write(&mut out, value, &names);
                serde_json::from_str::<JsonValue>(&out).ok()
            };
            let apply = |operation: gg_api::Operation, gesture: Option<u64>| {
                let apply = gg_api::Apply {
                    operations: vec![operation],
                    gesture,
                };
                let params = serde_json::to_value(apply).expect("an apply serializes");
                Some(("apply".to_owned(), params))
            };
            let id = |id: &EntityId| id.to_string();
            match ask {
                Ask::Types => Some(("types".to_owned(), JsonValue::Null)),
                Ask::List { skip } => {
                    let params = json!({"skip": skip, "take": PAGE});
                    Some(("entities".to_owned(), params))
                }
                Ask::Entity(of) => Some(("entity".to_owned(), json!({"id": id(of)}))),
                Ask::Edits => Some(("edits".to_owned(), json!({"author": self.client}))),
                Ask::Set {
                    id: of,
                    edit,
                    gesture,
                    ..
                } => {
                    let set = gg_api::Operation::Set {
                        id: id(of),
                        component: edit.component.clone(),
                        path: edit.path.clone(),
                        value: value(&edit.value)?,
                    };
                    apply(set, Some(*gesture))
                }
                Ask::Operation { command, .. } => {
                    let operation = match command {
                        Command::Create { parent, components } => gg_api::Operation::Create {
                            id: None,
                            parent: parent.as_ref().map(id),
                            components: components
                                .iter()
                                .map(|(name, given)| Some((name.to_string(), value(given)?)))
                                .collect::<Option<_>>()?,
                        },
                        Command::Duplicate(of) => gg_api::Operation::Duplicate {
                            id: id(of),
                            copies: Vec::new(),
                        },
                        Command::Delete(of) => gg_api::Operation::Delete { id: id(of) },
                        // The game's own default.
                        Command::Add(of, component, _) => gg_api::Operation::Add {
                            id: id(of),
                            component: component.clone(),
                            value: None,
                        },
                        Command::Remove(of, component) => gg_api::Operation::Remove {
                            id: id(of),
                            component: component.clone(),
                        },
                        Command::Revert(of, component, path) => gg_api::Operation::Revert {
                            id: id(of),
                            component: component.clone(),
                            path: path.clone(),
                        },
                        Command::Unlink(of) => gg_api::Operation::Unlink { id: id(of) },
                        Command::Reparent(of, parent) => gg_api::Operation::Reparent {
                            id: id(of),
                            parent: parent.as_ref().map(id),
                            in_place: true,
                        },
                        // A set of the name it has, as the list has it, or an add of one.
                        Command::Rename(of, name) => match self.name(*of) {
                            Some(_) => gg_api::Operation::Set {
                                id: id(of),
                                component: "Name".to_owned(),
                                path: "0".to_owned(),
                                value: value(&Value::String(name.clone()))?,
                            },
                            None => gg_api::Operation::Add {
                                id: id(of),
                                component: "Name".to_owned(),
                                value: Some(value(&Name(name.clone()).to_value())?),
                            },
                        },
                        Command::Set(..) | Command::Seal | Command::Step(_) => {
                            unreachable!("the menus choose no sets, seals or steps")
                        }
                    };
                    apply(operation, None)
                }
                Ask::Step(Step::Undo) => Some(("undo".to_owned(), JsonValue::Null)),
                Ask::Step(Step::Redo) => Some(("redo".to_owned(), JsonValue::Null)),
                Ask::SavePrefab(of) => Some(("save_prefab".to_owned(), json!({"id": id(of)}))),
            }
        }

        /// Says why a read of the game's world of kind `of` failed, `message`, of code `code`,
        /// until one of that kind doesn't, logging it as it's new.
        fn trouble(&mut self, of: Of, code: &str, message: String) {
            let kept = &mut self.troubles[of as usize];
            if kept.as_ref() != Some(&message) {
                warn!(code, "{message}");
                *kept = Some(message);
            }
        }

        /// The entity of ID `id`, as the picked entity's answer or the list gives it.
        fn known(&self, id: EntityId) -> Option<&Known> {
            match &self.picked {
                Some((of, Some(entity))) if *of == id => Some(entity),
                _ => self.list.by_id.get(&id).map(|&at| &self.list.entities[at]),
            }
        }

        fn parent(&self, id: EntityId) -> Option<EntityId> {
            self.known(id)?.parent
        }

        /// The places among the list of the entities with component `with`, or of them all.
        fn listed_with<'a>(&'a self, with: Option<&'a str>) -> impl Iterator<Item = EntityId> + 'a {
            let entities = self.list.entities.iter();
            let with = move |entity: &&Known| {
                with.is_none_or(|with| entity.components.iter().any(|name| name == with))
            };
            entities.filter(with).map(|entity| entity.id)
        }
    }

    impl Inspected for Remote {
        fn types(&self) -> &Types {
            &self.types
        }

        fn component_types(&self) -> Box<dyn Iterator<Item = &str> + '_> {
            Box::new(self.components.iter().map(|(name, _)| name.as_str()))
        }

        /// Alive if the picked entity's answer, or the list, has it, and not if the picked
        /// entity's answer says it's gone, or the list hasn't it; but the entity picked, until its
        /// answer comes, and the one the link asks to pick, as one just made, which the list may
        /// not have yet, aren't known.
        fn alive(&self, id: EntityId) -> Option<bool> {
            match &self.picked {
                Some((of, entity)) if *of == id => Some(entity.is_some()),
                _ if self.list.by_id.contains_key(&id) => Some(true),
                _ if self.wanted == Some(id) || self.linking == Some(id) || !self.list.read => None,
                _ => Some(false),
            }
        }

        fn name(&self, id: EntityId) -> Option<String> {
            self.known(id)?.name.clone()
        }

        fn parent(&self, id: EntityId) -> Option<EntityId> {
            Remote::parent(self, id)
        }

        fn children(&self, id: EntityId) -> Box<dyn Iterator<Item = EntityId> + '_> {
            let children = self.known(id).map(|entity| entity.children.iter().copied());
            Box::new(children.into_iter().flatten())
        }

        fn components(&self, id: EntityId) -> Vec<&str> {
            let known = self.known(id).map(|entity| entity.components.iter());
            known.into_iter().flatten().map(String::as_str).collect()
        }

        fn component(&self, id: EntityId, name: &str) -> Option<Result<Cow<'_, Value>, String>> {
            let Some((of, Some(entity))) = &self.picked else {
                return None;
            };
            if *of != id {
                return None;
            }
            let at = entity.components.iter().position(|of| of == name)?;
            let value = entity.values.get(at)?;
            Some(value.as_ref().map(Cow::Borrowed).map_err(Clone::clone))
        }

        fn default_of(&self, name: &str) -> Result<Value, String> {
            match self.components.iter().find(|(of, _)| of == name) {
                Some((_, default)) => default.clone(),
                None => Err(format!("the game has no component type `{name}`")),
            }
        }

        fn count(&self, with: Option<&str>) -> Result<usize, String> {
            if let Some(with) = with
                && !self.components.iter().any(|(name, _)| name == with)
            {
                return Err(format!("the game has no component type `{with}`"));
            }
            Ok(self.listed_with(with).count())
        }

        fn page(&self, with: Option<&str>, rows: Range<usize>) -> Vec<EntityId> {
            let page = self.listed_with(with).skip(rows.start).take(rows.len());
            page.collect()
        }

        fn roots(&self) -> usize {
            self.list.roots.len()
        }

        fn roots_page(&self, at: Range<usize>) -> Vec<EntityId> {
            let roots = &self.list.roots;
            roots[at.start.min(roots.len())..at.end.min(roots.len())].to_vec()
        }

        fn root_index(&self, id: EntityId) -> Option<usize> {
            self.list.root_at.get(&id).copied()
        }

        fn entity_label(&self, value: &Value) -> String {
            match value {
                Value::String(id) => id.clone(),
                _ => "none".to_owned(),
            }
        }

        fn asset_label(&self, value: &Value) -> String {
            match value {
                // Its name, its ID or else its path, as the game's values give it.
                Value::String(name) => name.clone(),
                _ => "none".to_owned(),
            }
        }

        /// As the last read of the picked entity gave it, while it's picked.
        fn instance(&self, id: EntityId) -> Option<Cow<'_, gg_api::Instanced>> {
            match &self.picked {
                Some((of, Some(entity))) if *of == id => {
                    entity.instance.as_ref().map(Cow::Borrowed)
                }
                _ => None,
            }
        }

        fn saves_prefabs(&self) -> bool {
            true
        }

        fn waiting(&self) -> Option<&str> {
            let reading = !(self.typed && self.list.read);
            let state = self.state.as_deref();
            let trouble = self.troubles.iter().flatten().next().map(String::as_str);
            state.or(trouble).or(reading.then_some(READING))
        }
    }

    /// What the panels say while no game serves, and while its world is first read, and once the
    /// connection has closed.
    const NOT_SERVING: &str = "Waiting for the game to serve";
    const READING: &str = "Reading the game's world";
    const CLOSED: &str = "The connection to the game closed";

    /// Connects to the game `session` names as client `client`, on a thread of its own, which
    /// sends each request it's given in turn, and passes each answer on, until the connection
    /// breaks or what it hears from goes.
    fn connect(session: Session, client: String) -> Connection {
        let (asks, asked) = mpsc::channel::<(String, JsonValue)>();
        let (hear, heard) = mpsc::channel();
        let talking = move || {
            let mut connection = match Client::connect(&session, &client, CONNECT) {
                Ok(connection) => connection,
                Err(failure) => {
                    let _ = hear.send(Heard::Failed(failure));
                    return;
                }
            };
            if let Err(failure) = connection.set_timeout(None) {
                let _ = hear.send(Heard::Failed(failure));
                return;
            }
            if hear.send(Heard::Connected).is_err() {
                return;
            }
            for (method, params) in asked {
                let answer = connection.call(&method, params);
                let answer = answer.map(|result| result.get().to_owned());
                if hear.send(Heard::Answer(answer)).is_err() {
                    return;
                }
            }
        };
        // A thread that doesn't start drops what it would hear through, which the Inspector takes
        // for the connection closed.
        let thread = thread::Builder::new()
            .name("gg-inspector".to_owned())
            .spawn(talking);
        if let Err(error) = thread {
            warn!(
                code = "api.connect",
                "Can't start the Inspector's connection: {error}"
            );
        }
        Connection { asks, heard }
    }

    /// The pose `space`, of an `entity` answer, gives, its floats read as a value's are, as `names`
    /// reads them: if it gives one, finite.
    fn space_of(space: &JsonValue, names: &Names) -> Option<Pose> {
        let matrix = space.get("matrix")?;
        let matrix = json::read(matrix, &Type::Array(Box::new(Type::F32), 16), names).ok()?;
        let Value::List(columns) = matrix else {
            return None;
        };
        let floats = columns.iter().map(|value| match *value {
            Value::F32(x) => Some(x),
            _ => None,
        });
        let floats: Vec<f32> = floats.collect::<Option<_>>()?;
        let matrix = Mat4::from_cols_slice(&floats);
        let rotation = json::read(space.get("rotation")?, &Type::Quat, names).ok()?;
        let Value::Quat(rotation) = rotation else {
            return None;
        };
        let finite = matrix.is_finite() && rotation.is_finite();
        finite.then_some(Pose { matrix, rotation })
    }

    /// `json`, an answer's result, as a `T`.
    ///
    /// Fails with `api.protocol` if it isn't one.
    fn read<T: serde::de::DeserializeOwned>(json: &str) -> Result<T, Failure> {
        serde_json::from_str(json).map_err(|error| {
            Failure::new("api.protocol", format!("an answer that isn't one: {error}"))
        })
    }

    #[cfg(test)]
    mod tests {
        use gg_core::math::{Quat, Vec3};

        use super::*;

        fn id(n: u64) -> EntityId {
            format!("{n:016x}").parse().expect("an ID")
        }

        /// A page of a list of `total` entities, each with its parent and children by their
        /// numbers, as `entities` answers it.
        fn page(total: usize, entities: &[(u64, Option<u64>, &[u64])]) -> Result<String, Failure> {
            let listed = entities
                .iter()
                .map(|&(n, parent, children)| gg_api::Listed {
                    id: id(n).to_string(),
                    name: None,
                    parent: parent.map(|parent| id(parent).to_string()),
                    children: children
                        .iter()
                        .map(|&child| id(child).to_string())
                        .collect(),
                    components: Vec::new(),
                });
            let list = gg_api::EntityList {
                total,
                entities: listed.collect(),
                types: 0,
            };
            Ok(serde_json::to_string(&list).expect("a list serializes"))
        }

        /// A list read in pages is whole once it has as many as the page counts, an entity read on
        /// two pages, as one an entity spawned ahead of it moved to the next, listed once.
        #[test]
        fn a_list_read_in_pages_lists_each_entity_once() {
            let mut remote = Remote::new("editor");
            remote.listed(0, page(3, &[(1, None, &[]), (2, None, &[])]));
            assert!(!remote.list.read, "not yet whole");
            remote.listed(2, page(4, &[(2, None, &[]), (3, None, &[])]));
            assert!(remote.list.read);
            assert_eq!(remote.count(None), Ok(3));
            assert_eq!(remote.roots_page(0..3), [id(1), id(2), id(3)]);
        }

        /// A list of another revision of the types than were read, as after a script's reload
        /// changed them, has them asked for again; one of the same leaves them be.
        #[test]
        fn a_list_of_another_revision_of_the_types_has_them_asked_for_again() {
            let listed = |types: u64| {
                let page = page(1, &[(1, None, &[])]).expect("a page");
                let mut list: gg_api::EntityList = serde_json::from_str(&page).expect("a list");
                list.types = types;
                Ok(serde_json::to_string(&list).expect("a list serializes"))
            };
            let mut remote = Remote::new("editor");
            (remote.typed, remote.types_due, remote.revision) = (true, false, 7);
            remote.listed(0, listed(7));
            assert!(!remote.types_due);
            remote.listed(0, listed(8));
            assert!(remote.types_due);
        }

        /// A hierarchy put together from reads at two moments that holds a cycle, as one entity's
        /// answer saying its parent is another whose listing says it's its parent, leaves the
        /// tree's rows and the walk up an entity's ancestors finite.
        #[test]
        fn a_cycle_between_reads_ends_the_walks() {
            let mut remote = Remote::new("editor");
            let listing: &[(u64, Option<u64>, &[u64])] = &[
                (1, None, &[2]),
                (2, Some(1), &[1]),
                (3, Some(4), &[]),
                (4, Some(3), &[]),
            ];
            remote.listed(0, page(4, listing));
            let expanded = HashSet::from([id(1), id(2)]);
            let tree = crate::inspector::entities::Tree::new(&remote, &expanded);
            assert_eq!(
                tree.len, 3,
                "the root, its child, and the root again, unopened"
            );
            let mut inspector = Inspector::default();
            inspector.open_ancestors(&remote, id(3));
            assert_eq!(inspector.expanded, HashSet::from([id(4)]));
        }

        /// A set the game has applied waits until an answer of its entity, read after it, shows it,
        /// while it's picked; a set of an entity not picked goes at any answer once it's applied;
        /// and an answer saying the entity picked is gone says it's despawned.
        #[test]
        fn sets_wait_until_a_read_of_their_entity_shows_them() {
            let mut remote = Remote::new("editor");
            let (a, b, c) = (id(1), id(2), id(3));
            let set = |number, id, applied| Waiting {
                number,
                id,
                edit: Edit {
                    component: "Transform".to_owned(),
                    path: "translation.x".to_owned(),
                    value: Value::F32(1.0),
                },
                applied,
            };
            remote.waiting = vec![
                set(1, a, true),
                set(2, a, false),
                set(3, b, true),
                set(4, c, true),
                set(5, c, false),
            ];
            let other = Err(Failure::new("ecs.no_entity", "gone"));
            remote.entity(b, other, Some(a));
            let numbers = |remote: &Remote| -> Vec<u64> {
                remote.waiting.iter().map(|set| set.number).collect()
            };
            assert_eq!(numbers(&remote), [1, 2, 5], "applied sets of others go");
            let answer = r#"{"id":"","name":null,"parent":null,"children":[],"components":{}}"#;
            remote.entity(a, Ok(answer.to_owned()), Some(a));
            assert_eq!(numbers(&remote), [2, 5], "shown once read after it");
            assert_eq!(remote.alive(a), Some(true));
            assert_eq!(
                remote.pending(Some(a)).waiting.len(),
                1,
                "the picked entity's alone"
            );
            remote.entity(a, Err(Failure::new("ecs.no_entity", "gone")), Some(a));
            assert_eq!(remote.alive(a), Some(false), "despawned");
        }

        /// The link's pick of an entity the list hasn't, as one the game has just made, picks it,
        /// its first read opening the rows above it, and the reads after leaving them be; and once
        /// a read of it has found it gone, unpicks.
        #[test]
        fn the_links_pick_of_an_entity_the_list_hasnt_yet_picks_it() {
            let mut remote = Remote::new("editor");
            remote.listed(0, page(1, &[(1, None, &[])]));
            let mut inspector = Inspector::default();
            let link = inspector.link();
            link.pick(Some(id(2)));
            remote.update(&mut inspector);
            inspector.follow(&remote);
            assert_eq!(inspector.picked, Some(id(2)), "not known yet");
            assert!(inspector.expanded.is_empty(), "its parent not known yet");
            remote.update(&mut inspector);
            let child = format!(
                r#"{{"name":null,"parent":"{}","children":[],"components":{{}}}}"#,
                id(1)
            );
            remote.answer(Ask::Entity(id(2)), Ok(child.clone()), &mut inspector);
            assert_eq!(inspector.expanded, HashSet::from([id(1)]), "by its read");
            inspector.expanded.clear();
            remote.answer(Ask::Entity(id(2)), Ok(child), &mut inspector);
            assert!(inspector.expanded.is_empty(), "closed, and left so");
            let gone = Err(Failure::new("ecs.no_entity", "gone"));
            remote.entity(id(2), gone, Some(id(2)));
            link.pick(Some(id(2)));
            remote.update(&mut inspector);
            inspector.follow(&remote);
            assert_eq!(inspector.picked, None, "found gone");
        }

        /// The entity picked is placed by a read of it, while it's picked: its transform and the
        /// space it's in, whose floats are read as values' are; a read without a space, or with
        /// one that isn't finite, places nothing; each reach counts a generation; and a read taken
        /// before the pick went elsewhere doesn't place it once it's back.
        #[test]
        fn a_pick_is_placed_by_a_read_of_it_alone() {
            let mut remote = Remote::new("editor");
            let types = crate::Context::seeded(0).world().types().clone();
            remote.types = types.clone();
            let (a, b) = (id(1), id(2));
            let answer = |space: &str| {
                let transform = r#"{"translation":[1,2,3],"rotation":[0,0,0,1],"scale":[1,1,1]}"#;
                let components = format!(r#""components":{{"Transform":{transform}}}"#);
                let entity = r#""name":null,"parent":null,"children":[]"#;
                Ok(format!("{{{entity},{components}{space}}}"))
            };
            let matrix = |x: &str| format!("[1,0,0,0,0,1,0,0,0,0,1,0,{x},5,0,1]");
            let space = |x: &str| {
                format!(
                    r#","space":{{"matrix":{},"rotation":[0,0,0,1]}}"#,
                    matrix(x)
                )
            };
            remote.entity(a, answer(&space("4")), Some(a));
            let placed = remote.placed(Some(a)).expect("placed");
            let local = Transform {
                translation: Vec3::new(1.0, 2.0, 3.0),
                ..Transform::IDENTITY
            };
            let parent = Pose {
                matrix: Mat4::from_translation(Vec3::new(4.0, 5.0, 0.0)),
                rotation: Quat::IDENTITY,
            };
            let expected = Placed {
                id: a,
                local,
                space: parent,
                generation: 0,
            };
            assert_eq!(placed, expected);
            assert_eq!(remote.placed(Some(b)), None, "while another is picked");
            for unplaced in ["", &space(r#""inf""#), r#","space":{"rotation":[0,0,0,1]}"#] {
                remote.entity(a, answer(unplaced), Some(a));
                assert_eq!(remote.placed(Some(a)), None, "{unplaced}");
            }
            remote.reach(None);
            remote.types = types;
            remote.entity(a, answer(&space("4")), Some(a));
            let generation = remote.placed(Some(a)).map(|placed| placed.generation);
            assert_eq!(generation, Some(1), "another reach");
            let mut inspector = Inspector {
                picked: Some(a),
                ..Inspector::default()
            };
            remote.update(&mut inspector);
            remote.entity(a, answer(&space("4")), Some(a));
            remote.update(&mut inspector);
            assert!(remote.placed(Some(a)).is_some(), "while it stays picked");
            inspector.picked = Some(b);
            remote.update(&mut inspector);
            inspector.picked = Some(a);
            assert_eq!(remote.placed(Some(a)), None, "back, until a read of it");
        }

        /// A read of the picked entity gives what of it differs from its prefab, while it's picked;
        /// and a save as a prefab, an unlink and a revert go to the game as the API's `save_prefab`
        /// and `apply` take them, each an edit, after which the entity is read again.
        #[test]
        fn an_instances_read_and_its_prefabs_edits_go_as_the_api_has_them() {
            let mut remote = Remote::new("editor");
            let a = id(1);
            let instance = format!(
                r#"{{"root":"{a}","prefab":"prefabs/lamp.prefab","link":"prefabs/lamp.prefab",
                "overrides":{{"Sprite":["color"]}},"added":["Bob"]}}"#
            );
            let answer = format!(
                r#"{{"name":null,"parent":null,"children":[],"components":{{}},
                "instance":{instance}}}"#
            );
            remote.entity(a, Ok(answer), Some(a));
            let read = remote.instance(a).expect("an instance's");
            assert_eq!(read.link.as_deref(), Some("prefabs/lamp.prefab"));
            let color = ["color".to_owned()];
            assert_eq!(
                read.overrides.get("Sprite").map(Vec::as_slice),
                Some(&color[..])
            );
            assert_eq!(read.added, ["Bob"]);
            assert!(remote.instance(id(2)).is_none(), "not picked");
            assert!(remote.saves_prefabs());
            let ask = |ask: &Ask| remote.request(ask).expect("a request");
            let save = ask(&Ask::SavePrefab(a));
            assert_eq!(
                save,
                ("save_prefab".to_owned(), json!({"id": a.to_string()}))
            );
            assert!(Ask::SavePrefab(a).edits());
            let operation = |command| Ask::Operation {
                command,
                picked: Some(a),
                parent: None,
                number: 1,
            };
            let unlink = ask(&operation(Command::Unlink(a)));
            let operations = json!({"operations": [{"unlink": {"id": a.to_string()}}]});
            assert_eq!(unlink, ("apply".to_owned(), operations));
            let revert = Command::Revert(a, "Sprite".to_owned(), "color".to_owned());
            let reverted = json!({"revert": {"id": a.to_string(), "component": "Sprite",
                "path": "color"}});
            let operations = json!({"operations": [reverted]});
            assert_eq!(ask(&operation(revert)), ("apply".to_owned(), operations));
        }

        /// A reparent goes to the game as the API's `reparent` takes it, to a parent or to the
        /// roots; and a rename as a set of the name the entity has, as the read of it says, or for
        /// one without, an add of one.
        #[test]
        fn reparents_and_renames_go_as_the_api_has_them() {
            let mut remote = Remote::new("editor");
            let (a, b) = (id(1), id(2));
            let ask = |remote: &Remote, command| {
                let operation = Ask::Operation {
                    command,
                    picked: Some(a),
                    parent: None,
                    number: 1,
                };
                remote.request(&operation).expect("a request")
            };
            let apply = |operation| ("apply".to_owned(), json!({ "operations": [operation] }));
            let (a_id, b_id) = (a.to_string(), b.to_string());
            let reparent = json!({"reparent": {"id": a_id, "parent": b_id, "in_place": true}});
            assert_eq!(ask(&remote, Command::Reparent(a, Some(b))), apply(reparent));
            let root = json!({"reparent": {"id": a_id, "parent": null, "in_place": true}});
            assert_eq!(ask(&remote, Command::Reparent(a, None)), apply(root));
            let read = |name: &str| {
                let answer =
                    format!(r#"{{"name":{name},"parent":null,"children":[],"components":{{}}}}"#);
                Ok(answer)
            };
            let rename = || Command::Rename(a, "Lamp".to_owned());
            remote.entity(a, read("null"), Some(a));
            // Whole, as a create's components go.
            let add = json!({"add": {"id": a_id, "component": "Name", "value": {"0": "Lamp"}}});
            assert_eq!(ask(&remote, rename()), apply(add));
            remote.entity(a, read(r#""Old""#), Some(a));
            let set =
                json!({"set": {"id": a_id, "component": "Name", "path": "0", "value": "Lamp"}});
            assert_eq!(ask(&remote, rename()), apply(set));
        }

        /// A set through the link waits, as the link says, until a read of its entity after the
        /// game's answer to it shows it, not only until the answer.
        #[test]
        fn the_links_sets_wait_until_a_read_shows_them() {
            let mut remote = Remote::new("editor");
            let mut inspector = Inspector::default();
            let a = id(1);
            remote.waiting.push(Waiting {
                number: 1,
                id: a,
                edit: Edit {
                    component: "Transform".to_owned(),
                    path: "translation.x".to_owned(),
                    value: Value::F32(1.0),
                },
                applied: false,
            });
            let mut waits = Vec::new();
            for applied in [false, true] {
                remote.waiting[0].applied = applied;
                remote.send(Pass::default(), &mut inspector);
                waits.push(inspector.link().waiting());
            }
            let answer = r#"{"id":"","name":null,"parent":null,"children":[],"components":{}}"#;
            remote.entity(a, Ok(answer.to_owned()), Some(a));
            remote.send(Pass::default(), &mut inspector);
            waits.push(inspector.link().waiting());
            assert_eq!(waits, [true, true, false]);
        }

        /// The link is idle once the game has answered every edit, operations as sets: not while a
        /// set made through it waits for the pass, nor while an edit waits to be sent, nor while
        /// its answer hasn't come, but before a read shows it; and edits that can't be sent,
        /// without a connection, keep nothing waiting.
        #[test]
        fn the_link_is_idle_once_the_game_has_answered_every_edit() {
            let mut remote = Remote::new("editor");
            let mut inspector = Inspector::default();
            let link = inspector.link();
            let (asks, asked) = mpsc::channel();
            let (hear, heard) = mpsc::channel();
            remote.connection = Some(Connection { asks, heard });
            (remote.typed, remote.types_due) = (true, false);
            let answer = |remote: &mut Remote, inspector: &mut Inspector, result: &str| {
                hear.send(Heard::Answer(Ok(result.to_owned())))
                    .expect("heard");
                remote.update(inspector);
                remote.send(Pass::default(), inspector);
            };
            let methods = || {
                asked
                    .try_iter()
                    .map(|(method, _)| method)
                    .collect::<Vec<_>>()
            };
            let a = id(1);
            assert!(link.idle(), "nothing made");
            link.set(a, "Transform", "translation.x", Value::F32(1.0));
            assert!(!link.idle(), "until the pass");
            remote.send(Pass::default(), &mut inspector);
            assert_eq!(methods(), ["apply"]);
            assert!(!link.idle(), "until its answer");
            answer(&mut remote, &mut inspector, r#"{"ticks":0,"entities":[]}"#);
            assert_eq!(methods(), ["entities"]);
            assert!(link.idle(), "answered");
            assert!(link.waiting(), "though no read shows it yet");
            let delete = Pass {
                operations: vec![Command::Delete(a)],
                ..Pass::default()
            };
            remote.send(delete, &mut inspector);
            assert!(!link.idle(), "queued behind the list");
            answer(&mut remote, &mut inspector, &page(0, &[]).expect("a page"));
            assert_eq!(methods(), ["apply"]);
            assert!(!link.idle(), "until its answer");
            answer(&mut remote, &mut inspector, r#"{"ticks":0,"entities":[]}"#);
            assert!(link.idle(), "answered");
            remote.connection = None;
            remote.queued.push_back(Ask::Step(Step::Undo));
            remote.send(Pass::default(), &mut inspector);
            assert!(link.idle(), "never to be sent");
        }
    }
}

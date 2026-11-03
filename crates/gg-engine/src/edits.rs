//! The edits the engine applies to the world for authors outside the layers: the tool API's
//! clients, and the scene files a game loaded, saved while it runs. Each author undoes and redoes
//! its own, through a [`Log`] of its own, as the Inspector does through its. They apply between
//! ticks, as a frame starts, which a recording keeps as values, the edits as their authors gave
//! them, before the commands of the tick after, and a replay applies there. So do the reloads of
//! scripts whose files were saved, which no author undoes.

use gg_core::{Error, Result};

use crate::reflect::{self, Ident, Value};
use crate::{EntityId, Log, Operation, World};

/// An edit, as an author asks for it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Edit {
    /// Applies `operations`, in order, as one edit of `author`'s: all of them or none.
    Apply {
        author: String,
        operations: Vec<Operation>,
    },
    /// Applies `operations` as the rest of `author`'s edit applied just before, as
    /// [`Log::append`] does: those whose values hold entities that edit made.
    Append {
        author: String,
        operations: Vec<Operation>,
    },
    /// Applies `operation`, a set, as part of `author`'s gesture `gesture`, such as a drag: as
    /// [`Log::amend`] does, joining `author`'s last edit if that's a set of the same field in the
    /// same gesture, so that the gesture undoes whole.
    Amend {
        author: String,
        gesture: u64,
        operation: Operation,
    },
    /// Undoes `author`'s last edit.
    Undo { author: String },
    /// Redoes the last edit `author` undid.
    Redo { author: String },
    /// Reloads the script whose path from the project's root is `path`, its text now `text`,
    /// through the [`Scripts`](crate::Scripts) that loaded it, as the context applies it.
    Script { path: String, text: String },
}

/// What an edit did: the IDs of the entities an apply's creates and duplicates spawned, in order,
/// or the operations of the edit an undo or a redo stepped through, if there was one.
#[derive(Debug, PartialEq)]
pub(crate) enum Done {
    Applied(Vec<EntityId>),
    Stepped(Option<Vec<Operation>>),
}

/// Each author's log of its edits, in the order the authors first edited, of the world whose serial
/// is `world`: another world, as one cleared, replaced or restored is, starts them afresh, as
/// their entries are refused there.
#[derive(Default)]
pub(crate) struct Authors {
    logs: Vec<Author>,
    world: Option<u64>,
}

/// An author's log, and the gesture its last edit is part of, if that's an amend's.
struct Author {
    name: String,
    log: Log,
    gesture: Option<u64>,
}

impl Authors {
    /// Applies `edit`, an author's, to `world`, and says what it did.
    ///
    /// Fails as [`Log::apply_all`], [`Log::amend`], [`Log::undo`] and [`Log::redo`] do.
    ///
    /// # Panics
    ///
    /// For a script's reload, which no author makes: the context reloads it.
    pub(crate) fn edit(&mut self, world: &mut World, edit: Edit) -> Result<Done> {
        if self.world != Some(world.serial()) {
            // Their entries are another world's, which keeps what they kept, if it's still about.
            self.logs.clear();
            self.world = Some(world.serial());
        }
        // Any other edit of the author's ends its gesture, refused or not, as the log's own end
        // its, but for a refused apply. One the API refuses as it reads it never comes here, nor
        // to a recording, and leaves it going on.
        if !matches!(edit, Edit::Amend { .. })
            && let Some(author) = self.logs.iter_mut().find(|a| a.name == edit.author())
        {
            author.log.seal();
            author.gesture = None;
        }
        match edit {
            Edit::Script { .. } => unreachable!("a context reloads scripts itself"),
            Edit::Amend {
                author,
                gesture,
                operation,
            } => {
                let author = self.author(author);
                if author.gesture != Some(gesture) {
                    author.log.seal();
                    author.gesture = Some(gesture);
                }
                author.log.amend(world, operation)?;
                Ok(Done::Applied(Vec::new()))
            }
            // Nothing to log, nor any author to list.
            Edit::Apply { operations, .. } if operations.is_empty() => {
                Ok(Done::Applied(Vec::new()))
            }
            Edit::Apply { author, operations } => {
                let log = &mut self.author(author).log;
                log.apply_all(world, operations)?;
                Ok(Done::Applied(made(log)))
            }
            Edit::Append { author, operations } => {
                let log = &mut self.author(author).log;
                log.append(world, operations)?;
                Ok(Done::Applied(made(log)))
            }
            Edit::Undo { author } | Edit::Redo { author } if !self.has(&author) => {
                Ok(Done::Stepped(None))
            }
            Edit::Undo { author } => {
                let log = &mut self.author(author).log;
                let undoable = log.undoable().map(<[Operation]>::to_vec);
                Ok(Done::Stepped(
                    log.undo(world)?.then_some(undoable).flatten(),
                ))
            }
            Edit::Redo { author } => {
                let log = &mut self.author(author).log;
                let redoable = log.redoable().map(<[Operation]>::to_vec);
                Ok(Done::Stepped(
                    log.redo(world)?.then_some(redoable).flatten(),
                ))
            }
        }
    }

    #[cfg(feature = "dev")]
    /// Each author's log, in the order the authors first edited `world`'s current entities: none
    /// for a world that took the place of the one they edited.
    pub(crate) fn logs(&self, world: &World) -> impl Iterator<Item = (&str, &Log)> {
        let current = self.world == Some(world.serial());
        let logs = self.logs.iter().filter(move |_| current);
        logs.map(|author| (author.name.as_str(), &author.log))
    }

    /// Whether `author` has edited the world.
    fn has(&self, author: &str) -> bool {
        self.logs.iter().any(|logged| logged.name == author)
    }

    /// `author`'s log and gesture, which start empty.
    fn author(&mut self, author: String) -> &mut Author {
        let at = match self.logs.iter().position(|logged| logged.name == author) {
            Some(at) => at,
            None => {
                self.logs.push(Author {
                    name: author,
                    log: Log::default(),
                    gesture: None,
                });
                self.logs.len() - 1
            }
        };
        &mut self.logs[at]
    }

    /// Drops every log, and what they kept, from `world`, if they're its.
    pub(crate) fn clear(&mut self, world: &mut World) {
        if self.world == Some(world.serial()) {
            for author in &mut self.logs {
                author.log.clear(world);
            }
        }
        self.logs.clear();
    }
}

/// The IDs of the entities the creates and duplicates of `log`'s last edit spawned, in order.
fn made(log: &Log) -> Vec<EntityId> {
    let made = log.undoable().unwrap_or_default().iter();
    let made = made.flat_map(|operation| match operation {
        Operation::Create { entity, .. } => entity.iter().copied().collect(),
        Operation::Duplicate { copies, .. } => copies.clone(),
        _ => Vec::new(),
    });
    made.collect()
}

impl Edit {
    /// Its author's name.
    fn author(&self) -> &str {
        match self {
            Self::Apply { author, .. }
            | Self::Append { author, .. }
            | Self::Amend { author, .. }
            | Self::Undo { author }
            | Self::Redo { author } => author,
            Self::Script { path, .. } => path,
        }
    }

    /// The edit as a value, which a recording keeps: variants `Apply` and `Append`, with its author
    /// and its operations, `Amend`, with its author, its gesture and its operation, `Undo` and
    /// `Redo`, with their author, and `Script`, with its path and its text; each operation a
    /// variant named as its [`Operation`]'s, with its entities' IDs as text.
    pub(crate) fn to_value(&self) -> Value {
        let text = |text: &str| Value::String(text.to_owned());
        let author = |author: &str| ("author".into(), text(author));
        match self {
            Self::Amend {
                author: by,
                gesture,
                operation,
            } => Value::Variant(
                "Amend".into(),
                vec![
                    author(by),
                    ("gesture".into(), Value::UInt(*gesture)),
                    ("operation".into(), operation_value(operation)),
                ],
            ),
            Self::Apply {
                author: by,
                operations,
            }
            | Self::Append {
                author: by,
                operations,
            } => {
                let name = if matches!(self, Self::Apply { .. }) {
                    "Apply"
                } else {
                    "Append"
                };
                let operations = operations.iter().map(operation_value).collect();
                Value::Variant(
                    name.into(),
                    vec![author(by), ("operations".into(), Value::List(operations))],
                )
            }
            Self::Undo { author: by } => Value::Variant("Undo".into(), vec![author(by)]),
            Self::Redo { author: by } => Value::Variant("Redo".into(), vec![author(by)]),
            Self::Script { path, text: script } => Value::Variant(
                "Script".into(),
                vec![("path".into(), text(path)), ("text".into(), text(script))],
            ),
        }
    }

    /// The edit `value` is, as [`to_value`](Self::to_value) writes it.
    ///
    /// Fails as [`Reflect::from_value`](crate::Reflect::from_value) does for a value that isn't
    /// one, saying where.
    pub(crate) fn from_value(value: &Value) -> Result<Self> {
        let (name, fields) = reflect::variant(value)?;
        match name {
            "Apply" | "Append" => {
                let [author, operations] = reflect::pick(fields, ["author", "operations"])?;
                let Value::List(operations) = operations else {
                    let error = reflect::wrong_kind("List", operations);
                    return Err(reflect::within(error, "operations"));
                };
                let operations = operations.iter().enumerate().map(|(i, value)| {
                    operation(value).map_err(|error| reflect::within(error, &format!("[{i}]")))
                });
                let author = text(author, "author")?;
                let operations = operations
                    .collect::<Result<_>>()
                    .map_err(|error| reflect::within(error, "operations"))?;
                Ok(if name == "Apply" {
                    Self::Apply { author, operations }
                } else {
                    Self::Append { author, operations }
                })
            }
            "Amend" => {
                let [author, gesture, operation] =
                    reflect::pick(fields, ["author", "gesture", "operation"])?;
                let &Value::UInt(gesture) = gesture else {
                    let error = reflect::wrong_kind("UInt", gesture);
                    return Err(reflect::within(error, "gesture"));
                };
                Ok(Self::Amend {
                    author: text(author, "author")?,
                    gesture,
                    operation: self::operation(operation)
                        .map_err(|error| reflect::within(error, "operation"))?,
                })
            }
            "Undo" | "Redo" => {
                let [author] = reflect::pick(fields, ["author"])?;
                let author = text(author, "author")?;
                Ok(if name == "Undo" {
                    Self::Undo { author }
                } else {
                    Self::Redo { author }
                })
            }
            "Script" => {
                let [path, script] = reflect::pick(fields, ["path", "text"])?;
                Ok(Self::Script {
                    path: text(path, "path")?,
                    text: text(script, "text")?,
                })
            }
            other => Err(reflect::no_variant(
                other,
                &["Apply", "Append", "Amend", "Undo", "Redo", "Script"],
            )),
        }
    }
}

/// `operation` as a value, as [`Edit::to_value`] writes it.
fn operation_value(operation: &Operation) -> Value {
    let text = |text: &str| Value::String(text.to_owned());
    let maybe =
        |held: &Option<EntityId>| Value::Option(held.as_ref().map(|held| Box::new(id(held))));
    let entity = |entity: &EntityId| -> (Ident, Value) { ("entity".into(), id(entity)) };
    let (name, fields) = match operation {
        Operation::Create {
            entity: made,
            parent,
            components,
        } => (
            "Create",
            vec![
                ("entity".into(), maybe(made)),
                ("parent".into(), maybe(parent)),
                ("components".into(), Value::Struct(components.clone())),
            ],
        ),
        Operation::Duplicate { entity: of, copies } => (
            "Duplicate",
            vec![
                entity(of),
                (
                    "copies".into(),
                    Value::List(copies.iter().map(id).collect()),
                ),
            ],
        ),
        Operation::Delete { entity: of } => ("Delete", vec![entity(of)]),
        Operation::Add {
            entity: of,
            component,
            value,
        } => (
            "Add",
            vec![
                entity(of),
                ("component".into(), text(component)),
                ("value".into(), value.clone()),
            ],
        ),
        Operation::Remove {
            entity: of,
            component,
        } => (
            "Remove",
            vec![entity(of), ("component".into(), text(component))],
        ),
        Operation::Set {
            entity: of,
            component,
            path,
            value,
        } => (
            "Set",
            vec![
                entity(of),
                ("component".into(), text(component)),
                ("path".into(), text(path)),
                ("value".into(), value.clone()),
            ],
        ),
        Operation::Reparent { entity: of, parent } => (
            "Reparent",
            vec![entity(of), ("parent".into(), maybe(parent))],
        ),
    };
    Value::Variant(name.into(), fields)
}

/// An entity's ID as an operation's value holds it, as text.
fn id(id: &EntityId) -> Value {
    Value::String(id.to_string())
}

/// The operation `value` is, as [`operation_value`] writes it.
fn operation(value: &Value) -> Result<Operation> {
    let (name, fields) = reflect::variant(value)?;
    let entity = |value: &Value| parse_id(value, "entity");
    let maybe = |value: &Value, field: &str| match value {
        Value::Option(None) => Ok(None),
        Value::Option(Some(id)) => parse_id(id, field).map(Some),
        other => Err(reflect::within(reflect::wrong_kind("Option", other), field)),
    };
    let component = |value: &Value| text(value, "component").map(Ident::from);
    Ok(match name {
        "Create" => {
            let [made, parent, components] =
                reflect::pick(fields, ["entity", "parent", "components"])?;
            let Value::Struct(components) = components else {
                let error = reflect::wrong_kind("Struct", components);
                return Err(reflect::within(error, "components"));
            };
            Operation::Create {
                entity: maybe(made, "entity")?,
                parent: maybe(parent, "parent")?,
                components: components.clone(),
            }
        }
        "Duplicate" => {
            let [of, copies] = reflect::pick(fields, ["entity", "copies"])?;
            let Value::List(copies) = copies else {
                return Err(reflect::within(
                    reflect::wrong_kind("List", copies),
                    "copies",
                ));
            };
            let copies = copies.iter().map(|copy| parse_id(copy, "copies"));
            Operation::Duplicate {
                entity: entity(of)?,
                copies: copies.collect::<Result<_>>()?,
            }
        }
        "Delete" => {
            let [of] = reflect::pick(fields, ["entity"])?;
            Operation::Delete {
                entity: entity(of)?,
            }
        }
        "Add" => {
            let [of, named, value] = reflect::pick(fields, ["entity", "component", "value"])?;
            Operation::Add {
                entity: entity(of)?,
                component: component(named)?,
                value: value.clone(),
            }
        }
        "Remove" => {
            let [of, named] = reflect::pick(fields, ["entity", "component"])?;
            Operation::Remove {
                entity: entity(of)?,
                component: component(named)?,
            }
        }
        "Set" => {
            let [of, named, path, value] =
                reflect::pick(fields, ["entity", "component", "path", "value"])?;
            Operation::Set {
                entity: entity(of)?,
                component: component(named)?,
                path: text(path, "path")?,
                value: value.clone(),
            }
        }
        "Reparent" => {
            let [of, parent] = reflect::pick(fields, ["entity", "parent"])?;
            Operation::Reparent {
                entity: entity(of)?,
                parent: maybe(parent, "parent")?,
            }
        }
        other => {
            let names = [
                "Create",
                "Duplicate",
                "Delete",
                "Add",
                "Remove",
                "Set",
                "Reparent",
            ];
            return Err(reflect::no_variant(other, &names));
        }
    })
}

/// The text `value` holds, as field `field`.
fn text(value: &Value, field: &str) -> Result<String> {
    match value {
        Value::String(text) => Ok(text.clone()),
        other => Err(reflect::within(reflect::wrong_kind("String", other), field)),
    }
}

/// The ID `value` holds as text, as field `field`.
fn parse_id(value: &Value, field: &str) -> Result<EntityId> {
    let id: Result<EntityId> = text(value, field)?.parse();
    id.map_err(|error: Error| reflect::within(error, field))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Transform;

    /// A world of one entity with a transform, and its ID.
    fn world() -> (World, EntityId) {
        let mut world = World::seeded(1);
        world.register::<Transform>().expect("registered");
        let entity = world.spawn(Transform::IDENTITY);
        let id = world.id(entity).expect("alive");
        (world, id)
    }

    fn set(id: EntityId, y: f32) -> Operation {
        Operation::Set {
            entity: id,
            component: "Transform".into(),
            path: "translation.y".to_owned(),
            value: Value::F32(y),
        }
    }

    fn amend(gesture: u64, operation: Operation) -> Edit {
        Edit::Amend {
            author: "editor".to_owned(),
            gesture,
            operation,
        }
    }

    /// An author's sets of one field in one gesture undo whole; another gesture, or any other edit
    /// of the author's between them, refused or not, starts another edit; and another author's
    /// edits leave the gesture going on.
    #[test]
    fn a_gestures_sets_undo_whole() {
        let (mut world, id) = world();
        let mut authors = Authors::default();
        let edit = |authors: &mut Authors, world: &mut World, edit| {
            authors.edit(world, edit).expect("applied");
        };
        let y = |world: &World| {
            let entity = world.entity(id).expect("alive");
            world
                .get::<Transform>(entity)
                .expect("a transform")
                .translation
                .y
        };
        let undo = || Edit::Undo {
            author: "editor".to_owned(),
        };
        for y in [1.0, 2.0, 3.0] {
            edit(&mut authors, &mut world, amend(1, set(id, y)));
        }
        let other = Edit::Apply {
            author: "agent".to_owned(),
            operations: vec![set(id, 9.0)],
        };
        edit(&mut authors, &mut world, other);
        edit(&mut authors, &mut world, amend(1, set(id, 4.0)));
        edit(&mut authors, &mut world, undo());
        assert_eq!(y(&world), 0.0, "the gesture undone whole");
        edit(&mut authors, &mut world, amend(2, set(id, 5.0)));
        edit(&mut authors, &mut world, amend(3, set(id, 6.0)));
        edit(&mut authors, &mut world, undo());
        assert_eq!(y(&world), 5.0, "another gesture, another edit");
        // A gesture going on, which the log's own refusal would leave open.
        edit(&mut authors, &mut world, amend(4, set(id, 7.0)));
        let refused = Edit::Apply {
            author: "editor".to_owned(),
            operations: vec![Operation::Delete {
                entity: "0000000000000001".parse().expect("an ID"),
            }],
        };
        assert!(authors.edit(&mut world, refused).is_err());
        edit(&mut authors, &mut world, amend(4, set(id, 8.0)));
        edit(&mut authors, &mut world, undo());
        assert_eq!(y(&world), 7.0, "a refused edit between ends the gesture");
    }

    /// An amend is kept as a value, as a recording keeps it, and read back as itself.
    #[test]
    fn an_amend_reads_back_as_it_was_written() {
        let (_, id) = world();
        let amend = amend(u64::MAX, set(id, 2.5));
        let value = amend.to_value();
        assert_eq!(Edit::from_value(&value).expect("an edit"), amend);
        let Value::Variant(name, mut fields) = value else {
            panic!("a variant");
        };
        fields[1].1 = Value::Int(3);
        let error = Edit::from_value(&Value::Variant(name, fields)).expect_err("refused");
        assert_eq!(error.code(), "reflect.wrong_kind", "{error}");
    }
}

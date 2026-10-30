//! The edits the engine applies to the world for authors outside the layers: the tool API's
//! clients, and the scene files a game loaded, saved while it runs. Each author undoes and redoes
//! its own, through a [`Log`] of its own, as the Inspector does through its. They apply between
//! ticks, as a frame starts, which a recording keeps as values, the edits as their authors gave
//! them, before the commands of the tick after, and a replay applies there.

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
    /// Undoes `author`'s last edit.
    Undo { author: String },
    /// Redoes the last edit `author` undid.
    Redo { author: String },
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
    logs: Vec<(String, Log)>,
    world: Option<u64>,
}

impl Authors {
    /// Applies `edit` to `world`, and says what it did.
    ///
    /// Fails as [`Log::apply_all`], [`Log::undo`] and [`Log::redo`] do.
    pub(crate) fn edit(&mut self, world: &mut World, edit: Edit) -> Result<Done> {
        if self.world != Some(world.serial()) {
            // Their entries are another world's, which keeps what they kept, if it's still about.
            self.logs.clear();
            self.world = Some(world.serial());
        }
        match edit {
            // Nothing to log, nor any author to list.
            Edit::Apply { operations, .. } if operations.is_empty() => {
                Ok(Done::Applied(Vec::new()))
            }
            Edit::Apply { author, operations } => {
                let log = self.log(author);
                log.apply_all(world, operations)?;
                Ok(Done::Applied(made(log)))
            }
            Edit::Append { author, operations } => {
                let log = self.log(author);
                log.append(world, operations)?;
                Ok(Done::Applied(made(log)))
            }
            Edit::Undo { author } | Edit::Redo { author } if !self.has(&author) => {
                Ok(Done::Stepped(None))
            }
            Edit::Undo { author } => {
                let log = self.log(author);
                let undoable = log.undoable().map(<[Operation]>::to_vec);
                Ok(Done::Stepped(
                    log.undo(world)?.then_some(undoable).flatten(),
                ))
            }
            Edit::Redo { author } => {
                let log = self.log(author);
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
        logs.map(|(author, log)| (author.as_str(), log))
    }

    /// Whether `author` has edited the world.
    fn has(&self, author: &str) -> bool {
        self.logs.iter().any(|(name, _)| name == author)
    }

    /// `author`'s log, which starts empty.
    fn log(&mut self, author: String) -> &mut Log {
        let at = match self.logs.iter().position(|(name, _)| *name == author) {
            Some(at) => at,
            None => {
                self.logs.push((author, Log::default()));
                self.logs.len() - 1
            }
        };
        &mut self.logs[at].1
    }

    /// Drops every log, and what they kept, from `world`, if they're its.
    pub(crate) fn clear(&mut self, world: &mut World) {
        if self.world == Some(world.serial()) {
            for (_, log) in &mut self.logs {
                log.clear(world);
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
    #[cfg(feature = "dev")]
    /// The edit as a value, which a recording keeps: variants `Apply` and `Append`, with its author
    /// and its operations, `Undo` and `Redo`, with their author; each operation a variant named as
    /// its [`Operation`]'s, with its entities' IDs as text.
    pub(crate) fn to_value(&self) -> Value {
        let text = |text: &str| Value::String(text.to_owned());
        let author = |author: &str| ("author".into(), text(author));
        match self {
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
            "Undo" | "Redo" => {
                let [author] = reflect::pick(fields, ["author"])?;
                let author = text(author, "author")?;
                Ok(if name == "Undo" {
                    Self::Undo { author }
                } else {
                    Self::Redo { author }
                })
            }
            other => Err(reflect::no_variant(
                other,
                &["Apply", "Append", "Undo", "Redo"],
            )),
        }
    }
}

#[cfg(feature = "dev")]
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

#[cfg(feature = "dev")]
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

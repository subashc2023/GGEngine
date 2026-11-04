//! The Inspector's commands: what its panels and its link did, which its next tick makes in order,
//! and the values a recording keeps them as.

use gg_core::Result;

use super::{Edit, Step};
use crate::reflect::{self, Ident, Value};
use crate::{Commands, EntityId};

/// What the panels did, which the next tick makes in the order they did it: the Inspector's
/// commands.
pub(super) enum Command {
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
        components: Vec<(Ident, Value)>,
    },
    /// Deletes the entity with this ID, and its descendants.
    Delete(EntityId),
    /// Copies the entity with this ID, and its descendants.
    Duplicate(EntityId),
    /// Gives the entity with this ID a component of the type named, made from the value.
    Add(EntityId, String, Value),
    /// Takes the component of the type named away from the entity with this ID.
    Remove(EntityId, String),
}

/// The Inspector's commands, which a recording reads as values: variants `Set`, with the
/// entity's ID as text, the component and the path, and the value; `Seal`; `Undo`; `Redo`;
/// `Create`, with its parent's ID as text, if it has one, and its components as a struct, a field
/// each named as its type; `Delete` and `Duplicate`, with the entity's ID as text; `Add`, with the
/// entity's ID as text, the component and its value; and `Remove`, with the entity's ID as text
/// and the component.
#[derive(Default)]
pub(super) struct Queue(pub(super) Vec<Command>);

impl Commands for Queue {
    fn len(&self) -> usize {
        self.0.len()
    }

    /// The Inspector's tick keeps its log of what they did, which it undoes and redoes through.
    fn edits(&self) -> bool {
        true
    }

    fn value(&self, i: usize) -> Value {
        let text = |text: &str| Value::String(text.to_owned());
        let entity = |id: &EntityId| -> (Ident, Value) { ("entity".into(), text(&id.to_string())) };
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
            Command::Duplicate(id) => ("Duplicate", vec![entity(id)]),
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
            "Delete" | "Duplicate" => {
                let [entity] = reflect::pick(fields, ["entity"])?;
                let entity = id(entity, "entity")?;
                match name {
                    "Delete" => Command::Delete(entity),
                    _ => Command::Duplicate(entity),
                }
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
                    "Set",
                    "Seal",
                    "Undo",
                    "Redo",
                    "Create",
                    "Delete",
                    "Duplicate",
                    "Add",
                    "Remove",
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

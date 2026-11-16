//! What the panels show: a world by its entities' IDs, a [`World`] or a game's in another process
//! through the tool API; and what waits to change it, which they draw as it will be.

use std::borrow::Cow;
use std::ops::Range;

use super::Edit;
use crate::reflect::{Types, Value};
use crate::{EntityId, World};

/// A world as the panels read it, by its entities' IDs: [`World`]'s own, and the remote
/// Inspector's copy of a game's.
pub(super) trait Inspected {
    fn types(&self) -> &Types;

    /// Component types, in the order they registered.
    fn component_types(&self) -> Box<dyn Iterator<Item = &str> + '_>;

    /// `None` while that isn't known, as for an entity another process has just made.
    fn alive(&self, id: EntityId) -> Option<bool>;

    /// As [`World::name`] says: its [`Name`](crate::Name), or a model's node's name in the model's
    /// file.
    fn name(&self, id: EntityId) -> Option<String>;

    fn parent(&self, id: EntityId) -> Option<EntityId>;

    /// In their order.
    fn children(&self, id: EntityId) -> Box<dyn Iterator<Item = EntityId> + '_>;

    /// Component types, in the order they registered.
    fn components(&self, id: EntityId) -> Vec<&str>;

    /// The component `name`, whole: `None` while it isn't known, `Err` with why it can't be read.
    fn component(&self, id: EntityId, name: &str) -> Option<Result<Cow<'_, Value>, String>>;

    /// What an add of component type `name` gives, or why an add would be refused.
    fn default_of(&self, name: &str) -> Result<Value, String>;

    /// How many entities have component `with`, or how many there are; `Err` says why they can't
    /// be listed.
    fn count(&self, with: Option<&str>) -> Result<usize, String>;

    /// The entities with component `with`, or all, at places `rows` among them, in query order.
    fn page(&self, with: Option<&str>, rows: Range<usize>) -> Vec<EntityId>;

    /// How many entities have no parent.
    fn roots(&self) -> usize;

    /// The entities without a parent at places `at`, in [`World::roots`]' order. A range, so a
    /// page of the tree takes them in one go.
    fn roots_page(&self, at: Range<usize>) -> Vec<EntityId>;

    /// The entity's place among those without a parent, if it's one.
    fn root_index(&self, id: EntityId) -> Option<usize>;

    /// What an entity field holding `value` shows.
    fn entity_label(&self, value: &Value) -> String;

    /// What a texture, model or font field holding `value` shows.
    fn asset_label(&self, value: &Value) -> String;

    /// What differs from its prefab, as the tool API's `entity` has it. `None` for a
    /// non-instance, or while unknown.
    fn instance(&self, id: EntityId) -> Option<Cow<'_, gg_api::Instanced>>;

    /// Whether the Entities panel saves prefabs, like the tool API's `save_prefab`. Only a game's,
    /// through the API: a save writes files, which a replay or lockstep peer of a game's own
    /// Inspector's commands wouldn't have.
    fn saves_prefabs(&self) -> bool {
        false
    }

    /// Why the world isn't shown yet, as while another process is reached.
    fn waiting(&self) -> Option<&str> {
        None
    }
}

impl Inspected for World {
    fn types(&self) -> &Types {
        World::types(self)
    }

    fn component_types(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(World::component_types(self))
    }

    fn alive(&self, id: EntityId) -> Option<bool> {
        Some(self.entity(id).is_some())
    }

    fn name(&self, id: EntityId) -> Option<String> {
        World::name(self, self.entity(id)?).map(str::to_owned)
    }

    fn parent(&self, id: EntityId) -> Option<EntityId> {
        let parent = World::parent(self, self.entity(id)?).ok()??;
        Some(self.id(parent).expect("a parent is alive"))
    }

    fn children(&self, id: EntityId) -> Box<dyn Iterator<Item = EntityId> + '_> {
        let children = self
            .entity(id)
            .and_then(|entity| World::children(self, entity).ok());
        let ids = children.into_iter().flatten();
        Box::new(ids.map(|child| self.id(child).expect("a child is alive")))
    }

    fn components(&self, id: EntityId) -> Vec<&str> {
        let components = self
            .entity(id)
            .and_then(|entity| self.components_of(entity).ok());
        components.into_iter().flatten().collect()
    }

    fn component(&self, id: EntityId, name: &str) -> Option<Result<Cow<'_, Value>, String>> {
        let value = self.get_field(self.entity(id)?, name, "");
        Some(value.map(Cow::Owned).map_err(|error| error.to_string()))
    }

    fn default_of(&self, name: &str) -> Result<Value, String> {
        World::default_of(self, name).map_err(|error| error.to_string())
    }

    fn count(&self, with: Option<&str>) -> Result<usize, String> {
        let entities = self.entities(with).map_err(|error| error.to_string())?;
        Ok(entities.len())
    }

    fn page(&self, with: Option<&str>, rows: Range<usize>) -> Vec<EntityId> {
        let Ok(entities) = self.entities(with) else {
            return Vec::new();
        };
        let page = entities.skip(rows.start).take(rows.len());
        page.map(|entity| self.id(entity).expect("the world lists living entities"))
            .collect()
    }

    fn roots(&self) -> usize {
        World::roots(self).len()
    }

    fn roots_page(&self, at: Range<usize>) -> Vec<EntityId> {
        // A skip steps over whole tables, as `nth` does.
        let page = World::roots(self).skip(at.start).take(at.len());
        page.map(|root| self.id(root).expect("a root is alive"))
            .collect()
    }

    fn root_index(&self, id: EntityId) -> Option<usize> {
        World::root_index(self, self.entity(id)?)
    }

    fn entity_label(&self, value: &Value) -> String {
        match value {
            // A despawned entity has no ID, only its handle.
            &Value::Entity(entity) => match self.id(entity) {
                Some(id) => id.to_string(),
                None => entity.to_string(),
            },
            _ => "?".to_owned(),
        }
    }

    fn asset_label(&self, value: &Value) -> String {
        match *value {
            // Shown by its path; picking another comes with the Assets panel's images.
            Value::Texture(texture) => match self.assets().path(texture) {
                Some(path) => path.to_owned(),
                None => format!("texture {}", texture.index()),
            },
            Value::Model(model) => self.assets().model_path(model).to_owned(),
            Value::Font(font) => self.assets().font_path(font).to_owned(),
            Value::Prefab(prefab) => self.assets().prefab_path(prefab).to_owned(),
            _ => "?".to_owned(),
        }
    }

    /// Found again each frame, parsing the prefab's text: for the picked entity alone.
    fn instance(&self, id: EntityId) -> Option<Cow<'_, gg_api::Instanced>> {
        crate::api::instanced(self, self.entity(id)?).map(Cow::Owned)
    }
}

/// What waits to change what the panels show, which they draw as it will be: the sets of the
/// picked entity's fields, in order; the adds and removes of components (`true` for an add), in
/// order; and the edits Undo and Redo would step through, in words.
#[derive(Default)]
pub(super) struct Pending {
    pub(super) waiting: Vec<Edit>,
    pub(super) changes: Vec<(EntityId, String, bool)>,
    pub(super) undo: Option<String>,
    pub(super) redo: Option<String>,
}

impl Pending {
    /// Whether an add (`Some(true)`) or a remove (`Some(false)`) of component `name` of entity
    /// `id` waits, the last if several do.
    pub(super) fn change(&self, id: EntityId, name: &str) -> Option<bool> {
        let changes = self.changes.iter().rev();
        let mut of = changes.filter(|(entity, component, _)| *entity == id && component == name);
        of.next().map(|&(_, _, adds)| adds)
    }
}

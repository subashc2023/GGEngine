//! Scenes: entities and their components as text, in the canonical form ARCHITECTURE.md
//! describes, so that saving the same entities always gives the same bytes, and changing a field
//! changes only its lines. The engine writes the text itself, and `ron` only reads it, through
//! seeds that reflection's descriptions drive. A prefab is a scene of one subtree, which is placed
//! as a copy whose IDs are mixed from its own. Scene files are written whole or not at all.

mod read;
#[cfg(test)]
pub(super) mod tests;
mod write;

use std::borrow::Cow;
use std::mem;
use std::path::Path;

use gg_core::{Error, Result};

use super::id::{self, EntityId};
use super::table::AnyColumn;
use super::{Entity, Parent, World};
use crate::ModelRoot;
use crate::assets::Asset;
use crate::reflect::{Meta, Show};
use read::Loaded;
pub(crate) use write::{float, quote};

/// The most levels a scene's values nest: a component's value is the first, and each value in a
/// struct, variant, option, list or array is a level deeper than it.
pub(crate) const MAX_DEPTH: u32 = 64;

/// The hints of a component's value, which has no field's.
pub(super) static NO_HINTS: Meta = Meta {
    min: None,
    max: None,
    show: Show::Plain,
    doc: Cow::Borrowed(""),
};

/// Writes the scene `text`, such as [`World::save_scene`] gives, to the file at `path`, whole or
/// not at all: to a file of its own beside it, which it flushes to the disk and then renames over
/// it, so that a save cut short, by a crash or the power going, leaves the last file whole. Through
/// a link it writes the file the link names, which keeps its permissions. It makes no directories.
///
/// Fails with `scene.file`, naming the path, if the file is read only, or can't be written or
/// replaced, removing what it wrote beside it.
pub fn write_scene(path: &Path, text: &str) -> Result<()> {
    let _zone = gg_core::zone!();
    crate::dirs::write_whole(path, text.as_bytes()).map_err(|e| {
        let message = format!("can't save the scene to {}", path.display());
        Error::new("scene.file", message).with_source(e)
    })
}

/// The text of the scene file at `path`, whole, for [`World::load_scene`].
///
/// Fails with `scene.file`, naming the path, if it can't be read, isn't a regular file or isn't
/// UTF-8.
pub fn read_scene(path: &Path) -> Result<String> {
    let _zone = gg_core::zone!();
    let error = || {
        let message = format!("can't read the scene from {}", path.display());
        Error::new("scene.file", message)
    };
    let bytes = crate::dirs::read_file(path, u64::MAX).map_err(|e| error().with_source(e))?;
    String::from_utf8(bytes).map_err(|e| error().with_source(e))
}

impl World {
    /// `roots` and their descendants as a scene's canonical text: a prefab, for one root. Each
    /// component is saved but those that are opaque as a whole, such as a
    /// [`MeshDraw`](crate::MeshDraw), and the fields that are opaque; a texture or a model as its
    /// name, its asset ID, or else its path as [`App::root`](crate::App::root) names it, as
    /// [`Context::add_texture`](crate::Context::add_texture) says; a root's [`Parent`] isn't, since
    /// its parent isn't saved; and a model's nodes aren't, which its root's load spawns again, as
    /// [`ModelRoot`] says. Each component is checked as [`World::set_field`] checks a value,
    /// and made again as a load makes it, so that every scene saved loads.
    ///
    /// Fails with `ecs.no_entity` for a root that isn't alive, or a field that refers to an entity
    /// that isn't; `ecs.model_node` for a root that's a model's node; `scene.model_node` for an
    /// entity under a node that isn't one; `scene.outside` for one that refers to an entity that
    /// isn't saved;
    /// `reflect.opaque` for something opaque in a list or an option, or in a variant or an option
    /// that a load would have to make; `asset.unnamed` for a texture that wasn't loaded from a
    /// file; `scene.bad_name` for a name that isn't an ASCII identifier, or is one of RON's words;
    /// `scene.too_deep` for a value nested more than 64 levels; and as `set_field` does for a value
    /// it refuses. Every error names the entity, the component and the path.
    pub fn save_scene(&self, roots: &[Entity]) -> Result<String> {
        let _zone = gg_core::zone!();
        write::save(self, roots, true)
    }

    /// Exactly `entities` as a scene's canonical text, as [`save_scene`](Self::save_scene) writes
    /// roots but without their descendants: each whose parent isn't among them is written as a
    /// root, so that a child the game made under one of them isn't saved.
    ///
    /// Fails as `save_scene` does.
    #[cfg(feature = "dev")]
    pub(crate) fn save_entities(&self, entities: &[Entity]) -> Result<String> {
        let _zone = gg_core::zone!();
        write::save(self, entities, false)
    }

    /// The IDs the scene `text` gives its entities, as its syntax alone has them, which says what a
    /// text holds that no longer loads, as after a rebuild took away a field it holds.
    ///
    /// Fails with `scene.syntax` for text that isn't a scene's RON, and with `ecs.bad_id` for a key
    /// that isn't an ID.
    #[cfg(feature = "dev")]
    pub(crate) fn scene_ids(text: &str) -> Result<Vec<EntityId>> {
        let _zone = gg_core::zone!();
        read::ids(text)
    }

    /// Spawns the scene `text`'s entities, with the IDs it gives them, and returns its roots, in
    /// the order of their IDs: all of them, or, if it's refused, none. Each parent's children are
    /// in the order of their IDs. Every component type it names must be registered, since a world
    /// knows a type by name once it has; what's opaque, which scenes leave out, comes from
    /// [`Component::base`](crate::Component::base); a texture or a model is found by its name or a
    /// path to its file among those loaded, as this loads none, where `Context::load_scene_file`
    /// loads those a scene names first; a field that isn't given, as in a scene saved before its
    /// type had it, takes its default; and each entity with a [`ModelRoot`] has its model's nodes
    /// spawned under it.
    ///
    /// Fails, changing nothing, with an error that begins with the line and column where the text
    /// is refused: `scene.syntax` for text that isn't a scene's RON, or enables RON's extensions;
    /// `reflect.unknown_type` for a component type that isn't registered; `ecs.bad_id` for an ID
    /// that isn't one; `scene.duplicate_id` for an entity, and `reflect.duplicate_component` for an
    /// entity's component, given twice; `ecs.id_taken` for an ID a living entity has;
    /// `scene.unknown_id` for a reference to an entity the scene doesn't give; `ecs.parent_cycle`
    /// for entities that would be their own ancestors; `asset.no_texture` or `asset.no_model` for
    /// a name or path no texture or model loaded has; `reflect.read_only` for a `ModelNode`, which
    /// the world keeps; `scene.too_deep` for a value nested more than 64 levels; and as
    /// [`World::set_field`] does for a value it refuses, such as `reflect.no_field` for a field the
    /// type hasn't, and with `reflect.missing_field` for one that isn't given and has no default.
    /// Every error but `scene.syntax` names the entity it's about, and the component and the path
    /// if it's about one.
    pub fn load_scene(&mut self, text: &str) -> Result<Vec<Entity>> {
        let _zone = gg_core::zone!();
        let entities = read::parse(self, text, false)?;
        let ids = entities.iter().map(|entity| entity.id).collect();
        Ok(self.commit(entities, ids, None))
    }

    /// The textures and models scene `text` names that none loaded has, each once, in the order
    /// it first names them: those [`load_scene`](Self::load_scene) would refuse it for, which the
    /// engine loads before it loads a scene, as `Context::load_scene_file` does.
    ///
    /// Fails as `load_scene` does into the world without its entities, but for those.
    pub(crate) fn scene_assets(&self, text: &str) -> Result<Vec<Asset<String>>> {
        let _zone = gg_core::zone!();
        read::assets(&self.emptied(), text)
    }

    /// Replaces the world's entities with the scene `text`'s, as [`load_scene`](Self::load_scene)
    /// loads it into the world [cleared](Self::clear), and returns its roots: all of them, or, if
    /// it's refused, none, the world as it was, its serial too. The IDs the world has are no bar,
    /// since its entities go: the scene it was saved to replaces it, under the same IDs. It's then
    /// another world, as `clear` says, which keeps the [`Resource`](crate::Resource)s.
    ///
    /// Fails as `load_scene` does, but for the IDs the world has.
    pub fn replace_scene(&mut self, text: &str) -> Result<Vec<Entity>> {
        let _zone = gg_core::zone!();
        let mut next = self.emptied();
        let roots = next.load_scene(text)?;
        next.resources = mem::take(&mut self.resources);
        *self = next;
        Ok(roots)
    }

    /// Places the prefab `text`, a scene of one subtree, as a copy of it whose root is the last of
    /// `parent`'s children, or a root itself; and returns the copy's root. Each of the copy's IDs
    /// is mixed from the prefab's, and another, drawn for the copy, so that each copy's are its
    /// own; references between the prefab's entities are to the copy's.
    ///
    /// Fails as [`load_scene`](Self::load_scene) does, but for the IDs the world has, which the
    /// copy doesn't keep; with `scene.prefab_roots` for a prefab of more than one root, or of none;
    /// and with `ecs.no_entity` if `parent` isn't alive.
    pub fn place_prefab(&mut self, text: &str, parent: Option<Entity>) -> Result<Entity> {
        let _zone = gg_core::zone!();
        if let Some(parent) = parent {
            self.locate(parent)?;
        }
        let entities = read::parse(self, text, true)?;
        // Drawn once the prefab is known to be placed, so that a refusal draws nothing.
        let ids = loop {
            let instance = self.stream.draw();
            let mixed = entities
                .iter()
                .map(|entity| id::placed(instance, entity.id).filter(|&id| !self.reserved(id)));
            match mixed.collect::<Option<Vec<EntityId>>>() {
                Some(ids) => break ids,
                None => {
                    tracing::debug!(
                        code = "ecs.reseed",
                        instance = %instance,
                        "A prefab's copy would have an ID that's taken"
                    );
                    self.stream.reseed();
                }
            }
        };
        let roots = self.commit(entities, ids, parent);
        Ok(roots[0])
    }

    /// Spawns `entities`, which a parse checked, with `ids`, each under its parent, and the roots
    /// under `parent`; and returns the roots, in the order of their IDs. It can't fail: the parse
    /// checked everything, and predicted the handles they take.
    fn commit(
        &mut self,
        entities: Vec<Loaded>,
        ids: Vec<EntityId>,
        parent: Option<Entity>,
    ) -> Vec<Entity> {
        let parent_id = self
            .component::<Parent>()
            .expect("every world has registered `Parent`");
        let handles: Vec<Entity> = entities.iter().map(|entity| entity.handle).collect();
        // Each child's ID, slot and parent's slot.
        let mut children = Vec::new();
        let mut roots = Vec::new();
        for (entity, id) in entities.into_iter().zip(ids) {
            let Loaded {
                handle,
                mut row,
                parent: up,
                ..
            } = entity;
            match (up, parent) {
                (Some((up, _)), _) => children.push((id, handle.index, handles[up].index)),
                (None, Some(parent)) => {
                    let at = row.partition_point(|&(component, _)| component < parent_id);
                    let column: Box<dyn AnyColumn> = Box::new(vec![Parent(parent)]);
                    row.insert(at, (parent_id, column));
                    children.push((id, handle.index, parent.index));
                    roots.push((id, handle));
                }
                (None, None) => roots.push((id, handle)),
            }
            // In the order the parse first named them, which is the order it predicted their
            // handles in.
            self.spawn_built(id, row, handle);
        }
        // So that each parent's children are in the order of their IDs, whatever the text's.
        children.sort_unstable_by_key(|&(id, ..)| id);
        for (_, child, parent) in children {
            self.append(parent, child);
        }
        // Each model's nodes, which the text leaves out, after its children, which it gave.
        for handle in handles {
            if self.get::<ModelRoot>(handle).is_some() {
                self.fit_nodes(handle, None);
            }
        }
        roots.sort_unstable_by_key(|&(id, _)| id);
        roots.into_iter().map(|(_, root)| root).collect()
    }
}

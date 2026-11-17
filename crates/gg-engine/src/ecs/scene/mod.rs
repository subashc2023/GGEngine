//! Scenes: entities and their components as canonical text, so saving the same entities gives the
//! same bytes and changing a field changes only its lines. The engine writes the text; `ron` only
//! reads it, through seeds driven by reflection's descriptions. Files are written whole or not at
//! all.
//!
//! A prefab is a scene of one subtree. An entity with a [`PrefabRoot`](crate::PrefabRoot) is a
//! linked instance: a scene saves only what differs from its prefab, and the prefab's
//! [template](Template) gives the rest on load. A placed prefab is a copy with IDs mixed from its
//! own.
//!
//! The canonical form:
//!
//! - A scene is `(entities: {...})`: each entity under its ID, as a string, in the order of IDs.
//! - An entity's components go by their types' names, in the order of the names; a component's
//!   fields in its description's order, every field written. No type's name is written, so a
//!   rename touches only its keys.
//! - A struct or a variant with fields, and a list with elements, is a block, a part to a line. A
//!   value of one part, `Some(v)` or a one-field tuple's `(v)`, wraps it, keeping its form. Leaves
//!   are inline: floats as `float` writes them, strings as `quote` escapes them.
//! - A child's `Parent` is its parent's ID; a root's isn't saved, nor an instance's entity's where
//!   it's the one its prefab gives.
//! - A texture, a model, a font or a prefab is saved by its name: its asset ID, or else its path.
//! - What's opaque isn't saved: a load builds each component on
//!   [`Component::base`](crate::Component::base).
//! - Values nest at most [`MAX_DEPTH`] levels.

mod read;
mod template;
#[cfg(test)]
pub(super) mod tests;
mod write;

use std::borrow::Cow;
use std::collections::HashMap;
use std::mem;
use std::path::Path;

use gg_core::{Error, Result};

use super::id::{self, EntityId};
use super::table::AnyColumn;
use super::{Entity, Parent, World};
use crate::ModelRoot;
use crate::assets::Asset;
use crate::package::Files;
use crate::reflect::{Meta, Show};
use read::Loaded;
pub(super) use template::{Expanding, Template};
pub(super) use write::WORDS;
pub(crate) use write::{float, quote};

/// The most levels a scene's values nest: a component's value is the first; each value inside a
/// struct, variant, option, list or array is one deeper.
pub(crate) const MAX_DEPTH: u32 = 64;

/// The hints of a component's value, which has no field's.
pub(super) static NO_HINTS: Meta = Meta {
    min: None,
    max: None,
    show: Show::Plain,
    doc: Cow::Borrowed(""),
};

/// Writes scene `text` (as [`World::save_scene`] gives) to the file at `path`, whole or not at all.
///
/// It writes a temporary file beside it, flushed, then renames it over, so a crash or power loss
/// leaves the last file whole. Through a link it writes the file the link names, keeping its
/// permissions. It makes no directories.
///
/// Fails with `scene.file`, naming the path, if the file is read only or can't be written or
/// replaced; it removes what it wrote beside it.
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
    read_scene_from(path, &Files::default())
}

/// [`read_scene`] from `files`: the disk or the game's package.
pub(crate) fn read_scene_from(path: &Path, files: &Files) -> Result<String> {
    let _zone = gg_core::zone!();
    let error = || {
        let message = format!("can't read the scene from {}", path.display());
        Error::new("scene.file", message)
    };
    let bytes = files
        .read(path, u64::MAX)
        .map_err(|e| error().with_source(e))?;
    String::from_utf8(bytes).map_err(|e| error().with_source(e))
}

impl World {
    /// `roots` and their descendants as canonical scene text; one root makes a prefab.
    ///
    /// - Opaque components (such as [`MeshDraw`](crate::MeshDraw)) and opaque fields aren't saved.
    /// - A texture, model, font or prefab is saved by name: its asset ID, else its path as
    ///   [`App::root`](crate::App::root) names it
    ///   ([`Context::add_texture`](crate::Context::add_texture)).
    /// - A root's [`Parent`] isn't saved, since its parent isn't.
    /// - A model's nodes aren't saved; its root's load respawns them ([`ModelRoot`]).
    /// - An instance whose prefab root is saved is saved as what differs from its prefab
    ///   ([`PrefabRoot`](crate::PrefabRoot)): its root's `PrefabRoot`, `Transform` and other
    ///   differences, and each entity the prefab gives where something differs, a named-field
    ///   struct as its differing fields. Its prefab's entities go with it, like a model's nodes.
    ///
    /// Each component is checked as [`World::set_field`] checks a value, and rebuilt as a load
    /// builds it, so every saved scene loads.
    ///
    /// Fails with:
    ///
    /// - `ecs.no_entity` for a root that isn't alive, or a field referring to an entity that isn't.
    /// - `ecs.model_node` for a root that's a model's node; `scene.model_node` for an entity under
    ///   a node that isn't one.
    /// - `scene.prefab_missing` for an instance missing an entity or component its prefab gives,
    ///   as code took away.
    /// - `scene.prefab_parent` for an entity an instance gives that code made a root or put under
    ///   an unsaved one; a load would put it back under its prefab's parent.
    /// - The error of an instance's prefab's template failing to load.
    /// - `scene.outside` for a reference to an unsaved entity.
    /// - `reflect.opaque` for opaque state in a list or option, or in a variant or option a load
    ///   would have to make.
    /// - `asset.unnamed` for a texture not loaded from a file.
    /// - `scene.bad_name` for a name that isn't an ASCII identifier, or is one of RON's words.
    /// - `scene.too_deep` for a value nested more than 64 levels.
    /// - `set_field`'s errors for a value it refuses.
    ///
    /// Every error names the entity, the component and the path.
    pub fn save_scene(&self, roots: &[Entity]) -> Result<String> {
        let _zone = gg_core::zone!();
        write::save(self, roots, true)
    }

    /// Like [`save_scene`](Self::save_scene), but exactly `entities`, without descendants: one
    /// whose parent isn't among them is written as a root, so a child the game made under one isn't
    /// saved.
    ///
    /// Fails like `save_scene`.
    #[cfg(feature = "dev")]
    pub(crate) fn save_entities(&self, entities: &[Entity]) -> Result<String> {
        let _zone = gg_core::zone!();
        write::save(self, entities, false)
    }

    /// `root` and its descendants as canonical prefab text. Linking `root` to it
    /// ([`Operation::Link`](crate::Operation::Link)) makes `root` an instance that keeps its
    /// entities as they are, under their IDs.
    ///
    /// - Each of the prefab's own entities, `root` too, gets the local ID that the instance rooted
    ///   at `root` mixes to its ID ([`id::unlinked`]).
    /// - Each inner instance's IDs are mixed again from its root's, as a prefab's text has them.
    /// - Each is saved as [`save_scene`](Self::save_scene) saves it, but the root's `Transform`
    ///   (the instance's own) is the identity, so the prefab sits at its root.
    ///
    /// Fails like `save_scene` (`scene.outside` for a reference outside the subtree), and with:
    ///
    /// - `ecs.prefab_entity` for a root an instance's prefab gives.
    /// - `scene.prefab_variant` for an instance's root: a prefab whose root links another is
    ///   refused.
    /// - `ecs.bad_id` for the one ID no local ID mixes to, and for two entities that would get
    ///   the same local ID, which only chance gives.
    /// - The error of linking `root` to the text ([`linkable`](Self::linkable)), so a save that
    ///   couldn't link writes nothing.
    #[cfg(feature = "dev")]
    pub(crate) fn save_prefab(&self, root: Entity) -> Result<String> {
        let _zone = gg_core::zone!();
        self.locate(root)?;
        self.not_a_node(root)?;
        self.not_a_prefabs(root)?;
        if self.get::<crate::PrefabRoot>(root).is_some() {
            return Err(Error::new(
                "scene.prefab_variant",
                format!(
                    "entity {} is an instance's root, and a prefab whose root links another is \
                     refused: unlink it first",
                    self.name_of(root)
                ),
            ));
        }
        let text = write::save(self, &[root], true)?;
        let mut scratch = self.scratch();
        let entities = read::parse(&scratch, &text, true, &mut Expanding::default())?;
        let held = self.id(root).expect("alive");
        let unmixable = |id: EntityId| {
            let message = format!(
                "entity {id} has the one ID that no local ID mixes to under entity {held}: give it \
                 another ID first"
            );
            Error::new("ecs.bad_id", message)
        };
        let ids = (0..entities.len())
            .map(|at| local_id(&entities, held, at).ok_or_else(|| unmixable(entities[at].id)));
        let ids: Vec<EntityId> = ids.collect::<Result<_>>()?;
        let distinct: std::collections::HashSet<EntityId> = ids.iter().copied().collect();
        if distinct.len() != ids.len() {
            let message = format!(
                "two entities under entity {held} would have the same ID in the prefab, by chance \
                 of about one in 2^64: give one another ID first"
            );
            return Err(Error::new("ecs.bad_id", message));
        }
        let roots = scratch.commit(entities, ids, None);
        if scratch.get::<crate::Transform>(roots[0]).is_some() {
            let identity = crate::reflect::Reflect::to_value(&crate::Transform::IDENTITY);
            scratch.set_field(roots[0], "Transform", "", &identity)?;
        }
        let text = write::save(&scratch, &roots, true)?;
        let template = Expanding::default().text_template(self, &text)?;
        self.linkable(root, template, "the prefab saved")?;
        Ok(text)
    }

    /// The entity IDs of scene `text` by syntax alone: what a text holds that no longer loads, as
    /// after a rebuild removed a field it holds.
    ///
    /// Fails with `scene.syntax` for text that isn't a scene's RON, `ecs.bad_id` for a key that
    /// isn't an ID.
    pub(crate) fn scene_ids(text: &str) -> Result<Vec<EntityId>> {
        let _zone = gg_core::zone!();
        read::ids(text)
    }

    /// Spawns scene `text`'s entities under the IDs it gives and returns its roots in ID order:
    /// all of them, or none if it's refused. Each parent's children are in ID order.
    ///
    /// - Every component type named must be registered.
    /// - What's opaque, which scenes leave out, comes from
    ///   [`Component::base`](crate::Component::base).
    /// - A texture, model, font or prefab is found by name or file path among those loaded. This
    ///   loads none; `Context::load_scene_file` first loads those a scene names.
    /// - A field not given, as in a scene saved before its type had it, takes its default.
    /// - An entity with a [`PrefabRoot`](crate::PrefabRoot) is an instance: its prefab's entities
    ///   spawn under it with IDs mixed from its own, and a field an instance's entity isn't given
    ///   takes its prefab's.
    /// - An entity with a [`ModelRoot`] gets its model's nodes spawned under it.
    ///
    /// Fails, changing nothing, with an error that begins with the line and column where the text
    /// is refused:
    ///
    /// - `scene.syntax` for text that isn't a scene's RON, or enables RON's extensions.
    /// - `reflect.unknown_type` for a component type that isn't registered.
    /// - `ecs.bad_id` for an ID that isn't one.
    /// - `scene.duplicate_id` for an entity given twice, and `reflect.duplicate_component` for an
    ///   entity's component given twice.
    /// - `ecs.id_taken` for an ID a living entity has.
    /// - `scene.unknown_id` for a reference to an entity the scene doesn't give.
    /// - `ecs.parent_cycle` for entities that would be their own ancestors.
    /// - `asset.no_texture`, `asset.no_model`, `asset.no_font` or `asset.no_prefab` for a name or
    ///   path no texture, model, font or prefab loaded has.
    /// - `scene.prefab_cycle` for a prefab that links itself, `scene.prefab_variant` for one whose
    ///   root links another, and the error of a prefab's text that fails to load, naming the
    ///   prefab.
    /// - `scene.prefab_entity` for an instance's entity given a `PrefabRoot` of its own.
    /// - `scene.duplicate_id` also for an entity two instances' prefabs give.
    /// - `reflect.read_only` for a `ModelNode`, which the world keeps.
    /// - `scene.too_deep` for a value nested more than 64 levels.
    /// - [`World::set_field`]'s errors for a value it refuses, such as `reflect.no_field` for a
    ///   field the type hasn't, and `reflect.missing_field` for a missing field with no default.
    ///
    /// Every error but `scene.syntax` names the entity it's about, and the component and the path
    /// if it's about one.
    pub fn load_scene(&mut self, text: &str) -> Result<Vec<Entity>> {
        let _zone = gg_core::zone!();
        let entities = read::parse(self, text, false, &mut Expanding::default())?;
        let ids = entities.iter().map(|entity| entity.id).collect();
        Ok(self.commit(entities, ids, None))
    }

    /// Parses scene `text` as [`load_scene`](Self::load_scene) does, spawning nothing. Call it on
    /// any thread, on a [scratch](Self::scratch) world with the types and assets of the world that
    /// will [`commit_scene`](Self::commit_scene).
    ///
    /// Fails like `load_scene`, except for taken IDs, which `commit_scene` checks.
    pub(crate) fn parse_scene(&self, text: &str) -> Result<Parsed> {
        let _zone = gg_core::zone!();
        let entities = read::parse(self, text, false, &mut Expanding::default())?;
        Ok(Parsed { entities })
    }

    /// Spawns `parsed` ([`parse_scene`](Self::parse_scene)'d in a world of this one's types and
    /// assets) as [`load_scene`](Self::load_scene) would have, and returns its roots. Entities get
    /// this world's handles and references between them are retargeted, so the world is as the
    /// load would have left it.
    ///
    /// Fails, spawning nothing, with `ecs.id_taken` for an entity whose ID a living entity has.
    pub(crate) fn commit_scene(&mut self, parsed: Parsed) -> Result<Vec<Entity>> {
        let _zone = gg_core::zone!();
        let Parsed { mut entities } = parsed;
        if let Some(taken) = entities
            .iter()
            .find(|entity| self.entity(entity.id).is_some())
        {
            let message = format!(
                "entity {} is in the world already, but a scene's entities are new: a load keeps \
                 the IDs it gives",
                taken.id
            );
            return Err(Error::new("ecs.id_taken", message));
        }
        // In the order the parse predicted the handles in, which `commit` spawns them in.
        let handles: HashMap<Entity, Entity> = {
            let mut slots = self.next_slots();
            let mut next = || slots.next().expect("there's always another slot");
            entities
                .iter()
                .map(|entity| (entity.handle, next()))
                .collect()
        };
        for entity in &mut entities {
            entity.handle = handles[&entity.handle];
            for (_, column) in &mut entity.row {
                column.entities_mut(0, &mut |held| {
                    if let Some(&handle) = handles.get(held) {
                        *held = handle;
                    }
                });
            }
        }
        let ids = entities.iter().map(|entity| entity.id).collect();
        Ok(self.commit(entities, ids, None))
    }

    /// The textures, models, fonts and prefabs scene `text` names that none loaded has, each once,
    /// in first-named order, then those the prefabs it names name in turn: what
    /// [`load_scene`](Self::load_scene) would refuse it for, and `Context::load_scene_file` loads
    /// first.
    ///
    /// Fails like `load_scene` into the world without its entities, except for those.
    pub(crate) fn scene_assets(&self, text: &str) -> Result<Vec<Asset<String>>> {
        let _zone = gg_core::zone!();
        read::assets(&self.emptied(), text)
    }

    /// Replaces the world's entities with scene `text`'s, as [`load_scene`](Self::load_scene) would
    /// into the [cleared](Self::clear) world, and returns its roots: all of them, or none if it's
    /// refused, leaving the world as it was, serial included. The world's IDs are no bar, since its
    /// entities go. It's then another world ([`clear`](Self::clear)) that keeps the
    /// [`Resource`](crate::Resource)s.
    ///
    /// Fails like `load_scene`, except for taken IDs.
    pub fn replace_scene(&mut self, text: &str) -> Result<Vec<Entity>> {
        let _zone = gg_core::zone!();
        let mut next = self.emptied();
        let roots = next.load_scene(text)?;
        next.resources = mem::take(&mut self.resources);
        *self = next;
        Ok(roots)
    }

    /// Places the prefab `text`, a scene of one subtree, as an unlinked copy whose root is the last
    /// of `parent`'s children, or a root itself, and returns the copy's root. Each of the copy's
    /// IDs is mixed from the prefab's and another drawn for the copy, so that each copy's are its
    /// own; an instance in it mixes its IDs from its root's copy's. References between the
    /// prefab's entities are to the copy's.
    ///
    /// Fails as [`load_scene`](Self::load_scene) does, but for the IDs the world has, which the
    /// copy doesn't keep; with `scene.prefab_roots` for a prefab of more than one root, or of none;
    /// and with `ecs.no_entity` if `parent` isn't alive.
    pub fn place_prefab(&mut self, text: &str, parent: Option<Entity>) -> Result<Entity> {
        let _zone = gg_core::zone!();
        if let Some(parent) = parent {
            self.locate(parent)?;
        }
        let entities = read::parse(self, text, true, &mut Expanding::default())?;
        // Drawn once the prefab is known to be placed, so that a refusal draws nothing.
        let ids = loop {
            let instance = self.stream.draw();
            let mixed = copied_ids(&entities, instance);
            let free = |ids: &Vec<EntityId>| ids.iter().all(|&id| !self.reserved(id));
            match mixed.filter(free) {
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

    /// Spawns `entities`, which a parse checked, with `ids`, each under its parent and the roots
    /// under `parent`, and returns the roots in the order of their IDs. It can't fail: the parse
    /// checked everything and predicted the handles.
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

/// A scene's entities parsed apart from their world by [`World::parse_scene`], under the handles
/// of the world that parsed them, for [`World::commit_scene`].
pub(crate) struct Parsed {
    entities: Vec<Loaded>,
}

/// The IDs of the copy of a prefab's `entities`, placed as `instance`, as
/// [`place_prefab`](World::place_prefab) gives them: each of the prefab's own mixed from the copy's
/// and its own, as [`id::placed`] mixes it; each an instance in it gives mixed from its root's
/// copy's. `None` where one mixes to zero.
fn copied_ids(entities: &[Loaded], instance: EntityId) -> Option<Vec<EntityId>> {
    fn id_of(
        entities: &[Loaded],
        instance: EntityId,
        at: usize,
        ids: &mut [Option<EntityId>],
    ) -> Option<EntityId> {
        if let Some(id) = ids[at] {
            return Some(id);
        }
        let id = match entities[at].spawned {
            None => id::placed(instance, entities[at].id)?,
            Some((by, local)) => id::linked(id_of(entities, instance, by, ids)?, local)?,
        };
        ids[at] = Some(id);
        Some(id)
    }
    let mut ids = vec![None; entities.len()];
    (0..entities.len())
        .map(|at| id_of(entities, instance, at, &mut ids))
        .collect()
}

/// The ID in a prefab of the entity at `at` among `entities`, a subtree's whose root's ID is
/// `root`, as [`World::save_prefab`] gives it: the local ID that the instance whose root is `root`
/// mixes to its ID, as [`id::unlinked`] finds it; for an instance's entity, mixed from its root's
/// in the prefab. `None` where one would be 0.
#[cfg(feature = "dev")]
fn local_id(entities: &[Loaded], root: EntityId, at: usize) -> Option<EntityId> {
    match entities[at].spawned {
        None => id::unlinked(root, entities[at].id),
        Some((by, local)) => id::linked(local_id(entities, root, by)?, local),
    }
}

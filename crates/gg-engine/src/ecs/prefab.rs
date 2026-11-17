//! Prefabs' instances in a world: the entities spawned under each [`PrefabRoot`]. Their IDs mix
//! from the root's, so its ID gives them wherever it's loaded, made or restored. Also which
//! entities an instance gives, what its prefab gives each, and, in development builds, what they
//! take as the prefab's file changes.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
#[cfg(feature = "dev")]
use std::sync::Arc;

use gg_core::{Error, Result};

use super::scene::{Expanding, Template};
use super::table::{AnyColumn, ComponentId};
use super::{Entity, EntityId, Parent, World};
use crate::reflect::{Shape, Type, Value};
use crate::{ModelRoot, Prefab, PrefabRoot, Transform};

/// An instance of a prefab in a world: its template, and its parts' IDs, the root's first.
pub(super) struct Instance {
    pub(super) template: Rc<Template>,
    pub(super) ids: Vec<EntityId>,
}

impl Instance {
    /// Part `part`'s `component` as the prefab gives it, in a one-row column. References to the
    /// prefab's entities point to the instance's in `world`, or to none if not alive. `None` if
    /// the prefab gives none.
    pub(super) fn column(
        &self,
        world: &World,
        part: usize,
        component: ComponentId,
    ) -> Option<Box<dyn AnyColumn>> {
        let row = &self.template.parts[part].row;
        let (_, column) = row.iter().find(|&&(of, _)| of == component)?;
        let mut column = column.cloned();
        let handles = self.handles(world);
        column.entities_mut(0, &mut |entity| {
            *entity = handles.get(entity).copied().unwrap_or(Entity::DEAD);
        });
        Some(column)
    }

    /// [`column`](Self::column)'s value.
    pub(super) fn value(
        &self,
        world: &World,
        part: usize,
        component: ComponentId,
    ) -> Option<Value> {
        let column = self.column(world, part, component)?;
        Some(column.value(0))
    }

    /// The component types the prefab gives part `part`, but not the root's `Transform`, which is
    /// the instance's own.
    pub(super) fn components(&self, world: &World, part: usize) -> Vec<ComponentId> {
        let transform = world.component::<Transform>();
        let row = self.template.parts[part].row.iter();
        let row = row.filter(|&&(component, _)| part > 0 || Some(component) != transform);
        row.map(|&(component, _)| component).collect()
    }

    fn handles(&self, world: &World) -> HashMap<Entity, Entity> {
        let parts = self.template.parts.iter().zip(&self.ids);
        let handles = parts.map(|(part, &id)| (part.handle, world.entity(id)));
        handles
            .map(|(handle, entity)| (handle, entity.unwrap_or(Entity::DEAD)))
            .collect()
    }

    /// Part `part`'s parent as an entity of `world`, if alive; none for the root.
    pub(super) fn parent(&self, world: &World, part: usize) -> Option<Entity> {
        let up = self.template.parts[part].parent?;
        world.entity(self.ids[up])
    }
}

/// The entities instances give in part of a world, by ID, with instance and part: each by the
/// outermost instance giving it, as the prefabs inside it do.
#[derive(Default)]
pub(super) struct Claims {
    pub(super) instances: Vec<(Entity, Instance)>,
    by_id: HashMap<EntityId, (usize, usize)>,
}

impl Claims {
    /// The instance giving `id` (its place in `instances`) and the entity's template part.
    pub(super) fn of(&self, id: EntityId) -> Option<(usize, usize)> {
        self.by_id.get(&id).copied()
    }

    pub(super) fn has(&self, id: EntityId) -> bool {
        self.by_id.contains_key(&id)
    }

    pub(super) fn claim(&mut self, root: Entity, instance: Instance) {
        let at = self.instances.len();
        for (part, &id) in instance.ids.iter().enumerate().skip(1) {
            self.by_id.insert(id, (at, part));
        }
        self.instances.push((root, instance));
    }
}

/// What of an instance's entity differs from its prefab, per [`World::overrides`]:
///
/// - the outermost instance whose prefab gives it, by root and prefab; for an instance's root
///   no instance gives, its own;
/// - each differing prefab component in table order, with the differing field names for a struct
///   of named fields, as a scene saves them, else `""` for the whole component;
/// - each component the prefab doesn't give it, which a scene saves whole, except the
///   instance's own (a root's `Transform` and `PrefabRoot`, a `Parent`) and wholly opaque ones,
///   which scenes don't save.
#[cfg(feature = "dev")]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Overrides {
    pub(crate) root: EntityId,
    pub(crate) prefab: Prefab,
    pub(crate) fields: Vec<(String, Vec<String>)>,
    pub(crate) added: Vec<String>,
}

/// What instances take as their prefab's file changes, per [`World::refits`]: each outermost
/// instance as the prefab gave it and gives it now.
#[cfg(feature = "dev")]
pub(crate) struct Refits {
    /// The serial of the only world that takes it.
    world: u64,
    instances: Vec<(Instance, Instance)>,
}

impl World {
    /// Like [`Prefab::spawn`].
    pub(crate) fn spawn_prefab(&mut self, prefab: Prefab, at: Transform) -> Result<Entity> {
        assert!(
            self.assets.has_prefab(prefab),
            "a prefab spawns into a world that shares the assets of the context that added it"
        );
        let template = Expanding::default().template(self, prefab)?;
        // Drawn until its entities' IDs are free too, as a model's are.
        let (id, ids) = loop {
            let id = self.stream.draw();
            let ids = template.ids(id).ok();
            let ids = ids.filter(|ids| ids.iter().all(|&id| !self.reserved(id)));
            if let Some(ids) = ids {
                break (id, ids);
            }
            tracing::debug!(
                code = "ecs.reseed",
                id = %id,
                "Drew an ID that's taken, or that an entity of the prefab would take"
            );
            self.stream.reseed();
        };
        let root = self.spawn_as(Some(id), at);
        self.put(root, PrefabRoot(prefab)).expect("spawned");
        self.spawn_instance(root, &Instance { template, ids });
        Ok(root)
    }

    /// The instance rooted at live `root`, of its [`PrefabRoot`]'s prefab, with the template
    /// `expanding` makes; `None` without one.
    ///
    /// Fails like [`Expanding::template`], and with `ecs.bad_id` for the one ID an entity of it
    /// mixes to zero.
    pub(super) fn instance(
        &self,
        root: Entity,
        expanding: &mut Expanding,
    ) -> Result<Option<Instance>> {
        let Some(&PrefabRoot(prefab)) = self.get::<PrefabRoot>(root) else {
            return Ok(None);
        };
        let template = expanding.template(self, prefab)?;
        let ids = template.ids(self.id(root).expect("alive"))?;
        Ok(Some(Instance { template, ids }))
    }

    /// What instances in live `tops`' subtrees give, each by the outermost; a parent's instance
    /// comes before its children's.
    ///
    /// Fails like [`instance`](Self::instance).
    pub(super) fn claims(&self, tops: &[Entity], expanding: &mut Expanding) -> Result<Claims> {
        let mut claims = Claims::default();
        let mut next: Vec<Entity> = tops.iter().rev().copied().collect();
        while let Some(entity) = next.pop() {
            let id = self.id(entity).expect("alive");
            if !claims.has(id)
                && let Some(instance) = self.instance(entity, expanding)?
            {
                claims.claim(entity, instance);
            }
            let children: Vec<Entity> = self.children(entity).expect("alive").collect();
            next.extend(children.into_iter().rev());
        }
        Ok(claims)
    }

    /// The outermost instance whose prefab gives live `entity`, with its root and the entity's
    /// template part. `None` if none does, as for an instance's root.
    ///
    /// Fails like [`instance`](Self::instance), for an ancestor's instance.
    pub(super) fn claimer(
        &self,
        entity: Entity,
        expanding: &mut Expanding,
    ) -> Result<Option<(Entity, Instance, usize)>> {
        let id = self.id(entity).expect("alive");
        let mut above = Vec::new();
        let mut at = entity;
        while let Some(up) = self.parent(at).expect("alive") {
            above.push(up);
            at = up;
        }
        let mut claimed = HashSet::new();
        for &root in above.iter().rev() {
            if claimed.contains(&self.id(root).expect("alive")) {
                continue;
            }
            let Some(instance) = self.instance(root, expanding)? else {
                continue;
            };
            if let Some(part) = instance.ids.iter().position(|&of| of == id) {
                return Ok(Some((root, instance, part)));
            }
            claimed.extend(instance.ids[1..].iter().copied());
        }
        Ok(None)
    }

    /// The entity tools change in place of live `entity`: the root of its model or outermost
    /// instance, in turn; `entity` itself if neither. An instance whose template doesn't load
    /// gives none.
    #[cfg(feature = "dev")]
    pub(crate) fn owner(&self, entity: Entity) -> Entity {
        let mut at = self.node_root(entity);
        let mut expanding = Expanding::default();
        while let Ok(Some((root, _, _))) = self.claimer(at, &mut expanding) {
            at = self.node_root(root);
        }
        at
    }

    /// Fails with `ecs.prefab_entity` if an instance's prefab gives live `entity` (per
    /// [`claimer`](Self::claimer)): tools can't delete or move it out of its instance.
    pub(super) fn not_a_prefabs(&self, entity: Entity) -> Result<()> {
        let Some((root, _, _)) = self.claimer(entity, &mut Expanding::default())? else {
            return Ok(());
        };
        Err(self.prefabs_own(entity, root, "it"))
    }

    /// The `ecs.prefab_entity` error: `what` of `entity` is its prefab's, given by the instance
    /// rooted at `root`.
    pub(super) fn prefabs_own(&self, entity: Entity, root: Entity, what: &str) -> Error {
        let prefab = self
            .get::<PrefabRoot>(root)
            .map(|&PrefabRoot(prefab)| prefab);
        let prefab = prefab.filter(|&prefab| self.assets.has_prefab(prefab));
        let path = prefab.map_or("", |prefab| self.assets.prefab_path(prefab));
        Error::new(
            "ecs.prefab_entity",
            format!(
                "{what} is prefab `{path}`'s, of entity {}, which the instance whose root is \
                 entity {} gives: change it, or the prefab's file, or unlink the instance first",
                self.name_of(entity),
                self.name_of(root)
            ),
        )
    }

    /// `entity`'s `component` at `path` as its prefab has it: for an instance's root the prefab
    /// root's, but its `Transform`; for a given entity as the outermost instance's prefab gives
    /// it, with the prefabs inside. References to the prefab's entities point to the instance's.
    ///
    /// Fails:
    ///
    /// - with `ecs.no_prefab` for an entity no instance gives that's no instance's root, or a
    ///   component its prefab doesn't give it;
    /// - like [`claimer`](Self::claimer);
    /// - like [`get_field`](Self::get_field), for a path the component hasn't.
    pub(super) fn prefab_value(
        &self,
        entity: Entity,
        component: &str,
        path: &str,
    ) -> Result<Value> {
        let mut expanding = Expanding::default();
        let (instance, part) = match self.claimer(entity, &mut expanding)? {
            Some((_, instance, part)) => (instance, part),
            None => match self.instance(entity, &mut expanding)? {
                Some(instance) => (instance, 0),
                None => {
                    return Err(Error::new(
                        "ecs.no_prefab",
                        format!(
                            "entity {} is neither an instance's root nor an entity an instance's \
                             prefab gives",
                            self.name_of(entity)
                        ),
                    ));
                }
            },
        };
        let id = self.named(component)?;
        let given = instance.components(self, part).contains(&id);
        let column = given.then(|| instance.column(self, part, id)).flatten();
        let Some(column) = column else {
            return Err(Error::new(
                "ecs.no_prefab",
                format!(
                    "entity {}'s prefab gives it no `{component}`, which is its own",
                    self.name_of(entity)
                ),
            ));
        };
        // Read through a world of one entity of it, as a path reads a component.
        let mut scratch = self.scratch();
        let held = scratch.take_slot();
        scratch.find_by_id(EntityId::new(1).expect("not zero"), held);
        scratch.place(
            held,
            EntityId::new(1).expect("not zero"),
            vec![(id, column)],
        );
        scratch.get_field(held, component, path)
    }

    /// What of live `entity` differs from its prefab, per [`Overrides`] and a scene save's
    /// comparison; `None` for an entity no instance gives that's no instance's root.
    ///
    /// Fails like [`claimer`](Self::claimer), as for a prefab whose text no longer loads.
    #[cfg(feature = "dev")]
    pub(crate) fn overrides(&self, entity: Entity) -> Result<Option<Overrides>> {
        let mut expanding = Expanding::default();
        let (root, instance, part) = match self.claimer(entity, &mut expanding)? {
            Some(claimed) => claimed,
            None => match self.instance(entity, &mut expanding)? {
                Some(instance) => (entity, instance, 0),
                None => return Ok(None),
            },
        };
        let &PrefabRoot(prefab) = self.get::<PrefabRoot>(root).expect("an instance's root");
        let given = instance.components(self, part);
        let own = [
            self.component::<Parent>(),
            self.component::<Transform>().filter(|_| part == 0),
            self.component::<PrefabRoot>().filter(|_| part == 0),
        ];
        let at = self.location(entity).expect("alive");
        let table = &self.tables[at.table as usize];
        let mut fields = Vec::new();
        let mut added = Vec::new();
        for (column, &component) in table.components().iter().enumerate() {
            if own.contains(&Some(component))
                || self.components.ty(component).is_opaque(self.types())
            {
                continue;
            }
            let name = self.components.name(component).to_owned();
            if !given.contains(&component) {
                added.push(name);
                continue;
            }
            let now = table.column_at(column).value(at.row as usize);
            let was = instance.value(self, part, component).expect("given");
            if super::diff::same(&now, self, &was, self) {
                continue;
            }
            let differ = match (&now, &was) {
                (Value::Struct(now), Value::Struct(was))
                    if self.named_fields(component) && now.len() == was.len() =>
                {
                    let differ = now.iter().zip(was);
                    let differ = differ
                        .filter(|((_, now), (_, was))| !super::diff::same(now, self, was, self));
                    differ.map(|((field, _), _)| field.to_string()).collect()
                }
                _ => vec![String::new()],
            };
            fields.push((name, differ));
        }
        Ok(Some(Overrides {
            root: self.id(root).expect("alive"),
            prefab,
            fields,
            added,
        }))
    }

    /// Whether `component` is a struct of named fields, saved field by field for an instance's
    /// entity.
    pub(super) fn named_fields(&self, component: ComponentId) -> bool {
        let Type::Named(index) = self.components.ty(component) else {
            return false;
        };
        let shape = &self.types().info(index).shape;
        matches!(shape, Shape::Struct { tuple: false, .. })
    }

    /// Links live `root` to `prefab` ([`Operation::Link`](super::Operation::Link)): gives it a
    /// [`PrefabRoot`], spawning nothing. Its subtree already holds the prefab's entities under
    /// the IDs the instance mixes, as an unlink or a save as a prefab left them.
    ///
    /// Fails, changing nothing:
    ///
    /// - with `reflect.has_component` if `root` has a `PrefabRoot`;
    /// - with `asset.no_prefab` for a prefab another context added;
    /// - like [`Expanding::template`], if the template fails to load;
    /// - like [`linkable`](Self::linkable).
    pub(super) fn link(&mut self, root: Entity, prefab: Prefab) -> Result<()> {
        if self.get::<PrefabRoot>(root).is_some() {
            return Err(Error::new(
                "reflect.has_component",
                "it has a `PrefabRoot` already: it's an instance's root, which an unlink makes an \
                 entity of its own",
            ));
        }
        if !self.assets.has_prefab(prefab) {
            let message = "it names a prefab the world's assets haven't, as another context's";
            return Err(Error::new("asset.no_prefab", message));
        }
        let template = Expanding::default().template(self, prefab)?;
        let named = format!("prefab `{}`", self.assets.prefab_path(prefab));
        self.linkable(root, template, &named)?;
        self.put(root, PrefabRoot(prefab)).expect("alive");
        Ok(())
    }

    /// Fails unless live `root`'s subtree holds what an instance of `template`'s prefab gives
    /// (messages name it `prefab`), so linking spawns nothing:
    ///
    /// - `ecs.bad_id` for the one ID an entity mixes to zero;
    /// - `ecs.prefab_missing` for a given entity that isn't alive, or lacks a component given it
    ///   (the root too);
    /// - `ecs.prefab_parent` for a given entity not under its prefab parent;
    /// - `ecs.prefab_entity` for one with a `PrefabRoot` the prefab doesn't give it, which a scene
    ///   saves and a load refuses.
    pub(super) fn linkable(
        &self,
        root: Entity,
        template: Rc<Template>,
        prefab: &str,
    ) -> Result<()> {
        let ids = template.ids(self.id(root).expect("alive"))?;
        let instance = Instance { template, ids };
        let link = self.component::<PrefabRoot>();
        for (part, &id) in instance.ids.iter().enumerate() {
            let missing = |what: String| {
                let message = format!(
                    "{prefab} gives {what}, which isn't there to link: its subtree holds the \
                     entities and components a prefab gives, under the IDs it mixes for them, to \
                     link"
                );
                Error::new("ecs.prefab_missing", message)
            };
            let Some(entity) = self.entity(id) else {
                return Err(missing(format!("entity {id}")));
            };
            if part > 0 && self.parent(entity).expect("alive") != instance.parent(self, part) {
                let message = format!(
                    "{prefab} gives entity {id} under another parent than it has: a prefab's \
                     entities are linked under the parents it gives them"
                );
                return Err(Error::new("ecs.prefab_parent", message));
            }
            let at = self.location(entity).expect("alive");
            let table = &self.tables[at.table as usize];
            if let Some(&gone) =
                (instance.components(self, part).iter()).find(|&&component| !table.has(component))
            {
                let name = self.components.name(gone);
                return Err(missing(format!("entity {id} a `{name}`")));
            }
            if part > 0
                && let Some(&held) = self.get::<PrefabRoot>(entity)
                && (link.and_then(|link| instance.column(self, part, link)))
                    .is_none_or(|given| given.vec::<PrefabRoot>()[0] != held)
            {
                let message = format!(
                    "{prefab} gives entity {id} another `PrefabRoot` than it has, or none: an \
                     instance's entities link the prefabs its prefab links them to, so take its \
                     `PrefabRoot` away or unlink it first"
                );
                return Err(Error::new("ecs.prefab_entity", message));
            }
        }
        Ok(())
    }

    /// Gives live `root` (which has `instance`'s ID and `PrefabRoot`) the prefab root's other
    /// components, but not its `Transform`, and spawns the prefab's other entities under it with
    /// `instance`'s IDs, which no living entity has. Children spawn in ID order, as a scene's
    /// load does, with references retargeted to the instance's, and models' nodes. Returns the
    /// component types given the root and the IDs of the spawned children.
    pub(super) fn spawn_instance(
        &mut self,
        root: Entity,
        instance: &Instance,
    ) -> (Vec<ComponentId>, Vec<EntityId>) {
        let template = &instance.template;
        let mut handles = vec![root];
        for &id in &instance.ids[1..] {
            let entity = self.take_slot();
            self.find_by_id(id, entity);
            handles.push(entity);
        }
        let by_handle: HashMap<Entity, Entity> = (template.parts.iter())
            .map(|part| part.handle)
            .zip(handles.iter().copied())
            .collect();
        let retarget = |column: &mut dyn AnyColumn| {
            column.entities_mut(0, &mut |entity| {
                *entity = by_handle.get(entity).copied().unwrap_or(Entity::DEAD);
            });
        };
        let mut given = Vec::new();
        for component in instance.components(self, 0) {
            let at = self.location(root).expect("alive");
            if self.tables[at.table as usize].has(component) {
                continue;
            }
            let (_, column) = (template.parts[0].row.iter())
                .find(|&&(of, _)| of == component)
                .expect("the root's");
            let mut column = column.cloned();
            retarget(&mut *column);
            self.give(root, at, component, column);
            given.push(component);
        }
        let parent_type = self.component::<Parent>();
        let parent_type = parent_type.expect("every world has registered `Parent`");
        let mut links = Vec::with_capacity(handles.len());
        for (part, of) in template.parts.iter().enumerate().skip(1) {
            let up = handles[of.parent.expect("a part but the root has a parent")];
            let mut row: Vec<(ComponentId, Box<dyn AnyColumn>)> = (of.row.iter())
                .map(|(component, column)| {
                    let mut column = column.cloned();
                    retarget(&mut *column);
                    (*component, column)
                })
                .collect();
            let column: Box<dyn AnyColumn> = Box::new(vec![Parent(up)]);
            row.push((parent_type, column));
            row.sort_unstable_by_key(|&(component, _)| component);
            self.place(handles[part], instance.ids[part], row);
            links.push((instance.ids[part], handles[part], up));
        }
        links.sort_unstable_by_key(|&(id, ..)| id);
        let children = links.iter().filter(|&&(.., up)| up == root);
        let children = children.map(|&(id, ..)| id).collect();
        for &(_, child, up) in &links {
            self.append(up.index, child.index);
        }
        let model = self.component::<ModelRoot>();
        if given.iter().any(|&component| Some(component) == model) {
            self.fit_nodes(root, None);
        }
        for &entity in &handles[1..] {
            if self.get::<ModelRoot>(entity).is_some() {
                self.fit_nodes(entity, None);
            }
        }
        (given, children)
    }

    /// What each instance takes when `prefab`'s file holds `text` in place of what it held, for
    /// [`refit_prefab`](Self::refit_prefab): each outermost instance as the prefab gave it and
    /// gives it now. A context checks every world takes the text before any does.
    ///
    /// Fails like [`Expanding::template`] for the prefab as it was or as `text` holds it:
    /// `scene.prefab_cycle` for a text that links the prefab itself.
    #[cfg(feature = "dev")]
    pub(crate) fn refits(&self, prefab: Prefab, text: &Arc<str>) -> Result<Refits> {
        let _zone = gg_core::zone!();
        let mut roots: Vec<Entity> = self
            .query::<(Entity, &PrefabRoot)>()
            .map(|(e, _)| e)
            .collect();
        roots.sort_unstable_by_key(|&root| (self.depth(root), self.id(root)));
        let mut was = Expanding::default();
        let mut now = Expanding::with_text(prefab, Arc::clone(text));
        let mut claimed = HashSet::new();
        let mut instances = Vec::new();
        for root in roots {
            let id = self.id(root).expect("alive");
            if claimed.contains(&id) {
                continue;
            }
            let old = self.instance(root, &mut was)?.expect("a root");
            let new = self.instance(root, &mut now)?.expect("a root");
            claimed.extend(old.ids[1..].iter().copied());
            instances.push((old, new));
        }
        Ok(Refits {
            world: self.serial,
            instances,
        })
    }

    /// Has each instance take what its prefab's file holds now, per `refits` from
    /// [`refits`](Self::refits) in this world:
    ///
    /// - a field still as the prefab gave it takes the new value; a differing one is kept;
    /// - a newly given component is added; a no-longer-given one is removed where it's as given;
    /// - a newly given entity is spawned under its parent; a no-longer-given one is despawned
    ///   with what's under it, under their IDs;
    /// - an entity still under its old prefab parent moves under the new one before the old
    ///   parent goes, so it doesn't go with it;
    /// - models' nodes follow their roots' models.
    ///
    /// Instances inside others' prefabs take it through the outermost, and so do those of other
    /// prefabs linking it.
    ///
    /// # Panics
    ///
    /// If `refits` is another world's.
    #[cfg(feature = "dev")]
    pub(crate) fn refit_prefab(&mut self, refits: Refits) {
        let _zone = gg_core::zone!();
        assert_eq!(
            refits.world, self.serial,
            "a world takes the refits found in it"
        );
        for (old, new) in refits.instances {
            self.refit(&old, &new);
        }
    }

    /// The outermost instances in live `root`'s subtree (per [`claims`](Self::claims)), by root
    /// ID with template. An operation that takes them out keeps these, so
    /// [`refit_kept`](Self::refit_kept) can apply the prefab's file changes once an undo or redo
    /// puts them back. An instance whose template doesn't load gives none.
    #[cfg(feature = "dev")]
    pub(super) fn kept_instances(&self, root: Entity) -> Vec<(EntityId, Rc<Template>)> {
        let claims = self.claims(&[root], &mut Expanding::default());
        let instances = claims.unwrap_or_default().instances.into_iter();
        let kept =
            instances.map(|(root, instance)| (self.id(root).expect("alive"), instance.template));
        kept.collect()
    }

    /// Has each instance in `kept` ([`kept_instances`](Self::kept_instances), put back by an undo
    /// or redo) take what its prefab gives now in place of then, like
    /// [`refit_prefab`](Self::refit_prefab). Skipped: an instance gone with one refitted before
    /// it, and one whose template doesn't load now, which a save then refuses.
    #[cfg(feature = "dev")]
    pub(super) fn refit_kept(&mut self, kept: &[(EntityId, Rc<Template>)]) {
        let mut now = Expanding::default();
        for (id, template) in kept {
            let Some(root) = self.entity(*id) else {
                continue;
            };
            let (Ok(ids), Ok(Some(new))) = (template.ids(*id), self.instance(root, &mut now))
            else {
                continue;
            };
            let template = Rc::clone(template);
            self.refit(&Instance { template, ids }, &new);
        }
    }

    #[cfg(feature = "dev")]
    fn depth(&self, entity: Entity) -> usize {
        let mut depth = 0;
        let mut at = entity;
        while let Some(up) = self.parent(at).expect("alive") {
            at = up;
            depth += 1;
        }
        depth
    }

    /// Refits one instance with a live root from `old` to `new`; see
    /// [`refit_prefab`](Self::refit_prefab).
    #[cfg(feature = "dev")]
    fn refit(&mut self, old: &Instance, new: &Instance) {
        let old_parts: HashMap<EntityId, usize> = (old.ids.iter().enumerate())
            .map(|(part, &id)| (id, part))
            .collect();
        let new_parts: HashSet<EntityId> = new.ids.iter().copied().collect();
        // Each part after its parent, so a parent is placed before it's asked of.
        let downward = new.template.downward();
        // Parts the prefab gives now that no living entity has, under a parent that's alive or
        // comes with them.
        let mut fresh = vec![false; new.ids.len()];
        for &part in &downward[1..] {
            let up = new.template.parts[part]
                .parent
                .expect("a part but the root has a parent");
            let placed = fresh[up] || self.entity(new.ids[up]).is_some();
            let id = new.ids[part];
            fresh[part] = !old_parts.contains_key(&id) && self.entity(id).is_none() && placed;
        }
        if fresh.contains(&true) {
            self.spawn_fresh(new, &fresh);
        }
        // Moved, under parents alive or just come, before those the prefab gives no more go, so
        // none goes with them.
        for &part in &downward[1..] {
            let id = new.ids[part];
            let (Some(&was), Some(entity)) = (old_parts.get(&id), self.entity(id)) else {
                continue;
            };
            let (from, to) = (old.parent(self, was), new.parent(self, part));
            if from != to
                && let Some(to) = to
                && self.parent(entity).expect("alive") == from
            {
                // A parent code moved it under keeps it where it is.
                let _ = self.set_parent(entity, Some(to));
            }
        }
        for id in &old.ids[1..] {
            if !new_parts.contains(id)
                && let Some(entity) = self.entity(*id)
            {
                self.despawn(entity).expect("alive");
            }
        }
        let parent_type = self.component::<Parent>().expect("registered");
        for (part, &id) in new.ids.iter().enumerate() {
            let (Some(&was), Some(entity), false) =
                (old_parts.get(&id), self.entity(id), fresh[part])
            else {
                continue;
            };
            let mut types = old.components(self, was);
            types.extend(new.components(self, part));
            types.retain(|&component| component != parent_type);
            types.sort_unstable();
            types.dedup();
            let model = self.get::<ModelRoot>(entity).copied();
            for component in types {
                self.refit_component(entity, component, old.value(self, was, component), {
                    new.value(self, part, component)
                });
            }
            if self.get::<ModelRoot>(entity).copied() != model {
                self.fit_nodes(entity, None);
            }
        }
    }

    /// Refits `entity`'s `component` from `old` to `new`, field by field for a struct of named
    /// fields; see [`refit_prefab`](Self::refit_prefab).
    #[cfg(feature = "dev")]
    fn refit_component(
        &mut self,
        entity: Entity,
        component: ComponentId,
        old: Option<Value>,
        new: Option<Value>,
    ) {
        let name = self.components.name(component).to_owned();
        let now = self.get_field(entity, &name, "").ok();
        let same = |a: &Value, b: &Value| super::diff::same(a, self, b, self);
        match (old, new, now) {
            (Some(old), Some(new), Some(now)) => {
                let changed: Vec<(String, Value)> = match (&old, &new, &now) {
                    (Value::Struct(old), Value::Struct(new), Value::Struct(now))
                        if old.len() == new.len() && old.len() == now.len() =>
                    {
                        let fields = old.iter().zip(new).zip(now);
                        let fields = fields.filter(|(((_, old), (_, new)), (_, now))| {
                            same(now, old) && !same(old, new)
                        });
                        let fields = fields.map(|(((name, _), (_, new)), _)| (name, new));
                        fields
                            .map(|(name, new)| (name.to_string(), new.clone()))
                            .collect()
                    }
                    _ if same(&now, &old) && !same(&old, &new) => vec![(String::new(), new)],
                    _ => Vec::new(),
                };
                for (path, value) in changed {
                    // A nested instance's link too; the outer instance's refit spawns and
                    // despawns its entities itself.
                    let set = self.write_field(entity, &name, &path, &value, true);
                    set.expect("a prefab's value, which loaded, sets");
                }
            }
            (None, Some(new), None) => {
                let column = self.build(component, &name, &new);
                let column = column.expect("a prefab's value, which loaded, builds");
                let at = self.location(entity).expect("alive");
                self.give(entity, at, component, column);
            }
            (Some(old), None, Some(now)) if same(&now, &old) => {
                let at = self.location(entity).expect("alive");
                self.take_out(entity, at, component);
            }
            _ => {}
        }
    }

    /// Spawns `new`'s parts marked in `fresh` under their parents, like
    /// [`spawn_instance`](Self::spawn_instance).
    #[cfg(feature = "dev")]
    fn spawn_fresh(&mut self, new: &Instance, fresh: &[bool]) {
        let mut handles: Vec<Option<Entity>> = new.ids.iter().map(|&id| self.entity(id)).collect();
        for (part, &id) in new.ids.iter().enumerate() {
            if fresh[part] {
                let entity = self.take_slot();
                self.find_by_id(id, entity);
                handles[part] = Some(entity);
            }
        }
        let template = &new.template;
        let by_handle: HashMap<Entity, Entity> = (template.parts.iter())
            .zip(&handles)
            .filter_map(|(part, handle)| Some((part.handle, (*handle)?)))
            .collect();
        let parent_type = self.component::<Parent>().expect("registered");
        let mut links = Vec::new();
        for (part, of) in template.parts.iter().enumerate() {
            if !fresh[part] {
                continue;
            }
            let up = handles[of.parent.expect("a part but the root")].expect("alive or fresh");
            let mut row: Vec<(ComponentId, Box<dyn AnyColumn>)> = (of.row.iter())
                .map(|(component, column)| {
                    let mut column = column.cloned();
                    column.entities_mut(0, &mut |entity| {
                        *entity = by_handle.get(entity).copied().unwrap_or(Entity::DEAD);
                    });
                    (*component, column)
                })
                .collect();
            let column: Box<dyn AnyColumn> = Box::new(vec![Parent(up)]);
            row.push((parent_type, column));
            row.sort_unstable_by_key(|&(component, _)| component);
            let entity = handles[part].expect("fresh");
            self.place(entity, new.ids[part], row);
            links.push((new.ids[part], entity, up));
        }
        links.sort_unstable_by_key(|&(id, ..)| id);
        for &(_, child, up) in &links {
            self.append(up.index, child.index);
        }
        for &(_, entity, _) in &links {
            if self.get::<ModelRoot>(entity).is_some() {
                self.fit_nodes(entity, None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use gg_core::math::{DVec3, Vec3, Vec4};

    use super::*;
    use crate::ecs::id::linked;
    use crate::ecs::id::tests::check_ids;
    use crate::ecs::{Log, Operation};
    use crate::reflect::Reflect;
    use crate::{Context, Name};

    #[derive(Clone, Debug, PartialEq, crate::Component)]
    struct Lamp {
        color: Vec4,
        bright: f32,
        aim: Option<Entity>,
    }

    /// A lamp aiming at its bulb, its root turned by a transform instances don't take.
    const LAMP: &str = r#"(
    entities: {
        "00000000000000a1": (
            Lamp: (
                color: (1.0, 0.5, 0.0, 1.0),
                bright: 2.0,
                aim: Some("00000000000000b2"),
            ),
            Name: ("lamp"),
            Transform: (
                translation: (5.0, 0.0, 0.0),
                rotation: (0.0, 0.0, 0.0, 1.0),
                scale: (1.0, 1.0, 1.0),
            ),
        ),
        "00000000000000b2": (
            Name: ("bulb"),
            Parent: ("00000000000000a1"),
            Transform: (
                translation: (0.0, 1.0, 0.0),
                rotation: (0.0, 0.0, 0.0, 1.0),
                scale: (1.0, 1.0, 1.0),
            ),
        ),
    },
)
"#;

    fn id(n: u64) -> EntityId {
        EntityId::new(n).expect("not zero")
    }

    /// A post: a root, and under it an instance of [`LAMP`], green, its bulb moved up.
    fn post() -> String {
        let bulb = linked(id(0xd4), id(0xb2)).expect("an ID");
        format!(
            r#"(
    entities: {{
        "00000000000000c3": (
            Name: ("post"),
        ),
        "00000000000000d4": (
            Lamp: (
                color: (0.0, 1.0, 0.0, 1.0),
            ),
            Parent: ("00000000000000c3"),
            PrefabRoot: ("lamp.prefab"),
            Transform: (
                translation: (0.0, 2.0, 0.0),
                rotation: (0.0, 0.0, 0.0, 1.0),
                scale: (1.0, 1.0, 1.0),
            ),
        ),
        "{bulb}": (
            Transform: (
                translation: (0.0, 9.0, 0.0),
            ),
        ),
    }},
)
"#
        )
    }

    /// A context with `Lamp` registered and a prefab per (path, text) of `texts`.
    fn with<const N: usize>(texts: [(&str, &str); N]) -> (Context, [Prefab; N]) {
        let mut cx = Context::seeded(1);
        let world = cx.world_mut();
        world.register::<Lamp>().expect("describes itself");
        let assets = world.assets_mut();
        let prefabs = texts
            .map(|(path, text)| assets.add_prefab(path.into(), path.into(), Some(text.into())));
        (cx, prefabs)
    }

    fn of(world: &World, root: Entity, local: u64) -> Entity {
        let root = world.id(root).expect("alive");
        let mixed = linked(root, id(local)).expect("an ID");
        world.entity(mixed).expect("spawned")
    }

    /// An instance's root takes its prefab root's components but its own transform; the prefab's
    /// other entities spawn under it with IDs mixed from its. A scene saves each instance as its
    /// root, link, transform and other differences, field by field, plus any given entity that
    /// differs. It loads to the same world and saves to the same bytes.
    #[test]
    fn a_scene_saves_what_an_instance_has_otherwise_than_its_prefab() {
        let (mut cx, [lamp]) = with([("lamp.prefab", LAMP)]);
        let world = cx.world_mut();
        let at = Transform {
            translation: DVec3::new(-2.0, 0.0, 0.0),
            ..Transform::IDENTITY
        };
        let first = lamp.spawn(world, at).expect("spawned");
        let second = lamp.spawn(world, Transform::IDENTITY).expect("spawned");
        check_ids(world);
        assert_eq!(world.get::<Transform>(first), Some(&at), "its own");
        assert_eq!(world.get::<Name>(first), Some(&Name("lamp".to_owned())));
        assert_eq!(world.get::<PrefabRoot>(first), Some(&PrefabRoot(lamp)));
        let bulb = of(world, first, 0xb2);
        assert_eq!(world.parent(bulb).expect("alive"), Some(first));
        assert_eq!(world.get::<Name>(bulb), Some(&Name("bulb".to_owned())));
        let aim = world.get::<Lamp>(first).expect("the prefab's").aim;
        assert_eq!(aim, Some(bulb), "its own bulb");

        let blue = Value::Vec4(Vec4::new(0.0, 0.0, 1.0, 1.0));
        world
            .set_field(second, "Lamp", "color", &blue)
            .expect("set");
        let other = of(world, second, 0xb2);
        let up = Value::DVec3(DVec3::new(0.0, 3.0, 0.0));
        world
            .set_field(other, "Transform", "translation", &up)
            .expect("set");
        let held = Lamp {
            color: Vec4::ONE,
            bright: 1.0,
            aim: None,
        };
        world.insert(other, held).expect("code may");
        let text = world.save_scene(&[first, second]).expect("saved");
        let [first_id, second_id, other_id] = [first, second, other].map(|e| world.id(e).unwrap());
        // An instance's root at `at`, with `lamp`, what else it changes.
        let root = |id: EntityId, lamp: &str, at: &str| {
            format!(
                r#"        "{id}": (
{lamp}            PrefabRoot: ("lamp.prefab"),
            Transform: (
                translation: {at},
                rotation: (0.0, 0.0, 0.0, 1.0),
                scale: (1.0, 1.0, 1.0),
            ),
        ),
"#
            )
        };
        let blue = r#"            Lamp: (
                color: (0.0, 0.0, 1.0, 1.0),
            ),
"#;
        let held = format!(
            r#"        "{other_id}": (
            Lamp: (
                color: (1.0, 1.0, 1.0, 1.0),
                bright: 1.0,
                aim: None,
            ),
            Transform: (
                translation: (0.0, 3.0, 0.0),
            ),
        ),
"#
        );
        let mut blocks = [
            (first_id, root(first_id, "", "(-2.0, 0.0, 0.0)")),
            (second_id, root(second_id, blue, "(0.0, 0.0, 0.0)")),
            (other_id, held),
        ];
        blocks.sort_by_key(|&(id, _)| id);
        let blocks: String = blocks.into_iter().map(|(_, block)| block).collect();
        assert_eq!(text, format!("(\n    entities: {{\n{blocks}    }},\n)\n"));

        let mut loaded = world.emptied();
        loaded.load_scene(&text).expect("loaded");
        check_ids(&loaded);
        assert_eq!(
            loaded.hash().content,
            world.hash().content,
            "the same world"
        );
        let roots = [first, second].map(|root| loaded.entity(world.id(root).unwrap()).unwrap());
        assert_eq!(
            loaded.save_scene(&roots).expect("saved"),
            text,
            "the same bytes"
        );
    }

    /// A field an instance's entity leaves out takes its prefab's, not its type's default. Saving
    /// is refused after code took away a prefab's entity or a component of one.
    #[test]
    fn what_an_instance_leaves_out_is_its_prefabs() {
        let (mut cx, [lamp]) = with([("lamp.prefab", LAMP)]);
        let text = r#"(
    entities: {
        "0000000000000100": (
            Lamp: (
                bright: 3.0,
            ),
            PrefabRoot: ("lamp.prefab"),
        ),
    },
)
"#;
        let world = cx.world_mut();
        let roots = world.load_scene(text).expect("loaded");
        let bulb = of(world, roots[0], 0xb2);
        let held = world.get::<Lamp>(roots[0]).expect("the prefab's").clone();
        let expected = Lamp {
            color: Vec4::new(1.0, 0.5, 0.0, 1.0),
            bright: 3.0,
            aim: Some(bulb),
        };
        assert_eq!(held, expected);
        assert_eq!(
            world.get::<Transform>(roots[0]),
            None,
            "the prefab's isn't its"
        );
        assert_eq!(world.get::<PrefabRoot>(roots[0]), Some(&PrefabRoot(lamp)));
        assert_eq!(world.save_scene(&roots).expect("saved"), text);
        // Nor does a root's own transform take what it leaves out from the prefab root's.
        let placed = text.replace("0000000000000100", "0000000000000200").replace(
            "            PrefabRoot: (\"lamp.prefab\"),\n",
            "            PrefabRoot: (\"lamp.prefab\"),\n            Transform: (\n                \
             scale: (2.0, 2.0, 2.0),\n            ),\n",
        );
        let other = world.load_scene(&placed).expect("loaded")[0];
        let at = world.get::<Transform>(other).expect("its own");
        assert_eq!(at.translation, DVec3::ZERO, "its default, not the prefab's");
        assert_eq!(at.scale, Vec3::splat(2.0));

        world.remove::<Name>(bulb).expect("code may");
        let error = world.save_scene(&roots).expect_err("its name gone");
        assert_eq!(error.code(), "scene.prefab_missing", "{error:#}");
        world.despawn(bulb).expect("code may");
        let error = world.save_scene(&roots).expect_err("its bulb gone");
        assert_eq!(error.code(), "scene.prefab_missing", "{error:#}");
    }

    /// An instance inside a prefab spawns under the outer instance, its IDs mixed again from its
    /// own root's, with what the outer prefab gives it. A scene saves the outer root alone, plus
    /// any differing inner entity under its mixed ID.
    #[test]
    fn a_prefabs_instances_nest() {
        let post = post();
        let (mut cx, [lamp, post]) = with([("lamp.prefab", LAMP), ("post.prefab", &post)]);
        let world = cx.world_mut();
        let root = post.spawn(world, Transform::IDENTITY).expect("spawned");
        let inner = of(world, root, 0xd4);
        assert_eq!(world.parent(inner).expect("alive"), Some(root));
        assert_eq!(world.get::<PrefabRoot>(inner), Some(&PrefabRoot(lamp)));
        let bulb = of(world, inner, 0xb2);
        let lit = world.get::<Lamp>(inner).expect("the lamp's").clone();
        let expected = Lamp {
            color: Vec4::new(0.0, 1.0, 0.0, 1.0),
            bright: 2.0,
            aim: Some(bulb),
        };
        assert_eq!(lit, expected, "the post's color, the lamp's brightness");
        let place = world
            .get::<Transform>(bulb)
            .expect("the lamp's")
            .translation;
        assert_eq!(place, DVec3::new(0.0, 9.0, 0.0), "where the post moved it");
        let root_id = world.id(root).expect("alive");
        let text = world.save_scene(&[root]).expect("saved");
        let expected = format!(
            "(\n    entities: {{\n        \"{root_id}\": (\n            PrefabRoot: \
             (\"post.prefab\"),\n            Transform: (\n                translation: (0.0, 0.0, \
             0.0),\n                rotation: (0.0, 0.0, 0.0, 1.0),\n                scale: (1.0, \
             1.0, 1.0),\n            ),\n        ),\n    }},\n)\n"
        );
        assert_eq!(text, expected);

        world
            .insert(bulb, Name("globe".to_owned()))
            .expect("code may");
        let text = world.save_scene(&[root]).expect("saved");
        let bulb_id = world.id(bulb).expect("alive");
        assert!(text.contains(&format!(
            "\"{bulb_id}\": (\n            Name: (\"globe\"),\n"
        )));
        let mut loaded = world.emptied();
        loaded.load_scene(&text).expect("loaded");
        assert_eq!(
            loaded.hash().content,
            world.hash().content,
            "the same world"
        );
    }

    /// Overrides are found as a scene save finds them, by the outermost instance giving the
    /// entity: named-field structs field by field, other types whole, plus added components. A
    /// root's transform is its own; a child of its own under an instance's entity, or an entity
    /// of no instance, has none.
    #[cfg(feature = "dev")]
    #[test]
    fn overrides_say_what_an_instances_entities_change() {
        let post = post();
        let (mut cx, [lamp, post]) = with([("lamp.prefab", LAMP), ("post.prefab", &post)]);
        let world = cx.world_mut();
        let root = post.spawn(world, Transform::IDENTITY).expect("spawned");
        let inner = of(world, root, 0xd4);
        let bulb = of(world, inner, 0xb2);
        let none = Overrides {
            root: world.id(root).expect("alive"),
            prefab: post,
            fields: Vec::new(),
            added: Vec::new(),
        };
        let at = Value::DVec3(DVec3::new(4.0, 0.0, 0.0));
        world
            .set_field(root, "Transform", "translation", &at)
            .expect("set");
        for entity in [root, inner, bulb] {
            let found = world.overrides(entity).expect("it loads");
            assert_eq!(
                found,
                Some(none.clone()),
                "the post's changes are its prefab's"
            );
        }
        let bright = Value::F32(5.0);
        world
            .set_field(inner, "Lamp", "bright", &bright)
            .expect("set");
        world
            .insert(bulb, Name("globe".to_owned()))
            .expect("code may");
        let held = Lamp {
            color: Vec4::ONE,
            bright: 1.0,
            aim: None,
        };
        world.insert(bulb, held).expect("code may");
        let changed = |fields: &[(&str, &str)], added: &[&str]| Overrides {
            fields: (fields.iter())
                .map(|&(component, field)| (component.to_owned(), vec![field.to_owned()]))
                .collect(),
            added: added.iter().map(|&added| added.to_owned()).collect(),
            ..none.clone()
        };
        let found = world.overrides(inner).expect("it loads");
        assert_eq!(found, Some(changed(&[("Lamp", "bright")], &[])));
        let found = world.overrides(bulb).expect("it loads");
        assert_eq!(
            found,
            Some(changed(&[("Name", "")], &["Lamp"])),
            "a tuple's whole"
        );

        let alone = lamp.spawn(world, Transform::IDENTITY).expect("spawned");
        let blue = Value::Vec4(Vec4::new(0.0, 0.0, 1.0, 1.0));
        world.set_field(alone, "Lamp", "color", &blue).expect("set");
        let expected = Overrides {
            root: world.id(alone).expect("alive"),
            prefab: lamp,
            fields: vec![("Lamp".to_owned(), vec!["color".to_owned()])],
            added: Vec::new(),
        };
        assert_eq!(world.overrides(alone).expect("it loads"), Some(expected));
        let child = world.spawn(Transform::IDENTITY);
        world.set_parent(child, Some(bulb)).expect("a child");
        assert_eq!(world.overrides(child).expect("no instance's"), None);
        let plain = world.spawn(Transform::IDENTITY);
        assert_eq!(world.overrides(plain).expect("no instance's"), None);
    }

    /// A link gives a root whose subtree holds what its prefab gives, under the IDs its instance
    /// mixes, its `PrefabRoot` back, spawning nothing, as an unlink's undo does: the world as it
    /// was, each entity under its handle. Refused for a root already linked, an entity an
    /// instance gives, and a subtree that differs from what its prefab gives.
    #[test]
    fn an_unlinked_instance_links_again_as_it_was() {
        let (mut cx, [lamp]) = with([("lamp.prefab", LAMP)]);
        let world = cx.world_mut();
        let root = lamp.spawn(world, Transform::IDENTITY).expect("spawned");
        let root_id = world.id(root).expect("alive");
        let bulb = of(world, root, 0xb2);
        let bulb_id = world.id(bulb).expect("alive");
        let content = world.hash().content;
        let mut log = Log::default();
        let link = Operation::Link {
            entity: root_id,
            prefab: lamp,
        };
        let error = log.apply(world, link.clone()).expect_err("linked already");
        assert_eq!(error.code(), "reflect.has_component", "{error:#}");
        let given = Operation::Link {
            entity: bulb_id,
            prefab: lamp,
        };
        let error = log.apply(world, given).expect_err("the prefab's");
        assert_eq!(error.code(), "ecs.prefab_entity", "{error:#}");
        log.apply(world, Operation::Unlink { entity: root_id })
            .expect("unlinked");
        log.apply(world, link.clone()).expect("linked again");
        assert_eq!(world.hash().content, content, "as it was");
        assert_eq!(world.entity(bulb_id), Some(bulb), "under its handle");
        check_ids(world);
        log.undo(world).expect("unlinked again");
        assert_eq!(world.get::<PrefabRoot>(root), None);
        log.redo(world).expect("linked again");
        assert_eq!(world.hash().content, content);

        log.apply(world, Operation::Unlink { entity: root_id })
            .expect("unlinked");
        let mut refused = |world: &mut World| {
            let error = log.apply(world, link.clone()).expect_err("refused");
            error.code().to_owned()
        };
        world.remove::<Name>(bulb).expect("code may");
        assert_eq!(refused(world), "ecs.prefab_missing", "a component gone");
        world
            .insert(bulb, Name("bulb".to_owned()))
            .expect("code may");
        world.set_parent(bulb, None).expect("a root");
        assert_eq!(refused(world), "ecs.prefab_parent");
        world.despawn(bulb).expect("code may");
        assert_eq!(refused(world), "ecs.prefab_missing", "an entity gone");
        assert_eq!(world.get::<PrefabRoot>(root), None, "nothing changed");
    }

    /// A link redone or an unlink undone is checked like a link, as another author may have
    /// changed the subtree: refused the same way, changing nothing, so a scene still saves. A
    /// link is also refused for a given entity that links a prefab its prefab doesn't link it to,
    /// as a scene load would refuse.
    #[test]
    fn a_link_undone_or_redone_is_checked_as_a_link_is() {
        // A stand and its bulb, which nothing refers to (a lamp's aim does), so a scene saves
        // without it.
        let stand = "(entities: {\"00000000000000a1\": (Name: (\"stand\")), \"00000000000000b2\": \
                     (Name: (\"bulb\"), Parent: (\"00000000000000a1\"))})\n";
        let dot = "(entities: {\"00000000000000e1\": (Name: (\"dot\"))})\n";
        let (mut cx, [stand, dot]) = with([("stand.prefab", stand), ("dot.prefab", dot)]);
        let world = cx.world_mut();
        let root = stand.spawn(world, Transform::IDENTITY).expect("spawned");
        let root_id = world.id(root).expect("alive");
        let bulb_id = world.id(of(world, root, 0xb2)).expect("alive");
        let (mut mine, mut theirs) = (Log::default(), Log::default());
        let link = Operation::Link {
            entity: root_id,
            prefab: stand,
        };
        let unlink = Operation::Unlink { entity: root_id };
        let delete = Operation::Delete { entity: bulb_id };
        mine.apply(world, unlink.clone()).expect("unlinked");
        mine.apply(world, link.clone()).expect("linked");
        mine.undo(world).expect("unlinked again");
        theirs
            .apply(world, delete.clone())
            .expect("an entity of its own");
        let error = mine.redo(world).expect_err("its bulb gone");
        assert_eq!(error.code(), "ecs.prefab_missing", "{error:#}");
        assert_eq!(world.get::<PrefabRoot>(root), None, "nothing changed");
        world.save_scene(&[root]).expect("it saves");

        theirs.undo(world).expect("the bulb back");
        mine.apply(world, link.clone()).expect("linked again");
        mine.apply(world, unlink).expect("unlinked");
        theirs.apply(world, delete).expect("an entity of its own");
        let error = mine.undo(world).expect_err("its bulb gone");
        assert_eq!(error.code(), "ecs.prefab_missing", "{error:#}");
        assert_eq!(world.get::<PrefabRoot>(root), None, "nothing changed");
        world.save_scene(&[root]).expect("it saves");

        theirs.undo(world).expect("the bulb back");
        let add = Operation::Add {
            entity: bulb_id,
            component: "PrefabRoot".into(),
            value: PrefabRoot(dot).to_value(),
        };
        theirs.apply(world, add).expect("an entity of its own");
        let error = mine.apply(world, link).expect_err("its bulb linked");
        assert_eq!(error.code(), "ecs.prefab_entity", "{error:#}");
        assert_eq!(world.get::<PrefabRoot>(root), None, "nothing changed");
    }

    /// A subtree saved as a prefab gives each entity the local ID that the instance rooted at the
    /// subtree mixes to its ID, and a nested instance's entities IDs mixed from its root's, the
    /// root at its origin. Linked, it's an instance holding every entity under its ID, model
    /// nodes too, with no differences, which a scene saves as the link and transform alone;
    /// undone, it's as it was. Refused: an instance's entity or root, a model's node, and a
    /// subtree that refers outside itself.
    #[cfg(feature = "dev")]
    #[test]
    fn a_subtree_saved_as_a_prefab_links_under_its_ids() {
        use crate::ecs::id::unlinked;

        let (mut cx, [_lamp]) = with([("lamp.prefab", LAMP)]);
        crate::model::tests::added(&mut cx, "m", crate::model::tests::imported());
        let world = cx.world_mut();
        let globe = linked(id(0x40), id(0xb2)).expect("an ID");
        let transform = |x: f32| {
            format!(
                "Transform: (translation: ({x:?}, 0.0, 0.0), rotation: (0.0, 0.0, 0.0, 1.0), \
                 scale: (1.0, 1.0, 1.0))"
            )
        };
        let text = format!(
            "(entities: {{\"0000000000000010\": (Name: (\"stand\"), {}), \
             \"0000000000000020\": (Lamp: (color: (1.0, 1.0, 1.0, 1.0), bright: 1.0, aim: \
             Some(\"0000000000000030\")), Parent: (\"0000000000000010\")), \
             \"0000000000000030\": (Name: (\"b\"), Parent: (\"0000000000000020\")), \
             \"0000000000000040\": (Parent: (\"0000000000000010\"), PrefabRoot: (\"lamp.prefab\"), \
             {}), \"{globe}\": (Name: (\"globe\")), \
             \"0000000000000050\": (ModelRoot: (\"m\"), Parent: (\"0000000000000010\"))}})\n",
            transform(3.0),
            transform(2.0)
        );
        let roots = world.load_scene(&text).expect("loaded");
        let root = roots[0];
        let root_id = id(0x10);
        let node = world.children(world.entity(id(0x50)).expect("alive"));
        let node = node.expect("alive").next().expect("a node");
        let node_id = world.id(node).expect("alive");
        let before = world.save_scene(&roots).expect("saved");
        let content = world.hash().content;
        let saved = world.save_prefab(root).expect("saved as a prefab");
        let own = [0x10, 0x20, 0x30, 0x40, 0x50].map(|n| unlinked(root_id, id(n)).expect("an ID"));
        let mut locals = own.to_vec();
        locals.push(linked(own[3], id(0xb2)).expect("an ID"));
        locals.sort();
        assert_eq!(World::scene_ids(&saved).expect("its IDs"), locals);
        assert!(saved.contains("translation: (0.0, 0.0, 0.0)"), "{saved}");
        assert!(!saved.contains("ModelNode"), "{saved}");
        let stand = world.assets_mut().add_prefab(
            "stand.prefab".into(),
            "stand.prefab".into(),
            Some(saved.into()),
        );

        let mut log = Log::default();
        let link = Operation::Link {
            entity: root_id,
            prefab: stand,
        };
        log.apply(world, link).expect("linked");
        let ids = [0x10, 0x20, 0x30, 0x40, 0x50].map(id);
        for held in ids.into_iter().chain([globe, node_id]) {
            assert!(world.entity(held).is_some(), "{held} kept");
        }
        check_ids(world);
        for held in ids.into_iter().chain([globe]) {
            let found = world.overrides(world.entity(held).expect("alive"));
            let found = found.expect("it loads").expect("an instance's");
            assert_eq!((found.root, found.prefab), (root_id, stand), "{held}");
            assert!(
                found.fields.is_empty() && found.added.is_empty(),
                "{held}: {found:?}"
            );
        }
        let instance = world.save_scene(&roots).expect("saved");
        let expected = "(\n    entities: {\n        \"0000000000000010\": (\n            \
                        PrefabRoot: (\"stand.prefab\"),\n            Transform: (\n                \
                        translation: (3.0, 0.0, 0.0),\n                rotation: (0.0, 0.0, 0.0, \
                        1.0),\n                scale: (1.0, 1.0, 1.0),\n            ),\n        \
                        ),\n    },\n)\n";
        assert_eq!(instance, expected);
        let mut loaded = world.emptied();
        loaded.load_scene(&instance).expect("loaded");
        assert_eq!(
            loaded.hash().content,
            world.hash().content,
            "the same world"
        );
        log.undo(world).expect("unlinked");
        assert_eq!(world.save_scene(&roots).expect("saved"), before);
        assert_eq!(world.hash().content, content, "as it was");
        log.redo(world).expect("linked again");
        assert_eq!(world.save_scene(&roots).expect("saved"), instance);

        let refused = |world: &World, held: Entity| {
            let error = world.save_prefab(held).expect_err("refused");
            error.code().to_owned()
        };
        assert_eq!(refused(world, root), "scene.prefab_variant");
        let given = world.entity(id(0x20)).expect("alive");
        assert_eq!(refused(world, given), "ecs.prefab_entity");
        let node = world.entity(node_id).expect("alive");
        assert_eq!(refused(world, node), "ecs.model_node");
        let outside = world.spawn(Transform::IDENTITY);
        let aimed = world.spawn(Lamp {
            color: Vec4::ONE,
            bright: 1.0,
            aim: Some(outside),
        });
        assert_eq!(refused(world, aimed), "scene.outside");
    }

    /// A model root in a prefab spawns its nodes in each instance, IDs mixed from the instance
    /// entity's; a scene saves the instance without them.
    #[test]
    fn a_model_in_a_prefab_spawns_its_nodes_in_each_instance() {
        let text = "(\n    entities: {\n        \"00000000000000e1\": (),\n        \
                    \"00000000000000f2\": (\n            ModelRoot: (\"m\"),\n            \
                    Parent: (\"00000000000000e1\"),\n        ),\n    },\n)\n";
        let (mut cx, [statue]) = with([("statue.prefab", text)]);
        let (model, _) = crate::model::tests::added(&mut cx, "m", crate::model::tests::imported());
        let world = cx.world_mut();
        let root = statue.spawn(world, Transform::IDENTITY).expect("spawned");
        let held = of(world, root, 0xf2);
        assert_eq!(world.get::<ModelRoot>(held), Some(&ModelRoot(model)));
        let held_id = world.id(held).expect("alive");
        let nodes: Vec<Entity> = world.children(held).expect("alive").collect();
        assert_eq!(nodes.len(), 2, "the model's two top nodes");
        let first = super::super::nodes::node_id(held_id, 0).expect("an ID");
        assert_eq!(world.id(nodes[0]), Some(first));
        let saved = world.save_scene(&[root]).expect("saved");
        assert!(!saved.contains("ModelNode"), "{saved}");
        let mut loaded = world.emptied();
        loaded.load_scene(&saved).expect("loaded");
        assert_eq!(
            loaded.hash().content,
            world.hash().content,
            "the same world"
        );
    }

    /// A prefab that links itself, through others or not, is refused, as is one whose root links
    /// another.
    #[test]
    fn a_prefab_that_links_itself_or_whose_root_links_one_is_refused() {
        // A prefab of a root and a child of it linking prefab `of`.
        let link = |root: u64, child: u64, of: &str| {
            format!(
                r#"(
    entities: {{
        "{root:016x}": (),
        "{child:016x}": (
            Parent: ("{root:016x}"),
            PrefabRoot: ("{of}"),
        ),
    }},
)
"#
            )
        };
        let [a, b, c] = [
            link(1, 2, "b.prefab"),
            link(3, 4, "a.prefab"),
            link(5, 6, "c.prefab"),
        ];
        let variant = r#"(
    entities: {
        "0000000000000007": (
            PrefabRoot: ("lamp.prefab"),
        ),
    },
)
"#;
        let (mut cx, [a, _, c, variant, _]) = with([
            ("a.prefab", &a),
            ("b.prefab", &b),
            ("c.prefab", &c),
            ("variant.prefab", variant),
            ("lamp.prefab", LAMP),
        ]);
        let world = cx.world_mut();
        let count = |world: &World| world.entities(None).map_or(0, |entities| entities.len());
        let before = count(world);
        for (prefab, code) in [
            (a, "scene.prefab_cycle"),
            (c, "scene.prefab_cycle"),
            (variant, "scene.prefab_variant"),
        ] {
            let error = prefab.spawn(world, Transform::IDENTITY).expect_err(code);
            assert_eq!(error.code(), code, "{error:#}");
        }
        let error = (world.load_scene(
            "(\n    entities: {\n        \"0000000000000100\": (\n            PrefabRoot: \
             (\"a.prefab\"),\n        ),\n    },\n)\n",
        ))
        .expect_err("a cycle");
        assert_eq!(error.code(), "scene.prefab_cycle", "{error:#}");
        assert!(
            error.to_string().contains("in prefab `a.prefab`"),
            "{error:#}"
        );
        assert_eq!(count(world), before, "nothing spawned");
    }

    /// Tools create an instance (one edit with its entities), change and revert its entities'
    /// fields, change its prefab or remove it (undo restores entities and fields), unlink it into
    /// entities of its own, and copy it with IDs mixed from the copy's. They can't remove its
    /// prefab's entities or components.
    #[test]
    fn tools_change_an_instance_and_take_none_of_its_prefab_away() {
        let post = post();
        let (mut cx, [lamp, post]) = with([("lamp.prefab", LAMP), ("post.prefab", &post)]);
        let world = cx.world_mut();
        let mut log = Log::default();
        let create = Operation::Create {
            entity: None,
            parent: None,
            components: vec![
                ("PrefabRoot".into(), PrefabRoot(lamp).to_value()),
                ("Transform".into(), Transform::IDENTITY.to_value()),
            ],
        };
        log.apply(world, create).expect("made");
        let Some(
            [
                Operation::Create {
                    entity: Some(root_id),
                    ..
                },
            ],
        ) = log.undoable()
        else {
            panic!("a create");
        };
        let root_id = *root_id;
        let root = world.entity(root_id).expect("made");
        let bulb = of(world, root, 0xb2);
        let bulb_id = world.id(bulb).expect("alive");
        assert_eq!(world.get::<Name>(root), Some(&Name("lamp".to_owned())));
        check_ids(world);

        let refused = [
            Operation::Delete { entity: bulb_id },
            Operation::Reparent {
                entity: bulb_id,
                parent: None,
            },
            Operation::Remove {
                entity: bulb_id,
                component: "Name".into(),
            },
            Operation::Remove {
                entity: root_id,
                component: "Lamp".into(),
            },
            Operation::Unlink { entity: bulb_id },
            Operation::Add {
                entity: bulb_id,
                component: "PrefabRoot".into(),
                value: PrefabRoot(lamp).to_value(),
            },
        ];
        for operation in refused {
            let said = operation.to_string();
            let error = log.apply(world, operation).expect_err(&said);
            assert_eq!(error.code(), "ecs.prefab_entity", "{said}: {error:#}");
        }
        let error = log.apply(
            world,
            Operation::Revert {
                entity: root_id,
                component: "Transform".into(),
                path: String::new(),
            },
        );
        assert_eq!(error.expect_err("its own").code(), "ecs.no_prefab");

        // A component of its own on the prefab's entity goes freely.
        let held = Operation::Add {
            entity: bulb_id,
            component: "Lamp".into(),
            value: Lamp {
                color: Vec4::ONE,
                bright: 1.0,
                aim: None,
            }
            .to_value(),
        };
        log.apply(world, held).expect("added");
        let remove = Operation::Remove {
            entity: bulb_id,
            component: "Lamp".into(),
        };
        log.apply(world, remove).expect("its own");
        log.undo(world).expect("given back");

        // A field set, and set back to the prefab's.
        let up = Value::DVec3(DVec3::new(0.0, 5.0, 0.0));
        let place = |world: &World| world.get::<Transform>(bulb).expect("alive").translation;
        let set = Operation::Set {
            entity: bulb_id,
            component: "Transform".into(),
            path: "translation".to_owned(),
            value: up,
        };
        log.apply(world, set).expect("set");
        let revert = Operation::Revert {
            entity: bulb_id,
            component: "Transform".into(),
            path: "translation".to_owned(),
        };
        log.apply(world, revert).expect("reverted");
        assert_eq!(place(world), DVec3::new(0.0, 1.0, 0.0), "the prefab's");
        log.undo(world).expect("undone");
        assert_eq!(place(world), DVec3::new(0.0, 5.0, 0.0));

        // Another prefab, and back, its entities and fields as they were.
        let content = world.hash().content;
        let relink = Operation::Set {
            entity: root_id,
            component: "PrefabRoot".into(),
            path: "0".to_owned(),
            value: Value::Prefab(post),
        };
        log.apply(world, relink).expect("relinked");
        assert_eq!(world.entity(bulb_id), None, "the lamp's bulb gone");
        assert_eq!(world.get::<Name>(root), Some(&Name("post".to_owned())));
        assert_eq!(world.get::<Lamp>(root), None, "the lamp's root's");
        let inner = of(world, root, 0xd4);
        assert_eq!(world.get::<PrefabRoot>(inner), Some(&PrefabRoot(lamp)));
        check_ids(world);
        log.undo(world).expect("undone");
        assert_eq!(world.hash().content, content, "as it was");
        assert_eq!(world.entity(bulb_id), Some(bulb), "under its handle");
        log.redo(world).expect("redone");
        assert_eq!(world.entity(bulb_id), None);
        log.undo(world).expect("undone again");

        // Taken away, its entities and its root's components with it.
        let remove = Operation::Remove {
            entity: root_id,
            component: "PrefabRoot".into(),
        };
        log.apply(world, remove).expect("taken away");
        assert_eq!(world.entity(bulb_id), None);
        assert_eq!(world.get::<Lamp>(root), None);
        assert_eq!(
            world.get::<Transform>(root),
            Some(&Transform::IDENTITY),
            "its own"
        );
        log.undo(world).expect("given back");
        assert_eq!(world.hash().content, content);

        // Unlinked, its entities are its own and go freely; the undo links again.
        log.apply(world, Operation::Unlink { entity: root_id })
            .expect("unlinked");
        assert_eq!(world.get::<PrefabRoot>(root), None);
        assert_eq!(world.entity(bulb_id), Some(bulb), "kept");
        log.apply(world, Operation::Delete { entity: bulb_id })
            .expect("its own");
        log.undo(world).expect("back");
        log.undo(world).expect("linked again");
        assert_eq!(world.hash().content, content);
        let error = log.apply(world, Operation::Delete { entity: bulb_id });
        assert_eq!(error.expect_err("the prefab's").code(), "ecs.prefab_entity");

        // A copy; its bulb's ID mixes from the copy's, its field kept.
        let duplicate = Operation::Duplicate {
            entity: root_id,
            copies: Vec::new(),
        };
        log.apply(world, duplicate).expect("copied");
        let Some([Operation::Duplicate { copies, .. }]) = log.undoable() else {
            panic!("a duplicate");
        };
        let copies = copies.clone();
        let copy = world.entity(copies[0]).expect("copied");
        assert_eq!(copies.len(), 1, "the root alone: {copies:?}");
        let copied = of(world, copy, 0xb2);
        let at = world.get::<Transform>(copied).expect("alive").translation;
        assert_eq!(at, DVec3::new(0.0, 5.0, 0.0));
        log.undo(world).expect("uncopied");
        assert_eq!(world.entity(copies[0]), None);
        check_ids(world);

        // Unmade whole, and made again under its IDs.
        while log.undo(world).expect("undone") {}
        assert_eq!(world.entity(root_id), None);
        assert_eq!(world.entity(bulb_id), None);
        log.redo(world).expect("made again");
        assert_eq!(
            world.entity(bulb_id).map(|bulb| world.id(bulb)),
            Some(Some(bulb_id))
        );
        check_ids(world);
    }

    /// A world with instances saves whole as a snapshot file, which loads to the same world.
    #[test]
    fn a_snapshot_file_keeps_instances() {
        let post = post();
        let (mut cx, [_, post]) = with([("lamp.prefab", LAMP), ("post.prefab", &post)]);
        let world = cx.world_mut();
        let root = post.spawn(world, Transform::IDENTITY).expect("spawned");
        let bulb = of(world, of(world, root, 0xd4), 0xb2);
        world
            .insert(bulb, Name("globe".to_owned()))
            .expect("code may");
        let mut text = String::new();
        world.save_whole(&mut text).expect("saved");
        let mut loaded = world.scratch();
        let lines = text.lines().enumerate().map(|(at, line)| (at + 1, line));
        loaded.load_whole(lines, true).expect("loaded");
        assert_eq!(loaded.hash(), world.hash());
        let saved = loaded.save_scene(&[loaded.entity(world.id(root).unwrap()).unwrap()]);
        assert_eq!(saved.ok(), world.save_scene(&[root]).ok());
    }

    /// As its prefab's file changes, an instance takes each field still as the prefab had it and
    /// keeps the rest; newly given entities spawn under their IDs, dropped ones go; a text that
    /// doesn't load changes nothing.
    #[cfg(feature = "dev")]
    #[test]
    fn instances_take_what_their_prefab_changes_and_keep_the_rest() {
        let (mut cx, [lamp]) = with([("lamp.prefab", LAMP)]);
        let world = cx.world_mut();
        let root = lamp.spawn(world, Transform::IDENTITY).expect("spawned");
        let blue = Value::Vec4(Vec4::new(0.0, 0.0, 1.0, 1.0));
        world.set_field(root, "Lamp", "color", &blue).expect("set");
        let bulb = of(world, root, 0xb2);
        let changed: Arc<str> = LAMP
            .replace("color: (1.0, 0.5, 0.0, 1.0)", "color: (0.2, 0.2, 0.2, 1.0)")
            .replace("bright: 2.0", "bright: 4.0")
            .replace("(\"bulb\")", "(\"globe\")")
            .replace(
                "    },\n)\n",
                "        \"00000000000000c5\": (\n            Name: (\"shade\"),\n            \
                 Parent: (\"00000000000000a1\"),\n        ),\n    },\n)\n",
            )
            .into();
        world.refit_prefab(world.refits(lamp, &changed).expect("loads"));
        world.assets_mut().set_prefab_text(lamp, changed);
        let held = world.get::<Lamp>(root).expect("kept").clone();
        let expected = Lamp {
            color: Vec4::new(0.0, 0.0, 1.0, 1.0),
            bright: 4.0,
            aim: Some(bulb),
        };
        assert_eq!(held, expected, "its own color, the prefab's brightness now");
        assert_eq!(world.get::<Name>(bulb), Some(&Name("globe".to_owned())));
        let shade = of(world, root, 0xc5);
        assert_eq!(world.parent(shade).expect("alive"), Some(root));
        check_ids(world);

        // Its bulb gone, and the lamp's aim with it, as the prefab's.
        let bulbless: Arc<str> = r#"(
    entities: {
        "00000000000000a1": (
            Lamp: (
                color: (0.2, 0.2, 0.2, 1.0),
                bright: 4.0,
                aim: None,
            ),
            Name: ("lamp"),
        ),
        "00000000000000c5": (
            Name: ("shade"),
            Parent: ("00000000000000a1"),
        ),
    },
)
"#
        .into();
        world.refit_prefab(world.refits(lamp, &bulbless).expect("loads"));
        world.assets_mut().set_prefab_text(lamp, bulbless);
        assert!(!world.contains(bulb), "gone with the prefab's");
        assert_eq!(world.get::<Lamp>(root).expect("kept").aim, None);
        check_ids(world);

        let hash = world.hash();
        let cycle: Arc<str> = r#"(
    entities: {
        "0000000000000001": (),
        "0000000000000002": (
            Parent: ("0000000000000001"),
            PrefabRoot: ("lamp.prefab"),
        ),
    },
)
"#
        .into();
        let error = world.refits(lamp, &cycle).err().expect("a cycle");
        assert_eq!(error.code(), "scene.prefab_cycle", "{error:#}");
        assert_eq!(world.hash(), hash, "nothing changed");
    }

    /// A scene edit changing an instance's entity sets its field; adding an instance makes the
    /// root with its link first, its prefab's entities with it, then the rest.
    #[cfg(feature = "dev")]
    #[test]
    fn a_scene_edit_links_an_instance_before_the_rest() {
        let (mut cx, [lamp]) = with([("lamp.prefab", LAMP)]);
        let world = cx.world_mut();
        let root = lamp.spawn(world, Transform::IDENTITY).expect("spawned");
        let old = world.save_scene(&[root]).expect("saved");
        let mut edited = world.clone();
        let bulb = of(&edited, root, 0xb2);
        let up = Value::DVec3(DVec3::new(0.0, 7.0, 0.0));
        edited
            .set_field(bulb, "Transform", "translation", &up)
            .expect("set");
        let other = lamp
            .spawn(&mut edited, Transform::IDENTITY)
            .expect("spawned");
        let new = edited.save_scene(&[root, other]).expect("saved");
        let crate::ecs::SceneEdit::Creates(creates) =
            world.scene_edit(Some(&old), &new).expect("an edit")
        else {
            panic!("the other's root first");
        };
        let other_id = edited.id(other).expect("alive");
        assert!(
            matches!(&creates[..], [Operation::Create { entity: Some(made), components, .. }]
                if *made == other_id && components[0].0 == "PrefabRoot"),
            "{creates:?}"
        );
        let mut log = Log::default();
        log.apply_all(world, creates).expect("made");
        let crate::ecs::SceneEdit::Changes(rest) =
            world.scene_edit(Some(&old), &new).expect("an edit")
        else {
            panic!("the rest");
        };
        log.apply_all(world, rest).expect("applied");
        assert_eq!(world.hash().content, edited.hash().content);
    }

    /// A scene edit unlinking an instance keeps its entities as its own, as the file holds them;
    /// relinking deletes the own entities holding the prefab's IDs first.
    #[cfg(feature = "dev")]
    #[test]
    fn a_scene_edit_unlinks_an_instance_and_links_it_again() {
        let (mut cx, [lamp]) = with([("lamp.prefab", LAMP)]);
        let world = cx.world_mut();
        let root = lamp.spawn(world, Transform::IDENTITY).expect("spawned");
        let root_id = world.id(root).expect("alive");
        let linked_text = world.save_scene(&[root]).expect("saved");
        let content = world.hash().content;
        let mut unlinked = world.clone();
        let unlink = Operation::Unlink { entity: root_id };
        Log::default()
            .apply(&mut unlinked, unlink)
            .expect("unlinked");
        let own = unlinked.entity(root_id).expect("alive");
        let unlinked_text = unlinked.save_scene(&[own]).expect("saved");
        // The edit from `old` to `new`: creates first, then the rest.
        let edit = |world: &mut World, log: &mut Log, old: &str, new: &str| {
            let crate::ecs::SceneEdit::Creates(creates) =
                world.scene_edit(Some(old), new).expect("an edit")
            else {
                panic!("the link first");
            };
            log.apply_all(world, creates).expect("linked");
            let crate::ecs::SceneEdit::Changes(rest) =
                world.scene_edit(Some(old), new).expect("an edit")
            else {
                panic!("the rest");
            };
            log.apply_all(world, rest).expect("applied");
        };
        let mut log = Log::default();
        edit(world, &mut log, &linked_text, &unlinked_text);
        assert_eq!(world.get::<PrefabRoot>(root), None);
        assert_eq!(
            world.get::<Name>(root),
            Some(&Name("lamp".to_owned())),
            "kept"
        );
        assert!(world.contains(of(world, root, 0xb2)), "its bulb kept");
        assert_eq!(world.hash().content, unlinked.hash().content);
        edit(world, &mut log, &unlinked_text, &linked_text);
        assert_eq!(world.get::<PrefabRoot>(root), Some(&PrefabRoot(lamp)));
        assert_eq!(world.hash().content, content, "linked again");
        check_ids(world);
    }

    /// Relinking by scene edit deletes the world's own entities holding the prefab's IDs, which
    /// clears what names them from outside, like a joint's bodies (`Component::cleared`). That
    /// then names the entities the link remakes under those IDs: the world is the file's, as
    /// before the unlink. Undo returns to the unlinked world, naming its own entity again.
    #[cfg(feature = "dev")]
    #[test]
    fn a_relink_keeps_what_names_its_entities_from_outside() {
        let (mut cx, [lamp]) = with([("lamp.prefab", LAMP)]);
        let world = cx.world_mut();
        let root = lamp.spawn(world, Transform::IDENTITY).expect("spawned");
        let root_id = world.id(root).expect("alive");
        let bulb = of(world, root, 0xb2);
        let joint = crate::Joint3D {
            body_a: Some(root),
            body_b: Some(bulb),
            ..crate::Joint3D::default()
        };
        let tie = world.spawn(joint);
        let linked_text = world.save_scene(&[root, tie]).expect("saved");
        let content = world.hash().content;
        let mut unlinked = world.clone();
        let unlink = Operation::Unlink { entity: root_id };
        Log::default()
            .apply(&mut unlinked, unlink)
            .expect("unlinked");
        let own = unlinked.entity(root_id).expect("alive");
        let unlinked_text = unlinked.save_scene(&[own, tie]).expect("saved");
        let edit = |world: &mut World, log: &mut Log, old: &str, new: &str| {
            let crate::ecs::SceneEdit::Creates(creates) =
                world.scene_edit(Some(old), new).expect("an edit")
            else {
                panic!("the link first");
            };
            log.apply_all(world, creates).expect("linked");
            let crate::ecs::SceneEdit::Changes(rest) =
                world.scene_edit(Some(old), new).expect("an edit")
            else {
                panic!("the rest");
            };
            log.apply_all(world, rest).expect("applied");
        };
        let mut log = Log::default();
        edit(world, &mut log, &linked_text, &unlinked_text);
        let held = world.get::<crate::Joint3D>(tie).expect("kept").body_b;
        assert_eq!(held, Some(bulb), "its own bulb, unlinked");
        edit(world, &mut log, &unlinked_text, &linked_text);
        let remade = of(world, root, 0xb2);
        assert_ne!(remade, bulb, "made again");
        let held = world.get::<crate::Joint3D>(tie).expect("kept").body_b;
        assert_eq!(held, Some(remade), "naming the bulb made again");
        assert_eq!(world.hash().content, content, "the file's world");
        let roots: Vec<Entity> = world.roots().collect();
        world
            .save_scene(&roots)
            .expect("nothing names an entity gone");
        assert!(log.undo(world).expect("undone"));
        assert!(log.undo(world).expect("undone"));
        let held = world.get::<crate::Joint3D>(tie).expect("kept").body_b;
        assert_eq!(held, Some(bulb), "its own bulb again");
        log.clear(world);
        check_ids(world);
    }

    /// Amended sets of an instance's link are each an edit: each undo restores the prior prefab,
    /// its entities under their handles.
    #[test]
    fn sets_of_an_instances_link_are_each_an_edit_of_its_own() {
        let post = post();
        let globe = LAMP.replace("(\"bulb\")", "(\"globe\")");
        let (mut cx, [lamp, post, globe]) = with([
            ("lamp.prefab", LAMP),
            ("post.prefab", &post),
            ("globe.prefab", &globe),
        ]);
        let world = cx.world_mut();
        let root = lamp.spawn(world, Transform::IDENTITY).expect("spawned");
        let root_id = world.id(root).expect("alive");
        let bulb = of(world, root, 0xb2);
        let bulb_id = world.id(bulb).expect("alive");
        let relink = |prefab| Operation::Set {
            entity: root_id,
            component: "PrefabRoot".into(),
            path: "0".to_owned(),
            value: Value::Prefab(prefab),
        };
        let lit = world.hash().content;
        let mut log = Log::default();
        log.amend(world, relink(post)).expect("relinked");
        let posted = world.hash().content;
        // A prefab whose bulb's ID is the lamp's, which the post's undo would find taken.
        log.amend(world, relink(globe)).expect("relinked");
        log.undo(world).expect("the post again");
        assert_eq!(world.hash().content, posted);
        log.undo(world).expect("the lamp again");
        assert_eq!(world.hash().content, lit);
        assert_eq!(world.entity(bulb_id), Some(bulb), "under its handle");
        check_ids(world);
    }

    /// A prefab's text of bare entities, each an ID and its parent's (none for the root).
    #[cfg(feature = "dev")]
    fn bare(entities: &[(u64, Option<u64>)]) -> Arc<str> {
        let mut text = String::from("(\n    entities: {\n");
        for &(id, parent) in entities {
            text.push_str(&match parent {
                None => format!("        \"{id:016x}\": (),\n"),
                Some(up) => format!(
                    "        \"{id:016x}\": (\n            Parent: (\"{up:016x}\"),\n        ),\n"
                ),
            });
        }
        text.push_str("    },\n)\n");
        text.into()
    }

    /// When the prefab's file drops an entity and puts its child in its place, the child moves
    /// out first and doesn't go with it.
    #[cfg(feature = "dev")]
    #[test]
    fn an_entity_whose_prefab_parent_goes_moves_out_first() {
        let chain = bare(&[(1, None), (2, Some(1)), (3, Some(2))]);
        let (mut cx, [prefab]) = with([("chain.prefab", &chain)]);
        let world = cx.world_mut();
        let root = prefab.spawn(world, Transform::IDENTITY).expect("spawned");
        let middle = of(world, root, 2);
        let shorter = bare(&[(1, None), (3, Some(1))]);
        world.refit_prefab(world.refits(prefab, &shorter).expect("loads"));
        world.assets_mut().set_prefab_text(prefab, shorter);
        assert!(!world.contains(middle), "gone");
        let last = of(world, root, 3);
        assert_eq!(world.parent(last).expect("alive"), Some(root));
        world.save_scene(&[root]).expect("saved whole");
        check_ids(world);
    }

    /// As its prefab's file gives an entity under a new one with a later ID, the instance spawns
    /// both, each under its parent.
    #[cfg(feature = "dev")]
    #[test]
    fn an_instance_spawns_what_its_prefab_gives_now_parents_first() {
        let alone = bare(&[(1, None)]);
        let (mut cx, [prefab]) = with([("alone.prefab", &alone)]);
        let world = cx.world_mut();
        let root = prefab.spawn(world, Transform::IDENTITY).expect("spawned");
        let nested = bare(&[(1, None), (5, Some(9)), (9, Some(1))]);
        world.refit_prefab(world.refits(prefab, &nested).expect("loads"));
        world.assets_mut().set_prefab_text(prefab, nested);
        let (up, under) = (of(world, root, 9), of(world, root, 5));
        assert_eq!(world.parent(up).expect("alive"), Some(root));
        assert_eq!(world.parent(under).expect("alive"), Some(up));
        world.save_scene(&[root]).expect("saved whole");
        check_ids(world);
    }

    /// A duplicate refuses a given ID that an instance copy's entity would take, or take from
    /// another. The ID drawn for an instance copy leaves free those its entities take.
    #[test]
    fn a_duplicates_instances_copies_take_the_ids_mixed_from_theirs() {
        let (mut cx, [lamp]) = with([("lamp.prefab", LAMP)]);
        let world = cx.world_mut();
        let root = lamp.spawn(world, Transform::IDENTITY).expect("spawned");
        let root_id = world.id(root).expect("alive");
        let mut log = Log::default();
        let child = Operation::Create {
            entity: None,
            parent: Some(root_id),
            components: Vec::new(),
        };
        log.apply(world, child).expect("an entity of its own");
        let copy = id(0xa0a0);
        let bulb = linked(copy, id(0xb2)).expect("an ID");
        let duplicate = |copies| Operation::Duplicate {
            entity: root_id,
            copies,
        };
        let error = log.apply(world, duplicate(vec![copy, bulb]));
        assert_eq!(error.expect_err("its bulb's").code(), "ops.bad_copies");
        let taken = Operation::Create {
            entity: Some(bulb),
            parent: None,
            components: Vec::new(),
        };
        log.apply(world, taken).expect("made");
        let error = log.apply(world, duplicate(vec![copy, id(0xc0c0)]));
        assert_eq!(error.expect_err("its bulb's, taken").code(), "ecs.id_taken");

        // The ID the world draws next, its copy's bulb's taken.
        let next = {
            let mut probe = world.clone();
            let drawn = probe.spawn(Transform::IDENTITY);
            probe.id(drawn).expect("alive")
        };
        let taken = Operation::Create {
            entity: Some(linked(next, id(0xb2)).expect("an ID")),
            parent: None,
            components: Vec::new(),
        };
        log.apply(world, taken).expect("made");
        log.apply(world, duplicate(Vec::new())).expect("copied");
        let Some([Operation::Duplicate { copies, .. }]) = log.undoable() else {
            panic!("a duplicate");
        };
        let copied = world.entity(copies[0]).expect("copied");
        assert_ne!(copies[0], next, "drawn again");
        assert_eq!(
            world.parent(of(world, copied, 0xb2)).expect("alive"),
            Some(copied)
        );
        world.save_scene(&[copied]).expect("saved whole");
        check_ids(world);
    }

    /// An instance an undo restores (after a delete or relink) takes the prefab file's changes
    /// made meanwhile; a redo removes what it took.
    #[cfg(feature = "dev")]
    #[test]
    fn an_instance_an_undo_puts_back_takes_what_its_prefab_changed_meanwhile() {
        let post = post();
        let (mut cx, [lamp, post]) = with([("lamp.prefab", LAMP), ("post.prefab", &post)]);
        let world = cx.world_mut();
        let root = lamp.spawn(world, Transform::IDENTITY).expect("spawned");
        let root_id = world.id(root).expect("alive");
        // The lamp's file with one more entity under its root, of local ID `local`.
        let with_child = |text: &str, local: &str, name: &str| -> Arc<str> {
            text.replace(
                "    },\n)\n",
                &format!(
                    "        \"{local}\": (\n            Name: (\"{name}\"),\n            Parent: \
                     (\"00000000000000a1\"),\n        ),\n    }},\n)\n"
                ),
            )
            .into()
        };
        let refit = |world: &mut World, text: Arc<str>| {
            world.refit_prefab(world.refits(lamp, &text).expect("loads"));
            world.assets_mut().set_prefab_text(lamp, text);
        };
        let mut log = Log::default();
        log.apply(world, Operation::Delete { entity: root_id })
            .expect("deleted");
        let shaded = with_child(LAMP, "00000000000000c5", "shade");
        refit(world, Arc::clone(&shaded));
        log.undo(world).expect("put back");
        let shade = of(world, root, 0xc5);
        assert_eq!(world.parent(shade).expect("alive"), Some(root));
        world.save_scene(&[root]).expect("saved whole");

        let relink = Operation::Set {
            entity: root_id,
            component: "PrefabRoot".into(),
            path: "0".to_owned(),
            value: Value::Prefab(post),
        };
        log.apply(world, relink).expect("relinked");
        refit(world, with_child(&shaded, "00000000000000c6", "cap"));
        log.undo(world).expect("the lamp again");
        let cap = of(world, root, 0xc6);
        let cap_id = world.id(cap).expect("alive");
        world.save_scene(&[root]).expect("saved whole");
        log.redo(world).expect("the post again");
        assert_eq!(world.entity(cap_id), None, "taken out with the lamp's");
        log.undo(world).expect("the lamp again");
        assert_eq!(world.entity(cap_id), Some(cap), "under its handle");
        world.save_scene(&[root]).expect("saved whole");
        check_ids(world);
    }

    /// Code reads an instance's link but can't change it: the entities under it would be another
    /// prefab's.
    #[test]
    fn code_reads_an_instances_link_but_cant_change_it() {
        let post = post();
        let (mut cx, [lamp, post]) = with([("lamp.prefab", LAMP), ("post.prefab", &post)]);
        let world = cx.world_mut();
        let root = lamp.spawn(world, Transform::IDENTITY).expect("spawned");
        let content = world.hash().content;
        for (path, value) in [
            ("0", Value::Prefab(post)),
            ("", PrefabRoot(post).to_value()),
        ] {
            let error = world.set_field(root, "PrefabRoot", path, &value);
            let error = error.expect_err("the world's to set");
            assert_eq!(error.code(), "reflect.read_only", "{error:#}");
        }
        let read = world.get_field(root, "PrefabRoot", "0");
        assert_eq!(read.ok(), Some(Value::Prefab(lamp)));
        assert_eq!(world.hash().content, content, "nothing changed");
    }

    /// Saving is refused for an instance entity code made a root or put under an unsaved entity,
    /// as a load would return it to its prefab parent. Under a saved one, it's saved there.
    #[test]
    fn an_instances_entity_is_saved_under_a_parent_the_scene_saves() {
        let (mut cx, [lamp]) = with([("lamp.prefab", LAMP)]);
        let world = cx.world_mut();
        let root = lamp.spawn(world, Transform::IDENTITY).expect("spawned");
        let bulb = of(world, root, 0xb2);
        let elsewhere = world.spawn(Transform::IDENTITY);
        for parent in [None, Some(elsewhere)] {
            world.set_parent(bulb, parent).expect("code may");
            let error = world.save_scene(&[root]).expect_err("put back as it loads");
            assert_eq!(error.code(), "scene.prefab_parent", "{error:#}");
        }
        let own = world.spawn(Transform::IDENTITY);
        world.set_parent(own, Some(root)).expect("no cycle");
        world.set_parent(bulb, Some(own)).expect("no cycle");
        let text = world.save_scene(&[root]).expect("saved");
        let mut loaded = world.emptied();
        loaded.load_scene(&text).expect("loaded");
        let moved = loaded.entity(world.id(bulb).expect("alive"));
        let up = loaded.parent(moved.expect("loaded")).expect("alive");
        assert_eq!(up.and_then(|up| loaded.id(up)), world.id(own));
    }
}

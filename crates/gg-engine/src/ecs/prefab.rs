//! Prefabs' instances in a world: the entities the world spawns under each entity with a
//! [`PrefabRoot`], as `PrefabRoot` says, their IDs mixed from the root's, so that the root's ID
//! gives them wherever it's loaded, made or restored; which of a world's entities an instance
//! gives, and what its prefab gives each; and, in development builds, what they take as their
//! prefab's file changes.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
#[cfg(feature = "dev")]
use std::sync::Arc;

use gg_core::{Error, Result};

use super::scene::{Expanding, Template};
use super::table::{AnyColumn, ComponentId};
use super::{Entity, EntityId, Parent, World};
use crate::reflect::Value;
use crate::{ModelRoot, Prefab, PrefabRoot, Transform};

/// An instance of a prefab in a world: its prefab's template, and the IDs its parts have in it,
/// its root's first.
pub(super) struct Instance {
    pub(super) template: Rc<Template>,
    pub(super) ids: Vec<EntityId>,
}

impl Instance {
    /// Part `part`'s component `component`, as the prefab gives it, in a one-row column of its own,
    /// its references to the prefab's entities retargeted to the instance's in `world`, or to none
    /// for one that isn't alive; `None` if the prefab gives it none.
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

    /// Part `part`'s component `component`, as [`column`](Self::column) gives it, as reflection
    /// reads it.
    pub(super) fn value(
        &self,
        world: &World,
        part: usize,
        component: ComponentId,
    ) -> Option<Value> {
        let column = self.column(world, part, component)?;
        Some(column.value(0))
    }

    /// The types of the components the prefab gives part `part`: the root's but its `Transform`,
    /// which is the instance's own.
    pub(super) fn components(&self, world: &World, part: usize) -> Vec<ComponentId> {
        let transform = world.component::<Transform>();
        let row = self.template.parts[part].row.iter();
        let row = row.filter(|&&(component, _)| part > 0 || Some(component) != transform);
        row.map(|&(component, _)| component).collect()
    }

    /// Each part's handle in the template's world, with the instance's entity of it in `world`.
    fn handles(&self, world: &World) -> HashMap<Entity, Entity> {
        let parts = self.template.parts.iter().zip(&self.ids);
        let handles = parts.map(|(part, &id)| (part.handle, world.entity(id)));
        handles
            .map(|(handle, entity)| (handle, entity.unwrap_or(Entity::DEAD)))
            .collect()
    }

    /// The parent part `part` has in the prefab, as the instance's entity of it in `world`, if
    /// it's alive: none for the root.
    pub(super) fn parent(&self, world: &World, part: usize) -> Option<Entity> {
        let up = self.template.parts[part].parent?;
        world.entity(self.ids[up])
    }
}

/// The entities instances give in part of a world, each by its ID, with its instance and its part:
/// each by the outermost instance that gives it, whose template gives it as the prefabs inside it
/// have it there.
#[derive(Default)]
pub(super) struct Claims {
    pub(super) instances: Vec<(Entity, Instance)>,
    by_id: HashMap<EntityId, (usize, usize)>,
}

impl Claims {
    /// The instance that gives the entity of ID `id`, by its place among `instances`, and the
    /// entity's part in its template, if one does.
    pub(super) fn of(&self, id: EntityId) -> Option<(usize, usize)> {
        self.by_id.get(&id).copied()
    }

    /// Whether an instance gives the entity of ID `id`.
    pub(super) fn has(&self, id: EntityId) -> bool {
        self.by_id.contains_key(&id)
    }

    /// Takes in what `instance`, whose root is `root`, gives.
    pub(super) fn claim(&mut self, root: Entity, instance: Instance) {
        let at = self.instances.len();
        for (part, &id) in instance.ids.iter().enumerate().skip(1) {
            self.by_id.insert(id, (at, part));
        }
        self.instances.push((root, instance));
    }
}

/// What each instance in a world takes as its prefab's file changes, as [`World::refits`] finds
/// it: each outermost instance as its prefab gave it, and as it gives it now.
#[cfg(feature = "dev")]
pub(crate) struct Refits {
    /// The serial of the world it was found in, which alone takes it.
    world: u64,
    instances: Vec<(Instance, Instance)>,
}

impl World {
    /// Spawns an instance of `prefab` at `at`, and returns its root, as [`Prefab::spawn`] says.
    pub(crate) fn spawn_prefab(&mut self, prefab: Prefab, at: Transform) -> Result<Entity> {
        assert!(
            self.assets.has_prefab(prefab),
            "a prefab spawns into a world that shares the assets of the context that added it"
        );
        let template = Expanding::default().template(self, prefab)?;
        // Drawn until its entities' IDs are free too, as a model's nodes' are.
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

    /// The instance whose root is `root`, which is alive, of the prefab its [`PrefabRoot`] names,
    /// as `expanding` makes its template; `None` if it has none.
    ///
    /// Fails as the template does, as [`Expanding::template`] says, and with `ecs.bad_id` for the
    /// one ID an entity of it mixes to zero.
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

    /// What the instances in `tops`' subtrees, which are alive, give, each by the outermost: as
    /// each parent's instance, if it has one, before its children's.
    ///
    /// Fails as [`instance`](Self::instance) does.
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

    /// The instance whose prefab gives `entity`, which is alive, with its root and the entity's
    /// part in its template: the outermost, whose template gives it as the prefabs inside the
    /// instance's have it; `None` if none does, as for an instance's root, or an entity given
    /// under an instance.
    ///
    /// Fails as [`instance`](Self::instance) does for the instance of one of its ancestors.
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

    /// The entity that `entity`, which is alive, goes with, which tools change in its place: the
    /// root of the model whose node it is, or of the outermost instance whose prefab gives it, in
    /// turn; `entity` itself if it's neither. An instance whose template doesn't load gives none.
    #[cfg(feature = "dev")]
    pub(crate) fn owner(&self, entity: Entity) -> Entity {
        let mut at = self.node_root(entity);
        let mut expanding = Expanding::default();
        while let Ok(Some((root, _, _))) = self.claimer(at, &mut expanding) {
            at = self.node_root(root);
        }
        at
    }

    /// Fails with `ecs.prefab_entity` if an instance's prefab gives `entity`, which is alive, as
    /// [`claimer`](Self::claimer) finds it: as a tool's delete of it, or a move of it, which would
    /// take it out of its instance, is refused.
    pub(super) fn not_a_prefabs(&self, entity: Entity) -> Result<()> {
        let Some((root, _, _)) = self.claimer(entity, &mut Expanding::default())? else {
            return Ok(());
        };
        Err(self.prefabs_own(entity, root, "it"))
    }

    /// The `ecs.prefab_entity` that `what` of `entity` is its prefab's, which the instance whose
    /// root is `root` gives it: so that tools take none of it away.
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

    /// The value of `entity`'s `component` at `path` as the prefab of the instance that gives it
    /// has it: the prefab's root's, but its `Transform`, for an instance's root; and as the
    /// outermost instance's prefab gives it otherwise, with the prefabs inside it, for an entity
    /// one gives; its references to the prefab's entities retargeted to the instance's.
    ///
    /// Fails with `ecs.no_prefab` for an entity no instance gives that's no instance's root, or a
    /// component its prefab doesn't give it; as [`claimer`](Self::claimer) does; and as
    /// [`get_field`](Self::get_field) fails for a path the component hasn't.
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

    /// Gives `root`, which is alive, of `instance`'s ID and with its `PrefabRoot`, each component
    /// the prefab's root has that it hasn't, but its `Transform`; and spawns the prefab's other
    /// entities under it, under `instance`'s IDs, which no living entity has, each parent's
    /// children in the order of their IDs, as a scene's load spawns them, their references to the
    /// prefab's entities the instance's, and their models' nodes. Returns the types of the
    /// components given the root, and the IDs of the entities spawned that are its children.
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

    /// What each instance in the world takes as prefab `prefab`'s file holds `text` in place of
    /// what it held, which [`refit_prefab`](Self::refit_prefab) has it take: each outermost
    /// instance, as its prefab gave it and as it gives it now, so that a context checks that each
    /// of its worlds takes the text before any does.
    ///
    /// Fails as an instance's template does, of the prefab as it was or as `text` holds it, as
    /// [`Expanding::template`] says, such as with `scene.prefab_cycle` for a text that links the
    /// prefab itself.
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

    /// Has each instance in the world take what its prefab's file holds now, in place of what it
    /// held, as `refits`, which [`refits`](Self::refits) found in this world, says: each field of
    /// an instance's entity that its prefab gave as it was takes what it gives now, and each that
    /// differed is kept; a component the prefab gives now that an entity hasn't is given, and one
    /// it gave no more is taken away where it's as the prefab gave it; an entity the prefab gives
    /// now is spawned, under its parent, and one it gives no more despawned, with what's under it,
    /// under their IDs; an entity's parent goes where the prefab has it now, where it was where the
    /// prefab had it, before what the prefab gives no more goes, so that it doesn't go with it;
    /// and models' nodes follow their roots' models. Instances inside others' prefabs take it
    /// through the outermost's, and those of other prefabs that link it take it too.
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

    /// The outermost instances in `root`'s subtree, which is alive, each by its root's ID with its
    /// template, as [`claims`](Self::claims) finds them: as an operation that takes them out of
    /// the world keeps them, so that each can take what its prefab's file changed while it was
    /// out, as [`refit_kept`](Self::refit_kept) has it, once an undo or a redo puts it back. An
    /// instance whose template doesn't load gives none.
    #[cfg(feature = "dev")]
    pub(super) fn kept_instances(&self, root: Entity) -> Vec<(EntityId, Rc<Template>)> {
        let claims = self.claims(&[root], &mut Expanding::default());
        let instances = claims.unwrap_or_default().instances.into_iter();
        let kept =
            instances.map(|(root, instance)| (self.id(root).expect("alive"), instance.template));
        kept.collect()
    }

    /// Has each instance `kept` names, as [`kept_instances`](Self::kept_instances) found it
    /// before an operation took it out, and which an undo or a redo has put back, take what its
    /// prefab gives now in place of what it gave then, as [`refit_prefab`](Self::refit_prefab) has
    /// a living one take it. An instance gone with one refitted before it is left out, as is one
    /// whose prefab's template doesn't load now, which a save then refuses.
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

    /// How many ancestors `entity`, which is alive, has.
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

    /// Has an instance, whose root is alive, take what its prefab gives as `new` has it, in place
    /// of what it gave as `old` had it, as [`refit_prefab`](Self::refit_prefab) says.
    #[cfg(feature = "dev")]
    fn refit(&mut self, old: &Instance, new: &Instance) {
        let old_parts: HashMap<EntityId, usize> = (old.ids.iter().enumerate())
            .map(|(part, &id)| (id, part))
            .collect();
        let new_parts: HashSet<EntityId> = new.ids.iter().copied().collect();
        // Each part after its parent, so that a part's parent is placed before it's asked of.
        let downward = new.template.downward();
        // Those the prefab gives now, which no living entity has, under a parent that's alive or
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
        // Moved before those the prefab gives no more go, so that none goes with them, under
        // parents that are alive or have just come.
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
                // A parent under it, as an entity code moved there, keeps it where it is.
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

    /// Has `entity`'s `component` take what its prefab gives now, `new`, where it has what the
    /// prefab gave, `old`, as [`refit_prefab`](Self::refit_prefab) says: field by field, for a
    /// struct of named fields.
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
                    // A nested instance's link too, whose entities the outer instance's refit
                    // spawns and despawns itself.
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

    /// Spawns the parts of `new` that `fresh` says, under their parents, as
    /// [`spawn_instance`](Self::spawn_instance) spawns an instance's.
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
    use gg_core::math::{Vec3, Vec4};

    use super::*;
    use crate::ecs::id::linked;
    use crate::ecs::id::tests::check_ids;
    use crate::reflect::Reflect;
    use crate::{Context, Log, Name, Operation};

    /// A lamp: its light's color and brightness, and the entity it aims at.
    #[derive(Clone, Debug, PartialEq, crate::Component)]
    struct Lamp {
        color: Vec4,
        bright: f32,
        aim: Option<Entity>,
    }

    /// A lamp, its root turned away by a transform its instances don't take, aiming at its bulb.
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

    /// A context whose world has registered `Lamp`, with prefabs of `texts`, each its path and its
    /// text, named by their paths.
    fn with<const N: usize>(texts: [(&str, &str); N]) -> (Context, [Prefab; N]) {
        let mut cx = Context::seeded(1);
        let world = cx.world_mut();
        world.register::<Lamp>().expect("describes itself");
        let assets = world.assets_mut();
        let prefabs = texts
            .map(|(path, text)| assets.add_prefab(path.into(), path.into(), Some(text.into())));
        (cx, prefabs)
    }

    /// The entity whose ID mixes from `root`'s and `local`, in `world`.
    fn of(world: &World, root: Entity, local: u64) -> Entity {
        let root = world.id(root).expect("alive");
        let mixed = linked(root, id(local)).expect("an ID");
        world.entity(mixed).expect("spawned")
    }

    /// An instance's root takes its prefab's root's components but its transform, its own; the
    /// prefab's other entities spawn under it, their IDs mixed from its, its references to them
    /// its own. A scene saves each instance as its root, its link, its transform and what else
    /// differs, field by field, and each entity the prefab gives where something does; the scene
    /// loads to the same world, and saves to the same bytes.
    #[test]
    fn a_scene_saves_what_an_instance_has_otherwise_than_its_prefab() {
        let (mut cx, [lamp]) = with([("lamp.prefab", LAMP)]);
        let world = cx.world_mut();
        let at = Transform {
            translation: Vec3::new(-2.0, 0.0, 0.0),
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
        let up = Value::Vec3(Vec3::new(0.0, 3.0, 0.0));
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
        // An instance's root, with what else it changes, `lamp`, at `at`.
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

    /// A field an instance's entity leaves out takes its prefab's, not its type's default; and an
    /// entity of the prefab code took away, or a component of one, is refused as it's saved.
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
        // Nor does a root's own transform take what it leaves out from the prefab's root's.
        let placed = text.replace("0000000000000100", "0000000000000200").replace(
            "            PrefabRoot: (\"lamp.prefab\"),\n",
            "            PrefabRoot: (\"lamp.prefab\"),\n            Transform: (\n                \
             scale: (2.0, 2.0, 2.0),\n            ),\n",
        );
        let other = world.load_scene(&placed).expect("loaded")[0];
        let at = world.get::<Transform>(other).expect("its own");
        assert_eq!(at.translation, Vec3::ZERO, "its default, not the prefab's");
        assert_eq!(at.scale, Vec3::splat(2.0));

        world.remove::<Name>(bulb).expect("code may");
        let error = world.save_scene(&roots).expect_err("its name gone");
        assert_eq!(error.code(), "scene.prefab_missing", "{error:#}");
        world.despawn(bulb).expect("code may");
        let error = world.save_scene(&roots).expect_err("its bulb gone");
        assert_eq!(error.code(), "scene.prefab_missing", "{error:#}");
    }

    /// An instance inside a prefab spawns under the instance of that prefab, its IDs mixed again
    /// from its own root's, with what the outer prefab gives it otherwise; a scene saves the outer
    /// instance's root alone, and an entity the inner gives that differs under its mixed ID.
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
        assert_eq!(place, Vec3::new(0.0, 9.0, 0.0), "where the post moved it");
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

    /// A model's root in a prefab spawns its model's nodes in each instance, their IDs mixed from
    /// the instance's entity's, as anywhere; a scene saves the instance without them.
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
        // A prefab of a root, and a child of it that links prefab `of`.
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

    /// Tools create an instance, one edit with its entities; change its entities' fields and
    /// revert them; change its prefab and take it away, its entities and their fields coming back
    /// with an undo; unlink it into entities of its own; and copy it, the copies' IDs mixed from
    /// the copy's; but take none of its prefab's entities or components away.
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

        // An entity of its own under the prefab's, and a component of its own, which go freely.
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
        let up = Value::Vec3(Vec3::new(0.0, 5.0, 0.0));
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
        assert_eq!(place(world), Vec3::new(0.0, 1.0, 0.0), "the prefab's");
        log.undo(world).expect("undone");
        assert_eq!(place(world), Vec3::new(0.0, 5.0, 0.0));

        // Another prefab, and back, its own entity and its field as they were.
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

        // Unlinked, its entities its own, which go freely; linked again by the undo.
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

        // A copy, its bulb its own copy's, mixed from the copy's ID, its field kept.
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
        assert_eq!(at, Vec3::new(0.0, 5.0, 0.0));
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

    /// As its prefab's file changes, an instance takes each field it had as the prefab did, and
    /// keeps the rest; the entities the prefab gives now spawn, under their IDs, and those it
    /// gives no more go; and a text that doesn't load changes nothing.
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

        // Its bulb gone, and its lamp's aim with it, as the prefab's.
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

    /// A scene file saved with an instance's entity changed sets that entity's field; saved with
    /// another instance, it makes the instance's root with its link first, its prefab's entities
    /// with it, and then the rest.
    #[cfg(feature = "dev")]
    #[test]
    fn a_scene_edit_links_an_instance_before_the_rest() {
        let (mut cx, [lamp]) = with([("lamp.prefab", LAMP)]);
        let world = cx.world_mut();
        let root = lamp.spawn(world, Transform::IDENTITY).expect("spawned");
        let old = world.save_scene(&[root]).expect("saved");
        let mut edited = world.clone();
        let bulb = of(&edited, root, 0xb2);
        let up = Value::Vec3(Vec3::new(0.0, 7.0, 0.0));
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

    /// A scene file saved with an instance unlinked unlinks it, its entities staying, its own, as
    /// the file holds them; saved linked again, it links it again, the entities of its own that
    /// hold the IDs its prefab gives going first.
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
        // The edit from `old` to `new`, its creates first, then the rest.
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

    /// Sets of an instance's link that a gesture amends are each an edit of its own: each undo
    /// puts back the prefab before, its entities under their handles.
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
        // A prefab whose bulb's ID is the lamp's, which the post's set's undo would find taken.
        log.amend(world, relink(globe)).expect("relinked");
        log.undo(world).expect("the post again");
        assert_eq!(world.hash().content, posted);
        log.undo(world).expect("the lamp again");
        assert_eq!(world.hash().content, lit);
        assert_eq!(world.entity(bulb_id), Some(bulb), "under its handle");
        check_ids(world);
    }

    /// A prefab's text of bare entities, each its ID and its parent's, the root's none.
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

    /// As its prefab's file takes an entity away and puts the one under it in its place, an
    /// instance's entity of that one moves out from under it before it goes, not going with it.
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

    /// As its prefab's file gives an entity under a new one whose ID comes after its own, an
    /// instance spawns both, each under its parent.
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

    /// A duplicate refuses an ID it's given that an instance's copy gives one of its entities, or
    /// that a copy's entity would take from another; and the ID it draws for an instance's copy
    /// leaves free those its entities take.
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

    /// An instance that an undo puts back takes what its prefab's file changed while it was out,
    /// as a deleted one's undo puts it back, and as a relinked one's does; and a redo takes out
    /// what it took.
    #[cfg(feature = "dev")]
    #[test]
    fn an_instance_an_undo_puts_back_takes_what_its_prefab_changed_meanwhile() {
        let post = post();
        let (mut cx, [lamp, post]) = with([("lamp.prefab", LAMP), ("post.prefab", &post)]);
        let world = cx.world_mut();
        let root = lamp.spawn(world, Transform::IDENTITY).expect("spawned");
        let root_id = world.id(root).expect("alive");
        // The lamp's file with an entity more under its root, of local ID `local`.
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

    /// Code reads an instance's link, but changes it no way, since the entities under it would
    /// then be another prefab's.
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

    /// An entity an instance gives that code made a root, or put under an entity a scene doesn't
    /// save, is refused as it's saved, as a load would put it back under its prefab's parent;
    /// under one the scene saves, it's saved there.
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

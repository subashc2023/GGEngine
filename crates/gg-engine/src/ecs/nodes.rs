//! A model's nodes in a world: the entities the world spawns under each entity with a
//! [`ModelRoot`], as `ModelRoot` says, their IDs mixed from the root's, so that the root's ID gives
//! them wherever it's loaded, made or restored.

use std::sync::Arc;

use gg_core::{Error, Result};

use super::id::{self, EntityId};
use super::{Entity, World};
use crate::model::Nodes;
use crate::{MeshDraw, Model, ModelNode, ModelRoot, Transform};

impl World {
    /// Spawns an entity at `at` with a [`ModelRoot`] of `model`, and its nodes, and returns it, as
    /// [`Model::spawn`] says.
    pub(crate) fn spawn_model(&mut self, model: Model, at: Transform) -> Entity {
        let assets = Arc::clone(&self.assets);
        let nodes = assets.nodes(model).expect(
            "a model spawns into a world that shares the assets of the context that added it",
        );
        // Drawn until its nodes' IDs are free too, as a prefab's copy's are.
        let id = loop {
            let id = self.stream.draw();
            let free = |id| !self.reserved(id);
            if free(id) && (0..nodes.parts.len()).all(|part| node_id(id, part).is_some_and(free)) {
                break id;
            }
            tracing::debug!(
                code = "ecs.reseed",
                id = %id,
                "Drew an ID that's taken, or that a node of the model would take"
            );
            self.stream.reseed();
        };
        let root = self.spawn_as(Some(id), at);
        self.put(root, ModelRoot(model)).expect("spawned");
        self.spawn_nodes(root, id, nodes);
        root
    }

    /// Makes `root`'s nodes those of the model its [`ModelRoot`] names, as the assets it shares
    /// hold them, or takes them away if it has none: as it's loaded or made with one, gains or
    /// loses one, and as its model's nodes change, which were `was`. Its nodes stay while
    /// they keep their places in the model's hierarchy, as code left them but for each one's draw,
    /// and, given `was`, the transforms the model changed; otherwise they're spawned again.
    pub(crate) fn fit_nodes(&mut self, root: Entity, was: Option<&Nodes>) {
        let model = self.get::<ModelRoot>(root).map(|&ModelRoot(model)| model);
        let assets = Arc::clone(&self.assets);
        let Some(nodes) = model.and_then(|model| assets.nodes(model)) else {
            self.despawn_nodes(root);
            return;
        };
        let had = self.nodes_of(root);
        let kept = had.len() == nodes.parts.len()
            && had
                .iter()
                .zip(&nodes.parts)
                .enumerate()
                .all(|(i, (&(part, node), wanted))| {
                    let parent = wanted.parent.map_or(root, |parent| had[parent].1);
                    part as usize == i && self.parent(node).ok() == Some(Some(parent))
                });
        if !kept {
            self.despawn_nodes(root);
            let id = self.id(root).expect("alive");
            self.spawn_nodes(root, id, nodes);
            return;
        }
        for (i, (&(_, node), part)) in had.iter().zip(&nodes.parts).enumerate() {
            let moved = was.is_some_and(|was| {
                was.parts.get(i).map(|was| was.transform) != Some(part.transform)
            });
            if moved {
                self.put(node, part.transform).expect("a node is alive");
            }
            let draw = part.primitive.map(|primitive| nodes.draws[primitive]);
            if self.get::<MeshDraw>(node).copied() != draw {
                let changed = match draw {
                    Some(draw) => self.put(node, draw),
                    None => self.take::<MeshDraw>(node).map(drop),
                };
                changed.expect("a node is alive");
            }
        }
    }

    /// Fits the nodes of each root of `model` to the model's nodes, as [`fit_nodes`] does, `was`
    /// being what they were: as its file's read again.
    ///
    /// [`fit_nodes`]: Self::fit_nodes
    #[cfg(feature = "dev")]
    pub(crate) fn fit_model(&mut self, model: Model, was: &Nodes) {
        let roots = self.query::<(Entity, &ModelRoot)>();
        let roots = roots.filter(|&(_, &ModelRoot(of))| of == model);
        let roots: Vec<Entity> = roots.map(|(root, _)| root).collect();
        for root in roots {
            self.fit_nodes(root, Some(was));
        }
    }

    /// Gives each model's node that has no draw the draw its part has, as the assets the world
    /// shares hold its model: as a world restored from a snapshot of a run that added other assets
    /// is, which leaves out what's opaque, draws among it. Nodes keep their places and the rest.
    pub(crate) fn redraw_nodes(&mut self) {
        let roots = self.query::<(Entity, &ModelRoot)>();
        let roots: Vec<(Entity, Model)> = roots
            .map(|(root, &ModelRoot(model))| (root, model))
            .collect();
        let assets = Arc::clone(&self.assets);
        for (root, model) in roots {
            let Some(nodes) = assets.nodes(model) else {
                continue;
            };
            for (part, node) in self.nodes_of(root) {
                let part = nodes.parts.get(part as usize);
                let draw = part
                    .and_then(|part| part.primitive)
                    .map(|at| nodes.draws[at]);
                if let Some(draw) = draw
                    && self.get::<MeshDraw>(node).is_none()
                {
                    self.put(node, draw).expect("a node is alive");
                }
            }
        }
    }

    /// Takes `root`'s nodes away, with whatever is under them.
    pub(crate) fn despawn_nodes(&mut self, root: Entity) {
        let Ok(children) = self.children(root) else {
            return;
        };
        let nodes: Vec<Entity> = children
            .filter(|&child| self.get::<ModelNode>(child).is_some())
            .collect();
        for node in nodes {
            self.despawn(node).expect("a node is alive");
        }
    }

    /// Fails with `ecs.model_node` if `entity` is a node of a model, which the world spawns under
    /// the model's root, as [`ModelRoot`] says: as a tool's change to it, or a child given it, is
    /// refused.
    pub(crate) fn not_a_node(&self, entity: Entity) -> Result<()> {
        let Some(&ModelNode(part)) = self.get::<ModelNode>(entity) else {
            return Ok(());
        };
        let root = self.node_root(entity);
        let name = |entity| self.name_of(entity);
        let model = self.get::<ModelRoot>(root).map(|&ModelRoot(model)| model);
        let path = model.map_or("", |model| self.assets.model_path(model));
        Err(Error::new(
            "ecs.model_node",
            format!(
                "entity {} is part {part} of model `{path}`, which the world spawns under its root, \
                 entity {}: change the model's file, or the root",
                name(entity),
                name(root)
            ),
        ))
    }

    /// The root of the model whose node `entity` is, which tools change in its place; `entity`
    /// itself if it's no model's node.
    pub(crate) fn node_root(&self, entity: Entity) -> Entity {
        let mut at = entity;
        while self.get::<ModelNode>(at).is_some() {
            match self.parent(at) {
                Ok(Some(parent)) => at = parent,
                _ => break,
            }
        }
        at
    }

    /// Spawns the nodes `nodes` gives under `root`, whose ID is `id`, each with its ID mixed from
    /// the root's and its place: but a node whose ID a living entity has, as a scene's entity
    /// might, which goes without, and those under it, warning `ecs.id_taken`. It allocates nothing
    /// once the world has spawned as many, as a game spawning a model each tick needs.
    fn spawn_nodes(&mut self, root: Entity, id: EntityId, nodes: &Nodes) {
        // Each part's node, or `Entity::DEAD` for one that goes without.
        let mut spawned = self.take_list();
        for (i, part) in nodes.parts.iter().enumerate() {
            let parent = part.parent.map_or(root, |parent| spawned[parent]);
            let free = node_id(id, i).filter(|node| !self.by_id.contains_key(node));
            let (true, Some(node_id)) = (parent != Entity::DEAD, free) else {
                if parent != Entity::DEAD {
                    tracing::warn!(
                        code = "ecs.id_taken",
                        root = %id,
                        part = i,
                        "Part {i} of the model that entity {id} is the root of isn't spawned, nor \
                         what's under it: another entity has its ID"
                    );
                }
                spawned.push(Entity::DEAD);
                continue;
            };
            let node = match part.primitive {
                Some(primitive) => {
                    self.spawn_as(Some(node_id), (part.transform, nodes.draws[primitive]))
                }
                None => self.spawn_as(Some(node_id), part.transform),
            };
            let part_at = u32::try_from(i).expect("fewer than 2^32 parts");
            self.put(node, ModelNode(part_at)).expect("spawned");
            let set = self.set_parent(node, Some(parent));
            set.expect("a new entity takes an earlier one as its parent");
            spawned.push(node);
        }
        self.keep_list(spawned);
    }

    /// `root`'s nodes, each with its part, reached from it through nodes alone, in the order of
    /// their parts.
    fn nodes_of(&self, root: Entity) -> Vec<(u32, Entity)> {
        let mut found = Vec::new();
        let mut next = vec![root];
        while let Some(entity) = next.pop() {
            for child in self.children(entity).expect("alive") {
                if let Some(&ModelNode(part)) = self.get::<ModelNode>(child) {
                    found.push((part, child));
                    next.push(child);
                }
            }
        }
        found.sort_unstable_by_key(|&(part, _)| part);
        found
    }
}

/// The ID of node `part` of the model whose root's ID is `root`: mixed from it, as a prefab's copy's
/// are, so that a root's ID gives its nodes'. `None` for the one part it mixes to zero.
pub(crate) fn node_id(root: EntityId, part: usize) -> Option<EntityId> {
    id::placed(root, EntityId::new(part as u64 + 1)?)
}

#[cfg(test)]
mod tests {
    use gg_core::math::Vec3;

    use super::*;
    use crate::model::tests::{added, imported};
    use crate::reflect::{Reflect, Value};
    use crate::{Context, Name, Operation};

    /// A context with model "m", of [`imported`], and a root of it spawned.
    fn spawned() -> (Context, Model, Entity) {
        let mut cx = Context::seeded(1);
        let (model, _) = added(&mut cx, "m", imported());
        let root = model.spawn(cx.world_mut(), Transform::IDENTITY);
        (cx, model, root)
    }

    /// The IDs of `root`'s nodes, by their parts, as `world` has them.
    fn node_ids(world: &World, root: Entity) -> Vec<EntityId> {
        let nodes = world.nodes_of(root).into_iter();
        nodes
            .map(|(_, node)| world.id(node).expect("alive"))
            .collect()
    }

    /// A scene saves a model's root alone, which spawns its nodes again as it loads, under the
    /// same IDs; one that holds a child the game gave a node is refused, as is saving a node.
    #[test]
    fn a_scene_saves_a_models_root_alone_and_its_load_spawns_the_nodes() {
        let (mut cx, model, root) = spawned();
        let world = cx.world();
        let text = world.save_scene(&[root]).expect("saved");
        let root_id = world.id(root).expect("alive");
        assert!(text.contains(&format!("\"{root_id}\"")), "{text}");
        assert!(text.contains("ModelRoot: (\"m\")"), "{text}");
        assert!(!text.contains("ModelNode"), "nodes aren't saved: {text}");
        let mut loaded = world.emptied();
        let roots = loaded.load_scene(&text).expect("loaded");
        assert_eq!(roots, [loaded.entity(root_id).expect("loaded")]);
        assert_eq!(node_ids(&loaded, roots[0]), node_ids(world, root));
        assert_eq!(
            loaded.hash().content,
            world.hash().content,
            "the same entities"
        );
        assert_eq!(loaded.get::<ModelRoot>(roots[0]), Some(&ModelRoot(model)));

        let node = world.nodes_of(root)[0].1;
        assert_eq!(
            world.save_scene(&[node]).expect_err("a node").code(),
            "ecs.model_node"
        );
        let world = cx.world_mut();
        let held = world.spawn(Transform::IDENTITY);
        world.set_parent(held, Some(node)).expect("code may");
        let error = world.save_scene(&[root]).expect_err("a child of a node");
        assert_eq!(error.code(), "scene.model_node");
        // Nor does a scene give an entity a node's component.
        let text = "(\n    entities: {\n        \"0000000000000100\": (\n            ModelNode: (0),\n        ),\n    },\n)\n";
        let error = world.emptied().load_scene(text).expect_err("the world's");
        assert_eq!(error.code(), "reflect.read_only", "{error:#}");
    }

    /// Tools give a root its model, change it and take it away, its nodes following, undone and
    /// redone exactly, and copy a root with its nodes, under IDs mixed from the copy's; but change
    /// no node, nor put anything under one, nor give an entity a `ModelNode`.
    #[test]
    fn tools_change_a_models_root_and_never_its_nodes() {
        let (mut cx, model, root) = spawned();
        let mut one = imported();
        one.parts.truncate(1);
        let (other, _) = added(&mut cx, "n", one);
        let world = cx.world_mut();
        let root_id = world.id(root).expect("alive");
        let first = node_ids(world, root);
        let node = first[0];
        let apply = |world: &mut World, mut operation: Operation| {
            world
                .apply(&mut operation)
                .map(|inverse| (operation, inverse))
        };

        let refused = [
            Operation::Set {
                entity: node,
                component: "Transform".into(),
                path: "scale".into(),
                value: Value::Vec3(Vec3::ONE),
            },
            Operation::Delete { entity: node },
            Operation::Add {
                entity: node,
                component: "Name".into(),
                value: Name("a".to_owned()).to_value(),
            },
            Operation::Remove {
                entity: node,
                component: "Transform".into(),
            },
            Operation::Reparent {
                entity: node,
                parent: None,
            },
            Operation::Duplicate {
                entity: node,
                copies: Vec::new(),
            },
            Operation::Create {
                entity: None,
                parent: Some(node),
                components: Vec::new(),
            },
        ];
        for operation in refused {
            let said = operation.to_string();
            let error = apply(world, operation).map(drop).expect_err(&said);
            assert_eq!(error.code(), "ecs.model_node", "{said}: {error:#}");
        }
        let mut given = Operation::Add {
            entity: root_id,
            component: "ModelNode".into(),
            value: ModelNode(0).to_value(),
        };
        let error = world.apply(&mut given).map(drop).expect_err("the world's");
        assert_eq!(error.code(), "reflect.read_only");

        // Another model, and back.
        let set = Operation::Set {
            entity: root_id,
            component: "ModelRoot".into(),
            path: "0".into(),
            value: Value::Model(other),
        };
        let (_, mut inverse) = apply(world, set.clone()).expect("set");
        assert_eq!(node_ids(world, root).len(), 1, "the other's one node");
        world.undo(&mut inverse).expect("undone");
        assert_eq!(
            node_ids(world, root),
            first,
            "its own again, under their IDs"
        );
        // Rolled back, as an edit refused after it rolls it back, the same.
        let (_, mut inverse) = apply(world, set).expect("set");
        world.roll_back(&mut inverse).expect("rolled back");
        assert_eq!(node_ids(world, root), first);

        // Taken away, and given back.
        let remove = Operation::Remove {
            entity: root_id,
            component: "ModelRoot".into(),
        };
        let (_, mut inverse) = apply(world, remove).expect("removed");
        assert_eq!(node_ids(world, root), [], "its nodes gone with it");
        world.undo(&mut inverse).expect("undone");
        assert_eq!(node_ids(world, root), first);
        world.undo(&mut inverse).expect("redone");
        assert_eq!(node_ids(world, root), []);
        world.undo(&mut inverse).expect("undone again");

        // Made with one, and unmade, its nodes with it.
        let create = Operation::Create {
            entity: None,
            parent: None,
            components: vec![("ModelRoot".into(), ModelRoot(model).to_value())],
        };
        let (create, mut inverse) = apply(world, create).expect("made");
        let made = world
            .entity(create.entity().expect("its ID"))
            .expect("alive");
        assert_eq!(node_ids(world, made).len(), 4);
        let entities = world.entities(None).map_or(0, |entities| entities.len());
        world.undo(&mut inverse).expect("unmade");
        let after = world.entities(None).map_or(0, |entities| entities.len());
        assert_eq!(after, entities - 5, "the root and its nodes");
        world.undo(&mut inverse).expect("made again");
        assert_eq!(node_ids(world, made).len(), 4);
        // Unmade whichever nodes its model has by then, as after its file changed.
        #[cfg(feature = "dev")]
        {
            let was = world.assets().nodes(model).expect("added").clone();
            let mut fewer = was.clone();
            fewer.parts.truncate(1);
            world.assets_mut().set_nodes(model, fewer);
            world.fit_model(model, &was);
            assert_eq!(node_ids(world, made).len(), 1);
            world
                .undo(&mut inverse)
                .expect("unmade, its one node with it");
            assert_eq!(world.entity(create.entity().expect("its ID")), None);
            world.assets_mut().set_nodes(model, was);
            world.undo(&mut inverse).expect("made again");
            world.fit_model(model, &world.assets().nodes(model).expect("added").clone());
        }

        // A copy, with nodes of its own, which its copies leave out, as what a scene names.
        let duplicate = Operation::Duplicate {
            entity: root_id,
            copies: Vec::new(),
        };
        let (duplicate, mut inverse) = apply(world, duplicate).expect("copied");
        let copy_id = duplicate.entity().expect("its copy's ID");
        let Operation::Duplicate { copies, .. } = &duplicate else {
            unreachable!("a duplicate");
        };
        assert_eq!(copies, &[copy_id], "the root alone");
        let copy = world.entity(copy_id).expect("alive");
        let mixed: Vec<_> = (0..4)
            .map(|i| node_id(copy_id, i).expect("an ID"))
            .collect();
        assert_eq!(node_ids(world, copy), mixed);
        world.undo(&mut inverse).expect("uncopied");
        assert_eq!(world.entity(copy_id), None);
        world.undo(&mut inverse).expect("copied again");
        assert_eq!(node_ids(world, copy), mixed);
        // Applied again with the IDs it was given, as a replay applies it, once its undo has let
        // them go.
        world.undo(&mut inverse).expect("uncopied");
        world.release(inverse);
        let mut again = duplicate.clone();
        world.apply(&mut again).expect("copied under its IDs");
        assert_eq!(
            node_ids(world, world.entity(copy_id).expect("alive")),
            mixed
        );
    }

    /// As its model changes, a root's nodes keep what code gave them, taking the model's draws and,
    /// where the model changed them, its transforms; nodes in other places are spawned again, under
    /// the same IDs. Their draws, left out as a restore of other assets leaves them, come back, to a
    /// node code took its draw away from too, the rest as it was, a node code despawned among it.
    #[cfg(feature = "dev")]
    #[test]
    fn nodes_fit_their_model_as_it_changes_keeping_what_code_gave_them() {
        let (mut cx, model, root) = spawned();
        let was = cx.world().assets().nodes(model).expect("added").clone();
        let world = cx.world_mut();
        let nodes = world.nodes_of(root);
        let ids = node_ids(world, root);
        let (turned, mirror) = (nodes[0].1, nodes[3].1);
        world
            .insert(turned, Name("turned".to_owned()))
            .expect("code may");
        let moved = Transform {
            translation: Vec3::new(0.0, 9.0, 0.0),
            ..Transform::IDENTITY
        };
        world.insert(mirror, moved).expect("code may");

        let mut now = was.clone();
        now.parts[0].transform = Transform::IDENTITY;
        now.draws.swap(0, 1);
        world.assets_mut().set_nodes(model, now.clone());
        world.fit_model(model, &was);
        assert_eq!(world.nodes_of(root), nodes, "the same nodes");
        assert_eq!(world.get::<Name>(turned), Some(&Name("turned".to_owned())));
        assert_eq!(world.get::<Transform>(turned), Some(&Transform::IDENTITY));
        assert_eq!(
            world.get::<Transform>(mirror),
            Some(&moved),
            "code's, the model's as it was"
        );
        let draw = |node: usize| world.get::<MeshDraw>(nodes[node].1).copied();
        assert_eq!(
            [draw(1), draw(2), draw(3)],
            [now.draws[0], now.draws[1], now.draws[1]].map(Some)
        );

        let hidden = world.take::<MeshDraw>(nodes[2].1).expect("alive");
        assert!(hidden.is_some(), "code may take a node's draw away");
        world.despawn(mirror).expect("code may");
        // As a restore of other assets leaves the draws out.
        world.take::<MeshDraw>(nodes[1].1).expect("alive");
        world.redraw_nodes();
        let draw = |node: usize| world.get::<MeshDraw>(nodes[node].1).copied();
        assert_eq!([draw(1), draw(2)], [now.draws[0], now.draws[1]].map(Some));
        assert_eq!(
            world.nodes_of(root),
            nodes[..3],
            "the node code took away stays away"
        );
        assert_eq!(world.get::<Transform>(turned), Some(&Transform::IDENTITY));

        // The model's own nodes in other places: spawned again, under the same IDs.
        let mut fewer = now.clone();
        fewer.parts.truncate(2);
        world.assets_mut().set_nodes(model, fewer);
        world.fit_model(model, &now);
        let respawned = world.nodes_of(root);
        assert_eq!(respawned.len(), 2);
        assert_eq!(world.get::<Name>(respawned[0].1), None, "spawned again");
        assert_eq!(node_ids(world, root), ids[..2]);
    }

    /// A scene file saved with its model's root moved moves the root alone: its nodes, which the
    /// file doesn't hold, are left as they are.
    #[cfg(feature = "dev")]
    #[test]
    fn a_scene_edit_leaves_a_models_nodes_alone() {
        let (cx, _, root) = spawned();
        let world = cx.world();
        let old = world.save_scene(&[root]).expect("saved");
        let new = old.replacen(
            "translation: (0.0, 0.0, 0.0)",
            "translation: (1.0, 0.0, 0.0)",
            1,
        );
        assert_ne!(new, old);
        let edit = world.scene_edit(Some(&old), &new).expect("an edit");
        let root_id = world.id(root).expect("alive");
        let crate::ecs::SceneEdit::Changes(operations) = edit else {
            panic!("changes, of what's there: {edit:?}");
        };
        let ids: Vec<_> = operations.iter().map(Operation::entity).collect();
        assert_eq!(ids, [Some(root_id)], "{operations:?}");
    }
}

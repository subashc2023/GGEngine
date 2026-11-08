//! Skinned models in the frame: each entity of a model's skinned part draws a mesh of its own, a
//! copy of its model's skinned mesh, so that each instance of the model poses apart; and each frame
//! gives the renderer its joints' matrices, from where the joints draw, which its skin pass moves
//! the copy's vertices by, as [`Draws::skin`] says.

use std::sync::Arc;

use gg_core::math::Mat4;
use gg_renderer::{Draws, Mesh, MeshDraw, Meshes, ModelMesh, PackedVertex};

use crate::ecs::node_id;
use crate::model::Made;
use crate::placed::Placed;
use crate::{Entity, ModelRoot, World};

/// The skinned entities' own meshes, by their slots, kept from frame to frame, so that a frame
/// allocates nothing once each has its own, and copies a model's mesh only for an entity that
/// hasn't one of it.
#[derive(Default)]
pub(crate) struct Skins {
    /// By slot: the entity that has a mesh of its own there, if one does.
    own: Vec<Option<Own>>,
    /// The slots `own` holds a mesh in.
    held: Vec<u32>,
    /// The entities given a mesh of their own this frame, which the scene sets again.
    fresh: Vec<Entity>,
    /// The frames found so far, the last of which each held mesh was found in.
    frame: u64,
}

/// An entity's own mesh: the model's mesh it's a copy of, and the frame it was last found in.
struct Own {
    entity: Entity,
    of: Arc<ModelMesh>,
    mesh: Mesh<PackedVertex>,
    found: u64,
}

impl Skins {
    /// Finds `world`'s skinned entities this frame, each a model's part whose node has a skin and
    /// draws a skinned mesh, as `made` has its model made, and gives each that hasn't one a mesh of
    /// its own, a copy of its model's, which `meshes` adds, and has `draws` skin it by its joints,
    /// where `placed` has them draw, each joint's matrix taking the mesh's space, as it was bound,
    /// to the entity's own, as the joint draws now: a joint gone, as code can despawn one, as where
    /// it was bound. A mesh of its own whose entity went, or isn't skinned now, or whose model's
    /// mesh changed, as it was read again, goes, which `meshes` removes. Called once a frame, after
    /// `placed` has drawn the world.
    pub(crate) fn find(
        &mut self,
        world: &World,
        placed: &Placed,
        made: &[Made],
        meshes: &mut Meshes,
        draws: &mut Draws,
    ) {
        self.frame += 1;
        self.fresh.clear();
        let assets = world.assets();
        for (root, &ModelRoot(model)) in world.query::<(Entity, &ModelRoot)>() {
            let (Some(nodes), Some(made), Some(id)) =
                (assets.nodes(model), made.get(model.index()), world.id(root))
            else {
                continue;
            };
            if nodes.skins.is_empty() {
                continue;
            }
            for (part, skinned) in nodes.parts.iter().enumerate() {
                let (Some(skin), Some(primitive)) = (skinned.skin, skinned.primitive) else {
                    continue;
                };
                let of = made.skinned.get(primitive).and_then(Option::as_ref);
                let node = node_id(id, part).and_then(|node| world.entity(node));
                let (Some(of), Some(entity)) = (of, node) else {
                    continue;
                };
                // A part whose draw code changed to another mesh draws that, unskinned.
                let model_mesh = made.meshes.get(primitive).map(|mesh| mesh.index());
                let drawn = world.get::<MeshDraw>(entity).map(|draw| draw.parts().0);
                let Some(place) = placed.drawn(entity).filter(|_| drawn == model_mesh) else {
                    continue;
                };
                let mesh = self.own(entity, of, meshes);
                let into_mesh = place.matrix.inverse();
                let skin = &nodes.skins[skin];
                let joints =
                    (skin.joints.iter().zip(&skin.inverse_binds)).map(|(&joint, &bind)| {
                        let joint = node_id(id, joint as usize).and_then(|node| world.entity(node));
                        match joint.and_then(|joint| placed.drawn(joint)) {
                            Some(joint) => into_mesh * joint.matrix * bind,
                            None => Mat4::IDENTITY,
                        }
                    });
                draws.skin(mesh, joints);
            }
        }
        let (own, frame) = (&mut self.own, self.frame);
        self.held.retain(|&slot| {
            let found = own[slot as usize]
                .as_ref()
                .is_some_and(|own| own.found == frame);
            if !found && let Some(gone) = own[slot as usize].take() {
                meshes.remove(gone.mesh);
            }
            found
        });
    }

    /// `entity`'s own mesh, a copy of `of`, which `meshes` adds if it hasn't one of it, marked
    /// found this frame.
    fn own(
        &mut self,
        entity: Entity,
        of: &Arc<ModelMesh>,
        meshes: &mut Meshes,
    ) -> Mesh<PackedVertex> {
        let slot = entity.slot();
        if self.own.len() <= slot {
            self.own.resize_with(slot + 1, || None);
        }
        match &mut self.own[slot] {
            Some(own) if own.entity == entity && Arc::ptr_eq(&own.of, of) => {
                own.found = self.frame;
                own.mesh
            }
            held => {
                if let Some(gone) = held.take() {
                    meshes.remove(gone.mesh);
                } else {
                    self.held.push(slot as u32);
                }
                let added = meshes.add_model("skinned", of);
                let mesh = added.expect("a model's mesh, checked as the renderer checks it");
                *held = Some(Own {
                    entity,
                    of: Arc::clone(of),
                    mesh,
                    found: self.frame,
                });
                self.fresh.push(entity);
                mesh
            }
        }
    }

    /// `draw`, `entity`'s, of the entity's own mesh in place of its model's, if it has one.
    pub(crate) fn draw_of(&self, entity: Entity, draw: &MeshDraw) -> MeshDraw {
        match self.own.get(entity.slot()) {
            Some(Some(own)) if own.entity == entity => draw.with_mesh(own.mesh),
            _ => *draw,
        }
    }

    /// The entities given a mesh of their own this frame.
    pub(crate) fn fresh(&self) -> &[Entity] {
        &self.fresh
    }

    /// How many meshes of their own it holds: for tests.
    #[cfg(test)]
    pub(crate) fn held(&self) -> usize {
        self.held.len()
    }
}

//! Skinned models in the frame: each entity of a model's skinned part draws a mesh of its own, a
//! copy of its model's skinned mesh, so that each instance of the model poses apart; and each frame
//! gives the renderer its joints' matrices, from where the joints draw, which its skin pass moves
//! the copy's vertices by, as [`Draws::skin`] says. The skin pass moves a copy into the frame's own
//! buffer, leaving the copy as it was, so a copy whose entity went is kept a while for the next
//! entity of the same model's mesh, which draws it at once, streaming nothing and adding no mesh.

use std::sync::Arc;

use gg_core::math::Mat4;
use gg_renderer::{Draws, Mesh, MeshDraw, Meshes, ModelMesh, PackedVertex};

use crate::ecs::node_id;
use crate::model::Made;
use crate::placed::Placed;
use crate::{Entity, ModelRoot, World};

/// The skinned entities' own meshes, by their slots, kept from frame to frame, so that a frame
/// allocates nothing once each has its own, and copies a model's mesh only for an entity that
/// hasn't one of it while no copy of it is spare.
#[derive(Default)]
pub(crate) struct Skins {
    /// By slot: the entity that has a mesh of its own there, if one does.
    own: Vec<Option<Own>>,
    /// The slots `own` holds a mesh in.
    held: Vec<u32>,
    /// The copies whose entities went, each with the frame it went in, each kept for
    /// [`SPARE_FRAMES`] while its model's mesh is the one it's a copy of.
    spare: Vec<(Copied, u64)>,
    /// The entities given a mesh of their own this frame, which the scene sets again.
    fresh: Vec<Entity>,
    /// The frames found so far, the last of which each held mesh was found in.
    frame: u64,
}

/// How many frames a copy is kept spare, some ten seconds at 60 a second: long enough that an
/// entity spawned as another of its model goes, as one respawned is, takes its copy, and short
/// enough that the copies of a crowd that went soon go, and their memory with them.
pub(crate) const SPARE_FRAMES: u64 = 600;

/// An entity's own mesh, and the frame it was last found in.
struct Own {
    entity: Entity,
    copy: Copied,
    found: u64,
}

/// A copy of a model's skinned mesh: the mesh it's a copy of, the model's and the primitive's
/// indices it was found at, and the copy.
struct Copied {
    of: Arc<ModelMesh>,
    from: (usize, usize),
    mesh: Mesh<PackedVertex>,
}

impl Copied {
    /// Whether it's still a copy of model `from`'s primitive's mesh, as `made` has it, which its
    /// model read again, or another model in its place, leaves it not.
    fn current(&self, made: &[Made]) -> bool {
        let (model, primitive) = self.from;
        let of = made.get(model).and_then(|made| made.skinned.get(primitive));
        of.and_then(Option::as_ref)
            .is_some_and(|of| Arc::ptr_eq(of, &self.of))
    }
}

impl Skins {
    /// Finds `world`'s skinned entities this frame, each a model's part whose node has a skin and
    /// draws a skinned mesh, as `made` has its model made, and gives each that hasn't one a mesh of
    /// its own, a spare copy of its model's, or else a copy that `meshes` adds, and has `draws`
    /// skin it by its joints, where `placed` has them draw, each joint's matrix taking the mesh's
    /// space, as it was bound, to the entity's own, as the joint draws now: a joint gone, as code
    /// can despawn one, as where it was bound. A mesh of its own whose entity went is spare for
    /// [`SPARE_FRAMES`]; one whose entity is alive and isn't skinned now, which the scene may still
    /// draw, goes, which `meshes` removes, as does a spare one whose time is up or whose model's
    /// mesh changed, as it was read again. Called once a frame, after `placed` has drawn the
    /// world.
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
                let mesh = self.own(entity, of, (model.index(), primitive), meshes);
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
        let (own, spare, frame) = (&mut self.own, &mut self.spare, self.frame);
        self.held.retain(|&slot| {
            let found = own[slot as usize]
                .as_ref()
                .is_some_and(|own| own.found == frame);
            if !found && let Some(gone) = own[slot as usize].take() {
                // An entity alive draws its copy until the scene sets it again, so no other may.
                if world.contains(gone.entity) {
                    meshes.remove(gone.copy.mesh);
                } else {
                    spare.push((gone.copy, frame));
                }
            }
            found
        });
        spare.retain(|(copy, went)| {
            let current = frame - went < SPARE_FRAMES && copy.current(made);
            if !current {
                meshes.remove(copy.mesh);
            }
            current
        });
    }

    /// `entity`'s own mesh, a copy of `of`, model `from`'s primitive's mesh, marked found this
    /// frame: a spare copy of it, or one `meshes` adds, if it hasn't one of it.
    fn own(
        &mut self,
        entity: Entity,
        of: &Arc<ModelMesh>,
        from: (usize, usize),
        meshes: &mut Meshes,
    ) -> Mesh<PackedVertex> {
        let slot = entity.slot();
        if self.own.len() <= slot {
            self.own.resize_with(slot + 1, || None);
        }
        match &mut self.own[slot] {
            Some(own) if own.entity == entity && Arc::ptr_eq(&own.copy.of, of) => {
                own.found = self.frame;
                own.copy.mesh
            }
            held => {
                // Another entity in the slot went, and its copy is spare; this one's own, of its
                // model's mesh before, goes.
                match held.take() {
                    Some(gone) if gone.entity != entity => self.spare.push((gone.copy, self.frame)),
                    Some(gone) => {
                        meshes.remove(gone.copy.mesh);
                    }
                    None => self.held.push(slot as u32),
                }
                let spare = (self.spare.iter()).position(|(copy, _)| Arc::ptr_eq(&copy.of, of));
                let copy = match spare {
                    Some(at) => self.spare.swap_remove(at).0,
                    None => {
                        let added = meshes.add_model("skinned", of);
                        Copied {
                            of: Arc::clone(of),
                            from,
                            mesh: added.expect("a model's mesh, checked as the renderer checks it"),
                        }
                    }
                };
                let mesh = copy.mesh;
                *held = Some(Own {
                    entity,
                    copy,
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
            Some(Some(own)) if own.entity == entity => draw.with_mesh(own.copy.mesh),
            _ => *draw,
        }
    }

    /// The entities given a mesh of their own this frame.
    pub(crate) fn fresh(&self) -> &[Entity] {
        &self.fresh
    }

    /// How many meshes of their own it holds, and how many spare: for tests.
    #[cfg(test)]
    pub(crate) fn held(&self) -> (usize, usize) {
        (self.held.len(), self.spare.len())
    }
}

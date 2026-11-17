//! Skinned models in the frame. Each entity of a model's skinned part draws its own copy of the
//! model's skinned mesh, so instances pose apart. Each frame gives the renderer the joints'
//! matrices, from where they draw, and its skin pass moves the copy's vertices by them
//! ([`Draws::skin`]). The pass writes into the frame's own buffer, leaving the copy as it was, so
//! a copy whose entity went is kept a while: the next entity of that mesh draws it at once,
//! streaming nothing and adding no mesh.

use std::sync::Arc;

use gg_core::math::Mat4;
use gg_renderer::{Draws, Mesh, MeshDraw, Meshes, ModelMesh, PackedVertex};

use crate::ecs::node_id;
use crate::model::Made;
use crate::placed::Placed;
use crate::{Entity, ModelRoot, World};

/// The skinned entities' own meshes, kept from frame to frame so a frame allocates nothing once
/// each has one. A model's mesh is copied only for an entity with none, when no copy is spare.
#[derive(Default)]
pub(crate) struct Skins {
    /// By slot: the entity with a mesh of its own there, if any.
    own: Vec<Option<Own>>,
    /// The slots `own` holds a mesh in.
    held: Vec<u32>,
    /// Copies whose entities went, with the frame they went in; kept for [`SPARE_FRAMES`] while
    /// they copy their model's current mesh.
    spare: Vec<(Copied, u64)>,
    /// The entities given a mesh of their own this frame, which the scene sets again.
    fresh: Vec<Entity>,
    /// Frames found so far.
    frame: u64,
}

/// How long a copy is kept spare: some ten seconds at 60 a second. Long enough that an entity
/// spawned as another goes (as a respawn is) takes its copy, short enough that a departed crowd's
/// copies and memory soon go.
pub(crate) const SPARE_FRAMES: u64 = 600;

/// An entity's own mesh, and the frame it was last found in.
struct Own {
    entity: Entity,
    copy: Copied,
    found: u64,
}

/// A copy of a model's skinned mesh, with its source and the model's and primitive's indices.
struct Copied {
    of: Arc<ModelMesh>,
    from: (usize, usize),
    mesh: Mesh<PackedVertex>,
}

impl Copied {
    /// Whether it still copies model `from`'s primitive's mesh as `made` has it: a model read
    /// again, or another in its place, leaves it not.
    fn current(&self, made: &[Made]) -> bool {
        let (model, primitive) = self.from;
        let of = made.get(model).and_then(|made| made.skinned.get(primitive));
        of.and_then(Option::as_ref)
            .is_some_and(|of| Arc::ptr_eq(of, &self.of))
    }
}

impl Skins {
    /// Finds `world`'s skinned entities (parts of a model, as `made` has it, whose node has a skin
    /// and draws a skinned mesh).
    ///
    /// - One with no mesh of its own gets a spare copy of its model's, else a copy `meshes` adds.
    /// - `draws` skins each by its joints where `placed` has them draw. A joint's matrix takes the
    ///   mesh's space as bound to the entity's as the joint draws now; a despawned joint stays as
    ///   bound.
    /// - A mesh whose entity went is spare for [`SPARE_FRAMES`].
    /// - A mesh whose entity is alive but no longer skinned (the scene may still draw it) is
    ///   removed from `meshes`, as is a spare one past its time or whose model's mesh changed.
    ///
    /// Called once a frame, after `placed` has drawn the world.
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
                let Some(place) = placed.drawn(world, entity).filter(|_| drawn == model_mesh)
                else {
                    continue;
                };
                let mesh = self.own(entity, of, (model.index(), primitive), meshes);
                let into_mesh = place.matrix.inverse();
                let skin = &nodes.skins[skin];
                let joints =
                    (skin.joints.iter().zip(&skin.inverse_binds)).map(|(&joint, &bind)| {
                        let joint = node_id(id, joint as usize).and_then(|node| world.entity(node));
                        // In `f64` up to the mesh's space, near the mesh, where `f32` holds.
                        match joint.and_then(|joint| placed.drawn(world, joint)) {
                            Some(joint) => {
                                Mat4::from((into_mesh * joint.matrix).as_affine3a()) * bind
                            }
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

    /// `entity`'s own copy of `of`, marked found this frame: a spare one or one `meshes` adds, if
    /// it has none.
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
                // Another entity's copy in the slot goes spare; this entity's own copy of its
                // model's earlier mesh is removed.
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

    /// `draw`, `entity`'s, with its own mesh in place of its model's, if it has one.
    pub(crate) fn draw_of(&self, entity: Entity, draw: &MeshDraw) -> MeshDraw {
        match self.own.get(entity.slot()) {
            Some(Some(own)) if own.entity == entity => draw.with_mesh(own.copy.mesh),
            _ => *draw,
        }
    }

    pub(crate) fn fresh(&self) -> &[Entity] {
        &self.fresh
    }

    #[cfg(test)]
    pub(crate) fn held(&self) -> (usize, usize) {
        (self.held.len(), self.spare.len())
    }
}

//! Prefabs' templates: what an instance spawns, as the prefab's text loads into a world of its own
//! with its own instances' entities, each field as the prefab has it; and how each entity's ID in
//! an instance mixes from the instance's root's.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gg_core::{Error, Result};

use super::read;
use crate::Prefab;
use crate::ecs::id::{self, EntityId};
use crate::ecs::table::{ComponentId, Row};
use crate::ecs::{Entity, Parent, World};

/// A prefab's entities as its text loads, which an instance takes: root first, then the rest in
/// first-named order, then those of the instances in it.
pub(crate) struct Template {
    pub(crate) parts: Vec<Part>,
}

/// An entity of a prefab, as its [`Template`] holds it.
pub(crate) struct Part {
    /// Its ID in the prefab: as the text gives it, or mixed from its instance's root's.
    pub(crate) id: EntityId,
    /// Its handle in the template's world, which references among the parts hold.
    pub(crate) handle: Entity,
    /// For an entity of an instance inside the prefab: the place of that instance's root among
    /// the parts, and its ID in that instance's prefab, which mixes from the root's. `None` for
    /// the prefab's own, whose ID in an instance mixes from the instance's root's and its own.
    pub(crate) spawner: Option<(usize, EntityId)>,
    /// Its parent's place among the parts; `None` for the root alone.
    pub(crate) parent: Option<usize>,
    /// Its components but `Parent` (`parent` gives it), each a one-row column; references are to
    /// the prefab's entities by their handles in its world.
    pub(crate) row: Row,
}

impl Template {
    /// A stand-in's: its instances are their roots alone.
    fn empty() -> Self {
        Self {
            parts: vec![Part {
                id: EntityId::new(1).expect("not zero"),
                handle: Entity::DEAD,
                spawner: None,
                parent: None,
                row: Vec::new(),
            }],
        }
    }

    /// The template of a prefab from its parsed entities `loaded`; `parent_type` is [`Parent`]'s.
    fn of(loaded: Vec<read::Loaded>, parent_type: ComponentId) -> Self {
        let root = (loaded.iter())
            .position(|entity| entity.parent.is_none())
            .expect("a prefab has a root");
        let mut order: Vec<usize> = (0..loaded.len()).collect();
        order.swap(0, root);
        let mut at = vec![0; loaded.len()];
        for (place, &was) in order.iter().enumerate() {
            at[was] = place;
        }
        let mut loaded: Vec<Option<read::Loaded>> = loaded.into_iter().map(Some).collect();
        let parts = order.iter().map(|&was| {
            let entity = loaded[was].take().expect("each once");
            let parent = entity.parent.map(|(up, _)| at[up]);
            let mut row = entity.row;
            row.retain(|&(component, _)| component != parent_type);
            Part {
                id: entity.id,
                handle: entity.handle,
                spawner: entity.spawned.map(|(by, local)| (at[by], local)),
                parent,
                row,
            }
        });
        Self {
            parts: parts.collect(),
        }
    }

    /// Its parts' places, the root's first and each part's after its parent's. The parts' own order
    /// is by first mention, so a child may come before its parent.
    #[cfg(feature = "dev")]
    pub(crate) fn downward(&self) -> Vec<usize> {
        let mut order = Vec::with_capacity(self.parts.len());
        let mut placed = vec![false; self.parts.len()];
        for part in 0..self.parts.len() {
            // It and its unplaced ancestors, pushed upward, then reversed to run down.
            let from = order.len();
            let mut at = Some(part);
            while let Some(of) = at.filter(|&of| !placed[of]) {
                placed[of] = true;
                order.push(of);
                at = self.parts[of].parent;
            }
            order[from..].reverse();
        }
        order
    }

    /// The IDs of its parts in the instance rooted at `root`: `root`, then each other part mixed
    /// from its instance's root's ([`id::linked`]).
    ///
    /// Fails with `ecs.bad_id` for the one ID a part mixes to zero under `root`.
    pub(crate) fn ids(&self, root: EntityId) -> Result<Vec<EntityId>> {
        let mut ids = vec![None; self.parts.len()];
        ids[0] = Some(root);
        for part in 1..self.parts.len() {
            self.id_of(part, &mut ids)?;
        }
        Ok(ids.into_iter().map(|id| id.expect("each mixed")).collect())
    }

    /// The ID of part `part`; `ids` holds those mixed so far.
    fn id_of(&self, part: usize, ids: &mut [Option<EntityId>]) -> Result<EntityId> {
        if let Some(id) = ids[part] {
            return Ok(id);
        }
        let of = &self.parts[part];
        let (from, local) = match of.spawner {
            None => (ids[0].expect("the root's"), of.id),
            Some((by, local)) => (self.id_of(by, ids)?, local),
        };
        let Some(id) = id::linked(from, local) else {
            return Err(Error::new(
                "ecs.bad_id",
                format!(
                    "entity {local} of its prefab mixes to no ID under entity {from}: give it \
                     another ID in the prefab"
                ),
            ));
        };
        ids[part] = Some(id);
        Ok(id)
    }
}

/// Makes prefabs' templates for a load, save or operation: each once, from its text; none for a
/// prefab that links itself.
#[derive(Default)]
pub(crate) struct Expanding {
    /// The prefabs whose templates are being made, each linking the next.
    making: Vec<Prefab>,
    made: HashMap<Prefab, Rc<Template>>,
    /// The text to make each prefab here from, in place of the assets', as a reload's.
    texts: HashMap<Prefab, Arc<str>>,
}

impl Expanding {
    /// Makes `prefab`'s template from `text`, and the rest from the assets.
    #[cfg(feature = "dev")]
    pub(crate) fn with_text(prefab: Prefab, text: Arc<str>) -> Self {
        Self {
            texts: HashMap::from([(prefab, text)]),
            ..Self::default()
        }
    }

    /// The template of a prefab of `text` that isn't among the assets, for a save as a prefab to
    /// check what it writes. None names it, so it links none of those it makes.
    ///
    /// Fails if the text fails to load ([`World::load_scene`]).
    #[cfg(feature = "dev")]
    pub(crate) fn text_template(&mut self, world: &World, text: &str) -> Result<Rc<Template>> {
        let scratch = world.scratch();
        let loaded = read::parse(&scratch, text, true, self)?;
        let parent_type = world.component::<Parent>();
        let parent_type = parent_type.expect("every world has registered `Parent`");
        Ok(Rc::new(Template::of(loaded, parent_type)))
    }

    /// `prefab`'s template: its text loaded into a world of `world`'s types and assets, made on
    /// first request. A prefab without text gets a stand-in's root alone.
    ///
    /// Fails with `scene.prefab_cycle` for a prefab that links itself, directly or through those
    /// it links; and if the text fails to load ([`World::load_scene`]), the message naming the
    /// prefab's path.
    pub(crate) fn template(&mut self, world: &World, prefab: Prefab) -> Result<Rc<Template>> {
        if let Some(made) = self.made.get(&prefab) {
            return Ok(Rc::clone(made));
        }
        let assets = world.assets();
        let path = |prefab: Prefab| assets.prefab_path(prefab).to_owned();
        if let Some(at) = self.making.iter().position(|&making| making == prefab) {
            let through = self.making[at + 1..]
                .iter()
                .map(|&by| format!("`{}`", path(by)));
            let through: Vec<String> = through.collect();
            let message = match through.len() {
                0 => format!("prefab `{}` links itself", path(prefab)),
                _ => format!(
                    "prefab `{}` links itself, through {}",
                    path(prefab),
                    crate::reflect::list(through)
                ),
            };
            return Err(Error::new("scene.prefab_cycle", message));
        }
        let text = (self.texts.get(&prefab)).or_else(|| assets.prefab_text(prefab));
        let template = match text.cloned() {
            None => Template::empty(),
            Some(text) => {
                let scratch = world.scratch();
                self.making.push(prefab);
                let loaded = read::parse(&scratch, &text, true, self);
                self.making.pop();
                let loaded = loaded.map_err(|error| {
                    let message = format!("in prefab `{}`, {error}", path(prefab));
                    error.with_message(message)
                })?;
                let parent_type = world.component::<Parent>();
                Template::of(
                    loaded,
                    parent_type.expect("every world has registered `Parent`"),
                )
            }
        };
        let template = Rc::new(template);
        self.made.insert(prefab, Rc::clone(&template));
        Ok(template)
    }
}

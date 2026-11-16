//! Clients' edits: the operations they ask for, read by the world's types, and what each answers.

use std::cell::RefCell;
use std::path::PathBuf;

use gg_api::{Applied, Failure, Patched, SavedPrefab};
use gg_core::{Error, Result};
use serde_json::Value;

use super::connection::Reply;
use super::{Api, failure};
use crate::api::json;
use crate::assets::Asset;
use crate::edits::Done;
use crate::layer::{Offered, Stack};
use crate::reflect::{self, Ident, Type};
use crate::{Context, Entity, EntityId, Font, Model, Operation, Prefab, Texture, World};

/// What an edit or a patch offered to a lockstep session was asked as, which its answer reads as.
pub(super) enum Offer {
    Apply,
    Step {
        author: String,
    },
    Patch,
    /// A save as a prefab's link, of the prefab whose file's path from the asset root and name
    /// these are.
    Prefab {
        path: String,
        name: String,
    },
}

impl Api {
    /// Answers `reply`, asked as `offer`, with what an edit did, if it applied now, the next tick
    /// to run being `ticks`; or keeps it until the tick it applies before in a lockstep session.
    pub(super) fn answer_offered(
        &mut self,
        reply: Reply,
        offered: Offered,
        offer: Offer,
        ticks: u64,
    ) {
        match offered {
            Offered::Now(done) => answer_done(reply, &offer, ticks, done),
            Offered::Later(ticket) => self.offered.push((reply, ticket, offer)),
        }
    }
}

/// Answers `reply`, asked as `offer`, with what its edit or patch did, `done`, before tick `ticks`.
pub(super) fn answer_done(reply: Reply, offer: &Offer, ticks: u64, done: Result<Done>) {
    match (offer, done) {
        (_, Err(error)) => reply.send(Err(failure(&error))),
        (Offer::Apply, Ok(Done::Applied(made))) => reply.result(&Applied {
            ticks,
            entities: made.iter().map(EntityId::to_string).collect(),
        }),
        (Offer::Step { author }, Ok(Done::Stepped(stepped))) => {
            let edit = stepped.map(|operations| gg_api::Edit {
                author: author.clone(),
                operations: words(&operations),
            });
            reply.result(&edit);
        }
        (Offer::Patch, Ok(_)) => reply.result(&Patched { ticks }),
        (Offer::Prefab { path, name }, Ok(Done::Applied(_))) => reply.result(&SavedPrefab {
            path: path.clone(),
            name: name.clone(),
            ticks,
        }),
        (Offer::Apply | Offer::Prefab { .. }, Ok(Done::Stepped(_))) => {
            unreachable!("an apply applies")
        }
        (Offer::Step { .. }, Ok(Done::Applied(_))) => unreachable!("an undo or a redo steps"),
    }
}

/// Saves the entity of ID `id` and its descendants as a prefab at `path`, as `save_prefab` asks,
/// as [`Context::save_prefab`] does, for its link to be offered.
///
/// Fails with `api.no_scene` in a game that runs from its package, which reads no file under its
/// root, and in a networked one, whose files would part from the session's others', as `scene`'s
/// `save` does; while a copy of the world as edited plays, whose entities would be saved and
/// linked, though `stop` takes the copy away; and as `Context::save_prefab` does.
pub(super) fn save_prefab(
    cx: &mut Context,
    stack: &Stack,
    id: EntityId,
    path: Option<PathBuf>,
) -> Result<crate::prefab::Saved> {
    let why = if cx.reads_package() {
        Some("it runs from its package, which holds no files under its root to write")
    } else if stack.is_peer() {
        Some("the run is networked, and its prefabs would part from the session's others'")
    } else if cx.edited.is_some() {
        Some(
            "a copy of the world as edited plays, whose entities `stop` takes away, the link \
             with them: stop it first",
        )
    } else {
        None
    };
    if let Some(why) = why {
        let message = format!("the game can't save a prefab: {why}");
        return Err(Error::new("api.no_scene", message));
    }
    cx.save_prefab(id, path.as_deref())
}

/// The textures, models, fonts and prefabs that the values of the operations `given` asks for name
/// that
/// none loaded has, as [`operations`] reads them in `world`: which a client's edit names, to load
/// before it's read.
pub(super) fn assets_named(world: &World, given: &[gg_api::Operation]) -> Vec<Asset<String>> {
    let gathering = Gathering {
        world,
        missing: RefCell::default(),
    };
    let _ = operations(world, &gathering, given.to_vec());
    gathering.missing.into_inner()
}

/// What reads values as a world does, `world`, but for a texture, a model, a font or a prefab that
/// none loaded has, which it keeps the name of, reading on as though it were the engine's white
/// texture, or the first model, font or prefab.
struct Gathering<'w> {
    world: &'w World,
    missing: RefCell<Vec<Asset<String>>>,
}

impl Gathering<'_> {
    /// Keeps `asset`, which none loaded has, unless it's kept.
    fn missed(&self, asset: Asset<&str>) {
        let mut missing = self.missing.borrow_mut();
        if !missing.iter().any(|missed| missed.as_deref() == asset) {
            missing.push(asset.owned());
        }
    }
}

impl json::Names for Gathering<'_> {
    fn types(&self) -> &reflect::Types {
        self.world.types()
    }

    fn id(&self, entity: Entity) -> Option<EntityId> {
        self.world.id(entity)
    }

    fn name(&self, texture: Texture) -> Option<&str> {
        self.world.assets().name(texture)
    }

    fn entity(&self, id: EntityId) -> Result<reflect::Value> {
        json::Names::entity(self.world, id)
    }

    fn texture(&self, name: &str) -> Result<reflect::Value> {
        if let Some(texture) = self.world.assets().texture(name) {
            return Ok(reflect::Value::Texture(texture));
        }
        self.missed(Asset::Texture(name));
        Ok(reflect::Value::Texture(Texture::WHITE))
    }

    fn model_name(&self, model: Model) -> Option<&str> {
        Some(self.world.assets().model_name(model))
    }

    fn model(&self, name: &str) -> Result<reflect::Value> {
        if let Some(model) = self.world.assets().model(name) {
            return Ok(reflect::Value::Model(model));
        }
        self.missed(Asset::Model(name));
        Ok(reflect::Value::Model(Model::new(0)))
    }

    fn font_name(&self, font: Font) -> Option<&str> {
        Some(self.world.assets().font_name(font))
    }

    fn font(&self, name: &str) -> Result<reflect::Value> {
        if let Some(font) = self.world.assets().font(name) {
            return Ok(reflect::Value::Font(font));
        }
        self.missed(Asset::Font(name));
        Ok(reflect::Value::Font(Font::new(0)))
    }

    fn prefab_name(&self, prefab: Prefab) -> Option<&str> {
        Some(self.world.assets().prefab_name(prefab))
    }

    fn prefab(&self, name: &str) -> Result<reflect::Value> {
        if let Some(prefab) = self.world.assets().prefab(name) {
            return Ok(reflect::Value::Prefab(prefab));
        }
        self.missed(Asset::Prefab(name));
        Ok(reflect::Value::Prefab(Prefab::new(0)))
    }
}

/// The engine's operations `given` asks for, their values read by the types of `world`'s components
/// and the fields they go in, as [`json::read`] reads them with `names`.
///
/// Fails, naming the operation, with `ecs.bad_id` for an ID that isn't one, `reflect.unknown_type`
/// for a component type that isn't registered, as a path to a field it hasn't fails, and as
/// `json::read` fails for a value.
pub(super) fn operations(
    world: &World,
    names: &dyn json::Names,
    given: Vec<gg_api::Operation>,
) -> Result<Vec<Operation>, Failure> {
    let count = given.len();
    let operations = given.into_iter().enumerate().map(|(i, operation)| {
        let read = match operation {
            gg_api::Operation::Reparent {
                id,
                parent,
                in_place: true,
            } => reparent_in_place(world, &id, parent.as_deref()),
            operation => operation_of(world, names, operation).map(|operation| vec![operation]),
        };
        read.map_err(|error| {
            let failure = failure(&error);
            if count == 1 {
                return failure;
            }
            Failure::new(
                failure.code,
                format!("operation {}: {}", i + 1, failure.message),
            )
        })
    });
    let operations: Vec<Vec<Operation>> = operations.collect::<Result<_, _>>()?;
    Ok(operations.into_iter().flatten().collect())
}

/// The operations a `reparent` `in_place` of entity `id` under `parent` asks for, as
/// [`World::reparent_in_place`] finds them in `world` as the request comes, which an edit applies
/// at once, outside a lockstep session.
fn reparent_in_place(world: &World, id: &str, parent: Option<&str>) -> Result<Vec<Operation>> {
    let parent = parent.map(str::parse).transpose()?;
    Ok(world.reparent_in_place(id.parse()?, parent))
}

/// The engine's operation `given` asks for, as [`operations`] reads it.
fn operation_of(
    world: &World,
    names: &dyn json::Names,
    given: gg_api::Operation,
) -> Result<Operation> {
    use gg_api::Operation as Given;

    let id = |text: &str| text.parse::<EntityId>();
    let maybe = |text: Option<String>| text.as_deref().map(id).transpose();
    // The type of `component`'s field at `path`, as its description gives it.
    let field = |component: &str, path: &str| -> Result<Type> {
        let types = world.types();
        let index = types.find(component).ok_or_else(|| {
            let names = world.component_types().map(|name| format!("`{name}`"));
            let message = format!(
                "no component type is named `{component}`; the world's are {}",
                reflect::list(names)
            );
            Error::new("reflect.unknown_type", message)
        })?;
        let resolved = reflect::resolve(types, &Type::Named(index), path)?;
        Ok(resolved.ty)
    };
    let value = |component: &str, path: &str, json: &Value| {
        let ty = field(component, path)?;
        json::read(json, &ty, names)
            .map_err(|error| reflect::within(error, &reflect::join(component, path)))
    };
    Ok(match given {
        Given::Create {
            id: made,
            parent,
            components,
        } => Operation::Create {
            entity: maybe(made)?,
            parent: maybe(parent)?,
            components: components
                .iter()
                .map(|(name, json)| Ok((Ident::from(name.clone()), value(name, "", json)?)))
                .collect::<Result<_>>()?,
        },
        Given::Duplicate { id: of, copies } => Operation::Duplicate {
            entity: id(&of)?,
            copies: copies.iter().map(|copy| id(copy)).collect::<Result<_>>()?,
        },
        Given::Delete { id: of } => Operation::Delete { entity: id(&of)? },
        Given::Add {
            id: of,
            component,
            value: given,
        } => Operation::Add {
            entity: id(&of)?,
            value: match given {
                Some(json) => value(&component, "", &json)?,
                None => world.default_of(&component)?,
            },
            component: component.into(),
        },
        Given::Remove { id: of, component } => Operation::Remove {
            entity: id(&of)?,
            component: component.into(),
        },
        Given::Set {
            id: of,
            component,
            path,
            value: json,
        } => Operation::Set {
            entity: id(&of)?,
            value: value(&component, &path, &json)?,
            component: component.into(),
            path,
        },
        Given::Reparent { id: of, parent, .. } => Operation::Reparent {
            entity: id(&of)?,
            parent: maybe(parent)?,
        },
        Given::Unlink { id: of } => Operation::Unlink { entity: id(&of)? },
        Given::Revert {
            id: of,
            component,
            path,
        } => {
            // Refused as a set of the path is, for a type or a field there's none of.
            field(&component, &path)?;
            Operation::Revert {
                entity: id(&of)?,
                component: component.into(),
                path,
            }
        }
    })
}

/// `operations` in words, as an edit is reported.
pub(super) fn words(operations: &[Operation]) -> Vec<String> {
    operations.iter().map(ToString::to_string).collect()
}

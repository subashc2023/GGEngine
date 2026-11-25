//! What the reads answer of the world and of the asset root.

use gg_api::asset::AssetKind;
use gg_api::{AssetList, EntityList, Failure, Listed, ListedAsset, ListedOrphan};
use gg_core::Error;
use gg_core::math::DMat4;
use gg_renderer::Meshes;

use super::connection::Reply;
use super::failure;
use crate::api::json;
use crate::bounds;
use crate::ecs::Pose;
use crate::{Context, Entity, EntityId, Name, PrefabRoot, World};

/// An `assets` waiting for a look at the asset root (numbered by `Context::ask_assets`), and its
/// page.
pub(super) struct Listing {
    pub(super) reply: Reply,
    pub(super) look: u64,
    pub(super) kind: Option<AssetKind>,
    pub(super) skip: usize,
    pub(super) take: usize,
}

/// An entity as a request names it: by its ID, or by its [`Name`]'s text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Subject {
    Id(EntityId),
    Name(String),
}

impl Subject {
    /// Text that parses as an ID is one; any other is a name.
    pub(super) fn of(text: &str) -> Self {
        text.parse()
            .map_or_else(|_| Self::Name(text.to_owned()), Self::Id)
    }

    /// The ID, or the ID of the one entity with the name.
    ///
    /// Fails with `ecs.no_entity` for a name none has, and `ecs.ambiguous_name`, listing their
    /// IDs, for a name several have.
    pub(super) fn resolve(&self, world: &World) -> Result<EntityId, Error> {
        let name = match self {
            Self::Id(id) => return Ok(*id),
            Self::Name(name) => name,
        };
        let named = world.entities(Some("Name"))?;
        let named = named.filter(|&entity| world.get::<Name>(entity).is_some_and(|n| n.0 == *name));
        let ids: Vec<EntityId> = named.filter_map(|entity| world.id(entity)).collect();
        match ids[..] {
            [] => {
                let message = format!("no living entity is named `{name}`, and it isn't an ID");
                Err(Error::new("ecs.no_entity", message))
            }
            [id] => Ok(id),
            _ => {
                let listed: Vec<String> = ids.iter().map(ToString::to_string).collect();
                let message = format!(
                    "{} entities are named `{name}`, IDs {}: give one's ID",
                    ids.len(),
                    listed.join(", ")
                );
                Err(Error::new("ecs.ambiguous_name", message))
            }
        }
    }
}

/// The page of `world`'s entities `entities` asks for.
pub(super) fn list(
    world: &World,
    with: Option<&str>,
    skip: usize,
    take: usize,
) -> Result<String, Failure> {
    let mut listing = world.entities(with).map_err(|error| failure(&error))?;
    let total = listing.len();
    let page = listing.by_ref().skip(skip).take(take);
    let entities = page.map(|entity| listed(world, entity)).collect();
    let list = EntityList {
        total,
        entities,
        types: world.types_revision(),
    };
    Ok(serde_json::to_string(&list).expect("a list serializes"))
}

/// The page of the assets under `cx`'s root that `assets` asks for, and the orphaned ID files, as
/// the asset database holds them.
///
/// Fails with `engine.no_root` for a game with no asset root, and with `asset.unlisted`, naming
/// the root, if it couldn't be listed.
pub(super) fn assets(
    cx: &Context,
    kind: Option<AssetKind>,
    skip: usize,
    take: usize,
) -> Result<String, Failure> {
    let Some(root) = &cx.root else {
        let message = "the game names no asset root, which App::root or App::project gives it";
        return Err(Failure::new("engine.no_root", message));
    };
    let root = root.display().to_string();
    let catalog = cx.catalog();
    if let Some(error) = catalog.unlisted() {
        let message = format!("can't scan the asset root, {root}: {error}");
        return Err(Failure::new("asset.unlisted", message));
    }
    let of_kind = |of: AssetKind| kind.is_none_or(|kind| of == kind);
    let total = catalog
        .listed()
        .filter(|(_, asset)| of_kind(asset.kind))
        .count();
    let page = (catalog.listed())
        .filter(|(_, asset)| of_kind(asset.kind))
        .skip(skip)
        .take(take);
    let assets = page.map(|(path, asset)| ListedAsset {
        id: asset.id.map(|id| id.to_string()),
        path: path.to_owned(),
        kind: asset.kind,
        loaded: cx.has_added(path),
        problem: (asset.problem.as_ref())
            .map(|problem| Failure::new(problem.code, &*problem.message)),
    });
    let orphans = (catalog.orphans().iter())
        .filter(|&&(_, kind, _)| of_kind(kind))
        .map(|(path, kind, id)| ListedOrphan {
            path: path.to_string(),
            kind: *kind,
            id: id.map(|id| id.to_string()),
        });
    let list = AssetList {
        total,
        assets: assets.collect(),
        orphans: orphans.collect(),
    };
    Ok(serde_json::to_string(&list).expect("a list serializes"))
}

/// Living `entity` as `entities` lists it.
pub(super) fn listed(world: &World, entity: Entity) -> Listed {
    let id = |entity| world.id(entity).map(|id| id.to_string());
    let parent = world.parent(entity).ok().flatten();
    let components = world
        .components_of(entity)
        .expect("a listed entity is alive");
    let children = world.children(entity).expect("a listed entity is alive");
    Listed {
        id: id(entity).expect("a listed entity is alive"),
        name: world.name(entity).map(str::to_owned),
        parent: parent.and_then(id),
        children: children
            .map(|child| id(child).expect("a child is alive"))
            .collect(),
        components: components.map(str::to_owned).collect(),
    }
}

/// Entity `id` as `entity` answers it, its `bounds` from the extents `meshes` keeps.
pub(super) fn entity(world: &World, meshes: &Meshes, id: EntityId) -> Result<String, Failure> {
    let entity = found(world, id)?;
    let about = |error: Error| failure(&error);
    let mut out = String::from("{\"id\":");
    json::string(&mut out, &id.to_string());
    out.push_str(",\"name\":");
    match world.name(entity) {
        Some(name) => json::string(&mut out, name),
        None => out.push_str("null"),
    }
    out.push_str(",\"parent\":");
    match world
        .parent(entity)
        .map_err(about)?
        .and_then(|up| world.id(up))
    {
        Some(up) => json::string(&mut out, &up.to_string()),
        None => out.push_str("null"),
    }
    out.push_str(",\"children\":[");
    for (i, child) in world.children(entity).map_err(about)?.enumerate() {
        if i > 0 {
            out.push(',');
        }
        let child = world.id(child).expect("a child is alive");
        json::string(&mut out, &child.to_string());
    }
    out.push_str("],\"components\":{");
    for (i, name) in world.components_of(entity).map_err(about)?.enumerate() {
        if i > 0 {
            out.push(',');
        }
        json::string(&mut out, name);
        out.push(':');
        let value = world.get_field(entity, name, "").map_err(about)?;
        json::write(&mut out, &value, world);
    }
    // The space its transform is in: its parent's pose.
    let space = match world.parent(entity).map_err(about)? {
        Some(parent) => world.pose(parent).map_err(about)?,
        None => Pose::IDENTITY,
    };
    out.push_str("},\"space\":{\"matrix\":");
    json::float64s(&mut out, &DMat4::from(space.matrix).to_cols_array());
    out.push_str(",\"rotation\":");
    json::floats(&mut out, &space.rotation.to_array());
    out.push('}');
    if let Some((center, radius)) = bounds::of(world, meshes, entity) {
        out.push_str(",\"bounds\":{\"center\":");
        json::float64s(&mut out, &center.to_array());
        out.push_str(",\"radius\":");
        json::float32(&mut out, radius as f32);
        out.push('}');
    }
    // An instance whose prefab no longer loads gives none; its load warned.
    if let Some(instanced) = instanced(world, entity) {
        out.push_str(",\"instance\":");
        out.push_str(&serde_json::to_string(&instanced).expect("an instance serializes"));
    }
    out.push('}');
    Ok(out)
}

/// What of the living `entity` differs from its prefab ([`World::overrides`]), as `entity` answers
/// it; `None` for a non-instance, or one whose prefab no longer loads.
pub(crate) fn instanced(world: &World, entity: Entity) -> Option<gg_api::Instanced> {
    let overrides = world.overrides(entity).ok()??;
    let assets = world.assets();
    let link = world.get::<PrefabRoot>(entity);
    Some(gg_api::Instanced {
        root: overrides.root.to_string(),
        prefab: assets.prefab_path(overrides.prefab).to_owned(),
        link: link.map(|&PrefabRoot(prefab)| assets.prefab_path(prefab).to_owned()),
        overrides: overrides.fields.into_iter().collect(),
        added: overrides.added,
    })
}

/// The field at `path` of `component` of entity `id`, as `get` answers it.
pub(super) fn field(
    world: &World,
    id: EntityId,
    component: &str,
    path: &str,
) -> Result<String, Failure> {
    let entity = found(world, id)?;
    let value = world
        .get_field(entity, component, path)
        .map_err(|error| failure(&error))?;
    let mut out = String::new();
    json::write(&mut out, &value, world);
    Ok(out)
}

/// The living entity of ID `id` in `world`.
///
/// Fails with `ecs.no_entity` if none has it.
fn found(world: &World, id: EntityId) -> Result<Entity, Failure> {
    world.entity(id).ok_or_else(|| {
        let message = format!("no living entity has ID {id}");
        Failure::new("ecs.no_entity", message)
    })
}

//! What scripts read of physics' contacts, as [`World::contacts`](crate::World::contacts) and
//! [`World::contact_events`](crate::World::contact_events) have them, 2D's then 3D's, each in their
//! order, by their entities' IDs, so that every peer and replay reads them alike:
//!
//! - `gg.contacts(entity)`, an array of a table for each entity whose colliders touch `entity`'s
//!   as the last step of physics left them, `{ other = entity, sensor = bool }`, `sensor` true
//!   where either collider is a sensor, which overlaps rather than pushes;
//! - `gg.contact_events(entity)`, an array of a table for each contact of `entity`'s that started
//!   or stopped at that step, `{ other = entity, sensor = bool, started = bool }`, which a script's
//!   tick reads at the tick after, as scripts run before physics. An entity gone since is in the
//!   events of its contacts stopping.
//!
//! A collider's `sensor`, `membership` and `filter` are its component's fields, which scripts reach
//! as they reach any, as `entity[gg.component("BoxCollider2D").sensor] = true`. A mod's scripts read
//! contacts only if it may read the whole world, as they tell where other entities are.

use mlua::{LightUserData, Lua, Table, Value as Lua_};

use super::{denied, entity_of, handle, shared};
use crate::grant::Capability;
use crate::{Contact, Entity, World};

/// Adds `gg.contacts` and `gg.contact_events` to `gg`.
pub(super) fn install(lua: &Lua, gg: &Table) -> mlua::Result<()> {
    gg.raw_set("contacts", lua.create_function(contacts)?)?;
    gg.raw_set("contact_events", lua.create_function(contact_events)?)?;
    Ok(())
}

/// `gg.contacts(entity)`: a table for each contact of `entity`'s that the last step left.
fn contacts(lua: &Lua, entity: LightUserData) -> mlua::Result<Table> {
    listed(lua, entity, |world, each| {
        for contact in world.contacts() {
            each(contact, None)?;
        }
        Ok(())
    })
}

/// `gg.contact_events(entity)`: a table for each contact of `entity`'s that started or stopped at
/// the last step.
fn contact_events(lua: &Lua, entity: LightUserData) -> mlua::Result<Table> {
    listed(lua, entity, |world, each| {
        for event in world.contact_events() {
            each(event.contact, Some(event.started))?;
        }
        Ok(())
    })
}

/// What gives each contact, and whether it started if it's an event's, to the callback it's given.
type Each<'a> = dyn FnMut(Contact, Option<bool>) -> mlua::Result<()> + 'a;

/// An array of a table for each contact of `entity`'s that `list` gives of the world scripts
/// reach, in its order: for a mod's scripts, only if the mod may read the whole world.
fn listed(
    lua: &Lua,
    entity: LightUserData,
    list: impl FnOnce(&World, &mut Each<'_>) -> mlua::Result<()>,
) -> mlua::Result<Table> {
    shared(|shared| {
        if let Some(held) = &shared.held
            && !held.grant.has(&Capability::Read(None))
        {
            let what = "a contact, which reads which entities' colliders touch,";
            return Err(denied("read", what));
        }
        let entity = entity_of(entity)?;
        let world = shared.world.borrow();
        let found = lua.create_table()?;
        let mut count = 0;
        list(&world, &mut |contact, started| {
            let Some(other) = contact.other(entity) else {
                return Ok(());
            };
            found.raw_set(count + 1, table(lua, other, contact.sensor, started)?)?;
            count += 1;
            Ok(())
        })?;
        Ok(found)
    })
}

/// A contact's table, `{ other = entity, sensor = bool }`, and `started` if it's an event's.
fn table(lua: &Lua, other: Entity, sensor: bool, started: Option<bool>) -> mlua::Result<Table> {
    let table = lua.create_table_with_capacity(0, 3)?;
    table.raw_set("other", Lua_::LightUserData(handle(other.to_bits())))?;
    table.raw_set("sensor", sensor)?;
    if let Some(started) = started {
        table.raw_set("started", started)?;
    }
    Ok(table)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use gg_core::math::{DVec3, Vec3};

    use super::super::Scripts;
    use crate::dirs::tests::Scratch;
    use crate::reflect::Value;
    use crate::{BodyType, BoxCollider2D, Context, RigidBody2D, Transform, step_physics};

    /// Makes its entity's box a sensor, and counts what enters and leaves it and what's in it, and
    /// keeps the last to enter.
    const ZONE: &str = r#"
local Box = gg.component("BoxCollider2D")
local Zone = gg.declare("Zone", { entered = 0, left = 0, inside = 0, last = gg.entity, solid = false })

function Zone.tick(entity, dt)
    entity[Box.sensor] = true
    for _, event in gg.contact_events(entity) do
        if event.started then
            entity[Zone.entered] = entity[Zone.entered] + 1
            entity[Zone.last] = event.other
        else
            entity[Zone.left] = entity[Zone.left] + 1
        end
        entity[Zone.solid] = entity[Zone.solid] or not event.sensor
    end
    entity[Zone.inside] = #gg.contacts(entity)
end

return Zone
"#;

    /// A script makes its entity's collider a sensor through the collider's field, and reads its
    /// contacts and their events at the ticks after the steps that made them: a box falling through
    /// it enters it, is inside it awhile, and leaves it, an overlap rather than a contact, and lands
    /// on the floor below.
    #[test]
    fn a_script_reads_what_enters_and_leaves_its_sensor() {
        let dir = Scratch::new("luau-contacts");
        fs::create_dir_all(dir.0.join("scripts")).expect("a directory");
        let mut cx = Context::seeded(1).root(&dir.0);
        let path = dir.0.join("scripts/zone.luau");
        fs::write(&path, ZONE).expect("written");
        let mut host = Scripts::new().expect("Luau starts");
        host.load(&mut cx, &path).expect("the script loads");
        let world = cx.world_mut();
        let fixed = RigidBody2D {
            body_type: BodyType::Static,
            ..RigidBody2D::default()
        };
        let at = |y, width| Transform {
            translation: DVec3::new(0.0, y, 0.0),
            scale: Vec3::new(width, 1.0, 1.0),
            ..Transform::IDENTITY
        };
        world.spawn((at(0.0, 10.0), fixed, BoxCollider2D::default()));
        let zone = world.spawn((at(4.0, 4.0), fixed, BoxCollider2D::default()));
        world.add_default(zone, "Zone").expect("added");
        let falling = (at(7.0, 1.0), RigidBody2D::default());
        let dropped = world.spawn((falling.0, falling.1, BoxCollider2D::default()));
        let mut most = 0.0;
        for _ in 0..240 {
            host.run(cx.world_mut());
            step_physics(cx.world_mut());
            let inside = cx
                .world()
                .get_field(zone, "Zone", "inside")
                .expect("a field");
            if let Value::F64(inside) = inside {
                most = f64::max(most, inside);
            }
        }
        let world = cx.world();
        let field = |name| world.get_field(zone, "Zone", name).expect("a field");
        assert_eq!(field("entered"), Value::F64(1.0));
        assert_eq!(field("left"), Value::F64(1.0));
        assert_eq!(field("inside"), Value::F64(0.0));
        assert_eq!(most, 1.0);
        assert_eq!(field("solid"), Value::Bool(false));
        let last = Value::Option(Some(Box::new(Value::Entity(dropped))));
        assert_eq!(field("last"), last);
        assert!(world.get::<BoxCollider2D>(zone).expect("alive").sensor);
        let y = world
            .get::<Transform>(dropped)
            .expect("alive")
            .translation
            .y;
        assert!((y - 1.0).abs() < 0.02, "on the floor: {y}");
    }
}

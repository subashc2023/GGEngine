//! `gg.contacts` and `gg.contact_events` (docs/LUAU.md): scripts' view of
//! [`World::contacts`](crate::World::contacts) and
//! [`World::contact_events`](crate::World::contact_events).
//!
//! The lists keep those methods' order, 2D's then 3D's, each by entity ID, so every peer and replay
//! reads them alike. A mod's scripts need `read` of the whole world: contacts tell where other
//! entities are.

use mlua::{LightUserData, Lua, Table, Value as Lua_};

use super::fields::{entity_of, handle};
use super::shared;
use crate::grant::Capability;
use crate::{Contact, Entity, World};

/// Adds `gg.contacts` and `gg.contact_events` to `gg`.
pub(super) fn install(lua: &Lua, gg: &Table) -> mlua::Result<()> {
    gg.raw_set("contacts", lua.create_function(contacts)?)?;
    gg.raw_set("contact_events", lua.create_function(contact_events)?)?;
    Ok(())
}

/// `gg.contacts(entity)`.
fn contacts(lua: &Lua, entity: LightUserData) -> mlua::Result<Table> {
    listed(lua, entity, |world, each| {
        for contact in world.contacts() {
            each(contact, None)?;
        }
        Ok(())
    })
}

/// `gg.contact_events(entity)`.
fn contact_events(lua: &Lua, entity: LightUserData) -> mlua::Result<Table> {
    listed(lua, entity, |world, each| {
        for event in world.contact_events() {
            each(event.contact, Some(event.started))?;
        }
        Ok(())
    })
}

/// Calls back with each contact, and `started` for an event's.
type Each<'a> = dyn FnMut(Contact, Option<bool>) -> mlua::Result<()> + 'a;

/// An array of a table for each contact of `entity`'s that `list` gives, in its order.
fn listed(
    lua: &Lua,
    entity: LightUserData,
    list: impl FnOnce(&World, &mut Each<'_>) -> mlua::Result<()>,
) -> mlua::Result<Table> {
    shared(|shared| {
        shared.needs(
            || Capability::Read(None),
            || "a contact, which reads which entities' colliders touch,".to_owned(),
        )?;
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

/// A contact's table: `other` and `sensor`, and `started` for an event's.
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

    /// Makes its box a sensor, counts what enters, leaves and is inside, and keeps the last to
    /// enter.
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

    /// A script makes its collider a sensor through its field and reads contacts and events at the
    /// ticks after the steps that made them: a box falling through enters, is inside awhile, leaves
    /// (an overlap, not a push), and lands on the floor below.
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

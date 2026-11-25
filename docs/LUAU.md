# Luau scripting reference

Everything Luau scripts and mods can use. The design is in
[ARCHITECTURE.md](../ARCHITECTURE.md#game-code); the Rust side, `Scripts` and `Mods`, is in
[luau/](../crates/gg-engine/src/luau/mod.rs). The unit test `luau::reference` fails when a `gg`
function or a script's diagnostic code has no entry here, or when this names a `gg` function that
isn't there: write a function as `gg.<name>` and a code as `` `script.bad` ``.

## A script

A script is a `.luau` file that declares one component type and returns it. Adding that component
to an entity (in a scene's RON, by `gg.add`, in a prefab) runs the script's `tick` for it.

```lua
local Shard = gg.component("Shard")                    -- a Rust component's field handles
local Spin = gg.declare("Spin", { speed = 1.5 })       -- this script's component

function Spin.tick(entity, dt)
    entity[Shard.angle] = (entity[Shard.angle] + entity[Spin.speed] * dt) % (2 * math.pi)
end

return Spin
```

Lifecycle, in order:

1. **Compile.** Refused: `^` (`script.pow`; use `x * x` or `math.pow`), `--!optimize` and
   `--!native` (`script.header`), text that doesn't compile (`script.bad`), and a function that
   assigns a local of the chunk (`script.state`).
2. **Chunk.** The file's top level runs at load, at each reload, and when a lockstep joiner or a
   run restored from a snapshot loads it. It must call `gg.declare` once and return the result,
   else `script.bad`. It may read, and call `gg.component`, `gg.declare` and `gg.quat`.
   `gg.component` finds only types already registered, so a game registers its Rust component
   types before it loads a script that names them, else `script.bad`. The chunk runs
   again where its changes are already made, so `gg.spawn`, `gg.despawn`, `gg.add`, `gg.remove`,
   `gg.tween`, `gg.play`, `gg.set_param` and field writes fail in it with `script.loading`,
   refusing the script. Once it has run, every table it keeps, globals included, is frozen; a
   chunk that keeps a coroutine or a buffer is refused (`script.state`). Keep state in
   components' fields.
3. **`tick(entity, dt)`.** The only hook, optional; anything but a function is `script.bad`. It
   runs once per entity with the component each tick, where the layer's tick chooses, before
   physics; `dt` is the fixed step, 1/64 s. Scripts run in their components' name order, each over
   the entities listed as its pass starts, skipping any despawned or without the component by its
   turn. An entity a script spawns or gives a component ticks that component's script from the
   next tick. A failing tick warns `script.error` once per script until it reloads; other ticks
   still run.
4. **Reload** (development builds, on save, between ticks). The chunk runs again. The component's
   name can't change (`script.renamed`). Fields migrate by name: a kept field keeps its value, a
   new one takes its default, a dropped one goes, and one whose kind changed takes its default and
   warns `script.field_reset`. A failed reload leaves the script as it was.

## Types

| In Luau | Is |
| :- | :- |
| entity | Light userdata of the entity's 64 bits, compared and used as a table key by them. `tostring` gives `entity <ID>`. Its metatable is frozen. `gg.entity` is "no entity". |
| field handle | Light userdata, one per field, in a `gg.component` or `gg.declare` table. Only a key of `entity[...]`; not an entity (`reflect.wrong_kind` where one is expected). |
| number | Every number field, integers too, as an `f64`: an integer past 2^53 loses its low bits. |
| boolean, nil | Bool fields. nil is an absent `Option<Entity>` or `Option<Vec3>` (a nav agent's destination). |
| vector | Luau's native four-wide `vector`, `vector.create(x, y, z, w)`, w optional. A `Vec2` reads as `(x, y, 0, 0)` and writes from `x`, `y`; a `Vec3` as `(x, y, z, 0)`; a `Vec4` whole. A `DVec3` position, such as a `Transform`'s `translation`, reads as a `Vec3`, each part rounded to the nearest `f32`, and writes from `x`, `y`, `z` as they are. So positions, the casts' included, step 2 mm at 16 km from the origin and 6 cm at 1,000 km. `vector.angle` is gone. |
| quaternion | A vector `(x, y, z, w)`, as `gg.quat` makes. Identity is `(0, 0, 0, 1)`. |

Strings, enums, structs and lists are fields scripts can't reach: they have no handle.

## Components and fields

| Call | Returns | Does |
| :- | :- | :- |
| `gg.component(name)` | table | A read-only table of type `name`'s field handles, by field name (`Shard.angle`); fields of kinds scripts can't reach are nil. Call it once, in the chunk. An unknown name fails without a code, naming the types; in a mod, a type it may neither read nor write fails with `mod.ungranted`. |
| `gg.declare(name, fields, options?)` | table | Declares this script's component; only in the chunk. `fields` maps name to default, which sets the field's kind: a number (`1.5`), a bool (`true`), a vector as a `Vec3` (`vector.create(1, 2, 3)`), or an entity or none (`gg.entity`). `options` is `{ on = "OtherType" }`, which makes it ride on that type: each entity with one gets this component, at its defaults, before the tick. Returns the table of its handles (`Spin`), which `tick` is assigned to. `script.bad` for a default of another kind, a field named `tick`, a non-string name, or a second call; `reflect.name_taken` for a name a Rust type or another script has. A reload keeps the name (`script.renamed`). |
| `gg.entity` | | Not a function: the entity-valued "none", as a field's default, or as a cast's `exclude` for none. `gg.alive` says it's dead; `gg.despawn` refuses it (`ecs.no_entity`). |

`entity[handle]` reads that field of that entity; `entity[handle] = value` writes it. A write is
checked against the field's description first, so a refused write changes nothing:

- `reflect.wrong_kind`: a value of another kind.
- `reflect.out_of_range`: a number outside the field's bounds or not finite; for an integer field,
  also a non-integer, or a negative number where the field has no sign.
- `reflect.not_normalized`: a quaternion that isn't a unit.
- `reflect.read_only`: the world's own fields (`Parent`, `ModelNode`, `PrefabRoot`).

Reads and writes fail with `ecs.no_entity` for a despawned entity and `reflect.missing_component`
for one without the component. Neither allocates.

## Changing entities

These change the world at once, mid-tick, as Rust calls do. They allocate as Rust's do, so don't
call them thousands of times per tick. Components go by type name and fields by name; fields not
given take their defaults. Errors are the engine's, with its codes, naming the entity and the
component:

- `script.bad_argument`: an argument of another kind; `script.loading`: called in a chunk.
- `reflect.unknown_type`: no such type; `reflect.no_field`: no such field.
- `reflect.read_only`: a type the world keeps (`Parent`, `ModelNode`, `PrefabRoot`, `ModelRoot`).
- `reflect.wrong_kind`: a value of another kind, or a field of a kind scripts don't reach.
- `reflect.no_default`: a field with no default that wasn't given.

| Call | Returns | Does |
| :- | :- | :- |
| `gg.spawn(components?, options?)` | entity | Spawns an entity. `components` maps type name to a table of fields. `options`: `parent = entity` makes it that entity's last child; `prefab = "id or path"` spawns that prefab's instance with it as the root, the components given replacing the prefab's, and `Transform` the identity if not given (`asset.no_prefab`). Any other option is `script.bad_option`. |
| `gg.despawn(entity)` | | Despawns it and its descendants. |
| `gg.add(entity, component, fields?)` | | Gives it a component it hasn't (`reflect.has_component`). |
| `gg.remove(entity, component)` | | Takes the component away (`reflect.missing_component`). |
| `gg.alive(entity)` | boolean | Whether the handle's entity is still in the world; false for `gg.entity` and nil. A handle to a despawned entity stays dead: no later entity in its slot is it. |

```lua
local kid = gg.spawn({ Transform = { translation = p }, Spin = {} }, { parent = entity })
```

## Math

`gg.quat` is a read-only table of rotations, as vectors. Angles are in radians.

| Call | Returns | Does |
| :- | :- | :- |
| `gg.quat.identity` | quaternion | The identity; a value, not a function. |
| `gg.quat.from_rotation_x(angle)`, `gg.quat.from_rotation_y(angle)`, `gg.quat.from_rotation_z(angle)` | quaternion | A rotation about that axis. |
| `gg.quat.from_axis_angle(axis, angle)` | quaternion | A rotation about `axis`, any length, which is normalized. `script.bad_argument` for an axis with a component that isn't finite or with none of magnitude 1e-6 or more, or an angle that isn't finite. |
| `gg.quat.mul(a, b)` | quaternion | The rotation that does `b`, then `a`. |
| `gg.quat.rotate(q, v)` | vector | `v` turned by `q`. |

`math` is Luau's, but its functions whose precision is the platform's are libm's, so results match
on every platform.

## Physics

These need 2D or 3D physics to have stepped, and read the world as the last step left it.

| Call | Returns | Does |
| :- | :- | :- |
| `gg.contacts(entity)` | `{ { other = entity, sensor = bool }, ... }` | The entities whose colliders touch `entity`'s, 2D's then 3D's, ordered by ID. `sensor` is true where either collider is a sensor (an overlap, not a push). Allocates its tables. |
| `gg.contact_events(entity)` | `{ { other, sensor, started = bool }, ... }` | The contacts that started (`started` true) or stopped at the last step. Scripts run before physics, so a tick reads them the tick after. An entity gone since is in the events of its contacts stopping. |
| `gg.cast_ray(from, toward, distance?, exclude?)` | entity, point, normal, distance | The first 3D collider a ray hits, sensors excepted: the entity, the point, the surface's normal there, and the distance along. Four nils for no hit. `toward` is any length but zero. A nil `distance` has no end. `exclude` is an entity whose colliders the ray passes through (`gg.entity` or nil for none). A ray that starts inside a collider hits it at distance 0. |
| `gg.cast_sphere(radius, from, toward, distance?, exclude?)` | as `gg.cast_ray` | A sphere swept along the line. |
| `gg.cast_box(size, rotation, from, toward, distance?, exclude?)` | as `gg.cast_ray` | A box as wide, tall and deep as `size` (a vector), turned by `rotation`, swept. |
| `gg.cast_capsule(height, radius, rotation, from, toward, distance?, exclude?)` | as `gg.cast_ray` | A capsule upright along Y, turned by `rotation`, swept. |

```lua
for _, event in gg.contact_events(entity) do
    if event.started then entity[Zone.entered] = entity[Zone.entered] + 1 end
end
```

A collider's `sensor`, `membership` and `filter` are its component's fields:
`entity[gg.component("BoxCollider2D").sensor] = true`.

## Navigation

| Call | Returns | Does |
| :- | :- | :- |
| `gg.find_path(from, to)` | `{ vector, ... }` or nil | The corners of the shortest path over the world's navmesh, `from` to `to`. nil without a navmesh volume, for an end off the mesh, or where no path joins them. |

An agent's destination steers it: `entity[NavAgent.destination] = to`, nil to stop.

## Animation

| Call | Returns | Does |
| :- | :- | :- |
| `gg.tween(entity, field, to, seconds, options?)` | | Eases the field whose handle is `field` from its value to `to` over `seconds`, replacing any tween of that field. The field is a number, a vector or a rotation; `to` is of its kind. `options`: `curve` (`"Linear"`, `"Sine"`, `"Quad"`, `"Cubic"`, `"Back"`, `"Elastic"`, `"Bounce"`), `ease` (`"In"`, `"Out"`, `"InOut"`), `playback` (`"Once"`, `"Loop"`, `"PingPong"`), and `delay`, seconds before it starts. |
| `gg.play(entity, clip, fade?)` | | Has `entity`, a model's root, play its model's clip named `clip`, cross-fading from what it plays over `fade` seconds, or cutting to it without one. A clip it hasn't fails with `animation.no_clip`, naming the model's clips. |
| `gg.set_param(entity, name, value)` | | Sets parameter `name` of its `AnimationStates`: a number for a float, a boolean for a bool or a trigger, which a transition that fires on it consumes. Allocates nothing once its VM has been given `name`, if it's among the first 64 names it's given. |

```lua
gg.tween(entity, Shard.angle, 3.14, 0.5, { curve = "Back", ease = "Out", playback = "PingPong" })
```

## The sandbox

A VM has the base library, `coroutine`, `table`, `string`, `utf8`, `bit32`, `math`, `buffer` and
`vector`. Gone: `os`, `io`, `debug`, `require`, `loadstring`, `getfenv`, `setfenv`, `newproxy`,
`collectgarbage`, `gcinfo`, `math.random`, `math.randomseed` and `vector.angle`. Globals and
libraries are read-only; a script's own globals freeze once its chunk has run.

- **Same everywhere.** `^` is refused. Library functions that make an integer of a number
  (`string.sub`'s positions, `bit32`'s and `buffer`'s arguments, `string.format`'s conversions)
  fail on a number past the integer's range or not finite; `string.format` also on a float it
  would write as NaN or infinity. `tostring` and `print` write a table, function, thread, buffer
  or userdata as its type's name, never an address.
- **`print(...)`** logs at info: its arguments as `tostring` writes them, between tabs, cut at
  4096 bytes with `[cut at 4096 bytes of N]`. A mod's lines carry `"mod"`; past 32 a second of
  ticks they go unprinted and `mod.unprinted` warns.
- **Budget.** A call into a script (one entity's tick, or a chunk as it loads) takes at most
  1,000,000 of Luau's interrupts, which come at each call and each turn of a loop; past them it
  fails (a chunk with `script.bad`, a tick with `script.error`). A mod's calls in a tick share one
  budget.
- **Memory.** A game's scripts' VM has no limit; a mod's holds at most 64 MiB, past which an
  allocation fails as an error does.
- **Garbage.** Collected at set points between calls, not by Luau's clock, so weak tables hold
  alike everywhere.
- **Debugging.** A development build serving the tool API debugs scripts: breakpoints by path and
  line, pauses and steps. It doesn't change what runs.

## Mods

A mod is a folder in the game's per-user `mods` folder (or the one `--mods` names) holding
`mod.ron` and the `.luau` scripts it lists, as source:

```ron
(
    name: "rainbow",
    version: "1.0.0",
    game: "Shards",
    capabilities: ["read:Shard", "write:Sprite"],
    scripts: ["rainbow.luau"],
)
```

A mod's scripts are game scripts in a VM of the mod's own, loaded in the order of the mods' names,
and kept to its `capabilities`: `read` and `write`, of the whole world or of one type
(`read:Shard`). The game must register its Rust component types before it loads mods
(`ecs.registered_late`). A mod is refused with a warning, and the game runs without it, for:

- a manifest that doesn't read (`mod.bad`), or files that can't be read (`mod.unreadable`);
- a script path that isn't a `.luau` file's inside the folder, a name another mod has, or a
  capability of the tool API's (`mod.bad`);
- a capability there isn't (`grant.unknown`);
- another game's manifest (`mod.wrong_game`);
- more than the game grants, or a chunk reaching past what it asked for (`mod.ungranted`);
- a script declaring a type another script has (`reflect.name_taken`);
- any failure of a script's load.

What a mod's scripts need:

| Call | Needs |
| :- | :- |
| `gg.component(T)` | `read` or `write` of `T` |
| `entity[T.f]` read, write | `read:T`, `write:T` (write doesn't read) |
| `gg.declare` | nothing: its types are its own, read and write; `on = T` needs `read:T`; a name another script has is `reflect.name_taken` |
| `gg.add`, `gg.remove` of `T` | `write:T` |
| `gg.spawn` | `write` of some type, and of each type it gives; with no components, under a `parent`, or of a `prefab`, `write` of the whole world |
| `gg.despawn` | `write` of the whole world |
| `gg.cast_ray`, `gg.cast_sphere`, `gg.cast_box`, `gg.cast_capsule`, `gg.find_path`, `gg.contacts`, `gg.contact_events` | `read` of the whole world |
| `gg.tween` | `write:T` of the field's type, and `write:Tweens` |
| `gg.play` | `write:Animator` |
| `gg.set_param` | `write:AnimationStates` |

A call without what it needs fails with `mod.ungranted`, naming the capability: a tick shows it as
its `script.error`'s `cause`, and in a chunk it refuses the mod. A mod reaches the game's entities
through its own component riding on the game's. A run's mods go whole into snapshots and
recordings, each granted no more than its game grants. A mod's limits are in [the
sandbox](#the-sandbox).

## Diagnostics

Each has a stable `code` and names its subject. The scripts' own:

| Code | When |
| :- | :- |
| `script.unreadable` | a script file can't be read, isn't a regular file or isn't UTF-8 |
| `script.pow` | the text uses `^` |
| `script.header` | the text sets `--!optimize` or `--!native` |
| `script.bad` | it doesn't compile, fails as its chunk runs, declares none or two components, returns other than its component, or has a bad `tick` or field |
| `script.state` | a function assigns a local of the chunk, or the chunk keeps a coroutine or a buffer |
| `script.vm` | Luau can't start, or its bytecode can't be read |
| `script.renamed` | a reload declares another component name |
| `script.unknown` | a reload of a path no layer loaded |
| `script.loading` | a call that changes the world, field writes included, in a chunk |
| `script.bad_argument` | an argument of the wrong kind to `gg.spawn`, `gg.despawn`, `gg.add`, `gg.remove` or `gg.alive`, or an axis or angle `gg.quat.from_axis_angle` refuses |
| `script.bad_option` | an unknown option of `gg.spawn` |
| `script.error` | warn, once per script until it reloads: a tick failed. Carries `script`, `entity` and `cause`, the code of the engine's error it failed with |
| `script.field_reset` | warn: a reload changed a field's kind, so each takes its default |
| `mod.bad`, `mod.unreadable`, `mod.wrong_game`, `mod.ungranted` | a mod's refusals, as above |
| `mod.unprinted` | warn: a mod printed past 32 lines in a second of ticks |
| `grant.unknown` | a manifest asks for a capability there isn't |

The engine's errors that reach a script through a call keep their codes, in a `script.error`'s
`cause`: `ecs.no_entity`, `ecs.registered_late`, `asset.no_prefab`, `mod.ungranted`,
`animation.no_clip` and the other `animation.*` of `gg.tween`, `gg.play` and `gg.set_param`, and
`reflect.*` (`reflect.wrong_kind`, `reflect.out_of_range`, `reflect.not_normalized`,
`reflect.read_only`, `reflect.no_field`, `reflect.unknown_type`, `reflect.no_default`,
`reflect.has_component`, `reflect.missing_component`, `reflect.name_taken`). A few carry text and
no code: `gg.component` of a type there isn't, and `gg.tween`'s arguments and options.

`Script reloaded` and `Mod loaded` log a script's life, at info.

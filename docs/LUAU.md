# Luau scripting reference

Everything a Luau script, and a mod, can use. The design and its reasons are in
[ARCHITECTURE.md](../ARCHITECTURE.md#game-code); the host's side, loading and running scripts from
Rust, is `Scripts` and `Mods` in [luau/](../crates/gg-engine/src/luau/mod.rs). The unit test
`luau::reference` fails when a `gg` function or a diagnostic code of a script has no entry here, or
this names a `gg` function that isn't there: keep a function's entry as `gg.<name>`, a code as
`` `script.bad` ``.

## A script

A script is a `.luau` file that declares one component type and returns it. Adding that component
to an entity (a scene's RON, `gg.add`, a prefab) runs the script's `tick` for it.

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
   `--!native` (`script.header`), text that doesn't compile (`script.bad`), a function that assigns
   a local of the chunk (`script.state`).
2. **Chunk.** The file's top level runs once at load, again at each reload, and again as a lockstep
   joiner or a run restored from a snapshot loads it. It must call `gg.declare` once and return
   what that returns, else `script.bad`. It may call `gg.component`, `gg.declare` and `gg.quat`.
   Every call that changes the world fails with `script.loading` in it, and refuses the script:
   `gg.spawn`, `gg.despawn`, `gg.add`, `gg.remove`, `gg.tween`, `gg.play`, `gg.set_param` and a
   write of a field, `entity[handle] = value`, as a chunk runs again where the change is already
   there: leave the world to `tick`. Reading is fine.
   Afterwards every table the chunk keeps, its globals among them, is frozen, and a chunk that
   keeps a coroutine or a buffer is refused (`script.state`): keep state in components' fields.
3. **`tick(entity, dt)`.** The only hook, optional, a function (`script.bad` if it's other):
   called each tick, where the layer's tick chooses, before physics, once for each entity with the
   component, `dt` the fixed step in seconds (1/64). Scripts run in the order of their components'
   names; each over the entities listed as its pass starts, passing over one despawned or without
   its component by its turn. An entity a script spawns, or gives a component, ticks that
   component's script at the next tick. A tick that fails warns `script.error` once for the script
   until it reloads, and the others' go on.
4. **Reload** (development builds, as a file is saved, between ticks). The chunk runs again: the
   component's name stays (`script.renamed`), its fields migrate by name: a kept field keeps its
   value, an added one takes its default, a dropped one goes, one whose kind changed takes its
   default and warns `script.field_reset`. A reload that fails leaves the script as it was.

`gg.declare`'s `on` option makes a component ride on another: each entity with that type is given
this one, at its defaults, before the tick.

## Types

| In Luau | Is |
| :- | :- |
| entity | Light userdata of the entity's 64 bits: compared and used as a table key by them, `tostring` gives `entity <ID>`, its metatable is frozen. `gg.entity` is "no entity": a field's default of none, which `gg.alive` says is dead and `gg.despawn` refuses (`ecs.no_entity`). |
| field handle | Light userdata a `gg.component` or `gg.declare` table holds for each field, only a key of `entity[...]`; it isn't an entity (`reflect.wrong_kind` where one is given). |
| number | Every number field, integers too, as an `f64`: an integer past 2^53 loses its low bits. |
| boolean, nil | Bool fields; nil is an absent `Option<Entity>` or `Option<Vec3>` (a nav agent's destination). |
| vector | Luau's four-wide native `vector`, `vector.create(x, y, z, w)`, w optional. A `Vec2` reads as `(x, y, 0, 0)` and writes from `x`, `y`; a `Vec3` as `(x, y, z, 0)`; a `Vec4` whole. `vector.angle` is gone. |
| quaternion | A vector `(x, y, z, w)`, as `gg.quat` makes. Identity is `(0, 0, 0, 1)`. |

Strings, enums, structs and lists are fields scripts don't reach: they have no handle.

## Components and fields

`gg.component(name)` gives a read-only table of the handles of component type `name`'s fields, by
name: `Shard.angle`. Fields of the kinds above are there; the rest are nil. A script's own
component's table, `Spin`, is what `gg.declare` returns.

`entity[handle]` reads, and `entity[handle] = value` writes, that field of that entity. A write is
checked against the field's description before it changes anything, so one refused changes
nothing: `reflect.wrong_kind` for another kind, `reflect.out_of_range` for a number outside the
field's bounds, that isn't finite, or, for an integer field, isn't an integer or is negative where
the field has no sign, `reflect.not_normalized` for a quaternion that isn't a unit,
`reflect.read_only` for the world's own fields (`Parent`, `ModelNode`, `PrefabRoot`). Both fail with
`ecs.no_entity` for an entity despawned and `reflect.missing_component` for one without the
component. Reading and writing allocate nothing.

A script's own fields hold a number, a bool, a vector (a `Vec3`) or an entity or none, by their
defaults: `1.5`, `true`, `vector.create(1, 2, 3)`, `gg.entity`. A default of another kind, a
field named `tick` or a non-string name is `script.bad`; so is a second `gg.declare`. A name a
Rust type or another script has is `reflect.name_taken`.

### gg.component

`gg.component(name) -> table`. Fails (no code, naming the types) for an unknown name; in a mod, with
`mod.ungranted` where it may neither read nor write the type. Call it in the chunk, once.

### gg.declare

`gg.declare(name, fields, options?) -> table`. Declares this script's component, `fields` a table
of name to default, `options` `{ on = "OtherType" }`. Only in the chunk. Returns the table of its
handles, which `tick` is assigned to. A reload keeps the name (`script.renamed`).

### gg.entity

`gg.entity`: the entity-valued "none", as a field's default, or as `exclude` of a cast for none.

## Changing entities

Changes happen at once, as Rust code's do, in a tick, and allocate as Rust's do: not in the
thousands per tick. Components go by their types' names, fields' values by name, those not given at
their defaults. They fail with the engine's errors and codes, naming the entity and the component,
and with `script.bad_argument` for an argument of another kind, `script.loading` in a chunk. A
different call's errors, `gg.tween`'s, `gg.play`'s and `gg.set_param`'s, keep the engine's codes too
(`animation.no_clip` for a clip there isn't).
`reflect.unknown_type` is a type that isn't there, `reflect.read_only` one the world keeps
(`Parent`, `ModelNode`, `PrefabRoot`, `ModelRoot`), `reflect.no_field` a field it hasn't,
`reflect.wrong_kind` a value, or a field of a kind scripts don't reach, and
`reflect.no_default` a field that has none and wasn't given.

### gg.spawn

`gg.spawn(components?, options?) -> entity`. `components` is a table of type name to a table of
fields; `options` is `{ parent = entity, prefab = "id or path" }`: `parent` makes it that entity's
last child, `prefab` spawns that prefab's instance with it as the root, the components given in
place of the prefab's, `Transform` the identity if not given (`asset.no_prefab`). Any other option
is `script.bad_option`.

```lua
local kid = gg.spawn({ Transform = { translation = p }, Spin = {} }, { parent = entity })
```

### gg.despawn

`gg.despawn(entity)`. Despawns it and its descendants.

### gg.add

`gg.add(entity, component, fields?)`. Gives it a component it hasn't (`reflect.has_component`).

### gg.remove

`gg.remove(entity, component)`. Takes it away (`reflect.missing_component`).

### gg.alive

`gg.alive(entity) -> boolean`. Whether the handle is of an entity still in the world; false for
`gg.entity` and nil. A handle to a despawned entity stays dead: no other entity in its slot is it.

## Math

### gg.quat

Rotations, as vectors; read-only table.

- `gg.quat.identity`: the identity.
- `gg.quat.from_rotation_x(angle)`, `gg.quat.from_rotation_y(angle)`,
  `gg.quat.from_rotation_z(angle)`: about an axis, radians.
- `gg.quat.from_axis_angle(axis, angle)`: about `axis`, any length, which is normalized; radians.
  Fails with `script.bad_argument` for an axis of a component that isn't finite or of none past
  1e-6, and for an angle that isn't finite.
- `gg.quat.mul(a, b)`: the rotation that does `b`, then `a`.
- `gg.quat.rotate(q, v)`: `v` turned by `q`.

`math` is Luau's, but its functions whose precision is the platform's are libm's, so that results
match on every platform.

## Physics

Need 3D or 2D physics to have stepped; they read the world as the last step left it.

### gg.contacts

`gg.contacts(entity) -> { { other = entity, sensor = bool }, ... }`. The entities whose colliders
touch `entity`'s as the last step left them, 2D's then 3D's, ordered by their IDs; `sensor` where
either collider is one (an overlap, not a push). Allocates its tables.

### gg.contact_events

`gg.contact_events(entity) -> { { other, sensor, started = bool }, ... }`. The contacts that
started (`started` true) or stopped at the last step, which a tick reads at the tick after, as
scripts run before physics. An entity gone since is in the events of its contacts stopping.

```lua
for _, event in gg.contact_events(entity) do
    if event.started then entity[Zone.entered] = entity[Zone.entered] + 1 end
end
```

A collider's `sensor`, `membership` and `filter` are its component's fields:
`entity[gg.component("BoxCollider2D").sensor] = true`.

### gg.cast_ray

`gg.cast_ray(from, toward, distance?, exclude?) -> entity, point, normal, distance`. What a ray hits
first of the 3D colliders, but sensors: the entity, where, the surface's normal there, and how far
along. Four nils for no hit. `toward` is any length but none; a nil `distance` is without end;
`exclude` an entity whose colliders it passes through (`gg.entity` or nil for none). A ray that
starts inside a collider hits it at distance 0.

### gg.cast_sphere

`gg.cast_sphere(radius, from, toward, distance?, exclude?)`: a sphere swept along the line; returns
as `gg.cast_ray`.

### gg.cast_box

`gg.cast_box(size, rotation, from, toward, distance?, exclude?)`: a box as wide, tall and deep as `size`
(a vector), turned by `rotation`, swept; returns as `gg.cast_ray`.

### gg.cast_capsule

`gg.cast_capsule(height, radius, rotation, from, toward, distance?, exclude?)`: a capsule, upright
along Y and turned, swept; returns as `gg.cast_ray`.

## Navigation

### gg.find_path

`gg.find_path(from, to) -> { vector, ... } | nil`. The corners of the shortest path over the
world's navmesh, `from` to `to`; nil without a navmesh volume, for an end off the mesh, or where no
path joins them. An agent's destination steers it: `entity[NavAgent.destination] = to`, nil to
stop.

## Animation

### gg.tween

`gg.tween(entity, field, to, seconds, options?)`. Eases the field whose handle is `field` from what
it holds to `to` over `seconds`, in place of a tween of that field. The field is a number, a
vector or a rotation; `to` is of its kind. `options`: `curve` (`"Linear"`, `"Sine"`, `"Quad"`,
`"Cubic"`, `"Back"`, `"Elastic"`, `"Bounce"`), `ease` (`"In"`, `"Out"`, `"InOut"`), `playback`
(`"Once"`, `"Loop"`, `"PingPong"`) and `delay`, seconds before it starts.

```lua
gg.tween(entity, Shard.angle, 3.14, 0.5, { curve = "Back", ease = "Out", playback = "PingPong" })
```

### gg.play

`gg.play(entity, clip, fade?)`. Has `entity`, a model's root, play its model's clip named `clip`,
cross-fading from what it plays over `fade` seconds, or cutting to it without one. Fails with a
message naming the model's clips, `animation.no_clip`, for one it hasn't.

### gg.set_param

`gg.set_param(entity, name, value)`. Sets parameter `name` of its `AnimationStates`: a number for a
float, a boolean for a bool or a trigger, which a transition that fires on it consumes. Allocates
nothing once its VM has been given the name, among the first 64 names it's given.

## The sandbox

A VM has the base library, `coroutine`, `table`, `string`, `utf8`, `bit32`, `math`, `buffer` and
`vector`. Gone: `os`, `io`, `debug`, `require`, `loadstring`, `getfenv`, `setfenv`, `newproxy`,
`collectgarbage`, `gcinfo`, `math.random`, `math.randomseed` and `vector.angle`. The globals and
libraries are read only; a script's own globals are frozen once its chunk has run.

- **Same everywhere.** `^` is refused. Library functions that make an integer of a number
  (`string.sub`'s positions, `bit32`'s and `buffer`'s arguments, `string.format`'s conversions)
  fail at a number past the integer's range or not finite, or, for `string.format`, a float it would
  write as NaN or infinity. `tostring` and `print` write a table, function, thread, buffer or
  userdata as its type's name, never an address.
- **`print(...)`** logs at info, its arguments as `tostring` writes them between tabs, cut at 4096
  bytes with `[cut at 4096 bytes of N]`; a mod's carry `"mod"` and are held to 32 lines a second of
  ticks.
- **Budget.** A call into a script, a tick for one entity or a chunk as it loads, takes at most
  1,000,000 of Luau's interrupts, which come at each call and each turn of a loop, then fails (a
  chunk, `script.bad`; a tick, `script.error`). A mod's calls in a tick share one budget.
- **Memory.** A game's scripts' VM has no limit; a mod's holds at most 64 MiB, past which an
  allocation fails as an error does.
- **Garbage.** Collected at set points between calls, not by Luau's clock, so that weak tables
  hold alike everywhere.
- **Debugging.** A development build serving the tool API debugs scripts: breakpoints, pauses and
  steps by path and line. It doesn't change what runs.

## Mods

A mod is a folder in the game's per-user `mods` (or the folder `--mods` names) holding `mod.ron`
and the `.luau` scripts it lists, as source:

```ron
(
    name: "rainbow",
    version: "1.0.0",
    game: "Shards",
    capabilities: ["read:Shard", "write:Sprite"],
    scripts: ["rainbow.luau"],
)
```

A mod's scripts are a game's scripts, in a VM of the mod's own, loaded in the order of the mods'
names, kept to its `capabilities`: `read` and `write`, of the whole world or of a type
(`read:Shard`). Refused with a warning, the game running without the mod: a manifest that doesn't
read (`mod.bad`, `mod.unreadable` for files that can't be read); a script path that isn't a
`.luau` file's inside the folder, a name another mod has, or a capability of the tool API's
(`mod.bad`); one that isn't a capability (`grant.unknown`); another game's (`mod.wrong_game`);
more than the game grants, or a chunk that reaches past what it asked for (`mod.ungranted`); a
script that declares a type another script has (`reflect.name_taken`); or any failure of a script's
load. The game must register its Rust component types before it loads mods
(`ecs.registered_late`).

What a mod's scripts may do:

| Call | Needs |
| :- | :- |
| `gg.component(T)` | `read` or `write` of `T` |
| `entity[T.f]` read, write | `read:T`, `write:T` (write doesn't read) |
| `gg.declare` | nothing; its types are its own, read and write; `on = T` needs `read:T`; a name another script has is `reflect.name_taken` |
| `gg.add`, `gg.remove` of `T` | `write:T` |
| `gg.spawn` | `write` of some type, and of each type it gives; with no components, `parent` or `prefab`, `write` of the whole world |
| `gg.despawn` | `write` of the whole world |
| `gg.cast_ray`, `gg.cast_sphere`, `gg.cast_box`, `gg.cast_capsule`, `gg.find_path`, `gg.contacts`, `gg.contact_events` | `read` of the whole world |
| `gg.tween` | `write:T` of the field's type and `write:Tweens` |
| `gg.play` | `write:Animator` |
| `gg.set_param` | `write:AnimationStates` |

A call without what it needs fails with `mod.ungranted` and a message naming the capability, which
a tick shows as its `script.error`'s `cause`; in a chunk the mod is refused. A mod reaches the game's
entities through its own component riding on the game's. A run's mods go whole in snapshots and
recordings, each granted no more than its game grants.

Limits: 64 MiB, one budget of 1,000,000 interrupts for its calls in a tick, 32 printed lines a
second of ticks, past which they go unprinted and `mod.unprinted` warns.

## Diagnostics

Each has a stable `code` and names its subject. The scripts' own, with the call that raises it:

| Code | When |
| :- | :- |
| `script.unreadable` | a script file can't be read, isn't a regular file or isn't UTF-8 |
| `script.pow` | the text uses `^` |
| `script.header` | the text sets `--!optimize` or `--!native` |
| `script.bad` | it doesn't compile, fails as its chunk runs, declares none or two components, returns other than its component, or a bad `tick` or field |
| `script.state` | a function assigns a local of the chunk, or the chunk keeps a coroutine or a buffer |
| `script.vm` | Luau can't start, or its bytecode can't be read |
| `script.renamed` | a reload declares another component name |
| `script.unknown` | a reload of a path no layer loaded |
| `script.loading` | a call that changes the world, entity fields among them, in a chunk |
| `script.bad_argument` | an argument of the wrong kind to `gg.spawn`, `gg.despawn`, `gg.add`, `gg.remove`, `gg.alive`, or an axis that is no direction to `gg.quat.from_axis_angle` |
| `script.bad_option` | an unknown option of `gg.spawn` |
| `script.error` | warn, once per script until it reloads: a tick failed; carries `script`, `entity` and `cause`, the code of an engine error it failed with |
| `script.field_reset` | warn: a reload changed a field's kind, so each takes its default |
| `mod.bad`, `mod.unreadable`, `mod.wrong_game`, `mod.ungranted` | a mod's refusals, as above |
| `mod.unprinted` | warn: a mod printed past 32 lines in a second of ticks |
| `grant.unknown` | a manifest asks for a capability there isn't |

The engine's errors that reach a script through a call carry their own codes, in a `script.error`'s
`cause`: `ecs.no_entity`, `ecs.registered_late`, `asset.no_prefab`, `animation.no_clip` and the other
`animation.*` of `gg.tween`, `gg.play` and `gg.set_param`, and `reflect.*`
(`reflect.wrong_kind`, `reflect.out_of_range`, `reflect.not_normalized`, `reflect.read_only`,
`reflect.no_field`, `reflect.unknown_type`, `reflect.no_default`, `reflect.has_component`,
`reflect.missing_component`, `reflect.name_taken`). `mod.ungranted` is one too. A few carry text and no code: `gg.component` of a type there isn't,
and `gg.tween`'s arguments and options.

Logs of a script's life: `Script reloaded` and `Mod loaded` at info.

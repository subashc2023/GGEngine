-- The shards' spin: each turns about the view's axis, `speed` radians a second, its angle kept in
-- its `Shard`, and its sprite turned to it, a corner up.
local Shard = gg.component("Shard")
local Transform = gg.component("Transform")

local Spin = gg.declare("Spin", { speed = 1.5 })

function Spin.tick(entity, dt)
    local angle = (entity[Shard.angle] + entity[Spin.speed] * dt) % (2 * math.pi)
    entity[Shard.angle] = angle
    entity[Transform.rotation] = gg.quat.from_rotation_z(angle + math.pi / 4)
end

return Spin

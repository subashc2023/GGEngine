-- A pulse: every `every` seconds, sets its entity's scale to `size` and has a tween ease it back
-- to `rest`, springing as it settles, through `gg.tween`.
local Transform = gg.component("Transform")

local Pulse = gg.declare("Pulse", {
    every = 1.5,
    rest = vector.create(1, 1, 1),
    size = vector.create(1.5, 1.5, 1),
    time = 0,
})

function Pulse.tick(entity, dt)
    local time = entity[Pulse.time] + dt
    if time >= entity[Pulse.every] then
        time = 0
        entity[Transform.scale] = entity[Pulse.size]
        gg.tween(entity, Transform.scale, entity[Pulse.rest], 0.8, { curve = "Elastic", ease = "Out" })
    end
    entity[Pulse.time] = time
end

return Pulse

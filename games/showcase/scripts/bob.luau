-- A bob: moves its entity up and down, along its parent's Y, as far as `height` either way of
-- where it starts, once every `period` seconds of the simulation's time; `time` keeps the seconds
-- it has run, as a script keeps its state in its components.
local Transform = gg.component("Transform")

local Bob = gg.declare("Bob", { height = 0, period = 2, time = 0 })

-- How far up of where it started `entity` is `time` seconds in.
local function at(entity, time)
    local period = math.max(entity[Bob.period], 0.1)
    return entity[Bob.height] * math.sin(2 * math.pi * time / period)
end

function Bob.tick(entity, dt)
    local before = entity[Bob.time]
    local time = before + dt
    entity[Bob.time] = time
    local up = at(entity, time) - at(entity, before)
    entity[Transform.translation] = entity[Transform.translation] + vector.create(0, up, 0)
end

return Bob

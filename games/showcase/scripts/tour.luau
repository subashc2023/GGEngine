-- A tour: sends its entity's agent to its four stops, `a` to `d`, in turn, by its `NavAgent`'s
-- destination, waiting `pause` seconds at each once it has arrived, and going on to the next after
-- `patience` seconds without arriving, as where another agent stands on its stop. `next` is the
-- stop it goes to next, from 0, `waiting` whether it's at one, and `time` the seconds since it
-- arrived or set off.
local NavAgent = gg.component("NavAgent")

local Tour = gg.declare("Tour", {
    a = vector.create(0, 0, 0),
    b = vector.create(0, 0, 0),
    c = vector.create(0, 0, 0),
    d = vector.create(0, 0, 0),
    next = 0,
    pause = 1,
    patience = 12,
    waiting = false,
    time = 0,
})

-- The tour's stop `index`, from 0.
local function stop(entity, index)
    if index == 0 then
        return entity[Tour.a]
    elseif index == 1 then
        return entity[Tour.b]
    elseif index == 2 then
        return entity[Tour.c]
    end
    return entity[Tour.d]
end

function Tour.tick(entity, dt)
    local waiting = entity[NavAgent.arrived] or entity[NavAgent.destination] == nil
    if waiting ~= entity[Tour.waiting] then
        -- It has just arrived, or just set off.
        entity[Tour.waiting] = waiting
        entity[Tour.time] = 0
    end
    local time = entity[Tour.time] + dt
    entity[Tour.time] = time
    if time < (waiting and entity[Tour.pause] or entity[Tour.patience]) then
        return
    end
    local next = entity[Tour.next]
    entity[NavAgent.destination] = stop(entity, next)
    entity[Tour.next] = (next + 1) % 4
    entity[Tour.time] = 0
end

return Tour

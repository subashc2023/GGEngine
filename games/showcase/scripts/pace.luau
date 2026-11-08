-- Pace: walks its fox round a circle of `radius` about `center`, at a speed that goes from standing
-- to a walk, a jog and a run and back each round of the speeds below, which it sets as its state
-- machine's `speed`, through `gg.set_param`; and `look_at` seconds into each round, walking, pulls
-- its state machine's `look` trigger and stands, at a speed of 0, for `look` seconds while the fox
-- looks about, before it goes on. `time` is how far into the round it is, `angle` where it is round
-- the circle, and `looking` the seconds of its look left.
local Transform = gg.component("Transform")

-- The speeds, in units a second, at seconds into a round: standing, walking, jogging and running,
-- each held a while, and slowing back to standing.
local SPEEDS = {
    { 0, 0 },
    { 2, 0 },
    { 3, 1.6 },
    { 6, 1.6 },
    { 7, 3.3 },
    { 10, 3.3 },
    { 11, 5 },
    { 15, 5 },
    { 17, 0 },
    { 18, 0 },
}
local ROUND = SPEEDS[#SPEEDS][1]

local Pace = gg.declare("Pace", {
    center = vector.create(0, 0, 0),
    radius = 3,
    look_at = 4.5,
    look = 2,
    time = 0,
    angle = 0,
    looking = 0,
})

-- The speed `time` seconds into a round, along a line between the speeds either side.
local function speed_at(time)
    for i = 2, #SPEEDS do
        local from, to = SPEEDS[i - 1], SPEEDS[i]
        if time <= to[1] then
            return from[2] + (to[2] - from[2]) * (time - from[1]) / (to[1] - from[1])
        end
    end
    return 0
end

function Pace.tick(entity, dt)
    local looking = entity[Pace.looking]
    if looking > 0 then
        entity[Pace.looking] = math.max(looking - dt, 0)
        return
    end
    local before = entity[Pace.time]
    local time = (before + dt) % ROUND
    entity[Pace.time] = time
    local speed = speed_at(time)
    local look_at = entity[Pace.look_at]
    if before < look_at and time >= look_at then
        gg.set_param(entity, "look", true)
        entity[Pace.looking] = entity[Pace.look]
        speed = 0
    end
    gg.set_param(entity, "speed", speed)
    local radius = entity[Pace.radius]
    local angle = (entity[Pace.angle] + speed * dt / radius) % (2 * math.pi)
    entity[Pace.angle] = angle
    local around = vector.create(radius * math.sin(angle), 0, radius * math.cos(angle))
    entity[Transform.translation] = entity[Pace.center] + around
    -- Facing the way it goes round, its +Z along the circle.
    entity[Transform.rotation] = gg.quat.from_rotation_y(angle + math.pi / 2)
end

return Pace

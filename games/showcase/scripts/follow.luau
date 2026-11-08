-- A follower: keeps its entity, a camera, `distance` behind `target` and `height` above it, looking
-- the way the target faces, `pitch` radians down; nearer where a sphere `radius` across, cast back
-- from above the target, hits a wall first, so that walls don't come between them.
local Transform = gg.component("Transform")

local Follow = gg.declare("Follow", {
    distance = 5,
    height = 2,
    pitch = 0.25,
    radius = 0.25,
    target = gg.entity,
})

local back = vector.create(0, 0, 1)

function Follow.tick(entity, dt)
    local target = entity[Follow.target]
    if target == nil then
        return
    end
    local turned = target[Transform.rotation]
    local behind = gg.quat.rotate(turned, back)
    local above = target[Transform.translation] + vector.create(0, entity[Follow.height], 0)
    local distance = entity[Follow.distance]
    local hit, _, _, reached = gg.cast_sphere(entity[Follow.radius], above, behind, distance, target)
    if hit ~= nil then
        distance = reached
    end
    entity[Transform.translation] = above + behind * distance
    entity[Transform.rotation] = gg.quat.mul(turned, gg.quat.from_rotation_x(-entity[Follow.pitch]))
end

return Follow

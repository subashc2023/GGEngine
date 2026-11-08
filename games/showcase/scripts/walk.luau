-- A walker: walks its character, a CharacterController3D's, the way `heading` says, which the game
-- sets from the keys or a pad: forward along -Z as `heading.z` is less than 0, turning about Y as
-- `heading.x` says; and jumps when `jump` is set, if it's standing, which the game sets as a key is
-- pressed and this clears as it takes it.
local Transform = gg.component("Transform")
local Character = gg.component("CharacterController3D")

local Walk = gg.declare("Walk", {
    heading = vector.create(0, 0, 0),
    jump = false,
    jump_speed = 5,
    speed = 4,
    turn_speed = 2.5,
})

local forward = vector.create(0, 0, -1)

function Walk.tick(entity, dt)
    local heading = entity[Walk.heading]
    local turn = gg.quat.from_rotation_y(-heading.x * entity[Walk.turn_speed] * dt)
    -- A unit again, as each product of two rounds a little off one.
    local turned = vector.normalize(gg.quat.mul(turn, entity[Transform.rotation]))
    entity[Transform.rotation] = turned
    local across = gg.quat.rotate(turned, forward) * (-heading.z * entity[Walk.speed])
    local velocity = entity[Character.velocity]
    local up = velocity.y
    if entity[Walk.jump] then
        entity[Walk.jump] = false
        if entity[Character.grounded] then
            up = entity[Walk.jump_speed]
        end
    end
    entity[Character.velocity] = vector.create(across.x, up, across.z)
end

return Walk

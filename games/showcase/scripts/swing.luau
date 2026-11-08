-- A swing: every `every` seconds, turns the motor of the joint on its entity around, through the
-- joint's own fields: where it drives the joint to and how fast, each to the other side of 0.
local Joint3D = gg.component("Joint3D")

local Swing = gg.declare("Swing", { every = 2, time = 0 })

function Swing.tick(entity, dt)
    local time = entity[Swing.time] + dt
    if time >= entity[Swing.every] then
        time -= entity[Swing.every]
        entity[Joint3D.target_position] = -entity[Joint3D.target_position]
        entity[Joint3D.target_velocity] = -entity[Joint3D.target_velocity]
    end
    entity[Swing.time] = time
end

return Swing

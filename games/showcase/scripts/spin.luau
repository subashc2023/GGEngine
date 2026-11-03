-- A spin: turns its entity about `axis`, in its parent's space, `speed` radians a second,
-- counterclockwise as the axis points at the viewer; about Z, out of the view, without an axis.
local Transform = gg.component("Transform")

local Spin = gg.declare("Spin", { axis = vector.create(0, 0, 0), speed = 0 })

-- The axis to turn about: `axis` made a unit, or Z for one of no length.
local function about(axis)
    local length = vector.magnitude(axis)
    if length > 0 and length < math.huge then
        return vector.normalize(axis)
    end
    return vector.create(0, 0, 1)
end

function Spin.tick(entity, dt)
    local turn = gg.quat.from_axis_angle(about(entity[Spin.axis]), entity[Spin.speed] * dt)
    -- A unit again, as each product of two rounds a little off one.
    entity[Transform.rotation] = vector.normalize(gg.quat.mul(turn, entity[Transform.rotation]))
end

return Spin

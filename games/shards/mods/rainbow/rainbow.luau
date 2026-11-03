-- A sample mod: each shard's color cycles through the hues, `turns` of them a second, its hue kept
-- in its own `Rainbow`, which rides on the game's `Shard`.
local Sprite = gg.component("Sprite")

local Rainbow = gg.declare("Rainbow", { hue = 0, turns = 0.25 }, { on = "Shard" })

function Rainbow.tick(entity, dt)
    local hue = (entity[Rainbow.hue] + entity[Rainbow.turns] * dt) % 1
    entity[Rainbow.hue] = hue
    local red = math.clamp(math.abs(hue * 6 - 3) - 1, 0, 1)
    local green = math.clamp(2 - math.abs(hue * 6 - 2), 0, 1)
    local blue = math.clamp(2 - math.abs(hue * 6 - 4), 0, 1)
    entity[Sprite.color] = vector.create(red, green, blue, 1)
end

return Rainbow

-- Logs the fight's result. Rust sets `Fight.result` (1 won, 2 lost) as a fight ends; this prints it
-- and clears it. Card effects are Rust's, so their numbers live in one place.
local Fight = gg.component("Fight")

local Effects = gg.declare("Effects", { logged = 0 })

function Effects.tick(entity, dt)
    local result = entity[Fight.result]
    if result == 1 then
        print("Fight won")
    elseif result == 2 then
        print("Fight lost")
    end
    if result ~= 0 then
        entity[Fight.result] = 0
        entity[Effects.logged] = entity[Effects.logged] + 1
    end
end

return Effects

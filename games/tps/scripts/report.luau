-- A reporter: logs each hit the score has gained since its last tick, which Rust code can't log.
local Score = gg.component("Score")

local Report = gg.declare("Report", { seen = 0 }, { on = "Score" })

function Report.tick(entity, dt)
    local hits = entity[Score.hits]
    if hits ~= entity[Report.seen] then
        entity[Report.seen] = hits
        local at = entity[Score.last]
        print("Target hit", hits, entity[Score.points], at.x, at.y, at.z)
    end
end

return Report

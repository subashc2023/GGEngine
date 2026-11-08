-- Moods: every `every` seconds, has its model's root cross-fade to its model's next clip, Survey,
-- then Walk, then Run, and round again, over `fade` seconds, through `gg.play`. `mood` is the clip
-- it's on, by its place among them, from 0.
local CLIPS = { "Survey", "Walk", "Run" }

local Moods = gg.declare("Moods", {
    every = 3,
    fade = 0.5,
    time = 0,
    mood = 0,
})

function Moods.tick(entity, dt)
    local time = entity[Moods.time] + dt
    if time >= entity[Moods.every] then
        time = 0
        local mood = (entity[Moods.mood] + 1) % #CLIPS
        entity[Moods.mood] = mood
        gg.play(entity, CLIPS[mood + 1], entity[Moods.fade])
    end
    entity[Moods.time] = time
end

return Moods

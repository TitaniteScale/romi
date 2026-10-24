-- romi couch mode for Hyprland's Lua config (0.55+). Paste into hyprland.lua, then
-- `hyprctl reload && hyprctl configerrors`.
--
-- romi gets a workspace of its own, named "romi" (status bars show the name). SUPER+SHIFT+G, or a
-- controller's Guide button through `romi guide`, goes there and starts romi if it isn't running.
-- Closing romi takes you back to the workspace you came from.

local couch = "name:romi"

hl.window_rule({
    name = "romi-couch",
    match = { class = "^romi\\.couch$" },
    workspace = couch,
    fullscreen = true,
})

-- The workspace to go back to: the last one you were on that isn't romi's.
local before_couch = nil
hl.on("workspace.active", function()
    local ws = hl.get_active_workspace()
    if ws and ws.name ~= "romi" then
        before_couch = ws.id
    end
end)

-- Global, so `romi guide` can call it with `hyprctl eval 'romi_couch()'`.
function romi_couch()
    hl.dispatch(hl.dsp.focus({ workspace = couch }))
    if #hl.get_windows({ class = "romi.couch" }) == 0 then
        hl.exec_cmd("romi-couch")
    end
end
hl.bind("SUPER + SHIFT + G", romi_couch)

-- When a window closes (checked a moment later, once it's really gone):
--  - a game that went full screen over romi took romi's full screen (one per workspace), so romi
--    gets it back once it's alone again;
--  - closing romi leaves its workspace empty, so go back to where you were instead.
hl.on("window.destroy", function()
    hl.timer(function()
        local romi = hl.get_windows({ class = "romi.couch" })[1]
        if romi and romi.fullscreen == 0 and romi.workspace and romi.workspace.windows == 1 then
            hl.dispatch(hl.dsp.window.fullscreen({ mode = "fullscreen", action = "set", window = romi }))
        end
        local ws = hl.get_active_workspace()
        if not romi and ws and ws.name == "romi" and ws.windows == 0 and before_couch then
            hl.dispatch(hl.dsp.focus({ workspace = before_couch }))
        end
    end, { timeout = 150, type = "oneshot" })
end)

-- The Guide button: `romi guide` watches the controllers (without taking them from romi or your
-- games) and runs this on Guide, unless a game started from romi is running.
hl.on("hyprland.start", function()
    hl.exec_cmd("romi guide -- hyprctl eval 'romi_couch()'")
end)

-- Optional: games open on romi's workspace even if they take a while to start and you've looked
-- elsewhere meanwhile. This applies to these programs however they're started. To find a game's
-- class, run `hyprctl clients -j | grep '"class"'` while it's open.
-- hl.window_rule({
--     name = "games-to-couch",
--     match = { class = "^(retroarch|steam_app_\\d+)$" },
--     workspace = couch,
-- })

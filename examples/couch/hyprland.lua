-- romi couch mode for Hyprland's Lua config (0.55+). Paste into hyprland.lua, then
-- `hyprctl reload && hyprctl configerrors`.

-- romi's couch window gets a workspace of its own.
local couch = 10

hl.window_rule({
    name = "romi-couch",
    match = { class = "^romi\\.couch$" },
    workspace = tostring(couch),
    fullscreen = true,
})

-- SUPER+SHIFT+G, or the controller's Guide button through xremap (see xremap.yml), goes to
-- romi's workspace and starts romi there if it isn't running.
local function couch_mode()
    hl.dispatch(hl.dsp.focus({ workspace = couch }))
    if #hl.get_windows({ class = "romi.couch" }) == 0 then
        hl.exec_cmd("romi-couch")
    end
end
hl.bind("SUPER + SHIFT + G", couch_mode)
hl.bind("F13", couch_mode)

-- Optional: games open on romi's workspace even if they take a while to start and you've looked
-- elsewhere meanwhile. This applies to these programs however they're started. To find a game's
-- class, run `hyprctl clients -j | grep '"class"'` while it's open.
hl.window_rule({
    name = "games-to-couch",
    match = { class = "^(retroarch|steam_app_\\d+)$" },
    workspace = tostring(couch),
})

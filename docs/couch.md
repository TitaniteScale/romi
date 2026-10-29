# Couch mode

romi full screen on the TV, opened from the couch with the controller's Guide button. The pieces
are in [`examples/couch`](../examples/couch):

- **`romi-couch`** starts romi in a Ghostty window of its own: full screen, no decorations, a big
  font (`ROMI_FONT_SIZE`, 18 by default, which leaves room for box art on a 1920-pixel-wide screen),
  and the class `romi.couch` so the compositor can tell it apart. Put it on your PATH:
  `install -m755 examples/couch/romi-couch ~/.local/bin/`.
- **`hyprland.lua`** (Hyprland 0.55+'s Lua config) gives that window a workspace of its own named
  `romi`, which status bars show by name, and binds SUPER+SHIFT+G to go there, starting romi if it
  isn't running. Games started from romi open over it on the same workspace, and focus goes back to
  romi, full screen again, when they exit. Closing romi takes you back to the workspace you came
  from. An optional rule sends slow starters (RetroArch, Steam games) there too, so they don't land
  wherever you looked while they loaded.
- **`romi guide`** makes the controller's Guide button do the same. It watches every controller
  the way romi does, without taking it from romi or your games, and runs a command on Guide:
  `romi guide -- hyprctl eval 'romi_couch()'`, started with your desktop (the example does that).
  While a game started from romi is running, Guide is left to the game (and Steam's overlay). In
  romi itself, Guide still opens the menu.

[Hooks](config.md#hooks) fit in here too: `"on_launch": "playerctl pause"` stops the music while you
play, and `"start_on": "last_played"` picks up where you left off.

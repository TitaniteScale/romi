# Roadmap

Where romi is headed, in the order I plan to build it. Bugs and smaller features are
[issues](https://github.com/TitaniteScale/romi/issues), and how what's here already works, with the
measurements behind it, is in [How it works](docs/how-it-works.md).

## Where things stand

_As of 2026-11-04._ romi runs my whole library: 491 games across 26 systems, covering ROMs, shell scripts,
pinball tables, native PC ports and Steam.

- **Plays in romi:** every emulated system in the library draws in the middle of the launcher, from the Game
  Boy to the PS2, with or without RetroArch installed, with sound, saves, save states, RetroAchievements
  and up to four players.
- **Works with just a controller:** menus, search with an on-screen keyboard, tags, filters, sorting, options
  and logging in to RetroAchievements.
- **The library:** box art in a list or a grid, game info, play time and stats, hiding games, and picking an
  emulator per game.
- **A real CLI:** `list`, `info`, `play`, `status` and `run`, with JSON and stable exit codes, for scripts and
  coding agents. `romi run` boots a game headless with scripted input, states and screenshots.
- **Setup:** `romi init` finds ROMs, emulators and cores, and a first run walks through it.

## What's next

1. **A big refactor, and learning the code.** I want to be able to explain why romi works, not just what it
   does, so this goes through it a part at a time, tidying as it goes.
   [#22](https://github.com/TitaniteScale/romi/issues/22) and [#21](https://github.com/TitaniteScale/romi/issues/21)
   are the known starting points.
2. **Find everything on init.** A fresh `romi init` only finds what ES-DE or romi's list of systems knows. My
   own library's native ports, recompilations, Java games and pinball tables were set up by hand, so a new
   user's wouldn't appear.
3. **Rewind.** Held to go back, built on save states. It should be cheap on 2D systems, and a 3D system's
   states are far bigger, so it may stay with the small ones.
4. **Play anything in the panel.** The last big one: games with no libretro core, like native ports,
   recompilations, Steam, and emulators for the Switch, PS3 and Wii U. The idea is to run a game in a compositor with no
   screen and draw its frames in the panel. Valve's `gamescope` has a headless backend and can stream over PipeWire,
   so romi may only need to read that stream. A first version works: `"mode": "gamescope"` plays Xbox 360 games
   in Xenia Canary in the panel at 60 frames a second, with the game reading the controller itself, and Steam
   games too, through a script in their launch options (tried with Balatro, through Proton). Still to find
   out: the lag, how it does under load, native ports, games that need a keyboard and mouse, and sizes other
   than 720p. If there's a general way to do it, it should be a library of its own.

Along the way, in no fixed order:

- **Input lag in 3D games** ([#11](https://github.com/TitaniteScale/romi/issues/11)): about 24 ms behind RetroArch.
- **Full screen during play**, as a toggle: the game filling the terminal, one button or key.
- **macOS, 2D first.** Ghostty and kitty both run there. Sound and frame timing are written for Linux, and 3D
  goes through EGL, which Macs don't have.
- **More consoles in the launch animation**: a PC, the DS, a landscape handheld, and the biggest TV systems.
- **Vulkan** for the renderers some cores offer, which won't start today.

## Done

- **Groundwork:** CI, and a test harness that drives the real binary in a fake terminal.
- **A CLI other programs can drive,** with safe shared state, JSON and shell completions.
- **`romi init`,** a first run with a welcome, and `romi add`.
- **Managing the library from inside romi:** multi-disc games, sorting, hiding, emulator picks, tags, covers,
  options and hot reload.
- **Couch mode and hooks.**
- **Looks:** the cover grid, title screens and screenshots, stats and achievement progress.
- **Playing in the terminal:** live frames, sound, screen filters, saves, save states, achievements and the launch animation.
- **Systems that need a GPU,** through OpenGL with no window, and core options.
- **Controls:** RetroArch's remaps, remapping in the TUI, and more than one player.
- **More systems in romi:** the DS, GameCube, Wii and PS2.

# romi

A terminal game launcher. Browse ROMs, scripts, native ports and Steam games from one list,
drive it with a keyboard or a controller, and launch from any terminal. It only uses your
terminal's palette, so it matches whatever theme you're running.

**Where it's going:** a scriptable CLI, `romi init` to set itself up, and eventually playing 2D games
inside the TUI. See the [roadmap](ROADMAP.md).

```
romi           # open the launcher
romi init      # find your ROMs and emulators and write a config for them
romi add ~/Games/celeste   # add a native game, script or .desktop launcher
romi check     # what got found, what looks broken, and what could fix it
romi list      # every game as TSV: system, name, key (handy for rofi/fzf scripts)
romi info chrono trigger   # everything romi knows about one game
romi play super metroid    # start it, from a script, a keybind, or another launcher
romi status    # what's running right now, and for how long
romi stats     # play time per system, most-played games, and this year month by month
romi achievements          # refresh your RetroAchievements progress, list games that have them
romi scrape    # find box art on disk, download the rest (or: romi scrape psx gba)
romi guide -- CMD          # run CMD when a controller's Guide button is pressed (couch mode)
```

## Install

On Arch, build the package from a clone. It builds the latest release, and sets up shell
completions and the couch mode examples:

```
git clone https://github.com/TitaniteScale/romi
cd romi/packaging/arch
makepkg -si
```

Anywhere else, from a clone, with Rust installed:

```
cargo install --path . --root ~/.local
```

Then run `romi`. With no config yet, it first does what `romi init` does: it finds your ROM folders,
works out which system each is for, picks an emulator you have installed for each, and shows what
it found before writing anything. No ROMs yet is fine too: it starts with your Steam games and
anything you `romi add`. Rerun `romi init` after installing an emulator or adding a folder, and
`romi check` when something doesn't launch. More in [setting up](docs/setup.md).

## Controls

| Keyboard          | Controller      | Does                                            |
|-------------------|-----------------|-------------------------------------------------|
| `j`/`k`, arrows   | d-pad, stick    | move (hold: speeds up after a second and a half) |
| `h`/`l`           | d-pad ←/→       | switch between systems and games                |
| `enter`           | A               | play (or open a system)                         |
| `esc`             | B               | back                                            |
| `tab`, `[` `]`    | LB / RB         | next / previous system                          |
| `ctrl-u`/`ctrl-d` |                 | page up / down                                  |
|                   | LT / RT         | previous / next letter                          |
| `f`               | Y               | favorite                                        |
| `r`               | X               | random game                                     |
| `d`               |                 | toggle details pane                             |
| `v`               | L3              | cover grid / list                               |
| `S`               | (menu)          | stats                                           |
| `A`               | (menu)          | achievements                                    |
| `/`               | Select          | fuzzy search everything                         |
| `F`               | R3              | filter by genre, players, decade, played, tag   |
| `s`               | (menu)          | sort: name, last played, most played, year      |
| `o`               | Start → game…   | the selected game's menu: favorite, hide, tags, emulator, achievements |
| `m`               | Start / Guide   | menu: the game's menu, search, filter, sort, random, cover grid, details, stats, achievements, show hidden, options, reload, quit |
| `ctrl-r`          |                 | reload config (it reloads by itself when saved) |
| `q`               |                 | quit                                            |

The controller is read directly, since terminals never see gamepad input, and ignored while a game
is running or the terminal isn't focused. [The launcher](docs/tui.md) covers the cover grid,
sorting, filters, tags, hiding games and picking another emulator.

## Couch mode

romi full screen on the TV, opened from the couch with the controller's Guide button: a script that
starts romi in a full-screen Ghostty window, a Hyprland config that gives it a workspace of its own,
and `romi guide` to bring it up from the controller. See [couch mode](docs/couch.md) and
[`examples/couch`](examples/couch).

## More

- [The command line](docs/cli.md): scripting with `list`, `info`, `play` and `status`, JSON, exit
  codes, completions, and running games headless with `romi run`
- [Config](docs/config.md): systems, commands, launch modes, overrides, hooks, BIOS checks,
  multi-disc games. [`default-config.jsonc`](default-config.jsonc) lists every option.
- [Achievements](docs/achievements.md): RetroAchievements and Steam
- [Game info and box art](docs/game-info.md): where descriptions, genres and covers come from
- [Working on romi](CONTRIBUTING.md): tests, releasing, and a map of the code
- [Credits](CREDITS.md): other people's work in this repository, like the 3D models the launch
  animation is made from

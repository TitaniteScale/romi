

https://github.com/user-attachments/assets/043b501c-1511-425e-bf12-7ec91c49a774



https://github.com/user-attachments/assets/3546be21-ad2e-4cfb-91fb-bc42bb2cdb4f



# romi

A game launcher for the terminal. ROMs, scripts, native ports and Steam games in one list, driven
by a keyboard or a controller. Game Boy, GBA, NES, SNES, Genesis and arcade games can also play
right inside it, with sound, saves and RetroAchievements.

<!-- Demo GIF goes here: browsing the cover grid, picking a Game Boy game, the launch animation,
     a few seconds of play with the CRT filter and an achievement unlocking. -->

- **Play in the terminal.** romi loads RetroArch's libretro cores itself and draws the game in the
  middle of the launcher. A wireframe of the console builds itself in your terminal's colors, then
  the camera dives into its screen as the game starts. Scanline, CRT and LCD filters,
  achievements that unlock as you play, and saves shared with RetroArch.
- **One library.** Box art in a list or a cover grid, genres, years and player counts, fuzzy
  search, filters, your own tags, favorites, play time and a stats screen.
- **Controller first.** Everything works from a gamepad, and couch mode opens romi full screen on
  the TV from the controller's Guide button.
- **Sets itself up.** `romi init` finds your ROM folders and the emulators you have installed.
  `romi check` says what's missing or broken and how to fix it.
- **Scriptable.** `list`, `info`, `play` and `status` with JSON output and stable exit codes, for
  rofi, fzf, keybinds or coding agents. `romi run` boots a game headless with scripted input and
  saves a screenshot, the same every time.
- **Matches your theme.** It only uses your terminal's palette.

romi doesn't come with any games: bring your own ROMs, and the emulators to run them.

## Requirements

- **Linux**, x86_64 or aarch64.
- **Any terminal** for the launcher. Box art is drawn with Kitty graphics, Sixel or iTerm2 images,
  or colored half-blocks where there are none.
- **Playing in the terminal** needs Kitty graphics with Unicode placeholders: Ghostty, where it's
  tested, or kitty. It runs libretro cores that draw in software, which covers the 2D systems;
  [systems that need a GPU](ROADMAP.md#7-systems-that-need-a-gpu) are still to come.
  Any other game launches in its emulator as usual.

## Install

**From a release:** download the tarball for your machine from the
[latest release](https://github.com/TitaniteScale/romi/releases/latest), and put `romi` on your
PATH:

```
tar -xzf romi-v*-x86_64-linux.tar.gz
install -Dm755 romi-v*-x86_64-linux/romi ~/.local/bin/romi
```

It needs `libudev` and ALSA's `libasound`, which nearly every desktop already has.
[Shell completions](docs/cli.md#completions) are set up with one line.

**On Arch,** build the package from a clone. It builds the latest release, and sets up shell
completions and the couch mode examples:

```
git clone https://github.com/TitaniteScale/romi
cd romi/packaging/arch
makepkg -si
```

**From source,** with Rust installed, and a C compiler and the headers for udev and ALSA:

```
sudo apt install build-essential pkg-config libudev-dev libasound2-dev        # Debian, Ubuntu
sudo dnf install gcc pkgconf-pkg-config systemd-devel alsa-lib-devel          # Fedora
cargo install --locked --git https://github.com/TitaniteScale/romi --root ~/.local
```

## Getting started

Run `romi`. With no config yet, it first does what `romi init` does: it finds your ROM folders,
works out which system each is for, picks an emulator you have installed for each, and shows what
it found before writing anything. No ROMs yet is fine too: it starts with your Steam games and
anything you `romi add`. Rerun `romi init` after installing an emulator or adding a folder, and
`romi check` when something doesn't launch. More in [setting up](docs/setup.md).

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

To play a game inside romi, open its menu (`o`) and pick **Play in
romi**. [Play in romi](docs/tui.md#play-in-romi) covers the filters, saves and the launch
animation, and [achievements](docs/achievements.md) how to log in to RetroAchievements.

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

- [The launcher](docs/tui.md): the cover grid, sorting, filters, tags, hiding games, picking
  another emulator, and playing in romi
- [The command line](docs/cli.md): scripting with `list`, `info`, `play` and `status`, JSON, exit
  codes, completions, and running games headless with `romi run`
- [Config](docs/config.md): systems, commands, launch modes, overrides, hooks, BIOS checks,
  multi-disc games. [`default-config.jsonc`](default-config.jsonc) lists every option.
- [Achievements](docs/achievements.md): RetroAchievements and Steam
- [Game info and box art](docs/game-info.md): where descriptions, genres and covers come from
- [Roadmap](ROADMAP.md): what's done and what's next
- [Working on romi](CONTRIBUTING.md): tests, releasing, and a map of the code
- [Credits](CREDITS.md): other people's work in this repository, like the 3D models the launch
  animation is made from

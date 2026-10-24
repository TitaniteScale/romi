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
romi scrape    # find box art on disk, download the rest (or: romi scrape psx gba)
romi guide -- CMD          # run CMD when a controller's Guide button is pressed (couch mode)
```

## Command line

`romi list`, `info`, `play` and `status` are meant for scripts as much as for people. Add `--json`
for machine-readable output.

```
romi list --system snes --genre rpg        # filters combine: --system (repeatable), --genre,
romi list --players 2 --decade 90s --json  # --players, --decade, --favorites, --played, --unplayed,
                                           # --tag NAME, --hidden (include hidden games)
romi info "super metroid"
romi info metroid --system gba --json
romi play "super metroid" --dry-run         # the command, cwd and env it would run
romi play hello --mode terminal             # override the configured launch mode
```

**Playing**: `romi play` records history and play time like the TUI does, and exits with the
game's own exit code (128 plus the signal number if it was killed). Without `--mode` it uses the
configured one: `wait` sends the game's output to `last.log`, `terminal` lets it print here, and
`detach` starts it and returns. The game stays in romi's process group, so ^C, or `timeout` in a
script, stops both; romi waits for the game to exit and records the session before it does. A
signal sent to romi alone is passed on to the game. `--json` prints the outcome: `exit_code`,
`played_secs` and the `log` path.

**Detached games** (from `--mode detach`, or a game configured that way) are played by a copy of
romi running in the background, so their play time is recorded even after the TUI or the command
that started them has gone. A launcher that hands off and returns right away, like Steam, still
counts as a few seconds.

**Status**: `romi status` lists the games running right now, however they were started.
`romi status --json` gives `key`, `name`, `system`, `system_name`, `pid` (the game's process),
`started` (Unix time) and `elapsed_secs` for each, which is all a waybar module needs.

**Completions** cover commands, flags, game names, system ids and genres, read from your library as
you type. Set them up once:

```
echo 'COMPLETE=fish romi | source' > ~/.config/fish/completions/romi.fish   # fish
echo 'source <(COMPLETE=bash romi)' >> ~/.bashrc                            # bash
echo 'source <(COMPLETE=zsh romi)' >> ~/.zshrc                              # zsh
```

**Naming a game**: `info` and `play` take a key from `romi list` (`snes:Chrono Trigger (USA).sfc`), an exact
name in any case, or words that all appear in the name (`romi info fusion metroid`). It stops at the
first of those that matches anything, and if that's more than one game it lists them rather than
guessing. `--system` narrows the search.

**JSON**: `list --json` prints an array of games and `info --json` prints one. Each has `key`,
`name`, `system` (the id), `system_name`, `hidden`, `tags` (yours, in name order), `emulator` (the one picked in the game's
menu, or null for the system's default), `path` (null for scripts, ports and Steam; the first disc
of a multi-disc game), `discs` (every disc, in order, for a multi-disc game; otherwise empty), `favorite`,
`playtime_secs`, `last_played` (Unix time or null), `art`, `title_screen` and `screenshot` (paths
or null), and `meta` with
`genres`, `year`, `players`, `developer`, `publisher`, `franchise` and `description`. New fields may
appear; existing ones won't be renamed or removed.

**Exit codes**: romi's own failures use codes games rarely do, so scripts can tell them apart.

| Code | Meaning                                              |
|------|------------------------------------------------------|
| 0    | fine                                                 |
| 1    | anything else that went wrong (the message says what) |
| 64   | bad arguments, or a `--system`/`--genre` that doesn't exist |
| 65   | more than one game matches; the candidates are listed |
| 66   | no game matches                                      |
| 69   | the game's emulator or program isn't installed        |
| 78   | the config can't be read, or a game's command in it is malformed |

`romi play` otherwise exits with whatever the game did.

## Install

```
cargo install --path . --root ~/.local
romi init
```

`romi init` looks for your ROM folders (the one set in ES-DE, `~/ROMs`, `~/roms`,
`~/Emulation/roms`, `~/RetroPie/roms`, or `--roms DIR`) and works out which system each folder is
for, by its name or else by its file types. It then picks an emulator you have installed for each
one. When ES-DE is installed it uses ES-DE's list of about 200 systems and their emulators,
otherwise the systems in romi's default config. It shows what it found and asks before writing:

```
  + gba            Nintendo Game Boy Advance      30 games  mGBA
  + gc             Nintendo GameCube              23 games  Dolphin (Standalone)
  ! xbox360        Microsoft Xbox 360              3 games  no emulator installed (ES-DE knows: xenia)
  ? notes          not a system romi knows
```

init only looks in those places: ES-DE's system files and settings, the folders on your PATH and
the few others ES-DE's find rules name, RetroArch's core folders, and two levels into your ROM
folder to count files. It sends nothing anywhere, and `init.jsonc` records only systems and
commands, not which games you have or how many, so it's safe in a public dotfiles repository.

What it finds goes in `init.jsonc` next to your config, which your config includes. Rerun init
after installing an emulator or adding a folder: it replaces `init.jsonc` and nothing else, and
systems you've set up in `config.json` take priority. `--print` shows the result without writing;
`--yes` skips the question.

`romi check` uses the same knowledge to suggest fixes: a system whose emulator is gone but which
something else installed can run (with the command to use), folders with games that aren't in your
config yet, and folders for systems you have no emulator for.

### Adding a game that isn't a ROM

```
romi add ~/Games/sonic3air_game              # a folder: adds the one program in it
romi add ~/Games/Celeste/Celeste.x86_64 --name Celeste
romi add ~/scripts/update_mods.sh            # scripts get the terminal
romi add ~/.local/share/applications/foo.desktop   # Flatpaks and anything else with a launcher
```

The entry runs the program from its own folder (native ports usually expect that) and is named
after the folder or file; `--name`, `--system` (a system id, or any label for a group of its own)
and `--mode` change that. In a folder with one program and some scripts (setup, build...), the
program is picked; if it's still unclear, romi lists the candidates. It shows the entry and asks
before adding it to `games.jsonc`, which your config includes. `--print` shows it without writing.

## Couch mode

romi full screen on the TV, opened from the couch with the controller's Guide button. The pieces
are in [`examples/couch`](examples/couch):

- **`romi-couch`** starts romi in a Ghostty window of its own: full screen, no decorations, a big
  font (`ROMI_FONT_SIZE`, 18 by default, which leaves room for box art on a 1920-pixel-wide screen),
  and the class `romi.couch` so the compositor can tell it apart. Put it on your PATH: `install -m755 examples/couch/romi-couch ~/.local/bin/`.
- **`hyprland.lua`** (Hyprland 0.55+'s Lua config) gives that window a workspace of its own named
  `romi`, which status bars show by name, and binds SUPER+SHIFT+G to go there, starting romi if it
  isn't running. Games started from romi open over it on the same workspace, and focus goes back to
  romi, full screen again, when they exit. Closing romi takes you back to the workspace you came
  from. An optional rule
  sends slow starters (RetroArch, Steam games) there too, so they don't land wherever you looked
  while they loaded.
- **`romi guide`** makes the controller's Guide button do the same. It watches every controller
  the way romi does, without taking it from romi or your games, and runs a command on Guide:
  `romi guide -- hyprctl eval 'romi_couch()'`, started with your desktop (the example does that).
  While a game started from romi is running, Guide is left to the game (and Steam's overlay). In
  romi itself, Guide still opens the menu.

Hooks fit in here too: `"on_launch": "playerctl pause"` stops the music while you play, and
`"start_on": "last_played"` picks up where you left off.

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
| `/`               | Select          | fuzzy search everything                         |
| `F`               | R3              | filter by genre, players, decade, played, tag   |
| `s`               | (menu)          | sort: name, last played, most played, year      |
| `o`               | Start → game…   | the selected game's menu: favorite, hide, tags, emulator |
| `m`               | Start / Guide   | menu: the game's menu, search, filter, sort, random, cover grid, details, show hidden, reload, quit |
| `ctrl-r`          |                 | reload config                                   |
| `q`               |                 | quit                                            |

**The cover grid** (`v`, L3, or Start → Cover grid) shows the games as box art with their names
underneath, in place of the list and the details pane; the selected game's system, year and genre
are under the grid. The d-pad moves around it, left from the first column goes to the systems as
in the list, and everything else (search, filters, sorting, letter jumps) works the same. Games
without art get a box naming their system. romi remembers which view you left it in.

**Sorting** is remembered per system. Favorites can also go in the order you added them (their
default), and Recent is always newest first. Sorted by year, LT/RT jump between years instead of
letters; sorted by play, they page. From the Start menu, A on Sort keeps trying the next order.
Set `"start_on": "last_played"` to start on the last game you played, in its system, instead of
wherever you left off.

**Hiding** a game (from its menu) takes it out of every list and search. Start → Show hidden games
lists them again, dimmed, so you can unhide them from the same menu. `romi play` still finds
hidden games, and `romi list --hidden` includes them.

**Tags** are your own collections, like "couch co-op" or "backlog". In a game's menu, Tags lists
them with a tick on the game's; A ticks or unticks one, and New tag names another (typed, or with
the on-screen keyboard: ✓ saves). Each tag shows in the systems pane as a collection (`#`), and
the filter has a Tag row, so "backlog + 2 players" works too. A tag goes when its last game
leaves it. Tags are kept in `state.json` with your favorites; `romi list --tag backlog` lists one.

**Emulators**: when a system's games can run in something else you have installed (ParaLLEl N64
next to Mupen64Plus-Next, mGBA's standalone app next to its core), the game's menu has an
Emulator entry; A cycles through them. The others come from ES-DE's list for the system, limited
to what's installed, plus any listed under the system's `"emulators"`. The pick is saved per game
in `overrides.jsonc`, with its command, and `romi info` shows a game's emulator and the others.

Searching with a controller brings up an on-screen keyboard: the d-pad picks a key, A types, X
deletes, Y adds a space, and B (or ✓) closes it, leaving the results to browse. B again leaves the
search.

The controller is read directly (via `gilrs`), since terminals never see gamepad input. romi
ignores it while a game is running and while the terminal window isn't focused, so playing a
detached Steam game won't scroll the list behind it.

## Config

`~/.config/romi/config.json` (or `$ROMI_CONFIG`). Comments are allowed. `romi init` writes one for
you; otherwise a default is written on first run. See [`default-config.jsonc`](default-config.jsonc)
for every option.

`"include": ["other.jsonc"]` adds the systems, games, vars and BIOS folders of other files
(relative to the config's folder). Anything the main config defines itself wins.

```jsonc
{
  "roms_dir": "~/ROMs",
  "gamelists_dir": "~/ES-DE/gamelists",            // optional: ES-DE names + descriptions
  "arcade_db": "/usr/share/es-de/resources/MAME",  // optional: MAME titles, hides BIOS zips
  "vars": { "cores": "~/.config/retroarch/cores" },

  "systems": [
    {
      "id": "psx", "name": "PlayStation",
      "dir": "psx",                                 // relative to roms_dir; defaults to id
      "extensions": ["chd", "cue", "m3u"],
      "exclude": ["scph*"],
      "command": "retroarch -L {cores}/mednafen_psx_hw_libretro.so {rom}",
      "bios": [["scph5501.bin", "scph5500.bin", "scph5502.bin"]],  // any one of these
      "overrides": {
        "Some Game (USA).chd": { "command": "duckstation-qt -batch {rom}" },
        "Demo Disc (USA).chd": { "hidden": true }
      }
    }
  ],

  "games": [
    { "name": "Sonic 3 A.I.R.", "command": "./sonic3air_linux", "cwd": "~/Games/sonic3air_game" }
  ],

  "steam": { "enabled": true, "exclude": ["Tabletop Simulator"] }
}
```

**Other emulators** for a system, for the game menu to offer alongside the ones ES-DE knows:

```jsonc
"emulators": [
  { "name": "DuckStation", "command": "duckstation-qt -batch {rom}" },
  { "name": "Standalone mGBA", "command": "mgba-qt -f {rom}", "mode": "wait" }
]
```

**Overrides by game key**: the top-level `"overrides"` change any game, Steam and hand-added ones
included, using its key from `romi list`. They take the same fields as a system's overrides and beat
everything else. romi's menus write theirs (hidden games and emulator picks) to `overrides.jsonc`
next to the config, adding it to `"include"` the first time. romi rewrites that file whole, so keep
your own overrides in config.json: for a game in both, yours win field by field, and an emulator
pick (`emulator` with its `command`, `mode` and `cwd`) only applies if you haven't set a `command`. Unlike `init.jsonc` it names your games, so think twice before putting it
in a public dotfiles repository.

```jsonc
"overrides": {
  "steam:1145360": { "hidden": true },
  "game:Sonic 3 A.I.R.": { "genre": "Platform", "year": 2022 }
}
```

**Hooks** run commands when a game starts and stops: pause music, switch power profiles, move to
a workspace. Set them at the top level for every game, or on a system for its games:

```jsonc
"hooks": {
  "on_launch": ["playerctl pause", "powerprofilesctl set performance"],
  "on_exit": ["powerprofilesctl set balanced", "playerctl play"]
},
"systems": [
  { "id": "ps2", ..., "hooks": { "on_launch": "hyprctl dispatch workspace 5", "on_exit": "hyprctl dispatch workspace 1" } }
]
```

`on_launch` hooks run before the game starts, global ones first, and romi waits for each (up to 10
seconds) so a workspace switch happens before the game's window opens. `on_exit` hooks run after
it exits, the system's first, and nothing waits for them except `romi play`, which finishes them
before it exits. A hook that fails or isn't installed never stops the game: the TUI or `romi play`
says so, and `~/.local/state/romi/hooks.log` has what happened and the hooks' output (each launch
starts it afresh). Hooks run for every launch mode, detached games included, and `romi play
--dry-run` lists them.

Hooks take a command or a list of them, split like launch commands, with the same placeholders
plus `{game}` (the name), `{key}` and `{system}` (the id), and on exit `{exit_code}` (128 plus the
signal number if the game was killed) and `{played_secs}`. `{rom}`, `{file}`, `{name}`, `{dir}` and
`{appid}` are empty for games that don't have them. The same values are in the environment as
`ROMI_GAME`, `ROMI_KEY`, `ROMI_SYSTEM`, `ROMI_SYSTEM_NAME`, `ROMI_ROM`, and on exit
`ROMI_EXIT_CODE` and `ROMI_PLAYED_SECS`, with `ROMI_EVENT` set to `launch` or `exit`. Inside
`sh -c '...'`, use those: a name like "Advance Wars (USA)" pasted into a shell script trips on the
parentheses.

**BIOS checks**: list a system's required files under `"bios"`, either one file or a list where any
one will do. Relative names are looked up in `bios_dirs` (default `~/.config/retroarch/system`) and
in the system's ROM folder. Absolute paths and `*` wildcards also work, e.g.
`"~/.config/PCSX2/bios/*.bin"`. Missing ones show as `!` in `romi check` and in the TUI.

**Placeholders**: `{rom}` `{file}` `{name}` `{dir}` for ROMs, `{appid}` for Steam, plus anything in
`vars`. Commands are split like a shell would (quotes work) and placeholders are filled in per
argument, so paths with spaces need no extra quoting. Need pipes or `&&`? Use `sh -c '...'`.

**Launch modes** (`"mode"` on a system, override or game):
- `wait` (default) shows a "now playing" screen until the game exits. Output goes to
  `~/.local/state/romi/last.log`.
- `terminal` hands the terminal to the command, for scripts and TUI games.
- `detach` starts it and moves on. Used for Steam, whose launcher returns immediately.

**Display names**, in priority order: override `name`, then the MAME title (for `"arcade": true`
systems), then the ES-DE gamelist name, then the file name with region tags stripped
(`King of Fighters 2002, The (Japan)` → `The King of Fighters 2002`).

**Multi-disc games** show as one entry: "Final Fantasy VII (USA) (Disc 1)" to "(Disc 3)" become
"Final Fantasy VII", launched through an `.m3u` playlist so the emulator can swap discs. That
happens for systems whose `extensions` include `m3u` (a sign the emulator takes playlists), and
for discs with the same name, folder and file type. romi writes the playlist to
`~/.cache/romi/m3u/<system>/` and never touches your ROM folders; `{rom}` is the playlist and the
game's key names it (`psx:Final Fantasy VII (USA).m3u`). Favorites, play time and art from before
the discs were merged carry over, and the old disc keys still work with `info` and `play`. An
override for the game goes under the playlist's name or disc 1's. If a folder already has its own
`.m3u`, the discs it lists are left to it. Note that RetroArch names save files after what it
launched, so saves made by booting a disc directly are under that disc's name.

State (favorites, recents, play time, last selection, sort orders, tags) lives in `~/.local/state/romi/state.json`.

## Game info and filters

The details pane shows genre, developer, publisher, year, players and a description, merged from
(best first): your config (`"genre"`, `"players"`, `"year"` on a game or override), ES-DE's
gamelists, Skyscraper's cache, and RetroArch's game databases (`~/.config/retroarch/database/rdb`).
RetroArch's databases are matched by ROM checksum where possible (zips are read from their
headers; other cartridges are hashed once and cached in `~/.cache/romi/crc.tsv`), then by file
name, then by title. An exact match also gives box art lookups the precise name to search for.
Arcade genres come from a MAME category list (`metadata.arcade_categories`, e.g. a `catver.ini`
from [progettosnaps](https://www.progettosnaps.net/catver/)). Genres from all these sources are
folded into one short list ("Shoot'em Up / Horizontal" and "Shmup" both become "Shoot 'em up").
The TUI starts instantly and fills database info in a moment later; `romi check` reports coverage.

The filter (Start → Filter, `F`, or R3) narrows every view at once by genre, minimum players,
decade, played/not yet, and your tags. Systems with no matches hide while it's on, so "Fighting + 2+ players"
becomes one cross-system list. Up/down picks a row, left/right changes it, and the list updates
as you go.

## Box art

`romi scrape` reports which games have art and downloads what it can. It's safe to rerun: only new
matches are fetched. romi looks in this order, using local files where they are:

1. `"art": "/path/to/image.png"` on a game entry or a system override
2. romi's cache, `~/.cache/romi/art/<system>/`
3. ES-DE's `downloaded_media` (covers, then 3D boxes); set `art.esde_media_dir`
4. Skyscraper's cache (`art.skyscraper_cache`), matched by ROM file name
5. Steam's library cache

Online sources need no accounts or keys:

- **[libretro thumbnails](https://thumbnails.libretro.com)**, RetroArch's box art. romi downloads
  each set's file list once a week (cached in `~/.cache/romi/libretro/`) and matches titles
  locally. It tries the exact name first, then the same words ignoring region tags, then the title
  plus a subtitle, then ignoring dashes. US/World releases win over others, and demos/bootlegs come
  last. Arcade systems match on the MAME title. A system's sets come from its id; override with
  `"libretro": ["Set Name", ...]` on the system.
- **Steam's CDN** for installed Steam games.

**Title screens and screenshots** come from the same places: libretro's `Named_Titles` and
`Named_Snaps` (matched like the covers), ES-DE's `titlescreens` and `screenshots`, and Skyscraper's
`screenshots`, and `romi scrape` gets them along with the covers, kept in `titles/` and `snaps/`
under each system's cache folder. The details pane shows them beside the cover when it's wide
enough, taking turns every five seconds; when it isn't, the cover, title screen and screenshot
take turns in its place, starting from the cover each time you move to another game. `romi info` gives their paths (`title_screen` and `screenshot`
in JSON). Set `"art": { "screens": false }` to stick to covers.

Covers show at the top of the details pane (and on the "now playing" screen). romi asks the
terminal what it supports at startup: Kitty graphics in Ghostty/kitty, Sixel or iTerm2 elsewhere,
colored half-blocks as a last resort. Images are decoded on background threads, so holding the
d-pad through a long list never stutters. Turn covers off with `"art": { "show": false }`.

Set `"art": { "online": false }` to only use what's already on disk. The art belongs to its
publishers: romi fetches and caches it on your machine and never ships any.

## Development

```
cargo test                                    # unit tests
python -m venv .venv && .venv/bin/pip install -r e2e/requirements.txt
.venv/bin/pytest e2e                          # the real binary, driven in a fake terminal
.venv/bin/python e2e/drive.py 100 30 keys:j shot   # print what the screen looks like
```

The end-to-end tests run romi in a pty with a small made-up library in a temporary home folder, so
your own config and state are never touched (`drive.py --real` uses yours). They type keys, press
buttons on a virtual gamepad, and read the screen with [pyte](https://github.com/selectel/pyte). The
controller tests need write access to `/dev/uinput` and are skipped without it. CI runs rustfmt,
clippy, the unit tests and the end-to-end tests on every push.

## Code map

| File              | What's in it                                                     |
|-------------------|------------------------------------------------------------------|
| `src/config.rs`   | config structs, JSONC loading, `~` expansion, globbing           |
| `src/library.rs`  | scanning ROM dirs, ES-DE gamelists, MAME names, name cleanup     |
| `src/steam.rs`    | installed Steam games from `libraryfolders.vdf` + app manifests  |
| `src/launch.rs`   | placeholder expansion, spawning, sanity checks for `romi check`  |
| `src/gamepad.rs`  | controller thread: buttons → actions, hold-to-repeat             |
| `src/app.rs`      | app state; keyboard and controller both become the same `Op`s    |
| `src/ui.rs`       | ratatui drawing                                                  |
| `src/state.rs`    | favorites, history, play time                                    |
| `src/art.rs`      | box art lookup and `romi scrape` (libretro thumbnails, Steam CDN) |
| `src/cover.rs`    | showing covers: decoding and resizing on worker threads, a render cache |
| `src/meta.rs`     | game info: RetroArch databases, Skyscraper, checksums, genres    |
| `src/filter.rs`   | filtering by genre, players, decade, played, tag                 |
| `src/sort.rs`     | sort orders for systems and favorites                            |
| `src/overrides.rs`| saving changes made in the menus to `overrides.jsonc`            |
| `src/emulators.rs`| the other emulators a game can run with, for its menu and `info` |
| `src/titles.rs`   | matching titles against database names (art and metadata share it) |
| `src/init.rs`     | `romi init`: ES-DE's system list, finding emulators, writing init.jsonc |
| `src/add.rs`      | `romi add`: guessing a game entry from a path, appending to games.jsonc |
| `src/cli.rs`      | the command line: arguments, commands, completions, exit codes   |
| `src/session.rs`  | recording a game's start and end, running games, background play |
| `src/hooks.rs`    | `on_launch` and `on_exit` hooks                                  |
| `src/main.rs`     | event loop, handing the terminal to `terminal`-mode commands     |
| `e2e/drive.py`    | runs romi in a fake terminal with a virtual gamepad (tests and by hand) |
| `e2e/test_*.py`   | end-to-end tests of the TUI and the CLI                          |

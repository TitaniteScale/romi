# romi

A terminal game launcher. Browse ROMs, scripts, native ports and Steam games from one list,
drive it with a keyboard or a controller, and launch from any terminal. It only uses your
terminal's palette, so it matches whatever theme you're running.

**Where it's going:** a scriptable CLI, `romi init` to set itself up, and eventually playing 2D games
inside the TUI. See the [roadmap](ROADMAP.md).

```
romi            # open the launcher
romi --check    # what got found, and what looks broken (missing emulators/cores)
romi --list     # every game as TSV: system, name, key (handy for rofi/fzf scripts)
romi --scrape   # find box art on disk, download the rest (or: romi --scrape psx gba)
```

## Install

```
cargo install --path . --root ~/.local
```

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
| `/`               | Select          | fuzzy search everything                         |
| `F`               | R3              | filter by genre, players, decade, played        |
| `m`               | Start / Guide   | menu: search, filter, random, details, reload, quit |
| `ctrl-r`          |                 | reload config                                   |
| `q`               |                 | quit                                            |

Searching with a controller brings up an on-screen keyboard: the d-pad picks a key, A types, X
deletes, Y adds a space, and B (or ✓) closes it, leaving the results to browse. B again leaves the
search.

The controller is read directly (via `gilrs`), since terminals never see gamepad input. romi
ignores it while a game is running and while the terminal window isn't focused, so playing a
detached Steam game won't scroll the list behind it.

## Config

`~/.config/romi/config.json` (or `$ROMI_CONFIG`). Comments are allowed. A default is written on first
run; see [`default-config.jsonc`](default-config.jsonc) for every option.

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

**BIOS checks**: list a system's required files under `"bios"`, either one file or a list where any
one will do. Relative names are looked up in `bios_dirs` (default `~/.config/retroarch/system`) and
in the system's ROM folder. Absolute paths and `*` wildcards also work, e.g.
`"~/.config/PCSX2/bios/*.bin"`. Missing ones show as `!` in `romi --check` and in the TUI.

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

State (favorites, recents, play time, last selection) lives in `~/.local/state/romi/state.json`.

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
The TUI starts instantly and fills database info in a moment later; `romi --check` reports coverage.

The filter (Start → Filter, `F`, or R3) narrows every view at once by genre, minimum players,
decade, and played/not yet. Systems with no matches hide while it's on, so "Fighting + 2+ players"
becomes one cross-system list. Up/down picks a row, left/right changes it, and the list updates
as you go.

## Box art

`romi --scrape` reports which games have art and downloads what it can. It's safe to rerun: only new
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

Covers show at the top of the details pane (and on the "now playing" screen). romi asks the
terminal what it supports at startup: Kitty graphics in Ghostty/kitty, Sixel or iTerm2 elsewhere,
colored half-blocks as a last resort. Images are decoded on a background thread, so holding the
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
| `src/launch.rs`   | placeholder expansion, spawning, sanity checks for `--check`     |
| `src/gamepad.rs`  | controller thread: buttons → actions, hold-to-repeat             |
| `src/app.rs`      | app state; keyboard and controller both become the same `Op`s    |
| `src/ui.rs`       | ratatui drawing                                                  |
| `src/state.rs`    | favorites, history, play time                                    |
| `src/art.rs`      | box art lookup and `--scrape` (libretro thumbnails, Steam CDN)   |
| `src/cover.rs`    | showing covers: background decode/resize, small render cache     |
| `src/meta.rs`     | game info: RetroArch databases, Skyscraper, checksums, genres    |
| `src/filter.rs`   | filtering by genre, players, decade, played                      |
| `src/titles.rs`   | matching titles against database names (art and metadata share it) |
| `src/main.rs`     | CLI, event loop, handing the terminal to `terminal`-mode commands |
| `e2e/drive.py`    | runs romi in a fake terminal with a virtual gamepad (tests and by hand) |
| `e2e/test_tui.py` | end-to-end tests                                                 |

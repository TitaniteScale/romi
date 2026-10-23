# romi

A terminal game launcher. Browse ROMs, scripts, native ports and Steam games from one list,
drive it with a keyboard or a controller, and launch from any terminal. It only uses your
terminal's palette, so it matches whatever theme you're running.

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

| Keyboard          | Controller      | Does                                  |
|-------------------|-----------------|---------------------------------------|
| `j`/`k`, arrows   | d-pad, stick    | move                                  |
| `h`/`l`           | d-pad ←/→       | switch between systems and games      |
| `enter`           | A / Start       | play (or open a system)               |
| `esc`             | B               | back                                  |
| `tab`, `[` `]`    | LB / RB         | next / previous system                |
| `ctrl-u`/`ctrl-d` | LT / RT         | page up / down                        |
| `f`               | Y               | favorite                              |
| `r`               | X               | random game                           |
| `d`               | Select          | toggle details pane                   |
| `/`               |                 | fuzzy search everything               |
| `ctrl-r`          |                 | reload config                         |
| `q`               |                 | quit                                  |

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

Set `"art": { "online": false }` to only use what's already on disk. The art belongs to its
publishers: romi fetches and caches it on your machine and never ships any.

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
| `src/main.rs`     | CLI, event loop, handing the terminal to `terminal`-mode commands |

## Ideas for later

- Show box art in the TUI via the kitty graphics protocol (Ghostty supports it; `ratatui-image`)
- Match ROMs by CRC against RetroArch's `.rdb` databases for titles that names can't match
- SteamGridDB (API key) for PC entries and Steam games with hashed CDN paths
- Collections / custom lists, hiding games from inside the UI
- Multiple commands per system to pick from (alternate emulators)
- Per-game play-time sorting, "continue playing" on start

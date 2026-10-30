# Config

`~/.config/romi/config.json` (or `$ROMI_CONFIG`). Comments are allowed. `romi init` (or the
first `romi`) writes one for you; other commands without one say to run it. See
[`default-config.jsonc`](../default-config.jsonc) for every option.

`"include": ["other.jsonc"]` adds the systems, games, vars and BIOS folders of other files
(relative to the config's folder). Anything the main config defines itself wins.

The TUI reloads the config by itself when it or a file it includes changes, a moment after the
last save, so there's no need to restart romi. While a game is running it waits until the game
stops. A config that doesn't parse is reported once, and romi carries on with the one it had.
`ctrl-r` reloads by hand.

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

State (favorites, recents, play time, last selection, sort orders, tags) lives in
`~/.local/state/romi/state.json`.

## Commands

**Placeholders**: `{rom}` `{file}` `{name}` `{dir}` for ROMs, `{appid}` for Steam, plus anything in
`vars`. Commands are split like a shell would (quotes work) and placeholders are filled in per
argument, so paths with spaces need no extra quoting. Need pipes or `&&`? Use `sh -c '...'`.

**Launch modes** (`"mode"` on a system, override or game):
- `wait` (default) shows a "now playing" screen until the game exits. Output goes to
  `~/.local/state/romi/last.log`.
- `terminal` hands the terminal to the command, for scripts and TUI games.
- `detach` starts it and moves on. Used for Steam, whose launcher returns immediately.

## Other emulators

Other emulators for a system, for the game menu to offer alongside the ones ES-DE knows:

```jsonc
"emulators": [
  { "name": "DuckStation", "command": "duckstation-qt -batch {rom}" },
  { "name": "Standalone mGBA", "command": "mgba-qt -f {rom}", "mode": "wait" }
]
```

## Overrides by game key

The top-level `"overrides"` change any game, Steam and hand-added ones included, using its key from
`romi list`. They take the same fields as a system's overrides and beat everything else. romi's
menus write theirs (hidden games and emulator picks) to `overrides.jsonc` next to the config,
adding it to `"include"` the first time. romi rewrites that file whole, so keep your own overrides
in config.json: for a game in both, yours win field by field, and an emulator pick (`emulator` with
its `command`, `mode` and `cwd`) only applies if you haven't set a `command`. Unlike `init.jsonc`
it names your games, so think twice before putting it in a public dotfiles repository.

```jsonc
"overrides": {
  "steam:1145360": { "hidden": true },
  "game:Sonic 3 A.I.R.": { "genre": "Platform", "year": 2022 }
}
```

## Hooks

Hooks run commands when a game starts and stops: pause music, switch power profiles, move to a
workspace. Set them at the top level for every game, or on a system for its games:

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

## BIOS checks

List a system's required files under `"bios"`, either one file or a list where any one will do.
Relative names are looked up in `bios_dirs` (default `~/.config/retroarch/system`) and in the
system's ROM folder. Absolute paths and `*` wildcards also work, e.g.
`"~/.config/PCSX2/bios/*.bin"`. Missing ones show as `!` in `romi check` and in the TUI.

## Names

**Display names**, in priority order: override `name`, then the MAME title (for `"arcade": true`
systems), then the ES-DE gamelist name, then the file name with region tags stripped
(`King of Fighters 2002, The (Japan)` → `The King of Fighters 2002`).

**Regional names**: some systems are called different things in different places (Genesis or Mega
Drive, Sega CD or Mega-CD, TurboGrafx-16 or PC Engine, Super Nintendo or Super Famicom, NES or
Famicom). A system with one of those ids and no `"name"` is named for `"region"`: `"na"`, `"eu"`
(Europe and most of the world), `"jp"`, or `"auto"` (the default), which goes by your locale
(`en_US` is North America). `romi init` leaves these systems unnamed; give one a `"name"` to fix it.

## Multi-disc games

Multi-disc games show as one entry: "Final Fantasy VII (USA) (Disc 1)" to "(Disc 3)" become "Final
Fantasy VII", launched through an `.m3u` playlist so the emulator can swap discs. That happens for
systems whose `extensions` include `m3u` (a sign the emulator takes playlists), and for discs with
the same name, folder and file type. romi writes the playlist to `~/.cache/romi/m3u/<system>/` and
never touches your ROM folders; `{rom}` is the playlist and the game's key names it
(`psx:Final Fantasy VII (USA).m3u`). Favorites, play time and art from before the discs were merged
carry over, and the old disc keys still work with `info` and `play`. An override for the game goes
under the playlist's name or disc 1's. If a folder already has its own `.m3u`, the discs it lists
are left to it. Note that RetroArch names save files after what it launched, so saves made by
booting a disc directly are under that disc's name.

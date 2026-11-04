# The command line

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

## Playing

`romi play` records history and play time like the TUI does, and exits with the game's own exit
code (128 plus the signal number if it was killed). Without `--mode` it uses the configured one:
`wait` sends the game's output to `last.log`, `terminal` lets it print here, and `detach` starts it
and returns. The game stays in romi's process group, so ^C, or `timeout` in a script, stops both;
romi waits for the game to exit and records the session before it does. A signal sent to romi
alone is passed on to the game. `--json` prints the outcome: `exit_code`, `played_secs` and the
`log` path.

**Detached games** (from `--mode detach`, or a game configured that way) are played by a copy of
romi running in the background, so their play time is recorded even after the TUI or the command
that started them has gone. A launcher that hands off and returns right away, like Steam, still
counts as a few seconds.

## Running a game headless

`romi run` plays a game inside romi, with no window and no RetroArch, for a set number of frames
(600 by default, 10 seconds of most consoles) and saves the last frame with `--screenshot out.png`.
It loads the libretro core from the game's `retroarch -L` command (for a game in the library, from
another of its emulators if its own isn't RetroArch), or the one given with `--core`, and takes a
game name or a ROM file, so it works on a build fresh out of a ROM hack or decomp. A ROM file belongs to
the system whose ROM folder it's in, or a folder named after a system, or the only system that takes
its extension. If several could take it, as with `.chd`, romi lists them (exit 65) and `--system` says
which.
`--input` holds buttons from a script of `<frame> <buttons...>` lines, each held until the next
line (`300 start`, then `306` to let go). Buttons are player 1's, or another player's after `p2:`
(up to `p4:`), as in `300 start p2:a`. `touch:XxY` touches the screen at pixel X, Y of the frame,
from its top left, as the mouse does playing: `500 touch:128x272` taps a DS game's lower screen
(it's the bottom half of the 256×384 frame). The buttons are the pad's, so the core sees them through the
game's remap, as it would playing. The same ROM, script and frame count always give the same
picture, so screenshots can be compared between builds. Screenshots are the core's pixels as they
are; `--json` gives `pixel_aspect`, how wide each is meant to look (under 1 for arcade boards that
squeeze a wide picture onto a 4:3 screen), and playing shows games at that shape. `--wav out.wav`
saves the sound of the whole run, which comes out the same every time too. Only cores that draw in
software work (most 2D systems, and arcade games through FinalBurn Neo). Zipped ROMs go as they are
to cores that open zips themselves, like FinalBurn Neo, and are unpacked to /tmp for the rest, as
RetroArch does; 7z files only work with cores that open them. Cores get the first of `bios_dirs` as
their system folder, and each run gets an empty save folder of its own, deleted afterwards, so
nothing saved from one run changes the next.

```
romi run "pokemon pinball" --frames 1200 --input start.txt --screenshot title.png
romi run build/game.gba --core /usr/lib/libretro/mgba_libretro.so --screenshot out.png --json
romi run "pokemon pinball" --frames 3600 --wav title-music.wav
romi run "metal slug" --frames 1800 --screenshot attract.png
romi run "pokemon pinball" --frames 1200 --input start.txt --save-state table.state
romi run "pokemon pinball" --load-state table.state --frames 60 --screenshot table.png
```

`--save-state s.state` saves the game's state after the last frame, and `--load-state s.state` starts
from one, so a run can skip to the part it's testing. States are in RetroArch's format, and RetroArch's
are read, compressed or not. They belong to the core that made them, and often to its version. Add
`--screenshot` and the picture is the same as a run that never stopped, for the cores that repeat
exactly. A state is loaded 120 frames in, which is too soon for ProSystem (Atari 7800, back to its boot
logo) and PPSSPP (a black picture); both load states fine while playing. `"save_state_bytes"` and the
time each took are in `--json`. With `--achievements`, states carry how
far along the achievements are, too.

`--achievements` earns RetroAchievements during the run, as playing in romi does, after
`romi achievements login`: it waits for the game's achievements to load, then checks them after
every frame, and says what it unlocked (`"achievements"` in `--json`: the game, what was unlocked,
and anything that went wrong). Unlocks are sent to your account, or to `ROMI_RA_HOST`, so a run
that scripts the buttons to earn one makes a regression test for a ROM hack. Without the flag a
run never touches the account.

## Playing in the terminal

`romi run <game> --live` plays it in the terminal instead, drawn with the Kitty graphics protocol.
It's an experiment on the way to playing inside the TUI: F1 switches how frames reach the terminal,
F2 whether the terminal or romi scales them, F4 mutes the sound, and the bottom line shows frame
rates, send times and how far ahead the sound is queued. Keys follow RetroArch's defaults (arrows,
X/Z for A/B, A/S for Y/X, Q/W for L/R, Enter for Start, Backspace for Select); a controller works
too. Esc, or holding Select+Start, quits.

In the TUI, a game's menu has **Play in romi**, described in [the launcher](tui.md#play-in-romi).

## Status

`romi status` lists the games running right now, however they were started. `romi status --json`
gives `key`, `name`, `system`, `system_name`, `pid` (the game's process), `started` (Unix time) and
`elapsed_secs` for each, which is all a waybar module needs.

## Completions

Completions cover commands, flags, game names, system ids and genres, read from your library as
you type. The Arch package sets them up for bash, zsh and fish; otherwise, set them up once:

```
echo 'COMPLETE=fish romi | source' > ~/.config/fish/completions/romi.fish   # fish
echo 'source <(COMPLETE=bash romi)' >> ~/.bashrc                            # bash
echo 'source <(COMPLETE=zsh romi)' >> ~/.zshrc                              # zsh
```

## Naming a game

`info` and `play` take a key from `romi list` (`snes:Chrono Trigger (USA).sfc`), an exact name in
any case, or words that all appear in the name (`romi info fusion metroid`). It stops at the first
of those that matches anything, and if that's more than one game it lists them rather than
guessing. `--system` narrows the search.

## JSON

`list --json` prints an array of games and `info --json` prints one. Each has:

- `key`, `name`, `system` (the id), `system_name`, `hidden`, `favorite`
- `tags`: yours, in name order
- `achievements`: RetroAchievements' `id`, `title`, `total`, `points`, your `earned` and
  `earned_hardcore`, your best `award`, and whether your dump is `supported`; null when there's
  nothing
- `emulator`: the one picked in the game's menu, or null for the system's default
- `path`: null for scripts, ports and Steam; the first disc of a multi-disc game
- `discs`: every disc, in order, for a multi-disc game; otherwise empty
- `playtime_secs`, `last_played` (Unix time or null)
- `art`, `title_screen` and `screenshot`: paths or null
- `meta`: `genres`, `year`, `players`, `developer`, `publisher`, `franchise` and `description`

New fields may appear; existing ones won't be renamed or removed.

## Exit codes

romi's own failures use codes games rarely do, so scripts can tell them apart.

| Code | Meaning                                                          |
|------|------------------------------------------------------------------|
| 0    | fine                                                             |
| 1    | anything else that went wrong (the message says what)            |
| 64   | bad arguments, or a `--system`/`--genre` that doesn't exist      |
| 65   | more than one game matches; the candidates are listed            |
| 66   | no game matches                                                  |
| 69   | the game's emulator or program isn't installed                   |
| 78   | the config can't be read, or a game's command in it is malformed |

`romi play` otherwise exits with whatever the game did.

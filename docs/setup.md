# Setting up

## romi init

With no config yet, `romi` first does what `romi init` does, asks before writing anything, then
opens. No ROMs yet is fine too: it starts with your Steam games and anything you `romi add`.

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
systems you've set up in `config.json` take priority. The one exception: if your config's
`roms_dir` no longer exists, init looks for your ROMs and points `roms_dir` at them. ROMs given
with `--roms` somewhere other than `roms_dir` get full paths instead, leaving `roms_dir` as it is.
`--print` shows the result without writing; `--yes` skips the question.

## romi check

`romi check` uses the same knowledge to suggest fixes: a system whose emulator is gone but which
something else installed can run (with the command to use), folders with games that aren't in your
config yet, and folders for systems you have no emulator for.

It also reads the settings of the emulators your games use (RetroArch, PCSX2, Dolphin and melonDS,
installed or as Flatpaks) and says what would get in the way: RetroAchievements turned off, not
logged in, or logged in as someone other than the username in your config (when it's set), and no
controller bound (when romi's controller support is on). It only reads those files; the fix is
in the emulator, and `romi check` says where:

```
emulators
  ✓ RetroArch  RetroAchievements is on, logged in as you
  ! Dolphin    GameCube port 1 uses the keyboard, not a controller (Controllers → Port 1 → Configure)
  ! melonDS    no controller buttons are bound (Config → Input and hotkeys)
```

## Adding a game that isn't a ROM

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

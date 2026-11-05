# Setting up

## romi init

With no config yet, `romi` first does what `romi init` does, asks before writing anything, then
opens. No ROMs yet is fine too: it starts with your Steam games and anything you `romi add`.

`romi init` looks for your ROM folders (the one set in ES-DE, `~/ROMs`, `~/roms`,
`~/Emulation/roms`, `~/RetroPie/roms`, or `--roms DIR`) and works out which system each folder is
for, by its name or else by its file types. It then picks an emulator you have installed for each
one. A libretro core counts on its own, with or without RetroArch, since romi plays cores itself;
without RetroArch, such a game can only play in romi. When ES-DE is installed it uses ES-DE's list
of about 200 systems and their emulators, otherwise the systems in romi's default config. It shows
what it found and asks before writing:

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

## The first run

With no config, `romi` opens on its title screen. Then it asks where your ROMs are, with the folder
filled in if it found one, and, if RetroArch is here, whether to share its folders or keep to romi's own.
It shows what it found before writing anything. Saying yes does what
`romi init` does, and the launcher opens. Esc goes back a screen, and from the title screen it leaves
with nothing written. A controller works too, though it can only take the folder offered, not type one.

You won't see the title screen again unless you want to: Options → Title screen at start, or
`"title_screen": true`, shows it each time romi opens, until something's pressed.

## Where things go

romi needs somewhere for BIOS files, in-game saves and downloaded cores. When init makes a new config,
it decides once, and writes the answer in the config, so installing RetroArch later doesn't move them.
A first run in a terminal asks, when RetroArch is here. `romi init --yes` goes by this:

| | RetroArch installed, or its folder there | Neither |
|---|---|---|
| BIOS files, and what cores keep there | `~/.config/retroarch/system`, or the one in its settings | `~/.local/share/romi/system` |
| In-game saves | RetroArch's save folder, shared with it | `~/.local/share/romi/saves/<core>` |
| Downloaded cores | `~/.config/retroarch/cores` | `~/.local/share/romi/cores` |

A Flatpak RetroArch counts, and has the same folders under
`~/.var/app/org.libretro.RetroArch/config/retroarch`, unless there's also a RetroArch on the machine
itself, which wins. Save states always go in `~/.local/share/romi/states`, so romi never writes over RetroArch's. Cores
installed by your distro are found wherever they are, and `~/.local/share/romi/cores` is always looked
in first. A config that's already there is never changed: to move yours, set `"bios_dirs"` and
`"saves_dir"` and move the files. A core gets one system folder, the first of `"bios_dirs"` that
exists, so BIOS files in another folder won't reach it.

## romi check

`romi check` says which terminal it's in and whether games can show in it (Ghostty and kitty), how
many libretro cores it found and where, or everywhere it looked if it found none, the folders it uses
for BIOS files, saves and states, and whether LRPS2 is missing its BIOS.

It uses the same knowledge as init to suggest fixes: a system whose emulator is gone but which
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

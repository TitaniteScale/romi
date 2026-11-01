# PS2 in romi

PS2 games can run three ways, and the choice has more consequences here than on other systems. PS2
in the terminal is still experimental. What's below was tested on one laptop, a Ryzen AI 9 HX 370
with a Radeon 890M and an RTX 4060, in November 2026.

| | What it is | Plays in romi | Notes |
|---|---|---|---|
| **LRPS2** | PCSX2's RetroArch core | Yes, first choice | Not packaged for Arch; you add it yourself |
| **Play!** | Another emulator's RetroArch core (`libretro-play`) | Yes, if LRPS2 isn't there | Runs fewer games, and slower |
| **PCSX2** | The standalone emulator | No, it opens its own window | Runs the most games; nothing here changes it |

A plays a game in romi using the first core it finds installed, LRPS2 before Play!. Your usual
emulator stays in the game's menu as **Play with**. [Play in romi](tui.md#play-in-romi) explains how
to turn that round, so that A starts the emulator.

## Adding LRPS2

Put libretro's nightly in RetroArch's cores folder, and link your BIOS where LRPS2 looks for it:

```
curl -LO https://buildbot.libretro.com/nightly/linux/x86_64/latest/pcsx2_libretro.so.zip
unzip pcsx2_libretro.so.zip -d ~/.config/retroarch/cores
mkdir -p ~/.config/retroarch/system/pcsx2/bios
ln -s ~/.config/PCSX2/bios/*.bin ~/.config/retroarch/system/pcsx2/bios/
```

If `"vars"` or `"bios_dirs"` in your config point elsewhere, use those folders instead. With no BIOS,
romi's status bar shows LRPS2's own message: `Could not find any valid PS2 BIOS File`. A nightly isn't
a fixed release, and LRPS2's graphics code changes often, so a new one can behave differently.

## How it draws

LRPS2 and Play! draw with OpenGL, on the GPU your screen is plugged into. On a laptop with two GPUs,
that's usually the integrated one. romi can't give a core Vulkan, so leave `pcsx2_renderer` on Auto or
OpenGL: its Vulkan and paraLLEl-GS renderers won't start here. Both Software renderers crashed as a
Burnout 3 race loaded.

Sharp 3D draws at 1×, 2× and so on, 448 lines a step, up to 1056 lines. So most panels get 2×, which
is 1280×896. Each frame is read back from the GPU and sent to the terminal, and that's most of what 2×
costs. `"sharp_3d": false` (or Options → Sharp 3D) uses `pcsx2_upscale_multiplier` from RetroArch's
options instead.

## Speed and sound

romi times each frame by its sound, so a frame that takes longer than 16.7 ms eats into the 40 ms of
sound queued ahead. If that keeps happening, the queue runs dry, and you hear stutter.

On Sly Cooper's opening, with the same scripted buttons at 2× and no terminal, LRPS2 ran at about 2.3
to 2.4 times full speed and Play! at 1.1 to 1.2. A stronger graphics card didn't help Play!: the RTX
4060 ran at about 1.0× where the 890M reached 1.4×, because reading frames back cost it more.

To see what a game is doing, record its frame timings:

```
ROMI_FRAME_LOG=~/frames.csv romi
```

When the game stops, `frames.txt` beside it says how often the sound ran dry, and how long frames
took. `ROMI_CORE_LOG=core.log` records what the core says.

### Games that read the GPU back

Some PS2 games read what they've drawn back to the console's CPU. Emulating that means waiting for the
GPU, more than once a frame in the worst case. ATV Offroad Fury 2 does it three times a frame. Live at
2× in Ghostty, it ran at 52 fps with the sound dry 56 times in 19 seconds. With readbacks off, it ran
at 59.8 fps and the sound never ran dry.

romi turns readbacks off by itself for ATV Offroad Fury 2 (USA), recognised by the serial on its
disc (`SCUS_972.11`, read from an `.iso`), when it plays through LRPS2. Anything you set yourself for
`pcsx2_hw_download_mode`, for the game or the core, wins over that.

For other games, or other regions' discs, set it yourself: in romi's
[`"core_options"`](config.md#core-options), for the game or the whole system, or in RetroArch's
per-game options. For those, make a file named after the ROM in RetroArch's folder for the core,
here `~/.config/retroarch/config/ParaLLEl PS2/ATV Offroad Fury 2 (USA) (v2.00).opt`:

```
pcsx2_hw_download_mode = "Disable Readbacks"
```

Don't turn this off for every game: some need that data to draw properly or to work at all. ATV looked
right with it off from a cold boot into a race, but that's only a few minutes of testing.

## Games tried

| Game | LRPS2 | Play! |
|---|---|---|
| Burnout 3: Takedown | Played. Races need the fix below. Sound stutters a little when it's busy | Stops at boot |
| ATV Offroad Fury 2 | Played. Needs the fix below, and readbacks off | Black screen a minute in |
| Sly Cooper and the Thievius Raccoonus | Script reached the opening cutscene | Played, slowly in places |
| Guilty Gear XX Accent Core | Script reached a match; 59.95 fps through its opening | Played |
| NeoGeo Battle Coliseum | Script reached a fight; 59.95 fps through its opening | Hangs after Start |
| Final Fantasy X | Script reached the memory card menu | Stops at boot |
| Ratchet & Clank: Up Your Arsenal | Script reached the memory card prompt | Stops at boot |

"Script" means romi pressed buttons by itself for about a minute: the game boots and draws, but nobody
has played it this way. Play!'s three stops at boot happen in RetroArch too, and
[its tracker](https://github.com/jpd002/Play-Compatibility/issues) rates Final Fantasy X as playable,
so they may not happen on every setup.

### Bugs in the nightly

Two bugs in LRPS2 show up as of the nightly from November 1, 2026. Fixes have been written but haven't
reached LRPS2 yet, so until they do, these games show the bugs on the nightly:

- **Burnout 3's races are white** below the sky. Its bloom effect goes wrong, in RetroArch too.
- **ATV Offroad Fury 2's tracks are noise** at 2× and up, and its minimap can show pieces of a font.
  At 1× it's fine: set `"sharp_3d": false`, with `pcsx2_upscale_multiplier` left at 1× in RetroArch's
  options.

## Saves

Each emulator keeps its own memory cards, so a save made in one doesn't show up in the others:

| | Memory cards |
|---|---|
| LRPS2 | `~/.config/retroarch/system/pcsx2/memcards` |
| Play! | `~/.local/share/Play Data Files/vfs` |
| PCSX2 | `~/.config/PCSX2/memcards` |

LRPS2 starts with blank cards, so a game may ask to format one the first time. Say yes, or it can't
save. LRPS2's cards and PCSX2's look like the same format, but moving saves between them hasn't been
tried, so back up first if you try it.

## Achievements

Games played in LRPS2 earn RetroAchievements in romi (see [Earning them in
romi](achievements.md#earning-them-in-romi)); Sly Cooper's was tried. LRPS2 gives no memory map,
but its system RAM is the 32 MB of the console's main memory plus the 16 KB scratchpad, which is
what RetroAchievements numbers addresses by. Play! hasn't been tried. PCSX2's own achievements
aren't affected.

A game is found by the hash of its disc's main program, read by rcheevos, so it has to be a dump
RetroAchievements has registered. `.iso` and `.cue` work; `.chd` doesn't yet. When a dump isn't
known, the notice at the start says so and gives its hash, to look up on the game's page under
Supported Game Hashes. Final Fantasy X, for one, only has its International (Japan) disc.

## When a game stops responding

A game that hangs inside its core can't be stopped from outside. romi waits 3 seconds after you leave,
then lets go of it and says so. It can't play another game inside it until you restart romi. This is
what NeoGeo Battle Coliseum does in Play! after Start.

## Play! outside romi

Play! crashes in RetroArch on Wayland with `Killed by a signal`, before it draws anything. Under X11
it starts, so if you want it outside romi, make that the game's command in `overrides.jsonc`:

```jsonc
"ps2:Sly Cooper and the Thievius Raccoonus (USA).iso": {
  "emulator": "Play!",
  "command": "env -u WAYLAND_DISPLAY retroarch -L /usr/lib/libretro/play_libretro.so {rom}"
}
```

Inside romi it doesn't crash, as romi handles what Play! trips over. LRPS2 runs on Wayland as it is.

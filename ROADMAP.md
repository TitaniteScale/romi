# Roadmap

Where romi is headed, in the order I plan to build it. Each phase is useful on its own, and the later
ones build on what the earlier ones put in place.

## Where things stand

_As of 2026-10-22._ romi runs my whole library: 462 games across 22 systems, covering ROMs, shell
scripts, pinball tables, native PC ports and Steam. Everything passes `romi check`, BIOS files
included.

- **Works with just a controller:** a menu, search with an on-screen keyboard, jumping by letter,
  scrolling that speeds up when you hold the d-pad, favorites, recently played and play time.
- **Box art:** 434 of the 462 games have covers, scraped online or found on disk. They're drawn with
  Kitty graphics, Sixel or iTerm2 images, or colored half-blocks.
- **Game info:** genre for 358 games, year for 310, player count for 320 and a description for 203.
  You can filter by genre, players, decade and whether you've played it.

## 0. Groundwork ✓

Small things that make everything after them easier to build and check. All done.

- ✓ **CI**: rustfmt, clippy, the unit tests and the end-to-end tests on every push.
- ✓ **The test harness is in the repo.** `e2e/drive.py` runs romi in a fake terminal with a virtual
  uinput gamepad, presses keys and buttons, and reads the screen. The tests in `e2e/` use it with a
  made-up library, and anyone, coding agents included, can use it to check a change to the TUI.
- ✓ **Startup quirk:** if a terminal never answered the image-support query, romi lost the first
  keypress. romi now checks that the terminal answers before asking.

## 1. A real CLI ✓

romi already has `--check`, `--list` and `--scrape`. The goal is a CLI other programs can drive without
scraping text: rofi scripts, a waybar module, other launchers, coding agents.

Built in this order, one commit each:

1. ✓ **Safe shared state.** Several romi processes can run at once (the TUI, `romi play`, a detached
   game's watcher). Every change to `state.json` now goes through one locked read-modify-write, and
   the TUI reloads state when its terminal regains focus.
2. ✓ **Subcommands, using `clap`**: `romi check`, `romi list` and `romi scrape` so far, with `info`,
   `play`, `random` and `status` to come. The old `--check`/`--list`/`--scrape` flags still work, and
   bad arguments exit with 64.
3. ✓ **`list` and `info` with `--json`**, in a documented, stable shape. Game keys
   (`psx:Some Game (USA).chd`) are the IDs. `list` takes the TUI's filters plus systems and
   favorites. `info` finds games by key, exact name, or words from the name, and lists the candidates
   (exit 65) rather than guessing.
4. ✓ **`romi play <game>`**, sharing one launch path with the TUI.
   - It finds games the same way `info` does.
   - Flags: `--mode wait|terminal|detach`, and `--dry-run`, which prints the expanded argv, cwd and env
     without starting anything. That's handy for testing config changes, and for agents.
   - Exit codes: the game's own exit code when it ran. romi's own failures use the sysexits range,
     which games rarely use: 64, 65 and 66 as above, 69 for a missing emulator and 78 for a config
     error.
   - ^C or a script's `timeout` stops the game as well as romi, and the session is still recorded.
5. ✓ **`romi status --json`, tracked detached games, and shell completions.** A detached game is
   played by a background copy of romi that records its play time. Every running game is listed in
   the state directory, which is what `romi status` reads. Completions cover commands, flags, game
   names, system ids and genres.

## 2. `romi init`

`romi init` finds what's on the machine and writes a config that works.

- ✓ **Find ROM folders**: the one set in ES-DE, `~/ROMs`, `~/roms`, `~/Emulation/roms` (EmuDeck),
  `~/RetroPie/roms`. Each folder's system comes from its name (including common alternates like `scd`
  and `ps1`), or else from its file extensions.
- ✓ **Find emulators**: ES-DE's find rules cover programs on PATH, Flatpaks, AppImages, and RetroArch
  cores in all the usual places.
- ✓ **Use ES-DE's system definitions when ES-DE is installed**: about 200 systems, each with a
  labelled command per emulator. The first one that's installed wins, and the others are noted in the
  generated file. Without ES-DE, the systems in romi's default config stand in.
- ✓ **BIOS**: `"bios"` is filled in for the systems that need it, so `romi check` reports what's
  missing.
- ✓ **Never overwrite your config.** Generated systems go in `init.jsonc`, which the config includes
  (`"include"` is a general option now). Rerunning init replaces only that file. `--print` writes to
  stdout instead, and `--yes` skips the question.
- ✓ **First run**: with no config, `romi` does what init does (asking first, in the terminal) and then
  opens, rather than writing a default that points at `~/ROMs` and gets in init's way.
- **First run with a controller**: the same steps in a TUI wizard, for when romi is opened from
  couch mode before there's a keyboard to hand.
- ✓ **`romi add <path>`** adds a program, script, folder or `.desktop` launcher as a game, run from
  its own folder, in a `games.jsonc` the config includes.

✓ The same system definitions let `romi check` suggest fixes: another installed emulator for a system
that can't launch, folders that aren't in the config yet, and systems with no emulator installed.

✓ `romi check` also reads the emulators' own settings, and says when RetroAchievements is off or
not logged in, or no controller is bound, in RetroArch, PCSX2, Dolphin or melonDS.

## 3. Managing the library from inside romi ✓

Changes made in the TUI are saved in `state.json` (sort orders, tags), in `overrides.jsonc`, a file
the config includes (hidden games, emulator picks), or, for the options, in the config itself. All
done.

- ✓ **Multi-disc games as one entry.** "Mr. Bones (Disc 1)" and "(Disc 2)" become one game, launched
  through a generated `.m3u` so the emulator can swap discs.
- ✓ **Sorting** by name, last played, most played or year. Available from the Start menu and a key, and
  remembered per system.
- ✓ **Continue playing**: optionally start with the last game you played selected.
- ✓ **Hide a game** with the controller.
- ✓ **Alternate emulators**: several commands per system, ES-DE style (Beetle PSX or DuckStation, say),
  with one picked per game from a menu. ES-DE's labelled commands provide these for free.
- ✓ **Your own tags and collections**, like "couch co-op" or "backlog". Assign them with the controller
  and filter by them.
- ✓ **Hot reload**: the TUI reloads the config when it or a file it includes is saved, waiting while a
  game runs. The controller's A/B swap, covers and the animation's colors change without a restart.
- ✓ **Options**: a panel in the menu for volume, rounded corners, the launch animation, covers, title
  screens, the accent color, borders and the confirm button. Each change goes into `config.json`
  itself, only that value's text changed (`config_edit.rs`), so comments and layout stay.

## 4. Couch mode and hooks ✓

- ✓ **Hooks**: `on_launch` and `on_exit` commands, either global or per system, with the same placeholders
  as launch commands. Use them to move a game to its own workspace, pause music, or set the CPU
  governor.
- ✓ **Couch mode**: a Hyprland keybind, or the controller's Guide button (through `romi guide`), opens romi in
  fullscreen Ghostty with a big font. Give romi its own workspace and add window rules for games, so a
  game no longer ends up on the wrong workspace. This is docs and example configs in `examples/couch`,
  and `romi guide`, which runs a command on the Guide button without taking the controller from
  anything else.

## 5. Looks ✓

- ✓ **Cover grid view**: a grid of box art in place of the list, toggled with a button. This is the
  screenshot moment in Ghostty.
- ✓ **Title screens and screenshots**: the libretro thumbnail server has `Named_Titles` and `Named_Snaps`
  next to the box art, and needs no key either. Scrape them, and cycle through them in the details pane
  or show one beside the cover.
- ✓ **Stats screen**: play time per system, most-played games, a year in games.
- ✓ **RetroAchievements** (free API key): achievement progress in the details pane.

## 6. Built-in emulation for 2D systems

The long-term goal: play Game Boy, GBA, NES, SNES, Genesis and similar games inside romi, drawn in a
scene in the middle of the TUI (a CRT TV, a Game Boy shell), with the option to go full screen.

romi would load libretro cores directly, the same `.so` files RetroArch uses, rather than embedding one
particular emulator. A minimal libretro frontend for 2D cores is small: load the core, call `retro_run`
once per frame, and answer the video, audio and input callbacks.

It's built in steps, and the first one pays off without any of the TUI work:

1. ✓ **Headless runner**: `romi run <rom> --frames 600 --screenshot out.png [--input script]` runs a core
   for a set number of frames with scripted input and saves the last frame. There's no window and no
   RetroArch. A coding agent working on a ROM hack, a decomp or a homebrew game can build it, boot it
   and look at the result. Runs come out the same every time, so the screenshots also work as
   regression tests.
2. **Live video in the terminal**: stream frames with the Kitty graphics protocol, replacing one image
   in place. Without Kitty graphics, fall back to truecolor half-blocks (a Game Boy screen is 160×72
   cells), drawing fewer frames than are emulated if needed.
   - ✓ **Spike** (`romi run --live`): full screen, a controller, clock pacing. In Ghostty, a GBA game
     holds 59.7 fps through shared memory, with nothing piling up unread. Ghostty smooths the picture
     when it scales it, which blurs pixel art, so romi should scale by a whole number itself and send
     the bigger frame through shared memory. zlib shrinks a GBA frame from 112 KB to a few KB, which
     makes direct transfer a real option for terminals without shared memory.
   - ✓ **In the TUI** ("Play in romi" in a game's menu): the game plays in the middle panel behind
     Kitty's Unicode placeholders, so ratatui owns the cells and only the image changes each frame.
     romi scales frames to the cells' exact pixel size, nearest neighbour; panels that aren't a whole
     multiple of the game's size get some pixels a screen pixel wider than others.
   - ✓ **Arcade games through FinalBurn Neo**: it opens zips itself, so archives go to cores that
     list them; romi unpacks zips for the rest (Genesis Plus GX for the Mega Drive). Games on their side are turned upright as the core asks. Cores get RetroArch's system
     folder and its save folder for the core (headless runs get a fresh temporary one, so they
     repeat exactly). Cores that only draw with the GPU (Flycast) can crash while loading, so the
     TUI starts a core in a separate process the first time it's played.
   - ✓ **Aspect ratio**: games are shown at the shape the core gives, not with square pixels, so
     CPS-1/2 games (384×224) come out 4:3. romi works out how wide each pixel is from the core's
     base size and aspect ratio, since cores like mGBA give a base size (the Super Game Boy's) that
     isn't the frames'. Headless screenshots stay pixel for pixel.
   - ✓ **Screen filters** (`"screen_filter"`): scanlines, a CRT (scanlines and a phosphor mask)
     or an LCD grid, drawn by romi into each frame after it's scaled, so it works the same in
     every terminal. Each is a brightness per screen row and column, about 2 ms a frame at
     1300×1100, and the picture is brightened to make up for the dark lines. Ghostty's
     `custom-shader` was the other way to do it, but it covers the whole window, menus too.
     Still to do: the launch animation's last frames are drawn without the filter, so it comes
     on as the camera lands; `romi run --live`; screen curvature and glow. Real shaders
     (RetroArch's `.slangp` presets, through the `librashader` crate on the GPU, reading the
     frame back to send it) would look better still, at the cost of a GPU and a big dependency.
   - ✓ **Smooth frames**: `ROMI_FRAME_LOG=frames.csv` records when each frame was made, sent and
     read by the terminal (see `CONTRIBUTING.md`). In the TUI, frames were picked up whenever the
     main loop came round, so they reached Ghostty 12 to 22 ms apart (sd 2.1 ms) for a game making
     them every 16.7: some stayed up for two of the screen's refreshes and the next was never seen.
     The loop now waits for the core's frame and sends it straight away, and scales into the last
     frame's buffer, RGBA in one pass, copying repeated rows: 15.5 to 18.5 ms apart (sd 0.7),
     7 ms from the core to Ghostty instead of 10, for Link's Awakening DX at 1218×1050 with the
     CRT filter and corners. `romi run --live` sends 160×144 and gets sd 0.3.
     Still to do: scaling straight into the shared memory, made and touched ahead of time, would
     save the copy (2.4 ms for 5 MB) and most of what's left of the unevenness, which is how long
     each frame takes to get ready (3.6 ms, 5.7 at p99). What the terminal does after reading a
     frame isn't seen: recording the screen at 60 fps and counting repeated frames would show it,
     and compare it with RetroArch. A game at 59.7 fps on a 60 Hz screen shows a frame twice every
     few seconds wherever it plays, unless it's run at the screen's rate, as RetroArch does with
     vsync; a terminal doesn't say when it refreshes.
3. **Audio and input**: audio through `cpal`, resampled to the device rate and used to pace emulation.
   The controller already goes through `gilrs`. Playing on a keyboard needs key-release events, so it
   only works in terminals that support the Kitty keyboard protocol.
   - ✓ **Sound**, through PulseAudio (which PipeWire also serves) or ALSA. Each frame takes as long
     as the sound it made, stretched or shortened by up to half a percent to keep about 40 ms
     queued, so frames stay evenly spaced and the pitch is exact. A core's sample rate turned out
     to be more reliable than its frame rate: mGBA says 61.2 fps for Game Boy games, which run at
     59.7. `romi run --wav` saves a headless run's sound, the same every time.
4. ✓ **Saves**: in-game saves (the cartridge's battery-backed RAM) go in RetroArch's save folder,
   under the same names, so a save made in romi carries over to RetroArch and back. romi checks
   every second whether the game has saved, and writes the file then and when the game stops. A
   save that can't be read stops the game from starting, so it's never written over. Save
   states are still to come.
5. **Scenes**: a PNG frame with a transparent screen area. romi draws each emulated frame into it and
   sends the result as one image. Terminals without graphics get an ASCII-art frame around a half-block
   screen.
6. **Full screen**: at first, scale the game to fill the terminal. Later, maybe, hand off to RetroArch
   mid-game through a save state. Whether romi's save states and RetroArch's are compatible still needs
   checking.
7. ✓ **RetroAchievements during play**: rcheevos 12.5.0 (`vendor/rcheevos`, MIT) is compiled in,
   with a little C glue (`src/emu/cheevos.c`) so the Rust side never mirrors its structs.
   `romi achievements login` swaps the password for a login token and keeps only that. A game
   played in romi loads its achievements by the ROM's hash, checks them after every frame through
   rcheevos' libretro helpers (which lay out memory from the core's memory map, now kept), and
   unlocks go to the account as they happen, with a notice over the game. The web API's `MemAddr`
   is only a hash of the conditions, so the login was needed. Unlocks are softcore: the server
   demotes hardcore unlocks from clients it hasn't approved. The e2e tests run against a fake
   server (`e2e/fake_ra.py`, through `ROMI_RA_HOST`). mGBA only gives its memory map once it has
   run a frame, so memory is laid out after frames, again whenever the core gives a new map, and
   achievements aren't started before the first frame; before that, every GBA achievement that
   reads EWRAM was turned off (found playing Pokémon Emerald).
   - ✓ **Badges** in the notices, downloaded when an unlock comes in and cached; the notice
     waits up to half a second for one. Drawn like the game, a Kitty image behind placeholder
     cells, square, three rows tall, with the text beside it; long descriptions wrap.
     `"retroachievements": { "badges": false }` turns them off (in the config only, for now).
   - Still to do: progress and challenge
     indicators, and leaderboards (rcheevos sends the events, romi ignores them); rich presence
     is sent but not shown; `romi run --live` and headless `romi run` (where a scripted run
     earning an achievement would make a good regression test for ROM hacks); unlocks still
     waiting when a game stops are lost; hardcore, which would need RetroAchievements to approve
     romi.
8. **Launch animation**: a wireframe of the system (a console and a CRT, or a cabinet) builds itself
   out of lines, then the camera dives into its screen and lands on the cells the game is drawn in.
   Each game's file name seeds how the lines enter (pen trace, reverse explode, scan flash, fly in,
   dots then lines, vortex) and in what order.
   - ✓ **Prototype** (`cargo run --release --example wireframe`): an NES and a TV, drawn through
     shared memory like live play, with a stand-in picture for the game. At 1280×720 (half a
     2560×1440 terminal's pixels) a frame takes about 8 ms to draw, and the glass lands within 2 px
     of the game's cells. Models are JSON: vertices in cm, edges, and the screen's four corners.
   - ✓ **Importer** (`cargo run --release --example wireframe-import`): turns a downloaded OBJ, STL,
     glTF or GLB model into that JSON. It keeps the edges where the surface folds, where it ends, and
     where a flat side bends into a curve, joins straight runs, and finds the glass (the biggest
     smooth patch facing front with one outline). Parts made with `--no-glass` go beside a TV with
     `--add`. Tried on a made-up CRT so far (curved glass, rounded corners, knobs): about 500
     lines, drawn in about 7 ms a frame.
   - ✓ **Game Boy**: `gameboy.json`, made from Jason Toff's model on Poly Pizza, played for `.gb`,
     `.gbc` and `.gba` games (the GBA's 3:2 picture sits in the middle of its 10:9 glass). The originals of five CC BY models (Game Boy, NES, SNES, an old TV, a DS)
     are in `examples/wireframe/models/`, credited in `CREDITS.md`.
   - ✓ **The terminal's look**: lines are drawn over the terminal's own background (RGBA frames,
     clear but for the lines and the game), in a colour each game's seed picks from the terminal's
     ANSI palette (asked for with OSC 4, and the background with OSC 11). Colours that don't stand
     out from the background are skipped, and light backgrounds get lines that don't burn white.
     `--colors` gives a list instead. A frame takes about 8 ms to draw, 1 ms more than the solid
     version.
   - ✓ **A TV for the rest**: `nes.json` and `snes.json` are the old TV with the console on the
     floor in front, and `tv.json` the TV on its own, for every system without a console of its
     own yet. A game that isn't the glass's shape (GBA's 3:2, an upright arcade game) sits in the
     middle of it, from the size the core gives when it loads, and the camera lands on that. Lines
     behind the glass fade as the game comes on. `examples/wireframe/models/build.sh` makes every
     wireframe again; the importer's `--pieces` and `--drop` left out the SNES's controller and
     the NES's grip ridges.
   - ✓ **Arcade cabinet**: `arcade.json`, from J-Toastie's model, for arcade boards and the Neo
     Geo (`arcade`, `mame`, `fbneo`, `neogeo`, `cps1` to `cps3`).
   - ✓ **Rounded corners**: `"corner_radius"` rounds the corners of a game in the middle panel,
     as a percentage of its height (0, square, by default). romi already scales each frame to its
     cells' pixels, so the corners are cut there, smoothly, and left clear for the terminal's
     background. The animation's last frame has the same corners, so the handover doesn't show.
   - Still to do: a PC; the DS (two screens); the picture's corners while the camera dives in
     (square until it lands); rounded corners in `romi run --live`, where the terminal does the
     scaling. Lines behind a part show through it, which gets busy on the Game Boy; hiding them
     would need the faces as well as the lines. The importer can't read Draco-compressed or
     quantized glTF, and draws no outline along the sides of smooth round parts like knobs, only
     their ends.
   - ✓ **In romi**: Game Boy and Game Boy Color games played in the middle panel start with it,
     across the whole panel. The core waits until the game starts fading in on the glass, so its
     first frames are the ones the camera lands on, and the animation's image goes only once the
     game's is in the same place. Any key or button skips it; `"launch_animation"` turns it off or
     lists colors. The code is `src/emu/wireframe.rs`, which the prototype includes. About 52 fps
     in a pseudo-terminal sized like 2560×1440.
   - Still to do: `romi run --live`. mGBA draws Game Boy games that
     support the Super Game Boy with its border (8:7), a little wider than the Game Boy's glass
     (10:9), so the picture widens a few pixels as the camera lands. It has only been run in a
     pseudo-terminal so far, with its frames read back from shared memory, so it still needs
     watching in Ghostty and kitty.

The spike settled 60 fps in Ghostty. Still to check: kitty and other terminals, and how much input
lag there is.

## 7. Systems that need a GPU

Playing in romi only worked with cores that draw in software, handing over each frame in memory.
Flycast (Dreamcast, Naomi), Dolphin (GameCube, Wii), PPSSPP (PSP), Mupen64Plus-Next (N64),
Beetle PSX HW, Citra (3DS) and LRPS2 (PS2) ask for an OpenGL or Vulkan context to draw into
(`SET_HW_RENDER`) instead. romi said no, and some crashed at that (Flycast did, which is why a core
is tried in a separate process first). In the order worth trying:

1. ✓ **GPU drawing for libretro cores**, as RetroArch does it but with no window: an OpenGL context
   made through Mesa's EGL with no surface, a framebuffer the core draws into, and each frame read
   back from the GPU into the same shared-memory Kitty path as now. Screen filters, rounded
   corners, achievements and saves all come after that, so they'd carry over. OpenGL alone covers
   Flycast, Dolphin, PPSSPP, Mupen64Plus-Next and Beetle PSX HW; Vulkan (which needs the core's
   context negotiation interface) can come later. Start with Flycast, since it's the crash already
   seen. The fiddly parts: the GL version and profile each core asks for, and cores that draw on
   their own threads. Reading frames back is cheap at these sizes, even rendered above native
   resolution.
   - ✓ **Done** (`src/emu/gl.rs`): libEGL opened when a core first asks, a context with no
     surface on the GPU with a screen plugged in (EGL's device platform; Mesa's surfaceless one
     otherwise), the framebuffer the core draws into, and the frame read back after each
     `retro_run`, upside down as GL draws it. The context follows the core to whichever thread
     runs it. Flycast (Crazy Taxi), Mupen64Plus-Next (Banjo-Kazooie, GLideN64 asking for GL 3.3
     core) and Beetle PSX HW (GL 3.3 core) all play in the TUI. Getting there also took: plugging
     a pad into all four ports (Flycast plugs in nothing, not even the VMU, until it's heard about
     every port), the sticks and analog triggers, cores' `printf`s kept off the screen, and
     keeping OpenMP loaded (unloading bsnes-hd took it with it, and its threads crashed romi).
   - ✓ **Sharp 3D** (`"sharp_3d"`): drawn at 640×480 and scaled up to the panel, 3D looked
     blocky, so cores' resolution options (Flycast's, Mupen's, Beetle PSX HW's) are set to the
     one closest to the panel's height, at least as tall, up to 960 lines: 1280×960 for a 910 px
     panel. Scanlines and the phosphor mask stay at the console's 240 lines, as they were on a
     TV, which also fixed rows one screen pixel tall being dimmed as if they were a scanline's
     top. Uncapped, a 3800×2280 window (a 4K screen at 2× scale) got Flycast drawing at
     1920×1440, and Monkey Ball ran at 50 fps with its sound breaking up.
   - ✓ **Frames read back from the GPU without waiting**: with the game big in Ghostty on a 4K
     screen, Ghostty and the compositor keep the Radeon 890M 95% busy even for a SNES game, and
     waiting for the GPU to finish each of Flycast's frames took 11–16 ms, so Monkey Ball fell
     behind its sound even at 640×480. Frames are now copied into a buffer while the core goes
     on, and taken a frame later: its frames went from 17 ms to 4, a steady 60 fps. The frame
     log counts the sound's gaps (`sound ran dry`).
   - ✓ **Warming up**: Flycast's first frames take 100 ms each (it compiles shaders), which
     stuttered the game as it came on and ran its sound dry. Cores now run their first frames,
     muted and unseen, behind the launch animation, until a few in a row come quickly.
     Monkey Ball still has six dark seconds of slow frames as it loads after the Naomi BIOS:
     with threaded rendering, Flycast waits 100 ms for a frame the game isn't drawing. RetroArch
     gets the same.
   - Still to do: Vulkan (Mupen's ParaLLEl-RDP, Flycast's and Beetle's Vulkan renderers, which
     some people pick in RetroArch, and which then won't start); the PlayStation controller type
     from RetroArch's remap files, since Ape Escape needs a DualShock (see 8);
     a core that changes its frame rate with `SET_SYSTEM_AV_INFO` keeps the old one for pacing;
     Dolphin and PPSSPP aren't installed here, so they're untried; Naomi needs `naomi.zip`.
2. ✓ **Core options** (`GET_VARIABLE`, which romi answers with "no" now, so every core runs on its
   defaults). On its own this lets romi pick the software renderers some cores still have:
   ParaLLEl/angrylion in Mupen64Plus-Next, and Dolphin's and PPSSPP's. Dolphin's is far too slow
   for full speed and the others depend on the CPU, so it's a stepping stone, but options are
   worth having anyway. Done with GPU drawing, since Mupen64Plus crashed on its defaults when
   asked nothing: cores get what RetroArch has for them (the core's `.opt`, then its folder's,
   then the game's), where it's a value the core takes, and their own default otherwise.
   Changing options from inside romi is still to come.
3. **Any emulator's window, captured**: run a standalone emulator in a compositor with no screen
   and draw its frames in the panel. This is the only way for systems with no libretro core at
   all: Switch (Ryujinx), PS3 (RPCS3), Wii U (Cemu), PCSX2. Valve's `gamescope` has a headless
   backend and can stream its output over PipeWire, so romi may only need to read that stream; not
   tried yet. The emulator keeps its own sound, controller, saves and achievements, so romi is just
   the window. Costs more latency and has many moving parts.

## 8. Controls for games played in romi

Every game played in romi gets the same fixed layout (`emu/controls.rs`): the pad's buttons by
position, SNES-style, and RetroArch's default keys. Every controller is merged into player 1, and
the core sees nothing on the other ports. Emulators that run on their own keep their own controls,
and as more systems play in romi, this is where their controls end up.

1. ✓ **RetroArch's remap files** (`emu/remap.rs`), read the way core options are: from
   `input_remapping_directory` (`config/remaps` by default), in the core's folder, the game's
   `.rmp`, else its folder's, else the core's, the first one found winning, as in RetroArch.
   Skipped when `auto_remaps_enable` or `input_remap_binds_enable` is off. Player 1's
   `input_player1_btn_*` and `_stk_*` lines say which button or stick direction each of the pad's
   sends (sticks to buttons and back included), and `input_libretro_device_pN` the controller type
   plugged into each port, which Ape Escape needs to be a DualShock. `romi run` uses them too, so
   its scripts press the pad's buttons. Left out: remaps sorted by controller
   (`input_remap_sort_by_controller_enable`), the keyboard lines (`_key_*`), turbo, and
   `analog_dpad_mode`, since romi decides for itself when the left stick is also the d-pad. There
   are no remap files on this machine yet, so it's only been tried with made-up ones.
2. **Remapping in the TUI**, for a system or a single game, using the controller or the keyboard:
   "press the button for A", one button at a time, with what the button does in the game next to
   it ("B — Jump"). Cores say that as they start (`SET_INPUT_DESCRIPTORS`), so romi would remember it
   from the last time the game was played. A single game's remap is saved in `overrides.jsonc`, and
   it wins over the system's, which wins over a RetroArch remap file, which wins over the default.
3. **More than one player**: each controller on a port of its own, in the order they connected,
   with a screen to swap them. The remap screen is the natural place for it.

## Filling the gaps, any time

Smaller items that fit between phases.

- ✓ **Steam game info**: `romi scrape` asks Steam's store (`IStoreBrowseService/GetItems`, the API
  its own pages use, with no key) about every Steam game, in batches of 50. Genres come from the
  tags players voted for most, the year from the first release, and players from the split-screen
  categories and tags. The store also gives the cover's address, under a hashed folder for some
  games, which brought in the four covers the CDN lookup missed.
- **Import play history**: Steam play time from Steam's local config, and `playcount`/`lastplayed` from
  ES-DE gamelists, so recently played and the stats include games played outside romi.
- **Exact matches for disc images**: CHD files store a SHA-1 of their contents, which can be matched
  against Redump. That gives PlayStation, Saturn, Dreamcast and the other disc systems the exact matches
  that cartridges already get from checksums.
- **More descriptions**: Wikipedia's summary API needs no key, but matching titles to articles is
  hit-or-miss. ScreenScraper and TheGamesDB need keys.
- **SteamGridDB** (free key) for covers on PC ports and pinball tables.
- **Tests that play leave frames behind**: `e2e/drive.py` ends romi with SIGKILL, and pyte never
  reads the frames, so each test that plays in romi leaves up to 30 in `/dev/shm` (about 150 MB
  for a whole run, until they're deleted or the machine restarts). Stopping romi with Esc, or a
  SIGTERM it cleans up after, would fix it.
- **An AUR package**: the AUR isn't taking new packages. Until it does, the recipe in `packaging/arch`
  builds with `makepkg -si`.
- **A waybar module**, if it turns out to be wanted: games usually run full screen, hiding the bar, so
  it's not planned. `romi status --json` already has what one needs.
- **7z ROMs in romi**: zips are unpacked for cores that can't open them, 7z files aren't. Only cores
  that open 7z themselves (FinalBurn Neo) can play them.
- **MAME in romi**: its core crashed while starting, before cores got a system and save folder. Not
  tried since; FinalBurn Neo covers most arcade sets.

Code to tidy, when it gets in the way:

- **One controller reader**: the TUI's thread (`gamepad.rs`) and each game played in romi
  (`emu/controls.rs`) open their own `Gilrs`, so starting a game reopens the controllers. It works;
  sharing one would mean handing the devices between the two.
- **Splitting `tui/app.rs`** (about 1,600 lines): menus, search and launching could each be a file.

# Roadmap

Where romi is headed, in the order I plan to build it. Each phase is useful on its own, and the later
ones build on what the earlier ones put in place.

## Where things stand

_As of 2026-11-01._ romi runs my whole library: 458 games across 22 systems, covering ROMs, shell
scripts, pinball tables, native PC ports and Steam. Everything passes `romi check`, BIOS files
included.

- **Works with just a controller:** a menu, search with an on-screen keyboard, jumping by letter,
  scrolling that speeds up when you hold the d-pad, favorites, recently played and play time.
- **Box art:** 432 of the 458 games have covers, scraped online or found on disk. They're drawn with
  Kitty graphics, Sixel or iTerm2 images, or colored half-blocks.
- **Game info:** genre for 365 games, year for 319, player count for 328 and a description for 212.
  You can filter by genre, players, decade and whether you've played it.
- **Plays in romi:** every emulated system in the library draws in the middle of the launcher, from
  the Game Boy to the PS2 (see the README's table), with or without RetroArch installed.
  RetroAchievements unlock as you play, with badges, progress, challenges and rich
  presence.

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
- ✓ **A folder of its own**: a new config on a machine with no RetroArch (no `retroarch` on PATH, no
  `~/.config/retroarch`, and no Flatpak of it) keeps BIOS files, in-game saves and downloaded cores in `~/.local/share/romi`.
  It's written in the config (`"bios_dirs"`, `"saves_dir"`), so installing RetroArch later doesn't move
  them, and a config that's already there isn't changed. A Flatpak RetroArch counts, with its own
  folders, unless there's also one on the machine itself. On a first run the launcher covers the line saying where BIOS files go
  ([#5](https://github.com/TitaniteScale/romi/issues/5)).
- ✓ **Never overwrite your config.** Generated systems go in `init.jsonc`, which the config includes
  (`"include"` is a general option now). Rerunning init replaces only that file. `--print` writes to
  stdout instead, and `--yes` skips the question.
- ✓ **First run**: with no config, `romi` does what init does (asking first, in the terminal) and then
  opens, rather than writing a default that points at `~/ROMs` and gets in init's way.
- ✓ **A first run with a welcome**: a title screen with the logo, then "Where are your ROMs?" with the
  folder filled in if one was found, whether to share RetroArch's folders or keep to romi's own if
  RetroArch is here, and what romi found, before anything's written. Saying yes
  runs `romi init`. The keyboard and a controller both work, but a controller can only take the folder
  offered, not type one. The title screen doesn't come back unless `"title_screen"` (Options → Title
  screen at start) is on, for a nice-looking start. The note about BIOS files now shows before the
  launcher opens ([#5](https://github.com/TitaniteScale/romi/issues/5)).
- ✓ **`romi add <path>`** adds a program, script, folder or `.desktop` launcher as a game, run from
  its own folder, in a `games.jsonc` the config includes.

✓ The same system definitions let `romi check` suggest fixes: another installed emulator for a system
that can't launch, folders that aren't in the config yet, and systems with no emulator installed.

✓ `romi check` also reads the emulators' own settings, and says when RetroAchievements is off or
not logged in, or no controller is bound, in RetroArch, PCSX2, Dolphin or melonDS.

## 3. Managing the library from inside romi ✓

Changes made in the TUI are saved in `state.json` (sort orders, tags), in `overrides.jsonc`, a file
the config includes (hidden games, emulator picks, covers), or, for the options, in the config itself. All
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
- ✓ **Your own covers**: Add art… in a game's menu takes an image dropped on the terminal (which
  pastes its path), and keeps a copy next to the config. Remove custom art takes it away.
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
   save that can't be read stops the game from starting, so it's never written over.
   - ✓ **Save states**: ten slots a game, Guide+R, Guide+L and Guide+Down (undo the last load) on a
     controller (`"state_button"` picks Select or off, and a controller without Guide uses Select), F2,
     F4 and F3 on a keyboard, and `romi run --save-state` and `--load-state`
     (`emu/savestate.rs`), in RetroArch's file format: a state from romi loaded in RetroArch 1.22.2,
     and its compressed state loaded in romi (mGBA, the only core tried both ways). A core gets 120
     frames before a state, as Mupen64Plus crashed on one before its first frame and Dolphin on a
     GameCube state after one. With one game per system, a state loaded in a new run against a run
     that never stopped: pixel for pixel on the 2D systems tried, and the same scene on PlayStation,
     N64, Dreamcast, GameCube and Wii, some of which don't repeat exactly even without a state.
     Largest states: Wii 109 MB, GameCube 93 MB, Naomi 68 MB, PS2 51 MB; a core takes at most 58 ms
     over one. `romi run` loads a state 120 frames in, and PPSSPP two seconds in, as it needs the time
     as well as the frames: Bust-A-Move stayed black for hundreds of frames without it. PPSSPP
     sometimes refuses to save, which trying again gets past. PPSSPP boots a game in the background,
     and unloading while it does finishes the boot, which crashed romi once `context_destroy` had taken
     the GL context, so stopping a game early took romi down, and so did romi's own one-frame try at
     a core, as "the core crashed while starting". PPSSPP now keeps its context as it unloads, and
     `romi run` never unloads (so its exit code and errors are always its own). Played through save, load and undo in
     a pty with mGBA, ProSystem, PPSSPP, Dolphin and LRPS2.
     States go in `~/.local/share/romi/states`, not RetroArch's folder, compressed as RetroArch
     compresses them (a Wii state goes from 109 MB to 4.5 MB in about 0.2 s), on another thread so the
     game doesn't stop for it. They carry how far along achievements are. A load that takes back the
     in-game save keeps the last five copies.
     Still to do: [#7](https://github.com/TitaniteScale/romi/issues/7), a picture of
     each slot; [#8](https://github.com/TitaniteScale/romi/issues/8), loading RetroArch's states from a menu; [#9](https://github.com/TitaniteScale/romi/issues/9), `romi run --live`.
5. **Scenes**: a PNG frame with a transparent screen area. romi draws each emulated frame into it and
   sends the result as one image. Terminals without graphics get an ASCII-art frame around a half-block
   screen.
6. **Full screen**: at first, scale the game to fill the terminal. Later, maybe, hand off to RetroArch
   mid-game through a save state, as they share a format (see 4).
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
   - ✓ **Unlocks the server missed** are written down as they're sent and crossed out when it
     answers (`emu/unlocks.rs`), so ones still waiting when a game stops are sent, re-signed with
     when they were earned, the next time a game starts or romi opens.
   - ✓ **Progress indicators**: an achievement that counts up (rcheevos' measured progress) shows
     `12/50` and its title in a box at the top left of the game while it changes, ended by
     rcheevos' own hide event, with the achievement's badge beside it once it's here.
   - ✓ **Challenge indicators**: while an achievement is primed (all but its last condition
     holds), its title is listed under "Challenge" at the top right, up to three and a count of
     the rest, each with its badge when every one is here and the picture is tall enough (titles
     only otherwise). It yields to the progress box if they'd overlap on a narrow picture. Badge
     images are one Kitty image per slot (`notice::SLOT_*`), the unlock notice's first.
   - ✓ **Logging in from inside romi**: **RetroAchievements** in Options logs in and out, with an
     on-screen keyboard that has capitals and symbols, for a controller. RetroArch's login can be used
     as it is, in Options and on a first run, as its token is the same one the server gives romi for
     the password. Anyone else gets a hint after their first game. None of it asks a new user for a password.
   - **Leaderboards are blocked on hardcore** (found 2026-11-01): rcheevos 12.5.0 only evaluates
     them when hardcore is on (`rc_client.c`, `client->state.hardcore ||
     allow_leaderboards_in_softcore`, and that field is internal, with no setter), so a softcore
     client never gets a started, tracker or scoreboard event. They come with hardcore, which would
     need RetroAchievements to approve romi and games played in romi to lose save states, rewind and
     the like. A read-only screen (a game's leaderboards, your rank and the top scores, from the web
     API) would work now; not built.
   - ✓ **Rich presence** is read once a second and shown in the line under the game, before how
     to stop, when both fit (not in `romi status`).
   - ✓ **Headless `romi run --achievements`**: logs in, loads the game's achievements after the
     first frame and checks them after each, then reports what it unlocked (`--json` has an
     `achievements` object). It sends the unlocks, to the account or `ROMI_RA_HOST`, so a scripted
     run earning one is a regression test for a ROM hack. Hashes the file itself (`ra::hash_file`),
     so it works for ROMs outside the library.
   - Still to do: `--live` with achievements (it has no notices to show them in).
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

The spike settled 60 fps in Ghostty. Input lag was measured on 2026-11-04, press to composited picture at
4K on a Radeon 890M. Frame size doesn't matter (a quarter-size frame was no better). Ghostty's
`background-blur` is what costs: 2D is 57 ms in RetroArch, 60 in Ghostty without blur and 106 with it,
and `romi check` warns about it. 3D is 58, 82 and 147, so about 24 ms of it is still romi's
([#11](https://github.com/TitaniteScale/romi/issues/11)). Other terminals are [#12](https://github.com/TitaniteScale/romi/issues/12).

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
     some people pick in RetroArch, and which then won't start); more PlayStation games that need a
     DualShock than Ape Escape (`DUALSHOCK_GAMES` in `emucfg.rs`, which romi plugs in by itself);
     a core that changes its frame rate with `SET_SYSTEM_AV_INFO` keeps the old one for pacing;
     PPSSPP isn't installed here, so it's untried (Dolphin plays, see 9.2); Naomi needs `naomi.zip`.
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
2. ✓ **Remapping in the TUI** (`tui/controls.rs`): Controls… in the menu of a game that plays in
   romi lists the game's buttons and the pad's button that sends each. Pick one and press the
   button you want for it, on the controller or the keyboard, and the two swap, so none is left
   doing nothing. The buttons are named as the core names them (`SET_INPUT_DESCRIPTORS`), which
   romi remembers in `buttons.json` in its state folder when a game played in romi stops, for the
   game and its system; until then, as on a Super Nintendo pad. The controller's are named as the
   hints name them (A at the bottom). Saved for the game in `overrides.jsonc`, or for the system in
   the config's top-level `"controls"`, keeping only the buttons that differ, and they go over one
   another button by button: the game's over the system's, over RetroArch's remap file. L3 and R3
   reach games now too; they weren't read before. Left out: stick directions can be sent from
   the config (`"l": "l_x+"`), not from the screen, and the keyboard's keys can't be changed, only
   what the pad's buttons they stand for send.
3. ✓ **More than one player**: each controller is a player of its own, up to four, on libretro's
   first four ports, with the keyboard as player 1. Players… in the menu, with two or more
   controllers connected, sets the order: a button on each controller in turn. It's saved in
   `state.json` by the controllers' names, a second of the same kind as "… #2" in the order gilrs
   finds them, so two identical controllers can trade places if they're reconnected in another
   order. Select and Start on any player's controller stops the game. RetroArch's remap applies to
   each player as it has them, and romi's to every player. `romi run` scripts press other players'
   buttons with `p2:a`. Tested with two virtual controllers; not yet with two real ones. Left out:
   multitaps (the SNES's four players need the core's multitap option), and swapping players
   while a game is running.

## 9. More systems played in romi

The systems that still only run outside romi: the DS, GameCube, PS2, and Steam with the systems
that have no libretro core. In the order they're worth doing:

1. ✓ **DS**, through RetroArch's melonDS core (0.9.3), which draws in software. Both screens come as
   one frame, one above the other (256×384), as `melonds_screen_layout` has them.
   - **Play in romi from any emulator**: a game whose own command isn't RetroArch (the DS games
     run in standalone melonDS) plays in romi through the first of its other emulators that has a
     core installed, ES-DE's "melonDS" among them. `romi run` does the same for games in the
     library.
   - **Saves shared with standalone melonDS**: the core writes `<name>.sav` in the save folder
     it's given, as melonDS does, so a DS game that runs in standalone melonDS gets melonDS's save
     folder (next to the ROM unless `SaveFilePath` says otherwise, `emucfg::shared_save_dir`).
   - **Touch with the mouse**: libretro's pointer (`RETRO_DEVICE_POINTER`) answers with player 1's
     touch. While a game plays, the terminal reports the left button's presses and drags (modes
     1000, 1002, 1006), in pixels where it says it can (1016, asked about with DECRQM at start),
     and romi maps that through the picture's cells and cropped edges to a pixel of the core's
     frame. melonDS's "Mouse" touch mode (RetroArch's default, which moves a cursor of its own by
     how far the mouse moves) is taken as "Touch" in romi. `romi run` scripts touch with
     `touch:XxY`; tapping YES at Platinum's "Please touch a button" moved it on, headless.
   - **A laptop's touchscreen** doesn't work in Ghostty 1.4 (GTK, Hyprland), though it does in
     standalone melonDS: Ghostty reports a tap as a left-button press and release, but at the
     mouse pointer's last position rather than the finger's, and a finger's drag sends no motion.
     romi can't tell where the finger was, so this waits on Ghostty. Reading the touchscreen
     itself (evdev) is out: it gives a place on the whole screen, and Wayland doesn't say where
     the window is.
   - Still to do: checking the mouse in kitty (it plays well in Ghostty); fitting the layout to the panel (side by side in a
     wide one); a DS wireframe (the model is in `examples/wireframe/models/`); the real BIOS and
     firmware (the core uses FreeBIOS, as standalone melonDS does here); touch in `romi run --live`;
     the microphone (L2 in the core blows into it) and closing the lid (L3); the newer melonDS DS
     core, which keeps saves in a folder of its own.
2. ✓ **GameCube**, through RetroArch's Dolphin core (`libretro-dolphin`, based on Dolphin 2609, the
   same as standalone here), drawing with OpenGL through section 7's GPU drawing.
   - **The C++ runtime, loaded first and shared**: Dolphin's core carries its own copy of parts
     of libstdc++ (`std::call_once`'s helper and the thread-local it reads). Loaded first, it had
     libstdc++ looking things up in the core's copies, while Mesa's LLVM, loaded for the GL
     context, used libstdc++'s own, and romi crashed in `LLVMInitializeAMDGPUTarget` as the
     context was made. romi now loads libstdc++ globally before any core, as RetroArch has it by
     linking it, so everything shares its copies. Loaded privately instead, it crashed inside
     Dolphin.
   - **Memory cards shared with standalone Dolphin**: both keep them as GCI folders under a user
     folder's `GC`, and the core's user folder is `User` in the save folder it's given, so romi
     gives it `~/.local/state/romi/dolphin` with `User/GC` linked to Dolphin's (only when Slot A is
     a GCI folder in the usual place). Its settings and caches stay out of Dolphin's way.
   - **Sharp 3D**: `dolphin_efb_scale` is fitted to the panel like the other cores' sizes, and the
     cap went from 960 lines to 1056 so Dolphin can draw at 2× (1280×1056, about 4 ms a frame
     headless); the other cores' next sizes are 1440 and over, so they're unchanged. Dolphin's
     on-screen messages are off unless RetroArch has them on.
   - **One CPU thread**: with Dolphin's dual core on (the core's default, and RetroArch's), its
     emulated CPU, which makes the sound, runs on a thread of its own, out of step with the frames
     romi asks for. A frame brought a refresh's sound (800 samples) or none, about 60% none for
     Melee and Kirby Air Ride, up to 29 in a row, so they played at the wrong speed and their sound
     ran dry. romi turns dual core off (`dolphin_main_cpu_thread`) unless RetroArch has it on:
     each frame then brings one refresh's sound, still at 9× full speed headless. (Monkey Ball 2's
     game INI already turned it off, which is why it sounded right.)
   - **Frames between refreshes**: from before that, a frame without sound, from a core that makes
     sound, takes no time and isn't handed over (up to 15 in a row; after that the core has gone
     quiet and keeps to its frame rate). It's what keeps dual core playable if it's chosen.
   - **Defaults a core doesn't list**: Dolphin's shader compilation default is the unexpanded
     macro `ShaderCompilationMode::Synchronous`, which romi handed over as it was. A default not
     among an option's values is now its first value, as in RetroArch (Synchronous).
   - Needs `dolphin-emu/Sys` in RetroArch's system folder; linked here to standalone Dolphin's.
   - **The core's GL state left alone**: romi reads frames back in the core's context, and left its
     own framebuffer bound; Dolphin remembers what it bound, so the start of each frame (Monkey
     Ball 2's sky and ground, Melee's and Kirby's menus) went where it was never shown. romi now
     puts back the bindings and pack settings it changes. RetroArch gives such a core a context
     of its own.
   - Still to do: Monkey Ball 2 played live after that fix (it played badly in Ghostty but
     evenly in a pty at 2×); frames that compile shaders (up to 400 ms the first time an effect is seen, where
     standalone Dolphin has a cache from playing; sharing `~/.cache/dolphin-emu/Shaders` might do,
     as it's the same version); a game loading a save from the shared card, seen in play (the core makes and
     reads `GC/USA/Card A` through the link, checked with an empty folder); `romi check` saying
     when `Sys` is missing; how it runs in a real terminal at 2× with the frame log; rumble; a
     GameCube wireframe; Wii, a separate job for the Wiimote.
3. ✓ **PS2**, through LRPS2 (PCSX2's core, which Arch doesn't package) or else RetroArch's Play!
   core (`libretro-play`). NeoGeo Battle Coliseum and Guilty Gear XX Accent Core play headless, to
   their intro and menus, at 3–5× full speed, with sound.
   - **GLEW without GLX**: Play! loads GL's functions with GLEW, whose `glewInit` loads them, then
     GLX's, and fails with no GLX display, as there isn't with romi's EGL context. Play! gave up
     setting up its drawing and crashed on the first frame, calling a function it never loaded.
     romi exports a `glewInit` of its own, which cores find before GLEW's: it calls the core's
     GLEW's `glewContextInit`, GL's part only. (glvnd's GL functions work with EGL as with GLX.)
     RetroArch on X11 makes GLX contexts, so it never meets this; on Wayland it may.
   - **Found as an emulator**: ES-DE doesn't list Play!'s core for PS2, so romi adds it after
     the system's own (`init::MORE_CORES`), and PS2 games that run in PCSX2 play in romi with it.
   - **Sharp 3D**: `play_res_multi` is fitted like the others, at 448 lines a step: 2× is
     1280×896, 4.5× full speed headless.
   - **A game that stops responding**: NeoGeo Battle Coliseum hangs in Play! after Start at the
     title (its emulated CPU spins in the game's code, and `retro_run` waits for a frame that never
     comes; it isn't on Play!'s compatibility tracker, so likely Play!'s). romi waited for the
     core's thread for good, and had to be killed. Now it waits 3 s, then leaves it running, says
     so, and plays no other game in romi until it restarts. Applies to every core.
   - **Sound Play! lost**: Play!'s libretro frontend kept only the newest block of sound
     (`CSH_LibreAudio::Write` replaced the buffer) when its emulator thread made two between
     frames: Guilty Gear XX Accent Core lost a fifth (49.8 s of sound in 60 s of frames), and in
     romi, which times frames by their sound, played fast with its sound cut. romi exports
     replacements for `Write` and `ProcessBuffer`, as for `glewInit`, that queue every block:
     60.1 s. A pull request to Play! with the fix is written up in `notes/play-audio-pr.md`, to
     send; once Arch's `libretro-play` has it, the replacements can go.
   - **Frames timed by the average sound**: even with every block kept, Play!'s sound comes in
     lumps, 0 to 3 blocks (22 ms each) a frame. Each frame took as long as its own sound, and one
     with none took no time and wasn't shown: Guilty Gear showed 30 frames a second, ±13.7 ms, the
     queue swung past 60 ms and romi threw sound away (2.1 s in 50, in a simulation of its
     frames), and it ran dry 12 times in 44 s. Now each frame takes the average over recent frames,
     those without sound counted while they're gaps between lumps (up to 15 in a row): 59.3 a
     second, ±3 ms, and dry only while Play! starts and once at a 150 ms stall of its own. Steady
     cores are unchanged (Monkey Ball 2 61.0 fps either way).
   - **A game that stops responding, and quitting**: once a stuck core has been left running,
     romi ends with `_exit`, as exiting normally tore down libraries under its thread and crashed
     romi on the way out (once in two, seen live).
   - **LRPS2, ahead of Play! (2026-11-01)**: libretro's nightly
     (`buildbot.libretro.com/nightly/linux/x86_64/latest/pcsx2_libretro.so.zip`, BIOS in
     `<system>/pcsx2/bios`) runs through romi's OpenGL path as it is, and is used wherever it's
     installed: first in ES-DE's list, and in `MORE_CORES` for the built-in one. Same Sly Cooper
     script, 2×, frames read back, AMD GPU, two alternating rounds, no terminal: LRPS2 2.3–2.4× full
     speed (1.9× at worst, 1.5% of frames over 16.7 ms), Play! 1.1–1.2× (0.9× at worst, up to 50%
     over), and LRPS2's sound is even, 800 or 801 samples every frame. Live, with nothing reading
     the frames: Guilty Gear and NeoGeo Battle Coliseum's openings at 59.95 fps, sd 0.4 ms, the sound
     never dry. All seven of Jake's games boot and reach menus, a cutscene or a match in it (a
     one-minute script, not real play; only Sly Cooper ran longer), where Play! aborts at boot on
     three (FFX, Burnout 3, Ratchet: an exception opening a file, the same in RetroArch under X11) and
     hangs in NeoGeo Battle Coliseum. An RTX 4060 doesn't help Play! (1.0× against the AMD's 1.4×, at
     35% use; reading back costs more). Play! crashes in RetroArch on Wayland, not X11:
     `env -u WAYLAND_DISPLAY` got Sly Cooper through 30 s.
   - **The context before the game**: romi called `retro_unload_game` before `context_destroy`, and
     LRPS2's `context_destroy` then waited for its stopped CPU thread for good, so romi hung as it
     quit. Now the context goes first, as in RetroArch. Dolphin, Flycast, Beetle PSX HW,
     Mupen64Plus-Next and Play! exit as before.
   - **Burnout 3's white races**: LRPS2's bloom hack (`GSC_BurnoutGames`) keeps a thin column's width
     on its second draw, so the bloom buffer becomes 128×2240, and the bloom comes out all
     white below the sky, in RetroArch too, and no option changes it. One line fixes it; Jake's core
     is built from `083de59` with it, and the nightly has the bug until a pull request written up in
     `notes/lrps2-burnout-pr.md` is sent and merged. Both Software renderers crash as that race
     loads, in LRPS2's generated code (not looked into).
   - **ATV Offroad Fury 2 upscaled**: noise for its tracks and the font sheet for its minimap, at 2×
     and 4×. It keeps packed colour in its depth buffer, and LRPS2 uploads to an upscaled depth target
     through a bilinear shader even when the game asks for nearest. A few lines fix it (in Jake's core,
     and `notes/lrps2-burnout-pr.md`).
   - **Readbacks cost ATV its sound**: it reads the GPU back three times a frame, waiting each time:
     live at 2× on the Radeon 890M, in Ghostty, 52 fps and dry 56 times in 19 s; with
     `pcsx2_hw_download_mode` at "Disable Readbacks", 59.8 fps and never. Jake has that in per-game
     `.opt` files in RetroArch's `config/ParaLLEl PS2`, which romi reads. Everyone else would need the
     same. ✓ romi now sets it itself for games known to need it, recognised by the disc's serial
     (`disc.rs` reads it from an `.iso`; `emucfg::LRPS2_GAMES`, under everything RetroArch has, so
     any option file wins): only ATV Offroad Fury 2 (USA, `SCUS_972.11`) so far. Whether that data
     matters to anything in the game isn't known; nothing looked wrong from a cold boot to a race.
   - ✓ **`"core_options"` in romi's config** (2026-11-01): a system's (top-level, by system id) and
     a game's (in its override) go over RetroArch's option files, the game's over the system's
     (`runner::options`), for games played in romi. The built-in table (`LRPS2_GAMES`) is the lowest layer.
   - ✓ **Achievements for LRPS2 (2026-11-01)**: it gives no memory map, but its system RAM is 0x2004000
     bytes, the size of rcheevos' PS2 table, so the flat fallback lines up; Sly Cooper unlocked live.
     PS2 discs are hashed by rcheevos (`rc_hash_generate`; `.iso`/`.cue`), and an unknown dump's
     notice gives its hash.
   - GameCube and Wii achievements: games are hashed from `.rvz` and the rest with the `nod` crate
     (rcheevos reads only plain ISOs), and RetroAchievements knows them. Still to do: an unlock
     played live to prove the memory reads right (the core gives MEM1 as plain system RAM, big-endian
     as RetroAchievements' Dolphin sets expect); Wii games whose achievements are in MEM2
     (0x90000000 and up) can't earn them, as Dolphin's core gives no memory map for it.
   - Memory when games are played one after another in one romi (measured by loading and dropping a
     core repeatedly): bsnes-hd leaked 230 MB a game, fixed by keeping it loaded (`STAYS_LOADED`).
     Mupen64Plus-Next still leaks about 55 MB a game and can't be kept loaded (its second game
     fails to load), PPSSPP about 10 MB, Dolphin, Flycast and Beetle PSX a few. A core that leaks
     like that would need its own process to give it all back.
   - Launch animation models: the other handhelds borrow the Game Boy, and every other console is the
     TV alone. Still to do: a landscape handheld for the Game Gear, Lynx, PSP and the like (the
     credited "Handheld videogame console" model is credited in CREDITS.md, not imported yet);
     consoles of their own for the biggest TV systems (Genesis, PlayStation, N64...), as each is
     one `build.sh` line with the NES's recipe. The NES was facing the TV; it faces you now.
   - Still to do for LRPS2: played on a real screen with a controller, past the menus; Burnout 3's
     sound when it's busy, which Jake hears stutter a little and scripted crashes didn't reproduce (live
     at 2×, 59.9 fps and never dry, a warm or empty LRPS2 shader cache alike, with Mesa's own cache
     still there; needs a frame log from real play); romi getting
     the core and linking the BIOS itself, as Arch has no package (by hand for now, in `docs/ps2.md`; a
     missing BIOS gets LRPS2's own message in the status line); sharing PCSX2's memory cards (the
     same format; LRPS2 keeps its own in `<system>/pcsx2/memcards`), as Dolphin's are; whether
     `.chd` PS2 discs for achievements (rcheevos can't open them by itself); whether Play!'s sound workaround,
     lump pacing and `glewInit` can go, if Play! stops mattering. Also seen: `romi run <path>` without
     `--system` chose the wrong core ([#1](https://github.com/TitaniteScale/romi/issues/1)).
   - Still to do: Guilty Gear played on a real screen, for a while; Super Metroid in
     bsnes-hd runs at 50 fps (PAL), by the core's own rate and sound, likely its region option;
     saves (Play!'s memory cards, in RetroArch's save folder, not
     PCSX2's); whether PAL games (512 lines) want a different step; more games, as Play!'s
     compatibility is well below PCSX2's.
4. **Steam, and systems with no core** (Switch, PS3, Wii U): 7.3, an emulator or game in a
   compositor with no screen, its frames drawn in the panel. Last, and starting with an experiment
   to see whether `gamescope`'s headless backend works here at all.

## Filling the gaps, any time

Bugs, smaller features and code to tidy are [issues](https://github.com/TitaniteScale/romi/issues), and only what isn't is left here.

- ✓ **Steam game info**: `romi scrape` asks Steam's store (`IStoreBrowseService/GetItems`, the API
  its own pages use, with no key) about every Steam game, in batches of 50. Genres come from the
  tags players voted for most, the year from the first release, and players from the split-screen
  categories and tags. The store also gives the cover's address, under a hashed folder for some
  games, which brought in the four covers the CDN lookup missed.
- **Tests that play leave frames behind**: `e2e/drive.py` ends romi with SIGKILL, and pyte never
  reads the frames, so each test that plays in romi leaves up to 30 in `/dev/shm` (about 150 MB
  for a whole run, until they're deleted or the machine restarts). Stopping romi with Esc, or a
  SIGTERM it cleans up after, would fix it.
- **An AUR package**: the AUR isn't taking new packages. Until it does, the recipe in `packaging/arch`
  builds with `makepkg -si`.
- **A waybar module**, if it turns out to be wanted: games usually run full screen, hiding the bar, so
  it's not planned. `romi status --json` already has what one needs.

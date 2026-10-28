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

Changes made in the TUI are saved in `state.json` (sort orders, tags) or in `overrides.jsonc`, a file
the config includes (hidden games, emulator picks). All done.

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
3. **Audio and input**: audio through `cpal`, resampled to the device rate and used to pace emulation.
   The controller already goes through `gilrs`. Playing on a keyboard needs key-release events, so it
   only works in terminals that support the Kitty keyboard protocol.
   - ✓ **Sound**, through PulseAudio (which PipeWire also serves) or ALSA. Each frame takes as long
     as the sound it made, stretched or shortened by up to half a percent to keep about 40 ms
     queued, so frames stay evenly spaced and the pitch is exact. A core's sample rate turned out
     to be more reliable than its frame rate: mGBA says 61.2 fps for Game Boy games, which run at
     59.7. `romi run --wav` saves a headless run's sound, the same every time.
4. **Saves**: keep in-game saves in RetroArch's save folder, under the same names, so a save made in romi
   carries over to RetroArch and back.
5. **Scenes**: a PNG frame with a transparent screen area. romi draws each emulated frame into it and
   sends the result as one image. Terminals without graphics get an ASCII-art frame around a half-block
   screen.
6. **Full screen**: at first, scale the game to fill the terminal. Later, maybe, hand off to RetroArch
   mid-game through a save state. Whether romi's save states and RetroArch's are compatible still needs
   checking.
7. **RetroAchievements during play**: unlock notifications drawn over the game. This needs `rcheevos`
   (RetroAchievements' C library, compiled into romi) to check each achievement's memory conditions
   against the core's memory every frame. Showing an unlock only needs the achievement list.
   Recording it on the user's account needs a login token from their password, because the web API
   key romi has now only reads.
8. **Launch animation**: a wireframe of the system (a console and a CRT, or a cabinet) builds itself
   out of lines, then the camera dives into its screen and lands on the cells the game is drawn in.
   Each game's file name seeds how the lines enter (pen trace, reverse explode, scan flash, fly in,
   dots then lines, vortex) and in what order.
   - ✓ **Prototype** (`cargo run --release --example wireframe`): an NES and a TV, drawn through
     shared memory like live play, with a stand-in picture for the game. At 1280×720 (half a
     2560×1440 terminal's pixels) a frame takes about 8 ms to draw, and the glass lands within 2 px
     of the game's cells. Models are JSON: vertices in cm, edges, and the screen's four corners.
   - Still to do: run it while the core loads, with a key to skip and a setting to turn it off; a
     model per system; hide lines behind the glass once the game shows on it, since the TV's back
     shows through for a moment. It has only been run in a pseudo-terminal so far, with its frames
     read back from shared memory, so it still needs watching in Ghostty and kitty.

The spike settled 60 fps in Ghostty. Still to check: kitty and other terminals, and how much input
lag there is.

## Filling the gaps, any time

Smaller items that fit between phases.

- **Steam game info**: Steam's public store API gives genres, a description and the release year with no
  key, which makes Steam games filterable. It can also give the hashed cover URLs that the CDN lookup
  misses.
- **Import play history**: Steam play time from Steam's local config, and `playcount`/`lastplayed` from
  ES-DE gamelists, so recently played and the stats include games played outside romi.
- **Exact matches for disc images**: CHD files store a SHA-1 of their contents, which can be matched
  against Redump. That gives PlayStation, Saturn, Dreamcast and the other disc systems the exact matches
  that cartridges already get from checksums.
- **More descriptions**: Wikipedia's summary API needs no key, but matching titles to articles is
  hit-or-miss. ScreenScraper and TheGamesDB need keys.
- **SteamGridDB** (free key) for covers on PC ports and pinball tables.
- **An AUR package**: the recipe is ready in `packaging/aur`, to publish once v0.1.0 is tagged.
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

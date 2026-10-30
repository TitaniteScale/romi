# Working on romi

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

## Releasing

The Arch package's recipe is in `packaging/arch`. It builds from the tagged release's tarball on
GitHub, so it's updated after each tag. (The AUR isn't taking new packages, so it isn't published
there; people build it with `makepkg -si`.) For a release:

1. Set the version in `Cargo.toml`, build so `Cargo.lock` follows, and commit.
2. Tag it and push the tag: `git tag v0.2.0 && git push origin main v0.2.0`.
3. In `packaging/arch`, set `pkgver` to match and `pkgrel=1`, then run `updpkgsums` (from
   `pacman-contrib`) to fill in the checksum of GitHub's tarball for the tag.
4. Build it: `makepkg -f`, or `pkgctl build` (from `devtools`) for a clean chroot that catches
   missing dependencies. `namcap PKGBUILD *.pkg.tar.zst` checks for common mistakes.
5. `makepkg --printsrcinfo > .SRCINFO`, and commit both files.

A fix to the recipe alone, with no new romi release, bumps `pkgrel` instead.

## Code map

| File                   | What's in it                                                                       |
| ---------------------- | ---------------------------------------------------------------------------------- |
| `src/config.rs`        | config structs, JSONC loading, `~` expansion, globbing                             |
| `src/library.rs`       | scanning ROM dirs, ES-DE gamelists, MAME names, name cleanup                       |
| `src/steam.rs`         | installed Steam games from `libraryfolders.vdf` + app manifests                    |
| `src/launch.rs`        | placeholder expansion, spawning, sanity checks for `romi check`                    |
| `src/gamepad.rs`       | controller thread: buttons → actions, hold-to-repeat                               |
| `src/state.rs`         | favorites, history, play time                                                      |
| `src/art.rs`           | box art lookup and `romi scrape` (libretro thumbnails, Steam CDN)                  |
| `src/meta.rs`          | game info: RetroArch databases, Skyscraper, checksums, genres                      |
| `src/filter.rs`        | filtering by genre, players, decade, played, tag                                   |
| `src/sort.rs`          | sort orders for systems and favorites                                              |
| `src/overrides.rs`     | saving changes made in the menus to `overrides.jsonc`                              |
| `src/emucfg.rs`        | emulators' own settings: achievements, controllers                                 |
| `src/emulators.rs`     | the other emulators a game can run with, for its menu and `info`                   |
| `src/titles.rs`        | matching titles against database names (art and metadata share it)                 |
| `src/init.rs`          | `romi init`: ES-DE's system list, finding emulators, writing init.jsonc            |
| `src/add.rs`           | `romi add`: guessing a game entry from a path, appending to games.jsonc            |
| `src/session.rs`       | recording a game's start and end, running games, background play                   |
| `src/stats.rs`         | the session history and the stats worked out from it                               |
| `src/ra.rs`            | RetroAchievements: game lists, your progress, ROM hashing                          |
| `src/hooks.rs`         | `on_launch` and `on_exit` hooks                                                    |
| `src/archive.rs`       | reading ROMs out of zips                                                           |
| `src/files.rs`         | atomic writes, caches keyed by file size and time, temp folders                    |
| `src/text.rs`          | counts, durations, dates and truncation for the TUI and CLI                        |
| `src/main.rs`          | event loop, handing the terminal to `terminal`-mode commands                       |
| `src/cli/mod.rs`       | the command line: arguments, completion, exit codes, dispatch                      |
| `src/cli/browse.rs`    | `romi list` and `romi info`                                                        |
| `src/cli/play.rs`      | `romi play` and `romi run`                                                         |
| `src/cli/setup.rs`     | `romi init`, `romi add`, the first run, `romi check`                               |
| `src/cli/reports.rs`   | `romi status`, `romi stats`, `romi achievements`                                   |
| `src/tui/app.rs`       | app state; keyboard and controller both become the same `Op`s                      |
| `src/tui/ui.rs`        | ratatui drawing                                                                    |
| `src/tui/cover.rs`     | showing covers: decoding and resizing on worker threads, a render cache            |
| `src/tui/cheevos.rs`   | the achievements screen: filters, list and details                                 |
| `src/emu/libretro.rs`  | a minimal libretro frontend for `romi run`: loading cores, frames, scripted input  |
| `src/emu/runner.rs`    | a game running in romi: the core thread, paced by its sound, and its folders       |
| `src/emu/controls.rs`  | game input: keys and controllers as joypad buttons, Select+Start to quit           |
| `src/emu/live.rs`      | `romi run --live`: a game in the whole terminal over Kitty graphics                |
| `src/emu/audio.rs`     | sound for games played in romi: to the speakers, pacing the game, or to a WAV file |
| `src/emu/embedded.rs`  | a game played in the TUI's middle panel, after its launch animation                |
| `src/emu/wireframe.rs` | the launch animation: wireframe models, the seeded plan, drawing, terminal colors  |
| `src/emu/kitty.rs`     | Kitty graphics: Unicode placeholders, shared memory                                |
| `e2e/drive.py`         | runs romi in a fake terminal with a virtual gamepad (tests and by hand)            |
| `e2e/test_*.py`        | end-to-end tests of the TUI and the CLI                                            |

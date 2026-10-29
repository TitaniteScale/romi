# The launcher

The keys and buttons are in the [README](../README.md#controls).

## The cover grid

`v`, L3, or Start → Cover grid shows the games as box art with their names underneath, in place of
the list and the details pane; the selected game's system, year and genre are under the grid. The
d-pad moves around it, left from the first column goes to the systems as in the list, and
everything else (search, filters, sorting, letter jumps) works the same. Games without art get a
box naming their system. romi remembers which view you left it in.

## Sorting

Sorting is remembered per system. Favorites can also go in the order you added them (their
default), and Recent is always newest first. Sorted by year, LT/RT jump between years instead of
letters; sorted by play, they page. From the Start menu, A on Sort keeps trying the next order.
Set `"start_on": "last_played"` to start on the last game you played, in its system, instead of
wherever you left off.

## Filtering

The filter (Start → Filter, `F`, or R3) narrows every view at once by genre, minimum players,
decade, played/not yet, and your tags. Systems with no matches hide while it's on, so "Fighting +
2+ players" becomes one cross-system list. Up/down picks a row, left/right changes it, and the list
updates as you go. Where genres and players come from is in [game info](game-info.md).

## Hiding

Hiding a game (from its menu) takes it out of every list and search. Start → Show hidden games
lists them again, dimmed, so you can unhide them from the same menu. `romi play` still finds
hidden games, and `romi list --hidden` includes them.

## Tags

Tags are your own collections, like "couch co-op" or "backlog". In a game's menu, Tags lists them
with a tick on the game's; A ticks or unticks one, and New tag names another (typed, or with the
on-screen keyboard: ✓ saves). Each tag shows in the systems pane as a collection (`#`), and the
filter has a Tag row, so "backlog + 2 players" works too. A tag goes when its last game leaves it.
Tags are kept in `state.json` with your favorites; `romi list --tag backlog` lists one.

## Emulators

When a system's games can run in something else you have installed (ParaLLEl N64 next to
Mupen64Plus-Next, mGBA's standalone app next to its core), the game's menu has an Emulator entry;
A cycles through them. The others come from ES-DE's list for the system, limited to what's
installed, plus any listed under the system's [`"emulators"`](config.md#other-emulators). The pick
is saved per game in `overrides.jsonc`, with its command, and `romi info` shows a game's emulator
and the others.

## Searching with a controller

Searching with a controller brings up an on-screen keyboard: the d-pad picks a key, A types, X
deletes, Y adds a space, and B (or ✓) closes it, leaving the results to browse. B again leaves the
search.

The controller is read directly (via `gilrs`), since terminals never see gamepad input. romi
ignores it while a game is running and while the terminal window isn't focused, so playing a
detached Steam game won't scroll the list behind it.

## Stats

`S`, Start → Stats, or `romi stats` (with `--json`) show your total play time, play time per
system, your most-played games, and this year month by month with its most-played games. Totals
come from all the play time romi has recorded; the months come from a log of every session,
`history.jsonl` in the state folder. romi started keeping it with the stats screen, so earlier play
only counts in the totals, and the year fills in as you play.

## Play in romi

A game's menu (`o`) has **Play in romi** for games run through a libretro core. It plays the game
in the middle panel, with the systems and details still around it, drawn behind Kitty Unicode
placeholders. Esc, or holding Select+Start, stops it. Arcade games on their side, like Ms. Pac-Man,
are turned upright.

This is experimental: Kitty graphics only, and no in-game saves yet, though what a core keeps
itself (FinalBurn Neo's high scores and NVRAM) goes to RetroArch's save folder for that core, so
it's shared with RetroArch. The first time a core is played, romi starts it in a separate process
for a moment, so one that crashes (Flycast, which only draws with OpenGL or Vulkan) shows an error
instead of taking romi down. Sound plays through PulseAudio (or PipeWire's stand-in for it), or
ALSA without either; `"volume"` in the config sets how loud, from 0 to 100.

The same cores also run from the command line, [headless or full screen](cli.md#running-a-game-headless).

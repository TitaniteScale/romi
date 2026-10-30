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

This is experimental: Kitty graphics only. In-game saves go where RetroArch keeps them: the ROM's
name with `.srm`, in RetroArch's save folder for that core (or next to the ROM, if RetroArch saves
there), so a game saved in romi carries on in RetroArch and back. romi writes the save within a
second of the game saving, and again when you stop. What a core keeps itself (FinalBurn Neo's high
scores and NVRAM) goes to the same folder. A save that's there but can't be read stops the game
from starting, rather than being written over. The first time a core is played, romi starts it in a separate process
for a moment, so one that crashes shows an error instead of taking romi down. Sound plays through
PulseAudio (or PipeWire's stand-in for it), or ALSA without either; `"volume"` in the config sets
how loud, from 0 to 100.

Cores that draw with the GPU work too: Flycast (Dreamcast, Naomi), Mupen64Plus-Next (N64) and
Beetle PSX HW draw with OpenGL, through EGL with no window, on the GPU your screen is plugged into
(on a laptop with two, the other one stays asleep), and romi reads each frame back. Vulkan isn't
there yet, so a core set to draw only with Vulkan won't start. Cores run with the options
RetroArch has for them (`config/<core>/<core>.opt`, and the ones for the game or its folder), so a
game plays the same in both, and with RetroArch's remap for the game, its folder or the core
(`config/remaps/<core>/<name>.rmp`, the first one there): which button each of the pad's buttons
and stick directions sends, and the type of controller plugged in (a DualShock for Ape Escape, say).
Controllers' sticks and analog triggers reach the core as they are;
in games played with the stick (N64 ones), the left stick isn't also the d-pad. On a keyboard, the
arrows push the stick all the way. `ROMI_CORE_LOG=file` writes what the core says there, which is
the place to look when a game won't start.

The picture is square by default, as the game drew it. `"corner_radius"` rounds its corners, as a
percentage of its height: about 4 looks like an old TV. The corners are left clear, so the
terminal's background shows there.

Many games leave black edges round their picture, which a CRT's overscan hid; most N64 games draw
inside a black border. romi crops them off: once a game has drawn for a couple of seconds and the
area it has drawn in covers most of the screen, only that area is shown. It only ever widens
again, if the game draws into an edge. `"crop_borders": false` (or Options → Crop black edges)
shows the whole picture, edges and all.

Games that draw with the GPU (N64, Dreamcast, PlayStation) draw about as big as they're shown,
rather than at the console's 640×480 scaled up, so 3D looks sharp: romi picks the core's
resolution option (Flycast's internal resolution, Mupen64Plus's screen size, Beetle PSX HW's
internal resolution) closest to the panel's height, at least as tall, up to 960 lines (bigger
costs more time than a frame has, as each is read back from the GPU). RetroArch's own setting for
it is left as it is. `"sharp_3d": false` (or Options → Sharp 3D) draws at RetroArch's setting.

`"screen_filter"` gives the picture an old screen's look: `"scanlines"`, `"crt"` (scanlines and a
phosphor mask of red, green and blue columns) or `"lcd"` (a dark grid between the pixels, like a
handheld's). romi draws it into each frame after scaling it, so it works the same in any terminal,
and brightens the picture to make up for the dark lines. They need a few screen pixels for each
of the game's, so a small panel shows less of them. `"none"`, the default, leaves the picture as
the game drew it.

Games start with a launch animation: a wireframe of the system builds itself out of lines across
the panel, then the camera dives into its screen as the game starts on it. Game Boy, Game Boy
Color and Game Boy Advance games get a Game Boy, NES and SNES games an old TV with the console in
front of it, arcade and Neo Geo games an upright cabinet, and every other system the TV on its
own. A game that isn't its screen's shape (the GBA's wide picture on the Game Boy) sits in the
middle of the glass. The game's file name decides how it builds and its color, so a game always starts
the same way.
The color comes from the terminal's own palette (red, green, yellow, blue, magenta, cyan and their
bright versions, leaving out any too close to the background), and the lines are drawn over the
terminal's background. Any key or button skips it. To turn it off, or pick the colors yourself:

```jsonc
"launch_animation": { "enabled": true, "colors": ["#9fe3ff", "#ffb547"] }
```

The same cores also run from the command line, [headless or full screen](cli.md#running-a-game-headless).

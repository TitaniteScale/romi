# Achievements

## RetroAchievements

Once you've [logged in](#earning-them-in-romi), romi shows each game's achievements in the details
pane: how many there are, what they're worth, and how many you've earned. Your account on
retroachievements.org is what's true; what romi keeps in `~/.cache/romi/ra` is a copy. The TUI
refreshes it when it starts, after a game, and when you log in, and `romi achievements` refreshes it
and lists your games that have achievements.

A login can't see your awards (mastered, beaten) or when you earned things. For those, add your web
API key, from Settings on retroachievements.org:

```sh
install -m 600 /dev/stdin ~/.config/romi/retroachievements.key   # paste it, then ctrl-d
```

`"retroachievements": { "key_file": "..." }` says where else it is, and `RA_API_KEY` in the
environment beats both. Keeping it out of `config.json` keeps it out of your dotfiles. Without a
login, the key also needs `"retroachievements": { "username": "you" }`.

Games played in another emulator earn achievements there, so each one needs RetroAchievements
turned on and your login. `romi check` tells you which aren't set up.

## Earning them in romi

Games played in romi (**Play in romi** in a game's menu) earn achievements themselves, once you've
logged in. **RetroAchievements** at the bottom of Options logs in and out, on the on-screen keyboard
from a controller, or from a terminal:

```sh
romi achievements login        # asks for your password; or pipe it in
romi achievements logout
```

If RetroArch is logged in, romi offers to use its login, on a first run and in Options, with no
password to type. Otherwise a hint after your first game says where to log in.

Earning needs your password, once, as a web API key can only read: romi swaps it for the login
token the server gives, the way RetroArch does, and keeps only the token, in
`~/.local/state/romi/retroachievements-login.json` (readable only by you). When a game starts, a
notice over it says how many of its achievements you have; each unlock gets one of its own, with
the achievement's badge, and is sent to your account as it happens. Badges are kept in
`~/.cache/romi/ra/badges/` once shown; `"retroachievements": { "badges": false }` leaves them
out. If the server can't be reached, unlocks wait and go through
when it's back. One still waiting when the game stops, or romi is closed or crashes, is kept in
`~/.local/state/romi/retroachievements-pending.json` (readable only by you, as it holds your
login token) and sent the next time you start a game or open romi, with the time you earned it.
Unlocks are dropped after 30 days.

Some achievements count up, like 12 of 50 coins. While one changes, a small box at the top left of
the game shows how far along it is and which achievement it is, and goes a couple of seconds after
it stops changing, with the achievement's badge beside it.

Challenges (stay alive, don't get hit) are listed under a Challenge heading at the top right while
they're under way, each with its badge when there's room, and go when the achievement is earned
or the challenge fails. `"retroachievements": { "badges": false }` leaves the badges out of these too.

Games that have a rich presence (what you're doing, like "Level 3, 4 lives", which RetroAchievements
shows on your profile) show it in the line under the game, before how to stop, when both fit. It's
read once a second.

romi uses RetroAchievements' own library, rcheevos, so achievements are checked the same way as in
RetroArch. Unlocks are softcore: the server only accepts hardcore unlocks from emulators it has
approved, and treats others' as softcore. Systems are the ones romi plays in the middle panel
(cartridge systems, mostly), and a game is found by its ROM's hash, as in
[How games are matched](#how-games-are-matched).

`ROMI_RA_LOG=/path/to/file` writes rcheevos' warnings to a file, for when something doesn't work:
how it laid out the game's memory, and any achievement it turned off because it reads memory the
core didn't give.

## The achievements screen

`A`, Start → Achievements, or Achievements… in a game's menu lists your games that have
achievements with your progress in each. ←/→ (or LB/RB) narrow it to a system, Tab (or X) to a
status (in progress, not started, finished, or needing another dump), and `/` (or Select) searches
by name. Opening a game lists its achievements: which you've earned (and when, with a web API key), their points, how
rare they are, and which are progression, win conditions or missable. They're fetched when you open
the game and cached, so they show at once next time.

## Steam achievements

Steam achievements show there too, with no setup: romi reads them from the Steam client's own cache
(`appcache/stats`), which covers games played on this machine. Hidden ones stay hidden until
earned, and how rare each is comes from Steam's public percentages when you open the game.

## How games are matched

Games are matched the way RetroArch does it, by hashing the ROM, for arcade sets (the set name) and
cartridges (the ROM, minus a copier header, in N64's native byte order), so a game only shows
achievements if your dump can earn them. When it can't but another version of the game can (often
a "Rev 1"), the details pane says so and links RetroAchievements' list of supported dumps. PlayStation 2, GameCube and Wii discs are matched by the hash of the disc's own files, read
in any format Dolphin plays (`.iso`, `.rvz`, `.wia`, `.ciso`, `.gcz`, `.wbfs`); PS2 `.chd` isn't read yet.
Other disc systems and the DS are matched by title. The first refresh hashes every cartridge, which reads a
few GB; after that only new or changed files are read. Everything is cached in `~/.cache/romi/ra`.

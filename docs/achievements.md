# Achievements

## RetroAchievements

With your username in the config and your web API key (from Settings on retroachievements.org) in
a file of its own, romi shows each game's achievements in the details pane: how many there are,
what they're worth, how many you've earned, and your best award.

```jsonc
"retroachievements": { "username": "you" }
```

The key goes in `~/.config/romi/retroachievements.key` (`chmod 600` it), or `"key_file"` says
where; `RA_API_KEY` in the environment beats both. Keeping it out of `config.json` keeps it out of
your dotfiles. The TUI refreshes your progress when it starts, and `romi achievements` refreshes it
and lists your games that have achievements.

Games played in another emulator earn achievements there, so each one needs RetroAchievements
turned on and your login. `romi check` tells you which aren't set up.

## Earning them in romi

Games played in romi (**Play in romi** in a game's menu) earn achievements themselves, once you've
logged in:

```sh
romi achievements login        # asks for your password; or pipe it in
romi achievements logout
```

The web API key can only read, so earning needs your password, once: romi swaps it for the login
token the server gives, the way RetroArch does, and keeps only the token, in
`~/.local/state/romi/retroachievements-login.json` (readable only by you). When a game starts, a
notice over it says how many of its achievements you have; each unlock gets one of its own, and is
sent to your account as it happens. If the server can't be reached, unlocks wait and go through
when it's back, as long as the game is still running.

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
by name. Opening a game lists its achievements: which you've earned and when, their points, how
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
a "Rev 1"), the details pane says so and links RetroAchievements' list of supported dumps. Disc
systems and the DS are matched by title. The first refresh hashes every cartridge, which reads a
few GB; after that only new or changed files are read. Everything is cached in `~/.cache/romi/ra`.

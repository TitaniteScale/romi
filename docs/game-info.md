# Game info and box art

## Game info

The details pane shows genre, developer, publisher, year, players and a description, merged from
(best first): your config (`"genre"`, `"players"`, `"year"` on a game or override), ES-DE's
gamelists, Skyscraper's cache, and RetroArch's game databases (`~/.config/retroarch/database/rdb`).
RetroArch's databases are matched by ROM checksum where possible (zips are read from their
headers; other cartridges are hashed once and cached in `~/.cache/romi/crc.tsv`), then by file
name, then by title. An exact match also gives box art lookups the precise name to search for.
Arcade genres come from a MAME category list (`metadata.arcade_categories`, e.g. a `catver.ini`
from [progettosnaps](https://www.progettosnaps.net/catver/)). Genres from all these sources are
folded into one short list ("Shoot'em Up / Horizontal" and "Shmup" both become "Shoot 'em up").
The TUI starts instantly and fills database info in a moment later; `romi check` reports coverage.

## Box art

`romi scrape` reports which games have art and downloads what it can. It's safe to rerun: only new
matches are fetched. romi looks in this order, using local files where they are:

1. `"art": "/path/to/image.png"` on a game entry or a system override
2. romi's cache, `~/.cache/romi/art/<system>/`
3. ES-DE's `downloaded_media` (covers, then 3D boxes); set `art.esde_media_dir`
4. Skyscraper's cache (`art.skyscraper_cache`), matched by ROM file name
5. Steam's library cache

Online sources need no accounts or keys:

- **[libretro thumbnails](https://thumbnails.libretro.com)**, RetroArch's box art. romi downloads
  each set's file list once a week (cached in `~/.cache/romi/libretro/`) and matches titles
  locally. It tries the exact name first, then the same words ignoring region tags, then the title
  plus a subtitle, then ignoring dashes. US/World releases win over others, and demos/bootlegs come
  last. Arcade systems match on the MAME title. A system's sets come from its id; override with
  `"libretro": ["Set Name", ...]` on the system.
- **Steam's CDN** for installed Steam games.

**Title screens and screenshots** come from the same places: libretro's `Named_Titles` and
`Named_Snaps` (matched like the covers), ES-DE's `titlescreens` and `screenshots`, and Skyscraper's
`screenshots`, and `romi scrape` gets them along with the covers, kept in `titles/` and `snaps/`
under each system's cache folder. The details pane shows them beside the cover when it's wide
enough, taking turns every five seconds; when it isn't, the cover, title screen and screenshot
take turns in its place, starting from the cover each time you move to another game. `romi info`
gives their paths (`title_screen` and `screenshot` in JSON). Set `"art": { "screens": false }` to
stick to covers.

Covers show at the top of the details pane (and on the "now playing" screen). romi asks the
terminal what it supports at startup: Kitty graphics in Ghostty/kitty, Sixel or iTerm2 elsewhere,
colored half-blocks as a last resort. Images are decoded on background threads, so holding the
d-pad through a long list never stutters. Turn covers off with `"art": { "show": false }`.

Set `"art": { "online": false }` to only use what's already on disk. The art belongs to its
publishers: romi fetches and caches it on your machine and never ships any.

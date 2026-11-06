//! Builds the game list from ROM folders, hand-added entries and Steam.

use crate::config::{self, Config, Emulator, Override, SystemConfig, expand_path, glob_match, missing_bios};
use crate::launch::{Launch, check_command};
use crate::state::State;
use crate::steam;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const ROM_KEYS: &[&str] = &["rom", "file", "name", "dir"];

pub use crate::meta::Meta;

/// Metadata given in the config, which beats anything scraped.
fn configured_meta(description: Option<String>, genre: Option<&str>, players: Option<u8>, year: Option<u16>) -> Meta {
    Meta { description, genres: genre.map(crate::meta::genres).unwrap_or_default(), players, year, ..Meta::default() }
}

#[derive(Debug, Clone)]
pub struct Game {
    /// Stable id used for favorites and history.
    pub key: String,
    pub name: String,
    /// Shown dimmed after the name when two games in a system would otherwise look identical.
    pub suffix: Option<String>,
    pub sort_key: String,
    pub system: String,
    pub path: Option<PathBuf>,
    pub meta: Meta,
    pub launch: Launch,
    pub art: ArtRef,
    /// Every disc, in order, when several are played as one game through a generated `.m3u`.
    /// `path` is the first.
    pub discs: Vec<PathBuf>,
    /// The keys those discs had as games of their own, so old history and scripts still find it.
    pub disc_keys: Vec<String>,
    /// Hidden from lists (from romi's menus or the config's top-level overrides).
    pub hidden: bool,
    /// How the game runs without a top-level override (unnamed), to go back to after another
    /// emulator was picked.
    pub default_run: Emulator,
    /// The emulator picked for it in its menu, by name; None for the default.
    pub emulator: Option<String>,
    /// Buttons remapped for it when it's played in romi, from its overrides.
    pub controls: crate::emu::remap::Controls,
    /// Core options set for it when it's played in romi, from its overrides.
    pub core_options: std::collections::BTreeMap<String, String>,
}

impl Game {
    /// Applies an override from the config's top-level `overrides`, which beats everything else.
    fn apply(&mut self, ov: &Override) {
        if let Some(name) = &ov.name {
            self.name.clone_from(name);
            self.sort_key = sort_key(name);
        }
        if let Some(command) = &ov.command {
            self.launch.command.clone_from(command);
        }
        if let Some(mode) = ov.mode {
            self.launch.mode = mode;
        }
        if let Some(cwd) = &ov.cwd {
            self.launch.cwd = Some(cwd.clone());
        }
        self.launch.env.extend(ov.env.clone());
        self.emulator.clone_from(&ov.emulator);
        if ov.art.is_some() {
            self.set_art_override(ov.art.as_deref());
        }
        let mut meta = configured_meta(None, ov.genre.as_deref(), ov.players, ov.year);
        meta.fill(&self.meta);
        self.meta = meta;
        self.hidden = ov.hidden;
        self.controls.0.extend(ov.controls.0.clone());
        self.core_options.extend(ov.core_options.clone());
    }

    /// Sets the image from the game's top-level override; with none, it's the one set lower down.
    pub fn set_art_override(&mut self, art: Option<&str>) {
        self.art.manual = art.map(expand_path).or_else(|| self.art.configured.clone());
    }
}

/// What box art lookups need to know about a game.
#[derive(Debug, Clone, Default)]
pub struct ArtRef {
    /// Title to search thumbnail sets for: the ROM's file stem, or the MAME title for arcade sets.
    pub title: Option<String>,
    pub appid: Option<String>,
    /// Image set by hand in the config, or added from the game's menu. It comes before all others.
    pub manual: Option<PathBuf>,
    /// The image set on a game entry or a system's override, which a top-level override beats.
    pub configured: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct System {
    pub id: String,
    pub name: String,
    /// Indexes into `Library::games`.
    pub games: Vec<usize>,
    /// Why this system probably can't launch anything (missing emulator or core).
    pub problem: Option<String>,
    /// libretro thumbnail sets to search, in order.
    pub libretro: Vec<String>,
    /// Folder name under ES-DE's media dir and Skyscraper's cache.
    pub esde: String,
    /// ROMs are MAME sets (named by short name, not by title).
    pub arcade: bool,
    pub skyscraper: String,
}

/// MAME short name lookups, from the lists ES-DE ships.
#[derive(Debug, Default)]
struct ArcadeDb {
    names: HashMap<String, String>,
    not_games: HashSet<String>,
}

impl ArcadeDb {
    fn load(dir: &Path) -> ArcadeDb {
        let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap_or_default();
        let names_xml = read("mamenames.xml");
        let short = tag_values(&names_xml, "mamename");
        let real = tag_values(&names_xml, "realname");
        let mut not_games: HashSet<String> = tag_values(&read("mamebioses.xml"), "bios").into_iter().collect();
        not_games.extend(tag_values(&read("mamedevices.xml"), "device"));
        ArcadeDb { names: short.into_iter().zip(real).collect(), not_games }
    }
}

/// BIOS files people keep next to their games, which are never games themselves, though their
/// extensions are (`.bin` for PlayStation discs, `.zip` for Dreamcast's). Matched as `glob_match`
/// does, so without case. The exact names in the systems' "bios" lists count too.
const BIOS_FILES: &[&str] = &[
    // PlayStation and PlayStation 2 ("SCPH-70012.bin" too), and the PSP's PlayStation.
    "scph*.bin",
    "psxonpsp660.bin",
    "ps1_rom.bin",
    "openbios.bin",
    // Saturn, Sega CD, and Genesis Plus GX's boot ROMs.
    "sega_101.bin",
    "mpr-17933.bin",
    "mpr-18811-mx.ic1",
    "mpr-19367-mx.ic1",
    "saturn_bios.bin",
    "bios_cd_?.bin",
    "bios_md.bin",
    "bios_?.sms",
    "bios.gg",
    // Dreamcast, NAOMI and Atomiswave.
    "dc_boot.bin",
    "dc_flash.bin",
    "dc_nvmem.bin",
    "naomi.zip",
    "naomi2.zip",
    "awbios.zip",
    "hod2bios.zip",
    "f355bios.zip",
    "f355dlx.zip",
    "airlbios.zip",
    // Neo Geo and Neo Geo CD.
    "neogeo.zip",
    "neocd*.bin",
    "neocd*.rom",
    "uni-bios*.rom",
    // PC Engine CD.
    "syscard*.pce",
    "gexpress.pce",
    // Nintendo handhelds, the Famicom Disk System and the 64DD.
    "gba_bios.bin",
    "gb_bios.bin",
    "gbc_bios.bin",
    "sgb*_bios.bin",
    "bios7.bin",
    "bios9.bin",
    "firmware.bin",
    "dsi_bios7.bin",
    "dsi_bios9.bin",
    "dsi_firmware.bin",
    "dsi_nand.bin",
    "disksys.rom",
    "64dd_ipl.bin",
    // Others.
    "lynxboot.img",
    "7800 bios (?).rom",
    "5200.rom",
    "colecovision.rom",
    "panafz*.bin",
    "goldstar.bin",
    "sanyotry.bin",
];

/// Whether `file_name` is a BIOS, not a game: one of `BIOS_FILES`, or named in a system's "bios"
/// list. A list's patterns don't count ("pcsx2/bios/*.bin" would be every `.bin`).
fn is_bios(cfg: &Config, file_name: &str) -> bool {
    BIOS_FILES.iter().any(|p| glob_match(p, file_name))
        || cfg.systems.iter().flat_map(|s| &s.bios).flat_map(|b| b.options()).any(|f| {
            let name = f.rsplit('/').next().unwrap_or(f);
            !name.contains(['*', '?']) && name.eq_ignore_ascii_case(file_name)
        })
}

/// Text of every `<tag>...</tag>` in order. These files are flat lists, so no XML parser needed.
fn tag_values(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    xml.match_indices(&open)
        .filter_map(|(i, _)| {
            let start = i + open.len();
            let end = start + xml[start..].find(&close)?;
            Some(unescape(&xml[start..end]))
        })
        .collect()
}

pub(crate) fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Why `Library::resolve` couldn't pick a game.
#[derive(Debug, PartialEq)]
pub enum Resolve {
    NotFound,
    Ambiguous(Vec<usize>),
}

#[derive(Debug, Default, Clone)]
pub struct Library {
    pub games: Vec<Game>,
    pub systems: Vec<System>,
    pub warnings: Vec<String>,
}

impl Library {
    /// The library plus database metadata, for command-line use. The TUI loads metadata in the
    /// background instead (see `App::load_metadata`).
    pub fn load_complete(cfg: &Config) -> Library {
        let mut lib = Library::load(cfg);
        let found = crate::meta::lookup_all(cfg, &lib);
        crate::meta::apply(&mut lib, found);
        lib
    }

    pub fn load(cfg: &Config) -> Library {
        let mut lib = Library::default();
        let roms_dir = expand_path(&cfg.roms_dir);
        let gamelists = cfg.gamelists_dir.as_deref().map(expand_path);

        let arcade = match &cfg.arcade_db {
            Some(dir) if cfg.systems.iter().any(SystemConfig::is_arcade) => Some(ArcadeDb::load(&expand_path(dir))),
            _ => None,
        };

        for sys in &cfg.systems {
            lib.scan_system(cfg, sys, &roms_dir, gamelists.as_deref(), arcade.as_ref());
        }

        for g in &cfg.games {
            let group = lib.group_for(&g.system);
            let launch = Launch {
                command: g.command.clone(),
                mode: g.mode,
                cwd: g.cwd.clone(),
                env: g.env.clone(),
                vars: cfg.vars.clone(),
            };
            let meta = configured_meta(g.description.clone(), g.genre.as_deref(), g.players, g.year);
            let art = ArtRef { manual: g.art.as_deref().map(expand_path), ..ArtRef::default() };
            lib.push_game(group, format!("game:{}", g.name), g.name.clone(), None, meta, launch, art);
        }

        if cfg.steam.enabled
            && let Some(dir) = steam::find_steam_dir(cfg.steam.dir.as_deref())
        {
            let group = lib.group_for(&cfg.steam.name);
            let store = steam::store_cache();
            for app in steam::installed_apps(&dir, &cfg.steam.exclude) {
                let mut vars = cfg.vars.clone();
                vars.insert("appid".into(), app.appid.clone());
                let launch = Launch {
                    command: cfg.steam.command.clone(),
                    mode: cfg.steam.mode,
                    cwd: None,
                    env: HashMap::new(),
                    vars,
                };
                let art = ArtRef { appid: Some(app.appid.clone()), ..ArtRef::default() };
                let meta = store.get(&app.appid).map(|s| s.meta.clone()).unwrap_or_default();
                lib.push_game(group, format!("steam:{}", app.appid), app.name, None, meta, launch, art);
            }
        }

        for game in &mut lib.games {
            game.art.configured.clone_from(&game.art.manual);
            if let Some(ov) = cfg.overrides.get(&game.key) {
                game.apply(ov);
            }
        }
        lib.systems.retain(|s| !s.games.is_empty());
        for sys in &mut lib.systems {
            let games = &lib.games;
            sys.games.sort_by(|&a, &b| games[a].sort_key.cmp(&games[b].sort_key));
            disambiguate(&mut lib.games, &sys.games);
        }
        lib.adopt_disc_history();
        lib
    }

    /// Favorites and play history of discs played before they were merged go to the merged game.
    fn adopt_disc_history(&self) {
        let moves: Vec<(&str, &str)> =
            self.games.iter().flat_map(|g| g.disc_keys.iter().map(|k| (k.as_str(), g.key.as_str()))).collect();
        if moves.is_empty() {
            return;
        }
        let state = State::load();
        if moves.iter().any(|(from, _)| state.mentions(from)) {
            State::update(|s| {
                for (from, to) in &moves {
                    s.rename_key(from, to);
                }
            });
        }
    }

    fn scan_system(
        &mut self,
        cfg: &Config,
        sys: &SystemConfig,
        roms_dir: &Path,
        gamelists: Option<&Path>,
        arcade: Option<&ArcadeDb>,
    ) {
        let arcade = arcade.filter(|_| sys.is_arcade());
        let dir_str = sys.dir.as_deref().unwrap_or(&sys.id);
        let dir = if dir_str.starts_with('/') || dir_str.starts_with('~') {
            expand_path(dir_str)
        } else {
            roms_dir.join(dir_str)
        };
        if !dir.is_dir() {
            return;
        }
        let exts: Vec<String> = sys.extensions.iter().map(|e| e.trim_start_matches('.').to_lowercase()).collect();
        let mut files = Vec::new();
        collect_files(&dir, &dir, sys.recursive, &mut files);

        let meta = gamelists.map(|g| read_gamelist(&g.join(&sys.id).join("gamelist.xml"))).unwrap_or_default();

        let idx = self.systems.len();
        self.systems.push(System {
            id: sys.id.clone(),
            name: cfg.system_name(sys),
            games: Vec::new(),
            problem: check_command(&sys.command, &cfg.vars, ROM_KEYS).or_else(|| {
                // Some cores also accept BIOS files next to the games (neogeo.zip, naomi.zip).
                let mut dirs = cfg.bios_dirs.clone();
                dirs.push(dir.to_string_lossy().into_owned());
                let missing = missing_bios(&sys.bios, &dirs);
                (!missing.is_empty()).then(|| format!("missing BIOS: {}", missing.join(", ")))
            }),
            libretro: sys.libretro.clone().unwrap_or_else(|| crate::art::default_libretro(&sys.id)),
            esde: sys.id.clone(),
            arcade: sys.is_arcade(),
            skyscraper: sys.skyscraper.clone().unwrap_or_else(|| crate::art::default_skyscraper(&sys.id)),
        });

        let mut kept = Vec::new();
        for rel in files {
            let rel_str = rel.to_string_lossy();
            let file_name = rel.file_name().unwrap_or_default().to_string_lossy();
            let ext = rel.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            if !exts.contains(&ext) || sys.exclude.iter().any(|p| glob_match(p, &file_name) || glob_match(p, &rel_str))
            {
                continue;
            }
            if sys.overrides.get(&*rel_str).or_else(|| sys.overrides.get(&*file_name)).is_some_and(|ov| ov.hidden) {
                continue;
            }
            let stem = rel.file_stem().unwrap_or_default().to_string_lossy();
            if arcade.is_some_and(|db| db.not_games.contains(&*stem)) || is_bios(cfg, &file_name) {
                continue;
            }
            kept.push(rel);
        }

        // Systems whose emulators take an .m3u get each multi-disc game as one entry. Discs that
        // an .m3u in the folder already lists are left to it.
        let entries = if exts.iter().any(|e| e == "m3u") {
            let listed = playlist_entries(&dir, &kept);
            kept.retain(|rel| !listed.contains(rel));
            group_discs(kept)
        } else {
            kept.into_iter().map(|rel| vec![rel]).collect()
        };

        for discs in entries {
            if discs.len() == 1 {
                self.push_rom(cfg, sys, idx, &dir, &discs, None, &meta, arcade);
                continue;
            }
            let stem = discs[0].file_stem().unwrap_or_default().to_string_lossy();
            let base = split_disc(&stem).map_or_else(|| stem.to_string(), |(base, _)| base);
            let rel = discs[0].with_file_name(format!("{base}.m3u"));
            let playlist = config::cache_dir().join("m3u").join(&sys.id).join(&rel);
            let paths: Vec<PathBuf> = discs.iter().map(|d| dir.join(d)).collect();
            match write_playlist(&playlist, &paths) {
                Ok(()) => self.push_rom(cfg, sys, idx, &dir, &discs, Some((rel, playlist)), &meta, arcade),
                Err(e) => {
                    self.warnings.push(format!("{}: can't write {}: {e}", sys.id, playlist.display()));
                    for disc in discs {
                        self.push_rom(cfg, sys, idx, &dir, &[disc], None, &meta, arcade);
                    }
                }
            }
        }
    }

    /// Adds a ROM, or several discs of one game with the playlist (relative to the system's
    /// folder, and where it was written) that launches them.
    #[allow(clippy::too_many_arguments)]
    fn push_rom(
        &mut self,
        cfg: &Config,
        sys: &SystemConfig,
        idx: usize,
        dir: &Path,
        discs: &[PathBuf],
        playlist: Option<(PathBuf, PathBuf)>,
        gamelist: &HashMap<String, (Option<String>, Meta)>,
        arcade: Option<&ArcadeDb>,
    ) {
        let rel = &discs[0];
        let path = dir.join(rel);
        let rel_str = rel.to_string_lossy().into_owned();
        let file_name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        // A playlist stands in for the file: it's what gets launched and what the key names.
        let (key_rel, launched) = match &playlist {
            Some((rel, at)) => (rel.to_string_lossy().into_owned(), at.clone()),
            None => (rel_str.clone(), path.clone()),
        };
        let launched_file = launched.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let launched_stem = launched.file_stem().unwrap_or_default().to_string_lossy().into_owned();

        let ov = [&key_rel, &launched_file, &rel_str, &file_name]
            .into_iter()
            .find_map(|k| sys.overrides.get(k))
            .cloned()
            .unwrap_or_default();
        if ov.hidden {
            return;
        }
        let (es_name, es_meta) = gamelist.get(&key_rel).or_else(|| gamelist.get(&rel_str)).cloned().unwrap_or_default();
        // Disc 1's entry in a gamelist may carry its disc number.
        let es_name = es_name.map(|n| if playlist.is_some() { split_disc(&n).map_or(n, |(base, _)| base) } else { n });
        let mut meta = configured_meta(None, ov.genre.as_deref(), ov.players, ov.year);
        meta.fill(&es_meta);
        let mame_title = arcade.and_then(|db| db.names.get(&stem)).cloned();
        // MAME appends alternate titles ("Art of Fighting / Ryuuko no Ken"); show only the first.
        // For arcade sets MAME's own title beats a scraped one (scrapers often mix up sequels).
        let mame_name = mame_title.as_deref().map(|t| clean_name(t.split(" / ").next().unwrap_or(t)));
        let name = ov.name.or(mame_name).or(es_name).unwrap_or_else(|| clean_name(&launched_stem));
        let art = ArtRef {
            // Thumbnail sets use the full title, alternates and all, and name discs separately.
            title: Some(mame_title.unwrap_or_else(|| stem.clone())),
            appid: None,
            manual: ov.art.as_deref().map(expand_path),
            ..ArtRef::default()
        };

        let mut vars = cfg.vars.clone();
        vars.insert("rom".into(), launched.to_string_lossy().into_owned());
        vars.insert("file".into(), launched_file);
        vars.insert("name".into(), launched_stem);
        vars.insert("dir".into(), launched.parent().unwrap_or(dir).to_string_lossy().into_owned());
        let mut env = sys.env.clone();
        env.extend(ov.env);
        let launch = Launch {
            command: ov.command.unwrap_or_else(|| sys.command.clone()),
            mode: ov.mode.unwrap_or(sys.mode),
            cwd: ov.cwd.or_else(|| sys.cwd.clone()),
            env,
            vars,
        };
        let game = self.push_game(idx, format!("{}:{key_rel}", sys.id), name, Some(path), meta, launch, art);
        game.controls = ov.controls;
        game.core_options = ov.core_options;
        if playlist.is_some() {
            game.discs = discs.iter().map(|d| dir.join(d)).collect();
            game.disc_keys = discs.iter().map(|d| format!("{}:{}", sys.id, d.to_string_lossy())).collect();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_game(
        &mut self,
        system: usize,
        key: String,
        name: String,
        path: Option<PathBuf>,
        meta: Meta,
        launch: Launch,
        art: ArtRef,
    ) -> &mut Game {
        let default_run = Emulator {
            name: String::new(),
            command: launch.command.clone(),
            mode: Some(launch.mode),
            cwd: launch.cwd.clone(),
        };
        self.games.push(Game {
            key,
            sort_key: sort_key(&name),
            name,
            suffix: None,
            system: self.systems[system].name.clone(),
            path,
            meta,
            launch,
            art,
            discs: Vec::new(),
            disc_keys: Vec::new(),
            hidden: false,
            default_run,
            emulator: None,
            controls: Default::default(),
            core_options: Default::default(),
        });
        self.systems[system].games.push(self.games.len() - 1);
        self.games.last_mut().expect("just pushed")
    }

    /// Finds a system by id or name, or makes a new group for hand-added games.
    fn group_for(&mut self, label: &str) -> usize {
        if let Some(i) = self.systems.iter().position(|s| s.id == label || s.name == label) {
            return i;
        }
        let id = label.to_lowercase();
        self.systems.push(System {
            esde: id.clone(),
            skyscraper: id.clone(),
            arcade: false,
            id,
            name: label.to_string(),
            games: Vec::new(),
            problem: None,
            libretro: Vec::new(),
        });
        self.systems.len() - 1
    }

    pub fn find(&self, key: &str) -> Option<usize> {
        self.games.iter().position(|g| g.key == key)
    }

    /// The system a game belongs to.
    pub fn system_of(&self, game: usize) -> &System {
        self.systems.iter().find(|s| s.games.contains(&game)).expect("every game is in a system")
    }

    /// Finds the one game a query means, for the command line. Tried in order, stopping at the
    /// first that matches anything: the exact key, the exact name (ignoring case), then names
    /// containing every word of the query. More than one match is an error, never a guess.
    pub fn resolve(&self, query: &str, system: Option<usize>) -> Result<usize, Resolve> {
        let candidates: Vec<usize> = match system {
            Some(s) => self.systems[s].games.clone(),
            None => (0..self.games.len()).collect(),
        };
        let words = |s: &str| s.to_lowercase().split_whitespace().map(str::to_string).collect::<Vec<_>>();
        let query_words = words(query);
        let tries: [&dyn Fn(&Game) -> bool; 3] = [
            &|g| g.key == query || g.disc_keys.iter().any(|k| k == query),
            &|g| g.name.eq_ignore_ascii_case(query.trim()),
            &|g| {
                let name = g.name.to_lowercase();
                !query_words.is_empty() && query_words.iter().all(|w| name.contains(w.as_str()))
            },
        ];
        for matches in tries {
            let found: Vec<usize> = candidates.iter().copied().filter(|&g| matches(&self.games[g])).collect();
            match found.len() {
                0 => continue,
                1 => return Ok(found[0]),
                _ => return Err(Resolve::Ambiguous(found)),
            }
        }
        Err(Resolve::NotFound)
    }
}

fn collect_files(root: &Path, dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        // Follow symlinks so linked ROMs and folders work.
        if path.is_dir() {
            if recursive {
                collect_files(root, &path, recursive, out);
            }
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_path_buf());
        }
    }
}

/// "Final Fantasy VII (USA) (Disc 2)" -> ("Final Fantasy VII (USA)", "2"). "Disk" and
/// "(Disc 2 of 3)" count too.
fn split_disc(stem: &str) -> Option<(String, String)> {
    let mut from = 0;
    while let Some(open) = stem[from..].find('(').map(|i| from + i) {
        let close = open + stem[open..].find(')')?;
        let inner = &stem[open + 1..close];
        let is_disc =
            inner.get(..5).is_some_and(|w| w.eq_ignore_ascii_case("disc ") || w.eq_ignore_ascii_case("disk "));
        if is_disc {
            let label = inner[5..].split(" of ").next().unwrap_or_default().trim();
            if !label.is_empty() && label.len() <= 3 && label.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Some((format!("{}{}", stem[..open].trim_end(), &stem[close + 1..]), label.to_string()));
            }
        }
        from = close;
    }
    None
}

/// Groups discs of the same game (same folder, extension and name apart from the disc number),
/// in disc order. Everything else, and a lone disc, stays on its own.
fn group_discs(files: Vec<PathBuf>) -> Vec<Vec<PathBuf>> {
    let mut entries: Vec<Vec<(String, PathBuf)>> = Vec::new();
    let mut groups: HashMap<(PathBuf, String, String), usize> = HashMap::new();
    for rel in files {
        let stem = rel.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let Some((base, label)) = split_disc(&stem) else {
            entries.push(vec![(String::new(), rel)]);
            continue;
        };
        let ext = rel.extension().unwrap_or_default().to_string_lossy().to_lowercase();
        let key = (rel.parent().unwrap_or(Path::new("")).to_path_buf(), base, ext);
        let i = *groups.entry(key).or_insert_with(|| {
            entries.push(Vec::new());
            entries.len() - 1
        });
        entries[i].push((label, rel));
    }
    let mut out = Vec::new();
    for mut discs in entries {
        discs.sort_by_key(|(label, _)| (label.parse::<u32>().unwrap_or(u32::MAX), label.to_lowercase()));
        // Two files claiming the same disc means these aren't one set; leave them be.
        if discs.windows(2).any(|w| w[0].0.eq_ignore_ascii_case(&w[1].0)) {
            out.extend(discs.into_iter().map(|(_, rel)| vec![rel]));
        } else {
            out.push(discs.into_iter().map(|(_, rel)| rel).collect());
        }
    }
    out
}

/// Files (relative to `dir`) that the .m3u playlists among `files` list.
fn playlist_entries(dir: &Path, files: &[PathBuf]) -> HashSet<PathBuf> {
    let mut listed = HashSet::new();
    for rel in files.iter().filter(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case("m3u"))) {
        let Ok(text) = std::fs::read_to_string(dir.join(rel)) else { continue };
        let base = dir.join(rel.parent().unwrap_or(Path::new("")));
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            if let Ok(entry) = base.join(line).strip_prefix(dir) {
                listed.insert(entry.to_path_buf());
            }
        }
    }
    listed
}

/// Writes an .m3u of absolute disc paths, unless it already says exactly that.
fn write_playlist(path: &Path, discs: &[PathBuf]) -> std::io::Result<()> {
    let text: String = discs.iter().map(|d| format!("{}\n", d.display())).collect();
    if std::fs::read_to_string(path).is_ok_and(|old| old == text) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)
}

/// Reads ES-DE's gamelist.xml into `relative path -> (name, metadata)`.
fn read_gamelist(path: &Path) -> HashMap<String, (Option<String>, Meta)> {
    let mut map = HashMap::new();
    let Ok(xml) = std::fs::read_to_string(path) else { return map };
    let Ok(doc) = roxmltree::Document::parse(&xml) else { return map };
    for game in doc.descendants().filter(|n| n.has_tag_name("game")) {
        let field = |tag: &str| {
            game.children()
                .find(|c| c.has_tag_name(tag))
                .and_then(|c| c.text())
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
        };
        let Some(p) = field("path") else { continue };
        let rel = p.trim_start_matches("./").to_string();
        let meta = Meta {
            description: field("desc"),
            developer: field("developer"),
            publisher: field("publisher"),
            genres: field("genre").map(|g| crate::meta::genres(&g)).unwrap_or_default(),
            year: field("releasedate").and_then(|d| crate::meta::year(&d)),
            players: field("players").and_then(|p| crate::meta::players(&p)),
            franchise: None,
        };
        map.insert(rel, (field("name"), meta));
    }
    map
}

/// "King of Fighters 2002, The (Japan) (En,Ja)" -> "The King of Fighters 2002".
/// Disc/side numbers are kept since they tell entries apart.
pub fn clean_name(stem: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    let mut group = String::new();
    for c in stem.chars() {
        match c {
            '(' | '[' => {
                depth += 1;
                if depth == 1 {
                    group.clear();
                    continue;
                }
            }
            ')' | ']' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    let g = group.trim();
                    let lower = g.to_lowercase();
                    if lower.starts_with("disc") || lower.starts_with("disk") || lower.starts_with("side") {
                        out.push_str(&format!("({g})"));
                    }
                    continue;
                }
            }
            _ => {}
        }
        if depth > 0 {
            group.push(c);
        } else {
            out.push(c);
        }
    }
    let mut name = out.split_whitespace().collect::<Vec<_>>().join(" ");
    for article in ["The", "A", "An"] {
        let pat = format!(", {article}");
        if let Some(pos) = name.find(&pat) {
            let after = &name[pos + pat.len()..];
            if after.is_empty() || after.starts_with(' ') {
                name = format!("{article} {}{}", &name[..pos], after);
                break;
            }
        }
    }
    if name.is_empty() { stem.to_string() } else { name }
}

fn sort_key(name: &str) -> String {
    let lower = name.to_lowercase();
    let trimmed = lower.strip_prefix("the ").unwrap_or(&lower);
    trimmed.chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect()
}

/// Same display name twice in one system (regional variants, .zip next to .md): show file names.
fn disambiguate(games: &mut [Game], ids: &[usize]) {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for &i in ids {
        *counts.entry(games[i].name.as_str()).or_default() += 1;
    }
    let dupes: Vec<usize> = ids.iter().copied().filter(|&i| counts[games[i].name.as_str()] > 1).collect();
    for i in dupes {
        games[i].suffix = games[i].path.as_ref().and_then(|p| p.file_name()).map(|f| f.to_string_lossy().into_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(clean_name("Breath of Fire III (USA)"), "Breath of Fire III");
        assert_eq!(clean_name("King of Fighters 2002, The (Japan) (En,Ja)"), "The King of Fighters 2002");
        assert_eq!(
            clean_name("King of Fighters, The - Dream Match 1999 (USA) (En,Ja,Es,Pt)"),
            "The King of Fighters - Dream Match 1999"
        );
        assert_eq!(clean_name("Mr. Bones (USA) (Disc 1)"), "Mr. Bones (Disc 1)");
        assert_eq!(clean_name("Apotris"), "Apotris");
        assert_eq!(clean_name("(weird)"), "(weird)");
    }

    #[test]
    fn bios_files_arent_games() {
        let mut cfg = Config::default();
        let bios = r#"[["mine.rom", "MORE.ROM"], "pcsx2/bios/*.bin", "dc/extra.zip"]"#;
        let sys = format!(r#"{{"id": "x", "extensions": ["rom"], "command": "true", "bios": {bios}}}"#);
        cfg.systems.push(serde_json::from_str(&sys).unwrap());
        for bios in
            ["scph1001.bin", "SCPH-70012.bin", "naomi.zip", "neogeo.zip", "bios_CD_U.bin", "more.rom", "extra.zip"]
        {
            assert!(is_bios(&cfg, bios), "{bios}");
        }
        for game in ["Crazy Taxi (USA).chd", "Ape Escape (USA) (Track 1).bin", "aof.zip", "other.bin", "mine.rom.zip"] {
            assert!(!is_bios(&cfg, game), "{game}");
        }
    }

    #[test]
    fn flat_xml_lists() {
        let xml = "<game>\n<mamename>sfa3u</mamename>\n<realname>Street Fighter Alpha 3 (USA)</realname>\n</game>\n<game><mamename>x</mamename><realname>A &amp; B</realname></game>";
        assert_eq!(tag_values(xml, "realname"), ["Street Fighter Alpha 3 (USA)", "A & B"]);
        assert_eq!(tag_values(xml, "mamename"), ["sfa3u", "x"]);
    }

    #[test]
    fn disc_numbers() {
        let split = split_disc;
        assert_eq!(split("Final Fantasy VII (USA) (Disc 2)"), Some(("Final Fantasy VII (USA)".into(), "2".into())));
        assert_eq!(
            split("Metal Gear Solid (USA) (Disc 1) (Rev 1)"),
            Some(("Metal Gear Solid (USA) (Rev 1)".into(), "1".into()))
        );
        assert_eq!(split("Riven (USA) (Disk 3 of 5)"), Some(("Riven (USA)".into(), "3".into())));
        assert_eq!(split("Discworld (Europe)"), None);
        assert_eq!(split("Mr. Bones (Discography)"), None);
        assert_eq!(split("Apotris"), None);
    }

    #[test]
    fn discs_group_in_order() {
        let files = [
            "Chrono Cross (USA) (Disc 2).chd",
            "Crash Bandicoot (USA).chd",
            "Chrono Cross (USA) (Disc 1).chd",
            "Final Fantasy IX (USA) (Disc 1).chd",
            "Final Fantasy IX (USA) (Disc 1).cue",
            "other/Chrono Cross (USA) (Disc 3).chd",
        ];
        let groups = group_discs(files.iter().map(PathBuf::from).collect());
        let names: Vec<Vec<&str>> = groups.iter().map(|g| g.iter().map(|p| p.to_str().unwrap()).collect()).collect();
        assert_eq!(
            names,
            [
                vec!["Chrono Cross (USA) (Disc 1).chd", "Chrono Cross (USA) (Disc 2).chd"],
                vec!["Crash Bandicoot (USA).chd"],
                vec!["Final Fantasy IX (USA) (Disc 1).chd"],
                vec!["Final Fantasy IX (USA) (Disc 1).cue"],
                vec!["other/Chrono Cross (USA) (Disc 3).chd"],
            ]
        );
    }

    #[test]
    fn sorting_ignores_the() {
        assert!(sort_key("The King of Fighters") > sort_key("Bomberman"));
        assert!(sort_key("The King of Fighters") < sort_key("Metal Slug"));
    }
}

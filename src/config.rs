//! Config file: `~/.config/romi/config.json` (JSON with `//` and `/* */` comments allowed).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG: &str = include_str!("../default-config.jsonc");

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Base directory that relative system `dir`s are resolved against.
    pub roms_dir: String,
    /// ES-DE gamelists directory; used for nicer names and descriptions when present.
    pub gamelists_dir: Option<String>,
    /// Folder with ES-DE's mamenames.xml / mamebioses.xml / mamedevices.xml, used by systems
    /// marked `"arcade": true` to turn short names like `sfa3u` into titles and hide BIOS sets.
    pub arcade_db: Option<String>,
    /// Folder with ES-DE's es_systems.xml and es_find_rules.xml, which `romi init` and
    /// `romi check` use to know systems and emulators. Only needed if ES-DE isn't installed in the
    /// usual place.
    pub esde_dir: Option<String>,
    /// Where relative `bios` files are looked for, in order.
    pub bios_dirs: Vec<String>,
    /// Extra `{placeholders}` usable in any command, cwd or env value.
    pub vars: HashMap<String, String>,
    /// Which game is selected when romi starts.
    pub start_on: StartOn,
    /// Where in the world names come from, for systems known by different names in different
    /// places (Genesis or Mega Drive) and given no name of their own.
    pub region: Region,
    /// Commands run when any game starts and stops. A system's own hooks run as well.
    pub hooks: Hooks,
    pub theme: Theme,
    pub controller: ControllerConfig,
    /// How loud games played inside romi are, from 0 (silent) to 100.
    pub volume: u8,
    /// How round the corners of a game played in the middle panel are, as a percentage of its
    /// height: 0 is square, about 4 is an old TV's.
    pub corner_radius: u8,
    /// An old screen's look for a game played in the middle panel: scanlines, a CRT's or an LCD's.
    pub screen_filter: crate::emu::screen::Filter,
    /// Black edges a game leaves round its picture (most N64 games do) are cropped off in the
    /// middle panel (see `emu::crop`).
    pub crop_borders: bool,
    /// Games that draw with the GPU draw about as big as they're shown, rather than at the
    /// console's size, so 3D looks sharp (see `emu::options`).
    pub sharp_3d: bool,
    pub systems: Vec<SystemConfig>,
    pub games: Vec<GameConfig>,
    /// Tweaks to any game (ROM, hand-added or Steam) by its key, as `romi list` shows it.
    /// romi's menus write these to `overrides.jsonc`.
    pub overrides: HashMap<String, Override>,
    pub steam: SteamConfig,
    pub art: ArtConfig,
    pub launch_animation: LaunchAnimation,
    pub metadata: MetadataConfig,
    pub retroachievements: RetroAchievementsConfig,
    /// More config files whose systems, games and vars are added to this one's (see `Include`).
    /// Relative paths are relative to this file's folder.
    pub include: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartOn {
    /// Wherever you were when romi last closed.
    #[default]
    LastSelected,
    /// The last game you played, in its system.
    LastPlayed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Region {
    /// From the locale: the United States and Canada are North America, Japan is Japan, and
    /// everywhere else (or no locale) gets the names used in most of the world, Europe's.
    #[default]
    Auto,
    /// North America: Genesis, Sega CD, TurboGrafx-16.
    Na,
    /// Europe, and most of the world: Mega Drive, Mega-CD, PC Engine.
    Eu,
    /// Japan: Mega Drive, Super Famicom, Famicom.
    Jp,
}

impl Region {
    /// `Auto` worked out from the locale; any other region as it is.
    pub fn resolve(self) -> Region {
        if self != Region::Auto {
            return self;
        }
        let locale =
            ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|v| std::env::var(v).ok().filter(|l| !l.is_empty()));
        Region::of_locale(locale.as_deref().unwrap_or(""))
    }

    /// The region of a locale like `en_US.UTF-8`, by its territory.
    fn of_locale(locale: &str) -> Region {
        let territory = locale.split(['.', '@']).next().unwrap_or("").split_once('_').map(|(_, t)| t);
        match territory {
            Some("US" | "CA") => Region::Na,
            Some("JP") => Region::Jp,
            _ => Region::Eu,
        }
    }
}

/// Systems known by different names in different places: the ids they go by, then their names
/// in North America, Europe and Japan.
const REGIONAL_NAMES: &[(&[&str], [&str; 3])] = &[
    (&["megadrive", "genesis"], ["Genesis", "Mega Drive", "Mega Drive"]),
    (&["scd", "segacd", "megacd"], ["Sega CD", "Mega-CD", "Mega-CD"]),
    (&["sega32x", "32x"], ["32X", "Mega Drive 32X", "Super 32X"]),
    (&["snes", "sfc", "superfamicom"], ["Super Nintendo", "Super Nintendo", "Super Famicom"]),
    (&["nes", "famicom"], ["NES", "NES", "Famicom"]),
    (&["pcengine", "tg16"], ["TurboGrafx-16", "PC Engine", "PC Engine"]),
    (&["pcenginecd", "tg-cd"], ["TurboGrafx-CD", "PC Engine CD", "PC Engine CD"]),
];

/// What the system with this id is called in `region`, if that depends on where you are.
pub fn regional_name(id: &str, region: Region) -> Option<&'static str> {
    let (_, names) = REGIONAL_NAMES.iter().find(|(ids, _)| ids.contains(&id))?;
    Some(match region.resolve() {
        Region::Na => names[0],
        Region::Jp => names[2],
        _ => names[1],
    })
}

/// What an included file may contain. `romi init` writes its findings to one, so they stay
/// apart from what you write by hand. Everything here only fills gaps: a system or var that the
/// main config also defines is the main config's.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Include {
    pub vars: HashMap<String, String>,
    pub bios_dirs: Vec<String>,
    pub systems: Vec<SystemConfig>,
    pub games: Vec<GameConfig>,
    pub overrides: HashMap<String, Override>,
}

impl Config {
    /// What a system is called: its own name, or for want of one its name in `region`, or its id.
    pub fn system_name(&self, sys: &SystemConfig) -> String {
        match &sys.name {
            Some(name) => name.clone(),
            None => regional_name(&sys.id, self.region).unwrap_or(&sys.id).to_string(),
        }
    }

    /// `volume` as what sound samples are multiplied by: 1 at 100.
    pub fn volume_gain(&self) -> f32 {
        f32::from(self.volume.min(100)) / 100.0
    }

    /// `corner_radius` as a fraction of the picture's height, at most half of it.
    pub fn corners(&self) -> f64 {
        f64::from(self.corner_radius.min(50)) / 100.0
    }

    /// The first system that lists `ext` among its file extensions.
    pub fn system_for_extension(&self, ext: &str) -> Option<&SystemConfig> {
        self.systems.iter().find(|s| s.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)))
    }

    /// The folder libretro cores romi runs get as their system folder, for BIOS files and their
    /// own data: the first of `bios_dirs` that exists (RetroArch's, unless changed).
    pub fn system_dir(&self) -> Option<PathBuf> {
        self.bios_dirs.iter().map(|d| expand_path(d)).find(|d| d.is_dir())
    }

    fn merge(&mut self, inc: Include) {
        for (k, v) in inc.vars {
            self.vars.entry(k).or_insert_with(|| expand_tilde(&v));
        }
        for dir in inc.bios_dirs {
            if !self.bios_dirs.contains(&dir) {
                self.bios_dirs.push(dir);
            }
        }
        for sys in inc.systems {
            if !self.systems.iter().any(|s| s.id == sys.id) {
                self.systems.push(sys);
            }
        }
        for game in inc.games {
            if !self.games.iter().any(|g| g.name == game.name) {
                self.games.push(game);
            }
        }
        for (key, ov) in inc.overrides {
            self.overrides.entry(key).or_default().fill(ov);
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            roms_dir: "~/ROMs".into(),
            gamelists_dir: None,
            arcade_db: None,
            esde_dir: None,
            bios_dirs: vec!["~/.config/retroarch/system".into()],
            vars: HashMap::new(),
            start_on: StartOn::default(),
            region: Region::default(),
            hooks: Hooks::default(),
            theme: Theme::default(),
            controller: ControllerConfig::default(),
            volume: 100,
            corner_radius: 0,
            screen_filter: crate::emu::screen::Filter::None,
            crop_borders: true,
            sharp_3d: true,
            systems: Vec::new(),
            games: Vec::new(),
            overrides: HashMap::new(),
            steam: SteamConfig::default(),
            art: ArtConfig::default(),
            launch_animation: LaunchAnimation::default(),
            metadata: MetadataConfig::default(),
            retroachievements: RetroAchievementsConfig::default(),
            include: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum LaunchMode {
    /// Hide output, show a "now playing" screen, and come back when the game exits.
    #[default]
    Wait,
    /// Hand the terminal over to the command (for scripts that print or prompt).
    Terminal,
    /// Fire and forget; romi stays usable. Needed for launchers that return immediately (Steam).
    Detach,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemConfig {
    /// Short id; also the ES-DE gamelist folder name.
    pub id: String,
    /// Shown everywhere. Systems known by different names in different places (Genesis or Mega
    /// Drive) can leave it out to be named for `region`; others are named by their id without it.
    #[serde(default)]
    pub name: Option<String>,
    /// ROM folder, relative to `roms_dir` unless absolute or `~`-prefixed. Defaults to `id`.
    pub dir: Option<String>,
    /// File extensions to list, without the dot. Case-insensitive.
    pub extensions: Vec<String>,
    pub command: String,
    #[serde(default)]
    pub mode: LaunchMode,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Glob patterns (`*`, `?`) matched against file names to skip, e.g. BIOS files.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Also scan subdirectories.
    #[serde(default)]
    pub recursive: bool,
    /// ROMs use MAME short names; look up titles and hide BIOS/device sets via `arcade_db`.
    #[serde(default)]
    pub arcade: bool,
    /// libretro thumbnail sets to search for art, in order. Known system ids have defaults.
    pub libretro: Option<Vec<String>>,
    /// Skyscraper cache platform folder, when it differs from `id` (e.g. `segacd`).
    pub skyscraper: Option<String>,
    /// Files the emulator needs, relative to a `bios_dirs` entry or absolute. Each entry is a file,
    /// or a list where any one will do. File names may use `*` and `?`.
    #[serde(default)]
    pub bios: Vec<BiosFile>,
    /// Per-file tweaks, keyed by path relative to the system dir (usually just the file name).
    #[serde(default)]
    pub overrides: HashMap<String, Override>,
    /// Other ways to run this system's games, offered in each game's menu along with the
    /// installed emulators ES-DE knows for the system.
    #[serde(default)]
    pub emulators: Vec<Emulator>,
    /// Commands run when one of this system's games starts and stops, after the global ones
    /// (and before them when it stops).
    #[serde(default)]
    pub hooks: Hooks,
}

/// Commands run around a game, with the same placeholders as launch commands plus `{game}`,
/// `{key}` and `{system}`, and `{exit_code}` and `{played_secs}` once it's stopped.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Hooks {
    /// Before the game starts. romi waits for each (up to a few seconds).
    pub on_launch: Commands,
    /// After it exits.
    pub on_exit: Commands,
}

/// One command, or several run in order.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Commands {
    One(String),
    Many(Vec<String>),
}

impl Default for Commands {
    fn default() -> Self {
        Commands::Many(Vec::new())
    }
}

impl Commands {
    pub fn list(&self) -> &[String] {
        match self {
            Commands::One(c) => std::slice::from_ref(c),
            Commands::Many(cs) => cs,
        }
    }
}

/// A named command for running a system's games.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Emulator {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub mode: Option<LaunchMode>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum BiosFile {
    One(String),
    AnyOf(Vec<String>),
}

impl BiosFile {
    pub fn options(&self) -> &[String] {
        match self {
            BiosFile::One(f) => std::slice::from_ref(f),
            BiosFile::AnyOf(fs) => fs,
        }
    }
}

/// Returns the BIOS requirements that none of `dirs` satisfy, as "a or b" descriptions.
pub fn missing_bios(required: &[BiosFile], dirs: &[String]) -> Vec<String> {
    let dirs: Vec<PathBuf> = dirs.iter().map(|d| expand_path(d)).collect();
    required
        .iter()
        .filter(|req| !req.options().iter().any(|f| bios_exists(f, &dirs)))
        .map(|req| req.options().join(" or "))
        .collect()
}

fn bios_exists(file: &str, dirs: &[PathBuf]) -> bool {
    let candidates: Vec<PathBuf> = if file.starts_with('/') || file.starts_with('~') {
        vec![expand_path(file)]
    } else {
        dirs.iter().map(|d| d.join(file)).collect()
    };
    candidates.iter().any(|p| {
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        if !name.contains(['*', '?']) {
            return p.is_file();
        }
        let Some(Ok(entries)) = p.parent().map(std::fs::read_dir) else { return false };
        entries.flatten().any(|e| e.path().is_file() && glob_match(&name, &e.file_name().to_string_lossy()))
    })
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Override {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The name of the emulator picked in the game's menu, which wrote its command here too.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emulator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<LaunchMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    /// In a system's overrides: not a game at all (a BIOS, a demo disc). In the top-level ones:
    /// left out of lists until "Show hidden games", but still playable by name.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// Image to use instead of scraped art.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
    /// Metadata that beats anything scraped, e.g. for filters: "Fighting", "Shoot 'em up / Puzzle".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub players: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<u16>,
}

impl Override {
    /// Takes whatever this doesn't say from `other`.
    fn fill(&mut self, other: Override) {
        fn pick<T>(a: &mut Option<T>, b: Option<T>) {
            if a.is_none() {
                *a = b;
            }
        }
        pick(&mut self.name, other.name);
        // An emulator's name, command, mode and cwd go together.
        if self.command.is_none() {
            pick(&mut self.emulator, other.emulator);
            self.command = other.command;
            pick(&mut self.mode, other.mode);
            pick(&mut self.cwd, other.cwd);
        }
        for (k, v) in other.env {
            self.env.entry(k).or_insert(v);
        }
        self.hidden |= other.hidden;
        pick(&mut self.art, other.art);
        pick(&mut self.genre, other.genre);
        pick(&mut self.players, other.players);
        pick(&mut self.year, other.year);
    }
}

/// A hand-added entry: anything that can be started with a command.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameConfig {
    pub name: String,
    pub command: String,
    /// Group to show it under: an existing system id, or any label (a new group is made).
    #[serde(default = "default_group")]
    pub system: String,
    #[serde(default)]
    pub mode: LaunchMode,
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub description: Option<String>,
    /// Image to show for this entry.
    pub art: Option<String>,
    pub genre: Option<String>,
    pub players: Option<u8>,
    pub year: Option<u16>,
}

fn default_group() -> String {
    "PC".into()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SteamConfig {
    pub enabled: bool,
    pub name: String,
    /// Steam install dir; auto-detected when unset.
    pub dir: Option<String>,
    pub command: String,
    pub mode: LaunchMode,
    /// App ids or exact names to hide (Proton and runtimes are always hidden).
    pub exclude: Vec<String>,
}

impl Default for SteamConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            name: "Steam".into(),
            dir: None,
            command: "steam steam://rungameid/{appid}".into(),
            mode: LaunchMode::Detach,
            exclude: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MetadataConfig {
    /// RetroArch's game databases (`*.rdb`), named like the libretro thumbnail sets.
    pub rdb_dir: Option<String>,
    /// Identify cartridge ROMs by CRC32 (read once, then cached) for exact database matches.
    pub checksums: bool,
    /// MAME category list (catver.ini or AdvanceMAME's category.ini) for arcade genres, which
    /// RetroArch's MAME database doesn't have.
    pub arcade_categories: Option<String>,
}

impl Default for MetadataConfig {
    fn default() -> Self {
        Self { rdb_dir: Some("~/.config/retroarch/database/rdb".into()), checksums: true, arcade_categories: None }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ArtConfig {
    /// ES-DE's downloaded_media folder; its covers are used as-is.
    pub esde_media_dir: Option<String>,
    /// Skyscraper's cache folder; its covers are used as-is.
    pub skyscraper_cache: Option<String>,
    /// Let `romi scrape` download from libretro thumbnails and Steam's CDN.
    pub online: bool,
    /// Show covers in the details pane (Kitty/Sixel/iTerm2 graphics, or half-blocks elsewhere).
    pub show: bool,
    /// Title screens and screenshots too: `romi scrape` gets them, and the details pane shows
    /// them beside the cover or in turn with it.
    pub screens: bool,
}

impl Default for ArtConfig {
    fn default() -> Self {
        Self {
            esde_media_dir: Some("~/ES-DE/downloaded_media".into()),
            skyscraper_cache: Some("~/.skyscraper/cache".into()),
            online: true,
            show: true,
            screens: true,
        }
    }
}

/// The wireframe that builds itself when a game starts inside romi, for systems that have one.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LaunchAnimation {
    pub enabled: bool,
    /// The colors the beam is picked from, as `#rrggbb`. Empty uses the terminal's own palette.
    pub colors: Vec<String>,
}

impl Default for LaunchAnimation {
    fn default() -> Self {
        Self { enabled: true, colors: Vec::new() }
    }
}

/// RetroAchievements: which games have achievements, and your progress in them.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetroAchievementsConfig {
    /// Your username on retroachievements.org. Without it, romi leaves achievements out.
    pub username: Option<String>,
    /// A file holding your web API key (retroachievements.org → Settings → Keys), kept out of
    /// this file so the config can be shared. `RA_API_KEY` in the environment overrides it.
    pub key_file: String,
    /// Show an achievement's badge in the notice when it unlocks in a game played in romi.
    pub badges: bool,
}

impl Default for RetroAchievementsConfig {
    fn default() -> Self {
        Self { username: None, key_file: "~/.config/romi/retroachievements.key".into(), badges: true }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
    /// Any of the 16 terminal colors by name, `default`, a 0-255 index, or `#rrggbb`.
    pub accent: String,
    /// `rounded`, `plain`, `thick`, `double`, or `none`.
    pub borders: String,
    pub show_details: bool,
}

impl Default for Theme {
    fn default() -> Self {
        Self { accent: "blue".into(), borders: "rounded".into(), show_details: true }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ControllerConfig {
    pub enabled: bool,
    /// Use the right face button to confirm (Nintendo style) instead of the bottom one.
    pub swap_confirm: bool,
    /// Ignore the controller while the terminal window isn't focused.
    pub require_focus: bool,
}

impl Default for ControllerConfig {
    fn default() -> Self {
        Self { enabled: true, swap_confirm: false, require_focus: true }
    }
}

pub fn config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("ROMI_CONFIG") {
        return PathBuf::from(p);
    }
    dirs::config_dir().unwrap_or_else(|| home().join(".config")).join("romi/config.json")
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(|| home().join(".cache")).join("romi")
}

pub fn state_dir() -> PathBuf {
    dirs::state_dir().unwrap_or_else(|| home().join(".local/state")).join("romi")
}

/// The config's files: the one at `path`, then the ones it includes.
pub fn files(path: &Path, cfg: &Config) -> Vec<PathBuf> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::iter::once(path.to_path_buf()).chain(cfg.include.iter().map(|inc| dir.join(expand_path(inc)))).collect()
}

/// Loads the config and the files it includes.
pub fn load(path: &Path) -> Result<Config> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut cfg = parse(&raw).with_context(|| format!("parsing {}", path.display()))?;
    for inc_path in files(path, &cfg).into_iter().skip(1) {
        let raw = std::fs::read_to_string(&inc_path)
            .with_context(|| format!("reading {} (included from {})", inc_path.display(), path.display()))?;
        let stripped = json_comments::StripComments::new(raw.as_bytes());
        let included = serde_json::from_reader(stripped).with_context(|| format!("parsing {}", inc_path.display()))?;
        cfg.merge(included);
    }
    Ok(cfg)
}

pub fn parse(raw: &str) -> Result<Config> {
    let stripped = json_comments::StripComments::new(raw.as_bytes());
    let mut cfg: Config = serde_json::from_reader(stripped)?;
    // Expand `~` once up front so vars work mid-token (`--path={cores}`).
    for v in cfg.vars.values_mut() {
        *v = expand_tilde(v);
    }
    Ok(cfg)
}

pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// `path` with the home folder written as `~`, for showing.
pub fn tilde(path: &Path) -> String {
    match path.strip_prefix(home()) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

pub fn expand_tilde(s: &str) -> String {
    if s == "~" {
        home().to_string_lossy().into_owned()
    } else if let Some(rest) = s.strip_prefix("~/") {
        home().join(rest).to_string_lossy().into_owned()
    } else {
        s.to_string()
    }
}

pub fn expand_path(s: &str) -> PathBuf {
    PathBuf::from(expand_tilde(s))
}

/// Case-insensitive glob supporting `*` and `?`.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_come_from_the_locales_territory() {
        assert_eq!(Region::of_locale("en_US.UTF-8"), Region::Na);
        assert_eq!(Region::of_locale("fr_CA"), Region::Na);
        assert_eq!(Region::of_locale("ja_JP.UTF-8"), Region::Jp);
        assert_eq!(Region::of_locale("en_GB.UTF-8"), Region::Eu);
        assert_eq!(Region::of_locale("pt_BR@latin"), Region::Eu);
        assert_eq!(Region::of_locale("C"), Region::Eu);
        assert_eq!(Region::of_locale(""), Region::Eu);
    }

    #[test]
    fn systems_without_a_name_are_named_for_the_region() {
        let mut cfg = parse(
            r#"{ "systems": [
                { "id": "megadrive", "extensions": ["md"], "command": "x {rom}" },
                { "id": "genesis", "name": "My Genesis", "extensions": ["md"], "command": "x {rom}" },
                { "id": "gba", "extensions": ["gba"], "command": "x {rom}" }
            ] }"#,
        )
        .unwrap();
        let names = |cfg: &Config| cfg.systems.iter().map(|s| cfg.system_name(s)).collect::<Vec<_>>();
        cfg.region = Region::Na;
        assert_eq!(names(&cfg), ["Genesis", "My Genesis", "gba"]);
        cfg.region = Region::Eu;
        assert_eq!(names(&cfg), ["Mega Drive", "My Genesis", "gba"]);
        assert_eq!(regional_name("pcengine", Region::Na), Some("TurboGrafx-16"));
        assert_eq!(regional_name("snes", Region::Jp), Some("Super Famicom"));
        assert_eq!(regional_name("gba", Region::Jp), None);
    }

    #[test]
    fn default_config_parses() {
        let cfg = parse(DEFAULT_CONFIG).unwrap();
        assert!(!cfg.systems.is_empty());
    }

    #[test]
    fn includes_fill_gaps_only() {
        let dir = std::env::temp_dir().join(format!("romi-include-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let main = r#"{
            "include": ["found.jsonc"],
            "vars": { "cores": "/mine" },
            "systems": [{ "id": "gba", "name": "Mine", "extensions": ["gba"], "command": "mgba {rom}" }]
        }"#;
        let found = r#"{
            // written by romi init
            "vars": { "cores": "/found", "extra": "/x" },
            "bios_dirs": ["~/bios"],
            "systems": [
                { "id": "gba", "name": "Found", "extensions": ["gba"], "command": "other {rom}" },
                { "id": "snes", "name": "SNES", "extensions": ["sfc"], "command": "snes9x {rom}" }
            ]
        }"#;
        std::fs::write(dir.join("config.json"), main).unwrap();
        std::fs::write(dir.join("found.jsonc"), found).unwrap();
        let cfg = load(&dir.join("config.json")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(cfg.vars["cores"], "/mine");
        assert_eq!(cfg.vars["extra"], "/x");
        assert!(cfg.bios_dirs.contains(&"~/bios".to_string()));
        let names: Vec<String> = cfg.systems.iter().map(|s| cfg.system_name(s)).collect();
        assert_eq!(names, ["Mine", "SNES"]);
    }

    #[test]
    fn included_overrides_fill_in_around_your_own() {
        let mut mine: Override = serde_json::from_str(r#"{ "name": "Mine", "command": "mine {rom}" }"#).unwrap();
        let menus: Override = serde_json::from_str(
            r#"{ "hidden": true, "emulator": "Other", "command": "other {rom}", "mode": "detach" }"#,
        )
        .unwrap();
        mine.fill(menus);
        assert_eq!(mine.name.as_deref(), Some("Mine"));
        assert!(mine.hidden);
        // The emulator pick comes as a whole or not at all.
        assert_eq!(mine.command.as_deref(), Some("mine {rom}"));
        assert_eq!((mine.emulator, mine.mode), (None, None));
    }

    #[test]
    fn missing_include_is_an_error() {
        let dir = std::env::temp_dir().join(format!("romi-include-missing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), r#"{ "include": ["nope.jsonc"] }"#).unwrap();
        let err = load(&dir.join("config.json")).unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(format!("{err:#}").contains("nope.jsonc"));
    }

    #[test]
    fn bios_lookup() {
        let dir = std::env::temp_dir().join(format!("romi-bios-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("dc")).unwrap();
        std::fs::write(dir.join("mpr-17933.bin"), b"x").unwrap();
        std::fs::write(dir.join("dc/dc_boot.bin"), b"x").unwrap();
        let dirs = vec![dir.to_string_lossy().into_owned()];
        let req: Vec<BiosFile> = serde_json::from_str(
            r#"[["sega_101.bin", "mpr-17933.bin"], "dc/dc_boot.bin", "*.bin", "bios_CD_U.bin", ["x.rom", "y.rom"]]"#,
        )
        .unwrap();
        assert_eq!(missing_bios(&req, &dirs), ["bios_CD_U.bin", "x.rom or y.rom"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn globs() {
        assert!(glob_match("naomi*.zip", "NAOMI2.zip"));
        assert!(glob_match("*.png", "arcade.png"));
        assert!(glob_match("mie.zip", "mie.zip"));
        assert!(!glob_match("mie.zip", "mie.zip.bak"));
        assert!(glob_match("scph?001.bin", "scph1001.bin"));
        assert!(!glob_match("*.png", "png"));
    }
}

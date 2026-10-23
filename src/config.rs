//! Config file: `~/.config/romi/config.json` (JSON with `//` and `/* */` comments allowed).

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG: &str = include_str!("../default-config.jsonc");

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Base directory that relative system `dir`s are resolved against.
    pub roms_dir: String,
    /// ES-DE gamelists directory; used for nicer names and descriptions when present.
    pub gamelists_dir: Option<String>,
    /// Folder with ES-DE's mamenames.xml / mamebioses.xml / mamedevices.xml, used by systems
    /// marked `"arcade": true` to turn short names like `sfa3u` into titles and hide BIOS sets.
    pub arcade_db: Option<String>,
    /// Extra `{placeholders}` usable in any command, cwd or env value.
    pub vars: HashMap<String, String>,
    pub theme: Theme,
    pub controller: ControllerConfig,
    pub systems: Vec<SystemConfig>,
    pub games: Vec<GameConfig>,
    pub steam: SteamConfig,
    pub art: ArtConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            roms_dir: "~/ROMs".into(),
            gamelists_dir: None,
            arcade_db: None,
            vars: HashMap::new(),
            theme: Theme::default(),
            controller: ControllerConfig::default(),
            systems: Vec::new(),
            games: Vec::new(),
            steam: SteamConfig::default(),
            art: ArtConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
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
    pub name: String,
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
    /// Per-file tweaks, keyed by path relative to the system dir (usually just the file name).
    #[serde(default)]
    pub overrides: HashMap<String, Override>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Override {
    pub name: Option<String>,
    pub command: Option<String>,
    pub mode: Option<LaunchMode>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub hidden: bool,
    /// Image to use instead of scraped art.
    pub art: Option<String>,
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
pub struct ArtConfig {
    /// ES-DE's downloaded_media folder; its covers are used as-is.
    pub esde_media_dir: Option<String>,
    /// Skyscraper's cache folder; its covers are used as-is.
    pub skyscraper_cache: Option<String>,
    /// Let `romi --scrape` download from libretro thumbnails and Steam's CDN.
    pub online: bool,
    /// Show covers in the details pane (Kitty/Sixel/iTerm2 graphics, or half-blocks elsewhere).
    pub show: bool,
}

impl Default for ArtConfig {
    fn default() -> Self {
        Self {
            esde_media_dir: Some("~/ES-DE/downloaded_media".into()),
            skyscraper_cache: Some("~/.skyscraper/cache".into()),
            online: true,
            show: true,
        }
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

/// Loads the config, writing the default one first if none exists.
pub fn load(path: &Path) -> Result<Config> {
    if !path.exists() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, DEFAULT_CONFIG)
            .with_context(|| format!("writing default config to {}", path.display()))?;
    }
    let raw = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse(&raw).with_context(|| format!("parsing {}", path.display()))
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
    fn default_config_parses() {
        let cfg = parse(DEFAULT_CONFIG).unwrap();
        assert!(!cfg.systems.is_empty());
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

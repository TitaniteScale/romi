//! Builds the game list from ROM folders, hand-added entries and Steam.

use crate::config::{expand_path, glob_match, Config, SystemConfig};
use crate::launch::{check_command, Launch};
use crate::steam;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const ROM_KEYS: &[&str] = &["rom", "file", "name", "dir"];

#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub description: Option<String>,
    pub developer: Option<String>,
    pub genre: Option<String>,
    pub released: Option<String>,
    pub players: Option<String>,
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
}

/// What box art lookups need to know about a game.
#[derive(Debug, Clone, Default)]
pub struct ArtRef {
    /// Title to search thumbnail sets for: the ROM's file stem, or the MAME title for arcade sets.
    pub title: Option<String>,
    pub appid: Option<String>,
    /// Image set by hand in the config.
    pub manual: Option<PathBuf>,
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

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

#[derive(Debug, Default)]
pub struct Library {
    pub games: Vec<Game>,
    pub systems: Vec<System>,
    pub warnings: Vec<String>,
}

impl Library {
    pub fn load(cfg: &Config) -> Library {
        let mut lib = Library::default();
        let roms_dir = expand_path(&cfg.roms_dir);
        let gamelists = cfg.gamelists_dir.as_deref().map(expand_path);

        let arcade = match &cfg.arcade_db {
            Some(dir) if cfg.systems.iter().any(|s| s.arcade) => Some(ArcadeDb::load(&expand_path(dir))),
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
            let meta = Meta { description: g.description.clone(), ..Meta::default() };
            let art = ArtRef { manual: g.art.as_deref().map(expand_path), ..ArtRef::default() };
            lib.push_game(group, format!("game:{}", g.name), g.name.clone(), None, meta, launch, art);
        }

        if cfg.steam.enabled {
            if let Some(dir) = steam::find_steam_dir(cfg.steam.dir.as_deref()) {
                let group = lib.group_for(&cfg.steam.name);
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
                    lib.push_game(group, format!("steam:{}", app.appid), app.name, None, Meta::default(), launch, art);
                }
            }
        }

        lib.systems.retain(|s| !s.games.is_empty());
        for sys in &mut lib.systems {
            let games = &lib.games;
            sys.games.sort_by(|&a, &b| games[a].sort_key.cmp(&games[b].sort_key));
            disambiguate(&mut lib.games, &sys.games);
        }
        lib
    }

    fn scan_system(&mut self, cfg: &Config, sys: &SystemConfig, roms_dir: &Path, gamelists: Option<&Path>, arcade: Option<&ArcadeDb>) {
        let arcade = arcade.filter(|_| sys.arcade);
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
            name: sys.name.clone(),
            games: Vec::new(),
            problem: check_command(&sys.command, &cfg.vars, ROM_KEYS),
            libretro: sys.libretro.clone().unwrap_or_else(|| crate::art::default_libretro(&sys.id)),
            esde: sys.id.clone(),
            skyscraper: sys.skyscraper.clone().unwrap_or_else(|| crate::art::default_skyscraper(&sys.id)),
        });

        for rel in files {
            let path = dir.join(&rel);
            let rel_str = rel.to_string_lossy().into_owned();
            let file_name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            if !exts.contains(&ext) || sys.exclude.iter().any(|p| glob_match(p, &file_name) || glob_match(p, &rel_str)) {
                continue;
            }
            let ov = sys.overrides.get(&rel_str).or_else(|| sys.overrides.get(&file_name)).cloned().unwrap_or_default();
            if ov.hidden {
                continue;
            }
            let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            if arcade.is_some_and(|db| db.not_games.contains(&stem)) {
                continue;
            }
            let (es_name, meta) = meta.get(&rel_str).cloned().unwrap_or_default();
            let mame_title = arcade.and_then(|db| db.names.get(&stem)).cloned();
            // MAME appends alternate titles ("Art of Fighting / Ryuuko no Ken"); show only the first.
            // For arcade sets MAME's own title beats a scraped one (scrapers often mix up sequels).
            let mame_name = mame_title.as_deref().map(|t| clean_name(t.split(" / ").next().unwrap_or(t)));
            let name = ov.name.or(mame_name).or(es_name).unwrap_or_else(|| clean_name(&stem));
            let art = ArtRef {
                // Thumbnail sets use the full title, alternates and all.
                title: Some(mame_title.unwrap_or_else(|| stem.clone())),
                appid: None,
                manual: ov.art.as_deref().map(expand_path),
            };

            let mut vars = cfg.vars.clone();
            vars.insert("rom".into(), path.to_string_lossy().into_owned());
            vars.insert("file".into(), file_name);
            vars.insert("name".into(), stem);
            vars.insert("dir".into(), path.parent().unwrap_or(&dir).to_string_lossy().into_owned());
            let mut env = sys.env.clone();
            env.extend(ov.env);
            let launch = Launch {
                command: ov.command.unwrap_or_else(|| sys.command.clone()),
                mode: ov.mode.unwrap_or(sys.mode),
                cwd: ov.cwd.or_else(|| sys.cwd.clone()),
                env,
                vars,
            };
            let key = format!("{}:{}", sys.id, rel_str);
            self.push_game(idx, key, name, Some(path), meta, launch, art);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_game(&mut self, system: usize, key: String, name: String, path: Option<PathBuf>, meta: Meta, launch: Launch, art: ArtRef) {
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
        });
        self.systems[system].games.push(self.games.len() - 1);
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
            genre: field("genre"),
            released: field("releasedate").map(|d| d.chars().take(4).collect()),
            players: field("players"),
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
    fn flat_xml_lists() {
        let xml = "<game>\n<mamename>sfa3u</mamename>\n<realname>Street Fighter Alpha 3 (USA)</realname>\n</game>\n<game><mamename>x</mamename><realname>A &amp; B</realname></game>";
        assert_eq!(tag_values(xml, "realname"), ["Street Fighter Alpha 3 (USA)", "A & B"]);
        assert_eq!(tag_values(xml, "mamename"), ["sfa3u", "x"]);
    }

    #[test]
    fn sorting_ignores_the() {
        assert!(sort_key("The King of Fighters") > sort_key("Bomberman"));
        assert!(sort_key("The King of Fighters") < sort_key("Metal Slug"));
    }
}

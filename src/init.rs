//! `romi init`: finds ROM folders and installed emulators and writes a config for them.
//!
//! What each system is and how to launch it comes from ES-DE when it's installed:
//! `es_systems.xml` lists about 200 systems with their folder names, file extensions and a
//! labelled command per emulator, and `es_find_rules.xml` says where to look for each emulator.
//! For each ROM folder, the first command whose emulator (and RetroArch core) is installed wins.
//! Without ES-DE, the systems in romi's default config stand in, with one command each.
//!
//! The result goes in its own file, `init.jsonc`, which the main config includes. Rerunning init
//! replaces that file and nothing else, and anything in the main config takes priority over it.

use crate::config::{self, BiosFile, Config, LaunchMode, expand_path};
use crate::launch::find_program;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Where ES-DE keeps its system definitions, in the order they're tried. A user's
/// `custom_systems` folder overrides individual systems and emulators from the shipped files.
const ESDE_DIRS: &[&str] =
    &["/usr/share/es-de/resources/systems/linux", "/usr/local/share/es-de/resources/systems/linux"];
const ESDE_CUSTOM: &str = "~/ES-DE/custom_systems";
const ESDE_SETTINGS: &str = "~/ES-DE/settings/es_settings.xml";

/// RetroArch's core folders, for when ES-DE's find rules aren't there to say.
const CORE_DIRS: &[&str] = &[
    "~/.config/retroarch/cores",
    "~/.var/app/org.libretro.RetroArch/config/retroarch/cores",
    "/usr/lib/libretro",
    "/usr/lib/x86_64-linux-gnu/libretro",
    "/usr/lib64/libretro",
];

/// Where people usually keep ROMs, in the order they're tried.
const ROM_DIRS: &[&str] = &["~/ROMs", "~/roms", "~/Emulation/roms", "~/RetroPie/roms"];

/// Other names people give ROM folders, and the ES-DE folder each one means.
const FOLDER_ALIASES: &[(&str, &str)] = &[
    ("scd", "segacd"),
    ("ps1", "psx"),
    ("playstation", "psx"),
    ("playstation2", "ps2"),
    ("gamecube", "gc"),
    ("md", "megadrive"),
    ("pce", "pcengine"),
    ("sfc", "snes"),
    ("supernintendo", "snes"),
];

/// Extensions too generic to say which system a folder is for.
const GENERIC_EXTS: &[&str] =
    &["zip", "7z", "bin", "iso", "chd", "cue", "m3u", "img", "ccd", "sub", "txt", "json", "xml", "png", "jpg"];

/// The file name of the include romi init writes, next to the main config.
pub const INCLUDE_FILE: &str = "init.jsonc";

/// A system definition: from ES-DE, or from romi's default config.
#[derive(Debug, Clone)]
pub struct SystemDef {
    pub id: String,
    pub name: String,
    pub folder: String,
    pub extensions: Vec<String>,
    /// (label, command), best first. ES-DE syntax or romi's, depending on `esde`.
    pub commands: Vec<(String, String)>,
    pub arcade: bool,
}

/// Where to find each emulator, and RetroArch's cores.
#[derive(Debug, Default)]
pub struct FindRules {
    /// Emulator name → program names to look for on PATH, then paths (globs allowed).
    emulators: HashMap<String, (Vec<String>, Vec<String>)>,
    core_dirs: Vec<String>,
}

pub struct Catalog {
    pub systems: Vec<SystemDef>,
    rules: FindRules,
    /// True when these came from ES-DE (commands use its `%PLACEHOLDERS%`).
    pub esde: bool,
}

impl Catalog {
    /// ES-DE's definitions from `dir`, or wherever ES-DE is installed.
    pub fn esde(dir: Option<&Path>) -> Option<Catalog> {
        let dir = match dir {
            Some(d) => d.to_path_buf(),
            None => ESDE_DIRS.iter().map(PathBuf::from).find(|d| d.join("es_systems.xml").is_file())?,
        };
        let custom = expand_path(ESDE_CUSTOM);
        let mut systems = parse_systems(&std::fs::read_to_string(dir.join("es_systems.xml")).ok()?);
        let mut rules = parse_rules(&std::fs::read_to_string(dir.join("es_find_rules.xml")).unwrap_or_default());
        if let Ok(xml) = std::fs::read_to_string(custom.join("es_systems.xml")) {
            for sys in parse_systems(&xml) {
                match systems.iter_mut().find(|s| s.id == sys.id) {
                    Some(s) => *s = sys,
                    None => systems.push(sys),
                }
            }
        }
        if let Ok(xml) = std::fs::read_to_string(custom.join("es_find_rules.xml")) {
            let more = parse_rules(&xml);
            rules.emulators.extend(more.emulators);
            if !more.core_dirs.is_empty() {
                rules.core_dirs = more.core_dirs.into_iter().chain(rules.core_dirs).collect();
            }
        }
        Some(Catalog { systems, rules, esde: true })
    }

    /// The systems in romi's default config, for when ES-DE isn't installed.
    pub fn builtin() -> Catalog {
        let cfg = config::parse(config::DEFAULT_CONFIG).expect("the default config parses");
        let systems = cfg
            .systems
            .into_iter()
            .map(|s| SystemDef {
                folder: s.dir.clone().unwrap_or_else(|| s.id.clone()),
                commands: vec![(String::new(), s.command)],
                id: s.id,
                name: s.name,
                extensions: s.extensions,
                arcade: s.arcade,
            })
            .collect();
        let rules = FindRules { core_dirs: CORE_DIRS.iter().map(|d| d.to_string()).collect(), ..FindRules::default() };
        Catalog { systems, rules, esde: false }
    }
}

fn parse_systems(xml: &str) -> Vec<SystemDef> {
    let Ok(doc) = roxmltree::Document::parse(xml) else { return Vec::new() };
    let text = |n: roxmltree::Node, tag: &str| {
        n.children().find(|c| c.has_tag_name(tag)).and_then(|c| c.text()).unwrap_or_default().trim().to_string()
    };
    doc.descendants()
        .filter(|n| n.has_tag_name("system"))
        .filter_map(|n| {
            let id = text(n, "name");
            let path = text(n, "path");
            let folder = path.strip_prefix("%ROMPATH%/").unwrap_or(&id).to_string();
            let mut extensions: Vec<String> = Vec::new();
            for e in text(n, "extension").split_whitespace() {
                let e = e.trim_start_matches('.').to_lowercase();
                if !extensions.contains(&e) {
                    extensions.push(e);
                }
            }
            let commands = n
                .children()
                .filter(|c| c.has_tag_name("command"))
                .map(|c| {
                    (c.attribute("label").unwrap_or_default().to_string(), c.text().unwrap_or_default().to_string())
                })
                .collect();
            let arcade = text(n, "platform").split(',').any(|p| p.trim() == "arcade");
            (!id.is_empty()).then(|| SystemDef { name: text(n, "fullname"), id, folder, extensions, commands, arcade })
        })
        .collect()
}

fn parse_rules(xml: &str) -> FindRules {
    let mut rules = FindRules::default();
    let Ok(doc) = roxmltree::Document::parse(xml) else { return rules };
    let entries = |rule: roxmltree::Node| -> Vec<String> {
        rule.children().filter(|c| c.has_tag_name("entry")).filter_map(|c| c.text()).map(str::to_string).collect()
    };
    for node in doc.root_element().children().filter(|n| n.is_element()) {
        let Some(name) = node.attribute("name") else { continue };
        let rules_of = |kind: &str| -> Vec<String> {
            node.children().filter(|r| r.attribute("type") == Some(kind)).flat_map(entries).collect()
        };
        if node.has_tag_name("emulator") {
            rules.emulators.insert(name.to_string(), (rules_of("systempath"), rules_of("staticpath")));
        } else if node.has_tag_name("core") && name == "RETROARCH" {
            rules.core_dirs = rules_of("corepath");
        }
    }
    rules
}

/// A command ready for romi's config.
#[derive(Debug, Clone)]
pub struct Translated {
    pub label: String,
    pub command: String,
    pub cwd: Option<String>,
    pub mode: Option<LaunchMode>,
}

/// Finds programs and cores, remembering what it found.
struct Finder<'a> {
    rules: &'a FindRules,
    programs: HashMap<String, Option<String>>,
    /// The folder `{cores}` stands for: the first core folder that has any cores in it.
    cores: Option<PathBuf>,
}

impl<'a> Finder<'a> {
    fn new(rules: &'a FindRules) -> Finder<'a> {
        let has_cores = |d: &PathBuf| {
            std::fs::read_dir(d).is_ok_and(|mut it| {
                it.any(|e| e.is_ok_and(|e| e.file_name().to_string_lossy().ends_with("_libretro.so")))
            })
        };
        let cores = rules.core_dirs.iter().map(|d| expand_path(d)).find(has_cores);
        Finder { rules, programs: HashMap::new(), cores }
    }

    /// How to run an ES-DE emulator here: a name on PATH, or a full path.
    fn emulator(&mut self, name: &str) -> Option<String> {
        if let Some(found) = self.programs.get(name) {
            return found.clone();
        }
        let found = self.rules.emulators.get(name).and_then(|(on_path, paths)| {
            on_path
                .iter()
                .find(|p| find_program(p).is_some())
                .cloned()
                .or_else(|| paths.iter().find_map(|p| first_match(p)).map(|p| crate::app::tilde(&p)))
        });
        self.programs.insert(name.to_string(), found.clone());
        found
    }

    /// A core file as it should appear in a command: under `{cores}` when it's there.
    fn core(&self, file: &str) -> Option<String> {
        if self.cores.as_ref().is_some_and(|d| d.join(file).is_file()) {
            return Some(format!("{{cores}}/{file}"));
        }
        self.rules
            .core_dirs
            .iter()
            .map(|d| expand_path(d).join(file))
            .find(|p| p.is_file())
            .map(|p| crate::app::tilde(&p))
    }

    /// Turns an ES-DE command into romi's, or says why it can't run here.
    fn translate_esde(&mut self, label: &str, raw: &str, roms_dir: &Path) -> Result<Translated, String> {
        let mut words = Vec::new();
        let (mut cwd, mut mode) = (None, None);
        for word in raw.split_whitespace() {
            if let Some(dir) = word.strip_prefix("%STARTDIR%=") {
                cwd = Some(match dir {
                    "%GAMEDIR%" | "%GAMEDIRRAW%" => "{dir}".to_string(),
                    d if !d.contains('%') => d.to_string(),
                    d => return Err(format!("starts in {d}")),
                });
                continue;
            }
            // %INJECT% adds arguments from a file next to the game, but only if there is one.
            if word.starts_with("%INJECT%=") {
                continue;
            }
            match word {
                "%RUNINBACKGROUND%" => mode = Some(LaunchMode::Detach),
                "%HIDEWINDOW%" | "%ESCAPESPECIALS%" => {}
                _ => words.push(word),
            }
        }
        let mut out = Vec::new();
        for word in words {
            let mut word = word.to_string();
            while let Some(start) = word.find('%') {
                let Some(len) = word[start + 1..].find('%') else { break };
                let token = &word[start..start + len + 2];
                let value = match &token[1..token.len() - 1] {
                    "ROM" | "ROMRAW" => "{rom}".to_string(),
                    "BASENAME" => "{name}".to_string(),
                    "FILENAME" => "{file}".to_string(),
                    "GAMEDIR" | "GAMEDIRRAW" => "{dir}".to_string(),
                    "ROMPATH" => crate::app::tilde(roms_dir),
                    "CORE_RETROARCH" => {
                        // The core's file name follows: `%CORE_RETROARCH%/x_libretro.so`.
                        let rest = &word[start + token.len()..];
                        let file = rest.trim_start_matches('/').split(['"', '\'']).next().unwrap_or_default();
                        let core = self.core(file).ok_or_else(|| format!("no {file}"))?;
                        word = format!("{}{core}{}", &word[..start], &rest[1 + file.len()..]);
                        continue;
                    }
                    t if t.starts_with("EMULATOR_") => {
                        let name = &t["EMULATOR_".len()..];
                        self.emulator(name).ok_or_else(|| format!("{} not installed", name.to_lowercase()))?
                    }
                    t => return Err(format!("needs %{t}%")),
                };
                word.replace_range(start..start + token.len(), &value);
            }
            out.push(word);
        }
        Ok(Translated { label: label.to_string(), command: out.join(" "), cwd, mode })
    }

    /// Checks a command already in romi's syntax (from the default config).
    fn check_romi(&mut self, raw: &str) -> Result<Translated, String> {
        let program = raw.split_whitespace().next().unwrap_or_default();
        if find_program(program).is_none() {
            return Err(format!("{program} not installed"));
        }
        if let Some(i) = raw.find("{cores}/") {
            let file = raw[i + "{cores}/".len()..].split_whitespace().next().unwrap_or_default();
            let core = self.core(file).ok_or_else(|| format!("no {file}"))?;
            return Ok(Translated {
                label: String::new(),
                command: raw.replacen(&format!("{{cores}}/{file}"), &core, 1),
                cwd: None,
                mode: None,
            });
        }
        Ok(Translated { label: String::new(), command: raw.to_string(), cwd: None, mode: None })
    }
}

/// The first existing file matching a path whose last part may contain `*`.
fn first_match(pattern: &str) -> Option<PathBuf> {
    let path = expand_path(pattern);
    let name = path.file_name()?.to_string_lossy().into_owned();
    if !name.contains('*') {
        return path.is_file().then_some(path);
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir(path.parent()?)
        .ok()?
        .flatten()
        .filter(|e| config::glob_match(&name, &e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .collect();
    found.sort();
    found.pop()
}

/// What init found in one ROM folder.
#[derive(Debug)]
pub enum Finding {
    /// A system to add.
    New { def: SystemDef, games: usize, run: Translated, others: Vec<String>, guessed: bool },
    /// The config already has a system for this folder.
    Configured { folder: String, id: String },
    /// Recognized, but nothing installed can run it.
    NoEmulator { def: SystemDef, games: usize, tried: Vec<String> },
    /// Recognized, but there are no games in it.
    Empty { def: SystemDef },
    /// Not a folder for any known system.
    Unknown { folder: String },
}

pub struct Report {
    pub roms_dir: PathBuf,
    pub findings: Vec<Finding>,
    /// The `{cores}` folder, when any system uses it.
    pub cores: Option<PathBuf>,
    pub esde: bool,
}

/// Finds the ROM folder: the one given, the config's, the one set in ES-DE, or the first usual
/// place that exists.
pub fn find_roms_dir(given: Option<&Path>, existing: Option<&Config>) -> Option<PathBuf> {
    if let Some(dir) = given {
        return Some(dir.to_path_buf());
    }
    if let Some(cfg) = existing {
        return Some(expand_path(&cfg.roms_dir));
    }
    esde_roms_dir().or_else(|| ROM_DIRS.iter().map(|d| expand_path(d)).find(|d| d.is_dir()))
}

/// ES-DE's ROMDirectory setting, when it's set (empty means ES-DE's default, `~/ROMs`).
fn esde_roms_dir() -> Option<PathBuf> {
    let settings = std::fs::read_to_string(expand_path(ESDE_SETTINGS)).ok()?;
    let line = settings.lines().find(|l| l.contains("name=\"ROMDirectory\""))?;
    let value = line.split("value=\"").nth(1)?.split('"').next()?;
    let dir = expand_path(value);
    (!value.is_empty() && dir.is_dir()).then_some(dir)
}

pub fn scan(catalog: &Catalog, roms_dir: &Path, existing: Option<&Config>) -> Report {
    let mut finder = Finder::new(&catalog.rules);
    let mut folders: Vec<String> = std::fs::read_dir(roms_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with('.'))
        .collect();
    folders.sort();
    // Ids already taken, by the config or by an earlier folder. A second folder for the same
    // system (`scd` and `segacd`, or one recognized by file type) goes by its folder name.
    let mut taken: Vec<String> = existing.map(|c| c.systems.iter().map(|s| s.id.clone()).collect()).unwrap_or_default();

    let findings = folders
        .into_iter()
        .map(|folder| {
            let path = roms_dir.join(&folder);
            if let Some(id) = configured_for(existing, roms_dir, &path) {
                return Finding::Configured { folder, id };
            }
            let lower = folder.to_lowercase();
            let alias = FOLDER_ALIASES.iter().find(|(a, _)| *a == lower).map_or(lower.as_str(), |(_, to)| to);
            let by_name = catalog
                .systems
                .iter()
                .find(|s| s.folder.eq_ignore_ascii_case(&folder))
                .or_else(|| catalog.systems.iter().find(|s| s.folder == alias));
            let (def, guessed) = match by_name {
                Some(def) => (SystemDef { folder: folder.clone(), ..def.clone() }, false),
                None => match guess_by_extension(catalog, &path) {
                    Some(def) => (SystemDef { folder: folder.clone(), ..def.clone() }, true),
                    None => return Finding::Unknown { folder },
                },
            };
            let games = count_games(&path, &def.extensions);
            if games == 0 {
                return Finding::Empty { def };
            }
            let mut runnable = Vec::new();
            let mut tried = Vec::new();
            for (label, raw) in &def.commands {
                let result =
                    if catalog.esde { finder.translate_esde(label, raw, roms_dir) } else { finder.check_romi(raw) };
                match result {
                    Ok(t) => runnable.push(t),
                    Err(why) => tried.push(if label.is_empty() { why } else { format!("{label}: {why}") }),
                }
            }
            if runnable.is_empty() {
                return Finding::NoEmulator { def, games, tried };
            }
            let run = runnable.remove(0);
            let others = runnable.into_iter().map(|t| t.label).filter(|l| !l.is_empty()).collect();
            let mut def = def;
            if taken.contains(&def.id) {
                def.id = folder.to_lowercase();
            }
            taken.push(def.id.clone());
            Finding::New { def, games, run, others, guessed }
        })
        .collect();
    Report { roms_dir: roms_dir.to_path_buf(), findings, cores: finder.cores, esde: catalog.esde }
}

/// The id of a system in the existing config that already covers this folder.
fn configured_for(existing: Option<&Config>, roms_dir: &Path, path: &Path) -> Option<String> {
    let canonical = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let target = canonical(path);
    existing?.systems.iter().find_map(|s| {
        let dir = s.dir.as_deref().unwrap_or(&s.id);
        let dir = if dir.starts_with('/') || dir.starts_with('~') { expand_path(dir) } else { roms_dir.join(dir) };
        (canonical(&dir) == target).then(|| s.id.clone())
    })
}

/// A system for a folder with an unfamiliar name, from the extension most of its files have,
/// as long as only one system uses that extension.
fn guess_by_extension<'a>(catalog: &'a Catalog, dir: &Path) -> Option<&'a SystemDef> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for file in files(dir, 2) {
        if let Some(ext) = file.extension() {
            let ext = ext.to_string_lossy().to_lowercase();
            if !GENERIC_EXTS.contains(&ext.as_str()) {
                *counts.entry(ext).or_default() += 1;
            }
        }
    }
    let (ext, _) = counts.into_iter().max_by_key(|(_, n)| *n)?;
    let mut matches = catalog.systems.iter().filter(|s| s.extensions.contains(&ext));
    let only = matches.next()?;
    matches.next().is_none().then_some(only)
}

fn count_games(dir: &Path, extensions: &[String]) -> usize {
    files(dir, 2)
        .iter()
        .filter(|f| f.extension().is_some_and(|e| extensions.contains(&e.to_string_lossy().to_lowercase())))
        .count()
}

/// Files under `dir`, down to `depth` levels of subfolders.
fn files(dir: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            if depth > 0 {
                out.extend(files(&path, depth - 1));
            }
        } else {
            out.push(path);
        }
    }
    out
}

/// BIOS files that common systems can't start without, by ES-DE/romi system id.
fn bios_for(id: &str) -> Vec<BiosFile> {
    let any = |files: &[&str]| BiosFile::AnyOf(files.iter().map(|f| f.to_string()).collect());
    match id {
        "psx" => vec![any(&["scph5501.bin", "scph5500.bin", "scph5502.bin", "scph1001.bin"])],
        "ps2" => vec![BiosFile::One("~/.config/PCSX2/bios/*.bin".into())],
        "saturn" => vec![any(&["sega_101.bin", "mpr-17933.bin"])],
        "segacd" | "megacd" | "scd" => vec![any(&["bios_CD_U.bin", "bios_CD_E.bin", "bios_CD_J.bin"])],
        "neogeo" => vec![BiosFile::One("neogeo.zip".into())],
        "naomi" => vec![any(&["naomi.zip", "dc/naomi.zip"])],
        "pcenginecd" | "tg-cd" => vec![BiosFile::One("syscard3.pce".into())],
        _ => Vec::new(),
    }
}

/// The contents of `init.jsonc`.
pub fn render(report: &Report) -> String {
    let mut out = String::new();
    let q = |s: &str| serde_json::to_string(s).unwrap_or_default();
    let _ = writeln!(out, "// Written by `romi init`. Running it again replaces this file.");
    let _ = writeln!(out, "// To change a system, copy it into config.json and edit it there: config.json wins.");
    out.push_str("{\n");
    let new: Vec<_> = report
        .findings
        .iter()
        .filter_map(|f| match f {
            Finding::New { def, games, run, others, guessed } => Some((def, games, run, others, guessed)),
            _ => None,
        })
        .collect();
    let uses_cores = new.iter().any(|(_, _, run, _, _)| run.command.contains("{cores}"));
    if let (true, Some(cores)) = (uses_cores, &report.cores) {
        let _ = writeln!(out, "  \"vars\": {{ \"cores\": {} }},", q(&crate::app::tilde(cores)));
    }
    out.push_str("  \"systems\": [");
    // Nothing here says what's in the ROM folders (not even how many games), so the file is safe
    // to keep in a public dotfiles repository.
    for (i, (def, _, run, others, guessed)) in new.iter().enumerate() {
        out.push_str(if i == 0 { "\n" } else { ",\n" });
        let mut notes = Vec::new();
        if **guessed {
            notes.push("Recognized by file type".to_string());
        }
        if !run.label.is_empty() {
            notes.push(format!("Using {}", run.label));
        }
        if !others.is_empty() {
            notes.push(format!("also installed: {}", others.join(", ")));
        }
        if !notes.is_empty() {
            let _ = writeln!(out, "    // {}", notes.join("; "));
        }
        out.push_str("    {\n");
        let _ = write!(out, "      \"id\": {}, \"name\": {}", q(&def.id), q(&def.name));
        if def.folder != def.id {
            let _ = write!(out, ", \"dir\": {}", q(&def.folder));
        }
        if def.arcade {
            out.push_str(", \"arcade\": true");
        }
        out.push_str(",\n");
        let exts: Vec<String> = def.extensions.iter().map(|e| q(e)).collect();
        let _ = writeln!(out, "      \"extensions\": [{}],", exts.join(", "));
        let bios = bios_for(&def.id);
        if !bios.is_empty() {
            let _ = writeln!(out, "      \"bios\": {},", serde_json::to_string(&bios).unwrap_or_default());
        }
        if let Some(cwd) = &run.cwd {
            let _ = writeln!(out, "      \"cwd\": {},", q(cwd));
        }
        if let Some(LaunchMode::Detach) = run.mode {
            out.push_str("      \"mode\": \"detach\",\n");
        }
        let _ = writeln!(out, "      \"command\": {}", q(&run.command));
        out.push_str("    }");
    }
    out.push_str(if new.is_empty() { "]\n" } else { "\n  ]\n" });
    out.push_str("}\n");
    out
}

/// A short account of what was found, for people.
pub fn summary(report: &Report) -> String {
    let mut out = String::new();
    let source = if report.esde { "ES-DE's system list" } else { "romi's built-in system list" };
    let _ = writeln!(out, "ROMs in {} (using {source})\n", crate::app::tilde(&report.roms_dir));
    let plural = |n: usize| format!("{n:>4} game{}", if n == 1 { " " } else { "s" });
    for f in &report.findings {
        let line = match f {
            Finding::New { def, games, run, guessed, .. } => {
                let how = if run.label.is_empty() { runs(&run.command) } else { run.label.clone() };
                let guess = if *guessed { "  (recognized by file type)" } else { "" };
                format!("  + {:<14} {:<28} {}  {how}{guess}", def.folder, def.name, plural(*games))
            }
            Finding::Configured { folder, id } => {
                format!("  = {folder:<14} already in your config as \"{id}\"")
            }
            Finding::NoEmulator { def, games, tried } => {
                let tried: Vec<&str> = tried.iter().map(|t| t.split(':').next().unwrap_or(t)).take(4).collect();
                format!(
                    "  ! {:<14} {:<28} {}  no emulator installed (ES-DE knows: {})",
                    def.folder,
                    def.name,
                    plural(*games),
                    tried.join(", ")
                )
            }
            Finding::Empty { def } => format!("  - {:<14} {:<28}    no games", def.folder, def.name),
            Finding::Unknown { folder } => format!("  ? {folder:<14} not a system romi knows"),
        };
        let _ = writeln!(out, "{}", line.trim_end());
    }
    let added = report.findings.iter().filter(|f| matches!(f, Finding::New { .. })).count();
    let _ = writeln!(out, "\n{added} system{} to add", if added == 1 { "" } else { "s" });
    out
}

/// "retroarch · mgba" for a romi command, like the TUI's details pane shows.
fn runs(command: &str) -> String {
    let mut words = command.split_whitespace();
    let program = words.next().unwrap_or_default();
    let program = program.rsplit('/').next().unwrap_or(program);
    match words.find_map(|w| w.rsplit('/').next()?.strip_suffix("_libretro.so")) {
        Some(core) => format!("{program} · {core}"),
        None => program.to_string(),
    }
}

/// A new main config, for when there isn't one yet.
pub fn fresh_config(roms_dir: &Path) -> String {
    let q = |s: &str| serde_json::to_string(s).unwrap_or_default();
    let mut out = String::from(
        "// romi config. Comments are allowed. Press ctrl-r in romi to reload after editing.\n\
         // Every option is described in default-config.jsonc in romi's repository.\n{\n",
    );
    let _ = writeln!(out, "  \"roms_dir\": {},", q(&crate::app::tilde(roms_dir)));
    let esde = expand_path("~/ES-DE/gamelists");
    if esde.is_dir() {
        out.push_str("  // Names and descriptions scraped by ES-DE.\n");
        let _ = writeln!(out, "  \"gamelists_dir\": {},", q(&crate::app::tilde(&esde)));
    }
    if let Some(mame) = ESDE_DIRS.iter().map(|d| Path::new(d).join("../../MAME")).find(|d| d.is_dir()) {
        out.push_str("  // MAME titles shipped with ES-DE, for arcade systems.\n");
        let mame = mame.canonicalize().unwrap_or(mame);
        let _ = writeln!(out, "  \"arcade_db\": {},", q(&mame.to_string_lossy()));
    }
    out.push_str("  // Systems found by `romi init`. Add your own below; they take priority.\n");
    let _ = writeln!(out, "  \"include\": [{}],", q(INCLUDE_FILE));
    out.push_str("  \"systems\": [],\n  \"games\": [],\n  \"steam\": { \"enabled\": true }\n}\n");
    out
}

/// Adds the include line to an existing config's text, keeping everything else as it is.
/// None when the config already includes other files (editing that list by hand is safer).
pub fn add_include(raw: &str, cfg: &Config) -> Option<String> {
    if cfg.include.iter().any(|i| i == INCLUDE_FILE) {
        return Some(raw.to_string());
    }
    if !cfg.include.is_empty() {
        return None;
    }
    let brace = opening_brace(raw)?;
    let line = format!(
        "\n  // Systems found by `romi init`. Anything set in this file takes priority.\n  \"include\": [\"{INCLUDE_FILE}\"],"
    );
    Some(format!("{}{line}{}", &raw[..=brace], &raw[brace + 1..]))
}

/// The byte offset of the `{` that opens the top-level object, skipping comments.
fn opening_brace(raw: &str) -> Option<usize> {
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => return Some(i),
            b'/' if bytes.get(i + 1) == Some(&b'/') => i = raw[i..].find('\n').map_or(bytes.len(), |n| i + n),
            b'/' if bytes.get(i + 1) == Some(&b'*') => i = raw[i + 2..].find("*/").map_or(bytes.len(), |n| i + n + 4),
            _ => i += 1,
        }
    }
    None
}

/// The main config as written, without its includes, so systems init added last time don't
/// count as configured.
pub fn existing_config(path: &Path) -> Option<(String, Config)> {
    let raw = std::fs::read_to_string(path).ok()?;
    let cfg = config::parse(&raw).ok()?;
    Some((raw, cfg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_esde_commands() {
        let dir = std::env::temp_dir().join(format!("romi-init-test-{}", std::process::id()));
        let cores = dir.join("cores");
        std::fs::create_dir_all(&cores).unwrap();
        std::fs::write(cores.join("gambatte_libretro.so"), "").unwrap();
        let mut rules = FindRules { core_dirs: vec![cores.to_string_lossy().into_owned()], ..FindRules::default() };
        rules.emulators.insert("RETROARCH".into(), (vec!["sh".into()], vec![]));
        rules.emulators.insert("OS-SHELL".into(), (vec!["sh".into()], vec![]));
        let mut finder = Finder::new(&rules);
        let roms = Path::new("/roms");

        let t = finder.translate_esde("x", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/gambatte_libretro.so %ROM%", roms);
        assert_eq!(t.unwrap().command, "sh -L {cores}/gambatte_libretro.so {rom}");
        let t = finder.translate_esde("x", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/sameboy_libretro.so %ROM%", roms);
        assert_eq!(t.unwrap_err(), "no sameboy_libretro.so");
        let t = finder.translate_esde("x", "%STARTDIR%=%GAMEDIR% %RUNINBACKGROUND% %EMULATOR_OS-SHELL% %ROM%", roms);
        let t = t.unwrap();
        assert_eq!(
            (t.command.as_str(), t.cwd.as_deref(), t.mode),
            ("sh {rom}", Some("{dir}"), Some(LaunchMode::Detach))
        );
        let t = finder.translate_esde("x", "%EMULATOR_DUCKSTATION% -batch %ROM%", roms);
        assert_eq!(t.unwrap_err(), "duckstation not installed");
        let t = finder.translate_esde("x", "%INJECT%=%BASENAME%.esprefix %EMULATOR_OS-SHELL% %ROM%", roms);
        assert_eq!(t.unwrap().command, "sh {rom}");
        let t = finder.translate_esde("x", "%EMUDIR%/x %ROM%", roms);
        assert!(t.unwrap_err().starts_with("needs %"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn include_goes_after_the_opening_brace() {
        let raw = "// a comment with a { brace\n/* and { another */\n{\n  \"roms_dir\": \"~/ROMs\"\n}\n";
        let cfg = config::parse(raw).unwrap();
        let out = add_include(raw, &cfg).unwrap();
        assert!(out.starts_with("// a comment with a { brace\n/* and { another */\n{\n  // Systems found"));
        let cfg = config::parse(&out).unwrap();
        assert_eq!(cfg.include, [INCLUDE_FILE]);
        assert_eq!(add_include(&out, &cfg).unwrap(), out);
    }

    #[test]
    fn builtin_catalog_parses() {
        let c = Catalog::builtin();
        assert!(c.systems.iter().any(|s| s.id == "psx" && !s.commands.is_empty()));
    }
}

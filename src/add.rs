//! `romi add <path>`: turns a program, script, folder or .desktop file into a game entry, kept in
//! `games.jsonc` next to the config (which the config includes, like init's file).

use crate::config::{self, Config, LaunchMode};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The file romi add writes to, next to the main config.
pub const GAMES_FILE: &str = "games.jsonc";

const GAMES_HEADER: &str = "// Games added with `romi add`. Edit them freely: romi add only appends to this list.\n";

/// A game entry, in the order its fields are written.
#[derive(Debug, Default)]
pub struct Entry {
    pub name: String,
    pub command: String,
    pub system: Option<String>,
    pub cwd: Option<String>,
    pub mode: Option<LaunchMode>,
}

impl Entry {
    /// One line of JSON, fields in a readable order.
    pub fn to_json(&self) -> String {
        let q = |s: &str| serde_json::to_string(s).unwrap_or_default();
        let mut fields = vec![format!("\"name\": {}", q(&self.name)), format!("\"command\": {}", q(&self.command))];
        if let Some(system) = &self.system {
            fields.push(format!("\"system\": {}", q(system)));
        }
        if let Some(cwd) = &self.cwd {
            fields.push(format!("\"cwd\": {}", q(cwd)));
        }
        if let Some(mode) = self.mode {
            let mode = serde_json::to_string(&mode).unwrap_or_default();
            fields.push(format!("\"mode\": {mode}"));
        }
        format!("{{ {} }}", fields.join(", "))
    }
}

/// Why a path can't be added as it is.
#[derive(Debug, PartialEq)]
pub enum Refusal {
    Missing,
    /// A folder with no programs in it.
    NothingToRun,
    /// A folder with several; the person picks one.
    Several(Vec<String>),
    NotExecutable,
    /// A ROM for a configured system, which belongs in that system's folder.
    Rom {
        system: String,
        folder: PathBuf,
    },
    Windows,
}

/// Works out the entry for `path`: its name, command, and where to run it from.
pub fn entry_for(path: &Path, cfg: Option<&Config>) -> Result<Entry, Refusal> {
    if !path.exists() {
        return Err(Refusal::Missing);
    }
    let path = path.canonicalize().map_err(|_| Refusal::Missing)?;
    if path.is_dir() {
        let mut programs: Vec<PathBuf> = std::fs::read_dir(&path)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .map(|e| e.path())
            .filter(|p| p.is_file() && (is_executable(p) || has_ext(p, "sh")))
            .collect();
        programs.sort();
        // One program among some scripts (setup, build, update...) is almost always the game.
        let (scripts, binaries): (Vec<PathBuf>, Vec<PathBuf>) =
            programs.into_iter().partition(|p| has_ext(p, "sh") || has_ext(p, "command") || has_ext(p, "py"));
        let programs = match binaries.len() {
            1 => binaries,
            0 => scripts,
            _ => binaries.into_iter().chain(scripts).collect(),
        };
        return match programs.len() {
            0 => Err(Refusal::NothingToRun),
            1 => {
                let mut entry = entry_for(&programs[0], cfg)?;
                // A folder's name ("Cave Story") usually beats its program's ("CaveStory.x86_64").
                entry.name = name_from(&path);
                Ok(entry)
            }
            _ => Err(Refusal::Several(programs.iter().map(|p| crate::config::tilde(p)).collect())),
        };
    }
    if has_ext(&path, "desktop") {
        return desktop_entry(&path).ok_or(Refusal::NothingToRun);
    }
    if has_ext(&path, "exe") {
        return Err(Refusal::Windows);
    }
    if let Some((system, folder)) = cfg.and_then(|c| rom_system(c, &path)) {
        return Err(Refusal::Rom { system, folder });
    }
    let dir = path.parent().map(crate::config::tilde);
    let file = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let file = shell_words::quote(&file).into_owned();
    if has_ext(&path, "sh") {
        // Scripts often print or ask things, so they get the terminal.
        let command = if is_executable(&path) { format!("./{file}") } else { format!("sh {file}") };
        return Ok(Entry { name: name_from(&path), command, cwd: dir, mode: Some(LaunchMode::Terminal), system: None });
    }
    if !is_executable(&path) {
        return Err(Refusal::NotExecutable);
    }
    // Native ports usually look for their data next to themselves, so run them from there.
    Ok(Entry { name: name_from(&path), command: format!("./{file}"), cwd: dir, mode: None, system: None })
}

fn has_ext(path: &Path, ext: &str) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

fn is_executable(path: &Path) -> bool {
    path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// "sonic3air_game" → "Sonic3air Game"; names with capitals are left as they are.
fn name_from(path: &Path) -> String {
    let stem = if path.is_dir() { path.file_name() } else { path.file_stem() };
    let stem = stem.unwrap_or_default().to_string_lossy();
    let words: Vec<String> = stem.split(['_', '-', '.', ' ']).filter(|w| !w.is_empty()).map(str::to_string).collect();
    if stem.chars().any(|c| c.is_uppercase()) {
        return words.join(" ");
    }
    words
        .iter()
        .map(|w| {
            let mut chars = w.chars();
            chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// A .desktop file's Name and Exec, minus the field codes (%U and so on) launchers fill in.
fn desktop_entry(path: &Path) -> Option<Entry> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut in_entry = false;
    let (mut name, mut exec) = (None, None);
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        if let Some(v) = line.strip_prefix("Name=") {
            name.get_or_insert_with(|| v.to_string());
        } else if let Some(v) = line.strip_prefix("Exec=") {
            exec.get_or_insert_with(|| v.to_string());
        }
    }
    let exec = exec?;
    let command: Vec<&str> = exec.split_whitespace().filter(|w| !(w.len() == 2 && w.starts_with('%'))).collect();
    Some(Entry { name: name?, command: command.join(" "), ..Entry::default() })
}

/// The configured system a ROM file belongs to, and the folder it should go in.
fn rom_system(cfg: &Config, path: &Path) -> Option<(String, PathBuf)> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    // Archives and disc images could be anything; only say so for a telling extension.
    if ["zip", "7z", "bin", "iso", "chd", "cue", "m3u", "sh"].contains(&ext.as_str()) {
        return None;
    }
    let sys = cfg.system_for_extension(&ext)?;
    let dir = sys.dir.as_deref().unwrap_or(&sys.id);
    let folder = if dir.starts_with('/') || dir.starts_with('~') {
        config::expand_path(dir)
    } else {
        config::expand_path(&cfg.roms_dir).join(dir)
    };
    Some((cfg.system_name(sys), folder))
}

/// The text of games.jsonc with `entry` added: a new file, or the existing one with one more
/// line in its list. None if the existing file can't be edited safely.
pub fn append(existing: Option<&str>, entry: &Entry) -> Option<String> {
    let Some(raw) = existing else {
        return Some(format!("{GAMES_HEADER}{{\n  \"games\": [\n    {}\n  ]\n}}\n", entry.to_json()));
    };
    let before: config::Include = parse(raw)?;
    let end = raw.rfind(']')?;
    let head = raw[..end].trim_end();
    let sep = if before.games.is_empty() { "" } else { "," };
    let edited = format!("{head}{sep}\n    {}\n  {}", entry.to_json(), &raw[end..]);
    // Make sure exactly one game was added, at the end.
    let after: config::Include = parse(&edited)?;
    let added = after.games.len() == before.games.len() + 1;
    (added && after.games.last().is_some_and(|g| g.name == entry.name)).then_some(edited)
}

fn parse(raw: &str) -> Option<config::Include> {
    serde_json::from_reader(json_comments::StripComments::new(raw.as_bytes())).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(name_from(Path::new("/g/sonic3air_game")), "Sonic3air Game");
        assert_eq!(name_from(Path::new("/g/CaveStory.x86_64")), "CaveStory");
        assert_eq!(name_from(Path::new("/g/Celeste")), "Celeste");
    }

    #[test]
    fn appends_to_the_list() {
        let a = Entry { name: "A".into(), command: "./a".into(), ..Entry::default() };
        let b = Entry { name: "B".into(), command: "./b".into(), cwd: Some("~/b".into()), ..Entry::default() };
        let first = append(None, &a).unwrap();
        let second = append(Some(&first), &b).unwrap();
        let games = parse(&second).unwrap().games;
        assert_eq!(games.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["A", "B"]);
        assert_eq!(games[1].cwd.as_deref(), Some("~/b"));
        // Something that isn't a list of games ending the file is left alone.
        assert_eq!(append(Some("{ \"games\": [], \"vars\": { \"x\": \"]\" } }"), &a), None);
    }
}

//! Installed Steam games, read from the library manifests (no Steam API needed).

use crate::config::{expand_path, home};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SteamApp {
    pub appid: String,
    pub name: String,
}

/// Tools Steam installs alongside games; never worth showing.
fn is_tool(name: &str) -> bool {
    const PREFIXES: &[&str] = &["Proton", "Steam Linux Runtime", "Steamworks Common", "SteamVR", "Steam Audio"];
    PREFIXES.iter().any(|p| name.starts_with(p))
}

pub fn find_steam_dir(configured: Option<&str>) -> Option<PathBuf> {
    if let Some(dir) = configured {
        return Some(expand_path(dir));
    }
    let h = home();
    [
        h.join(".local/share/Steam"),
        h.join(".steam/steam"),
        h.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ]
    .into_iter()
    .find(|p| p.join("steamapps").is_dir())
}

pub fn installed_apps(steam_dir: &Path, exclude: &[String]) -> Vec<SteamApp> {
    let mut libraries = vec![steam_dir.to_path_buf()];
    if let Ok(vdf) = std::fs::read_to_string(steam_dir.join("steamapps/libraryfolders.vdf")) {
        for (k, v) in kv_pairs(&vdf) {
            if k == "path" {
                let p = PathBuf::from(v);
                if !libraries.contains(&p) {
                    libraries.push(p);
                }
            }
        }
    }

    let mut apps = Vec::new();
    for lib in libraries {
        let Ok(entries) = std::fs::read_dir(lib.join("steamapps")) else { continue };
        for entry in entries.flatten() {
            let fname = entry.file_name();
            let fname = fname.to_string_lossy();
            if !(fname.starts_with("appmanifest_") && fname.ends_with(".acf")) {
                continue;
            }
            let Ok(acf) = std::fs::read_to_string(entry.path()) else { continue };
            if let Some(app) = parse_manifest(&acf) {
                let excluded = exclude.iter().any(|e| *e == app.appid || *e == app.name);
                if !is_tool(&app.name) && !excluded && !apps.iter().any(|a: &SteamApp| a.appid == app.appid) {
                    apps.push(app);
                }
            }
        }
    }
    apps
}

fn parse_manifest(acf: &str) -> Option<SteamApp> {
    let (mut appid, mut name, mut flags) = (None, None, None);
    for (k, v) in kv_pairs(acf) {
        match k.to_ascii_lowercase().as_str() {
            "appid" if appid.is_none() => appid = Some(v),
            "name" if name.is_none() => name = Some(v),
            "stateflags" if flags.is_none() => flags = v.parse::<u32>().ok(),
            _ => {}
        }
    }
    // StateFlags bit 2 (value 4) means "fully installed".
    if flags.is_some_and(|f| f & 4 == 0) {
        return None;
    }
    Some(SteamApp { appid: appid?, name: name? })
}

/// Pulls `"key" "value"` pairs out of Valve KeyValues text, ignoring nesting.
fn kv_pairs(text: &str) -> impl Iterator<Item = (String, String)> + '_ {
    text.lines().filter_map(|line| {
        let mut parts = line.split('"').skip(1).step_by(2);
        let k = parts.next()?;
        let v = parts.next()?;
        Some((k.to_string(), v.replace("\\\\", "\\")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest() {
        let acf = "\"AppState\"\n{\n\t\"appid\"\t\t\"2379780\"\n\t\"name\"\t\t\"Balatro\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"InstalledDepots\"\n\t{\n\t\t\"name\"\t\"x\"\n\t}\n}";
        let app = parse_manifest(acf).unwrap();
        assert_eq!(app.appid, "2379780");
        assert_eq!(app.name, "Balatro");
        assert!(parse_manifest(&acf.replace("\"4\"", "\"1026\"")).is_none());
    }

    #[test]
    fn tools_hidden() {
        assert!(is_tool("Proton Experimental"));
        assert!(is_tool("Steam Linux Runtime 3.0 (sniper)"));
        assert!(!is_tool("Stardew Valley"));
    }
}

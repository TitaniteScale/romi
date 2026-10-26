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

// ---- achievements ----

/// A value in Steam's binary KeyValues format (its `appcache` files).
#[derive(Debug, Clone, PartialEq)]
enum Kv {
    Map(Vec<(String, Kv)>),
    Str(String),
    Int(i64),
    Float(f32),
}

impl Kv {
    fn get(&self, key: &str) -> Option<&Kv> {
        match self {
            Kv::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn entries(&self) -> &[(String, Kv)] {
        match self {
            Kv::Map(entries) => entries,
            _ => &[],
        }
    }

    fn str(&self) -> Option<&str> {
        match self {
            Kv::Str(s) => Some(s),
            _ => None,
        }
    }

    fn int(&self) -> Option<i64> {
        match self {
            Kv::Int(n) => Some(*n),
            Kv::Str(s) => s.parse().ok(),
            _ => None,
        }
    }

    /// A display string: English if it's there, else the first language given.
    fn text(&self) -> Option<String> {
        match self {
            Kv::Str(s) => Some(s.clone()),
            Kv::Map(_) => self.get("english").or_else(|| self.entries().first().map(|(_, v)| v)).and_then(Kv::text),
            _ => None,
        }
    }
}

/// Parses binary KeyValues: each entry is a type byte, a NUL-terminated name, then the value.
fn parse_kv(b: &[u8]) -> Option<Kv> {
    fn cstr(b: &[u8], at: &mut usize) -> Option<String> {
        let end = *at + b.get(*at..)?.iter().position(|&c| c == 0)?;
        let s = String::from_utf8_lossy(&b[*at..end]).into_owned();
        *at = end + 1;
        Some(s)
    }
    fn map(b: &[u8], at: &mut usize) -> Option<Kv> {
        let mut entries = Vec::new();
        while let Some(&kind) = b.get(*at) {
            *at += 1;
            if kind == 8 || kind == 11 {
                break;
            }
            let name = cstr(b, at)?;
            let fixed = |at: &mut usize, n: usize| {
                let bytes = b.get(*at..*at + n)?;
                *at += n;
                Some(bytes)
            };
            let value = match kind {
                0 => map(b, at)?,
                1 => Kv::Str(cstr(b, at)?),
                2 | 4 | 6 => Kv::Int(i32::from_le_bytes(fixed(at, 4)?.try_into().ok()?) as i64),
                3 => Kv::Float(f32::from_le_bytes(fixed(at, 4)?.try_into().ok()?)),
                7 | 10 => Kv::Int(i64::from_le_bytes(fixed(at, 8)?.try_into().ok()?)),
                _ => return None,
            };
            entries.push((name, value));
        }
        Some(Kv::Map(entries))
    }
    map(b, &mut 0)
}

/// An app's achievements and your progress in them, from what the Steam client caches for games
/// played on this machine; None if it has none cached. In schema order; rarity isn't cached, so
/// it's unknown (negative) until `with_rarity`.
pub fn achievements(steam_dir: &Path, appid: &str) -> Option<Vec<crate::ra::Cheevo>> {
    let stats = steam_dir.join("appcache/stats");
    let schema = parse_kv(&std::fs::read(stats.join(format!("UserGameStatsSchema_{appid}.bin"))).ok()?)?;
    // One file per Steam account that's played it; the most recently written is yours.
    let suffix = format!("_{appid}.bin");
    let progress = std::fs::read_dir(&stats)
        .ok()?
        .flatten()
        .filter(|e| {
            e.file_name().to_string_lossy().starts_with("UserGameStats_")
                && e.file_name().to_string_lossy().ends_with(&suffix)
        })
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .and_then(|e| parse_kv(&std::fs::read(e.path()).ok()?));
    let progress = progress.as_ref().and_then(|p| p.get("cache"));

    let mut stats_ids: Vec<&(String, Kv)> = schema.entries().first()?.1.get("stats")?.entries().iter().collect();
    stats_ids.sort_by_key(|(id, _)| id.parse::<u32>().unwrap_or(u32::MAX));
    let mut out = Vec::new();
    for (id, stat) in stats_ids {
        let Some(bits) = stat.get("bits") else { continue };
        let mine = progress.and_then(|p| p.get(id));
        let data = mine.and_then(|m| m.get("data")).and_then(Kv::int).unwrap_or(0);
        let mut bits: Vec<&(String, Kv)> = bits.entries().iter().collect();
        bits.sort_by_key(|(bit, _)| bit.parse::<u32>().unwrap_or(u32::MAX));
        for (bit, a) in bits {
            let Ok(n) = bit.parse::<u32>() else { continue };
            let display = a.get("display");
            let earned = n < 32 && data & (1 << n) != 0;
            let hidden = display.and_then(|d| d.get("hidden")).and_then(Kv::int) == Some(1);
            let when = mine.and_then(|m| m.get("AchievementTimes")).and_then(|t| t.get(bit)).and_then(Kv::int);
            let (title, description) = if hidden && !earned {
                ("Hidden achievement".to_string(), String::new())
            } else {
                (
                    display.and_then(|d| d.get("name")).and_then(Kv::text).unwrap_or_default(),
                    display.and_then(|d| d.get("desc")).and_then(Kv::text).unwrap_or_default(),
                )
            };
            out.push(crate::ra::Cheevo {
                key: a.get("name").and_then(Kv::str).unwrap_or_default().to_string(),
                title,
                description,
                points: 0,
                kind: hidden.then(|| "hidden".to_string()),
                earned: earned
                    .then(|| when.map_or_else(|| "earned".to_string(), |t| crate::ui::fmt_datetime(t as u64))),
                earned_hardcore: None,
                rarity: -1.0,
            });
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Fills in how many players have each achievement, from Steam's public global percentages
/// (no key needed).
pub fn with_rarity(appid: &str, mut cheevos: Vec<crate::ra::Cheevo>) -> anyhow::Result<Vec<crate::ra::Cheevo>> {
    use anyhow::Context;
    #[derive(serde::Deserialize)]
    struct Reply {
        achievementpercentages: Percentages,
    }
    #[derive(serde::Deserialize)]
    struct Percentages {
        achievements: Vec<Percent>,
    }
    #[derive(serde::Deserialize)]
    struct Percent {
        name: String,
        // A number, or (as Steam sometimes sends it) a string.
        percent: serde_json::Value,
    }
    let url = format!(
        "https://api.steampowered.com/ISteamUserStats/GetGlobalAchievementPercentagesForApp/v0002/?gameid={appid}"
    );
    let bytes = crate::art::get(&crate::art::agent(), &url, 4 << 20)?.context("Steam: no such game")?;
    let reply: Reply = serde_json::from_slice(&bytes).context("Steam: unexpected achievement percentages")?;
    let percent = |v: &serde_json::Value| v.as_f64().or_else(|| v.as_str()?.parse().ok());
    let by_name: std::collections::HashMap<String, f64> = reply
        .achievementpercentages
        .achievements
        .iter()
        .filter_map(|p| Some((p.name.clone(), percent(&p.percent)?)))
        .collect();
    for c in &mut cheevos {
        if let Some(p) = by_name.get(&c.key) {
            c.rarity = (*p / 100.0) as f32;
        }
    }
    Ok(cheevos)
}

#[cfg(test)]
mod kv_tests {
    use super::*;

    #[test]
    fn binary_keyvalues() {
        let mut b = Vec::new();
        b.extend(b"\x00root\x00");
        b.extend(b"\x01name\x00BAL_01\x00");
        b.extend(b"\x02data\x00");
        b.extend(1073741824i32.to_le_bytes());
        b.extend(b"\x00display\x00\x00name\x00\x01german\x00Ante!\x00\x01english\x00Ante Up!\x00\x08\x08");
        b.extend(b"\x08\x08");
        let kv = parse_kv(&b).unwrap();
        let root = kv.get("root").unwrap();
        assert_eq!(root.get("name").and_then(Kv::str), Some("BAL_01"));
        assert_eq!(root.get("data").and_then(Kv::int), Some(1 << 30));
        assert_eq!(root.get("display").and_then(|d| d.get("name")).and_then(Kv::text).as_deref(), Some("Ante Up!"));
    }
}

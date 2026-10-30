//! Installed Steam games, read from the library manifests (no Steam API needed).

use crate::config::{expand_path, home};
use crate::meta::Meta;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
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
                earned: earned.then(|| when.map_or_else(|| "earned".to_string(), |t| crate::text::datetime(t as u64))),
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

// ---- the store ----

/// What Steam's store says about a game: genres from its players' tags, a description, the year,
/// and its cover's address, which for some games is under a hashed folder the usual one misses.
/// `romi scrape` fetches it, with no key, and the library reads it from the cache.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StoreInfo {
    pub meta: Meta,
    pub cover: Option<String>,
}

/// Where `romi scrape` keeps what the store said, by app id.
fn store_cache_path() -> PathBuf {
    crate::config::cache_dir().join("steam-store.json")
}

/// What the store said about each game the last time `romi scrape` asked, by app id.
pub fn store_cache() -> HashMap<String, StoreInfo> {
    std::fs::read(store_cache_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save_store_cache(cache: &HashMap<String, StoreInfo>) -> anyhow::Result<()> {
    let path = store_cache_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    Ok(crate::files::write_atomic(&path, serde_json::to_vec_pretty(cache)?)?)
}

const STORE_API: &str = "https://api.steampowered.com";
const STORE_ASSETS: &str = "https://shared.steamstatic.com/store_item_assets";
/// Games asked about in one request.
const STORE_BATCH: usize = 50;
/// Tags looked through for genres, most voted first, and the most genres a game gets.
const STORE_TAGS: usize = 20;
const MAX_GENRES: usize = 3;
/// Tags with fewer votes than this share of the top tag's are passing mentions ("Fishing" in
/// Stardew Valley), not what the game is, unless none above it names a genre.
const MIN_TAG_SHARE: f64 = 0.3;
/// Steam's player categories for playing together on one screen: plain, versus and co-op.
const LOCAL_MULTIPLAYER: [u64; 3] = [24, 37, 39];
/// Players' tags that say the same, which they give games the categories miss.
const LOCAL_MULTIPLAYER_TAGS: [&str; 3] = ["local co-op", "local multiplayer", "split screen"];
const SINGLE_PLAYER: u64 = 2;
const MULTIPLAYER: u64 = 1;

/// Asks Steam's store about these games, the way its own pages do, a batch at a time. Games it
/// doesn't know (removed from the store, say) are left out.
pub fn fetch_store_info(agent: &ureq::Agent, appids: &[String]) -> anyhow::Result<HashMap<String, StoreInfo>> {
    use anyhow::Context;
    let get = |url: &str| -> anyhow::Result<serde_json::Value> {
        let bytes = crate::art::get(agent, url, 16 << 20)?.context("Steam's store API isn't there")?;
        serde_json::from_slice(&bytes).context("Steam's store API sent something unexpected")
    };
    let tags = tag_names(&get(&format!("{STORE_API}/IStoreService/GetTagList/v1/?language=english"))?);
    let mut out = HashMap::new();
    for batch in appids.chunks(STORE_BATCH) {
        let ids: Vec<_> =
            batch.iter().filter_map(|id| id.parse::<u32>().ok()).map(|id| json!({ "appid": id })).collect();
        let request = json!({
            "ids": ids,
            "context": { "language": "english", "country_code": "US" },
            "data_request": {
                "include_tag_count": STORE_TAGS,
                "include_basic_info": true,
                "include_release": true,
                "include_assets": true,
            },
        });
        let url = format!(
            "{STORE_API}/IStoreBrowseService/GetItems/v1/?input_json={}",
            crate::art::encode(&request.to_string())
        );
        out.extend(parse_store_items(&get(&url)?, &tags));
    }
    Ok(out)
}

/// Steam's tag names by id, from `GetTagList`.
fn tag_names(reply: &serde_json::Value) -> HashMap<u64, String> {
    let tags = reply["response"]["tags"].as_array().map(Vec::as_slice).unwrap_or_default();
    tags.iter().filter_map(|t| Some((t["tagid"].as_u64()?, t["name"].as_str()?.to_string()))).collect()
}

/// What `GetItems` says about each game, by app id.
fn parse_store_items(reply: &serde_json::Value, tags: &HashMap<u64, String>) -> HashMap<String, StoreInfo> {
    let items = reply["response"]["store_items"].as_array().map(Vec::as_slice).unwrap_or_default();
    items
        .iter()
        .filter(|it| it["success"] == 1)
        .filter_map(|it| Some((it["appid"].as_u64()?.to_string(), store_info(it, tags))))
        .collect()
}

fn store_info(item: &serde_json::Value, tags: &HashMap<u64, String>) -> StoreInfo {
    let text = |v: &serde_json::Value| v.as_str().map(crate::library::unescape).filter(|s| !s.trim().is_empty());
    let first_name = |v: &serde_json::Value| v.as_array()?.first().and_then(|n| text(&n["name"]));
    let info = &item["basic_info"];

    // Players' tags with their votes, most voted first: "Puzzle", "Roguelike Deckbuilder",
    // "Singleplayer".
    let voted: Vec<(String, f64)> = item["tags"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|t| Some((tags.get(&t["tagid"].as_u64()?)?.to_lowercase(), t["weight"].as_f64()?)))
        .collect();
    let top = voted.first().map_or(0.0, |(_, votes)| *votes);
    let genres_in = |min_votes: f64| {
        let mut genres: Vec<String> = Vec::new();
        // "Singleplayer" isn't a genre, but would be taken for one ("sing").
        for (name, _) in voted.iter().filter(|(name, votes)| *votes >= min_votes && !name.contains("player")) {
            if let Some(g) = crate::meta::genre_of(name)
                && !genres.iter().any(|have| have == g)
                && genres.len() < MAX_GENRES
            {
                genres.push(g.to_string());
            }
        }
        genres
    };
    let mut genres = genres_in(top * MIN_TAG_SHARE);
    if genres.is_empty() {
        genres = genres_in(0.0);
    }

    // When the game first came out, which for games that left early access is before they did.
    let release = &item["release"];
    let year = ["original_release_date", "original_steam_release_date", "steam_release_date"]
        .iter()
        .find_map(|k| release[k].as_u64().filter(|&t| t > 0))
        .map(|t| crate::stats::local_year_month(t).0 as u16);

    // How many can play on one screen: at least two, or one when it's single-player only.
    let categories = &item["categories"]["supported_player_categoryids"];
    let categories: Vec<u64> =
        categories.as_array().map(|c| c.iter().filter_map(|v| v.as_u64()).collect()).unwrap_or_default();
    let local = categories.iter().any(|c| LOCAL_MULTIPLAYER.contains(c))
        || voted.iter().any(|(name, _)| LOCAL_MULTIPLAYER_TAGS.contains(&name.as_str()));
    let players = if local {
        Some(2)
    } else if categories.contains(&SINGLE_PLAYER) && !categories.contains(&MULTIPLAYER) {
        Some(1)
    } else {
        None
    };

    let assets = &item["assets"];
    // The portrait cover, at twice the size where there's one.
    let file = assets["library_capsule_2x"].as_str().or(assets["library_capsule"].as_str());
    let cover = match (assets["asset_url_format"].as_str(), file) {
        (Some(format), Some(file)) => Some(format!("{STORE_ASSETS}/{}", format.replace("${FILENAME}", file))),
        _ => None,
    };

    StoreInfo {
        meta: Meta {
            description: text(&info["short_description"]),
            developer: first_name(&info["developers"]),
            publisher: first_name(&info["publishers"]),
            franchise: first_name(&info["franchises"]),
            genres,
            year,
            players,
        },
        cover,
    }
}

#[cfg(test)]
mod store_tests {
    use super::*;

    #[test]
    fn store_items_give_genres_year_players_and_the_cover() {
        let tags = tag_names(&json!({ "response": { "tags": [
            { "tagid": 1664, "name": "Puzzle" },
            { "tagid": 4182, "name": "Singleplayer" },
            { "tagid": 492, "name": "Indie" },
            { "tagid": 1756, "name": "Great Soundtrack" },
            { "tagid": 1773, "name": "Arcade" },
            { "tagid": 4026, "name": "Puzzle Platformer" },
            { "tagid": 1716, "name": "Fishing" },
            { "tagid": 3841, "name": "Local Co-Op" },
        ] } }));
        let reply = json!({ "response": { "store_items": [
            {
                "appid": 736260, "success": 1,
                "tags": [
                    { "tagid": 1664, "weight": 1686 },
                    { "tagid": 4182, "weight": 1130 },
                    { "tagid": 492, "weight": 1062 },
                    { "tagid": 4026, "weight": 783 },
                    { "tagid": 1716, "weight": 400 },
                ],
                "categories": { "supported_player_categoryids": [2] },
                "basic_info": {
                    "short_description": "Baba Is You is a puzzle game where the rules &quot;are&quot; blocks.",
                    "developers": [{ "name": "Hempuli Oy" }],
                    "publishers": [{ "name": "Hempuli Oy" }],
                },
                "release": { "steam_release_date": 1552492825 },
                "assets": {
                    "asset_url_format": "steam/apps/736260/${FILENAME}?t=1760102539",
                    "library_capsule": "96947acc/library_capsule.jpg",
                },
            },
            {
                "appid": 646570, "success": 1,
                "categories": { "supported_player_categoryids": [2, 1, 9, 39] },
                "release": { "steam_release_date": 1548258674, "original_steam_release_date": 1510732543 },
            },
            { "appid": 3, "success": 2 },
        ] } });
        let found = parse_store_items(&reply, &tags);
        assert_eq!(found.len(), 2, "a game the store doesn't know is left out");

        let baba = &found["736260"];
        assert_eq!(
            baba.meta.genres,
            ["Puzzle", "Platformer"],
            "not Music for \"Singleplayer\", or Sports for \"Fishing\""
        );
        assert_eq!(
            baba.meta.description.as_deref(),
            Some("Baba Is You is a puzzle game where the rules \"are\" blocks.")
        );
        assert_eq!(baba.meta.developer.as_deref(), Some("Hempuli Oy"));
        assert_eq!((baba.meta.year, baba.meta.players), (Some(2019), Some(1)));
        assert_eq!(
            baba.cover.as_deref(),
            Some(
                "https://shared.steamstatic.com/store_item_assets/steam/apps/736260/96947acc/library_capsule.jpg?t=1760102539"
            )
        );

        let other = &found["646570"];
        assert_eq!(other.meta.year, Some(2017), "the year it first came out, in early access");
        assert_eq!(other.meta.players, Some(2), "split screen co-op");
        assert_eq!((other.meta.description.as_ref(), other.cover.as_ref()), (None, None));
    }

    #[test]
    fn players_and_genres_from_the_tags_players_voted_for_least() {
        // Online co-op isn't on one screen.
        let online = json!({ "categories": { "supported_player_categoryids": [2, 1, 9, 38] } });
        assert_eq!(store_info(&online, &HashMap::new()).meta.players, None);
        // The main tags say nothing of the genre ("Survival"), so the minor ones do.
        let zomboid = json!({ "tags": [{ "tagid": 1662, "weight": 1000 }, { "tagid": 599, "weight": 290 }] });
        let zomboid_tags = HashMap::from([(1662, "Survival".to_string()), (599, "Simulation".to_string())]);
        assert_eq!(store_info(&zomboid, &zomboid_tags).meta.genres, ["Simulation"]);
        let tagged = json!({
            "categories": { "supported_player_categoryids": [2, 1, 9, 38] },
            "tags": [{ "tagid": 19, "weight": 1000 }, { "tagid": 3841, "weight": 100 }],
        });
        let tags = HashMap::from([(19, "Action".to_string()), (3841, "Local Co-Op".to_string())]);
        assert_eq!(store_info(&tagged, &tags).meta.players, Some(2));
    }
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

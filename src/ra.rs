//! RetroAchievements: which games have achievements, and your progress in them, from
//! retroachievements.org. Your account there is what's true; everything here is a copy of it.
//!
//! Logging in is enough: the login token reads the API emulators use (`dorequest.php`), and needs
//! no setup. That API only says how many achievements you've earned and which, so a web API key (a
//! free one, from Settings on the site), when there is one, is used instead for your awards and the
//! dates you earned them.
//!
//! Games are matched to RetroAchievements' the way RetroArch does it, by a hash of the ROM, for
//! arcade sets and cartridges: so a game only shows achievements if that dump can earn them.
//! Disc-based systems, whose hashing is more involved, are matched by title instead. What's
//! fetched is cached under `~/.cache/romi/ra`: each system's game list for a week, the ROM hashes
//! until a file changes, and your progress until the next refresh (the TUI refreshes when it
//! starts; so does `romi achievements`).

use crate::config::{self, Config, expand_path};
use crate::emu::cheevos::Login;
use crate::files::StampCache;
use crate::library::Library;
use crate::titles::TitleIndex;
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

const API: &str = "https://retroachievements.org/API";
const SERVER: &str = "https://retroachievements.org";
const LIST_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
/// Your progress comes back in pages of at most this many games.
const PAGE: usize = 500;
/// The stand-in achievement the server adds to a game for emulators it doesn't recognise, to warn
/// that hardcore unlocks can't be earned. rcheevos knows it by this id too.
const WARNING_ACHIEVEMENT: u64 = 101_000_001;
/// `Flags` of an achievement that's in the game's official set, as opposed to an unofficial one.
const OFFICIAL: u32 = 3;

/// Where a game's achievements come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    RetroAchievements,
    /// The Steam client's local cache, for games played on this machine.
    Steam,
}

/// What RetroAchievements (or Steam) has for one of your games.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Info {
    pub source: Source,
    /// The game's id there: RetroAchievements' (retroachievements.org/game/<id>), or the Steam
    /// app id.
    pub id: u64,
    pub title: String,
    /// How many achievements it has, and the points they're worth.
    pub total: u32,
    pub points: u32,
    /// How many you've earned, in either mode and in hardcore.
    pub earned: u32,
    pub earned_hardcore: u32,
    /// Your best award for it: "beaten-softcore", "beaten-hardcore", "completed" or "mastered".
    pub award: Option<String>,
    /// Whether this dump can earn them. False when the game matched by title but its ROM's hash
    /// isn't one RetroAchievements knows: another version (often a "Rev 1") is needed.
    pub supported: bool,
}

impl Info {
    /// For people: "12/45 achievements · 330 points · mastered".
    pub fn summary(&self) -> String {
        if !self.supported {
            return format!("{} achievements, for another version of this ROM ({})", self.total, self.hashes_url());
        }
        let mut s = format!("{}/{} achievements", self.earned, self.total);
        if self.source == Source::RetroAchievements {
            s.push_str(&format!(" · {} points", self.points));
        }
        if let Some(award) = &self.award {
            s.push_str(&format!(" · {}", award.replace('-', " ")));
        }
        s
    }

    /// The page listing the dumps RetroAchievements supports for the game.
    pub fn hashes_url(&self) -> String {
        format!("retroachievements.org/game/{}/hashes", self.id)
    }
}

/// Your library's games that RetroAchievements knows, by game key.
#[derive(Debug, Default)]
pub struct Achievements {
    pub games: HashMap<String, Info>,
}

/// A game in a system's list.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Listed {
    title: String,
    #[serde(rename = "ID")]
    id: u64,
    num_achievements: u32,
    points: u32,
    /// MD5s of the dumps it supports (lowercase hex). Lists cached before `systemgames` was used
    /// call them `Hashes`.
    #[serde(default, alias = "SupportedHashes")]
    hashes: Vec<String>,
}

/// Your progress in one game.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct Progress {
    #[serde(rename = "GameID")]
    game_id: u64,
    num_awarded: u32,
    num_awarded_hardcore: u32,
    highest_award_kind: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ProgressPage {
    total: usize,
    results: Vec<Progress>,
}

/// How your progress is read.
enum Account {
    /// A web API key, which also gets your awards and the dates you earned things.
    Key { user: String, key: String },
    /// The saved login, which is all most people have.
    Login(Login),
}

/// A web API key if there's one and a username to go with it (the config's, or the login's), or
/// else the saved login. The key comes from `RA_API_KEY`, or else the key file.
fn account(cfg: &Config) -> Option<Account> {
    let login = crate::emu::cheevos::saved_login();
    let key = std::env::var("RA_API_KEY")
        .ok()
        .or_else(|| std::fs::read_to_string(expand_path(&cfg.retroachievements.key_file)).ok())
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty());
    let user = cfg
        .retroachievements
        .username
        .clone()
        .filter(|u| !u.trim().is_empty())
        .or_else(|| login.as_ref().map(|l| l.username.clone()));
    match (key, user) {
        (Some(key), Some(user)) => Some(Account::Key { user, key }),
        _ => login.map(Account::Login),
    }
}

/// Whether your progress can be read: you're logged in, or have a web API key.
pub fn set_up(cfg: &Config) -> bool {
    account(cfg).is_some()
}

/// Asks the API emulators use. The login token goes in the body, so it can't end up in an error.
fn ask(agent: &ureq::Agent, params: &[(&str, &str)]) -> Result<Vec<u8>> {
    let host = std::env::var(crate::emu::cheevos::HOST_VAR).unwrap_or_else(|_| SERVER.to_string());
    let url = format!("{host}/dorequest.php");
    let attempt = || -> Result<Vec<u8>, ureq::Error> {
        agent.post(&url).send_form(params.iter().copied())?.body_mut().with_config().limit(32 << 20).read_to_vec()
    };
    let mut result = attempt();
    if matches!(&result, Err(e) if !matches!(e, ureq::Error::StatusCode(_))) {
        std::thread::sleep(Duration::from_secs(2));
        result = attempt();
    }
    match result {
        Ok(bytes) => Ok(bytes),
        Err(ureq::Error::StatusCode(401 | 403)) => bail!("retroachievements.org refused your login: log in again"),
        Err(e) => Err(anyhow!("retroachievements.org: {e:#}")),
    }
}

/// One system's games, as `API_GetGameList` gives them but with no key needed.
fn fetch_list(agent: &ureq::Agent, console: u32) -> Result<serde_json::Value> {
    let bytes = ask(agent, &[("r", "systemgames"), ("s", &console.to_string())])?;
    let mut answer: serde_json::Value =
        serde_json::from_slice(&bytes).context("retroachievements.org: unexpected game list")?;
    let list = answer.get_mut("Response").map(serde_json::Value::take).unwrap_or_default();
    serde_json::from_value::<Vec<Listed>>(list.clone()).context("retroachievements.org: unexpected game list")?;
    Ok(list)
}

/// Your progress on these systems, from the login. Games you haven't started are left out.
fn login_progress(agent: &ureq::Agent, login: &Login, consoles: &[u32]) -> Result<Vec<Progress>> {
    #[derive(Default, Deserialize)]
    #[serde(rename_all = "PascalCase", default)]
    struct Entry {
        unlocked: u32,
        unlocked_hardcore: u32,
    }
    let mut progress = Vec::new();
    for c in consoles {
        let params = [("r", "allprogress"), ("u", &*login.username), ("t", &*login.token), ("c", &*c.to_string())];
        let answer: serde_json::Value =
            serde_json::from_slice(&ask(agent, &params)?).context("retroachievements.org: unexpected progress")?;
        // A system you have nothing on can come back as an empty list.
        let Some(games) = answer.get("Response").and_then(|r| r.as_object()) else { continue };
        for (id, entry) in games {
            let Ok(game_id) = id.parse() else { continue };
            let entry: Entry = serde_json::from_value(entry.clone()).unwrap_or_default();
            if entry.unlocked > 0 || entry.unlocked_hardcore > 0 {
                progress.push(Progress {
                    game_id,
                    // Hardcore unlocks count in either mode.
                    num_awarded: entry.unlocked.max(entry.unlocked_hardcore),
                    num_awarded_hardcore: entry.unlocked_hardcore,
                    highest_award_kind: None,
                });
            }
        }
    }
    Ok(progress)
}

/// A game's achievements and which you've earned, from the login.
fn login_game(agent: &ureq::Agent, login: &Login, id: u64) -> Result<Vec<Cheevo>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Answer {
        #[serde(default)]
        sets: Vec<Set>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Set {
        game_id: u64,
        #[serde(default)]
        achievements: Vec<Achievement>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Achievement {
        #[serde(rename = "ID")]
        id: u64,
        title: String,
        description: String,
        points: u32,
        flags: u32,
        #[serde(rename = "Type")]
        kind: Option<String>,
        /// A percentage.
        rarity: Option<f32>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Unlocks {
        #[serde(default)]
        user_unlocks: Vec<u64>,
    }
    let game = id.to_string();
    let (user, token) = (&*login.username, &*login.token);
    let params = [("r", "achievementsets"), ("u", user), ("t", token), ("g", &*game)];
    let answer: Answer =
        serde_json::from_slice(&ask(agent, &params)?).context("retroachievements.org: unexpected game info")?;
    let unlocks = |hardcore: &str| -> Result<Vec<u64>> {
        let params = [("r", "unlocks"), ("u", user), ("t", token), ("g", &*game), ("h", hardcore)];
        let answer: Unlocks =
            serde_json::from_slice(&ask(agent, &params)?).context("retroachievements.org: unexpected unlocks")?;
        Ok(answer.user_unlocks)
    };
    let (softcore, hardcore) = (unlocks("0")?, unlocks("1")?);
    // The answer also holds the game's subsets, which are games of their own.
    let cheevos = answer
        .sets
        .into_iter()
        .filter(|s| s.game_id == id)
        .flat_map(|s| s.achievements)
        .filter(|a| a.flags == OFFICIAL && a.id != WARNING_ACHIEVEMENT)
        .map(|a| {
            let in_hardcore = hardcore.contains(&a.id);
            Cheevo {
                key: a.id.to_string(),
                title: a.title,
                description: a.description,
                points: a.points,
                kind: a.kind.filter(|k| !k.is_empty()),
                earned: (in_hardcore || softcore.contains(&a.id)).then(String::new),
                earned_hardcore: in_hardcore.then(String::new),
                rarity: a.rarity.map_or(-1.0, |r| (r / 100.0).min(1.0)),
            }
        })
        .collect();
    Ok(cheevos)
}

/// One achievement of a game, as the achievements screen shows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cheevo {
    /// Its id where the source has one that matters (Steam's API name).
    #[serde(default)]
    pub key: String,
    pub title: String,
    pub description: String,
    pub points: u32,
    /// "progression", "win_condition", "missable", or none.
    pub kind: Option<String>,
    /// When you earned it ("2026-10-24 12:00:00"), in either mode and in hardcore. Empty when you
    /// have it but the date isn't known: a login doesn't get dates.
    pub earned: Option<String>,
    pub earned_hardcore: Option<String>,
    /// The share of the game's players who have it, 0 to 1; negative when it isn't known.
    pub rarity: f32,
}

/// A game's achievements as last fetched, in RetroAchievements' order.
pub fn cached_game(id: u64) -> Option<Vec<Cheevo>> {
    serde_json::from_slice(&std::fs::read(cache_dir().join(format!("game-{id}.json"))).ok()?).ok()
}

/// Fetches a game's achievements and your progress in them, and caches them.
pub fn fetch_game(cfg: &Config, id: u64) -> Result<Vec<Cheevo>> {
    let cheevos = match account(cfg).context("RetroAchievements isn't set up")? {
        Account::Key { user, key } => key_game(&user, &key, id)?,
        Account::Login(login) => login_game(&crate::art::agent(), &login, id)?,
    };
    std::fs::create_dir_all(cache_dir())?;
    std::fs::write(cache_dir().join(format!("game-{id}.json")), serde_json::to_vec(&cheevos)?)?;
    Ok(cheevos)
}

/// A game's achievements and your progress in them, from a web API key.
fn key_game(user: &str, key: &str, id: u64) -> Result<Vec<Cheevo>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Game {
        num_distinct_players: Option<u32>,
        #[serde(default)]
        achievements: HashMap<String, Achievement>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Achievement {
        title: String,
        description: String,
        points: u32,
        #[serde(rename = "Type")]
        kind: Option<String>,
        num_awarded: u32,
        display_order: i64,
        #[serde(rename = "ID")]
        id: u64,
        date_earned: Option<String>,
        date_earned_hardcore: Option<String>,
    }
    let url = format!("{API}/API_GetGameInfoAndUserProgress.php?g={id}&u={}&y={key}", crate::art::encode(user));
    let bytes = crate::art::get(&crate::art::agent(), &url, 32 << 20)
        .map_err(|e| anyhow!("retroachievements.org: {}", format!("{e:#}").replace(key, "…")))?
        .context("retroachievements.org: no such game")?;
    let game: Game = serde_json::from_slice(&bytes).context("retroachievements.org: unexpected game info")?;
    let players = game.num_distinct_players.unwrap_or(0).max(1) as f32;
    let mut list: Vec<Achievement> = game.achievements.into_values().collect();
    list.sort_by_key(|a| (a.display_order, a.id));
    Ok(list
        .into_iter()
        .map(|a| Cheevo {
            key: a.id.to_string(),
            title: a.title,
            description: a.description,
            points: a.points,
            kind: a.kind.filter(|k| !k.is_empty()),
            earned: a.date_earned,
            earned_hardcore: a.date_earned_hardcore,
            rarity: (a.num_awarded as f32 / players).min(1.0),
        })
        .collect())
}

/// RetroAchievements' console id for an ES-DE (or romi) system id.
pub fn console(system: &str) -> Option<u32> {
    Some(match system {
        "megadrive" | "genesis" => 1,
        "n64" => 2,
        "snes" | "sfc" => 3,
        "gb" => 4,
        "gba" => 5,
        "gbc" => 6,
        "nes" | "famicom" => 7,
        "pcengine" | "tg16" => 8,
        "segacd" | "megacd" | "scd" => 9,
        "sega32x" => 10,
        "mastersystem" => 11,
        "psx" => 12,
        "atarilynx" => 13,
        "ngp" | "ngpc" => 14,
        "gamegear" => 15,
        "gc" => 16,
        "atarijaguar" => 17,
        "nds" => 18,
        "wii" => 19,
        "ps2" => 21,
        "atari2600" => 25,
        // RetroAchievements' arcade sets cover FBNeo's games, Neo Geo and Naomi included.
        "arcade" | "mame" | "fbneo" | "neogeo" | "naomi" | "atomiswave" => 27,
        "virtualboy" => 28,
        "msx" => 29,
        "sg-1000" => 33,
        "saturn" => 39,
        "dreamcast" => 40,
        "psp" => 41,
        "3do" => 43,
        "colecovision" => 44,
        "intellivision" => 45,
        "vectrex" => 46,
        "pcfx" => 49,
        "atari7800" => 51,
        "wonderswan" | "wonderswancolor" => 53,
        "neogeocd" => 56,
        "pcenginecd" | "tg-cd" => 76,
        "fds" => 81,
        _ => return None,
    })
}

fn cache_dir() -> PathBuf {
    config::cache_dir().join("ra")
}

fn list_path(console: u32) -> PathBuf {
    cache_dir().join(format!("list-{console}.json"))
}

fn progress_path() -> PathBuf {
    cache_dir().join("progress.json")
}

impl Achievements {
    /// From what's cached, without going online: RetroAchievements' last refresh, and Steam's
    /// own cache.
    pub fn cached(cfg: &Config, lib: &Library) -> Achievements {
        let lists = consoles(lib).into_iter().filter_map(|c| Some((c, read_list(c)?))).collect();
        let progress = std::fs::read(progress_path())
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<Progress>>(&b).ok())
            .unwrap_or_default();
        let hashes = Hashes::load();
        let mut ach = Achievements::matched(lib, &lists, &progress, |g| hashes.known(lib, g));
        ach.add_steam(cfg, lib);
        ach
    }

    /// Steam games' achievements, from what the Steam client caches.
    fn add_steam(&mut self, cfg: &Config, lib: &Library) {
        let Some(dir) = crate::steam::find_steam_dir(cfg.steam.dir.as_deref()) else { return };
        for game in &lib.games {
            let Some(appid) = &game.art.appid else { continue };
            let Some(list) = crate::steam::achievements(&dir, appid) else { continue };
            let earned = list.iter().filter(|c| c.earned.is_some()).count() as u32;
            let total = list.len() as u32;
            let info = Info {
                source: Source::Steam,
                id: appid.parse().unwrap_or(0),
                title: game.name.clone(),
                total,
                points: 0,
                earned,
                earned_hardcore: 0,
                award: (earned == total).then(|| "perfect".to_string()),
                supported: true,
            };
            self.games.insert(game.key.clone(), info);
        }
    }

    /// Fetches the game lists that are more than a week old, and your progress, then matches.
    pub fn refresh(cfg: &Config, lib: &Library) -> Result<Achievements> {
        let account = account(cfg).context("RetroAchievements isn't set up")?;
        let agent = crate::art::agent();
        std::fs::create_dir_all(cache_dir())?;

        let mut lists = HashMap::new();
        for c in consoles(lib) {
            let fresh = std::fs::metadata(list_path(c))
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().unwrap_or(LIST_MAX_AGE) < LIST_MAX_AGE);
            if !fresh {
                std::fs::write(list_path(c), serde_json::to_vec(&fetch_list(&agent, c)?)?)?;
            }
            if let Some(list) = read_list(c) {
                lists.insert(c, list);
            }
        }

        let progress = match &account {
            Account::Key { user, key } => key_progress(&agent, user, key)?,
            Account::Login(login) => login_progress(&agent, login, &consoles(lib))?,
        };
        std::fs::write(progress_path(), serde_json::to_vec(&progress)?)?;

        let mut hashes = Hashes::load();
        let computed: HashMap<usize, String> =
            (0..lib.games.len()).filter_map(|g| Some((g, hashes.get(lib, g)?))).collect();
        hashes.save();
        let mut ach = Achievements::matched(lib, &lists, &progress, |g| computed.get(&g).cloned());
        ach.add_steam(cfg, lib);
        Ok(ach)
    }

    /// Matches the library's games to RetroAchievements' lists, system by system: by hash where
    /// `hash` has one for a game (and only by hash, since a dump RetroAchievements doesn't know
    /// can't earn anything), by title otherwise.
    fn matched(
        lib: &Library,
        lists: &HashMap<u32, Vec<Listed>>,
        progress: &[Progress],
        hash: impl Fn(usize) -> Option<String>,
    ) -> Achievements {
        let progress: HashMap<u64, &Progress> = progress.iter().map(|p| (p.game_id, p)).collect();
        let mut games = HashMap::new();
        for sys in &lib.systems {
            let Some(list) = console(&sys.id).and_then(|c| lists.get(&c)) else { continue };
            let by_hash: HashMap<String, &Listed> =
                list.iter().flat_map(|l| l.hashes.iter().map(move |h| (h.to_lowercase(), l))).collect();
            // Subsets are extra challenges for a game that's listed on its own anyway.
            // A few games go by two names at once ("HeartGold Version | SoulSilver Version"); each
            // name matches on its own.
            let listed: Vec<&Listed> = list
                .iter()
                .filter(|l| !l.title.contains("[Subset"))
                .flat_map(|l| l.title.split(" | ").map(move |_| l))
                .collect();
            let names = list
                .iter()
                .filter(|l| !l.title.contains("[Subset"))
                .flat_map(|l| l.title.split(" | ").map(matchable))
                .collect();
            let index = TitleIndex::new(names);
            for &g in &sys.games {
                let game = &lib.games[g];
                let titles = std::iter::once(game.name.as_str()).chain(game.art.title.as_deref());
                let by_title = || titles.clone().filter_map(|t| index.find(t)).map(|i| listed[i]).next();
                let (found, supported) = match hash(g) {
                    Some(h) => match by_hash.get(&h) {
                        Some(&l) => (Some(l), true),
                        // Saying another version has achievements needs to be sure it's the same
                        // game, not a sequel with a subtitle: the same words, then.
                        None => (by_title().filter(|l| same_words(&game.name, &l.title)), false),
                    },
                    None => (by_title(), true),
                };
                let Some(l) = found else { continue };
                let p = progress.get(&l.id);
                let info = Info {
                    source: Source::RetroAchievements,
                    id: l.id,
                    title: l.title.clone(),
                    total: l.num_achievements,
                    points: l.points,
                    earned: p.map_or(0, |p| p.num_awarded),
                    earned_hardcore: p.map_or(0, |p| p.num_awarded_hardcore),
                    award: p.and_then(|p| p.highest_award_kind.clone()),
                    supported,
                };
                games.insert(game.key.clone(), info);
            }
        }
        Achievements { games }
    }
}

/// Your progress in everything you've started, with your awards, from a web API key.
fn key_progress(agent: &ureq::Agent, user: &str, key: &str) -> Result<Vec<Progress>> {
    let mut progress = Vec::new();
    loop {
        let url = format!(
            "{API}/API_GetUserCompletionProgress.php?u={}&c={PAGE}&o={}&y={key}",
            crate::art::encode(user),
            progress.len()
        );
        // Errors mustn't carry the URL: it has the key in it.
        let bytes = crate::art::get(agent, &url, 32 << 20)
            .map_err(|e| anyhow!("retroachievements.org: {}", format!("{e:#}").replace(key, "…")))?
            .context("retroachievements.org: not found")?;
        let page: ProgressPage =
            serde_json::from_slice(&bytes).context("retroachievements.org: unexpected progress")?;
        let done = page.results.is_empty() || progress.len() + page.results.len() >= page.total;
        progress.extend(page.results);
        if done {
            return Ok(progress);
        }
    }
}

/// How RetroArch (rcheevos) hashes a console's games. Disc systems and the DS hash particular
/// parts of the image; those are left to title matching.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Hashing {
    /// The MD5 of the set's name ("mk2"), for arcade games.
    SetName,
    /// The MD5 of the ROM, after skipping a header of this many bytes when it's there.
    Rom(Header),
    /// N64 ROMs in any byte order, hashed in the native (big-endian, .z64) one.
    N64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Header {
    None,
    /// A 512-byte copier header, there when the size is 512 over a multiple of `block`.
    Copier {
        block: usize,
    },
    /// A header of `len` bytes starting with `magic` at `at`.
    Magic {
        magic: &'static [u8],
        at: usize,
        len: usize,
    },
}

fn hashing(console: u32) -> Option<Hashing> {
    Some(match console {
        27 => Hashing::SetName,
        2 => Hashing::N64,
        3 => Hashing::Rom(Header::Copier { block: 8192 }),
        8 => Hashing::Rom(Header::Copier { block: 131_072 }),
        7 => Hashing::Rom(Header::Magic { magic: b"NES\x1a", at: 0, len: 16 }),
        81 => Hashing::Rom(Header::Magic { magic: b"FDS\x1a", at: 0, len: 16 }),
        13 => Hashing::Rom(Header::Magic { magic: b"LYNX\0", at: 0, len: 64 }),
        51 => Hashing::Rom(Header::Magic { magic: b"ATARI7800", at: 1, len: 128 }),
        1 | 4 | 5 | 6 | 10 | 11 | 14 | 15 | 25 | 28 | 33 | 44 | 45 | 46 | 53 => Hashing::Rom(Header::None),
        _ => return None,
    })
}

/// The bytes rcheevos hashes for a ROM.
fn hashed_bytes(rom: Vec<u8>, header: Header) -> Vec<u8> {
    let skip = match header {
        Header::None => 0,
        Header::Copier { block } if rom.len() % block == 512 => 512,
        Header::Magic { magic, at, len } if rom.get(at..at + magic.len()) == Some(magic) => len,
        _ => 0,
    };
    rom.get(skip..).map_or(rom.clone(), <[u8]>::to_vec)
}

/// An N64 ROM in big-endian order, whatever order it came in.
fn n64_native(mut rom: Vec<u8>) -> Vec<u8> {
    match rom.get(..4) {
        // .v64: 16-bit words byte-swapped.
        Some([0x37, 0x80, 0x40, 0x12]) => rom.as_chunks_mut::<2>().0.iter_mut().for_each(|c| c.swap(0, 1)),
        // .n64: 32-bit words little-endian.
        Some([0x40, 0x12, 0x37, 0x80]) => rom.as_chunks_mut::<4>().0.iter_mut().for_each(|c| c.reverse()),
        _ => {}
    }
    rom
}

/// ROMs bigger than this aren't read for hashing.
const MAX_ROM: u64 = 256 << 20;

/// A ROM's contents: the file, or for a zip, the biggest file in it.
fn rom_bytes(path: &Path) -> Option<Vec<u8>> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    if ext == "zip" {
        return crate::archive::read_biggest(path, MAX_ROM).map(|(_, data)| data);
    }
    if ext == "7z" || std::fs::metadata(path).ok()?.len() > MAX_ROM {
        return None;
    }
    std::fs::read(path).ok()
}

/// ROM hashes, remembered so each file is read once.
struct Hashes(StampCache<String>);

impl Hashes {
    fn load() -> Hashes {
        Hashes(StampCache::load(cache_dir().join("hashes.tsv")))
    }

    fn save(&self) {
        self.0.save();
    }

    /// What a game hashes to for RetroAchievements, if its system is hashed and it's been hashed
    /// before; reads nothing.
    fn known(&self, lib: &Library, g: usize) -> Option<String> {
        let (path, how) = hashable(lib, g)?;
        if how == Hashing::SetName {
            return Some(set_name_hash(&path));
        }
        self.0.get(&path)
    }

    /// The same, hashing the ROM if it hasn't been.
    fn get(&mut self, lib: &Library, g: usize) -> Option<String> {
        if let Some(h) = self.known(lib, g) {
            return Some(h);
        }
        let (path, how) = hashable(lib, g)?;
        let rom = rom_bytes(&path)?;
        let bytes = match how {
            Hashing::N64 => n64_native(rom),
            Hashing::Rom(header) => hashed_bytes(rom, header),
            Hashing::SetName => unreachable!("set names are hashed in known()"),
        };
        let md5 = format!("{:x}", md5::compute(&bytes));
        self.0.insert(&path, md5.clone());
        Some(md5)
    }
}

/// RetroAchievements' console id for a game's system and the game's hash, hashing it if it hasn't
/// been; None for systems it doesn't hash.
pub fn game_hash(lib: &Library, g: usize) -> Option<(u32, String)> {
    let console = console(&lib.system_of(g).id)?;
    let mut hashes = Hashes::load();
    let hash = match disc_hashed(console) {
        true => hashes.get_disc(lib, g, console)?,
        false => hashes.get(lib, g)?,
    };
    hashes.save();
    Some((console, hash))
}

/// Whether rcheevos hashes this console's games from the disc's files, not the whole file. Only
/// the consoles played in romi are here; the rest go by title.
fn disc_hashed(console: u32) -> bool {
    matches!(console, 16 | 19 | 21)
}

impl Hashes {
    /// A disc game's hash, read by rcheevos (see `cheevos::hash_disc`), remembered.
    fn get_disc(&mut self, lib: &Library, g: usize, console: u32) -> Option<String> {
        let game = &lib.games[g];
        let path = game.path.clone().filter(|_| game.discs.is_empty())?;
        if let Some(hash) = self.0.get(&path) {
            return Some(hash);
        }
        let hash = crate::emu::cheevos::hash_disc(console, &path)?;
        self.0.insert(&path, hash.clone());
        Some(hash)
    }
}

/// What RetroAchievements knows a game file by on `console`, read from the file; None if the
/// console isn't hashed from files, or the file can't be read.
pub fn hash_file(console: u32, path: &Path) -> Option<String> {
    if disc_hashed(console) {
        return crate::emu::cheevos::hash_disc(console, path);
    }
    match hashing(console)? {
        Hashing::SetName => Some(set_name_hash(path)),
        how => {
            let rom = rom_bytes(path)?;
            let bytes = match how {
                Hashing::N64 => n64_native(rom),
                Hashing::Rom(header) => hashed_bytes(rom, header),
                Hashing::SetName => unreachable!("set names are hashed above"),
            };
            Some(format!("{:x}", md5::compute(&bytes)))
        }
    }
}

/// A game's ROM file and how its system hashes it, when it does.
fn hashable(lib: &Library, g: usize) -> Option<(PathBuf, Hashing)> {
    let game = &lib.games[g];
    // A multi-disc game is a disc system's, and those go by title anyway.
    if !game.discs.is_empty() {
        return None;
    }
    let how = hashing(console(&lib.system_of(g).id)?)?;
    Some((game.path.clone()?, how))
}

fn set_name_hash(path: &Path) -> String {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    format!("{:x}", md5::compute(stem.as_bytes()))
}

/// The same words, ignoring dashes and how they're spaced ("Megaman" for "Mega Man").
fn same_words(ours: &str, theirs: &str) -> bool {
    let joined = |t: &str| crate::titles::words(t).into_iter().filter(|w| w != "-").collect::<String>();
    joined(ours) == joined(&matchable(theirs))
}

/// The consoles the library has systems for.
fn consoles(lib: &Library) -> Vec<u32> {
    let mut c: Vec<u32> = lib.systems.iter().filter_map(|s| console(&s.id)).collect();
    c.sort_unstable();
    c.dedup();
    c
}

/// A system's games that have achievements, as last fetched.
fn read_list(console: u32) -> Option<Vec<Listed>> {
    let mut list: Vec<Listed> = serde_json::from_slice(&std::fs::read(list_path(console)).ok()?).ok()?;
    list.retain(|l| l.num_achievements > 0);
    Some(list)
}

/// RetroAchievements marks hacks, homebrew and the like with a `~Tag~` before the title. Turned
/// into a trailing tag, the title matcher ignores it for the words and ranks the real game first.
fn matchable(title: &str) -> String {
    let mut rest = title.trim();
    let mut tags = Vec::new();
    while let Some(after) = rest.strip_prefix('~') {
        let Some(end) = after.find('~') else { break };
        tags.push(format!("({})", &after[..end]));
        rest = after[end + 1..].trim_start();
    }
    if tags.is_empty() { rest.to_string() } else { format!("{rest} {}", tags.join(" ")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_move_to_the_end() {
        assert_eq!(matchable("~Hack~ Sonic 1 Easy Mode"), "Sonic 1 Easy Mode (Hack)");
        assert_eq!(matchable("~Hack~ ~Demo~ Gatoslip: Chapter 0"), "Gatoslip: Chapter 0 (Hack) (Demo)");
        assert_eq!(matchable("Sonic the Hedgehog 2"), "Sonic the Hedgehog 2");
    }

    #[test]
    fn real_games_beat_hacks_of_the_same_name() {
        let names = ["~Hack~ Sonic the Hedgehog 2", "Sonic the Hedgehog 2", "~Hack~ Amy Rose in Sonic the Hedgehog 2"];
        let index = TitleIndex::new(names.iter().map(|n| matchable(n)).collect());
        assert_eq!(index.find("Sonic The Hedgehog 2"), Some(1));
        assert_eq!(index.find("Amy Rose in Sonic the Hedgehog 2"), Some(2));
    }

    #[test]
    fn headers_and_byte_orders() {
        let mut snes = vec![0xAA; 512];
        snes.extend(vec![1; 8192]);
        assert_eq!(hashed_bytes(snes, Header::Copier { block: 8192 }), vec![1; 8192]);
        assert_eq!(hashed_bytes(vec![1; 8192], Header::Copier { block: 8192 }), vec![1; 8192]);
        let mut nes = b"NES\x1a".to_vec();
        nes.extend([0; 12]);
        nes.extend([7; 4]);
        assert_eq!(hashed_bytes(nes, Header::Magic { magic: b"NES\x1a", at: 0, len: 16 }), [7; 4]);
        let z64 = vec![0x80, 0x37, 0x12, 0x40, 1, 2, 3, 4];
        assert_eq!(n64_native(vec![0x37, 0x80, 0x40, 0x12, 2, 1, 4, 3]), z64);
        assert_eq!(n64_native(vec![0x40, 0x12, 0x37, 0x80, 4, 3, 2, 1]), z64);
        assert_eq!(set_name_hash(Path::new("/roms/arcade/mk2.zip")), "a683090976ec0f04dca81f6db9ca7484");
    }

    #[test]
    fn summaries() {
        let info = Info {
            source: Source::RetroAchievements,
            id: 1,
            title: "X".into(),
            total: 45,
            points: 330,
            earned: 45,
            earned_hardcore: 45,
            award: Some("beaten-hardcore".into()),
            supported: true,
        };
        assert_eq!(info.summary(), "45/45 achievements · 330 points · beaten hardcore");
        let other = Info { supported: false, ..info };
        assert_eq!(
            other.summary(),
            "45 achievements, for another version of this ROM (retroachievements.org/game/1/hashes)"
        );
    }
}

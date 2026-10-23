//! Box art: finding what's already on disk, and `romi --scrape` for the rest.
//!
//! Lookup order: a path set in the config, romi's own cache, ES-DE's downloaded_media, the
//! Skyscraper cache, then Steam's library cache. Only the cache is written to; the others are
//! read in place.
//!
//! Online sources need no keys: libretro's thumbnail server (RetroArch's box art, named after
//! No-Intro/Redump/MAME titles) and Steam's CDN. Art is fetched on the user's machine and cached
//! under `~/.cache/romi/art`; none of it ships with romi.

use crate::config::{self, Config, expand_path};
use crate::library::{Game, Library, System};
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

const LIBRETRO: &str = "https://thumbnails.libretro.com";
const STEAM_CDN: &str = "https://shared.steamstatic.com/store_item_assets/steam/apps";
const LISTING_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
const WORKERS: usize = 4;
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp"];

/// libretro thumbnail sets for ES-DE system ids, best first.
pub fn default_libretro(id: &str) -> Vec<String> {
    let sets: &[&str] = match id {
        "arcade" | "mame" | "fbneo" => {
            &["MAME", "FBNeo - Arcade Games", "Sega - Naomi", "Sega - Naomi 2", "Atomiswave"]
        }
        "neogeo" => &["SNK - Neo Geo", "FBNeo - Arcade Games", "MAME"],
        "naomi" | "naomi2" | "atomiswave" => &["Sega - Naomi", "Sega - Naomi 2", "Atomiswave", "MAME"],
        "gb" => &["Nintendo - Game Boy"],
        "gbc" => &["Nintendo - Game Boy Color", "Nintendo - Game Boy"],
        "gba" => &["Nintendo - Game Boy Advance"],
        "nds" => &["Nintendo - Nintendo DS"],
        "nes" => &["Nintendo - Nintendo Entertainment System"],
        "snes" => &["Nintendo - Super Nintendo Entertainment System"],
        "n64" => &["Nintendo - Nintendo 64"],
        "gc" => &["Nintendo - GameCube"],
        "wii" => &["Nintendo - Wii"],
        "mastersystem" => &["Sega - Master System - Mark III"],
        "gamegear" => &["Sega - Game Gear"],
        "megadrive" | "genesis" => &["Sega - Mega Drive - Genesis"],
        "sega32x" => &["Sega - 32X"],
        "scd" | "segacd" | "megacd" => &["Sega - Mega-CD - Sega CD"],
        "saturn" => &["Sega - Saturn"],
        "dreamcast" => &["Sega - Dreamcast"],
        "pcengine" | "tg16" => &["NEC - PC Engine - TurboGrafx 16", "NEC - PC Engine SuperGrafx"],
        "pcenginecd" | "tg-cd" => &["NEC - PC Engine CD - TurboGrafx-CD"],
        "ngp" => &["SNK - Neo Geo Pocket"],
        "ngpc" => &["SNK - Neo Geo Pocket Color", "SNK - Neo Geo Pocket"],
        "psx" => &["Sony - PlayStation"],
        "ps2" => &["Sony - PlayStation 2"],
        "psp" => &["Sony - PlayStation Portable"],
        "atari2600" => &["Atari - 2600"],
        "atarilynx" => &["Atari - Lynx"],
        "wonderswan" => &["Bandai - WonderSwan"],
        "wonderswancolor" => &["Bandai - WonderSwan Color"],
        _ => &[],
    };
    sets.iter().map(|s| s.to_string()).collect()
}

/// Skyscraper's platform folder names, where they differ from ES-DE's.
pub fn default_skyscraper(id: &str) -> String {
    match id {
        "scd" => "segacd",
        "gc" => "gamecube",
        other => other,
    }
    .to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Manual,
    Cache,
    EsDe,
    Skyscraper,
    Steam,
}

/// Finds art already on disk. Keeps Skyscraper's file-name index around between lookups.
pub struct Finder {
    esde: Option<PathBuf>,
    skyscraper: Option<PathBuf>,
    steam: Option<PathBuf>,
    sky_index: HashMap<String, HashMap<String, PathBuf>>,
}

impl Finder {
    pub fn new(cfg: &Config) -> Finder {
        Finder {
            esde: cfg.art.esde_media_dir.as_deref().map(expand_path),
            skyscraper: cfg.art.skyscraper_cache.as_deref().map(expand_path),
            steam: crate::steam::find_steam_dir(cfg.steam.dir.as_deref()).map(|d| d.join("appcache/librarycache")),
            sky_index: HashMap::new(),
        }
    }

    pub fn find(&mut self, sys: &System, game: &Game) -> Option<(PathBuf, Source)> {
        if let Some(p) = game.art.manual.as_ref().filter(|p| p.is_file()) {
            return Some((p.clone(), Source::Manual));
        }
        if let Some(p) = existing_image(&cache_stem(sys, game)) {
            return Some((p, Source::Cache));
        }
        if let (Some(root), Some(path)) = (&self.esde, &game.path) {
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            for kind in ["covers", "3dboxes"] {
                if let Some(p) = existing_image(&root.join(&sys.esde).join(kind).join(&*stem)) {
                    return Some((p, Source::EsDe));
                }
            }
        }
        if let Some(path) = &game.path {
            let file = path.file_name().unwrap_or_default().to_string_lossy();
            if let Some(p) = self.skyscraper_index(&sys.skyscraper).get(&*file) {
                return Some((p.clone(), Source::Skyscraper));
            }
        }
        if let (Some(cache), Some(appid)) = (&self.steam, &game.art.appid) {
            let p = cache.join(appid).join("library_600x900.jpg");
            if p.is_file() {
                return Some((p, Source::Steam));
            }
        }
        None
    }

    /// Skyscraper names covers by a content hash; `quickid.xml` maps file paths to those hashes.
    /// The paths may come from another machine (a handheld's SD card), so match on file name.
    fn skyscraper_index(&mut self, platform: &str) -> &HashMap<String, PathBuf> {
        let root = self.skyscraper.clone();
        self.sky_index.entry(platform.to_string()).or_insert_with(|| {
            let mut index = HashMap::new();
            let Some(dir) = root.map(|r| r.join(platform)) else { return index };
            let Ok(xml) = std::fs::read_to_string(dir.join("quickid.xml")) else { return index };
            let Ok(doc) = roxmltree::Document::parse(&xml) else { return index };
            for node in doc.descendants().filter(|n| n.has_tag_name("quickid")) {
                let (Some(path), Some(id)) = (node.attribute("filepath"), node.attribute("id")) else { continue };
                let cover = dir.join("covers/screenscraper").join(id);
                if cover.is_file() {
                    let name = Path::new(path).file_name().unwrap_or_default().to_string_lossy().into_owned();
                    index.insert(name, cover);
                }
            }
            index
        })
    }
}

fn existing_image(stem: &Path) -> Option<PathBuf> {
    IMAGE_EXTS.iter().map(|ext| with_ext(stem, ext)).find(|p| p.is_file())
}

/// Appends an extension without replacing one that's already there ("Game (USA).chd.png").
fn with_ext(stem: &Path, ext: &str) -> PathBuf {
    let mut s = stem.as_os_str().to_owned();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

/// Where the scraper stores art for a game, minus the image extension.
fn cache_stem(sys: &System, game: &Game) -> PathBuf {
    let (_, rest) = game.key.split_once(':').unwrap_or(("", &game.key));
    config::cache_dir().join("art").join(&sys.id).join(rest.replace('/', "__"))
}

// ---- matching against libretro's file names ----

/// libretro replaces these characters with `_` in thumbnail file names.
fn libretro_safe(title: &str) -> String {
    title.chars().map(|c| if "&*/:`<>?\\|\"".contains(c) { '_' } else { c }).collect()
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// A title reduced to comparable words: no tags, articles, "and", or punctuation. A lone "-"
/// marks where a subtitle starts.
fn words(title: &str) -> Vec<String> {
    let t = strip_tags(title).to_lowercase().replace('&', " and ").replace(['\'', '’'], "");
    let t: String = t.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { ' ' }).collect();
    let mut out: Vec<String> =
        t.split_whitespace().filter(|w| !matches!(*w, "the" | "and")).map(String::from).collect();
    if matches!(out.first().map(String::as_str), Some("a" | "an")) {
        out.remove(0);
    }
    out
}

/// Lower is better: real releases before demos/bootlegs, then by region, then the plainest name.
fn rank(entry: &str) -> (bool, usize, usize) {
    const BAD: &[&str] =
        &["(Demo", "(Beta", "(Proto", "(Sample", "(Kiosk", "(Hack", "(Pirate", "bootleg", "prototype", "Korean"];
    const REGIONS: &[&str] = &["(USA", "(World", "(Europe", "(Japan"];
    let bad = BAD.iter().any(|b| entry.contains(b));
    let region = REGIONS.iter().position(|r| entry.contains(r)).unwrap_or(REGIONS.len());
    (bad, region, entry.len())
}

/// One libretro thumbnail set's file names, pre-split for matching.
struct Listing {
    set: String,
    names: Vec<String>,
    exact: HashSet<String>,
    words: Vec<Vec<String>>,
}

impl Listing {
    fn new(set: String, names: Vec<String>) -> Listing {
        let words = names.iter().map(|n| words(n)).collect();
        let exact = names.iter().cloned().collect();
        Listing { set, names, exact, words }
    }

    fn url(&self, name: &str) -> String {
        format!("{LIBRETRO}/{}/Named_Boxarts/{}.png", encode(&self.set), encode(name))
    }
}

/// Finds the best thumbnail for `title` across sets, trying stricter matches first:
/// the exact file name, the same words, the same words plus a subtitle, then ignoring dashes.
fn best_match<'a>(title: &str, listings: &[&'a Listing]) -> Option<(&'a Listing, &'a str)> {
    let safe = libretro_safe(title);
    if let Some(l) = listings.iter().find(|l| l.exact.contains(&safe)) {
        return Some((l, l.names.iter().find(|n| **n == safe)?.as_str()));
    }
    let want = words(title);
    if want.is_empty() {
        return None;
    }
    let undashed = |w: &[String]| w.iter().filter(|x| *x != "-").cloned().collect::<Vec<_>>();
    let want_undashed = undashed(&want);
    type Tier<'t> = &'t dyn Fn(&[String]) -> bool;
    let tiers: [Tier; 3] =
        [&|w| w == want.as_slice(), &|w| w.len() > want.len() && w.starts_with(&want) && w[want.len()] == "-", &|w| {
            undashed(w) == want_undashed
        }];
    for tier in tiers {
        let best = listings
            .iter()
            .copied()
            .flat_map(|l| {
                l.words.iter().enumerate().filter(|(_, w)| tier(w)).map(move |(i, _)| (l, l.names[i].as_str()))
            })
            .min_by_key(|(_, name)| rank(name));
        if best.is_some() {
            return best;
        }
    }
    None
}

/// Percent-encodes a URL path segment.
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
        if let (b'%', Some(Ok(b))) = (bytes[i], hex.map(|h| u8::from_str_radix(h, 16))) {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---- scraping ----

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .user_agent(concat!("romi/", env!("CARGO_PKG_VERSION"), " (+https://github.com/TitaniteScale/romi)"))
        .build()
        .into()
}

/// GET with one retry for dropped connections. `None` means the server has no such file.
fn get(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Option<Vec<u8>>> {
    let attempt = || -> Result<Vec<u8>, ureq::Error> {
        agent.get(url).call()?.body_mut().with_config().limit(limit).read_to_vec()
    };
    let mut result = attempt();
    if matches!(&result, Err(e) if !matches!(e, ureq::Error::StatusCode(_))) {
        std::thread::sleep(Duration::from_secs(2));
        result = attempt();
    }
    match result {
        Ok(bytes) => Ok(Some(bytes)),
        Err(ureq::Error::StatusCode(404)) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// A set's file names, from a week-old-at-most local copy of its directory listing.
fn listing(agent: &ureq::Agent, set: &str) -> Result<Listing> {
    let cached = config::cache_dir().join("libretro").join(format!("{set}.txt"));
    let fresh = std::fs::metadata(&cached)
        .and_then(|m| m.modified())
        .is_ok_and(|t| t.elapsed().unwrap_or(LISTING_MAX_AGE) < LISTING_MAX_AGE);
    if fresh {
        let text = std::fs::read_to_string(&cached)?;
        return Ok(Listing::new(set.to_string(), text.lines().map(String::from).collect()));
    }
    let html = get(agent, &format!("{LIBRETRO}/{}/Named_Boxarts/", encode(set)), 64 << 20)
        .with_context(|| format!("listing {set}"))?
        .with_context(|| format!("no thumbnail set named \"{set}\" on {LIBRETRO}"))?;
    let html = String::from_utf8_lossy(&html);
    let names: Vec<String> = html
        .split("href=\"")
        .skip(1)
        .filter_map(|s| s.split('"').next())
        .filter(|href| href.ends_with(".png") && !href.contains('/'))
        .map(|href| decode(&href[..href.len() - 4]))
        .collect();
    if let Some(dir) = cached.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&cached, names.join("\n"))?;
    Ok(Listing::new(set.to_string(), names))
}

struct Job {
    url: String,
    dest_stem: PathBuf,
}

/// Saves one image. `Ok(false)` means the server doesn't have it.
fn download(agent: &ureq::Agent, job: &Job) -> Result<bool> {
    let Some(bytes) = get(agent, &job.url, 32 << 20)? else { return Ok(false) };
    let ext = if bytes.starts_with(b"\x89PNG") {
        "png"
    } else if bytes.starts_with(b"\xFF\xD8") {
        "jpg"
    } else {
        anyhow::bail!("not an image");
    };
    let dest = with_ext(&job.dest_stem, ext);
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = with_ext(&job.dest_stem, "part");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &dest)?;
    Ok(true)
}

#[derive(Default)]
struct Tally {
    local: usize,
    downloaded: usize,
    failed: usize,
    missing: Vec<String>,
}

/// `romi --scrape [system...]`: reports art already on disk and downloads what it can find.
pub fn scrape(cfg: &Config, lib: &Library, only: &[String]) -> Result<()> {
    let agent = agent();
    let mut finder = Finder::new(cfg);
    let mut out = std::io::stdout().lock();
    // Progress overwrites its own line on a terminal; piped output gets plain lines.
    let (clear_start, clear_end) = if out.is_terminal() { ("\r", "\x1b[K") } else { ("", "") };
    let mut total = Tally::default();
    let mut listings: HashMap<String, Option<Listing>> = HashMap::new();

    for sys in &lib.systems {
        if !only.is_empty() && !only.contains(&sys.id) {
            continue;
        }
        let mut tally = Tally::default();
        let mut jobs = Vec::new();
        let mut unmatched = Vec::new();

        for &g in &sys.games {
            let game = &lib.games[g];
            if finder.find(sys, game).is_some() {
                tally.local += 1;
            } else if !cfg.art.online {
                unmatched.push(g);
            } else if let Some(appid) = &game.art.appid {
                jobs.push((
                    g,
                    Job { url: format!("{STEAM_CDN}/{appid}/library_600x900.jpg"), dest_stem: cache_stem(sys, game) },
                ));
            } else {
                unmatched.push(g);
            }
        }

        if cfg.art.online && !unmatched.is_empty() && !sys.libretro.is_empty() {
            for set in &sys.libretro {
                if !listings.contains_key(set) {
                    let l = match listing(&agent, set) {
                        Ok(l) => Some(l),
                        Err(e) => {
                            writeln!(out, "  ! {e:#}")?;
                            None
                        }
                    };
                    listings.insert(set.clone(), l);
                }
            }
            let sets: Vec<&Listing> = sys.libretro.iter().filter_map(|s| listings[s].as_ref()).collect();
            unmatched.retain(|&g| {
                let game = &lib.games[g];
                let Some(title) = &game.art.title else { return true };
                match best_match(title, &sets) {
                    Some((l, name)) => {
                        jobs.push((g, Job { url: l.url(name), dest_stem: cache_stem(sys, game) }));
                        false
                    }
                    None => true,
                }
            });
        }

        let (not_found, failed) = run_jobs(&agent, &jobs, &sys.name);
        tally.downloaded = jobs.len() - not_found.len() - failed.len();
        tally.failed = failed.len();
        tally.missing = unmatched.iter().chain(&not_found).chain(&failed).map(|&g| lib.games[g].name.clone()).collect();
        tally.missing.sort();

        let mut line = format!("  {:<24} {:>4} games   {:>4} on disk", sys.name, sys.games.len(), tally.local);
        if tally.downloaded > 0 {
            line.push_str(&format!("   {:>4} downloaded", tally.downloaded));
        }
        if !tally.missing.is_empty() {
            line.push_str(&format!("   {:>4} missing", tally.missing.len()));
        }
        writeln!(out, "{clear_start}{line}{clear_end}")?;

        total.local += tally.local;
        total.downloaded += tally.downloaded;
        total.failed += tally.failed;
        total.missing.extend(tally.missing.into_iter().map(|n| format!("{}: {n}", sys.name)));
    }

    writeln!(out, "\n{} on disk, {} downloaded, {} without art", total.local, total.downloaded, total.missing.len())?;
    if !total.missing.is_empty() {
        writeln!(out, "\nno art found for:")?;
        for m in &total.missing {
            writeln!(out, "  {m}")?;
        }
        writeln!(out, "\nset \"art\": \"/path/to/image.png\" on a game or override to fill these in by hand.")?;
    }
    if total.failed > 0 {
        writeln!(out, "{} downloads failed; run again to retry.", total.failed)?;
    }
    Ok(())
}

/// Downloads in parallel. Returns the games the server had no image for, and those that failed.
fn run_jobs(agent: &ureq::Agent, jobs: &[(usize, Job)], label: &str) -> (Vec<usize>, Vec<usize>) {
    if jobs.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let progress = std::io::stderr().is_terminal();
    let next = Mutex::new(0usize);
    let done = Mutex::new(0usize);
    let not_found = Mutex::new(Vec::new());
    let failed = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..WORKERS.min(jobs.len()) {
            s.spawn(|| {
                loop {
                    let i = {
                        let mut n = next.lock().unwrap();
                        if *n >= jobs.len() {
                            return;
                        }
                        *n += 1;
                        *n - 1
                    };
                    let (g, job) = &jobs[i];
                    match download(agent, job) {
                        Ok(true) => {}
                        Ok(false) => not_found.lock().unwrap().push(*g),
                        Err(_) => failed.lock().unwrap().push(*g),
                    }
                    let mut d = done.lock().unwrap();
                    *d += 1;
                    if progress {
                        eprint!("\r  {label:<24} downloading {}/{}\x1b[K", *d, jobs.len());
                    }
                }
            });
        }
    });
    (not_found.into_inner().unwrap(), failed.into_inner().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matched(title: &str, names: &[&str]) -> Option<String> {
        let l = Listing::new("Test".into(), names.iter().map(|s| s.to_string()).collect());
        best_match(title, &[&l]).map(|(_, n)| n.to_string())
    }

    #[test]
    fn exact_and_sanitized() {
        assert_eq!(matched("Ape Escape (USA)", &["Ape Escape (USA)"]).as_deref(), Some("Ape Escape (USA)"));
        assert_eq!(
            matched(
                "Dungeons & Dragons: Shadow over Mystara (USA 960619)",
                &["Dungeons _ Dragons_ Shadow over Mystara (USA 960619)"]
            )
            .as_deref(),
            Some("Dungeons _ Dragons_ Shadow over Mystara (USA 960619)")
        );
    }

    #[test]
    fn fuzzy_tiers() {
        let names = [
            "Puzzle Link (Europe)",
            "Puzzle Link 2 (USA, Europe)",
            "Baseball Stars Color - Pocket Sports Series (World) (En,Ja)",
        ];
        assert_eq!(matched("Puzzle Link", &names).as_deref(), Some("Puzzle Link (Europe)"));
        assert_eq!(matched("Baseball Stars Color", &names).as_deref(), Some(names[2]));
        assert_eq!(
            matched(
                "Legend of Zelda, The - The Wind Waker (USA, Canada)",
                &["Legend of Zelda, The - The Wind Waker (USA)"]
            )
            .as_deref(),
            Some("Legend of Zelda, The - The Wind Waker (USA)")
        );
        assert_eq!(
            matched(
                "Sonic the Hedgehog Pocket Adventure",
                &[
                    "Sonic The Hedgehog - Pocket Adventure (World) (Demo)",
                    "Sonic The Hedgehog - Pocket Adventure (World)"
                ]
            )
            .as_deref(),
            Some("Sonic The Hedgehog - Pocket Adventure (World)")
        );
        assert_eq!(matched("Nige-Ron-Pa", &["Nigeronpa (Japan)"]), None);
    }

    #[test]
    fn prefers_usa_and_real_releases() {
        let names = ["Chibi-Robo! (Europe) (En,Fr,De,Es,It)", "Chibi-Robo! (USA) (Demo)", "Chibi-Robo! (Japan)"];
        assert_eq!(matched("Chibi-Robo! (USA)", &names).as_deref(), Some("Chibi-Robo! (Europe) (En,Fr,De,Es,It)"));
    }

    #[test]
    fn url_encoding_round_trips() {
        let s = "Pokémon - Red (USA, Europe) & more";
        assert_eq!(decode(&encode(s)), s);
        assert_eq!(encode("A B(C)"), "A%20B%28C%29");
    }
}

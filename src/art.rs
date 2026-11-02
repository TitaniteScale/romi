//! Box art, title screens and screenshots: finding what's already on disk, and `romi scrape` for
//! the rest.
//!
//! Lookup order: a path set in the config (covers only), romi's own cache, ES-DE's
//! downloaded_media, the Skyscraper cache, then Steam's library cache (covers only). romi only
//! writes to its cache and to the covers added from a game's menu; the others are read in place.
//!
//! Online sources need no keys: libretro's thumbnail server (RetroArch's box art, title screens
//! and screenshots, named after No-Intro/Redump/MAME titles) and Steam's CDN. Art is fetched on the user's machine and cached
//! under `~/.cache/romi/art`; none of it ships with romi.

use crate::config::{self, Config, expand_path};
use crate::library::{Game, Library, System};
use crate::titles::{TitleIndex, best_across};
use anyhow::{Context, Result};
use std::collections::HashMap;
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

/// What a picture of a game shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Cover,
    /// The title screen.
    Title,
    /// A screenshot of it being played.
    Snap,
}

impl Kind {
    /// The kinds `romi scrape` fetches and the TUI shows.
    pub fn wanted(cfg: &Config) -> &'static [Kind] {
        if cfg.art.screens { &[Kind::Cover, Kind::Title, Kind::Snap] } else { &[Kind::Cover] }
    }

    /// The folder on libretro's thumbnail server.
    fn libretro_dir(self) -> &'static str {
        match self {
            Kind::Cover => "Named_Boxarts",
            Kind::Title => "Named_Titles",
            Kind::Snap => "Named_Snaps",
        }
    }

    /// ES-DE's downloaded_media folders for it, best first.
    fn esde_dirs(self) -> &'static [&'static str] {
        match self {
            Kind::Cover => &["covers", "3dboxes"],
            Kind::Title => &["titlescreens"],
            Kind::Snap => &["screenshots"],
        }
    }

    /// Skyscraper's cache folder for it, if it keeps one.
    fn skyscraper_dir(self) -> Option<&'static str> {
        match self {
            Kind::Cover => Some("covers"),
            Kind::Title => None,
            Kind::Snap => Some("screenshots"),
        }
    }

    /// romi's cache keeps covers loose in a system's folder and the others in subfolders.
    fn cache_dir(self) -> Option<&'static str> {
        match self {
            Kind::Cover => None,
            Kind::Title => Some("titles"),
            Kind::Snap => Some("snaps"),
        }
    }

    /// For people: "covers", "titles", "snaps".
    pub fn plural(self) -> &'static str {
        match self {
            Kind::Cover => "covers",
            Kind::Title => "titles",
            Kind::Snap => "snaps",
        }
    }
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
    sky_index: HashMap<(String, &'static str), HashMap<String, PathBuf>>,
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

    /// A game's cover.
    pub fn find(&mut self, sys: &System, game: &Game) -> Option<(PathBuf, Source)> {
        self.find_kind(sys, game, Kind::Cover)
    }

    pub fn find_kind(&mut self, sys: &System, game: &Game, kind: Kind) -> Option<(PathBuf, Source)> {
        if let Some(p) = game.art.manual.as_ref().filter(|p| kind == Kind::Cover && p.is_file()) {
            return Some((p.clone(), Source::Manual));
        }
        // Art scraped before a game's discs were merged is still under the first disc's key.
        let keys = std::iter::once(&game.key).chain(&game.disc_keys);
        if let Some(p) = keys.map(|k| cache_stem_for(sys, k, kind)).find_map(|stem| existing_image(&stem)) {
            return Some((p, Source::Cache));
        }
        if let (Some(root), Some(path)) = (&self.esde, &game.path) {
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            for dir in kind.esde_dirs() {
                if let Some(p) = existing_image(&root.join(&sys.esde).join(dir).join(&*stem)) {
                    return Some((p, Source::EsDe));
                }
            }
        }
        if let (Some(path), Some(dir)) = (&game.path, kind.skyscraper_dir()) {
            let file = path.file_name().unwrap_or_default().to_string_lossy();
            if let Some(p) = self.skyscraper_index(&sys.skyscraper, dir).get(&*file) {
                return Some((p.clone(), Source::Skyscraper));
            }
        }
        if kind != Kind::Cover {
            return None;
        }
        if let (Some(cache), Some(appid)) = (&self.steam, &game.art.appid) {
            let p = cache.join(appid).join("library_600x900.jpg");
            if p.is_file() {
                return Some((p, Source::Steam));
            }
        }
        None
    }

    /// Skyscraper names its images by a content hash, the same for a game's cover and
    /// screenshot; `quickid.xml` maps file paths to those hashes. The paths may come from another
    /// machine (a handheld's SD card), so match on file name.
    fn skyscraper_index(&mut self, platform: &str, kind_dir: &'static str) -> &HashMap<String, PathBuf> {
        let root = self.skyscraper.clone();
        self.sky_index.entry((platform.to_string(), kind_dir)).or_insert_with(|| {
            let mut index = HashMap::new();
            let Some(dir) = root.map(|r| r.join(platform)) else { return index };
            let Ok(xml) = std::fs::read_to_string(dir.join("quickid.xml")) else { return index };
            let Ok(doc) = roxmltree::Document::parse(&xml) else { return index };
            for node in doc.descendants().filter(|n| n.has_tag_name("quickid")) {
                let (Some(path), Some(id)) = (node.attribute("filepath"), node.attribute("id")) else { continue };
                let image = dir.join(kind_dir).join("screenscraper").join(id);
                if image.is_file() {
                    let name = Path::new(path).file_name().unwrap_or_default().to_string_lossy().into_owned();
                    index.insert(name, image);
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
fn cache_stem(sys: &System, game: &Game, kind: Kind) -> PathBuf {
    cache_stem_for(sys, &game.key, kind)
}

fn cache_stem_for(sys: &System, key: &str, kind: Kind) -> PathBuf {
    let mut dir = config::cache_dir().join("art").join(&sys.id);
    if let Some(sub) = kind.cache_dir() {
        dir.push(sub);
    }
    dir.join(key_file_name(key))
}

/// A game's key as a file name, without the system it starts with.
fn key_file_name(key: &str) -> String {
    let (_, rest) = key.split_once(':').unwrap_or(("", key));
    rest.replace('/', "__")
}

// ---- covers added from a game's menu ----

/// The longest side of a saved cover, in pixels. Covers are shown far smaller.
const CUSTOM_MAX_SIDE: u32 = 1200;

/// Where a cover added from a game's menu is kept: next to the config, since it's yours and can't
/// be downloaded again, and not in the cache, which `romi scrape` writes to.
pub fn custom_path(cfg_path: &Path, system: &str, key: &str) -> PathBuf {
    let cfg_path = std::path::absolute(cfg_path).unwrap_or_else(|_| cfg_path.to_path_buf());
    let dir = cfg_path.parent().unwrap_or(Path::new(".")).join("art").join(system);
    with_ext(&dir.join(key_file_name(key)), "png")
}

/// Saves a copy of the image at `source` as a PNG at `dest`, upright and no bigger than a cover
/// needs to be. Fails, leaving `dest` as it was, if `source` isn't an image romi can read.
pub fn save_custom(source: &Path, dest: &Path) -> Result<()> {
    use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
    // Not only for a clear message: reading a named pipe would wait forever.
    if !source.is_file() {
        anyhow::bail!("There's no file at {}", config::tilde(source));
    }
    let not_an_image = "That isn't an image romi can read";
    let reader = ImageReader::open(source)
        .with_context(|| format!("Couldn't read {}", config::tilde(source)))?
        .with_guessed_format()
        .with_context(|| format!("Couldn't read {}", config::tilde(source)))?;
    let mut decoder = reader.into_decoder().context(not_an_image)?;
    // A photo of a box from a phone is usually stored sideways, with a note saying so.
    let orientation = decoder.orientation().context(not_an_image)?;
    let mut img = DynamicImage::from_decoder(decoder).context(not_an_image)?;
    img.apply_orientation(orientation);
    if img.width().max(img.height()) > CUSTOM_MAX_SIDE {
        img = img.resize(CUSTOM_MAX_SIDE, CUSTOM_MAX_SIDE, image::imageops::FilterType::Triangle);
    }
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png).context("Couldn't convert it to PNG")?;
    crate::files::write_atomic(dest, png).with_context(|| format!("Couldn't save {}", config::tilde(dest)))
}

/// The first line of pasted text with anything around it gone. Dropping several files pastes one
/// on each line in some terminals, which also turn newlines into carriage returns.
pub fn first_line(text: &str) -> &str {
    text.split(['\n', '\r']).map(str::trim).find(|line| !line.is_empty()).unwrap_or_default()
}

/// The file a terminal means by the text it pastes when one is dropped on it, or that was typed:
/// a plain path, one escaped or quoted for a shell, or a `file://` URL.
pub fn dropped_path(text: &str) -> PathBuf {
    let text = unquote(first_line(text));
    match text.strip_prefix("file://") {
        // What comes before the path is a host, usually none.
        Some(rest) => PathBuf::from(decode(&rest[rest.find('/').unwrap_or(rest.len())..])),
        None => expand_path(&text),
    }
}

/// Undoes a shell's backslashes and quotes. Quotes only count if the text starts with one, so the
/// apostrophe in a plain `/games/it's.png` stays.
fn unquote(text: &str) -> String {
    let quoted = text.starts_with(['\'', '"']);
    let mut out = String::new();
    let mut quote = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            // In double quotes a backslash only escapes these.
            (Some('"'), '\\') => match chars.clone().next() {
                Some(next @ ('"' | '\\' | '$' | '`')) => {
                    out.push(next);
                    chars.next();
                }
                _ => out.push('\\'),
            },
            (Some(_), c) => out.push(c),
            (None, '\\') => out.extend(chars.next()),
            (None, '\'' | '"') if quoted => quote = Some(c),
            (None, c) => out.push(c),
        }
    }
    out
}

// ---- libretro thumbnail sets ----

/// The file names (without `.png`) in one folder of a libretro thumbnail set.
struct Listing {
    set: String,
    kind: Kind,
    titles: TitleIndex,
}

impl Listing {
    fn new(set: String, kind: Kind, names: Vec<String>) -> Listing {
        Listing { set, kind, titles: TitleIndex::new(names) }
    }

    fn url(&self, name: &str) -> String {
        format!("{LIBRETRO}/{}/{}/{}.png", encode(&self.set), self.kind.libretro_dir(), encode(name))
    }
}

/// The best thumbnail for `title` across sets, as (set, file name).
fn best_match<'a>(title: &str, listings: &[&'a Listing]) -> Option<(&'a Listing, &'a str)> {
    let indexes: Vec<&TitleIndex> = listings.iter().map(|l| &l.titles).collect();
    let (n, i) = best_across(title, &indexes)?;
    Some((listings[n], listings[n].titles.names[i].as_str()))
}

/// Percent-encodes a URL path segment.
pub(crate) fn encode(s: &str) -> String {
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

pub(crate) fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .user_agent(concat!("romi/", env!("CARGO_PKG_VERSION"), " (+https://github.com/TitaniteScale/romi)"))
        .build()
        .into()
}

/// GET with one retry for dropped connections. `None` means the server has no such file.
pub(crate) fn get(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Option<Vec<u8>>> {
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

/// A set's file names for one kind, from a week-old-at-most local copy of its directory listing.
fn listing(agent: &ureq::Agent, set: &str, kind: Kind) -> Result<Listing> {
    let file = match kind {
        Kind::Cover => format!("{set}.txt"),
        _ => format!("{set} - {}.txt", kind.libretro_dir()),
    };
    let cached = config::cache_dir().join("libretro").join(file);
    let fresh = std::fs::metadata(&cached)
        .and_then(|m| m.modified())
        .is_ok_and(|t| t.elapsed().unwrap_or(LISTING_MAX_AGE) < LISTING_MAX_AGE);
    if fresh {
        let text = std::fs::read_to_string(&cached)?;
        return Ok(Listing::new(set.to_string(), kind, text.lines().map(String::from).collect()));
    }
    let html = get(agent, &format!("{LIBRETRO}/{}/{}/", encode(set), kind.libretro_dir()), 64 << 20)
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
    Ok(Listing::new(set.to_string(), kind, names))
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

/// `romi scrape [system...]`: reports art already on disk and downloads what it can find:
/// covers, and title screens and screenshots unless `art.screens` is off.
pub fn scrape(cfg: &Config, lib: &Library, only: &[String]) -> Result<()> {
    let agent = agent();
    let mut finder = Finder::new(cfg);
    let mut out = std::io::stdout().lock();
    // Progress overwrites its own line on a terminal; piped output gets plain lines.
    let (clear_start, clear_end) = if out.is_terminal() { ("\r", "\x1b[K") } else { ("", "") };
    let kinds = Kind::wanted(cfg);
    let mut totals: Vec<Tally> = kinds.iter().map(|_| Tally::default()).collect();
    let mut listings: HashMap<(String, Kind), Option<Listing>> = HashMap::new();
    let store = scrape_steam_store(cfg, lib, only, &agent, &mut out)?;

    for sys in &lib.systems {
        if !only.is_empty() && !only.contains(&sys.id) {
            continue;
        }
        let mut line = format!("  {:<24} {:>4} games", sys.name, sys.games.len());
        for (&kind, total) in kinds.iter().zip(&mut totals) {
            let tally = scrape_system(cfg, lib, sys, kind, &agent, &mut finder, &mut listings, &store, &mut out)?;
            line.push_str(&format!("   {:>4} {}", tally.local + tally.downloaded, kind.plural()));
            if tally.downloaded > 0 {
                line.push_str(&format!(" (+{})", tally.downloaded));
            }
            total.local += tally.local;
            total.downloaded += tally.downloaded;
            total.failed += tally.failed;
            total.missing.extend(tally.missing.into_iter().map(|n| format!("{}: {n}", sys.name)));
        }
        writeln!(out, "{clear_start}{line}{clear_end}")?;
    }

    // Covers are what's missed most; title screens and screenshots just show when they're there.
    let covers = &totals[0];
    writeln!(
        out,
        "\n{} on disk, {} downloaded, {} without art",
        covers.local,
        covers.downloaded,
        covers.missing.len()
    )?;
    for (kind, t) in kinds.iter().zip(&totals).skip(1) {
        writeln!(
            out,
            "{} {}: {} on disk, {} downloaded",
            t.local + t.downloaded,
            kind.plural(),
            t.local,
            t.downloaded
        )?;
    }
    if !covers.missing.is_empty() {
        writeln!(out, "\nno art found for:")?;
        for m in &covers.missing {
            writeln!(out, "  {m}")?;
        }
        writeln!(out, "\nset \"art\": \"/path/to/image.png\" on a game or override to fill these in by hand.")?;
    }
    let failed: usize = totals.iter().map(|t| t.failed).sum();
    if failed > 0 {
        writeln!(out, "{failed} downloads failed; run again to retry.")?;
    }
    Ok(())
}

/// Asks Steam's store about the Steam games being scraped, for their game info and covers, and
/// keeps what it says for the library to read. Without a connection, or if the store won't
/// answer, what it said last time stays.
fn scrape_steam_store(
    cfg: &Config,
    lib: &Library,
    only: &[String],
    agent: &ureq::Agent,
    out: &mut impl Write,
) -> Result<HashMap<String, crate::steam::StoreInfo>> {
    let mut cache = crate::steam::store_cache();
    let appids: Vec<String> = (0..lib.games.len())
        .filter(|&g| only.is_empty() || only.contains(&lib.system_of(g).id))
        .filter_map(|g| lib.games[g].art.appid.clone())
        .collect();
    if !cfg.art.online || appids.is_empty() {
        return Ok(cache);
    }
    match crate::steam::fetch_store_info(agent, &appids) {
        Ok(found) => {
            writeln!(out, "  Steam store: game info for {} of {} games", found.len(), appids.len())?;
            cache.extend(found);
            if let Err(e) = crate::steam::save_store_cache(&cache) {
                writeln!(out, "  ! can't save Steam's game info: {e:#}")?;
            }
        }
        Err(e) => writeln!(out, "  ! Steam store: {e:#}")?,
    }
    Ok(cache)
}

/// Finds or downloads one kind of picture for a system's games.
#[allow(clippy::too_many_arguments)]
fn scrape_system(
    cfg: &Config,
    lib: &Library,
    sys: &System,
    kind: Kind,
    agent: &ureq::Agent,
    finder: &mut Finder,
    listings: &mut HashMap<(String, Kind), Option<Listing>>,
    store: &HashMap<String, crate::steam::StoreInfo>,
    out: &mut impl Write,
) -> Result<Tally> {
    let mut tally = Tally::default();
    let mut jobs = Vec::new();
    let mut unmatched = Vec::new();

    for &g in &sys.games {
        let game = &lib.games[g];
        if finder.find_kind(sys, game, kind).is_some() {
            tally.local += 1;
        } else if !cfg.art.online {
            unmatched.push(g);
        } else if let (Kind::Cover, Some(appid)) = (kind, &game.art.appid) {
            // The store knows covers kept under a hashed folder, which the usual address misses.
            let cover = store.get(appid).and_then(|s| s.cover.clone());
            let url = cover.unwrap_or_else(|| format!("{STEAM_CDN}/{appid}/library_600x900.jpg"));
            jobs.push((g, Job { url, dest_stem: cache_stem(sys, game, kind) }));
        } else {
            unmatched.push(g);
        }
    }

    if cfg.art.online && !unmatched.is_empty() && !sys.libretro.is_empty() {
        for set in &sys.libretro {
            if let std::collections::hash_map::Entry::Vacant(slot) = listings.entry((set.clone(), kind)) {
                let l = match listing(agent, set, kind) {
                    Ok(l) => Some(l),
                    Err(e) => {
                        writeln!(out, "  ! {e:#}")?;
                        None
                    }
                };
                slot.insert(l);
            }
        }
        let sets: Vec<&Listing> = sys.libretro.iter().filter_map(|s| listings[&(s.clone(), kind)].as_ref()).collect();
        unmatched.retain(|&g| {
            let game = &lib.games[g];
            let Some(title) = &game.art.title else { return true };
            match best_match(title, &sets) {
                Some((l, name)) => {
                    jobs.push((g, Job { url: l.url(name), dest_stem: cache_stem(sys, game, kind) }));
                    false
                }
                None => true,
            }
        });
    }

    let (not_found, failed) = run_jobs(agent, &jobs, &format!("{} {}", sys.name, kind.plural()));
    tally.downloaded = jobs.len() - not_found.len() - failed.len();
    tally.failed = failed.len();
    tally.missing = unmatched.iter().chain(&not_found).chain(&failed).map(|&g| lib.games[g].name.clone()).collect();
    tally.missing.sort();
    Ok(tally)
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

    #[test]
    fn url_encoding_round_trips() {
        let s = "Pokémon - Red (USA, Europe) & more";
        assert_eq!(decode(&encode(s)), s);
        assert_eq!(encode("A B(C)"), "A%20B%28C%29");
    }

    #[test]
    fn dropped_paths_in_every_shape() {
        let plain = PathBuf::from("/a/b c.png");
        for text in [
            "/a/b c.png",
            "/a/b\\ c.png",
            "'/a/b c.png'",
            "\"/a/b c.png\"",
            "file:///a/b%20c.png",
            "file://localhost/a/b%20c.png",
            "'file:///a/b%20c.png'",
            "  /a/b\\ c.png \n",
            "/a/b\\ c.png\r",
            "\r\n/a/b\\ c.png\r\n/other.png\r\n",
            "/a/b\\ c.png\r/other.png",
        ] {
            assert_eq!(dropped_path(text), plain, "{text:?}");
        }
        assert_eq!(dropped_path("~/b.png"), config::home().join("b.png"));
        assert_eq!(dropped_path("'~/b c.png'"), config::home().join("b c.png"));
        // Apostrophes: plain, and the way a shell quotes them.
        assert_eq!(dropped_path("/a/it's.png"), PathBuf::from("/a/it's.png"));
        assert_eq!(dropped_path("/a/it\\'s.png"), PathBuf::from("/a/it's.png"));
        assert_eq!(dropped_path("'/a/it'\\''s.png'"), PathBuf::from("/a/it's.png"));
        assert_eq!(dropped_path("\"/a/say \\\"hi\\\" \\x.png\""), PathBuf::from("/a/say \"hi\" \\x.png"));
        assert_eq!(dropped_path("/a/\\(USA\\)\\&\\\\.png"), PathBuf::from("/a/(USA)&\\.png"));
        assert_eq!(dropped_path("file:///a/caf%C3%A9.png"), PathBuf::from("/a/café.png"));
    }

    #[test]
    fn custom_covers_are_named_after_the_key() {
        let cfg = Path::new("/home/me/.config/romi/config.json");
        let dest = "/home/me/.config/romi/art/gba/Advance Wars (USA).gba.png";
        assert_eq!(custom_path(cfg, "gba", "gba:Advance Wars (USA).gba"), PathBuf::from(dest));
        let dest = "/home/me/.config/romi/art/psx/Sub__Game.chd.png";
        assert_eq!(custom_path(cfg, "psx", "psx:Sub/Game.chd"), PathBuf::from(dest));
        // Games without a ROM: hand-added and Steam's.
        let dest = "/home/me/.config/romi/art/pc/Sonic 3 A.I.R..png";
        assert_eq!(custom_path(cfg, "pc", "game:Sonic 3 A.I.R."), PathBuf::from(dest));
        let dest = "/home/me/.config/romi/art/steam/736260.png";
        assert_eq!(custom_path(cfg, "steam", "steam:736260"), PathBuf::from(dest));
        // A config given by a relative path still gives a path that works from anywhere.
        assert!(custom_path(Path::new("config.json"), "gba", "gba:a.gba").is_absolute());
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("romi-art-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn saved(dest: &Path) -> image::DynamicImage {
        assert!(std::fs::read(dest).unwrap().starts_with(b"\x89PNG"), "not a PNG");
        image::open(dest).unwrap()
    }

    #[test]
    fn saving_converts_to_png_and_shrinks_only_big_images() {
        let dir = temp_dir("save");
        let dest = dir.join("art/gba/game.png");

        // A JPEG, misnamed for good measure; the folder gets made.
        let jpeg = dir.join("cover.png");
        image::RgbImage::new(300, 400).save_with_format(&jpeg, image::ImageFormat::Jpeg).unwrap();
        save_custom(&jpeg, &dest).unwrap();
        assert_eq!((saved(&dest).width(), saved(&dest).height()), (300, 400));
        assert!(jpeg.exists(), "the original stays");

        // Too big, with its shape kept. Saving again replaces the last one.
        let big = dir.join("big.png");
        image::RgbImage::new(1000, 2400).save(&big).unwrap();
        save_custom(&big, &dest).unwrap();
        assert_eq!((saved(&dest).width(), saved(&dest).height()), (500, 1200));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_turns_photos_upright() {
        let dir = temp_dir("exif");
        let mut jpeg = Vec::new();
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::new(40, 20));
        img.write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Jpeg).unwrap();
        // An Exif segment right after the start of the file, saying "rotate 90 degrees".
        let tiff: &[u8] = &[
            b'M', b'M', 0, 42, 0, 0, 0, 8, // big-endian, first directory at 8
            0, 1, // one entry
            1, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, // Orientation (a short): 6
            0, 0, 0, 0, // no more directories
        ];
        let mut segment = vec![0xFF, 0xE1];
        segment.extend(((2 + 6 + tiff.len()) as u16).to_be_bytes());
        segment.extend(b"Exif\0\0");
        segment.extend(tiff);
        jpeg.splice(2..2, segment);
        let source = dir.join("photo.jpg");
        std::fs::write(&source, jpeg).unwrap();

        let dest = dir.join("game.png");
        save_custom(&source, &dest).unwrap();
        assert_eq!((saved(&dest).width(), saved(&dest).height()), (20, 40));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_something_else_leaves_the_old_cover() {
        let dir = temp_dir("bad");
        let dest = dir.join("game.png");
        std::fs::write(&dest, b"the old cover").unwrap();

        let text = dir.join("notes.txt");
        std::fs::write(&text, "not a picture").unwrap();
        let bad = [
            (text, "That isn't an image romi can read"),
            (dir.join("missing.png"), "There's no file at"),
            (dir.clone(), "There's no file at"),
        ];
        for (source, message) in bad {
            let err = format!("{:#}", save_custom(&source, &dest).unwrap_err());
            assert!(err.starts_with(message), "{err}");
        }
        assert_eq!(std::fs::read(&dest).unwrap(), b"the old cover");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

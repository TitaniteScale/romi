//! Game metadata (description, genres, year, players...) merged from several sources, best first:
//! the config, ES-DE's scrapes (read in library.rs), Skyscraper's cache, and RetroArch's game
//! databases. RetroArch's `.rdb` files are matched by ROM checksum where possible, which also gives
//! the exact No-Intro/Redump name that box art lookups need.

use crate::config::{self, Config, expand_path};
use crate::library::Library;
use crate::titles::TitleIndex;
use std::collections::{BTreeSet, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Raw ROMs bigger than this aren't checksummed (it'd mean reading GBs at startup for disc images).
const MAX_HASH_SIZE: u64 = 64 << 20;
/// Formats whose checksum wouldn't match a database entry (discs, compressed images).
const UNHASHABLE: &[&str] = &[
    "chd", "cue", "bin", "iso", "img", "ccd", "mds", "cdi", "gdi", "m3u", "pbp", "rvz", "gcm", "gcz", "ciso", "wbfs",
    "wia", "cso", "zso", "gz", "7z", "sh", "vpx",
];

#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub description: Option<String>,
    pub developer: Option<String>,
    pub publisher: Option<String>,
    pub genres: Vec<String>,
    pub year: Option<u16>,
    pub players: Option<u8>,
    pub franchise: Option<String>,
}

impl Meta {
    /// Fills in whatever is still missing from `other`; what's already here wins.
    pub fn fill(&mut self, other: &Meta) {
        let pick = |a: &mut Option<String>, b: &Option<String>| {
            if a.is_none() {
                a.clone_from(b);
            }
        };
        pick(&mut self.description, &other.description);
        pick(&mut self.developer, &other.developer);
        pick(&mut self.publisher, &other.publisher);
        pick(&mut self.franchise, &other.franchise);
        if self.genres.is_empty() {
            self.genres.clone_from(&other.genres);
        }
        self.year = self.year.or(other.year);
        self.players = self.players.or(other.players);
    }
}

// ---- normalizing ----

/// Folds the many ways databases spell genres ("Shoot'em Up", "Race, Driving", "Fighting.") into
/// a short list. Words that only qualify a genre ("Horizontal") are dropped.
pub fn genres(raw: &str) -> Vec<String> {
    // MAME's category lists file brawlers under "Fighter" too; the second half tells them apart.
    let lower = raw.to_lowercase();
    let lower = lower.trim_start();
    if lower.starts_with("fighter / versus") || lower.starts_with("fighter / 3d") {
        return vec!["Fighting".into()];
    }
    if lower.starts_with("fighter /") {
        return vec!["Beat 'em up".into()];
    }
    if lower.starts_with("shooter / flying") {
        return vec!["Shoot 'em up".into()];
    }
    const MAP: &[(&[&str], &str)] = &[
        (&["shoot'em", "shoot 'em", "shootem", "shmup", "shoot-em"], "Shoot 'em up"),
        (&["beat'em", "beat 'em", "beatem", "brawler"], "Beat 'em up"),
        (&["fight", "wrestl", "boxing"], "Fighting"),
        (&["role", "rpg"], "RPG"),
        (&["platform", "plataform"], "Platformer"),
        (&["race", "racing", "driving", "kart"], "Racing"),
        (&["gun", "shooter", "shooting", "fps"], "Shooter"),
        (&["puzzle", "breakout", "tetris"], "Puzzle"),
        (&["board", "chess", "shogi", "mahjong", "card", "hanafuda"], "Board & cards"),
        (&["casino", "gambling", "pachinko", "poker", "slot"], "Casino"),
        (&["pinball"], "Pinball"),
        (&["quiz", "trivia"], "Quiz"),
        (&["music", "danc", "rhythm", "sing", "karaoke"], "Music"),
        (&["horror"], "Horror"),
        (&["strateg", "tactic"], "Strategy"),
        (&["simulat"], "Simulation"),
        (&["mini game", "minigame", "party"], "Party"),
        (&["educat"], "Educational"),
        (&["compilation"], "Compilation"),
        (
            &[
                "sport",
                "basketball",
                "football",
                "soccer",
                "tennis",
                "golf",
                "baseball",
                "bowling",
                "billiard",
                "skate",
                "snowboard",
                "ski",
                "hockey",
                "horse",
                "volleyball",
                "fishing",
                "hunting",
            ],
            "Sports",
        ),
        (&["adventure"], "Adventure"),
        (&["action"], "Action"),
        (&["maze"], "Maze"),
    ];
    let segments: Vec<String> = raw
        .split(['/', ',', ';', '|'])
        .map(|s| s.trim().trim_end_matches('.').trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    let mut out: Vec<String> = Vec::new();
    for seg in &segments {
        if let Some((_, g)) = MAP.iter().find(|(needles, _)| needles.iter().any(|n| seg.contains(n)))
            && !out.iter().any(|o| o == g)
        {
            out.push(g.to_string());
        }
    }
    if out.is_empty()
        && let Some(first) = segments.first()
    {
        let mut c = first.chars();
        out.push(c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default());
    }
    out
}

/// The first plausible year in a date string ("19910101T000000", "2001-09-10", "1998").
pub fn year(raw: &str) -> Option<u16> {
    let digits: Vec<char> = raw.chars().collect();
    digits.windows(4).find_map(|w| {
        let s: String = w.iter().collect();
        s.parse::<u16>().ok().filter(|y| (1950..=2100).contains(y))
    })
}

/// The most players a "1-4" / "2" / "1-2 VS" style string allows.
pub fn players(raw: &str) -> Option<u8> {
    raw.split(|c: char| !c.is_ascii_digit()).filter_map(|n| n.parse::<u8>().ok()).filter(|&n| n > 0).max()
}

// ---- RetroArch databases ----

/// Just enough MessagePack for `.rdb` files: flat maps of strings, binaries and integers.
#[derive(Debug)]
enum Value {
    Str(String),
    Bin(Vec<u8>),
    Int(u64),
    Nil,
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.i..self.i + n)?;
        self.i += n;
        Some(s)
    }

    fn uint(&mut self, n: usize) -> Option<u64> {
        Some(self.take(n)?.iter().fold(0u64, |acc, &x| acc << 8 | x as u64))
    }

    fn value(&mut self) -> Option<Value> {
        let t = *self.take(1)?.first()?;
        Some(match t {
            0x00..=0x7f => Value::Int(t as u64),
            0xa0..=0xbf => self.string((t & 0x1f) as usize)?,
            0xc0 => Value::Nil,
            0xc4..=0xc6 => {
                let n = self.uint(1 << (t - 0xc4))? as usize;
                Value::Bin(self.take(n)?.to_vec())
            }
            0xcc..=0xcf => Value::Int(self.uint(1 << (t - 0xcc))?),
            0xd9..=0xdb => {
                let n = self.uint(1 << (t - 0xd9))? as usize;
                self.string(n)?
            }
            _ => return None,
        })
    }

    fn string(&mut self, n: usize) -> Option<Value> {
        Some(Value::Str(String::from_utf8_lossy(self.take(n)?).into_owned()))
    }

    /// The next map, or `None` at the end of the records.
    fn map(&mut self) -> Option<HashMap<String, Value>> {
        let t = *self.b.get(self.i)?;
        let n = match t {
            0x80..=0x8f => {
                self.i += 1;
                (t & 0x0f) as usize
            }
            0xde | 0xdf => {
                self.i += 1;
                self.uint(if t == 0xde { 2 } else { 4 })? as usize
            }
            _ => return None,
        };
        let mut m = HashMap::with_capacity(n);
        for _ in 0..n {
            let Value::Str(k) = self.value()? else { return None };
            m.insert(k, self.value()?);
        }
        Some(m)
    }
}

struct RdbEntry {
    name: String,
    meta: Meta,
}

/// One RetroArch database, deduplicated by game name (discs list each track separately).
pub struct Rdb {
    entries: Vec<RdbEntry>,
    /// Built on first use: most games match by checksum or file name, and splitting every title
    /// (52k for MAME) up front would dominate startup.
    titles: std::cell::OnceCell<TitleIndex>,
    by_crc: HashMap<u32, usize>,
    by_rom: HashMap<String, usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    /// Same checksum or same file name: the entry is certainly this game.
    Exact,
    /// Title match only.
    Title,
}

impl Rdb {
    pub fn load(path: &Path) -> Option<Rdb> {
        let bytes = std::fs::read(path).ok()?;
        if !bytes.starts_with(b"RARCHDB\0") {
            return None;
        }
        let mut r = Reader { b: &bytes, i: 16 };
        let mut entries: Vec<RdbEntry> = Vec::new();
        let mut by_name: HashMap<String, usize> = HashMap::new();
        let mut by_crc = HashMap::new();
        let mut by_rom = HashMap::new();
        while let Some(m) = r.map() {
            let s = |k: &str| match m.get(k) {
                Some(Value::Str(v)) if !v.trim().is_empty() => Some(v.trim().to_string()),
                _ => None,
            };
            let n = |k: &str| match m.get(k) {
                Some(Value::Int(v)) => Some(*v),
                _ => None,
            };
            let Some(name) = s("name") else { continue };
            // Many entries' "description" is just the name again; only keep real summaries.
            let description = s("description").filter(|d| d.len() > 60 && *d != name);
            let meta = Meta {
                description,
                developer: s("developer"),
                publisher: s("publisher"),
                genres: s("genre").map(|g| genres(&g)).unwrap_or_default(),
                year: n("releaseyear").and_then(|y| u16::try_from(y).ok()).filter(|y| *y >= 1950),
                players: n("users").and_then(|u| u8::try_from(u).ok()).filter(|u| *u > 0),
                franchise: s("franchise"),
            };
            let idx = *by_name.entry(name.clone()).or_insert_with(|| {
                entries.push(RdbEntry { name: name.clone(), meta: Meta::default() });
                entries.len() - 1
            });
            entries[idx].meta.fill(&meta);
            if let Some(Value::Bin(crc)) = m.get("crc")
                && let Ok(b) = <[u8; 4]>::try_from(crc.as_slice())
            {
                by_crc.entry(u32::from_be_bytes(b)).or_insert(idx);
            }
            if let Some(rom) = s("rom_name") {
                by_rom.entry(file_stem(&rom).to_lowercase()).or_insert(idx);
            }
        }
        Some(Rdb { entries, titles: std::cell::OnceCell::new(), by_crc, by_rom })
    }

    /// Finds a game by checksum, then file name, then title.
    pub fn lookup(&self, crc: Option<u32>, file_stem: &str, title: &str) -> Option<(&str, &Meta, Precision)> {
        let exact = crc.and_then(|c| self.by_crc.get(&c)).or_else(|| self.by_rom.get(&file_stem.to_lowercase()));
        let (idx, precision) = match exact {
            Some(&i) => (i, Precision::Exact),
            None => {
                let titles =
                    self.titles.get_or_init(|| TitleIndex::new(self.entries.iter().map(|e| e.name.clone()).collect()));
                (titles.find(title)?, Precision::Title)
            }
        };
        let e = &self.entries[idx];
        Some((&e.name, &e.meta, precision))
    }
}

fn file_stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

// ---- Skyscraper ----

/// Skyscraper's cached metadata for one platform, by ROM file name.
fn skyscraper(dir: &Path) -> HashMap<String, Meta> {
    let mut out = HashMap::new();
    let ids = skyscraper_ids(dir);
    let Ok(xml) = std::fs::read_to_string(dir.join("db.xml")) else { return out };
    let Ok(doc) = roxmltree::Document::parse(&xml) else { return out };
    let mut by_id: HashMap<&str, Meta> = HashMap::new();
    for r in doc.descendants().filter(|n| n.has_tag_name("resource")) {
        let (Some(id), Some(kind), Some(text)) = (r.attribute("id"), r.attribute("type"), r.text()) else { continue };
        let text = text.trim();
        let m = by_id.entry(id).or_default();
        match kind {
            "description" if m.description.is_none() => m.description = Some(text.to_string()),
            "developer" if m.developer.is_none() => m.developer = Some(text.to_string()),
            "publisher" if m.publisher.is_none() => m.publisher = Some(text.to_string()),
            "tags" if m.genres.is_empty() => m.genres = genres(text),
            "releasedate" if m.year.is_none() => m.year = year(text),
            "players" if m.players.is_none() => m.players = players(text),
            _ => {}
        }
    }
    for (file, id) in ids {
        if let Some(m) = by_id.get(id.as_str()) {
            out.insert(file, m.clone());
        }
    }
    out
}

/// Skyscraper's `quickid.xml`: ROM file name -> the id its cache entries use. Paths may come from
/// another machine (a handheld's SD card), so only the file name counts.
pub fn skyscraper_ids(dir: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(xml) = std::fs::read_to_string(dir.join("quickid.xml")) else { return out };
    let Ok(doc) = roxmltree::Document::parse(&xml) else { return out };
    for node in doc.descendants().filter(|n| n.has_tag_name("quickid")) {
        if let (Some(path), Some(id)) = (node.attribute("filepath"), node.attribute("id")) {
            let name = Path::new(path).file_name().unwrap_or_default().to_string_lossy().into_owned();
            out.insert(name, id.to_string());
        }
    }
    out
}

// ---- MAME categories ----

/// A catver.ini-style file: `shortname=Category / Subcategory` under section headers.
fn arcade_categories(path: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.replace("* Mature *", "").trim().to_string()))
        .filter(|(k, v)| !k.is_empty() && !k.starts_with('[') && !k.starts_with(';') && !v.is_empty())
        .collect()
}

// ---- checksums ----

/// ROM checksums, remembered by path, size and modification time so each file is read once.
struct Crcs {
    path: PathBuf,
    known: HashMap<String, (u64, u64, u32)>,
    dirty: bool,
}

impl Crcs {
    fn load() -> Crcs {
        let path = config::cache_dir().join("crc.tsv");
        let known = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut f = l.rsplitn(4, '\t');
                let crc = u32::from_str_radix(f.next()?, 16).ok()?;
                let mtime = f.next()?.parse().ok()?;
                let size = f.next()?.parse().ok()?;
                Some((f.next()?.to_string(), (size, mtime, crc)))
            })
            .collect();
        Crcs { path, known, dirty: false }
    }

    fn save(&self) {
        if !self.dirty {
            return;
        }
        let mut lines: Vec<String> =
            self.known.iter().map(|(p, (size, mtime, crc))| format!("{p}\t{size}\t{mtime}\t{crc:08x}")).collect();
        lines.sort();
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = self.path.with_extension("tsv.tmp");
        if std::fs::write(&tmp, lines.join("\n")).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }

    fn get(&mut self, path: &Path) -> Option<u32> {
        let ext = path.extension()?.to_string_lossy().to_lowercase();
        if UNHASHABLE.contains(&ext.as_str()) {
            return None;
        }
        let md = std::fs::metadata(path).ok()?;
        let mtime = md.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
        let key = path.to_string_lossy().into_owned();
        if let Some(&(size, t, crc)) = self.known.get(&key)
            && size == md.len()
            && t == mtime
        {
            return Some(crc);
        }
        let crc = if ext == "zip" {
            zip_crc(path)?
        } else if md.len() <= MAX_HASH_SIZE {
            crc32fast::hash(&std::fs::read(path).ok()?)
        } else {
            return None;
        };
        self.known.insert(key, (md.len(), mtime, crc));
        self.dirty = true;
        Some(crc)
    }
}

/// The CRC of the biggest file in a zip, straight from its central directory (no decompressing).
fn zip_crc(path: &Path) -> Option<u32> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let tail_len = len.min(66_000);
    f.seek(SeekFrom::Start(len - tail_len)).ok()?;
    let mut tail = vec![0; tail_len as usize];
    f.read_exact(&mut tail).ok()?;
    let eocd = tail.windows(4).rposition(|w| w == b"PK\x05\x06")?;
    let le32 = |b: &[u8], at: usize| Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?));
    let le16 = |b: &[u8], at: usize| Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?) as usize);
    let (cd_size, cd_offset) = (le32(&tail, eocd + 12)?, le32(&tail, eocd + 16)?);
    if cd_offset == u32::MAX || cd_size as u64 > len {
        return None; // zip64; not worth handling for ROMs
    }
    f.seek(SeekFrom::Start(cd_offset as u64)).ok()?;
    let mut cd = vec![0; cd_size as usize];
    f.read_exact(&mut cd).ok()?;
    let (mut at, mut best) = (0, None::<(u32, u32)>);
    while cd.get(at..at + 4) == Some(b"PK\x01\x02") {
        let (crc, size) = (le32(&cd, at + 16)?, le32(&cd, at + 24)?);
        if best.is_none_or(|(_, s)| size > s) {
            best = Some((crc, size));
        }
        at += 46 + le16(&cd, at + 28)? + le16(&cd, at + 30)? + le16(&cd, at + 32)?;
    }
    best.map(|(crc, _)| crc)
}

// ---- putting it together ----

fn rdb_dir(cfg: &Config) -> Option<PathBuf> {
    let configured = cfg.metadata.rdb_dir.as_deref().map(expand_path);
    configured.into_iter().chain([PathBuf::from("/usr/share/libretro/database/rdb")]).find(|d| d.is_dir())
}

/// What the metadata sources had for one game.
pub struct Found {
    pub game: usize,
    pub meta: Meta,
    /// The exact database name, when the ROM was identified by checksum or file name. Thumbnail
    /// sets are named after it, so box art lookups use it.
    pub art_title: Option<String>,
}

/// Looks up Skyscraper and RetroArch database metadata for every ROM. Read-only, so it can run
/// on a background thread while the UI is already up; `apply` merges the results.
pub fn lookup_all(cfg: &Config, lib: &Library) -> Vec<Found> {
    let rdb_dir = rdb_dir(cfg);
    let sky_root = cfg.art.skyscraper_cache.as_deref().map(expand_path);
    let mut crcs = Crcs::load();
    let mut rdbs: HashMap<String, Option<Rdb>> = HashMap::new();
    let mut found = Vec::new();
    let categories = match &cfg.metadata.arcade_categories {
        Some(p) if lib.systems.iter().any(|s| s.arcade) => arcade_categories(&expand_path(p)),
        _ => HashMap::new(),
    };

    for sys in &lib.systems {
        let sky = sky_root.as_ref().map(|r| skyscraper(&r.join(&sys.skyscraper))).unwrap_or_default();
        if let Some(dir) = &rdb_dir {
            for set in &sys.libretro {
                rdbs.entry(set.clone()).or_insert_with(|| Rdb::load(&dir.join(format!("{set}.rdb"))));
            }
        }
        for &g in &sys.games {
            let game = &lib.games[g];
            let Some(path) = &game.path else { continue };
            let file = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let stem = file_stem(&file);
            let mut meta = sky.get(&file).cloned().unwrap_or_default();
            if sys.arcade
                && meta.genres.is_empty()
                && let Some(cat) = categories.get(stem)
            {
                meta.genres = genres(cat);
            }
            let crc = if cfg.metadata.checksums && !sys.arcade { crcs.get(path) } else { None };
            let title = game.art.title.as_deref().unwrap_or(stem);
            let hit = sys
                .libretro
                .iter()
                .filter_map(|set| rdbs.get(set)?.as_ref())
                .find_map(|rdb| rdb.lookup(crc, stem, title));
            let mut art_title = None;
            if let Some((name, m, precision)) = hit {
                meta.fill(m);
                // Arcade sets already use their MAME title.
                if precision == Precision::Exact && !sys.arcade {
                    art_title = Some(name.to_string());
                }
            }
            found.push(Found { game: g, meta, art_title });
        }
    }
    crcs.save();
    found
}

pub fn apply(lib: &mut Library, found: Vec<Found>) {
    for f in found {
        let Some(game) = lib.games.get_mut(f.game) else { continue };
        game.meta.fill(&f.meta);
        if f.art_title.is_some() {
            game.art.title = f.art_title;
        }
    }
}

/// All genres in the library with how many games have each, most common first.
pub fn genre_counts(lib: &Library) -> Vec<(String, usize)> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for g in &lib.games {
        for genre in g.meta.genres.iter().collect::<BTreeSet<_>>() {
            *counts.entry(genre).or_default() += 1;
        }
    }
    let mut out: Vec<(String, usize)> = counts.into_iter().map(|(g, n)| (g.to_string(), n)).collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genre_normalizing() {
        assert_eq!(genres("Shoot'em Up / Horizontal"), ["Shoot 'em up"]);
        assert_eq!(genres("Race, Driving"), ["Racing"]);
        assert_eq!(genres("Fighting."), ["Fighting"]);
        assert_eq!(genres("Role playing games"), ["RPG"]);
        assert_eq!(genres("Fighting / Mini Games"), ["Fighting", "Party"]);
        assert_eq!(genres("Lightgun Shooter"), ["Shooter"]);
        assert_eq!(genres("Interactive Comic"), ["Interactive comic"]);
        assert_eq!(genres("Fighter / Versus"), ["Fighting"]);
        assert_eq!(genres("Fighter / 2.5D"), ["Beat 'em up"]);
        assert_eq!(genres("Shooter / Flying Vertical"), ["Shoot 'em up"]);
        assert_eq!(genres("Platform / Shooter Scrolling"), ["Platformer", "Shooter"]);
        assert!(genres("").is_empty());
    }

    #[test]
    fn years_and_players() {
        assert_eq!(year("19910101T000000"), Some(1991));
        assert_eq!(year("2001-09-10"), Some(2001));
        assert_eq!(year("unknown"), None);
        assert_eq!(players("1-4"), Some(4));
        assert_eq!(players("2"), Some(2));
        assert_eq!(players("1-2 VS"), Some(2));
        assert_eq!(players(""), None);
    }

    #[test]
    fn msgpack_records() {
        // Two records, then nil: {"name": "A (USA)", "users": 2, "crc": bin(01020304)}, {"name": "B"}
        let mut b = b"RARCHDB\0\0\0\0\0\0\0\0\0".to_vec();
        b.extend([0x83, 0xa4]);
        b.extend(b"name");
        b.extend([0xa7]);
        b.extend(b"A (USA)");
        b.extend([0xa5]);
        b.extend(b"users");
        b.extend([0x02, 0xa3]);
        b.extend(b"crc");
        b.extend([0xc4, 0x04, 1, 2, 3, 4, 0x81, 0xa4]);
        b.extend(b"name");
        b.extend([0xa1, b'B', 0xc0]);
        let path = std::env::temp_dir().join(format!("romi-test-{}.rdb", std::process::id()));
        std::fs::write(&path, &b).unwrap();
        let rdb = Rdb::load(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let (name, meta, p) = rdb.lookup(Some(0x01020304), "whatever", "").unwrap();
        assert_eq!((name, meta.players, p), ("A (USA)", Some(2), Precision::Exact));
        assert_eq!(rdb.lookup(None, "x", "B").unwrap().2, Precision::Title);
    }
}

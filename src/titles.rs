//! Matching our game titles against database names (libretro thumbnails, RetroArch databases).
//! Those use No-Intro/Redump/MAME naming, which file names on disk only loosely follow.

use std::collections::HashMap;

/// libretro replaces these characters with `_` in thumbnail file names.
pub fn libretro_safe(title: &str) -> String {
    title.chars().map(|c| if "&*/:`<>?\\|\"".contains(c) { '_' } else { c }).collect()
}

pub fn strip_tags(s: &str) -> String {
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
pub fn words(title: &str) -> Vec<String> {
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

/// A list of names, pre-split for matching.
pub struct TitleIndex {
    pub names: Vec<String>,
    exact: HashMap<String, usize>,
    words: Vec<Vec<String>>,
}

impl TitleIndex {
    pub fn new(names: Vec<String>) -> TitleIndex {
        let words = names.iter().map(|n| words(n)).collect();
        let mut exact = HashMap::new();
        for (i, n) in names.iter().enumerate() {
            exact.entry(n.clone()).or_insert(i);
        }
        TitleIndex { names, exact, words }
    }

    pub fn find(&self, title: &str) -> Option<usize> {
        best_across(title, &[self]).map(|(_, i)| i)
    }
}

/// Finds the best name for `title` across indexes, trying stricter matches first: the exact name
/// (as-is or with libretro's character substitutions), the same words, the same words plus a
/// subtitle, then ignoring dashes. Returns (which index, which name).
pub fn best_across(title: &str, indexes: &[&TitleIndex]) -> Option<(usize, usize)> {
    let safe = libretro_safe(title);
    for (n, idx) in indexes.iter().enumerate() {
        if let Some(&i) = idx.exact.get(title).or_else(|| idx.exact.get(&safe)) {
            return Some((n, i));
        }
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
        let best = indexes
            .iter()
            .enumerate()
            .flat_map(|(n, idx)| idx.words.iter().enumerate().filter(|(_, w)| tier(w)).map(move |(i, _)| (n, i)))
            .min_by_key(|&(n, i)| rank(&indexes[n].names[i]));
        if best.is_some() {
            return best;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matched(title: &str, names: &[&str]) -> Option<String> {
        let idx = TitleIndex::new(names.iter().map(|s| s.to_string()).collect());
        idx.find(title).map(|i| idx.names[i].clone())
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
        // Databases keep the real characters.
        assert_eq!(matched("Rockman Battle & Fighters", &["Rockman - Battle & Fighters (Japan)"]).is_some(), true);
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
    fn earlier_index_wins_exact() {
        let a = TitleIndex::new(vec!["X (USA)".into()]);
        let b = TitleIndex::new(vec!["X (USA)".into()]);
        assert_eq!(best_across("X (USA)", &[&a, &b]), Some((0, 0)));
    }
}

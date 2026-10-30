//! Notices over a game played in romi: an achievement unlocking (with its badge), the game's
//! achievements loading, every one of them unlocked.
//!
//! A badge is fetched when its notice is queued, from romi's cache if it's been shown before, and
//! the notice waits for it for up to `BADGE_WAIT`, then goes up without it. The badge is drawn the
//! way the game is, as a Kitty image behind Unicode placeholder cells (see `embedded`), so the
//! layout here works in cells and leaves the badge's cells to them.

use crate::emu::cheevos::Event;
use ratatui::layout::Rect;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a notice waits for its badge before going up without it.
const BADGE_WAIT: Duration = Duration::from_millis(500);
/// How many rows tall a badge is: as tall as a notice's heading, title and first line of detail.
const BADGE_ROWS: u16 = 3;
/// Lines a title or detail wraps onto, at most.
const MAX_WRAP: usize = 3;
/// The narrowest text worth showing beside a badge; narrower, and the badge is left out.
const MIN_TEXT_BESIDE_BADGE: u16 = 12;

/// How a line of a notice is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Weight {
    /// In the accent colour, bold.
    Heading,
    Bold,
    Plain,
}

/// Where a notice goes over the game's picture, all in cells.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// The whole notice, border and all.
    pub area: Rect,
    /// The badge's cells, if it's shown.
    pub badge: Option<Rect>,
    /// Where the lines go, and whether they're centered there (without a badge) or start at its
    /// left (beside one).
    pub text: Rect,
    pub centered: bool,
    pub lines: Vec<(String, Weight)>,
}

enum BadgeState {
    Loading,
    Loaded(image::RgbaImage),
    Failed,
}

pub struct Toast {
    heading: String,
    /// The lines under the heading, the first bold.
    body: Vec<String>,
    badge: Option<Arc<Mutex<BadgeState>>>,
    lasts: Duration,
    queued: Instant,
    /// When it went up, once it has.
    shown: Option<Instant>,
    /// The cells its badge was last sent for, so it's sent once.
    pub badge_sent: Option<Rect>,
}

impl Toast {
    fn new(heading: String, body: Vec<String>, secs: u64) -> Toast {
        Toast {
            heading,
            body,
            badge: None,
            lasts: Duration::from_secs(secs),
            queued: Instant::now(),
            shown: None,
            badge_sent: None,
        }
    }

    /// The notice for an achievement event, with the achievement's badge if `badges`, or else
    /// what to say in the status bar, if anything.
    pub fn for_event(event: Event, badges: bool) -> Result<Toast, Option<String>> {
        let plural = |n: u32, one: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") };
        match event {
            Event::Loaded { title, total: 0, .. } => {
                Ok(Toast::new(title, vec!["No achievements for this game yet".into()], 4))
            }
            Event::Loaded { title, unlocked, total, points, points_total } => Ok(Toast::new(
                title,
                vec![format!(
                    "{unlocked} of {} unlocked · {points} of {points_total} points",
                    plural(total, "achievement")
                )],
                4,
            )),
            Event::Unlocked { title, description, points, badge_url } => {
                let mut toast = Toast::new(
                    format!("Achievement unlocked · {}", plural(points, "point")),
                    vec![title, description],
                    5,
                );
                toast.badge = badge_url.filter(|_| badges).map(|url| fetch_badge(&url));
                Ok(toast)
            }
            Event::Mastered { title } => {
                Ok(Toast::new("Mastered".into(), vec![title, "Every achievement unlocked".into()], 6))
            }
            Event::Offline => Err(Some("Can't reach RetroAchievements; unlocks will be sent when it's back".into())),
            Event::Online => Err(Some("RetroAchievements is back; unlocks sent".into())),
            Event::Problem(p) => Err(Some(p)),
        }
    }

    /// Ready to go up: its badge is here, or it has waited long enough for it.
    pub fn ready(&self) -> bool {
        match &self.badge {
            None => true,
            Some(state) => {
                !matches!(*state.lock().unwrap_or_else(|e| e.into_inner()), BadgeState::Loading)
                    || self.queued.elapsed() >= BADGE_WAIT
            }
        }
    }

    /// Marks it as up, from now if it wasn't already.
    pub fn show(&mut self) {
        self.shown.get_or_insert_with(Instant::now);
    }

    /// It has been up as long as it stays.
    pub fn over(&self) -> bool {
        self.shown.is_some_and(|s| s.elapsed() >= self.lasts)
    }

    /// Its badge, if it has one and it's here.
    pub fn badge(&self) -> Option<image::RgbaImage> {
        match &*self.badge.as_ref()?.lock().unwrap_or_else(|e| e.into_inner()) {
            BadgeState::Loaded(image) => Some(image.clone()),
            _ => None,
        }
    }

    /// Where it goes over a picture in `picture`'s cells, cells being `cell` pixels; None if the
    /// picture is too small for it.
    pub fn layout(&self, picture: Rect, cell: (u32, u32)) -> Option<Layout> {
        layout(&self.heading, &self.body, self.badge().is_some(), picture, cell)
    }

    #[cfg(test)]
    pub fn lines(&self) -> Vec<String> {
        std::iter::once(self.heading.clone()).chain(self.body.iter().cloned()).collect()
    }
}

fn layout(heading: &str, body: &[String], badge: bool, picture: Rect, (cw, ch): (u32, u32)) -> Option<Layout> {
    let width = |s: &str| s.chars().count() as u16;
    // Inside the border, with a cell's gap each side.
    let room = picture.width.checked_sub(4).filter(|&w| w > 0)?;
    let rows = picture.height.checked_sub(2).filter(|&h| h > 0)?;
    let badge_rows = BADGE_ROWS.min(rows);
    let badge_cols = (f64::from(badge_rows) * f64::from(ch) / f64::from(cw.max(1))).ceil() as u16;
    let badge = badge && room >= badge_cols + 1 + MIN_TEXT_BESIDE_BADGE;
    let beside = if badge { badge_cols + 1 } else { 0 };
    let longest = std::iter::once(heading).chain(body.iter().map(String::as_str)).map(width).max().unwrap_or(0);
    let text_w = longest.min(room - beside).max(1);

    let mut lines = vec![(crate::text::truncate(heading, text_w as usize), Weight::Heading)];
    for (i, part) in body.iter().enumerate() {
        let weight = if i == 0 { Weight::Bold } else { Weight::Plain };
        lines.extend(crate::text::wrap(part, text_w as usize, MAX_WRAP).into_iter().map(|l| (l, weight)));
    }
    lines.truncate(rows as usize);

    let inner_h = (lines.len() as u16).max(if badge { badge_rows } else { 0 });
    let (w, h) = (text_w + beside + 4, inner_h + 2);
    let area = Rect { x: picture.x + (picture.width - w) / 2, y: picture.y + picture.height - h, width: w, height: h };
    let badge = badge.then_some(Rect { x: area.x + 2, y: area.y + 1, width: badge_cols, height: badge_rows });
    let text = Rect { x: area.x + 2 + beside, y: area.y + 1, width: text_w, height: lines.len() as u16 };
    Some(Layout { area, badge, text, centered: badge.is_none(), lines })
}

/// A badge, `w`×`h` pixels of RGBA: square, as big as fits, in the middle, clear around it.
/// Scaled up nearest neighbour, as the game is, and down smoothly.
pub fn badge_pixels(badge: &image::RgbaImage, (w, h): (u32, u32)) -> Vec<u8> {
    use image::imageops::FilterType;
    let side = w.min(h).max(1);
    let filter = if side >= badge.width() { FilterType::Nearest } else { FilterType::Triangle };
    let scaled = image::imageops::resize(badge, side, side, filter);
    let mut out = image::RgbaImage::new(w, h);
    image::imageops::overlay(&mut out, &scaled, i64::from((w - side) / 2), i64::from((h - side) / 2));
    out.into_raw()
}

/// Where a badge is cached: romi's cache, by the badge's file name.
fn badge_path(url: &str) -> Option<PathBuf> {
    let name = url.rsplit('/').next()?.split(['?', '#']).next()?;
    let ok = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    ok.then(|| crate::config::cache_dir().join("ra").join("badges").join(name))
}

/// Fetches a badge in the background: from the cache, or else downloaded and cached.
fn fetch_badge(url: &str) -> Arc<Mutex<BadgeState>> {
    let state = Arc::new(Mutex::new(BadgeState::Loading));
    let (url, shared) = (url.to_string(), state.clone());
    std::thread::spawn(move || {
        let path = badge_path(&url);
        let bytes = match path.as_ref().and_then(|p| std::fs::read(p).ok()) {
            Some(bytes) => Some(bytes),
            None => crate::art::get(&crate::art::agent(), &url, 1 << 20).ok().flatten().inspect(|bytes| {
                if let Some(path) = &path {
                    let _ = path.parent().map(std::fs::create_dir_all);
                    let _ = crate::files::write_atomic(path, bytes);
                }
            }),
        };
        let image = bytes.and_then(|b| image::load_from_memory(&b).ok()).map(|i| i.to_rgba8());
        *shared.lock().unwrap_or_else(|e| e.into_inner()) = match image {
            Some(image) => BadgeState::Loaded(image),
            None => BadgeState::Failed,
        };
    });
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    const PICTURE: Rect = Rect { x: 10, y: 5, width: 60, height: 30 };

    fn body() -> Vec<String> {
        vec!["Zigzagone".into(), "Help out Professor Birch with the help of your new little friend".into()]
    }

    #[test]
    fn without_a_badge_the_lines_are_centered_and_long_ones_wrap() {
        let l = layout("Achievement unlocked · 1 point", &body(), false, PICTURE, (10, 20)).unwrap();
        assert!(l.centered && l.badge.is_none());
        let lines: Vec<&str> = l.lines.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(
            lines,
            [
                "Achievement unlocked · 1 point",
                "Zigzagone",
                "Help out Professor Birch with the help of your new",
                "little friend"
            ]
        );
        assert_eq!(
            l.lines.iter().map(|&(_, w)| w).collect::<Vec<_>>()[..3],
            [Weight::Heading, Weight::Bold, Weight::Plain]
        );
        assert_eq!((l.text.width, l.area.width), (56, 60), "the longest line fills the picture's width");
        assert_eq!(l.area.y + l.area.height, PICTURE.y + PICTURE.height, "along the picture's bottom");
        assert_eq!(l.area.height, 6);
    }

    #[test]
    fn a_badge_is_square_three_rows_tall_with_the_text_beside_it() {
        // Cells twice as tall as they're wide: three rows make six columns.
        let l = layout("Achievement unlocked · 1 point", &body(), true, PICTURE, (10, 20)).unwrap();
        assert_eq!(l.badge, Some(Rect { x: l.area.x + 2, y: l.area.y + 1, width: 6, height: 3 }));
        assert!(!l.centered);
        assert_eq!(l.text.x, l.area.x + 2 + 6 + 1);
        assert!(l.lines.iter().all(|(s, _)| s.chars().count() <= 56 - 7));
    }

    #[test]
    fn a_narrow_picture_leaves_the_badge_out_and_a_tiny_one_the_notice() {
        let narrow = Rect { width: 20, ..PICTURE };
        assert!(layout("Unlocked", &body(), true, narrow, (10, 20)).unwrap().badge.is_none());
        assert!(layout("Unlocked", &body(), true, Rect { width: 4, ..PICTURE }, (10, 20)).is_none());
    }

    #[test]
    fn badges_sit_square_in_the_middle_of_their_cells() {
        let badge = image::RgbaImage::from_pixel(64, 64, image::Rgba([200, 100, 50, 255]));
        let px = badge_pixels(&badge, (80, 60));
        let alpha = |x: usize, y: usize| px[(y * 80 + x) * 4 + 3];
        assert_eq!((alpha(5, 30), alpha(40, 30), alpha(75, 30)), (0, 255, 0), "clear either side of a 60px square");
    }

    #[test]
    fn achievement_events_become_notices_or_status() {
        let loaded =
            Event::Loaded { title: "Pocket Lantern".into(), unlocked: 3, total: 25, points: 20, points_total: 310 };
        assert_eq!(
            Toast::for_event(loaded, true).ok().unwrap().lines(),
            ["Pocket Lantern", "3 of 25 achievements unlocked · 20 of 310 points"]
        );
        let none = Event::Loaded { title: "Demo".into(), unlocked: 0, total: 0, points: 0, points_total: 0 };
        assert_eq!(Toast::for_event(none, true).ok().unwrap().lines()[1], "No achievements for this game yet");
        let unlocked = Event::Unlocked {
            title: "First Light".into(),
            description: "Light the lantern".into(),
            points: 1,
            badge_url: None,
        };
        let toast = Toast::for_event(unlocked, true).ok().unwrap();
        assert_eq!(toast.lines(), ["Achievement unlocked · 1 point", "First Light", "Light the lantern"]);
        assert!(toast.ready(), "with no badge to wait for");
        assert_eq!(Toast::for_event(Event::Problem("nope".into()), true).err(), Some(Some("nope".into())));
    }

    #[test]
    fn badges_are_cached_by_their_file_name_only() {
        let path = badge_path("https://media.retroachievements.org/Badge/609830.png?x=1").unwrap();
        assert!(path.ends_with("ra/badges/609830.png"));
        assert_eq!(badge_path("https://example.com/../../etc/passwd/"), None);
    }
}

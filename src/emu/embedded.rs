//! A game played inside the TUI, in the middle panel. The game runs on its own thread (see
//! `runner`), and the picture is a Kitty image behind Unicode placeholder cells: ratatui
//! lays those out like text, and each new frame only replaces the image behind them.
//!
//! romi scales each frame to the exact pixel size of its cells itself, nearest neighbour, because
//! a terminal scaling it would smooth the pixel art.
//!
//! For systems with a wireframe, the launch animation plays first, across the whole panel, as an
//! image of its own. The core waits until the game starts fading in on the glass, so the game's
//! first frames are the ones seen as the camera lands. The game's image is in place before the
//! animation's goes, so the handover doesn't show.

use crate::emu::cheevos::{self, Event as CheevoEvent};
use crate::emu::controls::Controls;
use crate::emu::kitty;
use crate::emu::libretro::{Core, Frame};
use crate::emu::runner::Shared;
use crate::emu::screen::{self, Filter};
use crate::emu::wireframe::{self, Beam, Model, Painter, Picture, Plan, Settings, View};
use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::{execute, terminal};
use ratatui::layout::Rect;
use std::collections::VecDeque;
use std::io::Write;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The image id, which the placeholder cells carry as their colour ("rom" in ASCII).
pub const IMAGE: u32 = 0x72_6f_6d;
/// The launch animation's image ("wir").
const INTRO_IMAGE: u32 = 0x77_69_72;
/// Frames sent but maybe not read yet; older ones still around were never read and are removed.
const PENDING: usize = 30;
/// The animation is drawn at 1/this of the panel's pixels, and the terminal scales it up.
const SHRINK: u32 = 2;
const INTRO_FPS: f64 = 60.0;

/// A notice over the game: an achievement unlocked, or the game's achievements loaded. The first
/// line is its heading.
#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    pub lines: Vec<String>,
    lasts: Duration,
    /// When it went up, once it has.
    shown: Option<Instant>,
}

impl Toast {
    fn new(lines: Vec<String>, secs: u64) -> Toast {
        Toast { lines, lasts: Duration::from_secs(secs), shown: None }
    }

    /// The notice for an achievement event, if it gets one, or else what to say in the status bar.
    fn for_event(event: CheevoEvent) -> Result<Toast, Option<String>> {
        let plural = |n: u32, one: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") };
        match event {
            CheevoEvent::Loaded { title, total: 0, .. } => {
                Ok(Toast::new(vec![title, "No achievements for this game yet".into()], 4))
            }
            CheevoEvent::Loaded { title, unlocked, total, points, points_total } => Ok(Toast::new(
                vec![
                    title,
                    format!(
                        "{unlocked} of {} unlocked · {points} of {points_total} points",
                        plural(total, "achievement")
                    ),
                ],
                4,
            )),
            CheevoEvent::Unlocked { title, description, points, .. } => Ok(Toast::new(
                vec![format!("Achievement unlocked · {}", plural(points, "point")), title, description],
                5,
            )),
            CheevoEvent::Mastered { title } => {
                Ok(Toast::new(vec!["Mastered".into(), title, "Every achievement unlocked".into()], 6))
            }
            CheevoEvent::Offline => {
                Err(Some("Can't reach RetroAchievements; unlocks will be sent when it's back".into()))
            }
            CheevoEvent::Online => Err(Some("RetroAchievements is back; unlocks sent".into())),
            CheevoEvent::Problem(p) => Err(Some(p)),
        }
    }
}

/// What the launch animation needs to know to play.
pub struct Launch {
    pub model: Model,
    pub seed: u32,
    pub beam: Beam,
}

/// The launch animation, while it plays.
struct Intro {
    launch: Launch,
    plan: Plan,
    painter: Painter,
    began: Instant,
    /// Seconds a key skipped.
    skipped: f64,
    painted: Option<Instant>,
    /// The panel's cells, from the last draw.
    area: Option<Rect>,
    /// The game's newest frame, once the core is going.
    game: Option<Frame>,
}

impl Intro {
    fn t(&self) -> f64 {
        self.began.elapsed().as_secs_f64() + self.skipped
    }

    fn end(&self) -> f64 {
        self.plan.zoom_end + wireframe::FADE
    }

    fn skip(&mut self) {
        self.skipped = self.skipped.max(self.end() - self.began.elapsed().as_secs_f64());
    }
}

pub struct Embedded {
    pub game: usize,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    /// Number of the last frame taken from the core.
    seen: u64,
    /// The size the newest frame is meant to be seen at, which decides the picture's shape.
    shape: (f64, f64),
    /// The cells the picture covers, from the last draw, and their size in pixels.
    cells: Option<(Rect, (u32, u32))>,
    controls: Controls,
    sent: u64,
    pending: VecDeque<String>,
    intro: Option<Intro>,
    /// The animation has ended, but its image is still up until the game's cells are drawn.
    intro_shown: bool,
    /// How round the picture's corners are, as a fraction of its height.
    corners: f64,
    filter: Filter,
    /// Achievement notices still to show, the one showing first.
    toasts: VecDeque<Toast>,
}

impl Embedded {
    /// Starts the game, after the launch animation if there's one, with corners `corners` of its
    /// height round and `filter` over the picture, and its achievements checked with `cheevos`.
    pub fn start(
        game: usize,
        core: Core,
        volume: f32,
        launch: Option<Launch>,
        corners: f64,
        filter: Filter,
        cheevos: Option<cheevos::Setup>,
    ) -> Result<Embedded> {
        // With the animation, the game's cells are placed before it draws, in the shape it says.
        let shape = if launch.is_some() { core.display_size() } else { (0.0, 0.0) };
        let (shared, thread) = Shared::start(core, volume, launch.is_some(), cheevos)?;
        // Key releases are needed to know when a button is let go.
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES | KeyboardEnhancementFlags::REPORT_EVENT_TYPES;
        execute!(std::io::stdout(), PushKeyboardEnhancementFlags(flags))?;
        let intro = launch.map(|launch| Intro {
            plan: Plan::new(&launch.model, &Settings::default(), launch.seed.max(1)),
            launch,
            painter: Painter::default(),
            began: Instant::now(),
            skipped: 0.0,
            painted: None,
            area: None,
            game: None,
        });
        Ok(Embedded {
            game,
            shared,
            thread: Some(thread),
            seen: 0,
            shape,
            cells: None,
            controls: Controls::new(),
            sent: 0,
            pending: VecDeque::new(),
            intro_shown: intro.is_some(),
            intro,
            corners,
            filter,
            toasts: VecDeque::new(),
        })
    }

    /// Takes a key. False when it stops the game. While the animation plays, any other key skips it.
    pub fn on_key(&mut self, k: KeyEvent) -> bool {
        if k.code == KeyCode::Esc && k.kind != KeyEventKind::Release {
            return false;
        }
        match &mut self.intro {
            Some(intro) if k.kind == KeyEventKind::Press => intro.skip(),
            Some(_) => {}
            None => self.controls.key(k),
        }
        true
    }

    /// Reads the controllers and hands the held buttons to the core. False when Select+Start
    /// has been held long enough to stop the game. While the animation plays, a button skips it.
    pub fn read_pads(&mut self) -> bool {
        let Some(buttons) = self.controls.buttons() else { return false };
        match &mut self.intro {
            Some(intro) if buttons != 0 => intro.skip(),
            Some(_) => {}
            None => self.shared.set_buttons(buttons),
        }
        true
    }

    /// Why the game has no sound, once, if it has none.
    pub fn sound_error(&self) -> Option<String> {
        self.shared.take_sound_error()
    }

    /// Why the game's save couldn't be written, once for each time it couldn't.
    pub fn save_error(&self) -> Option<String> {
        self.shared.take_save_error()
    }

    /// Queues notices for what's happened with the game's achievements, and gives back the last
    /// thing to say in the status bar, if there's one.
    pub fn cheevo_status(&mut self) -> Option<String> {
        let mut status = None;
        for event in self.shared.take_cheevo_events() {
            match Toast::for_event(event) {
                Ok(toast) => self.toasts.push_back(toast),
                Err(say) => status = say.or(status),
            }
        }
        status
    }

    /// The notice to show over the game now, if there's one.
    pub fn toast(&mut self) -> Option<&Toast> {
        while self.toasts.front().is_some_and(|t| t.shown.is_some_and(|s| s.elapsed() >= t.lasts)) {
            self.toasts.pop_front();
        }
        // Notices wait for the launch animation, so the first one isn't missed.
        if self.intro.is_some() {
            return None;
        }
        let toast = self.toasts.front_mut()?;
        toast.shown.get_or_insert_with(Instant::now);
        Some(toast)
    }

    /// Stops the game, which saves it a last time, and says why that save failed if it did.
    pub fn stop(mut self) -> Option<String> {
        self.shared.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.shared.take_save_error()
    }

    /// The cells to put the picture's placeholders in, and the image they show: the whole of
    /// `area` while the animation plays, then the game's cells. None until there's something.
    pub fn place(&mut self, area: Rect) -> Option<(Rect, u32)> {
        if let Some(intro) = &mut self.intro {
            intro.area = Some(area);
            return (!area.is_empty()).then_some((area, INTRO_IMAGE));
        }
        self.cells = self.fit(area);
        self.cells.map(|(rect, _)| (rect, IMAGE))
    }

    /// The game's cells in `area`: as big as fits, in the frame's shape (or `shape` until there's
    /// a frame), centered, and their size in pixels.
    fn fit(&self, area: Rect) -> Option<(Rect, (u32, u32))> {
        let (fw, fh) = self.shape;
        if fw <= 0.0 || fh <= 0.0 || area.is_empty() {
            return None;
        }
        let (cell_w, cell_h) = kitty::cell_size(&terminal::window_size().ok()?);
        let fit = (f64::from(u32::from(area.width) * cell_w) / fw).min(f64::from(u32::from(area.height) * cell_h) / fh);
        let cols = ((fw * fit / f64::from(cell_w)).round() as u16).clamp(1, area.width);
        let rows = ((fh * fit / f64::from(cell_h)).round() as u16).clamp(1, area.height);
        let rect = Rect::new(area.x + (area.width - cols) / 2, area.y + (area.height - rows) / 2, cols, rows);
        Some((rect, (u32::from(cols) * cell_w, u32::from(rows) * cell_h)))
    }

    /// Plays the animation's next frame, or sends the core's newest frame to the terminal if
    /// there's one it hasn't seen.
    pub fn pump(&mut self) -> Result<()> {
        if let Some(mut intro) = self.intro.take() {
            if self.play_intro(&mut intro)? {
                self.intro = Some(intro);
            }
            return Ok(());
        }
        if self.intro_shown && self.cells.is_some() {
            // The game's cells have been drawn over the animation's, so its image can go.
            self.intro_shown = false;
            write!(std::io::stdout(), "{}", kitty::delete(INTRO_IMAGE))?;
        }
        let Some((n, frame)) = self.shared.newer_frame(self.seen, Duration::ZERO) else { return Ok(()) };
        self.seen = n;
        if frame.width == 0 {
            return Ok(());
        }
        self.shape = frame.display_size();
        self.send_game(&frame)
    }

    /// One step of the animation. False once it's over, with the game's first frame sent.
    fn play_intro(&mut self, intro: &mut Intro) -> Result<bool> {
        let t = intro.t();
        if t >= intro.plan.game_in {
            self.shared.resume();
        }
        if let Some((n, frame)) = self.shared.newer_frame(self.seen, Duration::ZERO) {
            self.seen = n;
            if frame.width > 0 {
                self.shape = frame.display_size();
                intro.game = Some(frame);
            }
        }
        if self.shape.0 <= 0.0 || self.shape.1 <= 0.0 {
            self.shape = intro.launch.model.aspect;
        }
        intro.launch.model.fit(self.shape);
        let Some(area) = intro.area else { return Ok(true) };
        if t >= intro.end() {
            self.shared.resume();
            self.cells = self.fit(area);
            if let Some(frame) = &intro.game {
                self.send_game(frame)?;
            }
            return Ok(false);
        }
        if intro.painted.is_some_and(|p| p.elapsed().as_secs_f64() < 1.0 / INTRO_FPS) {
            return Ok(true);
        }
        intro.painted = Some(Instant::now());
        let (Some((rect, _)), Ok(size)) = (self.fit(area), terminal::window_size()) else { return Ok(true) };
        let (cell_w, cell_h) = kitty::cell_size(&size);
        let (cw, ch) = (f64::from(cell_w) / f64::from(SHRINK), f64::from(cell_h) / f64::from(SHRINK));
        let target = [
            f64::from(rect.x - area.x) * cw,
            f64::from(rect.y - area.y) * ch,
            f64::from(rect.width) * cw,
            f64::from(rect.height) * ch,
        ];
        let w = (f64::from(area.width) * cw).round() as usize;
        let h = (f64::from(area.height) * ch).round() as usize;
        let view = View { corners: self.corners, ..View::new(w, h, target, cw) };
        let game = intro.game.as_ref().map(|f| Picture { rgb: &f.rgb, w: f.width as usize, h: f.height as usize });
        let pixels = intro.painter.paint(&intro.launch.model, &intro.plan, &view, t, game, intro.launch.beam);
        self.send(INTRO_IMAGE, area, pixels, (view.w as u32, view.h as u32), 32)?;
        Ok(true)
    }

    /// Sends a game frame scaled to the game's cells, filtered, its corners rounded if they're to
    /// be: clear, so the terminal's background shows there.
    fn send_game(&mut self, frame: &Frame) -> Result<()> {
        let Some((rect, (w, h))) = self.cells else { return Ok(()) };
        let sizes = ((w as usize, h as usize), (frame.width as usize, frame.height as usize));
        if self.corners <= 0.0 {
            let mut pixels = frame.resample(w, h);
            screen::apply(&mut pixels, 3, sizes.0, sizes.1, self.filter);
            return self.send(IMAGE, rect, &pixels, (w, h), 24);
        }
        let mut pixels = frame.resample_rgba(w, h);
        screen::apply(&mut pixels, 4, sizes.0, sizes.1, self.filter);
        let (fw, fh) = (f64::from(w), f64::from(h));
        wireframe::round_corners(&mut pixels, w as usize, [0.0, 0.0, fw, fh], self.corners * fh);
        self.send(IMAGE, rect, &pixels, (w, h), 32)
    }

    /// Replaces image `id`, `w`×`h` pixels of RGB (24) or RGBA (32), stretched over `rect`.
    fn send(&mut self, id: u32, rect: Rect, pixels: &[u8], (w, h): (u32, u32), format: u8) -> Result<()> {
        self.sent += 1;
        let name = format!("/romi-{}-{}", std::process::id(), self.sent);
        kitty::write_shm(&name, pixels)?;
        let mut out = std::io::stdout().lock();
        write!(
            out,
            "\x1b_Ga=T,U=1,i={id},c={},r={},f={format},s={w},v={h},t=s,S={},q=2;{}\x1b\\",
            rect.width,
            rect.height,
            pixels.len(),
            BASE64.encode(&name),
        )?;
        out.flush()?;
        self.pending.push_back(name);
        while self.pending.len() > PENDING {
            kitty::remove_shm(&self.pending.pop_front().unwrap());
        }
        Ok(())
    }
}

impl Drop for Embedded {
    fn drop(&mut self) {
        self.shared.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let mut out = std::io::stdout();
        let _ = write!(out, "{}{}", kitty::delete(IMAGE), kitty::delete(INTRO_IMAGE));
        let _ = execute!(out, PopKeyboardEnhancementFlags);
        for name in &self.pending {
            kitty::remove_shm(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn achievement_events_become_notices_or_status() {
        let loaded = CheevoEvent::Loaded {
            title: "Pocket Lantern".into(),
            unlocked: 3,
            total: 25,
            points: 20,
            points_total: 310,
        };
        assert_eq!(
            Toast::for_event(loaded).unwrap().lines,
            ["Pocket Lantern", "3 of 25 achievements unlocked · 20 of 310 points"]
        );
        let none = CheevoEvent::Loaded { title: "Demo".into(), unlocked: 0, total: 0, points: 0, points_total: 0 };
        assert_eq!(Toast::for_event(none).unwrap().lines[1], "No achievements for this game yet");
        let unlocked = CheevoEvent::Unlocked {
            title: "First Light".into(),
            description: "Light the lantern".into(),
            points: 1,
            badge_url: None,
        };
        assert_eq!(
            Toast::for_event(unlocked).unwrap().lines,
            ["Achievement unlocked · 1 point", "First Light", "Light the lantern"]
        );
        assert_eq!(Toast::for_event(CheevoEvent::Problem("nope".into())), Err(Some("nope".into())));
    }
}

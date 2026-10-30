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

use crate::emu::cheevos;
use crate::emu::controls::Controls;
use crate::emu::crop::Crop;
use crate::emu::kitty;
use crate::emu::libretro::{Core, Frame};
use crate::emu::notice::{self, Toast};
use crate::emu::runner::Shared;
use crate::emu::screen::{self, Filter};
use crate::emu::timing::{self, Recorder};
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
/// An achievement's badge, in a notice over the game ("ach").
pub const BADGE_IMAGE: u32 = 0x61_63_68;
/// Frames sent but maybe not read yet; older ones still around were never read and are removed.
const PENDING: usize = 30;
/// The animation is drawn at 1/this of the panel's pixels, and the terminal scales it up.
const SHRINK: u32 = 2;
const INTRO_FPS: f64 = 60.0;

/// How the game looks in the panel.
#[derive(Debug, Clone, Copy)]
pub struct Appearance {
    /// How round the picture's corners are, as a fraction of its height.
    pub corners: f64,
    pub filter: Filter,
    /// Black edges round the picture are cropped off (see `crop`).
    pub crop: bool,
    /// Achievement notices show the achievement's badge.
    pub badges: bool,
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
    look: Appearance,
    /// Achievement notices still to show, the one showing first.
    toasts: VecDeque<Toast>,
    /// Frame timings, if `ROMI_FRAME_LOG` asks for them.
    timing: Option<Recorder>,
    /// The last frame sent, kept to scale the next one into.
    pixels: Vec<u8>,
    /// What's known of the game's black edges, if they're cropped.
    crop: Option<Crop>,
}

impl Embedded {
    /// Starts the game, after the launch animation if there's one, looking as `look` says, and
    /// its achievements checked with `cheevos`.
    pub fn start(
        game: usize,
        core: Core,
        volume: f32,
        launch: Option<Launch>,
        look: Appearance,
        cheevos: Option<cheevos::Setup>,
    ) -> Result<Embedded> {
        let about = format!("{} in the TUI, filter {}, corners {}", core.name, look.filter.name(), look.corners);
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
            look,
            toasts: VecDeque::new(),
            timing: Recorder::start(about),
            pixels: Vec::new(),
            crop: look.crop.then(Crop::default),
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

    /// Reads the controllers and hands what's held to the core. False when Select+Start has been
    /// held long enough to stop the game. While the animation plays, a button skips it.
    pub fn read_pads(&mut self) -> bool {
        let Some(input) = self.controls.input() else { return false };
        match &mut self.intro {
            Some(intro) if input.buttons != 0 => intro.skip(),
            Some(_) => {}
            None => self.shared.set_input(input),
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
            match Toast::for_event(event, self.look.badges) {
                Ok(toast) => self.toasts.push_back(toast),
                Err(say) => status = say.or(status),
            }
        }
        status
    }

    /// The notice to show over the game's `picture` now, if there's one, with its badge sent
    /// for the cells it goes in.
    pub fn place_toast(&mut self, picture: Rect) -> Option<notice::Layout> {
        while self.toasts.front().is_some_and(Toast::over) {
            if self.toasts.pop_front().is_some_and(|t| t.badge_sent.is_some()) {
                let _ = write!(std::io::stdout(), "{}", kitty::delete(BADGE_IMAGE));
            }
        }
        // Notices wait for the launch animation, so the first one isn't missed, and for their
        // badge, a little.
        if self.intro.is_some() || !self.toasts.front()?.ready() {
            return None;
        }
        let cell = terminal::window_size().map_or((8, 16), |size| kitty::cell_size(&size));
        let toast = self.toasts.front_mut()?;
        toast.show();
        let layout = toast.layout(picture, cell)?;
        if let Some(cells) = layout.badge
            && toast.badge_sent != Some(cells)
            && let Some(badge) = toast.badge()
        {
            toast.badge_sent = Some(cells);
            let size = (u32::from(cells.width) * cell.0, u32::from(cells.height) * cell.1);
            let pixels = notice::badge_pixels(&badge, size);
            if let Err(e) = self.send(BADGE_IMAGE, cells, &pixels, size, 32) {
                crate::emu::cheevos::note(&format!("can't show a badge: {e:#}"));
            }
        }
        Some(layout)
    }

    /// Writes the frame timings, if they're being recorded, and says where.
    pub fn write_timings(&mut self) -> Option<Result<std::path::PathBuf>> {
        let timing = self.timing.take()?;
        Some(timing.finish(&self.shared.take_made()))
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

    /// The frame with its black edges cropped off, if they're to be.
    fn cropped(&mut self, frame: Frame) -> Frame {
        match &mut self.crop {
            Some(crop) => crop.apply(frame),
            None => frame,
        }
    }

    /// Waits up to `wait` for the core's next frame, so it can be sent as soon as it's made. False,
    /// at once, while the launch animation plays: it keeps its own time.
    pub fn wait_for_frame(&self, wait: Duration) -> bool {
        if self.intro.is_some() {
            return false;
        }
        self.shared.wait_newer(self.seen, wait);
        true
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
        let frame = self.cropped(frame);
        self.shape = frame.display_size();
        self.send_game(n, &frame)
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
                let frame = self.cropped(frame);
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
                self.send_game(self.seen, frame)?;
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
        let view = View { corners: self.look.corners, ..View::new(w, h, target, cw) };
        let game = intro.game.as_ref().map(|f| Picture { rgb: &f.rgb, w: f.width as usize, h: f.height as usize });
        let pixels = intro.painter.paint(&intro.launch.model, &intro.plan, &view, t, game, intro.launch.beam);
        self.send(INTRO_IMAGE, area, pixels, (view.w as u32, view.h as u32), 32)?;
        Ok(true)
    }

    /// Sends game frame number `n` scaled to the game's cells, as `game_pixels` makes it.
    fn send_game(&mut self, n: u64, frame: &Frame) -> Result<()> {
        let Some((rect, (w, h))) = self.cells else { return Ok(()) };
        let taken = Instant::now();
        let mut pixels = std::mem::take(&mut self.pixels);
        let format = game_pixels(frame, (w, h), self.look, &mut pixels);
        let prepared = Instant::now();
        let sent = self.send(IMAGE, rect, &pixels, (w, h), format);
        let bytes = pixels.len();
        self.pixels = pixels;
        let (name, copied) = sent?;
        if let Some(timing) = &mut self.timing {
            let written = Instant::now();
            timing.handed_over(n, &name);
            timing.sent(timing::Sent { frame: n, taken, prepared, copied, written, size: (w, h), bytes });
        }
        Ok(())
    }

    /// Replaces image `id`, `w`×`h` pixels of RGB (24) or RGBA (32), stretched over `rect`. Gives
    /// back the shared memory object it went in, and when its pixels were copied there.
    fn send(
        &mut self,
        id: u32,
        rect: Rect,
        pixels: &[u8],
        (w, h): (u32, u32),
        format: u8,
    ) -> Result<(String, Instant)> {
        self.sent += 1;
        let name = format!("/romi-{}-{}", std::process::id(), self.sent);
        kitty::write_shm(&name, pixels)?;
        let copied = Instant::now();
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
        self.pending.push_back(name.clone());
        while self.pending.len() > PENDING {
            let old = self.pending.pop_front().unwrap();
            if kitty::remove_shm(&old)
                && let Some(timing) = &mut self.timing
            {
                timing.never_read(&old);
            }
        }
        Ok((name, copied))
    }
}

/// A game frame as it's sent, into `pixels` in place of what they were: scaled to `w`×`h`,
/// filtered, its corners rounded if they're to be (clear, so the terminal's background shows
/// there). Gives back their format: RGB (24) or RGBA (32).
pub fn game_pixels(frame: &Frame, (w, h): (u32, u32), look: Appearance, pixels: &mut Vec<u8>) -> u8 {
    let bpp = if look.corners > 0.0 { 4 } else { 3 };
    frame.resample_into(w, h, bpp, pixels);
    screen::apply(pixels, bpp, (w as usize, h as usize), console_size(frame), look.filter);
    if bpp == 3 {
        return 24;
    }
    let (fw, fh) = (f64::from(w), f64::from(h));
    wireframe::round_corners(pixels, w as usize, [0.0, 0.0, fw, fh], look.corners * fh);
    32
}

/// The picture's size on the console that made it, for the screen filter: the frame's own, or for
/// a 3D game drawn bigger than the console did (see `sharp_3d`), that many times smaller. Consoles
/// drew about 240 lines, so a frame well over that is taken as 240 drawn bigger, and the
/// scanlines and phosphor mask stay the size they were on a TV.
fn console_size(frame: &Frame) -> (usize, usize) {
    let times = (f64::from(frame.height) / 240.0).round().max(1.0) as usize;
    (frame.width as usize / times, frame.height as usize / times)
}

impl Drop for Embedded {
    fn drop(&mut self) {
        let _ = self.write_timings();
        self.shared.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let mut out = std::io::stdout();
        let _ = write!(out, "{}{}{}", kitty::delete(IMAGE), kitty::delete(INTRO_IMAGE), kitty::delete(BADGE_IMAGE));
        let _ = execute!(out, PopKeyboardEnhancementFlags);
        for name in &self.pending {
            kitty::remove_shm(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How long romi takes to get a frame ready for the terminal, by the panel's size in pixels
    /// and how the game looks. Not a test: run it with
    /// `cargo test --release frame_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn frame_cost() {
        let (fw, fh) = (240, 160);
        let rgb = (0..fw * fh * 3).map(|i| (i * 7 % 251) as u8).collect();
        let frame = Frame { width: fw, height: fh, rgb, pixel_aspect: 0.0 };
        let look = |filter, corners| Appearance { corners, filter, crop: false, badges: false };
        let looks = [
            ("plain", look(Filter::None, 0.0)),
            ("crt", look(Filter::Crt, 0.0)),
            ("corners", look(Filter::None, 0.02)),
            ("crt+corners", look(Filter::Crt, 0.02)),
        ];
        print!("\n{:>11} {:>7}", "panel", "MB");
        for (name, _) in &looks {
            print!(" {name:>11}");
        }
        println!(" {:>11}   (ms a frame, the copy into shared memory included)", "copy only");
        for (w, h) in [(720, 480), (1080, 720), (1440, 960), (1920, 1280), (2400, 1600), (3000, 2000)] {
            const RUNS: u32 = 30;
            print!("{:>11} {:>7.1}", format!("{w}×{h}"), f64::from(w * h * 4) / 1e6);
            let mut copy = Duration::ZERO;
            for (_, look) in looks {
                let started = Instant::now();
                let mut pixels = Vec::new();
                for i in 0..RUNS {
                    game_pixels(&frame, (w, h), look, &mut pixels);
                    let name = format!("/romi-{}-bench-{i}", std::process::id());
                    let copying = Instant::now();
                    kitty::write_shm(&name, &pixels).unwrap();
                    copy += copying.elapsed();
                    kitty::remove_shm(&name);
                }
                print!(" {:>11.2}", started.elapsed().as_secs_f64() * 1000.0 / f64::from(RUNS));
            }
            println!(" {:>11.2}", copy.as_secs_f64() * 1000.0 / f64::from(RUNS * looks.len() as u32));
        }
    }
}

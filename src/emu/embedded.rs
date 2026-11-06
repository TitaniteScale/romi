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
use crate::emu::controls::{Chord, Controls};
use crate::emu::crop::Crop;
use crate::emu::kitty;
use crate::emu::libretro::{Core, Frame};
use crate::emu::notice::{self, Toast};
use crate::emu::runner::{Shared, StateAsk};
use crate::emu::savestate::{self, Files};
use crate::emu::screen::{self, Filter};
use crate::emu::timing::{self, Recorder};
use crate::emu::wireframe::{self, Beam, Model, Painter, Picture, Plan, Settings, View};
use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::{execute, terminal};
use ratatui::layout::Rect;
use std::collections::VecDeque;
use std::io::Write;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Mouse reporting while a game plays: presses, releases and drags with a button held (1000,
/// 1002), told as SGR (1006), or in pixels rather than cells (1016) where the terminal can.
const MOUSE_ON: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1006h";
const PIXEL_MOUSE_ON: &str = "\x1b[?1016h";
const MOUSE_OFF: &str = "\x1b[?1016l\x1b[?1006l\x1b[?1002l\x1b[?1000l";

/// The image id, which the placeholder cells carry as their colour ("rom" in ASCII).
pub const IMAGE: u32 = 0x72_6f_6d;
/// The launch animation's image ("wir").
const INTRO_IMAGE: u32 = 0x77_69_72;
/// An achievement's badge, in a notice over the game ("ach").
pub const BADGE_IMAGE: u32 = 0x61_63_68;

/// The Kitty image a badge slot (`notice::SLOT_*`) is sent as: the unlock notice's is `BADGE_IMAGE`,
/// and the others follow it.
pub fn badge_image(slot: usize) -> u32 {
    BADGE_IMAGE + slot as u32
}
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

/// What a game starts with.
pub struct Start {
    pub game: usize,
    pub volume: f32,
    /// The launch animation, if there's one.
    pub launch: Option<Launch>,
    pub look: Appearance,
    /// Its achievements, checked after every frame.
    pub cheevos: Option<cheevos::Setup>,
    /// The mouse touches the screen, where it points to the pixel if the terminal tells it in pixels,
    /// else to the cell.
    pub pixel_mouse: bool,
    /// Where its save states go, the slot it was last on, and the controller button that saves them.
    pub states: Files,
    pub slot: u8,
    pub state_button: crate::config::StateButton,
}

/// What plays: a libretro core, or a program run in gamescope, whose frames are captured.
pub enum Source {
    Core(Box<Core>),
    /// The game's command, and the file its output goes to.
    #[cfg(target_os = "linux")]
    Captured(crate::launch::Prepared, std::path::PathBuf),
}

pub struct Embedded {
    pub game: usize,
    /// It's a program captured from gamescope, not a core, so there are no save states.
    captured: bool,
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
    /// Progress towards an achievement that's counting up, while it changes.
    tracker: Option<notice::Tracker>,
    /// What the game says the player is doing, if it has a rich presence.
    presence: String,
    /// Achievements whose challenges are under way, oldest first.
    challenges: Vec<notice::Challenge>,
    /// The cells each badge image (`notice::SLOT_*`) was last sent for, so it's sent once.
    badge_cells: [Option<Rect>; notice::SLOTS],
    /// Frame timings, if `ROMI_FRAME_LOG` asks for them.
    timing: Option<Recorder>,
    /// The last frame sent, kept to scale the next one into.
    pixels: Vec<u8>,
    /// What's known of the game's black edges, if they're cropped.
    crop: Option<Crop>,
    /// The part of the core's frame the picture shows, its edges cropped or not: left, top,
    /// right, bottom (the last two past the end).
    shown: [u32; 4],
    /// The terminal says where the mouse is in pixels, not cells.
    pixel_mouse: bool,
    /// Where the left mouse button is touching the game's screen, as a pixel of the core's frame.
    touch: Option<[u16; 2]>,
    states: Files,
    /// The save state slot it's on.
    slot: u8,
    /// A slot moved to and not yet remembered.
    slot_moved: Option<u8>,
}

impl Embedded {
    /// Starts the game, after the launch animation if there's one.
    pub fn start(source: Source, start: Start) -> Result<Embedded> {
        let Start { game, volume, launch, look, cheevos, pixel_mouse, states, slot, state_button } = start;
        let paused = launch.is_some();
        let (name, captured, shape, (shared, thread)) = match source {
            Source::Core(core) => {
                // With the animation, the game's cells are placed before it draws, in the shape it says.
                let shape = if paused { core.display_size() } else { (0.0, 0.0) };
                (core.name.clone(), false, shape, Shared::start(*core, volume, paused, cheevos, Some(states.clone()))?)
            }
            #[cfg(target_os = "linux")]
            Source::Captured(program, log) => {
                use crate::emu::capture;
                let shared = Shared::new(volume, paused);
                let thread = capture::start(&program, &log, shared.clone())?;
                let (w, h) = capture::SIZE;
                let shape = if paused { (f64::from(w), f64::from(h)) } else { (0.0, 0.0) };
                ("gamescope".to_string(), true, shape, (shared, thread))
            }
        };
        let about = format!("{name} in the TUI, filter {}, corners {}", look.filter.name(), look.corners);
        // Key releases are needed to know when a button is let go.
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES | KeyboardEnhancementFlags::REPORT_EVENT_TYPES;
        execute!(std::io::stdout(), PushKeyboardEnhancementFlags(flags))?;
        write!(std::io::stdout(), "{MOUSE_ON}{}", if pixel_mouse { PIXEL_MOUSE_ON } else { "" })?;
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
        kitty::sweep_stale_shm();
        Ok(Embedded {
            game,
            captured,
            shared,
            thread: Some(thread),
            seen: 0,
            shape,
            cells: None,
            controls: Controls::new(state_button),
            sent: 0,
            pending: VecDeque::new(),
            intro_shown: intro.is_some(),
            intro,
            look,
            toasts: VecDeque::new(),
            tracker: None,
            challenges: Vec::new(),
            presence: String::new(),
            badge_cells: [None; notice::SLOTS],
            timing: Recorder::start(about),
            pixels: Vec::new(),
            crop: look.crop.then(Crop::default),
            shown: [0; 4],
            pixel_mouse,
            touch: None,
            states,
            slot,
            slot_moved: None,
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
            None => match (k.code, k.kind) {
                // RetroArch's keys, and F3 next to them.
                (KeyCode::F(2), KeyEventKind::Press) => self.chord(Chord::SaveState),
                (KeyCode::F(3), KeyEventKind::Press) => self.chord(Chord::UndoLoad),
                (KeyCode::F(4), KeyEventKind::Press) => self.chord(Chord::LoadState),
                (KeyCode::F(6), KeyEventKind::Press) => self.chord(Chord::PreviousSlot),
                (KeyCode::F(7), KeyEventKind::Press) => self.chord(Chord::NextSlot),
                (KeyCode::F(_), _) => {}
                _ => self.controls.key(k),
            },
        }
        true
    }

    /// Does what a button pressed with Select, or a function key, is for.
    fn chord(&mut self, chord: Chord) {
        if self.captured {
            return self.shared.say_about_states(Err("No save states for games played through gamescope".into()));
        }
        let ask = match chord {
            Chord::SaveState => StateAsk::Save(self.slot),
            Chord::LoadState => StateAsk::Load(self.slot),
            Chord::UndoLoad => StateAsk::Undo,
            Chord::NextSlot => return self.move_slot(1),
            Chord::PreviousSlot => return self.move_slot(savestate::SLOTS - 1),
        };
        self.shared.ask_state(ask);
    }

    fn move_slot(&mut self, by: u8) {
        self.slot = (self.slot + by) % savestate::SLOTS;
        self.slot_moved = Some(self.slot);
        let age = std::fs::metadata(self.states.slot(self.slot))
            .and_then(|m| m.modified())
            .ok()
            .map(|saved| crate::text::ago(saved.elapsed().map_or(0, |d| d.as_secs())));
        let slot = self.slot;
        self.shared.say_about_states(Ok(match age {
            Some(age) => format!("Slot {slot}: saved {age}"),
            None => format!("Slot {slot}: empty"),
        }));
    }

    /// A slot moved to since the last time, to remember for the game.
    pub fn take_slot_moved(&mut self) -> Option<u8> {
        self.slot_moved.take()
    }

    /// What's come of the game's save states since the last time: something to say, or something
    /// that went wrong.
    pub fn state_news(&self) -> Vec<Result<String, String>> {
        self.shared.take_state_news()
    }

    /// Takes the mouse: the left button touches the game's screen where it's pressed, and goes on
    /// touching where it's dragged until it's let go. While the animation plays, a click skips it.
    pub fn on_mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => match &mut self.intro {
                Some(intro) => intro.skip(),
                None => self.touch = self.touched(m.column, m.row, false),
            },
            // Dragged off the picture, it touches the nearest edge, as a stylus would stop there.
            MouseEventKind::Drag(MouseButton::Left) if self.touch.is_some() => {
                self.touch = self.touched(m.column, m.row, true);
            }
            MouseEventKind::Up(MouseButton::Left) => self.touch = None,
            _ => {}
        }
    }

    /// The pixel of the core's frame under the mouse at `column`, `row` (pixels or cells, as the
    /// terminal tells them), or None off the picture, unless it's to be held to its edge.
    fn touched(&self, column: u16, row: u16, to_edge: bool) -> Option<[u16; 2]> {
        let (rect, (w, h)) = self.cells?;
        let (cell_w, cell_h) = kitty::cell_size(&terminal::window_size().ok()?);
        let at = |n: u16, cell: u32| match self.pixel_mouse {
            true => f64::from(n),
            false => (f64::from(n) + 0.5) * f64::from(cell),
        };
        let x = (at(column, cell_w) - f64::from(u32::from(rect.x) * cell_w)) / f64::from(w);
        let y = (at(row, cell_h) - f64::from(u32::from(rect.y) * cell_h)) / f64::from(h);
        touch_at(self.shown, [x, y], to_edge)
    }

    /// Reads the controllers and hands what's held to the core, with the mouse's touch as player
    /// 1's. False when Select+Start has been held long enough to stop the game. While the
    /// animation plays, a button skips it.
    pub fn read_pads(&mut self) -> bool {
        let Some(mut players) = self.controls.input() else { return false };
        players[0].touch = self.touch;
        let chords = self.controls.take_chords();
        match &mut self.intro {
            Some(intro) if players.iter().any(|p| p.buttons != 0) => intro.skip(),
            Some(_) => {}
            None => {
                self.shared.set_input(players);
                for chord in chords {
                    self.chord(chord);
                }
            }
        }
        true
    }

    /// Why the game ended by itself, once, if it has: a program captured from gamescope can.
    pub fn ended(&self) -> Option<String> {
        self.shared.take_ended()
    }

    /// Whether it's a program captured from gamescope rather than a core.
    pub fn captured(&self) -> bool {
        self.captured
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
            match event {
                cheevos::Event::Progress { title, progress, badge_url } => {
                    let url = badge_url.as_deref().filter(|_| self.look.badges);
                    match &mut self.tracker {
                        Some(tracker) => tracker.update(title, progress, url),
                        None => self.tracker = Some(notice::Tracker::new(title, progress, url)),
                    }
                    continue;
                }
                cheevos::Event::Presence(message) => {
                    self.presence = message;
                    continue;
                }
                cheevos::Event::ProgressHidden => {
                    self.tracker = None;
                    continue;
                }
                cheevos::Event::ChallengeShown { id, title, badge_url } => {
                    if !self.challenges.iter().any(|c| c.id == id) {
                        let badge = badge_url.filter(|_| self.look.badges).map(|u| notice::BadgeImage::fetch(&u));
                        self.challenges.push(notice::Challenge { id, title, badge });
                    }
                    continue;
                }
                cheevos::Event::ChallengeHidden { id } => {
                    self.challenges.retain(|c| c.id != id);
                    continue;
                }
                _ => {}
            }
            match Toast::for_event(event, self.look.badges) {
                Ok(toast) => self.toasts.push_back(toast),
                Err(say) => status = say.or(status),
            }
        }
        status
    }

    /// What the game says the player is doing, for the line under its picture; empty if it
    /// doesn't say.
    pub fn presence(&self) -> &str {
        &self.presence
    }

    /// Where to show progress towards an achievement over the game's `picture` now, if there is
    /// any, with its badge sent for the cells it goes in.
    pub fn place_tracker(&mut self, picture: Rect) -> Option<notice::Layout> {
        if self.tracker.as_ref().is_some_and(notice::Tracker::stale) {
            self.tracker = None;
        }
        if self.tracker.is_none() && self.badge_cells[notice::SLOT_TRACKER].is_none() {
            return None;
        }
        let cell = terminal::window_size().map_or((8, 16), |size| kitty::cell_size(&size));
        let layout = self.tracker.as_ref().filter(|_| self.intro.is_none()).and_then(|t| t.layout(picture, cell));
        let badge = self.tracker.as_ref().and_then(notice::Tracker::badge);
        self.sync_badges(notice::SLOT_TRACKER..=notice::SLOT_TRACKER, layout.as_ref(), cell, |_| badge.clone());
        layout
    }

    /// Where to list the achievements whose challenges are under way, over the game's `picture`,
    /// if there are any and there's room beside the progress box, with their badges sent.
    pub fn place_challenges(&mut self, picture: Rect) -> Option<notice::Layout> {
        let sent = self.badge_cells[notice::SLOT_CHALLENGE..].iter().any(Option::is_some);
        if self.challenges.is_empty() && !sent {
            return None;
        }
        let cell = terminal::window_size().map_or((8, 16), |size| kitty::cell_size(&size));
        let layout = notice::challenges_layout(&self.challenges, picture, cell).filter(|layout| {
            self.intro.is_none()
                && self
                    .tracker
                    .as_ref()
                    .and_then(|t| t.layout(picture, cell))
                    .is_none_or(|t| !t.area.intersects(layout.area))
        });
        let badges: Vec<_> = self
            .challenges
            .iter()
            .take(notice::MAX_CHALLENGES)
            .map(|c| c.badge.as_ref().and_then(|b| b.image()))
            .collect();
        let slots = notice::SLOT_CHALLENGE..=notice::SLOT_CHALLENGE + notice::MAX_CHALLENGES - 1;
        self.sync_badges(slots, layout.as_ref(), cell, |slot| badges.get(slot - notice::SLOT_CHALLENGE)?.clone());
        layout
    }

    /// Makes the badge images in `slots` match `layout`: each one it places is sent for its cells
    /// (once, `badge` giving the picture), and the rest are taken away.
    fn sync_badges(
        &mut self,
        slots: std::ops::RangeInclusive<usize>,
        layout: Option<&notice::Layout>,
        cell: (u32, u32),
        badge: impl Fn(usize) -> Option<image::RgbaImage>,
    ) {
        for slot in slots {
            let want = layout.and_then(|l| l.badges.iter().find(|(_, s)| *s == slot)).map(|(cells, _)| *cells);
            match want {
                Some(cells) if self.badge_cells[slot] != Some(cells) => {
                    let Some(image) = badge(slot) else { continue };
                    let size = (u32::from(cells.width) * cell.0, u32::from(cells.height) * cell.1);
                    let pixels = notice::badge_pixels(&image, size);
                    match self.send(badge_image(slot), cells, &pixels, size, 32) {
                        Ok(_) => self.badge_cells[slot] = Some(cells),
                        Err(e) => crate::emu::cheevos::note(&format!("can't show a badge: {e:#}")),
                    }
                }
                Some(_) => {}
                None if self.badge_cells[slot].is_some() => {
                    let _ = write!(std::io::stdout(), "{}", kitty::delete(badge_image(slot)));
                    self.badge_cells[slot] = None;
                }
                None => {}
            }
        }
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
        if let Some(&(cells, _)) = layout.badges.first()
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
        if let Some(thread) = self.thread.take()
            && !self.shared.stop_and_wait(thread)
        {
            return Some(crate::emu::runner::STUCK.to_string());
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

    /// The frame with its black edges cropped off, if they're to be, keeping which part shows.
    fn cropped(&mut self, frame: Frame) -> Frame {
        let whole = [0, 0, frame.width, frame.height];
        let Some(crop) = &mut self.crop else {
            self.shown = whole;
            return frame;
        };
        let frame = crop.apply(frame);
        self.shown = crop.cropped_to().unwrap_or(whole);
        frame
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

/// The pixel of the core's frame at `[x, y]` across the picture (0 to 1 each way), which shows the
/// `shown` part of the frame. None off the picture, unless it's held `to_edge`.
fn touch_at(shown: [u32; 4], [x, y]: [f64; 2], to_edge: bool) -> Option<[u16; 2]> {
    let [l, t, r, b] = shown;
    let on = (0.0..1.0).contains(&x) && (0.0..1.0).contains(&y);
    if r <= l || b <= t || !(on || to_edge) {
        return None;
    }
    let pixel = |at: f64, from: u32, to: u32| {
        let across = f64::from(to - from);
        (f64::from(from) + (at.clamp(0.0, 1.0) * across).min(across - 1.0)) as u16
    };
    Some([pixel(x, l, r), pixel(y, t, b)])
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
        if let Some(thread) = self.thread.take() {
            self.shared.stop_and_wait(thread);
        }
        let mut out = std::io::stdout();
        let _ = write!(out, "{}{}", kitty::delete(IMAGE), kitty::delete(INTRO_IMAGE));
        for slot in 0..notice::SLOTS {
            let _ = write!(out, "{}", kitty::delete(badge_image(slot)));
        }
        let _ = write!(out, "{MOUSE_OFF}");
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
    fn the_mouse_touches_the_pixel_it_points_at() {
        // A DS's two screens, 256×384, the bottom one touched.
        let whole = [0, 0, 256, 384];
        assert_eq!(touch_at(whole, [0.5, 0.75], false), Some([128, 288]));
        assert_eq!(touch_at(whole, [0.0, 0.0], false), Some([0, 0]));
        assert_eq!(touch_at(whole, [0.9999, 0.9999], false), Some([255, 383]));
        assert_eq!(touch_at(whole, [1.0, 0.5], false), None, "just off the right");
        assert_eq!(touch_at(whole, [-0.1, 0.5], false), None);
        assert_eq!(touch_at(whole, [1.3, -2.0], true), Some([255, 0]), "dragged off, held to the edge");
        // Black edges cropped off: the picture is only the middle of the frame.
        assert_eq!(touch_at([32, 16, 288, 224], [0.0, 0.5], false), Some([32, 120]));
        assert_eq!(touch_at([0; 4], [0.5, 0.5], false), None, "no frame yet");
    }

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

//! Plays romi's launch animation (`src/emu/wireframe.rs`) on its own, over the whole terminal, as
//! a prototype to try its options on: a wireframe console builds itself out of lines, then the
//! camera dives into its screen and lands on exactly the cells live play draws the game in.
//!
//! It draws through the same Kitty graphics path as `romi run --live`: pixels in shared memory, one
//! image replaced every frame. That shows how the animation looks and how fast it goes in a real
//! terminal. There's no core here, so a test picture stands in for the game.
//!
//! ```text
//! cargo run --release --example wireframe -- --style vortex "Moonlit Harbor (USA).nes"
//! ```
//!
//! The seed comes from the game's file name, and gives the same build as the browser prototype for
//! the same seed. Keys: any key skips ahead, r replays, n tries a new seed, q or Esc quits.
//!
//! Game Boy games get the Game Boy, as in romi, and everything else the hand-drawn NES and TV.
//! `--model` reads another, such as one made from a downloaded model by the wireframe-import
//! example.

#[allow(dead_code)]
#[path = "../../src/emu/kitty.rs"]
mod kitty;
#[path = "../../src/emu/wireframe.rs"]
mod wireframe;

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use clap::Parser;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen, WindowSize};
use ratatui::crossterm::{cursor, execute};
use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};
use wireframe::{FADE, Look, Model, Order, Painter, Picture, Plan, Rng, Settings, Style, View};

/// The id of the one image this keeps replacing, and of its one placement.
const IMAGE: u32 = 1;
/// Shared memory objects kept around for a terminal that hasn't read them yet.
const PENDING: usize = 3;
const FPS: f64 = 60.0;
/// The stand-in game's frame size: an NES's.
const PIC_W: usize = 256;
const PIC_H: usize = 240;

#[derive(Parser)]
#[command(about = "Plays romi's wireframe launch animation in the terminal")]
struct Args {
    /// The game file whose name seeds the animation, and whose extension picks the model.
    #[arg(default_value = "Moonlit Harbor (USA).nes")]
    game: String,
    /// Use this seed instead of hashing the file name.
    #[arg(long)]
    seed: Option<u32>,
    /// How lines enter (picked by the seed if not given).
    #[arg(long, value_enum)]
    style: Option<Style>,
    /// Which lines go first (picked by the seed if not given).
    #[arg(long, value_enum)]
    order: Option<Order>,
    /// Seconds the lines take to build.
    #[arg(long, default_value_t = Settings::default().build)]
    build: f64,
    /// How spread out the lines' starts are, from 0 (all at once) to 1 (one after another).
    #[arg(long, default_value_t = Settings::default().stagger)]
    stagger: f64,
    /// Seconds the zoom into the screen takes.
    #[arg(long, default_value_t = Settings::default().zoom)]
    zoom: f64,
    /// How far before the build ends the zoom starts, as a fraction of the build.
    #[arg(long, default_value_t = Settings::default().overlap)]
    overlap: f64,
    /// The camera's vertical field of view, in degrees.
    #[arg(long, default_value_t = Settings::default().fov)]
    fov: f64,
    /// Draw at 1/N of the terminal's pixels and let the terminal scale the picture up.
    #[arg(long, default_value_t = 2)]
    shrink: u32,
    /// The colors a game's beam is picked from: `terminal` for the terminal's own palette, or
    /// colors like 9fe3ff,ffb547.
    #[arg(long, default_value = "terminal")]
    colors: String,
    /// A model made by the wireframe-import example, instead of the built-in ones.
    #[arg(long)]
    model: Option<PathBuf>,
}

impl Args {
    fn settings(&self) -> Settings {
        Settings {
            style: self.style,
            order: self.order,
            build: self.build,
            stagger: self.stagger,
            zoom: self.zoom,
            overlap: self.overlap,
            fov: self.fov,
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let model = match &args.model {
        Some(path) => Model::load(&fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?)?,
        None => Model::load(built_in(&args.game))?,
    };
    let listed = match args.colors.as_str() {
        "terminal" => None,
        list => Some(wireframe::parse_colors(list.split(','))?),
    };
    let mut seed = args.seed.unwrap_or_else(|| wireframe::fnv1a(&args.game));
    let mut screen = Screen::enter()?;
    let look = Look::new(listed, wireframe::ask_colors());
    let mut painter = Painter::default();
    loop {
        match play(&args, &model, seed, &look, &mut screen, &mut painter)? {
            Next::Quit => return Ok(()),
            Next::Replay => {}
            Next::NewSeed => seed = wireframe::fnv1a(&format!("{:?}", SystemTime::now())),
        }
        screen.clear()?;
    }
}

/// The model for a game, by its file's extension: romi's for its system, or the NES and TV.
fn built_in(game: &str) -> &'static str {
    let ext = std::path::Path::new(game).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    wireframe::for_system(&ext).unwrap_or(include_str!("../../src/emu/wireframes/nes.json"))
}

enum Next {
    Quit,
    Replay,
    NewSeed,
}

/// Plays the animation once, then shows the stand-in game until a key says what's next.
fn play(
    args: &Args,
    model: &Model,
    seed: u32,
    look: &Look,
    screen: &mut Screen,
    painter: &mut Painter,
) -> Result<Next> {
    let plan = Plan::new(model, &args.settings(), seed.max(1));
    let beam = look.beam(seed);
    let clock = Instant::now();
    let mut skip = 0.0;
    let mut stats = Stats::default();
    loop {
        let frame_start = Instant::now();
        let window = Window::new(&terminal::window_size()?, args.shrink, model.aspect);
        let view = &window.view;
        let t = clock.elapsed().as_secs_f64() + skip;
        if t < plan.zoom_end + FADE {
            let began = Instant::now();
            let picture = (t > plan.game_in).then(|| test_picture(t - plan.game_in));
            let game = picture.as_deref().map(|rgb| Picture { rgb, w: PIC_W, h: PIC_H });
            let pixels = painter.paint(model, &plan, view, t, game, beam);
            let whole = Cells { col: 0, row: 0, cols: window.cols, rows: window.rows };
            screen.show(pixels, view.w, view.h, whole)?;
            stats.add(began.elapsed());
        } else {
            screen.show(&test_picture(t - plan.game_in), PIC_W, PIC_H, window.cells)?;
            let line = format!(
                " seed {seed} · {} · {} · #{:02x}{:02x}{:02x} {} · {} frames, draw {:.1} ms (worst {:.1}) at {}×{} · landed within {:.1} px · r replay n new seed q quit · {}",
                plan.style.name(),
                plan.order.name(),
                beam.color[0],
                beam.color[1],
                beam.color[2],
                look.source,
                stats.frames,
                stats.average_ms(),
                stats.worst.as_secs_f64() * 1000.0,
                view.w,
                view.h,
                plan.landing_error(model, view) * f64::from(args.shrink.max(1)),
                args.game,
            );
            screen.status(&window, &line)?;
        }

        let deadline = frame_start + Duration::from_secs_f64(1.0 / FPS);
        while event::poll(deadline.saturating_duration_since(Instant::now()))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(Next::Quit),
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(Next::Quit),
                    KeyCode::Char('r') => return Ok(Next::Replay),
                    KeyCode::Char('n') => return Ok(Next::NewSeed),
                    _ => skip = f64::max(skip, plan.zoom_end + FADE - clock.elapsed().as_secs_f64()),
                },
                Event::Resize(..) => screen.clear()?,
                _ => {}
            }
        }
    }
}

#[derive(Default)]
struct Stats {
    frames: u32,
    total: Duration,
    worst: Duration,
}

impl Stats {
    fn add(&mut self, took: Duration) {
        self.frames += 1;
        self.total += took;
        self.worst = self.worst.max(took);
    }

    fn average_ms(&self) -> f64 {
        self.total.as_secs_f64() * 1000.0 / f64::from(self.frames.max(1))
    }
}

// ---------- the terminal ----------

/// The terminal in the state the animation needs, put back however it ends.
struct Screen {
    sent: u64,
    pending: VecDeque<String>,
}

impl Screen {
    fn enter() -> Result<Screen> {
        terminal::enable_raw_mode()?;
        let screen = Screen { sent: 0, pending: VecDeque::new() };
        execute!(std::io::stdout(), EnterAlternateScreen, cursor::Hide, terminal::DisableLineWrap)?;
        Ok(screen)
    }

    fn clear(&mut self) -> Result<()> {
        let mut out = std::io::stdout();
        write!(out, "{}\x1b[2J", kitty::delete(IMAGE))?;
        out.flush()?;
        Ok(())
    }

    /// Replaces the picture with `pixels` (RGB, or RGBA where the terminal shows through, `w`×`h`),
    /// stretched over `cells`.
    fn show(&mut self, pixels: &[u8], w: usize, h: usize, cells: Cells) -> Result<()> {
        let format = if pixels.len() == w * h * 4 { 32 } else { 24 };
        self.sent += 1;
        let name = format!("/romi-wireframe-{}-{}", std::process::id(), self.sent);
        kitty::write_shm(&name, pixels)?;
        let mut out = std::io::stdout().lock();
        write!(
            out,
            "\x1b[{};{}H\x1b_Ga=T,i={IMAGE},p={IMAGE},q=2,C=1,f={format},s={w},v={h},c={},r={},t=s,S={};{}\x1b\\",
            cells.row + 1,
            cells.col + 1,
            cells.cols,
            cells.rows,
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

    /// Writes `line` dimmed on the bottom row, like live play's status line.
    fn status(&mut self, window: &Window, line: &str) -> Result<()> {
        let line: String = line.chars().take(window.cols as usize).collect();
        let mut out = std::io::stdout().lock();
        write!(out, "\x1b[{};1H\x1b[2K\x1b[2m{line}\x1b[0m", window.rows)?;
        out.flush()?;
        Ok(())
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let mut out = std::io::stdout();
        let _ = write!(out, "{}", kitty::delete(IMAGE));
        let _ = execute!(out, terminal::EnableLineWrap, cursor::Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
        for name in &self.pending {
            kitty::remove_shm(name);
        }
    }
}

#[derive(Clone, Copy)]
struct Cells {
    col: u32,
    row: u32,
    cols: u32,
    rows: u32,
}

/// The cells live play stretches a frame over with the status line showing: `Scale::Terminal` in
/// `lay_out()` in src/emu/live.rs. Kept in step by hand, since examples can't reach romi's modules.
fn lay_out(cols: u32, rows: u32, (cell_w, cell_h): (u32, u32), (width, height): (f64, f64)) -> Cells {
    let usable_rows = rows.saturating_sub(1).max(1);
    let (area_w, area_h) = (cols * cell_w, usable_rows * cell_h);
    let fit = (f64::from(area_w) / width).min(f64::from(area_h) / height);
    let c = ((width * fit / f64::from(cell_w)).round() as u32).clamp(1, cols);
    let r = ((height * fit / f64::from(cell_h)).round() as u32).clamp(1, usable_rows);
    Cells { col: (cols - c) / 2, row: (usable_rows - r) / 2, cols: c, rows: r }
}

/// The terminal's size, and the picture drawn to cover it.
struct Window {
    cols: u32,
    rows: u32,
    /// Where live play would put the game.
    cells: Cells,
    view: View,
}

impl Window {
    fn new(size: &WindowSize, shrink: u32, aspect: (f64, f64)) -> Window {
        let (cell_w, cell_h) = kitty::cell_size(size);
        let (cols, rows) = (u32::from(size.columns.max(1)), u32::from(size.rows.max(1)));
        let cells = lay_out(cols, rows, (cell_w, cell_h), aspect);
        let s = f64::from(shrink.max(1));
        let (cw, ch) = (f64::from(cell_w) / s, f64::from(cell_h) / s);
        let target = [
            f64::from(cells.col) * cw,
            f64::from(cells.row) * ch,
            f64::from(cells.cols) * cw,
            f64::from(cells.rows) * ch,
        ];
        let (w, h) = ((f64::from(cols) * cw).round() as usize, (f64::from(rows) * ch).round() as usize);
        Window { cols, rows, cells, view: View::new(w, h, target, cw) }
    }
}

// ---------- the stand-in game ----------

const KEEPER: [&str; 11] = [
    "..hhhh..", ".hhhhhh.", "..ffff..", "..fkfk..", "..ffff..", ".cccccc.", "cccccccl", "cccccc.L", ".cccc...",
    ".b..b...", ".b..b...",
];

/// A night harbour with a lighthouse keeper, `t` seconds in, as a 256×240 RGB frame.
fn test_picture(t: f64) -> Vec<u8> {
    let t = t.max(0.0);
    let mut p = vec![0; PIC_W * PIC_H * 3];
    let mut fill = |x: i32, y: i32, w: i32, h: i32, colour: u32| {
        let rgb = [(colour >> 16) as u8, (colour >> 8) as u8, colour as u8];
        for py in y.max(0)..(y + h).min(PIC_H as i32) {
            for px in x.max(0)..(x + w).min(PIC_W as i32) {
                p[(py as usize * PIC_W + px as usize) * 3..][..3].copy_from_slice(&rgb);
            }
        }
    };
    for (i, colour) in [0x0b0d33, 0x11154d, 0x1a1d66, 0x272683, 0x3a2f97].into_iter().enumerate() {
        fill(0, i as i32 * 24, 256, 24, colour);
    }
    let mut stars = Rng(99);
    for _ in 0..44 {
        let (x, y, phase) = ((stars.next() * 256.0) as i32, (stars.next() * 104.0) as i32, stars.next() * 6.0);
        if (t * 2.0 + phase).sin() > -0.3 {
            fill(x, y, 1, 1, 0xc8d0ff);
        }
    }
    for dy in -14..=14 {
        let w = f64::from(196 - dy * dy).sqrt().round() as i32;
        fill(196 - w, 42 + dy, w * 2, 1, 0xf8e6a8);
    }
    fill(190, 36, 4, 3, 0xe0c880);
    fill(201, 47, 3, 3, 0xe0c880);
    for x in (0..256).step_by(8) {
        let h = 10 + (8.0 * (f64::from(x) * 0.05).sin() + 5.0 * (f64::from(x) * 0.13).sin()).round() as i32;
        fill(x, 120 - h, 8, h + 20, 0x160f3a);
    }
    fill(0, 140, 256, 56, 0x0a1f5a);
    for y in (144..196).step_by(6) {
        let off = (t * 12.0 + f64::from(y) * 3.0).floor() as i32 % 32;
        for x in (-32 + off..256).step_by(32) {
            fill(x, y, 12, 1, 0x16357e);
        }
    }
    for y in (144..194).step_by(4) {
        let w = (8 - (y - 144) / 10 + (t * 6.0 + f64::from(y)).floor() as i32 % 3).max(1);
        fill(196 - w, y, w * 2, 1, 0xf8e6a8);
    }
    for i in 0..7 {
        fill(28, 78 + i * 9, 14, 9, if i % 2 == 1 { 0xd83030 } else { 0xe8e8f0 });
    }
    fill(20, 140, 30, 4, 0x241a3a);
    fill(26, 68, 18, 10, 0x2a2a40);
    fill(30, 70, 10, 6, if (t * 5.0).sin() > 0.0 { 0xffe680 } else { 0xc8a040 });
    fill(0, 196, 256, 44, 0x5a3418);
    let scroll = (t * 24.0).floor() as i32;
    for x in (-(scroll % 16)..256).step_by(16) {
        fill(x, 196, 2, 44, 0x3a200c);
    }
    fill(0, 196, 256, 2, 0x7a4a24);
    for x in (-(scroll % 64)..256).step_by(64) {
        fill(x + 8, 186, 6, 12, 0x2a1608);
    }
    let (kx, ky) = (118, 174 - ((t * 6.0).sin().abs() * 2.0).floor() as i32);
    for (y, row) in KEEPER.iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            let colour = match ch {
                'h' => 0x1c6e7a,
                'f' => 0xf0c090,
                'k' => 0x101018,
                'c' => 0xe0a828,
                'l' => 0xffe680,
                'L' => 0xffd040,
                'b' => 0x203040,
                _ => continue,
            };
            fill(kx + x as i32 * 2, ky + y as i32 * 2, 2, 2, colour);
        }
    }
    p
}

//! `romi run --live`: plays a game in the whole terminal, drawn with the Kitty graphics protocol.
//!
//! This is a spike for built-in emulation, to find out how fast frames can reach the terminal and
//! how they look there. The ways of sending a frame can be switched while playing: F1 changes the
//! transfer (shared memory, a temp file, or the pixels themselves, plain or compressed), F2 changes
//! who scales the picture (the terminal, or romi by a whole number), and the bottom line shows the
//! frame rates and how long each frame took to send.
//!
//! The core runs on its own thread, paced by its sound (or the clock, without sound), and the
//! terminal shows the newest frame whenever it can, so a slow terminal drops frames rather than
//! slowing the game. F4 mutes the sound.

use crate::audio;
use crate::retro::{Core, Frame, button};
use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use gilrs::{Axis, Button, Gilrs};
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::crossterm::{cursor, execute};
use std::collections::{HashSet, VecDeque};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// The id of the one image romi keeps replacing, and of its one placement.
const IMAGE: u32 = 1;
/// Base64 bytes per escape sequence when sending pixels directly (the protocol's limit).
const CHUNK: usize = 4096;
/// Holding Select and Start together this long quits.
const QUIT_HOLD: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, PartialEq)]
enum Transfer {
    /// A POSIX shared memory object the terminal reads and unlinks.
    Shm,
    /// A file in /tmp the terminal reads and deletes.
    File,
    /// The pixels in the escape sequence, base64 encoded.
    Direct,
    /// The same, zlib compressed first.
    Zlib,
}

impl Transfer {
    fn next(self) -> Transfer {
        match self {
            Transfer::Shm => Transfer::File,
            Transfer::File => Transfer::Direct,
            Transfer::Direct => Transfer::Zlib,
            Transfer::Zlib => Transfer::Shm,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Transfer::Shm => "shared memory",
            Transfer::File => "temp file",
            Transfer::Direct => "direct",
            Transfer::Zlib => "direct+zlib",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Scale {
    /// Send the frame as it is and let the terminal stretch it over the cells.
    Terminal,
    /// Scale it up by a whole number in romi, pixel for pixel, and send that.
    Integer,
}

/// What the core thread and the terminal share.
pub struct Shared {
    /// The newest frame and how many frames have run.
    pub latest: Mutex<(u64, Frame)>,
    ready: Condvar,
    pub buttons: AtomicU16,
    quit: AtomicBool,
    /// How loud the sound plays (1 is as the core made it).
    volume: f32,
    pub muted: AtomicBool,
    /// Milliseconds of sound queued for the device, or `NO_SOUND`.
    queued_ms: AtomicU32,
    /// Why there's no sound, once the core thread has found out.
    pub sound_error: Mutex<Option<String>>,
}

const NO_SOUND: u32 = u32::MAX;

impl Shared {
    /// Starts `core` on its own thread, running until `stop`, with its sound `volume` loud.
    pub fn start(core: Core, volume: f32) -> Result<(Arc<Shared>, std::thread::JoinHandle<()>)> {
        let shared = Arc::new(Shared {
            latest: Mutex::new((0, Frame::default())),
            ready: Condvar::new(),
            buttons: AtomicU16::new(0),
            quit: AtomicBool::new(false),
            volume,
            muted: AtomicBool::new(false),
            queued_ms: AtomicU32::new(NO_SOUND),
            sound_error: Mutex::new(None),
        });
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new().name("core".into()).spawn(move || run_core(core, &shared))?
        };
        Ok((shared, thread))
    }

    pub fn stop(&self) {
        self.quit.store(true, Ordering::SeqCst);
    }

    /// The newest frame, if it's newer than frame number `seen`, and its number.
    pub fn take_newer(&self, seen: u64) -> Option<(u64, Frame)> {
        let mut latest = self.latest.lock().unwrap_or_else(|e| e.into_inner());
        (latest.0 != seen).then(|| (latest.0, std::mem::take(&mut latest.1)))
    }
}

pub fn play(core: Core, name: &str, volume: f32) -> Result<()> {
    let fps = core.fps;
    let core_name = core.name.clone();
    let (shared, emu) = Shared::start(core, volume)?;
    let result = Screen::enter().and_then(|_screen| View::new(name, &core_name, fps).run(&shared));
    shared.stop();
    let _ = emu.join();
    result
}

/// Runs the core until told to quit, playing its sound, at the pace its sound sets (or at its
/// frame rate, without sound).
fn run_core(mut core: Core, shared: &Shared) {
    // The stream lives on this thread, so it's opened here.
    let mut sound = audio::Output::open(core.sample_rate, core.fps)
        .inspect_err(|e| *shared.sound_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!("{e:#}")))
        .ok();
    let mut samples = Vec::new();
    let period = Duration::from_secs_f64(1.0 / core.fps);
    let mut next = Instant::now();
    while !shared.quit.load(Ordering::Relaxed) {
        core.run_frame(shared.buttons.load(Ordering::Relaxed));
        let wait = match &mut sound {
            Some(sound) => {
                samples.clear();
                core.audio(&mut samples);
                let volume = if shared.muted.load(Ordering::Relaxed) { 0.0 } else { shared.volume };
                let lasts = sound.push(&samples, volume);
                shared.queued_ms.store(sound.ahead().as_millis() as u32, Ordering::Relaxed);
                lasts
            }
            None => period,
        };
        let frame = core.frame();
        {
            let mut latest = shared.latest.lock().unwrap_or_else(|e| e.into_inner());
            latest.0 += 1;
            latest.1 = frame;
        }
        shared.ready.notify_one();
        next += wait;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > period * 4 {
            // Too far behind to catch up without a burst of fast frames; carry on from here.
            next = now;
        }
    }
}

/// The terminal in the state live play needs, put back however play ends.
struct Screen;

impl Screen {
    fn enter() -> Result<Screen> {
        terminal::enable_raw_mode()?;
        let screen = Screen;
        // Key releases are needed to know when a button is let go.
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES | KeyboardEnhancementFlags::REPORT_EVENT_TYPES;
        execute!(std::io::stdout(), EnterAlternateScreen, cursor::Hide, PushKeyboardEnhancementFlags(flags))?;
        // Without line wrap, writing in the last row can never scroll the picture away.
        execute!(std::io::stdout(), terminal::DisableLineWrap)?;
        Ok(screen)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let mut out = std::io::stdout();
        let _ = write!(out, "\x1b_Ga=d,d=I,i={IMAGE},q=2\x1b\\");
        let _ =
            execute!(out, terminal::EnableLineWrap, PopKeyboardEnhancementFlags, cursor::Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

/// Where the picture goes, in cells and pixels.
#[derive(Default)]
struct Layout {
    /// The cell the picture's top left corner is in (0-based).
    col: u16,
    row: u16,
    /// Pixel offset within that cell, for centering a picture romi scaled itself.
    x: u32,
    y: u32,
    /// Cells the terminal stretches the picture over (Scale::Terminal only).
    cols: u16,
    rows: u16,
    /// Whole-number scale (Scale::Integer only).
    factor: u32,
    /// The row for the status line.
    status_row: u16,
}

struct View {
    name: String,
    core_name: String,
    fps: f64,
    transfer: Transfer,
    scale: Scale,
    show_status: bool,
    layout: Option<Layout>,
    /// Size of the frame the layout was worked out for.
    laid_out_for: (u32, u32),
    gilrs: Option<Gilrs>,
    keys: HashSet<KeyCode>,
    quit_held_since: Option<Instant>,
    /// Numbers the shared memory objects and temp files.
    sent: u64,
    /// Shared memory objects and temp files sent recently, to spot ones the terminal never read.
    pending: VecDeque<Handoff>,
    unread: u64,
    stats: Stats,
}

/// Counted over about a second, then shown on the status line.
struct Stats {
    since: Instant,
    core_frames_at_start: u64,
    shown: u64,
    bytes: u64,
    send_time: Duration,
    worst_send: Duration,
}

impl View {
    fn new(name: &str, core_name: &str, fps: f64) -> View {
        View {
            name: name.to_string(),
            core_name: core_name.to_string(),
            fps,
            transfer: Transfer::Shm,
            scale: Scale::Terminal,
            show_status: true,
            layout: None,
            laid_out_for: (0, 0),
            gilrs: Gilrs::new().ok(),
            keys: HashSet::new(),
            quit_held_since: None,
            sent: 0,
            pending: VecDeque::new(),
            unread: 0,
            stats: Stats {
                since: Instant::now(),
                core_frames_at_start: 0,
                shown: 0,
                bytes: 0,
                send_time: Duration::ZERO,
                worst_send: Duration::ZERO,
            },
        }
    }

    fn run(&mut self, shared: &Shared) -> Result<()> {
        let mut shown = 0;
        loop {
            let frame = {
                let latest = shared.latest.lock().unwrap_or_else(|e| e.into_inner());
                let (mut latest, _) = shared
                    .ready
                    .wait_timeout_while(latest, Duration::from_millis(8), |l| l.0 == shown)
                    .unwrap_or_else(|e| e.into_inner());
                (latest.0 != shown).then(|| {
                    shown = latest.0;
                    std::mem::take(&mut latest.1)
                })
            };
            if !self.read_input(shared)? {
                return Ok(());
            }
            if let Some(frame) = frame.filter(|f| f.width > 0) {
                self.draw(&frame)?;
            }
            self.update_stats(shared)?;
        }
    }

    /// Reads the keyboard and controllers and hands the held buttons to the core. False to quit.
    fn read_input(&mut self, shared: &Shared) -> Result<bool> {
        while event::poll(Duration::ZERO)? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Release => {
                    self.keys.remove(&k.code);
                }
                Event::Key(k) => match k.code {
                    KeyCode::Esc => return Ok(false),
                    KeyCode::F(1) if k.kind == KeyEventKind::Press => {
                        self.transfer = self.transfer.next();
                        self.stats.reset(shared.latest.lock().unwrap_or_else(|e| e.into_inner()).0);
                    }
                    KeyCode::F(2) if k.kind == KeyEventKind::Press => {
                        self.scale = if self.scale == Scale::Terminal { Scale::Integer } else { Scale::Terminal };
                        self.stats.reset(shared.latest.lock().unwrap_or_else(|e| e.into_inner()).0);
                        self.relayout()?;
                    }
                    KeyCode::F(3) if k.kind == KeyEventKind::Press => {
                        self.show_status = !self.show_status;
                        self.relayout()?;
                    }
                    KeyCode::F(4) if k.kind == KeyEventKind::Press => {
                        shared.muted.fetch_xor(true, Ordering::Relaxed);
                    }
                    code => {
                        self.keys.insert(code);
                    }
                },
                Event::Resize(..) => self.relayout()?,
                _ => {}
            }
        }
        let mut buttons = self.keys.iter().filter_map(|&k| key_button(k)).fold(0, |b, n| b | n);
        if let Some(gilrs) = &mut self.gilrs {
            while gilrs.next_event().is_some() {}
            for (_, pad) in gilrs.gamepads() {
                buttons |= pad_buttons(&pad);
            }
        }
        let quit_combo = button("select").unwrap() | button("start").unwrap();
        if buttons & quit_combo == quit_combo {
            let since = *self.quit_held_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= QUIT_HOLD {
                return Ok(false);
            }
        } else {
            self.quit_held_since = None;
        }
        shared.buttons.store(buttons, Ordering::Relaxed);
        Ok(true)
    }

    /// Forgets the layout, so the next frame works it out again, and clears the screen.
    fn relayout(&mut self) -> Result<()> {
        self.layout = None;
        let mut out = std::io::stdout();
        write!(out, "\x1b_Ga=d,d=I,i={IMAGE},q=2\x1b\\\x1b[2J")?;
        out.flush()?;
        Ok(())
    }

    fn lay_out(&self, width: u32, height: u32) -> Result<Layout> {
        let size = terminal::window_size()?;
        let (cols, rows) = (u32::from(size.columns.max(1)), u32::from(size.rows.max(1)));
        // Terminals that don't report pixels get a guess at a common cell size.
        let (cell_w, cell_h) = match (size.width, size.height) {
            (0, _) | (_, 0) => (8, 16),
            (w, h) => (u32::from(w) / cols, u32::from(h) / rows),
        };
        let usable_rows = if self.show_status { rows.saturating_sub(1).max(1) } else { rows };
        let (area_w, area_h) = (cols * cell_w, usable_rows * cell_h);
        let mut layout = Layout { status_row: (rows - 1) as u16, ..Layout::default() };
        match self.scale {
            Scale::Terminal => {
                let fit = (area_w as f64 / width as f64).min(area_h as f64 / height as f64);
                let c = ((width as f64 * fit / cell_w as f64).round() as u32).clamp(1, cols);
                let r = ((height as f64 * fit / cell_h as f64).round() as u32).clamp(1, usable_rows);
                layout.cols = c as u16;
                layout.rows = r as u16;
                layout.col = ((cols - c) / 2) as u16;
                layout.row = ((usable_rows - r) / 2) as u16;
            }
            Scale::Integer => {
                let factor = (area_w / width).min(area_h / height).max(1);
                let left = area_w.saturating_sub(width * factor) / 2;
                let top = area_h.saturating_sub(height * factor) / 2;
                layout.factor = factor;
                (layout.col, layout.x) = ((left / cell_w) as u16, left % cell_w);
                (layout.row, layout.y) = ((top / cell_h) as u16, top % cell_h);
            }
        }
        Ok(layout)
    }

    fn draw(&mut self, frame: &Frame) -> Result<()> {
        if self.layout.is_none() || self.laid_out_for != (frame.width, frame.height) {
            self.layout = Some(self.lay_out(frame.width, frame.height)?);
            self.laid_out_for = (frame.width, frame.height);
        }
        let layout = self.layout.as_ref().unwrap();
        let scaled;
        let (pixels, w, h) = match self.scale {
            Scale::Terminal => (&frame.rgb, frame.width, frame.height),
            Scale::Integer => {
                scaled = scale_up(frame, layout.factor);
                (&scaled, frame.width * layout.factor, frame.height * layout.factor)
            }
        };
        let mut keys = format!("a=T,i={IMAGE},p={IMAGE},q=2,C=1,f=24,s={w},v={h}");
        match self.scale {
            Scale::Terminal => keys += &format!(",c={},r={}", layout.cols, layout.rows),
            Scale::Integer => keys += &format!(",X={},Y={}", layout.x, layout.y),
        }
        let mut out = format!("\x1b[{};{}H", layout.row + 1, layout.col + 1).into_bytes();
        let pid = std::process::id();
        self.sent += 1;
        let bytes = match self.transfer {
            Transfer::Shm => {
                let name = format!("/romi-{pid}-{}", self.sent);
                crate::kitty::write_shm(&name, pixels)?;
                self.check_read(Handoff::Shm(name.clone()));
                write!(out, "\x1b_G{keys},t=s,S={};{}\x1b\\", pixels.len(), BASE64.encode(&name))?;
                pixels.len()
            }
            Transfer::File => {
                let path = std::env::temp_dir().join(format!("tty-graphics-protocol-romi-{pid}-{}", self.sent));
                std::fs::write(&path, pixels)?;
                let encoded = BASE64.encode(path.as_os_str().as_encoded_bytes());
                self.check_read(Handoff::File(path));
                write!(out, "\x1b_G{keys},t=t,S={};{encoded}\x1b\\", pixels.len())?;
                pixels.len()
            }
            Transfer::Direct => {
                put_chunked(&mut out, &keys, &BASE64.encode(pixels));
                out.len()
            }
            Transfer::Zlib => {
                use flate2::write::ZlibEncoder;
                let mut z = ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
                z.write_all(pixels)?;
                put_chunked(&mut out, &format!("{keys},o=z"), &BASE64.encode(z.finish()?));
                out.len()
            }
        };
        let started = Instant::now();
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(&out)?;
        stdout.flush()?;
        let took = started.elapsed();
        self.stats.shown += 1;
        self.stats.bytes += bytes as u64;
        self.stats.send_time += took;
        self.stats.worst_send = self.stats.worst_send.max(took);
        Ok(())
    }

    /// Remembers a frame handed over in shared memory or a file. The terminal deletes each one it
    /// reads, so one still there a few frames later was never read; it's removed so it doesn't
    /// pile up.
    fn check_read(&mut self, sent: Handoff) {
        self.pending.push_back(sent);
        while self.pending.len() > 30 {
            if self.pending.pop_front().unwrap().remove() {
                self.unread += 1;
            }
        }
    }

    fn update_stats(&mut self, shared: &Shared) -> Result<()> {
        let elapsed = self.stats.since.elapsed();
        if elapsed < Duration::from_secs(1) {
            return Ok(());
        }
        let core_frames = shared.latest.lock().unwrap_or_else(|e| e.into_inner()).0;
        let secs = elapsed.as_secs_f64();
        let s = &self.stats;
        let game_fps = (core_frames - s.core_frames_at_start) as f64 / secs;
        let shown_fps = s.shown as f64 / secs;
        let per_frame = s.bytes.checked_div(s.shown).unwrap_or(0);
        let avg_send = s.send_time.checked_div(s.shown as u32).unwrap_or_default();
        let scale = match (self.scale, &self.layout) {
            (Scale::Integer, Some(l)) => format!("{}x by romi", l.factor),
            (Scale::Integer, None) => "by romi".into(),
            (Scale::Terminal, _) => "by terminal".into(),
        };
        let unread = match self.unread {
            0 => String::new(),
            n => format!(" · {n} unread!"),
        };
        let sound = match shared.queued_ms.load(Ordering::Relaxed) {
            _ if shared.muted.load(Ordering::Relaxed) => "muted".to_string(),
            NO_SOUND => match &*shared.sound_error.lock().unwrap_or_else(|e| e.into_inner()) {
                Some(e) => format!("no sound: {e}"),
                None => "no sound yet".to_string(),
            },
            ms => format!("sound {ms} ms ahead"),
        };
        let line = format!(
            " game {game_fps:.1}/{:.1} fps · shown {shown_fps:.1} fps · {} · scaled {scale} · {} KB/frame · send {:.2} ms (worst {:.1}){unread} · {sound} · F1 transfer F2 scale F3 hide F4 mute · Esc or hold Select+Start to quit · {} · {}",
            self.fps,
            self.transfer.label(),
            per_frame / 1024,
            avg_send.as_secs_f64() * 1000.0,
            s.worst_send.as_secs_f64() * 1000.0,
            self.name,
            self.core_name,
        );
        self.stats.reset(core_frames);
        if self.show_status
            && let Some(layout) = &self.layout
        {
            let width = terminal::size().map_or(80, |(cols, _)| cols as usize);
            let line: String = line.chars().take(width).collect();
            let mut out = std::io::stdout().lock();
            write!(out, "\x1b[{};1H\x1b[2K\x1b[2m{line}\x1b[0m", layout.status_row + 1)?;
            out.flush()?;
        }
        Ok(())
    }
}

impl Stats {
    /// Starts counting again, as of `core_frames` frames run.
    fn reset(&mut self, core_frames: u64) {
        *self = Stats {
            since: Instant::now(),
            core_frames_at_start: core_frames,
            shown: 0,
            bytes: 0,
            send_time: Duration::ZERO,
            worst_send: Duration::ZERO,
        };
    }
}

/// Writes one image in escape sequences of at most `CHUNK` base64 bytes each.
fn put_chunked(out: &mut Vec<u8>, keys: &str, data: &str) {
    let chunks: Vec<&[u8]> = data.as_bytes().chunks(CHUNK).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        match i {
            0 => out.extend_from_slice(format!("\x1b_G{keys},m={more};").as_bytes()),
            _ => out.extend_from_slice(format!("\x1b_Gm={more};").as_bytes()),
        }
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }
}

/// The frame made `factor` times bigger, each pixel a square of copies.
fn scale_up(frame: &Frame, factor: u32) -> Vec<u8> {
    let (w, f) = (frame.width as usize, factor as usize);
    let mut out = Vec::with_capacity(frame.rgb.len() * f * f);
    let mut line = Vec::with_capacity(w * f * 3);
    for row in frame.rgb.chunks_exact(w * 3) {
        line.clear();
        for px in row.as_chunks::<3>().0 {
            for _ in 0..f {
                line.extend_from_slice(px);
            }
        }
        for _ in 0..f {
            out.extend_from_slice(&line);
        }
    }
    out
}

/// A frame left for the terminal to pick up.
enum Handoff {
    Shm(String),
    File(std::path::PathBuf),
}

impl Handoff {
    /// Deletes it; true if it was still there.
    fn remove(&self) -> bool {
        match self {
            Handoff::Shm(name) => crate::kitty::remove_shm(name),
            Handoff::File(path) => std::fs::remove_file(path).is_ok(),
        }
    }
}

impl Drop for View {
    fn drop(&mut self) {
        // The last few may not have been read yet, and nothing will read them now.
        for sent in &self.pending {
            sent.remove();
        }
    }
}

/// RetroArch's default keys: arrows, X and Z for A and B, A and S for L and R, Enter for Start,
/// and Backspace for Select (RetroArch uses right Shift, which terminals don't report alone).
pub fn key_button(code: KeyCode) -> Option<u16> {
    let name = match code {
        KeyCode::Up => "up",
        KeyCode::Down => "down",
        KeyCode::Left => "left",
        KeyCode::Right => "right",
        KeyCode::Char('x' | 'X') => "a",
        KeyCode::Char('z' | 'Z') => "b",
        KeyCode::Char('s' | 'S') => "x",
        KeyCode::Char('a' | 'A') => "y",
        KeyCode::Char('q' | 'Q') => "l",
        KeyCode::Char('w' | 'W') => "r",
        KeyCode::Enter => "start",
        KeyCode::Backspace => "select",
        _ => return None,
    };
    button(name)
}

/// The buttons held on one controller, in libretro's layout: A on the right face button, B on
/// the bottom one, as on a Super Nintendo pad. The left stick works as the d-pad.
pub fn pad_buttons(pad: &gilrs::Gamepad) -> u16 {
    const MAP: [(Button, &str); 14] = [
        (Button::East, "a"),
        (Button::South, "b"),
        (Button::North, "x"),
        (Button::West, "y"),
        (Button::LeftTrigger, "l"),
        (Button::RightTrigger, "r"),
        (Button::LeftTrigger2, "l2"),
        (Button::RightTrigger2, "r2"),
        (Button::Select, "select"),
        (Button::Start, "start"),
        (Button::DPadUp, "up"),
        (Button::DPadDown, "down"),
        (Button::DPadLeft, "left"),
        (Button::DPadRight, "right"),
    ];
    let mut buttons = MAP.iter().filter(|(b, _)| pad.is_pressed(*b)).fold(0, |acc, (_, n)| acc | button(n).unwrap());
    let (x, y) = (pad.value(Axis::LeftStickX), pad.value(Axis::LeftStickY));
    for (held, name) in [(x < -0.5, "left"), (x > 0.5, "right"), (y > 0.5, "up"), (y < -0.5, "down")] {
        if held {
            buttons |= button(name).unwrap();
        }
    }
    buttons
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling_up_repeats_each_pixel_in_a_square() {
        let frame = Frame { width: 2, height: 1, rgb: vec![1, 1, 1, 2, 2, 2] };
        let px = |v: u8| [v, v, v];
        let row = [px(1), px(1), px(2), px(2)].concat();
        assert_eq!(scale_up(&frame, 2), [row.clone(), row].concat());
    }

    #[test]
    fn direct_images_are_split_into_chunks() {
        let mut out = Vec::new();
        put_chunked(&mut out, "a=T", &"A".repeat(CHUNK + 1));
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("\x1b_Ga=T,m=1;AAAA"));
        assert!(text.ends_with("\x1b_Gm=0;A\x1b\\"));
        assert_eq!(text.matches("\x1b_G").count(), 2);
    }
}

//! `romi run --live`: plays a game in the whole terminal, drawn with the Kitty graphics protocol.
//!
//! This is a spike for built-in emulation, to find out how fast frames can reach the terminal and
//! how they look there. The ways of sending a frame can be switched while playing: F1 changes the
//! transfer (shared memory, a temp file, or the pixels themselves, plain or compressed), F2 changes
//! who scales the picture (the terminal, or romi by a whole number), and the bottom line shows the
//! frame rates and how long each frame took to send.
//!
//! The game runs on its own thread (see `runner`), and the terminal shows the newest frame whenever
//! it can, so a slow terminal drops frames rather than slowing the game. F4 mutes the sound.

use crate::emu::controls::Controls;
use crate::emu::kitty;
use crate::emu::libretro::{Core, Frame};
use crate::emu::runner::Shared;
use crate::emu::timing::{self, Recorder};
use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::crossterm::{cursor, execute};
use std::collections::VecDeque;
use std::io::Write;
use std::time::{Duration, Instant};

/// The id of the one image romi keeps replacing, and of its one placement.
const IMAGE: u32 = 1;
/// Base64 bytes per escape sequence when sending pixels directly (the protocol's limit).
const CHUNK: usize = 4096;

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

pub fn play(core: Core, name: &str, volume: f32) -> Result<()> {
    let fps = core.fps;
    let core_name = core.name.clone();
    let (shared, emu) = Shared::start(core, volume, false, None)?;
    let mut view = View::new(name, &core_name, fps);
    let result = Screen::enter().and_then(|_screen| view.run(&shared));
    shared.stop();
    let _ = emu.join();
    if let Some(timing) = view.timing.take() {
        let path = timing.finish(&shared.take_made())?;
        let summary = std::fs::read_to_string(timing::summary_path(&path)).unwrap_or_default();
        eprintln!("{summary}\nFrame timings in {}", path.display());
    }
    result?;
    match shared.take_save_error() {
        Some(e) => anyhow::bail!("the game wasn't saved: {e}"),
        None => Ok(()),
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
        let _ = write!(out, "{}", kitty::delete(IMAGE));
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
    /// Whole-number scale of the height, and the size romi scales the frame to (Scale::Integer
    /// only). The width follows the frame's shape, so pixels meant to be narrow stay narrow.
    factor: u32,
    size: (u32, u32),
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
    /// Size and pixel shape of the frame the layout was worked out for.
    laid_out_for: (u32, u32, u64),
    controls: Controls,
    /// Numbers the shared memory objects and temp files.
    sent: u64,
    /// Shared memory objects and temp files sent recently, to spot ones the terminal never read.
    pending: VecDeque<Handoff>,
    unread: u64,
    stats: Stats,
    /// Frame timings, if `ROMI_FRAME_LOG` asks for them.
    timing: Option<Recorder>,
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
            laid_out_for: (0, 0, 0),
            controls: Controls::new(),
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
            timing: Recorder::start(format!("{name}, {core_name}, in the whole terminal (romi run --live)")),
        }
    }

    fn run(&mut self, shared: &Shared) -> Result<()> {
        let mut shown = 0;
        loop {
            let frame = shared.newer_frame(shown, Duration::from_millis(8)).map(|(n, frame)| {
                shown = n;
                frame
            });
            if !self.read_input(shared)? {
                return Ok(());
            }
            if let Some(frame) = frame.filter(|f| f.width > 0) {
                self.draw(shown, &frame)?;
            }
            self.update_stats(shared)?;
        }
    }

    /// Reads the keyboard and controllers and hands the held buttons to the core. False to quit.
    fn read_input(&mut self, shared: &Shared) -> Result<bool> {
        while event::poll(Duration::ZERO)? {
            match event::read()? {
                Event::Key(k) => match k.code {
                    KeyCode::Esc if k.kind != KeyEventKind::Release => return Ok(false),
                    KeyCode::F(1) if k.kind == KeyEventKind::Press => {
                        self.transfer = self.transfer.next();
                        self.stats.reset(shared.frames_run());
                    }
                    KeyCode::F(2) if k.kind == KeyEventKind::Press => {
                        self.scale = if self.scale == Scale::Terminal { Scale::Integer } else { Scale::Terminal };
                        self.stats.reset(shared.frames_run());
                        self.relayout()?;
                    }
                    KeyCode::F(3) if k.kind == KeyEventKind::Press => {
                        self.show_status = !self.show_status;
                        self.relayout()?;
                    }
                    KeyCode::F(4) if k.kind == KeyEventKind::Press => shared.toggle_mute(),
                    _ => self.controls.key(k),
                },
                Event::Resize(..) => self.relayout()?,
                _ => {}
            }
        }
        let Some(buttons) = self.controls.buttons() else { return Ok(false) };
        shared.set_buttons(buttons);
        Ok(true)
    }

    /// Forgets the layout, so the next frame works it out again, and clears the screen.
    fn relayout(&mut self) -> Result<()> {
        self.layout = None;
        let mut out = std::io::stdout();
        write!(out, "{}\x1b[2J", kitty::delete(IMAGE))?;
        out.flush()?;
        Ok(())
    }

    fn lay_out(&self, frame: &Frame) -> Result<Layout> {
        let (width, height) = frame.display_size();
        let size = terminal::window_size()?;
        let (cols, rows) = (u32::from(size.columns.max(1)), u32::from(size.rows.max(1)));
        let (cell_w, cell_h) = kitty::cell_size(&size);
        let usable_rows = if self.show_status { rows.saturating_sub(1).max(1) } else { rows };
        let (area_w, area_h) = (cols * cell_w, usable_rows * cell_h);
        let mut layout = Layout { status_row: (rows - 1) as u16, ..Layout::default() };
        match self.scale {
            Scale::Terminal => {
                let fit = (area_w as f64 / width).min(area_h as f64 / height);
                let c = ((width * fit / cell_w as f64).round() as u32).clamp(1, cols);
                let r = ((height * fit / cell_h as f64).round() as u32).clamp(1, usable_rows);
                layout.cols = c as u16;
                layout.rows = r as u16;
                layout.col = ((cols - c) / 2) as u16;
                layout.row = ((usable_rows - r) / 2) as u16;
            }
            Scale::Integer => {
                let factor = (area_w as f64 / width).min(area_h as f64 / height).floor().max(1.0);
                let size = ((width * factor).round() as u32, frame.height * factor as u32);
                let left = area_w.saturating_sub(size.0) / 2;
                let top = area_h.saturating_sub(size.1) / 2;
                (layout.factor, layout.size) = (factor as u32, size);
                (layout.col, layout.x) = ((left / cell_w) as u16, left % cell_w);
                (layout.row, layout.y) = ((top / cell_h) as u16, top % cell_h);
            }
        }
        Ok(layout)
    }

    fn draw(&mut self, n: u64, frame: &Frame) -> Result<()> {
        let taken = Instant::now();
        let shape = (frame.width, frame.height, frame.pixel_aspect.to_bits());
        if self.layout.is_none() || self.laid_out_for != shape {
            self.layout = Some(self.lay_out(frame)?);
            self.laid_out_for = shape;
        }
        let layout = self.layout.as_ref().unwrap();
        let scaled;
        let (pixels, w, h) = match self.scale {
            Scale::Terminal => (&frame.rgb, frame.width, frame.height),
            Scale::Integer => {
                let (w, h) = layout.size;
                scaled = frame.resample(w, h);
                (&scaled, w, h)
            }
        };
        let prepared = Instant::now();
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
                crate::emu::kitty::write_shm(&name, pixels)?;
                if let Some(timing) = &mut self.timing {
                    timing.handed_over(n, &name);
                }
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
        if let Some(timing) = &mut self.timing {
            let (size, written) = ((w, h), Instant::now());
            timing.sent(timing::Sent { frame: n, taken, prepared, copied: started, written, size, bytes });
        }
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
            let old = self.pending.pop_front().unwrap();
            if old.remove() {
                self.unread += 1;
                if let (Some(timing), Handoff::Shm(name)) = (&mut self.timing, &old) {
                    timing.never_read(name);
                }
            }
        }
    }

    fn update_stats(&mut self, shared: &Shared) -> Result<()> {
        let elapsed = self.stats.since.elapsed();
        if elapsed < Duration::from_secs(1) {
            return Ok(());
        }
        let core_frames = shared.frames_run();
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
        let sound = match shared.queued_sound() {
            _ if shared.muted() => "muted".to_string(),
            Some(ahead) => format!("sound {} ms ahead", ahead.as_millis()),
            None => match shared.sound_error() {
                Some(e) => format!("no sound: {e}"),
                None => "no sound yet".to_string(),
            },
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

/// A frame left for the terminal to pick up.
enum Handoff {
    Shm(String),
    File(std::path::PathBuf),
}

impl Handoff {
    /// Deletes it; true if it was still there.
    fn remove(&self) -> bool {
        match self {
            Handoff::Shm(name) => crate::emu::kitty::remove_shm(name),
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

#[cfg(test)]
mod tests {
    use super::*;

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

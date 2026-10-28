//! A game played inside the TUI, in the middle panel. The game runs on its own thread (see
//! `runner`), and the picture is a Kitty image behind Unicode placeholder cells: ratatui
//! lays those out like text, and each new frame only replaces the image behind them.
//!
//! romi scales each frame to the exact pixel size of its cells itself, nearest neighbour, because
//! a terminal scaling it would smooth the pixel art.

use crate::emu::controls::Controls;
use crate::emu::kitty;
use crate::emu::libretro::Core;
use crate::emu::runner::Shared;
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
use std::time::Duration;

/// The image id, which the placeholder cells carry as their colour ("rom" in ASCII).
pub const IMAGE: u32 = 0x72_6f_6d;
/// Frames sent but maybe not read yet; older ones still around were never read and are removed.
const PENDING: usize = 30;

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
}

impl Embedded {
    pub fn start(game: usize, core: Core, volume: f32) -> Result<Embedded> {
        let (shared, thread) = Shared::start(core, volume)?;
        // Key releases are needed to know when a button is let go.
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES | KeyboardEnhancementFlags::REPORT_EVENT_TYPES;
        execute!(std::io::stdout(), PushKeyboardEnhancementFlags(flags))?;
        Ok(Embedded {
            game,
            shared,
            thread: Some(thread),
            seen: 0,
            shape: (0.0, 0.0),
            cells: None,
            controls: Controls::new(),
            sent: 0,
            pending: VecDeque::new(),
        })
    }

    /// Takes a key. False when it stops the game.
    pub fn on_key(&mut self, k: KeyEvent) -> bool {
        if k.code == KeyCode::Esc && k.kind != KeyEventKind::Release {
            return false;
        }
        self.controls.key(k);
        true
    }

    /// Reads the controllers and hands the held buttons to the core. False when Select+Start
    /// has been held long enough to stop the game.
    pub fn read_pads(&mut self) -> bool {
        let Some(buttons) = self.controls.buttons() else { return false };
        self.shared.set_buttons(buttons);
        true
    }

    /// Why the game has no sound, once, if it has none.
    pub fn sound_error(&self) -> Option<String> {
        self.shared.take_sound_error()
    }

    /// Works out the cells the picture covers in `area`: as big as fits, in the frame's shape,
    /// centered. None until the core has drawn its first frame.
    pub fn place(&mut self, area: Rect) -> Option<Rect> {
        let (fw, fh) = self.shape;
        if fw <= 0.0 || fh <= 0.0 || area.is_empty() {
            return None;
        }
        let (cell_w, cell_h) = kitty::cell_size(&terminal::window_size().ok()?);
        let fit = (f64::from(u32::from(area.width) * cell_w) / fw).min(f64::from(u32::from(area.height) * cell_h) / fh);
        let cols = ((fw * fit / f64::from(cell_w)).round() as u16).clamp(1, area.width);
        let rows = ((fh * fit / f64::from(cell_h)).round() as u16).clamp(1, area.height);
        let rect = Rect::new(area.x + (area.width - cols) / 2, area.y + (area.height - rows) / 2, cols, rows);
        self.cells = Some((rect, (u32::from(cols) * cell_w, u32::from(rows) * cell_h)));
        Some(rect)
    }

    /// Sends the core's newest frame to the terminal, if there's one it hasn't seen.
    pub fn pump(&mut self) -> Result<()> {
        let Some((n, frame)) = self.shared.newer_frame(self.seen, Duration::ZERO) else { return Ok(()) };
        self.seen = n;
        if frame.width == 0 {
            return Ok(());
        }
        self.shape = frame.display_size();
        let Some((rect, (w, h))) = self.cells else { return Ok(()) };
        let pixels = frame.resample(w, h);
        self.sent += 1;
        let name = format!("/romi-{}-{}", std::process::id(), self.sent);
        kitty::write_shm(&name, &pixels)?;
        let mut out = std::io::stdout().lock();
        write!(
            out,
            "\x1b_Ga=T,U=1,i={IMAGE},c={},r={},f=24,s={w},v={h},t=s,S={},q=2;{}\x1b\\",
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
        let _ = write!(out, "{}", kitty::delete(IMAGE));
        let _ = execute!(out, PopKeyboardEnhancementFlags);
        for name in &self.pending {
            kitty::remove_shm(name);
        }
    }
}

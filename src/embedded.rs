//! A game played inside the TUI, in the middle panel. The core runs on its own thread (as in
//! `romi run --live`), and the picture is a Kitty image behind Unicode placeholder cells: ratatui
//! lays those out like text, and each new frame only replaces the image behind them.
//!
//! romi scales each frame to the exact pixel size of its cells itself, nearest neighbour, because
//! a terminal scaling it would smooth the pixel art.

use crate::kitty;
use crate::live::{Shared, key_button, pad_buttons};
use crate::retro::{Core, Frame, button};
use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use gilrs::Gilrs;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::{execute, terminal};
use ratatui::layout::Rect;
use std::collections::{HashSet, VecDeque};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The image id, which the placeholder cells carry as their colour ("rom" in ASCII).
pub const IMAGE: u32 = 0x72_6f_6d;
/// Holding Select and Start together this long stops the game.
const QUIT_HOLD: Duration = Duration::from_secs(1);
/// Frames sent but maybe not read yet; older ones still around were never read and are removed.
const PENDING: usize = 30;

pub struct Embedded {
    pub game: usize,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    /// Number of the last frame taken from the core.
    seen: u64,
    /// Size of the newest frame, which decides the picture's shape.
    frame_size: (u32, u32),
    /// The cells the picture covers, from the last draw, and their size in pixels.
    cells: Option<(Rect, (u32, u32))>,
    gilrs: Option<Gilrs>,
    keys: HashSet<KeyCode>,
    quit_held_since: Option<Instant>,
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
            frame_size: (0, 0),
            cells: None,
            gilrs: Gilrs::new().ok(),
            keys: HashSet::new(),
            quit_held_since: None,
            sent: 0,
            pending: VecDeque::new(),
        })
    }

    /// Takes a key. False when it stops the game.
    pub fn on_key(&mut self, k: KeyEvent) -> bool {
        match k.kind {
            KeyEventKind::Release => {
                self.keys.remove(&k.code);
            }
            _ if k.code == KeyCode::Esc => return false,
            _ => {
                self.keys.insert(k.code);
            }
        }
        true
    }

    /// Reads the controllers and hands the held buttons to the core. False when Select+Start
    /// has been held long enough to stop the game.
    pub fn read_pads(&mut self) -> bool {
        let mut buttons = self.keys.iter().filter_map(|&k| key_button(k)).fold(0, |b, n| b | n);
        if let Some(gilrs) = &mut self.gilrs {
            while gilrs.next_event().is_some() {}
            for (_, pad) in gilrs.gamepads() {
                buttons |= pad_buttons(&pad);
            }
        }
        let quit = button("select").unwrap() | button("start").unwrap();
        if buttons & quit == quit {
            if self.quit_held_since.get_or_insert_with(Instant::now).elapsed() >= QUIT_HOLD {
                return false;
            }
        } else {
            self.quit_held_since = None;
        }
        self.shared.buttons.store(buttons, Ordering::Relaxed);
        true
    }

    /// Why the game has no sound, once, if it has none.
    pub fn sound_error(&self) -> Option<String> {
        self.shared.sound_error.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Works out the cells the picture covers in `area`: as big as fits, in the frame's shape,
    /// centered. None until the core has drawn its first frame.
    pub fn place(&mut self, area: Rect) -> Option<Rect> {
        let (fw, fh) = self.frame_size;
        if fw == 0 || fh == 0 || area.is_empty() {
            return None;
        }
        let size = terminal::window_size().ok()?;
        // Terminals that don't report pixels get a guess at a common cell size.
        let (cell_w, cell_h) = match (size.width, size.height) {
            (0, _) | (_, 0) => (8, 16),
            (w, h) => (u32::from(w) / u32::from(size.columns.max(1)), u32::from(h) / u32::from(size.rows.max(1))),
        };
        let fit = (f64::from(u32::from(area.width) * cell_w) / f64::from(fw))
            .min(f64::from(u32::from(area.height) * cell_h) / f64::from(fh));
        let cols = ((f64::from(fw) * fit / f64::from(cell_w)).round() as u16).clamp(1, area.width);
        let rows = ((f64::from(fh) * fit / f64::from(cell_h)).round() as u16).clamp(1, area.height);
        let rect = Rect::new(area.x + (area.width - cols) / 2, area.y + (area.height - rows) / 2, cols, rows);
        self.cells = Some((rect, (u32::from(cols) * cell_w, u32::from(rows) * cell_h)));
        Some(rect)
    }

    /// Sends the core's newest frame to the terminal, if there's one it hasn't seen.
    pub fn pump(&mut self) -> Result<()> {
        let Some((n, frame)) = self.shared.take_newer(self.seen) else { return Ok(()) };
        self.seen = n;
        if frame.width == 0 {
            return Ok(());
        }
        self.frame_size = (frame.width, frame.height);
        let Some((rect, (w, h))) = self.cells else { return Ok(()) };
        let pixels = resample(&frame, w, h);
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

/// The frame scaled to exactly `w`×`h`, nearest neighbour, so pixels stay sharp.
fn resample(frame: &Frame, w: u32, h: u32) -> Vec<u8> {
    let (fw, fh) = (frame.width as usize, frame.height as usize);
    let (w, h) = (w as usize, h as usize);
    let xs: Vec<usize> = (0..w).map(|x| x * fw / w * 3).collect();
    let mut out = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        let row = &frame.rgb[y * fh / h * fw * 3..][..fw * 3];
        for &x in &xs {
            out.extend_from_slice(&row[x..x + 3]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_repeats_the_nearest_pixel() {
        let frame = Frame { width: 2, height: 1, rgb: vec![1, 1, 1, 2, 2, 2] };
        assert_eq!(resample(&frame, 4, 2), [[1u8; 6], [2; 6], [1; 6], [2; 6]].concat());
        assert_eq!(resample(&frame, 3, 1), [1, 1, 1, 1, 1, 1, 2, 2, 2]);
    }
}

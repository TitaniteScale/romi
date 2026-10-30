//! Black edges cropped off a game's picture. Many games draw inside a safe area that a CRT's
//! overscan hid, and leave black all round it (most N64 games do, Banjo-Kazooie by a tenth of the
//! screen), which in the panel is a black box around the game. romi keeps the smallest box that
//! has held everything the game has drawn, and once the game has drawn for a while and that box
//! covers most of the screen, shows only what's in it. The box only grows, so the picture only
//! ever widens, if the game draws into an edge that was black.

use crate::emu::libretro::Frame;

/// Brighter than this, in any color, is drawn on; darker is the black of an edge.
const DARK: u8 = 16;
/// Frames with something drawn before the edges are trusted: a logo on black isn't the screen.
const SETTLE: u32 = 120;
/// The box must cover this much of the width and the height to be cropped to, so a game that has
/// only drawn in the middle so far isn't blown up.
const COVER: f64 = 0.7;
/// Edges thinner than this, as a part of the width or height, aren't worth cropping.
const MIN_EDGE: f64 = 0.02;

#[derive(Default)]
pub struct Crop {
    size: (u32, u32),
    /// The smallest box that has held everything drawn so far: left, top, right, bottom (the last
    /// two past the end).
    seen: Option<[u32; 4]>,
    /// Frames with anything drawn on them.
    drawn: u32,
}

impl Crop {
    /// The frame with its black edges cropped off, once they're known.
    pub fn apply(&mut self, frame: Frame) -> Frame {
        if (frame.width, frame.height) != self.size {
            *self = Crop { size: (frame.width, frame.height), ..Crop::default() };
        }
        if let Some([l, t, r, b]) = drawn_box(&frame) {
            self.drawn += 1;
            self.seen = Some(match self.seen {
                Some([sl, st, sr, sb]) => [sl.min(l), st.min(t), sr.max(r), sb.max(b)],
                None => [l, t, r, b],
            });
        }
        match self.cropped_to() {
            Some(rect) => cut(&frame, rect),
            None => frame,
        }
    }

    /// The box to crop to, if it's settled and worth it.
    fn cropped_to(&self) -> Option<[u32; 4]> {
        let [l, t, r, b] = self.seen?;
        let (w, h) = (f64::from(self.size.0), f64::from(self.size.1));
        let (cw, ch) = (f64::from(r - l), f64::from(b - t));
        let edges = [f64::from(l) / w, f64::from(t) / h, (w - f64::from(r)) / w, (h - f64::from(b)) / h];
        (self.drawn >= SETTLE && cw / w >= COVER && ch / h >= COVER && edges.iter().any(|&e| e >= MIN_EDGE))
            .then_some([l, t, r, b])
    }
}

/// The smallest box holding everything drawn on a frame, or None for a black frame.
fn drawn_box(frame: &Frame) -> Option<[u32; 4]> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let rgb = frame.rgb.get(..w * h * 3)?;
    let lit = |px: &[u8]| px.iter().any(|&c| c > DARK);
    let row_lit = |y: usize| rgb[y * w * 3..][..w * 3].as_chunks::<3>().0.iter().any(|px| lit(px));
    let top = (0..h).find(|&y| row_lit(y))?;
    let bottom = (top..h).rev().find(|&y| row_lit(y))? + 1;
    let column_lit = |x: usize| (top..bottom).any(|y| lit(&rgb[(y * w + x) * 3..][..3]));
    let left = (0..w).find(|&x| column_lit(x))?;
    let right = (left..w).rev().find(|&x| column_lit(x))? + 1;
    Some([left as u32, top as u32, right as u32, bottom as u32])
}

/// The part of a frame inside `[left, top, right, bottom]`.
fn cut(frame: &Frame, [l, t, r, b]: [u32; 4]) -> Frame {
    let (w, l, r) = (frame.width as usize, l as usize, r as usize);
    let mut rgb = Vec::with_capacity((r - l) * (b - t) as usize * 3);
    for y in t as usize..b as usize {
        rgb.extend_from_slice(&frame.rgb[(y * w + l) * 3..(y * w + r) * 3]);
    }
    Frame { width: (r - l) as u32, height: b - t, rgb, pixel_aspect: frame.pixel_aspect }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w`×`h` black frame with a grey box from (x0, y0) to (x1, y1).
    fn boxed(w: u32, h: u32, [x0, y0, x1, y1]: [u32; 4]) -> Frame {
        let mut rgb = vec![0; (w * h * 3) as usize];
        for y in y0..y1 {
            for x in x0..x1 {
                rgb[((y * w + x) * 3) as usize..][..3].copy_from_slice(&[90, 90, 90]);
            }
        }
        Frame { width: w, height: h, rgb, pixel_aspect: 0.0 }
    }

    #[test]
    fn black_edges_go_once_the_game_has_drawn_for_a_while() {
        let mut crop = Crop::default();
        let game = boxed(100, 80, [6, 4, 94, 76]);
        for _ in 1..SETTLE {
            assert_eq!(crop.apply(game.clone()).width, 100, "not trusted yet");
        }
        let cropped = crop.apply(game.clone());
        assert_eq!((cropped.width, cropped.height), (88, 72));
        assert!(cropped.rgb.iter().all(|&c| c == 90));
        // Drawing into the edge widens the picture again.
        assert_eq!(crop.apply(boxed(100, 80, [0, 4, 50, 76])).width, 94);
    }

    #[test]
    fn a_picture_drawn_only_in_the_middle_isnt_blown_up() {
        let mut crop = Crop::default();
        for _ in 0..SETTLE * 2 {
            assert_eq!(crop.apply(boxed(100, 80, [30, 30, 70, 50])).width, 100);
        }
    }

    #[test]
    fn black_frames_and_thin_edges_are_left_alone() {
        let mut crop = Crop::default();
        for _ in 0..SETTLE * 2 {
            assert_eq!(crop.apply(boxed(100, 80, [0, 0, 0, 0])).width, 100);
            assert_eq!(crop.apply(boxed(100, 80, [1, 0, 100, 80])).width, 100);
        }
    }
}

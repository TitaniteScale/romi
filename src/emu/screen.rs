//! Screen filters for a game played in romi: an old screen's look, drawn over the picture after
//! it's scaled to its cells. romi scales frames itself, nearest neighbour, so it knows which
//! screen pixels each of the game's pixels covers, and each filter is a brightness for every
//! screen row and every screen column, colour by colour:
//!
//! - scanlines: each of the game's rows is brightest in its middle and dims towards its top, as
//!   if drawn by a CRT's beam, with gaps between rows.
//! - crt: scanlines, and a phosphor mask, columns of red, green and blue.
//! - lcd: a dark line between the game's pixels, like a handheld's screen.
//!
//! The picture is brightened to make up for what a filter darkens, as CRT shaders do, so it's as
//! bright overall; its brightest parts clip a little. A filter needs a few screen pixels for each
//! of the game's pixels; at less than that, the parts that wouldn't fit are left out.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Filter {
    #[default]
    None,
    Scanlines,
    Crt,
    Lcd,
}

impl Filter {
    pub const ALL: [Filter; 4] = [Filter::None, Filter::Scanlines, Filter::Crt, Filter::Lcd];

    /// Its name in the config.
    pub fn name(self) -> &'static str {
        match self {
            Filter::None => "none",
            Filter::Scanlines => "scanlines",
            Filter::Crt => "crt",
            Filter::Lcd => "lcd",
        }
    }
}

/// Full brightness, in the fixed point brightnesses are kept in.
const ONE: u32 = 256;
/// How dark the gap between scanlines gets: the darkest a row gets is `1 - SCANLINE`.
const SCANLINE: f64 = 0.5;
/// How much the phosphor mask dims the two colours a column isn't.
const MASK: f64 = 0.7;
/// How bright the lines between an LCD's pixels are.
const GRID: f64 = 0.65;
/// Screen pixels a game pixel needs for scanlines, for an LCD's grid, and for the phosphor mask.
const MIN_FOR_SCANLINES: usize = 2;
const MIN_FOR_GRID: usize = 3;
const MIN_FOR_MASK: usize = 3;

/// Applies `filter` to a picture `w`×`h` pixels of `bpp` bytes each (RGB or RGBA; opacity is left
/// alone), scaled nearest neighbour, as `Frame::resample` does, from a picture `fw`×`fh` on the
/// console (the frame's own size, or less for a 3D game drawn bigger: see `game_pixels`).
pub fn apply(pixels: &mut [u8], bpp: usize, (w, h): (usize, usize), (fw, fh): (usize, usize), filter: Filter) {
    if filter == Filter::None || w == 0 || h == 0 || fw == 0 || fh == 0 {
        return;
    }
    let rows = runs(h, fh);
    let cols = runs(w, fw);
    let mut row_light: Vec<u32> = match filter {
        Filter::Scanlines | Filter::Crt => scanlines(&rows),
        Filter::Lcd => grid(&rows),
        Filter::None => unreachable!(),
    };
    let col_light: Vec<[u32; 3]> = match filter {
        Filter::Crt => mask(w, w / fw),
        Filter::Lcd => grid(&cols).into_iter().map(|l| [l; 3]).collect(),
        _ => vec![[ONE; 3]; w],
    };
    // Each row brightened by as much as the rows and columns dim on average.
    let mean = |sum: u64, n: usize| sum as f64 / n as f64 / f64::from(ONE);
    let rows_mean = mean(row_light.iter().map(|&l| u64::from(l)).sum(), h);
    let cols_mean = mean(col_light.iter().flatten().map(|&l| u64::from(l)).sum(), w * 3);
    let gain = 1.0 / (rows_mean * cols_mean);
    for l in &mut row_light {
        *l = (f64::from(*l) * gain).round() as u32;
    }
    for (row, &rl) in pixels.chunks_exact_mut(w * bpp).zip(&row_light) {
        for (px, cl) in row.chunks_exact_mut(bpp).zip(&col_light) {
            for c in 0..3 {
                px[c] = (u32::from(px[c]) * rl * cl[c] / (ONE * ONE)).min(255) as u8;
            }
        }
    }
}

/// For each of `n` screen pixels scaled from `of` game pixels, where it is in its game pixel's
/// run: (its place in the run, the run's length).
fn runs(n: usize, of: usize) -> Vec<(usize, usize)> {
    let src = |i: usize| i * of / n;
    let mut out = Vec::with_capacity(n);
    let mut start = 0;
    while start < n {
        let end = (start..n).find(|&i| src(i) != src(start)).unwrap_or(n);
        out.extend((0..end - start).map(|i| (i, end - start)));
        start = end;
    }
    out
}

fn fixed(x: f64) -> u32 {
    (x * f64::from(ONE)).round() as u32
}

/// A beam's brightness down each row: darkest at the row's top, brightest in its middle.
fn scanlines(rows: &[(usize, usize)]) -> Vec<u32> {
    if rows.iter().all(|&(_, len)| len < MIN_FOR_SCANLINES) {
        return vec![ONE; rows.len()];
    }
    rows.iter()
        .map(|&(i, len)| {
            // A row a single screen pixel tall has no room for a scanline; dimmed as a top, it
            // stood out as a dark line among the rest.
            if len < MIN_FOR_SCANLINES {
                return ONE;
            }
            let t = i as f64 / len as f64;
            fixed(1.0 - SCANLINE * (1.0 - (std::f64::consts::PI * t).sin()))
        })
        .collect()
}

/// A dark line along the first screen pixel of each game pixel.
fn grid(runs: &[(usize, usize)]) -> Vec<u32> {
    runs.iter().map(|&(i, len)| if i == 0 && len >= MIN_FOR_GRID { fixed(GRID) } else { ONE }).collect()
}

/// Columns of red, green and blue phosphors across the screen, one screen pixel each.
fn mask(w: usize, scale: usize) -> Vec<[u32; 3]> {
    if scale < MIN_FOR_MASK {
        return vec![[ONE; 3]; w];
    }
    let dim = fixed(MASK);
    (0..w)
        .map(|x| {
            let mut light = [dim; 3];
            light[x % 3] = ONE;
            light
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_cover_each_game_pixel_once() {
        assert_eq!(runs(6, 2), [(0, 3), (1, 3), (2, 3), (0, 3), (1, 3), (2, 3)]);
        // 7 over 3 game pixels, as `Frame::resample` spreads them: 3, 2, 2.
        assert_eq!(runs(7, 3), [(0, 3), (1, 3), (2, 3), (0, 2), (1, 2), (0, 2), (1, 2)]);
    }

    #[test]
    fn rows_one_screen_pixel_tall_get_no_scanline() {
        // 5 screen rows for 3 game rows: two of two pixels and one of one.
        let rows = runs(5, 3);
        let light = scanlines(&rows);
        for (&(_, len), &l) in rows.iter().zip(&light) {
            if len == 1 {
                assert_eq!(l, ONE);
            }
        }
        assert!(light.iter().any(|&l| l < ONE), "the taller rows still get theirs");
    }

    #[test]
    fn scanlines_dim_the_top_of_each_row_and_brighten_its_middle() {
        // A grey 1×2 frame, 4×8 on screen: four screen rows per game row.
        let mut px = vec![100; 4 * 8 * 3];
        apply(&mut px, 3, (4, 8), (1, 2), Filter::Scanlines);
        let row = |y: usize| px[y * 4 * 3];
        assert_eq!(row(0), row(4), "every game row is the same");
        assert!(row(0) < 100 && row(2) > 100, "dark between rows, brighter than it was in the middle");
        assert!(row(0) < row(1) && row(1) < row(2));
        let total: u32 = (0..4).map(|y| u32::from(row(y))).sum();
        assert!(total.abs_diff(400) <= 4, "as bright overall: {total}");
    }

    #[test]
    fn lcd_draws_a_grid_and_leaves_opacity_alone() {
        let mut px = vec![100; 6 * 6 * 4];
        apply(&mut px, 4, (6, 6), (2, 2), Filter::Lcd);
        let at = |x: usize, y: usize| px[(y * 6 + x) * 4];
        let (inside, line, cross) = (at(1, 1), at(3, 1), at(0, 0));
        assert!(cross < line && line < inside, "{cross} < {line} < {inside}");
        assert_eq!(at(1, 3), line, "lines run both ways");
        assert!(px.chunks(4).all(|p| p[3] == 100 && p[0] == p[1] && p[1] == p[2]));
    }

    #[test]
    fn small_pictures_go_without_what_doesnt_fit() {
        // One screen pixel per game pixel: nothing fits.
        for filter in Filter::ALL {
            let mut px = vec![200; 4 * 4 * 3];
            apply(&mut px, 3, (4, 4), (4, 4), filter);
            assert!(px.iter().all(|&v| v == 200), "{filter:?}");
        }
    }

    #[test]
    fn the_phosphor_mask_steps_through_red_green_and_blue() {
        let mut px = vec![100; 6 * 4 * 3];
        apply(&mut px, 3, (6, 4), (2, 1), Filter::Crt);
        let row: Vec<_> = px[2 * 6 * 3..][..6 * 3].chunks(3).map(|p| [p[0], p[1], p[2]]).collect();
        let (lit, dim) = (row[0][0], row[0][1]);
        assert!(dim < lit);
        assert_eq!(row[..3], [[lit, dim, dim], [dim, lit, dim], [dim, dim, lit]]);
        assert_eq!(row[3], row[0]);
    }
}

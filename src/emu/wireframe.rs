//! romi's launch animation: a wireframe of the system (a console and a TV, or a handheld) builds
//! itself out of lines, then the camera dives into its screen and lands on exactly the cells the
//! game is drawn in, as the game fades in on the glass.
//!
//! The game's file name seeds the run: how lines enter and in what order (the same as the browser
//! prototype's for a seed), the camera's path, and the beam's color, picked from the terminal's
//! palette. Frames are RGBA, clear but for the lines and the game, so the terminal's background
//! shows through.
//!
//! The models are JSON: vertices in cm, edges as pairs of vertex numbers, and `screen`, the glass's
//! corners as top left, top right, bottom right, bottom left. `examples/wireframe-import` makes
//! them from downloaded models; see CREDITS.md for whose. `examples/wireframe` plays this module
//! on its own, with a stand-in picture for the game, and so has to reach nothing else in romi.

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::f64::consts::PI;
use std::fmt::Write as _;
use std::io::Write;
use std::time::{Duration, Instant};

/// Nothing closer to the camera than this (in cm) is drawn.
const NEAR: f64 = 0.6;
/// How far behind the glass (in cm) a line is hidden by the game's picture.
const BEHIND: f64 = 0.5;
/// Seconds the lines take to fade once the camera has landed.
pub const FADE: f64 = 0.35;
/// The ANSI colors a beam can be when the terminal says what they are: red, green, yellow, blue,
/// magenta and cyan, and their bright versions.
const ANSI: [u8; 12] = [1, 2, 3, 4, 5, 6, 9, 10, 11, 12, 13, 14];
/// The beam's colors when the terminal doesn't say what its own are.
const DEFAULT_COLORS: [[u8; 3]; 4] = [[0x9f, 0xe3, 0xff], [0xff, 0xb5, 0x47], [0x9d, 0xf2, 0xa8], [0xff, 0x9e, 0xcf]];
/// How far a color has to stand out from the background (as a WCAG contrast ratio) to be a beam.
const CONTRAST: f64 = 3.0;
/// How long to wait for the terminal to say what its colors are.
const ASK_FOR: Duration = Duration::from_millis(300);

/// The wireframe for a system, by its id: its own console's, or a TV for the rest.
pub fn for_system(system: &str) -> &'static str {
    match system {
        "gb" | "gbc" => include_str!("wireframes/gameboy.json"),
        "nes" | "famicom" | "fds" => include_str!("wireframes/nes.json"),
        "snes" | "snesna" | "sfc" | "satellaview" | "sufami" => include_str!("wireframes/snes.json"),
        "arcade" | "mame" | "fbneo" | "neogeo" | "cps1" | "cps2" | "cps3" => include_str!("wireframes/arcade.json"),
        _ => include_str!("wireframes/tv.json"),
    }
}

/// How a run goes, besides what the seed picks.
#[derive(Clone, Debug)]
pub struct Settings {
    /// How lines enter, instead of the seed's pick.
    pub style: Option<Style>,
    /// Which lines go first, instead of the seed's pick.
    pub order: Option<Order>,
    /// Seconds the lines take to build.
    pub build: f64,
    /// How spread out the lines' starts are, from 0 (all at once) to 1 (one after another).
    pub stagger: f64,
    /// Seconds the zoom into the screen takes.
    pub zoom: f64,
    /// How far before the build ends the zoom starts, as a fraction of the build.
    pub overlap: f64,
    /// The camera's vertical field of view, in degrees.
    pub fov: f64,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { style: None, order: None, build: 1.8, stagger: 0.65, zoom: 1.3, overlap: 0.12, fov: 48.0 }
    }
}

/// The picture a frame is drawn as.
pub struct View {
    /// Its size in pixels.
    pub w: usize,
    pub h: usize,
    /// Where the game goes, in its pixels: x, y, width, height.
    pub target: [f64; 4],
    /// Its pixels per pixel of the browser prototype, which assumes 8×16 cells.
    pub unit: f64,
    /// How round the game's corners are once it has landed, as a fraction of its height, to match
    /// how it's shown afterwards.
    pub corners: f64,
}

impl View {
    /// A `w`×`h` picture whose cells are `cell_w` of its pixels wide.
    pub fn new(w: usize, h: usize, target: [f64; 4], cell_w: f64) -> View {
        View { w: w.max(1), h: h.max(1), target, unit: cell_w / 8.0, corners: 0.0 }
    }
}

/// The game's picture: RGB, `w`×`h`.
#[derive(Clone, Copy)]
pub struct Picture<'a> {
    pub rgb: &'a [u8],
    pub w: usize,
    pub h: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, ValueEnum)]
pub enum Style {
    Pen,
    Explode,
    Scan,
    Fly,
    Dots,
    Vortex,
}

#[derive(Clone, Copy, Debug, PartialEq, ValueEnum)]
pub enum Order {
    Walk,
    Bottom,
    Front,
    Screen,
    Random,
}

/// In the browser prototype's order, so a seed picks the same ones there and here.
const STYLES: [Style; 6] = [Style::Pen, Style::Explode, Style::Scan, Style::Fly, Style::Dots, Style::Vortex];
const ORDERS: [Order; 5] = [Order::Walk, Order::Bottom, Order::Front, Order::Screen, Order::Random];

impl Style {
    #[allow(dead_code)] // for the example's status line
    pub fn name(self) -> &'static str {
        match self {
            Style::Pen => "pen trace",
            Style::Explode => "reverse explode",
            Style::Scan => "scan flash",
            Style::Fly => "fly in",
            Style::Dots => "dots then lines",
            Style::Vortex => "vortex",
        }
    }
}

impl Order {
    #[allow(dead_code)] // for the example's status line
    pub fn name(self) -> &'static str {
        match self {
            Order::Walk => "graph walk",
            Order::Bottom => "bottom up",
            Order::Front => "front to back",
            Order::Screen => "outward from screen",
            Order::Random => "random",
        }
    }
}

// ---------- the model ----------

type V3 = [f64; 3];
type P2 = (f64, f64);

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn len(a: V3) -> f64 {
    dot(a, a).sqrt()
}
fn norm(a: V3) -> V3 {
    let l = len(a);
    if l == 0.0 { a } else { mul(a, 1.0 / l) }
}
fn lerp(a: V3, b: V3, t: f64) -> V3 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
/// Turns `v` by `angle` about the unit axis `k`.
fn rot(v: V3, k: V3, angle: f64) -> V3 {
    let (c, s) = (angle.cos(), angle.sin());
    add(add(mul(v, c), mul(cross(k, v), s)), mul(k, dot(k, v) * (1.0 - c)))
}
fn smooth(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}
fn ease_out(x: f64) -> f64 {
    1.0 - (1.0 - x).powi(3)
}
fn ease_in_out(x: f64) -> f64 {
    if x < 0.5 { 4.0 * x * x * x } else { 1.0 - (-2.0 * x + 2.0).powi(3) / 2.0 }
}

#[derive(Deserialize)]
struct ModelFile {
    display_aspect: [f64; 2],
    screen: [usize; 4],
    vertices: Vec<V3>,
    edges: Vec<[usize; 2]>,
}

pub struct Model {
    v: Vec<V3>,
    e: Vec<[usize; 2]>,
    /// The glass's corners: top left, top right, bottom right, bottom left.
    screen: [usize; 4],
    pub aspect: (f64, f64),
    /// Where the game goes, corners in the same order: the glass, or the middle of it in the
    /// game's shape (see `fit`).
    picture: [V3; 4],
    /// For each vertex, its edges and the vertex at their other end.
    adj: Vec<Vec<(usize, usize)>>,
    center: V3,
    radius: f64,
    /// The middle of the picture, and its height.
    glass: V3,
    glass_h: f64,
}

impl Model {
    pub fn load(json: &str) -> Result<Model> {
        let file: ModelFile = serde_json::from_str(json).context("reading the model")?;
        let n = file.vertices.len();
        if let Some(bad) = file.edges.iter().flatten().chain(&file.screen).find(|&&i| i >= n) {
            bail!("the model names vertex {bad}, but only has {n}");
        }
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for p in &file.vertices {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let center = lerp(lo, hi, 0.5);
        let radius = file.vertices.iter().map(|&p| len(sub(p, center))).fold(0.0, f64::max);
        let mut adj = vec![Vec::new(); n];
        for (i, &[a, b]) in file.edges.iter().enumerate() {
            adj[a].push((i, b));
            adj[b].push((i, a));
        }
        let picture = file.screen.map(|i| file.vertices[i]);
        let [tl, _, br, bl] = picture;
        Ok(Model {
            screen: file.screen,
            aspect: (file.display_aspect[0], file.display_aspect[1]),
            picture,
            adj,
            center,
            radius,
            glass: lerp(tl, br, 0.5),
            glass_h: len(sub(tl, bl)),
            v: file.vertices,
            e: file.edges,
        })
    }

    /// Puts the game's picture in the middle of the glass, as big as fits in the game's shape
    /// (`w`×`h`), the way a TV shows a picture that isn't its own shape. The camera lands on it.
    pub fn fit(&mut self, (w, h): (f64, f64)) {
        let [tl, tr, _, bl] = self.screen.map(|i| self.v[i]);
        let (across, down) = (sub(tr, tl), sub(bl, tl));
        let (gw, gh) = (len(across), len(down));
        if w <= 0.0 || h <= 0.0 || gw <= 0.0 || gh <= 0.0 {
            return;
        }
        let k = (gw / w).min(gh / h);
        let middle = add(tl, mul(add(across, down), 0.5));
        let (a, d) = (mul(across, w * k / gw / 2.0), mul(down, h * k / gh / 2.0));
        self.picture = [sub(sub(middle, a), d), sub(add(middle, a), d), add(add(middle, a), d), add(sub(middle, a), d)];
        self.glass = middle;
        self.glass_h = h * k;
    }
}

// ---------- the seeded plan ----------

/// mulberry32, the same generator the browser prototype uses.
pub struct Rng(pub u32);

impl Rng {
    pub fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6D2B79F5);
        let a = self.0;
        let mut t = (a ^ (a >> 15)).wrapping_mul(1 | a);
        t = t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t)) ^ t;
        f64::from(t ^ (t >> 14)) / 4294967296.0
    }

    fn unit(&mut self) -> V3 {
        let z = self.next() * 2.0 - 1.0;
        let p = self.next() * PI * 2.0;
        let s = (1.0 - z * z).sqrt();
        [s * p.cos(), z, s * p.sin()]
    }
}

pub fn fnv1a(s: &str) -> u32 {
    s.chars().fold(0x811c9dc5, |h: u32, c| (h ^ c as u32).wrapping_mul(0x01000193))
}

fn hash2(a: u32, b: u32) -> f64 {
    let h = a.wrapping_mul(374761393).wrapping_add(b.wrapping_mul(668265263));
    let h = (h ^ (h >> 13)).wrapping_mul(1274126177);
    f64::from(h ^ (h >> 16)) / 4294967296.0
}

/// Everything about one run that the seed decides, and when things happen.
pub struct Plan {
    pub style: Style,
    #[allow(dead_code)] // for the example's status line
    pub order: Order,
    yaw0: f64,
    pitch0: f64,
    drift: f64,
    /// Whether each edge grows from its second vertex rather than its first.
    from: Vec<bool>,
    dir: Vec<V3>,
    axis: Vec<V3>,
    spin: Vec<f64>,
    radial: Vec<V3>,
    /// When each edge starts, and how long every edge takes.
    start: Vec<f64>,
    dur: f64,
    /// When each vertex pops in (dots then lines only).
    vstart: Vec<f64>,
    build: f64,
    zoom: f64,
    fov: f64,
    zoom_start: f64,
    pub zoom_end: f64,
    /// When the game starts fading in on the glass.
    pub game_in: f64,
}

impl Plan {
    pub fn new(model: &Model, settings: &Settings, seed: u32) -> Plan {
        let mut r = Rng(seed);
        let seeded_style = STYLES[(r.next() * STYLES.len() as f64) as usize];
        let seeded_order = ORDERS[(r.next() * ORDERS.len() as f64) as usize];
        let style = settings.style.unwrap_or(seeded_style);
        let order = settings.order.unwrap_or(seeded_order);
        let side = if r.next() < 0.5 { -1.0 } else { 1.0 };
        let yaw0 = side * (0.38 + r.next() * 0.5);
        let pitch0 = 0.22 + r.next() * 0.28;
        let drift = -side * (0.08 + r.next() * 0.12);

        let (nv, ne) = (model.v.len(), model.e.len());
        let (mut vkey, mut ekey, mut from) = (vec![0.0; nv], vec![0.0; ne], vec![false; ne]);
        if order == Order::Walk {
            // Breadth-first from a seeded vertex; when a piece runs out, hop to the nearest
            // unvisited vertex, like a plotter would.
            fn visit(v: usize, seen: &mut [bool], rank: &mut [f64], queue: &mut Vec<usize>) {
                if !seen[v] {
                    seen[v] = true;
                    rank[v] = queue.len() as f64;
                    queue.push(v);
                }
            }
            let (mut seen, mut eseen, mut queue) = (vec![false; nv], vec![false; ne], Vec::new());
            let (mut head, mut ranked) = (0, 0);
            visit((r.next() * nv as f64) as usize, &mut seen, &mut vkey, &mut queue);
            loop {
                if head >= queue.len() {
                    let last = model.v[queue[queue.len() - 1]];
                    let nearest = (0..nv)
                        .filter(|&v| !seen[v])
                        .min_by(|&a, &b| len(sub(model.v[a], last)).total_cmp(&len(sub(model.v[b], last))));
                    let Some(nearest) = nearest else { break };
                    visit(nearest, &mut seen, &mut vkey, &mut queue);
                }
                let v = queue[head];
                head += 1;
                for &(ei, other) in &model.adj[v] {
                    if !eseen[ei] {
                        eseen[ei] = true;
                        ekey[ei] = ranked as f64;
                        ranked += 1;
                        from[ei] = model.e[ei][0] != v;
                        visit(other, &mut seen, &mut vkey, &mut queue);
                    }
                }
            }
            vkey.iter_mut().for_each(|k| *k /= (queue.len().max(2) - 1) as f64);
            ekey.iter_mut().for_each(|k| *k /= (ranked.max(2) - 1) as f64);
        } else {
            let m: Vec<f64> = model
                .v
                .iter()
                .map(|&p| match order {
                    Order::Bottom => p[1],
                    Order::Front => -p[2],
                    Order::Screen => len(sub(p, model.glass)),
                    _ => r.next(),
                })
                .collect();
            let lo = m.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = m.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let span = if hi > lo { hi - lo } else { 1.0 };
            for (k, &x) in vkey.iter_mut().zip(&m) {
                *k = (x - lo) / span;
            }
            for (i, &[a, b]) in model.e.iter().enumerate() {
                if order == Order::Random {
                    ekey[i] = r.next();
                    from[i] = r.next() >= 0.5;
                } else {
                    ekey[i] = vkey[a].min(vkey[b]);
                    from[i] = vkey[a] > vkey[b];
                }
            }
        }
        for k in &mut ekey {
            *k = (*k * 0.9 + r.next() * 0.1).clamp(0.0, 1.0);
        }

        let (mut dir, mut axis, mut spin, mut radial) = (vec![], vec![], vec![], vec![]);
        for &[a, b] in &model.e {
            dir.push(r.unit());
            axis.push(r.unit());
            spin.push((r.next() * 2.0 - 1.0) * PI);
            let _flicker = r.next();
            let out = sub(lerp(model.v[a], model.v[b], 0.5), model.center);
            radial.push(if len(out) > 0.01 { norm(out) } else { r.unit() });
        }

        let build = settings.build;
        let dots = style == Style::Dots;
        let (window, offset) = if dots { (build * 0.7, build * 0.3) } else { (build, 0.0) };
        let dur = f64::max(0.06, window * (1.0 - 0.9 * settings.stagger));
        let start = ekey.iter().map(|k| offset + k * (window - dur)).collect();
        let vstart = vkey.iter().map(|k| k * build * 0.45).collect();
        let zoom_start = build * (1.0 - settings.overlap);
        Plan {
            style,
            order,
            yaw0,
            pitch0,
            drift,
            from,
            dir,
            axis,
            spin,
            radial,
            start,
            dur,
            vstart,
            build,
            zoom: settings.zoom,
            fov: settings.fov,
            zoom_start,
            zoom_end: zoom_start + settings.zoom,
            game_in: zoom_start + settings.zoom * 0.55,
        }
    }

    /// Where the camera is at `t`: drifting around the model while it builds, then swinging round
    /// to face the glass from the distance that makes it exactly as tall as the game's cells.
    fn camera(&self, model: &Model, view: &View, t: f64) -> Cam {
        let fov = self.fov.to_radians();
        let fpx = view.h as f64 / 2.0 / (fov / 2.0).tan();
        let fit = model.radius / (fov / 2.0).sin() * 0.95;
        let orbit = |at: f64| {
            let k = (at / self.build.max(0.01)).clamp(0.0, 1.5);
            add(model.center, sph(self.yaw0 + self.drift * k, self.pitch0, fit * (1.1 - 0.08 * k.min(1.0))))
        };
        if t <= self.zoom_start {
            return Cam::look(orbit(t), model.center, fpx, view);
        }
        let e = ease_in_out(((t - self.zoom_start) / self.zoom).clamp(0.0, 1.0));
        let rel = sub(orbit(self.zoom_start), model.glass);
        let d0 = len(rel);
        let (yaw0, pitch0) = (rel[0].atan2(rel[2]), (rel[1] / d0).asin());
        let [x, y, w, h] = view.target;
        let d1 = fpx * model.glass_h / h;
        let (dx, dy) = (x + w / 2.0 - view.w as f64 / 2.0, y + h / 2.0 - view.h as f64 / 2.0);
        let (ox, oy) = (-dx * d1 / fpx, dy * d1 / fpx);
        let d = d0 * (d1 / d0).powf(e);
        let pos = add(add(model.glass, sph(yaw0 * (1.0 - e), pitch0 * (1.0 - e), d)), [ox * e, oy * e, 0.0]);
        let target = lerp(model.center, add(model.glass, [ox, oy, 0.0]), e);
        Cam::look(pos, target, fpx, view)
    }

    /// How far, in picture pixels, the glass's corners end up from the game's cells.
    #[allow(dead_code)] // for the example's status line
    pub fn landing_error(&self, model: &Model, view: &View) -> f64 {
        let cam = self.camera(model, view, self.zoom_end);
        let [x, y, w, h] = view.target;
        let want = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
        model
            .picture
            .iter()
            .zip(want)
            .filter_map(|(&p, (wx, wy))| cam.project(p).map(|(px, py)| (px - wx).hypot(py - wy)))
            .fold(0.0, f64::max)
    }
}

fn sph(yaw: f64, pitch: f64, d: f64) -> V3 {
    [d * pitch.cos() * yaw.sin(), d * pitch.sin(), d * pitch.cos() * yaw.cos()]
}

struct Cam {
    pos: V3,
    forward: V3,
    right: V3,
    up: V3,
    fpx: f64,
    center: P2,
}

impl Cam {
    fn look(pos: V3, target: V3, fpx: f64, view: &View) -> Cam {
        let forward = norm(sub(target, pos));
        let right = norm([-forward[2], 0.0, forward[0]]);
        Cam { pos, forward, right, up: cross(right, forward), fpx, center: (view.w as f64 / 2.0, view.h as f64 / 2.0) }
    }

    fn to_cam(&self, p: V3) -> V3 {
        let d = sub(p, self.pos);
        [dot(d, self.right), dot(d, self.up), dot(d, self.forward)]
    }

    fn to_screen(&self, c: V3) -> P2 {
        (self.center.0 + self.fpx * c[0] / c[2], self.center.1 - self.fpx * c[1] / c[2])
    }

    fn project(&self, p: V3) -> Option<P2> {
        let c = self.to_cam(p);
        (c[2] >= NEAR).then(|| self.to_screen(c))
    }
}

// ---------- drawing ----------

/// Buffers kept between frames.
#[derive(Default)]
pub struct Painter {
    /// How much beam lands on each pixel; over 1 burns white.
    light: Vec<f32>,
    small: Vec<f32>,
    spare: Vec<f32>,
    rgba: Vec<u8>,
    lines: Vec<(P2, P2, f32)>,
    dots: Vec<(P2, f32, f64)>,
}

impl Painter {
    /// A frame as RGBA, clear but for the lines and the game.
    pub fn paint(
        &mut self,
        model: &Model,
        plan: &Plan,
        view: &View,
        t: f64,
        game: Option<Picture>,
        beam: Beam,
    ) -> &[u8] {
        let Beam { color, light } = beam;
        let (w, h) = (view.w, view.h);
        self.rgba.clear();
        self.rgba.resize(w * h * 4, 0);
        let cam = plan.camera(model, view, t);

        let shown = smooth((t - plan.game_in) / (plan.zoom_end - plan.game_in).max(0.01));
        if let Some(picture) = game.filter(|p| shown > 0.0 && p.w > 0 && p.h > 0 && p.rgb.len() >= p.w * p.h * 3) {
            if t >= plan.zoom_end {
                blit_rect(&mut self.rgba, w, h, picture, view.target);
                round_corners(&mut self.rgba, w, view.target, view.corners * view.target[3]);
            } else if let [Some(a), Some(b), Some(c), Some(d)] = model.picture.map(|p| cam.project(p)) {
                blit_quad(&mut self.rgba, w, h, picture, [a, b, c, d], shown as f32);
            }
        }

        let wire = 1.0 - smooth((t - plan.zoom_end) / FADE);
        if wire > 0.0 {
            self.lines.clear();
            self.dots.clear();
            gather(model, plan, &cam, t, shown, &mut self.lines, &mut self.dots);
            self.light.clear();
            self.light.resize(w * h, 0.0);
            let mut canvas = Canvas { light: &mut self.light, w, h };
            for &(a, b, i) in &self.lines {
                if let Some((a, b)) = clip(a, b, w as f64, h as f64) {
                    canvas.line(a, b, i);
                }
            }
            for &(p, i, size) in &self.dots {
                canvas.disc(p, size * view.unit, i);
            }
            let (bw, bh) = bloom(&self.light, w, h, &mut self.small, &mut self.spare);
            let wire = wire as f32;
            let beam = color.map(f32::from);
            let (glows, burns) = if light { (1.0, 0.0) } else { (2.5, 0.7) };
            for y in 0..h {
                for x in 0..w {
                    let i = self.light[y * w + x];
                    let glow = sample(&self.small, bw, bh, (x as f32 + 0.5) / 4.0 - 0.5, (y as f32 + 0.5) / 4.0 - 0.5);
                    let base = (i.min(1.0) + glow * glows) * wire;
                    let hot = (i - 1.0).max(0.0) * burns * wire * 255.0;
                    if base <= 0.0 {
                        continue;
                    }
                    // As if added to a black background: past full strength it gets brighter.
                    let lit = beam.map(|c| (c * base.max(1.0) + hot).min(255.0));
                    over(&mut self.rgba[(y * w + x) * 4..][..4], lit, base.min(1.0));
                }
            }
        }
        &self.rgba
    }
}

/// Where `p` sits while the vortex still holds it: `k` = 1 is fully caught, 0 is home. Points near
/// the axis spin faster, so lines shear and curl as they settle.
fn vortex_at(model: &Model, p: V3, k: f64, jitter: f64) -> V3 {
    let (c, r) = (model.center, model.radius);
    let (dx, dz) = (p[0] - c[0], p[2] - c[2]);
    let (d, phi) = (dx.hypot(dz), dz.atan2(dx));
    let angle = phi + k * k * PI * (4.0 + jitter * 0.3) * (r / (d + 0.4 * r));
    let d2 = d * (1.0 + 1.6 * k) + r * 0.25 * k;
    let y = p[1] + (c[1] - p[1]) * 0.7 * k - r * 0.3 * k * k;
    [c[0] + d2 * angle.cos(), y, c[2] + d2 * angle.sin()]
}

/// The lines and dots on screen at `t`, as screen positions and brightness. Lines behind the glass
/// fade as the game comes on (`shown` of the way), since they'd be seen through the picture.
fn gather(
    model: &Model,
    plan: &Plan,
    cam: &Cam,
    t: f64,
    shown: f64,
    lines: &mut Vec<(P2, P2, f32)>,
    dots: &mut Vec<(P2, f32, f64)>,
) {
    let behind = std::cell::Cell::new(1.0);
    let mut seg = |a: V3, b: V3, i: f64| {
        let i = i * behind.get();
        if i <= 0.0 {
            return;
        }
        let (mut a, mut b) = (cam.to_cam(a), cam.to_cam(b));
        if a[2] < NEAR && b[2] < NEAR {
            return;
        }
        if a[2] < NEAR {
            a = lerp(a, b, (NEAR - a[2]) / (b[2] - a[2]));
        } else if b[2] < NEAR {
            b = lerp(b, a, (NEAR - b[2]) / (a[2] - b[2]));
        }
        lines.push((cam.to_screen(a), cam.to_screen(b), i as f32));
    };
    let mut dot = |p: V3, i: f64, size: f64| {
        if let Some(s) = cam.project(p) {
            dots.push((s, i as f32, size));
        }
    };
    let r = model.radius;
    for (i, &[a, b]) in model.e.iter().enumerate() {
        let raw = (t - plan.start[i]) / plan.dur;
        if raw <= 0.0 {
            continue;
        }
        let p = raw.min(1.0);
        let e = ease_out(p);
        let (a, b) = if plan.from[i] { (model.v[b], model.v[a]) } else { (model.v[a], model.v[b]) };
        let mid = lerp(a, b, 0.5);
        behind.set(if mid[2] < model.glass[2] - BEHIND { 1.0 - shown } else { 1.0 });
        match plan.style {
            Style::Pen => {
                let tip = lerp(a, b, e);
                seg(a, tip, 1.0);
                if p < 1.0 {
                    dot(tip, 2.2, 2.2);
                }
            }
            Style::Explode => {
                let k = 1.0 - e;
                let off = mul(plan.radial[i], r * 0.9 * k);
                let turn = |q: V3| add(add(mid, rot(sub(q, mid), plan.axis[i], plan.spin[i] * k)), off);
                seg(turn(a), turn(b), (p * 1.6).min(1.0) * (1.0 + 0.8 * k));
            }
            Style::Fly => {
                let mut at = |k: f64, bright: f64| {
                    let off = mul(plan.dir[i], r * 2.4 * k);
                    let turn = |q: V3| add(add(mid, rot(sub(q, mid), plan.axis[i], plan.spin[i] * 2.0 * k)), off);
                    seg(turn(a), turn(b), bright);
                };
                let k = 1.0 - e;
                if k > 0.002 {
                    at((k + 0.14).min(1.0), 0.15);
                    at((k + 0.07).min(1.0), 0.35);
                }
                at(k, 1.0);
            }
            Style::Scan => {
                if p < 0.35 && hash2(i as u32, (t * 40.0).floor() as u32) < 0.45 {
                    continue;
                }
                seg(a, b, 1.0 + 1.6 * (1.0 - p) * (1.0 - p));
            }
            Style::Dots => {
                seg(a, lerp(a, mid, e), 1.0);
                seg(b, lerp(b, mid, e), 1.0);
            }
            Style::Vortex => {
                // Split the line so it can bend along the swirl.
                let k = 1.0 - e;
                let steps = if k > 0.01 { 6 } else { 1 };
                let bright = (p * 1.4).min(1.0) * (1.0 + 0.6 * k);
                let mut prev = vortex_at(model, a, k, plan.spin[i]);
                for s in 1..=steps {
                    let q = vortex_at(model, lerp(a, b, f64::from(s) / f64::from(steps)), k, plan.spin[i]);
                    seg(prev, q, bright);
                    prev = q;
                }
            }
        }
    }
    if plan.style == Style::Dots {
        let fade = 1.0 - smooth((t - plan.build) / 0.4);
        if fade > 0.0 {
            for (v, &start) in model.v.iter().zip(&plan.vstart) {
                let q = (t - start) / 0.25;
                if q > 0.0 {
                    let pop = (1.0 - q).max(0.0);
                    dot(*v, (1.0 + 1.5 * pop) * fade, 1.2 + 1.6 * pop);
                }
            }
        }
    }
}

/// Liang–Barsky: the part of a line inside a `w`×`h` picture.
fn clip((x0, y0): P2, (x1, y1): P2, w: f64, h: f64) -> Option<(P2, P2)> {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let (mut t0, mut t1) = (0.0, 1.0);
    for (p, q) in [(-dx, x0), (dx, w - x0), (-dy, y0), (dy, h - y0)] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let r = q / p;
        if p < 0.0 {
            if r > t1 {
                return None;
            }
            t0 = f64::max(t0, r);
        } else {
            if r < t0 {
                return None;
            }
            t1 = f64::min(t1, r);
        }
    }
    Some(((x0 + t0 * dx, y0 + t0 * dy), (x0 + t1 * dx, y0 + t1 * dy)))
}

struct Canvas<'a> {
    light: &'a mut [f32],
    w: usize,
    h: usize,
}

impl Canvas<'_> {
    fn plot(&mut self, x: i64, y: i64, i: f32) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            self.light[y as usize * self.w + x as usize] += i;
        }
    }

    /// An antialiased line (Xiaolin Wu's), adding `i` of light along it.
    fn line(&mut self, (mut x0, mut y0): P2, (mut x1, mut y1): P2, i: f32) {
        let steep = (y1 - y0).abs() > (x1 - x0).abs();
        if steep {
            std::mem::swap(&mut x0, &mut y0);
            std::mem::swap(&mut x1, &mut y1);
        }
        if x0 > x1 {
            std::mem::swap(&mut x0, &mut x1);
            std::mem::swap(&mut y0, &mut y1);
        }
        let gradient = if x1 - x0 < 1e-9 { 0.0 } else { (y1 - y0) / (x1 - x0) };
        for x in x0.floor() as i64..=x1.floor() as i64 {
            let y = y0 + gradient * (x as f64 + 0.5 - x0) - 0.5;
            let (row, frac) = (y.floor() as i64, (y - y.floor()) as f32);
            if steep {
                self.plot(row, x, i * (1.0 - frac));
                self.plot(row + 1, x, i * frac);
            } else {
                self.plot(x, row, i * (1.0 - frac));
                self.plot(x, row + 1, i * frac);
            }
        }
    }

    fn disc(&mut self, (cx, cy): P2, radius: f64, i: f32) {
        for y in (cy - radius - 1.0).floor() as i64..=(cy + radius + 1.0).ceil() as i64 {
            for x in (cx - radius - 1.0).floor() as i64..=(cx + radius + 1.0).ceil() as i64 {
                let d = (x as f64 + 0.5 - cx).hypot(y as f64 + 0.5 - cy);
                let cover = (radius + 0.5 - d).clamp(0.0, 1.0) as f32;
                if cover > 0.0 {
                    self.plot(x, y, i * cover);
                }
            }
        }
    }
}

/// A glow: the light averaged down to a quarter of the size, then blurred. Returns its size.
fn bloom(light: &[f32], w: usize, h: usize, small: &mut Vec<f32>, spare: &mut Vec<f32>) -> (usize, usize) {
    let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
    small.clear();
    small.resize(bw * bh, 0.0);
    for (y, row) in light.chunks_exact(w).enumerate() {
        for (x, &v) in row.iter().enumerate() {
            small[(y / 4) * bw + x / 4] += v / 16.0;
        }
    }
    for _ in 0..2 {
        blur(small, spare, bw, bh, 2);
    }
    (bw, bh)
}

/// A box blur of radius `r`, across then down.
fn blur(buf: &mut [f32], spare: &mut Vec<f32>, w: usize, h: usize, r: usize) {
    spare.clear();
    spare.resize(w * h, 0.0);
    let n = (2 * r + 1) as f32;
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
            spare[y * w + x] = buf[y * w + a..=y * w + b].iter().sum::<f32>() / n;
        }
    }
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
            buf[y * w + x] = (a..=b).map(|yy| spare[yy * w + x]).sum::<f32>() / n;
        }
    }
}

fn sample(buf: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let (x, y) = (x.clamp(0.0, (w - 1) as f32), y.clamp(0.0, (h - 1) as f32));
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let top = buf[y0 * w + x0] * (1.0 - fx) + buf[y0 * w + x1] * fx;
    let bottom = buf[y1 * w + x0] * (1.0 - fx) + buf[y1 * w + x1] * fx;
    top * (1.0 - fy) + bottom * fy
}

/// Draws `picture` (RGB) onto an RGBA frame stretched over `[x, y, w, h]`, the way the terminal
/// stretches a frame over cells.
fn blit_rect(rgba: &mut [u8], w: usize, h: usize, picture: Picture, [x, y, rw, rh]: [f64; 4]) {
    let (pw, ph) = (picture.w, picture.h);
    let (x0, y0) = (x.round().max(0.0) as usize, y.round().max(0.0) as usize);
    let (x1, y1) = (((x + rw).round() as usize).min(w), ((y + rh).round() as usize).min(h));
    for py in y0..y1 {
        let v = (((py as f64 + 0.5 - y) / rh * ph as f64) as usize).min(ph - 1);
        for px in x0..x1 {
            let u = (((px as f64 + 0.5 - x) / rw * pw as f64) as usize).min(pw - 1);
            let to = &mut rgba[(py * w + px) * 4..][..4];
            to[..3].copy_from_slice(&picture.rgb[(v * pw + u) * 3..][..3]);
            to[3] = 255;
        }
    }
}

/// Draws `picture` (RGB) onto the glass in an RGBA frame in perspective, `alpha` of the way in.
fn blit_quad(rgba: &mut [u8], w: usize, h: usize, picture: Picture, quad: [P2; 4], alpha: f32) {
    let (pw, ph) = (picture.w, picture.h);
    let Some(m) = invert(square_to_quad(quad)) else { return };
    let xs = quad.map(|p| p.0);
    let ys = quad.map(|p| p.1);
    let x0 = xs.iter().copied().fold(f64::INFINITY, f64::min).floor().max(0.0) as usize;
    let x1 = (xs.iter().copied().fold(f64::NEG_INFINITY, f64::max).ceil().max(0.0) as usize).min(w);
    let y0 = ys.iter().copied().fold(f64::INFINITY, f64::min).floor().max(0.0) as usize;
    let y1 = (ys.iter().copied().fold(f64::NEG_INFINITY, f64::max).ceil().max(0.0) as usize).min(h);
    for py in y0..y1 {
        for px in x0..x1 {
            let (x, y) = (px as f64 + 0.5, py as f64 + 0.5);
            let z = m[6] * x + m[7] * y + m[8];
            let (u, v) = ((m[0] * x + m[1] * y + m[2]) / z, (m[3] * x + m[4] * y + m[5]) / z);
            if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                continue;
            }
            let from = &picture.rgb[((v * ph as f64) as usize * pw + (u * pw as f64) as usize) * 3..][..3];
            over(&mut rgba[(py * w + px) * 4..][..4], [0, 1, 2].map(|k| f32::from(from[k])), alpha);
        }
    }
}

/// The projective map from the unit square onto `quad` (corners clockwise from top left), as a
/// row-major 3×3 matrix. From Heckbert's "Fundamentals of Texture Mapping".
fn square_to_quad(quad: [P2; 4]) -> [f64; 9] {
    let [(x0, y0), (x1, y1), (x2, y2), (x3, y3)] = quad;
    let (dx1, dx2, dx3) = (x1 - x2, x3 - x2, x0 - x1 + x2 - x3);
    let (dy1, dy2, dy3) = (y1 - y2, y3 - y2, y0 - y1 + y2 - y3);
    let det = dx1 * dy2 - dx2 * dy1;
    let (g, h) =
        if det.abs() < 1e-12 { (0.0, 0.0) } else { ((dx3 * dy2 - dx2 * dy3) / det, (dx1 * dy3 - dx3 * dy1) / det) };
    [x1 - x0 + g * x1, x3 - x0 + h * x3, x0, y1 - y0 + g * y1, y3 - y0 + h * y3, y0, g, h, 1.0]
}

fn invert(m: [f64; 9]) -> Option<[f64; 9]> {
    let [a, b, c, d, e, f, g, h, i] = m;
    let (ca, cb, cc) = (e * i - f * h, f * g - d * i, d * h - e * g);
    let det = a * ca + b * cb + c * cc;
    if det.abs() < 1e-12 {
        return None;
    }
    Some([
        ca / det,
        (c * h - b * i) / det,
        (b * f - c * e) / det,
        cb / det,
        (a * i - c * g) / det,
        (c * d - a * f) / det,
        cc / det,
        (b * g - a * h) / det,
        (a * e - b * d) / det,
    ])
}

/// Rounds the corners of the rectangle `[x, y, w, h]` in an RGBA picture `width` pixels wide to
/// radius `r` (all in pixels), smoothly: a pixel on the edge keeps as much of its opacity as the
/// rounded rectangle covers of it, and one outside it is clear.
pub fn round_corners(rgba: &mut [u8], width: usize, [x, y, w, h]: [f64; 4], r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    if r <= 0.0 || width == 0 {
        return;
    }
    let height = rgba.len() / 4 / width;
    let pixels =
        |from: f64, to: f64, size: usize| (from.floor().max(0.0) as usize)..(to.ceil().max(0.0) as usize).min(size);
    // Each corner: its circle's middle, and which way is out.
    for (cx, cy, sx, sy) in [
        (x + r, y + r, -1.0, -1.0),
        (x + w - r, y + r, 1.0, -1.0),
        (x + w - r, y + h - r, 1.0, 1.0),
        (x + r, y + h - r, -1.0, 1.0),
    ] {
        let xs = if sx < 0.0 { pixels(x, cx, width) } else { pixels(cx, x + w, width) };
        for py in if sy < 0.0 { pixels(y, cy, height) } else { pixels(cy, y + h, height) } {
            for px in xs.clone() {
                let (dx, dy) = (px as f64 + 0.5 - cx, py as f64 + 0.5 - cy);
                if dx * sx <= 0.0 || dy * sy <= 0.0 {
                    continue;
                }
                let inside = (r - dx.hypot(dy) + 0.5).clamp(0.0, 1.0);
                let a = &mut rgba[(py * width + px) * 4 + 3];
                *a = (f64::from(*a) * inside).round() as u8;
            }
        }
    }
}

/// Puts color `c` (0 to 255 a channel) at opacity `a` over an RGBA pixel. Kitty takes straight
/// alpha, so the color isn't scaled by its opacity.
pub fn over(px: &mut [u8], c: [f32; 3], a: f32) {
    if px[3] == 0 {
        // Most of a frame: nothing under it to blend with.
        px.copy_from_slice(&[c[0].round() as u8, c[1].round() as u8, c[2].round() as u8, (a * 255.0).round() as u8]);
        return;
    }
    let below = f32::from(px[3]) / 255.0 * (1.0 - a);
    let out = a + below;
    if out <= 0.0 {
        return;
    }
    for k in 0..3 {
        px[k] = ((c[k] * a + f32::from(px[k]) * below) / out).round().min(255.0) as u8;
    }
    px[3] = (out * 255.0).round() as u8;
}

pub fn parse_colors<'a>(list: impl IntoIterator<Item = &'a str>) -> Result<Vec<[u8; 3]>> {
    list.into_iter()
        .map(|hex| {
            let hex = hex.trim().trim_start_matches('#');
            let n = u32::from_str_radix(hex, 16).ok().filter(|_| hex.len() == 6);
            let n = n.with_context(|| format!("colors are like 9fe3ff or #ffb547, not {hex}"))?;
            Ok([(n >> 16) as u8, (n >> 8) as u8, n as u8])
        })
        .collect()
}

/// The colors a beam is picked from, and whether the background is light.
pub struct Look {
    colors: Vec<[u8; 3]>,
    /// Where the colors came from, for the example's status line.
    #[allow(dead_code)]
    pub source: &'static str,
    pub light: bool,
}

impl Look {
    /// The colors given, or else the terminal's that stand out from its background, or else romi's.
    pub fn new(listed: Option<Vec<[u8; 3]>>, terminal: TerminalColors) -> Look {
        let background = terminal.background.unwrap_or([0; 3]);
        let light = contrast(background, [255; 3]) < contrast(background, [0; 3]);
        let mut usable: Vec<[u8; 3]> = Vec::new();
        for c in terminal.palette {
            if contrast(c, background) >= CONTRAST && !usable.contains(&c) {
                usable.push(c);
            }
        }
        let (colors, source) = match listed {
            Some(list) => (list, "from --colors"),
            None if !usable.is_empty() => (usable, "from the terminal"),
            None => (DEFAULT_COLORS.to_vec(), "romi's own"),
        };
        Look { colors, source, light }
    }

    /// The seed's beam. Its color has a generator of its own, so the plan's numbers stay the
    /// browser prototype's.
    pub fn beam(&self, seed: u32) -> Beam {
        let k = Rng(seed ^ 0x2545_f491).next() * self.colors.len() as f64;
        Beam { color: self.colors[(k as usize).min(self.colors.len() - 1)], light: self.light }
    }
}

/// How the lines are drawn: their color, and whether the background is light, where they don't
/// burn white where they cross, and glow less.
#[derive(Clone, Copy)]
pub struct Beam {
    pub color: [u8; 3],
    pub light: bool,
}

/// Relative luminance, as WCAG defines it.
fn luminance(c: [u8; 3]) -> f64 {
    let linear = |v: u8| {
        let v = f64::from(v) / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * linear(c[0]) + 0.7152 * linear(c[1]) + 0.0722 * linear(c[2])
}

/// WCAG's contrast ratio, from 1 (the same) to 21 (black and white).
fn contrast(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// What the terminal says its colors are.
#[derive(Clone, Default)]
pub struct TerminalColors {
    background: Option<[u8; 3]>,
    /// The ones in `ANSI` that it answered for, in that order.
    palette: Vec<[u8; 3]>,
}

/// Asks the terminal for its background and ANSI colors (OSC 11 and OSC 4), then for its device
/// attributes, which every terminal answers, so one that ignores the color queries doesn't keep
/// this waiting. Needs raw mode, and has to run before crossterm reads any input.
pub fn ask_colors() -> TerminalColors {
    let mut ask = String::from("\x1b]11;?\x1b\\");
    for n in ANSI {
        let _ = write!(ask, "\x1b]4;{n};?\x1b\\");
    }
    ask += "\x1b[c";
    let mut out = std::io::stdout();
    if out.write_all(ask.as_bytes()).and_then(|()| out.flush()).is_err() {
        return TerminalColors::default();
    }
    let stdin = std::io::stdin();
    let (mut got, mut buf) = (Vec::new(), [0u8; 4096]);
    let deadline = Instant::now() + ASK_FOR;
    while !answered(&got) {
        let Ok(left) = Timespec::try_from(deadline.saturating_duration_since(Instant::now())) else { break };
        let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
        if !matches!(poll(&mut fds, Some(&left)), Ok(n) if n > 0) {
            break;
        }
        match rustix::io::read(&stdin, &mut buf) {
            Ok(n) if n > 0 => got.extend_from_slice(&buf[..n]),
            _ => break,
        }
    }
    read_answers(&got)
}

/// Whether the device attributes answer, `ESC [ ? 6 2 ; … c`, has come.
fn answered(got: &[u8]) -> bool {
    got.windows(3).enumerate().any(|(i, w)| {
        w == b"\x1b[?" && got[i + 3..].iter().find(|b| !(b.is_ascii_digit() || **b == b';')) == Some(&b'c')
    })
}

/// Reads answers to OSC 11 and OSC 4, `ESC ] 11 ; rgb:R/G/B` and `ESC ] 4 ; N ; rgb:R/G/B`, each
/// ended by BEL or `ESC \`.
fn read_answers(got: &[u8]) -> TerminalColors {
    let text = String::from_utf8_lossy(got);
    let (mut background, mut ansi) = (None, BTreeMap::new());
    for part in text.split("\x1b]").skip(1) {
        let body = part.split(['\x07', '\x1b']).next().unwrap_or("");
        match body.split(';').collect::<Vec<_>>()[..] {
            ["11", spec] => background = parse_rgb(spec),
            ["4", n, spec] => {
                if let (Ok(n), Some(c)) = (n.parse::<u8>(), parse_rgb(spec)) {
                    ansi.insert(n, c);
                }
            }
            _ => {}
        }
    }
    TerminalColors { background, palette: ANSI.iter().filter_map(|n| ansi.get(n).copied()).collect() }
}

/// An X11 color, `rgb:R/G/B`, with 1 to 4 hex digits a channel.
fn parse_rgb(spec: &str) -> Option<[u8; 3]> {
    let channels: Vec<u8> = spec
        .strip_prefix("rgb:")?
        .split('/')
        .map(|h| {
            let v = u32::from_str_radix(h, 16).ok().filter(|_| (1..=4).contains(&h.len()))?;
            let max = (1u32 << (4 * h.len())) - 1;
            Some(((v * 255 + max / 2) / max) as u8)
        })
        .collect::<Option<_>>()?;
    channels.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_terminals_colors_however_they_end() {
        let got = b"\x1b]11;rgb:1e1e/2e2e/3e3e\x1b\\\x1b]4;1;rgb:ff/00/00\x07\x1b]4;12;rgb:0/8/f\x1b\\\x1b[?62;22c";
        assert!(answered(got));
        let colors = read_answers(got);
        assert_eq!(colors.background, Some([0x1e, 0x2e, 0x3e]));
        // In ANSI's order, whatever order they came in.
        assert_eq!(colors.palette, [[255, 0, 0], [0, 0x88, 255]]);
        assert!(!answered(b"\x1b]11;rgb:0/0/0\x07\x1b[?62;22"));
    }

    #[test]
    fn keeps_the_terminals_colors_that_stand_out() {
        let terminal = || TerminalColors {
            background: Some([0x28, 0x2a, 0x36]),
            palette: vec![[0xff, 0x55, 0x55], [0x30, 0x30, 0x40], [0xff, 0x55, 0x55], [0x8b, 0xe9, 0xfd]],
        };
        let look = Look::new(None, terminal());
        assert_eq!(look.colors, [[0xff, 0x55, 0x55], [0x8b, 0xe9, 0xfd]]);
        assert!(!look.light);
        assert_eq!(look.beam(42).color, look.beam(42).color);
        assert_eq!(Look::new(Some(vec![[1, 2, 3]]), terminal()).colors, [[1, 2, 3]]);
        assert_eq!(Look::new(None, TerminalColors::default()).colors, DEFAULT_COLORS);
        assert!(Look::new(None, TerminalColors { background: Some([0xfa; 3]), palette: vec![] }).light);
    }

    #[test]
    fn a_picture_not_the_glasss_shape_sits_in_its_middle() {
        let mut model = Model::load(
            r#"{"display_aspect":[4,3],"screen":[0,1,2,3],"vertices":[[-4,6,0],[4,6,0],[4,0,0],[-4,0,0]],"edges":[[0,1]]}"#,
        )
        .unwrap();
        // A wide game, as wide as the glass.
        model.fit((3.0, 2.0));
        let (top, bottom) = (3.0 + 8.0 / 3.0, 3.0 - 8.0 / 3.0);
        let want = [[-4.0, top, 0.0], [4.0, top, 0.0], [4.0, bottom, 0.0], [-4.0, bottom, 0.0]];
        assert!(model.picture.iter().zip(&want).all(|(p, q)| len(sub(*p, *q)) < 1e-9), "{:?}", model.picture);
        // A tall one, as tall.
        model.fit((3.0, 4.0));
        assert_eq!(model.picture[0], [-2.25, 6.0, 0.0]);
        assert_eq!((model.glass, model.glass_h), ([0.0, 3.0, 0.0], 6.0));
    }

    #[test]
    fn rounded_corners_clear_the_corners_and_keep_the_edges() {
        let (w, h) = (40, 30);
        let mut rgba = vec![255; w * h * 4];
        round_corners(&mut rgba, w, [0.0, 0.0, w as f64, h as f64], 10.0);
        let alpha = |x: usize, y: usize| rgba[(y * w + x) * 4 + 3];
        for (x, y) in [(0, 0), (w - 1, 0), (w - 1, h - 1), (0, h - 1)] {
            assert_eq!(alpha(x, y), 0, "corner ({x}, {y})");
        }
        // The middle of each edge, and the middle, stay.
        assert_eq!([alpha(w / 2, 0), alpha(0, h / 2), alpha(w / 2, h - 1), alpha(w / 2, h / 2)], [255; 4]);
        // Along the curve, partly.
        assert!((1..255).contains(&alpha(2, 3)), "{}", alpha(2, 3));
        assert_eq!(rgba.iter().step_by(4).filter(|&&c| c != 255).count(), 0, "colors are left alone");
    }

    #[test]
    fn every_system_has_a_wireframe() {
        for system in ["gb", "nes", "snes", "arcade", "gba"] {
            Model::load(for_system(system)).unwrap();
        }
    }

    #[test]
    fn color_over_clear_keeps_its_color() {
        let mut px = [0, 0, 0, 0];
        over(&mut px, [200.0, 100.0, 50.0], 0.5);
        assert_eq!(px, [200, 100, 50, 128]);
        over(&mut px, [0.0, 0.0, 0.0], 1.0);
        assert_eq!(px, [0, 0, 0, 255]);
    }

    #[test]
    fn colors_are_six_hex_digits() {
        assert_eq!(parse_colors(["9fe3ff", " #ffb547"]).unwrap(), [[0x9f, 0xe3, 0xff], [0xff, 0xb5, 0x47]]);
        assert!(parse_colors(["fff"]).is_err());
    }
}

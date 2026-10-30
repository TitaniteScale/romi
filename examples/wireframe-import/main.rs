//! Turns a 3D model (OBJ, STL, glTF or GLB) into a wireframe for the launch animation in
//! `examples/wireframe`.
//!
//! A downloaded model is made of triangles, often thousands of them, and drawing every edge gives a
//! mesh rather than a drawing. This keeps the edges a pen would draw: where the surface folds by
//! more than `--angle`, and where it ends. It joins lines that run straight on, sizes the model in
//! cm, stands it with y up and its front toward +z, and finds the screen's glass.
//!
//! ```text
//! cargo run --release --example wireframe-import -- tv.stl -o tv.json
//! cargo run --release --example wireframe -- --model tv.json
//! ```
//!
//! The glass is the biggest smooth patch that faces front and has one outline, which is how glass
//! set into a bezel looks. The candidates are listed, and `--glass` picks another. The picture is
//! the biggest rectangle of the display's shape that fits inside the glass's outline.

use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use clap::{Parser, ValueEnum};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

type V3 = [f64; 3];

/// Vertices closer than this, as a fraction of the model's size, are one vertex.
const WELD: f64 = 1e-5;
/// Lines that bend by less than this many degrees where they meet are joined into one.
const STRAIGHT: f64 = 1.5;
/// How far a surface's normal can be from straight ahead for it to be a candidate for the glass.
const FACING: f64 = 45.0;
/// How much a candidate for the glass counts for less when it isn't the picture's shape: one 20%
/// wider is worth half as much.
const SHAPE: f64 = 20.0;
/// Faces within this many degrees of each other are one flat side.
const FLAT: f64 = 1.0;
/// The picture's size as a fraction of the biggest rectangle that fits inside the glass's outline,
/// so its lines sit just inside the glass's rather than on top of them.
const INSET: f64 = 0.96;
const CANDIDATES_SHOWN: usize = 5;

#[derive(Parser)]
#[command(about = "Turns a 3D model into a wireframe for romi's launch animation")]
struct Args {
    /// The model: .obj, .stl, .gltf or .glb.
    input: PathBuf,
    /// Where to write the wireframe (to stdout if not given).
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Keep edges where the surface folds by more than this many degrees. Raise it if curved parts
    /// come out as bands of lines.
    #[arg(long, default_value_t = 30.0)]
    angle: f64,
    /// The model's units [default: mm for STL, m for the rest].
    #[arg(long, value_enum)]
    unit: Option<Unit>,
    /// Scale the model to this width in cm instead, left to right as seen from the front.
    #[arg(long, conflicts_with_all = ["unit", "height"])]
    width: Option<f64>,
    /// Scale the model to this height in cm instead.
    #[arg(long, conflicts_with = "unit")]
    height: Option<f64>,
    /// The model's up axis: x, y, z, -x, -y or -z [default: z for STL, y for the rest].
    #[arg(long, allow_hyphen_values = true)]
    up: Option<Axis>,
    /// The axis the model's front faces [default: -y for STL, z for the rest].
    #[arg(long, allow_hyphen_values = true)]
    front: Option<Axis>,
    /// Turn the model this many degrees about its up axis, counterclockwise seen from above.
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    turn: f64,
    /// Drop pieces smaller than this many cm across, like screw holes and lettering.
    #[arg(long, default_value_t = 0.0)]
    min_part: f64,
    /// Also draw where a flat side bends into a curve, like the two ends of a rounded corner, for
    /// sides bigger than this percent of the model's surface. 0 turns it off.
    #[arg(long, default_value_t = 2.0)]
    flat: f64,
    /// Which candidate is the glass, by its number in the list printed.
    #[arg(long, default_value_t = 1)]
    glass: usize,
    /// The model has no screen, like a console to put beside a TV with --add.
    #[arg(long, conflicts_with = "glass")]
    no_glass: bool,
    /// The shape of the picture on the glass, as W:H.
    #[arg(long, default_value = "4:3")]
    aspect: String,
    /// Put another wireframe made by this into this one: FILE:X,Z[,DEGREES], with X and Z in cm
    /// from the glass's center and DEGREES counterclockwise seen from above. Repeatable.
    #[arg(long, value_name = "FILE:X,Z[,DEGREES]")]
    add: Vec<String>,
    /// The system this is for, as named in romi's config [default: the file's name].
    #[arg(long)]
    system: Option<String>,
    /// Who made the model and its license, kept in the file for attribution.
    #[arg(long)]
    credit: Option<String>,
}

#[derive(Clone, Copy, ValueEnum)]
enum Unit {
    Mm,
    Cm,
    M,
    In,
}

impl Unit {
    fn cm(self) -> f64 {
        match self {
            Unit::Mm => 0.1,
            Unit::Cm => 1.0,
            Unit::M => 100.0,
            Unit::In => 2.54,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Axis {
    k: usize,
    sign: f64,
}

impl Axis {
    const fn new(k: usize, sign: f64) -> Axis {
        Axis { k, sign }
    }
    fn vec(self) -> V3 {
        let mut v = [0.0; 3];
        v[self.k] = self.sign;
        v
    }
}

impl FromStr for Axis {
    type Err = String;
    fn from_str(s: &str) -> Result<Axis, String> {
        let (sign, name) = match s.strip_prefix('-') {
            Some(rest) => (-1.0, rest),
            None => (1.0, s.strip_prefix('+').unwrap_or(s)),
        };
        let k = match name {
            "x" => 0,
            "y" => 1,
            "z" => 2,
            _ => return Err(format!("{s} isn't an axis: use x, y, z, -x, -y or -z")),
        };
        Ok(Axis::new(k, sign))
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Obj,
    Stl,
    Gltf,
    Glb,
}

impl Kind {
    fn of(path: &Path) -> Result<Kind> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        Ok(match ext.as_str() {
            "obj" => Kind::Obj,
            "stl" => Kind::Stl,
            "gltf" => Kind::Gltf,
            "glb" => Kind::Glb,
            _ => bail!("{} isn't an .obj, .stl, .gltf or .glb file", path.display()),
        })
    }

    /// What files of this kind usually are: units, up and front. STL comes from 3D printing, in mm
    /// with z up; glTF is in m with y up and its front toward +z; and so is OBJ out of Blender.
    fn conventions(self) -> (Unit, Axis, Axis) {
        match self {
            Kind::Stl => (Unit::Mm, Axis::new(2, 1.0), Axis::new(1, -1.0)),
            _ => (Unit::M, Axis::new(1, 1.0), Axis::new(2, 1.0)),
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let kind = Kind::of(&args.input)?;
    let aspect = parse_aspect(&args.aspect)?;
    let mut mesh = match kind {
        Kind::Obj => read_obj(&fs::read_to_string(&args.input).context("reading the model")?)?,
        Kind::Stl => read_stl(&fs::read(&args.input).context("reading the model")?)?,
        Kind::Gltf | Kind::Glb => read_gltf(&args.input, kind)?,
    };
    mesh.check()?;
    ensure!(!mesh.faces.is_empty() || !mesh.lines.is_empty(), "the model has no faces or lines");

    let (unit, up, front) = kind.conventions();
    let unit = args.unit.unwrap_or(unit);
    orient(&mut mesh.v, args.up.unwrap_or(up), args.front.unwrap_or(front), args.turn, unit.cm())?;
    if let Some((cm, k)) = args.width.map(|w| (w, 0)).or(args.height.map(|h| (h, 1))) {
        let (lo, hi) = bounds(&mesh.v);
        ensure!(hi[k] > lo[k], "the model is flat, so it can't be scaled to a size");
        let k = cm / (hi[k] - lo[k]);
        mesh.v.iter_mut().for_each(|p| *p = scale(*p, k));
    }
    let faces_read = mesh.faces.len();
    weld(&mut mesh);
    let (lo, hi) = bounds(&mesh.v);
    let size = sub(hi, lo);
    eprintln!("read {}: {faces_read} faces, {} vertices once joined", args.input.display(), mesh.v.len());
    eprintln!("size {:.1} × {:.1} × {:.1} cm (width × height × depth)", size[0], size[1], size[2]);
    if size[0].max(size[1]) > 400.0 || size[0].max(size[1]) < 2.0 {
        eprintln!("  that looks wrong for a console or a TV: check --unit, or set --width or --height");
    }

    let surface = Surface::new(&mesh);
    let cos = args.angle.to_radians().cos();
    let mut edges: BTreeSet<[usize; 2]> = surface.folds(cos).collect();
    edges.extend(surface.bends(args.flat / 100.0));
    edges.extend(mesh.lines.iter().map(|&[a, b]| key(a, b)));
    let mut screen = None;
    if !args.no_glass {
        let candidates = surface.glass(&mesh, cos, aspect);
        if candidates.is_empty() {
            bail!(
                "nothing facing front looks like glass. Check --up and --front, raise --angle if the \
                 glass is curved, or use --no-glass for a model without a screen"
            );
        }
        let pick = args.glass;
        ensure!((1..=candidates.len()).contains(&pick), "--glass {pick}: there are {} candidates", candidates.len());
        eprintln!("glass candidates (facing front, one outline):");
        for (i, c) in candidates.iter().enumerate().take(CANDIDATES_SHOWN.max(pick)) {
            let (lo, hi) = bounds(&c.outline.iter().flatten().map(|&i| mesh.v[i]).collect::<Vec<_>>());
            eprintln!(
                "  {}. {:.1} × {:.1} cm, {:.0} cm², centered {:.1} cm up{}",
                i + 1,
                c.size.0,
                c.size.1,
                c.area,
                (lo[1] + hi[1]) / 2.0 - lo_y(&mesh.v),
                if i + 1 == pick { "   ← the glass" } else { "" }
            );
        }
        // Face the glass straight on, since the camera lands looking down -z.
        let glass = &candidates[pick - 1];
        // The game covers the glass, so nothing on it is drawn.
        let on_glass: BTreeSet<usize> = glass.faces.iter().copied().collect();
        edges.retain(|e| !surface.edges.get(e).is_some_and(|f| f.iter().all(|f| on_glass.contains(f))));
        let yaw = glass.normal[0].atan2(glass.normal[2]);
        mesh.v.iter_mut().for_each(|p| *p = turn(*p, -yaw));
        screen = Some(fit_picture(&mesh.v, &glass.outline, aspect));
    }

    let folds = edges.len();
    let edges = straighten(&mesh.v, edges);
    let joined = edges.len();
    let edges = drop_small(&mesh.v, edges, args.min_part);
    if edges.is_empty() && screen.is_none() {
        bail!("no lines are left: try a smaller --angle or --min-part");
    }
    eprintln!("lines: {folds} folds, ends and bends, {joined} once straight runs are joined, {} kept", edges.len());
    let mut wire = Wire::new(&mesh.v, &edges, screen);
    for spec in &args.add {
        wire.add(spec)?;
    }
    if wire.e.len() > 2000 {
        eprintln!("  that's a lot of lines to draw: try a bigger --angle or --min-part");
    }

    let system = match &args.system {
        Some(s) => s.clone(),
        None => args.input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
    };
    let json = wire.to_json(&system, aspect, args.credit.as_deref());
    match &args.output {
        Some(path) => {
            fs::write(path, json).with_context(|| format!("writing {}", path.display()))?;
            eprintln!("wrote {}: {} vertices, {} lines", path.display(), wire.v.len(), wire.e.len());
        }
        None => print!("{json}"),
    }
    Ok(())
}

fn parse_aspect(s: &str) -> Result<(f64, f64)> {
    let parsed = s.split_once(':').and_then(|(w, h)| Some((w.trim().parse().ok()?, h.trim().parse().ok()?)));
    match parsed {
        Some((w, h)) if w > 0.0 && h > 0.0 => Ok((w, h)),
        _ => bail!("--aspect {s}: expected W:H, like 4:3"),
    }
}

// ---------- reading models ----------

#[derive(Default)]
struct Mesh {
    v: Vec<V3>,
    /// Polygons, as vertex numbers in order around them.
    faces: Vec<Vec<usize>>,
    /// Lines the file draws itself (OBJ's `l`), kept whatever the angle.
    lines: Vec<[usize; 2]>,
}

impl Mesh {
    fn check(&self) -> Result<()> {
        let n = self.v.len();
        if let Some(bad) = self.faces.iter().flatten().chain(self.lines.iter().flatten()).find(|&&i| i >= n) {
            bail!("the model names vertex {}, but only has {n}", bad + 1);
        }
        Ok(())
    }
}

fn read_obj(text: &str) -> Result<Mesh> {
    let mut mesh = Mesh::default();
    for (n, line) in text.lines().enumerate() {
        let at = || format!("line {} of the OBJ file", n + 1);
        let mut words = line.split_whitespace();
        let tag = words.next();
        // Numbers count from 1, and back from the latest vertex when negative.
        let count = mesh.v.len() as i64;
        let index = |w: &str| -> Result<usize> {
            let i: i64 = w.split('/').next().unwrap_or("").parse().with_context(at)?;
            Ok(match i {
                0 => bail!("{}: vertex 0", at()),
                i if i < 0 => usize::try_from(count + i).ok().with_context(|| format!("{}: vertex {i}", at()))?,
                i => (i - 1) as usize,
            })
        };
        match tag {
            Some("v") => {
                let p: Vec<f64> = words.take(3).map(str::parse).collect::<Result<_, _>>().with_context(at)?;
                ensure!(p.len() == 3, "{}: a vertex needs x, y and z", at());
                mesh.v.push([p[0], p[1], p[2]]);
            }
            Some("f") => {
                let face = words.map(index).collect::<Result<Vec<_>>>()?;
                if face.len() >= 3 {
                    mesh.faces.push(face);
                }
            }
            Some("l") => {
                let points = words.map(index).collect::<Result<Vec<_>>>()?;
                mesh.lines.extend(points.windows(2).map(|w| [w[0], w[1]]));
            }
            _ => {}
        }
    }
    Ok(mesh)
}

fn read_stl(bytes: &[u8]) -> Result<Mesh> {
    let mut mesh = Mesh::default();
    // Binary: an 80 byte header, a count, then 50 bytes a triangle. ASCII files can start with
    // "solid" too, so the size is what tells them apart.
    if let Some(count) = bytes.get(80..84).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
        && bytes.len() == 84 + count * 50
    {
        for t in bytes[84..].as_chunks::<50>().0 {
            let f = |i: usize| f64::from(f32::from_le_bytes(t[i..i + 4].try_into().unwrap()));
            let base = mesh.v.len();
            mesh.v.extend((0..3).map(|c| [f(12 + c * 12), f(16 + c * 12), f(20 + c * 12)]));
            mesh.faces.push(vec![base, base + 1, base + 2]);
        }
        return Ok(mesh);
    }
    let text = String::from_utf8_lossy(bytes);
    for line in text.lines() {
        let mut words = line.split_whitespace();
        if words.next() == Some("vertex") {
            let p: Vec<f64> = words.take(3).map(str::parse).collect::<Result<_, _>>().context("reading a vertex")?;
            ensure!(p.len() == 3, "a vertex in the STL file needs x, y and z");
            mesh.v.push([p[0], p[1], p[2]]);
        }
    }
    ensure!(mesh.v.len() % 3 == 0, "the STL file's vertices don't come in threes");
    mesh.faces = (0..mesh.v.len() / 3).map(|t| vec![t * 3, t * 3 + 1, t * 3 + 2]).collect();
    Ok(mesh)
}

/// A 4×4 matrix, column by column, as glTF stores them.
type M4 = [f64; 16];
const IDENTITY: M4 = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.];

fn read_gltf(path: &Path, kind: Kind) -> Result<Mesh> {
    let bytes = fs::read(path).context("reading the model")?;
    let (doc, bin): (Value, Option<Vec<u8>>) = if kind == Kind::Glb {
        ensure!(bytes.starts_with(b"glTF"), "{} isn't a GLB file", path.display());
        let (mut json, mut bin, mut at) = (None, None, 12);
        while at + 8 <= bytes.len() {
            let word = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
            let (n, tag) = (word(at) as usize, word(at + 4));
            let data = bytes.get(at + 8..at + 8 + n).context("the GLB file is cut short")?;
            match tag {
                0x4E4F534A => json = Some(serde_json::from_slice(data).context("reading the GLB file's JSON")?),
                0x004E4942 => bin = Some(data.to_vec()),
                _ => {}
            }
            at += 8 + n;
        }
        (json.context("the GLB file has no JSON")?, bin)
    } else {
        (serde_json::from_slice(&bytes).context("reading the glTF file")?, None)
    };
    if let Some(ext) = doc["extensionsRequired"].as_array().and_then(|e| e.first()) {
        bail!("the model needs the {ext} extension, which this can't read: export it again without compression");
    }

    let dir = path.parent().unwrap_or(Path::new("."));
    let mut buffers = Vec::new();
    for (i, b) in doc["buffers"].as_array().into_iter().flatten().enumerate() {
        buffers.push(match b["uri"].as_str() {
            None => bin.clone().with_context(|| format!("buffer {i} has no data"))?,
            Some(uri) if uri.starts_with("data:") => {
                let data = uri.split_once(',').map_or("", |(_, d)| d);
                BASE64.decode(data).with_context(|| format!("decoding buffer {i}"))?
            }
            Some(uri) => {
                let file = dir.join(percent_decode(uri));
                fs::read(&file).with_context(|| format!("reading {}", file.display()))?
            }
        });
    }
    let gltf = Gltf { doc: &doc, buffers };

    let mut mesh = Mesh::default();
    let mut skipped = 0;
    let nodes = doc["nodes"].as_array().map_or(&[][..], Vec::as_slice);
    if nodes.is_empty() {
        for m in 0..doc["meshes"].as_array().map_or(0, Vec::len) {
            skipped += gltf.add_mesh(m, &IDENTITY, &mut mesh)?;
        }
    } else {
        let scene = &doc["scenes"][doc["scene"].as_u64().unwrap_or(0) as usize];
        let roots: Vec<usize> = match scene["nodes"].as_array() {
            Some(roots) => roots.iter().filter_map(|n| n.as_u64().map(|n| n as usize)).collect(),
            None => {
                let children: BTreeSet<u64> = nodes
                    .iter()
                    .flat_map(|n| n["children"].as_array().into_iter().flatten())
                    .filter_map(Value::as_u64)
                    .collect();
                (0..nodes.len()).filter(|&n| !children.contains(&(n as u64))).collect()
            }
        };
        for root in roots {
            skipped += gltf.add_node(root, &IDENTITY, &mut mesh, 0)?;
        }
    }
    if skipped > 0 {
        eprintln!("skipped {skipped} parts made of points, lines or strips rather than triangles");
    }
    Ok(mesh)
}

struct Gltf<'a> {
    doc: &'a Value,
    buffers: Vec<Vec<u8>>,
}

impl Gltf<'_> {
    /// Adds a node's meshes and its children's, and returns how many parts it skipped.
    fn add_node(&self, n: usize, parent: &M4, mesh: &mut Mesh, depth: usize) -> Result<usize> {
        ensure!(depth < 64, "the glTF file's nodes go round in a loop");
        let node = &self.doc["nodes"][n];
        let local = match node["matrix"].as_array() {
            Some(m) => numbers::<16>(m).context("reading a node's matrix")?,
            None => {
                let t = node["translation"].as_array().map(|a| numbers::<3>(a)).transpose()?.unwrap_or([0.0; 3]);
                let r =
                    node["rotation"].as_array().map(|a| numbers::<4>(a)).transpose()?.unwrap_or([0.0, 0.0, 0.0, 1.0]);
                let s = node["scale"].as_array().map(|a| numbers::<3>(a)).transpose()?.unwrap_or([1.0; 3]);
                trs(t, r, s)
            }
        };
        let m = mul(parent, &local);
        let mut skipped = 0;
        if let Some(i) = node["mesh"].as_u64() {
            skipped += self.add_mesh(i as usize, &m, mesh)?;
        }
        for child in node["children"].as_array().into_iter().flatten().filter_map(Value::as_u64) {
            skipped += self.add_node(child as usize, &m, mesh, depth + 1)?;
        }
        Ok(skipped)
    }

    fn add_mesh(&self, i: usize, m: &M4, mesh: &mut Mesh) -> Result<usize> {
        let mut skipped = 0;
        for p in self.doc["meshes"][i]["primitives"].as_array().into_iter().flatten() {
            if p["mode"].as_u64().unwrap_or(4) != 4 {
                skipped += 1;
                continue;
            }
            let at = p["attributes"]["POSITION"].as_u64().context("a part of the model has no positions")? as usize;
            ensure!(self.doc["accessors"][at]["componentType"] == 5126, "the model's positions aren't floats");
            let pos = self.read(at, 3)?;
            let base = mesh.v.len();
            mesh.v.extend(pos.as_chunks::<3>().0.iter().map(|&c| apply(m, c)));
            let count = pos.len() / 3;
            let order: Vec<usize> = match p["indices"].as_u64() {
                Some(at) => self.read(at as usize, 1)?.into_iter().map(|i| i as usize).collect(),
                None => (0..count).collect(),
            };
            ensure!(order.iter().all(|&i| i < count), "a part of the model names a vertex it doesn't have");
            mesh.faces.extend(order.as_chunks::<3>().0.iter().map(|t| t.iter().map(|&i| base + i).collect()));
        }
        Ok(skipped)
    }

    /// An accessor's numbers, `width` to an element.
    fn read(&self, i: usize, width: usize) -> Result<Vec<f64>> {
        let a = &self.doc["accessors"][i];
        ensure!(a.get("sparse").is_none(), "accessor {i} is sparse, which this can't read");
        let want = if width == 3 { "VEC3" } else { "SCALAR" };
        ensure!(a["type"] == want, "accessor {i} isn't a {want}");
        let count = a["count"].as_u64().with_context(|| format!("accessor {i} has no count"))? as usize;
        let kind = a["componentType"].as_u64().unwrap_or(0);
        let size = match kind {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            _ => bail!("accessor {i} has numbers of a type this doesn't know"),
        };
        let Some(view) = a["bufferView"].as_u64() else { return Ok(vec![0.0; count * width]) };
        let view = &self.doc["bufferViews"][view as usize];
        let buffer = self.buffers.get(view["buffer"].as_u64().unwrap_or(0) as usize).context("a missing buffer")?;
        let start = (view["byteOffset"].as_u64().unwrap_or(0) + a["byteOffset"].as_u64().unwrap_or(0)) as usize;
        let stride = view["byteStride"].as_u64().map_or(size * width, |s| s as usize);
        let mut out = Vec::with_capacity(count * width);
        for e in 0..count {
            for c in 0..width {
                let at = start + e * stride + c * size;
                let b = buffer.get(at..at + size).with_context(|| format!("accessor {i} runs past its buffer"))?;
                out.push(match kind {
                    5120 => f64::from(b[0] as i8),
                    5121 => f64::from(b[0]),
                    5122 => f64::from(i16::from_le_bytes([b[0], b[1]])),
                    5123 => f64::from(u16::from_le_bytes([b[0], b[1]])),
                    5125 => f64::from(u32::from_le_bytes(b.try_into().unwrap())),
                    _ => f64::from(f32::from_le_bytes(b.try_into().unwrap())),
                });
            }
        }
        Ok(out)
    }
}

fn numbers<const N: usize>(a: &[Value]) -> Result<[f64; N]> {
    ensure!(a.len() == N, "expected {N} numbers, not {}", a.len());
    let mut out = [0.0; N];
    for (o, v) in out.iter_mut().zip(a) {
        *o = v.as_f64().context("expected a number")?;
    }
    Ok(out)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let (mut out, mut i) = (Vec::with_capacity(b.len()), 0);
    while i < b.len() {
        let hex =
            b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn trs(t: V3, [x, y, z, w]: [f64; 4], s: V3) -> M4 {
    #[rustfmt::skip]
    let m = [
        (1.0 - 2.0 * (y * y + z * z)) * s[0], 2.0 * (x * y + z * w) * s[0], 2.0 * (x * z - y * w) * s[0], 0.0,
        2.0 * (x * y - z * w) * s[1], (1.0 - 2.0 * (x * x + z * z)) * s[1], 2.0 * (y * z + x * w) * s[1], 0.0,
        2.0 * (x * z + y * w) * s[2], 2.0 * (y * z - x * w) * s[2], (1.0 - 2.0 * (x * x + y * y)) * s[2], 0.0,
        t[0], t[1], t[2], 1.0,
    ];
    m
}

fn mul(a: &M4, b: &M4) -> M4 {
    let mut m = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            m[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    m
}

fn apply(m: &M4, p: V3) -> V3 {
    [0, 1, 2].map(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r])
}

// ---------- shaping the mesh ----------

/// Stands the model with y up and its front toward +z, turned `turn` degrees, in cm.
fn orient(v: &mut [V3], up: Axis, front: Axis, turn_by: f64, cm: f64) -> Result<()> {
    ensure!(up.k != front.k, "--up and --front have to be different axes");
    let (u, f) = (up.vec(), front.vec());
    let x = cross(u, f);
    let a = turn_by.to_radians();
    for p in v {
        *p = scale(turn([dot(*p, x), dot(*p, u), dot(*p, f)], a), cm);
    }
    Ok(())
}

/// Turns a point about the y axis, counterclockwise seen from above: +z toward +x.
fn turn(p: V3, a: f64) -> V3 {
    let (s, c) = a.sin_cos();
    [p[0] * c + p[2] * s, p[1], p[2] * c - p[0] * s]
}

/// Joins vertices in the same place, which STL files and seams in other files repeat, so faces
/// that touch share edges. Then drops faces that are left with no area, and faces given twice.
fn weld(mesh: &mut Mesh) {
    let (lo, hi) = bounds(&mesh.v);
    let eps = (len(sub(hi, lo)) * WELD).max(1e-9);
    let cell = |p: V3| p.map(|c| (c / eps).floor() as i64);
    let mut grid: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    let mut kept: Vec<V3> = Vec::new();
    let mut remap = Vec::with_capacity(mesh.v.len());
    for &p in &mesh.v {
        let c = cell(p);
        let mut near = None;
        'look: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for &j in grid.get(&[c[0] + dx, c[1] + dy, c[2] + dz]).into_iter().flatten() {
                        if len(sub(kept[j], p)) <= eps {
                            near = Some(j);
                            break 'look;
                        }
                    }
                }
            }
        }
        remap.push(near.unwrap_or_else(|| {
            kept.push(p);
            grid.entry(c).or_default().push(kept.len() - 1);
            kept.len() - 1
        }));
    }
    mesh.v = kept;
    let mut seen = BTreeSet::new();
    let faces = std::mem::take(&mut mesh.faces);
    for face in faces {
        let mut f: Vec<usize> = face.iter().map(|&i| remap[i]).collect();
        f.dedup();
        while f.len() > 1 && f.first() == f.last() {
            f.pop();
        }
        let mut sorted = f.clone();
        sorted.sort_unstable();
        if f.len() >= 3 && seen.insert(sorted) {
            mesh.faces.push(f);
        }
    }
    mesh.lines = mesh.lines.iter().map(|l| l.map(|i| remap[i])).filter(|[a, b]| a != b).collect();
}

/// The mesh's faces and which faces meet at each edge.
struct Surface {
    /// Each face's normal, as a unit vector.
    normal: Vec<V3>,
    area: Vec<f64>,
    /// The faces along each edge, by its vertices lowest first. Faces with no area are left out.
    edges: HashMap<[usize; 2], Vec<usize>>,
}

impl Surface {
    fn new(mesh: &Mesh) -> Surface {
        let mut s = Surface { normal: Vec::new(), area: Vec::new(), edges: HashMap::new() };
        for (i, face) in mesh.faces.iter().enumerate() {
            // Newell's method: the normal of a polygon that may not be quite flat, times twice its area.
            let mut n = [0.0; 3];
            for (k, &a) in face.iter().enumerate() {
                let (p, q) = (mesh.v[a], mesh.v[face[(k + 1) % face.len()]]);
                n[0] += (p[1] - q[1]) * (p[2] + q[2]);
                n[1] += (p[2] - q[2]) * (p[0] + q[0]);
                n[2] += (p[0] - q[0]) * (p[1] + q[1]);
            }
            let twice = len(n);
            s.area.push(twice / 2.0);
            s.normal.push(if twice > 1e-12 { scale(n, 1.0 / twice) } else { [0.0; 3] });
            if twice > 1e-12 {
                for (k, &a) in face.iter().enumerate() {
                    s.edges.entry(key(a, face[(k + 1) % face.len()])).or_default().push(i);
                }
            }
        }
        s
    }

    /// The edges where faces meet at more than the angle whose cosine is `cos`, and where an open
    /// surface ends, which a drawing shows too.
    fn folds(&self, cos: f64) -> impl Iterator<Item = [usize; 2]> + '_ {
        self.edges.iter().filter(move |(_, f)| self.is_fold(f, cos)).map(|(&e, _)| e)
    }

    /// Edges where a flat side bends into a curve, like the two ends of a rounded corner. A curve
    /// has no folds, so without these a rounded box loses the lines along its corners. Only sides
    /// bigger than `share` of the whole surface count, so the strips a curve is made of don't.
    fn bends(&self, share: f64) -> Vec<[usize; 2]> {
        if share <= 0.0 {
            return Vec::new();
        }
        let n = self.normal.len();
        let mut next = vec![Vec::new(); n];
        for faces in self.edges.values() {
            if let [a, b] = faces[..] {
                next[a].push(b);
                next[b].push(a);
            }
        }
        next.iter_mut().for_each(|n| n.sort_unstable());
        // Each side grows from one face and takes in faces that face its way. Comparing faces with
        // their neighbours instead would let a gentle curve, a little at a time, count as flat.
        let level = FLAT.to_radians().cos();
        let (mut side, mut area) = (vec![usize::MAX; n], Vec::new());
        for seed in 0..n {
            if side[seed] != usize::MAX || self.area[seed] == 0.0 {
                continue;
            }
            side[seed] = area.len();
            let (mut total, mut stack) = (0.0, vec![seed]);
            while let Some(f) = stack.pop() {
                total += self.area[f];
                for &g in &next[f] {
                    if side[g] == usize::MAX && dot(self.normal[g], self.normal[seed]) >= level {
                        side[g] = area.len();
                        stack.push(g);
                    }
                }
            }
            area.push(total);
        }
        let big = share * self.area.iter().sum::<f64>();
        self.edges
            .iter()
            .filter_map(|(&e, faces)| {
                let [a, b] = faces[..] else { return None };
                let (a, b) = (side[a], side[b]);
                (a != b && area[a].max(area[b]) >= big).then_some(e)
            })
            .collect()
    }

    fn is_fold(&self, faces: &[usize], cos: f64) -> bool {
        faces.len() == 1
            || faces
                .iter()
                .enumerate()
                .any(|(i, &a)| faces[i + 1..].iter().any(|&b| dot(self.normal[a], self.normal[b]) < cos))
    }

    /// Smooth patches facing front with a single outline, the likeliest to be the glass first: the
    /// biggest, but counting shape too, since a model's front is often a flat panel bigger than its
    /// glass. The glass is shaped like the picture, `aspect`.
    fn glass(&self, mesh: &Mesh, cos: f64, (aw, ah): (f64, f64)) -> Vec<Glass> {
        let mut patches = Sets::new(mesh.faces.len());
        for faces in self.edges.values() {
            if faces.len() == 2 && !self.is_fold(faces, cos) {
                patches.join(faces[0], faces[1]);
            }
        }
        let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for f in 0..mesh.faces.len() {
            if self.area[f] > 0.0 {
                groups.entry(patches.find(f)).or_default().push(f);
            }
        }
        let facing = FACING.to_radians().cos();
        let mut out: Vec<Glass> = groups
            .values()
            .filter_map(|faces| {
                let sum = faces.iter().fold([0.0; 3], |n, &f| add(n, scale(self.normal[f], self.area[f])));
                let normal = norm(sum);
                (normal[2] >= facing).then(|| Glass {
                    area: faces.iter().map(|&f| self.area[f]).sum(),
                    normal,
                    size: (0.0, 0.0),
                    outline: Vec::new(),
                    faces: faces.clone(),
                })
            })
            .collect();
        out.sort_by(|a, b| b.area.total_cmp(&a.area));
        // Outlines only for the biggest few, since a detailed model has thousands of patches.
        out.truncate(40);
        for g in &mut out {
            let mut count: BTreeMap<[usize; 2], u32> = BTreeMap::new();
            for &f in &g.faces {
                let face = &mesh.faces[f];
                for (k, &a) in face.iter().enumerate() {
                    *count.entry(key(a, face[(k + 1) % face.len()])).or_default() += 1;
                }
            }
            g.outline = count.into_iter().filter(|&(_, n)| n == 1).map(|(e, _)| e).collect();
        }
        out.retain(|g| outlines(&g.outline) == 1);
        for g in &mut out {
            let across = norm(cross([0.0, 1.0, 0.0], g.normal));
            let extent = |f: &dyn Fn(V3) -> f64| {
                let (lo, hi) = g
                    .outline
                    .iter()
                    .flatten()
                    .map(|&i| f(mesh.v[i]))
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), x| (lo.min(x), hi.max(x)));
                hi - lo
            };
            g.size = (extent(&|p| dot(p, across)), extent(&|p| p[1]));
        }
        let likely = |g: &Glass| {
            let off = (g.size.0 / g.size.1.max(1e-9) / (aw / ah)).ln();
            g.area * (-SHAPE * off * off).exp()
        };
        out.sort_by(|a, b| likely(b).total_cmp(&likely(a)));
        out
    }
}

struct Glass {
    area: f64,
    /// Which way the patch faces on average.
    normal: V3,
    /// Its width and height in cm.
    size: (f64, f64),
    /// The edges around it.
    outline: Vec<[usize; 2]>,
    faces: Vec<usize>,
}

/// How many separate loops a set of edges makes.
fn outlines(edges: &[[usize; 2]]) -> usize {
    let index: BTreeMap<usize, usize> =
        edges.iter().flatten().copied().collect::<BTreeSet<_>>().into_iter().enumerate().map(|(n, v)| (v, n)).collect();
    let mut sets = Sets::new(index.len());
    for &[a, b] in edges {
        sets.join(index[&a], index[&b]);
    }
    (0..index.len()).filter(|&i| sets.find(i) == i).count()
}

/// The picture's corners (top left, top right, bottom right, bottom left): the biggest rectangle
/// of the display's shape, centered on the glass, that the glass's outline doesn't cross. It sits
/// at the outline's average depth, so a bulging glass bulges through it, which doesn't show since
/// a smooth surface has no lines.
fn fit_picture(v: &[V3], outline: &[[usize; 2]], (aw, ah): (f64, f64)) -> [V3; 4] {
    const SAMPLES: usize = 8;
    let points: Vec<V3> = outline
        .iter()
        .flat_map(|&[a, b]| (0..=SAMPLES).map(move |i| lerp(v[a], v[b], i as f64 / SAMPLES as f64)))
        .collect();
    let (lo, hi) = bounds(&points);
    let (cx, cy) = ((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
    let z = points.iter().map(|p| p[2]).sum::<f64>() / points.len() as f64;
    // A point is outside a rectangle of half size (aw·s, ah·s) when either distance is bigger.
    let s =
        points.iter().map(|p| f64::max((p[0] - cx).abs() / aw, (p[1] - cy).abs() / ah)).fold(f64::INFINITY, f64::min);
    let (w, h) = (aw * s * INSET, ah * s * INSET);
    [[cx - w, cy + h, z], [cx + w, cy + h, z], [cx + w, cy - h, z], [cx - w, cy - h, z]]
}

/// Joins lines that run straight on through a vertex no other line touches, since a curve or a
/// flat face split into triangles leaves its outline in short pieces.
fn straighten(v: &[V3], edges: BTreeSet<[usize; 2]>) -> BTreeSet<[usize; 2]> {
    let mut adj: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for &[a, b] in &edges {
        adj.entry(a).or_default().insert(b);
        adj.entry(b).or_default().insert(a);
    }
    let cos = STRAIGHT.to_radians().cos();
    let middles: Vec<usize> = adj.keys().copied().collect();
    for m in middles {
        let [a, b] = match adj[&m].iter().collect::<Vec<_>>()[..] {
            [&a, &b] => [a, b],
            _ => continue,
        };
        // Joining the two sides of a triangle would give the third twice.
        if adj[&a].contains(&b) || dot(norm(sub(v[m], v[a])), norm(sub(v[b], v[m]))) < cos {
            continue;
        }
        adj.remove(&m);
        for (from, to) in [(a, b), (b, a)] {
            let n = adj.get_mut(&from).unwrap();
            n.remove(&m);
            n.insert(to);
        }
    }
    adj.iter().flat_map(|(&a, n)| n.iter().filter(move |&&b| a < b).map(move |&b| [a, b])).collect()
}

/// Drops connected pieces that are less than `min` cm across.
fn drop_small(v: &[V3], edges: BTreeSet<[usize; 2]>, min: f64) -> BTreeSet<[usize; 2]> {
    if min <= 0.0 {
        return edges;
    }
    let mut sets = Sets::new(v.len());
    for &[a, b] in &edges {
        sets.join(a, b);
    }
    let mut extent: HashMap<usize, (V3, V3)> = HashMap::new();
    for &i in edges.iter().flatten() {
        let (lo, hi) = extent.entry(sets.find(i)).or_insert((v[i], v[i]));
        *lo = [0, 1, 2].map(|k| lo[k].min(v[i][k]));
        *hi = [0, 1, 2].map(|k| hi[k].max(v[i][k]));
    }
    edges
        .into_iter()
        .filter(|&[a, _]| {
            let (lo, hi) = extent[&sets.find(a)];
            len(sub(hi, lo)) >= min
        })
        .collect()
}

/// Union-find, for patches of faces and pieces of lines.
struct Sets(Vec<usize>);

impl Sets {
    fn new(n: usize) -> Sets {
        Sets((0..n).collect())
    }
    fn find(&mut self, mut i: usize) -> usize {
        while self.0[i] != i {
            self.0[i] = self.0[self.0[i]];
            i = self.0[i];
        }
        i
    }
    fn join(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        self.0[a.max(b)] = a.min(b);
    }
}

// ---------- the wireframe ----------

struct Wire {
    v: Vec<V3>,
    e: Vec<[usize; 2]>,
    screen: Option<[usize; 4]>,
}

#[derive(Deserialize)]
struct Part {
    vertices: Vec<V3>,
    edges: Vec<[usize; 2]>,
}

impl Wire {
    /// Keeps the vertices the lines use and adds the picture's rectangle. The origin ends up on
    /// the floor, centered on the glass and level with it, as in the NES model; or centered on the
    /// model if it has no glass.
    fn new(v: &[V3], edges: &BTreeSet<[usize; 2]>, screen: Option<[V3; 4]>) -> Wire {
        let used: BTreeSet<usize> = edges.iter().flatten().copied().collect();
        let index: HashMap<usize, usize> = used.iter().enumerate().map(|(n, &i)| (i, n)).collect();
        let mut w = Wire {
            v: used.iter().map(|&i| v[i]).collect(),
            e: edges.iter().map(|&[a, b]| [index[&a], index[&b]]).collect(),
            screen: None,
        };
        if let Some(corners) = screen {
            let n = w.v.len();
            w.v.extend(corners);
            w.e.extend([[n, n + 1], [n + 1, n + 2], [n + 2, n + 3], [n + 3, n]]);
            w.screen = Some([n, n + 1, n + 2, n + 3]);
        }
        let (lo, hi) = bounds(&w.v);
        let (x, z) = match screen {
            Some(c) => ((c[0][0] + c[1][0]) / 2.0, c[0][2]),
            None => ((lo[0] + hi[0]) / 2.0, (lo[2] + hi[2]) / 2.0),
        };
        w.v.iter_mut().for_each(|p| *p = sub(*p, [x, lo[1], z]));
        w
    }

    /// Adds a wireframe from another file, turned and moved along the floor.
    fn add(&mut self, spec: &str) -> Result<()> {
        let usage = || format!("--add {spec}: expected FILE:X,Z or FILE:X,Z,DEGREES");
        let (file, at) = spec.rsplit_once(':').with_context(usage)?;
        let nums: Vec<f64> = at.split(',').map(|s| s.trim().parse()).collect::<Result<_, _>>().with_context(usage)?;
        let (x, z, deg) = match nums[..] {
            [x, z] => (x, z, 0.0),
            [x, z, deg] => (x, z, deg),
            _ => bail!(usage()),
        };
        let part: Part = serde_json::from_str(&fs::read_to_string(file).with_context(|| format!("reading {file}"))?)
            .with_context(|| format!("reading {file}"))?;
        let n = part.vertices.len();
        ensure!(part.edges.iter().flatten().all(|&i| i < n), "{file} names a vertex it doesn't have");
        let base = self.v.len();
        self.v.extend(part.vertices.iter().map(|&p| add(turn(p, deg.to_radians()), [x, 0.0, z])));
        self.e.extend(part.edges.iter().map(|&[a, b]| [a + base, b + base]));
        Ok(())
    }

    /// In the layout of `nes.json`: a vertex or an edge a line.
    fn to_json(&self, system: &str, (aw, ah): (f64, f64), credit: Option<&str>) -> String {
        let num = |x: f64| format!("{}", (x * 100.0).round() / 100.0 + 0.0);
        let text = |s: &str| serde_json::to_string(s).unwrap();
        let mut out =
            format!("{{\"system\":{},\"units\":\"cm\",\"display_aspect\":[{},{}]", text(system), num(aw), num(ah));
        if let Some([a, b, c, d]) = self.screen {
            let _ = write!(out, ",\"screen\":[{a},{b},{c},{d}]");
        }
        if let Some(credit) = credit {
            let _ = write!(out, ",\n \"credit\":{}", text(credit));
        }
        let vertices: Vec<String> =
            self.v.iter().map(|p| format!("  [{},{},{}]", num(p[0]), num(p[1]), num(p[2]))).collect();
        let edges: Vec<String> = self.e.iter().map(|[a, b]| format!("  [{a},{b}]")).collect();
        let _ =
            write!(out, ",\n \"vertices\":[\n{}],\n \"edges\":[\n{}]\n}}\n", vertices.join(",\n"), edges.join(",\n"));
        out
    }
}

// ---------- vectors ----------

fn key(a: usize, b: usize) -> [usize; 2] {
    [a.min(b), a.max(b)]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: V3, k: f64) -> V3 {
    a.map(|c| c * k)
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
    if l > 0.0 { scale(a, 1.0 / l) } else { a }
}
fn lerp(a: V3, b: V3, t: f64) -> V3 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
fn bounds(v: &[V3]) -> (V3, V3) {
    v.iter().fold(([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]), |(lo, hi), p| {
        ([0, 1, 2].map(|k| lo[k].min(p[k])), [0, 1, 2].map(|k| hi[k].max(p[k])))
    })
}
fn lo_y(v: &[V3]) -> f64 {
    bounds(v).0[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A face with its vertices put in the order that makes it face `out`.
    fn face(mesh: &mut Mesh, points: &[V3], out: V3) {
        let base = mesh.v.len();
        mesh.v.extend(points);
        let mut f: Vec<usize> = (base..base + points.len()).collect();
        let n = cross(sub(points[1], points[0]), sub(points[2], points[0]));
        if dot(n, out) < 0.0 {
            f.reverse();
        }
        mesh.faces.push(f);
    }

    /// A box with each side split into `split`×`split` squares, each cut into two triangles.
    fn boxed(lo: V3, hi: V3, split: usize) -> Mesh {
        let mut mesh = Mesh::default();
        for k in 0..3 {
            let (u, w) = ((k + 1) % 3, (k + 2) % 3);
            for side in [lo[k], hi[k]] {
                let out = {
                    let mut o = [0.0; 3];
                    o[k] = if side == hi[k] { 1.0 } else { -1.0 };
                    o
                };
                let at = |i: usize, j: usize| {
                    let mut p = [0.0; 3];
                    p[k] = side;
                    p[u] = lo[u] + (hi[u] - lo[u]) * i as f64 / split as f64;
                    p[w] = lo[w] + (hi[w] - lo[w]) * j as f64 / split as f64;
                    p
                };
                for i in 0..split {
                    for j in 0..split {
                        face(&mut mesh, &[at(i, j), at(i + 1, j), at(i + 1, j + 1)], out);
                        face(&mut mesh, &[at(i, j), at(i + 1, j + 1), at(i, j + 1)], out);
                    }
                }
            }
        }
        mesh
    }

    fn lines(mesh: &mut Mesh) -> BTreeSet<[usize; 2]> {
        weld(mesh);
        let edges = Surface::new(mesh).folds(30f64.to_radians().cos()).collect();
        straighten(&mesh.v, edges)
    }

    #[test]
    fn a_box_split_into_triangles_is_twelve_lines() {
        for split in [1, 4] {
            let mut mesh = boxed([0.0; 3], [3.0, 2.0, 1.0], split);
            assert_eq!(lines(&mut mesh).len(), 12, "split {split}");
        }
    }

    #[test]
    fn a_rounded_box_keeps_the_lines_along_its_corners() {
        // A rounded rectangle, 10 × 6 with corners of radius 1 in 4 steps, 5 deep.
        let mut ring = Vec::new();
        for (cx, cy, a0) in [(4.0, 2.0, 0.0), (-4.0, 2.0, 90.0), (-4.0, -2.0, 180.0), (4.0, -2.0, 270.0)] {
            for i in 0..=4 {
                let a = f64::to_radians(a0 + 90.0 * f64::from(i) / 4.0);
                ring.push([cx + a.cos(), cy + a.sin()]);
            }
        }
        let mut mesh = Mesh::default();
        let at = |p: [f64; 2], z: f64| [p[0], p[1], z];
        for (i, &p) in ring.iter().enumerate() {
            let q = ring[(i + 1) % ring.len()];
            face(&mut mesh, &[at(p, 0.0), at(q, 0.0), at(q, 5.0), at(p, 5.0)], [p[0] + q[0], p[1] + q[1], 0.0]);
        }
        for z in [0.0, 5.0] {
            face(&mut mesh, &ring.iter().map(|&p| at(p, z)).collect::<Vec<_>>(), [0.0, 0.0, z - 2.5]);
        }
        weld(&mut mesh);
        let surface = Surface::new(&mesh);
        let along = |edges: &BTreeSet<[usize; 2]>| edges.iter().filter(|&&[a, b]| mesh.v[a][2] != mesh.v[b][2]).count();
        let folds: BTreeSet<_> = surface.folds(30f64.to_radians().cos()).collect();
        assert_eq!(along(&folds), 0, "the corners are smooth");
        let mut edges = folds.clone();
        edges.extend(surface.bends(0.02));
        assert_eq!(along(&edges), 8, "where each corner starts and ends");
        assert_eq!(edges.len() - folds.len(), 8);
    }

    #[test]
    fn an_open_surface_keeps_its_outline() {
        let mut mesh = boxed([0.0; 3], [1.0; 3], 3);
        mesh.faces.retain(|f| f.iter().all(|&i| mesh.v[i][2] == 1.0));
        assert_eq!(lines(&mut mesh).len(), 4);
    }

    /// A TV: a box with the glass set 2 cm into its front.
    fn tv() -> Mesh {
        let (w, h, d, gw, gh, r) = (24.0, 20.0, 30.0, 16.0, 12.0, 2.0);
        let mut mesh = boxed([-w, 0.0, -d], [w, 2.0 * h, 0.0], 1);
        mesh.faces.retain(|f| f.iter().any(|&i| mesh.v[i][2] != 0.0));
        let cy = h;
        let outer = [[-w, 0.0, 0.0], [w, 0.0, 0.0], [w, 2.0 * h, 0.0], [-w, 2.0 * h, 0.0]];
        let inner = [[-gw, cy - gh, 0.0], [gw, cy - gh, 0.0], [gw, cy + gh, 0.0], [-gw, cy + gh, 0.0]];
        let back = inner.map(|p| [p[0], p[1], -r]);
        for i in 0..4 {
            let j = (i + 1) % 4;
            face(&mut mesh, &[outer[i], outer[j], inner[j], inner[i]], [0.0, 0.0, 1.0]);
            let middle = scale(add(inner[i], inner[j]), 0.5);
            face(&mut mesh, &[inner[i], inner[j], back[j], back[i]], sub([0.0, cy, 0.0], middle));
        }
        face(&mut mesh, &back, [0.0, 0.0, 1.0]);
        mesh
    }

    #[test]
    fn finds_the_glass_set_into_a_tv() {
        let mut mesh = tv();
        weld(&mut mesh);
        let cos = 30f64.to_radians().cos();
        let surface = Surface::new(&mesh);
        // The box's 12, the bezel's rim, the recess's corners and the glass's rim.
        let edges = straighten(&mesh.v, surface.folds(cos).collect());
        assert_eq!(edges.len(), 24);
        let glass = surface.glass(&mesh, cos, (4.0, 3.0));
        assert_eq!(glass.len(), 1, "the bezel has two outlines, so only the glass is a candidate");
        assert!((glass[0].area - 32.0 * 24.0).abs() < 1e-9);
        let [tl, tr, br, bl] = fit_picture(&mesh.v, &glass[0].outline, (4.0, 3.0));
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(close(tl[2], -2.0) && close(br[2], -2.0));
        assert!(close(tr[1], 20.0 + 12.0 * INSET) && close(bl[1], 20.0 - 12.0 * INSET));
        assert!(close(tr[0], 16.0 * INSET) && close(tl[0], -16.0 * INSET));
    }

    #[test]
    fn the_glass_is_more_likely_shaped_like_the_picture() {
        // A tall panel, and a smaller 4:3 screen in front of it.
        let mut mesh = Mesh::default();
        let quad = |x0: f64, y0: f64, x1: f64, y1: f64, z: f64| [[x0, y0, z], [x1, y0, z], [x1, y1, z], [x0, y1, z]];
        face(&mut mesh, &quad(0.0, 0.0, 10.0, 15.0, 0.0), [0.0, 0.0, 1.0]);
        face(&mut mesh, &quad(2.0, 8.0, 8.0, 12.5, 0.5), [0.0, 0.0, 1.0]);
        weld(&mut mesh);
        let surface = Surface::new(&mesh);
        let cos = 30f64.to_radians().cos();
        let sizes: Vec<_> = surface.glass(&mesh, cos, (4.0, 3.0)).iter().map(|g| g.size).collect();
        assert_eq!(sizes, [(6.0, 4.5), (10.0, 15.0)]);
        let sizes: Vec<_> = surface.glass(&mesh, cos, (2.0, 3.0)).iter().map(|g| g.size).collect();
        assert_eq!(sizes[0], (10.0, 15.0));
    }

    #[test]
    fn a_wide_glass_fits_the_picture_by_height() {
        let v = [[0.0, 0.0, 0.0], [16.0, 0.0, 0.0], [16.0, 9.0, 0.0], [0.0, 9.0, 0.0]];
        let [tl, tr, _, bl] = fit_picture(&v, &[[0, 1], [1, 2], [2, 3], [0, 3]], (4.0, 3.0));
        assert!((tl[1] - bl[1] - 9.0 * INSET).abs() < 1e-9);
        assert!((tr[0] - tl[0] - 12.0 * INSET).abs() < 1e-9);
        assert!((tl[0] + tr[0] - 16.0).abs() < 1e-9);
    }

    #[test]
    fn reads_obj_faces_with_slashes_and_negative_numbers() {
        let mesh =
            read_obj("v 0 0 0\nv 1 0 0\nv 1 1 0\nvt 0 0\nf 1/1/1 2/1 3\nv 0 1 0\nf -4 -2 -1\nl 1 2 3\n").unwrap();
        assert_eq!(mesh.faces, [vec![0, 1, 2], vec![0, 2, 3]]);
        assert_eq!(mesh.lines, [[0, 1], [1, 2]]);
        assert!(read_obj("v 0 0 0\nf 1 2 0\n").is_err());
    }

    #[test]
    fn reads_binary_and_ascii_stl() {
        let mut binary = vec![0u8; 80];
        binary.extend(1u32.to_le_bytes());
        for c in [0.0f32; 3].iter().chain(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]) {
            binary.extend(c.to_le_bytes());
        }
        binary.extend([0, 0]);
        let ascii =
            "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\n";
        for bytes in [&binary[..], ascii.as_bytes()] {
            let mesh = read_stl(bytes).unwrap();
            assert_eq!(mesh.v, [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
            assert_eq!(mesh.faces, [vec![0, 1, 2]]);
        }
    }

    #[test]
    fn stands_a_z_up_model_facing_minus_y_upright_facing_the_viewer() {
        let mut v = vec![[1.0, -2.0, 3.0]];
        orient(&mut v, Axis::new(2, 1.0), Axis::new(1, -1.0), 0.0, 10.0).unwrap();
        assert_eq!(v[0], [10.0, 30.0, 20.0]);
        assert_eq!("-y".parse(), Ok(Axis::new(1, -1.0)));
        assert!("w".parse::<Axis>().is_err());
    }

    #[test]
    fn turning_moves_z_toward_x() {
        let p = turn([0.0, 0.0, 1.0], std::f64::consts::FRAC_PI_2);
        assert!((p[0] - 1.0).abs() < 1e-12 && p[2].abs() < 1e-12);
    }

    #[test]
    fn glb_nodes_are_moved_by_their_transforms() {
        let m = trs(
            [1.0, 2.0, 3.0],
            [0.0, 0.0, std::f64::consts::FRAC_1_SQRT_2, std::f64::consts::FRAC_1_SQRT_2],
            [2.0; 3],
        );
        let p = apply(&mul(&IDENTITY, &m), [1.0, 0.0, 0.0]);
        // A quarter turn about z takes x to y, then it's doubled and moved.
        assert!(len(sub(p, [1.0, 4.0, 3.0])) < 1e-12);
    }
}

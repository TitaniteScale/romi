//! A minimal libretro frontend: loads a core (the same `.so` RetroArch uses), gives it a ROM and
//! runs it one frame at a time. Only what 2D cores need: software video (turned upright for
//! arcade games on their side), stereo sound, a joypad.
//!
//! libretro's callbacks are plain C functions with no user data, so what they share with romi
//! lives in a global, and only one core can be loaded at a time.

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

// ---- the libretro ABI (libretro.h) ----

const API_VERSION: c_uint = 1;
const DEVICE_JOYPAD: c_uint = 1;
/// Asks for every button at once as a bitmask, when the frontend says it supports that.
const DEVICE_ID_JOYPAD_MASK: c_uint = 256;

const ENV_SET_ROTATION: c_uint = 1;
const ENV_GET_CAN_DUPE: c_uint = 3;
const ENV_SET_MESSAGE: c_uint = 6;
const ENV_SET_PERFORMANCE_LEVEL: c_uint = 8;
const ENV_GET_SYSTEM_DIRECTORY: c_uint = 9;
const ENV_SET_PIXEL_FORMAT: c_uint = 10;
const ENV_SET_INPUT_DESCRIPTORS: c_uint = 11;
const ENV_SET_VARIABLES: c_uint = 16;
const ENV_GET_VARIABLE_UPDATE: c_uint = 17;
const ENV_SET_SUPPORT_NO_GAME: c_uint = 18;
const ENV_GET_SAVE_DIRECTORY: c_uint = 31;
const ENV_SET_CONTROLLER_INFO: c_uint = 35;
const ENV_SET_MEMORY_MAPS: c_uint = 36;
const ENV_SET_GEOMETRY: c_uint = 37;
/// Flags a core may set on a command; the command is the same without them.
const ENV_FLAGS: c_uint = 0x10000 | 0x20000;

const PIXEL_0RGB1555: c_int = 0;
const PIXEL_XRGB8888: c_int = 1;
const PIXEL_RGB565: c_int = 2;

/// What the video callback gets instead of pixels when the core renders with OpenGL or Vulkan.
const HW_FRAME_BUFFER_VALID: *const c_void = usize::MAX as *const c_void;

#[repr(C)]
struct SystemInfo {
    library_name: *const c_char,
    library_version: *const c_char,
    valid_extensions: *const c_char,
    need_fullpath: bool,
    block_extract: bool,
}

#[repr(C)]
struct GameInfo {
    path: *const c_char,
    data: *const c_void,
    size: usize,
    meta: *const c_char,
}

#[repr(C)]
#[derive(Default)]
struct Geometry {
    base_width: c_uint,
    base_height: c_uint,
    max_width: c_uint,
    max_height: c_uint,
    aspect_ratio: f32,
}

#[repr(C)]
#[derive(Default)]
struct Timing {
    fps: f64,
    sample_rate: f64,
}

#[repr(C)]
#[derive(Default)]
struct AvInfo {
    geometry: Geometry,
    timing: Timing,
}

type EnvironmentFn = extern "C" fn(c_uint, *mut c_void) -> bool;
type VideoFn = extern "C" fn(*const c_void, c_uint, c_uint, usize);
type AudioSampleFn = extern "C" fn(i16, i16);
type AudioBatchFn = extern "C" fn(*const i16, usize) -> usize;
type InputPollFn = extern "C" fn();
type InputStateFn = extern "C" fn(c_uint, c_uint, c_uint, c_uint) -> i16;

/// The core's functions, copied out of the library so calling them doesn't need a lookup.
struct Api {
    deinit: unsafe extern "C" fn(),
    run: unsafe extern "C" fn(),
    unload_game: unsafe extern "C" fn(),
}

// ---- the joypad ----

/// libretro's joypad buttons, by the names `--input` scripts use, in libretro's id order.
pub const BUTTONS: [&str; 16] =
    ["b", "y", "select", "start", "up", "down", "left", "right", "a", "x", "l", "r", "l2", "r2", "l3", "r3"];

/// The bit for a button name, if it is one.
pub fn button(name: &str) -> Option<u16> {
    BUTTONS.iter().position(|b| b.eq_ignore_ascii_case(name)).map(|i| 1 << i)
}

// ---- what the callbacks share with romi ----

/// One video frame as packed 8-bit RGB, top row first.
#[derive(Clone, Default)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
    /// How wide each pixel is meant to look, for its height: under 1 for arcade boards that squeeze
    /// a wide picture onto a 4:3 screen. 0 is the same as 1, square.
    pub pixel_aspect: f64,
}

impl Frame {
    /// The size the frame is meant to be seen at, in square pixels as tall as the frame.
    pub fn display_size(&self) -> (f64, f64) {
        let par = if self.pixel_aspect > 0.0 { self.pixel_aspect } else { 1.0 };
        (f64::from(self.width) * par, f64::from(self.height))
    }

    /// The frame scaled to exactly `w`×`h`, nearest neighbour, so pixels stay sharp.
    pub fn resample(&self, w: u32, h: u32) -> Vec<u8> {
        let (fw, fh) = (self.width as usize, self.height as usize);
        let (w, h) = (w as usize, h as usize);
        let xs: Vec<usize> = (0..w).map(|x| x * fw / w * 3).collect();
        let mut out = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            let row = &self.rgb[y * fh / h * fw * 3..][..fw * 3];
            for &x in &xs {
                out.extend_from_slice(&row[x..x + 3]);
            }
        }
        out
    }
}

struct Shared {
    pixel_format: c_int,
    /// Quarter turns counterclockwise to stand the picture upright.
    rotation: c_uint,
    /// The picture's base size and the aspect ratio it's meant to be shown at, from the core.
    geometry: (c_uint, c_uint, f32),
    /// The folders from `Dirs`, kept here while the core may still be using them.
    system_dir: Option<CString>,
    save_dir: Option<CString>,
    frame: Frame,
    /// The last frame's sound: interleaved left and right samples at the core's sample rate.
    audio: Vec<i16>,
    buttons: u16,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    pixel_format: PIXEL_0RGB1555,
    rotation: 0,
    geometry: (0, 0, 0.0),
    system_dir: None,
    save_dir: None,
    frame: Frame { width: 0, height: 0, rgb: Vec::new(), pixel_aspect: 0.0 },
    audio: Vec::new(),
    buttons: 0,
});

static LOADED: AtomicBool = AtomicBool::new(false);

fn shared() -> std::sync::MutexGuard<'static, Shared> {
    // A panic while holding the lock can't leave the frame half-valid in any way that matters.
    SHARED.lock().unwrap_or_else(|e| e.into_inner())
}

extern "C" fn environment(cmd: c_uint, data: *mut c_void) -> bool {
    // SAFETY: each command defines what `data` points to (libretro.h); only those are read or written.
    unsafe {
        match cmd & !ENV_FLAGS {
            ENV_SET_ROTATION => shared().rotation = *(data as *const c_uint) % 4,
            ENV_GET_CAN_DUPE => *(data as *mut bool) = true,
            // The strings stay put until the next core is loaded.
            ENV_GET_SYSTEM_DIRECTORY => {
                *(data as *mut *const c_char) = shared().system_dir.as_ref().map_or(std::ptr::null(), |d| d.as_ptr())
            }
            ENV_GET_SAVE_DIRECTORY => {
                *(data as *mut *const c_char) = shared().save_dir.as_ref().map_or(std::ptr::null(), |d| d.as_ptr())
            }
            ENV_SET_PIXEL_FORMAT => {
                let format = *(data as *const c_int);
                if !matches!(format, PIXEL_0RGB1555 | PIXEL_XRGB8888 | PIXEL_RGB565) {
                    return false;
                }
                shared().pixel_format = format;
            }
            ENV_GET_VARIABLE_UPDATE => *(data as *mut bool) = false,
            ENV_SET_MESSAGE
            | ENV_SET_PERFORMANCE_LEVEL
            | ENV_SET_INPUT_DESCRIPTORS
            | ENV_SET_VARIABLES
            | ENV_SET_SUPPORT_NO_GAME
            | ENV_SET_CONTROLLER_INFO
            | ENV_SET_MEMORY_MAPS => {}
            ENV_SET_GEOMETRY => {
                let g = &*(data as *const Geometry);
                shared().geometry = (g.base_width, g.base_height, g.aspect_ratio);
            }
            // Everything else (core options, logging, bitmask input among them) is unsupported,
            // and cores fall back to their defaults.
            _ => return false,
        }
    }
    true
}

extern "C" fn video(data: *const c_void, width: c_uint, height: c_uint, pitch: usize) {
    // A null frame repeats the last one.
    if data.is_null() || data == HW_FRAME_BUFFER_VALID || width == 0 || height == 0 {
        return;
    }
    let mut s = shared();
    let bytes_per_pixel = if s.pixel_format == PIXEL_XRGB8888 { 4 } else { 2 };
    // SAFETY: the core hands over `height` rows of `pitch` bytes, valid for this call.
    let src = unsafe {
        std::slice::from_raw_parts(data as *const u8, pitch * (height as usize - 1) + width as usize * bytes_per_pixel)
    };
    let (format, rotation, geometry) = (s.pixel_format, s.rotation, s.geometry);
    let frame = &mut s.frame;
    frame.width = width;
    frame.height = height;
    frame.rgb.clear();
    frame.rgb.reserve(width as usize * height as usize * 3);
    for row in 0..height as usize {
        let line = &src[row * pitch..][..width as usize * bytes_per_pixel];
        to_rgb(format, line, &mut frame.rgb);
    }
    if rotation != 0 {
        *frame = rotate(frame, rotation);
    }
    frame.pixel_aspect = pixel_aspect(geometry, rotation);
}

/// How wide the core means each pixel to look, for its height: the aspect ratio it gives the
/// picture over the aspect ratio of its base size. The base size can differ from the frames (mGBA
/// gives the Super Game Boy's border size for Game Boy games), so this, unlike the picture's
/// aspect ratio, holds for every frame.
fn pixel_aspect((w, h, aspect): (c_uint, c_uint, f32), rotation: c_uint) -> f64 {
    // The aspect ratio is for the picture turned upright; the base size is from before.
    let (w, h) = if rotation % 2 == 1 { (h, w) } else { (w, h) };
    if aspect <= 0.0 || w == 0 || h == 0 {
        return 1.0;
    }
    let par = f64::from(aspect) / (f64::from(w) / f64::from(h));
    if (0.25..=4.0).contains(&par) { par } else { 1.0 }
}

/// The frame turned `quarter_turns` quarter turns counterclockwise.
fn rotate(frame: &Frame, quarter_turns: c_uint) -> Frame {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let (turned_w, turned_h) = if quarter_turns % 2 == 1 { (h, w) } else { (w, h) };
    let mut rgb = vec![0; frame.rgb.len()];
    for y in 0..h {
        for x in 0..w {
            let (tx, ty) = match quarter_turns {
                1 => (y, w - 1 - x),
                2 => (w - 1 - x, h - 1 - y),
                3 => (h - 1 - y, x),
                _ => (x, y),
            };
            let to = (ty * turned_w + tx) * 3;
            rgb[to..to + 3].copy_from_slice(&frame.rgb[(y * w + x) * 3..][..3]);
        }
    }
    Frame { width: turned_w as u32, height: turned_h as u32, rgb, pixel_aspect: frame.pixel_aspect }
}

/// Appends one row of the core's pixels as RGB.
fn to_rgb(format: c_int, line: &[u8], out: &mut Vec<u8>) {
    // Five or six bits stretched to eight, so full brightness stays 255.
    let five = |v: u16| ((v << 3) | (v >> 2)) as u8;
    let six = |v: u16| ((v << 2) | (v >> 4)) as u8;
    match format {
        PIXEL_XRGB8888 => {
            for p in line.as_chunks::<4>().0 {
                let [_, r, g, b] = u32::from_ne_bytes(*p).to_be_bytes();
                out.extend_from_slice(&[r, g, b]);
            }
        }
        PIXEL_RGB565 => {
            for p in line.as_chunks::<2>().0 {
                let v = u16::from_ne_bytes(*p);
                out.extend_from_slice(&[five(v >> 11), six((v >> 5) & 0x3f), five(v & 0x1f)]);
            }
        }
        _ => {
            for p in line.as_chunks::<2>().0 {
                let v = u16::from_ne_bytes(*p);
                out.extend_from_slice(&[five((v >> 10) & 0x1f), five((v >> 5) & 0x1f), five(v & 0x1f)]);
            }
        }
    }
}

extern "C" fn audio_sample(left: i16, right: i16) {
    shared().audio.extend_from_slice(&[left, right]);
}

extern "C" fn audio_batch(data: *const i16, frames: usize) -> usize {
    if !data.is_null() {
        // SAFETY: the core hands over `frames` pairs of samples, valid for this call.
        shared().audio.extend_from_slice(unsafe { std::slice::from_raw_parts(data, frames * 2) });
    }
    frames
}

extern "C" fn input_poll() {}

extern "C" fn input_state(port: c_uint, device: c_uint, _index: c_uint, id: c_uint) -> i16 {
    if port != 0 || device != DEVICE_JOYPAD {
        return 0;
    }
    let buttons = shared().buttons;
    match id {
        DEVICE_ID_JOYPAD_MASK => buttons as i16,
        0..16 => (buttons >> id & 1) as i16,
        _ => 0,
    }
}

// ---- a loaded core ----

/// Folders a core is given. Without them, cores fall back to the ROM's folder, and some write
/// there.
#[derive(Default)]
pub struct Dirs {
    /// Where BIOS files and the core's own data are (RetroArch's system folder).
    pub system: Option<PathBuf>,
    /// Where the core keeps what it writes itself, like FinalBurn Neo's NVRAM and high scores.
    pub saves: Option<PathBuf>,
}

pub struct Core {
    api: Api,
    /// The core's name and version, e.g. "mGBA 0.11".
    pub name: String,
    pub fps: f64,
    /// Sound samples per second, per channel.
    pub sample_rate: f64,
    /// The ROM's bytes, which the core may keep pointing into until the game is unloaded.
    _rom: Vec<u8>,
    /// Where the ROM was unpacked to, for a zip the core can't open itself.
    _unpacked: Option<crate::files::TempDir>,
    // Dropped last, after `Drop` has shut the core down.
    _lib: libloading::Library,
}

impl Core {
    /// Loads the core at `core_path` and starts `rom` in it, with `dirs` as its folders.
    pub fn load(core_path: &Path, rom: &Path, dirs: &Dirs) -> Result<Core> {
        if LOADED.swap(true, Ordering::SeqCst) {
            bail!("a core is already loaded");
        }
        Core::open(core_path, rom, dirs).inspect_err(|_| LOADED.store(false, Ordering::SeqCst))
    }

    fn open(core_path: &Path, rom: &Path, dirs: &Dirs) -> Result<Core> {
        let unpacked = unpack(core_path, rom)?;
        let rom = unpacked.as_ref().map_or(rom.to_path_buf(), |(_, path)| path.clone());
        let rom = rom.as_path();
        let c_path = |d: &Option<PathBuf>| d.as_ref().and_then(|d| CString::new(d.as_os_str().as_encoded_bytes()).ok());
        *shared() = Shared {
            pixel_format: PIXEL_0RGB1555,
            rotation: 0,
            geometry: (0, 0, 0.0),
            system_dir: c_path(&dirs.system),
            save_dir: c_path(&dirs.saves),
            frame: Frame::default(),
            audio: Vec::new(),
            buttons: 0,
        };
        // SAFETY: loading a core runs its initializers; libretro cores are made to be loaded this way.
        let lib = unsafe { libloading::Library::new(core_path) }
            .with_context(|| format!("can't load {}", core_path.display()))?;
        // SAFETY: the signatures below are libretro.h's, which every core exports.
        unsafe {
            macro_rules! sym {
                ($name:literal, $ty:ty) => {
                    *lib.get::<$ty>(concat!($name, "\0").as_bytes())
                        .with_context(|| format!("{} isn't a libretro core (no {})", core_path.display(), $name))?
                };
            }
            let api_version = sym!("retro_api_version", unsafe extern "C" fn() -> c_uint);
            if api_version() != API_VERSION {
                bail!("{} speaks libretro API {}, not {API_VERSION}", core_path.display(), api_version());
            }
            sym!("retro_set_environment", unsafe extern "C" fn(EnvironmentFn))(environment);
            sym!("retro_set_video_refresh", unsafe extern "C" fn(VideoFn))(video);
            sym!("retro_set_audio_sample", unsafe extern "C" fn(AudioSampleFn))(audio_sample);
            sym!("retro_set_audio_sample_batch", unsafe extern "C" fn(AudioBatchFn))(audio_batch);
            sym!("retro_set_input_poll", unsafe extern "C" fn(InputPollFn))(input_poll);
            sym!("retro_set_input_state", unsafe extern "C" fn(InputStateFn))(input_state);
            sym!("retro_init", unsafe extern "C" fn())();

            let api = Api {
                deinit: sym!("retro_deinit", unsafe extern "C" fn()),
                run: sym!("retro_run", unsafe extern "C" fn()),
                unload_game: sym!("retro_unload_game", unsafe extern "C" fn()),
            };
            let mut info = std::mem::zeroed::<SystemInfo>();
            sym!("retro_get_system_info", unsafe extern "C" fn(*mut SystemInfo))(&mut info);
            let text = |p: *const c_char| {
                if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
            };
            let name = format!("{} {}", text(info.library_name), text(info.library_version)).trim().to_string();

            let path = CString::new(rom.as_os_str().as_encoded_bytes()).context("ROM path has a NUL in it")?;
            let data = if info.need_fullpath {
                Vec::new()
            } else {
                std::fs::read(rom).with_context(|| format!("can't read {}", rom.display()))?
            };
            let game = GameInfo {
                path: path.as_ptr(),
                data: if data.is_empty() { std::ptr::null() } else { data.as_ptr().cast() },
                size: data.len(),
                meta: std::ptr::null(),
            };
            if !sym!("retro_load_game", unsafe extern "C" fn(*const GameInfo) -> bool)(&game) {
                (api.deinit)();
                bail!("{name} couldn't load {}", rom.display());
            }
            let mut av = AvInfo::default();
            sym!("retro_get_system_av_info", unsafe extern "C" fn(*mut AvInfo))(&mut av);
            shared().geometry = (av.geometry.base_width, av.geometry.base_height, av.geometry.aspect_ratio);
            Ok(Core {
                api,
                name,
                fps: av.timing.fps,
                sample_rate: av.timing.sample_rate,
                _rom: data,
                _unpacked: unpacked.map(|(dir, _)| dir),
                _lib: lib,
            })
        }
    }

    /// Runs one frame with these buttons held (bits from `button`).
    pub fn run_frame(&mut self, buttons: u16) {
        {
            let mut s = shared();
            s.buttons = buttons;
            s.audio.clear();
        }
        // SAFETY: the game is loaded, and only one thread drives a core.
        unsafe { (self.api.run)() }
    }

    /// The last frame the core drew.
    pub fn frame(&self) -> Frame {
        shared().frame.clone()
    }

    /// Appends the last frame's sound to `out`, as interleaved left and right samples.
    pub fn audio(&self, out: &mut Vec<i16>) {
        out.extend_from_slice(&shared().audio);
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        // SAFETY: the game was loaded and the core initialized in `load`.
        unsafe {
            (self.api.unload_game)();
            (self.api.deinit)();
        }
        LOADED.store(false, Ordering::SeqCst);
    }
}

/// The core in a RetroArch command line (`-L core.so` or `--libretro=core.so`).
pub fn core_in(argv: &[String]) -> Option<PathBuf> {
    argv.iter().enumerate().find_map(|(i, arg)| match arg.as_str() {
        "-L" | "--libretro" => argv.get(i + 1).map(PathBuf::from),
        _ => arg.strip_prefix("--libretro=").map(PathBuf::from),
    })
}

/// Whether the core at `core` can be given this ROM. A zip goes as it is to a core that opens zips
/// itself, as arcade cores like FinalBurn Neo do, and is unpacked for any other. romi can't unpack
/// a 7z, so those only go to cores that open them.
pub fn can_open(core: &Path, rom: &Path) -> bool {
    match archive_ext(rom).as_deref() {
        Some("7z") => opens(core, "7z"),
        _ => true,
    }
}

/// "zip" or "7z", lowercased, for an archive.
fn archive_ext(rom: &Path) -> Option<String> {
    let ext = rom.extension()?.to_string_lossy().to_lowercase();
    matches!(ext.as_str(), "zip" | "7z").then_some(ext)
}

fn opens(core: &Path, ext: &str) -> bool {
    info(core).is_some_and(|info| info.extensions.iter().any(|e| e == ext))
}

/// For a zip the core can't open itself, the ROM in it unpacked to a folder in /tmp, as RetroArch
/// does: the biggest file, which is the game in the zips ROM sets come in.
fn unpack(core: &Path, rom: &Path) -> Result<Option<(crate::files::TempDir, PathBuf)>> {
    if archive_ext(rom).as_deref() != Some("zip") || opens(core, "zip") {
        return Ok(None);
    }
    let (name, data) = crate::archive::read_biggest(rom, 1 << 30)
        .with_context(|| format!("can't read the game out of {}", rom.display()))?;
    let dir = crate::files::TempDir::new("rom").context("can't make a folder in /tmp to unpack into")?;
    let path = dir.path().join(if name.is_empty() { "rom" } else { &name });
    std::fs::write(&path, data).with_context(|| format!("can't unpack {}", rom.display()))?;
    Ok(Some((dir, path)))
}

/// What a core says about itself before it's started.
#[derive(Clone)]
pub struct Info {
    /// Its name without the version, as RetroArch names folders after it ("FinalBurn Neo").
    pub name: String,
    /// The file extensions it opens, lowercased.
    extensions: Vec<String>,
}

/// What the core at `core` says about itself, or None if it isn't a core. Asking loads the core,
/// which takes a moment for big ones, so each is asked once.
pub fn info(core: &Path) -> Option<Info> {
    static KNOWN: Mutex<BTreeMap<PathBuf, Option<Info>>> = Mutex::new(BTreeMap::new());
    let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    known.entry(core.to_path_buf()).or_insert_with(|| read_info(core).ok()).clone()
}

fn read_info(path: &Path) -> Result<Info> {
    // SAFETY: as in `Core::open`. libretro allows asking for system info before `retro_init`, and
    // the strings are copied out before the library is closed again.
    unsafe {
        let lib = libloading::Library::new(path)?;
        let mut info = std::mem::zeroed::<SystemInfo>();
        lib.get::<unsafe extern "C" fn(*mut SystemInfo)>(b"retro_get_system_info\0")?(&mut info);
        let text = |p: *const c_char| {
            if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
        };
        let extensions = text(info.valid_extensions).to_lowercase().split('|').map(str::to_string).collect();
        Ok(Info { name: text(info.library_name), extensions })
    }
}

// ---- scripted input ----

/// Buttons to hold over a run, from a script of `<frame> <buttons...>` lines. Each line holds its
/// buttons from that frame until the next line; a frame with no buttons lets go of everything.
///
/// ```text
/// # wait for the title screen, then tap start
/// 300 start
/// 306
/// 400 a
/// 404
/// ```
#[derive(Debug, Default, PartialEq)]
pub struct Script {
    /// (first frame, buttons), in frame order.
    steps: Vec<(u64, u16)>,
}

impl Script {
    pub fn parse(text: &str) -> Result<Script> {
        let mut steps = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            let mut words = line.split(|c: char| c.is_whitespace() || c == ',' || c == '+').filter(|w| !w.is_empty());
            let Some(first) = words.next() else { continue };
            let frame: u64 =
                first.parse().with_context(|| format!("line {}: {first:?} isn't a frame number", n + 1))?;
            let mut buttons = 0;
            for w in words {
                buttons |= button(w).with_context(|| {
                    format!("line {}: no button called {w:?} (buttons: {})", n + 1, BUTTONS.join(" "))
                })?;
            }
            if steps.last().is_some_and(|&(last, _)| frame < last) {
                bail!("line {}: frame {frame} comes before the line above it", n + 1);
            }
            steps.push((frame, buttons));
        }
        Ok(Script { steps })
    }

    /// The buttons held on `frame` (counting from 0).
    pub fn buttons_at(&self, frame: u64) -> u16 {
        self.steps.iter().take_while(|&&(start, _)| start <= frame).last().map_or(0, |&(_, b)| b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_hold_buttons_until_the_next_line() {
        let s = Script::parse("# title\n300 start\n306\n\n400 a+b  # both\n404 Left, UP\n").unwrap();
        assert_eq!(s.buttons_at(0), 0);
        assert_eq!(s.buttons_at(300), button("start").unwrap());
        assert_eq!(s.buttons_at(305), button("start").unwrap());
        assert_eq!(s.buttons_at(306), 0);
        assert_eq!(s.buttons_at(401), button("a").unwrap() | button("b").unwrap());
        assert_eq!(s.buttons_at(9999), button("left").unwrap() | button("up").unwrap());
    }

    #[test]
    fn scripts_reject_unknown_buttons_and_frames_out_of_order() {
        assert!(Script::parse("10 jump").unwrap_err().to_string().contains("no button called \"jump\""));
        assert!(Script::parse("20 a\n10 b").is_err());
        assert!(Script::parse("soon a").is_err());
    }

    #[test]
    fn rotation_turns_frames_counterclockwise() {
        // Two pixels side by side, 1 on the left and 2 on the right.
        let frame = Frame { width: 2, height: 1, rgb: vec![1, 1, 1, 2, 2, 2], ..Frame::default() };
        let turned = |n| {
            let f = rotate(&frame, n);
            (f.width, f.height, f.rgb.chunks(3).map(|p| p[0]).collect::<Vec<_>>())
        };
        // A quarter turn counterclockwise lifts the right end to the top.
        assert_eq!(turned(1), (1, 2, vec![2, 1]));
        assert_eq!(turned(2), (2, 1, vec![2, 1]));
        assert_eq!(turned(3), (1, 2, vec![1, 2]));
    }

    #[test]
    fn pixels_are_as_wide_as_the_cores_aspect_ratio_makes_them() {
        let close = |a: f64, b: f64| (a - b).abs() < 0.001;
        // CPS-2: 384×224 shown at 4:3.
        assert!(close(pixel_aspect((384, 224, 4.0 / 3.0), 0), 0.7778));
        // mGBA gives the Super Game Boy's size for Game Boy games, whose pixels are square.
        assert!(close(pixel_aspect((256, 224, 256.0 / 224.0), 0), 1.0));
        // Ms. Pac-Man: 288×224 on its side, shown upright at 3:4.
        assert!(close(pixel_aspect((288, 224, 0.75), 3), 0.9643));
        // No aspect ratio means square pixels.
        assert_eq!(pixel_aspect((256, 216, -1.0), 0), 1.0);
    }

    #[test]
    fn frames_are_shown_as_wide_as_their_pixels() {
        let frame = Frame { width: 384, height: 224, pixel_aspect: 0.7778, ..Frame::default() };
        let (w, h) = frame.display_size();
        assert!((w / h - 4.0 / 3.0).abs() < 0.001);
        assert_eq!(Frame { width: 160, height: 144, ..Frame::default() }.display_size(), (160.0, 144.0));
    }

    #[test]
    fn resampling_repeats_the_nearest_pixel() {
        let frame = Frame { width: 2, height: 1, rgb: vec![1, 1, 1, 2, 2, 2], ..Frame::default() };
        assert_eq!(frame.resample(4, 2), [[1u8; 6], [2; 6], [1; 6], [2; 6]].concat());
        assert_eq!(frame.resample(3, 1), [1, 1, 1, 1, 1, 1, 2, 2, 2]);
    }

    #[test]
    fn rotation_keeps_every_pixel_of_a_rectangle() {
        let frame = Frame { width: 3, height: 2, rgb: (0..18).collect(), ..Frame::default() };
        let back = rotate(&rotate(&frame, 1), 3);
        assert_eq!((back.width, back.height), (3, 2));
        assert_eq!(back.rgb, frame.rgb);
        assert_eq!(rotate(&rotate(&frame, 2), 2).rgb, frame.rgb);
    }

    #[test]
    fn zips_are_unpacked_and_7z_files_only_go_to_cores_that_open_them() {
        let no_core = Path::new("/nonexistent/core_libretro.so");
        assert!(can_open(no_core, Path::new("game.gba")));
        assert!(can_open(no_core, Path::new("rom")));
        assert!(can_open(no_core, Path::new("game.ZIP")));
        assert!(!can_open(no_core, Path::new("game.7z")));
    }

    #[test]
    fn pixels_convert_to_full_range_rgb() {
        let mut out = Vec::new();
        to_rgb(PIXEL_RGB565, &0xffffu16.to_ne_bytes(), &mut out);
        to_rgb(PIXEL_RGB565, &0xf800u16.to_ne_bytes(), &mut out);
        to_rgb(PIXEL_0RGB1555, &0x001fu16.to_ne_bytes(), &mut out);
        to_rgb(PIXEL_XRGB8888, &0x00112233u32.to_ne_bytes(), &mut out);
        assert_eq!(out, [255, 255, 255, 255, 0, 0, 0, 0, 255, 0x11, 0x22, 0x33]);
    }
}

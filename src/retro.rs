//! A minimal libretro frontend: loads a core (the same `.so` RetroArch uses), gives it a ROM and
//! runs it one frame at a time. Only what 2D cores need: software video, a joypad, no audio yet.
//!
//! libretro's callbacks are plain C functions with no user data, so what they share with romi
//! lives in a global, and only one core can be loaded at a time.

use anyhow::{Context, Result, bail};
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

// ---- the libretro ABI (libretro.h) ----

const API_VERSION: c_uint = 1;
const DEVICE_JOYPAD: c_uint = 1;
/// Asks for every button at once as a bitmask, when the frontend says it supports that.
const DEVICE_ID_JOYPAD_MASK: c_uint = 256;

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
}

struct Shared {
    pixel_format: c_int,
    frame: Frame,
    buttons: u16,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    pixel_format: PIXEL_0RGB1555,
    frame: Frame { width: 0, height: 0, rgb: Vec::new() },
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
            ENV_GET_CAN_DUPE => *(data as *mut bool) = true,
            ENV_GET_SYSTEM_DIRECTORY | ENV_GET_SAVE_DIRECTORY => *(data as *mut *const c_char) = std::ptr::null(),
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
            | ENV_SET_MEMORY_MAPS
            | ENV_SET_GEOMETRY => {}
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
    let format = s.pixel_format;
    let frame = &mut s.frame;
    frame.width = width;
    frame.height = height;
    frame.rgb.clear();
    frame.rgb.reserve(width as usize * height as usize * 3);
    for row in 0..height as usize {
        let line = &src[row * pitch..][..width as usize * bytes_per_pixel];
        to_rgb(format, line, &mut frame.rgb);
    }
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

extern "C" fn audio_sample(_left: i16, _right: i16) {}

extern "C" fn audio_batch(_data: *const i16, frames: usize) -> usize {
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

pub struct Core {
    api: Api,
    /// The core's name and version, e.g. "mGBA 0.11".
    pub name: String,
    pub fps: f64,
    /// The ROM's bytes, which the core may keep pointing into until the game is unloaded.
    _rom: Vec<u8>,
    // Dropped last, after `Drop` has shut the core down.
    _lib: libloading::Library,
}

impl Core {
    /// Loads the core at `core_path` and starts `rom` in it.
    pub fn load(core_path: &Path, rom: &Path) -> Result<Core> {
        if LOADED.swap(true, Ordering::SeqCst) {
            bail!("a core is already loaded");
        }
        Core::open(core_path, rom).inspect_err(|_| LOADED.store(false, Ordering::SeqCst))
    }

    fn open(core_path: &Path, rom: &Path) -> Result<Core> {
        *shared() = Shared { pixel_format: PIXEL_0RGB1555, frame: Frame::default(), buttons: 0 };
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
            Ok(Core { api, name, fps: av.timing.fps, _rom: data, _lib: lib })
        }
    }

    /// Runs one frame with these buttons held (bits from `button`).
    pub fn run_frame(&mut self, buttons: u16) {
        shared().buttons = buttons;
        // SAFETY: the game is loaded, and only one thread drives a core.
        unsafe { (self.api.run)() }
    }

    /// The last frame the core drew.
    pub fn frame(&self) -> Frame {
        shared().frame.clone()
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

/// Whether romi can open this ROM itself (it can't unpack archives yet).
pub fn can_open(rom: &Path) -> bool {
    !rom.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip") || e.eq_ignore_ascii_case("7z"))
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
    fn pixels_convert_to_full_range_rgb() {
        let mut out = Vec::new();
        to_rgb(PIXEL_RGB565, &0xffffu16.to_ne_bytes(), &mut out);
        to_rgb(PIXEL_RGB565, &0xf800u16.to_ne_bytes(), &mut out);
        to_rgb(PIXEL_0RGB1555, &0x001fu16.to_ne_bytes(), &mut out);
        to_rgb(PIXEL_XRGB8888, &0x00112233u32.to_ne_bytes(), &mut out);
        assert_eq!(out, [255, 255, 255, 255, 0, 0, 0, 0, 255, 0x11, 0x22, 0x33]);
    }
}

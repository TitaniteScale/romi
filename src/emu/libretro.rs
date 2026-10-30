//! A minimal libretro frontend: loads a core (the same `.so` RetroArch uses), gives it a ROM and
//! runs it one frame at a time. Video drawn in software or with OpenGL (see `gl`), turned upright
//! for arcade games on their side; stereo sound; a joypad.
//!
//! What cores log goes to the file `ROMI_CORE_LOG` names, with the requests romi turned down.
//!
//! libretro's callbacks are plain C functions with no user data, so what they share with romi
//! lives in a global, and only one core can be loaded at a time.

use crate::emu::options::{Options, Variable};
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::ThreadId;

// ---- the libretro ABI (libretro.h) ----

const API_VERSION: c_uint = 1;
const DEVICE_JOYPAD: c_uint = 1;
const DEVICE_ANALOG: c_uint = 5;
/// The analog device's indexes: its two sticks, and its buttons' analog values (the triggers).
const ANALOG_LEFT: c_uint = 0;
const ANALOG_RIGHT: c_uint = 1;
const ANALOG_BUTTON: c_uint = 2;
const ID_L2: c_uint = 12;
const ID_R2: c_uint = 13;
/// Asks for every button at once as a bitmask, when the frontend says it supports that.
const DEVICE_ID_JOYPAD_MASK: c_uint = 256;

const ENV_SET_ROTATION: c_uint = 1;
const ENV_GET_CAN_DUPE: c_uint = 3;
const ENV_SET_MESSAGE: c_uint = 6;
const ENV_SET_PERFORMANCE_LEVEL: c_uint = 8;
const ENV_GET_SYSTEM_DIRECTORY: c_uint = 9;
const ENV_SET_PIXEL_FORMAT: c_uint = 10;
const ENV_SET_INPUT_DESCRIPTORS: c_uint = 11;
const ENV_SET_HW_RENDER: c_uint = 14;
const ENV_GET_VARIABLE: c_uint = 15;
pub const ENV_SET_VARIABLES: c_uint = 16;
const ENV_GET_VARIABLE_UPDATE: c_uint = 17;
const ENV_SET_SUPPORT_NO_GAME: c_uint = 18;
const ENV_GET_LOG_INTERFACE: c_uint = 27;
const ENV_GET_SAVE_DIRECTORY: c_uint = 31;
const ENV_SET_SYSTEM_AV_INFO: c_uint = 32;
const ENV_SET_CONTROLLER_INFO: c_uint = 35;
const ENV_SET_MEMORY_MAPS: c_uint = 36;
const ENV_SET_GEOMETRY: c_uint = 37;
const ENV_SET_HW_SHARED_CONTEXT: c_uint = 44;
const ENV_GET_CORE_OPTIONS_VERSION: c_uint = 52;
pub const ENV_SET_CORE_OPTIONS: c_uint = 53;
pub const ENV_SET_CORE_OPTIONS_INTL: c_uint = 54;
const ENV_SET_CORE_OPTIONS_DISPLAY: c_uint = 55;
const ENV_GET_PREFERRED_HW_RENDER: c_uint = 56;
pub const ENV_SET_CORE_OPTIONS_V2: c_uint = 67;
pub const ENV_SET_CORE_OPTIONS_V2_INTL: c_uint = 68;
/// Flags a core may set on a command; the command is the same without them.
const ENV_FLAGS: c_uint = 0x10000 | 0x20000;

const PIXEL_0RGB1555: c_int = 0;
const PIXEL_XRGB8888: c_int = 1;
const PIXEL_RGB565: c_int = 2;

/// The game's battery-backed RAM, where cartridges keep their saves.
const MEMORY_SAVE_RAM: c_uint = 0;

/// One part of the console's address space and where the core keeps it (libretro.h's
/// `retro_memory_descriptor`). RetroAchievements reads memory through these.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MemoryDescriptor {
    flags: u64,
    ptr: *mut c_void,
    offset: usize,
    start: usize,
    select: usize,
    disconnect: usize,
    len: usize,
    addrspace: *const c_char,
}

/// libretro.h's `retro_memory_map`.
#[repr(C)]
pub struct MemoryMap {
    pub descriptors: *const MemoryDescriptor,
    pub num_descriptors: c_uint,
}

/// The memory map a core gave, copied, since the core needn't keep it, and which one it is: the
/// count goes up each time the core gives one. Cores give it while the game loads, or (mGBA) on
/// the first frame, and again when they reset.
#[derive(Clone, Default)]
pub struct Descriptors(pub Vec<MemoryDescriptor>, pub u64);

// SAFETY: the pointers are into the core's memory, which stays put while the game is loaded and is
// only read through them on the thread driving the core.
unsafe impl Send for Descriptors {}

/// The core's functions for its memory: where a kind of it is (`retro_get_memory_data`), and how
/// big (`retro_get_memory_size`).
pub type MemoryDataFn = unsafe extern "C" fn(c_uint) -> *mut c_void;
pub type MemorySizeFn = unsafe extern "C" fn(c_uint) -> usize;

/// What the video callback gets instead of pixels when the core renders with OpenGL or Vulkan.
const HW_FRAME_BUFFER_VALID: *const c_void = usize::MAX as *const c_void;
/// libretro's `RETRO_HW_CONTEXT_OPENGL`, the context romi says it prefers.
const HW_CONTEXT_OPENGL: c_uint = 1;

/// libretro.h's `retro_input_descriptor`: an input the game is played with, and what it does.
#[repr(C)]
struct InputDescriptor {
    port: c_uint,
    device: c_uint,
    index: c_uint,
    id: c_uint,
    description: *const c_char,
}

/// libretro.h's `retro_hw_render_callback`: how a core asks to draw with the GPU. The core fills
/// in what it wants, and the frontend the two functions it calls.
#[repr(C)]
struct HwRenderCallback {
    context_type: c_uint,
    context_reset: Option<unsafe extern "C" fn()>,
    get_current_framebuffer: Option<extern "C" fn() -> usize>,
    get_proc_address: Option<extern "C" fn(*const c_char) -> *const c_void>,
    depth: bool,
    stencil: bool,
    bottom_left_origin: bool,
    version_major: c_uint,
    version_minor: c_uint,
    cache_context: bool,
    context_destroy: Option<unsafe extern "C" fn()>,
    debug_context: bool,
}

/// What a core that draws with OpenGL asked for.
#[derive(Clone, Copy)]
struct Hw {
    request: crate::emu::gl::Request,
    /// Frames are drawn bottom row first, as GL does unless told otherwise.
    bottom_left: bool,
    /// Called once the context is made, to set up what the core draws with, and before it goes.
    reset: Option<unsafe extern "C" fn()>,
    destroy: Option<unsafe extern "C" fn()>,
}

extern "C" fn current_framebuffer() -> usize {
    crate::emu::gl::framebuffer() as usize
}

extern "C" fn proc_address(name: *const c_char) -> *const c_void {
    if name.is_null() {
        return std::ptr::null();
    }
    // SAFETY: the core gives a function's name as a C string.
    crate::emu::gl::proc_address(unsafe { CStr::from_ptr(name) })
}

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
    get_memory_data: MemoryDataFn,
    get_memory_size: MemorySizeFn,
}

impl Api {
    /// The game's battery-backed RAM, empty if it has none. The core owns it and changes it as the
    /// game runs; it stays put while the game is loaded.
    fn save_ram(&mut self) -> &mut [u8] {
        // SAFETY: the game is loaded, and the core keeps its save RAM until it's unloaded. Only the
        // thread driving the core touches it, between frames.
        unsafe {
            let data = (self.get_memory_data)(MEMORY_SAVE_RAM);
            let size = (self.get_memory_size)(MEMORY_SAVE_RAM);
            if data.is_null() || size == 0 {
                return &mut [];
            }
            std::slice::from_raw_parts_mut(data.cast(), size)
        }
    }
}

// ---- the joypad ----

/// libretro's joypad buttons, by the names `--input` scripts use, in libretro's id order.
pub const BUTTONS: [&str; 16] =
    ["b", "y", "select", "start", "up", "down", "left", "right", "a", "x", "l", "r", "l2", "r2", "l3", "r3"];

/// The bit for a button name, if it is one.
pub fn button(name: &str) -> Option<u16> {
    BUTTONS.iter().position(|b| b.eq_ignore_ascii_case(name)).map(|i| 1 << i)
}

/// What's held on the pad for a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Input {
    /// Bits from `button`.
    pub buttons: u16,
    /// The left and right sticks, x then y, from -32767 (left, up) to 32767 (right, down).
    pub sticks: [[i16; 2]; 2],
    /// How far L2 and R2 are pressed, from 0 to 32767.
    pub triggers: [i16; 2],
}

impl Input {
    pub fn buttons(buttons: u16) -> Input {
        Input { buttons, ..Input::default() }
    }

    /// The buttons, with the left stick pushed most of the way also pressing the d-pad, for cores
    /// that only read the d-pad.
    fn with_stick_as_dpad(&self) -> u16 {
        let [x, y] = self.sticks[0];
        let far = i16::MAX / 2;
        [(x < -far, "left"), (x > far, "right"), (y < -far, "up"), (y > far, "down")]
            .into_iter()
            .filter(|&(held, _)| held)
            .fold(self.buttons, |b, (_, name)| b | button(name).unwrap())
    }
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
        let mut out = Vec::new();
        self.resample_into(w, h, 3, &mut out);
        out
    }

    /// The same as `resample`, into `out` in place of what it held, as RGB (`bpp` 3) or RGBA (4,
    /// all opaque). Played frames are big, so reusing one `out` saves the time it takes the system
    /// to hand out fresh memory for each.
    pub fn resample_into(&self, w: u32, h: u32, bpp: usize, out: &mut Vec<u8>) {
        let (fw, fh) = (self.width as usize, self.height as usize);
        let (w, h) = (w as usize, h as usize);
        let xs: Vec<usize> = (0..w).map(|x| x * fw / w * 3).collect();
        out.clear();
        out.reserve(w * h * bpp);
        let mut last = None;
        for y in 0..h {
            let from = y * fh / h;
            if last == Some(from) {
                // Scaled up, most rows are the row above again.
                out.extend_from_within(out.len() - w * bpp..);
                continue;
            }
            last = Some(from);
            let row = &self.rgb[from * fw * 3..][..fw * 3];
            for &x in &xs {
                out.extend_from_slice(&row[x..x + 3]);
                if bpp == 4 {
                    out.push(255);
                }
            }
        }
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
    input: Input,
    /// The core says the game is played with the left stick (see `stick_is_stick`).
    uses_left_stick: bool,
    /// The core has asked where the sticks are.
    reads_sticks: bool,
    memory_map: Descriptors,
    /// What the core asked for to draw with OpenGL, if it did.
    hw: Option<Hw>,
    /// The size of the frame it last drew with OpenGL, not yet read back.
    hw_frame: Option<(c_uint, c_uint)>,
    /// The biggest picture the core says it draws, which its framebuffer must hold.
    max_size: (c_uint, c_uint),
    options: Option<Options>,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    pixel_format: PIXEL_0RGB1555,
    rotation: 0,
    geometry: (0, 0, 0.0),
    system_dir: None,
    save_dir: None,
    frame: Frame { width: 0, height: 0, rgb: Vec::new(), pixel_aspect: 0.0 },
    audio: Vec::new(),
    input: Input { buttons: 0, sticks: [[0; 2]; 2], triggers: [0; 2] },
    uses_left_stick: false,
    reads_sticks: false,
    memory_map: Descriptors(Vec::new(), 0),
    hw: None,
    hw_frame: None,
    max_size: (0, 0),
    options: None,
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
            ENV_GET_CORE_OPTIONS_VERSION => *(data as *mut c_uint) = 2,
            ENV_SET_VARIABLES
            | ENV_SET_CORE_OPTIONS
            | ENV_SET_CORE_OPTIONS_INTL
            | ENV_SET_CORE_OPTIONS_V2
            | ENV_SET_CORE_OPTIONS_V2_INTL => {
                let declared = crate::emu::options::declared(cmd & !ENV_FLAGS, data);
                shared().options.get_or_insert_default().declare(declared);
            }
            ENV_GET_VARIABLE => {
                let var = &mut *(data as *mut Variable);
                let Some(key) = (!var.key.is_null()).then(|| CStr::from_ptr(var.key).to_string_lossy()) else {
                    return false;
                };
                match shared().options.get_or_insert_default().get(&key) {
                    Some(value) => var.value = value,
                    None => return false,
                }
            }
            ENV_GET_LOG_INTERFACE => *(data as *mut *const c_void) = romi_core_log_to(on_core_log),
            ENV_GET_PREFERRED_HW_RENDER if crate::emu::gl::available().is_ok() => {
                *(data as *mut c_uint) = HW_CONTEXT_OPENGL
            }
            ENV_SET_HW_RENDER => {
                let cb = &mut *(data as *mut HwRenderCallback);
                let api = crate::emu::gl::Api::for_context(cb.context_type, cb.version_major, cb.version_minor);
                let Some(api) = api else {
                    note(&format!(
                        "romi: the core asked to draw with context type {}, which romi can't make",
                        cb.context_type
                    ));
                    return false;
                };
                if let Err(e) = crate::emu::gl::available() {
                    note(&format!("romi: the core asked to draw with OpenGL, but {e:#}"));
                    return false;
                }
                note(&format!("romi: the core draws with {api:?}"));
                cb.get_current_framebuffer = Some(current_framebuffer);
                cb.get_proc_address = Some(proc_address);
                let request =
                    crate::emu::gl::Request { api, depth: cb.depth, stencil: cb.stencil, debug: cb.debug_context };
                shared().hw = Some(Hw {
                    request,
                    bottom_left: cb.bottom_left_origin,
                    reset: cb.context_reset,
                    destroy: cb.context_destroy,
                });
            }
            ENV_SET_SYSTEM_AV_INFO => {
                let av = &*(data as *const AvInfo);
                let g = &av.geometry;
                let mut s = shared();
                s.geometry = (g.base_width, g.base_height, g.aspect_ratio);
                s.max_size = (g.max_width.max(g.base_width), g.max_height.max(g.base_height));
            }
            ENV_SET_INPUT_DESCRIPTORS => {
                let mut d = data as *const InputDescriptor;
                let mut left_stick = false;
                while !(*d).description.is_null() {
                    let d_ = &*d;
                    left_stick |= d_.port == 0 && d_.device == DEVICE_ANALOG && d_.index == ANALOG_LEFT;
                    d = d.add(1);
                }
                shared().uses_left_stick = left_stick;
            }
            ENV_SET_MESSAGE
            | ENV_SET_PERFORMANCE_LEVEL
            | ENV_SET_CORE_OPTIONS_DISPLAY
            | ENV_SET_SUPPORT_NO_GAME
            | ENV_SET_CONTROLLER_INFO
            | ENV_SET_HW_SHARED_CONTEXT => {}
            ENV_SET_MEMORY_MAPS => {
                let map = &*(data as *const MemoryMap);
                let descriptors = if map.descriptors.is_null() {
                    &[][..]
                } else {
                    std::slice::from_raw_parts(map.descriptors, map.num_descriptors as usize)
                };
                let mut s = shared();
                let given = s.memory_map.1 + 1;
                s.memory_map = Descriptors(descriptors.to_vec(), given);
            }
            ENV_SET_GEOMETRY => {
                let g = &*(data as *const Geometry);
                shared().geometry = (g.base_width, g.base_height, g.aspect_ratio);
            }
            // Everything else (bitmask input among it) is unsupported, and cores do without.
            other => {
                turned_down(other);
                return false;
            }
        }
    }
    true
}

// ---- what cores log ----

/// The file what cores log goes to, when it's set.
const LOG_VAR: &str = "ROMI_CORE_LOG";

unsafe extern "C" {
    /// From core_log.c: sends what cores log to `to`, and gives the function to hand them for it.
    fn romi_core_log_to(to: extern "C" fn(c_int, *const c_char)) -> *const c_void;
    /// C's standard streams (glibc's), which cores print to.
    static mut stdout: *mut libc::FILE;
    static mut stderr: *mut libc::FILE;
}

/// Points C's stdout and stderr at the `ROMI_CORE_LOG` file, or nowhere, for good. Some cores
/// print as they go (Mupen64Plus prints "plugin_start_gfx"), and in the TUI that landed on the
/// screen and scrolled it. romi itself writes to the terminal through Rust, which C's streams
/// don't touch.
fn quiet_c_streams() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let path = std::env::var_os(LOG_VAR).and_then(|p| CString::new(p.as_encoded_bytes()).ok());
        // SAFETY: fopen with valid strings; the streams are swapped once, before any core runs,
        // and the old ones are flushed first and left open, as nothing else should still use them.
        unsafe {
            let file = match &path {
                Some(path) => libc::fopen(path.as_ptr(), c"a".as_ptr()),
                None => std::ptr::null_mut(),
            };
            let file = if file.is_null() { libc::fopen(c"/dev/null".as_ptr(), c"w".as_ptr()) } else { file };
            if file.is_null() {
                return;
            }
            libc::setvbuf(file, std::ptr::null_mut(), libc::_IOLBF, 0);
            libc::fflush(std::ptr::null_mut());
            stdout = file;
            stderr = file;
        }
    });
}

/// Writes `message` to the `ROMI_CORE_LOG` file, when it's set.
pub fn note(message: &str) {
    use std::io::Write;
    let Some(path) = std::env::var_os(LOG_VAR) else { return };
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{}", message.trim_end());
    }
}

/// The last error the core logged, to say why it couldn't load a game.
static LAST_ERROR: Mutex<String> = Mutex::new(String::new());

extern "C" fn on_core_log(level: c_int, message: *const c_char) {
    if message.is_null() {
        return;
    }
    // SAFETY: core_log.c gives a NUL-terminated message, valid for this call.
    let message = unsafe { CStr::from_ptr(message) }.to_string_lossy();
    // Debug messages come by the thousand a second from some cores.
    if level == 0 || message.trim().is_empty() {
        return;
    }
    let label = ["debug", "info", "warn", "error"].get(level as usize).copied().unwrap_or("?");
    note(&format!("[{label}] {message}"));
    if level >= 3 {
        *LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = message.trim().to_string();
    }
}

/// Notes the first time the core asks for something romi doesn't do.
fn turned_down(cmd: c_uint) {
    static SEEN: Mutex<BTreeMap<c_uint, ()>> = Mutex::new(BTreeMap::new());
    if SEEN.lock().unwrap_or_else(|e| e.into_inner()).insert(cmd, ()).is_none() {
        note(&format!("romi: turned down environment call {cmd}"));
    }
}

extern "C" fn video(data: *const c_void, width: c_uint, height: c_uint, pitch: usize) {
    if data == HW_FRAME_BUFFER_VALID {
        // Drawn with OpenGL; it's read back once the frame is done (see `Core::run_frame`).
        shared().hw_frame = Some((width, height));
        return;
    }
    // A null frame repeats the last one.
    if data.is_null() || width == 0 || height == 0 {
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
    upright(frame, rotation, geometry);
}

/// Turns a frame just drawn upright, and says how wide its pixels are meant to look.
fn upright(frame: &mut Frame, rotation: c_uint, geometry: (c_uint, c_uint, f32)) {
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

extern "C" fn input_state(port: c_uint, device: c_uint, index: c_uint, id: c_uint) -> i16 {
    if port != 0 {
        return 0;
    }
    let (input, stick_is_stick) = {
        let mut s = shared();
        if device == DEVICE_ANALOG && !s.reads_sticks {
            s.reads_sticks = true;
            if s.uses_left_stick {
                note("romi: the game is played with the left stick, so it isn't also the d-pad");
            }
        }
        (s.input, s.uses_left_stick && s.reads_sticks)
    };
    match device {
        // The left stick is the d-pad too, unless the game plays with it as a stick: the core says
        // it does, and asks where it is. An N64 game would see the d-pad pressed whenever you
        // walk, and some cores list a stick they never read (Beetle Saturn's for its 3D pad).
        DEVICE_JOYPAD => {
            let buttons = match stick_is_stick {
                true => input.buttons,
                false => input.with_stick_as_dpad(),
            };
            match id {
                DEVICE_ID_JOYPAD_MASK => buttons as i16,
                0..16 => (buttons >> id & 1) as i16,
                _ => 0,
            }
        }
        DEVICE_ANALOG => {
            match (index, id) {
                (ANALOG_LEFT | ANALOG_RIGHT, 0..2) => input.sticks[index as usize][id as usize],
                (ANALOG_BUTTON, ID_L2) => input.triggers[0],
                (ANALOG_BUTTON, ID_R2) => input.triggers[1],
                // Other buttons are all the way down or not at all.
                (ANALOG_BUTTON, 0..16) => (input.buttons >> id & 1) as i16 * i16::MAX,
                _ => 0,
            }
        }
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
    /// The game's save file (`.srm`), read when it starts and written by `save`. Without one, the
    /// game starts with nothing saved and what it saves is lost.
    pub save_file: Option<PathBuf>,
    /// Core options set for the core and the game, by key (see `options`).
    pub options: BTreeMap<String, String>,
}

pub struct Core {
    api: Api,
    /// The core's name and version, e.g. "mGBA 0.11".
    pub name: String,
    pub fps: f64,
    /// Sound samples per second, per channel.
    pub sample_rate: f64,
    save_file: Option<PathBuf>,
    /// The save as it is in the save file, to tell when the game has saved since.
    saved: Vec<u8>,
    /// The ROM's bytes, which the core may keep pointing into until the game is unloaded.
    _rom: Vec<u8>,
    /// Where the ROM was unpacked to, for a zip the core can't open itself.
    _unpacked: Option<crate::files::TempDir>,
    /// For a core that draws with OpenGL: what it asked for, its context, and the thread that has
    /// the context current.
    hw: Option<Hw>,
    gl: Option<crate::emu::gl::Context>,
    gl_thread: Option<ThreadId>,
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
            input: Input::default(),
            uses_left_stick: false,
            reads_sticks: false,
            memory_map: Descriptors::default(),
            hw: None,
            hw_frame: None,
            max_size: (0, 0),
            options: Some(Options::new(dirs.options.clone())),
        };
        LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner()).clear();
        // SAFETY: loading a core runs its initializers; libretro cores are made to be loaded this way.
        let lib = unsafe { libloading::Library::new(core_path) }
            .with_context(|| format!("can't load {}", core_path.display()))?;
        keep_thread_pools_loaded();
        quiet_c_streams();
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
                get_memory_data: sym!("retro_get_memory_data", MemoryDataFn),
                get_memory_size: sym!("retro_get_memory_size", MemorySizeFn),
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
                let why = std::mem::take(&mut *LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner()));
                let why = if why.is_empty() { String::new() } else { format!(": {why}") };
                bail!("{name} couldn't load {}{why}", rom.display());
            }
            // A pad in each port, as RetroArch says by default. Some cores plug nothing in until
            // they're told: Flycast waits to hear about all four ports before it plugs in a pad and
            // its memory card, and Mupen64Plus has no controllers without it.
            if let Ok(plug_in) = lib.get::<unsafe extern "C" fn(c_uint, c_uint)>(b"retro_set_controller_port_device\0")
            {
                for port in 0..4 {
                    plug_in(port, DEVICE_JOYPAD);
                }
            }
            let mut av = AvInfo::default();
            sym!("retro_get_system_av_info", unsafe extern "C" fn(*mut AvInfo))(&mut av);
            let g = &av.geometry;
            let (hw, max_size) = {
                let mut s = shared();
                s.geometry = (g.base_width, g.base_height, g.aspect_ratio);
                s.max_size = (g.max_width.max(g.base_width), g.max_height.max(g.base_height));
                (s.hw, s.max_size)
            };
            // Not under the lock: the core calls back while it sets up.
            let gl = match hw.map(|hw| start_gl(&hw, max_size)) {
                Some(Ok(gl)) => Some(gl),
                Some(Err(e)) => {
                    (api.unload_game)();
                    (api.deinit)();
                    return Err(e.context(format!("{name} draws with OpenGL")));
                }
                None => None,
            };
            let mut core = Core {
                api,
                name,
                fps: av.timing.fps,
                sample_rate: av.timing.sample_rate,
                save_file: dirs.save_file.clone(),
                saved: Vec::new(),
                _rom: data,
                _unpacked: unpacked.map(|(dir, _)| dir),
                hw,
                gl,
                gl_thread: None,
                _lib: lib,
            };
            if let Err(e) = core.load_save() {
                // Dropping the core saves, which would write over the save that couldn't be read.
                core.save_file = None;
                return Err(e);
            }
            Ok(core)
        }
    }

    /// Puts the save file into the game's save RAM, as RetroArch does: as much of it as fits. A
    /// save that's there but can't be read stops the game from starting, rather than letting it
    /// write over the save later.
    fn load_save(&mut self) -> Result<()> {
        let Some(path) = &self.save_file else { return Ok(()) };
        let ram = self.api.save_ram();
        if ram.is_empty() {
            return Ok(());
        }
        match std::fs::read(path) {
            Ok(save) => {
                let n = save.len().min(ram.len());
                ram[..n].copy_from_slice(&save[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("can't read the save {}", path.display())),
        }
        self.saved = ram.to_vec();
        Ok(())
    }

    /// Writes the game's save RAM to its save file, if the game has saved since it was read or
    /// last written. Returns whether it wrote.
    pub fn save(&mut self) -> Result<bool> {
        let Some(path) = &self.save_file else { return Ok(false) };
        let ram = self.api.save_ram();
        if ram.is_empty() || *ram == *self.saved {
            return Ok(false);
        }
        crate::files::write_atomic(path, &*ram).with_context(|| format!("can't write the save {}", path.display()))?;
        self.saved = ram.to_vec();
        Ok(true)
    }

    /// Runs one frame with `input` held.
    pub fn run_frame(&mut self, input: Input) {
        let max_size = {
            let mut s = shared();
            s.input = input;
            s.audio.clear();
            s.max_size
        };
        if let Some(gl) = &mut self.gl {
            // The context follows the core to whichever thread runs it.
            let here = std::thread::current().id();
            if self.gl_thread != Some(here) {
                if let Err(e) = gl.make_current() {
                    return note(&format!("romi: {e:#}"));
                }
                self.gl_thread = Some(here);
            }
            let (w, h) = gl.size();
            if (max_size.0 > w || max_size.1 > h)
                && let Err(e) = gl.resize((max_size.0.max(w), max_size.1.max(h)))
            {
                note(&format!("romi: {e:#}"));
            }
        }
        // SAFETY: the game is loaded, and only one thread drives a core.
        unsafe { (self.api.run)() }
        let (Some(gl), Some(hw)) = (&mut self.gl, &self.hw) else { return };
        let mut s = shared();
        let Some((w, h)) = s.hw_frame.take().filter(|&(w, h)| w > 0 && h > 0) else { return };
        let (rotation, geometry) = (s.rotation, s.geometry);
        let frame = &mut s.frame;
        gl.read((w, h), hw.bottom_left, &mut frame.rgb);
        let (w, h) = (w.min(gl.size().0), h.min(gl.size().1));
        (frame.width, frame.height) = (w, h);
        upright(frame, rotation, geometry);
    }

    /// The last frame the core drew.
    /// The size the core says its picture is meant to be seen at, before it has drawn any: its
    /// base size turned upright, in square pixels as tall as it is. Frames can still differ.
    pub fn display_size(&self) -> (f64, f64) {
        let s = shared();
        let (w, h, _) = s.geometry;
        let (w, h) = if s.rotation % 2 == 1 { (h, w) } else { (w, h) };
        (f64::from(w) * pixel_aspect(s.geometry, s.rotation), f64::from(h))
    }

    pub fn frame(&self) -> Frame {
        shared().frame.clone()
    }

    /// The memory map the core gave, if it gave one, for reading the console's memory.
    pub fn memory_map(&self) -> Descriptors {
        shared().memory_map.clone()
    }

    /// Which memory map the core gave last (see `Descriptors`), to tell when there's a new one.
    pub fn memory_map_given(&self) -> u64 {
        shared().memory_map.1
    }

    /// The core's own functions for finding its memory, which stay valid while it's loaded.
    pub fn memory_functions(&self) -> (MemoryDataFn, MemorySizeFn) {
        (self.api.get_memory_data, self.api.get_memory_size)
    }

    /// Appends the last frame's sound to `out`, as interleaved left and right samples.
    pub fn audio(&self, out: &mut Vec<i16>) {
        out.extend_from_slice(&shared().audio);
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        // Whoever drove the core has saved and reported what failed; this only catches the rest.
        let _ = self.save();
        // The core lets go of what it drew with as the game unloads, so its context is current.
        if let Some(gl) = &self.gl
            && self.gl_thread != Some(std::thread::current().id())
        {
            let _ = gl.make_current();
        }
        // SAFETY: the game was loaded and the core initialized in `load`.
        unsafe {
            (self.api.unload_game)();
            if let Some(destroy) = self.hw.and_then(|hw| hw.destroy) {
                destroy();
            }
            (self.api.deinit)();
        }
        self.gl = None;
        LOADED.store(false, Ordering::SeqCst);
    }
}

/// Keeps OpenMP loaded for good once a core has loaded it. Its threads outlive the core that
/// started them, and unloading the library under them, as unloading the core did, crashed romi
/// when a game played with bsnes-hd stopped.
fn keep_thread_pools_loaded() {
    // SAFETY: with RTLD_NOLOAD, dlopen only marks a library that's already loaded; it loads nothing.
    unsafe { libc::dlopen(c"libgomp.so.1".as_ptr(), libc::RTLD_NOW | libc::RTLD_NOLOAD | libc::RTLD_NODELETE) };
}

/// Makes the OpenGL context a core asked for, with a framebuffer of `size`, and lets the core set
/// up what it draws with. The context is let go after, for whichever thread runs the core.
fn start_gl(hw: &Hw, size: (c_uint, c_uint)) -> Result<crate::emu::gl::Context> {
    let gl = crate::emu::gl::Context::new(hw.request, size)?;
    note(&format!("romi: drawing on {}, {}×{}", gl.renderer(), size.0, size.1));
    if let Some(reset) = hw.reset {
        // SAFETY: the core's own callback, called as libretro says: once its context is current.
        unsafe { reset() };
    }
    gl.release();
    Ok(gl)
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
        let mut rgba = vec![9; 100];
        frame.resample_into(2, 2, 4, &mut rgba);
        assert_eq!(rgba, [1, 1, 1, 255, 2, 2, 2, 255].repeat(2));
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

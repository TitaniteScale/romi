//! OpenGL for cores that draw with the GPU (libretro's `SET_HW_RENDER`), with no window: a context
//! made through EGL, current on whichever thread runs the core, and a framebuffer the core draws
//! each frame into, read back into memory to be shown like any other frame.
//!
//! libEGL is opened the first time a core asks, so romi still runs where there's none. It draws on
//! the GPU with a screen plugged in, which on a laptop with two is the one the desktop is on, so
//! the other can stay asleep; failing that, any GPU EGL lists, or Mesa's surfaceless display. GL's
//! own functions come from `eglGetProcAddress`, as the core's do.

use anyhow::{Context as _, Result, bail};
use std::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

type Display = *mut c_void;
type Config = *mut c_void;
type EglContext = *mut c_void;

const EGL_NONE: i32 = 0x3038;
const EGL_TRUE: u32 = 1;
const EGL_EXTENSIONS: i32 = 0x3055;
const EGL_SURFACE_TYPE: i32 = 0x3033;
const EGL_RENDERABLE_TYPE: i32 = 0x3040;
const EGL_RED_SIZE: i32 = 0x3024;
const EGL_GREEN_SIZE: i32 = 0x3023;
const EGL_BLUE_SIZE: i32 = 0x3022;
const EGL_OPENGL_BIT: i32 = 0x0008;
const EGL_OPENGL_ES2_BIT: i32 = 0x0004;
const EGL_OPENGL_ES3_BIT: i32 = 0x0040;
const EGL_OPENGL_API: u32 = 0x30A2;
const EGL_OPENGL_ES_API: u32 = 0x30A0;
const EGL_CONTEXT_MAJOR_VERSION: i32 = 0x3098;
const EGL_CONTEXT_MINOR_VERSION: i32 = 0x30FB;
const EGL_CONTEXT_OPENGL_PROFILE_MASK: i32 = 0x30FD;
const EGL_CONTEXT_OPENGL_CORE_PROFILE_BIT: i32 = 0x1;
const EGL_CONTEXT_OPENGL_COMPATIBILITY_PROFILE_BIT: i32 = 0x2;
const EGL_CONTEXT_OPENGL_DEBUG: i32 = 0x31B0;
const EGL_PLATFORM_SURFACELESS_MESA: u32 = 0x31DD;
const EGL_PLATFORM_DEVICE_EXT: u32 = 0x313F;
const EGL_DRM_RENDER_NODE_FILE_EXT: i32 = 0x3377;

const GL_FRAMEBUFFER: u32 = 0x8D40;
const GL_RENDERBUFFER: u32 = 0x8D41;
const GL_COLOR_ATTACHMENT0: u32 = 0x8CE0;
const GL_DEPTH_ATTACHMENT: u32 = 0x8D00;
const GL_DEPTH_STENCIL_ATTACHMENT: u32 = 0x821A;
const GL_FRAMEBUFFER_COMPLETE: u32 = 0x8CD5;
const GL_RGBA8: u32 = 0x8058;
const GL_DEPTH_COMPONENT24: u32 = 0x81A6;
const GL_DEPTH24_STENCIL8: u32 = 0x88F0;
const GL_RGBA: u32 = 0x1908;
const GL_UNSIGNED_BYTE: u32 = 0x1401;
const GL_PIXEL_PACK_BUFFER: u32 = 0x88EB;
const GL_PACK_ALIGNMENT: u32 = 0x0D05;
const GL_PACK_ROW_LENGTH: u32 = 0x0D02;
const GL_RENDERER: u32 = 0x1F01;
const GL_VERSION: u32 = 0x1F02;

/// Which OpenGL a core asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Api {
    /// OpenGL with everything old still in (the compatibility profile), as new as there is.
    Compat,
    /// The core profile, at least this version.
    Core(u32, u32),
    /// OpenGL ES, at least this version.
    Gles(u32, u32),
}

impl Api {
    /// The API for libretro's `retro_hw_context_type`, and the version a core gave with it. None
    /// for what romi can't make (Vulkan, Direct3D).
    pub fn for_context(context_type: u32, major: u32, minor: u32) -> Option<Api> {
        match context_type {
            1 => Some(Api::Compat),
            2 => Some(Api::Gles(2, 0)),
            3 if major == 0 => Some(Api::Core(3, 2)),
            3 => Some(Api::Core(major, minor)),
            4 => Some(Api::Gles(3, 0)),
            5 => Some(Api::Gles(major, minor)),
            _ => None,
        }
    }
}

/// What a core needs drawn into.
#[derive(Clone, Copy, Debug)]
pub struct Request {
    pub api: Api,
    pub depth: bool,
    pub stencil: bool,
    pub debug: bool,
}

/// libEGL's functions.
struct Egl {
    _lib: libloading::Library,
    get_proc_address: unsafe extern "C" fn(*const c_char) -> *const c_void,
    get_platform_display: unsafe extern "C" fn(u32, *mut c_void, *const isize) -> Display,
    initialize: unsafe extern "C" fn(Display, *mut i32, *mut i32) -> u32,
    query_string: unsafe extern "C" fn(Display, i32) -> *const c_char,
    bind_api: unsafe extern "C" fn(u32) -> u32,
    choose_config: unsafe extern "C" fn(Display, *const i32, *mut Config, i32, *mut i32) -> u32,
    create_context: unsafe extern "C" fn(Display, Config, EglContext, *const i32) -> EglContext,
    make_current: unsafe extern "C" fn(Display, *mut c_void, *mut c_void, EglContext) -> u32,
    destroy_context: unsafe extern "C" fn(Display, EglContext) -> u32,
    get_error: unsafe extern "C" fn() -> i32,
}

// SAFETY: plain function pointers into a library that's never unloaded.
unsafe impl Send for Egl {}
unsafe impl Sync for Egl {}

/// libEGL, opened the first time it's wanted.
fn egl() -> Result<&'static Egl> {
    static EGL: OnceLock<Result<Egl, String>> = OnceLock::new();
    EGL.get_or_init(|| open_egl().map_err(|e| format!("{e:#}"))).as_ref().map_err(|e| anyhow::anyhow!("{e}"))
}

fn open_egl() -> Result<Egl> {
    // SAFETY: libEGL's initializers are made to run when it's loaded; the signatures are EGL 1.5's.
    unsafe {
        let lib = libloading::Library::new("libEGL.so.1").context("can't load libEGL.so.1")?;
        macro_rules! sym {
            ($name:literal) => {
                *lib.get(concat!($name, "\0").as_bytes()).context(concat!("libEGL has no ", $name))?
            };
        }
        Ok(Egl {
            get_proc_address: sym!("eglGetProcAddress"),
            get_platform_display: sym!("eglGetPlatformDisplay"),
            initialize: sym!("eglInitialize"),
            query_string: sym!("eglQueryString"),
            bind_api: sym!("eglBindAPI"),
            choose_config: sym!("eglChooseConfig"),
            create_context: sym!("eglCreateContext"),
            make_current: sym!("eglMakeCurrent"),
            destroy_context: sym!("eglDestroyContext"),
            get_error: sym!("eglGetError"),
            _lib: lib,
        })
    }
}

/// Whether romi can make OpenGL contexts here, and why not.
pub fn available() -> Result<()> {
    egl().map(|_| ())
}

/// A GL or EGL function by name, for the core (libretro's `get_proc_address`) and for romi.
pub fn proc_address(name: &CStr) -> *const c_void {
    match egl() {
        // SAFETY: eglGetProcAddress takes any name, and gives null for one it doesn't know.
        Ok(egl) => unsafe { (egl.get_proc_address)(name.as_ptr()) },
        Err(_) => std::ptr::null(),
    }
}

/// The framebuffer the core draws into, for libretro's `get_current_framebuffer`.
static FRAMEBUFFER: AtomicU32 = AtomicU32::new(0);

pub fn framebuffer() -> u32 {
    FRAMEBUFFER.load(Ordering::Relaxed)
}

/// The GL functions romi calls itself.
struct Fns {
    gen_framebuffers: unsafe extern "C" fn(i32, *mut u32),
    delete_framebuffers: unsafe extern "C" fn(i32, *const u32),
    bind_framebuffer: unsafe extern "C" fn(u32, u32),
    framebuffer_renderbuffer: unsafe extern "C" fn(u32, u32, u32, u32),
    check_framebuffer_status: unsafe extern "C" fn(u32) -> u32,
    gen_renderbuffers: unsafe extern "C" fn(i32, *mut u32),
    delete_renderbuffers: unsafe extern "C" fn(i32, *const u32),
    bind_renderbuffer: unsafe extern "C" fn(u32, u32),
    renderbuffer_storage: unsafe extern "C" fn(u32, u32, i32, i32),
    bind_buffer: unsafe extern "C" fn(u32, u32),
    pixel_storei: unsafe extern "C" fn(u32, i32),
    read_pixels: unsafe extern "C" fn(i32, i32, i32, i32, u32, u32, *mut c_void),
    get_string: unsafe extern "C" fn(u32) -> *const c_char,
    get_error: unsafe extern "C" fn() -> u32,
}

impl Fns {
    fn load() -> Result<Fns> {
        macro_rules! gl {
            ($name:literal) => {
                // SAFETY: each field's type is the signature of the GL function of that name.
                unsafe { function(CStr::from_bytes_with_nul(concat!($name, "\0").as_bytes()).unwrap())? }
            };
        }
        Ok(Fns {
            gen_framebuffers: gl!("glGenFramebuffers"),
            delete_framebuffers: gl!("glDeleteFramebuffers"),
            bind_framebuffer: gl!("glBindFramebuffer"),
            framebuffer_renderbuffer: gl!("glFramebufferRenderbuffer"),
            check_framebuffer_status: gl!("glCheckFramebufferStatus"),
            gen_renderbuffers: gl!("glGenRenderbuffers"),
            delete_renderbuffers: gl!("glDeleteRenderbuffers"),
            bind_renderbuffer: gl!("glBindRenderbuffer"),
            renderbuffer_storage: gl!("glRenderbufferStorage"),
            bind_buffer: gl!("glBindBuffer"),
            pixel_storei: gl!("glPixelStorei"),
            read_pixels: gl!("glReadPixels"),
            get_string: gl!("glGetString"),
            get_error: gl!("glGetError"),
        })
    }
}

/// The GL or EGL function called `name`, as a function pointer of type `F`.
///
/// # Safety
///
/// `F` must be a function pointer type with that function's signature.
unsafe fn function<F: Copy>(name: &CStr) -> Result<F> {
    let f = proc_address(name);
    if f.is_null() {
        bail!("OpenGL has no {}", name.to_string_lossy());
    }
    assert_eq!(size_of::<F>(), size_of::<*const c_void>(), "not a function pointer");
    // SAFETY: a non-null pointer to the function, read as the pointer type the caller vouches for.
    Ok(unsafe { std::mem::transmute_copy::<*const c_void, F>(&f) })
}

/// A context with a framebuffer for the core to draw into.
pub struct Context {
    display: Display,
    context: EglContext,
    fns: Fns,
    request: Request,
    fbo: u32,
    /// The color and depth buffers behind it.
    buffers: [u32; 2],
    size: (u32, u32),
    /// The last frame read back, as the GPU gives it: RGBA, bottom row first.
    pixels: Vec<u8>,
}

// SAFETY: an EGL context can be made current on any thread, once no other thread has it current,
// and romi only uses it from one thread at a time (see `make_current`).
unsafe impl Send for Context {}

impl Context {
    /// Makes a context as `request` asks, current on this thread, with a framebuffer of `size`.
    pub fn new(request: Request, size: (u32, u32)) -> Result<Context> {
        let egl = egl()?;
        let display = display(egl)?;
        // SAFETY: EGL calls on an initialized display, with attribute lists ended by EGL_NONE.
        unsafe {
            let (api, renderable) = match request.api {
                Api::Gles(3.., _) => (EGL_OPENGL_ES_API, EGL_OPENGL_ES3_BIT),
                Api::Gles(..) => (EGL_OPENGL_ES_API, EGL_OPENGL_ES2_BIT),
                _ => (EGL_OPENGL_API, EGL_OPENGL_BIT),
            };
            if (egl.bind_api)(api) != EGL_TRUE {
                bail!("EGL can't make OpenGL contexts here ({:#x})", (egl.get_error)());
            }
            // No surfaces are drawn to, so any kind of config will do.
            let attribs = [
                EGL_SURFACE_TYPE,
                0,
                EGL_RENDERABLE_TYPE,
                renderable,
                EGL_RED_SIZE,
                8,
                EGL_GREEN_SIZE,
                8,
                EGL_BLUE_SIZE,
                8,
                EGL_NONE,
            ];
            let mut config = std::ptr::null_mut();
            let mut found = 0;
            if (egl.choose_config)(display, attribs.as_ptr(), &mut config, 1, &mut found) != EGL_TRUE || found == 0 {
                bail!("EGL has no config for {:?} ({:#x})", request.api, (egl.get_error)());
            }
            let mut attribs = Vec::new();
            match request.api {
                Api::Compat => {}
                Api::Core(major, minor) => attribs.extend([
                    EGL_CONTEXT_MAJOR_VERSION,
                    major as i32,
                    EGL_CONTEXT_MINOR_VERSION,
                    minor as i32,
                    EGL_CONTEXT_OPENGL_PROFILE_MASK,
                    EGL_CONTEXT_OPENGL_CORE_PROFILE_BIT,
                ]),
                Api::Gles(major, minor) => {
                    attribs.extend([EGL_CONTEXT_MAJOR_VERSION, major as i32, EGL_CONTEXT_MINOR_VERSION, minor as i32])
                }
            }
            if request.api == Api::Compat {
                attribs.extend([EGL_CONTEXT_OPENGL_PROFILE_MASK, EGL_CONTEXT_OPENGL_COMPATIBILITY_PROFILE_BIT]);
            }
            if request.debug {
                attribs.extend([EGL_CONTEXT_OPENGL_DEBUG, 1]);
            }
            attribs.push(EGL_NONE);
            let context = (egl.create_context)(display, config, std::ptr::null_mut(), attribs.as_ptr());
            if context.is_null() {
                bail!("EGL can't make a {:?} context ({:#x})", request.api, (egl.get_error)());
            }
            if (egl.make_current)(display, std::ptr::null_mut(), std::ptr::null_mut(), context) != EGL_TRUE {
                let error = (egl.get_error)();
                (egl.destroy_context)(display, context);
                bail!("EGL can't use a context without a surface ({error:#x})");
            }
            let fns = Fns::load()?;
            let mut gl =
                Context { display, context, fns, request, fbo: 0, buffers: [0; 2], size: (0, 0), pixels: Vec::new() };
            (gl.fns.gen_framebuffers)(1, &mut gl.fbo);
            (gl.fns.gen_renderbuffers)(2, gl.buffers.as_mut_ptr());
            gl.resize(size)?;
            Ok(gl)
        }
    }

    /// What's drawing: the GPU's name and the GL version, as the driver says.
    pub fn renderer(&self) -> String {
        let text = |name| {
            // SAFETY: the context is current; glGetString gives a static string or null.
            let s = unsafe { (self.fns.get_string)(name) };
            if s.is_null() { String::new() } else { unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned() }
        };
        format!("{}, OpenGL {}", text(GL_RENDERER), text(GL_VERSION))
    }

    /// Makes the context current on this thread. Whichever thread had it must have let it go.
    pub fn make_current(&self) -> Result<()> {
        let egl = egl()?;
        // SAFETY: the display and context are this one's, alive until it's dropped.
        unsafe {
            if (egl.make_current)(self.display, std::ptr::null_mut(), std::ptr::null_mut(), self.context) != EGL_TRUE {
                bail!("can't make the OpenGL context current ({:#x})", (egl.get_error)());
            }
        }
        Ok(())
    }

    /// Lets go of the context on this thread, so another can take it.
    pub fn release(&self) {
        if let Ok(egl) = egl() {
            // SAFETY: releasing whatever is current on this thread is always allowed.
            unsafe {
                (egl.make_current)(self.display, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
            };
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Gives the framebuffer room for `size` pixels. The context must be current.
    pub fn resize(&mut self, (w, h): (u32, u32)) -> Result<()> {
        let (w, h) = (w.max(1), h.max(1));
        let f = &self.fns;
        // SAFETY: GL calls with the context current, on objects it made.
        unsafe {
            (f.bind_renderbuffer)(GL_RENDERBUFFER, self.buffers[0]);
            (f.renderbuffer_storage)(GL_RENDERBUFFER, GL_RGBA8, w as i32, h as i32);
            (f.bind_framebuffer)(GL_FRAMEBUFFER, self.fbo);
            (f.framebuffer_renderbuffer)(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_RENDERBUFFER, self.buffers[0]);
            if self.request.depth {
                let (format, attachment) = match self.request.stencil {
                    true => (GL_DEPTH24_STENCIL8, GL_DEPTH_STENCIL_ATTACHMENT),
                    false => (GL_DEPTH_COMPONENT24, GL_DEPTH_ATTACHMENT),
                };
                (f.bind_renderbuffer)(GL_RENDERBUFFER, self.buffers[1]);
                (f.renderbuffer_storage)(GL_RENDERBUFFER, format, w as i32, h as i32);
                (f.framebuffer_renderbuffer)(GL_FRAMEBUFFER, attachment, GL_RENDERBUFFER, self.buffers[1]);
            }
            let status = (f.check_framebuffer_status)(GL_FRAMEBUFFER);
            (f.bind_renderbuffer)(GL_RENDERBUFFER, 0);
            if status != GL_FRAMEBUFFER_COMPLETE {
                bail!("can't make a {w}×{h} framebuffer to draw into ({status:#x})");
            }
        }
        self.size = (w, h);
        FRAMEBUFFER.store(self.fbo, Ordering::Relaxed);
        Ok(())
    }

    /// Reads back the `w`×`h` frame the core drew, into `rgb` as packed RGB, top row first. A core
    /// drawing with GL's usual bottom-left origin (`bottom_left`) has it upside down.
    pub fn read(&mut self, (w, h): (u32, u32), bottom_left: bool, rgb: &mut Vec<u8>) {
        let (w, h) = (w.min(self.size.0) as usize, h.min(self.size.1) as usize);
        self.pixels.resize(w * h * 4, 0);
        let f = &self.fns;
        // SAFETY: GL calls with the context current; `pixels` holds exactly w×h RGBA pixels, packed
        // with no padding, which is what the pack settings ask for.
        unsafe {
            // The core may have left these set for its own reads.
            (f.bind_buffer)(GL_PIXEL_PACK_BUFFER, 0);
            (f.pixel_storei)(GL_PACK_ALIGNMENT, 4);
            (f.pixel_storei)(GL_PACK_ROW_LENGTH, 0);
            (f.bind_framebuffer)(GL_FRAMEBUFFER, self.fbo);
            (f.read_pixels)(0, 0, w as i32, h as i32, GL_RGBA, GL_UNSIGNED_BYTE, self.pixels.as_mut_ptr().cast());
            // OpenGL ES 2 has no row length to set; the error is left for no one.
            (f.get_error)();
        }
        rgb.clear();
        rgb.reserve(w * h * 3);
        for row in 0..h {
            let y = if bottom_left { h - 1 - row } else { row };
            for px in self.pixels[y * w * 4..][..w * 4].as_chunks::<4>().0 {
                rgb.extend_from_slice(&px[..3]);
            }
        }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        let Ok(egl) = egl() else { return };
        // SAFETY: the objects are this context's; it's made current to delete them, then let go.
        unsafe {
            if self.make_current().is_ok() {
                (self.fns.delete_framebuffers)(1, &self.fbo);
                (self.fns.delete_renderbuffers)(2, self.buffers.as_ptr());
            }
            self.release();
            (egl.destroy_context)(self.display, self.context);
        }
        FRAMEBUFFER.store(0, Ordering::Relaxed);
    }
}

/// An initialized EGL display with no window system, on the GPU `pick_device` chooses, or Mesa's
/// surfaceless one. It's never terminated, since a later core uses it again.
fn display(egl: &Egl) -> Result<Display> {
    // SAFETY: EGL's client queries take no display; each display is initialized before use, and
    // the extension functions are only called when EGL says it has them.
    unsafe {
        let extensions = (egl.query_string)(std::ptr::null_mut(), EGL_EXTENSIONS);
        let extensions = if extensions.is_null() { "" } else { CStr::from_ptr(extensions).to_str().unwrap_or("") };
        let has = |name: &str| extensions.split(' ').any(|e| e == name);
        let init = |display: Display| {
            let (mut major, mut minor) = (0, 0);
            (!display.is_null() && (egl.initialize)(display, &mut major, &mut minor) == EGL_TRUE).then_some(display)
        };
        if has("EGL_EXT_platform_device")
            && let Some(device) = pick_device()
            && let Some(display) = init((egl.get_platform_display)(EGL_PLATFORM_DEVICE_EXT, device, std::ptr::null()))
        {
            return Ok(display);
        }
        if has("EGL_MESA_platform_surfaceless")
            && let Some(display) =
                init((egl.get_platform_display)(EGL_PLATFORM_SURFACELESS_MESA, std::ptr::null_mut(), std::ptr::null()))
        {
            return Ok(display);
        }
        bail!(
            "EGL has no display without a window system to draw with (it needs EGL_EXT_platform_device or \
             EGL_MESA_platform_surfaceless)"
        )
    }
}

/// The GPU to draw on, from those EGL lists: one with a screen plugged in, or else the first with
/// a DRM render node (software renderers have none).
fn pick_device() -> Option<*mut c_void> {
    // SAFETY: EGL_EXT_device_enumeration's and EGL_EXT_device_query's functions, by their spec's
    // signatures; the strings they give are EGL's, valid while the device is.
    unsafe {
        let query: unsafe extern "C" fn(i32, *mut *mut c_void, *mut i32) -> u32 =
            function(c"eglQueryDevicesEXT").ok()?;
        let query_string: unsafe extern "C" fn(*mut c_void, i32) -> *const c_char =
            function(c"eglQueryDeviceStringEXT").ok()?;
        let mut devices = [std::ptr::null_mut(); 16];
        let mut n = 0;
        if query(devices.len() as i32, devices.as_mut_ptr(), &mut n) != EGL_TRUE {
            return None;
        }
        let with_nodes: Vec<(*mut c_void, String)> = devices[..n.clamp(0, 16) as usize]
            .iter()
            .filter_map(|&device| {
                let node = query_string(device, EGL_DRM_RENDER_NODE_FILE_EXT);
                (!node.is_null()).then(|| (device, CStr::from_ptr(node).to_string_lossy().into_owned()))
            })
            .collect();
        with_nodes.iter().find(|(_, node)| drives_a_screen(node)).or(with_nodes.first()).map(|&(device, _)| device)
    }
}

/// Whether the GPU behind a DRM render node (`/dev/dri/renderD128`) has a screen plugged in.
fn drives_a_screen(node: &str) -> bool {
    let Some(name) = std::path::Path::new(node).file_name() else { return false };
    let drm = std::path::Path::new("/sys/class/drm");
    let Ok(nodes) = std::fs::read_dir(drm.join(name).join("device/drm")) else { return false };
    nodes.flatten().any(|card| {
        let card = card.file_name().to_string_lossy().into_owned();
        let prefix = format!("{card}-");
        card.starts_with("card")
            && std::fs::read_dir(drm.join(&card)).is_ok_and(|connectors| {
                connectors.flatten().any(|c| {
                    c.file_name().to_string_lossy().starts_with(&prefix)
                        && std::fs::read_to_string(c.path().join("status")).is_ok_and(|s| s.trim() == "connected")
                })
            })
    })
}

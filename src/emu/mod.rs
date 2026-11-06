//! Playing games inside romi: a minimal libretro frontend, the thread a game runs on, its sound
//! and controls, and the two ways of showing it, the whole terminal (`romi run --live`) and the
//! TUI's middle panel, both over the Kitty graphics protocol.

pub mod audio;
#[cfg(all(target_os = "linux", feature = "gamescope"))]
pub mod capture;
pub mod cheevos;
pub mod controls;
pub mod crop;
pub mod embedded;
pub mod gl;
pub mod kitty;
pub mod libretro;
pub mod live;
pub mod notice;
pub mod options;
pub mod remap;
pub mod runner;
pub mod savestate;
pub mod screen;
pub mod timing;
pub mod unlocks;
pub mod wireframe;

/// Why games in the `gamescope` launch mode can't play in romi's panel here, if they can't. They
/// open in a window instead. Looked at once, so installing gamescope needs romi restarted.
pub fn gamescope_missing() -> Option<&'static str> {
    if !cfg!(target_os = "linux") {
        return Some("gamescope is Linux only");
    }
    if !cfg!(feature = "gamescope") {
        return Some("this romi is built without the gamescope mode");
    }
    #[cfg(all(target_os = "linux", feature = "gamescope"))]
    {
        static INSTALLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*INSTALLED.get_or_init(capture::available) {
            return Some("gamescope isn't installed");
        }
    }
    None
}

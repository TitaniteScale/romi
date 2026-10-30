//! Playing games inside romi: a minimal libretro frontend, the thread a game runs on, its sound
//! and controls, and the two ways of showing it, the whole terminal (`romi run --live`) and the
//! TUI's middle panel, both over the Kitty graphics protocol.

pub mod audio;
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
pub mod runner;
pub mod screen;
pub mod timing;
pub mod wireframe;

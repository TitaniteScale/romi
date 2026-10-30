//! Compiles rcheevos (RetroAchievements' C library, in `vendor/rcheevos`) and romi's glue to it,
//! for achievements in games played inside romi, and the shim that takes what cores log. Also
//! works out the version `romi --version` says: Cargo.toml's, with `ROMI_BUILD` after a `+` when
//! it's set, as the edge builds set it to `edge.<commit>`.

use std::path::Path;

fn main() {
    let version = env!("CARGO_PKG_VERSION");
    let version = match std::env::var("ROMI_BUILD") {
        Ok(build) if !build.is_empty() => format!("{version}+{build}"),
        _ => version.to_string(),
    };
    println!("cargo:rustc-env=ROMI_VERSION={version}");
    println!("cargo:rerun-if-env-changed=ROMI_BUILD");
    let vendor = Path::new("vendor");
    let src = vendor.join("rcheevos/src");
    let mut sources = vec![Path::new("src/emu/cheevos.c").to_path_buf()];
    let mut dirs = vec![src.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("vendor/rcheevos/src is there") {
            let path = entry.expect("vendor/rcheevos/src can be read").path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "c") {
                sources.push(path);
            }
        }
    }
    sources.sort();
    cc::Build::new()
        .files(&sources)
        .include(vendor.join("rcheevos/include"))
        .include(&src)
        .include(vendor.join("libretro"))
        // Its code, not ours: warnings in it aren't romi's to fix.
        .warnings(false)
        .compile("rcheevos");
    cc::Build::new().file("src/emu/core_log.c").include(vendor.join("libretro")).compile("romi_core_log");
    println!("cargo:rerun-if-changed=src/emu/cheevos.c");
    println!("cargo:rerun-if-changed=src/emu/core_log.c");
    println!("cargo:rerun-if-changed=vendor");
}

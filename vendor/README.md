# Vendored C code

- `rcheevos/`: [rcheevos](https://github.com/RetroAchievements/rcheevos) v12.5.0, RetroAchievements'
  own library for checking achievements against a game's memory and talking to its server. MIT
  (`rcheevos/LICENSE`). Its `include/` and the `.c` and `.h` files under `src/`, unchanged.
- `libretro/libretro.h`: the libretro API header from
  [libretro-common](https://github.com/libretro/libretro-common), which rcheevos' libretro helpers
  include. MIT (its own header).

`build.rs` compiles them, with romi's glue in `src/emu/cheevos.c`.

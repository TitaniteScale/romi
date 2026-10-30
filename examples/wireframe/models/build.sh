#!/bin/sh
# Makes romi's wireframes (src/emu/wireframes) from the models here. Run it from the repository's
# root after changing a model or the importer. The models are credited in CREDITS.md.
set -eu

import="target/release/examples/wireframe-import"
cargo build --quiet --release --example wireframe-import
models="examples/wireframe/models"
out="src/emu/wireframes"
parts=$(mktemp -d)
trap 'rm -rf "$parts"' EXIT

tv_credit='"old tv" by Justin Randall (https://poly.pizza/m/9y-mTmCyefL)'
nes_credit='"NES" by Jason Toff (https://poly.pizza/m/bEUJNyZrdlp)'
snes_credit='"SNES" by Gabriel Valdivia (https://poly.pizza/m/7yVvvpp-ftQ)'
arcade_credit='"Arcade Machine" by J-Toastie (https://poly.pizza/m/GLDkMhiynM)'
license='CC BY 3.0, made into a wireframe'

# A 25" set on legs, facing -z in the file. Its knobs, buttons and logo are packed so tight that their
# lines glow into blobs, so pieces under 2.5 cm across are left out.
tv="--front -z --width 64 --min-part 2.5"

# The Game Boy stands facing +x. Its picture is 10:9.
$import "$models/gameboy/model.obj" --front x --height 14.8 --aspect 10:9 --system gb \
  --credit "\"Gameboy\" by Jason Toff (https://poly.pizza/m/dczUnD4aslM), $license" -o "$out/gameboy.json"

# An upright cabinet facing +x, for arcade boards. The glass is the opening in its bezel.
$import "$models/arcade-machine/ArcadeMachine.obj" --front x --height 178 --system arcade \
  --credit "$arcade_credit, $license" -o "$out/arcade.json"

# The TV on its own, for every system without a console of its own.
$import "$models/old-tv/model.obj" $tv --system tv --credit "$tv_credit, $license" -o "$out/tv.json"

# The consoles, on the floor in front of the TV and to its right, turned a little toward the
# middle. Pieces 4 on are the NES's grip ridges, dozens of thin plates that draw as a smudge.
$import "$models/nes/model.obj" --no-glass --width 25.4 --drop 4-59 -o "$parts/nes.json"
$import "$models/old-tv/model.obj" $tv --add "$parts/nes.json:24,26,-20" --system nes \
  --credit "$tv_credit and $nes_credit, $license" -o "$out/nes.json"

# The SNES faces -x, with its controller propped in front of it: pieces 2, 3 and 8 on are the
# controller, its cord and the plug.
$import "$models/snes/model.obj" --no-glass --front -x --width 25 --drop 2,3,8-20 -o "$parts/snes.json"
$import "$models/old-tv/model.obj" $tv --add "$parts/snes.json:24,26,-20" --system snes \
  --credit "$tv_credit and $snes_credit, $license" -o "$out/snes.json"

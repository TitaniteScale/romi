# Wireframe launch animation

A prototype of romi's launch animation. A console builds itself out of lines (an NES and a TV, or a
Game Boy), then the camera dives into its screen and lands on the cells where live play draws the
game. A test
picture stands in for the game.

It needs a terminal that takes Kitty graphics through shared memory, such as Ghostty or kitty.

Run these from the repository's root.

## Run it

```sh
cargo run --release --example wireframe
```

The game's file name seeds the animation, so a game always builds the same way. Its extension picks
the model: Game Boy games (`.gb`, `.gbc`) get the Game Boy, and everything else the NES and TV.

```sh
cargo run --release --example wireframe -- "Some Game (USA).nes"
cargo run --release --example wireframe -- "Some Game (USA).gb"
cargo run --release --example wireframe -- --seed 12345
```

## Pick how lines enter

```sh
cargo run --release --example wireframe -- --style pen
cargo run --release --example wireframe -- --style explode
cargo run --release --example wireframe -- --style scan
cargo run --release --example wireframe -- --style fly
cargo run --release --example wireframe -- --style dots
cargo run --release --example wireframe -- --style vortex
```

## Pick which lines go first

```sh
cargo run --release --example wireframe -- --order walk
cargo run --release --example wireframe -- --order bottom
cargo run --release --example wireframe -- --order front
cargo run --release --example wireframe -- --order screen
cargo run --release --example wireframe -- --order random
```

## Timing and looks

```sh
# Slow everything down: a 4 second build and a 3 second zoom
cargo run --release --example wireframe -- --build 4 --zoom 3

# Lines one after another instead of all at once, and the zoom starting halfway through the build
cargo run --release --example wireframe -- --stagger 1 --overlap 0.5

# A wider camera
cargo run --release --example wireframe -- --fov 70

# Draw at the terminal's full pixel size instead of half
cargo run --release --example wireframe -- --shrink 1
```

Every option, with its default:

```sh
cargo run --release --example wireframe -- --help
```

## Colours

The lines are drawn over the terminal's own background, in a colour the seed picks, so a game always
gets the same one. At the start the animation asks the terminal for its background and its ANSI
colours (red, green, yellow, blue, magenta, cyan and their bright versions). Ghostty and kitty both
answer. The beam can be any of them that stands out from the background (a contrast ratio of 3 or
more), so it fits whatever theme is on. On a light background the lines don't burn white where they
cross, and glow less.

A terminal that doesn't answer gets a few colours of romi's own. Or pick the colours:

```sh
cargo run --release --example wireframe -- --colours ffb547
cargo run --release --example wireframe -- --colours 9fe3ff,ffb547,ff79c6
```

The status line at the end says which colour it was and where it came from.

## Keys

| Key        | Does                |
| ---------- | ------------------- |
| any key    | skips to the end    |
| `r`        | replays             |
| `n`        | tries a new seed    |
| `q` or Esc | quits               |

When the animation ends, the bottom line shows the seed, the style and order, how long frames took to
draw, and how close the screen landed to the game's cells.

## The model

`nes.json` holds the model: `vertices` in cm (y up, +z toward the viewer), `edges` as pairs of vertex
numbers, and `screen`, the numbers of the glass's corners as top left, top right, bottom right and
bottom left. The camera ends up facing those four corners.

To play another model, such as one made by the importer below:

```sh
cargo run --release --example wireframe -- --model tv.json
```

## The models

`nes.json` was drawn by hand. `gameboy.json` was made by `wireframe-import` from a model in
`models/`, which holds the downloaded originals. They're other people's work under CC BY 3.0, so
each one is credited in [CREDITS.md](../../CREDITS.md), and a new one needs a line there too.

To make `gameboy.json` again:

```sh
cargo run --release --example wireframe-import -- examples/wireframe/models/gameboy/model.obj \
  --front x --height 14.8 --aspect 10:9 --system gb \
  --credit '"Gameboy" by Jason Toff (https://poly.pizza/m/dczUnD4aslM), CC BY 3.0, made into a wireframe' \
  -o examples/wireframe/gameboy.json
```

## Make a model from a downloaded one

`wireframe-import` turns an OBJ, STL, glTF or GLB file into a model for this. Models on Printables
and Thingiverse (STL, for 3D printing) are often close to real size; Sketchfab has more, in glTF.
Use ones licensed CC0 or CC-BY, and put who made it in `--credit`.

```sh
cargo run --release --example wireframe-import -- tv.stl -o tv.json
```

A downloaded model is made of triangles, and drawing every edge gives a mesh rather than a drawing.
The importer keeps the edges where the surface folds by more than `--angle` degrees (30), where it
ends, and where a big flat side bends into a curve, so a rounded corner shows as two lines. Then it
joins lines that run straight on.

It prints the model's size and a list of surfaces that might be the glass, which is worth a look.
The glass is the likeliest of the smooth surfaces that face front and have one outline: big, and
shaped like the picture (`--aspect`).


```text
size 48.0 × 40.0 × 44.0 cm (width × height × depth)
glass candidates (facing front, one outline):
  1. 33.0 × 25.0 cm, 824 cm², centered 21.5 cm up   ← the glass
  2. 3.6 × 3.6 cm, 10 cm², centered 30.0 cm up
lines: 494 folds, ends and bends, 494 once straight runs are joined, 494 kept
```

| If                                       | Try                                                      |
| ---------------------------------------- | -------------------------------------------------------- |
| the size is wrong                        | `--unit mm`, `cm`, `m` or `in`, or `--width 48` (in cm)  |
| it's lying down or facing the wrong way  | `--up z` and `--front -y` (any of x, y, z, -x, -y, -z)   |
| the wrong surface is the glass           | `--glass 2`, by its number in the list                   |
| curved parts come out as bands of lines  | a bigger `--angle`, like 40                              |
| tiny details clutter it                  | `--min-part 1`, dropping pieces under 1 cm across        |
| the picture isn't 4:3                    | `--aspect 10:9`, say, for a Game Boy                     |

STL files are taken to be in mm with z up and their front toward -y, as 3D printing files usually
are. Everything else is taken to be in m with y up and its front toward +z, as glTF files are and
OBJ files from Blender are.

### A console beside a TV

Import the console with `--no-glass`, then add it when importing the TV. The numbers are where it
goes, in cm from the middle of the glass along the floor (right, then toward you), and how far to
turn it, counterclockwise seen from above:

```sh
cargo run --release --example wireframe-import -- nes.stl --no-glass -o console.json
cargo run --release --example wireframe-import -- tv.stl --add console.json:32,28,-18 --system nes -o nes.json
```


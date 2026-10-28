# Wireframe launch animation

A prototype of romi's launch animation. An NES and a TV build themselves out of lines, then the
camera dives into the TV's screen and lands on the cells where live play draws the game. A test
picture stands in for the game.

It needs a terminal that takes Kitty graphics through shared memory, such as Ghostty or kitty.

Run these from the repository's root.

## Run it

```sh
cargo run --release --example wireframe
```

The game's file name seeds the animation, so a game always builds the same way:

```sh
cargo run --release --example wireframe -- "Some Game (USA).nes"
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

# A wider camera and an amber beam
cargo run --release --example wireframe -- --fov 70 --beam ffb547

# Draw at the terminal's full pixel size instead of half
cargo run --release --example wireframe -- --shrink 1
```

Every option, with its default:

```sh
cargo run --release --example wireframe -- --help
```

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

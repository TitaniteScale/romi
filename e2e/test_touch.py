"""Touching a game's screen with the mouse, as a DS game's lower screen is touched: the left button
held over the picture is a libretro pointer pressed there."""

import json
import subprocess

from drive import CONFIG
from test_run import DRAWING_CORE, GAME_IMAGE, play_in_romi

# The drawing core, writing where its screen is touched to SEEN on every frame: x, y, and whether
# it is.
TOUCH_CORE = DRAWING_CORE.replace(
    "void retro_set_input_state(void *f) {}",
    """#include <stdio.h>
typedef short (*state_fn)(unsigned, unsigned, unsigned, unsigned);
static state_fn state;
void retro_set_input_state(state_fn f) { state = f; }
static void record(void) {
    FILE *f = fopen("SEEN" ".tmp", "w");
    fprintf(f, "%d %d %d", state(0, 6, 0, 0), state(0, 6, 0, 1), state(0, 6, 0, 2));
    fclose(f);
    rename("SEEN" ".tmp", "SEEN");
}""",
).replace("void retro_run(void) {", "void retro_run(void) { record();")

PLACEHOLDER = "\U0010eeee"


def set_up(sandbox):
    seen = sandbox.home / "seen.txt"
    (sandbox.home / "touch.c").write_text(TOUCH_CORE.replace('"SEEN"', f'"{seen}"'))
    core = sandbox.home / "touch_libretro.so"
    subprocess.run(["cc", "-shared", "-fPIC", "-o", str(core), str(sandbox.home / "touch.c")], check=True)
    (sandbox.home / "ROMs/gb").mkdir(parents=True)
    (sandbox.home / "ROMs/gb/Pocket Lantern (USA).gb").touch()
    config = json.loads(json.dumps(CONFIG))
    config["systems"] = [{"id": "gb", "name": "Game Boy", "extensions": ["gb"], "command": f"true -L {core} {{rom}}"}]
    config.update(launch_animation={"enabled": False}, volume=0)
    sandbox.write_config(config)
    return seen


def picture(romi):
    """The picture's cells: left, top, right, bottom, the last two past the end."""
    cells = [(x, y) for y, row in romi.screen.buffer.items() for x, c in row.items() if c.data.startswith(PLACEHOLDER)]
    xs, ys = [x for x, _ in cells], [y for _, y in cells]
    return min(xs), min(ys), max(xs) + 1, max(ys) + 1


def touched(romi, seen):
    """What the core sees of the pointer, a moment after the last change."""
    romi.pump(0.4)
    return [int(n) for n in seen.read_text().split()]


def mouse(romi, col, row, press=True, button=0):
    """The mouse's button pressed or let go over a cell, told in cells as SGR (mode 1006)."""
    romi.send(b"\x1b[<%d;%d;%d%s" % (button, col + 1, row + 1, b"M" if press else b"m"))


def pointer(at, across):
    """romi's pointer position for pixel `at` of `across`: its middle, from -32768 to 32767."""
    return round((at + 0.5) * 65536 / across - 32768)


def test_the_mouse_touches_the_pixel_under_it(start, sandbox):
    seen = set_up(sandbox)
    romi = start()
    play_in_romi(romi)
    assert b"\x1b[?1000h" in romi.output and b"\x1b[?1006h" in romi.output
    # The test terminal doesn't answer whether it can tell the mouse in pixels.
    assert b"\x1b[?1016h" not in romi.output
    romi.wait_for(lambda: PLACEHOLDER in romi.text())
    left, top, right, bottom = picture(romi)
    assert touched(romi, seen) == [0, 0, 0]

    # Each cell's middle, as a part of the 160×144 picture, is the pixel touched.
    col, row = (left + right) // 2, top
    mouse(romi, col, row)
    x = int((col - left + 0.5) / (right - left) * 160)
    y = int(0.5 / (bottom - top) * 144)
    assert touched(romi, seen) == [pointer(x, 160), pointer(y, 144), 1]
    # Dragged, it follows; dragged off the picture, it holds to the edge.
    romi.send(b"\x1b[<32;%d;%dM" % (col + 1, bottom))
    assert touched(romi, seen)[1:] == [pointer(int((bottom - 1 - top + 0.5) / (bottom - top) * 144), 144), 1]
    romi.send(b"\x1b[<32;%d;%dM" % (col + 1, bottom + 2))
    assert touched(romi, seen)[1:] == [pointer(143, 144), 1]
    mouse(romi, col, row, press=False)
    assert touched(romi, seen) == [0, 0, 0]

    # Off the picture, or another button, touches nothing.
    mouse(romi, 0, 0)
    assert touched(romi, seen) == [0, 0, 0]
    mouse(romi, 0, 0, press=False)
    mouse(romi, col, row, button=2)
    assert touched(romi, seen) == [0, 0, 0]
    mouse(romi, col, row, press=False, button=2)

    at = len(romi.output)
    romi.keys("\x1b")
    romi.wait_for(lambda: b"a=d,d=I," + GAME_IMAGE[:-1] in romi.output[at:])
    assert b"\x1b[?1000l" in romi.output[at:], "the mouse is the terminal's again"


def test_the_mouse_is_told_in_pixels_where_the_terminal_can(start, sandbox):
    seen = set_up(sandbox)
    romi = start(answer_modes={1016: 2})
    play_in_romi(romi)
    assert b"\x1b[?1016h" in romi.output
    romi.wait_for(lambda: PLACEHOLDER in romi.text())
    left, top, right, bottom = picture(romi)
    # The test terminal gives no pixel sizes, so romi takes cells as 8×16. Its pixels count from 1,
    # as cells do: the picture's top left pixel, then one 3 pixels in and 10 down.
    for dx, dy in [(0, 0), (3, 10)]:
        romi.send(b"\x1b[<0;%d;%dM" % (left * 8 + dx + 1, top * 16 + dy + 1))
        x, y = int(dx / ((right - left) * 8) * 160), int(dy / ((bottom - top) * 16) * 144)
        assert touched(romi, seen) == [pointer(x, 160), pointer(y, 144), 1]
        romi.send(b"\x1b[<0;%d;%dm" % (left * 8 + dx + 1, top * 16 + dy + 1))

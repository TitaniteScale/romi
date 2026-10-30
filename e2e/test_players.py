"""More than one player: each controller on a port of its own, simulated with two virtual
controllers, and the players screen, which says who's player 1."""

import json
import subprocess

from drive import CONFIG, Pad
from test_run import DRAWING_CORE, GAME_IMAGE, play_in_romi, run_cli

A, B, START = 1 << 8, 1 << 0, 1 << 3
# What romi calls the two virtual controllers, the second as the second of the same kind.
VIRTUAL = ["romi test pad", "romi test pad #2"]

# The drawing core, writing what's held on each of its four ports to SEEN on every frame.
PORTS_CORE = DRAWING_CORE.replace(
    "void retro_set_input_state(void *f) {}",
    """#include <stdio.h>
typedef short (*state_fn)(unsigned, unsigned, unsigned, unsigned);
static state_fn state;
void retro_set_input_state(state_fn f) { state = f; }
static void record(void) {
    FILE *f = fopen("SEEN", "w");
    for (unsigned port = 0; port < 4; port++) {
        unsigned held = 0;
        for (unsigned id = 0; id < 16; id++) held |= (state(port, 1, 0, id) != 0) << id;
        fprintf(f, "%u ", held);
    }
    fclose(f);
}""",
).replace("void retro_run(void) {", "void retro_run(void) { record();")


def set_up(sandbox):
    seen = sandbox.home / "seen.txt"
    (sandbox.home / "ports.c").write_text(PORTS_CORE.replace('"SEEN"', f'"{seen}"'))
    core = sandbox.home / "ports_libretro.so"
    subprocess.run(["cc", "-shared", "-fPIC", "-o", str(core), str(sandbox.home / "ports.c")], check=True)
    config = json.loads(json.dumps(CONFIG))
    config["systems"][0]["command"] = f"true -L {core} {{rom}}"
    config.update(controller={"enabled": True}, launch_animation={"enabled": False}, volume=0)
    sandbox.write_config(config)
    # The virtual controllers first, ahead of any real one plugged into the machine.
    sandbox.write_state({"players": VIRTUAL})
    return core, seen


def ports(romi, seen):
    """What each port holds, a moment after the last change."""
    romi.pump(0.5)
    return [int(n) for n in seen.read_text().split()]


def held(romi, seen, *holding):
    """What each port holds while each (pad, button) in `holding` is held."""
    for pad, button in holding:
        pad.down(button)
    try:
        return ports(romi, seen)
    finally:
        for pad, button in holding:
            pad.up(button)
        romi.pump(0.3)


def stop(romi):
    seen = len(romi.output)
    romi.keys("\x1b")
    romi.wait_for(lambda: b"a=d,d=I," + GAME_IMAGE[:-1] in romi.output[seen:])
    romi.pump(1)  # past the pause for buttons still held from the game


def test_each_controller_is_a_player_of_its_own(start, sandbox, pad):
    _, seen = set_up(sandbox)
    second = Pad()
    try:
        romi = start(pad=pad)
        play_in_romi(romi)
        assert ports(romi, seen) == [0, 0, 0, 0]
        both = held(romi, seen, (pad, "SOUTH"), (second, "EAST"))
        # One of them is player 1 and the other player 2, each with its own button.
        assert sorted(both[:2]) == sorted([B, A]) and both[2:] == [0, 0], both
        first_player = 0 if both[0] == B else 1
        alone = held(romi, seen, (second, "EAST"))
        assert alone[1 - first_player] == A and alone[first_player] == 0, alone
        # Select and Start held on player 2's controller still stops the game.
        at = len(romi.output)
        second.down("SELECT")
        second.down("START")
        romi.wait_for(lambda: b"a=d,d=I," + GAME_IMAGE[:-1] in romi.output[at:], timeout=5)
        second.up("SELECT")
        second.up("START")
    finally:
        second.close()


def open_players(romi):
    romi.keys("m")
    romi.wait_for("Players…")
    for _ in range(20):
        if "Players…" in " ".join(romi.selected()):
            break
        romi.keys("j")
    romi.keys("\r")
    romi.wait_for("Press a button on player 1's controller")


def tap(romi, pad, button="SOUTH"):
    pad.down(button)
    romi.pump(0.1)
    pad.up(button)
    romi.pump(0.3)


def test_the_players_screen_says_whose_controller_is_player_1(start, sandbox, pad):
    _, seen = set_up(sandbox)
    second = Pad()
    try:
        romi = start(pad=pad)
        for first, then in [(second, pad), (pad, second)]:
            open_players(romi)
            tap(romi, first)
            romi.wait_for("Press a button on player 2's controller")
            tap(romi, then)  # the last one closes the screen...
            if "Players" in romi.text() and "Players: 1 " not in romi.text():
                tap(romi, then, "EAST")  # ...or B, with a real controller plugged in too
            romi.wait_for(lambda: "Players: 1 " in romi.text())
            order = sandbox.state()["players"]
            assert sorted(order[:2]) == VIRTUAL
            play_in_romi(romi)
            assert held(romi, seen, (first, "SOUTH"))[:2] == [B, 0]
            assert held(romi, seen, (then, "EAST"))[:2] == [0, A]
            stop(romi)
    finally:
        second.close()


def test_scripts_press_other_players_buttons(sandbox):
    core, seen = set_up(sandbox)
    script = sandbox.home / "input.txt"
    script.write_text("0 start p2:a p4:b\n")
    out = run_cli(sandbox, "run", "advance wars", "--core", str(core), "--frames", "2", "--input", str(script))
    assert out.returncode == 0, out.stderr
    assert [int(n) for n in seen.read_text().split()] == [START, A, 0, B]

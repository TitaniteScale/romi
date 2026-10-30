"""The controls screen: remapping a game's buttons, or its system's, for playing it in romi."""

import json
import subprocess

from drive import CONFIG
from test_run import DRAWING_CORE, GAME_IMAGE, PAD_CORE, play_in_romi, run_cli
from test_hide import jsonc, overrides

KEY = "gba:Advance Wars (USA).gba"
A, B, X, START = 1 << 8, 1 << 0, 1 << 9, 1 << 3


def set_up(sandbox, source=PAD_CORE, name="pad", **settings):
    """The test library, its Game Boy Advance games played with a made-up core."""
    seen = sandbox.home / "seen.txt"
    (sandbox.home / f"{name}.c").write_text(source.replace('"SEEN"', f'"{seen}"'))
    core = sandbox.home / f"{name}_libretro.so"
    subprocess.run(["cc", "-shared", "-fPIC", "-o", str(core), str(sandbox.home / f"{name}.c")], check=True)
    config = json.loads(json.dumps(CONFIG))
    config["systems"][0]["command"] = f"true -L {core} {{rom}}"
    config.update(settings)
    sandbox.write_config(config)
    return core, seen


def held_in_a_run(sandbox, core, seen, buttons):
    """What the core sees with `buttons` held, in `romi run`, which plays with the same controls."""
    script = sandbox.home / "input.txt"
    script.write_text(f"0 {buttons}\n")
    out = run_cli(sandbox, "run", "advance wars", "--core", str(core), "--frames", "2", "--input", str(script))
    assert out.returncode == 0, out.stderr
    return int(seen.read_text().split()[1])


def open_controls(romi):
    romi.keys("o")
    romi.wait_for("Controls…")
    romi.keys("j\r")
    romi.wait_for("Controls: Advance Wars")


def row(romi):
    """The selected row's text."""
    return " ".join(romi.selected())


def select(romi, label):
    for _ in range(20):
        if label in row(romi):
            return
        romi.keys("j")
    raise AssertionError(f"no row {label!r} in\n{romi.text()}")


def test_a_games_buttons_swap_and_play_that_way(start, sandbox):
    core, seen = set_up(sandbox)
    romi = start()
    open_controls(romi)
    # With no names from the core yet, the buttons are named as on libretro's pad, and each is
    # sent by its own key.
    assert "D-pad up" in row(romi)
    select(romi, "A ")
    assert "x" in row(romi)
    romi.keys("\r")
    romi.wait_for("press a button…")
    romi.keys("s")  # X's key, for A
    romi.wait_for(lambda: "• s" in row(romi))
    assert overrides(sandbox) == {KEY: {"controls": {"a": "x", "x": "a"}}}
    assert held_in_a_run(sandbox, core, seen, "x start") == A | START
    assert held_in_a_run(sandbox, core, seen, "a") == X

    # Reset goes back to the buttons as they were.
    select(romi, "Reset")
    romi.keys("\r")
    romi.wait_for(lambda: "• s" not in romi.text())
    assert overrides(sandbox) == {}
    assert held_in_a_run(sandbox, core, seen, "x") == X
    # Its own saves aren't taken for someone else's: no reload.
    romi.pump(1.5)
    assert "reloaded" not in romi.text()


def test_a_systems_buttons_go_in_the_config_under_the_games(start, sandbox):
    core, seen = set_up(sandbox)
    romi = start()
    open_controls(romi)
    romi.keys("k")
    romi.wait_for(lambda: "This game" in row(romi))
    romi.keys("l")
    romi.wait_for("All Game Boy Advance games")
    select(romi, "B ")
    romi.keys("\r")
    romi.wait_for("press a button…")
    romi.keys("x")  # A's key, for B
    romi.wait_for(lambda: "• x" in row(romi))
    assert jsonc(sandbox.config_path)["controls"] == {"gba": {"a": "b", "b": "a"}}
    assert held_in_a_run(sandbox, core, seen, "a") == B

    # The game's own go over the system's: Start for A (where B was).
    romi.keys("kkkkkk")
    romi.wait_for(lambda: "All Game Boy Advance games" in row(romi))
    romi.keys("h")
    romi.wait_for(lambda: "This game" in row(romi))
    select(romi, "A ")
    assert "z" in row(romi), "A comes from B's key, as the system has it"
    romi.keys("\r")
    romi.wait_for("press a button…")
    romi.send(b"\r")  # Start's key
    romi.wait_for(lambda: "• enter" in row(romi))
    assert overrides(sandbox) == {KEY: {"controls": {"b": "start", "start": "a"}}}
    assert held_in_a_run(sandbox, core, seen, "start") == A
    assert held_in_a_run(sandbox, core, seen, "b") == START
    assert held_in_a_run(sandbox, core, seen, "a") == B, "the system's, where the game says nothing"


# The drawing core, telling romi what its buttons do.
NAMING_CORE = DRAWING_CORE.replace(
    "void retro_set_environment(void *f) {}",
    """struct desc { unsigned port, device, index, id; const char *description; };
static struct desc descs[] = { { 0, 1, 0, 8, "Fire" }, { 0, 1, 0, 0, "Jump" }, { 0, 1, 0, 3, "Pause" }, { 0 } };
void retro_set_environment(_Bool (*env)(unsigned, void *)) { env(11, descs); }""",
)


def test_the_cores_names_for_the_buttons_are_remembered_from_playing(start, sandbox):
    set_up(sandbox, NAMING_CORE, "naming", launch_animation={"enabled": False}, volume=0)
    romi = start()
    play_in_romi(romi)
    seen = len(romi.output)
    romi.keys("\x1b")
    romi.wait_for(lambda: b"a=d,d=I," + GAME_IMAGE[:-1] in romi.output[seen:])
    romi.pump(1)
    open_controls(romi)
    # Only the buttons the game uses, in the d-pad, face buttons, Start order.
    names = [word for line in romi.lines() for word in line.split() if word in ("Fire", "Jump", "Pause")]
    assert names == ["Fire", "Jump", "Pause"], romi.text()
    assert "D-pad up" not in romi.text()


def test_a_controller_picks_the_button_and_is_named_as_in_the_hints(start, sandbox, pad):
    set_up(sandbox, controller={"enabled": True})
    romi = start(pad=pad)
    romi.wait_for("A play")
    open_controls(romi)
    # libretro's A is the right face button, B on an Xbox pad, as the hints call it.
    for _ in range(4):
        romi.press("DOWN")
    romi.wait_for(lambda: "A " in row(romi) and "B" in row(romi).split("A ", 1)[1])
    romi.press("SOUTH")  # A on the pad: change
    romi.wait_for("press a button…")
    romi.press("WEST")  # X on the pad
    romi.wait_for(lambda: "• X" in row(romi))
    assert overrides(sandbox) == {KEY: {"controls": {"a": "y", "y": "a"}}}

"""The cover grid: toggled with v (or L3, or the menu), remembered, and moved around in 2D."""

import time

from drive import CONFIG


def eventually(check, timeout=3.0):
    end = time.time() + timeout
    while not check():
        assert time.time() < end, "timed out"
        time.sleep(0.05)


def caption(romi):
    """The grid's caption: the selected game's name, at the bottom of its pane."""
    return [line for line in romi.lines() if "·" in line and "╰" in line][-1]


def test_toggle_and_remember(start, sandbox):
    romi = start()
    romi.keys("v")
    romi.wait_for("Advance Wars · Game Boy Advance · 2001 · Strategy")
    eventually(lambda: sandbox.state().get("grid") is True)
    romi.close()

    again = start()
    again.wait_for("Advance Wars · Game Boy Advance")
    again.keys("v")
    again.wait_for(lambda: "Advance Wars" in again.selected())
    eventually(lambda: sandbox.state().get("grid") is False)


def test_moving_around_the_grid(start, sandbox):
    sandbox.write_state({"grid": True})
    romi = start()
    romi.wait_for("Advance Wars · Game Boy Advance")
    # Both GBA games share a row: right moves along it, down has no row to go to and stops at
    # the last game, left goes back, and left again leaves for the systems.
    romi.keys("l")
    romi.wait_for(lambda: "Metroid Fusion ·" in caption(romi))
    romi.keys("h")
    romi.wait_for(lambda: "Advance Wars ·" in caption(romi))
    romi.keys("j")
    romi.wait_for(lambda: "Metroid Fusion ·" in caption(romi))
    romi.keys("hh")
    romi.keys("j")
    romi.wait_for(lambda: any(s.startswith("Super Nintendo") for s in romi.selected()))


def test_games_without_art_show_their_system(start, sandbox):
    sandbox.write_state({"grid": True})
    romi = start()
    romi.wait_for("Advance Wars · Game Boy Advance")
    # The box where the cover would be names the system; the name goes underneath.
    assert romi.text().count("Game Boy Advance") >= 3


def test_l3_toggles_the_grid(sandbox, start, pad):
    sandbox.write_config(dict(CONFIG, controller={"enabled": True}))
    romi = start(pad=pad)
    romi.press("THUMBL")
    romi.wait_for("Advance Wars · Game Boy Advance")

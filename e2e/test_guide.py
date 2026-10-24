"""`romi guide`: a command run on the controller's Guide button, outside of games."""

import json
import subprocess
import time

from drive import binary


def eventually(check, timeout=5.0):
    end = time.time() + timeout
    while not check():
        assert time.time() < end, "timed out"
        time.sleep(0.05)


def tap(pad, button):
    pad.down(button)
    time.sleep(0.08)
    pad.up(button)


def presses(path):
    return len(path.read_text().splitlines()) if path.exists() else 0


def test_guide_runs_the_command_outside_games(sandbox, pad):
    out = sandbox.root / "presses.txt"
    guide = subprocess.Popen(
        [binary(), "guide", "--", "sh", "-c", f"echo pressed >> '{out}'"],
        env=sandbox.env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
    )
    try:
        time.sleep(1.0)  # let it find the pad
        tap(pad, "MODE")
        eventually(lambda: presses(out) == 1)
        # Other buttons do nothing, and a second press right away counts as the same one.
        tap(pad, "SOUTH")
        tap(pad, "MODE")
        time.sleep(0.5)
        assert presses(out) == 1

        # While a game romi started is running, Guide belongs to the game.
        game = subprocess.Popen(["sleep", "30"])
        running = sandbox.home / ".local/state/romi/running"
        running.mkdir(parents=True, exist_ok=True)
        record = {"key": "gba:x.gba", "name": "X", "system": "gba", "system_name": "GBA",
                  "pid": game.pid, "started": 0}
        (running / f"{game.pid}.json").write_text(json.dumps(record))
        time.sleep(1.1)
        tap(pad, "MODE")
        time.sleep(0.5)
        assert presses(out) == 1
        game.kill()
        game.wait()
        tap(pad, "MODE")
        eventually(lambda: presses(out) == 2)
    finally:
        guide.kill()
        guide.wait()


def test_guide_needs_a_command(sandbox):
    out = subprocess.run([binary(), "guide"], env=sandbox.env, capture_output=True, text=True, timeout=10)
    assert out.returncode == 64

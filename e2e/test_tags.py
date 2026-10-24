"""Your own tags: set from a game's menu, shown as collections, filtered by."""

import json
import subprocess
import time

from drive import CONFIG, binary

KEY = "gba:Advance Wars (USA).gba"


def run_cli(sandbox, *args):
    return subprocess.run([binary(), *args], env=sandbox.env, capture_output=True, text=True, timeout=10)


def eventually(check, timeout=3.0):
    end = time.time() + timeout
    while not check():
        assert time.time() < end, "timed out"
        time.sleep(0.05)


def test_tag_a_game_with_a_new_tag(start, sandbox):
    romi = start()
    romi.keys("o")
    romi.wait_for("Tags…")
    romi.keys("jj\r")
    romi.wait_for("+ New tag…")
    romi.keys("\r")
    romi.wait_for("New tag")
    romi.keys("Couch co-op\r")
    romi.wait_for("✓ Couch co-op")
    eventually(lambda: sandbox.state().get("tags") == {"Couch co-op": [KEY]})
    # Back to the game's menu, which lists its tags, and the collection in the systems pane.
    romi.keys("\x1b")
    romi.wait_for("Tags: Couch co-op")
    romi.keys("\x1b")
    romi.wait_for("# Couch co-op")

    # Ticking it again untags the game, and the tag goes with its last game.
    romi.keys("ojj\r")
    romi.wait_for("✓ Couch co-op")
    romi.keys("\r")
    eventually(lambda: sandbox.state().get("tags") == {})


def test_typing_an_existing_tag_reuses_it(start, sandbox):
    # The collection would come first; start on the GBA games.
    sandbox.write_state({"tags": {"backlog": ["snes:Chrono Trigger (USA).sfc"]}, "selected_system": "gba"})
    romi = start()
    romi.keys("ojj\r")
    romi.wait_for("backlog")
    romi.keys("j\r")
    romi.wait_for("New tag")
    romi.keys("BACKLOG\r")
    eventually(lambda: sandbox.state()["tags"] == {"backlog": ["snes:Chrono Trigger (USA).sfc", KEY]})


def test_list_by_tag(sandbox):
    sandbox.write_state({"tags": {"backlog": ["snes:Chrono Trigger (USA).sfc", KEY]}})
    out = run_cli(sandbox, "list", "--tag", "Backlog")
    assert [line.split("\t")[1] for line in out.stdout.splitlines()] == ["Advance Wars", "Chrono Trigger"]
    games = {g["key"]: g for g in json.loads(run_cli(sandbox, "list", "--json").stdout)}
    assert games[KEY]["tags"] == ["backlog"] and games["gba:Metroid Fusion (USA).gba"]["tags"] == []
    bad = run_cli(sandbox, "list", "--tag", "nope")
    assert bad.returncode == 64 and "tags: backlog" in bad.stderr


def test_filter_by_tag(start, sandbox):
    sandbox.write_state({"tags": {"backlog": ["snes:Chrono Trigger (USA).sfc"]}})
    romi = start()
    romi.keys("F")
    romi.wait_for("Clear all")
    romi.keys("jjjjl")
    romi.wait_for("backlog (1)")
    romi.keys("\x1b")
    romi.wait_for(lambda: any("Chrono Trigger" in s for s in romi.selected()))
    assert "Advance Wars" not in romi.text()


def test_name_a_tag_with_the_controller(sandbox, start, pad):
    sandbox.write_config(dict(CONFIG, controller={"enabled": True}))
    romi = start(pad=pad)
    romi.press("START")
    romi.wait_for("Advance Wars…")
    romi.press("SOUTH")
    romi.wait_for("Tags…")
    romi.press("DOWN")
    romi.press("DOWN")
    romi.press("SOUTH")
    romi.wait_for("+ New tag…")
    romi.press("SOUTH")
    romi.wait_for("New tag")
    romi.press("SOUTH")  # the keyboard starts on A
    romi.press("UP")  # wraps to the bottom row...
    romi.press("LEFT")  # ...and its last key, ✓
    romi.press("SOUTH")
    eventually(lambda: sandbox.state().get("tags") == {"a": [KEY]})

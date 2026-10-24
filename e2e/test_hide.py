"""Hiding games from the game menu: saved in overrides.jsonc, which the config includes."""

import json
import subprocess

from drive import binary

KEY = "gba:Advance Wars (USA).gba"


def run_cli(sandbox, *args):
    return subprocess.run([binary(), *args], env=sandbox.env, capture_output=True, text=True, timeout=10)


def jsonc(path):
    return json.loads("\n".join(line for line in path.read_text().splitlines() if not line.strip().startswith("//")))


def overrides(sandbox):
    return jsonc(sandbox.config_path.parent / "overrides.jsonc")["overrides"]


def test_hide_and_bring_back(start, sandbox):
    romi = start()
    assert "Advance Wars" in romi.selected()
    romi.keys("o")
    romi.wait_for("Favorite")
    romi.keys("j\r")
    romi.wait_for("Hid Advance Wars")
    assert not any("Advance Wars" in line for line in romi.lines()[:-1])
    assert "Metroid Fusion" in romi.selected()
    assert overrides(sandbox) == {KEY: {"hidden": True}}
    assert "overrides.jsonc" in jsonc(sandbox.config_path)["include"]

    # Start menu → Show hidden games, then the game menu says Unhide.
    romi.keys("m")
    romi.wait_for("Show hidden games (1)")
    for _ in range(20):
        if "Show hidden games (1)" in romi.selected():
            break
        romi.keys("j")
    romi.keys("\r")
    romi.wait_for("Advance Wars")
    romi.keys("k")
    romi.wait_for(lambda: "Advance Wars" in romi.selected())
    romi.keys("o")
    romi.wait_for("Unhide")
    romi.keys("j\r")
    romi.wait_for("Unhid Advance Wars")
    assert overrides(sandbox) == {}


def test_hidden_games_leave_list_but_still_play(sandbox):
    (sandbox.config_path.parent / "overrides.jsonc").write_text(json.dumps({"overrides": {KEY: {"hidden": True}}}))
    config = json.loads(sandbox.config_path.read_text())
    sandbox.write_config(dict(config, include=["overrides.jsonc"]))
    assert KEY not in run_cli(sandbox, "list").stdout
    games = {g["key"]: g for g in json.loads(run_cli(sandbox, "list", "--hidden", "--json").stdout)}
    assert games[KEY]["hidden"] is True
    assert run_cli(sandbox, "play", "advance wars", "--dry-run").returncode == 0


def test_top_level_overrides_change_any_game(sandbox):
    config = json.loads(sandbox.config_path.read_text())
    sandbox.write_config(dict(config, overrides={"game:Hello Script": {"name": "Hi", "command": "echo hi"}}))
    game = json.loads(run_cli(sandbox, "info", "hi", "--json").stdout)
    assert game["key"] == "game:Hello Script"
    out = run_cli(sandbox, "play", "hi", "--dry-run", "--json")
    assert json.loads(out.stdout)["argv"] == ["echo", "hi"]

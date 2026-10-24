"""Picking another emulator for a game from its menu."""

import json
import subprocess

from drive import CONFIG, binary

KEY = "gba:Advance Wars (USA).gba"


def run_cli(sandbox, *args):
    return subprocess.run([binary(), *args], env=sandbox.env, capture_output=True, text=True, timeout=10)


def jsonc(path):
    return json.loads("\n".join(line for line in path.read_text().splitlines() if not line.strip().startswith("//")))


def with_other_emulator(sandbox):
    config = json.loads(json.dumps(CONFIG))
    # No ES-DE, so the only other emulator is the one configured here.
    config["esde_dir"] = str(sandbox.root / "no-esde")
    config["systems"][0]["emulators"] = [{"name": "Other", "command": "echo other {rom}", "mode": "terminal"}]
    sandbox.write_config(config)


def test_info_lists_other_emulators(sandbox):
    with_other_emulator(sandbox)
    out = run_cli(sandbox, "info", "advance wars")
    assert "  emulator   true" in out.stdout
    assert any(line.startswith("  also") and "Other" in line for line in out.stdout.splitlines())


def test_pick_an_emulator_from_the_game_menu(start, sandbox):
    with_other_emulator(sandbox)
    romi = start()
    romi.keys("o")
    romi.wait_for("Emulator: true")
    romi.keys("jjj\r")
    romi.wait_for("Emulator: Other")
    assert jsonc(sandbox.root / "overrides.jsonc")["overrides"][KEY] == {
        "emulator": "Other", "command": "echo other {rom}", "mode": "terminal",
    }
    out = json.loads(run_cli(sandbox, "play", KEY, "--dry-run", "--json").stdout)
    assert out["argv"][:2] == ["echo", "other"] and out["mode"] == "terminal"
    assert json.loads(run_cli(sandbox, "info", KEY, "--json").stdout)["emulator"] == "Other"

    # Around the list and back to the default, which leaves no override.
    for _ in range(10):
        romi.keys("\r")
        if "Emulator: true" in romi.text():
            break
    romi.wait_for("Emulator: true")
    assert jsonc(sandbox.root / "overrides.jsonc")["overrides"] == {}


def test_hand_added_games_have_no_emulator_choice(start, sandbox):
    with_other_emulator(sandbox)
    romi = start()
    romi.keys("\t\t")
    romi.wait_for(lambda: "Hello Script" in romi.selected())
    romi.keys("o")
    romi.wait_for("Favorite")
    assert "Emulator" not in romi.text()

import json
import subprocess
import time

from drive import CONFIG, Sandbox, binary


def eventually(check, timeout=3.0):
    end = time.time() + timeout
    while not check():
        assert time.time() < end, "timed out"
        time.sleep(0.05)


def test_lists_systems_and_cleans_names(start):
    romi = start()
    text = romi.text()
    for name in ["Game Boy Advance", "Super Nintendo", "PC", "Advance Wars", "Metroid Fusion"]:
        assert name in text
    assert "(USA)" not in text
    assert "Advance Wars" in romi.selected()


def test_moving_and_switching_systems(start):
    romi = start()
    romi.keys("j")
    romi.wait_for(lambda: "Metroid Fusion" in romi.selected())
    romi.keys("\t")
    romi.wait_for(lambda: "Chrono Trigger" in romi.selected())
    assert "Super Nintendo" in romi.text()


def test_search_spans_systems(start):
    romi = start()
    romi.keys("/metroid")
    romi.wait_for("Super Metroid")
    assert "Metroid Fusion" in romi.text()
    assert "Advance Wars" not in romi.text()


def test_launch_records_play(start, sandbox):
    romi = start()
    romi.keys("\r")
    romi.wait_for("Played Advance Wars")
    key = "gba:Advance Wars (USA).gba"
    eventually(lambda: sandbox.state().get("recent", [None])[0] == key)
    assert key in sandbox.state()["last_played"]


def test_terminal_mode_hands_over_the_terminal(start, sandbox):
    romi = start()
    romi.keys("\t\t")
    romi.wait_for(lambda: "Hello Script" in romi.selected())
    romi.keys("\r")
    romi.wait_for("hello from a script")
    romi.wait_for("exited 0")
    romi.keys(" ")
    romi.wait_for("Systems")
    eventually(lambda: sandbox.state().get("recent", [None])[0] == "game:Hello Script")


def test_favorite_is_saved(start, sandbox):
    romi = start()
    romi.keys("f")
    eventually(lambda: "gba:Advance Wars (USA).gba" in sandbox.state().get("favorites", []))


def test_first_key_works_when_terminal_ignores_image_query(start):
    romi = start(answer_queries=False)
    romi.keys("j")
    romi.wait_for(lambda: "Metroid Fusion" in romi.selected(), timeout=2)


def test_controller_moves_and_launches(sandbox, start, pad):
    sandbox.write_config(dict(CONFIG, controller={"enabled": True}))
    romi = start(pad=pad)
    romi.press("DOWN")
    romi.wait_for(lambda: "Metroid Fusion" in romi.selected())
    romi.press("SOUTH")
    romi.wait_for("Played Metroid Fusion")


def run_cli(sandbox, *args):
    return subprocess.run([binary(), *args], env=sandbox.env, capture_output=True, text=True, timeout=10)


def test_list_prints_tsv(sandbox):
    out = run_cli(sandbox, "--list")
    assert out.returncode == 0
    rows = [line.split("\t") for line in out.stdout.splitlines()]
    assert ["Game Boy Advance", "Advance Wars", "gba:Advance Wars (USA).gba"] in rows
    assert len(rows) == 5


def test_check_flags_missing_emulator(tmp_path):
    config = json.loads(json.dumps(CONFIG))
    config["systems"].append(
        {"id": "nes", "name": "NES", "extensions": ["nes"], "command": "definitely-not-an-emulator {rom}"}
    )
    sandbox = Sandbox(tmp_path, config=config, roms={"nes": ["Zelda (USA).nes"]})
    out = run_cli(sandbox, "--check")
    assert out.returncode == 0
    nes = next(line for line in out.stdout.splitlines() if "NES" in line)
    assert "!" in nes and "definitely-not-an-emulator not found" in nes

"""`romi status`, detached games, and shell completions."""

import json
import os
import subprocess
import time

from drive import CONFIG, binary
from test_play import alive, children, eventually, run_cli


def with_games(sandbox, *games):
    config = json.loads(json.dumps(CONFIG))
    config["games"] += [{"name": name, "command": command, "mode": mode} for name, command, mode in games]
    sandbox.write_config(config)


def status(sandbox):
    out = run_cli(sandbox, "status", "--json")
    assert out.returncode == 0, out.stderr
    return json.loads(out.stdout)


def running_dir(sandbox):
    return sandbox.home / ".local/state/romi/running"


def test_nothing_running(sandbox):
    assert status(sandbox) == []
    assert run_cli(sandbox, "status").stdout == "nothing running\n"


def test_detached_play_returns_and_is_tracked(sandbox):
    with_games(sandbox, ("Sleeper", "sleep 1.5", "detach"))
    started = time.time()
    out = run_cli(sandbox, "play", "sleeper", "--json")
    assert out.returncode == 0, out.stderr
    assert time.time() - started < 1
    assert json.loads(out.stdout)["mode"] == "detach"
    eventually(lambda: status(sandbox))
    [game] = status(sandbox)
    assert game["key"] == "game:Sleeper" and game["system"] == "pc" and game["system_name"] == "PC"
    assert alive(game["pid"])
    assert "Sleeper  (PC, " in run_cli(sandbox, "status").stdout
    eventually(lambda: "game:Sleeper" in sandbox.state().get("playtime", {}))
    assert sandbox.state()["playtime"]["game:Sleeper"] >= 1
    assert status(sandbox) == []
    assert list(running_dir(sandbox).iterdir()) == []


def test_detached_play_checks_the_program_first(sandbox):
    with_games(sandbox, ("Missing", "definitely-not-an-emulator", "detach"))
    out = run_cli(sandbox, "play", "missing")
    assert out.returncode == 69


def test_status_lists_a_waiting_play(sandbox):
    with_games(sandbox, ("Sleeper", "sleep 30", "wait"))
    romi = subprocess.Popen([binary(), "play", "sleeper"], env=sandbox.env)
    try:
        eventually(lambda: status(sandbox))
        [game] = status(sandbox)
        assert game["pid"] == children(romi.pid)[0]
    finally:
        romi.terminate()
        romi.wait(timeout=5)
    assert status(sandbox) == []


def test_status_drops_games_that_are_gone(sandbox):
    dead = subprocess.Popen(["true"])
    dead.wait()
    running_dir(sandbox).mkdir(parents=True)
    record = {"key": "gba:Advance Wars (USA).gba", "name": "Advance Wars", "system": "gba",
              "system_name": "Game Boy Advance", "started": 1}
    (running_dir(sandbox) / f"{dead.pid}.json").write_text(json.dumps(dict(record, pid=dead.pid, proc_start=None)))
    # A live process, but not the one that was recorded: its pid was reused.
    (running_dir(sandbox) / f"{os.getpid()}.json").write_text(json.dumps(dict(record, pid=os.getpid(), proc_start=1)))
    assert status(sandbox) == []
    assert list(running_dir(sandbox).iterdir()) == []


def test_tui_wait_game_shows_in_status(start, sandbox):
    with_games(sandbox, ("Sleeper", "sleep 30", "wait"))
    romi = start()
    romi.keys("\t\tj")
    romi.wait_for(lambda: "Sleeper" in romi.selected())
    romi.keys("\r")
    romi.wait_for("Now playing")
    eventually(lambda: [g["key"] for g in status(sandbox)] == ["game:Sleeper"])


def test_tui_detached_game_is_recorded_after_tui_quits(start, sandbox):
    with_games(sandbox, ("Sleeper", "sleep 1.5", "detach"))
    romi = start()
    romi.keys("\t\tj")
    romi.wait_for(lambda: "Sleeper" in romi.selected())
    romi.keys("\r")
    romi.wait_for("Started Sleeper")
    romi.close()
    assert [g["key"] for g in status(sandbox)] == ["game:Sleeper"]
    eventually(lambda: "game:Sleeper" in sandbox.state().get("playtime", {}))
    assert status(sandbox) == []


def complete(sandbox, *words):
    env = dict(sandbox.env, COMPLETE="fish")
    out = subprocess.run([binary(), "--", "romi", *words], env=env, capture_output=True, text=True, timeout=10)
    assert out.returncode == 0, out.stderr
    return [line.split("\t") for line in out.stdout.splitlines()]


def test_completes_game_names(sandbox):
    assert complete(sandbox, "play", "chr") == [["Chrono Trigger", "Super Nintendo"]]
    names = [c[0] for c in complete(sandbox, "info", "SUPER")]
    assert names == ["Super Metroid"]


def test_completes_systems_and_genres(sandbox):
    assert complete(sandbox, "list", "--system", "s") == [["snes", "Super Nintendo"]]
    assert complete(sandbox, "list", "--genre", "R") == [["RPG", "1 game"]]


def test_completes_commands(sandbox):
    assert "status" in [c[0] for c in complete(sandbox, "st")]


def test_completion_never_writes_a_config(sandbox):
    env = dict(sandbox.env, COMPLETE="fish", ROMI_CONFIG=str(sandbox.root / "missing.json"))
    out = subprocess.run([binary(), "--", "romi", "play", "chr"], env=env, capture_output=True, text=True)
    assert out.returncode == 0 and out.stdout == ""
    assert not (sandbox.root / "missing.json").exists()

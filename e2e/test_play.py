"""`romi play`: finding the game, running it in each mode, exit codes, and stopping it."""

import json
import os
import signal
import subprocess
import time

import pytest

from drive import CONFIG, binary


def run_cli(sandbox, *args, timeout=10):
    return subprocess.run([binary(), *args], env=sandbox.env, capture_output=True, text=True, timeout=timeout)


def with_games(sandbox, *games):
    config = json.loads(json.dumps(CONFIG))
    config["games"] += [{"name": name, "command": command} for name, command in games]
    sandbox.write_config(config)


def eventually(check, timeout=5.0):
    end = time.time() + timeout
    while not check():
        assert time.time() < end, "timed out"
        time.sleep(0.05)


def children(pid):
    path = f"/proc/{pid}/task/{pid}/children"
    return [int(p) for p in open(path).read().split()] if os.path.exists(path) else []


def alive(pid):
    try:
        with open(f"/proc/{pid}/stat") as f:
            return f.read().split(") ")[1][0] != "Z"
    except FileNotFoundError:
        return False


def test_dry_run_prints_the_command(sandbox):
    out = run_cli(sandbox, "play", "advance wars", "--dry-run")
    assert out.returncode == 0
    rom = sandbox.home / "ROMs/gba/Advance Wars (USA).gba"
    assert out.stdout.splitlines() == ["mode     wait", f"command  true '{rom}'"]
    assert sandbox.state() == {}


def test_dry_run_json(sandbox):
    out = run_cli(sandbox, "play", "hello", "--dry-run", "--json")
    command = json.loads(out.stdout)
    assert command["argv"] == ["sh", "-c", "echo hello from a script"]
    assert command["mode"] == "terminal" and command["cwd"] is None and command["env"] == {}


def test_play_records_the_session(sandbox):
    out = run_cli(sandbox, "play", "gba:Advance Wars (USA).gba", "--json")
    assert out.returncode == 0, out.stderr
    outcome = json.loads(out.stdout)
    assert outcome["exit_code"] == 0 and outcome["mode"] == "wait"
    assert outcome["log"].endswith("/romi/last.log")
    state = sandbox.state()
    assert state["recent"][0] == "gba:Advance Wars (USA).gba"
    assert "gba:Advance Wars (USA).gba" in state["playtime"]


def test_terminal_mode_output_comes_through(sandbox):
    out = run_cli(sandbox, "play", "hello")
    assert out.returncode == 0
    assert out.stdout == "hello from a script\n"


def test_wait_mode_output_goes_to_the_log(sandbox):
    out = run_cli(sandbox, "play", "hello", "--mode", "wait")
    assert out.returncode == 0 and out.stdout == ""
    assert (sandbox.home / ".local/state/romi/last.log").read_text() == "hello from a script\n"


def test_game_exit_code_passes_through(sandbox):
    with_games(sandbox, ("Exit Three", "sh -c 'exit 3'"))
    out = run_cli(sandbox, "play", "exit three")
    assert out.returncode == 3
    assert "Exit Three exited with code 3" in out.stderr
    assert "game:Exit Three" in sandbox.state()["playtime"]


@pytest.mark.parametrize(
    "game, command, code",
    [
        ("Missing Emulator", "definitely-not-an-emulator --fullscreen", 69),
        ("Bad Placeholder", "true {nope}", 78),
    ],
)
def test_romi_failures_use_their_own_codes(sandbox, game, command, code):
    with_games(sandbox, (game, command))
    out = run_cli(sandbox, "play", game)
    assert out.returncode == code, out.stderr
    assert game in out.stderr
    assert sandbox.state() == {}


@pytest.mark.parametrize("query, code", [("metroid", 65), ("zelda", 66)])
def test_play_needs_exactly_one_game(sandbox, query, code):
    assert run_cli(sandbox, "play", query).returncode == code


def start_sleeper(sandbox, **popen):
    with_games(sandbox, ("Sleeper", "sleep 30"))
    romi = subprocess.Popen([binary(), "play", "sleeper"], env=sandbox.env, **popen)
    eventually(lambda: children(romi.pid))
    game = children(romi.pid)[0]
    return romi, game


def test_signal_to_romi_alone_is_passed_on(sandbox):
    romi, game = start_sleeper(sandbox)
    time.sleep(1.1)
    romi.send_signal(signal.SIGTERM)
    assert romi.wait(timeout=5) == 128 + signal.SIGTERM
    assert not alive(game)
    assert sandbox.state()["playtime"]["game:Sleeper"] >= 1


def test_ctrl_c_stops_the_game_and_romi(sandbox):
    romi, game = start_sleeper(sandbox, start_new_session=True)
    started = time.time()
    os.killpg(romi.pid, signal.SIGINT)  # what ^C does: the whole foreground process group
    assert romi.wait(timeout=5) == 128 + signal.SIGINT
    assert time.time() - started < 0.9, "romi should not have needed to pass the signal on"
    assert not alive(game)
    assert "game:Sleeper" in sandbox.state()["playtime"]

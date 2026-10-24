"""on_launch and on_exit hooks, global and per system, from `romi play` and the TUI."""

import json
import subprocess
import time

from drive import CONFIG, binary


def run_cli(sandbox, *args):
    return subprocess.run([binary(), *args], env=sandbox.env, capture_output=True, text=True, timeout=20)


def eventually(check, timeout=5.0):
    end = time.time() + timeout
    while not check():
        assert time.time() < end, "timed out"
        time.sleep(0.05)


def with_hooks(sandbox, hooks, gba_hooks=None):
    config = json.loads(json.dumps(CONFIG))
    config["vars"] = {"out": str(sandbox.root / "hooks.txt")}
    config["hooks"] = hooks
    if gba_hooks:
        config["systems"][0]["hooks"] = gba_hooks
    sandbox.write_config(config)
    return sandbox.root / "hooks.txt"


def lines(path):
    return path.read_text().splitlines() if path.exists() else []


def test_hooks_run_in_order_with_placeholders(sandbox):
    out = with_hooks(
        sandbox,
        {"on_launch": "sh -c 'echo launch {game} $ROMI_SYSTEM >> {out}'",
         "on_exit": ["sh -c 'echo exit {exit_code} $ROMI_EVENT >> {out}'"]},
        {"on_launch": "sh -c 'echo gba-launch \"{file}\" >> {out}'", "on_exit": "sh -c 'echo gba-exit >> {out}'"},
    )
    result = run_cli(sandbox, "play", "advance wars")
    assert result.returncode == 0, result.stderr
    # The system's hooks run inside the global ones.
    assert lines(out) == [
        "launch Advance Wars gba",
        "gba-launch Advance Wars (USA).gba",
        "gba-exit",
        "exit 0 exit",
    ]


def test_global_hooks_work_for_games_without_a_rom(sandbox):
    out = with_hooks(sandbox, {"on_launch": "sh -c 'echo [{rom}] {key} >> {out}'"})
    assert run_cli(sandbox, "play", "hello").returncode == 0
    assert lines(out) == ["[] game:Hello Script"]


def test_a_failing_hook_does_not_stop_the_game(sandbox):
    with_hooks(sandbox, {"on_launch": ["false", "definitely-not-a-program"]})
    result = run_cli(sandbox, "play", "advance wars", "--json")
    assert result.returncode == 0
    assert "on_launch `false` failed" in result.stderr and "(and 1 more)" in result.stderr
    assert json.loads(result.stdout)["exit_code"] == 0
    log = (sandbox.home / ".local/state/romi/hooks.log").read_text()
    assert "$ false" in log and "definitely-not-a-program" in log


def test_dry_run_shows_hooks(sandbox):
    with_hooks(sandbox, {"on_launch": "notify-send {game}", "on_exit": "echo {exit_code}"})
    out = run_cli(sandbox, "play", "advance wars", "--dry-run")
    assert "on_launch notify-send 'Advance Wars'" in out.stdout.splitlines()
    assert "on_exit   echo 0" in out.stdout.splitlines()
    hooks = json.loads(run_cli(sandbox, "play", "advance wars", "--dry-run", "--json").stdout)["hooks"]
    assert hooks == {"on_launch": [["notify-send", "Advance Wars"]], "on_exit": [["echo", "0"]]}
    assert not (sandbox.root / "hooks.txt").exists()


def test_tui_runs_hooks(start, sandbox):
    out = with_hooks(sandbox, {"on_launch": "sh -c 'echo launch >> {out}'", "on_exit": "sh -c 'echo exit >> {out}'"})
    romi = start()
    romi.keys("\r")
    romi.wait_for("Played Advance Wars")
    eventually(lambda: lines(out) == ["launch", "exit"])


def test_detached_games_run_hooks_in_the_background(sandbox):
    out = with_hooks(sandbox, {"on_launch": "sh -c 'echo launch >> {out}'", "on_exit": "sh -c 'echo exit >> {out}'"})
    assert run_cli(sandbox, "play", "advance wars", "--mode", "detach").returncode == 0
    eventually(lambda: lines(out) == ["launch", "exit"])

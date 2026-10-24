"""Play statistics: `romi stats` and the stats screen, from state.json and the session history."""

import json
import subprocess
import time

from drive import binary

AW, CT = "gba:Advance Wars (USA).gba", "snes:Chrono Trigger (USA).sfc"


def run_cli(sandbox, *args):
    return subprocess.run([binary(), *args], env=sandbox.env, capture_output=True, text=True, timeout=10)


def with_history(sandbox):
    sandbox.write_state({"playtime": {AW: 7200, CT: 1800, "gba:Gone (USA).gba": 60}})
    now = int(time.time())
    lines = [{"key": CT, "at": now - 60, "secs": 1800}, {"key": AW, "at": now - 30, "secs": 600}]
    path = sandbox.home / ".local/state/romi/history.jsonl"
    path.write_text("".join(json.dumps(line) + "\n" for line in lines))


def test_stats_text_and_json(sandbox):
    with_history(sandbox)
    out = run_cli(sandbox, "stats").stdout
    assert out.splitlines()[0] == "Played 2h 31m across 3 games"
    assert "Game Boy Advance" in out and "Super Nintendo" in out
    # A game that's no longer in the library still counts, under its key.
    assert "Gone (USA).gba" in out
    stats = json.loads(run_cli(sandbox, "stats", "--json").stdout)
    assert stats["systems"][0] == ["Game Boy Advance", 7260]
    assert stats["top"][0] == ["Advance Wars", 7200]
    assert stats["year"]["sessions"] == 2 and sum(stats["year"]["months"]) == 2400
    assert stats["year"]["top"][0] == ["Chrono Trigger", 1800]


def test_nothing_played_yet(sandbox):
    assert run_cli(sandbox, "stats").stdout.strip() == "Nothing played from romi yet."


def test_sessions_are_recorded(sandbox):
    assert run_cli(sandbox, "play", "advance wars").returncode == 0
    history = (sandbox.home / ".local/state/romi/history.jsonl").read_text().splitlines()
    assert len(history) == 1 and json.loads(history[0])["key"] == AW


def test_stats_screen(start, sandbox):
    with_history(sandbox)
    romi = start()
    romi.keys("S")
    romi.wait_for("2h 31m played across 3 games")
    assert "Most played" in romi.text() and "40m in 2 sessions · most: Chrono Trigger (30m)" in romi.text()
    romi.keys("\x1b")
    romi.wait_for(lambda: "Most played" not in romi.text())

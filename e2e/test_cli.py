"""The command-line side of romi: subcommands, the old flags, exit codes."""

import json
import subprocess

import pytest

from drive import CONFIG, Sandbox, binary


def run_cli(sandbox, *args):
    return subprocess.run([binary(), *args], env=sandbox.env, capture_output=True, text=True, timeout=10)


@pytest.mark.parametrize("args", [["list"], ["--list"]])
def test_list_prints_tsv(sandbox, args):
    out = run_cli(sandbox, *args)
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
    for args in (["check"], ["--check"]):
        out = run_cli(sandbox, *args)
        assert out.returncode == 0
    nes = next(line for line in out.stdout.splitlines() if "NES" in line)
    assert "!" in nes and "definitely-not-an-emulator not found" in nes


@pytest.mark.parametrize("args", [["scrape", "gba"], ["--scrape", "gba"]])
def test_scrape_reports_missing_art(sandbox, args):
    # The sandbox config has "online": false, so this only looks on disk.
    out = run_cli(sandbox, *args)
    assert out.returncode == 0
    assert "Game Boy Advance: Metroid Fusion" in out.stdout
    assert "Super Nintendo" not in out.stdout


@pytest.mark.parametrize("before", [True, False])
def test_config_option_goes_before_or_after_the_command(sandbox, before):
    env = dict(sandbox.env)
    path = env.pop("ROMI_CONFIG")
    args = ["--config", path, "list"] if before else ["list", "--config", path]
    out = subprocess.run([binary(), *args], env=env, capture_output=True, text=True, timeout=10)
    assert out.returncode == 0 and "Advance Wars" in out.stdout


@pytest.mark.parametrize("args", [["nope"], ["list", "--nope"], ["--check", "list"], ["--check", "--list"]])
def test_bad_arguments_exit_64(sandbox, args):
    out = run_cli(sandbox, *args)
    assert out.returncode == 64
    assert out.stderr.startswith("error:")


def test_help_and_version_exit_0(sandbox):
    help = run_cli(sandbox, "--help")
    assert help.returncode == 0 and "scrape" in help.stdout and "controller:" in help.stdout
    assert run_cli(sandbox, "-V").stdout.startswith("romi ")


def lines(out):
    return [line.split("\t")[1] for line in out.stdout.splitlines()]


def test_list_is_in_tui_order(sandbox):
    out = run_cli(sandbox, "list")
    assert lines(out) == ["Advance Wars", "Metroid Fusion", "Chrono Trigger", "Super Metroid", "Hello Script"]


def test_list_json(sandbox):
    sandbox.write_state({"favorites": ["snes:Chrono Trigger (USA).sfc"], "playtime": {"snes:Chrono Trigger (USA).sfc": 90},
                         "last_played": {"snes:Chrono Trigger (USA).sfc": 1700000000}})
    out = run_cli(sandbox, "list", "--json")
    assert out.returncode == 0
    games = {g["key"]: g for g in json.loads(out.stdout)}
    assert len(games) == 5
    chrono = games["snes:Chrono Trigger (USA).sfc"]
    assert chrono["name"] == "Chrono Trigger"
    assert chrono["system"] == "snes" and chrono["system_name"] == "Super Nintendo"
    assert chrono["path"].endswith("/ROMs/snes/Chrono Trigger (USA).sfc")
    assert chrono["favorite"] is True
    assert chrono["playtime_secs"] == 90 and chrono["last_played"] == 1700000000
    assert chrono["art"] is None
    assert chrono["meta"]["genres"] == ["RPG"] and chrono["meta"]["year"] == 1995
    script = games["game:Hello Script"]
    assert script["path"] is None and script["favorite"] is False and script["last_played"] is None


@pytest.mark.parametrize(
    "args, expected",
    [
        (["--system", "snes"], ["Chrono Trigger", "Super Metroid"]),
        (["--system", "super nintendo"], ["Chrono Trigger", "Super Metroid"]),
        (["--system", "pc", "--system", "gba"], ["Advance Wars", "Metroid Fusion", "Hello Script"]),
        (["--genre", "rpg"], ["Chrono Trigger"]),
        (["--decade", "90s"], ["Chrono Trigger", "Super Metroid"]),
        (["--decade", "2000"], ["Advance Wars"]),
        (["--players", "2"], ["Advance Wars"]),
        (["--system", "snes", "--genre", "Action"], ["Super Metroid"]),
        (["--favorites"], ["Metroid Fusion"]),
        (["--played"], ["Super Metroid"]),
        (["--unplayed", "--system", "snes"], ["Chrono Trigger"]),
    ],
)
def test_list_filters(sandbox, args, expected):
    sandbox.write_state({"favorites": ["gba:Metroid Fusion (USA).gba"],
                         "last_played": {"snes:Super Metroid (Japan, USA).sfc": 1700000000}})
    out = run_cli(sandbox, "list", *args)
    assert out.returncode == 0, out.stderr
    assert lines(out) == expected


@pytest.mark.parametrize(
    "args, hint",
    [(["--system", "n64"], "gba, snes"), (["--genre", "Racing"], "Strategy"), (["--decade", "1995"], "1990s")],
)
def test_list_rejects_unknown_filter_values(sandbox, args, hint):
    out = run_cli(sandbox, "list", *args)
    assert out.returncode == 64
    assert hint in out.stderr


@pytest.mark.parametrize(
    "query, key",
    [
        ("snes:Super Metroid (Japan, USA).sfc", "snes:Super Metroid (Japan, USA).sfc"),
        ("CHRONO TRIGGER", "snes:Chrono Trigger (USA).sfc"),
        ("fusion metroid", "gba:Metroid Fusion (USA).gba"),
        ("hello", "game:Hello Script"),
    ],
)
def test_info_finds_games(sandbox, query, key):
    out = run_cli(sandbox, "info", query, "--json")
    assert out.returncode == 0, out.stderr
    assert json.loads(out.stdout)["key"] == key


def test_info_text(sandbox):
    out = run_cli(sandbox, "info", "advance wars")
    assert out.returncode == 0
    assert out.stdout.splitlines()[0] == "Advance Wars"
    assert "  genre      Strategy" in out.stdout
    assert "  command    true {rom}" in out.stdout


def test_info_ambiguous_lists_candidates(sandbox):
    out = run_cli(sandbox, "info", "metroid")
    assert out.returncode == 65
    assert "gba:Metroid Fusion (USA).gba" in out.stderr and "snes:Super Metroid (Japan, USA).sfc" in out.stderr
    narrowed = run_cli(sandbox, "info", "metroid", "--system", "snes", "--json")
    assert json.loads(narrowed.stdout)["name"] == "Super Metroid"


def test_info_not_found(sandbox):
    out = run_cli(sandbox, "info", "zelda")
    assert out.returncode == 66
    assert "zelda" in out.stderr

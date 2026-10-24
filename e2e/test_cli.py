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

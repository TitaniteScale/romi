"""Multi-disc games: one entry per game, launched through a generated .m3u."""

import json
import subprocess

from drive import Sandbox, binary

PSX = {"id": "psx", "name": "PlayStation", "extensions": ["chd", "m3u"], "command": "true {rom}"}
DISCS = ["Mr. Bones (USA) (Disc 2).chd", "Mr. Bones (USA) (Disc 1).chd", "Crash Bandicoot (USA).chd"]
KEY = "psx:Mr. Bones (USA).m3u"
DISC_1 = "psx:Mr. Bones (USA) (Disc 1).chd"
DISC_2 = "psx:Mr. Bones (USA) (Disc 2).chd"


def psx_sandbox(tmp_path, system=PSX, files=DISCS):
    config = {"roms_dir": "~/ROMs", "art": {"online": False}, "controller": {"enabled": False},
              "steam": {"enabled": False}, "systems": [system]}
    return Sandbox(tmp_path, config=config, roms={"psx": files})


def run_cli(sandbox, *args):
    return subprocess.run([binary(), *args], env=sandbox.env, capture_output=True, text=True, timeout=10)


def keys(sandbox):
    return sorted(line.split("\t")[2] for line in run_cli(sandbox, "list").stdout.splitlines())


def test_discs_become_one_game(tmp_path):
    sandbox = psx_sandbox(tmp_path)
    assert keys(sandbox) == ["psx:Crash Bandicoot (USA).chd", KEY]
    game = json.loads(run_cli(sandbox, "info", "mr bones", "--json").stdout)
    roms = sandbox.home / "ROMs/psx"
    assert game["name"] == "Mr. Bones"
    assert game["path"] == str(roms / "Mr. Bones (USA) (Disc 1).chd")
    assert game["discs"] == [str(roms / "Mr. Bones (USA) (Disc 1).chd"), str(roms / "Mr. Bones (USA) (Disc 2).chd")]


def test_the_playlist_is_launched_from_the_cache(tmp_path):
    sandbox = psx_sandbox(tmp_path)
    out = run_cli(sandbox, "play", "mr bones", "--dry-run", "--json")
    m3u = sandbox.home / ".cache/romi/m3u/psx/Mr. Bones (USA).m3u"
    assert json.loads(out.stdout)["argv"] == ["true", str(m3u)]
    roms = sandbox.home / "ROMs/psx"
    assert m3u.read_text() == f"{roms}/Mr. Bones (USA) (Disc 1).chd\n{roms}/Mr. Bones (USA) (Disc 2).chd\n"
    assert not (roms / "Mr. Bones (USA).m3u").exists()


def test_disc_history_moves_to_the_merged_game(tmp_path):
    sandbox = psx_sandbox(tmp_path)
    sandbox.write_state({"favorites": [DISC_1], "recent": [DISC_2, "psx:Crash Bandicoot (USA).chd", DISC_1],
                         "playtime": {DISC_1: 100, DISC_2: 50}, "last_played": {DISC_1: 1000, DISC_2: 2000}})
    game = json.loads(run_cli(sandbox, "info", "mr bones", "--json").stdout)
    assert game["favorite"] is True and game["playtime_secs"] == 150 and game["last_played"] == 2000
    state = sandbox.state()
    assert state["recent"] == [KEY, "psx:Crash Bandicoot (USA).chd"]
    assert DISC_1 not in state["playtime"] and DISC_2 not in state["last_played"]


def test_old_disc_keys_still_find_the_game(tmp_path):
    sandbox = psx_sandbox(tmp_path)
    out = run_cli(sandbox, "info", DISC_2, "--json")
    assert out.returncode == 0 and json.loads(out.stdout)["key"] == KEY


def test_a_playlist_in_the_folder_keeps_its_discs(tmp_path):
    sandbox = psx_sandbox(tmp_path)
    (sandbox.home / "ROMs/psx/Mr. Bones.m3u").write_text(
        "Mr. Bones (USA) (Disc 1).chd\nMr. Bones (USA) (Disc 2).chd\n"
    )
    assert keys(sandbox) == ["psx:Crash Bandicoot (USA).chd", "psx:Mr. Bones.m3u"]
    assert not (sandbox.home / ".cache/romi/m3u").exists()


def test_systems_without_m3u_keep_discs_apart(tmp_path):
    sandbox = psx_sandbox(tmp_path, system={**PSX, "extensions": ["chd"]})
    assert keys(sandbox) == ["psx:Crash Bandicoot (USA).chd", DISC_1, DISC_2]

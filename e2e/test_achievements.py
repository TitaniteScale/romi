"""RetroAchievements, from cached data only (no network in tests): matching, the details pane, setup."""

import json
import os
import subprocess

from drive import CONFIG, binary

AW = "gba:Advance Wars (USA).gba"
EMPTY_MD5 = "d41d8cd98f00b204e9800998ecf8427e"  # the sandbox's ROMs are empty files


def run_cli(sandbox, *args):
    return subprocess.run([binary(), *args], env=sandbox.env, capture_output=True, text=True, timeout=10)


def with_ra(sandbox, rom_md5=None):
    config = json.loads(json.dumps(CONFIG))
    config["retroachievements"] = {"username": "someone"}
    sandbox.write_config(config)
    ra = sandbox.home / ".cache/romi/ra"
    ra.mkdir(parents=True)
    listed = [{"Title": "Advance Wars", "ID": 506, "NumAchievements": 92, "Points": 800, "Hashes": [EMPTY_MD5]},
              {"Title": "~Hack~ Advance Wars Plus", "ID": 9, "NumAchievements": 5, "Points": 50, "Hashes": []}]
    (ra / "list-5.json").write_text(json.dumps(listed))
    progress = [{"GameID": 506, "NumAwarded": 3, "NumAwardedHardcore": 2, "HighestAwardKind": None}]
    (ra / "progress.json").write_text(json.dumps(progress))
    if rom_md5:
        rom = sandbox.home / "ROMs/gba/Advance Wars (USA).gba"
        st = os.stat(rom)
        (ra / "hashes.tsv").write_text(f"{rom}\t{st.st_size}\t{int(st.st_mtime)}\t{rom_md5}")


def achievements(sandbox, key=AW):
    return json.loads(run_cli(sandbox, "info", key, "--json").stdout)["achievements"]


def test_matched_by_hash(sandbox):
    with_ra(sandbox, rom_md5=EMPTY_MD5)
    a = achievements(sandbox)
    assert a["id"] == 506 and a["supported"] is True
    assert (a["earned"], a["earned_hardcore"], a["total"], a["points"]) == (3, 2, 92, 800)
    assert "  cheevos    3/92 achievements · 800 points" in run_cli(sandbox, "info", AW).stdout
    assert achievements(sandbox, "gba:Metroid Fusion (USA).gba") is None


def test_another_dump_is_told_apart(sandbox):
    with_ra(sandbox, rom_md5="0" * 32)
    a = achievements(sandbox)
    assert a["supported"] is False
    assert "for another version of this ROM (retroachievements.org/game/506/hashes)" in run_cli(sandbox, "info", AW).stdout


def test_by_title_before_hashing(sandbox):
    # Until a refresh has hashed the ROM, the title stands in.
    with_ra(sandbox)
    assert achievements(sandbox)["supported"] is True


def test_not_set_up(sandbox):
    out = run_cli(sandbox, "achievements")
    assert out.returncode == 78 and "retroachievements" in out.stderr and "RA_API_KEY" in out.stderr
    assert achievements(sandbox) is None


def test_details_pane(start, sandbox):
    with_ra(sandbox, rom_md5=EMPTY_MD5)
    romi = start(cols=140)  # wide enough for the details pane
    romi.wait_for("3/92 achievements · 800 points")


def with_screen_data(sandbox):
    with_ra(sandbox, rom_md5=EMPTY_MD5)
    ra = sandbox.home / ".cache/romi/ra"
    (ra / "list-3.json").write_text(json.dumps(
        [{"Title": "Super Metroid", "ID": 236, "NumAchievements": 69, "Points": 700, "Hashes": []}]))
    (ra / "progress.json").write_text(json.dumps([
        {"GameID": 506, "NumAwarded": 3, "NumAwardedHardcore": 2, "HighestAwardKind": None},
        {"GameID": 236, "NumAwarded": 69, "NumAwardedHardcore": 69, "HighestAwardKind": "mastered"}]))
    (ra / "game-506.json").write_text(json.dumps([
        {"title": "First Strike", "description": "Win the first battle", "points": 5, "kind": "progression",
         "earned": "2026-10-20 18:02:11", "earned_hardcore": None, "rarity": 0.8},
        {"title": "Perfect Campaign", "description": "S-rank every mission", "points": 50, "kind": None,
         "earned": None, "earned_hardcore": None, "rarity": 0.02}]))


def list_rows(romi):
    return [line for line in romi.lines() if "pts" in line]


def test_screen_filters_and_opens_a_game(start, sandbox):
    with_screen_data(sandbox)
    romi = start()
    romi.keys("A")
    romi.wait_for("‹ all systems ›   showing all")
    assert len(list_rows(romi)) == 2
    romi.keys("\t\t\t")  # in progress → not started → finished
    romi.wait_for("showing finished")
    assert len(list_rows(romi)) == 1 and "mastered" in list_rows(romi)[0]
    romi.keys("\t\t")  # need another dump → all
    romi.keys("l")
    romi.wait_for("‹ Game Boy Advance ›")
    assert len(list_rows(romi)) == 1
    romi.keys("\r")
    romi.wait_for("✓ First Strike")
    assert "earned 2026-10-20" in romi.text() and "· Perfect Campaign  50 points · 2% of players" in romi.text()
    romi.keys("\x1b")
    romi.wait_for("‹ Game Boy Advance ›")
    romi.keys("\x1b")
    romi.wait_for(lambda: "Achievements" not in romi.text())


def test_search_by_name(start, sandbox):
    with_screen_data(sandbox)
    romi = start()
    romi.keys("A")
    romi.wait_for("showing all")
    romi.keys("/metroid\r")
    romi.wait_for("1 game")
    assert "Super Metroid" in list_rows(romi)[0]


def test_from_the_game_menu(start, sandbox):
    with_screen_data(sandbox)
    romi = start()
    romi.keys("o")
    romi.wait_for("Achievements…")
    for _ in range(6):
        if "Achievements…" in romi.selected():
            break
        romi.keys("j")
    romi.keys("\r")
    romi.wait_for("✓ First Strike")
    # Opened on one game, going back closes the screen.
    romi.keys("\x1b")
    romi.wait_for(lambda: "First Strike" not in romi.text() and "Systems" in romi.text())

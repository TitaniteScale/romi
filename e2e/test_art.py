"""Adding a game's cover from its menu: drop an image on the terminal, which pastes its path."""

import json
import subprocess

import pytest

from drive import CONFIG, binary
from fake_ra import png

KEY = "gba:Advance Wars (USA).gba"
RED, BLUE, GREEN = (255, 0, 0), (0, 0, 255), (0, 255, 0)
# Wide enough for the details pane, where the selected game's cover shows.
WIDE = {"cols": 130, "rows": 40}


def run_cli(sandbox, *args):
    return subprocess.run([binary(), *args], env=sandbox.env, capture_output=True, text=True, timeout=10)


def jsonc(path):
    return json.loads("\n".join(line for line in path.read_text().splitlines() if not line.strip().startswith("//")))


def overrides(sandbox):
    path = sandbox.config_path.parent / "overrides.jsonc"
    return jsonc(path)["overrides"] if path.exists() else {}


def saved_cover(sandbox):
    return sandbox.root / "art/gba/Advance Wars (USA).gba.png"


def image(sandbox, name, color=RED):
    path = sandbox.home / "Pictures" / name
    path.parent.mkdir(exist_ok=True)
    path.write_bytes(png(60, 80, color))
    return path


def paste(romi, text):
    romi.send(f"\x1b[200~{text}\x1b[201~")


def showing(romi, color):
    """Whether the screen has the color on it, which half-block covers are drawn in."""
    wanted = "%02x%02x%02x" % color
    return any(
        wanted in (cell.fg, cell.bg)
        for y in range(romi.rows)
        for cell in (romi.screen.buffer[y][x] for x in range(romi.cols))
    )


def open_prompt(romi):
    romi.keys("o")
    romi.wait_for("Add art…")
    romi.choose("Add art")
    romi.wait_for("Drag an image here")


@pytest.mark.parametrize(
    "shape",
    [
        lambda p: str(p).replace(" ", "\\ "),
        lambda p: f"'{p}'",
        lambda p: f'"{p}"',
        lambda p: "file://" + str(p).replace(" ", "%20"),
        lambda p: f"{p}\n",
    ],
    ids=["backslashes", "single quotes", "double quotes", "file url", "trailing newline"],
)
def test_pasted_path_becomes_the_cover(start, sandbox, shape):
    source = image(sandbox, "my cover.png")
    romi = start()
    open_prompt(romi)
    paste(romi, shape(source))
    romi.wait_for("Art saved")
    assert "Drag an image here" not in romi.text()

    dest = saved_cover(sandbox)
    assert dest.read_bytes().startswith(b"\x89PNG")
    assert overrides(sandbox) == {KEY: {"art": str(dest)}}
    assert "overrides.jsonc" in jsonc(sandbox.config_path)["include"]
    assert json.loads(run_cli(sandbox, "info", "advance wars", "--json").stdout)["art"] == str(dest)

    # A copy: the original can go.
    source.unlink()
    assert dest.exists()


def test_cover_shows_at_once_and_can_be_replaced_and_removed(start, sandbox):
    romi = start(**WIDE)
    assert not showing(romi, RED)

    open_prompt(romi)
    paste(romi, image(sandbox, "red.png"))
    romi.wait_for("Art saved")
    romi.wait_for(lambda: showing(romi, RED))

    # The same file name again, so only forgetting the old picture can change what's shown.
    open_prompt(romi)
    paste(romi, image(sandbox, "blue.png", BLUE))
    romi.wait_for(lambda: showing(romi, BLUE))
    assert not showing(romi, RED)

    romi.keys("o")
    romi.wait_for("Remove custom art")
    romi.choose("Remove custom art")
    romi.wait_for("Art removed")
    romi.wait_for(lambda: not showing(romi, BLUE))
    assert overrides(sandbox) == {}
    assert not saved_cover(sandbox).exists()

    romi.keys("o")
    romi.wait_for("Add art…")
    assert "Remove custom art" not in romi.text()


def test_status_stays_after_saving(start, sandbox):
    # Saved through romi's own reading of the config, so the file watcher has nothing to report.
    romi = start()
    open_prompt(romi)
    paste(romi, image(sandbox, "red.png"))
    romi.wait_for("Art saved")
    romi.pump(1.5)
    assert "Art saved" in romi.text()
    assert "reloaded" not in romi.text()


def test_something_else_keeps_the_prompt_open(start, sandbox):
    text_file = sandbox.home / "notes.txt"
    text_file.write_text("not a picture")
    romi = start()
    open_prompt(romi)

    paste(romi, str(text_file))
    romi.wait_for("That isn't an image romi can read")
    romi.wait_for("Drag an image here")
    paste(romi, str(sandbox.home / "missing.png"))
    romi.wait_for("There's no file at")
    romi.wait_for("Drag an image here")
    assert overrides(sandbox) == {}
    assert not saved_cover(sandbox).exists()

    # Then a good one works, without opening it again.
    paste(romi, str(image(sandbox, "red.png")))
    romi.wait_for("Art saved")


def test_a_path_can_be_typed(start, sandbox):
    # For a terminal without bracketed paste, where a dropped path arrives as keys.
    source = image(sandbox, "my cover.png")
    romi = start()
    open_prompt(romi)
    romi.keys("x")
    romi.keys("\x7f")
    romi.keys(str(source).replace(" ", "\\ ") + "\r")
    romi.wait_for("Art saved")
    assert saved_cover(sandbox).exists()


def test_cancelling_leaves_everything_alone(start, sandbox):
    romi = start()
    open_prompt(romi)
    romi.keys("/nothing")
    romi.keys("\x1b")
    romi.wait_for(lambda: "Drag an image here" not in romi.text())
    assert overrides(sandbox) == {}
    # Keys go back to the list.
    romi.keys("j")
    romi.wait_for(lambda: "Metroid Fusion" in romi.selected())


def test_pastes_elsewhere_are_ignored(start, sandbox):
    source = image(sandbox, "red.png")
    romi = start()
    # Taken as keys, this would quit.
    paste(romi, f"q{source}")
    romi.pump(0.3)
    assert romi.exited is None
    assert "Advance Wars" in romi.selected()
    assert overrides(sandbox) == {}

    # And with a menu open, where it's no more a path than a key.
    romi.keys("o")
    romi.wait_for("Add art…")
    paste(romi, str(source))
    romi.wait_for("Add art…")
    assert overrides(sandbox) == {}


def test_pastes_still_reach_text_fields(start, sandbox):
    romi = start()
    romi.keys("/")
    paste(romi, "chrono\n")
    romi.wait_for("/ chrono")
    romi.wait_for(lambda: any("Chrono Trigger" in s for s in romi.selected()))


def test_paste_mode_is_on_only_while_romi_has_the_terminal(sandbox, start):
    romi = start()
    assert b"\x1b[?2004h" in romi.output
    assert b"\x1b[?2004l" not in romi.output

    # A game in the terminal shouldn't get pastes wrapped in escape sequences.
    for _ in range(8):
        if any("Hello Script" in s for s in romi.selected()):
            break
        romi.keys("\t")
    else:
        raise AssertionError(romi.text())
    romi.keys("\r")
    romi.wait_for("press any key")
    assert romi.output.count(b"\x1b[?2004l") == 1
    romi.keys(" ")
    romi.wait_for("Systems")
    romi.wait_for(lambda: romi.output.count(b"\x1b[?2004h") == 2)

    romi.pump(0.7)
    romi.keys("q")
    romi.wait_for(lambda: romi.output.count(b"\x1b[?2004l") == 2)


def test_hand_added_game_without_a_file(start, sandbox):
    key, dest = "game:Hello Script", sandbox.root / "art/pc/Hello Script.png"
    sandbox.write_state({"selected_system": "pc"})
    romi = start(**WIDE)
    assert "Hello Script" in romi.selected()
    open_prompt(romi)
    paste(romi, image(sandbox, "red.png"))
    romi.wait_for("Art saved")
    romi.wait_for(lambda: showing(romi, RED))
    assert overrides(sandbox) == {key: {"art": str(dest)}}
    assert dest.read_bytes().startswith(b"\x89PNG")
    assert json.loads(run_cli(sandbox, "info", "hello script", "--json").stdout)["art"] == str(dest)

    romi.keys("o")
    romi.wait_for("Remove custom art")
    romi.choose("Remove custom art")
    romi.wait_for("Art removed")
    romi.wait_for(lambda: not showing(romi, RED))
    assert overrides(sandbox) == {}
    assert not dest.exists()


def test_art_on_a_hand_added_game_comes_back(start, sandbox):
    mine = image(sandbox, "mine.png", GREEN)
    config = json.loads(json.dumps(CONFIG))
    config["games"][0]["art"] = str(mine)
    sandbox.write_config(config)
    sandbox.write_state({"selected_system": "pc"})
    romi = start(**WIDE)
    romi.wait_for(lambda: showing(romi, GREEN))
    open_prompt(romi)
    paste(romi, image(sandbox, "red.png"))
    romi.wait_for(lambda: showing(romi, RED))

    romi.keys("o")
    romi.wait_for("Remove custom art")
    romi.choose("Remove custom art")
    romi.wait_for(lambda: showing(romi, GREEN))
    assert mine.exists()


def test_steam_game_cover_goes_over_steams_and_comes_back(start, sandbox):
    steam = sandbox.root / "steam"
    (steam / "steamapps").mkdir(parents=True)
    (steam / "steamapps/appmanifest_4242.acf").write_text(
        '"AppState"\n{\n\t"appid"\t\t"4242"\n\t"name"\t\t"Fake Game"\n}\n'
    )
    cover = steam / "appcache/librarycache/4242/library_600x900.jpg"
    cover.parent.mkdir(parents=True)
    cover.write_bytes(png(60, 80, GREEN))
    config = json.loads(json.dumps(CONFIG))
    config["steam"] = {"enabled": True, "dir": str(steam), "command": "true {appid}"}
    sandbox.write_config(config)
    sandbox.write_state({"selected_system": "steam"})

    romi = start(**WIDE)
    assert "Fake Game" in romi.selected()
    romi.wait_for(lambda: showing(romi, GREEN))
    open_prompt(romi)
    paste(romi, image(sandbox, "red.png"))
    romi.wait_for("Art saved")
    romi.wait_for(lambda: showing(romi, RED))
    assert overrides(sandbox) == {"steam:4242": {"art": str(sandbox.root / "art/steam/4242.png")}}

    romi.keys("o")
    romi.wait_for("Remove custom art")
    romi.choose("Remove custom art")
    romi.wait_for(lambda: showing(romi, GREEN))
    assert cover.exists(), "Steam's own picture is never touched"
    assert not (sandbox.root / "art/steam/4242.png").exists()


def test_art_set_by_hand_is_never_deleted(start, sandbox):
    mine = image(sandbox, "mine.png", GREEN)
    config = json.loads(json.dumps(CONFIG))
    config["systems"][0]["overrides"][KEY.split(":")[1]]["art"] = str(mine)
    sandbox.write_config(config)
    romi = start(**WIDE)
    romi.wait_for(lambda: showing(romi, GREEN))

    romi.keys("o")
    romi.wait_for("Add art…")
    assert "Remove custom art" not in romi.text()
    romi.choose("Add art")
    romi.wait_for("Drag an image here")
    paste(romi, image(sandbox, "red.png"))
    romi.wait_for("Art saved")
    romi.wait_for(lambda: showing(romi, RED))

    # Removing what was added brings back what was there, in the same session, and keeps the file.
    romi.keys("o")
    romi.wait_for("Remove custom art")
    romi.choose("Remove custom art")
    romi.wait_for("Art removed")
    romi.wait_for(lambda: showing(romi, GREEN))
    assert mine.exists()
    assert not saved_cover(sandbox).exists()


def test_art_in_the_main_configs_overrides_comes_first(start, sandbox):
    mine = image(sandbox, "mine.png", GREEN)
    sandbox.write_config(dict(CONFIG, overrides={KEY: {"art": str(mine)}}))
    romi = start(**WIDE)
    romi.wait_for(lambda: showing(romi, GREEN))
    open_prompt(romi)
    paste(romi, image(sandbox, "red.png"))
    romi.wait_for('the "art" in your config comes first')
    romi.pump(0.3)
    assert showing(romi, GREEN) and not showing(romi, RED)
    romi.keys("o")
    romi.wait_for("Add art…")
    assert "Remove custom art" not in romi.text()
    assert mine.exists()


def test_scrape_leaves_an_added_cover_alone(start, sandbox):
    romi = start()
    open_prompt(romi)
    paste(romi, image(sandbox, "red.png"))
    romi.wait_for("Art saved")
    romi.close()
    before = saved_cover(sandbox).read_bytes()

    out = run_cli(sandbox, "scrape", "gba").stdout
    assert "1 on disk" in out
    assert "Advance Wars" not in out.split("no art found for:")[1]
    assert saved_cover(sandbox).read_bytes() == before
    assert not list((sandbox.home / ".cache/romi/art").glob("gba/Advance*"))

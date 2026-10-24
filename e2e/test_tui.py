import json
import time

from drive import CONFIG


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


def add_favorite_from_outside(sandbox, key):
    """Plays another romi process changing state.json while the TUI is open."""
    path = sandbox.home / ".local/state/romi/state.json"
    state = sandbox.state()
    state.setdefault("favorites", []).append(key)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(state))


def test_tui_keeps_changes_made_by_other_processes(start, sandbox):
    romi = start()
    add_favorite_from_outside(sandbox, "snes:Chrono Trigger (USA).sfc")
    romi.keys("f")
    eventually(lambda: "gba:Advance Wars (USA).gba" in sandbox.state().get("favorites", []))
    romi.close()
    assert "snes:Chrono Trigger (USA).sfc" in sandbox.state()["favorites"]


def test_tui_picks_up_changes_when_focused(start, sandbox):
    romi = start()
    assert "Favorites" not in romi.text()
    add_favorite_from_outside(sandbox, "snes:Chrono Trigger (USA).sfc")
    romi.send("\x1b[O")  # focus lost
    romi.send("\x1b[I")  # and gained
    romi.wait_for("Favorites")


def test_sorting_is_remembered_per_system(start, sandbox):
    romi = start()
    romi.keys("\t")
    romi.wait_for(lambda: "Chrono Trigger" in romi.selected())
    text = romi.text()
    assert text.index("Chrono Trigger") < text.index("Super Metroid")
    romi.keys("sss")
    romi.wait_for("Super Nintendo · by year")
    text = romi.text()
    assert text.index("Super Metroid") < text.index("Chrono Trigger")
    assert "Chrono Trigger" in romi.selected()
    eventually(lambda: sandbox.state().get("sort") == {"snes": "year"})
    romi.keys("s")
    romi.wait_for(lambda: "by year" not in romi.text())
    eventually(lambda: sandbox.state().get("sort") == {})


def test_sorting_by_last_played(start, sandbox):
    sandbox.write_state({"last_played": {"gba:Metroid Fusion (USA).gba": 1700000000}})
    romi = start()
    romi.keys("s")
    romi.wait_for("Game Boy Advance · by last played")
    text = romi.text()
    assert text.index("Metroid Fusion") < text.index("Advance Wars")


def test_start_on_last_played(start, sandbox):
    sandbox.write_config(dict(CONFIG, start_on="last_played"))
    sandbox.write_state({"recent": ["snes:Chrono Trigger (USA).sfc"], "selected_system": "gba",
                         "selected_game": "gba:Metroid Fusion (USA).gba"})
    romi = start()
    assert "Chrono Trigger" in romi.selected()

"""`romi add`: turning programs, scripts, folders and .desktop files into games."""

import json
import re
import subprocess

import pytest

from drive import CONFIG, binary


def romi(sandbox, *args):
    return subprocess.run([binary(), *args], env=sandbox.env, capture_output=True, text=True, timeout=10)


def jsonc(text):
    return json.loads(re.sub(r"^\s*//.*$", "", text, flags=re.M))


def program(path, executable=True):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("#!/bin/sh\necho running\n")
    path.chmod(0o755 if executable else 0o644)
    return path


def added(sandbox):
    return jsonc((sandbox.root / "games.jsonc").read_text())["games"]


def test_adds_the_program_in_a_folder(sandbox):
    program(sandbox.home / "Games/cave_story/CaveStory.x86_64")
    program(sandbox.home / "Games/cave_story/setup_linux.sh")
    program(sandbox.home / "Games/cave_story/.gitignore")
    (sandbox.home / "Games/cave_story/data.pak").touch()
    out = romi(sandbox, "add", str(sandbox.home / "Games/cave_story"), "--yes")
    assert out.returncode == 0, out.stderr
    assert "Added Cave Story. Try it with: romi play 'Cave Story'" in out.stdout
    assert added(sandbox) == [{"name": "Cave Story", "command": "./CaveStory.x86_64", "cwd": "~/Games/cave_story"}]
    assert jsonc(sandbox.config_path.read_text())["include"] == ["games.jsonc"]
    dry = json.loads(romi(sandbox, "play", "cave story", "--dry-run", "--json").stdout)
    assert dry["argv"] == ["./CaveStory.x86_64"] and dry["cwd"] == str(sandbox.home / "Games/cave_story")
    # It runs, from its own folder.
    assert romi(sandbox, "play", "cave story", "--mode", "terminal").stdout == "running\n"


def test_scripts_get_the_terminal(sandbox):
    program(sandbox.home / "scripts/update_mods.sh", executable=False)
    out = romi(sandbox, "add", str(sandbox.home / "scripts/update_mods.sh"), "--yes")
    assert out.returncode == 0, out.stderr
    [game] = added(sandbox)
    assert game == {"name": "Update Mods", "command": "sh update_mods.sh", "cwd": "~/scripts", "mode": "terminal"}


def test_desktop_files(sandbox):
    desktop = sandbox.home / "celeste.desktop"
    desktop.write_text(
        "[Desktop Entry]\nType=Application\nName=Celeste\nExec=flatpak run com.example.Celeste %U\n"
        "[Desktop Action Other]\nName=Not this\nExec=nope\n"
    )
    out = romi(sandbox, "add", str(desktop), "--yes")
    assert out.returncode == 0, out.stderr
    assert added(sandbox) == [{"name": "Celeste", "command": "flatpak run com.example.Celeste"}]


def test_options_and_a_second_game(sandbox):
    program(sandbox.home / "a/run")
    program(sandbox.home / "b/start")
    romi(sandbox, "add", str(sandbox.home / "a/run"), "--yes")
    out = romi(sandbox, "add", str(sandbox.home / "b/start"), "--name", "Bee", "--system", "Ports", "--mode", "detach", "--yes")
    assert out.returncode == 0, out.stderr
    assert [g["name"] for g in added(sandbox)] == ["Run", "Bee"]
    assert added(sandbox)[1]["system"] == "Ports" and added(sandbox)[1]["mode"] == "detach"
    assert jsonc(sandbox.config_path.read_text())["include"] == ["games.jsonc"]
    assert "Ports\tBee\tgame:Bee" in romi(sandbox, "list").stdout


def test_joins_init_in_the_include_list(sandbox):
    sandbox.write_config(dict(CONFIG, include=["init.jsonc"]))
    (sandbox.root / "init.jsonc").write_text('{ "systems": [] }')
    program(sandbox.home / "x/run")
    assert romi(sandbox, "add", str(sandbox.home / "x/run"), "--yes").returncode == 0
    assert jsonc(sandbox.config_path.read_text())["include"] == ["init.jsonc", "games.jsonc"]


def test_print_writes_nothing(sandbox):
    program(sandbox.home / "x/run")
    out = romi(sandbox, "add", str(sandbox.home / "x/run"), "--print")
    assert json.loads(out.stdout) == {"name": "Run", "command": "./run", "cwd": "~/x"}
    assert not (sandbox.root / "games.jsonc").exists()
    assert "include" not in sandbox.config_path.read_text()


def test_asks_first(sandbox):
    program(sandbox.home / "x/run")
    out = romi(sandbox, "add", str(sandbox.home / "x/run"))
    assert out.returncode == 64 and "--yes" in out.stderr
    assert not (sandbox.root / "games.jsonc").exists()


@pytest.mark.parametrize(
    "setup, code, message",
    [
        (lambda h: h / "missing", 66, "doesn't exist"),
        (lambda h: (h / "empty").mkdir() or h / "empty", 66, "nothing to run"),
        (lambda h: program(h / "two/a") and program(h / "two/b") and h / "two", 65, "~/two/a"),
        (lambda h: program(h / "s/build.sh") and program(h / "s/play.command") and h / "s", 65, "~/s/play.command"),
        (lambda h: program(h / "t/a") and program(h / "t/b") and program(h / "t/c.sh") and h / "t", 65, "~/t/a\n  ~/t/b\n  ~/t/c.sh"),
        (lambda h: program(h / "x/data.bin", executable=False), 64, "chmod +x"),
        (lambda h: (h / "game.exe").touch() or h / "game.exe", 64, "Wine"),
        (lambda h: (h / "Golden Sun.gba").touch() or h / "Golden Sun.gba", 64, "Game Boy Advance game; put it in ~/ROMs/gba"),
        (lambda h: program(h / "dup/hello_script"), 64, "already a game called"),
    ],
)
def test_refusals(sandbox, setup, code, message):
    path = setup(sandbox.home)
    out = romi(sandbox, "add", str(path), "--yes")
    assert out.returncode == code, out.stderr
    assert message in out.stderr
    assert not (sandbox.root / "games.jsonc").exists()


def test_needs_a_config(sandbox):
    sandbox.config_path.unlink()
    program(sandbox.home / "x/run")
    out = romi(sandbox, "add", str(sandbox.home / "x/run"), "--yes")
    assert out.returncode == 78 and "romi init" in out.stderr

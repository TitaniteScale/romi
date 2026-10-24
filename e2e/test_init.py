"""`romi init`: finding systems and emulators, and writing the config."""

import json
import re
import subprocess

import pytest

from drive import CONFIG, binary

SYSTEMS = """<?xml version="1.0"?>
<systemList>
  <system>
    <name>gb</name><fullname>Nintendo Game Boy</fullname><path>%ROMPATH%/gb</path>
    <extension>.gb .GB .zip</extension>
    <command label="Gambatte">%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/gambatte_libretro.so %ROM%</command>
    <command label="FakeGB (Standalone)">%INJECT%=%BASENAME%.esprefix %EMULATOR_FAKEGB% -f %ROM%</command>
    <platform>gb</platform>
  </system>
  <system>
    <name>gba</name><fullname>Nintendo Game Boy Advance</fullname><path>%ROMPATH%/gba</path>
    <extension>.gba .zip</extension>
    <command label="mGBA">%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/mgba_libretro.so %ROM%</command>
    <platform>gba</platform>
  </system>
  <system>
    <name>snes</name><fullname>Nintendo SNES</fullname><path>%ROMPATH%/snes</path>
    <extension>.sfc</extension>
    <command label="Nope">%EMULATOR_NOPE% %ROM%</command>
    <platform>snes</platform>
  </system>
  <system>
    <name>segacd</name><fullname>Sega CD</fullname><path>%ROMPATH%/segacd</path>
    <extension>.chd .cue</extension>
    <command label="Genesis Plus GX">%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/genesis_plus_gx_libretro.so %ROM%</command>
    <platform>segacd</platform>
  </system>
  <system>
    <name>n64</name><fullname>Nintendo 64</fullname><path>%ROMPATH%/n64</path>
    <extension>.z64</extension>
    <command label="Mupen64Plus-Next">%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/mupen64plus_next_libretro.so %ROM%</command>
    <platform>n64</platform>
  </system>
</systemList>
"""

RULES = """<?xml version="1.0"?>
<ruleList>
  <emulator name="RETROARCH">
    <rule type="systempath"><entry>fake-retroarch</entry></rule>
  </emulator>
  <core name="RETROARCH">
    <rule type="corepath"><entry>~/nowhere</entry><entry>~/cores</entry></rule>
  </core>
  <emulator name="FAKEGB">
    <rule type="systempath"><entry>fakegb-not-on-path</entry></rule>
    <rule type="staticpath"><entry>~/Apps/FakeGB*.AppImage</entry></rule>
  </emulator>
  <emulator name="NOPE">
    <rule type="systempath"><entry>nope-emu</entry></rule>
  </emulator>
</ruleList>
"""


def executable(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("#!/bin/sh\nexit 0\n")
    path.chmod(0o755)


@pytest.fixture
def machine(sandbox):
    """A sandbox with a fake ES-DE, some emulators and cores, and a few more ROM folders."""
    home = sandbox.home
    esde = sandbox.root / "esde"
    esde.mkdir()
    (esde / "es_systems.xml").write_text(SYSTEMS)
    (esde / "es_find_rules.xml").write_text(RULES)
    executable(sandbox.root / "bin/fake-retroarch")
    executable(home / "Apps/FakeGB-1.2.AppImage")
    (home / "cores").mkdir()
    for core in ["mgba_libretro.so", "genesis_plus_gx_libretro.so"]:
        (home / "cores" / core).touch()
    for folder, files in {
        "gb": ["Tetris (World).gb"],
        "scd": ["Sonic CD (USA).chd"],
        "handheld": ["Golden Sun (USA).gba", "Mother 3 (Japan).gba"],
        "n64": [],
        "notes": ["todo.txt"],
    }.items():
        (home / "ROMs" / folder).mkdir(parents=True)
        for f in files:
            (home / "ROMs" / folder / f).touch()
    sandbox.esde = esde
    sandbox.env_extra = {"PATH": f"{sandbox.root / 'bin'}:{sandbox.env['PATH']}"}
    return sandbox


def romi(sandbox, *args, input=None):
    env = dict(sandbox.env, **getattr(sandbox, "env_extra", {}))
    return subprocess.run([binary(), *args], env=env, capture_output=True, text=True, timeout=20, input=input)


def init(sandbox, *args):
    return romi(sandbox, "init", "--esde", str(sandbox.esde), *args)


def jsonc(text):
    return json.loads(re.sub(r"^\s*//.*$", "", text, flags=re.M))


def systems(text):
    return {s["id"]: s for s in jsonc(text)["systems"]}


def fresh(sandbox):
    sandbox.config_path.unlink()
    return sandbox


def test_print_shows_what_it_found_and_writes_nothing(machine):
    fresh(machine)
    out = init(machine, "--print")
    assert out.returncode == 0, out.stderr
    found = systems(out.stdout)
    assert jsonc(out.stdout)["vars"] == {"cores": "~/cores"}
    # Gambatte's core isn't there, so the next command that works wins; %INJECT% is dropped.
    assert found["gb"]["command"] == "~/Apps/FakeGB-1.2.AppImage -f {rom}"
    assert found["gba"]["command"] == "fake-retroarch -L {cores}/mgba_libretro.so {rom}"
    # A folder name ES-DE doesn't use, but a common one for the same system.
    assert found["segacd"]["dir"] == "scd"
    assert found["segacd"]["bios"] == [["bios_CD_U.bin", "bios_CD_E.bin", "bios_CD_J.bin"]]
    # Recognized by its .gba files; "gba" is taken by the gba folder, so it goes by its own name
    # (which is also its folder, so no "dir").
    assert found["handheld"]["name"] == "Nintendo Game Boy Advance" and "dir" not in found["handheld"]
    assert set(found) == {"gb", "gba", "segacd", "handheld"}
    summary = out.stderr
    assert re.search(r"! snes .* no emulator installed \(ES-DE knows: Nope\)", summary)
    assert re.search(r"- n64 .* no games", summary)
    assert re.search(r"\? notes .* not a system romi knows", summary)
    assert "recognized by file type" in summary
    assert not machine.config_path.exists()
    assert not (machine.root / "init.jsonc").exists()
    # Nothing about the games themselves goes in the file.
    assert "Tetris" not in out.stdout and not re.search(r"\d+ games?\b", out.stdout)


def test_fresh_init_writes_a_working_config(machine):
    fresh(machine)
    out = init(machine, "--yes")
    assert out.returncode == 0, out.stderr
    assert "Done: 6 games in 4 systems" in out.stdout
    config = jsonc(machine.config_path.read_text())
    assert config["include"] == ["init.jsonc"] and config["roms_dir"] == "~/ROMs"
    games = {g["name"]: g for g in json.loads(romi(machine, "list", "--json").stdout)}
    assert games["Tetris"]["system"] == "gb"
    assert games["Golden Sun"]["system"] == "handheld"
    played = romi(machine, "play", "tetris", "--dry-run")
    assert f"{machine.home}/Apps/FakeGB-1.2.AppImage -f" in played.stdout


def test_init_adds_to_an_existing_config_without_touching_it(machine):
    text = "// my config\n{\n  // my systems\n" + json.dumps(CONFIG, indent=2)[1:]
    machine.config_path.write_text(text)
    out = init(machine, "--yes")
    assert out.returncode == 0, out.stderr
    assert re.search(r'= gba +already in your config as "gba"', out.stdout)
    written = machine.config_path.read_text()
    assert written.startswith("// my config\n{\n  // Systems found by `romi init`")
    assert written.endswith(text[2 + text.index("{\n") :])
    found = systems((machine.root / "init.jsonc").read_text())
    assert "gba" not in found and "snes" not in found and "gb" in found
    assert found["handheld"]["extensions"] == ["gba", "zip"]
    names = romi(machine, "list").stdout
    assert "Tetris" in names and "Advance Wars" in names


def test_rerunning_replaces_only_its_own_file(machine):
    fresh(machine)
    init(machine, "--yes")
    config = machine.config_path.read_text()
    first = (machine.root / "init.jsonc").read_text()
    (machine.home / "ROMs/n64/Mario Kart 64 (USA).z64").touch()
    (machine.home / "cores/mupen64plus_next_libretro.so").touch()
    out = init(machine, "--yes")
    assert out.returncode == 0, out.stderr
    assert machine.config_path.read_text() == config
    second = systems((machine.root / "init.jsonc").read_text())
    assert set(second) == set(systems(first)) | {"n64"}


def test_asks_before_writing(machine):
    fresh(machine)
    out = init(machine)
    assert out.returncode == 64
    assert "--yes" in out.stderr
    assert not machine.config_path.exists()


def test_joins_an_existing_include_list(machine):
    machine.write_config(dict(CONFIG, include=["mine.jsonc"]))
    (machine.root / "mine.jsonc").write_text("{}")
    out = init(machine, "--yes")
    assert out.returncode == 0, out.stderr
    assert jsonc(machine.config_path.read_text())["include"] == ["mine.jsonc", "init.jsonc"]


def test_builtin_systems_without_esde(machine):
    fresh(machine)
    executable(machine.root / "bin/retroarch")
    cores = machine.home / ".config/retroarch/cores"
    cores.mkdir(parents=True)
    (cores / "mgba_libretro.so").touch()
    out = romi(machine, "init", "--no-esde", "--print")
    assert out.returncode == 0, out.stderr
    assert "romi's built-in system list" in out.stderr
    found = systems(out.stdout)
    assert found["gba"]["command"] == "retroarch -L {cores}/mgba_libretro.so {rom}"
    assert jsonc(out.stdout)["vars"]["cores"] == "~/.config/retroarch/cores"


def test_bad_esde_folder(machine):
    out = romi(machine, "init", "--esde", str(machine.root), "--print")
    assert out.returncode == 64


def test_no_roms_found(machine):
    fresh(machine)
    out = romi(machine, "init", "--esde", str(machine.esde), "--print", "--roms", str(machine.root / "missing"))
    assert out.returncode == 66
    (machine.home / "ROMs").rename(machine.home / "elsewhere")
    out = romi(machine, "init", "--esde", str(machine.esde), "--print")
    assert out.returncode == 66 and "--roms" in out.stderr


def test_nothing_to_add_leaves_the_config_alone(machine):
    config = json.loads(json.dumps(CONFIG))
    config["systems"] = [{"id": f, "name": f, "extensions": ["gba"], "command": "true {rom}"} for f in ["gb", "scd", "handheld"]]
    config["systems"] += CONFIG["systems"]
    machine.write_config(config)
    before = machine.config_path.read_text()
    out = init(machine, "--yes")
    assert out.returncode == 0, out.stderr
    assert "Nothing new to add" in out.stdout
    assert machine.config_path.read_text() == before
    assert not (machine.root / "init.jsonc").exists()


def test_uses_the_rom_folder_set_in_esde(machine):
    fresh(machine)
    (machine.home / "ROMs").rename(machine.home / "Games")
    settings = machine.home / "ES-DE/settings/es_settings.xml"
    settings.parent.mkdir(parents=True)
    settings.write_text('<?xml version="1.0"?>\n<string name="ROMDirectory" value="~/Games" />\n')
    out = init(machine, "--print")
    assert out.returncode == 0, out.stderr
    assert out.stderr.startswith("ROMs in ~/Games")


def check(sandbox):
    out = romi(sandbox, "check")
    assert out.returncode == 0, out.stderr
    return out.stdout


def with_esde(sandbox, config):
    sandbox.write_config(dict(config, esde_dir=str(sandbox.esde)))


def test_check_suggests_an_installed_emulator(machine):
    config = json.loads(json.dumps(CONFIG))
    config["systems"].append({"id": "gb", "name": "Game Boy", "extensions": ["gb"], "command": "gone-emu {rom}"})
    with_esde(machine, config)
    out = check(machine)
    assert re.search(r"! Game Boy .* gone-emu not found", out)
    assert re.search(r'  gb +FakeGB \(Standalone\) is installed and can run it.*\n +"~/Apps/FakeGB-1.2.AppImage -f \{rom\}"', out)


def test_check_suggests_folders_to_add_and_emulators_to_install(machine):
    config = json.loads(json.dumps(CONFIG))
    config["systems"] = [s for s in config["systems"] if s["id"] != "snes"]
    with_esde(machine, config)
    out = check(machine)
    assert "  scd          1 game for Sega CD, not in your config. Genesis Plus GX can run them: `romi init` adds them." in out
    assert "  snes         2 games for Nintendo SNES, but no emulator for it is installed (ES-DE knows: Nope)" in out
    # Empty and unknown folders aren't worth mentioning.
    assert "n64" not in out and "notes" not in out


def test_check_has_no_suggestions_when_all_is_well(sandbox):
    # (The heading, not the word: pytest puts the test's name in the config's path.)
    assert "\nsuggestions\n" not in check(sandbox)


def test_init_uses_esde_dir_from_the_config(machine):
    with_esde(machine, CONFIG)
    out = romi(machine, "init", "--print")
    assert out.returncode == 0, out.stderr
    assert "ES-DE's system list" in out.stderr and "gb" in systems(out.stdout)

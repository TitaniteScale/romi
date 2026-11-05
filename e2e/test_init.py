"""`romi init`: finding systems and emulators, and writing the config."""

import json
import re
import shutil
import subprocess
import types

import pytest

from drive import CONFIG, Romi, binary

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


def no_retroarch(machine):
    """Only the sandbox's own programs, and no RetroArch among them."""
    (machine.root / "bin/fake-retroarch").unlink()
    machine.env_extra = {"PATH": str(machine.root / "bin")}


def test_a_core_is_enough_without_retroarch(machine):
    fresh(machine)
    no_retroarch(machine)
    # Gambatte's core comes first in ES-DE's list for the Game Boy, ahead of FakeGB.
    (machine.home / "cores/gambatte_libretro.so").touch()
    found = systems(init(machine, "--print").stdout)
    assert found["segacd"]["command"] == "retroarch -L {cores}/genesis_plus_gx_libretro.so {rom}"
    # What can start by itself comes before a core that can only play in romi.
    assert found["gb"]["command"] == "~/Apps/FakeGB-1.2.AppImage -f {rom}"
    # No core and no emulator for the N64.
    assert "n64" not in found


def test_built_in_systems_without_retroarch(machine):
    fresh(machine)
    no_retroarch(machine)
    cores = machine.home / ".config/retroarch/cores"
    cores.mkdir(parents=True)
    (cores / "mesen_libretro.so").touch()
    (machine.home / "ROMs/nes").mkdir()
    (machine.home / "ROMs/nes/Donkey Kong (World).nes").touch()
    out = romi(machine, "init", "--no-esde", "--print")
    assert out.returncode == 0, out.stderr
    assert systems(out.stdout)["nes"]["command"] == "retroarch -L {cores}/mesen_libretro.so {rom}"


def test_a_core_without_retroarch_passes_check(machine):
    fresh(machine)
    no_retroarch(machine)
    assert init(machine, "--yes").returncode == 0
    out = romi(machine, "check")
    assert out.returncode == 0, out.stdout + out.stderr
    assert "retroarch not found" not in out.stdout


def test_other_names_for_rom_folders_on_the_built_in_list(machine):
    # The names Batocera uses, among them.
    fresh(machine)
    cores = machine.home / ".config/retroarch/cores"
    cores.mkdir(parents=True)
    executable(machine.root / "bin/retroarch")
    for core in ["genesis_plus_gx", "mame", "fbneo", "handy"]:
        (cores / f"{core}_libretro.so").touch()
    for folder, game in {"segacd": "a.chd", "mame": "a.zip", "fbneo": "b.zip", "lynx": "a.zip"}.items():
        (machine.home / "ROMs" / folder).mkdir()
        (machine.home / "ROMs" / folder / game).touch()
    found = systems(romi(machine, "init", "--no-esde", "--print").stdout)
    # The sandbox has an scd folder too, so this one goes by its own name.
    assert found["segacd"]["command"].endswith("genesis_plus_gx_libretro.so {rom}")
    assert found["arcade"]["dir"] == "mame"
    assert found["fbneo"]["command"].endswith("fbneo_libretro.so {rom}")
    assert found["atarilynx"]["dir"] == "lynx"


def test_a_new_config_without_retroarch_keeps_everything_in_romis_folder(machine):
    fresh(machine)
    no_retroarch(machine)
    shutil.rmtree(machine.home / ".config/retroarch", ignore_errors=True)
    out = romi(machine, "init", "--no-esde", "--yes")
    assert out.returncode == 0, out.stderr
    assert "BIOS files go in ~/.local/share/romi/system, and downloaded cores in ~/.local/share/romi/cores" in out.stdout
    own = machine.home / ".local/share/romi"
    assert all((own / name).is_dir() for name in ["system", "saves", "cores"])
    config = jsonc(machine.config_path.read_text())
    assert config["bios_dirs"] == ["~/.local/share/romi/system"]
    assert config["saves_dir"] == "~/.local/share/romi/saves"
    assert not (machine.home / ".config/retroarch").exists()

    # A core put there is found, as the first place to look.
    (own / "cores/mgba_libretro.so").touch()
    out = romi(machine, "init", "--no-esde", "--yes")
    assert out.returncode == 0, out.stderr
    assert jsonc((machine.config_path.parent / "init.jsonc").read_text())["vars"]["cores"] == "~/.local/share/romi/cores"
    shown = check(machine)
    assert "cores     1 in ~/.local/share/romi/cores" in shown
    assert "bios      ~/.local/share/romi/system" in shown
    assert "saves     ~/.local/share/romi/saves" in shown
    assert "states    ~/.local/share/romi/states" in shown

    # Installing RetroArch later doesn't move anything: the config says where they are.
    (machine.home / ".config/retroarch/system").mkdir(parents=True)
    executable(machine.root / "bin/retroarch")
    assert romi(machine, "init", "--no-esde", "--yes").returncode == 0
    assert jsonc(machine.config_path.read_text())["saves_dir"] == "~/.local/share/romi/saves"
    assert "bios      ~/.local/share/romi/system" in check(machine)


FLATPAK_RETROARCH = ".var/app/org.libretro.RetroArch/config/retroarch"


@pytest.mark.parametrize("found_by", ["its folder", "being installed"])
def test_a_new_config_with_only_a_flatpak_retroarch_uses_its_folders(machine, found_by):
    fresh(machine)
    no_retroarch(machine)
    if found_by == "its folder":
        (machine.home / FLATPAK_RETROARCH).mkdir(parents=True)
    else:
        (machine.home / ".local/share/flatpak/app/org.libretro.RetroArch").mkdir(parents=True)
    out = romi(machine, "init", "--no-esde", "--yes")
    assert out.returncode == 0, out.stderr
    assert f"BIOS files go in ~/{FLATPAK_RETROARCH}/system." in out.stdout
    config = jsonc(machine.config_path.read_text())
    assert config["bios_dirs"] == [f"~/{FLATPAK_RETROARCH}/system"]
    assert "saves_dir" not in config
    assert not (machine.home / ".local/share/romi").exists()
    shown = check(machine)
    assert f"bios      ~/{FLATPAK_RETROARCH}/system" in shown
    assert f"saves     ~/{FLATPAK_RETROARCH}/saves" in shown


def test_a_flatpak_retroarchs_save_folder_comes_from_its_own_settings(machine):
    fresh(machine)
    no_retroarch(machine)
    (machine.home / FLATPAK_RETROARCH).mkdir(parents=True)
    (machine.home / FLATPAK_RETROARCH / "retroarch.cfg").write_text('savefile_directory = "~/flatpak-saves"\n')
    assert romi(machine, "init", "--no-esde", "--yes").returncode == 0
    assert "saves     ~/flatpak-saves" in check(machine)


@pytest.mark.parametrize("also", ["binary", "folder"])
def test_a_retroarch_on_the_machine_itself_wins_over_a_flatpak(machine, also):
    fresh(machine)
    no_retroarch(machine)
    (machine.home / FLATPAK_RETROARCH).mkdir(parents=True)
    if also == "binary":
        executable(machine.root / "bin/retroarch")
    else:
        (machine.home / ".config/retroarch").mkdir(parents=True)
    out = romi(machine, "init", "--no-esde", "--yes")
    assert out.returncode == 0, out.stderr
    assert "BIOS files go in ~/.config/retroarch/system." in out.stdout
    assert "saves     ~/.config/retroarch/saves" in check(machine)


@pytest.mark.parametrize("left_behind", ["binary", "folder"])
def test_a_new_config_with_retroarch_keeps_using_its_folders(machine, left_behind):
    fresh(machine)
    no_retroarch(machine)
    shutil.rmtree(machine.home / ".config/retroarch", ignore_errors=True)
    if left_behind == "binary":
        executable(machine.root / "bin/retroarch")
    else:
        (machine.home / ".config/retroarch").mkdir(parents=True)
    out = romi(machine, "init", "--no-esde", "--yes")
    assert out.returncode == 0, out.stderr
    assert "BIOS files go in ~/.config/retroarch/system." in out.stdout
    config = jsonc(machine.config_path.read_text())
    assert "saves_dir" not in config and "bios_dirs" not in config
    assert not (machine.home / ".local/share/romi").exists()
    assert "saves     ~/.config/retroarch/saves" in check(machine)


def test_an_existing_config_stays_as_it_is_without_retroarch(machine):
    with_esde(machine, CONFIG)
    no_retroarch(machine)
    shutil.rmtree(machine.home / ".config/retroarch", ignore_errors=True)
    before = machine.config_path.read_text()
    romi(machine, "init", "--yes")
    assert jsonc(machine.config_path.read_text()).get("saves_dir") is None
    assert "bios_dirs" not in before or jsonc(before)["bios_dirs"] == jsonc(machine.config_path.read_text())["bios_dirs"]
    assert not (machine.home / ".local/share/romi").exists()
    assert "bios      ! ~/.config/retroarch/system doesn't exist" in check(machine)


def test_retroarch_never_opened_still_gives_cores_a_system_folder(machine):
    fresh(machine)
    no_retroarch(machine)
    shutil.rmtree(machine.home / ".config/retroarch", ignore_errors=True)
    executable(machine.root / "bin/retroarch")
    out = romi(machine, "init", "--no-esde", "--yes")
    assert out.returncode == 0, out.stderr
    assert "BIOS files go in ~/.config/retroarch/system." in out.stdout
    assert "bios      ~/.config/retroarch/system" in check(machine)


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


def test_check_says_what_the_terminal_can_show(sandbox):
    sandbox.extra_env["TERM"] = "xterm-ghostty"
    assert "terminal  Ghostty: games play inside romi" in check(sandbox)
    sandbox.extra_env["TERM"] = "foot"
    assert "terminal  ! foot: games only show inside romi in Ghostty or kitty" in check(sandbox)


def test_check_says_where_cores_are(machine):
    with_esde(machine, CONFIG)
    assert "cores     2 in ~/cores" in check(machine)
    for core in (machine.home / "cores").iterdir():
        core.unlink()
    assert "cores     ! none found, looked in ~/.local/share/romi/cores, ~/nowhere, ~/cores" in check(machine)


def test_check_says_when_lrps2_has_no_bios(machine):
    config = json.loads(json.dumps(CONFIG))
    config["systems"].append({"id": "ps2", "name": "PlayStation 2", "extensions": ["iso"], "command": "true {rom}"})
    with_esde(machine, config)
    (machine.home / "ROMs/ps2").mkdir()
    (machine.home / "ROMs/ps2/a.iso").touch()
    assert "LRPS2" not in check(machine), "no LRPS2 installed"
    (machine.home / "cores/pcsx2_libretro.so").touch()
    assert "! LRPS2 has no BIOS in ~/.config/retroarch/system/pcsx2/bios" in check(machine)
    bios = machine.home / ".config/retroarch/system/pcsx2/bios"
    bios.mkdir(parents=True)
    (bios / "ps2.bin").touch()
    assert "LRPS2" not in check(machine)


def test_check_has_no_suggestions_when_all_is_well(sandbox):
    # (The heading, not the word: pytest puts the test's name in the config's path.)
    assert "\nsuggestions\n" not in check(sandbox)


def test_init_uses_esde_dir_from_the_config(machine):
    with_esde(machine, CONFIG)
    out = romi(machine, "init", "--print")
    assert out.returncode == 0, out.stderr
    assert "ES-DE's system list" in out.stderr and "gb" in systems(out.stdout)


def test_commands_without_a_config_say_to_run_init(machine):
    fresh(machine)
    out = romi(machine, "list")
    assert out.returncode == 78 and "run `romi init` first" in out.stderr
    assert not machine.config_path.exists()
    # status only needs the state folder.
    assert romi(machine, "status").returncode == 0


def test_init_finds_the_roms_when_roms_dir_is_gone(machine):
    machine.config_path.write_text('// mine\n{\n  "roms_dir": "~/Gone",\n  "games": []\n}\n')
    out = init(machine, "--yes")
    assert out.returncode == 0, out.stderr
    config = machine.config_path.read_text()
    assert config.startswith("// mine\n{\n") and '"roms_dir": "~/ROMs"' in config
    assert "Tetris" in romi(machine, "list").stdout


def test_roms_somewhere_else_get_full_paths(machine):
    # The config's own systems are in its roms_dir, so that stays as it is.
    (machine.root / "more/gb").mkdir(parents=True)
    (machine.root / "more/gb/Wario Land (USA).gb").touch()
    out = init(machine, "--roms", str(machine.root / "more"), "--yes")
    assert out.returncode == 0, out.stderr
    assert jsonc(machine.config_path.read_text())["roms_dir"] == "~/ROMs"
    assert systems((machine.root / "init.jsonc").read_text())["gb"]["dir"] == str(machine.root / "more/gb")
    names = romi(machine, "list").stdout
    assert "Wario Land" in names and "Advance Wars" in names


def first_run(sandbox, pad=None):
    """romi with no config and no ES-DE of its own; it may find the machine's, so only what
    doesn't depend on that is checked."""
    fresh(sandbox)
    executable(sandbox.root / "bin/retroarch")
    cores = sandbox.home / ".config/retroarch/cores"
    cores.mkdir(parents=True)
    (cores / "mgba_libretro.so").touch()
    env = dict(sandbox.env, PATH=f"{sandbox.root / 'bin'}:{sandbox.env['PATH']}")
    return Romi(types.SimpleNamespace(env=env), pad=pad)


def test_first_run_works_from_a_controller(machine, pad):
    romi = first_run(machine, pad=pad)
    try:
        romi.wait_for("Press any key to get started")
        romi.press("WEST")
        romi.wait_for("Where are your ROMs?")
        romi.press("SOUTH")
        romi.wait_for("Where should romi keep BIOS files and saves?")
        romi.press("DOWN")
        romi.wait_for("› romi's own folder")
        romi.press("EAST")
        romi.wait_for("Where are your ROMs?")
        romi.press("SOUTH")
        romi.wait_for("Where should romi keep BIOS files and saves?")
        romi.press("SOUTH")
        romi.wait_for("Here's what romi found")
        assert "BIOS files go in ~/.local/share/romi/system." in romi.text()
        romi.press("SOUTH")
        romi.wait_for(lambda: machine.config_path.exists())
        assert jsonc(machine.config_path.read_text())["saves_dir"] == "~/.local/share/romi/saves"
    finally:
        romi.close()


def test_first_run_welcomes_asks_and_opens(machine):
    romi = first_run(machine)
    try:
        romi.wait_for("Press any key to get started")
        assert "Play your games in your terminal" in romi.text()
        assert b"\x1b_G" in romi.output, "the logo, as Kitty graphics"
        assert not machine.config_path.exists(), "nothing's written yet"
        romi.keys(" ")
        romi.wait_for("Where are your ROMs?")
        assert "~/ROMs" in romi.text()
        romi.keys("\r")
        romi.wait_for("Where should romi keep BIOS files and saves?")
        assert "› RetroArch's folders" in romi.text(), "unless it's changed"
        romi.keys("\r")
        romi.wait_for("Here's what romi found")
        assert "ROMs in ~/ROMs" in romi.text()
        assert "BIOS files go in ~/.config/retroarch/system." in romi.text()
        assert not machine.config_path.exists(), "still nothing"
        romi.keys("\r")
        romi.wait_for("Systems")
        assert jsonc(machine.config_path.read_text())["include"] == ["init.jsonc"]
        assert (machine.config_path.parent / "init.jsonc").exists()
        # A first run doesn't turn the title screen on.
        assert "title_screen" not in machine.config_path.read_text()
    finally:
        romi.close()


def test_first_run_turned_down_writes_nothing(machine):
    romi = first_run(machine)
    try:
        romi.wait_for("Press any key to get started")
        romi.keys(" ")
        romi.wait_for("Where are your ROMs?")
        romi.keys("\r")
        romi.wait_for("Where should romi keep BIOS files and saves?")
        romi.keys("\r")
        romi.wait_for("Here's what romi found")
        # Back out, a screen at a time.
        romi.keys("\x1b")
        romi.wait_for("Where should romi keep BIOS files and saves?")
        romi.keys("\x1b")
        romi.wait_for("Where are your ROMs?")
        romi.keys("\x1b")
        romi.wait_for("Press any key to get started")
        romi.keys("\x1b")
        romi.wait_for(lambda: romi.exited is not None)
        assert romi.exited == 0
        assert not machine.config_path.exists()
    finally:
        romi.close()


def test_first_run_without_roms_still_opens(machine):
    (machine.home / "ROMs").rename(machine.home / "elsewhere")
    romi = first_run(machine)
    try:
        romi.wait_for("Press any key to get started")
        romi.keys(" ")
        romi.wait_for("Where are your ROMs?")
        assert "found none" in romi.text()
        romi.keys("\r")
        romi.wait_for("Where should romi keep BIOS files and saves?")
        romi.keys("\r")
        romi.wait_for("No ROMs yet")
        romi.keys("\r")
        romi.wait_for("No games found")
        assert jsonc(machine.config_path.read_text())["roms_dir"] == "~/ROMs"
    finally:
        romi.close()


def test_first_run_takes_a_folder_typed_or_dropped(machine):
    (machine.home / "ROMs").rename(machine.home / "elsewhere")
    romi = first_run(machine)
    try:
        romi.wait_for("Press any key to get started")
        romi.keys(" ")
        romi.wait_for("Where are your ROMs?")
        for _ in "~/ROMs":
            romi.keys("\x7f", settle=0.02)
        romi.keys("~/nowhere\r")
        romi.wait_for("~/nowhere isn't a folder")
        # Dropped on the terminal, in quotes, as some do, it replaces what's there.
        romi.send(f"\x1b[200~'{machine.home / 'elsewhere'}'\x1b[201~")
        romi.keys("\r")
        romi.wait_for("Where should romi keep BIOS files and saves?")
        romi.keys("\r")
        romi.wait_for("ROMs in ~/elsewhere")
        romi.keys("\r")
        romi.wait_for("Systems")
        assert jsonc(machine.config_path.read_text())["roms_dir"] == "~/elsewhere"
    finally:
        romi.close()


def test_first_run_can_keep_things_in_romis_folder_with_retroarch_here(machine):
    romi = first_run(machine)
    try:
        romi.wait_for("Press any key to get started")
        romi.keys(" ")
        romi.wait_for("Where are your ROMs?")
        romi.keys("\r")
        romi.wait_for("Where should romi keep BIOS files and saves?")
        romi.send("\x1b[B")
        romi.wait_for("› romi's own folder")
        romi.keys("\r")
        romi.wait_for("Saves and save states go in ~/.local/share/romi.")
        assert "BIOS files go in ~/.local/share/romi/system." in romi.text()
        romi.keys("\r")
        romi.wait_for("Systems")
        config = jsonc(machine.config_path.read_text())
        assert config["bios_dirs"] == ["~/.local/share/romi/system"]
        assert config["saves_dir"] == "~/.local/share/romi/saves"
        assert (machine.home / ".local/share/romi/system").is_dir()
    finally:
        romi.close()


def test_first_run_doesnt_ask_about_retroarch_without_it(machine):
    fresh(machine)
    no_retroarch(machine)
    shutil.rmtree(machine.home / ".config/retroarch", ignore_errors=True)
    env = dict(machine.env, PATH=str(machine.root / "bin"))
    romi = Romi(types.SimpleNamespace(env=env))
    try:
        romi.wait_for("Press any key to get started")
        romi.keys(" ")
        romi.wait_for("Where are your ROMs?")
        romi.keys("\r")
        romi.wait_for("Here's what romi found")
        assert "BIOS files go in ~/.local/share/romi/system." in romi.text()
        romi.keys("\r")
        # The launcher's first screen depends on what cores this machine has.
        romi.wait_for(lambda: machine.config_path.exists())
        assert jsonc(machine.config_path.read_text())["saves_dir"] == "~/.local/share/romi/saves"
    finally:
        romi.close()


def test_first_run_goes_by_the_bios_folder_in_retroarchs_settings(machine):
    settings = machine.home / ".config/retroarch/retroarch.cfg"
    settings.parent.mkdir(parents=True)
    settings.write_text('system_directory = "~/my-bios"\n')
    romi = first_run(machine)
    try:
        romi.wait_for("Press any key to get started")
        romi.keys(" ")
        romi.wait_for("Where are your ROMs?")
        romi.keys("\r")
        romi.wait_for("BIOS files in ~/my-bios")
        romi.keys("\r")
        romi.wait_for("BIOS files go in ~/my-bios.")
        romi.keys("\r")
        romi.wait_for("Systems")
        assert jsonc(machine.config_path.read_text())["bios_dirs"] == ["~/my-bios"]
        assert (machine.home / "my-bios").is_dir()
    finally:
        romi.close()


def test_the_title_screen_only_comes_back_if_its_turned_on(sandbox):
    romi = Romi(sandbox)
    try:
        romi.wait_for("Systems")
        assert "Press any key" not in romi.text()
    finally:
        romi.close()
    config = json.loads(json.dumps(CONFIG))
    config["title_screen"] = True
    sandbox.write_config(config)
    romi = Romi(sandbox)
    try:
        romi.wait_for("Press any key")
        assert "Systems" not in romi.text()
        romi.keys(" ")
        romi.wait_for("Systems")
        assert "Press any key" not in romi.text()
    finally:
        romi.close()


def test_init_counts_match_what_romi_lists_for_a_messy_tree(machine):
    """Wrapper folders, names with spaces, saves beside ROMs and games in subfolders."""
    (machine.home / "cores/mupen64plus_next_libretro.so").touch()
    roms = machine.home / "Messy"
    for path in [
        "by-system/GBA/Golden Sun (USA).gba",
        "by-system/GBA/Mother 3 (Japan).gba",
        "by-system/GBA/Hacks/Cool Hack.gba",
        "by-system/Nintendo 64/Mario Kart 64 (USA).z64",
        "by-system/Nintendo 64/Mario Kart 64 (USA).sav",
        "odd/Some Hack.gba",
        "odd/Some Hack.sav",
        "odd/Other Hack.gba",
        "odd/Other Hack.sav",
    ]:
        (roms / path).parent.mkdir(parents=True, exist_ok=True)
        (roms / path).touch()
    fresh(machine)
    out = init(machine, "--roms", str(roms), "--yes")
    assert out.returncode == 0, out.stderr
    assert "not a system romi knows" not in out.stdout + out.stderr
    promised = int(re.search(r"Done: (\d+) games?", out.stdout).group(1))
    games = json.loads(romi(machine, "list", "--json").stdout)
    assert len(games) == promised == 6
    assert {g["system"] for g in games} == {"gba", "n64", "odd"}
    assert "Cool Hack" in {g["name"] for g in games}


def test_check_has_nothing_to_add_after_init_on_a_messy_tree(machine):
    roms = machine.home / "Messy"
    for path in ["by-system/GBA/Golden Sun (USA).gba", "by-system/GBA/Hacks/Cool Hack.gba", "odd/A.gba", "odd/A.sav"]:
        (roms / path).parent.mkdir(parents=True, exist_ok=True)
        (roms / path).touch()
    fresh(machine)
    assert init(machine, "--roms", str(roms), "--yes").returncode == 0
    out = romi(machine, "check")
    assert "romi init" not in out.stdout + out.stderr, out.stdout + out.stderr


def test_check_warns_about_ghosttys_blur(sandbox):
    sandbox.extra_env.update(TERM="xterm-ghostty", TERM_PROGRAM="ghostty")
    warning = "background-blur in Ghostty's config adds lag to games"
    assert warning not in check(sandbox)
    (sandbox.home / ".config/ghostty").mkdir(parents=True)
    (sandbox.home / ".config/ghostty/config").write_text("background-blur = 20\n")
    assert warning in check(sandbox)
    (sandbox.home / ".config/ghostty/config").write_text("background-blur = false\n")
    assert warning not in check(sandbox)

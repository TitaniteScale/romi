"""`romi run`: finding the core and refusing what it can't run. Running a real core needs a ROM, so
that's checked by hand."""

import subprocess

from drive import binary


def run_cli(sandbox, *args):
    return subprocess.run([binary(), *args], env=sandbox.env, capture_output=True, text=True, timeout=10)


def test_a_game_without_a_libretro_core_asks_for_one(sandbox):
    out = run_cli(sandbox, "run", "advance wars")
    assert out.returncode == 64
    assert "doesn't run through a libretro core; pass --core" in out.stderr


def test_a_missing_core_is_unavailable(sandbox):
    out = run_cli(sandbox, "run", "advance wars", "--core", str(sandbox.home / "nope_libretro.so"))
    assert out.returncode == 69
    assert "no core at" in out.stderr


def test_a_library_that_isnt_a_core_is_refused(sandbox):
    not_a_core = sandbox.home / "fake_libretro.so"
    not_a_core.write_bytes(b"not an ELF file")
    out = run_cli(sandbox, "run", "advance wars", "--core", str(not_a_core))
    assert out.returncode == 1
    assert "can't load" in out.stderr


def test_bad_input_scripts_name_the_line(sandbox):
    script = sandbox.home / "input.txt"
    script.write_text("10 a\n20 jump\n")
    core = sandbox.home / "fake_libretro.so"
    core.write_bytes(b"")
    out = run_cli(sandbox, "run", "advance wars", "--core", str(core), "--input", str(script))
    assert out.returncode == 64
    assert 'line 2: no button called "jump"' in out.stderr

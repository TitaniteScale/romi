"""`romi run` and playing in romi: finding the core and refusing what it can't run. Running a real
core needs a ROM, so that's checked by hand."""

import json
import subprocess

from drive import CONFIG, binary


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


CRASHING_CORE = r"""
struct info { const char *name, *version, *extensions; _Bool need_fullpath, block_extract; };
unsigned retro_api_version(void) { return 1; }
void retro_get_system_info(struct info *i) { i->name = "Crashy"; i->version = "1"; i->extensions = "gba"; i->need_fullpath = 1; }
void retro_set_environment(void *f) {}
void retro_set_video_refresh(void *f) {}
void retro_set_audio_sample(void *f) {}
void retro_set_audio_sample_batch(void *f) {}
void retro_set_input_poll(void *f) {}
void retro_set_input_state(void *f) {}
void retro_init(void) {}
void retro_deinit(void) {}
void retro_run(void) {}
void retro_unload_game(void) {}
void retro_get_system_av_info(void *av) {}
_Bool retro_load_game(const void *game) { *(volatile int *)0 = 0; return 1; }
"""


def test_a_core_that_crashes_while_loading_leaves_the_tui_running(start, sandbox):
    # Like Flycast, which only draws with OpenGL or Vulkan and crashes when romi loads it.
    source = sandbox.home / "crashy.c"
    source.write_text(CRASHING_CORE)
    core = sandbox.home / "crashy_libretro.so"
    subprocess.run(["cc", "-shared", "-fPIC", "-o", str(core), str(source)], check=True)
    config = json.loads(json.dumps(CONFIG))
    config["systems"][0]["command"] = f"true -L {core} {{rom}}"
    sandbox.write_config(config)

    romi = start()
    romi.keys("o")
    romi.wait_for("Play in romi")
    romi.keys("\r")
    romi.wait_for("crashed while starting", timeout=10)
    assert romi.exited is None

"""Core options set in romi: for a system in the top-level "core_options", for a game in its
override's, over the ones RetroArch has for the core, and the game's over the system's."""

import json
import subprocess

from drive import CONFIG, binary

# A core that asks for the option "demo_option" and writes what it's told to SEEN, or "unset".
OPTION_CORE = r"""
#include <stdio.h>
#include <string.h>
struct info { const char *name, *version, *extensions; _Bool need_fullpath, block_extract; };
struct av { unsigned w, h, max_w, max_h; float aspect; double fps, rate; };
struct var { const char *key, *value; };
typedef void (*video_fn)(const void *, unsigned, unsigned, unsigned long);
typedef unsigned long (*batch_fn)(const short *, unsigned long);
typedef _Bool (*env_fn)(unsigned, void *);
static video_fn video;
static batch_fn batch;
static env_fn env;
static unsigned short pixels[160 * 144];
static short silence[2 * 546];
unsigned retro_api_version(void) { return 1; }
void retro_get_system_info(struct info *i) { i->name = "Optioned"; i->version = "1"; i->extensions = "gb"; i->need_fullpath = 1; }
void retro_get_system_av_info(struct av *a) { a->w = a->max_w = 160; a->h = a->max_h = 144; a->aspect = 0; a->fps = 60; a->rate = 32760; }
void retro_set_environment(env_fn f) { env = f; }
void retro_set_video_refresh(video_fn f) { video = f; }
void retro_set_audio_sample(void *f) {}
void retro_set_audio_sample_batch(batch_fn f) { batch = f; }
void retro_set_input_poll(void *f) {}
void retro_set_input_state(void *f) {}
void retro_init(void) {}
void retro_deinit(void) {}
void retro_run(void) {
    struct var v = { "demo_option", 0 };
    const char *got = env(15, &v) && v.value ? v.value : "unset";
    FILE *f = fopen("SEEN" ".tmp", "w");
    fprintf(f, "%s", got);
    fclose(f);
    rename("SEEN" ".tmp", "SEEN");
    video(pixels, 160, 144, 320);
    batch(silence, 546);
}
void retro_unload_game(void) {}
void *retro_get_memory_data(unsigned id) { return 0; }
unsigned long retro_get_memory_size(unsigned id) { return 0; }
_Bool retro_load_game(const void *game) { return 1; }
"""

KEY = "gb:Pocket Lantern (USA).gb"


def option_seen(sandbox, **settings):
    """What the core is told "demo_option" is, with the config having these settings."""
    seen = sandbox.home / "seen.txt"
    (sandbox.home / "optioned.c").write_text(OPTION_CORE.replace('"SEEN"', f'"{seen}"'))
    core = sandbox.home / "optioned_libretro.so"
    subprocess.run(["cc", "-shared", "-fPIC", "-o", str(core), str(sandbox.home / "optioned.c")], check=True)
    (sandbox.home / "ROMs/gb").mkdir(parents=True, exist_ok=True)
    (sandbox.home / "ROMs/gb/Pocket Lantern (USA).gb").touch()
    config = json.loads(json.dumps(CONFIG))
    config["systems"] = [{"id": "gb", "name": "Game Boy", "extensions": ["gb"], "command": f"true -L {core} {{rom}}"}]
    config.update(settings)
    sandbox.write_config(config)
    out = subprocess.run(
        [binary(), "run", "pocket lantern", "--frames", "3"], env=sandbox.env, capture_output=True, text=True, timeout=30
    )
    assert out.returncode == 0, out.stderr
    return seen.read_text()


def retroarch_has(sandbox, value):
    folder = sandbox.home / ".config/retroarch/config/Optioned"
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "Optioned.opt").write_text(f'demo_option = "{value}"\n')


def test_a_core_asks_for_an_option_nobody_set_and_is_told_nothing(sandbox):
    assert option_seen(sandbox) == "unset"


def test_a_systems_core_options_go_over_retroarchs(sandbox):
    retroarch_has(sandbox, "retroarch")
    assert option_seen(sandbox) == "retroarch"
    assert option_seen(sandbox, core_options={"gb": {"demo_option": "system"}}) == "system"
    assert option_seen(sandbox, core_options={"nes": {"demo_option": "other"}}) == "retroarch", "another system's"


def test_a_games_core_options_go_over_its_systems(sandbox):
    retroarch_has(sandbox, "retroarch")
    game = {KEY: {"core_options": {"demo_option": "game"}}}
    assert option_seen(sandbox, overrides=game) == "game"
    assert option_seen(sandbox, overrides=game, core_options={"gb": {"demo_option": "system"}}) == "game"

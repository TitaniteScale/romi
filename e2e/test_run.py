"""`romi run` and playing in romi: finding the core and refusing what it can't run, and the launch
animation before a game. Running a real core needs a ROM, so that's checked by hand; the animation
is checked with a made-up core that draws a blank Game Boy screen."""

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
void *retro_get_memory_data(unsigned id) { return 0; }
unsigned long retro_get_memory_size(unsigned id) { return 0; }
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


# Writes the controller type plugged into port 1 and the buttons it sees held there, as a bitmask
# in libretro's order, to SEEN after every frame.
PAD_CORE = r"""
#include <stdio.h>
struct info { const char *name, *version, *extensions; _Bool need_fullpath, block_extract; };
struct av { unsigned w, h, max_w, max_h; float aspect; double fps, rate; };
typedef short (*state_fn)(unsigned, unsigned, unsigned, unsigned);
static state_fn state;
static unsigned device;
unsigned retro_api_version(void) { return 1; }
void retro_get_system_info(struct info *i) { i->name = "Pad Reader"; i->version = "1"; i->extensions = "gba"; i->need_fullpath = 1; }
void retro_get_system_av_info(struct av *a) { a->w = a->max_w = 240; a->h = a->max_h = 160; a->aspect = 0; a->fps = 60; a->rate = 32768; }
void retro_set_environment(void *f) {}
void retro_set_video_refresh(void *f) {}
void retro_set_audio_sample(void *f) {}
void retro_set_audio_sample_batch(void *f) {}
void retro_set_input_poll(void *f) {}
void retro_set_input_state(state_fn f) { state = f; }
void retro_set_controller_port_device(unsigned port, unsigned d) { if (port == 0) device = d; }
void retro_init(void) {}
void retro_deinit(void) {}
void retro_run(void) {
    unsigned held = 0;
    for (unsigned id = 0; id < 16; id++) held |= (state(0, 1, 0, id) != 0) << id;
    FILE *f = fopen("SEEN", "w");
    fprintf(f, "%u %u\n", device, held);
    fclose(f);
}
void retro_unload_game(void) {}
void *retro_get_memory_data(unsigned id) { return 0; }
unsigned long retro_get_memory_size(unsigned id) { return 0; }
_Bool retro_load_game(const void *game) { return 1; }
"""


def test_runs_use_retroarchs_remap_for_the_game(sandbox):
    seen = sandbox.home / "seen.txt"
    source = sandbox.home / "pad.c"
    source.write_text(PAD_CORE.replace('"SEEN"', f'"{seen}"'))
    core = sandbox.home / "pad_libretro.so"
    subprocess.run(["cc", "-shared", "-fPIC", "-o", str(core), str(source)], check=True)
    script = sandbox.home / "input.txt"
    script.write_text("0 a start\n")

    def run():
        out = run_cli(sandbox, "run", "advance wars", "--core", str(core), "--frames", "2", "--input", str(script))
        assert out.returncode == 0, out.stderr
        device, held = map(int, seen.read_text().split())
        return device, held

    a, b, start = 1 << 8, 1 << 0, 1 << 3
    assert run() == (1, a | start), "a pad, with the buttons as pressed"
    remaps = sandbox.home / ".config/retroarch/config/remaps/Pad Reader"
    remaps.mkdir(parents=True)
    # A and B swapped, and a DualShock plugged in.
    (remaps / "Advance Wars (USA).rmp").write_text(
        'input_libretro_device_p1 = "517"\ninput_player1_btn_a = "0"\ninput_player1_btn_b = "8"\n'
    )
    assert run() == (517, b | start)


# Draws a blank 160x144 screen, with a frame of silence, 60 times a second. Its game saves once,
# on its first frame: one more than the count it found in its 8 KB of save RAM.
DRAWING_CORE = r"""
#include <string.h>
struct info { const char *name, *version, *extensions; _Bool need_fullpath, block_extract; };
struct av { unsigned w, h, max_w, max_h; float aspect; double fps, rate; };
typedef void (*video_fn)(const void *, unsigned, unsigned, unsigned long);
typedef unsigned long (*batch_fn)(const short *, unsigned long);
static video_fn video;
static batch_fn batch;
static unsigned short pixels[160 * 144];
static short silence[2 * 546];
static unsigned char sram[8192];
static int frames;
unsigned retro_api_version(void) { return 1; }
void retro_get_system_info(struct info *i) { i->name = "Blank"; i->version = "1"; i->extensions = "gb"; i->need_fullpath = 1; }
void retro_get_system_av_info(struct av *a) { a->w = a->max_w = 160; a->h = a->max_h = 144; a->aspect = 0; a->fps = 60; a->rate = 32760; }
void retro_set_environment(void *f) {}
void retro_set_video_refresh(video_fn f) { video = f; }
void retro_set_audio_sample(void *f) {}
void retro_set_audio_sample_batch(batch_fn f) { batch = f; }
void retro_set_input_poll(void *f) {}
void retro_set_input_state(void *f) {}
void retro_init(void) {}
void retro_deinit(void) {}
void retro_run(void) { if (!frames++) sram[0]++; video(pixels, 160, 144, 320); batch(silence, 546); }
void retro_unload_game(void) {}
void *retro_get_memory_data(unsigned id) { return id == 0 ? sram : 0; }
unsigned long retro_get_memory_size(unsigned id) { return id == 0 ? sizeof sram : 0; }
_Bool retro_load_game(const void *game) { return 1; }
"""

# The animation's and the game's Kitty image ids, as Kitty commands name them.
INTRO_IMAGE = b"i=%d," % 0x776972
GAME_IMAGE = b"i=%d," % 0x726F6D


def set_up_a_game_boy_game(sandbox, **settings):
    source = sandbox.home / "blank.c"
    source.write_text(DRAWING_CORE)
    core = sandbox.home / "blank_libretro.so"
    subprocess.run(["cc", "-shared", "-fPIC", "-o", str(core), str(source)], check=True)
    (sandbox.home / "ROMs/gb").mkdir(parents=True)
    (sandbox.home / "ROMs/gb/Pocket Lantern (USA).gb").touch()
    config = json.loads(json.dumps(CONFIG))
    config["volume"] = 0
    config["systems"] = [{"id": "gb", "name": "Game Boy", "extensions": ["gb"], "command": f"true -L {core} {{rom}}"}]
    config.update(settings)
    sandbox.write_config(config)


def play_in_romi(romi):
    # Waits for the game's image rather than the words under it, which ALSA's complaints about the
    # sandbox's sound devices can write over.
    seen = len(romi.output)
    romi.keys("o")
    romi.wait_for("Play in romi")
    romi.keys("\r")
    romi.wait_for(lambda: GAME_IMAGE in romi.output[seen:], timeout=10)


def play_a_game_boy_game(start, sandbox, **settings):
    set_up_a_game_boy_game(sandbox, **settings)
    romi = start()
    play_in_romi(romi)
    romi.pump(4)
    return bytes(romi.output)


def test_a_game_boy_game_starts_with_its_wireframe(start, sandbox):
    out = play_a_game_boy_game(start, sandbox)
    first_intro, first_game = out.find(INTRO_IMAGE), out.find(GAME_IMAGE)
    assert 0 <= first_intro < first_game, "the animation plays, then the game"
    assert out.count(INTRO_IMAGE) > 20, "the animation has frames of its own"
    # Its image goes once the game's has taken over.
    assert out.find(b"a=d,d=I," + INTRO_IMAGE[:-1], first_game) > first_game


def test_the_launch_animation_can_be_turned_off(start, sandbox):
    out = play_a_game_boy_game(start, sandbox, launch_animation={"enabled": False})
    assert INTRO_IMAGE not in out
    # Square corners, so plain RGB.
    assert b"f=24," in out.split(GAME_IMAGE, 1)[1][:100]


def test_round_corners_clear_the_corners_of_the_picture(start, sandbox):
    # The game's frames are RGBA, so the corners can be clear.
    out = play_a_game_boy_game(start, sandbox, launch_animation={"enabled": False}, corner_radius=4)
    assert b"f=32," in out.split(GAME_IMAGE, 1)[1][:100]



# Where RetroArch keeps the game's save, sorted by core as it does by default.
SAVE = ".config/retroarch/saves/Blank/Pocket Lantern (USA).srm"


def test_games_played_in_romi_save_where_retroarch_does_and_carry_on_from_it(start, sandbox):
    set_up_a_game_boy_game(sandbox, launch_animation={"enabled": False})
    save = sandbox.home / SAVE
    romi = start()
    for count in (1, 2):
        seen = len(romi.output)
        play_in_romi(romi)
        romi.keys("\x1b")
        romi.wait_for(lambda: b"a=d,d=I," + GAME_IMAGE[:-1] in romi.output[seen:])
        romi.pump(1)  # past the pause for buttons still held from the game
        data = save.read_bytes()
        assert len(data) == 8192
        assert data[0] == count, "the game found what it saved last time"


def test_a_save_that_cant_be_read_stops_the_game_and_is_left_alone(start, sandbox):
    set_up_a_game_boy_game(sandbox, launch_animation={"enabled": False})
    save = sandbox.home / SAVE
    save.mkdir(parents=True)
    romi = start()
    romi.keys("o")
    romi.wait_for("Play in romi")
    romi.keys("\r")
    romi.wait_for("can't read the save", timeout=10)
    assert save.is_dir()


def test_a_screen_filter_keeps_the_game_playing(start, sandbox):
    out = play_a_game_boy_game(start, sandbox, launch_animation={"enabled": False}, screen_filter="crt")
    assert out.count(GAME_IMAGE) > 20

"""`romi run` and playing in romi: finding the core and refusing what it can't run, and the launch
animation before a game. Running a real core needs a ROM, so that's checked by hand; the animation
is checked with a made-up core that draws a blank Game Boy screen."""

import json
import subprocess
import zlib

from drive import CONFIG, binary
from test_play import eventually


def run_cli(sandbox, *args):
    return subprocess.run([binary(), *args], env=sandbox.env, capture_output=True, text=True, timeout=10)


def test_a_game_without_a_libretro_core_asks_for_one(sandbox):
    # Given as a file, so only its system's command counts: a game in the library would also try
    # its other emulators, and the machine may have RetroArch with a core for it.
    out = run_cli(sandbox, "run", str(sandbox.home / "ROMs/gba/Advance Wars (USA).gba"))
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
    FILE *f = fopen("SEEN" ".tmp", "w");
    fprintf(f, "%u %u\n", device, held);
    fclose(f);
    rename("SEEN" ".tmp", "SEEN");
}
void retro_unload_game(void) {}
void *retro_get_memory_data(unsigned id) { return 0; }
unsigned long retro_get_memory_size(unsigned id) { return 0; }
_Bool retro_load_game(const void *game) { return 1; }
"""


# Counts its frames, and writes the count to SEEN after every frame. Its state is the count, and it
# takes nothing but a state of the right size.
STATE_CORE = r"""
#include <stdio.h>
struct info { const char *name, *version, *extensions; _Bool need_fullpath, block_extract; };
struct av { unsigned w, h, max_w, max_h; float aspect; double fps, rate; };
static unsigned frames;
unsigned retro_api_version(void) { return 1; }
void retro_get_system_info(struct info *i) { i->name = "Counter"; i->version = "1"; i->extensions = "gba"; i->need_fullpath = 1; }
void retro_get_system_av_info(struct av *a) { a->w = a->max_w = 240; a->h = a->max_h = 160; a->aspect = 0; a->fps = 60; a->rate = 32768; }
void retro_set_environment(void *f) {}
void retro_set_video_refresh(void *f) {}
void retro_set_audio_sample(void *f) {}
void retro_set_audio_sample_batch(void *f) {}
void retro_set_input_poll(void *f) {}
void retro_set_input_state(void *f) {}
void retro_init(void) {}
void retro_deinit(void) {}
void retro_run(void) {
    frames++;
    FILE *f = fopen("SEEN" ".tmp", "w");
    fprintf(f, "%u\n", frames);
    fclose(f);
    rename("SEEN" ".tmp", "SEEN");
}
unsigned long retro_serialize_size(void) { return STATE_SIZE; }
_Bool retro_serialize(void *data, unsigned long size) { *(unsigned *)data = frames; return 1; }
_Bool retro_unserialize(const void *data, unsigned long size) {
    if (size != sizeof frames) return 0;
    frames = *(const unsigned *)data;
    return 1;
}
void retro_unload_game(void) {}
void *retro_get_memory_data(unsigned id) { return 0; }
unsigned long retro_get_memory_size(unsigned id) { return 0; }
_Bool retro_load_game(const void *game) { return 1; }
"""


def counting_core(sandbox, state_size="sizeof frames"):
    """A made-up core, and how to run it: `run(frames, *args)` returns romi's result and the count."""
    seen = sandbox.home / "seen.txt"
    source = sandbox.home / "counter.c"
    source.write_text(STATE_CORE.replace('"SEEN"', f'"{seen}"').replace("STATE_SIZE", state_size))
    core = sandbox.home / "counter_libretro.so"
    subprocess.run(["cc", "-shared", "-fPIC", "-o", str(core), str(source)], check=True)

    def run(frames, *args):
        out = run_cli(sandbox, "run", "advance wars", "--core", str(core), "--frames", str(frames), *args)
        return out, int(seen.read_text()) if seen.exists() else None

    return run


def test_a_run_carries_on_from_a_state_another_run_saved(sandbox):
    run = counting_core(sandbox)
    state = sandbox.home / "states/game.state"
    out, count = run(10, "--save-state", str(state))
    assert out.returncode == 0, out.stderr
    assert count == 10
    assert "Saved the state" in out.stdout
    out, count = run(5, "--load-state", str(state))
    assert out.returncode == 0, out.stderr
    assert count == 15, "from where it was saved"


def test_states_are_in_retroarchs_format_and_its_are_read(sandbox):
    run = counting_core(sandbox)
    state = sandbox.home / "game.state"
    run(7, "--save-state", str(state))
    count = (7).to_bytes(4, "little")
    assert state.read_bytes() == b"RASTATE\x01MEM \x04\0\0\0" + count + b"\0\0\0\0END \0\0\0\0"

    held = (100).to_bytes(4, "little")
    blocks = b"RASTATE\x01MEM \x04\0\0\0" + held + b"\0\0\0\0END \0\0\0\0"
    packed = zlib.compress(blocks)
    rzip = b"#RZIPv\x01#" + (131072).to_bytes(4, "little") + len(blocks).to_bytes(8, "little")
    for file in [held, blocks, rzip + len(packed).to_bytes(4, "little") + packed]:
        state.write_bytes(file)
        out, count = run(1, "--load-state", str(state))
        assert out.returncode == 0, out.stderr
        assert count == 101, file


def test_a_state_that_cant_load_says_why(sandbox):
    run = counting_core(sandbox)
    state = sandbox.home / "game.state"
    out, _ = run(1, "--load-state", str(state))
    assert out.returncode != 0 and "can't read" in out.stderr, "no file"
    state.write_bytes(b"not the right size")
    out, _ = run(1, "--load-state", str(state))
    assert out.returncode != 0 and "Counter 1 couldn't load this save state" in out.stderr
    state.write_bytes(b"RASTATE\x01MEM \xFF\0\0\0")
    out, _ = run(1, "--load-state", str(state))
    assert out.returncode != 0 and "it's cut short" in out.stderr


def test_a_core_with_no_states_says_so_and_leaves_nothing_behind(sandbox):
    run = counting_core(sandbox, state_size="0")
    state = sandbox.home / "game.state"
    out, _ = run(1, "--save-state", str(state))
    assert out.returncode != 0 and "can't make a save state" in out.stderr
    assert not state.exists()


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


def test_in_a_terminal_that_cant_show_games_a_starts_the_emulator(start, sandbox):
    set_up_a_game_boy_game(sandbox, play_in_romi=True)
    romi = start(term="foot")
    romi.keys("\r")
    romi.wait_for("foot can't show games in romi, so A starts their emulators")
    assert GAME_IMAGE not in romi.output
    # Once is enough.
    romi.pump(1)
    romi.keys("\r")
    romi.wait_for("Played Pocket Lantern")


def test_a_says_why_when_it_cant_show_a_game_or_start_its_emulator(start, sandbox):
    set_up_a_game_boy_game(sandbox, play_in_romi=True)
    config = json.loads(sandbox.config_path.read_text())
    config["systems"][0]["command"] = config["systems"][0]["command"].replace("true -L", "no-such-emulator -L")
    sandbox.write_config(config)
    romi = start(term="foot")
    romi.keys("\r")
    romi.wait_for("Games only show in Ghostty or kitty, and no-such-emulator: not found")


def test_a_plays_in_romi_in_a_terminal_that_shows_games(start, sandbox):
    set_up_a_game_boy_game(sandbox, play_in_romi=True, launch_animation={"enabled": False})
    romi = start(term="xterm-ghostty")
    romi.keys("\r")
    romi.wait_for(lambda: GAME_IMAGE in romi.output, timeout=10)


def test_the_menu_can_still_try_a_terminal_that_may_not_show_games(start, sandbox):
    set_up_a_game_boy_game(sandbox)
    play_in_romi(start(term="foot"))


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


def test_with_play_in_romi_on_enter_plays_in_romi_and_the_menu_starts_the_emulator(start, sandbox):
    set_up_a_game_boy_game(sandbox, launch_animation={"enabled": False}, play_in_romi=True)
    romi = start()
    seen = len(romi.output)
    romi.keys("\r")
    romi.wait_for(lambda: GAME_IMAGE in romi.output[seen:], timeout=10)
    romi.keys("\x1b")
    romi.wait_for(lambda: b"a=d,d=I," + GAME_IMAGE[:-1] in romi.output[seen:])
    romi.pump(1)  # past the pause for buttons still held from the game
    romi.keys("o")
    romi.wait_for("Play with")
    assert "Play in romi" not in romi.text()
    romi.choose("Play with")
    romi.wait_for("Played Pocket Lantern")
    eventually(lambda: sandbox.state().get("recent", [None])[0] == "gb:Pocket Lantern (USA).gb")


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

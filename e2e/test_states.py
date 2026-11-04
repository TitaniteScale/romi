"""Save states while a game plays in romi, with a made-up core that counts its frames."""

import json
import subprocess

from drive import CONFIG
from test_run import GAME_IMAGE

# Counts its frames, and writes the count and the buttons it sees held to SEEN after every frame. Its
# state is the count and its save RAM, and its game saves every 2 seconds of play: its save RAM's
# first byte is how many have gone by. It takes nothing but a state of the right size.
CORE = r"""
#include <stdio.h>
#include <string.h>
struct info { const char *name, *version, *extensions; _Bool need_fullpath, block_extract; };
struct av { unsigned w, h, max_w, max_h; float aspect; double fps, rate; };
typedef void (*video_fn)(const void *, unsigned, unsigned, unsigned long);
typedef unsigned long (*batch_fn)(const short *, unsigned long);
typedef short (*state_fn)(unsigned, unsigned, unsigned, unsigned);
static video_fn video;
static batch_fn batch;
static state_fn state;
static unsigned short pixels[160 * 144];
static short silence[2 * 546];
static unsigned char sram[8192];
static unsigned frames;
unsigned retro_api_version(void) { return 1; }
void retro_get_system_info(struct info *i) { i->name = "Blank"; i->version = "1"; i->extensions = "gb"; i->need_fullpath = 1; }
void retro_get_system_av_info(struct av *a) { a->w = a->max_w = 160; a->h = a->max_h = 144; a->aspect = 0; a->fps = 60; a->rate = 32760; }
void retro_set_environment(void *f) {}
void retro_set_video_refresh(video_fn f) { video = f; }
void retro_set_audio_sample(void *f) {}
void retro_set_audio_sample_batch(batch_fn f) { batch = f; }
void retro_set_input_poll(void *f) {}
void retro_set_input_state(state_fn f) { state = f; }
void retro_init(void) {}
void retro_deinit(void) {}
void retro_run(void) {
    frames++;
    sram[0] = frames / 120;
    video(pixels, 160, 144, 320);
    batch(silence, 546);
    unsigned held = 0;
    for (unsigned id = 0; id < 16; id++) held |= (state(0, 1, 0, id) != 0) << id;
    FILE *f = fopen("SEEN" ".tmp", "w");
    fprintf(f, "%u %u\n", frames, held);
    fclose(f);
    rename("SEEN" ".tmp", "SEEN");
}
unsigned long retro_serialize_size(void) { return sizeof frames + sizeof sram; }
_Bool retro_serialize(void *data, unsigned long size) {
    memcpy(data, &frames, sizeof frames);
    memcpy((char *)data + sizeof frames, sram, sizeof sram);
    return 1;
}
_Bool retro_unserialize(const void *data, unsigned long size) {
    if (size != sizeof frames + sizeof sram) return 0;
    memcpy(&frames, data, sizeof frames);
    memcpy(sram, (const char *)data + sizeof frames, sizeof sram);
    return 1;
}
void retro_unload_game(void) {}
void *retro_get_memory_data(unsigned id) { return id == 0 ? sram : 0; }
unsigned long retro_get_memory_size(unsigned id) { return id == 0 ? sizeof sram : 0; }
_Bool retro_load_game(const void *game) { return 1; }
"""

F2, F3, F4, F6, F7 = "\x1bOQ", "\x1bOR", "\x1bOS", "\x1b[17~", "\x1b[18~"
STATE_NAME = "Pocket Lantern (USA).state"
BUTTON_R = 1 << 11


class Game:
    """A game playing in romi, and how to read what its core has seen."""

    def __init__(self, start, sandbox, **options):
        self.sandbox = sandbox
        self.seen = sandbox.home / "seen.txt"
        source = sandbox.home / "core.c"
        source.write_text(CORE.replace('"SEEN"', f'"{self.seen}"'))
        core = sandbox.home / "blank_libretro.so"
        subprocess.run(["cc", "-shared", "-fPIC", "-o", str(core), str(source)], check=True)
        (sandbox.home / "ROMs/gb").mkdir(parents=True, exist_ok=True)
        (sandbox.home / "ROMs/gb/Pocket Lantern (USA).gb").touch()
        config = json.loads(json.dumps(CONFIG))
        config["volume"] = 0
        config["launch_animation"] = {"enabled": False}
        config["systems"] = [
            {"id": "gb", "name": "Game Boy", "extensions": ["gb"], "command": f"true -L {core} {{rom}}"}
        ]
        sandbox.write_config(config)
        self.start, self.options = start, options
        self.romi = None
        self.states = sandbox.home / ".local/share/romi/states/Blank"

    def play(self):
        self.romi = self.start(**self.options)
        seen = len(self.romi.output)
        self.romi.keys("o")
        self.romi.wait_for("Play in romi")
        self.romi.keys("\r")
        self.romi.wait_for(lambda: GAME_IMAGE in self.romi.output[seen:], timeout=10)
        return self.romi

    def read(self):
        for _ in range(100):
            text = self.seen.read_text() if self.seen.exists() else ""
            if text.strip():
                frames, held = text.split()
                return int(frames), int(held)
            self.romi.pump(0.02)
        raise AssertionError("the core never ran")

    def frames(self):
        return self.read()[0]

    def wait_until_frame(self, frame):
        self.romi.wait_for(lambda: self.frames() >= frame, timeout=20)
        return self.frames()

    def says(self, words):
        self.romi.wait_for(lambda: words.encode() in bytes(self.romi.output) or words in self.romi.text(), timeout=10)


def test_a_state_saved_while_playing_loads_and_goes_in_romis_own_folder(start, sandbox):
    game = Game(start, sandbox)
    romi = game.play()
    saved_at = game.wait_until_frame(150)
    romi.send(F2)
    game.says("Saved a state to slot 0")
    file = game.states / STATE_NAME
    assert file.read_bytes().startswith(b"#RZIPv\x01#"), "compressed, as RetroArch's are"
    assert not (sandbox.home / ".config/retroarch/states").exists()
    later = game.wait_until_frame(saved_at + 120)
    romi.send(F4)
    game.says("Loaded slot 0")
    romi.wait_for(lambda: game.frames() < later, timeout=5)
    assert game.frames() >= saved_at, "from where it was saved"


def test_states_wait_until_a_game_has_got_going(start, sandbox):
    game = Game(start, sandbox)
    romi = game.play()
    romi.send(F2)
    game.says("only just started")
    assert not game.states.exists()


def test_slots_can_be_empty_and_the_game_remembers_which_it_was_on(start, sandbox):
    game = Game(start, sandbox)
    romi = game.play()
    game.wait_until_frame(150)
    romi.send(F4)
    game.says("Slot 0 is empty")
    romi.send(F7)
    game.says("Slot 1: empty")
    romi.send(F2)
    game.says("Saved a state to slot 1")
    assert (game.states / (STATE_NAME + "1")).exists()
    assert not (game.states / STATE_NAME).exists()
    romi.send(F6)
    game.says("Slot 0: empty")
    romi.send(F7)
    game.says("Slot 1: saved just now")
    romi.close()
    state = json.loads((sandbox.home / ".local/state/romi/state.json").read_text())
    assert list(state["state_slots"].values()) == [1]
    romi = game.play()
    game.wait_until_frame(150)
    romi.send(F2)
    game.says("Saved a state to slot 1")


def test_a_load_can_be_undone_and_the_undo_undone(start, sandbox):
    game = Game(start, sandbox)
    romi = game.play()
    game.wait_until_frame(150)
    romi.send(F3)
    game.says("There's no load to undo")
    romi.send(F2)
    game.says("Saved a state to slot 0")
    saved_at = game.frames()
    later = game.wait_until_frame(saved_at + 180)
    romi.send(F4)
    game.says("Loaded slot 0")
    romi.wait_for(lambda: game.frames() < later, timeout=5)
    romi.send(F3)
    game.says("Went back to before loading")
    romi.wait_for(lambda: game.frames() >= later, timeout=5)


def test_loading_a_state_that_takes_the_games_save_back_keeps_a_copy(start, sandbox):
    saves = sandbox.home / ".config/retroarch/saves/Blank"
    game = Game(start, sandbox)
    romi = game.play()
    game.wait_until_frame(150)
    romi.send(F2)
    game.says("Saved a state to slot 0")
    save = saves / "Pocket Lantern (USA).srm"
    # The game saves again, and that reaches the file within a second.
    game.wait_until_frame(260)
    romi.wait_for(lambda: save.exists() and save.read_bytes()[0] >= 2, timeout=5)
    newest = save.read_bytes()
    romi.send(F4)
    game.says("also took back the game's save")
    copies = list((game.states / "saves before loading").iterdir())
    assert len(copies) == 1
    assert copies[0].read_bytes()[0] >= newest[0], "the save from before the load"
    romi.wait_for(lambda: save.read_bytes()[0] < copies[0].read_bytes()[0], timeout=5)


def test_a_state_that_wont_load_says_so_and_the_game_carries_on(start, sandbox):
    game = Game(start, sandbox)
    game.states.mkdir(parents=True)
    (game.states / STATE_NAME).write_bytes(b"not the right size")
    romi = game.play()
    before = game.wait_until_frame(150)
    romi.send(F4)
    game.says("couldn't load this save state")
    romi.pump(0.3)
    assert game.frames() > before
    (game.states / STATE_NAME).write_bytes(b"RASTATE\x01MEM \xff\0\0\0")
    romi.send(F4)
    game.says("it's cut short")


def test_a_state_being_written_is_finished_when_the_game_stops(start, sandbox):
    game = Game(start, sandbox)
    romi = game.play()
    game.wait_until_frame(150)
    romi.send(F2)
    romi.keys("\x1b")
    romi.wait_for(lambda: (game.states / STATE_NAME).exists(), timeout=5)


def test_the_line_under_the_game_says_how_to_save_when_it_fits(start, sandbox):
    game = Game(start, sandbox, cols=160)
    romi = game.play()
    game.says("Select+R saves")


def test_select_and_a_shoulder_button_save_and_load_and_dont_reach_the_game(start, sandbox, pad):
    game = Game(start, sandbox, pad=pad)
    # The virtual controller first, ahead of any real one plugged into the machine.
    sandbox.write_state({"players": ["romi test pad"]})
    romi = game.play()
    saved_at = game.wait_until_frame(150)
    pad.down("SELECT")
    romi.pump(0.1)
    pad.down("TR")
    romi.pump(0.2)
    assert not game.read()[1] & BUTTON_R, "the game doesn't see R"
    pad.up("TR")
    pad.up("SELECT")
    game.says("Saved a state to slot 0")
    later = game.wait_until_frame(saved_at + 180)
    # Alone, it's the game's.
    pad.down("TR")
    romi.wait_for(lambda: game.read()[1] & BUTTON_R, timeout=2)
    pad.up("TR")
    pad.down("SELECT")
    romi.pump(0.1)
    pad.down("TL")
    romi.pump(0.2)
    pad.up("TL")
    pad.up("SELECT")
    game.says("Loaded slot 0")
    romi.wait_for(lambda: game.frames() < later, timeout=5)
    pad.down("SELECT")
    romi.pump(0.1)
    pad.down("RIGHT")
    romi.pump(0.2)
    pad.up("RIGHT")
    pad.up("SELECT")
    game.says("Slot 1: empty")
    pad.down("SELECT")
    romi.pump(0.1)
    pad.down("DOWN")
    romi.pump(0.2)
    pad.up("DOWN")
    pad.up("SELECT")
    game.says("Went back to before loading")


def test_a_config_can_keep_in_game_saves_out_of_retroarchs_folder(start, sandbox):
    game = Game(start, sandbox)
    config = json.loads(sandbox.config_path.read_text())
    config["saves_dir"] = "~/my-saves"
    sandbox.write_config(config)
    game.play()
    save = sandbox.home / "my-saves/Blank/Pocket Lantern (USA).srm"
    game.romi.wait_for(save.exists, timeout=10)
    assert not (sandbox.home / ".config/retroarch").exists()

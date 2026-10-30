"""RetroAchievements in games played in romi: logging in, and achievements unlocking as a game runs,
against a stand-in for retroachievements.org (fake_ra.py) and a made-up core whose memory counts
frames."""

import json
import os
import subprocess

import pytest

from drive import CONFIG, binary
from fake_ra import PASSWORD, TOKEN, USER, FakeRetroAchievements

# An NES with 2 KB of RAM whose first byte counts frames, wrapping round every 256, and a blank
# 256x240 picture with a frame of silence.
COUNTING_CORE = r"""
struct info { const char *name, *version, *extensions; _Bool need_fullpath, block_extract; };
struct av { unsigned w, h, max_w, max_h; float aspect; double fps, rate; };
typedef void (*video_fn)(const void *, unsigned, unsigned, unsigned long);
typedef unsigned long (*batch_fn)(const short *, unsigned long);
static video_fn video;
static batch_fn batch;
static unsigned short pixels[256 * 240];
static short silence[2 * 735];
static unsigned char ram[2048];
unsigned retro_api_version(void) { return 1; }
void retro_get_system_info(struct info *i) { i->name = "Counter"; i->version = "1"; i->extensions = "nes"; i->need_fullpath = 1; }
void retro_get_system_av_info(struct av *a) { a->w = a->max_w = 256; a->h = a->max_h = 240; a->aspect = 0; a->fps = 60; a->rate = 44100; }
void retro_set_environment(void *f) {}
void retro_set_video_refresh(video_fn f) { video = f; }
void retro_set_audio_sample(void *f) {}
void retro_set_audio_sample_batch(batch_fn f) { batch = f; }
void retro_set_input_poll(void *f) {}
void retro_set_input_state(void *f) {}
void retro_init(void) {}
void retro_deinit(void) {}
void retro_run(void) { ram[0]++; video(pixels, 256, 240, 512); batch(silence, 735); }
void retro_unload_game(void) {}
void *retro_get_memory_data(unsigned id) { return id == 2 ? ram : 0; }
unsigned long retro_get_memory_size(unsigned id) { return id == 2 ? sizeof ram : 0; }
_Bool retro_load_game(const void *game) { return 1; }
"""

# The same, but like mGBA: its memory is only in the memory map, given on its first frame.
LATE_MAP_CORE = COUNTING_CORE.replace(
    "void retro_set_environment(void *f) {}",
    """
struct desc { unsigned long long flags; void *ptr; unsigned long offset, start, select, disconnect, len; const char *addrspace; };
struct map { const struct desc *descriptors; unsigned num_descriptors; };
typedef _Bool (*env_fn)(unsigned, void *);
static env_fn env;
static int mapped;
void retro_set_environment(env_fn f) { env = f; }
""",
).replace(
    "void retro_run(void) { ram[0]++;",
    """void retro_run(void) {
  if (!mapped++) {
    static struct desc d = { 0, ram, 0, 0, 0, 0, sizeof ram, 0 };
    struct map m = { &d, 1 };
    env(36 | 0x10000, &m);
  }
  ram[0]++;""",
).replace("return id == 2 ? ram : 0;", "return 0;").replace("return id == 2 ? sizeof ram : 0;", "return 0;")

LOGIN = ".local/state/romi/retroachievements-login.json"

# The fake server answers on a thread of this process, which Python warns about when the pty is
# forked; the child only execs romi, so it's safe.
pytestmark = pytest.mark.filterwarnings("ignore:This process .* is multi-threaded:DeprecationWarning")


@pytest.fixture
def server():
    fake = FakeRetroAchievements([(5501, "Count to 200", 5, "0xH0000=200")])
    yield fake
    fake.close()


def run_cli(sandbox, *args, stdin=""):
    return subprocess.run([binary(), *args], env=sandbox.env, input=stdin, capture_output=True, text=True,
                          timeout=30)


def set_up_an_nes_game(sandbox, server, core_source=COUNTING_CORE, animation=False):
    source = sandbox.home / "counter.c"
    source.write_text(core_source)
    core = sandbox.home / "counter_libretro.so"
    subprocess.run(["cc", "-shared", "-fPIC", "-o", str(core), str(source)], check=True)
    (sandbox.home / "ROMs/nes").mkdir(parents=True)
    (sandbox.home / "ROMs/nes/Pocket Lantern (USA).nes").write_bytes(b"NES\x1a" + bytes(12) + b"lantern" * 64)
    config = json.loads(json.dumps(CONFIG))
    config["volume"] = 0
    config["launch_animation"] = {"enabled": animation}
    config["systems"] = [{"id": "nes", "name": "NES", "extensions": ["nes"], "command": f"true -L {core} {{rom}}"}]
    sandbox.write_config(config)
    sandbox.extra_env["ROMI_RA_HOST"] = server.url


def wait_for_unlock(romi, server):
    """Waits for the fake server to be sent an unlock, then for romi to draw its notice. romi's
    output is searched rather than the screen, which ALSA's complaints about the sandbox's sound
    devices can scroll and garble."""
    romi.wait_for(lambda: server.calls("awardachievement"), timeout=20)
    romi.wait_for(lambda: b"Achievement unlocked" in romi.output, timeout=5)


def log_in(sandbox):
    out = run_cli(sandbox, "achievements", "login", USER, stdin=PASSWORD + "\n")
    assert out.returncode == 0, out.stderr
    return out


def test_logging_in_keeps_a_token_and_never_the_password(sandbox, server):
    sandbox.extra_env["ROMI_RA_HOST"] = server.url
    out = log_in(sandbox)
    assert f"Logged in to RetroAchievements as {USER}" in out.stdout
    saved = sandbox.home / LOGIN
    assert json.loads(saved.read_text()) == {"username": USER, "token": TOKEN}
    assert PASSWORD not in saved.read_text()
    assert os.stat(saved).st_mode & 0o777 == 0o600, "only you can read it"
    assert server.calls("login2")[0]["user-agent"].startswith("romi/")

    out = run_cli(sandbox, "achievements", "logout")
    assert out.returncode == 0 and not saved.exists()


def test_a_wrong_password_says_so_and_saves_nothing(sandbox, server):
    sandbox.extra_env["ROMI_RA_HOST"] = server.url
    out = run_cli(sandbox, "achievements", "login", USER, stdin="wrong\n")
    assert out.returncode == 69
    assert "Invalid User/Password" in out.stderr
    assert not (sandbox.home / LOGIN).exists()


def test_achievements_unlock_while_a_game_plays_in_romi(start, sandbox, server):
    set_up_an_nes_game(sandbox, server)
    log_in(sandbox)
    romi = start()
    romi.keys("o")
    romi.wait_for("Play in romi")
    romi.keys("\r")
    romi.wait_for(lambda: b"0 of 1 achievement unlocked" in romi.output, timeout=10)
    # The counter reaches 200 within about four seconds of play.
    wait_for_unlock(romi, server)
    assert b"Count to 200" in romi.output
    award = server.calls("awardachievement")
    assert [(a["a"], a["h"]) for a in award] == [("5501", "0")], "sent once, softcore"
    # The game was asked for by the ROM's hash, which skips the iNES header.
    session = server.calls("startsession")[0]
    assert session["g"] == "1234" and len(server.calls("achievementsets")[0]["m"]) == 32


# The badge's Kitty image id, as Kitty commands name it ("ach").
BADGE_IMAGE = b"i=%d," % 0x616368


def test_an_unlock_shows_its_badge_which_is_cached(start, sandbox, server):
    set_up_an_nes_game(sandbox, server)
    log_in(sandbox)
    romi = start()
    romi.keys("o")
    romi.wait_for("Play in romi")
    romi.keys("\r")
    wait_for_unlock(romi, server)
    assert server.badges == ["/Badge/5501.png"]
    romi.wait_for(lambda: BADGE_IMAGE in romi.output, timeout=5)
    # RGBA, so the badge's cells are clear around a square badge.
    assert b"f=32," in romi.output.split(BADGE_IMAGE, 1)[1][:40]
    # Every image cell says where it is, so the game's cells after the notice in a row don't start
    # the picture again from its left edge.
    romi.pump(0.3)
    cells = [c.data for row in romi.screen.buffer.values() for c in row.values() if c.data.startswith("\U0010EEEE")]
    assert cells and all(len(c) == 3 for c in cells), [c for c in cells if len(c) != 3][:3]
    assert (sandbox.home / ".cache/romi/ra/badges/5501.png").exists()


def test_badges_can_be_turned_off(start, sandbox, server):
    set_up_an_nes_game(sandbox, server)
    config = json.loads(sandbox.config_path.read_text())
    config["retroachievements"] = {"badges": False}
    sandbox.write_config(config)
    log_in(sandbox)
    romi = start()
    romi.keys("o")
    romi.wait_for("Play in romi")
    romi.keys("\r")
    wait_for_unlock(romi, server)
    romi.pump(1)
    assert server.badges == [] and BADGE_IMAGE not in romi.output


def test_memory_a_core_maps_on_its_first_frame_is_found(start, sandbox, server):
    # mGBA gives its memory map only once it has run a frame, after the launch animation, which
    # is when the achievements may have loaded. They were disabled for want of their memory.
    set_up_an_nes_game(sandbox, server, LATE_MAP_CORE, animation=True)
    log = sandbox.root / "ra.log"
    sandbox.extra_env["ROMI_RA_LOG"] = str(log)
    log_in(sandbox)
    romi = start()
    romi.keys("o")
    romi.wait_for("Play in romi")
    romi.keys("\r")
    wait_for_unlock(romi, server)
    assert [a["a"] for a in server.calls("awardachievement")] == ["5501"]
    assert "Disabled" not in log.read_text()


def test_a_game_retroachievements_doesnt_know_says_so(start, sandbox):
    fake = FakeRetroAchievements([(5501, "Count to 200", 5, "0xH0000=200")], known_hashes=set())
    try:
        set_up_an_nes_game(sandbox, fake)
        log_in(sandbox)
        romi = start()
        romi.keys("o")
        romi.wait_for("Play in romi")
        romi.keys("\r")
        # The status bar only redraws the letters that changed, so this is looked for on screen.
        romi.wait_for("RetroAchievements:", timeout=10)
        assert not fake.calls("awardachievement")
    finally:
        fake.close()


def test_without_a_login_games_play_without_achievements(start, sandbox, server):
    set_up_an_nes_game(sandbox, server)
    romi = start()
    romi.keys("o")
    romi.wait_for("Play in romi")
    romi.keys("\r")
    romi.wait_for(lambda: b"i=%d," % 0x726F6D in romi.output, timeout=10)
    romi.pump(1)
    assert server.requests == []

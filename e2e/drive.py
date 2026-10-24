"""Run romi in a fake terminal, send keys and controller input, and read the screen.

As a script it prints screens, which is handy for checking a TUI change by eye:

    python e2e/drive.py [--real] COLS ROWS STEP...

where STEP is `keys:<text>` (escapes like \\x1b allowed), `pad:<BUTTON>[:<hold secs>]`
(SOUTH, EAST, START, UP, DOWN, ...), `wait:<secs>` or `shot` (print the screen). romi runs in a
sandbox with a small made-up library unless `--real` is given, which uses your own config and state.

The tests in this folder import `Sandbox` and `Romi` from here. The binary is target/debug/romi
unless ROMI_BIN says otherwise. Controller input needs python-evdev and write access to /dev/uinput.
"""

import fcntl
import json
import os
import pty
import re
import select
import shutil
import struct
import sys
import tempfile
import termios
import time
from pathlib import Path

import pyte

REPO = Path(__file__).resolve().parent.parent
ALT_SCREEN = re.compile(rb"(\x1b\[\?1049[hl])")

ROMS = {
    "gba": ["Advance Wars (USA).gba", "Metroid Fusion (USA).gba"],
    "snes": ["Super Metroid (Japan, USA).sfc", "Chrono Trigger (USA).sfc"],
}

CONFIG = {
    "roms_dir": "~/ROMs",
    "art": {"online": False},
    "controller": {"enabled": False},
    "steam": {"enabled": False},
    "systems": [
        {"id": "gba", "name": "Game Boy Advance", "extensions": ["gba"], "command": "true {rom}"},
        {"id": "snes", "name": "Super Nintendo", "extensions": ["sfc"], "command": "true {rom}"},
    ],
    "games": [{"name": "Hello Script", "command": "sh -c 'echo hello from a script'", "mode": "terminal"}],
}


def binary():
    return os.environ.get("ROMI_BIN") or str(REPO / "target/debug/romi")


class Sandbox:
    """A home folder with a small library, so romi never touches your real config or state."""

    def __init__(self, root=None, config=None, roms=None):
        self.root = Path(root or tempfile.mkdtemp(prefix="romi-e2e-"))
        self.home = self.root / "home"
        for system, files in (ROMS if roms is None else roms).items():
            (self.home / "ROMs" / system).mkdir(parents=True, exist_ok=True)
            for f in files:
                (self.home / "ROMs" / system / f).touch()
        self.config_path = self.root / "config.json"
        self.write_config(CONFIG if config is None else config)

    def write_config(self, config):
        self.config_path.write_text(json.dumps(config, indent=2))

    @property
    def env(self):
        return {
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "HOME": str(self.home),
            "XDG_CONFIG_HOME": str(self.home / ".config"),
            "XDG_STATE_HOME": str(self.home / ".local/state"),
            "XDG_CACHE_HOME": str(self.home / ".cache"),
            "XDG_DATA_HOME": str(self.home / ".local/share"),
            "ROMI_CONFIG": str(self.config_path),
            "LANG": "C.UTF-8",
        }

    def state(self):
        path = self.home / ".local/state/romi/state.json"
        return json.loads(path.read_text()) if path.exists() else {}

    def cleanup(self):
        shutil.rmtree(self.root, ignore_errors=True)


class Pad:
    """A virtual gamepad. Create it before starting romi so romi finds it at startup."""

    HATS = {"UP": ("ABS_HAT0Y", -1), "DOWN": ("ABS_HAT0Y", 1), "LEFT": ("ABS_HAT0X", -1), "RIGHT": ("ABS_HAT0X", 1)}

    def __init__(self):
        from evdev import AbsInfo, UInput
        from evdev import ecodes as e

        self.e = e
        axis = AbsInfo(value=0, min=-32768, max=32767, fuzz=16, flat=128, resolution=0)
        hat = AbsInfo(value=0, min=-1, max=1, fuzz=0, flat=0, resolution=0)
        buttons = ["SOUTH", "EAST", "NORTH", "WEST", "TL", "TR", "TL2", "TR2", "SELECT", "START", "MODE"]
        caps = {
            e.EV_KEY: [getattr(e, "BTN_" + b) for b in buttons + ["THUMBL", "THUMBR"]],
            e.EV_ABS: [(e.ABS_X, axis), (e.ABS_Y, axis), (e.ABS_RX, axis), (e.ABS_RY, axis),
                       (e.ABS_HAT0X, hat), (e.ABS_HAT0Y, hat)],
        }
        self.dev = UInput(caps, name="romi test pad", vendor=0x1234, product=0x5678, version=1, bustype=e.BUS_USB)
        time.sleep(1.0)

    def down(self, name):
        e = self.e
        if name in self.HATS:
            axis, v = self.HATS[name]
            self.dev.write(e.EV_ABS, getattr(e, axis), v)
        elif name == "STICKDOWN":
            self.dev.write(e.EV_ABS, e.ABS_Y, 30000)
        else:
            self.dev.write(e.EV_KEY, getattr(e, "BTN_" + name), 1)
        self.dev.syn()

    def up(self, name):
        e = self.e
        if name in self.HATS:
            self.dev.write(e.EV_ABS, getattr(e, self.HATS[name][0]), 0)
        elif name == "STICKDOWN":
            self.dev.write(e.EV_ABS, e.ABS_Y, 0)
        else:
            self.dev.write(e.EV_KEY, getattr(e, "BTN_" + name), 0)
        self.dev.syn()

    def close(self):
        self.dev.close()


def pad_available():
    try:
        import evdev  # noqa: F401
    except ImportError:
        return False
    return os.access("/dev/uinput", os.W_OK)


class Romi:
    """romi running in a pty, with the screen kept up to date by pyte.

    `answer_queries=False` plays a terminal that ignores romi's image-support query.
    """

    def __init__(self, sandbox=None, cols=100, rows=30, args=(), pad=None, answer_queries=True):
        self.cols, self.rows = cols, rows
        self.pad = pad
        self.answer_queries = answer_queries
        self.screen = pyte.Screen(cols, rows)
        self.stream = pyte.ByteStream(self.screen)
        env = sandbox.env if sandbox else dict(os.environ)
        env["TERM"] = "xterm-256color"
        argv = [binary(), *args]
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.execve(argv[0], argv, env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.exited = None

    def pump(self, secs):
        """Reads output for `secs` seconds, answering the terminal queries romi sends."""
        end = time.time() + secs
        while time.time() < end and self.exited is None:
            r, _, _ = select.select([self.fd], [], [], 0.02)
            if not r:
                continue
            try:
                data = os.read(self.fd, 65536)
            except OSError:
                self._reap()
                return
            # Cursor position, so crossterm doesn't stall.
            if b"\x1b[6n" in data:
                os.write(self.fd, b"\x1b[1;1R")
            # Like a real terminal without graphics: cell size, device attributes, status.
            if self.answer_queries:
                if b"\x1b[16t" in data:
                    os.write(self.fd, b"\x1b[6;20;10t")
                if b"\x1b[c" in data:
                    os.write(self.fd, b"\x1b[?62;22c")
                if b"\x1b[5n" in data:
                    os.write(self.fd, b"\x1b[0n")
            # pyte has no alternate screen. Switching either way starts from a blank one, which is
            # what a real terminal shows here, since nothing ran in the main screen before romi.
            for i, part in enumerate(ALT_SCREEN.split(data)):
                if i % 2:
                    self.screen.reset()
                else:
                    self.stream.feed(part)

    def _reap(self):
        pid, status = os.waitpid(self.pid, os.WNOHANG)
        if pid == self.pid:
            self.exited = os.waitstatus_to_exitcode(status)

    def text(self):
        return "\n".join(self.lines())

    def lines(self):
        return [
            "".join(self.screen.buffer[y][x].data or " " for x in range(self.cols)).rstrip()
            for y in range(self.rows)
        ]

    def selected(self):
        """The highlighted (reverse video) text on each row, left to right."""
        out = []
        for y in range(self.rows):
            row, run = self.screen.buffer[y], ""
            for x in range(self.cols):
                if row[x].reverse:
                    run += row[x].data
                elif run:
                    out.append(run.strip())
                    run = ""
            if run:
                out.append(run.strip())
        return out

    def wait_for(self, check, timeout=5.0):
        """Pumps until `check` (a string to find on screen, or a function) is true."""
        test = (lambda: check in self.text()) if isinstance(check, str) else check
        end = time.time() + timeout
        while not test():
            if time.time() > end:
                raise AssertionError(f"timed out waiting for {check!r}; screen:\n{self.text()}")
            if self.exited is not None:
                raise AssertionError(f"romi exited ({self.exited}) waiting for {check!r}; screen:\n{self.text()}")
            self.pump(0.05)

    def keys(self, text, settle=0.15):
        for ch in text.encode():
            os.write(self.fd, bytes([ch]))
            self.pump(0.03)
        self.pump(settle)

    def press(self, button, hold=0.08, settle=0.25):
        self.pad.down(button)
        self.pump(hold)
        self.pad.up(button)
        self.pump(settle)

    def close(self):
        if self.exited is None:
            try:
                os.write(self.fd, b"q")
            except OSError:
                pass
            self.pump(0.5)
        if self.exited is None:
            try:
                os.kill(self.pid, 9)
            except ProcessLookupError:
                pass
            os.waitpid(self.pid, 0)
        os.close(self.fd)


def main(argv):
    real = "--real" in argv
    argv = [a for a in argv if a != "--real"]
    if len(argv) < 2:
        sys.exit(__doc__)
    cols, rows, steps = int(argv[0]), int(argv[1]), argv[2:]
    sandbox = None if real else Sandbox()
    pad = Pad() if any(s.startswith("pad:") for s in steps) else None
    romi = Romi(sandbox, cols, rows, pad=pad)
    try:
        romi.pump(1.5)
        for step in steps:
            if step == "shot":
                print(romi.text())
                print("-" * cols)
            elif step.startswith("wait:"):
                romi.pump(float(step[5:]))
            elif step.startswith("pad:"):
                name, _, hold = step[4:].partition(":")
                romi.press(name, hold=float(hold or 0.08))
            elif step.startswith("keys:"):
                romi.keys(step[5:].encode().decode("unicode_escape"))
            else:
                sys.exit(f"unknown step: {step}")
    finally:
        romi.close()
        if pad:
            pad.close()
        if sandbox:
            sandbox.cleanup()
    print("romi exited" if romi.exited is not None else "romi was still running")


if __name__ == "__main__":
    main(sys.argv[1:])

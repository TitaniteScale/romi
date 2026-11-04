import os
import subprocess

import pytest

from drive import REPO, Pad, Romi, Sandbox, pad_available


@pytest.fixture(scope="session", autouse=True)
def build():
    if not os.environ.get("ROMI_BIN"):
        subprocess.run(["cargo", "build", "--quiet"], cwd=REPO, check=True)


@pytest.fixture
def sandbox(tmp_path):
    return Sandbox(tmp_path)


@pytest.fixture
def start(sandbox):
    """Starts romi in the sandbox; every instance is shut down after the test."""
    running = []

    def start(**kwargs):
        romi = Romi(sandbox, **kwargs)
        running.append(romi)
        romi.wait_for("Systems")
        return romi

    yield start
    for romi in running:
        romi.close()


def real_romis():
    """The romis running outside these tests, by their command lines. Tests' own are closed by now."""
    out = subprocess.run(["pgrep", "-ax", "romi"], capture_output=True, text=True).stdout
    return [line.split(" ", 1)[1] for line in out.splitlines() if " " in line]


@pytest.fixture
def guide_pad(pad):
    """A virtual controller to press Guide on, which a real `romi guide` would act on."""
    if real_romis():
        pytest.skip("`romi guide` is running, and Guide on this virtual controller would set it off")
    return pad


@pytest.fixture
def pad():
    if not pad_available():
        pytest.skip("needs python-evdev and write access to /dev/uinput")
    # A virtual controller reaches everything reading controllers on this machine.
    if any(" guide" not in romi for romi in real_romis()):
        pytest.skip("a romi is open, which this virtual controller would press buttons in")
    pad = Pad()
    yield pad
    pad.close()

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


@pytest.fixture
def pad():
    if not pad_available():
        pytest.skip("needs python-evdev and write access to /dev/uinput")
    pad = Pad()
    yield pad
    pad.close()

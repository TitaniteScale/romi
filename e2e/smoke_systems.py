#!/usr/bin/env python3
"""Boots a game from each system in your library, headless, with `romi run`, and says which
systems play in romi: whether a core was found, the game drew something, it made sound, and how
fast it ran.

It's a smoke test, not a test of playing: a game that sits on a black screen for its first ten
seconds (a BIOS that takes its time, a game that waits for Start) reads as "blank picture", and one
that fails on its third level reads as fine. It tells you which systems to look at by hand, and
which are plainly broken. Nothing is saved and nothing is sent anywhere: runs start with an empty
save folder, and don't touch RetroAchievements.

    python3 e2e/smoke_systems.py                      # one game per system, 600 frames each
    python3 e2e/smoke_systems.py --frames 1800 --systems psx,n64
    python3 e2e/smoke_systems.py --game psx="Ape Escape" --per-system 2
    python3 e2e/smoke_systems.py --json > matrix.json

Uses the `romi` on your PATH (or --romi), the config and library it already has, and only the
standard library.
"""

import argparse
import array
import json
import struct
import subprocess
import sys
import tempfile
import time
import wave
import zlib
from pathlib import Path

# Systems that aren't ROMs a core can run.
NOT_CORES = {"steam", "pc", "shell", "vpinball"}

# A picture with fewer pixels than this (or this share of them) unlike its commonest colour is blank:
# a logo on black is a few hundred pixels, and a cursor on black a few more.
BLANK_PIXELS = 16
BLANK_SHARE = 0.0001
# Less than this share of sound samples louder than a whisper, and it's silent.
SILENT_BELOW = 0.002
# A game that's blank or silent is run again for this many times as long, in case it was slow to start.
RETRY_TIMES = 3


def romi(args, binary, timeout):
    return subprocess.run([binary, *args], capture_output=True, text=True, timeout=timeout)


def library(binary):
    out = romi(["list", "--json"], binary, 120)
    if out.returncode != 0:
        sys.exit(f"romi list failed: {out.stderr.strip()}")
    return json.loads(out.stdout)


def unfilter(raw, width, height, bpp):
    """A PNG's pixel bytes, from its filtered rows (RGB or RGBA, 8 bits, not interlaced)."""
    stride = width * bpp
    prev = bytearray(stride)
    out = bytearray()
    at = 0
    for _ in range(height):
        kind = raw[at]
        row = bytearray(raw[at + 1 : at + 1 + stride])
        at += 1 + stride
        if kind == 1:
            for i in range(bpp, stride):
                row[i] = (row[i] + row[i - bpp]) & 255
        elif kind == 2:
            for i in range(stride):
                row[i] = (row[i] + prev[i]) & 255
        elif kind == 3:
            for i in range(stride):
                left = row[i - bpp] if i >= bpp else 0
                row[i] = (row[i] + ((left + prev[i]) >> 1)) & 255
        elif kind == 4:
            for i in range(stride):
                a = row[i - bpp] if i >= bpp else 0
                b, c = prev[i], (prev[i - bpp] if i >= bpp else 0)
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                row[i] = (row[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        out += row
        prev = row
    return out


def blank(path):
    """Whether a PNG is one colour, bar a few pixels: a black frame, or a single logo's worth."""
    data = Path(path).read_bytes()
    at, idat, header = 8, b"", None
    while at < len(data):
        (length,) = struct.unpack(">I", data[at : at + 4])
        kind = data[at + 4 : at + 8]
        body = data[at + 8 : at + 8 + length]
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            idat += body
        at += 12 + length
    if header is None or header[2] != 8 or header[3] not in (2, 6) or header[6] != 0:
        return None
    width, height, bpp = header[0], header[1], 3 if header[3] == 2 else 4
    pixels = unfilter(zlib.decompress(idat), width, height, bpp)
    counts = {}
    for i in range(0, len(pixels), bpp):
        px = bytes(pixels[i : i + bpp])
        counts[px] = counts.get(px, 0) + 1
    others = width * height - max(counts.values())
    return others < max(BLANK_PIXELS, BLANK_SHARE * width * height)


def sound_share(path):
    """The share of a WAV's samples louder than a whisper."""
    try:
        with wave.open(str(path)) as wav:
            if wav.getsampwidth() != 2:
                return None
            samples = array.array("h", wav.readframes(wav.getnframes()))
    except (wave.Error, EOFError, FileNotFoundError):
        return None
    return sum(1 for s in samples if abs(s) > 16) / max(len(samples), 1)


def pick(games, per_system, wanted, only):
    """The games to try, by system: the one asked for, else the first few by name."""
    by_system = {}
    for g in games:
        if g.get("hidden") or not g.get("path") or g["system"] in NOT_CORES:
            continue
        if only and g["system"] not in only:
            continue
        by_system.setdefault(g["system"], []).append(g)
    chosen = []
    for system, found in sorted(by_system.items()):
        found.sort(key=lambda g: g["name"].lower())
        if system in wanted:
            query = wanted[system].lower()
            found = [g for g in found if query in g["name"].lower()] or found
        chosen.extend((system, g) for g in found[:per_system])
    return chosen


def run_once(game, binary, frames, limit, tmp):
    """One `romi run`: the process's outcome, or (None, what went wrong) if it didn't finish."""
    shot, wav = Path(tmp) / "last.png", Path(tmp) / "sound.wav"
    started = time.monotonic()
    try:
        out = romi(
            ["run", game["key"], "--frames", str(frames), "--json", "--screenshot", str(shot), "--wav", str(wav)],
            binary,
            limit,
        )
    except subprocess.TimeoutExpired:
        return None, {"status": "timed out", "note": f"no answer in {limit}s (a hang, or too slow for the limit)"}
    wall = round(time.monotonic() - started, 1)
    if out.returncode != 0:
        why = (out.stderr.strip().splitlines() or ["failed"])[-1].removeprefix("romi: ")
        status = {64: "no core", 69: "core missing"}.get(out.returncode, f"failed ({out.returncode})")
        return None, {"status": status, "note": why, "wall_secs": wall}
    info = json.loads(out.stdout.strip().splitlines()[-1])
    is_blank, sound = blank(shot), sound_share(wav)
    return {
        "info": info,
        "wall_secs": wall,
        "speed": round(frames / max(info["elapsed_secs"], 1e-9) / max(info["fps"], 1e-9), 1),
        "blank": is_blank,
        "silent": sound is not None and sound < SILENT_BELOW,
    }, None


def smoke(game, binary, frames, limit):
    """What happened when `game` was run: a dict that says what's notable."""
    result = {"game": game["name"], "key": game["key"], "system": game["system"]}
    with tempfile.TemporaryDirectory() as tmp:
        ran, failed = run_once(game, binary, frames, limit, tmp)
        if failed:
            return result | failed
        notes, at = [], frames
        if ran["blank"] or ran["silent"]:
            # Slow to start, or broken? Run it longer.
            again, failed = run_once(game, binary, frames * RETRY_TIMES, limit * RETRY_TIMES, tmp)
            if again and not (again["blank"] or again["silent"]):
                notes.append(f"blank or silent at {frames} frames, fine at {frames * RETRY_TIMES}")
            if again:
                ran, at = again, frames * RETRY_TIMES
            elif failed:
                notes.append(f"longer run: {failed['note']}")
        info = ran["info"]
        result.update(
            core=info["core_name"],
            size=f'{info["width"]}x{info["height"]}',
            speed=ran["speed"],
            wall_secs=ran["wall_secs"],
            picture="blank" if ran["blank"] else "drew",
            sound="silent" if ran["silent"] else "yes",
            frames=at,
        )
        if ran["blank"]:
            notes.append(f"blank or one colour after {at} frames")
        if ran["silent"]:
            notes.append(f"silent after {at} frames")
        if ran["speed"] < 1.0:
            notes.append("slower than the console")
        problems = ran["blank"] or ran["silent"] or ran["speed"] < 1.0
        result["status"] = "check" if problems else "plays"
        result["note"] = "; ".join(notes)
    return result


def table(results):
    cells = ["System", "Game", "Result", "Core", "Size", "Picture", "Sound", "Speed", "Notes"]
    rows = [cells, ["---"] * len(cells)]
    for r in results:
        speed = f'{r["speed"]}x' if "speed" in r else ""
        rows.append(
            [r["system"], r["game"], r["status"], r.get("core", ""), r.get("size", ""), r.get("picture", ""),
             r.get("sound", ""), speed, r.get("note", "")]
        )
    return "\n".join("| " + " | ".join(row) + " |" for row in rows)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--romi", default="romi", help="the romi to run [romi]")
    ap.add_argument("--frames", type=int, default=600, help="frames to run each game [600]")
    ap.add_argument("--limit", type=int, default=180, help="seconds to wait for each run before calling it hung [180]")
    ap.add_argument("--systems", help="only these system ids, separated by commas")
    ap.add_argument("--per-system", type=int, default=1, help="games to try in each system [1]")
    ap.add_argument("--game", action="append", default=[], metavar="SYSTEM=WORDS", help="the game to try in a system")
    ap.add_argument("--json", action="store_true", help="print the results as JSON instead of a table")
    args = ap.parse_args()

    wanted = dict(item.split("=", 1) for item in args.game)
    only = set(args.systems.split(",")) if args.systems else None
    todo = pick(library(args.romi), args.per_system, wanted, only)
    if not todo:
        sys.exit("no games to try")
    results = []
    for n, (system, game) in enumerate(todo, 1):
        print(f"[{n}/{len(todo)}] {system}: {game['name']}", file=sys.stderr, flush=True)
        results.append(smoke(game, args.romi, args.frames, args.limit))
    print(json.dumps(results, indent=2) if args.json else table(results))


if __name__ == "__main__":
    main()

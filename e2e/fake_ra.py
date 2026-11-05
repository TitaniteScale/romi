"""A stand-in for retroachievements.org's `dorequest.php`, the API emulators use, for romi to log in,
load a game's achievements and send unlocks to without an account. Point romi at it with
ROMI_RA_HOST. Answers are shaped like the real server's (as rcheevos' own tests have them)."""

import json
import struct
import threading
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs

USER, PASSWORD, TOKEN = "Lantern", "hunter2", "T0KEN"


def png(width, height, rgb):
    """A PNG of one colour, made without an image library."""
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    rows = b"".join(b"\0" + bytes(rgb) * width for _ in range(height))
    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")


class FakeRetroAchievements:
    def __init__(self, achievements, game_id=1234, title="Pocket Lantern", console=7, known_hashes=None, rich_presence=""):
        # Achievements as (id, title, points, condition), conditions in RetroAchievements' syntax.
        self.achievements = achievements
        self.game_id, self.title, self.console = game_id, title, console
        # Hashes that are this game; None means any.
        self.known_hashes = known_hashes
        # The game's rich presence script, in RetroAchievements' syntax.
        self.rich_presence = rich_presence
        self.requests = []
        # Badges asked for, by path.
        self.badges = []
        self.unlocked = set()
        # Answer unlocks as a server that's busy does, so they have to wait.
        self.fail_awards = False
        # Add the stand-in achievement the real server warns unknown emulators with.
        self.warn_unknown_emulator = False
        fake = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                body = self.rfile.read(int(self.headers.get("Content-Length", 0))).decode()
                params = {k: v[0] for k, v in parse_qs(body).items()}
                params["user-agent"] = self.headers.get("User-Agent", "")
                fake.requests.append(params)
                status, answer = fake.answer(params)
                data = json.dumps(answer).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def do_GET(self):
                fake.badges.append(self.path)
                data = png(64, 64, (200, 120, 40))
                self.send_response(200)
                self.send_header("Content-Type", "image/png")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def log_message(self, *args):
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()

    def calls(self, kind):
        return [r for r in self.requests if r.get("r") == kind]

    def answer(self, p):
        r = p.get("r")
        signed_in = p.get("u") == USER and p.get("t") == TOKEN
        if r == "systemgames":
            games = [{"ID": self.game_id, "Title": self.title, "NumAchievements": len(self.achievements),
                      "Points": sum(a[2] for a in self.achievements), "SupportedHashes": sorted(self.known_hashes or [])},
                     {"ID": 99, "Title": "Nothing To Earn", "NumAchievements": 0, "Points": 0, "SupportedHashes": []}]
            return 200, {"Success": True, "Response": games if int(p["s"]) == self.console else []}
        if r == "login2":
            if p.get("u") == USER and (p.get("p") == PASSWORD or p.get("t") == TOKEN):
                return 200, {"Success": True, "User": USER, "Token": TOKEN, "Score": 0, "SoftcoreScore": 0,
                             "Messages": 0, "Permissions": 1, "AccountType": "Registered"}
            return 401, {"Success": False, "Error": "Invalid User/Password combination. Please try again",
                         "Code": "invalid_credentials"}
        if not signed_in:
            return 401, {"Success": False, "Error": "Invalid user/token", "Code": "invalid_credentials"}
        if r == "achievementsets":
            by_id = p.get("g") == str(self.game_id)
            if not by_id and self.known_hashes is not None and p.get("m") not in self.known_hashes:
                return 200, {"Success": True, "GameId": 0, "Title": "Unknown Game", "ConsoleId": 0, "ImageIconUrl": "", "RichPresenceGameId": 0, "RichPresencePatch": "", "Sets": []}
            achievements = [
                {"ID": i, "Title": t, "Description": f"Do {t.lower()}", "Flags": 3, "Points": pts, "MemAddr": cond,
                 "Author": "Tester", "BadgeName": f"{i}", "BadgeURL": f"{self.url}/Badge/{i}.png",
                 "BadgeLockedURL": f"{self.url}/Badge/{i}_lock.png", "Created": 1700000000, "Modified": 1700000000,
                 "Type": None, "Rarity": 25.0, "RarityHardcore": 10.0}
                for i, t, pts, cond in self.achievements
            ]
            if self.warn_unknown_emulator:
                achievements.append(
                    {"ID": 101000001, "Title": "Warning: Unknown Emulator", "Flags": 3, "Points": 0, "MemAddr": "0=1",
                     "Description": "Hardcore unlocks cannot be earned using this emulator.", "Author": "",
                     "BadgeName": "00000", "BadgeURL": "", "BadgeLockedURL": "", "Created": 1700000000,
                     "Modified": 1700000000, "Type": None, "Rarity": 0, "RarityHardcore": 0})
            return 200, {"Success": True, "GameId": self.game_id, "Title": self.title, "ConsoleId": self.console,
                         "ImageIconUrl": f"{self.url}/Images/000001.png", "RichPresenceGameId": self.game_id,
                         "RichPresencePatch": self.rich_presence, "Sets": [{
                             "AchievementSetId": 1, "GameId": self.game_id, "Title": None, "Type": "core",
                             "ImageIconUrl": f"{self.url}/Images/000001.png",
                             "Achievements": achievements, "Leaderboards": []}]}
        if r == "allprogress":
            # As PHP sends an empty map: a list.
            if int(p["c"]) != self.console:
                return 200, {"Success": True, "Response": []}
            entry = {"Achievements": len(self.achievements)}
            if self.unlocked:
                entry["Unlocked"] = len(self.unlocked)
            return 200, {"Success": True, "Response": {str(self.game_id): entry}}
        if r == "unlocks":
            return 200, {"Success": True, "GameID": self.game_id, "HardcoreMode": p.get("h") == "1",
                         "UserUnlocks": [] if p.get("h") == "1" else sorted(self.unlocked)}
        if r == "startsession":
            return 200, {"Success": True, "Unlocks": [{"ID": i, "When": 1700000000} for i in sorted(self.unlocked)],
                         "HardcoreUnlocks": [], "ServerNow": 1700000000}
        if r == "awardachievement":
            if self.fail_awards:
                return 503, {"Success": False, "Error": "Service unavailable"}
            self.unlocked.add(int(p["a"]))
            remaining = len(self.achievements) - len(self.unlocked)
            return 200, {"Success": True, "Score": 0, "SoftcoreScore": 5, "AchievementID": int(p["a"]),
                         "AchievementsRemaining": remaining}
        if r == "ping":
            return 200, {"Success": True}
        return 400, {"Success": False, "Error": f"no such request: {r}"}

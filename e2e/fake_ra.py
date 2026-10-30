"""A stand-in for retroachievements.org's `dorequest.php`, the API emulators use, for romi to log in,
load a game's achievements and send unlocks to without an account. Point romi at it with
ROMI_RA_HOST. Answers are shaped like the real server's (as rcheevos' own tests have them)."""

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs

USER, PASSWORD, TOKEN = "Lantern", "hunter2", "T0KEN"


class FakeRetroAchievements:
    def __init__(self, achievements, game_id=1234, title="Pocket Lantern", console=7, known_hashes=None):
        # Achievements as (id, title, points, condition), conditions in RetroAchievements' syntax.
        self.achievements = achievements
        self.game_id, self.title, self.console = game_id, title, console
        # Hashes that are this game; None means any.
        self.known_hashes = known_hashes
        self.requests = []
        self.unlocked = set()
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
        if r == "login2":
            if p.get("u") == USER and (p.get("p") == PASSWORD or p.get("t") == TOKEN):
                return 200, {"Success": True, "User": USER, "Token": TOKEN, "Score": 0, "SoftcoreScore": 0,
                             "Messages": 0, "Permissions": 1, "AccountType": "Registered"}
            return 401, {"Success": False, "Error": "Invalid User/Password combination. Please try again",
                         "Code": "invalid_credentials"}
        if not signed_in:
            return 401, {"Success": False, "Error": "Invalid user/token", "Code": "invalid_credentials"}
        if r == "achievementsets":
            if self.known_hashes is not None and p.get("m") not in self.known_hashes:
                return 200, {"Success": True, "GameId": 0}
            achievements = [
                {"ID": i, "Title": t, "Description": f"Do {t.lower()}", "Flags": 3, "Points": pts, "MemAddr": cond,
                 "Author": "Tester", "BadgeName": "00001", "Created": 1700000000, "Modified": 1700000000}
                for i, t, pts, cond in self.achievements
            ]
            return 200, {"Success": True, "GameId": self.game_id, "Title": self.title, "ConsoleId": self.console,
                         "ImageIconUrl": f"{self.url}/Images/000001.png", "RichPresenceGameId": self.game_id,
                         "RichPresencePatch": "", "Sets": [{
                             "AchievementSetId": 1, "GameId": self.game_id, "Title": None, "Type": "core",
                             "ImageIconUrl": f"{self.url}/Images/000001.png",
                             "Achievements": achievements, "Leaderboards": []}]}
        if r == "startsession":
            return 200, {"Success": True, "Unlocks": [{"ID": i, "When": 1700000000} for i in sorted(self.unlocked)],
                         "HardcoreUnlocks": [], "ServerNow": 1700000000}
        if r == "awardachievement":
            self.unlocked.add(int(p["a"]))
            remaining = len(self.achievements) - len(self.unlocked)
            return 200, {"Success": True, "Score": 0, "SoftcoreScore": 5, "AchievementID": int(p["a"]),
                         "AchievementsRemaining": remaining}
        if r == "ping":
            return 200, {"Success": True}
        return 400, {"Success": False, "Error": f"no such request: {r}"}

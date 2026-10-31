"""A fake crates.io, npm, PyPI, Go proxy and OSV for test/deps.bats.

`jouet deps` sends every request to $JOUET_DEPS_API_URL/<service>/...,
so one server answers for all of them, from responses recorded from the
real services (trimmed) in this directory.

    python3 server.py ROUTES PORT_FILE LOG_FILE

ROUTES maps "GET /path?query" to a recorded file, or to {"status", "body"}.
Its "osv" table maps "ECOSYSTEM NAME VERSION" to the advisory IDs OSV's
querybatch answers with; GET /osv/v1/vulns/ID serves osv/ID.json. Anything
else is a 404. Each request is appended to LOG_FILE as one line, "METHOD
PATH", with a querybatch's queries after it, so a test can check exactly
what left the machine.
"""

import json
import os
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
ROUTES = json.load(open(sys.argv[1]))
PORT_FILE = sys.argv[2]
LOG_FILE = sys.argv[3]


def log(line):
    with open(LOG_FILE, "a") as f:
        f.write(line + "\n")


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, status, body):
        data = body.encode() if isinstance(body, str) else body
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        log("GET " + self.path)
        route = ROUTES.get("GET " + self.path)
        if route is None and self.path.startswith("/osv/v1/vulns/"):
            name = self.path.rsplit("/", 1)[1] + ".json"
            path = os.path.join(HERE, "osv", name)
            if os.path.isfile(path):
                route = "osv/" + name
        if route is None:
            return self.reply(404, '{"error": "not found"}')
        if isinstance(route, dict):
            body = route["body"]
            if not isinstance(body, str):
                body = json.dumps(body)
            return self.reply(route["status"], body)
        with open(os.path.join(HERE, route), "rb") as f:
            self.reply(200, f.read())

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = json.loads(self.rfile.read(length) or b"{}")
        if self.path != "/osv/v1/querybatch":
            log("POST " + self.path)
            return self.reply(404, '{"error": "not found"}')
        results = []
        asked = []
        for q in body.get("queries", []):
            key = "%s %s %s" % (q["package"]["ecosystem"], q["package"]["name"], q["version"])
            asked.append(key)
            ids = ROUTES.get("osv", {}).get(key, [])
            results.append({"vulns": [{"id": i} for i in ids]} if ids else {})
        log("POST " + self.path + " " + "; ".join(asked))
        self.reply(200, json.dumps({"results": results}))


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open(PORT_FILE + ".tmp", "w") as f:
    f.write(str(server.server_address[1]))
os.replace(PORT_FILE + ".tmp", PORT_FILE)
server.serve_forever()

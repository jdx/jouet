"""A fake GitHub for benchmark tasks, served from a task's `github.json`.

Both arms reach the same fake: `gh` through a wrapper that points it at
`github.localhost` with this server as its HTTP proxy, and `jouet` through
`JOUET_GITHUB_API_URL`. The fake serves:

- GraphQL at `/graphql`, executed by `graphql.Executor` against the
  fixture's `graphql` tree, with the review-thread mutations;
- REST: the fixture's `rest` map of path (with or without its query) to a
  JSON response, to text in `{"$text": ...}` or a zip archive of files in
  `{"$zip": {name: text}}`, with `"$status"` for an error; plus a pull request's
  review comments derived from its GraphQL threads, and replies to them;
- time: any object with `"$until": SECONDS` and `"$before": {...}` reads as
  its `$before` overlay until that many seconds after the server started,
  which is how checks stay pending and then fail.

Every request is logged with whether the fake could answer it, so a report
can tell a missing fixture from an agent's mistake. Requests it cannot
answer get a 404 like GitHub's.
"""

from __future__ import annotations

import copy
import io
import json
import threading
import time
import zipfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from . import graphql


def materialize(tree: Any, elapsed: float) -> Any:
    """`tree` as it reads `elapsed` seconds after the server started."""
    if isinstance(tree, dict):
        if "$until" in tree and elapsed < float(tree["$until"]):
            tree = {**tree, **tree.get("$before", {})}
        return {k: materialize(v, elapsed) for k, v in tree.items() if not k.startswith("$")}
    if isinstance(tree, list):
        return [materialize(v, elapsed) for v in tree]
    return tree


class World:
    """The fixture, the mutations made to it, and the request log."""

    def __init__(self, fixture: dict[str, Any], log_path: Path | None = None, offset_s: float = 0):
        self.fixture = copy.deepcopy(fixture)
        # A positive offset starts the clock that far in, past any `$until`.
        self.started = time.monotonic() - offset_s
        self.lock = threading.Lock()
        self.log_path = log_path
        self.log: list[dict[str, Any]] = []
        self.next_id = 900_000

    def elapsed(self) -> float:
        return time.monotonic() - self.started

    def record(self, entry: dict[str, Any]) -> None:
        entry["t"] = round(self.elapsed(), 3)
        self.log.append(entry)
        if self.log_path:
            with self.log_path.open("a") as f:
                f.write(json.dumps(entry) + "\n")

    def graphql(self, query: str, variables: dict[str, Any]) -> tuple[dict[str, Any], list[str]]:
        root = materialize(self.fixture.get("graphql", {}), self.elapsed())
        executor = graphql.Executor(root, self.mutations())
        response = executor.execute(query, variables)
        return response, executor.missing

    # Mutations change the fixture itself, so later requests see them.
    def mutations(self) -> dict[str, graphql.Mutation]:
        def thread(thread_id: str) -> dict[str, Any]:
            node = graphql.find_node(self.fixture.get("graphql", {}), thread_id)
            if node is None or node.get("__typename") != "PullRequestReviewThread":
                raise graphql.GraphQLError(
                    f"Could not resolve to a PullRequestReviewThread with the id of '{thread_id}'."
                )
            return node

        def set_resolved(value: bool):
            def run(args: dict[str, Any], _: graphql.Executor) -> dict[str, Any]:
                node = thread(args["input"]["threadId"])
                node["isResolved"] = value
                return {"thread": node}

            return run

        def reply(args: dict[str, Any], _: graphql.Executor) -> dict[str, Any]:
            data = args["input"]
            node = thread(data["pullRequestReviewThreadId"])
            return {"comment": self.add_reply(node, data["body"])}

        def add_comment(args: dict[str, Any], _: graphql.Executor) -> dict[str, Any]:
            data = args["input"]
            pr = graphql.find_node(self.fixture.get("graphql", {}), data["subjectId"])
            if pr is None:
                raise graphql.GraphQLError(f"Could not resolve to a node with the id of '{data['subjectId']}'.")
            comment = self.new_comment(data["body"], pr.get("number", 0))
            pr.setdefault("comments", {"nodes": []})["nodes"].append(comment)
            return {"commentEdge": {"node": comment}, "subject": pr}

        return {
            "resolveReviewThread": set_resolved(True),
            "unresolveReviewThread": set_resolved(False),
            "addPullRequestReviewThreadReply": reply,
            "addComment": add_comment,
        }

    # New comments carry every field GitHub's schema makes non-null, and the
    # IDs it fills in, so a later read finds what a real one would.
    def new_comment(self, body: str, number: int) -> dict[str, Any]:
        """A conversation comment on PR `number`, by the viewer."""
        self.next_id += 1
        viewer = self.fixture.get("graphql", {}).get("viewer", {})
        return {
            "__typename": "IssueComment",
            "id": f"IC_bench{self.next_id}",
            "databaseId": self.next_id,
            "fullDatabaseId": str(self.next_id),
            "body": body,
            "author": {
                "__typename": "User",
                "id": viewer.get("id", "U_bench"),
                "login": viewer.get("login", "bench"),
                "name": viewer.get("name"),
            },
            "authorAssociation": "OWNER",
            "createdAt": "2026-10-30T12:00:00Z",
            "updatedAt": "2026-10-30T12:00:00Z",
            "url": f"https://github.com/{self.fixture.get('repo', 'o/r')}/pull/{number}#issuecomment-{self.next_id}",
        }

    def add_reply(self, thread: dict[str, Any], body: str) -> dict[str, Any]:
        """A reply in `thread`: its first comment's position and commit,
        with the viewer's ID, body and URL."""
        number = next((n for n, t in self.threads() if t is thread), 0)
        first = (thread.get("comments", {}).get("nodes") or [{}])[0]
        own = self.new_comment(body, number)
        comment = {
            **copy.deepcopy(first),
            **own,
            "__typename": "PullRequestReviewComment",
            "id": f"PRRC_bench{self.next_id}",
            "path": first.get("path", thread.get("path")),
            "url": f"https://github.com/{self.fixture.get('repo', 'o/r')}/pull/{number}#discussion_r{self.next_id}",
            "replyTo": {"databaseId": first.get("databaseId")},
            "state": "SUBMITTED",
            "pullRequestReview": {"id": f"PRR_bench{self.next_id}"},
        }
        thread.setdefault("comments", {"nodes": []})["nodes"].append(comment)
        return comment

    def threads(self) -> list[dict[str, Any]]:
        out = []
        for pr in self.fixture.get("graphql", {}).get("repository", {}).get("pullRequests", {}).get("nodes", []):
            for t in pr.get("reviewThreads", {}).get("nodes", []):
                out.append((pr["number"], t))
        return out

    def rest(self, method: str, path: str, body: Any) -> tuple[int, Any]:
        """A REST response, or 404 when the fixture has none."""
        repo = self.fixture.get("repo", "")
        bare = urlsplit(path).path.rstrip("/")
        elapsed = self.elapsed()
        if method == "GET":
            for key in (path, bare):
                if key in self.fixture.get("rest", {}):
                    return self.canned(self.fixture["rest"][key], elapsed)
            prefix = f"/repos/{repo}/pulls/"
            if bare.startswith(prefix) and bare.endswith("/comments"):
                number = bare[len(prefix) : -len("/comments")]
                if number.isdigit():
                    return 200, self.review_comments(int(number))
            if bare == "/user":
                login = self.fixture.get("graphql", {}).get("viewer", {}).get("login", "bench")
                return 200, {"login": login, "id": 1, "type": "User"}
            if bare == "/rate_limit":
                return 200, {"resources": {"core": {"limit": 5000, "remaining": 4999, "reset": 0}}}
        prefix = f"/repos/{repo}/pulls/"
        if method == "PATCH" and bare.startswith(f"{prefix}comments/"):
            found = self.find_comment(bare.split("/comments/")[1])
            if found:
                c, t = found
                c["body"] = (body or {}).get("body", c.get("body"))
                return 200, self.rest_comment(c, t)
        if method == "POST":
            # A reply, at `comments/ID/replies` or as `in_reply_to` on
            # `PR/comments`.
            comment_id = None
            if bare.startswith(prefix) and "/comments/" in bare and bare.endswith("/replies"):
                comment_id = bare.split("/comments/")[1].split("/")[0]
            elif bare.startswith(prefix) and bare.endswith("/comments") and (body or {}).get("in_reply_to"):
                comment_id = str(body["in_reply_to"])
            found = self.find_comment(comment_id) if comment_id else None
            if found:
                reply = self.add_reply(found[1], (body or {}).get("body", ""))
                return 201, self.rest_comment(reply, found[1])
            if bare.startswith(f"/repos/{repo}/issues/") and bare.endswith("/comments"):
                number = int(bare.split("/issues/")[1].split("/")[0])
                for pr in (
                    self.fixture.get("graphql", {}).get("repository", {}).get("pullRequests", {}).get("nodes", [])
                ):
                    if pr.get("number") == number:
                        comment = self.new_comment((body or {}).get("body", ""), number)
                        pr.setdefault("comments", {"nodes": []})["nodes"].append(comment)
                        return 201, {"id": comment["databaseId"], "body": comment["body"]}
        return 404, {"message": "Not Found", "documentation_url": "https://docs.github.com/rest"}

    def find_comment(self, database_id: str) -> tuple[dict[str, Any], dict[str, Any]] | None:
        """A review comment by its database ID, and its thread."""
        for _, t in self.threads():
            for c in t.get("comments", {}).get("nodes", []):
                if str(c.get("databaseId")) == database_id:
                    return c, t
        return None

    @staticmethod
    def canned(raw: Any, elapsed: float) -> tuple[int, Any]:
        """A fixture response: JSON, or text from `$text`, with `$status`."""
        if not isinstance(raw, dict):
            return 200, materialize(raw, elapsed)
        now = {**raw, **raw.get("$before", {})} if "$until" in raw and elapsed < float(raw["$until"]) else raw
        status = int(now.get("$status", 200))
        if status >= 400:
            return status, {"message": "Not Found" if status == 404 else "Error", "status": str(status)}
        if "$text" in now:
            return status, now["$text"]
        if "$zip" in now:
            return status, zipped(now["$zip"])
        return status, materialize(raw, elapsed)

    def review_comments(self, number: int) -> list[dict[str, Any]]:
        out = []
        for n, t in self.threads():
            if n == number:
                out.extend(self.rest_comment(c, t) for c in t.get("comments", {}).get("nodes", []))
        return out

    def rest_comment(self, c: dict[str, Any], thread: dict[str, Any]) -> dict[str, Any]:
        first = (thread.get("comments", {}).get("nodes") or [{}])[0]
        out = {
            "id": c.get("databaseId"),
            "node_id": c.get("id"),
            "body": c.get("body"),
            "path": c.get("path", thread.get("path")),
            "line": thread.get("line"),
            "user": {"login": (c.get("author") or {}).get("login")},
            "created_at": c.get("createdAt"),
            "html_url": c.get("url"),
        }
        if c is not first:
            out["in_reply_to_id"] = first.get("databaseId")
        return out

    def summary(self) -> dict[str, Any]:
        """What a run's report keeps: request counts and the writes made."""
        unanswered = [e for e in self.log if e.get("status", 200) >= 400 or e.get("missing")]
        return {
            "requests": len(self.log),
            "graphql": sum(1 for e in self.log if e["path"] == "/graphql"),
            "rest": sum(1 for e in self.log if e["path"] != "/graphql"),
            "unanswered": len(unanswered),
            "unanswered_examples": sorted({describe(e) for e in unanswered})[:10],
        }


def zipped(files: dict[str, str]) -> bytes:
    """A zip archive of `files`, as GitHub serves a run's logs."""
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        for name, text in files.items():
            z.writestr(name, text)
    return buf.getvalue()


def describe(entry: dict[str, Any]) -> str:
    if entry.get("missing"):
        return f"graphql field {entry['missing'][0]}"
    return f"{entry['method']} {entry['path']} -> {entry.get('status')}"


class Handler(BaseHTTPRequestHandler):
    world: World  # set on the subclass `serve` makes

    def log_message(self, format: str, *args: Any) -> None:  # noqa: A002
        pass

    def path_only(self) -> str:
        # As an HTTP proxy, `gh` sends absolute URIs: http://api.github.localhost/x.
        parts = urlsplit(self.path)
        path = parts.path if parts.scheme else self.path.split("?")[0]
        query = parts.query if parts.scheme else urlsplit(self.path).query
        # GitHub Enterprise-style prefixes, in case a client adds one.
        for prefix in ("/api/v3", "/api"):
            if path.startswith(prefix + "/"):
                path = path[len(prefix) :]
        return path + (f"?{query}" if query else "")

    def body(self) -> Any:
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b""
        try:
            return json.loads(raw) if raw else None
        except ValueError:
            return None

    def reply(self, status: int, payload: Any) -> None:
        if isinstance(payload, bytes):
            data, kind = payload, "application/zip"
        elif isinstance(payload, str):
            data, kind = payload.encode(), "text/plain; charset=utf-8"
        else:
            data, kind = json.dumps(payload).encode(), "application/json; charset=utf-8"
        self.send_response(status)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(data)))
        # The server speaks HTTP/1.0 and closes every connection after its
        # response. Saying so keeps a client from reusing one from its pool
        # and racing the close ("Peer disconnected").
        self.send_header("Connection", "close")
        self.send_header("X-RateLimit-Limit", "5000")
        self.send_header("X-RateLimit-Remaining", "4999")
        self.end_headers()
        self.wfile.write(data)

    def handle_any(self) -> None:
        path = self.path_only()
        body = self.body()
        world = self.world
        with world.lock:
            if path.split("?")[0] == "/graphql" and self.command == "POST":
                query = (body or {}).get("query", "")
                response, missing = world.graphql(query, (body or {}).get("variables") or {})
                status = 200
                entry = {"method": "POST", "path": "/graphql", "status": status, "query": query[:2000]}
                if missing:
                    entry["missing"] = missing
                if "errors" in response:
                    entry["errors"] = [e["message"] for e in response["errors"]]
            else:
                status, response = world.rest(self.command, path, body)
                entry = {"method": self.command, "path": path, "status": status}
            world.record(entry)
        self.reply(status, response)

    do_GET = do_POST = do_PATCH = do_PUT = do_DELETE = handle_any


class FakeGitHub:
    """The server, on a free localhost port, in a background thread."""

    def __init__(self, fixture: dict[str, Any], log_path: Path | None = None, offset_s: float = 0):
        self.world = World(fixture, log_path, offset_s)
        handler = type("BoundHandler", (Handler,), {"world": self.world})
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self) -> FakeGitHub:
        self.thread.start()
        return self

    def __exit__(self, *_: Any) -> None:
        self.server.shutdown()
        self.server.server_close()

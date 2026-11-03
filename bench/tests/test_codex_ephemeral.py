import json
import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from jbench.harness import Codex


WORKER = Path(__file__).resolve().parents[1] / "jbench/codex_ephemeral.py"


class CodexEphemeralTest(unittest.TestCase):
    def test_command_preserves_a_prompt_that_starts_with_a_dash(self):
        cmd = Codex().command("codex", "--task", {"codex_model": "test"}, Path("/tmp"))
        self.assertEqual(cmd[-2:], ["--", "--task"])
        self.assertNotIn("--dangerously-bypass-hook-trust", cmd)

    def test_live_usage_and_completion_from_ephemeral_turn(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / "codex"
            binary.write_text('''#!/usr/bin/env python3
import json, os, sys
def recv(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value), flush=True)
assert recv()["method"] == "initialize"
send({"id": 1, "result": {}})
assert recv()["method"] == "initialized"
start = recv()
assert start["method"] == "thread/start" and start["params"]["ephemeral"] is True
send({"id": 2, "result": {"thread": {"id": "thread-1", "ephemeral": not bool(os.getenv("TEST_NON_EPHEMERAL"))}}})
turn = recv()
assert turn["method"] == "turn/start" and turn["params"]["input"][0]["text"] == "do work"
send({"id": 3, "result": {}})
send({"method": "thread/tokenUsage/updated", "params": {"threadId": "thread-1", "tokenUsage": {
    "total": {"inputTokens": None if os.getenv("TEST_BAD_USAGE") else 100,
              "cachedInputTokens": 80, "outputTokens": 10}}}})
send({"method": "item/completed", "params": {"threadId": "thread-1", "item": {
    "type": "agentMessage", "text": "done"}}})
send({"method": "turn/completed", "params": {"threadId": "thread-1", "turn": {"status": "completed"}}})
''')
            binary.chmod(0o755)
            result = subprocess.run([sys.executable, str(WORKER), "--codex", str(binary),
                                     "--cwd", directory, "--model", "test", "do work"],
                                    capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            events = [json.loads(line) for line in result.stdout.splitlines()]
            self.assertEqual([event["type"] for event in events],
                             ["jouet.ephemeral", "thread.started", "jouet.token_usage", "item.completed", "turn.completed"])
            self.assertEqual(events[2]["usage"]["inputTokens"], 100)
            self.assertEqual(events[-1]["usage"]["cached_input_tokens"], 80)
            self.assertEqual(events[3]["item"]["type"], "agent_message")
            lines = result.stdout.splitlines()
            harness = Codex()
            self.assertEqual(harness.tokens_so_far(lines[:1], Path(directory)), 0)
            self.assertEqual(harness.tokens_so_far(lines[:3], Path(directory)), 110)
            self.assertEqual(harness.usage(lines, Path(directory)).cached_input_tokens, 80)
            interrupted = harness.usage(lines[:3], Path(directory))
            self.assertEqual((interrupted.input_tokens, interrupted.output_tokens), (100, 10))
            self.assertTrue(interrupted.is_error)
            self.assertFalse(harness.usage(lines[:1], Path(directory)).extra["usage_observed"])
            for key in ("TEST_NON_EPHEMERAL", "TEST_BAD_USAGE"):
                with self.subTest(key=key):
                    bad = subprocess.run([sys.executable, str(WORKER), "--codex", str(binary),
                                          "--cwd", directory, "--model", "test", "do work"],
                                         env={**os.environ, key: "1"}, capture_output=True, text=True, timeout=10)
                    self.assertNotEqual(bad.returncode, 0)
                    self.assertNotIn('"type": "turn.completed"', bad.stdout)

    @unittest.skipUnless(shutil.which("codex"), "Codex is not installed")
    def test_real_ephemeral_codex_does_not_persist_prompt_or_tool_output(self):
        prompt = "bench-prompt-canary-945218"
        tool_output = "bench-tool-canary-390147"
        requests = []

        class Model(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                length = int(self.headers["Content-Length"])
                requests.append(json.loads(self.rfile.read(length)))
                if len(requests) % 2 == 1:
                    value = tool_output if len(requests) == 1 else "bench-second-tool"
                    item = {"id": "call_item", "type": "custom_tool_call", "call_id": "call_123",
                            "namespace": "functions", "name": "exec", "status": "completed",
                            "input": f'text("{value}")'}
                else:
                    item = {"id": "msg", "type": "message", "status": "completed", "role": "assistant",
                            "content": [{"type": "output_text", "text": "done", "annotations": []}]}
                response = {"id": f"resp_{len(requests)}", "object": "response", "created_at": 1,
                            "status": "completed", "model": "gpt-6-sol", "output": [item],
                            "usage": {"input_tokens": 12, "input_tokens_details": {"cached_tokens": 2},
                                      "output_tokens": 3, "output_tokens_details": {"reasoning_tokens": 0},
                                      "total_tokens": 15}}
                events = [{"type": "response.created", "response": {**response, "status": "in_progress",
                                                                       "output": [], "usage": None}},
                          {"type": "response.output_item.done", "output_index": 0, "item": item},
                          {"type": "response.completed", "response": response}]
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                for event in events:
                    self.wfile.write(f"event: {event['type']}\ndata: {json.dumps(event)}\n\n".encode())

        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            config = home / ".codex"
            config.mkdir()
            server = ThreadingHTTPServer(("127.0.0.1", 0), Model)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                (config / "config.toml").write_text(
                    'model_provider = "mock"\n[model_providers.mock]\nname = "mock"\n'
                    f'base_url = "http://127.0.0.1:{server.server_port}/v1"\n'
                    'wire_api = "responses"\nrequires_openai_auth = false\nsupports_websockets = false\n'
                )
                env = {"HOME": str(home), "CODEX_HOME": str(config), "PATH": os.environ["PATH"]}
                result = subprocess.run([sys.executable, str(WORKER), "--codex", shutil.which("codex"),
                                         "--cwd", directory, "--model", "gpt-6-sol", prompt],
                                        env=env, capture_output=True, text=True, timeout=30)
                second = subprocess.run([sys.executable, str(WORKER), "--codex", shutil.which("codex"),
                                         "--cwd", directory, "--model", "gpt-6-sol", "second task"],
                                        env=env, capture_output=True, text=True, timeout=30)
            finally:
                server.shutdown()
                server.server_close()
                thread.join()
            self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
            self.assertEqual(second.returncode, 0, second.stderr + second.stdout)
            self.assertEqual(len(requests), 4)
            self.assertIn(tool_output, json.dumps(requests[1]["input"]))
            self.assertNotIn(prompt, json.dumps(requests[2]["input"]))
            self.assertNotIn(tool_output, json.dumps(requests[2]["input"]))
            events = [json.loads(line) for line in result.stdout.splitlines()]
            self.assertEqual(events[-1]["type"], "turn.completed")
            self.assertEqual(events[-1]["usage"]["input_tokens"], 24)
            for path in home.rglob("*"):
                if path.is_file():
                    data = path.read_bytes()
                    self.assertNotIn(prompt.encode(), data, str(path))
                    self.assertNotIn(tool_output.encode(), data, str(path))
            db = config / "state_5.sqlite"
            if db.exists():
                with sqlite3.connect(db) as connection:
                    self.assertEqual(connection.execute("select count(*) from threads").fetchone()[0], 0)

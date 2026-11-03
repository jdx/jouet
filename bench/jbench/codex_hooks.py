"""Ask Codex which hooks it will load without changing its trust decisions."""

from __future__ import annotations

import json
import queue
import subprocess
import threading
import time
import tomllib
from pathlib import Path
from typing import Any


class HookPreflightError(RuntimeError):
    pass


def list_hooks(binary: str, env: dict[str, str], cwd: Path, timeout: float = 15) -> dict[str, Any]:
    """Query the experimental app-server hooks/list API; fail closed on errors."""
    messages: queue.Queue[str | None] = queue.Queue()
    with subprocess.Popen([binary, "app-server", "--listen", "stdio://",
                           "-c", 'cli_auth_credentials_store="file"'], cwd=cwd, env=env,
                          stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                          text=True) as proc:
        assert proc.stdin is not None and proc.stdout is not None

        def read() -> None:
            for line in proc.stdout:
                messages.put(line)
            messages.put(None)

        thread = threading.Thread(target=read, daemon=True)
        thread.start()

        def request(message: dict[str, Any]) -> None:
            proc.stdin.write(json.dumps(message) + "\n")
            proc.stdin.flush()

        def response(request_id: int) -> dict[str, Any]:
            deadline = time.monotonic() + timeout
            while True:
                try:
                    line = messages.get(timeout=max(0, deadline - time.monotonic()))
                except queue.Empty as error:
                    raise HookPreflightError("Codex hook preflight timed out") from error
                if line is None:
                    raise HookPreflightError("Codex hook preflight ended before replying")
                try:
                    message = json.loads(line)
                except json.JSONDecodeError as error:
                    raise HookPreflightError("Codex returned invalid hook preflight output") from error
                if message.get("id") == request_id:
                    if "error" in message or not isinstance(message.get("result"), dict):
                        raise HookPreflightError("Codex rejected the hook preflight")
                    return message["result"]

        try:
            request({"id": 1, "method": "initialize", "params": {
                "clientInfo": {"name": "jouet-bench", "version": "1"},
                "capabilities": {"experimentalApi": True},
            }})
            response(1)
            request({"method": "initialized", "params": {}})
            request({"id": 2, "method": "hooks/list", "params": {"cwds": [str(cwd)]}})
            return response(2)
        except (OSError, BrokenPipeError) as error:
            raise HookPreflightError("Could not query Codex hooks") from error
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()


def copy_trust(reviewed: Path, run: Path) -> None:
    """Copy the reviewed home's trust in its own hooks to the run's hooks.

    Only entries for the reviewed `hooks.json` are copied, under the run's
    path; a review started elsewhere also trusts that directory's hooks, and
    those are not the benchmark's.
    """
    source = str(reviewed / "hooks.json") + ":"
    try:
        reviewed_config = reviewed / "config.toml"
        reviewed_text = reviewed_config.read_text() if reviewed_config.exists() else ""
        state = tomllib.loads(reviewed_text).get("hooks", {}).get("state", {})
        config = run / "config.toml"
        existing = tomllib.loads(config.read_text()) if config.exists() else {}
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise HookPreflightError("Cannot read the reviewed Codex trust") from error
    if "hooks" in existing:
        raise HookPreflightError("The run's Codex configuration already has hook settings")
    entries = [(key[len(source):], value["trusted_hash"]) for key, value in state.items()
               if key.startswith(source) and isinstance(value, dict) and isinstance(value.get("trusted_hash"), str)]
    if not entries:
        raise HookPreflightError("Codex hooks in the benchmark home have not been reviewed")
    target = str(run / "hooks.json")
    with config.open("a") as file:
        for suffix, digest in sorted(entries):
            file.write(f"\n[hooks.state.{json.dumps(f'{target}:{suffix}')}]\ntrusted_hash = {json.dumps(digest)}\n")


def require_reviewed(result: dict[str, Any], hook_file: Path) -> None:
    """Require the enabled jouet hooks to be trusted and reject extra hooks."""
    data = result.get("data")
    if not isinstance(data, list) or len(data) != 1 or not isinstance(data[0], dict):
        raise HookPreflightError("Codex returned no usable hook inventory")
    entry = data[0]
    if entry.get("errors") or entry.get("warnings") or not isinstance(entry.get("hooks"), list):
        raise HookPreflightError("Codex reported hook configuration errors or warnings")
    hooks = entry["hooks"]
    expected = {"sessionStart", "preToolUse", "postToolUse", "stop"}
    seen: set[str] = set()
    for hook in hooks:
        if not isinstance(hook, dict):
            raise HookPreflightError("Codex returned an invalid hook")
        if not hook.get("enabled"):
            continue
        if hook.get("sourcePath") != str(hook_file) or hook.get("trustStatus") != "trusted":
            raise HookPreflightError("Codex has unreviewed or unexpected enabled hooks")
        event = hook.get("eventName")
        if event not in expected or event in seen:
            raise HookPreflightError("Codex has unexpected or duplicate benchmark hooks")
        seen.add(event)
    if seen != expected:
        raise HookPreflightError("Codex is missing an enabled reviewed benchmark hook")

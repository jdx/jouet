"""Run one ephemeral Codex turn and expose live usage on a JSONL stream."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from typing import Any


def emit(event: dict[str, Any]) -> None:
    print(json.dumps(event), flush=True)


def run(binary: str, cwd: str, model: str, effort: str | None, prompt: str) -> int:
    """Run the app server in this process group so the runner can stop it."""
    emit({"type": "jouet.ephemeral", "model": model})
    with subprocess.Popen([binary, "app-server", "--listen", "stdio://",
                           "-c", 'cli_auth_credentials_store="file"'],
                          stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True) as proc:
        assert proc.stdin is not None and proc.stdout is not None

        def send(message: dict[str, Any]) -> None:
            proc.stdin.write(json.dumps(message) + "\n")
            proc.stdin.flush()

        send({"id": 1, "method": "initialize", "params": {
            "clientInfo": {"name": "jouet-bench", "version": "1"},
            "capabilities": {"experimentalApi": True},
        }})
        thread_id = None
        usage = None
        try:
            for line in proc.stdout:
                message = json.loads(line)
                request_id = message.get("id")
                method = message.get("method")
                params = message.get("params") or {}
                if request_id is not None and "error" in message:
                    emit({"type": "error", "message": "Codex app-server rejected a request"})
                    return 1
                if request_id == 1:
                    send({"method": "initialized", "params": {}})
                    send({"id": 2, "method": "thread/start", "params": {
                        "cwd": cwd, "model": model, "ephemeral": True,
                        "approvalPolicy": "never", "sandbox": "danger-full-access",
                    }})
                elif request_id == 2:
                    thread = message.get("result", {}).get("thread", {})
                    thread_id = thread.get("id")
                    if not thread_id or thread.get("ephemeral") is not True:
                        emit({"type": "error", "message": "Codex did not start an ephemeral thread"})
                        return 1
                    emit({"type": "thread.started", "thread_id": thread_id,
                          "model": message.get("result", {}).get("model")})
                    turn: dict[str, Any] = {"threadId": thread_id, "input": [{"type": "text", "text": prompt}]}
                    if effort:
                        turn["effort"] = effort
                    send({"id": 3, "method": "turn/start", "params": turn})
                elif method == "thread/tokenUsage/updated" and params.get("threadId") == thread_id:
                    usage = params.get("tokenUsage", {}).get("total")
                    if (not isinstance(usage, dict)
                            or any(type(usage.get(key)) is not int or usage[key] < 0
                                   for key in ("inputTokens", "cachedInputTokens", "outputTokens"))
                            or usage["cachedInputTokens"] > usage["inputTokens"]):
                        emit({"type": "error", "message": "Codex returned invalid token usage"})
                        return 1
                    emit({"type": "jouet.token_usage", "usage": usage})
                elif method in ("item/started", "item/completed") and params.get("threadId") == thread_id:
                    item = dict(params.get("item") or {})
                    if item.get("type") == "agentMessage":
                        item["type"] = "agent_message"
                    emit({"type": method.replace("/", "."), "item": item})
                elif method == "turn/completed" and params.get("threadId") == thread_id:
                    if params.get("turn", {}).get("status") != "completed" or usage is None:
                        error = params.get("turn", {}).get("error") or {}
                        detail = error.get("message") if isinstance(error, dict) else None
                        emit({"type": "turn.failed", "error": {"message": detail or "Codex turn failed or lacked usage"}})
                        return 1
                    emit({"type": "turn.completed", "usage": {
                        "input_tokens": usage["inputTokens"],
                        "cached_input_tokens": usage["cachedInputTokens"],
                        "output_tokens": usage["outputTokens"],
                        "cache_write_input_tokens": usage.get("cacheWriteInputTokens", 0),
                        "reasoning_output_tokens": usage.get("reasoningOutputTokens"),
                    }})
                    return 0
                elif request_id is not None and method:
                    emit({"type": "error", "message": "Codex requested unsupported interaction"})
                    return 1
            emit({"type": "error", "message": "Codex app-server exited before the turn finished"})
            return 1
        except (BrokenPipeError, KeyError, TypeError, ValueError):
            emit({"type": "error", "message": "Codex app-server returned invalid output"})
            return 1
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--codex", required=True)
    parser.add_argument("--cwd", required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--effort")
    parser.add_argument("prompt")
    args = parser.parse_args()
    return run(args.codex, args.cwd, args.model, args.effort, args.prompt)


if __name__ == "__main__":
    sys.exit(main())
